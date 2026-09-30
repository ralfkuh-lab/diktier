Auftrag: WP3 aus docs/clipboard-restore-plan.md umsetzen: Hinweiskarte im Overlay nach dem Diktat. Diktier, Rust, Windows-only. Basis: aktueller Working Tree. Er enthält die uncommitteten Ergebnisse von WP1/WP2 (Mehrformat-Clipboard-Restore, `--clipboard-check`). Die sind Vorgabe, nicht Arbeitsgegenstand.

Lies zuerst:
1. docs/clipboard-restore-plan.md (v2): Leitentscheidung 8 komplett, WP3, G4 #7–#9.
2. docs/SPEC.md §4.2, §4.3 (TrayClick), §4.5 mit „Hinweiskarte (v1.8)“, §7.1 P2/P7, §7.3. Verbindlich; weicht der Plan ab, gilt die SPEC, Widersprüche in den Bericht.
3. docs/reviews/impl-clipboard-wp1-notes.md: welche Felder `InjectOutcome`/`RestoreDecision` jetzt tragen.
4. docs/overlay-plan.md, Leitentscheidungen (Thread-Modell, Fokusregel, Rendering-Vertrag) und den Nachtrag 2026-09-07 (Topmost-Band).
5. src/state.rs (`InjectReport`, `InjectFinished`, `finish_run`, `enter_error`, Press/TrayClick/Pause/Quit/Tick-Behandlung, Tests), src/daemon/mod.rs (`overlay_visible`, `flush_presentation`, Tests), src/daemon/workers.rs (Inject-Worker `InjectCmd::Paste`/`CopyOnly`, `map_copy_reason`, `OverlayCmd`, `drain_overlay_commands`, `overlay_loop`), src/overlay.rs komplett (reine Zeichenfunktionen, Modul `windows`).

Umfang nach Leitentscheidung 8:
- Kern: `Notice` (Tabelle aus SPEC §4.5), `InjectReport::Pasted { notice }`, `runtime.notice` mit Ablauf über `runtime.now` (`NOTICE_DURATION = 3 s`). Setzen und Löschen exakt nach den normativen Regeln. `CopyOnly`-Gründe inklusive TrayClick abbilden. Der Inject-Worker leitet den Hinweis aus dem WP1-Ausgang ab.
- `overlay_view(runtime) -> OverlayView { Hidden, Level, Notice(Notice) }` mit der Prioritätsliste. `flush_presentation` sendet `OverlayCmd::View(..)` beim Wechsel. Der Worker koalesziert auf die letzte Ansicht, `Shutdown` hat Vorrang, `Level ↔ Notice` ohne `hide()` und ohne Aktivierung.
- Overlay: Notice-Karte (Warn-Glyphe aus Primitiven in `WAVE_HOT_COLOR`, zwei Zeilen). Text per GDI in ein eigenes Top-down-32-bit-DIB, schwarz initialisiert, Segoe UI, `ANTIALIASED_QUALITY`, Zeile 1 halbfett, DPI-skaliert, `DT_SINGLELINE | DT_END_ELLIPSIS`, Luminanz als Maske. `Canvas::blend_mask` als reine Funktion (Source-over, premultipliziert, Invariante `RGB ≤ A`). Fallback nur Glyphe bei Textfehler, Log-Warnung, Overlay bleibt aktiv. Eine Logzeile `Hinweis: <Fall>` je gezeigtem Hinweis.
- Fokusregel §4.2: `WS_EX_NOACTIVATE`, `SW_SHOWNOACTIVATE`, `SWP_NOACTIVATE`, durchklickbar, keine Fokus-APIs. Das bestehende Topmost-Verhalten bleibt.
- Tests nach WP3 im Plan (state, daemon, workers, overlay).

Gates (Befehl und tatsächliche Summary-Zeile zitieren):
1. `cargo fmt --check`
2. `cargo clippy --all-targets -- -D warnings` ohne Meldung
3. `cargo test` grün
4. `cargo test -- --ignored stt_smoke_fixtures` grün
5. `cargo build --release` erfolgreich. Ein visueller Test mit laufendem Daemon ist **nicht** dein Gate. Den macht Ralf später (G4). Optional darfst du einen ignorierten Hilfs-Test ergänzen, der die Hinweiskarte ohne Fenster bei 96/144/192 dpi als PNG unter `target/overlay-notice/` ablegt (das `png`-Crate ist dafür als dev-dependency erlaubt), damit der Orchestrator sie ansehen kann. Führ ihn dann aus und nenne die Pfade im Bericht.

Regeln:
- Nicht committen.
- Keine Änderungen an docs/SPEC.md, docs/clipboard-restore-plan.md, docs/reviews/* (außer deinem Bericht), README.md, Cargo-Version, src/config.rs, src/daemon/debug_wav.rs. **src/inject/* gar nicht ändern**: Dort läuft parallel ein Review auf dem jetzigen Stand. `InjectOutcome::Pasted { restore, .. }` und `CopyOnly { reason }` tragen alles, was du brauchst (siehe WP1-Bericht, Abschnitt „Hinweise für WP3“). Die Ableitung des Hinweises gehört in den Inject-Worker (src/daemon/workers.rs). Fehlt dir dort wirklich etwas, beschreib es im Bericht, statt src/inject/ zu ändern.
- Kein laufender Daemon, keine installierte Version anfassen. Keine ignorierten `clipboard_live_*`-Tests ausführen, nichts, was Ralfs Zwischenablage verändert. Kein Fenster, das den Fokus nimmt.
- Kein herdr, keine weiteren Panes.

Bericht nach docs/reviews/impl-clipboard-wp3-notes.md: Umgesetzt, Abweichungen und warum, Gate-Ausgaben wörtlich, Offen und Hinweise für den Live-Test G4. TUI-Ausgabe knapp; das Ergebnis zählt nur aus Bericht und `git diff`.
