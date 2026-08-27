# Phase 5, Paket C (WP4 + WP5) — Implementierungsnotizen

Stand: 2026-08-27, Implementierer: Claude Opus 5.0 (Windows 11, MSVC x64,
Rust 1.95.0). Working-Tree, **nicht** committet.

Auftrag: [windows-plan.md](../windows-plan.md) WP4 (Tray `Shell_NotifyIconW`)
und WP5 (Single-Instance, Signale, Subsystem, Autostart), Leitlinie „nicht
überengineeren — lauffähiger Dev-Milestone".

## Geänderte Dateien

| Datei | Änderung |
|---|---|
| `Cargo.toml` | vier neue windows-sys-Features (`Win32_UI_Shell`, `Win32_System_Console`, `Win32_Security`, `Win32_Storage_FileSystem`), Kommentartabelle für WP4/WP5 fortgeschrieben |
| `src/tray.rs` | neues `#[cfg(windows)] mod windows` (~620 Zeilen), `AnyTray::Win32`, Windows-`new_backend`, `open_config_dir` per `explorer.exe`, 8 neue Tests |
| `src/single_instance.rs` | Windows-Zweig als Named Mutex (`mod win`), `FileLock` → `PathLock`, `acquire_instance_lock`/`try_lock`/`acquire_all` plattformgetrennt, 3 neue Tests, 8 Tests auf `cfg(target_os = "linux")` |
| `src/daemon/signals.rs` | `#[cfg(windows)] mod imp` mit `SetConsoleCtrlHandler` statt des leeren `cfg(not(unix))`-Stubs |
| `src/main.rs` | `#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]`, `attach_parent_console()` als erster Schritt in `main`, SNI-Diagnosezeile im Tray-Spike auf Linux beschränkt |
| `src/autostart.rs` | `AutostartError::ExeHasQuote`, Doc zu „ohne `/min`", plattformneutraler Testmarker, 2 neue Windows-Tests |

`git diff --stat` über genau diese Dateien: 1455 Einfügungen, 35 Löschungen.
Nichts aus Paket A (`src/hotkey.rs`) oder B (`src/inject/*`) angefasst;
`src/daemon/workers.rs` blieb unverändert (siehe „Abweichung 1").

---

## ✅ WP4 — Tray (`src/tray.rs`, `mod windows`)

- **Verstecktes Top-Level-Fenster** `DiktierTrayOwner`, `WS_POPUP` ohne
  `WS_VISIBLE`, `WS_EX_TOOLWINDOW`, Parent `NULL`. Kein `HWND_MESSAGE` — nur
  echte Top-Level-Fenster bekommen `TaskbarCreated` und `WM_ENDSESSION`.
- **Anlegen**: `RegisterWindowMessageW("TaskbarCreated")` (0 → Fehler),
  `Shell_NotifyIconW(NIM_ADD)` mit `NIF_MESSAGE|NIF_ICON|NIF_TIP|NIF_SHOWTIP`,
  danach `NIM_SETVERSION(NOTIFYICON_VERSION_4)`. Beide Rückgabewerte geprüft;
  scheitert `NIM_SETVERSION`, wird das gerade angelegte Icon per `NIM_DELETE`
  zurückgenommen, bevor `TrayError::Failed` hochgeht (§10 → Exit 1).
- **`TaskbarCreated`** wiederholt die komplette Sequenz aus dem gemerkten
  Icon/Tooltip; ein Fehlschlag ist nur eine stderr-Zeile (der Daemon läuft
  weiter, §10 macht ausdrücklich nur den Aufbau **beim Start** fatal).
- **`update`** setzt Tooltip (`tooltip_text`, identisch zu Linux) und Icon je
  `TrayStatus` und ruft `NIM_MODIFY` **direkt**. Geprüft und dokumentiert: Der
  `update`-Aufruf läuft in beiden Aufrufern auf dem Owner-Thread —
  `daemon/workers.rs::tray_loop` erzeugt das Backend, ruft `update`/`poll` und
  lässt es am Schleifenende fallen, alles im Thread `diktier-tray`; der Spike
  `--tray-test` macht dasselbe im Hauptthread. Deshalb **kein** Mutex-Slot und
  kein `PostMessageW(WM_APP+2)` wie im Plan skizziert (Abweichung 1).
- **Tooltip-Kürzung** `tooltip_utf16`: 127 UTF-16-Codeunits + NUL, geschnitten
  an Zeichengrenzen (kein halbes Surrogatpaar).
- **Icons**: ein `HICON` je Zustand, eine Größe `GetSystemMetrics(SM_CXSMICON)`,
  aus 32-bpp-Top-down-`CreateDIBSection` (gefüllter Kreis, Alpha 0/255,
  vorpremultipliziert) plus genullter Monochrom-Maske über
  `CreateIconIndirect`. Beide `HBITMAP`s sofort `DeleteObject`; die `HICON`s
  erst nach `NIM_DELETE`. Farben exakt wie das Linux-`IconSet`.
- **`poll`**: `PeekMessageW`-Pump (max. 256 Nachrichten je Aufruf, kein
  `TranslateMessage`) plus `VecDeque`-Queue im gemeinsamen `RefCell`-Zustand.
- **Callback (Version 4)**: `LOWORD(lParam)` = Ereignis, `wParam` = Anker.
  `NIN_SELECT`/`NIN_KEYSELECT` → `LeftClick` (genau einmal, `WM_LBUTTONUP`
  wird nicht zusätzlich ausgewertet), `WM_CONTEXTMENU` → Menü.
- **Menü**: `CreatePopupMenu`, IDs 1000 Status (`MF_STRING|MF_GRAYED`), 1001
  Pausieren/Aktivieren (Text aus `pause_menu_label(paused)`), 1002
  Config-Ordner öffnen, `MF_SEPARATOR`, 1003 Beenden. Ablauf nach der
  §4.2-Ausnahme: `SetForegroundWindow(hwnd)` →
  `TrackPopupMenu(TPM_RETURNCMD|TPM_RIGHTBUTTON|TPM_NONOTIFY, x, y)` →
  `PostMessageW(hwnd, WM_NULL)` → `NIM_SETFOCUS` → `DestroyMenu` → Mapping über
  `menu_action` + `route_menu`. Über `TrackPopupMenu` hinweg wird **kein**
  `RefCell`-Borrow gehalten (es pumpt intern und ruft den `WndProc` erneut).
- **Session-Ende**: `WM_QUERYENDSESSION` → `TRUE`; `WM_ENDSESSION` mit
  `wParam != 0` → `TrayEvent::Quit` in die Queue (der Daemon behandelt das
  bereits), `wParam == 0` ignoriert.
- **`Drop`** (auf dem Owner-Thread): `NIM_DELETE` (mit stderr-Zeile
  „Tray: Icon entfernt (NIM_DELETE)"), `DestroyWindow`, `UnregisterClassW`,
  `DestroyIcon`.
- **`open_config_dir`**: `explorer.exe <dir>` per `std::process::Command`,
  ohne `wait()` (der Explorer meldet gern Exitcode 1, und Windows kennt keine
  Zombies). Der Stub-Fehler ist weg — damit auch die vorbestehende Warnung
  `unreachable expression` in `tray.rs:268`.

### Abweichungen vom Plan (WP4)

1. **Kein Command-Channel, kein `WM_APP+2`, kein Stop-Command im `Drop`.** Der
   Plan beschreibt `update` als „Mutex-Slot + `PostMessageW`" und `Drop` als
   „Stop-Command senden und joinen". Beides setzt einen **eigenen** Tray-Thread
   im Backend voraus — den gibt es nicht: der Thread gehört bereits dem
   `TrayWorker` in `daemon/workers.rs`, und der ruft `new`, `update`, `poll`
   und `drop` alle auf sich selbst (Zeilen 1090–1160). Ein zweiter Thread wäre
   eine Umleitung ohne Nutzen und würde `daemon/workers.rs` anfassen, das laut
   Auftrag unberührt bleiben soll. Dokumentiert im Modul-Doc.
2. **`EndMenu` im `Drop` weggelassen.** Es kann kein Menü offen sein, wenn
   `Drop` läuft: `TrackPopupMenu` blockiert innerhalb von `poll()`, und erst
   danach kommt die Schleife wieder zum Shutdown-Command.
3. **`AnyTray::Stub` entfernt** statt behalten — wie Paket A bei
   `AnyHotkeyBackend::Stub`. Die Variante wäre auf beiden Plattformen tot.
   `StubTray` selbst bleibt als Vertragsprobe im Testmodul.
4. **`NIN_KEYSELECT` als eigene Konstante** (`NIN_SELECT + 1`): windows-sys
   0.61 exportiert `NIN_SELECT`, aber nicht `NIN_KEYSELECT`. Der Wert
   (`WM_USER + 1`) ist stabile `shellapi.h`-ABI — dieselbe Begründung wie bei
   `CF_UNICODETEXT` in `inject/windows.rs`.
5. **Kein `catch_unwind` im `WndProc`.** Paket A hat das im Hook-Proc gemacht,
   `inject/windows.rs` nicht. Hier wird stattdessen durchgängig `try_borrow`/
   `try_borrow_mut` benutzt; alle anderen Operationen sind panikfrei.

---

## ✅ WP5

### Single-Instance (`src/single_instance.rs`)

- Neues `#[cfg(windows)] mod win`: RAII-`MutexLock` (`CloseHandle` im `Drop`),
  `create(name)` → `Held`/`Busy`. `CreateMutexW(NULL, FALSE, name)`,
  `GetLastError` **sofort** danach gelesen (es trägt `ERROR_ALREADY_EXISTS`
  auch bei Erfolg); Handle `NULL` → `LockError::Mutex` mit dem Win32-Fehler
  (dort landet auch `ERROR_INVALID_HANDLE`, also „anderer Objekttyp trägt
  diesen Namen"); `ERROR_ALREADY_EXISTS` → `Busy`.
- **Instanzsperre**: fester Name `Local\FerberDiktier.v1.instance`. `describe()`
  liefert ihn — im Log steht jetzt `Sperre gehalten:
  Local\FerberDiktier.v1.instance`.
- **Download-Lock**: `try_lock(path)` ist auf Windows der pfadgebundene Mutex
  `Local\FerberDiktier.v1.download.<hex(sha256(pfad))>`, Pfad vorher
  kleingeschrieben (Windows-Pfade sind case-insensitiv). `download.rs` blieb
  unverändert — es ruft weiter `single_instance::try_lock(lock_path)`.
- **Struktur**: `FileLock` heißt jetzt `PathLock` und hat cfg-abhängige
  Innereien (`File` bzw. `MutexLock`); `InstanceLock` ebenso (`Vec<PathLock>`
  bzw. ein `MutexLock`). `acquire_all`, `identity`, `instance_lock_candidates`,
  `open_lock_file` und `flock_exclusive_nonblocking` sind jetzt Linux-only —
  auf Windows gibt es genau **ein** Sperrobjekt, keine Kandidatenliste, keine
  Sperrdatei.

**Warum der größere Eingriff statt „nur `flock_exclusive_nonblocking`
ersetzen":** Die kleinere Variante hätte den Instanz-Mutex nach dem
Sperrdateipfad benannt (`%LOCALAPPDATA%\diktier\diktier.lock`) statt nach dem
im Auftrag genannten festen Namen und hätte auf Windows weiter eine nutzlose
Datei angelegt. Der jetzige Zuschnitt ist ehrlicher und macht `describe()`
brauchbar; der Preis sind acht `cfg`-Attribute an Tests.

**Testlage der 8 Fehlschläge:**

| Test | Ergebnis |
|---|---|
| `single_instance::second_lock_on_same_path_is_busy` | ✅ grün, unverändert (Mutexname aus dem Pfad ⇒ zweiter Versuch im selben Prozess ist `Busy`) |
| `single_instance::lock_is_released_when_dropped` | ✅ grün, unverändert |
| `single_instance::instance_lock_takes_every_candidate` | `#[cfg(target_os = "linux")]` |
| `single_instance::duplicate_candidates_do_not_lock_the_process_out` | `#[cfg(target_os = "linux")]` |
| `single_instance::a_process_locking_only_the_fallback_blocks_a_process_with_both` | `#[cfg(target_os = "linux")]` |
| `single_instance::a_busy_candidate_releases_the_locks_already_taken` | `#[cfg(target_os = "linux")]` |
| `download::parallel_download_is_refused_while_the_lock_is_held` | ✅ grün, unverändert |
| `autostart::install_updates_a_moved_binary_instead_of_duplicating` | ✅ grün, Marker plattformabhängig |

Begründung für die vier `cfg(linux)`: Sie prüfen ausschließlich die
Mehr-Orte-Logik (`$XDG_RUNTIME_DIR` **und** State-Verzeichnis, Symlink-Dedup,
Freigabe schon genommener Sperren, Ausweichen bei unbenutzbarem Ort). Auf
Windows gibt es diese Logik nicht und soll es nach §5.3 auch nicht geben.
Zusätzlich `cfg(linux)` bekamen `stale_lock_file_does_not_block`,
`lock_file_is_created_with_private_mode` (Dateimodus `0600`),
`an_unusable_candidate_is_skipped_not_fatal` und `no_usable_candidate_is_an_error`.

Der Autostart-Test prüfte „genau ein `Exec=`" — ein Desktop-Entry-Muster. Die
Regel aus §9 („eigenen Eintrag aktualisieren, nicht verdoppeln") ist
plattformneutral, die Schreibweise nicht; der Test benutzt jetzt einen
`ENTRY_MARKER` (`Exec=` bzw. `start "" `).

### Signale (`src/daemon/signals.rs`)

`SetConsoleCtrlHandler` (Rückgabewert geprüft, Fehlschlag nur eine
stderr-Zeile). `CTRL_C_EVENT`/`CTRL_BREAK_EVENT`/`CTRL_CLOSE_EVENT` setzen
dasselbe `AtomicBool` wie im Unix-Zweig und geben `TRUE` zurück; alles andere
`FALSE`. `take_quit_request()` unverändert semantisch. **Kein**
Cleanup-Ack-Warten bei `CTRL_CLOSE_EVENT` (Auftrag; offener Punkt unten).

### Subsystem/Konsole (`src/main.rs`)

- `#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]`. Das
  `not(test)` ist nötig: ohne das erbt der Test-Harness das GUI-Subsystem und
  `cargo test` liefe stumm.
- `attach_parent_console()` als **erste** Anweisung in `main`, vor Clap und vor
  `signals::install()`: `AttachConsole(ATTACH_PARENT_PROCESS)`, kein
  `AllocConsole`. Danach werden `STD_INPUT/OUTPUT/ERROR_HANDLE` **nur dann**
  aus `CONIN$`/`CONOUT$` neu gesetzt, wenn `GetStdHandle` NULL oder
  `INVALID_HANDLE_VALUE` liefert — eine Umleitung des Aufrufers (`> log.txt`,
  Pipe) bleibt unangetastet.

Der Zusatzschritt war **nötig**: ein GUI-Subsystem-Prozess bekommt beim Start
aus cmd.exe keine Standardhandles, `AttachConsole` setzt sie nicht, und ohne
das bliebe jedes `eprintln!` stumm. Nachgemessen (Gate unten): In einer echten
Konsole erscheinen stdout **und** stderr, und `%ERRORLEVEL%` stimmt.

### Autostart (`src/autostart.rs`)

`.cmd` bleibt, `/min` war nie drin (Doc ergänzt, Test prüft es). Pfade mit `"`
werden jetzt mit `AutostartError::ExeHasQuote` (Exit 1) abgelehnt statt eine
`.cmd` zu schreiben, die cmd.exe falsch parst.

### Cargo.toml

Neu: `Win32_UI_Shell` (Notify-Icon), `Win32_System_Console`
(`SetConsoleCtrlHandler`, `AttachConsole`, `Get/SetStdHandle`),
`Win32_Security` (windows-sys deklariert `CreateMutexW` mit
`*const SECURITY_ATTRIBUTES` und blendet es ohne dieses Feature aus — übergeben
wird `NULL`), `Win32_Storage_FileSystem` (`CreateFileW` für `CONOUT$`/`CONIN$`).
`Win32_Graphics_Gdi` war schon da und wird jetzt wirklich gebraucht.
`Cargo.lock` unverändert (nur Features, kein neuer Knoten).

---

## Tests

11 neue, alle rein (kein Win32-Aufruf):

- `tray::tests::win`: `menu_ids_map_to_their_actions`,
  `unknown_menu_ids_do_nothing` (inkl. `0` = „daneben geklickt"),
  `menu_ids_reach_the_tray_events`, `short_tooltips_survive_unchanged_with_nul`,
  `long_tooltips_are_cut_to_127_code_units`,
  `tooltips_never_split_a_surrogate_pair`,
  `an_error_tooltip_is_cut_but_stays_valid`,
  `every_status_has_its_own_icon_slot_and_color`.
- `single_instance::tests::win`: `the_instance_mutex_is_session_local_and_fixed`,
  `download_mutex_names_are_derived_from_the_path`,
  `the_same_path_in_another_case_is_the_same_mutex`.
- `autostart::tests`: `windows_entry_quotes_the_path_and_stays_unminimized`,
  `windows_rejects_a_path_with_a_quote`.

Keine Testabstraktion über Win32 gebaut (Auftrag).

---

## Gates

### ✅ `cargo build --release`

Grün, **eine** Warnung — vorbestehend, keine neue. Die zweite Warnung aus
Paket A (`unreachable expression` in `tray.rs:268`) ist mit dem echten
`open_config_dir` verschwunden:

```
warning: function `x11_keysym` is never used
  --> src\hotkey.rs:73:8
warning: `diktier` (bin "diktier") generated 1 warning
    Finished `release` profile [optimized] target(s)
```

### ✅ `cargo test` — 0 failed

```
test result: ok. 297 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out
```

Vorher (Stand Paket A/B): `284 passed; 8 failed`.

### ✅ `cargo fmt --check`

Sauber. `rustfmt --edition 2024` lief nur über `src/tray.rs` und
`src/single_instance.rs`, kein `cargo fmt` über den Baum.

### ✅ Smoke `--tray-test 15 --foreground`

```
SPIKE --tray-test (kein Produktionspfad)
SPIKE tray-backend=shell-notifyicon
SPIKE tray: zustand=starting tooltip=starting — parakeet-tdt-0.6b-v3-int8
SPIKE tray: zustand=downloading tooltip=downloading — parakeet-tdt-0.6b-v3-int8
SPIKE tray: zustand=loading tooltip=loading — parakeet-tdt-0.6b-v3-int8
SPIKE tray: zustand=idle tooltip=idle — parakeet-tdt-0.6b-v3-int8
SPIKE tray-test: 15s vorbei
Tray: Icon entfernt (NIM_DELETE)
ExitCode=0
```

`NIM_ADD`, `NIM_SETVERSION`, vier `NIM_MODIFY` und `NIM_DELETE` haben alle
Erfolg gemeldet (jeder Fehlschlag hätte den Spike mit Exit 1 beendet). Ob das
Icon **sichtbar** ist, kann nur ein Mensch prüfen — offener Punkt.

### ✅ Zweiter Daemon-Start → Exit 0

```
erste Instanz laeuft: True
zweite Instanz ExitCode=0
--- stderr der zweiten Instanz ---
diktier läuft bereits — dieser Start endet ohne Wirkung.
--- Log der ersten Instanz ---
[+   0.002s] Sperre gehalten: Local\FerberDiktier.v1.instance
```

### ✅ Ctrl+C: sauberes Ende, Icon abgeräumt, Konsolenausgabe sichtbar

Aufbau: `cmd.exe` mit eigener (versteckter) Konsole startet
`diktier --foreground` **ohne** Umleitung; nach 12 s `AttachConsole` +
`GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0)`; danach der Konsoleninhalt per
`ReadConsoleOutputCharacterW` ausgelesen. Damit ist zugleich bewiesen, dass
`AttachConsole` greift — ein nicht angehängter GUI-Subsystem-Prozess bekäme
gar kein `CTRL_C_EVENT`.

```
diktier PID=36000 laeuft nach 12 s
GenerateConsoleCtrlEvent=True
diktier beendet=True
--- Konsoleninhalt ---
[+   0.001s] Sperre gehalten: Local\FerberDiktier.v1.instance
[+   0.013s] Tray-Backend: shell-notifyicon
[+   0.014s] Hotkey-Backend: win32-ll-hook (F9, Push-to-Talk)
[+   2.153s] Modell geladen in 2.138 s (parakeet-tdt-0.6b-v3-int8)
[+   2.153s] Zustand: idle
[+  12.021s] Signal empfangen — Beenden angefordert
[+  12.022s] Kern hat das Beenden bestätigt
[+  12.038s] Clipboard beim Beenden: kein Clipboard-Eigentum
Tray: Icon entfernt (NIM_DELETE)
[+  12.157s] beendet
```

Kein Panic, kein `STATUS_CONTROL_C_EXIT` (Paket A sah dort noch
`0xC000013A`), kein hängender Prozess.

### ✅ Konsolenausgabe und Exitcodes im GUI-Subsystem

Aus derselben echten Konsole, ohne Umleitung:

```
BAT-MARKER-VOR
diktier 0.1.0
EXITCODE=0
error: unexpected argument '--gibtsnicht' found
Usage: diktier.exe [OPTIONS]
For more information, try '--help'.
EXITCODE2=2
```

Aus **Git Bash** ebenfalls vollständig (`--version` → `diktier 0.1.0`,
Exit 0; unbekanntes Flag → Clap-Fehler, Exit 2). `windows_subsystem` bleibt
also drin.

### ✅ Autostart Install/Remove zweimal, Pfad mit Leerzeichen im Test

```
--install-autostart exit=0 : Autostart angelegt: …\Startup\diktier.cmd
  Inhalt: @echo off | start "" "D:\dev\diktier\target\release\diktier.exe" |
--install-autostart exit=0 : Autostart unverändert: …\Startup\diktier.cmd
--remove-autostart  exit=0 : Autostart entfernt: …\Startup\diktier.cmd
--remove-autostart  exit=0 : Autostart war nicht vorhanden: …\Startup\diktier.cmd
Rest im Startup-Ordner: An OneNote senden.lnk, Ollama.lnk, subst_b.cmd,
                        whisper-dictate.vbs, Wispr Flow.lnk
```

Kein `/min`, fremde Einträge unberührt, nach dem Test kein `diktier.cmd` mehr.

### ❌ `cargo check --target x86_64-unknown-linux-gnu` — weiterhin nicht ausführbar

Unverändert zu Paket A: `onig_sys` und `ring` brauchen einen Linux-C-Cross-
Compiler, der auf diesem Rechner fehlt. Dieses Paket fasst Linux-sichtbaren
Code an (Umbenennung `FileLock` → `PathLock`, `cfg`-Aufteilung in
`single_instance.rs`, `AnyTray` ohne `Stub`, `ENTRY_MARKER`), das Gate ist
also diesmal **wichtiger** als bei A. Siehe offene Punkte.

---

## Offene Punkte

### 🔍 Sichtprüfung des Trays (nur durch einen Menschen)

Icon sichtbar; Tooltip `idle — …`; Rechtsklick zeigt alle vier Einträge und
schließt bei einem Klick daneben; Linksklick startet/stoppt eine Aufnahme;
„Hotkey pausieren" gibt F9 frei; „Config-Ordner öffnen" startet den Explorer;
„Beenden" räumt das Icon weg; `taskkill /f /im explorer.exe && start explorer`
bringt es zurück; DPI 100 % und 150 %.

### 🔍 Linux-Gate

`cargo check` (oder `cargo test`) auf Mint bzw. einer Maschine mit
Linux-C-Toolchain. Erwartung: grün, unveränderte Warnungslage; die
Linux-Pfade in `single_instance.rs`/`tray.rs`/`autostart.rs` sind logisch
unverändert, aber die `cfg`-Aufteilung ist nur auf Windows kompiliert worden.

### 🔍 Kein Cleanup-Ack bei `CTRL_CLOSE_EVENT`

Auftragsgemäß weggelassen. Heute setzt der Handler nur das Flag und gibt
`TRUE` zurück; Windows beendet den Prozess danach, ohne auf den §5.2-Shutdown
zu warten. Beim Fensterschließen kann also der Tray-`Drop` (`NIM_DELETE`) und
der Clipboard-Quit-Pfad ausfallen. Fehlt: `CreateEventW`-Ack, das der Daemon
nach seinem regulären Shutdown setzt, und ein `WaitForSingleObject(3 s)` im
Handler.

### 🔍 Ausgabe geht verloren, wenn cmd.exe umleitet

`cmd /c diktier.exe --version > datei` schreibt **nichts** und wartet nicht:
cmd behandelt ein GUI-Subsystem-Programm wie `start` und reicht die
Umleitungshandles nicht durch. Betroffen ist nur dieser Weg — echte Konsole,
Git Bash und `Start-Process -Wait -RedirectStandardError` funktionieren.
Gehört als Bedienhinweis in die README (WP6), zusammen mit dem schon im Plan
notierten „PowerShell wartet nicht auf GUI-Subsystem-Programme".

### 🔍 Logoff-Test steht aus

`WM_QUERYENDSESSION`/`WM_ENDSESSION` sind implementiert, aber nur durch
Code-Lesen belegt — einmal wirklich ab- und wieder anmelden.

### 🤔 Fehlgeschlagenes `NIM_MODIFY` meldet bei jedem Update

Bleibt die Shell dauerhaft unerreichbar (Explorer tot, `TaskbarCreated` nicht
angekommen), liefert `update` bei jedem Zustandswechsel `TrayError::Failed`,
und `tray_loop` schreibt jedes Mal eine Warnzeile. Kein Fehlerfall im
Normalbetrieb; ein Entprellen wäre der saubere Folgeschritt.

### 🤔 Kein `ChangeWindowMessageFilterEx` für `TaskbarCreated`

Läuft diktier je erhöht, kommt der Broadcast wegen UIPI nicht an und das Icon
kehrt nach einem Explorer-Neustart nicht zurück. Für den Dev-Milestone
irrelevant (der Daemon läuft nicht erhöht), aber vor einem Release zu
entscheiden.

### 🤔 `x11_keysym is never used` auf Windows

Unverändert die einzige Warnung; Vorschlag aus Paket A (Verschieben unter
`cfg(target_os = "linux")`) weiterhin offen, weil es Linux-Code anfasst.

### 🤔 WP-Status im Plan

`docs/windows-plan.md` führt WP1–WP7 alle noch als 🔍. Weder Paket A noch B
haben die Überschriften aktualisiert; dieses Paket auch nicht, um dem
Orchestrator nicht in ein parallel bearbeitetes Dokument zu schreiben.
