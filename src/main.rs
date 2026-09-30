//! §9: Das Binary läuft im **Windows-Subsystem** — ein Autostart-Eintrag oder
//! ein Doppelklick soll kein Konsolenfenster aufreißen. `not(test)` ist wichtig:
//! ohne das erbte auch der Test-Harness das Subsystem und `cargo test` liefe
//! stumm.
#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

mod audio;
mod autostart;
mod config;
mod daemon;
mod download;
mod engine;
mod hotkey;
#[cfg(windows)]
mod hotkey_dialog;
mod inject;
/// Aufnahme-Overlay (§4.5) — Windows-only, wie der Hotkey-Dialog.
#[cfg(windows)]
mod overlay;
mod paths;
mod single_instance;
mod state;
mod transcribe_list;
mod tray;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use clap::Parser;

use audio::{AudioError, AudioSource, CpalAudioSource};
use config::ConfigError;
use engine::{ParakeetTranscriber, transcribe_pcm};
use hotkey::{HotkeyBackend, HotkeyEvent, HotkeySpec, new_backend};
use inject::{CaptureContext, InjectOutcome, OutputSink};
use state::{AppState, RecordingSource, Runtime};
use tray::{TrayBackend, TrayEvent};

/// Lokales Push-to-Talk-Diktiertool.
#[derive(Debug, Parser)]
#[command(
    name = "diktier",
    version,
    about = "Lokales Push-to-Talk-Diktiertool",
    disable_help_subcommand = true
)]
struct Cli {
    /// Logs auf stderr, auch mit Konsole.
    #[arg(long, conflicts_with_all = ["install_autostart", "remove_autostart"])]
    foreground: bool,

    /// Autostart-Eintrag anlegen.
    #[arg(long, conflicts_with = "remove_autostart")]
    install_autostart: bool,

    /// Autostart-Eintrag entfernen.
    #[arg(long)]
    remove_autostart: bool,

    /// WAV transkribieren (16 kHz mono PCM). Impliziert --foreground.
    #[arg(
        long,
        value_name = "DATEI",
        conflicts_with_all = ["install_autostart", "remove_autostart", "tray_test"]
    )]
    transcribe_wav: Option<PathBuf>,

    /// WAVs aus einer Liste transkribieren (UTF-8, eine Datei je Zeile), das
    /// Modell nur einmal geladen. Je Datei eine JSONL-Zeile auf stdout:
    /// file, status (text | rejected | error), text, infer_ms, samples.
    /// Exitcode 1, sobald eine Datei `error` hat.
    #[arg(
        long,
        value_name = "LISTE",
        conflicts_with_all = ["install_autostart", "remove_autostart", "transcribe_wav", "gate_analyze", "clipboard_check", "inject_test", "hotkey_test", "record_test", "tray_test", "hotkey_dialog_test", "overlay_test"]
    )]
    transcribe_list: Option<PathBuf>,

    /// Modellschlüssel aus dem Manifest statt `engine.model` (nur mit
    /// --transcribe-wav oder --transcribe-list). Die Config bleibt unverändert.
    #[arg(long, value_name = "SCHLÜSSEL")]
    model: Option<String>,

    /// Gemessene Inferenzläufe nach einem ungezählten Warmup (nur mit
    /// --transcribe-wav oder --transcribe-list; dort n Zeilen je Datei mit `run`).
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    runs: Option<u32>,

    /// SHA-256 des eingebauten Modellmanifests (models.toml) ausgeben, ohne
    /// ein Modell zu laden oder die Config anzufassen (Release-Prüfung).
    #[arg(long, exclusive = true)]
    manifest_sha256: bool,

    /// Silence-Gate je WAV auswerten, mit Alternativen für Marge und Laufdauer
    /// (§6.4, kein Modell nötig).
    #[arg(
        long,
        value_name = "DATEI",
        num_args = 1..,
        conflicts_with_all = ["install_autostart", "remove_autostart", "transcribe_wav", "inject_test", "hotkey_test", "record_test", "tray_test"]
    )]
    gate_analyze: Vec<PathBuf>,

    /// Zwischenablage diagnostizieren (nur lesend): je Format ID, Name, Klasse
    /// und Größe, nie Inhalte.
    ///
    /// Zeigt, was Diktier vor einem Diktat sichern und danach zurückschreiben
    /// würde (Spec §7.1.1): gesichert (Nutz-/Begleitformat), synthetisch
    /// ersetzt, OLE-Verweis entfällt oder Verlust mit Grund. Exitcode 0 = alle
    /// Formate gesichert (oder leer), 3 = teilweise bzw. nicht sicherbar,
    /// 1 = Fehler.
    ///
    /// Hinweis: Das Lesen kann bei der Quelle verzögert gerenderte Formate
    /// anstoßen (Office, Browser); bei großen Inhalten dauert es entsprechend.
    /// Nicht während eines laufenden Diktats ausführen — der Daemon könnte das
    /// Lesen seines Transkripts für ein Einfügen halten (§7.1 Punkt 7).
    #[arg(
        long,
        conflicts_with_all = ["install_autostart", "remove_autostart", "transcribe_wav", "gate_analyze", "inject_test", "hotkey_test", "record_test", "tray_test", "hotkey_dialog_test", "overlay_test"]
    )]
    clipboard_check: bool,

    /// Nur mit --clipboard-check: ÜBERSCHREIBT die Zwischenablage kurzzeitig.
    ///
    /// Snapshot → Testtext setzen → Restore → erneut lesen und vergleichen
    /// (IDs, Reihenfolge, Bytes je gesichertem Format). Verweigert den Start,
    /// solange der Daemon läuft. „Nutzdaten byte-identisch“ ist keine Aussage
    /// über OLE-Objekte, Paste-Link, virtuelle Dateien oder den Owner (z. B.
    /// Excels Laufrahmen) — die gehen beim Restore grundsätzlich verloren.
    /// Nicht sicherbare Inhalte werden nicht angefasst. Exitcode 0 =
    /// byte-identisch, 3 = teilweise, 1 = Fehler oder verweigert.
    #[arg(long, requires = "clipboard_check")]
    roundtrip: bool,

    /// SPIKE: nach 3s den kompletten Inject-Pfad ausführen (nur mit --foreground).
    #[arg(
        long,
        value_name = "TEXT",
        conflicts_with_all = ["install_autostart", "remove_autostart", "transcribe_wav", "hotkey_test", "record_test", "tray_test"]
    )]
    inject_test: Option<String>,

    /// SPIKE: F9 Press/Release 30s loggen (nur mit --foreground). Exit mit Ctrl+C.
    #[arg(
        long,
        conflicts_with_all = ["install_autostart", "remove_autostart", "transcribe_wav", "record_test", "tray_test"]
    )]
    hotkey_test: bool,

    /// SPIKE: SECS Sekunden vom Default-Mic aufnehmen, Pipeline + Transkript (nur mit --foreground).
    #[arg(
        long,
        value_name = "SECS",
        conflicts_with_all = ["install_autostart", "remove_autostart", "transcribe_wav", "inject_test", "hotkey_test", "tray_test"]
    )]
    record_test: Option<u32>,

    /// SPIKE: „Hotkey ändern…"-Dialog öffnen; SECS > 0 schließt ihn von selbst (nur mit --foreground).
    #[arg(
        long,
        value_name = "AUTOCLOSE_SECS",
        num_args = 0..=1,
        default_missing_value = "0",
        conflicts_with_all = ["install_autostart", "remove_autostart", "transcribe_wav", "inject_test", "hotkey_test", "record_test", "tray_test"]
    )]
    hotkey_dialog_test: Option<u32>,

    /// SPIKE: Tray SECS Sekunden anzeigen, Zustände rotieren (nur mit --foreground).
    #[arg(
        long,
        value_name = "SECS",
        conflicts_with_all = ["install_autostart", "remove_autostart", "transcribe_wav", "inject_test", "hotkey_test", "record_test"]
    )]
    tray_test: Option<u32>,

    /// SPIKE: Aufnahme-Overlay SECS Sekunden mit Live-Pegel zeigen (nur mit --foreground).
    #[arg(
        long,
        value_name = "SECS",
        num_args = 0..=1,
        default_missing_value = "15",
        conflicts_with_all = ["install_autostart", "remove_autostart", "transcribe_wav", "inject_test", "hotkey_test", "record_test", "tray_test", "hotkey_dialog_test"]
    )]
    overlay_test: Option<u32>,
}

/// §9/Plan WP5: Als Windows-Subsystem-Programm erbt der Prozess **keine**
/// Konsole. `AttachConsole(ATTACH_PARENT_PROCESS)` hängt ihn an die des
/// Aufrufers, damit `--foreground`, `--help`, `--version` und die Spikes wie
/// gewohnt auf stderr/stdout schreiben. Gibt es keine Eltern-Konsole (Autostart,
/// Doppelklick), passiert nichts — **kein** `AllocConsole` (Entscheidung
/// 2026-08-27): der Daemon loggt dann in `diktier.log` (§10).
///
/// Muss vor jeder Ausgabe laufen: Rusts stdio holt sich das Handle bei jedem
/// Schreiben über `GetStdHandle`, ein Attach danach käme also zu spät für alles
/// schon Geschriebene.
#[cfg(windows)]
fn attach_parent_console() {
    use std::ptr;

    use windows_sys::Win32::Foundation::{
        GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE,
        STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
    };

    // SAFETY: dokumentierte Konstante, kein Zeiger. Fehlschlag (keine
    // Eltern-Konsole, schon eine angehängt) ist ausdrücklich in Ordnung.
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } == 0 {
        return;
    }

    // `AttachConsole` verschafft dem Prozess eine Konsole, setzt aber die
    // Standardhandles nicht: bei einem GUI-Subsystem-Programm sind sie NULL,
    // und Rusts stdio schriebe ins Leere. Deshalb genau dann `CONIN$`/`CONOUT$`
    // öffnen und eintragen, wenn noch kein brauchbares Handle da ist — eine
    // Umleitung des Aufrufers (`> log.txt`, Pipe) bleibt so unangetastet.
    let fix = |which: STD_HANDLE, name: &str, access: u32| {
        // SAFETY: parameterloser Lesezugriff auf die Handle-Tabelle.
        let existing: HANDLE = unsafe { GetStdHandle(which) };
        if !existing.is_null() && existing != INVALID_HANDLE_VALUE {
            return;
        }
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: `wide` ist NUL-terminiert und lebt über den Aufruf; die
        // Konsolen-Pseudodateien existieren, seit `AttachConsole` gelang.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null(),
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            )
        };
        if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
            // SAFETY: gültiges, gerade geöffnetes Handle; der Prozess besitzt
            // es bis zum Ende und schließt es nie — genau das will die
            // Handle-Tabelle.
            unsafe { SetStdHandle(which, handle) };
        }
    };
    fix(STD_INPUT_HANDLE, "CONIN$", GENERIC_READ | GENERIC_WRITE);
    fix(STD_OUTPUT_HANDLE, "CONOUT$", GENERIC_READ | GENERIC_WRITE);
    fix(STD_ERROR_HANDLE, "CONOUT$", GENERIC_READ | GENERIC_WRITE);
}

fn main() -> ExitCode {
    // Erster Schritt, vor Clap und vor `signals::install()`.
    attach_parent_console();
    ExitCode::from(cli_main(std::env::args_os()))
}

fn cli_main<I, T>(args: I) -> u8
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(err) => {
            let code = err.exit_code();
            let _ = err.print();
            return if code == 0 { 0 } else { 2 };
        }
    };

    // §9 (v1.10): modusabhängige Optionen vor **jeder** Aktion prüfen, auch
    // vor Autostart — ein ignorierter Schalter wäre ein stiller Bedienfehler.
    if let Err(code) = check_mode_options(&cli) {
        return code;
    }

    // §5.3: Die CLI-Modi laufen **vor** der Single-Instance-Sperre und fordern
    // sie nie an; §10: sie loggen nur nach stderr, nie in `diktier.log`.
    if cli.manifest_sha256 {
        // Ohne Panik bei geschlossener Pipe: ein Aufrufer, der nicht liest,
        // bekommt Exit 1 statt eines Absturzes.
        use std::io::Write;
        return match writeln!(std::io::stdout(), "{}", download::manifest_sha256()) {
            Ok(()) => 0,
            Err(_) => 1,
        };
    }
    if cli.install_autostart {
        return install_autostart();
    }
    if cli.remove_autostart {
        return remove_autostart();
    }
    if let Some(path) = cli.transcribe_wav {
        return transcribe_wav(&path, cli.model.as_deref(), cli.runs.unwrap_or(1));
    }
    if let Some(list) = cli.transcribe_list {
        return transcribe_list(&list, cli.model.as_deref(), cli.runs);
    }
    if !cli.gate_analyze.is_empty() {
        return gate_analyze(&cli.gate_analyze);
    }
    if cli.clipboard_check {
        return if cli.roundtrip {
            clipboard_roundtrip()
        } else {
            clipboard_check()
        };
    }
    if cli.inject_test.is_some() && !cli.foreground {
        eprintln!("diktier: --inject-test nur mit --foreground (SPIKE)");
        return 2;
    }
    if cli.hotkey_test && !cli.foreground {
        eprintln!("diktier: --hotkey-test nur mit --foreground (SPIKE)");
        return 2;
    }
    if cli.record_test.is_some() && !cli.foreground {
        eprintln!("diktier: --record-test nur mit --foreground (SPIKE)");
        return 2;
    }
    if cli.tray_test.is_some() && !cli.foreground {
        eprintln!("diktier: --tray-test nur mit --foreground (SPIKE)");
        return 2;
    }
    if cli.hotkey_dialog_test.is_some() && !cli.foreground {
        eprintln!("diktier: --hotkey-dialog-test nur mit --foreground (SPIKE)");
        return 2;
    }
    if cli.overlay_test.is_some() && !cli.foreground {
        eprintln!("diktier: --overlay-test nur mit --foreground (SPIKE)");
        return 2;
    }
    if let Some(text) = cli.inject_test {
        return inject_test(&text);
    }
    if cli.hotkey_test {
        return hotkey_test();
    }
    if let Some(secs) = cli.record_test {
        return record_test(secs);
    }
    if let Some(secs) = cli.tray_test {
        return tray_test(secs);
    }
    if let Some(autoclose) = cli.hotkey_dialog_test {
        return hotkey_dialog_test(autoclose);
    }
    if let Some(secs) = cli.overlay_test {
        return overlay_test(secs);
    }

    run_daemon(cli.foreground)
}

/// §9: Autostart-Eintrag anlegen bzw. aktualisieren, idempotent.
fn install_autostart() -> u8 {
    match autostart::install() {
        Ok((outcome, path)) => {
            eprintln!("Autostart {}: {}", outcome.as_str(), path.display());
            0
        }
        Err(err) => {
            eprintln!("diktier: {err}");
            err.exit_code()
        }
    }
}

/// §9: Eigenen Eintrag entfernen. Kein Eintrag da heißt trotzdem Exit 0.
fn remove_autostart() -> u8 {
    match autostart::remove() {
        Ok((outcome, path)) => {
            eprintln!("Autostart {}: {}", outcome.as_str(), path.display());
            0
        }
        Err(err) => {
            eprintln!("diktier: {err}");
            err.exit_code()
        }
    }
}

/// §9 (v1.10): `--model` und `--runs` gelten nur mit `--transcribe-wav` oder
/// `--transcribe-list`; außerhalb davon Exit 2, bevor irgendetwas passiert.
/// Ein unbekannter Schlüssel ist ebenfalls Exit 2.
fn check_mode_options(cli: &Cli) -> Result<(), u8> {
    let transcribing = cli.transcribe_wav.is_some() || cli.transcribe_list.is_some();
    if cli.runs.is_some() && !transcribing {
        eprintln!("diktier: --runs gilt nur zusammen mit --transcribe-wav oder --transcribe-list");
        return Err(2);
    }
    if let Some(key) = &cli.model {
        if !transcribing {
            eprintln!(
                "diktier: --model gilt nur zusammen mit --transcribe-wav oder --transcribe-list"
            );
            return Err(2);
        }
        check_model_key(key)?;
    }
    Ok(())
}

/// §9 (v1.10): `--model` nimmt nur Manifest-Schlüssel; unbekannt ist ein
/// Bedienfehler (Exit 2), kein Ersatzmodell.
fn check_model_key(key: &str) -> Result<(), u8> {
    match download::model_keys() {
        Ok(keys) if keys.contains(&key) => Ok(()),
        Ok(keys) => {
            eprintln!(
                "diktier: unbekannter Modellschlüssel {key:?} (erlaubt: {})",
                download::allowed_models_hint(&keys)
            );
            Err(2)
        }
        Err(err) => {
            eprintln!("diktier: {err}");
            Err(1)
        }
    }
}

/// Config für die Transkriptionsmodi: Warnungen auf stderr, Fehler als Exitcode.
fn load_cli_config() -> Result<config::Config, u8> {
    let loaded = match config::load() {
        Ok(loaded) => loaded,
        Err(err) => {
            eprintln!("{err}");
            return Err(match err {
                ConfigError::Io(_) => 1,
                _ => 2,
            });
        }
    };
    for warning in &loaded.warnings {
        eprintln!("Warnung: {warning}");
    }
    Ok(loaded.config)
}

/// `model_override` ist `--model`; ohne ihn gilt `engine.model` (§9).
fn transcribe_wav(path: &std::path::Path, model_override: Option<&str>, runs: u32) -> u8 {
    let config = match load_cli_config() {
        Ok(config) => config,
        Err(code) => return code,
    };

    let pcm = match audio::read_wav_16k_mono(path) {
        Ok(pcm) => pcm,
        Err(err) => {
            eprintln!("{err}");
            return match err {
                AudioError::Format(_) => 2,
                AudioError::Io(_) | AudioError::Failed(_) => 1,
            };
        }
    };

    // §6.4: der Gate-Report gehört auf stderr, der Text bleibt auf stdout.
    // Hier (und nicht erst aus `transcribe_pcm`), damit eine abgelehnte
    // Aufnahme das Modell gar nicht erst lädt.
    let report = engine::silence_gate(&pcm);
    eprintln!("Gate: {report}");
    if report.is_rejected() {
        println!();
        return 0;
    }

    let load_start = Instant::now();
    // §6.2: genau ein Modell, ausgewählt an dieser einen Stelle — `--model`
    // oder das Config-Modell.
    let key = model_override.unwrap_or(&config.engine.model);
    let model = match engine::model_artifacts(key) {
        Ok(model) => model,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    let mut transcriber = match ParakeetTranscriber::load(&model, config.engine.threads) {
        Ok(t) => t,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    eprintln!(
        "Modell geladen in {:.3} s",
        load_start.elapsed().as_secs_f64()
    );

    // Warmup, ungezählt.
    if let (_, Err(err)) = transcribe_pcm(&mut transcriber, &pcm) {
        eprintln!("{err}");
        return 1;
    }

    let mut last = engine::Transcription::empty();
    for _ in 0..runs {
        let infer_start = Instant::now();
        // Der Report ist derselbe wie oben — der Gate ist deterministisch.
        match transcribe_pcm(&mut transcriber, &pcm).1 {
            Ok(result) => last = result,
            Err(err) => {
                eprintln!("{err}");
                return 1;
            }
        }
        eprintln!("Inferenz {:.3} s", infer_start.elapsed().as_secs_f64());
    }
    println!("{}", last.text);
    0
}

/// `--transcribe-list` (§9, v1.10): JSONL auf stdout, Diagnose auf stderr.
fn transcribe_list(list: &std::path::Path, model_override: Option<&str>, runs: Option<u32>) -> u8 {
    let config = match load_cli_config() {
        Ok(config) => config,
        Err(code) => return code,
    };
    let files = match transcribe_list::read_list(list) {
        Ok(files) => files,
        Err(err) => {
            eprintln!("{err}");
            return err.exit_code();
        }
    };
    let key = model_override.unwrap_or(&config.engine.model).to_string();
    let threads = config.engine.threads;
    let load = || {
        let model = engine::model_artifacts(&key)?;
        ParakeetTranscriber::load(&model, threads)
    };
    transcribe_list::run_batch(
        &files,
        runs,
        load,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    )
}

/// Silence-Gate-Werkzeug (§6.4, silence-gate-plan WP0): je WAV den Report plus
/// die Läufe bei alternativen Margen und Laufdauern — damit sich die Konstanten
/// ohne Rebuild bewerten lassen. Kein Modell, kein Capture.
fn gate_analyze(paths: &[PathBuf]) -> u8 {
    const MARGINS_DB: [f32; 3] = [10.0, engine::RELATIVE_MARGIN_DB, 15.0];
    const RUN_SECS: [f32; 3] = [1.0, engine::MIN_SPEECH_RUN_REL_SECS, 2.0];

    let mut code = 0_u8;
    for (i, path) in paths.iter().enumerate() {
        if i > 0 {
            println!();
        }
        println!("{}", path.display());
        let pcm = match audio::read_wav_16k_mono(path) {
            Ok(pcm) => pcm,
            Err(err) => {
                eprintln!("{err}");
                code = code.max(match err {
                    AudioError::Format(_) => 2,
                    AudioError::Io(_) | AudioError::Failed(_) => 1,
                });
                continue;
            }
        };
        let report = engine::silence_gate(&pcm);
        println!("  {report}");

        // Absolute Pfade B3/B2 ohne Rebuild bewertbar (§6.4, v1.7).
        let windows = engine::window_rms(&pcm);
        let abs_run = |threshold: f32| {
            engine::longest_run_samples(&windows, threshold, engine::ABS_FLOOR) as f32 / 16_000.0
        };
        println!(
            "  absolut {:.4} / {:.4}: Lauf {:.2} s / {:.2} s (B3/B2 ab {:.1} s)",
            engine::QUIET_SPEECH_RMS,
            engine::RMS_SILENCE_THRESHOLD,
            abs_run(engine::QUIET_SPEECH_RMS),
            abs_run(engine::RMS_SILENCE_THRESHOLD),
            engine::MIN_SPEECH_RUN_ABS_SECS
        );

        let Some(floor) = report.metrics.and_then(|m| m.floor) else {
            println!("  (kein volles Fenster — Regel A entscheidet)");
            continue;
        };
        println!(
            "  Marge   Schwelle  Lauf      {}",
            RUN_SECS
                .iter()
                .map(|s| format!("≥{s:.1} s"))
                .collect::<Vec<_>>()
                .join("  ")
        );
        for db in MARGINS_DB {
            let threshold = (floor * engine::db_to_ratio(db)).max(engine::MIN_ACTIVE_RMS);
            let run = engine::longest_run_samples(&windows, threshold, engine::ABS_FLOOR) as f32
                / 16_000.0;
            let verdicts = RUN_SECS
                .iter()
                .map(|&needed| format!("{:<6}", if run >= needed { "ja" } else { "nein" }))
                .collect::<Vec<_>>()
                .join("  ");
            println!(
                "  +{db:>2.0} dB  {threshold:.5}   {run:>5.2} s   {}",
                verdicts.trim_end()
            );
        }
    }
    code
}

/// Exitcodes von `--clipboard-check` (clipboard-restore-plan WP2).
const CLIPBOARD_PARTIAL: u8 = 3;

/// Eine Tabellenzeile je Format. Standard-IDs mit Konstantennamen,
/// registrierte mit bereinigtem Namen in Anführungszeichen (§10).
fn print_snapshot(snapshot: &inject::ClipboardSnapshot) {
    use inject::formats::{self, RowOutcome, SnapshotKind};

    let report = &snapshot.report;
    let lost = report.lost();
    let verdict = match snapshot.kind {
        SnapshotKind::Empty => "leer".to_string(),
        SnapshotKind::Formats if lost.is_empty() => {
            "alle auslesbaren Nutzdaten gesichert".to_string()
        }
        SnapshotKind::Formats => format!("teilweise ({} verloren)", lost.len()),
        SnapshotKind::Unrestorable => "nicht sicherbar (kein Nutzformat)".to_string(),
    };
    println!("Zwischenablage: {} Formate · {verdict}", report.rows.len());
    if report.rows.is_empty() {
        return;
    }
    println!("   #  ID      {:<42}  {:<28}  Größe", "Name", "Klasse");
    for (index, row) in report.rows.iter().enumerate() {
        let name = match (&row.format.name, formats::standard_name(row.format.id)) {
            (Some(name), _) => format!("\"{name}\""),
            (None, Some(name)) => name.to_string(),
            (None, None) => "?".to_string(),
        };
        let (class, size) = match row.outcome {
            RowOutcome::Saved { bytes, useful } => (
                if useful {
                    "gesichert (Nutzformat)".to_string()
                } else {
                    "gesichert (Begleitformat)".to_string()
                },
                formats::format_bytes(bytes),
            ),
            RowOutcome::Replaced => ("synthetisch ersetzt".to_string(), "—".to_string()),
            RowOutcome::OleDropped => ("OLE-Verweis entfällt".to_string(), "—".to_string()),
            RowOutcome::Lost(reason) => (format!("Verlust: {}", reason.as_str()), "—".to_string()),
        };
        println!(
            "  {:>2}  0x{:04X}  {name:<42}  {class:<28}  {size}",
            index + 1,
            row.format.id
        );
    }
    println!(
        "Gesichert: {} in {} Formaten, {} ms",
        formats::format_bytes(report.saved_bytes()),
        report.saved_count(),
        report.duration.as_millis()
    );
}

fn snapshot_exit_code(snapshot: &inject::ClipboardSnapshot) -> u8 {
    use inject::formats::SnapshotKind;
    // Ein gescheiterter Snapshot ist im Daemon `Unrestorable`, für die
    // Diagnose aber ein Fehler (Exit 1).
    if snapshot.report.failure().is_some() {
        return 1;
    }
    match snapshot.kind {
        SnapshotKind::Empty => 0,
        SnapshotKind::Formats if snapshot.report.lost().is_empty() => 0,
        SnapshotKind::Formats | SnapshotKind::Unrestorable => CLIPBOARD_PARTIAL,
    }
}

/// WP2, Default: nur lesend. §10: CLI-Modi schreiben nie in `diktier.log`.
fn clipboard_check() -> u8 {
    match inject::clipboard_check() {
        Ok(snapshot) => {
            print_snapshot(&snapshot);
            snapshot_exit_code(&snapshot)
        }
        Err(err) => {
            eprintln!("diktier: {err}");
            1
        }
    }
}

/// WP2, `--roundtrip`: nur ohne laufenden Daemon. Die Single-Instance-Sperre
/// aus §5.3 wird dafür nur **angefragt**: Hält der Daemon sie, schließt
/// `CreateMutexW` das eigene Handle sofort wieder, und der Daemon bleibt
/// unberührt. Sonst hält der Roundtrip sie bis zum Ende, damit kein Daemon
/// mitten hinein startet.
fn clipboard_roundtrip() -> u8 {
    use inject::RoundtripRestore;
    use single_instance::InstanceAcquire;

    let _lock = match single_instance::acquire_instance_lock(&mut |_| {}) {
        Ok(InstanceAcquire::Held(lock)) => lock,
        Ok(InstanceAcquire::Busy) => {
            eprintln!(
                "diktier: --roundtrip verweigert: der Daemon läuft. Erst Diktier beenden \
                 (Tray → Beenden), dann erneut starten."
            );
            return 1;
        }
        Err(err) => {
            eprintln!("diktier: {err}");
            return 1;
        }
    };
    eprintln!("Achtung: --roundtrip überschreibt die Zwischenablage kurzzeitig.");

    let roundtrip = match inject::clipboard_roundtrip() {
        Ok(roundtrip) => roundtrip,
        Err(err) => {
            eprintln!("diktier: {err}");
            return 1;
        }
    };
    print_snapshot(&roundtrip.before);
    // Sol-Impl-Review Blocker 3: Die Nachprüfung scheiterte. Dann wenigstens
    // sagen, was platziert wurde, und dass der aktuelle Inhalt unbekannt ist.
    if let Some(error) = &roundtrip.after_error {
        println!(
            "Roundtrip: {} — aktueller Inhalt nicht abfragbar: {error}",
            roundtrip_placement(&roundtrip.restore)
        );
        return 1;
    }
    let after = roundtrip
        .after
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    match &roundtrip.restore {
        RoundtripRestore::NotAttempted => {
            println!("Roundtrip: nicht ausgeführt — nichts sicherbar, Zwischenablage unverändert");
            snapshot_exit_code(&roundtrip.before)
        }
        RoundtripRestore::Restored | RoundtripRestore::RestoredPartial => {
            if !roundtrip.lost_restore.is_empty() {
                println!(
                    "Beim Zurückschreiben verloren: {}",
                    inject::formats::lost_list(&roundtrip.lost_restore)
                );
            }
            for mismatch in &roundtrip.mismatches {
                println!("Abweichung: {mismatch}");
            }
            // Budget erschöpft ist keine Abweichung, aber auch kein Beleg.
            for format in &roundtrip.unchecked {
                println!("Nicht geprüft (Budget erschöpft): {format}");
            }
            let identical = roundtrip.mismatches.is_empty() && roundtrip.unchecked.is_empty();
            if identical {
                println!("Roundtrip: Nutzdaten byte-identisch (gesicherte Formate)");
            } else if roundtrip.mismatches.is_empty() {
                println!("Roundtrip: keine Abweichung, aber nicht alle Formate geprüft");
            }
            println!("Keine Aussage über OLE-Objekte, virtuelle Dateien oder den Owner.");
            if identical
                && roundtrip.lost_restore.is_empty()
                && snapshot_exit_code(&roundtrip.before) == 0
            {
                0
            } else {
                CLIPBOARD_PARTIAL
            }
        }
        RoundtripRestore::RestoreFailed => {
            println!("Roundtrip: nicht wiederhergestellt — im Clipboard liegt jetzt: {after}");
            1
        }
        RoundtripRestore::Foreign => {
            println!("Roundtrip: fremder Copy dazwischen — dessen Inhalt bleibt: {after}");
            1
        }
        RoundtripRestore::Failed(message) => {
            println!("Roundtrip abgebrochen: {message}");
            println!(
                "Im Clipboard liegt jetzt: {}",
                if after.is_empty() { "nichts" } else { &after }
            );
            1
        }
    }
}

/// Was der Restore im Roundtrip platziert hat — auch dann, wenn die
/// Nachprüfung danach scheitert.
fn roundtrip_placement(restore: &inject::RoundtripRestore) -> String {
    use inject::RoundtripRestore;
    match restore {
        RoundtripRestore::NotAttempted => "nicht ausgeführt, Zwischenablage unverändert".into(),
        RoundtripRestore::Restored => "vorheriger Inhalt zurückgeschrieben".into(),
        RoundtripRestore::RestoredPartial => "vorheriger Inhalt teilweise zurückgeschrieben".into(),
        RoundtripRestore::RestoreFailed => {
            "nicht wiederhergestellt, der Testtext wurde gesetzt".into()
        }
        RoundtripRestore::Foreign => "fremder Copy dazwischen, dessen Inhalt blieb".into(),
        RoundtripRestore::Failed(message) => format!("abgebrochen ({message})"),
    }
}

fn inject_test(text: &str) -> u8 {
    eprintln!("SPIKE --inject-test (kein Produktionspfad)");
    let loaded = match config::load() {
        Ok(loaded) => loaded,
        Err(err) => {
            eprintln!("{err}");
            return match err {
                ConfigError::Io(_) => 1,
                _ => 2,
            };
        }
    };
    for warning in &loaded.warnings {
        eprintln!("Warnung: {warning}");
    }

    // SPIKE: Gate-Text aus §12 byte-exakt — leading_space nicht anwenden.
    let mut spike_out = loaded.config.output.clone();
    spike_out.leading_space = false;
    let mut sink = match inject::new_sink(spike_out) {
        Ok(sink) => sink,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };

    let start = sink.current_window_id();
    eprintln!("SPIKE start_window_id={}", format_window(start));
    eprintln!("SPIKE: 3s — Ziel im Vordergrund halten für Paste, wechsele für copy_only …");
    std::thread::sleep(std::time::Duration::from_secs(3));

    let target = sink.current_window_id();
    eprintln!("SPIKE target_window_id={}", format_window(target));
    let ctx = CaptureContext {
        start_window_id: start,
        target_window_id: target,
        ended_at: Instant::now(),
    };

    let outcome = match sink.paste(text, &ctx) {
        Ok(outcome) => outcome,
        Err(err) => {
            eprintln!("SPIKE inject-fehler: {err}");
            return 1;
        }
    };
    log_inject_outcome(text, start, target, sink.current_window_id(), &outcome);
    for warning in sink.take_warnings() {
        eprintln!("SPIKE warnung: {warning}");
    }

    let restored = matches!(&outcome, InjectOutcome::Pasted { restored: true, .. });
    if restored {
        match sink.serve_until_read(inject::RESTORED_SERVE_GRACE) {
            Ok(n) => eprintln!("SPIKE restored_served={n}"),
            Err(err) => eprintln!("SPIKE serve: {err}"),
        }
    } else if let Err(err) = sink.serve_for(std::time::Duration::from_secs(2)) {
        eprintln!("SPIKE serve: {err}");
    }
    0
}

fn log_inject_outcome(
    text: &str,
    start: Option<inject::WindowId>,
    target: Option<inject::WindowId>,
    current: Option<inject::WindowId>,
    outcome: &InjectOutcome,
) {
    eprintln!("SPIKE text_bytes={}", text.len());
    eprintln!(
        "SPIKE windows start={} target={} current={}",
        format_window(start),
        format_window(target),
        format_window(current)
    );
    match outcome {
        InjectOutcome::Pasted {
            restored,
            shortcut,
            window,
            wm_class,
            reads,
            restore,
            clipboard,
            transcript,
        } => {
            let class = match wm_class {
                Some((instance, class)) => format!("{instance},{class}"),
                None => "unbekannt".into(),
            };
            eprintln!("SPIKE pfad=paste");
            eprintln!("SPIKE window=0x{:x}", window.0);
            eprintln!("SPIKE wm_class={class}");
            eprintln!("SPIKE shortcut={} (config/auto)", shortcut.as_str());
            eprintln!("SPIKE selection_requests(data)={reads}");
            eprintln!(
                "SPIKE {}",
                inject::formats::snapshot_log_line(&clipboard.snapshot)
            );
            eprintln!(
                "SPIKE restored={restored} restore {}",
                inject::restore_log(*restore, clipboard)
            );
            eprintln!("SPIKE transkript={}", transcript.describe());
        }
        InjectOutcome::CopyOnly {
            reason,
            history_excluded,
            snapshot,
            transcript,
        } => {
            eprintln!("SPIKE pfad=copy_only");
            eprintln!("SPIKE grund={}", reason.as_str());
            if let Some(snapshot) = snapshot {
                eprintln!("SPIKE {}", inject::formats::snapshot_log_line(snapshot));
            }
            eprintln!("SPIKE verlauf_ausgeschlossen={history_excluded}");
            eprintln!("SPIKE transkript={}", transcript.describe());
        }
    }
}

fn format_window(id: Option<inject::WindowId>) -> String {
    match id {
        Some(id) => format!("0x{:x}", id.0),
        None => "None".into(),
    }
}

fn hotkey_test() -> u8 {
    eprintln!("SPIKE --hotkey-test (kein Produktionspfad)");
    // §4.4: die konfigurierte Taste, nicht mehr hart F9.
    let spec = match config::load() {
        Ok(loaded) => HotkeySpec::from_config(&loaded.config.hotkey),
        Err(err) => {
            eprintln!("{err}");
            return 2;
        }
    };
    eprintln!(
        "SPIKE: {} 30s lang halten/loslassen; Exit mit Ctrl+C",
        spec.describe()
    );
    let mut backend = match new_backend(&spec) {
        Ok(backend) => backend,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    eprintln!("SPIKE hotkey-backend={}", backend.backend_name());
    if let Err(err) = backend.register() {
        eprintln!("{err}");
        return 1;
    }
    let end = Instant::now() + std::time::Duration::from_secs(30);
    while Instant::now() < end {
        match backend.poll() {
            Ok(Some(HotkeyEvent::Press)) => eprintln!("SPIKE hotkey: press (entprellt)"),
            Ok(Some(HotkeyEvent::Release)) => eprintln!("SPIKE hotkey: release (entprellt)"),
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
            Err(err) => {
                eprintln!("{err}");
                return 1;
            }
        }
    }
    eprintln!("SPIKE hotkey-test: 30s vorbei");
    0
}

fn record_test(secs: u32) -> u8 {
    eprintln!("SPIKE --record-test (kein Produktionspfad)");
    if secs == 0 {
        eprintln!("diktier: --record-test SECS muss ≥ 1 sein");
        return 2;
    }
    let loaded = match config::load() {
        Ok(loaded) => loaded,
        Err(err) => {
            eprintln!("{err}");
            return match err {
                ConfigError::Io(_) => 1,
                _ => 2,
            };
        }
    };
    for warning in &loaded.warnings {
        eprintln!("Warnung: {warning}");
    }

    // §4.5 / Gate A des Overlay-Plans: derselbe LevelTap wie im Daemon, damit
    // sich beim Sprechen plausible Peaks und bei Stille 0 nachweisen lassen.
    let tap = audio::level::new_tap();
    let mut src = CpalAudioSource::new(&loaded.config.audio, Some(tap.clone()));
    let t_cap = Instant::now();
    if let Err(err) = src.start() {
        eprintln!("{err}");
        return 1;
    }
    let record_end = Instant::now() + std::time::Duration::from_secs(u64::from(secs));
    let mut window_peak = 0.0_f32;
    let mut next_report = Instant::now() + std::time::Duration::from_millis(500);
    while Instant::now() < record_end {
        std::thread::sleep(std::time::Duration::from_millis(25));
        window_peak = window_peak.max(tap.take());
        if Instant::now() >= next_report {
            eprintln!(
                "SPIKE level peak={window_peak:.6} bar={:.2}",
                audio::level::bar_height(window_peak)
            );
            window_peak = 0.0;
            next_report += std::time::Duration::from_millis(500);
        }
    }
    let captured = match src.stop() {
        Ok(c) => c,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    let capture_secs = t_cap.elapsed().as_secs_f64();
    if let Some(st) = src.last_stats() {
        eprintln!("SPIKE device={}", st.device_name);
        eprintln!(
            "SPIKE native_rate={} native_format={} native_channels={}",
            st.native_rate, st.native_format, st.native_channels
        );
        eprintln!(
            "SPIKE input_frames={} input_samples={} output_samples_16k={}",
            st.input_frames, st.input_samples, st.output_samples
        );
        eprintln!("SPIKE overflow_frames={}", st.overflow_frames);
        eprintln!(
            "SPIKE convert_resample_secs={:.3} capture_wall_secs={:.3}",
            st.convert_resample_secs, capture_secs
        );
    }

    // §6.4 / Gate 4 des Silence-Gate-Plans: Report auf stderr, Text auf stdout.
    let report = engine::silence_gate(&captured.samples);
    eprintln!("Gate: {report}");
    if report.is_rejected() {
        println!();
        return 0;
    }

    let load_start = Instant::now();
    // §6.2: genau das Config-Modell, ausgewählt an dieser einen Stelle.
    let model = match engine::model_artifacts(&loaded.config.engine.model) {
        Ok(model) => model,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    let mut transcriber = match ParakeetTranscriber::load(&model, loaded.config.engine.threads) {
        Ok(t) => t,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    eprintln!(
        "SPIKE model_load_secs={:.3}",
        load_start.elapsed().as_secs_f64()
    );
    let infer_start = Instant::now();
    let result = match transcribe_pcm(&mut transcriber, &captured.samples).1 {
        Ok(r) => r,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    eprintln!(
        "SPIKE infer_secs={:.3}",
        infer_start.elapsed().as_secs_f64()
    );
    println!("{}", result.text);
    0
}

/// SPIKE (§4.3-Menü „Hotkey ändern…"): Dialog einmal öffnen und das Ergebnis
/// melden — ohne Daemon, ohne Tray, ohne Modell.
///
/// `autoclose_secs > 0` schließt das Fenster nach der Zeit von außen (das ist
/// ein Abbruch). Damit lässt sich der Dialog automatisiert prüfen; ohne den
/// Wert bleibt er offen, bis jemand ihn bedient.
#[cfg(windows)]
fn hotkey_dialog_test(autoclose_secs: u32) -> u8 {
    use hotkey_dialog::DialogOutcome;

    eprintln!("SPIKE --hotkey-dialog-test (kein Produktionspfad)");
    let spec = match config::load() {
        Ok(loaded) => HotkeySpec::from_config(&loaded.config.hotkey),
        Err(err) => {
            eprintln!("SPIKE hotkey-dialog: Config nicht lesbar ({err}) — Default");
            HotkeySpec::default()
        }
    };
    eprintln!("SPIKE hotkey-dialog: aktuell {}", spec.describe());

    if autoclose_secs > 0 {
        eprintln!("SPIKE hotkey-dialog: schließt nach {autoclose_secs}s von selbst");
        let delay = std::time::Duration::from_secs(u64::from(autoclose_secs));
        if let Err(err) = std::thread::Builder::new()
            .name("diktier-dialog-autoclose".into())
            .spawn(move || {
                std::thread::sleep(delay);
                let closed = hotkey_dialog::close_open_dialog();
                eprintln!("SPIKE hotkey-dialog: autoclose gefunden={closed}");
            })
        {
            eprintln!("SPIKE hotkey-dialog: Autoclose-Thread nicht startbar: {err}");
        }
    }

    match hotkey_dialog::ask(&spec) {
        Ok(DialogOutcome::Applied(next)) => {
            eprintln!("SPIKE hotkey-dialog: übernommen {}", next.describe());
            eprintln!(
                "SPIKE hotkey-dialog: key={:?} modifiers={:?}",
                next.key, next.modifiers
            );
            0
        }
        Ok(DialogOutcome::Cancelled) => {
            eprintln!("SPIKE hotkey-dialog: abgebrochen");
            0
        }
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

/// SPIKE (§4.5): Aufnahme-Overlay ohne Daemon zeigen — Gate B des
/// Overlay-Plans (Fokusprobe, Klickdurchgriff, DPI/Monitorwechsel) lässt sich
/// damit fahren, bevor Config und Kernzustand verdrahtet sind.
///
/// Der Pegel kommt vom Default-Gerät. Gibt es keins (oder streikt es), läuft
/// ein synthetischer Sweep — die Karte soll sich auch dann bewegen.
#[cfg(windows)]
fn overlay_test(secs: u32) -> u8 {
    eprintln!("SPIKE --overlay-test (kein Produktionspfad)");
    if secs == 0 {
        eprintln!("diktier: --overlay-test SECS muss ≥ 1 sein");
        return 2;
    }
    let loaded = match config::load() {
        Ok(loaded) => loaded,
        Err(err) => {
            eprintln!("{err}");
            return match err {
                ConfigError::Io(_) => 1,
                _ => 2,
            };
        }
    };

    let tap = audio::level::new_tap();
    let mut source = CpalAudioSource::new(&loaded.config.audio, Some(tap.clone()));
    let live = match source.start() {
        Ok(()) => true,
        Err(err) => {
            eprintln!("SPIKE overlay: kein Mikrofon ({err}) — synthetischer Sinus-Sweep");
            false
        }
    };
    eprintln!(
        "SPIKE overlay: pegelquelle={}",
        if live { "mikrofon" } else { "sweep" }
    );

    let mut window = match overlay::OverlayWindow::new() {
        Ok(window) => window,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    if let Err(err) = window.show_level() {
        eprintln!("{err}");
        return 1;
    }
    eprintln!("SPIKE overlay: karte {}", window.describe());
    eprintln!(
        "SPIKE overlay: {secs}s — jetzt in Notepad tippen (§4.2) und durch die Karte klicken"
    );

    let started = Instant::now();
    let end = started + std::time::Duration::from_secs(u64::from(secs));
    let mut last_note = started;
    while Instant::now() < end {
        window.pump();
        let level = if live {
            tap.take()
        } else {
            sweep_level(started.elapsed())
        };
        if let Err(err) = window.frame(level) {
            eprintln!("{err}");
            return 1;
        }
        if last_note.elapsed() >= std::time::Duration::from_secs(3) {
            last_note = Instant::now();
            eprintln!("SPIKE overlay: karte {}", window.describe());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    window.hide();
    if live {
        let _ = source.stop();
    }
    drop(window);
    eprintln!("SPIKE overlay-test: {secs}s vorbei");
    0
}

/// Ersatzpegel ohne Mikrofon: langsam an- und abschwellend, mit kleinem
/// Flattern — genug, damit Waveform, Meter und Peak-Hold erkennbar arbeiten.
#[cfg(windows)]
fn sweep_level(elapsed: std::time::Duration) -> f32 {
    let t = elapsed.as_secs_f32();
    let envelope = 0.5 - 0.5 * (t * 0.8).cos();
    let flutter = 0.5 + 0.5 * (t * 13.0).sin();
    (envelope * (0.25 + 0.75 * flutter)).clamp(0.0, 1.0)
}

fn tray_test(secs: u32) -> u8 {
    eprintln!("SPIKE --tray-test (kein Produktionspfad)");
    if secs == 0 {
        eprintln!("diktier: --tray-test SECS muss ≥ 1 sein");
        return 2;
    }

    let loaded = match config::load() {
        Ok(loaded) => loaded,
        Err(err) => {
            eprintln!("{err}");
            return match err {
                ConfigError::Io(_) => 1,
                _ => 2,
            };
        }
    };
    for warning in &loaded.warnings {
        eprintln!("Warnung: {warning}");
    }

    let model = loaded.config.engine.model.clone();
    let cycle = tray_cycle();
    let mut runtime = cycle[0].clone();
    let mut tray = match tray::new_backend(&runtime, &model) {
        Ok(tray) => tray,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    eprintln!("SPIKE tray-backend={}", tray.backend_name());
    eprintln!(
        "SPIKE tray: zustand={} tooltip={}",
        tray::tray_status(&runtime).as_str(),
        tray::tooltip_text(&runtime, &model)
    );

    let end = Instant::now() + std::time::Duration::from_secs(u64::from(secs));
    let mut next_rotate = Instant::now() + std::time::Duration::from_secs(5);
    let mut idx = 0usize;
    loop {
        if Instant::now() >= end {
            eprintln!("SPIKE tray-test: {secs}s vorbei");
            break;
        }
        match tray.poll() {
            Ok(Some(TrayEvent::Quit)) => {
                eprintln!("SPIKE tray: event={}", TrayEvent::Quit.as_str());
                break;
            }
            Ok(Some(event)) => {
                eprintln!("SPIKE tray: event={}", event.as_str());
                if event == TrayEvent::OpenConfigDir {
                    match tray::open_config() {
                        Ok(()) => {
                            if let Ok(path) = config::config_path() {
                                eprintln!("SPIKE tray: config-datei={}", path.display());
                            }
                        }
                        Err(err) => eprintln!("SPIKE tray: config-datei: {err}"),
                    }
                }
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(err) => {
                eprintln!("{err}");
                return 1;
            }
        }
        if Instant::now() >= next_rotate {
            idx = (idx + 1) % cycle.len();
            runtime = cycle[idx].clone();
            if let Err(err) = tray.update(&runtime, &model) {
                eprintln!("{err}");
                return 1;
            }
            eprintln!(
                "SPIKE tray: zustand={} tooltip={}",
                tray::tray_status(&runtime).as_str(),
                tray::tooltip_text(&runtime, &model)
            );
            next_rotate += std::time::Duration::from_secs(5);
        }
    }
    0
}

fn tray_cycle() -> [Runtime; 8] {
    let state = |state: AppState, paused: bool| Runtime {
        state,
        paused,
        ..Runtime::default()
    };
    [
        state(AppState::Starting, false),
        state(AppState::Downloading, false),
        state(AppState::Loading, false),
        state(AppState::Idle, false),
        state(
            AppState::Recording {
                source: RecordingSource::TrayClick,
            },
            false,
        ),
        state(
            AppState::Transcribing {
                source: RecordingSource::TrayClick,
            },
            false,
        ),
        state(AppState::Error, false),
        state(AppState::Idle, true),
    ]
}

fn run_daemon(foreground: bool) -> u8 {
    daemon::run(foreground)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_exits_0() {
        assert_eq!(cli_main(["diktier", "--help"]), 0);
    }

    #[test]
    fn version_exits_0() {
        assert_eq!(cli_main(["diktier", "--version"]), 0);
    }

    #[test]
    fn unknown_flag_exits_2() {
        assert_eq!(cli_main(["diktier", "--nope"]), 2);
    }

    // Die Wirkung von `--install-autostart` / `--remove-autostart` prüft
    // `autostart::tests` gegen ein Temp-`HOME`: hier aufgerufen würden sie im
    // echten `~/.config/autostart` schreiben.

    #[test]
    fn autostart_and_foreground_conflict_exit_2() {
        assert_eq!(
            cli_main(["diktier", "--foreground", "--install-autostart"]),
            2
        );
        assert_eq!(
            cli_main(["diktier", "--foreground", "--remove-autostart"]),
            2
        );
    }

    #[test]
    fn conflicting_autostart_flags_exit_2() {
        assert_eq!(
            cli_main(["diktier", "--install-autostart", "--remove-autostart"]),
            2
        );
    }

    #[test]
    fn transcribe_wav_without_path_exits_2() {
        assert_eq!(cli_main(["diktier", "--transcribe-wav"]), 2);
    }

    #[test]
    fn transcribe_wav_missing_file_exits_1() {
        assert_eq!(
            cli_main([
                "diktier",
                "--transcribe-wav",
                "/no/such/diktier-missing.wav"
            ]),
            1
        );
    }

    #[test]
    fn transcribe_wav_invalid_format_exits_2() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        writer.write_sample(0_i16).unwrap();
        writer.finalize().unwrap();
        assert_eq!(
            cli_main([
                "diktier",
                "--transcribe-wav",
                path.to_str().expect("utf-8 path"),
            ]),
            2
        );
    }

    #[test]
    fn gate_analyze_without_file_exits_2() {
        assert_eq!(cli_main(["diktier", "--gate-analyze"]), 2);
    }

    #[test]
    fn gate_analyze_conflicts_with_transcribe_wav() {
        assert_eq!(
            cli_main([
                "diktier",
                "--gate-analyze",
                "a.wav",
                "--transcribe-wav",
                "b.wav"
            ]),
            2
        );
    }

    #[test]
    fn gate_analyze_missing_file_exits_1() {
        assert_eq!(
            cli_main(["diktier", "--gate-analyze", "/no/such/diktier-missing.wav"]),
            1
        );
    }

    /// §6.4-Werkzeug: mehrere Dateien in einem Lauf, ohne Modell. Die
    /// 5-s-Rampe reicht für volle Fenster und einen floor.
    #[test]
    fn gate_analyze_reads_wavs_without_model() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ton.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..16_000 * 5 {
            let loud = (16_000 * 2..16_000 * 4).contains(&i);
            writer
                .write_sample(if loud { 2_000_i16 } else { 30_i16 })
                .unwrap();
        }
        writer.finalize().unwrap();
        let arg = path.to_str().expect("utf-8 path");
        assert_eq!(cli_main(["diktier", "--gate-analyze", arg, arg]), 0);
    }

    /// WP2: `--roundtrip` gibt es nur zusammen mit `--clipboard-check`. Der
    /// Check selbst läuft hier nicht — er läse die echte Zwischenablage.
    #[test]
    fn roundtrip_requires_clipboard_check() {
        assert_eq!(cli_main(["diktier", "--roundtrip"]), 2);
    }

    #[test]
    fn clipboard_check_conflicts_with_other_modes() {
        assert_eq!(
            cli_main(["diktier", "--clipboard-check", "--transcribe-wav", "a.wav"]),
            2
        );
        assert_eq!(
            cli_main(["diktier", "--clipboard-check", "--install-autostart"]),
            2
        );
        assert_eq!(
            cli_main(["diktier", "--clipboard-check", "--gate-analyze", "a.wav"]),
            2
        );
    }

    #[test]
    fn clipboard_check_help_warns() {
        use clap::CommandFactory;
        let mut cmd = Cli::command();
        let help = cmd.render_long_help().to_string();
        assert!(help.contains("--clipboard-check"), "{help}");
        assert!(help.contains("ÜBERSCHREIBT"), "{help}");
        assert!(help.contains("verzögert"), "{help}");
    }

    #[test]
    fn runs_without_transcribe_wav_exits_2() {
        assert_eq!(cli_main(["diktier", "--runs", "5"]), 2);
        assert_eq!(
            cli_main(["diktier", "--transcribe-list", "l.txt", "--runs", "0"]),
            2
        );
    }

    /// Zwei Sekunden digitale Stille: Regel C, nie ein Modell.
    fn silent_wav(dir: &std::path::Path) -> String {
        let path = dir.join("still.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for _ in 0..16_000 * 2 {
            writer.write_sample(0_i16).unwrap();
        }
        writer.finalize().unwrap();
        path.to_str().expect("utf-8 path").to_string()
    }

    const ULTRA: &str = "parakeet-ultra-0.6b-int8-pc";

    /// §9 (v1.10): `--model` gibt es nur mit einem Transkriptionsmodus.
    #[test]
    fn model_without_transcription_mode_exits_2() {
        assert_eq!(cli_main(["diktier", "--model", config::DEFAULT_MODEL]), 2);
        assert_eq!(cli_main(["diktier", "--foreground", "--model", ULTRA]), 2);
        assert_eq!(
            cli_main(["diktier", "--gate-analyze", "a.wav", "--model", ULTRA]),
            2
        );
        assert_eq!(
            cli_main(["diktier", "--transcribe-wav", "a.wav", "--model"]),
            2
        );
    }

    /// Nur Manifest-Schlüssel; die Prüfung kommt vor Config und Datei.
    #[test]
    fn unknown_model_key_exits_2() {
        let missing = "/no/such/diktier-missing.wav";
        assert_eq!(
            cli_main(["diktier", "--transcribe-wav", missing, "--model", "nope"]),
            2
        );
        assert_eq!(
            cli_main([
                "diktier",
                "--transcribe-list",
                missing,
                "--model",
                "Parakeet-TDT-0.6b-v3-int8"
            ]),
            2
        );
    }

    /// L1 (Review WP2): Clap nimmt Autostart mit `--model`/`--runs` an; die
    /// Optionsprüfung — in `cli_main` der erste Schritt nach dem Parsen, vor
    /// jeder Aktion — lehnt es mit 2 ab, auch mit gültigem Schlüssel. Nur
    /// geparst, nie `cli_main`: der echte Autostart bleibt unberührt.
    #[test]
    fn mode_options_are_checked_before_autostart() {
        for action in ["--install-autostart", "--remove-autostart"] {
            for extra in [
                vec!["--model", "nope"],
                vec!["--model", config::DEFAULT_MODEL],
                vec!["--model", ULTRA],
                vec!["--runs", "3"],
            ] {
                let mut args = vec!["diktier", action];
                args.extend(extra.iter().copied());
                let cli = Cli::try_parse_from(args.clone()).expect("Clap nimmt es an");
                assert_eq!(check_mode_options(&cli), Err(2), "{args:?}");
            }
            let cli = Cli::try_parse_from(["diktier", action]).unwrap();
            assert_eq!(check_mode_options(&cli), Ok(()), "{action} allein");
        }
    }

    /// §9 (v1.10): `--manifest-sha256` gibt es nur allein; die Prüfung selbst
    /// kommt ohne Modell und Config aus.
    #[test]
    fn manifest_sha256_is_exclusive_and_needs_nothing() {
        assert_eq!(cli_main(["diktier", "--manifest-sha256"]), 0);
        for other in [
            vec!["--foreground"],
            vec!["--install-autostart"],
            vec!["--model", ULTRA],
            vec!["--transcribe-list", "l.txt"],
        ] {
            let mut args = vec!["diktier", "--manifest-sha256"];
            args.extend(other.iter().copied());
            assert_eq!(cli_main(args.clone()), 2, "{args:?}");
        }
    }

    #[test]
    fn transcribe_list_parser_conflicts_exit_2() {
        assert_eq!(cli_main(["diktier", "--transcribe-list"]), 2);
        for other in [
            vec!["--transcribe-wav", "a.wav"],
            vec!["--gate-analyze", "a.wav"],
            vec!["--clipboard-check"],
            vec!["--install-autostart"],
            vec!["--foreground", "--record-test", "3"],
        ] {
            let mut args = vec!["diktier", "--transcribe-list", "l.txt"];
            args.extend(other.iter().copied());
            assert_eq!(cli_main(args.clone()), 2, "{args:?}");
        }
    }

    #[test]
    fn transcribe_list_unreadable_or_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("fehlt.txt");
        assert_eq!(
            cli_main(["diktier", "--transcribe-list", missing.to_str().unwrap()]),
            1
        );
        let empty = dir.path().join("leer.txt");
        std::fs::write(&empty, "\n  \n").unwrap();
        assert_eq!(
            cli_main(["diktier", "--transcribe-list", empty.to_str().unwrap()]),
            2
        );
    }

    /// Ende zu Ende ohne Modell: nur abgelehnte Aufnahmen, `--runs` und ein
    /// anderer Schlüssel als die Config — das Modell wird nie gebraucht.
    #[test]
    fn transcribe_list_with_only_rejected_files_needs_no_model() {
        let dir = tempfile::tempdir().unwrap();
        let wav = silent_wav(dir.path());
        let list = dir.path().join("liste.txt");
        std::fs::write(&list, format!("{wav}\r\n\r\n{wav}\r\n")).unwrap();
        let list = list.to_str().unwrap();
        assert_eq!(
            cli_main([
                "diktier",
                "--transcribe-list",
                list,
                "--model",
                ULTRA,
                "--runs",
                "2"
            ]),
            0
        );
        assert_eq!(
            cli_main(["diktier", "--transcribe-wav", &wav, "--model", ULTRA]),
            0
        );
    }

    #[test]
    fn inject_test_without_foreground_exits_2() {
        assert_eq!(cli_main(["diktier", "--inject-test", "hi"]), 2);
    }

    #[test]
    fn hotkey_test_without_foreground_exits_2() {
        assert_eq!(cli_main(["diktier", "--hotkey-test"]), 2);
    }

    #[test]
    fn record_test_without_foreground_exits_2() {
        assert_eq!(cli_main(["diktier", "--record-test", "3"]), 2);
    }

    #[test]
    fn tray_test_without_foreground_exits_2() {
        assert_eq!(cli_main(["diktier", "--tray-test", "5"]), 2);
    }

    #[test]
    fn hotkey_dialog_test_without_foreground_exits_2() {
        assert_eq!(cli_main(["diktier", "--hotkey-dialog-test"]), 2);
        assert_eq!(cli_main(["diktier", "--hotkey-dialog-test", "5"]), 2);
    }

    #[test]
    fn tray_test_zero_exits_2() {
        assert_eq!(cli_main(["diktier", "--foreground", "--tray-test", "0"]), 2);
    }

    /// §4.5-Spike: wie die übrigen SPIKE-Flags nur mit `--foreground`, und
    /// eine Laufzeit von 0 s ergibt kein sinnvolles Overlay.
    #[test]
    fn overlay_test_without_foreground_exits_2() {
        assert_eq!(cli_main(["diktier", "--overlay-test"]), 2);
        assert_eq!(cli_main(["diktier", "--overlay-test", "5"]), 2);
    }

    #[test]
    fn overlay_test_zero_exits_2() {
        assert_eq!(
            cli_main(["diktier", "--foreground", "--overlay-test", "0"]),
            2
        );
    }

    /// Zwei Spikes gleichzeitig ergeben keinen Sinn — clap fängt das ab.
    #[test]
    fn overlay_test_conflicts_with_the_other_spikes() {
        assert_eq!(
            cli_main([
                "diktier",
                "--foreground",
                "--overlay-test",
                "5",
                "--tray-test",
                "5"
            ]),
            2
        );
    }
}
