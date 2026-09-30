Review-Auftrag: Umsetzung SPEC v1.9 (Vorlauf-Stille vor der Engine, Debug-WAV als 32-bit-Float) in Diktier (Rust, Windows-only). Du bist ein delegierter Sub-Agent: nicht orchestrieren, nicht weiterdelegieren, nur dieses Review erledigen. Nur lesen und Befehle ausführen, keine Dateien außer deinem Bericht ändern.

Der Working Tree enthält zusätzlich den uncommitteten Stand 0.4.0 (Clipboard-Restore, WAV-Ring). Der ist NICHT Gegenstand des Reviews. Beschränke dich auf:

1. Vorgabe: docs/SPEC.md, Abschnitt „#### Vorlauf-Stille (v1.9)“ in §6.4, im §10 der Absatz `DIKTIER_DEBUG_WAV` und §18 #15. Kontext: docs/SPIKES.md, letzter Abschnitt. Auftrag an den Implementierer: docs/reviews/impl-lead-in-silence-prompt.md.
2. Code: `git diff src/engine.rs`. Dieser Diff gehört vollständig zum Paket. In src/daemon/debug_wav.rs nur `write_wav`, das Modul-Doc und die Tests `the_dump_is_float32_and_bit_exact` und `writes_the_wav_atomically`. Dazu `read_wav_16k_mono` in src/audio/mod.rs (Float-Zweig) und in src/daemon/workers.rs die Stelle, an der `dump_debug_wav` und `transcribe_pcm` aufgerufen werden.
3. Bericht des Implementierers: docs/reviews/impl-lead-in-silence-notes.md.

Prüffragen:
- Bekommt jeder Engine-Pfad (Daemon, `--transcribe-wav` inkl. Warmup, `--record-test`, stt-smoke) die Stille genau einmal? Umgeht irgendein Pfad `transcribe_pcm`?
- Rechnen Gate und Report garantiert auf dem unveränderten Puffer? Ist die Dauer korrekt, auch im Fehlerfall? Liest noch irgendetwas `Transcription.timing` und erwartet dort die Länge mit Stille (Watchdog, Logzeilen, Tests)?
- Ist die Debug-WAV bitgleich zu dem, was `transcribe_pcm` bekommt? Nimmt der Float-Zweig beim Lesen Werte außerhalb von [-1, 1] oder NaN korrekt an bzw. lehnt sie korrekt ab (Konsistenz zwischen Schreiben und Lesen)?
- Decken die Tests die Spec ab? Welche Fehlerbilder würden die Tests nicht fangen?
- Weicht etwas von SPEC v1.9 ab?

Führe selbst aus: `cargo test engine::tests` und `cargo test daemon::debug_wav`. Zitiere die Summary-Zeilen. Das Modell-Gate `stt_smoke_fixtures` NICHT ausführen, kein `cargo build --release`, nichts in `%TEMP%\diktier` anfassen, keine installierte Version und keinen laufenden Daemon anfassen.

Bericht nach docs/reviews/impl-lead-in-silence-sol.md: Befunde nach Schwere (kritisch / wichtig / Kleinigkeit), je mit Datei:Zeile, konkretem Fehlerszenario und Vorschlag. Getrennt: statisch geprüft, selbst ausgeführte Tests, übernommene Aussagen aus dem Bericht. Ist nichts zu beanstanden, das ausdrücklich sagen. TUI-Ausgabe knapp.
