mod audio;
mod autostart;
mod config;
mod daemon;
mod download;
mod engine;
mod hotkey;
mod inject;
mod paths;
mod single_instance;
mod state;
mod tray;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use clap::Parser;

use audio::{AudioError, AudioSource, CpalAudioSource};
use config::ConfigError;
use engine::{ParakeetTranscriber, transcribe_pcm};
use hotkey::{HotkeyBackend, HotkeyEvent, new_backend};
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

    /// Gemessene Inferenzläufe nach einem ungezählten Warmup (nur mit --transcribe-wav).
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
    runs: u32,

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

    /// SPIKE: Tray SECS Sekunden anzeigen, Zustände rotieren (nur mit --foreground).
    #[arg(
        long,
        value_name = "SECS",
        conflicts_with_all = ["install_autostart", "remove_autostart", "transcribe_wav", "inject_test", "hotkey_test", "record_test"]
    )]
    tray_test: Option<u32>,
}

fn main() -> ExitCode {
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

    // §5.3: Die CLI-Modi laufen **vor** der Single-Instance-Sperre und fordern
    // sie nie an; §10: sie loggen nur nach stderr, nie in `diktier.log`.
    if cli.install_autostart {
        return install_autostart();
    }
    if cli.remove_autostart {
        return remove_autostart();
    }
    if cli.runs != 1 && cli.transcribe_wav.is_none() {
        eprintln!("diktier: --runs gilt nur zusammen mit --transcribe-wav");
        return 2;
    }
    if let Some(path) = cli.transcribe_wav {
        return transcribe_wav(&path, cli.runs);
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

fn transcribe_wav(path: &std::path::Path, runs: u32) -> u8 {
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

    let rms = engine::rms_f32(&pcm);
    eprintln!("SPIKE rms={rms:.6}");
    if engine::is_silence_or_short(&pcm) {
        if pcm.len() < engine::MIN_SAMPLES_16KHZ {
            eprintln!("Aufnahme < 250 ms, Engine nicht aufgerufen.");
        }
        println!();
        return 0;
    }

    let load_start = Instant::now();
    let mut transcriber = match ParakeetTranscriber::load(
        &loaded.config.engine.model,
        loaded.config.engine.threads,
    ) {
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

    if let Err(err) = transcribe_pcm(&mut transcriber, &pcm) {
        eprintln!("{err}");
        return 1;
    }

    let mut last = engine::Transcription::empty();
    for _ in 0..runs {
        let infer_start = Instant::now();
        match transcribe_pcm(&mut transcriber, &pcm) {
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
            eprintln!("SPIKE restored={restored} ({})", restore.as_str());
        }
        InjectOutcome::CopyOnly { reason } => {
            eprintln!("SPIKE pfad=copy_only");
            eprintln!("SPIKE grund={}", reason.as_str());
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
    eprintln!("SPIKE: F9 30s lang halten/loslassen; Exit mit Ctrl+C");
    let mut backend = new_backend();
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

    let mut src = CpalAudioSource::new(&loaded.config.audio);
    let t_cap = Instant::now();
    if let Err(err) = src.start() {
        eprintln!("{err}");
        return 1;
    }
    std::thread::sleep(std::time::Duration::from_secs(u64::from(secs)));
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

    let rms = engine::rms_f32(&captured.samples);
    eprintln!("SPIKE rms={rms:.6}");
    if engine::is_silence_or_short(&captured.samples) {
        if captured.samples.len() < engine::MIN_SAMPLES_16KHZ {
            eprintln!("Aufnahme < 250 ms, Engine nicht aufgerufen.");
        }
        println!();
        return 0;
    }

    let load_start = Instant::now();
    let mut transcriber = match ParakeetTranscriber::load(
        &loaded.config.engine.model,
        loaded.config.engine.threads,
    ) {
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
    let result = match transcribe_pcm(&mut transcriber, &captured.samples) {
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
        "SPIKE tray-sni=org.kde.StatusNotifierItem-{}-1",
        std::process::id()
    );
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
                    match tray::open_config_dir() {
                        Ok(()) => {
                            if let Ok(dir) = tray::config_dir() {
                                eprintln!("SPIKE tray: config-ordner={}", dir.display());
                            }
                        }
                        Err(err) => eprintln!("SPIKE tray: config-ordner: {err}"),
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
    fn runs_without_transcribe_wav_exits_2() {
        assert_eq!(cli_main(["diktier", "--runs", "5"]), 2);
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
    fn tray_test_zero_exits_2() {
        assert_eq!(cli_main(["diktier", "--foreground", "--tray-test", "0"]), 2);
    }
}
