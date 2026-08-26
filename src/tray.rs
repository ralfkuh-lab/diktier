//! Tray-Backend (Spec §4.3). Phase 0: Stub, kein betrayer.
#![allow(dead_code)]

use thiserror::Error;

use crate::state::Runtime;

#[derive(Debug, Error)]
pub enum TrayError {
    #[error("Tray fehlgeschlagen: {0}")]
    Failed(String),
}

pub trait TrayBackend {
    fn update(&mut self, runtime: &Runtime, model_key: &str) -> Result<(), TrayError>;
}

#[derive(Debug, Default)]
pub struct StubTray;

impl TrayBackend for StubTray {
    fn update(&mut self, _runtime: &Runtime, _model_key: &str) -> Result<(), TrayError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_MODEL;

    #[test]
    fn stub_tray_update_succeeds() {
        let mut tray = StubTray;
        tray.update(&Runtime::default(), DEFAULT_MODEL).unwrap();
    }
}
