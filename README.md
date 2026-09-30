# Diktier

Lokales Push-to-Talk-Diktat für **Windows**. Taste halten, sprechen,
loslassen — der Text landet am Cursor. Läuft komplett offline mit NVIDIA
Parakeet (TDT 0.6B v3), kein Cloud-Dienst, kein Konto.

Status: **läuft auf Windows 11** (Hotkey, Tray, Einfügen am Cursor,
Modell-Download, Autostart, Aufnahme-Overlay mit Mikrofonpegel).
Aktuelle Version: [v0.4.0](https://github.com/ralfkuh-lab/diktier/releases/tag/v0.4.0).
Privates Werkzeug, bewusst klein gehalten.

Linux (Mint/X11) war die Ausgangsplattform; der Linux-Code ist inzwischen
vollständig entfernt — Diktier ist Windows-only.

## Voraussetzungen

- Windows 10 22H2 oder Windows 11, x64
- Ein Mikrofon (Standard-Aufnahmegerät von Windows)
- Beim ersten Start Internet für den einmaligen Modell-Download (~640 MB)

Kein Admin nötig. Diktier fügt **nicht** in als Administrator gestartete
Programme ein (Windows-Schutz UIPI) — der Text liegt dann in der
Zwischenablage.

## Installation

Die Setup-Exe (`Diktier_<version>_x64-setup.exe`) aus den
[Releases](https://github.com/ralfkuh-lab/diktier/releases) herunterladen
und starten. Sie ist **nicht signiert** — Windows SmartScreen meldet deshalb
„Der Computer wurde durch Windows geschützt" und „Unbekannter Herausgeber":

> **Weitere Informationen** → **Trotzdem ausführen**

Der Installer braucht keine Administratorrechte und macht:

- Programm, `lib\onnxruntime.dll`, Lizenzen und `versions.toml` nach
  `%LOCALAPPDATA%\Programs\Diktier`
- eine Verknüpfung im Startmenü (optional zusätzlich auf dem Desktop)
- auf Wunsch den Autostart-Eintrag („Mit Windows starten", vorausgewählt)
- einen Eintrag in „Apps & Features" für die Deinstallation

Ein laufendes Diktier wird vor dem Kopieren beendet; danach kann es direkt von
der letzten Seite des Installers aus starten. Das Sprachmodell ist **nicht**
im Setup enthalten — es wird beim ersten Start geladen (siehe unten).

**Deinstallation**: Windows-Einstellungen → „Apps" → „Installierte Apps" →
Diktier → Deinstallieren. Am Ende fragt der Uninstaller, ob auch das
heruntergeladene Sprachmodell und die Einstellungen weg sollen (~650 MB in
`%LOCALAPPDATA%\diktier` und `%APPDATA%\diktier`).

Wer lieber nichts installiert: das Zip `diktier-<version>-win-x64.zip` aus
denselben Releases irgendwohin entpacken und `diktier.exe` starten — der
Ordner ist portabel, solange `lib\` daneben bleibt.

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

Release-Paket bauen (Bundle, Zip und Setup-Exe in `dist\`):

```powershell
scripts\release.ps1
```

Das Skript liest die Version aus `Cargo.toml`, baut mit `--locked`, legt
`dist\diktier-<version>-win-x64\` samt `versions.toml` an, zippt es und ruft
`makensis` mit `installer\diktier.nsi` auf (NSIS 3.x; gefunden wird
`%LOCALAPPDATA%\tauri\NSIS\makensis.exe`, `makensis` im `PATH` oder
`%ProgramFiles(x86)%\NSIS`). Mit `-TargetDir target-dev` baut es neben einem
laufenden Daemon, `-SkipInstaller` lässt das Setup weg. Das Exe-Icon kommt aus
`assets\diktier.ico` (neu erzeugen: `python scripts\make-icon.py`).

## Das erste Diktat

1. Tray-Symbol abwarten, bis der Tooltip `idle` zeigt. Tipp: Das Symbol
   einmal aus dem Überlauf (`^`) in die Taskleiste ziehen, damit es immer
   sichtbar ist — Windows merkt sich das.
2. Cursor dorthin setzen, wo der Text hin soll (Editor, Browser, Teams …).
3. **F9 halten**, sprechen, loslassen. Während der Aufnahme zeigt eine
   kleine Karte unten am Bildschirm den Mikrofonpegel (siehe
   [Aufnahme-Overlay](#aufnahme-overlay)) — bewegt sich die Wellenform
   beim Sprechen, kommt auch etwas an.
4. Der Text wird über die Zwischenablage eingefügt. Nach einem bedienten
   Clipboard-Read und der Mindestwartezeit schreibt Diktier den vorherigen
   Clipboard-Inhalt zurück (Voreinstellung `restore_clipboard = true`). Der
   Read ist kein Beweis für erfolgreiches Einfügen — auch ein
   Clipboard-Manager kann ihn auslösen. Seit 0.4.0 kommen dabei auch
   Bilder, Dateien und formatierter Text zurück, nicht nur reiner Text. Geht
   dabei etwas verloren, meldet es eine Hinweiskarte (siehe
   [Zwischenablage](#zwischenablage)). Im Windows-Verlauf (Win+V) erscheinen
   weder das Diktat noch die Wiederherstellung, sofern Diktier den
   Verlaufsausschluss setzen konnte (sonst steht es im Log).

Diktier öffnet beim Diktieren kein Fenster und wechselt den Fokus nie.
Wechselst du während der Aufnahme das Fenster, wird **nicht** eingefügt —
der Text liegt dann in der Zwischenablage (Strg+V).

Tray:
- **Linksklick**: Aufnahme starten/stoppen ohne Hotkey — Text landet nur in
  der Zwischenablage.
- **Rechtsklick**: Status, Hotkey pausieren, **Mit Windows starten**
  (Häkchen = Autostart-Eintrag vorhanden; Klick legt ihn an bzw. entfernt
  ihn), Hotkey ändern…, Konfiguration bearbeiten, Beenden.

Das Mikrofon bleibt im Hintergrund geöffnet, damit die Aufnahme sofort
startet — Windows zeigt deshalb dauerhaft „Mikrofon wird verwendet von
diktier". Andere Programme (Teams, Zoom) können das Mikrofon trotzdem
gleichzeitig nutzen; außerhalb einer Aufnahme wird nichts gespeichert.

## Konfiguration

`%APPDATA%\diktier\config.toml` (Tray → „Konfiguration bearbeiten"). Für die
meisten reicht der Hotkey:

```toml
[hotkey]
key = "F9"          # z. B. "F9", "ScrollLock", "Pause", "RCtrl", "F12"
modifiers = []      # z. B. ["Ctrl", "Alt"] — Hotkey ist dann Ctrl+Alt+<key>
```

Gute Push-to-Talk-Tasten sind solche, die sonst nichts tun: `ScrollLock`
(Rollen), `Pause`, `RCtrl` (die rechte Strg-Taste), hohe F-Tasten. Der Hotkey
erreicht das aktive Programm nie — F9 togglet also keinen Breakpoint in
VS Code.

`RCtrl` ist die rechte Strg-Taste als **Taste**, nicht als Modifier (Aliase:
`RightCtrl`, `RStrg`, `Strg rechts`). Die linke Strg bleibt Modifier: mit
`key = "RCtrl"` und `modifiers = ["ctrl"]` löst erst *linke Strg + rechte
Strg* aus, mit `modifiers = []` die rechte Strg allein.

### Aufnahme-Overlay

Während der Aufnahme erscheint unten mittig auf dem Bildschirm des aktiven
Fensters eine kleine dunkle Karte mit dem Mikrofonpegel: eine mitlaufende
Wellenform und darunter ein Pegelmeter mit Peak-Marke. Sie bleibt stehen, bis
der Text eingefügt ist, und verschwindet dann von selbst.

Die Karte nimmt **nie** den Fokus: Tippen läuft ununterbrochen weiter, und
Klicks gehen durch sie hindurch auf das Fenster darunter. Eine flache Linie
trotz Sprechens heißt, dass das Mikrofon stumm oder viel zu leise ist.

Abschalten:

```toml
[overlay]
enabled = false
```

Weitere Schlüssel (selten nötig): `[audio] device`, `max_duration_secs`
(Obergrenze je Aufnahme, 60 s), `[output] leading_space` (führendes
Leerzeichen, an), `paste_shortcut` (`auto` erkennt Windows Terminal und nimmt
dort Strg+Shift+V), `restore_clipboard`. Das Sprachmodell ist fest.

Änderungen gelten nach einem Neustart von Diktier.

### `output.mode`: nur noch `"paste"` (Breaking Change in 0.4.0)

`output.mode` kennt nur noch den Wert `"paste"`. Das frühere `"type"` wurde
bis 0.3.0 zwar gelesen, aber nie ausgewertet — Diktier hat trotzdem über die
Zwischenablage eingefügt. Seit 0.4.0 ist `"type"` ein Konfigurationsfehler:
Tray `error`, kein Hotkey, keine Aufnahme; Tooltip und Log melden

```
output.mode "type" gibt es nicht mehr — bitte "paste" eintragen oder die Zeile löschen
```

Migration: in `config.toml` unter `[output]` `mode = "paste"` eintragen oder
die Zeile `mode = …` löschen (fehlt sie, gilt `"paste"`), dann Diktier neu
starten. Configs mit `"paste"` oder ohne den Schlüssel sind nicht betroffen.

## Wenn etwas nicht klappt

- **Text erscheint nicht, liegt aber in der Zwischenablage.** Fokus hat
  gewechselt, oder das Zielprogramm läuft als Administrator. Strg+V drücken.
- **Nächstes Diktat wird erst nach ein paar Sekunden eingefügt.** Kommt nach
  dem Einfügen kein bedienter Clipboard-Read, wartet Diktier bis zu
  5 Sekunden darauf, bevor es den Text endgültig in die Zwischenablage legt —
  auch mit `restore_clipboard = false` oder wenn sich nichts sichern ließ.
  Ein in dieser Zeit fertiges Diktat kommt danach dran, ebenso ein Beenden.
  Kommt der Read früher, entfällt der Rest der Wartezeit; ein Beweis für
  erfolgreiches Einfügen ist er nicht.
- **Tray `error` mit „Zwischenablage leer — Transkript verloren“.** Windows
  hat das Setzen des Transkripts verweigert, nachdem die Zwischenablage schon
  geleert war (selten). Das kann auch einige Sekunden nach dem Diktat
  passieren, wenn ein späterer Versuch scheitert; läuft dann schon das
  nächste Diktat, steht es nur im Log. Das Diktat ist weg; neu diktieren.
- **Log-Warnung „Transkript beim Beenden nicht gesichert — Zwischenablage
  kann leer sein“.** Beim Beenden ließ sich das zuletzt diktierte Transkript
  nicht endgültig in die Zwischenablage legen (Grund in Klammern). Danach
  kann sie leer sein.
- **Nichts wird erkannt.** Mikrofon gemutet (Headset-Taste) oder das
  falsche Gerät aktiv — das Overlay zeigt es sofort: flache Linie trotz
  Sprechens. Genauer nachmessen: Das Log schreibt zu **jeder** Aufnahme
  eine Zeile `Lauf N: Gate: …` mit Entscheidung und Messwerten.
- **Overlay zeigt Pegel, aber es wird nichts eingefügt, auch im
  Editor nicht.** Dann hat der Silence-Gate die Aufnahme verworfen und die
  Engine gar nicht erst aufgerufen. Die Gate-Zeile im Log nennt die Regel:
  `Regel A` = kürzer als 250 ms, `Regel C` = Pegel unter der absoluten
  Untergrenze (Mikrofon praktisch stumm), `Regel D` = kein
  zusammenhängender Sprachabschnitt von 1,5 s, der 12 dB über dem
  Grundrauschen der Aufnahme liegt. **Seit 0.3.0** misst der Gate
  **relativ** zum Grundrauschen der Aufnahme — ein durchweg leises Mikrofon
  allein führt also nicht mehr zu „leer“ —, und `Regel B3` lässt zusätzlich
  leises Sprechen ohne Pause durch (2 s am Stück über 0,004), bei dem es
  kein Rauschfenster als Bezug gibt. Bleibt es bei `Regel D`, hilft meist:
  deutlicher sprechen, eine kurze Pause vor dem Diktat lassen und den
  Eingangspegel in den Windows-Soundeinstellungen anheben.
- **Gate nachrechnen.** `diktier.exe --gate-analyze <datei.wav> …` wertet
  fertige 16-kHz-Mono-WAVs ohne Modell aus: Report je Datei, die Laufdauern
  über den absoluten Schwellen von B3 und B2 sowie die Laufdauern bei
  +10/+12/+15 dB Marge. Aufnahmen zum Nachrechnen liefert
  `--foreground --record-test 10` (Text auf stdout, Gate-Report auf
  stderr) oder der Daemon mit `DIKTIER_DEBUG_WAV=1` (siehe
  [Debug-WAV](#debug-wav)).
- **Overlay ist weg, obwohl das Log „Overlay sichtbar" meldet.** Windows
  hat das Fenster aus dem Topmost-Band genommen, es liegt unter dem
  Zielfenster. Seit 0.2.1 behauptet Diktier die Position bei jedem
  Einblenden neu; bei älteren Versionen hilft ein Neustart von Diktier.
- **Hotkey geht nicht.** Tray zeigt `error`, Tooltip nennt den Grund. Andere
  Taste eintragen, neu starten. Linksklick im Tray geht immer.
- **„läuft bereits".** Es läuft schon eine Instanz (Autostart). Der zweite
  Start endet absichtlich mit Exit 0.
- **`output.mode "type" gibt es nicht mehr`** im Log, Tray `error`: siehe
  [`output.mode`](#outputmode-nur-noch-paste-breaking-change-in-040).
- **Log:** `%LOCALAPPDATA%\diktier\diktier.log` (rotiert bei 2 MiB). Dort
  stehen nie Transkripte oder Clipboard-Inhalte, nur Metadaten wie
  Format-IDs, Größen und Dauern.

### Zwischenablage

Vor dem Einfügen sichert Diktier den Inhalt der Zwischenablage. Zurück
schreibt es ihn nur mit `restore_clipboard = true` (Voreinstellung) und erst
nach einem bedienten Clipboard-Read des Transkripts und der Mindestwartezeit
(`restore_clipboard_delay_ms`). Der Read ist eine Heuristik, kein Beweis für
erfolgreiches Einfügen: Auch ein Clipboard-Manager kann ihn auslösen. Kommt
innerhalb von 5 Sekunden keiner, bleibt das Transkript in der
Zwischenablage. Gesichert werden seit
0.4.0 **alle Formate, die Windows auslesen lässt**, Byte für Byte und in der
ursprünglichen Reihenfolge: reiner und formatierter Text (RTF, HTML), Bilder
und Screenshots (DIB, PNG, EMF), im Explorer kopierte oder ausgeschnittene
Dateien samt Kopieren/Verschieben-Kennung und die privaten Formate der
Anwendungen. Lässt sich davon etwas nicht sichern oder zurückschreiben, ist
die Wiederherstellung nur teilweise — dann erscheint die Hinweiskarte.

Das Transkript und der wiederhergestellte Inhalt sind vom Windows-Verlauf
(Win+V) und von der Cloud-Zwischenablage ausgeschlossen, sofern Diktier den
Ausschluss-Marker setzen konnte: Diktate und Wiederherstellungen erscheinen
dort dann nicht, das Original steht vom ursprünglichen Kopieren schon drin.
Gelingt der Marker nicht, steht im Log `Verlauf ausgeschlossen: nein` bzw.
eine Warnung `Verlaufsausschluss nicht …` — dieses Diktat kann dann im
Verlauf landen. Für Clipboard-Manager anderer Hersteller (Ditto u. a.) gibt
es keine Zusage.

Grenzen — das geht auch bei „vollständig“ nicht mit:

- **OLE-Objekte.** Was eine Anwendung als lebendes Objekt anbietet, ist nach
  dem Zurückschreiben nur noch Daten. Typisch sind kopierte
  **Outlook-Elemente** (Mails, Termine): Sie kommen nur teilweise oder gar
  nicht zurück, dann erscheint die Hinweiskarte. „Verknüpfung einfügen“,
  virtuelle Dateien und Rückmeldungen an die Quelle nach dem Einfügen
  entfallen ebenfalls; bei im Explorer **ausgeschnittenen** Dateien kann das
  Verschieben deshalb anders ausgehen als ohne Diktat.
- **Excels Kopierrahmen.** Die Zellen sind wieder in der Zwischenablage, aber
  Excel ist nicht mehr ihr Besitzer: Der laufende Rahmen um den kopierten
  Bereich verschwindet.
- **Synthetisch ersetzte Bildformate.** `CF_BITMAP`, `CF_PALETTE` und
  `CF_METAFILEPICT` sichert Diktier nicht selbst; Windows erzeugt sie aus dem
  gesicherten DIB bzw. EMF neu. Nur wenn eine Quelle darin andere Daten als
  im DIB/EMF angeboten hat, bekommt die einfügende Anwendung die erzeugte
  Variante. Das Log nennt die Formate, einen Hinweis gibt es dafür nicht.

**Hinweiskarte.** Ist der vorherige Inhalt ganz oder teilweise weg, zeigt
die [Overlay-Karte](#aufnahme-overlay) nach dem Einfügen für 3 Sekunden
einen Hinweis mit Warnzeichen. Auch sie nimmt nie den Fokus.

| Zeile 1 | Zeile 2 | Bedeutung |
|---|---|---|
| Zwischenablage nicht gesichert | Vorheriger Inhalt wurde überschrieben | nichts Einfügbares ließ sich sichern |
| Zwischenablage nicht wiederhergestellt | Vorheriger Inhalt wurde überschrieben | Zurückschreiben gescheitert, das Transkript liegt in der Zwischenablage |
| Zwischenablage teilweise wiederhergestellt | Nicht alle Formate ließen sich sichern | einzelne Formate schon beim Sichern verloren |
| Zwischenablage teilweise wiederhergestellt | Nicht alle Formate ließen sich zurückschreiben | einzelne Formate beim Zurückschreiben verloren |
| Einfügen nicht bestätigt | Text liegt in der Zwischenablage | innerhalb von 5 s kein Clipboard-Read (z. B. Programm als Administrator) |
| Fokus gewechselt – nicht eingefügt | Text liegt in der Zwischenablage | Fenster während des Diktats gewechselt |
| Text liegt in der Zwischenablage | Mit Strg+V einfügen | Diktat per Linksklick im Tray |

Kein Hinweis erscheint bei vollständiger Wiederherstellung (auch wenn
OLE-Verweise entfallen oder Bildformate ersetzt wurden), wenn während des
Diktats jemand anderes kopiert hat, bei `restore_clipboard = false` und bei
leerem Transkript. Ist das Overlay abgeschaltet oder ausgefallen, steht der
Hinweis nur im Log (`Hinweis: …`).

**Nachsehen, was gesichert würde:**

```powershell
.\diktier.exe --clipboard-check
```

listet jedes Format der aktuellen Zwischenablage mit ID, Name, Klasse
(gesichert, synthetisch ersetzt, OLE-Verweis entfällt, Verlust samt Grund)
und Größe, nie Inhalte. Es ist nur lesend, darf also neben dem laufenden
Daemon laufen; es kann aber bei der Quelle das verzögerte Erzeugen eines
Formats anstoßen. Exitcode `0` = alles Auslesbare sicherbar, `3` = Verluste
oder nichts sicherbar, `1` = Fehler.

```powershell
.\diktier.exe --clipboard-check --roundtrip
```

> **Achtung:** `--roundtrip` **überschreibt die Zwischenablage** kurz mit
> einem Testtext und schreibt dann den gesicherten Inhalt zurück. Er startet
> nur, wenn Diktier nicht läuft (vorher Tray → Beenden). Danach vergleicht
> er IDs, Reihenfolge und Bytes. „Byte-identisch“ sagt nichts über
> OLE-Objekte, virtuelle Dateien oder den Besitzer (Excels Rahmen) — die
> gehen beim Zurückschreiben wie oben beschrieben verloren. Wer nicht
> riskieren will, den aktuellen Inhalt zu verlieren, nimmt nur
> `--clipboard-check`.

### Debug-WAV

Mit der Umgebungsvariable `DIKTIER_DEBUG_WAV=1` speichert der Daemon jede
Aufnahme als 16-kHz-Mono-WAV:

```
%TEMP%\diktier\rec_<UTC-Zeit bis Millisekunde>_lauf-<N>.wav
z. B. rec_2026-09-25T14-47-13-512Z_lauf-703.wav
```

Die Zeit ist UTC wie im Log, `<N>` ist die Laufnummer aus den Logzeilen
`Lauf N: …`; jeder Dump steht dort zusätzlich als eine Zeile
`DIKTIER_DEBUG_WAV: <pfad>`. Ist der Name schon belegt (etwa nach einem
Neustart mit gleicher Laufnummer), hängt Diktier `-2`, `-3` … an
(`rec_…_lauf-703-2.wav`); eine vorhandene Datei wird nie überschrieben.
Diktier behält die **zehn jüngsten** Dateien dieses Musters und löscht
ältere; liegengebliebene `.part`-Reste eines abgebrochenen Dumps entfernt es
erst, wenn sie **älter als eine Stunde** sind. Andere Dateien im Ordner fasst
es nicht an. Die frühere `last_recording.wav` (bis 0.3.0) wird beim ersten
Dump entfernt.

Einschalten als Benutzervariable, danach Diktier neu starten:

```powershell
[Environment]::SetEnvironmentVariable("DIKTIER_DEBUG_WAV", "1", "User")
```

Ausschalten: denselben Befehl mit `$null` statt `"1"`. Die Aufnahmen
enthalten, was du gesagt hast — nicht weitergeben.

## Technik in einem Absatz

Rust, ohne GUI-Framework. Hotkey über einen `WH_KEYBOARD_LL`-Hook,
Einfügen über Clipboard + `SendInput` (Strg+V) mit Wiederherstellung des
alten Inhalts, Tray über `Shell_NotifyIcon`, das Aufnahme-Overlay als
selbst gezeichnetes Layered Window (`UpdateLayeredWindow`, nimmt nie den
Fokus). Spracherkennung mit
[parakeet-rs](https://crates.io/crates/parakeet-rs) auf der ONNX Runtime
(CPU, INT8) — auf einem aktuellen Laptop rund 0,1 s pro Diktat. Details und
Entscheidungen: [docs/SPEC.md](docs/SPEC.md), Windows-Portierung:
[docs/windows-plan.md](docs/windows-plan.md), Overlay:
[docs/overlay-plan.md](docs/overlay-plan.md).

## Lizenz

Diktier: MIT ([LICENSE](LICENSE)). Modell: NVIDIA Parakeet TDT 0.6B v3,
ONNX-INT8-Konvertierung
[istupakov/parakeet-tdt-0.6b-v3-onnx](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx),
[CC-BY-4.0](LICENSES/CC-BY-4.0.txt), Attribution in
[LICENSES/NOTICE-parakeet.md](LICENSES/NOTICE-parakeet.md). ONNX Runtime:
MIT ([LICENSES/ONNXRUNTIME-LICENSE.txt](LICENSES/ONNXRUNTIME-LICENSE.txt)).
Weitere Bestandteile: [LICENSES/THIRD-PARTY.md](LICENSES/THIRD-PARTY.md).
