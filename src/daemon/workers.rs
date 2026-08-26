//! Worker-Threads des Daemons (Spec §5: „Worker-Thread für Inferenz,
//! cpal-Callback, Tray-Eventloop. Inferenz darf den Tray-Thread nicht
//! blockieren.").
//!
//! Alle Worker reden nur über Kanäle mit der Event-Loop: Kommandos hinein,
//! [`Msg`] heraus. Kein Worker ruft `transition` auf, und die Event-Loop
//! blockiert nie auf einem Worker — das ist die Bedingung dafür, dass
//! `QuitRequested` jederzeit greift (codex H4 zu §7.1 P6).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::audio::{AudioSource, CpalAudioSource};
use crate::config::{AudioConfig, OutputConfig};
use crate::engine::{ParakeetTranscriber, transcribe_pcm};
use crate::hotkey::{HotkeyBackend, HotkeyEvent, new_backend};
use crate::inject::{
    self, CaptureContext, ClipboardSave, CopyOnlyReason, InjectOutcome, OutputSink, WindowId,
};
use crate::state::{AppState, CopyReason, Event, InjectReport, RunId, Runtime};
use crate::tray::{self, TrayBackend, TrayError, TrayEvent};

use super::debug_wav;
use super::logging::Logger;

/// Alles, was aus den Workern in die Event-Loop zeigt.
pub enum Msg {
    /// Direktes Kern-Event.
    Event(Event),
    /// Fertige Aufnahme. Die Samples bleiben im Wiring, der Kern sieht nur die
    /// Länge (`Event::AudioReady`).
    Audio { run: RunId, samples: Vec<f32> },
    /// §4.4 / §10: Hotkey nicht registrierbar — Tray-Click bleibt bedienbar.
    HotkeyUnavailable(String),
    /// §10: Tray weg heißt kein GUI-Kanal mehr → Prozessende, Exit 1.
    TrayLost(String),
    /// §4.3-Menü „Config-Ordner öffnen" — kein Kern-Event.
    OpenConfigDir,
}

/// §4.3: Tray-Ereignisse, die den Kern erreichen. „Config-Ordner öffnen" ist
/// reine Wiring-Aktion und hat deshalb kein Kern-Event.
pub fn tray_event_to_core(event: TrayEvent) -> Option<Event> {
    match event {
        TrayEvent::LeftClick => Some(Event::TrayClickToggle),
        TrayEvent::TogglePause => Some(Event::PauseToggle),
        TrayEvent::Quit => Some(Event::QuitRequested),
        TrayEvent::OpenConfigDir => None,
    }
}

pub fn hotkey_event_to_core(event: HotkeyEvent) -> Event {
    match event {
        HotkeyEvent::Press => Event::HotkeyPress,
        HotkeyEvent::Release => Event::HotkeyRelease,
    }
}

/// §7.1/§7.3-Ausgang der Inject-Schicht auf die Kern-Abstraktion.
pub fn map_copy_reason(reason: CopyOnlyReason) -> CopyReason {
    match reason {
        CopyOnlyReason::FocusChanged => CopyReason::FocusChanged,
        CopyOnlyReason::FocusUnknown => CopyReason::FocusUnknown,
    }
}

/// Frist für den `SAVE_TARGETS`-Handshake innerhalb des Inject-Threads.
const SAVE_TARGETS_BUDGET: Duration = Duration::from_millis(1_500);

/// Join mit Frist — beim Quit darf kein Worker den Prozess festhalten (§5.2).
/// `true` heißt: Thread ist beendet.
pub fn join_with_timeout(join: JoinHandle<()>, timeout: Duration) -> bool {
    let (tx, rx) = mpsc::channel();
    let spawned = thread::Builder::new()
        .name("diktier-join".into())
        .spawn(move || {
            let _ = join.join();
            let _ = tx.send(());
        });
    if spawned.is_err() {
        return false;
    }
    rx.recv_timeout(timeout).is_ok()
}

// --------------------------------------------------------------- Engine

pub enum EngineCmd {
    Load { run: RunId },
    Transcribe { run: RunId, samples: Vec<f32> },
    Shutdown,
}

/// Modell resident auf einem eigenen Thread (§5).
pub struct EngineWorker {
    tx: Sender<EngineCmd>,
    join: Option<JoinHandle<()>>,
}

impl EngineWorker {
    pub fn spawn(model: String, threads: u32, out: Sender<Msg>, log: Arc<Logger>) -> Self {
        let (tx, rx) = mpsc::channel();
        let join = thread::Builder::new()
            .name("diktier-engine".into())
            .spawn(move || engine_loop(rx, out, &model, threads, &log))
            .ok();
        Self { tx, join }
    }

    pub fn load(&self, run: RunId) {
        let _ = self.tx.send(EngineCmd::Load { run });
    }

    pub fn transcribe(&self, run: RunId, samples: Vec<f32>) {
        let _ = self.tx.send(EngineCmd::Transcribe { run, samples });
    }

    pub fn request_shutdown(&self) {
        let _ = self.tx.send(EngineCmd::Shutdown);
    }

    /// Nach dem Watchdog (§5.2) wird die laufende Inferenz verworfen. Abbrechen
    /// lässt sie sich in `parakeet-rs` nicht — der Thread läuft aus und beendet
    /// sich danach selbst; seine Antwort trägt eine tote Generation und wird
    /// vom Kern verworfen. Ein frischer Worker übernimmt den Reinit.
    pub fn abandon(mut self) {
        let _ = self.tx.send(EngineCmd::Shutdown);
        self.join.take();
    }

    pub fn shutdown(&mut self, timeout: Duration) -> bool {
        self.request_shutdown();
        match self.join.take() {
            Some(join) => join_with_timeout(join, timeout),
            None => true,
        }
    }
}

fn engine_loop(rx: Receiver<EngineCmd>, out: Sender<Msg>, model: &str, threads: u32, log: &Logger) {
    let mut engine: Option<ParakeetTranscriber> = None;
    while let Ok(cmd) = rx.recv() {
        match cmd {
            EngineCmd::Load { run } => {
                let t0 = Instant::now();
                match ParakeetTranscriber::load(model, threads) {
                    Ok(loaded) => {
                        engine = Some(loaded);
                        log.info(format!(
                            "Modell geladen in {:.3} s ({model})",
                            t0.elapsed().as_secs_f64()
                        ));
                        let _ = out.send(Msg::Event(Event::ModelLoaded { run }));
                    }
                    Err(err) => {
                        log.error(format!("Modell laden: {err}"));
                        let _ = out.send(Msg::Event(Event::ModelLoadFailed {
                            run,
                            message: err.to_string(),
                        }));
                    }
                }
            }
            EngineCmd::Transcribe { run, samples } => {
                let Some(engine) = engine.as_mut() else {
                    let _ = out.send(Msg::Event(Event::TranscriptionFailed {
                        run,
                        message: "Modell ist nicht geladen".into(),
                    }));
                    continue;
                };
                let t0 = Instant::now();
                match transcribe_pcm(engine, &samples) {
                    Ok(result) => {
                        // §10: keine Transkripte ins Log — nur Länge und Zeit.
                        log.info(format!(
                            "Inferenz {:.3} s, {} Zeichen",
                            t0.elapsed().as_secs_f64(),
                            result.text.chars().count()
                        ));
                        let _ = out.send(Msg::Event(Event::TranscriptionDone {
                            run,
                            text: result.text,
                        }));
                    }
                    Err(err) => {
                        log.error(format!("Transkription: {err}"));
                        let _ = out.send(Msg::Event(Event::TranscriptionFailed {
                            run,
                            message: err.to_string(),
                        }));
                    }
                }
            }
            EngineCmd::Shutdown => break,
        }
    }
}

// ---------------------------------------------------------------- Audio

pub enum AudioCmd {
    /// §5: Gerät in `idle` vorab öffnen, damit der Aufnahmestart nicht wartet.
    Prepare,
    /// §4.3: Bei `paused` das Mikrofon wieder hergeben.
    Release,
    Start {
        run: RunId,
    },
    Stop {
        run: RunId,
        discard: bool,
    },
    Shutdown,
}

/// cpal lebt komplett auf diesem Thread — `Stream` ist nicht `Send`, und
/// Downmix/Resample beim Stop gehören ohnehin nicht in die Event-Loop (§6.4).
pub struct AudioWorker {
    tx: Sender<AudioCmd>,
    join: Option<JoinHandle<()>>,
}

impl AudioWorker {
    pub fn spawn(config: AudioConfig, out: Sender<Msg>, log: Arc<Logger>) -> Self {
        let (tx, rx) = mpsc::channel();
        let join = thread::Builder::new()
            .name("diktier-audio".into())
            .spawn(move || audio_loop(rx, out, &config, &log))
            .ok();
        Self { tx, join }
    }

    /// Idempotent: der Worker öffnet nur, wenn kein Stream bereitsteht.
    pub fn prepare(&self) {
        let _ = self.tx.send(AudioCmd::Prepare);
    }

    /// Idempotent: der Worker gibt nur her, was offen ist.
    pub fn release(&self) {
        let _ = self.tx.send(AudioCmd::Release);
    }

    pub fn start(&self, run: RunId) {
        let _ = self.tx.send(AudioCmd::Start { run });
    }

    pub fn stop(&self, run: RunId, discard: bool) {
        let _ = self.tx.send(AudioCmd::Stop { run, discard });
    }

    pub fn shutdown(&mut self, timeout: Duration) -> bool {
        let _ = self.tx.send(AudioCmd::Shutdown);
        match self.join.take() {
            Some(join) => join_with_timeout(join, timeout),
            None => true,
        }
    }
}

fn audio_loop(rx: Receiver<AudioCmd>, out: Sender<Msg>, config: &AudioConfig, log: &Logger) {
    let mut source = CpalAudioSource::new(config);
    let mut recording = false;
    while let Ok(cmd) = rx.recv() {
        match cmd {
            AudioCmd::Prepare => {
                if source.is_open() {
                    continue;
                }
                let t0 = Instant::now();
                match source.prepare() {
                    Ok(()) => log.info(format!(
                        "Aufnahmegerät bereit in {:.3} s (Stream läuft, Frames werden verworfen)",
                        t0.elapsed().as_secs_f64()
                    )),
                    // §6.4: kein Fehlerzustand — der nächste Press versucht es
                    // erneut, dann meldet `start()` einen echten `CaptureFailed`.
                    Err(err) => log.warn(format!("Aufnahmegerät nicht vorbereitet: {err}")),
                }
            }
            AudioCmd::Release => {
                if !source.is_open() {
                    continue;
                }
                source.release();
                log.info("Aufnahmegerät freigegeben (pausiert)");
            }
            AudioCmd::Start { run } => {
                let was_open = source.is_open();
                let t0 = Instant::now();
                match source.start() {
                    Ok(()) => {
                        recording = true;
                        log.run(
                            run,
                            format!(
                                "Aufnahme läuft nach {:.3} s ({})",
                                t0.elapsed().as_secs_f64(),
                                if was_open {
                                    "Gerät war vorbereitet"
                                } else {
                                    "Gerät musste geöffnet werden"
                                }
                            ),
                        );
                    }
                    Err(err) => {
                        recording = false;
                        log.error(format!("Mikrofon: {err}"));
                        let _ = out.send(Msg::Event(Event::CaptureFailed {
                            run,
                            message: err.to_string(),
                        }));
                    }
                }
            }
            AudioCmd::Stop { run, discard } => {
                if !recording {
                    // Nichts offen (z. B. Start schlug fehl) — nichts zu melden.
                    continue;
                }
                recording = false;
                match source.stop() {
                    Ok(captured) => {
                        if let Some(stats) = source.last_stats() {
                            log.info(format!(
                                "Capture: {} · {} Hz {} {} ch · {} Frames → {} Samples · overflow {} · Konvertierung {:.3} s",
                                stats.device_name,
                                stats.native_rate,
                                stats.native_format,
                                stats.native_channels,
                                stats.input_frames,
                                stats.output_samples,
                                stats.overflow_frames,
                                stats.convert_resample_secs
                            ));
                        }
                        if discard {
                            log.run(run, "Aufnahme verworfen");
                            continue;
                        }
                        dump_debug_wav(&captured.samples, log);
                        let _ = out.send(Msg::Audio {
                            run,
                            samples: captured.samples,
                        });
                    }
                    Err(err) => {
                        log.error(format!("Aufnahme beenden: {err}"));
                        if !discard {
                            let _ = out.send(Msg::Event(Event::CaptureFailed {
                                run,
                                message: err.to_string(),
                            }));
                        }
                    }
                }
            }
            AudioCmd::Shutdown => {
                if recording {
                    let _ = source.stop();
                }
                break;
            }
        }
    }
}

/// §10 `DIKTIER_DEBUG_WAV=1`: genau ein Dump, genau eine Logzeile.
fn dump_debug_wav(samples: &[f32], log: &Logger) {
    if !debug_wav::enabled() {
        return;
    }
    match debug_wav::write_last_recording(&debug_wav::debug_dir(), samples) {
        Ok(path) => log.info(format!("DIKTIER_DEBUG_WAV: {}", path.display())),
        Err(err) => log.warn(format!("DIKTIER_DEBUG_WAV fehlgeschlagen: {err}")),
    }
}

// --------------------------------------------------------------- Inject

pub enum InjectCmd {
    /// §7.3: Vordergrund beim Aufnahmestart merken.
    MarkStart {
        run: RunId,
    },
    /// §7.3: Vordergrund beim Aufnahmeende (Release oder Cap) merken.
    MarkTarget {
        run: RunId,
    },
    Paste {
        run: RunId,
        text: String,
    },
    CopyOnly {
        run: RunId,
        text: String,
        reason: CopyReason,
    },
    /// Quit-Pfad: Clipboard an den Clipboard-Manager übergeben.
    SaveTargets {
        reply: Sender<ClipboardSave>,
    },
    Shutdown,
}

/// Die X11-Connection lebt hier — inklusive des bis zu 5 s langen
/// Restore-Wartens aus §7.1 P7. Genau deshalb ist der Paste ein eigener Thread:
/// die Event-Loop bleibt reaktiv, `QuitRequested` greift jederzeit (codex H4).
pub struct InjectWorker {
    tx: Sender<InjectCmd>,
    join: Option<JoinHandle<()>>,
}

impl InjectWorker {
    pub fn spawn(
        output: OutputConfig,
        out: Sender<Msg>,
        log: Arc<Logger>,
    ) -> Result<Self, inject::InjectError> {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let join = thread::Builder::new()
            .name("diktier-inject".into())
            .spawn(move || inject_loop(rx, out, output, &ready_tx, &log))
            .map_err(|e| inject::InjectError::Failed(format!("Inject-Thread: {e}")))?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self {
                tx,
                join: Some(join),
            }),
            Ok(Err(message)) => {
                join_with_timeout(join, Duration::from_secs(2));
                Err(inject::InjectError::Failed(message))
            }
            Err(_) => {
                join_with_timeout(join, Duration::from_secs(2));
                Err(inject::InjectError::Failed(
                    "Inject-Thread antwortet nicht".into(),
                ))
            }
        }
    }

    pub fn mark_start(&self, run: RunId) {
        let _ = self.tx.send(InjectCmd::MarkStart { run });
    }

    pub fn mark_target(&self, run: RunId) {
        let _ = self.tx.send(InjectCmd::MarkTarget { run });
    }

    pub fn paste(&self, run: RunId, text: String) {
        let _ = self.tx.send(InjectCmd::Paste { run, text });
    }

    pub fn copy_only(&self, run: RunId, text: String, reason: CopyReason) {
        let _ = self.tx.send(InjectCmd::CopyOnly { run, text, reason });
    }

    /// Blockiert höchstens `timeout` — der Thread kann noch in einem Paste stehen.
    pub fn save_targets(&self, timeout: Duration) -> ClipboardSave {
        let (reply_tx, reply_rx) = mpsc::channel();
        if self
            .tx
            .send(InjectCmd::SaveTargets { reply: reply_tx })
            .is_err()
        {
            return ClipboardSave::NotOwner;
        }
        reply_rx
            .recv_timeout(timeout)
            .unwrap_or(ClipboardSave::Timeout)
    }

    pub fn shutdown(&mut self, timeout: Duration) -> bool {
        let _ = self.tx.send(InjectCmd::Shutdown);
        match self.join.take() {
            Some(join) => join_with_timeout(join, timeout),
            None => true,
        }
    }
}

/// §7.3-Buchführung: eine Generation, zwei Fensterkennungen.
#[derive(Debug, Default)]
struct ContextSlot {
    run: Option<RunId>,
    start: Option<WindowId>,
    target: Option<WindowId>,
    ended_at: Option<Instant>,
}

impl ContextSlot {
    fn context_for(&self, run: RunId) -> CaptureContext {
        if self.run == Some(run) {
            CaptureContext {
                start_window_id: self.start,
                target_window_id: self.target,
                ended_at: self.ended_at.unwrap_or_else(Instant::now),
            }
        } else {
            // Fremde Generation: keine belastbare Kennung → Fokusverlust (§7.3).
            CaptureContext {
                start_window_id: None,
                target_window_id: None,
                ended_at: Instant::now(),
            }
        }
    }
}

fn inject_loop(
    rx: Receiver<InjectCmd>,
    out: Sender<Msg>,
    output: OutputConfig,
    ready: &Sender<Result<(), String>>,
    log: &Logger,
) {
    let mut sink = match inject::new_sink(output) {
        Ok(sink) => {
            let _ = ready.send(Ok(()));
            sink
        }
        Err(err) => {
            let _ = ready.send(Err(err.to_string()));
            return;
        }
    };
    let mut slot = ContextSlot::default();

    loop {
        match rx.try_recv() {
            Ok(InjectCmd::MarkStart { run }) => {
                slot = ContextSlot {
                    run: Some(run),
                    start: sink.current_window_id(),
                    target: None,
                    ended_at: None,
                };
                log.run(run, format!("Startfenster {}", window_str(slot.start)));
            }
            Ok(InjectCmd::MarkTarget { run }) => {
                if slot.run == Some(run) {
                    slot.target = sink.current_window_id();
                    slot.ended_at = Some(Instant::now());
                    log.run(run, format!("Zielfenster {}", window_str(slot.target)));
                }
            }
            Ok(InjectCmd::Paste { run, text }) => {
                let ctx = slot.context_for(run);
                let report = match sink.paste(&text, &ctx) {
                    Ok(InjectOutcome::Pasted {
                        restored,
                        shortcut,
                        reads,
                        restore,
                        ..
                    }) => {
                        log.run(
                            run,
                            format!(
                                "Paste {} · {} Bytes · reads {reads} · restore {} ({})",
                                shortcut.as_str(),
                                text.len(),
                                restored,
                                restore.as_str()
                            ),
                        );
                        InjectReport::Pasted
                    }
                    Ok(InjectOutcome::CopyOnly { reason }) => {
                        log.run(run, format!("copy_only: {}", reason.as_str()));
                        InjectReport::CopyOnly {
                            reason: map_copy_reason(reason),
                        }
                    }
                    Err(err) => {
                        log.error(format!("Einfügen: {err}"));
                        InjectReport::Failed {
                            message: err.to_string(),
                        }
                    }
                };
                let _ = out.send(Msg::Event(Event::InjectFinished { run, report }));
            }
            Ok(InjectCmd::CopyOnly { run, text, reason }) => {
                let report = match sink.copy_only(&text) {
                    Ok(()) => {
                        log.run(run, format!("copy_only · {} Bytes", text.len()));
                        InjectReport::CopyOnly { reason }
                    }
                    Err(err) => {
                        log.error(format!("Clipboard: {err}"));
                        InjectReport::Failed {
                            message: err.to_string(),
                        }
                    }
                };
                let _ = out.send(Msg::Event(Event::InjectFinished { run, report }));
            }
            Ok(InjectCmd::SaveTargets { reply }) => {
                let saved = match sink.save_to_clipboard_manager(SAVE_TARGETS_BUDGET) {
                    Ok(saved) => saved,
                    Err(err) => {
                        log.warn(format!("SAVE_TARGETS: {err}"));
                        ClipboardSave::Timeout
                    }
                };
                let _ = reply.send(saved);
            }
            Ok(InjectCmd::Shutdown) | Err(TryRecvError::Disconnected) => break,
            Err(TryRecvError::Empty) => {
                // §7.1 P8: solange Diktier Owner ist, muss die Selection
                // bedient werden — sonst hängt jedes fremde Paste.
                if let Err(err) = sink.serve_for(Duration::from_millis(10)) {
                    log.warn(format!("X11-Selection: {err}"));
                }
            }
        }
    }
}

fn window_str(id: Option<WindowId>) -> String {
    match id {
        Some(id) => format!("0x{:x}", id.0),
        None => "unbekannt".into(),
    }
}

// --------------------------------------------------------------- Hotkey

/// §5: eigener Thread, entprellte Press/Release-Events in den Kanal.
pub struct HotkeyWorker {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl HotkeyWorker {
    pub fn spawn(out: Sender<Msg>, log: Arc<Logger>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let join = thread::Builder::new()
            .name("diktier-hotkey".into())
            .spawn(move || hotkey_loop(&flag, &out, &log))
            .ok();
        Self { stop, join }
    }

    pub fn shutdown(&mut self, timeout: Duration) -> bool {
        self.stop.store(true, Ordering::Release);
        match self.join.take() {
            Some(join) => join_with_timeout(join, timeout),
            None => true,
        }
    }
}

fn hotkey_loop(stop: &AtomicBool, out: &Sender<Msg>, log: &Logger) {
    let mut backend = new_backend();
    let name = backend.backend_name();
    if let Err(err) = backend.register() {
        log.error(format!("Hotkey-Registrierung: {err}"));
        let _ = out.send(Msg::HotkeyUnavailable(err.to_string()));
        return;
    }
    if name == "stub" {
        // §4.4: kein Backend heißt kein PTT — Tray-Click bleibt der Weg.
        let _ = out.send(Msg::HotkeyUnavailable(
            "kein Hotkey-Backend verfügbar (global-hotkey und XGrabKey fehlgeschlagen)".into(),
        ));
        return;
    }
    log.info(format!("Hotkey-Backend: {name} (F9, Push-to-Talk)"));

    while !stop.load(Ordering::Acquire) {
        match backend.poll() {
            Ok(Some(event)) => {
                if out.send(Msg::Event(hotkey_event_to_core(event))).is_err() {
                    return;
                }
            }
            Ok(None) => thread::sleep(Duration::from_millis(5)),
            Err(err) => {
                log.error(format!("Hotkey: {err}"));
                let _ = out.send(Msg::HotkeyUnavailable(err.to_string()));
                return;
            }
        }
    }
}

// ----------------------------------------------------------------- Tray

pub enum TrayCmd {
    Update { state: AppState, paused: bool },
    Shutdown,
}

/// betrayer hält seinen D-Bus-Thread selbst; dieser Thread hält das Icon und
/// hält damit `set_icon`/`set_menu` aus der Event-Loop heraus (§5).
pub struct TrayWorker {
    tx: Sender<TrayCmd>,
    join: Option<JoinHandle<()>>,
}

impl TrayWorker {
    pub fn spawn(
        model: String,
        state: AppState,
        paused: bool,
        out: Sender<Msg>,
        log: Arc<Logger>,
    ) -> Result<Self, TrayError> {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let join = thread::Builder::new()
            .name("diktier-tray".into())
            .spawn(move || tray_loop(rx, out, &model, state, paused, &ready_tx, &log))
            .map_err(|e| TrayError::Failed(format!("Tray-Thread: {e}")))?;
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => Ok(Self {
                tx,
                join: Some(join),
            }),
            Ok(Err(message)) => {
                join_with_timeout(join, Duration::from_secs(2));
                Err(TrayError::Failed(message))
            }
            Err(_) => {
                join_with_timeout(join, Duration::from_secs(2));
                Err(TrayError::Failed("Tray-Thread antwortet nicht".into()))
            }
        }
    }

    pub fn update(&self, state: AppState, paused: bool) {
        let _ = self.tx.send(TrayCmd::Update { state, paused });
    }

    pub fn shutdown(&mut self, timeout: Duration) -> bool {
        let _ = self.tx.send(TrayCmd::Shutdown);
        match self.join.take() {
            Some(join) => join_with_timeout(join, timeout),
            None => true,
        }
    }
}

fn tray_loop(
    rx: Receiver<TrayCmd>,
    out: Sender<Msg>,
    model: &str,
    state: AppState,
    paused: bool,
    ready: &Sender<Result<(), String>>,
    log: &Logger,
) {
    let mut runtime = Runtime {
        state,
        paused,
        ..Runtime::default()
    };
    let mut tray = match tray::new_backend(&runtime, model) {
        Ok(tray) => {
            let _ = ready.send(Ok(()));
            tray
        }
        Err(err) => {
            let _ = ready.send(Err(err.to_string()));
            return;
        }
    };
    log.info(format!("Tray-Backend: {}", tray.backend_name()));

    loop {
        let mut idle = true;
        match rx.try_recv() {
            Ok(TrayCmd::Update { state, paused }) => {
                idle = false;
                runtime.state = state;
                runtime.paused = paused;
                if let Err(err) = tray.update(&runtime, model) {
                    log.warn(format!("Tray-Update: {err}"));
                }
            }
            Ok(TrayCmd::Shutdown) | Err(TryRecvError::Disconnected) => break,
            Err(TryRecvError::Empty) => {}
        }
        match tray.poll() {
            Ok(Some(event)) => {
                idle = false;
                log.info(format!("Tray-Ereignis: {}", event.as_str()));
                let msg = match tray_event_to_core(event) {
                    Some(core) => Msg::Event(core),
                    None => Msg::OpenConfigDir,
                };
                if out.send(msg).is_err() {
                    break;
                }
            }
            Ok(None) => {}
            Err(err) => {
                let _ = out.send(Msg::TrayLost(err.to_string()));
                break;
            }
        }
        if idle {
            thread::sleep(Duration::from_millis(20));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §4.3: Menü und Linksklick landen auf den richtigen Kern-Events.
    #[test]
    fn tray_events_map_to_core_events() {
        assert_eq!(
            tray_event_to_core(TrayEvent::LeftClick),
            Some(Event::TrayClickToggle)
        );
        assert_eq!(
            tray_event_to_core(TrayEvent::TogglePause),
            Some(Event::PauseToggle)
        );
        assert_eq!(
            tray_event_to_core(TrayEvent::Quit),
            Some(Event::QuitRequested)
        );
        assert_eq!(
            tray_event_to_core(TrayEvent::OpenConfigDir),
            None,
            "Config-Ordner ist Wiring-Aktion, kein Kern-Event"
        );
    }

    #[test]
    fn hotkey_events_map_to_core_events() {
        assert_eq!(hotkey_event_to_core(HotkeyEvent::Press), Event::HotkeyPress);
        assert_eq!(
            hotkey_event_to_core(HotkeyEvent::Release),
            Event::HotkeyRelease
        );
    }

    /// §7.3: Die Inject-Schicht meldet Fokusverlust, der Kern kennt ihn als
    /// `CopyReason` — beide Gründe müssen erhalten bleiben.
    #[test]
    fn copy_only_reasons_survive_the_mapping() {
        assert_eq!(
            map_copy_reason(CopyOnlyReason::FocusChanged),
            CopyReason::FocusChanged
        );
        assert_eq!(
            map_copy_reason(CopyOnlyReason::FocusUnknown),
            CopyReason::FocusUnknown
        );
    }

    /// §7.3: Eine Antwort mit fremder Generation bekommt keine Fensterkennung —
    /// damit fällt sie in der Inject-Schicht auf `copy_only` zurück.
    #[test]
    fn context_of_a_foreign_run_has_no_window_ids() {
        let slot = ContextSlot {
            run: Some(RunId(7)),
            start: Some(WindowId(0x42)),
            target: Some(WindowId(0x42)),
            ended_at: Some(Instant::now()),
        };
        let ours = slot.context_for(RunId(7));
        assert_eq!(ours.start_window_id, Some(WindowId(0x42)));
        assert_eq!(ours.target_window_id, Some(WindowId(0x42)));

        let foreign = slot.context_for(RunId(8));
        assert_eq!(foreign.start_window_id, None);
        assert_eq!(foreign.target_window_id, None);
    }

    /// §5.2: Der Quit joint mit Frist — ein hängender Worker hält nicht auf.
    #[test]
    fn join_with_timeout_reports_finished_and_stuck_threads() {
        let quick = thread::spawn(|| thread::sleep(Duration::from_millis(10)));
        assert!(join_with_timeout(quick, Duration::from_secs(2)));

        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let stuck = thread::spawn(move || {
            while !flag.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(5));
            }
        });
        let t0 = Instant::now();
        assert!(!join_with_timeout(stuck, Duration::from_millis(80)));
        assert!(
            t0.elapsed() < Duration::from_secs(1),
            "der Join darf nicht über die Frist hinaus warten"
        );
        stop.store(true, Ordering::Release);
    }

    #[test]
    fn window_ids_are_logged_as_hex_or_unknown() {
        assert_eq!(window_str(Some(WindowId(0x6600325))), "0x6600325");
        assert_eq!(window_str(None), "unbekannt");
    }
}
