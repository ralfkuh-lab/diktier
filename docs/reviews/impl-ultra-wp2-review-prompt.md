Review-Auftrag: Code-Review der Pakete WP2a–WP2c (0.5.0) für den Ultra-Alltagstest in Diktier (Rust, Windows-only). Du bist ein delegierter Sub-Agent: nicht orchestrieren, nicht weiterdelegieren, nur dieses Review erledigen. Nur lesen und die unten genannten Befehle ausführen; keine Dateien außer deinem Bericht ändern.

Gegenstand ist der Working Tree gegen `HEAD` (`2d10ee0`). Diff: `git diff HEAD -- src scripts Cargo.toml` plus die neuen Dateien `src/daemon/model_wiring_tests.rs`, `src/transcribe_list.rs`, `scripts/compare-models.ps1`, `scripts/compare-models.html`, `scripts/ultra-test-lib.ps1`, `scripts/bench-models.ps1`, `scripts/quantize-ultra.py`, `scripts/quantize-ultra.requirements.txt`. Lies die Dateien gezielt, nicht das ganze Repo. Halte den Kontext klein, denn ab 272K Input-Token wird es teurer.

Vorgaben:
- docs/SPEC.md v1.10: §6.2, §6.3 (Ultra-Artefakte, Unveränderliche URL, Prüfumfang), §8, §9 (Entwickler-Modi), §10 (Log-Vertrag, Debug-WAV v1.10), §11 (versions.toml).
- docs/ultra-alltagstest-plan.md: Leitentscheidungen, Bewertungsprotokoll, Abnahmekriterien, Datenschutz, WP2a–c.
- Berichte der Implementierer: docs/reviews/impl-ultra-wp0-notes.md, impl-ultra-wp2a-notes.md, impl-ultra-wp2b-notes.md, impl-ultra-wp2c-notes.md; Integrationsgate: impl-ultra-wp2d-notes.md.

Prüffragen:
- **Modellwahl:** Wird genau ein `SelectedModel` aus der Config an alle Verbraucher durchgereicht (Daemon, Download, Engine, Tray, `model_artifacts`, CLI)? Gibt es irgendeinen Pfad mit stillem v3-Fallback oder eigener Auswahl? Bleibt der v3-Eintrag im Manifest byte-gleich?
- **Download und Integrität:** Stimmen Größe und SHA-256 der Ultra-Einträge mit SPEC §6.3 überein? Weist `parse_catalog` Redirect-Ziele und `latest` wirklich ab? Verhalten bei Busy, Abbruch und Teil-Download in getrennten Verzeichnissen?
- **Config:** Unbekannter Schlüssel fatal, Meldung, Vorlage. Rückweg auf v3 ohne Netz.
- **Belegsammlung:** Werden die drei Umgebungsvariablen genau einmal gelesen? Stimmen Grenzen und Warnzeilen? Gibt es Startzeilen ohne Inhalte? Kann `prune` mit großer Kapazität im fremden Verzeichnis etwas Falsches löschen?
- **CLI/JSONL:** Stimmen Exitcodes, Zustände, `run` und Escaping? Bleibt `--transcribe-wav` ohne `--model` unverändert? Lädt das Modell nur, wenn nötig?
- **Skripte:** Datenschutz (keine Texte außerhalb des Auswertungsordners, `zusammenfassung.md` ohne Texte), verdecktes A/B (verrät die Seite irgendwo die Zuordnung, etwa durch Reihenfolge, IDs, Dateinamen, Zeiten oder Audio-Pfade?), Korrektheit der Kennzahlen gegen die Abnahmekriterien (Wilson, K5 als gleich, Veto, Mindestmengen), Inventur über Daemon-Sitzungen, `bench-models.ps1` gegen Kriterium 4. Prozessaufrufe: stdout und stderr ohne Deadlock?
- **release.ps1:** Bundle-Gate, TOML-Leser, Pflichtdateien.
- **quantize-ultra.py:** Das Quellverzeichnis wird nur gelesen, Hashprüfung vor und nach der Quantisierung, Abbruchpfade.
- **Tests:** Was fangen sie nicht? Welche Fehlerbilder bleiben ungetestet und sind relevant?

Selbst ausführen: `cargo test` (Summary zitieren) und `cargo clippy --all-targets -- -D warnings`. **Nicht** ausführen: stt-smoke, Release-Build, die PowerShell-Skripte gegen echte Daten, nichts, was Modelle lädt oder in `%LOCALAPPDATA%` bzw. `%TEMP%` schreibt. Keine installierte Version, keinen laufenden Daemon anfassen, nichts herunterladen.

Bericht nach docs/reviews/impl-ultra-wp2-sol.md: Befunde nach Schwere (kritisch / wichtig / Kleinigkeit), je mit Datei:Zeile, konkretem Fehlerszenario und Vorschlag. Getrennt: statisch geprüft, selbst ausgeführt, übernommen aus Berichten. Ist ein Bereich in Ordnung, nur kurz so vermerken. TUI-Ausgabe knapp.
