//! Transcriber-Vertrag (Spec §5.1) und Parakeet-TDT-Engine (Phase 1).
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use parakeet_rs::{ExecutionConfig, ParakeetTDT};
use thiserror::Error;

use crate::download::{self, ArtifactManifest, DownloadError};

/// 16 kHz × 250 ms. Kürzer → kein Engine-Aufruf (Spec §6.4).
pub const MIN_SAMPLES_16KHZ: usize = 16_000 * 250 / 1_000;

/// RMS-Schwelle für den Silence-Gate (Spec §12: Halluzination auf
/// Stille/Rauschen darf Diktier vorschalten — Engine wird nicht aufgerufen).
///
/// `0.0075` ≈ 20·log₁₀(0.0075) ≈ −42,5 dBFS.
/// `testdata/stt/rauschen.wav` RMS ≈ 0,0051 muss darunter bleiben;
/// leiseste Sprachaufnahme `alltag.wav` ≈ 0,0215 klar darüber.
pub const RMS_SILENCE_THRESHOLD: f32 = 0.0075;

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

/// Maximum der RMS-Werte über nicht überlappende 250-ms-Fenster.
/// Lange Stille plus kurze leise Sprache bleibt so über der Schwelle (agy B3 / codex N1).
pub fn max_window_rms(pcm_f32_16khz: &[f32]) -> f32 {
    if pcm_f32_16khz.is_empty() {
        return 0.0;
    }
    let win = MIN_SAMPLES_16KHZ;
    if pcm_f32_16khz.len() <= win {
        return rms_f32(pcm_f32_16khz);
    }
    let mut max = 0.0_f32;
    let mut i = 0;
    while i < pcm_f32_16khz.len() {
        let end = (i + win).min(pcm_f32_16khz.len());
        let r = rms_f32(&pcm_f32_16khz[i..end]);
        if r > max {
            max = r;
        }
        if end == pcm_f32_16khz.len() {
            break;
        }
        i += win;
    }
    max
}

/// Zu kurz oder unter der RMS-Schwelle — Engine nicht laden/aufrufen.
///
/// Primär: `max(RMS über 250-ms-Fenster) < 0.0075` → leer (agy B3 / codex N1).
/// Wenn der Gesamtpuffer unter der Schwelle bleibt, einzelne laute Fenster
/// aber drüber sind (Klick in `rauschen.wav` vs. 2 s leise Sprache in langer
/// Stille): nur Durchläufe von mindestens 2 s über der Schwelle gelten als
/// Sprache — sonst WAV-Regression (`rauschen.wav`) würde kippen.
pub fn is_silence_or_short(pcm_f32_16khz: &[f32]) -> bool {
    if pcm_f32_16khz.len() < MIN_SAMPLES_16KHZ {
        return true;
    }
    if max_window_rms(pcm_f32_16khz) < RMS_SILENCE_THRESHOLD {
        return true;
    }
    if rms_f32(pcm_f32_16khz) >= RMS_SILENCE_THRESHOLD {
        return false;
    }
    longest_loud_run_secs(pcm_f32_16khz) < 2.0
}

fn longest_loud_run_secs(pcm: &[f32]) -> f32 {
    let win = MIN_SAMPLES_16KHZ;
    let mut run = 0_usize;
    let mut best = 0_usize;
    let mut i = 0;
    while i < pcm.len() {
        let end = (i + win).min(pcm.len());
        if rms_f32(&pcm[i..end]) >= RMS_SILENCE_THRESHOLD {
            run += end - i;
            if run > best {
                best = run;
            }
        } else {
            run = 0;
        }
        if end == pcm.len() {
            break;
        }
        i += win;
    }
    best as f32 / 16_000.0
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

/// Überspringt die Engine bei Audio kürzer als 250 ms oder unter der
/// RMS-Silence-Schwelle (Spec §6.4 / §12).
pub fn transcribe_pcm<T: Transcriber>(
    engine: &mut T,
    pcm_f32_16khz: &[f32],
) -> Result<Transcription, EngineError> {
    if is_silence_or_short(pcm_f32_16khz) {
        return Ok(Transcription::empty());
    }
    engine.transcribe(pcm_f32_16khz)
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
        let sample_rate = 16_000u32;
        let result = parakeet_rs::Transcriber::transcribe_samples(
            &mut self.inner,
            pcm_f32_16khz.to_vec(),
            sample_rate,
            1,
            None,
        )
        .map_err(|e| EngineError::Failed(format!("parakeet-rs: {e}")))?;
        let duration = Duration::from_secs_f64(pcm_f32_16khz.len() as f64 / f64::from(sample_rate));
        Ok(Transcription {
            text: result.text,
            language: None,
            timing: Some(Timing { duration }),
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
/// 2. `../lib/<name>` (Unix-Prefix `bin/`+`lib/`; Cargo-Test: `deps/` → `../lib`)
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
        "ONNX-Runtime-Library {name} nicht gefunden (gesucht: {} und {}; scripts/fetch-ort.sh)",
        candidates[0].display(),
        candidates[1].display()
    )))
}

pub(crate) fn ort_lib_filename() -> &'static str {
    #[cfg(windows)]
    {
        "onnxruntime.dll"
    }
    #[cfg(target_os = "linux")]
    {
        "libonnxruntime.so"
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        compile_error!("diktier unterstützt nur Linux und Windows");
    }
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

    #[test]
    fn stub_silence_yields_empty_transcript() {
        let mut engine = StubTranscriber;
        let out = engine.transcribe(&[]).unwrap();
        assert!(out.text.is_empty());
        assert!(out.language.is_none());
        assert!(out.timing.is_none());
    }

    #[test]
    fn audio_shorter_than_250ms_skips_engine() {
        let mut stub = CountingStub { calls: 0 };
        let short = vec![0.1_f32; MIN_SAMPLES_16KHZ - 1];
        let out = transcribe_pcm(&mut stub, &short).unwrap();
        assert!(out.text.is_empty());
        assert_eq!(stub.calls, 0);

        let exact = vec![0.1_f32; MIN_SAMPLES_16KHZ];
        let out = transcribe_pcm(&mut stub, &exact).unwrap();
        assert_eq!(stub.calls, 1);
        assert_eq!(out.text, MIN_SAMPLES_16KHZ.to_string());
    }

    fn testdata(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/stt")
            .join(name)
    }

    #[test]
    fn silence_and_noise_wavs_skip_engine() {
        for name in ["stille.wav", "rauschen.wav"] {
            let pcm = crate::audio::read_wav_16k_mono(&testdata(name)).unwrap();
            let rms = rms_f32(&pcm);
            assert!(
                rms < RMS_SILENCE_THRESHOLD,
                "{name}: rms {rms} sollte unter {RMS_SILENCE_THRESHOLD} liegen"
            );
            let mut stub = CountingStub { calls: 0 };
            let out = transcribe_pcm(&mut stub, &pcm).unwrap();
            assert_eq!(stub.calls, 0, "{name}: Engine darf nicht laufen");
            assert!(
                out.text.is_empty(),
                "{name}: erwartet leer, got {:?}",
                out.text
            );
        }
    }

    #[test]
    fn alltag_wav_above_threshold_calls_engine() {
        let pcm = crate::audio::read_wav_16k_mono(&testdata("alltag.wav")).unwrap();
        let rms = rms_f32(&pcm);
        assert!(
            rms > RMS_SILENCE_THRESHOLD,
            "alltag.wav: rms {rms} sollte über {RMS_SILENCE_THRESHOLD} liegen"
        );
        let mut stub = CountingStub { calls: 0 };
        let out = transcribe_pcm(&mut stub, &pcm).unwrap();
        assert_eq!(stub.calls, 1);
        assert_eq!(out.text, pcm.len().to_string());
    }

    #[test]
    fn rms_silence_threshold_is_exclusive_below() {
        let n = MIN_SAMPLES_16KHZ;
        let just_below = vec![RMS_SILENCE_THRESHOLD * 0.999; n];
        let at = vec![RMS_SILENCE_THRESHOLD; n];
        let just_above = vec![RMS_SILENCE_THRESHOLD * 1.001; n];
        assert!(rms_f32(&just_below) < RMS_SILENCE_THRESHOLD);
        assert!((rms_f32(&at) - RMS_SILENCE_THRESHOLD).abs() < 1e-9);
        assert!(rms_f32(&just_above) > RMS_SILENCE_THRESHOLD);

        let mut stub = CountingStub { calls: 0 };
        let out = transcribe_pcm(&mut stub, &just_below).unwrap();
        assert_eq!(stub.calls, 0);
        assert!(out.text.is_empty());

        let out = transcribe_pcm(&mut stub, &at).unwrap();
        assert_eq!(stub.calls, 1);
        assert_eq!(out.text, n.to_string());

        let out = transcribe_pcm(&mut stub, &just_above).unwrap();
        assert_eq!(stub.calls, 2);
        assert_eq!(out.text, n.to_string());
    }

    #[test]
    fn windowed_rms_keeps_short_speech_in_long_silence() {
        let silence_n = 16_000 * 23;
        let speech_n = 16_000 * 2;
        let mut pcm = vec![0.001_f32; silence_n];
        pcm.extend(std::iter::repeat_n(0.02_f32, speech_n));
        assert!(rms_f32(&pcm) < RMS_SILENCE_THRESHOLD);
        assert!(max_window_rms(&pcm) > RMS_SILENCE_THRESHOLD);
        let mut stub = CountingStub { calls: 0 };
        transcribe_pcm(&mut stub, &pcm).unwrap();
        assert_eq!(stub.calls, 1);
    }

    #[test]
    fn pure_silence_still_skips_engine() {
        let pcm = vec![0.001_f32; 16_000 * 5];
        assert!(is_silence_or_short(&pcm));
        let mut stub = CountingStub { calls: 0 };
        let out = transcribe_pcm(&mut stub, &pcm).unwrap();
        assert_eq!(stub.calls, 0);
        assert!(out.text.is_empty());
    }

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

    #[test]
    #[ignore = "stt-smoke: Golden Set + ORT-Library + testdata/stt/*.wav"]
    fn stt_smoke_fixtures() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let testdata = root.join("testdata/stt");
        let fixtures = [
            ("stille.wav", true),
            ("rauschen.wav", true),
            ("alltag.wav", false),
            ("fachwoerter.wav", false),
            ("zahlen_umlaute.wav", false),
        ];
        for (name, _) in fixtures {
            let wav = testdata.join(name);
            if !wav.is_file() {
                panic!(
                    "testdata fehlt: {} — Aufnahme siehe testdata/stt/README.md",
                    wav.display()
                );
            }
        }
        if let Err(err) = resolve_ort_lib() {
            panic!("ORT-Library fehlt: {err}\nHinweis: scripts/fetch-ort.sh");
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

        let mut engine =
            ParakeetTranscriber::load(DEFAULT_MODEL, 0).unwrap_or_else(|e| panic!("{e}"));

        for (name, expect_empty) in fixtures {
            let wav = testdata.join(name);
            let pcm =
                crate::audio::read_wav_16k_mono(&wav).unwrap_or_else(|e| panic!("{name}: {e}"));
            let out = transcribe_pcm(&mut engine, &pcm).unwrap_or_else(|e| panic!("{name}: {e}"));
            if expect_empty {
                assert!(
                    out.text.trim().is_empty(),
                    "{name}: erwartet leer, got {:?}",
                    out.text
                );
            } else {
                assert!(!out.text.trim().is_empty(), "{name}: erwartet nicht-leer");
            }
        }

        let stille = crate::audio::read_wav_16k_mono(&testdata.join("stille.wav"))
            .unwrap_or_else(|e| panic!("{e}"));
        let n = MIN_SAMPLES_16KHZ.saturating_sub(1).min(stille.len());
        let snippet = &stille[..n];
        let t0 = Instant::now();
        let out = transcribe_pcm(&mut engine, snippet).unwrap_or_else(|e| panic!("{e}"));
        let elapsed = t0.elapsed();
        assert!(out.text.trim().is_empty(), "Schnipsel sollte leer sein");
        assert!(
            elapsed < Duration::from_millis(50),
            "Schnipsel < 250 ms darf die Engine nicht rufen, dauerte {elapsed:?}"
        );
    }
}
