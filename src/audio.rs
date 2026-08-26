//! AudioSource-Vertrag (Spec §5.1 / §6.4). Phase 0: Stub, kein cpal.
#![allow(dead_code)]

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("Audio fehlgeschlagen: {0}")]
    Failed(String),
}

/// 16 kHz, mono, f32 — Engine-Zielrate in v1.
#[derive(Debug, Clone, PartialEq)]
pub struct CapturedAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

pub trait AudioSource {
    fn start(&mut self) -> Result<(), AudioError>;
    fn stop(&mut self) -> Result<CapturedAudio, AudioError>;
}

#[derive(Debug, Default)]
pub struct StubAudioSource;

impl AudioSource for StubAudioSource {
    fn start(&mut self) -> Result<(), AudioError> {
        Ok(())
    }

    fn stop(&mut self) -> Result<CapturedAudio, AudioError> {
        Ok(CapturedAudio {
            samples: Vec::new(),
            sample_rate: 16_000,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_capture_returns_empty_16khz() {
        let mut src = StubAudioSource;
        src.start().unwrap();
        let captured = src.stop().unwrap();
        assert!(captured.samples.is_empty());
        assert_eq!(captured.sample_rate, 16_000);
    }
}
