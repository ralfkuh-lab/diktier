//! HotkeyBackend: Press/Release (Spec §4.4 / §5.1). Linux: global-hotkey, Fallback XGrabKey.

use thiserror::Error;

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

pub trait HotkeyBackend {
    fn register(&mut self) -> Result<(), HotkeyError>;
    fn poll(&mut self) -> Result<Option<HotkeyEvent>, HotkeyError>;
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

    fn poll(&mut self) -> Result<Option<HotkeyEvent>, HotkeyError> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Global(inner) => inner.poll(),
            #[cfg(target_os = "linux")]
            Self::Grab(inner) => inner.poll(),
            Self::Stub(inner) => inner.poll(),
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

    use global_hotkey::hotkey::{Code, HotKey};
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
    use x11rb::connection::Connection;
    use x11rb::protocol::Event;
    use x11rb::protocol::xkb;
    use x11rb::protocol::xkb::ConnectionExt as XkbExt;
    use x11rb::protocol::xproto::{ConnectionExt as XprotoExt, GrabMode, Keycode, ModMask, Window};
    use x11rb::rust_connection::RustConnection;

    use super::{Debounce, HotkeyBackend, HotkeyError, HotkeyEvent};

    pub struct GlobalHotkeyBackend {
        manager: GlobalHotKeyManager,
        hotkey: HotKey,
        debounce: Debounce,
        registered: bool,
    }

    impl GlobalHotkeyBackend {
        pub fn try_new() -> Result<Self, HotkeyError> {
            let manager = GlobalHotKeyManager::new()
                .map_err(|e| HotkeyError::Failed(format!("global-hotkey: {e}")))?;
            Ok(Self {
                manager,
                hotkey: HotKey::new(None, Code::F9),
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
            self.manager
                .register(self.hotkey)
                .map_err(|e| HotkeyError::Failed(format!("global-hotkey register: {e}")))?;
            // Probe-Roundtrip: hält jemand F9 (Access), ist der X11-Thread lebendig.
            if !probe_f9_grabbed() {
                let _ = self.manager.unregister(self.hotkey);
                return Err(HotkeyError::Failed(
                    "global-hotkey: X11-Thread tot nach Register".into(),
                ));
            }
            self.registered = true;
            Ok(())
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
            if self.registered {
                let _ = self.manager.unregister(self.hotkey);
            }
        }
    }

    enum GrabCmd {
        Shutdown,
    }

    pub struct X11GrabKeyBackend {
        cmd_tx: Option<Sender<GrabCmd>>,
        events: Receiver<HotkeyEvent>,
        debounce: Debounce,
        join: Option<thread::JoinHandle<()>>,
    }

    impl X11GrabKeyBackend {
        pub fn try_new() -> Result<Self, HotkeyError> {
            let (event_tx, events) = mpsc::channel();
            let (cmd_tx, cmd_rx) = mpsc::channel();
            let (ready_tx, ready_rx) = mpsc::channel();
            let join = thread::Builder::new()
                .name("diktier-xgrab".into())
                .spawn(move || grab_thread(event_tx, cmd_rx, ready_tx))
                .map_err(|e| HotkeyError::Failed(format!("XGrabKey-Thread: {e}")))?;
            match ready_rx.recv_timeout(Duration::from_secs(2)) {
                Ok(Ok(())) => Ok(Self {
                    cmd_tx: Some(cmd_tx),
                    events,
                    debounce: Debounce::default(),
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
    }

    impl HotkeyBackend for X11GrabKeyBackend {
        fn register(&mut self) -> Result<(), HotkeyError> {
            Ok(())
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
        tx: Sender<HotkeyEvent>,
        cmd_rx: Receiver<GrabCmd>,
        ready: Sender<Result<(), String>>,
    ) {
        if let Err(err) = grab_thread_inner(tx, cmd_rx, &ready) {
            let _ = ready.send(Err(err));
        }
    }

    fn grab_thread_inner(
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
        let keycode = keysym_to_keycode(&conn, 0xffc6)?; // XK_F9
        grab_all_lock_combos(&conn, root, keycode)?;
        conn.flush().map_err(|e| e.to_string())?;
        let _ = ready.send(Ok(()));

        let mut pressed = false;
        let mut pending_release: Option<u32> = None;
        loop {
            if matches!(cmd_rx.try_recv(), Ok(GrabCmd::Shutdown)) {
                break;
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

        for extra in extra_lock_masks() {
            let _ = conn.ungrab_key(keycode, root, extra);
        }
        let _ = conn.flush();
        Ok(())
    }

    fn extra_lock_masks() -> [ModMask; 4] {
        [
            ModMask::default(),
            ModMask::M2,
            ModMask::LOCK,
            ModMask::M2 | ModMask::LOCK,
        ]
    }

    fn grab_all_lock_combos(
        conn: &RustConnection,
        root: Window,
        keycode: Keycode,
    ) -> Result<(), String> {
        for mods in extra_lock_masks() {
            conn.grab_key(false, root, mods, keycode, GrabMode::ASYNC, GrabMode::ASYNC)
                .map_err(|e| e.to_string())?
                .check()
                .map_err(|e| format!("XGrabKey F9: {e}"))?;
        }
        Ok(())
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
        Err("F9-Keycode nicht gefunden".into())
    }

    fn probe_f9_grabbed() -> bool {
        let Ok((conn, screen)) = RustConnection::connect(None) else {
            return false;
        };
        let Ok(keycode) = keysym_to_keycode(&conn, 0xffc6) else {
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

#[cfg(target_os = "linux")]
pub fn new_backend() -> AnyHotkeyBackend {
    if let Ok(mut backend) = linux::GlobalHotkeyBackend::try_new()
        && backend.register().is_ok()
    {
        return AnyHotkeyBackend::Global(backend);
    }
    match linux::X11GrabKeyBackend::try_new() {
        Ok(backend) => AnyHotkeyBackend::Grab(backend),
        Err(_) => AnyHotkeyBackend::Stub(StubHotkeyBackend),
    }
}

#[cfg(windows)]
pub fn new_backend() -> AnyHotkeyBackend {
    AnyHotkeyBackend::Stub(StubHotkeyBackend)
}

/// Expliziter XGrabKey-Fallback (Spike-Auswahl, Spec §3).
#[cfg(target_os = "linux")]
#[allow(dead_code)]
pub fn new_xgrab_backend() -> Result<AnyHotkeyBackend, HotkeyError> {
    linux::X11GrabKeyBackend::try_new().map(AnyHotkeyBackend::Grab)
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
}
