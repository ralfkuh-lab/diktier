//! Log des Daemons (Spec §10): stderr (im `--foreground`) **plus**
//! `%LOCALAPPDATA%diktierdiktier.log`.
//!
//! Der Vertrag aus §10: **keine Transkripte, keine Clipboard-Inhalte, keine
//! Fenstertitel**. Wo eine Textmenge interessant ist, steht ihre Länge in Bytes.
//!
//! Ein Writer besitzt die Datei — deshalb hängt der Daemon den Datei-Sink erst
//! **nach** der Single-Instance-Sperre an (§5.3), und die CLI-Modi (`--help`,
//! `--version`, `--install-autostart`, `--remove-autostart`) benutzen ihn nie.
//!
//! Rotation statt In-Place-Truncate: erreicht `diktier.log` die Grenze (2 MiB,
//! im Test injizierbar), wandert sie per `rename` nach `diktier.log.1` — genau
//! eine Backup-Datei.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::paths;
use crate::state::{AppState, CopyReason, ErrorKind, LogEvent, RecordingSource, RunId};

/// Kurzname eines Zustands für Logzeilen (§5.2-Namen, klein geschrieben).
pub fn state_name(state: AppState) -> &'static str {
    match state {
        AppState::Starting => "starting",
        AppState::Downloading => "downloading",
        AppState::Loading => "loading",
        AppState::Idle => "idle",
        AppState::Recording {
            source: RecordingSource::Hotkey,
        } => "recording(hotkey)",
        AppState::Recording {
            source: RecordingSource::TrayClick,
        } => "recording(tray-click)",
        AppState::Transcribing {
            source: RecordingSource::Hotkey,
        } => "transcribing(hotkey)",
        AppState::Transcribing {
            source: RecordingSource::TrayClick,
        } => "transcribing(tray-click)",
        AppState::Injecting {
            source: RecordingSource::Hotkey,
        } => "injecting(hotkey)",
        AppState::Injecting {
            source: RecordingSource::TrayClick,
        } => "injecting(tray-click)",
        AppState::Error => "error",
    }
}

pub fn copy_reason_name(reason: CopyReason) -> &'static str {
    match reason {
        CopyReason::TrayClickPath => "Tray-Click-Pfad",
        CopyReason::FocusChanged => "Fokus geändert",
        CopyReason::FocusUnknown => "Fokus nicht ermittelbar",
    }
}

pub fn error_kind_name(kind: ErrorKind) -> &'static str {
    kind.as_str()
}

/// Menschenlesbare Zeile zu einem [`LogEvent`] des Kerns.
pub fn describe(event: &LogEvent) -> String {
    match event {
        LogEvent::IgnoredPress { state } => {
            format!("Hotkey-Press ignoriert (Zustand {})", state_name(*state))
        }
        LogEvent::IgnoredRelease { state } => {
            format!("Hotkey-Release ignoriert (Zustand {})", state_name(*state))
        }
        LogEvent::IgnoredTrayClick { state } => {
            format!("Tray-Linksklick ignoriert (Zustand {})", state_name(*state))
        }
        LogEvent::IgnoredWhilePaused => "Hotkey-Press ignoriert (pausiert)".into(),
        LogEvent::StaleRun { what, got, current } => format!(
            "verspätete Antwort verworfen: {what} (Lauf {}, aktuell {})",
            got.0, current.0
        ),
        LogEvent::AudioTooShort { millis } => {
            format!("Aufnahme {millis} ms < 250 ms — keine Transkription")
        }
        LogEvent::EmptyTranscript => "Transkript leer — nichts eingefügt".into(),
        LogEvent::RecordingDiscarded => "Pause aktiviert — laufende Aufnahme verworfen".into(),
        LogEvent::CopyOnlyNotice { reason } => format!(
            "Text liegt in der Zwischenablage ({})",
            copy_reason_name(*reason)
        ),
        LogEvent::Failure { kind } => format!("Fehlerzustand: {}", error_kind_name(*kind)),
        LogEvent::IgnoredRetry { state } => {
            format!("Retry ignoriert (Zustand {})", state_name(*state))
        }
        LogEvent::QuitRequested { state } => {
            format!("Beenden angefordert (Zustand {})", state_name(*state))
        }
        LogEvent::IgnoredAfterQuit => "Ereignis nach dem Beenden ignoriert".into(),
    }
}

/// Logger mit monotoner Laufzeitmarke, stderr und optionalem Datei-Sink.
///
/// `info` erscheint auf stderr nur mit `--foreground` (§9); in der Datei steht
/// es immer — sonst wäre das Log eines Autostart-Daemons leer. Warnungen und
/// Fehler gehen immer nach stderr.
pub struct Logger {
    start: Instant,
    foreground: bool,
    /// `None`, solange kein Datei-Log angehängt ist (CLI-Modi, Tests).
    file: Mutex<Option<FileSink>>,
}

/// Loglevel — bestimmt das Präfix und ob stderr die Zeile sieht.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    fn tag(self) -> &'static str {
        match self {
            Self::Info => "INFO ",
            Self::Warn => "WARN ",
            Self::Error => "ERROR",
        }
    }
}

impl Logger {
    /// Nur stderr. Der Daemon hängt danach [`Logger::attach_file`] an.
    pub fn new(foreground: bool) -> Self {
        Self {
            start: Instant::now(),
            foreground,
            file: Mutex::new(None),
        }
    }

    /// Datei-Sink öffnen (§10). Fehler sind nicht fatal: ohne Datei bleibt
    /// stderr, und der Daemon soll nicht daran scheitern, dass
    /// `%LOCALAPPDATA%diktier` nicht beschreibbar ist.
    pub fn attach_file(&self, path: &Path, limit: u64) -> io::Result<()> {
        let sink = FileSink::open(path, limit)?;
        let mut slot = self.file.lock().unwrap_or_else(|e| e.into_inner());
        *slot = Some(sink);
        Ok(())
    }

    /// Schreibt dieser Logger in eine Datei?
    pub fn has_file(&self) -> bool {
        let slot = self.file.lock().unwrap_or_else(|e| e.into_inner());
        slot.is_some()
    }

    fn stamp(&self) -> String {
        format!("[+{:8.3}s]", self.start.elapsed().as_secs_f64())
    }

    fn emit(&self, level: Level, message: &str) {
        let stamp = self.stamp();
        match level {
            Level::Info => {
                if self.foreground {
                    eprintln!("{stamp} {message}");
                }
            }
            Level::Warn | Level::Error => eprintln!("{stamp} {} {message}", level.tag()),
        }
        let mut slot = self.file.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(sink) = slot.as_mut() {
            let line = format!("{} {stamp} {} {message}", now_utc(), level.tag());
            if let Err(err) = sink.write_line(&line) {
                // Genau einmal meckern, dann ohne Datei weiterlaufen — ein
                // kaputtes Log darf das Diktieren nicht anhalten.
                eprintln!("{stamp} WARN  Datei-Log {}: {err}", sink.path.display());
                *slot = None;
            }
        }
    }

    pub fn info(&self, message: impl AsRef<str>) {
        self.emit(Level::Info, message.as_ref());
    }

    pub fn warn(&self, message: impl AsRef<str>) {
        self.emit(Level::Warn, message.as_ref());
    }

    pub fn error(&self, message: impl AsRef<str>) {
        self.emit(Level::Error, message.as_ref());
    }

    /// Kern-Logeffekt (§5.2). Ignorierte Eingaben sind Alltag → `info`.
    pub fn core(&self, event: &LogEvent) {
        match event {
            LogEvent::Failure { .. } => self.warn(describe(event)),
            other => self.info(describe(other)),
        }
    }

    /// Zustandswechsel, den der Tray sichtbar macht (§4.3).
    pub fn transition(&self, state: AppState, paused: bool) {
        if paused {
            self.info(format!("Zustand: {} (pausiert)", state_name(state)));
        } else {
            self.info(format!("Zustand: {}", state_name(state)));
        }
    }

    pub fn run(&self, run: RunId, message: impl AsRef<str>) {
        self.info(format!("Lauf {}: {}", run.0, message.as_ref()));
    }
}

/// Datei-Sink mit Rotation (§10).
struct FileSink {
    file: File,
    path: PathBuf,
    backup: PathBuf,
    /// Aktuelle Dateigröße in Bytes — mitgezählt statt bei jeder Zeile `stat`.
    len: u64,
    limit: u64,
}

impl FileSink {
    fn open(path: &Path, limit: u64) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            paths::create_private_dir(parent)?;
        }
        let file = open_append(path)?;
        let len = file.metadata()?.len();
        let backup = backup_path(path);
        Ok(Self {
            file,
            path: path.to_path_buf(),
            backup,
            len,
            limit,
        })
    }

    fn write_line(&mut self, line: &str) -> io::Result<()> {
        // §10: „Rotation, nicht In-Place-Truncate." Erst rotieren, dann
        // schreiben — so beginnt die neue Datei mit einer vollständigen Zeile.
        if self.len >= self.limit {
            self.rotate()?;
        }
        let bytes = line.len() + 1;
        writeln!(self.file, "{line}")?;
        self.len += bytes as u64;
        Ok(())
    }

    /// `diktier.log` → `diktier.log.1` (eine Backup-Datei), dann neue Datei.
    /// `rename` ersetzt ein vorhandenes Backup atomar.
    fn rotate(&mut self) -> io::Result<()> {
        std::fs::rename(&self.path, &self.backup)?;
        self.file = open_append(&self.path)?;
        self.len = 0;
        Ok(())
    }
}

fn open_append(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    options.open(path)
}

fn backup_path(path: &Path) -> PathBuf {
    match path.file_name() {
        Some(name) => {
            let mut name = name.to_os_string();
            name.push(".1");
            path.with_file_name(name)
        }
        None => path.with_extension("1"),
    }
}

/// Wanduhrzeit als `YYYY-MM-DDThh:mm:ssZ` (UTC).
///
/// Das Datei-Log überlebt den Prozess — eine rein monotone Marke wäre darin
/// wertlos. Lokalzeit bräuchte eine Zeitzonendatenbank; UTC ist eindeutig und
/// kostet keine Dependency.
fn now_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    format_utc(secs)
}

fn format_utc(unix_secs: i64) -> String {
    let days = unix_secs.div_euclid(86_400);
    let rem = unix_secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Tage seit 1970-01-01 → (Jahr, Monat, Tag), proleptischer gregorianischer
/// Kalender (Algorithmus `civil_from_days`, Howard Hinnant).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_names_cover_every_state() {
        let states = [
            AppState::Starting,
            AppState::Downloading,
            AppState::Loading,
            AppState::Idle,
            AppState::Recording {
                source: RecordingSource::Hotkey,
            },
            AppState::Recording {
                source: RecordingSource::TrayClick,
            },
            AppState::Transcribing {
                source: RecordingSource::Hotkey,
            },
            AppState::Transcribing {
                source: RecordingSource::TrayClick,
            },
            AppState::Injecting {
                source: RecordingSource::Hotkey,
            },
            AppState::Injecting {
                source: RecordingSource::TrayClick,
            },
            AppState::Error,
        ];
        for state in states {
            assert!(!state_name(state).is_empty(), "{state:?}");
        }
        assert_eq!(
            state_name(AppState::Recording {
                source: RecordingSource::TrayClick
            }),
            "recording(tray-click)"
        );
    }

    /// §10: Logzeilen enthalten nie Transkripte — nur Zustände, Zahlen, Gründe.
    #[test]
    fn describe_never_contains_transcript_text() {
        let events = [
            LogEvent::EmptyTranscript,
            LogEvent::CopyOnlyNotice {
                reason: CopyReason::FocusChanged,
            },
            LogEvent::AudioTooShort { millis: 120 },
            LogEvent::StaleRun {
                what: "transcription-done",
                got: RunId(3),
                current: RunId(5),
            },
            LogEvent::Failure {
                kind: ErrorKind::Inject,
            },
            LogEvent::QuitRequested {
                state: AppState::Idle,
            },
            LogEvent::IgnoredAfterQuit,
            LogEvent::IgnoredWhilePaused,
            LogEvent::RecordingDiscarded,
            LogEvent::IgnoredRetry {
                state: AppState::Idle,
            },
            LogEvent::IgnoredPress {
                state: AppState::Loading,
            },
            LogEvent::IgnoredRelease {
                state: AppState::Idle,
            },
            LogEvent::IgnoredTrayClick {
                state: AppState::Loading,
            },
        ];
        for event in &events {
            let line = describe(event);
            assert!(!line.is_empty(), "{event:?}");
            assert!(!line.contains('\n'), "einzeilig: {line}");
        }
        assert!(describe(&events[3]).contains("Lauf 3"));
        assert!(describe(&events[2]).contains("120 ms"));
    }

    // ------------------------------------------------- Datei-Log (§10, Teil 2)

    #[test]
    fn file_log_receives_info_even_without_foreground() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(paths::LOG_NAME);
        let log = Logger::new(false);
        log.attach_file(&path, paths::LOG_LIMIT_BYTES).unwrap();

        log.info("gestartet");
        log.warn("schief");
        log.error("kaputt");

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("INFO  gestartet"), "{text}");
        assert!(text.contains("WARN  schief"), "{text}");
        assert!(text.contains("ERROR kaputt"), "{text}");
        assert_eq!(text.lines().count(), 3);
    }

    #[test]
    fn file_log_appends_across_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(paths::LOG_NAME);

        let first = Logger::new(false);
        first.attach_file(&path, paths::LOG_LIMIT_BYTES).unwrap();
        first.info("lauf eins");
        drop(first);

        let second = Logger::new(false);
        second.attach_file(&path, paths::LOG_LIMIT_BYTES).unwrap();
        second.info("lauf zwei");

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("lauf eins") && text.contains("lauf zwei"),
            "{text}"
        );
    }

    /// §10: Rotation statt In-Place-Truncate, genau eine Backup-Datei.
    #[test]
    fn file_log_rotates_at_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(paths::LOG_NAME);
        let backup = dir.path().join(paths::LOG_BACKUP_NAME);
        let log = Logger::new(false);
        // Künstlich kleine Grenze: eine Zeile passt, die zweite rotiert.
        log.attach_file(&path, 80).unwrap();

        log.info("zeile-eins-die-ueber-achtzig-bytes-lang-wird-damit-die-grenze-faellt-xxxxxxxxxx");
        assert!(!backup.exists(), "vor der Grenze wird nicht rotiert");
        log.info("zeile-zwei");

        assert!(backup.is_file(), "Backup fehlt");
        let rotated = std::fs::read_to_string(&backup).unwrap();
        let current = std::fs::read_to_string(&path).unwrap();
        assert!(rotated.contains("zeile-eins"), "{rotated}");
        assert!(current.contains("zeile-zwei"), "{current}");
        assert!(!current.contains("zeile-eins"), "{current}");

        // Zweite Rotation überschreibt dasselbe Backup — kein Ringpuffer.
        log.info("zeile-drei-die-ebenfalls-ueber-achtzig-bytes-lang-wird-xxxxxxxxxxxxxxxxxxxxxxxx");
        log.info("zeile-vier");
        let rotated = std::fs::read_to_string(&backup).unwrap();
        assert!(rotated.contains("zeile-drei"), "{rotated}");
        assert!(!rotated.contains("zeile-eins"), "{rotated}");
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 2, "genau Log + eine Backup-Datei: {names:?}");
    }

    #[test]
    fn rotated_lines_are_never_split() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(paths::LOG_NAME);
        let log = Logger::new(false);
        log.attach_file(&path, 100).unwrap();
        for i in 0..40 {
            log.info(format!(
                "zeile {i} mit etwas Text, damit die Grenze oft fällt"
            ));
        }
        for file in [path, dir.path().join(paths::LOG_BACKUP_NAME)] {
            let text = std::fs::read_to_string(&file).unwrap();
            for line in text.lines() {
                assert!(line.contains(" INFO  zeile "), "abgeschnitten: {line}");
                assert!(line.starts_with("20"), "kein Zeitstempel: {line}");
            }
        }
    }

    #[test]
    fn logger_without_file_stays_silent_on_disk() {
        let log = Logger::new(false);
        assert!(!log.has_file());
        log.info("nichts");
        log.warn("nichts");
    }

    #[test]
    fn attach_file_creates_the_directory_and_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join(paths::LOG_NAME);
        let log = Logger::new(false);
        log.attach_file(&path, paths::LOG_LIMIT_BYTES).unwrap();
        assert!(log.has_file());
        assert!(path.is_file());
    }

    #[test]
    fn backup_path_appends_dot_one() {
        assert_eq!(
            backup_path(Path::new("/x/diktier.log")),
            PathBuf::from("/x/diktier.log.1")
        );
        assert_eq!(
            backup_path(Path::new("/x/diktier.log"))
                .file_name()
                .unwrap(),
            paths::LOG_BACKUP_NAME
        );
    }

    #[test]
    fn utc_formatting_matches_known_instants() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc(1_000_000_000), "2001-09-09T01:46:40Z");
        // Schaltjahr-Kante.
        assert_eq!(format_utc(1_709_164_800), "2024-02-29T00:00:00Z");
        assert_eq!(format_utc(1_798_675_200), "2026-12-31T00:00:00Z");
        assert_eq!(format_utc(1_798_761_599), "2026-12-31T23:59:59Z");
    }
}
