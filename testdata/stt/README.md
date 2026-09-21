# STT-Testdaten (Spec §12 Phase 1)

Selbst gesprochen, lizenzfrei. **16 kHz, mono, PCM** (16-bit Integer oder 32-bit Float).
Kein Resample in Phase 1 — andere Raten oder Stereo sind ein Fehler.

## Dateien

| Datei | Inhalt |
|---|---|
| `alltag.wav` | deutsche Alltagssprache |
| `alltag.ref.txt` | wortgetreuer Referenztext |
| `fachwoerter.wav` | Fachwörter |
| `fachwoerter.ref.txt` | wortgetreuer Referenztext |
| `zahlen_umlaute.wav` | Zahlen und Umlaute |
| `zahlen_umlaute.ref.txt` | wortgetreuer Referenztext (Zahlenschreibweise wie Parakeet: Ziffern vs. Wort) |
| `stille.wav` | echte Stille, erwartet leer |
| `rauschen.wav` | Raumrauschen, erwartet leer |

Die WAVs und Referenztexte kommen vom User; dieses Verzeichnis ist nur das Gerüst.

## Aufnahme

Am Mikrofon in 16 kHz mono aufnehmen (z. B. Audacity-Export).
Keine Nachbearbeitung außer Zuschneiden. Referenztexte in UTF-8, eine Zeile oder umbrochen — die Normalisierung kollabiert Whitespace.

## Synthetisch abgesenkte Ableitungen (Silence-Gate, §6.4)

Für den relativen Silence-Gate (`docs/silence-gate-plan.md`, WP1) liegen
digital abgesenkte Kopien der Sprachdateien bei. Sie belegen, dass die Engine
pegelrobust ist: Parakeet erkennt sie wortidentisch zum Original, obwohl der
absolute Gate von vor v1.6 sie verworfen hätte.

Erzeugt mit `attenuate.py`: jedes Sample × `10^(−dB/20)`, **kaufmännisch**
gerundet (halbe Werte vom Nullpunkt weg, nicht Pythons `round()`), auf i16
gesättigt, wieder als 16-bit-PCM geschrieben. Kein Dithering, keine Filter,
keine Normalisierung — dieselbe Eingabe ergibt byteweise dieselbe Ausgabe.
Referenztext ist der des Originals.

```bash
python3 testdata/stt/attenuate.py --selftest
python3 testdata/stt/attenuate.py testdata/stt/alltag.wav 16
```

| Datei | Quelle | dB | Faktor | SHA-256 |
|---|---|---|---|---|
| `alltag_-16db.wav` | `alltag.wav` | −16 | 0,15848931924611134 | `70a839046c2c494fe028cbd290a46c58ce1090201c557861bbc084a1c2fdba14` |
| `alltag_-22db.wav` | `alltag.wav` | −22 | 0,079432823472428138 | `faa650c2d6f1ae06bf94292880cfaf16f8d13b3dd504d459f9ca1fc2124cc3b3` |
| `fachwoerter_-16db.wav` | `fachwoerter.wav` | −16 | 0,15848931924611134 | `9d57aee5f74aac6ad7fd00ea4f06da38c0f9e4505d3ce34e0f86714719829292` |
| `zahlen_umlaute_-16db.wav` | `zahlen_umlaute.wav` | −16 | 0,15848931924611134 | `cab750b2d0d19e6e0afaaac11dc9fd811890117b1f21ad67f09c9ac07902377b` |

Quell-SHA-256 (damit die Ableitung nachvollziehbar bleibt):
`alltag.wav` `8f48f2e4b4974eeb19da7b845eb2e75632750bc3a7dae6ec13870deeef6d3f98`,
`fachwoerter.wav` `91893efe998c249f95363b151f7f268b2187e399166746f365adc81f15088872`,
`zahlen_umlaute.wav` `8d556ad61776d916cfe79d90c5794bf75806e857ef7dc567bab4ea72cb40bab8`.

**Gültigkeitsbereich:** Die Absenkung skaliert Sprache **und** Grundrauschen
gemeinsam; der Sprach-Rausch-Abstand bleibt also erhalten. Der reale Fall
„Sprache wird leiser, das Grundrauschen bleibt“ ist damit nur angenähert
(Astra W3). Die erneute 16-bit-Quantisierung kann einzelne Fenster-RMS
verschieben — bei −22 dB liegt der Gesamt-RMS nur noch wenige dB über
`stille.wav`. Diese Dateien belegen deshalb die Pegelrobustheit der Engine und
die Ja-Entscheidung des Gates, **nicht** sein Verhalten gegen echtes
Nicht-Sprachsignal. Dafür sind die echten Aufnahmen aus WP0 zuständig.

## Echte Kalibrierungsaufnahmen

`testdata/stt/local/` ist in `.gitignore` und enthält die realen Aufnahmen der
Kalibrierungsmatrix (leise Sprache, Tippen, Lüfter, Atmen, Stuhlrollen,
zweites Mikrofon). Sie bleiben lokal — Privatsphäre und Repo-Größe. Kennzahlen
und Pass/Fail stehen in `docs/SPIKES.md`; auswerten mit

```powershell
diktier.exe --gate-analyze testdata\stt\local\*.wav
```

## Vergleich

```bash
python3 testdata/stt/normalize.py --selftest
python3 testdata/stt/normalize.py testdata/stt/alltag.ref.txt
python3 testdata/stt/normalize.py WER testdata/stt/alltag.ref.txt /tmp/diktier-out.txt
```

Normalisierung: Kleinbuchstaben; `-`/`–`/`—` zu Leerzeichen; übrige Interpunktion
(`[.,!?;:"'` plus typografische Anführungszeichen) entfernen; Whitespace kollabieren.
`WER = Wort-Levenshtein(Referenz, Hypothese) / |Referenz|` nach dieser
Normalisierung. Dieselben Regeln sind für den `stt_smoke_fixtures`-Test in Rust
nachgebaut (`src/engine.rs`), damit der Test ohne Python läuft.

Gate (Spec §12, Puffer nach §18 #11): nach Normalisierung Diktier = Voxtype,
oder `WER(Diktier, Referenz) ≤ WER(Voxtype, Referenz) + 0,05` — der Puffer gilt
für den Vergleich mit Voxtype, nicht als Zuschlag auf einen ohnehin
schlechteren Diktier-Wert. Für die abgesenkten Ableitungen gilt derselbe Puffer
gegen **dieselbe Datei unabgesenkt**: `WER(abgesenkt) ≤ WER(Original) + 0,05`.
