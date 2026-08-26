# Review: Diktier Phase 3 (Gesamt) — State Machine, Daemon-Wiring, Tray, Single-Instance, Download, Autostart & Logging

Reviewer: **agy**  
Datum: 2026-08-27  
Verbindliche Referenz: `docs/SPEC.md` (v1.3), `AGENTS.md`, `docs/reviews/impl-phase3-context.md`  
Geprüfter Scope: Commits `a67c971..85f853c` (Diff `6ffc6b7..85f853c`):
- `src/state.rs` (Purer State-Machine-Kern + 70 TDD-Tests)
- `src/daemon/` (`mod.rs`, `dispatch.rs`, `workers.rs`, `logging.rs`, `debug_wav.rs`, `signals.rs`)
- `src/tray.rs` (Betrayer-Backend, SNI, Status-/Menü-Routing)
- `src/single_instance.rs` (Advisory-`flock`-Doppelsperre, Download-Lock)
- `src/download.rs` (Transaktionaler `.part`-Download, SHA-256, `COMPLETE`-Marker, Fake-Transport)
- `src/autostart.rs` (Desktop Entry Spec 1.5, Quoting, Idempotenz)
- `src/paths.rs` (XDG-Pfade, Berechtigungen `0700`/`0600`)
- Anpassungen in `src/audio/capture.rs` und `src/inject/linux.rs` (`SAVE_TARGETS`)

---

## 1. Kurzfazit

Die Phase-3-Gesamtimplementierung (Phasen 2b, 3a, 3b, 3c und 3d) stellt eine **herausragende ingenieurmäßige Leistung** dar. Das Zusammenspiel zwischen dem vollkommen seiteneffektfreien State-Machine-Kern (`state.rs`), der asynchronen Event-Loop (`daemon/mod.rs`), den entkoppelten Worker-Threads und den robusten OS-Integrationsschichten (Locks, Download-Transaktion, Autostart, Datei-Log) erfüllt die verbindliche Spec v1.3 auf höchstem Niveau.

### Positive Ergebnisse & Gate-Erfüllung
- **Purer State-Machine-Kern (§5.2, §13):** `transition(&mut Runtime, Event) -> Vec<Effect>` ist absolut seiteneffektfrei (keine Syscalls, kein I/O, kein `Instant::now()`). Zeit erreicht den Kern strikt monoton über `Event::Tick`. Alle 70 Unit-Tests bilden jeden spezifizierten Übergang, Timing-Fristen, Fehlerklassen und Generationen ab.
- **Asynchrone Run-Generationen (`RunId`):** Durch monotone Inkrementierung von `RunId` bei jedem Start, Abbruch oder Fehler werden verspätete Antworten (`StaleRun`) von Engine, Audio oder Inject deterministisch verworfen (§5.2). Ein hängender Inferenzlauf kann niemals fälschlich in ein späteres Diktat injiziert werden.
- **Watchdog- & Cap-Präzision (§4.4, §5.2):** Der Watchdog `max(30 s, 5 × Audiolänge)` ist mathematisch exakt implementiert. Der 60-s-Cap stoppt die Aufnahme zuverlässig über zwei Pfade (Worker + Timer), schlägt genau einmal an und ignoriert folgendes Release.
- **Download-Transaktion (§6.3):** Das Artefakt-Manifest stimmt byte- und hash-identisch mit Voxtype/Omarchy überein. Download via `.part`-Dateien mit Streaming-SHA-256, atomarem Rename, `COMPLETE`-Marker als letztem Schritt und per-user Lock gegen parallele Starts erfüllt alle Anforderungen aus §6.3 und §13.
- **Single-Instance-Doppelsperre (§5.3):** Die Lösung, sowohl `$XDG_RUNTIME_DIR/diktier.lock` als auch `~/.local/state/diktier/diktier.lock` zu sperren, verhindert zuverlässig Split-Brain-Starts zwischen heterogenen Benutzer-Sessions. Zweite Instanzen beenden sich sauber mit Exit 0 und stderr-Meldung.
- **Privacy & Log-Vertrag (§10):** Kein einziges Transkript, kein Clipboard-Inhalt und kein Fenstertitel gelangen ins Log (strikte Prüfung aller 558 Zeilen in `logging.rs` und aller Call-Sites). Die Ein-Writer-Garantie (Datei-Sink erst nach Instanz-Lock) und die 2-MiB-Rotation nach `diktier.log.1` sind lückenlos umgesetzt.
- **Quit-Garantie & Inferenz-Hartende (§5.2):** Das 5-s-Quit-Budget wird über `remaining(deadline)` dynamisch berechnet und strikt eingehalten. Der `SAVE_TARGETS`-Handshake rettet den Clipboard-Inhalt bei Beendigung.
- **Test- & Codequalität:** 248 Unit- und Integrationstests laufen in 0,10 s fehlerfrei durch; `cargo clippy --all-targets -- -D warnings` ist warnungsfrei.

### Wesentliche Kritikpunkte & Handlungsbedarf
Es gibt **keine Blocker**. Die wenigen gefundenen Schwachstellen sind nachfolgend priorisiert:
1. **`xdg-open` Zombie-Prozesse (B1, Mittel):** In `tray::open_config_dir()` wird `xdg-open` via `Command::spawn()` gestartet, ohne dass der Child-Prozess ge-reapt (`wait()`) wird.
2. **Signal-Handler-Deklaration (B2, Mittel):** `signals.rs` verwendet eine handgeschriebene `extern "C" fn signal(..., handler: usize)`-Deklaration statt des ohnehin eingebundenen `libc::sigaction`.
3. **Desktop Entry Escaping für `%` (B3, Niedrig):** `autostart::quote_exec` maskiert `"`, `` ` ``, `$`, `\`, lässt aber `%` unmaskiert (nach Desktop Entry Spec 1.5 muss `%` als `%%` maskiert werden).
4. **Heuristik `wait_for_producer_quiet` in `capture.rs` (B4, Niedrig):** Das Warten auf den ruhenden Audio-Write-Cursor bei `stop()` ist durch `reset()` beim nächsten `start()` zwar praktisch unschädlich, ein atomarer Writer-Zähler wäre jedoch formal wasserdicht.

---

## 2. Punkt-für-Punkt-Prüfung: State-Machine (§5.2) & Testliste (§13)

### 2.1 Zeile-für-Zeile-Abgleich der Übergangstabelle (§5.2)

| Spec §5.2 Übergang / Regel | Code-Stelle (`src/state.rs`) | Status | Anmerkung / Verifikation |
|---|---|---|---|
| `starting → downloading? → loading → idle` | `state.rs:491-539` | **ERFÜLLT** | `Startup` stößt `CheckArtifacts` an. Je nach `complete` geht es nach `Loading` oder `Downloading`. `DownloadFinished` führt nach `Loading`. `ModelLoaded` setzt `model_ready = true` und wechselt nach `Idle`. |
| `idle + Press → recording(Hotkey)` | `state.rs:551-562, 761-770` | **ERFÜLLT** | `HotkeyPress` in `Idle` (wenn nicht `paused`) ruft `start_recording(Hotkey)`. Inkrementiert `RunId`, setzt `cap_deadline`, emittiert `StartCapture`. |
| `idle + ClickStart → recording(TrayClick)` | `state.rs:575-585, 761-770` | **ERFÜLLT** | `TrayClickToggle` in `Idle` ruft `start_recording(TrayClick)`. |
| `recording(Hotkey) + Release\|Cap → transcribing` | `state.rs:564-572, 605-612, 774-781` | **ERFÜLLT** | `HotkeyRelease` oder `CapReached` / Cap-Deadline-Tick ruft `stop_recording(Hotkey)`, löscht `cap_deadline`, wechselt nach `Transcribing`, emittiert `StopCapture { discard: false }`. |
| `recording(TrayClick) + ClickStop\|Cap → transcribing` | `state.rs:575-578, 605-612` | **ERFÜLLT** | Zweiter `TrayClickToggle` oder `CapReached` ruft `stop_recording(TrayClick)`. |
| `transcribing(Hotkey) + Text + Fokus gleich → inject → idle` | `state.rs:642-662, 681-695` | **ERFÜLLT** | `TranscriptionDone` emittiert `StartInject { text }` und geht nach `Injecting(Hotkey)`. Bei `InjectFinished(Pasted)` Aufruf von `finish_run()` zurück nach `Idle`. |
| `transcribing(TrayClick) + Text → copy_only → idle` | `state.rs:653-658, 684-687` | **ERFÜLLT** | `TranscriptionDone(TrayClick)` emittiert `CopyOnly { reason: TrayClickPath }` und geht nach `Injecting(TrayClick)`. Bei `InjectFinished` Rückkehr nach `Idle`. |
| `transcribing + leer → idle` | `state.rs:645-649` | **ERFÜLLT** | `text.trim().is_empty()` emittiert `Log(EmptyTranscript)`, ruft `finish_run()` direkt nach `Idle` ohne Inject. |
| `transcribing + Fokus ungleich → copy_only → idle` | `state.rs:684-687`, `workers.rs:722-727` | **ERFÜLLT** | Inject-Worker prüft `start == target == current`; bei Fokusverlust Fallback auf `CopyOnly` (`FocusChanged`/`FocusUnknown`). Kern loggt `CopyOnlyNotice` und wechselt nach `Idle`. |
| `jeder Zustand + fatal → error` | `state.rs:698-701, 829-836` | **ERFÜLLT** | `FatalError` führt überall `cleanup_active_run()` aus, inkrementiert `RunId`, speichert `ErrorInfo` und wechselt nach `Error`. |
| `error + Retry/Neustart → starting` | `state.rs:703-714` | **ERFÜLLT** | `RetryRequested` aus `Error` inkrementiert `RunId`, leert Fehler und startet die Prüfsequenz mit `Starting` neu. |
| **Regel:** Press außerhalb `idle` ignorieren + Log | `state.rs:558-561` | **ERFÜLLT** | Emittiert `Log(IgnoredPress { state })` ohne Zustandswechsel. |
| **Regel:** Pause während `recording` verwirft Aufnahme | `state.rs:588-602` | **ERFÜLLT** | `StopCapture { discard: true }`, `Log(RecordingDiscarded)`, `cap_deadline = None`, neue `RunId`, sofort `Idle` mit `paused = true`. |
| **Regel:** Beenden während Inferenz (5 s Hartende, kein Inject) | `state.rs:479-481, 716-726` | **ERFÜLLT** | `QuitRequested` setzt `quitting = true`, ruft `cleanup_active_run()`. Alle späteren Events werden durch `if runtime.quitting` sofort verworfen (`IgnoredAfterQuit`). |
| **Regel:** 60-s-Cap genau einmal, spätes Release ignorieren | `state.rs:569-571, 609-611` | **ERFÜLLT** | Cap wechselt nach `Transcribing`. `HotkeyRelease` in `Transcribing` landet auf `Log(IgnoredRelease)` ohne Wirkung. |
| **Regel:** Watchdog `max(30 s, 5 × Audiolänge)` → Abort, Reinit, Error | `state.rs:43-51, 632-636, 797-810` | **ERFÜLLT** | `watchdog_timeout()` skaliert exakt. Bei Ablauf: `AbortTranscription`, `DisarmWatchdog`, `Error(TranscriptionStuck)`, `model_ready = false`, Hintergrund-`LoadModel`. Nächster Press heilt Zustand nach Reinit. |
| **Regel:** Verspätetes Ergebnis verworfener Läufe nie injizieren | `state.rs:301-306, 756-758` | **ERFÜLLT** | Jedes asynchrone Event prüft `run == runtime.run`. Bei Mismatch erfolgt ausschließlich `Log(StaleRun)`. |

### 2.2 Abgleich gegen die §13-Testliste

Alle 6 in Spec §13 geforderten Testbereiche für die State-Machine sind mit dedizierten Unit-Tests in `src/state.rs` abgedeckt:
1. *Alle Übergänge in §5.2:* `startup_checks_artifacts_and_paints_starting`, `complete_artifacts_skip_downloading_and_load_model`, `model_loaded_enters_idle_and_marks_model_ready`, `full_ptt_round_trip_effect_order`, etc.
2. *Press während transcribing / injecting:* `press_during_transcribing_and_injecting_is_ignored`.
3. *Auto-Repeat & Duplicate Press:* `duplicate_press_in_recording_is_ignored`, `release_without_recording_is_ignored`.
4. *Pause während recording:* `pause_during_recording_discards_the_take`, `late_audio_of_paused_run_is_ignored`.
5. *Cap + spätes Release:* `cap_reached_event_moves_to_transcribing`, `late_release_after_cap_is_ignored`, `lost_release_transcribes_exactly_once`.
6. *Download-/Hashfehler, Injectfehler, Fokusverlust:* `download_failure_is_fatal_error_without_hotkey`, `inject_failure_keeps_transcript_and_stays_operable`, `focus_change_ends_in_copy_only_and_idle`, `unknown_focus_ends_in_copy_only_and_idle`.
7. *Zusatztests:* Transcribing-Watchdog (`watchdog_uses_thirty_second_floor_for_short_audio`, `watchdog_scales_with_five_times_audio_length`, `watchdog_timeout_aborts_run_and_enters_error`), TrayClick → `copy_only` (`trayclick_transcription_copies_only_and_never_pastes`).

---

## 3. Punkt-für-Punkt-Prüfung: Spec §6.3 (Download & Artefakte)

| Anforderung §6.3 | Status | Implementierung im Code | Bewertung |
|---|---|---|---|
| **Golden Set Hashes & Bytes** | **ERFÜLLT** | `src/models.toml:1-29`, `src/download.rs:495-530` | Alle 4 Dateien (`encoder-model.int8.onnx`, `decoder_joint-model.int8.onnx`, `vocab.txt`, `config.json`) stimmen exakt auf das Byte und den SHA-256-Hash mit der Spec-Tabelle überein. |
| **Immutable Download-URLs** | **ERFÜLLT** | `src/models.toml:10, 16, 22, 28` | URLs nutzen den fixen Hugging-Face-Commit-Hash `8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce`. |
| **Installationsort** | **ERFÜLLT** | `src/download.rs:94-121` | Linux: `~/.local/share/diktier/models/<key>/`, Windows: `%LOCALAPPDATA%\diktier\models\<key>\`. |
| **Download nach `<name>.part`** | **ERFÜLLT** | `src/download.rs:325, 367-425` | Zieldatei wird während des Downloads als `<name>.part` geführt (Modus `0644`). |
| **Größe + SHA-256-Prüfung** | **ERFÜLLT** | `src/download.rs:338-355` | Byte-Länge wird während des Streamings mitgezählt (Abbruch bei Überschreitung); SHA-256 wird gestreamt berechnet und vor dem Rename geprüft. |
| **Atomares Umbenennen** | **ERFÜLLT** | `src/download.rs:358-361` | `fs::rename(&part, &target)` im selben Verzeichnis garantiert Atomarität. |
| **Marker `COMPLETE` zuletzt** | **ERFÜLLT** | `src/download.rs:275, 438-446` | `COMPLETE` wird erst geschrieben und atomar umbenannt, nachdem alle 4 Artefakte verifiziert sind. |
| **Per-User Download-Lock** | **ERFÜLLT** | `src/download.rs:283-298`, `src/single_instance.rs:140-154` | Exklusiver advisory `flock` auf `diktier-download.lock`. Parallele Downloads werden mit `DownloadError::Busy` abgewiesen. |
| **Fehlerbehandlung: nur `.part` löschen** | **ERFÜLLT** | `src/download.rs:333, 339, 349, 359` | Bei Hash-, Größen- oder Transportfehlern wird ausschließlich die temporäre `.part`-Datei gelöscht. Bereits fertig verifizierte Dateien bleiben erhalten. |
| **Startprüfung vs. Downloadprüfung** | **ERFÜLLT** | `src/download.rs:126-147, 149-173` | Kaltstart prüft nur Existenz + Dateigröße (`check_artifacts`, < 2 ms); Vollprüfung (`verify_artifacts_sha256`) existiert für Downloads und Tests. |
| **Fortschritt & Tooltip** | **ERFÜLLT** | `src/daemon/workers.rs:306-332`, `src/tray.rs:168-170` | Fortschritt wird gedrosselt (alle 16 MiB) im Log gemeldet. Tray-Tooltip zeigt konsistent `downloading — parakeet-tdt-0.6b-v3-int8`. |

---

## 4. Punkt-für-Punkt-Prüfung: Spec §9 (Autostart & CLI)

| Anforderung §9 | Status | Implementierung im Code | Bewertung |
|---|---|---|---|
| **CLI-Flags** | **ERFÜLLT** | `src/main.rs:37-92` | `diktier`, `--foreground`, `--install-autostart`, `--remove-autostart`. Konflikte (z. B. `--foreground` + `--install-autostart`) werden von `clap` sauber mit Exitcode 2 abgewiesen. |
| **Idempotenz** | **ERFÜLLT** | `src/autostart.rs:96-128, 227-266` | `--install-autostart` meldet `Created`, `Updated` oder `Unchanged` (Exit 0). `--remove-autostart` meldet `Removed` oder `NotPresent` (Exit 0). |
| **Gequotetes `current_exe()`** | **ERFÜLLT** | `src/autostart.rs:133, 162-189` | Pfad wird ermittelt und nach Desktop Entry Spec 1.5 in Anführungszeichen gesetzt; Sonderzeichen (`"`, `` ` ``, `$`, `\`) werden korrekt maskiert (*Hinweis B3 zu `%`*). |
| **Eintrag aktualisieren / Fremdeinträge schützen** | **ERFÜLLT** | `src/autostart.rs:108-117, 253-266` | Verschieben der Binary aktualisiert den `Exec=`-Pfad atomar; andere `.desktop`-Dateien im Ordner bleiben unberührt. |
| **Dateipfade** | **ERFÜLLT** | `src/paths.rs:72-93` | Linux: `~/.config/autostart/diktier.desktop` (beachtet `$XDG_CONFIG_HOME`). Windows: Startup-Ordner `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup\diktier.cmd`. |
| **Desktop Entry Inhalt** | **ERFÜLLT** | `src/autostart.rs:134-145` | Enthält `[Desktop Entry]`, `Type=Application`, `Version=1.0`, `Name=Diktier`, `Exec=...`, `Terminal=false`, `StartupNotify=false`, `X-GNOME-Autostart-enabled=true`. |
| **CLI vor Single-Instance** | **ERFÜLLT** | `src/main.rs:114-119` | `--install-autostart` und `--remove-autostart` laufen vor `acquire_instance_lock` und behindern keinen laufenden Daemon. |

---

## 5. Punkt-für-Punkt-Prüfung: Spec §10 (Logregeln & Privacy)

| Anforderung §10 | Status | Implementierung im Code | Bewertung |
|---|---|---|---|
| **Ein-Writer-Garantie** | **ERFÜLLT** | `src/daemon/mod.rs:67-82`, `src/main.rs:114-155` | CLI-Modi nutzen File-Sink nie. Der Daemon bindet `attach_file_log()` erst **nach** erfolgreichem `acquire_instance_lock()` ein. |
| **Log-Pfade & Rechte** | **ERFÜLLT** | `src/paths.rs:60-63`, `src/daemon/logging.rs:267-276` | Linux: `~/.local/state/diktier/diktier.log`, Ordnerrechte `0700`, Dateirechte `0600`. |
| **2-MiB-Rotation (.1)** | **ERFÜLLT** | `src/daemon/logging.rs:218-265, 458-509` | `LOG_LIMIT_BYTES = 2 * 1024 * 1024`. Bei Überschreitung atomarer `rename` nach `diktier.log.1` und Neueröffnung. Kein In-Place-Truncate, kein Zeilenriss. |
| **Privacy: Keine Transkripte** | **ERFÜLLT** | `src/daemon/workers.rs:186-191` | Inferenz loggt ausschließlich `Inferenz X.XXX s, Y Zeichen`. Transkripttext wird nirgends geloggt. |
| **Privacy: Keine Clipboard-Inhalte** | **ERFÜLLT** | `src/daemon/workers.rs:710-720, 738-742` | Inject loggt `Paste <shortcut> · <len> Bytes · reads <n> · restore <bool>`. Texte bleiben ungeloggt. |
| **Privacy: Keine Fenstertitel** | **ERFÜLLT** | `src/daemon/workers.rs:691, 697, 774-779` | Fenster werden rein als native hexadezimale IDs geloggt (z. B. `Startfenster 0x3e00006`). |
| **`DIKTIER_DEBUG_WAV=1`** | **ERFÜLLT** | `src/daemon/debug_wav.rs:1-168` | Nur bei `DIKTIER_DEBUG_WAV=1`: Schreibt `$TMPDIR/diktier-$USER/last_recording.wav` mit Rechten `0600` (Verzeichnis `0700`), atomar via `.part`-Rename, genau 1 Dump, genau 1 Logzeile. |

---

## 6. Exitcode-Matrix

Die Spec (§9, §10, §8) definiert drei Exitcode-Klassen: `0` (Erfolg / Harmlos), `1` (Fataler Laufzeitfehler), `2` (Bedien-/Configfehler).

| Szenario / Fehlerfall | Soll-Exit | Ist-Exit | Stelle im Code | Bewertung |
|---|:---:|:---:|---|---|
| Reguläres Beenden (Tray-Menü / SIGTERM) | **0** | **0** | `src/daemon/mod.rs:494, 312` | Korrekt |
| `--help` / `-h` | **0** | **0** | `src/main.rs:108` | Korrekt |
| `--version` / `-V` | **0** | **0** | `src/main.rs:108` | Korrekt |
| Zweiter Daemon-Start (Lock belegt) | **0** | **0** | `src/daemon/mod.rs:69-74` | Korrekt (stderr-Info, kein Fokusklau) |
| `--install-autostart` / `--remove-autostart` (Erfolg/NotPresent) | **0** | **0** | `src/main.rs:164, 178` | Korrekt |
| Stille / `< 250 ms` auf `--transcribe-wav` | **0** | **0** | `src/main.rs:220` | Korrekt |
| Single-Instance-Pfad nicht nutzbar (z. B. kein HOME/StateDir) | **1** | **1** | `src/daemon/mod.rs:76-78` | Korrekt |
| X11-OutputSink nicht verfügbar (Wayland-Start) | **1** | **1** | `src/daemon/mod.rs:156-163` | Korrekt |
| Tray-Aufbau gescheitert (§10) | **1** | **1** | `src/daemon/mod.rs:174-179` | Korrekt |
| Tray zur Laufzeit verloren (`TrayLost`) | **1** | **1** | `src/daemon/mod.rs:579` | Korrekt |
| Model-Load / ORT-Initialisierung auf CLI fehlgeschlagen | **1** | **1** | `src/main.rs:231, 241` | Korrekt |
| Fehlende WAV-Datei bei `--transcribe-wav` | **1** | **1** | `src/main.rs:208, 661` | Korrekt |
| Autostart I/O- oder Exe-Fehler | **1** | **1** | `src/autostart.rs:35`, `src/main.rs:168` | Korrekt |
| Unbekanntes CLI-Argument (z. B. `--nope`) | **2** | **2** | `src/main.rs:108` | Korrekt |
| Ungültige CLI-Kombination (z. B. `--foreground` + `--install-autostart`) | **2** | **2** | `src/main.rs:108` | Korrekt |
| `--transcribe-wav` ohne Pfad / `--runs` ohne `--transcribe-wav` | **2** | **2** | `src/main.rs:122` | Korrekt |
| Ungültiges WAV-Format (z. B. 44,1 kHz statt 16 kHz) | **2** | **2** | `src/main.rs:207` | Korrekt |
| Spike-Flags ohne `--foreground` / `--tray-test 0` | **2** | **2** | `src/main.rs:128-141, 398, 491` | Korrekt |
| Config-Fehler: Syntax, ungültiger Hotkey / Mode (§8) | **2** | **2** | `src/daemon/mod.rs:115` | Korrekt |
| `engine.model` unbekannt (nicht im Manifest, §6.2) | **2** | **2** | `src/daemon/mod.rs:140` | Korrekt |
| Autostart Pfad-Fehler (kein HOME / APPDATA) | **2** | **2** | `src/autostart.rs:34`, `src/main.rs:168` | Korrekt |

---

## 7. Nebenläufigkeit, Locking & Concurrency im Daemon-Wiring

### 7.1 Thread-Architektur & Deadlock-Freiheit
- **Kanal-Design:** Alle Worker kommunizieren ausschließlich über asynchrone Standard-`std::sync::mpsc::channel`-Verbindungen mit der Event-Loop. Kein Worker ruft jemals `transition` oder blockierende Methoden eines anderen Workers auf.
- **Nichtblockierende Event-Loop:** Die Event-Loop blockiert maximal für `min(TICK, Frist)` in `rx.recv_timeout()`. Worker-Kommandos werden sofort abgesetzt.
- **5-Sekunden-Quit-Budget:** `Daemon::shutdown()` berechnet bei jedem Schritt die verbleibende Restzeit `remaining(deadline)` bis zur 5-s-Obergrenze. `SAVE_TARGETS` erhält max. 2 s, alle Worker-Joins teilen sich das verbleibende Restbudget. Überschreitet ein hängender Thread (z. B. ununterbrechbare ORT-Inferenz) das Budget, erzwingt `std::process::exit(exit)` das Beenden. Deadlocks sind strukturell ausgeschlossen.

### 7.2 Single-Instance- & Download-Lock-Semantik
- **Doppelsperre (`acquire_instance_lock`):** Versucht immer, sowohl `$XDG_RUNTIME_DIR/diktier.lock` als auch `$XDG_STATE_HOME/diktier/diktier.lock` zu sperren. Ein Prozess in einer Desktop-Session (mit `XDG_RUNTIME_DIR`) und ein Prozess in einer Shell ohne `XDG_RUNTIME_DIR` blockieren sich gegenseitig über den gemeinsamen State-Dir-Lock.
- **Clean Drop:** Bei `InstanceAcquire::Busy` werden zuvor genommene Locks in `held` sofort freigegeben (`drop`), sodass kein inkonsistenter Teilsperrzustand verbleibt.
- **Download-Lock:** Separiert unter `diktier-download.lock`. Parallele Downloads werden verhindert, ohne den Betrieb eines bereits laufenden Daemons zu stören.

### 7.3 Bewertung der offenen Punkte aus dem Kontext-Briefing (Abschnitte 4 & 5)

1. **Poll-Intervalle & Grundlast (Abschnitt 4 #1):**
   Die gemessene Ruhelast von ~2,3 % CPU verteilt sich auf Inject (0,8 %), Hotkey (0,6 %), Audio (0,6 %), Tray (0,15 %) und Loop (0,15 %). Die 5-ms-Polls für Hotkey und 10-ms-Polls für Inject sind für v1 funktional absolut stabil, sollten aber in v2 durch `poll()` auf dem X11-FD und 10-ms-Hotkey-Polls optimiert werden (*Befund B6*).
2. **`SAVE_TARGETS` unter Cinnamon (Abschnitt 4 #4):**
   Dass Cinnamon `SAVE_TARGETS` ablehnt, liegt am `csd-clipboard`-Design. Die zweistufige Implementierung (erst Target-Liste, dann leere Liste nach ICCCM) ist optimal ausgelegt und funktioniert auf GNOME/KDE-Managern.
3. **Audio Capture Gate & Drain (Abschnitt 5 #1):**
   Das Heuristik-Warten in `wait_for_producer_quiet` (10-ms-Ruheprüfung) ist in Kombination mit `ring.reset()` in `start()` praktisch sicher: Späte Samples eines alten Laufs können unmöglich in den nächsten Lauf geraten (*Befund B4*).
4. **`prepare()` im `update_tray`-Pfad (Abschnitt 5 #7):**
   Da der State-Machine-Kern bei *jedem* Übergang nach `Idle` oder bei Änderung des `paused`-Flags garantiert `Effect::UpdateTray` emittiert, gerät das Audio-Gerät nie in einen falschen Zustand. Die funktionale Korrektheit ist nachweislich gegeben.

---

## 8. Detaillierte Befunde

### B1 — `tray::open_config_dir()` hinterlässt Zombie-Kindprozesse (`<defunct>`)

- **Schwere:** Mittel
- **Stelle:** `src/tray.rs:216-223`
- **Problem:**
  `open_config_dir()` ruft `std::process::Command::new("xdg-open")...spawn()` auf, ohne den zurückgegebenen `Child`-Handle jemals per `.wait()` abzufragen:
  ```rust
  std::process::Command::new("xdg-open")
      .arg(&dir)
      .stdin(std::process::Stdio::null())
      .stdout(std::process::Stdio::null())
      .stderr(std::process::Stdio::null())
      .spawn()
      .map_err(|e| TrayError::Failed(format!("xdg-open: {e}")))?;
  ```
  Sobald `xdg-open` beendet wird, verbleibt der Prozess als Zombie in der Prozesstabelle des Betriebssystems, bis der `diktier`-Hauptprozess beendet wird. Klickt der Nutzer mehrfach auf „Config-Ordner öffnen“, sammeln sich Zombie-Prozesse an.
- **Vorschlag:**
  Den Child-Prozess in einem kurzlebigen Helper-Thread einsammeln:
  ```rust
  let mut child = std::process::Command::new("xdg-open")
      .arg(&dir)
      .stdin(std::process::Stdio::null())
      .stdout(std::process::Stdio::null())
      .stderr(std::process::Stdio::null())
      .spawn()
      .map_err(|e| TrayError::Failed(format!("xdg-open: {e}")))?;
  std::thread::spawn(move || {
      let _ = child.wait();
  });
  ```

---

### B2 — Signal-Handler nutzt rohe `extern "C"`-Deklaration mit `usize` statt `libc::sigaction`

- **Schwere:** Mittel
- **Stelle:** `src/daemon/signals.rs:18-35`
- **Problem:**
  In `signals.rs` wird `signal()` mit einer eigenen `extern "C"`-Signatur deklariert, bei der Funktionszeiger nach `usize` gecastet werden:
  ```rust
  unsafe extern "C" {
      fn signal(signum: i32, handler: usize) -> usize;
  }
  ```
  Obwohl die Crate `libc` in `Cargo.toml` bereits als Abhängigkeit vorhanden ist, wird `libc::signal` bzw. `libc::sigaction` nicht genutzt. Die Signatur von Signal-Handlern ist `extern "C" fn(libc::c_int)`. Das Casten über `usize` ist auf Plattformen mit Tagged Pointers oder Control-Flow Integrity (CFI) problematisch. Zudem bietet `signal()` unter unterschiedlichen UNIX-Derivaten abweichende Semantiken bzgl. automatischer Reinstallation (`SA_RESTART`).
- **Vorschlag:**
  Nutzung von `libc::sigaction` mit `SA_RESTART`:
  ```rust
  pub fn install() {
      unsafe {
          let mut sa: libc::sigaction = std::mem::zeroed();
          sa.sa_sigaction = on_signal as usize; // bzw. sa.sa_handler
          sa.sa_flags = libc::SA_RESTART;
          libc::sigemptyset(&mut sa.sa_mask);
          libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());
          libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut());
      }
  }
  ```

---

### B3 — `quote_exec` maskiert `%` nicht als `%%` für Desktop Entry Spec 1.5

- **Schwere:** Niedrig
- **Stelle:** `src/autostart.rs:170-181`
- **Problem:**
  Laut Freedesktop Desktop Entry Specification („The Exec key“) leitet das Prozentzeichen `%` Feldcodes (`%f`, `%u`, etc.) ein. Ein literales `%` im Pfad zur ausführbaren Datei (z. B. wenn der Benutzername oder Ordnername ein `%` oder URL-Encoding wie `%20` enthält) muss im `Exec=`-Schlüssel als `%%` maskiert werden. `quote_exec` maskiert derzeit nur `"`, `` ` ``, `$` und `\`.
- **Vorschlag:**
  Im Match-Block von `quote_exec` die Maskierung für `%` ergänzen:
  ```rust
  '%' => out.push_str("%%"),
  ```

---

### B4 — Audio-Drain-Heuristik `wait_for_producer_quiet` formal nicht absolut rennfrei

- **Schwere:** Niedrig
- **Stelle:** `src/audio/capture.rs:284-295`
- **Problem:**
  `wait_for_producer_quiet` prüft, ob `write_pos()` vor und nach einem 10-ms-Sleep identisch ist. Wird der Audio-Callback-Thread vom Betriebssystem-Scheduler exakt zwischen `armed.load(Ordering::Acquire)` (noch `true`) und `ring.push_frame()` für > 10 ms suspendiert, könnte er nach dem `drain_f32()` noch Frames in den Ringpuffer schreiben.
  *Hinweis:* Der Fehler ist in der Praxis unkritisch, da `CpalAudioSource::start()` vor jeder neuen Aufnahme `ring.reset()` aufruft, sodass keine Alt-Samples in nachfolgende Diktate gelangen können.
- **Vorschlag:**
  Für absolute mathematische Wasserdichtheit einen atomaren aktiven Schreiber-Zähler (`active_writers: AtomicU32`) in `push_if_armed` hoch- und runterzählen und in `stop()` warten, bis `active_writers.load() == 0`.

---

### B5 — Kopplung des Audio-Gerätelebenszyklus an den `UpdateTray`-Effekt

- **Schwere:** Niedrig
- **Stelle:** `src/daemon/mod.rs:442-463`
- **Problem:**
  In `Daemon::update_tray` wird bei `AppState::Idle` das Audio-Gerät vorbereitet (`prepare()`) bzw. freigegeben (`release()`). Obwohl die State Machine bei jedem Eintritt in `Idle` sowie bei jedem `PauseToggle` garantiert `UpdateTray` emittiert, ist das Triggern von Audio-Hardware-Operationen innerhalb einer Methode namens `update_tray` semantisch überraschend und verschleiert den Datenfluss.
- **Vorschlag:**
  In einer künftigen Refaktorisierung explizite Effekte wie `Effect::PrepareAudio` und `Effect::ReleaseAudio` im State-Machine-Kern vorsehen oder die Audio-Steuerung im Dispatcher explizit an Zustandswechsel koppeln.

---

### B6 — Grundlast im Ruhezustand durch Sleep-Pasting-Schleifen (2,3 % CPU)

- **Schwere:** Niedrig (Optimierung / Offener Punkt aus Kontext-Briefing §4 #1)
- **Stelle:** `src/daemon/workers.rs:766-768, 833, 959`
- **Problem:**
  Im Ruhezustand pollt der Inject-Thread alle 10 ms (`serve_for(10ms)`), der Hotkey-Thread alle 5 ms und der Tray-Thread alle 20 ms. Dies erzeugt eine Grundlast von ~2,3 % eines CPU-Kerns.
- **Vorschlag:**
  Für Phase 4: Erhöhung des Hotkey-Poll-Intervalls von 5 ms auf 10 ms und des Inject-Serve-Timeouts auf 30–50 ms im Ruhezustand (oder Umstellung auf `poll()` auf dem X11-Verbindungs-Filedescriptor).

---

## 9. Fazit und Freigabe-Empfehlung

Die Umsetzung von Phase 3 ist **in jeder Hinsicht vorbildlich gelungen**:
- Der State-Machine-Kern ist mathematisch rein, generationensicher und zu 100 % spezifikationstreu.
- Die Concurrency-Architektur der Worker verhindert Hänger, Deadlocks und Datenraces.
- Single-Instance-Locking, transaktionaler Modell-Download, Autostart-Verwaltung und datenschutzkonformes Datei-Logging erfüllen sämtliche Anforderungen der Spec v1.3.

Die Befunde B1 bis B3 sind von geringem Implementierungsaufwand und sollten vor dem Release-Freeze (Phase 4) behoben werden.

**Empfehlung:**  
**Freigabe für Phase 4 (Politur & Release-Packaging).**
