# Phase 5, Paket D — ScrollLock/Pause + Config direkt öffnen

Stand: 2026-08-27, Claude Opus 5.0, Windows 11 MSVC x64. Nicht committet.

## ✅ 1 — Hotkey-Namen `ScrollLock` und `Pause`

`config.rs` `NAMED_KEYS` erweitert (Normalizer greift beide case-insensitiv);
Alias „Rollen" → `ScrollLock` als zwei Zeilen vor der Schleife, im Stil von
`parse_modifier`, keine neue Struktur. `hotkey.rs`: VK `0x91`/`0x13`, Keysym
`0xff14`/`0xff13`, die drei Mapping-/Vollständigkeitstests mitgezogen plus
neuer Config-Test `lock_keys_are_valid_hotkeys`. Kommentar `# z. B. "F9",
"ScrollLock", "Pause"` in `DEFAULT_TOML` — die Default-Datei sieht der Nutzer ab
jetzt per Menü. **README bewusst nicht angefasst** (Orchestrator schreibt sie
parallel neu); die gleiche Zeile fehlt dort noch im Config-Beispiel, ebenso die
Menü-Umbenennung im Tray-Absatz.

## ✅ 2 — Tray öffnet `config.toml` statt des Ordners

Menütext (Linux + Win32) → „Konfiguration bearbeiten"; `open_config_dir` →
`open_config` (4 Aufrufstellen), legt die Datei bei Bedarf per
`config::load_from` an (schreibt `DEFAULT_TOML` atomar inkl. Ordner) und öffnet
sie mit `explorer.exe <datei>` (Standard-App, kein Konsolenblitzer), Linux
`xdg-open <datei>`. Log in `daemon/mod.rs` (2×): „Konfiguration geöffnet —
Änderungen gelten nach Neustart". Enum-Varianten `OpenConfigDir` blieben
absichtlich stehen (Churn ohne Nutzen).

## ✅ Gates

Build grün, einzige Warnung die vorbestehende `x11_keysym is never used`
(Linux-only). `cargo test`: 298 passed, 0 failed, 1 ignored. `cargo fmt --check`
sauber. Smoke mit `key = "Rollen"` (testet Alias *und* Registrierung):
`--hotkey-test --foreground` meldet `SPIKE: ScrollLock …` auf `win32-ll-hook`;
Config danach per `diff` verifiziert zurückgespielt. `.gitignore`:
`/target-dev/` ergänzt, `/target/` deckte es nicht ab. Nebenbei entfernt:
`tray::config_dir()` war nach dem Umbau der einzige tote Rest (`#![allow(dead_code)]`
in `tray.rs` hätte ihn verschwiegen).

## ⚠️ Offener Punkt

`.toml` ist hier keiner App zugeordnet (`assoc .toml` → nicht gefunden), der
erste Klick zeigt daher Windows' „Wie möchtest du diese Datei öffnen?"; nach
einmaliger Auswahl merkt Windows es sich. Harter Fallback auf `notepad.exe`
würde die Nutzerwahl übergehen — bewusst nicht eingebaut.
