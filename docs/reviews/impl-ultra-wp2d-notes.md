# WP2d — Lokales Ultra-Integrationsgate (2026-09-30, Orchestrator)

Stand: Working Tree 0.5.0 (WP0–WP2c, uncommittet), Binary `target-dev\release\diktier.exe`, gebündelte ORT 1.28.0, Produktionsthreads (Default).
Modellwurzel über `LOCALAPPDATA` nur für den Kindprozess: `.herd\wp2c-root\` mit einer Kopie des installierten v3-Verzeichnisses und den WP0-Artefakten (`COMPLETE` wie `download::write_marker`).

## ✅ Batch mit beiden Modellen

- `--transcribe-list` mit 23 Dateien: 9 Fixtures (einschließlich `stille.wav` und `rauschen.wav`), 12 gesicherte Aufnahmen aus `testdata\stt\local\herr_praesident\`, 2 f32-Aufnahmen aus dem 0.4.1-Ring.
- Aufruf je Modell mit `--model`. Beide Exit 0, JSONL in derselben Dateireihenfolge.
- `stille.wav` und `rauschen.wav` sind bei beiden Modellen `rejected`; alle Sprachdateien liefern `text`.
- f32-WAVs aus dem 0.4.1-Ring: bei beiden Modellen `text`.
- „Herr Präsident“ am Anfang: 0 bei v3, 0 bei Ultra (mit 300 ms Vorlauf).
- Fixtures: Ultra schreibt „ONNX“ und „23. März … 250 Euro“ wie im Spike; die Zahlenschreibweise ist getrennt zu bewerten (Kriterium 6).
- `infer_ms`, nur zur Plausibilität, nicht als Kriterium-4-Messung: Ultra ist bei 17 von 21 Sprachdateien gleich schnell oder schneller. Der Daemon lief parallel.

Beobachtung beim eigenen Aufruf: Wer `diktier.exe` mit umgeleitetem stdout und stderr startet, muss beide parallel lesen. Mein erster Versuch las nacheinander und hing am vollen stderr-Puffer. Die Skripte aus WP2c lesen korrekt; das war ein Fehler in meinem Ad-hoc-Aufruf, nicht im Produkt.

## ✅ Rückweg lokal

- Ultra-Verzeichnis mit einem um 1000 Byte gekürzten Encoder: `--transcribe-wav --model parakeet-ultra-0.6b-int8-pc` → Exit 1, Meldung „Modellartefakt …\encoder-model.int8.onnx hat 700506227 Bytes, erwartet 700507227“. Kein Laden, kein Netz.
- Dieselbe Wurzel mit `--model parakeet-tdt-0.6b-v3-int8` → Exit 0, normales Transkript, ohne Netz.
- Den Daemon-Rückweg (Config umstellen, Neustart) prüft WP3b Schritt 6 live.

## Gates

`cargo fmt --check` (0), `cargo clippy --all-targets -- -D warnings` (ohne Meldung), `cargo test`: `test result: ok. 550 passed; 0 failed; 10 ignored`. stt-smoke (v3) laut WP2a grün, seitdem keine Änderung an Engine und Modellpfad. `release.ps1 -SkipInstaller` mit Bundle-Gate und Ultra-NOTICE laut WP2c grün.
