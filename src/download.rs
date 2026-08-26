//! Modell-Artefakte (Spec §6.3). Phase 0: Manifest laden, kein Netz.
#![allow(dead_code)]

use std::path::PathBuf;

use serde::Deserialize;
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
    #[cfg(unix)]
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
    #[cfg(not(any(unix, windows)))]
    {
        compile_error!("diktier unterstützt nur Unix und Windows");
    }
}

/// Phase 0: kein Netz, kein Schreiben.
pub fn download_model(_manifest: &ArtifactManifest) -> Result<(), DownloadError> {
    Err(DownloadError::NotImplemented)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_MODEL;

    #[test]
    fn golden_set_matches_spec() {
        let manifest = load_manifest().unwrap();
        assert_eq!(manifest.key, DEFAULT_MODEL);
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
            assert!(
                file.url.is_empty(),
                "{}: URL erst im STT-Spike, got {:?}",
                file.name,
                file.url
            );
        }
    }

    #[test]
    fn download_is_stubbed() {
        let manifest = load_manifest().unwrap();
        let err = download_model(&manifest).unwrap_err();
        assert!(matches!(err, DownloadError::NotImplemented));
    }

    #[test]
    fn model_dir_matches_spec() {
        let dir = model_dir(DEFAULT_MODEL).unwrap();
        #[cfg(unix)]
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
