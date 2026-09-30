# Nacharbeit zum abschließenden Review (Sol) — Bericht

Basis: `56a6a4e` + Working Tree (WP1–WP4), Review `docs/reviews/impl-clipboard-final-sol.md`.
Stand: 2026-09-25. Nicht committet. Keine Änderung an `docs/SPEC.md`,
`docs/clipboard-restore-plan.md` oder anderen Dateien unter `docs/reviews/`.
Keine Live-Tests (`clipboard_live_*`), kein `--roundtrip`, kein Daemon, kein Fenster.

## ✅ 1. Blocker 1 — Materialisierung verliert das Transkript nie still

**Umsetzung**

- `src/inject/mod.rs`: neuer Typ `TranscriptState { Secured, PromiseOpen(String), Lost(String) }`
  mit `LOST = "Zwischenablage leer — Transkript verloren"`, `lost_message()` und `describe()`.
  `InjectOutcome::Pasted` und `InjectOutcome::CopyOnly` tragen jetzt `transcript`.
- `src/inject/windows.rs`, `TakeFailure`: statt `Foreign | Failed` jetzt
  `Foreign | Untouched | Promised | Lost`.
  - `Untouched`: `GlobalAlloc` vor dem Öffnen, `OpenClipboard`, `EmptyClipboard` **ohne**
    Sequenzänderung. Eigentum, Serve-Text und offenes Versprechen bleiben; das pauschale
    `forget_ownership()` ist weg (`take_clipboard`, Zweig `Filled::NotEmptied` mit `seq == before_seq`).
    `before_seq` wird jetzt im geöffneten Clipboard vor `EmptyClipboard` gelesen.
  - `Promised`: `fill_open_clipboard` (neu: Ergebnis `Filled`). Scheitert nach `EmptyClipboard`
    das eager `SetClipboardData(CF_UNICODETEXT, h)`, setzt die Funktion noch im geöffneten
    Clipboard `SetClipboardData(CF_UNICODETEXT, NULL)`, prüft es über
    `IsClipboardFormatAvailable` und legt den Marker dazu. `take_clipboard` bucht das dann als
    eigenen Inhalt mit `delayed = true` und neuer `expected_seq`.
  - `Lost`: Auch das Rückfall-Versprechen steht nicht, oder `EmptyClipboard` scheiterte mit
    Sequenzänderung. Dann `forget_ownership()`.
- `materialize_transcript` (Trait `ClipboardHost`, `src/inject/protocol.rs`) gibt jetzt
  `TranscriptState` zurück, statt per `eprintln!` zu warnen. Zuordnung über `transcript_state()`:
  `Ok`/`Foreign` → `Secured`, `Untouched`/`Promised` → `PromiseOpen`, `Lost` → `Lost`.
- `src/inject/protocol.rs`, `paste_as_owner`: Das Ergebnis der Materialisierung kommt in den Ausgang.
  Das gilt für alle drei Stellen: `NoPromise`/`Disabled`, `NoReadTimeout` und den Fokuswechsel vor dem Chord.
- `src/daemon/workers.rs`, `paste_report`/`copy_only_report` → `finish_with_transcript`:
  - `PromiseOpen` und `Lost` gehen als Warnung über den Daemon-`Logger`, Format
    `Lauf N: …` (`transcript_warning`).
  - `Lost` macht aus dem Report `InjectReport::Failed { message: "Zwischenablage leer — Transkript verloren (<Grund>)" }`,
    der Tray zeigt `error` (`transcript_report`).
- Fake (`src/inject/fake.rs`): `MaterializeFault { Blocked, EmptyFails, SetFails, SetAndPromiseFail }`
  mit Zähler (`with_materialize_fault(fault, times)`), dazu `materialize_attempts`.

**Tests** (`src/inject/mod.rs`)

- `empty_failure_during_materialization_keeps_promise_and_ownership`: `PromiseOpen`, weiter
  Owner, `promise_recorded`; der Quit danach sichert.
- `set_failure_during_materialization_promises_again`: `PromiseOpen` mit „erneut versprochen“,
  `delayed`, weiter Owner, Marker liegt.
- `set_and_promise_failure_during_materialization_is_lost`: `Lost`, Clipboard leer. Dasselbe am
  `CopyOnly`-Ausgang beim Fokuswechsel vor dem Chord.
- `src/daemon/workers.rs`, `transcript_state_maps_to_report_and_warning`: `Lost` → `Failed`
  mit dem festen Text, `PromiseOpen` → Warnung plus Erfolgs-Report.

## ✅ 2. Blocker 2 — kein offenes Versprechen verschwindet still

**Fehlerausgänge nach `become_owner`.** `inject_paste_inner` ist geteilt: Alles nach
`become_owner` läuft in `paste_as_owner` (`src/inject/protocol.rs`). Jeder `Err` daraus ruft vor
der Rückgabe `materialize_transcript()` auf, also Shortcut, Pump, `still_owner` und
`wait_for_first_read`. Der ursprüngliche Fehler bleibt der Ausgang. Nur bei `Lost` wird
`; Zwischenablage leer — Transkript verloren (…)` angehängt.

**Idle-Retry** (`src/daemon/workers.rs`)

- `PromiseRetry` ist die reine Zeitplan-Logik (`restart`, `due`, `record`):
  `PROMISE_RETRY_INTERVAL` = 500 ms und `PROMISE_RETRY_LIMIT` = 10.
  - Der erste Versuch kommt frühestens 500 ms nach dem Lauf, denn der Lauf hat selbst schon materialisiert.
  - Nach dem 10. Fehlschlag kommt genau eine Warnung: „Transkript nach 10 Versuchen nicht eager
    hinterlegt — Versprechen bleibt offen, Zwischenablage kann beim Beenden leer sein (…)“.
  - `Secured` und `Lost` beenden die Versuche; `Lost` wird ebenfalls gewarnt.
- `idle_promise_step(sink, retry, now)` läuft in der Worker-Schleife nach `serve_for(10 ms)`.
  Nach jedem `Paste`/`CopyOnly` wird `restart` aufgerufen.
- Neue `OutputSink`-Methoden in `src/inject/mod.rs`, jeweils mit Default:
  - `pending_promise()`: Win32 prüft erst `delayed`, dann Owner und Sequenz über `is_still_owner`.
  - `materialize_pending()`.
  - `take_warnings()` (siehe Punkt 4).
- Ein fremder Copy bedeutet: `pending_promise() == false`, es wird nichts versucht. Kommt der fremde
  Copy genau im Übergang, stoppt die Sequenzprüfung in `take_clipboard` den Versuch (`Foreign` → `Secured`).

**Quit**

- `save_transcript_on_quit` (`src/inject/protocol.rs`, generisch, Win32 und Fake). Es
  unterscheidet `NotOwner` von `PromiseForeign` (neue Variante von `ClipboardSave`: Versprechen offen,
  aber fremd überschrieben), und zwar über die neue Host-Methode `promise_recorded()`.
  Bei blockiertem Clipboard versucht es bis zur Frist erneut, alle 100 ms (`QUIT_RETRY_SLICE`),
  mit Pumpen dazwischen. Danach `Err` mit Grund; bei `Lost` `Err` mit dem Verlust-Text.
- `InjectCmd::SaveTargets` trägt jetzt `budget`. Das ist die Daemon-Wartezeit minus 300 ms,
  höchstens 1,5 s, damit die Antwort noch ankommt. Die Antwort ist `Result<ClipboardSave, String>`.
- `InjectWorker::save_targets` → `Result<ClipboardSave, String>`:
  - Timeout ergibt `Ok(Timeout)`.
  - Nicht erreichbar oder ohne Antwort beendet ergibt `Err` (bisher `NotOwner`).
- `Daemon::shutdown` (`src/daemon/mod.rs`) nutzt `workers::quit_save_log`.
  - **Warnung**, Text „Transkript beim Beenden nicht gesichert — Zwischenablage kann leer sein (<Grund>)“:
    `Timeout`, `PromiseForeign`, `Refused` und jedes `Err`.
  - **Info**: `Saved` („Clipboard beim Beenden gesichert“), `NotOwner` und `NoManager`.
  - Die frühere zweite Warnung im Worker entfällt; die Zeile steht genau einmal im Log.

**Tests**

- `shortcut_failure_materializes_before_returning` (Erfolg, blockiert → Versprechen bleibt,
  `Lost` → Text angehängt)
- `quit_retries_a_blocked_promise_within_the_budget` (Retry sichert, Frist abgelaufen → `Err`,
  Verlust)
- `quit_distinguishes_not_owner_from_a_foreign_overwritten_promise`
- in `src/daemon/workers.rs`, Modul `tests::promise`:
  - `the_retry_schedule_is_bounded`
  - `idle_retry_secures_the_promise`
  - `idle_retry_leaves_a_foreign_copy_alone`
  - `idle_retry_warns_once_after_ten_failures`
  - `shortcut_failure_then_blocked_quit_logs_the_warning` (Shortcut-Fehler + blockiertes
    Clipboard + Quit → exakte Warnzeile)
- `quit_save_lines_warn_on_every_unsecured_outcome`

Die Worker-Tests nutzen `FakeHost` als `OutputSink`: `impl OutputSink for FakeHost` in `fake.rs`,
dafür ist `inject::fake` jetzt `pub(crate)` unter `cfg(test)`.

## ✅ 3. Blocker 3 — Debug-WAV kollisionssicher

`src/daemon/debug_wav.rs`

- `create_part`: legt die Temp-Datei `<ziel>.<pid>-<zähler>.part` exklusiv an, über
  `create_private_file` = `File::options().write(true).create_new(true)`. Der Zähler ist ein
  prozessweiter `AtomicU64`. Ist der Name belegt, kommt der nächste Zählerstand dran, höchstens
  1000 Versuche. Eine fremde oder alte `.part` wird nie trunkiert. Im Fehlerfall wird nur die
  eigene, gerade angelegte Temp-Datei gelöscht.
- `finalize_unique` + `rename_no_replace`: `MoveFileExW` **ohne** `MOVEFILE_REPLACE_EXISTING`
  (mit `MOVEFILE_WRITE_THROUGH`). Das Feature `Win32_Storage_FileSystem` war schon aktiv, der
  Kommentar in `Cargo.toml` ist ergänzt. Bei `AlreadyExists` kommt der nächste Suffix `-2`, `-3` …
  Außerhalb von Windows gibt es als Rückfall `hard_link` + `remove_file`.
- `parse_name` → `RingName { stamp, run, suffix }`. Das Muster ist
  `rec_<stamp>_lauf-<N>[-<k>].wav` mit `k ≥ 2` ohne führende Null. Der Ring sortiert nach
  `(stamp, run, suffix)`.
- `is_own_part`: erkennt `<ringname>.<pid>-<n>.part` und die alte Form `<ringname>.part`.
  Weiter gilt die 1-h-Altersregel.
- Kleinigkeit: `is_stamp` prüft zusätzlich die Kalendergrenzen (Monat 1–12, Tag 1–31,
  Stunde 0–23, Minute/Sekunde 0–59, ms 0–999).

**Tests**

- `an_existing_final_name_is_never_replaced` (Datei und Verzeichnis am Zielnamen → `-2`,
  Original unverändert)
- `existing_part_files_are_neither_truncated_nor_deleted` (alte `.part` plus die nächsten 8
  Zählerstände vorbelegt, alle bleiben byte-gleich)
- `the_same_run_and_time_twice_gives_two_files`
- `suffixed_dumps_are_younger_than_their_base`
- `own_part_names_follow_the_ring_pattern`
- Kalender- und Suffixfälle in `only_the_exact_pattern_belongs_to_the_ring`

**Abweichung.** `a_failed_write_counts_nothing_and_deletes_nothing` erzwang den Fehler bisher
über ein Verzeichnis am Zielnamen. Das ist jetzt eine Kollision und ergibt `-2`. Der Test injiziert
deshalb ein scheiterndes Finalisieren (`write_recording_with`, nur intern).

## ✅ 4. Marker und Snapshot bei `CopyOnly`

- `InjectOutcome::CopyOnly { reason, history_excluded, snapshot: Option<SnapshotReport>, transcript }`.
  Der Snapshot ist `Some`, wenn er vor dem Fokuswechsel schon lief, also nach dem Snapshot und
  vor dem Chord, sonst `None`.
- Worker (`paste_report`): schreibt die Snapshot-Zeile, falls vorhanden, dann
  `copy_only: <Grund>` mit `· Verlauf ausgeschlossen: nein`, wenn der Ausschluss fehlt (`with_history`).
- Tray-Klick-Pfad: `OutputSink::copy_only` liefert jetzt `Copied { history_excluded, transcript }`,
  die Logzeile ist `copy_only · N Bytes[ · Verlauf ausgeschlossen: nein]`.
- Die Warnungen aus `Win32OutputSink::new` (Registrierung), `marker_handle` (`GlobalAlloc`) und
  `place_marker` (`SetClipboardData`) sammelt der Sink in `warnings`. Dasselbe gilt für
  „Clipboard-Restore unterblieben: …“ aus `restore_snapshot`.
- `OutputSink::take_warnings()` gibt sie heraus. Der Worker loggt sie nach dem Anlegen des Sinks,
  nach jedem Kommando und im Idle als Warnung.
- CLI-Pfade ohne Logger (`clipboard_check`, `clipboard_roundtrip`, `--inject-test`) geben sie
  auf stderr aus.
- **Abweichung vom Vorschlag:** `take_warnings() -> Vec<String>` statt `take_startup_warning()`,
  weil auch die Marker-Fehler pro Lauf ins Log sollen. `Drop` meldet weiter über `eprintln!`, dort
  gibt es keinen Logger mehr.
- Tests: `copy_only_carries_marker_and_snapshot`, `copy_only_lines_name_a_missing_history_exclusion`.

## ✅ 5. `GdiFlush`

`src/overlay/windows.rs`, `render_notice_text`: Liefert der Aufruf 0, gibt es
`OverlayError` „GdiFlush fehlgeschlagen: Win32-Fehler …“. Damit greift der Glyphen-Fallback mit
Warnung. Dafür gibt es keinen eigenen Test, denn GDI lässt sich ohne Fenster nicht gezielt zum
Scheitern bringen.

## ✅ 6. README

- „Das erste Diktat“ Punkt 4 und der Abschnitt „Zwischenablage“: Das Restore kommt nur nach
  bedientem Read und mit `restore_clipboard = true`, eventuell nur teilweise (Hinweiskarte). Der
  Win+V-Ausschluss gilt nur, „sofern Diktier den Ausschluss-Marker setzen konnte“; sonst stehen
  Logzeile und Warnung im Log.
- „Debug-WAV“: Suffix `-2`/`-3`, nie überschreiben, `.part`-Reste erst ab einer Stunde.
- „Wenn etwas nicht klappt“:
  - Neu: der Satz zur 5-s-Wartezeit, auch bei `restore_clipboard = false` oder wenn nichts
    sicherbar ist; sie entfällt bei normalem Einfügen.
  - Neu: „Zwischenablage leer — Transkript verloren“.
  - Neu: die Quit-Warnung.

## ✅ 7. Roundtrip-Budget

- `src/inject/formats.rs`: `read_saved` liefert `SavedRead { formats, unchecked }`.
  - `unchecked` sind gesicherte Formate, die wegen Zeitbudget, `ByteBudget`/`TimeBudget` oder
    einer Länge über dem Restbudget nicht geprüft wurden.
  - Echte Lesefehler bleiben „nicht lesbar“.
- `verify_budget` = Summe der gesicherten Längen + 64 KiB je Format (`VERIFY_SLACK_PER_FORMAT`).
- `compare_roundtrip(…, unchecked)` meldet `unchecked` nicht als Abweichung.
- `read_after` nimmt `(id, länge)`; `Roundtrip.unchecked` ist neu.
- `src/main.rs`: gibt „Nicht geprüft (Budget erschöpft): …“ aus. „Byte-identisch“ kommt nur
  ohne Abweichung **und** ohne ungeprüfte Formate, sonst „keine Abweichung, aber nicht alle
  Formate geprüft“ mit Exitcode 3.
- Test: `exhausted_verify_budget_is_unchecked_not_a_mismatch`. Die Live-Tests prüfen zusätzlich
  `unchecked.is_empty()`; ausgeführt wurden sie nicht.

## Weitere Abweichungen (bewusst, klein)

- **Nach dem eigenen `CloseClipboard` ist ein anderer Owner da.** Bisher gab es
  `Failed("Clipboard-Ownership nicht übernommen")`, jetzt `TakeFailure::Foreign`.
  - `become_owner`/`copy_transcript` melden dann „Clipboard zwischenzeitlich fremd geändert“.
  - Die Materialisierung wertet es als `Secured`, weil dann fremder Inhalt liegt.
- **Auch `copy_transcript` (CopyOnly) bekommt das Rückfall-Versprechen.** Es liefert
  `Ok(PromiseOpen)`, danach übernimmt der Idle-Retry. Ist nichts mehr zu retten (Clipboard nach
  `EmptyClipboard` leer), kommt `Err` mit „Zwischenablage leer — Transkript verloren (…)“. Das gilt
  auch für `become_owner`, wenn `EmptyClipboard` die Sequenz änderte oder das Delayed-Versprechen
  nicht steht. Bisher lautete der Text nur „EmptyClipboard: …“ bzw. „Delayed Rendering … nicht registriert“.
- `ClipboardSave::Timeout.as_str()` heißt jetzt „keine Antwort des Inject-Workers innerhalb der
  Frist“ statt „… des Clipboard-Managers“. Auf Windows entsteht der Timeout nur beim Warten auf den Worker.
- **Der Quit nutzt die Restzeit für Retries.** Der Review-Vorschlag „beim Quit verbleibende Zeit
  für einen Retry nutzen“ stand nicht wörtlich im Auftrag. Ich habe ihn mitgenommen, weil der Quit
  sonst bei kurz blockiertem Clipboard sofort scheitert.

## SPEC-Widersprüche (Orchestrator zieht nach)

1. **§10 Debug-WAV:**
   - Das Muster kennt keinen Kollisionssuffix `-<k>`.
   - „`.part`-Reste werden gelöscht“ ohne die 1-h-Altersregel (die galt schon vor dieser
     Nacharbeit im Code).
   - Die Temp-Form `<ziel>.<pid>-<n>.part` und das Finalisieren ohne Ersetzen fehlen.
2. **§7.1 / §10 Inject-Fehlerbild:**
   - Neu ist der Tray-`error` „Zwischenablage leer — Transkript verloren (<Grund>)“ (Ausgang `Lost`).
   - Neu sind der Idle-Retry (500 ms, höchstens 10 Versuche, dann Warnung) und das
     Rückfall-Versprechen nach gescheitertem eager `SetClipboardData`.
3. **§7.1 Punkt 8 / Quit:**
   - Die Warnzeile „Transkript beim Beenden nicht gesichert — Zwischenablage kann leer sein (<Grund>)“
     gilt für Timeout, Fehler und `PromiseForeign`.
   - Der Retry innerhalb der Frist ist in der SPEC nicht beschrieben.
4. **§9 Exitcodes `--clipboard-check`:** Ein Roundtrip mit ungeprüften Formaten (Budget erschöpft)
   endet mit `3`, obwohl „Verlust“ nicht ganz zutrifft; die SPEC kennt „nicht geprüft“ nicht.

## Gates (wörtlich)

1. `cargo fmt --check` → keine Ausgabe, `fmt exit=0`
2. `cargo clippy --all-targets -- -D warnings` → ohne Meldung, letzte Zeile
   `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 2.26s`, `clippy exit=0`
3. `cargo test` → `test result: ok. 497 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 0.27s`
   `cargo test --no-run` → `Finished \`test\` profile [unoptimized + debuginfo] target(s) in 0.22s`,
   `Executable unittests src\main.rs (target\debug\deps\diktier-07675913812095fa.exe)`, `no-run exit=0`
4. `cargo test -- --ignored stt_smoke_fixtures` → `test engine::tests::stt_smoke_fixtures ... ok`,
   `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 506 filtered out; finished in 17.73s`
5. `cargo build --release` → `Finished \`release\` profile [optimized] target(s) in 14.34s`, `release exit=0`

Zum Vergleich: Das Review zählte 476 Tests, jetzt sind es 497 (21 neue: 7 in `inject`, 1 in `formats`, 5 in `debug_wav`, 8 in `workers`; dazu erweiterte bestehende Tests).

## 🔍 Offen

- **Win32-Fehlerpfade sind nur über den Fake belegt.** Das betrifft das Rückfall-Versprechen in
  `fill_open_clipboard`, `Untouched` bei `EmptyClipboard` ohne Sequenzänderung und `GdiFlush`.
  `MoveFileExW` ohne Ersetzen ist dagegen echt getestet, weil die Debug-WAV-Tests auf die Platte
  schreiben. Kein Live-Test wurde ausgeführt, G1/G3/G4/G5 stehen weiter aus und brauchen die
  gesonderte Zustimmung.
- **Pump-Fehler** nach `become_owner` laufen über denselben Fehlerzweig wie der Shortcut-Fehler.
  Einen eigenen Fake-Schalter „nur `pump` scheitert“ gibt es nicht, deshalb auch keinen eigenen Test.
- **`Lost` im Idle-Retry** meldet der Worker nur als Warnung im Log. Der Lauf ist dann schon
  abgeschlossen, einen nachträglichen Tray-`error` gibt es nicht. Soll das ein eigenes Kern-Event
  werden, ist das eine Entscheidung für den Orchestrator.
- Die 5-s-Wartezeit bei `NoPromise`/`Disabled` ohne Read bleibt wie beauftragt. Die README nennt
  sie jetzt.

---

# Nacharbeit 2 (nach `impl-clipboard-final-sol-2.md`)

Stand 2026-09-25, gleiche Regeln: nicht committet, keine Live-Tests, kein `--roundtrip`, kein
Daemon, kein Fenster. SPEC, Plan und andere Review-Dateien sind unverändert.

## ✅ N2-1. Blocker 1 — Verlust im Idle-Retry wird ein Kern-Ereignis

- **`src/state.rs`**
  - Neu ist `Event::TranscriptLost { run, message }`.
  - In `transition`: Nur aus `AppState::Idle` ruft der Kern
    `enter_error(ErrorKind::Inject, message)` auf. Der Tray zeigt dann `error` mit
    „Zwischenablage leer — Transkript verloren (<Grund>)“, der Hotkey bleibt scharf wie bei jedem
    Inject-Fehler, und das nächste Diktat startet normal.
  - In jedem anderen Zustand (recording, transcribing, injecting, error, …) gibt es keinen
    Wechsel. Stattdessen kommt `LogEvent::TranscriptLostWhileBusy { run, state }`, im Log als
    „Transkript von Lauf N nachträglich verloren — kein Fehlerzustand, weil gerade <zustand> läuft“
    (`src/daemon/logging.rs`, `describe`). Ein späteres Nachholen, sobald der Kern wieder idle
    ist, gibt es nicht; so war es beauftragt.
  - Der Kern prüft `run` nicht gegen `runtime.run`: `finish_run` zählt die Laufnummer beim
    Abschluss schon weiter, der ursprüngliche Lauf ist also nie mehr der aktuelle.
    Entscheidend ist nur „gerade idle“. `run` steht zur Zuordnung im Event und in der Logzeile.
- **`src/daemon/workers.rs`**
  - `PromiseRetry` merkt sich den Lauf (`restart(now, run)` nach jedem `Paste`/`CopyOnly`).
  - `idle_promise_step` liefert `IdleReport { warning, lost: Option<Event> }`. Bei
    `TranscriptState::Lost` schickt die Worker-Schleife
    `Event::TranscriptLost { run: <ursprünglicher Lauf>, message }` an die Event-Loop, zusätzlich
    zur Log-Warnung.
- **Tests**
  - `state::tests::transcript_lost_while_idle_enters_inject_error`: idle → error Inject, Hotkey
    scharf, Effekte `Failure` + Tray, danach startet Press die Aufnahme.
  - `state::tests::transcript_lost_while_recording_only_logs`: kein Wechsel, Laufnummer und
    `error` unverändert, nur die Logzeile.
  - `workers::tests::promise::idle_retry_loss_becomes_a_core_event`: `Blocked` im Lauf,
    `SetAndPromiseFail` beim ersten Idle-Retry, dann `Event::TranscriptLost` für den
    ursprünglichen Lauf, danach Ruhe.

## ✅ N2-2. Blocker 2 — Quit mit absoluter, monotoner Deadline

- **`src/inject/protocol.rs`, `save_transcript_on_quit(host, deadline: Instant)`**
  - Nach jedem Materialisierungsversuch und nach jedem Pump liest die Funktion `Instant::now()`
    neu. Nach Ablauf beginnt kein Versuch und kein Warten mehr.
  - Die Pump-Scheibe ist `min(100 ms, deadline − jetzt)`.
  - Ein Read während des Pumps macht das Versprechen eager; dann kommt `Saved` bzw.
    `PromiseForeign` (`settled`).
  - Der **erste** Versuch läuft auch dann, wenn die Frist beim Eintreffen schon vorbei ist. Das
    ist die letzte Chance vor dem Fensterabbau, und der Daemon wartet dann ohnehin nicht mehr.
    Überziehen kann die Frist also höchstens um die Dauer eines Versuchs.
- **`OutputSink::save_to_clipboard_manager(&mut self, deadline: Instant)`** (vorher `Duration`):
  Win32, Fake und Stub sind angepasst.
- **`src/daemon/workers.rs`**
  - `InjectCmd::SaveTargets { reply, deadline }`.
  - `InjectWorker::save_targets(timeout)` rechnet alles von einem `Instant` aus: Antwort-Ende
    `reply_by = gesendet + timeout`, Worker-Deadline `reply_by − SAVE_TARGETS_MARGIN`, gekappt auf
    `gesendet + 1,5 s`, nie vor `gesendet`. `recv_timeout` wartet genau bis `reply_by`.
- **Marge (dokumentiert an `SAVE_TARGETS_MARGIN`):** 300 ms. Der schlechteste einzelne Win32-Versuch
  ist `open_clipboard` mit 10 × `OpenClipboard` und 9 × 10 ms Pump, also rund 90–110 ms mit
  Timer-Auflösung, dazu `EmptyClipboard`/`SetClipboardData` und das Senden. Die Marge ist das
  Dreifache davon.
- **Quit während eines Pastes im 5-s-Fenster ohne Read (erwartetes Verhalten)**
  - `SaveTargets` liegt in der Queue hinter dem Paste. Der einzige Inject-Worker bearbeitet es
    erst, wenn der Paste zurückkehrt; `NoReadTimeout` materialisiert dabei selbst.
  - Reicht die Frist, meldet Quit `Saved` (Info).
  - Reicht sie nicht, meldet `save_targets` `Timeout`, und `Daemon::shutdown` loggt die Warnung
    „Transkript beim Beenden nicht gesichert — Zwischenablage kann leer sein (keine Antwort des
    Inject-Workers innerhalb der Frist)“. Danach folgen `InjectWorker::shutdown` und der `Drop`
    mit dem letzten Versuch.
  - Ein Abbruch des laufenden Pastes ist nicht eingebaut (erklärtes Verhalten, siehe README).
- **Fake (`src/inject/fake.rs`):**
  - `with_materialize_cost(d)`: Jeder Versuch schläft real `d`.
  - `with_real_pump(n)`: `pump(t)` schläft real `t / n`, als Zeitraffer.
  - `OutputSink::current_window_id` liefert das Fake-Fenster, damit der Worker-Test wirklich
    einfügt statt `CopyOnly`.
- **Worker per Fake:** `InjectWorker::spawn_with(make, …)` ist intern und nimmt einen generischen
  Sink. Er entsteht auf dem Worker-Thread; `spawn` ruft `spawn_with(new_sink)` auf.
- **Tests**
  - `inject::tests::quit_deadline_counts_the_real_cost_of_each_attempt`: 90 ms je Versuch,
    500 ms Frist. Dauer ≥ 500 ms und < 500 + 90 + 60 ms, 5–6 Versuche. Bei schon verstrichener
    Frist genau ein Versuch.
  - `inject::tests::quit_retries_a_blocked_promise_within_the_budget`: umgestellt auf reale
    Pump-Zeit. Die Frist von 250 ms wird eingehalten (< 400 ms), mindestens drei Quit-Versuche.
  - `workers::tests::promise::quit_during_the_read_window_waits_or_warns`, echter Worker-Thread
    mit Fake-Sink und Zeitraffer 10 (5-s-Fenster ≈ 0,5 s real), drei Fälle:
    1. Frist 2 s → `Saved`, Info.
    2. Frist 150 ms → `Timeout`, exakt die Warnzeile.
    3. Clipboard blockiert und 90 ms je Versuch, Frist 1,2 s → die Antwort `Err(„…nicht zu
       öffnen…“)` kommt **vor** dem `recv_timeout`, die Warnzeile enthält den Grund.
  - Die zeitabhängigen Tests liefen fünfmal hintereinander grün
    (`cargo test --quiet -- quit_deadline quit_retries quit_during_the_read_window idle_retry_loss`
    → jeweils `test result: ok. 4 passed; 0 failed; … finished in 1.95s`–`1.96s`).

## ✅ N2-3. README — Read-Heuristik ist keine Bestätigung

- „Das erste Diktat“ Punkt 4: „Nach einem bedienten Clipboard-Read und der Mindestwartezeit …
  Der Read ist kein Beweis für erfolgreiches Einfügen — auch ein Clipboard-Manager kann ihn
  auslösen.“
- „Zwischenablage“: dieselbe Formulierung, dazu „Kommt innerhalb von 5 Sekunden keiner, bleibt
  das Transkript in der Zwischenablage.“
- Troubleshooting zur 5-s-Wartezeit: „Kommt nach dem Einfügen kein bedienter Clipboard-Read …“,
  der Hinweis, dass auch ein Beenden solange wartet, und „ein Beweis für erfolgreiches Einfügen
  ist er nicht“.
- Hinweistabelle „Einfügen nicht bestätigt“: „innerhalb von 5 s kein Clipboard-Read“ statt „das
  Ziel hat den Text nicht abgeholt“.
- Troubleshooting „Transkript verloren“: Das kann auch Sekunden nach dem Diktat passieren; läuft
  schon das nächste Diktat, steht es nur im Log.

## ✅ N2-4. `is_stamp` — Monatslängen und Schaltjahr

- `src/daemon/debug_wav.rs`: Neu ist `days_in_month(year, month)`, gregorianisch (durch 4, nicht
  durch 100 außer durch 400). Der Tag ist nur bis zur Monatslänge gültig.
- Test `impossible_calendar_dates_are_foreign`:
  - Gültig sind 2028-02-29, 2000-02-29, 02-28, 04-30, 01-31 und 12-31.
  - Fremd sind 2026-02-29, 1900-02-29, 02-30, 02-31, 04-31, 06-31, 09-31 und 11-31.
  - Der eigene Stempel für 2028-02-29T12:00Z wird als Ring-Name erkannt.

## Abweichungen

- **Die Signatur von `save_to_clipboard_manager` hat sich geändert** (`Instant` statt `Duration`).
  Der Trait ist nur intern.
- **Erster Quit-Versuch auch nach Ablauf der Frist**, siehe N2-2. „Kein neuer Versuch nach
  Ablauf“ gilt für die Wiederholungen.
- **Der Kern gleicht `TranscriptLost` nicht mit der aktuellen Laufnummer ab**, siehe N2-1:
  `finish_run` hat sie schon weitergezählt.

## SPEC-Widersprüche (zusätzlich zu oben)

1. **§5.2 Zustandsautomat:** Das Event `TranscriptLost` (idle → error, Inject) und die Logzeile
   für den Fall „gerade beschäftigt“ fehlen.
2. **§7.1 Punkt 8 / Quit:** Absolute Deadline, 300-ms-Marge, der letzte Versuch nach Ablauf und
   das Verhalten bei `SaveTargets` hinter einem laufenden Paste sind nicht beschrieben.
3. **§10 Debug-WAV:** Die SPEC sagt „mit gültigen Kalenderwerten“ (Zeile 787 f.). Der Code liest
   das jetzt als Monatslänge plus gregorianisches Schaltjahr. Das ist kein Widerspruch, höchstens
   eine Präzisierung.

## Gates (wörtlich)

1. `cargo fmt --check` → keine Ausgabe, `fmt exit=0`
2. `cargo clippy --all-targets -- -D warnings` → ohne Meldung, `Finished \`dev\` profile
   [unoptimized + debuginfo] target(s) in 0.21s`, `clippy exit=0`. Der erste Lauf meldete
   `int_plus_one` in einem Test und `manual_is_multiple_of` in `days_in_month`; beides ist
   behoben, danach erneut ausgeführt.
3. `cargo test` → `test result: ok. 503 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 2.01s`.
   `cargo test --no-run` → `Finished \`test\` profile [unoptimized + debuginfo] target(s) in
   0.21s`, `Executable unittests src\main.rs (target\debug\deps\diktier-07675913812095fa.exe)`,
   `no-run exit=0`.
4. `cargo test -- --ignored stt_smoke_fixtures` → `test engine::tests::stt_smoke_fixtures ... ok`,
   `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 512 filtered out; finished in 17.11s`
5. `cargo build --release` → `Finished \`release\` profile [optimized] target(s) in 15.57s`,
   `release exit=0`

Die Testlaufzeit steigt von 0,3 s auf 2,0 s; das sind die realzeitlichen Quit-Tests.

## 🔍 Offen (Nacharbeit 2)

- Das Rückfall-Versprechen, `Untouched` und `GdiFlush` sind weiter nur über den Fake bzw.
  statisch geprüft. Live-Gates G1/G3/G4/G5 brauchen die gesonderte Zustimmung.
- Ein Quit während eines laufenden Pastes wartet, bis der Paste fertig ist (bis 5 s). Reicht die
  Frist nicht, bleibt nur die Warnung und der letzte Versuch im `Drop`. Einen Abbruch des Pastes
  gibt es nicht.
- Der Punkt „`Lost` im Idle-Retry nur als Log-Warnung“ aus dem ersten Offen-Abschnitt ist durch
  N2-1 erledigt, soweit der Kern idle ist.
