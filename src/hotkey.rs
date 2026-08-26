//! HotkeyBackend: Press/Release (Spec §5.1). Phase 0: Stub, kein global-hotkey / WH_KEYBOARD_LL.
#![allow(dead_code)]

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

#[cfg(windows)]
mod windows {
    use super::StubHotkeyBackend;

    pub fn create() -> StubHotkeyBackend {
        StubHotkeyBackend
    }
}

#[cfg(unix)]
mod unix {
    use super::StubHotkeyBackend;

    pub fn create() -> StubHotkeyBackend {
        StubHotkeyBackend
    }
}

#[cfg(unix)]
pub use unix::create as new_backend;
#[cfg(windows)]
pub use windows::create as new_backend;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_backend_registers_and_is_idle() {
        let mut backend = new_backend();
        backend.register().unwrap();
        assert_eq!(backend.poll().unwrap(), None);
    }
}
