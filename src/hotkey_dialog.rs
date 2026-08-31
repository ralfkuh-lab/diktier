//! „Hotkey ändern…" — kleines Win32-Fenster aus dem Tray-Menü (Windows only).
//!
//! **Fokus (SPEC §4.2).** Der PTT-Pfad öffnet nie ein Fenster. Dieser Dialog
//! ist die zweite eng begrenzte Ausnahme neben dem Tray-Kontextmenü: Er
//! erscheint **ausschließlich** nach einem expliziten Klick auf den Menüpunkt
//! „Hotkey ändern…", und weil er Tasten erfassen soll, muss er den Fokus
//! haben — genau das hat der Nutzer mit dem Klick verlangt. Kein anderer Pfad
//! ruft [`ask`].
//!
//! **Erfassung.** Der laufende `WH_KEYBOARD_LL`-Hook aus [`crate::hotkey`]
//! wird dabei *nicht* angefasst; der Dialog wertet schlicht `WM_KEYDOWN`/
//! `WM_SYSKEYDOWN` seines eigenen Fensters aus. Der Daemon gibt die Taste vor
//! dem Öffnen per `HotkeyCmd::Ungrab` frei, sonst schluckte der Hook genau den
//! Tastendruck, den der Nutzer hier vorführen will.
//!
//! **Thread.** [`ask`] baut Fensterklasse, Fenster und Message-Loop auf dem
//! aufrufenden Thread auf und kehrt erst zurück, wenn das Fenster zu ist. Der
//! Daemon ruft es deshalb auf einem eigenen Thread (`diktier-hotkey-dialog`)
//! — nicht auf dem Tray-Thread: der hält das Notify-Icon, und ein blockierter
//! Tray-Thread würde beim Beenden in den 2-s-Join-Timeout des `TrayWorker`
//! laufen und das Icon stehen lassen. Der Spike `--hotkey-dialog-test` ruft
//! `ask` direkt im Hauptthread.

use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr;

use thiserror::Error;
use windows_sys::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_BTNFACE, CreateFontW, DEFAULT_CHARSET,
    DEFAULT_PITCH, DeleteObject, FF_DONTCARE, FW_NORMAL, FW_SEMIBOLD, GetSysColor,
    GetSysColorBrush, HDC, HFONT, OUT_DEFAULT_PRECIS, SetBkColor, UpdateWindow,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, GetAsyncKeyState, SetFocus, VK_CONTROL, VK_ESCAPE, VK_LCONTROL, VK_LMENU,
    VK_LSHIFT, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RETURN, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRect, BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CREATESTRUCTW, CreateWindowExW,
    DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowW, GWLP_USERDATA, GetMessageW,
    GetSystemMetrics, GetWindowLongPtrW, HMENU, IDC_ARROW, LoadCursorW, MSG, PostMessageW,
    RegisterClassW, SM_CXSCREEN, SM_CYSCREEN, SW_SHOW, SendMessageW, SetForegroundWindow,
    SetWindowLongPtrW, SetWindowTextW, ShowWindow, TranslateMessage, UnregisterClassW, WM_CLOSE,
    WM_COMMAND, WM_CTLCOLORSTATIC, WM_DESTROY, WM_KEYDOWN, WM_KEYUP, WM_NCCREATE, WM_NCDESTROY,
    WM_SETFONT, WM_SYSCHAR, WM_SYSKEYDOWN, WM_SYSKEYUP, WNDCLASSW, WS_BORDER, WS_CAPTION, WS_CHILD,
    WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};

use crate::config::Modifier;
use crate::hotkey::{HotkeySpec, modifier_name, windows::vk_name};

/// `SS_CENTER` (1) und `SS_CENTERIMAGE` (0x200) liegen in windows-sys 0.61
/// unter `Win32_System_SystemServices` — ein sehr großes Feature für zwei
/// Zahlen. Die Werte sind stabile `winuser.h`-ABI; dieselbe Begründung wie bei
/// `NIN_KEYSELECT` in `tray::windows` und `CF_UNICODETEXT` in `inject::windows`.
const SS_CENTER: u32 = 0x0000_0001;
const SS_CENTERIMAGE: u32 = 0x0000_0200;

/// Fensterklasse. Prozessweit eindeutig, wie `DiktierTrayOwner`.
const CLASS_NAME: &str = "DiktierHotkeyDialog";
const TITLE: &str = "Diktier – Hotkey";
const PROMPT: &str = "Drücke die gewünschte Taste(nkombination):";
/// Zweizeilig — das Feld darunter ist hoch genug, `SS_CENTER` bricht um.
const HINT: &str = "Enter übernimmt, Esc bricht ab — beide sind deshalb nicht wählbar.";

/// Anzeigetext, wenn die gedrückte Taste keinen Config-Schlüssel hat.
pub const UNSUPPORTED_TEXT: &str = "nicht unterstützt";
/// Platzhalter für „noch keine Taste, nur Modifier".
const PENDING: &str = "…";

const ID_APPLY: usize = 1;
const ID_CANCEL: usize = 2;

/// Clientfläche; das Fenster wird daraus per `AdjustWindowRect` berechnet.
const CLIENT_W: i32 = 360;
const CLIENT_H: i32 = 176;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogOutcome {
    Applied(HotkeySpec),
    Cancelled,
}

#[derive(Debug, Error)]
pub enum DialogError {
    #[error("Hotkey-Dialog fehlgeschlagen: {0}")]
    Failed(String),
}

// ------------------------------------------------------------- reine Logik

/// Was ein `WM_KEYDOWN` im Dialog bedeutet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAction {
    /// `Escape` — Dialog schließen, nichts übernehmen.
    Cancel,
    /// `Enter` — die aktuelle Auswahl übernehmen.
    Apply,
    /// Nur ein Modifier: Vorschau zeigen, aber nichts übernehmen.
    ModifierOnly,
    /// Kanonischer Config-Schlüssel der gedrückten Taste.
    Key(String),
    /// Taste ohne Config-Schlüssel (Lock-Tasten, Medientasten, …).
    Unsupported,
}

/// Generisches `VK_CONTROL` in die seitenspezifische Variante übersetzen.
///
/// Fenster-Nachrichten (`WM_KEYDOWN` & Co.) melden für **beide** Strg-Tasten
/// immer `VK_CONTROL` (0x11) — nie `VK_LCONTROL`/`VK_RCONTROL`. Welche Seite
/// gedrückt wurde, steckt allein im Extended-Bit von `lparam` (Bit 24): gesetzt
/// = rechte Strg. Ohne diese Übersetzung erreichte der `VK_RCONTROL`-Arm in
/// [`classify`] nie einen Tastendruck und RCtrl blieb im Dialog ein bloßer
/// Modifier („Ctrl+…"). Der Low-Level-Hook in [`crate::hotkey`] braucht das
/// nicht: `KBDLLHOOKSTRUCT.vkCode` unterscheidet die Seiten von sich aus.
///
/// Nur Strg wird angefasst — RCtrl ist die einzige Taste, die §8 als Taste
/// zulässt; Shift/Alt/Win bleiben unverändert generisch und damit Modifier.
///
/// Randfall AltGr: Der Treiber erzeugt dafür ein synthetisches **linkes** Ctrl
/// (Extended-Bit **nicht** gesetzt) plus `VK_RMENU`. AltGr wird hier also zu
/// `VK_LCONTROL` und bleibt Modifier — keine Kollision mit RCtrl.
fn side_specific_vk(vk: u16, extended: bool) -> u16 {
    if vk == VK_CONTROL {
        if extended { VK_RCONTROL } else { VK_LCONTROL }
    } else {
        vk
    }
}

/// Extended-Bit (Bit 24) aus dem `lparam` einer Tastatur-Nachricht.
fn is_extended(lparam: LPARAM) -> bool {
    (lparam >> 24) & 1 != 0
}

/// `Escape` und `Enter` sind im Dialog belegt und deshalb **nicht** als
/// Hotkey wählbar — beide werden vor der VK-Tabelle abgefangen. Das ist der
/// bewusste Preis dafür, dass der Dialog ohne Maus bedienbar ist; im Fenster
/// steht der Hinweis.
pub fn classify(vk: u16) -> KeyAction {
    match vk {
        VK_ESCAPE => KeyAction::Cancel,
        VK_RETURN => KeyAction::Apply,
        // §8: Die **rechte** Strg-Taste ist als Taste wählbar. Deshalb vor
        // [`is_modifier_vk`] — sie ist beides, und hier gilt sie als Taste. Die
        // linke Strg und alle übrigen Modifier bleiben Modifier.
        VK_RCONTROL => match vk_name(VK_RCONTROL) {
            Some(name) => KeyAction::Key(name),
            None => KeyAction::Unsupported,
        },
        vk if is_modifier_vk(vk) => KeyAction::ModifierOnly,
        vk => match vk_name(vk) {
            Some(name) => KeyAction::Key(name),
            None => KeyAction::Unsupported,
        },
    }
}

/// Modifier melden sich als eigener Tastendruck — der Chord ist damit aber
/// noch nicht fertig.
fn is_modifier_vk(vk: u16) -> bool {
    matches!(
        vk,
        VK_SHIFT
            | VK_CONTROL
            | VK_MENU
            | VK_LSHIFT
            | VK_RSHIFT
            | VK_LCONTROL
            | VK_RCONTROL
            | VK_LMENU
            | VK_RMENU
            | VK_LWIN
            | VK_RWIN
    )
}

/// `Ctrl+Alt+F12` für die Anzeige — mit `None` als Taste die Vorschau
/// `Ctrl+Alt+…`. Die Reihenfolge ist die kanonische aus [`Modifier`], nicht
/// die Reihenfolge des Drückens.
pub fn chord_text(modifiers: &[Modifier], key: Option<&str>) -> String {
    let mut out = String::new();
    for modifier in modifiers {
        out.push_str(modifier_name(*modifier));
        out.push('+');
    }
    out.push_str(key.unwrap_or(PENDING));
    out
}

/// Aus welchem Virtual-Key der Ctrl-Anteil kommt.
///
/// Ist die gerade gedrückte Taste selbst die rechte Strg (die als Hotkey-Taste
/// wählbar ist), darf `VK_CONTROL` nicht gefragt werden — es meldet beide
/// Seiten, der Dialog zeigte „Ctrl+RCtrl" und schriebe genau das in die Config.
/// Dieselbe Maskierung wie im Hook
/// (`hotkey::windows::ModifierState::mask_hotkey_key`).
fn ctrl_probe_vk(pressed: u16) -> u16 {
    if pressed == VK_RCONTROL {
        VK_LCONTROL
    } else {
        VK_CONTROL
    }
}

/// Physisch gehaltene Modifier, in kanonischer Reihenfolge.
///
/// `pressed` ist die Taste, um die es gerade geht — sie bestimmt die
/// Ctrl-Maskierung (siehe [`ctrl_probe_vk`]).
///
/// Bewusst `GetAsyncKeyState` und nicht `GetKeyState`: Der Hook aus
/// [`crate::hotkey`] vergleicht später mit **genau** dieser Quelle
/// (`ModifierState::current_for`). Was der Dialog anzeigt, ist damit das, was
/// der Hook zur Laufzeit fordert — inklusive der dort dokumentierten
/// AltGr-Kollision (AltGr erscheint als `Ctrl+Alt`).
fn held_modifiers(pressed: u16) -> Vec<Modifier> {
    let mut out = Vec::new();
    if is_down(ctrl_probe_vk(pressed)) {
        out.push(Modifier::Ctrl);
    }
    if is_down(VK_SHIFT) {
        out.push(Modifier::Shift);
    }
    if is_down(VK_MENU) {
        out.push(Modifier::Alt);
    }
    if is_down(VK_LWIN) || is_down(VK_RWIN) {
        out.push(Modifier::Super);
    }
    out
}

fn is_down(vk: u16) -> bool {
    // SAFETY: nimmt den VK als Wert, schreibt nichts, liefert für unbekannte
    // Codes 0 (wie `hotkey::windows::is_down`).
    let state = unsafe { GetAsyncKeyState(i32::from(vk)) };
    (state as u16 & 0x8000) != 0
}

// ------------------------------------------------------------------ Zustand

/// Was das Anzeigefeld gerade zeigt.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Display {
    /// Die übernehmbare Auswahl (`selected`).
    Selected,
    /// Nur Modifier gedrückt — der Chord ist unfertig.
    Preview(Vec<Modifier>),
    /// Taste ohne Config-Schlüssel.
    Unsupported,
}

struct DialogState {
    field: HWND,
    apply: HWND,
    /// Der zuletzt vollständige Chord; startet mit dem aktuellen Hotkey,
    /// damit „Öffnen und Enter" den Hotkey unverändert lässt.
    selected: HotkeySpec,
    display: Display,
    outcome: DialogOutcome,
    /// Das Fenster ist zerstört — die Message-Loop darf enden.
    done: bool,
}

impl DialogState {
    /// Text und Zustand der Übernehmen-Schaltfläche aus [`Display`].
    fn view(&self) -> (String, bool) {
        match &self.display {
            Display::Selected => (
                chord_text(&self.selected.modifiers, Some(&self.selected.key)),
                true,
            ),
            Display::Preview(modifiers) => (chord_text(modifiers, None), false),
            Display::Unsupported => (UNSUPPORTED_TEXT.to_string(), false),
        }
    }
}

/// Anzeigefeld und Schaltfläche an den Zustand angleichen. Der `RefCell`-Borrow
/// endet **vor** den Win32-Aufrufen.
fn refresh(cell: &RefCell<DialogState>) {
    let Ok(state) = cell.try_borrow() else {
        return;
    };
    let (text, enabled) = state.view();
    let (field, apply) = (state.field, state.apply);
    drop(state);

    let wide = wide(&text);
    // SAFETY: eigene Controls dieses Threads, NUL-terminierter Puffer.
    unsafe {
        SetWindowTextW(field, wide.as_ptr());
        EnableWindow(apply, i32::from(enabled));
    }
}

// ------------------------------------------------------------------ WndProc

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        // SAFETY: Für `WM_NCCREATE` garantiert Windows eine gültige
        // `CREATESTRUCTW`; `lpCreateParams` ist der Zeiger aus `CreateWindowExW`.
        let create = unsafe { &*(lparam as *const CREATESTRUCTW) };
        // SAFETY: `hwnd` ist gültig, `GWLP_USERDATA` gehört der Anwendung.
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize) };
        // SAFETY: unveränderte Parameter an die Default-Behandlung.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }

    // SAFETY: `hwnd` ist gültig; der Wert ist entweder 0 (vor `WM_NCCREATE`,
    // nach `WM_NCDESTROY`) oder der oben gesetzte Zeiger.
    let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const RefCell<DialogState>;
    if raw.is_null() {
        // SAFETY: unveränderte Parameter an die Default-Behandlung.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    // SAFETY: Der Zeiger stammt aus der `Box` in `ask`, die länger lebt als
    // das Fenster; der `WndProc` läuft nur auf dem Thread, dem beides gehört.
    let cell = unsafe { &*raw };

    match msg {
        // `WM_SYSKEYDOWN` ist derselbe Fall: Alt-Kombinationen und F10 kommen
        // als System-Taste. Beide werden **nicht** an `DefWindowProcW`
        // weitergereicht — sonst öffnete Alt das Systemmenü.
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            on_key_down(cell, hwnd, wparam as u16, is_extended(lparam));
            return 0;
        }
        // Beim Loslassen eines Modifiers schrumpft die Vorschau wieder.
        WM_KEYUP | WM_SYSKEYUP => {
            let vk = side_specific_vk(wparam as u16, is_extended(lparam));
            if is_modifier_vk(vk) {
                set_preview(cell, vk);
            }
            return 0;
        }
        // Ohne das quittiert Windows jedes Alt+Taste mit einem Piepton.
        WM_SYSCHAR => return 0,
        WM_COMMAND => {
            match (wparam as u32) & 0xFFFF {
                id if id as usize == ID_APPLY => apply(cell, hwnd),
                id if id as usize == ID_CANCEL => close(cell, hwnd, DialogOutcome::Cancelled),
                _ => {}
            }
            return 0;
        }
        // Labels auf der Fensterfarbe zeichnen, nicht auf Weiß.
        WM_CTLCOLORSTATIC => {
            // SAFETY: `wparam` ist der HDC des Controls (dokumentiert);
            // `GetSysColorBrush` gehört dem System und darf nicht freigegeben
            // werden.
            unsafe {
                SetBkColor(wparam as HDC, GetSysColor(COLOR_BTNFACE));
                return GetSysColorBrush(COLOR_BTNFACE) as LRESULT;
            }
        }
        // Das Schließkreuz ist ein Abbruch.
        WM_CLOSE => {
            close(cell, hwnd, DialogOutcome::Cancelled);
            return 0;
        }
        WM_DESTROY => {
            if let Ok(mut state) = cell.try_borrow_mut() {
                state.done = true;
            }
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

/// `extended` ist das Extended-Bit der Nachricht — es entscheidet, ob ein
/// generisches `VK_CONTROL` die linke oder die rechte Strg war (siehe
/// [`side_specific_vk`]).
fn on_key_down(cell: &RefCell<DialogState>, hwnd: HWND, vk: u16, extended: bool) {
    let vk = side_specific_vk(vk, extended);
    match classify(vk) {
        KeyAction::Cancel => close(cell, hwnd, DialogOutcome::Cancelled),
        KeyAction::Apply => apply(cell, hwnd),
        KeyAction::ModifierOnly => set_preview(cell, vk),
        KeyAction::Key(key) => {
            if let Ok(mut state) = cell.try_borrow_mut() {
                state.selected = HotkeySpec {
                    key,
                    modifiers: held_modifiers(vk),
                };
                state.display = Display::Selected;
            }
            refresh(cell);
        }
        KeyAction::Unsupported => {
            if let Ok(mut state) = cell.try_borrow_mut() {
                state.display = Display::Unsupported;
            }
            refresh(cell);
        }
    }
}

/// Modifier-Vorschau nachziehen. Ist kein Modifier mehr gedrückt, steht wieder
/// die letzte vollständige Auswahl da.
///
/// `vk` ist die Taste, die den Wechsel ausgelöst hat — sie geht nur in die
/// Ctrl-Maskierung ein (siehe [`held_modifiers`]).
fn set_preview(cell: &RefCell<DialogState>, vk: u16) {
    let held = held_modifiers(vk);
    if let Ok(mut state) = cell.try_borrow_mut() {
        state.display = if held.is_empty() {
            Display::Selected
        } else {
            Display::Preview(held)
        };
    }
    refresh(cell);
}

/// Übernehmen — nur, wenn gerade eine vollständige Auswahl dasteht.
fn apply(cell: &RefCell<DialogState>, hwnd: HWND) {
    let outcome = match cell.try_borrow() {
        Ok(state) if state.display == Display::Selected => {
            DialogOutcome::Applied(state.selected.clone())
        }
        // Unfertiger Chord oder nicht unterstützte Taste: nichts tun, der
        // Dialog bleibt offen.
        _ => return,
    };
    close(cell, hwnd, outcome);
}

fn close(cell: &RefCell<DialogState>, hwnd: HWND, outcome: DialogOutcome) {
    if let Ok(mut state) = cell.try_borrow_mut() {
        state.outcome = outcome;
    }
    // SAFETY: eigenes Fenster dieses Threads; `WM_DESTROY` setzt gleich
    // `done`, `WM_NCDESTROY` löscht den Zustandszeiger.
    unsafe { DestroyWindow(hwnd) };
}

// --------------------------------------------------------------- Aufbau

/// NUL-terminierter UTF-16-Puffer für die `W`-APIs.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn last_error() -> u32 {
    // SAFETY: parameterlos, liest den Fehlercode dieses Threads.
    unsafe { GetLastError() }
}

/// Segoe UI in der gewünschten Höhe; `None`, wenn GDI nicht mitspielt (dann
/// bleibt die Systemschrift, das Fenster ist trotzdem bedienbar).
fn font(height: i32, weight: i32) -> HFONT {
    let face = wide("Segoe UI");
    // SAFETY: `face` ist NUL-terminiert und lebt über den Aufruf; alle
    // übrigen Parameter sind dokumentierte Konstanten.
    unsafe {
        CreateFontW(
            -height,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            u32::from(DEFAULT_CHARSET),
            u32::from(OUT_DEFAULT_PRECIS),
            u32::from(CLIP_DEFAULT_PRECIS),
            u32::from(CLEARTYPE_QUALITY),
            u32::from(DEFAULT_PITCH | FF_DONTCARE),
            face.as_ptr(),
        )
    }
}

fn set_font(hwnd: HWND, hfont: HFONT) {
    if hfont.is_null() {
        return;
    }
    // SAFETY: eigenes Control dieses Threads; `WM_SETFONT` übernimmt die
    // Schrift nicht, sie muss uns überleben (siehe `Fonts::drop`).
    unsafe { SendMessageW(hwnd, WM_SETFONT, hfont as WPARAM, 1) };
}

/// Beide Schriften gehören dem Dialog und werden am Ende freigegeben.
struct Fonts {
    body: HFONT,
    big: HFONT,
}

impl Drop for Fonts {
    fn drop(&mut self) {
        for handle in [self.body, self.big] {
            if !handle.is_null() {
                // SAFETY: eigene GDI-Objekte; die Fenster, die sie benutzt
                // haben, sind zu diesem Zeitpunkt zerstört.
                unsafe { DeleteObject(handle as *mut c_void) };
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn child(
    class: &str,
    text: &str,
    style: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    parent: HWND,
    id: usize,
    instance: HINSTANCE,
) -> HWND {
    let class = wide(class);
    let text = wide(text);
    // SAFETY: Beide Puffer sind NUL-terminiert und leben über den Aufruf;
    // `parent` ist unser Fenster, `id` wird als Control-ID übergeben (die
    // dokumentierte Bedeutung von `hMenu` bei `WS_CHILD`).
    unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            text.as_ptr(),
            WS_CHILD | WS_VISIBLE | style,
            x,
            y,
            w,
            h,
            parent,
            id as HMENU,
            instance,
            ptr::null(),
        )
    }
}

/// Öffnet den Dialog und kehrt erst zurück, wenn er geschlossen ist.
///
/// `current` ist der gerade gültige Hotkey; er steht beim Öffnen im Feld und
/// bleibt die Auswahl, wenn der Nutzer sofort „Übernehmen" drückt.
pub fn ask(current: &HotkeySpec) -> Result<DialogOutcome, DialogError> {
    // SAFETY: `GetModuleHandleW(NULL)` liefert das eigene Modul-Handle und
    // überträgt kein Eigentum.
    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    let class_name = wide(CLASS_NAME);

    let class = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: ptr::null_mut(),
        // SAFETY: Systemcursor, kein Eigentum, kein Freigeben.
        hCursor: unsafe { LoadCursorW(ptr::null_mut(), IDC_ARROW) },
        // SAFETY: Systempinsel, gehört dem System.
        hbrBackground: unsafe { GetSysColorBrush(COLOR_BTNFACE) },
        lpszMenuName: ptr::null(),
        lpszClassName: class_name.as_ptr(),
    };
    // SAFETY: `class` ist vollständig initialisiert, `lpszClassName` zeigt in
    // `class_name`, das noch lebt; `wnd_proc` hat die von `WNDPROC` geforderte
    // Signatur.
    let atom = unsafe { RegisterClassW(&class) };
    let owns_class = if atom == 0 {
        let err = last_error();
        if err != ERROR_CLASS_ALREADY_EXISTS {
            return Err(DialogError::Failed(format!(
                "Fensterklasse {CLASS_NAME} nicht registrierbar: Win32-Fehler {err}"
            )));
        }
        false
    } else {
        true
    };

    let state = Box::new(RefCell::new(DialogState {
        field: ptr::null_mut(),
        apply: ptr::null_mut(),
        selected: current.clone(),
        display: Display::Selected,
        outcome: DialogOutcome::Cancelled,
        done: false,
    }));
    let state_ptr: *const RefCell<DialogState> = &*state;

    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: CLIENT_W,
        bottom: CLIENT_H,
    };
    // SAFETY: `rect` ist gültiger Speicher; `FALSE` = kein Menü.
    unsafe { AdjustWindowRect(&mut rect, style, 0) };
    let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
    // SAFETY: parameterlose Lesezugriffe auf Systemmetriken.
    let (screen_w, screen_h) =
        unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let x = ((screen_w - width) / 2).max(0);
    let y = ((screen_h - height) / 2).max(0);

    let title = wide(TITLE);
    // SAFETY: Alle Zeiger zeigen auf lebende, NUL-terminierte Puffer;
    // `state_ptr` erreicht den `WndProc` als `lpCreateParams` in `WM_NCCREATE`,
    // und die Box lebt bis zum Ende dieser Funktion — also länger als das
    // Fenster, das vorher zerstört wird.
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            style,
            x,
            y,
            width,
            height,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            state_ptr as *const c_void,
        )
    };
    if hwnd.is_null() {
        let err = last_error();
        if owns_class {
            // SAFETY: eigene Klasse, es existiert kein Fenster dazu.
            unsafe { UnregisterClassW(class_name.as_ptr(), instance) };
        }
        return Err(DialogError::Failed(format!(
            "Hotkey-Fenster nicht erzeugbar: Win32-Fehler {err}"
        )));
    }

    let fonts = Fonts {
        body: font(15, FW_NORMAL as i32),
        big: font(26, FW_SEMIBOLD as i32),
    };

    let prompt = child(
        "STATIC",
        PROMPT,
        SS_CENTER,
        16,
        14,
        CLIENT_W - 32,
        20,
        hwnd,
        0,
        instance,
    );
    let field = child(
        "STATIC",
        &chord_text(&current.modifiers, Some(&current.key)),
        SS_CENTER | SS_CENTERIMAGE | WS_BORDER,
        16,
        40,
        CLIENT_W - 32,
        44,
        hwnd,
        0,
        instance,
    );
    let hint = child(
        "STATIC",
        HINT,
        SS_CENTER,
        16,
        92,
        CLIENT_W - 32,
        40,
        hwnd,
        0,
        instance,
    );
    let apply_button = child(
        "BUTTON",
        "Übernehmen",
        BS_DEFPUSHBUTTON as u32 | WS_TABSTOP,
        CLIENT_W - 32 - 200,
        140,
        96,
        28,
        hwnd,
        ID_APPLY,
        instance,
    );
    let cancel_button = child(
        "BUTTON",
        "Abbrechen",
        BS_PUSHBUTTON as u32 | WS_TABSTOP,
        CLIENT_W - 32 - 96,
        140,
        96,
        28,
        hwnd,
        ID_CANCEL,
        instance,
    );
    for control in [prompt, hint, apply_button, cancel_button] {
        set_font(control, fonts.body);
    }
    set_font(field, fonts.big);

    if let Ok(mut inner) = state.try_borrow_mut() {
        inner.field = field;
        inner.apply = apply_button;
    }

    // SPEC §4.2-Ausnahme: Der Nutzer hat den Dialog gerade über das Tray-Menü
    // angefordert und muss Tasten hineindrücken können — ohne Vordergrund und
    // Fokus sähe das Fenster kein einziges `WM_KEYDOWN`. Der Fokus bleibt
    // **auf dem Fenster selbst**, nicht auf einer Schaltfläche: sonst
    // verschluckte der Button-WndProc Leertaste und Pfeiltasten.
    // SAFETY: eigenes Fenster dieses Threads.
    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);
        SetForegroundWindow(hwnd);
        SetFocus(hwnd);
    }

    // Eigene, kleine Message-Loop bis zum `WM_DESTROY` des Fensters. Kein
    // `IsDialogMessageW`: Der Dialog ist bewusst kein echtes Dialogfenster —
    // dessen Tastaturnavigation würde Enter, Escape, Tab und Leertaste
    // abfangen, also genau die Tasten, die hier erfasst werden sollen.
    // SAFETY: `MSG` ist POD; `GetMessageW` füllt die Struktur.
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    loop {
        // SAFETY: `msg` ist gültiger Speicher; `hWnd = NULL` holt die
        // Nachrichten **dieses** Threads.
        let got = unsafe { GetMessageW(&mut msg, ptr::null_mut(), 0, 0) };
        // 0 = `WM_QUIT`, -1 = Fehler. Beides beendet die Schleife.
        if got <= 0 {
            break;
        }
        // SAFETY: `msg` stammt unverändert aus `GetMessageW`.
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if state.try_borrow().map(|s| s.done).unwrap_or(false) {
            break;
        }
    }

    let outcome = state
        .try_borrow()
        .map(|s| s.outcome.clone())
        .unwrap_or(DialogOutcome::Cancelled);
    let alive = !state.try_borrow().map(|s| s.done).unwrap_or(false);
    if alive {
        // `WM_QUIT` oder ein `GetMessageW`-Fehler: das Fenster steht noch.
        // SAFETY: eigenes Fenster dieses Threads.
        unsafe { DestroyWindow(hwnd) };
    }
    drop(fonts);
    if owns_class {
        // SAFETY: eigene Klasse, ihr einziges Fenster ist zerstört.
        unsafe { UnregisterClassW(class_name.as_ptr(), instance) };
    }
    Ok(outcome)
}

/// Nur für den Spike `--hotkey-dialog-test`: ein offenes Dialogfenster von
/// außen schließen (das ist ein Abbruch, wie das Schließkreuz).
///
/// `PostMessageW` statt `SendMessageW`: Der Aufrufer ist ein anderer Thread,
/// und ein blockierendes `SendMessageW` würde auf dessen Message-Loop warten.
/// Liefert `false`, wenn gar kein Dialog offen ist.
pub fn close_open_dialog() -> bool {
    let class = wide(CLASS_NAME);
    // SAFETY: NUL-terminierter Puffer, der über den Aufruf lebt; ein
    // Fenstername von `NULL` heißt „egal welcher Titel".
    let hwnd = unsafe { FindWindowW(class.as_ptr(), ptr::null()) };
    if hwnd.is_null() {
        return false;
    }
    // SAFETY: `WM_CLOSE` ohne Parameter an ein fremdes, aber prozesseigenes
    // Fenster — genau das, was das Schließkreuz schickt.
    unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) != 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Enter und Escape steuern den Dialog und dürfen deshalb nie als Hotkey
    /// herauskommen — sonst wäre der Dialog nach dem Übernehmen nicht mehr
    /// bedienbar.
    #[test]
    fn enter_and_escape_stay_dialog_commands() {
        assert_eq!(classify(VK_RETURN), KeyAction::Apply);
        assert_eq!(classify(VK_ESCAPE), KeyAction::Cancel);
        // Beide haben sehr wohl einen Config-Namen — er wird hier bewusst
        // nicht benutzt.
        assert_eq!(vk_name(VK_RETURN).as_deref(), Some("Enter"));
        assert_eq!(vk_name(VK_ESCAPE).as_deref(), Some("Escape"));
    }

    #[test]
    fn modifiers_alone_do_not_finish_a_chord() {
        for vk in [
            VK_SHIFT,
            VK_CONTROL,
            VK_MENU,
            VK_LSHIFT,
            VK_RSHIFT,
            VK_LCONTROL,
            VK_LMENU,
            VK_RMENU,
            VK_LWIN,
            VK_RWIN,
        ] {
            assert_eq!(classify(vk), KeyAction::ModifierOnly, "VK {vk:#04x}");
        }
    }

    /// §8: Die rechte Strg-Taste ist als **Taste** wählbar — sie ist die
    /// einzige Modifier-Taste, die der Dialog nicht als Modifier verbucht.
    #[test]
    fn right_ctrl_is_selectable_as_a_key() {
        assert_eq!(classify(VK_RCONTROL), KeyAction::Key("RCtrl".into()));
        assert_eq!(
            classify(VK_LCONTROL),
            KeyAction::ModifierOnly,
            "die linke Strg bleibt Modifier"
        );
        assert_eq!(classify(VK_CONTROL), KeyAction::ModifierOnly);
        assert_eq!(chord_text(&[], Some("RCtrl")), "RCtrl");
    }

    /// Fenster-Nachrichten melden für beide Strg-Tasten `VK_CONTROL`; erst das
    /// Extended-Bit macht daraus eine Seite. Ohne diese Übersetzung wäre der
    /// `VK_RCONTROL`-Arm in [`classify`] toter Code.
    #[test]
    fn generic_ctrl_becomes_side_specific() {
        assert_eq!(side_specific_vk(VK_CONTROL, true), VK_RCONTROL);
        assert_eq!(side_specific_vk(VK_CONTROL, false), VK_LCONTROL);
        // Alle anderen VKs bleiben unangetastet — auch mit Extended-Bit, das
        // bei ihnen etwas anderes bedeutet (Ziffernblock, Pfeiltasten, …).
        for vk in [VK_SHIFT, VK_MENU, VK_RMENU, VK_LWIN, VK_RETURN, 0x7b, 0x41] {
            assert_eq!(side_specific_vk(vk, false), vk, "VK {vk:#04x}");
            assert_eq!(side_specific_vk(vk, true), vk, "VK {vk:#04x} extended");
        }
    }

    /// Der Weg, den `wnd_proc` geht: erst seitenspezifisch machen, dann
    /// klassifizieren. Rechte Strg ist eine Taste, linke bleibt Modifier.
    #[test]
    fn extended_bit_decides_key_versus_modifier() {
        assert_eq!(
            classify(side_specific_vk(VK_CONTROL, true)),
            KeyAction::Key("RCtrl".into())
        );
        assert_eq!(
            classify(side_specific_vk(VK_CONTROL, false)),
            KeyAction::ModifierOnly,
            "linke Strg (und AltGr, das ein linkes Ctrl erzeugt) bleibt Modifier"
        );
    }

    #[test]
    fn extended_bit_is_bit_24_of_lparam() {
        assert!(is_extended(0x0100_0000));
        assert!(!is_extended(0x0000_0000));
        // Repeat-Count, Scancode und die übrigen Flags stören nicht.
        assert!(is_extended(0xC11D_0001));
        assert!(!is_extended(0xC01D_0001));
    }

    /// Sonst zeigte der Dialog beim Drücken von RCtrl „Ctrl+RCtrl" und
    /// schriebe genau das in die Config: `VK_CONTROL` meldet beide Seiten.
    #[test]
    fn right_ctrl_does_not_count_itself_as_a_modifier() {
        assert_eq!(ctrl_probe_vk(VK_RCONTROL), VK_LCONTROL);
        // Jede andere Taste fragt weiter beide Seiten ab — wie der Hook.
        assert_eq!(ctrl_probe_vk(0x7b), VK_CONTROL);
        assert_eq!(ctrl_probe_vk(VK_LCONTROL), VK_CONTROL);
    }

    #[test]
    fn real_keys_become_config_names_unknown_ones_do_not() {
        assert_eq!(classify(0x7b), KeyAction::Key("F12".into()));
        assert_eq!(classify(0x91), KeyAction::Key("ScrollLock".into()));
        assert_eq!(classify(0x41), KeyAction::Key("A".into()));
        // VK_CAPITAL und VK_NUMLOCK haben keinen Config-Schlüssel.
        assert_eq!(classify(0x14), KeyAction::Unsupported);
        assert_eq!(classify(0x90), KeyAction::Unsupported);
    }

    #[test]
    fn chord_text_shows_preview_and_full_chord() {
        let mods = [Modifier::Ctrl, Modifier::Alt];
        assert_eq!(chord_text(&mods, Some("F12")), "Ctrl+Alt+F12");
        assert_eq!(chord_text(&mods, None), "Ctrl+Alt+…");
        assert_eq!(chord_text(&[], Some("ScrollLock")), "ScrollLock");
        assert_eq!(chord_text(&[], None), "…");
    }

    /// Was das Feld zeigt und ob „Übernehmen" klickbar ist, hängt allein am
    /// [`Display`]-Zustand.
    #[test]
    fn only_a_complete_chord_enables_apply() {
        let mut state = DialogState {
            field: ptr::null_mut(),
            apply: ptr::null_mut(),
            selected: HotkeySpec {
                key: "F9".into(),
                modifiers: vec![],
            },
            display: Display::Selected,
            outcome: DialogOutcome::Cancelled,
            done: false,
        };
        assert_eq!(state.view(), ("F9".to_string(), true));

        state.display = Display::Preview(vec![Modifier::Ctrl]);
        assert_eq!(state.view(), ("Ctrl+…".to_string(), false));

        state.display = Display::Unsupported;
        assert_eq!(state.view(), (UNSUPPORTED_TEXT.to_string(), false));
    }
}
