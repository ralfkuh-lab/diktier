//! Autostart-Eintrag (Spec §9).
//!
//! `--install-autostart` / `--remove-autostart` sind **idempotent**, der Pfad
//! ist das gequotete `current_exe()`, und es wird ausschließlich die eigene
//! Datei angefasst — fremde Einträge im selben Verzeichnis bleiben unberührt.
//!
//! Beide Modi laufen **vor** der Single-Instance-Sperre (§5.3) und loggen nur
//! nach stderr (§10, Ein-Writer-Regel für `diktier.log`).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::paths::{self, PathError};

#[derive(Debug, Error)]
pub enum AutostartError {
    #[error("Autostart-Pfad: {0}")]
    Path(#[from] PathError),
    #[error("Programmpfad nicht ermittelbar: {0}")]
    Exe(io::Error),
    #[error("Programmpfad ist nicht UTF-8: {0}")]
    ExeNotUtf8(PathBuf),
    /// In einer `.cmd` lässt sich ein `"` im Pfad nicht verlässlich quoten —
    /// cmd.exe kennt dafür kein Escape. NTFS erlaubt das Zeichen in
    /// Dateinamen ohnehin nicht; der Fall kann nur über exotische Geräte- oder
    /// Netzwerkpfade kommen und wird abgelehnt, statt eine kaputte Datei zu
    /// schreiben (Plan WP5).
    #[error("Programmpfad enthält Anführungszeichen: {0}")]
    ExeHasQuote(PathBuf),
    #[error("Autostart-Datei {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
}

impl AutostartError {
    /// §9: `1` fataler Laufzeitfehler, `2` Bedien-/Configfehler.
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Path(_) => 2,
            Self::Exe(_) | Self::ExeNotUtf8(_) | Self::Io { .. } => 1,
            Self::ExeHasQuote(_) => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallOutcome {
    /// Es gab noch keinen Eintrag.
    Created,
    /// Eigener Eintrag zeigte woandershin (verschobene portable Binary, §9).
    Updated,
    /// Schon exakt so vorhanden — der zweite `--install-autostart`.
    Unchanged,
}

impl InstallOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "angelegt",
            Self::Updated => "aktualisiert",
            Self::Unchanged => "unverändert",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveOutcome {
    Removed,
    /// Kein Eintrag da — laut §9 trotzdem Exit 0.
    NotPresent,
}

impl RemoveOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Removed => "entfernt",
            Self::NotPresent => "war nicht vorhanden",
        }
    }
}

/// Eintrag für das laufende Binary anlegen oder aktualisieren.
pub fn install() -> Result<(InstallOutcome, PathBuf), AutostartError> {
    let path = paths::autostart_path()?;
    let exe = current_exe()?;
    let outcome = install_at(&path, &exe)?;
    Ok((outcome, path))
}

/// Eintrag entfernen, falls vorhanden.
pub fn remove() -> Result<(RemoveOutcome, PathBuf), AutostartError> {
    let path = paths::autostart_path()?;
    let outcome = remove_at(&path)?;
    Ok((outcome, path))
}

/// Gibt es den eigenen Autostart-Eintrag? Für das Häkchen im Tray-Menü
/// (Phase 5, Paket F).
///
/// Bewusst `bool` statt `Result`: Der Aufrufer ist ein Menüaufbau, der keinen
/// Fehlerkanal hat. Ist der Pfad nicht ermittelbar, gibt es auch keinen
/// Eintrag — und der Klick darauf meldet den Fehler dann ordentlich.
pub fn is_installed() -> bool {
    paths::autostart_path()
        .map(|path| path.is_file())
        .unwrap_or(false)
}

fn current_exe() -> Result<PathBuf, AutostartError> {
    std::env::current_exe().map_err(AutostartError::Exe)
}

/// Kern der Installation, gegen ein Temp-`HOME` testbar.
pub fn install_at(path: &Path, exe: &Path) -> Result<InstallOutcome, AutostartError> {
    let wanted = entry_contents(exe)?;
    let existing = match fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(AutostartError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if existing.as_deref() == Some(wanted.as_str()) {
        return Ok(InstallOutcome::Unchanged);
    }
    write_atomic(path, &wanted)?;
    Ok(if existing.is_some() {
        InstallOutcome::Updated
    } else {
        InstallOutcome::Created
    })
}

pub fn remove_at(path: &Path) -> Result<RemoveOutcome, AutostartError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(RemoveOutcome::Removed),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(RemoveOutcome::NotPresent),
        Err(source) => Err(AutostartError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Inhalt des Eintrags: `.cmd` im Startup-Ordner (§9 nennt nur
/// „Startup-Ordner des Users").
///
/// **Ohne `/min`** (Plan WP5): Das Binary läuft im Windows-Subsystem, es gibt
/// kein Fenster, das minimiert werden könnte. Der leere Titel nach `start` ist
/// dagegen Pflicht — sonst deutet cmd.exe den gequoteten Pfad als Fenstertitel
/// und startet nichts.
pub fn entry_contents(exe: &Path) -> Result<String, AutostartError> {
    let exec = quote_exec(exe)?;
    Ok(format!("@echo off\r\nstart \"\" {exec}\r\n"))
}

/// §9: „Pfad = gequotetes `current_exe()`."
///
/// Die Quotierung genügt: `\` ist unter Windows das Pfadtrennzeichen und wird
/// innerhalb der Anführungszeichen nicht als Escape gelesen. Ein `"` im Pfad
/// ließe sich dagegen nicht schützen und wird abgelehnt.
pub fn quote_exec(exe: &Path) -> Result<String, AutostartError> {
    let raw = exe
        .to_str()
        .ok_or_else(|| AutostartError::ExeNotUtf8(exe.to_path_buf()))?;
    if raw.contains('"') {
        return Err(AutostartError::ExeHasQuote(exe.to_path_buf()));
    }
    Ok(format!("\"{raw}\""))
}

fn write_atomic(path: &Path, contents: &str) -> Result<(), AutostartError> {
    let parent = path.parent().ok_or_else(|| AutostartError::Io {
        path: path.to_path_buf(),
        source: io::Error::other("Autostart-Pfad hat kein Elternverzeichnis"),
    })?;
    let io_err = |source: io::Error| AutostartError::Io {
        path: path.to_path_buf(),
        source,
    };
    fs::create_dir_all(parent).map_err(io_err)?;
    let temp = path.with_extension("tmp");
    fs::write(&temp, contents).map_err(io_err)?;
    // Rename im selben Verzeichnis: ein halb geschriebener Eintrag wäre beim
    // nächsten Login ein kaputter Autostart.
    if let Err(err) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(io_err(err));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wonach genau ein eigener Eintrag in der erzeugten `.cmd` zu suchen ist
    /// — für die Regel aus §9: „eigenen Eintrag aktualisieren, nicht
    /// verdoppeln".
    const ENTRY_MARKER: &str = "start \"\" ";

    fn temp_home() -> (tempfile::TempDir, PathBuf) {
        let home = tempfile::tempdir().unwrap();
        let path = home
            .path()
            .join(".config")
            .join("autostart")
            .join(paths::AUTOSTART_NAME);
        (home, path)
    }

    #[test]
    fn install_is_idempotent() {
        let (_home, path) = temp_home();
        let exe = PathBuf::from("/opt/diktier/diktier");

        assert_eq!(install_at(&path, &exe).unwrap(), InstallOutcome::Created);
        let first = fs::read_to_string(&path).unwrap();
        assert_eq!(install_at(&path, &exe).unwrap(), InstallOutcome::Unchanged);
        let second = fs::read_to_string(&path).unwrap();
        assert_eq!(first, second, "zweiter Install darf nichts ändern");
    }

    #[test]
    fn install_updates_a_moved_binary_instead_of_duplicating() {
        let (_home, path) = temp_home();
        install_at(&path, Path::new("/opt/alt/diktier")).unwrap();
        assert_eq!(
            install_at(&path, Path::new("/opt/neu/diktier")).unwrap(),
            InstallOutcome::Updated
        );
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(
            text.matches(ENTRY_MARKER).count(),
            1,
            "genau ein eigener Eintrag in:\n{text}"
        );
        assert!(text.contains("/opt/neu/diktier"));
        assert!(!text.contains("/opt/alt/diktier"));
    }

    #[test]
    fn remove_is_idempotent_and_keeps_foreign_entries() {
        let (home, path) = temp_home();
        let foreign = path.with_file_name("fremd.desktop");

        install_at(&path, Path::new("/opt/diktier/diktier")).unwrap();
        fs::write(&foreign, "[Desktop Entry]\nExec=/usr/bin/fremd\n").unwrap();

        assert_eq!(remove_at(&path).unwrap(), RemoveOutcome::Removed);
        assert_eq!(remove_at(&path).unwrap(), RemoveOutcome::NotPresent);
        assert!(!path.exists());
        assert!(foreign.is_file(), "fremder Eintrag darf nicht verschwinden");
        assert!(home.path().is_dir());
    }

    #[test]
    fn install_leaves_no_temp_file_behind() {
        let (_home, path) = temp_home();
        install_at(&path, Path::new("/opt/diktier/diktier")).unwrap();
        let dir = path.parent().unwrap();
        let names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
    }

    /// §9: gequoteter Pfad; kein `/min`; ein `"` im Pfad wird abgelehnt, statt
    /// eine kaputte `.cmd` zu schreiben (Plan WP5).
    #[test]
    fn windows_entry_quotes_the_path_and_stays_unminimized() {
        let text = entry_contents(Path::new(r"C:\Program Files\a b\diktier.exe")).unwrap();
        assert!(
            text.contains(r#"start "" "C:\Program Files\a b\diktier.exe""#),
            "{text}"
        );
        assert!(!text.contains("/min"), "{text}");
        assert!(text.ends_with("\r\n"), "{text:?}");
    }

    #[test]
    fn windows_rejects_a_path_with_a_quote() {
        let err = quote_exec(Path::new("C:\\a\"b\\diktier.exe")).unwrap_err();
        assert!(matches!(err, AutostartError::ExeHasQuote(_)), "{err}");
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn exit_codes_follow_spec() {
        assert_eq!(
            AutostartError::Path(PathError::Missing("x".into())).exit_code(),
            2
        );
        assert_eq!(
            AutostartError::Io {
                path: PathBuf::from("/x"),
                source: io::Error::other("x"),
            }
            .exit_code(),
            1
        );
    }
}
