Nachreview 2: Die Reste F1–F6 aus deinem Nachreview docs/reviews/impl-ultra-wp2-sol-2.md sind in WP2f bearbeitet. Du bist weiterhin delegierter Sub-Agent: nur lesen, die unten genannten Befehle ausführen und den Bericht schreiben.

Lies: docs/reviews/impl-ultra-wp2f-notes.md und im Plan docs/ultra-alltagstest-plan.md die mit v2.2 markierten Stellen (Zahlenformat-Angabe, Verlängerungsregel).

Prüfe je F1–F6 an den jetzigen Codestellen, ob der Befund behoben ist, und ob die Änderungen neue Fehler einführen: scripts/compare-models.ps1, scripts/compare-models.html, scripts/bench-models.ps1, scripts/ultra-test-lib.ps1, scripts/release.ps1, scripts/tests/Test-UltraScripts.ps1. Nur gezielt lesen.

Selbst ausführen, mit TEMP/TMP auf ein eigenes Verzeichnis unter D:\DEV\diktier\.herd\ wie bisher:
- `pwsh -File scripts\tests\Test-UltraScripts.ps1`
- `python scripts\tests\test_quantize_ultra.py`

Summary zitieren. Nichts, was Modelle lädt, Netz braucht oder in %LOCALAPPDATA%/%APPDATA% schreibt.

Bericht nach docs/reviews/impl-ultra-wp2-sol-3.md: Tabelle F1–F6 (behoben / teilweise / offen, mit Begründung), neue Befunde nach Schwere mit Datei:Zeile und Szenario, und ein Urteil, ob die Auswertungswerkzeuge für WP5 abgenommen werden können. Knapp halten.
