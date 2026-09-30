# Umsetzungsnotizen: Clipboard-Restore WP4 (`type` raus, Debug-WAV-Ring, Doku, 0.4.0)

Basis: Working Tree mit den uncommitteten WP1–WP3-Ergebnissen. Nicht
committet. Kein `clipboard_live_*`, kein `--roundtrip`, kein Eingriff in die
installierte Version oder den laufenden Daemon (PID 31916 aus
`%LOCALAPPDATA%\Programs\Diktier` lief während der Arbeit weiter).

Geänderte Dateien: `src/config.rs`, `src/daemon/debug_wav.rs`,
`src/daemon/workers.rs`, `src/daemon/logging.rs` (eine Sichtbarkeit),
`src/daemon/mod.rs` (nur `overlay_view` samt Tests), `README.md`,
`docs/windows-plan.md`, `Cargo.toml`, `Cargo.lock` (von `cargo build`
nachgezogen). `cargo fmt` lief über die ganze Crate, hat aber nur meine
Dateien berührt (per Änderungszeit geprüft).

## ✅ Umgesetzt

### `output.mode` (Leitentscheidung 10, `src/config.rs`)

- `OutputMode::Type` entfällt; `OutputMode` hat nur noch `Paste` (Muster wie
  `HotkeyMode::PushToTalk`). Außerhalb von `config.rs` gab es keine Zweige
  auf `Type`, es war also nichts Totes weiter zu entfernen.
- `"type"` → `ConfigError::Fatal` mit genau
  `output.mode "type" gibt es nicht mehr — bitte "paste" eintragen oder die Zeile löschen`
  (Konstante `REMOVED_TYPE_MODE`). Der Daemon zeigt sie wie jeden Config-Fatal
  im Log (`log.error`) und im Tray-Tooltip (`config_error_mode`).
- Anderer unbekannter Wert → Fatal `ungültiges output.mode "…" (v1 nur paste)`.
- Default-Datei: `mode = "paste"          # v1 nur dieser Wert`.
- Tests: `removed_type_mode_is_fatal_with_migration_hint` (Meldung per
  `assert_eq!`), `paste_mode_is_accepted`, `missing_output_mode_means_paste`
  (fehlender Schlüssel und leere Datei), `default_file_documents_the_single_output_mode`,
  der bestehende `fatal_invalid_output_mode` (`"clipboard"`).

### Debug-WAV-Ring (Leitentscheidung 11, SPEC §10, `src/daemon/debug_wav.rs`)

- `file_name(at, run)` → `rec_<YYYY-MM-DDThh-mm-ss-mmmZ>_lauf-<N>.wav`. Die
  UTC-Umrechnung nutzt `logging::civil_from_days` (dafür `pub(super)`), keine
  neue Dependency.
- `write_recording(dir, samples, run, at)`: `<ziel>.part` schreiben, Rename.
  Erst **nach** erfolgreichem Rename wird aufgeräumt:
  - Ring: Dateien des genauen Musters (`parse_name`: Präfix, 24-stelliger
    Zeitstempel mit fester Zeichenmaske, `_lauf-`, nur Ziffern, `.wav`, nur
    reguläre Dateien) über `KEEP = 10` werden gelöscht, älteste zuerst.
    „Älteste“ heißt nach Zeitstempel im Namen, bei Gleichstand nach
    Laufnummer **numerisch**; nicht nach Änderungszeit.
  - `.part`-Reste des genauen Musters, die älter als eine Stunde sind, werden
    entfernt. Sie zählen nie zum Ring. Fremde `.part` bleiben.
  - `last_recording.wav` wird entfernt (siehe Abweichung 2).
- Scheitert Schreiben oder Rename, wird nur der eigene `.part` entfernt;
  Ring, Altlast und alte Reste bleiben unberührt, der Fehler geht als
  `DIKTIER_DEBUG_WAV fehlgeschlagen: …` ins Log (wie bisher).
- `workers.rs`: `dump_debug_wav(run, samples, log)`; die Laufnummer kommt aus
  `AudioCmd::Stop { run, .. }`, dieselbe, die `log.run` als `Lauf N:` schreibt.
  Die Logzeile bleibt `DIKTIER_DEBUG_WAV: <pfad>`.
- Tests (alle in `tempfile::tempdir()`, nie in `%TEMP%\diktier`):
  - `the_file_name_carries_utc_to_the_millisecond_and_the_run` (Beispiel aus
    dem Plan exakt, Epoch, Schalttag 2000-02-29T23:59:59.999)
  - `only_the_exact_pattern_belongs_to_the_ring` (11 fremde Namen)
  - `writes_the_wav_atomically`
  - `twelve_dumps_leave_the_ten_newest` (G5 im Kleinen)
  - `the_oldest_go_first_by_name_not_by_mtime` (gleiche ms, Lauf 9 vor 10;
    Änderungszeiten absichtlich vertauscht)
  - `foreign_files_are_neither_counted_nor_deleted` (inkl. Verzeichnis mit
    Musternamen)
  - `stale_part_leftovers_are_removed_fresh_and_foreign_ones_stay`
  - `the_legacy_dump_is_removed_after_a_successful_dump`
  - `a_failed_write_counts_nothing_and_deletes_nothing` (Zielname als
    Verzeichnis belegt → Rename scheitert; zehn Ring-Dateien, Altlast und
    alter `.part`-Rest bleiben, eigener `.part` ist weg)
  - `a_missing_directory_is_created`, `clipping_stays_in_range`

### WP1-Nacharbeiten in `src/daemon/workers.rs`

- **`history_excluded` in der Paste-Zeile:** `paste_log_line(…)` hängt
  ` · Verlauf ausgeschlossen: nein` nur an, wenn `clipboard.history_excluded`
  false ist. Test `the_paste_line_names_a_missing_history_exclusion_only`.
- **Quit-Warnung:** Liefert `save_to_clipboard_manager` im Worker einen
  Fehler, loggt er jetzt
  `Transkript beim Beenden nicht gesichert — Zwischenablage kann leer sein (<fehler>)`
  statt `SAVE_TARGETS: <fehler>`. Test `a_failed_quit_save_is_an_unambiguous_warning`.
- **Snapshot-Zeile bei `CopyOnly` nach dem Snapshot: nicht umgesetzt.**
  `InjectOutcome::CopyOnly` (`src/inject/mod.rs:198`) trägt nur `reason`, der
  Report wird in `protocol.rs::inject_paste_inner` verworfen. Ohne Änderung
  an `src/inject/*` (gesperrt) kommt `workers.rs` nicht an ihn heran. Siehe
  Offen.

### `overlay_view` (`src/daemon/mod.rs`)

- Die Zeile `_ if runtime.paused => OverlayView::Hidden` ist entfernt.
  Reihenfolge jetzt: `quitting` → `error` → aktive Zustände (Pegel) →
  Hinweis in `idle` → verborgen, `paused` spielt keine Rolle. Doc-Kommentar
  angepasst.
- Tests: `expected_view` ohne `paused`-Zweig (Vollmatrix 8 × 11 × 2 × 2
  bleibt), Eckpunkt `Idle` + Hinweis + pausiert → `Notice(TrayCopy)`, neuer
  Kerntest `a_tray_dictation_while_paused_shows_its_notice`: Pause an →
  verborgen; Tray-Diktat → Pegel; `InjectFinished(CopyOnly(TrayClickPath))` →
  Hinweis sichtbar trotz Pause; Pause aus → Hinweis gelöscht, verborgen.

### README, windows-plan, Version

- README: Versionszeile v0.4.0; „Das erste Diktat“ Punkt 4 nennt den
  Mehrformat-Restore und Win+V; neuer Abschnitt „`output.mode`: nur noch
  `"paste"` (Breaking Change in 0.4.0)“ mit Meldung und Migration; neuer
  Abschnitt „Zwischenablage“ (was wiederhergestellt wird, Verlaufs- und
  Cloud-Ausschluss — Diktate und Wiederherstellungen erscheinen in Win+V
  nicht mehr, keine Zusage für Drittanbieter-Manager; Grenzen aus F1:
  OLE-Objekte/Outlook-Elemente, Excels Kopierrahmen, synthetisch ersetzte
  Bildformate; Hinweiskarte mit allen sieben Texten aus SPEC §4.5 und den
  Fällen ohne Hinweis; `--clipboard-check` mit Exitcodes;
  `--clipboard-check --roundtrip` mit Warnkasten); neuer Abschnitt
  „Debug-WAV“ (Muster, zehn Dateien, `.part`, Altlast, Einschalten als
  Benutzervariable); Troubleshooting-Stichpunkt zur `type`-Meldung; Log-Punkt
  um „nur Metadaten“ ergänzt.
- `docs/windows-plan.md`: WP6-Absatz um die 0.4.0-Notiz ergänzt.
- `Cargo.toml` 0.4.0; `Cargo.lock` ändert beim Paket `diktier` nur die
  Version (der übrige Lock-Diff stammt aus WP3, `png`).

## Abweichungen und warum

1. **`.part`-Reste mit Altersgrenze eine Stunde.** SPEC §10 sagt nur
   „`.part`-Reste werden gelöscht“, der Plan (Leitentscheidung 11) nennt
   „`.part`-Reste älter als eine Stunde“. Ich habe die Stunde übernommen: Sie
   widerspricht der SPEC nicht (Reste verschwinden trotzdem) und schützt
   einen Schreibvorgang, der gerade läuft. Der eigene `.part` ist zum
   Aufräumzeitpunkt schon umbenannt.
2. **Altlast „einmalig“.** `last_recording.wav` wird nach **jedem**
   erfolgreichen Dump per `remove_file` entfernt (NotFound ignoriert), nicht
   über ein Prozess-Flag. Weil nichts die Datei mehr erzeugt, ist das
   faktisch einmalig und ohne globalen Zustand testbar.
3. **Unbekannter `output.mode`-Wert:** Meldung um `(v1 nur paste)` ergänzt,
   analog zu `hotkey.mode`. Plan und SPEC legen dafür keinen Wortlaut fest.
4. **Quit-Warnung nur bei Fehler im Worker.** Antwortet der Inject-Worker
   nicht innerhalb des Budgets (er hängt z. B. in einem Paste), liefert
   `InjectWorker::save_targets` weiter `Timeout`, und `Daemon::shutdown`
   loggt wie bisher `Clipboard beim Beenden: timeout`. Diese Zeile steht in
   `src/daemon/mod.rs`, das außer `overlay_view` gesperrt war. Nach der neuen
   Warnung folgt dort zusätzlich diese Info-Zeile (der Fehlerpfad gibt
   weiter `ClipboardSave::Timeout` zurück).

## Gate-Ausgaben (wörtlich)

1. `cargo fmt --check` → keine Ausgabe, `fmt exit 0`
2. `cargo clippy --all-targets -- -D warnings` →
   ```
       Checking diktier v0.4.0 (D:\DEV\diktier)
       Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.08s
   ```
   keine Meldung, `clippy exit 0`
3. `cargo test` →
   `test result: ok. 476 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 0.28s`
4. `cargo test -- --ignored stt_smoke_fixtures` →
   ```
   test engine::tests::stt_smoke_fixtures ... ok
   test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 485 filtered out; finished in 17.20s
   ```
5. `cargo build --release` →
   ```
      Compiling diktier v0.4.0 (D:\DEV\diktier)
       Finished `release` profile [optimized] target(s) in 15.04s
   ```
   `target/release/diktier.exe --version` → `diktier 0.4.0`

## Offen

- **Snapshot-Report bei `CopyOnly` nach dem Snapshot** (Sol-Review
  Kleinigkeit 3): braucht einen optionalen `ClipboardReport` in
  `InjectOutcome::CopyOnly` bzw. dessen Durchreichen in
  `protocol.rs::inject_paste_inner` (Fokuswechsel nach `become_owner`).
  Danach in `workers.rs` im `CopyOnly`-Arm dieselbe
  `snapshot_log_line`-Zeile wie beim Paste schreiben. Kleines Folgepaket in
  `src/inject/*`.
- **Quit-Timeout ohne Worker-Antwort** (Abweichung 4): Soll auch
  `Clipboard beim Beenden: timeout` zur eindeutigen Warnung werden, ist das
  eine Zeile in `Daemon::shutdown` (`src/daemon/mod.rs`).
- **Wiederholung einer an `OpenClipboard` gescheiterten Quit-Sicherung** mit
  kurzer Wartezeit (Vorschlag aus WP1-Nacharbeit 2): nicht umgesetzt, gehört
  nach `src/inject/windows.rs`.
- **G5 live** (zwölf Diktate mit `DIKTIER_DEBUG_WAV=1`) und G3/G4 stehen aus;
  sie brauchen den installierten 0.4.0-Daemon bzw. Ralfs Zustimmung.
- README-Aussagen zu Outlook, Excel und ausgeschnittenen Explorer-Dateien
  folgen Plan F1 und G4 #5/#7; sie sind noch nicht live bestätigt und nach G4
  gegebenenfalls nachzuschärfen.
