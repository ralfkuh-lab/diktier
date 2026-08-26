//! Effekt-Dispatcher und Timer-Ableitung (Spec §5.2, §7.3, §12 Phase 3).
//!
//! Der State-Machine-Kern (`crate::state`) bleibt pur: er liefert
//! [`Effect`]-Listen, hier werden sie in Worker-Kommandos und Timer übersetzt.
//! Beides ist über das Trait [`Actors`] gegen Fakes testbar — der echte Daemon
//! implementiert es mit Threads, die Tests mit einem Rekorder.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::state::{AppState, CopyReason, Effect, Event, LogEvent, RunId, Runtime, transition};

/// Ausgabeseite des Wirings: eine Methode je Aktionseffekt.
pub trait Actors {
    /// §6.3: Artefakte prüfen, Antwort als `ArtifactsChecked`.
    fn check_artifacts(&mut self, run: RunId);
    /// §6.3: Download anstoßen (Phase 3d — in 3c ein verständlicher Fehler).
    fn start_download(&mut self, run: RunId);
    /// §6.1: ORT initialisieren und Modell resident laden.
    fn load_model(&mut self, run: RunId);
    /// §7.3: Aufnahme starten **und** `CaptureContext.start_window_id` merken.
    fn start_capture(&mut self, run: RunId, cap: Duration);
    /// §7.3: Aufnahme beenden **und** `CaptureContext.target_window_id` merken.
    fn stop_capture(&mut self, run: RunId, discard: bool);
    fn start_transcription(&mut self, run: RunId);
    fn abort_transcription(&mut self, run: RunId);
    fn start_inject(&mut self, run: RunId, text: String);
    fn copy_only(&mut self, run: RunId, text: String, reason: CopyReason);
    fn update_tray(&mut self, state: AppState, paused: bool);
    fn log(&mut self, event: &LogEvent);
    fn quit(&mut self);
    /// §5.2 „kein Inject mehr": Ein Ausgabeeffekt wurde nach dem Quit-Latch
    /// verworfen. Nur zum Protokollieren — es passiert nichts mehr.
    fn output_suppressed(&mut self, run: RunId);
    /// Events, die ein Effekt synchron erzeugt hat (Artefaktprüfung, Fehler).
    fn take_emitted(&mut self) -> Vec<Event> {
        Vec::new()
    }
}

/// §5.2 „Beenden während Inferenz: … kein Inject mehr."
///
/// Der Kern verwirft nach `QuitRequested` alles (`IgnoredAfterQuit`), aber
/// zwischen einem zeitgleich eintreffenden Engine-Ergebnis und dem Quit gäbe es
/// sonst eine Prioritätsinversion: FIFO würde erst `StartInject` dispatchen und
/// den Lauf danach verwerfen (codex H2). Der Latch schließt in dem Moment, in
/// dem das Quit **bekannt** wird — nicht erst, wenn der Kern es verarbeitet hat.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct QuitLatch {
    closed: bool,
}

impl QuitLatch {
    /// `true`, wenn dieser Aufruf den Latch geschlossen hat (für genau eine
    /// Logzeile).
    pub fn close(&mut self) -> bool {
        let first = !self.closed;
        self.closed = true;
        first
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

/// Eine empfangene Nachrichten-Batch einreihen — ein enthaltenes
/// `QuitRequested` kommt **vor** allem, was schon wartet (codex H2).
/// Rückgabe: war ein Quit dabei?
pub fn enqueue_batch(queue: &mut VecDeque<Event>, batch: Vec<Event>) -> bool {
    let has_quit = batch.iter().any(|e| matches!(e, Event::QuitRequested));
    if has_quit {
        queue.push_front(Event::QuitRequested);
    }
    for event in batch {
        if matches!(event, Event::QuitRequested) {
            continue;
        }
        queue.push_back(event);
    }
    has_quit
}

/// Die Queue vollständig abarbeiten: Kern rechnen lassen, Effekte verteilen,
/// synchron erzeugte Events anhängen. Rückgabe: `true` = der Kern hat `Quit`
/// gemeldet, die Event-Loop endet.
pub fn drive<A: Actors>(
    queue: &mut VecDeque<Event>,
    runtime: &mut Runtime,
    timers: &mut Timers,
    actors: &mut A,
    latch: &mut QuitLatch,
    now: Instant,
) -> bool {
    while let Some(event) = queue.pop_front() {
        let effects = transition(runtime, event);
        let quit = dispatch(effects, timers, actors, latch, now);
        for extra in actors.take_emitted() {
            queue.push_back(extra);
        }
        if quit {
            return true;
        }
    }
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Deadline {
    run: RunId,
    at: Instant,
}

/// Die beiden Fristen aus §5.2 als Wiring-Timer.
///
/// Der Kern führt dieselben Fristen in seiner eigenen Uhr (`Runtime::now`, die
/// nur über `Event::Tick` wächst) und würde sie beim Tick ebenfalls auslösen.
/// Damit nichts doppelt feuert, prüft die Event-Loop **erst** diese Timer und
/// schickt den Tick danach: `CapReached`/`WatchdogTimeout` räumen die
/// Kern-Deadline mit ab.
#[derive(Debug, Default)]
pub struct Timers {
    cap: Option<Deadline>,
    watchdog: Option<Deadline>,
}

impl Timers {
    pub fn arm_cap(&mut self, run: RunId, cap: Duration, now: Instant) {
        self.cap = Some(Deadline { run, at: now + cap });
    }

    pub fn disarm_cap(&mut self) {
        self.cap = None;
    }

    pub fn arm_watchdog(&mut self, run: RunId, timeout: Duration, now: Instant) {
        self.watchdog = Some(Deadline {
            run,
            at: now + timeout,
        });
    }

    pub fn disarm_watchdog(&mut self) {
        self.watchdog = None;
    }

    pub fn cap_deadline(&self) -> Option<Instant> {
        self.cap.map(|d| d.at)
    }

    pub fn watchdog_deadline(&self) -> Option<Instant> {
        self.watchdog.map(|d| d.at)
    }

    /// Nächster Weckzeitpunkt für `recv_timeout` der Event-Loop.
    pub fn next_deadline(&self) -> Option<Instant> {
        match (self.cap_deadline(), self.watchdog_deadline()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Fällige Fristen als Kern-Events; ein gefeuerter Timer ist danach aus.
    pub fn due(&mut self, now: Instant) -> Vec<Event> {
        let mut out = Vec::new();
        if let Some(deadline) = self.cap
            && now >= deadline.at
        {
            self.cap = None;
            out.push(Event::CapReached { run: deadline.run });
        }
        if let Some(deadline) = self.watchdog
            && now >= deadline.at
        {
            self.watchdog = None;
            out.push(Event::WatchdogTimeout { run: deadline.run });
        }
        out
    }
}

/// Ein Effektpaket ausführen. Reihenfolge bleibt exakt die des Kerns.
/// Rückgabe: `true`, wenn `Effect::Quit` dabei war.
pub fn dispatch<A: Actors>(
    effects: Vec<Effect>,
    timers: &mut Timers,
    actors: &mut A,
    latch: &mut QuitLatch,
    now: Instant,
) -> bool {
    let mut quit = false;
    for effect in effects {
        match effect {
            Effect::CheckArtifacts { run } => actors.check_artifacts(run),
            Effect::StartDownload { run } => actors.start_download(run),
            Effect::LoadModel { run } => actors.load_model(run),
            Effect::StartCapture { run, cap } => {
                // §4.4: Der 60-s-Cap ist eine Wiring-Frist, nicht nur eine
                // Zusage des Capture-Workers.
                timers.arm_cap(run, cap, now);
                actors.start_capture(run, cap);
            }
            Effect::StopCapture { run, discard } => {
                timers.disarm_cap();
                actors.stop_capture(run, discard);
            }
            Effect::StartTranscription { run } => actors.start_transcription(run),
            Effect::AbortTranscription { run } => actors.abort_transcription(run),
            // §5.2: Nach dem Quit verlässt kein Text mehr den Prozess.
            Effect::StartInject { run, text } => {
                if latch.is_closed() {
                    actors.output_suppressed(run);
                } else {
                    actors.start_inject(run, text);
                }
            }
            Effect::CopyOnly { run, text, reason } => {
                if latch.is_closed() {
                    actors.output_suppressed(run);
                } else {
                    actors.copy_only(run, text, reason);
                }
            }
            Effect::UpdateTray { state, paused } => actors.update_tray(state, paused),
            Effect::ArmWatchdog { run, timeout } => timers.arm_watchdog(run, timeout, now),
            Effect::DisarmWatchdog => timers.disarm_watchdog(),
            Effect::Log(event) => actors.log(&event),
            Effect::Quit => {
                latch.close();
                quit = true;
                actors.quit();
            }
        }
    }
    quit
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{
        AudioInfo, InjectReport, RecordingSource, Runtime, WATCHDOG_MIN, watchdog_timeout,
    };

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        CheckArtifacts(RunId),
        StartDownload(RunId),
        LoadModel(RunId),
        StartCapture(RunId, Duration),
        StopCapture(RunId, bool),
        StartTranscription(RunId),
        AbortTranscription(RunId),
        StartInject(RunId, String),
        CopyOnly(RunId, String, CopyReason),
        UpdateTray(AppState, bool),
        Log(LogEvent),
        Quit,
        Suppressed(RunId),
    }

    #[derive(Debug, Default)]
    struct Recorder {
        calls: Vec<Call>,
        emitted: Vec<Event>,
    }

    impl Recorder {
        fn injects(&self) -> usize {
            self.calls
                .iter()
                .filter(|c| matches!(c, Call::StartInject(..) | Call::CopyOnly(..)))
                .count()
        }
    }

    impl Actors for Recorder {
        fn check_artifacts(&mut self, run: RunId) {
            self.calls.push(Call::CheckArtifacts(run));
        }
        fn start_download(&mut self, run: RunId) {
            self.calls.push(Call::StartDownload(run));
        }
        fn load_model(&mut self, run: RunId) {
            self.calls.push(Call::LoadModel(run));
        }
        fn start_capture(&mut self, run: RunId, cap: Duration) {
            self.calls.push(Call::StartCapture(run, cap));
        }
        fn stop_capture(&mut self, run: RunId, discard: bool) {
            self.calls.push(Call::StopCapture(run, discard));
        }
        fn start_transcription(&mut self, run: RunId) {
            self.calls.push(Call::StartTranscription(run));
        }
        fn abort_transcription(&mut self, run: RunId) {
            self.calls.push(Call::AbortTranscription(run));
        }
        fn start_inject(&mut self, run: RunId, text: String) {
            self.calls.push(Call::StartInject(run, text));
        }
        fn copy_only(&mut self, run: RunId, text: String, reason: CopyReason) {
            self.calls.push(Call::CopyOnly(run, text, reason));
        }
        fn update_tray(&mut self, state: AppState, paused: bool) {
            self.calls.push(Call::UpdateTray(state, paused));
        }
        fn log(&mut self, event: &LogEvent) {
            self.calls.push(Call::Log(event.clone()));
        }
        fn quit(&mut self) {
            self.calls.push(Call::Quit);
        }
        fn output_suppressed(&mut self, run: RunId) {
            self.calls.push(Call::Suppressed(run));
        }
        fn take_emitted(&mut self) -> Vec<Event> {
            std::mem::take(&mut self.emitted)
        }
    }

    /// Treibt den Kern und den Dispatcher gemeinsam — so läuft auch der Daemon.
    fn feed(
        runtime: &mut Runtime,
        timers: &mut Timers,
        rec: &mut Recorder,
        now: Instant,
        events: Vec<Event>,
    ) {
        let mut latch = QuitLatch::default();
        let mut queue: VecDeque<Event> = events.into_iter().collect();
        drive(&mut queue, runtime, timers, rec, &mut latch, now);
    }

    fn booted(now: Instant) -> (Runtime, Timers, Recorder) {
        let mut runtime = Runtime::default();
        let mut timers = Timers::default();
        let mut rec = Recorder::default();
        feed(
            &mut runtime,
            &mut timers,
            &mut rec,
            now,
            vec![
                Event::Startup,
                Event::ArtifactsChecked {
                    run: RunId(0),
                    complete: true,
                },
                Event::ModelLoaded { run: RunId(0) },
            ],
        );
        rec.calls.clear();
        (runtime, timers, rec)
    }

    /// Startsequenz: jeder Aktionseffekt landet genau einmal beim Worker.
    #[test]
    fn startup_effects_route_to_actors() {
        let now = Instant::now();
        let mut runtime = Runtime::default();
        let mut timers = Timers::default();
        let mut rec = Recorder::default();
        feed(
            &mut runtime,
            &mut timers,
            &mut rec,
            now,
            vec![
                Event::Startup,
                Event::ArtifactsChecked {
                    run: RunId(0),
                    complete: true,
                },
                Event::ModelLoaded { run: RunId(0) },
            ],
        );
        assert_eq!(
            rec.calls,
            vec![
                Call::CheckArtifacts(RunId(0)),
                Call::UpdateTray(AppState::Starting, false),
                Call::LoadModel(RunId(0)),
                Call::UpdateTray(AppState::Loading, false),
                Call::UpdateTray(AppState::Idle, false),
            ]
        );
    }

    /// Fehlende Artefakte → `StartDownload` (in 3c der verständliche Fehler).
    #[test]
    fn missing_artifacts_route_to_download() {
        let now = Instant::now();
        let mut runtime = Runtime::default();
        let mut timers = Timers::default();
        let mut rec = Recorder::default();
        feed(
            &mut runtime,
            &mut timers,
            &mut rec,
            now,
            vec![
                Event::Startup,
                Event::ArtifactsChecked {
                    run: RunId(0),
                    complete: false,
                },
            ],
        );
        assert!(rec.calls.contains(&Call::StartDownload(RunId(0))));
    }

    /// Voller PTT-Durchlauf: Worker-Aufrufe in Kern-Reihenfolge (§4.1).
    #[test]
    fn full_ptt_round_trip_routes_in_order() {
        let now = Instant::now();
        let (mut runtime, mut timers, mut rec) = booted(now);
        feed(
            &mut runtime,
            &mut timers,
            &mut rec,
            now,
            vec![Event::HotkeyPress],
        );
        let run = runtime.run;
        feed(
            &mut runtime,
            &mut timers,
            &mut rec,
            now,
            vec![
                Event::HotkeyRelease,
                Event::AudioReady {
                    run,
                    audio: AudioInfo::from_millis(2_000),
                },
                Event::TranscriptionDone {
                    run,
                    text: "Diktat".into(),
                },
                Event::InjectFinished {
                    run,
                    report: InjectReport::Pasted,
                },
            ],
        );
        assert_eq!(
            rec.calls,
            vec![
                Call::StartCapture(run, Duration::from_secs(60)),
                Call::UpdateTray(
                    AppState::Recording {
                        source: RecordingSource::Hotkey
                    },
                    false
                ),
                Call::StopCapture(run, false),
                Call::UpdateTray(
                    AppState::Transcribing {
                        source: RecordingSource::Hotkey
                    },
                    false
                ),
                Call::StartTranscription(run),
                Call::StartInject(run, "Diktat".into()),
                Call::UpdateTray(
                    AppState::Injecting {
                        source: RecordingSource::Hotkey
                    },
                    false
                ),
                Call::UpdateTray(AppState::Idle, false),
            ]
        );
        assert_eq!(timers.cap_deadline(), None, "Cap ist mit dem Stop erledigt");
        assert_eq!(timers.watchdog_deadline(), None, "Watchdog entwaffnet");
    }

    /// §4.3: Der Tray-Click-Pfad landet in `copy_only`, nie im Paste.
    #[test]
    fn tray_click_path_routes_to_copy_only() {
        let now = Instant::now();
        let (mut runtime, mut timers, mut rec) = booted(now);
        feed(
            &mut runtime,
            &mut timers,
            &mut rec,
            now,
            vec![Event::TrayClickToggle],
        );
        let run = runtime.run;
        feed(
            &mut runtime,
            &mut timers,
            &mut rec,
            now,
            vec![
                Event::TrayClickToggle,
                Event::AudioReady {
                    run,
                    audio: AudioInfo::from_millis(3_000),
                },
                Event::TranscriptionDone {
                    run,
                    text: "Klicktext".into(),
                },
            ],
        );
        assert!(
            !rec.calls.iter().any(|c| matches!(c, Call::StartInject(..))),
            "kein Paste auf dem Tray-Click-Pfad"
        );
        assert!(rec.calls.contains(&Call::CopyOnly(
            run,
            "Klicktext".into(),
            CopyReason::TrayClickPath
        )));
    }

    /// §4.4: `StartCapture` armiert den Cap-Timer auf `audio.max_duration_secs`.
    #[test]
    fn start_capture_arms_cap_timer_from_config() {
        let now = Instant::now();
        let cap = Duration::from_secs(12);
        let mut runtime = Runtime::with_cap(cap);
        let mut timers = Timers::default();
        let mut rec = Recorder::default();
        feed(
            &mut runtime,
            &mut timers,
            &mut rec,
            now,
            vec![
                Event::Startup,
                Event::ArtifactsChecked {
                    run: RunId(0),
                    complete: true,
                },
                Event::ModelLoaded { run: RunId(0) },
                Event::HotkeyPress,
            ],
        );
        let run = runtime.run;
        assert_eq!(timers.cap_deadline(), Some(now + cap));
        assert!(rec.calls.contains(&Call::StartCapture(run, cap)));

        assert!(timers.due(now + cap - Duration::from_millis(1)).is_empty());
        assert_eq!(timers.due(now + cap), vec![Event::CapReached { run }]);
        assert_eq!(
            timers.cap_deadline(),
            None,
            "ein gefeuerter Cap ist danach aus (§4.4: genau einmal)"
        );
    }

    /// §5.2: Watchdog-Frist = `max(30 s, 5 × Audiolänge)`, vom Kern berechnet.
    #[test]
    fn arm_watchdog_uses_core_timeout_and_fires_once() {
        let now = Instant::now();
        let (mut runtime, mut timers, mut rec) = booted(now);
        feed(
            &mut runtime,
            &mut timers,
            &mut rec,
            now,
            vec![Event::HotkeyPress],
        );
        let run = runtime.run;
        feed(
            &mut runtime,
            &mut timers,
            &mut rec,
            now,
            vec![
                Event::HotkeyRelease,
                Event::AudioReady {
                    run,
                    audio: AudioInfo::from_millis(20_000),
                },
            ],
        );
        let expected = watchdog_timeout(Duration::from_secs(20));
        assert_eq!(expected, Duration::from_secs(100));
        assert_eq!(timers.watchdog_deadline(), Some(now + expected));
        assert!(
            timers
                .due(now + expected - Duration::from_millis(1))
                .is_empty()
        );
        assert_eq!(
            timers.due(now + expected),
            vec![Event::WatchdogTimeout { run }]
        );
        assert_eq!(timers.watchdog_deadline(), None);
    }

    /// Kurze Aufnahmen bekommen die Untergrenze von 30 s (§5.2 / §18 #5).
    #[test]
    fn short_audio_watchdog_uses_lower_bound() {
        let now = Instant::now();
        let (mut runtime, mut timers, mut rec) = booted(now);
        feed(
            &mut runtime,
            &mut timers,
            &mut rec,
            now,
            vec![Event::HotkeyPress],
        );
        let run = runtime.run;
        feed(
            &mut runtime,
            &mut timers,
            &mut rec,
            now,
            vec![
                Event::HotkeyRelease,
                Event::AudioReady {
                    run,
                    audio: AudioInfo::from_millis(1_000),
                },
            ],
        );
        assert_eq!(timers.watchdog_deadline(), Some(now + WATCHDOG_MIN));
    }

    /// Der Watchdog löst Abbruch und Reinit aus; die Frist ist danach weg (§5.2).
    #[test]
    fn watchdog_timeout_routes_abort_and_reload() {
        let now = Instant::now();
        let (mut runtime, mut timers, mut rec) = booted(now);
        feed(
            &mut runtime,
            &mut timers,
            &mut rec,
            now,
            vec![Event::HotkeyPress],
        );
        let run = runtime.run;
        feed(
            &mut runtime,
            &mut timers,
            &mut rec,
            now,
            vec![
                Event::HotkeyRelease,
                Event::AudioReady {
                    run,
                    audio: AudioInfo::from_millis(1_000),
                },
            ],
        );
        rec.calls.clear();
        let fired = timers.due(now + WATCHDOG_MIN);
        assert_eq!(fired, vec![Event::WatchdogTimeout { run }]);
        feed(&mut runtime, &mut timers, &mut rec, now, fired);
        assert!(rec.calls.contains(&Call::AbortTranscription(run)));
        assert!(rec.calls.iter().any(|c| matches!(c, Call::LoadModel(_))));
        assert_eq!(timers.watchdog_deadline(), None);
    }

    /// Pause während `recording`: Aufnahme verworfen, Cap-Timer aus (§5.2).
    #[test]
    fn pause_during_recording_discards_and_disarms_cap() {
        let now = Instant::now();
        let (mut runtime, mut timers, mut rec) = booted(now);
        feed(
            &mut runtime,
            &mut timers,
            &mut rec,
            now,
            vec![Event::HotkeyPress],
        );
        let run = runtime.run;
        assert!(timers.cap_deadline().is_some());
        rec.calls.clear();
        feed(
            &mut runtime,
            &mut timers,
            &mut rec,
            now,
            vec![Event::PauseToggle],
        );
        assert_eq!(rec.calls[0], Call::StopCapture(run, true));
        assert_eq!(timers.cap_deadline(), None);
        assert!(rec.calls.contains(&Call::UpdateTray(AppState::Idle, true)));
    }

    /// Quit räumt den laufenden Lauf ab und meldet `Quit` als letzten Effekt.
    #[test]
    fn quit_during_recording_stops_capture_then_quits() {
        let now = Instant::now();
        let (mut runtime, mut timers, mut rec) = booted(now);
        feed(
            &mut runtime,
            &mut timers,
            &mut rec,
            now,
            vec![Event::HotkeyPress],
        );
        let run = runtime.run;
        rec.calls.clear();
        feed(
            &mut runtime,
            &mut timers,
            &mut rec,
            now,
            vec![Event::QuitRequested],
        );
        assert_eq!(rec.calls.first(), Some(&Call::StopCapture(run, true)));
        assert_eq!(rec.calls.last(), Some(&Call::Quit));
        assert_eq!(timers.cap_deadline(), None);
    }

    /// §4.4 „genau einmal": Der Wiring-Timer feuert vor dem Tick, die Kern-Uhr
    /// darf denselben Übergang danach nicht wiederholen. Die Event-Loop hält
    /// exakt diese Reihenfolge ein.
    #[test]
    fn cap_fires_once_even_when_the_core_clock_reaches_the_same_deadline() {
        let now = Instant::now();
        let cap = Duration::from_secs(5);
        let mut runtime = Runtime::with_cap(cap);
        let mut timers = Timers::default();
        let mut rec = Recorder::default();
        feed(
            &mut runtime,
            &mut timers,
            &mut rec,
            now,
            vec![
                Event::Startup,
                Event::ArtifactsChecked {
                    run: RunId(0),
                    complete: true,
                },
                Event::ModelLoaded { run: RunId(0) },
                Event::HotkeyPress,
            ],
        );
        let run = runtime.run;
        rec.calls.clear();

        // Reihenfolge der Loop: erst fällige Timer, dann der Tick.
        let fired = timers.due(now + cap);
        assert_eq!(fired, vec![Event::CapReached { run }]);
        feed(&mut runtime, &mut timers, &mut rec, now + cap, fired);
        feed(
            &mut runtime,
            &mut timers,
            &mut rec,
            now + cap,
            vec![Event::Tick { elapsed: cap }],
        );

        let stops = rec
            .calls
            .iter()
            .filter(|c| matches!(c, Call::StopCapture(..)))
            .count();
        assert_eq!(stops, 1, "genau ein Stop: {:?}", rec.calls);
        assert_eq!(
            runtime.state,
            AppState::Transcribing {
                source: RecordingSource::Hotkey
            }
        );
    }

    /// Ein neuer Lauf überschreibt die alte Frist — es gibt nie zwei Cap-Timer.
    #[test]
    fn arming_a_new_cap_replaces_the_previous_one() {
        let now = Instant::now();
        let mut timers = Timers::default();
        timers.arm_cap(RunId(1), Duration::from_secs(60), now);
        timers.arm_cap(RunId(2), Duration::from_secs(5), now);
        assert_eq!(timers.cap_deadline(), Some(now + Duration::from_secs(5)));
        assert_eq!(
            timers.due(now + Duration::from_secs(5)),
            vec![Event::CapReached { run: RunId(2) }]
        );
        assert!(timers.due(now + Duration::from_secs(120)).is_empty());
    }

    /// Beide Fristen können in derselben Runde fällig sein — Cap zuerst.
    #[test]
    fn both_timers_can_fire_in_one_round() {
        let now = Instant::now();
        let mut timers = Timers::default();
        timers.arm_cap(RunId(1), Duration::from_secs(1), now);
        timers.arm_watchdog(RunId(1), Duration::from_secs(2), now);
        let fired = timers.due(now + Duration::from_secs(3));
        assert_eq!(
            fired,
            vec![
                Event::CapReached { run: RunId(1) },
                Event::WatchdogTimeout { run: RunId(1) },
            ]
        );
        assert_eq!(timers.next_deadline(), None);
    }

    // ------------------------------------------------- Quit-Latch (codex H2)

    /// Läuft bis `transcribing` und gibt die laufende `RunId` zurück.
    fn transcribing(now: Instant) -> (Runtime, Timers, Recorder, RunId) {
        let (mut runtime, mut timers, mut rec) = booted(now);
        feed(
            &mut runtime,
            &mut timers,
            &mut rec,
            now,
            vec![Event::HotkeyPress],
        );
        let run = runtime.run;
        feed(
            &mut runtime,
            &mut timers,
            &mut rec,
            now,
            vec![
                Event::HotkeyRelease,
                Event::AudioReady {
                    run,
                    audio: AudioInfo::from_millis(2_000),
                },
            ],
        );
        rec.calls.clear();
        (runtime, timers, rec, run)
    }

    /// §5.2 „kein Inject mehr": Kommen Engine-Ergebnis und Quit in **einer**
    /// Batch, darf das Ergebnis nicht mehr injiziert werden — auch wenn es in
    /// der Reihenfolge vorne stand.
    #[test]
    fn quit_in_the_same_batch_beats_a_finished_transcription() {
        let now = Instant::now();
        let (mut runtime, mut timers, mut rec, run) = transcribing(now);
        let mut queue: VecDeque<Event> = VecDeque::new();
        let mut latch = QuitLatch::default();

        let quit = enqueue_batch(
            &mut queue,
            vec![
                Event::TranscriptionDone {
                    run,
                    text: "Diktat".into(),
                },
                Event::QuitRequested,
            ],
        );
        assert!(quit, "die Batch enthält ein Quit");
        latch.close();
        assert_eq!(
            queue.front(),
            Some(&Event::QuitRequested),
            "Quit steht vorn"
        );

        let ended = drive(
            &mut queue,
            &mut runtime,
            &mut timers,
            &mut rec,
            &mut latch,
            now,
        );
        assert!(ended, "der Kern hat Quit gemeldet");
        assert_eq!(rec.injects(), 0, "kein Inject: {:?}", rec.calls);
        assert!(rec.calls.contains(&Call::Quit));
        assert!(runtime.quitting);
    }

    /// Derselbe Fall aus der anderen Richtung: Das Signal ist gesetzt, während
    /// das Ergebnis **schon** in der Queue liegt.
    #[test]
    fn pending_transcription_is_not_injected_when_the_signal_arrived() {
        let now = Instant::now();
        let (mut runtime, mut timers, mut rec, run) = transcribing(now);
        let mut queue: VecDeque<Event> = VecDeque::new();
        let mut latch = QuitLatch::default();

        // Ergebnis wartet bereits …
        queue.push_back(Event::TranscriptionDone {
            run,
            text: "Diktat".into(),
        });
        // … dann sieht die Loop das Signal und zieht das Quit nach vorn.
        latch.close();
        queue.push_front(Event::QuitRequested);

        let ended = drive(
            &mut queue,
            &mut runtime,
            &mut timers,
            &mut rec,
            &mut latch,
            now,
        );
        assert!(ended);
        assert_eq!(rec.injects(), 0, "kein Inject: {:?}", rec.calls);
    }

    /// Der Latch allein genügt: Selbst wenn ein Ausgabeeffekt entstünde,
    /// verlässt kein Text mehr den Prozess.
    #[test]
    fn closed_latch_suppresses_output_effects() {
        let now = Instant::now();
        let mut timers = Timers::default();
        let mut rec = Recorder::default();
        let mut latch = QuitLatch::default();
        assert!(latch.close(), "erster Schluss meldet sich");
        assert!(!latch.close(), "danach nicht mehr");

        dispatch(
            vec![
                Effect::StartInject {
                    run: RunId(3),
                    text: "Diktat".into(),
                },
                Effect::CopyOnly {
                    run: RunId(3),
                    text: "Diktat".into(),
                    reason: CopyReason::TrayClickPath,
                },
            ],
            &mut timers,
            &mut rec,
            &mut latch,
            now,
        );
        assert_eq!(rec.injects(), 0);
        assert_eq!(
            rec.calls,
            vec![Call::Suppressed(RunId(3)), Call::Suppressed(RunId(3))]
        );
    }

    /// Ohne Quit bleibt alles wie gehabt — der Latch darf nichts blockieren,
    /// solange niemand beenden will.
    #[test]
    fn open_latch_lets_output_through() {
        let now = Instant::now();
        let (mut runtime, mut timers, mut rec, run) = transcribing(now);
        feed(
            &mut runtime,
            &mut timers,
            &mut rec,
            now,
            vec![Event::TranscriptionDone {
                run,
                text: "Diktat".into(),
            }],
        );
        assert!(rec.calls.contains(&Call::StartInject(run, "Diktat".into())));
    }

    /// `enqueue_batch` erhält die Reihenfolge der übrigen Events.
    #[test]
    fn batch_without_quit_keeps_its_order() {
        let mut queue: VecDeque<Event> = VecDeque::new();
        queue.push_back(Event::Tick {
            elapsed: Duration::from_millis(20),
        });
        let quit = enqueue_batch(&mut queue, vec![Event::HotkeyPress, Event::HotkeyRelease]);
        assert!(!quit);
        assert_eq!(
            queue.into_iter().collect::<Vec<_>>(),
            vec![
                Event::Tick {
                    elapsed: Duration::from_millis(20)
                },
                Event::HotkeyPress,
                Event::HotkeyRelease,
            ]
        );
    }

    /// Synchron erzeugte Events (Artefaktprüfung, Worker-Fehler) laufen im
    /// selben Durchlauf weiter — sonst bliebe die Startsequenz stehen.
    #[test]
    fn emitted_events_are_processed_in_the_same_pass() {
        let now = Instant::now();
        let mut runtime = Runtime::default();
        let mut timers = Timers::default();
        let mut rec = Recorder::default();
        let mut latch = QuitLatch::default();
        rec.emitted = vec![Event::ArtifactsChecked {
            run: RunId(0),
            complete: true,
        }];
        let mut queue: VecDeque<Event> = VecDeque::from(vec![Event::Startup]);
        drive(
            &mut queue,
            &mut runtime,
            &mut timers,
            &mut rec,
            &mut latch,
            now,
        );
        assert_eq!(runtime.state, AppState::Loading);
        assert!(rec.calls.contains(&Call::LoadModel(RunId(0))));
    }

    /// `next_deadline` weckt die Event-Loop auf die frühere der beiden Fristen.
    #[test]
    fn next_deadline_picks_the_earlier_timer() {
        let now = Instant::now();
        let mut timers = Timers::default();
        assert_eq!(timers.next_deadline(), None);
        timers.arm_watchdog(RunId(1), Duration::from_secs(30), now);
        assert_eq!(timers.next_deadline(), Some(now + Duration::from_secs(30)));
        timers.arm_cap(RunId(1), Duration::from_secs(5), now);
        assert_eq!(timers.next_deadline(), Some(now + Duration::from_secs(5)));
        timers.disarm_cap();
        assert_eq!(timers.next_deadline(), Some(now + Duration::from_secs(30)));
        timers.disarm_watchdog();
        assert_eq!(timers.next_deadline(), None);
    }
}
