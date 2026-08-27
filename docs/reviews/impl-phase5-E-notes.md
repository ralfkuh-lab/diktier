# Phase 5, Paket E — „Hotkey ändern…"-Dialog

Stand: 2026-08-27, Claude Opus 5.0, Windows 11 MSVC x64. Nicht committet.

## ✅ Dateien

| Datei | Änderung |
|---|---|
| `src/hotkey_dialog.rs` | **neu**, cfg(windows): Fenster + WndProc + eigene Message-Loop, `ask()`, `close_open_dialog()` (nur Spike), 5 Tests |
| `src/hotkey.rs` | `mod windows` → `pub(crate)`; `NAMED_KEYS`-Tabelle für **beide** Richtungen, neu `vk_name`, 2 Tests |
| `src/config.rs` | `save_hotkey(path, key, modifiers)` + `modifier_config_name` (cfg(windows), `toml_edit`), 2 Tests |
| `src/tray.rs` | Menü-ID 1004 „Hotkey ändern…" über „Konfiguration bearbeiten", `TrayEvent`/`MenuAction::ChangeHotkey` |
| `src/daemon/workers.rs` | `Msg::{ChangeHotkey, HotkeyChanged}`, `HotkeyCmd::Rebind` + `HotkeyWorker::rebind`, `grabbed`-Flag im `hotkey_loop` |
| `src/daemon/mod.rs` | `open_hotkey_dialog`/`finish_hotkey_dialog`/`report_hotkey_not_saved`, Felder `hotkey_spec`/`hotkey_dialog_open` |
| `src/main.rs` | `mod hotkey_dialog`, Spike `--hotkey-dialog-test [AUTOCLOSE_SECS]`, 1 Test |
| `Cargo.toml`/`.lock` | `toml_edit = "0.22"` (stand transitiv schon im Lock; Lock +1 Zeile, kein neuer Knoten) |

## ⚠️ Abweichungen

1. **Eigener Thread statt Tray-Thread** (die im Auftrag genannte Alternative):
   `TrayWorker::shutdown` joint mit 2 s — ein im Dialog stehender Tray-Thread ließe beim
   Beenden das Icon stehen. So bleibt der Daemon bedienbar; ein Flag verhindert ein
   zweites Fenster. Ungrab vor / Grab nach dem Dialog: 2 Zeilen im Daemon.
2. **`vk_name` liefert `Option<String>`**, nicht `&'static str` — F1–F24, A–Z, 0–9 sind
   gerechnet; ein statischer Name bräuchte die zweite Liste, die der Auftrag vermeidet.
3. **`toml_edit`** statt `to_string_pretty`: alle Kommentare bleiben stehen, auch der
   hinter der geänderten `key`-Zeile (Test deckt es ab).
4. **Schreibfehler**: Log **plus** einmaliges Tray-Update `error` ohne `shown` anzufassen
   (§4.3 kennt keinen Warnkanal). Verschwindet beim nächsten Zustandswechsel; der neue
   Hotkey greift trotzdem sofort.
5. **`SS_CENTER`/`SS_CENTERIMAGE` lokal** statt Feature `Win32_System_SystemServices` für
   zwei stabile `winuser.h`-Werte (wie `NIN_KEYSELECT`/`CF_UNICODETEXT`).
6. **Linux**: kein Menüpunkt; `Msg::HotkeyChanged`/`HotkeyCmd::Rebind` cfg(windows) gegen
   „never constructed". Linux weiterhin **nicht** kompiliert (Cross-Toolchain fehlt).

## ✅ Gates

- `build --release` grün, **eine** Warnung — die vorbestehende `x11_keysym is never used`.
- `cargo test`: `308 passed; 0 failed; 1 ignored` (vorher 298). `cargo fmt --check` sauber.
- Spike `--hotkey-dialog-test 5`: Fenster „Diktier – Hotkey", 376×215, zentriert, schließt
  sich selbst → `abgebrochen`, Exit 0. `PrintWindow`-Screenshot: Prompt, großes Feld
  (`ScrollLock`), zweizeiliger Hinweis, beide Schaltflächen.
- Tasten per `PostMessageW` ans Fenster: `F12`+`Enter` → `übernommen F12`;
  `CapsLock`+`Enter`+`Esc` → `abgebrochen` (Übernehmen gesperrt); echtes `Ctrl+Alt`
  gehalten + `F12` → `übernommen Ctrl+Alt+F12`, `modifiers=[Ctrl, Alt]`.

## 🔍 Offene Punkte

- **Ende-zu-Ende im Daemon ungetestet** (zweiter Daemon scheitert am Instanz-Mutex): Klick
  → Übernehmen → `config.toml` geschrieben → neue Taste sofort scharf, alte tot; Abbrechen
  ändert nichts; „Beenden" bei offenem Dialog.
- **Pause-Wechsel bei offenem Dialog** greift die alte Taste wieder — enger Fall, offen.
- **Win-Taste**: `Super`-Chords kaum wählbar, die Shell fängt sie vorher ab.
- **DPI**: nur 100 % gesehen, feste Pixelmaße, kein `GetDpiForWindow`.
