//! Zustandstypen nach Spec §5.2. Übergangslogik folgt in Phase 3.
#![allow(dead_code)]

/// Quelle einer laufenden oder gerade beendeten Aufnahme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingSource {
    Hotkey,
    TrayClick,
}

/// Prozesszustand ohne das orthogonale `paused`-Flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppState {
    Starting,
    Downloading,
    Loading,
    Idle,
    Recording { source: RecordingSource },
    Transcribing { source: RecordingSource },
    Error,
}

/// Laufzeitstatus: Zustand plus orthogonales Pause-Flag (Hotkey aus).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    pub state: AppState,
    pub paused: bool,
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            state: AppState::Starting,
            paused: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paused_is_orthogonal_to_state() {
        let mut runtime = Runtime::default();
        assert_eq!(runtime.state, AppState::Starting);
        assert!(!runtime.paused);

        runtime.state = AppState::Recording {
            source: RecordingSource::Hotkey,
        };
        runtime.paused = true;
        assert!(runtime.paused);
        assert_eq!(
            runtime.state,
            AppState::Recording {
                source: RecordingSource::Hotkey
            }
        );

        runtime.state = AppState::Recording {
            source: RecordingSource::TrayClick,
        };
        assert!(runtime.paused);
    }
}
