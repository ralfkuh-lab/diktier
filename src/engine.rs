//! Transcriber-Vertrag (Spec §5.1). Phase 0: Stub, kein ORT/parakeet-rs.
#![allow(dead_code)]

use std::time::Duration;

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcription {
    pub text: String,
    pub language: Option<String>,
    pub timing: Option<Timing>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timing {
    pub duration: Duration,
}

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("Transkription fehlgeschlagen: {0}")]
    Failed(String),
}

pub trait Transcriber {
    fn transcribe(&mut self, pcm_f32_16khz: &[f32]) -> Result<Transcription, EngineError>;
}

#[derive(Debug, Default)]
pub struct StubTranscriber;

impl Transcriber for StubTranscriber {
    fn transcribe(&mut self, _pcm_f32_16khz: &[f32]) -> Result<Transcription, EngineError> {
        Ok(Transcription {
            text: String::new(),
            language: None,
            timing: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_silence_yields_empty_transcript() {
        let mut engine = StubTranscriber;
        let out = engine.transcribe(&[]).unwrap();
        assert!(out.text.is_empty());
        assert!(out.language.is_none());
        assert!(out.timing.is_none());
    }
}
