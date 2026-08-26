//! Reiner State-Machine-Kern nach Spec §5.2 (plus §4.1, §4.3, §4.4, §7.3, §10).
//!
//! **Vertrag dieses Moduls:**
//!
//! - [`transition`] ist eine *pure* Funktion über `(&mut Runtime, Event) -> Vec<Effect>`:
//!   kein I/O, keine Threads, **kein `Instant::now()`**. Zeit erreicht den Kern
//!   ausschließlich über [`Event::Tick`], das die monotone Laufzeituhr
//!   [`Runtime::now`] weiterdreht.
//! - Alles, was Wirkung nach außen hat, ist ein [`Effect`]. Das Wiring (Phase 3b)
//!   führt sie aus und schickt die Antworten als Events zurück.
//! - Jede asynchrone Antwort trägt die [`RunId`] ihres Laufs. Passt sie nicht zur
//!   aktuellen Generation, wird sie verworfen (§5.2: „Ein verspätetes Ergebnis
//!   eines verworfenen Laufs wird nie injiziert.").
//!
//! **Effektreihenfolge** (verbindlich, die Tests prüfen sie):
//!
//! 1. Aufräum-/Abbrucheffekte (`StopCapture`, `AbortTranscription`, `DisarmWatchdog`)
//! 2. Log-Effekte zum Übergang
//! 3. Aktionseffekte (`StartCapture`, `StartTranscription`, `ArmWatchdog`,
//!    `StartInject`, `CopyOnly`, `CheckArtifacts`, `StartDownload`, `LoadModel`)
//! 4. `UpdateTray` — genau dann, wenn sich `(state, paused)` geändert hat
//! 5. `Quit` als allerletzter Effekt
//!
//! Wird ein Event ignoriert, ist ein `Log`-Effekt der einzige Effekt und `UpdateTray`
//! entfällt.

#![allow(dead_code)]

use std::time::Duration;

/// `audio.max_duration_secs` Default (§8). Pro `Runtime` überschreibbar.
pub const DEFAULT_CAP: Duration = Duration::from_secs(60);

/// Kürzere Aufnahmen gehen nicht in die Engine (§6.4).
pub const MIN_CAPTURE: Duration = Duration::from_millis(250);

/// Untergrenze des Transcribing-Watchdogs (§5.2 / §18 #5).
pub const WATCHDOG_MIN: Duration = Duration::from_secs(30);

/// Faktor auf die Audiolänge für den Watchdog (§5.2).
pub const WATCHDOG_FACTOR: u32 = 5;

/// `max(30 s, 5 × Audiolänge)` — Watchdog-Frist einer Transkription (§5.2).
pub fn watchdog_timeout(audio: Duration) -> Duration {
    let scaled = audio.saturating_mul(WATCHDOG_FACTOR);
    if scaled > WATCHDOG_MIN {
        scaled
    } else {
        WATCHDOG_MIN
    }
}

/// Generation eines asynchronen Kontexts: ein Diktat (Aufnahme → Transkription →
/// Inject) oder eine Startsequenz. Wird ein Lauf verworfen, zählt der Kern hoch;
/// verspätete Antworten des alten Laufs sind damit erkennbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct RunId(pub u64);

impl RunId {
    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

/// Quelle einer laufenden oder gerade beendeten Aufnahme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingSource {
    Hotkey,
    TrayClick,
}

/// Prozesszustand ohne das orthogonale `paused`-Flag.
///
/// `Injecting` steht in §5.2 nicht als eigene Zeile — die Spec schreibt
/// „transcribing + Text + Fokus gleich → inject → idle". Weil der Paste-Pfad
/// nichtblockierend werden muss (codex H4 zu §7.1 P6), braucht der Kern einen
/// eigenen Zustand zwischen `StartInject`/`CopyOnly` und `InjectFinished`.
/// Sichtbar bleibt er `transcribing` (§4.3 kennt keinen Tray-Zustand „injecting").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppState {
    Starting,
    Downloading,
    Loading,
    Idle,
    Recording { source: RecordingSource },
    Transcribing { source: RecordingSource },
    Injecting { source: RecordingSource },
    Error,
}

/// Fehlerklassen aus §10 (plus §4.4, §6.2, §6.4, §7.1). `AppState::Error` allein
/// sagt nichts über die Bedienbarkeit — §4.3 nennt „error" ausdrücklich
/// „fatal oder bedienbar, siehe §10".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// TOML-Syntax, ungültiger Hotkey/Modellschlüssel (§8-Tabelle, §6.2).
    Config,
    /// Download-, Größen- oder Hashfehler der Artefakte (§6.3).
    ModelDownload,
    /// ORT-Init bzw. Modell laden gescheitert (§6.1).
    ModelLoad,
    /// Hotkey-Registrierung gescheitert (§4.4) — Tray-Click bleibt bedienbar.
    HotkeyRegistration,
    /// Tray-Aufbau gescheitert (§10) — das Wiring beendet den Prozess.
    Tray,
    /// Mikrofon tot, Aufnahme startete nicht (§6.4) — nächster Press versucht neu.
    Mic,
    /// Einzelne Inferenz gescheitert — nächster Press versucht neu.
    Engine,
    /// Watchdog in `transcribing` hat zugeschlagen (§5.2).
    TranscriptionStuck,
    /// Paste-API/UIPI gescheitert (§7.1) — Transkript bleibt im Clipboard.
    Inject,
}

impl ErrorKind {
    /// §10: bleibt der Hotkey in diesem Fehlerzustand scharf?
    pub fn hotkey_armed(self) -> bool {
        matches!(
            self,
            Self::Mic | Self::Engine | Self::TranscriptionStuck | Self::Inject
        )
    }

    /// §10 / §4.4: bleibt der Tray-Linksklick in diesem Fehlerzustand bedienbar?
    pub fn tray_click_armed(self) -> bool {
        self.hotkey_armed() || matches!(self, Self::HotkeyRegistration)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::ModelDownload => "model-download",
            Self::ModelLoad => "model-load",
            Self::HotkeyRegistration => "hotkey-registration",
            Self::Tray => "tray",
            Self::Mic => "mic",
            Self::Engine => "engine",
            Self::TranscriptionStuck => "transcription-stuck",
            Self::Inject => "inject",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorInfo {
    pub kind: ErrorKind,
    pub message: String,
}

/// Laufzeitstatus: Zustand plus orthogonales Pause-Flag (Hotkey aus) plus die
/// Buchführung, die der Kern für Generationen und Fristen braucht.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    pub state: AppState,
    /// §5.2: orthogonal zum Zustand. Hotkey aus, Tray-Click bleibt aktiv (§4.3).
    pub paused: bool,
    /// Generation des laufenden asynchronen Kontexts.
    pub run: RunId,
    /// Modell geladen und benutzbar (§5.2: „`idle` heißt: Modell geladen, bereit").
    pub model_ready: bool,
    /// Grund des aktuellen `Error`-Zustands.
    pub error: Option<ErrorInfo>,
    /// Beenden angefordert (§5.2) — der Kern nimmt keine Arbeit mehr an.
    pub quitting: bool,
    /// Monotone Laufzeituhr. Wächst **nur** durch [`Event::Tick`].
    pub now: Duration,
    /// `audio.max_duration_secs` (§8).
    pub cap: Duration,
    /// Fällig-Zeitpunkt des 60-s-Caps, solange aufgenommen wird.
    pub cap_deadline: Option<Duration>,
    /// Fällig-Zeitpunkt des Transcribing-Watchdogs, solange transkribiert wird.
    pub watchdog_deadline: Option<Duration>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            state: AppState::Starting,
            paused: false,
            run: RunId(0),
            model_ready: false,
            error: None,
            quitting: false,
            now: Duration::ZERO,
            cap: DEFAULT_CAP,
            cap_deadline: None,
            watchdog_deadline: None,
        }
    }
}

impl Runtime {
    /// Startzustand mit konfiguriertem Cap (§8 `audio.max_duration_secs`).
    pub fn with_cap(cap: Duration) -> Self {
        Self {
            cap,
            ..Self::default()
        }
    }

    /// §5.2: Press wird nur aus `idle` (bzw. einem bedienbaren Fehler) angenommen
    /// und nur, wenn nicht pausiert ist.
    pub fn hotkey_armed(&self) -> bool {
        if self.paused || self.quitting || !self.model_ready {
            return false;
        }
        match self.state {
            AppState::Idle => true,
            AppState::Error => self.error.as_ref().is_some_and(|e| e.kind.hotkey_armed()),
            _ => false,
        }
    }

    /// Steht der `Error`-Zustand auf genau dieser Klasse?
    pub fn error_is(&self, kind: ErrorKind) -> bool {
        self.error.as_ref().is_some_and(|e| e.kind == kind)
    }

    /// Ist die Frist mit dem aktuellen Uhrenstand fällig? Einzige Zeitprüfung
    /// des Kerns — `now` wächst nur über [`Event::Tick`].
    pub fn due(&self, deadline: Option<Duration>) -> bool {
        deadline.is_some_and(|at| self.now >= at)
    }

    /// §4.3: Der Tray-Linksklick bleibt auch bei `paused` und bei Hotkey-Fehlern aktiv.
    pub fn tray_click_armed(&self) -> bool {
        if self.quitting || !self.model_ready {
            return false;
        }
        match self.state {
            AppState::Idle => true,
            AppState::Recording {
                source: RecordingSource::TrayClick,
            } => true,
            AppState::Error => self
                .error
                .as_ref()
                .is_some_and(|e| e.kind.tray_click_armed()),
            _ => false,
        }
    }
}

/// Was der Kern über eine fertige Aufnahme wissen muss — nicht die Samples selbst.
/// Die bleiben beim Worker (`audio::CapturedAudio`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioInfo {
    pub duration: Duration,
}

impl AudioInfo {
    pub fn from_millis(millis: u64) -> Self {
        Self {
            duration: Duration::from_millis(millis),
        }
    }
}

/// Warum ein Transkript nur ins Clipboard ging statt eingefügt zu werden.
/// Kern-Abstraktion über `inject::CopyOnlyReason` (§4.3, §7.1, §7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyReason {
    /// §4.3: TrayClick-Diktate enden **immer** in `copy_only`.
    TrayClickPath,
    /// §7.3: Start-/Ende-/Vordergrundkennung stimmen nicht überein.
    FocusChanged,
    /// §7.3: Kennung nicht ermittelbar (NULL, Secure Desktop, gesperrt).
    FocusUnknown,
}

/// Ergebnis des Ausgabepfads. Kern-Abstraktion über `inject::InjectOutcome`
/// plus dem Fehlerfall aus §7.1 („Paste-API-Fehler oder UIPI").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InjectReport {
    /// Paste am Cursor gelaufen (Restore-Details interessieren den Kern nicht).
    Pasted,
    /// Transkript liegt im Clipboard, kein Paste-Key gesendet.
    CopyOnly { reason: CopyReason },
    /// §7.1: Transkript bleibt im Clipboard, Tray `error`, Hotkey bleibt scharf.
    Failed { message: String },
}

/// Strukturierte Logzeilen. Kein freier Text, damit Tests exakt prüfen können.
/// Nie Transkripte, Clipboard-Inhalte oder Fenstertitel (§10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogEvent {
    /// §5.2: „Press außerhalb `idle`: ignorieren, Log."
    IgnoredPress {
        state: AppState,
    },
    IgnoredRelease {
        state: AppState,
    },
    /// §4.3: Linksklick in `recording(Hotkey)`, `transcribing`, `downloading`, `loading`.
    IgnoredTrayClick {
        state: AppState,
    },
    /// §5.2: Press bei `paused`.
    IgnoredWhilePaused,
    /// Antwort eines verworfenen Laufs (§5.2 Watchdog-Regel).
    StaleRun {
        what: &'static str,
        got: RunId,
        current: RunId,
    },
    /// §6.4: `< 250 ms` wird nicht transkribiert.
    AudioTooShort {
        millis: u64,
    },
    /// §4.1 Punkt 5: leeres Transkript, kein Inject, kein Fehlerdialog.
    EmptyTranscript,
    /// §5.2: Pause während `recording` verwirft die Aufnahme.
    RecordingDiscarded,
    /// §4.3 / §7.3: Tooltip-Anlass für den `copy_only`-Ausgang.
    CopyOnlyNotice {
        reason: CopyReason,
    },
    /// §10: Fehlerklasse betreten.
    Failure {
        kind: ErrorKind,
    },
    /// „Retry" außerhalb von `error` — es gibt nichts zu wiederholen (§5.2).
    IgnoredRetry {
        state: AppState,
    },
    /// §5.2: Beenden während laufender Arbeit.
    QuitRequested {
        state: AppState,
    },
    /// Nach `Quit` nimmt der Kern nichts mehr an.
    IgnoredAfterQuit,
}

/// Alles, was von außen in den Kern zeigt. Asynchrone Antworten tragen die
/// [`RunId`] ihres Laufs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Prozess hochgefahren — stößt die Startsequenz `starting → …` an (§5.2).
    Startup,
    /// Ergebnis der Artefaktprüfung: `complete` überspringt `downloading` (§5.2, §6.3).
    ArtifactsChecked {
        run: RunId,
        complete: bool,
    },
    DownloadFinished {
        run: RunId,
    },
    DownloadFailed {
        run: RunId,
        message: String,
    },
    ModelLoaded {
        run: RunId,
    },
    ModelLoadFailed {
        run: RunId,
        message: String,
    },
    /// Entprelltes logisches Press (§4.4 — Auto-Repeat filtert das Backend).
    HotkeyPress,
    /// Entprelltes logisches Release.
    HotkeyRelease,
    /// Tray-Linksklick: startet bzw. stoppt die Toggle-Aufnahme (§4.3).
    TrayClickToggle,
    /// Menü „Hotkey pausieren / wieder aktivieren" (§4.3).
    PauseToggle,
    /// Der Capture-Worker hat `audio.max_duration_secs` erreicht (§4.4).
    /// Zweiter Auslöser desselben Übergangs neben der Cap-Deadline in [`Event::Tick`].
    CapReached {
        run: RunId,
    },
    /// Mikrofon/Gerät tot (§6.4, §10).
    CaptureFailed {
        run: RunId,
        message: String,
    },
    /// Aufnahme liegt vor (Länge; Samples bleiben beim Worker).
    AudioReady {
        run: RunId,
        audio: AudioInfo,
    },
    TranscriptionDone {
        run: RunId,
        text: String,
    },
    TranscriptionFailed {
        run: RunId,
        message: String,
    },
    /// Externer Watchdog-Timer abgelaufen (§5.2). Zweiter Auslöser desselben
    /// Übergangs neben der Watchdog-Deadline in [`Event::Tick`].
    WatchdogTimeout {
        run: RunId,
    },
    InjectFinished {
        run: RunId,
        report: InjectReport,
    },
    /// Fataler Fehler von außen (Config, Hotkey-Registrierung, Tray) — §8, §4.4, §10.
    FatalError {
        kind: ErrorKind,
        message: String,
    },
    /// „Retry/Neustart" aus §5.2 — zurück nach `starting`.
    RetryRequested,
    /// Tray-Menü „Beenden" (§4.3, §5.2).
    QuitRequested,
    /// Einzige Zeitquelle des Kerns. `elapsed` ist der Fortschritt seit dem letzten Tick.
    Tick {
        elapsed: Duration,
    },
}

/// Alles, was der Kern nach außen anordnet. Das Wiring führt sie aus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// §6.3: Artefakte prüfen und `ArtifactsChecked` zurückschicken.
    CheckArtifacts {
        run: RunId,
    },
    StartDownload {
        run: RunId,
    },
    /// ORT initialisieren und Modell laden (§6.1) — auch Reinit nach Watchdog (§5.2).
    LoadModel {
        run: RunId,
    },
    /// Aufnahme starten; `cap` ist die Frist aus §8. §7.3: Das Wiring merkt sich hier
    /// `CaptureContext.start_window_id`.
    StartCapture {
        run: RunId,
        cap: Duration,
    },
    /// Aufnahme beenden. `discard: true` verwirft sie ersatzlos (§5.2 Pause-Regel).
    /// §7.3: Das Wiring merkt sich hier `CaptureContext.target_window_id`.
    StopCapture {
        run: RunId,
        discard: bool,
    },
    StartTranscription {
        run: RunId,
    },
    /// Laufende Inferenz verwerfen und Engine neu initialisieren (§5.2 Watchdog).
    AbortTranscription {
        run: RunId,
    },
    /// Paste am Cursor (§7.1). Die Fokusprüfung nach §7.3 macht die Inject-Schicht.
    StartInject {
        run: RunId,
        text: String,
    },
    /// Nur ins Clipboard, kein Paste-Key (§4.3 TrayClick-Pfad).
    CopyOnly {
        run: RunId,
        text: String,
        reason: CopyReason,
    },
    /// Sichtbaren Zustand nachziehen (§4.3). Das Mapping macht `tray::tray_status`.
    UpdateTray {
        state: AppState,
        paused: bool,
    },
    /// Watchdog-Frist der laufenden Transkription (§5.2).
    ArmWatchdog {
        run: RunId,
        timeout: Duration,
    },
    DisarmWatchdog,
    Log(LogEvent),
    /// Prozess beenden (§5.2: hartes Ende nach 5 s ist Sache des Wirings).
    Quit,
}

/// Der Übergang. Pur: kein I/O, keine Zeitabfrage, keine Threads.
pub fn transition(runtime: &mut Runtime, event: Event) -> Vec<Effect> {
    // §5.2: Nach dem Quit nimmt der Kern nichts mehr an — auch kein Ergebnis
    // eines noch laufenden Workers („kein Inject mehr").
    if runtime.quitting {
        return vec![Effect::Log(LogEvent::IgnoredAfterQuit)];
    }

    let before = (runtime.state, runtime.paused);
    let mut out = Vec::new();
    // `starting` malt den Tray auch ohne Zustandswechsel — es ist der erste Anstrich.
    let mut force_tray = false;
    let mut quit = false;

    match event {
        // ------------------------------------------- Startsequenz (§5.2, §6.3)
        Event::Startup => {
            runtime.state = AppState::Starting;
            runtime.error = None;
            out.push(Effect::CheckArtifacts { run: runtime.run });
            force_tray = true;
        }

        Event::ArtifactsChecked { run, complete } => match runtime.state {
            AppState::Starting if run == runtime.run => {
                if complete {
                    runtime.state = AppState::Loading;
                    out.push(Effect::LoadModel { run });
                } else {
                    runtime.state = AppState::Downloading;
                    out.push(Effect::StartDownload { run });
                }
            }
            _ => stale(&mut out, "artifacts-checked", run, runtime.run),
        },

        Event::DownloadFinished { run } => match runtime.state {
            AppState::Downloading if run == runtime.run => {
                runtime.state = AppState::Loading;
                out.push(Effect::LoadModel { run });
            }
            _ => stale(&mut out, "download-finished", run, runtime.run),
        },

        Event::DownloadFailed { run, message } => match runtime.state {
            AppState::Downloading if run == runtime.run => {
                enter_error(runtime, ErrorKind::ModelDownload, message, &mut out);
            }
            _ => stale(&mut out, "download-failed", run, runtime.run),
        },

        Event::ModelLoaded { run } => match runtime.state {
            AppState::Loading if run == runtime.run => {
                runtime.model_ready = true;
                runtime.error = None;
                runtime.state = AppState::Idle;
            }
            // §5.2: Nach dem Watchdog läuft der Reinit im Hintergrund. Der
            // Tray bleibt auf `error`, geheilt wird erst der nächste Press.
            AppState::Error
                if run == runtime.run && runtime.error_is(ErrorKind::TranscriptionStuck) =>
            {
                runtime.model_ready = true;
            }
            _ => stale(&mut out, "model-loaded", run, runtime.run),
        },

        Event::ModelLoadFailed { run, message } => match runtime.state {
            AppState::Loading | AppState::Error if run == runtime.run => {
                runtime.model_ready = false;
                enter_error(runtime, ErrorKind::ModelLoad, message, &mut out);
            }
            _ => stale(&mut out, "model-load-failed", run, runtime.run),
        },

        // ------------------------------------------------ Hotkey (§4.1, §4.4)
        Event::HotkeyPress => {
            if runtime.paused {
                // §5.2: `paused` heißt Hotkey aus — der Tray-Click bleibt.
                out.push(Effect::Log(LogEvent::IgnoredWhilePaused));
            } else if runtime.hotkey_armed() {
                start_recording(runtime, RecordingSource::Hotkey, &mut out);
            } else {
                out.push(Effect::Log(LogEvent::IgnoredPress {
                    state: runtime.state,
                }));
            }
        }

        Event::HotkeyRelease => match runtime.state {
            AppState::Recording {
                source: source @ RecordingSource::Hotkey,
            } => stop_recording(runtime, source, &mut out),
            // §4.4: verlorenes Release nach dem Cap, Release in `recording(TrayClick)`.
            _ => out.push(Effect::Log(LogEvent::IgnoredRelease {
                state: runtime.state,
            })),
        },

        // -------------------------------------------------- Tray-Click (§4.3)
        Event::TrayClickToggle => match runtime.state {
            AppState::Recording {
                source: source @ RecordingSource::TrayClick,
            } => stop_recording(runtime, source, &mut out),
            _ if runtime.tray_click_armed() => {
                start_recording(runtime, RecordingSource::TrayClick, &mut out);
            }
            _ => out.push(Effect::Log(LogEvent::IgnoredTrayClick {
                state: runtime.state,
            })),
        },

        // ------------------------------------------------------- Pause (§5.2)
        Event::PauseToggle => {
            runtime.paused = !runtime.paused;
            // Pause **aktivieren** verwirft eine laufende Aufnahme; das Aufheben
            // der Pause lässt eine Tray-Click-Aufnahme laufen.
            if runtime.paused && matches!(runtime.state, AppState::Recording { .. }) {
                out.push(Effect::StopCapture {
                    run: runtime.run,
                    discard: true,
                });
                out.push(Effect::Log(LogEvent::RecordingDiscarded));
                runtime.cap_deadline = None;
                runtime.run = runtime.run.next();
                runtime.state = AppState::Idle;
            }
        }

        // ------------------------------------------------ Aufnahme (§4.4, §6.4)
        Event::CapReached { run } => match runtime.state {
            AppState::Recording { source } if run == runtime.run => {
                stop_recording(runtime, source, &mut out);
            }
            // §5.2: „genau einmal nach `transcribing`" — ein zweiter Cap-Report
            // trifft einen Zustand, der ihn nicht mehr annimmt.
            _ => stale(&mut out, "cap-reached", run, runtime.run),
        },

        Event::CaptureFailed { run, message } => match runtime.state {
            AppState::Recording { .. } | AppState::Transcribing { .. } if run == runtime.run => {
                disarm_watchdog(runtime, &mut out);
                runtime.cap_deadline = None;
                enter_error(runtime, ErrorKind::Mic, message, &mut out);
            }
            _ => stale(&mut out, "capture-failed", run, runtime.run),
        },

        Event::AudioReady { run, audio } => match runtime.state {
            AppState::Transcribing { .. } if run == runtime.run => {
                if audio.duration < MIN_CAPTURE {
                    // §6.4: zu kurze Buffer gehen nicht in die Engine.
                    out.push(Effect::Log(LogEvent::AudioTooShort {
                        millis: audio.duration.as_millis() as u64,
                    }));
                    finish_run(runtime);
                } else {
                    let timeout = watchdog_timeout(audio.duration);
                    runtime.watchdog_deadline = Some(runtime.now + timeout);
                    out.push(Effect::StartTranscription { run });
                    out.push(Effect::ArmWatchdog { run, timeout });
                }
            }
            _ => stale(&mut out, "audio-ready", run, runtime.run),
        },

        // ------------------------------------------ Transkription (§4.1, §5.2)
        Event::TranscriptionDone { run, text } => match runtime.state {
            AppState::Transcribing { source } if run == runtime.run => {
                disarm_watchdog(runtime, &mut out);
                if text.trim().is_empty() {
                    // §4.1 Punkt 5: nichts einfügen, kein Fehlerdialog.
                    out.push(Effect::Log(LogEvent::EmptyTranscript));
                    finish_run(runtime);
                } else {
                    match source {
                        RecordingSource::Hotkey => out.push(Effect::StartInject { run, text }),
                        // §4.3 / §18 #7: TrayClick endet immer in `copy_only`.
                        RecordingSource::TrayClick => out.push(Effect::CopyOnly {
                            run,
                            text,
                            reason: CopyReason::TrayClickPath,
                        }),
                    }
                    runtime.state = AppState::Injecting { source };
                }
            }
            _ => stale(&mut out, "transcription-done", run, runtime.run),
        },

        Event::TranscriptionFailed { run, message } => match runtime.state {
            AppState::Transcribing { .. } if run == runtime.run => {
                disarm_watchdog(runtime, &mut out);
                enter_error(runtime, ErrorKind::Engine, message, &mut out);
            }
            _ => stale(&mut out, "transcription-failed", run, runtime.run),
        },

        Event::WatchdogTimeout { run } => match runtime.state {
            AppState::Transcribing { .. } if run == runtime.run => {
                fire_watchdog(runtime, run, &mut out);
            }
            _ => stale(&mut out, "watchdog-timeout", run, runtime.run),
        },

        // ----------------------------------------------- Ausgabepfad (§7)
        Event::InjectFinished { run, report } => match runtime.state {
            AppState::Injecting { .. } if run == runtime.run => match report {
                InjectReport::Pasted => finish_run(runtime),
                InjectReport::CopyOnly { reason } => {
                    out.push(Effect::Log(LogEvent::CopyOnlyNotice { reason }));
                    finish_run(runtime);
                }
                // §7.1: Transkript bleibt im Clipboard, Tray `error`,
                // §10: Hotkey bleibt scharf, Retry ist das nächste Diktat.
                InjectReport::Failed { message } => {
                    enter_error(runtime, ErrorKind::Inject, message, &mut out);
                }
            },
            _ => stale(&mut out, "inject-finished", run, runtime.run),
        },

        // ------------------------------------------------ Fehler, Retry, Quit
        Event::FatalError { kind, message } => {
            cleanup_active_run(runtime, &mut out);
            enter_error(runtime, kind, message, &mut out);
        }

        Event::RetryRequested => match runtime.state {
            AppState::Error => {
                runtime.run = runtime.run.next();
                runtime.error = None;
                runtime.state = AppState::Starting;
                out.push(Effect::CheckArtifacts { run: runtime.run });
                force_tray = true;
            }
            _ => out.push(Effect::Log(LogEvent::IgnoredRetry {
                state: runtime.state,
            })),
        },

        Event::QuitRequested => {
            cleanup_active_run(runtime, &mut out);
            out.push(Effect::Log(LogEvent::QuitRequested {
                state: runtime.state,
            }));
            runtime.run = runtime.run.next();
            runtime.cap_deadline = None;
            runtime.watchdog_deadline = None;
            runtime.quitting = true;
            quit = true;
        }

        // ------------------------------------------------------ Zeit (§5.2)
        Event::Tick { elapsed } => {
            runtime.now += elapsed;
            match runtime.state {
                AppState::Recording { source } if runtime.due(runtime.cap_deadline) => {
                    stop_recording(runtime, source, &mut out);
                }
                AppState::Transcribing { .. } if runtime.due(runtime.watchdog_deadline) => {
                    let run = runtime.run;
                    fire_watchdog(runtime, run, &mut out);
                }
                _ => {}
            }
        }
    }

    if force_tray || (runtime.state, runtime.paused) != before {
        out.push(Effect::UpdateTray {
            state: runtime.state,
            paused: runtime.paused,
        });
    }
    if quit {
        out.push(Effect::Quit);
    }
    out
}

fn stale(out: &mut Vec<Effect>, what: &'static str, got: RunId, current: RunId) {
    out.push(Effect::Log(LogEvent::StaleRun { what, got, current }));
}

/// §4.1 Schritt 1 / §7.3: neuer Lauf, Cap-Frist scharf, Capture an.
fn start_recording(runtime: &mut Runtime, source: RecordingSource, out: &mut Vec<Effect>) {
    runtime.run = runtime.run.next();
    runtime.error = None;
    runtime.state = AppState::Recording { source };
    runtime.cap_deadline = Some(runtime.now + runtime.cap);
    out.push(Effect::StartCapture {
        run: runtime.run,
        cap: runtime.cap,
    });
}

/// §5.2: `recording → transcribing`. Die Samples liefert der Worker später als
/// `AudioReady` nach; erst dann startet die Inferenz.
fn stop_recording(runtime: &mut Runtime, source: RecordingSource, out: &mut Vec<Effect>) {
    runtime.cap_deadline = None;
    runtime.state = AppState::Transcribing { source };
    out.push(Effect::StopCapture {
        run: runtime.run,
        discard: false,
    });
}

/// Lauf regulär abgeschlossen: zurück nach `idle`, neue Generation.
fn finish_run(runtime: &mut Runtime) {
    runtime.run = runtime.run.next();
    runtime.cap_deadline = None;
    runtime.watchdog_deadline = None;
    runtime.state = AppState::Idle;
}

fn disarm_watchdog(runtime: &mut Runtime, out: &mut Vec<Effect>) {
    if runtime.watchdog_deadline.take().is_some() {
        out.push(Effect::DisarmWatchdog);
    }
}

/// §5.2: Lauf verwerfen, Engine neu initialisieren, `error`. Der Retry ist der
/// nächste Press — ein verspätetes Ergebnis trägt dann eine tote Generation.
fn fire_watchdog(runtime: &mut Runtime, run: RunId, out: &mut Vec<Effect>) {
    out.push(Effect::AbortTranscription { run });
    disarm_watchdog(runtime, out);
    enter_error(
        runtime,
        ErrorKind::TranscriptionStuck,
        "Transkription hängt".into(),
        out,
    );
    runtime.model_ready = false;
    out.push(Effect::LoadModel { run: runtime.run });
}

/// Aufräumen vor einem fatalen Fehler oder dem Quit — je nach laufender Arbeit.
fn cleanup_active_run(runtime: &mut Runtime, out: &mut Vec<Effect>) {
    match runtime.state {
        AppState::Recording { .. } => out.push(Effect::StopCapture {
            run: runtime.run,
            discard: true,
        }),
        AppState::Transcribing { .. } => {
            out.push(Effect::AbortTranscription { run: runtime.run });
            disarm_watchdog(runtime, out);
        }
        _ => {}
    }
}

/// §10: Fehlerklasse betreten. Was danach noch bedienbar ist, entscheidet
/// [`ErrorKind::hotkey_armed`] / [`ErrorKind::tray_click_armed`].
fn enter_error(runtime: &mut Runtime, kind: ErrorKind, message: String, out: &mut Vec<Effect>) {
    out.push(Effect::Log(LogEvent::Failure { kind }));
    runtime.run = runtime.run.next();
    runtime.cap_deadline = None;
    runtime.watchdog_deadline = None;
    runtime.error = Some(ErrorInfo { kind, message });
    runtime.state = AppState::Error;
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------- Helfer

    fn tray(state: AppState, paused: bool) -> Effect {
        Effect::UpdateTray { state, paused }
    }

    fn hotkey_rec() -> AppState {
        AppState::Recording {
            source: RecordingSource::Hotkey,
        }
    }

    fn tray_rec() -> AppState {
        AppState::Recording {
            source: RecordingSource::TrayClick,
        }
    }

    fn hotkey_trans() -> AppState {
        AppState::Transcribing {
            source: RecordingSource::Hotkey,
        }
    }

    fn tray_trans() -> AppState {
        AppState::Transcribing {
            source: RecordingSource::TrayClick,
        }
    }

    /// Führt die Events der Reihe nach aus und gibt die Effekte des **letzten** zurück.
    fn feed(rt: &mut Runtime, events: Vec<Event>) -> Vec<Effect> {
        let mut last = Vec::new();
        for event in events {
            last = transition(rt, event);
        }
        last
    }

    /// `starting → loading → idle`, Modell geladen, nichts pausiert.
    fn booted() -> Runtime {
        let mut rt = Runtime::default();
        feed(
            &mut rt,
            vec![
                Event::Startup,
                Event::ArtifactsChecked {
                    run: RunId(0),
                    complete: true,
                },
                Event::ModelLoaded { run: RunId(0) },
            ],
        );
        rt
    }

    /// Läuft bis `recording(Hotkey)`.
    fn recording_hotkey() -> Runtime {
        let mut rt = booted();
        feed(&mut rt, vec![Event::HotkeyPress]);
        rt
    }

    /// Läuft bis `recording(TrayClick)`.
    fn recording_tray() -> Runtime {
        let mut rt = booted();
        feed(&mut rt, vec![Event::TrayClickToggle]);
        rt
    }

    /// Läuft bis `transcribing(source)` mit laufender Inferenz über `audio_ms` Audio.
    fn transcribing(source: RecordingSource, audio_ms: u64) -> Runtime {
        let mut rt = match source {
            RecordingSource::Hotkey => recording_hotkey(),
            RecordingSource::TrayClick => recording_tray(),
        };
        let run = rt.run;
        let stop = match source {
            RecordingSource::Hotkey => Event::HotkeyRelease,
            RecordingSource::TrayClick => Event::TrayClickToggle,
        };
        feed(
            &mut rt,
            vec![
                stop,
                Event::AudioReady {
                    run,
                    audio: AudioInfo::from_millis(audio_ms),
                },
            ],
        );
        rt
    }

    fn has_update_tray(effects: &[Effect]) -> bool {
        effects
            .iter()
            .any(|e| matches!(e, Effect::UpdateTray { .. }))
    }

    // ------------------------------------------- A. Startsequenz (§5.2-Kette)

    /// 1 — `Event::Startup` stößt die Artefaktprüfung an und malt `starting`.
    #[test]
    fn startup_checks_artifacts_and_paints_starting() {
        let mut rt = Runtime::default();
        let fx = transition(&mut rt, Event::Startup);
        assert_eq!(rt.state, AppState::Starting);
        assert_eq!(
            fx,
            vec![
                Effect::CheckArtifacts { run: rt.run },
                tray(AppState::Starting, false),
            ]
        );
    }

    /// 2 — Vollständige Artefakte überspringen `downloading` (§5.2 „downloading?").
    #[test]
    fn complete_artifacts_skip_downloading_and_load_model() {
        let mut rt = Runtime::default();
        let fx = feed(
            &mut rt,
            vec![
                Event::Startup,
                Event::ArtifactsChecked {
                    run: RunId(0),
                    complete: true,
                },
            ],
        );
        assert_eq!(rt.state, AppState::Loading);
        assert_eq!(
            fx,
            vec![
                Effect::LoadModel { run: rt.run },
                tray(AppState::Loading, false),
            ]
        );
    }

    /// 3 — Fehlende Artefakte führen nach `downloading` (§5.2, §6.3).
    #[test]
    fn missing_artifacts_enter_downloading() {
        let mut rt = Runtime::default();
        let fx = feed(
            &mut rt,
            vec![
                Event::Startup,
                Event::ArtifactsChecked {
                    run: RunId(0),
                    complete: false,
                },
            ],
        );
        assert_eq!(rt.state, AppState::Downloading);
        assert_eq!(
            fx,
            vec![
                Effect::StartDownload { run: rt.run },
                tray(AppState::Downloading, false),
            ]
        );
    }

    /// 4 — `downloading → loading` nach erfolgreichem Download.
    #[test]
    fn download_finished_enters_loading() {
        let mut rt = Runtime::default();
        let fx = feed(
            &mut rt,
            vec![
                Event::Startup,
                Event::ArtifactsChecked {
                    run: RunId(0),
                    complete: false,
                },
                Event::DownloadFinished { run: RunId(0) },
            ],
        );
        assert_eq!(rt.state, AppState::Loading);
        assert_eq!(
            fx,
            vec![
                Effect::LoadModel { run: rt.run },
                tray(AppState::Loading, false),
            ]
        );
    }

    /// 5 — `loading → idle`; erst jetzt gilt das Modell als bereit (§5.2).
    #[test]
    fn model_loaded_enters_idle_and_marks_model_ready() {
        let mut rt = Runtime::default();
        let fx = feed(
            &mut rt,
            vec![
                Event::Startup,
                Event::ArtifactsChecked {
                    run: RunId(0),
                    complete: true,
                },
                Event::ModelLoaded { run: RunId(0) },
            ],
        );
        assert_eq!(rt.state, AppState::Idle);
        assert!(rt.model_ready);
        assert!(rt.hotkey_armed());
        assert_eq!(fx, vec![tray(AppState::Idle, false)]);
    }

    /// 6 — Download-/Hashfehler ist fatal: `error`, Hotkey aus (§6.3, §10).
    #[test]
    fn download_failure_is_fatal_error_without_hotkey() {
        let mut rt = Runtime::default();
        let fx = feed(
            &mut rt,
            vec![
                Event::Startup,
                Event::ArtifactsChecked {
                    run: RunId(0),
                    complete: false,
                },
                Event::DownloadFailed {
                    run: RunId(0),
                    message: "sha256 mismatch".into(),
                },
            ],
        );
        assert_eq!(rt.state, AppState::Error);
        assert_eq!(
            rt.error.as_ref().map(|e| e.kind),
            Some(ErrorKind::ModelDownload)
        );
        assert!(!rt.hotkey_armed());
        assert!(!rt.tray_click_armed());
        assert_eq!(
            fx,
            vec![
                Effect::Log(LogEvent::Failure {
                    kind: ErrorKind::ModelDownload
                }),
                tray(AppState::Error, false),
            ]
        );
    }

    /// 7 — ORT-/Modellfehler ist fatal: `error`, Hotkey aus (§6.1, §10).
    #[test]
    fn model_load_failure_is_fatal_error_without_hotkey() {
        let mut rt = Runtime::default();
        let fx = feed(
            &mut rt,
            vec![
                Event::Startup,
                Event::ArtifactsChecked {
                    run: RunId(0),
                    complete: true,
                },
                Event::ModelLoadFailed {
                    run: RunId(0),
                    message: "ort init".into(),
                },
            ],
        );
        assert_eq!(rt.state, AppState::Error);
        assert_eq!(
            rt.error.as_ref().map(|e| e.kind),
            Some(ErrorKind::ModelLoad)
        );
        assert!(!rt.hotkey_armed());
        assert!(!rt.model_ready);
        assert!(has_update_tray(&fx));
    }

    /// 8 — Antwort eines alten Startlaufs wird verworfen (Generation).
    #[test]
    fn stale_startup_answer_is_ignored() {
        let mut rt = booted();
        let current = rt.run;
        let stale = RunId(current.0 + 7);
        let fx = transition(&mut rt, Event::ModelLoaded { run: stale });
        assert_eq!(rt.state, AppState::Idle);
        assert_eq!(
            fx,
            vec![Effect::Log(LogEvent::StaleRun {
                what: "model-loaded",
                got: stale,
                current,
            })]
        );
    }

    // --------------------------------------------------- B. PTT-Kern (§4.1)

    /// 9 — Press aus `idle` startet die Aufnahme mit neuer Generation (§5.2).
    #[test]
    fn idle_press_starts_hotkey_recording() {
        let mut rt = booted();
        let before = rt.run;
        let fx = transition(&mut rt, Event::HotkeyPress);
        assert_eq!(rt.state, hotkey_rec());
        assert!(rt.run > before, "Aufnahmestart eröffnet einen neuen Lauf");
        assert_eq!(rt.cap_deadline, Some(rt.now + rt.cap));
        assert_eq!(
            fx,
            vec![
                Effect::StartCapture {
                    run: rt.run,
                    cap: rt.cap
                },
                tray(hotkey_rec(), false),
            ]
        );
    }

    /// 10 — Release beendet die Aufnahme und geht nach `transcribing` (§5.2).
    #[test]
    fn hotkey_release_stops_capture_and_enters_transcribing() {
        let mut rt = recording_hotkey();
        let run = rt.run;
        let fx = transition(&mut rt, Event::HotkeyRelease);
        assert_eq!(rt.state, hotkey_trans());
        assert_eq!(rt.cap_deadline, None, "Cap ist mit dem Stop erledigt");
        assert_eq!(
            fx,
            vec![
                Effect::StopCapture {
                    run,
                    discard: false
                },
                tray(hotkey_trans(), false),
            ]
        );
    }

    /// 11 — Erst die fertige Aufnahme startet die Inferenz und armiert den Watchdog.
    #[test]
    fn audio_ready_starts_transcription_and_arms_watchdog() {
        let mut rt = recording_hotkey();
        let run = rt.run;
        let fx = feed(
            &mut rt,
            vec![
                Event::HotkeyRelease,
                Event::AudioReady {
                    run,
                    audio: AudioInfo::from_millis(4_000),
                },
            ],
        );
        assert_eq!(rt.state, hotkey_trans());
        assert_eq!(
            fx,
            vec![
                Effect::StartTranscription { run },
                Effect::ArmWatchdog {
                    run,
                    timeout: WATCHDOG_MIN,
                },
            ],
            "kein UpdateTray — der Zustand ändert sich nicht"
        );
    }

    /// 12 — Transkript aus dem Hotkey-Pfad geht in den Paste-Pfad (§5.2, §7.1).
    #[test]
    fn transcription_done_injects_on_hotkey_path() {
        let mut rt = transcribing(RecordingSource::Hotkey, 4_000);
        let run = rt.run;
        let fx = transition(
            &mut rt,
            Event::TranscriptionDone {
                run,
                text: "Hallo Welt".into(),
            },
        );
        assert_eq!(
            rt.state,
            AppState::Injecting {
                source: RecordingSource::Hotkey
            }
        );
        assert_eq!(rt.watchdog_deadline, None);
        assert_eq!(
            fx,
            vec![
                Effect::DisarmWatchdog,
                Effect::StartInject {
                    run,
                    text: "Hallo Welt".into()
                },
                tray(
                    AppState::Injecting {
                        source: RecordingSource::Hotkey
                    },
                    false
                ),
            ]
        );
    }

    /// 13 — Nach erfolgreichem Paste zurück nach `idle` (§4.1 Punkt 4).
    #[test]
    fn inject_finished_returns_to_idle() {
        let mut rt = transcribing(RecordingSource::Hotkey, 4_000);
        let run = rt.run;
        let fx = feed(
            &mut rt,
            vec![
                Event::TranscriptionDone {
                    run,
                    text: "Hallo".into(),
                },
                Event::InjectFinished {
                    run,
                    report: InjectReport::Pasted,
                },
            ],
        );
        assert_eq!(rt.state, AppState::Idle);
        assert_eq!(fx, vec![tray(AppState::Idle, false)]);
    }

    /// 14 — Ganzer PTT-Durchlauf: Effektfolge und Endzustand (§4.1).
    #[test]
    fn full_ptt_round_trip_effect_order() {
        let mut rt = booted();
        let mut all = Vec::new();
        all.extend(transition(&mut rt, Event::HotkeyPress));
        let run = rt.run;
        all.extend(transition(&mut rt, Event::HotkeyRelease));
        all.extend(transition(
            &mut rt,
            Event::AudioReady {
                run,
                audio: AudioInfo::from_millis(2_000),
            },
        ));
        all.extend(transition(
            &mut rt,
            Event::TranscriptionDone {
                run,
                text: "Diktat".into(),
            },
        ));
        all.extend(transition(
            &mut rt,
            Event::InjectFinished {
                run,
                report: InjectReport::Pasted,
            },
        ));
        assert_eq!(rt.state, AppState::Idle);
        assert_eq!(
            all,
            vec![
                Effect::StartCapture {
                    run,
                    cap: DEFAULT_CAP
                },
                tray(hotkey_rec(), false),
                Effect::StopCapture {
                    run,
                    discard: false
                },
                tray(hotkey_trans(), false),
                Effect::StartTranscription { run },
                Effect::ArmWatchdog {
                    run,
                    timeout: WATCHDOG_MIN
                },
                Effect::DisarmWatchdog,
                Effect::StartInject {
                    run,
                    text: "Diktat".into()
                },
                tray(
                    AppState::Injecting {
                        source: RecordingSource::Hotkey
                    },
                    false
                ),
                tray(AppState::Idle, false),
            ]
        );
    }

    // ------------------------------------------------- C. Tray-Pfad (§4.3)

    /// 15 — Linksklick aus `idle` startet die Toggle-Aufnahme mit Quelle `TrayClick`.
    #[test]
    fn idle_tray_click_starts_trayclick_recording() {
        let mut rt = booted();
        let fx = transition(&mut rt, Event::TrayClickToggle);
        assert_eq!(rt.state, tray_rec());
        assert_eq!(
            fx,
            vec![
                Effect::StartCapture {
                    run: rt.run,
                    cap: rt.cap
                },
                tray(tray_rec(), false),
            ]
        );
    }

    /// 16 — Zweiter Linksklick stoppt und transkribiert (§4.3).
    #[test]
    fn second_tray_click_stops_and_transcribes() {
        let mut rt = recording_tray();
        let run = rt.run;
        let fx = transition(&mut rt, Event::TrayClickToggle);
        assert_eq!(rt.state, tray_trans());
        assert_eq!(
            fx,
            vec![
                Effect::StopCapture {
                    run,
                    discard: false
                },
                tray(tray_trans(), false),
            ]
        );
    }

    /// 17 — In `recording(TrayClick)` werden F9-Press und -Release ignoriert (§4.3).
    #[test]
    fn tray_recording_ignores_hotkey_press_and_release() {
        let mut rt = recording_tray();
        let press = transition(&mut rt, Event::HotkeyPress);
        assert_eq!(rt.state, tray_rec());
        assert_eq!(
            press,
            vec![Effect::Log(LogEvent::IgnoredPress { state: tray_rec() })]
        );
        let release = transition(&mut rt, Event::HotkeyRelease);
        assert_eq!(rt.state, tray_rec(), "Stop nur per Klick oder Cap");
        assert_eq!(
            release,
            vec![Effect::Log(LogEvent::IgnoredRelease { state: tray_rec() })]
        );
    }

    /// 18 — In `recording(Hotkey)` wird der Linksklick ignoriert (§4.3).
    #[test]
    fn hotkey_recording_ignores_tray_click() {
        let mut rt = recording_hotkey();
        let fx = transition(&mut rt, Event::TrayClickToggle);
        assert_eq!(rt.state, hotkey_rec());
        assert_eq!(
            fx,
            vec![Effect::Log(LogEvent::IgnoredTrayClick {
                state: hotkey_rec()
            })]
        );
    }

    /// 19 — TrayClick-Diktate enden **immer** in `copy_only`, nie im Paste (§4.3, §18 #7).
    #[test]
    fn trayclick_transcription_copies_only_and_never_pastes() {
        let mut rt = transcribing(RecordingSource::TrayClick, 3_000);
        let run = rt.run;
        let fx = transition(
            &mut rt,
            Event::TranscriptionDone {
                run,
                text: "Klicktext".into(),
            },
        );
        assert!(
            !fx.iter().any(|e| matches!(e, Effect::StartInject { .. })),
            "kein Paste-Key auf dem TrayClick-Pfad"
        );
        assert_eq!(
            fx,
            vec![
                Effect::DisarmWatchdog,
                Effect::CopyOnly {
                    run,
                    text: "Klicktext".into(),
                    reason: CopyReason::TrayClickPath,
                },
                tray(
                    AppState::Injecting {
                        source: RecordingSource::TrayClick
                    },
                    false
                ),
            ]
        );
    }

    /// 20 — Linksklick während `transcribing` wird ignoriert (§4.3).
    #[test]
    fn tray_click_ignored_while_transcribing() {
        let mut rt = transcribing(RecordingSource::Hotkey, 3_000);
        let fx = transition(&mut rt, Event::TrayClickToggle);
        assert_eq!(rt.state, hotkey_trans());
        assert_eq!(
            fx,
            vec![Effect::Log(LogEvent::IgnoredTrayClick {
                state: hotkey_trans()
            })]
        );
    }

    /// 21 — Linksklick während `downloading`/`loading` wird ignoriert (§4.3, §5).
    #[test]
    fn tray_click_ignored_while_downloading_and_loading() {
        let mut downloading = Runtime::default();
        feed(
            &mut downloading,
            vec![
                Event::Startup,
                Event::ArtifactsChecked {
                    run: RunId(0),
                    complete: false,
                },
            ],
        );
        let fx = transition(&mut downloading, Event::TrayClickToggle);
        assert_eq!(downloading.state, AppState::Downloading);
        assert_eq!(
            fx,
            vec![Effect::Log(LogEvent::IgnoredTrayClick {
                state: AppState::Downloading
            })]
        );

        let mut loading = Runtime::default();
        feed(
            &mut loading,
            vec![
                Event::Startup,
                Event::ArtifactsChecked {
                    run: RunId(0),
                    complete: true,
                },
            ],
        );
        let fx = transition(&mut loading, Event::TrayClickToggle);
        assert_eq!(loading.state, AppState::Loading);
        assert_eq!(
            fx,
            vec![Effect::Log(LogEvent::IgnoredTrayClick {
                state: AppState::Loading
            })]
        );
    }

    // ------------------------------------- D. Press außerhalb idle, Pause

    /// 22 — Press in `loading` und `downloading` startet keine Aufnahme (§5).
    #[test]
    fn press_before_idle_is_ignored_with_log() {
        let mut loading = Runtime::default();
        feed(
            &mut loading,
            vec![
                Event::Startup,
                Event::ArtifactsChecked {
                    run: RunId(0),
                    complete: true,
                },
            ],
        );
        let fx = transition(&mut loading, Event::HotkeyPress);
        assert_eq!(loading.state, AppState::Loading);
        assert_eq!(
            fx,
            vec![Effect::Log(LogEvent::IgnoredPress {
                state: AppState::Loading
            })]
        );

        let mut downloading = Runtime::default();
        feed(
            &mut downloading,
            vec![
                Event::Startup,
                Event::ArtifactsChecked {
                    run: RunId(0),
                    complete: false,
                },
            ],
        );
        let fx = transition(&mut downloading, Event::HotkeyPress);
        assert_eq!(downloading.state, AppState::Downloading);
        assert_eq!(
            fx,
            vec![Effect::Log(LogEvent::IgnoredPress {
                state: AppState::Downloading
            })]
        );
    }

    /// 23 — Press während `transcribing` und `injecting` wird ignoriert (§5.2, §13).
    #[test]
    fn press_during_transcribing_and_injecting_is_ignored() {
        let mut rt = transcribing(RecordingSource::Hotkey, 3_000);
        let fx = transition(&mut rt, Event::HotkeyPress);
        assert_eq!(rt.state, hotkey_trans());
        assert_eq!(
            fx,
            vec![Effect::Log(LogEvent::IgnoredPress {
                state: hotkey_trans()
            })]
        );

        let run = rt.run;
        feed(
            &mut rt,
            vec![Event::TranscriptionDone {
                run,
                text: "Text".into(),
            }],
        );
        let injecting = AppState::Injecting {
            source: RecordingSource::Hotkey,
        };
        let fx = transition(&mut rt, Event::HotkeyPress);
        assert_eq!(rt.state, injecting);
        assert_eq!(
            fx,
            vec![Effect::Log(LogEvent::IgnoredPress { state: injecting })]
        );
    }

    /// 24 — Press bei `paused` wird ignoriert; der Zustand bleibt `idle` (§5.2).
    #[test]
    fn press_while_paused_is_ignored() {
        let mut rt = booted();
        feed(&mut rt, vec![Event::PauseToggle]);
        assert!(rt.paused);
        let fx = transition(&mut rt, Event::HotkeyPress);
        assert_eq!(rt.state, AppState::Idle);
        assert_eq!(fx, vec![Effect::Log(LogEvent::IgnoredWhilePaused)]);
    }

    /// 25 — Der Tray-Linksklick bleibt bei `paused` bedienbar (§4.3-Tabelle).
    #[test]
    fn tray_click_still_records_while_paused() {
        let mut rt = booted();
        feed(&mut rt, vec![Event::PauseToggle]);
        let fx = transition(&mut rt, Event::TrayClickToggle);
        assert_eq!(rt.state, tray_rec());
        assert!(rt.paused, "Pause bleibt orthogonal bestehen");
        assert_eq!(
            fx,
            vec![
                Effect::StartCapture {
                    run: rt.run,
                    cap: rt.cap
                },
                tray(tray_rec(), true),
            ]
        );
    }

    /// 26 — `paused` ist in jedem Zustand togglebar und ändert den Zustand nicht (§5.2).
    #[test]
    fn pause_toggle_is_orthogonal_in_every_state() {
        let mut starting = Runtime::default();
        feed(&mut starting, vec![Event::Startup]);
        for rt in [
            &mut starting,
            &mut booted(),
            &mut transcribing(RecordingSource::Hotkey, 3_000),
        ] {
            let before = rt.state;
            let fx = transition(rt, Event::PauseToggle);
            assert_eq!(rt.state, before, "Pause ändert den Zustand nicht");
            assert!(rt.paused);
            assert_eq!(fx, vec![tray(before, true)]);
            let fx = transition(rt, Event::PauseToggle);
            assert!(!rt.paused);
            assert_eq!(fx, vec![tray(before, false)]);
        }
    }

    /// 27 — Pause während `recording` verwirft die Aufnahme: kein `StartTranscription` (§5.2).
    #[test]
    fn pause_during_recording_discards_the_take() {
        let mut rt = recording_hotkey();
        let run = rt.run;
        let fx = transition(&mut rt, Event::PauseToggle);
        assert_eq!(rt.state, AppState::Idle);
        assert!(rt.paused);
        assert_eq!(rt.cap_deadline, None);
        assert!(
            rt.run > run,
            "verworfener Lauf bekommt eine neue Generation"
        );
        assert_eq!(
            fx,
            vec![
                Effect::StopCapture { run, discard: true },
                Effect::Log(LogEvent::RecordingDiscarded),
                tray(AppState::Idle, true),
            ]
        );
        assert!(
            !fx.iter()
                .any(|e| matches!(e, Effect::StartTranscription { .. })),
            "verworfene Aufnahme wird nie transkribiert"
        );
    }

    /// 28 — Die Aufnahme des verworfenen Laufs kommt zu spät und wird ignoriert.
    #[test]
    fn late_audio_of_paused_run_is_ignored() {
        let mut rt = recording_hotkey();
        let run = rt.run;
        feed(&mut rt, vec![Event::PauseToggle]);
        let current = rt.run;
        let fx = transition(
            &mut rt,
            Event::AudioReady {
                run,
                audio: AudioInfo::from_millis(3_000),
            },
        );
        assert_eq!(rt.state, AppState::Idle);
        assert_eq!(
            fx,
            vec![Effect::Log(LogEvent::StaleRun {
                what: "audio-ready",
                got: run,
                current,
            })]
        );
    }

    /// 29 — Nach dem Aufheben der Pause greift der Hotkey wieder (§4.3).
    #[test]
    fn resume_from_pause_rearms_the_hotkey() {
        let mut rt = booted();
        feed(&mut rt, vec![Event::PauseToggle, Event::PauseToggle]);
        assert!(!rt.paused);
        let fx = transition(&mut rt, Event::HotkeyPress);
        assert_eq!(rt.state, hotkey_rec());
        assert!(has_update_tray(&fx));
    }

    /// 30 — Doppeltes Press in `recording` wird ignoriert (Entprellung liegt im
    /// Hotkey-Backend, §4.4 — der Kern sieht nur logische Events).
    #[test]
    fn duplicate_press_in_recording_is_ignored() {
        let mut rt = recording_hotkey();
        let run = rt.run;
        let fx = transition(&mut rt, Event::HotkeyPress);
        assert_eq!(rt.state, hotkey_rec());
        assert_eq!(rt.run, run, "kein neuer Lauf");
        assert_eq!(
            fx,
            vec![Effect::Log(LogEvent::IgnoredPress {
                state: hotkey_rec()
            })]
        );
    }

    /// 31 — Release ohne laufende Aufnahme wird ignoriert (§4.4 verlorenes Release).
    #[test]
    fn release_without_recording_is_ignored() {
        let mut rt = booted();
        let fx = transition(&mut rt, Event::HotkeyRelease);
        assert_eq!(rt.state, AppState::Idle);
        assert_eq!(
            fx,
            vec![Effect::Log(LogEvent::IgnoredRelease {
                state: AppState::Idle
            })]
        );
    }

    // ----------------------------------- E. Cap und verlorenes Release (§4.4)

    /// 32 — `CapReached` beendet die Aufnahme wie ein Release (§5.2).
    #[test]
    fn cap_reached_event_moves_to_transcribing() {
        let mut rt = recording_hotkey();
        let run = rt.run;
        let fx = transition(&mut rt, Event::CapReached { run });
        assert_eq!(rt.state, hotkey_trans());
        assert_eq!(
            fx,
            vec![
                Effect::StopCapture {
                    run,
                    discard: false
                },
                tray(hotkey_trans(), false),
            ]
        );
    }

    /// 33 — Die Cap-Deadline der Kern-Uhr löst denselben Übergang aus (`Tick`).
    #[test]
    fn tick_reaching_cap_deadline_stops_recording() {
        let mut rt = recording_hotkey();
        let run = rt.run;
        let fx = transition(
            &mut rt,
            Event::Tick {
                elapsed: DEFAULT_CAP,
            },
        );
        assert_eq!(rt.now, DEFAULT_CAP, "Zeit kommt nur über Tick");
        assert_eq!(rt.state, hotkey_trans());
        assert_eq!(
            fx,
            vec![
                Effect::StopCapture {
                    run,
                    discard: false
                },
                tray(hotkey_trans(), false),
            ]
        );
    }

    /// 34 — Ein Tick vor der Deadline bewirkt nichts außer Zeitfortschritt.
    #[test]
    fn tick_before_cap_deadline_does_nothing() {
        let mut rt = recording_hotkey();
        let fx = transition(
            &mut rt,
            Event::Tick {
                elapsed: Duration::from_secs(59),
            },
        );
        assert_eq!(rt.state, hotkey_rec());
        assert_eq!(rt.now, Duration::from_secs(59));
        assert!(fx.is_empty());
    }

    /// 35 — Der Cap zieht genau einmal: ein zweiter Cap-Report läuft ins Leere (§5.2).
    #[test]
    fn cap_fires_exactly_once_even_when_reported_twice() {
        let mut rt = recording_hotkey();
        let run = rt.run;
        transition(&mut rt, Event::CapReached { run });
        let fx = transition(&mut rt, Event::CapReached { run });
        assert_eq!(rt.state, hotkey_trans());
        assert_eq!(
            fx,
            vec![Effect::Log(LogEvent::StaleRun {
                what: "cap-reached",
                got: run,
                current: run,
            })],
            "der Lauf ist derselbe, der Zustand nimmt den Cap aber nicht mehr an"
        );
    }

    /// 36 — Spätes Release nach dem Cap wird ignoriert (§4.4, §5.2).
    #[test]
    fn late_release_after_cap_is_ignored() {
        let mut rt = recording_hotkey();
        let run = rt.run;
        transition(&mut rt, Event::CapReached { run });
        let fx = transition(&mut rt, Event::HotkeyRelease);
        assert_eq!(rt.state, hotkey_trans());
        assert_eq!(
            fx,
            vec![Effect::Log(LogEvent::IgnoredRelease {
                state: hotkey_trans()
            })]
        );
    }

    /// 37 — Verlorenes Release bei gesperrtem Desktop: genau **eine** Transkription
    /// über den ganzen Ablauf (§4.4).
    #[test]
    fn lost_release_transcribes_exactly_once() {
        let mut rt = recording_hotkey();
        let run = rt.run;
        let mut all = Vec::new();
        all.extend(transition(
            &mut rt,
            Event::Tick {
                elapsed: DEFAULT_CAP,
            },
        ));
        all.extend(transition(
            &mut rt,
            Event::AudioReady {
                run,
                audio: AudioInfo::from_millis(60_000),
            },
        ));
        all.extend(transition(&mut rt, Event::HotkeyRelease));
        all.extend(transition(
            &mut rt,
            Event::Tick {
                elapsed: Duration::from_secs(1),
            },
        ));
        let starts = all
            .iter()
            .filter(|e| matches!(e, Effect::StartTranscription { .. }))
            .count();
        assert_eq!(starts, 1, "genau eine Transkription");
        let stops = all
            .iter()
            .filter(|e| matches!(e, Effect::StopCapture { .. }))
            .count();
        assert_eq!(stops, 1, "genau ein Aufnahmestop");
    }

    /// 38 — In `recording(TrayClick)` zieht der Cap ebenfalls (§4.3).
    #[test]
    fn cap_also_stops_trayclick_recording() {
        let mut rt = recording_tray();
        let run = rt.run;
        let fx = transition(
            &mut rt,
            Event::Tick {
                elapsed: DEFAULT_CAP,
            },
        );
        assert_eq!(rt.state, tray_trans());
        assert_eq!(
            fx,
            vec![
                Effect::StopCapture {
                    run,
                    discard: false
                },
                tray(tray_trans(), false),
            ]
        );
    }

    /// 39 — Der Cap kommt aus der Config, nicht aus einer Konstanten (§8).
    #[test]
    fn configured_cap_is_used_for_capture_and_deadline() {
        let cap = Duration::from_secs(10);
        let mut rt = Runtime::with_cap(cap);
        feed(
            &mut rt,
            vec![
                Event::Startup,
                Event::ArtifactsChecked {
                    run: RunId(0),
                    complete: true,
                },
                Event::ModelLoaded { run: RunId(0) },
            ],
        );
        let fx = transition(&mut rt, Event::HotkeyPress);
        assert_eq!(fx.first(), Some(&Effect::StartCapture { run: rt.run, cap }));
        assert_eq!(rt.cap_deadline, Some(cap));
        let fx = transition(&mut rt, Event::Tick { elapsed: cap });
        assert_eq!(rt.state, hotkey_trans());
        assert!(has_update_tray(&fx));
    }

    // -------------------------------- F. Watchdog und Lauf-Generation (§5.2)

    /// 40 — Kurzes Audio: der Watchdog steht auf der 30-s-Untergrenze (§18 #5).
    #[test]
    fn watchdog_uses_thirty_second_floor_for_short_audio() {
        let mut rt = recording_hotkey();
        let run = rt.run;
        let fx = feed(
            &mut rt,
            vec![
                Event::HotkeyRelease,
                Event::AudioReady {
                    run,
                    audio: AudioInfo::from_millis(2_000),
                },
            ],
        );
        assert_eq!(
            fx.last(),
            Some(&Effect::ArmWatchdog {
                run,
                timeout: Duration::from_secs(30)
            })
        );
        assert_eq!(rt.watchdog_deadline, Some(rt.now + Duration::from_secs(30)));
    }

    /// 41 — Langes Audio skaliert die Frist auf `5 × Audiolänge` (§5.2).
    #[test]
    fn watchdog_scales_with_five_times_audio_length() {
        let mut rt = recording_hotkey();
        let run = rt.run;
        let fx = feed(
            &mut rt,
            vec![
                Event::HotkeyRelease,
                Event::AudioReady {
                    run,
                    audio: AudioInfo::from_millis(20_000),
                },
            ],
        );
        assert_eq!(
            fx.last(),
            Some(&Effect::ArmWatchdog {
                run,
                timeout: Duration::from_secs(100)
            })
        );
    }

    /// 42 — `WatchdogTimeout` verwirft den Lauf, initialisiert die Engine neu und
    /// geht nach `error` (§5.2).
    #[test]
    fn watchdog_timeout_aborts_run_and_enters_error() {
        let mut rt = transcribing(RecordingSource::Hotkey, 4_000);
        let run = rt.run;
        let fx = transition(&mut rt, Event::WatchdogTimeout { run });
        assert_eq!(rt.state, AppState::Error);
        assert_eq!(
            rt.error.as_ref().map(|e| e.kind),
            Some(ErrorKind::TranscriptionStuck)
        );
        assert!(!rt.model_ready, "Engine wird neu initialisiert");
        assert!(rt.run > run, "der hängende Lauf ist verworfen");
        assert_eq!(rt.watchdog_deadline, None);
        assert_eq!(
            fx,
            vec![
                Effect::AbortTranscription { run },
                Effect::DisarmWatchdog,
                Effect::Log(LogEvent::Failure {
                    kind: ErrorKind::TranscriptionStuck
                }),
                Effect::LoadModel { run: rt.run },
                tray(AppState::Error, false),
            ]
        );
    }

    /// 43 — Dieselbe Wirkung über die Watchdog-Deadline der Kern-Uhr (`Tick`).
    #[test]
    fn watchdog_timeout_via_tick_deadline() {
        let mut rt = transcribing(RecordingSource::Hotkey, 4_000);
        let run = rt.run;
        let fx = transition(
            &mut rt,
            Event::Tick {
                elapsed: Duration::from_secs(30),
            },
        );
        assert_eq!(rt.state, AppState::Error);
        assert_eq!(
            rt.error.as_ref().map(|e| e.kind),
            Some(ErrorKind::TranscriptionStuck)
        );
        assert_eq!(fx.first(), Some(&Effect::AbortTranscription { run }));
    }

    /// 44 — Das verspätete Ergebnis eines verworfenen Laufs wird **nie** injiziert (§5.2).
    #[test]
    fn late_result_of_discarded_run_is_never_injected() {
        let mut rt = transcribing(RecordingSource::Hotkey, 4_000);
        let stale = rt.run;
        transition(&mut rt, Event::WatchdogTimeout { run: stale });
        let current = rt.run;
        let fx = transition(
            &mut rt,
            Event::TranscriptionDone {
                run: stale,
                text: "zu spät".into(),
            },
        );
        assert!(
            !fx.iter()
                .any(|e| matches!(e, Effect::StartInject { .. } | Effect::CopyOnly { .. })),
            "kein Inject aus einem verworfenen Lauf"
        );
        assert_eq!(rt.state, AppState::Error);
        assert_eq!(
            fx,
            vec![Effect::Log(LogEvent::StaleRun {
                what: "transcription-done",
                got: stale,
                current,
            })]
        );
    }

    /// 45 — Der Watchdog feuert nach dem Abschluss der Transkription nicht mehr.
    #[test]
    fn watchdog_disarmed_after_transcription_completed() {
        let mut rt = transcribing(RecordingSource::Hotkey, 4_000);
        let run = rt.run;
        feed(
            &mut rt,
            vec![
                Event::TranscriptionDone {
                    run,
                    text: "fertig".into(),
                },
                Event::InjectFinished {
                    run,
                    report: InjectReport::Pasted,
                },
            ],
        );
        assert_eq!(rt.watchdog_deadline, None);
        let fx = transition(
            &mut rt,
            Event::Tick {
                elapsed: Duration::from_secs(600),
            },
        );
        assert_eq!(rt.state, AppState::Idle);
        assert!(fx.is_empty());
    }

    /// 46 — Nach dem Watchdog bleibt `error` stehen, bis die Engine neu geladen ist
    /// und der nächste Press kommt (§5.2 „Retry beim nächsten Press").
    #[test]
    fn press_after_watchdog_error_needs_reloaded_engine() {
        let mut rt = transcribing(RecordingSource::Hotkey, 4_000);
        let run = rt.run;
        transition(&mut rt, Event::WatchdogTimeout { run });
        let reload = rt.run;

        let fx = transition(&mut rt, Event::HotkeyPress);
        assert_eq!(rt.state, AppState::Error, "Engine noch nicht bereit");
        assert_eq!(
            fx,
            vec![Effect::Log(LogEvent::IgnoredPress {
                state: AppState::Error
            })]
        );

        transition(&mut rt, Event::ModelLoaded { run: reload });
        assert_eq!(rt.state, AppState::Error, "Tray bleibt auf error (§5.2)");
        assert!(rt.model_ready);

        let fx = transition(&mut rt, Event::HotkeyPress);
        assert_eq!(rt.state, hotkey_rec(), "nächster Press heilt den Fehler");
        assert_eq!(rt.error, None);
        assert!(has_update_tray(&fx));
    }

    // ------------------------- G. Leeres Transkript und zu kurze Aufnahme

    /// 47 — Leeres Transkript: kein Inject, zurück nach `idle` (§4.1 Punkt 5, §5.2).
    #[test]
    fn empty_transcript_does_not_inject() {
        let mut rt = transcribing(RecordingSource::Hotkey, 3_000);
        let run = rt.run;
        let fx = transition(
            &mut rt,
            Event::TranscriptionDone {
                run,
                text: String::new(),
            },
        );
        assert_eq!(rt.state, AppState::Idle);
        assert_eq!(
            fx,
            vec![
                Effect::DisarmWatchdog,
                Effect::Log(LogEvent::EmptyTranscript),
                tray(AppState::Idle, false),
            ]
        );
    }

    /// 48 — Nur Whitespace zählt als leer (§4.1 „nach Normalisierung leer").
    #[test]
    fn whitespace_only_transcript_counts_as_empty() {
        let mut rt = transcribing(RecordingSource::TrayClick, 3_000);
        let run = rt.run;
        let fx = transition(
            &mut rt,
            Event::TranscriptionDone {
                run,
                text: "  \n\t ".into(),
            },
        );
        assert_eq!(rt.state, AppState::Idle);
        assert!(
            !fx.iter().any(|e| matches!(e, Effect::CopyOnly { .. })),
            "auch der TrayClick-Pfad kopiert kein leeres Transkript"
        );
    }

    /// 49 — Aufnahmen unter 250 ms gehen nicht in die Engine (§6.4).
    #[test]
    fn audio_shorter_than_minimum_is_not_transcribed() {
        let mut rt = recording_hotkey();
        let run = rt.run;
        let fx = feed(
            &mut rt,
            vec![
                Event::HotkeyRelease,
                Event::AudioReady {
                    run,
                    audio: AudioInfo::from_millis(120),
                },
            ],
        );
        assert_eq!(rt.state, AppState::Idle);
        assert_eq!(
            fx,
            vec![
                Effect::Log(LogEvent::AudioTooShort { millis: 120 }),
                tray(AppState::Idle, false),
            ]
        );
        assert_eq!(rt.watchdog_deadline, None, "kein Watchdog ohne Inferenz");
    }

    // ---------------------------------- H. Ergebnisse des Ausgabepfads (§7)

    /// 50 — Fokuswechsel endet in `copy_only` und `idle` (§5.2, §7.3).
    #[test]
    fn focus_change_ends_in_copy_only_and_idle() {
        let mut rt = transcribing(RecordingSource::Hotkey, 3_000);
        let run = rt.run;
        let fx = feed(
            &mut rt,
            vec![
                Event::TranscriptionDone {
                    run,
                    text: "Text".into(),
                },
                Event::InjectFinished {
                    run,
                    report: InjectReport::CopyOnly {
                        reason: CopyReason::FocusChanged,
                    },
                },
            ],
        );
        assert_eq!(rt.state, AppState::Idle);
        assert_eq!(rt.error, None, "Fokusverlust ist kein Fehlerzustand");
        assert_eq!(
            fx,
            vec![
                Effect::Log(LogEvent::CopyOnlyNotice {
                    reason: CopyReason::FocusChanged
                }),
                tray(AppState::Idle, false),
            ]
        );
    }

    /// 51 — Nicht ermittelbare Fensterkennung zählt als Fokusverlust (§7.3, §18 #4).
    #[test]
    fn unknown_focus_ends_in_copy_only_and_idle() {
        let mut rt = transcribing(RecordingSource::Hotkey, 3_000);
        let run = rt.run;
        let fx = feed(
            &mut rt,
            vec![
                Event::TranscriptionDone {
                    run,
                    text: "Text".into(),
                },
                Event::InjectFinished {
                    run,
                    report: InjectReport::CopyOnly {
                        reason: CopyReason::FocusUnknown,
                    },
                },
            ],
        );
        assert_eq!(rt.state, AppState::Idle);
        assert_eq!(
            fx.first(),
            Some(&Effect::Log(LogEvent::CopyOnlyNotice {
                reason: CopyReason::FocusUnknown
            }))
        );
    }

    /// 52 — TrayClick-`copy_only` beendet den Lauf regulär in `idle` (§4.3).
    #[test]
    fn trayclick_copy_only_finishes_in_idle() {
        let mut rt = transcribing(RecordingSource::TrayClick, 3_000);
        let run = rt.run;
        let fx = feed(
            &mut rt,
            vec![
                Event::TranscriptionDone {
                    run,
                    text: "Klicktext".into(),
                },
                Event::InjectFinished {
                    run,
                    report: InjectReport::CopyOnly {
                        reason: CopyReason::TrayClickPath,
                    },
                },
            ],
        );
        assert_eq!(rt.state, AppState::Idle);
        assert_eq!(rt.error, None);
        assert!(has_update_tray(&fx));
    }

    /// 53 — Paste-API-/UIPI-Fehler: Tray `error`, Transkript bleibt im Clipboard,
    /// Hotkey bleibt scharf (§7.1, §10-Zeile „Inject/UIPI/Fokus").
    #[test]
    fn inject_failure_keeps_transcript_and_stays_operable() {
        let mut rt = transcribing(RecordingSource::Hotkey, 3_000);
        let run = rt.run;
        let fx = feed(
            &mut rt,
            vec![
                Event::TranscriptionDone {
                    run,
                    text: "Text".into(),
                },
                Event::InjectFinished {
                    run,
                    report: InjectReport::Failed {
                        message: "SendInput blockiert".into(),
                    },
                },
            ],
        );
        assert_eq!(rt.state, AppState::Error);
        assert_eq!(rt.error.as_ref().map(|e| e.kind), Some(ErrorKind::Inject));
        assert!(rt.hotkey_armed(), "§10: Hotkey bleibt an");
        assert_eq!(
            fx,
            vec![
                Effect::Log(LogEvent::Failure {
                    kind: ErrorKind::Inject
                }),
                tray(AppState::Error, false),
            ]
        );
    }

    /// 54 — Das nächste Diktat läuft nach einem Injectfehler normal an (§10 „Retry:
    /// nächstes Diktat").
    #[test]
    fn press_after_inject_error_starts_new_recording() {
        let mut rt = transcribing(RecordingSource::Hotkey, 3_000);
        let run = rt.run;
        feed(
            &mut rt,
            vec![
                Event::TranscriptionDone {
                    run,
                    text: "Text".into(),
                },
                Event::InjectFinished {
                    run,
                    report: InjectReport::Failed {
                        message: "UIPI".into(),
                    },
                },
            ],
        );
        let fx = transition(&mut rt, Event::HotkeyPress);
        assert_eq!(rt.state, hotkey_rec());
        assert_eq!(rt.error, None);
        assert_eq!(
            fx,
            vec![
                Effect::StartCapture {
                    run: rt.run,
                    cap: rt.cap
                },
                tray(hotkey_rec(), false),
            ]
        );
    }

    // -------------------------------------------- I. Fehlerklassen aus §10

    /// 55 — Totes Mikrofon: Aufnahme startet nicht, Hotkey bleibt an (§6.4, §10).
    #[test]
    fn mic_failure_aborts_recording_but_keeps_hotkey_armed() {
        let mut rt = recording_hotkey();
        let run = rt.run;
        let fx = transition(
            &mut rt,
            Event::CaptureFailed {
                run,
                message: "device lost".into(),
            },
        );
        assert_eq!(rt.state, AppState::Error);
        assert_eq!(rt.error.as_ref().map(|e| e.kind), Some(ErrorKind::Mic));
        assert!(rt.hotkey_armed());
        assert_eq!(rt.cap_deadline, None);
        assert!(rt.run > run);
        assert_eq!(
            fx,
            vec![
                Effect::Log(LogEvent::Failure {
                    kind: ErrorKind::Mic
                }),
                tray(AppState::Error, false),
            ]
        );
        assert!(
            !fx.iter()
                .any(|e| matches!(e, Effect::StartTranscription { .. })),
            "gescheiterte Aufnahme wird nicht transkribiert"
        );
    }

    /// 56 — Der nächste Press öffnet das Gerät erneut (§6.4, §10 „Retry").
    #[test]
    fn press_after_mic_error_retries_capture() {
        let mut rt = recording_hotkey();
        let run = rt.run;
        transition(
            &mut rt,
            Event::CaptureFailed {
                run,
                message: "device lost".into(),
            },
        );
        let fx = transition(&mut rt, Event::HotkeyPress);
        assert_eq!(rt.state, hotkey_rec());
        assert_eq!(rt.error, None);
        assert_eq!(
            fx.first(),
            Some(&Effect::StartCapture {
                run: rt.run,
                cap: rt.cap
            })
        );
    }

    /// 57 — Hotkey-Registrierungsfehler: `error`, Hotkey tot, Tray-Click bleibt
    /// bedienbar (§4.4, §10).
    #[test]
    fn hotkey_registration_failure_leaves_tray_click_operable() {
        let mut rt = booted();
        let fx = transition(
            &mut rt,
            Event::FatalError {
                kind: ErrorKind::HotkeyRegistration,
                message: "F9 belegt".into(),
            },
        );
        assert_eq!(rt.state, AppState::Error);
        assert!(!rt.hotkey_armed(), "§10: Hotkey aus");
        assert!(rt.tray_click_armed(), "§4.4: Tray-Click bleibt aktiv");
        assert!(has_update_tray(&fx));

        let press = transition(&mut rt, Event::HotkeyPress);
        assert_eq!(rt.state, AppState::Error);
        assert_eq!(
            press,
            vec![Effect::Log(LogEvent::IgnoredPress {
                state: AppState::Error
            })]
        );

        let click = transition(&mut rt, Event::TrayClickToggle);
        assert_eq!(rt.state, tray_rec());
        assert_eq!(
            click.first(),
            Some(&Effect::StartCapture {
                run: rt.run,
                cap: rt.cap
            })
        );
    }

    /// 58 — Gescheiterte Inferenz ist ein bedienbarer Fehler (§10, analog Mic).
    #[test]
    fn transcription_failure_is_operable_error() {
        let mut rt = transcribing(RecordingSource::Hotkey, 3_000);
        let run = rt.run;
        let fx = transition(
            &mut rt,
            Event::TranscriptionFailed {
                run,
                message: "decode".into(),
            },
        );
        assert_eq!(rt.state, AppState::Error);
        assert_eq!(rt.error.as_ref().map(|e| e.kind), Some(ErrorKind::Engine));
        assert!(rt.hotkey_armed());
        assert_eq!(rt.watchdog_deadline, None);
        assert_eq!(
            fx,
            vec![
                Effect::DisarmWatchdog,
                Effect::Log(LogEvent::Failure {
                    kind: ErrorKind::Engine
                }),
                tray(AppState::Error, false),
            ]
        );
    }

    /// 59 — Fataler Configfehler aus jedem Zustand heraus (§8, §6.2).
    #[test]
    fn fatal_config_error_from_any_state() {
        let mut rt = recording_hotkey();
        let run = rt.run;
        let fx = transition(
            &mut rt,
            Event::FatalError {
                kind: ErrorKind::Config,
                message: "engine.model unbekannt".into(),
            },
        );
        assert_eq!(rt.state, AppState::Error);
        assert!(!rt.hotkey_armed());
        assert!(!rt.tray_click_armed());
        assert_eq!(
            fx,
            vec![
                Effect::StopCapture { run, discard: true },
                Effect::Log(LogEvent::Failure {
                    kind: ErrorKind::Config
                }),
                tray(AppState::Error, false),
            ]
        );
    }

    /// 60 — `error + Retry → starting`; die Startsequenz beginnt von vorn (§5.2).
    #[test]
    fn retry_from_error_restarts_the_startup_sequence() {
        let mut rt = Runtime::default();
        feed(
            &mut rt,
            vec![
                Event::Startup,
                Event::ArtifactsChecked {
                    run: RunId(0),
                    complete: false,
                },
                Event::DownloadFailed {
                    run: RunId(0),
                    message: "hash".into(),
                },
            ],
        );
        let failed_run = rt.run;
        let fx = transition(&mut rt, Event::RetryRequested);
        assert_eq!(rt.state, AppState::Starting);
        assert_eq!(rt.error, None);
        assert!(rt.run > failed_run, "Retry eröffnet einen neuen Lauf");
        assert_eq!(
            fx,
            vec![
                Effect::CheckArtifacts { run: rt.run },
                tray(AppState::Starting, false),
            ]
        );
    }

    /// 61 — Nach dem Retry sind Antworten des alten Laufs wertlos (§5.2).
    #[test]
    fn stale_download_answer_after_retry_is_ignored() {
        let mut rt = Runtime::default();
        feed(
            &mut rt,
            vec![
                Event::Startup,
                Event::ArtifactsChecked {
                    run: RunId(0),
                    complete: false,
                },
                Event::DownloadFailed {
                    run: RunId(0),
                    message: "hash".into(),
                },
            ],
        );
        let stale = rt.run;
        transition(&mut rt, Event::RetryRequested);
        let current = rt.run;
        let fx = transition(&mut rt, Event::DownloadFinished { run: stale });
        assert_eq!(rt.state, AppState::Starting);
        assert_eq!(
            fx,
            vec![Effect::Log(LogEvent::StaleRun {
                what: "download-finished",
                got: stale,
                current,
            })]
        );
    }

    // ------------------------------------------------------- J. Beenden (§5.2)

    /// 62 — Beenden während `transcribing`: Quit-Effekt, laufender Lauf verworfen.
    #[test]
    fn quit_during_transcribing_emits_quit_and_kills_the_run() {
        let mut rt = transcribing(RecordingSource::Hotkey, 4_000);
        let run = rt.run;
        let fx = transition(&mut rt, Event::QuitRequested);
        assert!(rt.quitting);
        assert!(rt.run > run);
        assert_eq!(
            fx,
            vec![
                Effect::AbortTranscription { run },
                Effect::DisarmWatchdog,
                Effect::Log(LogEvent::QuitRequested {
                    state: hotkey_trans()
                }),
                Effect::Quit,
            ]
        );
    }

    /// 63 — Nach dem Quit wird nichts mehr injiziert (§5.2 „kein Inject mehr").
    #[test]
    fn transcription_after_quit_is_never_injected() {
        let mut rt = transcribing(RecordingSource::Hotkey, 4_000);
        let run = rt.run;
        transition(&mut rt, Event::QuitRequested);
        let fx = transition(
            &mut rt,
            Event::TranscriptionDone {
                run,
                text: "zu spät".into(),
            },
        );
        assert!(
            !fx.iter()
                .any(|e| matches!(e, Effect::StartInject { .. } | Effect::CopyOnly { .. })),
            "kein Inject nach Quit"
        );
        assert_eq!(fx, vec![Effect::Log(LogEvent::IgnoredAfterQuit)]);
    }

    /// 64 — Beenden während `recording` verwirft die Aufnahme.
    #[test]
    fn quit_during_recording_discards_the_capture() {
        let mut rt = recording_hotkey();
        let run = rt.run;
        let fx = transition(&mut rt, Event::QuitRequested);
        assert!(rt.quitting);
        assert_eq!(
            fx,
            vec![
                Effect::StopCapture { run, discard: true },
                Effect::Log(LogEvent::QuitRequested {
                    state: hotkey_rec()
                }),
                Effect::Quit,
            ]
        );
    }

    /// 65 — Nach dem Quit nimmt der Kern keine Eingaben mehr an.
    #[test]
    fn input_after_quit_is_ignored() {
        let mut rt = booted();
        transition(&mut rt, Event::QuitRequested);
        for event in [
            Event::HotkeyPress,
            Event::TrayClickToggle,
            Event::PauseToggle,
            Event::RetryRequested,
        ] {
            let fx = transition(&mut rt, event);
            assert_eq!(fx, vec![Effect::Log(LogEvent::IgnoredAfterQuit)]);
        }
        assert_eq!(rt.state, AppState::Idle);
    }

    /// 66 — Beenden aus `idle` meldet nur `Quit`.
    #[test]
    fn quit_from_idle_emits_only_quit() {
        let mut rt = booted();
        let fx = transition(&mut rt, Event::QuitRequested);
        assert_eq!(
            fx,
            vec![
                Effect::Log(LogEvent::QuitRequested {
                    state: AppState::Idle
                }),
                Effect::Quit,
            ]
        );
    }

    // --------------------------------------- K. Zustand → Tray-Mapping (§4.3)

    /// 67 — Jeder Zustandswechsel emittiert genau ein `UpdateTray`, und zwar zuletzt.
    #[test]
    fn every_state_change_emits_exactly_one_update_tray_last() {
        let mut rt = Runtime::default();
        let sequence = vec![
            Event::Startup,
            Event::ArtifactsChecked {
                run: RunId(0),
                complete: false,
            },
            Event::DownloadFinished { run: RunId(0) },
            Event::ModelLoaded { run: RunId(0) },
            Event::HotkeyPress,
        ];
        for event in sequence {
            let before = (rt.state, rt.paused);
            let fx = transition(&mut rt, event);
            let after = (rt.state, rt.paused);
            let updates = fx
                .iter()
                .filter(|e| matches!(e, Effect::UpdateTray { .. }))
                .count();
            if before == after && !matches!(fx.first(), Some(Effect::CheckArtifacts { .. })) {
                assert_eq!(updates, 0, "kein Zustandswechsel, kein Tray-Update");
            } else {
                assert_eq!(updates, 1, "genau ein Tray-Update je Wechsel");
                assert_eq!(
                    fx.last(),
                    Some(&Effect::UpdateTray {
                        state: after.0,
                        paused: after.1
                    }),
                    "UpdateTray ist der letzte Effekt"
                );
            }
        }
    }

    /// 68 — Ignorierte Events malen den Tray nicht neu.
    #[test]
    fn ignored_events_do_not_repaint_the_tray() {
        let mut rt = recording_hotkey();
        for event in [
            Event::HotkeyPress,
            Event::TrayClickToggle,
            Event::Tick {
                elapsed: Duration::from_secs(1),
            },
        ] {
            let fx = transition(&mut rt, event);
            assert!(!has_update_tray(&fx), "{fx:?}");
        }
        assert_eq!(rt.state, hotkey_rec());
    }

    /// 69 — Das Pause-Flag geht mit in den Tray-Effekt, auch ohne Zustandswechsel (§4.3).
    #[test]
    fn pause_toggle_repaints_tray_without_state_change() {
        let mut rt = booted();
        let fx = transition(&mut rt, Event::PauseToggle);
        assert_eq!(
            fx,
            vec![Effect::UpdateTray {
                state: AppState::Idle,
                paused: true
            }]
        );
    }

    /// 70 — `paused` bleibt orthogonal: Zustand und Flag wandern unabhängig
    /// (Regressionsschutz für den Phase-2b-Test).
    #[test]
    fn paused_is_orthogonal_to_state() {
        let mut runtime = Runtime::default();
        assert_eq!(runtime.state, AppState::Starting);
        assert!(!runtime.paused);

        runtime.state = AppState::Recording {
            source: RecordingSource::Hotkey,
        };
        runtime.paused = true;
        assert!(runtime.paused);
        assert_eq!(
            runtime.state,
            AppState::Recording {
                source: RecordingSource::Hotkey
            }
        );

        runtime.state = AppState::Recording {
            source: RecordingSource::TrayClick,
        };
        assert!(runtime.paused);
    }
}
