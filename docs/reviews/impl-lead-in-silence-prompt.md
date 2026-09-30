Auftrag: SPEC v1.9 umsetzen, also Vorlauf-Stille vor jedem Engine-Aufruf (§6.4) und Debug-WAV als 32-bit-Float (§10). Diktier, Rust, Windows-only. Basis ist der aktuelle Working Tree. Er enthält den uncommitteten Stand 0.4.0 (Clipboard-Restore, WAV-Ring). Der ist Vorgabe, nicht Arbeitsgegenstand.

Lies zuerst:
1. docs/SPEC.md: Kopf „v1.9“, §6.4 „Vorlauf-Stille (v1.9)“, §10 Absatz `DIKTIER_DEBUG_WAV`, §18 #15. Die SPEC ist verbindlich.
2. docs/SPIKES.md, letzter Abschnitt „Vorlauf-Stille gegen ‚Herr Präsident‘“ (warum 300 ms, warum f32).
3. src/engine.rs (`transcribe_pcm`, `Transcriber`, `ParakeetTranscriber::transcribe`, die Tests um Zeile 700–790 und `stt_smoke_fixtures` ab ~1379), src/daemon/debug_wav.rs (`write_wav`, `to_i16`, Tests), src/audio/mod.rs (`read_wav_16k_mono` liest Float-32 bereits), src/daemon/workers.rs (`dump_debug_wav`, Aufruf von `transcribe_pcm`), src/main.rs (`transcribe_wav`, `--record-test`).

Umfang:
- engine.rs: Konstante `LEAD_IN_SILENCE_SAMPLES: usize = 4800` mit Doc-Kommentar (Verweis SPEC §6.4 v1.9). `transcribe_pcm` ruft den Gate wie bisher auf dem unveränderten Puffer. Nur bei Freigabe baut es einen neuen Puffer (4800 × `0.0`, dann die Samples) und übergibt ihn an `engine.transcribe`. Der Gate-Report bleibt unverändert. `Transcription.timing.duration` bezieht sich auf die Originallänge ohne Stille. Wenn das im `ParakeetTranscriber` berechnet wird, dort korrigieren, und zwar so, dass keine zweite Stelle die Stille kennen muss (z. B. Dauer in `transcribe_pcm` setzen). Bei Ablehnung bleibt alles wie bisher: kein Engine-Aufruf, keine Allokation.
- Tests in engine.rs ohne Modell: Ein Recording-Stub merkt sich den übergebenen Puffer. Dann prüfen: Länge = Original + 4800, die ersten 4800 Samples sind exakt `0.0`, danach folgt das Original bitgleich, der Report ist identisch zum Aufruf ohne Stille, die Dauer entspricht der Originallänge, bei Ablehnung wird der Stub nicht aufgerufen. Bestehende Tests anpassen, wo sie die Pufferlänge oder die Dauer prüfen.
- debug_wav.rs: `write_wav` schreibt 16 kHz mono, `bits_per_sample: 32`, `SampleFormat::Float`, Samples unverändert (kein Clamp, keine Rundung). `to_i16` entfällt, wenn nichts anderes es nutzt. Test: Roundtrip über `audio::read_wav_16k_mono` ist bitgleich (inklusive Werten wie `1e-6`, `-0.5`, `0.99999`). Die bestehenden Ring-Tests bleiben grün. Die aufgezeichneten Samples sind die Capture-Samples **vor** der Stille; das ergibt sich, weil der Dump nicht über `transcribe_pcm` läuft. Das im Bericht bestätigen.
- README.md: Im Abschnitt Debug-WAV das Format (32-bit-Float, etwa doppelte Dateigröße) nennen. Im Troubleshooting kurz „Herr Präsident“ am Anfang: seit 0.4.1 durch 300 ms Vorlauf-Stille behoben; ein vorangestelltes „Ich“ bei Befehlen bleibt eine bekannte Grenze. Versionszeile 0.4.1.
- Cargo.toml Version 0.4.1 (Cargo.lock zieht `cargo build` nach). docs/windows-plan.md: WP6-Absatz um einen Satz zu 0.4.1, analog zu den bisherigen Versionsnotizen.

Gates (Befehl und tatsächliche Summary-Zeile zitieren):
1. `cargo fmt --check`
2. `cargo clippy --all-targets -- -D warnings` ohne Meldung
3. `cargo test` grün
4. `cargo test -- --ignored stt_smoke_fixtures` grün; die WER je Fixture aus der Testausgabe in den Bericht übernehmen
5. `$env:CARGO_TARGET_DIR="target-dev"; cargo build --release`. Der Daemon läuft aus `%LOCALAPPDATA%\Programs\Diktier`, `target` und `target-dev` sind frei, `target-dev` vermeidet Kollisionen mit `scripts\release.ps1`. Danach `target-dev\release\diktier.exe --version` (zeigt 0.4.1) und `target-dev\release\diktier.exe --transcribe-wav testdata\stt\local\herr_praesident\lauf-523_2026-09-30T08-59-48Z_herr-praesident.wav`. Der Text darf nicht mit „Herr Präsident“ beginnen, Ausgabe wörtlich in den Bericht.

Regeln:
- Nicht committen.
- Keine Änderungen an docs/SPEC.md, docs/SPIKES.md, docs/reviews/* (außer deinem Bericht), src/inject/*, src/overlay*, src/state.rs, testdata/*. Stößt du dort auf einen Fehler, beschreib ihn im Bericht.
- Keine Config-Option für die Stille, keine Änderung am Silence-Gate oder seinen Konstanten.
- Keine installierte Diktier-Version anfassen, keinen laufenden Daemon beenden, kein `scripts\release.ps1`, nichts in `%TEMP%\diktier` schreiben oder löschen (Tests arbeiten in temporären Verzeichnissen).
- Kein herdr, keine weiteren Panes.
- Brauchst du eine Entscheidung, schreib die Frage nach `D:\DEV\diktier\.herd\fragen\impl-leadin.md` und beende den Turn mit der Zeile `RUECKFRAGE: D:\DEV\diktier\.herd\fragen\impl-leadin.md`.

Bericht nach docs/reviews/impl-lead-in-silence-notes.md: Umgesetzt, Abweichungen und warum, Gate-Ausgaben wörtlich, Offen. TUI-Ausgabe knapp; das Ergebnis zählt nur aus Bericht und `git diff`.
