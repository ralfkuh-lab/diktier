mod audio;
mod config;
mod download;
mod engine;
mod hotkey;
mod inject;
mod state;
mod tray;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use clap::Parser;

use audio::{AudioError, AudioSource, StubAudioSource};
use config::ConfigError;
use download::load_manifest;
use engine::{ParakeetTranscriber, StubTranscriber, Transcriber, transcribe_pcm};
use hotkey::{HotkeyBackend, new_backend};
use inject::{OutputSink, StubOutputSink};
use state::Runtime;
use tray::{StubTray, TrayBackend};

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
    #[arg(long, value_name = "DATEI", conflicts_with_all = ["install_autostart", "remove_autostart"])]
    transcribe_wav: Option<PathBuf>,

    /// Gemessene Inferenzläufe nach einem ungezählten Warmup (nur mit --transcribe-wav).
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
    runs: u32,
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

    if cli.install_autostart {
        eprintln!("diktier: --install-autostart ist noch nicht implementiert");
        return 1;
    }
    if cli.remove_autostart {
        eprintln!("diktier: --remove-autostart ist noch nicht implementiert");
        return 1;
    }
    if cli.runs != 1 && cli.transcribe_wav.is_none() {
        eprintln!("diktier: --runs gilt nur zusammen mit --transcribe-wav");
        return 2;
    }
    if let Some(path) = cli.transcribe_wav {
        return transcribe_wav(&path, cli.runs);
    }

    run_daemon(cli.foreground)
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

    if pcm.len() < engine::MIN_SAMPLES_16KHZ {
        eprintln!("Aufnahme < 250 ms, Engine nicht aufgerufen.");
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

fn run_daemon(foreground: bool) -> u8 {
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

    let manifest = match load_manifest() {
        Ok(manifest) => manifest,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };

    if foreground {
        eprintln!("diktier: --foreground");
    }

    let mut transcriber = StubTranscriber;
    let mut sink = StubOutputSink;
    let mut hotkey = new_backend();
    let mut audio = StubAudioSource;
    let mut tray = StubTray;
    let runtime = Runtime::default();

    if let Err(err) = hotkey.register() {
        eprintln!("{err}");
        return 1;
    }
    if let Err(err) = tray.update(&runtime, &manifest.key) {
        eprintln!("{err}");
        return 1;
    }
    let _ = transcriber.transcribe(&[]);
    let _ = sink.copy_only("");
    let _ = audio.start();
    let _ = audio.stop();

    eprintln!("Phase-0-Gerüst: Daemon-Schleife noch nicht verdrahtet.");
    0
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

    #[test]
    fn install_autostart_is_stub_exit_1() {
        assert_eq!(cli_main(["diktier", "--install-autostart"]), 1);
    }

    #[test]
    fn remove_autostart_is_stub_exit_1() {
        assert_eq!(cli_main(["diktier", "--remove-autostart"]), 1);
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
}
