# Umsetzung SPEC v1.9: Vorlauf-Stille und Float-Debug-WAV (Notizen)

Auftrag: [impl-lead-in-silence-prompt.md](impl-lead-in-silence-prompt.md).
Basis: Working Tree mit dem uncommitteten Stand 0.4.0. Nicht committet.

## ✅ Umgesetzt

- **src/engine.rs**
  - Neue Konstante `LEAD_IN_SILENCE_SAMPLES: usize = 4800` mit
    Doc-Kommentar (SPEC §6.4 v1.9, SPIKES 2026-09-30).
  - `transcribe_pcm`: Der Gate läuft wie bisher auf dem unveränderten Puffer.
    Nur bei Freigabe entsteht ein neuer `Vec` (Kapazität 4800 + n, 4800 × `0.0`,
    dann `extend_from_slice` mit dem Original), und nur dieser geht an
    `engine.transcribe`. Bei Ablehnung bleibt es beim frühen `return` ohne
    Allokation und ohne Engine-Aufruf. Der Report ist der des Originals.
  - Dauer: Bisher rechnete `ParakeetTranscriber::transcribe` sie aus der
    Länge seines Eingangspuffers. Mit der Stille wären das 300 ms zu viel
    gewesen. Jetzt setzt nur noch `transcribe_pcm` bei `Ok` den Wert
    `timing = Some(Timing { duration: n / ENGINE_RATE })` aus der
    Originallänge. `ParakeetTranscriber` liefert `timing: None` und kennt
    die Stille nicht. Die Rate kommt aus `crate::audio::ENGINE_RATE`; das
    frühere lokale `16_000u32` ist damit weg. Außerhalb der Tests liest
    niemand `timing`, das Verhalten nach außen ändert sich also nicht.
  - Tests ohne Modell:
    - `accepted_audio_gets_lead_in_silence` nutzt einen `RecordingStub`, der
      den Puffer speichert und absichtlich eine falsche Dauer (999 s) meldet.
      Geprüft werden: Länge = Original + 4800; die ersten 4800 Samples sind
      bitgleich `+0.0`; danach folgt das Original bitgleich (Sinus plus
      `1e-6`-Anteile); der Report entspricht `silence_gate(&pcm)`,
      `report.samples` der Originallänge; die Dauer ist genau 3 s statt 999 s;
      der Text wird durchgereicht. Außerdem steht dort
      `LEAD_IN_SILENCE_SAMPLES == 16_000 * 300 / 1_000`.
    - `rejected_audio_never_reaches_the_engine`: Stille (Regel D) und
      < 250 ms (Regel A) rufen den Stub nicht auf, das Ergebnis ist
      `Transcription::empty()`.
    - Angepasst: `expect_speech` und `audio_shorter_than_250ms_skips_engine`
      erwarten jetzt vom `CountingStub` die Länge `n + LEAD_IN_SILENCE_SAMPLES`.
- **src/daemon/debug_wav.rs**
  - `write_wav` schreibt 16 kHz mono, `bits_per_sample: 32`,
    `SampleFormat::Float`, die Samples unverändert, ohne Clamp und ohne
    Rundung. Das Modul-Doc und ein Doc-Kommentar an `write_wav` nennen das
    Format.
  - `to_i16` ist entfernt, weil es sonst niemand nutzte. Mit ihm ist auch der
    Test `clipping_stays_in_range` weggefallen.
  - Neuer Test `the_dump_is_float32_and_bit_exact`: Er prüft die Spec des
    Headers (1 Kanal, 16 kHz, 32 bit, Float) und dass der Roundtrip über
    `audio::read_wav_16k_mono` bitgleich ist. Werte: `0.0`, `-0.0`, `±1e-6`,
    `f32::MIN_POSITIVE`, `±0.5`, `±0.99999`, `±1.0`, `1.5`, `-2.0`,
    `0.12345679`. `writes_the_wav_atomically` prüft jetzt ebenfalls Bits statt
    einer Toleranz von 1e-3. Die Ring-Tests sind unverändert grün.
  - **Bestätigung „vor der Stille“:** `workers.rs` ruft
    `dump_debug_wav(run, &captured.samples, log)` im Capture-Zweig auf, bevor
    `captured.samples` als `Msg::Audio` an die Engine geht. Der Dump läuft
    also nicht über `transcribe_pcm`, und die Stille entsteht erst dort als
    neuer Puffer. Das WAV enthält damit die Capture-Samples ohne Stille,
    bitgleich zu dem, was Gate und `transcribe_pcm` bekommen.
- **README.md**
  - Versionszeile v0.4.1.
  - Troubleshooting: neuer Punkt zu „Herr Präsident.“ am Anfang. Seit 0.4.1
    ist das durch 300 ms Vorlauf-Stille behoben; das vorangestellte „Ich“
    bei Befehlen bleibt als bekannte Grenze.
  - Abschnitt Debug-WAV: Format 32-bit-Float seit 0.4.1 (bis 0.4.0 16-bit),
    Samples ohne Rundung und ohne Vorlauf-Stille, etwa doppelte Dateigröße
    (rund 64 KB je Sekunde), per `--transcribe-wav` bitgenau nachrechenbar.
- **Cargo.toml** 0.4.1, **Cargo.lock** durch den Build nachgezogen
  (`name = "diktier"` / `version = "0.4.1"`).
- **docs/windows-plan.md**, WP6: ein Satz zu 0.4.1 im Stil der bisherigen
  Versionsnotizen.

## ⚠️ Abweichungen und warum

- **Report-Vergleich:** Der Auftrag verlangt, dass der Report „identisch zum
  Aufruf ohne Stille“ ist. Einen Aufrufpfad ohne Stille gibt es nicht mehr.
  Der Test vergleicht deshalb mit `silence_gate(&pcm)` auf dem Original. Das
  ist genau der Report, den `transcribe_pcm` vor v1.9 zurückgab.
- **`ParakeetTranscriber` liefert `timing: None`:** Der Auftrag ließ offen,
  ob die Dauer dort korrigiert oder in `transcribe_pcm` gesetzt wird. Ich
  habe sie nur in `transcribe_pcm` gesetzt, damit keine zweite Stelle die
  Stille kennen muss. Wer `ParakeetTranscriber::transcribe` direkt ruft,
  bekommt keine Dauer mehr. Solche Aufrufer gibt es nicht; `daemon/mod.rs:845`
  ist ein anderes Engine-Handle, das über `workers.rs` auf `transcribe_pcm`
  führt.
- **README-Versionslink:** Der Link zeigt auf
  `releases/tag/v0.4.1`, analog zur bisherigen Zeile. Dieses Release gibt es
  noch nicht; laut Memory gilt das auch für v0.4.0. Er bleibt tot, bis ein
  Release erstellt wird.

## ✅ Gates

1. `cargo fmt --check`: keine Ausgabe, Exit 0.
2. `cargo clippy --all-targets -- -D warnings`:
   ```
      Compiling diktier v0.4.1 (D:\DEV\diktier)
       Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.47s
   ```
   keine Meldung, Exit 0.
3. `cargo test`:
   ```
   test result: ok. 505 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 2.02s
   ```
   Darunter `engine::tests::accepted_audio_gets_lead_in_silence ... ok`,
   `engine::tests::rejected_audio_never_reaches_the_engine ... ok` und
   `daemon::debug_wav::tests::the_dump_is_float32_and_bit_exact ... ok`.
4. `cargo test -- --ignored stt_smoke_fixtures`:
   ```
   test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 514 filtered out; finished in 17.40s
   ```
   WER je Fixture (aus einem zweiten Lauf mit `--nocapture`, Ergebnis dort
   ebenfalls `ok`, 17.47 s):

   | Fixture | WER | Gate |
   |---|---|---|
   | alltag.wav | 0.0000 | B1 |
   | fachwoerter.wav | 0.0500 | B1 |
   | zahlen_umlaute.wav | 0.1333 | B1 |
   | alltag_-16db.wav | 0.0000 | D |
   | alltag_-22db.wav | 0.0000 | D |
   | fachwoerter_-16db.wav | 0.1000 | D |
   | zahlen_umlaute_-16db.wav | 0.1333 | D |

   `stille.wav` und `rauschen.wav` sind abgelehnt (Regel D), die Engine
   lief nicht. Alle abgesenkten Dateien liegen innerhalb von Baseline + 0,05.
   Einen direkten Vergleich mit den WER-Werten ohne Stille habe ich in
   diesem Lauf nicht gerechnet; die Matrix dazu steht in SPIKES.md.
5. `$env:CARGO_TARGET_DIR="target-dev"; cargo build --release`:
   ```
      Compiling diktier v0.4.1 (D:\DEV\diktier)
       Finished `release` profile [optimized] target(s) in 30.07s
   ```
   `target-dev\release\diktier.exe --version` (Exit 0):
   ```
   diktier 0.4.1
   ```
   `target-dev\release\diktier.exe --transcribe-wav testdata\stt\local\herr_praesident\lauf-523_2026-09-30T08-59-48Z_herr-praesident.wav`
   (Exit 0, stderr und stdout zusammen):
   ```
   Gate: Engine (Regel B1: Gesamt-RMS ≥ 0.00750) — 159360 Samples (9.960 s), Fenster 39/40 voll, RMS 0.02427, max. Fenster 0.04755, floor 0.00081, Schwelle D 0.00322, Lauf abs 3.50 s, Lauf 0.004 3.50 s, Lauf rel 3.50 s
   Modell geladen in 2.230 s
   Wir werden so vorgehen, dass wir unseren Pull Request abbrechen, und Sebastian macht seinen Branch fertig und erstell einen eigenen Pull Request.
   Inferenz 0.483 s
   ```
   Der Text beginnt nicht mit „Herr Präsident“. Der Gate-Report zählt
   159360 Samples, also das Original ohne die 4800 Nullen.

   Zusätzlich, nicht verlangt (stdout, gleiche Binary):
   - Lauf 545 (roh, 16-bit-Datei): „Den Befund kannst du da schon mal
     eintragen. Kann meinetwegen ruhig eine eigene Markdown Datei sein, wo das
     so schön erklärt ist, wie du es mir gerade erklärt hast.“ Kein „Herr
     Präsident“. Die Phrase zeigte die rohe Datei laut SPIKES auch vorher
     nicht, der Beleg ist also schwach.
   - Lauf 547: „Ich schaue auch mal ganz kurz im Internet nach, …“. Das
     vorangestellte „Ich“ bleibt, wie in SPEC §6.4 als Grenze beschrieben.

## 🔍 Offen

- Die installierte Version (0.4.0) ist nicht angefasst; ein laufender Daemon
  wurde nicht beendet. Installation und Release von 0.4.1 bleiben offen.
- Commit steht aus (laut Auftrag verboten).
- In geschützten Dateien (SPEC, SPIKES, reviews, inject, overlay, state,
  testdata) sind mir keine Fehler aufgefallen.
- Zur Kenntnis: `testdata/stt/local/herr_praesident/lauf-523_….wav` ist
  weiterhin 16-bit (318764 Byte für 159360 Samples). Das ist erwartbar, weil
  die Datei vor v1.9 entstand; bitgenaue Belege liefern erst neue Dumps.

## ✅ Nacharbeit

Grundlage ist [impl-lead-in-silence-sol.md](impl-lead-in-silence-sol.md),
Tabelle „Testgrenzen“. Umgesetzt sind nur die Zeilen 1 und 2, und nur mit
Tests in `src/engine.rs`. Die Zeilen 3 (Wiring-Tests Daemon/CLI) und 4
(NaN/Inf im Dump-Writer) sind wie beauftragt nicht umgesetzt. Am
Produktionscode hat sich nichts geändert.

- **Test-Hilfen:** `RecordingStub` hat jetzt das Feld `fail` sowie die
  Konstruktoren `ok()` und `failing()`. `failing()` speichert den Puffer und
  liefert danach `Err(EngineError::Failed("kaputt"))`. Neu sind außerdem die
  Helfer `assert_lead_in_then_original` (Länge, 4800 × bitgleich `+0.0`,
  Original bitgleich) und `wobble` (Testsignal mit Sinus und `1e-6`-Anteilen).
  `accepted_audio_gets_lead_in_silence` und
  `rejected_audio_never_reaches_the_engine` nutzen diese Helfer, prüfen aber
  inhaltlich dasselbe wie vorher.
- **Zeile 1:**
  - `duration_is_exact_for_a_fraction_of_a_second`: 48001 Samples, Regel B1.
    Die Dauer muss exakt `Duration::new(3, 62_500)` sein, also 48001/16000 s.
    Eine Kürzung auf Sekunden oder Millisekunden würde auffallen. Außerdem
    geprüft: der Puffer ist bitgleich mit Vorlauf, und
    `report == silence_gate(&pcm)`.
  - `lead_in_silence_also_on_rule_d`: 10 s Nullen, dann 3 s abwechselnd
    ±0.0035. Das liegt unter `QUIET_SPEECH_RMS`, deshalb greift weder B1/B2
    noch B3. Geprüft: die Entscheidung ist `Speech(SpeechRule::D)`, der Puffer
    ist bitgleich mit Vorlauf, `report == silence_gate(&pcm)`, die Dauer ist
    13 s.
- **Zeile 2:** `engine_error_keeps_report_and_lead_in` prüft mit
  `RecordingStub::failing()`: `report == silence_gate(&pcm)`, der
  empfangene Puffer besteht aus 4800 Nullen und dem bitgleichen Original, das
  Ergebnis ist `Err(EngineError::Failed("kaputt"))`. Der ältere Test
  `report_survives_an_engine_error` bleibt unverändert.

Gates:

1. `cargo fmt --check`: keine Ausgabe, Exit 0.
2. `cargo clippy --all-targets -- -D warnings`:
   ```
       Checking diktier v0.4.1 (D:\DEV\diktier)
       Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.32s
   ```
   keine Meldung, Exit 0.
3. `cargo test`:
   ```
   test result: ok. 508 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 2.00s
   ```
   Darunter `engine::tests::duration_is_exact_for_a_fraction_of_a_second ... ok`,
   `engine::tests::lead_in_silence_also_on_rule_d ... ok` und
   `engine::tests::engine_error_keeps_report_and_lead_in ... ok`.

Nicht committet.
