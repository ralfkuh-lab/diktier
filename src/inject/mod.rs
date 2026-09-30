//! OutputSink: `paste` | `copy_only` (Spec §5.1 / §7). `review` ist v2 und wird nicht vorbereitet.

#![allow(dead_code)]

use std::time::{Duration, Instant};

use thiserror::Error;

use crate::config::OutputConfig;

pub mod formats;
mod protocol;

#[cfg(test)]
pub(crate) mod fake;

mod windows;

use formats::{LostFormat, SnapshotReport};

pub use protocol::{ClipboardSnapshot, RESTORED_SERVE_GRACE, ResolvedShortcut};
pub use windows::{RoundtripRestore, clipboard_check, clipboard_roundtrip};

/// Native Vordergrund-Kennung (HWND), als portable Zahl.
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

/// Entscheidung bzw. Ausgang des Restores (§7.1, §7.1.1).
///
/// `Wait` und `Restore` sind Zwischenstände von `RestoreSession::decide`
/// (P5–P7, unverändert). In [`InjectOutcome::Pasted`] steht immer ein
/// Endausgang: `Restored`, `RestoredPartial`, `RestoreFailed`,
/// `NoReadTimeout`, `ForeignOwner`, `NoPromise` oder `Disabled`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreDecision {
    Wait,
    /// Restore fällig (Read bedient, Mindestwartezeit um).
    Restore,
    /// Alles Gesicherte zurückgeschrieben, beim Sichern nichts verloren.
    Restored,
    /// Mindestens ein Nutzformat zurück, aber Verluste. `lost_on_restore`:
    /// auch beim Zurückschreiben ging etwas verloren (nicht nur beim Sichern) —
    /// die Hinweiszeile unterscheidet beide Fälle (Plan K3).
    RestoredPartial {
        lost_on_restore: bool,
    },
    /// Kein Nutzformat zurückgeschrieben; das Transkript liegt in der
    /// Zwischenablage.
    RestoreFailed,
    NoReadTimeout,
    ForeignOwner,
    /// Snapshot `Unrestorable`: kein Restore-Versprechen (§7.1 Punkt 2).
    NoPromise,
    Disabled,
}

impl RestoreDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wait => "wait",
            Self::Restore => "restore fällig",
            Self::Restored => "restored",
            Self::RestoredPartial { .. } => "teilweise wiederhergestellt",
            Self::RestoreFailed => {
                "Zwischenablage nicht wiederhergestellt — Transkript liegt in der Zwischenablage"
            }
            Self::NoReadTimeout => "Einfügen nicht bestätigt — Text liegt in der Zwischenablage",
            Self::ForeignOwner => "fremder Clipboard-Inhalt bleibt (kein Restore)",
            Self::NoPromise => {
                "Zwischenablage nicht gesichert — Transkript liegt in der Zwischenablage"
            }
            Self::Disabled => "restore_clipboard=false",
        }
    }

    /// Der vorherige Inhalt ist (ganz oder teilweise) zurück.
    pub fn is_restored(self) -> bool {
        matches!(self, Self::Restored | Self::RestoredPartial { .. })
    }
}

/// Clipboard-Details eines Paste-Laufs, nur fürs Log und für WP3 (Hinweis).
/// Keine Inhalte: IDs, bereinigte Namen, Größen, Dauer, Verlustgründe (§10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardReport {
    pub snapshot: SnapshotReport,
    /// Verluste in Phase „Wiederherstellen“ (leer, wenn nicht restauriert).
    pub lost_restore: Vec<LostFormat>,
    /// Der eigene Inhalt am Ende des Laufs (restauriert oder Transkript)
    /// trägt den Verlaufsausschluss (Leitentscheidung 7). `true` auch, wenn
    /// kein eigener Inhalt mehr liegt (fremder Copy, leeres Clipboard).
    /// Für die Logzeile (WP4, `workers.rs`).
    pub history_excluded: bool,
}

impl ClipboardReport {
    pub fn lost_save(&self) -> Vec<LostFormat> {
        self.snapshot.lost()
    }
}

/// Der `restore …`-Teil der Paste-Logzeile (Leitentscheidung 9):
/// `true (restored)`, `partial (Sichern: …; Zurückschreiben: …)` bzw.
/// `false (<Grund>)`.
pub fn restore_log(restore: RestoreDecision, clipboard: &ClipboardReport) -> String {
    match restore {
        RestoreDecision::Restored => "true (restored)".into(),
        RestoreDecision::RestoredPartial { .. } => format!(
            "partial ({})",
            formats::partial_detail(&clipboard.lost_save(), &clipboard.lost_restore)
        ),
        RestoreDecision::RestoreFailed if !clipboard.lost_restore.is_empty() => format!(
            "false ({}; Zurückschreiben: {})",
            restore.as_str(),
            formats::lost_list(&clipboard.lost_restore)
        ),
        other => format!("false ({})", other.as_str()),
    }
}

/// Ausgang der Clipboard-Sicherung im Quit-Pfad (`save_targets`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardSave {
    /// Diktier hält das Clipboard nicht (mehr) — nichts zu sichern.
    NotOwner,
    /// Diktier hatte ein offenes Versprechen, aber das Clipboard wurde
    /// inzwischen fremd geändert. Ungeklärt, ob das Transkript noch irgendwo
    /// liegt (Final-Review Blocker 2) — anders als [`Self::NotOwner`] eine
    /// Warnung.
    PromiseForeign,
    /// Niemand da, der den Inhalt übernehmen könnte (Trait-Default/Stub).
    NoManager,
    /// Der Inhalt ist gesichert.
    Saved,
    /// Die Übernahme wurde abgelehnt.
    Refused,
    /// Keine Antwort innerhalb der Frist.
    Timeout,
}

impl ClipboardSave {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotOwner => "kein Clipboard-Eigentum",
            Self::PromiseForeign => "fremde Änderung bei offenem Versprechen",
            Self::NoManager => "kein Clipboard-Manager",
            Self::Saved => "an den Clipboard-Manager übergeben",
            Self::Refused => "vom Clipboard-Manager abgelehnt",
            Self::Timeout => "keine Antwort des Inject-Workers innerhalb der Frist",
        }
    }

    pub fn saved(self) -> bool {
        self == Self::Saved
    }
}

/// Verbleib des eigenen Transkripts am Ende eines Laufs (Final-Review
/// Blocker 1/2). Kommt aus dem Inject-Ausgang statt aus `eprintln!`, damit der
/// Worker es über den Daemon-Logger meldet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptState {
    /// Kein offenes eigenes Versprechen: eager hinterlegt, restauriert, oder
    /// ein fremder Copy liegt darüber.
    Secured,
    /// Das Delayed-Rendering-Versprechen ist noch offen — nichts angefasst
    /// (Clipboard blockiert, `EmptyClipboard` ohne Änderung) oder nach
    /// gescheitertem eager Setzen neu versprochen. Der Inject-Worker versucht
    /// es im Idle erneut.
    PromiseOpen(String),
    /// Weder Versprechen noch Daten: die Zwischenablage ist leer.
    Lost(String),
}

impl TranscriptState {
    /// Anfang der Fehlermeldung bei [`Self::Lost`] (Tray `error`).
    pub const LOST: &'static str = "Zwischenablage leer — Transkript verloren";

    /// `Zwischenablage leer — Transkript verloren (<Grund>)`.
    pub fn lost_message(detail: &str) -> String {
        format!("{} ({detail})", Self::LOST)
    }

    /// Kurzform für Log und Spike-Ausgabe.
    pub fn describe(&self) -> String {
        match self {
            Self::Secured => "gesichert".into(),
            Self::PromiseOpen(detail) => format!("Versprechen offen ({detail})"),
            Self::Lost(detail) => Self::lost_message(detail),
        }
    }
}

/// Ausgang von [`OutputSink::copy_only`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Copied {
    /// Der Verlaufsausschluss liegt dabei (Leitentscheidung 7).
    pub history_excluded: bool,
    pub transcript: TranscriptState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InjectOutcome {
    Pasted {
        restored: bool,
        shortcut: ResolvedShortcut,
        window: WindowId,
        wm_class: Option<(String, String)>,
        reads: u32,
        /// Endausgang. WP3 liest daraus den Hinweis: `RestoredPartial`
        /// samt `lost_on_restore`, `RestoreFailed`, `NoPromise`,
        /// `NoReadTimeout`.
        restore: RestoreDecision,
        clipboard: ClipboardReport,
        /// `Lost` bucht der Worker als Inject-Fehler, `PromiseOpen` als
        /// Warnung (Final-Review Blocker 1).
        transcript: TranscriptState,
    },
    CopyOnly {
        reason: CopyOnlyReason,
        /// Wie `ClipboardReport::history_excluded` (Final-Review, Hinweis
        /// Marker).
        history_excluded: bool,
        /// Der Snapshot-Report, falls der Snapshot vor dem Fokuswechsel schon
        /// lief (Leitentscheidung 3: Messgrundlage). `None` vor dem Snapshot.
        snapshot: Option<SnapshotReport>,
        transcript: TranscriptState,
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
    /// `Err` auch bei [`TranscriptState::Lost`] (Zwischenablage leer).
    fn copy_only(&mut self, text: &str) -> Result<Copied, InjectError>;
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
    /// Quit-Pfad (Spec §7.1 Punkt 8): den eigenen Clipboard-Inhalt vor dem
    /// Prozessende so sichern, dass er ihn überlebt — bis zur absoluten
    /// `deadline` (monoton, Nachkontrolle Blocker 2). Der Name stammt vom
    /// ICCCM-`SAVE_TARGETS`-Handshake, unter Windows ist es ein eager Render
    /// (siehe `windows::Win32OutputSink::save_to_clipboard_manager`).
    fn save_to_clipboard_manager(
        &mut self,
        _deadline: Instant,
    ) -> Result<ClipboardSave, InjectError> {
        Ok(ClipboardSave::NoManager)
    }
    /// Liegt noch ein offenes **eigenes** Versprechen (Owner und Sequenz
    /// geprüft)? Für den Idle-Retry im Inject-Worker (Final-Review Blocker 2).
    fn pending_promise(&mut self) -> bool {
        false
    }
    /// Ein offenes eigenes Versprechen eager hinterlegen; ein fremder Copy
    /// bleibt unberührt (`Secured`).
    fn materialize_pending(&mut self) -> TranscriptState {
        TranscriptState::Secured
    }
    /// Warnungen des Sinks seit dem letzten Aufruf (Marker-Registrierung,
    /// Marker nicht gesetzt, Restore unterblieben) — für den Daemon-Logger
    /// statt `eprintln!`.
    fn take_warnings(&mut self) -> Vec<String> {
        Vec::new()
    }
}

#[derive(Debug, Default)]
pub struct StubOutputSink;

impl OutputSink for StubOutputSink {
    fn paste(&mut self, _text: &str, _ctx: &CaptureContext) -> Result<InjectOutcome, InjectError> {
        Ok(InjectOutcome::CopyOnly {
            reason: CopyOnlyReason::FocusUnknown,
            history_excluded: true,
            snapshot: None,
            transcript: TranscriptState::Secured,
        })
    }

    fn copy_only(&mut self, _text: &str) -> Result<Copied, InjectError> {
        Ok(Copied {
            history_excluded: true,
            transcript: TranscriptState::Secured,
        })
    }
}

pub type PlatformSink = windows::Win32OutputSink;

pub fn new_sink(output: OutputConfig) -> Result<PlatformSink, InjectError> {
    windows::Win32OutputSink::new(output)
}

#[cfg(test)]
mod tests {
    use super::fake::{
        FakeContent, FakeFormat, FakeHost, KeyStroke, MaterializeFault, ScriptEvent, utf16_bytes,
    };
    use super::formats::{
        self, CF_BITMAP, CF_DIB, CF_DIBV5, CF_HDROP, CF_LOCALE, CF_OWNERDISPLAY, CF_TEXT,
        CF_UNICODETEXT, FormatRef, LossReason, Phase, RowOutcome, SnapshotKind,
    };
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

    /// Grund eines `CopyOnly`-Ausgangs; alles andere ist ein Testfehler.
    fn copy_reason(outcome: &InjectOutcome) -> CopyOnlyReason {
        match outcome {
            InjectOutcome::CopyOnly { reason, .. } => *reason,
            other => panic!("expected CopyOnly, got {other:?}"),
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

    /// Nur `Saved` heißt, dass ein Clipboard-Manager den Inhalt übernommen hat;
    /// jeder andere Ausgang bekommt im Quit-Log eine eigene Begründung (§7.1 P8).
    #[test]
    fn clipboard_save_outcomes_are_distinguishable() {
        assert!(ClipboardSave::Saved.saved());
        for other in [
            ClipboardSave::NotOwner,
            ClipboardSave::PromiseForeign,
            ClipboardSave::NoManager,
            ClipboardSave::Refused,
            ClipboardSave::Timeout,
        ] {
            assert!(!other.saved(), "{other:?}");
            assert_ne!(other.as_str(), ClipboardSave::Saved.as_str());
        }
        let mut sink = StubOutputSink;
        assert_eq!(
            sink.save_to_clipboard_manager(Instant::now()).unwrap(),
            ClipboardSave::NoManager
        );
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
                assert_eq!(restore, RestoreDecision::Restored);
            }
            other => panic!("expected Pasted, got {other:?}"),
        }
        assert_eq!(host.clipboard_text().as_deref(), Some("vorher"));
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
        assert_eq!(host.clipboard_text().as_deref(), Some("transkript"));
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
        assert_eq!(host.clipboard_text().as_deref(), Some("fremd"));
        assert!(!host.still_owner().unwrap());
    }

    /// windows-plan Leitentscheidung 4: Der **eigene** `WM_RENDERFORMAT` erhöht
    /// `GetClipboardSequenceNumber()`. Ein „Sequenz unverändert“-Test würde
    /// deshalb jeden legitimen Restore verwerfen; die eigene Generation wandert
    /// mit, der Restore findet statt.
    #[test]
    fn own_render_bumps_the_sequence_and_keeps_ownership() {
        let mut host = FakeHost::new()
            .with_text("vorher")
            .with_script(vec![(Duration::from_millis(10), ScriptEvent::OwnRender)]);
        let outcome = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        match outcome {
            InjectOutcome::Pasted {
                restored,
                reads,
                restore,
                ..
            } => {
                assert!(restored);
                assert_eq!(reads, 1);
                assert_eq!(restore, RestoreDecision::Restored);
            }
            other => panic!("expected Pasted, got {other:?}"),
        }
        assert_eq!(host.clipboard_text().as_deref(), Some("vorher"));
        assert!(host.still_owner().unwrap());
    }

    /// Fremder Copy **vor** dem Render: der Render kann danach nicht mehr
    /// greifen, es gibt keinen bedienten Read, und restauriert wird nie
    /// (§7.1 Punkt 5).
    #[test]
    fn foreign_copy_before_the_render_is_foreign_owner() {
        let mut host = FakeHost::new().with_text("vorher").with_script(vec![
            (
                Duration::from_millis(10),
                ScriptEvent::ForeignTakeover(FakeContent::Text("fremd".into())),
            ),
            (Duration::from_millis(20), ScriptEvent::OwnRender),
        ]);
        let outcome = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        match outcome {
            InjectOutcome::Pasted {
                restored,
                reads,
                restore,
                ..
            } => {
                assert!(!restored);
                assert_eq!(reads, 0);
                assert_eq!(restore, RestoreDecision::ForeignOwner);
            }
            other => panic!("expected Pasted, got {other:?}"),
        }
        assert_eq!(host.clipboard_text().as_deref(), Some("fremd"));
        assert!(!host.still_owner().unwrap());
    }

    /// Owner ist weiter unser Fenster, die Sequenznummer stammt aber von einer
    /// fremden Mutation — `still_owner` muss das erkennen, obwohl kein
    /// `lost_ownership`-Ereignis kam (windows-plan `WM_DESTROYCLIPBOARD`-Guard).
    #[test]
    fn same_owner_with_foreign_sequence_never_restores() {
        let mut host = FakeHost::new().with_text("vorher").with_script(vec![
            (Duration::from_millis(10), ScriptEvent::OwnRender),
            (Duration::from_millis(20), ScriptEvent::ForeignSequenceBump),
        ]);
        let outcome = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        match outcome {
            InjectOutcome::Pasted {
                restored,
                reads,
                restore,
                ..
            } => {
                assert!(!restored);
                assert_eq!(reads, 1);
                assert_eq!(restore, RestoreDecision::ForeignOwner);
            }
            other => panic!("expected Pasted, got {other:?}"),
        }
        // Das Transkript bleibt liegen — §7.1: niemals über eine fremde
        // Änderung hinweg restaurieren.
        assert_eq!(host.clipboard_text().as_deref(), Some("transkript"));
        assert!(!host.still_owner().unwrap());
    }

    /// `SendInput` kann wegen UIPI Events verschlucken. Dann darf keine Taste
    /// unten bleiben — auch nicht beim längeren Ctrl+Shift+V-Chord.
    #[test]
    fn ctrl_shift_v_failure_releases_every_key_it_pressed() {
        for fail_at in 1..=3 {
            let mut host = FakeHost::new()
                .with_text("vorher")
                .with_wm_class("WindowsTerminal.exe", "WindowsTerminal.exe")
                .with_fail_key_after(fail_at);
            assert!(inject_paste(&mut host, "transkript", &ctx(1), &output()).is_err());
            let downs: Vec<_> = host.sent.iter().filter(|s| s.down).map(|s| s.key).collect();
            let ups: Vec<_> = host
                .sent
                .iter()
                .filter(|s| !s.down)
                .map(|s| s.key)
                .collect();
            for key in downs {
                assert!(
                    ups.contains(&key),
                    "Taste {key:?} ohne Up (fail_at {fail_at})"
                );
            }
        }
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
        assert_eq!(host.clipboard_text().as_deref(), Some("transkript"));
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
        assert_eq!(copy_reason(&outcome), CopyOnlyReason::FocusUnknown);
        assert_eq!(host.clipboard_text().as_deref(), Some("transkript"));

        let mut host = FakeHost::new()
            .with_text("vorher")
            .with_window(Some(WindowId(1)));
        let blank = CaptureContext {
            start_window_id: None,
            target_window_id: Some(WindowId(1)),
            ended_at: Instant::now(),
        };
        let outcome = inject_paste(&mut host, "transkript", &blank, &output()).unwrap();
        assert_eq!(copy_reason(&outcome), CopyOnlyReason::FocusUnknown);
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
        assert_eq!(copy_reason(&outcome), CopyOnlyReason::FocusChanged);
        assert_eq!(host.clipboard_text().as_deref(), Some("transkript"));
        assert!(host.sent.is_empty());
    }

    #[test]
    fn restore_session_wait_then_restore() {
        let session = RestoreSession::new(
            &ClipboardSnapshot::new(SnapshotKind::Formats, SnapshotReport::empty(false)),
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
        assert_eq!(host.clipboard_text().as_deref(), Some("vorher"));
        host.queue_clear(FakeContent::Text("fremd".into()));
        let outcome = inject_paste(&mut host, "zwei", &ctx(1), &output()).unwrap();
        match outcome {
            InjectOutcome::Pasted {
                restored, restore, ..
            } => {
                assert!(restored);
                assert_eq!(restore, RestoreDecision::Restored);
            }
            other => panic!("expected restore of foreign snapshot, got {other:?}"),
        }
        assert_eq!(host.clipboard_text().as_deref(), Some("fremd"));
    }

    #[test]
    fn takeover_before_empty_restore_keeps_foreign() {
        let mut host = FakeHost::new()
            .with_script(vec![(
                Duration::from_millis(10),
                ScriptEvent::SelectionRequest,
            )])
            .with_foreign_copy_before_restore(FakeContent::Text("fremd".into()));
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
        assert_eq!(host.clipboard_text().as_deref(), Some("fremd"));
    }

    #[test]
    fn focus_change_during_snapshot_is_copy_only() {
        let mut host = FakeHost::new()
            .with_text("vorher")
            .with_focus_after_snapshot(Some(WindowId(2)));
        let outcome = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        assert_eq!(copy_reason(&outcome), CopyOnlyReason::FocusChanged);
        assert_eq!(host.clipboard_text().as_deref(), Some("transkript"));
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
        assert_eq!(host.clipboard_text().as_deref(), Some("transkript"));
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
        assert_eq!(host.clipboard_text().as_deref(), Some(" Hi"));
    }

    // ------------------------------------------- WP1: Mehrformat-Restore

    const HTML: &str = "HTML Format";
    const HTML_ID: u32 = 0xC0A1;

    fn read_at_10ms() -> Vec<(Duration, ScriptEvent)> {
        vec![(Duration::from_millis(10), ScriptEvent::SelectionRequest)]
    }

    fn pasted(outcome: InjectOutcome) -> (RestoreDecision, ClipboardReport) {
        match outcome {
            InjectOutcome::Pasted {
                restore, clipboard, ..
            } => (restore, clipboard),
            other => panic!("expected Pasted, got {other:?}"),
        }
    }

    /// Word/Browser-typisch: Text, Locale, synthetisiertes CF_TEXT, HTML,
    /// DIBV5 plus GDI-Bitmap und OLE-Zeiger.
    fn office_formats() -> Vec<FakeFormat> {
        vec![
            FakeFormat::text("vorher"),
            FakeFormat::new(CF_LOCALE, 0x0407_u32.to_le_bytes()),
            FakeFormat::new(CF_TEXT, b"vorher\0".to_vec()),
            FakeFormat::named(HTML_ID, HTML, b"<b>vorher</b>".to_vec()),
            FakeFormat::new(CF_DIBV5, vec![7; 124]),
            FakeFormat::new(CF_BITMAP, Vec::new()),
            FakeFormat::named(0xC010, "DataObject", vec![0; 8]),
        ]
    }

    #[test]
    fn full_restore_keeps_order_and_bytes() {
        let mut host = FakeHost::new()
            .with_formats(office_formats())
            .with_script(read_at_10ms());
        let (restore, report) =
            pasted(inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap());
        // „Synthetisch ersetzt“ und „OLE-Verweis entfällt“ machen nichts
        // partiell (Leitentscheidung 8: kein Hinweis).
        assert_eq!(restore, RestoreDecision::Restored);
        assert!(restore.is_restored());
        assert!(report.lost_save().is_empty());
        assert!(report.lost_restore.is_empty());
        assert_eq!(
            report.snapshot.replaced(),
            vec![FormatRef::new(CF_BITMAP, None)]
        );
        assert_eq!(
            report.snapshot.ole_dropped(),
            vec![FormatRef::new(0xC010, Some("DataObject"))]
        );
        let originals = office_formats();
        let expected: Vec<(u32, Vec<u8>)> = originals
            .iter()
            .filter(|f| ![CF_BITMAP, 0xC010].contains(&f.id))
            .map(|f| (f.id, f.data.clone()))
            .collect();
        let got: Vec<(u32, Vec<u8>)> = host
            .clipboard_formats()
            .into_iter()
            .map(|f| (f.id, f.data))
            .collect();
        assert_eq!(got, expected, "IDs, Reihenfolge und Bytes");
        assert!(
            host.clipboard.excluded,
            "Restore trägt den Verlaufsausschluss"
        );
        assert!(host.still_owner().unwrap());
    }

    #[test]
    fn partial_in_save_phase() {
        let mut formats = office_formats();
        formats.push(FakeFormat::new(CF_OWNERDISPLAY, Vec::new()));
        formats[3] = FakeFormat::named(HTML_ID, HTML, b"x".to_vec()).failing_read();
        let mut host = FakeHost::new()
            .with_formats(formats)
            .with_script(read_at_10ms());
        let (restore, report) =
            pasted(inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap());
        assert_eq!(
            restore,
            RestoreDecision::RestoredPartial {
                lost_on_restore: false
            }
        );
        let lost: Vec<_> = report
            .lost_save()
            .into_iter()
            .map(|l| (l.format.id, l.reason, l.phase))
            .collect();
        assert_eq!(
            lost,
            vec![
                (HTML_ID, LossReason::NoData, Phase::Save),
                (CF_OWNERDISPLAY, LossReason::NeverCopyable, Phase::Save),
            ]
        );
        assert!(report.lost_restore.is_empty());
        assert!(!host.clipboard_ids().contains(&HTML_ID));
        assert_eq!(host.clipboard_text().as_deref(), Some("vorher"));
        assert_eq!(
            restore_log(restore, &report),
            "partial (Sichern: 0xC0A1 \"HTML Format\" [keine Daten], 0x0080 [nie kopierbar])"
        );
    }

    #[test]
    fn partial_in_restore_phase() {
        let mut formats = office_formats();
        formats[3] = FakeFormat::named(HTML_ID, HTML, b"<i>x</i>".to_vec()).failing_set();
        let mut host = FakeHost::new()
            .with_formats(formats)
            .with_script(read_at_10ms());
        let (restore, report) =
            pasted(inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap());
        assert_eq!(
            restore,
            RestoreDecision::RestoredPartial {
                lost_on_restore: true
            }
        );
        assert!(report.lost_save().is_empty());
        assert_eq!(report.lost_restore.len(), 1);
        assert_eq!(report.lost_restore[0].format.id, HTML_ID);
        assert_eq!(report.lost_restore[0].phase, Phase::Restore);
        assert!(matches!(
            report.lost_restore[0].reason,
            LossReason::SetFailed(_)
        ));
        assert_eq!(
            host.clipboard_ids(),
            vec![CF_UNICODETEXT, CF_LOCALE, CF_TEXT, CF_DIBV5]
        );
        assert_eq!(
            restore_log(restore, &report),
            "partial (Zurückschreiben: 0xC0A1 \"HTML Format\" [SetClipboardData 5])"
        );
    }

    /// Plan WP1: alle Nutzformate scheitern beim Setzen → Transkript liegt.
    #[test]
    fn all_useful_formats_fail_to_set_leaves_the_transcript() {
        let mut host = FakeHost::new()
            .with_formats(vec![
                FakeFormat::text("vorher").failing_set(),
                FakeFormat::new(CF_LOCALE, 0x0407_u32.to_le_bytes()),
                FakeFormat::named(HTML_ID, HTML, b"x".to_vec()).failing_set(),
            ])
            .with_script(read_at_10ms());
        let (restore, report) =
            pasted(inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap());
        assert_eq!(restore, RestoreDecision::RestoreFailed);
        assert!(!restore.is_restored());
        assert_eq!(report.lost_restore.len(), 2);
        assert_eq!(host.clipboard_text().as_deref(), Some("transkript"));
        assert!(host.clipboard.excluded);
        assert!(
            restore_log(restore, &report)
                .starts_with("false (Zwischenablage nicht wiederhergestellt")
        );
    }

    /// Auch das Fallback scheitert → Inject-Fehler (Tray `error`).
    #[test]
    fn failing_fallback_is_an_inject_error() {
        let mut host = FakeHost::new()
            .with_formats(vec![FakeFormat::text("vorher").failing_set()])
            .with_failing_fallback()
            .with_script(read_at_10ms());
        let err = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap_err();
        assert!(err.to_string().contains("Zwischenablage leer"), "{err}");
        assert!(host.clipboard_ids().is_empty());
    }

    /// Nur Begleitformate gesichert → `Unrestorable`, kein Versprechen.
    #[test]
    fn companions_only_is_unrestorable() {
        let mut host = FakeHost::new()
            .with_formats(vec![
                FakeFormat::new(CF_LOCALE, 0x0407_u32.to_le_bytes()),
                FakeFormat::named(0xC0B0, "Preferred DropEffect", 1_u32.to_le_bytes()),
                FakeFormat::new(CF_BITMAP, Vec::new()),
                FakeFormat::named(0xC0B1, "FileContents", vec![1]).failing_read(),
            ])
            .with_script(read_at_10ms());
        let (restore, report) =
            pasted(inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap());
        assert_eq!(restore, RestoreDecision::NoPromise);
        assert_eq!(report.snapshot.saved_count(), 2);
        assert_eq!(host.clipboard_text().as_deref(), Some("transkript"));
        assert!(host.clipboard.excluded);
    }

    #[test]
    fn empty_snapshot_restores_an_empty_clipboard() {
        let mut host = FakeHost::new().with_script(read_at_10ms());
        let (restore, report) =
            pasted(inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap());
        assert_eq!(restore, RestoreDecision::Restored);
        assert!(report.snapshot.rows.is_empty());
        assert!(host.clipboard_ids().is_empty());
        assert_eq!(host.clipboard_text(), None);
        // Wirklich leer — auch ohne Verlaufsmarker.
        assert!(!host.clipboard.excluded);
        assert!(host.still_owner().unwrap());
    }

    /// Fremder Copy zwischen Vorbereitung und `OpenClipboard` → `Foreign`,
    /// nichts angefasst.
    #[test]
    fn foreign_copy_during_preparation_is_foreign() {
        let mut host = FakeHost::new()
            .with_formats(office_formats())
            .with_foreign_copy_before_restore(FakeContent::Text("fremd".into()))
            .with_script(read_at_10ms());
        let (restore, _) =
            pasted(inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap());
        assert_eq!(restore, RestoreDecision::ForeignOwner);
        assert_eq!(host.clipboard_text().as_deref(), Some("fremd"));
        assert!(!host.still_owner().unwrap());
    }

    /// Bestand, auf Mehrformat-Inhalt umgestellt: fremde Änderung während
    /// der Wartezeit → nie restaurieren.
    #[test]
    fn foreign_change_during_wait_never_restores_formats() {
        let mut host = FakeHost::new()
            .with_formats(office_formats())
            .with_script(vec![
                (Duration::from_millis(10), ScriptEvent::SelectionRequest),
                (
                    Duration::from_millis(50),
                    ScriptEvent::ForeignTakeover(FakeContent::Formats(vec![FakeFormat::new(
                        CF_HDROP,
                        vec![1, 2, 3],
                    )])),
                ),
            ]);
        let (restore, _) =
            pasted(inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap());
        assert_eq!(restore, RestoreDecision::ForeignOwner);
        assert_eq!(host.clipboard_ids(), vec![CF_HDROP]);
    }

    /// Leitentscheidung 6, Folge 1: nach einem (partiellen) Restore sind
    /// genau die platzierten Formate der nächste Snapshot, ohne Read.
    #[test]
    fn own_content_after_restore_is_the_next_snapshot() {
        let mut formats = office_formats();
        formats[3] = FakeFormat::named(HTML_ID, HTML, b"x".to_vec()).failing_set();
        let mut host = FakeHost::new()
            .with_formats(formats)
            .with_script(read_at_10ms());
        inject_paste(&mut host, "eins", &ctx(1), &output()).unwrap();
        let placed = host.clipboard_formats();
        let reads_before = host.snapshot_reads;

        let outcome = inject_paste(&mut host, "zwei", &ctx(1), &output()).unwrap();
        let (restore, report) = pasted(outcome.clone());
        assert!(report.snapshot.own);
        assert_eq!(
            host.snapshot_reads, reads_before,
            "kein eigenes GetClipboardData"
        );
        let snapshot_ids: Vec<u32> = report.snapshot.rows.iter().map(|r| r.format.id).collect();
        assert_eq!(
            snapshot_ids,
            placed.iter().map(|f| f.id).collect::<Vec<_>>()
        );
        // Die eigene Payload ist vollständig: diesmal nichts verloren.
        assert_eq!(restore, RestoreDecision::Restored);
        assert_eq!(host.clipboard_formats(), placed);
        match outcome {
            InjectOutcome::Pasted { reads, .. } => assert_eq!(reads, 1, "nur der Script-Read"),
            other => panic!("{other:?}"),
        }
    }

    /// Folge 2: nach `NoReadTimeout` ist das Transkript der nächste Snapshot.
    #[test]
    fn own_content_after_no_read_timeout_is_the_transcript() {
        let mut host = FakeHost::new().with_formats(office_formats());
        let (restore, _) = pasted(inject_paste(&mut host, "eins", &ctx(1), &output()).unwrap());
        assert_eq!(restore, RestoreDecision::NoReadTimeout);
        assert!(host.clipboard.excluded);
        let reads_before = host.snapshot_reads;

        let mut host = host.with_script(read_at_10ms());
        let (restore, report) =
            pasted(inject_paste(&mut host, "zwei", &ctx(1), &output()).unwrap());
        assert!(report.snapshot.own);
        assert_eq!(host.snapshot_reads, reads_before);
        assert_eq!(report.snapshot.rows.len(), 1);
        assert_eq!(report.snapshot.rows[0].format.id, CF_UNICODETEXT);
        assert_eq!(restore, RestoreDecision::Restored);
        assert_eq!(host.clipboard_text().as_deref(), Some("eins"));
        assert_eq!(host.clipboard_formats()[0].data, utf16_bytes("eins"));
    }

    /// Folge 3: nach `CopyOnly` ist das Transkript der nächste Snapshot;
    /// `CopyOnly` selbst hat keinen Snapshot und keine Restore-Zusage.
    #[test]
    fn own_content_after_copy_only_is_the_transcript() {
        let mut host = FakeHost::new()
            .with_formats(office_formats())
            .with_window(Some(WindowId(2)));
        let outcome = inject_paste(&mut host, "eins", &ctx(1), &output()).unwrap();
        assert!(matches!(outcome, InjectOutcome::CopyOnly { .. }));
        assert_eq!(host.snapshot_reads, 0, "CopyOnly ohne Snapshot");
        assert!(host.clipboard.excluded);

        let mut host = host
            .with_window(Some(WindowId(1)))
            .with_script(read_at_10ms());
        let (restore, report) =
            pasted(inject_paste(&mut host, "zwei", &ctx(1), &output()).unwrap());
        assert!(report.snapshot.own);
        assert_eq!(host.snapshot_reads, 0);
        assert_eq!(restore, RestoreDecision::Restored);
        assert_eq!(host.clipboard_text().as_deref(), Some("eins"));
    }

    #[test]
    fn byte_budget_makes_the_restore_partial() {
        let mut host = FakeHost::new()
            .with_formats(vec![
                FakeFormat::text("vorher"),
                FakeFormat::new(CF_DIB, vec![0; 100]),
                FakeFormat::named(HTML_ID, HTML, b"<p/>".to_vec()),
            ])
            .with_budget(64, Duration::from_secs(1))
            .with_script(read_at_10ms());
        let (restore, report) =
            pasted(inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap());
        assert_eq!(
            restore,
            RestoreDecision::RestoredPartial {
                lost_on_restore: false
            }
        );
        let lost = report.lost_save();
        assert_eq!(lost.len(), 1);
        assert_eq!(
            (lost[0].format.id, lost[0].reason),
            (CF_DIB, LossReason::ByteBudget)
        );
        assert_eq!(host.clipboard_ids(), vec![CF_UNICODETEXT, HTML_ID]);
    }

    #[test]
    fn time_budget_makes_the_restore_partial() {
        let mut host = FakeHost::new()
            .with_formats(vec![
                FakeFormat::text("vorher"),
                FakeFormat::named(HTML_ID, HTML, b"<p/>".to_vec()).costing(Duration::from_secs(3)),
                FakeFormat::new(CF_DIB, vec![0; 10]),
                FakeFormat::new(CF_BITMAP, Vec::new()),
            ])
            .with_script(read_at_10ms());
        let (restore, report) =
            pasted(inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap());
        assert_eq!(
            restore,
            RestoreDecision::RestoredPartial {
                lost_on_restore: false
            }
        );
        let outcomes: Vec<RowOutcome> = report.snapshot.rows.iter().map(|r| r.outcome).collect();
        assert_eq!(outcomes[2], RowOutcome::Lost(LossReason::TimeBudget));
        assert_eq!(outcomes[3], RowOutcome::Lost(LossReason::NoCounterpart));
        assert_eq!(report.snapshot.duration, Duration::from_secs(3));
        // Das langsame HTML war schon angefordert und bleibt gesichert.
        assert_eq!(host.clipboard_ids(), vec![CF_UNICODETEXT, HTML_ID]);
    }

    /// Plan B6: Fokuswechsel zwischen `become_owner` und dem ersten
    /// Key-Event → `CopyOnly`, kein Chord.
    #[test]
    fn focus_change_after_become_owner_is_copy_only() {
        let mut host = FakeHost::new()
            .with_formats(office_formats())
            .with_focus_after_become_owner(Some(WindowId(2)))
            .with_script(read_at_10ms());
        let outcome = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        assert_eq!(copy_reason(&outcome), CopyOnlyReason::FocusChanged);
        assert!(host.sent.is_empty(), "kein einziges Key-Event");
        assert_eq!(host.clipboard_text().as_deref(), Some("transkript"));
        assert!(host.clipboard.excluded);

        // Fenster weg (NULL) zählt als unbekannt.
        let mut host = FakeHost::new()
            .with_text("vorher")
            .with_focus_after_become_owner(None);
        let outcome = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        assert_eq!(copy_reason(&outcome), CopyOnlyReason::FocusUnknown);
        assert!(host.sent.is_empty());
    }

    #[test]
    fn restore_log_forms() {
        let report = ClipboardReport {
            snapshot: SnapshotReport::empty(false),
            lost_restore: Vec::new(),
            history_excluded: true,
        };
        assert_eq!(
            restore_log(RestoreDecision::Restored, &report),
            "true (restored)"
        );
        assert_eq!(
            restore_log(RestoreDecision::NoReadTimeout, &report),
            "false (Einfügen nicht bestätigt — Text liegt in der Zwischenablage)"
        );
        assert_eq!(
            restore_log(RestoreDecision::NoPromise, &report),
            "false (Zwischenablage nicht gesichert — Transkript liegt in der Zwischenablage)"
        );
        assert!(!RestoreDecision::ForeignOwner.is_restored());
        assert!(
            RestoreDecision::RestoredPartial {
                lost_on_restore: true
            }
            .is_restored()
        );
    }

    // ------------------------------------- Nacharbeit: Snapshot-Fehler

    /// `CountClipboardFormats`/`EnumClipboardFormats` mit echtem Fehlercode:
    /// kein Abbruch, `Unrestorable` mit Verlust „Snapshot-Fehler <code>“, der
    /// Paste läuft ohne Restore-Versprechen weiter.
    #[test]
    fn enumeration_error_is_unrestorable_and_pastes_anyway() {
        // ERROR_CLIPBOARD_NOT_OPEN
        let mut host = FakeHost::new()
            .with_formats(office_formats())
            .with_snapshot_failure(1418)
            .with_script(read_at_10ms());
        let (restore, report) =
            pasted(inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap());
        assert_eq!(restore, RestoreDecision::NoPromise);
        assert_eq!(report.snapshot.failure(), Some(1418));
        let lost = report.lost_save();
        assert_eq!(lost.len(), 1);
        assert_eq!(lost[0].reason, LossReason::SnapshotFailed(1418));
        assert_eq!(lost[0].reason.as_str(), "Snapshot-Fehler 1418");
        assert!(has_stroke(&host.sent, PasteKey::V, true), "Paste lief");
        assert_eq!(host.clipboard_text().as_deref(), Some("transkript"));
        assert_eq!(host.snapshot_reads, 0);
        assert!(
            formats::snapshot_log_line(&report.snapshot).contains("[Snapshot-Fehler 1418]"),
            "{}",
            formats::snapshot_log_line(&report.snapshot)
        );
    }

    /// `OpenClipboard` scheitert schon im Snapshot: ebenfalls `Unrestorable`;
    /// danach entscheidet `become_owner` wie bisher.
    #[test]
    fn open_failure_in_snapshot_is_unrestorable() {
        // ERROR_ACCESS_DENIED
        let mut host = FakeHost::new()
            .with_text("vorher")
            .with_snapshot_failure(5)
            .with_script(read_at_10ms());
        let (restore, report) =
            pasted(inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap());
        assert_eq!(restore, RestoreDecision::NoPromise);
        assert_eq!(report.snapshot.failure(), Some(5));
        assert_eq!(host.clipboard_text().as_deref(), Some("transkript"));
        assert!(host.clipboard.excluded);
    }

    /// Scheitert auch `become_owner`, bleibt es ein Inject-Fehler.
    #[test]
    fn open_failure_in_snapshot_and_become_owner_is_an_error() {
        let mut host = FakeHost::new()
            .with_text("vorher")
            .with_snapshot_failure(5)
            .with_failing_become_owner();
        let err = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap_err();
        assert!(err.to_string().contains("nicht zu öffnen"), "{err}");
        assert!(host.sent.is_empty(), "kein Chord ohne Transkript");
        assert_eq!(host.clipboard_text().as_deref(), Some("vorher"));
    }

    // --------------------------- Nacharbeit 2 (Sol-Impl-Review)

    /// Blocker 1: Snapshot-Open scheitert → fremder Copy → Take-Open gelingt.
    /// Die beim Snapshot gelesene Sequenz schützt den fremden Inhalt.
    #[test]
    fn foreign_copy_after_failed_snapshot_open_is_not_overwritten() {
        let mut host = FakeHost::new()
            .with_text("vorher")
            .with_snapshot_failure(5)
            .with_foreign_copy_before_become_owner(FakeContent::Text("fremd".into()))
            .with_script(read_at_10ms());
        let err = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap_err();
        assert!(err.to_string().contains("fremd geändert"), "{err}");
        assert_eq!(host.clipboard_text().as_deref(), Some("fremd"));
        assert!(host.sent.is_empty(), "kein Chord");
        assert!(!host.still_owner().unwrap());
    }

    /// Blocker 2: nach `NoReadTimeout` ist das Transkript eager, die eigene
    /// Generation fortgeschrieben, und das Folgediktat sieht die eigene
    /// Payload ohne Read-Zählung.
    #[test]
    fn no_read_timeout_materializes_the_transcript() {
        let mut host = FakeHost::new().with_formats(office_formats());
        let (restore, report) =
            pasted(inject_paste(&mut host, "eins", &ctx(1), &output()).unwrap());
        assert_eq!(restore, RestoreDecision::NoReadTimeout);
        assert!(!host.clipboard.delayed, "kein offenes Versprechen mehr");
        assert_eq!(host.materializations, 1);
        assert!(host.still_owner().unwrap(), "Sequenz fortgeschrieben");
        assert!(host.clipboard.excluded);
        assert!(report.history_excluded);

        let reads_before = host.snapshot_reads;
        let mut host = host.with_script(read_at_10ms());
        let outcome = inject_paste(&mut host, "zwei", &ctx(1), &output()).unwrap();
        let (restore, report) = pasted(outcome.clone());
        assert!(report.snapshot.own);
        assert_eq!(host.snapshot_reads, reads_before);
        assert_eq!(restore, RestoreDecision::Restored);
        assert_eq!(host.clipboard_text().as_deref(), Some("eins"));
        match outcome {
            InjectOutcome::Pasted { reads, .. } => assert_eq!(reads, 1, "nur der Script-Read"),
            other => panic!("{other:?}"),
        }
    }

    /// `NoPromise` ohne Read: erst nach dem 5-s-Fenster eager, nicht mitten
    /// in das Einfügen hinein.
    #[test]
    fn no_promise_materializes_after_the_read_window() {
        let companions = || {
            vec![
                FakeFormat::new(CF_LOCALE, 0x0407_u32.to_le_bytes()),
                FakeFormat::new(CF_BITMAP, Vec::new()),
            ]
        };
        let mut host = FakeHost::new().with_formats(companions());
        let (restore, _) =
            pasted(inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap());
        assert_eq!(restore, RestoreDecision::NoPromise);
        assert!(!host.clipboard.delayed);
        assert_eq!(host.materializations, 1);
        assert!(
            host.elapsed() >= protocol::READ_TIMEOUT,
            "erst nach dem Fenster"
        );
        assert!(host.still_owner().unwrap());

        // Mit Read: der Render hat schon eager gemacht, nichts zu tun.
        let mut host = FakeHost::new()
            .with_formats(companions())
            .with_script(read_at_10ms());
        let outcome = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        assert!(!host.clipboard.delayed);
        assert_eq!(host.materializations, 0);
        assert!(host.elapsed() < protocol::READ_TIMEOUT);
        match outcome {
            InjectOutcome::Pasted { reads, restore, .. } => {
                assert_eq!(restore, RestoreDecision::NoPromise);
                assert_eq!(reads, 1);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn disabled_restore_materializes_too() {
        let mut cfg = output();
        cfg.restore_clipboard = false;
        let mut host = FakeHost::new().with_text("vorher");
        let (restore, _) = pasted(inject_paste(&mut host, "transkript", &ctx(1), &cfg).unwrap());
        assert_eq!(restore, RestoreDecision::Disabled);
        assert!(!host.clipboard.delayed);
        assert_eq!(host.clipboard_text().as_deref(), Some("transkript"));
        assert!(host.still_owner().unwrap());
    }

    /// `CopyOnly` setzt direkt eager — keine Materialisierung nötig.
    #[test]
    fn copy_only_sets_the_transcript_eager() {
        let mut host = FakeHost::new()
            .with_formats(office_formats())
            .with_window(Some(WindowId(2)));
        let outcome = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        assert!(matches!(outcome, InjectOutcome::CopyOnly { .. }));
        assert!(!host.clipboard.delayed);
        assert_eq!(host.materializations, 0);
        assert!(host.clipboard.excluded);
        assert!(host.still_owner().unwrap());

        // Fokuswechsel nach dem Snapshot: ebenfalls direkt eager.
        let mut host = FakeHost::new()
            .with_formats(office_formats())
            .with_focus_after_snapshot(Some(WindowId(2)));
        inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        assert!(!host.clipboard.delayed);
        assert_eq!(host.materializations, 0);
    }

    /// Fokuswechsel nach `become_owner`: das schon gesetzte Versprechen wird
    /// sofort eager.
    #[test]
    fn focus_change_after_become_owner_materializes() {
        let mut host = FakeHost::new()
            .with_formats(office_formats())
            .with_focus_after_become_owner(Some(WindowId(2)));
        inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        assert!(!host.clipboard.delayed);
        assert_eq!(host.materializations, 1);
        assert!(host.still_owner().unwrap());
    }

    #[test]
    fn foreign_copy_before_materialization_is_left_alone() {
        let mut host = FakeHost::new()
            .with_formats(office_formats())
            .with_foreign_copy_before_materialize(FakeContent::Text("fremd".into()));
        let (restore, report) =
            pasted(inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap());
        assert_eq!(restore, RestoreDecision::NoReadTimeout);
        assert_eq!(host.materializations, 0);
        assert_eq!(host.clipboard_text().as_deref(), Some("fremd"));
        assert!(!host.still_owner().unwrap());
        assert!(report.history_excluded, "kein eigener Inhalt mehr");
    }

    /// Clipboard blockiert: das Versprechen bleibt offen, der Quit-Pfad ist
    /// die zweite Chance.
    #[test]
    fn failed_materialization_keeps_the_promise() {
        let mut host = FakeHost::new()
            .with_formats(office_formats())
            .with_failing_materialize();
        let (restore, _) =
            pasted(inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap());
        assert_eq!(restore, RestoreDecision::NoReadTimeout);
        assert!(host.clipboard.delayed);
        assert!(host.still_owner().unwrap());
    }

    /// Hinweis Fehlerpfade: `EmptyClipboard` scheitert ohne Sequenzänderung →
    /// `RestoreFailed`, das Transkript liegt.
    #[test]
    fn empty_clipboard_failure_without_sequence_change_keeps_the_transcript() {
        let mut host = FakeHost::new()
            .with_formats(office_formats())
            .with_failing_empty(false)
            .with_script(read_at_10ms());
        let (restore, _) =
            pasted(inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap());
        assert_eq!(restore, RestoreDecision::RestoreFailed);
        assert_eq!(host.clipboard_text().as_deref(), Some("transkript"));
        assert!(host.still_owner().unwrap());
    }

    /// … mit Sequenzänderung ist der Zustand unbekannt → Inject-Fehler.
    #[test]
    fn empty_clipboard_failure_with_sequence_change_is_an_error() {
        let mut host = FakeHost::new()
            .with_formats(office_formats())
            .with_failing_empty(true)
            .with_script(read_at_10ms());
        let err = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap_err();
        assert!(err.to_string().contains("EmptyClipboard"), "{err}");
    }

    /// Marker-Ausfall: Restore trotzdem ok, aber `history_excluded == false`.
    #[test]
    fn marker_failure_still_restores() {
        let mut host = FakeHost::new()
            .with_formats(office_formats())
            .with_failing_marker()
            .with_script(read_at_10ms());
        let (restore, report) =
            pasted(inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap());
        assert_eq!(restore, RestoreDecision::Restored);
        assert!(!report.history_excluded);
        assert_eq!(host.clipboard_text().as_deref(), Some("vorher"));

        let mut host = FakeHost::new()
            .with_formats(office_formats())
            .with_script(read_at_10ms());
        let (_, report) =
            pasted(inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap());
        assert!(report.history_excluded);
    }

    // ----------------------------- Final-Review (Sol) Nacharbeit

    fn transcript_of(outcome: &InjectOutcome) -> TranscriptState {
        match outcome {
            InjectOutcome::Pasted { transcript, .. }
            | InjectOutcome::CopyOnly { transcript, .. } => transcript.clone(),
        }
    }

    /// Blocker 1: `EmptyClipboard` scheitert **während der Materialisierung**
    /// ohne Änderung → Eigentum und Versprechen bleiben, Ausgang
    /// `PromiseOpen`. Der Quit-Pfad erkennt das Versprechen danach noch als
    /// eigenes.
    #[test]
    fn empty_failure_during_materialization_keeps_promise_and_ownership() {
        let mut host = FakeHost::new()
            .with_formats(office_formats())
            .with_materialize_fault(MaterializeFault::EmptyFails, 1);
        let outcome = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        assert!(
            matches!(transcript_of(&outcome), TranscriptState::PromiseOpen(ref d) if d.contains("EmptyClipboard")),
            "{outcome:?}"
        );
        let (restore, _) = pasted(outcome);
        assert_eq!(restore, RestoreDecision::NoReadTimeout);
        assert!(host.clipboard.delayed, "Versprechen offen");
        assert!(host.still_owner().unwrap(), "Eigentum behalten");
        assert!(host.promise_recorded());
        // Zweite Chance (Quit): jetzt gelingt es.
        assert_eq!(
            protocol::save_transcript_on_quit(&mut host, Instant::now()).unwrap(),
            ClipboardSave::Saved
        );
        assert!(!host.clipboard.delayed);
        assert_eq!(host.clipboard_text().as_deref(), Some("transkript"));
    }

    /// Blocker 1: Nach `EmptyClipboard` scheitert das eager `SetClipboardData`
    /// → noch im geöffneten Clipboard neu versprochen: `PromiseOpen`,
    /// `delayed`, weiter Owner, Marker dabei.
    #[test]
    fn set_failure_during_materialization_promises_again() {
        let mut host = FakeHost::new()
            .with_formats(office_formats())
            .with_materialize_fault(MaterializeFault::SetFails, 1);
        let outcome = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        assert!(
            matches!(transcript_of(&outcome), TranscriptState::PromiseOpen(ref d) if d.contains("erneut versprochen")),
            "{outcome:?}"
        );
        let (_, report) = pasted(outcome);
        assert!(report.history_excluded);
        assert!(host.clipboard.delayed);
        assert!(
            host.still_owner().unwrap(),
            "eigene Generation fortgeschrieben"
        );
        assert_eq!(host.clipboard_text().as_deref(), Some("transkript"));
        assert!(host.clipboard.excluded);
    }

    /// Blocker 1: Scheitert auch das Rückfall-Versprechen, ist das Transkript
    /// verloren — ausdrücklich im Ausgang, nicht nur auf stderr.
    #[test]
    fn set_and_promise_failure_during_materialization_is_lost() {
        let mut host = FakeHost::new()
            .with_formats(office_formats())
            .with_materialize_fault(MaterializeFault::SetAndPromiseFail, 1);
        let outcome = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        assert!(
            matches!(transcript_of(&outcome), TranscriptState::Lost(_)),
            "{outcome:?}"
        );
        assert!(host.clipboard_ids().is_empty(), "Zwischenablage leer");
        assert!(!host.still_owner().unwrap());

        // Fokuswechsel vor dem Chord: derselbe Verlust auch am CopyOnly-Ausgang.
        let mut host = FakeHost::new()
            .with_formats(office_formats())
            .with_focus_after_become_owner(Some(WindowId(2)))
            .with_materialize_fault(MaterializeFault::SetAndPromiseFail, 1);
        let outcome = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        assert_eq!(copy_reason(&outcome), CopyOnlyReason::FocusChanged);
        assert!(matches!(transcript_of(&outcome), TranscriptState::Lost(_)));
        assert_eq!(
            TranscriptState::lost_message("x"),
            "Zwischenablage leer — Transkript verloren (x)"
        );
    }

    /// Blocker 2: Ein Shortcut-Fehler nach `become_owner` materialisiert vor
    /// der Rückgabe; der ursprüngliche Fehler bleibt der Ausgang.
    #[test]
    fn shortcut_failure_materializes_before_returning() {
        let mut host = FakeHost::new()
            .with_formats(office_formats())
            .with_failing_shortcut();
        let err = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap_err();
        assert!(err.to_string().contains("SendInput"), "{err}");
        assert_eq!(host.materializations, 1);
        assert!(!host.clipboard.delayed, "kein offenes Versprechen");
        assert!(host.still_owner().unwrap());
        assert_eq!(host.clipboard_text().as_deref(), Some("transkript"));

        // Clipboard blockiert: das Versprechen bleibt offen und eigen — der
        // Idle-Retry bzw. Quit holt es nach.
        let mut host = FakeHost::new()
            .with_formats(office_formats())
            .with_failing_shortcut()
            .with_failing_materialize();
        let err = inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap_err();
        assert!(err.to_string().contains("SendInput"), "{err}");
        assert!(!err.to_string().contains("verloren"), "{err}");
        assert_eq!(host.materialize_attempts, 1);
        assert!(host.promise_open());

        // Materialisierung verliert das Transkript: der Fehler sagt es dazu.
        let mut host = FakeHost::new()
            .with_formats(office_formats())
            .with_failing_shortcut()
            .with_materialize_fault(MaterializeFault::SetAndPromiseFail, 1);
        let err = inject_paste(&mut host, "transkript", &ctx(1), &output())
            .unwrap_err()
            .to_string();
        assert!(err.contains("SendInput"), "{err}");
        assert!(err.contains(TranscriptState::LOST), "{err}");
    }

    /// Hinweis Snapshot/Marker: `CopyOnly` trägt Verlaufsausschluss und —
    /// nur wenn er schon lief — den Snapshot-Report.
    #[test]
    fn copy_only_carries_marker_and_snapshot() {
        // Fokusverlust vor dem Snapshot: kein Report.
        let mut host = FakeHost::new()
            .with_formats(office_formats())
            .with_window(Some(WindowId(2)));
        match inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap() {
            InjectOutcome::CopyOnly {
                history_excluded,
                snapshot,
                transcript,
                ..
            } => {
                assert!(history_excluded);
                assert!(snapshot.is_none());
                assert_eq!(transcript, TranscriptState::Secured);
            }
            other => panic!("{other:?}"),
        }

        // Fokuswechsel nach dem Snapshot, Marker scheitert.
        let mut host = FakeHost::new()
            .with_formats(office_formats())
            .with_focus_after_snapshot(Some(WindowId(2)))
            .with_failing_marker();
        match inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap() {
            InjectOutcome::CopyOnly {
                history_excluded,
                snapshot,
                ..
            } => {
                assert!(!history_excluded, "Marker fehlt");
                let snapshot = snapshot.expect("Snapshot lief");
                assert!(snapshot.saved_count() > 0);
                assert!(formats::snapshot_log_line(&snapshot).starts_with("Clipboard-Snapshot:"));
            }
            other => panic!("{other:?}"),
        }

        // Fokuswechsel nach `become_owner`: ebenfalls mit Report.
        let mut host = FakeHost::new()
            .with_formats(office_formats())
            .with_focus_after_become_owner(Some(WindowId(2)));
        match inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap() {
            InjectOutcome::CopyOnly { snapshot, .. } => assert!(snapshot.is_some()),
            other => panic!("{other:?}"),
        }
    }

    /// Blocker 2, Quit: offenes Versprechen bei blockiertem Clipboard. Mit
    /// Restzeit wird erneut versucht; ohne sie ist es ein Fehler.
    #[test]
    fn quit_retries_a_blocked_promise_within_the_budget() {
        let open_promise = |fault: MaterializeFault, times: u32| {
            let mut host = FakeHost::new()
                .with_formats(office_formats())
                .with_materialize_fault(fault, times);
            inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
            assert!(host.promise_open());
            host
        };

        // Nach dem Lauf noch zweimal blockiert: der dritte Quit-Versuch sichert.
        let mut host = open_promise(MaterializeFault::Blocked, 3);
        assert_eq!(
            protocol::save_transcript_on_quit(
                &mut host,
                Instant::now() + Duration::from_millis(1_000)
            )
            .unwrap(),
            ClipboardSave::Saved
        );
        assert_eq!(host.materialize_attempts, 1 + 3);
        assert_eq!(host.clipboard_text().as_deref(), Some("transkript"));

        // Dauerhaft blockiert, Pump kostet reale Zeit: nach Ablauf `Err` mit
        // dem Grund, und zwar pünktlich.
        let mut host = open_promise(MaterializeFault::Blocked, u32::MAX).with_real_pump(1);
        let started = Instant::now();
        let err =
            protocol::save_transcript_on_quit(&mut host, started + Duration::from_millis(250))
                .unwrap_err();
        let took = started.elapsed();
        assert!(err.to_string().contains("nicht zu öffnen"), "{err}");
        assert!(took >= Duration::from_millis(250), "{took:?}");
        assert!(
            took < Duration::from_millis(400),
            "Frist überzogen: {took:?}"
        );
        // Scheiben höchstens 100 ms: mindestens drei Quit-Versuche.
        assert!(
            host.materialize_attempts > 3,
            "{}",
            host.materialize_attempts
        );

        // Verloren beim Quit.
        let mut host = open_promise(MaterializeFault::Blocked, 1)
            .with_materialize_fault(MaterializeFault::SetAndPromiseFail, 1);
        let err = protocol::save_transcript_on_quit(&mut host, Instant::now()).unwrap_err();
        assert!(err.to_string().contains(TranscriptState::LOST), "{err}");
    }

    /// Nachkontrolle Blocker 2: Jeder Versuch kostet real 90 ms (wie ein
    /// blockiertes `OpenClipboard` mit zehn Versuchen). Die absolute Frist
    /// wird nach jedem Versuch neu gemessen: kein Versuch beginnt nach
    /// Ablauf, überzogen wird höchstens um einen Versuch.
    #[test]
    fn quit_deadline_counts_the_real_cost_of_each_attempt() {
        let cost = Duration::from_millis(90);
        let mut host = FakeHost::new()
            .with_formats(office_formats())
            .with_failing_materialize();
        inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        let mut host = host.with_materialize_cost(cost);
        let before = host.materialize_attempts;
        let started = Instant::now();
        let budget = Duration::from_millis(500);
        assert!(protocol::save_transcript_on_quit(&mut host, started + budget).is_err());
        let took = started.elapsed();
        let attempts = host.materialize_attempts - before;
        assert!(took >= budget, "{took:?}");
        assert!(took < budget + cost + Duration::from_millis(60), "{took:?}");
        // 500 ms / 90 ms: höchstens sechs Versuche, der letzte beginnt vor Ablauf.
        assert!(
            (5..=6).contains(&attempts),
            "{attempts} Versuche in {took:?}"
        );

        // Frist beim Eintreffen schon vorbei: genau ein letzter Versuch.
        let before = host.materialize_attempts;
        let started = Instant::now();
        assert!(protocol::save_transcript_on_quit(&mut host, started).is_err());
        assert_eq!(host.materialize_attempts - before, 1);
        assert!(started.elapsed() < cost + Duration::from_millis(60));
    }

    /// Blocker 2, Quit: `NotOwner` ohne Versprechen ist normal; ein
    /// Versprechen, das fremd überschrieben wurde, ist `PromiseForeign`.
    #[test]
    fn quit_distinguishes_not_owner_from_a_foreign_overwritten_promise() {
        let mut host = FakeHost::new().with_text("fremd");
        assert_eq!(
            protocol::save_transcript_on_quit(&mut host, Instant::now()).unwrap(),
            ClipboardSave::NotOwner
        );

        let mut host = FakeHost::new()
            .with_formats(office_formats())
            .with_failing_materialize();
        inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        host.foreign_sequence_bump();
        assert_eq!(
            protocol::save_transcript_on_quit(&mut host, Instant::now()).unwrap(),
            ClipboardSave::PromiseForeign
        );

        // Schon eager: gesichert, ohne neuen Versuch.
        let mut host = FakeHost::new().with_formats(office_formats());
        inject_paste(&mut host, "transkript", &ctx(1), &output()).unwrap();
        let attempts = host.materialize_attempts;
        assert_eq!(
            protocol::save_transcript_on_quit(&mut host, Instant::now()).unwrap(),
            ClipboardSave::Saved
        );
        assert_eq!(host.materialize_attempts, attempts);
    }
}
