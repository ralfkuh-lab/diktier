# Diktier — Spec v1.4

Stand: 2026-08-31. Verbindlich für die Implementierung. Änderungen nur über
diesen Text.

v1.1: Codex-Review (`docs/reviews/spec-codex.md`). v1.2: Agy-Kreuz-Review
(`docs/reviews/spec-agy.md`). v1.3: Claude-Review
(`docs/reviews/spec-claude.md`). Entscheidungen in §17 und §18.
v1.4 (2026-08-31, Ralf): Aufnahme-Overlay aus v2 vorgezogen — §2-Eintrag
gestrichen, neu §4.5 und `[overlay]` in §8; Plan
`docs/overlay-plan.md`, Review `docs/reviews/plan-overlay-sol.md`.

## 1. Ziel

Ein lokales Push-to-Talk-Diktiertool für **Windows 10 22H2+ x64** und
**Linux Mint 22.x, Cinnamon, X11, x86_64**. Der Nutzer hält eine Taste,
spricht, lässt los. Nach der Transkription steht der Text im gerade
fokussierten Eingabefeld — aber nur, wenn dasselbe Fenster noch vorn ist
(§7.3).

Qualitätziel: dieselbe Erkennung wie Voxtype auf Omarchy mit den
byte-identischen Artefakten `parakeet-tdt-0.6b-v3-int8` (§6.3, Gate §12).
Diktier ersetzt WhisperDictate auf den Nicht-Omarchy-Rechnern.
WhisperDictate bleibt unangetastet.

v1 ist bewusst schmal: Daemon, PTT, ein Parakeet-Modell, Einfügen am
Cursor, Tray, Config, Autostart. Kein Preview-Dialog.

## 2. Nicht-Ziele (v1)

- Preview-/Korrektur-Dialog (v2, zweiter Hotkey)
- Whisper / faster-whisper
- Streaming (Nemotron)
- HTTP-API / Local-AI Cockpit
- macOS
- Omarchy/Hyprland als Zielplattform (dort bleibt Voxtype)
- Cinnamon **Wayland** (Programm startet mit verständlichem Fehler, ohne
  Hotkey; Tray darf Quit anbieten)
- Meeting-Transkription, Diarization
- ~~OSD/Waveform-Overlay~~ (v1.4: vorgezogen, siehe §4.5)
- Cloud, Telemetrie
- Whisper-`initial_prompt`, Wortersetzungen
- Tastensimulation als Default-Ausgabe
- Inject in **erhöhte** Windows-Prozesse (UAC/UIPI)
- Verlustfreies Clipboard-Restore für Nicht-Text (Bilder, HTML, Dateien)
- Weitere Parakeet-Varianten (unquantisiert, English-only) — erst nach
  eigenem Gate

## 3. Nutzer und Plattformen

| Plattform | Session | Audio | Hotkey | Ausgabe |
|---|---|---|---|---|
| Windows 10 22H2 x64, Windows 11 x64 | Desktop, nicht erhöht | WASAPI via cpal | `WH_KEYBOARD_LL` auf **eigenem** Message-Pump-Thread (§5) | Clipboard + Paste |
| Linux Mint 22.x Cinnamon **X11** x86_64 | typischer Alltag | Pulse/PipeWire via gepinnte cpal-Features | zuerst `global-hotkey`; Fallback `x11rb` + `XGrabKey` | Clipboard + Paste |

Kein Admin/root für den Normalbetrieb. Kein evdev, keine Gruppe `input`
in v1 (das wäre der Wayland/uinput-Pfad).

Kein CUDA. v1 ist CPU/INT8. Designpunkt für Tempo: Haswell-Klasse
(i7-4500U, AVX2) als langsames Gate; ein aktueller 4+-Kern-Laptop als
schnelles Gate. Beide Rechner in `docs/SPIKES.md` namentlich eintragen.
Peak-RSS-Ziel: **≤ 2 GiB** mit geladenem Default-Modell.

Linux-Release-Builds entstehen verbindlich auf Mint-22-Basis (VM,
Container oder CI; Ubuntu 24.04, glibc 2.39) — eine neuere Build-glibc
lässt sich nicht „wegtesten“, das Binary fällt auf Mint schlicht um.
Windows: MSVC x64.

## 4. UX

### 4.1 Push-to-Talk

1. Hotkey **drücken und halten** (nur aus `idle`) → Aufnahme startet.
   Tray „recording“. Kein Fenster, kein Fokuswechsel.
2. Sprechen.
3. Hotkey **loslassen** oder 60-s-Cap → Aufnahme stoppt, Transkription
   läuft. Tray „transcribing“.
4. Fertig und Fenster unverändert: Text einfügen. Tray „idle“.
5. Leer / nur Stille / Transkript nach Normalisierung leer: nichts
   einfügen, kein Fehlerdialog, Tray „idle“.

„60-s-Cap“ meint in dieser Spec durchgehend `audio.max_duration_secs`
(Default 60, §8).

### 4.2 Fokusregel (nicht verhandelbar)

Der PTT-Pfad darf **kein** Fenster öffnen, das den Fokus nimmt. Feedback
nur über Tray-Icon, Tooltip und optionale Desktop-Notification bei
Fehlern.

Diktier aktiviert niemals selbst ein Fenster. Zweiter-Instanz-Start
ebenfalls nicht.

Einzige, enge Ausnahme (Windows, Entscheidung 2026-08-27, Phase 5): Nach
einem **expliziten Rechtsklick** des Nutzers auf das Tray-Icon darf das
**unsichtbare** Menü-Owner-Fenster per `SetForegroundWindow` aktiviert
werden — Win32 schließt das Popup-Menü sonst nicht. Danach `WM_NULL` an das
Owner-Fenster und `NIM_SETFOCUS`. Der PTT-/Inject-Pfad durchläuft diesen Weg
nie.

### 4.3 Tray

Crate: `betrayer`, Version in `Cargo.lock` pinnen. Fallback hinter
`TrayBackend`: Windows `Shell_NotifyIconW`, Linux `ksni`. `tray-icon`
nur, wenn SNI unter Cinnamon nachweislich scheitert und GTK-Abhängigkeit
bewusst akzeptiert wird.

| Zustand | Bedeutung |
|---|---|
| starting | Prozess hoch, noch nicht bereit |
| downloading | Modellartefakte fehlen, Download läuft |
| loading | ORT + Modell werden geladen |
| idle | geladen, Hotkey scharf (außer Pause) |
| recording | PTT gehalten oder Tray-Toggle-Aufnahme |
| transcribing | Inferenz |
| error | fatal oder bedienbar, siehe §10 |
| paused | Hotkey aus; Tray-Click bleibt aktiv |

Menü (Rechtsklick):

- Statuszeile (nicht klickbar): Zustand + Modellschlüssel
- Hotkey pausieren / wieder aktivieren
- Config-Ordner öffnen
- Beenden

Linksklick: Toggle-Aufnahme, Fallback wenn der Hotkey nicht greift.
Ein Klick startet, nochmal stoppt und transkribiert. `recording` merkt
die Quelle (`Hotkey` vs. `TrayClick`):

- `recording(Hotkey)`: Linksklick ignorieren.
- `recording(TrayClick)`: F9 Press/Release ignorieren (Log). Stop nur
  durch zweiten Klick oder 60-s-Cap.

TrayClick-Diktate enden **immer** in `copy_only` — kein Paste-Key auf
diesem Pfad. Beim Klick aufs Tray-Icon kann Panel/Taskbar den Vordergrund
halten; eine Fokusprüfung gegen das eigentliche Ziel ist nicht
verlässlich. Transkript ins Clipboard, Tooltip „Text liegt in der
Zwischenablage“.

Während `transcribing`/`downloading`/`loading`: beide Eingaben ignorieren
(Log-Warnung).

### 4.4 Default-Hotkey

`F9`, ohne Modifier, Push-to-Talk. Nur über Config änderbar in v1.
Registrierungsfehler → Zustand `error` (Hotkey tot), Tray-Click bleibt
aktiv. Tooltip nennt den Konflikt.

Der PTT-Hotkey erreicht die fokussierte Anwendung **nie** (X11:
`XGrabKey` schluckt implizit; Windows: der `WH_KEYBOARD_LL`-Hook gibt
für den PTT-Key non-zero zurück). Sonst togglet jedes Diktat in VS Code
einen Breakpoint.

Auto-Repeat der Haltetaste wird entprellt: ein logisches Press, ein
logisches Release.

Verlorenes Release (z. B. Desktop gesperrt): nach dem 60-s-Cap genau
einmal transkribieren; ein späteres Release ignorieren.

### 4.5 Aufnahme-Overlay (Windows, v1.4)

Während `recording`, `transcribing` und `injecting` zeigt ein kleines,
randloses Layered Window unten mittig (Monitor des fokussierten
Fensters) den Mikrofonpegel: scrollende Waveform-Historie plus
Pegelmeter mit Peak-Hold, Optik nach Omarchy-Voxtype-OSD. Bei `idle`,
`error` und in allen Abbruchpfaden (Pause-Discard, Quit) verschwindet
es.

Die Fokusregel §4.2 gilt uneingeschränkt: `WS_EX_NOACTIVATE`,
`SW_SHOWNOACTIVATE`, keine Fokus-APIs, durchklickbar
(`WS_EX_TRANSPARENT` + `WM_NCHITTEST → HTTRANSPARENT`). Ein
Overlay-Fehler (Fensterbau, Rendering) deaktiviert nur das Overlay
(Log-Warnung); Diktieren läuft weiter. Abschaltbar über
`[overlay] enabled` (§8). Windows-only; Details und Verträge:
`docs/overlay-plan.md`.

## 5. Architektur

Ein Anwendungsprozess, ausführbare Datei `diktier` **plus** gebündelte
ONNX-Runtime-Library (§11) **plus** heruntergeladenes Modell. Kein
Python, kein Sidecar.

```
diktier
  ├── tray        betrayer, UI-/Event-Thread
  ├── hotkey      HotkeyBackend: press/release
  ├── audio       AudioSource: native Rate → f32 → 16 kHz
  ├── engine      Trait Transcriber (parakeet-rs)
  ├── inject      OutputSink: paste | copy_only
  └── config      TOML, kein Hot-Reload in v1
```

Modell resident (`on_demand_loading = false`). Kaltstart darf Sekunden
brauchen. **Audio-Callback darf niemals auf Modellladen oder Inferenz
warten.** Aufnahme aus `idle` startet sofort. Aus `loading`/`downloading`
startet keine Aufnahme (Press ignorieren, Log).

Worker-Thread für Inferenz, cpal-Callback, Tray-Eventloop. Inferenz darf
den Tray-Thread nicht blockieren. Keine Pflicht-Runtime; `tokio` ist
erlaubt.

Windows-Hotkey: eigener Thread mit minimaler `GetMessageW`-Loop, der
**nur** den `WH_KEYBOARD_LL`-Hook hält, Down/Up entprellt und Events über
einen Channel an die State-Machine schickt. Nicht auf dem `betrayer`-
Thread — sonst hängt Windows den Hook bei offenem Tray-Menü aus
(`LowLevelHooksTimeout`).

### 5.1 Module

```
src/main.rs
src/config.rs
src/state.rs
src/audio.rs
src/engine.rs         Transcriber
src/hotkey.rs         cfg(windows) / cfg(unix)
src/inject.rs         OutputSink
src/tray.rs
src/download.rs
src/models.toml       Artefakt-Manifest, im Repo
```

Verträge, die v2 nicht umwerfen (ohne v2-Code):

```text
Transcription { text, language?, timing? }
CaptureContext { start_window_id, target_window_id, ended_at }
OutputSink: paste | copy_only   // v2 ergänzt review
```

v1 verdrahtet nur `paste` und bei Fokusverlust `copy_only`. Engine kennt
kein Inject, Inject kein Audio.

### 5.2 State-Machine

Orthogonales Flag `paused` (Hotkey aus). Sonst:

```
starting → downloading? → loading → idle
idle + Press                → recording(Hotkey)
idle + ClickStart           → recording(TrayClick)
recording(Hotkey) + Release|Cap     → transcribing
recording(TrayClick) + ClickStop|Cap → transcribing
transcribing(Hotkey) + Text + Fokus gleich → inject → idle
transcribing(TrayClick) + Text             → copy_only → idle
transcribing + leer                → idle
transcribing + Fokus ungleich      → copy_only → idle
jeder Zustand + fatal              → error
error + Retry/Neustart             → starting
```

Regeln:

- Press außerhalb `idle` (und nicht `paused`): ignorieren, Log.
- Pause während `recording`: Aufnahme verwerfen, **keine** Transkription,
  zurück nach `idle` mit `paused=true`.
- Beenden während Inferenz: Prozess darf nach Inferenz-Timeout (5 s)
  hart enden; kein Inject mehr.
- 60-s-Cap: genau einmal nach `transcribing`; folgendes Release ignorieren.
- Watchdog in `transcribing`: kein Engine-Ergebnis nach
  max(30 s, 5 × Audiolänge) → Lauf verwerfen, Engine neu initialisieren,
  `error` (Tooltip „Transkription hängt“); Retry beim nächsten Press.
  Ein verspätetes Ergebnis eines verworfenen Laufs wird nie injiziert.
- `idle` heißt: Modell geladen, bereit. Das widerspricht nicht dem
  Audio-Callback-Verbot — Aufnahme gibt es erst ab `idle`.

### 5.3 Single-Instance

Kein PID-File.

- Windows: Named Mutex **pro interaktiver Session** (`Local\`-Namensraum;
  präzisiert 2026-08-27 — Tray und Hotkey sind ohnehin sessiongebunden,
  eine zweite RDP-/Fast-User-Switching-Session bekommt ihre eigene Instanz).
- Linux: gehaltener advisory `flock` unter `$XDG_RUNTIME_DIR/diktier.lock`,
  Fallback `$XDG_STATE_HOME/diktier/diktier.lock`. Liegengebliebene Datei
  ist egal, allein der Lock zählt.

Zweiter **Daemon**-Start: optionale lokale Notify-Nachricht an den ersten
Prozess (Tray-Tooltip „läuft bereits“), **kein** Fensterfokus, Exitcode 0,
kurze stderr-Meldung. Kein fremder Prozess wird beendet.

`--help`, `--version`, `--install-autostart`, `--remove-autostart` laufen
**vor** der Sperre und fordern weder Mutex noch `flock`. Nur
`diktier` / `diktier --foreground` (Daemon) nimmt die Sperre.

## 6. Engine und Modelle

### 6.1 Runtime

Erste Wahl: `parakeet-rs` + ONNX Runtime CPU, Feature `load-dynamic`.
Vor jeder ORT-Nutzung: `ort::init_from(<absolute path relativ zu current_exe()>)`.
`parakeet-rs`, `ort`/`ort-sys` und das ORT-Binary in Phase 1 **exakt**
pinnen (`Cargo.lock` + Manifest).

Den TDT-Decode-Loop nicht selbst schreiben.

`parakeet-rs` gilt als gescheitert, wenn auf einer Pflichtplattform
Laden, Qualitätsfälle, Stille oder Zeitlimit fehlschlagen. Dann darf
**eine** gepinnte `transcribe-rs`-Version durch **dasselbe** Gate — aber
erst nachdem `nemo128.onnx` (URL, Bytes, SHA-256) in `models.toml`
nachgetragen ist. Das Golden Set in §6.3 bleibt Voxtype-identisch und
enthält diese Datei bewusst **nicht**. Nur wenn auch `transcribe-rs`
scheitert: `sherpa-onnx` mit eigenem Artefaktsatz und identischem
Qualitätsgate. Phase 1 startet nur mit `parakeet-rs`.

### 6.2 Freigegebenes Modell (v1)

Nur ein Schlüssel:

| Schlüssel | Rolle |
|---|---|
| `parakeet-tdt-0.6b-v3-int8` | Default und einziges v1-Modell, 25 Sprachen, Auto-Detect |

Unbekannter Schlüssel: fataler Configfehler, Tray `error`, kein
Default-Fallback, kein Hotkey.

`language` gibt es in v1 **nicht** in der Config. TDT läuft immer auf
Auto-Detect. (Eine spätere `language = "de"`-Option braucht nachgewiesen
wirksames API.)

### 6.3 Artefakte — Golden Set

Byte-identisch zu Voxtype auf dem Omarchy-Rechner, Stand 2026-08-26,
Verzeichnis `~/.local/share/voxtype/models/parakeet-tdt-0.6b-v3-int8/`.

| Datei | Bytes | SHA-256 |
|---|---:|---|
| `encoder-model.int8.onnx` | 652183999 | `6139d2fa7e1b086097b277c7149725edbab89cc7c7ae64b23c741be4055aff09` |
| `decoder_joint-model.int8.onnx` | 18202004 | `eea7483ee3d1a30375daedc8ed83e3960c91b098812127a0d99d1c8977667a70` |
| `vocab.txt` | 93939 | `d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d` |
| `config.json` | 97 | `666903c76b9798caf2c210afd4f6cd60b08a8dbf9800ec8d7a3bc0d2148ac466` |

`models.toml` im Repo hält dieselben Werte plus **immutable** Download-URLs
(`resolve/<git-commit>/…` auf Hugging Face, sobald die Herkunfts-Revision
im STT-Spike dokumentiert ist). Bis dahin läuft Phase 1 mit einer **Kopie
des Omarchy-Golden-Sets** — die Hashes aus der Tabelle müssen stimmen.
Lizenz/Notice der Artefakte (NVIDIA
Parakeet, CC-BY-4.0) in README und `LICENSES/`.

Installationsort:

- Linux: `~/.local/share/diktier/models/parakeet-tdt-0.6b-v3-int8/`
- Windows: `%LOCALAPPDATA%\diktier\models\parakeet-tdt-0.6b-v3-int8\`

Download je Datei nach `<name>.part`, Größe + SHA-256 prüfen, dann atomar
umbenennen. Zuletzt Marker `COMPLETE` schreiben. Per-user Download-Lock
gegen parallele Starts. Hashfehler: nur `.part` löschen, Tray `error`,
Retry erst nach explizitem Neustart/Retry. Tooltip währenddessen
„Lade Modell …“.

### 6.4 Audio

`audio.sample_rate` ist die **Engine-Zielrate**, in v1 nur `16000`.
Das Gerät wird in einer nativ unterstützten Rate/Sampledarstellung
geöffnet (typisch 44,1/48 kHz, intern Stereo möglich).

Der `cpal`-Callback ist lock-free und allokationsfrei: er schiebt nur
rohe Frames in einen SPSC-Ringpuffer. Kanal-Mittelung, f32-Skalierung
und `rubato`-Resample (16 kHz, Flush beim Stop) laufen **ausschließlich**
auf dem Audio-/Transkriptions-Worker. Linear-Resampler ist kein Fallback.

Gerät verloren: beim nächsten Aufnahmeversuch einmal neu öffnen; bleibt
es tot → `error` Mic, Retry beim nächsten Press.

Zu kurze Buffer (`< 250 ms`) nicht transkribieren.

Phase-1-STT-Spike mit fertiger WAV prüft **nicht** den Capture-Pfad;
das ist Pflicht in Phase 2 (echte 44,1- und 48-kHz-Geräte bzw. Fixtures,
Stereo, Device-lost).

## 7. Text am Cursor

Default ist **nicht** Zeichen-für-Zeichen-Tippen.

### 7.1 Default: Clipboard + Paste

Nur Unicode-Plaintext ist der v1-Vertrag.

1. Wenn der aktuelle Clipboard-Inhalt als Unicode-Text snapshotbar ist:
   merken (Text + Windows-Sequenznummer bzw. X11-Ownership).
2. Sonst: kein Restore-Versprechen; nach Paste bleibt das Transkript im
   Clipboard, Tooltip „Nicht-Text-Clipboard konnte nicht restauriert
   werden“.
3. Transkript setzen. Diktier merkt die **eigene** Generation/Ownership.
4. Paste-Shortcut senden (§7.2).
5. Restore **nur**, wenn Diktier noch Owner ist bzw. die Windows-
   Sequenznummer unverändert blieb. Fremde Änderung: niemals restaurieren.
6. `restore_clipboard_delay_ms` ist eine **Mindestwartezeit**, keine
   Erfolgserkennung. **Kein** `sleep` auf dem Thread, der die X11-
   Connection hält: `SelectionRequest` muss während der Wartezeit
   beantwortet werden (Timer in der State-Machine).
7. Restore **nur nach mindestens einem bedienten Read** des Transkripts
   — X11: `SelectionRequest`; Windows: Delayed Rendering
   (`SetClipboardData(CF_UNICODETEXT, NULL)` → `WM_RENDERFORMAT`) — und
   frühestens nach der Mindestwartezeit. Kommt innerhalb von 5 s **kein**
   Read (UIPI, verschluckter Chord, falscher Shortcut — am
   API-Rückgabewert oft nicht erkennbar), unterbleibt das Restore
   endgültig: Transkript bleibt im Clipboard, Tooltip „Einfügen nicht
   bestätigt — Text liegt in der Zwischenablage“. Clipboard-Manager und
   Win+V-History erzeugen ggf. False-Positive-Reads; akzeptiert. Ein zu
   Unrecht unterbliebenes Restore ist der akzeptierte Preis — ein
   weggewischtes Transkript nicht.
8. Nach dem Restore bedient Diktier die restaurierte X11-Selection bis
   zum Ownership-Verlust weiter. Bei Prozessende verschwindet sie —
   X11-Natur, ein Clipboard-Manager wird nicht vorausgesetzt.

Vor dem Paste-Shortcut: störende Modifier (`Shift`, `Alt`, `Super`/`Win`)
per Up-Event lösen — sonst wird `Ctrl+V` bei gehaltenem Shift zu
`Ctrl+Shift+V`. Wiederhergestellt (synthetisches Down) wird ein Modifier
**nur**, wenn die Taste zum Restore-Zeitpunkt physisch noch gehalten ist
(`GetAsyncKeyState` / `XQueryKeymap`); sonst unterbleibt das Restore. Ein
hängender synthetischer Modifier wäre schlimmer als ein einmalig
verlorener — Nicht-Restaurieren heilt sich mit dem nächsten physischen
Tastendruck selbst.

Paste-API-Fehler oder UIPI: Transkript **im Clipboard lassen**, Tray
`error` „Text liegt in der Zwischenablage“. Kein stilles Verwerfen.
Diktier fordert keine Elevation an.

### 7.2 Paste-Shortcut

Config: `output.paste_shortcut = "auto" | "ctrl_v" | "ctrl_shift_v" | "shift_insert"`.

`auto`:

- Windows: `Ctrl+Shift+V` nur bei `WindowsTerminal.exe` (bindet beide
  Chords auf Paste). `conhost` kennt `Ctrl+Shift+V` nicht → `Ctrl+V`
  (Console-Setting bzw. PSReadLine). Sonst `Ctrl+V`.
- Linux X11: `Ctrl+Shift+V` bei VTE/Freedesktop-Terminals (`gnome-terminal`,
  `xfce4-terminal`, Tilix, Alacritty, Kitty, Ghostty); `Shift+Insert` bei
  `xterm`/`uxterm` und generischen X11-Terminals; sonst `Ctrl+V` in
  normalen GUI-Fenstern.

Unbekanntes Ziel: `Ctrl+V`. Scheitert Auto-Erkennung im Spike auf dem
Pflichtterminal: Override in der Config, nicht Type-Modus.

### 7.3 Fokus bei Inject

Beim **Aufnahmestart** (Press) und beim **Aufnahmeende** (Release oder
Cap) die native Vordergrund-Kennung speichern
(`CaptureContext.start_window_id` / `target_window_id`). Das ist das
**Top-Level**-Fenster: Windows `HWND` via `GetForegroundWindow()`, X11
`Window` via `_NET_ACTIVE_WINDOW`. Fokuswechsel in Child-Controls oder
Tabs desselben Top-Level-Fensters zählt **nicht** als Verlust (sonst
scheitert VS Code).

Inject nur, wenn Start-Kennung, Ende-Kennung und Vordergrund vor Inject
**übereinstimmen**. Sonst **kein** Paste-Key, Transkript bleibt im
Clipboard, Tooltip „Fokus geändert — Text liegt im Clipboard“. Eine nicht
ermittelbare Kennung (NULL, Secure Desktop, gesperrter Bildschirm) zählt
als Fokusverlust — das verhindert auch den Paste in den Unlock-Dialog
eines X11-Lockers nach dem 60-s-Cap bei gesperrtem Desktop.

### 7.4 Inhalt

- Modelltext unverändert (Satzzeichen, soweit Parakeet sie setzt).
- `output.leading_space` Default `true` (Diktat in laufenden Satz).
  Bekannter Artefakt: in leeren Feldern ein führendes Leerzeichen; in
  Shells mit `HISTCONTROL=ignorespace` fällt der Befehl aus der History.
  Wen das stört: `leading_space = false`.
- Keine Spoken-Punctuation, keine Replacements in v1.

### 7.5 Optional `output.mode = "type"`

Nur Config-Option, ohne v1-Garantie. Spike darf es versuchen. Scheitert
es, bleibt Paste der Release-Pfad.

## 8. Config

- Linux: `~/.config/diktier/config.toml`
- Windows: `%APPDATA%\diktier\config.toml`

Fehlt die Datei: Defaults **atomar** schreiben (Temp + Rename) und starten.

```toml
[hotkey]
key = "F9"
modifiers = []
mode = "push_to_talk"   # v1 nur dieser Wert

[audio]
device = "default"
sample_rate = 16000     # Engine-Zielrate, nur 16000
max_duration_secs = 60

[engine]
model = "parakeet-tdt-0.6b-v3-int8"
threads = 0             # 0 = Runtime-Default

[output]
mode = "paste"          # "paste" | "type"
paste_shortcut = "auto"
leading_space = true
restore_clipboard = true
restore_clipboard_delay_ms = 200

[tray]
show_notifications_on_error = true

[overlay]
enabled = true          # Aufnahme-Overlay (§4.5), Windows-only
```

Validierung:

| Klasse | Beispiele | Wirkung |
|---|---|---|
| Fatal | TOML-Syntax, ungültiges `hotkey.key`, `output.mode`, `engine.model` | kein Hotkey, keine Aufnahme, Tray `error` |
| Unbekannte Keys | Tippfehler in Schlüsselnamen | ignorieren + Warnung |
| Clamped | Zahlen außerhalb | Warnung + Grenze |

Grenzen: `max_duration_secs` 1..=60, `restore_clipboard_delay_ms` 0..=5000,
`threads` 0..=(logische CPUs).

Kein Hot-Reload. Tray „Config-Ordner öffnen“.

## 9. Autostart und CLI

```
diktier                     # Daemon
diktier --foreground        # Logs auf stderr, auch mit Konsole
diktier --install-autostart
diktier --remove-autostart
```

Install/Remove **idempotent**. Pfad = gequotetes `current_exe()`. Eigenen
Eintrag aktualisieren, fremde Einträge nie löschen.

- Windows: Startup-Ordner des Users. Build: Windows-Subsystem (kein
  Konsolenfenster beim Doppelklick); `--foreground` hängt eine Konsole an
  bzw. schreibt trotzdem stderr, wenn eine da ist.
- Linux: `~/.config/autostart/diktier.desktop`.

Exitcodes: `0` ok (auch zweiter Start), `1` fataler Laufzeitfehler,
`2` Bedien-/Configfehler.

Phase 4: Pfad mit Leerzeichen, zweimal Install/Remove, verschobene
portable Binary (erneutes `--install-autostart` aktualisiert).

## 10. Fehler, Recovery, Logs

| Klasse | Hotkey | Retry | Text/Audio |
|---|---|---|---|
| Download/ORT/Modell fatal | aus | Neustart oder expliziter Retry | kein Inject |
| Hotkey-Registrierung | aus | Config ändern + Neustart | Tray-Click aktiv |
| Mic tot | an | nächster Press öffnet Device neu, einmal | Aufnahme startet nicht |
| Inject/UIPI/Fokus | an | nächstes Diktat | Transkript bleibt im Clipboard |
| Tray-Aufbau gescheitert | — | — | Prozessende, stderr+Log, Exit 1 |

Desktop-Notification nur Zusatz. Wenn der Tray nicht startet, gibt es
keinen zweiten GUI-Kanal.

Log: stderr (im `--foreground`) plus

- Linux: `~/.local/state/diktier/diktier.log`
- Windows: `%LOCALAPPDATA%\diktier\diktier.log`

Ein Writer besitzt die Datei. CLI-Modi (`--help`, `--version`,
`--install-autostart`, `--remove-autostart`) loggen nur nach stderr, nie
in `diktier.log` — der Daemon kann parallel laufen (Ein-Writer-Regel).
Rotation, nicht In-Place-Truncate: erreicht
`diktier.log` 2 MiB, atomar nach `diktier.log.1` (eine Backup-Datei),
neue `diktier.log`. Keine Transkripte, keine Clipboard-Inhalte, keine
Fenstertitel.

`DIKTIER_DEBUG_WAV=1`: schreibt `$TMPDIR/diktier-$USER/last_recording.wav`
bzw. `%TEMP%\diktier\last_recording.wav`, Rechte `0600`. Jede neue Debug-
Aufnahme überschreibt diese Datei atomar — genau ein Dump. Pfad eine
Logzeile. Nie hochladen.

## 11. Verteilung

Bundle, nicht „eine Datei“:

```
diktier[.exe]
lib/onnxruntime.dll          # Windows, fester Name
lib/libonnxruntime.so        # Linux, fester Name, kein Symlink-Zwang
LICENSES/
versions.toml                # App, ORT-ABI, Crate-Lock-Hinweis
```

Release-Skript kopiert die ORT-Library unter genau diesen Dateinamen.
Kein `PATH`, kein `LD_LIBRARY_PATH`, kein System-ORT. Laden ausschließlich
über `ort::init_from` relativ zu `current_exe()`.

Release-Gate: Archiv in ein **leeres** Verzeichnis einer sauberen Win10-,
Win11- und Mint-22-VM entpacken, ORT-Umgebungsvariablen entfernen, STT
laden.

Portable Start aus Ordner muss gehen. `cargo build --release` ist der
Dev-Weg. `Cargo.lock` committen.

ORT-CPU-Instruktionen: die gepinnte ORT-Build-Variante dokumentieren.
Haswell hat AVX2; wenn ORT AVX2 verlangt, ist das die Mindest-CPU.

## 12. Implementierungsreihenfolge

Nicht GUI zuerst. Nächste Phase erst, wenn das Gate in `docs/SPIKES.md`
abgehakt ist.

### Phase 0 — Gerüst

`Cargo.toml`, Module-Stubs, Config-Defaults, CLI `--help`, `models.toml`
mit den Hashes aus §6.3. Gate: `cargo test` und `cargo build` auf Linux.

### Phase 1 — STT-Spike

`testdata/stt/` (selbst gesprochen, lizenzfrei):

- mindestens **drei** deutsche Äußerungen: Alltag, Fachwörter, Zahlen/Umlaute
- jeweils wortgetreuer Referenztext
- eine echte Stille-Datei
- eine Raumrausch-Datei

Voxtype und Diktier verwenden die Artefakte aus §6.3. Normalisierung für
den Vergleich (ein kleines Script in `testdata/`): Kleinbuchstaben,
Interpunktion `[.,!?;:\-–—"']` weg, Whitespace kollabieren. Referenztexte
nutzen dieselbe Zahlenschreibweise wie Parakeet (Ziffern vs. Wort).

- nach Normalisierung: Diktier-Text = Voxtype-Text, oder
  `WER(Diktier, Referenz) ≤ WER(Voxtype, Referenz) + 0,05` (Puffer
  wiederhergestellt, §18 #11: Artefakte byte-identisch, aber die
  Mel-Frontends nicht — Voxtype Kaldi-Style, parakeet-rs NeMo-Style)
- kein Diktier-Ergebnis darf einen nicht gesprochenen Satz enthalten
- markierte Fachwörter mindestens so oft korrekt wie Voxtype
- Stille, Raumrauschen, `< 250 ms` → leer
- Zeit: Median aus fünf **warmen** Läufen, Modellladen separat;
  10 s Audio **≤ 5 s** auf dem benannten Büro-Laptop, **≤ 20 s** auf der
  Haswell-Maschine
- Peak-RSS zusätzlich mit einer 60-s-Datei messen (Ziel ≤ 2 GiB, §3)
- Halluziniert die Engine auf Stille/Rauschen, darf Diktier einen
  dokumentierten RMS-Silence-Gate vorschalten (Schwelle in
  `docs/SPIKES.md`); das gilt **nicht** als Scheitern von `parakeet-rs`,
  solange die Sprach-Gates bestehen

`docs/SPIKES.md` hält CPU, RAM, OS, Crate-/ORT-Version, Threads,
Artefakt-SHA256, Rohtexte, normalisierte Texte, Zeiten.

Windows-ORT lädt aus dem Bundlepfad ohne PATH-Hack.

### Phase 2 — Inject- und Capture-Spike

Pflichtmatrix:

| OS | Editor | Terminal |
|---|---|---|
| Windows 10 x64 | Notepad, VS Code | Windows Terminal / PowerShell |
| Windows 11 x64 | Notepad, VS Code | Windows Terminal / PowerShell |
| Mint 22 Cinnamon X11 | xed, VS Code | Standard-VTE-Terminal (`gnome-terminal` / Mint-Terminal). Nicht xterm — das wäre `Shift+Insert`. |

Pro Fall in `docs/SPIKES.md`:

- Fensterkennung vor/nach Paste identisch
- exakter Text `Grüße, Öl, Spaß — Zeile 1\nZeile 2`
- vorhandener Unicode-Clipboard-Wert nach Restore-Regel wieder da, oder
  dokumentiert nicht restaurierbar (Nicht-Text)
- absichtlicher API-Fehler → Transkript bleibt im Clipboard
- Fokuswechsel während Transkription → kein Paste, Text im Clipboard
- fremder Copy während Restore-Fenster → fremder Inhalt bleibt
- Paste ohne Clipboard-Read (Ziel liest nicht, z. B. erhöhtes Notepad)
  → kein Restore, Transkript bleibt im Clipboard
- PTT-Key erreicht die App nicht: F9 halten in VS Code → kein
  Breakpoint-Toggle, kein Zeichen
- kein `^V`
- 44,1- und 48-kHz-Capture bzw. Fixture, Stereo-Downmix
- PTT Press/Release, 60-s-Cap, Auto-Repeat, Registrierungsfehler
- Windows: erhöhtes Notepad — kein Paste, kein Fokuswechsel, Text im Clipboard

Bestehen = exakte Zeichen- und Zeilengleichheit in jedem Editor-Pflichtfall.

### Phase 2b — Tray-Smoke (vor Phase 3)

Gepinnte `betrayer`-Version, Windows 10/11 und Mint Cinnamon: Links-/
Rechtsklick getrennt, Tooltip/Icon-Update, Pause/Resume, Panel-Neustart,
Quit, kein Fokuswechsel.

### Phase 3 — Daemon

State-Machine, Single-Instance, Download, Autostart-CLI. Gate: kalter
Start, F9 PTT, Text im Editor, Beenden über Tray, Parallelstart Exit 0,
Neustart nach Kill des ersten Prozesses.

### Phase 4 — Politur

Autostart, Bundle-Layout, README-Install, Log-Kappen, saubere VM ohne ORT
im PATH.

## 13. Tests

Automatisch, mit Fake-Backends:

- Config: die drei Klassen aus §8; atomare Defaults-Datei.
- State: alle Übergänge in §5.2 inklusive Press während transcribing,
  Auto-Repeat, Pause während recording, Cap + spätes Release, Download-/
  Hashfehler, Injectfehler, Fokusverlust.
- Download: lokaler Fake-Transport — Abbruch, falsche Größe, falscher
  Hash, atomarer Abschluss, Parallelstart.
- Clipboard-Fake: Generationen/Ownership, Reads, Fremdänderung,
  Nicht-Text; kein Read → kein Restore.
- State zusätzlich: Transcribing-Watchdog, TrayClick → `copy_only`.
- Engine: Stille → leer, soweit ohne ORT mockbar.

`stt-smoke` mit echtem Modell: `#[ignore]` im normalen `cargo test`,
Pflicht in Phase 1 auf beiden OS.

Kein GUI-Snapshot.

## 14. v2 (nicht bauen, nicht verbauen)

- Zweiter Hotkey → Review-Dialog (darf Fokus nehmen). PTT bleibt fokusfrei.
- `OutputSink::review`.
- Wortersetzungen.
- `parakeet-primeline`, weitere Modelle nach eigenem Gate. (OSD: seit
  v1.4 in v1, §4.5.)
- HTTP ist Nicht-Ziel und wird nicht vorbereitet.

## 15. Abgrenzung WhisperDictate

| | WhisperDictate | Diktier v1 |
|---|---|---|
| Sprache | Python | Rust |
| Engine | faster-whisper medium | Parakeet TDT v3 INT8 |
| UX | Toggle, Dialog, Clipboard | PTT, Tray, Paste am Cursor |
| Plattform | Win + Linux, GUI-first | Win + Mint X11, Daemon-first |
| Repo | `Whisper-dictate` | `diktier` |

Kein Code-Import.

## 16. Offene Punkte, die die Spec festlegt

- Name `diktier` ist Arbeitstitel.
- Paste statt Type als Default.
- `F9` als Default-PTT.
- Preview-Dialog nicht in v1.
- Wayland nicht in v1.
- Nur ein Modell in v1.
- Omarchy bleibt bei Voxtype.

## 17. Entscheidungen zum Codex-Review

| # | Frage | Entscheidung |
|---|---|---|
| 1 | Golden Set | Die vier Hashes in §6.3, von diesem Omarchy-Voxtype-Stand. HF-Revision im Spike nachtragen, sobald die URL feststeht. |
| 2 | Rechner / RAM | Haswell = i7-4500U (dieses Omarchy-Gerät) als langsames Zeit-Gate. Büro-Laptop in SPIKES.md namentlich. Peak-RSS ≤ 2 GiB. |
| 3 | WAV-Menge | Mindestens drei Sprach-WAVs plus Stille und Rauschen. |
| 4 | `language` | In v1 aus der Config entfernen. |
| 5 | Weitere Modelle | Aus v1-Liste. |
| 6 | Wayland | Außerhalb des Supportvertrags. |
| 7 | Fokuswechsel | Abbruch des Paste, Text bleibt im Clipboard. |
| 8 | Clipboard-Restore | Nur Unicode-Plaintext. |
| 9 | Terminal-Shortcut | `paste_shortcut` mit `auto` + Override. |
| 10 | Elevated Windows | Nicht unterstützt. |
| 11 | ORT | Immer private Library neben der Binary, nie Systempaket. |
| 12 | Toolchain | Windows MSVC x64; Linux glibc 2.39 (Mint 22). |
| 13 | CLI vs. Lock | Autostart/help/version **vor** Single-Instance (Agy B1). |
| 14 | `nemo128.onnx` | Erst spezifizieren, wenn `parakeet-rs` in Phase 1 fällt (Agy B2). |
| 15 | Win-Hook-Thread | Eigene Message-Pump, nicht Tray-Thread (Agy H1). |
| 16 | X11-Clipboard | Kein Sleep auf der X11-Connection (Agy H2). |
| 17 | Audio-Callback | Nur Ringpuffer; `rubato` auf Worker (Agy H5). |
| 18 | Log | Rotation `diktier.log` / `.1`, kein In-Place-Truncate (Agy M1). |
| 19 | WER-Puffer | +0,05 gestrichen (Agy) — **revidiert in §18 #11**: die Frontends sind nicht identisch. |

## 18. Entscheidungen zum Claude-Review

| # | Frage | Entscheidung |
|---|---|---|
| 1 | Restore-Regel | Kein Restore ohne bedienten Read; Windows Delayed Rendering; 5-s-Fenster (Claude H1). |
| 2 | PTT-Key | Erreicht die fokussierte App nie; LL-Hook schluckt (Claude H2). |
| 3 | Modifier-Restore | Nur bei physisch gehaltener Taste (Claude M1). |
| 4 | Fokus | Start- = Ende-Kennung = Vordergrund; nicht ermittelbar = Verlust (Claude M2). |
| 5 | Watchdog | max(30 s, 5× Audiolänge) → error; RSS-Gate auch 60 s (Claude M3). |
| 6 | conhost | `Ctrl+V`, nicht `Ctrl+Shift+V` (Claude M4). |
| 7 | TrayClick | Immer `copy_only`, kein Paste-Key (Claude M5). |
| 8 | Stille | RMS-Gate in Diktier erlaubt, kein Engine-Fail (Claude N1). |
| 9 | Phase-1-Artefakte | Omarchy-Kopie zulässig bis zur HF-URL (Claude N2). |
| 10 | Linux-Build | Verbindlich Mint-22-Basis (Claude N3). |
| 11 | WER-Puffer (Phase-1-Beleg) | +0,05 wiederhergestellt (Owner, 2026-08-26): byte-gleiche Artefakte, aber verschiedene Mel-Frontends (Voxtype Kaldi-fbank, parakeet-rs NeMo-Style); 4/5 Dateien wortidentisch, „Werstadt“-Fall in `docs/SPIKES.md`. |
