# Phase 3c — Kontext-Briefing für das Kreuz-Review

Stand: 2026-08-26, Commit `8eb46e1` („Phase 3c: Daemon-Wiring (Linux)").
Verbindlich bleibt [docs/SPEC.md](../SPEC.md) v1.3. Dieses Dokument beschreibt
**nur**, was der Implementierer gebaut, entschieden und live geprüft hat — es
ersetzt keine eigene Prüfung und ist bewusst auch dort ehrlich, wo es unangenehm
wird (Abschnitte 4 und 5).

Reviewgegenstand ist der Diff von `3b7b98d` nach `8eb46e1`:

```
neu       src/daemon/{mod,dispatch,workers,logging,debug_wav,signals}.rs   ~2 600 Zeilen
geändert  src/audio/{capture,mod,spsc}.rs   Stream-Lebenszyklus (Owner-Entscheidungen)
geändert  src/inject/{mod,linux}.rs         SAVE_TARGETS + ClipboardSave
geändert  src/main.rs                       run_daemon → daemon::run
unberührt src/state.rs                      der pure Kern aus 3a/3b, Tests unantastbar
```

**Nicht Teil von 3c** (kommt als 3d, bitte nicht als Lücke melden):
Single-Instance-Lock (§5.3), Modell-Download (§6.3), Autostart (§9),
Datei-Log + Rotation (§10 Teil 2). Windows ist durchgehend `cfg`-Stub.

Gates am Reviewstand: `cargo build`, `cargo test` (204 grün, 1 `#[ignore]`
stt-smoke), `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`.

## 1. Architektur des Daemon-Wirings

### Module

| Datei | Rolle |
|---|---|
| `daemon/mod.rs` | `run()`, Event-Loop, `Daemon`-Struct (implementiert `Actors`), Quit-Pfad |
| `daemon/dispatch.rs` | `Actors`-Trait, `dispatch()`, `Timers` (Cap/Watchdog) — gegen Fakes testbar |
| `daemon/workers.rs` | Engine-, Audio-, Inject-, Hotkey-, Tray-Thread; `Msg`; Event-Mappings |
| `daemon/logging.rs` | stderr-Log (§10 Teil 1), `LogEvent` → deutscher Klartext |
| `daemon/debug_wav.rs` | `DIKTIER_DEBUG_WAV=1`, atomar, `0600` |
| `daemon/signals.rs` | SIGTERM/SIGINT → reguläres `QuitRequested` (SIGHUP bewusst nicht) |

### Threads

| Thread | Besitzt | Wartet auf | Sendet |
|---|---|---|---|
| main (Event-Loop) | `Runtime`, `Timers`, Worker-Handles | `rx.recv_timeout` (≤ 20 ms) | Worker-Kommandos |
| `diktier-engine` | `ParakeetTranscriber` (resident) | `rx.recv()` blockierend | `ModelLoaded/Failed`, `TranscriptionDone/Failed` |
| `diktier-audio` | `CpalAudioSource` (cpal `Stream` ist `!Send`) | `rx.recv()` blockierend | `Msg::Audio`, `CaptureFailed` |
| `diktier-inject` | X11-Connection (`X11OutputSink`) | `try_recv` + 10 ms X11-Pump | `InjectFinished` |
| `diktier-hotkey` | `HotkeyBackend` (global-hotkey / XGrabKey) | 5 ms Poll | `HotkeyPress/Release`, `HotkeyUnavailable` |
| `diktier-tray` | `betrayer`-Icon (D-Bus-Thread liegt darunter) | 20 ms Poll | `TrayClickToggle`, `PauseToggle`, `QuitRequested`, `OpenConfigDir`, `TrayLost` |

Die Loop macht **nichts Langes**: jeder Effekt wird zu einem Kanal-Kommando.
Das ist die Bedingung dafür, dass `QuitRequested` auch während einer Inferenz
oder eines Restore-Wartens greift (codex H4 zu §7.1 P6, in Phase 2 vertagt).

### Event- und Effektfluss

```
Worker/Timer/Signale ──► VecDeque<Event> ──► state::transition(&mut Runtime, ev)
                                                        │  Vec<Effect>
                        Timers ◄── dispatch(effects, timers, daemon, now) ──► Actors
                                                                              (Worker)
```

Reihenfolge in jeder Runde — die Invarianten, an denen sich Fehler zeigen:

1. Queue leerfahren (`transition` + `dispatch`), dabei entstandene Events anhängen.
2. Signal-Flag prüfen → `QuitRequested`.
3. Gepufferten Hotkey-Fehler einspeisen, **sobald** `state == Idle`.
4. `recv_timeout(min(nächster Tick, nächste Frist))`, dann alles Wartende drainen.
5. **Erst** `Timers::due(now)`, **dann** `Event::Tick{elapsed}` in die Queue.

Schritt 5 ist bewusst so: Kern und Wiring führen dieselben Fristen (der Kern in
`Runtime::now`, das nur über `Tick` wächst). Weil der Wiring-Timer zuerst feuert,
räumt `CapReached`/`WatchdogTimeout` die Kern-Deadline mit ab — sonst löste
derselbe Übergang zweimal aus (§4.4 „genau einmal"). Test:
`cap_fires_once_even_when_the_core_clock_reaches_the_same_deadline`.

### run-Generationen (`RunId`)

Der Kern zählt bei jedem Aufnahmestart, Abschluss, Fehler und Quit hoch; jede
asynchrone Antwort trägt ihre `RunId` und wird bei Nichtübereinstimmung als
`StaleRun` verworfen. Das Wiring hält daran zwei Dinge fest:

- `Daemon::pending_audio: Option<(RunId, Vec<f32>)>` — die Samples bleiben im
  Wiring, der Kern sieht nur `AudioInfo::duration`. `StartTranscription{run}`
  nimmt sie nur, wenn die `RunId` passt, sonst `TranscriptionFailed`.
- `ContextSlot` im Inject-Thread (§7.3): `start_window_id` bei `StartCapture`,
  `target_window_id` bei `StopCapture{discard:false}`. Ein `Paste` mit fremder
  Generation bekommt bewusst `None`-Kennungen und fällt damit auf `copy_only`.

`AbortTranscription` (Watchdog, fataler Fehler, Quit) gibt den Engine-Worker
auf (`abandon()`): `parakeet-rs` kennt keinen Abbruch, der Thread läuft aus,
seine Antwort trägt eine tote Generation. Der Reinit bekommt einen frischen
Worker.

## 2. Owner-Entscheidungen (jeweils ein Satz Begründung)

| # | Entscheidung | Warum |
|---|---|---|
| 1 | **Capture-Stream bleibt in `idle` offen und läuft** (`prepare()` beim Idle-Eintritt; der cpal-Callback verwirft Frames, solange nicht `armed`) | Ein neu geöffneter Stream kostete gemessen ~2 s Geräteanlauf und schnitt damit den Anfang jedes Diktats ab; ein nur *pausierter* Stream hilft nicht, weil PipeWire die Quelle nach ~3 s suspendiert. |
| 2 | **Gerät wird bei `paused` freigegeben** (`release()`), Tray-Click zahlt dann wieder den Anlauf | „Pausiert" heißt „ich will jetzt nicht diktieren" — dann soll auch kein Mikrofon offen stehen; der Anlauf im Tray-Click-Pfad ist der akzeptierte Preis. |
| 3 | **Injectfehler → `error`** (`ErrorKind::Inject`), Transkript bleibt im Clipboard, Hotkey bleibt scharf | §7.1: kein stilles Verwerfen; der Retry ist das nächste Diktat, deshalb kein Hotkey-Entzug. |
| 4 | **Eigener Kern-Zustand `Injecting`** (in §5.2 nicht genannt), im Tray weiter als `transcribing` sichtbar | Der Paste muss nichtblockierend sein (codex H4), also braucht der Kern einen Zustand zwischen `StartInject`/`CopyOnly` und `InjectFinished`; §4.3 kennt aber keinen sichtbaren Zustand „injecting". |
| 5 | **RMS-Silence-Gate fensterbasiert** (`max` über 250-ms-Fenster, plus 2-s-Loud-Run-Regel) statt Gesamt-RMS | Sonst fällt kurze leise Sprache in langer Stille unter die Schwelle, während `rauschen.wav` weiterhin leer bleiben muss (agy B3 / codex N1). |

Weitere Wiring-Entscheidungen des Implementierers (nicht Owner-Ebene, aber
prüfenswert):

- Hotkey-Registrierungsfehler wird **bis `idle` gepuffert** — sofort gemeldet
  würde `FatalError` die Startsequenz töten und der Tray-Click hätte nie ein
  geladenes Modell (§4.4 verlangt genau umgekehrt, dass er bedienbar bleibt).
- `CheckArtifacts`/`StartDownload` laufen **synchron** in der Loop (vier `stat`
  bzw. eine Fehlermeldung) und speisen ihr Ergebnis über `Daemon::emitted` zurück.
- Scheitert der X11-Sink-Aufbau, endet der Start mit **Exit 1** statt in einen
  degradierten Wayland-Modus zu gehen (§2 skizziert einen solchen; offen für 3d/4).
- `SAVE_TARGETS` beim Quit mit Fallback: erst explizite Targetliste, bei
  Ablehnung ein zweiter Versuch mit leerer Liste (ICCCM: „alle Targets").

## 3. Live belegt (Mint 22, Cinnamon, X11, `--foreground`)

Alles unten wurde am laufenden Binary geprüft, nicht nur im Test:

- Kaltstart `starting → loading → idle` in ~2,0 s; RSS 0,99–1,11 GiB (< 2 GiB).
- SNI-Item im Panel, Tooltip `idle — <modellschlüssel>`, Menü exakt §4.3.
- **PTT-Diktat in xed**: Paste `ctrl_v`, Start- = Zielfenster, `restore true`;
  Umlaute (Jörg, Björn, März, grüße) byte-korrekt.
- **Diktat ohne Vorlauf** (F9 → 150 ms → sprechen): Satzanfang vollständig —
  der Beleg für Entscheidung 1.
- **Ruhezustand puffert nichts**: 13 s Sprache abgespielt *ohne* F9, danach
  1,5 s F9 → exakt 1,536 s Audio, Transkript leer (Privacy-Beleg zu Entsch. 1).
- **Pause-Semantik** (pactl): `source-outputs` 1 → **0** beim Pausieren, Quelle
  `SUSPENDED`; nach Pause-Ende zurück auf 1/`RUNNING`, Diktat danach mit
  `Aufnahme läuft nach 0.000 s`. Tray-Click *in* der Pause: `1.990 s` Anlauf,
  Diktat läuft trotzdem durch, danach sofort wieder freigegeben.
- §7.3 **Fokuswechsel** während der Aufnahme → kein Paste, `copy_only`, Ziel
  unverändert.
- §4.3 **Tray-Click** → immer `copy_only`, kein Paste-Key.
- §4.4 **Cap** (mit `max_duration_secs = 6`): Stop exakt 6,001 s nach dem
  Aufnahmestart, spätes Release wird ignoriert.
- §4.1 P5 **leeres Transkript** → kein Inject, kein Fehler.
- §10 **`DIKTIER_DEBUG_WAV=1`**: Datei `0600` in Verzeichnis `0700`, atomar
  überschrieben, genau eine Logzeile.
- **Quit** über Tray-Menü (0,31 s) und SIGTERM (0,21 s), auch mitten in
  `recording` und in `transcribing` — Lauf verworfen, kein Inject, **Exit 0**,
  keine Restprozesse; Mikrofon wird freigegeben (`source-outputs → 0`).
- **Fehlende Artefakte**: verständlicher Fehler, Tray `error`, F9 tot,
  Prozess bleibt über den Tray beendbar.

Nicht live geprüft (nur Unit-Test bzw. Code-Inspektion): Watchdog-Auslösung mit
echter hängender Engine, hartes Prozessende nach 5 s, Device-lost-Recovery mit
physisch entferntem Gerät, Panel-Neustart, alles unter Windows.

## 4. Offene Punkte und bekannte Schwächen

1. **Poll-Intervalle** (vom Owner ausdrücklich diesem Review überlassen):
   Grundlast im Ruhezustand ~2,3 % einer CPU, aufgeschlüsselt
   `inject 0,80 %`, `hotkey 0,50 + 0,10 %`, `audio 0,50 + 0,10 %`,
   `tray 0,15 %`, `loop 0,15 %`. Vorschlag war Inject-Serve 10 → 50 ms und
   Hotkey-Poll 5 → 10 ms (spart ~1 %); sauberer, aber aufwendiger wäre für den
   Inject-Thread ein `poll()` auf den X11-Filedescriptor statt der Schleife.
2. **ORT-stderr beim Quit-Abbruch**: Wird eine laufende Inferenz vom Quit
   abgeschnitten, schreibt ONNX Runtime noch eine C++-Fehlerzeile
   (`GetElementType is not implemented`), gefolgt von unserer `ERROR
   Transkription:`-Zeile — **nach** `beendet`. Exit bleibt 0, kein Zombie;
   unterdrückbar wäre nur die eigene Zeile.
3. **`betrayer` verschluckt den ersten `Activate`** nach Prozessstart (bekannt
   seit 2b): der erste Tray-Linksklick im Leben eines Prozesses tut nichts.
4. **Cinnamon lehnt `SAVE_TARGETS` ab** (`property == None`, beide
   Anfrageformen, Antwort in ~29 ms). Der Handshake selbst funktioniert; der
   Phase-2-Verlustfall ist trotzdem entschärft, weil `csd-clipboard` seinen
   eigenen Fetch wiederherstellt — belegt, aber eben nicht durch unser
   Protokoll. Auf Desktops mit echtem Manager (gsd, Klipper, clipman) greift
   der reguläre Pfad; dort ist es **ungetestet**.
5. **`xdg-open` wird nicht abgeräumt**: `tray::open_config_dir()` macht
   `Command::spawn()` ohne `wait()` — das Kind bleibt bis zum Prozessende
   Zombie. Bestehender 2b-Code, jetzt aber im Daemon-Pfad aktiv.
6. **Wayland**: Scheitert der X11-Sink, endet der Start mit Exit 1 statt in den
   in §2 skizzierten degradierten Modus („Tray darf Quit anbieten") zu gehen.
7. **MULTIPLE-Target** im X11-Selection-Handler weiterhin nicht implementiert
   (dokumentierte v1-Lücke aus Phase 2).
8. **Startsequenz prüft nur Größe**, nicht SHA-256 der Artefakte
   (`check_artifacts`) — bewusst, wegen Kaltstart-Budget; der Vollcheck gehört
   in den Download-Pfad in 3d.
9. **Zwei Instanzen** greifen sich derzeit gegenseitig Hotkey und Clipboard —
   Single-Instance ist 3d, aber bis dahin ein reales Betriebsrisiko.

## 5. Wo Fehler am ehesten stecken (Selbsteinschätzung)

Priorisiert, mit Fundstelle. Genau hier lohnt der zweite Blick:

1. **`stop()` in `audio/capture.rs` — Rennen zwischen Gate und Drain.**
   `armed = false` → `wait_for_producer_quiet()` → `drain`. Die Wartefunktion
   erkennt nur einen *stillstehenden* Write-Cursor; ein Callback, der exakt
   zwischen `armed.load()` (noch `true`) und `push_frame` steht, kann nach dem
   Drain noch schreiben. Fenster ist winzig und der nächste `start()` macht
   `reset()`, aber die Argumentation ist nicht wasserdicht — bitte prüfen, ob
   ein Sequenzzähler oder ein „drain bis zwei leere Runden" sauberer wäre.
2. **`Msg::Audio` wird gespeichert, bevor der Kern das Event bewertet**
   (`daemon/mod.rs::handle_msg`). Bei einem verworfenen Lauf bleiben bis zu
   ~3,8 MB Samples liegen, bis das nächste Diktat oder ein `AbortTranscription`
   sie räumt. Kein Korrektheitsfehler, aber prüfen, ob es einen Pfad gibt, auf
   dem alte Samples doch noch in einen neuen Lauf geraten.
3. **`ContextSlot` wird bei `discard: true` nicht geräumt**
   (`daemon/workers.rs::inject_loop`). Praktisch unschädlich, weil der Kern nach
   einem verworfenen Lauf die Generation hochzählt — aber genau das ist eine
   Annahme über den Kern, die im Wiring nirgends abgesichert ist.
4. **Reihenfolge Timer-vor-Tick** (`daemon/mod.rs::event_loop`, Schritt 5).
   Wenn `recv_timeout` spät zurückkehrt, landen Cap-Event und Tick in derselben
   Runde; die Argumentation „Kern-Deadline ist dann schon geräumt" hängt daran,
   dass `stop_recording` `cap_deadline` löscht. Lohnt eine Gegenprobe mit
   künstlich verzögerter Loop.
5. **Quit-Budget.** `save_targets` darf bis 2 s kosten, danach joinen vier
   Worker mit je bis zu 2 s, alles gedeckelt auf 5 s Restzeit
   (`Daemon::shutdown`). Prüfen, ob eine Kombination (Inject hängt im Paste +
   Engine in der Inferenz) die 5-s-Zusage aus §5.2 doch reißen kann.
6. **Hotkey-Fehler-Puffer.** Wird `idle` nie erreicht (z. B. Modellfehler),
   erscheint der Registrierungsfehler nur als WARN im Log, nie im Tray. Ist das
   die richtige Auslegung von §4.4/§10?
7. **`prepare()` im `update_tray`-Pfad.** Der Gerätezustand hängt jetzt an einem
   *Tray*-Effekt. Konzeptionell fragwürdig — falls der Kern je `UpdateTray`
   auslässt (er tut es, wenn sich `(state, paused)` nicht ändert), bleibt das
   Gerät im falschen Zustand. Prüfen, ob es einen Pfad nach `idle` ohne
   `UpdateTray` gibt.
8. **`TrayLost` → Exit 1** beendet den Prozess, ohne dass der Kern je
   `QuitRequested` gesehen hat; der Shutdown läuft trotzdem über
   `Daemon::shutdown`. Ist das Verhalten bei einem Panel-Neustart wirklich
   gewünscht (§10 sagt „Prozessende" nur für den *Aufbau*-Fehler)?
9. **Signal-Handler** (`daemon/signals.rs`) nutzt `signal()` mit einer eigenen
   `extern "C"`-Deklaration statt `sigaction`; der Handler schreibt nur ein
   `AtomicBool`. Bitte auf async-signal-safety und Wiederinstallation prüfen.
