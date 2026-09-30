# WP2b — Belegsammlung: Umsetzungsbericht

Auftrag: [impl-ultra-wp2b-prompt.md](impl-ultra-wp2b-prompt.md), Plan
[ultra-alltagstest-plan.md](../ultra-alltagstest-plan.md) (v2) WP2b, SPEC v1.10
§10. Nicht committet.

## ✅ Umgesetzt

- **Einmal lesen, dann durchreichen.** `debug_wav::from_env()` liest
  `DIKTIER_DEBUG_WAV`, `DIKTIER_DEBUG_WAV_KEEP`, `DIKTIER_DEBUG_WAV_DIR` und
  `TEMP` genau einmal in `daemon::run` (direkt nach der Startzeile
  „diktier … startet“). Das Ergebnis `Option<DebugWavConfig { dir, keep }>`
  geht über `AudioWorker::spawn` → `audio_loop` → `dump_debug_wav`. Die alten
  Funktionen `enabled()` und `debug_dir()`, die bei jedem Dump die Umgebung
  lasen, sind entfernt.
- **Reine Auswertung.** `resolve(switch, keep, dir, default_dir)`,
  `parse_keep`, `parse_dir` und `default_dir(temp)` bekommen die Werte
  übergeben und lesen keine Umgebung.
  - Schalter: nur exakt `1`, wie bisher.
  - KEEP: nur ASCII-Ziffern (kein Vorzeichen, kein Leerraum), Wert 1–5000.
    Führende Nullen sind erlaubt (`0010` = 10).
  - DIR: leer oder nur Leerraum ist ungültig, relativ ist ungültig
    (`Path::is_absolute`; unter Windows also auch `\wav` und `C:wav`).
    `%…%` wird nicht expandiert und gilt damit als relativ.
  - Je ungültigem Wert genau eine Warnzeile, z. B.
    `Debug-WAV: DIKTIER_DEBUG_WAV_KEEP liegt außerhalb von 1–5000 — nehme den Default 10`.
    Der Wert selbst steht nicht in der Zeile, nur der Grund. Nur der
    betroffene Wert fällt auf den Default.
- **Startzeile** bei eingeschaltetem Dump:
  `Debug-WAV an: <verzeichnis>, behalte <n>` (Info). Pro Dump bleibt die
  bestehende Zeile `DIKTIER_DEBUG_WAV: <pfad>`.
- **Ring** (`write_recording(dir, keep, …)`, `prune(dir, keep, now)`): Die
  Kapazität kommt aus der Konfiguration, das Verzeichnis ebenso. Unverändert
  bleiben Muster, `.part`-Regeln (eine Stunde), Altlast `last_recording.wav`,
  Rename ohne Ersetzen und `create_dir_all` bei Bedarf. `KEEP` heißt jetzt
  `DEFAULT_KEEP` (10).
- **README**, Abschnitt Debug-WAV: die beiden Variablen, Bereich/Default,
  Warnung und Startzeile. Dazu der Hinweis „(einstellbar, siehe unten)“ an
  der Stelle „zehn jüngsten“. Sonst nichts im README geändert.
- **Tests** (neu, `src/daemon/debug_wav.rs`), alle ohne Prozessumgebung:
  - `keep_accepts_exactly_one_to_five_thousand`: 0/1/5000/5001, `000`,
    Überlauf, leer, Leerraum vorn/hinten, `+10`, `-1`, `1.5`, `1e3`,
    Tab, Vollbreiten-Ziffern
  - `dir_must_be_non_empty_and_absolute`: absolut, UNC, leer, Leerraum,
    relativ, `.\`, `..\`, führendes Leerzeichen, `\wav`, `C:wav`, `%LOCALAPPDATA%\…`
  - `the_default_dir_is_temp_diktier`: `<TEMP>\diktier`, ohne TEMP das
    System-Temp (`std::env::temp_dir` liest nur, verändert nichts)
  - `switch_off_means_no_dump_and_no_checks`,
    `switch_on_without_settings_uses_the_defaults`,
    `valid_settings_are_taken_over` (5000 + eigenes Verzeichnis, Startzeile),
    `each_invalid_value_warns_once_and_falls_back` (je eine Zeile, zwei bei
    zwei ungültigen Werten)
  - `a_small_ring_keeps_its_capacity_and_leaves_foreign_files`: Kapazität 3
    in einem eigenen Temp-Unterverzeichnis. Fremde Dateien und ein junger
    `.part`-Rest bleiben.
  - `capacity_one_and_large_capacity`: Kapazität 1, danach 5000 mit 14
    Dateien, keine gelöscht.
  - Die bestehenden Ring-Tests laufen unverändert mit `DEFAULT_KEEP`.

## ⚠️ Abweichungen und Entscheidungen

- **„Aus“-Zeile nur, wenn der Schalter fehlt.** Ist der Dump aus, schreibt
  der Daemon keine Zeile. Ausnahme: `DIKTIER_DEBUG_WAV_KEEP` oder `_DIR` ist
  gesetzt, `DIKTIER_DEBUG_WAV` aber nicht `1`. Dann kommt
  `Debug-WAV aus: DIKTIER_DEBUG_WAV_KEEP/DIKTIER_DEBUG_WAV_DIR gesetzt, aber DIKTIER_DEBUG_WAV ist nicht 1`
  (Info). Warum: Das ist genau der Fall aus Sol-Review W1 (der Schalter wird
  stillschweigend vorausgesetzt). Normale Nutzer ohne diese Variablen sehen
  keine zusätzliche Zeile. Das passt zum bestehenden Stil
  („Aufnahme-Overlay ausgeschaltet …“). Wer das nicht will, entfernt den
  Zweig `None if self.ignored_settings` in `Resolved::start_line`.
- **Bei ausgeschaltetem Dump werden KEEP/DIR nicht geprüft**, also keine
  Warnzeilen. Sie wirken dann ohnehin nicht, der Hinweis oben reicht.
- **Keine Prozessumgebung in Tests.** Kein Test setzt oder liest die
  Diktier-Variablen, deshalb braucht es auch keine Serialisierung.
  `from_env()` selbst hat keinen eigenen Test: Es ist ein Einzeiler über
  `resolve`.
- **Formatierung:** Einmal lief `cargo fmt` über das ganze Crate, bevor mir
  auffiel, dass das auch Dateien des parallelen WP2c-Agenten berühren kann
  (src/main.rs). Danach habe ich nur noch `rustfmt` auf den eigenen Dateien
  ausgeführt. Falls main.rs zu dem Zeitpunkt unformatiert war, hat fmt es
  umformatiert, inhaltlich aber nichts geändert. Bitte beim WP2c-Abgleich
  im Blick behalten.

## ✅ Gates (wörtlich)

1. `cargo fmt --check` → keine Ausgabe, `fmt exit=0`
2. `cargo clippy --all-targets -- -D warnings` →
   `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 3.45s`, ohne
   Meldung (`clippy exit=0`). Im ersten Lauf gab es einen Befund
   (`nonminimal_bool` in `resolve`), der ist behoben.
3. `cargo test` →
   `test result: ok. 535 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 2.03s`

Umgebung: Nichts in `%LOCALAPPDATA%\diktier` oder `%TEMP%\diktier`
geschrieben (Tests nutzen `tempfile`). Keine Benutzervariablen gesetzt. Die
installierte Version und der Daemon sind unberührt.

## 🔍 Offen

- Den Live-Nachweis der Startzeile mit Testverzeichnis und 5000 liefert
  WP3b Schritt 5 (Integrationsgate), nicht dieses Paket.
- SPEC §10 sagt „Pfad eine Logzeile“ (je Dump) und „eine Startzeile“. Beides
  ist so umgesetzt. Die optionale „aus“-Zeile steht nicht in der Spec. Ob
  sie dort nachgetragen werden soll, entscheidet der Orchestrator.
