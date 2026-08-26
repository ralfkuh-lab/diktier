# Review Phase 3 — Tray, State-Machine und Daemon (codex)

Datum: 2026-08-27  
Verbindliche Referenz: `docs/SPEC.md` v1.3  
Geprüfter Scope: `git diff 6ffc6b7..85f853c`

## Kurzfazit

Der pure State-Kern ist breit und überwiegend präzise getestet; Run-Generationen,
Cap und Watchdog sind im Kern sowie im Timer-Dispatcher konsistent. Der globale
5-s-Shutdown-Etat wird im Wiring tatsächlich als gemeinsame Deadline geführt.
Auch die Downloadtransaktion hält die Reihenfolge aus §6.3 ein: `.part`,
Größe+Hash, `sync_all`, Rename und `COMPLETE` zuletzt. Ein Prozessabbruch nach
einem Datei-Rename und vor `COMPLETE` ist wiederaufnehmbar. Der Linux-Lock hält
seine FDs über die gesamte Daemon-Lebensdauer; liegengebliebene Dateien sind
unschädlich.

Nicht freigabefähig ist der Stand wegen zweier Nebenläufigkeitsfehler: Das
Audio-Gate beweist nicht, dass der Callback vor `drain`/`reset` beendet ist, und
ein Quit kann hinter einem zeitgleichen Engine-Ergebnis einsortiert werden,
wodurch doch noch ein Paste startet. Außerdem ignoriert das Produktions-Wiring
die Hotkey-Config vollständig und „Pause“ lässt den globalen F9-Grab aktiv.

Verifikation auf diesem Linux-Host:

- `cargo build`: erfolgreich.
- `cargo test`: 248 bestanden, 1 modellabhängiger STT-Smoke ignoriert.
- `cargo clippy --all-targets -- -D warnings`: erfolgreich.
- `cargo fmt --check` und `git diff --check`: erfolgreich.
- Keine Live-GUI-Tests, wie beauftragt.

## Befunde

### H1 — `armed=false` trennt Callback und Ring-Consumer nicht sicher

- **Schwere:** Hoch
- **Stelle:** `src/audio/capture.rs:280-294`, `src/audio/capture.rs:314-325`,
  `src/audio/capture.rs:404-429`, `src/audio/spsc.rs:63-107`
- **Problem:** Ein Callback kann `armed == true` gelesen haben und danach
  descheduled werden. `stop()` setzt dann `armed=false`; ein 10 ms lang
  unveränderter Write-Cursor sagt aber nicht, dass kein Callback mehr zwischen
  Gate, Slot-Schreibzugriffen und Cursor-Publish steckt. Ebenso läuft der Code
  nach 200 ms bewusst weiter. Damit können `drain()` oder ein späteres
  `reset()` mit `push_frame()` überlappen. Wegen der manuell als `Sync`
  markierten `UnsafeCell`-Slots ist das nicht nur ein verlorener Suffix,
  sondern potentiell ein nicht synchronisierter Read/Write und damit Undefined
  Behavior; späte Samples können zudem in die nächste Session geraten.
- **Vorschlag:** Im Callback vor der Gate-Prüfung einen atomaren
  In-flight-Zähler/Epoch betreten und nach dem letzten Ringzugriff verlassen.
  Stop: erst `armed=false`, dann nachweislich `in_flight==0`, erst danach
  drainen. Bei Ablauf der Obergrenze nicht trotzdem lesen, sondern Capture als
  fehlgeschlagen behandeln und Stream/Ring neu aufbauen. Ein deterministischer
  Test muss den Callback nach erfolgreichem Gate an einer Barriere festhalten
  und gleichzeitig Stop/Reset auslösen.

### H2 — Quit hat keine Priorität vor einem zeitgleichen Engine-Ergebnis

- **Schwere:** Hoch
- **Stelle:** `src/daemon/mod.rs:487-502`, `src/daemon/mod.rs:526-550`,
  `src/daemon/mod.rs:561-568`, `src/state.rs:642-660`
- **Problem:** Die Loop leert zuerst die vorhandene Queue und prüft erst danach
  das Signal-Flag. Bei Channel-Ereignissen werden alle Nachrichten FIFO
  angehängt. Liegen in `transcribing` zunächst `TranscriptionDone` und dahinter
  `QuitRequested`, dispatcht das erste Event sofort `StartInject`; erst danach
  verwirft Quit den Lauf. Dasselbe Rennen besteht, wenn SIGTERM bereits gesetzt
  ist, aber ein Engine-Event in der Queue liegt. RunIds schützen nur vor
  Ergebnissen **nach verarbeitetem** Quit, nicht vor dieser Prioritätsinversion.
  Das verletzt §5.2 „Beenden während Inferenz: kein Inject mehr“ und kann nach
  der Benutzeraktion noch Clipboard/Paste verändern.
- **Vorschlag:** Quit als Wiring-Latch mit höchster Priorität behandeln: Signal
  vor jedem Queue-Drain prüfen, eine empfangene Batch zuerst auf Quit scannen
  und ab dann keine Ausgabe-Effekte mehr dispatchen. Integrationstests für
  `[TranscriptionDone, QuitRequested]` sowie „Signal gesetzt + Ergebnis bereits
  queued“ müssen beweisen, dass kein Inject-Kommando gesendet wird.

### H3 — Hotkey-Config und Pause erreichen das Backend nicht

- **Schwere:** Hoch
- **Stelle:** `src/daemon/mod.rs:181-184`, `src/daemon/workers.rs:784-840`,
  `src/hotkey.rs:128-155`, `src/hotkey.rs:326-328`,
  `src/state.rs:587-602`
- **Problem:** Obwohl die Config `hotkey.key` und Modifier validiert, wird
  `HotkeyWorker::spawn` ohne `HotkeyConfig` aufgerufen; beide Linux-Backends
  registrieren hart F9 ohne Modifier. Außerdem ändert `PauseToggle` nur den
  Kernzustand. Der Worker bleibt registriert und der X11-Grab schluckt F9
  weiterhin, während der Kern die Events lediglich ignoriert. Damit wirkt eine
  konfigurierte Ausweichtaste nicht, und „Hotkey pausieren“ gibt die Taste der
  fokussierten Anwendung nicht zurück.
- **Vorschlag:** Die validierte Hotkey-Config bis in beide Backends reichen und
  den Worker über einen Command-Channel explizit registrieren/unregistrieren.
  Fake-Backend-Tests müssen exakte Taste+Modifier sowie Pause → Ungrab und
  Resume → Regrab prüfen.

### M1 — Hotkey-Ausfall und erforderliche Tooltip-Hinweise gehen verloren

- **Schwere:** Mittel
- **Stelle:** `src/state.rs:575-585`, `src/state.rs:760-789`,
  `src/daemon/workers.rs:845-933`, `src/tray.rs:153-170`,
  `src/tray.rs:297-346`
- **Problem:** Nach einem Registrierungsfehler darf ein Tray-Klick zwar eine
  Aufnahme starten, `start_recording()` löscht aber den Fehler und
  `finish_run()` endet in `idle`. Der Hotkey-Thread ist weiterhin beendet; nach
  genau einem Tray-Diktat zeigt die App trotzdem „idle“ und hält den Hotkey
  logisch für scharf. Zusätzlich bekommt der Tray-Worker nur `(state, paused)`
  und baut einen `Runtime` ohne `error` nach. Tooltips können daher weder den
  Hotkey-Konflikt noch Download-/Injectfehler, Fokusverlust oder
  „Text liegt in der Zwischenablage“ anzeigen; auch `CopyOnlyNotice` ist nur
  eine Logzeile. Das widerspricht §4.3, §4.4, §6.3, §7.3 und §10.
- **Vorschlag:** Backend-Verfügbarkeit und persistente Nutzerhinweise getrennt
  vom aktuellen Diktat-Zustand modellieren und vollständig an `TrayCmd::Update`
  übertragen. Ein kompletter TrayClick-Roundtrip aus
  `HotkeyRegistration` muss wieder im degradierten Fehlerzustand landen. Für
  jede spezifizierte Fehler-/copy_only-Ursache den exakten Tooltip testen.

### M2 — Worker-Start und Kanalabbruch werden als Erfolg verschluckt

- **Schwere:** Mittel
- **Stelle:** `src/daemon/workers.rs:111-127`,
  `src/daemon/workers.rs:220-246`, `src/daemon/workers.rs:374-405`,
  `src/daemon/workers.rs:789-805`, `src/state.rs:623-636`
- **Problem:** Engine-, Download-, Audio- und Hotkey-Thread verwenden bei
  `spawn()` jeweils `.ok()`; alle Command-Sends ignorieren ihr Ergebnis. Ein
  Spawnfehler oder späterer Worker-Panic wird deshalb nicht in einen
  Fehlerzustand übersetzt. Je nach Worker bleibt der Daemon unbegrenzt in
  `loading`, `downloading`, `recording` oder nach dem Stop in `transcribing`.
  Letzterer Pfad hat nicht einmal einen Watchdog, weil dieser erst nach
  `AudioReady` scharf wird. Das ist ein stiller Liveness-Verlust, kein sauberer
  Fehlerpfad.
- **Vorschlag:** Worker-Konstruktoren als `Result` ausführen, Sendefehler an die
  Loop melden und unerwartetes Threadende beaufsichtigen. Für Audio-Stop/-Ready
  eine eigene Frist vorsehen. Tests mit absichtlich geschlossenem Receiver und
  injiziertem Spawnfehler müssen jeweils einen terminierten Fehlerübergang
  statt eines Hängers prüfen.

### M3 — Fatale Configfehler umgehen den vorgeschriebenen Tray-Fehlerzustand

- **Schwere:** Mittel
- **Stelle:** `src/daemon/mod.rs:107-143`, `src/daemon/mod.rs:151-184`
- **Problem:** TOML-/Validierungsfehler und ein unbekannter Modellschlüssel
  kehren zurück, bevor Inject- und Tray-Worker aufgebaut werden. Der Prozess
  endet mit 2 und zeigt nur stderr/Dateilog. §8 und §10 verlangen für diese
  Klasse dagegen: kein Hotkey, keine Aufnahme, Tray `error`, über den der
  Prozess bedienbar bleibt.
- **Vorschlag:** Einen minimalen Tray-only-Fehlermodus vorsehen, der die
  verständliche Configursache zeigt und Quit/Config-Ordner bedient, ohne Audio,
  Hotkey oder Engine zu starten. Den Startpfad mit Fake-Tray für Syntaxfehler
  und unbekanntes Modell testen.

### M4 — Aliasierte Lock-Kandidaten können den ersten Prozess gegen sich selbst sperren

- **Schwere:** Mittel
- **Stelle:** `src/single_instance.rs:111-135`,
  `src/single_instance.rs:159-176`, `src/single_instance.rs:183-205`
- **Problem:** `acquire_all` dedupliziert seine Kandidaten nicht. Zeigen
  `$XDG_RUNTIME_DIR/diktier.lock` und der State-Fallback lexikalisch oder über
  einen Symlink auf dieselbe Datei, hält der erste Versuch den Lock und der
  zweite `open()+flock()` sieht wegen der neuen Open-File-Description den
  **eigenen** Lock als `Busy`. Der erste Daemon meldet dann Exit 0 „läuft
  bereits“, obwohl keine Instanz existiert. FD-Lebensdauer und normale
  `flock`-Freigabe sind ansonsten korrekt.
- **Vorschlag:** Kandidaten vor dem Sperren mindestens lexikalisch, bei
  existierenden Pfaden auch nach stabiler Dateiidentität deduplizieren. Tests
  für doppelte Pfade und zwei Symlink-Aliase ergänzen; die bestehenden Tests
  decken nur zwei verschiedene Dateien ab.

## Nicht als Befund bewertet

- Die dokumentierten Owner-Entscheidungen wurden nicht erneut aufgelistet.
- Die Reihenfolge und Wiederaufnahme der Downloadtransaktion ist für einen
  normalen Prozessabbruch konsistent; insbesondere bleibt ein bereits korrekt
  umbenanntes Artefakt nutzbar und wird beim nächsten Download per Hash
  übersprungen. `COMPLETE` wird erst nach allen Artefakten geschrieben.
- Der globale Shutdown-Etat überschreitet durch die sequentiellen Joins nicht
  additiv 5 s; alle Einzelbudgets werden gegen dieselbe Restdeadline gekappt.
- Die Poll-Last ist kein Korrektheitsblocker. Für den Inject-Thread ist ein
  `poll()` auf X11-FD plus Command-Wakeup langfristig sauberer als nur das
  Sleep-Intervall von 10 auf 50 ms zu erhöhen.
