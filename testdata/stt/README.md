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

Am Mikrofon in 16 kHz mono aufnehmen (z. B. `arecord -r 16000 -c 1 -f S16_LE` oder Audacity Export).
Keine Nachbearbeitung außer Zuschneiden. Referenztexte in UTF-8, eine Zeile oder umbrochen — die Normalisierung kollabiert Whitespace.

## Vergleich

```bash
python3 testdata/stt/normalize.py --selftest
python3 testdata/stt/normalize.py testdata/stt/alltag.ref.txt
python3 testdata/stt/normalize.py WER testdata/stt/alltag.ref.txt /tmp/diktier-out.txt
```

Normalisierung: Kleinbuchstaben; `-`/`–`/`—` zu Leerzeichen; übrige Interpunktion
(`[.,!?;:"'` plus typografische Anführungszeichen) entfernen; Whitespace kollabieren.
Gate: nach Normalisierung Diktier = Voxtype, oder `WER(Diktier, Referenz) ≤ WER(Voxtype, Referenz)`.
