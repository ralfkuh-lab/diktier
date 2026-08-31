//! Modell-Artefakte (Spec §6.3): Manifest, Prüfung und Download.
//!
//! Download je Datei nach `<name>.part`, Größe **und** SHA-256 gegen das
//! Manifest, dann atomar umbenennen; zuletzt der Marker `COMPLETE`. Ein
//! Hashfehler löscht nur die `.part` — nichts Halbes wird je zur Zieldatei
//! (§6.3, §13).
//!
//! Der Netzzugriff steckt hinter [`Transport`], damit die Tests mit einem
//! lokalen Fake laufen (§13: Abbruch, falsche Größe, falscher Hash, atomarer
//! Abschluss, Parallelstart).

#![allow(dead_code)]

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::single_instance::{self, Acquire};

const MANIFEST_TOML: &str = include_str!("models.toml");

/// Marker, den der Download **zuletzt** schreibt (§6.3).
pub const COMPLETE_MARKER: &str = "COMPLETE";

/// Endung der unfertigen Datei (§6.3).
const PART_SUFFIX: &str = ".part";

/// Puffer je Lesevorgang. Groß genug, dass 650 MB nicht in Syscalls ersticken.
const CHUNK: usize = 256 * 1024;

/// Fortschritt wird höchstens alle so vielen Bytes gemeldet — das Log soll den
/// Download begleiten, nicht zumüllen (§6.3: „Fortschritt als Logzeilen").
const PROGRESS_STEP: u64 = 16 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("Artefakt-Manifest: {0}")]
    Manifest(String),
    #[error("Modellpfad: {0}")]
    Path(String),
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
    #[error("Download von {url} gescheitert: {message}")]
    Transport { url: String, message: String },
    #[error("Download abgebrochen")]
    Cancelled,
    #[error("Ein anderer Prozess lädt die Modellartefakte bereits ({0})")]
    Busy(PathBuf),
    #[error("Download-Sperre: {0}")]
    Lock(String),
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

/// `%LOCALAPPDATA%\diktier\models\<key>\`.
pub fn model_dir(key: &str) -> Result<PathBuf, DownloadError> {
    let local = std::env::var_os("LOCALAPPDATA").ok_or_else(|| {
        DownloadError::Path("Umgebungsvariable LOCALAPPDATA ist nicht gesetzt".into())
    })?;
    Ok(PathBuf::from(local)
        .join("diktier")
        .join("models")
        .join(key))
}

/// Existenz und Dateigröße gegen das Manifest. SHA-256 nur im Download-Pfad
/// und in `verify_artifacts_sha256` (stt-smoke / Phase 3).
///
/// Der `COMPLETE`-Marker aus §6.3 wird hier **bewusst nicht** verlangt (Owner,
/// Phase 3d): §6.3 schreibt ihn dem Download vor, macht ihn aber nicht zur
/// Startbedingung. Er bleibt reine Download-Quittung — sonst hielte diese
/// Prüfung jedes von Hand hierher kopierte Golden Set für unvollständig und
/// löste einen 640-MiB-Download aus. Startprüfung bleibt Existenz + Größe.
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

/// Pfad des `COMPLETE`-Markers (§6.3).
pub fn complete_marker(dir: &Path) -> PathBuf {
    dir.join(COMPLETE_MARKER)
}

/// Netzzugriff hinter einem Trait — die Tests setzen einen lokalen Fake ein.
pub trait Transport: Send + Sync {
    /// Body als Stream. Die Größe kennt der Aufrufer aus dem Manifest, deshalb
    /// braucht es kein `Content-Length` aus der Antwort.
    fn get(&self, url: &str) -> Result<Box<dyn Read + Send>, DownloadError>;
}

/// Fortschritt eines laufenden Downloads (§6.3: „Fortschritt als Logzeilen").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress<'a> {
    /// Datei ist schon vollständig und korrekt da — nichts zu tun.
    Skipped {
        name: &'a str,
        index: usize,
        total: usize,
    },
    Started {
        name: &'a str,
        index: usize,
        total: usize,
        bytes: u64,
    },
    /// Zwischenstand, gedrosselt auf [`PROGRESS_STEP`].
    Bytes {
        name: &'a str,
        done: u64,
        bytes: u64,
    },
    /// Größe und SHA-256 geprüft, Datei umbenannt.
    Verified {
        name: &'a str,
        index: usize,
        total: usize,
    },
}

/// Alle fehlenden Artefakte laden (§6.3).
///
/// Vorhandene, in Größe **und** Hash korrekte Dateien werden übersprungen —
/// nach einem abgebrochenen Download muss nicht alles neu geladen werden.
/// `cancel` bricht zwischen zwei Blöcken ab (Quit-Pfad).
pub fn download_model(
    dir: &Path,
    manifest: &ArtifactManifest,
    transport: &dyn Transport,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress<'_>),
) -> Result<(), DownloadError> {
    std::fs::create_dir_all(dir)?;
    let total = manifest.files.len();

    for (i, file) in manifest.files.iter().enumerate() {
        let index = i + 1;
        if cancel.load(Ordering::Relaxed) {
            return Err(DownloadError::Cancelled);
        }
        let target = dir.join(&file.name);
        if is_already_good(&target, file)? {
            progress(Progress::Skipped {
                name: &file.name,
                index,
                total,
            });
            continue;
        }
        progress(Progress::Started {
            name: &file.name,
            index,
            total,
            bytes: file.bytes,
        });
        download_one(dir, file, transport, cancel, progress)?;
        progress(Progress::Verified {
            name: &file.name,
            index,
            total,
        });
    }

    // §6.3: „zuletzt Marker COMPLETE schreiben." Erst wenn wirklich alle vier
    // Dateien geprüft an ihrem Platz liegen.
    write_marker(dir, &manifest.key)?;
    Ok(())
}

/// [`download_model`] mit dem per-user Download-Lock aus §6.3.
///
/// Ein zweiter Prozess bekommt [`DownloadError::Busy`] statt in dasselbe
/// Verzeichnis zu schreiben.
pub fn download_model_locked(
    lock_path: &Path,
    dir: &Path,
    manifest: &ArtifactManifest,
    transport: &dyn Transport,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress<'_>),
) -> Result<(), DownloadError> {
    let lock = match single_instance::try_lock(lock_path) {
        Ok(Acquire::Held(lock)) => lock,
        Ok(Acquire::Busy) => return Err(DownloadError::Busy(lock_path.to_path_buf())),
        Err(err) => return Err(DownloadError::Lock(err.to_string())),
    };
    let result = download_model(dir, manifest, transport, cancel, progress);
    drop(lock);
    result
}

/// Vorhandene Datei: Größe **und** Hash müssen stimmen, sonst wird neu geladen.
fn is_already_good(target: &Path, file: &Artifact) -> Result<bool, DownloadError> {
    if !target.is_file() {
        return Ok(false);
    }
    if std::fs::metadata(target)?.len() != file.bytes {
        return Ok(false);
    }
    Ok(sha256_file(target)?.eq_ignore_ascii_case(&file.sha256))
}

fn download_one(
    dir: &Path,
    file: &Artifact,
    transport: &dyn Transport,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress<'_>),
) -> Result<(), DownloadError> {
    if file.url.is_empty() {
        return Err(DownloadError::Manifest(format!(
            "{}: keine Download-URL im Manifest",
            file.name
        )));
    }
    let part = dir.join(format!("{}{PART_SUFFIX}", file.name));
    let target = dir.join(&file.name);

    let outcome = stream_to_part(&part, file, transport, cancel, progress);
    let (done, digest) = match outcome {
        Ok(pair) => pair,
        Err(err) => {
            // §6.3: Bei jedem Fehler bleibt nur die `.part` auf der Strecke.
            let _ = std::fs::remove_file(&part);
            return Err(err);
        }
    };

    if done != file.bytes {
        let _ = std::fs::remove_file(&part);
        return Err(DownloadError::SizeMismatch {
            path: target,
            actual: done,
            expected: file.bytes,
        });
    }
    if !digest.eq_ignore_ascii_case(&file.sha256) {
        // §6.3: „Hashfehler: nur `.part` löschen." Kein Retry in derselben
        // Sitzung — der Kern geht in `error`, Retry braucht Neustart.
        let _ = std::fs::remove_file(&part);
        return Err(DownloadError::HashMismatch {
            path: target,
            actual: digest,
            expected: file.sha256.clone(),
        });
    }

    // Atomar: Rename im selben Verzeichnis. Ab hier ist die Datei gültig.
    std::fs::rename(&part, &target).map_err(|err| {
        let _ = std::fs::remove_file(&part);
        DownloadError::Io(err)
    })?;
    Ok(())
}

/// Body in die `.part` schreiben. Rückgabe: (geschriebene Bytes, SHA-256).
fn stream_to_part(
    part: &Path,
    file: &Artifact,
    transport: &dyn Transport,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress<'_>),
) -> Result<(u64, String), DownloadError> {
    let mut reader = transport.get(&file.url)?;
    let mut out = io::BufWriter::new(create_part(part)?);
    let mut hasher = Sha256::new();
    let mut buf = vec![0_u8; CHUNK];
    let mut done: u64 = 0;
    let mut next_report = PROGRESS_STEP;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(DownloadError::Cancelled);
        }
        let read = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => {
                return Err(DownloadError::Transport {
                    url: file.url.clone(),
                    message: err.to_string(),
                });
            }
        };
        // Mehr als erwartet ist genauso falsch wie zu wenig — hier abbrechen,
        // statt die Platte vollzuschreiben.
        done += read as u64;
        if done > file.bytes {
            return Err(DownloadError::SizeMismatch {
                path: part.to_path_buf(),
                actual: done,
                expected: file.bytes,
            });
        }
        hasher.update(&buf[..read]);
        out.write_all(&buf[..read])?;
        if done >= next_report {
            progress(Progress::Bytes {
                name: &file.name,
                done,
                bytes: file.bytes,
            });
            next_report = done + PROGRESS_STEP;
        }
    }

    let mut out = out.into_inner().map_err(|err| {
        DownloadError::Io(io::Error::other(format!("Puffer nicht geschrieben: {err}")))
    })?;
    out.flush()?;
    // Ohne `sync_all` könnte nach einem Stromausfall eine leere Datei mit
    // gültigem Namen dastehen.
    out.sync_all()?;
    Ok((done, format!("{:x}", hasher.finalize())))
}

fn create_part(path: &Path) -> io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    options.open(path)
}

fn write_marker(dir: &Path, key: &str) -> Result<(), DownloadError> {
    let temp = dir.join(format!("{COMPLETE_MARKER}{PART_SUFFIX}"));
    std::fs::write(&temp, format!("{key}\n"))?;
    if let Err(err) = std::fs::rename(&temp, complete_marker(dir)) {
        let _ = std::fs::remove_file(&temp);
        return Err(DownloadError::Io(err));
    }
    Ok(())
}

/// HTTPS-Transport für den echten Download (§6.3: immutable Hugging-Face-URLs).
pub struct HttpTransport {
    agent: ureq::Agent,
}

impl HttpTransport {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(30)))
            // Kein globales Timeout: 650 MB dürfen dauern. Der Body-Timeout ist
            // die Reißleine gegen eine Verbindung, die nie endet.
            .timeout_recv_body(Some(Duration::from_secs(30 * 60)))
            .user_agent(concat!("diktier/", env!("CARGO_PKG_VERSION")))
            .build();
        Self {
            agent: config.into(),
        }
    }
}

impl Default for HttpTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for HttpTransport {
    fn get(&self, url: &str) -> Result<Box<dyn Read + Send>, DownloadError> {
        let response = self
            .agent
            .get(url)
            .call()
            .map_err(|err| DownloadError::Transport {
                url: url.to_string(),
                message: err.to_string(),
            })?;
        Ok(Box::new(response.into_body().into_reader()))
    }
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
        assert!(
            dir.ends_with(format!("diktier\\models\\{DEFAULT_MODEL}"))
                || dir.ends_with(format!("diktier/models/{DEFAULT_MODEL}"))
        );
    }

    // ------------------------------------------- Download mit Fake-Transport
    // §13: „Download: lokaler Fake-Transport — Abbruch, falsche Größe, falscher
    // Hash, atomarer Abschluss, Parallelstart."

    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    /// Antwort des Fakes: Nutzdaten und optional ein Abriss mittendrin.
    #[derive(Clone)]
    struct FakeBody {
        data: Vec<u8>,
        /// Nach so vielen gelieferten Bytes bricht die „Verbindung" ab.
        fail_after: Option<usize>,
    }

    impl FakeBody {
        fn ok(data: &[u8]) -> Self {
            Self {
                data: data.to_vec(),
                fail_after: None,
            }
        }

        fn cut_after(data: &[u8], n: usize) -> Self {
            Self {
                data: data.to_vec(),
                fail_after: Some(n),
            }
        }
    }

    struct FakeReader {
        body: FakeBody,
        pos: usize,
    }

    impl Read for FakeReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if let Some(limit) = self.body.fail_after
                && self.pos >= limit
            {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionReset,
                    "Verbindung abgerissen",
                ));
            }
            let mut end = (self.pos + buf.len()).min(self.body.data.len());
            if let Some(limit) = self.body.fail_after {
                end = end.min(limit);
            }
            let n = end - self.pos;
            buf[..n].copy_from_slice(&self.body.data[self.pos..end]);
            self.pos = end;
            Ok(n)
        }
    }

    #[derive(Default)]
    struct FakeTransport {
        bodies: HashMap<String, FakeBody>,
        calls: Mutex<Vec<String>>,
        served: AtomicUsize,
    }

    impl FakeTransport {
        fn with(bodies: Vec<(&str, FakeBody)>) -> Self {
            Self {
                bodies: bodies
                    .into_iter()
                    .map(|(url, body)| (url.to_string(), body))
                    .collect(),
                ..Self::default()
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Transport for FakeTransport {
        fn get(&self, url: &str) -> Result<Box<dyn Read + Send>, DownloadError> {
            self.calls.lock().unwrap().push(url.to_string());
            self.served.fetch_add(1, Ordering::Relaxed);
            match self.bodies.get(url) {
                Some(body) => Ok(Box::new(FakeReader {
                    body: body.clone(),
                    pos: 0,
                })),
                None => Err(DownloadError::Transport {
                    url: url.to_string(),
                    message: "404".into(),
                }),
            }
        }
    }

    fn sha_hex(data: &[u8]) -> String {
        format!("{:x}", Sha256::digest(data))
    }

    fn artifact(name: &str, data: &[u8]) -> Artifact {
        Artifact {
            name: name.into(),
            bytes: data.len() as u64,
            sha256: sha_hex(data),
            url: format!("https://example.invalid/{name}"),
        }
    }

    fn two_file_manifest() -> (ArtifactManifest, Vec<u8>, Vec<u8>) {
        let first = b"erste-datei-inhalt".to_vec();
        let second = vec![7_u8; 1024];
        let manifest = ArtifactManifest {
            key: "fake-model".into(),
            files: vec![artifact("a.bin", &first), artifact("b.bin", &second)],
        };
        (manifest, first, second)
    }

    fn no_cancel() -> AtomicBool {
        AtomicBool::new(false)
    }

    fn run(
        dir: &Path,
        manifest: &ArtifactManifest,
        transport: &dyn Transport,
    ) -> (Result<(), DownloadError>, Vec<String>) {
        let cancel = no_cancel();
        let mut seen = Vec::new();
        let result = download_model(dir, manifest, transport, &cancel, &mut |p| {
            seen.push(format!("{p:?}"));
        });
        (result, seen)
    }

    fn dir_entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn download_writes_every_file_and_marks_complete_last() {
        let temp = tempfile::tempdir().unwrap();
        let (manifest, first, second) = two_file_manifest();
        let transport = FakeTransport::with(vec![
            ("https://example.invalid/a.bin", FakeBody::ok(&first)),
            ("https://example.invalid/b.bin", FakeBody::ok(&second)),
        ]);

        let (result, _) = run(temp.path(), &manifest, &transport);
        result.unwrap();

        assert_eq!(std::fs::read(temp.path().join("a.bin")).unwrap(), first);
        assert_eq!(std::fs::read(temp.path().join("b.bin")).unwrap(), second);
        assert_eq!(dir_entries(temp.path()), ["COMPLETE", "a.bin", "b.bin"]);
        assert_eq!(
            std::fs::read_to_string(complete_marker(temp.path())).unwrap(),
            "fake-model\n"
        );
        // Danach besteht die reguläre Startprüfung.
        check_artifacts(temp.path(), &manifest).unwrap();
        verify_artifacts_sha256(temp.path(), &manifest).unwrap();
    }

    #[test]
    fn aborted_transfer_leaves_neither_target_nor_part() {
        let temp = tempfile::tempdir().unwrap();
        let (manifest, first, second) = two_file_manifest();
        let transport = FakeTransport::with(vec![
            ("https://example.invalid/a.bin", FakeBody::ok(&first)),
            (
                "https://example.invalid/b.bin",
                FakeBody::cut_after(&second, 400),
            ),
        ]);

        let (result, _) = run(temp.path(), &manifest, &transport);
        match result.unwrap_err() {
            DownloadError::Transport { url, .. } => assert!(url.ends_with("b.bin")),
            other => panic!("erwartet Transport-Fehler, bekam {other:?}"),
        }
        // Die erste Datei ist fertig und bleibt; von der zweiten darf nichts
        // übrig sein — vor allem kein COMPLETE.
        assert_eq!(dir_entries(temp.path()), ["a.bin"]);
    }

    #[test]
    fn short_body_is_a_size_error() {
        let temp = tempfile::tempdir().unwrap();
        let data = vec![3_u8; 512];
        let mut manifest = ArtifactManifest {
            key: "fake-model".into(),
            files: vec![artifact("a.bin", &data)],
        };
        // Manifest erwartet mehr, als der Server liefert.
        manifest.files[0].bytes = 1024;
        let transport =
            FakeTransport::with(vec![("https://example.invalid/a.bin", FakeBody::ok(&data))]);

        let (result, _) = run(temp.path(), &manifest, &transport);
        match result.unwrap_err() {
            DownloadError::SizeMismatch {
                actual, expected, ..
            } => {
                assert_eq!(actual, 512);
                assert_eq!(expected, 1024);
            }
            other => panic!("erwartet SizeMismatch, bekam {other:?}"),
        }
        assert!(dir_entries(temp.path()).is_empty(), "nichts darf bleiben");
    }

    #[test]
    fn oversized_body_is_a_size_error_too() {
        let temp = tempfile::tempdir().unwrap();
        let data = vec![3_u8; 2048];
        let mut manifest = ArtifactManifest {
            key: "fake-model".into(),
            files: vec![artifact("a.bin", &data)],
        };
        manifest.files[0].bytes = 1024;
        let transport =
            FakeTransport::with(vec![("https://example.invalid/a.bin", FakeBody::ok(&data))]);

        let (result, _) = run(temp.path(), &manifest, &transport);
        assert!(matches!(
            result.unwrap_err(),
            DownloadError::SizeMismatch { .. }
        ));
        assert!(dir_entries(temp.path()).is_empty());
    }

    /// §6.3: „Hashfehler: nur `.part` löschen."
    #[test]
    fn wrong_hash_removes_only_the_part_and_keeps_earlier_files() {
        let temp = tempfile::tempdir().unwrap();
        let (mut manifest, first, second) = two_file_manifest();
        // Gleiche Größe, anderer Inhalt — nur der Hash entlarvt das.
        let corrupt = vec![9_u8; second.len()];
        manifest.files[1].sha256 = sha_hex(&second);
        let transport = FakeTransport::with(vec![
            ("https://example.invalid/a.bin", FakeBody::ok(&first)),
            ("https://example.invalid/b.bin", FakeBody::ok(&corrupt)),
        ]);

        let (result, _) = run(temp.path(), &manifest, &transport);
        match result.unwrap_err() {
            DownloadError::HashMismatch {
                actual, expected, ..
            } => {
                assert_eq!(actual, sha_hex(&corrupt));
                assert_eq!(expected, sha_hex(&second));
            }
            other => panic!("erwartet HashMismatch, bekam {other:?}"),
        }
        assert_eq!(dir_entries(temp.path()), ["a.bin"]);
        assert!(!complete_marker(temp.path()).exists());
    }

    #[test]
    fn existing_valid_files_are_not_fetched_again() {
        let temp = tempfile::tempdir().unwrap();
        let (manifest, first, second) = two_file_manifest();
        std::fs::write(temp.path().join("a.bin"), &first).unwrap();
        let transport = FakeTransport::with(vec![
            ("https://example.invalid/a.bin", FakeBody::ok(&first)),
            ("https://example.invalid/b.bin", FakeBody::ok(&second)),
        ]);

        let (result, seen) = run(temp.path(), &manifest, &transport);
        result.unwrap();
        assert_eq!(transport.calls(), ["https://example.invalid/b.bin"]);
        assert!(
            seen.iter().any(|line| line.contains("Skipped")),
            "Skipped fehlt: {seen:?}"
        );
    }

    #[test]
    fn existing_file_with_right_size_but_wrong_content_is_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let (manifest, first, second) = two_file_manifest();
        std::fs::write(temp.path().join("a.bin"), vec![0_u8; first.len()]).unwrap();
        let transport = FakeTransport::with(vec![
            ("https://example.invalid/a.bin", FakeBody::ok(&first)),
            ("https://example.invalid/b.bin", FakeBody::ok(&second)),
        ]);

        let (result, _) = run(temp.path(), &manifest, &transport);
        result.unwrap();
        assert_eq!(std::fs::read(temp.path().join("a.bin")).unwrap(), first);
        assert_eq!(transport.calls().len(), 2);
    }

    #[test]
    fn cancel_stops_the_download_without_leftovers() {
        let temp = tempfile::tempdir().unwrap();
        let (manifest, first, second) = two_file_manifest();
        let transport = FakeTransport::with(vec![
            ("https://example.invalid/a.bin", FakeBody::ok(&first)),
            ("https://example.invalid/b.bin", FakeBody::ok(&second)),
        ]);

        let cancel = AtomicBool::new(true);
        let result = download_model(temp.path(), &manifest, &transport, &cancel, &mut |_| {});
        assert!(matches!(result.unwrap_err(), DownloadError::Cancelled));
        assert!(transport.calls().is_empty());
        assert!(dir_entries(temp.path()).is_empty());
    }

    /// §6.3: „Per-user Download-Lock gegen parallele Starts."
    #[test]
    fn parallel_download_is_refused_while_the_lock_is_held() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("models");
        let lock_path = temp.path().join("diktier-download.lock");
        let (manifest, first, second) = two_file_manifest();
        let transport = FakeTransport::with(vec![
            ("https://example.invalid/a.bin", FakeBody::ok(&first)),
            ("https://example.invalid/b.bin", FakeBody::ok(&second)),
        ]);
        let cancel = no_cancel();

        // Erster Prozess hält den Lock.
        let held = single_instance::try_lock(&lock_path)
            .unwrap()
            .held()
            .unwrap();
        let busy = download_model_locked(
            &lock_path,
            &dir,
            &manifest,
            &transport,
            &cancel,
            &mut |_| {},
        );
        match busy.unwrap_err() {
            DownloadError::Busy(path) => assert_eq!(path, lock_path),
            other => panic!("erwartet Busy, bekam {other:?}"),
        }
        assert!(
            transport.calls().is_empty(),
            "kein Byte trotz Parallelstart"
        );

        // Ist der erste fertig, läuft der zweite Versuch durch.
        drop(held);
        download_model_locked(
            &lock_path,
            &dir,
            &manifest,
            &transport,
            &cancel,
            &mut |_| {},
        )
        .unwrap();
        assert!(complete_marker(&dir).is_file());
    }

    #[test]
    fn missing_url_in_manifest_is_reported() {
        let temp = tempfile::tempdir().unwrap();
        let manifest = ArtifactManifest {
            key: "fake-model".into(),
            files: vec![Artifact {
                name: "a.bin".into(),
                bytes: 4,
                sha256: sha_hex(b"aaaa"),
                url: String::new(),
            }],
        };
        let transport = FakeTransport::default();
        let (result, _) = run(temp.path(), &manifest, &transport);
        assert!(matches!(result.unwrap_err(), DownloadError::Manifest(_)));
    }

    #[test]
    fn progress_reports_every_file_in_order() {
        let temp = tempfile::tempdir().unwrap();
        let (manifest, first, second) = two_file_manifest();
        let transport = FakeTransport::with(vec![
            ("https://example.invalid/a.bin", FakeBody::ok(&first)),
            ("https://example.invalid/b.bin", FakeBody::ok(&second)),
        ]);
        let cancel = no_cancel();
        let mut steps = Vec::new();
        download_model(temp.path(), &manifest, &transport, &cancel, &mut |p| {
            steps.push(match p {
                Progress::Started { name, index, .. } => format!("start {index} {name}"),
                Progress::Verified { name, index, .. } => format!("fertig {index} {name}"),
                Progress::Skipped { name, index, .. } => format!("uebersprungen {index} {name}"),
                Progress::Bytes { .. } => "bytes".into(),
            });
        })
        .unwrap();
        assert_eq!(
            steps,
            [
                "start 1 a.bin",
                "fertig 1 a.bin",
                "start 2 b.bin",
                "fertig 2 b.bin",
            ]
        );
    }

    #[test]
    fn complete_marker_sits_in_the_model_dir() {
        assert_eq!(
            complete_marker(Path::new("/x/models/key")),
            PathBuf::from("/x/models/key/COMPLETE")
        );
    }
}
