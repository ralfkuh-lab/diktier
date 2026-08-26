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

/// Überspringt die Engine bei Audio kürzer als 250 ms.
pub fn transcribe_pcm<T: Transcriber>(
    engine: &mut T,
    pcm_f32_16khz: &[f32],
) -> Result<Transcription, EngineError> {
    if pcm_f32_16khz.len() < MIN_SAMPLES_16KHZ {
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
