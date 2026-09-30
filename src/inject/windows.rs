//! Win32-OutputSink: Message-only-Fenster als Clipboard-Owner + `SendInput`
//! (Spec §7, windows-plan WP3).
//!
//! Zwei Eigenheiten von Windows prägen den ganzen Modul-Aufbau:
//!
//! 1. **Delayed Rendering statt Selection-Ownership.** Diktier legt kein
//!    Transkript ins Clipboard, sondern ein Versprechen
//!    (`SetClipboardData(CF_UNICODETEXT, NULL)`). Erst wenn jemand einfügt,
//!    schickt Windows `WM_RENDERFORMAT` — genau das ist der „bediente Read“
//!    aus §7.1 Punkt 7. Dafür braucht es ein Fenster und eine Message-Pump,
//!    die **dauerhaft** läuft (auch im Idle, `serve_for(10 ms)` im
//!    Inject-Worker), sonst hängen Win+V und Clipboard-Manager.
//! 2. **Die eigene Generation.** `GetClipboardSequenceNumber()` steigt auch
//!    durch den eigenen Render. „Sequenz unverändert“ wäre deshalb nach jedem
//!    erfolgreichen Paste falsch; der Sink führt `expected_seq` und schreibt
//!    es nach **jeder eigenen** Mutation fort (windows-plan
//!    Leitentscheidung 4).
//!
//! `AttachThreadInput` wird nicht verwendet, `HWND`s werden nur als opake
//! [`WindowId`] weitergereicht (Leitentscheidung 2).
//!
//! Seit Spec v1.8 (§7.1.1) sichert der Snapshot **alle** Win32-auslesbaren
//! Formate nach der Matrix in [`super::formats`] und schreibt sie beim Restore
//! eager zurück; Delayed Rendering gibt es nur noch für das Transkript.
//!
//! **Wo `WM_RENDERFORMAT` noch gebraucht wird** (Sol-Impl-Review Blocker 2):
//! nur auf dem Paste-Pfad, von `become_owner` bis zur Restore-Entscheidung —
//! dort ist der bediente Read die Heuristik aus §7.1 Punkt 7. Alle anderen
//! Pfade setzen das Transkript eager: `copy_only`/`copy_transcript` sofort,
//! `NoReadTimeout`/`NoPromise`/`Disabled` und der Fokuswechsel vor dem Chord
//! über `materialize_transcript`, der Restore ohnehin. Bleibt ein Versprechen
//! trotzdem offen (Materialisierung scheiterte, Clipboard blockiert), holt es
//! der Idle-Retry des Inject-Workers nach (`OutputSink::materialize_pending`,
//! Final-Review Blocker 2); danach sind `WM_RENDERALLFORMATS` und der
//! Quit-Pfad (`save_to_clipboard_manager`, `Drop`) die letzten Chancen.

use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr;
use std::rc::Rc;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_CLASS_ALREADY_EXISTS, ERROR_SUCCESS, GetLastError, GlobalFree, HANDLE,
    HGLOBAL, HINSTANCE, HWND, LPARAM, LRESULT, SetLastError, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    DeleteEnhMetaFile, GetEnhMetaFileBits, HENHMETAFILE, SetEnhMetaFileBits,
};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, CountClipboardFormats, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardFormatNameW, GetClipboardOwner, GetClipboardSequenceNumber,
    IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, SendInput, VK_CONTROL, VK_INSERT, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA,
    GetForegroundWindow, GetWindowLongPtrW, GetWindowThreadProcessId, HWND_MESSAGE, MSG,
    MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT, RegisterClassW,
    SetWindowLongPtrW, UnregisterClassW, WM_DESTROYCLIPBOARD, WM_NCCREATE, WM_NCDESTROY,
    WM_RENDERALLFORMATS, WM_RENDERFORMAT, WNDCLASSW,
};

use crate::config::OutputConfig;

use super::formats::{
    self, DataKind, EXCLUDE_FROM_MONITOR, Enumerated, FIRST_REGISTERED, FormatRef, FormatRow,
    LossReason, LostFormat, MAX_SNAPSHOT_BYTES, MAX_SNAPSHOT_TIME, Phase, RowOutcome, SnapshotKind,
    SnapshotReport,
};
use super::protocol::{
    ClipboardHost, ClipboardSnapshot, ModifierState, PumpEvents, RestoreResult,
    apply_leading_space, inject_paste, save_transcript_on_quit, serve_restored_until_read,
};
use super::{
    CaptureContext, ClipboardSave, Copied, InjectError, InjectOutcome, OutputSink, PasteKey,
    TranscriptState, WindowId,
};

/// `CF_UNICODETEXT` liegt in windows-sys 0.61 unter `Win32_System_Ole` — ein
/// COM-Feature, von dem hier sonst nichts gebraucht wird. Der Wert ist seit
/// Windows NT 3.1 Teil der stabilen ABI (`winuser.h`), die Konstante direkt zu
/// setzen ist billiger als ein zusätzlicher Feature-Baum.
const CF_UNICODETEXT: u32 = formats::CF_UNICODETEXT;

/// Puffer für `GetClipboardFormatNameW` (Zeichen). Registrierte Namen sind
/// kurz; längere werden abgeschnitten und ohnehin auf 40 Zeichen bereinigt.
const FORMAT_NAME_CHARS: usize = 256;

/// Text, den `--clipboard-check --roundtrip` kurzzeitig einsetzt.
const ROUNDTRIP_TEXT: &str = "diktier --clipboard-check --roundtrip";

/// `'V'` — Windows führt Buchstaben-VKs auf dem Großbuchstaben (`VK_V` gibt es
/// als benannte Konstante nicht).
const VK_V: u16 = 0x56;

/// Fensterklasse des Clipboard-Owners. Prozessweit eindeutig; das Fenster
/// selbst ist `HWND_MESSAGE` und damit unsichtbar und ohne Taskbar-Eintrag.
const CLASS_NAME: &str = "DiktierClipboardOwner";

/// `OpenClipboard` scheitert kurzzeitig, wenn ein Clipboard-Manager oder die
/// Zielanwendung gerade offen hat. Zehn Versuche à 10 ms sind reichlich und
/// bleiben weit unter dem 5-s-Read-Fenster aus §7.1 Punkt 7.
const OPEN_RETRIES: u32 = 10;
const OPEN_RETRY_WAIT: Duration = Duration::from_millis(10);

/// Obergrenze je Pump-Durchlauf. Nachrichten, die nicht mehr hineinpassen,
/// bleiben in der Queue und kommen beim nächsten Aufruf dran — die Schleife
/// kann so nicht endlos drehen, wenn jemand Nachrichten schneller schickt, als
/// sie verarbeitet werden.
const MAX_MESSAGES_PER_PUMP: u32 = 256;

/// Was `SetClipboardData` nach dem `EmptyClipboard` bekommt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fill {
    /// `NULL` — Delayed Rendering, der Text kommt erst bei `WM_RENDERFORMAT`.
    Delayed,
    /// Echte Daten. Nach einem Render ist das der einzig wirksame Weg, den
    /// Clipboard-Inhalt noch zu ändern (Quit-Pfad).
    Eager,
}

/// Gesicherte Rohdaten eines Formats. `Rc`, damit eigene Payload, Stash und
/// Restore dieselben Bytes teilen, statt sie zu kopieren (Speicherspitze,
/// Leitentscheidung 2).
#[derive(Debug, Clone)]
struct RawFormat {
    format: FormatRef,
    kind: DataKind,
    useful: bool,
    data: Rc<Vec<u8>>,
}

impl RawFormat {
    fn row(&self) -> FormatRow {
        FormatRow {
            format: self.format.clone(),
            outcome: RowOutcome::Saved {
                bytes: self.data.len(),
                useful: self.useful,
            },
        }
    }
}

/// Leitentscheidung 6: die **zuletzt tatsächlich gesetzte** eigene Payload.
#[derive(Debug, Clone)]
enum Payload {
    /// Das Transkript in `ClipboardState::serve` (delayed oder eager).
    Transcript,
    /// Nach `Restored`/`RestoredPartial`: genau die platzierten Formate.
    Formats(Vec<RawFormat>),
    /// Nach dem Restore eines leeren Snapshots — oder nichts Eigenes.
    Empty,
}

// ------------------------------------------------------------------ RAII

/// Ein noch nicht an das Clipboard übergebenes `HGLOBAL`. `Drop` gibt es frei;
/// nach erfolgreichem `SetClipboardData` gehört es dem System
/// ([`OwnedGlobal::release`]).
struct OwnedGlobal(HGLOBAL);

impl OwnedGlobal {
    fn release(mut self) {
        self.0 = ptr::null_mut();
    }
}

impl Drop for OwnedGlobal {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: Das Handle stammt aus `GlobalAlloc`, ist nicht gesperrt
            // und wurde nie übergeben (sonst hätte `release` es genullt).
            unsafe { GlobalFree(self.0) };
        }
    }
}

/// Ein noch nicht übergebenes EMF-Handle aus `SetEnhMetaFileBits`. Nach
/// **fehlgeschlagenem** `SetClipboardData` `DeleteEnhMetaFile`, nach Erfolg
/// nie (Sol-Review H2).
struct OwnedEmf(HENHMETAFILE);

impl OwnedEmf {
    fn release(mut self) {
        self.0 = ptr::null_mut();
    }
}

impl Drop for OwnedEmf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: eigenes Handle aus `SetEnhMetaFileBits`, nie übergeben.
            unsafe { DeleteEnhMetaFile(self.0) };
        }
    }
}

/// Vorab erzeugtes Handle für ein Format (Leitentscheidung 5.1).
enum Prepared {
    Global(OwnedGlobal),
    Emf(OwnedEmf),
}

impl Prepared {
    fn raw(&self) -> HANDLE {
        match self {
            Self::Global(handle) => handle.0 as HANDLE,
            Self::Emf(handle) => handle.0 as HANDLE,
        }
    }

    fn release(self) {
        match self {
            Self::Global(handle) => handle.release(),
            Self::Emf(handle) => handle.release(),
        }
    }
}

/// `SetClipboardData` bei geöffnetem Clipboard. Bei Erfolg geht das Handle
/// ans System, sonst gibt `Drop` es frei; der Fehlercode wird **vorher**
/// gesichert (`GlobalFree` überschreibt ihn).
fn place(id: u32, handle: Prepared) -> Result<(), u32> {
    // SAFETY: Das Clipboard ist von unserem Fenster geöffnet und nach
    // `EmptyClipboard` unseres; `handle` ist ein gültiges, eigenes
    // `GMEM_MOVEABLE`- bzw. EMF-Handle passend zur Handle-Klasse von `id`.
    let placed = unsafe { SetClipboardData(id, handle.raw()) };
    if placed.is_null() {
        // SAFETY: parameterlos, direkt nach dem gescheiterten Aufruf.
        let err = unsafe { GetLastError() };
        drop(handle);
        return Err(err);
    }
    handle.release();
    Ok(())
}

/// Warum eine Übernahme unterblieb. Die Unterscheidung ist nötig, weil die
/// Aufrufer verschieden reagieren: ein fremder Copy ist kein Fehler, sondern
/// §7.1 Punkt 5 („niemals restaurieren“), ein Win32-Fehler dagegen schon —
/// und ob er schon etwas verändert hat, entscheidet über das Transkript
/// (Final-Review Blocker 1).
enum TakeFailure {
    /// Fremde Änderung zwischen Prüfung/Snapshot und `EmptyClipboard` (das
    /// Clipboard wurde nicht angefasst) oder direkt nach dem eigenen
    /// `CloseClipboard` (fremder Inhalt liegt).
    Foreign,
    /// Nichts angefasst (`GlobalAlloc`, `OpenClipboard`, `EmptyClipboard` ohne
    /// Sequenzänderung). Bisheriger Inhalt, Eigentum und ein offenes
    /// Versprechen bleiben.
    Untouched(InjectError),
    /// Nur [`Fill::Eager`]: nach `EmptyClipboard` scheiterte
    /// `SetClipboardData`; das Transkript liegt stattdessen als neues
    /// Delayed-Versprechen (mit Marker). `delayed = true`.
    Promised(InjectError),
    /// Nach der Mutation liegt nichts Nutzbares: Zwischenablage leer.
    Lost(InjectError),
}

impl TakeFailure {
    fn into_error(self) -> InjectError {
        match self {
            Self::Foreign => {
                InjectError::Failed("Clipboard zwischenzeitlich fremd geändert".into())
            }
            Self::Untouched(err) | Self::Promised(err) => err,
            Self::Lost(err) => InjectError::Failed(TranscriptState::lost_message(&err.to_string())),
        }
    }
}

/// Was die Materialisierung eines offenen Versprechens ergab (intern, für
/// Paste-Pfad, Idle-Retry und Quit).
fn transcript_state(result: Result<(), TakeFailure>) -> TranscriptState {
    match result {
        Ok(()) | Err(TakeFailure::Foreign) => TranscriptState::Secured,
        Err(TakeFailure::Untouched(err)) => TranscriptState::PromiseOpen(err.to_string()),
        Err(TakeFailure::Promised(err)) => {
            TranscriptState::PromiseOpen(format!("{err} — Transkript erneut versprochen"))
        }
        Err(TakeFailure::Lost(err)) => TranscriptState::Lost(err.to_string()),
    }
}

/// Der Zustand, den `WndProc` und die Sink-Methoden teilen. Beide laufen auf
/// **demselben** Thread (dem Inject-Worker), deshalb reicht `RefCell`: der
/// `WndProc` wird nur aus `PeekMessageW`/`DispatchMessageW`/`SendMessage`
/// dieses Threads aufgerufen.
struct ClipboardState {
    /// Text, den `WM_RENDERFORMAT` liefert bzw. der zuletzt eager gesetzt wurde.
    serve: String,
    /// Was Diktier zuletzt tatsächlich ins Clipboard gesetzt hat
    /// (Leitentscheidung 6). Nur gültig, solange `owned`.
    payload: Payload,
    /// Sequenznummer nach der letzten **eigenen** Mutation
    /// (Leitentscheidung 4).
    expected_seq: u32,
    /// Sequenznummer zum Zeitpunkt des letzten Snapshots (§7.1 Punkt 5). Die
    /// folgende Übernahme prüft sie im geöffneten Clipboard erneut, sonst
    /// überschreibt sie einen fremden Copy, der zwischen Snapshot und
    /// `EmptyClipboard` liegt (Sol-Review Blocker 1). Wird bei der Übernahme
    /// konsumiert: ein `become_owner` ohne eigenen Snapshot (`copy_only`,
    /// Fokusverlust-Pfad) darf nicht gegen eine veraltete Sequenz prüfen.
    snapshot_seq: Option<u32>,
    /// Wir halten (nach eigener Buchführung) das Clipboard.
    owned: bool,
    /// Ein Delayed-Rendering-Versprechen ist offen, der Text liegt also noch
    /// nicht wirklich im Clipboard.
    delayed: bool,
    /// Seit dem letzten `pump()` bediente Reads.
    reads: u32,
    /// Seit dem letzten `pump()` beobachteter Ownership-Verlust.
    lost: bool,
    /// Tiefe eines laufenden **eigenen** Übergangs. Solange > 0 zählt
    /// `WM_DESTROYCLIPBOARD` nicht als fremd — unser eigenes `EmptyClipboard`
    /// schickt die Nachricht an uns selbst zurück.
    guard: u32,
    /// Der eigene Inhalt trägt den Verlaufsausschluss (Leitentscheidung 7).
    excluded: bool,
}

impl ClipboardState {
    fn new() -> Self {
        Self {
            serve: String::new(),
            payload: Payload::Empty,
            expected_seq: 0,
            snapshot_seq: None,
            owned: false,
            delayed: false,
            reads: 0,
            lost: false,
            guard: 0,
            excluded: false,
        }
    }

    fn forget_ownership(&mut self) {
        self.owned = false;
        self.delayed = false;
        // Bis zu 128 MiB restaurierter Formate nicht länger halten als nötig.
        self.payload = Payload::Empty;
    }

    /// Leitentscheidung 6: die eigene Payload als Snapshot, ohne
    /// `GetClipboardData` — ein eigener `WM_RENDERFORMAT` würde sonst als
    /// Read zählen.
    fn own_snapshot(&self) -> (ClipboardSnapshot, Vec<RawFormat>) {
        let raw = match &self.payload {
            Payload::Transcript => vec![RawFormat {
                format: FormatRef::new(CF_UNICODETEXT, None),
                kind: DataKind::Global,
                useful: true,
                data: Rc::new(utf16_bytes(&self.serve)),
            }],
            Payload::Formats(formats) => formats.clone(),
            Payload::Empty => Vec::new(),
        };
        if raw.is_empty() {
            return (
                ClipboardSnapshot::new(SnapshotKind::Empty, SnapshotReport::empty(true)),
                raw,
            );
        }
        let rows: Vec<FormatRow> = raw.iter().map(RawFormat::row).collect();
        let kind = formats::snapshot_kind(rows.len(), &rows);
        (
            ClipboardSnapshot::new(
                kind,
                SnapshotReport {
                    rows,
                    duration: Duration::ZERO,
                    own: true,
                },
            ),
            raw,
        )
    }
}

// --------------------------------------------------------------- WndProc

/// `WM_RENDERFORMAT`: **kein** `OpenClipboard` (das Clipboard gehört in diesem
/// Moment dem Anfordernden), nur `SetClipboardData` mit echten Daten.
fn on_render_format(cell: &RefCell<ClipboardState>) {
    // `try_borrow_mut`: ein Panic im WndProc wäre ein Unwind über die
    // Win32-Grenze (undefiniert). Reentranz kann es hier nicht geben, der
    // sichere Ausgang kostet aber nichts.
    let Ok(mut state) = cell.try_borrow_mut() else {
        return;
    };
    let Some(handle) = alloc_utf16(&state.serve) else {
        return;
    };
    // `place` übergibt das frische `GMEM_MOVEABLE`-Handle bzw. gibt es bei
    // einem Fehlschlag selbst frei.
    if place(CF_UNICODETEXT, Prepared::Global(handle)).is_err() {
        return;
    }
    state.delayed = false;
    // §7.1 Punkt 7: nur ein tatsächlich bedientes `CF_UNICODETEXT`-Render
    // zählt, keine Format- oder Viewer-Abfrage.
    state.reads = state.reads.saturating_add(1);
    // SAFETY: parameterlos, liest nur einen Zähler.
    state.expected_seq = unsafe { GetClipboardSequenceNumber() };
}

/// `WM_RENDERALLFORMATS`: hier **muss** geöffnet werden, und zwischen
/// Nachricht und `OpenClipboard` kann ein fremder Copy liegen — deshalb den
/// Owner erneut prüfen (MSDN).
fn on_render_all_formats(cell: &RefCell<ClipboardState>, hwnd: HWND) {
    let Ok(mut state) = cell.try_borrow_mut() else {
        return;
    };
    if !state.delayed {
        return;
    }
    // SAFETY: `hwnd` ist unser eigenes, noch existierendes Fenster.
    if unsafe { OpenClipboard(hwnd) } == 0 {
        return;
    }
    // SAFETY: beide parameterlos. Owner **und** Sequenz müssen im geöffneten
    // Clipboard noch die unseren sein — sonst hat zwischen Nachricht und
    // `OpenClipboard` jemand anderes kopiert (Sol-Review Blocker 2).
    if unsafe { GetClipboardOwner() } == hwnd
        && unsafe { GetClipboardSequenceNumber() } == state.expected_seq
        && let Some(handle) = alloc_utf16(&state.serve)
    {
        // Wie in `on_render_format`; kein `EmptyClipboard`, das würde die
        // bereits vorhandenen Daten (Marker) zerstören.
        if place(CF_UNICODETEXT, Prepared::Global(handle)).is_ok() {
            state.delayed = false;
            // SAFETY: parameterlos.
            state.expected_seq = unsafe { GetClipboardSequenceNumber() };
        }
    }
    // SAFETY: genau das oben geöffnete Clipboard wird einmal geschlossen.
    unsafe { CloseClipboard() };
}

/// `WM_DESTROYCLIPBOARD` ist **kein** Ownership-Beweis: die Nachricht entsteht
/// auch durch den eigenen Übergang (neues Transkript, Restore, Quit-Pfad).
/// Gewertet wird nur, was Owner und Sequenz danach wirklich sagen (Sol-Review).
fn on_destroy_clipboard(cell: &RefCell<ClipboardState>, hwnd: HWND) {
    let Ok(mut state) = cell.try_borrow_mut() else {
        return;
    };
    if state.guard > 0 || !state.owned {
        return;
    }
    // SAFETY: beide parameterlos.
    let owner = unsafe { GetClipboardOwner() };
    let seq = unsafe { GetClipboardSequenceNumber() };
    if owner != hwnd || seq != state.expected_seq {
        state.forget_ownership();
        state.lost = true;
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        // SAFETY: Für `WM_NCCREATE` garantiert Windows, dass `lparam` auf eine
        // gültige `CREATESTRUCTW` zeigt; `lpCreateParams` ist der Zeiger, den
        // `Win32OutputSink::new` an `CreateWindowExW` übergeben hat.
        let create = unsafe { &*(lparam as *const CREATESTRUCTW) };
        // SAFETY: `hwnd` ist gültig, `GWLP_USERDATA` gehört der Anwendung.
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize) };
        // SAFETY: unveränderte Parameter an die Default-Behandlung.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }

    // SAFETY: `hwnd` ist gültig; der Wert ist entweder 0 (vor `WM_NCCREATE`,
    // nach `WM_NCDESTROY`) oder der oben gesetzte Zeiger.
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const RefCell<ClipboardState>;
    if ptr.is_null() {
        // SAFETY: unveränderte Parameter an die Default-Behandlung.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    // SAFETY: Der Zeiger stammt aus dem `Box` in `Win32OutputSink::state`. Die
    // Box lebt länger als das Fenster: `Drop::drop` ruft `DestroyWindow`, erst
    // danach werden die Felder freigegeben. Der `WndProc` läuft ausschließlich
    // auf dem Thread, dem beide gehören.
    let cell = unsafe { &*ptr };

    match msg {
        WM_RENDERFORMAT if wparam as u32 == CF_UNICODETEXT => {
            on_render_format(cell);
            return 0;
        }
        WM_RENDERALLFORMATS => {
            on_render_all_formats(cell, hwnd);
            return 0;
        }
        WM_DESTROYCLIPBOARD => {
            on_destroy_clipboard(cell, hwnd);
            return 0;
        }
        WM_NCDESTROY => {
            // Ab hier darf niemand mehr über das Fenster an den Zustand.
            // SAFETY: `hwnd` ist noch gültig, letzte Nachricht des Fensters.
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
        }
        _ => {}
    }
    // SAFETY: unveränderte Parameter an die Default-Behandlung.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

// ------------------------------------------------------------------ Sink

pub struct Win32OutputSink {
    hwnd: HWND,
    instance: HINSTANCE,
    class_name: Vec<u16>,
    /// Nur wenn wir die Klasse selbst registriert haben, wird sie im `Drop`
    /// auch wieder abgemeldet.
    owns_class: bool,
    /// Boxed, damit die Adresse stabil bleibt — der `WndProc` kennt sie über
    /// `GWLP_USERDATA`.
    state: Box<RefCell<ClipboardState>>,
    output: OutputConfig,
    start: Instant,
    /// ID von `ExcludeClipboardContentFromMonitorProcessing`
    /// (Leitentscheidung 7). `None`, wenn die Registrierung scheiterte — dann
    /// fehlt nur der Verlaufsausschluss.
    marker: Option<u32>,
    /// Rohdaten des letzten Snapshots bis zum Restore (Leitentscheidung 4:
    /// sie bleiben hinter dem Host).
    stash: Option<Vec<RawFormat>>,
    /// Warnungen für den Daemon-Logger (`OutputSink::take_warnings`) statt
    /// `eprintln!` — ohne Konsole kämen sie sonst nie in `diktier.log`.
    warnings: Vec<String>,
}

impl Win32OutputSink {
    pub fn new(output: OutputConfig) -> Result<Self, InjectError> {
        let class_name = wide(CLASS_NAME);
        let window_name = wide("diktier clipboard");

        // SAFETY: `GetModuleHandleW(NULL)` liefert das Modul-Handle des eigenen
        // Prozesses, nimmt den Nullzeiger als dokumentiertes Argument und
        // überträgt kein Eigentum.
        let instance = unsafe { GetModuleHandleW(ptr::null()) };

        let class = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: ptr::null_mut(),
            hCursor: ptr::null_mut(),
            hbrBackground: ptr::null_mut(),
            lpszMenuName: ptr::null(),
            lpszClassName: class_name.as_ptr(),
        };
        // SAFETY: `class` ist vollständig initialisiert und lebt über den
        // Aufruf hinaus; `lpszClassName` zeigt in `class_name`, das ebenfalls
        // noch lebt. `wnd_proc` hat die von `WNDPROC` geforderte Signatur.
        let atom = unsafe { RegisterClassW(&class) };
        let owns_class = if atom == 0 {
            // SAFETY: parameterlos, liest den Fehlercode dieses Threads.
            let err = unsafe { GetLastError() };
            if err != ERROR_CLASS_ALREADY_EXISTS {
                return Err(InjectError::Failed(format!(
                    "Fensterklasse {CLASS_NAME} nicht registrierbar: Win32-Fehler {err}"
                )));
            }
            // Eine frühere Instanz auf diesem Prozess hat sie schon angemeldet.
            false
        } else {
            true
        };

        let state = Box::new(RefCell::new(ClipboardState::new()));
        let state_ptr: *const RefCell<ClipboardState> = &*state;

        // SAFETY: Alle Zeiger zeigen auf lebende, NUL-terminierte Puffer.
        // `HWND_MESSAGE` als Parent erzeugt ein Message-only-Fenster (keine
        // Darstellung, kein Fokus — §4.2). `state_ptr` erreicht den `WndProc`
        // als `lpCreateParams` in `WM_NCCREATE`; die Box lebt länger als das
        // Fenster (siehe `Drop`).
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class_name.as_ptr(),
                window_name.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                ptr::null_mut(),
                instance,
                state_ptr as *const c_void,
            )
        };
        if hwnd.is_null() {
            // SAFETY: parameterlos.
            let err = unsafe { GetLastError() };
            if owns_class {
                // SAFETY: Die Klasse wurde gerade von diesem Modul
                // registriert, und es existiert kein Fenster dazu.
                unsafe { UnregisterClassW(class_name.as_ptr(), instance) };
            }
            return Err(InjectError::Failed(format!(
                "Clipboard-Fenster nicht erzeugbar: Win32-Fehler {err}"
            )));
        }

        let marker_name = wide(EXCLUDE_FROM_MONITOR);
        let mut warnings = Vec::new();
        // SAFETY: `marker_name` ist NUL-terminiert und lebt über den Aufruf.
        // Derselbe Name liefert systemweit dieselbe ID; 0 heißt Fehler.
        let marker = match unsafe { RegisterClipboardFormatW(marker_name.as_ptr()) } {
            0 => {
                // SAFETY: parameterlos, direkt nach dem Aufruf.
                let err = unsafe { GetLastError() };
                // Kein Abbruch: es fehlt nur der Verlaufsausschluss. Der
                // Worker holt die Warnung nach dem Anlegen ab.
                warnings.push(format!(
                    "Verlaufsausschluss nicht registrierbar (Win32-Fehler {err}) — \
                     Transkript und Restore können in Win+V/Cloud landen"
                ));
                None
            }
            id => Some(id),
        };

        Ok(Self {
            hwnd,
            instance,
            class_name,
            owns_class,
            state,
            output,
            start: Instant::now(),
            marker,
            stash: None,
            warnings,
        })
    }

    /// Das kleine HGLOBAL für den Verlaufsausschluss. Laut Microsoft genügen
    /// beliebige Daten; es ist ein DWORD 0.
    fn marker_handle(&mut self) -> Option<(u32, OwnedGlobal)> {
        let id = self.marker?;
        let Some(handle) = alloc_bytes(&0_u32.to_le_bytes()) else {
            self.warnings
                .push("Verlaufsausschluss nicht gesetzt (GlobalAlloc fehlgeschlagen)".into());
            return None;
        };
        Some((id, handle))
    }

    /// Die Pump. Muss dauerhaft laufen können (Sol-Review): ohne sie hängt
    /// jedes fremde Einfügen und jeder Clipboard-Manager am offenen
    /// `WM_RENDERFORMAT`.
    fn pump_messages(&mut self, timeout: Duration) {
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        if ms > 0 {
            // SAFETY: `ncount == 0` mit `NULL`-Handle-Array ist die
            // dokumentierte Form „nur auf Eingabe warten“. Der Rückgabewert
            // unterscheidet nur Timeout von Nachricht — beides führt in die
            // Peek-Schleife.
            unsafe { MsgWaitForMultipleObjects(0, ptr::null(), 0, ms, QS_ALLINPUT) };
        }
        let mut msg = MSG::default();
        for _ in 0..MAX_MESSAGES_PER_PUMP {
            // SAFETY: `msg` ist ausgerichtet und beschreibbar; `NULL` als
            // `hwnd` holt alle Nachrichten dieses Threads.
            if unsafe { PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_REMOVE) } == 0 {
                break;
            }
            // Kein `TranslateMessage`: an dieses Fenster geht keine
            // Tastatureingabe, es gibt nichts zu übersetzen.
            // SAFETY: `msg` wurde gerade von `PeekMessageW` gefüllt.
            unsafe { DispatchMessageW(&msg) };
        }
    }

    fn take_events(&mut self) -> PumpEvents {
        let mut state = self.state.borrow_mut();
        PumpEvents {
            reads: std::mem::take(&mut state.reads),
            lost_ownership: std::mem::take(&mut state.lost),
        }
    }

    /// `GetClipboardOwner() == hwnd && seq == expected_seq`
    /// (Leitentscheidung 4). Vergleich nur per Gleichheit — ein DWORD-Wrap der
    /// Sequenznummer ist damit egal.
    fn is_still_owner(&self) -> bool {
        let mut state = self.state.borrow_mut();
        if !state.owned {
            return false;
        }
        // SAFETY: beide parameterlos.
        let owner = unsafe { GetClipboardOwner() };
        let seq = unsafe { GetClipboardSequenceNumber() };
        let ours = owner == self.hwnd && seq == state.expected_seq;
        if !ours {
            state.forget_ownership();
        }
        ours
    }

    /// `OpenClipboard` mit begrenztem Retry; zwischen den Versuchen wird
    /// gepumpt (der Clipboard-Manager, der gerade offen hat, wartet
    /// möglicherweise selbst auf unser `WM_RENDERFORMAT`).
    fn open_clipboard(&mut self) -> Result<(), InjectError> {
        self.try_open_clipboard().map_err(|err| {
            InjectError::Failed(format!(
                "Clipboard nicht zu öffnen ({OPEN_RETRIES} Versuche): Win32-Fehler {err}"
            ))
        })
    }

    /// Wie [`Self::open_clipboard`], liefert aber den Win32-Code des letzten
    /// Versuchs (der Snapshot braucht ihn für seinen Verlust-Eintrag).
    fn try_open_clipboard(&mut self) -> Result<(), u32> {
        for attempt in 0..OPEN_RETRIES {
            // SAFETY: `self.hwnd` ist unser lebendes Fenster.
            if unsafe { OpenClipboard(self.hwnd) } != 0 {
                return Ok(());
            }
            if attempt + 1 < OPEN_RETRIES {
                self.pump_messages(OPEN_RETRY_WAIT);
            }
        }
        // SAFETY: parameterlos, direkt nach dem letzten `OpenClipboard`.
        Err(unsafe { GetLastError() })
    }

    /// Eigener Übergang: `EmptyClipboard` (macht uns zum Owner) und je nach
    /// [`Fill`] das Versprechen, die Daten oder gar nichts. Aktualisiert
    /// anschließend `expected_seq`.
    ///
    /// `expect` schließt das Check-then-act-Fenster (Sol-Review Blocker 1/2):
    /// Zwischen Snapshot bzw. `is_still_owner` und diesem Punkt kann ein
    /// fremder Copy liegen. Innerhalb des geöffneten Clipboards ist der
    /// Zustand stabil, deshalb wird dort direkt vor `EmptyClipboard` erneut
    /// verglichen — Sequenz immer, Owner zusätzlich, wenn wir das Clipboard
    /// nach eigener Buchführung halten.
    fn take_clipboard(
        &mut self,
        text: String,
        fill: Fill,
        expect: Option<u32>,
    ) -> Result<(), TakeFailure> {
        // Speicher **vor** `OpenClipboard` besorgen (Sol-Review Blocker 3,
        // Leitentscheidung 5.1): scheitert `GlobalAlloc`, sind bisheriger
        // Inhalt und Transkript noch da, und das Clipboard war nicht blockiert.
        let data = match fill {
            Fill::Delayed => None,
            Fill::Eager => Some(alloc_utf16(&text).ok_or_else(|| {
                TakeFailure::Untouched(InjectError::Failed(
                    "Clipboard-Speicher (GlobalAlloc) fehlgeschlagen".into(),
                ))
            })?),
        };
        let marker = self.marker_handle();

        // Vor dem Guard öffnen: `open_clipboard` pumpt, und ein fremdes
        // `WM_DESTROYCLIPBOARD` in dieser Zeit soll noch gewertet werden.
        // Scheitert das Öffnen, bleibt der bisherige Serve-Text stehen — ein
        // noch offenes altes Versprechen wird sonst mit dem neuen Text bedient.
        self.open_clipboard().map_err(TakeFailure::Untouched)?;

        // SAFETY: parameterlos; das Clipboard ist offen und damit gegen
        // fremde Mutation gesperrt.
        let before_seq = unsafe { GetClipboardSequenceNumber() };
        if let Some(expected) = expect {
            // SAFETY: parameterlos, Clipboard ist offen.
            let owner = unsafe { GetClipboardOwner() };
            let owned = self.state.borrow().owned;
            if before_seq != expected || (owned && owner != self.hwnd) {
                // Kein `EmptyClipboard`, kein Guard: das folgende
                // `WM_DESTROYCLIPBOARD` (falls es kommt) stammt dann wirklich
                // von fremd und darf als Ownership-Verlust zählen.
                // SAFETY: genau das oben geöffnete Clipboard wird geschlossen.
                unsafe { CloseClipboard() };
                let mut state = self.state.borrow_mut();
                if owned {
                    state.forget_ownership();
                    state.lost = true;
                }
                return Err(TakeFailure::Foreign);
            }
        }

        self.state.borrow_mut().guard += 1;
        let filled = fill_open_clipboard(data, fill, marker, &mut self.warnings);
        // SAFETY: genau das oben geöffnete Clipboard wird einmal geschlossen.
        unsafe { CloseClipboard() };
        // SAFETY: beide parameterlos.
        let owner = unsafe { GetClipboardOwner() };
        let seq = unsafe { GetClipboardSequenceNumber() };

        let mut state = self.state.borrow_mut();
        state.guard -= 1;
        let (excluded, promised) = match filled {
            Filled::Placed(excluded) => (excluded, None),
            Filled::Promised(excluded, err) => (excluded, Some(err)),
            Filled::NotEmptied(err) if seq == before_seq => {
                // Final-Review Blocker 1: `EmptyClipboard` scheiterte, ohne
                // etwas zu ändern. Eigentum, Serve-Text und ein offenes
                // Versprechen bleiben — sonst erkennt der Quit-Pfad das
                // unverändert bestehende eigene Versprechen nicht mehr.
                return Err(TakeFailure::Untouched(err));
            }
            Filled::NotEmptied(err) | Filled::Lost(err) => {
                state.forget_ownership();
                return Err(TakeFailure::Lost(err));
            }
        };
        if owner != self.hwnd {
            // Direkt nach dem eigenen `CloseClipboard` hat jemand anderes
            // übernommen: dessen Inhalt liegt.
            state.forget_ownership();
            return Err(TakeFailure::Foreign);
        }
        // Zwischen `CloseClipboard` und hier kann kein `WM_RENDERFORMAT`
        // dazwischenkommen: gesendete Nachrichten erreichen den `WndProc` erst
        // beim nächsten Pumpen.
        state.serve = text;
        state.payload = Payload::Transcript;
        state.owned = true;
        state.delayed = fill == Fill::Delayed || promised.is_some();
        state.expected_seq = seq;
        state.excluded = excluded;
        // Reads vor dieser Übernahme galten dem alten Inhalt — auch ein
        // eigener Render, den ein Snapshot bei fremder Sequenz noch auslösen
        // kann. Für das neue Transkript zählen sie nicht (P7).
        state.reads = 0;
        match promised {
            None => Ok(()),
            Some(err) => Err(TakeFailure::Promised(err)),
        }
    }

    /// Die Sequenz, gegen die eine Übernahme bei **bestehendem** Eigentum
    /// geprüft wird (nach `is_still_owner`).
    fn owned_seq(&self) -> Option<u32> {
        Some(self.state.borrow().expected_seq)
    }

    /// Ein offenes Delayed-Rendering-Versprechen stirbt mit dem Prozess. Der
    /// Text wird deshalb eager hinterlegt (§7.1 Punkt 8) — im Paste-Pfad, im
    /// Idle-Retry, beim Quit und im `Drop`. Die Fehlerarten von
    /// [`TakeFailure`] sagen, ob das Versprechen noch steht.
    fn materialize(&mut self) -> Result<(), TakeFailure> {
        let text = self.state.borrow().serve.clone();
        let expect = self.owned_seq();
        self.take_clipboard(text, Fill::Eager, expect)
    }
}

/// Was nach dem Übergang im noch offenen Clipboard liegt.
enum Filled {
    /// Wie bestellt: Daten bzw. Versprechen. `bool`: mit Verlaufsausschluss.
    Placed(bool),
    /// Nur [`Fill::Eager`]: `SetClipboardData` mit Daten scheiterte, das
    /// Rückfall-Versprechen steht. `bool`: mit Verlaufsausschluss.
    Promised(bool, InjectError),
    /// `EmptyClipboard` scheiterte; ob sich etwas änderte, sagt die Sequenz.
    NotEmptied(InjectError),
    /// Geleert, aber weder Daten noch Versprechen: Zwischenablage leer.
    Lost(InjectError),
}

/// Der Teil, der ein **offenes** Clipboard voraussetzt. `data` ist bei
/// [`Fill::Eager`] das vorab allozierte Transkript, `marker` der
/// Verlaufsausschluss (Leitentscheidung 7). Nicht übergebene Handles gibt
/// `Drop` frei.
///
/// Final-Review Blocker 1: Scheitert nach `EmptyClipboard` das eager
/// `SetClipboardData`, wäre das Transkript sonst weg. Noch im geöffneten
/// Clipboard wird es deshalb wieder als Delayed-Versprechen gesetzt
/// ([`Filled::Promised`]); erst wenn auch das nicht steht, ist es
/// [`Filled::Lost`].
fn fill_open_clipboard(
    data: Option<OwnedGlobal>,
    fill: Fill,
    marker: Option<(u32, OwnedGlobal)>,
    warnings: &mut Vec<String>,
) -> Filled {
    // SAFETY: Das Clipboard ist von unserem Fenster geöffnet. `EmptyClipboard`
    // macht es zu unserem und schickt `WM_DESTROYCLIPBOARD` an den bisherigen
    // Owner — sind das wir selbst, hält der Guard des Aufrufers die Nachricht
    // von `lost_ownership` fern.
    if unsafe { EmptyClipboard() } == 0 {
        // SAFETY: parameterlos.
        let err = unsafe { GetLastError() };
        return Filled::NotEmptied(InjectError::Failed(format!(
            "EmptyClipboard: Win32-Fehler {err}"
        )));
    }
    let eager_error = data.and_then(|handle| {
        place(CF_UNICODETEXT, Prepared::Global(handle))
            .err()
            .map(|err| InjectError::Failed(format!("SetClipboardData: Win32-Fehler {err}")))
    });
    if fill == Fill::Delayed || eager_error.is_some() {
        // SAFETY: `NULL` ist die dokumentierte Form für Delayed Rendering; es
        // wird kein Handle übergeben. Der Rückgabewert ist auch bei Erfolg
        // `NULL` und wird deshalb unten über die Verfügbarkeit geprüft.
        unsafe { SetClipboardData(CF_UNICODETEXT, ptr::null_mut()) };
        // `GetClipboardOwner() == hwnd` beweist nur das `EmptyClipboard`. Ob
        // das Versprechen wirklich steht, sagt allein die
        // Formatverfügbarkeit — noch im offenen Clipboard geprüft.
        // SAFETY: parameterlos bis auf das Format, Clipboard ist offen.
        if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) } == 0 {
            // SAFETY: parameterlos.
            let err = unsafe { GetLastError() };
            let detail = match &eager_error {
                Some(eager) => {
                    format!("{eager}; Rückfall-Versprechen nicht registriert: Win32-Fehler {err}")
                }
                None => {
                    format!(
                        "Delayed Rendering für CF_UNICODETEXT nicht registriert: Win32-Fehler {err}"
                    )
                }
            };
            return Filled::Lost(InjectError::Failed(detail));
        }
    }
    let excluded = place_marker(marker, warnings);
    match eager_error {
        None => Filled::Placed(excluded),
        Some(err) => Filled::Promised(excluded, err),
    }
}

impl ClipboardHost for Win32OutputSink {
    fn mark_start(&mut self) {
        self.start = Instant::now();
    }

    fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    fn current_window(&self) -> Option<WindowId> {
        foreground_window()
    }

    /// Auf Windows ist `wm_class` ein portabler Trait-Platzhalter: geliefert
    /// wird zweimal der Prozess-Basename (windows-plan WP3). Zugriffsfehler
    /// (erhöhtes Ziel, UIPI) sind `None` und **kein** Inject-Fehler.
    fn wm_class(&self, window: WindowId) -> Option<(String, String)> {
        let name = process_basename(window)?;
        Some((name.clone(), name))
    }

    fn snapshot_clipboard(&mut self) -> Result<ClipboardSnapshot, InjectError> {
        // Angelaufene Nachrichten zuerst verarbeiten — ein fremder Copy kurz
        // vor dem Snapshot darf nicht in die neue Session hineinlecken.
        self.pump_messages(Duration::ZERO);
        let _ = self.take_events();
        self.stash = None;

        if self.is_still_owner() {
            let (snapshot, raw) = {
                let mut state = self.state.borrow_mut();
                let seq = state.expected_seq;
                state.snapshot_seq = Some(seq);
                state.own_snapshot()
            };
            self.stash = Some(raw);
            return Ok(snapshot);
        }

        // Nacharbeit WP1: Ein gescheiterter Snapshot bricht den Paste nicht
        // ab. Er wird `Unrestorable` mit dem Win32-Code als Verlust; der Paste
        // läuft ohne Restore-Versprechen weiter (`NoPromise`), und
        // `become_owner` entscheidet wie bisher.
        let started = Instant::now();
        if let Err(err) = self.try_open_clipboard() {
            // Sol-Impl-Review Blocker 1: Auch ohne gelesenen Inhalt schützt die
            // Übernahme den Zustand, den Diktier jetzt sieht. Die Sequenz ist
            // ohne geöffnetes Clipboard lesbar; `take_clipboard` vergleicht sie
            // im geöffneten Clipboard. Ein fremder Copy dazwischen →
            // `TakeFailure::Foreign` → nichts überschrieben, Inject-Fehler.
            // SAFETY: parameterlos, braucht kein geöffnetes Clipboard.
            let seq = unsafe { GetClipboardSequenceNumber() };
            self.state.borrow_mut().snapshot_seq = Some(seq);
            self.stash = Some(Vec::new());
            return Ok(ClipboardSnapshot::failed(err, started.elapsed()));
        }
        let read = read_open_clipboard(started);
        // §7.1 Punkt 1: zum Snapshot gehört auf Windows die Sequenznummer.
        // Noch im offenen Clipboard gelesen, damit sie wirklich zu dem gerade
        // gelesenen Inhalt gehört.
        // SAFETY: parameterlos.
        let seq = unsafe { GetClipboardSequenceNumber() };
        // SAFETY: genau das oben geöffnete Clipboard wird einmal geschlossen —
        // auch dann, wenn das Lesen scheiterte.
        unsafe { CloseClipboard() };
        self.state.borrow_mut().snapshot_seq = Some(seq);
        let (kind, rows, raw) = match read {
            Ok(read) => read,
            Err(err) => {
                self.stash = Some(Vec::new());
                return Ok(ClipboardSnapshot::failed(err, started.elapsed()));
            }
        };
        self.stash = Some(raw);
        Ok(ClipboardSnapshot::new(
            kind,
            SnapshotReport {
                rows,
                duration: started.elapsed(),
                own: false,
            },
        ))
    }

    fn become_owner(&mut self, text: String) -> Result<(), InjectError> {
        // `take()`: nur die Übernahme direkt nach einem Snapshot prüft gegen
        // dessen Sequenz. `copy_only` und der Fokusverlust-Pfad übernehmen
        // ohne Restore-Versprechen und ohne vorherigen Snapshot.
        let expect = self.state.borrow_mut().snapshot_seq.take();
        self.take_clipboard(text, Fill::Delayed, expect)
            .map_err(TakeFailure::into_error)
    }

    /// `CopyOnly`: eager, kein Versprechen (Blocker 2). Nach einem Snapshot
    /// mit derselben Sequenzprüfung wie `become_owner`. Scheitert das eager
    /// Setzen nach `EmptyClipboard`, liegt das Transkript als Versprechen
    /// (`Ok(PromiseOpen)`, Final-Review Blocker 1).
    fn copy_transcript(&mut self, text: String) -> Result<TranscriptState, InjectError> {
        let expect = self.state.borrow_mut().snapshot_seq.take();
        match self.take_clipboard(text, Fill::Eager, expect) {
            Ok(()) => Ok(TranscriptState::Secured),
            Err(TakeFailure::Promised(err)) => Ok(TranscriptState::PromiseOpen(format!(
                "{err} — Transkript erneut versprochen"
            ))),
            Err(other) => Err(other.into_error()),
        }
    }

    /// Blocker 2: ein offenes Versprechen sofort eager hinterlegen, mit Marker,
    /// Guard und `expect`-Sequenz wie im Quit-Pfad (`materialize`). Das
    /// Ergebnis geht in den Inject-Ausgang (Final-Review Blocker 1).
    fn materialize_transcript(&mut self) -> TranscriptState {
        if !self.is_still_owner() || !self.state.borrow().delayed {
            return TranscriptState::Secured;
        }
        transcript_state(self.materialize())
    }

    fn promise_recorded(&self) -> bool {
        let state = self.state.borrow();
        state.owned && state.delayed
    }

    fn history_excluded(&mut self) -> bool {
        if !self.is_still_owner() {
            return true;
        }
        let state = self.state.borrow();
        matches!(state.payload, Payload::Empty) || state.excluded
    }

    fn still_owner(&mut self) -> Result<bool, InjectError> {
        Ok(self.is_still_owner())
    }

    /// Leitentscheidung 5. Die Daten gehen immer **eager** zurück: nach
    /// `WM_RENDERFORMAT` liegt das Transkript als echte Daten im Clipboard,
    /// ein späterer Leser fragt uns nicht mehr.
    fn restore_snapshot(
        &mut self,
        snapshot: &ClipboardSnapshot,
        transcript: &str,
    ) -> RestoreResult {
        let stash = self.stash.take().unwrap_or_default();
        if snapshot.kind == SnapshotKind::Unrestorable {
            // Kein Versprechen (§7.1 Punkt 2); das Protokoll ruft hier nicht an.
            return RestoreResult::RestoreFailed {
                lost_restore: Vec::new(),
            };
        }

        // 1. Vor `OpenClipboard`: alle Kopien, das Transkript-Fallback und der
        //    Marker. Jedes Handle liegt in einer RAII-Hülle.
        let mut lost_restore = Vec::new();
        let mut prepared = Vec::new();
        if snapshot.kind == SnapshotKind::Formats {
            for raw in stash {
                match prepare(&raw) {
                    Some(handle) => prepared.push((raw, handle)),
                    None => lost_restore.push(LostFormat {
                        format: raw.format.clone(),
                        reason: LossReason::AllocFailed,
                        phase: Phase::Restore,
                    }),
                }
            }
        }
        let Some(fallback) = alloc_utf16(transcript) else {
            // Ohne Fallback nicht leeren: das Transkript liegt noch.
            self.warnings.push(
                "Clipboard-Restore unterblieben: kein Speicher für das Transkript-Fallback".into(),
            );
            return RestoreResult::RestoreFailed { lost_restore };
        };
        let marker = self.marker_handle();

        // 2. Öffnen, Owner **und** Sequenz im geöffneten Clipboard prüfen.
        if let Err(err) = self.open_clipboard() {
            if !self.is_still_owner() {
                return RestoreResult::Foreign;
            }
            // Nichts angefasst — das Transkript liegt noch im Clipboard.
            self.warnings
                .push(format!("Clipboard-Restore unterblieben: {err}"));
            return RestoreResult::RestoreFailed { lost_restore };
        }
        let before_seq = {
            let mut state = self.state.borrow_mut();
            // SAFETY: beide parameterlos; das Clipboard ist offen und damit
            // gegen fremde Mutation gesperrt.
            let owner = unsafe { GetClipboardOwner() };
            let seq = unsafe { GetClipboardSequenceNumber() };
            if !state.owned || owner != self.hwnd || seq != state.expected_seq {
                // SAFETY: genau das oben geöffnete Clipboard wird geschlossen.
                unsafe { CloseClipboard() };
                if state.owned {
                    state.forget_ownership();
                    state.lost = true;
                }
                // `prepared`, `fallback`, `marker` gibt `Drop` frei.
                return RestoreResult::Foreign;
            }
            state.guard += 1;
            seq
        };

        // 3./4. Leeren, in Originalreihenfolge setzen, ggf. Fallback.
        let placement = restore_open_clipboard(
            snapshot.kind,
            prepared,
            fallback,
            marker,
            &mut lost_restore,
            &mut self.warnings,
        );
        // 5. `CloseClipboard` auf jedem Pfad.
        // SAFETY: genau das oben geöffnete Clipboard wird einmal geschlossen.
        unsafe { CloseClipboard() };
        // SAFETY: beide parameterlos.
        let owner = unsafe { GetClipboardOwner() };
        let seq = unsafe { GetClipboardSequenceNumber() };

        let hwnd = self.hwnd;
        let mut state = self.state.borrow_mut();
        state.guard -= 1;
        let own = |state: &mut ClipboardState, payload: Payload| {
            state.owned = owner == hwnd;
            state.delayed = false;
            state.expected_seq = seq;
            state.payload = if state.owned { payload } else { Payload::Empty };
        };
        match placement {
            Placement::Formats(placed, excluded) => {
                own(&mut state, Payload::Formats(placed));
                state.excluded = excluded;
                let lost_save = snapshot.report.lost();
                if lost_save.is_empty() && lost_restore.is_empty() {
                    RestoreResult::Restored
                } else {
                    RestoreResult::RestoredPartial {
                        lost_save,
                        lost_restore,
                    }
                }
            }
            Placement::Empty => {
                own(&mut state, Payload::Empty);
                state.excluded = false;
                RestoreResult::Restored
            }
            Placement::Transcript(excluded) => {
                state.serve = transcript.to_string();
                own(&mut state, Payload::Transcript);
                state.excluded = excluded;
                RestoreResult::RestoreFailed { lost_restore }
            }
            Placement::NotEmptied(err) if seq == before_seq => {
                // `EmptyClipboard` scheiterte, ohne etwas zu ändern: das
                // Transkript liegt unverändert, wir sind weiter Owner.
                self.warnings
                    .push(format!("Clipboard-Restore unterblieben: {err}"));
                RestoreResult::RestoreFailed { lost_restore }
            }
            Placement::NotEmptied(err) | Placement::Lost(err) => {
                own(&mut state, Payload::Empty);
                RestoreResult::Failed(err)
            }
        }
    }

    fn discard_snapshot(&mut self) {
        self.stash = None;
    }

    fn query_modifiers(&self) -> Result<ModifierState, InjectError> {
        Ok(ModifierState {
            shift: key_is_down(VK_SHIFT),
            alt: key_is_down(VK_MENU),
            super_key: key_is_down(VK_LWIN) || key_is_down(VK_RWIN),
            ctrl: key_is_down(VK_CONTROL),
        })
    }

    fn key_down(&mut self, key: PasteKey) -> Result<(), InjectError> {
        send_key(key, true)
    }

    fn key_up(&mut self, key: PasteKey) -> Result<(), InjectError> {
        send_key(key, false)
    }

    fn pump(&mut self, timeout: Duration) -> Result<PumpEvents, InjectError> {
        self.pump_messages(timeout);
        Ok(self.take_events())
    }
}

impl OutputSink for Win32OutputSink {
    fn paste(&mut self, text: &str, ctx: &CaptureContext) -> Result<InjectOutcome, InjectError> {
        let output = self.output.clone();
        inject_paste(self, text, ctx, &output)
    }

    /// Eager statt delayed (Blocker 2): hier wird kein Read gebraucht.
    fn copy_only(&mut self, text: &str) -> Result<Copied, InjectError> {
        let text = apply_leading_space(text, self.output.leading_space);
        let transcript = self.copy_transcript(text)?;
        Ok(Copied {
            history_excluded: ClipboardHost::history_excluded(self),
            transcript,
        })
    }

    fn current_window_id(&self) -> Option<WindowId> {
        foreground_window()
    }

    fn serve_for(&mut self, duration: Duration) -> Result<(), InjectError> {
        let _ = self.pump(duration)?;
        Ok(())
    }

    fn serve_until_read(&mut self, timeout: Duration) -> Result<u32, InjectError> {
        serve_restored_until_read(self, timeout)
    }

    /// Windows-Äquivalent zum ICCCM-`SAVE_TARGETS`: den Text eager rendern,
    /// damit er den Prozess überlebt. Es gibt keinen Manager, der ablehnen
    /// könnte — entweder das Clipboard gehört uns und der Text steht drin
    /// (`Saved`), oder wir sind nicht Owner (`NotOwner`, bei offenem
    /// Versprechen `PromiseForeign`). Blockiert das Clipboard, wird bis
    /// `deadline` erneut versucht (Final-Review Blocker 2); danach `Err`.
    fn save_to_clipboard_manager(
        &mut self,
        deadline: Instant,
    ) -> Result<ClipboardSave, InjectError> {
        save_transcript_on_quit(self, deadline)
    }

    fn pending_promise(&mut self) -> bool {
        // Erst die eigene Buchführung, damit der Idle-Takt (10 ms) ohne
        // offenes Versprechen keine Win32-Abfrage kostet.
        let delayed = self.state.borrow().delayed;
        delayed && self.is_still_owner()
    }

    fn materialize_pending(&mut self) -> TranscriptState {
        ClipboardHost::materialize_transcript(self)
    }

    fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }
}

impl Drop for Win32OutputSink {
    fn drop(&mut self) {
        // Ohne das wäre der Text nach dem Prozessende weg — der Spike
        // `--inject-test` ruft `save_to_clipboard_manager` gar nicht auf.
        if self.is_still_owner() && self.state.borrow().delayed {
            match self.materialize() {
                Ok(()) => {}
                Err(TakeFailure::Foreign) => {
                    eprintln!(
                        "Clipboard-Sicherung unterblieben: fremde Änderung seit der Übernahme"
                    );
                }
                Err(err) => {
                    eprintln!("Clipboard-Sicherung fehlgeschlagen: {}", err.into_error());
                }
            }
        }
        if !self.hwnd.is_null() {
            // SAFETY: Das Fenster gehört diesem Thread und existiert noch;
            // `WM_NCDESTROY` löscht dabei den `GWLP_USERDATA`-Zeiger. Die Box
            // in `self.state` wird erst nach diesem `drop` freigegeben.
            unsafe { DestroyWindow(self.hwnd) };
            self.hwnd = ptr::null_mut();
        }
        if self.owns_class {
            // SAFETY: Die Klasse wurde von diesem Modul registriert, das
            // einzige Fenster dazu ist gerade zerstört worden.
            unsafe { UnregisterClassW(self.class_name.as_ptr(), self.instance) };
        }
    }
}

// ------------------------------------------------- Diagnose-CLI (WP2)

impl Win32OutputSink {
    /// Nachprüfung im Roundtrip (Sol-Impl-Review): alle IDs als Metadaten,
    /// byteweise aber nur die im Snapshot gesicherten, mit Zeitbudget und
    /// einem Byte-Budget aus den gesicherten Längen plus Reserve
    /// ([`formats::verify_budget`]). Ohne die eigene Payload als Abkürzung.
    fn read_after(&mut self, saved: &[(u32, usize)]) -> Result<formats::SavedRead, InjectError> {
        self.open_clipboard()?;
        let started = Instant::now();
        let ids: Vec<u32> = saved.iter().map(|(id, _)| *id).collect();
        let lengths: Vec<usize> = saved.iter().map(|(_, len)| *len).collect();
        let read = enumerate_open_clipboard()
            .map_err(|err| InjectError::Failed(format!("EnumClipboardFormats: Win32-Fehler {err}")))
            .map(|entries| {
                formats::read_saved(
                    &entries,
                    &ids,
                    formats::verify_budget(&lengths),
                    MAX_SNAPSHOT_TIME,
                    || started.elapsed(),
                    |entry, kind, remaining| read_format(entry.id, kind, remaining),
                )
            });
        // SAFETY: genau das oben geöffnete Clipboard wird einmal geschlossen.
        unsafe { CloseClipboard() };
        read
    }
}

/// `--clipboard-check`: nur lesend. Das Lesen kann bei der Quelle verzögert
/// gerenderte Formate anstoßen; sonst ändert sich nichts.
pub fn clipboard_check() -> Result<ClipboardSnapshot, InjectError> {
    let mut sink = Win32OutputSink::new(OutputConfig::default())?;
    let snapshot = sink.snapshot_clipboard();
    sink.discard_snapshot();
    print_cli_warnings(&mut sink);
    snapshot
}

/// CLI-Pfade ohne Daemon-Logger: die Sink-Warnungen nach stderr.
fn print_cli_warnings(sink: &mut Win32OutputSink) {
    for warning in sink.take_warnings() {
        eprintln!("Warnung: {warning}");
    }
}

/// Ausgang des Roundtrips aus `--clipboard-check --roundtrip`.
#[derive(Debug)]
pub enum RoundtripRestore {
    /// Snapshot `Unrestorable`: das Clipboard wurde nicht angefasst.
    NotAttempted,
    Restored,
    RestoredPartial,
    /// Kein Nutzformat zurück; der Testtext liegt in der Zwischenablage.
    RestoreFailed,
    /// Ein fremder Copy kam dazwischen; dessen Inhalt bleibt.
    Foreign,
    Failed(String),
}

#[derive(Debug)]
pub struct Roundtrip {
    pub before: ClipboardSnapshot,
    pub restore: RoundtripRestore,
    pub lost_restore: Vec<LostFormat>,
    /// Abweichungen je gesichertem Format (IDs, Reihenfolge, Bytes). Nur nach
    /// einem Restore aussagekräftig.
    pub mismatches: Vec<String>,
    /// Gesicherte Formate, die das Budget der Nachprüfung nicht mehr lesen
    /// ließ — „nicht geprüft“, keine Abweichung.
    pub unchecked: Vec<FormatRef>,
    /// Was danach im Clipboard liegt, in Enumerationsreihenfolge.
    pub after: Vec<FormatRef>,
    /// Sol-Impl-Review Blocker 3: die Nachprüfung scheiterte (Clipboard
    /// blockiert). `restore` sagt dann, was platziert wurde; der aktuelle
    /// Inhalt ist unbekannt.
    pub after_error: Option<String>,
}

/// `--clipboard-check --roundtrip`: Snapshot → Testtext → Restore → erneut
/// lesen und vergleichen. **Überschreibt die Zwischenablage kurzzeitig.** Die
/// Prüfung, dass kein Daemon läuft, macht der Aufrufer (`main.rs`). Keine
/// Aussage über OLE- oder Owner-Semantik (F1).
pub fn clipboard_roundtrip() -> Result<Roundtrip, InjectError> {
    let mut sink = Win32OutputSink::new(OutputConfig::default())?;
    let result = roundtrip_with(&mut sink);
    print_cli_warnings(&mut sink);
    result
}

fn roundtrip_with(sink: &mut Win32OutputSink) -> Result<Roundtrip, InjectError> {
    let before = sink.snapshot_clipboard()?;
    if before.kind == SnapshotKind::Unrestorable {
        // Ohne Restore-Versprechen wird nichts überschrieben.
        sink.discard_snapshot();
        let (after, after_error) = match sink.read_after(&[]) {
            Ok(after) => (after.formats.into_iter().map(|(f, _)| f).collect(), None),
            Err(err) => (Vec::new(), Some(err.to_string())),
        };
        return Ok(Roundtrip {
            before,
            restore: RoundtripRestore::NotAttempted,
            lost_restore: Vec::new(),
            mismatches: Vec::new(),
            unchecked: Vec::new(),
            after,
            after_error,
        });
    }
    let expected: Vec<(FormatRef, Vec<u8>)> = sink
        .stash
        .iter()
        .flatten()
        .map(|raw| (raw.format.clone(), raw.data.as_ref().clone()))
        .collect();

    let (restore, lost_restore) = match sink.become_owner(ROUNDTRIP_TEXT.to_string()) {
        Err(err) => (RoundtripRestore::Failed(err.to_string()), Vec::new()),
        Ok(()) => match sink.restore_snapshot(&before, ROUNDTRIP_TEXT) {
            RestoreResult::Restored => (RoundtripRestore::Restored, Vec::new()),
            RestoreResult::RestoredPartial { lost_restore, .. } => {
                (RoundtripRestore::RestoredPartial, lost_restore)
            }
            RestoreResult::RestoreFailed { lost_restore } => {
                (RoundtripRestore::RestoreFailed, lost_restore)
            }
            RestoreResult::Foreign => (RoundtripRestore::Foreign, Vec::new()),
            RestoreResult::Failed(err) => (RoundtripRestore::Failed(err.to_string()), Vec::new()),
        },
    };
    let saved: Vec<(u32, usize)> = expected.iter().map(|(f, b)| (f.id, b.len())).collect();
    let after = match sink.read_after(&saved) {
        Ok(after) => after,
        // Kein `?`: das Platzierungsergebnis muss mit hinaus.
        Err(err) => {
            return Ok(Roundtrip {
                before,
                restore,
                lost_restore,
                mismatches: Vec::new(),
                unchecked: Vec::new(),
                after: Vec::new(),
                after_error: Some(err.to_string()),
            });
        }
    };
    let unchecked_ids: Vec<u32> = after.unchecked.iter().map(|f| f.id).collect();
    let mismatches = match restore {
        RoundtripRestore::Restored | RoundtripRestore::RestoredPartial => {
            let known: Vec<u32> = before
                .report
                .rows
                .iter()
                .map(|row| row.format.id)
                .chain(sink.marker)
                .collect();
            formats::compare_roundtrip(&expected, &after.formats, &known, &unchecked_ids)
        }
        _ => Vec::new(),
    };
    Ok(Roundtrip {
        before,
        restore,
        lost_restore,
        mismatches,
        unchecked: after.unchecked,
        after: after.formats.into_iter().map(|(f, _)| f).collect(),
        after_error: None,
    })
}

// ------------------------------------------------------- freie Helfer

/// NUL-terminierter UTF-16-Puffer für die `W`-APIs.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Clipboard-Text als UTF-16 **mit** NUL — genau das erwartet
/// `CF_UNICODETEXT`.
fn to_utf16_nul(text: &str) -> Vec<u16> {
    wide(text)
}

/// UTF-16 bis zur ersten NUL. `from_utf16_lossy` fängt unpaarige Surrogates
/// ab, die eine fremde Anwendung durchaus abgelegt haben kann.
fn utf16_until_nul(units: &[u16]) -> String {
    let end = units.iter().position(|u| *u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

/// Letzte Pfadkomponente. `QueryFullProcessImageNameW` liefert einen
/// Win32-Pfad; Forward-Slashes sind trotzdem erlaubt.
fn basename(path: &str) -> &str {
    match path.rfind(['\\', '/']) {
        Some(idx) => &path[idx + 1..],
        None => path,
    }
}

/// `GMEM_MOVEABLE`-Handle mit genau diesen Bytes. Das Handle geht bei
/// erfolgreichem `SetClipboardData` in das Eigentum des Systems über; bis
/// dahin gibt [`OwnedGlobal`] es frei.
fn alloc_bytes(bytes: &[u8]) -> Option<OwnedGlobal> {
    if bytes.is_empty() {
        return None;
    }
    // SAFETY: `GMEM_MOVEABLE` ist die für Clipboard-Handles vorgeschriebene
    // Form; der Rückgabewert wird sofort geprüft.
    let handle = OwnedGlobal(unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) });
    if handle.0.is_null() {
        return None;
    }
    // SAFETY: frisch alloziertes Handle, das ausschließlich uns gehört.
    let ptr = unsafe { GlobalLock(handle.0) } as *mut u8;
    if ptr.is_null() {
        // `handle` wird ungesperrt freigegeben.
        return None;
    }
    // SAFETY: `bytes.len()` Bytes wurden gerade alloziert, Quelle und Ziel
    // überlappen nicht.
    unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len()) };
    // SAFETY: genau ein `GlobalLock` wird zurückgenommen.
    unsafe { GlobalUnlock(handle.0) };
    Some(handle)
}

/// UTF-16LE mit NUL als Bytes — der Inhalt eines `CF_UNICODETEXT`.
fn utf16_bytes(text: &str) -> Vec<u8> {
    to_utf16_nul(text)
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// `GMEM_MOVEABLE`-Handle mit dem Text als UTF-16+NUL.
fn alloc_utf16(text: &str) -> Option<OwnedGlobal> {
    alloc_bytes(&utf16_bytes(text))
}

/// Kopie für das Zurückschreiben, **vor** `OpenClipboard` (Leitentscheidung
/// 5.1). `None` zählt als Verlust in Phase „Wiederherstellen“.
fn prepare(raw: &RawFormat) -> Option<Prepared> {
    match raw.kind {
        DataKind::Global => alloc_bytes(&raw.data).map(Prepared::Global),
        DataKind::Emf => {
            let size = u32::try_from(raw.data.len()).ok()?;
            // SAFETY: `raw.data` hat genau `size` lesbare Bytes; die API liest
            // nur und legt ein neues, eigenes EMF an (oder liefert NULL).
            let handle = OwnedEmf(unsafe { SetEnhMetaFileBits(size, raw.data.as_ptr()) });
            if handle.0.is_null() {
                None
            } else {
                Some(Prepared::Emf(handle))
            }
        }
    }
}

/// Verlaufs- und Cloud-Ausschluss (Leitentscheidung 7). Ein Fehlschlag kostet
/// nur den Ausschluss, nicht den Inhalt. `true`: der Marker liegt.
fn place_marker(marker: Option<(u32, OwnedGlobal)>, warnings: &mut Vec<String>) -> bool {
    let Some((id, handle)) = marker else {
        // Registrierung bzw. Allokation scheiterte; gewarnt wurde dort.
        return false;
    };
    match place(id, Prepared::Global(handle)) {
        Ok(()) => true,
        Err(err) => {
            warnings.push(format!(
                "Verlaufsausschluss nicht gesetzt (Win32-Fehler {err})"
            ));
            false
        }
    }
}

/// Was nach dem Zurückschreiben im Clipboard liegt.
enum Placement {
    /// Mindestens ein Nutzformat; genau diese Formate. `bool`: mit
    /// Verlaufsausschluss.
    Formats(Vec<RawFormat>, bool),
    /// Leerer Snapshot, leeres Clipboard.
    Empty,
    /// Kein Nutzformat platzierbar, das Transkript-Fallback liegt. `bool`:
    /// mit Verlaufsausschluss.
    Transcript(bool),
    /// `EmptyClipboard` scheiterte; ob sich etwas änderte, sagt die Sequenz.
    NotEmptied(InjectError),
    /// Nach dem Leeren ließ sich gar nichts Nutzbares setzen.
    Lost(InjectError),
}

/// Schritte 3 und 4 aus Leitentscheidung 5, bei geöffnetem Clipboard und
/// gesetztem Guard.
fn restore_open_clipboard(
    kind: SnapshotKind,
    prepared: Vec<(RawFormat, Prepared)>,
    fallback: OwnedGlobal,
    marker: Option<(u32, OwnedGlobal)>,
    lost_restore: &mut Vec<LostFormat>,
    warnings: &mut Vec<String>,
) -> Placement {
    // SAFETY: Das Clipboard ist von unserem Fenster geöffnet; das folgende
    // `WM_DESTROYCLIPBOARD` an uns selbst hält der Guard des Aufrufers fern.
    if unsafe { EmptyClipboard() } == 0 {
        // SAFETY: parameterlos, direkt nach dem gescheiterten Aufruf.
        let err = unsafe { GetLastError() };
        return Placement::NotEmptied(InjectError::Failed(format!(
            "EmptyClipboard: Win32-Fehler {err}"
        )));
    }
    if kind == SnapshotKind::Empty {
        // Wirklich leer, auch ohne Marker (`CountClipboardFormats() == 0`).
        return Placement::Empty;
    }

    let mut placed = Vec::with_capacity(prepared.len());
    for (raw, handle) in prepared {
        match place(raw.format.id, handle) {
            Ok(()) => placed.push(raw),
            Err(err) => lost_restore.push(LostFormat {
                format: raw.format.clone(),
                reason: LossReason::SetFailed(err),
                phase: Phase::Restore,
            }),
        }
    }
    if placed.iter().any(|raw| raw.useful) {
        // Trug das Original den Ausschluss schon selbst, ist er mit
        // zurückgekommen; sonst kommt unserer dazu (Vorrang vor den
        // Policy-Formaten, Leitentscheidung 7).
        let already = marker
            .as_ref()
            .is_some_and(|(id, _)| placed.iter().any(|raw| raw.format.id == *id));
        let excluded = already || place_marker(marker, warnings);
        return Placement::Formats(placed, excluded);
    }

    // 4. Kein Nutzformat platziert: noch im geöffneten Clipboard das
    //    Transkript. Eventuell gesetzte Begleitformate bleiben daneben stehen;
    //    allein tragen sie keinen Inhalt.
    match place(CF_UNICODETEXT, Prepared::Global(fallback)) {
        Ok(()) => Placement::Transcript(place_marker(marker, warnings)),
        Err(err) => Placement::Lost(InjectError::Failed(format!(
            "Zwischenablage leer — Transkript und vorheriger Inhalt verloren \
             (SetClipboardData: Win32-Fehler {err})"
        ))),
    }
}

/// Name eines registrierten Formats (roh, nur für die Matrix und zum
/// Bereinigen). Standard-IDs haben keinen.
fn registered_name(id: u32) -> Option<String> {
    if id < FIRST_REGISTERED {
        return None;
    }
    let mut buf = [0_u16; FORMAT_NAME_CHARS];
    // SAFETY: `buf` hat `FORMAT_NAME_CHARS` beschreibbare `u16`, und genau das
    // steht im Längenparameter; die API schreibt höchstens so viele inklusive
    // NUL und liefert die Zeichenzahl ohne NUL.
    let len = unsafe { GetClipboardFormatNameW(id, buf.as_mut_ptr(), FORMAT_NAME_CHARS as i32) };
    let len = usize::try_from(len).ok().filter(|len| *len > 0)?;
    Some(String::from_utf16_lossy(&buf[..len.min(buf.len())]))
}

/// Leitentscheidung 1: erst **alle** IDs in Enumerationsreihenfolge, danach
/// die Daten. Ende ist `0` mit `ERROR_SUCCESS`, jeder andere Code ist ein
/// Snapshot-Fehler. `Err` trägt den Win32-Code bzw.
/// [`formats::ENUM_INCONSISTENT`] (wiederholte ID, Schranke 0x10000).
fn enumerate_open_clipboard() -> Result<Vec<Enumerated>, u32> {
    let ids = formats::enumerate_ids(|current| {
        // SAFETY: parameterlos; setzt nur den Fehlercode dieses Threads, damit
        // ein `ERROR_SUCCESS` danach wirklich von `EnumClipboardFormats` stammt.
        unsafe { SetLastError(ERROR_SUCCESS) };
        // SAFETY: Das Clipboard ist geöffnet; `current` ist 0 oder die zuletzt
        // gelieferte ID.
        let next = unsafe { EnumClipboardFormats(current) };
        if next == 0 {
            // SAFETY: parameterlos, direkt nach dem Aufruf.
            let err = unsafe { GetLastError() };
            if err != ERROR_SUCCESS {
                return Err(err);
            }
        }
        Ok(next)
    })?;
    // Namen erst nach der Enumeration; das Clipboard ist weiter offen.
    Ok(ids
        .into_iter()
        .map(|id| Enumerated {
            id,
            raw_name: registered_name(id),
        })
        .collect())
}

/// Bytes eines HGLOBAL-Formats, nur nach geprüfter Speicherklassifikation
/// (`GlobalSize > 0`, `GlobalLock` gelingt). Nie für GDI-Handles.
fn copy_global(handle: HGLOBAL, remaining: usize) -> Result<Vec<u8>, LossReason> {
    // SAFETY: `handle` stammt aus `GetClipboardData` für ein Format der
    // HGLOBAL-Klasse bei geöffnetem Clipboard und ist bis zum `CloseClipboard`
    // gültig. `GlobalSize` liest nur.
    let size = unsafe { GlobalSize(handle) };
    if size == 0 {
        return Err(LossReason::Unreadable);
    }
    if size > remaining {
        return Err(LossReason::ByteBudget);
    }
    // SAFETY: wie oben; `GlobalLock` liefert einen für `size` Bytes gültigen
    // Zeiger oder `NULL`.
    let ptr = unsafe { GlobalLock(handle) } as *const u8;
    if ptr.is_null() {
        return Err(LossReason::Unreadable);
    }
    // SAFETY: `size` Bytes ab `ptr` sind lesbar, solange die Sperre steht; der
    // Slice wird vor dem `GlobalUnlock` vollständig kopiert.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, size) }.to_vec();
    // SAFETY: genau ein `GlobalLock` wird zurückgenommen.
    unsafe { GlobalUnlock(handle) };
    Ok(bytes)
}

/// Bytes eines `CF_ENHMETAFILE`. Das Quellhandle gehört weiter dem Clipboard.
fn copy_emf(handle: HENHMETAFILE, remaining: usize) -> Result<Vec<u8>, LossReason> {
    // SAFETY: `handle` ist das EMF aus `GetClipboardData(CF_ENHMETAFILE)` bei
    // geöffnetem Clipboard; mit `NULL`-Puffer liefert die API nur die Größe.
    let size = unsafe { GetEnhMetaFileBits(handle, 0, ptr::null_mut()) };
    if size == 0 {
        return Err(LossReason::Unreadable);
    }
    let len = usize::try_from(size).map_err(|_| LossReason::Unreadable)?;
    if len > remaining {
        return Err(LossReason::ByteBudget);
    }
    let mut bytes = vec![0_u8; len];
    // SAFETY: `bytes` hat genau `size` beschreibbare Bytes, und genau das
    // steht im Größenparameter. Das Handle bleibt gültig.
    let copied = unsafe { GetEnhMetaFileBits(handle, size, bytes.as_mut_ptr()) };
    if copied != size {
        return Err(LossReason::Unreadable);
    }
    Ok(bytes)
}

/// Ein kopierbares Format lesen. `GetClipboardData` kann bei der Quelle
/// synchron rendern und lässt sich nicht abbrechen (Leitentscheidung 2).
fn read_format(id: u32, kind: DataKind, remaining: usize) -> Result<Vec<u8>, LossReason> {
    // SAFETY: Das Clipboard ist geöffnet; das Handle gehört dem Clipboard und
    // wird nur bis zum `CloseClipboard` gelesen, nie freigegeben.
    let handle = unsafe { GetClipboardData(id) };
    if handle.is_null() {
        return Err(LossReason::NoData);
    }
    match kind {
        DataKind::Global => copy_global(handle as HGLOBAL, remaining),
        DataKind::Emf => copy_emf(handle as HENHMETAFILE, remaining),
    }
}

type OpenRead = (SnapshotKind, Vec<FormatRow>, Vec<RawFormat>);

/// Der Teil des Snapshots, der ein **offenes** Clipboard voraussetzt (§7.1.1).
/// `Err` trägt den Win32-Code von `CountClipboardFormats` bzw.
/// `EnumClipboardFormats`.
fn read_open_clipboard(started: Instant) -> Result<OpenRead, u32> {
    // Wirklich leer ist nur, was gar kein Format mehr trägt. `0` ist auch der
    // Fehlerwert — daher der Fehlercode.
    // SAFETY: parameterlos; setzt nur den Fehlercode dieses Threads.
    unsafe { SetLastError(ERROR_SUCCESS) };
    // SAFETY: parameterlos, Clipboard ist offen.
    let count = unsafe { CountClipboardFormats() };
    if count <= 0 {
        // SAFETY: parameterlos, direkt nach dem Aufruf.
        let err = unsafe { GetLastError() };
        if err != ERROR_SUCCESS {
            return Err(err);
        }
        return Ok((SnapshotKind::Empty, Vec::new(), Vec::new()));
    }
    let entries = enumerate_open_clipboard()?;
    let collected = formats::collect(
        &entries,
        MAX_SNAPSHOT_BYTES,
        MAX_SNAPSHOT_TIME,
        || started.elapsed(),
        |entry, kind, remaining| read_format(entry.id, kind, remaining),
    );
    let format_count = usize::try_from(count).unwrap_or(entries.len());
    let kind = formats::snapshot_kind(format_count.max(entries.len()), &collected.rows);
    let raw = collected
        .saved
        .into_iter()
        .map(|saved| RawFormat {
            format: saved.format,
            kind: saved.kind,
            useful: saved.useful,
            data: Rc::new(saved.bytes),
        })
        .collect();
    Ok((kind, collected.rows, raw))
}

/// §7.3: `NULL` (Secure Desktop, gesperrter Bildschirm, Fokus im Nirgendwo)
/// zählt als Fokusverlust. Kein `AttachThreadInput` — `GetForegroundWindow`
/// braucht keines (Sol-Review).
fn foreground_window() -> Option<WindowId> {
    // SAFETY: parameterlos, threadunabhängig.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        None
    } else {
        Some(WindowId(hwnd as usize as u64))
    }
}

/// RAII um ein Prozess-Handle — ohne das leckt jede Shortcut-Auflösung ein
/// Kernel-Handle (Sol-Review).
struct ProcessHandle(HANDLE);

impl ProcessHandle {
    fn open(pid: u32) -> Option<Self> {
        // SAFETY: `PROCESS_QUERY_LIMITED_INFORMATION` ist das schwächste Recht,
        // das `QueryFullProcessImageNameW` braucht, und funktioniert auch über
        // Integritätsgrenzen hinweg. Der Rückgabewert wird sofort geprüft.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            None
        } else {
            Some(Self(handle))
        }
    }

    fn image_path(&self) -> Option<String> {
        // `MAX_PATH` reicht fast immer; der Puffer wächst, bis der
        // Windows-Pfadgrenzwert erreicht ist.
        let mut cap = 260_usize;
        loop {
            let mut buf = vec![0_u16; cap];
            let mut len = u32::try_from(cap).ok()?;
            // SAFETY: `self.0` ist ein lebendes Prozess-Handle, `buf` hat
            // `cap` beschreibbare `u16`, und `len` sagt der API genau das.
            let ok = unsafe {
                QueryFullProcessImageNameW(self.0, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len)
            };
            if ok != 0 {
                let end = usize::try_from(len).ok()?.min(buf.len());
                return Some(utf16_until_nul(&buf[..end]));
            }
            if cap >= 32768 {
                return None;
            }
            cap *= 2;
        }
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        // SAFETY: Handle aus einem erfolgreichen `OpenProcess`, wird genau
        // einmal geschlossen.
        unsafe { CloseHandle(self.0) };
    }
}

fn process_basename(window: WindowId) -> Option<String> {
    let hwnd = window.0 as usize as HWND;
    let mut pid: u32 = 0;
    // SAFETY: `hwnd` wird nur an Win32 zurückgereicht, nie dereferenziert; ein
    // inzwischen zerstörtes Fenster liefert 0 und keinen Fehler. `pid` ist ein
    // gültiger, beschreibbarer `u32`.
    let thread = unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if thread == 0 || pid == 0 {
        return None;
    }
    let path = ProcessHandle::open(pid)?.image_path()?;
    let name = basename(&path);
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// High-Bit von `GetAsyncKeyState`: die Taste ist gerade physisch unten (§7.1).
fn key_is_down(vk: u16) -> bool {
    // SAFETY: nimmt den VK als Wert, schreibt nichts, liefert für unbekannte
    // Codes 0.
    let state = unsafe { GetAsyncKeyState(i32::from(vk)) };
    (state as u16 & 0x8000) != 0
}

/// `PasteKey` → Virtual-Key plus die Flags, die diese Taste zusätzlich braucht.
fn virtual_key(key: PasteKey) -> (u16, u32) {
    match key {
        PasteKey::Shift => (VK_SHIFT, 0),
        PasteKey::Alt => (VK_MENU, 0),
        PasteKey::Super => (VK_LWIN, 0),
        PasteKey::Ctrl => (VK_CONTROL, 0),
        PasteKey::V => (VK_V, 0),
        // `Insert` ist eine Extended Key; ohne das Flag landet auf manchen
        // Layouts der Ziffernblock-Insert (Sol-Review).
        PasteKey::Insert => (VK_INSERT, KEYEVENTF_EXTENDEDKEY),
    }
}

/// Ein Tastenereignis. Der Rückgabewert von `SendInput` wird exakt geprüft —
/// UIPI verschluckt Events, ohne `GetLastError` zuverlässig zu setzen. Das
/// Lösen der bereits gedrückten Tasten übernimmt das Protokoll
/// (`protocol::chord_*` löst in umgekehrter Reihenfolge), sobald hier ein
/// Fehler zurückkommt.
fn send_key(key: PasteKey, down: bool) -> Result<(), InjectError> {
    let (vk, extra) = virtual_key(key);
    let mut flags = extra;
    if !down {
        flags |= KEYEVENTF_KEYUP;
    }
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    // SAFETY: genau ein vollständig initialisiertes `INPUT` mit der von der
    // API erwarteten Strukturgröße; `SendInput` liest nur.
    let sent = unsafe { SendInput(1, &input, i32::try_from(size_of::<INPUT>()).unwrap_or(0)) };
    if sent != 1 {
        // SAFETY: parameterlos.
        let err = unsafe { GetLastError() };
        let dir = if down { "down" } else { "up" };
        return Err(InjectError::Failed(format!(
            "SendInput {key:?} {dir}: {sent} von 1 Events, Win32-Fehler {err}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_round_trip_keeps_umlauts_and_stops_at_nul() {
        let text = "Grüße, Jörg – Zeile eins";
        let units = to_utf16_nul(text);
        assert_eq!(units.last(), Some(&0));
        assert_eq!(utf16_until_nul(&units), text);
        // Was hinter der NUL steht, gehört nicht mehr zum Text — Windows
        // liefert `GlobalSize` in Blöcken, nicht auf das Byte genau.
        let mut padded = units.clone();
        padded.extend_from_slice(&[0x41, 0x42, 0x00]);
        assert_eq!(utf16_until_nul(&padded), text);
    }

    #[test]
    fn utf16_handles_emoji_and_lone_surrogates() {
        let text = "Zeile\n📋 fertig";
        assert_eq!(utf16_until_nul(&to_utf16_nul(text)), text);
        // Unpaariges High-Surrogate: `from_utf16_lossy` ersetzt, statt zu
        // panicken oder den Rest zu verwerfen.
        let broken = [0x0041_u16, 0xD800, 0x0042, 0x0000];
        let out = utf16_until_nul(&broken);
        assert!(out.starts_with('A'));
        assert!(out.ends_with('B'));
    }

    #[test]
    fn utf16_without_nul_is_read_completely() {
        let units: Vec<u16> = "abc".encode_utf16().collect();
        assert_eq!(utf16_until_nul(&units), "abc");
        assert_eq!(utf16_until_nul(&[]), "");
    }

    #[test]
    fn basename_takes_the_last_path_component() {
        assert_eq!(
            basename(r"C:\Program Files\WindowsApps\WindowsTerminal.exe"),
            "WindowsTerminal.exe"
        );
        assert_eq!(basename(r"C:\Windows\System32\notepad.exe"), "notepad.exe");
        assert_eq!(basename("C:/tmp/mixed/sep.exe"), "sep.exe");
        assert_eq!(basename("notepad.exe"), "notepad.exe");
        assert_eq!(basename(r"C:\ends\with\slash\"), "");
    }

    #[test]
    fn paste_keys_map_to_the_exact_virtual_keys() {
        assert_eq!(virtual_key(PasteKey::Shift), (VK_SHIFT, 0));
        assert_eq!(virtual_key(PasteKey::Alt), (VK_MENU, 0));
        assert_eq!(virtual_key(PasteKey::Super), (VK_LWIN, 0));
        assert_eq!(virtual_key(PasteKey::Ctrl), (VK_CONTROL, 0));
        assert_eq!(virtual_key(PasteKey::V), (0x56, 0));
        // Die einzige Taste mit Zusatzflag.
        assert_eq!(
            virtual_key(PasteKey::Insert),
            (VK_INSERT, KEYEVENTF_EXTENDEDKEY)
        );
    }

    #[test]
    fn only_insert_is_extended() {
        for key in [
            PasteKey::Shift,
            PasteKey::Alt,
            PasteKey::Super,
            PasteKey::Ctrl,
            PasteKey::V,
        ] {
            assert_eq!(virtual_key(key).1, 0, "{key:?}");
        }
    }

    #[test]
    fn clipboard_format_constant_matches_winuser() {
        // `CF_UNICODETEXT` aus `winuser.h`; hier hartkodiert, um das
        // COM-Feature `Win32_System_Ole` zu sparen.
        assert_eq!(CF_UNICODETEXT, 13);
    }

    #[test]
    fn wide_strings_are_nul_terminated() {
        let w = wide("ab");
        assert_eq!(w, vec![0x61, 0x62, 0x00]);
        assert_eq!(wide(""), vec![0x00]);
    }

    #[test]
    fn utf16_bytes_are_little_endian_with_nul() {
        assert_eq!(utf16_bytes("aü"), vec![0x61, 0x00, 0xFC, 0x00, 0x00, 0x00]);
        assert_eq!(utf16_bytes(""), vec![0x00, 0x00]);
    }

    /// Windows-Integrationstests aus clipboard-restore-plan WP1 / Gate G1.
    ///
    /// **Achtung: Diese Tests überschreiben die echte Zwischenablage** des
    /// angemeldeten Benutzers. Deshalb `#[ignore]`; sie laufen nur manuell,
    /// seriell und mit ausdrücklicher Zustimmung:
    ///
    /// `cargo test clipboard_live_ -- --ignored --test-threads=1`
    ///
    /// Ein eigenes Owner-Fenster mit Pump (derselbe Thread wie der Sink, damit
    /// `WM_RENDERFORMAT` synchron ankommt) legt die Fixtures ab. Jeder Test:
    /// Snapshot → Transkript → Restore → Vergleich von Formatliste (IDs,
    /// Reihenfolge) und Bytes je gesichertem Format, dazu die erwarteten
    /// Verlust- und Ersetzt-Listen.
    mod live {
        use super::*;
        use windows_sys::Win32::Graphics::Gdi::{CloseEnhMetaFile, CreateEnhMetaFileW, Rectangle};

        const FIXTURE_CLASS: &str = "DiktierClipboardFixture";
        const TRANSCRIPT: &str = "Transkript aus dem Live-Test";

        /// Was das Fixture-Fenster bei `WM_RENDERFORMAT` tut.
        #[derive(Clone)]
        enum Render {
            Data(Vec<u8>),
            /// Rendert nichts → `GetClipboardData == NULL`.
            Null,
            /// Hängt so lange, dann Daten (Beleg für B5).
            Block(Duration, Vec<u8>),
        }

        enum Fixture {
            Global(u32, Vec<u8>),
            Emf,
            Delayed(u32, Render),
        }

        thread_local! {
            static RENDERS: RefCell<Vec<(u32, Render)>> = const { RefCell::new(Vec::new()) };
        }

        unsafe extern "system" fn fixture_proc(
            hwnd: HWND,
            msg: u32,
            wparam: WPARAM,
            lparam: LPARAM,
        ) -> LRESULT {
            if msg == WM_RENDERFORMAT {
                let id = wparam as u32;
                let render = RENDERS.with(|renders| {
                    renders
                        .borrow()
                        .iter()
                        .find(|(format, _)| *format == id)
                        .map(|(_, render)| render.clone())
                });
                let bytes = match render {
                    Some(Render::Data(bytes)) => Some(bytes),
                    Some(Render::Block(wait, bytes)) => {
                        std::thread::sleep(wait);
                        Some(bytes)
                    }
                    Some(Render::Null) | None => None,
                };
                if let Some(handle) = bytes.as_deref().and_then(alloc_bytes) {
                    // Während `WM_RENDERFORMAT` ohne `OpenClipboard`.
                    let _ = place(id, Prepared::Global(handle));
                }
                return 0;
            }
            // SAFETY: unveränderte Parameter an die Default-Behandlung.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        /// Fremder Owner für die Fixtures (Message-only, kein Fokus).
        struct FixtureOwner {
            hwnd: HWND,
            instance: HINSTANCE,
            class_name: Vec<u16>,
        }

        impl FixtureOwner {
            fn new() -> Self {
                let class_name = wide(FIXTURE_CLASS);
                // SAFETY: wie in `Win32OutputSink::new`.
                let instance = unsafe { GetModuleHandleW(ptr::null()) };
                let class = WNDCLASSW {
                    style: 0,
                    lpfnWndProc: Some(fixture_proc),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: instance,
                    hIcon: ptr::null_mut(),
                    hCursor: ptr::null_mut(),
                    hbrBackground: ptr::null_mut(),
                    lpszMenuName: ptr::null(),
                    lpszClassName: class_name.as_ptr(),
                };
                // SAFETY: `class` ist vollständig initialisiert; ein zweites
                // Registrieren im selben Prozess scheitert harmlos.
                unsafe { RegisterClassW(&class) };
                // SAFETY: gültige, NUL-terminierte Puffer; Message-only-Fenster.
                let hwnd = unsafe {
                    CreateWindowExW(
                        0,
                        class_name.as_ptr(),
                        class_name.as_ptr(),
                        0,
                        0,
                        0,
                        0,
                        0,
                        HWND_MESSAGE,
                        ptr::null_mut(),
                        instance,
                        ptr::null(),
                    )
                };
                assert!(!hwnd.is_null(), "Fixture-Fenster nicht erzeugbar");
                Self {
                    hwnd,
                    instance,
                    class_name,
                }
            }

            fn place(&self, fixtures: Vec<Fixture>) {
                RENDERS.with(|renders| renders.borrow_mut().clear());
                let mut opened = false;
                for _ in 0..50 {
                    // SAFETY: eigenes, lebendes Fenster.
                    if unsafe { OpenClipboard(self.hwnd) } != 0 {
                        opened = true;
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                assert!(opened, "Clipboard nicht zu öffnen");
                // SAFETY: Clipboard ist von `self.hwnd` geöffnet.
                assert_ne!(unsafe { EmptyClipboard() }, 0);
                for fixture in fixtures {
                    match fixture {
                        Fixture::Global(id, bytes) => {
                            let handle = alloc_bytes(&bytes).expect("GlobalAlloc");
                            place(id, Prepared::Global(handle)).expect("SetClipboardData");
                        }
                        Fixture::Emf => {
                            // SAFETY: Referenz-DC `NULL`, kein Dateiname, kein
                            // Rahmen, keine Beschreibung — ein Speicher-EMF.
                            let dc = unsafe {
                                CreateEnhMetaFileW(
                                    ptr::null_mut(),
                                    ptr::null(),
                                    ptr::null(),
                                    ptr::null(),
                                )
                            };
                            assert!(!dc.is_null());
                            // SAFETY: `dc` ist der gerade erzeugte Metafile-DC.
                            unsafe { Rectangle(dc, 1, 1, 40, 20) };
                            // SAFETY: schließt genau diesen DC, liefert das EMF.
                            let emf = OwnedEmf(unsafe { CloseEnhMetaFile(dc) });
                            assert!(!emf.0.is_null());
                            place(formats::CF_ENHMETAFILE, Prepared::Emf(emf))
                                .expect("SetClipboardData(EMF)");
                        }
                        Fixture::Delayed(id, render) => {
                            RENDERS.with(|renders| renders.borrow_mut().push((id, render)));
                            // SAFETY: Delayed Rendering, kein Handle.
                            unsafe { SetClipboardData(id, ptr::null_mut()) };
                        }
                    }
                }
                // SAFETY: genau das oben geöffnete Clipboard.
                unsafe { CloseClipboard() };
            }
        }

        impl Drop for FixtureOwner {
            fn drop(&mut self) {
                // SAFETY: eigenes Fenster dieses Threads, eigene Klasse.
                unsafe {
                    DestroyWindow(self.hwnd);
                    UnregisterClassW(self.class_name.as_ptr(), self.instance);
                }
            }
        }

        fn registered(name: &str) -> u32 {
            let wide_name = wide(name);
            // SAFETY: NUL-terminiert, lebt über den Aufruf.
            let id = unsafe { RegisterClipboardFormatW(wide_name.as_ptr()) };
            assert_ne!(id, 0, "{name}");
            id
        }

        struct Outcome {
            snapshot: ClipboardSnapshot,
            result: RestoreResult,
            saved: Vec<(FormatRef, Vec<u8>)>,
            after: Vec<(FormatRef, Option<Vec<u8>>)>,
            unchecked: Vec<u32>,
            known: Vec<u32>,
        }

        impl Outcome {
            fn saved_ids(&self) -> Vec<u32> {
                self.saved.iter().map(|(f, _)| f.id).collect()
            }

            fn saved_bytes(&self, id: u32) -> &[u8] {
                &self
                    .saved
                    .iter()
                    .find(|(f, _)| f.id == id)
                    .unwrap_or_else(|| panic!("0x{id:04X} nicht gesichert"))
                    .1
            }

            fn lost(&self) -> Vec<(u32, LossReason)> {
                self.snapshot
                    .report
                    .lost()
                    .into_iter()
                    .map(|l| (l.format.id, l.reason))
                    .collect()
            }

            fn replaced(&self) -> Vec<u32> {
                self.snapshot
                    .report
                    .replaced()
                    .into_iter()
                    .map(|f| f.id)
                    .collect()
            }

            /// Nutzdaten byte-identisch in Originalreihenfolge, und jede als
            /// „ersetzt“ gemeldete ID ist nach dem Restore wieder verfügbar
            /// (synthetisiert, Sol-Impl-Review).
            fn assert_identical(&self) {
                let mismatches = formats::compare_roundtrip(
                    &self.saved,
                    &self.after,
                    &self.known,
                    &self.unchecked,
                );
                assert!(
                    self.unchecked.is_empty(),
                    "nicht geprüft: {:04X?}",
                    self.unchecked
                );
                assert!(mismatches.is_empty(), "{mismatches:#?}");
                let after_ids: Vec<u32> = self.after.iter().map(|(f, _)| f.id).collect();
                for id in self.replaced() {
                    assert!(
                        after_ids.contains(&id),
                        "ersetzt gemeldet, aber nicht verfügbar: 0x{id:04X} in {after_ids:04X?}"
                    );
                }
            }
        }

        fn roundtrip(fixtures: Vec<Fixture>) -> Outcome {
            let owner = FixtureOwner::new();
            owner.place(fixtures);
            let mut sink = Win32OutputSink::new(OutputConfig::default()).expect("Sink");
            let snapshot = sink.snapshot_clipboard().expect("Snapshot");
            eprintln!("{}", formats::snapshot_log_line(&snapshot.report));
            let saved: Vec<(FormatRef, Vec<u8>)> = sink
                .stash
                .iter()
                .flatten()
                .map(|raw| (raw.format.clone(), raw.data.as_ref().clone()))
                .collect();
            sink.become_owner(TRANSCRIPT.into()).expect("Transkript");
            let result = sink.restore_snapshot(&snapshot, TRANSCRIPT);
            let saved_ids: Vec<(u32, usize)> = saved.iter().map(|(f, b)| (f.id, b.len())).collect();
            let read = sink.read_after(&saved_ids).expect("erneut lesen");
            let unchecked = read.unchecked.iter().map(|f| f.id).collect();
            let after = read.formats;
            let known = snapshot
                .report
                .rows
                .iter()
                .map(|row| row.format.id)
                .chain(sink.marker)
                .collect();
            drop(owner);
            Outcome {
                snapshot,
                result,
                saved,
                after,
                unchecked,
                known,
            }
        }

        fn starts_with(haystack: &[u8], needle: &[u8]) -> bool {
            haystack.len() >= needle.len() && &haystack[..needle.len()] == needle
        }

        #[test]
        #[ignore = "überschreibt die echte Zwischenablage"]
        fn clipboard_live_text_locale_dsptext() {
            let text = utf16_bytes("Grüße, Jörg");
            let out = roundtrip(vec![
                Fixture::Global(CF_UNICODETEXT, text.clone()),
                Fixture::Global(formats::CF_LOCALE, 0x0407_u32.to_le_bytes().to_vec()),
                Fixture::Global(formats::CF_DSPTEXT, b"Anzeige\0".to_vec()),
            ]);
            assert!(
                matches!(out.result, RestoreResult::Restored),
                "{:?}",
                out.result
            );
            for id in [CF_UNICODETEXT, formats::CF_LOCALE, formats::CF_DSPTEXT] {
                assert!(out.saved_ids().contains(&id), "0x{id:04X}");
            }
            assert!(starts_with(out.saved_bytes(CF_UNICODETEXT), &text));
            assert!(out.lost().is_empty(), "{:?}", out.lost());
            out.assert_identical();
        }

        #[test]
        #[ignore = "überschreibt die echte Zwischenablage"]
        fn clipboard_live_html() {
            let html = registered("HTML Format");
            let body = b"Version:0.9\r\nStartHTML:0\r\n<b>fett</b>\0".to_vec();
            let out = roundtrip(vec![
                Fixture::Global(CF_UNICODETEXT, utf16_bytes("fett")),
                Fixture::Global(html, body.clone()),
            ]);
            assert!(
                matches!(out.result, RestoreResult::Restored),
                "{:?}",
                out.result
            );
            assert!(starts_with(out.saved_bytes(html), &body));
            out.assert_identical();
        }

        /// 2×2 Pixel, 32 bpp, `BITMAPV5HEADER` (124 Bytes) plus Bits.
        fn dibv5() -> Vec<u8> {
            let mut bytes = Vec::new();
            let push = |bytes: &mut Vec<u8>, v: u32| bytes.extend_from_slice(&v.to_le_bytes());
            push(&mut bytes, 124); // bV5Size
            push(&mut bytes, 2); // bV5Width
            push(&mut bytes, 2); // bV5Height
            bytes.extend_from_slice(&1_u16.to_le_bytes()); // bV5Planes
            bytes.extend_from_slice(&32_u16.to_le_bytes()); // bV5BitCount
            push(&mut bytes, 0); // BI_RGB
            push(&mut bytes, 16); // bV5SizeImage
            push(&mut bytes, 2835); // XPelsPerMeter
            push(&mut bytes, 2835); // YPelsPerMeter
            push(&mut bytes, 0); // ClrUsed
            push(&mut bytes, 0); // ClrImportant
            for _ in 0..4 {
                push(&mut bytes, 0); // Masken R, G, B, A
            }
            push(&mut bytes, 0x7352_4742); // LCS_sRGB
            bytes.extend_from_slice(&[0; 36]); // CIEXYZTRIPLE
            for _ in 0..3 {
                push(&mut bytes, 0); // Gamma R, G, B
            }
            push(&mut bytes, 4); // LCS_GM_IMAGES
            push(&mut bytes, 0); // ProfileData
            push(&mut bytes, 0); // ProfileSize
            push(&mut bytes, 0); // Reserved
            assert_eq!(bytes.len(), 124);
            bytes.extend_from_slice(&[0x10, 0x20, 0x30, 0xFF].repeat(4));
            bytes
        }

        #[test]
        #[ignore = "überschreibt die echte Zwischenablage"]
        fn clipboard_live_dibv5() {
            let image = dibv5();
            let out = roundtrip(vec![Fixture::Global(formats::CF_DIBV5, image.clone())]);
            assert!(
                matches!(
                    out.result,
                    RestoreResult::Restored | RestoreResult::RestoredPartial { .. }
                ),
                "{:?}",
                out.result
            );
            assert!(starts_with(out.saved_bytes(formats::CF_DIBV5), &image));
            // Windows bietet CF_BITMAP synthetisiert an; es wird ersetzt.
            assert!(
                out.replaced().contains(&formats::CF_BITMAP),
                "{:?}",
                out.replaced()
            );
            out.assert_identical();
        }

        #[test]
        #[ignore = "überschreibt die echte Zwischenablage"]
        fn clipboard_live_emf() {
            let out = roundtrip(vec![Fixture::Emf]);
            assert!(
                matches!(out.result, RestoreResult::Restored),
                "{:?}",
                out.result
            );
            assert!(out.saved_ids().contains(&formats::CF_ENHMETAFILE));
            assert!(
                out.replaced().contains(&formats::CF_METAFILEPICT),
                "{:?}",
                out.replaced()
            );
            out.assert_identical();
        }

        /// `DROPFILES` (20 Bytes, `fWide`) plus zwei Pfade, doppelt NUL.
        fn hdrop() -> Vec<u8> {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&20_u32.to_le_bytes()); // pFiles
            bytes.extend_from_slice(&[0; 8]); // pt
            bytes.extend_from_slice(&0_u32.to_le_bytes()); // fNC
            bytes.extend_from_slice(&1_u32.to_le_bytes()); // fWide
            for path in [r"C:\diktier-test\a.txt", r"C:\diktier-test\b.txt"] {
                bytes.extend(utf16_bytes(path));
            }
            bytes.extend_from_slice(&[0, 0]);
            bytes
        }

        #[test]
        #[ignore = "überschreibt die echte Zwischenablage"]
        fn clipboard_live_hdrop_and_drop_effect() {
            let effect = registered("Preferred DropEffect");
            let drop_files = hdrop();
            let out = roundtrip(vec![
                Fixture::Global(formats::CF_HDROP, drop_files.clone()),
                // DROPEFFECT_COPY
                Fixture::Global(effect, 1_u32.to_le_bytes().to_vec()),
            ]);
            assert!(
                matches!(out.result, RestoreResult::Restored),
                "{:?}",
                out.result
            );
            assert!(starts_with(out.saved_bytes(formats::CF_HDROP), &drop_files));
            assert!(starts_with(out.saved_bytes(effect), &1_u32.to_le_bytes()));
            out.assert_identical();
        }

        #[test]
        #[ignore = "überschreibt die echte Zwischenablage"]
        fn clipboard_live_delayed_and_null_render() {
            let delayed = registered("Diktier Live Delayed");
            let null = registered("Diktier Live Null");
            let payload = b"spaet gerendert".to_vec();
            let out = roundtrip(vec![
                Fixture::Global(CF_UNICODETEXT, utf16_bytes("Text")),
                Fixture::Delayed(delayed, Render::Data(payload.clone())),
                Fixture::Delayed(null, Render::Null),
            ]);
            assert!(starts_with(out.saved_bytes(delayed), &payload));
            assert_eq!(out.lost(), vec![(null, LossReason::NoData)]);
            match &out.result {
                RestoreResult::RestoredPartial {
                    lost_save,
                    lost_restore,
                } => {
                    assert_eq!(lost_save.len(), 1);
                    assert!(lost_restore.is_empty());
                }
                other => panic!("{other:?}"),
            }
            out.assert_identical();
        }

        /// B5: eine Quelle blockiert `WM_RENDERFORMAT` 3 s. Die Dauer steht im
        /// Report, die restlichen Formate sind nach dem Zeitbudget verloren.
        #[test]
        #[ignore = "überschreibt die echte Zwischenablage"]
        fn clipboard_live_blocking_source_hits_the_time_budget() {
            let slow = registered("Diktier Live Block");
            let later = registered("Diktier Live Danach");
            let out = roundtrip(vec![
                Fixture::Global(CF_UNICODETEXT, utf16_bytes("Text")),
                Fixture::Delayed(
                    slow,
                    Render::Block(Duration::from_secs(3), b"langsam".to_vec()),
                ),
                Fixture::Global(later, b"danach".to_vec()),
            ]);
            assert!(out.snapshot.report.duration >= Duration::from_secs(3));
            // Schon angefordert: bleibt gesichert (weiches Limit).
            assert!(out.saved_ids().contains(&slow));
            assert!(
                out.lost().contains(&(later, LossReason::TimeBudget)),
                "{:?}",
                out.lost()
            );
            assert!(matches!(out.result, RestoreResult::RestoredPartial { .. }));
        }

        /// G3 #8 als Test: leeres Clipboard über den Test-Owner.
        #[test]
        #[ignore = "überschreibt die echte Zwischenablage"]
        fn clipboard_live_empty() {
            let out = roundtrip(Vec::new());
            assert_eq!(out.snapshot.kind, SnapshotKind::Empty);
            assert!(matches!(out.result, RestoreResult::Restored));
            assert!(out.after.is_empty(), "{:?}", out.after);
        }
    }
}
