mod audio;
mod config;
mod download;
mod engine;
mod hotkey;
mod inject;
mod state;
mod tray;

use std::ffi::OsString;
use std::process::ExitCode;

use clap::Parser;

use audio::{AudioSource, StubAudioSource};
use config::ConfigError;
use download::load_manifest;
use engine::{StubTranscriber, Transcriber};
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

    run_daemon(cli.foreground)
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
}
