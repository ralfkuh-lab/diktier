//! Benutzerpfade außerhalb der Config (Spec §5.3, §9, §10).
//!
//! Die Config liegt in [`crate::config::config_path`], die Modelle in
//! [`crate::download::model_dir`]. Hier stehen die übrigen drei Orte:
//! Zustandsverzeichnis (Datei-Log, Lock-Fallback), Laufzeitverzeichnis
//! (Single-Instance-Lock) und der Autostart-Eintrag.
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

/// Zustandsverzeichnis des Users.
///
/// Linux: `$XDG_STATE_HOME/diktier`, Fallback `~/.local/state/diktier` (§10).
/// Windows: `%LOCALAPPDATA%\diktier`.
pub fn state_dir() -> Result<PathBuf, PathError> {
    #[cfg(target_os = "linux")]
    {
        if let Some(base) = non_empty_env("XDG_STATE_HOME") {
            let base = PathBuf::from(base);
            // XDG: relative Angaben sind ungültig und werden ignoriert.
            if base.is_absolute() {
                return Ok(base.join("diktier"));
            }
        }
        let home = home_dir()?;
        Ok(home.join(".local").join("state").join("diktier"))
    }
    #[cfg(windows)]
    {
        let local = non_empty_env("LOCALAPPDATA").ok_or_else(|| {
            PathError::Missing("Umgebungsvariable LOCALAPPDATA ist nicht gesetzt".into())
        })?;
        Ok(PathBuf::from(local).join("diktier"))
    }
}

/// `$XDG_RUNTIME_DIR` (Linux), wenn gesetzt und absolut. Kein Fallback — den
/// wählt [`crate::single_instance`] selbst (§5.3).
#[cfg(target_os = "linux")]
pub fn runtime_dir() -> Option<PathBuf> {
    let base = PathBuf::from(non_empty_env("XDG_RUNTIME_DIR")?);
    base.is_absolute().then_some(base)
}

/// Datei-Log des Daemons (§10). Linux `~/.local/state/diktier/diktier.log`.
pub fn log_path() -> Result<PathBuf, PathError> {
    Ok(state_dir()?.join(LOG_NAME))
}

pub const LOG_NAME: &str = "diktier.log";
/// Eine Backup-Datei, kein Ringpuffer (§10).
pub const LOG_BACKUP_NAME: &str = "diktier.log.1";
/// §10: „erreicht `diktier.log` 2 MiB, atomar nach `diktier.log.1`".
pub const LOG_LIMIT_BYTES: u64 = 2 * 1024 * 1024;

/// Autostart-Eintrag (§9). Linux: `~/.config/autostart/diktier.desktop`.
#[cfg(target_os = "linux")]
pub fn autostart_path() -> Result<PathBuf, PathError> {
    let base = match non_empty_env("XDG_CONFIG_HOME").map(PathBuf::from) {
        Some(base) if base.is_absolute() => base,
        _ => home_dir()?.join(".config"),
    };
    Ok(base.join("autostart").join(AUTOSTART_NAME))
}

/// Windows: Startup-Ordner des Users (§9).
#[cfg(windows)]
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

#[cfg(target_os = "linux")]
pub const AUTOSTART_NAME: &str = "diktier.desktop";
#[cfg(windows)]
pub const AUTOSTART_NAME: &str = "diktier.cmd";

#[cfg(target_os = "linux")]
fn home_dir() -> Result<PathBuf, PathError> {
    let home = non_empty_env("HOME")
        .ok_or_else(|| PathError::Missing("Umgebungsvariable HOME ist nicht gesetzt".into()))?;
    let home = PathBuf::from(home);
    if !home.is_absolute() {
        return Err(PathError::Missing("HOME ist kein absoluter Pfad".into()));
    }
    Ok(home)
}

fn non_empty_env(key: &str) -> Option<std::ffi::OsString> {
    let value = std::env::var_os(key)?;
    (!value.is_empty()).then_some(value)
}

/// Verzeichnis anlegen, falls nötig — Rechte `0700` unter Linux.
///
/// Zustands- und Laufzeitverzeichnis enthalten zwar keine Transkripte (§10),
/// aber es gibt keinen Grund, das Log für andere Nutzer lesbar zu machen.
pub fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::DirBuilderExt;
        match std::fs::DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(dir)
        {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(err) => Err(err),
        }
    }
    #[cfg(windows)]
    {
        std::fs::create_dir_all(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_dir_follows_spec() {
        let dir = state_dir().unwrap();
        #[cfg(target_os = "linux")]
        assert!(
            dir.ends_with(".local/state/diktier") || dir.ends_with("diktier"),
            "{}",
            dir.display()
        );
        assert!(dir.is_absolute());
    }

    #[test]
    fn log_path_is_in_state_dir() {
        let path = log_path().unwrap();
        assert_eq!(path.file_name().unwrap(), LOG_NAME);
        assert_eq!(path.parent().unwrap(), state_dir().unwrap());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn autostart_path_follows_spec() {
        let path = autostart_path().unwrap();
        assert!(
            path.ends_with("autostart/diktier.desktop"),
            "{}",
            path.display()
        );
    }

    #[test]
    fn log_limit_is_two_mib() {
        assert_eq!(LOG_LIMIT_BYTES, 2_097_152);
    }

    #[test]
    fn create_private_dir_is_idempotent_and_private() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("a").join("b");
        create_private_dir(&dir).unwrap();
        create_private_dir(&dir).unwrap();
        assert!(dir.is_dir());
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "{mode:o}");
        }
    }
}
