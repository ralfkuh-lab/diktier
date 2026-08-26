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
    /// Nur der Windows-Pfad baut ihn (dort folgt das Backend in Phase 4);
    /// unter Linux meldet `new_backend` stattdessen einen Fehler.
    #[allow(dead_code)]
    Stub(StubHotkeyBackend),
}

impl HotkeyBackend for AnyHotkeyBackend {
    fn register(&mut self) -> Result<(), HotkeyError> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Global(inner) => inner.register(),
            #[cfg(target_os = "linux")]
            Self::Grab(inner) => inner.register(),
            Self::Stub(inner) => inner.register(),
        }
    }

    fn unregister(&mut self) -> Result<(), HotkeyError> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Global(inner) => inner.unregister(),
            #[cfg(target_os = "linux")]
            Self::Grab(inner) => inner.unregister(),
            Self::Stub(inner) => inner.unregister(),
        }
    }

    fn poll(&mut self) -> Result<Option<HotkeyEvent>, HotkeyError> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Global(inner) => inner.poll(),
            #[cfg(target_os = "linux")]
            Self::Grab(inner) => inner.poll(),
            Self::Stub(inner) => inner.poll(),
        }
    }

    fn is_registered(&self) -> bool {
        match self {
            #[cfg(target_os = "linux")]
            Self::Global(inner) => inner.is_registered(),
            #[cfg(target_os = "linux")]
            Self::Grab(inner) => inner.is_registered(),
            Self::Stub(inner) => inner.is_registered(),
        }
    }

    fn backend_name(&self) -> &'static str {
        match self {
            #[cfg(target_os = "linux")]
            Self::Global(inner) => inner.backend_name(),
            #[cfg(target_os = "linux")]
            Self::Grab(inner) => inner.backend_name(),
            Self::Stub(inner) => inner.backend_name(),
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

#[cfg(windows)]
pub fn new_backend(_spec: &HotkeySpec) -> Result<AnyHotkeyBackend, HotkeyError> {
    Ok(AnyHotkeyBackend::Stub(StubHotkeyBackend))
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
