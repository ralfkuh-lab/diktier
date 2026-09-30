Auftrag: Nacharbeit zum abschließenden Review des Pakets „Mehrformat-Clipboard-Restore und Overlay-Hinweis“ (Diktier 0.4.0, Rust, Windows-only). Basis: aktueller Working Tree (Commit 56a6a4e plus alle uncommitteten Änderungen aus WP1–WP4). Niemand sonst arbeitet parallel am Code.

Lies zuerst:
1. docs/reviews/impl-clipboard-final-sol.md: das Review, das du abarbeitest
2. docs/SPEC.md §4.5, §7.1/§7.1.1, §10, §18 #14 (verbindlich)
3. docs/clipboard-restore-plan.md (v2) und die Berichte docs/reviews/impl-clipboard-wp1-notes.md (vor allem „Nacharbeit 2“), impl-clipboard-wp3-notes.md, impl-clipboard-wp4-notes.md
4. die betroffenen Dateien: src/inject/protocol.rs, src/inject/windows.rs, src/inject/mod.rs, src/inject/fake.rs, src/daemon/workers.rs, src/daemon/mod.rs (`shutdown`), src/daemon/debug_wav.rs, src/overlay/windows.rs (`render_notice_text`), README.md

Umsetzen (Entscheidungen des Orchestrators):

1. **Blocker 1 — Materialisierung darf das Transkript nie verlieren.**
   - In `take_clipboard` bzw. `fill_open_clipboard` unterscheiden, ob schon mutiert wurde.
   - Scheitert `EmptyClipboard` und die Sequenz ist unverändert: Eigentum und Versprechen **behalten** (kein `forget_ownership`), Ergebnis „nicht materialisiert, Versprechen offen“.
   - Scheitert nach `EmptyClipboard` das eager `SetClipboardData(CF_UNICODETEXT, h)`: noch im geöffneten Clipboard ein Delayed-Versprechen `SetClipboardData(CF_UNICODETEXT, NULL)` als Rückfall setzen (plus Marker). Das Versprechen lebt dann weiter, `delayed = true`, und Nr. 2 kümmert sich darum.
   - Scheitert auch das, ist das Transkript verloren. Dann meldet der Inject-Ausgang das ausdrücklich, sodass der Worker `InjectReport::Failed` mit „Zwischenablage leer — Transkript verloren“ bucht (Tray `error`).
   - Das Ergebnis der Materialisierung kommt in den Inject-Ausgang (z. B. `transcript: Secured | PromiseOpen | Lost`), statt nur per `eprintln!`. Der Worker loggt `PromiseOpen` und `Lost` über den Daemon-`Logger` als Warnung.
   - Fake-Schalter und Tests für beide Fehlerstellen **während der Materialisierung**.

2. **Blocker 2 — kein offenes Versprechen, das still verschwindet.**
   - Jeder Fehlerausgang von `inject_paste` **nach** `become_owner` (`send_paste_shortcut`-Fehler, Pump-Fehler) versucht vor der Rückgabe `materialize_transcript`. Der ursprüngliche Fehler bleibt der Ausgang. Test.
   - Der Inject-Worker versucht im Idle, ein noch offenes eigenes Versprechen zu materialisieren: in der Worker-Schleife, wenn keine Paste-Session läuft und `delayed` ist, höchstens alle 500 ms, höchstens 10 Versuche, danach eine Warnung über den Logger. Er respektiert Owner und Sequenz (fremder Copy = aufgeben, nichts tun). Dafür eine Sink-Methode (z. B. `pending_promise() -> bool` plus `materialize_transcript`) nutzen. Die Retry-Logik als reine, testbare Funktion (Zeitplan, Abbruch).
   - Quit: `Daemon::shutdown` loggt jeden nicht gesicherten oder ungeklärten Ausgang von `save_targets` (Timeout, Fehler, `NotOwner` nach offenem Versprechen) als **Warnung** mit dem eindeutigen Text „Transkript beim Beenden nicht gesichert — Zwischenablage kann leer sein (<Grund>)“. `NotOwner` ohne offenes Versprechen ist normal und bleibt Info.
   - Tests für Shortcut-Fehler → Materialisierung, Idle-Retry (Erfolg, fremder Copy, 10 Fehlschläge → Warnung) und die Quit-Logzeilen.

3. **Blocker 3 — Debug-WAV kollisionssicher.**
   - `.part` exklusiv mit `create_new` und eindeutigem Namen anlegen (z. B. `<ziel>.<pid>-<zähler>.part`), niemals eine fremde oder alte `.part` trunkieren oder löschen außer über die Altersregel.
   - Finalisierung ohne Ersetzen: Existiert der Zielname schon, `-2`, `-3` … anhängen. Das Muster in `parse_name` erlaubt das Suffix.
   - Tests: bestehender endgültiger Name, bestehende `.part`, zweimal `write_recording` mit identischem `(at, run)` → zwei Dateien, keine überschrieben.
   - Kleinigkeit aus dem Review: `parse_name` prüft zusätzlich Kalendergrenzen (Monat 1–12, Tag 1–31, Stunde 0–23, Minute/Sekunde 0–59, ms 0–999).

4. **Hinweis Marker und Snapshot bei `CopyOnly`.** `InjectOutcome::CopyOnly` trägt zusätzlich `history_excluded: bool` und optional den `ClipboardReport`/`SnapshotReport`, falls der Snapshot schon lief. Der Worker loggt die Snapshot-Zeile und `· Verlauf ausgeschlossen: nein` wie beim Paste. Die Marker-Warnungen aus `Win32OutputSink::new`/`marker_handle`/`place_marker` erreichen den Logger: Die Registrierungswarnung gibt der Sink über eine abfragbare Startwarnung heraus (z. B. `take_startup_warning()`), die der Worker nach dem Anlegen loggt.

5. **Hinweis `GdiFlush`.** Rückgabewert prüfen, bei 0 `OverlayError`, damit der Glyphen-Fallback mit Warnung greift.

6. **Hinweis README.** Bedingte Zusagen genau formulieren: Restore nur nach bedientem Read und mit `restore_clipboard = true`, eventuell partiell (Hinweiskarte). Win+V-Ausschluss mit Einschränkung „sofern der Marker gesetzt werden konnte (sonst Logzeile)“. `.part`-Reste werden nach einer Stunde entfernt. Namenssuffix `-2` bei Kollision.

7. **Kleinigkeit Roundtrip.** `read_saved`/Ausgabe unterscheidet „Budget erschöpft, nicht geprüft“ von echter Abweichung. Das Budget für die Nachprüfung ist die Summe der gesicherten Längen plus Reserve für die Aufrundung (z. B. + 64 KiB je Format).

Nicht umsetzen: die 5-s-Wartezeit bei `NoPromise`/`Disabled` ohne Read. Sie bleibt, ist dokumentiert und greift nur, wenn kein Einfügen stattfand. Ergänze dazu einen Satz in der README (Troubleshooting).

Gates (Befehl und tatsächliche Summary-Zeile zitieren):
1. `cargo fmt --check`
2. `cargo clippy --all-targets -- -D warnings` ohne Meldung
3. `cargo test` grün, `cargo test --no-run` baut die ignorierten Tests
4. `cargo test -- --ignored stt_smoke_fixtures` grün
5. `cargo build --release`

Regeln:
- Nicht committen.
- Keine Änderungen an docs/SPEC.md, docs/clipboard-restore-plan.md, docs/reviews/* (außer deinem Bericht). Widerspricht eine Entscheidung oben der SPEC, setz sie um und nenn den Widerspruch im Bericht (der Orchestrator zieht die SPEC nach).
- Nichts ausführen, was Ralfs Zwischenablage verändert (keine `clipboard_live_*`, kein `--roundtrip`), keinen Daemon starten, keine installierte Version anfassen, kein Fenster öffnen.
- Kein herdr, keine weiteren Panes. Interne Sub-Agenten für Recherche sind erlaubt.

Bericht nach docs/reviews/impl-clipboard-final-fix-notes.md: je Punkt Umsetzung mit Datei/Funktion, Abweichungen und SPEC-Widersprüche, Gate-Ausgaben wörtlich, Offen. TUI-Ausgabe knapp; das Ergebnis zählt nur aus Bericht und `git diff`.
