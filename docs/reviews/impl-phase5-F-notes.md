# Phase 5, Paket F — Autostart-Toggle, Icon, Release-Skript, Installer

Stand: 2026-08-27, Claude Opus 5.0, Windows 11 MSVC x64. Nicht committet.

## ✅ Dateien

| Datei | Änderung |
|---|---|
| `src/autostart.rs` | neu `is_installed()` (Eintrag im Startup-Ordner vorhanden?) |
| `src/tray.rs` | `TrayEvent`/`MenuAction::ToggleAutostart`, Windows-Menü-ID **1005** „Mit Windows starten" mit `MF_CHECKED`/`MF_UNCHECKED`, über „Hotkey ändern…"; 4 Tests erweitert |
| `src/daemon/workers.rs` | `Msg::ToggleAutostart`, Dispatch im `tray_loop`, `tray_event_to_core` → `None` |
| `src/daemon/mod.rs` | freie Fn `toggle_autostart(&Logger)` (install/remove + Log-Zeile), auch im `config_error_mode` bedienbar |
| `build.rs` | **neu**, cfg(windows): Icon + Versionsressource via `winresource` |
| `Cargo.toml`/`.lock` | `[target.'cfg(windows)'.build-dependencies] winresource = "0.1.31"`; Lock +4 Knoten (winresource, toml 1.1.4, serde_spanned, toml_writer) |
| `assets/diktier.ico` | **neu**, 16/32/48/256: Idle-grüner Kreis, weißes Mikrofon |
| `scripts/make-icon.py` | **neu**, Pillow-Generator (8-fach überabgetastet) |
| `scripts/release.ps1` | **neu**, `-TargetDir`/`-SkipBuild`/`-SkipInstaller` |
| `installer/diktier.nsi` | **neu**, MUI2, Unicode, per-User, deutsch + englisch |
| `README.md` | „Installation" vor „Bauen und starten", `release.ps1` ergänzt, Tray-Absatz erweitert |

## ⚠️ Abweichungen

1. **Bundle-README = Kopie der Repo-README** statt gekürzter Fassung wie in `release.sh` — die Repo-README ist bereits die Windows-Anleitung.
2. **Linux: Menüpunkt weggelassen** (wie „Hotkey ändern…"); `TrayEvent`/`MenuAction` bleiben plattformneutral, `tray.rs` hat `#![allow(dead_code)]`, also keine Warnung.
3. **`is_installed() -> bool`**, kein `Result`: Aufrufer ist ein Menüaufbau ohne Fehlerkanal; ein Fehler beim Klick wird geloggt.
4. **`.ps1` und `.nsi` mit UTF-8-BOM.** Ohne BOM liest Windows PowerShell 5.1 CP1252 — das dritte Byte von `—` wird dort zu `”` und beendet Strings.
5. **Keine Sprachauswahl-Dialoge** (`MUI_LANGDLL_DISPLAY` weg): NSIS wählt nach Systemsprache.
6. **Zusätzliches Define `/DOUTFILE=…`** neben `VERSION`/`SRCDIR` — `OutFile` kommt aus dem Define und landet direkt in `dist\`.
7. **Keine neuen Testfunktionen**, nur bestehende erweitert (Menü-IDs, Routing).

## ✅ Gates

- `CARGO_TARGET_DIR=target-dev cargo build --release`: grün, **eine** Warnung — die vorbestehende `x11_keysym is never used`. Kein `cargo:warning` aus `build.rs`; Exe trägt Icon und `ProductName=Diktier`.
- `cargo test`: `308 passed; 0 failed; 1 ignored`. `cargo fmt --check`: sauber.
- `powershell -File scripts\release.ps1 -TargetDir target-dev` durchgelaufen; makensis (`%LOCALAPPDATA%\tauri\NSIS\makensis.exe`, Script als UTF8 erkannt) **ohne Warnung**: „6 pages, 3 sections (1 required), 694 instructions, 2 language tables", „Total size: 6126002 / 22479577 bytes (27.2%)".
- `dist\diktier-0.1.0-win-x64.zip` 8,0 MB, `dist\Diktier_0.1.0_x64-setup.exe` 5,8 MB (ProductVersion 0.1.0, eigenes Icon). Setup **nicht** ausgeführt (Auftrag).

## 🔍 Offene Punkte

- **Setup-Exe nie ausgeführt**: Installation, Autostart-Sektion, Startmenü, Finish-„Diktier jetzt starten", Uninstaller inkl. Purge-Abfrage sind nur gelesen.
- **Menüpunkt nicht bedient** (Auftrag: Orchestrator): Häkchen, beide Richtungen, Log-Zeile.
- **Linux-Gate offen**: Cross-Toolchain fehlt hier (unverändert zu Paket A/C/E).
- `dist/` stand schon als `/dist/` in `.gitignore`.
