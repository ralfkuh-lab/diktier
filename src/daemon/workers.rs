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

use crate::audio::{AudioSource, CpalAudioSource, LevelTap};
use crate::config::{AudioConfig, OutputConfig};
use crate::download::{self, ArtifactManifest, DownloadError, HttpTransport, Progress};
use crate::engine::{ParakeetTranscriber, transcribe_pcm};
use crate::hotkey::{HotkeyBackend, HotkeyEvent, HotkeySpec, new_backend};
use crate::inject::{
    self, CaptureContext, ClipboardSave, Copied, CopyOnlyReason, InjectOutcome, OutputSink,
    RestoreDecision, TranscriptState, WindowId,
};
use crate::single_instance;
use crate::state::{
    AppState, CopyReason, ErrorInfo, ErrorKind, Event, InjectReport, Notice, RunId, Runtime,
};
use crate::tray::{self, TrayBackend, TrayError, TrayEvent};

#[cfg(windows)]
use super::OverlayView;
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
    /// codex M2: Ein Worker ist nicht mehr erreichbar (Spawn gescheitert,
    /// Thread weg, Kanal zu). Ohne diese Meldung bliebe der Daemon stumm in
    /// `loading`, `downloading`, `recording` oder `transcribing` stehen.
    WorkerFailed { what: WorkerKind, message: String },
    /// §10: Tray weg heißt kein GUI-Kanal mehr → Prozessende, Exit 1.
    TrayLost(String),
    /// §4.3-Menü „Config-Ordner öffnen" — kein Kern-Event.
    OpenConfigDir,
    /// §4.3-Menü „Hotkey ändern…" — kein Kern-Event, der Daemon öffnet den
    /// Dialog.
    ChangeHotkey,
    /// §9-Menüpunkt „Mit Windows starten" — kein Kern-Event, der Daemon legt
    /// den Autostart-Eintrag an bzw. entfernt ihn.
    ToggleAutostart,
    /// Der Hotkey-Dialog ist zu. `Ok(None)` = abgebrochen, `Ok(Some(spec))` =
    /// übernommen, `Err` = das Fenster kam gar nicht erst hoch.
    #[cfg(windows)]
    HotkeyChanged(Result<Option<HotkeySpec>, String>),
}

/// Welcher Worker ausgefallen ist — und in welche Fehlerklasse aus §10 das fällt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerKind {
    Engine,
    Audio,
    Inject,
}

impl WorkerKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Engine => "Engine-Worker",
            Self::Audio => "Audio-Worker",
            Self::Inject => "Inject-Worker",
        }
    }

    /// §10-Zuordnung: Mic, Engine und Inject bleiben bedienbar — der Retry ist
    /// der nächste Press. (Der Download meldet seine Fehler selbst als
    /// `DownloadFailed`, er hat keinen Kommandokanal, der brechen könnte.)
    pub fn to_fatal(self, message: String) -> Event {
        match self {
            Self::Engine => Event::FatalError {
                kind: ErrorKind::Engine,
                message,
            },
            Self::Audio => Event::FatalError {
                kind: ErrorKind::Mic,
                message,
            },
            Self::Inject => Event::FatalError {
                kind: ErrorKind::Inject,
                message,
            },
        }
    }
}

/// Kommando abschicken; ein toter Worker wird zur Meldung an die Event-Loop.
fn send_or_report<C>(tx: &Sender<C>, cmd: C, out: &Sender<Msg>, what: WorkerKind) {
    if tx.send(cmd).is_err() {
        let _ = out.send(Msg::WorkerFailed {
            what,
            message: "Worker-Thread ist nicht mehr erreichbar".into(),
        });
    }
}

/// §4.3: Tray-Ereignisse, die den Kern erreichen. „Config-Ordner öffnen" ist
/// reine Wiring-Aktion und hat deshalb kein Kern-Event.
///
/// **Offen für v2** (Owner-Entscheidung Phase 3d): Der Kern kennt
/// `Event::RetryRequested`, aber niemand erzeugt es — §4.3 legt das Tray-Menü
/// abschließend fest, und ein „Erneut versuchen"-Eintrag stünde nicht darin.
/// Der explizite Retry aus §6.3/§10 ist in v1 deshalb der Neustart des
/// Prozesses; ein Menüeintrag wäre eine Spec-Änderung.
pub fn tray_event_to_core(event: TrayEvent) -> Option<Event> {
    match event {
        TrayEvent::LeftClick => Some(Event::TrayClickToggle),
        TrayEvent::TogglePause => Some(Event::PauseToggle),
        TrayEvent::Quit => Some(Event::QuitRequested),
        TrayEvent::OpenConfigDir | TrayEvent::ChangeHotkey | TrayEvent::ToggleAutostart => None,
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

/// §4.5-Tabelle „Hinweiskarte": Welcher Restore-Ausgang des Paste-Pfads
/// (WP1) einen Hinweis zeigt. Kein Hinweis bei vollständigem Restore (auch mit
/// synthetisch ersetzten GDI- oder entfallenen OLE-Formaten), fremder
/// Änderung und `restore_clipboard = false`. `Wait`/`Restore` sind
/// Zwischenstände von `RestoreSession::decide` und stehen nie im Ausgang.
pub fn restore_notice(restore: RestoreDecision) -> Option<Notice> {
    match restore {
        RestoreDecision::NoPromise => Some(Notice::ClipboardNotSaved),
        RestoreDecision::RestoreFailed => Some(Notice::ClipboardNotRestored),
        RestoreDecision::RestoredPartial {
            lost_on_restore: false,
        } => Some(Notice::PartialSave),
        RestoreDecision::RestoredPartial {
            lost_on_restore: true,
        } => Some(Notice::PartialRestore),
        RestoreDecision::NoReadTimeout => Some(Notice::PasteUnconfirmed),
        RestoreDecision::Restored
        | RestoreDecision::ForeignOwner
        | RestoreDecision::Disabled
        | RestoreDecision::Wait
        | RestoreDecision::Restore => None,
    }
}

/// Frist für den `SAVE_TARGETS`-Handshake innerhalb des Inject-Threads
/// (Obergrenze ab Absenden; der Worker bekommt als absolute Deadline das
/// Antwort-Ende des Daemons abzüglich [`SAVE_TARGETS_MARGIN`]).
const SAVE_TARGETS_BUDGET: Duration = Duration::from_millis(1_500);

/// Reserve zwischen Worker-Deadline und `recv_timeout` des Daemons
/// (Nachkontrolle Blocker 2). `save_transcript_on_quit` beginnt nach der
/// Deadline keinen Versuch mehr, kann sie aber um **einen** laufenden
/// Versuch überziehen: Win32 `open_clipboard` = höchstens 10 × `OpenClipboard`
/// mit 9 × 10 ms Pump dazwischen, also rund 90–110 ms mit Timer-Auflösung,
/// plus `EmptyClipboard`/`SetClipboardData` und das Senden der Antwort. 300 ms
/// ist das Dreifache dieses schlechtesten Versuchs.
const SAVE_TARGETS_MARGIN: Duration = Duration::from_millis(300);

/// Final-Review Blocker 2: Abstand der Idle-Versuche für ein offenes eigenes
/// Versprechen.
pub const PROMISE_RETRY_INTERVAL: Duration = Duration::from_millis(500);

/// … und ihre Höchstzahl je Lauf; danach eine Warnung.
pub const PROMISE_RETRY_LIMIT: u32 = 10;

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
    out: Sender<Msg>,
    join: Option<JoinHandle<()>>,
}

impl EngineWorker {
    pub fn spawn(
        model: String,
        threads: u32,
        out: Sender<Msg>,
        log: Arc<Logger>,
    ) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel();
        let worker_out = out.clone();
        let join = thread::Builder::new()
            .name("diktier-engine".into())
            .spawn(move || engine_loop(rx, worker_out, &model, threads, &log))
            .map_err(|e| format!("Engine-Thread: {e}"))?;
        Ok(Self {
            tx,
            out,
            join: Some(join),
        })
    }

    pub fn load(&self, run: RunId) {
        send_or_report(
            &self.tx,
            EngineCmd::Load { run },
            &self.out,
            WorkerKind::Engine,
        );
    }

    pub fn transcribe(&self, run: RunId, samples: Vec<f32>) {
        send_or_report(
            &self.tx,
            EngineCmd::Transcribe { run, samples },
            &self.out,
            WorkerKind::Engine,
        );
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
                let (report, result) = transcribe_pcm(engine, &samples);
                // §6.4: der Gate-Report jeder Aufnahme ins Log — auch bei
                // Annahme und auch, wenn die Inferenz danach scheitert; er ist
                // die Datenbasis für die Nachkalibrierung (§10: nur Messwerte,
                // kein Audio, kein Text).
                log.run(run, format!("Gate: {report}"));
                match result {
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

// -------------------------------------------------------------- Download

/// §6.3: Der Download läuft auf einem eigenen Thread — 650 MB dürfen die
/// Event-Loop nicht anhalten, sonst wäre der Tray während des Ladens tot.
pub struct DownloadWorker {
    cancel: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl DownloadWorker {
    pub fn spawn(
        run: RunId,
        manifest: ArtifactManifest,
        out: Sender<Msg>,
        log: Arc<Logger>,
    ) -> Result<Self, String> {
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let join = thread::Builder::new()
            .name("diktier-download".into())
            .spawn(move || download_loop(run, &manifest, &out, &log, &flag))
            .map_err(|e| format!("Download-Thread: {e}"))?;
        Ok(Self {
            cancel,
            join: Some(join),
        })
    }

    /// Bricht zwischen zwei Blöcken ab (Quit-Pfad, §5.2).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    pub fn shutdown(&mut self, timeout: Duration) -> bool {
        self.cancel();
        match self.join.take() {
            Some(join) => join_with_timeout(join, timeout),
            None => true,
        }
    }
}

fn download_loop(
    run: RunId,
    manifest: &ArtifactManifest,
    out: &Sender<Msg>,
    log: &Logger,
    cancel: &AtomicBool,
) {
    let fail = |message: String| {
        log.error(&message);
        let _ = out.send(Msg::Event(Event::DownloadFailed { run, message }));
    };

    let dir = match download::model_dir(&manifest.key) {
        Ok(dir) => dir,
        Err(err) => return fail(err.to_string()),
    };
    let lock_path = match single_instance::download_lock_path() {
        Ok(path) => path,
        Err(err) => return fail(err.to_string()),
    };

    let total: u64 = manifest.files.iter().map(|f| f.bytes).sum();
    log.info(format!(
        "Modell wird geladen: {} Dateien, {} nach {}",
        manifest.files.len(),
        human_bytes(total),
        dir.display()
    ));

    let transport = HttpTransport::new();
    let t0 = Instant::now();
    let result = download::download_model_locked(
        &lock_path,
        &dir,
        manifest,
        &transport,
        cancel,
        &mut |progress| log_progress(log, progress),
    );

    match result {
        Ok(()) => {
            log.info(format!(
                "Modellartefakte vollständig und geprüft ({:.1} s)",
                t0.elapsed().as_secs_f64()
            ));
            let _ = out.send(Msg::Event(Event::DownloadFinished { run }));
        }
        // Beim Beenden ist der Abbruch gewollt: kein Fehlerzustand, keine
        // Fehlerzeile — der Quit-Pfad läuft ohnehin schon.
        Err(DownloadError::Cancelled) => log.info("Download abgebrochen (Beenden)"),
        Err(err) => fail(err.to_string()),
    }
}

/// §6.3: „Fortschritt als Logzeilen (keine UI)."
fn log_progress(log: &Logger, progress: Progress<'_>) {
    match progress {
        Progress::Skipped { name, index, total } => {
            log.info(format!("[{index}/{total}] {name}: bereits vorhanden"));
        }
        Progress::Started {
            name,
            index,
            total,
            bytes,
        } => log.info(format!(
            "[{index}/{total}] {name}: lade {} …",
            human_bytes(bytes)
        )),
        Progress::Bytes { name, done, bytes } => log.info(format!(
            "    {name}: {} / {} ({} %)",
            human_bytes(done),
            human_bytes(bytes),
            percent(done, bytes)
        )),
        Progress::Verified { name, index, total } => {
            log.info(format!(
                "[{index}/{total}] {name}: Größe und SHA-256 geprüft"
            ));
        }
    }
}

fn percent(done: u64, total: u64) -> u64 {
    if total == 0 {
        return 100;
    }
    (done.saturating_mul(100)) / total
}

fn human_bytes(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / MIB)
    } else {
        format!("{bytes} B")
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
    out: Sender<Msg>,
    join: Option<JoinHandle<()>>,
}

impl AudioWorker {
    /// `level`: geteilter Pegel fürs Aufnahme-Overlay (§4.5) oder `None`, wenn
    /// `[overlay] enabled = false` ist — dann rechnet der cpal-Callback ihn
    /// gar nicht erst aus.
    pub fn spawn(
        config: AudioConfig,
        level: Option<Arc<LevelTap>>,
        out: Sender<Msg>,
        log: Arc<Logger>,
    ) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel();
        let worker_out = out.clone();
        let join = thread::Builder::new()
            .name("diktier-audio".into())
            .spawn(move || audio_loop(rx, worker_out, &config, level, &log))
            .map_err(|e| format!("Audio-Thread: {e}"))?;
        Ok(Self {
            tx,
            out,
            join: Some(join),
        })
    }

    /// Idempotent: der Worker öffnet nur, wenn kein Stream bereitsteht.
    pub fn prepare(&self) {
        send_or_report(&self.tx, AudioCmd::Prepare, &self.out, WorkerKind::Audio);
    }

    /// Idempotent: der Worker gibt nur her, was offen ist.
    pub fn release(&self) {
        send_or_report(&self.tx, AudioCmd::Release, &self.out, WorkerKind::Audio);
    }

    pub fn start(&self, run: RunId) {
        send_or_report(
            &self.tx,
            AudioCmd::Start { run },
            &self.out,
            WorkerKind::Audio,
        );
    }

    pub fn stop(&self, run: RunId, discard: bool) {
        send_or_report(
            &self.tx,
            AudioCmd::Stop { run, discard },
            &self.out,
            WorkerKind::Audio,
        );
    }

    pub fn shutdown(&mut self, timeout: Duration) -> bool {
        let _ = self.tx.send(AudioCmd::Shutdown);
        match self.join.take() {
            Some(join) => join_with_timeout(join, timeout),
            None => true,
        }
    }
}

fn audio_loop(
    rx: Receiver<AudioCmd>,
    out: Sender<Msg>,
    config: &AudioConfig,
    level: Option<Arc<LevelTap>>,
    log: &Logger,
) {
    let mut source = CpalAudioSource::new(config, level);
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
                        dump_debug_wav(run, &captured.samples, log);
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

/// §10 `DIKTIER_DEBUG_WAV=1`: ein Dump je Aufnahme im Ring der letzten zehn,
/// genau eine Logzeile. Die Laufnummer im Dateinamen passt zu „Lauf N:“ im Log.
fn dump_debug_wav(run: RunId, samples: &[f32], log: &Logger) {
    if !debug_wav::enabled() {
        return;
    }
    match debug_wav::write_recording(
        &debug_wav::debug_dir(),
        samples,
        run,
        std::time::SystemTime::now(),
    ) {
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
    /// Quit-Pfad: Clipboard an den Clipboard-Manager übergeben. `Err` trägt
    /// den Grund, warum das Transkript nicht gesichert ist.
    SaveTargets {
        reply: Sender<Result<ClipboardSave, String>>,
        /// Absolute, monotone Frist für die Sicherung (Nachkontrolle
        /// Blocker 2). Liegt das Kommando hinter einem laufenden Paste in der
        /// Queue, kann sie beim Eintreffen schon verstrichen sein — dann gibt
        /// es genau einen letzten Versuch.
        deadline: Instant,
    },
    Shutdown,
}

/// Das Clipboard-Fenster lebt hier — inklusive des bis zu 5 s langen
/// Restore-Wartens aus §7.1 P7. Genau deshalb ist der Paste ein eigener Thread:
/// die Event-Loop bleibt reaktiv, `QuitRequested` greift jederzeit (codex H4).
pub struct InjectWorker {
    tx: Sender<InjectCmd>,
    out: Sender<Msg>,
    join: Option<JoinHandle<()>>,
}

impl InjectWorker {
    pub fn spawn(
        output: OutputConfig,
        out: Sender<Msg>,
        log: Arc<Logger>,
    ) -> Result<Self, inject::InjectError> {
        Self::spawn_with(move || inject::new_sink(output), out, log)
    }

    /// Wie [`Self::spawn`], mit austauschbarem Sink (Worker-Tests mit dem
    /// Fake). Der Sink entsteht **auf** dem Worker-Thread — das
    /// Clipboard-Fenster gehört dem Thread, der es pumpt.
    fn spawn_with<S, F>(
        make: F,
        out: Sender<Msg>,
        log: Arc<Logger>,
    ) -> Result<Self, inject::InjectError>
    where
        S: OutputSink,
        F: FnOnce() -> Result<S, inject::InjectError> + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let worker_out = out.clone();
        let join = thread::Builder::new()
            .name("diktier-inject".into())
            .spawn(move || inject_loop(rx, worker_out, make, &ready_tx, &log))
            .map_err(|e| inject::InjectError::Failed(format!("Inject-Thread: {e}")))?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self {
                tx,
                out,
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
        send_or_report(
            &self.tx,
            InjectCmd::MarkStart { run },
            &self.out,
            WorkerKind::Inject,
        );
    }

    pub fn mark_target(&self, run: RunId) {
        send_or_report(
            &self.tx,
            InjectCmd::MarkTarget { run },
            &self.out,
            WorkerKind::Inject,
        );
    }

    pub fn paste(&self, run: RunId, text: String) {
        send_or_report(
            &self.tx,
            InjectCmd::Paste { run, text },
            &self.out,
            WorkerKind::Inject,
        );
    }

    pub fn copy_only(&self, run: RunId, text: String, reason: CopyReason) {
        send_or_report(
            &self.tx,
            InjectCmd::CopyOnly { run, text, reason },
            &self.out,
            WorkerKind::Inject,
        );
    }

    /// Blockiert höchstens `timeout` — der Thread kann noch in einem Paste
    /// stehen (`Ok(Timeout)`). Ein nicht erreichbarer Worker ist ungeklärt
    /// (`Err`), kein `NotOwner`. Eine Uhr für den ganzen Pfad: Antwort-Ende
    /// und Worker-Deadline hängen am selben `Instant`.
    pub fn save_targets(&self, timeout: Duration) -> Result<ClipboardSave, String> {
        let (reply_tx, reply_rx) = mpsc::channel();
        let sent = Instant::now();
        let reply_by = sent + timeout;
        let deadline = reply_by
            .checked_sub(SAVE_TARGETS_MARGIN)
            .unwrap_or(sent)
            .max(sent)
            .min(sent + SAVE_TARGETS_BUDGET);
        if self
            .tx
            .send(InjectCmd::SaveTargets {
                reply: reply_tx,
                deadline,
            })
            .is_err()
        {
            return Err("Inject-Worker nicht erreichbar".into());
        }
        match reply_rx.recv_timeout(reply_by.saturating_duration_since(Instant::now())) {
            Ok(saved) => saved,
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(ClipboardSave::Timeout),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err("Inject-Worker ohne Antwort beendet".into())
            }
        }
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

fn inject_loop<S, F>(
    rx: Receiver<InjectCmd>,
    out: Sender<Msg>,
    make: F,
    ready: &Sender<Result<(), String>>,
    log: &Logger,
) where
    S: OutputSink,
    F: FnOnce() -> Result<S, inject::InjectError>,
{
    let mut sink = match make() {
        Ok(sink) => {
            let _ = ready.send(Ok(()));
            sink
        }
        Err(err) => {
            let _ = ready.send(Err(err.to_string()));
            return;
        }
    };
    // Final-Review, Hinweis Marker: z. B. die gescheiterte Registrierung des
    // Verlaufsausschlusses — ohne Konsole sonst unsichtbar.
    log_sink_warnings(&mut sink, log);
    let mut slot = ContextSlot::default();
    let mut retry = PromiseRetry::default();

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
                let result = sink.paste(&text, &ctx);
                let report = paste_report(run, text.len(), result, log);
                log_sink_warnings(&mut sink, log);
                // Der Lauf hat selbst schon materialisiert; der Idle-Retry
                // setzt frühestens ein Intervall später ein.
                retry.restart(Instant::now(), run);
                let _ = out.send(Msg::Event(Event::InjectFinished { run, report }));
            }
            Ok(InjectCmd::CopyOnly { run, text, reason }) => {
                let result = sink.copy_only(&text);
                let report = copy_only_report(run, text.len(), reason, result, log);
                log_sink_warnings(&mut sink, log);
                retry.restart(Instant::now(), run);
                let _ = out.send(Msg::Event(Event::InjectFinished { run, report }));
            }
            Ok(InjectCmd::SaveTargets { reply, deadline }) => {
                // Die Warnung dazu schreibt `Daemon::shutdown`, genau einmal.
                let saved = sink
                    .save_to_clipboard_manager(deadline)
                    .map_err(|err| err.to_string());
                log_sink_warnings(&mut sink, log);
                let _ = reply.send(saved);
            }
            Ok(InjectCmd::Shutdown) | Err(TryRecvError::Disconnected) => break,
            Err(TryRecvError::Empty) => {
                // §7.1 P8: solange Diktier Owner ist, muss die Selection
                // bedient werden — sonst hängt jedes fremde Paste.
                if let Err(err) = sink.serve_for(Duration::from_millis(10)) {
                    log.warn(format!("Clipboard-Bedienung: {err}"));
                }
                if let Some(report) = idle_promise_step(&mut sink, &mut retry, Instant::now()) {
                    log.warn(report.warning);
                    // Nachkontrolle Blocker 1: ein nachträglicher Verlust
                    // erreicht den Kern (Tray `error`, sofern er idle ist).
                    if let Some(event) = report.lost {
                        let _ = out.send(Msg::Event(event));
                    }
                }
                log_sink_warnings(&mut sink, log);
            }
        }
    }
}

fn log_sink_warnings<S: OutputSink + ?Sized>(sink: &mut S, log: &Logger) {
    for warning in sink.take_warnings() {
        log.warn(warning);
    }
}

/// Paste-Ausgang → Logzeilen und Kern-Report. `TranscriptState::Lost` wird
/// zum Inject-Fehler (Tray `error`, Final-Review Blocker 1).
fn paste_report(
    run: RunId,
    bytes: usize,
    result: Result<InjectOutcome, inject::InjectError>,
    log: &Logger,
) -> InjectReport {
    match result {
        Ok(InjectOutcome::Pasted {
            shortcut,
            reads,
            restore,
            clipboard,
            transcript,
            ..
        }) => {
            // Leitentscheidung 9: Metadaten, nie Inhalte (§10).
            log.run(run, inject::formats::snapshot_log_line(&clipboard.snapshot));
            log.run(
                run,
                paste_log_line(
                    shortcut.as_str(),
                    bytes,
                    reads,
                    &inject::restore_log(restore, &clipboard),
                    clipboard.history_excluded,
                ),
            );
            finish_with_transcript(
                run,
                &transcript,
                InjectReport::Pasted {
                    notice: restore_notice(restore),
                },
                log,
            )
        }
        Ok(InjectOutcome::CopyOnly {
            reason,
            history_excluded,
            snapshot,
            transcript,
        }) => {
            // Final-Review, Hinweis Snapshot: lief er schon, gehört seine
            // Zeile auch hierher (Messgrundlage, Leitentscheidung 3).
            if let Some(snapshot) = &snapshot {
                log.run(run, inject::formats::snapshot_log_line(snapshot));
            }
            log.run(
                run,
                with_history(format!("copy_only: {}", reason.as_str()), history_excluded),
            );
            finish_with_transcript(
                run,
                &transcript,
                InjectReport::CopyOnly {
                    reason: map_copy_reason(reason),
                },
                log,
            )
        }
        Err(err) => {
            log.error(format!("Einfügen: {err}"));
            InjectReport::Failed {
                message: err.to_string(),
            }
        }
    }
}

/// Tray-Click-Pfad (`copy_only`) → Logzeile und Kern-Report.
fn copy_only_report(
    run: RunId,
    bytes: usize,
    reason: CopyReason,
    result: Result<Copied, inject::InjectError>,
    log: &Logger,
) -> InjectReport {
    match result {
        Ok(Copied {
            history_excluded,
            transcript,
        }) => {
            log.run(
                run,
                with_history(format!("copy_only · {bytes} Bytes"), history_excluded),
            );
            finish_with_transcript(run, &transcript, InjectReport::CopyOnly { reason }, log)
        }
        Err(err) => {
            log.error(format!("Clipboard: {err}"));
            InjectReport::Failed {
                message: err.to_string(),
            }
        }
    }
}

/// `PromiseOpen` und `Lost` als Warnung ins Log; `Lost` ersetzt den Report
/// durch einen Inject-Fehler.
fn finish_with_transcript(
    run: RunId,
    transcript: &TranscriptState,
    ok: InjectReport,
    log: &Logger,
) -> InjectReport {
    if let Some(line) = transcript_warning(transcript) {
        log.warn(format!("Lauf {}: {line}", run.0));
    }
    transcript_report(transcript, ok)
}

/// Warnzeile zum Verbleib des Transkripts; `None` im Normalfall.
fn transcript_warning(transcript: &TranscriptState) -> Option<String> {
    match transcript {
        TranscriptState::Secured => None,
        TranscriptState::PromiseOpen(detail) => Some(format!(
            "Transkript noch nicht eager in der Zwischenablage — Versprechen offen, \
             neuer Versuch im Leerlauf ({detail})"
        )),
        TranscriptState::Lost(detail) => Some(TranscriptState::lost_message(detail)),
    }
}

fn transcript_report(transcript: &TranscriptState, ok: InjectReport) -> InjectReport {
    match transcript {
        TranscriptState::Lost(detail) => InjectReport::Failed {
            message: TranscriptState::lost_message(detail),
        },
        TranscriptState::Secured | TranscriptState::PromiseOpen(_) => ok,
    }
}

/// Idle-Retry für ein offenes eigenes Versprechen (Final-Review Blocker 2):
/// höchstens alle [`PROMISE_RETRY_INTERVAL`], höchstens
/// [`PROMISE_RETRY_LIMIT`] Versuche je Lauf, danach genau eine Warnung. Rein
/// über Zeitpunkte gesteuert, damit es ohne Uhr testbar ist.
#[derive(Debug, Clone, Default)]
struct PromiseRetry {
    attempts: u32,
    last: Option<Instant>,
    done: bool,
    /// Der Lauf, dessen Transkript das Versprechen trägt (Zuordnung für
    /// `Event::TranscriptLost`).
    run: Option<RunId>,
}

impl PromiseRetry {
    /// Nach jedem Paste/CopyOnly: neue Zählung, erster Versuch frühestens ein
    /// Intervall nach dem Lauf (der hat selbst schon materialisiert).
    fn restart(&mut self, now: Instant, run: RunId) {
        *self = Self {
            attempts: 0,
            last: Some(now),
            done: false,
            run: Some(run),
        };
    }

    /// Ist nach Zeitplan ein Versuch erlaubt? Ob ein Versprechen offen ist,
    /// fragt der Aufrufer erst danach (spart im 10-ms-Takt die Abfrage).
    fn due(&self, now: Instant) -> bool {
        !self.done
            && self.attempts < PROMISE_RETRY_LIMIT
            && self
                .last
                .is_none_or(|last| now.saturating_duration_since(last) >= PROMISE_RETRY_INTERVAL)
    }

    /// Ergebnis eines Versuchs. `Some(line)`: Warnung für den Logger.
    fn record(&mut self, now: Instant, state: &TranscriptState) -> Option<String> {
        self.last = Some(now);
        self.attempts += 1;
        match state {
            TranscriptState::Secured => {
                self.done = true;
                None
            }
            TranscriptState::PromiseOpen(detail) if self.attempts >= PROMISE_RETRY_LIMIT => {
                self.done = true;
                Some(format!(
                    "Transkript nach {PROMISE_RETRY_LIMIT} Versuchen nicht eager hinterlegt — \
                     Versprechen bleibt offen, Zwischenablage kann beim Beenden leer sein \
                     ({detail})"
                ))
            }
            TranscriptState::PromiseOpen(_) => None,
            TranscriptState::Lost(detail) => {
                self.done = true;
                Some(TranscriptState::lost_message(detail))
            }
        }
    }
}

/// Was ein Idle-Schritt zu melden hat.
#[derive(Debug, Clone, PartialEq, Eq)]
struct IdleReport {
    /// Warnung für den Logger.
    warning: String,
    /// Nachkontrolle Blocker 1: `Event::TranscriptLost` für den Kern, wenn
    /// das Transkript im Retry verloren ging.
    lost: Option<Event>,
}

/// Ein Idle-Schritt: nach Zeitplan und nur bei offenem **eigenem** Versprechen
/// (Owner und Sequenz prüft der Sink; ein fremder Copy heißt: nichts tun).
fn idle_promise_step<S: OutputSink + ?Sized>(
    sink: &mut S,
    retry: &mut PromiseRetry,
    now: Instant,
) -> Option<IdleReport> {
    if !retry.due(now) || !sink.pending_promise() {
        return None;
    }
    let state = sink.materialize_pending();
    let warning = retry.record(now, &state)?;
    let lost = match (&state, retry.run) {
        (TranscriptState::Lost(detail), Some(run)) => Some(Event::TranscriptLost {
            run,
            message: TranscriptState::lost_message(detail),
        }),
        _ => None,
    };
    Some(IdleReport { warning, lost })
}

/// Paste-Logzeile (Leitentscheidung 9, §10): Metadaten, nie Inhalte. Der
/// Verlaufsausschluss (Leitentscheidung 7) erscheint nur, wenn er **fehlt** —
/// der Normalfall bleibt kurz.
fn paste_log_line(
    shortcut: &str,
    bytes: usize,
    reads: u32,
    restore: &str,
    history_excluded: bool,
) -> String {
    with_history(
        format!("Paste {shortcut} · {bytes} Bytes · reads {reads} · restore {restore}"),
        history_excluded,
    )
}

/// Hängt `· Verlauf ausgeschlossen: nein` an, wenn der Ausschluss fehlt —
/// beim Paste wie bei `copy_only` (Final-Review, Hinweis Marker).
fn with_history(mut line: String, history_excluded: bool) -> String {
    if !history_excluded {
        line.push_str(" · Verlauf ausgeschlossen: nein");
    }
    line
}

/// Quit-Pfad: Das Transkript ist nicht gesichert (z. B. `OpenClipboard`
/// blockiert). Eindeutig statt `SAVE_TARGETS: … → Timeout`, denn der Inhalt
/// kann mit dem Prozess verschwinden.
fn quit_save_failed_line(err: &str) -> String {
    format!("Transkript beim Beenden nicht gesichert — Zwischenablage kann leer sein ({err})")
}

/// Logzeile für den Ausgang von [`InjectWorker::save_targets`] im Quit-Pfad
/// (Final-Review Blocker 2). `true`: Warnung. Jeder nicht gesicherte oder
/// ungeklärte Ausgang ist eine Warnung; `NotOwner` ohne offenes Versprechen
/// (und der Stub-Ausgang `NoManager`) ist normal.
pub fn quit_save_log(result: &Result<ClipboardSave, String>) -> (bool, String) {
    match result {
        Ok(ClipboardSave::Saved) => (false, "Clipboard beim Beenden gesichert".into()),
        Ok(save @ (ClipboardSave::NotOwner | ClipboardSave::NoManager)) => {
            (false, format!("Clipboard beim Beenden: {}", save.as_str()))
        }
        Ok(
            save
            @ (ClipboardSave::PromiseForeign | ClipboardSave::Refused | ClipboardSave::Timeout),
        ) => (true, quit_save_failed_line(save.as_str())),
        Err(err) => (true, quit_save_failed_line(err)),
    }
}

fn window_str(id: Option<WindowId>) -> String {
    match id {
        Some(id) => format!("0x{:x}", id.0),
        None => "unbekannt".into(),
    }
}

// --------------------------------------------------------------- Hotkey

/// §4.4: Was der Daemon dem Hotkey-Thread sagen kann.
pub enum HotkeyCmd {
    /// Pause aufgehoben — Taste wieder greifen.
    Grab,
    /// Pausiert — Taste freigeben, damit die fokussierte App sie bekommt.
    Ungrab,
    /// §4.4 + „Hotkey ändern…": andere Taste, **sofort** und ohne Neustart.
    /// Nur der Windows-Dialog erzeugt das.
    #[cfg(windows)]
    Rebind(HotkeySpec),
    Shutdown,
}

/// §5: eigener Thread, entprellte Press/Release-Events in den Kanal.
pub struct HotkeyWorker {
    tx: Sender<HotkeyCmd>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl HotkeyWorker {
    pub fn spawn(spec: HotkeySpec, out: Sender<Msg>, log: Arc<Logger>) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let (tx, rx) = mpsc::channel();
        let join = thread::Builder::new()
            .name("diktier-hotkey".into())
            .spawn(move || hotkey_loop(spec, &flag, &rx, &out, &log))
            .map_err(|e| format!("Hotkey-Thread: {e}"))?;
        Ok(Self {
            tx,
            stop,
            join: Some(join),
        })
    }

    /// §4.4: Grab an den Pausezustand angleichen. Idempotent.
    pub fn set_grabbed(&self, grabbed: bool) {
        let _ = self.tx.send(if grabbed {
            HotkeyCmd::Grab
        } else {
            HotkeyCmd::Ungrab
        });
    }

    /// §4.4: Neue Taste ab sofort greifen. Der Pausezustand bleibt, wie er
    /// ist — der Aufrufer gleicht ihn danach mit [`Self::set_grabbed`] ab.
    #[cfg(windows)]
    pub fn rebind(&self, spec: HotkeySpec) {
        let _ = self.tx.send(HotkeyCmd::Rebind(spec));
    }

    pub fn shutdown(&mut self, timeout: Duration) -> bool {
        self.stop.store(true, Ordering::Release);
        let _ = self.tx.send(HotkeyCmd::Shutdown);
        match self.join.take() {
            Some(join) => join_with_timeout(join, timeout),
            None => true,
        }
    }
}

fn hotkey_loop(
    mut spec: HotkeySpec,
    stop: &AtomicBool,
    cmd_rx: &Receiver<HotkeyCmd>,
    out: &Sender<Msg>,
    log: &Logger,
) {
    let mut backend = match new_backend(&spec) {
        Ok(backend) => backend,
        Err(err) => {
            // §4.4/§10: kein Hotkey heißt Fehlerzustand — der Tray-Click
            // bleibt der bedienbare Weg, und der Tooltip nennt den Konflikt.
            log.error(format!("Hotkey-Registrierung: {err}"));
            let _ = out.send(Msg::HotkeyUnavailable(err.to_string()));
            return;
        }
    };
    if let Err(err) = backend.register() {
        log.error(format!("Hotkey-Registrierung: {err}"));
        let _ = out.send(Msg::HotkeyUnavailable(format!(
            "{} nicht greifbar: {err}",
            spec.describe()
        )));
        return;
    }
    log.info(format!(
        "Hotkey-Backend: {} ({}, Push-to-Talk)",
        backend.backend_name(),
        spec.describe()
    ));

    // Was der Daemon zuletzt wollte — ein Rebind darf den Pausezustand nicht
    // umkehren (der frische Hook installiert sich beim Aufbau selbst).
    let mut grabbed = true;

    while !stop.load(Ordering::Acquire) {
        match cmd_rx.try_recv() {
            Ok(HotkeyCmd::Grab) => {
                grabbed = true;
                if let Err(err) = backend.register() {
                    // §4.4/§10: Beim Resume gilt derselbe Maßstab wie beim
                    // Start — ohne Grab ist der Hotkey tot. Nur zu warnen
                    // hinterließe eine State-Machine, die sich für „idle"
                    // hält, während keine Taste mehr greift (Sol-Review). Auf
                    // Windows ist das real: jeder Resume ruft erneut
                    // `SetWindowsHookExW`.
                    log.error(format!("Hotkey erneut greifen: {err}"));
                    let _ = out.send(Msg::HotkeyUnavailable(err.to_string()));
                    return;
                }
                if backend.is_registered() {
                    log.info(format!("Hotkey {} wieder scharf", spec.describe()));
                }
            }
            Ok(HotkeyCmd::Ungrab) => {
                grabbed = false;
                if let Err(err) = backend.unregister() {
                    log.warn(format!("Hotkey freigeben: {err}"));
                } else {
                    log.info(format!("Hotkey {} freigegeben (pausiert)", spec.describe()));
                }
            }
            // Erst das neue Backend bauen, dann das alte hergeben: scheitert
            // der Aufbau, greift weiter die **alte** Taste, statt gar keine.
            #[cfg(windows)]
            Ok(HotkeyCmd::Rebind(next)) => match new_backend(&next) {
                Ok(mut fresh) => {
                    let ok = if grabbed {
                        fresh.register()
                    } else {
                        fresh.unregister()
                    };
                    match ok {
                        Ok(()) => {
                            let _ = backend.unregister();
                            backend = fresh;
                            spec = next;
                            log.info(format!(
                                "Hotkey jetzt: {} ({})",
                                spec.describe(),
                                if grabbed { "scharf" } else { "pausiert" }
                            ));
                        }
                        Err(err) => {
                            log.error(format!("Hotkey {}: {err}", next.describe()));
                            let _ = out.send(Msg::HotkeyUnavailable(err.to_string()));
                        }
                    }
                }
                Err(err) => {
                    log.error(format!("Hotkey {}: {err}", next.describe()));
                    let _ = out.send(Msg::HotkeyUnavailable(err.to_string()));
                }
            },
            Ok(HotkeyCmd::Shutdown) | Err(TryRecvError::Disconnected) => break,
            Err(TryRecvError::Empty) => {}
        }
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
    // Beim Beenden die Taste zurückgeben, bevor die Verbindung fällt.
    let _ = backend.unregister();
}

// ----------------------------------------------------------------- Tray

pub enum TrayCmd {
    /// §4.3/§4.4: Zustand **und** Fehlergrund — der Tooltip muss den Konflikt
    /// nennen können (codex M1).
    Update {
        state: AppState,
        paused: bool,
        error: Option<ErrorInfo>,
    },
    Shutdown,
}

/// Dieser Thread hält das Icon samt seiner Nachrichtenschleife und hält damit
/// `set_icon`/`set_menu` aus der Event-Loop heraus (§5).
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

    pub fn update(&self, state: AppState, paused: bool, error: Option<ErrorInfo>) {
        let _ = self.tx.send(TrayCmd::Update {
            state,
            paused,
            error,
        });
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
            Ok(TrayCmd::Update {
                state,
                paused,
                error,
            }) => {
                idle = false;
                runtime.state = state;
                runtime.paused = paused;
                // §4.4: Der Fehlergrund gehört in den Tooltip, nicht nur ins Log.
                runtime.error = error;
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
                    None if event == TrayEvent::ChangeHotkey => Msg::ChangeHotkey,
                    None if event == TrayEvent::ToggleAutostart => Msg::ToggleAutostart,
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

// -------------------------------------------------------------- Overlay

/// §4.5: Was der Daemon dem Overlay-Thread sagen kann. Mehr braucht es nicht —
/// der Pegel kommt am Kanal vorbei über den geteilten `LevelTap`.
#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayCmd {
    /// Die Ansicht, die der Kernzustand gerade verlangt (`overlay_view`).
    View(OverlayView),
    Shutdown,
}

/// Takt der Overlay-Schleife. Sichtbar rendert jeder Durchlauf (≈50 Frames/s),
/// unsichtbar schläft sie nur.
#[cfg(windows)]
const OVERLAY_TICK: Duration = Duration::from_millis(20);

/// Das Overlay-Fenster lebt komplett auf diesem Thread — wie das Tray-Icon auf
/// seinem (Phase-5-Leitentscheidung 2). Fremde Threads schicken Kommandos,
/// niemand sonst fasst das `HWND` an.
#[cfg(windows)]
pub struct OverlayWorker {
    tx: Sender<OverlayCmd>,
    join: Option<JoinHandle<()>>,
}

#[cfg(windows)]
impl OverlayWorker {
    /// Ready-Handshake mit 10-s-Frist wie beim [`TrayWorker`]. Ein Fehler ist
    /// **nie** fatal (SPEC §4.5): Der Aufrufer loggt eine Warnung und
    /// diktiert ohne Overlay weiter.
    pub fn spawn(level: Arc<LevelTap>, log: Arc<Logger>) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let join = thread::Builder::new()
            .name("diktier-overlay".into())
            .spawn(move || overlay_loop(rx, level, &ready_tx, &log))
            .map_err(|e| format!("Overlay-Thread: {e}"))?;
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => Ok(Self {
                tx,
                join: Some(join),
            }),
            Ok(Err(message)) => {
                join_with_timeout(join, Duration::from_secs(2));
                Err(message)
            }
            Err(_) => {
                join_with_timeout(join, Duration::from_secs(2));
                Err("Overlay-Thread antwortet nicht".into())
            }
        }
    }

    /// Idempotent; der Worker koalesziert ohnehin auf die letzte Ansicht.
    pub fn set_view(&self, view: OverlayView) {
        let _ = self.tx.send(OverlayCmd::View(view));
    }

    pub fn shutdown(&mut self, timeout: Duration) -> bool {
        let _ = self.tx.send(OverlayCmd::Shutdown);
        match self.join.take() {
            Some(join) => join_with_timeout(join, timeout),
            None => true,
        }
    }
}

/// Was in einer Runde an Kommandos anlag.
#[cfg(windows)]
#[derive(Debug, Default, PartialEq, Eq)]
pub struct OverlayRound {
    /// Die **letzte** Ansicht der Runde — eine schnelle Folge
    /// `Level → Hidden → Level` zeigt nie ein veraltetes Fenster (Sol Major 7).
    pub view: Option<OverlayView>,
    /// `Shutdown` hat Vorrang: Was danach kommt, wird nicht mehr ausgeführt.
    pub shutdown: bool,
}

/// Kommandos einer Runde **vollständig** drainen und auf die letzte Ansicht
/// reduzieren.
#[cfg(windows)]
fn drain_overlay_commands(rx: &Receiver<OverlayCmd>) -> OverlayRound {
    let mut round = OverlayRound::default();
    loop {
        match rx.try_recv() {
            Ok(OverlayCmd::View(view)) => round.view = Some(view),
            // Der abgerissene Kanal heißt „Daemon ist weg" — dasselbe wie
            // `Shutdown`.
            Ok(OverlayCmd::Shutdown) | Err(TryRecvError::Disconnected) => {
                round.shutdown = true;
                round.view = None;
                return round;
            }
            Err(TryRecvError::Empty) => return round,
        }
    }
}

/// Was der Worker vom Fenster braucht — als Trait, damit die Übergänge
/// zwischen den Ansichten ohne echtes Fenster testbar sind.
#[cfg(windows)]
trait OverlaySurface {
    fn show_level(&mut self) -> Result<(), String>;
    fn show_notice(&mut self, title: &'static str, detail: &'static str) -> Result<(), String>;
    fn hide(&mut self);
}

#[cfg(windows)]
impl OverlaySurface for crate::overlay::OverlayWindow {
    fn show_level(&mut self) -> Result<(), String> {
        crate::overlay::OverlayWindow::show_level(self).map_err(|e| e.to_string())
    }

    fn show_notice(&mut self, title: &'static str, detail: &'static str) -> Result<(), String> {
        crate::overlay::OverlayWindow::show_notice(self, title, detail).map_err(|e| e.to_string())
    }

    fn hide(&mut self) {
        crate::overlay::OverlayWindow::hide(self);
    }
}

/// Eine neue Ansicht auf das Fenster bringen. `Level ↔ Notice` tauscht nur den
/// Inhalt — **kein** `hide()` dazwischen (§4.5: ohne Ausblenden, ohne
/// Aktivierung). Ausgeblendet wird ausschließlich für `Hidden`.
#[cfg(windows)]
fn apply_overlay_view(
    surface: &mut impl OverlaySurface,
    current: OverlayView,
    next: OverlayView,
) -> Result<(), String> {
    if current == next {
        return Ok(());
    }
    match next {
        OverlayView::Hidden => {
            surface.hide();
            Ok(())
        }
        OverlayView::Level => surface.show_level(),
        OverlayView::Notice(notice) => surface.show_notice(notice.title(), notice.detail()),
    }
}

#[cfg(windows)]
fn overlay_loop(
    rx: Receiver<OverlayCmd>,
    level: Arc<LevelTap>,
    ready: &Sender<Result<(), String>>,
    log: &Logger,
) {
    use crate::overlay::OverlayWindow;

    let mut window = match OverlayWindow::new() {
        Ok(window) => {
            let _ = ready.send(Ok(()));
            window
        }
        Err(err) => {
            // Ohne Fenster gibt es keinen Consumer mehr — der Audio-Callback
            // soll den Pegel gar nicht erst ausrechnen (Sol-Impl-Review
            // Major 4). Der Daemon verwirft den Tap zusätzlich, weil der
            // Ready-Handshake fehlschlägt; beides zusammen deckt auch den
            // Fall ab, dass er ihn doch schon weitergereicht hätte.
            level.deactivate();
            let _ = ready.send(Err(err.to_string()));
            return;
        }
    };
    log.info("Overlay bereit (per-Monitor-DPI v2)");

    let mut current = OverlayView::Hidden;
    loop {
        let round = drain_overlay_commands(&rx);
        if round.shutdown {
            break;
        }
        if let Some(next) = round.view
            && next != current
        {
            let was_visible = window.is_visible();
            if let Err(err) = apply_overlay_view(&mut window, current, next) {
                // §4.5: nie fatal — Warnung, Overlay aus, Diktieren läuft.
                log.warn(format!("Overlay nicht anzeigbar: {err}"));
                break;
            }
            current = next;
            if !was_visible && window.is_visible() {
                log.info(format!("Overlay sichtbar: {}", window.describe()));
            }
        }
        window.pump();
        if window.is_visible()
            && let Err(err) = window.frame(level.take())
        {
            log.warn(format!("Overlay-Frame: {err}"));
            break;
        }
        // §4.5 Fallback: Scheitert der Text, steht nur die Warn-Glyphe — das
        // Overlay bleibt aktiv, die Ursache gehört ins Log.
        if let Some(warning) = window.take_text_warning() {
            log.warn(format!(
                "Hinweistext nicht darstellbar, nur Warn-Glyphe: {warning}"
            ));
        }
        thread::sleep(OVERLAY_TICK);
    }
    // Egal ob regulärer Shutdown oder dauerhafter Fehler: Ab hier gibt es
    // keinen Consumer mehr, und der Audio-Callback hört auf zu rechnen
    // (Sol-Impl-Review Major 4).
    level.deactivate();
    // Der `Drop` räumt Fenster, Klasse und DIB ab — auf dem Owner-Thread.
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
        assert_eq!(
            tray_event_to_core(TrayEvent::ToggleAutostart),
            None,
            "Autostart ist Wiring-Aktion, kein Kern-Event"
        );
    }

    #[test]
    fn the_paste_line_names_a_missing_history_exclusion_only() {
        assert_eq!(
            paste_log_line("ctrl+v", 12, 1, "true (restored)", true),
            "Paste ctrl+v · 12 Bytes · reads 1 · restore true (restored)"
        );
        assert_eq!(
            paste_log_line("ctrl+v", 12, 1, "true (restored)", false),
            "Paste ctrl+v · 12 Bytes · reads 1 · restore true (restored) · Verlauf ausgeschlossen: nein"
        );
    }

    #[test]
    fn a_failed_quit_save_is_an_unambiguous_warning() {
        let line =
            quit_save_failed_line("Ausgabe fehlgeschlagen: OpenClipboard: Zugriff verweigert");
        assert!(
            line.starts_with(
                "Transkript beim Beenden nicht gesichert — Zwischenablage kann leer sein"
            ),
            "{line}"
        );
        assert!(line.contains("OpenClipboard"), "{line}");
        assert!(!line.contains("SAVE_TARGETS"), "{line}");
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

    /// codex M2: Jeder Worker-Ausfall landet in seiner §10-Fehlerklasse — statt
    /// den Daemon stumm in `loading`/`recording`/`transcribing` stehen zu lassen.
    #[test]
    fn worker_failures_map_to_their_error_class() {
        let cases = [
            (WorkerKind::Engine, ErrorKind::Engine),
            (WorkerKind::Audio, ErrorKind::Mic),
            (WorkerKind::Inject, ErrorKind::Inject),
        ];
        for (what, expected) in cases {
            match what.to_fatal("Thread weg".into()) {
                Event::FatalError { kind, message } => {
                    assert_eq!(kind, expected, "{}", what.label());
                    assert_eq!(message, "Thread weg");
                }
                other => panic!("erwartet FatalError, bekam {other:?}"),
            }
            assert!(!what.label().is_empty());
        }
    }

    /// Ein toter Kommandokanal meldet sich bei der Event-Loop, statt still zu
    /// verpuffen (codex M2).
    #[test]
    fn a_dead_command_channel_reports_to_the_loop() {
        let (out_tx, out_rx) = mpsc::channel::<Msg>();
        let (cmd_tx, cmd_rx) = mpsc::channel::<u8>();
        drop(cmd_rx); // Worker-Thread ist weg.

        send_or_report(&cmd_tx, 1_u8, &out_tx, WorkerKind::Engine);
        match out_rx.try_recv() {
            Ok(Msg::WorkerFailed { what, message }) => {
                assert_eq!(what, WorkerKind::Engine);
                assert!(message.contains("nicht mehr erreichbar"), "{message}");
            }
            other => panic!(
                "erwartet WorkerFailed, bekam etwas anderes: {}",
                other.is_ok()
            ),
        }
    }

    /// §4.5-Tabelle: Jeder Restore-Ausgang aus WP1 bekommt seinen Hinweis —
    /// oder ausdrücklich keinen.
    #[test]
    fn restore_outcomes_map_to_their_notice() {
        let cases = [
            (RestoreDecision::NoPromise, Some(Notice::ClipboardNotSaved)),
            (
                RestoreDecision::RestoreFailed,
                Some(Notice::ClipboardNotRestored),
            ),
            (
                RestoreDecision::RestoredPartial {
                    lost_on_restore: false,
                },
                Some(Notice::PartialSave),
            ),
            (
                RestoreDecision::RestoredPartial {
                    lost_on_restore: true,
                },
                Some(Notice::PartialRestore),
            ),
            (
                RestoreDecision::NoReadTimeout,
                Some(Notice::PasteUnconfirmed),
            ),
            // Vollständig (auch mit ersetzten/entfallenen Formaten), fremde
            // Änderung, abgeschaltet: kein Hinweis.
            (RestoreDecision::Restored, None),
            (RestoreDecision::ForeignOwner, None),
            (RestoreDecision::Disabled, None),
            // Zwischenstände, die nie im Ausgang stehen.
            (RestoreDecision::Wait, None),
            (RestoreDecision::Restore, None),
        ];
        for (decision, expected) in cases {
            assert_eq!(restore_notice(decision), expected, "{decision:?}");
        }
    }

    #[cfg(windows)]
    fn view(view: OverlayView) -> OverlayCmd {
        OverlayCmd::View(view)
    }

    /// Sol Major 7: Pro Runde werden alle Overlay-Kommandos gedraint und auf
    /// die **letzte** Ansicht reduziert. Sonst zeigte ein schneller
    /// Tray-Toggle (`Level → Hidden → Level`) die Karte verzögert oder gar nach
    /// dem Ende der Aufnahme — und ein veralteter Hinweis bliebe stehen.
    #[cfg(windows)]
    #[test]
    fn overlay_commands_coalesce_and_shutdown_wins() {
        let (tx, rx) = mpsc::channel();
        assert_eq!(
            drain_overlay_commands(&rx),
            OverlayRound {
                view: None,
                shutdown: false
            },
            "leere Runde ändert nichts"
        );

        for cmd in [
            view(OverlayView::Level),
            view(OverlayView::Hidden),
            view(OverlayView::Level),
        ] {
            tx.send(cmd).unwrap();
        }
        assert_eq!(
            drain_overlay_commands(&rx),
            OverlayRound {
                view: Some(OverlayView::Level),
                shutdown: false
            }
        );

        // Auch der Hinweis koalesziert: nur die letzte Ansicht zählt.
        for cmd in [
            view(OverlayView::Level),
            view(OverlayView::Notice(Notice::PasteUnconfirmed)),
            view(OverlayView::Hidden),
            view(OverlayView::Notice(Notice::FocusChanged)),
        ] {
            tx.send(cmd).unwrap();
        }
        assert_eq!(
            drain_overlay_commands(&rx).view,
            Some(OverlayView::Notice(Notice::FocusChanged))
        );

        // `Shutdown` hat Vorrang — auch über eine Ansicht, die noch dahinter
        // liegt.
        tx.send(view(OverlayView::Hidden)).unwrap();
        tx.send(OverlayCmd::Shutdown).unwrap();
        tx.send(view(OverlayView::Notice(Notice::TrayCopy)))
            .unwrap();
        let round = drain_overlay_commands(&rx);
        assert!(round.shutdown);
        assert_eq!(round.view, None, "nach dem Shutdown wird nichts gezeigt");

        // Ein abgerissener Kanal (Daemon weg) ist dasselbe wie `Shutdown`.
        let (tx, rx) = mpsc::channel::<OverlayCmd>();
        tx.send(view(OverlayView::Level)).unwrap();
        drop(tx);
        assert!(drain_overlay_commands(&rx).shutdown);
    }

    /// Zeichnet nur auf, was der Worker vom Fenster verlangt.
    #[cfg(windows)]
    #[derive(Debug, Default)]
    struct RecordingSurface {
        calls: Vec<String>,
        fail_level: bool,
    }

    #[cfg(windows)]
    impl OverlaySurface for RecordingSurface {
        fn show_level(&mut self) -> Result<(), String> {
            self.calls.push("level".into());
            if self.fail_level {
                return Err("DIB weg".into());
            }
            Ok(())
        }

        fn show_notice(&mut self, title: &'static str, detail: &'static str) -> Result<(), String> {
            self.calls.push(format!("notice: {title} / {detail}"));
            Ok(())
        }

        fn hide(&mut self) {
            self.calls.push("hide".into());
        }
    }

    /// §4.5: `Level → Notice → Level` tauscht nur den Inhalt — kein `hide()`
    /// dazwischen. Ausgeblendet wird nur für `Hidden`, und eine unveränderte
    /// Ansicht fasst das Fenster gar nicht an.
    #[cfg(windows)]
    #[test]
    fn level_notice_level_switches_content_without_hiding() {
        let mut surface = RecordingSurface::default();
        let sequence = [
            OverlayView::Level,
            OverlayView::Notice(Notice::PartialSave),
            OverlayView::Level,
            OverlayView::Level,
            OverlayView::Notice(Notice::TrayCopy),
            OverlayView::Hidden,
            OverlayView::Notice(Notice::FocusChanged),
        ];
        let mut current = OverlayView::Hidden;
        for next in sequence {
            apply_overlay_view(&mut surface, current, next).unwrap();
            current = next;
        }
        assert_eq!(
            surface.calls,
            vec![
                "level".to_string(),
                "notice: Zwischenablage teilweise wiederhergestellt / Nicht alle Formate \
                 ließen sich sichern"
                    .to_string(),
                "level".to_string(),
                "notice: Text liegt in der Zwischenablage / Mit Strg+V einfügen".to_string(),
                "hide".to_string(),
                "notice: Fokus gewechselt – nicht eingefügt / Text liegt in der \
                 Zwischenablage"
                    .to_string(),
            ]
        );

        // Ein Fensterfehler kommt beim Worker an (der schaltet das Overlay ab).
        let mut broken = RecordingSurface {
            fail_level: true,
            ..RecordingSurface::default()
        };
        assert!(apply_overlay_view(&mut broken, OverlayView::Hidden, OverlayView::Level).is_err());
    }

    /// Ein lebender Kanal meldet nichts — sonst hätte jeder normale Befehl
    /// einen Fehlerzustand ausgelöst.
    #[test]
    fn a_live_command_channel_stays_quiet() {
        let (out_tx, out_rx) = mpsc::channel::<Msg>();
        let (cmd_tx, cmd_rx) = mpsc::channel::<u8>();
        send_or_report(&cmd_tx, 7_u8, &out_tx, WorkerKind::Audio);
        assert_eq!(cmd_rx.try_recv().unwrap(), 7);
        assert!(out_rx.try_recv().is_err(), "keine Fehlermeldung");
    }

    // ----------------------------- Final-Review (Sol) Nacharbeit

    mod promise {
        use super::*;
        use crate::inject::fake::{FakeContent, FakeFormat, FakeHost, MaterializeFault};

        fn ctx() -> CaptureContext {
            CaptureContext {
                start_window_id: Some(WindowId(1)),
                target_window_id: Some(WindowId(1)),
                ended_at: Instant::now(),
            }
        }

        /// Ein Lauf ohne Read, dessen Materialisierung `blocked`-mal
        /// scheitert: danach liegt ein offenes eigenes Versprechen.
        fn open_promise(blocked: u32) -> FakeHost {
            let mut host = FakeHost::new()
                .with_formats(vec![FakeFormat::text("vorher")])
                .with_materialize_fault(MaterializeFault::Blocked, blocked);
            let outcome = host.paste("transkript", &ctx()).unwrap();
            assert!(
                matches!(
                    &outcome,
                    InjectOutcome::Pasted {
                        transcript: TranscriptState::PromiseOpen(_),
                        ..
                    }
                ),
                "{outcome:?}"
            );
            assert!(host.pending_promise());
            host
        }

        fn step(t0: Instant, ms: u64) -> Instant {
            t0 + Duration::from_millis(ms)
        }

        /// Zeitplan als reine Funktion: nicht vor 500 ms nach dem Lauf, dann
        /// höchstens alle 500 ms, höchstens zehn Versuche, dann genau eine
        /// Warnung.
        #[test]
        fn the_retry_schedule_is_bounded() {
            let t0 = Instant::now();
            let mut retry = PromiseRetry::default();
            assert!(retry.due(t0), "ohne Lauf: sofort erlaubt");
            retry.restart(t0, RunId(7));
            assert!(!retry.due(step(t0, 499)));
            assert!(retry.due(step(t0, 500)));

            let open = TranscriptState::PromiseOpen("blockiert".into());
            let mut now = step(t0, 500);
            for attempt in 1..PROMISE_RETRY_LIMIT {
                assert!(retry.due(now), "Versuch {attempt}");
                assert_eq!(retry.record(now, &open), None);
                assert!(!retry.due(now + Duration::from_millis(499)));
                now += PROMISE_RETRY_INTERVAL;
            }
            let warning = retry.record(now, &open).expect("Warnung nach dem zehnten");
            assert!(warning.contains("nach 10 Versuchen"), "{warning}");
            assert!(warning.contains("blockiert"), "{warning}");
            assert!(!retry.due(now + Duration::from_secs(3600)), "danach Ruhe");

            // Neuer Lauf: neue Zählung.
            retry.restart(now, RunId(8));
            assert!(retry.due(now + PROMISE_RETRY_INTERVAL));

            // Erfolg und Verlust beenden die Versuche.
            let mut retry = PromiseRetry::default();
            assert_eq!(retry.record(t0, &TranscriptState::Secured), None);
            assert!(!retry.due(step(t0, 10_000)));
            let mut retry = PromiseRetry::default();
            let lost = retry
                .record(t0, &TranscriptState::Lost("SetClipboardData".into()))
                .unwrap();
            assert!(lost.starts_with(TranscriptState::LOST), "{lost}");
            assert!(!retry.due(step(t0, 10_000)));
        }

        /// Idle-Retry, Erfolg: nach zwei weiteren Blockaden sichert der
        /// dritte Idle-Versuch; danach nichts mehr.
        #[test]
        fn idle_retry_secures_the_promise() {
            let mut host = open_promise(3);
            let t0 = Instant::now();
            let mut retry = PromiseRetry::default();
            retry.restart(t0, RunId(7));
            assert_eq!(idle_promise_step(&mut host, &mut retry, step(t0, 10)), None);
            assert_eq!(host.materialize_attempts, 1, "nicht vor 500 ms");
            for ms in [500, 1_000, 1_500] {
                assert_eq!(idle_promise_step(&mut host, &mut retry, step(t0, ms)), None);
            }
            assert_eq!(host.materialize_attempts, 4);
            assert_eq!(host.materializations, 1);
            assert!(!host.pending_promise());
            // `OutputConfig::default()` setzt das führende Leerzeichen.
            assert_eq!(host.clipboard_text().as_deref(), Some(" transkript"));
            assert_eq!(
                idle_promise_step(&mut host, &mut retry, step(t0, 2_000)),
                None
            );
            assert_eq!(host.materialize_attempts, 4);
        }

        /// Idle-Retry, fremder Copy: kein offenes eigenes Versprechen mehr —
        /// nichts tun, der fremde Inhalt bleibt.
        #[test]
        fn idle_retry_leaves_a_foreign_copy_alone() {
            let mut host = open_promise(u32::MAX);
            host.foreign_copy(FakeContent::Text("fremd".into()));
            let t0 = Instant::now();
            let mut retry = PromiseRetry::default();
            for ms in [0, 500, 1_000] {
                assert_eq!(idle_promise_step(&mut host, &mut retry, step(t0, ms)), None);
            }
            assert_eq!(host.materialize_attempts, 1, "nur der Versuch im Lauf");
            assert_eq!(host.clipboard_text().as_deref(), Some("fremd"));
        }

        /// Idle-Retry, zehn Fehlschläge: genau eine Warnung, danach Ruhe; das
        /// Versprechen bleibt für den Quit-Pfad.
        #[test]
        fn idle_retry_warns_once_after_ten_failures() {
            let mut host = open_promise(u32::MAX);
            let t0 = Instant::now();
            let mut retry = PromiseRetry::default();
            retry.restart(t0, RunId(7));
            let mut warnings = Vec::new();
            for i in 1..=30_u64 {
                if let Some(report) = idle_promise_step(&mut host, &mut retry, step(t0, 500 * i)) {
                    assert_eq!(report.lost, None, "offen ist nicht verloren");
                    warnings.push(report.warning);
                }
            }
            assert_eq!(host.materialize_attempts, 1 + PROMISE_RETRY_LIMIT);
            assert_eq!(warnings.len(), 1, "{warnings:?}");
            assert!(
                warnings[0].contains("Zwischenablage kann beim Beenden leer sein"),
                "{warnings:?}"
            );
            assert!(host.pending_promise());
        }

        /// Nachkontrolle Blocker 1: Im Lauf blockiert, beim ersten
        /// Idle-Retry scheitern Eager-Set **und** Rückfall-Versprechen → der
        /// Kern bekommt `TranscriptLost` für den ursprünglichen Lauf.
        #[test]
        fn idle_retry_loss_becomes_a_core_event() {
            let mut host =
                open_promise(1).with_materialize_fault(MaterializeFault::SetAndPromiseFail, 1);
            let t0 = Instant::now();
            let mut retry = PromiseRetry::default();
            retry.restart(t0, RunId(42));
            let report =
                idle_promise_step(&mut host, &mut retry, step(t0, 500)).expect("Verlust gemeldet");
            assert!(
                report.warning.starts_with(TranscriptState::LOST),
                "{report:?}"
            );
            match report.lost {
                Some(Event::TranscriptLost { run, message }) => {
                    assert_eq!(run, RunId(42));
                    assert!(
                        message.starts_with("Zwischenablage leer — Transkript verloren"),
                        "{message}"
                    );
                }
                other => panic!("{other:?}"),
            }
            // Einmal gemeldet, danach Ruhe.
            assert_eq!(
                idle_promise_step(&mut host, &mut retry, step(t0, 1_000)),
                None
            );
        }

        /// Ein Worker mit Fake-Sink. `real_pump(10)`: das 5-s-Read-Fenster
        /// dauert real rund 0,5 s.
        fn worker(host: FakeHost) -> (InjectWorker, Receiver<Msg>) {
            let (out_tx, out_rx) = mpsc::channel();
            let worker =
                InjectWorker::spawn_with(move || Ok(host), out_tx, Arc::new(Logger::new(false)))
                    .expect("Worker");
            (worker, out_rx)
        }

        fn start_paste(worker: &InjectWorker) {
            worker.mark_start(RunId(1));
            worker.mark_target(RunId(1));
            worker.paste(RunId(1), "transkript".into());
        }

        /// Nachkontrolle Blocker 2: Quit, während ein Paste im 5-s-Fenster
        /// ohne Read steht. `SaveTargets` liegt hinter dem Paste in der Queue
        /// und wird erst danach bearbeitet (erklärtes Verhalten: der einzige
        /// Inject-Worker ist belegt, siehe README).
        ///
        /// - Reicht die Frist, sichert der Paste selbst (`NoReadTimeout` →
        ///   Materialisierung) und Quit meldet `Saved`.
        /// - Reicht sie nicht, meldet `save_targets` `Timeout`, und die
        ///   Quit-Zeile ist die eindeutige Warnung.
        /// - Bleibt das Clipboard blockiert, antwortet der Worker mit `Err`
        ///   **vor** dem `recv_timeout` des Daemons (Marge), auch wenn jeder
        ///   Versuch real 90 ms kostet.
        #[test]
        fn quit_during_the_read_window_waits_or_warns() {
            let host = || {
                FakeHost::new()
                    .with_formats(vec![FakeFormat::text("vorher")])
                    .with_real_pump(10)
            };

            // Frist reicht: gesichert, Info.
            let (mut w, _out) = worker(host());
            start_paste(&w);
            let result = w.save_targets(Duration::from_secs(2));
            assert_eq!(result, Ok(ClipboardSave::Saved));
            assert!(!quit_save_log(&result).0);
            assert!(w.shutdown(Duration::from_secs(2)));

            // Frist reicht nicht: Timeout → Warnung.
            let (mut w, _out) = worker(host());
            start_paste(&w);
            let started = Instant::now();
            let result = w.save_targets(Duration::from_millis(150));
            assert!(started.elapsed() < Duration::from_millis(400));
            assert_eq!(result, Ok(ClipboardSave::Timeout));
            let (warn, line) = quit_save_log(&result);
            assert!(warn);
            assert_eq!(
                line,
                "Transkript beim Beenden nicht gesichert — Zwischenablage kann leer sein \
                 (keine Antwort des Inject-Workers innerhalb der Frist)"
            );
            assert!(w.shutdown(Duration::from_secs(3)));

            // Blockiert, jeder Versuch 90 ms: Antwort `Err` vor dem Timeout.
            let (mut w, _out) = worker(
                host()
                    .with_failing_materialize()
                    .with_materialize_cost(Duration::from_millis(90)),
            );
            start_paste(&w);
            let started = Instant::now();
            let timeout = Duration::from_millis(1_200);
            let result = w.save_targets(timeout);
            assert!(started.elapsed() < timeout, "{:?}", started.elapsed());
            let err = result.clone().unwrap_err();
            assert!(err.contains("nicht zu öffnen"), "{err}");
            let (warn, line) = quit_save_log(&result);
            assert!(warn);
            assert!(line.contains("nicht zu öffnen"), "{line}");
            assert!(w.shutdown(Duration::from_secs(2)));
        }

        /// Shortcut-Fehler + blockiertes Clipboard + Quit: die Logzeile ist die
        /// eindeutige Warnung (Final-Review Blocker 2).
        #[test]
        fn shortcut_failure_then_blocked_quit_logs_the_warning() {
            let mut host = FakeHost::new()
                .with_formats(vec![FakeFormat::text("vorher")])
                .with_failing_shortcut()
                .with_failing_materialize();
            assert!(host.paste("transkript", &ctx()).is_err());
            assert!(host.pending_promise());
            let result = host
                .save_to_clipboard_manager(Instant::now() + Duration::from_millis(200))
                .map_err(|err| err.to_string());
            let (warn, line) = quit_save_log(&result);
            assert!(warn);
            assert!(
                line.starts_with(
                    "Transkript beim Beenden nicht gesichert — Zwischenablage kann leer sein ("
                ),
                "{line}"
            );
            assert!(line.contains("nicht zu öffnen"), "{line}");
        }
    }

    /// Jeder nicht gesicherte oder ungeklärte Quit-Ausgang ist eine Warnung;
    /// `NotOwner` (ohne offenes Versprechen) und `Saved` bleiben Info.
    #[test]
    fn quit_save_lines_warn_on_every_unsecured_outcome() {
        let prefix = "Transkript beim Beenden nicht gesichert — Zwischenablage kann leer sein";
        assert_eq!(
            quit_save_log(&Ok(ClipboardSave::Saved)),
            (false, "Clipboard beim Beenden gesichert".into())
        );
        assert_eq!(
            quit_save_log(&Ok(ClipboardSave::NotOwner)),
            (
                false,
                "Clipboard beim Beenden: kein Clipboard-Eigentum".into()
            )
        );
        assert!(!quit_save_log(&Ok(ClipboardSave::NoManager)).0);
        for save in [
            ClipboardSave::Timeout,
            ClipboardSave::PromiseForeign,
            ClipboardSave::Refused,
        ] {
            let (warn, line) = quit_save_log(&Ok(save));
            assert!(warn, "{save:?}");
            assert_eq!(line, format!("{prefix} ({})", save.as_str()));
        }
        let (warn, line) = quit_save_log(&Err("Inject-Worker nicht erreichbar".into()));
        assert!(warn);
        assert_eq!(line, format!("{prefix} (Inject-Worker nicht erreichbar)"));
        assert_eq!(
            quit_save_log(&Ok(ClipboardSave::Timeout)).1,
            format!("{prefix} (keine Antwort des Inject-Workers innerhalb der Frist)")
        );
    }

    /// Blocker 1: `Lost` wird zum Inject-Fehler mit festem Text, `PromiseOpen`
    /// bleibt der Erfolgs-Report mit Warnung.
    #[test]
    fn transcript_state_maps_to_report_and_warning() {
        let ok = InjectReport::Pasted { notice: None };
        assert_eq!(transcript_report(&TranscriptState::Secured, ok.clone()), ok);
        assert_eq!(transcript_warning(&TranscriptState::Secured), None);

        let open = TranscriptState::PromiseOpen("OpenClipboard".into());
        assert_eq!(transcript_report(&open, ok.clone()), ok);
        let line = transcript_warning(&open).unwrap();
        assert!(line.contains("Versprechen offen"), "{line}");
        assert!(line.contains("OpenClipboard"), "{line}");

        let lost = TranscriptState::Lost("SetClipboardData: Win32-Fehler 8".into());
        assert_eq!(
            transcript_report(&lost, ok),
            InjectReport::Failed {
                message: "Zwischenablage leer — Transkript verloren \
                          (SetClipboardData: Win32-Fehler 8)"
                    .into()
            }
        );
        assert_eq!(
            transcript_warning(&lost).as_deref(),
            Some("Zwischenablage leer — Transkript verloren (SetClipboardData: Win32-Fehler 8)")
        );
    }

    /// Hinweis Marker: auch `copy_only` nennt einen fehlenden
    /// Verlaufsausschluss.
    #[test]
    fn copy_only_lines_name_a_missing_history_exclusion() {
        assert_eq!(
            with_history("copy_only · 12 Bytes".into(), true),
            "copy_only · 12 Bytes"
        );
        assert_eq!(
            with_history("copy_only: Fokus geändert".into(), false),
            "copy_only: Fokus geändert · Verlauf ausgeschlossen: nein"
        );
    }
}
