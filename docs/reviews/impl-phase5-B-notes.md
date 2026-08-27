# Phase 5, Paket B (WP3) — Implementierungsnotizen

Stand: 2026-08-27, Implementierer: Claude Opus 5.0 (Windows 11, MSVC x64,
Rust 1.95.0). Working-Tree, **nicht** committet.

Auftrag: [windows-plan.md](../windows-plan.md) WP3 (Inject: Clipboard +
`SendInput`) mit den Blockern und Hinweisen aus
[plan-phase5-sol.md](plan-phase5-sol.md).

## Geänderte Dateien

| Datei | Änderung |
|---|---|
| `src/inject/windows.rs` | **neu**, 1054 Zeilen: `Win32OutputSink` (`ClipboardHost` + `OutputSink`), Message-only-Fenster, `WndProc`, `SendInput`, Prozessname; 8 Windows-Tests |
| `src/inject/mod.rs` | `#[cfg(windows)] mod windows;`, `PlatformSink = windows::Win32OutputSink`, `new_sink` (cfg(windows)); 4 neue plattformneutrale Tests |
| `src/inject/protocol.rs` | `auto_shortcut` um den Windows-Zweig erweitert (`windows_process_shortcut`); neues `mod tests` mit 4 Tests |
| `src/inject/fake.rs` | zwei neue `ScriptEvent`-Varianten (`OwnRender`, `ForeignSequenceBump`) für die Windows-Sequenznummer-Semantik |
| `Cargo.toml` | drei neue `windows-sys`-Features, Kommentartabelle nach WP2/WP3 getrennt fortgeschrieben |
| `Cargo.lock` | unverändert gegenüber Paket A (die eine Zeile `windows-sys 0.61.2` stand schon) |

Nicht angefasst: `src/hotkey.rs`, `src/daemon/workers.rs`, `src/main.rs`,
`src/inject/linux.rs`. In `src/inject/mod.rs` sind ausschließlich die drei
`cfg(windows)`-Stellen und der Testblock berührt; `StubOutputSink` bleibt
unverändert stehen (die Datei hat `#![allow(dead_code)]`, es entsteht also
keine Warnung, und zwei Tests prüfen damit weiter den Trait-Vertrag).

> **Hinweis zum Working-Tree:** Während dieses Pakets sind parallel
> Paket-A-Nachbesserungen in `src/hotkey.rs`, `src/daemon/workers.rs` und
> `docs/windows-plan.md` erschienen (Orchestrator). Sie stammen nicht aus
> diesem Paket. `cargo fmt` läuft über den ganzen Crate — falls dabei etwas
> daraus umformatiert wurde, entspricht das der Projektnorm
> (`cargo fmt --check` ist grün).

## ✅ WP3 — `Win32OutputSink` (`src/inject/windows.rs`)

Aufbau nach Plan:

- **Message-only-Fenster** (`HWND_MESSAGE`) auf dem Thread, der den Sink
  erzeugt (Inject-Worker). Klasse `DiktierClipboardOwner`, `RegisterClassW`
  im Konstruktor, `UnregisterClassW` im `Drop`;
  `ERROR_CLASS_ALREADY_EXISTS` ist kein Fehler, dann meldet der Sink die
  Klasse aber auch nicht ab.
- **Thread-affiner State** über `GWLP_USERDATA`: ein
  `Box<RefCell<ClipboardState>>` gehört dem Sink, der Zeiger erreicht den
  `WndProc` als `lpCreateParams` in `WM_NCCREATE`. `WM_NCDESTROY` nullt ihn
  wieder. `Drop::drop` ruft `DestroyWindow`, bevor die Box freigegeben wird
  — die Lebensdauer stimmt garantiert. Im `WndProc` `try_borrow_mut` (ein
  Panic wäre ein Unwind über die Win32-Grenze).
- **`pump(timeout)`** = `MsgWaitForMultipleObjects(0, NULL, FALSE, ms,
  QS_ALLINPUT)` + `PeekMessageW`/`DispatchMessageW`-Schleife, danach werden
  `reads`/`lost` aus dem State in `PumpEvents` gezogen. Läuft dauerhaft:
  `serve_for(10 ms)` im Idle des Inject-Workers geht direkt hier hinein.
- **Snapshot**: erst pumpen (angelaufene fremde Änderungen), dann — wenn wir
  selbst Owner sind — der eigene Serve-Text ohne `OpenClipboard`
  (Analog zum X11-Sink). Sonst `OpenClipboard` mit 10 Versuchen à 10 ms
  Pump; `CountClipboardFormats()==0` → `Text("")`, kein `CF_UNICODETEXT` →
  `NonText`, sonst `GlobalLock`/`GlobalSize` → UTF-16 bis NUL → `Text`.
- **`become_owner`** = `EmptyClipboard` + `SetClipboardData(CF_UNICODETEXT,
  NULL)`. `WM_RENDERFORMAT` liefert ohne `OpenClipboard`, zählt `reads += 1`
  und schreibt `expected_seq` fort; `WM_RENDERALLFORMATS` öffnet, prüft den
  Owner erneut, setzt eager, schließt; `WM_DESTROYCLIPBOARD` wird nur bei
  `guard == 0` **und** (Owner ≠ hwnd oder Sequenz ≠ `expected_seq`) als
  `lost_ownership` gewertet.
- **`still_owner`** = `GetClipboardOwner()==hwnd && seq==expected_seq`
  (Leitentscheidung 4), Vergleich nur per Gleichheit.
- **`query_modifiers`** per `GetAsyncKeyState` (Shift/Alt/LWin|RWin/Ctrl,
  High-Bit); `key_down`/`key_up` je ein `SendInput`,
  `VK_INSERT` mit `KEYEVENTF_EXTENDEDKEY`, Rückgabewert exakt gegen 1
  geprüft.
- **`current_window`** = `GetForegroundWindow()`, `NULL` → `None`. Kein
  `AttachThreadInput`, `HWND` wird nur als opake Zahl geführt und
  ausschließlich an Win32 zurückgereicht.
- **`wm_class`** = `(basename, basename)` über
  `GetWindowThreadProcessId` → `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)`
  mit RAII-`CloseHandle` (`ProcessHandle`) →
  `QueryFullProcessImageNameW` (Puffer verdoppelt sich bis 32768).
  Jeder Fehler ist `None`, nie ein Inject-Fehler.
- Jeder `unsafe`-Block hat einen SAFETY-Kommentar (Stil wie
  `src/hotkey.rs` `mod windows`).

### Abweichungen vom Plan (mit Begründung)

1. **`set_serve_text` materialisiert, statt nur den Serve-Text zu tauschen.**
   Der Plan legt den Restore unter `release_ownership` ab; das Protokoll ruft
   `release_ownership` aber nur für einen **leeren** Snapshot auf und für
   jeden anderen `set_serve_text` (`protocol.rs`, `inject_paste`). Auf X11
   genügt der Tausch, weil dort jede Anfrage bei uns landet. Auf Windows
   liegt nach `WM_RENDERFORMAT` echter Text im Clipboard — ein späterer Leser
   fragt uns nicht mehr. Ohne eager setzen wäre der Restore ein reiner
   Buchhaltungsvorgang und §7.1 Punkt 5 faktisch wirkungslos. `set_serve_text`
   ist deshalb: nicht Owner → nur merken; Owner → `EmptyClipboard` +
   `SetClipboardData(<echte Daten>)` + `expected_seq` fortschreiben.
2. **`release_ownership` leert das Clipboard wirklich.** Windows kennt kein
   Ablegen der Ownership. Das Gegenstück zum X11-`SetSelectionOwner(None)`
   für einen leeren Snapshot ist `EmptyClipboard` ohne `SetClipboardData`;
   der nächste Snapshot liest das über `CountClipboardFormats()==0` wieder
   als `Text("")`.
3. **Fehler in `set_serve_text` gibt es nicht zu melden — also gilt der
   Zustand als unbekannt.** Der Trait gibt dort `()` zurück. Scheitert die
   Materialisierung, setzt `take_clipboard` `owned = false`; `still_owner()`
   meldet danach false und `inject_paste` bucht `ForeignOwner`, also
   `restored == false`. Lieber ein zu Unrecht ausgelassener Restore als ein
   gemeldeter, der nicht stattgefunden hat (§7.1 Punkt 7, gleiche Richtung).
4. **`Drop` materialisiert ein offenes Delayed-Rendering-Versprechen.** Der
   Plan nennt nur `save_to_clipboard_manager` vor dem Quit; der Spike
   `--inject-test` ruft das aber nie auf, und ein Delayed-Versprechen stirbt
   mit dem Prozess. Ohne diesen Schritt wäre der Text nach jedem Prozessende
   weg — das widerspräche §7.1 Punkt 8 und §10 („Transkript bleibt im
   Clipboard"). Reihenfolge im `Drop`: eager rendern → `DestroyWindow` →
   `UnregisterClassW`.
5. **Kein Protokoll-Hook für den `SendInput`-Teilfehler.** Geprüft wie
   beauftragt: `protocol::chord_ctrl_v`/`chord_shift_insert` führen bereits
   eine `pressed`-Liste und lösen bei jedem `Err` in umgekehrter Reihenfolge
   (`for key in pressed.into_iter().rev()`). Der Host muss also nur
   fehlschlagen. **`protocol.rs` wurde dafür nicht geändert** — der bestehende
   Test `chord_failure_releases_keys_we_pressed` deckt Ctrl+V ab, neu dazu
   `ctrl_shift_v_failure_releases_every_key_it_pressed` für den längeren
   Chord an drei Fehlerpositionen.
6. **`CF_UNICODETEXT` als lokale Konstante statt `Win32_System_Ole`.** In
   windows-sys 0.61.2 liegt die Konstante ausgerechnet unter dem
   COM-Feature `Win32_System_Ole`, von dem sonst nichts gebraucht wird. Der
   Wert 13 ist stabile `winuser.h`-ABI; ein Test hält ihn fest.
7. **`Win32_Graphics_Gdi` musste mit ins Feature-Set.** windows-sys stellt
   `WNDCLASSW`/`RegisterClassW` erst mit diesem Feature bereit
   (`hbrBackground` ist ein `HBRUSH`). WP4 braucht es ohnehin
   (`CreateDIBSection`, `HICON`); es kommt hier nur eine Stufe früher.
8. **`auto_shortcut` erkennt die Windows-Form an `(exe, exe)` + `.exe`.**
   Die Funktion ist plattformneutral und hat Linux-Tests. Der neue Zweig
   `windows_process_shortcut` greift nur, wenn beide Hälften
   ASCII-case-insensitiv gleich sind **und** auf `.exe` enden — genau die
   Platzhalterform, die der Windows-Sink liefert. X11-Klassen erfüllen das
   nicht; die Linux-Tabelle und ihre Tests sind unverändert. Ein Wine-Fenster
   mit der Klasse `foo.exe` bekäme `CtrlV`, also denselben Default wie
   vorher.
9. **Obergrenze von 256 Nachrichten je Pump-Durchlauf.** Reine
   Vorsichtsmaßnahme: die `PeekMessageW`-Schleife kann so nicht endlos
   drehen. Übrige Nachrichten bleiben in der Queue.
10. **Kein `TranslateMessage`.** An ein `HWND_MESSAGE`-Fenster geht keine
    Tastatureingabe; es gäbe nichts zu übersetzen.

## Tests

**Plattformneutral, Fake-Host** (`src/inject/mod.rs`), 4 neu:

- `own_render_bumps_the_sequence_and_keeps_ownership` — der eigene Render
  erhöht die Sequenz, die eigene Generation wandert mit, Restore findet
  statt (Leitentscheidung 4 / Sol-Blocker 2).
- `foreign_copy_before_the_render_is_foreign_owner` — fremder Copy vor dem
  Render: kein Read, `ForeignOwner`, fremder Inhalt bleibt.
- `same_owner_with_foreign_sequence_never_restores` — Owner gleich,
  Sequenz fremd: kein Restore, obwohl kein `lost_ownership`-Ereignis kam.
- `ctrl_shift_v_failure_releases_every_key_it_pressed` — `SendInput`-Teil-
  fehler an Position 1/2/3, jede gedrückte Taste bekommt ihr Up.

Dafür zwei neue `ScriptEvent`-Varianten in `fake.rs`
(`OwnRender`, `ForeignSequenceBump`) — sparsam gehalten, das
Generationsmodell (`generation` / `our_generation`) war bereits da und passt
1:1 auf `GetClipboardSequenceNumber` / `expected_seq`.

**`protocol.rs`**, 4 neu: `windows_terminal_gets_ctrl_shift_v`,
`other_windows_processes_get_ctrl_v` (u. a. `conhost.exe`,
`WindowsTerminalPreview.exe`), `windows_rule_needs_both_halves_equal`,
`names_without_exe_suffix_use_the_x11_table`.

**Windows-only** (`src/inject/windows.rs`, hinter `cfg(windows)`), 8 neu:
`utf16_round_trip_keeps_umlauts_and_stops_at_nul`,
`utf16_handles_emoji_and_lone_surrogates`,
`utf16_without_nul_is_read_completely`,
`basename_takes_the_last_path_component`,
`paste_keys_map_to_the_exact_virtual_keys`, `only_insert_is_extended`,
`clipboard_format_constant_matches_winuser`,
`wide_strings_are_nul_terminated`.

## Gates

### ✅ `cargo build` / `cargo build --release`

Beide grün, **keine neue Warnung** — dieselben 2 wie vor dem Paket:

```
warning: unreachable expression
   --> src\tray.rs:268:5           (vorbestehend, WP4)
warning: function `x11_keysym` is never used
  --> src\hotkey.rs:73:8           (vorbestehend auf Windows)
warning: `diktier` (bin "diktier") generated 2 warnings
    Finished `release` profile [optimized] target(s) in 12.76s
```

### ✅ `cargo test`

```
test result: FAILED. 284 passed; 8 failed; 1 ignored; 0 measured; 0 filtered out
```

284 statt 268 (Paket A) = **16 neue Tests**, alle grün. Die 8 Fehlschläge
sind Zeile für Zeile die bekannten Vorbestands-Fehlschläge aus dem
WP5-Gebiet, unverändert:

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

`cargo test inject` (50 Tests) und `cargo test protocol` (4 Tests) sind
vollständig grün.

### ✅ `cargo fmt --check`

Sauber (nach einem `cargo fmt`-Lauf über den Crate).

### ❌ Manueller Smoke — **auf diesem Rechner nicht durchführbar: Bildschirm gesperrt**

`--inject-test` zweimal hintereinander, jeweils identisch:

```
SPIKE --inject-test (kein Produktionspfad)
SPIKE start_window_id=0x10cc0
SPIKE: 3s — Ziel im Vordergrund halten für Paste, wechsele für copy_only …
SPIKE target_window_id=0x10cc0
SPIKE inject-fehler: Ausgabe fehlgeschlagen: Clipboard nicht zu öffnen (10 Versuche): Win32-Fehler 5
EXIT=1
```

Win32-Fehler 5 ist `ERROR_ACCESS_DENIED` beim `OpenClipboard`. Das ist
**keine** Eigenheit des neuen Codes — auf dieser Maschine kann derzeit
**kein** Prozess das Clipboard öffnen:

```
Set-Clipboard : Der angeforderte Clipboard-Vorgang war nicht erfolgreich.
"VORHER" | clip.exe   →  FEHLER: Zugriff verweigert
```

Diagnose (P/Invoke aus PowerShell, damit es nicht am Wrapper liegt):

```
WindowStation = WinSta0
Desktop       = Default
OpenClipboardWindow = 0        (niemand hält das Clipboard offen)
ClipboardOwner      = 0
OpenClipboard ok=False err=5   (auch nach drei Versuchen)
Integritätsstufe    = S-1-16-8192 (Medium, also keine UIPI-Bremse)
Job-UI-Restrictions = 0x0        (kein READ/WRITECLIPBOARD-Verbot)
```

… und die Ursache:

```
Get-Process LogonUI  →  LogonUI  13684
query session        →  >console  rakul  2  Aktiv
```

**`LogonUI.exe` läuft, der Arbeitsplatz ist gesperrt.** Der Eingabedesktop
ist `Winlogon`, Prozesse auf `Default` bekommen für Clipboard-Operationen
`ERROR_ACCESS_DENIED`. `Set-Clipboard "VORHER"` / `Get-Clipboard` sind
damit ebenfalls nicht ausführbar, der Vorher-/Nachher-Vergleich entfällt.

Was der Lauf trotzdem belegt: `RegisterClassW`, `CreateWindowExW`
(`HWND_MESSAGE`), die Pump und der Aufräumpfad funktionieren — beide Läufe
kamen bis zum Snapshot, endeten sauber mit Exit 1 und ohne Hänger, und der
zweite Lauf verhielt sich identisch zum ersten (Fensterklasse sauber
freigegeben). Der Fehlerpfad selbst ist §10-konform: klarer Text mit
Win32-Code, kein stilles Verwerfen.

## Offene Punkte

### 🔍 Smoke-Gate steht aus (Bildschirm entsperren)

Bei entsperrtem Desktop nachzuholen, in dieser Reihenfolge:

```powershell
Set-Clipboard "VORHER"
start notepad
```
```bash
./target/release/diktier.exe --inject-test "Grüße, Jörg – Zeile eins" --foreground
```
```powershell
Get-Clipboard
```

Erwartung: Bei Notepad im Vordergrund `Pasted` mit `reads >= 1` und
`restore restored`, Text in Notepad, `Get-Clipboard` liefert danach wieder
`VORHER`. Bei Fokuswechsel während der 3 s `CopyOnly { FocusChanged }` und
`Get-Clipboard` liefert das Transkript. Zusätzlich aus dem Plan: Windows
Terminal (Ctrl+Shift+V-Regel greift über den Prozessnamen) sowie Win+V-
History an und aus.

### 🔍 Gesperrter Bildschirm ist laut §7.3 Fokusverlust — `GetForegroundWindow` liefert das nicht

Beobachtung aus dem Fehlversuch oben: bei gesperrtem Arbeitsplatz gab
`GetForegroundWindow()` weiterhin `0x10cc0` zurück, nicht `NULL`. Start-,
Ziel- und aktuelles Fenster stimmen dann überein, das Protokoll würde den
Paste-Chord senden — auf einen Desktop, den niemand sieht. SPEC §7.3 nennt
den gesperrten Bildschirm ausdrücklich als Fokusverlust (der X11-Teil begründet
das mit dem Unlock-Dialog nach dem 60-s-Cap).

Sauber wäre eine Prüfung „läuft mein Desktop gerade als Eingabedesktop?"
(`OpenInputDesktop` + Vergleich mit `GetThreadDesktop`, Feature
`Win32_System_StationsAndDesktops`). Das steht **nicht** im WP3-Auftrag und
ist hier bewusst nicht dazuerfunden worden — Vorschlag für WP5 oder ein
eigenes kleines Paket, mit Entscheidung des Orchestrators.

In der Praxis fängt der aktuelle Stand den Fall trotzdem ab: `OpenClipboard`
scheitert, es wird nichts gesendet und der Fehler steht im Log. Nur die
Begründung im Tooltip wäre die falsche („Ausgabe fehlgeschlagen" statt
„Fokus geändert").

### 🤔 `serve_until_read` ist auf Windows nach dem Restore ein Leerlauf

Nach dem Restore liegt der alte Text **eager** im Clipboard; es kommt kein
`WM_RENDERFORMAT` mehr, `serve_restored_until_read` läuft die volle
`RESTORED_SERVE_GRACE` (3 s) leer und liefert 0. Das kostet nur im Spike
Zeit — der Daemon ruft die Methode nicht auf (`workers.rs` nutzt
`serve_for`). Die Pump läuft dabei weiter, was ohnehin richtig ist. Falls
der Spike-Ablauf stört: auf Windows direkt `Ok(0)` zurückgeben.

### ⏸️ Linux-Gate weiterhin offen

`cargo check --target x86_64-unknown-linux-gnu` ist auf diesem Rechner
mangels Linux-C-Cross-Toolchain nicht ausführbar (Begründung und Nachweis
in [impl-phase5-A-notes.md](impl-phase5-A-notes.md)). Für dieses Paket gilt
dasselbe Ersatzargument: der gesamte neue Code liegt in einer Datei hinter
`#[cfg(windows)]`; außerhalb davon sind nur drei `cfg(windows)`-Zeilen in
`mod.rs`, der additive `windows_process_shortcut`-Zweig samt Tests in
`protocol.rs` (die Linux-Tabelle und ihre Tests sind unverändert grün) und
zwei zusätzliche Enum-Varianten im Test-Fake berührt.
