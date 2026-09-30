Nachreview: Die Befunde aus deinem Review docs/reviews/impl-ultra-wp2-sol.md sind in WP2e bearbeitet. Du bist weiterhin delegierter Sub-Agent: nicht orchestrieren, nicht weiterdelegieren, nur lesen, Befehle ausführen und den Bericht schreiben.

Lies:
1. docs/reviews/impl-ultra-wp2e-notes.md: was je Befund umgesetzt ist und welche Abweichungen es gibt.
2. docs/ultra-alltagstest-plan.md, Abschnitte „Bewertungsprotokoll“, „Abnahmekriterien“ 2 und 3 sowie „Datenschutz/Urteile speichern“. Sie wurden nach deinem Review als v2.1 präzisiert: Urteil für beide Seiten, „gleicher Fall wie“, „Ich“-Regel, verbindlicher Lauf, Speichern per File System Access API ohne Download-Ersatz.
3. SPEC §9: neu `--manifest-sha256` und die Regel zu modusabhängigen Optionen.

Prüfe je Befund K1, W1–W9, L1–L3 und die Blindheit an den jetzigen Codestellen, ob er behoben ist. Ist er es nicht oder nur teilweise, beschreibe das konkrete Restszenario. Prüfe außerdem, ob die Änderungen neue Fehler einführen, besonders in scripts/compare-models.ps1, scripts/compare-models.html, scripts/bench-models.ps1, scripts/ultra-test-lib.ps1, scripts/release.ps1, scripts/quantize-ultra.py, src/transcribe_list.rs, src/main.rs und src/daemon/debug_wav.rs. Nur gezielt lesen, Kontext klein halten.

Selbst ausführen, mit `TEMP`/`TMP` auf ein eigenes Verzeichnis unter `D:\DEV\diktier\.herd\` wie beim letzten Mal:
- `cargo test`
- `pwsh -File scripts\tests\Test-UltraScripts.ps1`
- `python scripts\tests\test_quantize_ultra.py`

Summary-Zeilen zitieren. Sonst nichts ausführen, was Modelle lädt, Netz braucht oder in `%LOCALAPPDATA%` schreibt.

Bericht nach docs/reviews/impl-ultra-wp2-sol-2.md: Tabelle je Befund (behoben / teilweise / offen, mit Begründung), dann neue Befunde nach Schwere mit Datei:Zeile, Szenario und Vorschlag, und ein Urteil, ob WP3a (Veröffentlichen) aus Code-Sicht starten kann. TUI-Ausgabe knapp.
