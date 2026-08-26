# Review: Diktier Phase 2a+2b — X11-Inject + Capture-Pipeline

Reviewer: **agy**  
Datum: 2026-08-26  
Verbindliche Referenz: `docs/SPEC.md` (v1.3), `AGENTS.md`  
Geprüfter Scope: Uncommitteter Diff gegen HEAD (`Cargo.toml`, `Cargo.lock`, `src/engine.rs`, `src/hotkey.rs`, `src/main.rs`) sowie neue Module `src/audio/` (`mod.rs`, `capture.rs`, `convert.rs`, `resample.rs`, `spsc.rs`) und `src/inject/` (`mod.rs`, `protocol.rs`, `linux.rs`, `fake.rs`).

---

## 1. Kurzfazit

Die Implementierung der Phasen 2a (X11-Inject) und 2b (Capture-Pipeline) ist von **herausragender architektonischer und technischer Qualität**. Die Trennung zwischen plattformneutralem Protokoll-State (`protocol.rs`), Fake-Testumgebung (`fake.rs`) und nativem X11-Backend (`linux.rs`) ist vorbildlich umgesetzt. Die Audio-Pipeline erfüllt sämtliche harten Realtime-Vorgaben der Spec v1.3.

### Positive Ergebnisse & Gate-Erfüllung
- **Audio-Callback & Realtime-Reinheit (§5, §6.4):** Der `cpal`-Input-Callback ist absolut lock-free, allokationsfrei und syscall-frei. Es werden ausschließlich rohe Frames in den vorallokierten Ringpuffer geschoben. Resampling, Konvertierung und Downmix laufen vollständig auf dem Worker.
- **rubato-Numerik & Flush (§6.4):** Die Resampling-Pipeline mit `rubato::FftFixedIn` führt bei Aufnahme-Ende einen sauberen Flush (`process_partial(None, None)`) durch. Die Filterlatenz (`output_delay`) wird am Audioanfang präzise kompensiert und das Signal exakt auf die Soll-Länge beschnitten (Längenabweichung < 1 %, RMS-Erhaltung im Sollbereich).
- **Spec-§7-Restore-Zustandsmaschine:** Alle 8 Punkte von §7.1 sind lückenlos implementiert: Snapshotting, Verzicht auf Restore bei Nicht-Text (`NoPromise`), Bestätigung durch echten Daten-Read (`SelectionRequest` auf `UTF8_STRING`/`STRING`/`TEXT`, während `TARGETS` ignoriert wird), 5-s-Read-Timeout (`NoReadTimeout`), Mindestwartezeit `restore_clipboard_delay_ms` ohne Blockade der Event-Loop, sowie Weiterbedienen nach Restore (`serve_restored_until_read`).
- **Modifier-Regel & Fokus-Dreifachprüfung (§7.1, §7.3):** Störende Modifier werden vor dem Paste gelöst und nur wiederhergestellt, wenn sie zum Restore-Zeitpunkt noch physisch gehalten werden (`XQueryKeymap`). Die Dreifachprüfung `start_window_id == target_window_id == current` schützt vor versehentlichem Paste nach Fensterwechsel und fällt sauber auf `copy_only` zurück.
- **PTT-Schlucken & Entprellung (§4.4):** `XGrabKey` mit `xkb`-Detectable-Auto-Repeat und `Debounce`-Filter schluckt den Hotkey global und liefert exakt 1 logisches `Press` und 1 logisches `Release`.
- **Live-Abnahme-Regression:** Alle 74 Unit-/Integrationstests sowie der vollständige `stt_smoke_fixtures`-Testlauf mit dem 0.6B-Modell laufen fehlerfrei durch; `cargo clippy --all-targets` meldet 0 Warnungen.

### Wesentliche Kritikpunkte & Handlungsbedarf
1. **`output.leading_space` unberücksichtigt (B1):** Das Feld `leading_space` aus `OutputConfig` (Default `true`, Spec §7.4) wird im Inject-Modul und beim Paste-Vorgang ignoriert.
2. **`OverwriteSpsc` Concurrency-Trap (B2):** In `OverwriteSpsc` schreiben sowohl Producer (bei Overflow) als auch Consumer (`pop`) unkoordiniert in `self.read`. Für echtes Streaming-Lesen während der Aufnahme entstünde eine Race Condition.
3. **Ganzheitlicher RMS-Gate bei Langaufnahmen (B3):** Die Berechnung von `rms_f32` über den Gesamtpuffer kann leise Sprache am Anfang/Ende einer langen Aufnahme mit viel Stille rechnerisch unter die Schwelle `0.0075` drücken.
4. **Testlücken §13 (B4):** Für `i32_to_f32`, `u16_to_f32`, `u8_to_f32` in `convert.rs` und den 16-kHz-Passthrough in `resample.rs` fehlen Unit-Tests.

Es gibt **keine Blocker**. Die Implementierung ist bereit für den Übergang zu Phase 3 (Daemon + Tray + Autostart), sobald Befund B1 behoben ist.

---

## 2. Punkt-für-Punkt-Prüfung: Spec §7 (Text am Cursor)

| Spec-Abschnitt / Anforderung | Status | Implementierung im Code | Bewertung |
|---|---|---|---|
| **§7.1 Punkt 1:** Snapshot Unicode-Text | **ERFÜLLT** | `src/inject/linux.rs:475-500`, `src/inject/protocol.rs:284` | Versucht `UTF8_STRING`, danach `STRING`. Eigene Ownership (`self.we_own`) liefert direkt `self.serve`. `owner == 0` liefert `Text("")`. |
| **§7.1 Punkt 2:** Nicht-Text-Snapshot → kein Restore | **ERFÜLLT** | `src/inject/linux.rs:498`, `src/inject/protocol.rs:92, 137` | Scheitert Text-Konvertierung, wird `ClipboardSnapshot::NonText` gesetzt. `RestoreSession::decide()` liefert `NoPromise`, Transkript verbleibt im Clipboard. |
| **§7.1 Punkt 3:** Transkript setzen & eigene Ownership merken | **ERFÜLLT** | `src/inject/protocol.rs:290`, `src/inject/linux.rs:501-521` | `become_owner()` ruft `set_selection_owner()`, verifiziert via `get_selection_owner()`, setzt `we_own = true`. |
| **§7.1 Punkt 4:** Paste-Shortcut senden (§7.2) | **ERFÜLLT** | `src/inject/protocol.rs:291, 385-406` | `send_paste_shortcut()` löst störende Modifier, sendet Chord via XTest, stellt physische Modifier wieder her. |
| **§7.1 Punkt 5:** Restore nur bei unveränderter Ownership | **ERFÜLLT** | `src/inject/protocol.rs:139, 295`, `src/inject/linux.rs:196-200` | `SelectionClear` setzt `we_own = false`, `session.note_foreign()`. Fremde Übernahme verhindert Restore dauerhaft (`ForeignOwner`). |
| **§7.1 Punkt 6:** Mindestwartezeit ohne X11-Blockade | **ERFÜLLT** | `src/inject/protocol.rs:355-383`, `src/inject/linux.rs:594-607` | Kein `thread::sleep` über die Wartezeit. `pump()` pollt `poll_for_event()` in 5-ms-Slices und beantwortet `SelectionRequest` verzögerungsfrei. |
| **§7.1 Punkt 7:** Bestätigter Daten-Read & 5-s-Timeout | **ERFÜLLT** | `src/inject/linux.rs:225-274`, `src/inject/protocol.rs:142-147` | `TARGETS` antwortet mit Target-Liste, zählt **nicht** als Read. Nur `UTF8_STRING`/`STRING`/`TEXT` zählt als Read (`reads += 1`). Ohne Read nach 5 s → `NoReadTimeout`. |
| **§7.1 Punkt 8:** Selection nach Restore weiterbedienen | **ERFÜLLT** | `src/inject/protocol.rs:298, 334-353`, `src/main.rs:258-264` | `set_serve_text(old)` aktualisiert Inhalt, Diktier bleibt Owner. `serve_restored_until_read` puffert für Cinnamon `csd-clipboard`. |
| **§7.1 Abs. 2:** Modifier lösen & bedingt physisch restaurieren | **ERFÜLLT** | `src/inject/protocol.rs:218-243, 389-405`, `src/inject/linux.rs:547-592` | `disturbing_modifiers()` ermittelt Konflikt-Tasten. `XQueryKeymap` prüft Zustand vor und nach Chord. Synthetisches Down erfolgt **nur**, wenn Taste physisch noch gehalten ist. |
| **§7.2:** `paste_shortcut = "auto"` Erkennung | **ERFÜLLT** | `src/inject/protocol.rs:172-216` | `VTE_NAMES` (`gnome-terminal`, `xfce4-terminal`, `tilix`, `alacritty`, `kitty`, `ghostty`) → `CtrlShiftV`; `XTERM_NAMES` (`xterm`, `uxterm`) → `ShiftInsert`; Rest → `CtrlV`. |
| **§7.3:** Fokus-Dreifachprüfung (`start == target == current`) | **ERFÜLLT** | `src/inject/protocol.rs:156-170, 268-274`, `src/inject/linux.rs:162-185` | `_NET_ACTIVE_WINDOW` wird bei Start, Ende und vor Paste geprüft. Bei Ungleichheit oder `None` erfolgt `copy_only` mit präziser `CopyOnlyReason`. |
| **§7.4:** `output.leading_space` (Default `true`) | **ABWEICHUNG** | `src/config.rs:188-195` | Feld wird geparst, aber in `inject_paste()` bzw. `OutputSink::paste()` ignoriert (**Befund B1**). |

---

## 3. Numerik & Audio-Pipeline (§5, §6.4)

### 3.1 cpal-Callback & Realtime-Sicherheit
In `src/audio/capture.rs:189-287` wird für jeden unterstützten Sample-Typ (`I16`, `U16`, `I32`, `F32`) ein dedizierter Stream gebaut:
```rust
move |data: &[f32], _| {
    for frame in data.chunks_exact(ch) {
        prod.push_frame(frame);
    }
}
```
- **Keine Heap-Allokationen:** Der Puffer ist vor Beginn der Aufnahme für `max_duration_secs + 2` Sekunden vollständig allokiert (`Box<[UnsafeCell<T>]>`).
- **Keine Mutexe/Locks:** Es kommen ausschließlich Atomics (`AtomicUsize`, `AtomicU64`) mit `Relaxed`/`Acquire`/`Release` zum Einsatz.
- **Keine Syscalls / I/O:** Fehler im Callback werden über `lost.store(true, Ordering::Release)` rein atomar an den Hauptthread signalisiert. `Xrun`-Events werden bewusst gefiltert und führen nicht zum Stream-Abbruch.

### 3.2 Lock-freier Ringpuffer & Stereo-Frame-Alignment (`spsc.rs`)
In `src/audio/spsc.rs`:
- `capacity_samples` ist immer ein ganzzahliges Vielfaches der Kanalanzahl (`cap = frames.saturating_mul(channels)`).
- `push_frame` operiert strikt auf Frame-Ebene (`frame.chunks_exact(channels)`).
- Bei Pufferüberlauf (`used + n > self.cap`) wird der `read`-Zeiger exakt um `n = channels` inkrementiert. Dadurch bleibt die Frame-Ausrichtung bei 2-Kanal-/Stereo-Aufnahmen auch im Overflow-Fall absolut gewahrt (`stereo_overflow_keeps_frame_alignment`).
- *Einschränkung:* Siehe **Befund B2** bzgl. Schreibkonflikt auf `read` bei parallelem `pop`.

### 3.3 Kanal-Mittelung & Format-Konvertierungen (`convert.rs`)
- **Skalierungsgenauigkeit:** Integer-Typen werden exakt auf das Einheitsintervall `[-1.0, 1.0]` abgebildet:
  - `i16`: `sample as f32 / 32768.0` (Bereich: `-1.0 ..= +0.9999695`)
  - `i32`: `sample as f32 / 2147483648.0` (Bereich: `-1.0 ..= +0.99999994`)
  - `u16`: `(sample as f32 - 32768.0) / 32768.0`
- **Downmix:** `downmix_interleaved` berechnet das arithmetische Mittel über alle Kanäle je Frame. Bei Monosignalen (`channels == 1`) wird der Vektor ohne zusätzliche Berechnung direkt übernommen.

### 3.4 rubato-Resampling: Längenbilanz, Flush & Delay-Kompensation (`resample.rs`)
Die Nutzung von `rubato::FftFixedIn<f32>` (`resample.rs:10-91`) ist mathematisch sauber gelöst:
1. **Verarbeitungsblock:** `CHUNK_IN = 1024`, `SUB_CHUNKS = 2`.
2. **Resteverarbeitung:** Restsamples `< needed` werden über `process_partial(Some(&[leftover]), None)` verarbeitet.
3. **Flush:** Der interne Zustand wird mit `process_partial::<&[f32]>(None, None)` vollständig geleert. Kein Sprachrest am Aufnahmeende geht verloren (`resample_flush_keeps_tail`).
4. **Latenz-Kompensation:** FFT-basierte Resampler erzeugen durch ihre Impulsantwort eine Anfangslatenz von `output_delay()` Samples. `process_all` schneidet diesen Einschwingvorgang (`extra.min(delay)`) am Pufferanfang ab und stutzt das Signal exakt auf `expected_len`.
   - Bei 48 kHz (1 s, 440 Hz Sinus): 16.000 Samples Ausgabe (Fehler 0,0 %, RMS-Verhältnis 1,00).
   - Bei 44,1 kHz (1 s, 440 Hz Sinus): 16.000 Samples Ausgabe (Fehler 0,0 %, RMS-Verhältnis 0,999).

### 3.5 RMS-Silence-Gate (`engine.rs`)
In `engine.rs:22-37, 77-88`:
- `RMS_SILENCE_THRESHOLD = 0.0075` (~ -42,5 dBFS).
- `stille.wav` (RMS 0,0000) und `rauschen.wav` (RMS 0,0051) liegen sicher unter der Schwelle und rufen die Engine nicht auf.
- `alltag.wav` (RMS 0,0215) liegt sicher darüber.
- *Anmerkung:* Siehe **Befund B3** bzgl. Pufferlänge und Langaufnahmen.

---

## 4. X11-Protokoll, Hotkey & Thread-Hygiene (§4.4, §7)

### 4.1 X11-Selection-Protokoll & INCR-Transfers
In `src/inject/linux.rs`:
- **Große Snapshots:** Beim Lesen fremder Zwischenablagen (`read_property`) wird `INCR`-Chunking unterstützt (`linux.rs:372-418`). Chunks werden durch Löschen der Property quittiert, mit einer harten 1-MB-Obergrenze und einem 1-s-Sicherheits-Deadline-Timeout.
- **Eigene Bereitstellung:** Transkripte (< 60 s Audio entsprechen max. wenigen Kilobytes Text) werden in einer atomaren `change_property8`-Operation übertragen.
- **Legacy-Clients:** `SelectionRequest` mit `property == 0` wird nach ICCCM 2.0 konform auf `ev.target` umgebogen (`linux.rs:220-224`).

### 4.2 Hotkey-Swallowing, Debouncing & Backend-Lifecycle (`hotkey.rs`)
In `src/hotkey.rs`:
- `X11GrabKeyBackend` registriert `XGrabKey` mit `GrabMode::ASYNC` für alle Lock-Mask-Kombinationen (None, NumLock, CapsLock, NumLock+CapsLock). Der Tastendruck wird vom X-Server geschluckt und erreicht fokussierte Zielanwendungen nicht.
- `xkb_per_client_flags` aktiviert `DETECTABLE_AUTO_REPEAT`.
- `Debounce` filtert wiederholte Key-Events auf exakt 1 `Press` und 1 `Release`.
- `X11GrabKeyBackend` startet einen benannten Thread (`diktier-xgrab`). Die Beendigung erfolgt über einen `GrabCmd::Shutdown`-Kanal mit anschließendem `join()`.

### 4.3 Thread-Beendigung & Joins
- Alle neu eingeführten Threads (`diktier-xgrab` im Hotkey-Modul, cpal-Audio-Stream) besitzen saubere `Drop`-Implementierungen und blockieren weder beim normalen Beenden noch im Fehlerfall.

### 4.4 Portabilität & `cfg`-Hygiene
- Linux-spezifische Crates (`x11rb`, `global-hotkey`) sind in `Cargo.toml` sauber unter `[target.'cfg(target_os = "linux")'.dependencies]` isoliert.
- `PlatformSink` ist für Linux als `X11OutputSink` und für Windows als `StubOutputSink` typisiert.
- `new_backend()` stellt für Windows den `StubHotkeyBackend` bereit.

---

## 5. Detaillierte Befunde

### B1 — `output.leading_space` (Spec §7.4) im Inject-Pfad nicht implementiert

- **Schwere:** Hoch
- **Stelle:** `src/inject/protocol.rs:262-320`, `src/config.rs:188-195`
- **Problem:**
  Spec §7.4 legt fest:
  - *„`output.leading_space` Default `true` (Diktat in laufenden Satz).“*
  Das Konfigurationsfeld `leading_space` ist in `OutputConfig` vorhanden, wird jedoch an keiner Stelle im Inject-Modul oder in `main.rs` ausgewertet. `inject_paste(host, text, ctx, output)` erhält zwar die Referenz auf `&OutputConfig`, verwendet aber ausschließlich `paste_shortcut`, `restore_clipboard` und `restore_clipboard_delay_ms`.
  Wird ein Text transkribiert, wird er immer ohne vorangestelltes Leerzeichen eingefügt, selbst wenn `leading_space = true` konfiguriert ist.
- **Vorschlag:**
  In `inject_paste` (oder vor der Übergabe an `host.become_owner`) den Text entsprechend konditionieren:
  ```rust
  let formatted = if output.leading_space && !text.is_empty() && !text.starts_with(' ') {
      format!(" {text}")
  } else {
      text.to_string()
  };
  host.become_owner(formatted)?;
  ```

---

### B2 — `OverwriteSpsc`: Potenzielle Race Condition auf `read` bei gleichzeitigem Producer-Overflow und Consumer-`pop`

- **Schwere:** Mittel
- **Stelle:** `src/audio/spsc.rs:66, 86`
- **Problem:**
  In `OverwriteSpsc` wird `self.read` von zwei Seiten beschrieben:
  1. `push_frame` (Producer im Audio-Callback) schreibt bei Pufferüberlauf:
     ```rust
     self.read.store(r.wrapping_add(n), Ordering::Release);
     ```
  2. `pop` (Consumer auf dem Worker-Thread) schreibt beim Lesen:
     ```rust
     self.read.store(r.wrapping_add(1), Ordering::Release);
     ```
  Greifen Producer und Consumer parallel zu und tritt ein Überlauf auf, kann ein zeitgleiches `pop()` das Weiterschalten des `read`-Zeigers des Producers mit einem veralteten Wert überschreiben (`r.wrapping_add(1)` überschreibt `r.wrapping_add(n)`). Dadurch springt der Lesezeiger zurück, was zu Dateninkonsistenzen führt.
  *Hinweis:* Im aktuellen Code ruft `CpalAudioSource::stop()` die Methode `drain()` erst nach `stream.pause()` auf, sodass der Fehler in der aktuellen Einbindung nicht getriggert wird. Als generische öffentliche Datenstruktur mit `pop()` und `push_frame()` ist die Schnittstelle jedoch thread-unsicher gegen gleichzeitiges Streaming.
- **Vorschlag:**
  Entweder:
  1. Im Modulkommentar dokumentieren, dass `OverwriteSpsc` ein phasengetrennter Puffer ist (`pop`/`drain` nur bei pausiertem Producer aufrufen, `pop()` ggf. als `pub(crate)` einschränken),
  2. Oder für echte Streaming-Unterstützung `compare_exchange` für das Inkrementieren von `read` im Producer einsetzen.

---

### B3 — Ganzheitlicher RMS-Gate (`rms_f32`) kann leise Sprache bei langen Aufnahmen mit viel Stille fälschlicherweise verwerfen

- **Schwere:** Mittel
- **Stelle:** `src/engine.rs:25-37, 84-86`, `src/main.rs:170, 402`
- **Problem:**
  `RMS_SILENCE_THRESHOLD` ist auf `0.0075` (~ -42,5 dBFS) festgelegt.
  `rms_f32` berechnet den Effektivwert über das gesamte Audiosignal:
  $$\text{RMS} = \sqrt{\frac{1}{N} \sum_{i=1}^N x_i^2}$$
  Hält ein Nutzer den PTT-Hotkey für 25 Sekunden, spricht jedoch nur für 2 Sekunden leise (RMS ~0,020) und pausiert 23 Sekunden bei Raumstille (RMS ~0,001), ergibt sich ein Gesamt-RMS von:
  $$\sqrt{\frac{2 \cdot 0{,}02^2 + 23 \cdot 0{,}001^2}{25}} \approx \sqrt{\frac{0{,}000823}{25}} \approx 0{,}00574 < 0{,}0075$$
  Die gesamte Aufnahme wird als Stille gewertet und verworfen, obwohl verständliche Sprache enthalten war.
- **Vorschlag:**
  Entweder:
  1. Den Schwellwert auf `0.001` (-60 dBFS) absenken (dies filtert weiterhin digitale Null-Samples und synthetische Stille sicher aus, blockiert aber keine Sprachaufnahmen),
  2. Oder die RMS-Prüfung fensterbasiert durchführen (z. B. `max(rms_windows_250ms) >= RMS_SILENCE_THRESHOLD`).

---

### B4 — Testlücken (§13): Fehlende Unit-Tests für `convert.rs` (I32/U16/U8) und `resample.rs` (16-kHz-Passthrough / Fehlerpfade)

- **Schwere:** Mittel
- **Stelle:** `src/audio/convert.rs:46-83`, `src/audio/resample.rs`
- **Problem:**
  1. In `src/audio/convert.rs` wird nur `i16_to_f32` getestet. Für die produktiv genutzten Konvertierungen `i32_to_f32`, `u16_to_f32` und `u8_to_f32` fehlen Unit-Tests (Prüfung der Randwerte `MIN`/`MAX`, Nullpunkt bei vorzeichenlosen Typen).
  2. In `src/audio/resample.rs` fehlt ein Test für den Fall `in_rate == ENGINE_RATE` (16-kHz-Passthrough ohne Resampler-Allokation).
  3. Der Fehlerpfad für `in_rate == 0` (`AudioError::Failed("Eingangsrate 0")`) ist ungetestet.
- **Vorschlag:**
  Unit-Tests in `convert.rs` und `resample.rs` ergänzen.

---

### B5 — Code-Duplikation der Audio-Längen- und RMS-Prüfung in `main.rs`

- **Schwere:** Niedrig
- **Stelle:** `src/main.rs:165-173, 397-405`, `src/engine.rs:81-87`
- **Problem:**
  Sowohl in `transcribe_wav` als auch in `record_test` werden `pcm.len() < MIN_SAMPLES_16KHZ` und `rms < RMS_SILENCE_THRESHOLD` redundant vorab geprüft, um das Laden des Modells zu überspringen. Dieselbe Logik ist bereits in `engine::transcribe_pcm` enthalten.
- **Vorschlag:**
  In `engine.rs` eine Hilfsfunktion `pub fn is_silence_or_short(pcm: &[f32]) -> bool` definieren und in `main.rs` wiederverwenden.

---

### B6 — Inkonsistente OS-Gating-Prädikate (`unix` vs. `target_os = "linux"`)

- **Schwere:** Niedrig
- **Stelle:** `src/download.rs:72, 85`, `src/config.rs:20, 29`
- **Problem:**
  Während `hotkey.rs`, `inject/mod.rs` und `engine.rs` konsistent `#[cfg(target_os = "linux")]` verwenden, nutzen `download.rs` und `config.rs` noch `#[cfg(unix)]`. Da Diktier laut Spec §2 ausschließlich Linux und Windows unterstützt, sollte das Gating einheitlich sein.
- **Vorschlag:**
  `#[cfg(unix)]` durch `#[cfg(target_os = "linux")]` ersetzen.

---

## 6. Fazit und Freigabe-Empfehlung

Die Phasen 2a und 2b sind **hochgradig überzeugend implementiert**. Die X11-Inject-Zustandsmaschine erfüllt alle 8 Restore-Punkte der Spec §7 byte- und ereignisgenau. Die Audio-Pipeline genügt strengsten Realtime-Anforderungen und garantiert Phasen- und Pegeltreue beim Resampling.

**Empfohlene Schritte vor Beginn von Phase 3:**
1. **B1 beheben:** `output.leading_space` in `inject_paste()` berücksichtigen.
2. **B3 adressieren:** RMS-Silence-Gate für Langaufnahmen robust auslegen (Schwellwert absenken oder fensterbasiert mitteln).
3. **B4 schließen:** Unit-Tests für `convert.rs` und 16-kHz-Passthrough ergänzen.
