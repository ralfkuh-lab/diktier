//! OutputSink: `paste` | `copy_only` (Spec §5.1 / §7). `review` ist v2 und wird nicht vorbereitet.

#![allow(dead_code)]

use std::time::{Duration, Instant};

use thiserror::Error;

use crate::config::OutputConfig;

mod protocol;

#[cfg(test)]
mod fake;

#[cfg(target_os = "linux")]
mod linux;

pub use protocol::{RESTORED_SERVE_GRACE, ResolvedShortcut};

/// Native Vordergrund-Kennung (HWND bzw. X11-Window), als portable Zahl.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureContext {
    pub start_window_id: Option<WindowId>,
    pub target_window_id: Option<WindowId>,
    pub ended_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteKey {
    Shift,
    Alt,
    Super,
    Ctrl,
    V,
    Insert,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyOnlyReason {
    FocusChanged,
    FocusUnknown,
}

impl CopyOnlyReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FocusChanged => "Fokus geändert — Text liegt im Clipboard",
            Self::FocusUnknown => "Fokus nicht ermittelbar — Text liegt im Clipboard",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreDecision {
    Wait,
    Restore,
    NoReadTimeout,
    ForeignOwner,
    NoPromise,
    Disabled,
}

impl RestoreDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wait => "wait",
            Self::Restore => "restored",
            Self::NoReadTimeout => "Einfügen nicht bestätigt — Text liegt in der Zwischenablage",
            Self::ForeignOwner => "fremder Clipboard-Inhalt bleibt (kein Restore)",
            Self::NoPromise => "Nicht-Text-Clipboard konnte nicht restauriert werden",
            Self::Disabled => "restore_clipboard=false",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InjectOutcome {
    Pasted {
        restored: bool,
        shortcut: ResolvedShortcut,
        window: WindowId,
        wm_class: Option<(String, String)>,
        reads: u32,
        restore: RestoreDecision,
    },
    CopyOnly {
        reason: CopyOnlyReason,
    },
}

#[derive(Debug, Error)]
pub enum InjectError {
    #[error("Ausgabe fehlgeschlagen: {0}")]
    Failed(String),
}

pub trait OutputSink {
    /// Paste am Cursor. **Phase-3-Vorgabe (codex H4 / Spec §7.1 P6):** der
    /// Restore-Wait darf den Aufrufer nicht blockieren — nichtblockierende
    /// Session, Timer in der State-Machine, Quit-Reaktivität. Kein Umbau in v1-Spike.
    fn paste(&mut self, text: &str, ctx: &CaptureContext) -> Result<InjectOutcome, InjectError>;
    fn copy_only(&mut self, text: &str) -> Result<(), InjectError>;
    fn current_window_id(&self) -> Option<WindowId> {
        None
    }
    fn serve_for(&mut self, _duration: Duration) -> Result<(), InjectError> {
        Ok(())
    }
    /// Spike: restaurierte Selection bedienen, bis ein Daten-Read kam oder `timeout`.
    fn serve_until_read(&mut self, _timeout: Duration) -> Result<u32, InjectError> {
        Ok(0)
    }
}

#[derive(Debug, Default)]
pub struct StubOutputSink;

impl OutputSink for StubOutputSink {
    fn paste(&mut self, _text: &str, _ctx: &CaptureContext) -> Result<InjectOutcome, InjectError> {
        Ok(InjectOutcome::CopyOnly {
            reason: CopyOnlyReason::FocusUnknown,
        })
    }

    fn copy_only(&mut self, _text: &str) -> Result<(), InjectError> {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
pub type PlatformSink = linux::X11OutputSink;
#[cfg(windows)]
pub type PlatformSink = StubOutputSink;

pub fn new_sink(output: OutputConfig) -> Result<PlatformSink, InjectError> {
    #[cfg(target_os = "linux")]
    {
        linux::X11OutputSink::new(output)
    }
    #[cfg(windows)]
    {
        let _ = output;
        Ok(StubOutputSink)
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{FakeContent, FakeHost, KeyStroke, ScriptEvent};
    use super::protocol::{
        ClipboardHost, ClipboardSnapshot, ModifierState, RestoreSession, auto_shortcut,
        inject_paste, modifiers_to_clear, modifiers_to_restore, resolve_paste_shortcut,
        serve_restored_until_read,
    };
    use super::*;
    use crate::config::{OutputConfig, PasteShortcut};
    use std::time::Duration;

    fn ctx(id: u64) -> CaptureContext {
        CaptureContext {
            start_window_id: Some(WindowId(id)),
            target_window_id: Some(WindowId(id)),
            ended_at: Instant::now(),
        }
    }

    fn output() -> OutputConfig {
        OutputConfig {
            restore_clipboard: true,
            restore_clipboard_delay_ms: 200,
            paste_shortcut: PasteShortcut::Auto,
            leading_space: false,
            ..OutputConfig::default()
        }
    }

    fn has_stroke(sent: &[KeyStroke], key: PasteKey, down: bool) -> bool {
        sent.iter().any(|s| s.key == key && s.down == down)
    }

    #[test]
    fn stub_paste_and_copy_only_succeed() {
        let mut sink = StubOutputSink;
        let outcome = sink.paste("Hallo", &ctx(1)).unwrap();
        assert!(matches!(outcome, InjectOutcome::CopyOnly { .. }));
        sink.copy_only("Hallo").unwrap();
    }

    #[test]
    fn paste_restore_after_read_and_delay() {
        let mut host = FakeHost::new().with_text("vorher").with_script(vec![(
            Duration::from_millis(10),
            ScriptEvent::SelectionRequest,
        )]);
        let outcome = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        match outcome {
            InjectOutcome::Pasted {
                restored,
                reads,
                restore,
                ..
            } => {
                assert!(restored);
                assert!(reads >= 1);
                assert_eq!(restore, RestoreDecision::Restore);
            }
            other => panic!("expected Pasted, got {other:?}"),
        }
        assert_eq!(host.clipboard_text(), Some("vorher"));
        assert!(host.still_owner().unwrap());
    }

    #[test]
    fn no_read_means_no_restore() {
        let mut host = FakeHost::new().with_text("vorher");
        let outcome = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        match outcome {
            InjectOutcome::Pasted {
                restored, restore, ..
            } => {
                assert!(!restored);
                assert_eq!(restore, RestoreDecision::NoReadTimeout);
            }
            other => panic!("expected Pasted, got {other:?}"),
        }
        assert_eq!(host.clipboard_text(), Some("transkript"));
        assert!(host.still_owner().unwrap());
    }

    #[test]
    fn foreign_change_during_wait_never_restores() {
        let mut host = FakeHost::new().with_text("vorher").with_script(vec![
            (Duration::from_millis(10), ScriptEvent::SelectionRequest),
            (
                Duration::from_millis(50),
                ScriptEvent::ForeignTakeover(FakeContent::Text("fremd".into())),
            ),
        ]);
        let outcome = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        match outcome {
            InjectOutcome::Pasted {
                restored, restore, ..
            } => {
                assert!(!restored);
                assert_eq!(restore, RestoreDecision::ForeignOwner);
            }
            other => panic!("expected Pasted, got {other:?}"),
        }
        assert_eq!(host.clipboard_text(), Some("fremd"));
        assert!(!host.still_owner().unwrap());
    }

    #[test]
    fn non_text_snapshot_has_no_restore_promise() {
        let mut host = FakeHost::new().with_non_text().with_script(vec![(
            Duration::from_millis(10),
            ScriptEvent::SelectionRequest,
        )]);
        let outcome = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        match outcome {
            InjectOutcome::Pasted {
                restored, restore, ..
            } => {
                assert!(!restored);
                assert_eq!(restore, RestoreDecision::NoPromise);
            }
            other => panic!("expected Pasted, got {other:?}"),
        }
        assert_eq!(host.clipboard_text(), Some("transkript"));
    }

    #[test]
    fn modifier_restore_only_if_physically_held() {
        let held = ModifierState {
            shift: true,
            alt: true,
            ..ModifierState::default()
        };
        let cleared = modifiers_to_clear(held, ResolvedShortcut::CtrlV);
        assert_eq!(cleared, vec![PasteKey::Shift, PasteKey::Alt]);

        let still_shift = ModifierState {
            shift: true,
            ..ModifierState::default()
        };
        assert_eq!(
            modifiers_to_restore(&cleared, still_shift),
            vec![PasteKey::Shift]
        );
        assert!(modifiers_to_restore(&cleared, ModifierState::default()).is_empty());
    }

    #[test]
    fn modifier_restore_skipped_when_query_shows_up() {
        let mut host = FakeHost::new().with_text("vorher").with_script(vec![(
            Duration::from_millis(10),
            ScriptEvent::SelectionRequest,
        )]);
        host.physical.shift = true;
        host.synthetic_affects_physical = true;
        inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        assert!(has_stroke(&host.sent, PasteKey::Shift, false));
        assert!(!has_stroke(&host.sent, PasteKey::Shift, true));
    }

    #[test]
    fn modifier_restore_sent_when_still_physically_held() {
        let mut host = FakeHost::new().with_text("vorher").with_script(vec![(
            Duration::from_millis(10),
            ScriptEvent::SelectionRequest,
        )]);
        host.physical.shift = true;
        host.synthetic_affects_physical = false;
        inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        let ups = host
            .sent
            .iter()
            .filter(|s| s.key == PasteKey::Shift && !s.down)
            .count();
        let downs = host
            .sent
            .iter()
            .filter(|s| s.key == PasteKey::Shift && s.down)
            .count();
        assert_eq!(ups, 1);
        assert_eq!(downs, 1);
    }

    #[test]
    fn auto_shortcut_mapping_table() {
        let cases = [
            (
                ("gnome-terminal", "Gnome-terminal"),
                ResolvedShortcut::CtrlShiftV,
            ),
            (
                ("gnome-terminal-server", "Gnome-terminal"),
                ResolvedShortcut::CtrlShiftV,
            ),
            (
                ("org.gnome.Terminal", "Gnome-terminal"),
                ResolvedShortcut::CtrlShiftV,
            ),
            (
                ("xfce4-terminal", "Xfce4-terminal"),
                ResolvedShortcut::CtrlShiftV,
            ),
            (("tilix", "Tilix"), ResolvedShortcut::CtrlShiftV),
            (("Alacritty", "Alacritty"), ResolvedShortcut::CtrlShiftV),
            (("kitty", "kitty"), ResolvedShortcut::CtrlShiftV),
            (("ghostty", "Ghostty"), ResolvedShortcut::CtrlShiftV),
            (("xterm", "XTerm"), ResolvedShortcut::ShiftInsert),
            (("uxterm", "UXTerm"), ResolvedShortcut::ShiftInsert),
            (("xed", "Xed"), ResolvedShortcut::CtrlV),
            (("code", "Code"), ResolvedShortcut::CtrlV),
            (("firefox", "Firefox"), ResolvedShortcut::CtrlV),
        ];
        for ((instance, class), expected) in cases {
            assert_eq!(
                auto_shortcut(Some((instance, class))),
                expected,
                "{instance}/{class}"
            );
            assert_eq!(
                resolve_paste_shortcut(PasteShortcut::Auto, Some((instance, class))),
                expected,
                "auto {instance}/{class}"
            );
        }
        assert_eq!(auto_shortcut(None), ResolvedShortcut::CtrlV);
        assert_eq!(
            resolve_paste_shortcut(
                PasteShortcut::CtrlV,
                Some(("gnome-terminal", "Gnome-terminal"))
            ),
            ResolvedShortcut::CtrlV
        );
        assert_eq!(
            resolve_paste_shortcut(PasteShortcut::CtrlShiftV, Some(("xed", "Xed"))),
            ResolvedShortcut::CtrlShiftV
        );
    }

    #[test]
    fn gnome_terminal_auto_sends_ctrl_shift_v() {
        let mut host = FakeHost::new()
            .with_text("vorher")
            .with_wm_class("gnome-terminal-server", "Gnome-terminal")
            .with_script(vec![(
                Duration::from_millis(10),
                ScriptEvent::SelectionRequest,
            )]);
        let outcome = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        match outcome {
            InjectOutcome::Pasted { shortcut, .. } => {
                assert_eq!(shortcut, ResolvedShortcut::CtrlShiftV);
            }
            other => panic!("expected Pasted, got {other:?}"),
        }
        assert!(has_stroke(&host.sent, PasteKey::Ctrl, true));
        assert!(has_stroke(&host.sent, PasteKey::Shift, true));
        assert!(has_stroke(&host.sent, PasteKey::V, true));
        assert!(!has_stroke(&host.sent, PasteKey::Insert, true));
    }

    #[test]
    fn none_window_id_is_focus_loss() {
        let mut host = FakeHost::new().with_text("vorher").with_window(None);
        let outcome = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        assert_eq!(
            outcome,
            InjectOutcome::CopyOnly {
                reason: CopyOnlyReason::FocusUnknown
            }
        );
        assert_eq!(host.clipboard_text(), Some("transkript"));

        let mut host = FakeHost::new()
            .with_text("vorher")
            .with_window(Some(WindowId(1)));
        let blank = CaptureContext {
            start_window_id: None,
            target_window_id: Some(WindowId(1)),
            ended_at: Instant::now(),
        };
        let outcome = inject_paste(&mut host, "transkript", &blank, &output()).unwrap();
        assert_eq!(
            outcome,
            InjectOutcome::CopyOnly {
                reason: CopyOnlyReason::FocusUnknown
            }
        );
    }

    #[test]
    fn mismatched_focus_is_copy_only() {
        let mut host = FakeHost::new()
            .with_text("vorher")
            .with_window(Some(WindowId(2)));
        let start_end = CaptureContext {
            start_window_id: Some(WindowId(1)),
            target_window_id: Some(WindowId(1)),
            ended_at: Instant::now(),
        };
        let outcome = inject_paste(&mut host, "transkript", &start_end, &output()).unwrap();
        assert!(matches!(
            outcome,
            InjectOutcome::CopyOnly {
                reason: CopyOnlyReason::FocusChanged
            }
        ));
        assert_eq!(host.clipboard_text(), Some("transkript"));
        assert!(host.sent.is_empty());
    }

    #[test]
    fn restore_session_wait_then_restore() {
        let session = RestoreSession::new(
            ClipboardSnapshot::Text("alt".into()),
            Duration::from_millis(200),
            true,
        );
        assert_eq!(
            session.decide(Duration::from_millis(10)),
            RestoreDecision::Wait
        );
        let mut session = session;
        session.note_read();
        assert_eq!(
            session.decide(Duration::from_millis(10)),
            RestoreDecision::Wait
        );
        assert_eq!(
            session.decide(Duration::from_millis(200)),
            RestoreDecision::Restore
        );
    }

    #[test]
    fn spike_serves_restored_selection_until_read() {
        let mut host = FakeHost::new().with_script(vec![(
            Duration::from_millis(10),
            ScriptEvent::SelectionRequest,
        )]);
        host.become_owner("snapshot".into()).unwrap();
        let n = serve_restored_until_read(&mut host, RESTORED_SERVE_GRACE).unwrap();
        assert_eq!(n, 1);
        assert!(host.elapsed() < RESTORED_SERVE_GRACE);
    }

    #[test]
    fn spike_restored_serve_times_out_without_read() {
        let mut host = FakeHost::new();
        host.become_owner("snapshot".into()).unwrap();
        let n = serve_restored_until_read(&mut host, RESTORED_SERVE_GRACE).unwrap();
        assert_eq!(n, 0);
        assert_eq!(host.elapsed(), RESTORED_SERVE_GRACE);
    }

    #[test]
    fn queued_selection_clear_before_paste_uses_foreign_snapshot() {
        let mut host = FakeHost::new().with_text("vorher").with_script(vec![(
            Duration::from_millis(10),
            ScriptEvent::SelectionRequest,
        )]);
        inject_paste(&mut host, "eins", &ctx(1), &output()).unwrap();
        assert_eq!(host.clipboard_text(), Some("vorher"));
        host.queue_clear(FakeContent::Text("fremd".into()));
        let outcome = inject_paste(&mut host, "zwei", &ctx(1), &output()).unwrap();
        match outcome {
            InjectOutcome::Pasted {
                restored, restore, ..
            } => {
                assert!(restored);
                assert_eq!(restore, RestoreDecision::Restore);
            }
            other => panic!("expected restore of foreign snapshot, got {other:?}"),
        }
        assert_eq!(host.clipboard_text(), Some("fremd"));
    }

    #[test]
    fn takeover_before_empty_restore_keeps_foreign() {
        let mut host = FakeHost::new()
            .with_script(vec![(
                Duration::from_millis(10),
                ScriptEvent::SelectionRequest,
            )])
            .with_takeover_before_release(FakeContent::Text("fremd".into()));
        let outcome = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        match outcome {
            InjectOutcome::Pasted {
                restored, restore, ..
            } => {
                assert!(!restored);
                assert_eq!(restore, RestoreDecision::ForeignOwner);
            }
            other => panic!("expected Pasted, got {other:?}"),
        }
        assert_eq!(host.clipboard_text(), Some("fremd"));
    }

    #[test]
    fn focus_change_during_snapshot_is_copy_only() {
        let mut host = FakeHost::new()
            .with_text("vorher")
            .with_focus_after_snapshot(Some(WindowId(2)));
        let outcome = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        assert_eq!(
            outcome,
            InjectOutcome::CopyOnly {
                reason: CopyOnlyReason::FocusChanged
            }
        );
        assert_eq!(host.clipboard_text(), Some("transkript"));
        assert!(host.sent.is_empty());
    }

    #[test]
    fn failed_data_request_does_not_count_as_read() {
        let mut host = FakeHost::new()
            .with_text("vorher")
            .with_fail_data_request()
            .with_script(vec![(
                Duration::from_millis(10),
                ScriptEvent::SelectionRequest,
            )]);
        let outcome = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        match outcome {
            InjectOutcome::Pasted {
                restored, restore, ..
            } => {
                assert!(!restored);
                assert_eq!(restore, RestoreDecision::NoReadTimeout);
            }
            other => panic!("expected Pasted, got {other:?}"),
        }
        assert_eq!(host.clipboard_text(), Some("transkript"));
    }

    #[test]
    fn dead_connection_is_error_not_foreign() {
        let mut host = FakeHost::new().with_text("vorher").with_dead_connection();
        let err = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap_err();
        assert!(err.to_string().contains("tot"));
    }

    #[test]
    fn chord_failure_releases_keys_we_pressed() {
        let mut host = FakeHost::new()
            .with_text("vorher")
            .with_fail_key_after(2)
            .with_script(vec![(
                Duration::from_millis(10),
                ScriptEvent::SelectionRequest,
            )]);
        let err = inject_paste(&mut host, "transkript", &ctx(1), &output());
        assert!(err.is_err());
        let downs: Vec<_> = host.sent.iter().filter(|s| s.down).map(|s| s.key).collect();
        let ups: Vec<_> = host
            .sent
            .iter()
            .filter(|s| !s.down)
            .map(|s| s.key)
            .collect();
        for key in downs {
            assert!(ups.contains(&key), "Taste {key:?} ohne Up nach Fehler");
        }
    }

    #[test]
    fn leading_space_prefixed_unless_empty_or_present() {
        use super::protocol::apply_leading_space;
        assert_eq!(apply_leading_space("Hallo", true), " Hallo");
        assert_eq!(apply_leading_space(" Hallo", true), " Hallo");
        assert_eq!(apply_leading_space("", true), "");
        assert_eq!(apply_leading_space("Hallo", false), "Hallo");

        let mut cfg = output();
        cfg.leading_space = true;
        let mut host = FakeHost::new().with_window(None);
        inject_paste(&mut host, "Hi", &ctx(1), &cfg).unwrap();
        assert_eq!(host.clipboard_text(), Some(" Hi"));
    }
}
