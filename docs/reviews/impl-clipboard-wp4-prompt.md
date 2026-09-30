Auftrag: WP4 aus docs/clipboard-restore-plan.md umsetzen: `output.mode = "type"` entfernen, Debug-WAV als Ring der letzten zehn, README und Version 0.4.0. Diktier, Rust, Windows-only. Basis: aktueller Working Tree. Er enthält die uncommitteten Ergebnisse von WP1–WP3 (Clipboard-Restore, Overlay-Hinweis). Die sind Vorgabe, nicht Arbeitsgegenstand.

Lies zuerst:
1. docs/clipboard-restore-plan.md (v2): Leitentscheidungen 10 und 11, WP4, G5; Entscheidung F1 (Grenzen für die README).
2. docs/SPEC.md §7.5, §8, §10, §18 #14 (verbindlich; weicht der Plan ab, gilt die SPEC, Widersprüche in den Bericht).
3. src/config.rs (`OutputMode`, Default-Datei, Validierung, `hotkey.mode` als Muster), src/daemon/debug_wav.rs, src/daemon/workers.rs (`dump_debug_wav` und sein Aufrufer, woher die Laufnummer kommt), README.md, docs/windows-plan.md (WP6-Notizen), Cargo.toml.
4. docs/reviews/impl-clipboard-wp1-notes.md (inkl. „Nacharbeit 2“, Liste „Weiter offen für WP4“), docs/reviews/impl-clipboard-wp1-sol.md und docs/reviews/impl-clipboard-wp3-notes.md (Berichte der Vorgänger: was `--clipboard-check` kann, welche Hinweise es gibt).

Umfang:
- config.rs nach Leitentscheidung 10, mit Tests: `"type"` → Fatal mit genau der Meldung aus dem Plan, `"paste"` ok, unbekannter Wert Fatal, fehlender Schlüssel = `"paste"`, Default-Datei mit `# v1 nur dieser Wert`. `OutputMode::Type` entfällt samt toter Zweige.
- Debug-WAV nach Leitentscheidung 11 und SPEC §10, mit Tests: Dateiname aus UTC bis Millisekunde und Laufnummer, Ring (nur genaues eigenes Muster, älteste zuerst, genau zehn), `.part`-Reste, einmaliges Entfernen von `last_recording.wav`, ein gescheiterter Schreibvorgang zählt nicht und löscht nichts. Die Laufnummer an `dump_debug_wav` durchreichen. Tests arbeiten in einem temporären Verzeichnis, nie in `%TEMP%\diktier`.
- README: Versionszeile auf v0.4.0. Troubleshooting „Zwischenablage“: was wiederhergestellt wird, die Grenzen aus F1 (OLE-Objekte wie kopierte Outlook-Elemente, Excels Kopierrahmen, synthetisch ersetzte Bildformate), die Hinweiskarte im Overlay mit ihren Texten, `--clipboard-check` und `--roundtrip` mit Warnhinweis. Abschnitt Debug-WAV (neues Muster, zehn Dateien). Breaking Change `output.mode` mit Migrationshinweis. Verweise auf Win+V: Diktate und Wiederherstellungen erscheinen dort nicht mehr.
- Aus dem WP1-Review (docs/reviews/impl-clipboard-wp1-sol.md, Kleinigkeit 3, und docs/reviews/impl-clipboard-wp1-notes.md, Abschnitt „Nacharbeit 2“, Punkte „nur notiert“) in src/daemon/workers.rs:
  - die Snapshot-Logzeile auch bei `CopyOnly` nach dem Snapshot schreiben, sofern WP1 den Report dort liefert; sonst im Bericht vermerken
  - `history_excluded` in die Paste-Logzeile (z. B. `· Verlauf ausgeschlossen: nein`, nur wenn false)
  - im Quit-Pfad eine eindeutige Warnung, wenn die Sicherung des Transkripts scheitert: „Transkript beim Beenden nicht gesichert — Zwischenablage kann leer sein“
  Mit Tests, soweit die Logik ohne Win32 prüfbar ist.
- Aus dem WP3-Bericht (docs/reviews/impl-clipboard-wp3-notes.md, Abweichungen 1 und 2): Abweichung 1 ist bestätigt. Zu Abweichung 2 hat der Orchestrator entschieden: `paused` verbirgt den Hinweis **nicht** mehr (SPEC §4.5 ist angepasst). In src/daemon/mod.rs `overlay_view` entsprechend umstellen (`quitting` → `error` → aktive Zustände → Hinweis in `idle` → verborgen) und die Tests anpassen, auch den Tray-Diktat-während-Pause-Fall (Hinweis sichtbar). Das ist die einzige erlaubte Änderung in src/daemon/mod.rs.
- Cargo.toml Version 0.4.0 (Cargo.lock zieht `cargo build` nach). docs/windows-plan.md: WP6-Absatz um einen Satz zu 0.4.0 ergänzen, analog zu den bisherigen Versionsnotizen.

Gates (Befehl und tatsächliche Summary-Zeile zitieren):
1. `cargo fmt --check`
2. `cargo clippy --all-targets -- -D warnings` ohne Meldung
3. `cargo test` grün
4. `cargo test -- --ignored stt_smoke_fixtures` grün
5. `cargo build --release`, `target/release/diktier.exe --version` zeigt 0.4.0

Regeln:
- Nicht committen.
- Keine Änderungen an docs/SPEC.md, docs/clipboard-restore-plan.md, docs/reviews/* (außer deinem Bericht), src/inject/*, src/overlay.rs, src/state.rs. Stößt du dort auf einen Fehler, beschreib ihn im Bericht.
- Keine ignorierten `clipboard_live_*`-Tests ausführen, kein `--roundtrip`, nichts, was Ralfs Zwischenablage verändert. Keine installierte Diktier-Version anfassen, keinen laufenden Daemon beenden.
- Kein herdr, keine weiteren Panes.

Bericht nach docs/reviews/impl-clipboard-wp4-notes.md: Umgesetzt, Abweichungen und warum, Gate-Ausgaben wörtlich, Offen. TUI-Ausgabe knapp; das Ergebnis zählt nur aus Bericht und `git diff`.
