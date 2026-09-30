//! `--transcribe-list` (Spec §9, v1.10): viele WAVs mit einmal geladenem
//! Modell, eine JSONL-Zeile je Datei (bzw. je Messlauf mit `--runs`).
//!
//! Vertrag:
//!
//! - Liste: UTF-8, eine WAV je Zeile; Leerzeilen, Leerraum an den Rändern und
//!   ein BOM am Anfang zählen nicht.
//! - stdout: `{"file","status","text","infer_ms","samples"}` in dieser
//!   Reihenfolge, `status` = `text` | `rejected` | `error`. Bei `rejected` und
//!   `error` ist `text` leer und `infer_ms` `null`; `samples` ist `null`, wenn
//!   die Datei nicht gelesen werden konnte. Mit `--runs n` genau n Zeilen je
//!   Datei mit zusätzlichem `run` (1…n), auch bei Ablehnung und Fehler — so
//!   bleibt die Zeilenzahl ohne Blick auf den Status vorhersagbar.
//! - Das Modell wird erst geladen, wenn die erste Datei den Gate passiert, und
//!   höchstens einmal; danach ein ungezählter Warmup auf dieser Datei.
//!   Scheitert der Warmup, ist diese Datei `error` (mit ihrer vollen
//!   Zeilenzahl), und die nächste freigegebene Datei wärmt erneut auf.
//! - stderr: Gate-Report, Ladezeit und Fehlerursachen, je mit Dateibezug, nie
//!   ein Transkript.
//! - Exitcode 1, sobald eine Datei `error` hat (auch ein gescheiterter Warmup
//!   oder ein nicht ladbares Modell); die übrigen Dateien laufen trotzdem.
//!   Exit 1 heißt also immer: mindestens eine Zeile `error`.

use std::io::{self, Write};
use std::path::Path;
use std::time::Instant;

use serde::Serialize;

use crate::audio;
use crate::engine::{self, EngineError, Transcriber, transcribe_pcm};

/// Eine nicht lesbare oder nicht verwendbare Liste.
#[derive(Debug)]
pub enum ListError {
    /// Nicht gefunden, keine Rechte, Lesefehler → Exit 1.
    Io(String),
    /// Kein UTF-8 oder keine einzige Datei → Exit 2.
    Format(String),
}

impl ListError {
    pub fn exit_code(&self) -> u8 {
        match self {
            ListError::Io(_) => 1,
            ListError::Format(_) => 2,
        }
    }
}

impl std::fmt::Display for ListError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ListError::Io(msg) | ListError::Format(msg) => f.write_str(msg),
        }
    }
}

/// Liste lesen: eine Datei je nicht-leerer Zeile, in Dateireihenfolge.
pub fn read_list(path: &Path) -> Result<Vec<String>, ListError> {
    let bytes =
        std::fs::read(path).map_err(|e| ListError::Io(format!("Liste {}: {e}", path.display())))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| ListError::Format(format!("Liste {} ist kein UTF-8", path.display())))?;
    let files = parse_list(&text);
    if files.is_empty() {
        return Err(ListError::Format(format!(
            "Liste {} nennt keine Datei",
            path.display()
        )));
    }
    Ok(files)
}

fn parse_list(text: &str) -> Vec<String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Status {
    Text,
    Rejected,
    Error,
}

/// Eine JSONL-Zeile. Die Feldreihenfolge ist die aus Spec §9; das Escaping
/// übernimmt `serde_json`.
#[derive(Serialize)]
struct Line<'a> {
    file: &'a str,
    status: Status,
    text: &'a str,
    infer_ms: Option<f64>,
    samples: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    run: Option<u32>,
}

enum EngineSlot<T> {
    NotLoaded,
    Loaded(T),
    Failed,
}

/// Millisekunden auf eine Mikrosekunde gerundet, damit die Zeile keine
/// Gleitkomma-Schwänze trägt.
fn millis(start: Instant) -> f64 {
    (start.elapsed().as_secs_f64() * 1_000_000.0).round() / 1_000.0
}

/// Der Batch hinter `--transcribe-list`. `load` lädt das Modell (höchstens
/// einmal, nur bei Bedarf); `out` bekommt die JSONL-Zeilen, `diag` die
/// textfreie Diagnose. `runs = None` heißt: eine Zeile je Datei ohne `run`.
pub fn run_batch<T, L, W, E>(
    files: &[String],
    runs: Option<u32>,
    mut load: L,
    out: &mut W,
    diag: &mut E,
) -> u8
where
    T: Transcriber,
    L: FnMut() -> Result<T, EngineError>,
    W: Write,
    E: Write,
{
    let mut code = 0_u8;
    let mut slot = EngineSlot::NotLoaded;
    let mut warmed = false;
    let run_ids: Vec<Option<u32>> = match runs {
        None => vec![None],
        Some(n) => (1..=n).map(Some).collect(),
    };

    for file in files {
        let mut emit = |status: Status, text: &str, infer_ms, samples, run| {
            let line = Line {
                file,
                status,
                text,
                infer_ms,
                samples,
                run,
            };
            write_line(out, &line)
        };

        let pcm = match audio::read_wav_16k_mono(Path::new(file)) {
            Ok(pcm) => pcm,
            Err(err) => {
                let _ = writeln!(diag, "{file}: {err}");
                code = 1;
                for &run in &run_ids {
                    if emit(Status::Error, "", None, None, run).is_err() {
                        return 1;
                    }
                }
                continue;
            }
        };
        let samples = Some(pcm.len());

        // §6.4: der Report je Aufnahme; der Gate ist deterministisch, die
        // Messläufe rechnen ihn in `transcribe_pcm` identisch nach.
        let report = engine::silence_gate(&pcm);
        let _ = writeln!(diag, "{file}: Gate: {report}");
        if report.is_rejected() {
            for &run in &run_ids {
                if emit(Status::Rejected, "", None, samples, run).is_err() {
                    return 1;
                }
            }
            continue;
        }

        if matches!(slot, EngineSlot::NotLoaded) {
            let load_start = Instant::now();
            slot = match load() {
                Ok(engine) => {
                    let _ = writeln!(
                        diag,
                        "Modell geladen in {:.3} s",
                        load_start.elapsed().as_secs_f64()
                    );
                    EngineSlot::Loaded(engine)
                }
                Err(err) => {
                    let _ = writeln!(diag, "Modell nicht geladen: {err}");
                    EngineSlot::Failed
                }
            };
        }

        let engine = match &mut slot {
            EngineSlot::Loaded(engine) => engine,
            EngineSlot::Failed => {
                let _ = writeln!(diag, "{file}: kein Modell");
                code = 1;
                for &run in &run_ids {
                    if emit(Status::Error, "", None, samples, run).is_err() {
                        return 1;
                    }
                }
                continue;
            }
            EngineSlot::NotLoaded => unreachable!("oben geladen oder gescheitert"),
        };

        // Warmup, ungezählt, auf der ersten freigegebenen Datei. Scheitert er,
        // ist diese Datei `error` (alle ihre Zeilen, keine Messung auf kalter
        // Engine), und die nächste freigegebene Datei wärmt erneut auf.
        if !warmed {
            if let (_, Err(err)) = transcribe_pcm(engine, &pcm) {
                let _ = writeln!(diag, "{file}: Warmup: {err}");
                code = 1;
                for &run in &run_ids {
                    if emit(Status::Error, "", None, samples, run).is_err() {
                        return 1;
                    }
                }
                continue;
            }
            warmed = true;
        }

        for &run in &run_ids {
            let start = Instant::now();
            let written = match transcribe_pcm(engine, &pcm).1 {
                Ok(result) => {
                    let ms = millis(start);
                    emit(Status::Text, &result.text, Some(ms), samples, run)
                }
                Err(err) => {
                    let _ = writeln!(diag, "{file}: {err}");
                    code = 1;
                    emit(Status::Error, "", None, samples, run)
                }
            };
            if written.is_err() {
                return 1;
            }
        }
    }
    code
}

/// Eine Zeile schreiben und sofort ausspülen: bricht der Prozess mitten im
/// Batch ab, sind die fertigen Zeilen schon draußen.
fn write_line<W: Write>(out: &mut W, line: &Line<'_>) -> io::Result<()> {
    serde_json::to_writer(&mut *out, line).map_err(io::Error::other)?;
    out.write_all(b"\n")?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Transcription;
    use std::path::PathBuf;

    /// Liefert je Puffer einen festen Text oder einen Fehler. Die Länge des
    /// Puffers (ohne Vorlauf-Stille) wählt die Antwort.
    struct ScriptedStub {
        answers: Vec<(usize, Result<&'static str, &'static str>)>,
    }

    impl Transcriber for ScriptedStub {
        fn transcribe(&mut self, pcm: &[f32]) -> Result<Transcription, EngineError> {
            let len = pcm.len() - engine::LEAD_IN_SILENCE_SAMPLES;
            let answer = self
                .answers
                .iter()
                .find(|(n, _)| *n == len)
                .map(|(_, a)| *a)
                .unwrap_or(Ok("?"));
            match answer {
                Ok(text) => Ok(Transcription {
                    text: text.to_string(),
                    language: None,
                    timing: None,
                }),
                Err(msg) => Err(EngineError::Failed(msg.to_string())),
            }
        }
    }

    fn write_wav(dir: &Path, name: &str, samples: usize, value: i16) -> String {
        let path: PathBuf = dir.join(name);
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for _ in 0..samples {
            writer.write_sample(value).unwrap();
        }
        writer.finalize().unwrap();
        path.to_str().expect("utf-8 path").to_string()
    }

    /// Laut (B1) — die Samplezahl unterscheidet die Dateien für den Stub.
    fn loud(dir: &Path, name: &str, samples: usize) -> String {
        write_wav(dir, name, samples, 2_000)
    }

    /// Digitale Stille — Regel C, nie Engine.
    fn silent(dir: &Path, name: &str) -> String {
        write_wav(dir, name, 16_000 * 2, 0)
    }

    struct Outcome {
        code: u8,
        lines: Vec<serde_json::Value>,
        raw: String,
        diag: String,
        loads: usize,
        calls: usize,
    }

    fn run(
        files: &[String],
        runs: Option<u32>,
        answers: Vec<(usize, Result<&'static str, &'static str>)>,
        load_fails: bool,
    ) -> Outcome {
        let mut loads = 0;
        let mut out = Vec::new();
        let mut diag = Vec::new();
        let calls = std::cell::Cell::new(0);
        let code = run_batch(
            files,
            runs,
            || {
                loads += 1;
                if load_fails {
                    Err(EngineError::Artifacts("fehlt".into()))
                } else {
                    Ok(CallCounter {
                        inner: ScriptedStub {
                            answers: answers.clone(),
                        },
                        calls: &calls,
                    })
                }
            },
            &mut out,
            &mut diag,
        );
        let raw = String::from_utf8(out).unwrap();
        let lines = raw
            .lines()
            .map(|l| serde_json::from_str(l).expect("jede Zeile ist JSON"))
            .collect();
        Outcome {
            code,
            lines,
            raw,
            diag: String::from_utf8(diag).unwrap(),
            loads,
            calls: calls.get(),
        }
    }

    /// Zählt Engine-Aufrufe über das Ende des Batches hinaus.
    struct CallCounter<'a> {
        inner: ScriptedStub,
        calls: &'a std::cell::Cell<usize>,
    }

    impl Transcriber for CallCounter<'_> {
        fn transcribe(&mut self, pcm: &[f32]) -> Result<Transcription, EngineError> {
            self.calls.set(self.calls.get() + 1);
            self.inner.transcribe(pcm)
        }
    }

    fn field<'a>(line: &'a serde_json::Value, key: &str) -> &'a serde_json::Value {
        line.get(key)
            .unwrap_or_else(|| panic!("Feld {key} fehlt: {line}"))
    }

    #[test]
    fn list_ignores_blank_lines_bom_and_crlf() {
        let text = "\u{feff}a.wav\r\n\r\n   \r\n  b c.wav  \n\n\tc.wav\n";
        assert_eq!(parse_list(text), ["a.wav", "b c.wav", "c.wav"]);
    }

    #[test]
    fn list_file_errors_have_exit_codes() {
        let dir = tempfile::tempdir().unwrap();
        let missing = read_list(&dir.path().join("nope.txt")).unwrap_err();
        assert_eq!(missing.exit_code(), 1);

        let empty = dir.path().join("leer.txt");
        std::fs::write(&empty, "\r\n  \n").unwrap();
        assert_eq!(read_list(&empty).unwrap_err().exit_code(), 2);

        let latin1 = dir.path().join("latin1.txt");
        std::fs::write(&latin1, b"gr\xfc\xdfe.wav\n").unwrap();
        assert_eq!(read_list(&latin1).unwrap_err().exit_code(), 2);

        let ok = dir.path().join("ok.txt");
        std::fs::write(&ok, "grüße.wav\n\nzwei.wav\n").unwrap();
        assert_eq!(read_list(&ok).unwrap(), ["grüße.wav", "zwei.wav"]);
    }

    /// Die drei Zustände in einem Lauf: Text, Ablehnung, Lesefehler. Die
    /// Feldreihenfolge ist die aus Spec §9, ohne `run`.
    #[test]
    fn states_text_rejected_error() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![
            loud(dir.path(), "a.wav", 16_000 * 3),
            silent(dir.path(), "still.wav"),
            dir.path().join("fehlt.wav").to_str().unwrap().to_string(),
        ];
        let o = run(&files, None, vec![(16_000 * 3, Ok("Hallo"))], false);
        assert_eq!(o.code, 1, "Lesefehler ist `error`");
        assert_eq!(o.lines.len(), 3);

        let text = &o.lines[0];
        assert_eq!(field(text, "status"), "text");
        assert_eq!(field(text, "text"), "Hallo");
        assert!(field(text, "infer_ms").is_f64(), "{text}");
        assert_eq!(field(text, "samples"), 48_000);
        assert!(text.get("run").is_none());

        let rejected = &o.lines[1];
        assert_eq!(field(rejected, "status"), "rejected");
        assert_eq!(field(rejected, "text"), "");
        assert!(field(rejected, "infer_ms").is_null());
        assert_eq!(field(rejected, "samples"), 32_000);

        let error = &o.lines[2];
        assert_eq!(field(error, "status"), "error");
        assert_eq!(field(error, "text"), "");
        assert!(field(error, "infer_ms").is_null());
        assert!(field(error, "samples").is_null());

        let first = o.raw.lines().next().unwrap();
        let keys = [
            "\"file\"",
            "\"status\"",
            "\"text\"",
            "\"infer_ms\"",
            "\"samples\"",
        ];
        let positions: Vec<usize> = keys.iter().map(|k| first.find(k).unwrap()).collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]), "{first}");

        // Diagnose mit Dateibezug, ohne Transkript.
        assert!(
            o.diag.contains(&format!("{}: Gate: ", files[1])),
            "{}",
            o.diag
        );
        assert!(o.diag.contains(&format!("{}: ", files[2])), "{}", o.diag);
        assert!(!o.diag.contains("Hallo"), "{}", o.diag);
        // Ein Laden, ein Warmup, ein Messlauf.
        assert_eq!((o.loads, o.calls), (1, 2));
    }

    #[test]
    fn jsonl_escapes_quotes_tab_newline_and_keeps_umlauts() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![loud(dir.path(), "Grüße b.wav", 16_000)];
        let tricky = "Er sagte \"Hallo\"\tund\ndann: Grüße, Maß \\ Ende";
        let o = run(&files, None, vec![(16_000, Ok(tricky))], false);
        assert_eq!(o.code, 0);
        assert_eq!(
            o.raw.lines().count(),
            1,
            "ein Zeilenumbruch im Text bricht die Zeile nicht"
        );
        assert!(o.raw.contains(r#"\"Hallo\""#), "{}", o.raw);
        assert!(o.raw.contains(r"\t"), "{}", o.raw);
        assert!(o.raw.contains(r"\n"), "{}", o.raw);
        assert!(o.raw.contains(r"\\ Ende"), "{}", o.raw);
        assert!(
            o.raw.contains("Grüße, Maß"),
            "UTF-8 bleibt lesbar: {}",
            o.raw
        );
        assert_eq!(field(&o.lines[0], "text"), tricky);
        assert_eq!(field(&o.lines[0], "file"), files[0].as_str());
    }

    /// Ein Engine-Fehler in der Mitte: Exit 1, aber jede Datei hat ihre Zeile.
    #[test]
    fn engine_error_mid_batch_exits_1_with_complete_output() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![
            loud(dir.path(), "1.wav", 16_000),
            loud(dir.path(), "2.wav", 16_001),
            loud(dir.path(), "3.wav", 16_002),
        ];
        let answers = vec![
            (16_000, Ok("eins")),
            (16_001, Err("kaputt")),
            (16_002, Ok("drei")),
        ];
        let o = run(&files, None, answers, false);
        assert_eq!(o.code, 1);
        let statuses: Vec<_> = o.lines.iter().map(|l| field(l, "status").clone()).collect();
        assert_eq!(statuses, ["text", "error", "text"]);
        assert_eq!(field(&o.lines[2], "text"), "drei");
        assert!(o.diag.contains(&format!(
            "{}: Transkription fehlgeschlagen: kaputt",
            files[1]
        )));
        assert_eq!(o.loads, 1);
    }

    #[test]
    fn runs_emit_numbered_lines_per_file_after_one_warmup() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![
            loud(dir.path(), "a.wav", 16_000),
            silent(dir.path(), "still.wav"),
            loud(dir.path(), "b.wav", 16_001),
        ];
        let answers = vec![(16_000, Ok("a")), (16_001, Ok("b"))];
        let o = run(&files, Some(3), answers, false);
        assert_eq!(o.code, 0);
        assert_eq!(o.lines.len(), 9);
        for (i, line) in o.lines.iter().enumerate() {
            assert_eq!(field(line, "file"), files[i / 3].as_str());
            assert_eq!(field(line, "run"), (i % 3 + 1) as u64);
        }
        assert!(
            o.lines[3..6]
                .iter()
                .all(|l| field(l, "status") == "rejected")
        );
        // Ein Warmup (nur auf der ersten freigegebenen Datei) plus 2 × 3 Läufe.
        assert_eq!((o.loads, o.calls), (1, 7));
    }

    /// Nur abgelehnte Dateien: das Modell wird nie geladen.
    #[test]
    fn only_rejected_files_never_load_the_model() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![silent(dir.path(), "1.wav"), silent(dir.path(), "2.wav")];
        let o = run(&files, Some(2), vec![], true);
        assert_eq!(o.code, 0);
        assert_eq!((o.loads, o.calls), (0, 0));
        assert_eq!(o.lines.len(), 4);
        assert!(o.lines.iter().all(|l| field(l, "status") == "rejected"));
    }

    /// Ein nicht ladbares Modell macht jede freigegebene Datei zu `error`,
    /// abgelehnte bleiben `rejected`; geladen wird nur einmal versucht.
    #[test]
    fn failed_model_load_marks_speech_as_error_once() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![
            loud(dir.path(), "1.wav", 16_000),
            silent(dir.path(), "still.wav"),
            loud(dir.path(), "2.wav", 16_001),
        ];
        let o = run(&files, None, vec![], true);
        assert_eq!(o.code, 1);
        let statuses: Vec<_> = o.lines.iter().map(|l| field(l, "status").clone()).collect();
        assert_eq!(statuses, ["error", "rejected", "error"]);
        assert_eq!(field(&o.lines[0], "samples"), 16_000);
        assert_eq!(o.loads, 1);
        assert!(o.diag.contains("Modell nicht geladen: fehlt"), "{}", o.diag);
    }

    /// Ein Engine-Ergebnis „leer“ ist `text` mit leerem Text, keine Ablehnung.
    #[test]
    fn empty_engine_output_is_text_not_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![loud(dir.path(), "a.wav", 16_000)];
        let o = run(&files, None, vec![(16_000, Ok(""))], false);
        assert_eq!(o.code, 0);
        assert_eq!(field(&o.lines[0], "status"), "text");
        assert_eq!(field(&o.lines[0], "text"), "");
    }

    /// W3 (Review WP2): Das erste Engine-Ergebnis (der Warmup) scheitert, alle
    /// folgenden gelingen. Die betroffene Datei ist maschinenlesbar `error`
    /// mit voller Zeilenzahl, Exit 1; die nächste freigegebene Datei wärmt
    /// neu auf und wird gemessen. Exit 1 hat damit immer eine `error`-Zeile.
    #[test]
    fn failed_warmup_marks_file_as_error() {
        struct FirstCallFails(usize);
        impl Transcriber for FirstCallFails {
            fn transcribe(&mut self, _pcm: &[f32]) -> Result<Transcription, EngineError> {
                self.0 += 1;
                if self.0 == 1 {
                    return Err(EngineError::Failed("warm".into()));
                }
                Ok(Transcription {
                    text: "ok".into(),
                    language: None,
                    timing: None,
                })
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let files = vec![
            loud(dir.path(), "a.wav", 16_000),
            silent(dir.path(), "still.wav"),
            loud(dir.path(), "b.wav", 16_001),
        ];
        for runs in [None, Some(3)] {
            let mut out = Vec::new();
            let mut diag = Vec::new();
            let mut loads = 0;
            let code = run_batch(
                &files,
                runs,
                || {
                    loads += 1;
                    Ok(FirstCallFails(0))
                },
                &mut out,
                &mut diag,
            );
            assert_eq!(code, 1, "{runs:?}");
            assert_eq!(loads, 1);
            let diag = String::from_utf8(diag).unwrap();
            assert!(diag.contains(&format!("{}: Warmup: ", files[0])), "{diag}");
            let lines: Vec<serde_json::Value> = String::from_utf8(out)
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect();
            let per = runs.unwrap_or(1) as usize;
            assert_eq!(lines.len(), 3 * per, "{runs:?}");
            let statuses: Vec<_> = lines
                .iter()
                .map(|l| field(l, "status").as_str().unwrap().to_string())
                .collect();
            let expected: Vec<&str> = ["error", "rejected", "text"]
                .iter()
                .flat_map(|s| std::iter::repeat_n(*s, per))
                .collect();
            assert_eq!(statuses, expected, "{runs:?}");
            assert!(field(&lines[0], "infer_ms").is_null());
            assert_eq!(field(&lines[0], "samples"), 16_000);
            if let Some(n) = runs {
                for (i, line) in lines.iter().enumerate() {
                    assert_eq!(field(line, "run"), (i % n as usize + 1) as u64);
                }
            }
        }
    }
}
