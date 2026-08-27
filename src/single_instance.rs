//! Single-Instance und Download-Lock (Spec §5.3, §6.3).
//!
//! Zwei Plattformen, zwei Mechanismen — beide hinter [`Acquire`]/[`PathLock`]:
//!
//! - **Windows** nimmt Named Mutexe im `Local\`-Namensraum. Der ist bereits
//!   **pro interaktiver Session** (§5.3, präzisiert 2026-08-27), deshalb steckt
//!   kein User-Hash im Namen und eine zweite RDP-Session bekommt ihre eigene
//!   Instanz. Es gibt dort **keine** Sperrdatei: `CreateMutexW` meldet
//!   `ERROR_ALREADY_EXISTS` auch dann, wenn der zweite Aufruf aus demselben
//!   Prozess kommt — genau die Semantik, die die Tests brauchen.
//! - **Linux** nimmt den gehaltenen advisory `flock`, siehe unten.
//!
//! Kein PID-File. Linux: ein **gehaltener** advisory `flock` unter
//! `$XDG_RUNTIME_DIR/diktier.lock`, Fallback `$XDG_STATE_HOME/diktier/diktier.lock`.
//! Eine liegengebliebene Datei ist egal — allein der Lock zählt, und der stirbt
//! mit dem Prozess (auch bei `kill -9`, der Kernel räumt den Deskriptor ab).
//!
//! `flock` hängt an der *open file description*, nicht am Prozess: zwei
//! `open()`+`flock()` im selben Prozess konkurrieren genauso wie zwei Prozesse.
//! Genau das nutzen die Tests.
//!
//! Wer die Sperre nimmt, steht in §5.3: **nur** `diktier` bzw.
//! `diktier --foreground`. `--help`, `--version`, `--install-autostart` und
//! `--remove-autostart` laufen davor und fordern sie nie an.

// `Acquire::held` und die Namenskonstanten benutzen nur die Tests bzw. der
// Windows-Zweig — wie in `download.rs` ist das kein toter Code.
#![allow(dead_code)]

#[cfg(target_os = "linux")]
use std::collections::HashSet;
#[cfg(target_os = "linux")]
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::paths::{self, PathError};

#[derive(Debug, Error)]
pub enum LockError {
    #[error("Sperrdatei {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    /// Windows: `CreateMutexW` hat kein Handle geliefert. Häufigster Fall ist
    /// `ERROR_INVALID_HANDLE` — dann trägt ein **anderer Objekttyp** bereits
    /// diesen Namen, und das ist kein „läuft schon".
    #[cfg(windows)]
    #[error("Sperrobjekt {name}: {source}")]
    Mutex { name: String, source: io::Error },
    #[error("Sperrpfad: {0}")]
    Path(#[from] PathError),
}

/// Ergebnis eines Sperrversuchs.
#[derive(Debug)]
pub enum Acquire {
    /// Sperre gehört uns, solange der [`PathLock`] lebt.
    Held(PathLock),
    /// Ein anderer Prozess hält sie (§5.3: Exit 0, kurze Meldung, sonst nichts).
    Busy,
}

impl Acquire {
    pub fn held(self) -> Option<PathLock> {
        match self {
            Self::Held(lock) => Some(lock),
            Self::Busy => None,
        }
    }
}

/// Gehaltene, an einen Pfad gebundene Sperre.
///
/// Linux: der Lock endet mit dem Schließen des Deskriptors, also beim `Drop` —
/// die Datei selbst bleibt bewusst liegen (Löschen wäre ein Rennen gegen den
/// nächsten Starter, der sie schon geöffnet hat). Windows: der Pfad ist nur
/// noch der **Schlüssel** für den Mutex-Namen, eine Datei entsteht nicht.
#[derive(Debug)]
pub struct PathLock {
    #[cfg(target_os = "linux")]
    _file: File,
    #[cfg(windows)]
    _mutex: win::MutexLock,
    path: PathBuf,
}

impl PathLock {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Die gehaltene Single-Instance-Sperre. Linux: **alle** nutzbaren Orte auf
/// einmal. Windows: der eine Named Mutex aus §5.3.
#[derive(Debug)]
pub struct InstanceLock {
    #[cfg(target_os = "linux")]
    locks: Vec<PathLock>,
    #[cfg(windows)]
    mutex: win::MutexLock,
}

impl InstanceLock {
    #[cfg(target_os = "linux")]
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.locks.iter().map(PathLock::path)
    }

    /// Für die Startzeile im Log.
    #[cfg(target_os = "linux")]
    pub fn describe(&self) -> String {
        self.paths()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Für die Startzeile im Log — auf Windows der Mutex-Name (Plan WP5).
    #[cfg(windows)]
    pub fn describe(&self) -> String {
        self.mutex.name().to_string()
    }
}

/// Ergebnis der Instanzsperre.
#[derive(Debug)]
pub enum InstanceAcquire {
    Held(InstanceLock),
    Busy,
}

/// Single-Instance-Sperre des Daemons (§5.3).
///
/// §5.3 nennt `$XDG_RUNTIME_DIR` und das Zustandsverzeichnis als Fallback.
/// Genommen werden **beide**: Ein Prozess mit gesetztem `XDG_RUNTIME_DIR` und
/// einer ohne griffen sonst zu verschiedenen Dateien und liefen doppelt (real
/// reproduziert). Die Reihenfolge bleibt die der Spec, und eine Datei, die sich
/// gar nicht anlegen lässt, wird übersprungen statt den Start abzubrechen —
/// solange mindestens ein Ort gehalten wird, ist die Instanz eindeutig.
#[cfg(target_os = "linux")]
pub fn acquire_instance_lock(
    on_problem: &mut dyn FnMut(String),
) -> Result<InstanceAcquire, LockError> {
    acquire_all(&instance_lock_candidates()?, on_problem)
}

/// Windows-Variante nach §5.3: **ein** Named Mutex mit festem Namen, kein
/// Kandidatenpfad und keine Sperrdatei. `on_problem` bleibt ungenutzt — es gibt
/// keinen Ort, auf den ausgewichen werden könnte.
#[cfg(windows)]
pub fn acquire_instance_lock(
    _on_problem: &mut dyn FnMut(String),
) -> Result<InstanceAcquire, LockError> {
    match win::create(win::INSTANCE_MUTEX)? {
        win::Taken::Held(mutex) => Ok(InstanceAcquire::Held(InstanceLock { mutex })),
        win::Taken::Busy => Ok(InstanceAcquire::Busy),
    }
}

/// Kern der Instanzsperre, gegen Temp-Pfade testbar. Linux-only: auf Windows
/// gibt es genau ein Sperrobjekt und keine Ausweichorte.
#[cfg(target_os = "linux")]
fn acquire_all(
    candidates: &[PathBuf],
    on_problem: &mut dyn FnMut(String),
) -> Result<InstanceAcquire, LockError> {
    let mut held: Vec<PathLock> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for path in candidates {
        // codex M4: Zeigen zwei Kandidaten (Symlink, `..`, gleicher Pfad) auf
        // dieselbe Datei, sähe der zweite `open()+flock()` den **eigenen** Lock
        // als „belegt" — der erste Daemon sperrte sich selbst aus.
        if !seen.insert(identity(path)) {
            continue;
        }
        match try_lock(path) {
            Ok(Acquire::Held(lock)) => held.push(lock),
            // Ein belegter Ort genügt: es läuft schon einer. Die bereits
            // genommenen Sperren fallen mit `held` aus dem Gültigkeitsbereich.
            Ok(Acquire::Busy) => return Ok(InstanceAcquire::Busy),
            Err(err) => on_problem(format!(
                "Sperrdatei {} nicht nutzbar ({}) — nächster Ort",
                path.display(),
                err
            )),
        }
    }
    if held.is_empty() {
        return Err(LockError::Path(PathError::Missing(
            "kein nutzbarer Ort für die Sperrdatei".into(),
        )));
    }
    Ok(InstanceAcquire::Held(InstanceLock { locks: held }))
}

/// Stabile Identität eines Sperrkandidaten.
///
/// Die Datei selbst existiert beim ersten Start noch nicht, das Verzeichnis
/// aber schon (`try_lock` legt es an) — deshalb wird das **Elternverzeichnis**
/// aufgelöst und der Dateiname angehängt. Lässt sich nichts auflösen, bleibt
/// der Pfad selbst der Schlüssel: dann sind zwei Schreibweisen zwar nicht
/// erkennbar, aber auch nicht schlechter als vorher.
#[cfg(target_os = "linux")]
fn identity(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => {
            match parent.canonicalize() {
                Ok(dir) => dir.join(name),
                Err(_) => path.to_path_buf(),
            }
        }
        _ => path.to_path_buf(),
    }
}

/// Ort des per-user Download-Locks aus §6.3 — dieselbe Reihenfolge wie bei der
/// Instanzsperre. Genommen wird er in [`crate::download::download_model_locked`],
/// damit der Test den Parallelstart nachstellen kann.
pub fn download_lock_path() -> Result<PathBuf, LockError> {
    let mut candidates = lock_candidates(DOWNLOAD_LOCK_NAME)?;
    // Ein Kandidat taugt, wenn sein Verzeichnis anlegbar ist; sonst der nächste.
    if let Some(index) = candidates.iter().position(|path| {
        path.parent()
            .is_some_and(|dir| paths::create_private_dir(dir).is_ok())
    }) {
        return Ok(candidates.swap_remove(index));
    }
    candidates.pop().ok_or_else(|| {
        LockError::Path(PathError::Missing(
            "kein nutzbarer Ort für die Download-Sperre".into(),
        ))
    })
}

pub const INSTANCE_LOCK_NAME: &str = "diktier.lock";
pub const DOWNLOAD_LOCK_NAME: &str = "diktier-download.lock";

#[cfg(target_os = "linux")]
fn instance_lock_candidates() -> Result<Vec<PathBuf>, LockError> {
    lock_candidates(INSTANCE_LOCK_NAME)
}

/// Reihenfolge aus §5.3: Laufzeitverzeichnis zuerst, Zustandsverzeichnis als
/// Fallback. Fehlt beides, ist das ein Pfadfehler.
fn lock_candidates(name: &str) -> Result<Vec<PathBuf>, LockError> {
    let mut out = Vec::new();
    #[cfg(target_os = "linux")]
    if let Some(dir) = paths::runtime_dir() {
        out.push(dir.join(name));
    }
    match paths::state_dir() {
        Ok(dir) => out.push(dir.join(name)),
        Err(err) if out.is_empty() => return Err(err.into()),
        Err(_) => {}
    }
    Ok(out)
}

/// Datei anlegen/öffnen und nichtblockierend exklusiv sperren.
///
/// `Ok(Busy)` heißt „läuft schon", `Err` heißt „diesen Ort können wir nicht
/// benutzen" (der Aufrufer weicht dann aus).
#[cfg(target_os = "linux")]
pub fn try_lock(path: &Path) -> Result<Acquire, LockError> {
    if let Some(parent) = path.parent() {
        paths::create_private_dir(parent).map_err(|source| LockError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }
    let file = open_lock_file(path).map_err(|source| LockError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    match flock_exclusive_nonblocking(&file) {
        Ok(true) => Ok(Acquire::Held(PathLock {
            _file: file,
            path: path.to_path_buf(),
        })),
        Ok(false) => Ok(Acquire::Busy),
        Err(source) => Err(LockError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Windows: der Pfad wird zum Mutex-Namen (§6.3-Download-Lock, Plan WP5).
///
/// Es entsteht **keine** Datei — der `Local\`-Mutex trägt die Sperre allein.
/// Damit verhält sich `try_lock` genauso wie unter Linux: derselbe Pfad zweimal
/// ergibt `Busy`, auch innerhalb eines Prozesses, und das Fallenlassen gibt die
/// Sperre frei.
#[cfg(windows)]
pub fn try_lock(path: &Path) -> Result<Acquire, LockError> {
    let name = win::download_mutex_name(path);
    match win::create(&name)? {
        win::Taken::Held(mutex) => Ok(Acquire::Held(PathLock {
            _mutex: mutex,
            path: path.to_path_buf(),
        })),
        win::Taken::Busy => Ok(Acquire::Busy),
    }
}

#[cfg(target_os = "linux")]
fn open_lock_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// `true` = Sperre gehört uns, `false` = jemand anderes hält sie.
#[cfg(target_os = "linux")]
fn flock_exclusive_nonblocking(file: &File) -> io::Result<bool> {
    use std::os::fd::AsRawFd;

    // SAFETY: `fd` ist gültig, solange `file` lebt; `flock` verändert nur den
    // Sperrzustand der open file description.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Ok(true);
    }
    let err = io::Error::last_os_error();
    match err.raw_os_error() {
        // EWOULDBLOCK == EAGAIN: hält ein anderer.
        Some(code) if code == libc::EWOULDBLOCK || code == libc::EINTR => {
            if code == libc::EINTR {
                // Ein Signal hat den Aufruf unterbrochen — nichts über den
                // Sperrzustand gelernt, also als „belegt" behandeln wäre falsch.
                // Ein einziger Wiederholungsversuch genügt: LOCK_NB kehrt sofort
                // zurück.
                // SAFETY: wie oben.
                let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
                if rc == 0 {
                    return Ok(true);
                }
                let err = io::Error::last_os_error();
                return match err.raw_os_error() {
                    Some(code) if code == libc::EWOULDBLOCK => Ok(false),
                    _ => Err(err),
                };
            }
            Ok(false)
        }
        _ => Err(err),
    }
}

/// Named Mutexe im `Local\`-Namensraum (§5.3, Plan WP5).
///
/// Ein Mutex-Objekt lebt, solange irgendein Prozess ein Handle darauf hält;
/// `CloseHandle` im `Drop` gibt es frei. Gewartet wird nie — allein die
/// **Existenz** des Namens ist die Sperre, und `CreateMutexW` meldet sie über
/// `ERROR_ALREADY_EXISTS`. Das gilt auch prozessintern, deshalb stellen die
/// Tests einen zweiten Start korrekt nach.
#[cfg(windows)]
mod win {
    use std::io;
    use std::path::Path;
    use std::ptr;

    use sha2::{Digest, Sha256};
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    /// §5.3: `Local\` ist bereits pro interaktiver Session — Tray und Hotkey
    /// sind ohnehin sessiongebunden. Kein User-Hash nötig.
    pub const INSTANCE_MUTEX: &str = r"Local\FerberDiktier.v1.instance";

    /// Präfix des Download-Locks (§6.3). Der Pfad wird gehasht, damit der Name
    /// keine Backslashes enthält — die trennen im Objektnamensraum Verzeichnisse.
    const DOWNLOAD_PREFIX: &str = r"Local\FerberDiktier.v1.download.";

    /// Ein gehaltenes Handle. Der Name bleibt für `describe()` erhalten.
    #[derive(Debug)]
    pub struct MutexLock {
        handle: HANDLE,
        name: String,
    }

    impl MutexLock {
        pub fn name(&self) -> &str {
            &self.name
        }
    }

    impl Drop for MutexLock {
        fn drop(&mut self) {
            if !self.handle.is_null() {
                // SAFETY: eigenes Handle aus `CreateMutexW`, genau einmal
                // geschlossen (danach genullt).
                unsafe { CloseHandle(self.handle) };
                self.handle = ptr::null_mut();
            }
        }
    }

    pub enum Taken {
        Held(MutexLock),
        Busy,
    }

    /// Windows-Pfade sind case-insensitiv; kleingeschrieben ergeben zwei
    /// Schreibweisen desselben Pfades denselben Namen.
    pub fn download_mutex_name(path: &Path) -> String {
        let key = path.to_string_lossy().to_lowercase();
        let digest = Sha256::digest(key.as_bytes());
        let mut name = String::with_capacity(DOWNLOAD_PREFIX.len() + 64);
        name.push_str(DOWNLOAD_PREFIX);
        for byte in digest {
            use std::fmt::Write;
            let _ = write!(name, "{byte:02x}");
        }
        name
    }

    pub fn create(name: &str) -> Result<Taken, super::LockError> {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: `wide` ist NUL-terminiert und lebt über den Aufruf; ein
        // NULL-`SECURITY_ATTRIBUTES` bedeutet „Default-DACL, nicht vererbbar".
        // `bInitialOwner = FALSE`: wir wollen den Namen, nicht den Besitz.
        let handle = unsafe { CreateMutexW(ptr::null(), 0, wide.as_ptr()) };
        // Sofort lesen: `GetLastError` trägt `ERROR_ALREADY_EXISTS` **auch bei
        // Erfolg**, und jeder weitere Win32-Aufruf würde den Wert überschreiben.
        // SAFETY: parameterlos.
        let err = unsafe { GetLastError() };
        if handle.is_null() {
            return Err(super::LockError::Mutex {
                name: name.to_string(),
                source: io::Error::from_raw_os_error(err as i32),
            });
        }
        let lock = MutexLock {
            handle,
            name: name.to_string(),
        };
        if err == ERROR_ALREADY_EXISTS {
            // Das eigene Handle wieder schließen; das Objekt gehört weiter dem
            // anderen Halter.
            drop(lock);
            return Ok(Taken::Busy);
        }
        Ok(Taken::Held(lock))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_lock_on_same_path_is_busy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("diktier.lock");

        let first = try_lock(&path).unwrap();
        assert!(
            matches!(first, Acquire::Held(_)),
            "erster Lock muss greifen"
        );

        // Zweiter Versuch im selben Prozess: `flock` hängt an der open file
        // description, nicht am Prozess — das verhält sich wie ein zweiter Start.
        let second = try_lock(&path).unwrap();
        assert!(
            matches!(second, Acquire::Busy),
            "zweiter Lock muss belegt sein"
        );
    }

    #[test]
    fn lock_is_released_when_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("diktier.lock");

        let first = try_lock(&path).unwrap().held().expect("gehalten");
        assert!(matches!(try_lock(&path).unwrap(), Acquire::Busy));
        drop(first);

        // Wie nach `kill -9`: der Deskriptor ist weg, die Sperre frei.
        assert!(
            matches!(try_lock(&path).unwrap(), Acquire::Held(_)),
            "nach dem Freigeben muss der Lock wieder zu haben sein"
        );
    }

    /// Reine Dateisemantik: auf Windows gibt es keine Sperrdatei, die
    /// liegenbleiben könnte (§5.3 nennt dort nur den Named Mutex).
    #[cfg(target_os = "linux")]
    #[test]
    fn stale_lock_file_does_not_block() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("diktier.lock");
        // Liegengebliebene Datei mit Inhalt (§5.3: „ist egal, allein der Lock zählt").
        std::fs::write(&path, b"1234\n").unwrap();

        let lock = try_lock(&path).unwrap();
        assert!(matches!(lock, Acquire::Held(_)));
    }

    /// Ebenfalls reine Dateisemantik — inklusive `0600`.
    #[cfg(target_os = "linux")]
    #[test]
    fn lock_file_is_created_with_private_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("diktier.lock");
        let _lock = try_lock(&path).unwrap().held().expect("gehalten");
        assert!(path.is_file());
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{mode:o}");
        }
    }

    #[test]
    fn lock_reports_its_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("diktier.lock");
        let lock = try_lock(&path).unwrap().held().unwrap();
        assert_eq!(lock.path(), path);
    }

    #[test]
    fn instance_and_download_lock_are_independent() {
        let dir = tempfile::tempdir().unwrap();
        let instance = try_lock(&dir.path().join(INSTANCE_LOCK_NAME))
            .unwrap()
            .held()
            .expect("instanz");
        let download = try_lock(&dir.path().join(DOWNLOAD_LOCK_NAME))
            .unwrap()
            .held()
            .expect("download");
        assert_ne!(instance.path(), download.path());
    }

    // ------------------------------------- Instanzsperre über beide Orte (§5.3)
    //
    // Der ganze Block ist Linux: `acquire_all` und die Kandidatenliste gibt es
    // nur dort. Windows nimmt nach §5.3 **einen** Named Mutex mit festem Namen,
    // kennt weder Ausweichorte noch Symlink-Dedup und hat deshalb nichts, was
    // diese Tests prüfen könnten.

    #[cfg(target_os = "linux")]
    fn quiet() -> impl FnMut(String) {
        |problem| panic!("unerwartetes Sperrproblem: {problem}")
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn instance_lock_takes_every_candidate() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = dir.path().join("run").join(INSTANCE_LOCK_NAME);
        let state = dir.path().join("state").join(INSTANCE_LOCK_NAME);

        let mut noop = quiet();
        let held = match acquire_all(&[runtime.clone(), state.clone()], &mut noop).unwrap() {
            InstanceAcquire::Held(lock) => lock,
            InstanceAcquire::Busy => panic!("erster Start muss die Sperre bekommen"),
        };
        assert_eq!(held.paths().collect::<Vec<_>>(), [&*runtime, &*state]);
        assert!(held.describe().contains("run"));

        // Beide Orte sind jetzt belegt — egal, welchen ein zweiter Start ansieht.
        assert!(matches!(try_lock(&runtime).unwrap(), Acquire::Busy));
        assert!(matches!(try_lock(&state).unwrap(), Acquire::Busy));
    }

    /// codex M4: Derselbe Pfad zweimal in der Liste darf den ersten Start nicht
    /// gegen sich selbst sperren.
    #[cfg(target_os = "linux")]
    #[test]
    fn duplicate_candidates_do_not_lock_the_process_out() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run").join(INSTANCE_LOCK_NAME);

        let mut noop = quiet();
        let held = match acquire_all(&[path.clone(), path.clone()], &mut noop).unwrap() {
            InstanceAcquire::Held(lock) => lock,
            InstanceAcquire::Busy => panic!("der eigene Lock darf nicht als belegt gelten"),
        };
        assert_eq!(held.paths().count(), 1, "genau eine Sperre");
    }

    /// Derselbe Fall über einen Symlink: `$XDG_RUNTIME_DIR` zeigt auf dasselbe
    /// Verzeichnis wie der State-Fallback (codex M4).
    #[cfg(target_os = "linux")]
    #[test]
    fn symlinked_candidates_are_deduplicated() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("state");
        std::fs::create_dir_all(&real).unwrap();
        let alias = dir.path().join("run");
        std::os::unix::fs::symlink(&real, &alias).unwrap();

        let via_real = real.join(INSTANCE_LOCK_NAME);
        let via_alias = alias.join(INSTANCE_LOCK_NAME);
        assert_ne!(via_real, via_alias, "lexikalisch verschieden");
        assert_eq!(identity(&via_real), identity(&via_alias), "gleiche Datei");

        let mut noop = quiet();
        let held = match acquire_all(&[via_alias, via_real.clone()], &mut noop).unwrap() {
            InstanceAcquire::Held(lock) => lock,
            InstanceAcquire::Busy => panic!("Selbstsperre über den Symlink-Alias"),
        };
        assert_eq!(held.paths().count(), 1);

        // Ein echter zweiter Start sieht die Datei weiterhin als belegt.
        assert!(matches!(try_lock(&via_real).unwrap(), Acquire::Busy));
    }

    /// Der Fall, der mit reinem Fallback zwei Instanzen erlaubte: A kennt nur
    /// das Zustandsverzeichnis, B zusätzlich `$XDG_RUNTIME_DIR`.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_process_locking_only_the_fallback_blocks_a_process_with_both() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = dir.path().join("run").join(INSTANCE_LOCK_NAME);
        let state = dir.path().join("state").join(INSTANCE_LOCK_NAME);

        let mut noop = quiet();
        let _a = acquire_all(std::slice::from_ref(&state), &mut noop).unwrap();
        let b = acquire_all(&[runtime.clone(), state.clone()], &mut noop).unwrap();
        assert!(matches!(b, InstanceAcquire::Busy), "B darf nicht starten");

        // Und die Gegenrichtung: A hat beide, B kennt nur den Fallback.
        drop(_a);
        let _a = acquire_all(&[runtime, state.clone()], &mut noop).unwrap();
        assert!(matches!(
            acquire_all(&[state], &mut noop).unwrap(),
            InstanceAcquire::Busy
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_busy_candidate_releases_the_locks_already_taken() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = dir.path().join("diktier.lock");
        let state = dir.path().join("state.lock");

        let mut noop = quiet();
        let blocker = try_lock(&state).unwrap().held().unwrap();
        let busy = acquire_all(&[runtime.clone(), state.clone()], &mut noop).unwrap();
        assert!(matches!(busy, InstanceAcquire::Busy));

        // Der zuerst genommene Runtime-Lock darf nicht hängenbleiben, sonst
        // sperrte sich der nächste Start selbst aus.
        assert!(matches!(try_lock(&runtime).unwrap(), Acquire::Held(_)));
        drop(blocker);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_unusable_candidate_is_skipped_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        // Datei statt Verzeichnis: `create_private_dir` scheitert hier.
        let blocked_dir = dir.path().join("keindir");
        std::fs::write(&blocked_dir, b"x").unwrap();
        let unusable = blocked_dir.join(INSTANCE_LOCK_NAME);
        let usable = dir.path().join("state").join(INSTANCE_LOCK_NAME);

        let mut problems = Vec::new();
        let held = acquire_all(&[unusable, usable.clone()], &mut |p| problems.push(p)).unwrap();
        match held {
            InstanceAcquire::Held(lock) => {
                assert_eq!(lock.paths().collect::<Vec<_>>(), [&*usable]);
            }
            InstanceAcquire::Busy => panic!("ein nutzbarer Ort blieb übrig"),
        }
        assert_eq!(problems.len(), 1, "{problems:?}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn no_usable_candidate_is_an_error() {
        let mut problems = Vec::new();
        let err = acquire_all(&[], &mut |p| problems.push(p)).unwrap_err();
        assert!(matches!(err, LockError::Path(_)));
    }

    #[test]
    fn download_lock_path_is_usable_and_separate() {
        let path = download_lock_path().unwrap();
        assert_eq!(path.file_name().unwrap(), DOWNLOAD_LOCK_NAME);
        assert!(path.is_absolute());
        assert!(
            path.parent().unwrap().is_dir(),
            "Verzeichnis muss angelegt sein: {}",
            path.display()
        );
        assert_ne!(path.file_name().unwrap(), INSTANCE_LOCK_NAME);
    }

    #[test]
    fn candidates_prefer_runtime_dir_then_state_dir() {
        let candidates = lock_candidates(INSTANCE_LOCK_NAME).unwrap();
        assert!(!candidates.is_empty());
        for path in &candidates {
            assert_eq!(path.file_name().unwrap(), INSTANCE_LOCK_NAME);
            assert!(path.is_absolute());
        }
        #[cfg(target_os = "linux")]
        if let Some(runtime) = paths::runtime_dir() {
            assert_eq!(candidates[0], runtime.join(INSTANCE_LOCK_NAME));
            assert_eq!(
                candidates.len(),
                2,
                "Fallback ins Zustandsverzeichnis fehlt"
            );
        }
    }
    /// Namensbildung des Windows-Zweigs (Plan WP5) — reine Logik, kein Win32.
    #[cfg(windows)]
    mod win {
        use super::super::win::{INSTANCE_MUTEX, download_mutex_name};
        use std::path::Path;

        /// §5.3: fester Name im sessionlokalen Namensraum.
        #[test]
        fn the_instance_mutex_is_session_local_and_fixed() {
            assert_eq!(INSTANCE_MUTEX, r"Local\FerberDiktier.v1.instance");
        }

        /// §6.3: ein Download-Lock je Pfad, Name ohne Backslash (der trennt im
        /// Objektnamensraum Verzeichnisse) und kurz genug für `MAX_PATH`.
        #[test]
        fn download_mutex_names_are_derived_from_the_path() {
            let a = download_mutex_name(Path::new(r"C:\Users\x\AppData\Local\diktier\d.lock"));
            let b = download_mutex_name(Path::new(r"C:\Users\y\AppData\Local\diktier\d.lock"));
            assert_ne!(a, b, "verschiedene Pfade, verschiedene Sperren");
            assert!(a.starts_with(r"Local\FerberDiktier.v1.download."), "{a}");
            assert_eq!(a.len(), r"Local\FerberDiktier.v1.download.".len() + 64);
            assert!(
                a.rsplit('.')
                    .next()
                    .unwrap()
                    .chars()
                    .all(|c| c.is_ascii_hexdigit()),
                "{a}"
            );
            assert_ne!(a, INSTANCE_MUTEX);
        }

        /// Windows-Pfade sind case-insensitiv — zwei Schreibweisen desselben
        /// Pfades dürfen nicht zwei Sperren ergeben.
        #[test]
        fn the_same_path_in_another_case_is_the_same_mutex() {
            assert_eq!(
                download_mutex_name(Path::new(r"C:\Temp\Diktier\D.LOCK")),
                download_mutex_name(Path::new(r"c:\temp\diktier\d.lock"))
            );
        }
    }
}
