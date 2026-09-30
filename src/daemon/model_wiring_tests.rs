//! Modellwahl durch den Daemonpfad (Spec §6.2, v1.10; Plan WP2a, Sol W3/W8).
//!
//! Ohne Win32 und ohne ONNX: Der echte Kern (`transition` + `drive`) treibt
//! eine Probe, die für Artefaktprüfung und Download dieselben Funktionen
//! benutzt wie der Daemon ([`super::artifacts_checked`],
//! [`super::workers::run_download`]). Das Laden prüft, was
//! `ParakeetTranscriber::load` vor `parakeet-rs` prüft, und merkt sich, welches
//! Modell verlangt wurde. Beide Schlüssel laufen durch dieselben Fälle.

use std::collections::{HashMap, VecDeque};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use super::artifacts_checked;
use super::dispatch::{Actors, QuitLatch, Timers, drive};
use super::logging::Logger;
use super::workers::run_download;
use crate::download::{self, ArtifactManifest, DownloadError, SelectedModel, Transport};
use crate::state::{AppState, CopyReason, ErrorKind, Event, LogEvent, RunId, Runtime};
use crate::tray;

/// Liefert feste Bytes je URL und merkt sich jede Anfrage.
#[derive(Default)]
struct MapTransport {
    bodies: HashMap<String, Vec<u8>>,
    calls: Mutex<Vec<String>>,
}

impl MapTransport {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl Transport for MapTransport {
    fn get(&self, url: &str) -> Result<Box<dyn Read + Send>, DownloadError> {
        self.calls.lock().unwrap().push(url.to_string());
        match self.bodies.get(url) {
            Some(data) => Ok(Box::new(io::Cursor::new(data.clone()))),
            None => Err(DownloadError::Transport {
                url: url.to_string(),
                message: "404".into(),
            }),
        }
    }
}

/// Das gewählte Modell mit echtem Schlüssel, echten Dateinamen, echten URLs
/// und echtem Verzeichnis unter `root` — nur Größe und Hash passen zu kleinen
/// Fake-Inhalten.
fn shrunk_selection(root: &Path, key: &str) -> (SelectedModel, Vec<(String, Vec<u8>)>) {
    let real = SelectedModel::select_in(root, key).unwrap();
    let mut manifest: ArtifactManifest = real.manifest().clone();
    let mut bodies = Vec::new();
    for file in &mut manifest.files {
        let data = format!("{key}/{}", file.name).into_bytes();
        file.bytes = data.len() as u64;
        file.sha256 = format!("{:x}", Sha256::digest(&data));
        bodies.push((file.url.clone(), data));
    }
    (
        SelectedModel::new(real.dir().to_path_buf(), manifest),
        bodies,
    )
}

fn other_key(key: &str) -> &'static str {
    download::model_keys()
        .unwrap()
        .into_iter()
        .find(|k| *k != key)
        .unwrap()
}

struct Probe<'a> {
    model: SelectedModel,
    transport: &'a MapTransport,
    lock_path: PathBuf,
    log: Logger,
    emitted: Vec<Event>,
    trays: Vec<AppState>,
    loaded: Vec<(String, PathBuf)>,
}

impl<'a> Probe<'a> {
    fn new(model: SelectedModel, transport: &'a MapTransport, lock_path: PathBuf) -> Self {
        Self {
            model,
            transport,
            lock_path,
            log: Logger::new(false),
            emitted: Vec::new(),
            trays: Vec::new(),
            loaded: Vec::new(),
        }
    }

    /// Startet den Kern und arbeitet alle Folge-Events ab.
    fn boot(&mut self) -> Runtime {
        let mut runtime = Runtime::default();
        let mut queue = VecDeque::from([Event::Startup]);
        drive(
            &mut queue,
            &mut runtime,
            &mut Timers::default(),
            self,
            &mut QuitLatch::default(),
            Instant::now(),
        );
        runtime
    }
}

impl Actors for Probe<'_> {
    fn check_artifacts(&mut self, run: RunId) {
        let event = artifacts_checked(&self.model, run, &self.log);
        self.emitted.push(event);
    }

    fn start_download(&mut self, run: RunId) {
        let cancel = AtomicBool::new(false);
        if let Some(event) = run_download(
            run,
            &self.model,
            self.transport,
            &self.lock_path,
            &cancel,
            &self.log,
        ) {
            self.emitted.push(event);
        }
    }

    fn load_model(&mut self, run: RunId) {
        // Wie `EngineWorker` → `ParakeetTranscriber::load(&model, …)`: dasselbe
        // Modell, dieselbe Startprüfung, nur ohne ONNX.
        self.loaded
            .push((self.model.key().to_string(), self.model.dir().to_path_buf()));
        self.emitted.push(match self.model.check() {
            Ok(()) => Event::ModelLoaded { run },
            Err(err) => Event::ModelLoadFailed {
                run,
                message: err.to_string(),
            },
        });
    }

    fn start_capture(&mut self, _run: RunId, _cap: Duration) {}
    fn stop_capture(&mut self, _run: RunId, _discard: bool) {}
    fn start_transcription(&mut self, _run: RunId) {}
    fn abort_transcription(&mut self, _run: RunId) {}
    fn start_inject(&mut self, _run: RunId, _text: String) {}
    fn copy_only(&mut self, _run: RunId, _text: String, _reason: CopyReason) {}

    fn update_tray(&mut self, state: AppState, _paused: bool) {
        self.trays.push(state);
    }

    fn log(&mut self, _event: &LogEvent) {}
    fn quit(&mut self) {}
    fn output_suppressed(&mut self, _run: RunId) {}

    fn take_emitted(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.emitted)
    }
}

/// Für jeden Schlüssel: Auswahl → `downloading → loading → idle`, Download
/// nur von den eigenen URLs ins eigene Verzeichnis, geladen wird genau dieses
/// Modell. Der zweite Start findet die Artefakte und lädt ohne Netz.
#[test]
fn each_model_downloads_loads_and_idles_on_its_own_path() {
    for key in download::model_keys().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("models");
        let (model, bodies) = shrunk_selection(&root, key);
        let (_, other_bodies) = shrunk_selection(&root, other_key(key));
        let own_urls: Vec<String> = bodies.iter().map(|(u, _)| u.clone()).collect();
        // Der Transport könnte auch das andere Modell liefern — ein Fallback
        // fiele dadurch auf.
        let transport = MapTransport {
            bodies: bodies.into_iter().chain(other_bodies).collect(),
            ..MapTransport::default()
        };

        let mut probe = Probe::new(
            model.clone(),
            &transport,
            temp.path().join("diktier-download.lock"),
        );
        let runtime = probe.boot();

        assert_eq!(runtime.state, AppState::Idle, "{key}");
        assert!(runtime.hotkey_armed(), "{key}");
        assert_eq!(
            probe.trays,
            [
                AppState::Starting,
                AppState::Downloading,
                AppState::Loading,
                AppState::Idle
            ],
            "{key}"
        );
        assert_eq!(transport.calls(), own_urls, "{key}: nur die eigenen URLs");
        assert_eq!(probe.loaded, [(key.to_string(), root.join(key))], "{key}");
        assert_eq!(
            std::fs::read_to_string(download::complete_marker(model.dir())).unwrap(),
            format!("{key}\n")
        );
        assert!(
            !root.join(other_key(key)).exists(),
            "{key}: anderes Modell angefasst"
        );
        // Tray: der Daemon spawnt ihn mit `model.key()`.
        let tip = tray::tooltip_text(&runtime, model.key());
        assert_eq!(tip, format!("idle — {key}"));

        // Zweiter Start: Artefakte vollständig, kein Download.
        let mut again = Probe::new(model, &transport, temp.path().join("diktier-download.lock"));
        let runtime = again.boot();
        assert_eq!(runtime.state, AppState::Idle, "{key}");
        assert_eq!(
            again.trays,
            [AppState::Starting, AppState::Loading, AppState::Idle],
            "{key}"
        );
        assert_eq!(transport.calls().len(), own_urls.len(), "{key}: kein Netz");
    }
}

/// Download scheitert → `error` ohne scharfen Hotkey, und zwar auch dann,
/// wenn das andere Modell vollständig daneben liegt: kein Laden, kein Wechsel.
#[test]
fn a_failed_download_is_fatal_without_fallback_to_the_other_model() {
    for key in download::model_keys().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("models");
        let lock = temp.path().join("diktier-download.lock");

        // Das andere Modell vorher vollständig herunterladen.
        let other = other_key(key);
        let (other_model, other_bodies) = shrunk_selection(&root, other);
        let other_transport = MapTransport {
            bodies: other_bodies.into_iter().collect(),
            ..MapTransport::default()
        };
        assert_eq!(
            Probe::new(other_model.clone(), &other_transport, lock.clone())
                .boot()
                .state,
            AppState::Idle
        );
        let other_before = dir_listing(other_model.dir());

        // Für das gewählte Modell liefert der Transport nichts.
        let (model, _) = shrunk_selection(&root, key);
        let transport = MapTransport {
            bodies: other_transport.bodies.clone(),
            ..MapTransport::default()
        };
        let mut probe = Probe::new(model.clone(), &transport, lock);
        let runtime = probe.boot();

        assert_eq!(runtime.state, AppState::Error, "{key}");
        assert_eq!(
            runtime.error.as_ref().map(|e| e.kind),
            Some(ErrorKind::ModelDownload),
            "{key}"
        );
        assert!(!runtime.hotkey_armed(), "{key}: Hotkey muss aus bleiben");
        assert!(!runtime.tray_click_armed(), "{key}");
        assert!(probe.loaded.is_empty(), "{key}: nichts geladen");
        assert_eq!(
            probe.trays,
            [AppState::Starting, AppState::Downloading, AppState::Error],
            "{key}"
        );
        assert_eq!(
            transport.calls(),
            [model.manifest().files[0].url.clone()],
            "{key}: nur die eigene erste URL"
        );
        assert!(!download::complete_marker(model.dir()).exists());
        assert_eq!(dir_listing(other_model.dir()), other_before, "{key}");
    }
}

/// Ein Modellverzeichnis mit falscher Größe gilt als unvollständig und wird
/// neu geladen — für jeden Schlüssel in seinem eigenen Verzeichnis.
#[test]
fn a_wrong_size_in_the_selected_dir_triggers_its_own_download() {
    for key in download::model_keys().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("models");
        let (model, bodies) = shrunk_selection(&root, key);
        std::fs::create_dir_all(model.dir()).unwrap();
        let first = &model.manifest().files[0];
        std::fs::write(model.dir().join(&first.name), b"zu kurz").unwrap();
        let transport = MapTransport {
            bodies: bodies.into_iter().collect(),
            ..MapTransport::default()
        };
        let mut probe = Probe::new(model.clone(), &transport, temp.path().join("lock"));
        let runtime = probe.boot();
        assert_eq!(runtime.state, AppState::Idle, "{key}");
        assert_eq!(probe.trays[1], AppState::Downloading, "{key}");
        model.check().unwrap();
    }
}

fn dir_listing(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut entries: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect();
    entries.sort();
    entries
}
