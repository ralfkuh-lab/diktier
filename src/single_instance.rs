//! Single-Instance und Download-Lock (Spec §5.3, §6.3).
//!
//! Beides läuft über Named Mutexe im `Local\`-Namensraum, hinter
//! [`Acquire`]/[`PathLock`]. Der Namensraum ist bereits **pro interaktiver
//! Session** (§5.3, präzisiert 2026-08-27), deshalb steckt kein User-Hash im
//! Namen und eine zweite RDP-Session bekommt ihre eigene Instanz.
//!
//! Kein PID-File und **keine** Sperrdatei: `CreateMutexW` meldet
//! `ERROR_ALREADY_EXISTS` auch dann, wenn der zweite Aufruf aus demselben
//! Prozess kommt — genau die Semantik, die die Tests brauchen. Und das Objekt
//! stirbt mit dem letzten Handle, also auch bei einem hart abgeschossenen
//! Prozess.
//!
//! Wer die Sperre nimmt, steht in §5.3: **nur** `diktier` bzw.
//! `diktier --foreground`. `--help`, `--version`, `--install-autostart` und
//! `--remove-autostart` laufen davor und fordern sie nie an.

// `Acquire::held` und die Namenskonstanten benutzen nur die Tests bzw. der
// `win`-Zweig — wie in `download.rs` ist das kein toter Code.
#![allow(dead_code)]

use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::paths::{self, PathError};

#[derive(Debug, Error)]
pub enum LockError {
    /// `CreateMutexW` hat kein Handle geliefert. Häufigster Fall ist
    /// `ERROR_INVALID_HANDLE` — dann trägt ein **anderer Objekttyp** bereits
    /// diesen Namen, und das ist kein „läuft schon".
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
/// Der Pfad ist nur der **Schlüssel** für den Mutex-Namen, eine Datei entsteht
/// nicht. Das Objekt wird beim `Drop` freigegeben.
#[derive(Debug)]
pub struct PathLock {
    _mutex: win::MutexLock,
    path: PathBuf,
}

impl PathLock {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Die gehaltene Single-Instance-Sperre: der eine Named Mutex aus §5.3.
#[derive(Debug)]
pub struct InstanceLock {
    mutex: win::MutexLock,
}

impl InstanceLock {
    /// Für die Startzeile im Log — der Mutex-Name (Plan WP5).
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

/// Single-Instance-Sperre des Daemons nach §5.3: **ein** Named Mutex mit festem
/// Namen, kein Kandidatenpfad und keine Sperrdatei. `on_problem` bleibt
/// ungenutzt — es gibt keinen Ort, auf den ausgewichen werden könnte.
pub fn acquire_instance_lock(
    _on_problem: &mut dyn FnMut(String),
) -> Result<InstanceAcquire, LockError> {
    match win::create(win::INSTANCE_MUTEX)? {
        win::Taken::Held(mutex) => Ok(InstanceAcquire::Held(InstanceLock { mutex })),
        win::Taken::Busy => Ok(InstanceAcquire::Busy),
    }
}

/// Ort des per-user Download-Locks aus §6.3. Genommen wird er in
/// [`crate::download::download_model_locked`], damit der Test den Parallelstart
/// nachstellen kann.
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

/// §5.3: das Zustandsverzeichnis. Fehlt es, ist das ein Pfadfehler.
fn lock_candidates(name: &str) -> Result<Vec<PathBuf>, LockError> {
    Ok(vec![paths::state_dir()?.join(name)])
}

/// Der Pfad wird zum Mutex-Namen (§6.3-Download-Lock, Plan WP5).
///
/// Es entsteht **keine** Datei — der `Local\`-Mutex trägt die Sperre allein.
/// Derselbe Pfad zweimal ergibt `Busy`, auch innerhalb eines Prozesses, und das
/// Fallenlassen gibt die Sperre frei.
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

/// Named Mutexe im `Local\`-Namensraum (§5.3, Plan WP5).
///
/// Ein Mutex-Objekt lebt, solange irgendein Prozess ein Handle darauf hält;
/// `CloseHandle` im `Drop` gibt es frei. Gewartet wird nie — allein die
/// **Existenz** des Namens ist die Sperre, und `CreateMutexW` meldet sie über
/// `ERROR_ALREADY_EXISTS`. Das gilt auch prozessintern, deshalb stellen die
/// Tests einen zweiten Start korrekt nach.
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

        // Zweiter Versuch im selben Prozess: `CreateMutexW` meldet den Namen
        // auch prozessintern als vorhanden — das verhält sich wie ein zweiter
        // Start.
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

        // Wie nach einem hart abgeschossenen Prozess: das letzte Handle ist weg,
        // die Sperre frei.
        assert!(
            matches!(try_lock(&path).unwrap(), Acquire::Held(_)),
            "nach dem Freigeben muss der Lock wieder zu haben sein"
        );
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
    fn candidates_are_absolute_and_in_the_state_dir() {
        let candidates = lock_candidates(INSTANCE_LOCK_NAME).unwrap();
        assert!(!candidates.is_empty());
        for path in &candidates {
            assert_eq!(path.file_name().unwrap(), INSTANCE_LOCK_NAME);
            assert!(path.is_absolute());
            assert_eq!(path.parent().unwrap(), paths::state_dir().unwrap());
        }
    }

    /// Namensbildung des Mutex-Zweigs (Plan WP5) — reine Logik, kein Win32.
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
