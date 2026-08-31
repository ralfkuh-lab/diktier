//! Plattformneutrales Inject-Protokoll (Spec §7). Kein Win32.

use std::time::Duration;

use crate::config::{OutputConfig, PasteShortcut};

use super::{
    CaptureContext, CopyOnlyReason, InjectError, InjectOutcome, PasteKey, RestoreDecision, WindowId,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardSnapshot {
    /// Unicode-Text, ggf. leer (kein Owner). Restore-Versprechen.
    Text(String),
    /// Kein Unicode-Text (Bild, HTML, Dateien, …). Kein Restore-Versprechen.
    NonText,
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
    snapshot: Option<String>,
    reads: u32,
    delay: Duration,
    enabled: bool,
    foreign: bool,
}

impl RestoreSession {
    pub fn new(snapshot: ClipboardSnapshot, delay: Duration, enabled: bool) -> Self {
        let snapshot = match snapshot {
            ClipboardSnapshot::Text(text) => Some(text),
            ClipboardSnapshot::NonText => None,
        };
        Self {
            snapshot,
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

    pub fn snapshot_text(&self) -> Option<&str> {
        self.snapshot.as_deref()
    }

    pub fn delay(&self) -> Duration {
        self.delay
    }

    pub fn decide(&self, elapsed: Duration) -> RestoreDecision {
        if !self.enabled {
            return RestoreDecision::Disabled;
        }
        if self.snapshot.is_none() {
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
    fn snapshot_clipboard(&mut self) -> Result<ClipboardSnapshot, InjectError>;
    fn become_owner(&mut self, text: String) -> Result<(), InjectError>;
    fn still_owner(&mut self) -> Result<bool, InjectError>;
    fn set_serve_text(&mut self, text: String);
    fn release_ownership(&mut self) -> Result<(), InjectError>;
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
    let text = apply_leading_space(text, output.leading_space);
    let current = host.current_window();
    if !focus_allows_inject(ctx, current) {
        host.become_owner(text)?;
        return Ok(InjectOutcome::CopyOnly {
            reason: copy_only_reason(ctx, current),
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
    // Finale Fokusprüfung unmittelbar vor dem ersten Key-Event (codex H2).
    // Snapshot darf davor liegen (INCR/ConvertSelection).
    let current_now = host.current_window();
    if !focus_allows_inject(ctx, current_now) {
        host.become_owner(text)?;
        return Ok(InjectOutcome::CopyOnly {
            reason: copy_only_reason(ctx, current_now),
        });
    }
    let mut session = RestoreSession::new(
        snapshot,
        Duration::from_millis(u64::from(output.restore_clipboard_delay_ms)),
        output.restore_clipboard,
    );
    host.become_owner(text)?;
    send_paste_shortcut(host, shortcut)?;
    host.mark_start();

    let decision = wait_for_restore(host, &mut session)?;
    let ours = host.still_owner()?;
    if decision == RestoreDecision::Restore && ours {
        match session.snapshot_text() {
            Some("") => host.release_ownership()?,
            Some(old) => host.set_serve_text(old.to_string()),
            None => {}
        }
    } else if decision == RestoreDecision::Restore && !ours {
        session.note_foreign();
    }

    let decision = if !host.still_owner()? && decision == RestoreDecision::Restore {
        RestoreDecision::ForeignOwner
    } else {
        decision
    };

    Ok(InjectOutcome::Pasted {
        restored: decision == RestoreDecision::Restore,
        shortcut,
        window,
        wm_class,
        reads: session.reads(),
        restore: decision,
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
