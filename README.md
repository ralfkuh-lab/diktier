# Diktier

Lokales Push-to-Talk-Diktat für Windows und Linux Mint. Halten, sprechen,
loslassen — der Text landet am Cursor. Läuft offline mit NVIDIA Parakeet.

Status: **Linux fertig** (Daemon, Tray, Hotkey, Paste, Modell-Download,
Autostart, Release-Bundle). **Windows in Arbeit** — der Code hat die
Plattformschichten, aber Tray, Inject und Hotkey sind dort noch `cfg`-Stubs.
Verbindlich ist [docs/SPEC.md](docs/SPEC.md); Messwerte und Gate-Protokolle
stehen in [docs/SPIKES.md](docs/SPIKES.md).

## Zielplattformen (v1)

- Windows 10 22H2+ / Windows 11, x64
- Linux Mint 22.x, Cinnamon, **X11**, x86_64

Cinnamon/Wayland ist in v1 kein Supportziel.

Nicht das gleiche Tool wie [Voxtype](https://voxtype.io) auf Omarchy — Diktier
soll dieselbe Erkennungsqualität auf den übrigen Rechnern liefern.

## Installation (Linux)

Das Release ist ein **Bundle**, kein Einzelprogramm: Binary, ONNX Runtime,
Lizenzen und `versions.toml` gehören zusammen und bleiben in einem Ordner.

```bash
tar -xzf diktier-<version>-linux-x64.tar.gz
cd diktier-<version>-linux-x64
./diktier --install-autostart   # Eintrag in ~/.config/autostart/
./diktier --foreground          # erster Start, Logs im Terminal
```

Der Ordner darf liegen, wo er will (`~/opt/diktier`, ein USB-Stick, `/opt`) —
die ONNX Runtime wird immer aus `lib/` **neben der Binary** geladen, nie aus
dem System und nie über `PATH` oder `LD_LIBRARY_PATH`. Wird der Ordner später
verschoben, genügt ein erneutes `./diktier --install-autostart`: der bestehende
Eintrag wird aktualisiert, nicht verdoppelt.

Voraussetzungen: X11-Sitzung (Cinnamon), ein Tray, der StatusNotifierItem
spricht, und `libasound2t64` — auf einem Desktop mit Audio ohnehin vorhanden.
Alles Weitere steckt im Bundle; Details in
[LICENSES/THIRD-PARTY.md](LICENSES/THIRD-PARTY.md).

### Erster Start: Modell-Download

Beim ersten Start fehlt das Sprachmodell. Diktier lädt es dann selbst nach
`~/.local/share/diktier/models/parakeet-tdt-0.6b-v3-int8/`:

- **rund 650 MB**, vier Dateien, einmalig
- Quelle: [`istupakov/parakeet-tdt-0.6b-v3-onnx`](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx),
  feste Revision — die URLs stehen in `src/models.toml`
- jede Datei wird gegen Größe **und** SHA-256 geprüft, bevor sie gültig wird
- der Tray zeigt währenddessen „Lade Modell …", der Fortschritt steht im Log
- Lizenz der Artefakte: CC-BY-4.0, Attribution in
  [LICENSES/NOTICE-parakeet.md](LICENSES/NOTICE-parakeet.md)

Danach startet Diktier in rund zwei Sekunden. Wer die Dateien schon hat (etwa
von einem anderen Rechner), kopiert sie einfach in dieses Verzeichnis — der
Download entfällt dann.

## Das erste Diktat

1. Tray-Symbol abwarten, bis der Tooltip `idle` zeigt.
2. Cursor dorthin setzen, wo der Text hin soll (Editor, Terminal, Browser).
3. **F9 halten**, sprechen, loslassen.
4. Der Text wird über die Zwischenablage am Cursor eingefügt; der vorherige
   Clipboard-Inhalt wird kurz darauf wiederhergestellt.

Der Fokus wandert dabei nie: Diktier öffnet auf dem Diktatpfad kein Fenster.
Wechselst du während der Aufnahme das Fenster, wird **nicht** eingefügt — der
Text liegt dann in der Zwischenablage und du fügst ihn selbst ein.

Ein Linksklick auf das Tray-Symbol startet und stoppt eine Aufnahme ebenfalls;
dieser Weg fügt bewusst nichts ein, sondern legt den Text nur in die
Zwischenablage. Das Tray-Menü kennt außerdem Pause, „Config-Ordner öffnen"
und Beenden.

## Konfiguration

`~/.config/diktier/config.toml` wird beim ersten Start mit den Defaults
angelegt. Unbekannte Schlüssel und ungültige Werte werden gemeldet, nicht
stillschweigend übernommen.

```toml
[hotkey]
key = "F9"              # Push-to-Talk-Taste
modifiers = []          # z. B. ["Ctrl", "Alt"]

[audio]
device = "default"
max_duration_secs = 60  # harte Obergrenze je Aufnahme

[engine]
model = "parakeet-tdt-0.6b-v3-int8"
threads = 0             # 0 = Runtime-Default

[output]
mode = "paste"                  # "paste" | "type"
paste_shortcut = "auto"         # "auto" | "ctrl_v" | "ctrl_shift_v" | "shift_insert"
leading_space = true            # führendes Leerzeichen vor dem Text
restore_clipboard = true
restore_clipboard_delay_ms = 200

[tray]
show_notifications_on_error = true
```

Änderungen wirken nach einem Neustart des Daemons.

## Troubleshooting

**F9 ist schon belegt.** Der Tray zeigt `error`, das Log nennt
„Hotkey-Registrierung". Eine andere Taste in `config.toml` eintragen und
Diktier neu starten. Der Tray-Linksklick funktioniert auch ohne Hotkey.

**„Diktier v1 unterstützt nur X11" beim Start.** Die Sitzung läuft unter
Wayland. In der Anmeldemaske eine X11-Sitzung wählen (`echo $XDG_SESSION_TYPE`
zeigt `x11`, wenn es passt).

**Der Text erscheint nicht, liegt aber in der Zwischenablage.** Das Zielfenster
hat den Paste abgelehnt oder der Fokus hat gewechselt — Diktier verwirft in dem
Fall nichts, sondern übergibt an die Zwischenablage.

**„diktier läuft bereits".** Es läuft schon eine Instanz (Autostart). Der zweite
Start endet absichtlich wirkungslos mit Exit 0.

**Wo steht das Log?** `~/.local/state/diktier/diktier.log`, rotiert bei 2 MiB
nach `diktier.log.1`. Transkripte, Zwischenablage-Inhalte und Fenstertitel
stehen dort **nie** drin. Mit `--foreground` läuft dasselbe Log im Terminal mit.

**Autostart wieder loswerden:** `./diktier --remove-autostart`.

## Aus dem Quellcode bauen

```bash
scripts/fetch-ort.sh     # lädt lib/libonnxruntime.so (ONNX Runtime 1.28.0)
cargo build --release
scripts/release.sh       # baut dist/diktier-<version>-linux-x64[.tar.gz]
```

`scripts/release.sh` erzeugt das Bundle aus [docs/SPEC.md](docs/SPEC.md) §11
samt `versions.toml` (App-, ORT-, Crate-, Modell- und Toolchain-Versionen).

## Lizenz

Die Anwendung steht unter MIT. Siehe [LICENSE](LICENSE).

Die mitgelieferten bzw. heruntergeladenen **Parakeet-Modellartefakte**
(NVIDIA Parakeet TDT 0.6B v3, ONNX-INT8-Konvertierung
[istupakov/parakeet-tdt-0.6b-v3-onnx](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx),
Revision `8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce`) stehen unter
[CC-BY-4.0](LICENSES/CC-BY-4.0.txt). Attribution und Herkunft:
[LICENSES/NOTICE-parakeet.md](LICENSES/NOTICE-parakeet.md).

Die mitgelieferte **ONNX Runtime** steht unter MIT
([LICENSES/ONNXRUNTIME-LICENSE.txt](LICENSES/ONNXRUNTIME-LICENSE.txt)).
Weitere Fremdbestandteile: [LICENSES/THIRD-PARTY.md](LICENSES/THIRD-PARTY.md).
