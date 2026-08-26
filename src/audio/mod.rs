//! AudioSource-Vertrag (Spec §5.1 / §6.4). Capture: natives Gerät → 16 kHz mono f32.

#![allow(dead_code)]

mod capture;
mod convert;
mod resample;
mod spsc;

use std::path::Path;

use thiserror::Error;

pub use capture::CpalAudioSource;

pub const ENGINE_RATE: u32 = 16_000;

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
    /// Gerät öffnen, ohne aufzunehmen — damit `start()` nicht auf den
    /// Geräteaufbau wartet (Spec §5: „Aufnahme aus `idle` startet sofort").
    /// Idempotent; ein verlorenes Gerät wird dabei einmal neu geöffnet (§6.4).
    fn prepare(&mut self) -> Result<(), AudioError> {
        Ok(())
    }
    /// Gegenstück zu [`AudioSource::prepare`]: Gerät wieder hergeben.
    ///
    /// §4.3 `paused` heißt „ich will jetzt nicht diktieren" — dann soll auch
    /// kein Mikrofon offen stehen. Eine laufende Aufnahme wird nie abgebrochen.
    fn release(&mut self) {}
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
            sample_rate: ENGINE_RATE,
        })
    }
}

/// Nur 16 kHz mono PCM (i16 oder f32). Resample/Stereo ist der Capture-Pfad.
pub fn read_wav_16k_mono(path: &Path) -> Result<Vec<f32>, AudioError> {
    let mut reader = hound::WavReader::open(path).map_err(|e| map_hound_open(path, e))?;
    let spec = reader.spec();
    if spec.sample_rate != ENGINE_RATE {
        return Err(AudioError::Format(format!(
            "WAV ist {} Hz, erwartet {ENGINE_RATE} Hz (Capture/Resample ist Phase 2)",
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
                samples.push(convert::i16_to_f32(v));
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
    use super::capture::process_interleaved_f32;
    use super::*;
    use std::f32::consts::PI;
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

    fn sine(rate: u32, secs: f32, freq: f32, amp: f32) -> Vec<f32> {
        let n = (rate as f32 * secs).round() as usize;
        (0..n)
            .map(|i| amp * (2.0 * PI * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        if x.is_empty() {
            return 0.0;
        }
        (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
    }

    fn assert_len_within_pct(got: usize, expected: usize, pct: f32) {
        let exp = expected as f32;
        let lo = (exp * (1.0 - pct)).floor() as usize;
        let hi = (exp * (1.0 + pct)).ceil() as usize;
        assert!(
            got >= lo && got <= hi,
            "len {got} not in {lo}..={hi} (expected {expected} ±{pct})"
        );
    }

    #[test]
    fn stub_capture_returns_empty_16khz() {
        let mut src = StubAudioSource;
        src.start().unwrap();
        let captured = src.stop().unwrap();
        assert!(captured.samples.is_empty());
        assert_eq!(captured.sample_rate, ENGINE_RATE);
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

    #[test]
    fn resample_440hz_44100_mono() {
        let input = sine(44_100, 1.0, 440.0, 0.5);
        let out = process_interleaved_f32(&input, 1, 44_100, 60).unwrap();
        assert_eq!(out.sample_rate, ENGINE_RATE);
        let expected = resample::expected_len(input.len(), 44_100);
        assert_len_within_pct(out.samples.len(), expected, 0.01);
        let ratio = rms(&out.samples) / rms(&input);
        assert!(
            (0.7..=1.3).contains(&ratio),
            "RMS-Verhältnis {ratio} (in {} out {})",
            rms(&input),
            rms(&out.samples)
        );
    }

    #[test]
    fn resample_440hz_48000_mono() {
        let input = sine(48_000, 1.0, 440.0, 0.5);
        let out = process_interleaved_f32(&input, 1, 48_000, 60).unwrap();
        assert_eq!(out.sample_rate, ENGINE_RATE);
        let expected = resample::expected_len(input.len(), 48_000);
        assert_len_within_pct(out.samples.len(), expected, 0.01);
        let ratio = rms(&out.samples) / rms(&input);
        assert!((0.7..=1.3).contains(&ratio), "RMS-Verhältnis {ratio}");
    }

    #[test]
    fn resample_440hz_48000_stereo_downmix() {
        let mono = sine(48_000, 1.0, 440.0, 0.5);
        let mut stereo_eq = Vec::with_capacity(mono.len() * 2);
        for s in &mono {
            stereo_eq.push(*s);
            stereo_eq.push(*s);
        }
        let out_eq = process_interleaved_f32(&stereo_eq, 2, 48_000, 60).unwrap();
        let out_mono = process_interleaved_f32(&mono, 1, 48_000, 60).unwrap();
        assert_eq!(out_eq.sample_rate, ENGINE_RATE);
        let expected = resample::expected_len(mono.len(), 48_000);
        assert_len_within_pct(out_eq.samples.len(), expected, 0.01);
        let n = out_eq.samples.len().min(out_mono.samples.len());
        let mut max_diff = 0.0_f32;
        for i in 0..n {
            max_diff = max_diff.max((out_eq.samples[i] - out_mono.samples[i]).abs());
        }
        assert!(max_diff < 1e-5, "L=R vs mono max_diff {max_diff}");

        let mut stereo_inv = Vec::with_capacity(mono.len() * 2);
        for s in &mono {
            stereo_inv.push(*s);
            stereo_inv.push(-*s);
        }
        let out_inv = process_interleaved_f32(&stereo_inv, 2, 48_000, 60).unwrap();
        assert!(
            rms(&out_inv.samples) < 1e-4,
            "L=-R RMS {}",
            rms(&out_inv.samples)
        );
    }

    #[test]
    fn resample_flush_keeps_tail() {
        let rate = 48_000;
        let n = 1024 + 100;
        let input: Vec<f32> = (0..n)
            .map(|i| (2.0 * PI * 440.0 * i as f32 / rate as f32).sin() * 0.5)
            .collect();
        let out = process_interleaved_f32(&input, 1, rate, 60).unwrap();
        let expected = resample::expected_len(n, rate);
        assert_len_within_pct(out.samples.len(), expected, 0.01);
        assert!(
            out.samples.len() as f32 > expected as f32 * 0.95,
            "Flush verlor den Rest: got {} expected ~{expected}",
            out.samples.len()
        );
    }

    #[test]
    fn max_duration_truncates_first_cap_seconds() {
        let input = sine(16_000, 3.0, 440.0, 0.2);
        let out = process_interleaved_f32(&input, 1, 16_000, 1).unwrap();
        assert_eq!(out.samples.len(), ENGINE_RATE as usize);
    }
}
