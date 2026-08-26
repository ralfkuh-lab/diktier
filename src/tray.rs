//! Tray-Backend (Spec §4.3). Phase 2b: Linux-betrayer, Windows cfg-Stub.
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

pub enum AnyTray {
    #[cfg(target_os = "linux")]
    Betrayer(Box<linux::BetrayerTray>),
    Stub(StubTray),
}

impl TrayBackend for AnyTray {
    fn update(&mut self, runtime: &Runtime, model_key: &str) -> Result<(), TrayError> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Betrayer(inner) => inner.update(runtime, model_key),
            Self::Stub(inner) => inner.update(runtime, model_key),
        }
    }

    fn poll(&mut self) -> Result<Option<TrayEvent>, TrayError> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Betrayer(inner) => inner.poll(),
            Self::Stub(inner) => inner.poll(),
        }
    }

    fn backend_name(&self) -> &'static str {
        match self {
            #[cfg(target_os = "linux")]
            Self::Betrayer(inner) => inner.backend_name(),
            Self::Stub(inner) => inner.backend_name(),
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
pub fn new_backend(_runtime: &Runtime, _model_key: &str) -> Result<AnyTray, TrayError> {
    Ok(AnyTray::Stub(StubTray))
}

pub fn tray_status(runtime: &Runtime) -> TrayStatus {
    if runtime.paused {
        return TrayStatus::Paused;
    }
    match runtime.state {
        AppState::Starting => TrayStatus::Starting,
        AppState::Downloading => TrayStatus::Downloading,
        AppState::Loading => TrayStatus::Loading,
        AppState::Idle => TrayStatus::Idle,
        AppState::Recording { .. } => TrayStatus::Recording,
        AppState::Transcribing { .. } => TrayStatus::Transcribing,
        AppState::Error => TrayStatus::Error,
    }
}

pub fn tooltip_text(runtime: &Runtime, model_key: &str) -> String {
    format!("{} — {model_key}", tray_status(runtime).as_str())
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

/// Öffnet den Config-Ordner. Linux: `xdg-open`. Kein Fokusklau auf dem PTT-Pfad —
/// nur nach explizitem Menüklick.
pub fn open_config_dir() -> Result<(), TrayError> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| TrayError::Failed(e.to_string()))?;
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| TrayError::Failed(format!("xdg-open: {e}")))?;
    }
    #[cfg(windows)]
    {
        let _ = dir;
        return Err(TrayError::Failed(
            "Windows: Config-Ordner öffnen ist Stub (Phase 2b)".into(),
        ));
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
        Runtime { state, paused }
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

    #[test]
    fn paused_flag_overrides_app_state_for_tooltip() {
        let rt = runtime(
            AppState::Recording {
                source: RecordingSource::TrayClick,
            },
            true,
        );
        assert_eq!(tray_status(&rt), TrayStatus::Paused);
        assert_eq!(
            tooltip_text(&rt, DEFAULT_MODEL),
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
}
