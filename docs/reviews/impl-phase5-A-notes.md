# Phase 5, Paket A (WP1 + WP2) — Implementierungsnotizen

Stand: 2026-08-27, Implementierer: Claude Opus 5.0 (Windows 11, MSVC x64,
Rust 1.95.0). Working-Tree, **nicht** committet.

Auftrag: [windows-plan.md](../windows-plan.md) WP1 (Cargo/cfg) und WP2
(Hotkey `WH_KEYBOARD_LL`), mit den Blockern aus
[plan-phase5-sol.md](plan-phase5-sol.md).

## Geänderte Dateien

| Datei | Änderung |
|---|---|
| `Cargo.toml` | `betrayer` nach `[target.'cfg(target_os = "linux")'.dependencies]`; neuer Block `[target.'cfg(windows)'.dependencies]` mit `windows-sys = "=0.61.2"` (5 Features) |
| `Cargo.lock` | genau **eine** neue Zeile: `"windows-sys 0.61.2"` in den `diktier`-Dependencies |
| `src/hotkey.rs` | neues `#[cfg(windows)] mod windows` (~470 Zeilen), 11 neue Tests, `AnyHotkeyBackend::WinHook`, Windows-`new_backend` |

`cargo fmt` ist gelaufen (`cargo fmt --check` war auf `HEAD` sauber und ist
es danach wieder).

`git diff --stat`: 951 Einfügungen, 14 Löschungen. Die 14 Löschungen sind
ausschließlich die `AnyHotkeyBackend::Stub`-Variante samt ihrer fünf
Match-Arme und der alte Windows-Stub-`new_backend` — **kein** Byte in
`mod linux`.

## ✅ WP1 — Cargo/cfg-Grundlagen

`betrayer` steht jetzt im Linux-Block; der Kommentar nennt den Grund
(Leitentscheidung 1: Windows bekommt `Shell_NotifyIconW` direkt, betrayer
brächte dort einen zweiten Eventloop mit).

Neuer Windows-Block mit den Features, die **WP2 wirklich aufruft** — die
vollständige Tabelle für WP3–WP5 bleibt im Plan:

| Feature | wofür in WP2 |
|---|---|
| `Win32_Foundation` | `WPARAM`/`LPARAM`/`LRESULT`, `GetLastError` |
| `Win32_UI_WindowsAndMessaging` | `SetWindowsHookExW`, `CallNextHookEx`, `UnhookWindowsHookEx`, `GetMessageW`, `PeekMessageW`, `PostThreadMessageW`, `MSG`, `KBDLLHOOKSTRUCT`, `LLKHF_*`, `WM_*` |
| `Win32_UI_Input_KeyboardAndMouse` | `GetAsyncKeyState`, `VK_*` |
| `Win32_System_LibraryLoader` | `GetModuleHandleW` |
| `Win32_System_Threading` | `GetCurrentThreadId` |

**Kein zweiter windows-sys-Baum.** Der Lock-Diff ist vollständig:

```diff
@@ -682,6 +682,7 @@ dependencies = [
   "thiserror 2.0.20",
   "toml",
   "ureq",
+ "windows-sys 0.61.2",
   "x11rb",
  ]
```

Der Knoten `windows-sys 0.61.2` existierte schon (u. a. über cpal, ureq,
tokenizers) und wird wiederverwendet; kein neues Paket, keine zweite
`windows-link`-Kopie. Die im Lock ebenfalls stehenden 0.52.0/0.59.0 sind
vorbestehend und gehören anderen Crates — sie sind durch diese Änderung
weder entstanden noch vermeidbar.

## ✅ WP2 — `WinHookBackend` (`src/hotkey.rs`, `mod windows`)

Aufbau exakt nach Plan:

- **Persistenter Hook-Thread** `diktier-hook`. Er erzwingt zuerst per
  `PeekMessageW(WM_USER, WM_USER, PM_NOREMOVE)` seine Nachrichtenqueue,
  liest dann `GetCurrentThreadId`, installiert den Hook und meldet
  `Ready::Up(thread_id)` bzw. `Ready::Failed(<Win32-Fehler>)` über einen
  Handshake-Channel (Timeout 2 s). Damit kann `PostThreadMessageW` nach dem
  Handshake nicht mehr ins Leere laufen (Sol-Review).
- **Commands** `WM_APP+1/+2/+3` = Install/Remove/Stop, jeweils mit Ack über
  einen zweiten Channel (Timeout 2 s). `register`/`unregister` sind
  idempotent (Frühausstieg über `registered`), passen also zu
  `HotkeyCmd::{Grab,Ungrab}`, die der Worker bei jedem Pause-Wechsel
  schickt. Der Thread bleibt zwischen Grab und Ungrab stehen.
- **`GetMessageW <= 0`** (‑1 = Fehler, 0 = `WM_QUIT`) beendet den Thread;
  danach sieht `poll()` `TryRecvError::Disconnected` und meldet
  `HotkeyError::Failed` → §10: Hotkey aus, Tray-Click bleibt.
- **Hook-Proc** `unsafe extern "system" fn hook_proc`:
  `nCode < 0` → sofort `CallNextHookEx` ohne jede Auswertung;
  `LLKHF_INJECTED | LLKHF_LOWER_IL_INJECTED` → `Pass` → `CallNextHookEx`;
  sonst `HookState::on_event`. Geschluckte Events geben `1` zurück (§4.4).
- **Thread-lokaler State** per `thread_local! { RefCell<Option<HookContext>> }`
  — kein Mutex, kein blockierendes Lock im Callback, die einzige Allokation
  ist der `Sender::send`.
- **Zustandslogik** `HookState::on_event` mit `accepted_down`: Down mit
  exaktem Modifier-Treffer → `Emit(Press)`; weiteres Down → `Swallow`
  (Auto-Repeat); Up bei `accepted_down` → **immer** `Emit(Release)`,
  unabhängig vom aktuellen Modifier-Zustand; sonst `Pass`. `reset()` beim
  Ungrab und beim Stop.
- **Modifier-Vergleich** exakt: `ModifierState::required(&spec.modifiers)`
  gegen `ModifierState::current()` (`GetAsyncKeyState`, High-Bit).
  `VK_CONTROL`/`VK_SHIFT`/`VK_MENU` fassen L und R zusammen, `VK_LWIN|VK_RWIN`
  werden verodert. Lock-Tasten kommen im Typ gar nicht vor und können den
  Vergleich deshalb nicht verfälschen. `Shift+F9` ist bei Config `F9` kein
  Treffer.
- **VK-Mapping** `virtual_key()` analog `x11_keysym`: F1–F24, A–Z, 0–9,
  Space, Tab, Enter, Escape, Backspace, Insert, Delete, Home, End,
  PageUp/Down, Pfeiltasten. Unbekannt → `None` → `HotkeyError::Failed`.
- **Fehlertext** durchgängig `Hotkey nicht verfügbar (<Chord>): <Grund>`,
  mit `Win32-Fehler <code>` aus `GetLastError` als Grund.
- **`Drop`** postet `MSG_STOP` und joint über einen Waiter-Thread mit 2-s-
  Timeout (wie `join_timeout` im Linux-Modul).
- `backend_name()` = `"win32-ll-hook"`; `new_backend` (cfg(windows)) liefert
  `AnyHotkeyBackend::WinHook`.
- Jeder `unsafe`-Block hat einen SAFETY-Kommentar (Stil wie
  `single_instance.rs`/`daemon/signals.rs`).

### Abweichungen vom Plan (mit Begründung)

1. **`AnyHotkeyBackend::Stub` entfernt statt behalten.** Der Plan sagt „liefert
   das neue Backend statt des Stubs"; die Variante wäre danach auf beiden
   Plattformen tot gewesen (neue Dead-Code-Warnung). `StubHotkeyBackend`
   selbst bleibt — als Vertragsprobe im Test — und trägt jetzt
   `#[allow(dead_code)]` plus einen Kommentar, der sagt warum. Das ist die
   einzige Zeile außerhalb von `#[cfg(windows)]`, die Verhalten berührt (und
   auch dort nur die Lint-Sicht).
2. **`on_event` nimmt die Modifier als `impl FnOnce() -> ModifierState`,
   nicht als Wert.** So werden die fünf `GetAsyncKeyState`-Aufrufe nur beim
   Down der richtigen Taste gemacht und nicht bei jedem Tastendruck des
   Systems — der Callback liegt im globalen Eingabepfad und zählt gegen
   `LowLevelHooksTimeout`. Für die Tests ändert sich nichts (`|| mods(..)`
   bzw. `ModifierState::default`).
3. **`try_borrow_mut` statt `borrow_mut` im Callback.** Ein Panic im
   Hook-Proc wäre ein Unwind über die Win32-Grenze (undefiniert) und risse
   den Hook-Thread mit. Reentranz ist hier nicht möglich, der sichere
   Ausgang (`Decision::Pass`) kostet aber nichts.
4. **Schluckt nicht weiter, wenn niemand mehr zuhört.** Schlägt
   `Sender::send` im Callback fehl (Backend fallengelassen, Empfänger weg),
   wird `accepted_down` zurückgesetzt und das Event doch durchgereicht —
   sonst verschwände die Taste stumm aus dem System, bis der Prozess endet.
5. **Datei nicht aufgeteilt.** `src/hotkey.rs` bleibt eine Datei mit
   `mod linux` und `mod windows` nebeneinander; das hält den Linux-Teil
   nachweislich unberührt (siehe Diff oben).
6. **Kein zusätzlicher Auto-Repeat-Filter über `KF_REPEAT`.** `accepted_down`
   erledigt das bereits und ist unabhängig davon, ob Windows das Flag setzt.
   Das `Debounce` aus dem Trait-Umfeld läuft trotzdem in `poll()` mit — wie
   in beiden Linux-Backends — und wird bei jedem Grab/Ungrab zurückgesetzt.

## Tests

11 neue Tests in `hotkey::tests::win`, alle rein (kein Win32-Aufruf):

`config_key_maps_to_the_exact_virtual_key`,
`every_x11_key_also_has_a_virtual_key`, `required_modifiers_are_exact`,
`extra_modifiers_are_not_a_match` (Shift+F9 ≠ F9),
`missing_modifiers_are_not_a_match_either` (Ctrl+F9 ≠ Ctrl+Shift+F9),
`lock_keys_are_ignored`, `auto_repeat_is_swallowed_without_event`,
`release_after_modifier_change_is_still_swallowed_and_reported`,
`reset_releases_a_held_key_to_the_application`,
`injected_events_never_match`, `other_keys_are_never_touched`.

## Gates

### ✅ `cargo build` / `cargo build --release` (Windows, MSVC x64)

Beide grün. Warnungen **von 6 auf 2 gesunken**, keine neue:

```
warning: unreachable expression        (src\tray.rs:268, vorbestehend, WP4)
warning: function `x11_keysym` is never used  (vorbestehend auf Windows)
warning: `diktier` (bin "diktier") generated 2 warnings
    Finished `release` profile [optimized] target(s) in 13.61s
```

Baseline auf `HEAD` (010e914) zum Vergleich: 6 Warnungen — zusätzlich
`Debounce is never constructed`, `methods on_press/on_release are never
used`, `variant Failed is never constructed`, `variants Press and Release
are never constructed`. Alle vier sind weg, weil das Backend sie jetzt
benutzt.

### ✅ `cargo test` (Windows)

```
test result: FAILED. 268 passed; 8 failed; 1 ignored; 0 measured
```

Die 8 Fehlschläge sind **vorbestehend und identisch auf `HEAD`**
(dort `257 passed; 8 failed`) — es sind genau die Single-Instance-/
Download-Lock-/Autostart-Tests, die WP5 grün machen soll:

```
autostart::tests::install_updates_a_moved_binary_instead_of_duplicating
download::tests::parallel_download_is_refused_while_the_lock_is_held
single_instance::tests::a_busy_candidate_releases_the_locks_already_taken
single_instance::tests::a_process_locking_only_the_fallback_blocks_a_process_with_both
single_instance::tests::duplicate_candidates_do_not_lock_the_process_out
single_instance::tests::instance_lock_takes_every_candidate
single_instance::tests::lock_is_released_when_dropped
single_instance::tests::second_lock_on_same_path_is_busy
```

Anmerkung zum Plan-Text „der Stub lässt sie heute fälschlich passieren":
Auf diesem Rechner **fallen** sie, sie passieren nicht. WP5 sollte davon
ausgehen.

Alle Hotkey-Tests grün (`cargo test hotkey`):

```
test hotkey::tests::win::auto_repeat_is_swallowed_without_event ... ok
test hotkey::tests::win::config_key_maps_to_the_exact_virtual_key ... ok
test hotkey::tests::win::every_x11_key_also_has_a_virtual_key ... ok
test hotkey::tests::win::injected_events_never_match ... ok
test hotkey::tests::win::lock_keys_are_ignored ... ok
test hotkey::tests::win::other_keys_are_never_touched ... ok
test hotkey::tests::win::release_after_modifier_change_is_still_swallowed_and_reported ... ok
test hotkey::tests::win::missing_modifiers_are_not_a_match_either ... ok
test hotkey::tests::win::extra_modifiers_are_not_a_match ... ok
test hotkey::tests::win::required_modifiers_are_exact ... ok
test hotkey::tests::win::reset_releases_a_held_key_to_the_application ... ok
test result: ok. 30 passed; 0 failed; 0 ignored; 0 measured; 247 filtered out
```

### ❌ `cargo check --target x86_64-unknown-linux-gnu` — auf diesem Rechner nicht ausführbar

Target ist installiert (`rustup target add x86_64-unknown-linux-gnu`), aber
zwei Build-Skripte brauchen einen Linux-C-Cross-Compiler, der hier fehlt:

```
error: failed to run custom build command for `onig_sys v69.9.3`
error: failed to run custom build command for `ring v0.17.14`
  error occurred in cc-rs: failed to find tool "x86_64-linux-gnu-gcc": program not found
```

**Identisch auf `HEAD` ohne meine Änderung** — also kein Effekt dieses
Pakets, sondern eine Lücke der Werkbank (kein gcc/clang/zig, kein
WSL-Distro auf dem Rechner). `onig_sys` kommt über `tokenizers` →
`parakeet-rs`, `ring` über `ureq`/rustls; beide sind nicht abwählbar.

Ersatzargument, bis das Gate auf einem Linux-Rechner oder der Mint-VM
läuft: Der komplette Diff in `src/hotkey.rs` ist additiv **außer** der
`Stub`-Variante und ihren fünf Match-Armen (siehe „Geänderte Dateien").
`mod linux` ist unverändert, `x11_keysym`, `Debounce`, `HotkeySpec` und der
Trait sind unverändert, das neue Modul liegt vollständig hinter
`#[cfg(windows)]`, und der Linux-`new_backend` ist Zeile für Zeile
derselbe. Das Gate bleibt trotzdem offen (siehe unten).

### ⚠️ Manueller Smoke `--hotkey-test --foreground`

30-s-Lauf aus Git Bash, sauber:

```
SPIKE --hotkey-test (kein Produktionspfad)
SPIKE: F9 30s lang halten/loslassen; Exit mit Ctrl+C
SPIKE hotkey-backend=win32-ll-hook
SPIKE hotkey-test: 30s vorbei
EXIT=0
```

Registrierung erfolgreich, Backend-Name korrekt, Prozess endet nach 30 s
mit Exit 0 — `Drop` hängt also nicht am Join.

Zusätzlich als Negativprobe zum `LLKHF_INJECTED`-Pfad: während eines
zweiten 30-s-Laufs 5× `F9` per `SendKeys` (→ `keybd_event`, injiziert)
geschickt. Im Log **kein einziges** Press/Release — genau das Verhalten,
das WP3 braucht, damit der eigene `SendInput`-Paste-Pfad nicht als Hotkey
gilt.

**Offen bleibt der eigentliche Nachweis**: echtes F9 drücken (Press/Release
im Log) und prüfen, dass F9 Notepad nicht erreicht, Shift+F9 aber schon.
Das kann nur physisch gedrückt werden — übernimmt der Orchestrator.

## Offene Punkte

### 🔍 Gate „echtes F9" steht aus

`--hotkey-test --foreground` starten, F9 halten/loslassen: Log muss
`press (entprellt)` / `release (entprellt)` zeigen. Danach in Notepad
gegenprüfen: F9 kommt **nicht** an, Shift+F9 kommt an.

### 🔍 Linux-Gate steht aus

`cargo check --target x86_64-unknown-linux-gnu` auf einem Rechner mit
Linux-C-Toolchain (oder direkt `cargo check` auf Mint). Erwartung: grün,
unveränderte Warnungslage.

### 🤔 `x11_keysym is never used` auf Windows

Vorbestehende Warnung, jetzt eine von nur noch zweien. Sauber wäre
`#[cfg_attr(windows, allow(dead_code))]` oder das Verschieben unter
`cfg(target_os = "linux")`. Nicht gemacht, weil das Linux-Code anfassen
würde und nicht zum Auftrag gehört — Vorschlag für WP5 oder einen
Aufräum-Commit.

### 🤔 Hotkey-Fehler erreicht den Tooltip noch nicht

`hotkey_loop` schickt bei Fehlern `Msg::HotkeyUnavailable(...)`; der
Fehlertext hat jetzt die Form aus §4.4. Ob der Tooltip ihn tatsächlich
zeigt, hängt am Tray (WP4) und ist hier nicht geprüft.

### ⏸️ `WM_APP`-Kollision

Der Hook-Thread besitzt kein Fenster, seine Thread-Queue gehört nur uns —
`WM_APP+1..3` sind dort kollisionsfrei. WP3/WP4 posten ihre eigenen
`WM_APP+n` an **ihre** Fenster/Threads; die Nummernräume sind getrennt,
müssen aber beim Lesen des Codes nicht verwechselt werden.

## ✅ Gate: physisches F9 (Orchestrator, 2026-08-27 ~11:15)

`diktier.exe --hotkey-test --foreground`, Ralf drückt F9 fünfmal (Binary von
11:10, vor dem Fixpaket):

```
SPIKE hotkey-backend=win32-ll-hook
SPIKE hotkey: press (entprellt)
SPIKE hotkey: release (entprellt)
… (5× Press/Release, keine Doppler)
SPIKE hotkey-test: 30s vorbei
```

Offen: „F9 erreicht VS Code nicht / Shift+F9 schon" wird mit dem Daemon-
End-to-End-Gate (nach Paket C) geprüft.

---

# Fixpaket nach Sol-Review

Stand: 2026-08-27, Implementierer: Claude Opus 5.0. Working-Tree, **nicht**
committet. Geändert nur `src/hotkey.rs` (`mod windows` + ein Testname) und
`src/daemon/workers.rs` (Hotkey-Worker, `HotkeyCmd::Grab`); Review:
[impl-phase5-A-sol.md](impl-phase5-A-sol.md).

## Blocker

### ✅ Blocker 1 — Ready-Timeout hinterlässt keinen Zombie-Hook mehr

`ThreadPorts` trägt jetzt ein `cancel: Arc<AtomicBool>`, das **vor** dem Spawn
angelegt wird. Der Hook-Thread prüft es an drei Stellen: nach dem
`PeekMessageW` (Queue erzwungen, noch nichts installiert), nach
`SetWindowsHookExW` (zusammen mit dem `ready.send`) und ein drittes Mal
unmittelbar vor dem Eintritt in `GetMessageW`. Ist es gesetzt, wird ein schon
installierter Hook über `remove_hook_finally` entfernt und der Thread endet.

`try_new` setzt das Flag im Timeout- **und** im `Ready::Failed`-Zweig und
droppt `ready_rx` vor dem Join. Zusätzlich eine Nachfrist von 250 ms
(`CANCEL_GRACE`): Kommt das `Ready::Up` knapp zu spät, trägt es die
Thread-ID — dann geht ein gezieltes `MSG_STOP` an den Thread, der schon in
`GetMessageW` steht. Damit ist auch das letzte Fenster zwischen der dritten
Cancel-Prüfung und dem blockierenden `GetMessageW` geschlossen.

Der vom Review gewünschte deterministische Test („Timeout, danach verspätetes
erfolgreiches Install") ist ohne Driver-Abstraktion um `SetWindowsHookExW`
nicht schreibbar — siehe offene Punkte.

### ✅ Blocker 2 — `UnhookWindowsHookEx`-Rückgabe wird ausgewertet

`remove_hook` liefert `Result<(), String>`; das Handle wird **nur** nach
Rückgabewert ungleich 0 genullt, der Fehlertext kommt aus dem unmittelbar
danach gelesenen `GetLastError`. `MSG_REMOVE` setzt `HookState::reset` und den
Ack `Ok(())` nur nach erfolgreichem Unhook, sonst geht der Win32-Fehler über
den Ack an `unregister()`, wo `?` ihn weiterträgt — `registered` bleibt dann
**wahr**, weil der Hook weiterschluckt.

Beim Threadende (`MSG_STOP`, `WM_QUIT`, `GetMessageW == -1`) läuft
`remove_hook_finally`: Fehler auf stderr (`eprintln!`, wie an anderen Stellen
des Repos), genau ein zweiter Versuch, dann beenden mit der Meldung, dass der
Hook erst mit dem Prozess endet. Der SAFETY-Kommentar sagt jetzt, was
tatsächlich gilt (genullt nur nach Erfolg, ein Fehlschlag darf wiederholt
werden).

### ✅ Blocker 3 — Ack-Race: Backend nach Command-Timeout irreversibel defekt

Neues Feld `broken: bool`. Jeder unbestätigte Command (`recv_timeout`-Fehler,
auch `Disconnected`) und jedes fehlgeschlagene `PostThreadMessageW` rufen
`mark_broken()`: `broken = true`, `registered = false`, Cancel-Flag setzen,
`MSG_STOP` posten. Danach liefern `register`, `unregister`, `poll` und
`command` sofort `HotkeyError::Failed("Hotkey nicht verfügbar (<Chord>):
Hook-Thread antwortet nicht")`, und `is_registered()` ist `false`. Es wird nie
wieder ein Command auf diesen Thread geschickt — ein verspäteter Ack kann
deshalb keinen neuen Command mehr bestätigen. Über §10 landet das als
`Msg::HotkeyUnavailable` im Daemon (Hotkey aus, Tray-Click bleibt).

Die einfache Variante statt Request-IDs war die Orchestrator-Entscheidung.

### ✅ Blocker 4 — Resume-Fehler beendet den Hotkey-Worker (`workers.rs`)

`HotkeyCmd::Grab` mit fehlgeschlagenem `backend.register()` verhält sich jetzt
wie der Startup-Pfad: `log.error(...)`, `Msg::HotkeyUnavailable(err)` und
`return` aus `hotkey_loop`. Vorher nur `log.warn` — die State-Machine wäre
nicht mehr pausiert gewesen, während kein Hotkey greift.

⏸️ **Test weggelassen** (bewusst, siehe Auftrag): `hotkey_loop` baut sein
Backend über `new_backend(spec)` selbst, es gibt keinen Injektionspunkt und
keinen bestehenden Test im Daemon-Dispatch, der das prüfbar macht. Ein Test
bräuchte eine Extraktion der Kommandobehandlung in eine Funktion über
`&mut dyn HotkeyBackend` plus einen Fake im Testmodul von `workers.rs` — das
geht über den Auftragsumfang (nur der Worker selbst) hinaus, zumal parallel
ein zweiter Implementierer im selben Working-Tree arbeitet. Vorschlag für
einen Aufräum-Commit, siehe offene Punkte.

### ⏸️ Blocker 5 — `LowLevelHooksTimeout`-Watchdog zurückgestellt

Orchestrator-Entscheidung: Für den Dev-Milestone wird das Restrisiko
akzeptiert, ein Probe-Event-Watchdog kommt als Folgepunkt. Umgesetzt ist nur
die Korrektur des irreführenden Kommentars bei `poll()`: Der Kanalabbruch
erfasst **Threadtod** (`GetMessageW == -1`, Panic), nicht einen von Windows
still aus der Kette genommenen Hook — in dem Fall lebt der Thread weiter, der
Kanal bleibt offen und der Hotkey ist trotzdem tot. Als offener Punkt unten
geführt.

## Wichtige Hinweise und Kleinigkeiten

### ✅ `WM_KEYUP`/`WM_SYSKEYUP` explizit, alles andere `Pass`

`hook_proc` matcht `wparam` jetzt auf genau die vier dokumentierten
Nachrichten; jeder andere Wert geht unverändert an `CallNextHookEx`, statt als
Up gedeutet zu werden und ein `accepted_down` zu beenden.

### ✅ Panic-Grenze im Callback

Die **gesamte** Rust-Auswertung liegt in
`panic::catch_unwind(AssertUnwindSafe(..))` mit `Decision::Pass` als Rückfall;
`HOOK.try_with` statt `HOOK.with` (TLS kann beim Threadende schon zerstört
sein). Der Kommentar behauptet nicht mehr, `try_borrow_mut` allein löse die
Unwind-Sicherheit, und begründet `AssertUnwindSafe`: einziger geteilter
Zustand ist der thread-lokale `HookState`, ein halb geänderter `HookState`
kostet höchstens ein verwaistes Up.

### ✅ Stop mit Ack, Receiver bis danach halten

Der Hook-Thread sendet den Ack zum Stop **erst nach** dem bestätigten Unhook
(`remove_hook_finally` → `clear_context` → `acks.send(result)`). `Drop` setzt
Cancel, postet `MSG_STOP`, wartet 2 s auf diesen Ack (Fehler und Timeout auf
stderr) und joint danach. `events`/`acks` sind Felder von `self` und werden
erst nach dem `Drop`-Rumpf zerstört — der Callback kann also mitten im
Shutdown kein Down mehr schlucken, dessen Release ins Leere ginge. Ist das
Backend `broken`, entfällt das Warten (der Thread antwortet definitionsgemäß
nicht mehr).

`join_timeout` nutzt jetzt `thread::Builder::spawn` und meldet Spawnfehler wie
Timeout auf stderr, statt im Aufräumpfad panicken zu können.

### ✅ `GetMessageW == -1` nennt den Fehlercode

Neuer Status-Kanal (`Sender<String>` im Thread, `Receiver<String>` im
Backend). Bei `rc < 0` wird `GetLastError` **sofort** gesichert und als
`GetMessageW: Win32-Fehler <code>` gesendet; `poll()` liest ihn im
`Disconnected`-Zweig und nennt ihn statt „Hook-Thread beendet". `rc == 0`
(`WM_QUIT`) ist davon getrennt und gilt als geordnetes Ende.

### ✅ SAFETY-Kommentar bei `install_hook` präzisiert

`dwThreadId = 0` ist jetzt als **gewählt** beschrieben (globaler
Desktop-Hook, ein Thread-Filter sähe fremde Tasten nicht) statt als von
`WH_KEYBOARD_LL` „verlangt".

### ✅ AltGr dokumentiert, nicht umgebaut

Doc-Kommentar an `ModifierState::current`: Windows erzeugt für AltGr
`VK_RMENU` **plus** synthetisches `VK_LCONTROL`, deshalb erscheint AltGr hier
als `Ctrl+Alt` — ein konfigurierter `Ctrl+Alt+<Key>`-Hotkey kann beim Tippen
von `@`, `€`, `\` auslösen. Bewusst nicht umgebaut
(Orchestrator-Entscheidung); der Default `F9` ist nicht betroffen, weil
zusätzliche Modifier den exakten Vergleich ohnehin verfehlen.

### ✅ Ungrab bei gehaltenem Key: akzeptierte Ausnahme, Test umbenannt

Verhalten unverändert (das verwaiste Up erreicht die Anwendung). Der Test
heißt jetzt `reset_passes_orphaned_up` statt
`reset_releases_a_held_key_to_the_application` und trägt die Begründung im
Doc-Kommentar: ein Up ohne Down ist für Windows folgenlos, die Alternative
(„disabled, pending release") hielte den globalen Hook über die Pause hinaus
am Leben und widerspräche der Pause-Semantik. Wörtlich ist es eine Ausnahme zu
§4.4 („erreicht die fokussierte Anwendung **nie**") — hier als akzeptiert
geführt.

### ✅ `mpsc::Sender::send` im Callback bleibt, als Risiko dokumentiert

Kommentar am Feld `HookContext::events`: nicht blockierend, aber weder lock-
noch allokationsfrei garantiert; läuft nur bei echtem Press/Release des
Hotkeys (höchstens zwei pro Diktat), nicht bei jedem Tastendruck des Systems.
`sync_channel` + `try_send` bleibt der Folgeschritt, falls
`LowLevelHooksTimeout` je zuschlägt.

## Gates

### ✅ `cargo build --release`

Grün, unverändert **2** Warnungen — beide vorbestehend, keine neue:

```
warning: unreachable expression               --> src\tray.rs:268:5
warning: function `x11_keysym` is never used  --> src\hotkey.rs:73:8
warning: `diktier` (bin "diktier") generated 2 warnings
    Finished `release` profile [optimized] target(s) in 12.65s
```

### ✅ `cargo test hotkey`

```
test hotkey::tests::win::reset_passes_orphaned_up ... ok
test result: ok. 30 passed; 0 failed; 0 ignored; 0 measured; 247 filtered out
```

### ✅ `cargo test` (gesamt)

```
test result: FAILED. 268 passed; 8 failed; 1 ignored; 0 measured
```

Exakt die **8 bekannten Vorbestands-Fehlschläge** (autostart 1, download 1,
single_instance 6), Zahlen identisch zum Lauf vor dem Fixpaket.

### ✅ `rustfmt --check src/hotkey.rs src/daemon/workers.rs`

Sauber (`--edition 2024`), keine Ausgabe. Kein `cargo fmt` über den Baum.

### ✅ Smoke `--hotkey-test --foreground` (30 s)

```
SPIKE --hotkey-test (kein Produktionspfad)
SPIKE: F9 30s lang halten/loslassen; Exit mit Ctrl+C
SPIKE hotkey-backend=win32-ll-hook
SPIKE hotkey-test: 30s vorbei
EXIT=0
```

Registrierung erfolgreich, sauberes Ende, **keine** neue stderr-Zeile aus
`remove_hook_finally`/`join_timeout` — der Stop-Ack kam also, und der Unhook
gelang beim ersten Versuch.

### ✅ Smoke Daemon `--foreground` + Ctrl+C

`kill -INT` aus Git Bash erreicht einen nativen Windows-Prozess **nicht** (die
MSYS-Emulation beendet nur den Job-Wrapper; der Daemon lief nach `EXIT=130`
unverändert weiter). Deshalb echtes `CTRL_C_EVENT` über `AttachConsole` +
`GenerateConsoleCtrlEvent` an eine eigene Konsole:

```
gestartet PID=28828
nach 10 s laeuft: True
CTRL_C gesendet=True  beendet=True  ExitCode=-1073741510
kein diktier-Prozess
```

`-1073741510` = `0xC000013A` = `STATUS_CONTROL_C_EXIT`. Log bis `idle`
sauber, **kein Panic**, kein hängender Prozess. Erwartungsgemäß läuft kein
`Drop`: `signals.rs` installiert unter `cfg(not(unix))` noch keinen
`SetConsoleCtrlHandler` (Kommentar dort: „bekommt seinen
`SetConsoleCtrlHandler` mit dem Windows-Backend"), Windows reißt den Prozess
also hart ab. Der globale Hook wird dabei vom System mit dem Prozess entfernt.

## Offene Punkte des Fixpakets

### 🔍 `LowLevelHooksTimeout`-Watchdog (Folgepunkt)

Nimmt Windows den Hook wegen einer überschrittenen `LowLevelHooksTimeout`
still aus der Kette, lebt der Hook-Thread weiter, `poll()` sieht weder
Disconnect noch Fehler, und `is_registered()` bleibt `true`, obwohl der Hotkey
tot ist. Zurückgestellt (Orchestrator: Restrisiko für den Dev-Milestone
akzeptiert). Vorgeschlagene Lösung aus dem Review: periodischer eindeutig
markierter injizierter Probe-Event, den der Hook nur als Liveness-Signal zählt
und immer an `CallNextHookEx` weitergibt.

### 🔍 Lebenszyklus-Tests brauchen eine Driver-Abstraktion

Weder „Ready-Timeout mit verspätetem Install" noch „Unhook schlägt fehl" noch
„Command-Timeout → broken" sind heute testbar: `install_hook`/`remove_hook`
rufen Win32 direkt. Ein kleines Trait (`install`/`remove`/`post`) mit einem
Fake im Test würde alle drei deterministisch machen. Nicht Teil dieses
Fixpakets.

### 🔍 Test „Ungrab ok, Grab scheitert → HotkeyUnavailable"

Braucht die oben beschriebene Extraktion der Kommandobehandlung aus
`hotkey_loop`. Verhalten ist umgesetzt, der Test fehlt.

### 🔍 Gate „echtes F9" und Linux-Gate weiterhin offen

Unverändert gegenüber dem ersten Notizenstand: physisches F9 (Press/Release im
Log, F9 erreicht Notepad nicht, Shift+F9 schon) und
`cargo check --target x86_64-unknown-linux-gnu` auf einer Maschine mit
Linux-C-Toolchain.

### 🔍 `SetConsoleCtrlHandler` fehlt noch

Ohne ihn endet der Daemon bei Ctrl+C hart, `Drop for WinHookBackend` läuft
nie, und der §5.2-Quit-Pfad (Clipboard-Handshake) wird auf Windows
übersprungen. Vorbestehend und außerhalb von Paket A — gehört zum
Windows-Backend-Paket, siehe `src/daemon/signals.rs`.
