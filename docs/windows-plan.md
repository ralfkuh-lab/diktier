# Phase 5 — Windows-Portierung (Plan, v3)

Stand: 2026-08-31, v3 (v2: 2026-08-27 nach Sol-Review,
[reviews/plan-phase5-sol.md](reviews/plan-phase5-sol.md)). Ziel dieser
Phase war ein **Windows-Dev-Milestone**: Diktier läuft auf dem
Entwicklungsrechner (Win 11, Jabra Evolve2 40) end-to-end — **erreicht**
(WP1–6 ✅). Die ursprünglich als Release-Voraussetzung geführten
Clean-VM-Gates (WP7) sind am 2026-08-31 **verworfen** (privates
Werkzeug, siehe WP7); erstes veröffentlichtes Release ist v0.2.0 mit dem
Aufnahme-Overlay ([overlay-plan.md](overlay-plan.md)). Verbindlich ist
[SPEC.md](SPEC.md) (aktuell v1.5).

## Ausgangslage (auf diesem Rechner verifiziert)

- `cargo build --release` (MSVC x64, Rust 1.95) läuft durch, 6 Dead-Code-
  Warnungen, keine Fehler.
- `scripts/fetch-ort.ps1` liefert `lib/onnxruntime.dll` (ORT 1.28.0, SHA
  geprüft); `ort::init_from` findet sie relativ zur Binary.
- Modell-Download (639 MiB, 4 Dateien, SHA-256) funktioniert nach
  `%LOCALAPPDATA%\diktier\models\…`, Modell-Load ~1,9 s, Inferenz ~0,6 s für
  12-s-WAVs; Transkripte der drei Referenz-WAVs plausibel.
- Audio-Capture per cpal/WASAPI läuft (48 kHz F32 mono → 16 kHz, keine
  Overflows).
- Daemon startet, erreicht `idle`, meldet aber `Tray-Backend: stub` und
  `Hotkey-Backend: stub`.

**Stubs, die zu ersetzen sind** (alle hinter bestehenden Traits):

| Baustein | Stelle | Vertrag |
|---|---|---|
| Hotkey | `src/hotkey.rs:908` `new_backend` → `StubHotkeyBackend` | `HotkeyBackend { register, unregister, poll, is_registered, backend_name }`, Events `HotkeyEvent::{Press,Release}`; Worker in `daemon/workers.rs` schickt `HotkeyCmd::{Grab,Ungrab}` (Pause) |
| Inject | `src/inject/mod.rs:176` `PlatformSink = StubOutputSink` | `OutputSink { paste, copy_only, current_window_id, serve_for, serve_until_read, save_to_clipboard_manager }`; Protokoll `inject_paste<H: ClipboardHost>` in `protocol.rs` ist plattformneutral; `PumpEvents { reads, lost_ownership }` |
| Tray | `src/tray.rs:145` `new_backend` → `StubTray`; `open_config_dir` Fehler | `TrayBackend { update, poll, backend_name }`, Events `TrayEvent::{LeftClick,TogglePause,OpenConfigDir,Quit}` |
| Single-Instance | `src/single_instance.rs:284` `flock_exclusive_nonblocking` → immer `true`; Download-Lock in `download.rs` ebenso | SPEC §5.3 (per-session Named Mutex) |
| Signale | `src/daemon/signals.rs` `cfg(not(unix))` leer | `install()` / `take_quit_request()` |
| Autostart | `src/autostart.rs:149` schreibt `diktier.cmd` in den Startup-Ordner | SPEC §9, vorhanden, ungetestet |
| Subsystem | `src/main.rs` ohne `windows_subsystem` | SPEC §9: Windows-Subsystem, `--foreground` hängt Konsole an |

## Leitentscheidungen

1. **Kein neues Tray-Crate, kein GUI-Framework.** Direkt Win32 über
   `windows-sys` `=0.61.2` (steht bereits transitiv im Lock, kein zweiter
   Baum). `betrayer` wird Linux-only. API→Feature-Tabelle in WP1.
2. **Zwei getrennte Owner-Threads, keine Änderung der Worker-Architektur.**
   Das Tray-`HWND` lebt ausschließlich auf dem bestehenden Tray-Worker-Thread,
   das Clipboard-`HWND` ausschließlich auf dem bestehenden Inject-Worker-
   Thread; der Hook auf einem eigenen Hook-Thread (SPEC §5). Jeder dieser
   Threads kombiniert Command-Polling (Channel) und Win32-Message-Pump.
   Fenster, Menüs, Icons, Notify-Icon und deren Cleanup werden **nur auf
   ihrem Owner-Thread** erzeugt und zerstört; fremde Threads wecken per
   Channel oder `PostMessageW`. **`AttachThreadInput` wird nicht verwendet.**
   `HWND`s werden nur als opake `WindowId` gespeichert, nie dereferenziert.
3. **Inject nutzt das bestehende Protokoll** (`inject_paste`,
   `serve_restored_until_read`, `RestoreSession`) und implementiert nur
   `ClipboardHost` neu. Die getesteten Regeln (kein Restore ohne bedienten
   Read, Fokus-Vergleich, Modifier-Lösen) bleiben unverändert.
4. **Eigene Clipboard-Generation statt „Sequenznummer unverändert".** Das
   eigene `WM_RENDERFORMAT` erhöht `GetClipboardSequenceNumber()`; deshalb
   führt der Sink `expected_seq`, das nach jeder eigenen erfolgreichen
   Mutation (EmptyClipboard/SetClipboardData/Render) aktualisiert wird.
   `still_owner` = `GetClipboardOwner()==hwnd && seq==expected_seq`
   (Vergleich nur per Gleichheit, DWORD-Wrap egal).
5. **Scope zügig, ehrlich benannt:** WP1–5 ergeben den Dev-Milestone; WP6/7
   sind Release-Voraussetzung und werden nicht als „Feinschliff" geführt.

## Arbeitspakete

### ✅ WP1 — Cargo/cfg-Grundlagen

- `Cargo.toml`: `betrayer` nach `[target.'cfg(target_os = "linux")'.dependencies]`;
  neu `[target.'cfg(windows)'.dependencies] windows-sys = { version = "=0.61.2", features = [...] }`.
  Feature-Tabelle (wird beim Build verifiziert, überflüssige gestrichen):

  | API | Feature |
  |---|---|
  | `SetWindowsHookExW`, `CallNextHookEx`, `GetMessageW`, `PostThreadMessageW`, `CreateWindowExW`, `DefWindowProcW`, `RegisterClassW`, `TrackPopupMenu`, `CreatePopupMenu`, `SetForegroundWindow`, `GetForegroundWindow`, `GetWindowThreadProcessId`, `RegisterWindowMessageW`, `KBDLLHOOKSTRUCT` | `Win32_UI_WindowsAndMessaging` |
  | `SendInput`, `GetAsyncKeyState`, `INPUT`, `VK_*` | `Win32_UI_Input_KeyboardAndMouse` |
  | `Shell_NotifyIconW`, `NOTIFYICONDATAW`, `NIN_*` | `Win32_UI_Shell` |
  | `OpenClipboard`, `EmptyClipboard`, `SetClipboardData`, `GetClipboardData`, `GetClipboardOwner`, `GetClipboardSequenceNumber`, `IsClipboardFormatAvailable`, `EnumClipboardFormats`, `CountClipboardFormats` | `Win32_System_DataExchange` |
  | `GlobalAlloc`, `GlobalLock`, `GlobalUnlock`, `GlobalSize`, `GlobalFree` | `Win32_System_Memory` |
  | `CreateMutexW`, `OpenProcess`, `QueryFullProcessImageNameW`, `CreateEventW`, `WaitForSingleObject`, `MsgWaitForMultipleObjects` | `Win32_System_Threading` |
  | `SetConsoleCtrlHandler`, `AttachConsole`, `GetStdHandle` | `Win32_System_Console` |
  | `CreateIconIndirect`, `CreateDIBSection`, `DeleteObject`, `DestroyIcon`, `BITMAPINFO` | `Win32_Graphics_Gdi` |
  | `GetModuleHandleW` | `Win32_System_LibraryLoader` |
  | `HANDLE`, `HWND`, `CloseHandle`, `GetLastError`, `ERROR_ALREADY_EXISTS`, `ERROR_INVALID_HANDLE` | `Win32_Foundation` |

- Kein Registry-, SID- oder COM-Feature in dieser Phase (Entscheidungen
  unten).
- Gate: `cargo build` (Windows) grün; `cargo check --target x86_64-unknown-linux-gnu`
  grün (nur check, Linux-Pfad bleibt unverändert). Beide Kommandos gehören
  in jedes WP-Gate.

### ✅ WP2 — Hotkey: `WH_KEYBOARD_LL` (`src/hotkey.rs`, Modul `windows`)

- `WinHookBackend`: **persistenter Hook-Thread** mit eigener Message-Queue.
  `register()` startet den Thread beim ersten Mal und wartet auf ein
  Ready-/Fehler-Handshake (Channel); der Thread selbst ruft
  `SetWindowsHookExW(WH_KEYBOARD_LL, proc, GetModuleHandleW(NULL), 0)` und
  pumpt mit `GetMessageW`. `unregister()` schickt eine Command-Message
  (`WM_APP+1` per `PostThreadMessageW`, erst nach Ready, sonst Channel),
  der Thread entfernt den Hook per `UnhookWindowsHookEx` selbst und
  bestätigt; der Thread bleibt stehen und wartet auf erneutes `register()`.
  Beide Operationen sind idempotent; `Drop` sendet Stop, joint mit Timeout.
  `GetMessageW == -1`, Threadtod oder Hook-Fehler → `HotkeyError::Failed`
  mit `GetLastError`.
- Hook-Proc (thread-lokaler State, kein globaler Mutex im Callback):
  - `nCode < 0` oder `LLKHF_INJECTED`/`LLKHF_LOWER_IL_INJECTED` gesetzt →
    sofort `CallNextHookEx` (unsere `SendInput`-Events aus dem Paste-Pfad
    dürfen nie als Hotkey gelten).
  - Down: `vkCode` == VK des konfigurierten Keys **und** exakter
    Modifier-Vergleich per `GetAsyncKeyState` (L/R-Varianten
    zusammengefasst, Lock-Tasten ignoriert; Shift+F9 ist bei Config `F9`
    **kein** Treffer). Treffer → `accepted_down = true`, `Press` senden,
    `return 1`.
  - Auto-Repeat-Down bei `accepted_down` → schlucken, kein Event.
  - Up mit `accepted_down` → unabhängig vom Modifier-Zustand `Release`
    senden, `accepted_down = false`, `return 1` (sonst kommt der Release in
    der Zielanwendung an).
  - Bei `unregister` State zurücksetzen (ein gehaltener Key wird dann nicht
    mehr geschluckt — akzeptiert, identisch zur Pause-Semantik in §5.2).
- VK-Mapping `HotkeySpec.key` → VK analog `x11_keysym` (F1–F24, A–Z, 0–9,
  Space, Insert, …), unbekannt → `HotkeyError::Failed`.
- Tooltip bei Fehler: `Hotkey nicht verfügbar (<Chord>): <Win32-Fehler>`.
- Tests: VK-Mapping, Modifier-Vergleich, State-Maschine Down/Repeat/Up
  (reine Funktionen ohne Win32-Aufruf).
- Gate: `--hotkey-test` loggt F9 Press/Release; F9 erreicht Notepad nicht;
  Shift+F9 erreicht Notepad.

### ✅ WP3 — Inject: Clipboard + `SendInput` (`src/inject/windows.rs`)

`Win32OutputSink` implementiert `ClipboardHost` + `OutputSink`; erzeugt auf
dem Inject-Worker-Thread (dort läuft bereits `serve_for(10 ms)` im Idle,
`daemon/workers.rs:883`).

- **Fenster:** `HWND_MESSAGE`-Fenster (Message-only) als Clipboard-Owner,
  `WndProc` mit thread-affinem State (`SetWindowLongPtrW(GWLP_USERDATA)`).
  `pump(timeout)` = `MsgWaitForMultipleObjects(0, NULL, FALSE, timeout, QS_ALLINPUT)`
  + `PeekMessageW`-Schleife; liefert `PumpEvents { reads, lost_ownership }`.
  **Die Pump läuft dauerhaft** — auch nach `copy_only`, Fokusverlust,
  TrayClick und im Idle (`serve_for`), sonst hängen Win+V/Clipboard-
  Manager an `WM_RENDERFORMAT`.
- **Snapshot** (`snapshot_clipboard`): `OpenClipboard(hwnd)` mit begrenztem
  Retry (bis 10×, dazwischen pumpen; Clipboard-Manager halten es kurz).
  `CountClipboardFormats()==0` → `Text("")` (wirklich leer);
  `CF_UNICODETEXT` verfügbar → `GetClipboardData` → `GlobalLock`/`GlobalSize`
  → UTF-16 bis zur ersten NUL, `from_utf16_lossy` → `Text`; sonst `NonText`.
  Sequenznummer merken.
- **Owner werden** (`become_owner(text)`): `OpenClipboard` → `EmptyClipboard`
  → **Delayed Rendering** `SetClipboardData(CF_UNICODETEXT, NULL)` →
  `CloseClipboard`; `expected_seq = GetClipboardSequenceNumber()`.
  - `WM_RENDERFORMAT(CF_UNICODETEXT)`: **kein** `OpenClipboard`; Text als
    `GlobalAlloc(GMEM_MOVEABLE)` UTF-16+NUL per `SetClipboardData` liefern,
    **`reads += 1`**, `expected_seq` auf die neue Sequenznummer setzen.
  - `WM_RENDERALLFORMATS`: `OpenClipboard(hwnd)`, erneut prüfen
    `GetClipboardOwner()==hwnd`, dann eager setzen, `CloseClipboard`.
  - `WM_DESTROYCLIPBOARD`: nicht blind als fremd werten — nur wenn
    `GetClipboardOwner()!=hwnd` **oder** aktuelle Sequenz ≠ `expected_seq`
    und kein eigener Übergang gerade läuft (Guard-Flag) → `lost_ownership`.
- `still_owner`: Leitentscheidung 4.
- `release_ownership` (Restore): nur wenn `still_owner`; Snapshot-Text
  **eager** setzen (kein Delayed Rendering nötig); `expected_seq`
  aktualisieren.
- `set_serve_text`: Text für spätere Renders austauschen.
- `save_to_clipboard_manager`: Windows-Äquivalent = aktuellen Text eager
  rendern (wie `WM_RENDERALLFORMATS`), Rückgabe `Saved`; vor Quit aufrufen,
  damit der Text den Prozess überlebt.
- **Modifier/Tasten:** `query_modifiers` per `GetAsyncKeyState`
  (VK_SHIFT/VK_MENU/VK_LWIN|VK_RWIN/VK_CONTROL, High-Bit). `key_down`/
  `key_up`: `SendInput` je Taste; `PasteKey` → `VK_SHIFT`, `VK_MENU`,
  `VK_LWIN`, `VK_CONTROL`, `'V'`, `VK_INSERT` (**mit `KEYEVENTF_EXTENDEDKEY`**).
  **Rückgabewert prüfen** (== Anzahl Events); bei Fehler alle in diesem
  Paste selbst gedrückten Tasten best-effort in umgekehrter Reihenfolge
  lösen und `InjectError::Failed` liefern (Transkript bleibt im Clipboard,
  §10).
- `current_window`: `GetForegroundWindow()` → `WindowId(hwnd as u64)`;
  `0` → `None` (= Fokusverlust/copy_only, §7.3). Unmittelbar vor dem ersten
  Key-Event erneut prüfen (macht `inject_paste` bereits).
- `wm_class(window)` — **auf Windows: Trait-Platzhalter = `(exe_basename, exe_basename)`**:
  `GetWindowThreadProcessId` → `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)`
  (RAII-`CloseHandle`) → `QueryFullProcessImageNameW` (wachsender Puffer).
  Zugriffsfehler (elevated Ziel) → `None`, **kein** Inject-Fehler.
  `protocol::auto_shortcut` um Windows-Regel erweitern: Basename
  ASCII-case-insensitive == `WindowsTerminal.exe` → `CtrlShiftV`, sonst
  `CtrlV` (§7.2); Test in `protocol.rs`.
- Tests (Fake-Host, plattformneutral): „eigener Render erhöht Sequenz und
  bleibt Owner", „fremder Copy vor Render → ForeignOwner", „Owner gleich,
  Sequenz fremd → kein Restore", `SendInput`-Teilfehler löst Tasten.
- Gate: `--inject-test "Grüße, Jörg — zweite Zeile\nfertig"` in Notepad
  und Windows Terminal (Ctrl+Shift+V-Regel), vorheriger Clipboard-Text
  danach wieder da; mit Win+V-History **an und aus** (False-Positive-Read
  laut §7.1 akzeptiert, aber im Log sichtbar); Fokuswechsel während der 3 s
  → copy_only.

### ✅ WP4 — Tray: `Shell_NotifyIconW` (`src/tray.rs`, Modul `windows`)

- `Win32Tray::new`: läuft auf dem **Tray-Worker-Thread**; versteckte
  Top-Level-Fensterklasse (kein `HWND_MESSAGE` — Broadcasts wie
  `TaskbarCreated` und `WM_ENDSESSION` kommen nur bei echten Top-Level-
  Fenstern an). `RegisterWindowMessageW("TaskbarCreated")` → 0 ist ein
  Fehler. `Shell_NotifyIconW(NIM_ADD)` mit
  `NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP`, danach `NIM_SETVERSION`
  (`NOTIFYICON_VERSION_4`); beide Rückgabewerte prüfen, Fehler →
  `TrayError::Failed` → Exit 1 (§10). Nach `TaskbarCreated`: komplette
  Sequenz `NIM_ADD` + `NIM_SETVERSION` wiederholen.
- `poll`/`update` vom Worker: Loop kombiniert `PeekMessageW`-Pump mit dem
  Command-Channel (`update` legt Tooltip/Status in einen Mutex-Slot und
  postet `WM_APP+2`; der Owner-Thread ruft `NIM_MODIFY`).
- Tooltip: `<status> · <model_key>` bzw. Fehlertext aus vorhandenem
  Mapping; auf **127 UTF-16-Codeunits + NUL** kürzen.
- Icons: pro `TrayStatus` ein `HICON` in **einer** Größe
  (`GetSystemMetrics(SM_CXSMICON)`), erzeugt aus 32-bpp-DIB
  (`CreateDIBSection`, gefüllter Farbkreis wie Linux-`IconSet`) +
  `CreateIconIndirect`; temporäre `HBITMAP`s sofort `DeleteObject`, `HICON`s
  erst nach `NIM_DELETE` per `DestroyIcon` auf dem Owner-Thread. DPI 100/150 %
  im Smoke prüfen.
- Callback-Dekodierung (Version 4): Event = `LOWORD(lParam)`, Icon-ID =
  `HIWORD(lParam)`, Anker = `GET_X/Y_LPARAM(wParam)`.
  `NIN_SELECT`/`NIN_KEYSELECT` → `LeftClick` (genau einmal; `WM_LBUTTONUP`
  wird **nicht** zusätzlich ausgewertet). `WM_CONTEXTMENU` → Menü (nur
  dieses; kein `WM_RBUTTONUP`).
- Menü: `CreatePopupMenu`; feste IDs `1000 Status (MF_GRAYED|MF_STRING)`,
  `1001 Pausieren/Aktivieren`, `1002 Config-Ordner öffnen`, `MF_SEPARATOR`,
  `1003 Beenden`; geschlossenes Mapping → `MenuAction`. Ablauf laut
  SPEC §4.2-Ausnahme: `SetForegroundWindow(hwnd)` →
  `TrackPopupMenu(TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, x, y)` →
  `PostMessageW(hwnd, WM_NULL)` → `Shell_NotifyIconW(NIM_SETFOCUS)` →
  `DestroyMenu`. Während des blockierenden `TrackPopupMenu` werden Commands
  verzögert (akzeptiert; Menü ist Nutzerinteraktion).
- Session-Ende: `WM_QUERYENDSESSION` → `return TRUE` (nichts tun);
  `WM_ENDSESSION` mit `wParam != 0` → Quit-Latch setzen (siehe WP5);
  `wParam == 0` (Shutdown abgebrochen) ignorieren.
- Shutdown: `Drop` sendet nur Stop-Command und joint (Timeout → Log
  „Tray-Cleanup unvollständig"); der Owner-Thread schließt ein offenes Menü
  (`EndMenu`), `NIM_DELETE`, `DestroyWindow`, `UnregisterClassW`,
  `DestroyIcon`, dann Loop-Ende.
- `open_config_dir`: `explorer.exe <dir>` per `std::process::Command`
  (Stub-Fehler entfernen).
- Gate: Icon sichtbar, Tooltip `idle`, Rechtsklick-Menü vollständig und
  schließt bei Klick daneben, Linksklick startet/stoppt Aufnahme (Text im
  Clipboard), Pause gibt F9 frei, Beenden räumt Icon weg, Explorer-Neustart
  (`taskkill /f /im explorer.exe && start explorer`) bringt das Icon zurück.

### ✅ WP5 — Single-Instance, Signale, Subsystem, Autostart

- **Single-Instance** (`single_instance.rs`): plattformspezifische
  Lock-Abstraktion — Linux behält `FileLock`, Windows bekommt `MutexLock`
  (RAII, `CloseHandle` im Drop). Name
  `Local\FerberDiktier.v1.instance` (kein User-Hash nötig, `Local\` ist
  bereits per Session, §5.3). `CreateMutexW(NULL, FALSE, name)` → Handle
  `NULL` = Fehler (`GetLastError` sofort lesen; `ERROR_INVALID_HANDLE` =
  Namenskollision mit anderem Objekttyp, separat melden);
  `ERROR_ALREADY_EXISTS` → `Busy`. `describe()` nennt den Mutex-Namen.
  **Download-Lock** (`download.rs`): analog
  `Local\FerberDiktier.v1.download.<hex(sha256(model_dir))>`. Bestehende
  Tests (`second_lock_on_same_path_is_busy`,
  `parallel_download_is_refused_while_the_lock_is_held`) müssen auf Windows
  grün werden — der Stub lässt sie heute fälschlich passieren; prüfen, ob
  sie überhaupt Busy erzwingen, sonst schärfen.
- **Signale** (`daemon/signals.rs`, Windows): gemeinsamer atomarer
  Quit-Latch + `CreateEventW`-Ack. `SetConsoleCtrlHandler` (Rückgabe
  prüfen): `CTRL_C`/`CTRL_BREAK` → Latch setzen, `return TRUE`;
  `CTRL_CLOSE_EVENT` → Latch setzen, dann auf Cleanup-Ack warten (max 3 s,
  unter der Windows-Frist von 5 s), `return TRUE`. Der Daemon signalisiert
  das Ack nach seinem regulären Shutdown. `WM_ENDSESSION(TRUE)` aus WP4
  setzt denselben Latch.
- **Subsystem/Console** (`main.rs`): `#![cfg_attr(windows, windows_subsystem = "windows")]`.
  **Erster Schritt in `main`, vor Clap und vor `signals::install()`:**
  `AttachConsole(ATTACH_PARENT_PROCESS)`; bei Erfolg Standardhandles neu
  beziehen (`GetStdHandle` gültig → Rust-stdio funktioniert). Ohne Parent-
  Konsole **kein** `AllocConsole` (Entscheidung 2026-08-27): dann gibt es
  keine stderr-Ausgabe, der Daemon loggt weiter ins Datei-Log (§10), CLI-
  Modi liefern ihren Exitcode. Folge für den Bediener: PowerShell wartet
  auf ein GUI-Subsystem-Programm nicht — `--foreground` per
  `Start-Process -Wait -NoNewWindow` oder `& .\diktier.exe --foreground | Out-Host`;
  in README dokumentieren.
- **Autostart** (`autostart.rs`): SPEC §9 bleibt (Startup-Ordner,
  Entscheidung 2026-08-27). `.cmd` beibehalten, **ohne** `/min`; Pfade mit
  `"` ablehnen (`AutostartError`). `.lnk` via `IShellLinkW`/`IPersistFile`
  als Folge-WP notieren (braucht COM-Features). Gate: Install/Remove
  zweimal idempotent, Pfad mit Leerzeichen, Eintrag startet den Daemon
  beim Login (Tray-Icon erscheint).
- Gate zusätzlich: `--help`, `--version`, unbekanntes Flag,
  `--transcribe-wav`, alle Spikes und Ctrl+C aus PowerShell zeigen Ausgabe
  und korrekten Exitcode; zweiter Daemon-Start → Exit 0 „läuft bereits";
  Ctrl+C und Fenster-Schließen beenden sauber (Icon weg, Clipboard-Text
  bleibt lesbar); Logoff-Test einmal manuell.

### ✅ WP6 — Release-Skript und Doku

- `scripts/release.ps1` (Build, Bundle, Zip, Setup-Exe via
  `installer\diktier.nsi`, `versions.toml`, Selbstprüfung) seit Phase 5
  vorhanden; erstes veröffentlichtes Release ist
  [v0.2.0](https://github.com/ralfkuh-lab/diktier/releases/tag/v0.2.0)
  (2026-08-31, mit Aufnahme-Overlay).
- Gate am 2026-08-31 gefahren: 0.2.0-Zip in leeren Ordner **mit
  Leerzeichen im Pfad** entpackt, `--version` und `--transcribe-wav`
  (Modell-Load 2,2 s, Transkript korrekt, Exit 0).
- README hat Installations-Abschnitt und Status-Zeile. Die ursprünglich
  geplanten Windows-Messwerte in `docs/SPIKES.md` sind **gestrichen**
  (kein Nutzen mehr; die relevanten Zahlen stehen in diesem Plan und in
  den Release-Notes).

### ❌ WP7 — Release-Gates (SPEC §11/§12) — verworfen

Entscheidung Ralf 2026-08-31: Diktier bleibt ein privates Werkzeug für
diesen Rechner; Clean-VM-Gates (Win10/Win11, Inject-Matrix §12) werden
nicht gefahren. Releases (ab v0.2.0) gelten ohne dieses Gate — bewusst,
nicht vergessen. Sollte das Tool je an Dritte verteilt werden, ist
dieser Punkt neu zu bewerten.

## Plattform-Entscheidung 2026-08-27

Windows ist die **einzige Plattform** (Ralf). Linux wird nicht mehr
berücksichtigt — kein Mint-Gate, kein Linux-`cargo check`, keine
Linux-Pendants. **Nachtrag 2026-08-31: Der Linux-Code (X11-Hotkey,
betrayer-Tray, Pulse/ALSA-Pfade, FileLock, Unix-Signale, Shell-Skripte)
ist vollständig entfernt.**

## Nicht in dieser Phase (bewusst, kein Feinschliff-Etikett)

- Gerätewahl-UI (der Mikrofon-**Pegel** ist seit dem Aufnahme-Overlay
  sichtbar, [overlay-plan.md](overlay-plan.md) — das damalige
  Jabra-Rätsel „RMS 0,0007" war tatsächlich Mute am Headset).
- `output.mode = "type"`, Notifications, Icon-Design, `.lnk`-Autostart.
- ❌ **Modifier-only-Hotkey** (z. B. `Ctrl+Win` wie bei Wispr Flow,
  Wunsch Ralf 2026-08-27) — **verworfen 2026-08-31**: `RCtrl` als
  Einzeltaste (seit 82a9155) deckt den Bedarf; das eigene
  Hook-Zustandsmodell lohnt nicht mehr.
- ✅ **Aufnahme-Indikator** (Wunsch Ralf 2026-08-27): Tray-Icons lassen sich
  unter Win11 nicht programmatisch sichtbar erzwingen (einmal manuell
  anheften, pfadgebunden). Umgesetzt als randloses Layered-Window-Overlay
  (`WS_EX_LAYERED|WS_EX_TOPMOST|WS_EX_TOOLWINDOW|WS_EX_NOACTIVATE|WS_EX_TRANSPARENT`
  plus `WM_NCHITTEST → HTTRANSPARENT`) mit Mikrofonpegel, sichtbar von
  `recording` bis `idle`, kein Fokuswechsel — SPEC §4.2/§4.5-konform. Plan und
  Verträge: [overlay-plan.md](overlay-plan.md).
- ❌ Sol-Gesamtreview-Restpunkte — **geschlossen 2026-08-31 als
  akzeptiertes Restrisiko** (privates Werkzeug, keiner der Punkte hat
  sich im Alltag gemeldet): `CTRL_CLOSE_EVENT` ohne Cleanup-Ack (nur
  `--foreground`-Konsole), `WM_ENDSESSION` nur über Poll-Queue,
  Tray-Retry nach fehlgeschlagenem `NIM_ADD`, Pfad-Kanonisierung
  Download-Mutex.
- ❌ Watchdog gegen still entfernten LL-Hook (`LowLevelHooksTimeout`) —
  Restrisiko akzeptiert (unverändert seit Paket A).

## Umsetzung

Delegation an Opus in vier Paketen: **A** WP1+WP2, **B** WP3, **C** WP4,
**D** WP5 (+WP6, wenn im Budget). Je Paket eine frische Opus-Session mit
Briefing (dieser Plan + SPEC-Abschnitte + Sol-Review); Zweit-Review durch
Sol (`gpt-5.6-sol` via copilot, effort medium) nach
`docs/reviews/impl-phase5-<paket>-sol.md`; Orchestrator fährt die Gates
auf diesem Rechner selbst nach. Commit je Paket; kein Push ohne Freigabe.
