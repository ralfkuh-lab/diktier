//! Plattformneutrales Inject-Protokoll (Spec §7). Kein Win32.

use std::time::{Duration, Instant};

use crate::config::{OutputConfig, PasteShortcut};

use super::formats::{LostFormat, SnapshotKind, SnapshotReport};
use super::{
    CaptureContext, ClipboardReport, ClipboardSave, CopyOnlyReason, InjectError, InjectOutcome,
    PasteKey, RestoreDecision, TranscriptState, WindowId,
};

/// 5-s-Fenster für den ersten Clipboard-Read (Spec §7.1 Punkt 7).
pub const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Grace-Timeout, falls nach Restore niemand die Selection liest (nur Spike).
pub const RESTORED_SERVE_GRACE: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedShortcut {
    CtrlV,
    CtrlShiftV,
    ShiftInsert,
}

impl ResolvedShortcut {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CtrlV => "ctrl_v",
            Self::CtrlShiftV => "ctrl_shift_v",
            Self::ShiftInsert => "shift_insert",
        }
    }
}

/// Snapshot nach Spec §7.1.1 (Leitentscheidung 4). Die Rohdaten bleiben im
/// Host; das Protokoll sieht nur den Ausgang und den Report (Zähler, Größen,
/// Verlustlisten).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardSnapshot {
    pub kind: SnapshotKind,
    pub report: SnapshotReport,
}

impl ClipboardSnapshot {
    pub fn new(kind: SnapshotKind, report: SnapshotReport) -> Self {
        Self { kind, report }
    }

    /// Nacharbeit WP1: Der Snapshot selbst scheiterte (Win32-Code). Kein
    /// Abbruch des Paste, sondern `Unrestorable` ohne Restore-Versprechen.
    pub fn failed(code: u32, duration: std::time::Duration) -> Self {
        Self::new(
            SnapshotKind::Unrestorable,
            SnapshotReport::failed(code, duration),
        )
    }

    /// `Empty` und `Formats` tragen ein Restore-Versprechen, `Unrestorable`
    /// nicht (§7.1 Punkt 2).
    pub fn has_promise(&self) -> bool {
        self.kind != SnapshotKind::Unrestorable
    }
}

/// Ergebnis von [`ClipboardHost::restore_snapshot`] (Leitentscheidung 5).
#[derive(Debug)]
pub enum RestoreResult {
    /// Alles Gesicherte ist platziert, und beim Sichern ging nichts verloren.
    Restored,
    /// Mindestens ein Nutzformat platziert, aber Verluste beim Sichern
    /// und/oder beim Zurückschreiben.
    RestoredPartial {
        lost_save: Vec<LostFormat>,
        lost_restore: Vec<LostFormat>,
    },
    /// Kein Nutzformat platziert; das Transkript liegt (wieder) in der
    /// Zwischenablage.
    RestoreFailed { lost_restore: Vec<LostFormat> },
    /// Owner oder Sequenz stimmten im geöffneten Clipboard nicht mehr. Nichts
    /// angefasst (§7.1 Punkt 5).
    Foreign,
    /// Auch das Transkript-Fallback ließ sich nicht setzen, bzw. der Zustand
    /// nach einem gescheiterten `EmptyClipboard` ist unbekannt →
    /// Inject-Fehler, Tray `error`.
    Failed(InjectError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ModifierState {
    pub shift: bool,
    pub alt: bool,
    pub super_key: bool,
    pub ctrl: bool,
}

impl ModifierState {
    pub fn is_down(self, key: PasteKey) -> bool {
        match key {
            PasteKey::Shift => self.shift,
            PasteKey::Alt => self.alt,
            PasteKey::Super => self.super_key,
            PasteKey::Ctrl => self.ctrl,
            PasteKey::V | PasteKey::Insert => false,
        }
    }

    pub fn set(&mut self, key: PasteKey, down: bool) {
        match key {
            PasteKey::Shift => self.shift = down,
            PasteKey::Alt => self.alt = down,
            PasteKey::Super => self.super_key = down,
            PasteKey::Ctrl => self.ctrl = down,
            PasteKey::V | PasteKey::Insert => {}
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PumpEvents {
    pub reads: u32,
    pub lost_ownership: bool,
}

/// Restore-Zustandsmaschine, Spec §7.1 Punkte 5–8.
#[derive(Debug, Clone)]
pub struct RestoreSession {
    promise: bool,
    reads: u32,
    delay: Duration,
    enabled: bool,
    foreign: bool,
}

impl RestoreSession {
    pub fn new(snapshot: &ClipboardSnapshot, delay: Duration, enabled: bool) -> Self {
        Self {
            promise: snapshot.has_promise(),
            reads: 0,
            delay,
            enabled,
            foreign: false,
        }
    }

    pub fn note_read(&mut self) {
        self.reads = self.reads.saturating_add(1);
    }

    pub fn note_foreign(&mut self) {
        self.foreign = true;
    }

    pub fn apply_pump(&mut self, events: PumpEvents) {
        if events.lost_ownership {
            self.note_foreign();
        }
        for _ in 0..events.reads {
            self.note_read();
        }
    }

    pub fn reads(&self) -> u32 {
        self.reads
    }

    pub fn delay(&self) -> Duration {
        self.delay
    }

    pub fn decide(&self, elapsed: Duration) -> RestoreDecision {
        if !self.enabled {
            return RestoreDecision::Disabled;
        }
        if !self.promise {
            return RestoreDecision::NoPromise;
        }
        if self.foreign {
            return RestoreDecision::ForeignOwner;
        }
        if self.reads == 0 {
            if elapsed >= READ_TIMEOUT {
                return RestoreDecision::NoReadTimeout;
            }
            return RestoreDecision::Wait;
        }
        if elapsed >= self.delay {
            RestoreDecision::Restore
        } else {
            RestoreDecision::Wait
        }
    }
}

/// Spec §7.3: Start = Ende = aktueller Vordergrund; `None` = Fokusverlust.
pub fn focus_allows_inject(ctx: &CaptureContext, current: Option<WindowId>) -> bool {
    matches!(
        (ctx.start_window_id, ctx.target_window_id, current),
        (Some(start), Some(end), Some(now)) if start == end && end == now
    )
}

pub fn copy_only_reason(ctx: &CaptureContext, current: Option<WindowId>) -> CopyOnlyReason {
    if ctx.start_window_id.is_none() || ctx.target_window_id.is_none() || current.is_none() {
        CopyOnlyReason::FocusUnknown
    } else {
        CopyOnlyReason::FocusChanged
    }
}

/// VTE / moderne Terminals → Ctrl+Shift+V (Spec §7.2).
const VTE_NAMES: &[&str] = &[
    "gnome-terminal",
    "gnome-terminal-server",
    "org.gnome.terminal",
    "xfce4-terminal",
    "tilix",
    "alacritty",
    "kitty",
    "ghostty",
];

/// xterm-Familie → Shift+Insert (Spec §7.2).
const XTERM_NAMES: &[&str] = &["xterm", "uxterm"];

pub fn resolve_paste_shortcut(
    config: PasteShortcut,
    wm_class: Option<(&str, &str)>,
) -> ResolvedShortcut {
    match config {
        PasteShortcut::CtrlV => ResolvedShortcut::CtrlV,
        PasteShortcut::CtrlShiftV => ResolvedShortcut::CtrlShiftV,
        PasteShortcut::ShiftInsert => ResolvedShortcut::ShiftInsert,
        PasteShortcut::Auto => auto_shortcut(wm_class),
    }
}

pub fn auto_shortcut(wm_class: Option<(&str, &str)>) -> ResolvedShortcut {
    let Some((instance, class)) = wm_class else {
        return ResolvedShortcut::CtrlV;
    };
    if let Some(shortcut) = windows_process_shortcut(instance, class) {
        return shortcut;
    }
    if matches_any(instance, class, VTE_NAMES) {
        return ResolvedShortcut::CtrlShiftV;
    }
    if matches_any(instance, class, XTERM_NAMES) {
        return ResolvedShortcut::ShiftInsert;
    }
    ResolvedShortcut::CtrlV
}

/// Windows-Zweig (Spec §7.2, windows-plan WP3).
///
/// Eine `WM_CLASS` gibt es hier nicht; der Sink liefert als
/// Trait-Platzhalter **zweimal den Prozess-Basenamen**, also z. B.
/// `("notepad.exe", "notepad.exe")`. Genau diese Form wird hier erkannt:
/// beide Werte gleich **und** ein `.exe`-Suffix. Die Namen der
/// Terminal-Tabelle darunter tragen keins, die bleibt deshalb unberührt.
///
/// Die Regel selbst ist kurz: `WindowsTerminal.exe` bindet beide Chords auf
/// Paste, `conhost`/PowerShell kennen `Ctrl+Shift+V` nicht.
fn windows_process_shortcut(instance: &str, class: &str) -> Option<ResolvedShortcut> {
    if !instance.eq_ignore_ascii_case(class) {
        return None;
    }
    let name = instance.to_ascii_lowercase();
    if !name.ends_with(".exe") {
        return None;
    }
    Some(if name == "windowsterminal.exe" {
        ResolvedShortcut::CtrlShiftV
    } else {
        ResolvedShortcut::CtrlV
    })
}

fn matches_any(instance: &str, class: &str, names: &[&str]) -> bool {
    let instance = instance.to_ascii_lowercase();
    let class = class.to_ascii_lowercase();
    names.iter().any(|name| instance == *name || class == *name)
}

/// Störende Modifier, die den Chord verfälschen würden — ohne die, die der
/// Shortcut selbst braucht (Spec §7.1).
pub fn disturbing_modifiers(shortcut: ResolvedShortcut) -> &'static [PasteKey] {
    match shortcut {
        ResolvedShortcut::CtrlV => &[PasteKey::Shift, PasteKey::Alt, PasteKey::Super],
        ResolvedShortcut::CtrlShiftV | ResolvedShortcut::ShiftInsert => {
            &[PasteKey::Alt, PasteKey::Super]
        }
    }
}

pub fn modifiers_to_clear(held: ModifierState, shortcut: ResolvedShortcut) -> Vec<PasteKey> {
    disturbing_modifiers(shortcut)
        .iter()
        .copied()
        .filter(|key| held.is_down(*key))
        .collect()
}

pub fn modifiers_to_restore(cleared: &[PasteKey], still_held: ModifierState) -> Vec<PasteKey> {
    cleared
        .iter()
        .copied()
        .filter(|key| still_held.is_down(*key))
        .collect()
}

/// Host, den das Protokoll steuert. Fake und Win32-Sink implementieren das.
pub trait ClipboardHost {
    fn mark_start(&mut self);
    fn elapsed(&self) -> Duration;
    fn current_window(&self) -> Option<WindowId>;
    fn wm_class(&self, window: WindowId) -> Option<(String, String)>;
    /// Snapshot nach §7.1.1. Die Rohdaten behält der Host bis zum
    /// `restore_snapshot` bzw. `discard_snapshot`.
    fn snapshot_clipboard(&mut self) -> Result<ClipboardSnapshot, InjectError>;
    /// Transkript als Delayed-Rendering-Versprechen setzen — nur für Paste,
    /// wo der bediente Read (P7) gebraucht wird.
    fn become_owner(&mut self, text: String) -> Result<(), InjectError>;
    /// Transkript direkt **eager** setzen (`CopyOnly`-Pfade): dort wird kein
    /// Read gebraucht, und ein offenes Versprechen könnte beim Beenden
    /// verloren gehen (Sol-Impl-Review Blocker 2). `Ok(PromiseOpen)`: das
    /// eager Setzen scheiterte nach `EmptyClipboard`, das Transkript liegt
    /// als Versprechen. Ist gar nichts mehr zu retten, `Err` mit
    /// [`TranscriptState::LOST`].
    fn copy_transcript(&mut self, text: String) -> Result<TranscriptState, InjectError>;
    /// Ein noch offenes Versprechen des eigenen Transkripts sofort eager
    /// hinterlegen (mit Marker und Sequenzprüfung). `Secured`, wenn Diktier
    /// nicht mehr Owner ist, schon eager, oder ein fremder Copy dazwischen
    /// kam (der bleibt unberührt). Final-Review Blocker 1: Ein Fehlschlag
    /// ohne Mutation behält Eigentum und Versprechen (`PromiseOpen`); scheitert
    /// nach `EmptyClipboard` das eager Setzen, wird noch im geöffneten
    /// Clipboard neu versprochen (`PromiseOpen`); gelingt auch das nicht,
    /// `Lost`.
    fn materialize_transcript(&mut self) -> TranscriptState;
    /// Trägt der eigene Clipboard-Inhalt den Verlaufsausschluss
    /// (Leitentscheidung 7)? Ohne eigenen Inhalt (fremd, leer) `true`.
    fn history_excluded(&mut self) -> bool;
    /// Nach eigener Buchführung liegt ein offenes Versprechen (ohne
    /// Owner-/Sequenzabfrage). Der Quit-Pfad unterscheidet damit ein normales
    /// `NotOwner` von einem Versprechen, das fremd überschrieben wurde.
    fn promise_recorded(&self) -> bool;
    fn still_owner(&mut self) -> Result<bool, InjectError>;
    /// Leitentscheidung 5: den zuletzt gesicherten Inhalt zurückschreiben.
    /// `transcript` ist das Fallback, falls kein Nutzformat platziert werden
    /// kann.
    fn restore_snapshot(&mut self, snapshot: &ClipboardSnapshot, transcript: &str)
    -> RestoreResult;
    /// Gesicherte Rohdaten verwerfen (kein Restore mehr fällig).
    fn discard_snapshot(&mut self);
    fn query_modifiers(&self) -> Result<ModifierState, InjectError>;
    fn key_down(&mut self, key: PasteKey) -> Result<(), InjectError>;
    fn key_up(&mut self, key: PasteKey) -> Result<(), InjectError>;
    fn pump(&mut self, timeout: Duration) -> Result<PumpEvents, InjectError>;
}

pub fn apply_leading_space(text: &str, enabled: bool) -> String {
    if enabled && !text.is_empty() && !text.starts_with(' ') {
        format!(" {text}")
    } else {
        text.to_string()
    }
}

pub fn inject_paste<H: ClipboardHost>(
    host: &mut H,
    text: &str,
    ctx: &CaptureContext,
    output: &OutputConfig,
) -> Result<InjectOutcome, InjectError> {
    let result = inject_paste_inner(host, text, ctx, output);
    // Auf jedem Pfad: die gesicherten Rohdaten (bis 128 MiB) nicht bis zum
    // nächsten Snapshot festhalten. Nach einem Restore ist der Stash schon leer.
    host.discard_snapshot();
    result
}

fn inject_paste_inner<H: ClipboardHost>(
    host: &mut H,
    text: &str,
    ctx: &CaptureContext,
    output: &OutputConfig,
) -> Result<InjectOutcome, InjectError> {
    let text = apply_leading_space(text, output.leading_space);
    let current = host.current_window();
    if !focus_allows_inject(ctx, current) {
        let transcript = host.copy_transcript(text)?;
        return Ok(InjectOutcome::CopyOnly {
            reason: copy_only_reason(ctx, current),
            history_excluded: host.history_excluded(),
            snapshot: None,
            transcript,
        });
    }
    let window = current.expect("focus_allows_inject garantiert Some");
    let wm_class = host.wm_class(window);
    let shortcut = resolve_paste_shortcut(
        output.paste_shortcut,
        wm_class
            .as_ref()
            .map(|(instance, class)| (instance.as_str(), class.as_str())),
    );

    let snapshot = host.snapshot_clipboard()?;
    // Fokusprüfung nach dem Snapshot (codex H2). Der Snapshot kann seit v1.8
    // merklich dauern (alle Formate, §7.1.1).
    let current_now = host.current_window();
    if !focus_allows_inject(ctx, current_now) {
        host.discard_snapshot();
        let transcript = host.copy_transcript(text)?;
        return Ok(InjectOutcome::CopyOnly {
            reason: copy_only_reason(ctx, current_now),
            history_excluded: host.history_excluded(),
            snapshot: Some(snapshot.report),
            transcript,
        });
    }
    let mut session = RestoreSession::new(
        &snapshot,
        Duration::from_millis(u64::from(output.restore_clipboard_delay_ms)),
        output.restore_clipboard,
    );
    host.become_owner(text.clone())?;
    let owned = Owned {
        text,
        shortcut,
        window,
        wm_class,
        snapshot,
    };
    match paste_as_owner(host, ctx, owned, &mut session) {
        Ok(outcome) => Ok(outcome),
        // Final-Review Blocker 2: Nach `become_owner` liegt ein Versprechen.
        // Jeder Fehlerausgang (Shortcut, Pump, Owner-Abfrage) versucht es vor
        // der Rückgabe eager zu machen; der ursprüngliche Fehler bleibt der
        // Ausgang. Bleibt es offen, holt es der Idle-Retry des Workers nach.
        Err(err) => Err(match host.materialize_transcript() {
            TranscriptState::Lost(detail) => {
                InjectError::Failed(format!("{err}; {}", TranscriptState::lost_message(&detail)))
            }
            TranscriptState::Secured | TranscriptState::PromiseOpen(_) => err,
        }),
    }
}

/// Was nach `become_owner` feststeht.
struct Owned {
    text: String,
    shortcut: ResolvedShortcut,
    window: WindowId,
    wm_class: Option<(String, String)>,
    snapshot: ClipboardSnapshot,
}

/// Der Teil nach `become_owner`: ab hier liegt das Transkript als Versprechen
/// im Clipboard.
fn paste_as_owner<H: ClipboardHost>(
    host: &mut H,
    ctx: &CaptureContext,
    owned: Owned,
    session: &mut RestoreSession,
) -> Result<InjectOutcome, InjectError> {
    let Owned {
        text,
        shortcut,
        window,
        wm_class,
        snapshot,
    } = owned;
    // §7.3 (v1.8, Plan B6): letzte Prüfung unmittelbar vor dem ersten
    // Key-Event, nach dem Setzen des Transkripts. Bei Wechsel kein Chord und
    // keine Fensteraktivierung — das Transkript liegt schon im Clipboard.
    let before_chord = host.current_window();
    if !focus_allows_inject(ctx, before_chord) {
        // Kein Chord, also auch kein Read zu erwarten: das Versprechen
        // sofort eager machen (Blocker 2).
        let transcript = host.materialize_transcript();
        return Ok(InjectOutcome::CopyOnly {
            reason: copy_only_reason(ctx, before_chord),
            history_excluded: host.history_excluded(),
            snapshot: Some(snapshot.report),
            transcript,
        });
    }
    send_paste_shortcut(host, shortcut)?;
    host.mark_start();

    let decision = wait_for_restore(host, session)?;
    let mut lost_restore = Vec::new();
    let decision = if decision == RestoreDecision::Restore {
        if host.still_owner()? {
            match host.restore_snapshot(&snapshot, &text) {
                RestoreResult::Restored => RestoreDecision::Restored,
                RestoreResult::RestoredPartial {
                    lost_restore: lost, ..
                } => {
                    let lost_on_restore = !lost.is_empty();
                    lost_restore = lost;
                    RestoreDecision::RestoredPartial { lost_on_restore }
                }
                RestoreResult::RestoreFailed { lost_restore: lost } => {
                    lost_restore = lost;
                    RestoreDecision::RestoreFailed
                }
                RestoreResult::Foreign => RestoreDecision::ForeignOwner,
                RestoreResult::Failed(err) => return Err(err),
            }
        } else {
            RestoreDecision::ForeignOwner
        }
    } else {
        decision
    };

    // Sol-Impl-Review Blocker 2: Ohne Restore bleibt das Transkript liegen.
    // Ein offenes Delayed-Rendering-Versprechen darf dann nicht länger leben
    // als nötig — sonst gehen beim Beenden Original **und** Transkript
    // verloren, wenn das Rendern scheitert. `NoPromise` und `Disabled` enden
    // schon vor dem ersten Read; eager zu setzen, während das Ziel gerade
    // einfügt, würde dessen `OpenClipboard` stören. Deshalb dort erst den
    // ersten Read (der rendert ohnehin eager) oder das 5-s-Fenster abwarten.
    let transcript = match decision {
        RestoreDecision::NoPromise | RestoreDecision::Disabled => {
            wait_for_first_read(host, session)?;
            if host.still_owner()? {
                host.materialize_transcript()
            } else {
                TranscriptState::Secured
            }
        }
        RestoreDecision::NoReadTimeout if host.still_owner()? => host.materialize_transcript(),
        _ => TranscriptState::Secured,
    };

    Ok(InjectOutcome::Pasted {
        restored: decision.is_restored(),
        shortcut,
        window,
        wm_class,
        reads: session.reads(),
        restore: decision,
        clipboard: ClipboardReport {
            snapshot: snapshot.report,
            lost_restore,
            history_excluded: host.history_excluded(),
        },
        transcript,
    })
}

/// Spec §7.1 Punkt 8: nach Restore bedient Diktier den restaurierten Inhalt
/// bis zum Ownership-Verlust weiter. Der Daemon hält sein Clipboard-Fenster
/// sowieso — kein Extra-Wait.
///
/// Der Spike `--inject-test` beendet den Prozess sonst direkt nach Restore.
/// Stirbt der Owner, bevor irgendwer den restaurierten Inhalt geholt hat,
/// wirkt der Restore netto nicht. Deshalb wartet nur der Spike-Pfad hier, bis
/// der restaurierte Inhalt mindestens einmal als Daten-Read bedient wurde,
/// sonst `RESTORED_SERVE_GRACE`.
///
/// Clipboard-Manager erzeugen dabei False-Positive-Reads; Spec §7.1 Punkt 7
/// akzeptiert das.
pub fn serve_restored_until_read<H: ClipboardHost>(
    host: &mut H,
    grace: Duration,
) -> Result<u32, InjectError> {
    if !host.still_owner()? {
        return Ok(0);
    }
    let mut remaining = grace;
    let mut reads = 0_u32;
    while reads == 0 && !remaining.is_zero() {
        let slice = remaining.min(Duration::from_millis(50));
        let events = host.pump(slice)?;
        reads = reads.saturating_add(events.reads);
        if events.lost_ownership || !host.still_owner()? {
            break;
        }
        remaining = remaining.saturating_sub(slice);
    }
    Ok(reads)
}

/// Längste Pause zwischen zwei Sicherungsversuchen im Quit-Pfad.
pub const QUIT_RETRY_SLICE: Duration = Duration::from_millis(100);

/// Quit-Pfad (§7.1 Punkt 8, Final-Review Blocker 2): ein offenes eigenes
/// Versprechen eager hinterlegen, bei blockiertem Clipboard erneut, bis zur
/// absoluten, monotonen `deadline` (Nachkontrolle Blocker 2). Nach **jedem**
/// Versuch und jedem Pump wird die Uhr neu gelesen; nach Ablauf beginnt kein
/// weiterer Versuch und kein weiteres Warten. Der erste Versuch läuft auch
/// dann, wenn die Frist beim Eintreffen schon verstrichen ist — er ist die
/// letzte Chance vor dem Fensterabbau. Überziehen kann die Frist deshalb
/// höchstens um die Dauer **eines** Versuchs (Win32: bis zu
/// `OPEN_RETRIES` × `OPEN_RETRY_WAIT` für ein blockiertes `OpenClipboard`).
///
/// - `NotOwner`: kein eigenes Versprechen (normal).
/// - `PromiseForeign`: das Versprechen war offen, das Clipboard ist inzwischen
///   fremd — ungeklärt, Warnung.
/// - `Saved`: der Text liegt eager.
/// - `Err`: nicht gesichert (weiter blockiert oder Zwischenablage leer).
pub fn save_transcript_on_quit<H: ClipboardHost>(
    host: &mut H,
    deadline: Instant,
) -> Result<ClipboardSave, InjectError> {
    let had_promise = host.promise_recorded();
    if !host.still_owner()? {
        return Ok(if had_promise {
            ClipboardSave::PromiseForeign
        } else {
            ClipboardSave::NotOwner
        });
    }
    if !had_promise {
        // Schon eager im Clipboard (Restore, Render oder Materialisierung).
        return Ok(ClipboardSave::Saved);
    }
    loop {
        let detail = match host.materialize_transcript() {
            TranscriptState::Secured => return settled(host),
            TranscriptState::Lost(detail) => {
                return Err(InjectError::Failed(TranscriptState::lost_message(&detail)));
            }
            TranscriptState::PromiseOpen(detail) => detail,
        };
        let now = Instant::now();
        if now >= deadline {
            return Err(InjectError::Failed(detail));
        }
        // Ein Read während des Pumpens rendert eager — dann ist es gesichert.
        host.pump(QUIT_RETRY_SLICE.min(deadline - now))?;
        if !host.promise_recorded() {
            return settled(host);
        }
        if Instant::now() >= deadline {
            return Err(InjectError::Failed(detail));
        }
    }
}

/// Kein offenes Versprechen mehr: eigen und eager (`Saved`) oder fremd.
fn settled<H: ClipboardHost>(host: &mut H) -> Result<ClipboardSave, InjectError> {
    Ok(if host.still_owner()? {
        ClipboardSave::Saved
    } else {
        ClipboardSave::PromiseForeign
    })
}

/// Für `NoPromise`/`Disabled`: bis zum ersten bedienten Read, zu einem
/// fremden Copy oder zum Ende des 5-s-Fensters pumpen. Die Entscheidung
/// selbst ändert sich dadurch nicht.
fn wait_for_first_read<H: ClipboardHost>(
    host: &mut H,
    session: &mut RestoreSession,
) -> Result<(), InjectError> {
    while session.reads() == 0 {
        let remaining = READ_TIMEOUT.saturating_sub(host.elapsed());
        if remaining.is_zero() {
            break;
        }
        let events = host.pump(remaining.min(Duration::from_millis(50)))?;
        session.apply_pump(events);
        if !host.still_owner()? {
            break;
        }
    }
    Ok(())
}

fn wait_for_restore<H: ClipboardHost>(
    host: &mut H,
    session: &mut RestoreSession,
) -> Result<RestoreDecision, InjectError> {
    loop {
        let elapsed = host.elapsed();
        let decision = session.decide(elapsed);
        if decision != RestoreDecision::Wait {
            return Ok(decision);
        }
        let remaining = if session.reads() == 0 {
            READ_TIMEOUT.saturating_sub(elapsed)
        } else {
            session.delay().saturating_sub(elapsed)
        };
        // Kleine Scheiben, damit Fake-Skripte und echte Events nicht über das
        // Entscheidungsfenster hinwegschießen.
        let slice = if remaining.is_zero() {
            Duration::from_millis(1)
        } else {
            remaining.min(Duration::from_millis(50))
        };
        let events = host.pump(slice)?;
        session.apply_pump(events);
        if !host.still_owner()? {
            session.note_foreign();
        }
    }
}

fn send_paste_shortcut<H: ClipboardHost>(
    host: &mut H,
    shortcut: ResolvedShortcut,
) -> Result<(), InjectError> {
    let held = host.query_modifiers()?;
    let cleared = modifiers_to_clear(held, shortcut);
    for key in &cleared {
        host.key_up(*key)?;
    }

    match shortcut {
        ResolvedShortcut::CtrlV => chord_ctrl_v(host, false)?,
        ResolvedShortcut::CtrlShiftV => chord_ctrl_v(host, true)?,
        ResolvedShortcut::ShiftInsert => chord_shift_insert(host)?,
    }

    // Restore nur nach frischer Query. XQueryKeymap nach synthetischem Up
    // zeigt die Taste oft logisch oben, selbst wenn sie physisch gehalten
    // wird — dann unterbleibt das Restore (codex M3: kein hängender Modifier).
    for key in &cleared {
        if host.query_modifiers()?.is_down(*key) {
            host.key_down(*key)?;
        }
    }
    Ok(())
}

fn chord_ctrl_v<H: ClipboardHost>(host: &mut H, with_shift: bool) -> Result<(), InjectError> {
    let now = host.query_modifiers()?;
    let need_ctrl = !now.ctrl;
    let need_shift = with_shift && !now.shift;
    let mut pressed = Vec::new();
    let result = (|| {
        if need_ctrl {
            host.key_down(PasteKey::Ctrl)?;
            pressed.push(PasteKey::Ctrl);
        }
        if need_shift {
            host.key_down(PasteKey::Shift)?;
            pressed.push(PasteKey::Shift);
        }
        host.key_down(PasteKey::V)?;
        pressed.push(PasteKey::V);
        host.key_up(PasteKey::V)?;
        pressed.retain(|k| *k != PasteKey::V);
        if need_shift {
            host.key_up(PasteKey::Shift)?;
            pressed.retain(|k| *k != PasteKey::Shift);
        }
        if need_ctrl {
            host.key_up(PasteKey::Ctrl)?;
            pressed.retain(|k| *k != PasteKey::Ctrl);
        }
        Ok(())
    })();
    if result.is_err() {
        for key in pressed.into_iter().rev() {
            let _ = host.key_up(key);
        }
    }
    result
}

fn chord_shift_insert<H: ClipboardHost>(host: &mut H) -> Result<(), InjectError> {
    let now = host.query_modifiers()?;
    let need_shift = !now.shift;
    let mut pressed = Vec::new();
    let result = (|| {
        if need_shift {
            host.key_down(PasteKey::Shift)?;
            pressed.push(PasteKey::Shift);
        }
        host.key_down(PasteKey::Insert)?;
        pressed.push(PasteKey::Insert);
        host.key_up(PasteKey::Insert)?;
        pressed.retain(|k| *k != PasteKey::Insert);
        if need_shift {
            host.key_up(PasteKey::Shift)?;
            pressed.retain(|k| *k != PasteKey::Shift);
        }
        Ok(())
    })();
    if result.is_err() {
        for key in pressed.into_iter().rev() {
            let _ = host.key_up(key);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §7.2: `WindowsTerminal.exe` → `Ctrl+Shift+V`, alles andere → `Ctrl+V`.
    /// Der Vergleich ist ASCII-case-insensitiv — `QueryFullProcessImageNameW`
    /// liefert die Schreibweise des Dateisystems, nicht die des Herstellers.
    #[test]
    fn windows_terminal_gets_ctrl_shift_v() {
        for name in [
            "WindowsTerminal.exe",
            "windowsterminal.exe",
            "WINDOWSTERMINAL.EXE",
        ] {
            assert_eq!(
                auto_shortcut(Some((name, name))),
                ResolvedShortcut::CtrlShiftV,
                "{name}"
            );
        }
    }

    #[test]
    fn other_windows_processes_get_ctrl_v() {
        for name in [
            "notepad.exe",
            "Code.exe",
            "conhost.exe",
            "powershell.exe",
            "WindowsTerminalPreview.exe",
            "explorer.EXE",
        ] {
            assert_eq!(
                auto_shortcut(Some((name, name))),
                ResolvedShortcut::CtrlV,
                "{name}"
            );
        }
    }

    /// Die `.exe`-Regel greift nur bei der Platzhalterform `(exe, exe)`. Ein
    /// Paar mit abweichenden Hälften fällt weiter in die Terminal-Tabelle —
    /// auch dann, wenn eine Hälfte auf `.exe` endet.
    #[test]
    fn windows_rule_needs_both_halves_equal() {
        assert_eq!(
            auto_shortcut(Some(("gnome-terminal-server", "Gnome-terminal"))),
            ResolvedShortcut::CtrlShiftV
        );
        // Ungleiche Hälften: die `.exe`-Regel greift nicht, die
        // Terminal-Tabelle kennt den Namen nicht → Default.
        assert_eq!(
            auto_shortcut(Some(("windowsterminal.exe", "Xed"))),
            ResolvedShortcut::CtrlV
        );
        assert_eq!(
            auto_shortcut(Some(("xterm", "xterm.exe"))),
            ResolvedShortcut::ShiftInsert
        );
    }

    /// Ohne `.exe` bleibt alles wie vor Phase 5.
    #[test]
    fn names_without_exe_suffix_use_the_terminal_table() {
        assert_eq!(
            auto_shortcut(Some(("kitty", "kitty"))),
            ResolvedShortcut::CtrlShiftV
        );
        assert_eq!(
            auto_shortcut(Some(("xterm", "XTerm"))),
            ResolvedShortcut::ShiftInsert
        );
        assert_eq!(
            auto_shortcut(Some(("code", "Code"))),
            ResolvedShortcut::CtrlV
        );
        assert_eq!(auto_shortcut(None), ResolvedShortcut::CtrlV);
    }
}
