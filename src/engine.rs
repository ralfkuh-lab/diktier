//! Transcriber-Vertrag (Spec §5.1) und Parakeet-TDT-Engine (Phase 1).
#![allow(dead_code)]

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use parakeet_rs::{ExecutionConfig, ParakeetTDT};
use thiserror::Error;

use crate::audio::ENGINE_RATE;
use crate::download::{self, ArtifactManifest, DownloadError};

/// 16 kHz × 250 ms. Kürzer → kein Engine-Aufruf (Spec §6.4).
pub const MIN_SAMPLES_16KHZ: usize = 16_000 * 250 / 1_000;

/// Fensterbreite des Gates: 250 ms bei 16 kHz (Spec §6.4).
const GATE_WINDOW: usize = MIN_SAMPLES_16KHZ;

/// Regel B1/B2: „sicher laut“ (Spec §6.4). `0.0075` ≈ −42,5 dBFS.
///
/// Kalibrierung 2026-08 (docs/SPIKES.md): `rauschen.wav` RMS ≈ 0,0051
/// bleibt darunter, die leiseste Sprachaufnahme `alltag.wav` ≈ 0,0215
/// klar darüber.
pub const RMS_SILENCE_THRESHOLD: f32 = 0.0075;

/// Regel B2: Mindestdauer eines Laufs über [`RMS_SILENCE_THRESHOLD`].
pub const MIN_SPEECH_RUN_ABS_SECS: f32 = 2.0;

/// Regel B3 (v1.7): „leise, aber sicher Sprache“ — 0,004 ≈ −48 dBFS.
///
/// Kalibrierung 2026-09-21 (docs/SPIKES.md): leise Diktate halten über dieser
/// Grenze 4,0–4,5 s durch, Störgeräusche (Stuhl, Kabel, Klick, Atmen)
/// höchstens 1,0 s. Die Laufdauer ist dieselbe wie bei B2
/// ([`MIN_SPEECH_RUN_ABS_SECS`]).
pub const QUIET_SPEECH_RMS: f32 = 0.004;

/// Vorlauf-Stille (Spec §6.4, v1.9): 300 ms digitale Nullen, die
/// [`transcribe_pcm`] einem freigegebenen Puffer voranstellt, bevor die Engine
/// ihn bekommt. Gegen das erfundene „Herr Präsident.“ am Diktatanfang, das
/// schon feines Rauschen im Vorlauf auslöst (docs/SPIKES.md 2026-09-30).
/// Gate, Report und Dauer sehen die Stille nicht.
pub const LEAD_IN_SILENCE_SAMPLES: usize = 4800;

/// Regel C/D: absolute Untergrenze, −70,5 dBFS ≈ 9,8 LSB bei 16 bit.
/// Fenster darunter sind nie aktiv und unterbrechen einen Lauf.
pub const ABS_FLOOR: f32 = 0.0003;

/// Regel D: geforderter Abstand eines aktiven Fensters über dem Grundrauschen.
pub const RELATIVE_MARGIN_DB: f32 = 12.0;

/// Regel D: absolute Aktivitätsgrenze, −60 dBFS ≈ 33 LSB bei 16 bit.
/// Verhindert, dass ein Grundrauschen nahe null jede Regung aktiv macht
/// (Astra B1).
pub const MIN_ACTIVE_RMS: f32 = 0.0010;

/// Regel D: Mindestdauer eines Laufs über der relativen Schwelle.
pub const MIN_SPEECH_RUN_REL_SECS: f32 = 1.5;

/// Anteil der leisesten vollen Fenster, der das Grundrauschen schätzt
/// (Nearest-Rank, siehe [`noise_floor`]).
const FLOOR_QUANTILE: f32 = 0.1;

/// RMS des 16-kHz-f32-Signals über den gegebenen Ausschnitt.
pub fn rms_f32(pcm_f32_16khz: &[f32]) -> f32 {
    if pcm_f32_16khz.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = pcm_f32_16khz
        .iter()
        .map(|&s| {
            let x = f64::from(s);
            x * x
        })
        .sum();
    (sum_sq / pcm_f32_16khz.len() as f64).sqrt() as f32
}

/// Ein Gate-Fenster mit seiner echten Sample-Zahl — das letzte Fenster einer
/// Aufnahme ist in der Regel kürzer als 250 ms (Astra B3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    pub rms: f32,
    pub samples: usize,
}

impl Window {
    /// Volles 250-ms-Fenster? Nur solche schätzen das Grundrauschen.
    pub fn is_full(&self) -> bool {
        self.samples == GATE_WINDOW
    }
}

/// Nicht überlappende Fenster von 250 ms ab Sample 0; das letzte darf kürzer
/// sein und behält seine echte Sample-Zahl (Spec §6.4).
pub fn window_rms(pcm_f32_16khz: &[f32]) -> Vec<Window> {
    pcm_f32_16khz
        .chunks(GATE_WINDOW)
        .map(|chunk| Window {
            rms: rms_f32(chunk),
            samples: chunk.len(),
        })
        .collect()
}

/// Maximum der Fenster-RMS (inklusive Restfenster); `0.0` ohne Fenster.
pub fn max_window_rms_of(windows: &[Window]) -> f32 {
    windows.iter().fold(0.0_f32, |acc, w| acc.max(w.rms))
}

/// Maximum der RMS-Werte über nicht überlappende 250-ms-Fenster.
/// Lange Stille plus kurze leise Sprache bleibt so über der Schwelle
/// (agy B3 / codex N1).
pub fn max_window_rms(pcm_f32_16khz: &[f32]) -> f32 {
    max_window_rms_of(&window_rms(pcm_f32_16khz))
}

/// Grundrauschen: RMS der **vollen** Fenster aufsteigend sortiert, davon
/// Element `⌊0,1 · (n − 1)⌋` (Nearest-Rank nach unten, ohne Interpolation;
/// für `n ≤ 10` ist das das Minimum, Spec §6.4).
///
/// `None`, wenn kein volles Fenster vorliegt — dann greift ohnehin Regel A.
pub fn noise_floor(windows: &[Window]) -> Option<f32> {
    let mut full: Vec<f32> = windows
        .iter()
        .filter(|w| w.is_full())
        .map(|w| w.rms)
        .collect();
    if full.is_empty() {
        return None;
    }
    full.sort_by(|a, b| a.partial_cmp(b).expect("endliche RMS-Werte"));
    let n = full.len();
    let index = (FLOOR_QUANTILE * (n - 1) as f32).floor() as usize;
    Some(full[index.min(n - 1)])
}

/// Längster zusammenhängender Lauf von Fenstern mit `rms >= threshold`, in
/// **Samples** (Restfenster zählt mit seiner echten Sample-Zahl).
///
/// Fenster mit `rms < break_below` sind nie aktiv und unterbrechen einen Lauf —
/// digitale Nullen dürfen Läufe weder verlängern noch verbinden (Astra B1).
pub fn longest_run_samples(windows: &[Window], threshold: f32, break_below: f32) -> usize {
    let mut run = 0_usize;
    let mut best = 0_usize;
    for w in windows {
        if w.rms >= threshold && w.rms >= break_below {
            run += w.samples;
            best = best.max(run);
        } else {
            run = 0;
        }
    }
    best
}

fn samples_to_secs(samples: usize) -> f32 {
    samples as f32 / 16_000.0
}

/// Warum der Silence-Gate die Engine nicht aufgerufen hat (Spec §6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SilenceGate {
    /// Regel A: kürzer als 250 ms.
    TooShort,
    /// Regel C: kein Fenster erreicht [`ABS_FLOOR`] — Pegelgrenze, **kein**
    /// Gerätebefund (die Geräte-Recovery aus §10 ist davon unabhängig).
    BelowAbsoluteFloor,
    /// Regel D: kein aktiver Lauf von [`MIN_SPEECH_RUN_REL_SECS`].
    NoRelativeRun,
    /// Eingabe enthält nicht-endliche Samples (NaN/Inf). Keine Bereinigung im
    /// Gate (Astra K1).
    InvalidInput { non_finite: usize },
}

impl SilenceGate {
    fn reason(&self) -> String {
        match self {
            Self::TooShort => format!(
                "Regel A: zu kurz (< {:.3} s)",
                samples_to_secs(MIN_SAMPLES_16KHZ)
            ),
            Self::BelowAbsoluteFloor => {
                format!("Regel C: max. Fenster unter {ABS_FLOOR:.5}")
            }
            Self::NoRelativeRun => {
                format!("Regel D: kein aktiver Lauf ≥ {MIN_SPEECH_RUN_REL_SECS:.2} s")
            }
            Self::InvalidInput { non_finite } => {
                format!("nicht-endliche Samples ({non_finite})")
            }
        }
    }
}

impl fmt::Display for SilenceGate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason())
    }
}

/// Welche Ja-Regel die Engine freigegeben hat (Spec §6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechRule {
    /// B1: Gesamt-RMS ≥ [`RMS_SILENCE_THRESHOLD`].
    B1,
    /// B2: absoluter Lauf ≥ [`MIN_SPEECH_RUN_ABS_SECS`].
    B2,
    /// B3: Lauf ≥ [`MIN_SPEECH_RUN_ABS_SECS`] über [`QUIET_SPEECH_RMS`].
    B3,
    /// D: relativer Lauf ≥ [`MIN_SPEECH_RUN_REL_SECS`] über dem Grundrauschen.
    D,
}

impl SpeechRule {
    fn reason(&self) -> String {
        match self {
            Self::B1 => format!("Regel B1: Gesamt-RMS ≥ {RMS_SILENCE_THRESHOLD:.5}"),
            Self::B2 => format!(
                "Regel B2: Lauf ≥ {MIN_SPEECH_RUN_ABS_SECS:.2} s über {RMS_SILENCE_THRESHOLD:.5}"
            ),
            Self::B3 => format!(
                "Regel B3: Lauf ≥ {MIN_SPEECH_RUN_ABS_SECS:.2} s über {QUIET_SPEECH_RMS:.5}"
            ),
            Self::D => format!("Regel D: Lauf ≥ {MIN_SPEECH_RUN_REL_SECS:.2} s über Schwelle D"),
        }
    }
}

impl fmt::Display for SpeechRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason())
    }
}

/// Entscheidung des Gates: Engine rufen (mit der Regel, die es erlaubt) oder
/// leeres Ergebnis (mit dem Grund).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateDecision {
    Speech(SpeechRule),
    Rejected(SilenceGate),
}

impl GateDecision {
    pub fn is_rejected(&self) -> bool {
        matches!(self, Self::Rejected(_))
    }

    pub fn rejected(&self) -> Option<SilenceGate> {
        match self {
            Self::Rejected(reason) => Some(*reason),
            Self::Speech(_) => None,
        }
    }
}

/// Messwerte einer Aufnahme. Fehlt nur bei [`SilenceGate::InvalidInput`] — dort
/// gibt es keine gültigen Zahlen, und der Gate erfindet keine (Astra W4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GateMetrics {
    /// Fenster mit vollen 250 ms (Basis des Grundrauschens).
    pub full_windows: usize,
    /// Fenster insgesamt, inklusive Restfenster.
    pub windows: usize,
    pub rms: f32,
    pub max_window_rms: f32,
    /// `None`, wenn kein volles Fenster vorliegt (nur bei Regel A).
    pub floor: Option<f32>,
    /// `max(floor · 10^(12/20), MIN_ACTIVE_RMS)`; `None` ohne Grundrauschen.
    pub threshold_d: Option<f32>,
    /// Längster Lauf über [`RMS_SILENCE_THRESHOLD`] (Regel B2).
    pub longest_abs_run_secs: f32,
    /// Längster Lauf über [`QUIET_SPEECH_RMS`] (Regel B3).
    pub longest_quiet_run_secs: f32,
    /// Längster Lauf über [`GateMetrics::threshold_d`] (Regel D).
    pub longest_rel_run_secs: f32,
}

/// Entscheidung plus Messwerte einer Aufnahme — die Datenbasis für
/// Nachkalibrierung im Betrieb (§10: nur Zahlen, kein Audio, kein Text).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GateReport {
    pub decision: GateDecision,
    pub samples: usize,
    pub metrics: Option<GateMetrics>,
}

impl GateReport {
    pub fn is_rejected(&self) -> bool {
        self.decision.is_rejected()
    }
}

impl fmt::Display for GateReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (verdict, reason) = match &self.decision {
            GateDecision::Speech(rule) => ("Engine", rule.reason()),
            GateDecision::Rejected(reason) => ("leer", reason.reason()),
        };
        write!(
            f,
            "{verdict} ({reason}) — {} Samples ({:.3} s)",
            self.samples,
            samples_to_secs(self.samples)
        )?;
        let Some(m) = &self.metrics else {
            return f.write_str(", keine Messwerte");
        };
        write!(
            f,
            ", Fenster {}/{} voll, RMS {:.5}, max. Fenster {:.5}, floor ",
            m.full_windows, m.windows, m.rms, m.max_window_rms
        )?;
        match m.floor {
            Some(floor) => write!(f, "{floor:.5}")?,
            None => f.write_str("—")?,
        }
        f.write_str(", Schwelle D ")?;
        match m.threshold_d {
            Some(thr) => write!(f, "{thr:.5}")?,
            None => f.write_str("—")?,
        }
        write!(
            f,
            ", Lauf abs {:.2} s, Lauf {QUIET_SPEECH_RMS:.3} {:.2} s, Lauf rel {:.2} s",
            m.longest_abs_run_secs, m.longest_quiet_run_secs, m.longest_rel_run_secs
        )
    }
}

/// Silence-Gate nach Spec §6.4 — deterministisch, ohne Engine.
///
/// **Eingabe:** 16-kHz-Mono-f32. Nicht-endliche Samples ergeben
/// [`SilenceGate::InvalidInput`]; der Gate bereinigt nichts selbst (Astra K1).
///
/// **Fensterung:** Fenster von 4000 Samples (250 ms) ab Sample 0, nicht
/// überlappend. Das letzte Fenster darf kürzer sein; es geht mit seiner echten
/// Sample-Zahl in Maximum und Laufdauern ein, **nicht** in das Grundrauschen.
/// Laufdauern zählen Samples und werden erst zur Ausgabe in Sekunden
/// umgerechnet.
///
/// **Grundrauschen:** RMS der vollen Fenster aufsteigend sortiert, Element
/// `⌊0,1 · (n − 1)⌋` (Nearest-Rank nach unten; für `n ≤ 10` das Minimum).
///
/// **Regeln, die erste zutreffende entscheidet:**
///
/// - **A** `len < 4000` → leer ([`SilenceGate::TooShort`]).
/// - **B1** Gesamt-RMS ≥ [`RMS_SILENCE_THRESHOLD`] → Engine.
/// - **B2** Lauf von Fenstern ≥ [`RMS_SILENCE_THRESHOLD`] über
///   [`MIN_SPEECH_RUN_ABS_SECS`] → Engine.
/// - **B3** Lauf von Fenstern ≥ [`QUIET_SPEECH_RMS`] über
///   [`MIN_SPEECH_RUN_ABS_SECS`] → Engine (v1.7; leises Sprechen ohne Pause,
///   bei dem D keinen Kontrast findet).
/// - **C** max. Fenster-RMS < [`ABS_FLOOR`] → leer
///   ([`SilenceGate::BelowAbsoluteFloor`]). Pegelgrenze, kein Gerätebefund.
/// - **D** `thr = max(floor · 10^(12/20), MIN_ACTIVE_RMS)`; Fenster ≥ `thr`
///   sind aktiv, Fenster unter [`ABS_FLOOR`] nie. Längster aktiver Lauf ≥
///   [`MIN_SPEECH_RUN_REL_SECS`] → Engine, sonst leer
///   ([`SilenceGate::NoRelativeRun`]).
///
/// B1 und B2 sind die Ja-Pfade von vor v1.6 und bleiben erhalten: der Umbau
/// darf kein Signal verwerfen, das bisher durchkam (Astra B2/F2). D kommt
/// hinzu, weil die Engine pegelrobust ist — `alltag.wav` um 22 dB abgesenkt
/// wird wortidentisch erkannt (docs/silence-gate-plan.md, 2026-09-21).
///
/// Der Gate ist Halluzinationsschutz gegen Stille und Rauschen ohne Sprache,
/// **keine** Sprachklassifikation. Bekannte Grenzen (Spec §6.4): leise Diktate
/// ohne 1,5 s zusammenhängenden Kontrast und Signale unter [`MIN_ACTIVE_RMS`]
/// bzw. [`ABS_FLOOR`] bleiben verworfen, sofern nicht B1/B2/B3 greifen. Ein
/// gleichmäßiges Geräusch zwischen [`QUIET_SPEECH_RMS`] und
/// [`RMS_SILENCE_THRESHOLD`] über ≥ 2 s erreicht seit v1.7 die Engine; der
/// Schutz ist dort die Engine selbst (SPEC §18 #13).
///
/// Messwerte sind vollständig — auch bei Annahme —, damit sich die Konstanten
/// aus dem Betriebslog nachkalibrieren lassen (Astra W4).
pub fn silence_gate(pcm_f32_16khz: &[f32]) -> GateReport {
    let samples = pcm_f32_16khz.len();
    let non_finite = pcm_f32_16khz.iter().filter(|s| !s.is_finite()).count();
    if non_finite > 0 {
        return GateReport {
            decision: GateDecision::Rejected(SilenceGate::InvalidInput { non_finite }),
            samples,
            metrics: None,
        };
    }

    let windows = window_rms(pcm_f32_16khz);
    let floor = noise_floor(&windows);
    let threshold_d = floor.map(|f| (f * db_to_ratio(RELATIVE_MARGIN_DB)).max(MIN_ACTIVE_RMS));
    let metrics = GateMetrics {
        full_windows: windows.iter().filter(|w| w.is_full()).count(),
        windows: windows.len(),
        rms: rms_f32(pcm_f32_16khz),
        max_window_rms: max_window_rms_of(&windows),
        floor,
        threshold_d,
        longest_abs_run_secs: samples_to_secs(longest_run_samples(
            &windows,
            RMS_SILENCE_THRESHOLD,
            ABS_FLOOR,
        )),
        longest_quiet_run_secs: samples_to_secs(longest_run_samples(
            &windows,
            QUIET_SPEECH_RMS,
            ABS_FLOOR,
        )),
        longest_rel_run_secs: threshold_d.map_or(0.0, |thr| {
            samples_to_secs(longest_run_samples(&windows, thr, ABS_FLOOR))
        }),
    };

    let decision = decide(samples, &metrics);
    GateReport {
        decision,
        samples,
        metrics: Some(metrics),
    }
}

/// Regeln A, B1, B2, B3, C, D in dieser Reihenfolge — die erste zutreffende entscheidet.
fn decide(samples: usize, m: &GateMetrics) -> GateDecision {
    if samples < MIN_SAMPLES_16KHZ {
        return GateDecision::Rejected(SilenceGate::TooShort);
    }
    if m.rms >= RMS_SILENCE_THRESHOLD {
        return GateDecision::Speech(SpeechRule::B1);
    }
    if m.longest_abs_run_secs >= MIN_SPEECH_RUN_ABS_SECS {
        return GateDecision::Speech(SpeechRule::B2);
    }
    if m.longest_quiet_run_secs >= MIN_SPEECH_RUN_ABS_SECS {
        return GateDecision::Speech(SpeechRule::B3);
    }
    if m.max_window_rms < ABS_FLOOR {
        return GateDecision::Rejected(SilenceGate::BelowAbsoluteFloor);
    }
    if m.longest_rel_run_secs >= MIN_SPEECH_RUN_REL_SECS {
        return GateDecision::Speech(SpeechRule::D);
    }
    GateDecision::Rejected(SilenceGate::NoRelativeRun)
}

/// Amplitudenverhältnis zu einer dB-Angabe: `10^(db/20)`.
pub fn db_to_ratio(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

/// Der Gate lehnt ab — Engine nicht laden/aufrufen.
pub fn is_silence_or_short(pcm_f32_16khz: &[f32]) -> bool {
    silence_gate(pcm_f32_16khz).decision.is_rejected()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcription {
    pub text: String,
    pub language: Option<String>,
    pub timing: Option<Timing>,
}

impl Transcription {
    pub fn empty() -> Self {
        Self {
            text: String::new(),
            language: None,
            timing: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timing {
    pub duration: Duration,
}

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("{0}")]
    Ort(String),
    #[error("{0}")]
    Artifacts(String),
    #[error("Transkription fehlgeschlagen: {0}")]
    Failed(String),
}

pub trait Transcriber {
    fn transcribe(&mut self, pcm_f32_16khz: &[f32]) -> Result<Transcription, EngineError>;
}

/// [`silence_gate`] genau einmal rechnen und die Engine nur bei Freigabe
/// rufen (Spec §6.4 / §12). Der zurückgegebene [`GateReport`] ist die
/// Logzeile des Aufrufers — er soll den Gate nicht ein zweites Mal rechnen
/// (Astra K2).
///
/// Der Report steht **neben** dem Ergebnis und liegt deshalb auch vor, wenn
/// die Inferenz scheitert: §6.4 verlangt die Gate-Zeile pro Aufnahme, nicht
/// pro geglückter Aufnahme.
///
/// Bei Ablehnung: leeres Transkript, **kein** Engine-Aufruf.
///
/// Bei Freigabe bekommt die Engine den Puffer mit
/// [`LEAD_IN_SILENCE_SAMPLES`] vorangestellten Nullen (Spec §6.4, v1.9). Das
/// geschieht nur hier, damit Daemon, `--transcribe-wav`, `--record-test` und
/// stt-smoke gleich rechnen. Die Dauer im Ergebnis setzt ebenfalls nur diese
/// Funktion, und zwar aus der Originallänge ohne Stille.
pub fn transcribe_pcm<T: Transcriber>(
    engine: &mut T,
    pcm_f32_16khz: &[f32],
) -> (GateReport, Result<Transcription, EngineError>) {
    let report = silence_gate(pcm_f32_16khz);
    if report.is_rejected() {
        return (report, Ok(Transcription::empty()));
    }
    let mut padded = Vec::with_capacity(LEAD_IN_SILENCE_SAMPLES + pcm_f32_16khz.len());
    padded.resize(LEAD_IN_SILENCE_SAMPLES, 0.0);
    padded.extend_from_slice(pcm_f32_16khz);
    let result = engine.transcribe(&padded).map(|mut out| {
        out.timing = Some(Timing {
            duration: Duration::from_secs_f64(pcm_f32_16khz.len() as f64 / f64::from(ENGINE_RATE)),
        });
        out
    });
    (report, result)
}

#[derive(Debug, Default)]
pub struct StubTranscriber;

impl Transcriber for StubTranscriber {
    fn transcribe(&mut self, _pcm_f32_16khz: &[f32]) -> Result<Transcription, EngineError> {
        Ok(Transcription::empty())
    }
}

/// TDT über `parakeet-rs`. Kein eigener Decoder.
pub struct ParakeetTranscriber {
    inner: ParakeetTDT,
}

impl ParakeetTranscriber {
    pub fn load(model_key: &str, threads: u32) -> Result<Self, EngineError> {
        let manifest = download::load_manifest().map_err(artifacts_err)?;
        if manifest.key != model_key {
            return Err(EngineError::Artifacts(format!(
                "engine.model {model_key:?} passt nicht zum Manifest {}",
                manifest.key
            )));
        }
        ensure_ort_initialized()?;
        let dir = download::model_dir(model_key).map_err(artifacts_err)?;
        download::check_artifacts(&dir, &manifest).map_err(artifacts_err)?;

        let exec = if threads == 0 {
            // 0 = Runtime-Default von parakeet-rs (intra=4, inter=1).
            None
        } else {
            Some(ExecutionConfig::default().with_intra_threads(threads as usize))
        };

        let inner = ParakeetTDT::from_pretrained(&dir, exec)
            .map_err(|e| EngineError::Failed(format!("parakeet-rs: {e}")))?;
        Ok(Self { inner })
    }
}

impl Transcriber for ParakeetTranscriber {
    fn transcribe(&mut self, pcm_f32_16khz: &[f32]) -> Result<Transcription, EngineError> {
        let result = parakeet_rs::Transcriber::transcribe_samples(
            &mut self.inner,
            pcm_f32_16khz.to_vec(),
            ENGINE_RATE,
            1,
            None,
        )
        .map_err(|e| EngineError::Failed(format!("parakeet-rs: {e}")))?;
        // Die Dauer setzt `transcribe_pcm` aus der Länge ohne Vorlauf-Stille.
        Ok(Transcription {
            text: result.text,
            language: None,
            timing: None,
        })
    }
}

fn artifacts_err(err: DownloadError) -> EngineError {
    EngineError::Artifacts(err.to_string())
}

/// ONNX Runtime nur über `ort::init_from`, Pfad relativ zu `current_exe()`.
///
/// Suchreihenfolge (absolute Pfade):
/// 1. `lib/<name>` neben der Binary (Bundle-Layout, Spec §11)
/// 2. `../lib/<name>` (Cargo-Test: `deps/` → `../lib`)
pub fn resolve_ort_lib() -> Result<PathBuf, EngineError> {
    let exe = std::env::current_exe()
        .map_err(|e| EngineError::Ort(format!("current_exe() fehlgeschlagen: {e}")))?;
    let exe_dir = exe
        .parent()
        .ok_or_else(|| EngineError::Ort("current_exe() hat kein Elternverzeichnis".into()))?;
    resolve_ort_lib_from_exe_dir(exe_dir)
}

pub(crate) fn resolve_ort_lib_from_exe_dir(exe_dir: &Path) -> Result<PathBuf, EngineError> {
    let name = ort_lib_filename();
    let candidates = [
        exe_dir.join("lib").join(name),
        exe_dir.join("..").join("lib").join(name),
    ];
    for candidate in &candidates {
        if candidate.is_file() {
            return Ok(abspath(candidate));
        }
    }
    Err(EngineError::Ort(format!(
        "ONNX-Runtime-Library {name} nicht gefunden (gesucht: {} und {}; scripts/fetch-ort.ps1)",
        candidates[0].display(),
        candidates[1].display()
    )))
}

pub(crate) fn ort_lib_filename() -> &'static str {
    "onnxruntime.dll"
}

fn abspath(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn ensure_ort_initialized() -> Result<(), EngineError> {
    static INIT: OnceLock<PathBuf> = OnceLock::new();
    static LOCK: Mutex<()> = Mutex::new(());

    if INIT.get().is_some() {
        return Ok(());
    }
    let _guard = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if INIT.get().is_some() {
        return Ok(());
    }

    let path = resolve_ort_lib()?;
    let committed = ort::init_from(&path)
        .map_err(|e| EngineError::Ort(e.to_string()))?
        .with_telemetry(false)
        .commit();
    if !committed {
        return Err(EngineError::Ort(
            "ORT-Umgebung konnte nicht committet werden (bereits fremd initialisiert)".into(),
        ));
    }
    let _ = INIT.set(path);
    Ok(())
}

/// Für Tests und stt-smoke: Artefaktverzeichnis plus Manifest.
pub fn model_artifacts(model_key: &str) -> Result<(PathBuf, ArtifactManifest), EngineError> {
    let manifest = download::load_manifest().map_err(artifacts_err)?;
    let dir = download::model_dir(model_key).map_err(artifacts_err)?;
    Ok((dir, manifest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_MODEL;
    use std::time::Instant;

    struct CountingStub {
        calls: usize,
    }

    impl CountingStub {
        fn new() -> Self {
            Self { calls: 0 }
        }
    }

    impl Transcriber for CountingStub {
        fn transcribe(&mut self, pcm_f32_16khz: &[f32]) -> Result<Transcription, EngineError> {
            self.calls += 1;
            Ok(Transcription {
                text: format!("{}", pcm_f32_16khz.len()),
                language: None,
                timing: None,
            })
        }
    }

    /// Zählt die echten Engine-Aufrufe um einen beliebigen Transcriber herum —
    /// im stt-smoke um [`ParakeetTranscriber`] (Astra W3: „leer“ allein belegt
    /// den Gate nicht).
    struct CountingEngine<T: Transcriber> {
        inner: T,
        calls: usize,
    }

    impl<T: Transcriber> Transcriber for CountingEngine<T> {
        fn transcribe(&mut self, pcm_f32_16khz: &[f32]) -> Result<Transcription, EngineError> {
            self.calls += 1;
            self.inner.transcribe(pcm_f32_16khz)
        }
    }

    /// Konstantes Signal: RMS = |level|.
    fn level(value: f32, samples: usize) -> Vec<f32> {
        vec![value; samples]
    }

    /// Aneinandergereihte Abschnitte `(Pegel, Samples)`.
    fn seq(parts: &[(f32, usize)]) -> Vec<f32> {
        let mut out = Vec::with_capacity(parts.iter().map(|(_, n)| n).sum());
        for &(value, n) in parts {
            out.extend(std::iter::repeat_n(value, n));
        }
        out
    }

    fn secs(n: f32) -> usize {
        (n * 16_000.0) as usize
    }

    /// Ein Durchlauf durch den geprüften Aufrufpfad: Entscheidung plus Beleg,
    /// ob die Engine wirklich lief.
    fn run_gate(pcm: &[f32]) -> (GateReport, usize, String) {
        let mut stub = CountingStub::new();
        let (report, result) = transcribe_pcm(&mut stub, pcm);
        let out = result.expect("Stub schlägt nie fehl");
        (report, stub.calls, out.text)
    }

    fn expect_speech(pcm: &[f32], rule: SpeechRule) -> GateReport {
        let (report, calls, text) = run_gate(pcm);
        assert_eq!(
            report.decision,
            GateDecision::Speech(rule),
            "erwartet {rule:?}, Report: {report}"
        );
        assert_eq!(calls, 1, "Engine muss laufen: {report}");
        // Die Engine sieht den Puffer samt Vorlauf-Stille (Spec §6.4, v1.9).
        assert_eq!(text, (pcm.len() + LEAD_IN_SILENCE_SAMPLES).to_string());
        report
    }

    fn expect_rejected(pcm: &[f32], reason: SilenceGate) -> GateReport {
        let (report, calls, text) = run_gate(pcm);
        assert_eq!(
            report.decision,
            GateDecision::Rejected(reason),
            "erwartet {reason:?}, Report: {report}"
        );
        assert_eq!(calls, 0, "Engine darf nicht laufen: {report}");
        assert!(text.is_empty());
        report
    }

    fn metrics(pcm: &[f32]) -> GateMetrics {
        silence_gate(pcm).metrics.expect("endliche Eingabe")
    }

    /// §6.4 verlangt die Gate-Zeile **pro Aufnahme**: scheitert die Inferenz,
    /// muss der Report trotzdem beim Aufrufer ankommen.
    #[test]
    fn report_survives_an_engine_error() {
        struct FailingStub;
        impl Transcriber for FailingStub {
            fn transcribe(&mut self, _pcm: &[f32]) -> Result<Transcription, EngineError> {
                Err(EngineError::Failed("kaputt".into()))
            }
        }

        let pcm = level(0.02, secs(3.0));
        let (report, result) = transcribe_pcm(&mut FailingStub, &pcm);
        assert_eq!(report.decision, GateDecision::Speech(SpeechRule::B1));
        assert!(report.metrics.is_some(), "{report}");
        assert!(matches!(result, Err(EngineError::Failed(_))));
    }

    /// Merkt sich den übergebenen Puffer. Im Erfolgsfall meldet er absichtlich
    /// eine falsche Dauer — `transcribe_pcm` muss sie durch die Originallänge
    /// ersetzen. Mit `fail` liefert er nach dem Aufzeichnen einen Fehler.
    struct RecordingStub {
        seen: Option<Vec<f32>>,
        fail: bool,
    }

    impl RecordingStub {
        fn ok() -> Self {
            Self {
                seen: None,
                fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                seen: None,
                fail: true,
            }
        }
    }

    impl Transcriber for RecordingStub {
        fn transcribe(&mut self, pcm_f32_16khz: &[f32]) -> Result<Transcription, EngineError> {
            assert!(self.seen.is_none(), "Engine zweimal gerufen");
            self.seen = Some(pcm_f32_16khz.to_vec());
            if self.fail {
                return Err(EngineError::Failed("kaputt".into()));
            }
            Ok(Transcription {
                text: "ok".into(),
                language: None,
                timing: Some(Timing {
                    duration: Duration::from_secs(999),
                }),
            })
        }
    }

    /// Der Engine-Puffer ist [`LEAD_IN_SILENCE_SAMPLES`] × `+0.0`, gefolgt vom
    /// bitgleichen Original.
    fn assert_lead_in_then_original(seen: &[f32], pcm: &[f32]) {
        assert_eq!(seen.len(), pcm.len() + LEAD_IN_SILENCE_SAMPLES);
        let (lead_in, rest) = seen.split_at(LEAD_IN_SILENCE_SAMPLES);
        assert!(
            lead_in.iter().all(|s| s.to_bits() == 0.0f32.to_bits()),
            "Vorlauf muss exakt +0.0 sein"
        );
        assert_eq!(rest.len(), pcm.len());
        assert!(
            rest.iter()
                .zip(pcm)
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "Original muss bitgleich folgen"
        );
    }

    /// Nicht-konstantes Signal (B1) mit Werten, die bei einem Versatz oder
    /// einer Rundung auffallen würden.
    fn wobble(samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|i| 0.02 * ((i as f32) * 0.05).sin() + 1e-6 * (i % 7) as f32)
            .collect()
    }

    /// Spec §6.4 (v1.9): 4800 Nullen vor dem unveränderten Original, Gate und
    /// Dauer ohne Stille.
    #[test]
    fn accepted_audio_gets_lead_in_silence() {
        assert_eq!(LEAD_IN_SILENCE_SAMPLES, 16_000 * 300 / 1_000);
        let pcm = wobble(secs(3.0));

        let mut stub = RecordingStub::ok();
        let (report, result) = transcribe_pcm(&mut stub, &pcm);
        let out = result.expect("Stub schlägt hier nicht fehl");
        let seen = stub.seen.expect("Engine muss laufen");

        assert_lead_in_then_original(&seen, &pcm);
        assert_eq!(report, silence_gate(&pcm), "Report ohne Stille gerechnet");
        assert_eq!(report.samples, pcm.len());
        assert_eq!(
            out.timing,
            Some(Timing {
                duration: Duration::from_secs(3)
            }),
            "Dauer = Originallänge"
        );
        assert_eq!(out.text, "ok");
    }

    /// Keine Ganzsekunden: 48001 Samples = 3 s + 62 500 ns. Eine Trunkierung
    /// auf Sekunden oder Millisekunden fiele hier auf.
    #[test]
    fn duration_is_exact_for_a_fraction_of_a_second() {
        let pcm = wobble(48_001);

        let mut stub = RecordingStub::ok();
        let (report, result) = transcribe_pcm(&mut stub, &pcm);
        let out = result.expect("Stub schlägt hier nicht fehl");
        let seen = stub.seen.expect("Engine muss laufen");

        assert_eq!(report.decision, GateDecision::Speech(SpeechRule::B1));
        assert_lead_in_then_original(&seen, &pcm);
        assert_eq!(report, silence_gate(&pcm));
        assert_eq!(
            out.timing,
            Some(Timing {
                duration: Duration::new(3, 62_500)
            }),
            "Dauer = 48001 / 16000 s"
        );
    }

    /// Dieselben Zusagen, wenn Regel D freigibt: leise Sprache nach Stille,
    /// unter [`QUIET_SPEECH_RMS`], damit weder B1/B2 noch B3 greifen.
    #[test]
    fn lead_in_silence_also_on_rule_d() {
        let mut pcm = level(0.0, secs(10.0));
        pcm.extend((0..secs(3.0)).map(|i| if i % 2 == 0 { 0.0035 } else { -0.0035 }));

        let mut stub = RecordingStub::ok();
        let (report, result) = transcribe_pcm(&mut stub, &pcm);
        let out = result.expect("Stub schlägt hier nicht fehl");
        let seen = stub.seen.expect("Engine muss laufen");

        assert_eq!(
            report.decision,
            GateDecision::Speech(SpeechRule::D),
            "{report}"
        );
        assert_lead_in_then_original(&seen, &pcm);
        assert_eq!(report, silence_gate(&pcm), "Report ohne Stille gerechnet");
        assert_eq!(
            out.timing,
            Some(Timing {
                duration: Duration::from_secs(13)
            })
        );
    }

    /// Scheitert die Engine, bleiben Report und Engine-Puffer dieselben wie im
    /// Erfolgsfall; der Fehler kommt unverändert durch.
    #[test]
    fn engine_error_keeps_report_and_lead_in() {
        let pcm = wobble(secs(3.0));

        let mut stub = RecordingStub::failing();
        let (report, result) = transcribe_pcm(&mut stub, &pcm);
        let seen = stub.seen.expect("Engine muss laufen");

        assert_eq!(report, silence_gate(&pcm), "Report ohne Stille gerechnet");
        assert_lead_in_then_original(&seen, &pcm);
        assert!(
            matches!(result, Err(EngineError::Failed(ref m)) if m == "kaputt"),
            "{result:?}"
        );
    }

    #[test]
    fn rejected_audio_never_reaches_the_engine() {
        let mut stub = RecordingStub::ok();
        let pcm = level(0.0, secs(3.0));
        let (report, result) = transcribe_pcm(&mut stub, &pcm);
        assert!(report.is_rejected(), "{report}");
        assert!(stub.seen.is_none(), "Engine darf nicht laufen");
        assert_eq!(result.unwrap(), Transcription::empty());

        let short = level(0.1, MIN_SAMPLES_16KHZ - 1);
        let (report, _) = transcribe_pcm(&mut stub, &short);
        assert!(report.is_rejected(), "{report}");
        assert!(stub.seen.is_none(), "Engine darf nicht laufen");
    }

    #[test]
    fn stub_silence_yields_empty_transcript() {
        let mut engine = StubTranscriber;
        let out = engine.transcribe(&[]).unwrap();
        assert!(out.text.is_empty());
        assert!(out.language.is_none());
        assert!(out.timing.is_none());
    }

    // ------------------------------------------------------------- Regel A

    #[test]
    fn rule_a_too_short_below_4000_samples() {
        expect_rejected(&level(0.1, MIN_SAMPLES_16KHZ - 1), SilenceGate::TooShort);
        expect_speech(&level(0.1, MIN_SAMPLES_16KHZ), SpeechRule::B1);
    }

    #[test]
    fn audio_shorter_than_250ms_skips_engine() {
        let mut stub = CountingStub::new();
        let short = level(0.1, MIN_SAMPLES_16KHZ - 1);
        let (report, result) = transcribe_pcm(&mut stub, &short);
        assert!(result.unwrap().text.is_empty());
        assert_eq!(stub.calls, 0);
        assert!(report.is_rejected());
        assert_eq!(report.samples, MIN_SAMPLES_16KHZ - 1);

        let exact = level(0.1, MIN_SAMPLES_16KHZ);
        let (_, result) = transcribe_pcm(&mut stub, &exact);
        assert_eq!(stub.calls, 1);
        assert_eq!(
            result.unwrap().text,
            (MIN_SAMPLES_16KHZ + LEAD_IN_SILENCE_SAMPLES).to_string()
        );
    }

    // ------------------------------------------------------------ Regel B1

    #[test]
    fn rule_b1_is_exclusive_below_threshold() {
        let n = MIN_SAMPLES_16KHZ;
        let just_below = level(RMS_SILENCE_THRESHOLD * 0.999, n);
        let at = level(RMS_SILENCE_THRESHOLD, n);
        let just_above = level(RMS_SILENCE_THRESHOLD * 1.001, n);
        assert!(rms_f32(&just_below) < RMS_SILENCE_THRESHOLD);
        assert!((rms_f32(&at) - RMS_SILENCE_THRESHOLD).abs() < 1e-9);
        assert!(rms_f32(&just_above) > RMS_SILENCE_THRESHOLD);

        // Ein einziges Fenster: floor = max, D kann nie greifen (Astra W1).
        expect_rejected(&just_below, SilenceGate::NoRelativeRun);
        expect_speech(&at, SpeechRule::B1);
        expect_speech(&just_above, SpeechRule::B1);
    }

    #[test]
    fn rule_b1_lets_uniform_noise_through() {
        // Gleichmäßiges Rauschen auf 0,0080 passiert B1 — bewusst: „sicher
        // laut“ ist keine Sprachfeststellung, aber heutiges Verhalten (Astra B2).
        let noise = level(0.0080, secs(10.0));
        let report = expect_speech(&noise, SpeechRule::B1);
        let m = report.metrics.unwrap();
        assert_eq!(m.longest_rel_run_secs, 0.0, "kein Kontrast: {report}");
    }

    // ------------------------------------------------------------ Regel B2

    #[test]
    fn rule_b2_keeps_the_absolute_run_path() {
        // Astra B2: 8 s bei 0,003, dann 2 s bei 0,010. Gesamt-RMS ≈ 0,0052,
        // D-Schwelle ≈ 0,0119 — ohne B2 ginge dieses heute akzeptierte Signal
        // verloren.
        let pcm = seq(&[(0.003, secs(8.0)), (0.010, secs(2.0))]);
        let m = metrics(&pcm);
        assert!(m.rms < RMS_SILENCE_THRESHOLD, "{m:?}");
        assert!(m.threshold_d.unwrap() > 0.010, "{m:?}");
        expect_speech(&pcm, SpeechRule::B2);
    }

    #[test]
    fn rule_b2_needs_two_full_seconds() {
        let pcm = seq(&[(0.003, secs(8.0)), (0.010, secs(1.75))]);
        let m = metrics(&pcm);
        assert!((m.longest_abs_run_secs - 1.75).abs() < 1e-6, "{m:?}");
        // Der Lauf liegt auch über QUIET_SPEECH_RMS, bleibt aber unter den
        // 2,0 s, die B3 fordert — B3 rettet dieses Signal nicht (v1.7).
        assert!((m.longest_quiet_run_secs - 1.75).abs() < 1e-6, "{m:?}");
        expect_rejected(&pcm, SilenceGate::NoRelativeRun);
    }

    // ------------------------------------------------------------ Regel B3

    /// Nachbau von „Aufnahme 07“ (docs/SPIKES.md, 2026-09-21): leise Sprache
    /// ohne Pause — das Grundrauschen ist die Sprache selbst, D findet keinen
    /// Kontrast, B3 gibt frei.
    #[test]
    fn rule_b3_admits_quiet_speech_without_a_pause() {
        let pcm = seq(&[(0.003, secs(1.5)), (0.0055, secs(4.0)), (0.003, secs(1.5))]);
        let m = metrics(&pcm);
        assert!(m.rms < RMS_SILENCE_THRESHOLD, "{m:?}");
        assert_eq!(m.longest_abs_run_secs, 0.0, "B2 darf nicht greifen: {m:?}");
        assert!((m.longest_quiet_run_secs - 4.0).abs() < 1e-6, "{m:?}");
        assert!(
            m.threshold_d.unwrap() > m.max_window_rms,
            "D findet keinen Kontrast: {m:?}"
        );
        assert_eq!(m.longest_rel_run_secs, 0.0, "{m:?}");
        expect_speech(&pcm, SpeechRule::B3);
    }

    #[test]
    fn rule_b3_needs_two_full_seconds() {
        // 1,75 s über 0,004 in Stille: B3 greift nicht, dann entscheidet D.
        let pcm = seq(&[(0.0, secs(10.0)), (0.0055, secs(1.75))]);
        let m = metrics(&pcm);
        assert!((m.longest_quiet_run_secs - 1.75).abs() < 1e-6, "{m:?}");
        assert!((m.longest_rel_run_secs - 1.75).abs() < 1e-6, "{m:?}");
        expect_speech(&pcm, SpeechRule::D);

        // Dasselbe ohne Kontrast (floor = Sprache) bleibt leer.
        let flat = seq(&[(0.003, secs(10.0)), (0.0055, secs(1.75))]);
        let m = metrics(&flat);
        assert!((m.longest_quiet_run_secs - 1.75).abs() < 1e-6, "{m:?}");
        expect_rejected(&flat, SilenceGate::NoRelativeRun);
    }

    #[test]
    fn rule_b3_run_is_exact_at_32000_samples() {
        let quiet = (0.003, secs(10.0));
        let short = seq(&[quiet, (0.0055, 31_999)]);
        let exact = seq(&[quiet, (0.0055, 32_000)]);
        let m = metrics(&short);
        assert!(
            (m.longest_quiet_run_secs - 31_999.0 / 16_000.0).abs() < 1e-6,
            "{m:?}"
        );
        expect_rejected(&short, SilenceGate::NoRelativeRun);
        let m = metrics(&exact);
        assert!((m.longest_quiet_run_secs - 2.0).abs() < 1e-6, "{m:?}");
        expect_speech(&exact, SpeechRule::B3);
    }

    #[test]
    fn rule_b3_does_not_shadow_b2() {
        // 8 s @ 0,003 + 2 s @ 0,010: der Lauf erfüllt B2 **und** B3 — die
        // Reihenfolge entscheidet für B2.
        let pcm = seq(&[(0.003, secs(8.0)), (0.010, secs(2.0))]);
        let m = metrics(&pcm);
        assert!((m.longest_quiet_run_secs - 2.0).abs() < 1e-6, "{m:?}");
        expect_speech(&pcm, SpeechRule::B2);
    }

    #[test]
    fn rule_b3_ignores_a_one_second_disturbance() {
        // Nachbau Stuhl/Kabel bzw. Klick (SPIKES: Lauf ≤ 1,0 s): laut, aber
        // zu kurz — und der Rest ist digitale Null.
        let pcm = seq(&[(0.0, secs(15.0)), (0.03, secs(1.0)), (0.0, secs(6.0))]);
        let m = metrics(&pcm);
        assert!(m.rms < RMS_SILENCE_THRESHOLD, "{m:?}");
        assert!((m.longest_quiet_run_secs - 1.0).abs() < 1e-6, "{m:?}");
        assert!((m.longest_abs_run_secs - 1.0).abs() < 1e-6, "{m:?}");
        expect_rejected(&pcm, SilenceGate::NoRelativeRun);
    }

    // ------------------------------------------------------------- Regel C

    #[test]
    fn rule_c_below_absolute_floor() {
        let below = level(ABS_FLOOR * 0.999, secs(5.0));
        expect_rejected(&below, SilenceGate::BelowAbsoluteFloor);

        // An der Grenze greift C nicht mehr — dann entscheidet D (und lehnt
        // mangels Kontrast ab).
        expect_rejected(&level(ABS_FLOOR, secs(5.0)), SilenceGate::NoRelativeRun);
        expect_rejected(
            &level(ABS_FLOOR * 1.001, secs(5.0)),
            SilenceGate::NoRelativeRun,
        );
    }

    #[test]
    fn rule_c_catches_digital_zero_and_one_lsb() {
        expect_rejected(&level(0.0, secs(5.0)), SilenceGate::BelowAbsoluteFloor);
        // 1 LSB bei 16 bit ≈ 0,0000305.
        expect_rejected(
            &level(1.0 / 32_768.0, secs(5.0)),
            SilenceGate::BelowAbsoluteFloor,
        );
    }

    // ------------------------------------------------------------- Regel D

    #[test]
    fn rule_d_zero_floor_does_not_admit_a_single_click() {
        // Astra B1: 10 s digitale Null plus 250 ms bei 0,001. floor = 0,
        // Schwelle D = MIN_ACTIVE_RMS — der Lauf bleibt bei 0,25 s.
        let pcm = seq(&[(0.0, secs(10.0)), (0.001, MIN_SAMPLES_16KHZ)]);
        let m = metrics(&pcm);
        assert_eq!(m.floor, Some(0.0), "{m:?}");
        assert_eq!(m.threshold_d, Some(MIN_ACTIVE_RMS), "{m:?}");
        assert!((m.longest_rel_run_secs - 0.25).abs() < 1e-6, "{m:?}");
        expect_rejected(&pcm, SilenceGate::NoRelativeRun);
    }

    #[test]
    fn rule_d_zeros_never_connect_two_clicks() {
        let pcm = seq(&[
            (0.0, secs(5.0)),
            (0.001, secs(1.0)),
            (0.0, secs(1.0)),
            (0.001, secs(1.0)),
        ]);
        let m = metrics(&pcm);
        assert!((m.longest_rel_run_secs - 1.0).abs() < 1e-6, "{m:?}");
        expect_rejected(&pcm, SilenceGate::NoRelativeRun);
    }

    #[test]
    fn rule_d_admits_quiet_speech_after_silence() {
        // Unter QUIET_SPEECH_RMS, damit D entscheidet und nicht B3 (v1.7).
        let pcm = seq(&[(0.0, secs(10.0)), (0.0035, secs(3.0))]);
        let m = metrics(&pcm);
        assert!(m.rms < RMS_SILENCE_THRESHOLD, "{m:?}");
        assert_eq!(m.longest_quiet_run_secs, 0.0, "{m:?}");
        assert!((m.longest_rel_run_secs - 3.0).abs() < 1e-6, "{m:?}");
        expect_speech(&pcm, SpeechRule::D);

        // Derselbe Fall über 0,004 wird seit v1.7 schon von B3 abgefangen.
        expect_speech(
            &seq(&[(0.0, secs(10.0)), (0.004, secs(3.0))]),
            SpeechRule::B3,
        );
    }

    #[test]
    fn rule_d_same_cases_with_one_lsb_residual() {
        let lsb = 1.0 / 32_768.0;
        // Restpegel statt digitaler Null: floor ist winzig, MIN_ACTIVE_RMS
        // hält die Schwelle; Fenster unter ABS_FLOOR bleiben inaktiv.
        let click = seq(&[(lsb, secs(10.0)), (0.001, MIN_SAMPLES_16KHZ)]);
        let m = metrics(&click);
        assert_eq!(m.threshold_d, Some(MIN_ACTIVE_RMS), "{m:?}");
        expect_rejected(&click, SilenceGate::NoRelativeRun);

        let two_clicks = seq(&[
            (lsb, secs(5.0)),
            (0.001, secs(1.0)),
            (lsb, secs(1.0)),
            (0.001, secs(1.0)),
        ]);
        let m = metrics(&two_clicks);
        assert!((m.longest_rel_run_secs - 1.0).abs() < 1e-6, "{m:?}");
        expect_rejected(&two_clicks, SilenceGate::NoRelativeRun);

        let speech = seq(&[(lsb, secs(10.0)), (0.0035, secs(3.0))]);
        expect_speech(&speech, SpeechRule::D);
    }

    #[test]
    fn rule_d_margin_is_exact_at_twelve_db() {
        let thr = (0.001_f32 * db_to_ratio(RELATIVE_MARGIN_DB)).max(MIN_ACTIVE_RMS);
        assert!((thr - 0.0039810717).abs() < 1e-7, "{thr}");

        let quiet = (0.001, secs(10.0));
        let below = seq(&[quiet, (thr * 0.999, secs(2.0))]);
        let above = seq(&[quiet, (thr * 1.001, secs(2.0))]);
        assert_eq!(metrics(&below).floor, Some(0.001));
        assert_eq!(metrics(&above).floor, Some(0.001));
        expect_rejected(&below, SilenceGate::NoRelativeRun);
        expect_speech(&above, SpeechRule::D);
    }

    #[test]
    fn rule_d_run_is_exact_at_24000_samples() {
        let quiet = (0.001, secs(10.0));
        let short = seq(&[quiet, (0.005, 23_999)]);
        let exact = seq(&[quiet, (0.005, 24_000)]);
        let m = metrics(&short);
        assert_eq!(m.windows, 40 + 6, "{m:?}");
        assert_eq!(
            m.full_windows,
            40 + 5,
            "Restfenster zählt nicht voll: {m:?}"
        );
        assert!(
            (m.longest_rel_run_secs - 23_999.0 / 16_000.0).abs() < 1e-6,
            "{m:?}"
        );
        expect_rejected(&short, SilenceGate::NoRelativeRun);
        expect_speech(&exact, SpeechRule::D);
    }

    #[test]
    fn rule_d_separate_runs_are_not_added() {
        let pcm = seq(&[
            (0.001, secs(10.0)),
            (0.005, secs(1.0)),
            (0.001, secs(0.25)),
            (0.005, secs(1.0)),
        ]);
        let m = metrics(&pcm);
        assert!((m.longest_rel_run_secs - 1.0).abs() < 1e-6, "{m:?}");
        expect_rejected(&pcm, SilenceGate::NoRelativeRun);
    }

    // -------------------------------------------------- Fensterung / floor

    #[test]
    fn remainder_window_counts_for_run_but_not_for_floor() {
        // Restfenster ist leiser als alle vollen — der floor darf es nicht sehen.
        let with_quiet_tail = seq(&[(0.005, 4 * MIN_SAMPLES_16KHZ), (0.0002, 1_000)]);
        let m = metrics(&with_quiet_tail);
        assert_eq!(m.windows, 5, "{m:?}");
        assert_eq!(m.full_windows, 4, "{m:?}");
        assert_eq!(m.floor, Some(0.005), "Restfenster im floor: {m:?}");

        // … und es zählt mit seinen echten 1.000 Samples in den Lauf.
        let with_loud_tail = seq(&[
            (0.001, 3 * MIN_SAMPLES_16KHZ),
            (0.005, MIN_SAMPLES_16KHZ),
            (0.005, 1_000),
        ]);
        let m = metrics(&with_loud_tail);
        assert_eq!(m.full_windows, 4, "{m:?}");
        assert_eq!(m.floor, Some(0.001), "{m:?}");
        assert!(
            (m.longest_rel_run_secs - 5_000.0 / 16_000.0).abs() < 1e-6,
            "{m:?}"
        );
    }

    #[test]
    fn noise_floor_uses_nearest_rank_index() {
        // Werte 0,1,2,… → floor ist der Wert am erwarteten Index.
        for (n, expected_index) in [(1, 0), (2, 0), (8, 0), (10, 0), (11, 1), (20, 1)] {
            let windows: Vec<Window> = (0..n)
                .map(|i| Window {
                    rms: i as f32,
                    samples: MIN_SAMPLES_16KHZ,
                })
                .collect();
            assert_eq!(
                noise_floor(&windows),
                Some(expected_index as f32),
                "n = {n}"
            );
        }
    }

    #[test]
    fn noise_floor_ignores_partial_windows_and_empty_input() {
        let windows = vec![
            Window {
                rms: 0.005,
                samples: MIN_SAMPLES_16KHZ,
            },
            Window {
                rms: 0.0001,
                samples: 10,
            },
        ];
        assert_eq!(noise_floor(&windows), Some(0.005));
        assert_eq!(noise_floor(&[]), None);
        assert_eq!(
            noise_floor(&[Window {
                rms: 0.001,
                samples: 10
            }]),
            None
        );
    }

    #[test]
    fn windows_are_non_overlapping_from_sample_zero() {
        let pcm = seq(&[
            (0.01, MIN_SAMPLES_16KHZ),
            (0.02, MIN_SAMPLES_16KHZ),
            (0.03, 7),
        ]);
        let windows = window_rms(&pcm);
        assert_eq!(windows.len(), 3);
        assert_eq!(windows[0].samples, MIN_SAMPLES_16KHZ);
        assert_eq!(windows[2].samples, 7);
        assert!((windows[0].rms - 0.01).abs() < 1e-6);
        assert!((windows[1].rms - 0.02).abs() < 1e-6);
        assert!((windows[2].rms - 0.03).abs() < 1e-6);
        assert!((max_window_rms(&pcm) - 0.03).abs() < 1e-6);
    }

    // ------------------------------------------------------- InvalidInput

    #[test]
    fn non_finite_samples_are_reported_not_cleaned() {
        let mut pcm = level(0.02, secs(3.0));
        pcm[7] = f32::NAN;
        let report = expect_rejected(&pcm, SilenceGate::InvalidInput { non_finite: 1 });
        assert!(report.metrics.is_none(), "{report}");

        let mut pcm = level(0.02, secs(3.0));
        pcm[7] = f32::INFINITY;
        pcm[9] = f32::NEG_INFINITY;
        expect_rejected(&pcm, SilenceGate::InvalidInput { non_finite: 2 });
    }

    // ------------------------------------------------------------- Display

    #[test]
    fn display_carries_every_measurement() {
        let cases = [
            level(0.1, MIN_SAMPLES_16KHZ - 1),               // A
            level(0.02, secs(3.0)),                          // B1
            seq(&[(0.003, secs(8.0)), (0.010, secs(2.0))]),  // B2
            level(0.0, secs(5.0)),                           // C
            level(0.001, secs(5.0)),                         // D leer
            seq(&[(0.0, secs(10.0)), (0.0035, secs(3.0))]),  // D Engine
            seq(&[(0.003, secs(3.0)), (0.0055, secs(4.0))]), // B3
        ];
        for pcm in cases {
            let report = silence_gate(&pcm);
            let text = report.to_string();
            let m = report.metrics.expect("endliche Eingabe");
            for needle in [
                format!("{} Samples", report.samples),
                format!("Fenster {}/{} voll", m.full_windows, m.windows),
                format!("RMS {:.5}", m.rms),
                format!("max. Fenster {:.5}", m.max_window_rms),
                format!("Lauf abs {:.2} s", m.longest_abs_run_secs),
                format!(
                    "Lauf {QUIET_SPEECH_RMS:.3} {:.2} s",
                    m.longest_quiet_run_secs
                ),
                format!("Lauf rel {:.2} s", m.longest_rel_run_secs),
                match m.floor {
                    Some(v) => format!("floor {v:.5}"),
                    None => "floor —".into(),
                },
                match m.threshold_d {
                    Some(v) => format!("Schwelle D {v:.5}"),
                    None => "Schwelle D —".into(),
                },
            ] {
                assert!(text.contains(&needle), "fehlt {needle:?} in {text:?}");
            }
            let verdict = if report.is_rejected() {
                "leer"
            } else {
                "Engine"
            };
            assert!(text.starts_with(verdict), "{text}");
        }
    }

    #[test]
    fn display_of_too_short_and_invalid_input() {
        let short = silence_gate(&level(0.1, 100)).to_string();
        assert!(short.starts_with("leer (Regel A: zu kurz"), "{short}");
        assert!(short.contains("floor —"), "{short}");
        assert!(short.contains("Schwelle D —"), "{short}");

        let invalid = silence_gate(&[f32::NAN; 8_000]).to_string();
        assert!(
            invalid.contains("nicht-endliche Samples (8000)"),
            "{invalid}"
        );
        assert!(invalid.contains("keine Messwerte"), "{invalid}");
    }

    #[test]
    fn rule_names_appear_in_the_report() {
        for (pcm, needle) in [
            (level(0.02, secs(3.0)), "Regel B1"),
            (seq(&[(0.003, secs(8.0)), (0.010, secs(2.0))]), "Regel B2"),
            (seq(&[(0.003, secs(3.0)), (0.0055, secs(4.0))]), "Regel B3"),
            (seq(&[(0.0, secs(10.0)), (0.0035, secs(3.0))]), "Regel D"),
            (level(0.0, secs(5.0)), "Regel C"),
        ] {
            let text = silence_gate(&pcm).to_string();
            assert!(text.contains(needle), "fehlt {needle:?} in {text:?}");
        }
    }

    // ------------------------------------------------------------ Fixtures

    fn testdata(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/stt")
            .join(name)
    }

    #[test]
    fn silence_and_noise_wavs_skip_engine() {
        for name in ["stille.wav", "rauschen.wav"] {
            let pcm = crate::audio::read_wav_16k_mono(&testdata(name)).unwrap();
            let report = silence_gate(&pcm);
            assert!(report.is_rejected(), "{name}: {report}");
            let (report, calls, text) = run_gate(&pcm);
            assert_eq!(calls, 0, "{name}: Engine darf nicht laufen ({report})");
            assert!(text.is_empty(), "{name}: erwartet leer, got {text:?}");
        }
    }

    #[test]
    fn speech_wavs_call_the_engine() {
        for name in [
            "alltag.wav",
            "fachwoerter.wav",
            "zahlen_umlaute.wav",
            "alltag_-16db.wav",
            "alltag_-22db.wav",
            "fachwoerter_-16db.wav",
            "zahlen_umlaute_-16db.wav",
        ] {
            let pcm = crate::audio::read_wav_16k_mono(&testdata(name)).unwrap();
            let (report, calls, _) = run_gate(&pcm);
            assert!(!report.is_rejected(), "{name}: {report}");
            assert_eq!(calls, 1, "{name}: Engine muss laufen ({report})");
        }
    }

    #[test]
    fn windowed_rms_keeps_short_speech_in_long_silence() {
        let pcm = seq(&[(0.001, secs(23.0)), (0.02, secs(2.0))]);
        assert!(rms_f32(&pcm) < RMS_SILENCE_THRESHOLD);
        assert!(max_window_rms(&pcm) > RMS_SILENCE_THRESHOLD);
        expect_speech(&pcm, SpeechRule::B2);
    }

    #[test]
    fn pure_silence_still_skips_engine() {
        let pcm = level(0.001, secs(5.0));
        assert!(is_silence_or_short(&pcm));
        expect_rejected(&pcm, SilenceGate::NoRelativeRun);
    }

    // ------------------------------------------------------- Normalisierung

    /// Portierung von `testdata/stt/normalize.py` (Spec §12): dieselben
    /// Regeln, damit der stt-smoke die WER ohne Python rechnet.
    fn normalize(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for ch in text.to_lowercase().chars() {
            match ch {
                '-' | '–' | '—' => out.push(' '),
                '.' | ',' | '!' | '?' | ';' | ':' | '"' | '\'' | '„' | '“' | '”' | '‚' | '‘'
                | '’' | '»' | '«' => {}
                other => out.push(other),
            }
        }
        out.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// Wort-Levenshtein / |Referenz|, beide Seiten normalisiert.
    fn wer(reference: &str, hypothesis: &str) -> f64 {
        let reference = normalize(reference);
        let hypothesis = normalize(hypothesis);
        let refs: Vec<&str> = reference.split(' ').filter(|s| !s.is_empty()).collect();
        let hyps: Vec<&str> = hypothesis.split(' ').filter(|s| !s.is_empty()).collect();
        if refs.is_empty() {
            return if hyps.is_empty() { 0.0 } else { 1.0 };
        }
        let mut prev: Vec<usize> = (0..=hyps.len()).collect();
        for (i, r) in refs.iter().enumerate() {
            let mut cur = vec![i + 1];
            for (j, h) in hyps.iter().enumerate() {
                let ins = cur[j] + 1;
                let del = prev[j + 1] + 1;
                let sub = prev[j] + usize::from(r != h);
                cur.push(ins.min(del).min(sub));
            }
            prev = cur;
        }
        prev[hyps.len()] as f64 / refs.len() as f64
    }

    #[test]
    fn normalize_and_wer_match_the_python_selftest() {
        assert_eq!(normalize("Hallo, Welt!"), "hallo welt");
        assert_eq!(normalize("rust-daemon"), "rust daemon");
        assert_eq!(normalize("Grüße, Öl, Spaß — Zeile"), "grüße öl spaß zeile");
        assert_eq!(normalize("a–b"), "a b");
        assert_eq!(normalize("„Zitat“ »x« ”y” ‚z‘ ‘w’"), "zitat x y z w");
        assert_eq!(normalize("a.,!?;:\"'b"), "ab");
        assert_eq!(wer("eins zwei drei", "eins zwei drei"), 0.0);
        assert!((wer("a b c", "a x c") - 1.0 / 3.0).abs() < 1e-9);
        assert!((wer("a b", "a x b") - 0.5).abs() < 1e-9);
        assert!((wer("a b c", "a c") - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!(wer("", ""), 0.0);
        assert_eq!(wer("", "x"), 1.0);
        assert_eq!(wer("x", ""), 1.0);
    }

    // ------------------------------------------------------------- Sonstige

    #[test]
    fn load_rejects_unknown_model_key() {
        let err = match ParakeetTranscriber::load("whisper-medium", 0) {
            Err(err) => err,
            Ok(_) => panic!("expected Artifacts error"),
        };
        match err {
            EngineError::Artifacts(msg) => {
                assert!(msg.contains("whisper-medium"), "{msg}");
                assert!(msg.contains("Manifest"), "{msg}");
            }
            other => panic!("expected Artifacts, got {other:?}"),
        }
    }

    #[test]
    fn resolve_ort_lib_errors_without_library() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve_ort_lib_from_exe_dir(dir.path()).unwrap_err();
        match err {
            EngineError::Ort(msg) => {
                assert!(msg.contains(ort_lib_filename()), "{msg}");
                assert!(msg.contains("nicht gefunden"), "{msg}");
            }
            other => panic!("expected Ort, got {other:?}"),
        }
    }

    /// WER-Puffer aus Spec §12/§18 #11: die abgesenkte Datei darf höchstens
    /// 0,05 schlechter sein als dieselbe Aufnahme unabgesenkt.
    const WER_BUFFER: f64 = 0.05;

    #[test]
    #[ignore = "stt-smoke: Golden Set + ORT-Library + testdata/stt/*.wav"]
    fn stt_smoke_fixtures() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let testdata = root.join("testdata/stt");
        let rejected = ["stille.wav", "rauschen.wav"];
        // (Datei, Referenz-Baseline) — `None` ist selbst die Baseline.
        let speech: [(&str, Option<&str>); 7] = [
            ("alltag.wav", None),
            ("fachwoerter.wav", None),
            ("zahlen_umlaute.wav", None),
            ("alltag_-16db.wav", Some("alltag.wav")),
            ("alltag_-22db.wav", Some("alltag.wav")),
            ("fachwoerter_-16db.wav", Some("fachwoerter.wav")),
            ("zahlen_umlaute_-16db.wav", Some("zahlen_umlaute.wav")),
        ];
        for name in rejected.iter().chain(speech.iter().map(|(n, _)| n)) {
            let wav = testdata.join(name);
            if !wav.is_file() {
                panic!(
                    "testdata fehlt: {} — Herkunft siehe testdata/stt/README.md",
                    wav.display()
                );
            }
        }
        if let Err(err) = resolve_ort_lib() {
            panic!("ORT-Library fehlt: {err}\nHinweis: scripts/fetch-ort.ps1");
        }
        let (dir, manifest) = model_artifacts(DEFAULT_MODEL).unwrap_or_else(|e| {
            panic!("Modellpfad/Manifest: {e}");
        });
        if let Err(err) = download::check_artifacts(&dir, &manifest) {
            panic!(
                "Modellartefakte fehlen oder Größe stimmt nicht ({err}). Erwartet in {}",
                dir.display()
            );
        }
        if let Err(err) = download::verify_artifacts_sha256(&dir, &manifest) {
            panic!("SHA-256-Prüfung fehlgeschlagen: {err}");
        }

        let mut engine = CountingEngine {
            inner: ParakeetTranscriber::load(DEFAULT_MODEL, 0).unwrap_or_else(|e| panic!("{e}")),
            calls: 0,
        };

        // Stille/Rauschen: abgelehnt **und** ohne Engine-Aufruf (Astra W3).
        for name in rejected {
            let pcm = crate::audio::read_wav_16k_mono(&testdata.join(name))
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            let before = engine.calls;
            let (report, result) = transcribe_pcm(&mut engine, &pcm);
            let out = result.unwrap_or_else(|e| panic!("{name}: {e}"));
            eprintln!("{name}: {report}");
            assert!(report.is_rejected(), "{name}: {report}");
            assert_eq!(engine.calls, before, "{name}: Engine wurde gerufen");
            assert!(out.text.trim().is_empty(), "{name}: {:?}", out.text);
        }

        // Sprache: Engine läuft, WER gegen den Referenztext.
        let mut baselines: Vec<(&str, f64)> = Vec::new();
        for (name, baseline_of) in speech {
            let pcm = crate::audio::read_wav_16k_mono(&testdata.join(name))
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            let before = engine.calls;
            let (report, result) = transcribe_pcm(&mut engine, &pcm);
            let out = result.unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(!report.is_rejected(), "{name}: {report}");
            assert_eq!(engine.calls, before + 1, "{name}: Engine lief nicht");
            assert!(!out.text.trim().is_empty(), "{name}: erwartet nicht-leer");

            let stem = baseline_of.unwrap_or(name).trim_end_matches(".wav");
            let reference = std::fs::read_to_string(testdata.join(format!("{stem}.ref.txt")))
                .unwrap_or_else(|e| panic!("{stem}.ref.txt: {e}"));
            let measured = wer(&reference, &out.text);
            eprintln!("{name}: WER {measured:.4} — {report}");
            match baseline_of {
                None => baselines.push((name, measured)),
                Some(base) => {
                    let baseline = baselines
                        .iter()
                        .find(|(n, _)| *n == base)
                        .map(|(_, w)| *w)
                        .unwrap_or_else(|| panic!("{name}: Baseline {base} fehlt"));
                    assert!(
                        measured <= baseline + WER_BUFFER,
                        "{name}: WER {measured:.4} > Baseline {baseline:.4} + {WER_BUFFER}"
                    );
                }
            }
        }

        let stille = crate::audio::read_wav_16k_mono(&testdata.join("stille.wav"))
            .unwrap_or_else(|e| panic!("{e}"));
        let n = MIN_SAMPLES_16KHZ.saturating_sub(1).min(stille.len());
        let snippet = &stille[..n];
        let t0 = Instant::now();
        let (report, result) = transcribe_pcm(&mut engine, snippet);
        let out = result.unwrap_or_else(|e| panic!("{e}"));
        let elapsed = t0.elapsed();
        assert!(out.text.trim().is_empty(), "Schnipsel sollte leer sein");
        assert!(
            matches!(
                report.decision,
                GateDecision::Rejected(SilenceGate::TooShort)
            ),
            "{report}"
        );
        assert!(
            elapsed < Duration::from_millis(50),
            "Schnipsel < 250 ms darf die Engine nicht rufen, dauerte {elapsed:?}"
        );
    }
}
