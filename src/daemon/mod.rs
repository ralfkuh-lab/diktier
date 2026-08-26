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
//! Nicht in Phase 3c (kommt in 3d): Single-Instance-Lock (§5.3),
//! Modell-Download (§6.3), Autostart (§9), Datei-Log samt Rotation (§10).

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
use crate::inject::ClipboardSave;
use crate::state::{
    AppState, AudioInfo, CopyReason, ErrorKind, Event, LogEvent, RunId, Runtime, transition,
};
use crate::tray;

use dispatch::{Actors, Timers, dispatch};
use logging::Logger;
use workers::{AudioWorker, EngineWorker, HotkeyWorker, InjectWorker, Msg, TrayWorker};

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

    let loaded = match config::load() {
        Ok(loaded) => loaded,
        Err(err) => {
            log.error(err.to_string());
            return match err {
                ConfigError::Io(_) => 1,
                _ => 2,
            };
        }
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
        // §6.2: unbekannter Modellschlüssel ist ein fataler Configfehler.
        log.error(format!(
            "engine.model {:?} ist unbekannt — v1 kennt nur {:?}",
            config.engine.model, manifest.key
        ));
        return 2;
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

    let audio = AudioWorker::spawn(config.audio.clone(), tx.clone(), log.clone());
    let hotkey = HotkeyWorker::spawn(tx.clone(), log.clone());

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
        pending_audio: None,
        emitted: Vec::new(),
        quit: false,
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
    /// Samples der letzten Aufnahme; der Kern kennt nur ihre Länge.
    pending_audio: Option<(RunId, Vec<f32>)>,
    /// Events, die ein Effekt synchron erzeugt hat (Artefaktprüfung, Fehler).
    emitted: Vec<Event>,
    quit: bool,
}

impl Daemon {
    fn take_emitted(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.emitted)
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
        // Phase 3c lädt nichts nach (§12 Phase 3 ist auf 3d aufgeteilt) —
        // stattdessen ein Fehler, der sagt, was zu tun ist.
        let message = format!(
            "Modellartefakte fehlen in {}. Der automatische Download folgt; \
             bis dahin die vier Dateien aus src/models.toml dorthin legen.",
            self.model_dir_hint()
        );
        self.log.error(&message);
        self.emitted.push(Event::DownloadFailed { run, message });
    }

    fn load_model(&mut self, run: RunId) {
        if self.engine.is_none() {
            self.engine = Some(EngineWorker::spawn(
                self.manifest.key.clone(),
                self.threads,
                self.tx.clone(),
                self.log.clone(),
            ));
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
        self.tray.update(state, paused);
        // §5: „Aufnahme aus `idle` startet sofort." Das Gerät wird deshalb im
        // Ruhezustand offen gehalten — sonst kostet jeder Aufnahmestart das
        // Neuöffnen (~2 s gemessen) und schneidet den Anfang des Diktats ab.
        //
        // §4.3: `paused` heißt „jetzt nicht diktieren" — dann gibt Diktier das
        // Mikrofon wieder her (Owner-Entscheidung 3c). Der Tray-Click bleibt
        // bedienbar, zahlt in der Pause aber wieder den Geräteanlauf.
        //
        // Beide Kommandos sind idempotent; die übrigen Zustände fassen das
        // Gerät nicht an (`recording` gehört dem laufenden Diktat, und in
        // `transcribing`/`injecting` folgt gleich wieder `idle`).
        if state == AppState::Idle {
            if paused {
                self.audio.release();
            } else {
                self.audio.prepare();
            }
        }
    }

    fn log(&mut self, event: &LogEvent) {
        self.log.core(event);
    }

    fn quit(&mut self) {
        self.quit = true;
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
    let mut hotkey_error: Option<String> = None;
    let mut last_tick = Instant::now();
    queue.push_back(Event::Startup);

    loop {
        // 1. Kern treiben, bis nichts mehr ansteht.
        while let Some(event) = queue.pop_front() {
            let effects = transition(runtime, event);
            dispatch(effects, &mut timers, daemon, Instant::now());
            queue.extend(daemon.take_emitted());
            if daemon.quit {
                return 0;
            }
        }

        // 2. SIGTERM/SIGINT auf den regulären Quit-Pfad.
        if signals::take_quit_request() {
            daemon.log.info("Signal empfangen — Beenden angefordert");
            queue.push_back(Event::QuitRequested);
            continue;
        }

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
        //    bzw. bis zur nächsten Frist.
        let now = Instant::now();
        let mut wait = (last_tick + TICK).saturating_duration_since(now);
        if let Some(deadline) = timers.next_deadline() {
            wait = wait.min(deadline.saturating_duration_since(now));
        }
        match rx.recv_timeout(wait.max(Duration::from_millis(1))) {
            Ok(msg) => {
                if let Flow::Stop(code) = handle_msg(msg, &mut queue, daemon, &mut hotkey_error) {
                    return code;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return 1,
        }
        while let Ok(msg) = rx.try_recv() {
            if let Flow::Stop(code) = handle_msg(msg, &mut queue, daemon, &mut hotkey_error) {
                return code;
            }
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
    queue: &mut VecDeque<Event>,
    daemon: &mut Daemon,
    hotkey_error: &mut Option<String>,
) -> Flow {
    match msg {
        Msg::Event(event) => queue.push_back(event),
        Msg::Audio { run, samples } => {
            let audio = AudioInfo {
                duration: audio_duration(samples.len()),
            };
            daemon.pending_audio = Some((run, samples));
            queue.push_back(Event::AudioReady { run, audio });
        }
        Msg::HotkeyUnavailable(message) => {
            daemon.log.warn(format!(
                "Hotkey nicht verfügbar: {message} — Tray-Linksklick bleibt bedienbar"
            ));
            *hotkey_error = Some(message);
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
}
