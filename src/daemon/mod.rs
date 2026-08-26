//! Daemon-Wiring (Spec §5, §12 Phase 3): eine Event-Loop treibt den puren
//! State-Machine-Kern, die Worker führen die Effekte aus.
//!
//! ```text
//!   Hotkey ┐                        ┌─ Engine  (Modell resident, Inferenz)
//!   Tray   ├─ mpsc::Sender<Msg> ─▶  │─ Audio   (cpal, Downmix, Resample)
//!   Audio  │      Event-Loop        │─ Inject  (X11: Clipboard, Paste, Fokus)
//!   Inject │   transition(…) →      └─ Tray    (betrayer, D-Bus)
//!   Timer  ┘   dispatch(effects)
//! ```
//!
//! Die Loop selbst macht **nichts Langes**: jeder Effekt wird zu einem
//! Kanal-Kommando. Damit greift `QuitRequested` auch während einer laufenden
//! Inferenz oder eines Restore-Wartens (codex H4 zu §7.1 P6).
//!
//! Phase 3d ergänzt: Single-Instance-Lock (§5.3), Modell-Download in einem
//! eigenen Worker (§6.3) und das Datei-Log samt Rotation (§10). Die Reihenfolge
//! beim Start ist dabei nicht beliebig: **erst** die Sperre, **dann** der
//! Datei-Sink — sonst schrieben zwei Prozesse in `diktier.log` (§10:
//! „Ein Writer besitzt die Datei.").

mod debug_wav;
mod dispatch;
mod logging;
mod signals;
mod workers;

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use crate::config::{self, ConfigError};
use crate::download::{self, ArtifactManifest, load_manifest};
use crate::hotkey::HotkeySpec;
use crate::inject::ClipboardSave;
use crate::paths;
use crate::single_instance::{self, InstanceAcquire};
use crate::state::{
    AppState, AudioInfo, CopyReason, ErrorInfo, ErrorKind, Event, LogEvent, RunId, Runtime,
};
use crate::tray;

use dispatch::{Actors, QuitLatch, Timers, drive, enqueue_batch};
use logging::Logger;
use workers::{
    AudioWorker, DownloadWorker, EngineWorker, HotkeyWorker, InjectWorker, Msg, TrayWorker,
    WorkerKind,
};

/// Takt der Kern-Uhr (`Event::Tick`). Kurz genug, dass Tray-Klicks und
/// Fristen ohne spürbare Verzögerung greifen, lang genug für ~50 Ticks/s.
const TICK: Duration = Duration::from_millis(20);

/// §5.2: „Beenden während Inferenz: Prozess darf nach Inferenz-Timeout (5 s)
/// hart enden."
const QUIT_HARD_LIMIT: Duration = Duration::from_secs(5);

/// Frist für den `SAVE_TARGETS`-Handshake im Quit-Pfad.
const SAVE_TARGETS_TIMEOUT: Duration = Duration::from_secs(2);

/// Der Daemon: `diktier` bzw. `diktier --foreground` (§9).
pub fn run(foreground: bool) -> u8 {
    let log = Arc::new(Logger::new(foreground));

    // §5.3: Nur der Daemon nimmt die Sperre, und er nimmt sie als Erstes —
    // an allen nutzbaren Orten zugleich, sonst liefen zwei Prozesse mit
    // unterschiedlichem `XDG_RUNTIME_DIR` aneinander vorbei.
    let lock = match single_instance::acquire_instance_lock(&mut |problem| log.warn(problem)) {
        Ok(InstanceAcquire::Held(lock)) => lock,
        Ok(InstanceAcquire::Busy) => {
            // Kurze stderr-Meldung, Exit 0, kein Fensterfokus, kein fremder
            // Prozess wird angefasst.
            eprintln!("diktier läuft bereits — dieser Start endet ohne Wirkung.");
            return 0;
        }
        Err(err) => {
            log.error(format!("Single-Instance-Sperre: {err}"));
            return 1;
        }
    };
    // §10: ab hier — und nur hier — gehört `diktier.log` diesem Prozess.
    attach_file_log(&log, foreground);
    log.info(format!("Sperre gehalten: {}", lock.describe()));

    let exit = run_locked(foreground, &log);
    drop(lock);
    exit
}

/// §10: Datei-Log anhängen. Scheitert das, bleibt stderr — ein unbeschreibbares
/// `~/.local/state` ist kein Grund, das Diktieren zu verweigern.
fn attach_file_log(log: &Logger, foreground: bool) {
    match paths::log_path() {
        Ok(path) => match log.attach_file(&path, paths::LOG_LIMIT_BYTES) {
            Ok(()) => log.info(format!("Log: {}", path.display())),
            Err(err) => log.warn(format!("Datei-Log {}: {err}", path.display())),
        },
        Err(err) => log.warn(format!("Datei-Log: {err}")),
    }
    if !log.has_file() && !foreground {
        // Ohne Datei und ohne Konsole bliebe von diesem Lauf nichts übrig.
        log.warn(
            "Kein Datei-Log und kein --foreground — Warnungen und Fehler gehen nur nach stderr",
        );
    }
}

/// §8-Tabelle „Fatal … Tray `error`" (codex M3): Ein Configfehler beendet den
/// Prozess nicht stumm, sondern zeigt einen bedienbaren Tray, der den Grund
/// nennt, den Config-Ordner öffnet und sich beenden lässt. Ohne Audio, ohne
/// Hotkey, ohne Engine — es gibt nichts zu diktieren.
///
/// Scheitert schon der Tray, gilt weiter §10: Prozessende, Exit 1.
fn config_error_mode(message: String, log: &Arc<Logger>) -> u8 {
    log.error(&message);
    signals::install();

    let (tx, rx) = mpsc::channel::<Msg>();
    let mut tray = match TrayWorker::spawn(
        config::DEFAULT_MODEL.to_string(),
        AppState::Error,
        false,
        tx,
        log.clone(),
    ) {
        Ok(worker) => worker,
        Err(err) => {
            log.error(format!("Tray-Aufbau gescheitert: {err}"));
            return 1;
        }
    };
    tray.update(
        AppState::Error,
        false,
        Some(ErrorInfo {
            kind: ErrorKind::Config,
            message,
        }),
    );
    log.info("Configfehler — Tray zeigt `error`, Beenden über das Menü");

    loop {
        if signals::take_quit_request() {
            break;
        }
        match rx.recv_timeout(TICK) {
            Ok(Msg::Event(Event::QuitRequested)) => break,
            Ok(Msg::OpenConfigDir) => match tray::open_config_dir() {
                Ok(()) => log.info("Config-Ordner geöffnet"),
                Err(err) => log.warn(format!("Config-Ordner: {err}")),
            },
            Ok(Msg::TrayLost(err)) => {
                log.error(format!("Tray verloren: {err}"));
                break;
            }
            // Pause/Tray-Click haben ohne Engine keine Wirkung.
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    tray.shutdown(Duration::from_secs(2));
    // §9: Bedien-/Configfehler.
    2
}

fn run_locked(foreground: bool, log: &Arc<Logger>) -> u8 {
    let log = log.clone();
    let loaded = match config::load() {
        Ok(loaded) => loaded,
        Err(ConfigError::Io(err)) => {
            // Kein Config-Inhalt, sondern ein I/O-Problem: dafür sieht §8 keinen
            // bedienbaren Zustand vor.
            log.error(format!("Config-Datei: {err}"));
            return 1;
        }
        // §8-Tabelle: „kein Hotkey, keine Aufnahme, Tray `error`" (codex M3).
        Err(err) => return config_error_mode(err.to_string(), &log),
    };
    for warning in &loaded.warnings {
        log.warn(format!("Config: {warning}"));
    }
    if loaded.created {
        log.info("Config-Datei mit Defaults angelegt");
    }
    let config = loaded.config;

    let manifest = match load_manifest() {
        Ok(manifest) => manifest,
        Err(err) => {
            log.error(err.to_string());
            return 1;
        }
    };
    if config.engine.model != manifest.key {
        // §6.2: unbekannter Modellschlüssel ist ein fataler Configfehler —
        // auch der zeigt sich im Tray (§8-Tabelle, codex M3).
        return config_error_mode(
            format!(
                "engine.model {:?} ist unbekannt — v1 kennt nur {:?}",
                config.engine.model, manifest.key
            ),
            &log,
        );
    }

    signals::install();
    log.info(format!(
        "diktier {} startet ({}, Modell {})",
        env!("CARGO_PKG_VERSION"),
        if foreground { "--foreground" } else { "Daemon" },
        manifest.key
    ));

    let (tx, rx) = mpsc::channel::<Msg>();

    // Inject zuerst: ohne X11-Ausgabe hätte ein Diktat kein Ziel.
    let inject = match InjectWorker::spawn(config.output.clone(), tx.clone(), log.clone()) {
        Ok(worker) => worker,
        Err(err) => {
            log.error(format!(
                "Ausgabepfad nicht verfügbar: {err}. Diktier v1 unterstützt nur X11 \
                 (Cinnamon/X11); unter Wayland fehlt der Paste-Pfad."
            ));
            return 1;
        }
    };

    // §10: „Tray-Aufbau gescheitert → Prozessende, stderr+Log, Exit 1."
    let tray_worker = match TrayWorker::spawn(
        manifest.key.clone(),
        AppState::Starting,
        false,
        tx.clone(),
        log.clone(),
    ) {
        Ok(worker) => worker,
        Err(err) => {
            log.error(format!("Tray-Aufbau gescheitert: {err}"));
            let mut inject = inject;
            inject.shutdown(Duration::from_secs(2));
            return 1;
        }
    };

    // codex M2: Ein Worker, der nicht startet, ist ein Startfehler — sonst
    // liefe der Daemon ohne Mikrofon bzw. ohne Hotkey stumm weiter.
    let audio = match AudioWorker::spawn(config.audio.clone(), tx.clone(), log.clone()) {
        Ok(worker) => worker,
        Err(err) => {
            log.error(format!("Audio-Worker nicht gestartet: {err}"));
            let (mut inject, mut tray_worker) = (inject, tray_worker);
            inject.shutdown(Duration::from_secs(2));
            tray_worker.shutdown(Duration::from_secs(2));
            return 1;
        }
    };
    // §4.4: Die konfigurierte Taste — nicht mehr hart F9 (codex H3).
    let spec = HotkeySpec::from_config(&config.hotkey);
    let hotkey = match HotkeyWorker::spawn(spec.clone(), tx.clone(), log.clone()) {
        Ok(worker) => worker,
        Err(err) => {
            log.error(format!("Hotkey-Worker nicht gestartet: {err}"));
            let (mut inject, mut tray_worker, mut audio) = (inject, tray_worker, audio);
            inject.shutdown(Duration::from_secs(2));
            tray_worker.shutdown(Duration::from_secs(2));
            audio.shutdown(Duration::from_secs(2));
            return 1;
        }
    };

    let mut daemon = Daemon {
        log: log.clone(),
        manifest,
        threads: config.engine.threads,
        tx,
        engine: None,
        audio,
        inject,
        tray: tray_worker,
        hotkey,
        hotkey_grabbed: true,
        download: None,
        pending_audio: None,
        emitted: Vec::new(),
        tray_dirty: false,
        shown: None,
        audio_intent: None,
    };

    let cap = Duration::from_secs(u64::from(config.audio.max_duration_secs.max(1)));
    let mut runtime = Runtime::with_cap(cap);
    let exit = event_loop(&mut daemon, &mut runtime, &rx);
    daemon.shutdown(exit)
}

struct Daemon {
    log: Arc<Logger>,
    manifest: ArtifactManifest,
    threads: u32,
    tx: Sender<Msg>,
    /// `None`, solange kein Modell geladen wird — nach dem Watchdog kurzzeitig
    /// auch dann, wenn der alte Worker noch in einer Inferenz steht (§5.2).
    engine: Option<EngineWorker>,
    audio: AudioWorker,
    inject: InjectWorker,
    tray: TrayWorker,
    hotkey: HotkeyWorker,
    /// §4.4: Hält der Hotkey-Worker die Taste gerade gegriffen?
    hotkey_grabbed: bool,
    /// Läuft nur, solange der Kern in `downloading` steht (§6.3).
    download: Option<DownloadWorker>,
    /// Samples der letzten Aufnahme; der Kern kennt nur ihre Länge.
    pending_audio: Option<(RunId, Vec<f32>)>,
    /// Events, die ein Effekt synchron erzeugt hat (Artefaktprüfung, Fehler).
    emitted: Vec<Event>,
    /// Der Kern hat `UpdateTray` gemeldet — beim nächsten Abgleich neu malen.
    tray_dirty: bool,
    /// Was der Tray zuletzt gezeigt hat (inklusive Fehlergrund, codex M1).
    shown: Option<Presentation>,
    /// Was das Audio-Gerät zuletzt tun sollte (agy B5).
    audio_intent: Option<AudioIntent>,
}

/// Sichtbarer Zustand — mehr, als `Effect::UpdateTray` transportiert: der
/// Fehlergrund kommt aus dem `Runtime` dazu, damit §4.4 („Tooltip nennt den
/// Konflikt") erfüllt ist.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Presentation {
    state: AppState,
    paused: bool,
    error: Option<ErrorInfo>,
}

/// agy B5: Der Gerätelebenszyklus hängt jetzt am beobachteten Kernzustand,
/// nicht mehr als Seiteneffekt am Tray-Update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AudioIntent {
    /// §5: In `idle` steht das Gerät bereit, damit die Aufnahme sofort startet.
    Ready,
    /// §4.3: Pausiert heißt „jetzt nicht diktieren" — Mikrofon hergeben.
    Released,
}

/// Was soll mit dem Aufnahmegerät geschehen? `None` = nicht anfassen
/// (`recording` gehört dem laufenden Diktat, in der Startsequenz und in
/// `transcribing`/`injecting` folgt gleich wieder `idle`).
fn audio_intent(runtime: &Runtime) -> Option<AudioIntent> {
    match runtime.state {
        AppState::Idle if runtime.paused => Some(AudioIntent::Released),
        AppState::Idle => Some(AudioIntent::Ready),
        _ => None,
    }
}

impl Daemon {
    /// Nach jedem Kern-Durchlauf: Tray, Hotkey-Grab und Audio-Gerät an den
    /// tatsächlichen Zustand angleichen. Bewusst **hier** und nicht im
    /// `UpdateTray`-Effekt — Geräte- und Grab-Steuerung sind eigene Anliegen
    /// (agy B5), und der Fehlergrund steht nur im `Runtime` (codex M1).
    fn flush_presentation(&mut self, runtime: &Runtime) {
        let next = Presentation {
            state: runtime.state,
            paused: runtime.paused,
            error: runtime.error.clone(),
        };
        if self.tray_dirty || self.shown.as_ref() != Some(&next) {
            self.tray
                .update(next.state, next.paused, next.error.clone());
            self.shown = Some(next);
            self.tray_dirty = false;
        }

        // §4.4: „paused = Hotkey aus" heißt Grab freigeben, nicht nur ignorieren.
        let grabbed = !runtime.paused;
        if self.hotkey_grabbed != grabbed {
            self.hotkey.set_grabbed(grabbed);
            self.hotkey_grabbed = grabbed;
        }

        if let Some(intent) = audio_intent(runtime)
            && self.audio_intent != Some(intent)
        {
            match intent {
                AudioIntent::Ready => self.audio.prepare(),
                AudioIntent::Released => self.audio.release(),
            }
            self.audio_intent = Some(intent);
        }
    }

    /// codex M2: Ein ausgefallener Worker wird vergessen, damit der nächste
    /// Anlauf einen frischen spawnt statt in einen toten Kanal zu schreiben.
    fn forget_worker(&mut self, what: WorkerKind) {
        match what {
            WorkerKind::Engine => {
                if let Some(engine) = self.engine.take() {
                    engine.abandon();
                }
            }
            // Audio und Inject leben so lange wie der Daemon; ein Neuaufbau
            // wäre ein eigener Fehlerpfad (§6.4 heilt das Mikrofon über den
            // nächsten Press, der Inject-Sink über das nächste Diktat).
            WorkerKind::Audio | WorkerKind::Inject => {}
        }
    }

    /// §6.3: Existenz und Größe der Artefakte. Der SHA-256-Vollcheck gehört in
    /// den Download-Pfad (Phase 3d) — er kostet beim Kaltstart Sekunden.
    fn artifacts_complete(&self) -> Result<(), String> {
        let dir = download::model_dir(&self.manifest.key).map_err(|e| e.to_string())?;
        download::check_artifacts(&dir, &self.manifest).map_err(|e| e.to_string())
    }

    fn model_dir_hint(&self) -> String {
        match download::model_dir(&self.manifest.key) {
            Ok(dir) => dir.display().to_string(),
            Err(err) => err.to_string(),
        }
    }

    /// Quit-Pfad (§5.2): Clipboard sichern, Worker beenden, notfalls hart raus.
    fn shutdown(mut self, exit: u8) -> u8 {
        let deadline = Instant::now() + QUIT_HARD_LIMIT;
        self.log.info("Beenden …");

        // Zuerst das Abbruchsignal, noch vor `save_targets`: der Download-Thread
        // hat damit die ganze Restzeit, um den laufenden Block zu Ende zu lesen.
        if let Some(download) = &self.download {
            download.cancel();
        }

        // Phase-2-Erkenntnis (`csd-clipboard`): ohne SAVE_TARGETS verliert
        // Cinnamon beim Owner-Exit den Clipboard-Inhalt.
        let budget = remaining(deadline).min(SAVE_TARGETS_TIMEOUT);
        if !budget.is_zero() {
            match self.inject.save_targets(budget) {
                ClipboardSave::Saved => self
                    .log
                    .info("Clipboard an den Clipboard-Manager übergeben"),
                other => self
                    .log
                    .info(format!("Clipboard beim Beenden: {}", other.as_str())),
            }
        }

        let mut stuck: Vec<&'static str> = Vec::new();
        if !self
            .hotkey
            .shutdown(remaining(deadline).min(Duration::from_secs(2)))
        {
            stuck.push("hotkey");
        }
        if !self
            .tray
            .shutdown(remaining(deadline).min(Duration::from_secs(2)))
        {
            stuck.push("tray");
        }
        if !self
            .audio
            .shutdown(remaining(deadline).min(Duration::from_secs(2)))
        {
            stuck.push("audio");
        }
        if !self
            .inject
            .shutdown(remaining(deadline).min(Duration::from_secs(2)))
        {
            stuck.push("inject");
        }
        if let Some(mut download) = self.download.take()
            && !download.shutdown(remaining(deadline).min(Duration::from_secs(2)))
        {
            // Ein blockierender `read` auf dem Socket lässt sich nicht
            // abbrechen — dann endet der Prozess hart (§5.2).
            stuck.push("download");
        }
        if let Some(mut engine) = self.engine.take()
            && !engine.shutdown(remaining(deadline))
        {
            stuck.push("engine");
        }

        if stuck.is_empty() {
            self.log.info("beendet");
            return exit;
        }
        // §5.2: laufende Inferenz hält den Prozess nicht auf.
        self.log.warn(format!(
            "Worker nicht beendet ({}) — hartes Prozessende nach {} s",
            stuck.join(", "),
            QUIT_HARD_LIMIT.as_secs()
        ));
        std::process::exit(i32::from(exit));
    }
}

fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

impl Actors for Daemon {
    fn check_artifacts(&mut self, run: RunId) {
        match self.artifacts_complete() {
            Ok(()) => self.emitted.push(Event::ArtifactsChecked {
                run,
                complete: true,
            }),
            Err(problem) => {
                self.log.warn(format!("Modellartefakte: {problem}"));
                self.emitted.push(Event::ArtifactsChecked {
                    run,
                    complete: false,
                });
            }
        }
    }

    fn start_download(&mut self, run: RunId) {
        // §6.3: eigener Thread. Ein alter Worker kann hier nicht mehr laufen —
        // der Kern verlässt `downloading` nur über `DownloadFinished`/`Failed`.
        if let Some(mut old) = self.download.take() {
            old.shutdown(Duration::from_millis(100));
        }
        self.log.run(
            run,
            format!("Modellartefakte fehlen in {}", self.model_dir_hint()),
        );
        match DownloadWorker::spawn(
            run,
            self.manifest.clone(),
            self.tx.clone(),
            self.log.clone(),
        ) {
            Ok(worker) => self.download = Some(worker),
            // codex M2: sonst bliebe der Kern für immer in `downloading`.
            Err(message) => {
                self.log.error(&message);
                self.emitted.push(Event::DownloadFailed { run, message });
            }
        }
    }

    fn load_model(&mut self, run: RunId) {
        if self.engine.is_none() {
            match EngineWorker::spawn(
                self.manifest.key.clone(),
                self.threads,
                self.tx.clone(),
                self.log.clone(),
            ) {
                Ok(worker) => self.engine = Some(worker),
                // codex M2: sonst bliebe der Kern für immer in `loading`.
                Err(message) => {
                    self.log.error(&message);
                    self.emitted.push(Event::ModelLoadFailed { run, message });
                    return;
                }
            }
        }
        if let Some(engine) = &self.engine {
            self.log.run(run, "Modell wird geladen");
            engine.load(run);
        }
    }

    fn start_capture(&mut self, run: RunId, cap: Duration) {
        // §7.3: Start-Fensterkennung, bevor gesprochen wird.
        self.inject.mark_start(run);
        self.audio.start(run);
        self.log
            .run(run, format!("Aufnahme (Cap {} s)", cap.as_secs()));
    }

    fn stop_capture(&mut self, run: RunId, discard: bool) {
        if !discard {
            // §7.3: Ziel-Fensterkennung zum Aufnahmeende.
            self.inject.mark_target(run);
        }
        self.audio.stop(run, discard);
    }

    fn start_transcription(&mut self, run: RunId) {
        let samples = match self.pending_audio.take() {
            Some((have, samples)) if have == run => samples,
            other => {
                // Kann nur passieren, wenn ein Lauf dazwischen verworfen wurde.
                if let Some((have, _)) = other {
                    self.log.warn(format!(
                        "Aufnahme von Lauf {} passt nicht zu {}",
                        have.0, run.0
                    ));
                }
                self.emitted.push(Event::TranscriptionFailed {
                    run,
                    message: "Aufnahme nicht verfügbar".into(),
                });
                return;
            }
        };
        match &self.engine {
            Some(engine) => engine.transcribe(run, samples),
            None => self.emitted.push(Event::TranscriptionFailed {
                run,
                message: "Modell ist nicht geladen".into(),
            }),
        }
    }

    fn abort_transcription(&mut self, run: RunId) {
        // Anlass ist der Watchdog (§5.2), ein fataler Fehler oder das Beenden —
        // die Logzeile daneben sagt, welcher.
        self.log.run(run, "Inferenz verworfen");
        // `parakeet-rs` kennt keinen Abbruch: der Worker läuft aus, sein
        // Ergebnis trägt dann eine tote Generation. Der Reinit bekommt einen
        // frischen Worker (`load_model`).
        if let Some(engine) = self.engine.take() {
            engine.abandon();
        }
        self.pending_audio = None;
    }

    fn start_inject(&mut self, run: RunId, text: String) {
        self.inject.paste(run, text);
    }

    fn copy_only(&mut self, run: RunId, text: String, reason: CopyReason) {
        self.inject.copy_only(run, text, reason);
    }

    fn update_tray(&mut self, state: AppState, paused: bool) {
        self.log.transition(state, paused);
        // Gemalt wird im Abgleich nach dem Kern-Durchlauf: erst dort steht der
        // Fehlergrund für den Tooltip zur Verfügung (codex M1).
        self.tray_dirty = true;
    }

    fn log(&mut self, event: &LogEvent) {
        self.log.core(event);
    }

    fn quit(&mut self) {
        self.log.info("Kern hat das Beenden bestätigt");
    }

    fn output_suppressed(&mut self, run: RunId) {
        // §5.2: „kein Inject mehr" — der Text bleibt, wo er ist.
        self.log
            .run(run, "Ausgabe nach dem Beenden unterdrückt (kein Inject)");
    }

    fn take_emitted(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.emitted)
    }
}

enum Flow {
    Continue,
    Stop(u8),
}

/// Die zentrale Schleife: Events sammeln, Kern rechnen lassen, Effekte verteilen.
fn event_loop(daemon: &mut Daemon, runtime: &mut Runtime, rx: &Receiver<Msg>) -> u8 {
    let mut queue: VecDeque<Event> = VecDeque::new();
    let mut timers = Timers::default();
    let mut latch = QuitLatch::default();
    let mut hotkey_error: Option<String> = None;
    let mut last_tick = Instant::now();
    queue.push_back(Event::Startup);

    loop {
        // 1. §5.2/codex H2: Das Quit hat Vorrang **vor** allem, was schon in
        //    der Queue liegt — sonst injizierte ein zeitgleich eingetroffenes
        //    Engine-Ergebnis noch, bevor der Kern das Beenden sieht.
        if signals::take_quit_request() {
            if latch.close() {
                daemon.log.info("Signal empfangen — Beenden angefordert");
            }
            queue.push_front(Event::QuitRequested);
        }

        // 2. Kern treiben, bis nichts mehr ansteht.
        if drive(
            &mut queue,
            runtime,
            &mut timers,
            daemon,
            &mut latch,
            Instant::now(),
        ) {
            return 0;
        }
        daemon.flush_presentation(runtime);

        // 3. §4.4/§10: Der Hotkey-Fehler wird erst gemeldet, wenn das Modell
        //    steht. Sonst stürbe die Startsequenz, und der Tray-Click — der
        //    laut §4.3 bedienbar bleiben soll — bräuchte ein Modell, das nie
        //    geladen würde.
        if runtime.state == AppState::Idle
            && let Some(message) = hotkey_error.take()
        {
            queue.push_back(Event::FatalError {
                kind: ErrorKind::HotkeyRegistration,
                message,
            });
            continue;
        }

        // 4. Auf Worker-Nachrichten warten — höchstens bis zum nächsten Tick
        //    bzw. bis zur nächsten Frist. Die ganze Batch wird eingesammelt und
        //    gemeinsam einsortiert, damit ein Quit darin nach vorn kommt.
        let now = Instant::now();
        let mut wait = (last_tick + TICK).saturating_duration_since(now);
        if let Some(deadline) = timers.next_deadline() {
            wait = wait.min(deadline.saturating_duration_since(now));
        }
        let mut batch: Vec<Event> = Vec::new();
        match rx.recv_timeout(wait.max(Duration::from_millis(1))) {
            Ok(msg) => {
                if let Flow::Stop(code) = handle_msg(msg, &mut batch, daemon, &mut hotkey_error) {
                    return code;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return 1,
        }
        while let Ok(msg) = rx.try_recv() {
            if let Flow::Stop(code) = handle_msg(msg, &mut batch, daemon, &mut hotkey_error) {
                return code;
            }
        }
        if enqueue_batch(&mut queue, batch) && latch.close() {
            daemon.log.info("Beenden angefordert — keine Ausgabe mehr");
        }

        // 5. Fristen **vor** dem Tick: sonst löst die Kern-Uhr denselben
        //    Übergang ein zweites Mal aus (§4.4: „genau einmal").
        let now = Instant::now();
        for event in timers.due(now) {
            queue.push_back(event);
        }
        let elapsed = now.duration_since(last_tick);
        if elapsed >= TICK {
            last_tick = now;
            queue.push_back(Event::Tick { elapsed });
        }
    }
}

fn handle_msg(
    msg: Msg,
    batch: &mut Vec<Event>,
    daemon: &mut Daemon,
    hotkey_error: &mut Option<String>,
) -> Flow {
    match msg {
        Msg::Event(event) => batch.push(event),
        Msg::Audio { run, samples } => {
            let audio = AudioInfo {
                duration: audio_duration(samples.len()),
            };
            daemon.pending_audio = Some((run, samples));
            batch.push(Event::AudioReady { run, audio });
        }
        Msg::HotkeyUnavailable(message) => {
            daemon.log.warn(format!(
                "Hotkey nicht verfügbar: {message} — Tray-Linksklick bleibt bedienbar"
            ));
            *hotkey_error = Some(message);
        }
        // codex M2: Ein Worker, der nicht mehr erreichbar ist, darf den Daemon
        // nicht stumm hängen lassen — er wird zum Fehlerzustand seiner Klasse.
        Msg::WorkerFailed { what, message } => {
            daemon.log.error(format!("{}: {message}", what.label()));
            daemon.forget_worker(what);
            batch.push(what.to_fatal(message));
        }
        Msg::TrayLost(message) => {
            // §10: ohne Tray gibt es keinen zweiten GUI-Kanal.
            daemon.log.error(format!("Tray verloren: {message}"));
            return Flow::Stop(1);
        }
        Msg::OpenConfigDir => match tray::open_config_dir() {
            Ok(()) => daemon.log.info("Config-Ordner geöffnet"),
            Err(err) => daemon.log.warn(format!("Config-Ordner: {err}")),
        },
    }
    Flow::Continue
}

fn audio_duration(samples: usize) -> Duration {
    Duration::from_secs_f64(samples as f64 / f64::from(crate::audio::ENGINE_RATE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_duration_uses_engine_rate() {
        assert_eq!(audio_duration(16_000), Duration::from_secs(1));
        assert_eq!(audio_duration(4_000), Duration::from_millis(250));
        assert_eq!(audio_duration(0), Duration::ZERO);
    }

    #[test]
    fn remaining_never_goes_negative() {
        let past = Instant::now() - Duration::from_secs(1);
        assert_eq!(remaining(past), Duration::ZERO);
        let future = Instant::now() + Duration::from_secs(30);
        assert!(remaining(future) > Duration::from_secs(29));
    }

    fn runtime_in(state: AppState, paused: bool) -> Runtime {
        Runtime {
            state,
            paused,
            ..Runtime::default()
        }
    }

    /// agy B5: Der Gerätezustand hängt am Kernzustand, nicht am Tray-Effekt.
    /// In `idle` bereit, pausiert freigegeben — sonst wird nichts angefasst.
    #[test]
    fn audio_intent_follows_the_core_state() {
        assert_eq!(
            audio_intent(&runtime_in(AppState::Idle, false)),
            Some(AudioIntent::Ready)
        );
        assert_eq!(
            audio_intent(&runtime_in(AppState::Idle, true)),
            Some(AudioIntent::Released)
        );
        for state in [
            AppState::Starting,
            AppState::Downloading,
            AppState::Loading,
            AppState::Error,
            AppState::Recording {
                source: crate::state::RecordingSource::Hotkey,
            },
            AppState::Transcribing {
                source: crate::state::RecordingSource::Hotkey,
            },
            AppState::Injecting {
                source: crate::state::RecordingSource::TrayClick,
            },
        ] {
            assert_eq!(
                audio_intent(&runtime_in(state, false)),
                None,
                "{state:?} darf das Gerät nicht anfassen"
            );
        }
    }
}
