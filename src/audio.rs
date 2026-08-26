//! AudioSource-Vertrag (Spec §5.1 / §6.4). Phase 1: WAV-Lesen für den Spike.
#![allow(dead_code)]

use std::path::Path;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("Audio fehlgeschlagen: {0}")]
    Failed(String),
    /// Echte I/O-Fehler (nicht gefunden, Permission, Lesefehler) → Exit 1.
    #[error("{0}")]
    Io(String),
    /// Format/Rate/Kanäle/Bittiefe/kaputte Datei → Exit 2.
    #[error("{0}")]
    Format(String),
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

/// Nur 16 kHz mono PCM (i16 oder f32). Resample/Stereo ist Phase 2.
pub fn read_wav_16k_mono(path: &Path) -> Result<Vec<f32>, AudioError> {
    let mut reader = hound::WavReader::open(path).map_err(|e| map_hound_open(path, e))?;
    let spec = reader.spec();
    if spec.sample_rate != 16_000 {
        return Err(AudioError::Format(format!(
            "WAV ist {} Hz, erwartet 16000 Hz (Capture/Resample ist Phase 2)",
            spec.sample_rate
        )));
    }
    if spec.channels != 1 {
        return Err(AudioError::Format(format!(
            "WAV hat {} Kanäle, erwartet mono (Downmix ist Phase 2)",
            spec.channels
        )));
    }
    match spec.sample_format {
        hound::SampleFormat::Int => {
            if spec.bits_per_sample != 16 {
                return Err(AudioError::Format(format!(
                    "WAV ist {}-bit Integer-PCM, erwartet 16-bit",
                    spec.bits_per_sample
                )));
            }
            let mut samples = Vec::with_capacity(reader.duration() as usize);
            for sample in reader.samples::<i16>() {
                let v = sample.map_err(|e| map_hound_sample(path, e))?;
                samples.push(f32::from(v) / 32768.0);
            }
            Ok(samples)
        }
        hound::SampleFormat::Float => {
            if spec.bits_per_sample != 32 {
                return Err(AudioError::Format(format!(
                    "WAV ist {}-bit Float-PCM, erwartet 32-bit",
                    spec.bits_per_sample
                )));
            }
            let mut samples = Vec::with_capacity(reader.duration() as usize);
            for sample in reader.samples::<f32>() {
                let v = sample.map_err(|e| map_hound_sample(path, e))?;
                if !v.is_finite() {
                    return Err(AudioError::Format(format!(
                        "WAV enthält nicht-finite Samples (NaN/Inf): {}",
                        path.display()
                    )));
                }
                samples.push(v);
            }
            Ok(samples)
        }
    }
}

fn map_hound_open(path: &Path, err: hound::Error) -> AudioError {
    match err {
        hound::Error::IoError(io) => AudioError::Io(format!("WAV I/O ({}): {io}", path.display())),
        other => AudioError::Format(format!("WAV-Format ({}): {other}", path.display())),
    }
}

fn map_hound_sample(path: &Path, err: hound::Error) -> AudioError {
    match err {
        hound::Error::IoError(io) => {
            AudioError::Io(format!("WAV-Lesefehler ({}): {io}", path.display()))
        }
        other => AudioError::Format(format!("WAV beschädigt ({}): {other}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn write_i16(path: &std::path::Path, rate: u32, channels: u16, bits: u16, samples: &[i16]) {
        let spec = hound::WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: bits,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for s in samples {
            writer.write_sample(*s).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn stub_capture_returns_empty_16khz() {
        let mut src = StubAudioSource;
        src.start().unwrap();
        let captured = src.stop().unwrap();
        assert!(captured.samples.is_empty());
        assert_eq!(captured.sample_rate, 16_000);
    }

    #[test]
    fn wav_rejects_wrong_rate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.wav");
        write_i16(&path, 44_100, 1, 16, &[0]);
        let err = read_wav_16k_mono(&path).unwrap_err();
        match err {
            AudioError::Format(msg) => assert!(msg.contains("44100"), "{msg}"),
            other => panic!("expected Format, got {other:?}"),
        }
    }

    #[test]
    fn wav_rejects_stereo() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stereo.wav");
        write_i16(&path, 16_000, 2, 16, &[0, 0]);
        let err = read_wav_16k_mono(&path).unwrap_err();
        match err {
            AudioError::Format(msg) => assert!(msg.contains("2 Kanäle"), "{msg}"),
            other => panic!("expected Format, got {other:?}"),
        }
    }

    #[test]
    fn wav_rejects_8bit_and_24bit() {
        let dir = tempfile::tempdir().unwrap();
        let p8 = dir.path().join("a8.wav");
        write_i16(&p8, 16_000, 1, 8, &[0]);
        match read_wav_16k_mono(&p8).unwrap_err() {
            AudioError::Format(msg) => assert!(msg.contains("8-bit"), "{msg}"),
            other => panic!("expected Format, got {other:?}"),
        }

        let p24 = dir.path().join("a24.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 24,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&p24, spec).unwrap();
        writer.write_sample(0_i32).unwrap();
        writer.finalize().unwrap();
        match read_wav_16k_mono(&p24).unwrap_err() {
            AudioError::Format(msg) => assert!(msg.contains("24-bit"), "{msg}"),
            other => panic!("expected Format, got {other:?}"),
        }
    }

    #[test]
    fn wav_i16_scales_to_unit_range() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("i16.wav");
        write_i16(&path, 16_000, 1, 16, &[0, 32767, -32768]);
        let pcm = read_wav_16k_mono(&path).unwrap();
        assert_eq!(pcm.len(), 3);
        assert!((pcm[0]).abs() < f32::EPSILON);
        assert!((pcm[1] - 32767.0 / 32768.0).abs() < 1e-6);
        assert!((-1.0 - pcm[2]).abs() < 1e-6);
        assert!(pcm.iter().all(|s| (-1.0..=1.0).contains(s)));
    }

    #[test]
    fn wav_f32_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f32.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        writer.write_sample(0.5_f32).unwrap();
        writer.write_sample(-0.25_f32).unwrap();
        writer.finalize().unwrap();
        let pcm = read_wav_16k_mono(&path).unwrap();
        assert_eq!(pcm, vec![0.5, -0.25]);
    }

    #[test]
    fn wav_f32_rejects_nan_and_inf() {
        let dir = tempfile::tempdir().unwrap();
        for (name, value) in [("nan.wav", f32::NAN), ("inf.wav", f32::INFINITY)] {
            let path = dir.path().join(name);
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            };
            let mut writer = hound::WavWriter::create(&path, spec).unwrap();
            writer.write_sample(value).unwrap();
            writer.finalize().unwrap();
            match read_wav_16k_mono(&path).unwrap_err() {
                AudioError::Format(msg) => assert!(msg.contains("finite"), "{msg}"),
                other => panic!("expected Format, got {other:?}"),
            }
        }
    }

    #[test]
    fn wav_missing_file_is_io() {
        let path = PathBuf::from("/no/such/diktier-wav-missing.wav");
        match read_wav_16k_mono(&path).unwrap_err() {
            AudioError::Io(msg) => assert!(msg.contains("WAV I/O"), "{msg}"),
            other => panic!("expected Io, got {other:?}"),
        }
    }
}
