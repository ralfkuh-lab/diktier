//! Tray-Backend (Spec §4.3). Linux: betrayer/SNI. Windows: `Shell_NotifyIconW`.
#![allow(dead_code)]

use std::path::PathBuf;

use thiserror::Error;

use crate::config;
use crate::state::{AppState, Runtime};

/// Ereignisse an den Aufrufer (State-Machine in Phase 3). Kein Fokusklau (§4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    /// Linksklick: Toggle-Aufnahme folgt in Phase 3 — hier nur Durchreichung.
    LeftClick,
    TogglePause,
    OpenConfigDir,
    Quit,
}

impl TrayEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LeftClick => "left-click",
            Self::TogglePause => "toggle-pause",
            Self::OpenConfigDir => "open-config-dir",
            Self::Quit => "quit",
        }
    }
}

/// Sichtbarer Tray-Zustand aus Spec §4.3 (Tabelle). `paused` schlägt AppState.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayStatus {
    Starting,
    Downloading,
    Loading,
    Idle,
    Recording,
    Transcribing,
    Error,
    Paused,
}

impl TrayStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Downloading => "downloading",
            Self::Loading => "loading",
            Self::Idle => "idle",
            Self::Recording => "recording",
            Self::Transcribing => "transcribing",
            Self::Error => "error",
            Self::Paused => "paused",
        }
    }
}

/// Rohklick, den ein Backend liefern kann. Mapping ist Linux-betrayer (Phase 2b).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayClick {
    Left,
    Right,
    Double,
}

/// Menüeinträge nach §4.3. Die Statuszeile hat ein Signal, wird aber verworfen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Status,
    TogglePause,
    OpenConfigDir,
    Quit,
}

#[derive(Debug, Error)]
pub enum TrayError {
    #[error("Tray fehlgeschlagen: {0}")]
    Failed(String),
}

/// Gemeinsamer Vertrag für betrayer (jetzt), später ksni / Shell_NotifyIconW.
///
/// Backends besitzen den UI-/D-Bus-Thread, schicken Events über einen Channel
/// und blockieren den Aufrufer nicht. `poll` ist nicht blockierend.
pub trait TrayBackend {
    fn update(&mut self, runtime: &Runtime, model_key: &str) -> Result<(), TrayError>;
    fn poll(&mut self) -> Result<Option<TrayEvent>, TrayError> {
        Ok(None)
    }
    fn backend_name(&self) -> &'static str {
        "stub"
    }
}

#[derive(Debug, Default)]
pub struct StubTray;

impl TrayBackend for StubTray {
    fn update(&mut self, _runtime: &Runtime, _model_key: &str) -> Result<(), TrayError> {
        Ok(())
    }
}

/// Genau ein echtes Backend je Plattform. Die frühere `Stub`-Variante ist mit
/// dem Windows-Backend entfallen — sie wäre auf beiden Plattformen tot
/// (Paket A hat dasselbe bei `AnyHotkeyBackend` gemacht). `StubTray` selbst
/// bleibt als Vertragsprobe im Testmodul.
pub enum AnyTray {
    #[cfg(target_os = "linux")]
    Betrayer(Box<linux::BetrayerTray>),
    #[cfg(windows)]
    Win32(Box<windows::Win32Tray>),
}

impl TrayBackend for AnyTray {
    fn update(&mut self, runtime: &Runtime, model_key: &str) -> Result<(), TrayError> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Betrayer(inner) => inner.update(runtime, model_key),
            #[cfg(windows)]
            Self::Win32(inner) => inner.update(runtime, model_key),
        }
    }

    fn poll(&mut self) -> Result<Option<TrayEvent>, TrayError> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Betrayer(inner) => inner.poll(),
            #[cfg(windows)]
            Self::Win32(inner) => inner.poll(),
        }
    }

    fn backend_name(&self) -> &'static str {
        match self {
            #[cfg(target_os = "linux")]
            Self::Betrayer(inner) => inner.backend_name(),
            #[cfg(windows)]
            Self::Win32(inner) => inner.backend_name(),
        }
    }
}

#[cfg(target_os = "linux")]
pub fn new_backend(runtime: &Runtime, model_key: &str) -> Result<AnyTray, TrayError> {
    linux::BetrayerTray::new(runtime, model_key)
        .map(Box::new)
        .map(AnyTray::Betrayer)
}

#[cfg(windows)]
pub fn new_backend(runtime: &Runtime, model_key: &str) -> Result<AnyTray, TrayError> {
    windows::Win32Tray::new(runtime, model_key)
        .map(Box::new)
        .map(AnyTray::Win32)
}

/// §4.3-Tabelle. Aktive Arbeit hat Vorrang vor `paused`: eine per Tray-Click
/// gestartete Aufnahme läuft auch bei ausgeschaltetem Hotkey und muss sichtbar
/// sein. `paused` zeigt sich deshalb nur in den Ruhezuständen.
pub fn tray_status(runtime: &Runtime) -> TrayStatus {
    match runtime.state {
        AppState::Recording { .. } => TrayStatus::Recording,
        // §4.3 kennt keinen sichtbaren Zustand „injecting" — der Ausgabepfad
        // gehört für den Nutzer noch zur Transkription.
        AppState::Transcribing { .. } | AppState::Injecting { .. } => TrayStatus::Transcribing,
        _ if runtime.paused => TrayStatus::Paused,
        AppState::Starting => TrayStatus::Starting,
        AppState::Downloading => TrayStatus::Downloading,
        AppState::Loading => TrayStatus::Loading,
        AppState::Idle => TrayStatus::Idle,
        AppState::Error => TrayStatus::Error,
    }
}

/// §4.3: „Zustand + Modellschlüssel". Im Fehlerzustand kommt der Grund dazu —
/// §4.4 verlangt für den Hotkey-Konflikt ausdrücklich, dass der Tooltip ihn
/// nennt; §6.3/§7.1 wollen dasselbe für Download- und Injectfehler (codex M1).
pub fn tooltip_text(runtime: &Runtime, model_key: &str) -> String {
    let base = format!("{} — {model_key}", tray_status(runtime).as_str());
    match &runtime.error {
        Some(info) if tray_status(runtime) == TrayStatus::Error && !info.message.is_empty() => {
            format!("{base} — {}", info.message)
        }
        _ => base,
    }
}

pub fn pause_menu_label(paused: bool) -> &'static str {
    if paused {
        "Hotkey wieder aktivieren"
    } else {
        "Hotkey pausieren"
    }
}

/// Linux-betrayer (Phase 2b):
/// - `Double` = SNI `Activate` (Linksklick; betrayer verschluckt den ersten).
/// - `Left` = dbusmenu-Root `opened` (Rechtsklick öffnet das Menü) — kein Toggle.
/// - `Right` kommt unter Linux nie.
///
/// Windows-Mapping folgt mit dem echten Backend.
pub fn route_click(click: TrayClick) -> Option<TrayEvent> {
    match click {
        TrayClick::Double => Some(TrayEvent::LeftClick),
        TrayClick::Left | TrayClick::Right => None,
    }
}

pub fn route_menu(action: MenuAction) -> Option<TrayEvent> {
    match action {
        MenuAction::Status => None,
        MenuAction::TogglePause => Some(TrayEvent::TogglePause),
        MenuAction::OpenConfigDir => Some(TrayEvent::OpenConfigDir),
        MenuAction::Quit => Some(TrayEvent::Quit),
    }
}

pub fn config_dir() -> Result<PathBuf, TrayError> {
    let path = config::config_path().map_err(|e| TrayError::Failed(e.to_string()))?;
    path.parent()
        .map(PathBuf::from)
        .ok_or_else(|| TrayError::Failed("Config-Pfad hat kein Elternverzeichnis".into()))
}

/// Kindprozess einsammeln, sonst bleibt er bis zum Prozessende ein Zombie
/// (agy B1). Der Helferthread endet, sobald `xdg-open` fertig ist — auf den
/// Rückgabewert wartet niemand, das Öffnen ist „fire and forget".
#[cfg(target_os = "linux")]
fn reap(child: std::process::Child) {
    use std::sync::{Arc, Mutex};

    let slot = Arc::new(Mutex::new(Some(child)));
    let worker = slot.clone();
    let spawned = std::thread::Builder::new()
        .name("diktier-xdg-open".into())
        .spawn(move || {
            let taken = worker.lock().ok().and_then(|mut slot| slot.take());
            if let Some(mut child) = taken {
                let _ = child.wait();
            }
        });
    if spawned.is_err() {
        // Kein Helferthread verfügbar: lieber hier kurz warten als einen
        // Zombie hinterlassen — `xdg-open` startet nur den Dateimanager.
        let taken = slot.lock().ok().and_then(|mut slot| slot.take());
        if let Some(mut child) = taken {
            let _ = child.wait();
        }
    }
}

/// Öffnet den Config-Ordner. Linux: `xdg-open`. Kein Fokusklau auf dem PTT-Pfad —
/// nur nach explizitem Menüklick.
pub fn open_config_dir() -> Result<(), TrayError> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| TrayError::Failed(e.to_string()))?;
    #[cfg(target_os = "linux")]
    {
        let child = std::process::Command::new("xdg-open")
            .arg(&dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| TrayError::Failed(format!("xdg-open: {e}")))?;
        reap(child);
    }
    #[cfg(windows)]
    {
        // `explorer.exe <dir>` — kein Warten: der Explorer meldet regelmäßig
        // Exitcode 1, obwohl das Fenster aufgeht, und Windows kennt keine
        // Zombies, ein fallengelassenes `Child` kostet nur das Handle bis zum
        // Prozessende. Nur nach explizitem Menüklick, nie auf dem PTT-Pfad
        // (§4.2).
        std::process::Command::new("explorer.exe")
            .arg(&dir)
            .spawn()
            .map_err(|e| TrayError::Failed(format!("explorer.exe: {e}")))?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::mpsc::{self, Receiver, TryRecvError};

    use betrayer::{
        ClickType, Icon, Menu, MenuItem, TrayEvent as BetrayerEvent, TrayIcon, TrayIconBuilder,
    };

    use super::*;

    const ICON_SIZE: u32 = 32;

    struct IconSet {
        starting: Icon,
        downloading: Icon,
        loading: Icon,
        idle: Icon,
        recording: Icon,
        transcribing: Icon,
        error: Icon,
        paused: Icon,
    }

    impl IconSet {
        fn new() -> Result<Self, TrayError> {
            Ok(Self {
                starting: rgb_icon(128, 128, 128)?,
                downloading: rgb_icon(30, 144, 255)?,
                loading: rgb_icon(0, 191, 255)?,
                idle: rgb_icon(46, 204, 64)?,
                recording: rgb_icon(220, 50, 47)?,
                transcribing: rgb_icon(230, 126, 34)?,
                error: rgb_icon(192, 57, 43)?,
                paused: rgb_icon(241, 196, 15)?,
            })
        }

        fn get(&self, status: TrayStatus) -> Icon {
            match status {
                TrayStatus::Starting => self.starting.clone(),
                TrayStatus::Downloading => self.downloading.clone(),
                TrayStatus::Loading => self.loading.clone(),
                TrayStatus::Idle => self.idle.clone(),
                TrayStatus::Recording => self.recording.clone(),
                TrayStatus::Transcribing => self.transcribing.clone(),
                TrayStatus::Error => self.error.clone(),
                TrayStatus::Paused => self.paused.clone(),
            }
        }
    }

    fn rgb_icon(r: u8, g: u8, b: u8) -> Result<Icon, TrayError> {
        let mut rgba = vec![0u8; (ICON_SIZE * ICON_SIZE * 4) as usize];
        for px in rgba.chunks_exact_mut(4) {
            px[0] = r;
            px[1] = g;
            px[2] = b;
            px[3] = 255;
        }
        Icon::from_rgba(rgba, ICON_SIZE, ICON_SIZE)
            .map_err(|e| TrayError::Failed(format!("Icon: {e}")))
    }

    fn build_menu(runtime: &Runtime, model_key: &str) -> Menu<MenuAction> {
        Menu::new([
            MenuItem::button(tooltip_text(runtime, model_key), MenuAction::Status),
            MenuItem::separator(),
            MenuItem::button(pause_menu_label(runtime.paused), MenuAction::TogglePause),
            MenuItem::button("Config-Ordner öffnen", MenuAction::OpenConfigDir),
            MenuItem::separator(),
            MenuItem::button("Beenden", MenuAction::Quit),
        ])
    }

    fn map_betrayer(event: BetrayerEvent<MenuAction>) -> Option<TrayEvent> {
        match event {
            BetrayerEvent::Tray(ClickType::Left) => route_click(TrayClick::Left),
            BetrayerEvent::Tray(ClickType::Right) => route_click(TrayClick::Right),
            BetrayerEvent::Tray(ClickType::Double) => route_click(TrayClick::Double),
            BetrayerEvent::Menu(action) => route_menu(action),
        }
    }

    pub struct BetrayerTray {
        icon: TrayIcon<MenuAction>,
        icons: IconSet,
        rx: Receiver<TrayEvent>,
    }

    impl BetrayerTray {
        pub fn new(runtime: &Runtime, model_key: &str) -> Result<Self, TrayError> {
            let icons = IconSet::new()?;
            let (tx, rx) = mpsc::channel();
            let icon = TrayIconBuilder::new()
                .with_icon(icons.get(tray_status(runtime)))
                .with_tooltip(tooltip_text(runtime, model_key))
                .with_menu(build_menu(runtime, model_key))
                .build(move |event| {
                    if let Some(mapped) = map_betrayer(event) {
                        let _ = tx.send(mapped);
                    }
                })
                .map_err(|e| TrayError::Failed(format!("betrayer: {e}")))?;
            Ok(Self { icon, icons, rx })
        }
    }

    impl TrayBackend for BetrayerTray {
        fn update(&mut self, runtime: &Runtime, model_key: &str) -> Result<(), TrayError> {
            let status = tray_status(runtime);
            self.icon.set_icon(Some(self.icons.get(status)));
            self.icon.set_tooltip(tooltip_text(runtime, model_key));
            self.icon.set_menu(Some(build_menu(runtime, model_key)));
            Ok(())
        }

        fn poll(&mut self) -> Result<Option<TrayEvent>, TrayError> {
            match self.rx.try_recv() {
                Ok(event) => Ok(Some(event)),
                Err(TryRecvError::Empty) => Ok(None),
                Err(TryRecvError::Disconnected) => {
                    Err(TrayError::Failed("Tray-Thread beendet".into()))
                }
            }
        }

        fn backend_name(&self) -> &'static str {
            "betrayer"
        }
    }
}

/// Windows: Notify-Icon über `Shell_NotifyIconW` (windows-plan WP4).
///
/// Aufbau nach Leitentscheidung 2: **ein Owner-Thread**. Fenster, Icons, Menü
/// und das Notify-Icon werden ausschließlich auf dem Thread erzeugt, benutzt
/// und zerstört, der `Win32Tray::new` aufgerufen hat — das ist in beiden
/// Aufrufern derselbe: der Tray-Worker (`daemon/workers.rs::tray_loop` ruft
/// `new_backend`, `update`, `poll` und den `Drop` nacheinander in seiner
/// Schleife) und der Spike `--tray-test` (alles im Hauptthread). Deshalb
/// **kein** `PostMessageW`-Umweg für `update`: `NIM_MODIFY` läuft direkt im
/// Aufruf, und der `WndProc` teilt sich mit den Backend-Methoden denselben
/// `RefCell`-Zustand.
///
/// Das Fenster ist ein **Top-Level**-Fenster (kein `HWND_MESSAGE`): nur solche
/// bekommen die Broadcasts `TaskbarCreated` und `WM_QUERYENDSESSION`/
/// `WM_ENDSESSION`. Sichtbar wird es nie (kein `WS_VISIBLE`,
/// `WS_EX_TOOLWINDOW`), es nimmt also auch keinen Fokus — bis auf die eine
/// Ausnahme aus §4.2, das `SetForegroundWindow` vor dem Kontextmenü.
#[cfg(windows)]
mod windows {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::ffi::c_void;
    use std::ptr;

    use windows_sys::Win32::Foundation::{
        ERROR_CLASS_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
    };
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
        DeleteObject, HBITMAP,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Shell::{
        NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETFOCUS,
        NIM_SETVERSION, NIN_SELECT, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CREATESTRUCTW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW,
        DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW, GWLP_USERDATA,
        GetSystemMetrics, GetWindowLongPtrW, HICON, ICONINFO, MF_GRAYED, MF_SEPARATOR, MF_STRING,
        MSG, PM_REMOVE, PeekMessageW, PostMessageW, RegisterClassW, RegisterWindowMessageW,
        SM_CXSMICON, SetForegroundWindow, SetWindowLongPtrW, TPM_NONOTIFY, TPM_RETURNCMD,
        TPM_RIGHTBUTTON, TrackPopupMenu, UnregisterClassW, WM_APP, WM_CONTEXTMENU, WM_ENDSESSION,
        WM_NCCREATE, WM_NCDESTROY, WM_NULL, WM_QUERYENDSESSION, WNDCLASSW, WS_EX_TOOLWINDOW,
        WS_POPUP,
    };

    use super::*;

    /// `NIN_KEYSELECT` (`WM_USER + 1`) fehlt in windows-sys 0.61, `NIN_SELECT`
    /// (`WM_USER + 0`) gibt es. Der Wert ist stabile `shellapi.h`-ABI — dieselbe
    /// Begründung wie bei `CF_UNICODETEXT` in `inject::windows`.
    const NIN_KEYSELECT: u32 = NIN_SELECT + 1;

    /// Fensterklasse des Tray-Owners. Prozessweit eindeutig.
    const CLASS_NAME: &str = "DiktierTrayOwner";

    /// Es gibt genau ein Icon; die ID ist zusammen mit dem `HWND` sein
    /// Schlüssel für `Shell_NotifyIconW`.
    const ICON_ID: u32 = 1;

    /// `uCallbackMessage`. Eigener Nummernraum: das Fenster gehört nur uns,
    /// der Hook-Thread aus WP2 postet seine `WM_APP+n` an eine Thread-Queue.
    const CALLBACK_MSG: u32 = WM_APP + 1;

    /// `NOTIFYICONDATAW::szTip` ist `[u16; 128]` — 127 Codeunits plus NUL.
    const TOOLTIP_MAX: usize = 127;

    /// Obergrenze je `poll()`: nicht abgearbeitete Nachrichten bleiben in der
    /// Queue, die Schleife kann so nicht endlos drehen (wie in `inject::windows`).
    const MAX_MESSAGES_PER_PUMP: u32 = 256;

    /// Feste Menü-IDs nach Plan WP4. `0` ist reserviert: `TrackPopupMenu`
    /// liefert es für „nichts ausgewählt".
    const MENU_STATUS: u32 = 1000;
    const MENU_TOGGLE_PAUSE: u32 = 1001;
    const MENU_OPEN_CONFIG: u32 = 1002;
    const MENU_QUIT: u32 = 1003;

    /// Geschlossenes Mapping der Menü-IDs (§4.3). Unbekannte IDs — auch die `0`
    /// für „Menü ohne Auswahl geschlossen" — ergeben keine Aktion.
    pub(super) fn menu_action(id: u32) -> Option<MenuAction> {
        match id {
            MENU_STATUS => Some(MenuAction::Status),
            MENU_TOGGLE_PAUSE => Some(MenuAction::TogglePause),
            MENU_OPEN_CONFIG => Some(MenuAction::OpenConfigDir),
            MENU_QUIT => Some(MenuAction::Quit),
            _ => None,
        }
    }

    /// Tooltip als NUL-terminierter UTF-16-Puffer, gekürzt auf 127 Codeunits.
    ///
    /// Gekürzt wird an **Zeichengrenzen**: ein Surrogatpaar (Emoji in einer
    /// Fehlermeldung) darf nicht halbiert werden, sonst zeigt Windows ein
    /// Ersatzzeichen.
    pub(super) fn tooltip_utf16(text: &str) -> Vec<u16> {
        let mut out: Vec<u16> = Vec::with_capacity(TOOLTIP_MAX + 1);
        let mut buf = [0u16; 2];
        for ch in text.chars() {
            let encoded = ch.encode_utf16(&mut buf);
            if out.len() + encoded.len() > TOOLTIP_MAX {
                break;
            }
            out.extend_from_slice(encoded);
        }
        out.push(0);
        out
    }

    /// Farben wie im Linux-`IconSet` — derselbe Zustand sieht auf beiden
    /// Plattformen gleich aus.
    pub(super) fn icon_rgb(status: TrayStatus) -> (u8, u8, u8) {
        match status {
            TrayStatus::Starting => (128, 128, 128),
            TrayStatus::Downloading => (30, 144, 255),
            TrayStatus::Loading => (0, 191, 255),
            TrayStatus::Idle => (46, 204, 64),
            TrayStatus::Recording => (220, 50, 47),
            TrayStatus::Transcribing => (230, 126, 34),
            TrayStatus::Error => (192, 57, 43),
            TrayStatus::Paused => (241, 196, 15),
        }
    }

    /// Platz im `IconSet`.
    pub(super) fn icon_index(status: TrayStatus) -> usize {
        match status {
            TrayStatus::Starting => 0,
            TrayStatus::Downloading => 1,
            TrayStatus::Loading => 2,
            TrayStatus::Idle => 3,
            TrayStatus::Recording => 4,
            TrayStatus::Transcribing => 5,
            TrayStatus::Error => 6,
            TrayStatus::Paused => 7,
        }
    }

    pub(super) const ALL_STATES: [TrayStatus; 8] = [
        TrayStatus::Starting,
        TrayStatus::Downloading,
        TrayStatus::Loading,
        TrayStatus::Idle,
        TrayStatus::Recording,
        TrayStatus::Transcribing,
        TrayStatus::Error,
        TrayStatus::Paused,
    ];

    /// NUL-terminierter UTF-16-Puffer für die `W`-APIs.
    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn last_error() -> u32 {
        // SAFETY: parameterlos, liest den Fehlercode dieses Threads.
        unsafe { GetLastError() }
    }

    // ------------------------------------------------------------- Icons

    /// Ein `HICON` je Zustand, in **einer** Größe (`SM_CXSMICON`). Die Shell
    /// skaliert selbst, wenn ein Monitor eine andere DPI hat; ein zweiter Satz
    /// wäre für den Dev-Milestone Aufwand ohne sichtbaren Gewinn.
    struct IconSet {
        icons: [HICON; 8],
    }

    impl IconSet {
        fn new() -> Result<Self, TrayError> {
            // SAFETY: parameterloser Lesezugriff auf eine Systemmetrik.
            let size = unsafe { GetSystemMetrics(SM_CXSMICON) };
            let size = if size <= 0 { 16 } else { size };
            let mut icons: [HICON; 8] = [ptr::null_mut(); 8];
            for status in ALL_STATES {
                let (r, g, b) = icon_rgb(status);
                match make_icon(size, r, g, b) {
                    Ok(icon) => icons[icon_index(status)] = icon,
                    Err(err) => {
                        // Schon erzeugte Icons nicht liegen lassen.
                        for icon in icons.into_iter().filter(|i| !i.is_null()) {
                            // SAFETY: eigenes, noch nicht benutztes Icon.
                            unsafe { DestroyIcon(icon) };
                        }
                        return Err(err);
                    }
                }
            }
            Ok(Self { icons })
        }

        fn get(&self, status: TrayStatus) -> HICON {
            self.icons[icon_index(status)]
        }

        /// Erst **nach** `NIM_DELETE` aufrufen — vorher zeigt die Shell noch
        /// darauf.
        fn destroy(&mut self) {
            for icon in self.icons.iter_mut() {
                if !icon.is_null() {
                    // SAFETY: eigenes Icon aus `CreateIconIndirect`; die Shell
                    // hält nach `NIM_DELETE` keine Referenz mehr.
                    unsafe { DestroyIcon(*icon) };
                    *icon = ptr::null_mut();
                }
            }
        }
    }

    /// Gefüllter Farbkreis auf transparentem Grund, 32 bpp mit Alpha.
    fn make_icon(size: i32, r: u8, g: u8, b: u8) -> Result<HICON, TrayError> {
        // SAFETY: `BITMAPINFO` ist ein reiner POD-Header ohne Zeiger; genullt
        // ist er ein gültiger Ausgangszustand.
        let mut bmi: BITMAPINFO = unsafe { std::mem::zeroed() };
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            // Negativ = top-down: Zeile 0 ist die oberste.
            biHeight: -size,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        };

        let mut bits: *mut c_void = ptr::null_mut();
        // SAFETY: `bmi` lebt über den Aufruf; `hdc`/`hSection` dürfen NULL sein
        // (dokumentierte Form „DIB im Prozessspeicher"). `bits` wird gesetzt
        // und gehört danach dem zurückgegebenen `HBITMAP`.
        let color: HBITMAP = unsafe {
            CreateDIBSection(
                ptr::null_mut(),
                &bmi,
                DIB_RGB_COLORS,
                &mut bits,
                ptr::null_mut(),
                0,
            )
        };
        if color.is_null() || bits.is_null() {
            return Err(TrayError::Failed(format!(
                "Icon-DIB nicht erzeugbar: Win32-Fehler {}",
                last_error()
            )));
        }

        {
            let count = (size as usize) * (size as usize);
            // SAFETY: `CreateDIBSection` hat genau `size*size` 32-Bit-Pixel
            // alloziert (bei 32 bpp sind die Zeilen von Haus aus
            // 4-Byte-ausgerichtet), und niemand sonst hält den Zeiger.
            let pixels = unsafe { std::slice::from_raw_parts_mut(bits as *mut u32, count) };
            // Vorpremultipliziertes BGRA. Alpha ist nur 0 oder 255, deshalb ist
            // die Premultiplikation entweder identisch oder alles null.
            let opaque = u32::from(b) | (u32::from(g) << 8) | (u32::from(r) << 16) | 0xFF00_0000;
            let center = (size as f32 - 1.0) / 2.0;
            let radius = (size as f32) / 2.0 - 0.5;
            for y in 0..size {
                for x in 0..size {
                    let dx = x as f32 - center;
                    let dy = y as f32 - center;
                    let inside = dx * dx + dy * dy <= radius * radius;
                    pixels[(y as usize) * (size as usize) + x as usize] =
                        if inside { opaque } else { 0 };
                }
            }
        }

        // `CreateIconIndirect` verlangt eine Maske, auch bei 32-bpp-Alpha.
        // Ganz null heißt „überall zeichnen"; die Sichtbarkeit steuert der
        // Alphakanal. Monochrome Bitmaps sind 2-Byte-zeilenausgerichtet.
        let stride = (size as usize).div_ceil(16) * 2;
        let mask_bits = vec![0u8; stride * size as usize];
        // SAFETY: `mask_bits` ist groß genug für `size` Zeilen à `stride` Bytes
        // und lebt über den Aufruf; GDI kopiert die Daten.
        let mask: HBITMAP =
            unsafe { CreateBitmap(size, size, 1, 1, mask_bits.as_ptr() as *const c_void) };
        if mask.is_null() {
            let err = last_error();
            // SAFETY: eigenes, noch nirgends verwendetes GDI-Objekt.
            unsafe { DeleteObject(color as *mut c_void) };
            return Err(TrayError::Failed(format!(
                "Icon-Maske nicht erzeugbar: Win32-Fehler {err}"
            )));
        }

        let info = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        // SAFETY: beide Bitmaps sind gültig und gleich groß;
        // `CreateIconIndirect` kopiert sie, deshalb dürfen sie danach weg.
        let icon = unsafe { CreateIconIndirect(&info) };
        let err = last_error();
        // SAFETY: eigene GDI-Objekte, vom Icon nur kopiert, nicht übernommen.
        unsafe {
            DeleteObject(mask as *mut c_void);
            DeleteObject(color as *mut c_void);
        }
        if icon.is_null() {
            return Err(TrayError::Failed(format!(
                "Icon nicht erzeugbar: Win32-Fehler {err}"
            )));
        }
        Ok(icon)
    }

    // ------------------------------------------------------------- State

    /// Was `WndProc` und die Backend-Methoden teilen. Beide laufen auf
    /// **demselben** Thread, deshalb `RefCell` statt Mutex (wie in
    /// `inject::windows`).
    struct TrayState {
        /// Von `poll()` abgeholte Ereignisse.
        events: VecDeque<TrayEvent>,
        /// Für die Beschriftung „Hotkey pausieren"/„… wieder aktivieren".
        paused: bool,
        /// Die (nicht klickbare) Statuszeile des Menüs = Tooltip-Text.
        status_line: String,
        /// Zuletzt gesetztes Icon und Tooltip — `TaskbarCreated` baut damit
        /// das Notify-Icon unverändert wieder auf.
        icon: HICON,
        tip: Vec<u16>,
        /// Message-ID von `RegisterWindowMessageW("TaskbarCreated")`.
        taskbar_created: u32,
    }

    impl TrayState {
        fn push(&mut self, event: TrayEvent) {
            self.events.push_back(event);
        }
    }

    /// Grundgerüst für jeden `Shell_NotifyIconW`-Aufruf.
    fn notify_data(hwnd: HWND) -> NOTIFYICONDATAW {
        // SAFETY: `NOTIFYICONDATAW` ist POD; genullt ist es der dokumentierte
        // Ausgangszustand, `cbSize` sagt der Shell, welche Felder gelten.
        let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = hwnd;
        data.uID = ICON_ID;
        data
    }

    /// `NIM_ADD`/`NIM_MODIFY` mit Icon und Tooltip aus dem Zustand.
    fn icon_data(hwnd: HWND, state: &TrayState) -> NOTIFYICONDATAW {
        let mut data = notify_data(hwnd);
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        data.uCallbackMessage = CALLBACK_MSG;
        data.hIcon = state.icon;
        let len = state.tip.len().min(data.szTip.len());
        data.szTip[..len].copy_from_slice(&state.tip[..len]);
        data
    }

    /// `NIM_ADD` + `NIM_SETVERSION(NOTIFYICON_VERSION_4)` — die vollständige
    /// Sequenz, die auch nach `TaskbarCreated` wiederholt werden muss.
    fn add_icon(hwnd: HWND, state: &TrayState) -> Result<(), TrayError> {
        let data = icon_data(hwnd, state);
        // SAFETY: `data` ist vollständig initialisiert und lebt über den
        // Aufruf; `hWnd` gehört diesem Thread.
        if unsafe { Shell_NotifyIconW(NIM_ADD, &data) } == 0 {
            return Err(TrayError::Failed(format!(
                "Notify-Icon nicht anlegbar (NIM_ADD): Win32-Fehler {}",
                last_error()
            )));
        }
        let mut version = notify_data(hwnd);
        version.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        // SAFETY: wie oben; für `NIM_SETVERSION` zählen cbSize/hWnd/uID/uVersion.
        if unsafe { Shell_NotifyIconW(NIM_SETVERSION, &version) } == 0 {
            let err = last_error();
            // SAFETY: das gerade angelegte Icon wieder abräumen.
            unsafe { Shell_NotifyIconW(NIM_DELETE, &notify_data(hwnd)) };
            return Err(TrayError::Failed(format!(
                "Notify-Icon-Version 4 abgelehnt (NIM_SETVERSION): Win32-Fehler {err}"
            )));
        }
        Ok(())
    }

    // ----------------------------------------------------------- WndProc

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_NCCREATE {
            // SAFETY: Für `WM_NCCREATE` garantiert Windows eine gültige
            // `CREATESTRUCTW`; `lpCreateParams` ist der Zeiger aus
            // `CreateWindowExW`.
            let create = unsafe { &*(lparam as *const CREATESTRUCTW) };
            // SAFETY: `hwnd` ist gültig, `GWLP_USERDATA` gehört der Anwendung.
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize) };
            // SAFETY: unveränderte Parameter an die Default-Behandlung.
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        }

        // SAFETY: `hwnd` ist gültig; der Wert ist entweder 0 (vor `WM_NCCREATE`,
        // nach `WM_NCDESTROY`) oder der oben gesetzte Zeiger.
        let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const RefCell<TrayState>;
        if raw.is_null() {
            // SAFETY: unveränderte Parameter an die Default-Behandlung.
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        }
        // SAFETY: Der Zeiger stammt aus der `Box` in `Win32Tray::state`, die
        // länger lebt als das Fenster (`Drop` zerstört erst das Fenster). Der
        // `WndProc` läuft nur auf dem Thread, dem beides gehört.
        let cell = unsafe { &*raw };

        if msg == CALLBACK_MSG {
            on_callback(cell, hwnd, wparam, lparam);
            return 0;
        }
        // Dynamische ID, deshalb kein `match`-Arm.
        let taskbar_created = cell.try_borrow().map(|s| s.taskbar_created).unwrap_or(0);
        if taskbar_created != 0 && msg == taskbar_created {
            on_taskbar_created(cell, hwnd);
            return 0;
        }

        match msg {
            // Plan WP4: zustimmen, sonst nichts tun.
            WM_QUERYENDSESSION => return 1,
            WM_ENDSESSION => {
                // `wParam == 0` heißt „Abmeldung doch abgebrochen".
                if wparam != 0
                    && let Ok(mut state) = cell.try_borrow_mut()
                {
                    state.push(TrayEvent::Quit);
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

    /// Callback-Dekodierung für `NOTIFYICON_VERSION_4`: das Ereignis steht im
    /// **Low-Word von `lParam`**, der Ankerpunkt in `wParam`.
    fn on_callback(cell: &RefCell<TrayState>, hwnd: HWND, wparam: WPARAM, lparam: LPARAM) {
        let event = (lparam as u32) & 0xFFFF;
        match event {
            // Genau einmal pro Linksklick bzw. Tastaturauswahl; `WM_LBUTTONUP`
            // wird bewusst **nicht** zusätzlich ausgewertet (Plan WP4).
            NIN_SELECT | NIN_KEYSELECT => {
                if let Ok(mut state) = cell.try_borrow_mut() {
                    state.push(TrayEvent::LeftClick);
                }
            }
            WM_CONTEXTMENU => {
                let x = i32::from((wparam as u32 & 0xFFFF) as i16);
                let y = i32::from(((wparam as u32 >> 16) & 0xFFFF) as i16);
                show_menu(cell, hwnd, x, y);
            }
            _ => {}
        }
    }

    /// Explorer-Neustart: die komplette Anlegesequenz wiederholen.
    fn on_taskbar_created(cell: &RefCell<TrayState>, hwnd: HWND) {
        let Ok(state) = cell.try_borrow() else {
            return;
        };
        if let Err(err) = add_icon(hwnd, &state) {
            // Kein Logger in diesem Modul (wie `inject::windows`); der Tray ist
            // danach unsichtbar, der Daemon läuft weiter — §10 macht nur den
            // Aufbau **beim Start** fatal.
            eprintln!("Tray: Icon nach TaskbarCreated nicht wiederhergestellt: {err}");
        }
    }

    /// Kontextmenü nach §4.3, Ablauf nach der §4.2-Ausnahme.
    ///
    /// Während `TrackPopupMenu` blockiert, wird nichts gepumpt und kein Command
    /// bearbeitet — akzeptiert, das Menü ist eine Nutzerinteraktion. Wichtig:
    /// **kein** `RefCell`-Borrow über den Aufruf hinweg, `TrackPopupMenu` pumpt
    /// intern und ruft diesen `WndProc` erneut auf.
    fn show_menu(cell: &RefCell<TrayState>, hwnd: HWND, x: i32, y: i32) {
        let Ok((paused, status_line)) = cell
            .try_borrow()
            .map(|state| (state.paused, state.status_line.clone()))
        else {
            return;
        };

        // SAFETY: parameterlos.
        let menu = unsafe { CreatePopupMenu() };
        if menu.is_null() {
            eprintln!(
                "Tray: Kontextmenü nicht erzeugbar: Win32-Fehler {}",
                last_error()
            );
            return;
        }

        let status = wide(&status_line);
        let pause = wide(pause_menu_label(paused));
        let config = wide("Config-Ordner öffnen");
        let quit = wide("Beenden");
        // SAFETY: `menu` ist gültig, alle Textzeiger sind NUL-terminiert und
        // leben bis nach `TrackPopupMenu`; `AppendMenuW` kopiert sie ohnehin.
        unsafe {
            AppendMenuW(
                menu,
                MF_STRING | MF_GRAYED,
                MENU_STATUS as usize,
                status.as_ptr(),
            );
            AppendMenuW(menu, MF_STRING, MENU_TOGGLE_PAUSE as usize, pause.as_ptr());
            AppendMenuW(menu, MF_STRING, MENU_OPEN_CONFIG as usize, config.as_ptr());
            AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null());
            AppendMenuW(menu, MF_STRING, MENU_QUIT as usize, quit.as_ptr());
        }

        // SPEC §4.2, einzige Ausnahme: ohne aktiviertes Owner-Fenster schließt
        // Win32 das Popup bei einem Klick daneben nicht.
        // SAFETY: eigenes Fenster dieses Threads.
        unsafe { SetForegroundWindow(hwnd) };
        // SAFETY: `menu` gehört uns, `hwnd` ist das Owner-Fenster,
        // `prcRect = NULL` ist die dokumentierte Form „keine Sperrzone".
        let chosen = unsafe {
            TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
                x,
                y,
                0,
                hwnd,
                ptr::null(),
            )
        };
        // SAFETY: die vom Plan geforderte Nachbereitung — `WM_NULL` weckt das
        // Fenster, `NIM_SETFOCUS` gibt den Fokus an die Shell zurück.
        unsafe {
            PostMessageW(hwnd, WM_NULL, 0, 0);
            Shell_NotifyIconW(NIM_SETFOCUS, &notify_data(hwnd));
            DestroyMenu(menu);
        }

        if chosen > 0
            && let Some(action) = menu_action(chosen as u32)
            && let Some(event) = route_menu(action)
            && let Ok(mut state) = cell.try_borrow_mut()
        {
            state.push(event);
        }
    }

    // ----------------------------------------------------------- Backend

    pub struct Win32Tray {
        hwnd: HWND,
        instance: HINSTANCE,
        class_name: Vec<u16>,
        /// Nur eine selbst registrierte Klasse wird im `Drop` abgemeldet.
        owns_class: bool,
        /// Boxed, damit die Adresse stabil bleibt — der `WndProc` kennt sie
        /// über `GWLP_USERDATA`.
        state: Box<RefCell<TrayState>>,
        icons: IconSet,
        added: bool,
    }

    impl Win32Tray {
        pub fn new(runtime: &Runtime, model_key: &str) -> Result<Self, TrayError> {
            let mut icons = IconSet::new()?;
            let class_name = wide(CLASS_NAME);
            let window_name = wide("diktier tray");

            let taskbar_name = wide("TaskbarCreated");
            // SAFETY: NUL-terminierter Puffer, der über den Aufruf lebt.
            let taskbar_created = unsafe { RegisterWindowMessageW(taskbar_name.as_ptr()) };
            if taskbar_created == 0 {
                let err = last_error();
                icons.destroy();
                return Err(TrayError::Failed(format!(
                    "TaskbarCreated nicht registrierbar: Win32-Fehler {err}"
                )));
            }

            // SAFETY: `GetModuleHandleW(NULL)` liefert das eigene Modul-Handle
            // und überträgt kein Eigentum.
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
            // SAFETY: `class` ist vollständig initialisiert, `lpszClassName`
            // zeigt in `class_name`, das noch lebt; `wnd_proc` hat die von
            // `WNDPROC` geforderte Signatur.
            let atom = unsafe { RegisterClassW(&class) };
            let owns_class = if atom == 0 {
                let err = last_error();
                if err != ERROR_CLASS_ALREADY_EXISTS {
                    icons.destroy();
                    return Err(TrayError::Failed(format!(
                        "Fensterklasse {CLASS_NAME} nicht registrierbar: Win32-Fehler {err}"
                    )));
                }
                false
            } else {
                true
            };

            let status_line = tooltip_text(runtime, model_key);
            let state = Box::new(RefCell::new(TrayState {
                events: VecDeque::new(),
                paused: runtime.paused,
                icon: icons.get(tray_status(runtime)),
                tip: tooltip_utf16(&status_line),
                status_line,
                taskbar_created,
            }));
            let state_ptr: *const RefCell<TrayState> = &*state;

            // SAFETY: Alle Zeiger zeigen auf lebende, NUL-terminierte Puffer.
            // Top-Level (Parent NULL) wegen der Broadcasts, ohne `WS_VISIBLE`
            // und mit `WS_EX_TOOLWINDOW` also unsichtbar und ohne
            // Taskbar-Eintrag. `state_ptr` erreicht den `WndProc` als
            // `lpCreateParams` in `WM_NCCREATE`; die Box lebt länger als das
            // Fenster (siehe `Drop`).
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_TOOLWINDOW,
                    class_name.as_ptr(),
                    window_name.as_ptr(),
                    WS_POPUP,
                    0,
                    0,
                    0,
                    0,
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
                icons.destroy();
                return Err(TrayError::Failed(format!(
                    "Tray-Fenster nicht erzeugbar: Win32-Fehler {err}"
                )));
            }

            let mut tray = Self {
                hwnd,
                instance,
                class_name,
                owns_class,
                state,
                icons,
                added: false,
            };
            // §10: Ein gescheiterter Tray-Aufbau ist fatal (Exit 1). Das `?`
            // lässt `tray` fallen — der `Drop` räumt Fenster, Klasse und Icons
            // ab, `added` ist dabei noch `false`.
            add_icon(tray.hwnd, &tray.state.borrow())?;
            tray.added = true;
            Ok(tray)
        }

        /// `PeekMessageW`-Pump. Nicht blockierend: `poll()` darf die
        /// Worker-Schleife nicht aufhalten.
        fn pump(&mut self) {
            // SAFETY: `MSG` ist POD; `PeekMessageW` füllt die Struktur.
            let mut msg: MSG = unsafe { std::mem::zeroed() };
            for _ in 0..MAX_MESSAGES_PER_PUMP {
                // SAFETY: `msg` ist gültiger Speicher; `hWnd = NULL` holt die
                // Nachrichten **dieses** Threads — dort gehört nur unser
                // Fenster uns.
                if unsafe { PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_REMOVE) } == 0 {
                    break;
                }
                // Kein `TranslateMessage`: der Tray verarbeitet keine
                // Zeicheneingabe.
                // SAFETY: `msg` stammt unverändert aus `PeekMessageW`.
                unsafe { DispatchMessageW(&msg) };
            }
        }
    }

    impl TrayBackend for Win32Tray {
        /// Läuft auf dem Owner-Thread (siehe Modul-Doc), deshalb direkt
        /// `NIM_MODIFY` statt `PostMessageW` + Mutex-Slot.
        fn update(&mut self, runtime: &Runtime, model_key: &str) -> Result<(), TrayError> {
            let status = tray_status(runtime);
            let tip = tooltip_text(runtime, model_key);
            let icon = self.icons.get(status);
            let data = {
                let mut state = self.state.borrow_mut();
                state.paused = runtime.paused;
                state.tip = tooltip_utf16(&tip);
                state.status_line = tip;
                state.icon = icon;
                icon_data(self.hwnd, &state)
            };
            // SAFETY: `data` ist vollständig initialisiert und lebt über den
            // Aufruf; das Icon existiert, sonst gäbe es dieses Backend nicht.
            if unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) } == 0 {
                return Err(TrayError::Failed(format!(
                    "Tooltip/Icon nicht aktualisierbar (NIM_MODIFY): Win32-Fehler {}",
                    last_error()
                )));
            }
            Ok(())
        }

        fn poll(&mut self) -> Result<Option<TrayEvent>, TrayError> {
            self.pump();
            Ok(self.state.borrow_mut().events.pop_front())
        }

        fn backend_name(&self) -> &'static str {
            "shell-notifyicon"
        }
    }

    impl Drop for Win32Tray {
        /// Läuft auf dem Owner-Thread — `tray_loop` legt das Backend am Ende
        /// seiner eigenen Schleife ab, der Spike beim Verlassen von
        /// `tray_test`.
        fn drop(&mut self) {
            if self.added {
                // SAFETY: `notify_data` ist vollständig initialisiert; Icon und
                // Fenster existieren noch.
                let ok = unsafe { Shell_NotifyIconW(NIM_DELETE, &notify_data(self.hwnd)) } != 0;
                if ok {
                    eprintln!("Tray: Icon entfernt (NIM_DELETE)");
                } else {
                    eprintln!(
                        "Tray: NIM_DELETE fehlgeschlagen: Win32-Fehler {}",
                        last_error()
                    );
                }
                self.added = false;
            }
            if !self.hwnd.is_null() {
                // SAFETY: eigenes Fenster dieses Threads; `WM_NCDESTROY` löscht
                // dabei den `GWLP_USERDATA`-Zeiger, die Box lebt bis danach.
                unsafe { DestroyWindow(self.hwnd) };
                self.hwnd = ptr::null_mut();
            }
            if self.owns_class {
                // SAFETY: eigene Klasse, ihr einziges Fenster ist zerstört.
                unsafe { UnregisterClassW(self.class_name.as_ptr(), self.instance) };
                self.owns_class = false;
            }
            // Erst jetzt: vorher zeigte die Shell noch auf das Icon.
            self.icons.destroy();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_MODEL;
    use crate::state::RecordingSource;
    use std::collections::VecDeque;

    #[derive(Default)]
    struct FakeTray {
        tooltip: String,
        pause_label: String,
        status: Option<TrayStatus>,
        queued: VecDeque<TrayEvent>,
    }

    impl TrayBackend for FakeTray {
        fn update(&mut self, runtime: &Runtime, model_key: &str) -> Result<(), TrayError> {
            self.status = Some(tray_status(runtime));
            self.tooltip = tooltip_text(runtime, model_key);
            self.pause_label = pause_menu_label(runtime.paused).into();
            Ok(())
        }

        fn poll(&mut self) -> Result<Option<TrayEvent>, TrayError> {
            Ok(self.queued.pop_front())
        }

        fn backend_name(&self) -> &'static str {
            "fake"
        }
    }

    impl FakeTray {
        fn push(&mut self, event: TrayEvent) {
            self.queued.push_back(event);
        }
    }

    fn runtime(state: AppState, paused: bool) -> Runtime {
        Runtime {
            state,
            paused,
            ..Runtime::default()
        }
    }

    #[test]
    fn stub_tray_update_succeeds() {
        let mut tray = StubTray;
        tray.update(&Runtime::default(), DEFAULT_MODEL).unwrap();
    }

    #[test]
    fn tooltip_maps_all_spec_states() {
        let model = DEFAULT_MODEL;
        let cases = [
            (runtime(AppState::Starting, false), "starting — "),
            (runtime(AppState::Downloading, false), "downloading — "),
            (runtime(AppState::Loading, false), "loading — "),
            (runtime(AppState::Idle, false), "idle — "),
            (
                runtime(
                    AppState::Recording {
                        source: RecordingSource::Hotkey,
                    },
                    false,
                ),
                "recording — ",
            ),
            (
                runtime(
                    AppState::Transcribing {
                        source: RecordingSource::TrayClick,
                    },
                    false,
                ),
                "transcribing — ",
            ),
            (runtime(AppState::Error, false), "error — "),
            (runtime(AppState::Idle, true), "paused — "),
        ];
        for (rt, prefix) in cases {
            let tip = tooltip_text(&rt, model);
            assert!(tip.starts_with(prefix), "unexpected tooltip {tip}");
            assert!(tip.ends_with(model), "tooltip missing model: {tip}");
        }
    }

    /// §4.3: Eine laufende Tray-Click-Aufnahme muss sichtbar bleiben, auch wenn
    /// der Hotkey pausiert ist — `recording`/`transcribing` schlagen `paused`.
    #[test]
    fn active_states_outrank_the_paused_flag() {
        let rec = runtime(
            AppState::Recording {
                source: RecordingSource::TrayClick,
            },
            true,
        );
        assert_eq!(tray_status(&rec), TrayStatus::Recording);
        assert_eq!(
            tooltip_text(&rec, DEFAULT_MODEL),
            format!("recording — {DEFAULT_MODEL}")
        );

        for state in [
            AppState::Transcribing {
                source: RecordingSource::TrayClick,
            },
            AppState::Injecting {
                source: RecordingSource::Hotkey,
            },
        ] {
            assert_eq!(
                tray_status(&runtime(state, true)),
                TrayStatus::Transcribing,
                "{state:?}"
            );
        }
    }

    /// `paused` zeigt sich nur in den Ruhezuständen (§4.3-Tabelle).
    #[test]
    fn paused_flag_shows_in_resting_states() {
        for state in [
            AppState::Starting,
            AppState::Downloading,
            AppState::Loading,
            AppState::Idle,
            AppState::Error,
        ] {
            assert_eq!(
                tray_status(&runtime(state, true)),
                TrayStatus::Paused,
                "{state:?}"
            );
        }
        assert_eq!(
            tooltip_text(&runtime(AppState::Idle, true), DEFAULT_MODEL),
            format!("paused — {DEFAULT_MODEL}")
        );
    }

    #[test]
    fn fake_backend_records_tooltip_on_update() {
        let mut tray = FakeTray::default();
        let rt = runtime(AppState::Idle, false);
        tray.update(&rt, DEFAULT_MODEL).unwrap();
        assert_eq!(tray.status, Some(TrayStatus::Idle));
        assert_eq!(tray.tooltip, format!("idle — {DEFAULT_MODEL}"));
        assert_eq!(tray.pause_label, "Hotkey pausieren");

        let paused = runtime(AppState::Idle, true);
        tray.update(&paused, DEFAULT_MODEL).unwrap();
        assert_eq!(tray.status, Some(TrayStatus::Paused));
        assert_eq!(tray.pause_label, "Hotkey wieder aktivieren");
    }

    /// §4.4: „Tooltip nennt den Konflikt" — der Fehlergrund steht im Tooltip,
    /// nicht nur im Log (codex M1).
    #[test]
    fn error_tooltip_names_the_reason() {
        let mut rt = runtime(AppState::Error, false);
        rt.error = Some(crate::state::ErrorInfo {
            kind: crate::state::ErrorKind::HotkeyRegistration,
            message: "F9 nicht greifbar: HotKey already registered".into(),
        });
        let tip = tooltip_text(&rt, DEFAULT_MODEL);
        assert!(tip.starts_with("error — "), "{tip}");
        assert!(tip.contains(DEFAULT_MODEL), "{tip}");
        assert!(tip.contains("HotKey already registered"), "{tip}");
    }

    /// Ohne Fehler bleibt der Tooltip exakt wie in §4.3 beschrieben.
    #[test]
    fn tooltip_without_error_stays_short() {
        let mut rt = runtime(AppState::Idle, false);
        assert_eq!(
            tooltip_text(&rt, DEFAULT_MODEL),
            format!("idle — {DEFAULT_MODEL}")
        );
        // Ein Fehlergrund aus einem früheren Lauf zeigt sich nicht in `idle`.
        rt.error = Some(crate::state::ErrorInfo {
            kind: crate::state::ErrorKind::Mic,
            message: "Mikrofon weg".into(),
        });
        assert_eq!(
            tooltip_text(&rt, DEFAULT_MODEL),
            format!("idle — {DEFAULT_MODEL}"),
            "nur der sichtbare Fehlerzustand nennt einen Grund"
        );
    }

    #[test]
    fn menu_event_routing_matches_spec() {
        assert_eq!(route_menu(MenuAction::Status), None);
        assert_eq!(
            route_menu(MenuAction::TogglePause),
            Some(TrayEvent::TogglePause)
        );
        assert_eq!(
            route_menu(MenuAction::OpenConfigDir),
            Some(TrayEvent::OpenConfigDir)
        );
        assert_eq!(route_menu(MenuAction::Quit), Some(TrayEvent::Quit));
    }

    #[test]
    fn click_routing_linux_betrayer() {
        assert_eq!(route_click(TrayClick::Double), Some(TrayEvent::LeftClick));
        assert_eq!(route_click(TrayClick::Left), None);
        assert_eq!(route_click(TrayClick::Right), None);
    }

    #[test]
    fn fake_backend_routes_pause_toggle() {
        let mut tray = FakeTray::default();
        tray.push(TrayEvent::TogglePause);
        tray.push(TrayEvent::LeftClick);
        assert_eq!(tray.poll().unwrap(), Some(TrayEvent::TogglePause));
        assert_eq!(tray.poll().unwrap(), Some(TrayEvent::LeftClick));
        assert_eq!(tray.poll().unwrap(), None);
    }

    #[test]
    fn fake_backend_quit_and_config_events() {
        let mut tray = FakeTray::default();
        tray.push(TrayEvent::OpenConfigDir);
        tray.push(TrayEvent::Quit);
        assert_eq!(tray.poll().unwrap(), Some(TrayEvent::OpenConfigDir));
        assert_eq!(tray.poll().unwrap(), Some(TrayEvent::Quit));
    }

    #[test]
    fn pause_label_toggles_with_flag() {
        assert_eq!(pause_menu_label(false), "Hotkey pausieren");
        assert_eq!(pause_menu_label(true), "Hotkey wieder aktivieren");
    }

    /// Reine Logik des Windows-Backends (WP4) — kein Win32-Aufruf, deshalb
    /// laufen diese Tests auch ohne Sitzung/Tray.
    #[cfg(windows)]
    mod win {
        use super::super::windows::{ALL_STATES, icon_index, icon_rgb, menu_action, tooltip_utf16};
        use super::*;

        /// Plan WP4: feste IDs 1000..1003, geschlossenes Mapping.
        #[test]
        fn menu_ids_map_to_their_actions() {
            assert_eq!(menu_action(1000), Some(MenuAction::Status));
            assert_eq!(menu_action(1001), Some(MenuAction::TogglePause));
            assert_eq!(menu_action(1002), Some(MenuAction::OpenConfigDir));
            assert_eq!(menu_action(1003), Some(MenuAction::Quit));
        }

        /// `TrackPopupMenu` liefert `0`, wenn der Nutzer daneben klickt — das
        /// darf keine Aktion auslösen, ebenso wenig eine fremde ID.
        #[test]
        fn unknown_menu_ids_do_nothing() {
            for id in [0, 1, 999, 1004, u32::MAX] {
                assert_eq!(menu_action(id), None, "{id}");
            }
        }

        /// Ende-zu-Ende über `route_menu`: was das Menü liefert, wird zum
        /// Tray-Ereignis; die Statuszeile bleibt stumm (§4.3).
        #[test]
        fn menu_ids_reach_the_tray_events() {
            let event = |id| menu_action(id).and_then(route_menu);
            assert_eq!(event(1000), None);
            assert_eq!(event(1001), Some(TrayEvent::TogglePause));
            assert_eq!(event(1002), Some(TrayEvent::OpenConfigDir));
            assert_eq!(event(1003), Some(TrayEvent::Quit));
        }

        #[test]
        fn short_tooltips_survive_unchanged_with_nul() {
            let tip = tooltip_utf16("idle — parakeet");
            assert_eq!(*tip.last().unwrap(), 0);
            assert_eq!(
                String::from_utf16(&tip[..tip.len() - 1]).unwrap(),
                "idle — parakeet"
            );
        }

        /// `szTip` fasst 128 Codeunits: 127 Text plus NUL.
        #[test]
        fn long_tooltips_are_cut_to_127_code_units() {
            let tip = tooltip_utf16(&"a".repeat(500));
            assert_eq!(tip.len(), 128);
            assert_eq!(*tip.last().unwrap(), 0);
        }

        /// Ein Surrogatpaar darf die Grenze nicht halbieren — sonst zeigt
        /// Windows ein Ersatzzeichen statt des Zeichens.
        #[test]
        fn tooltips_never_split_a_surrogate_pair() {
            // 126 ASCII + Emoji (2 Codeunits): das Emoji passt nicht mehr ganz.
            let text = format!("{}🎤", "a".repeat(126));
            let tip = tooltip_utf16(&text);
            assert_eq!(tip.len(), 127, "126 Zeichen + NUL");
            assert_eq!(*tip.last().unwrap(), 0);
            String::from_utf16(&tip[..tip.len() - 1]).expect("keine halbe Ersatzzeichenfolge");

            // Ein Zeichen früher passt es vollständig.
            let text = format!("{}🎤", "a".repeat(125));
            let tip = tooltip_utf16(&text);
            assert_eq!(tip.len(), 128);
            assert!(
                String::from_utf16(&tip[..tip.len() - 1])
                    .unwrap()
                    .ends_with('🎤')
            );
        }

        /// Der echte Fehlertooltip aus §4.4 kann lang werden und muss trotzdem
        /// gültiges UTF-16 bleiben.
        #[test]
        fn an_error_tooltip_is_cut_but_stays_valid() {
            let mut rt = runtime(AppState::Error, false);
            rt.error = Some(crate::state::ErrorInfo {
                kind: crate::state::ErrorKind::HotkeyRegistration,
                message: "F9 nicht greifbar: ".repeat(20),
            });
            let text = tooltip_text(&rt, DEFAULT_MODEL);
            assert!(text.encode_utf16().count() > 127);
            let tip = tooltip_utf16(&text);
            assert_eq!(tip.len(), 128);
            let shown = String::from_utf16(&tip[..tip.len() - 1]).unwrap();
            assert!(shown.starts_with("error — "), "{shown}");
        }

        /// Jeder §4.3-Zustand hat genau einen Platz im `IconSet` und eine
        /// eigene Farbe (dieselben wie unter Linux).
        #[test]
        fn every_status_has_its_own_icon_slot_and_color() {
            let mut slots: Vec<usize> = ALL_STATES.iter().copied().map(icon_index).collect();
            slots.sort_unstable();
            assert_eq!(slots, (0..8).collect::<Vec<_>>());

            let mut colors: Vec<(u8, u8, u8)> = ALL_STATES.iter().copied().map(icon_rgb).collect();
            colors.sort_unstable();
            colors.dedup();
            assert_eq!(colors.len(), 8, "jeder Zustand eine eigene Farbe");
            assert_eq!(icon_rgb(TrayStatus::Idle), (46, 204, 64));
            assert_eq!(icon_rgb(TrayStatus::Recording), (220, 50, 47));
        }
    }
}
