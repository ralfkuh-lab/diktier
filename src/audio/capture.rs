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
use super::gate::CaptureGate;
use super::resample::resample_mono_to_16k;
use super::spsc::OverwriteSpsc;
use super::{AudioError, CapturedAudio, ENGINE_RATE};

/// Frist, in der der cpal-Callback das Gate verlassen haben muss (codex H1).
/// Läuft sie ab, gilt die Aufnahme als gescheitert — gelesen wird nicht.
const PRODUCER_IDLE_LIMIT: std::time::Duration = std::time::Duration::from_millis(500);

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
    /// Nimmt der cpal-Callback die Frames an, und steht gerade einer im Ring?
    /// Scharf nur zwischen `start()` und `stop()`; sonst verwirft der Callback
    /// sofort (der Stream läuft trotzdem weiter, damit das Gerät nicht
    /// suspendiert).
    gate: Arc<CaptureGate>,
    recording: bool,
    native_rate: u32,
    native_channels: u16,
    native_format: String,
    device_name: String,
    last_stats: Option<CaptureStats>,
    last_open_secs: Option<f64>,
}

impl CpalAudioSource {
    pub fn new(config: &AudioConfig) -> Self {
        Self {
            wanted_device: config.device.clone(),
            max_duration_secs: config.max_duration_secs.max(1),
            stream: None,
            ring: None,
            lost: Arc::new(AtomicBool::new(false)),
            gate: Arc::new(CaptureGate::new()),
            recording: false,
            native_rate: 0,
            native_channels: 0,
            native_format: String::new(),
            device_name: String::new(),
            last_stats: None,
            last_open_secs: None,
        }
    }

    pub fn last_stats(&self) -> Option<&CaptureStats> {
        self.last_stats.as_ref()
    }

    /// Öffnungszeit des zuletzt aufgebauten Streams — die Zeit, die ein Diktat
    /// ohne Vorbereitung am Anfang verlöre.
    pub fn last_open_secs(&self) -> Option<f64> {
        self.last_open_secs
    }

    /// Läuft ein Stream (aufnahmebereit; ob er Frames annimmt, sagt `armed`)?
    pub fn is_open(&self) -> bool {
        self.stream.is_some() && !self.lost.load(Ordering::Acquire)
    }

    /// Nimmt der Callback gerade Frames an? Nur während einer Aufnahme.
    pub fn is_armed(&self) -> bool {
        self.gate.is_armed()
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
        let opened_at = Instant::now();
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

        // Frischer Ring, frisches Gate: ein Callback der alten Sitzung hält den
        // alten `Arc` und kann den neuen Ring nicht mehr sehen.
        self.gate = Arc::new(CaptureGate::new());
        let gate = &self.gate;
        let (stream, ring) = match sample_format {
            SampleFormat::I8 => {
                build_i8(&device, &config, min_samples, channels, &self.lost, gate)?
            }
            SampleFormat::U8 => {
                build_u8(&device, &config, min_samples, channels, &self.lost, gate)?
            }
            SampleFormat::I16 => {
                build_i16(&device, &config, min_samples, channels, &self.lost, gate)?
            }
            SampleFormat::U16 => {
                build_u16(&device, &config, min_samples, channels, &self.lost, gate)?
            }
            SampleFormat::I32 => {
                build_i32(&device, &config, min_samples, channels, &self.lost, gate)?
            }
            SampleFormat::U32 => {
                build_u32(&device, &config, min_samples, channels, &self.lost, gate)?
            }
            SampleFormat::I64 => {
                build_i64(&device, &config, min_samples, channels, &self.lost, gate)?
            }
            SampleFormat::F32 => {
                build_f32(&device, &config, min_samples, channels, &self.lost, gate)?
            }
            SampleFormat::F64 => {
                build_f64(&device, &config, min_samples, channels, &self.lost, gate)?
            }
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
        self.last_open_secs = Some(opened_at.elapsed().as_secs_f64());
        Ok(())
    }

    /// `play()` ist idempotent; ein Fehler heißt Gerät verloren (§6.4).
    fn play_stream(&mut self) -> Result<(), AudioError> {
        let result = self
            .stream
            .as_ref()
            .ok_or_else(|| AudioError::Failed("Stream nicht geöffnet".into()))?
            .play()
            .map_err(|e| AudioError::Failed(format!("Stream play: {e}")));
        if result.is_err() {
            self.lost.store(true, Ordering::Release);
            self.stream = None;
        }
        result
    }

    /// Gerät und Ring wegwerfen, weil der Producer nicht zur Ruhe kam
    /// (codex H1). Der `Arc` auf Ring und Gate lebt im Callback weiter, bis der
    /// Stream-Drop ihn abräumt — der neue Lauf bekommt frische Exemplare.
    fn discard_after_stuck_producer(&mut self) {
        self.lost.store(true, Ordering::Release);
        self.stream = None;
        self.ring = None;
        self.gate = Arc::new(CaptureGate::new());
    }
}

fn err_fn(lost: Arc<AtomicBool>) -> impl FnMut(cpal::Error) + Send + 'static {
    move |err| {
        if err.kind() != ErrorKind::Xrun {
            lost.store(true, Ordering::Release);
        }
    }
}

/// Der komplette cpal-Callback: lock-frei, allokationsfrei (§6.4).
///
/// Außerhalb einer Aufnahme lässt das Gate niemanden herein — die Frames
/// werden sofort verworfen, nichts wird gepuffert, gespeichert oder
/// weitergereicht. Der Stream läuft trotzdem weiter, sonst suspendiert das
/// Gerät und der nächste Aufnahmestart wartet auf das Aufwecken (§5: „Aufnahme
/// aus `idle` startet sofort").
///
/// Der Guard hält den In-flight-Zähler über **alle** Ringzugriffe — daran
/// erkennt der Consumer, wann `drain`/`reset` sicher sind (codex H1).
#[inline]
fn push_if_armed<T: Copy + Default>(
    gate: &CaptureGate,
    ring: &OverwriteSpsc<T>,
    data: &[T],
    channels: usize,
) {
    let Some(_guard) = gate.enter() else {
        return;
    };
    for frame in data.chunks_exact(channels) {
        ring.push_frame(frame);
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
            gate: &Arc<CaptureGate>,
        ) -> Result<(Stream, TypedRing), AudioError> {
            let ring = Arc::new(OverwriteSpsc::<$ty>::new(min_samples, channels as usize));
            let prod = ring.clone();
            let gate = gate.clone();
            let ch = channels as usize;
            let stream = device
                .build_input_stream(
                    *config,
                    move |data: &[$ty], _| push_if_armed(&gate, &prod, data, ch),
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
    /// Gerät vorab öffnen, ohne aufzunehmen (Spec §5: „Aufnahme aus `idle`
    /// startet sofort"). Der Stream bleibt danach pausiert liegen; `start()`
    /// muss ihn nur noch entkorken.
    fn prepare(&mut self) -> Result<(), AudioError> {
        if self.recording {
            return Ok(());
        }
        if self.lost.load(Ordering::Acquire) || self.stream.is_none() {
            self.open()?;
        }
        // Laufen lassen, ohne anzunehmen: Ein nur geöffneter (pausierter)
        // Stream hält das Gerät nicht wach — PipeWire suspendiert die Quelle
        // nach wenigen Sekunden, und das Entkorken kostet dann wieder ~2 s.
        self.gate.disarm();
        self.play_stream()
    }

    /// §4.3: Bei `paused` gibt Diktier das Mikrofon wieder her. Der Stream wird
    /// gedroppt, die Quelle darf suspendieren; der nächste `prepare()`/`start()`
    /// öffnet neu (und zahlt dann wieder den Geräteanlauf).
    fn release(&mut self) {
        if self.recording {
            // Nie mitten in einer Aufnahme — die gehört dem laufenden Diktat.
            return;
        }
        self.gate.disarm();
        self.stream = None;
        self.ring = None;
    }

    fn start(&mut self) -> Result<(), AudioError> {
        if self.recording {
            return Ok(());
        }
        let lost = self.lost.load(Ordering::Acquire);
        if lost || self.stream.is_none() {
            self.open()?;
        }
        // `reset` gehört dem Consumer allein: erst entwaffnen, dann nachweislich
        // warten, bis kein Callback mehr im Ring steht (codex H1).
        self.gate.disarm();
        if !self.gate.wait_idle(PRODUCER_IDLE_LIMIT) {
            self.discard_after_stuck_producer();
            return Err(AudioError::Failed(
                "Aufnahme-Callback reagiert nicht — Gerät wird neu geöffnet".into(),
            ));
        }
        if let Some(ring) = &self.ring {
            ring.reset();
        }
        self.play_stream()?;
        self.gate.arm();
        self.recording = true;
        Ok(())
    }

    fn stop(&mut self) -> Result<CapturedAudio, AudioError> {
        self.recording = false;
        // Der Stream läuft weiter — nur die Annahme wird abgeschaltet. Würde
        // hier pausiert, suspendierte das Gerät und der nächste Aufnahmestart
        // kostete wieder ~2 s (gemessen auf Mint 22, Owner-Entscheidung 3c).
        self.gate.disarm();
        // codex H1: Erst wenn nachweislich kein Callback mehr im Ring steht,
        // darf gedraint werden. Kommt er nicht heraus, wird **nicht** gelesen —
        // die Aufnahme gilt als gescheitert und das Gerät wird neu aufgebaut.
        if !self.gate.wait_idle(PRODUCER_IDLE_LIMIT) {
            self.discard_after_stuck_producer();
            return Err(AudioError::Failed(
                "Aufnahme-Callback reagiert nicht — Aufnahme verworfen, Gerät wird neu geöffnet"
                    .into(),
            ));
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
    use std::time::Duration;

    /// Spiegelt die Öffnungslogik von [`CpalAudioSource`]: geöffnet wird nur,
    /// wenn kein Stream offen ist oder das Gerät als verloren gilt.
    struct FakeCapture {
        lost: bool,
        opened: u32,
        released: u32,
        open: bool,
        recording: bool,
        samples: Vec<f32>,
        pause_then_drop: bool,
        dropped: bool,
    }

    impl FakeCapture {
        fn new(samples: Vec<f32>) -> Self {
            Self {
                lost: false,
                opened: 0,
                released: 0,
                open: false,
                recording: false,
                samples,
                pause_then_drop: false,
                dropped: false,
            }
        }
    }

    impl AudioSource for FakeCapture {
        fn prepare(&mut self) -> Result<(), AudioError> {
            if self.lost || !self.open {
                self.opened += 1;
                self.lost = false;
                self.open = true;
            }
            Ok(())
        }

        fn release(&mut self) {
            if self.recording || !self.open {
                return;
            }
            self.open = false;
            self.released += 1;
        }

        fn start(&mut self) -> Result<(), AudioError> {
            if self.lost || !self.open {
                self.opened += 1;
                self.lost = false;
                self.open = true;
            }
            self.recording = true;
            Ok(())
        }

        fn stop(&mut self) -> Result<CapturedAudio, AudioError> {
            self.recording = false;
            if self.pause_then_drop {
                self.dropped = true;
                self.open = false;
            }
            Ok(CapturedAudio {
                samples: self.samples.clone(),
                sample_rate: ENGINE_RATE,
            })
        }
    }

    #[test]
    fn device_lost_reopens_on_next_start() {
        let mut src = FakeCapture::new(vec![0.1; 100]);
        src.start().unwrap();
        assert_eq!(src.opened, 1);
        src.lost = true;
        src.stop().unwrap();
        src.start().unwrap();
        assert_eq!(src.opened, 2);
    }

    #[test]
    fn pause_failure_still_drops_before_drain() {
        let mut src = FakeCapture::new(vec![0.2; 50]);
        src.pause_then_drop = true;
        src.start().unwrap();
        let out = src.stop().unwrap();
        assert!(src.dropped);
        assert_eq!(out.samples.len(), 50);
    }

    /// §5: Nach `prepare()` wartet der Aufnahmestart nicht mehr auf das Gerät —
    /// und über mehrere Diktate hinweg wird genau einmal geöffnet.
    #[test]
    fn prepare_opens_once_and_start_reuses_the_open_stream() {
        let mut src = FakeCapture::new(vec![0.3; 20]);
        src.prepare().unwrap();
        assert_eq!(src.opened, 1);
        src.prepare().unwrap();
        assert_eq!(src.opened, 1, "prepare ist idempotent");

        for _ in 0..3 {
            src.start().unwrap();
            src.stop().unwrap();
            src.prepare().unwrap();
        }
        assert_eq!(
            src.opened, 1,
            "der offene Stream überlebt Start/Stop — kein Neuöffnen je Diktat"
        );
    }

    /// §4.3: `paused` gibt das Mikrofon her, das Aufheben der Pause holt es
    /// zurück — beides beliebig oft, ohne dass etwas hängen bleibt.
    #[test]
    fn pause_releases_the_device_and_resume_reopens_it() {
        let mut src = FakeCapture::new(vec![0.5; 10]);
        src.prepare().unwrap();
        assert!(src.open);
        assert_eq!(src.opened, 1);

        src.release();
        assert!(!src.open, "pausiert heißt: kein offenes Mikrofon");
        assert_eq!(src.released, 1);
        src.release();
        assert_eq!(src.released, 1, "release ist idempotent");

        src.prepare().unwrap();
        assert!(src.open);
        assert_eq!(src.opened, 2, "das Aufheben der Pause öffnet neu");

        // Zweiter Durchlauf: Pause → weiter → Diktat funktioniert.
        src.release();
        src.prepare().unwrap();
        src.start().unwrap();
        let out = src.stop().unwrap();
        assert_eq!(out.samples.len(), 10);
        assert_eq!(src.opened, 3);
    }

    /// Eine laufende Aufnahme wird nie abgebrochen — auch nicht, wenn ein
    /// Freigeben dazwischenkäme (§4.3: Pause während `recording` verwirft den
    /// Lauf im Kern, der Stream gehört bis zum `stop()` dem Diktat).
    #[test]
    fn release_never_interrupts_a_running_capture() {
        let mut src = FakeCapture::new(vec![0.1; 32]);
        src.prepare().unwrap();
        src.start().unwrap();
        src.release();
        assert!(src.open, "Gerät bleibt während der Aufnahme offen");
        assert_eq!(src.released, 0);
        let out = src.stop().unwrap();
        assert_eq!(out.samples.len(), 32);
        src.release();
        assert_eq!(src.released, 1, "nach dem Stop greift das Freigeben");
    }

    /// §6.4: Ein verlorenes Gerät wird beim nächsten Vorbereiten einmal neu
    /// geöffnet — das Offenhalten darf die Recovery nicht aushebeln.
    #[test]
    fn prepare_reopens_a_lost_device() {
        let mut src = FakeCapture::new(vec![0.4; 10]);
        src.prepare().unwrap();
        assert_eq!(src.opened, 1);
        src.lost = true;
        src.prepare().unwrap();
        assert_eq!(src.opened, 2);
        src.start().unwrap();
        assert_eq!(src.opened, 2, "nach der Recovery kein zweites Öffnen");
    }

    /// Der laufende Stream im Ruhezustand darf **nichts** sammeln: der Callback
    /// verwirft jedes Frame, solange keine Aufnahme läuft.
    #[test]
    fn unarmed_callback_discards_every_frame() {
        let ring = OverwriteSpsc::<f32>::new(64, 1);
        let gate = CaptureGate::new();
        for _ in 0..50 {
            push_if_armed(&gate, &ring, &[0.5, 0.5, 0.5, 0.5], 1);
        }
        let mut out = Vec::new();
        ring.drain(&mut out);
        assert!(out.is_empty(), "Ruhezustand darf nichts puffern: {out:?}");
        assert_eq!(ring.overflow(), 0, "und auch keinen Overflow erzeugen");
        assert_eq!(ring.write_pos(), 0);
        assert_eq!(gate.in_flight(), 0, "kein Producer bleibt im Gate hängen");

        gate.arm();
        push_if_armed(&gate, &ring, &[0.25, 0.5], 1);
        let mut out = Vec::new();
        ring.drain(&mut out);
        assert_eq!(out, vec![0.25, 0.5], "scharf nimmt der Callback an");
        assert_eq!(gate.in_flight(), 0);
    }

    /// Stereo bleibt frame-aligned, auch über das Gate.
    #[test]
    fn armed_callback_keeps_frame_alignment() {
        let ring = OverwriteSpsc::<i16>::new(8, 2);
        let gate = CaptureGate::new();
        gate.arm();
        push_if_armed(&gate, &ring, &[1, 2, 3, 4, 5], 2);
        let mut out = Vec::new();
        ring.drain(&mut out);
        assert_eq!(out, vec![1, 2, 3, 4], "unvollständiges Frame bleibt liegen");
    }

    /// codex H1, der harte Fall: Ein Callback hängt **im** Ring fest. Dann darf
    /// der Consumer nicht drainen — er wirft die Aufnahme weg und baut das Gerät
    /// neu auf. Der Test hält den Producer an einer Barriere fest.
    #[test]
    fn stuck_producer_makes_the_take_fail_instead_of_racing_the_drain() {
        let gate = Arc::new(CaptureGate::new());
        let ring = Arc::new(OverwriteSpsc::<f32>::new(1_024, 1));
        gate.arm();

        let entered = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        let producer = {
            let (gate, ring) = (gate.clone(), ring.clone());
            let (entered, release) = (entered.clone(), release.clone());
            std::thread::spawn(move || {
                let guard = gate.enter().expect("Gate war scharf");
                ring.push_frame(&[0.5]);
                entered.wait();
                release.wait();
                ring.push_frame(&[0.75]);
                drop(guard);
            })
        };

        entered.wait();
        gate.disarm();
        // Genau das Fenster, das die alte Heuristik nicht abgedeckt hat:
        // Write-Cursor steht still, der Callback ist aber noch drin.
        assert_eq!(ring.write_pos(), 1);
        assert!(
            !gate.wait_idle(Duration::from_millis(20)),
            "der Consumer darf hier nicht 'ruhig' melden"
        );

        release.wait();
        producer.join().unwrap();
        assert!(gate.wait_idle(Duration::from_secs(2)));
        // Erst jetzt ist der Ring dem Consumer allein überlassen.
        let mut out = Vec::new();
        ring.drain(&mut out);
        assert_eq!(out, vec![0.5, 0.75]);
    }
}
