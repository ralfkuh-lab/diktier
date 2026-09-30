Du bist ein delegierter Sub-Agent: nicht orchestrieren, nicht weiterdelegieren — nur diese Aufgabe erledigen.

Aufgabe: Implementierungs-Review von WP1/WP2 (Mehrformat-Clipboard-Snapshot/-Restore, `--clipboard-check`) in Diktier (Rust, Windows-only). Stand: Commit 56a6a4e plus uncommittete Änderungen im Working Tree. Geprüft wird der Diff in `src/inject/` (inkl. neuem `src/inject/formats.rs`), `src/main.rs`, `src/daemon/workers.rs` und `Cargo.toml`. Parallel arbeitet ein anderer Agent an WP3 in `src/state.rs`, `src/daemon/mod.rs`, `src/overlay.rs` und am Overlay-Teil von `src/daemon/workers.rs`. Diese Änderungen sind **nicht** Gegenstand des Reviews. Ändert sich während deines Reviews etwas in `src/inject/`, sag das im Bericht.

Lies:
1. docs/SPEC.md §7.1, §7.1.1, §7.3, §9 (CLI, Exitcodes), §10, §18 #14 — verbindlich
2. docs/clipboard-restore-plan.md (v2), Entscheidungen F1–F3, Leitentscheidungen 1–7 und 9, WP1, WP2
3. docs/reviews/impl-clipboard-wp1-notes.md (Bericht des Implementierers inkl. Abschnitt „Nacharbeit“)
4. `git diff -- src/inject src/main.rs src/daemon/workers.rs Cargo.toml` und `src/inject/formats.rs` komplett. Für Kontext die ganzen Dateien `src/inject/windows.rs` und `src/inject/protocol.rs`, nicht das ganze Repo.

Prüfe kritisch, mit Fokus auf Datenverlust, Win32-Korrektheit und Rennen:
- `unsafe`/Win32: Handle-Eigentum (HGLOBAL, HENHMETAFILE) auf jedem Pfad, RAII, kein Double-Free, kein Leak nach gescheitertem `SetClipboardData`, `GlobalLock`/`GlobalUnlock`-Paare, `CloseClipboard` auf jedem Pfad, `GetLastError`-Reihenfolge, `SetLastError(ERROR_SUCCESS)` vor `EnumClipboardFormats`/`CountClipboardFormats`.
- Nie ein GDI-Handle per `GlobalLock`, nie `GetClipboardData` für Nicht-Datenklassen.
- Restore-Ablauf gegen §7.1.1 und P5: Kann irgendein Pfad den vorherigen Inhalt **und** das Transkript verlieren, ohne `Failed` zu melden? Kann ein fremder Copy überschrieben werden? Guard/`WM_DESTROYCLIPBOARD`, `expected_seq`, `delayed`, `WM_RENDERFORMAT`/`WM_RENDERALLFORMATS` nach dem Umbau.
- Eigene Payload als Snapshot: richtig nach Restore, `RestoreFailed`, `NoReadTimeout`, `CopyOnly`, Quit-Materialisierung? `reads = 0` nach Übernahme: Verdeckt das einen echten Read?
- Verlaufsmarker auf allen Transkript-Pfaden und beim Restore; nicht beim Restore von `Empty`.
- Budgets: korrekt weich, keine Endlosschleife, 4096-Format-Schranke sinnvoll?
- Snapshot-Fehler → `Unrestorable` (Nacharbeit): Paste läuft weiter, kein Versprechen.
- Dreifache Fokusprüfung, keine Aktivierung.
- Formatmatrix gegen die Microsoft-Doku (Abweichungen 3/4 im Bericht bewerten).
- CLI: nur lesend per Default; `--roundtrip` verweigert bei laufendem Daemon, ohne den Daemon zu stören (Single-Instance-Mechanik); Präfixvergleich; Exitcodes nach §9.
- Log (§10): keine Inhalte, keine `CF_HDROP`-Pfade, Namensbereinigung.
- Tests: Decken die Fake-Tests die Fehlerpfade ab? Sind die `clipboard_live_*`-Tests so gebaut, dass sie beim manuellen Lauf etwas beweisen (Fixture-Owner, Pump, Blockade)? Fehlen Tests?

Du darfst `cargo test`, `cargo clippy --all-targets -- -D warnings` und `cargo fmt --check` ausführen. **Nicht** ausführen: die ignorierten `clipboard_live_*`-Tests, `--clipboard-check --roundtrip`, irgendetwas, das die Zwischenablage verändert. Laufen die Gates, während der WP3-Agent parallel Dateien ändert, kann der Build zwischendurch rot sein. Unterscheide das dann im Bericht.

Schreibe das Ergebnis nach docs/reviews/impl-clipboard-wp1-sol.md:
- Kurzurteil (3 Sätze)
- Blocker (vor Freigabe zu beheben)
- Wichtige Hinweise
- Kleinigkeiten
- Selbst ausgeführte Prüfungen (Befehl + Summary) getrennt von statischer Prüfung

Jeder Punkt mit Fundstelle (Datei:Zeile/Funktion), Problem und konkretem Vorschlag. Keine Code-Änderungen. Halte die TUI-Ausgabe knapp; das Ergebnis zählt nur aus der Datei.
