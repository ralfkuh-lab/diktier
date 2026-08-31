//! cpal-AudioSource: natives Gerät, lock-freier Callback, Worker-Pipeline.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, SampleFormat, Stream, StreamConfig, SupportedStreamConfig};

use crate::config::AudioConfig;

use super::convert::{
    ToUnitF32, downmix_interleaved, f64_interleaved_to_f32, i8_interleaved_to_f32,
    i16_interleaved_to_f32, i32_interleaved_to_f32, i64_interleaved_to_f32, u8_interleaved_to_f32,
    u16_interleaved_to_f32, u32_interleaved_to_f32,
};
use super::gate::CaptureGate;
use super::level::{self, LevelTap};
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
    /// Pegel fürs Aufnahme-Overlay (§4.5). `None` = Overlay aus; dann rechnet
    /// der Callback gar nicht erst (Overlay-Plan Leitentscheidung 2).
    level: Option<Arc<LevelTap>>,
    /// Generation des **aktuell offenen** Streams. Sein Callback trägt genau
    /// diesen Wert; wechselt er, ist der alte Stream für den Tap tot
    /// (Sol-Impl-Review Major 2).
    level_generation: u32,
    recording: bool,
    native_rate: u32,
    native_channels: u16,
    native_format: String,
    device_name: String,
    last_stats: Option<CaptureStats>,
    last_open_secs: Option<f64>,
}

impl CpalAudioSource {
    /// `level`: geteilter Pegel fürs Overlay oder `None`, wenn es aus ist.
    pub fn new(config: &AudioConfig, level: Option<Arc<LevelTap>>) -> Self {
        let level_generation = level.as_ref().map(|tap| tap.generation()).unwrap_or(0);
        Self {
            wanted_device: config.device.clone(),
            max_duration_secs: config.max_duration_secs.max(1),
            stream: None,
            ring: None,
            lost: Arc::new(AtomicBool::new(false)),
            gate: Arc::new(CaptureGate::new()),
            level,
            level_generation,
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
        // **Vor** dem ersten falliblen Schritt (Sol-Impl-Review Major 3): Der
        // alte Stream ist gerade gefallen, seine Generation ist damit tot —
        // auch wenn das Öffnen gleich scheitert. Der neue Callback bekommt die
        // frische Generation weiter unten mit.
        self.new_level_generation();
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
        let tap = self.level.as_ref();
        let generation = self.level_generation;
        let (stream, ring) = match sample_format {
            SampleFormat::I8 => build_i8(
                &device,
                &config,
                min_samples,
                channels,
                &self.lost,
                gate,
                tap,
                generation,
            )?,
            SampleFormat::U8 => build_u8(
                &device,
                &config,
                min_samples,
                channels,
                &self.lost,
                gate,
                tap,
                generation,
            )?,
            SampleFormat::I16 => build_i16(
                &device,
                &config,
                min_samples,
                channels,
                &self.lost,
                gate,
                tap,
                generation,
            )?,
            SampleFormat::U16 => build_u16(
                &device,
                &config,
                min_samples,
                channels,
                &self.lost,
                gate,
                tap,
                generation,
            )?,
            SampleFormat::I32 => build_i32(
                &device,
                &config,
                min_samples,
                channels,
                &self.lost,
                gate,
                tap,
                generation,
            )?,
            SampleFormat::U32 => build_u32(
                &device,
                &config,
                min_samples,
                channels,
                &self.lost,
                gate,
                tap,
                generation,
            )?,
            SampleFormat::I64 => build_i64(
                &device,
                &config,
                min_samples,
                channels,
                &self.lost,
                gate,
                tap,
                generation,
            )?,
            SampleFormat::F32 => build_f32(
                &device,
                &config,
                min_samples,
                channels,
                &self.lost,
                gate,
                tap,
                generation,
            )?,
            SampleFormat::F64 => build_f64(
                &device,
                &config,
                min_samples,
                channels,
                &self.lost,
                gate,
                tap,
                generation,
            )?,
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
            // Reset-Matrix (c): ein fehlgeschlagenes `play()` heißt Gerät
            // verloren. Der Stream ist weg — also stirbt auch seine
            // Generation, und das Overlay bleibt nicht auf dem letzten Peak
            // stehen.
            self.new_level_generation();
        }
        result
    }

    /// Neue Stream-Generation: Peak auf 0 **und** alle Callbacks des alten
    /// Streams dauerhaft abgeklemmt (Sol-Impl-Review Major 2 und 3).
    ///
    /// Genau hier liegt der Unterschied zu einem bloßen `clear()`: Ein
    /// Callback, der den Gate-Abschnitt schon betreten hat, läuft nach einem
    /// `disarm()` noch bis zu seinem Publish weiter. Ein gelöschter Peak wäre
    /// von ihm sofort wieder überschrieben; eine gewechselte Generation nicht
    /// — sein `compare_exchange` scheitert und er bricht ab.
    ///
    /// Aufgerufen wird das dort, wo ein Stream **entsteht oder verschwindet**:
    /// `open()`, `release()`, `discard_after_stuck_producer()` und
    /// fehlgeschlagenes `play()`.
    fn new_level_generation(&mut self) {
        if let Some(tap) = &self.level {
            self.level_generation = tap.bump_generation();
        }
    }

    /// Peak auf Stille, Generation unverändert — für die Stellen, an denen
    /// derselbe Stream weiterläuft: `start()` vor `arm()` und `stop()` nach
    /// `disarm()` + erfolgreichem `wait_idle()`.
    ///
    /// Der Zeitpunkt ist kein Detail: Weil der Tap **innerhalb** desselben
    /// Gate-Abschnitts wie die Ring-Writes publiziert wird, beweist
    /// `wait_idle()` auch das Ende der Tap-Publikation (Sol Major 3 des
    /// Plan-Reviews). Wo dieser Beweis fehlt, steht [`Self::new_level_generation`].
    fn clear_level(&self) {
        if let Some(tap) = &self.level {
            tap.clear();
        }
    }

    /// Gerät und Ring wegwerfen, weil der Producer nicht zur Ruhe kam
    /// (codex H1). Der `Arc` auf Ring und Gate lebt im Callback weiter, bis der
    /// Stream-Drop ihn abräumt — der neue Lauf bekommt frische Exemplare.
    fn discard_after_stuck_producer(&mut self) {
        self.lost.store(true, Ordering::Release);
        self.stream = None;
        self.ring = None;
        // Der neue Lauf bekommt ein **eigenes** Gate. Der hängende Callback
        // hält aber weiter den alten Guard und darf bis zu seinem Publish
        // laufen — deshalb reicht das Gate hier nicht, und die Generation
        // wechselt mit (Sol-Impl-Review Major 2).
        self.gate = Arc::new(CaptureGate::new());
        self.new_level_generation();
    }
}

/// §6.4: Ein Fehler außer Xrun heißt „Gerät verloren".
///
/// Der Pegel wird hier **nicht** angefasst (Sol-Impl-Review Major 3): Dieser
/// Callback entwaffnet kein Gate und wartet auf keinen In-flight-Zähler, ein
/// Reset von hier aus wäre also sofort von einem parallel laufenden
/// Daten-Callback überschreibbar. Der stabile Reset passiert auf dem
/// Owner-Thread — `is_open()` meldet das verlorene Gerät, der nächste
/// `prepare()`/`start()` läuft in `open()` und wechselt dort die Generation.
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
///
/// Der Pegel fürs Overlay (§4.5) wird **innerhalb** desselben Gate-Abschnitts
/// publiziert und trägt die Generation des Streams, zu dem dieser Callback
/// gehört. Ist `tap` `None` (Overlay aus) oder hat der Consumer sich
/// abgemeldet, kostet das einen Branch bzw. einen Relaxed-Load **pro
/// Callback-Buffer**, nicht pro Sample — gerechnet wird dann gar nichts
/// (Sol Major 4 des Plan-Reviews, Major 4 des Impl-Reviews).
#[inline]
fn push_if_armed<T: Copy + Default + ToUnitF32>(
    gate: &CaptureGate,
    ring: &OverwriteSpsc<T>,
    tap: Option<&LevelTap>,
    generation: u32,
    data: &[T],
    channels: usize,
) {
    let Some(_guard) = gate.enter() else {
        return;
    };
    for frame in data.chunks_exact(channels) {
        ring.push_frame(frame);
    }
    publish_level(tap, generation, data, channels);
}

/// Der Publish-Schritt aus [`push_if_armed`], als eigene Funktion — so kann
/// der Test einen Callback exakt an dieser Stelle anhalten (Sol-Impl-Review
/// Major 2 verlangt die Barriere **innerhalb** des betretenen Abschnitts).
#[inline]
fn publish_level<T: ToUnitF32>(
    tap: Option<&LevelTap>,
    generation: u32,
    data: &[T],
    channels: usize,
) {
    let Some(tap) = tap else {
        return;
    };
    // Erst fragen, dann rechnen: Ohne Consumer wird der Buffer nicht mehr
    // angefasst.
    if !tap.is_active() {
        return;
    }
    tap.publish(generation, level::buffer_peak(data, channels));
}

macro_rules! impl_build {
    ($name:ident, $ty:ty, $variant:ident) => {
        #[allow(clippy::too_many_arguments)]
        fn $name(
            device: &cpal::Device,
            config: &StreamConfig,
            min_samples: usize,
            channels: u16,
            lost: &Arc<AtomicBool>,
            gate: &Arc<CaptureGate>,
            level: Option<&Arc<LevelTap>>,
            generation: u32,
        ) -> Result<(Stream, TypedRing), AudioError> {
            let ring = Arc::new(OverwriteSpsc::<$ty>::new(min_samples, channels as usize));
            let prod = ring.clone();
            let gate = gate.clone();
            // Die Generation gehört zu **diesem** Stream und ändert sich nie
            // mehr: Wechselt der Owner-Thread sie, ist dieser Callback für den
            // Tap tot (Sol-Impl-Review Major 2).
            let tap = level.cloned();
            let ch = channels as usize;
            let stream = device
                .build_input_stream(
                    *config,
                    move |data: &[$ty], _| {
                        push_if_armed(&gate, &prod, tap.as_deref(), generation, data, ch)
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
        // Reset-Matrix (c): pausiert wird kein Pegel mehr angezeigt. Hier
        // fällt der Stream, ohne dass auf seinen Callback gewartet wird —
        // also die Generation wechseln, nicht bloß den Peak löschen.
        self.new_level_generation();
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
        // Reset-Matrix (a): **vor** `arm()`, damit kein Peak der letzten
        // Aufnahme in die neue hineinragt. Der Stream bleibt derselbe — seine
        // Generation muss also stehen bleiben, sonst publizierte sein
        // Callback nie wieder.
        self.clear_level();
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
        // Reset-Matrix (b): erst **nach** `disarm()` + erfolgreichem
        // `wait_idle()` — jetzt ist bewiesen, dass kein Callback mehr
        // publiziert. Während `transcribing`/`injecting` läuft die Waveform
        // damit sichtbar leer (Overlay-Plan Leitentscheidung 5).
        self.clear_level();
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
            push_if_armed(&gate, &ring, None, 0, &[0.5, 0.5, 0.5, 0.5], 1);
        }
        let mut out = Vec::new();
        ring.drain(&mut out);
        assert!(out.is_empty(), "Ruhezustand darf nichts puffern: {out:?}");
        assert_eq!(ring.overflow(), 0, "und auch keinen Overflow erzeugen");
        assert_eq!(ring.write_pos(), 0);
        assert_eq!(gate.in_flight(), 0, "kein Producer bleibt im Gate hängen");

        gate.arm();
        push_if_armed(&gate, &ring, None, 0, &[0.25, 0.5], 1);
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
        push_if_armed(&gate, &ring, None, 0, &[1, 2, 3, 4, 5], 2);
        let mut out = Vec::new();
        ring.drain(&mut out);
        assert_eq!(out, vec![1, 2, 3, 4], "unvollständiges Frame bleibt liegen");
    }

    /// §4.5: Der Pegel entsteht im selben Gate-Abschnitt wie die Ring-Writes.
    /// Also gilt auch für ihn: entwaffnet wird nichts publiziert.
    #[test]
    fn the_level_tap_is_published_only_inside_the_gate() {
        let ring = OverwriteSpsc::<f32>::new(64, 1);
        let gate = CaptureGate::new();
        let tap = level::new_tap();
        let generation = tap.generation();

        for _ in 0..10 {
            push_if_armed(&gate, &ring, Some(&tap), generation, &[0.9, -0.9], 1);
        }
        assert_eq!(
            tap.take(),
            0.0,
            "ein entwaffnetes Gate darf keinen Pegel durchlassen"
        );

        gate.arm();
        push_if_armed(&gate, &ring, Some(&tap), generation, &[0.25, -0.5, 0.1], 1);
        assert!((tap.take() - 0.5).abs() < 1e-6, "Betragspeak");
        assert_eq!(tap.take(), 0.0, "danach steht der Tap wieder auf 0");
    }

    /// Sol-Impl-Review Major 4: Meldet sich der Consumer ab, wird im Callback
    /// weder gerechnet noch publiziert — der Pegelpfad kostet dann nur noch
    /// den einen Relaxed-Load pro Buffer.
    #[test]
    fn a_deactivated_consumer_stops_the_level_path_in_the_callback() {
        let ring = OverwriteSpsc::<f32>::new(256, 1);
        let gate = CaptureGate::new();
        let tap = level::new_tap();
        let generation = tap.generation();
        gate.arm();

        push_if_armed(&gate, &ring, Some(&tap), generation, &[0.5], 1);
        assert!((tap.take() - 0.5).abs() < 1e-6);

        tap.deactivate();
        for _ in 0..100 {
            push_if_armed(&gate, &ring, Some(&tap), generation, &[1.0], 1);
        }
        assert_eq!(tap.take(), 0.0, "ohne Consumer wird nichts mehr publiziert");

        // Die Aufnahme selbst läuft davon unbeeindruckt weiter.
        let mut out = Vec::new();
        ring.drain(&mut out);
        assert_eq!(out.len(), 101, "der Ring bekommt weiter jedes Frame");
    }

    /// Sol-Impl-Review Major 2, der harte Fall: Der alte Callback hat den
    /// Gate-Abschnitt **schon betreten**, als der Owner-Thread den
    /// Stuck-Producer-Pfad läuft (`disarm`, neues Gate, neue Generation, neu
    /// `arm`). Erst danach kommt er zu seinem Publish — und darf den Pegel des
    /// neuen Streams trotzdem nicht anfassen.
    ///
    /// Die Barriere sitzt deshalb **innerhalb** des betretenen Abschnitts,
    /// genau an der Stelle, an der `push_if_armed` seinen Publish macht (das
    /// ist `publish_level`, dieselbe Zeile).
    #[test]
    fn an_in_flight_callback_of_the_old_stream_cannot_publish_into_the_new_one() {
        let tap = level::new_tap();
        let old_gate = Arc::new(CaptureGate::new());
        let old_ring = Arc::new(OverwriteSpsc::<f32>::new(64, 1));
        let old_generation = tap.generation();
        old_gate.arm();

        let entered = Arc::new(std::sync::Barrier::new(2));
        let switched = Arc::new(std::sync::Barrier::new(2));
        let straggler = {
            let (gate, ring, tap) = (old_gate.clone(), old_ring.clone(), tap.clone());
            let (entered, switched) = (entered.clone(), switched.clone());
            std::thread::spawn(move || {
                let guard = gate.enter().expect("Gate war scharf");
                ring.push_frame(&[0.8]);
                entered.wait();
                switched.wait();
                // Ab hier ist der Stream längst weggeworfen — der Callback
                // weiß das nicht und läuft in seinen Publish.
                publish_level(Some(&tap), old_generation, &[1.0_f32], 1);
                drop(guard);
            })
        };

        entered.wait();
        // Genau die Sequenz aus `stop()`/`discard_after_stuck_producer()`:
        old_gate.disarm();
        assert!(
            !old_gate.wait_idle(Duration::from_millis(20)),
            "der Producer steht noch im Gate — deshalb der Stuck-Pfad"
        );
        let new_gate = Arc::new(CaptureGate::new());
        let new_ring = OverwriteSpsc::<f32>::new(64, 1);
        let new_generation = tap.bump_generation();
        new_gate.arm();

        switched.wait();
        straggler.join().unwrap();
        assert_eq!(
            tap.take(),
            0.0,
            "der Nachzügler darf den neuen Lauf nicht vollaussteuern"
        );

        // … und der neue Stream schreibt ganz normal weiter.
        push_if_armed(&new_gate, &new_ring, Some(&tap), new_generation, &[0.3], 1);
        assert!((tap.take() - 0.3).abs() < 1e-6);
    }

    /// Sol-Impl-Review Major 3: Der Fehler-Callback markiert nur `lost`. Er
    /// entwaffnet kein Gate und wartet auf keinen In-flight-Zähler — ein Reset
    /// von dort wäre vom laufenden Daten-Callback sofort überschrieben. Der
    /// stabile Reset kommt vom Owner-Thread über die Generation.
    #[test]
    fn a_device_error_does_not_race_the_running_callback_for_the_tap() {
        let gate = CaptureGate::new();
        let ring = OverwriteSpsc::<f32>::new(64, 1);
        let tap = level::new_tap();
        let generation = tap.generation();
        let lost = Arc::new(AtomicBool::new(false));
        let mut on_error = err_fn(lost.clone());
        gate.arm();

        // Gerätefehler …
        on_error(cpal::Error::new(ErrorKind::DeviceNotAvailable));
        assert!(lost.load(Ordering::Acquire), "das Gerät gilt als verloren");

        // … und danach ein noch laufender Daten-Callback. Früher hätte der
        // gerade gelöschte Tap hier sofort wieder einen Peak bekommen.
        push_if_armed(&gate, &ring, Some(&tap), generation, &[0.9], 1);
        assert!((tap.take() - 0.9).abs() < 1e-6);

        // Erst der Owner-Thread räumt stabil ab: neue Generation, alter
        // Callback dauerhaft abgeklemmt.
        let next = tap.bump_generation();
        push_if_armed(&gate, &ring, Some(&tap), generation, &[1.0], 1);
        assert_eq!(tap.take(), 0.0, "der alte Callback kommt nicht mehr durch");
        assert_ne!(next, generation);

        // Ein Xrun ist kein Geräteverlust.
        let quiet = Arc::new(AtomicBool::new(false));
        let mut on_xrun = err_fn(quiet.clone());
        on_xrun(cpal::Error::new(ErrorKind::Xrun));
        assert!(!quiet.load(Ordering::Acquire));
    }

    /// Barrieren-Test analog `wait_idle_blocks_until_the_producer_left_the_gate`:
    /// Was `wait_idle()` als ruhig meldet, publiziert auch keinen Pegel mehr.
    #[test]
    fn no_level_is_published_after_disarm_and_a_successful_wait_idle() {
        let gate = Arc::new(CaptureGate::new());
        let ring = Arc::new(OverwriteSpsc::<f32>::new(1_024, 1));
        let tap = level::new_tap();
        let generation = tap.generation();
        gate.arm();

        let entered = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        let producer = {
            let (gate, ring, tap) = (gate.clone(), ring.clone(), tap.clone());
            let (entered, release) = (entered.clone(), release.clone());
            std::thread::spawn(move || {
                let guard = gate.enter().expect("Gate war scharf");
                entered.wait();
                release.wait();
                // Der Callback steht noch **im** Gate und publiziert erst jetzt.
                publish_level(Some(&tap), generation, &[0.6_f32], 1);
                ring.push_frame(&[0.6]);
                drop(guard);
            })
        };

        entered.wait();
        gate.disarm();
        assert!(
            !gate.wait_idle(Duration::from_millis(20)),
            "solange der Producer im Gate steht, ist es nicht ruhig"
        );

        release.wait();
        producer.join().unwrap();
        assert!(gate.wait_idle(Duration::from_secs(2)));
        // Genau hier räumt `stop()` den Tap ab (Reset-Matrix b) — der Stream
        // läuft weiter, also bleibt die Generation stehen …
        tap.clear();
        for _ in 0..100 {
            push_if_armed(&gate, &ring, Some(&tap), generation, &[1.0], 1);
        }
        assert_eq!(tap.take(), 0.0, "… und das entwaffnete Gate hält dicht");
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
