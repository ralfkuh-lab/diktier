# Diktier

Lokales Push-to-Talk-Diktat für **Windows**. Taste halten, sprechen,
loslassen — der Text landet am Cursor. Läuft komplett offline mit NVIDIA
Parakeet (TDT 0.6B v3), kein Cloud-Dienst, kein Konto.

Status: **läuft auf Windows 11** (Hotkey, Tray, Einfügen am Cursor,
Modell-Download, Autostart). Privates Werkzeug, bewusst klein gehalten. Es
gibt noch kein fertiges Release-Paket — Bauen aus dem Quellcode, siehe unten.

Linux (Mint/X11) war die Ausgangsplattform und ist im Code noch enthalten,
wird aber nicht mehr weiterentwickelt.

## Voraussetzungen

- Windows 10 22H2 oder Windows 11, x64
- Ein Mikrofon (Standard-Aufnahmegerät von Windows)
- Beim ersten Start Internet für den einmaligen Modell-Download (~640 MB)

Kein Admin nötig. Diktier fügt **nicht** in als Administrator gestartete
Programme ein (Windows-Schutz UIPI) — der Text liegt dann in der
Zwischenablage.

## Bauen und starten

Rust-Toolchain (MSVC) installieren, dann:

```powershell
scripts\fetch-ort.ps1          # lädt lib\onnxruntime.dll (ONNX Runtime 1.28.0)
cargo build --release
.\target\release\diktier.exe --foreground   # erster Start mit Log im Terminal
```

Die `onnxruntime.dll` muss in `lib\` **neben der Exe** liegen; das Skript
legt sie auch nach `target\release\lib\`. Der Ordner mit Exe + `lib\` ist
portabel und darf verschoben werden.

Beim ersten Start lädt Diktier das Sprachmodell nach
`%LOCALAPPDATA%\diktier\models\parakeet-tdt-0.6b-v3-int8\` (vier Dateien,
jede gegen Größe und SHA-256 geprüft; Tray zeigt „Lade Modell …").
Danach ist der Start in etwa zwei Sekunden erledigt.

Autostart mit Windows:

```powershell
.\target\release\diktier.exe --install-autostart   # Eintrag im Startup-Ordner
.\target\release\diktier.exe --remove-autostart
```

Wird die Exe später verschoben, genügt ein erneutes `--install-autostart`.

## Das erste Diktat

1. Tray-Symbol abwarten, bis der Tooltip `idle` zeigt. Tipp: Das Symbol
   einmal aus dem Überlauf (`^`) in die Taskleiste ziehen, damit es immer
   sichtbar ist — Windows merkt sich das.
2. Cursor dorthin setzen, wo der Text hin soll (Editor, Browser, Teams …).
3. **F9 halten**, sprechen, loslassen.
4. Der Text wird über die Zwischenablage eingefügt; der vorherige
   Clipboard-Inhalt wird direkt danach wiederhergestellt.

Diktier öffnet beim Diktieren kein Fenster und wechselt den Fokus nie.
Wechselst du während der Aufnahme das Fenster, wird **nicht** eingefügt —
der Text liegt dann in der Zwischenablage (Strg+V).

Tray:
- **Linksklick**: Aufnahme starten/stoppen ohne Hotkey — Text landet nur in
  der Zwischenablage.
- **Rechtsklick**: Status, Hotkey pausieren, Konfiguration bearbeiten,
  Beenden.

Das Mikrofon bleibt im Hintergrund geöffnet, damit die Aufnahme sofort
startet — Windows zeigt deshalb dauerhaft „Mikrofon wird verwendet von
diktier". Andere Programme (Teams, Zoom) können das Mikrofon trotzdem
gleichzeitig nutzen; außerhalb einer Aufnahme wird nichts gespeichert.

## Konfiguration

`%APPDATA%\diktier\config.toml` (Tray → „Konfiguration bearbeiten"). Für die
meisten reicht der Hotkey:

```toml
[hotkey]
key = "F9"          # z. B. "F9", "ScrollLock", "Pause", "F12"
modifiers = []      # z. B. ["Ctrl", "Alt"] — Hotkey ist dann Ctrl+Alt+<key>
```

Gute Push-to-Talk-Tasten sind solche, die sonst nichts tun: `ScrollLock`
(Rollen), `Pause`, hohe F-Tasten. Der Hotkey erreicht das aktive Programm
nie — F9 togglet also keinen Breakpoint in VS Code.

Weitere Schlüssel (selten nötig): `[audio] device`, `max_duration_secs`
(Obergrenze je Aufnahme, 60 s), `[output] leading_space` (führendes
Leerzeichen, an), `paste_shortcut` (`auto` erkennt Windows Terminal und nimmt
dort Strg+Shift+V), `restore_clipboard`. Das Sprachmodell ist fest.

Änderungen gelten nach einem Neustart von Diktier.

## Wenn etwas nicht klappt

- **Text erscheint nicht, liegt aber in der Zwischenablage.** Fokus hat
  gewechselt, oder das Zielprogramm läuft als Administrator. Strg+V drücken.
- **Nichts wird erkannt.** Pegel zu leise oder Mikrofon gemutet (Headset-
  Taste). Mit `--foreground` zeigt das Log `rms=…`; Werte unter 0,0075
  gelten als Stille.
- **Hotkey geht nicht.** Tray zeigt `error`, Tooltip nennt den Grund. Andere
  Taste eintragen, neu starten. Linksklick im Tray geht immer.
- **„läuft bereits".** Es läuft schon eine Instanz (Autostart). Der zweite
  Start endet absichtlich mit Exit 0.
- **Log:** `%LOCALAPPDATA%\diktier\diktier.log` (rotiert bei 2 MiB). Dort
  stehen nie Transkripte oder Clipboard-Inhalte.

## Technik in einem Absatz

Rust, ohne GUI-Framework. Hotkey über einen `WH_KEYBOARD_LL`-Hook,
Einfügen über Clipboard + `SendInput` (Strg+V) mit Wiederherstellung des
alten Inhalts, Tray über `Shell_NotifyIcon`. Spracherkennung mit
[parakeet-rs](https://crates.io/crates/parakeet-rs) auf der ONNX Runtime
(CPU, INT8) — auf einem aktuellen Laptop rund 0,1 s pro Diktat. Details und
Entscheidungen: [docs/SPEC.md](docs/SPEC.md), Windows-Portierung:
[docs/windows-plan.md](docs/windows-plan.md).

## Lizenz

Diktier: MIT ([LICENSE](LICENSE)). Modell: NVIDIA Parakeet TDT 0.6B v3,
ONNX-INT8-Konvertierung
[istupakov/parakeet-tdt-0.6b-v3-onnx](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx),
[CC-BY-4.0](LICENSES/CC-BY-4.0.txt), Attribution in
[LICENSES/NOTICE-parakeet.md](LICENSES/NOTICE-parakeet.md). ONNX Runtime:
MIT ([LICENSES/ONNXRUNTIME-LICENSE.txt](LICENSES/ONNXRUNTIME-LICENSE.txt)).
Weitere Bestandteile: [LICENSES/THIRD-PARTY.md](LICENSES/THIRD-PARTY.md).
