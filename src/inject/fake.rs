//! Fake-Clipboard und Fake-Host für Unit-Tests. Kein Win32.
//!
//! Der Snapshot läuft über dieselbe Formatmatrix wie auf Windows
//! ([`formats::collect`]); nur Enumeration, Leser und Uhr sind nachgebaut.

use std::cell::Cell;
use std::time::{Duration, Instant};

use super::formats::{
    self, CF_BITMAP, CF_UNICODETEXT, Enumerated, FormatRef, FormatRow, LossReason, LostFormat,
    MAX_SNAPSHOT_BYTES, MAX_SNAPSHOT_TIME, Phase, RowOutcome, SnapshotKind, SnapshotReport,
};
use super::protocol::{
    ClipboardHost, ClipboardSnapshot, ModifierState, PumpEvents, RestoreResult, inject_paste,
};
use super::{
    CaptureContext, Copied, InjectError, InjectOutcome, OutputSink, PasteKey, TranscriptState,
    WindowId,
};
use crate::config::OutputConfig;

/// Ein Format im Fake-Clipboard, mit Fehlerschaltern je Format (Plan WP1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeFormat {
    pub id: u32,
    /// Rohname (nur registrierte Formate).
    pub name: Option<String>,
    pub data: Vec<u8>,
    /// `GetClipboardData` liefert `NULL` (Delayed Rendering der Quelle scheitert).
    pub fail_read: bool,
    /// `SetClipboardData` scheitert beim Zurückschreiben.
    pub fail_set: bool,
    /// So lange rendert die Quelle beim Lesen (Zeitbudget).
    pub read_cost: Duration,
}

impl FakeFormat {
    pub fn new(id: u32, data: impl Into<Vec<u8>>) -> Self {
        Self {
            id,
            name: None,
            data: data.into(),
            fail_read: false,
            fail_set: false,
            read_cost: Duration::ZERO,
        }
    }

    pub fn named(id: u32, name: &str, data: impl Into<Vec<u8>>) -> Self {
        Self {
            name: Some(name.into()),
            ..Self::new(id, data)
        }
    }

    /// `CF_UNICODETEXT` als UTF-16LE mit NUL.
    pub fn text(text: &str) -> Self {
        Self::new(CF_UNICODETEXT, utf16_bytes(text))
    }

    pub fn failing_read(mut self) -> Self {
        self.fail_read = true;
        self
    }

    pub fn failing_set(mut self) -> Self {
        self.fail_set = true;
        self
    }

    pub fn costing(mut self, cost: Duration) -> Self {
        self.read_cost = cost;
        self
    }
}

pub fn utf16_bytes(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn text_from_utf16(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|unit| *unit != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FakeContent {
    /// Nur `CF_UNICODETEXT`.
    Text(String),
    /// Nichts Sicherbares (ein `CF_BITMAP` ohne DIB) → `Unrestorable`.
    NonText,
    Formats(Vec<FakeFormat>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FakeOwner {
    None,
    Us,
    Foreign,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptEvent {
    SelectionRequest,
    ForeignTakeover(FakeContent),
    /// Ungepumpter SelectionClear + Fremdinhalt, wird beim nächsten Snapshot drainiert (codex H1).
    QueuedClear(FakeContent),
    /// Windows-Delayed-Rendering (windows-plan Leitentscheidung 4): der eigene
    /// Render liefert die Daten, zählt als Read **und** erhöht die
    /// Sequenznummer. Die eigene Generation wandert mit — wir bleiben Owner.
    OwnRender,
    /// Fremde Mutation **ohne** Ownership-Wechsel: die Sequenznummer springt,
    /// `GetClipboardOwner()` zeigt aber weiter auf uns. Kein `lost_ownership`;
    /// nur der Generationsvergleich in `still_owner` fängt das ab.
    ForeignSequenceBump,
}

/// Wo die Materialisierung scheitert (Final-Review Blocker 1). Windows:
/// `take_clipboard(Fill::Eager)` im Übergang Delayed → Eager.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterializeFault {
    /// `OpenClipboard` blockiert: nichts angefasst, Versprechen offen.
    Blocked,
    /// `EmptyClipboard` scheitert ohne Sequenzänderung: nichts angefasst,
    /// Eigentum und Versprechen bleiben.
    EmptyFails,
    /// Nach `EmptyClipboard` scheitert das eager `SetClipboardData`; das
    /// Rückfall-Versprechen `SetClipboardData(CF_UNICODETEXT, NULL)` gelingt.
    SetFails,
    /// … und auch das Rückfall-Versprechen scheitert: Zwischenablage leer.
    SetAndPromiseFail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyStroke {
    pub key: PasteKey,
    pub down: bool,
}

#[derive(Debug, Clone)]
pub struct FakeClipboard {
    pub owner: FakeOwner,
    pub generation: u64,
    pub our_generation: Option<u64>,
    pub content: FakeContent,
    /// `ExcludeClipboardContentFromMonitorProcessing` liegt dabei
    /// (Leitentscheidung 7).
    pub excluded: bool,
    /// Das eigene Transkript ist nur ein Delayed-Rendering-Versprechen
    /// (Windows: `SetClipboardData(CF_UNICODETEXT, NULL)`, noch nicht
    /// gerendert).
    pub delayed: bool,
}

impl Default for FakeClipboard {
    fn default() -> Self {
        Self {
            owner: FakeOwner::None,
            generation: 0,
            our_generation: None,
            content: FakeContent::Text(String::new()),
            excluded: false,
            delayed: false,
        }
    }
}

pub struct FakeHost {
    pub elapsed: Duration,
    pub window: Option<WindowId>,
    pub wm_class: Option<(String, String)>,
    pub clipboard: FakeClipboard,
    pub physical: ModifierState,
    /// Wenn true, ändern synthetische Up/Down die „physische“ Map — Modell
    /// für XQueryKeymap nach XTEST.
    pub synthetic_affects_physical: bool,
    pub sent: Vec<KeyStroke>,
    /// Zahl der `GetClipboardData`-Aufrufe in Snapshots. Ein Snapshot der
    /// eigenen Payload darf sie nicht erhöhen (Leitentscheidung 6).
    pub snapshot_reads: u32,
    script: Vec<(Duration, ScriptEvent)>,
    script_idx: usize,
    pending: Vec<ScriptEvent>,
    /// Nach `snapshot_clipboard` gesetztes Fenster (codex H2).
    window_after_snapshot: Option<Option<WindowId>>,
    /// Nach `become_owner` gesetztes Fenster (Plan B6).
    window_after_become_owner: Option<Option<WindowId>>,
    /// Fremder Copy zwischen Vorbereitung und `OpenClipboard` im Restore.
    foreign_before_restore: Option<FakeContent>,
    /// Auch das Transkript-Fallback lässt sich nicht setzen.
    fail_fallback: bool,
    /// Der Snapshot scheitert mit diesem Win32-Code (`OpenClipboard`,
    /// `CountClipboardFormats` oder `EnumClipboardFormats`).
    snapshot_failure: Option<u32>,
    /// `become_owner` scheitert (z. B. `OpenClipboard` weiter blockiert).
    fail_become_owner: bool,
    /// Fremder Copy zwischen Snapshot und Übernahme (Sol-Impl-Review B1).
    foreign_before_become_owner: Option<FakeContent>,
    /// Die beim Snapshot beobachtete Generation; die Übernahme prüft dagegen
    /// (Windows: `snapshot_seq` → `take_clipboard`-`expect`).
    snapshot_generation: Option<u64>,
    /// Fremder Copy unmittelbar vor der Materialisierung (Blocker 2).
    foreign_before_materialize: Option<FakeContent>,
    /// Die Materialisierung scheitert so, noch so viele Male.
    materialize_fault: Option<(MaterializeFault, u32)>,
    /// Zahl erfolgreicher Materialisierungen.
    pub materializations: u32,
    /// Zahl der Materialisierungsversuche bei offenem eigenem Versprechen.
    pub materialize_attempts: u32,
    /// Der Paste-Shortcut scheitert (`SendInput` verweigert).
    fail_shortcut: bool,
    /// So lange dauert jeder Materialisierungsversuch **real** (Win32: ein
    /// blockiertes `OpenClipboard` mit zehn Versuchen). Nachkontrolle
    /// Blocker 2.
    materialize_cost: Duration,
    /// `pump(t)` schläft real `t / n` (Zeitraffer), statt nur die Fake-Uhr
    /// weiterzustellen. `None`: keine reale Zeit.
    real_pump: Option<u32>,
    /// `EmptyClipboard` im Restore scheitert; `true`: mit Sequenzänderung.
    fail_empty: Option<bool>,
    /// Der Verlaufsausschluss lässt sich nicht setzen.
    fail_marker: bool,
    max_bytes: usize,
    max_time: Duration,
    /// Gesicherte Rohdaten des letzten Snapshots.
    stash: Option<Vec<FakeFormat>>,
    fail_data_request: bool,
    connection_dead: bool,
    fail_key_after: Option<usize>,
    key_downs: usize,
}

impl FakeHost {
    pub fn new() -> Self {
        Self {
            elapsed: Duration::ZERO,
            window: Some(WindowId(1)),
            wm_class: Some(("xed".into(), "Xed".into())),
            clipboard: FakeClipboard::default(),
            physical: ModifierState::default(),
            synthetic_affects_physical: false,
            sent: Vec::new(),
            snapshot_reads: 0,
            script: Vec::new(),
            script_idx: 0,
            pending: Vec::new(),
            window_after_snapshot: None,
            window_after_become_owner: None,
            foreign_before_restore: None,
            fail_fallback: false,
            snapshot_failure: None,
            fail_become_owner: false,
            foreign_before_become_owner: None,
            snapshot_generation: None,
            foreign_before_materialize: None,
            materialize_fault: None,
            materializations: 0,
            materialize_attempts: 0,
            fail_shortcut: false,
            materialize_cost: Duration::ZERO,
            real_pump: None,
            fail_empty: None,
            fail_marker: false,
            max_bytes: MAX_SNAPSHOT_BYTES,
            max_time: MAX_SNAPSHOT_TIME,
            stash: None,
            fail_data_request: false,
            connection_dead: false,
            fail_key_after: None,
            key_downs: 0,
        }
    }

    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.clipboard.owner = FakeOwner::Foreign;
        self.clipboard.generation = 1;
        self.clipboard.content = FakeContent::Text(text.into());
        self
    }

    pub fn with_non_text(mut self) -> Self {
        self.clipboard.owner = FakeOwner::Foreign;
        self.clipboard.generation = 1;
        self.clipboard.content = FakeContent::NonText;
        self
    }

    pub fn with_formats(mut self, formats: Vec<FakeFormat>) -> Self {
        self.clipboard.owner = FakeOwner::Foreign;
        self.clipboard.generation = 1;
        self.clipboard.content = FakeContent::Formats(formats);
        self
    }

    pub fn with_script(mut self, script: Vec<(Duration, ScriptEvent)>) -> Self {
        self.script = script;
        self
    }

    pub fn with_wm_class(mut self, instance: &str, class: &str) -> Self {
        self.wm_class = Some((instance.into(), class.into()));
        self
    }

    pub fn with_window(mut self, id: Option<WindowId>) -> Self {
        self.window = id;
        self
    }

    pub fn with_queued_clear(mut self, content: FakeContent) -> Self {
        self.pending.push(ScriptEvent::QueuedClear(content));
        self
    }

    pub fn queue_clear(&mut self, content: FakeContent) {
        self.pending.push(ScriptEvent::QueuedClear(content));
    }

    pub fn with_focus_after_snapshot(mut self, id: Option<WindowId>) -> Self {
        self.window_after_snapshot = Some(id);
        self
    }

    pub fn with_focus_after_become_owner(mut self, id: Option<WindowId>) -> Self {
        self.window_after_become_owner = Some(id);
        self
    }

    pub fn with_foreign_copy_before_restore(mut self, content: FakeContent) -> Self {
        self.foreign_before_restore = Some(content);
        self
    }

    pub fn with_failing_fallback(mut self) -> Self {
        self.fail_fallback = true;
        self
    }

    pub fn with_snapshot_failure(mut self, code: u32) -> Self {
        self.snapshot_failure = Some(code);
        self
    }

    pub fn with_foreign_copy_before_become_owner(mut self, content: FakeContent) -> Self {
        self.foreign_before_become_owner = Some(content);
        self
    }

    pub fn with_foreign_copy_before_materialize(mut self, content: FakeContent) -> Self {
        self.foreign_before_materialize = Some(content);
        self
    }

    /// Clipboard blockiert: jede Materialisierung scheitert ohne Mutation.
    pub fn with_failing_materialize(self) -> Self {
        self.with_materialize_fault(MaterializeFault::Blocked, u32::MAX)
    }

    /// Die nächsten `times` Materialisierungen scheitern an `fault`.
    pub fn with_materialize_fault(mut self, fault: MaterializeFault, times: u32) -> Self {
        self.materialize_fault = Some((fault, times));
        self
    }

    /// Jeder Materialisierungsversuch kostet `cost` reale Zeit.
    pub fn with_materialize_cost(mut self, cost: Duration) -> Self {
        self.materialize_cost = cost;
        self
    }

    /// `pump(t)` schläft real `t / divisor` (Zeitraffer für Worker-Tests).
    pub fn with_real_pump(mut self, divisor: u32) -> Self {
        self.real_pump = Some(divisor.max(1));
        self
    }

    /// Jeder Tastendruck des Shortcuts scheitert (UIPI).
    pub fn with_failing_shortcut(mut self) -> Self {
        self.fail_shortcut = true;
        self
    }

    /// Fremder Copy jetzt (außerhalb eines Pump-Skripts, z. B. im Idle).
    pub fn foreign_copy(&mut self, content: FakeContent) {
        self.apply_takeover(content);
    }

    /// Fremde Sequenzänderung jetzt, ohne dass ein `WM_DESTROYCLIPBOARD`
    /// schon gepumpt wurde: die eigene Buchführung hält das Versprechen noch.
    pub fn foreign_sequence_bump(&mut self) {
        self.apply_foreign_sequence_bump();
    }

    /// Ein eigenes Versprechen ist offen (Owner und Generation geprüft).
    pub fn promise_open(&self) -> bool {
        self.is_ours() && self.clipboard.delayed
    }

    /// `EmptyClipboard` im Restore scheitert; `sequence_changes`: der Zustand
    /// danach ist unbekannt.
    pub fn with_failing_empty(mut self, sequence_changes: bool) -> Self {
        self.fail_empty = Some(sequence_changes);
        self
    }

    pub fn with_failing_marker(mut self) -> Self {
        self.fail_marker = true;
        self
    }

    pub fn with_failing_become_owner(mut self) -> Self {
        self.fail_become_owner = true;
        self
    }

    pub fn with_budget(mut self, max_bytes: usize, max_time: Duration) -> Self {
        self.max_bytes = max_bytes;
        self.max_time = max_time;
        self
    }

    pub fn with_fail_data_request(mut self) -> Self {
        self.fail_data_request = true;
        self
    }

    pub fn with_dead_connection(mut self) -> Self {
        self.connection_dead = true;
        self
    }

    pub fn with_fail_next_key(mut self) -> Self {
        self.fail_key_after = Some(1);
        self
    }

    pub fn with_fail_key_after(mut self, n: usize) -> Self {
        self.fail_key_after = Some(n);
        self
    }

    pub fn clipboard_text(&self) -> Option<String> {
        match &self.clipboard.content {
            FakeContent::Text(text) => Some(text.clone()),
            FakeContent::NonText => None,
            FakeContent::Formats(formats) => formats
                .iter()
                .find(|f| f.id == CF_UNICODETEXT)
                .map(|f| text_from_utf16(&f.data)),
        }
    }

    /// Format-IDs in Reihenfolge; ein Transkript ist genau `CF_UNICODETEXT`.
    pub fn clipboard_ids(&self) -> Vec<u32> {
        match &self.clipboard.content {
            FakeContent::Text(text)
                if text.is_empty() && self.clipboard.owner == FakeOwner::None =>
            {
                Vec::new()
            }
            FakeContent::Text(_) => vec![CF_UNICODETEXT],
            FakeContent::NonText => vec![CF_BITMAP],
            FakeContent::Formats(formats) => formats.iter().map(|f| f.id).collect(),
        }
    }

    pub fn clipboard_formats(&self) -> Vec<FakeFormat> {
        match &self.clipboard.content {
            FakeContent::Formats(formats) => formats.clone(),
            _ => Vec::new(),
        }
    }

    fn is_ours(&self) -> bool {
        self.clipboard.owner == FakeOwner::Us
            && self.clipboard.our_generation == Some(self.clipboard.generation)
    }

    /// Der eigene Render: Sequenz hoch, eigene Generation mit — genau das, was
    /// `WM_RENDERFORMAT` + `expected_seq` auf Windows tun.
    fn apply_own_render(&mut self, out: &mut PumpEvents) {
        if self.fail_data_request {
            return;
        }
        if !self.is_ours() {
            return;
        }
        self.clipboard.generation = self.clipboard.generation.saturating_add(1);
        self.clipboard.our_generation = Some(self.clipboard.generation);
        self.clipboard.delayed = false;
        out.reads += 1;
    }

    /// Fremde Sequenzänderung ohne Ownership-Wechsel: die eigene Generation
    /// bleibt stehen und passt danach nicht mehr.
    fn apply_foreign_sequence_bump(&mut self) {
        self.clipboard.generation = self.clipboard.generation.saturating_add(1);
    }

    fn apply_takeover(&mut self, content: FakeContent) {
        self.clipboard.owner = FakeOwner::Foreign;
        self.clipboard.generation = self.clipboard.generation.saturating_add(1);
        self.clipboard.content = content;
        self.clipboard.our_generation = None;
        self.clipboard.excluded = false;
        self.clipboard.delayed = false;
    }

    /// Eine eigene, eager Mutation (Windows: `EmptyClipboard` +
    /// `SetClipboardData`). `excluded`: der Marker soll dabei sein.
    fn own_mutation(&mut self, content: FakeContent, excluded: bool) {
        self.clipboard.generation = self.clipboard.generation.saturating_add(1);
        self.clipboard.our_generation = Some(self.clipboard.generation);
        self.clipboard.owner = FakeOwner::Us;
        self.clipboard.content = content;
        self.clipboard.excluded = excluded && !self.fail_marker;
        self.clipboard.delayed = false;
    }

    /// Die Übernahme des Transkripts, wie `take_clipboard`: nach einem
    /// Snapshot nur, wenn die Generation noch die beobachtete ist.
    fn take_transcript(&mut self, text: String, delayed: bool) -> Result<(), InjectError> {
        if self.connection_dead {
            return Err(InjectError::Failed("Clipboard-Verbindung tot".into()));
        }
        if let Some(content) = self.foreign_before_become_owner.take() {
            self.apply_takeover(content);
        }
        let expect = self.snapshot_generation.take();
        if self.fail_become_owner {
            return Err(InjectError::Failed(
                "Clipboard nicht zu öffnen (10 Versuche): Win32-Fehler 5".into(),
            ));
        }
        if expect.is_some_and(|generation| generation != self.clipboard.generation) {
            // Windows: `TakeFailure::Foreign` → nichts angefasst.
            return Err(InjectError::Failed(
                "Clipboard zwischenzeitlich fremd geändert".into(),
            ));
        }
        self.own_mutation(FakeContent::Text(text), true);
        self.clipboard.delayed = delayed;
        Ok(())
    }

    fn drain_pending(&mut self, out: &mut PumpEvents) {
        let pending = std::mem::take(&mut self.pending);
        for event in pending {
            match event {
                ScriptEvent::QueuedClear(content) | ScriptEvent::ForeignTakeover(content) => {
                    self.apply_takeover(content);
                    out.lost_ownership = true;
                }
                ScriptEvent::SelectionRequest => {
                    if self.fail_data_request {
                        continue;
                    }
                    if self.clipboard.owner == FakeOwner::Us {
                        self.clipboard.delayed = false;
                        out.reads += 1;
                    }
                }
                ScriptEvent::OwnRender => self.apply_own_render(out),
                ScriptEvent::ForeignSequenceBump => self.apply_foreign_sequence_bump(),
            }
        }
    }

    fn apply_due_events(&mut self, until: Duration, out: &mut PumpEvents) {
        while self.script_idx < self.script.len() {
            let (at, _) = &self.script[self.script_idx];
            if *at > until {
                break;
            }
            let (_, event) = self.script[self.script_idx].clone();
            self.script_idx += 1;
            match event {
                ScriptEvent::SelectionRequest => {
                    if self.fail_data_request {
                        continue;
                    }
                    if self.is_ours() {
                        self.clipboard.delayed = false;
                        out.reads += 1;
                    }
                }
                ScriptEvent::ForeignTakeover(content) | ScriptEvent::QueuedClear(content) => {
                    self.apply_takeover(content);
                    out.lost_ownership = true;
                }
                ScriptEvent::OwnRender => self.apply_own_render(out),
                ScriptEvent::ForeignSequenceBump => self.apply_foreign_sequence_bump(),
            }
        }
    }

    /// Leitentscheidung 6: die zuletzt gesetzte eigene Payload, ohne Read.
    fn own_snapshot(&mut self) -> ClipboardSnapshot {
        let formats = match &self.clipboard.content {
            FakeContent::Text(text) => vec![FakeFormat::text(text)],
            FakeContent::Formats(formats) => formats.clone(),
            FakeContent::NonText => Vec::new(),
        };
        if formats.is_empty() {
            self.stash = Some(Vec::new());
            return ClipboardSnapshot::new(SnapshotKind::Empty, SnapshotReport::empty(true));
        }
        let rows: Vec<FormatRow> = formats
            .iter()
            .map(|f| FormatRow {
                format: FormatRef::new(f.id, f.name.as_deref()),
                outcome: RowOutcome::Saved {
                    bytes: f.data.len(),
                    useful: !formats::is_companion(f.id, f.name.as_deref()),
                },
            })
            .collect();
        let kind = formats::snapshot_kind(rows.len(), &rows);
        self.stash = Some(formats);
        ClipboardSnapshot::new(
            kind,
            SnapshotReport {
                rows,
                duration: Duration::ZERO,
                own: true,
            },
        )
    }

    fn foreign_formats(&self) -> Vec<FakeFormat> {
        match &self.clipboard.content {
            FakeContent::Text(text) if self.clipboard.owner != FakeOwner::None => {
                vec![FakeFormat::text(text)]
            }
            FakeContent::Text(_) => Vec::new(),
            FakeContent::NonText => vec![FakeFormat::new(CF_BITMAP, Vec::new())],
            FakeContent::Formats(formats) => formats.clone(),
        }
    }
}

impl Default for FakeHost {
    fn default() -> Self {
        Self::new()
    }
}

impl ClipboardHost for FakeHost {
    fn mark_start(&mut self) {
        self.elapsed = Duration::ZERO;
        self.script_idx = 0;
    }

    fn elapsed(&self) -> Duration {
        self.elapsed
    }

    fn current_window(&self) -> Option<WindowId> {
        self.window
    }

    fn wm_class(&self, _window: WindowId) -> Option<(String, String)> {
        self.wm_class.clone()
    }

    fn snapshot_clipboard(&mut self) -> Result<ClipboardSnapshot, InjectError> {
        if self.connection_dead {
            return Err(InjectError::Failed("Clipboard-Verbindung tot".into()));
        }
        let mut dummy = PumpEvents::default();
        self.drain_pending(&mut dummy);
        if let Some(id) = self.window_after_snapshot.take() {
            self.window = id;
        }
        // Windows: `snapshot_seq`, auch nach gescheitertem `OpenClipboard`
        // gelesen (Sol-Impl-Review B1).
        self.snapshot_generation = Some(self.clipboard.generation);
        if self.is_ours() {
            return Ok(self.own_snapshot());
        }
        if let Some(code) = self.snapshot_failure {
            // Wie Windows: kein Abbruch, sondern `Unrestorable` mit Verlust.
            self.stash = Some(Vec::new());
            return Ok(ClipboardSnapshot::failed(code, Duration::ZERO));
        }

        let formats = self.foreign_formats();
        if formats.is_empty() {
            self.stash = Some(Vec::new());
            return Ok(ClipboardSnapshot::new(
                SnapshotKind::Empty,
                SnapshotReport::empty(false),
            ));
        }
        let entries: Vec<Enumerated> = formats
            .iter()
            .map(|f| Enumerated::new(f.id, f.name.as_deref()))
            .collect();
        let clock = Cell::new(Duration::ZERO);
        let reads = Cell::new(0_u32);
        let collected = formats::collect(
            &entries,
            self.max_bytes,
            self.max_time,
            || clock.get(),
            |entry, _kind, remaining| {
                let format = formats
                    .iter()
                    .find(|f| f.id == entry.id)
                    .expect("Enumeration stammt aus derselben Liste");
                reads.set(reads.get() + 1);
                clock.set(clock.get() + format.read_cost);
                if format.fail_read {
                    return Err(LossReason::NoData);
                }
                if format.data.len() > remaining {
                    return Err(LossReason::ByteBudget);
                }
                Ok(format.data.clone())
            },
        );
        self.snapshot_reads += reads.get();
        let kind = formats::snapshot_kind(formats.len(), &collected.rows);
        let stash = collected
            .saved
            .iter()
            .map(|saved| {
                let original = formats
                    .iter()
                    .find(|f| f.id == saved.format.id)
                    .expect("gesichert heißt enumeriert");
                FakeFormat {
                    data: saved.bytes.clone(),
                    ..original.clone()
                }
            })
            .collect();
        self.stash = Some(stash);
        Ok(ClipboardSnapshot::new(
            kind,
            SnapshotReport {
                rows: collected.rows,
                duration: clock.get(),
                own: false,
            },
        ))
    }

    fn become_owner(&mut self, text: String) -> Result<(), InjectError> {
        self.take_transcript(text, true)?;
        if let Some(id) = self.window_after_become_owner.take() {
            self.window = id;
        }
        Ok(())
    }

    fn copy_transcript(&mut self, text: String) -> Result<TranscriptState, InjectError> {
        self.take_transcript(text, false)?;
        Ok(TranscriptState::Secured)
    }

    fn materialize_transcript(&mut self) -> TranscriptState {
        if !self.is_ours() || !self.clipboard.delayed {
            return TranscriptState::Secured;
        }
        self.materialize_attempts += 1;
        if !self.materialize_cost.is_zero() {
            std::thread::sleep(self.materialize_cost);
        }
        if let Some(content) = self.foreign_before_materialize.take() {
            // Fremder Copy im Übergang: die Sequenzprüfung lässt ihn stehen.
            self.apply_takeover(content);
            return TranscriptState::Secured;
        }
        if let Some((fault, times)) = self.materialize_fault
            && times > 0
        {
            self.materialize_fault = Some((fault, times.saturating_sub(1)));
            return match fault {
                MaterializeFault::Blocked => TranscriptState::PromiseOpen(
                    "Clipboard nicht zu öffnen (10 Versuche): Win32-Fehler 5".into(),
                ),
                // Nichts geändert: Eigentum und Versprechen bleiben.
                MaterializeFault::EmptyFails => {
                    TranscriptState::PromiseOpen("EmptyClipboard: Win32-Fehler 5".into())
                }
                MaterializeFault::SetFails => {
                    // Geleert und neu versprochen: eigene Generation mit.
                    let content = self.clipboard.content.clone();
                    self.own_mutation(content, true);
                    self.clipboard.delayed = true;
                    TranscriptState::PromiseOpen(
                        "SetClipboardData: Win32-Fehler 8 — Transkript erneut versprochen".into(),
                    )
                }
                MaterializeFault::SetAndPromiseFail => {
                    // Geleert, nichts gesetzt; Windows vergisst das Eigentum.
                    self.own_mutation(FakeContent::Formats(Vec::new()), false);
                    self.clipboard.our_generation = None;
                    TranscriptState::Lost("SetClipboardData: Win32-Fehler 8".into())
                }
            };
        }
        let content = self.clipboard.content.clone();
        self.own_mutation(content, true);
        self.materializations += 1;
        TranscriptState::Secured
    }

    fn promise_recorded(&self) -> bool {
        self.clipboard.owner == FakeOwner::Us && self.clipboard.delayed
    }

    fn history_excluded(&mut self) -> bool {
        if !self.is_ours() {
            return true;
        }
        matches!(&self.clipboard.content, FakeContent::Formats(f) if f.is_empty())
            || self.clipboard.excluded
    }

    fn still_owner(&mut self) -> Result<bool, InjectError> {
        if self.connection_dead {
            return Err(InjectError::Failed("Clipboard-Verbindung tot".into()));
        }
        let ours = self.is_ours();
        if !ours {
            self.clipboard.our_generation = None;
        }
        Ok(ours)
    }

    fn restore_snapshot(
        &mut self,
        snapshot: &ClipboardSnapshot,
        transcript: &str,
    ) -> RestoreResult {
        if self.connection_dead {
            return RestoreResult::Failed(InjectError::Failed("Clipboard-Verbindung tot".into()));
        }
        // Die Vorbereitung (Kopien) liegt vor dem Öffnen; ein fremder Copy
        // genau dazwischen wird erst im geöffneten Clipboard sichtbar.
        let stash = self.stash.take().unwrap_or_default();
        if let Some(content) = self.foreign_before_restore.take() {
            self.apply_takeover(content);
        }
        if !self.is_ours() {
            return RestoreResult::Foreign;
        }
        if let Some(sequence_changes) = self.fail_empty {
            if !sequence_changes {
                // Nichts geändert: das Transkript liegt, wir bleiben Owner.
                return RestoreResult::RestoreFailed {
                    lost_restore: Vec::new(),
                };
            }
            self.own_mutation(FakeContent::Formats(Vec::new()), false);
            return RestoreResult::Failed(InjectError::Failed(
                "EmptyClipboard: Win32-Fehler 5".into(),
            ));
        }
        if snapshot.kind == SnapshotKind::Empty {
            // Wirklich leer, ohne Marker (Leitentscheidung 6: danach `Empty`).
            self.own_mutation(FakeContent::Formats(Vec::new()), false);
            return RestoreResult::Restored;
        }

        let mut placed = Vec::new();
        let mut lost_restore = Vec::new();
        for format in stash {
            if format.fail_set {
                lost_restore.push(LostFormat {
                    format: FormatRef::new(format.id, format.name.as_deref()),
                    // ERROR_ACCESS_DENIED, beliebig — der Code steht nur im Log.
                    reason: LossReason::SetFailed(5),
                    phase: Phase::Restore,
                });
            } else {
                placed.push(format);
            }
        }
        let useful_placed = placed
            .iter()
            .any(|f| !formats::is_companion(f.id, f.name.as_deref()));
        if !useful_placed {
            if self.fail_fallback {
                self.own_mutation(FakeContent::Formats(Vec::new()), false);
                return RestoreResult::Failed(InjectError::Failed(
                    "Zwischenablage leer — Transkript und vorheriger Inhalt verloren".into(),
                ));
            }
            self.own_mutation(FakeContent::Text(transcript.to_string()), true);
            return RestoreResult::RestoreFailed { lost_restore };
        }
        self.own_mutation(FakeContent::Formats(placed), true);
        let lost_save = snapshot.report.lost();
        if lost_save.is_empty() && lost_restore.is_empty() {
            RestoreResult::Restored
        } else {
            RestoreResult::RestoredPartial {
                lost_save,
                lost_restore,
            }
        }
    }

    fn discard_snapshot(&mut self) {
        self.stash = None;
    }

    fn query_modifiers(&self) -> Result<ModifierState, InjectError> {
        Ok(self.physical)
    }

    fn key_down(&mut self, key: PasteKey) -> Result<(), InjectError> {
        if self.fail_shortcut {
            return Err(InjectError::Failed(
                "SendInput Ctrl down: 0 von 1 Events, Win32-Fehler 5".into(),
            ));
        }
        self.key_downs += 1;
        if self.fail_key_after == Some(self.key_downs) {
            return Err(InjectError::Failed("Key-Event fehlgeschlagen".into()));
        }
        self.sent.push(KeyStroke { key, down: true });
        if self.synthetic_affects_physical {
            self.physical.set(key, true);
        }
        Ok(())
    }

    fn key_up(&mut self, key: PasteKey) -> Result<(), InjectError> {
        self.sent.push(KeyStroke { key, down: false });
        if self.synthetic_affects_physical {
            self.physical.set(key, false);
        }
        Ok(())
    }

    fn pump(&mut self, timeout: Duration) -> Result<PumpEvents, InjectError> {
        if self.connection_dead {
            return Err(InjectError::Failed("Clipboard-Verbindung tot".into()));
        }
        if let Some(divisor) = self.real_pump {
            std::thread::sleep(timeout / divisor);
        }
        let until = self.elapsed.saturating_add(timeout);
        let mut out = PumpEvents::default();
        self.drain_pending(&mut out);
        self.apply_due_events(until, &mut out);
        self.elapsed = until;
        Ok(out)
    }
}

/// Für die Worker-Tests (Idle-Retry, Quit): derselbe Fake als `OutputSink`,
/// wie `Win32OutputSink` über das Protokoll.
impl OutputSink for FakeHost {
    fn paste(&mut self, text: &str, ctx: &CaptureContext) -> Result<InjectOutcome, InjectError> {
        inject_paste(self, text, ctx, &OutputConfig::default())
    }

    fn copy_only(&mut self, text: &str) -> Result<Copied, InjectError> {
        let transcript = self.copy_transcript(text.to_string())?;
        Ok(Copied {
            history_excluded: self.history_excluded(),
            transcript,
        })
    }

    fn save_to_clipboard_manager(
        &mut self,
        deadline: Instant,
    ) -> Result<super::ClipboardSave, InjectError> {
        super::protocol::save_transcript_on_quit(self, deadline)
    }

    fn current_window_id(&self) -> Option<WindowId> {
        self.window
    }

    fn pending_promise(&mut self) -> bool {
        self.promise_open()
    }

    fn materialize_pending(&mut self) -> TranscriptState {
        self.materialize_transcript()
    }
}
