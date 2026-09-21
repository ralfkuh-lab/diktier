# Relativer Silence-Gate (Plan, v1)

Stand: 2026-09-21, v1 (Entwurf, noch ohne Review). Anlass: Befund vom
2026-09-21 — die Jabra Evolve2 40 lieferte über Stunden ein um rund
15 dB leiseres Signal als gewohnt, Diktate endeten kommentarlos mit
„Transkript leer“, obwohl Overlay und Capture normal liefen und die
Aufnahme fürs Ohr gut verständlich war. 0.2.2 loggt seit heute den
Gate-Grund mit Messwerten; dieser Plan macht den Gate selbst robust
gegen den Absolutpegel.

**Spec-Status:** Verbindlich ist SPEC v1.5. §6.4 („Zu kurze Buffer
< 250 ms nicht transkribieren“) und §12 Phase 1 („dokumentierter
RMS-Silence-Gate“ als Halluzinationsschutz, Schwelle in SPIKES.md) sind
betroffen. Die Umsetzung braucht einen Spec-Nachtrag (→ v1.6, WP4).
Unverändert: §10 (nie Audio oder Text ins Log, nur Messwerte), §4.2
(Fokusregel — der Gate berührt keinen Fensterpfad).

## Ausgangslage (HEAD `afcaf77`, 0.2.2)

- Gate in `src/engine.rs::silence_gate`, drei Stufen mit **absoluter**
  Schwelle `RMS_SILENCE_THRESHOLD = 0.0075` (−42,5 dBFS):
  1. `< 250 ms` → leer.
  2. `max(RMS über 250-ms-Fenster) < 0.0075` → leer.
  3. Gesamt-RMS `< 0.0075` **und** kein zusammenhängender Lauf von
     ≥ 2 s über der Schwelle → leer (Schutz gegen den Klick in
     `rauschen.wav`).
- Aufrufer: `transcribe_pcm` (Engine-Worker, `--transcribe-wav`) und
  direkt die CLI-Spikes in `main.rs` (`transcribe_wav`, `record_test`).
- Die Schwelle wurde 2026-08 gegen einen stillen Raum kalibriert
  (Live-Halluzination „Ich bin jetzt wohl weiter zu machen.“, SPIKES.md
  Phase 2). Referenz: `stille.wav` RMS 0,0012, `rauschen.wav` 0,0051,
  leiseste Sprache `alltag.wav` 0,0215.

## Befunde vom 2026-09-21

### Die Aufnahme war Sprache, der Gate sagte Stille

Debug-WAV des Laufs 83 (4,0 s, Jabra, fürs Ohr klar verständlich):

| Wert | Aufnahme | Gate |
|---|---|---|
| RMS gesamt | 0,0037 (−48,7 dBFS) | Schwelle 0,0075 |
| max. 250-ms-Fenster | 0,0117 | über Schwelle → Stufe 3 |
| längster Lauf ≥ Schwelle | 0,50 s | < 2 s → **leer** |

Der Nutzer hatte nichts verstellt; der Pegel lag einfach ~15 dB unter
den Kalibrierungsaufnahmen. Wenige Minuten später (lauter gesprochen)
ging es „einigermaßen“ wieder.

### Die Engine braucht den Gate nicht für leise Sprache

`alltag.wav` digital abgesenkt und mit umgangenem Gate durch
`--transcribe-wav` (Debug-Build, gleicher Modellstand):

| Datei | RMS | Ergebnis |
|---|---|---|
| `alltag.wav` | 0,0215 | Referenztext („Werstadt“-Fall wie in SPIKES) |
| −16 dB | 0,0034 | **wortidentisch** zum Original, 0,78 s |
| −22 dB | 0,0017 | **wortidentisch** zum Original, 0,80 s |

Bei −22 dB liegt der Gesamt-RMS nur noch 3 dB über `stille.wav`. Der
Gate verwirft also Signale, die Parakeet fehlerfrei erkennt. Sein
einziger Zweck bleibt der Halluzinationsschutz bei **echter** Stille
oder Rauschen ohne Sprache.

### Verhältnis statt Absolutwert trennt die Testdateien sauber

Grundrauschen = 10. Perzentil der 250-ms-Fenster-RMS („floor“).
Lauteste Fenster relativ zum floor:

| Datei | floor | max. Fenster | Abstand |
|---|---|---|---|
| `stille.wav` | 0,0010 | 0,0016 | 4 dB |
| `rauschen.wav` | 0,0012 | 0,0204 | 25 dB (Klick) |
| `alltag.wav` | 0,0012 | 0,0873 | 37 dB |
| `fachwoerter.wav` | 0,0011 | 0,2542 | 47 dB |
| `zahlen_umlaute.wav` | 0,0011 | 0,2580 | 48 dB |

Fenster ≥ floor + X dB und längster zusammenhängender Lauf:

| Datei | +10 dB | +12 dB | +15 dB |
|---|---|---|---|
| `stille.wav` | 0/24 · 0 s | 0/24 · 0 s | 0/24 · 0 s |
| `rauschen.wav` | 7/36 · 1,00 s | 7/36 · 1,00 s | 5/36 · 0,75 s |
| `alltag.wav` | 39/48 · 5,25 s | 36/48 · 3,00 s | 33/48 · 2,75 s |

Der Klick in `rauschen.wav` hält den Abstand, aber nicht die Dauer:
sein längster Lauf ist 1,0 s. Das Dauer-Kriterium bleibt also nötig,
kann aber von 2 s auf **1,5 s** sinken. Die Absenkung einer Sprachdatei
ändert Abstände und Läufe nicht (digitale Skalierung) — genau das
macht den relativen Gate pegelunabhängig.

Vorbehalt: Ein floor aus dem 10. Perzentil setzt voraus, dass die
Aufnahme Pausen enthält. Bei 12 s Testdatei sind das ~5 Fenster; bei
einem 0,5-s-Diktat ist der floor praktisch das leiseste von zwei
Fenstern. Deshalb behält der Plan die absolute Stufe als schnellen
Ja-Pfad (siehe Regel B).

## Zielverhalten

Der Gate lässt Sprache unabhängig vom Absolutpegel durch und verwirft
weiterhin Stille, Raumrauschen und Einzelklicks ohne Engine-Aufruf.
Regeln in Reihenfolge:

- **A — zu kurz:** `< 250 ms` → leer (unverändert, §6.4).
- **B — sicher laut:** Gesamt-RMS ≥ 0,0075 → Sprache (unverändert;
  schneller Ja-Pfad für normal eingepegelte Mikros, keine Regression
  gegen heute).
- **C — digitale Null / totes Gerät:** floor **und** max. Fenster
  < `ABS_FLOOR = 0,0003` (−70 dBFS) → leer. Schutz gegen exakte
  Nullen (SPIKES: 0,5 s Nullen ergaben „Yeah.“) und gegen den Fall,
  dass der Verhältnis-Test auf Quantisierungsrauschen anspringt.
- **D — relativ:** floor = 10. Perzentil der 250-ms-Fenster-RMS
  (mindestens das Minimum). Sprache, wenn ein zusammenhängender Lauf
  von ≥ **1,5 s** Fenstern mit RMS ≥ floor · 10^(12/20) (+12 dB)
  existiert. Sonst leer.

Erwartung gegen die Befunde: `stille.wav` → C oder D leer,
`rauschen.wav` → D leer (Lauf 1,0 s < 1,5 s), alle Sprach-WAVs → B,
dieselben um 16/22 dB abgesenkt → D Sprache, Aufnahme Lauf 83
(floor unbekannt, max. Fenster 0,0117 bei RMS 0,0037) → D, sofern der
Lauf ≥ 1,5 s ist; sonst bleibt sie leer und der Log-Grund nennt es.

Log-Grund (`SilenceGate`, seit 0.2.2) wird erweitert: neue Varianten
`DeadInput { floor, max_window_rms }` und `NoRelativeRun { rms, floor,
max_window_rms, longest_run_secs, ratio_db }`; Display bleibt reine
Messwerte (§10).

### Nicht Teil des Plans

- **Windows-Mikrofonpegel automatisch anheben** (Nutzerfrage vom
  2026-09-21): verworfen. Diktier würde eine Systemeinstellung ändern,
  die Teams und alle anderen Programme mitbetrifft, ohne dass der
  Nutzer es sieht. Der Engine-Test oben zeigt, dass es nicht nötig ist.
- **Digitale Normalisierung vor der Engine**: nicht nötig (Engine ist
  pegelrobust), würde Rauschen mit anheben.
- **Overlay-Hinweis „zu leise“**: eigenes Folgepaket (unten, WP5),
  da es §4.5 berührt und ein Zustandsdetail durch `flush_presentation`
  ziehen muss.

## Arbeitspakete

### 🔍 WP1 — Fixtures

- `testdata/stt/alltag_-16db.wav` und `alltag_-22db.wav` aus
  `alltag.wav` per Skript erzeugen (deterministisch, Skalierung im
  16-bit-Bereich, kein Dithering) — `testdata/stt/normalize.py`
  erweitern oder kleines `attenuate.py` daneben. Ref-Text ist der von
  `alltag.wav`.
- `testdata/stt/README.md` um die Herkunft ergänzen.
- Optional, wenn reproduzierbar: eine echte leise Jabra-Aufnahme
  (`leise_jabra.wav`, `DIKTIER_DEBUG_WAV=1`, Pegel wie am 2026-09-21).
  Die Datei vom Lauf 83 ist überschrieben; nur aufnehmen, wenn der
  Zustand wieder auftritt.

### 🔍 WP2 — Gate umbauen (`src/engine.rs`)

- Konstanten: `RMS_SILENCE_THRESHOLD` bleibt (Regel B),
  `ABS_FLOOR = 0.0003`, `RELATIVE_MARGIN_DB = 12.0`,
  `MIN_SPEECH_RUN_SECS = 1.5`.
- Hilfsfunktionen: `window_rms(pcm) -> Vec<f32>` (einmal berechnen,
  statt max/Lauf getrennt zu scannen), `noise_floor(&[f32]) -> f32`
  (10. Perzentil, mind. Minimum), `longest_run_secs(windows, thr)`.
- `silence_gate` nach Regeln A–D; `SilenceGate` um die zwei Varianten
  erweitern; `is_silence_or_short` bleibt Wrapper. Rustdoc mit den
  Messwerten aus diesem Plan.
- Unit-Tests: bestehende bleiben grün (`rauschen.wav`-Nachbau mit
  Klick: Lauf 1,0 s → leer; Nullen → leer; 4 s leise + 0,5 s laut →
  leer mit `NoRelativeRun`); neu: synthetische Sprache bei −16/−22 dB
  → `None`; 12 s Signal mit floor 0,0001 und Lauf 2 s bei 0,002 →
  `None` (relativ), bei floor 0,0001 und max 0,0002 → `DeadInput`.
- `stt-smoke` (ignored-Test mit Golden Set) um die beiden
  abgesenkten Fixtures erweitern: Erwartung Ref-Text, WER-Puffer wie
  `alltag.wav`.

### 🔍 WP3 — CLI und Doku-Zeilen

- `--transcribe-wav` gibt bei Gate-Treffer den `SilenceGate`-Grund
  auf stderr aus (heute nur `rms=…` bzw. „< 250 ms“).
- README-Troubleshooting („Nichts wird erkannt“ und der 0.2.2-Eintrag)
  auf den relativen Gate umformulieren: Schwelle 0,0075 ist nicht mehr
  die einzige Grenze.

### 🔍 WP4 — Spec-Nachtrag (SPEC v1.6)

- §6.4: Absatz „Silence-Gate“ mit den Regeln A–D und den vier
  Konstanten; Verweis auf diesen Plan für die Kalibrierung.
- §12 Phase 1: Satz „Schwelle in docs/SPIKES.md“ → „Regeln in §6.4“.
- §18-Entscheidungstabelle: neue Zeile „relativer Gate, Engine ist
  pegelrobust (−22 dB wortidentisch)“.

### 🔍 WP5 — Folgepaket: Overlay-Hinweis bei Gate-Treffer

- Wenn der Gate greift, zeigt das Overlay statt sofortigem Ausblenden
  ~1,5 s „zu leise / nichts erkannt“ (Text oder Glyphe) und blendet
  dann aus. Nur Präsentation, kein Fokus, kein neuer `Effect`; Weg
  über `flush_presentation` wie beim Overlay selbst. Eigener Plan
  oder Nachtrag in `overlay-plan.md`; nicht Voraussetzung für WP1–4.

## Gates (Abnahme)

1. `cargo test` grün, Clippy ohne Meldung.
2. `stt-smoke` (mit ORT und Modell): alle fünf bisherigen Dateien wie
   bisher, plus `alltag_-16db.wav` und `alltag_-22db.wav` mit Ref-Text
   im WER-Puffer, `stille.wav` und `rauschen.wav` leer **ohne**
   Engine-Aufruf (Zähler im Test).
3. Live-Gate: `--record-test` bzw. Daemon mit `DIKTIER_DEBUG_WAV=1` im
   stillen Raum (10 s, kein Wort) → Log „Silence-Gate: …“, kein Text.
   Anschließend ein normales Diktat → eingefügt. Beides aus dem Log
   belegen (nur Messwerte, §10).
4. Keine Halluzination in Gate 3 über fünf Stille-Versuche.

## Risiken und offene Fragen

- **Raumrauschen mit Struktur** (Lüfter, der an- und ausgeht, Tastatur
  über 2 s): +12 dB über floor mit 1,5 s Lauf ist erreichbar. Dann ruft
  die Engine — und muss auf Nicht-Sprache leer liefern. Das war bisher
  vom Gate abgeschirmt. Abschätzung: Tippgeräusche sind impulsiv (kurze
  Läufe), Lüfter heben den floor selbst. Live-Gate 3 deckt den stillen
  Raum ab; ein Tipp-Test (10 s tippen, kein Wort) gehört dazu. Wenn er
  kippt: Marge auf +15 dB oder Lauf auf 2,0 s, beides in den Tabellen
  oben noch verträglich mit `alltag.wav` (2,75 s).
- **Sehr kurze Diktate (0,3–1,5 s)**: Regel D verlangt 1,5 s Lauf; ein
  leises Ein-Wort-Diktat unter 1,5 s bleibt leer, sofern nicht Regel B
  greift. Heute ist das genauso (Stufe 3 verlangt 2 s), also keine
  Verschlechterung, aber auch keine Lösung. Alternative für später:
  Lauf-Anforderung relativ zur Gesamtlänge (z. B. ≥ 60 % bei < 2 s
  Aufnahme). Vorerst dokumentierte Lücke.
- **Kalibrierung auf einer Stimme, einem Headset**: die Testdateien
  stammen alle von derselben Aufnahmesituation. Die Konstanten sind
  Startwerte; der Log-Grund (0.2.2) liefert im Betrieb die Daten, um
  sie nachzuziehen.
- **Review**: Plan vor Umsetzung durch Astra (`gpt-6-astra`, medium)
  prüfen lassen; Fragen: Perzentil-Wahl für den floor bei kurzen
  Aufnahmen, Regel C gegen Quantisierungsrauschen, ob Regel B als
  Ja-Pfad Halluzinationen bei lautem Rauschen (Regel-B-Pegel ohne
  Sprache) wieder zulässt — heute identisch, also keine Regression.
