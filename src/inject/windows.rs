//! Win32-OutputSink: Message-only-Fenster als Clipboard-Owner + `SendInput`
//! (Spec §7, windows-plan WP3).
//!
//! Das Gegenstück zu [`super::linux`]. Zwei Dinge sind auf Windows anders und
//! prägen den ganzen Modul-Aufbau:
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

use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_CLASS_ALREADY_EXISTS, GetLastError, GlobalFree, HANDLE, HGLOBAL, HINSTANCE,
    HWND, LPARAM, LRESULT, WPARAM,
};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, CountClipboardFormats, EmptyClipboard, GetClipboardData, GetClipboardOwner,
    GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard, SetClipboardData,
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

use super::protocol::{
    ClipboardHost, ClipboardSnapshot, ModifierState, PumpEvents, apply_leading_space, inject_paste,
    serve_restored_until_read,
};
use super::{
    CaptureContext, ClipboardSave, InjectError, InjectOutcome, OutputSink, PasteKey, WindowId,
};

/// `CF_UNICODETEXT` liegt in windows-sys 0.61 unter `Win32_System_Ole` — ein
/// COM-Feature, von dem hier sonst nichts gebraucht wird. Der Wert ist seit
/// Windows NT 3.1 Teil der stabilen ABI (`winuser.h`), die Konstante direkt zu
/// setzen ist billiger als ein zusätzlicher Feature-Baum.
const CF_UNICODETEXT: u32 = 13;

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
    /// Clipboard-Inhalt noch zu ändern (Restore, Quit-Pfad).
    Eager,
    /// Gar nichts — ein wirklich leeres Clipboard (`CountClipboardFormats()==0`).
    Empty,
}

/// Warum eine Übernahme unterblieb. Die Unterscheidung ist nötig, weil die
/// Aufrufer verschieden reagieren: ein fremder Copy ist kein Fehler, sondern
/// §7.1 Punkt 5 („niemals restaurieren“), ein Win32-Fehler dagegen schon.
enum TakeFailure {
    /// Fremde Änderung zwischen Prüfung/Snapshot und `EmptyClipboard`. Das
    /// Clipboard wurde **nicht** angefasst.
    Foreign,
    /// Win32-Fehler beim eigenen Übergang.
    Failed(InjectError),
}

impl TakeFailure {
    fn into_error(self) -> InjectError {
        match self {
            Self::Foreign => {
                InjectError::Failed("Clipboard zwischenzeitlich fremd geändert".into())
            }
            Self::Failed(err) => err,
        }
    }
}

/// Der Zustand, den `WndProc` und die Sink-Methoden teilen. Beide laufen auf
/// **demselben** Thread (dem Inject-Worker), deshalb reicht `RefCell`: der
/// `WndProc` wird nur aus `PeekMessageW`/`DispatchMessageW`/`SendMessage`
/// dieses Threads aufgerufen.
struct ClipboardState {
    /// Text, den `WM_RENDERFORMAT` liefert bzw. der zuletzt eager gesetzt wurde.
    serve: String,
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
}

impl ClipboardState {
    fn new() -> Self {
        Self {
            serve: String::new(),
            expected_seq: 0,
            snapshot_seq: None,
            owned: false,
            delayed: false,
            reads: 0,
            lost: false,
            guard: 0,
        }
    }

    fn forget_ownership(&mut self) {
        self.owned = false;
        self.delayed = false;
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
    // SAFETY: `handle` ist ein frisch alloziertes `GMEM_MOVEABLE`-Handle mit
    // gültigem UTF-16-NUL-Inhalt. Bei Erfolg übernimmt das System das
    // Eigentum, sonst wird es unten selbst freigegeben.
    let placed = unsafe { SetClipboardData(CF_UNICODETEXT, handle as HANDLE) };
    if placed.is_null() {
        // SAFETY: Die Übergabe ist gescheitert, das Handle gehört noch uns und
        // wurde noch nicht freigegeben.
        unsafe { GlobalFree(handle) };
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
        // SAFETY: wie in `on_render_format`; kein `EmptyClipboard`, das würde
        // die bereits vorhandenen Daten zerstören.
        let placed = unsafe { SetClipboardData(CF_UNICODETEXT, handle as HANDLE) };
        if placed.is_null() {
            // SAFETY: Übergabe gescheitert, Handle noch unseres.
            unsafe { GlobalFree(handle) };
        } else {
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

        Ok(Self {
            hwnd,
            instance,
            class_name,
            owns_class,
            state,
            output,
            start: Instant::now(),
        })
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
        for attempt in 0..OPEN_RETRIES {
            // SAFETY: `self.hwnd` ist unser lebendes Fenster.
            if unsafe { OpenClipboard(self.hwnd) } != 0 {
                return Ok(());
            }
            if attempt + 1 < OPEN_RETRIES {
                self.pump_messages(OPEN_RETRY_WAIT);
            }
        }
        // SAFETY: parameterlos.
        let err = unsafe { GetLastError() };
        Err(InjectError::Failed(format!(
            "Clipboard nicht zu öffnen ({OPEN_RETRIES} Versuche): Win32-Fehler {err}"
        )))
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
        // Vor dem Guard öffnen: `open_clipboard` pumpt, und ein fremdes
        // `WM_DESTROYCLIPBOARD` in dieser Zeit soll noch gewertet werden.
        // Scheitert das Öffnen, bleibt der bisherige Serve-Text stehen — ein
        // noch offenes altes Versprechen wird sonst mit dem neuen Text bedient.
        self.open_clipboard().map_err(TakeFailure::Failed)?;

        if let Some(expected) = expect {
            // SAFETY: beide parameterlos; das Clipboard ist offen und damit
            // gegen fremde Mutation gesperrt.
            let seq = unsafe { GetClipboardSequenceNumber() };
            let owner = unsafe { GetClipboardOwner() };
            let owned = self.state.borrow().owned;
            if seq != expected || (owned && owner != self.hwnd) {
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
        let result = fill_open_clipboard(&text, fill);
        // SAFETY: genau das oben geöffnete Clipboard wird einmal geschlossen.
        unsafe { CloseClipboard() };
        // SAFETY: beide parameterlos.
        let owner = unsafe { GetClipboardOwner() };
        let seq = unsafe { GetClipboardSequenceNumber() };

        let mut state = self.state.borrow_mut();
        state.guard -= 1;
        match result {
            Ok(()) if owner == self.hwnd => {
                // Zwischen `CloseClipboard` und hier kann kein
                // `WM_RENDERFORMAT` dazwischenkommen: gesendete Nachrichten
                // erreichen den `WndProc` erst beim nächsten Pumpen.
                state.serve = text;
                state.owned = true;
                state.delayed = fill == Fill::Delayed;
                state.expected_seq = seq;
                Ok(())
            }
            Ok(()) => {
                state.forget_ownership();
                Err(TakeFailure::Failed(InjectError::Failed(
                    "Clipboard-Ownership nicht übernommen".into(),
                )))
            }
            Err(err) => {
                state.forget_ownership();
                Err(TakeFailure::Failed(err))
            }
        }
    }

    /// Die Sequenz, gegen die eine Übernahme bei **bestehendem** Eigentum
    /// geprüft wird (nach `is_still_owner`).
    fn owned_seq(&self) -> Option<u32> {
        Some(self.state.borrow().expected_seq)
    }

    /// Quit-Pfad: ein offenes Delayed-Rendering-Versprechen stirbt mit dem
    /// Prozess. Der Text wird deshalb eager hinterlegt, damit er das
    /// Prozessende überlebt (§7.1 Punkt 8).
    fn materialize(&mut self) -> Result<(), TakeFailure> {
        let text = self.state.borrow().serve.clone();
        let expect = self.owned_seq();
        self.take_clipboard(text, Fill::Eager, expect)
    }
}

/// Der Teil, der ein **offenes** Clipboard voraussetzt.
fn fill_open_clipboard(text: &str, fill: Fill) -> Result<(), InjectError> {
    // Speicher **vor** `EmptyClipboard` besorgen (Sol-Review Blocker 3):
    // scheitert `GlobalAlloc`, sind bisheriger Inhalt und Transkript noch da.
    let handle = match fill {
        Fill::Empty | Fill::Delayed => ptr::null_mut(),
        Fill::Eager => alloc_utf16(text).ok_or_else(|| {
            InjectError::Failed("Clipboard-Speicher (GlobalAlloc) fehlgeschlagen".into())
        })?,
    };
    // SAFETY: Das Clipboard ist von unserem Fenster geöffnet. `EmptyClipboard`
    // macht es zu unserem und schickt `WM_DESTROYCLIPBOARD` an den bisherigen
    // Owner — sind das wir selbst, hält der Guard des Aufrufers die Nachricht
    // von `lost_ownership` fern.
    if unsafe { EmptyClipboard() } == 0 {
        // SAFETY: parameterlos.
        let err = unsafe { GetLastError() };
        if !handle.is_null() {
            // SAFETY: noch nicht übergeben, das Handle gehört uns.
            unsafe { GlobalFree(handle) };
        }
        return Err(InjectError::Failed(format!(
            "EmptyClipboard: Win32-Fehler {err}"
        )));
    }
    if fill == Fill::Empty {
        return Ok(());
    }
    // SAFETY: `NULL` ist die dokumentierte Form für Delayed Rendering; sonst
    // ist `handle` ein gültiges `GMEM_MOVEABLE`-Handle, dessen Eigentum bei
    // Erfolg an das System übergeht.
    let placed = unsafe { SetClipboardData(CF_UNICODETEXT, handle as HANDLE) };
    if placed.is_null() && !handle.is_null() {
        // SAFETY: Übergabe gescheitert, das Handle gehört noch uns.
        unsafe { GlobalFree(handle) };
        // SAFETY: parameterlos.
        let err = unsafe { GetLastError() };
        return Err(InjectError::Failed(format!(
            "SetClipboardData: Win32-Fehler {err}"
        )));
    }
    if fill == Fill::Delayed {
        // `SetClipboardData(_, NULL)` liefert auch bei Erfolg `NULL`, und
        // `GetClipboardOwner() == hwnd` beweist nur das `EmptyClipboard`.
        // Ob das Versprechen wirklich steht, sagt allein die
        // Formatverfügbarkeit — noch im offenen Clipboard geprüft.
        // SAFETY: parameterlos bis auf das Format, Clipboard ist offen.
        if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) } == 0 {
            // SAFETY: parameterlos.
            let err = unsafe { GetLastError() };
            return Err(InjectError::Failed(format!(
                "Delayed Rendering für CF_UNICODETEXT nicht registriert: Win32-Fehler {err}"
            )));
        }
    }
    Ok(())
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
        // vor dem Snapshot darf nicht in die neue Session hineinlecken
        // (Analog zu `drain_events` im X11-Sink).
        self.pump_messages(Duration::ZERO);
        let _ = self.take_events();

        if self.is_still_owner() {
            let mut state = self.state.borrow_mut();
            let seq = state.expected_seq;
            state.snapshot_seq = Some(seq);
            return Ok(ClipboardSnapshot::Text(state.serve.clone()));
        }

        self.open_clipboard()?;
        let snapshot = read_open_clipboard();
        // §7.1 Punkt 1: zum Snapshot gehört auf Windows die Sequenznummer.
        // Noch im offenen Clipboard gelesen, damit sie wirklich zu dem gerade
        // gelesenen Inhalt gehört.
        // SAFETY: parameterlos.
        let seq = unsafe { GetClipboardSequenceNumber() };
        // SAFETY: genau das oben geöffnete Clipboard wird einmal geschlossen.
        unsafe { CloseClipboard() };
        self.state.borrow_mut().snapshot_seq = Some(seq);
        Ok(snapshot)
    }

    fn become_owner(&mut self, text: String) -> Result<(), InjectError> {
        // `take()`: nur die Übernahme direkt nach einem Snapshot prüft gegen
        // dessen Sequenz. `copy_only` und der Fokusverlust-Pfad übernehmen
        // ohne Restore-Versprechen und ohne vorherigen Snapshot.
        let expect = self.state.borrow_mut().snapshot_seq.take();
        self.take_clipboard(text, Fill::Delayed, expect)
            .map_err(TakeFailure::into_error)
    }

    fn still_owner(&mut self) -> Result<bool, InjectError> {
        Ok(self.is_still_owner())
    }

    /// Restore eines **nicht leeren** Snapshots. Anders als auf X11 genügt es
    /// nicht, den Serve-Text zu tauschen: nach `WM_RENDERFORMAT` liegt das
    /// Transkript als echte Daten im Clipboard, ein späterer Leser fragt uns
    /// nicht mehr. Der Text muss deshalb eager materialisiert werden.
    fn set_serve_text(&mut self, text: String) {
        if !self.is_still_owner() {
            self.state.borrow_mut().serve = text;
            return;
        }
        // Das Trait hat hier keinen Fehlerkanal. Schlägt die Materialisierung
        // fehl oder war der Copy fremd, gilt der Zustand als unbekannt
        // (`take_clipboard` setzt `owned = false`); `still_owner()` meldet dann
        // false und `inject_paste` bucht `ForeignOwner` statt eines Restores,
        // das nicht stattgefunden hat. Still verschluckt wird der Grund nicht.
        let expect = self.owned_seq();
        match self.take_clipboard(text, Fill::Eager, expect) {
            Ok(()) => {}
            Err(TakeFailure::Foreign) => {
                eprintln!("Clipboard-Restore unterblieben: fremde Änderung seit der Übernahme");
            }
            Err(TakeFailure::Failed(err)) => {
                eprintln!("Clipboard-Restore fehlgeschlagen: {err}");
            }
        }
    }

    /// Restore eines **leeren** Snapshots. Windows kennt kein Ablegen der
    /// Ownership; das Gegenstück ist ein wirklich leeres Clipboard, das der
    /// nächste Snapshot wieder als `Text("")` liest.
    fn release_ownership(&mut self) -> Result<(), InjectError> {
        if !self.is_still_owner() {
            return Ok(());
        }
        let expect = self.owned_seq();
        match self.take_clipboard(String::new(), Fill::Empty, expect) {
            Ok(()) => Ok(()),
            // Fremder Copy zwischen Prüfung und `EmptyClipboard`: §7.1 Punkt 5
            // verlangt genau das Nichtstun. Kein Inject-Fehler — `still_owner()`
            // meldet jetzt false, `inject_paste` bucht `ForeignOwner`.
            Err(TakeFailure::Foreign) => {
                eprintln!("Clipboard-Restore unterblieben: fremde Änderung seit der Übernahme");
                Ok(())
            }
            Err(TakeFailure::Failed(err)) => Err(err),
        }
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

    fn copy_only(&mut self, text: &str) -> Result<(), InjectError> {
        let text = apply_leading_space(text, self.output.leading_space);
        self.become_owner(text)
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
    /// oder schweigen könnte — entweder das Clipboard gehört uns und der Text
    /// steht drin (`Saved`), oder wir sind nicht Owner (`NotOwner`).
    fn save_to_clipboard_manager(
        &mut self,
        _timeout: Duration,
    ) -> Result<ClipboardSave, InjectError> {
        if !self.is_still_owner() {
            return Ok(ClipboardSave::NotOwner);
        }
        if !self.state.borrow().delayed {
            // Schon eager im Clipboard (Restore oder früherer Render).
            return Ok(ClipboardSave::Saved);
        }
        match self.materialize() {
            Ok(()) => Ok(ClipboardSave::Saved),
            // Zwischen Prüfung und `EmptyClipboard` hat jemand anderes kopiert:
            // dessen Inhalt bleibt stehen, gesichert wurde nichts.
            Err(TakeFailure::Foreign) => {
                eprintln!("Clipboard-Sicherung unterblieben: fremde Änderung seit der Übernahme");
                Ok(ClipboardSave::NotOwner)
            }
            Err(TakeFailure::Failed(err)) => Err(err),
        }
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
                Err(TakeFailure::Failed(err)) => {
                    eprintln!("Clipboard-Sicherung fehlgeschlagen: {err}");
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

/// `GMEM_MOVEABLE`-Handle mit dem Text als UTF-16+NUL. Das Handle geht bei
/// erfolgreichem `SetClipboardData` in das Eigentum des Systems über.
fn alloc_utf16(text: &str) -> Option<HGLOBAL> {
    let units = to_utf16_nul(text);
    let bytes = units.len().checked_mul(2)?;
    // SAFETY: `GMEM_MOVEABLE` ist die für Clipboard-Handles vorgeschriebene
    // Form; der Rückgabewert wird sofort geprüft.
    let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) };
    if handle.is_null() {
        return None;
    }
    // SAFETY: frisch alloziertes Handle, das ausschließlich uns gehört.
    let ptr = unsafe { GlobalLock(handle) } as *mut u16;
    if ptr.is_null() {
        // SAFETY: nicht gesperrt, noch nicht übergeben.
        unsafe { GlobalFree(handle) };
        return None;
    }
    // SAFETY: `bytes == units.len() * 2` wurde gerade alloziert, Quelle und
    // Ziel überlappen nicht.
    unsafe { ptr::copy_nonoverlapping(units.as_ptr(), ptr, units.len()) };
    // SAFETY: genau ein `GlobalLock` wird zurückgenommen.
    unsafe { GlobalUnlock(handle) };
    Some(handle)
}

/// Liest den Text aus einem Clipboard-Handle. `None` heißt „kein lesbarer
/// Unicode-Text“ und führt zu [`ClipboardSnapshot::NonText`].
fn read_utf16(handle: HGLOBAL) -> Option<String> {
    // SAFETY: `handle` stammt aus `GetClipboardData` bei geöffnetem Clipboard
    // und ist bis zum `CloseClipboard` gültig.
    let size = unsafe { GlobalSize(handle) };
    if size < 2 {
        return None;
    }
    // SAFETY: wie oben; `GlobalLock` liefert einen für `size` Bytes gültigen
    // Zeiger oder `NULL`.
    let ptr = unsafe { GlobalLock(handle) } as *const u16;
    if ptr.is_null() {
        return None;
    }
    let units = size / 2;
    // SAFETY: `units * 2 <= size` Bytes sind lesbar und für `u16` ausgerichtet
    // (`GlobalAlloc`-Speicher ist mindestens 8-Byte-aligned). Der Slice wird
    // vor dem `GlobalUnlock` vollständig kopiert.
    let text = utf16_until_nul(unsafe { std::slice::from_raw_parts(ptr, units) });
    // SAFETY: genau ein `GlobalLock` wird zurückgenommen.
    unsafe { GlobalUnlock(handle) };
    Some(text)
}

/// Der Teil des Snapshots, der ein **offenes** Clipboard voraussetzt (§7.1
/// Punkte 1/2).
fn read_open_clipboard() -> ClipboardSnapshot {
    // Sol-Review: „kein `CF_UNICODETEXT`“ heißt nicht „leer“ — ein Bild- oder
    // Datei-Clipboard hat ebenfalls keinen Unicode-Text. Wirklich leer ist nur,
    // was gar kein Format mehr trägt.
    // SAFETY: parameterlos, Clipboard ist offen.
    if unsafe { CountClipboardFormats() } == 0 {
        return ClipboardSnapshot::Text(String::new());
    }
    // SAFETY: parameterlos bis auf das Format, Clipboard ist offen.
    if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) } == 0 {
        return ClipboardSnapshot::NonText;
    }
    // SAFETY: Clipboard ist offen; das Handle gehört dem Clipboard und darf
    // nur bis zum `CloseClipboard` gelesen werden.
    let handle = unsafe { GetClipboardData(CF_UNICODETEXT) };
    if handle.is_null() {
        // Delayed Rendering einer fremden Anwendung, die nicht liefern konnte.
        return ClipboardSnapshot::NonText;
    }
    match read_utf16(handle as HGLOBAL) {
        Some(text) => ClipboardSnapshot::Text(text),
        None => ClipboardSnapshot::NonText,
    }
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
}
