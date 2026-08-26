# Diktier — Spec v1

Stand: 2026-08-26. Verbindlich für die Implementierung. Änderungen nur über
diesen Text.

## 1. Ziel

Ein lokales Push-to-Talk-Diktiertool für **Windows 10+** und **Linux Mint
(Cinnamon)**. Der Nutzer hält eine Taste, spricht, lässt los. Nach der
Transkription steht der Text im gerade fokussierten Eingabefeld.

Qualitätziel: dieselbe Erkennungsliga wie Voxtype auf Omarchy
(`parakeet-tdt-0.6b-v3-int8`). Diktier ersetzt WhisperDictate auf den
Nicht-Omarchy-Rechnern. WhisperDictate bleibt unangetastet.

v1 ist bewusst schmal: Daemon, PTT, Parakeet, Einfügen am Cursor, Tray,
Config, Autostart. Kein Preview-Dialog.

## 2. Nicht-Ziele (v1)

- Preview-/Korrektur-Dialog (kommt ggf. als v2, zweiter Hotkey)
- Whisper / faster-whisper
- Streaming (Nemotron)
- HTTP-API / Anbindung Local-AI Cockpit
- macOS, Omarchy/Hyprland als Zielplattform
- Meeting-Transkription, Diarization
- OSD/Waveform-Overlay (Tray-Zustand reicht)
- Cloud, Telemetrie
- Whisper-`initial_prompt`
- Tastensimulation als Default-Ausgabe (siehe §7)

## 3. Nutzer und Plattformen

| Plattform | Session | Audio | Hotkey v1 | Ausgabe v1 |
|---|---|---|---|---|
| Windows 10/11 x64 | Desktop | WASAPI via cpal | nativer Low-Level-Hook | Clipboard + Paste, Restore |
| Linux Mint, Cinnamon, X11 | typischer Alltag | PipeWire/Pulse via cpal | `global-hotkey` / X11 | Clipboard + Paste, Restore |
| Linux Mint, Cinnamon, Wayland | Best-effort | wie X11 | evdev, falls X11-Grab versagt | `wtype`/`dotool`/`ydotool`, sonst Clipboard |

Kein Admin/root für den Normalbetrieb. Auf Linux darf evdev die Gruppe
`input` brauchen — das ist der einzige erwartbare Extra-Schritt, und nur
falls der X11-Hotkey nicht greift.

Kein CUDA-Zwang. v1 läuft auf CPU (INT8). Die GT 750M / Kepler-Klasse und
typische Büro-PCs ohne nutzbare GPU sind der Designpunkt.

## 4. UX

### 4.1 Push-to-Talk

1. Hotkey **drücken und halten** → Aufnahme startet. Tray wechselt auf
   „recording“. Kein Fenster, kein Fokuswechsel.
2. Sprechen.
3. Hotkey **loslassen** → Aufnahme stoppt, Transkription läuft. Tray
   „transcribing“.
4. Fertig: Text wird in das **weiterhin fokussierte** Fenster eingefügt.
   Tray zurück auf „idle“.

Leere Aufnahme, nur Stille oder Transkript, das nach Normalisierung leer
ist: nichts einfügen, kein Fehlerdialog. Tray kurz „idle“.

Maximale Aufnahmedauer: 60 s, danach hart stoppen und transkribieren wie
beim Loslassen. Optionaler Hinweis im Tray-Tooltip.

### 4.2 Fokusregel (nicht verhandelbar)

Der PTT-Pfad darf **kein** Fenster öffnen, das den Fokus nimmt. Kein
Tk/GTK-Dialog, keine unsichtbare Top-Level-Window, die den Input stehlt.
Feedback nur über Tray-Icon, Tray-Tooltip und optional eine
Desktop-Notification bei Fehlern (Modell fehlt, Mic tot, Paste
fehlgeschlagen).

### 4.3 Tray

Crate: `betrayer` (Windows nativ, Linux StatusNotifierItem). Nicht
`tray-icon`, weil das auf Linux GTK3+AppIndicator nachzieht.

Zustände am Icon (Farbe oder Badge, ein Satz Icons reicht):

| Zustand | Bedeutung |
|---|---|
| idle | geladen, bereit |
| recording | PTT gehalten |
| transcribing | Modell arbeitet |
| error | Mic/Modell/Inject kaputt; Tooltip erklärt |

Menü (Rechtsklick):

- Statuszeile (nicht klickbar): Zustand + Modellname
- Hotkey pausieren / wieder aktivieren
- Config-Ordner öffnen
- Beenden

Linksklick: Toggle-Aufnahme (Start/Stop), als Fallback wenn der Hotkey
nicht greift. Das ist **kein** PTT — einmal klicken startet, nochmal
stoppt und transkribiert. Während PTT gehalten wird, ignoriert der
Linksklick.

### 4.4 Default-Hotkey

`F9`, ohne Modifier, Push-to-Talk. Konfigurierbar. Begründung: auf
Omarchy bereits als PTT belegt, auf Mint/Windows selten vergeben.

Konflikt: wenn F9 in einer App belegt ist, gewinnt die App nicht — der
globale Hook schon. Deshalb muss der Hotkey wechselbar sein, bevor jemand
täglich damit arbeitet. Wechsel nur über Config-Datei in v1 (kein
Settings-GUI).

## 5. Architektur

Ein Prozess, ein Binary `diktier`. Kein Python, kein Sidecar-Server.

```
diktier (Daemon)
  ├── tray        betrayer, UI-Thread / Event-Loop
  ├── hotkey      Plattform-Hook, PTT press/release
  ├── audio       cpal, 16 kHz mono f32, resample falls nötig
  ├── engine      parakeet-rs, Modell resident
  ├── inject      Plattform: Clipboard+Paste (Default)
  └── config      TOML, einmal laden, bei Pause/Resume nicht heiß neu
```

Modell bleibt im RAM (`on_demand_loading = false`). INT8 v3 ist ~640 MB
auf Disk, Inferenz auf CPU. Kaltstart darf einige Sekunden brauchen;
danach muss PTT ohne Ladeverzögerung aufnehmen (Aufnahme startet sofort,
auch wenn das Modell noch lädt — Transkription wartet).

Single-Instance: zweiter Start bringt den laufenden Prozess in den
Vordergrund (Tray-Balloon / Log) und beendet sich. Lock-Datei unter
`$XDG_RUNTIME_DIR/diktier.lock` bzw. `%LOCALAPPDATA%\diktier\diktier.lock`.

### 5.1 Module (vorgeschlagen)

```
src/main.rs
src/config.rs
src/state.rs          idle | recording | transcribing | error
src/audio.rs
src/engine.rs         Trait Transcriber, parakeet-rs dahinter
src/hotkey.rs         cfg(windows) / cfg(unix)
src/inject.rs         cfg(windows) / cfg(unix)
src/tray.rs
src/download.rs       Modell-Dateien beim ersten Start
```

`engine` kennt keine GUI. `inject` kennt kein Audio. Tray und Hotkey
senden Kommandos in eine zentrale State-Machine (ein Thread oder
async-Runtime, eine Queue).

Runtime: `tokio` ist ok, muss aber nicht sein. Ein Worker-Thread für
Inferenz plus cpal-Callback plus Tray-Eventloop reichen. Keine
Anforderung an eine bestimmte Runtime, solange der Inferenz-Thread den
Tray-Thread nicht blockiert.

## 6. Engine und Modelle

### 6.1 Runtime

Erste Wahl: [`parakeet-rs`](https://github.com/altunenes/parakeet-rs) über
ONNX Runtime (CPU).

Grund: dieselben Dateien wie Voxtype auf Omarchy:

```
encoder-model.int8.onnx
decoder_joint-model.int8.onnx
vocab.txt
```

Damit ist der Qualitätsvergleich mit Omarchy ehrlich. Den TDT-Decode-Loop
nicht selbst schreiben.

Fallback, falls der STT-Spike mit `parakeet-rs` scheitert (leere
Ergebnisse, Windows-ORT-Link, falsches Feature-Frontend):

1. `transcribe-rs` (ebenfalls Joint-Decoder-ONNX, extra `nemo128.onnx`)
2. `sherpa-onnx` Rust-API — **letzter** Ausweg, weil das Modell *anders*
   exportiert ist (`encoder` / `decoder` / `joiner` + `tokens.txt`).
   Spike muss dann Voxtype-Qualität neu belegen, nicht Datei-Identität.

### 6.2 Konfigurierbare Modelle

| Schlüssel | Dateien | Rolle |
|---|---|---|
| `parakeet-tdt-0.6b-v3-int8` | Joint-INT8, ~640 MB | **Default**, 25 Sprachen inkl. Deutsch |
| `parakeet-tdt-0.6b-v3` | unquantisiert | Qualität gegen RAM/Tempo |
| `parakeet-unified-en-0.6b` | English-only | falls jemand nur Englisch will |

Unbekannter Schlüssel: nicht starten, Tray `error`, Log mit den gültigen
Namen.

Sprache: Default `auto`. Optional `language = "de"` in der Config, wenn
die Runtime das durchreicht. Nicht erzwingen — v3 erkennt Deutsch.

Kein Whisper in v1, auch nicht als Fallback.

### 6.3 Download

Beim ersten Start, wenn das gewählte Modell fehlt: Download nach

- Linux: `~/.local/share/diktier/models/<schlüssel>/`
- Windows: `%LOCALAPPDATA%\diktier\models\<schlüssel>\`

Quelle: dieselben Hugging-Face-/Voxtype-Artefakte, die Omarchy nutzt.
Checksum (SHA256) in der Binary oder einer `models.toml` im Repo
festhalten. Fehlschlag → Tray `error`, Retry beim nächsten Start.

Kein stiller Download ohne sichtbares Signal: Tooltip „Lade Modell …“.

### 6.4 Audioformat fürs Modell

16 kHz, mono, f32, Peak grob in [-1, 1]. cpal-Device darf 44.1/48 kHz
liefern; resample im Audio-Pfad (z. B. `rubato` oder Linear für v1, wenn
die Qualität im Spike hält).

Zu kurze Buffer (`< 250 ms`) nicht transkribieren.

## 7. Text am Cursor

Härtester Teil. Default ist **nicht** Zeichen-für-Zeichen-Tippen.

### 7.1 Default: Clipboard + Paste

1. Aktuellen Clipboard-Inhalt merken.
2. Transkript setzen.
3. Paste: Windows `Ctrl+V`; in bekannten Terminals `Ctrl+Shift+V`.
   Linux X11 analog (`xclip`/`xsel` + `xdotool key`).
4. Nach kurzem Delay (Default 200 ms, konfigurierbar) Clipboard
   wiederherstellen.

Wenn Paste scheitert: Transkript **im Clipboard lassen**, Tray `error`
„Text liegt in der Zwischenablage“, nicht still verwerfen.

### 7.2 Optional: `output.mode = "type"`

Echtes Tippen (`enigo` oder Plattform-API) nur als Config-Option. Unter
deutschem Layout (Umlaute, ß, tote Tasten) ist das der bekannte
Fehlerpfad. v1 muss das nicht perfekt können; der Spike darf es
versuchen und verwerfen.

### 7.3 Inhalt

- Transkript so einfügen, wie das Modell es liefert (Satzzeichen, soweit
  Parakeet sie setzt).
- Kein automatisches Anhängen eines Leerzeichens vor dem Text in v1 —
  lieber ein führendes Leerzeichen als Config (`output.leading_space`,
  Default `true`), weil man meist in laufenden Satz diktiert.
- Keine Spoken-Punctuation-Engine in v1.
- Keine Wort-Ersetzungstabelle in v1 (v2; Ersatz für Whisper-Prompt).

## 8. Config

Pfad:

- Linux: `~/.config/diktier/config.toml`
- Windows: `%APPDATA%\diktier\config.toml`

Fehlt die Datei: Defaults schreiben und mit denen starten.

```toml
[hotkey]
key = "F9"
modifiers = []          # z. B. ["ctrl", "alt"]
mode = "push_to_talk"   # v1 nur dieser Wert

[audio]
device = "default"
sample_rate = 16000
max_duration_secs = 60

[engine]
model = "parakeet-tdt-0.6b-v3-int8"
language = "auto"       # oder "de"
threads = 0             # 0 = Runtime-Default

[output]
mode = "paste"          # "paste" | "type"
leading_space = true
restore_clipboard = true
restore_clipboard_delay_ms = 200

[tray]
show_notifications_on_error = true
```

Ungültige Werte: Default + Log-Warnung, nicht abstürzen.

Hot-Reload in v1 nicht nötig. Änderung greift nach Neustart. Tray-Menü
„Config-Ordner öffnen“ reicht.

## 9. Autostart

Opt-in per CLI, analog WhisperDictate:

```
diktier --install-autostart
diktier --remove-autostart
```

- Windows: Verknüpfung im Startup-Ordner des Users (kein Admin).
- Linux: `~/.config/autostart/diktier.desktop`.

Ohne Flag startet `diktier` den Daemon im Vordergrund (Terminal) bzw.
als Session-App ohne Konsole, wenn vom .desktop/Autostart gestartet.
`--foreground` erzwingt Log auf stderr.

## 10. Fehler und Logs

Log nach stderr und zusätzlich:

- Linux: `~/.local/state/diktier/diktier.log`
- Windows: `%LOCALAPPDATA%\diktier\diktier.log`

Rotation: eine Datei, kappen bei 2 MB (einfaches Truncate, kein Log-Stack
in v1).

Keine personenbezogenen Transkripte loggen. Audio nie auf Disk, außer
explizitem Debug-Flag (`DIKTIER_DEBUG_WAV=1` schreibt eine WAV nach
Temp, Dokumentation in der Spec reicht, kein GUI).

## 11. Verteilung

- Ein Binary. Cargo-Lock committen (Anwendung, nicht Library).
- Windows: ZIP mit `diktier.exe` + `onnxruntime.dll` (Version pinnen).
  ORT darf nicht „irgendwo im PATH“ erwartet werden.
- Linux Mint: Binary; `onnxruntime` entweder statisch oder `.so` neben
  der Binary / über Paket. Spike entscheidet, was auf Mint 22 ohne
  Handstand läuft.
- Kein Installer-Zwang. Portable Start aus Ordner muss gehen.
- `cargo build --release` ist der Dev-Weg.

## 12. Implementierungsreihenfolge

Nicht GUI zuerst. Jede Phase hat ein Gate. Nächste Phase erst, wenn das
Gate hält.

### Phase 0 — Repo-Gerüst

`Cargo.toml`, Module-Stubs, Config-Defaults, CLI `--help`. Kein Mic, kein
Tray. Gate: `cargo test` und `cargo build` grün auf Linux.

### Phase 1 — STT-Spike (entscheidend)

Eine feste deutsche WAV (Alltagssatz + ein paar Fachwörter, 5–15 s), ins
Repo unter `testdata/` (kurz, lizenzfrei, selbst gesprochen).

Dieselbe Datei:

1. hier auf Omarchy durch Voxtype/Parakeet
2. durch Diktier-Engine (`parakeet-rs`) auf Linux
3. durch Diktier-Engine auf Windows

Gate:

- Text in derselben Liga wie Voxtype (keine erfundenen Sätze, keine
  Prompt-Halluzination, Fachwörter nicht schlechter als Voxtype).
- Stille-WAV → leeres Transkript.
- Laufzeit auf CPU akzeptabel: für 10 s Audio deutlich unter 10 s Wall
  auf einem aktuellen Büro-Laptop; auf schwächerer CPU (Haswell-Klasse)
  unter ~realtime × 2.
- Windows-ORT lädt ohne Entwickler-Maschine-PATH-Hack.

Scheitert `parakeet-rs`: Fallback-Crate, nicht an der App weiterbauen.

### Phase 2 — Inject-Spike

PTT auf **beiden** Zielplattformen, ohne Tray:

- Fokus bleibt im Editor (Notepad / xed / VS Code / ein Terminal).
- Satz mit Umlauten und ß landet korrekt.
- Neue Zeile im Transkript wird zur echten Zeile.
- Clipboard ist nach 1 s wieder der alte Inhalt (wenn Restore an).
- Terminal: Paste kommt an, nicht als Literal `^V`.

Gate: manuell, aber als Checkliste in `docs/SPIKES.md` abhaken. Scheitert
Tippen, bleibt Paste — und Paste **muss** halten, sonst ist v1 tot.

### Phase 3 — Daemon + Tray + Config + Autostart

State-Machine, Icon-Zustände, Single-Instance, Modell-Download, Autostart
CLI. Gate: kalter Start, F9 PTT, Text im Editor, Beenden über Tray, zweiter
Start wird abgewiesen.

### Phase 4 — Politur

Autostart getestet, ZIP/Binary-Layout, README-Install, Log-Kappen,
`--install-autostart` auf beiden OS.

## 13. Tests

Automatisch:

- Config parsen: Defaults, Overlay, unbekannter Key, ungültiges Modell.
- State-Machine: idle→recording→transcribing→idle; Release ohne Press
  ignorieren; 60 s Cap; leeres Transkript fügt nichts ein.
- Engine: Stille → leer ( testdata). Wo CI kein ORT hat, Test
  `#[ignore]` und lokal/Spike-Pflicht.
- Inject: Unit-Tests nur für Clipboard-Restore-Logik mit Fake, nicht für
  echte SendInput.

Manuell (Gate von Phase 2 und 3): Checkliste Windows + Mint.

Kein GUI-Snapshot, kein Benchmark-Zwang über das Spike-Gate hinaus.

## 14. v2 (nicht bauen, nur nicht verbauen)

- Zweiter Hotkey öffnet einen Review-Dialog (editieren, dann einfügen
  oder kopieren). Darf Fokus nehmen. PTT-Pfad bleibt fokusfrei.
- Wort-Ersetzungen (`replacements` in der Config).
- Optional OSD.
- Optional `language = "de"`-Feintuning / `parakeet-primeline`.
- HTTP `/transcribe` fürs Cockpit, falls VRAM-Sharing je wieder Thema
  wird — bei CPU-INT8 unwahrscheinlich.

Die Module `engine` / `inject` / `hotkey` müssen das zulassen, ohne
umgeworfen zu werden. Kein Preview-Code in v1 hinter Feature-Flags.

## 15. Abgrenzung WhisperDictate

| | WhisperDictate | Diktier v1 |
|---|---|---|
| Sprache | Python | Rust |
| Engine | faster-whisper medium | Parakeet TDT v3 INT8 |
| UX | Toggle, Dialog, Clipboard | PTT, Tray, Paste am Cursor |
| Plattform | Win + Linux, GUI-first | Win + Mint, Daemon-first |
| Repo | `Whisper-dictate` | `diktier` |

Kein Code-Import. Höchstens die Idee des Autostart-Flags und der
Config-Pfade.

## 16. Offene Punkte, die die Spec bewusst festlegt

- Name `diktier` ist Arbeitstitel; Rename bleibt möglich.
- Paste statt Type als Default: ja.
- `F9` als Default-PTT: ja.
- Preview-Dialog: nicht in v1.
- Omarchy bleibt bei Voxtype; Diktier wird dort nicht der Default.
