# Umsetzung WP1–WP3, relativer Silence-Gate

Auftrag: [impl-silence-gate-prompt.md](impl-silence-gate-prompt.md), Plan
[../silence-gate-plan.md](../silence-gate-plan.md) v2, Vertrag SPEC §6.4
(v1.6). Basis: Commit `9f0f847`. Nicht committet; SPEC, Plan und Reviews
unverändert.

## ✅ WP1 — Synthetische Fixtures

- `testdata/stt/attenuate.py` (neu): 16-bit-WAV × `10^(−dB/20)`,
  kaufmännisch gerundet (eigene Funktion — Pythons `round()` rundet zur
  geraden Zahl), auf i16 gesättigt, kein Dithering, nur Stdlib (`wave`,
  `array`), druckt Faktor und SHA-256 der Ausgabe. `--selftest` prüft
  Rundung, Faktoren und Sättigung.
- Erzeugt (nicht committet, liegen im Working Tree):
  `alltag_-16db.wav`, `alltag_-22db.wav`, `fachwoerter_-16db.wav`,
  `zahlen_umlaute_-16db.wav`. Zweiter Lauf ergibt denselben SHA-256.
- `testdata/stt/README.md`: Herkunftstabelle (Quelle, dB, Faktor,
  SHA-256 der Ausgabe **und** der Quelldateien), Gültigkeitsbereich der
  synthetischen Ableitungen, Verweis auf `testdata/stt/local/` (war
  bereits in `.gitignore`), WER-Definition mit dem +0,05-Puffer aus
  §12/§18 #11.

## ✅ WP2 — Gate (`src/engine.rs`)

Konstanten, Fensterung, floor und Regeln A–D exakt nach der normativen
Definition; Rustdoc an `silence_gate` gibt sie vollständig wieder.
Neu: `Window { rms, samples }`, `window_rms`, `noise_floor`,
`longest_run_samples(windows, threshold, break_below)`, `db_to_ratio`.
`SilenceGate` = `TooShort | BelowAbsoluteFloor | NoRelativeRun |
InvalidInput { non_finite }`; `BelowThreshold`/`NoLoudRun` entfallen.
`silence_gate` liefert `GateReport`, `is_silence_or_short` ist Wrapper,
`transcribe_pcm` gibt Transkript und Report zurück (Signatur seit
[Nachtrag 1](#-nachtrag-1--gate-zeile-auch-im-inferenz-fehlerfall):
`(GateReport, Result<Transcription, EngineError>)`).

24 neue bzw. umgestellte Unit-Tests ohne Modell, alle über
`transcribe_pcm` mit `CountingStub` (Positivfälle belegen den erfolgten
Aufruf, Negativfälle die 0): Regel A an 3.999/4.000 Samples, B1 exakt
unter/an/über 0,0075 (auch bei einem einzigen Fenster und bei
gleichmäßigem Rauschen auf 0,0080), Astras B2-Gegenbeispiel
(8 s @ 0,003 + 2 s @ 0,010 → B2; mit 1,75 s → D lehnt ab), C knapp
unter/an/über `ABS_FLOOR` plus reine Null und 1 LSB, Astras
B1-Gegenbeispiel (10 s Null + 250 ms @ 0,001 → leer), zwei Klicks mit
Nullpause, Null + 3 s @ 0,004 → Engine (D), dieselben Fälle mit 1 LSB
Restpegel, D exakt an +12 dB (0,0039811) knapp darunter/darüber, Lauf
23.999 vs. 24.000 Samples, getrennte Läufe 1,0 s + 1,0 s, Restfenster
(floor ignoriert es, Lauf zählt seine 1.000 Samples), Perzentil-Index für
n = 1/2/8/10/11/20, `InvalidInput` mit NaN und ±Inf, Display jeder
Variante.

`stt_smoke_fixtures` (ignored) um die vier abgesenkten Dateien erweitert:
Sprachfälle prüfen `WER(normalisiert) ≤ WER(dieselbe Datei unabgesenkt) +
0,05`, Stille/Rauschen prüfen `Rejected` **und** den unveränderten Zähler
eines `CountingEngine`-Wrappers um `ParakeetTranscriber`.

## ✅ WP3 — Aufrufer, CLI, Doku

- `workers.rs::engine_loop`: genau ein Gate-Aufruf (über `transcribe_pcm`),
  INFO-Zeile `Lauf N: Gate: …` für **jede** Aufnahme, auch angenommene;
  die 0.2.2-Zeile „Silence-Gate: … — Engine nicht aufgerufen“ ist ersetzt,
  `silence_gate` wird dort nicht mehr direkt importiert.
- `main.rs::transcribe_wav` und `record_test`: `Gate: …` auf stderr, Text
  weiter auf stdout; die alte `SPIKE rms=…`-Zeile ist darin aufgegangen.
- Neu `--gate-analyze <wav>…` (kein Modell, kein `--foreground`): je Datei
  Report plus Alternativspalten für +10/+12/+15 dB und ≥1,0/1,5/2,0 s.
  Vier CLI-Tests (fehlendes Argument, Konflikt mit `--transcribe-wav`,
  fehlende Datei, zwei Dateien in einem Lauf).
- README-Troubleshooting: „Nichts wird erkannt“ und der 0.2.2-Eintrag auf
  den relativen Gate umgeschrieben (Regeln A/C/D benannt), neuer Punkt
  „Gate nachrechnen“ mit `--gate-analyze`.

## Entscheidungen im Spielraum

- **`GateReport.metrics: Option<GateMetrics>`** statt eines flachen
  Structs. Nur `InvalidInput` hat gar keine gültigen Zahlen; so muss der
  Report für diesen Fall keine erfinden (Astra W4). Innerhalb der Metriken
  sind `floor` und `threshold_d` `Option`, weil ohne volles Fenster (nur
  bei Regel A) kein Grundrauschen existiert. Display zeigt dort `—`.
- **`InvalidInput` wird vor Regel A geprüft.** Es ist eine Aussage über
  die Eingabedomäne, nicht eine der Regeln A–D; ein NaN in einem zu
  kurzen Puffer bleibt ein Eingabefehler.
- **Messwerte werden immer vollständig berechnet**, auch wenn eine frühe
  Regel entscheidet. Die Entscheidungsreihenfolge bleibt A → B1 → B2 →
  C → D; nur so steht in der Logzeile einer per B1 angenommenen Aufnahme
  auch floor/Schwelle D für die Nachkalibrierung. Kosten: ein
  Fenster-Durchlauf.
- **Display-Format:** `Engine (Regel B1: …) — 192000 Samples (12.000 s),
  Fenster 48/48 voll, RMS 0.02145, max. Fenster 0.08735, floor 0.00124,
  Schwelle D 0.00492, Lauf abs 2.75 s, Lauf rel 3.00 s`. RMS-Werte
  `{:.5}`, Laufdauern `{:.2}` s. Ein Format für Log, CLI und Analyse.
- **Normalisierung/WER in Rust nachgebaut** (Testmodul in `engine.rs`),
  kein Python-Aufruf. Die Portierung wird von einem eigenen, nicht
  ignorierten Test gegen den `normalize.py`-Selftest geprüft und wurde
  zusätzlich gegen `normalize.py` auf den echten Transkripten
  gegengerechnet (identisch: 0,0476 / 0,1000 / 0,1333).
- **WER-Baseline** ist dieselbe Datei unabgesenkt, im selben Testlauf
  gemessen — kein Zuschlag auf einen ohnehin schlechteren Wert (Astra W3).
- **`--gate-analyze`** braucht kein `--foreground` (wie
  `--transcribe-wav`), verarbeitet mehrere Dateien und liefert Exit 2 bei
  Formatfehlern, 1 bei Lesefehlern, sonst 0.
- **`transcribe_wav`/`record_test` rechnen den Gate vor dem Modellladen**
  selbst (`engine::silence_gate`) und ignorieren den Report aus
  `transcribe_pcm`. Sonst müsste eine abgelehnte Aufnahme erst das Modell
  laden. K2 („einmal rechnen“) zielt auf den Daemon-Pfad; dort ist es
  erfüllt.
- **Behalten:** `rms_f32` und `max_window_rms` bleiben öffentlich (jetzt
  über die Fensterliste implementiert), damit bestehende Tests und
  Diagnose weiter funktionieren.

## Gate-Ausgaben

```
$ cargo fmt --check           # keine Ausgabe
$ cargo clippy --all-targets  # Finished `dev` profile … (keine Meldung)

$ cargo test
running 378 tests
test result: ok. 377 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.22s

$ cargo test -- --ignored stt_smoke_fixtures
running 1 test
test engine::tests::stt_smoke_fixtures ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 377 filtered out; finished in 17.19s
```

Diese Zahlen sind der Stand vor Nachtrag 1; die aktuellen stehen dort.

WER und Gate-Regel aus demselben Lauf (`--nocapture`):

| Datei | Regel | WER |
|---|---|---|
| `stille.wav` | D → leer, Engine nicht gerufen | — |
| `rauschen.wav` | D → leer, Engine nicht gerufen | — |
| `alltag.wav` | B1 | 0,0476 (Baseline) |
| `fachwoerter.wav` | B1 | 0,1000 (Baseline) |
| `zahlen_umlaute.wav` | B1 | 0,1333 (Baseline) |
| `alltag_-16db.wav` | D | 0,0476 |
| `alltag_-22db.wav` | D | 0,0476 |
| `fachwoerter_-16db.wav` | D | 0,1000 |
| `zahlen_umlaute_-16db.wav` | D | 0,1333 |

Alle abgesenkten Dateien sind wortidentisch zum Original — die
Pegelrobustheit aus dem Plan ist damit maschinell belegt.

Live-Beleg für den stderr-Pfad (`--foreground --record-test 2`, Jabra
stummgeschaltet):

```
SPIKE device=Mikrofon (Jabra Evolve2 40)
Gate: leer (Regel C: max. Fenster unter 0.00030) — 32000 Samples (2.000 s), Fenster 8/8 voll, RMS 0.00000, max. Fenster 0.00000, floor 0.00000, Schwelle D 0.00100, Lauf abs 0.00 s, Lauf rel 0.00 s
```

## `--gate-analyze` über alle `testdata/stt/*.wav`

```
testdata/stt/stille.wav
  leer (Regel D: kein aktiver Lauf ≥ 1.50 s) — 96000 Samples (6.000 s), Fenster 24/24 voll, RMS 0.00119, max. Fenster 0.00157, floor 0.00096, Schwelle D 0.00384, Lauf abs 0.00 s, Lauf rel 0.00 s
  Marge   Schwelle  Lauf      ≥1.0 s  ≥1.5 s  ≥2.0 s
  +10 dB  0.00305    0.00 s   nein    nein    nein
  +12 dB  0.00384    0.00 s   nein    nein    nein
  +15 dB  0.00542    0.00 s   nein    nein    nein

testdata/stt/rauschen.wav
  leer (Regel D: kein aktiver Lauf ≥ 1.50 s) — 144000 Samples (9.000 s), Fenster 36/36 voll, RMS 0.00514, max. Fenster 0.02036, floor 0.00117, Schwelle D 0.00464, Lauf abs 0.50 s, Lauf rel 1.00 s
  Marge   Schwelle  Lauf      ≥1.0 s  ≥1.5 s  ≥2.0 s
  +10 dB  0.00369    1.00 s   ja      nein    nein
  +12 dB  0.00464    1.00 s   ja      nein    nein
  +15 dB  0.00656    0.75 s   nein    nein    nein

testdata/stt/alltag.wav
  Engine (Regel B1: Gesamt-RMS ≥ 0.00750) — 192000 Samples (12.000 s), Fenster 48/48 voll, RMS 0.02145, max. Fenster 0.08735, floor 0.00124, Schwelle D 0.00492, Lauf abs 2.75 s, Lauf rel 3.00 s
  Marge   Schwelle  Lauf      ≥1.0 s  ≥1.5 s  ≥2.0 s
  +10 dB  0.00391    5.25 s   ja      ja      ja
  +12 dB  0.00492    3.00 s   ja      ja      ja
  +15 dB  0.00696    2.75 s   ja      ja      ja

testdata/stt/alltag_-16db.wav
  Engine (Regel D: Lauf ≥ 1.50 s über Schwelle D) — 192000 Samples (12.000 s), Fenster 48/48 voll, RMS 0.00340, max. Fenster 0.01384, floor 0.00020, Schwelle D 0.00100, Lauf abs 0.25 s, Lauf rel 2.75 s
  Marge   Schwelle  Lauf      ≥1.0 s  ≥1.5 s  ≥2.0 s
  +10 dB  0.00100    2.75 s   ja      ja      ja
  +12 dB  0.00100    2.75 s   ja      ja      ja
  +15 dB  0.00110    2.75 s   ja      ja      ja

testdata/stt/alltag_-22db.wav
  Engine (Regel D: Lauf ≥ 1.50 s über Schwelle D) — 192000 Samples (12.000 s), Fenster 48/48 voll, RMS 0.00170, max. Fenster 0.00694, floor 0.00010, Schwelle D 0.00100, Lauf abs 0.00 s, Lauf rel 1.50 s
  Marge   Schwelle  Lauf      ≥1.0 s  ≥1.5 s  ≥2.0 s
  +10 dB  0.00100    1.50 s   ja      ja      nein
  +12 dB  0.00100    1.50 s   ja      ja      nein
  +15 dB  0.00100    1.50 s   ja      ja      nein

testdata/stt/fachwoerter.wav
  Engine (Regel B1: Gesamt-RMS ≥ 0.00750) — 192000 Samples (12.000 s), Fenster 48/48 voll, RMS 0.04215, max. Fenster 0.25421, floor 0.00114, Schwelle D 0.00453, Lauf abs 5.75 s, Lauf rel 5.75 s
  Marge   Schwelle  Lauf      ≥1.0 s  ≥1.5 s  ≥2.0 s
  +10 dB  0.00360    5.75 s   ja      ja      ja
  +12 dB  0.00453    5.75 s   ja      ja      ja
  +15 dB  0.00640    5.75 s   ja      ja      ja

testdata/stt/fachwoerter_-16db.wav
  Engine (Regel D: Lauf ≥ 1.50 s über Schwelle D) — 192000 Samples (12.000 s), Fenster 48/48 voll, RMS 0.00668, max. Fenster 0.04029, floor 0.00018, Schwelle D 0.00100, Lauf abs 0.25 s, Lauf rel 5.75 s
  Marge   Schwelle  Lauf      ≥1.0 s  ≥1.5 s  ≥2.0 s
  +10 dB  0.00100    5.75 s   ja      ja      ja
  +12 dB  0.00100    5.75 s   ja      ja      ja
  +15 dB  0.00101    5.75 s   ja      ja      ja

testdata/stt/zahlen_umlaute.wav
  Engine (Regel B1: Gesamt-RMS ≥ 0.00750) — 192000 Samples (12.000 s), Fenster 48/48 voll, RMS 0.04277, max. Fenster 0.25804, floor 0.00106, Schwelle D 0.00424, Lauf abs 5.00 s, Lauf rel 5.00 s
  Marge   Schwelle  Lauf      ≥1.0 s  ≥1.5 s  ≥2.0 s
  +10 dB  0.00337    5.00 s   ja      ja      ja
  +12 dB  0.00424    5.00 s   ja      ja      ja
  +15 dB  0.00599    5.00 s   ja      ja      ja

testdata/stt/zahlen_umlaute_-16db.wav
  Engine (Regel D: Lauf ≥ 1.50 s über Schwelle D) — 192000 Samples (12.000 s), Fenster 48/48 voll, RMS 0.00678, max. Fenster 0.04090, floor 0.00017, Schwelle D 0.00100, Lauf abs 0.25 s, Lauf rel 5.00 s
  Marge   Schwelle  Lauf      ≥1.0 s  ≥1.5 s  ≥2.0 s
  +10 dB  0.00100    5.00 s   ja      ja      ja
  +12 dB  0.00100    5.00 s   ja      ja      ja
  +15 dB  0.00100    5.00 s   ja      ja      ja
```

Die Erwartung aus dem Plan trifft mit der normativen Definition exakt zu:
`stille.wav` und `rauschen.wav` → D leer, Original-Sprache → B1, abgesenkte
Sprache → D Engine. Gemessene floor-Werte weichen leicht von der v1-Messung
ab (Nearest-Rank über nur volle Fenster): `stille.wav` 0,00096 statt 0,0010,
`rauschen.wav` 0,00117, `alltag.wav` 0,00124.

## Offen

- **`alltag_-22db.wav` hat keine Reserve:** Lauf rel genau 1,50 s. Eine
  Verschärfung auf 2,0 s Laufdauer (Fallback aus der WP0-Analysetabelle)
  würde diese Datei verwerfen — bei einer Nachkalibrierung nach WP0 ist
  das der erste Fall, der kippt. `rauschen.wav` liegt mit 1,00 s auf der
  anderen Seite nur 0,5 s von der Grenze entfernt; der Abstand zwischen
  Ja und Nein beträgt in diesem Satz also genau 0,5 s.
- ✅ ~~**Gate-Zeile fehlt, wenn die Engine selbst scheitert.**~~ Erledigt in
  [Nachtrag 1](#-nachtrag-1--gate-zeile-auch-im-inferenz-fehlerfall):
  `transcribe_pcm` liefert den Report jetzt neben dem `Result`, der Daemon
  loggt ihn vor dessen Auswertung.
- **WER `fachwoerter.wav` = 0,1000**, `docs/SPIKES.md` notiert für dieselbe
  Datei 0,0500. `normalize.py` und die Rust-Portierung liefern beide
  0,1000 auf dem heutigen Transkript („Rust Demon“ **und** „ONX Modell“ =
  2 von 20 Wörtern); der historische Eintrag zählte offenbar nur ein Wort.
  Kein Einfluss auf den Test (er vergleicht relativ), aber SPIKES.md ist
  an der Stelle irreführend. Nicht angefasst — SPIKES gehört zu WP0.
- **Version nicht angehoben.** `Cargo.toml` steht weiter auf 0.2.2, die
  README-Versionszeile ebenfalls; der README-Troubleshooting-Text nennt
  deshalb keine Version für den relativen Gate. Bump und Release-Notiz
  gehören zur Freigabe nach WP0.
- **Zeilenenden:** die neu geschriebenen Dateien wurden auf CRLF gebracht,
  passend zum restlichen Working Tree (`core.autocrlf=true`).

## ✅ Nachtrag 1 — Gate-Zeile auch im Inferenz-Fehlerfall

Auftrag vom 2026-09-21 im Anschluss an den Bericht: den offenen Punkt
beheben, damit der Log-Vertrag aus §6.4 („pro Aufnahme genau eine Zeile
mit dem Gate-Report“) auch dann gilt, wenn die Inferenz scheitert.

- `engine.rs`: `transcribe_pcm(...) -> (GateReport, Result<Transcription,
  EngineError>)`. Der Report steht neben dem Ergebnis statt darin; er liegt
  damit in jedem Fall vor. Bei Ablehnung unverändert leeres Transkript ohne
  Engine-Aufruf. Rustdoc nennt die Begründung.
- `workers.rs::engine_loop`: `let (report, result) = transcribe_pcm(…);`,
  danach `log.run(run, format!("Gate: {report}"))` **vor** dem `match` über
  `result`. Die Fehlerzeile `Transkription: <Fehler>` bleibt dahinter — ein
  gescheiterter Lauf hat jetzt beide Zeilen.
- `main.rs`: `transcribe_wav` (Warmup und Messschleife) und `record_test`
  greifen auf `.1` zu; die Report-Zeile dieser Pfade kommt weiterhin aus dem
  Gate-Aufruf vor dem Modellladen.
- Tests: `run_gate`-Helfer, `audio_shorter_than_250ms_skips_engine` und die
  drei Aufrufe in `stt_smoke_fixtures` auf das Tupel umgestellt. Neu
  `report_survives_an_engine_error` — ein `FailingStub` belegt, dass der
  Report bei `Err` vollständig ankommt (Entscheidung B1, Metriken vorhanden).

```
$ cargo fmt --check           # keine Ausgabe
$ cargo clippy --all-targets  # Finished `dev` profile … (keine Meldung)

$ cargo test
running 379 tests
test engine::tests::report_survives_an_engine_error ... ok
test result: ok. 378 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.22s

$ cargo test -- --ignored stt_smoke_fixtures
running 1 test
test engine::tests::stt_smoke_fixtures ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 378 filtered out; finished in 19.18s
```

`--gate-analyze` ist von der Änderung nicht berührt (eigener
`silence_gate`-Aufruf); die Tabelle oben gilt unverändert. Nicht committet.
