//! Benutzerpfade außerhalb der Config (Spec §5.3, §9, §10).
//!
//! Die Config liegt in [`crate::config::config_path`], die Modelle in
//! [`crate::download::model_dir`]. Hier stehen die übrigen zwei Orte:
//! Zustandsverzeichnis (Datei-Log) und der Autostart-Eintrag.
//!
//! Alle Funktionen lesen die Umgebung **bei jedem Aufruf** und nehmen den
//! Basispfad als Parameter, wo die Tests ihn brauchen — kein `set_var` in
//! Tests (das ist seit Rust 2024 `unsafe` und bei parallelen Tests falsch).

// `LOG_BACKUP_NAME` steht hier als Name der Rotationsdatei und wird nur von
// den Tests gelesen — der Rotationspfad selbst entsteht in `logging.rs`.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PathError {
    #[error("Pfad: {0}")]
    Missing(String),
}

/// Zustandsverzeichnis des Users: `%LOCALAPPDATA%\diktier` (§10).
pub fn state_dir() -> Result<PathBuf, PathError> {
    let local = non_empty_env("LOCALAPPDATA").ok_or_else(|| {
        PathError::Missing("Umgebungsvariable LOCALAPPDATA ist nicht gesetzt".into())
    })?;
    Ok(PathBuf::from(local).join("diktier"))
}

/// Datei-Log des Daemons (§10): `%LOCALAPPDATA%\diktier\diktier.log`.
pub fn log_path() -> Result<PathBuf, PathError> {
    Ok(state_dir()?.join(LOG_NAME))
}

pub const LOG_NAME: &str = "diktier.log";
/// Eine Backup-Datei, kein Ringpuffer (§10).
pub const LOG_BACKUP_NAME: &str = "diktier.log.1";
/// §10: „erreicht `diktier.log` 2 MiB, atomar nach `diktier.log.1`".
pub const LOG_LIMIT_BYTES: u64 = 2 * 1024 * 1024;

/// Autostart-Eintrag (§9): Startup-Ordner des Users.
pub fn autostart_path() -> Result<PathBuf, PathError> {
    let appdata = non_empty_env("APPDATA")
        .ok_or_else(|| PathError::Missing("Umgebungsvariable APPDATA ist nicht gesetzt".into()))?;
    Ok(PathBuf::from(appdata)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Startup")
        .join(AUTOSTART_NAME))
}

pub const AUTOSTART_NAME: &str = "diktier.cmd";

fn non_empty_env(key: &str) -> Option<std::ffi::OsString> {
    let value = std::env::var_os(key)?;
    (!value.is_empty()).then_some(value)
}

/// Verzeichnis anlegen, falls nötig.
///
/// Das Zustandsverzeichnis liegt unter `%LOCALAPPDATA%` und erbt damit die
/// ACL des Benutzerprofils — andere Nutzer kommen ohnehin nicht heran.
pub fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_dir_follows_spec() {
        let dir = state_dir().unwrap();
        assert!(dir.ends_with("diktier"), "{}", dir.display());
        assert!(dir.is_absolute());
    }

    #[test]
    fn log_path_is_in_state_dir() {
        let path = log_path().unwrap();
        assert_eq!(path.file_name().unwrap(), LOG_NAME);
        assert_eq!(path.parent().unwrap(), state_dir().unwrap());
    }

    #[test]
    fn autostart_path_follows_spec() {
        let path = autostart_path().unwrap();
        assert!(path.ends_with(r"Startup\diktier.cmd"), "{}", path.display());
    }

    #[test]
    fn log_limit_is_two_mib() {
        assert_eq!(LOG_LIMIT_BYTES, 2_097_152);
    }

    #[test]
    fn create_private_dir_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("a").join("b");
        create_private_dir(&dir).unwrap();
        create_private_dir(&dir).unwrap();
        assert!(dir.is_dir());
    }
}
