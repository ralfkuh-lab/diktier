//! Modell-Artefakte (Spec §6.2, §6.3): Manifest, Prüfung und Download.
//!
//! `models.toml` beschreibt jeden freigegebenen Modellschlüssel. Ein Lauf
//! wählt über [`SelectedModel::select`] genau **einen** Eintrag aus
//! `engine.model`; Daemon, Download, Engine und Tray bekommen diesen Eintrag
//! durchgereicht und wählen nie selbst (§6.2, kein Fallback auf ein anderes
//! Modell).
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
use std::sync::OnceLock;
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
    #[error("Modellschlüssel {key:?} ist unbekannt (erlaubt: {allowed})")]
    UnknownModel { key: String, allowed: String },
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

/// Was Download, Prüfung und Engine von einem Modell brauchen: Schlüssel und
/// Dateisatz. Die Herkunft bleibt im Katalog ([`RawModel`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactManifest {
    pub key: String,
    pub files: Vec<Artifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
    #[serde(default)]
    pub url: String,
}

/// Herkunftsart eines Modells (§6.3 „Unveränderliche URL", v1.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ModelSource {
    /// `https://huggingface.co/<repository>/resolve/<revision>/<datei>`
    Huggingface,
    /// `https://github.com/<repository>/releases/download/<release_tag>/<datei>`
    GithubRelease,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCatalog {
    default_model: String,
    models: Vec<RawModel>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawModel {
    key: String,
    source: ModelSource,
    repository: String,
    #[serde(default)]
    revision: Option<String>,
    #[serde(default)]
    release_tag: Option<String>,
    files: Vec<Artifact>,
}

impl RawModel {
    fn manifest(&self) -> ArtifactManifest {
        ArtifactManifest {
            key: self.key.clone(),
            files: self.files.clone(),
        }
    }

    /// Die einzige URL, die aus der Herkunft folgt. Nichts wird aus der URL
    /// zurückgeraten — umgekehrt muss die URL hierzu passen.
    fn expected_url(&self, name: &str) -> Result<String, String> {
        let key = &self.key;
        match (self.source, &self.revision, &self.release_tag) {
            (ModelSource::Huggingface, Some(rev), None) => {
                if !is_lower_hex(rev, 40) {
                    return Err(format!(
                        "{key}: revision muss ein voller Git-Commit sein (40 Hex-Zeichen)"
                    ));
                }
                Ok(format!(
                    "https://huggingface.co/{}/resolve/{rev}/{name}",
                    self.repository
                ))
            }
            (ModelSource::GithubRelease, None, Some(tag)) => {
                if !is_safe_component(tag) {
                    return Err(format!("{key}: release_tag {tag:?} ist kein sicherer Name"));
                }
                Ok(format!(
                    "https://github.com/{}/releases/download/{tag}/{name}",
                    self.repository
                ))
            }
            (ModelSource::Huggingface, ..) => Err(format!(
                "{key}: source huggingface braucht revision und kein release_tag"
            )),
            (ModelSource::GithubRelease, ..) => Err(format!(
                "{key}: source github-release braucht release_tag und keine revision"
            )),
        }
    }
}

/// Geprüfter Katalog aus `models.toml`.
#[derive(Debug, Clone)]
struct Catalog {
    default_model: String,
    models: Vec<RawModel>,
}

fn parse_catalog(text: &str) -> Result<Catalog, String> {
    let raw: RawCatalog = toml::from_str(text).map_err(|e| e.to_string())?;
    if raw.models.is_empty() {
        return Err("keine Modelle".into());
    }
    let mut keys: Vec<&str> = Vec::with_capacity(raw.models.len());
    for model in &raw.models {
        validate_model(model)?;
        if keys.contains(&model.key.as_str()) {
            return Err(format!("Modellschlüssel {:?} doppelt", model.key));
        }
        keys.push(&model.key);
    }
    if !keys.contains(&raw.default_model.as_str()) {
        return Err(format!(
            "default_model {:?} steht nicht unter [[models]]",
            raw.default_model
        ));
    }
    Ok(Catalog {
        default_model: raw.default_model,
        models: raw.models,
    })
}

fn validate_model(model: &RawModel) -> Result<(), String> {
    let key = &model.key;
    // Der Schlüssel wird zum Verzeichnisnamen unter `models\` (§6.3).
    if !is_safe_component(key) {
        return Err(format!(
            "Modellschlüssel {key:?} ist kein sicherer Verzeichnisname"
        ));
    }
    let repo_ok = model
        .repository
        .split_once('/')
        .is_some_and(|(owner, name)| is_safe_component(owner) && is_safe_component(name));
    if !repo_ok {
        return Err(format!(
            "{key}: repository {:?} muss <owner>/<name> sein",
            model.repository
        ));
    }
    if model.files.is_empty() {
        return Err(format!("{key}: keine Dateien"));
    }
    let mut names: Vec<&str> = Vec::with_capacity(model.files.len());
    for file in &model.files {
        let name = &file.name;
        if !is_safe_component(name) || name == COMPLETE_MARKER || name.ends_with(PART_SUFFIX) {
            return Err(format!("{key}: Dateiname {name:?} ist nicht zulässig"));
        }
        if names.contains(&name.as_str()) {
            return Err(format!("{key}: Datei {name:?} doppelt"));
        }
        names.push(name);
        if file.bytes == 0 {
            return Err(format!("{key}/{name}: bytes = 0"));
        }
        if !is_lower_hex(&file.sha256, 64) {
            return Err(format!("{key}/{name}: sha256 muss 64 Hex-Zeichen haben"));
        }
        let expected = model.expected_url(name)?;
        if file.url != expected {
            return Err(format!(
                "{key}/{name}: url {:?} folgt nicht aus der Herkunft (erwartet {expected:?})",
                file.url
            ));
        }
    }
    Ok(())
}

/// Ein Pfadbestandteil ohne Trenner, ohne `..`, ohne führenden Punkt.
fn is_safe_component(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && !text.contains("..")
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

fn is_lower_hex(text: &str, len: usize) -> bool {
    text.len() == len && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Der eingebaute Katalog, einmal geparst und geprüft.
fn catalog() -> Result<&'static Catalog, DownloadError> {
    static CATALOG: OnceLock<Result<Catalog, String>> = OnceLock::new();
    CATALOG
        .get_or_init(|| parse_catalog(MANIFEST_TOML))
        .as_ref()
        .map_err(|e| DownloadError::Manifest(e.clone()))
}

/// Alle freigegebenen Modellschlüssel in Manifest-Reihenfolge (§6.2).
pub fn model_keys() -> Result<Vec<&'static str>, DownloadError> {
    Ok(catalog()?.models.iter().map(|m| m.key.as_str()).collect())
}

/// `default_model` aus dem Manifest; ein Test hält es gleich `DEFAULT_MODEL`.
pub fn default_model_key() -> Result<&'static str, DownloadError> {
    Ok(catalog()?.default_model.as_str())
}

/// Erlaubte Schlüssel für Fehlermeldungen: `"a", "b"`.
pub fn allowed_models_hint(keys: &[&str]) -> String {
    keys.iter()
        .map(|k| format!("{k:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// SHA-256 (klein, hex) der eingebauten `models.toml`-Bytes, ungeparst
/// (§9 `--manifest-sha256`): Das Release-Skript vergleicht damit das Binary
/// mit `src\models.toml`, auch bei `-SkipBuild`.
pub fn manifest_sha256() -> String {
    format!("{:x}", Sha256::digest(MANIFEST_TOML.as_bytes()))
}

/// Manifest eines Schlüssels. Unbekannt ist ein Fehler — es gibt keinen
/// Ersatz durch ein anderes Modell (§6.2).
pub fn load_manifest(key: &str) -> Result<ArtifactManifest, DownloadError> {
    let catalog = catalog()?;
    match catalog.models.iter().find(|m| m.key == key) {
        Some(model) => Ok(model.manifest()),
        None => Err(DownloadError::UnknownModel {
            key: key.to_string(),
            allowed: allowed_models_hint(
                &catalog
                    .models
                    .iter()
                    .map(|m| m.key.as_str())
                    .collect::<Vec<_>>(),
            ),
        }),
    }
}

/// `%LOCALAPPDATA%\diktier\models\` — darunter ein Verzeichnis je Schlüssel.
pub fn models_root() -> Result<PathBuf, DownloadError> {
    let local = std::env::var_os("LOCALAPPDATA").ok_or_else(|| {
        DownloadError::Path("Umgebungsvariable LOCALAPPDATA ist nicht gesetzt".into())
    })?;
    Ok(PathBuf::from(local).join("diktier").join("models"))
}

/// `%LOCALAPPDATA%\diktier\models\<key>\`.
pub fn model_dir(key: &str) -> Result<PathBuf, DownloadError> {
    Ok(models_root()?.join(key))
}

/// Der für diesen Lauf gewählte Manifesteintrag samt Verzeichnis (§6.2).
///
/// Entsteht einmal aus `engine.model` und wird von dort an durchgereicht:
/// Daemon, Download-Worker, Engine, Tray und stt-smoke lesen Schlüssel,
/// Dateisatz und Verzeichnis nur hier ab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedModel {
    manifest: ArtifactManifest,
    dir: PathBuf,
}

impl SelectedModel {
    /// Auswahl unter `%LOCALAPPDATA%\diktier\models\`.
    pub fn select(key: &str) -> Result<Self, DownloadError> {
        let manifest = load_manifest(key)?;
        Ok(Self::new(model_dir(&manifest.key)?, manifest))
    }

    /// Auswahl unter einem anderen Wurzelverzeichnis (Tests).
    pub fn select_in(root: &Path, key: &str) -> Result<Self, DownloadError> {
        let manifest = load_manifest(key)?;
        Ok(Self::new(root.join(&manifest.key), manifest))
    }

    /// Frei zusammengesetzt (Fakes mit kleinen Dateien).
    pub fn new(dir: PathBuf, manifest: ArtifactManifest) -> Self {
        Self { manifest, dir }
    }

    pub fn key(&self) -> &str {
        &self.manifest.key
    }

    pub fn manifest(&self) -> &ArtifactManifest {
        &self.manifest
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Startprüfung: Existenz und Größe (§6.3 „Prüfumfang").
    pub fn check(&self) -> Result<(), DownloadError> {
        check_artifacts(&self.dir, &self.manifest)
    }
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

    // §6.3: „zuletzt Marker COMPLETE schreiben." Erst wenn wirklich alle
    // Dateien dieses Manifests geprüft an ihrem Platz liegen.
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

/// HTTPS-Transport für den echten Download (§6.3: unveränderliche URLs —
/// Hugging-Face-Commit oder Asset eines immutable GitHub-Releases).
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

    const ULTRA: &str = "parakeet-ultra-0.6b-int8-pc";

    /// §9 `--manifest-sha256`: Digest der eingebetteten Bytes, gleich dem der
    /// Quelldatei auf Platte (so vergleicht es `release.ps1`), klein und hex.
    #[test]
    fn manifest_sha256_matches_source_file() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("models.toml");
        let on_disk = std::fs::read(path).unwrap();
        let expected = format!("{:x}", Sha256::digest(&on_disk));
        assert_eq!(manifest_sha256(), expected);
        assert!(is_lower_hex(&manifest_sha256(), 64));
    }

    /// v3 bleibt in allen Werten wie vor dem Mehrmodell-Manifest, URLs
    /// eingeschlossen (Golden Set, §6.3).
    #[test]
    fn golden_set_matches_spec() {
        let manifest = load_manifest(DEFAULT_MODEL).unwrap();
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

    /// Ultra-Artefakte nach der SPEC-Tabelle (§6.3 „Ultra-Artefakte", v1.10):
    /// drei Dateien, kein `config.json`, kanonische Release-URLs.
    #[test]
    fn ultra_set_matches_spec() {
        let manifest = load_manifest(ULTRA).unwrap();
        assert_eq!(manifest.key, ULTRA);
        const BASE: &str = "https://github.com/ralfkuh-lab/diktier-models/releases/download/model-parakeet-ultra-0.6b-int8-pc-r1";
        let expected = [
            (
                "encoder-model.int8.onnx",
                700_507_227_u64,
                "2cc01c15a08d6976ca9ebe97739d15890f3088cfedd3a4aa4d969ba7a1702038",
            ),
            (
                "decoder_joint-model.int8.onnx",
                18_300_628,
                "afcb9459250ab5c2e48e657d852501c233e8b7f5daed1894a2a7101d21165a5e",
            ),
            (
                "vocab.txt",
                93_939,
                "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
            ),
        ];
        assert_eq!(manifest.files.len(), expected.len());
        for (file, (name, bytes, sha)) in manifest.files.iter().zip(expected) {
            assert_eq!(file.name, name);
            assert_eq!(file.bytes, bytes);
            assert_eq!(file.sha256, sha);
            assert_eq!(file.url, format!("{BASE}/{name}"));
        }
        assert!(manifest.files.iter().all(|f| f.name != "config.json"));
    }

    #[test]
    fn catalog_origin_is_structured_per_model() {
        let catalog = catalog().unwrap();
        let v3 = catalog
            .models
            .iter()
            .find(|m| m.key == DEFAULT_MODEL)
            .unwrap();
        assert_eq!(v3.source, ModelSource::Huggingface);
        assert_eq!(v3.repository, "istupakov/parakeet-tdt-0.6b-v3-onnx");
        assert_eq!(
            v3.revision.as_deref(),
            Some("8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce")
        );
        assert_eq!(v3.release_tag, None);

        let ultra = catalog.models.iter().find(|m| m.key == ULTRA).unwrap();
        assert_eq!(ultra.source, ModelSource::GithubRelease);
        assert_eq!(ultra.repository, "ralfkuh-lab/diktier-models");
        assert_eq!(ultra.revision, None);
        assert_eq!(
            ultra.release_tag.as_deref(),
            Some("model-parakeet-ultra-0.6b-int8-pc-r1")
        );
    }

    /// §6.2: genau zwei Schlüssel, eindeutig, Default = `DEFAULT_MODEL`.
    #[test]
    fn catalog_keys_are_unique_and_default_is_v3() {
        let keys = model_keys().unwrap();
        assert_eq!(keys, [DEFAULT_MODEL, ULTRA]);
        assert_eq!(default_model_key().unwrap(), DEFAULT_MODEL);
    }

    /// Jeder Schlüssel ist ein eigenes, sicheres Verzeichnis direkt unter der
    /// Modellwurzel; die beiden Verzeichnisse sind verschieden.
    #[test]
    fn model_dirs_are_safe_and_separate() {
        let root = Path::new("C:/root/models");
        let mut dirs = Vec::new();
        for key in model_keys().unwrap() {
            assert!(is_safe_component(key), "{key}");
            assert!(!key.contains(['/', '\\', ':']), "{key}");
            let model = SelectedModel::select_in(root, key).unwrap();
            assert_eq!(model.key(), key);
            assert_eq!(model.dir().parent(), Some(root));
            assert_eq!(model.dir().file_name().unwrap(), key);
            dirs.push(model.dir().to_path_buf());
        }
        dirs.dedup();
        assert_eq!(dirs.len(), 2);
    }

    #[test]
    fn unknown_key_is_an_error_naming_the_allowed_keys() {
        let err = load_manifest("whisper-medium").unwrap_err();
        match &err {
            DownloadError::UnknownModel { key, .. } => assert_eq!(key, "whisper-medium"),
            other => panic!("erwartet UnknownModel, bekam {other:?}"),
        }
        let msg = err.to_string();
        assert!(msg.contains("whisper-medium"), "{msg}");
        assert!(msg.contains(DEFAULT_MODEL), "{msg}");
        assert!(msg.contains(ULTRA), "{msg}");
        assert!(SelectedModel::select("whisper-medium").is_err());
    }

    /// Ein gültiger Mini-Katalog als Grundlage für die Negativfälle.
    const MINI_HF: &str = r#"[[models]]
key = "m"
source = "huggingface"
repository = "o/r"
revision = "0123456789abcdef0123456789abcdef01234567"

[[models.files]]
name = "a.bin"
bytes = 4
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
url = "https://huggingface.co/o/r/resolve/0123456789abcdef0123456789abcdef01234567/a.bin"
"#;

    fn mini_catalog(model_block: &str) -> String {
        format!("default_model = \"m\"\n\n{model_block}")
    }

    #[test]
    fn parse_catalog_accepts_the_mini_catalog() {
        let catalog = parse_catalog(&mini_catalog(MINI_HF)).unwrap();
        assert_eq!(catalog.models.len(), 1);
    }

    #[test]
    fn parse_catalog_rejects_broken_entries() {
        let key = |to: &str| mini_catalog(&MINI_HF.replace(r#"key = "m""#, to));
        let name = |to: &str| mini_catalog(&MINI_HF.replace(r#"name = "a.bin""#, to));
        let cases: Vec<(String, &str)> = vec![
            (mini_catalog(&format!("{MINI_HF}\n{MINI_HF}")), "doppelt"),
            (key(r#"key = "../m""#), "Verzeichnis"),
            (key(r#"key = "a/b""#), "Verzeichnis"),
            (key(r#"key = "a\\b""#), "Verzeichnis"),
            (key(r#"key = ".m""#), "Verzeichnis"),
            (key(r#"key = """#), "Verzeichnis"),
            (name(r#"name = "../a.bin""#), "nicht zulässig"),
            (name(r#"name = "sub/a.bin""#), "nicht zulässig"),
            (name(r#"name = "COMPLETE""#), "nicht zulässig"),
            (name(r#"name = "a.bin.part""#), "nicht zulässig"),
            (
                mini_catalog(&MINI_HF.replace(r#"/a.bin""#, r#"/b.bin""#)),
                "folgt nicht",
            ),
            (
                mini_catalog(&MINI_HF.replace("revision = ", "release_tag = \"t\"\nrevision = ")),
                "kein release_tag",
            ),
            (
                mini_catalog(&MINI_HF.replace("0123456789abcdef01234567\"\n", "main\"\n")),
                "40 Hex",
            ),
            (
                mini_catalog(&MINI_HF.replace(r#""huggingface""#, r#""mirror""#)),
                "unknown variant",
            ),
            (
                mini_catalog(&MINI_HF.replace("bytes = 4", "bytes = 4\nsize = 4")),
                "unknown field",
            ),
            (
                mini_catalog(&MINI_HF.replace("bytes = 4", "bytes = 0")),
                "bytes = 0",
            ),
            (
                mini_catalog(&MINI_HF.replace(r#"repository = "o/r""#, r#"repository = "o""#)),
                "repository",
            ),
            (
                format!("default_model = \"x\"\n\n{MINI_HF}"),
                "default_model",
            ),
            (
                String::from("default_model = \"m\"\nmodels = []\n"),
                "keine Modelle",
            ),
        ];
        for (text, needle) in cases {
            let err = parse_catalog(&text).unwrap_err();
            assert!(err.contains(needle), "erwartet {needle:?} in {err:?}");
        }
    }

    /// Eine GitHub-Herkunft verlangt `release_tag` und die kanonische
    /// `releases/download`-Adresse — kein Redirect-Ziel, kein „latest“.
    #[test]
    fn github_origin_requires_the_canonical_release_url() {
        let block = r#"[[models]]
key = "m"
source = "github-release"
repository = "o/r"
release_tag = "t-1"

[[models.files]]
name = "a.bin"
bytes = 4
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
url = "https://github.com/o/r/releases/download/t-1/a.bin"
"#;
        parse_catalog(&mini_catalog(block)).unwrap();
        let redirected = block.replace(
            "https://github.com/o/r/releases/download/t-1/a.bin",
            "https://objects.githubusercontent.com/o/r/a.bin",
        );
        let err = parse_catalog(&mini_catalog(&redirected)).unwrap_err();
        assert!(err.contains("folgt nicht"), "{err}");
        let latest = block.replace("releases/download/t-1/", "releases/latest/download/");
        assert!(parse_catalog(&mini_catalog(&latest)).is_err());
        let no_tag = block.replace("release_tag = \"t-1\"\n", "");
        let err = parse_catalog(&mini_catalog(&no_tag)).unwrap_err();
        assert!(err.contains("braucht release_tag"), "{err}");
    }

    #[test]
    fn check_artifacts_reports_missing_and_size() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = load_manifest(DEFAULT_MODEL).unwrap();
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

    // ------------------------------------------------ Mehrmodell (v1.10)

    /// Das echte Manifest eines Schlüssels mit kleinen Fake-Inhalten: Schlüssel,
    /// Dateinamen und URLs bleiben, Größe und Hash passen zu den Fakes.
    fn shrunk(key: &str) -> (ArtifactManifest, FakeTransport) {
        let mut manifest = load_manifest(key).unwrap();
        let mut bodies = Vec::new();
        for file in &mut manifest.files {
            let data = format!("{key}/{}", file.name).into_bytes();
            file.bytes = data.len() as u64;
            file.sha256 = sha_hex(&data);
            bodies.push((file.url.clone(), FakeBody::ok(&data)));
        }
        let transport = FakeTransport {
            bodies: bodies.into_iter().collect(),
            ..FakeTransport::default()
        };
        (manifest, transport)
    }

    /// Drei-Datei-Satz ohne `config.json` (Ultra, §6.3): genau diese drei
    /// URLs, danach `COMPLETE` mit dem Ultra-Schlüssel.
    #[test]
    fn three_file_set_without_config_json_downloads_completely() {
        let temp = tempfile::tempdir().unwrap();
        let (manifest, transport) = shrunk(ULTRA);
        let model = SelectedModel::new(temp.path().join(ULTRA), manifest.clone());

        let cancel = no_cancel();
        download_model(
            model.dir(),
            model.manifest(),
            &transport,
            &cancel,
            &mut |_| {},
        )
        .unwrap();

        let urls: Vec<String> = manifest.files.iter().map(|f| f.url.clone()).collect();
        assert_eq!(transport.calls(), urls);
        assert!(
            urls.iter()
                .all(|u| u.starts_with("https://github.com/ralfkuh-lab/diktier-models/")),
            "{urls:?}"
        );
        assert_eq!(
            dir_entries(model.dir()),
            [
                "COMPLETE",
                "decoder_joint-model.int8.onnx",
                "encoder-model.int8.onnx",
                "vocab.txt"
            ]
        );
        assert_eq!(
            std::fs::read_to_string(complete_marker(model.dir())).unwrap(),
            format!("{ULTRA}\n")
        );
        model.check().unwrap();
        verify_artifacts_sha256(model.dir(), model.manifest()).unwrap();
    }

    /// Die Download-Sperre bleibt **eine** gemeinsame (Sol W3): Lädt gerade
    /// v3, bekommt Ultra `Busy`; danach landet jedes Modell in seinem eigenen
    /// Verzeichnis, keines berührt das andere.
    #[test]
    fn shared_download_lock_with_separate_model_dirs() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("models");
        let lock_path = temp.path().join("diktier-download.lock");
        let (v3_manifest, v3_transport) = shrunk(DEFAULT_MODEL);
        let (ultra_manifest, ultra_transport) = shrunk(ULTRA);
        let v3 = SelectedModel::new(root.join(DEFAULT_MODEL), v3_manifest);
        let ultra = SelectedModel::new(root.join(ULTRA), ultra_manifest);
        let cancel = no_cancel();

        let held = single_instance::try_lock(&lock_path)
            .unwrap()
            .held()
            .unwrap();
        let busy = download_model_locked(
            &lock_path,
            ultra.dir(),
            ultra.manifest(),
            &ultra_transport,
            &cancel,
            &mut |_| {},
        );
        assert!(matches!(busy.unwrap_err(), DownloadError::Busy(_)));
        assert!(ultra_transport.calls().is_empty());
        assert!(!ultra.dir().exists());
        drop(held);

        download_model_locked(
            &lock_path,
            ultra.dir(),
            ultra.manifest(),
            &ultra_transport,
            &cancel,
            &mut |_| {},
        )
        .unwrap();
        assert!(!v3.dir().exists(), "Ultra-Download berührt v3 nicht");
        assert!(v3_transport.calls().is_empty());

        download_model_locked(
            &lock_path,
            v3.dir(),
            v3.manifest(),
            &v3_transport,
            &cancel,
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(dir_entries(&root), [DEFAULT_MODEL, ULTRA]);
        assert_eq!(dir_entries(v3.dir()).len(), 5, "vier Dateien + COMPLETE");
        assert_eq!(dir_entries(ultra.dir()).len(), 4, "drei Dateien + COMPLETE");
        v3.check().unwrap();
        ultra.check().unwrap();
        // Der Ultra-Satz besteht die v3-Prüfung nicht (anderer Dateisatz).
        assert!(check_artifacts(ultra.dir(), v3.manifest()).is_err());
    }
}
