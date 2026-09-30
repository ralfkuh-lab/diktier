//! `DIKTIER_DEBUG_WAV=1` (Spec §10, v1.8): Ring der letzten Aufnahmen.
//!
//! Seit v1.10 einstellbar, einmal beim Daemon-Start gelesen ([`from_env`]):
//! `DIKTIER_DEBUG_WAV_KEEP` (1–5000, Default [`DEFAULT_KEEP`]) und
//! `DIKTIER_DEBUG_WAV_DIR` (absolut, Default `%TEMP%\diktier`).
//!
//! `<dir>\rec_<UTC bis ms>_lauf-<N>.wav`, z. B.
//! `rec_2026-09-25T14-47-13-512Z_lauf-703.wav`. Jede Datei entsteht **atomar**
//! über eine eigene, exklusiv angelegte Temp-Datei
//! `<ziel>.<pid>-<zähler>.part` im selben Verzeichnis und einen Rename, der
//! **nie ersetzt**: Ist der Zielname schon belegt (Neustart, Uhrkorrektur,
//! gleicher Lauf zweimal), bekommt die neue Datei `-2`, `-3` … angehängt
//! (Final-Review Blocker 3). Erst nach dem Rename zählt sie; danach werden die
//! ältesten Dateien des **genauen** Musters über der Kapazität gelöscht, verwaiste
//! `.part`-Reste desselben Musters ab einer Stunde Alter entfernt und — nur im
//! Default-Verzeichnis `%TEMP%\diktier`, wo sie entstand — die Altlast
//! `last_recording.wav` (bis 0.3.0) gelöscht. In einem über
//! `DIKTIER_DEBUG_WAV_DIR` gewählten Verzeichnis ist eine Datei dieses Namens
//! fremd (Code-Review WP2, W1). Fremde Dateien bleiben unangetastet. Ein
//! gescheiterter Schreibvorgang löscht nur die eigene Temp-Datei. Nie
//! hochladen — deshalb steht der Pfad genau einmal im Log.
//!
//! Format seit v1.9: 16 kHz mono 32-bit-Float, bitgleich zum Capture-Puffer —
//! also ohne die Vorlauf-Stille, die erst `engine::transcribe_pcm` voranstellt.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::logging::civil_from_days;
use crate::audio::ENGINE_RATE;
use crate::state::RunId;

/// So viele Dumps bleiben ohne `DIKTIER_DEBUG_WAV_KEEP` liegen (Plan
/// Leitentscheidung 11 des Clipboard-Pakets).
pub const DEFAULT_KEEP: usize = 10;
/// Gültiger Bereich von `DIKTIER_DEBUG_WAV_KEEP` (Spec §10, v1.10).
const KEEP_MIN: usize = 1;
const KEEP_MAX: usize = 5000;

const ENV_SWITCH: &str = "DIKTIER_DEBUG_WAV";
const ENV_KEEP: &str = "DIKTIER_DEBUG_WAV_KEEP";
const ENV_DIR: &str = "DIKTIER_DEBUG_WAV_DIR";

const PREFIX: &str = "rec_";
const RUN_SEP: &str = "_lauf-";
const EXT: &str = ".wav";
const PART_EXT: &str = ".part";
/// Dump-Datei bis 0.3.0 — wird nach dem ersten erfolgreichen Dump entfernt.
const LEGACY_NAME: &str = "last_recording.wav";
/// Jüngere `.part`-Reste könnten noch geschrieben werden: nicht anfassen.
const STALE_PART_AGE: Duration = Duration::from_secs(3600);
/// `YYYY-MM-DDThh-mm-ss-mmmZ`
const STAMP_LEN: usize = 24;
/// Obergrenze für Namenssuffix und Temp-Zähler — danach Fehler statt
/// Endlosschleife.
const MAX_ATTEMPTS: u32 = 1000;

/// Prozessweiter Zähler für eindeutige Temp-Namen.
static PART_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Der effektive Dump: Verzeichnis und Kapazität des Rings. Es gibt ihn nur,
/// wenn `DIKTIER_DEBUG_WAV=1` gesetzt ist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugWavConfig {
    pub dir: PathBuf,
    pub keep: usize,
    /// Nur wenn `dir` der Default ist (kein oder ein ungültiges
    /// `DIKTIER_DEBUG_WAV_DIR`): dort ist `last_recording.wav` die eigene
    /// Altlast und wird entfernt. Sonst nie.
    pub legacy_cleanup: bool,
}

/// Ergebnis des einmaligen Lesens beim Start: der Dump (`None` = aus) und je
/// ungültigem Wert genau eine Warnzeile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub config: Option<DebugWavConfig>,
    pub warnings: Vec<String>,
    /// Aus, obwohl `DIKTIER_DEBUG_WAV_KEEP` oder `_DIR` gesetzt ist: Dann
    /// fehlt vermutlich nur der Schalter (Sol-Review zum Alltagstest, W1).
    pub ignored_settings: bool,
}

impl Resolved {
    /// Die Startzeile: bei eingeschaltetem Dump Verzeichnis und Kapazität,
    /// ohne Inhalte. Aus: nur wenn KEEP/DIR gesetzt sind, sonst keine Zeile.
    pub fn start_line(&self) -> Option<String> {
        match &self.config {
            Some(config) => Some(format!(
                "Debug-WAV an: {}, behalte {}",
                config.dir.display(),
                config.keep
            )),
            None if self.ignored_settings => Some(format!(
                "Debug-WAV aus: {ENV_KEEP}/{ENV_DIR} gesetzt, aber {ENV_SWITCH} ist nicht 1"
            )),
            None => None,
        }
    }
}

/// Liest die Variablen **einmal** aus der Prozessumgebung (Daemon-Start).
pub fn from_env() -> Resolved {
    resolve(
        std::env::var_os(ENV_SWITCH).as_deref(),
        std::env::var_os(ENV_KEEP).as_deref(),
        std::env::var_os(ENV_DIR).as_deref(),
        default_dir(std::env::var_os("TEMP").as_deref()),
    )
}

/// Reine Auswertung von Schalter, Kapazität und Verzeichnis. Nur exakt `1`
/// schaltet den Dump ein. Ist er aus, werden KEEP und DIR nicht geprüft —
/// sie wirken dann ohnehin nicht.
pub fn resolve(
    switch: Option<&OsStr>,
    keep: Option<&OsStr>,
    dir: Option<&OsStr>,
    default_dir: PathBuf,
) -> Resolved {
    if switch.is_none_or(|value| value != "1") {
        return Resolved {
            config: None,
            warnings: Vec::new(),
            ignored_settings: keep.is_some() || dir.is_some(),
        };
    }
    let mut warnings = Vec::new();
    let keep = match keep.map(parse_keep) {
        None => DEFAULT_KEEP,
        Some(Ok(keep)) => keep,
        Some(Err(problem)) => {
            warnings.push(format!(
                "{ENV_KEEP} {problem} — nehme den Default {DEFAULT_KEEP}"
            ));
            DEFAULT_KEEP
        }
    };
    let (dir, legacy_cleanup) = match dir.map(parse_dir) {
        None => (default_dir, true),
        Some(Ok(dir)) => (dir, false),
        Some(Err(problem)) => {
            warnings.push(format!(
                "{ENV_DIR} {problem} — nehme den Default {}",
                default_dir.display()
            ));
            (default_dir, true)
        }
    };
    Resolved {
        config: Some(DebugWavConfig {
            dir,
            keep,
            legacy_cleanup,
        }),
        warnings,
        ignored_settings: false,
    }
}

/// `DIKTIER_DEBUG_WAV_KEEP`: nur Ziffern (kein Vorzeichen, kein Leerraum),
/// Wert 1–5000. Der Fehlertext nennt den Grund, nicht den Wert.
fn parse_keep(raw: &OsStr) -> Result<usize, String> {
    let Some(text) = raw.to_str().filter(|text| is_number(text)) else {
        return Err(format!("ist keine ganze Zahl ({KEEP_MIN}–{KEEP_MAX})"));
    };
    match text.parse::<usize>() {
        Ok(keep) if (KEEP_MIN..=KEEP_MAX).contains(&keep) => Ok(keep),
        _ => Err(format!("liegt außerhalb von {KEEP_MIN}–{KEEP_MAX}")),
    }
}

/// `DIKTIER_DEBUG_WAV_DIR`: nicht leer (auch nicht nur Leerraum) und absolut.
fn parse_dir(raw: &OsStr) -> Result<PathBuf, String> {
    if raw.to_string_lossy().trim().is_empty() {
        return Err("ist leer".to_owned());
    }
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return Err("ist kein absoluter Pfad".to_owned());
    }
    Ok(path)
}

/// Default-Verzeichnis `%TEMP%\diktier`, ohne `TEMP` das System-Temp.
fn default_dir(temp: Option<&OsStr>) -> PathBuf {
    temp.map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("diktier")
}

/// `rec_<UTC bis ms>_lauf-<N>.wav`. Doppelpunkte und Punkt sind durch `-`
/// ersetzt, damit der Name unter Windows gültig ist und lexikografisch nach
/// der Zeit sortiert.
pub fn file_name(at: SystemTime, run: RunId) -> String {
    format!("{PREFIX}{}{RUN_SEP}{}{EXT}", stamp(at), run.0)
}

/// Der Name bei belegtem Ziel: `suffix` 1 ist der Grundname, ab 2
/// `rec_…_lauf-<N>-<suffix>.wav`.
fn suffixed_name(base: &str, suffix: u32) -> String {
    if suffix <= 1 {
        return base.to_owned();
    }
    let stem = base.strip_suffix(EXT).unwrap_or(base);
    format!("{stem}-{suffix}{EXT}")
}

fn stamp(at: SystemTime) -> String {
    let millis = at
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let secs = millis.div_euclid(1_000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}-{:02}-{:02}-{:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60,
        millis.rem_euclid(1_000)
    )
}

/// Ein Name des genauen Musters, zerlegt: Zeitstempel, Laufnummer und
/// Kollisionssuffix (1 = ohne).
#[derive(Debug, Clone, PartialEq, Eq)]
struct RingName<'a> {
    stamp: &'a str,
    run: u64,
    suffix: u32,
}

/// Zerlegt einen Namen des genauen Musters
/// `rec_<stamp>_lauf-<N>[-<k>].wav` mit `k ≥ 2` ohne führende Null. Alles
/// andere — auch `.part`, andere Endungen, Großschreibung, unmögliche
/// Kalenderwerte — ist fremd.
fn parse_name(name: &str) -> Option<RingName<'_>> {
    let rest = name.strip_prefix(PREFIX)?.strip_suffix(EXT)?;
    let (stamp, tail) = rest.split_once(RUN_SEP)?;
    let (run, suffix) = match tail.split_once('-') {
        Some((run, suffix)) => {
            if !is_number(suffix) || suffix.starts_with('0') {
                return None;
            }
            let suffix: u32 = suffix.parse().ok()?;
            if suffix < 2 {
                return None;
            }
            (run, suffix)
        }
        None => (tail, 1),
    };
    if !is_stamp(stamp) || !is_number(run) {
        return None;
    }
    Some(RingName {
        stamp,
        run: run.parse().ok()?,
        suffix,
    })
}

fn is_number(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// Maske **und** Kalender (Final-Review, Kleinigkeit; Nachkontrolle): Monat
/// 1–12, Tag bis zur Länge des Monats samt Schaltjahr (gregorianisch),
/// Stunde 0–23, Minute/Sekunde 0–59, Millisekunde 0–999.
fn is_stamp(stamp: &str) -> bool {
    let masked = stamp.len() == STAMP_LEN
        && stamp.bytes().enumerate().all(|(i, b)| match i {
            4 | 7 | 13 | 16 | 19 => b == b'-',
            10 => b == b'T',
            23 => b == b'Z',
            _ => b.is_ascii_digit(),
        });
    if !masked {
        return false;
    }
    let field = |from: usize, to: usize| stamp[from..to].parse::<u32>().unwrap_or(u32::MAX);
    let (year, month) = (field(0, 4), field(5, 7));
    (1..=12).contains(&month)
        && (1..=days_in_month(year, month)).contains(&field(8, 10))
        && field(11, 13) <= 23
        && field(14, 16) <= 59
        && field(17, 19) <= 59
        && field(20, 23) <= 999
}

/// Tage im Monat (1–12) des gregorianischen Kalenders.
fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        _ => 31,
    }
}

/// Eine Temp-Datei dieses Musters: `<ringname>.<pid>-<zähler>.part` bzw. die
/// ältere Form `<ringname>.part`.
fn is_own_part(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(PART_EXT) else {
        return false;
    };
    if parse_name(stem).is_some() {
        return true;
    }
    let Some((base, unique)) = stem.rsplit_once('.') else {
        return false;
    };
    let Some((pid, counter)) = unique.split_once('-') else {
        return false;
    };
    parse_name(base).is_some() && is_number(pid) && is_number(counter)
}

/// Schreibt 16-kHz-mono-f32 als 32-bit-Float-WAV nach
/// `<dir>/rec_<UTC bis ms>_lauf-<N>.wav` (bei belegtem Namen mit `-2`, `-3` …)
/// und pflegt danach den Ring mit Kapazität `keep`. Rückgabe ist der
/// endgültige Pfad.
pub fn write_recording(
    config: &DebugWavConfig,
    samples: &[f32],
    run: RunId,
    at: SystemTime,
) -> io::Result<PathBuf> {
    write_recording_with(
        &config.dir,
        config.keep,
        config.legacy_cleanup,
        samples,
        run,
        at,
        rename_no_replace,
    )
}

/// Wie [`write_recording`], mit austauschbarem Finalisieren (Tests).
fn write_recording_with<F>(
    dir: &Path,
    keep: usize,
    legacy_cleanup: bool,
    samples: &[f32],
    run: RunId,
    at: SystemTime,
    finalize: F,
) -> io::Result<PathBuf>
where
    F: Fn(&Path, &Path) -> io::Result<()>,
{
    create_private_dir(dir)?;
    let base = file_name(at, run);
    let (temp, file) = create_part(dir, &base)?;

    if let Err(err) = write_wav(file, samples) {
        // Nur die eigene, gerade exklusiv angelegte Temp-Datei.
        let _ = fs::remove_file(&temp);
        return Err(err);
    }
    let final_path = match finalize_unique(dir, &base, &temp, finalize) {
        Ok(path) => path,
        Err(err) => {
            let _ = fs::remove_file(&temp);
            return Err(err);
        }
    };

    // Erst jetzt zählt die Datei. Aufräumen ist best effort: ein Rest, der
    // sich nicht löschen lässt, verhindert den Dump nicht.
    prune(dir, keep, SystemTime::now());
    if legacy_cleanup {
        let _ = fs::remove_file(dir.join(LEGACY_NAME));
    }
    Ok(final_path)
}

/// Exklusiv (`create_new`) angelegte Temp-Datei mit eindeutigem Namen — eine
/// vorhandene fremde oder alte `.part` wird weder trunkiert noch gelöscht.
fn create_part(dir: &Path, base: &str) -> io::Result<(PathBuf, fs::File)> {
    let pid = std::process::id();
    for _ in 0..MAX_ATTEMPTS {
        let counter = PART_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp = dir.join(format!("{base}.{pid}-{counter}{PART_EXT}"));
        match create_private_file(&temp) {
            Ok(file) => return Ok((temp, file)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "keine freie Temp-Datei für den Debug-WAV",
    ))
}

/// Rename ohne Ersetzen; ist der Name belegt, der nächste Suffix.
fn finalize_unique<F>(dir: &Path, base: &str, temp: &Path, finalize: F) -> io::Result<PathBuf>
where
    F: Fn(&Path, &Path) -> io::Result<()>,
{
    for suffix in 1..=MAX_ATTEMPTS {
        let candidate = dir.join(suffixed_name(base, suffix));
        match finalize(temp, &candidate) {
            Ok(()) => return Ok(candidate),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "kein freier Name für den Debug-WAV",
    ))
}

/// `MoveFileExW` **ohne** `MOVEFILE_REPLACE_EXISTING`: belegt das Ziel schon
/// eine Datei oder ein Verzeichnis, scheitert es mit `ERROR_ALREADY_EXISTS`
/// (→ `AlreadyExists`) statt zu ersetzen. `std::fs::rename` ersetzt unter
/// Windows.
#[cfg(windows)]
fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};

    let wide = |path: &Path| -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };
    let (from, to) = (wide(from), wide(to));
    // SAFETY: Beide Puffer sind NUL-terminiert und leben über den Aufruf;
    // die API liest sie nur.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Außerhalb von Windows: `hard_link` scheitert bei belegtem Ziel, danach die
/// Temp-Datei entfernen.
#[cfg(not(windows))]
fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    fs::hard_link(from, to)?;
    let _ = fs::remove_file(from);
    Ok(())
}

/// 16 kHz mono 32-bit-Float (Spec §10, v1.9): die Samples unverändert, ohne
/// Clamp und Rundung, damit ein Fall bitgenau nachstellbar ist
/// (docs/SPIKES.md 2026-09-30: die 16-bit-Rundung verdeckte Lauf 545).
fn write_wav(file: fs::File, samples: &[f32]) -> io::Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: ENGINE_RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::new(io::BufWriter::new(file), spec).map_err(hound_io)?;
    for &sample in samples {
        writer.write_sample(sample).map_err(hound_io)?;
    }
    writer.finalize().map_err(hound_io)
}

/// Ring: die ältesten Dateien des genauen Musters über `keep` löschen,
/// `.part`-Reste desselben Musters ab [`STALE_PART_AGE`] entfernen. „Älteste"
/// nach Zeitstempel im Namen, bei Gleichstand nach Laufnummer und Suffix —
/// nicht nach Änderungszeit, die ein Kopieren verfälscht.
fn prune(dir: &Path, keep: usize, now: SystemTime) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut dumps: Vec<(String, u64, u32, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        if let Some(ring) = parse_name(&name) {
            dumps.push((ring.stamp.to_owned(), ring.run, ring.suffix, entry.path()));
        } else if is_own_part(&name) {
            let stale = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age >= STALE_PART_AGE);
            if stale {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    if dumps.len() <= keep {
        return;
    }
    dumps.sort_by(|a, b| (&a.0, a.1, a.2).cmp(&(&b.0, b.1, b.2)));
    let excess = dumps.len() - keep;
    for (_, _, _, path) in dumps.into_iter().take(excess) {
        let _ = fs::remove_file(path);
    }
}

fn hound_io(err: hound::Error) -> io::Error {
    match err {
        hound::Error::IoError(io) => io,
        other => io::Error::other(other.to_string()),
    }
}

/// Das Default-Verzeichnis liegt unter `%TEMP%` im Benutzerprofil und erbt
/// damit dessen ACL; ein eigenes `DIKTIER_DEBUG_WAV_DIR` erbt die ACL seines
/// Elternverzeichnisses.
fn create_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)
}

/// Exklusiv: eine vorhandene Datei gleichen Namens bleibt unberührt
/// (`AlreadyExists`).
fn create_private_file(path: &Path) -> io::Result<fs::File> {
    fs::File::options().write(true).create_new(true).open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::read_wav_16k_mono;

    /// Der Ring im Default-Verzeichnis (mit Altlast-Bereinigung), wie ihn die
    /// bisherigen Tests voraussetzen. Verdeckt das `write_recording` des
    /// Moduls, das eine [`DebugWavConfig`] nimmt.
    fn write_recording(
        dir: &Path,
        keep: usize,
        samples: &[f32],
        run: RunId,
        at: SystemTime,
    ) -> io::Result<PathBuf> {
        let config = DebugWavConfig {
            dir: dir.to_path_buf(),
            keep,
            legacy_cleanup: true,
        };
        super::write_recording(&config, samples, run, at)
    }

    /// 2026-09-25T14:47:13.512Z
    fn sample_time() -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(1_790_347_633_512)
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    fn ring_names(dir: &Path) -> Vec<String> {
        names(dir)
            .into_iter()
            .filter(|n| parse_name(n).is_some() && dir.join(n).is_file())
            .collect()
    }

    /// Legt eine Ring-Datei `minute` Minuten nach der Beispielzeit an.
    fn place(dir: &Path, minute: u64, run: u64) -> String {
        let name = file_name(sample_time() + Duration::from_secs(60 * minute), RunId(run));
        fs::write(dir.join(&name), b"alt").unwrap();
        name
    }

    fn set_age(path: &Path, age: Duration) {
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() - age).unwrap();
    }

    #[test]
    fn the_file_name_carries_utc_to_the_millisecond_and_the_run() {
        assert_eq!(
            file_name(sample_time(), RunId(703)),
            "rec_2026-09-25T14-47-13-512Z_lauf-703.wav"
        );
        assert_eq!(
            file_name(UNIX_EPOCH + Duration::from_millis(7), RunId(0)),
            "rec_1970-01-01T00-00-00-007Z_lauf-0.wav"
        );
        // Schaltjahr, Jahreswechsel
        assert_eq!(
            file_name(
                UNIX_EPOCH + Duration::from_millis(951_868_799_999),
                RunId(1)
            ),
            "rec_2000-02-29T23-59-59-999Z_lauf-1.wav"
        );
    }

    #[test]
    fn only_the_exact_pattern_belongs_to_the_ring() {
        assert_eq!(
            parse_name("rec_2026-09-25T14-47-13-512Z_lauf-703.wav"),
            Some(RingName {
                stamp: "2026-09-25T14-47-13-512Z",
                run: 703,
                suffix: 1
            })
        );
        // Kollisionssuffix ab 2.
        assert_eq!(
            parse_name("rec_2026-09-25T14-47-13-512Z_lauf-703-2.wav").map(|r| (r.run, r.suffix)),
            Some((703, 2))
        );
        assert_eq!(
            parse_name("rec_2026-09-25T14-47-13-512Z_lauf-703-17.wav").map(|r| r.suffix),
            Some(17)
        );
        for foreign in [
            "rec_2026-09-25T14-47-13-512Z_lauf-703-1.wav",
            "rec_2026-09-25T14-47-13-512Z_lauf-703-0.wav",
            "rec_2026-09-25T14-47-13-512Z_lauf-703-02.wav",
            "rec_2026-09-25T14-47-13-512Z_lauf-703-.wav",
            "rec_2026-09-25T14-47-13-512Z_lauf-703-2-3.wav",
            "rec_2026-09-25T14-47-13-512Z_lauf-703-x.wav",
            // Kalendergrenzen (Final-Review, Kleinigkeit).
            "rec_2026-00-25T14-47-13-512Z_lauf-1.wav",
            "rec_2026-13-25T14-47-13-512Z_lauf-1.wav",
            "rec_2026-99-25T14-47-13-512Z_lauf-1.wav",
            "rec_2026-09-00T14-47-13-512Z_lauf-1.wav",
            "rec_2026-09-32T14-47-13-512Z_lauf-1.wav",
            "rec_2026-09-25T24-47-13-512Z_lauf-1.wav",
            "rec_2026-09-25T14-60-13-512Z_lauf-1.wav",
            "rec_2026-09-25T14-47-60-512Z_lauf-1.wav",
        ] {
            assert_eq!(parse_name(foreign), None, "{foreign}");
        }
        // Die Grenzen selbst gehören dazu.
        for edge in [
            "rec_2026-01-01T00-00-00-000Z_lauf-1.wav",
            "rec_2026-12-31T23-59-59-999Z_lauf-1.wav",
        ] {
            assert!(parse_name(edge).is_some(), "{edge}");
        }
    }

    /// Nachkontrolle: Monatslängen und Schaltjahr — eine fremde WAV mit
    /// unmöglichem Datum gehört nicht zum Ring (und wird nie gelöscht).
    #[test]
    fn impossible_calendar_dates_are_foreign() {
        let name = |date: &str| format!("rec_{date}T12-00-00-000Z_lauf-1.wav");
        for valid in [
            "2028-02-29", // Schaltjahr
            "2000-02-29", // durch 400 teilbar
            "2026-02-28",
            "2026-04-30",
            "2026-01-31",
            "2026-12-31",
        ] {
            assert!(parse_name(&name(valid)).is_some(), "{valid}");
        }
        for invalid in [
            "2026-02-29", // kein Schaltjahr
            "1900-02-29", // durch 100, nicht durch 400
            "2028-02-30",
            "2026-02-31",
            "2026-04-31",
            "2026-06-31",
            "2026-09-31",
            "2026-11-31",
        ] {
            assert_eq!(parse_name(&name(invalid)), None, "{invalid}");
        }
        // Der eigene Stempel erzeugt nie ein unmögliches Datum.
        let leap_day = UNIX_EPOCH + Duration::from_millis(1_835_438_400_000); // 2028-02-29T12:00Z
        assert!(parse_name(&file_name(leap_day, RunId(1))).is_some());
        assert!(file_name(leap_day, RunId(1)).starts_with("rec_2028-02-29T12-00-00"));
        for foreign in [
            "last_recording.wav",
            "rec_2026-09-25T14-47-13-512Z_lauf-703.wav.part",
            "rec_2026-09-25T14-47-13-512Z_lauf-703.WAV",
            "REC_2026-09-25T14-47-13-512Z_lauf-703.wav",
            "rec_2026-09-25T14-47-13-512Z_lauf-.wav",
            "rec_2026-09-25T14-47-13-512Z_lauf-7x.wav",
            "rec_2026-09-25T14-47-13-51Z_lauf-7.wav",
            "rec_2026-09-25T14:47:13.512Z_lauf-7.wav",
            "rec_2026-09-25T14-47-13-512Z_run-7.wav",
            "rec_eigene-notiz_lauf-7.wav",
            "xrec_2026-09-25T14-47-13-512Z_lauf-7.wav",
        ] {
            assert_eq!(parse_name(foreign), None, "{foreign}");
        }
    }

    #[test]
    fn own_part_names_follow_the_ring_pattern() {
        let base = "rec_2026-09-25T14-47-13-512Z_lauf-703.wav";
        for own in [
            format!("{base}.part"),
            format!("{base}.4711-0.part"),
            format!("{base}.4711-12.part"),
        ] {
            assert!(is_own_part(&own), "{own}");
        }
        for foreign in [
            format!("{base}.4711.part"),
            format!("{base}.4711-x.part"),
            format!("{base}.-1.part"),
            format!("{base}.4711-1.tmp"),
            "rec_eigene-notiz_lauf-7.wav.4711-0.part".to_string(),
            "notiz.part".to_string(),
        ] {
            assert!(!is_own_part(&foreign), "{foreign}");
        }
    }

    #[test]
    fn writes_the_wav_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("diktier-test");
        let samples: Vec<f32> = (0..16_000).map(|i| (i as f32 / 16_000.0) - 0.5).collect();
        let path =
            write_recording(&target, DEFAULT_KEEP, &samples, RunId(703), sample_time()).unwrap();
        assert_eq!(
            path,
            target.join("rec_2026-09-25T14-47-13-512Z_lauf-703.wav")
        );
        assert_eq!(
            names(&target),
            vec!["rec_2026-09-25T14-47-13-512Z_lauf-703.wav"],
            "kein .part-Rest"
        );

        let read_back = read_wav_16k_mono(&path).unwrap();
        assert_eq!(read_back.len(), samples.len());
        assert_eq!(read_back[0].to_bits(), samples[0].to_bits());
    }

    /// Spec §10 (v1.9): 32-bit-Float, Roundtrip bitgleich — auch Werte, die
    /// 16 bit auf 0 oder einen Nachbarwert gerundet hätte, und Werte über 1,
    /// die früher abgeschnitten wurden.
    #[test]
    fn the_dump_is_float32_and_bit_exact() {
        let dir = tempfile::tempdir().unwrap();
        let samples = [
            0.0,
            -0.0,
            1e-6,
            -1e-6,
            f32::MIN_POSITIVE,
            -0.5,
            0.5,
            0.99999,
            -0.99999,
            1.0,
            -1.0,
            1.5,
            -2.0,
            0.123_456_79,
        ];
        let path =
            write_recording(dir.path(), DEFAULT_KEEP, &samples, RunId(1), sample_time()).unwrap();

        let spec = hound::WavReader::open(&path).unwrap().spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, ENGINE_RATE);
        assert_eq!(spec.bits_per_sample, 32);
        assert_eq!(spec.sample_format, hound::SampleFormat::Float);

        let read_back = read_wav_16k_mono(&path).unwrap();
        assert_eq!(read_back.len(), samples.len());
        for (got, want) in read_back.iter().zip(&samples) {
            assert_eq!(got.to_bits(), want.to_bits(), "{got} ≠ {want}");
        }
    }

    /// G5 im Kleinen: zwölf Dumps → genau zehn, die zwei ältesten sind weg.
    #[test]
    fn twelve_dumps_leave_the_ten_newest() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path();
        let mut written = Vec::new();
        for run in 1..=12u64 {
            let at = sample_time() + Duration::from_millis(run);
            written
                .push(write_recording(target, DEFAULT_KEEP, &[0.0; 160], RunId(run), at).unwrap());
        }
        let expected: Vec<String> = written[2..]
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(ring_names(target), expected);
        assert_eq!(names(target).len(), DEFAULT_KEEP);
    }

    /// Älteste zuerst nach Namen (Zeit, dann Laufnummer numerisch) — nicht
    /// nach Änderungszeit und nicht lexikografisch über die Laufnummer.
    #[test]
    fn the_oldest_go_first_by_name_not_by_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path();
        // Zehn alte Dateien: Minute 0 mit Lauf 9 und 10 (gleiche ms), dann 1..=8.
        let same_ms_9 = file_name(sample_time(), RunId(9));
        let same_ms_10 = file_name(sample_time(), RunId(10));
        fs::write(target.join(&same_ms_9), b"alt").unwrap();
        fs::write(target.join(&same_ms_10), b"alt").unwrap();
        let mut newest_old = String::new();
        for minute in 1..=8 {
            newest_old = place(target, minute, 100 + minute);
        }
        // Die älteste Datei ist frisch geändert, die jüngste alte uralt.
        set_age(&target.join(&same_ms_9), Duration::ZERO);
        set_age(&target.join(&newest_old), Duration::from_secs(86_400));

        let new = write_recording(
            target,
            DEFAULT_KEEP,
            &[0.0; 160],
            RunId(200),
            sample_time() + Duration::from_secs(3600),
        )
        .unwrap();
        let ring = ring_names(target);
        assert_eq!(ring.len(), DEFAULT_KEEP);
        assert!(!ring.contains(&same_ms_9), "Lauf 9 vor Lauf 10: {ring:?}");
        assert!(ring.contains(&same_ms_10), "{ring:?}");
        assert!(ring.contains(&newest_old), "{ring:?}");
        assert!(ring.contains(&new.file_name().unwrap().to_string_lossy().into_owned()));
    }

    #[test]
    fn foreign_files_are_neither_counted_nor_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path();
        let foreign = [
            "notiz.txt",
            "rec_eigene-notiz_lauf-7.wav",
            "REC_2026-09-25T14-47-13-512Z_lauf-1.wav",
            "rec_2026-09-25T14-47-13-512Z_lauf-1.wav.bak",
            "aufnahme.wav",
        ];
        for name in foreign {
            fs::write(target.join(name), b"fremd").unwrap();
            set_age(&target.join(name), Duration::from_secs(86_400));
        }
        fs::create_dir(target.join("rec_2020-01-01T00-00-00-000Z_lauf-1.wav")).unwrap();
        for minute in 0..9 {
            place(target, minute, minute);
        }
        write_recording(
            target,
            DEFAULT_KEEP,
            &[0.0; 160],
            RunId(99),
            sample_time() + Duration::from_secs(3600),
        )
        .unwrap();
        // Neun alte + eine neue = zehn: nichts gelöscht, fremde bleiben.
        assert_eq!(ring_names(target).len(), DEFAULT_KEEP);
        for name in foreign {
            assert!(target.join(name).exists(), "{name} gelöscht");
        }
        assert!(
            target
                .join("rec_2020-01-01T00-00-00-000Z_lauf-1.wav")
                .is_dir()
        );
    }

    #[test]
    fn stale_part_leftovers_are_removed_fresh_and_foreign_ones_stay() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path();
        let stale = format!("{}.part", file_name(sample_time(), RunId(1)));
        let stale_unique = format!("{}.4711-3.part", file_name(sample_time(), RunId(4)));
        let fresh = format!("{}.part", file_name(sample_time(), RunId(2)));
        let foreign = "rec_eigene-notiz_lauf-7.wav.part";
        for name in [
            stale.as_str(),
            stale_unique.as_str(),
            fresh.as_str(),
            foreign,
        ] {
            fs::write(target.join(name), b"halb").unwrap();
        }
        for name in [&stale, &stale_unique] {
            set_age(&target.join(name), STALE_PART_AGE + Duration::from_secs(60));
        }
        set_age(
            &target.join(foreign),
            STALE_PART_AGE + Duration::from_secs(60),
        );
        set_age(&target.join(&fresh), Duration::from_secs(60));

        write_recording(target, DEFAULT_KEEP, &[0.0; 160], RunId(3), sample_time()).unwrap();
        assert!(
            !target.join(&stale).exists(),
            "alter .part-Rest bleibt liegen"
        );
        assert!(
            !target.join(&stale_unique).exists(),
            "alter eindeutiger .part-Rest bleibt liegen"
        );
        assert!(target.join(&fresh).exists(), "junger .part-Rest gelöscht");
        assert!(target.join(foreign).exists(), "fremder .part-Rest gelöscht");
        // `.part`-Reste zählen nicht zum Ring.
        assert_eq!(ring_names(target).len(), 1);
    }

    #[test]
    fn the_legacy_dump_is_removed_after_a_successful_dump() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path();
        fs::write(target.join(LEGACY_NAME), b"alt").unwrap();
        write_recording(target, DEFAULT_KEEP, &[0.0; 160], RunId(1), sample_time()).unwrap();
        assert!(!target.join(LEGACY_NAME).exists());
        // Ohne Altlast ist ein weiterer Dump kein Fehler.
        write_recording(
            target,
            DEFAULT_KEEP,
            &[0.0; 160],
            RunId(2),
            sample_time() + Duration::from_millis(1),
        )
        .unwrap();
        assert_eq!(ring_names(target).len(), 2);
    }

    /// W1 (Code-Review WP2): In einem über `DIKTIER_DEBUG_WAV_DIR` gewählten
    /// Verzeichnis ist `last_recording.wav` fremd und bleibt, ebenso fremde
    /// `.part`-Dateien jeden Alters; Kapazität 5000 wie im Alltagstest.
    #[test]
    fn a_chosen_dir_keeps_a_foreign_last_recording() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("ultra-test").join("wav");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join(LEGACY_NAME), b"fremd").unwrap();
        let foreign_parts = ["aufnahme.wav.part", "rec_notiz.part", "x.12-3.part"];
        for (i, name) in foreign_parts.iter().enumerate() {
            fs::write(target.join(name), b"fremd").unwrap();
            if i > 0 {
                set_age(&target.join(name), STALE_PART_AGE + Duration::from_secs(60));
            }
        }
        let resolved = resolve(
            Some(os("1")),
            Some(os("5000")),
            Some(target.as_os_str()),
            fallback(),
        );
        let config = resolved.config.unwrap();
        assert!(!config.legacy_cleanup);
        super::write_recording(&config, &[0.0; 160], RunId(1), sample_time()).unwrap();
        assert!(
            target.join(LEGACY_NAME).exists(),
            "fremde last_recording.wav gelöscht"
        );
        for name in foreign_parts {
            assert!(target.join(name).exists(), "{name} gelöscht");
        }
        assert_eq!(ring_names(&target).len(), 1);

        // Gegenprobe: dasselbe Verzeichnis als Default (Variable nicht oder
        // ungültig gesetzt) räumt die eigene Altlast weg.
        for dir_var in [None, Some(os("relativ"))] {
            let resolved = resolve(Some(os("1")), None, dir_var, target.clone());
            let config = resolved.config.unwrap();
            assert!(config.legacy_cleanup, "{dir_var:?}");
        }
        let default = DebugWavConfig {
            dir: target.clone(),
            keep: KEEP_MAX,
            legacy_cleanup: true,
        };
        super::write_recording(
            &default,
            &[0.0; 160],
            RunId(2),
            sample_time() + Duration::from_millis(1),
        )
        .unwrap();
        assert!(!target.join(LEGACY_NAME).exists());
        for name in foreign_parts {
            assert!(target.join(name).exists(), "{name} gelöscht");
        }
    }

    /// Ein gescheiterter Schreibvorgang zählt nicht: kein Ring-Eintrag, kein
    /// Löschen im Ring, keine Altlast entfernt, kein eigener `.part`-Rest.
    #[test]
    fn a_failed_write_counts_nothing_and_deletes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path();
        let mut before = Vec::new();
        for minute in 0..DEFAULT_KEEP as u64 {
            before.push(place(target, minute, minute));
        }
        fs::write(target.join(LEGACY_NAME), b"alt").unwrap();
        let stale = format!("{}.part", file_name(sample_time(), RunId(77)));
        fs::write(target.join(&stale), b"halb").unwrap();
        set_age(
            &target.join(&stale),
            STALE_PART_AGE + Duration::from_secs(60),
        );

        // Das Finalisieren scheitert mit einem echten Fehler (kein
        // „belegt“, sonst gäbe es einen Suffix).
        let at = sample_time() + Duration::from_secs(3600);
        let failing =
            |_: &Path, _: &Path| -> io::Result<()> { Err(io::Error::other("Rename verweigert")) };
        assert!(
            write_recording_with(
                target,
                DEFAULT_KEEP,
                true,
                &[0.0; 160],
                RunId(500),
                at,
                failing
            )
            .is_err()
        );
        assert_eq!(ring_names(target), {
            let mut b = before.clone();
            b.sort();
            b
        });
        assert!(
            target.join(LEGACY_NAME).exists(),
            "Altlast trotz Fehler gelöscht"
        );
        assert!(target.join(&stale).exists(), "Aufräumen trotz Fehler");
        let blocked = file_name(at, RunId(500));
        assert!(
            !names(target).iter().any(|n| n.starts_with(&blocked)),
            "eigener .part-Rest bleibt liegen: {:?}",
            names(target)
        );
    }

    /// Blocker 3: Ein belegter Zielname wird nie ersetzt — die neue Datei
    /// bekommt `-2`. Auch ein Verzeichnis dieses Namens gilt als belegt.
    #[test]
    fn an_existing_final_name_is_never_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path();
        let name = file_name(sample_time(), RunId(703));
        fs::write(target.join(&name), b"vorhanden").unwrap();

        let path =
            write_recording(target, DEFAULT_KEEP, &[0.0; 160], RunId(703), sample_time()).unwrap();
        assert_eq!(
            path,
            target.join("rec_2026-09-25T14-47-13-512Z_lauf-703-2.wav")
        );
        assert_eq!(fs::read(target.join(&name)).unwrap(), b"vorhanden");
        assert!(read_wav_16k_mono(&path).is_ok());

        let blocked = file_name(sample_time(), RunId(9));
        fs::create_dir(target.join(&blocked)).unwrap();
        let path =
            write_recording(target, DEFAULT_KEEP, &[0.0; 160], RunId(9), sample_time()).unwrap();
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            "rec_2026-09-25T14-47-13-512Z_lauf-9-2.wav"
        );
        assert!(target.join(&blocked).is_dir());
    }

    /// Blocker 3: Vorhandene `.part`-Dateien desselben Ziels — alte Form und
    /// eindeutige Form mit den nächsten Zählerständen — bleiben unberührt.
    #[test]
    fn existing_part_files_are_neither_truncated_nor_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path();
        let base = file_name(sample_time(), RunId(42));
        let pid = std::process::id();
        let next = PART_COUNTER.load(Ordering::Relaxed);
        let mut parts = vec![format!("{base}.part")];
        parts.extend((next..next + 8).map(|n| format!("{base}.{pid}-{n}.part")));
        for part in &parts {
            fs::write(target.join(part), b"fremd").unwrap();
        }

        let path =
            write_recording(target, DEFAULT_KEEP, &[0.0; 160], RunId(42), sample_time()).unwrap();
        assert_eq!(path, target.join(&base));
        for part in &parts {
            assert_eq!(fs::read(target.join(part)).unwrap(), b"fremd", "{part}");
        }
        // Außer den vorhandenen kein weiterer Rest.
        let leftovers = names(target)
            .into_iter()
            .filter(|n| n.ends_with(PART_EXT))
            .count();
        assert_eq!(leftovers, parts.len());
    }

    /// Blocker 3: zweimal `write_recording` mit identischem `(at, run)` →
    /// zwei Dateien, die erste bleibt byte-gleich.
    #[test]
    fn the_same_run_and_time_twice_gives_two_files() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path();
        let first =
            write_recording(target, DEFAULT_KEEP, &[0.25; 160], RunId(5), sample_time()).unwrap();
        let first_bytes = fs::read(&first).unwrap();
        let second =
            write_recording(target, DEFAULT_KEEP, &[-0.5; 320], RunId(5), sample_time()).unwrap();
        assert_ne!(first, second);
        assert_eq!(
            fs::read(&first).unwrap(),
            first_bytes,
            "erste überschrieben"
        );
        assert_eq!(read_wav_16k_mono(&second).unwrap().len(), 320);
        assert_eq!(
            ring_names(target),
            vec![
                "rec_2026-09-25T14-47-13-512Z_lauf-5-2.wav".to_string(),
                "rec_2026-09-25T14-47-13-512Z_lauf-5.wav".to_string(),
            ]
        );
    }

    /// Der Suffix ordnet sich im Ring hinter den Grundnamen.
    #[test]
    fn suffixed_dumps_are_younger_than_their_base() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path();
        // Zehn Dateien, die älteste ist der Grundname von Lauf 1.
        let base = place(target, 0, 1);
        let suffixed = suffixed_name(&base, 2);
        fs::write(target.join(&suffixed), b"alt").unwrap();
        for minute in 1..=8 {
            place(target, minute, 100 + minute);
        }
        write_recording(
            target,
            DEFAULT_KEEP,
            &[0.0; 160],
            RunId(200),
            sample_time() + Duration::from_secs(3600),
        )
        .unwrap();
        let ring = ring_names(target);
        assert_eq!(ring.len(), DEFAULT_KEEP);
        assert!(!ring.contains(&base), "{ring:?}");
        assert!(ring.contains(&suffixed), "{ring:?}");
    }

    #[test]
    fn a_missing_directory_is_created() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a").join("b");
        let path =
            write_recording(&target, DEFAULT_KEEP, &[0.0; 160], RunId(1), sample_time()).unwrap();
        assert!(path.is_file());
    }

    // ------------------------------------------ Einstellungen (Spec §10, v1.10)
    //
    // Alles über `resolve`/`parse_*` mit übergebenen Werten: kein Test liest
    // oder verändert die Prozessumgebung.

    fn os(text: &str) -> &OsStr {
        OsStr::new(text)
    }

    #[cfg(windows)]
    const ABSOLUTE: &str = r"C:\Users\test\AppData\Local\diktier\ultra-test\wav";
    #[cfg(not(windows))]
    const ABSOLUTE: &str = "/home/test/diktier/wav";

    fn fallback() -> PathBuf {
        PathBuf::from(ABSOLUTE).join("fallback")
    }

    #[test]
    fn keep_accepts_exactly_one_to_five_thousand() {
        assert_eq!(parse_keep(os("1")), Ok(1));
        assert_eq!(parse_keep(os("10")), Ok(10));
        assert_eq!(parse_keep(os("5000")), Ok(5000));
        assert_eq!(parse_keep(os("0010")), Ok(10));
        for out_of_range in ["0", "5001", "000", "99999999999999999999999"] {
            assert_eq!(
                parse_keep(os(out_of_range)),
                Err("liegt außerhalb von 1–5000".to_owned()),
                "{out_of_range}"
            );
        }
        for not_a_number in [
            "", " ", "zehn", "10 ", " 10", "+10", "-1", "1.5", "1e3", "10\t", "１０",
        ] {
            assert_eq!(
                parse_keep(os(not_a_number)),
                Err("ist keine ganze Zahl (1–5000)".to_owned()),
                "{not_a_number:?}"
            );
        }
    }

    #[test]
    fn dir_must_be_non_empty_and_absolute() {
        assert_eq!(parse_dir(os(ABSOLUTE)), Ok(PathBuf::from(ABSOLUTE)));
        #[cfg(windows)]
        assert_eq!(
            parse_dir(os(r"\\server\freigabe\wav")),
            Ok(PathBuf::from(r"\\server\freigabe\wav"))
        );
        for empty in ["", " ", "\t \t"] {
            assert_eq!(
                parse_dir(os(empty)),
                Err("ist leer".to_owned()),
                "{empty:?}"
            );
        }
        let mut relative = vec!["wav", r"diktier\wav", r".\wav", r"..\wav", " C:\\wav"];
        // Unter Windows ist `\wav` (Wurzel ohne Laufwerk) und `C:wav`
        // (Laufwerk ohne Wurzel) relativ.
        #[cfg(windows)]
        relative.extend([r"\wav", "C:wav", "%LOCALAPPDATA%\\diktier"]);
        for path in relative {
            assert_eq!(
                parse_dir(os(path)),
                Err("ist kein absoluter Pfad".to_owned()),
                "{path}"
            );
        }
    }

    /// Umgebungsvariablen werden nicht expandiert — `%TEMP%` bleibt Text.
    #[test]
    fn the_default_dir_is_temp_diktier() {
        let temp = PathBuf::from(ABSOLUTE).join("Temp");
        assert_eq!(default_dir(Some(temp.as_os_str())), temp.join("diktier"));
        assert_eq!(default_dir(None), std::env::temp_dir().join("diktier"));
    }

    #[test]
    fn switch_off_means_no_dump_and_no_checks() {
        for switch in [
            None,
            Some("0"),
            Some(""),
            Some("true"),
            Some(" 1"),
            Some("1 "),
        ] {
            let resolved = resolve(switch.map(os), None, None, fallback());
            assert_eq!(resolved.config, None, "{switch:?}");
            assert!(resolved.warnings.is_empty());
            assert_eq!(resolved.start_line(), None);
        }
        // Ungültige KEEP/DIR bei ausgeschaltetem Dump: keine Warnung, aber
        // der Hinweis, dass der Schalter fehlt (W1).
        let resolved = resolve(None, Some(os("0")), Some(os("wav")), fallback());
        assert_eq!(resolved.config, None);
        assert!(resolved.warnings.is_empty());
        assert_eq!(
            resolved.start_line().as_deref(),
            Some(
                "Debug-WAV aus: DIKTIER_DEBUG_WAV_KEEP/DIKTIER_DEBUG_WAV_DIR gesetzt, \
                 aber DIKTIER_DEBUG_WAV ist nicht 1"
            )
        );
        assert!(resolve(Some(os("0")), None, Some(os(ABSOLUTE)), fallback()).ignored_settings);
    }

    #[test]
    fn switch_on_without_settings_uses_the_defaults() {
        let resolved = resolve(Some(os("1")), None, None, fallback());
        assert_eq!(
            resolved.config,
            Some(DebugWavConfig {
                dir: fallback(),
                keep: DEFAULT_KEEP,
                legacy_cleanup: true,
            })
        );
        assert!(resolved.warnings.is_empty());
        assert_eq!(
            resolved.start_line(),
            Some(format!(
                "Debug-WAV an: {}, behalte 10",
                fallback().display()
            ))
        );
    }

    /// Der Testfall aus Leitentscheidung 5: 5000 in einem eigenen Verzeichnis.
    #[test]
    fn valid_settings_are_taken_over() {
        let resolved = resolve(
            Some(os("1")),
            Some(os("5000")),
            Some(os(ABSOLUTE)),
            fallback(),
        );
        assert_eq!(
            resolved.config,
            Some(DebugWavConfig {
                dir: PathBuf::from(ABSOLUTE),
                keep: 5000,
                legacy_cleanup: false,
            })
        );
        assert!(resolved.warnings.is_empty());
        assert_eq!(
            resolved.start_line(),
            Some(format!("Debug-WAV an: {ABSOLUTE}, behalte 5000"))
        );
    }

    /// Je ungültigem Wert genau eine Warnzeile, und nur der betroffene Wert
    /// fällt auf den Default.
    #[test]
    fn each_invalid_value_warns_once_and_falls_back() {
        let resolved = resolve(
            Some(os("1")),
            Some(os("5001")),
            Some(os(ABSOLUTE)),
            fallback(),
        );
        assert_eq!(resolved.config.as_ref().unwrap().keep, DEFAULT_KEEP);
        assert_eq!(
            resolved.config.as_ref().unwrap().dir,
            PathBuf::from(ABSOLUTE)
        );
        assert_eq!(
            resolved.warnings,
            vec!["DIKTIER_DEBUG_WAV_KEEP liegt außerhalb von 1–5000 — nehme den Default 10"]
        );

        let resolved = resolve(Some(os("1")), Some(os("7")), Some(os("wav")), fallback());
        assert_eq!(
            resolved.config,
            Some(DebugWavConfig {
                dir: fallback(),
                keep: 7,
                legacy_cleanup: true,
            })
        );
        assert_eq!(
            resolved.warnings,
            vec![format!(
                "DIKTIER_DEBUG_WAV_DIR ist kein absoluter Pfad — nehme den Default {}",
                fallback().display()
            )]
        );

        let resolved = resolve(Some(os("1")), Some(os("viele")), Some(os(" ")), fallback());
        assert_eq!(
            resolved.config,
            Some(DebugWavConfig {
                dir: fallback(),
                keep: DEFAULT_KEEP,
                legacy_cleanup: true,
            })
        );
        assert_eq!(resolved.warnings.len(), 2, "{:?}", resolved.warnings);
        assert!(resolved.warnings[0].starts_with("DIKTIER_DEBUG_WAV_KEEP ist keine ganze Zahl"));
        assert!(resolved.warnings[1].starts_with("DIKTIER_DEBUG_WAV_DIR ist leer"));
    }

    /// Ring mit kleiner Kapazität im eigenen Verzeichnis: drei bleiben, die
    /// ältesten gehen, fremde Dateien und ein junger `.part`-Rest bleiben.
    #[test]
    fn a_small_ring_keeps_its_capacity_and_leaves_foreign_files() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("ultra-test").join("wav");
        fs::create_dir_all(&target).unwrap();
        let foreign = ["notiz.txt", "aufnahme.wav", "rec_eigene-notiz_lauf-7.wav"];
        for name in foreign {
            fs::write(target.join(name), b"fremd").unwrap();
        }
        let fresh_part = format!("{}.part", file_name(sample_time(), RunId(0)));
        fs::write(target.join(&fresh_part), b"halb").unwrap();

        let mut written = Vec::new();
        for run in 1..=5u64 {
            let at = sample_time() + Duration::from_secs(run);
            written.push(write_recording(&target, 3, &[0.0; 160], RunId(run), at).unwrap());
        }
        let expected: Vec<String> = written[2..]
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(ring_names(&target), expected);
        for name in foreign {
            assert!(target.join(name).exists(), "{name} gelöscht");
        }
        assert!(target.join(&fresh_part).exists());
    }

    /// Kapazität 1 und eine Kapazität über dem Bestand: nichts Falsches weg.
    #[test]
    fn capacity_one_and_large_capacity() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path();
        for run in 1..=3u64 {
            let at = sample_time() + Duration::from_secs(run);
            write_recording(target, 1, &[0.0; 160], RunId(run), at).unwrap();
        }
        assert_eq!(
            ring_names(target),
            vec![file_name(sample_time() + Duration::from_secs(3), RunId(3))]
        );

        // 5000: vorhandene Dateien bleiben alle, auch über den alten zehn.
        for minute in 1..=12 {
            place(target, minute, 100 + minute);
        }
        write_recording(
            target,
            KEEP_MAX,
            &[0.0; 160],
            RunId(200),
            sample_time() + Duration::from_secs(3600),
        )
        .unwrap();
        assert_eq!(ring_names(target).len(), 14);
    }
}
