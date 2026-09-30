Du bist ein delegierter Sub-Agent: nicht orchestrieren, nicht weiterdelegieren — nur diese Aufgabe erledigen.

Aufgabe: Abschließendes Implementierungs-Review des Pakets „Mehrformat-Clipboard-Restore und Overlay-Hinweis“ (Diktier 0.4.0, Rust, Windows-only). Stand: Commit 56a6a4e plus alle uncommitteten Änderungen im Working Tree. Niemand ändert während deines Reviews Dateien.

WP1/WP2 (src/inject/*, `--clipboard-check`) hast du schon einmal geprüft: docs/reviews/impl-clipboard-wp1-sol.md. Seitdem kam „Nacharbeit 2“ dazu. Prüfe jetzt:
1. **WP1-Nacharbeit 2:** Sind deine Blocker 1–3 und Hinweise wirklich behoben? Grundlage: docs/reviews/impl-clipboard-wp1-notes.md, Abschnitt „Nacharbeit 2“. Besonders die neue sofortige Materialisierung des Transkripts (`copy_transcript`, `materialize_transcript`, `wait_for_first_read`): Rennen mit dem einfügenden Ziel, fremder Copy, Sequenz/Guard, Read-Zählung, 5-s-Belegung des Inject-Workers bei `NoPromise`/`Disabled` ohne Read.
2. **WP3 (Hinweiskarte):** docs/reviews/impl-clipboard-wp3-notes.md; Code in src/state.rs, src/daemon/mod.rs, src/daemon/workers.rs (`restore_notice`, `OverlayCmd::View`, `drain_overlay_commands`, `apply_overlay_view`, `overlay_loop`), src/daemon/logging.rs, src/overlay.rs, src/overlay/windows.rs.
3. **WP4:** docs/reviews/impl-clipboard-wp4-notes.md; src/config.rs, src/daemon/debug_wav.rs, workers.rs (Debug-WAV, Logzeilen, Quit-Warnung), README.md, Cargo.toml, docs/windows-plan.md.

Verbindlich: docs/SPEC.md v1.8 (§4.2, §4.3, §4.5 mit Hinweiskarte, §7.1/§7.1.1, §7.3, §7.5, §8, §9, §10, §18 #14). Plan: docs/clipboard-restore-plan.md (v2 mit Nachträgen).

Halte den Kontext klein: Lies die Berichte, dann gezielt `git diff -- <datei>` der genannten Dateien. Nicht das ganze Repo, keine Datei doppelt.

Prüfe kritisch:
- Fokusregel §4.2: Kann die Hinweiskarte je aktivieren, Fokus nehmen oder Klicks abfangen, auch beim Einblenden aus dem Unsichtbaren?
- Kernzustand: Setzen und Löschen des Hinweises (alle Pfade in §4.5), Prioritäten `overlay_view` inkl. Pause-Regel, verspätete Events, kein hängender Hinweis. Koaleszenz im Worker, `Shutdown`-Vorrang.
- GDI: Ressourcenfreigabe (Font, DIB, DC, Selektion) auf jedem Pfad, `blend_mask` premultipliziert korrekt, Fallback nur Glyphe, keine Abschaltung des Overlays bei Textfehler.
- Config: `"type"` Fatal mit Meldung, fehlender Schlüssel = paste, Default-Datei.
- Debug-WAV-Ring: nur eigenes Muster löschen, atomar, Altlast, Laufnummer, keine Kollision, Tests ohne echtes `%TEMP%\diktier`.
- Log §10: keine Inhalte, keine Pfade, Hinweiszeilen, Quit-Warnung.
- README: stimmt sie mit SPEC und Verhalten überein (Grenzen F1, Texte der Karte, CLI, Breaking Change)?
- Tests: Lücken?

Du darfst `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` und `cargo test` ausführen. **Nicht** ausführen: die ignorierten `clipboard_live_*`-Tests, `--clipboard-check --roundtrip`, einen Daemon, irgendetwas, das die Zwischenablage verändert oder Fenster öffnet.

Schreibe das Ergebnis nach docs/reviews/impl-clipboard-final-sol.md:
- Kurzurteil (3 Sätze, mit klarer Freigabe-Empfehlung für den Live-Test ja/nein)
- Blocker
- Wichtige Hinweise
- Kleinigkeiten
- Status deiner früheren WP1-Befunde (je Befund: behoben / teilweise / offen)
- Selbst ausgeführte Prüfungen (Befehl + Summary) getrennt von statischer Prüfung

Jeder Punkt mit Fundstelle (Datei:Zeile/Funktion), Problem und konkretem Vorschlag. Keine Code-Änderungen. TUI-Ausgabe knapp; das Ergebnis zählt nur aus der Datei.
