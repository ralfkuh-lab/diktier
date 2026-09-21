# Relativer Silence-Gate (Plan, v2)

Stand: 2026-09-21, v2 nach Plan-Review durch Astra
([reviews/plan-silence-gate-astra.md](reviews/plan-silence-gate-astra.md),
Auftrag [reviews/plan-silence-gate-prompt.md](reviews/plan-silence-gate-prompt.md)):
alle vier Blocker (B1–B4) und die Hinweise W1–W5 eingearbeitet, K1–K3
übernommen, F1–F4 entschieden (Abschnitt „Entscheidungen“). v1 vom selben
Tag war der Entwurf ohne Review.

Anlass: Befund vom 2026-09-21 — die Jabra Evolve2 40 lieferte über Stunden
ein um rund 15 dB leiseres Signal als gewohnt, Diktate endeten kommentarlos
mit „Transkript leer“, obwohl Overlay und Capture normal liefen und die
Aufnahme fürs Ohr gut verständlich war. 0.2.2 loggt seit dem Tag den
Gate-Grund mit Messwerten; dieser Plan macht den Gate selbst robust gegen
den Absolutpegel — in dem belegten Rahmen, nicht als Klassifikationsgarantie.

**Spec-Status:** Verbindlich ist SPEC v1.5. §6.4 („Zu kurze Buffer
< 250 ms nicht transkribieren“) und §12 Phase 1 („dokumentierter
RMS-Silence-Gate“) sind betroffen. **Reihenfolge nach Review (W5):** erst
der Spec-Nachtrag (WP4, → v1.6) mit der normativen Definition aus diesem
Plan, dann Code. Unverändert: §10 (nie Audio oder Text ins Log, nur
Messwerte), §4.2 (der Gate berührt keinen Fensterpfad).

## Ausgangslage (HEAD `5315181`, 0.2.2)

- Gate in `src/engine.rs::silence_gate`, drei Stufen mit **absoluter**
  Schwelle `RMS_SILENCE_THRESHOLD = 0.0075` (−42,5 dBFS):
  1. `< 250 ms` → leer.
  2. `max(RMS über 250-ms-Fenster) < 0.0075` → leer.
  3. Gesamt-RMS `< 0.0075` **und** kein zusammenhängender Lauf von
     ≥ 2 s über der Schwelle → leer (Schutz gegen den Klick in
     `rauschen.wav`).
- Aufrufer: `transcribe_pcm` (Engine-Worker, `--transcribe-wav`) und
  direkt die CLI-Spikes in `main.rs` (`transcribe_wav`, `record_test`).
  Der Daemon ruft den Gate heute zweimal (Logzeile + `transcribe_pcm`).
- Kalibriert 2026-08 gegen einen stillen Raum (Live-Halluzination „Ich
  bin jetzt wohl weiter zu machen.“, SPIKES.md Phase 2). Referenz:
  `stille.wav` RMS 0,0012, `rauschen.wav` 0,0051, leiseste Sprache
  `alltag.wav` 0,0215.

## Befunde vom 2026-09-21

### Die Aufnahme war Sprache, der Gate sagte Stille

Debug-WAV des Laufs 83 (4,0 s, Jabra, fürs Ohr klar verständlich; die
Datei ist inzwischen überschrieben, nur diese Kennzahlen sind gesichert):

| Wert | Aufnahme | Gate |
|---|---|---|
| RMS gesamt | 0,0037 (−48,7 dBFS) | Schwelle 0,0075 |
| max. 250-ms-Fenster | 0,0117 | über Schwelle → Stufe 3 |
| längster Lauf ≥ Schwelle | 0,50 s | < 2 s → **leer** |

Der Nutzer hatte nichts verstellt; der Pegel lag ~15 dB unter den
Kalibrierungsaufnahmen. Wenige Minuten später (lauter gesprochen) ging es
„einigermaßen“ wieder. Aus max. Fenster und Gesamt-RMS lässt sich der
Lauf nach der neuen Regel D **nicht** ableiten (Astra F3) — Lauf 83 ist
Anlass, kein Testfall.

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
Zweck bleibt der Halluzinationsschutz bei Stille oder Rauschen **ohne**
Sprache. Vorbehalt (Astra W3): digitale Absenkung skaliert Sprache und
Rauschen gemeinsam; der reale Fall „Sprache leiser bei gleichem
Grundrauschen“ ist damit nur angenähert — deshalb WP0.

### Verhältnis statt Absolutwert trennt die Testdateien

Grundrauschen („floor“) = unteres Quantil der 250-ms-Fenster-RMS,
Messung mit Python (`sorted[int(0.1·n)]`, alle Fenster inkl. Rest — v1;
die normative Definition unten weicht leicht ab, WP0 misst neu):

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

Der Klick in `rauschen.wav` hält den Abstand, aber nicht die Dauer (1,0 s).
Das Dauer-Kriterium bleibt nötig; 1,5 s trennt mit Reserve nach beiden
Seiten (Klick 1,0 s, Sprache 2,75 s bei +15 dB).

## Entscheidungen (Antworten auf Astra F1–F4)

- **F1 — Zusage von „pegelunabhängig“:** Zugesagt ist nur: *Sprache mit
  mindestens 1,5 s zusammenhängendem Kontrast von +12 dB über dem
  Grundrauschen der Aufnahme, oberhalb der absoluten Aktivitätsgrenze,
  wird durchgelassen.* Leise Ein-Wort-Diktate, pausenlose leise Diktate
  ohne Rauschfenster und Signale unter der Aktivitätsgrenze bleiben
  **außerhalb dieses Pakets** und werden in §6.4 als bekannte Grenze
  dokumentiert. Kein neuer Ja-Pfad ohne Messbeleg.
- **F2 — Bisherige Ja-Entscheidungen:** **Alle erhalten.** Der absolute
  2-s-Lauf-Pfad bleibt als Regel B2 bestehen. Der Umbau darf kein Signal
  verwerfen, das heute durchkommt; Astras Gegenbeispiel (8 s bei 0,003,
  dann 2 s bei 0,010) wird Regressionstest.
- **F3 — Reale Aufnahmematrix:** Ralf nimmt die Matrix aus WP0 auf
  (Jabra, plus wenn verfügbar das Laptop-Mikrofon mit dokumentiertem
  Windows-Effekte-Zustand). Positivfälle und Nicht-Sprachfälle werden
  als lokale, **nicht committete** Fixtures gesichert (Privatsphäre,
  Repo-Größe) und in SPIKES.md mit Kennzahlen dokumentiert. Committet
  werden nur die synthetischen Ableitungen der vorhandenen Sprach-WAVs.
- **F4 — Live-Erfolg:** Getrennt in (a) CLI-Capture-Abnahme mit
  `--record-test` (stderr zeigt Gate-Grund und Transkriptlänge, Text auf
  stdout, manueller Vergleich) und (b) Daemon-End-to-End mit einem
  **leisen** und einem normalen Diktat ins Zielprogramm, Pass/Fail
  manuell im Abnahmeprotokoll, Log nur Messwerte.

## Zielverhalten — normative Definition

Eingabe: `pcm: &[f32]`, 16 kHz mono, Werte endlich (K1). Nicht-endliche
Samples → `SilenceGate::InvalidInput { non_finite: usize }`, leer, Engine
nicht aufgerufen; kein eigener Bereinigungsversuch im Gate.

**Fensterung (B3):** Fenster von `W = 4000` Samples (250 ms), beginnend
bei Sample 0, nicht überlappend, Schrittweite W. Das letzte Fenster darf
kürzer sein (Restfenster) und geht mit seiner echten Sample-Zahl in
`max` und in Laufdauern ein. In die **floor**-Schätzung gehen nur volle
Fenster ein; gibt es kein volles Fenster (Aufnahme < 250 ms), greift
ohnehin Regel A. Laufdauern werden in **Samples** gezählt und erst zur
Ausgabe in Sekunden umgerechnet.

**floor (B3, W1):** `sorted` = aufsteigend sortierte RMS der vollen
Fenster, `n = len(sorted)`. `floor = sorted[⌊0,1 · (n − 1)⌋]`
(Nearest-Rank nach unten, ohne Interpolation). Für `n ≤ 10` ist das
das Minimum. Bewusst einfach: ein Ordnungswert, reproduzierbar in Python
und Rust mit identischem Ergebnis.

**Konstanten (Startwerte, Nachkalibrierung durch WP0):**

| Name | Wert | Bedeutung |
|---|---|---|
| `MIN_SAMPLES_16KHZ` | 4000 | Regel A (unverändert, §6.4) |
| `RMS_SILENCE_THRESHOLD` | 0,0075 | Regel B1/B2 (unverändert) |
| `MIN_SPEECH_RUN_ABS_SECS` | 2,0 | Regel B2 (unverändert) |
| `ABS_FLOOR` | 0,0003 | Regel C, −70,5 dBFS ≈ 9,8 LSB bei 16 bit |
| `RELATIVE_MARGIN_DB` | 12,0 | Regel D, Abstand über floor |
| `MIN_ACTIVE_RMS` | 0,0010 | Regel D, absolute Aktivitätsgrenze (B1), −60 dBFS ≈ 33 LSB |
| `MIN_SPEECH_RUN_REL_SECS` | 1,5 | Regel D, Laufdauer |

**Regeln — die erste zutreffende entscheidet (K1):**

- **A — zu kurz:** `len < MIN_SAMPLES_16KHZ` → leer, `TooShort`.
- **B1 — sicher laut:** Gesamt-RMS ≥ 0,0075 → Engine (heutiges
  Verhalten; kein Sprachbeweis, sondern Erhalt der bisherigen
  Ja-Entscheidungen und schneller Pfad).
- **B2 — absoluter Lauf:** zusammenhängender Lauf von Fenstern mit
  RMS ≥ 0,0075 über ≥ 2,0 s → Engine (heutiges Verhalten, F2).
- **C — unter absoluter Untergrenze (W2):** `max_window_rms < ABS_FLOOR`
  → leer, `BelowAbsoluteFloor { rms, max_window_rms }`. Das ist eine
  Pegelgrenze, **kein** Gerätebefund; Geräte-Recovery bleibt bei §10.
  Sie ist die dokumentierte Ausnahme von „pegelunabhängig“.
- **D — relativ:** `thr_D = max(floor · 10^(12/20), MIN_ACTIVE_RMS)`.
  Fenster mit RMS ≥ `thr_D` sind „aktiv“. Fenster mit RMS < `ABS_FLOOR`
  sind nie aktiv und **unterbrechen** einen Lauf (B1: Nullen dürfen
  Läufe weder verlängern noch verbinden — durch `MIN_ACTIVE_RMS` ohnehin
  ausgeschlossen, hier zusätzlich explizit). Längster zusammenhängender
  aktiver Lauf ≥ 1,5 s → Engine. Sonst leer,
  `NoRelativeRun { rms, floor, max_window_rms, threshold, longest_run_secs }`.

Kein Fall bleibt unentschieden: A/B1/B2/C sind frühe Ausgänge, D hat
einen expliziten Nein-Zweig. B1 und C schließen sich aus, weil
`max_window_rms ≥ rms`.

**Erwartung gegen die Befunde:** `stille.wav` → D leer (kein Fenster
erreicht 0,0040), `rauschen.wav` → D leer (Lauf 1,0 s), Sprach-WAVs → B1,
dieselben um 16/22 dB abgesenkt → D Engine (nach v1-Messung; WP0 misst
mit der normativen Definition nach), Astra-B1-Beispiel (10 s Null +
250 ms bei 0,001) → D leer (thr_D = max(0, 0,001) = 0,001, ein Fenster
= 0,25 s < 1,5 s), Astra-B2-Beispiel → B2 Engine.

### Gate-Ergebnis und Log (W4, K2)

`transcribe_pcm` liefert künftig `(Transcription, GateReport)`.
`GateReport { decision: Speech(SpeechRule::B1|B2|D) | Rejected(SilenceGate),
samples, full_windows, rms, floor, max_window_rms, threshold_d,
longest_abs_run_secs, longest_rel_run_secs }` mit `Display`. Der Gate
wird pro Aufnahme **einmal** gerechnet; Daemon und CLI loggen den Report,
nicht ein zweites Gate-Ergebnis. Der Daemon loggt den Report für **jede**
Aufnahme (auch akzeptierte) in einer INFO-Zeile, Präzision `{:.5}` für
RMS-Werte — das ist die Datenbasis für Nachkalibrierung im Betrieb. §10:
nur Zahlen, kein Audio, kein Text, kein Fenstertitel. Die Zeile steht im
Lauf-Kontext („Lauf N: Gate …“), damit Capture-Zeile (Gerät, Rate) und
Gate korrelierbar sind.

`SilenceGate`-Varianten nach Umbau: `TooShort`, `BelowAbsoluteFloor`,
`NoRelativeRun`, `InvalidInput`. Die heutigen `BelowThreshold`/`NoLoudRun`
entfallen (ihre Semantik ist in D aufgegangen); die 0.2.2-Tests werden
auf die neuen Varianten umgestellt, ihre Verhaltensabsicht bleibt.

### Nicht Teil des Plans

- **Windows-Mikrofonpegel automatisch anheben** (Nutzerfrage
  2026-09-21): verworfen — Systemeinstellung, die Teams und alle anderen
  Programme mitbetrifft, ohne dass der Nutzer es sieht; nicht nötig, da
  die Engine pegelrobust ist.
- **Digitale Normalisierung vor der Engine:** nicht nötig, hebt Rauschen
  mit an.
- **Komplexere floor-Schätzer** (Median der leisesten k, gleitendes
  Minimum): erst, wenn WP0 zeigt, dass der Ordnungswert nicht reicht (W1).
- **Overlay-Hinweis „zu leise“:** Folgepaket WP5, eigener Nachtrag in
  `overlay-plan.md`; kein verdecktes Kriterium für WP1–4 (W5).

## Arbeitspakete

Reihenfolge: WP4 (Spec) → WP1–WP3 (Code, ein Opus-Paket) parallel zu
WP0 (Aufnahmen durch Ralf) → Kalibrierung/Abnahme → Release.
WP0 blockiert nicht den Code, aber die **Freigabe** (B4, W5).

### 🔍 WP0 — Kalibrierungsmatrix (Ralf + Analyse-CLI)

Analyse-CLI (Teil von WP3): `diktier --gate-analyze <wav>…` gibt je
Datei den `GateReport` als Tabelle auf stdout aus, plus Fensteranzahl
und die Werte für alternative Margen (+10/+15 dB) und Laufdauern
(1,0/2,0 s), damit die Konstanten ohne Rebuild bewertet werden können.

Aufnahmen (16 kHz mono über den Windows-Capture-Pfad, `--record-test`
oder Daemon mit `DIKTIER_DEBUG_WAV=1`, je 8–12 s, Gerät und
Windows-Audioeffekte-Zustand notieren), lokal unter `testdata/stt/local/`
(gitignored):

| Klasse | Aufnahme | Erwartung Gate | Erwartung Engine |
|---|---|---|---|
| Positiv leise | Jabra, normal sprechen, Windows-Pegel auf ~20 % | Engine (D) | Referenztext |
| Positiv leise, mit Vorlauf | 3 s Stille, dann leise Sprache | Engine (D) | Referenztext |
| Positiv leise, sofort | Sprache ab Sample 0, keine Pause | Engine (D) oder dokumentierte Grenze (F1) | — |
| Positiv kurz leise | ein Wort, ~0,8 s | dokumentierte Grenze (F1) | — |
| Negativ Tippen | 10 s tippen, kein Wort | leer **oder** Engine mit leerem Text | leer |
| Negativ Lüfter-Anlauf | ruhig, dann Lüfter hoch | leer oder Engine leer | leer |
| Negativ Atmen/Wind | Atmen ins Headset-Mikro | leer oder Engine leer | leer |
| Negativ Stuhl/Kabel | Stuhlrollen, Kabelreiben | leer oder Engine leer | leer |
| Negativ Stille | stiller Raum, 10 s, fünfmal | leer (D) | — |
| Zweites Mikro | Laptop-Mikro, leise Sprache + Stille | wie oben | wie oben |

Für jede Datei festhalten: Gate-Entscheidung und Regel, Engine
aufgerufen ja/nein, Zeichenzahl des Ergebnisses, Pass/Fail gegen die
Erwartung. Ergebnis als datierter Block in `docs/SPIKES.md`
(„Kalibrierung relativer Gate 2026-09“), der historische Phase-2-Block
bleibt unverändert (W5). Kippt ein Negativfall (Engine liefert Text),
zuerst Marge +15 dB oder Lauf 2,0 s aus der Analyse-Tabelle bewerten,
dann Positivfälle **alle** erneut prüfen (B4).

### 🔍 WP1 — Synthetische Fixtures

- `testdata/stt/attenuate.py` (eigenes Skript, K3; `normalize.py` bleibt
  Text/WER): liest 16-bit-WAV, multipliziert mit `10^(−dB/20)`, rundet
  kaufmännisch zur nächsten Ganzzahl, sättigt auf i16, schreibt 16 bit.
  Deterministisch, kein Dithering. README dokumentiert Faktor, Rundung
  und SHA-256 der Ausgaben.
- Erzeugen: `alltag_-16db.wav`, `alltag_-22db.wav`,
  `fachwoerter_-16db.wav`, `zahlen_umlaute_-16db.wav` (W3: nicht nur eine
  Stimme/Äußerung). Ref-Texte sind die der Originale.
- `testdata/stt/README.md`: Herkunftstabelle, Gültigkeitsbereich der
  synthetischen Ableitungen (skaliert Rauschen mit), Verweis auf WP0 für
  echte Aufnahmen; WER-Definition an §12/§18 angleichen (+0,05-Puffer
  nennen, K3). `testdata/stt/local/` in `.gitignore`.

### 🔍 WP2 — Gate umbauen (`src/engine.rs`)

- Konstanten wie in der Tabelle; Rustdoc mit der normativen Definition
  und den Messwerten aus diesem Plan.
- Hilfsfunktionen: `window_rms(pcm) -> Vec<Window { rms, samples }>`
  (einmal berechnen, Sample-Zahl je Fenster erhalten — B3),
  `noise_floor(&[Window]) -> Option<f32>` (nur volle Fenster,
  Nearest-Rank), `longest_run_samples(&[Window], thr, break_below)`.
- `silence_gate(pcm) -> GateReport`; `is_silence_or_short` bleibt
  Wrapper (`report.decision.is_rejected()`); `transcribe_pcm` gibt
  `(Transcription, GateReport)` zurück (K2).
- **Unit-Tests ohne Modell** (W3, B1–B3), alle mit `CountingStub`, der
  auch den erfolgten Aufruf in Positivfällen belegt:
  - A: 3.999 Samples → `TooShort`; 4.000 Samples laut → B1.
  - B1 exakt unter/an/über 0,0075, auch bei 1 Fenster und bei
    gleichmäßigem Rauschen auf 0,0080 (Engine wird gerufen — bewusst,
    heutiges Verhalten).
  - B2: 8 s bei 0,003 + 2 s bei 0,010 → Engine (Astra-Gegenbeispiel);
    dieselbe mit 1,75 s → nicht B2, dann D-Entscheidung.
  - C: max. Fenster knapp unter/an/über `ABS_FLOOR`; reine Null; Signal
    mit 1 LSB (0,0000305) Rauschen → C.
  - D/B1-Astra: 10 s Null + 250 ms bei 0,001 → leer; zwei Klicks mit
    Nullpause → leer (nicht verbunden); Null + 3 s leise Sprache
    (0,004) → Engine; dieselben mit 1 LSB Restpegel.
  - D exakt: floor 0,001, Fenster an 0,003981 (+12 dB) knapp
    darunter/darüber; Lauf 23.999 vs. 24.000 Samples; getrennte Läufe
    1,0 s + 1,0 s mit einem stillen Fenster dazwischen → leer.
  - Restfenster: 4 volle + 1 Restfenster von 1.000 Samples; floor aus
    den vollen; Lauf zählt die 1.000 Samples mit.
  - Perzentil: n = 1, 2, 8, 10, 11, 20 mit bekannter Sortierung →
    erwarteter Index.
  - `InvalidInput`: ein NaN-Sample → leer, Engine nicht gerufen.
  - Display jeder Variante enthält alle Messwerte mit `{:.5}`.
- **stt-smoke** (ignored, mit Modell) erweitern (W3): Fixtures um die
  vier abgesenkten Dateien; Sprachfälle prüfen `WER(normalisiert) ≤
  WER-Baseline + 0,05` gegen die Ref-Texte (Normalisierung wie
  `normalize.py`, in Rust nachgebaut oder Python per `std::process`
  aufgerufen — Entscheidung des Implementierers, dokumentieren);
  Stille/Rauschen prüfen `GateReport::Rejected` **und** dass der
  Engine-Aufruf nicht stattfand (Wrapper-Transcriber mit Zähler um
  `ParakeetTranscriber`).

### 🔍 WP3 — Aufrufer, CLI, Doku-Zeilen

- `src/daemon/workers.rs::engine_loop`: ein Gate-Aufruf über
  `transcribe_pcm`, Report-Zeile für jede Aufnahme (W4), keine
  doppelte Berechnung (K2).
- `src/main.rs::transcribe_wav` und `record_test`: Report auf stderr
  (beide Pfade, W4), Text weiter auf stdout.
- Neu `--gate-analyze <wav>…` (WP0-Werkzeug): Report je Datei plus
  Alternativspalten (+10/+15 dB, 1,0/2,0 s). Kein Modell nötig.
- README-Troubleshooting („Nichts wird erkannt“, 0.2.2-Eintrag) auf den
  relativen Gate umformulieren; Hinweis auf `--gate-analyze`.

### 🔍 WP4 — Spec-Nachtrag (SPEC v1.6) — **vor** WP1–3

- Kopfzeile „Spec v1.4“ → „Spec v1.6“, Versionshistorie um v1.6
  ergänzen (K3: v1.5 fehlt bislang im Titel).
- §6.4: Abschnitt „Silence-Gate“ mit der normativen Definition
  (Fensterung, floor, Konstanten, Regeln A–D, erste Regel entscheidet,
  Gate-Ablehnung = leeres Ergebnis ohne Engine-Aufruf), bekannte Grenzen
  (F1), Log-Vertrag (Report pro Aufnahme, nur Messwerte).
- §12 Phase 1: „Schwelle in docs/SPIKES.md“ → „Regeln in §6.4,
  Kalibrierung in docs/SPIKES.md (2026-08 absolut, 2026-09 relativ)“.
- §18: neue Zeile 12 „Relativer Gate: bisherige Ja-Pfade erhalten,
  D zusätzlich; Engine ist pegelrobust (−22 dB wortidentisch); Grenzen
  siehe §6.4“.

### 🔍 WP5 — Folgepaket: Overlay-Hinweis bei Gate-Treffer

Wenn der Gate ablehnt, zeigt das Overlay statt sofortigem Ausblenden
~1,5 s „nichts erkannt“ und blendet dann aus. Nur Präsentation, kein
Fokus, kein neuer `Effect`; Weg über `flush_presentation`. Eigener
Nachtrag in `overlay-plan.md`; nicht Voraussetzung für WP1–4.

## Gates (Abnahme)

1. `cargo test` grün (inkl. aller WP2-Falltests), Clippy ohne Meldung,
   `cargo fmt --check`.
2. `stt-smoke` mit Modell: fünf bisherige Dateien plus vier abgesenkte;
   Sprachfälle im WER-Puffer, `stille.wav`/`rauschen.wav` `Rejected` ohne
   Engine-Aufruf (Zähler).
3. WP0-Matrix vollständig mit Pass/Fail in SPIKES.md; alle Positivfälle
   „Engine (D)“ mit Referenztext, alle Negativfälle ohne Text. Kippt
   etwas, Konstanten nachziehen und Gate 1–3 wiederholen.
4. CLI-Capture (F4a): `--record-test 10` leise gesprochen → stderr zeigt
   Report mit Regel D, stdout den Text.
5. Daemon-End-to-End (F4b): `DIKTIER_DEBUG_WAV=1`, ein leises und ein
   normales Diktat ins Zielprogramm, manueller Vergleich Pass/Fail im
   Abnahmeprotokoll; Log zeigt je Lauf die Report-Zeile.
6. Fünf Stille-Versuche im Daemon ohne Text (heutiges Gate 4).

## Risiken

- **Strukturiertes Nicht-Sprachsignal** (Tippen über mehrere Fenster,
  Lüfter-Anlauf, Atmen) kann D erfüllen; dann ruft die Engine und muss
  leer liefern. WP0 misst genau das; Fallback-Konstanten stehen in der
  Analyse-Tabelle.
- **Kein Rauschfenster in der Aufnahme** (pausenlos, kurz): floor ist
  dann leise Sprache, D scheitert — dokumentierte Grenze (F1), heute
  identisch.
- **Absolute Untergrenzen** (`ABS_FLOOR`, `MIN_ACTIVE_RMS`): unter
  −60 dBFS wird auch Sprache verworfen — dokumentierte Ausnahme von
  „pegelunabhängig“. Die Jabra lag am 2026-09-21 mit 0,0117 im lauten
  Fenster 21 dB darüber.
- **Kalibrierung auf einer Stimme, einem Raum:** WP0 mit zweitem Mikro
  ist die einzige Verbreiterung; die Report-Zeile pro Lauf liefert im
  Betrieb die Daten zum Nachziehen.
