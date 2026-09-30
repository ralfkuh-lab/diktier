Auftrag: Spike „Parakeet Ultra gegen Parakeet v3“. Du misst und berichtest; am Produkt änderst du nichts. Diktier, Rust, Windows-only, Stand Commit 2d10ee0 (0.4.1).

Hintergrund: Moondream hat am 2026-09-22 „Parakeet Ultra“ veröffentlicht, ein nachtrainiertes parakeet-tdt-0.6b-v3 mit gleicher Architektur und gleichem Tokenizer. `parakeet-rs` hat es am 2026-09-24 als ONNX aufgenommen (PR #132, laut PR nur Modelldateien tauschen, `ParakeetTDT::from_pretrained(dir, None)`). Diktier nutzt produktiv `parakeet-tdt-0.6b-v3-int8`. Offene Fragen: Ist Ultra auf Ralfs deutschen Diktaten besser? Tritt „Herr Präsident“ seltener auf? Verschwindet das vorangestellte „Ich“ bei Befehlen (Lauf 547)? Was kostet es an Latenz und RAM?

Lies zuerst:
1. docs/SPIKES.md, die Abschnitte „Vorlauf-Stille gegen ‚Herr Präsident‘“ und „Kalibrierung relativer Silence-Gate“.
2. docs/SPEC.md §6.2, §6.3 und §6.4 (Vorlauf-Stille), src/engine.rs (`ParakeetTranscriber::load`, `ensure_ort_initialized`, `transcribe_pcm`), src/audio/mod.rs (`read_wav_16k_mono`), Cargo.toml (Pins `parakeet-rs =0.3.7`, `ort =2.0.0-rc.13`, `load-dynamic`).
3. testdata/stt/README.md, testdata/stt/normalize.py, testdata/stt/local/herr_praesident/FAELLE.md.

Modelle (Dateien und SHA-256 laut Hugging Face `altunenes/parakeet-rs`; LFS-oid = SHA-256, nach dem Download prüfen):
- **v3-int8 (Produktion):** `%LOCALAPPDATA%\diktier\models\parakeet-tdt-0.6b-v3-int8\` nur lesen, nichts dort ändern.
- **v3-fp32:** `tdt/encoder-model.onnx`, `tdt/encoder-model.onnx.data`, `tdt/decoder_joint-model.onnx`, `tdt/vocab.txt` (ggf. `tdt/nemo128.onnx`, falls parakeet-rs es braucht) → `D:\DEV\diktier\models\spike\v3-fp32\`
- **Ultra-fp32:** `parakeet-ultra/encoder-model.onnx`, `parakeet-ultra/encoder-model.onnx.data`, `parakeet-ultra/decoder_joint-model.onnx`, `parakeet-ultra/vocab.txt` → `D:\DEV\diktier\models\spike\ultra-fp32\`
- **Ultra-int8 (optional, nur wenn machbar):** Encoder und Decoder aus Ultra-fp32 selbst dynamisch nach int8 quantisieren, möglichst so, wie der v3-int8-Export entstanden ist (die Methode steht vielleicht in der README auf HF; falls nicht, `onnxruntime.quantization.quantize_dynamic` mit per-channel und QInt8 und das im Bericht so benennen). Python-Pakete nur isoliert, z. B. `uv run --with onnxruntime --with onnx python …`, keine globale Installation. → `D:\DEV\diktier\models\spike\ultra-int8\`
Download-URL-Form: `https://huggingface.co/altunenes/parakeet-rs/resolve/main/<pfad>`. `/models/` ist gitignoriert.

Messwerkzeug: eigenständige Rust-Crate unter `D:\DEV\diktier\.herd\spike-ultra\` (lokal git-ausgeschlossen) mit denselben Pins wie Diktier (`parakeet-rs =0.3.7`, gleiche Features, `ort =2.0.0-rc.13` load-dynamic). Die ONNX-Runtime-DLL wie Diktier laden (siehe `ensure_ort_initialized`; eine vorhandene DLL liegt z. B. in `%LOCALAPPDATA%\Programs\Diktier\`). Eingabe: Modellverzeichnis und eine Liste von WAVs. Je WAV: 16-bit → f32 wie `read_wav_16k_mono`, 4800 Nullen voranstellen wie `transcribe_pcm`, transkribieren, Text und Inferenzzeit ausgeben. Modell einmal laden, einen Warmup-Lauf ungezählt, gleiche Thread-Zahl für alle Modelle (Default von parakeet-rs, wie `engine.threads = 0`).

Messmatrix, jede Kombination Modell × Datei:
- Fixtures mit Referenz: `testdata/stt/{alltag,alltag_-16db,alltag_-22db,fachwoerter,fachwoerter_-16db,zahlen_umlaute,zahlen_umlaute_-16db}.wav` → WER mit `normalize.py` (Funktion `wer`).
- Kalibrierung: `testdata/stt/local/0{5,6,7,9}_*.wav` → WER gegen `alltag.ref.txt`; `00_normal_jabra.wav` nur Text.
- Ring: alle WAVs in `testdata/stt/local/herr_praesident/`. Zählen, wie oft der Text mit „Herr Präsident“ beginnt (Lauf 527 enthält es wirklich gesprochen, dort nicht zählen). Für Lauf 545 zusätzlich 24 Varianten mit ±1 LSB Zufallsrauschen (Python `random.Random(seed)`, seed 0–23, je Sample `+choice((-1,0,1))`, auf i16 sättigen). Lauf 547: Beginnt der Text mit „Schau“ oder mit „Ich schaue“? Ebenfalls mit den 24 Dither-Seeds.
- Leistung je Modell: Ladezeit, mittlere und maximale Inferenzzeit sowie Echtzeitfaktor über alle Fixtures, maximaler Working Set des Prozesses.

Bericht nach docs/reviews/spike-parakeet-ultra-notes.md:
- Tabellen: WER je Fixture und Modell mit Summe, „Herr Präsident“-Rate je Modell (Ring und 545 mit 24 Seeds), „Ich“-Rate bei 547, Leistung.
- Auffällige Textunterschiede wörtlich.
- Wie Ultra-int8 entstanden ist, oder warum es fehlt.
- Klare Einschätzung mit Belegen, ob sich ein Wechsel lohnt und mit welcher Variante. Was ein Wechsel im Produkt bedeuten würde (Manifest, Artefakte, Download-Größe, `check_artifacts`, SPEC §6.2/§6.3), ohne es umzusetzen.
- Rohdaten als TSV unter `.herd/spike-ultra/results.tsv` und im Bericht verlinken.

Regeln:
- Keine Änderungen an src/, Cargo.toml, Cargo.lock, docs/ (außer deinem Bericht), testdata/, scripts/. Nicht committen.
- Keine installierte Diktier-Version und keinen laufenden Daemon anfassen, nichts in `%LOCALAPPDATA%\diktier` oder `%TEMP%\diktier` schreiben.
- Kein herdr, keine weiteren Panes.
- Scheitert ein Download oder passt ein SHA-256 nicht, abbrechen und berichten, nicht umgehen.
- Brauchst du eine Entscheidung, schreib die Frage nach `D:\DEV\diktier\.herd\fragen\spike-ultra.md` und beende den Turn mit der Zeile `RUECKFRAGE: D:\DEV\diktier\.herd\fragen\spike-ultra.md`.

TUI-Ausgabe knapp; das Ergebnis zählt nur aus dem Bericht.
