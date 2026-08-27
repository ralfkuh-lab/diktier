//! HotkeyBackend: Press/Release (Spec §4.4 / §5.1). Linux: global-hotkey, Fallback XGrabKey.

use thiserror::Error;

use crate::config::{HotkeyConfig, Modifier};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Press,
    Release,
}

#[derive(Debug, Error)]
pub enum HotkeyError {
    #[error("Hotkey fehlgeschlagen: {0}")]
    Failed(String),
}

/// Die tatsächlich zu greifende Taste (§4.4: „Nur über Config änderbar").
///
/// Bis Phase 3c registrierten beide Linux-Backends hart `F9` und ignorierten
/// die validierte Config (codex H3). Diese Struktur ist der Weg von
/// [`HotkeyConfig`] bis in den X11-Grab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeySpec {
    /// Kanonischer Schlüsselname aus der Config (`F9`, `A`, `Space`, …).
    pub key: String,
    pub modifiers: Vec<Modifier>,
}

impl Default for HotkeySpec {
    fn default() -> Self {
        Self {
            key: "F9".into(),
            modifiers: Vec::new(),
        }
    }
}

impl HotkeySpec {
    pub fn from_config(config: &HotkeyConfig) -> Self {
        Self {
            key: config.key.clone(),
            modifiers: config.modifiers.clone(),
        }
    }

    /// Menschenlesbar für Log und Tooltip: `Ctrl+Shift+F9`.
    pub fn describe(&self) -> String {
        let mut out = String::new();
        for modifier in &self.modifiers {
            out.push_str(modifier_name(*modifier));
            out.push('+');
        }
        out.push_str(&self.key);
        out
    }
}

pub fn modifier_name(modifier: Modifier) -> &'static str {
    match modifier {
        Modifier::Ctrl => "Ctrl",
        Modifier::Shift => "Shift",
        Modifier::Alt => "Alt",
        Modifier::Super => "Super",
    }
}

/// X11-Keysym zum kanonischen Config-Schlüssel (§8-Tabelle).
///
/// Buchstaben werden als Kleinbuchstabe gegriffen — das ist die Ebene, auf der
/// X11 den Keycode führt; Shift ist ein Modifier, kein anderer Keysym.
pub fn x11_keysym(key: &str) -> Option<u32> {
    if let Some(rest) = key.strip_prefix('F')
        && let Ok(n) = rest.parse::<u8>()
        && (1..=24).contains(&n)
    {
        // XK_F1 = 0xffbe, danach fortlaufend bis XK_F24.
        return Some(0xffbe + u32::from(n) - 1);
    }
    if key.len() == 1 {
        let c = key.chars().next()?;
        if c.is_ascii_alphabetic() {
            return Some(u32::from(c.to_ascii_lowercase()));
        }
        if c.is_ascii_digit() {
            return Some(u32::from(c));
        }
    }
    Some(match key {
        "Space" => 0x0020,
        "Tab" => 0xff09,
        "Enter" => 0xff0d,
        "Escape" => 0xff1b,
        "Backspace" => 0xff08,
        "Insert" => 0xff63,
        "Delete" => 0xffff,
        "Home" => 0xff50,
        "End" => 0xff57,
        "PageUp" => 0xff55,
        "PageDown" => 0xff56,
        "Left" => 0xff51,
        "Up" => 0xff52,
        "Right" => 0xff53,
        "Down" => 0xff54,
        _ => return None,
    })
}

pub trait HotkeyBackend {
    fn register(&mut self) -> Result<(), HotkeyError>;
    /// §4.4/§5.2: „Hotkey pausieren" muss den Grab wirklich freigeben — sonst
    /// schluckt X11 die Taste weiter und die fokussierte App sieht sie nie
    /// (codex H3).
    fn unregister(&mut self) -> Result<(), HotkeyError> {
        Ok(())
    }
    fn poll(&mut self) -> Result<Option<HotkeyEvent>, HotkeyError>;
    fn is_registered(&self) -> bool {
        false
    }
    fn backend_name(&self) -> &'static str {
        "stub"
    }
}

/// Auto-Repeat → ein logisches Press, ein logisches Release (Spec §4.4).
#[derive(Debug, Default)]
struct Debounce {
    held: bool,
}

impl Debounce {
    fn on_press(&mut self) -> Option<HotkeyEvent> {
        if self.held {
            None
        } else {
            self.held = true;
            Some(HotkeyEvent::Press)
        }
    }

    fn on_release(&mut self) -> Option<HotkeyEvent> {
        if self.held {
            self.held = false;
            Some(HotkeyEvent::Release)
        } else {
            None
        }
    }
}

/// Vertragsprobe: registriert folgenlos und meldet nie ein Event.
///
/// Seit Phase 5/WP2 hat jede unterstützte Plattform ein echtes Backend
/// (Linux: `global-hotkey`/XGrabKey, Windows: `win32-ll-hook`) — `new_backend`
/// baut den Stub nirgends mehr. Er bleibt als Prüfstein für die
/// Trait-Defaults im Test.
#[allow(dead_code)]
#[derive(Debug, Default)]
pub struct StubHotkeyBackend;

impl HotkeyBackend for StubHotkeyBackend {
    fn register(&mut self) -> Result<(), HotkeyError> {
        Ok(())
    }

    fn poll(&mut self) -> Result<Option<HotkeyEvent>, HotkeyError> {
        Ok(None)
    }
}

pub enum AnyHotkeyBackend {
    #[cfg(target_os = "linux")]
    Global(linux::GlobalHotkeyBackend),
    #[cfg(target_os = "linux")]
    Grab(linux::X11GrabKeyBackend),
    /// Windows (Phase 5/WP2): `WH_KEYBOARD_LL` auf eigenem Hook-Thread (§5).
    #[cfg(windows)]
    WinHook(windows::WinHookBackend),
}

impl HotkeyBackend for AnyHotkeyBackend {
    fn register(&mut self) -> Result<(), HotkeyError> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Global(inner) => inner.register(),
            #[cfg(target_os = "linux")]
            Self::Grab(inner) => inner.register(),
            #[cfg(windows)]
            Self::WinHook(inner) => inner.register(),
        }
    }

    fn unregister(&mut self) -> Result<(), HotkeyError> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Global(inner) => inner.unregister(),
            #[cfg(target_os = "linux")]
            Self::Grab(inner) => inner.unregister(),
            #[cfg(windows)]
            Self::WinHook(inner) => inner.unregister(),
        }
    }

    fn poll(&mut self) -> Result<Option<HotkeyEvent>, HotkeyError> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Global(inner) => inner.poll(),
            #[cfg(target_os = "linux")]
            Self::Grab(inner) => inner.poll(),
            #[cfg(windows)]
            Self::WinHook(inner) => inner.poll(),
        }
    }

    fn is_registered(&self) -> bool {
        match self {
            #[cfg(target_os = "linux")]
            Self::Global(inner) => inner.is_registered(),
            #[cfg(target_os = "linux")]
            Self::Grab(inner) => inner.is_registered(),
            #[cfg(windows)]
            Self::WinHook(inner) => inner.is_registered(),
        }
    }

    fn backend_name(&self) -> &'static str {
        match self {
            #[cfg(target_os = "linux")]
            Self::Global(inner) => inner.backend_name(),
            #[cfg(target_os = "linux")]
            Self::Grab(inner) => inner.backend_name(),
            #[cfg(windows)]
            Self::WinHook(inner) => inner.backend_name(),
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
    use std::thread;
    use std::time::Duration;

    use global_hotkey::hotkey::{Code, HotKey, Modifiers};
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
    use x11rb::connection::Connection;
    use x11rb::protocol::Event;
    use x11rb::protocol::xkb;
    use x11rb::protocol::xkb::ConnectionExt as XkbExt;
    use x11rb::protocol::xproto::{ConnectionExt as XprotoExt, GrabMode, Keycode, ModMask, Window};
    use x11rb::rust_connection::RustConnection;

    use super::{Debounce, HotkeyBackend, HotkeyError, HotkeyEvent, HotkeySpec, x11_keysym};
    use crate::config::Modifier;

    /// Config-Schlüssel → `global-hotkey`-Code (§8-Tabelle).
    pub fn global_code(key: &str) -> Option<Code> {
        if let Some(rest) = key.strip_prefix('F')
            && let Ok(n) = rest.parse::<u8>()
        {
            return function_code(n);
        }
        if key.len() == 1 {
            let c = key.chars().next()?;
            if c.is_ascii_alphabetic() {
                return letter_code(c.to_ascii_uppercase());
            }
            if c.is_ascii_digit() {
                return digit_code(c);
            }
        }
        Some(match key {
            "Space" => Code::Space,
            "Tab" => Code::Tab,
            "Enter" => Code::Enter,
            "Escape" => Code::Escape,
            "Backspace" => Code::Backspace,
            "Insert" => Code::Insert,
            "Delete" => Code::Delete,
            "Home" => Code::Home,
            "End" => Code::End,
            "PageUp" => Code::PageUp,
            "PageDown" => Code::PageDown,
            "Left" => Code::ArrowLeft,
            "Up" => Code::ArrowUp,
            "Right" => Code::ArrowRight,
            "Down" => Code::ArrowDown,
            _ => return None,
        })
    }

    fn function_code(n: u8) -> Option<Code> {
        Some(match n {
            1 => Code::F1,
            2 => Code::F2,
            3 => Code::F3,
            4 => Code::F4,
            5 => Code::F5,
            6 => Code::F6,
            7 => Code::F7,
            8 => Code::F8,
            9 => Code::F9,
            10 => Code::F10,
            11 => Code::F11,
            12 => Code::F12,
            13 => Code::F13,
            14 => Code::F14,
            15 => Code::F15,
            16 => Code::F16,
            17 => Code::F17,
            18 => Code::F18,
            19 => Code::F19,
            20 => Code::F20,
            21 => Code::F21,
            22 => Code::F22,
            23 => Code::F23,
            24 => Code::F24,
            _ => return None,
        })
    }

    fn letter_code(c: char) -> Option<Code> {
        Some(match c {
            'A' => Code::KeyA,
            'B' => Code::KeyB,
            'C' => Code::KeyC,
            'D' => Code::KeyD,
            'E' => Code::KeyE,
            'F' => Code::KeyF,
            'G' => Code::KeyG,
            'H' => Code::KeyH,
            'I' => Code::KeyI,
            'J' => Code::KeyJ,
            'K' => Code::KeyK,
            'L' => Code::KeyL,
            'M' => Code::KeyM,
            'N' => Code::KeyN,
            'O' => Code::KeyO,
            'P' => Code::KeyP,
            'Q' => Code::KeyQ,
            'R' => Code::KeyR,
            'S' => Code::KeyS,
            'T' => Code::KeyT,
            'U' => Code::KeyU,
            'V' => Code::KeyV,
            'W' => Code::KeyW,
            'X' => Code::KeyX,
            'Y' => Code::KeyY,
            'Z' => Code::KeyZ,
            _ => return None,
        })
    }

    fn digit_code(c: char) -> Option<Code> {
        Some(match c {
            '0' => Code::Digit0,
            '1' => Code::Digit1,
            '2' => Code::Digit2,
            '3' => Code::Digit3,
            '4' => Code::Digit4,
            '5' => Code::Digit5,
            '6' => Code::Digit6,
            '7' => Code::Digit7,
            '8' => Code::Digit8,
            '9' => Code::Digit9,
            _ => return None,
        })
    }

    pub fn global_modifiers(modifiers: &[Modifier]) -> Option<Modifiers> {
        if modifiers.is_empty() {
            return None;
        }
        let mut out = Modifiers::empty();
        for modifier in modifiers {
            out |= match modifier {
                Modifier::Ctrl => Modifiers::CONTROL,
                Modifier::Shift => Modifiers::SHIFT,
                Modifier::Alt => Modifiers::ALT,
                Modifier::Super => Modifiers::SUPER,
            };
        }
        Some(out)
    }

    /// X11-Modifiermaske für `XGrabKey`.
    pub fn x11_mod_mask(modifiers: &[Modifier]) -> ModMask {
        let mut mask = ModMask::default();
        for modifier in modifiers {
            mask |= match modifier {
                Modifier::Ctrl => ModMask::CONTROL,
                Modifier::Shift => ModMask::SHIFT,
                // Alt liegt üblicherweise auf mod1, Super auf mod4.
                Modifier::Alt => ModMask::M1,
                Modifier::Super => ModMask::M4,
            };
        }
        mask
    }

    pub struct GlobalHotkeyBackend {
        /// `None`, solange pausiert ist: `GlobalHotKeyManager::unregister`
        /// entfernt den Hotkey nur aus seiner Tabelle, der X11-Grab bleibt
        /// bestehen (live nachgewiesen). Erst das Fallenlassen des Managers
        /// schließt seine X11-Verbindung — und damit gibt der Server den Grab
        /// frei, wie §4.4 es für „Hotkey pausieren" verlangt (codex H3).
        manager: Option<GlobalHotKeyManager>,
        hotkey: HotKey,
        keysym: u32,
        debounce: Debounce,
        registered: bool,
    }

    impl GlobalHotkeyBackend {
        pub fn try_new(spec: &HotkeySpec) -> Result<Self, HotkeyError> {
            let code = global_code(&spec.key).ok_or_else(|| {
                HotkeyError::Failed(format!(
                    "hotkey.key {:?} kennt global-hotkey nicht",
                    spec.key
                ))
            })?;
            let keysym = x11_keysym(&spec.key).ok_or_else(|| {
                HotkeyError::Failed(format!("hotkey.key {:?} hat kein X11-Keysym", spec.key))
            })?;
            let manager = GlobalHotKeyManager::new()
                .map_err(|e| HotkeyError::Failed(format!("global-hotkey: {e}")))?;
            Ok(Self {
                manager: Some(manager),
                hotkey: HotKey::new(global_modifiers(&spec.modifiers), code),
                keysym,
                debounce: Debounce::default(),
                registered: false,
            })
        }
    }

    impl HotkeyBackend for GlobalHotkeyBackend {
        fn register(&mut self) -> Result<(), HotkeyError> {
            if self.registered {
                return Ok(());
            }
            // Nach einer Pause gibt es keinen Manager mehr — dann einen neuen.
            if self.manager.is_none() {
                self.manager = Some(
                    GlobalHotKeyManager::new()
                        .map_err(|e| HotkeyError::Failed(format!("global-hotkey: {e}")))?,
                );
            }
            let manager = self
                .manager
                .as_ref()
                .expect("Manager wurde gerade angelegt");
            manager
                .register(self.hotkey)
                .map_err(|e| HotkeyError::Failed(format!("global-hotkey register: {e}")))?;
            // Probe-Roundtrip: hält jemand die Taste (Access), ist der
            // X11-Thread lebendig.
            if !probe_grabbed(self.keysym) {
                let _ = manager.unregister(self.hotkey);
                self.manager = None;
                return Err(HotkeyError::Failed(
                    "global-hotkey: X11-Thread tot nach Register".into(),
                ));
            }
            // Alles, was der alte Manager noch in den globalen Kanal gelegt hat,
            // gehört nicht mehr zu diesem Grab.
            drain_pending_events();
            self.debounce = Debounce::default();
            self.registered = true;
            Ok(())
        }

        fn unregister(&mut self) -> Result<(), HotkeyError> {
            if !self.registered {
                return Ok(());
            }
            if let Some(manager) = self.manager.take() {
                // Best effort; entscheidend ist das Fallenlassen danach.
                let _ = manager.unregister(self.hotkey);
                drop(manager);
            }
            drain_pending_events();
            self.registered = false;
            self.debounce = Debounce::default();
            Ok(())
        }

        fn is_registered(&self) -> bool {
            self.registered
        }

        fn poll(&mut self) -> Result<Option<HotkeyEvent>, HotkeyError> {
            let receiver = GlobalHotKeyEvent::receiver();
            loop {
                match receiver.try_recv() {
                    Ok(ev) if ev.id == self.hotkey.id() => match ev.state {
                        HotKeyState::Pressed => {
                            if let Some(event) = self.debounce.on_press() {
                                return Ok(Some(event));
                            }
                        }
                        HotKeyState::Released => {
                            if let Some(event) = self.debounce.on_release() {
                                return Ok(Some(event));
                            }
                        }
                    },
                    Ok(_) => {}
                    Err(_) => return Ok(None),
                }
            }
        }

        fn backend_name(&self) -> &'static str {
            "global-hotkey"
        }
    }

    impl Drop for GlobalHotkeyBackend {
        fn drop(&mut self) {
            if let (true, Some(manager)) = (self.registered, self.manager.as_ref()) {
                let _ = manager.unregister(self.hotkey);
            }
        }
    }

    enum GrabCmd {
        /// §4.4: Grab wieder aufnehmen (Pause aufgehoben).
        Grab,
        /// §4.4: Grab freigeben — die Taste gehört wieder der fokussierten App.
        Ungrab,
        Shutdown,
    }

    pub struct X11GrabKeyBackend {
        cmd_tx: Option<Sender<GrabCmd>>,
        events: Receiver<HotkeyEvent>,
        debounce: Debounce,
        registered: bool,
        join: Option<thread::JoinHandle<()>>,
    }

    impl X11GrabKeyBackend {
        pub fn try_new(spec: &HotkeySpec) -> Result<Self, HotkeyError> {
            let keysym = x11_keysym(&spec.key).ok_or_else(|| {
                HotkeyError::Failed(format!("hotkey.key {:?} hat kein X11-Keysym", spec.key))
            })?;
            let mods = x11_mod_mask(&spec.modifiers);
            let (event_tx, events) = mpsc::channel();
            let (cmd_tx, cmd_rx) = mpsc::channel();
            let (ready_tx, ready_rx) = mpsc::channel();
            let join = thread::Builder::new()
                .name("diktier-xgrab".into())
                .spawn(move || grab_thread(keysym, mods, event_tx, cmd_rx, ready_tx))
                .map_err(|e| HotkeyError::Failed(format!("XGrabKey-Thread: {e}")))?;
            match ready_rx.recv_timeout(Duration::from_secs(2)) {
                Ok(Ok(())) => Ok(Self {
                    cmd_tx: Some(cmd_tx),
                    events,
                    debounce: Debounce::default(),
                    registered: true,
                    join: Some(join),
                }),
                Ok(Err(msg)) => {
                    let _ = cmd_tx.send(GrabCmd::Shutdown);
                    join_timeout(join, Duration::from_secs(2));
                    Err(HotkeyError::Failed(msg))
                }
                Err(_) => {
                    let _ = cmd_tx.send(GrabCmd::Shutdown);
                    join_timeout(join, Duration::from_secs(2));
                    Err(HotkeyError::Failed(
                        "XGrabKey-Thread antwortet nicht".into(),
                    ))
                }
            }
        }

        fn command(&self, cmd: GrabCmd) -> Result<(), HotkeyError> {
            match &self.cmd_tx {
                Some(tx) => tx
                    .send(cmd)
                    .map_err(|_| HotkeyError::Failed("XGrabKey-Thread beendet".into())),
                None => Err(HotkeyError::Failed("XGrabKey-Thread beendet".into())),
            }
        }
    }

    impl HotkeyBackend for X11GrabKeyBackend {
        fn register(&mut self) -> Result<(), HotkeyError> {
            if self.registered {
                return Ok(());
            }
            self.command(GrabCmd::Grab)?;
            self.registered = true;
            Ok(())
        }

        fn unregister(&mut self) -> Result<(), HotkeyError> {
            if !self.registered {
                return Ok(());
            }
            self.command(GrabCmd::Ungrab)?;
            self.registered = false;
            self.debounce = Debounce::default();
            Ok(())
        }

        fn is_registered(&self) -> bool {
            self.registered
        }

        fn poll(&mut self) -> Result<Option<HotkeyEvent>, HotkeyError> {
            loop {
                match self.events.try_recv() {
                    Ok(HotkeyEvent::Press) => {
                        if let Some(event) = self.debounce.on_press() {
                            return Ok(Some(event));
                        }
                    }
                    Ok(HotkeyEvent::Release) => {
                        if let Some(event) = self.debounce.on_release() {
                            return Ok(Some(event));
                        }
                    }
                    Err(TryRecvError::Empty) => return Ok(None),
                    Err(TryRecvError::Disconnected) => {
                        return Err(HotkeyError::Failed("XGrabKey-Thread beendet".into()));
                    }
                }
            }
        }

        fn backend_name(&self) -> &'static str {
            "x11rb-XGrabKey"
        }
    }

    impl Drop for X11GrabKeyBackend {
        fn drop(&mut self) {
            if let Some(tx) = self.cmd_tx.take() {
                let _ = tx.send(GrabCmd::Shutdown);
            }
            if let Some(join) = self.join.take() {
                join_timeout(join, Duration::from_secs(2));
            }
        }
    }

    fn join_timeout(join: thread::JoinHandle<()>, timeout: Duration) {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = join.join();
            let _ = tx.send(());
        });
        let _ = rx.recv_timeout(timeout);
    }

    fn grab_thread(
        keysym: u32,
        mods: ModMask,
        tx: Sender<HotkeyEvent>,
        cmd_rx: Receiver<GrabCmd>,
        ready: Sender<Result<(), String>>,
    ) {
        if let Err(err) = grab_thread_inner(keysym, mods, tx, cmd_rx, &ready) {
            let _ = ready.send(Err(err));
        }
    }

    fn grab_thread_inner(
        keysym: u32,
        mods: ModMask,
        tx: Sender<HotkeyEvent>,
        cmd_rx: Receiver<GrabCmd>,
        ready: &Sender<Result<(), String>>,
    ) -> Result<(), String> {
        let (conn, screen) =
            RustConnection::connect(None).map_err(|e| format!("X11-Connect: {e}"))?;
        XkbExt::xkb_use_extension(&conn, 1, 0)
            .map_err(|e| format!("xkb: {e}"))?
            .reply()
            .map_err(|e| format!("xkb: {e}"))?;
        let mut filter_repeats = true;
        if let Ok(cookie) = XkbExt::xkb_per_client_flags(
            &conn,
            xkb::ID::USE_CORE_KBD.into(),
            xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT,
            xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT,
            Default::default(),
            Default::default(),
            Default::default(),
        ) && let Ok(reply) = cookie.reply()
        {
            filter_repeats = !reply
                .value
                .contains(xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT);
        }

        let root = conn.setup().roots[screen].root;
        let keycode = keysym_to_keycode(&conn, keysym)?;
        grab_all_lock_combos(&conn, root, keycode, mods)?;
        conn.flush().map_err(|e| e.to_string())?;
        let _ = ready.send(Ok(()));
        let mut grabbed = true;

        let mut pressed = false;
        let mut pending_release: Option<u32> = None;
        loop {
            match cmd_rx.try_recv() {
                Ok(GrabCmd::Shutdown) => break,
                // §4.4: Pause gibt die Taste wirklich frei — ab hier sieht die
                // fokussierte Anwendung sie wieder (codex H3).
                Ok(GrabCmd::Ungrab) if grabbed => {
                    ungrab_all_lock_combos(&conn, root, keycode, mods);
                    let _ = conn.flush();
                    grabbed = false;
                    pressed = false;
                    pending_release = None;
                }
                Ok(GrabCmd::Grab) if !grabbed => {
                    if let Err(err) = grab_all_lock_combos(&conn, root, keycode, mods) {
                        drop(tx);
                        return Err(err);
                    }
                    let _ = conn.flush();
                    grabbed = true;
                }
                Ok(_) => {}
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => break,
            }
            loop {
                match conn.poll_for_event() {
                    Err(err) => {
                        drop(tx);
                        return Err(err.to_string());
                    }
                    Ok(None) => break,
                    Ok(Some(event)) => match event {
                        Event::KeyPress(ev) if ev.detail == keycode => {
                            if filter_repeats {
                                if pending_release == Some(ev.time) {
                                    pending_release = None;
                                    continue;
                                }
                                if let Some(_) = pending_release.take()
                                    && pressed
                                {
                                    pressed = false;
                                    if tx.send(HotkeyEvent::Release).is_err() {
                                        return Ok(());
                                    }
                                }
                            }
                            if !pressed {
                                pressed = true;
                                if tx.send(HotkeyEvent::Press).is_err() {
                                    return Ok(());
                                }
                            }
                        }
                        Event::KeyRelease(ev) if ev.detail == keycode => {
                            if filter_repeats {
                                pending_release = Some(ev.time);
                            } else if pressed {
                                pressed = false;
                                if tx.send(HotkeyEvent::Release).is_err() {
                                    return Ok(());
                                }
                            }
                        }
                        _ => {}
                    },
                }
            }
            if let Some(_) = pending_release.take()
                && pressed
            {
                pressed = false;
                if tx.send(HotkeyEvent::Release).is_err() {
                    return Ok(());
                }
            }
            thread::sleep(Duration::from_millis(10));
        }

        if grabbed {
            ungrab_all_lock_combos(&conn, root, keycode, mods);
        }
        let _ = conn.flush();
        Ok(())
    }

    /// NumLock und CapsLock dürfen den Grab nicht aushebeln — deshalb jede
    /// Kombination zusätzlich zur konfigurierten Modifiermaske.
    fn lock_variants(base: ModMask) -> [ModMask; 4] {
        [
            base,
            base | ModMask::M2,
            base | ModMask::LOCK,
            base | ModMask::M2 | ModMask::LOCK,
        ]
    }

    fn grab_all_lock_combos(
        conn: &RustConnection,
        root: Window,
        keycode: Keycode,
        base: ModMask,
    ) -> Result<(), String> {
        for mods in lock_variants(base) {
            conn.grab_key(false, root, mods, keycode, GrabMode::ASYNC, GrabMode::ASYNC)
                .map_err(|e| e.to_string())?
                .check()
                .map_err(|e| format!("XGrabKey: {e}"))?;
        }
        Ok(())
    }

    fn ungrab_all_lock_combos(
        conn: &RustConnection,
        root: Window,
        keycode: Keycode,
        base: ModMask,
    ) {
        for mods in lock_variants(base) {
            let _ = conn.ungrab_key(keycode, root, mods);
        }
    }

    fn keysym_to_keycode(conn: &RustConnection, keysym: u32) -> Result<Keycode, String> {
        let setup = conn.setup();
        let min = setup.min_keycode;
        let max = setup.max_keycode;
        let mapping = conn
            .get_keyboard_mapping(min, max - min + 1)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        let per = usize::from(mapping.keysyms_per_keycode);
        if per == 0 {
            return Err("leeres Keyboard-Mapping".into());
        }
        for (i, chunk) in mapping.keysyms.chunks(per).enumerate() {
            if chunk.contains(&keysym) {
                return Ok(min + i as u8);
            }
        }
        Err(format!("kein Keycode für Keysym {keysym:#x}"))
    }

    /// Reste im globalen `global-hotkey`-Kanal verwerfen (er überlebt den
    /// Manager).
    fn drain_pending_events() {
        let receiver = GlobalHotKeyEvent::receiver();
        while receiver.try_recv().is_ok() {}
    }

    fn probe_grabbed(keysym: u32) -> bool {
        let Ok((conn, screen)) = RustConnection::connect(None) else {
            return false;
        };
        let Ok(keycode) = keysym_to_keycode(&conn, keysym) else {
            return false;
        };
        let root = conn.setup().roots[screen].root;
        let Ok(cookie) = conn.grab_key(
            false,
            root,
            ModMask::default(),
            keycode,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        ) else {
            return false;
        };
        match cookie.check() {
            Err(x11rb::errors::ReplyError::X11Error(err))
                if matches!(err.error_kind, x11rb::protocol::ErrorKind::Access) =>
            {
                true
            }
            Ok(()) => {
                let _ = conn.ungrab_key(keycode, root, ModMask::default());
                let _ = conn.flush();
                false
            }
            Err(_) => false,
        }
    }
}

/// Windows-Hotkey über `WH_KEYBOARD_LL` (Spec §3/§5, Plan WP2).
///
/// `RegisterHotKey` scheidet aus: es meldet kein Release und kann die Taste
/// nicht vor der Zielanwendung verstecken. Der Low-Level-Hook sieht Down und
/// Up, und ein Rückgabewert ≠ 0 schluckt das Ereignis — §4.4: „Der
/// PTT-Hotkey erreicht die fokussierte Anwendung **nie**".
///
/// Der Hook lebt auf einem **eigenen** Thread mit eigener Message-Queue
/// (§5), nie auf dem Tray-Thread: dessen `TrackPopupMenu`-Modalschleife
/// würde den Hook in `LowLevelHooksTimeout` laufen lassen und Windows
/// entfernte ihn stillschweigend.
#[cfg(windows)]
mod windows {
    use std::cell::RefCell;
    use std::panic::{self, AssertUnwindSafe};
    use std::ptr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{GetLastError, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, HHOOK, KBDLLHOOKSTRUCT, LLKHF_INJECTED,
        LLKHF_LOWER_IL_INJECTED, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW,
        SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_APP, WM_KEYDOWN, WM_KEYUP,
        WM_SYSKEYDOWN, WM_SYSKEYUP, WM_USER,
    };

    use super::{Debounce, HotkeyBackend, HotkeyError, HotkeyEvent, HotkeySpec};
    use crate::config::Modifier;

    /// Command-Nachrichten an den Hook-Thread. Der `WM_APP`-Bereich gehört der
    /// Anwendung, und diese Thread-Queue gehört ausschließlich uns.
    const MSG_INSTALL: u32 = WM_APP + 1;
    const MSG_REMOVE: u32 = WM_APP + 2;
    const MSG_STOP: u32 = WM_APP + 3;

    /// Handshake und Join: kurz genug, dass ein hängender Hook-Thread den
    /// Daemon nicht festhält, lang genug für einen belasteten Rechner.
    const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);

    /// Nachfrist nach einem Ready-Timeout: Kommt das `Ready::Up` doch noch,
    /// trägt es die Thread-ID — dann lässt sich `MSG_STOP` gezielt posten,
    /// statt sich allein auf das Cancel-Flag zu verlassen.
    const CANCEL_GRACE: Duration = Duration::from_millis(250);

    /// Ein Command wurde nicht bestätigt. Danach ist das Backend irreversibel
    /// defekt (siehe [`WinHookBackend::mark_broken`]).
    const NO_ANSWER: &str = "Hook-Thread antwortet nicht";

    // ------------------------------------------------------------ VK-Mapping

    /// Config-Schlüssel → Windows-Virtual-Key (§8-Tabelle), das Gegenstück zu
    /// [`super::x11_keysym`].
    ///
    /// Buchstaben liegen auf dem **Großbuchstaben** — so führt Windows den VK
    /// (`VK_A == 'A'`); Shift ist ein Modifier, kein anderer Code. Genau
    /// umgekehrt zu X11, wo der Keysym der Kleinbuchstabe ist.
    pub fn virtual_key(key: &str) -> Option<u16> {
        if let Some(rest) = key.strip_prefix('F')
            && let Ok(n) = rest.parse::<u8>()
            && (1..=24).contains(&n)
        {
            // VK_F1 = 0x70, danach fortlaufend bis VK_F24 = 0x87.
            return Some(0x70 + u16::from(n) - 1);
        }
        if key.len() == 1 {
            let c = key.chars().next()?;
            if c.is_ascii_alphabetic() {
                return u16::try_from(u32::from(c.to_ascii_uppercase())).ok();
            }
            if c.is_ascii_digit() {
                // VK_0..VK_9 sind die ASCII-Ziffern.
                return u16::try_from(u32::from(c)).ok();
            }
        }
        Some(match key {
            "Space" => 0x20,     // VK_SPACE
            "Tab" => 0x09,       // VK_TAB
            "Enter" => 0x0d,     // VK_RETURN
            "Escape" => 0x1b,    // VK_ESCAPE
            "Backspace" => 0x08, // VK_BACK
            "Insert" => 0x2d,    // VK_INSERT
            "Delete" => 0x2e,    // VK_DELETE
            "Home" => 0x24,      // VK_HOME
            "End" => 0x23,       // VK_END
            "PageUp" => 0x21,    // VK_PRIOR
            "PageDown" => 0x22,  // VK_NEXT
            "Left" => 0x25,      // VK_LEFT
            "Up" => 0x26,        // VK_UP
            "Right" => 0x27,     // VK_RIGHT
            "Down" => 0x28,      // VK_DOWN
            _ => return None,
        })
    }

    // -------------------------------------------------------------- Modifier

    /// Modifier-Zustand, wie ihn der Hook sieht.
    ///
    /// Linke und rechte Taste sind zusammengefasst (`VK_CONTROL`/`VK_SHIFT`/
    /// `VK_MENU` melden beide Seiten, `VK_LWIN`/`VK_RWIN` werden verodert).
    /// Lock-Tasten (Caps/Num/Scroll) kommen hier gar nicht erst vor und können
    /// den Vergleich deshalb auch nicht verfälschen — das X11-Backend erreicht
    /// dasselbe über `lock_variants`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct ModifierState {
        pub ctrl: bool,
        pub shift: bool,
        pub alt: bool,
        pub win: bool,
    }

    impl ModifierState {
        /// Der **geforderte** Zustand: was in der Config steht, ist gedrückt —
        /// alles andere nicht. Der Vergleich ist damit exakt, `Shift+F9` ist
        /// bei Config `F9` kein Treffer (Sol-Review; gleiche Semantik wie
        /// `XGrabKey`, das nur die angegebene Maske greift).
        pub fn required(modifiers: &[Modifier]) -> Self {
            let mut out = Self::default();
            for modifier in modifiers {
                match modifier {
                    Modifier::Ctrl => out.ctrl = true,
                    Modifier::Shift => out.shift = true,
                    Modifier::Alt => out.alt = true,
                    Modifier::Super => out.win = true,
                }
            }
            out
        }

        /// Live-Abfrage im Hook-Callback: fünf reine Wertaufrufe, kein Lock,
        /// keine Allokation.
        ///
        /// **Bekannte Kollision (AltGr).** Windows erzeugt für AltGr auf
        /// deutschem Layout `VK_RMENU` **plus** ein synthetisches
        /// `VK_LCONTROL`. `VK_CONTROL`/`VK_MENU` fassen beide Seiten zusammen,
        /// hier erscheint AltGr deshalb als `Ctrl+Alt`. Ein konfigurierter
        /// `Ctrl+Alt+<Key>`-Hotkey kann also beim Tippen von `@`, `€`, `\` …
        /// auslösen. Bewusst nicht umgebaut (Sol-Review): AltGr sauber zu
        /// trennen hieße, die LL-Ereignisfolge mitzuführen (linkes Ctrl mit
        /// `LLKHF_INJECTED`-ähnlicher Herkunft), was für den Default `F9`
        /// (und jeden Chord ohne Ctrl+Alt) nichts bringt. Der Default ist
        /// nicht betroffen, weil zusätzliche Modifier den exakten Vergleich
        /// ohnehin verfehlen.
        fn current() -> Self {
            Self {
                ctrl: is_down(VK_CONTROL),
                shift: is_down(VK_SHIFT),
                alt: is_down(VK_MENU),
                win: is_down(VK_LWIN) || is_down(VK_RWIN),
            }
        }
    }

    /// High-Bit von `GetAsyncKeyState`: die Taste ist gerade unten.
    fn is_down(vk: u16) -> bool {
        // SAFETY: `GetAsyncKeyState` nimmt den VK als Wert, schreibt nichts,
        // hat keine Vorbedingungen und liefert für unbekannte Codes 0.
        let state = unsafe { GetAsyncKeyState(i32::from(vk)) };
        (state as u16 & 0x8000) != 0
    }

    // -------------------------------------------------------- Zustandslogik

    /// Was mit einem Tastenereignis geschieht.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Decision {
        /// Nicht unseres: `CallNextHookEx`, die Zielanwendung bekommt es.
        Pass,
        /// Unseres, aber kein neues logisches Event (Auto-Repeat): `return 1`.
        Swallow,
        /// Unseres: `return 1` **und** melden.
        Emit(HotkeyEvent),
    }

    /// Ein Tastenereignis ohne Win32-Typen — damit bleibt die Zustandslogik
    /// ohne einen einzigen API-Aufruf testbar.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct KeyEvent {
        pub vk: u16,
        pub down: bool,
        /// `LLKHF_INJECTED` oder `LLKHF_LOWER_IL_INJECTED` gesetzt.
        pub injected: bool,
    }

    /// Der thread-lokale Zustand des Hooks.
    pub struct HookState {
        vk: u16,
        required: ModifierState,
        /// Das Down zu diesem Hotkey wurde akzeptiert **und geschluckt**. Dann
        /// muss auch das Up geschluckt werden, unabhängig davon, ob die
        /// Modifier inzwischen losgelassen wurden — sonst sieht die
        /// Zielanwendung ein Up ohne Down (Sol-Review).
        accepted_down: bool,
    }

    impl HookState {
        pub fn new(vk: u16, required: ModifierState) -> Self {
            Self {
                vk,
                required,
                accepted_down: false,
            }
        }

        /// §4.4/§5.2: Beim Freigeben (Pause) fällt der Zustand zurück. Ein
        /// gerade gehaltener Key wird danach nicht mehr geschluckt — genau die
        /// Semantik, die die Pause auch sonst hat.
        pub fn reset(&mut self) {
            self.accepted_down = false;
        }

        /// `live` wird nur ausgewertet, wenn es wirklich auf die Modifier
        /// ankommt — der Callback liegt im globalen Eingabepfad, jeder
        /// gesparte Aufruf zählt gegen `LowLevelHooksTimeout`.
        pub fn on_event(
            &mut self,
            event: KeyEvent,
            live: impl FnOnce() -> ModifierState,
        ) -> Decision {
            // Injizierte Events (unser eigenes `SendInput` aus dem Paste-Pfad)
            // dürfen nie als Hotkey gelten und müssen trotzdem weiterlaufen.
            if event.injected || event.vk != self.vk {
                return Decision::Pass;
            }
            if event.down {
                if self.accepted_down {
                    // Auto-Repeat: §4.4 verlangt genau ein logisches Press.
                    return Decision::Swallow;
                }
                if live() == self.required {
                    self.accepted_down = true;
                    return Decision::Emit(HotkeyEvent::Press);
                }
                // Falscher Chord (z. B. Shift+F9 bei Config `F9`) — die Taste
                // gehört der Zielanwendung.
                Decision::Pass
            } else if self.accepted_down {
                self.accepted_down = false;
                Decision::Emit(HotkeyEvent::Release)
            } else {
                Decision::Pass
            }
        }
    }

    // ------------------------------------------------------- Hook-Callback

    struct HookContext {
        state: HookState,
        /// **Bewusst akzeptiertes Risiko** (Sol-Review): `mpsc::Sender::send`
        /// ist der einzige potenziell langsame Schritt im globalen
        /// Eingabepfad — nicht blockierend, aber weder lock- noch
        /// allokationsfrei garantiert. Er läuft nur bei einem echten
        /// Press/Release des Hotkeys (höchstens zwei pro Diktat), nicht bei
        /// jedem Tastendruck des Systems; ein `sync_channel` + `try_send`
        /// bliebe als Folgeschritt, falls `LowLevelHooksTimeout` je zuschlägt.
        events: Sender<HotkeyEvent>,
    }

    thread_local! {
        /// Nur der Hook-Thread liest und schreibt das: Windows ruft einen
        /// Low-Level-Hook auf genau dem Thread auf, der ihn installiert hat,
        /// und zwar während dieser Nachrichten pumpt. Der Callback braucht
        /// deshalb keinen Mutex und blockiert nie.
        static HOOK: RefCell<Option<HookContext>> = const { RefCell::new(None) };
    }

    unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code < 0 {
            // Dokumentierte Pflicht: unter 0 nichts auswerten, nur weiterreichen.
            // SAFETY: unveränderte Parameter weiterreichen; `NULL` als `hhk`
            // ist der für `WH_KEYBOARD_LL` dokumentierte Wert (Windows
            // ignoriert das Handle und nimmt den nächsten Hook der Kette).
            return unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) };
        }
        // `WH_KEYBOARD_LL` kennt genau diese vier Nachrichten. Alles andere
        // wird unverändert weitergereicht, statt es als Up zu deuten — sonst
        // beendete ein unerwarteter Wert ein akzeptiertes `accepted_down`
        // (Sol-Review).
        let down = match wparam as u32 {
            WM_KEYDOWN | WM_SYSKEYDOWN => true,
            WM_KEYUP | WM_SYSKEYUP => false,
            _ => {
                // SAFETY: wie oben — unveränderte Parameter an die Kette.
                return unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) };
            }
        };
        // SAFETY: Für `nCode >= 0` (`HC_ACTION`) garantiert Windows, dass
        // `lparam` auf eine gültige, ausgerichtete `KBDLLHOOKSTRUCT` zeigt,
        // die für die Dauer des Callbacks lebt. Es wird nur gelesen und sofort
        // kopiert (`KBDLLHOOKSTRUCT` ist `Copy`).
        let raw = unsafe { *(lparam as *const KBDLLHOOKSTRUCT) };
        let event = KeyEvent {
            vk: raw.vkCode as u16,
            down,
            injected: raw.flags & (LLKHF_INJECTED | LLKHF_LOWER_IL_INJECTED) != 0,
        };

        // Ein Unwind über die Win32-Grenze ist undefiniert, und ein Panic hier
        // risse zusätzlich den Hook-Thread mit. `try_borrow_mut` allein reicht
        // dafür nicht (Sol-Review): auch `LocalKey::with` kann panicken (TLS
        // wird gerade zerstört), und `Sender::send` allokiert. Deshalb liegt
        // die **gesamte** Rust-Auswertung in `catch_unwind` und fällt im
        // Zweifel auf `Pass` zurück — die Taste gehört dann der Anwendung.
        // `AssertUnwindSafe` ist zulässig, weil der einzige geteilte Zustand
        // der thread-lokale `HookState` ist: ein halb geänderter `HookState`
        // führt höchstens zu einem verwaisten Up, nie zu Unsicherheit.
        let decision = panic::catch_unwind(AssertUnwindSafe(|| {
            HOOK.try_with(|cell| {
                // Reentranz kann es hier nicht geben (der Callback läuft in
                // `GetMessageW`, die Commands danach), der sichere Ausgang
                // kostet aber nichts.
                let Ok(mut slot) = cell.try_borrow_mut() else {
                    return Decision::Pass;
                };
                let Some(ctx) = slot.as_mut() else {
                    return Decision::Pass;
                };
                let decision = ctx.state.on_event(event, ModifierState::current);
                if let Decision::Emit(hotkey) = decision
                    && ctx.events.send(hotkey).is_err()
                {
                    // Niemand hört mehr zu — dann nicht weiter schlucken, sonst
                    // verschwindet die Taste stumm aus dem System. Im geordneten
                    // Shutdown kann das nicht passieren: der Empfänger lebt bis
                    // nach dem bestätigten Stop (siehe `Drop`).
                    ctx.state.reset();
                    return Decision::Pass;
                }
                decision
            })
            .unwrap_or(Decision::Pass)
        }))
        .unwrap_or(Decision::Pass);

        match decision {
            Decision::Pass => {
                // SAFETY: wie oben — unveränderte Parameter an die Kette.
                unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) }
            }
            // §4.4: non-zero heißt „die fokussierte Anwendung sieht die Taste nie".
            Decision::Swallow | Decision::Emit(_) => 1,
        }
    }

    // ---------------------------------------------------------- Hook-Thread

    /// Ergebnis des Startup-Handshakes.
    enum Ready {
        /// Hook steht, der Thread pumpt; `u32` ist seine Thread-ID als Ziel
        /// für `PostThreadMessageW`.
        Up(u32),
        Failed(String),
    }

    fn install_hook() -> Result<HHOOK, String> {
        // SAFETY: `GetModuleHandleW(NULL)` liefert das Modul-Handle des
        // eigenen Prozesses, nimmt einen Nullzeiger als dokumentiertes
        // Argument und überträgt kein Eigentum.
        let module = unsafe { GetModuleHandleW(ptr::null()) };
        // SAFETY: `hook_proc` hat die von `HOOKPROC` geforderte Signatur und
        // lebt so lange wie der Prozess; `dwThreadId = 0` ist hier **gewählt**
        // — es ist keine Vorbedingung der Signatur, sondern der Wert, der den
        // für uns nötigen globalen Desktop-Hook installiert (ein Thread-Filter
        // sähe die Tasten fremder Anwendungen nicht). Der Rückgabewert wird
        // sofort geprüft.
        let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), module, 0) };
        if hook.is_null() {
            return Err(last_error());
        }
        Ok(hook)
    }

    /// Entfernt den Hook und meldet **den Fehlschlag** (Sol-Review): Wird das
    /// Ergebnis ignoriert und das Handle trotzdem genullt, bleibt der Hook bei
    /// einem echten Fehler aktiv und schluckt weiter, während `unregister()`
    /// Erfolg meldet.
    fn remove_hook(hook: &mut HHOOK) -> Result<(), String> {
        if hook.is_null() {
            return Ok(());
        }
        // SAFETY: Das Handle stammt aus einem erfolgreichen
        // `SetWindowsHookExW` auf genau diesem Thread und wurde noch nicht
        // entfernt — genullt wird es nur nach einem erfolgreichen Unhook, ein
        // zweites Unhook desselben Handles ist also ausgeschlossen. Nach einem
        // Fehlschlag bleibt das Handle stehen und darf erneut versucht werden.
        let ok = unsafe { UnhookWindowsHookEx(*hook) };
        if ok == 0 {
            return Err(last_error());
        }
        *hook = ptr::null_mut();
        Ok(())
    }

    /// Letzter Versuch beim Threadende: Ein hier verbliebener Hook lebt bis
    /// zum Prozessende weiter, deshalb wird der Fehlschlag laut gemeldet und
    /// genau einmal wiederholt.
    fn remove_hook_finally(hook: &mut HHOOK) -> Result<(), String> {
        match remove_hook(hook) {
            Ok(()) => Ok(()),
            Err(first) => {
                eprintln!("diktier: Hook entfernen fehlgeschlagen ({first}) — zweiter Versuch");
                match remove_hook(hook) {
                    Ok(()) => Ok(()),
                    Err(second) => {
                        eprintln!(
                            "diktier: Hook bleibt installiert ({second}) — er endet erst mit dem Prozess"
                        );
                        Err(second)
                    }
                }
            }
        }
    }

    /// §4.4: Der Tooltip nennt den nackten Win32-Code — ein Low-Level-Hook hat
    /// keinen „Konflikt" wie `RegisterHotKey`, und eine erfundene Erklärung
    /// wäre irreführend (Sol-Review).
    fn last_error() -> String {
        // SAFETY: parameterlos, liest nur den Fehlercode dieses Threads.
        let code = unsafe { GetLastError() };
        format!("Win32-Fehler {code}")
    }

    fn with_state(f: impl FnOnce(&mut HookState)) {
        HOOK.with(|cell| {
            if let Ok(mut slot) = cell.try_borrow_mut()
                && let Some(ctx) = slot.as_mut()
            {
                f(&mut ctx.state);
            }
        });
    }

    /// Alles, was der Hook-Thread von außen bekommt.
    struct ThreadPorts {
        events: Sender<HotkeyEvent>,
        ready: Sender<Ready>,
        acks: Sender<Result<(), String>>,
        /// `GetMessageW == -1`: der gesicherte Win32-Fehler, damit `poll()`
        /// ihn nennen kann statt nur „Hook-Thread beendet" (Sol-Review).
        status: Sender<String>,
        /// Der Ready-Handshake ist abgelaufen — das Backend gibt es nicht mehr
        /// (oder gleich nicht). Wer das Flag gesetzt sieht, installiert nichts
        /// mehr, entfernt einen schon installierten Hook und endet. Ohne das
        /// überlebte ein knapp zu spät fertiger Thread samt globalem Hook den
        /// fehlgeschlagenen Start (Sol-Review).
        cancel: Arc<AtomicBool>,
    }

    impl ThreadPorts {
        fn cancelled(&self) -> bool {
            self.cancel.load(Ordering::Acquire)
        }
    }

    fn hook_thread(vk: u16, required: ModifierState, ports: ThreadPorts) {
        // `PostThreadMessageW` scheitert, solange der Thread noch keine
        // Nachrichtenqueue hat. Ein `PeekMessageW` erzwingt sie **vor** dem
        // Handshake — danach ist jedes Post gültig (Sol-Review).
        let mut msg = MSG::default();
        // SAFETY: `msg` ist ein vollständig initialisiertes `MSG`;
        // `hwnd = NULL` meint „alle Fenster dieses Threads", `PM_NOREMOVE`
        // entnimmt nichts, und der Filter `WM_USER..=WM_USER` fasst nichts an,
        // was sonst gebraucht würde.
        unsafe { PeekMessageW(&mut msg, ptr::null_mut(), WM_USER, WM_USER, PM_NOREMOVE) };
        // SAFETY: parameterlos, liefert die ID des aufrufenden Threads.
        let thread_id = unsafe { GetCurrentThreadId() };

        // 1. Cancel-Prüfung: Wer hier schon zu spät ist, installiert gar
        // nichts erst.
        if ports.cancelled() {
            return;
        }

        HOOK.with(|cell| {
            *cell.borrow_mut() = Some(HookContext {
                state: HookState::new(vk, required),
                events: ports.events.clone(),
            });
        });

        let mut hook = match install_hook() {
            Ok(hook) => hook,
            Err(err) => {
                let _ = ports.ready.send(Ready::Failed(err));
                clear_context();
                return;
            }
        };
        // 2. Cancel-Prüfung: Der Hook steht, aber niemand wartet mehr darauf —
        // dann muss er sofort wieder weg.
        if ports.cancelled() || ports.ready.send(Ready::Up(thread_id)).is_err() {
            let _ = remove_hook_finally(&mut hook);
            clear_context();
            return;
        }
        // 3. Cancel-Prüfung: Zwischen `send` und dem blockierenden
        // `GetMessageW` kann das Backend aufgegeben haben. Danach hält nur noch
        // das gezielte `MSG_STOP` aus der Nachfrist (`CANCEL_GRACE`), für das
        // das Backend die eben gesendete Thread-ID braucht.
        if ports.cancelled() {
            let _ = remove_hook_finally(&mut hook);
            clear_context();
            return;
        }

        loop {
            // SAFETY: `msg` ist gültig; `hwnd = NULL` mit Filter 0/0 heißt
            // „alle Nachrichten dieses Threads", also auch die geposteten
            // Commands. Der Rückgabewert wird auf -1 und 0 geprüft.
            let rc = unsafe { GetMessageW(&mut msg, ptr::null_mut(), 0, 0) };
            if rc < 0 {
                // -1 = Fehler: der Thread kann nicht weiterpumpen, also fällt
                // der Hook aus. Den Code **sofort** sichern (jeder weitere
                // Win32-Aufruf überschriebe ihn) und ans Backend melden, damit
                // `poll()` ihn nennen kann statt nur „Thread beendet".
                let err = last_error();
                let _ = ports.status.send(format!("GetMessageW: {err}"));
                break;
            }
            if rc == 0 {
                // `WM_QUIT` — geordnetes Ende, kein Fehler.
                break;
            }
            match msg.message {
                MSG_INSTALL => {
                    let result = if hook.is_null() {
                        install_hook().map(|new| hook = new)
                    } else {
                        Ok(())
                    };
                    if result.is_ok() {
                        with_state(HookState::reset);
                    }
                    let _ = ports.acks.send(result);
                }
                MSG_REMOVE => {
                    // §4.4: State und `registered` dürfen erst nach einem
                    // **bestätigten** Unhook fallen — sonst meldete
                    // `unregister()` Erfolg, während der Hook weiterschluckt.
                    let result = remove_hook(&mut hook);
                    if result.is_ok() {
                        with_state(HookState::reset);
                    }
                    let _ = ports.acks.send(result);
                }
                MSG_STOP => break,
                // Der Thread besitzt kein Fenster; alles andere ist irrelevant
                // und braucht weder `TranslateMessage` noch `DispatchMessageW`.
                _ => {}
            }
        }

        // Der Ack zum Stop kommt erst, wenn der Hook wirklich weg ist: `Drop`
        // wartet darauf, bevor er Sender und Empfänger fallen lässt.
        let result = remove_hook_finally(&mut hook);
        clear_context();
        let _ = ports.acks.send(result);
    }

    fn clear_context() {
        HOOK.with(|cell| {
            if let Ok(mut slot) = cell.try_borrow_mut() {
                *slot = None;
            }
        });
    }

    /// Join, der nie länger als `timeout` hängt (wie im Linux-Modul: ein
    /// hängender Owner-Thread darf den Shutdown nicht blockieren).
    ///
    /// `Builder::spawn` statt `thread::spawn`: Letzteres panickt, wenn das
    /// System keinen Thread mehr hergibt — ausgerechnet im Aufräumpfad
    /// (Sol-Review). Timeout und Spawnfehler werden gemeldet, statt still zu
    /// bleiben; der Hook-Thread lebt dann bis zum Prozessende weiter.
    fn join_timeout(join: JoinHandle<()>, timeout: Duration) {
        let (tx, rx) = mpsc::channel();
        let waiter = thread::Builder::new()
            .name("diktier-hook-join".into())
            .spawn(move || {
                let _ = join.join();
                let _ = tx.send(());
            });
        match waiter {
            Ok(_) => {
                if rx.recv_timeout(timeout).is_err() {
                    eprintln!(
                        "diktier: Hook-Thread endet nicht innerhalb von {} s — aufgegeben",
                        timeout.as_secs_f32()
                    );
                }
            }
            Err(err) => eprintln!("diktier: Join-Thread für den Hook nicht startbar: {err}"),
        }
    }

    // ------------------------------------------------------------- Backend

    /// `HotkeyBackend` über einen persistenten Hook-Thread.
    ///
    /// `register`/`unregister` installieren und entfernen nur den Hook; der
    /// Thread bleibt stehen und wartet auf den nächsten Command. Beide sind
    /// idempotent — der Daemon schickt `HotkeyCmd::{Grab,Ungrab}` bei jedem
    /// Pause-Wechsel, auch mehrfach hintereinander.
    pub struct WinHookBackend {
        chord: String,
        thread_id: u32,
        events: Receiver<HotkeyEvent>,
        acks: Receiver<Result<(), String>>,
        status: Receiver<String>,
        cancel: Arc<AtomicBool>,
        join: Option<JoinHandle<()>>,
        debounce: Debounce,
        registered: bool,
        /// Ein Command blieb unbestätigt. Danach ist **nicht mehr feststellbar**,
        /// zu welchem Command ein später eintreffender Ack gehört — ein
        /// verspäteter Remove-Ack könnte einen noch gar nicht ausgeführten
        /// Install bestätigen (Sol-Review). Deshalb ist das Backend ab dann
        /// irreversibel defekt: Stop ist gepostet, weitere Commands gibt es
        /// nicht, und `register`/`unregister`/`poll` melden den Fehler, damit
        /// §10 greift (Hotkey aus, Tray-Click bleibt).
        broken: bool,
    }

    impl WinHookBackend {
        pub fn try_new(spec: &HotkeySpec) -> Result<Self, HotkeyError> {
            let chord = spec.describe();
            let vk = virtual_key(&spec.key).ok_or_else(|| {
                failed_with(
                    &chord,
                    &format!("hotkey.key {:?} hat keinen Virtual-Key-Code", spec.key),
                )
            })?;
            let required = ModifierState::required(&spec.modifiers);
            let (event_tx, events) = mpsc::channel();
            let (ack_tx, acks) = mpsc::channel();
            let (status_tx, status) = mpsc::channel();
            let (ready_tx, ready_rx) = mpsc::channel();
            let cancel = Arc::new(AtomicBool::new(false));
            let ports = ThreadPorts {
                events: event_tx,
                ready: ready_tx,
                acks: ack_tx,
                status: status_tx,
                cancel: cancel.clone(),
            };
            let join = thread::Builder::new()
                .name("diktier-hook".into())
                .spawn(move || hook_thread(vk, required, ports))
                .map_err(|e| failed_with(&chord, &format!("Hook-Thread: {e}")))?;

            match ready_rx.recv_timeout(HANDSHAKE_TIMEOUT) {
                Ok(Ready::Up(thread_id)) => Ok(Self {
                    chord,
                    thread_id,
                    events,
                    acks,
                    status,
                    cancel,
                    join: Some(join),
                    debounce: Debounce::default(),
                    registered: true,
                    broken: false,
                }),
                Ok(Ready::Failed(err)) => {
                    cancel.store(true, Ordering::Release);
                    drop(ready_rx);
                    join_timeout(join, HANDSHAKE_TIMEOUT);
                    Err(failed_with(&chord, &err))
                }
                Err(_) => {
                    // Der Thread darf ab jetzt nichts mehr installieren und
                    // muss einen schon installierten Hook wieder entfernen —
                    // sonst überlebte ein globaler Hook den fehlgeschlagenen
                    // Start, und ein zweiter Versuch installierte einen
                    // weiteren (Sol-Review).
                    cancel.store(true, Ordering::Release);
                    // Nachfrist: Kommt das Ready knapp zu spät, trägt es die
                    // Thread-ID — dann beendet ein gezieltes `MSG_STOP` auch
                    // den Thread, der schon in `GetMessageW` steht.
                    if let Ok(Ready::Up(thread_id)) = ready_rx.recv_timeout(CANCEL_GRACE) {
                        // SAFETY: wie in `command` — reines Posten von Werten
                        // an die Queue, die dieser Thread vor dem Handshake
                        // erzwungen hat.
                        unsafe { PostThreadMessageW(thread_id, MSG_STOP, 0, 0) };
                    }
                    drop(ready_rx);
                    join_timeout(join, HANDSHAKE_TIMEOUT);
                    Err(failed_with(&chord, NO_ANSWER))
                }
            }
        }

        /// Command an den Hook-Thread und auf seine Bestätigung warten. Die
        /// Queue existiert seit dem Handshake, also kann das Post nur
        /// scheitern, wenn der Thread tot ist.
        fn command(&mut self, message: u32) -> Result<(), HotkeyError> {
            if self.broken {
                return Err(self.failed(NO_ANSWER));
            }
            // Alte Bestätigungen dürfen die nächste Antwort nicht vortäuschen.
            // Nach einem Timeout reicht das Drainen nicht (der alte Ack kann
            // erst danach eintreffen) — dann ist das Backend defekt und kommt
            // gar nicht mehr hierher.
            while self.acks.try_recv().is_ok() {}
            // SAFETY: `thread_id` stammt aus dem Handshake genau dieses
            // Threads; `PostThreadMessageW` kopiert nur Werte, `wparam` und
            // `lparam` sind 0. Der Rückgabewert wird geprüft.
            let posted = unsafe { PostThreadMessageW(self.thread_id, message, 0, 0) };
            if posted == 0 {
                let err = last_error();
                self.mark_broken();
                return Err(self.failed(&err));
            }
            match self.acks.recv_timeout(HANDSHAKE_TIMEOUT) {
                Ok(Ok(())) => Ok(()),
                Ok(Err(err)) => Err(self.failed(&err)),
                Err(_) => {
                    self.mark_broken();
                    Err(self.failed(NO_ANSWER))
                }
            }
        }

        /// Ab hier ist die Zuordnung Command → Ack verloren. Stop posten,
        /// Cancel setzen und nichts mehr an diesen Thread schicken.
        fn mark_broken(&mut self) {
            if self.broken {
                return;
            }
            self.broken = true;
            // §10: Der Hotkey gilt als tot, auch wenn der Hook womöglich noch
            // steht — der Daemon soll den Fehlerzustand sehen.
            self.registered = false;
            self.cancel.store(true, Ordering::Release);
            // SAFETY: wie oben; ein toter Thread liefert 0, was hier nichts
            // mehr ändert.
            unsafe { PostThreadMessageW(self.thread_id, MSG_STOP, 0, 0) };
        }

        fn failed(&self, reason: &str) -> HotkeyError {
            failed_with(&self.chord, reason)
        }

        /// Alles, was vor einem Zustandswechsel noch im Kanal lag, gehört nicht
        /// mehr dazu.
        fn drain_events(&self) {
            while self.events.try_recv().is_ok() {}
        }
    }

    fn failed_with(chord: &str, reason: &str) -> HotkeyError {
        HotkeyError::Failed(format!("Hotkey nicht verfügbar ({chord}): {reason}"))
    }

    impl HotkeyBackend for WinHookBackend {
        fn register(&mut self) -> Result<(), HotkeyError> {
            if self.broken {
                return Err(self.failed(NO_ANSWER));
            }
            if self.registered {
                return Ok(());
            }
            self.command(MSG_INSTALL)?;
            self.drain_events();
            self.debounce = Debounce::default();
            self.registered = true;
            Ok(())
        }

        fn unregister(&mut self) -> Result<(), HotkeyError> {
            if self.broken {
                return Err(self.failed(NO_ANSWER));
            }
            if !self.registered {
                return Ok(());
            }
            // Scheitert das Unhook, meldet der Ack den Win32-Fehler und `?`
            // trägt ihn weiter — `registered` bleibt dann **wahr**, denn der
            // Hook schluckt weiter (Sol-Review).
            self.command(MSG_REMOVE)?;
            self.drain_events();
            self.debounce = Debounce::default();
            self.registered = false;
            Ok(())
        }

        fn is_registered(&self) -> bool {
            self.registered && !self.broken
        }

        fn poll(&mut self) -> Result<Option<HotkeyEvent>, HotkeyError> {
            if self.broken {
                return Err(self.failed(NO_ANSWER));
            }
            loop {
                match self.events.try_recv() {
                    Ok(HotkeyEvent::Press) => {
                        if let Some(event) = self.debounce.on_press() {
                            return Ok(Some(event));
                        }
                    }
                    Ok(HotkeyEvent::Release) => {
                        if let Some(event) = self.debounce.on_release() {
                            return Ok(Some(event));
                        }
                    }
                    Err(TryRecvError::Empty) => return Ok(None),
                    // Der Hook-Thread ist gestorben (`GetMessageW == -1`,
                    // Threadtod) — §10: Hotkey aus, Tray-Click bleibt.
                    //
                    // **Nicht** erfasst: ein Hook, den Windows wegen
                    // `LowLevelHooksTimeout` still aus der Kette genommen hat.
                    // Dann lebt der Thread weiter, der Kanal bleibt offen und
                    // der Hotkey ist trotzdem tot. Bekanntes Restrisiko —
                    // Watchdog bewusst zurückgestellt (siehe Plan/Notes).
                    Err(TryRecvError::Disconnected) => {
                        let reason = self
                            .status
                            .try_recv()
                            .unwrap_or_else(|_| "Hook-Thread beendet".to_string());
                        self.mark_broken();
                        return Err(self.failed(&reason));
                    }
                }
            }
        }

        fn backend_name(&self) -> &'static str {
            "win32-ll-hook"
        }
    }

    impl Drop for WinHookBackend {
        fn drop(&mut self) {
            self.cancel.store(true, Ordering::Release);
            // SAFETY: wie in `command` — reines Posten von Werten an eine
            // Thread-Queue, die seit dem Handshake existiert. Ein toter Thread
            // liefert 0, was hier nichts mehr ändert.
            let posted = unsafe { PostThreadMessageW(self.thread_id, MSG_STOP, 0, 0) };
            // Auf den **bestätigten** Unhook warten, bevor die Kanäle fallen:
            // `events`/`acks` liegen als Felder in `self` und werden erst nach
            // diesem Rumpf zerstört. So kann der Callback nicht mitten im
            // Shutdown ein Down schlucken, dessen Release dann ins Leere ginge
            // (Sol-Review).
            if posted != 0 && !self.broken {
                match self.acks.recv_timeout(HANDSHAKE_TIMEOUT) {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => {
                        eprintln!("diktier: Hook-Thread meldet beim Stop: {err}");
                    }
                    Err(_) => eprintln!("diktier: Hook-Thread bestätigt den Stop nicht"),
                }
            }
            if let Some(join) = self.join.take() {
                join_timeout(join, HANDSHAKE_TIMEOUT);
            }
        }
    }
}

/// §4.4: Backendwahl für die **konfigurierte** Taste. Erst `global-hotkey`,
/// dann der XGrabKey-Fallback; scheitert beides, bleibt der Stub (der Daemon
/// meldet das als `HotkeyRegistration`-Fehler und bleibt per Tray bedienbar).
#[cfg(target_os = "linux")]
pub fn new_backend(spec: &HotkeySpec) -> Result<AnyHotkeyBackend, HotkeyError> {
    let global = linux::GlobalHotkeyBackend::try_new(spec);
    let global_err = match global {
        Ok(mut backend) => match backend.register() {
            Ok(()) => return Ok(AnyHotkeyBackend::Global(backend)),
            Err(err) => err,
        },
        Err(err) => err,
    };
    match linux::X11GrabKeyBackend::try_new(spec) {
        Ok(backend) => Ok(AnyHotkeyBackend::Grab(backend)),
        // §4.4: „Tooltip nennt den Konflikt" — beide Ursachen bleiben sichtbar.
        Err(grab_err) => Err(HotkeyError::Failed(format!(
            "{} nicht greifbar: {global_err}; XGrabKey: {grab_err}",
            spec.describe()
        ))),
    }
}

/// §4.4/§5: Windows greift die Taste per `WH_KEYBOARD_LL`. Es gibt keinen
/// zweiten Weg (`RegisterHotKey` liefert kein Release), also auch keine
/// Fallback-Kette wie unter Linux — scheitert der Hook, meldet der Daemon
/// `HotkeyUnavailable` und bleibt per Tray-Click bedienbar (§10).
#[cfg(windows)]
pub fn new_backend(spec: &HotkeySpec) -> Result<AnyHotkeyBackend, HotkeyError> {
    windows::WinHookBackend::try_new(spec).map(AnyHotkeyBackend::WinHook)
}

/// Expliziter XGrabKey-Fallback (Spike-Auswahl, Spec §3).
#[cfg(target_os = "linux")]
#[allow(dead_code)]
pub fn new_xgrab_backend(spec: &HotkeySpec) -> Result<AnyHotkeyBackend, HotkeyError> {
    linux::X11GrabKeyBackend::try_new(spec).map(AnyHotkeyBackend::Grab)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_backend_registers_and_is_idle() {
        let mut backend = StubHotkeyBackend;
        backend.register().unwrap();
        assert_eq!(backend.poll().unwrap(), None);
    }

    #[test]
    fn debounce_collapses_auto_repeat() {
        let mut d = Debounce::default();
        assert_eq!(d.on_press(), Some(HotkeyEvent::Press));
        assert_eq!(d.on_press(), None);
        assert_eq!(d.on_press(), None);
        assert_eq!(d.on_release(), Some(HotkeyEvent::Release));
        assert_eq!(d.on_release(), None);
        assert_eq!(d.on_press(), Some(HotkeyEvent::Press));
        assert_eq!(d.on_release(), Some(HotkeyEvent::Release));
    }

    /// §4.4: Die Config bestimmt die Taste — bis in den X11-Keysym (codex H3).
    #[test]
    fn config_key_maps_to_the_exact_x11_keysym() {
        assert_eq!(x11_keysym("F9"), Some(0xffc6));
        assert_eq!(x11_keysym("F1"), Some(0xffbe));
        assert_eq!(x11_keysym("F12"), Some(0xffc9));
        assert_eq!(x11_keysym("F24"), Some(0xffd5));
        assert_eq!(
            x11_keysym("A"),
            Some(0x0061),
            "Buchstaben als Kleinbuchstabe"
        );
        assert_eq!(x11_keysym("Z"), Some(0x007a));
        assert_eq!(x11_keysym("7"), Some(0x0037));
        assert_eq!(x11_keysym("Space"), Some(0x0020));
        assert_eq!(x11_keysym("Enter"), Some(0xff0d));
        assert_eq!(x11_keysym("PageDown"), Some(0xff56));
        assert_eq!(x11_keysym("Down"), Some(0xff54));
        assert_eq!(x11_keysym("F99"), None);
        assert_eq!(x11_keysym("Grüße"), None);
    }

    #[test]
    fn spec_from_config_keeps_key_and_modifiers() {
        let config = HotkeyConfig {
            key: "F7".into(),
            modifiers: vec![Modifier::Ctrl, Modifier::Shift],
            ..HotkeyConfig::default()
        };
        let spec = HotkeySpec::from_config(&config);
        assert_eq!(spec.key, "F7");
        assert_eq!(spec.modifiers, vec![Modifier::Ctrl, Modifier::Shift]);
        assert_eq!(spec.describe(), "Ctrl+Shift+F7");
        assert_eq!(HotkeySpec::default().describe(), "F9");
    }

    /// Ein Backend-Doppel, das die Registrierung mitschreibt — damit ist der
    /// Pause-Pfad ohne X11 prüfbar.
    #[derive(Debug, Default)]
    struct FakeBackend {
        spec: HotkeySpec,
        registered: bool,
        grabs: u32,
        ungrabs: u32,
        queued: Vec<HotkeyEvent>,
        debounce: Debounce,
    }

    impl HotkeyBackend for FakeBackend {
        fn register(&mut self) -> Result<(), HotkeyError> {
            if !self.registered {
                self.registered = true;
                self.grabs += 1;
            }
            Ok(())
        }

        fn unregister(&mut self) -> Result<(), HotkeyError> {
            if self.registered {
                self.registered = false;
                self.ungrabs += 1;
                self.debounce = Debounce::default();
            }
            Ok(())
        }

        fn poll(&mut self) -> Result<Option<HotkeyEvent>, HotkeyError> {
            // Ein freigegebener Grab liefert nichts mehr — die Taste gehört
            // der fokussierten Anwendung.
            if !self.registered {
                self.queued.clear();
                return Ok(None);
            }
            while !self.queued.is_empty() {
                let raw = self.queued.remove(0);
                let mapped = match raw {
                    HotkeyEvent::Press => self.debounce.on_press(),
                    HotkeyEvent::Release => self.debounce.on_release(),
                };
                if let Some(event) = mapped {
                    return Ok(Some(event));
                }
            }
            Ok(None)
        }

        fn is_registered(&self) -> bool {
            self.registered
        }

        fn backend_name(&self) -> &'static str {
            "fake"
        }
    }

    /// §4.4/§5.2: „Hotkey pausieren" gibt den Grab frei, das Aufheben nimmt ihn
    /// zurück — und dazwischen erreicht die Taste den Daemon nicht (codex H3).
    #[test]
    fn pause_ungrabs_and_resume_regrabs() {
        let mut backend = FakeBackend {
            spec: HotkeySpec::default(),
            ..FakeBackend::default()
        };
        backend.register().unwrap();
        assert!(backend.is_registered());
        assert_eq!((backend.grabs, backend.ungrabs), (1, 0));

        backend.queued = vec![HotkeyEvent::Press, HotkeyEvent::Release];
        assert_eq!(backend.poll().unwrap(), Some(HotkeyEvent::Press));
        assert_eq!(backend.poll().unwrap(), Some(HotkeyEvent::Release));

        // Pause: Grab weg, Tastendrücke erreichen uns nicht mehr.
        backend.unregister().unwrap();
        assert!(!backend.is_registered());
        assert_eq!((backend.grabs, backend.ungrabs), (1, 1));
        backend.queued = vec![HotkeyEvent::Press, HotkeyEvent::Release];
        assert_eq!(backend.poll().unwrap(), None, "pausiert: kein Event");

        // Pause aufheben: neuer Grab, alles wieder wie vorher.
        backend.register().unwrap();
        assert!(backend.is_registered());
        assert_eq!((backend.grabs, backend.ungrabs), (2, 1));
        backend.queued = vec![HotkeyEvent::Press, HotkeyEvent::Release];
        assert_eq!(backend.poll().unwrap(), Some(HotkeyEvent::Press));
        assert_eq!(backend.poll().unwrap(), Some(HotkeyEvent::Release));
        assert_eq!(backend.spec, HotkeySpec::default());
    }

    /// Windows (Phase 5/WP2): VK-Mapping, exakter Modifier-Vergleich und die
    /// Zustandsmaschine des Hooks — alles reine Funktionen, kein Win32-Aufruf.
    #[cfg(windows)]
    mod win {
        use super::super::windows::{Decision, HookState, KeyEvent, ModifierState, virtual_key};
        use super::super::{HotkeyEvent, Modifier, x11_keysym};

        const VK_F9: u16 = 0x78;

        /// §4.4: Die Config bestimmt die Taste — bis in den Virtual-Key.
        #[test]
        fn config_key_maps_to_the_exact_virtual_key() {
            assert_eq!(virtual_key("F1"), Some(0x70));
            assert_eq!(virtual_key("F9"), Some(VK_F9));
            assert_eq!(virtual_key("F12"), Some(0x7b));
            assert_eq!(virtual_key("F24"), Some(0x87));
            assert_eq!(
                virtual_key("a"),
                Some(0x41),
                "Buchstaben liegen auf dem Großbuchstaben — anders als in X11"
            );
            assert_eq!(virtual_key("A"), Some(0x41));
            assert_eq!(virtual_key("F"), Some(0x46), "kein F-Tasten-Präfix");
            assert_eq!(virtual_key("Z"), Some(0x5a));
            assert_eq!(virtual_key("0"), Some(0x30));
            assert_eq!(virtual_key("7"), Some(0x37));
            assert_eq!(virtual_key("Space"), Some(0x20));
            assert_eq!(virtual_key("Enter"), Some(0x0d));
            assert_eq!(virtual_key("Insert"), Some(0x2d));
            assert_eq!(virtual_key("Delete"), Some(0x2e));
            assert_eq!(virtual_key("Home"), Some(0x24));
            assert_eq!(virtual_key("End"), Some(0x23));
            assert_eq!(virtual_key("PageUp"), Some(0x21));
            assert_eq!(virtual_key("PageDown"), Some(0x22));
            assert_eq!(virtual_key("Left"), Some(0x25));
            assert_eq!(virtual_key("Up"), Some(0x26));
            assert_eq!(virtual_key("Right"), Some(0x27));
            assert_eq!(virtual_key("Down"), Some(0x28));
            assert_eq!(virtual_key("F0"), None);
            assert_eq!(virtual_key("F25"), None);
            assert_eq!(virtual_key("Grüße"), None);
        }

        /// Jeder Config-Schlüssel, den X11 kennt, muss auch einen VK haben —
        /// sonst wäre dieselbe Config auf Windows unbrauchbar.
        #[test]
        fn every_x11_key_also_has_a_virtual_key() {
            for key in [
                "F1",
                "F9",
                "F24",
                "A",
                "Z",
                "0",
                "9",
                "Space",
                "Tab",
                "Enter",
                "Escape",
                "Backspace",
                "Insert",
                "Delete",
                "Home",
                "End",
                "PageUp",
                "PageDown",
                "Left",
                "Up",
                "Right",
                "Down",
            ] {
                assert!(x11_keysym(key).is_some(), "{key}: X11");
                assert!(virtual_key(key).is_some(), "{key}: VK");
            }
        }

        fn mods(ctrl: bool, shift: bool, alt: bool, win: bool) -> ModifierState {
            ModifierState {
                ctrl,
                shift,
                alt,
                win,
            }
        }

        fn down(vk: u16) -> KeyEvent {
            KeyEvent {
                vk,
                down: true,
                injected: false,
            }
        }

        fn up(vk: u16) -> KeyEvent {
            KeyEvent {
                vk,
                down: false,
                injected: false,
            }
        }

        #[test]
        fn required_modifiers_are_exact() {
            assert_eq!(ModifierState::required(&[]), ModifierState::default());
            assert_eq!(
                ModifierState::required(&[Modifier::Ctrl, Modifier::Shift]),
                mods(true, true, false, false)
            );
            assert_eq!(
                ModifierState::required(&[Modifier::Alt, Modifier::Super]),
                mods(false, false, true, true)
            );
        }

        /// Sol-Review: `Shift+F9` darf bei Config `F9` **nicht** greifen —
        /// sonst verhielte sich Windows anders als `XGrabKey`.
        #[test]
        fn extra_modifiers_are_not_a_match() {
            let mut state = HookState::new(VK_F9, ModifierState::default());
            assert_eq!(
                state.on_event(down(VK_F9), || mods(false, true, false, false)),
                Decision::Pass,
                "Shift+F9 ist kein F9"
            );
            assert_eq!(
                state.on_event(up(VK_F9), || mods(false, true, false, false)),
                Decision::Pass,
                "das Up gehört der Zielanwendung, es gab kein akzeptiertes Down"
            );
            assert_eq!(
                state.on_event(down(VK_F9), ModifierState::default),
                Decision::Emit(HotkeyEvent::Press)
            );
        }

        #[test]
        fn missing_modifiers_are_not_a_match_either() {
            let required = ModifierState::required(&[Modifier::Ctrl, Modifier::Shift]);
            let mut state = HookState::new(VK_F9, required);
            assert_eq!(
                state.on_event(down(VK_F9), || mods(true, false, false, false)),
                Decision::Pass,
                "Ctrl+F9 ist kein Ctrl+Shift+F9"
            );
            assert_eq!(
                state.on_event(down(VK_F9), || mods(true, true, false, false)),
                Decision::Emit(HotkeyEvent::Press),
                "der vollständige Chord trifft"
            );
        }

        /// Lock-Tasten tauchen im Vergleich gar nicht auf — Caps/Num/Scroll
        /// können den Treffer deshalb nicht verhindern (X11: `lock_variants`).
        #[test]
        fn lock_keys_are_ignored() {
            // `ModifierState` hat schlicht kein Feld für Caps/Num/Scroll, und
            // `current()` fragt sie nie ab: jeder Lock-Zustand ist derselbe
            // Wert, der Vergleich kann daran nicht scheitern.
            let mut state = HookState::new(VK_F9, ModifierState::default());
            assert_eq!(
                state.on_event(down(VK_F9), ModifierState::default),
                Decision::Emit(HotkeyEvent::Press)
            );
            assert_eq!(
                state.on_event(up(VK_F9), ModifierState::default),
                Decision::Emit(HotkeyEvent::Release)
            );
        }

        /// §4.4: ein logisches Press, ein logisches Release — Auto-Repeat wird
        /// geschluckt, erreicht die Zielanwendung aber trotzdem nie.
        #[test]
        fn auto_repeat_is_swallowed_without_event() {
            let mut state = HookState::new(VK_F9, ModifierState::default());
            assert_eq!(
                state.on_event(down(VK_F9), ModifierState::default),
                Decision::Emit(HotkeyEvent::Press)
            );
            assert_eq!(
                state.on_event(down(VK_F9), ModifierState::default),
                Decision::Swallow
            );
            assert_eq!(
                state.on_event(down(VK_F9), ModifierState::default),
                Decision::Swallow
            );
            assert_eq!(
                state.on_event(up(VK_F9), ModifierState::default),
                Decision::Emit(HotkeyEvent::Release)
            );
            assert_eq!(
                state.on_event(up(VK_F9), ModifierState::default),
                Decision::Pass,
                "ein zweites Up gehört nicht mehr uns"
            );
        }

        /// Sol-Review: Wer das Down geschluckt hat, muss auch das Up schlucken
        /// — sonst sieht die Zielanwendung ein Up ohne Down.
        #[test]
        fn release_after_modifier_change_is_still_swallowed_and_reported() {
            let mut state = HookState::new(VK_F9, ModifierState::required(&[Modifier::Ctrl]));
            assert_eq!(
                state.on_event(down(VK_F9), || mods(true, false, false, false)),
                Decision::Emit(HotkeyEvent::Press)
            );
            // Ctrl wird vor F9 losgelassen — VK_CONTROL ist eine fremde Taste.
            assert_eq!(
                state.on_event(up(0x11), || mods(false, false, false, false)),
                Decision::Pass
            );
            assert_eq!(
                state.on_event(up(VK_F9), || mods(false, false, false, false)),
                Decision::Emit(HotkeyEvent::Release)
            );
        }

        /// Plan WP2: Beim Ungrab fällt der Zustand zurück, ein gehaltener Key
        /// wird danach nicht mehr geschluckt (§5.2-Pause-Semantik).
        ///
        /// **Bewusste Ausnahme zu §4.4** (Sol-Review): Wird während eines
        /// geschluckten Downs pausiert, erreicht das folgende Up die
        /// Zielanwendung ohne vorheriges Down. Das lässt keine Taste hängen —
        /// ein Up ohne Down ist für Windows folgenlos —, ist aber wörtlich ein
        /// Ereignis, das die App vom PTT-Key sieht. Die Alternative („disabled,
        /// pending release": erst das Up schlucken, dann unhooken) hielte den
        /// globalen Hook über die Pause hinaus am Leben und widerspräche der
        /// Pause-Semantik, deshalb bleibt es dabei.
        #[test]
        fn reset_passes_orphaned_up() {
            let mut state = HookState::new(VK_F9, ModifierState::default());
            assert_eq!(
                state.on_event(down(VK_F9), ModifierState::default),
                Decision::Emit(HotkeyEvent::Press)
            );
            state.reset();
            assert_eq!(
                state.on_event(up(VK_F9), ModifierState::default),
                Decision::Pass
            );
            assert_eq!(
                state.on_event(down(VK_F9), ModifierState::default),
                Decision::Emit(HotkeyEvent::Press),
                "danach greift der Hotkey wieder"
            );
        }

        /// Eigene `SendInput`-Events (WP3-Paste-Pfad) sind nie ein Hotkey und
        /// laufen immer an `CallNextHookEx` weiter.
        #[test]
        fn injected_events_never_match() {
            let mut state = HookState::new(VK_F9, ModifierState::default());
            let injected_down = KeyEvent {
                vk: VK_F9,
                down: true,
                injected: true,
            };
            assert_eq!(
                state.on_event(injected_down, ModifierState::default),
                Decision::Pass
            );
            let injected_up = KeyEvent {
                vk: VK_F9,
                down: false,
                injected: true,
            };
            assert_eq!(
                state.on_event(injected_up, ModifierState::default),
                Decision::Pass
            );
        }

        #[test]
        fn other_keys_are_never_touched() {
            let mut state = HookState::new(VK_F9, ModifierState::default());
            assert_eq!(
                state.on_event(down(0x41), ModifierState::default),
                Decision::Pass
            );
            assert_eq!(
                state.on_event(up(0x41), ModifierState::default),
                Decision::Pass
            );
        }
    }

    /// Beide Aufrufe sind idempotent — der Daemon schickt sie bei jedem
    /// Pause-Wechsel, auch mehrfach.
    #[test]
    fn register_and_unregister_are_idempotent() {
        let mut backend = FakeBackend::default();
        backend.register().unwrap();
        backend.register().unwrap();
        assert_eq!(backend.grabs, 1);
        backend.unregister().unwrap();
        backend.unregister().unwrap();
        assert_eq!(backend.ungrabs, 1);
    }
}
