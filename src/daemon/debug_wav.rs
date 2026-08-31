//! `DIKTIER_DEBUG_WAV=1` (Spec §10): genau ein Dump der letzten Aufnahme.
//!
//! `%TEMP%\diktier\last_recording.wav`; jede neue Aufnahme überschreibt die
//! Datei **atomar** (Temp + Rename im selben Verzeichnis).
//! Nie hochladen — deshalb steht der Pfad genau einmal im Log.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::audio::ENGINE_RATE;

const FILE_NAME: &str = "last_recording.wav";
const TEMP_NAME: &str = "last_recording.wav.part";

/// `DIKTIER_DEBUG_WAV=1` — nur exakt `1` schaltet den Dump ein.
pub fn enabled() -> bool {
    std::env::var_os("DIKTIER_DEBUG_WAV").is_some_and(|value| value == "1")
}

/// Zielverzeichnis des Dumps.
pub fn debug_dir() -> PathBuf {
    let tmp = std::env::var_os("TEMP")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    tmp.join("diktier")
}

/// Schreibt 16-kHz-mono-f32 als 16-bit-PCM-WAV nach `<dir>/last_recording.wav`.
/// Rückgabe ist der endgültige Pfad.
pub fn write_last_recording(dir: &Path, samples: &[f32]) -> io::Result<PathBuf> {
    create_private_dir(dir)?;
    let temp = dir.join(TEMP_NAME);
    let final_path = dir.join(FILE_NAME);

    let file = create_private_file(&temp)?;
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: ENGINE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::new(io::BufWriter::new(file), spec).map_err(hound_io)?;
    for sample in samples {
        writer.write_sample(to_i16(*sample)).map_err(hound_io)?;
    }
    writer.finalize().map_err(hound_io)?;

    // Atomar: Rename im selben Verzeichnis ersetzt den vorherigen Dump.
    match fs::rename(&temp, &final_path) {
        Ok(()) => Ok(final_path),
        Err(err) => {
            let _ = fs::remove_file(&temp);
            Err(err)
        }
    }
}

fn to_i16(sample: f32) -> i16 {
    let clamped = sample.clamp(-1.0, 1.0);
    (clamped * i16::MAX as f32).round() as i16
}

fn hound_io(err: hound::Error) -> io::Error {
    match err {
        hound::Error::IoError(io) => io,
        other => io::Error::other(other.to_string()),
    }
}

/// Das Dump-Verzeichnis liegt unter `%TEMP%` im Benutzerprofil und erbt damit
/// dessen ACL.
fn create_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)
}

fn create_private_file(path: &Path) -> io::Result<fs::File> {
    fs::File::create(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::read_wav_16k_mono;

    #[test]
    fn writes_the_wav_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("diktier-test");
        let samples: Vec<f32> = (0..16_000).map(|i| (i as f32 / 16_000.0) - 0.5).collect();
        let path = write_last_recording(&target, &samples).unwrap();
        assert!(path.ends_with(FILE_NAME));
        assert!(!target.join(TEMP_NAME).exists(), "Temp-Datei aufgeräumt");

        let read_back = read_wav_16k_mono(&path).unwrap();
        assert_eq!(read_back.len(), samples.len());
        assert!((read_back[0] - samples[0]).abs() < 1e-3);
    }

    #[test]
    fn second_dump_replaces_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("diktier-test");
        write_last_recording(&target, &vec![0.0; 4_000]).unwrap();
        let path = write_last_recording(&target, &vec![0.25; 8_000]).unwrap();
        let read_back = read_wav_16k_mono(&path).unwrap();
        assert_eq!(read_back.len(), 8_000, "genau ein Dump, überschrieben");
        let entries: Vec<_> = fs::read_dir(&target)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries.len(), 1, "kein Rest, kein Verlauf: {entries:?}");
    }

    #[test]
    fn clipping_stays_in_range() {
        assert_eq!(to_i16(2.0), i16::MAX);
        assert_eq!(to_i16(-2.0), -i16::MAX);
        assert_eq!(to_i16(0.0), 0);
    }
}
