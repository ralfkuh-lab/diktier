//! Modell-Artefakte (Spec §6.3). Phase 0/1: Manifest + Existenz/Größe, kein Netz.
#![allow(dead_code)]

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

const MANIFEST_TOML: &str = include_str!("models.toml");

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("Artefakt-Manifest: {0}")]
    Manifest(String),
    #[error("Modellpfad: {0}")]
    Path(String),
    #[error("Download ist noch nicht implementiert")]
    NotImplemented,
    #[error("Modellartefakt fehlt: {0}")]
    Missing(PathBuf),
    #[error("Modellartefakt {path} hat {actual} Bytes, erwartet {expected}")]
    SizeMismatch {
        path: PathBuf,
        actual: u64,
        expected: u64,
    },
    #[error("Modellartefakt: {0}")]
    Io(#[from] io::Error),
    #[error("Modellartefakt {path}: SHA-256 {actual} stimmt nicht mit Manifest {expected}")]
    HashMismatch {
        path: PathBuf,
        actual: String,
        expected: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ArtifactManifest {
    pub key: String,
    pub files: Vec<Artifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Artifact {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
    #[serde(default)]
    pub url: String,
}

pub fn load_manifest() -> Result<ArtifactManifest, DownloadError> {
    toml::from_str(MANIFEST_TOML).map_err(|e| DownloadError::Manifest(e.to_string()))
}

/// Linux: `~/.local/share/diktier/models/<key>/`.
/// Windows: `%LOCALAPPDATA%\diktier\models\<key>\`.
pub fn model_dir(key: &str) -> Result<PathBuf, DownloadError> {
    #[cfg(target_os = "linux")]
    {
        let home = std::env::var_os("HOME").ok_or_else(|| {
            DownloadError::Path("Umgebungsvariable HOME ist nicht gesetzt".into())
        })?;
        Ok(PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("diktier")
            .join("models")
            .join(key))
    }
    #[cfg(windows)]
    {
        let local = std::env::var_os("LOCALAPPDATA").ok_or_else(|| {
            DownloadError::Path("Umgebungsvariable LOCALAPPDATA ist nicht gesetzt".into())
        })?;
        Ok(PathBuf::from(local)
            .join("diktier")
            .join("models")
            .join(key))
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        compile_error!("diktier unterstützt nur Linux und Windows");
    }
}

/// Existenz und Dateigröße gegen das Manifest. SHA-256 nur im Download-Pfad
/// und in `verify_artifacts_sha256` (stt-smoke / Phase 3).
pub fn check_artifacts(dir: &Path, manifest: &ArtifactManifest) -> Result<(), DownloadError> {
    for file in &manifest.files {
        let path = dir.join(&file.name);
        if !path.is_file() {
            return Err(DownloadError::Missing(path));
        }
        let actual = std::fs::metadata(&path)?.len();
        if actual != file.bytes {
            return Err(DownloadError::SizeMismatch {
                path,
                actual,
                expected: file.bytes,
            });
        }
    }
    Ok(())
}

/// SHA-256-Vollprüfung, streaming.
///
/// Pflicht im Download-Pfad (Spec §6.3, Phase 3). Der normale Start prüft nur
/// Existenz+Größe (`check_artifacts`) — Kaltstart-Budget. stt-smoke ruft diese
/// Routine einmal auf.
pub fn verify_artifacts_sha256(
    dir: &Path,
    manifest: &ArtifactManifest,
) -> Result<(), DownloadError> {
    for file in &manifest.files {
        let path = dir.join(&file.name);
        if !path.is_file() {
            return Err(DownloadError::Missing(path));
        }
        let actual = sha256_file(&path)?;
        if !actual.eq_ignore_ascii_case(&file.sha256) {
            return Err(DownloadError::HashMismatch {
                path,
                actual,
                expected: file.sha256.clone(),
            });
        }
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, DownloadError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0_u8; 8192];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Phase 0/1: kein Netz, kein Schreiben.
pub fn download_model(_manifest: &ArtifactManifest) -> Result<(), DownloadError> {
    Err(DownloadError::NotImplemented)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_MODEL;
    use sha2::{Digest, Sha256};

    #[test]
    fn golden_set_matches_spec() {
        let manifest = load_manifest().unwrap();
        assert_eq!(manifest.key, DEFAULT_MODEL);
        const REV: &str = "8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce";
        const BASE: &str = "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve";
        let expected = [
            (
                "encoder-model.int8.onnx",
                652_183_999_u64,
                "6139d2fa7e1b086097b277c7149725edbab89cc7c7ae64b23c741be4055aff09",
            ),
            (
                "decoder_joint-model.int8.onnx",
                18_202_004,
                "eea7483ee3d1a30375daedc8ed83e3960c91b098812127a0d99d1c8977667a70",
            ),
            (
                "vocab.txt",
                93_939,
                "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
            ),
            (
                "config.json",
                97,
                "666903c76b9798caf2c210afd4f6cd60b08a8dbf9800ec8d7a3bc0d2148ac466",
            ),
        ];
        assert_eq!(manifest.files.len(), expected.len());
        for (file, (name, bytes, sha)) in manifest.files.iter().zip(expected) {
            assert_eq!(file.name, name);
            assert_eq!(file.bytes, bytes);
            assert_eq!(file.sha256, sha);
            assert_eq!(file.url, format!("{BASE}/{REV}/{name}"));
        }
    }

    #[test]
    fn download_is_stubbed() {
        let manifest = load_manifest().unwrap();
        let err = download_model(&manifest).unwrap_err();
        assert!(matches!(err, DownloadError::NotImplemented));
    }

    #[test]
    fn check_artifacts_reports_missing_and_size() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = load_manifest().unwrap();
        let err = check_artifacts(dir.path(), &manifest).unwrap_err();
        assert!(matches!(err, DownloadError::Missing(_)));

        let first = &manifest.files[0];
        std::fs::write(dir.path().join(&first.name), vec![0_u8; 4]).unwrap();
        let err = check_artifacts(dir.path(), &manifest).unwrap_err();
        match err {
            DownloadError::SizeMismatch {
                actual, expected, ..
            } => {
                assert_eq!(actual, 4);
                assert_eq!(expected, first.bytes);
            }
            other => panic!("expected SizeMismatch, got {other:?}"),
        }
    }

    #[test]
    fn verify_sha256_detects_wrong_content_same_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tiny.bin");
        std::fs::write(&path, b"aaaa").unwrap();
        let expected = format!("{:x}", Sha256::digest(b"aaaa"));
        let good = ArtifactManifest {
            key: "tiny".into(),
            files: vec![Artifact {
                name: "tiny.bin".into(),
                bytes: 4,
                sha256: expected.clone(),
                url: String::new(),
            }],
        };
        verify_artifacts_sha256(dir.path(), &good).unwrap();

        std::fs::write(&path, b"bbbb").unwrap();
        let err = verify_artifacts_sha256(dir.path(), &good).unwrap_err();
        match err {
            DownloadError::HashMismatch {
                actual, expected, ..
            } => {
                assert_ne!(actual, expected);
                assert_eq!(actual, format!("{:x}", Sha256::digest(b"bbbb")));
            }
            other => panic!("expected HashMismatch, got {other:?}"),
        }
    }

    #[test]
    fn model_dir_matches_spec() {
        let dir = model_dir(DEFAULT_MODEL).unwrap();
        #[cfg(target_os = "linux")]
        {
            assert!(dir.ends_with(format!(".local/share/diktier/models/{DEFAULT_MODEL}")));
        }
        #[cfg(windows)]
        {
            assert!(
                dir.ends_with(format!("diktier\\models\\{DEFAULT_MODEL}"))
                    || dir.ends_with(format!("diktier/models/{DEFAULT_MODEL}"))
            );
        }
    }
}
