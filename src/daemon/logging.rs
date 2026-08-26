//! stderr-Log des Daemons (Spec §10, Teil 1).
//!
//! Phase 3c schreibt ausschließlich nach stderr; Datei-Log und Rotation sind
//! Phase 3d. Der Vertrag aus §10 gilt trotzdem schon hier: **keine
//! Transkripte, keine Clipboard-Inhalte, keine Fenstertitel**. Wo eine
//! Textmenge interessant ist, steht ihre Länge in Bytes.

use std::time::Instant;

use crate::state::{AppState, CopyReason, ErrorKind, LogEvent, RecordingSource, RunId};

/// Kurzname eines Zustands für Logzeilen (§5.2-Namen, klein geschrieben).
pub fn state_name(state: AppState) -> &'static str {
    match state {
        AppState::Starting => "starting",
        AppState::Downloading => "downloading",
        AppState::Loading => "loading",
        AppState::Idle => "idle",
        AppState::Recording {
            source: RecordingSource::Hotkey,
        } => "recording(hotkey)",
        AppState::Recording {
            source: RecordingSource::TrayClick,
        } => "recording(tray-click)",
        AppState::Transcribing {
            source: RecordingSource::Hotkey,
        } => "transcribing(hotkey)",
        AppState::Transcribing {
            source: RecordingSource::TrayClick,
        } => "transcribing(tray-click)",
        AppState::Injecting {
            source: RecordingSource::Hotkey,
        } => "injecting(hotkey)",
        AppState::Injecting {
            source: RecordingSource::TrayClick,
        } => "injecting(tray-click)",
        AppState::Error => "error",
    }
}

pub fn copy_reason_name(reason: CopyReason) -> &'static str {
    match reason {
        CopyReason::TrayClickPath => "Tray-Click-Pfad",
        CopyReason::FocusChanged => "Fokus geändert",
        CopyReason::FocusUnknown => "Fokus nicht ermittelbar",
    }
}

pub fn error_kind_name(kind: ErrorKind) -> &'static str {
    kind.as_str()
}

/// Menschenlesbare Zeile zu einem [`LogEvent`] des Kerns.
pub fn describe(event: &LogEvent) -> String {
    match event {
        LogEvent::IgnoredPress { state } => {
            format!("Hotkey-Press ignoriert (Zustand {})", state_name(*state))
        }
        LogEvent::IgnoredRelease { state } => {
            format!("Hotkey-Release ignoriert (Zustand {})", state_name(*state))
        }
        LogEvent::IgnoredTrayClick { state } => {
            format!("Tray-Linksklick ignoriert (Zustand {})", state_name(*state))
        }
        LogEvent::IgnoredWhilePaused => "Hotkey-Press ignoriert (pausiert)".into(),
        LogEvent::StaleRun { what, got, current } => format!(
            "verspätete Antwort verworfen: {what} (Lauf {}, aktuell {})",
            got.0, current.0
        ),
        LogEvent::AudioTooShort { millis } => {
            format!("Aufnahme {millis} ms < 250 ms — keine Transkription")
        }
        LogEvent::EmptyTranscript => "Transkript leer — nichts eingefügt".into(),
        LogEvent::RecordingDiscarded => "Pause aktiviert — laufende Aufnahme verworfen".into(),
        LogEvent::CopyOnlyNotice { reason } => format!(
            "Text liegt in der Zwischenablage ({})",
            copy_reason_name(*reason)
        ),
        LogEvent::Failure { kind } => format!("Fehlerzustand: {}", error_kind_name(*kind)),
        LogEvent::IgnoredRetry { state } => {
            format!("Retry ignoriert (Zustand {})", state_name(*state))
        }
        LogEvent::QuitRequested { state } => {
            format!("Beenden angefordert (Zustand {})", state_name(*state))
        }
        LogEvent::IgnoredAfterQuit => "Ereignis nach dem Beenden ignoriert".into(),
    }
}

/// stderr-Logger mit monotoner Laufzeitmarke.
///
/// `info` erscheint nur mit `--foreground` (§9). Warnungen und Fehler gehen
/// immer nach stderr — auch ohne Konsole schadet das nicht, und ohne sie hätte
/// ein Startfehler in Phase 3c gar keinen Kanal.
pub struct Logger {
    start: Instant,
    foreground: bool,
}

impl Logger {
    pub fn new(foreground: bool) -> Self {
        Self {
            start: Instant::now(),
            foreground,
        }
    }

    fn stamp(&self) -> String {
        format!("[+{:8.3}s]", self.start.elapsed().as_secs_f64())
    }

    pub fn info(&self, message: impl AsRef<str>) {
        if self.foreground {
            eprintln!("{} {}", self.stamp(), message.as_ref());
        }
    }

    pub fn warn(&self, message: impl AsRef<str>) {
        eprintln!("{} WARN  {}", self.stamp(), message.as_ref());
    }

    pub fn error(&self, message: impl AsRef<str>) {
        eprintln!("{} ERROR {}", self.stamp(), message.as_ref());
    }

    /// Kern-Logeffekt (§5.2). Ignorierte Eingaben sind Alltag → `info`.
    pub fn core(&self, event: &LogEvent) {
        match event {
            LogEvent::Failure { .. } => self.warn(describe(event)),
            other => self.info(describe(other)),
        }
    }

    /// Zustandswechsel, den der Tray sichtbar macht (§4.3).
    pub fn transition(&self, state: AppState, paused: bool) {
        if paused {
            self.info(format!("Zustand: {} (pausiert)", state_name(state)));
        } else {
            self.info(format!("Zustand: {}", state_name(state)));
        }
    }

    pub fn run(&self, run: RunId, message: impl AsRef<str>) {
        self.info(format!("Lauf {}: {}", run.0, message.as_ref()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_names_cover_every_state() {
        let states = [
            AppState::Starting,
            AppState::Downloading,
            AppState::Loading,
            AppState::Idle,
            AppState::Recording {
                source: RecordingSource::Hotkey,
            },
            AppState::Recording {
                source: RecordingSource::TrayClick,
            },
            AppState::Transcribing {
                source: RecordingSource::Hotkey,
            },
            AppState::Transcribing {
                source: RecordingSource::TrayClick,
            },
            AppState::Injecting {
                source: RecordingSource::Hotkey,
            },
            AppState::Injecting {
                source: RecordingSource::TrayClick,
            },
            AppState::Error,
        ];
        for state in states {
            assert!(!state_name(state).is_empty(), "{state:?}");
        }
        assert_eq!(
            state_name(AppState::Recording {
                source: RecordingSource::TrayClick
            }),
            "recording(tray-click)"
        );
    }

    /// §10: Logzeilen enthalten nie Transkripte — nur Zustände, Zahlen, Gründe.
    #[test]
    fn describe_never_contains_transcript_text() {
        let events = [
            LogEvent::EmptyTranscript,
            LogEvent::CopyOnlyNotice {
                reason: CopyReason::FocusChanged,
            },
            LogEvent::AudioTooShort { millis: 120 },
            LogEvent::StaleRun {
                what: "transcription-done",
                got: RunId(3),
                current: RunId(5),
            },
            LogEvent::Failure {
                kind: ErrorKind::Inject,
            },
            LogEvent::QuitRequested {
                state: AppState::Idle,
            },
            LogEvent::IgnoredAfterQuit,
            LogEvent::IgnoredWhilePaused,
            LogEvent::RecordingDiscarded,
            LogEvent::IgnoredRetry {
                state: AppState::Idle,
            },
            LogEvent::IgnoredPress {
                state: AppState::Loading,
            },
            LogEvent::IgnoredRelease {
                state: AppState::Idle,
            },
            LogEvent::IgnoredTrayClick {
                state: AppState::Loading,
            },
        ];
        for event in &events {
            let line = describe(event);
            assert!(!line.is_empty(), "{event:?}");
            assert!(!line.contains('\n'), "einzeilig: {line}");
        }
        assert!(describe(&events[3]).contains("Lauf 3"));
        assert!(describe(&events[2]).contains("120 ms"));
    }
}
