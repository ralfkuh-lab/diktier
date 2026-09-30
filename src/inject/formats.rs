//! Formatmatrix, Budgets, Nutz-/Begleitformate und Namensbereinigung für
//! Snapshot und Restore aller Clipboard-Formate (Spec §7.1.1,
//! clipboard-restore-plan Leitentscheidungen 1, 2, 4 und 9). Kein Win32.
//!
//! Der Win32-Host und der Fake-Host teilen sich [`collect`]: beide liefern
//! nur die Enumeration und einen Leser, die Matrix und die Budget-Abrechnung
//! stecken hier. So prüfen die Fake-Tests dieselbe Klassifikation, die
//! produktiv läuft.

use std::collections::HashSet;
use std::fmt;
use std::time::Duration;

// Standard-IDs aus `winuser.h` (Microsoft „Standard Clipboard Formats“).
// windows-sys 0.61 führt sie unter `Win32_System_Ole`; wie `CF_UNICODETEXT`
// in `windows.rs` stehen sie deshalb als stabile ABI-Werte hier.
pub const CF_TEXT: u32 = 1;
pub const CF_BITMAP: u32 = 2;
pub const CF_METAFILEPICT: u32 = 3;
pub const CF_SYLK: u32 = 4;
pub const CF_DIF: u32 = 5;
pub const CF_TIFF: u32 = 6;
pub const CF_OEMTEXT: u32 = 7;
pub const CF_DIB: u32 = 8;
pub const CF_PALETTE: u32 = 9;
pub const CF_PENDATA: u32 = 10;
pub const CF_RIFF: u32 = 11;
pub const CF_WAVE: u32 = 12;
pub const CF_UNICODETEXT: u32 = 13;
pub const CF_ENHMETAFILE: u32 = 14;
pub const CF_HDROP: u32 = 15;
pub const CF_LOCALE: u32 = 16;
pub const CF_DIBV5: u32 = 17;
pub const CF_OWNERDISPLAY: u32 = 0x0080;
pub const CF_DSPTEXT: u32 = 0x0081;
pub const CF_DSPBITMAP: u32 = 0x0082;
pub const CF_DSPMETAFILEPICT: u32 = 0x0083;
pub const CF_DSPENHMETAFILE: u32 = 0x008E;
pub const CF_PRIVATEFIRST: u32 = 0x0200;
pub const CF_PRIVATELAST: u32 = 0x02FF;
pub const CF_GDIOBJFIRST: u32 = 0x0300;
pub const CF_GDIOBJLAST: u32 = 0x03FF;

/// Ab hier vergibt `RegisterClipboardFormat` die IDs.
pub const FIRST_REGISTERED: u32 = 0xC000;

/// Leitentscheidung 2: Summe der gesicherten Bytes, weiches Limit.
pub const MAX_SNAPSHOT_BYTES: usize = 128 * 1024 * 1024;
/// Leitentscheidung 2: Laufzeit des Snapshots, weiches Limit, geprüft vor
/// jedem weiteren Format.
pub const MAX_SNAPSHOT_TIME: Duration = Duration::from_secs(1);

/// Microsoft „Clipboard Formats“, Abschnitt Cloud Clipboard and Clipboard
/// History: beliebige Daten in diesem Format halten **alle** Formate aus
/// Verlauf und Cloud heraus (Leitentscheidung 7).
pub const EXCLUDE_FROM_MONITOR: &str = "ExcludeClipboardContentFromMonitorProcessing";
pub const CAN_INCLUDE_IN_HISTORY: &str = "CanIncludeInClipboardHistory";
pub const CAN_UPLOAD_TO_CLOUD: &str = "CanUploadToCloudClipboard";

/// OLE-interne Formate: prozessgebundener Zeiger bzw. OLE-Buchführung
/// (Sol-Review B2). Werden nie kopiert.
const OLE_INTERNAL: &[&str] = &["DataObject", "Ole Private Data"];

/// Begleitformate (H4): tragen allein keinen einfügbaren Inhalt.
const COMPANION_NAMES: &[&str] = &[
    "Preferred DropEffect",
    "Shell Object Offsets",
    EXCLUDE_FROM_MONITOR,
    CAN_INCLUDE_IN_HISTORY,
    CAN_UPLOAD_TO_CLOUD,
];

/// Höchstlänge eines bereinigten Formatnamens (Spec §10).
pub const MAX_NAME_CHARS: usize = 40;

/// Schranke der Enumeration: die ID-Domäne selbst. Format-IDs sind 16-Bit-Werte
/// (Standard `< 0xC000`, registriert `0xC000..=0xFFFF`), mehr verschiedene
/// IDs kann es nicht geben (Sol-Impl-Review, Kleinigkeit).
pub const MAX_ENUMERATED_FORMATS: usize = 0x10000;

/// Snapshot-Fehlercode für eine inkonsistente Enumeration (wiederholte ID
/// oder Schranke überschritten) — kein Win32-Code.
pub const ENUM_INCONSISTENT: u32 = 0;

/// Leitentscheidung 1: alle IDs in Enumerationsreihenfolge. `next(current)`
/// ist `EnumClipboardFormats`: `Ok(0)` heißt Ende (mit `ERROR_SUCCESS`),
/// `Err(code)` ein echter Fehler. Eine wiederholte ID oder mehr IDs als die
/// Domäne hergibt ist ebenfalls ein Snapshot-Fehler
/// ([`ENUM_INCONSISTENT`]) statt einer Endlosschleife.
pub fn enumerate_ids<F>(mut next: F) -> Result<Vec<u32>, u32>
where
    F: FnMut(u32) -> Result<u32, u32>,
{
    let mut ids = Vec::new();
    let mut seen = HashSet::new();
    let mut current = 0_u32;
    loop {
        let id = next(current)?;
        if id == 0 {
            return Ok(ids);
        }
        if !seen.insert(id) || ids.len() >= MAX_ENUMERATED_FORMATS {
            return Err(ENUM_INCONSISTENT);
        }
        ids.push(id);
        current = id;
    }
}

/// Wie die Bytes eines kopierbaren Formats gelesen und geschrieben werden.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataKind {
    /// `GlobalSize`/`GlobalLock` → neues `GMEM_MOVEABLE`.
    Global,
    /// `GetEnhMetaFileBits` → `SetEnhMetaFileBits`.
    Emf,
}

/// Welches gesicherte Format ein GDI-Handle ersetzen kann.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GdiCounterpart {
    /// `CF_BITMAP`, `CF_PALETTE`: Windows synthetisiert sie aus `CF_DIB` bzw.
    /// `CF_DIBV5`.
    Dib,
    /// `CF_METAFILEPICT`: Windows synthetisiert es aus `CF_ENHMETAFILE`.
    Emf,
}

/// Handle-Klasse einer Format-ID (Matrix aus Leitentscheidung 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatClass {
    Data(DataKind),
    Gdi(GdiCounterpart),
    OleInternal,
    /// Freigabe-Semantik liegt beim Owner oder es gibt gar keine Daten.
    NeverCopyable,
    /// Standard-ID (`< 0xC000`) außerhalb der Matrix: konservativ Verlust.
    UnknownStandard,
}

/// Matrix. `raw_name` ist der unbereinigte Name eines registrierten Formats
/// (nur für den Vergleich, nie fürs Log).
pub fn classify(id: u32, raw_name: Option<&str>) -> FormatClass {
    match id {
        // HGLOBAL laut „Standard Clipboard Formats“ bzw. der Freigabetabelle in
        // „Clipboard Operations“ (GlobalFree). CF_SYLK/DIF/TIFF/RIFF/WAVE nennt
        // Microsoft ohne Handle-Typ; sie sind reine Datenblöcke und fallen
        // unter die allgemeine `GMEM_MOVEABLE`-Regel von `SetClipboardData`.
        // Gelesen wird ohnehin nur nach geprüfter Speicherklassifikation.
        CF_TEXT | CF_SYLK | CF_DIF | CF_TIFF | CF_OEMTEXT | CF_DIB | CF_RIFF | CF_WAVE
        | CF_UNICODETEXT | CF_HDROP | CF_LOCALE | CF_DIBV5 | CF_DSPTEXT => {
            FormatClass::Data(DataKind::Global)
        }
        CF_ENHMETAFILE => FormatClass::Data(DataKind::Emf),
        CF_BITMAP | CF_PALETTE => FormatClass::Gdi(GdiCounterpart::Dib),
        CF_METAFILEPICT => FormatClass::Gdi(GdiCounterpart::Emf),
        CF_OWNERDISPLAY | CF_DSPBITMAP | CF_DSPMETAFILEPICT | CF_DSPENHMETAFILE => {
            FormatClass::NeverCopyable
        }
        CF_PRIVATEFIRST..=CF_PRIVATELAST | CF_GDIOBJFIRST..=CF_GDIOBJLAST => {
            FormatClass::NeverCopyable
        }
        id if id >= FIRST_REGISTERED => {
            if raw_name.is_some_and(|name| {
                OLE_INTERNAL
                    .iter()
                    .any(|ole| name.eq_ignore_ascii_case(ole))
            }) {
                FormatClass::OleInternal
            } else {
                FormatClass::Data(DataKind::Global)
            }
        }
        // CF_PENDATA (Pen-Extensions, Handle-Typ undokumentiert) und alles
        // andere unterhalb von 0xC000.
        _ => FormatClass::UnknownStandard,
    }
}

/// H4: Begleitformate entscheiden nicht über den Ausgang. Registrierte Namen
/// vergleicht Windows ohne Groß-/Kleinschreibung.
pub fn is_companion(id: u32, raw_name: Option<&str>) -> bool {
    if id == CF_LOCALE {
        return true;
    }
    id >= FIRST_REGISTERED
        && raw_name.is_some_and(|name| {
            COMPANION_NAMES
                .iter()
                .any(|companion| name.eq_ignore_ascii_case(companion))
        })
}

/// Spec §10: nur druckbares ASCII, sonst `?`, höchstens 40 Zeichen. `"` wird
/// ebenfalls zu `?`, damit die Anführungszeichen im Log eindeutig bleiben.
pub fn sanitize_name(raw: &str) -> String {
    raw.chars()
        .take(MAX_NAME_CHARS)
        .map(|c| {
            if (' '..='~').contains(&c) && c != '"' {
                c
            } else {
                '?'
            }
        })
        .collect()
}

/// Konstantenname einer Standard-ID, für die Diagnose-CLI.
pub fn standard_name(id: u32) -> Option<&'static str> {
    Some(match id {
        CF_TEXT => "CF_TEXT",
        CF_BITMAP => "CF_BITMAP",
        CF_METAFILEPICT => "CF_METAFILEPICT",
        CF_SYLK => "CF_SYLK",
        CF_DIF => "CF_DIF",
        CF_TIFF => "CF_TIFF",
        CF_OEMTEXT => "CF_OEMTEXT",
        CF_DIB => "CF_DIB",
        CF_PALETTE => "CF_PALETTE",
        CF_PENDATA => "CF_PENDATA",
        CF_RIFF => "CF_RIFF",
        CF_WAVE => "CF_WAVE",
        CF_UNICODETEXT => "CF_UNICODETEXT",
        CF_ENHMETAFILE => "CF_ENHMETAFILE",
        CF_HDROP => "CF_HDROP",
        CF_LOCALE => "CF_LOCALE",
        CF_DIBV5 => "CF_DIBV5",
        CF_OWNERDISPLAY => "CF_OWNERDISPLAY",
        CF_DSPTEXT => "CF_DSPTEXT",
        CF_DSPBITMAP => "CF_DSPBITMAP",
        CF_DSPMETAFILEPICT => "CF_DSPMETAFILEPICT",
        CF_DSPENHMETAFILE => "CF_DSPENHMETAFILE",
        CF_PRIVATEFIRST..=CF_PRIVATELAST => "CF_PRIVATE…",
        CF_GDIOBJFIRST..=CF_GDIOBJLAST => "CF_GDIOBJ…",
        _ => return None,
    })
}

/// Phase eines Verlusts (Hinweiszeile in WP3 unterscheidet beide, K3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Save,
    Restore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LossReason {
    /// `CF_OWNERDISPLAY`, Display-GDI-Formate, private und GDI-Objekt-Bereich.
    NeverCopyable,
    /// Standard-ID außerhalb der Matrix.
    UnknownStandard,
    /// GDI-Format ohne gesichertes DIB/DIBV5 bzw. EMF.
    NoCounterpart,
    /// `GetClipboardData == NULL` (z. B. gescheitertes Delayed Rendering).
    NoData,
    /// Size/Lock bzw. `GetEnhMetaFileBits` gescheitert.
    Unreadable,
    /// Passte nicht mehr ins Byte-Budget.
    ByteBudget,
    /// Zeitbudget war schon erschöpft.
    TimeBudget,
    /// Kopie für das Zurückschreiben nicht erzeugbar.
    AllocFailed,
    /// `SetClipboardData` gescheitert, mit Win32-Fehlercode.
    SetFailed(u32),
    /// Der Snapshot selbst scheiterte (`OpenClipboard`, `CountClipboardFormats`
    /// oder `EnumClipboardFormats` mit echtem Fehlercode; `0` = Enumeration
    /// über der Formatschranke). Der Paste läuft ohne Restore-Versprechen
    /// weiter (Nacharbeit WP1).
    SnapshotFailed(u32),
}

impl LossReason {
    pub fn as_str(self) -> String {
        match self {
            Self::NeverCopyable => "nie kopierbar".into(),
            Self::UnknownStandard => "unbekanntes Standardformat".into(),
            Self::NoCounterpart => "GDI ohne Gegenstück".into(),
            Self::NoData => "keine Daten".into(),
            Self::Unreadable => "nicht lesbar".into(),
            Self::ByteBudget => "Byte-Budget".into(),
            Self::TimeBudget => "Zeitbudget".into(),
            Self::AllocFailed => "Speicher".into(),
            Self::SetFailed(err) => format!("SetClipboardData {err}"),
            Self::SnapshotFailed(err) => format!("Snapshot-Fehler {err}"),
        }
    }
}

/// ID plus bereinigter Name (nur registrierte Formate haben einen).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatRef {
    pub id: u32,
    pub name: Option<String>,
}

impl FormatRef {
    pub fn new(id: u32, raw_name: Option<&str>) -> Self {
        Self {
            id,
            name: raw_name.map(sanitize_name),
        }
    }
}

/// Log-Form aus Leitentscheidung 9: `0x0080` bzw. `0xC0A1 "HTML Format"`.
impl fmt::Display for FormatRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:04X}", self.id)?;
        if let Some(name) = &self.name {
            write!(f, " \"{name}\"")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LostFormat {
    pub format: FormatRef,
    pub reason: LossReason,
    pub phase: Phase,
}

/// Was aus einem enumerierten Format geworden ist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowOutcome {
    Saved {
        bytes: usize,
        useful: bool,
    },
    /// Windows synthetisiert es nach dem Restore aus dem gesicherten Format.
    Replaced,
    /// OLE-Verweis entfällt (F1).
    OleDropped,
    Lost(LossReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatRow {
    pub format: FormatRef,
    pub outcome: RowOutcome,
}

/// Eine ID aus `EnumClipboardFormats`, mit dem Rohnamen (nur ≥ 0xC000).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enumerated {
    pub id: u32,
    pub raw_name: Option<String>,
}

impl Enumerated {
    pub fn new(id: u32, raw_name: Option<&str>) -> Self {
        Self {
            id,
            raw_name: raw_name.map(str::to_string),
        }
    }
}

/// Gesicherte Rohdaten eines Formats. Bleiben im Host, das Protokoll sieht
/// nur [`FormatRow`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedData {
    pub format: FormatRef,
    pub kind: DataKind,
    pub useful: bool,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collected {
    /// Eine Zeile je enumeriertem Format, in Enumerationsreihenfolge.
    pub rows: Vec<FormatRow>,
    /// Die gesicherten Formate, in Enumerationsreihenfolge.
    pub saved: Vec<SavedData>,
}

/// Leitentscheidungen 1 und 2: erst die Matrix, dann die Daten, mit weichen
/// Budgets. `read(entry, kind, remaining)` liefert die Bytes eines kopierbaren
/// Formats und prüft **vor** dem Kopieren gegen `remaining`
/// ([`LossReason::ByteBudget`]). Liefert er trotzdem mehr, zählt das Format
/// ebenfalls als Budget-Verlust.
///
/// GDI-Formate, OLE-interne, nie kopierbare und unbekannte IDs werden nicht
/// angefordert — `GetClipboardData` würde bei der Quelle unnötig rendern.
pub fn collect<C, R>(
    entries: &[Enumerated],
    max_bytes: usize,
    max_time: Duration,
    elapsed: C,
    mut read: R,
) -> Collected
where
    C: Fn() -> Duration,
    R: FnMut(&Enumerated, DataKind, usize) -> Result<Vec<u8>, LossReason>,
{
    let mut outcomes: Vec<Option<RowOutcome>> = vec![None; entries.len()];
    let mut saved: Vec<(usize, SavedData)> = Vec::new();
    let mut used = 0_usize;
    let mut time_up = false;

    for (index, entry) in entries.iter().enumerate() {
        let raw_name = entry.raw_name.as_deref();
        let FormatClass::Data(kind) = classify(entry.id, raw_name) else {
            continue;
        };
        if !time_up && elapsed() >= max_time {
            time_up = true;
        }
        if time_up {
            outcomes[index] = Some(RowOutcome::Lost(LossReason::TimeBudget));
            continue;
        }
        let remaining = max_bytes.saturating_sub(used);
        let outcome = match read(entry, kind, remaining) {
            Ok(bytes) if bytes.len() > remaining => RowOutcome::Lost(LossReason::ByteBudget),
            Ok(bytes) => {
                used += bytes.len();
                let useful = !is_companion(entry.id, raw_name);
                let row = RowOutcome::Saved {
                    bytes: bytes.len(),
                    useful,
                };
                saved.push((
                    index,
                    SavedData {
                        format: FormatRef::new(entry.id, raw_name),
                        kind,
                        useful,
                        bytes,
                    },
                ));
                row
            }
            Err(reason) => RowOutcome::Lost(reason),
        };
        outcomes[index] = Some(outcome);
    }

    let saved_ids: Vec<u32> = saved.iter().map(|(_, data)| data.format.id).collect();
    let dib_saved = saved_ids.contains(&CF_DIB) || saved_ids.contains(&CF_DIBV5);
    let emf_saved = saved_ids.contains(&CF_ENHMETAFILE);

    let rows = entries
        .iter()
        .zip(outcomes)
        .map(|(entry, outcome)| {
            let raw_name = entry.raw_name.as_deref();
            let outcome = outcome.unwrap_or_else(|| match classify(entry.id, raw_name) {
                FormatClass::Gdi(GdiCounterpart::Dib) if dib_saved => RowOutcome::Replaced,
                FormatClass::Gdi(GdiCounterpart::Emf) if emf_saved => RowOutcome::Replaced,
                FormatClass::Gdi(_) => RowOutcome::Lost(LossReason::NoCounterpart),
                FormatClass::OleInternal => RowOutcome::OleDropped,
                FormatClass::NeverCopyable => RowOutcome::Lost(LossReason::NeverCopyable),
                FormatClass::UnknownStandard | FormatClass::Data(_) => {
                    RowOutcome::Lost(LossReason::UnknownStandard)
                }
            });
            FormatRow {
                format: FormatRef::new(entry.id, raw_name),
                outcome,
            }
        })
        .collect();

    Collected {
        rows,
        saved: saved.into_iter().map(|(_, data)| data).collect(),
    }
}

/// Leitentscheidung 4.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotKind {
    /// `CountClipboardFormats() == 0`. Restore = leeres Clipboard.
    Empty,
    /// Mindestens ein Nutzformat gesichert. Restore-Versprechen.
    Formats,
    /// Formate vorhanden, aber kein Nutzformat gesichert. Kein Versprechen.
    Unrestorable,
}

pub fn snapshot_kind(format_count: usize, rows: &[FormatRow]) -> SnapshotKind {
    if format_count == 0 {
        return SnapshotKind::Empty;
    }
    let useful_saved = rows
        .iter()
        .any(|row| matches!(row.outcome, RowOutcome::Saved { useful: true, .. }));
    if useful_saved {
        SnapshotKind::Formats
    } else {
        SnapshotKind::Unrestorable
    }
}

/// Was das Protokoll und das Log über einen Snapshot erfahren — Zähler, Größen
/// und Verlustlisten, nie Inhalte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotReport {
    pub rows: Vec<FormatRow>,
    pub duration: Duration,
    /// Leitentscheidung 6: Diktier war noch Owner, die eigene Payload diente
    /// als Snapshot (kein `GetClipboardData`).
    pub own: bool,
}

impl SnapshotReport {
    pub fn empty(own: bool) -> Self {
        Self {
            rows: Vec::new(),
            duration: Duration::ZERO,
            own,
        }
    }

    /// Gescheiterter Snapshot: eine einzige Verlustzeile ohne Format-ID
    /// (`0x0000`), damit Log und CLI den Grund zeigen. Zusammen mit
    /// [`SnapshotKind::Unrestorable`] verwendet.
    pub fn failed(code: u32, duration: Duration) -> Self {
        Self {
            rows: vec![FormatRow {
                format: FormatRef { id: 0, name: None },
                outcome: RowOutcome::Lost(LossReason::SnapshotFailed(code)),
            }],
            duration,
            own: false,
        }
    }

    /// Der Win32-Fehlercode, falls der Snapshot selbst scheiterte.
    pub fn failure(&self) -> Option<u32> {
        self.rows.iter().find_map(|row| match row.outcome {
            RowOutcome::Lost(LossReason::SnapshotFailed(code)) => Some(code),
            _ => None,
        })
    }

    pub fn saved_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| matches!(row.outcome, RowOutcome::Saved { .. }))
            .count()
    }

    pub fn saved_bytes(&self) -> usize {
        self.rows
            .iter()
            .map(|row| match row.outcome {
                RowOutcome::Saved { bytes, .. } => bytes,
                _ => 0,
            })
            .sum()
    }

    pub fn lost(&self) -> Vec<LostFormat> {
        self.rows
            .iter()
            .filter_map(|row| match row.outcome {
                RowOutcome::Lost(reason) => Some(LostFormat {
                    format: row.format.clone(),
                    reason,
                    phase: Phase::Save,
                }),
                _ => None,
            })
            .collect()
    }

    pub fn replaced(&self) -> Vec<FormatRef> {
        self.filter_refs(RowOutcome::Replaced)
    }

    pub fn ole_dropped(&self) -> Vec<FormatRef> {
        self.filter_refs(RowOutcome::OleDropped)
    }

    fn filter_refs(&self, wanted: RowOutcome) -> Vec<FormatRef> {
        self.rows
            .iter()
            .filter(|row| row.outcome == wanted)
            .map(|row| row.format.clone())
            .collect()
    }
}

/// Größe mit deutschem Dezimalkomma, dezimale Einheiten (Plan: „66,4 MB“).
pub fn format_bytes(bytes: usize) -> String {
    let with_unit = |value: f64, unit: &str| format!("{value:.1} {unit}").replace('.', ",");
    if bytes >= 1_000_000 {
        with_unit(bytes as f64 / 1_000_000.0, "MB")
    } else if bytes >= 1_000 {
        with_unit(bytes as f64 / 1_000.0, "kB")
    } else {
        format!("{bytes} B")
    }
}

/// `0x0080 [nie kopierbar], 0xC0A1 "HTML Format" [Zeitbudget]`.
pub fn lost_list(lost: &[LostFormat]) -> String {
    lost.iter()
        .map(|l| format!("{} [{}]", l.format, l.reason.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn ref_list(refs: &[FormatRef]) -> String {
    refs.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Leitentscheidung 9: eine Logzeile je Snapshot, ohne Inhalte.
/// `Clipboard-Snapshot: 9 Formate (7 gesichert, 1 ersetzt, 1 OLE), 66,4 MB, 14 ms, verloren 0`
pub fn snapshot_log_line(report: &SnapshotReport) -> String {
    let total = report.rows.len();
    let replaced = report.replaced();
    let ole = report.ole_dropped();
    let lost = report.lost();
    let mut line = format!(
        "Clipboard-Snapshot: {total} {} ({} gesichert, {} ersetzt, {} OLE), {}, {} ms, verloren {}",
        if total == 1 { "Format" } else { "Formate" },
        report.saved_count(),
        replaced.len(),
        ole.len(),
        format_bytes(report.saved_bytes()),
        report.duration.as_millis(),
        lost.len()
    );
    if report.own {
        line.push_str(" · eigener Inhalt");
    }
    if !lost.is_empty() {
        line.push_str(" · verloren: ");
        line.push_str(&lost_list(&lost));
    }
    if !replaced.is_empty() {
        line.push_str(" · ersetzt: ");
        line.push_str(&ref_list(&replaced));
    }
    if !ole.is_empty() {
        line.push_str(" · OLE: ");
        line.push_str(&ref_list(&ole));
    }
    line
}

/// Die Klammer hinter `restore partial` (Leitentscheidung 9):
/// `Sichern: 0x0080 [nie kopierbar]; Zurückschreiben: 0xC0A1 "HTML Format" [SetClipboardData 5]`.
pub fn partial_detail(lost_save: &[LostFormat], lost_restore: &[LostFormat]) -> String {
    let mut parts = Vec::new();
    if !lost_save.is_empty() {
        parts.push(format!("Sichern: {}", lost_list(lost_save)));
    }
    if !lost_restore.is_empty() {
        parts.push(format!("Zurückschreiben: {}", lost_list(lost_restore)));
    }
    parts.join("; ")
}

fn id_list(ids: &[u32]) -> String {
    ids.iter()
        .map(|id| format!("0x{id:04X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// WP2-Roundtrip, Nachprüfung (Sol-Impl-Review): nach dem Restore werden nur
/// die im Snapshot **gesicherten** IDs byteweise gelesen, mit denselben
/// Budgets wie beim Snapshot. Alle anderen IDs erscheinen nur als Metadaten
/// (`None`) — ein beim Snapshot übersprungenes großes oder blockierendes
/// Format wird nicht ein zweites Mal angefordert. Was das Budget nicht mehr
/// hergibt, bleibt ebenfalls `None`, steht aber zusätzlich in
/// [`SavedRead::unchecked`] — „nicht geprüft“ ist keine Abweichung
/// (Final-Review, Kleinigkeit).
pub fn read_saved<C, R>(
    entries: &[Enumerated],
    saved: &[u32],
    max_bytes: usize,
    max_time: Duration,
    elapsed: C,
    mut read: R,
) -> SavedRead
where
    C: Fn() -> Duration,
    R: FnMut(&Enumerated, DataKind, usize) -> Result<Vec<u8>, LossReason>,
{
    let mut used = 0_usize;
    let mut unchecked = Vec::new();
    let formats = entries
        .iter()
        .map(|entry| {
            let raw_name = entry.raw_name.as_deref();
            let format = FormatRef::new(entry.id, raw_name);
            let data = match classify(entry.id, raw_name) {
                FormatClass::Data(kind) if saved.contains(&entry.id) => {
                    let remaining = max_bytes.saturating_sub(used);
                    if elapsed() >= max_time {
                        unchecked.push(format.clone());
                        None
                    } else {
                        match read(entry, kind, remaining) {
                            Ok(bytes) if bytes.len() <= remaining => {
                                used += bytes.len();
                                Some(bytes)
                            }
                            Ok(_) | Err(LossReason::ByteBudget | LossReason::TimeBudget) => {
                                unchecked.push(format.clone());
                                None
                            }
                            Err(_) => None,
                        }
                    }
                }
                _ => None,
            };
            (format, data)
        })
        .collect();
    SavedRead { formats, unchecked }
}

/// Ergebnis von [`read_saved`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedRead {
    /// Alle IDs in Enumerationsreihenfolge; Bytes nur für gelesene Formate.
    pub formats: Vec<(FormatRef, Option<Vec<u8>>)>,
    /// Gesicherte Formate, die das Budget nicht mehr prüfen ließ.
    pub unchecked: Vec<FormatRef>,
}

/// Reserve je Format für die Nachprüfung: `GlobalSize` darf aufrunden.
pub const VERIFY_SLACK_PER_FORMAT: usize = 64 * 1024;

/// Byte-Budget der Nachprüfung: die gesicherten Längen plus
/// [`VERIFY_SLACK_PER_FORMAT`] je Format — nicht die vollen 128 MiB, und kein
/// Falsch-Positiv, nur weil der letzte Block aufgerundet zurückkommt.
pub fn verify_budget(saved_lengths: &[usize]) -> usize {
    saved_lengths.iter().fold(0_usize, |sum, len| {
        sum.saturating_add(*len)
            .saturating_add(VERIFY_SLACK_PER_FORMAT)
    })
}

/// Bytes gelten als gleich, wenn der neue Block mindestens so lang ist wie der
/// gesicherte und dessen volle alte Länge als Präfix identisch trägt:
/// `GlobalSize` darf laut Microsoft aufrunden („may be larger than the size
/// requested“). Kürzer oder abweichend bleibt eine Abweichung.
pub fn same_payload(saved: &[u8], got: &[u8]) -> bool {
    got.len() >= saved.len() && got[..saved.len()] == *saved
}

/// WP2-Roundtrip: vergleicht die gesicherten Formate mit dem, was nach dem
/// Restore im Clipboard liegt — IDs, Reihenfolge, Bytes je gesichertem Format.
/// `known` sind alle IDs, die vorher enumeriert wurden, plus der eigene
/// Verlaufsausschluss; alles andere danach ist „zusätzlich“. `unchecked`
/// (Budget erschöpft) gilt nicht als Abweichung. Die Meldungen enthalten IDs,
/// Namen und Größen, nie Inhalte.
pub fn compare_roundtrip(
    expected: &[(FormatRef, Vec<u8>)],
    after: &[(FormatRef, Option<Vec<u8>>)],
    known: &[u32],
    unchecked: &[u32],
) -> Vec<String> {
    let mut out = Vec::new();
    for (format, bytes) in expected {
        match after.iter().find(|(f, _)| f.id == format.id) {
            None => out.push(format!("fehlt: {format}")),
            Some((_, None)) if unchecked.contains(&format.id) => {}
            Some((_, None)) => out.push(format!("nicht lesbar: {format}")),
            Some((_, Some(got))) if !same_payload(bytes, got) => out.push(format!(
                "Bytes weichen ab: {format} (vorher {}, nachher {})",
                format_bytes(bytes.len()),
                format_bytes(got.len())
            )),
            Some(_) => {}
        }
    }
    let expected_ids: Vec<u32> = expected.iter().map(|(f, _)| f.id).collect();
    let got: Vec<u32> = after
        .iter()
        .map(|(f, _)| f.id)
        .filter(|id| expected_ids.contains(id))
        .collect();
    let want: Vec<u32> = expected_ids
        .iter()
        .copied()
        .filter(|id| got.contains(id))
        .collect();
    if got != want {
        out.push(format!(
            "Reihenfolge weicht ab: vorher {}, nachher {}",
            id_list(&want),
            id_list(&got)
        ));
    }
    for (format, _) in after {
        if !known.contains(&format.id) {
            out.push(format!("zusätzlich: {format}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const HTML: &str = "HTML Format";

    #[test]
    fn enumeration_stops_at_the_end_and_on_errors() {
        let list = [CF_UNICODETEXT, CF_LOCALE, 0xC0A1];
        let next = |current: u32| -> Result<u32, u32> {
            let pos = list.iter().position(|id| *id == current);
            Ok(match pos {
                None => list[0],
                Some(i) if i + 1 < list.len() => list[i + 1],
                Some(_) => 0,
            })
        };
        assert_eq!(enumerate_ids(next), Ok(list.to_vec()));
        assert_eq!(enumerate_ids(|_| Ok(0)), Ok(Vec::new()));
        // Echter Fehlercode (ERROR_CLIPBOARD_NOT_OPEN) mitten in der Liste.
        let mut calls = 0;
        assert_eq!(
            enumerate_ids(|_| {
                calls += 1;
                if calls == 3 { Err(1418) } else { Ok(calls) }
            }),
            Err(1418)
        );
    }

    /// Eine wiederholte ID ist das Ende mit Snapshot-Fehler, keine
    /// Endlosschleife (Sol-Impl-Review, Kleinigkeit).
    #[test]
    fn repeated_ids_end_the_enumeration_with_an_error() {
        // 13 → 16 → 13 → …
        let next = |current: u32| -> Result<u32, u32> {
            Ok(if current == CF_UNICODETEXT {
                CF_LOCALE
            } else {
                CF_UNICODETEXT
            })
        };
        assert_eq!(enumerate_ids(next), Err(ENUM_INCONSISTENT));
        // Sich selbst als Nachfolger.
        assert_eq!(enumerate_ids(|_| Ok(0xC0A1)), Err(ENUM_INCONSISTENT));
        // Die Schranke ist die ID-Domäne, nicht mehr 4096.
        let mut id = 0_u32;
        let many = enumerate_ids(|_| {
            id += 1;
            Ok(if id <= 5_000 { id } else { 0 })
        });
        assert_eq!(many.map(|ids| ids.len()), Ok(5_000));
    }

    /// Hinweis read_raw: ein beim Snapshot übersprungenes großes Format wird
    /// in der Nachprüfung nicht angefordert, nur als Metadatum geführt.
    #[test]
    fn read_saved_only_reads_saved_ids_within_budget() {
        let entries = [
            Enumerated::new(CF_UNICODETEXT, None),
            Enumerated::new(CF_DIB, None),
            Enumerated::new(0xC0A1, Some(HTML)),
            Enumerated::new(CF_BITMAP, None),
        ];
        let mut asked = Vec::new();
        let after = read_saved(
            &entries,
            &[CF_UNICODETEXT, 0xC0A1],
            MAX_SNAPSHOT_BYTES,
            MAX_SNAPSHOT_TIME,
            || Duration::ZERO,
            |entry, _, _| {
                asked.push(entry.id);
                Ok(vec![1; 4])
            },
        );
        assert_eq!(asked, vec![CF_UNICODETEXT, 0xC0A1], "kein Read für das DIB");
        let data: Vec<_> = after
            .formats
            .iter()
            .map(|(f, d)| (f.id, d.is_some()))
            .collect();
        assert_eq!(
            data,
            vec![
                (CF_UNICODETEXT, true),
                (CF_DIB, false),
                (0xC0A1, true),
                (CF_BITMAP, false),
            ]
        );
        // Das übersprungene DIB war im Snapshot bekannt: keine Abweichung.
        let expected = vec![
            (FormatRef::new(CF_UNICODETEXT, None), vec![1; 4]),
            (FormatRef::new(0xC0A1, Some(HTML)), vec![1; 4]),
        ];
        let known = [CF_UNICODETEXT, CF_DIB, 0xC0A1, CF_BITMAP];
        assert!(compare_roundtrip(&expected, &after.formats, &known, &[]).is_empty());

        // Budget gilt auch hier: das zweite Format passt nicht mehr.
        let after = read_saved(
            &entries,
            &[CF_UNICODETEXT, 0xC0A1],
            6,
            MAX_SNAPSHOT_TIME,
            || Duration::ZERO,
            |_, _, _| Ok(vec![1; 4]),
        );
        assert!(after.formats[0].1.is_some());
        assert!(after.formats[2].1.is_none());
        assert_eq!(after.unchecked, vec![FormatRef::new(0xC0A1, Some(HTML))]);
        // Zeitbudget erschöpft: gar nichts mehr lesen.
        let after = read_saved(
            &entries,
            &[CF_UNICODETEXT],
            MAX_SNAPSHOT_BYTES,
            MAX_SNAPSHOT_TIME,
            || MAX_SNAPSHOT_TIME,
            |_, _, _| panic!("kein Read nach Ablauf"),
        );
        assert!(after.formats.iter().all(|(_, d)| d.is_none()));
        assert_eq!(after.unchecked, vec![FormatRef::new(CF_UNICODETEXT, None)]);
    }

    /// Kleinigkeit Roundtrip: Budget erschöpft heißt „nicht geprüft“, keine
    /// Abweichung; ein echter Lesefehler bleibt „nicht lesbar“. Das Budget
    /// sind die gesicherten Längen plus Reserve je Format.
    #[test]
    fn exhausted_verify_budget_is_unchecked_not_a_mismatch() {
        assert_eq!(verify_budget(&[]), 0);
        assert_eq!(verify_budget(&[10, 20]), 30 + 2 * VERIFY_SLACK_PER_FORMAT);
        assert_eq!(verify_budget(&[usize::MAX, 1]), usize::MAX);

        let entries = [
            Enumerated::new(CF_UNICODETEXT, None),
            Enumerated::new(0xC0A1, Some(HTML)),
        ];
        let saved = [CF_UNICODETEXT, 0xC0A1];
        // Der zweite Block passt nicht mehr ins restliche Budget.
        let after = read_saved(
            &entries,
            &saved,
            6,
            MAX_SNAPSHOT_TIME,
            || Duration::ZERO,
            |entry, _, remaining| {
                if entry.id == 0xC0A1 {
                    Err(if remaining < 100 {
                        LossReason::ByteBudget
                    } else {
                        LossReason::NoData
                    })
                } else {
                    Ok(vec![1; 4])
                }
            },
        );
        let expected = vec![
            (FormatRef::new(CF_UNICODETEXT, None), vec![1; 4]),
            (FormatRef::new(0xC0A1, Some(HTML)), vec![2; 4]),
        ];
        let known = [CF_UNICODETEXT, 0xC0A1];
        let unchecked: Vec<u32> = after.unchecked.iter().map(|f| f.id).collect();
        assert_eq!(unchecked, vec![0xC0A1]);
        assert!(
            compare_roundtrip(&expected, &after.formats, &known, &unchecked).is_empty(),
            "Budget ist keine Abweichung"
        );

        // Echter Lesefehler: weiter „nicht lesbar“.
        let after = read_saved(
            &entries,
            &saved,
            MAX_SNAPSHOT_BYTES,
            MAX_SNAPSHOT_TIME,
            || Duration::ZERO,
            |entry, _, _| {
                if entry.id == 0xC0A1 {
                    Err(LossReason::NoData)
                } else {
                    Ok(vec![1; 4])
                }
            },
        );
        assert!(after.unchecked.is_empty());
        assert_eq!(
            compare_roundtrip(&expected, &after.formats, &known, &[]),
            vec!["nicht lesbar: 0xC0A1 \"HTML Format\"".to_string()]
        );
    }

    #[test]
    fn roundtrip_bytes_allow_global_size_rounding() {
        assert!(same_payload(&[1, 2, 3], &[1, 2, 3]));
        // Aufgerundeter Block: Präfix in voller alter Länge gleich.
        assert!(same_payload(&[1, 2, 3], &[1, 2, 3, 0, 0, 0, 0, 0]));
        assert!(same_payload(&[], &[0]));
        // Kürzer oder abweichend bleibt Abweichung.
        assert!(!same_payload(&[1, 2, 3], &[1, 2]));
        assert!(!same_payload(&[1, 2, 3], &[1, 2, 4, 0]));
        assert!(!same_payload(&[1, 2, 3], &[0, 1, 2, 3]));

        let text = FormatRef::new(CF_UNICODETEXT, None);
        let expected = vec![(text.clone(), vec![1, 2])];
        let known = [CF_UNICODETEXT];
        let rounded = vec![(text.clone(), Some(vec![1, 2, 0, 0, 0, 0, 0, 0]))];
        assert!(compare_roundtrip(&expected, &rounded, &known, &[]).is_empty());
        let shorter = vec![(text, Some(vec![1]))];
        assert_eq!(
            compare_roundtrip(&expected, &shorter, &known, &[]),
            vec!["Bytes weichen ab: 0x000D (vorher 2 B, nachher 1 B)".to_string()]
        );
    }

    #[test]
    fn failed_snapshot_report_names_the_code() {
        let report = SnapshotReport::failed(1418, Duration::from_millis(3));
        assert_eq!(report.failure(), Some(1418));
        assert_eq!(snapshot_kind(1, &report.rows), SnapshotKind::Unrestorable);
        assert_eq!(
            snapshot_log_line(&report),
            "Clipboard-Snapshot: 1 Format (0 gesichert, 0 ersetzt, 0 OLE), 0 B, 3 ms, verloren 1 · verloren: 0x0000 [Snapshot-Fehler 1418]"
        );
        assert_eq!(SnapshotReport::empty(false).failure(), None);
    }

    #[test]
    fn roundtrip_compare_accepts_identical_and_reports_differences() {
        let text = FormatRef::new(CF_UNICODETEXT, None);
        let html = FormatRef::new(0xC0A1, Some(HTML));
        let marker = FormatRef::new(0xC0FF, Some(EXCLUDE_FROM_MONITOR));
        let expected = vec![(text.clone(), vec![1, 2]), (html.clone(), vec![3])];
        let known = [CF_UNICODETEXT, 0xC0A1, CF_TEXT, 0xC0FF];

        // Identisch, plus synthetisiertes CF_TEXT und unser Marker.
        let same = vec![
            (text.clone(), Some(vec![1, 2])),
            (FormatRef::new(CF_TEXT, None), Some(vec![9])),
            (html.clone(), Some(vec![3])),
            (marker.clone(), Some(vec![0; 4])),
        ];
        assert!(compare_roundtrip(&expected, &same, &known, &[]).is_empty());

        let broken = vec![
            (html.clone(), Some(vec![4])),
            (text.clone(), Some(vec![1, 2])),
            (FormatRef::new(0xC200, Some("Neu")), None),
        ];
        assert_eq!(
            compare_roundtrip(&expected, &broken, &known, &[]),
            vec![
                "Bytes weichen ab: 0xC0A1 \"HTML Format\" (vorher 1 B, nachher 1 B)".to_string(),
                "Reihenfolge weicht ab: vorher 0x000D 0xC0A1, nachher 0xC0A1 0x000D".to_string(),
                "zusätzlich: 0xC200 \"Neu\"".to_string(),
            ]
        );

        let missing = vec![(text, None)];
        assert_eq!(
            compare_roundtrip(&expected, &missing, &known, &[]),
            vec![
                "nicht lesbar: 0x000D".to_string(),
                "fehlt: 0xC0A1 \"HTML Format\"".to_string(),
            ]
        );
    }

    fn entry(id: u32, name: Option<&str>) -> Enumerated {
        Enumerated::new(id, name)
    }

    fn ok_reader(
        size: usize,
    ) -> impl FnMut(&Enumerated, DataKind, usize) -> Result<Vec<u8>, LossReason> {
        move |_, _, remaining| {
            if size > remaining {
                Err(LossReason::ByteBudget)
            } else {
                Ok(vec![0xAB; size])
            }
        }
    }

    #[test]
    fn matrix_standard_hglobal_formats() {
        for id in [
            CF_TEXT,
            CF_SYLK,
            CF_DIF,
            CF_TIFF,
            CF_OEMTEXT,
            CF_DIB,
            CF_RIFF,
            CF_WAVE,
            CF_UNICODETEXT,
            CF_HDROP,
            CF_LOCALE,
            CF_DIBV5,
            CF_DSPTEXT,
        ] {
            assert_eq!(
                classify(id, None),
                FormatClass::Data(DataKind::Global),
                "0x{id:04X}"
            );
        }
        assert_eq!(
            classify(CF_ENHMETAFILE, None),
            FormatClass::Data(DataKind::Emf)
        );
    }

    #[test]
    fn matrix_gdi_formats_are_never_locked() {
        assert_eq!(
            classify(CF_BITMAP, None),
            FormatClass::Gdi(GdiCounterpart::Dib)
        );
        assert_eq!(
            classify(CF_PALETTE, None),
            FormatClass::Gdi(GdiCounterpart::Dib)
        );
        assert_eq!(
            classify(CF_METAFILEPICT, None),
            FormatClass::Gdi(GdiCounterpart::Emf)
        );
    }

    #[test]
    fn matrix_never_copyable_and_unknown() {
        for id in [
            CF_OWNERDISPLAY,
            CF_DSPBITMAP,
            CF_DSPMETAFILEPICT,
            CF_DSPENHMETAFILE,
            CF_PRIVATEFIRST,
            0x0250,
            CF_PRIVATELAST,
            CF_GDIOBJFIRST,
            0x0350,
            CF_GDIOBJLAST,
        ] {
            assert_eq!(classify(id, None), FormatClass::NeverCopyable, "0x{id:04X}");
        }
        for id in [0, CF_PENDATA, 18, 0x007F, 0x0084, 0x0400, 0xBFFF] {
            assert_eq!(
                classify(id, None),
                FormatClass::UnknownStandard,
                "0x{id:04X}"
            );
        }
    }

    #[test]
    fn matrix_registered_formats() {
        assert_eq!(
            classify(0xC0A1, Some(HTML)),
            FormatClass::Data(DataKind::Global)
        );
        // Ohne lesbaren Namen trotzdem ein registriertes HGLOBAL-Format.
        assert_eq!(classify(0xC0A1, None), FormatClass::Data(DataKind::Global));
        assert_eq!(
            classify(0xFFFF, Some("PNG")),
            FormatClass::Data(DataKind::Global)
        );
        assert_eq!(
            classify(0xC010, Some("DataObject")),
            FormatClass::OleInternal
        );
        assert_eq!(
            classify(0xC011, Some("ole private data")),
            FormatClass::OleInternal
        );
    }

    #[test]
    fn companion_formats() {
        assert!(is_companion(CF_LOCALE, None));
        for name in [
            "Preferred DropEffect",
            "shell object offsets",
            EXCLUDE_FROM_MONITOR,
            CAN_INCLUDE_IN_HISTORY,
            CAN_UPLOAD_TO_CLOUD,
        ] {
            assert!(is_companion(0xC100, Some(name)), "{name}");
        }
        assert!(!is_companion(CF_UNICODETEXT, None));
        assert!(!is_companion(CF_HDROP, None));
        assert!(!is_companion(0xC100, Some(HTML)));
        assert!(!is_companion(0xC100, None));
        // Ein Standardformat mit demselben Namen gibt es nicht; die ID zählt.
        assert!(!is_companion(CF_TEXT, Some("Preferred DropEffect")));
    }

    #[test]
    fn names_are_sanitized() {
        assert_eq!(sanitize_name(HTML), HTML);
        assert_eq!(sanitize_name("Zeile\nzwei"), "Zeile?zwei");
        assert_eq!(sanitize_name("Grüße"), "Gr??e");
        assert_eq!(sanitize_name("a\"b"), "a?b");
        assert_eq!(sanitize_name("\u{1b}[31m"), "?[31m");
        let long = "x".repeat(60);
        assert_eq!(sanitize_name(&long).len(), MAX_NAME_CHARS);
        assert_eq!(sanitize_name(""), "");
    }

    #[test]
    fn format_ref_display() {
        assert_eq!(FormatRef::new(0x80, None).to_string(), "0x0080");
        assert_eq!(
            FormatRef::new(0xC0A1, Some(HTML)).to_string(),
            "0xC0A1 \"HTML Format\""
        );
        assert_eq!(
            FormatRef::new(0xC0A2, Some("a\"\n")).to_string(),
            "0xC0A2 \"a??\""
        );
    }

    #[test]
    fn collect_keeps_order_and_classifies() {
        let entries = [
            entry(CF_UNICODETEXT, None),
            entry(CF_LOCALE, None),
            entry(CF_TEXT, None),
            entry(CF_BITMAP, None),
            entry(0xC010, Some("DataObject")),
            entry(0xC0A1, Some(HTML)),
            entry(CF_OWNERDISPLAY, None),
            entry(CF_DIB, None),
        ];
        let collected = collect(
            &entries,
            MAX_SNAPSHOT_BYTES,
            MAX_SNAPSHOT_TIME,
            || Duration::ZERO,
            ok_reader(4),
        );
        let outcomes: Vec<_> = collected.rows.iter().map(|r| r.outcome).collect();
        assert_eq!(
            outcomes,
            vec![
                RowOutcome::Saved {
                    bytes: 4,
                    useful: true
                },
                RowOutcome::Saved {
                    bytes: 4,
                    useful: false
                },
                RowOutcome::Saved {
                    bytes: 4,
                    useful: true
                },
                // Das DIB steht **hinter** dem Bitmap und zählt trotzdem.
                RowOutcome::Replaced,
                RowOutcome::OleDropped,
                RowOutcome::Saved {
                    bytes: 4,
                    useful: true
                },
                RowOutcome::Lost(LossReason::NeverCopyable),
                RowOutcome::Saved {
                    bytes: 4,
                    useful: true
                },
            ]
        );
        let saved: Vec<_> = collected.saved.iter().map(|s| s.format.id).collect();
        assert_eq!(
            saved,
            vec![CF_UNICODETEXT, CF_LOCALE, CF_TEXT, 0xC0A1, CF_DIB]
        );
        assert_eq!(collected.saved[3].format.name.as_deref(), Some(HTML));
    }

    #[test]
    fn collect_never_reads_non_data_formats() {
        let entries = [
            entry(CF_BITMAP, None),
            entry(CF_PALETTE, None),
            entry(CF_METAFILEPICT, None),
            entry(CF_OWNERDISPLAY, None),
            entry(0x0210, None),
            entry(0x0310, None),
            entry(0xC010, Some("DataObject")),
            entry(CF_PENDATA, None),
        ];
        let mut calls = 0;
        let collected = collect(
            &entries,
            MAX_SNAPSHOT_BYTES,
            MAX_SNAPSHOT_TIME,
            || Duration::ZERO,
            |_, _, _| {
                calls += 1;
                Ok(Vec::new())
            },
        );
        assert_eq!(calls, 0);
        assert!(collected.saved.is_empty());
        // Ohne DIB/EMF sind die GDI-Formate Verlust.
        assert_eq!(
            collected.rows[0].outcome,
            RowOutcome::Lost(LossReason::NoCounterpart)
        );
        assert_eq!(
            collected.rows[2].outcome,
            RowOutcome::Lost(LossReason::NoCounterpart)
        );
        assert_eq!(
            collected.rows[7].outcome,
            RowOutcome::Lost(LossReason::UnknownStandard)
        );
    }

    #[test]
    fn metafilepict_is_replaced_only_with_saved_emf() {
        let entries = [entry(CF_ENHMETAFILE, None), entry(CF_METAFILEPICT, None)];
        let ok = collect(
            &entries,
            MAX_SNAPSHOT_BYTES,
            MAX_SNAPSHOT_TIME,
            || Duration::ZERO,
            ok_reader(8),
        );
        assert_eq!(ok.rows[1].outcome, RowOutcome::Replaced);
        assert_eq!(ok.saved[0].kind, DataKind::Emf);

        // EMF nicht lesbar → das METAFILEPICT hat kein Gegenstück.
        let failed = collect(
            &entries,
            MAX_SNAPSHOT_BYTES,
            MAX_SNAPSHOT_TIME,
            || Duration::ZERO,
            |_, _, _| Err(LossReason::Unreadable),
        );
        assert_eq!(
            failed.rows[1].outcome,
            RowOutcome::Lost(LossReason::NoCounterpart)
        );
    }

    #[test]
    fn byte_budget_skips_one_format_and_continues() {
        let entries = [
            entry(CF_UNICODETEXT, None),
            entry(CF_DIB, None),
            entry(CF_DIBV5, None),
            entry(0xC0A1, Some(HTML)),
        ];
        let sizes = [10_usize, 60, 60, 20];
        let mut next = 0;
        let collected = collect(
            &entries,
            100,
            MAX_SNAPSHOT_TIME,
            || Duration::ZERO,
            |_, _, remaining| {
                let size = sizes[next];
                next += 1;
                if size > remaining {
                    Err(LossReason::ByteBudget)
                } else {
                    Ok(vec![0; size])
                }
            },
        );
        let outcomes: Vec<_> = collected.rows.iter().map(|r| r.outcome).collect();
        assert_eq!(
            outcomes,
            vec![
                RowOutcome::Saved {
                    bytes: 10,
                    useful: true
                },
                RowOutcome::Saved {
                    bytes: 60,
                    useful: true
                },
                RowOutcome::Lost(LossReason::ByteBudget),
                RowOutcome::Saved {
                    bytes: 20,
                    useful: true
                },
            ]
        );
    }

    #[test]
    fn byte_budget_boundary_is_inclusive() {
        let entries = [entry(CF_UNICODETEXT, None), entry(CF_TEXT, None)];
        let collected = collect(
            &entries,
            8,
            MAX_SNAPSHOT_TIME,
            || Duration::ZERO,
            ok_reader(4),
        );
        assert_eq!(collected.saved.len(), 2, "4 + 4 = 8 passt genau");
        // Ein Leser, der die Grenze ignoriert, wird trotzdem abgerechnet.
        let collected = collect(
            &entries,
            6,
            MAX_SNAPSHOT_TIME,
            || Duration::ZERO,
            |_, _, _| Ok(vec![0; 4]),
        );
        assert_eq!(
            collected.rows[1].outcome,
            RowOutcome::Lost(LossReason::ByteBudget)
        );
    }

    #[test]
    fn time_budget_loses_all_remaining_formats() {
        let entries = [
            entry(CF_UNICODETEXT, None),
            entry(0xC0A1, Some(HTML)),
            entry(CF_BITMAP, None),
            entry(CF_DIB, None),
            entry(CF_LOCALE, None),
        ];
        let clock = std::cell::Cell::new(Duration::ZERO);
        let collected = collect(
            &entries,
            MAX_SNAPSHOT_BYTES,
            Duration::from_secs(1),
            || clock.get(),
            |e, _, _| {
                // Das HTML hängt 3 s bei der Quelle.
                if e.id == 0xC0A1 {
                    clock.set(clock.get() + Duration::from_secs(3));
                }
                Ok(vec![1])
            },
        );
        let outcomes: Vec<_> = collected.rows.iter().map(|r| r.outcome).collect();
        assert_eq!(
            outcomes,
            vec![
                RowOutcome::Saved {
                    bytes: 1,
                    useful: true
                },
                // Schon angefordert: bleibt gesichert (weiches Limit).
                RowOutcome::Saved {
                    bytes: 1,
                    useful: true
                },
                // DIB nach Ablauf verloren → das Bitmap hat kein Gegenstück.
                RowOutcome::Lost(LossReason::NoCounterpart),
                RowOutcome::Lost(LossReason::TimeBudget),
                RowOutcome::Lost(LossReason::TimeBudget),
            ]
        );
    }

    #[test]
    fn snapshot_kind_needs_a_useful_format() {
        let row = |outcome| FormatRow {
            format: FormatRef::new(1, None),
            outcome,
        };
        assert_eq!(snapshot_kind(0, &[]), SnapshotKind::Empty);
        assert_eq!(
            snapshot_kind(
                1,
                &[row(RowOutcome::Saved {
                    bytes: 1,
                    useful: true
                })]
            ),
            SnapshotKind::Formats
        );
        // Nur Begleitformate gesichert → Unrestorable.
        assert_eq!(
            snapshot_kind(
                2,
                &[
                    row(RowOutcome::Saved {
                        bytes: 4,
                        useful: false
                    }),
                    row(RowOutcome::Lost(LossReason::NoData)),
                ]
            ),
            SnapshotKind::Unrestorable
        );
        assert_eq!(
            snapshot_kind(1, &[row(RowOutcome::Replaced)]),
            SnapshotKind::Unrestorable
        );
    }

    #[test]
    fn byte_sizes_use_a_decimal_comma() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1_500), "1,5 kB");
        assert_eq!(format_bytes(66_400_000), "66,4 MB");
    }

    #[test]
    fn snapshot_log_line_has_counts_and_no_content() {
        let report = SnapshotReport {
            rows: vec![
                FormatRow {
                    format: FormatRef::new(CF_UNICODETEXT, None),
                    outcome: RowOutcome::Saved {
                        bytes: 2_000,
                        useful: true,
                    },
                },
                FormatRow {
                    format: FormatRef::new(CF_BITMAP, None),
                    outcome: RowOutcome::Replaced,
                },
                FormatRow {
                    format: FormatRef::new(0xC010, Some("DataObject")),
                    outcome: RowOutcome::OleDropped,
                },
                FormatRow {
                    format: FormatRef::new(CF_OWNERDISPLAY, None),
                    outcome: RowOutcome::Lost(LossReason::NeverCopyable),
                },
            ],
            duration: Duration::from_millis(14),
            own: false,
        };
        assert_eq!(
            snapshot_log_line(&report),
            "Clipboard-Snapshot: 4 Formate (1 gesichert, 1 ersetzt, 1 OLE), 2,0 kB, 14 ms, \
             verloren 1 · verloren: 0x0080 [nie kopierbar] · ersetzt: 0x0002 · OLE: 0xC010 \"DataObject\""
        );
        let own = SnapshotReport {
            own: true,
            ..SnapshotReport::empty(true)
        };
        assert_eq!(
            snapshot_log_line(&own),
            "Clipboard-Snapshot: 0 Formate (0 gesichert, 0 ersetzt, 0 OLE), 0 B, 0 ms, verloren 0 · eigener Inhalt"
        );
    }

    #[test]
    fn partial_detail_names_both_phases() {
        let save = [LostFormat {
            format: FormatRef::new(CF_OWNERDISPLAY, None),
            reason: LossReason::NeverCopyable,
            phase: Phase::Save,
        }];
        let restore = [LostFormat {
            format: FormatRef::new(0xC0A1, Some(HTML)),
            reason: LossReason::SetFailed(5),
            phase: Phase::Restore,
        }];
        assert_eq!(
            partial_detail(&save, &restore),
            "Sichern: 0x0080 [nie kopierbar]; Zurückschreiben: 0xC0A1 \"HTML Format\" [SetClipboardData 5]"
        );
        assert_eq!(
            partial_detail(&save, &[]),
            "Sichern: 0x0080 [nie kopierbar]"
        );
        assert_eq!(
            partial_detail(&[], &restore),
            "Zurückschreiben: 0xC0A1 \"HTML Format\" [SetClipboardData 5]"
        );
    }
}
