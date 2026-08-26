//! cpal-AudioSource: natives Gerät, lock-freier Callback, Worker-Pipeline.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, SampleFormat, Stream, StreamConfig, SupportedStreamConfig};

use crate::config::AudioConfig;

use super::convert::{
    downmix_interleaved, f64_interleaved_to_f32, i8_interleaved_to_f32, i16_interleaved_to_f32,
    i32_interleaved_to_f32, i64_interleaved_to_f32, u8_interleaved_to_f32, u16_interleaved_to_f32,
    u32_interleaved_to_f32,
};
use super::resample::resample_mono_to_16k;
use super::spsc::OverwriteSpsc;
use super::{AudioError, CapturedAudio, ENGINE_RATE};

#[derive(Debug, Clone)]
pub struct CaptureStats {
    pub device_name: String,
    pub native_rate: u32,
    pub native_format: String,
    pub native_channels: u16,
    pub input_frames: usize,
    pub input_samples: usize,
    pub output_samples: usize,
    pub overflow_frames: u64,
    pub convert_resample_secs: f64,
}

enum TypedRing {
    I8(Arc<OverwriteSpsc<i8>>),
    U8(Arc<OverwriteSpsc<u8>>),
    I16(Arc<OverwriteSpsc<i16>>),
    U16(Arc<OverwriteSpsc<u16>>),
    I32(Arc<OverwriteSpsc<i32>>),
    U32(Arc<OverwriteSpsc<u32>>),
    I64(Arc<OverwriteSpsc<i64>>),
    F32(Arc<OverwriteSpsc<f32>>),
    F64(Arc<OverwriteSpsc<f64>>),
}

impl TypedRing {
    fn reset(&self) {
        match self {
            Self::I8(r) => r.reset(),
            Self::U8(r) => r.reset(),
            Self::I16(r) => r.reset(),
            Self::U16(r) => r.reset(),
            Self::I32(r) => r.reset(),
            Self::U32(r) => r.reset(),
            Self::I64(r) => r.reset(),
            Self::F32(r) => r.reset(),
            Self::F64(r) => r.reset(),
        }
    }

    fn overflow(&self) -> u64 {
        match self {
            Self::I8(r) => r.overflow(),
            Self::U8(r) => r.overflow(),
            Self::I16(r) => r.overflow(),
            Self::U16(r) => r.overflow(),
            Self::I32(r) => r.overflow(),
            Self::U32(r) => r.overflow(),
            Self::I64(r) => r.overflow(),
            Self::F32(r) => r.overflow(),
            Self::F64(r) => r.overflow(),
        }
    }

    fn drain_f32(&self) -> Vec<f32> {
        match self {
            Self::I8(r) => drain_map(r, i8_interleaved_to_f32),
            Self::U8(r) => drain_map(r, u8_interleaved_to_f32),
            Self::I16(r) => drain_map(r, i16_interleaved_to_f32),
            Self::U16(r) => drain_map(r, u16_interleaved_to_f32),
            Self::I32(r) => drain_map(r, i32_interleaved_to_f32),
            Self::U32(r) => drain_map(r, u32_interleaved_to_f32),
            Self::I64(r) => drain_map(r, i64_interleaved_to_f32),
            Self::F32(r) => {
                let mut raw = Vec::new();
                r.drain(&mut raw);
                raw
            }
            Self::F64(r) => drain_map(r, f64_interleaved_to_f32),
        }
    }
}

fn drain_map<T: Copy + Default>(ring: &OverwriteSpsc<T>, map: fn(&[T]) -> Vec<f32>) -> Vec<f32> {
    let mut raw = Vec::new();
    ring.drain(&mut raw);
    map(&raw)
}

pub struct CpalAudioSource {
    wanted_device: String,
    max_duration_secs: u32,
    stream: Option<Stream>,
    ring: Option<TypedRing>,
    lost: Arc<AtomicBool>,
    recording: bool,
    native_rate: u32,
    native_channels: u16,
    native_format: String,
    device_name: String,
    last_stats: Option<CaptureStats>,
}

impl CpalAudioSource {
    pub fn new(config: &AudioConfig) -> Self {
        Self {
            wanted_device: config.device.clone(),
            max_duration_secs: config.max_duration_secs.max(1),
            stream: None,
            ring: None,
            lost: Arc::new(AtomicBool::new(false)),
            recording: false,
            native_rate: 0,
            native_channels: 0,
            native_format: String::new(),
            device_name: String::new(),
            last_stats: None,
        }
    }

    pub fn last_stats(&self) -> Option<&CaptureStats> {
        self.last_stats.as_ref()
    }

    fn host_and_device(&self) -> Result<(cpal::Host, cpal::Device), AudioError> {
        let host = cpal::default_host();
        let device = if self.wanted_device.is_empty() || self.wanted_device == "default" {
            host.default_input_device()
                .ok_or_else(|| AudioError::Failed("kein Default-Eingabegerät".into()))?
        } else {
            let wanted = self.wanted_device.clone();
            host.input_devices()
                .map_err(|e| AudioError::Failed(format!("Geräteliste: {e}")))?
                .find(|d| d.to_string() == wanted)
                .ok_or_else(|| {
                    AudioError::Failed(format!(
                        "Eingabegerät {:?} nicht gefunden",
                        self.wanted_device
                    ))
                })?
        };
        Ok((host, device))
    }

    fn open(&mut self) -> Result<(), AudioError> {
        self.stream = None;
        self.ring = None;
        let (_host, device) = self.host_and_device()?;
        let name = device
            .description()
            .map(|d| d.name().to_string())
            .unwrap_or_else(|_| device.to_string());
        let supported: SupportedStreamConfig = device
            .default_input_config()
            .map_err(|e| AudioError::Failed(format!("Input-Config: {e}")))?;
        let sample_format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let rate = config.sample_rate;
        let channels = config.channels;
        let min_samples = (u64::from(self.max_duration_secs + 2)
            * u64::from(rate)
            * u64::from(channels)) as usize;
        self.lost.store(false, Ordering::Release);

        let (stream, ring) = match sample_format {
            SampleFormat::I8 => build_i8(&device, &config, min_samples, channels, &self.lost)?,
            SampleFormat::U8 => build_u8(&device, &config, min_samples, channels, &self.lost)?,
            SampleFormat::I16 => build_i16(&device, &config, min_samples, channels, &self.lost)?,
            SampleFormat::U16 => build_u16(&device, &config, min_samples, channels, &self.lost)?,
            SampleFormat::I32 => build_i32(&device, &config, min_samples, channels, &self.lost)?,
            SampleFormat::U32 => build_u32(&device, &config, min_samples, channels, &self.lost)?,
            SampleFormat::I64 => build_i64(&device, &config, min_samples, channels, &self.lost)?,
            SampleFormat::F32 => build_f32(&device, &config, min_samples, channels, &self.lost)?,
            SampleFormat::F64 => build_f64(&device, &config, min_samples, channels, &self.lost)?,
            other => {
                return Err(AudioError::Failed(format!(
                    "Sampleformat {other:?} wird nicht unterstützt"
                )));
            }
        };

        self.native_rate = rate;
        self.native_channels = channels;
        self.native_format = format!("{sample_format:?}");
        self.device_name = name;
        self.stream = Some(stream);
        self.ring = Some(ring);
        Ok(())
    }
}

fn err_fn(lost: Arc<AtomicBool>) -> impl FnMut(cpal::Error) + Send + 'static {
    move |err| {
        if err.kind() != ErrorKind::Xrun {
            lost.store(true, Ordering::Release);
        }
    }
}

macro_rules! impl_build {
    ($name:ident, $ty:ty, $variant:ident) => {
        fn $name(
            device: &cpal::Device,
            config: &StreamConfig,
            min_samples: usize,
            channels: u16,
            lost: &Arc<AtomicBool>,
        ) -> Result<(Stream, TypedRing), AudioError> {
            let ring = Arc::new(OverwriteSpsc::<$ty>::new(min_samples, channels as usize));
            let prod = ring.clone();
            let ch = channels as usize;
            let stream = device
                .build_input_stream(
                    *config,
                    move |data: &[$ty], _| {
                        for frame in data.chunks_exact(ch) {
                            prod.push_frame(frame);
                        }
                    },
                    err_fn(lost.clone()),
                    None,
                )
                .map_err(|e| AudioError::Failed(format!("Stream {}: {e}", stringify!($ty))))?;
            Ok((stream, TypedRing::$variant(ring)))
        }
    };
}

impl_build!(build_i8, i8, I8);
impl_build!(build_u8, u8, U8);
impl_build!(build_i16, i16, I16);
impl_build!(build_u16, u16, U16);
impl_build!(build_i32, i32, I32);
impl_build!(build_u32, u32, U32);
impl_build!(build_i64, i64, I64);
impl_build!(build_f32, f32, F32);
impl_build!(build_f64, f64, F64);

impl super::AudioSource for CpalAudioSource {
    fn start(&mut self) -> Result<(), AudioError> {
        if self.recording {
            return Ok(());
        }
        let lost = self.lost.load(Ordering::Acquire);
        if lost || self.stream.is_none() {
            self.open()?;
        }
        if let Some(ring) = &self.ring {
            ring.reset();
        }
        self.stream
            .as_ref()
            .ok_or_else(|| AudioError::Failed("Stream nicht geöffnet".into()))?
            .play()
            .map_err(|e| AudioError::Failed(format!("Stream play: {e}")))?;
        self.recording = true;
        Ok(())
    }

    fn stop(&mut self) -> Result<CapturedAudio, AudioError> {
        self.recording = false;
        // Stream zuerst droppen/joinen, bevor drain/reset (codex H5).
        if let Some(stream) = self.stream.take() {
            let _ = stream.pause();
            drop(stream);
        }
        let Some(ring) = &self.ring else {
            return Err(AudioError::Failed("keine Aufnahme".into()));
        };
        let overflow = ring.overflow();
        let interleaved = ring.drain_f32();
        let input_samples = interleaved.len();
        let channels = self.native_channels.max(1) as usize;
        let input_frames = input_samples / channels;
        let t0 = Instant::now();
        let mono = downmix_interleaved(&interleaved, channels);
        let mut samples = resample_mono_to_16k(&mono, self.native_rate)?;
        let cap = (self.max_duration_secs as usize).saturating_mul(ENGINE_RATE as usize);
        if samples.len() > cap {
            samples.truncate(cap);
        }
        let convert_resample_secs = t0.elapsed().as_secs_f64();
        let output_samples = samples.len();
        self.last_stats = Some(CaptureStats {
            device_name: self.device_name.clone(),
            native_rate: self.native_rate,
            native_format: self.native_format.clone(),
            native_channels: self.native_channels,
            input_frames,
            input_samples,
            output_samples,
            overflow_frames: overflow,
            convert_resample_secs,
        });
        Ok(CapturedAudio {
            samples,
            sample_rate: ENGINE_RATE,
        })
    }
}

/// Test-/Worker-Pipeline ohne Gerät: interleaved f32 → 16 kHz mono.
pub fn process_interleaved_f32(
    interleaved: &[f32],
    channels: u16,
    native_rate: u32,
    max_duration_secs: u32,
) -> Result<CapturedAudio, AudioError> {
    let mono = downmix_interleaved(interleaved, channels.max(1) as usize);
    let mut samples = resample_mono_to_16k(&mono, native_rate)?;
    let cap = (max_duration_secs.max(1) as usize).saturating_mul(ENGINE_RATE as usize);
    if samples.len() > cap {
        samples.truncate(cap);
    }
    Ok(CapturedAudio {
        samples,
        sample_rate: ENGINE_RATE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::AudioSource;

    struct FakeCapture {
        lost: bool,
        opened: u32,
        samples: Vec<f32>,
        pause_then_drop: bool,
        dropped: bool,
    }

    impl AudioSource for FakeCapture {
        fn start(&mut self) -> Result<(), AudioError> {
            if self.lost || self.opened == 0 {
                self.opened += 1;
                self.lost = false;
            }
            Ok(())
        }

        fn stop(&mut self) -> Result<CapturedAudio, AudioError> {
            if self.pause_then_drop {
                self.dropped = true;
            }
            Ok(CapturedAudio {
                samples: self.samples.clone(),
                sample_rate: ENGINE_RATE,
            })
        }
    }

    #[test]
    fn device_lost_reopens_on_next_start() {
        let mut src = FakeCapture {
            lost: false,
            opened: 0,
            samples: vec![0.1; 100],
            pause_then_drop: false,
            dropped: false,
        };
        src.start().unwrap();
        assert_eq!(src.opened, 1);
        src.lost = true;
        src.stop().unwrap();
        src.start().unwrap();
        assert_eq!(src.opened, 2);
    }

    #[test]
    fn pause_failure_still_drops_before_drain() {
        let mut src = FakeCapture {
            lost: false,
            opened: 0,
            samples: vec![0.2; 50],
            pause_then_drop: true,
            dropped: false,
        };
        src.start().unwrap();
        let out = src.stop().unwrap();
        assert!(src.dropped);
        assert_eq!(out.samples.len(), 50);
    }
}
