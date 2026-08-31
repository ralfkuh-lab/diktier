//! Fake-Clipboard und Fake-Host für Unit-Tests. Kein Win32.

use std::time::Duration;

use super::protocol::{ClipboardHost, ClipboardSnapshot, ModifierState, PumpEvents};
use super::{InjectError, PasteKey, WindowId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FakeContent {
    Text(String),
    NonText,
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
}

impl Default for FakeClipboard {
    fn default() -> Self {
        Self {
            owner: FakeOwner::None,
            generation: 0,
            our_generation: None,
            content: FakeContent::Text(String::new()),
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
    script: Vec<(Duration, ScriptEvent)>,
    script_idx: usize,
    pending: Vec<ScriptEvent>,
    /// Nach `snapshot_clipboard` gesetztes Fenster (codex H2).
    window_after_snapshot: Option<Option<WindowId>>,
    /// Vor `release_ownership` fremde Übernahme (codex H1 Empty-Restore).
    takeover_before_release: Option<FakeContent>,
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
            script: Vec::new(),
            script_idx: 0,
            pending: Vec::new(),
            window_after_snapshot: None,
            takeover_before_release: None,
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

    pub fn with_takeover_before_release(mut self, content: FakeContent) -> Self {
        self.takeover_before_release = Some(content);
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

    pub fn clipboard_text(&self) -> Option<&str> {
        match &self.clipboard.content {
            FakeContent::Text(text) => Some(text.as_str()),
            FakeContent::NonText => None,
        }
    }

    /// Der eigene Render: Sequenz hoch, eigene Generation mit — genau das, was
    /// `WM_RENDERFORMAT` + `expected_seq` auf Windows tun.
    fn apply_own_render(&mut self, out: &mut PumpEvents) {
        if self.fail_data_request {
            return;
        }
        if self.clipboard.owner != FakeOwner::Us
            || self.clipboard.our_generation != Some(self.clipboard.generation)
        {
            return;
        }
        self.clipboard.generation = self.clipboard.generation.saturating_add(1);
        self.clipboard.our_generation = Some(self.clipboard.generation);
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
                    if self.clipboard.owner == FakeOwner::Us
                        && self.clipboard.our_generation == Some(self.clipboard.generation)
                    {
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
        Ok(match &self.clipboard.content {
            FakeContent::Text(text) if self.clipboard.owner != FakeOwner::None => {
                ClipboardSnapshot::Text(text.clone())
            }
            FakeContent::Text(_) => ClipboardSnapshot::Text(String::new()),
            FakeContent::NonText => ClipboardSnapshot::NonText,
        })
    }

    fn become_owner(&mut self, text: String) -> Result<(), InjectError> {
        if self.connection_dead {
            return Err(InjectError::Failed("Clipboard-Verbindung tot".into()));
        }
        self.clipboard.generation = self.clipboard.generation.saturating_add(1);
        self.clipboard.our_generation = Some(self.clipboard.generation);
        self.clipboard.owner = FakeOwner::Us;
        self.clipboard.content = FakeContent::Text(text);
        Ok(())
    }

    fn still_owner(&mut self) -> Result<bool, InjectError> {
        if self.connection_dead {
            return Err(InjectError::Failed("Clipboard-Verbindung tot".into()));
        }
        let ours = self.clipboard.owner == FakeOwner::Us
            && self.clipboard.our_generation == Some(self.clipboard.generation);
        if !ours {
            self.clipboard.our_generation = None;
        }
        Ok(ours)
    }

    fn set_serve_text(&mut self, text: String) {
        if self.clipboard.owner == FakeOwner::Us
            && self.clipboard.our_generation == Some(self.clipboard.generation)
        {
            self.clipboard.content = FakeContent::Text(text);
        }
    }

    fn release_ownership(&mut self) -> Result<(), InjectError> {
        if let Some(content) = self.takeover_before_release.take() {
            self.apply_takeover(content);
        }
        // Nur freigeben, wenn wir noch Owner der eigenen Generation sind
        // (Server-Timestamp-Analogon, codex H1).
        if self.clipboard.owner == FakeOwner::Us
            && self.clipboard.our_generation == Some(self.clipboard.generation)
        {
            self.clipboard.owner = FakeOwner::None;
            self.clipboard.our_generation = None;
            self.clipboard.content = FakeContent::Text(String::new());
        }
        Ok(())
    }

    fn query_modifiers(&self) -> Result<ModifierState, InjectError> {
        Ok(self.physical)
    }

    fn key_down(&mut self, key: PasteKey) -> Result<(), InjectError> {
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
        let until = self.elapsed.saturating_add(timeout);
        let mut out = PumpEvents::default();
        self.drain_pending(&mut out);
        self.apply_due_events(until, &mut out);
        self.elapsed = until;
        Ok(out)
    }
}
