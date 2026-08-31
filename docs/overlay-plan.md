# Aufnahme-Overlay mit Mikrofonpegel (Plan, v3)

Stand: 2026-08-31, v3. v2 nach Plan-Review
([reviews/plan-overlay-sol.md](reviews/plan-overlay-sol.md) — Blocker 1
und alle Majors eingearbeitet, Minors 10–14 übernommen); v3 nach
Implementierungs-Review
([reviews/impl-overlay-sol.md](reviews/impl-overlay-sol.md)):
Generationen-Tap statt nacktem AtomicU32 (Befunde 2–4), DPI aus
`GetDpiForWindow` des eigenen Fensters statt `GetDpiForMonitor`
(Blocker 1), `WM_SETTINGCHANGE`/`SPI_SETWORKAREA` (Befund 6). Ziel: Während
der Aufnahme erscheint unten mittig ein kleines, randloses Overlay im
Stil des Omarchy-Voxtype-OSD
([omarchy-voxtype-osd](https://github.com/rmacy/omarchy-voxtype-osd)):
dunkle abgerundete Karte (~400×72 px Referenzmaß), Mikrofon-Glyphe,
**live scrollende Waveform-Historie** des Mikrofonpegels und ein
Pegelmeter mit Peak-Hold. Es erscheint bei Aufnahmebeginn, bleibt bis
zum Abschluss des Ausgabepfads stehen (Waveform läuft nach dem
Loslassen leer) und verschwindet bei `idle`.

**Spec-Status:** Verbindlich ist **SPEC v1.4** (2026-08-31): §2-Eintrag
„OSD/Waveform-Overlay" gestrichen, neu **§4.5 Aufnahme-Overlay** und
`[overlay]` in §8 (Sol-Blocker 1 damit ausgeräumt). Unverändert strikt:
**§4.2 (Fokusregel)** — das Overlay ist `WS_EX_NOACTIVATE`, wird nie
aktiviert, ruft keine Fokus-APIs und ist durchklickbar. Windows-only
(Plattform-Entscheidung 2026-08-27); Linux-Pfade bleiben unangetastet.
Der Plan setzt den Folgepaket-Eintrag „🔍 Aufnahme-Indikator" aus
[windows-plan.md](windows-plan.md) um.

## Ausgangslage (Befunde aus dem Code, HEAD `93811ba`)

- **Kein Broadcast nötig:** `Daemon::flush_presentation`
  (`src/daemon/mod.rs:382-414`) ist die designierte Stelle, an der der
  Kernzustand zustandsgetrieben auf Präsentations-/Geräte-Konsumenten
  verteilt wird (Tray-Update, Hotkey-Grab, `audio_intent`). Das Overlay
  hängt sich dort als weiterer Konsument ein — **kein neuer `Effect`,
  keine Änderung an `state.rs`** (die ~60 Tests auf exakte
  Effektreihenfolge bleiben unberührt).
- **Pegel gibt es heute nirgends:** Während `recording` liest niemand
  den SPSC-Ring; er wird erst in `CpalAudioSource::stop()` einmalig
  gedraint (`src/audio/capture.rs:407-455`). Ein nebenläufiges Peek aus
  einem anderen Thread wäre UB (`src/audio/spsc.rs:3-7`). Der einzige
  Ort mit Live-Samples ist der cpal-Callback `push_if_armed()`
  (`src/audio/capture.rs:298-311`) — Realtime-Vertrag: kein Lock, keine
  Allokation (`src/audio/capture.rs:288-297`).
- **Win32-Muster vorhanden:** Fensterklasse/WndProc/`GWLP_USERDATA`
  nach Vorbild `hotkey_dialog.rs:560-786`; Owner-Thread mit
  `PeekMessageW`-Pump + Command-Channel nach Vorbild
  `Win32Tray`/`TrayWorker` (`src/tray.rs:1030-1245`,
  `src/daemon/workers.rs:1089-1240`). `IconSet::make_icon`
  (`src/tray.rs:652-748`) baut bereits 32-bpp-BGRA-DIBs — dort ist
  Alpha aber nur 0/255; für die Kartenränder braucht das Overlay echtes
  Zwischenalpha mit Premultiplikation (Sol Major 8).
- **DPI:** keinerlei DPI-Awareness im Projekt (kein Manifest, kein
  HiDpi-Feature). Hotkey-Dialog rechnet hart in Pixeln,
  primärmonitor-only.
- **Chunk-Takt:** cpal-Buffergröße ist Gerätedefault (WASAPI Shared
  typisch ~10 ms). Der Pegel-Tap darf keine Annahme über die
  Chunk-Größe treffen.

## Leitentscheidungen

1. **Pegel-Tap im cpal-Callback, Übergabe per Atomic — der Ring bleibt
   tabu.** Gemessen wird der **ASR-Eingangspegel**, nicht der lauteste
   Rohkanal (Sol Minor 10): pro Frame derselbe arithmetische
   Kanalmittelwert wie `downmix_interleaved`, davon der Betragspeak
   über den Callback-Buffer. **Normierungs-Vertrag** (reine Funktion,
   Sol Major 2): Sample erst nach f32 wandeln (sample-weise, ohne
   Allokation; signed: `s as f32 / -(MIN as f32)`-Semantik statt
   `MIN.abs()`; unsigned: Offset-Binary um `2^(n-1)` mit demselben
   Nenner), dann `abs()`, **nicht-endliche Werte → 0**, auf `[0, 1]`
   clampen (`abs` kanonisiert `-0.0` mit). Publikation in ein
   **AtomicU64 mit Stream-Generation** (High-Word Generation, Low-Word
   f32-Bits des Peaks; für kanonische endliche Werte in `[0, 1]` ist
   die u32-Bitordnung ordnungserhaltend): `publish(gen, level)` als
   CAS-Loop, der abbricht, sobald die gespeicherte Generation nicht
   mehr die eigene ist — der 64-Bit-Vergleich schließt das
   Check-then-Act-Rennen eines In-flight-Callbacks über einen
   Generationswechsel hinweg (Impl-Review Befund 2). Der Renderer
   konsumiert per Generation-erhaltendem `fetch_and` (Peak-Bits
   nullen) — Peak-Hold zwischen zwei Frames, kein verpasster Transient
   bei 10-ms-Chunks vs. ~33-ms-Rendertakt. Alles `Ordering::Relaxed`.
   Einordnung: **lock-frei, nicht wait-frei** — akzeptiert, keine
   Allokation, kein Lock, O(n) über den ohnehin angefassten Buffer.
2. **Ein optionaler „LevelTap" (`Arc<{ state: AtomicU64, active: AtomicBool }>`),
   erzeugt im Daemon.** `CpalAudioSource::new` nimmt `Option<LevelTap>`;
   bei `None` (Overlay deaktiviert) wird die Pegelberechnung **pro
   Callback-Buffer** übersprungen (ein Branch pro Callback, nicht pro
   Sample) — der bisherige Pfad bleibt praktisch unverändert (Sol
   Major 4). Das `active`-Flag wird zusätzlich einmal pro Callback
   geprüft: Stirbt der Overlay-Thread später (Spawn ok, aber
   dauerhafter show/frame-Fehler), setzt er selbst `active = false` —
   der Realtime-Callback rechnet dann nicht mehr für einen toten
   Consumer (Impl-Review Befund 4). Der Arc überlebt
   Prepare/Release-Zyklen; die Waveform-**Historie** entsteht
   ausschließlich im Overlay-Thread.
   **Reset-Matrix und Race-Regel (Sol Major 3, Impl-Review Befunde
   2–3):** Publiziert wird nur innerhalb desselben Gate-geschützten
   Abschnitts wie die Ring-Writes (`CaptureGate`) — damit beweist
   `wait_idle()` auch das Ende der Tap-Publikation. Der Reset läuft
   ausschließlich auf dem Owner-Thread, in zwei Stärken: **Wo ein
   Stream entsteht oder verschwindet**, ist er ein Generationswechsel
   (`bump_generation`: neue Generation, Peak 0) — `open()` vor dem
   ersten falliblen Schritt, `release()`,
   `discard_after_stuck_producer()`, fehlgeschlagenes `play()`. **Wo
   derselbe Stream weiterlebt** (sein Callback hält seine Generation
   fest — ein Bump ließe ihn dauerhaft verstummen), genügt `clear()`
   (Peak 0, Generation bleibt): `start()` vor `arm()`, `stop()` nach
   `disarm()` + erfolgreichem `wait_idle()` — genau die zwei Stellen,
   an denen das Gate beweist, dass kein Callback publiziert.
   `err_fn` (Device-Lost) setzt **nur**
   `lost` — kein Clear aus dem Fehler-Callback, der stabile Reset
   folgt auf dem Owner-Thread. Ein In-flight-Callback der alten
   Generation kann nach dem Wechsel nie mehr publizieren
   (CAS-Generation, Leitentscheidung 1); Barrieren-Test mit Barriere
   **innerhalb** des betretenen `push_if_armed`-Abschnitts.
3. **Eigener Overlay-Thread (`diktier-overlay`), Fenster lebt nur
   dort.** `OverlayWorker` wie `TrayWorker`: mpsc-Commands
   (`Show`/`Hide`/`Shutdown`) + `PeekMessageW`-Pump, Loop-Takt 20 ms;
   sichtbar rendert jeder Durchlauf, unsichtbar schläft er.
   **Präzisierte Semantik (Sol Major 7):** Spawn mit
   Ready-/Fehler-Handshake (10-s-Frist wie Tray). **Fehlerpolitik:**
   Ein Overlay-Fehler (Spawn, Fensterbau, DIB) ist nie fatal —
   Log-Warnung, Overlay dauerhaft deaktiviert, Diktieren läuft weiter
   (SPEC §4.5). Pro Runde werden Commands **vollständig gedraint** und
   auf den letzten Sichtbarkeitszustand koalesziert (schnelles
   Show→Hide→Show zeigt nie ein veraltetes Fenster); `Shutdown` hat
   Vorrang. **Spawn-Reihenfolge:** OverlayWorker **vor** dem
   AudioWorker; nur bei erfolgreichem Ready-Handshake bekommt Audio
   `Some(tap)` — nach einem Spawn-Fehler rechnet der Callback gar
   nicht erst (Impl-Review Befund 4). Daemon-Shutdown stoppt das
   Overlay **vor** Tray und Audio; Join-Timeout wird wie bei den
   übrigen Workern als `stuck` in den bestehenden harten Exit
   aufgenommen. Fremde Threads fassen das `HWND` nie an
   (Leitentscheidung 2 aus Phase 5 gilt fort).
4. **Fensterstil und Rendering-Vertrag:** `WS_POPUP` +
   `WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT`,
   zusätzlich `WM_NCHITTEST → HTTRANSPARENT` (Klickdurchgriff nicht
   allein dem Ex-Stil überlassen, Sol Major 8). Anzeige ausschließlich
   über `UpdateLayeredWindow` mit
   `BLENDFUNCTION { SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA }`:
   **ein persistenter top-down 32-bpp-DIB + Memory-DC pro Größe,
   wiederverwendet über Frames** (kein Neuaufbau pro Frame); alle
   semitransparenten Pixel premultipliziert (B,G,R jeweils × A/255);
   beim Abbau altes GDI-Objekt zurückselektieren, dann `DeleteObject`/
   `DeleteDC`, alles auf dem Owner-Thread. Kein `WM_PAINT`-Pfad.
   `ShowWindow(SW_SHOWNOACTIVATE)` erst **nach** dem ersten
   erfolgreichen `UpdateLayeredWindow`. Kein `SetForegroundWindow`,
   kein `SetFocus`, nie (§4.2).
5. **Sichtbarkeit zustandsgetrieben aus `flush_presentation`:**
   `overlay_visible(runtime) = matches!(state, Recording{..} | Transcribing{..} | Injecting{..})`
   — **inklusive `Injecting`** (Sol Major 5: sonst verschwände die
   Karte vor Abschluss des Paste-/Copy-only-Pfads; Vertrag ist
   „sichtbar bis `idle`"). Deckt Release, Tray-Klick, 60-s-Cap,
   Pause-Discard und FatalError ab, weil der Abgleich zustands- und
   nicht ereignisgetrieben ist (Design „agy B5",
   `src/daemon/mod.rs:359-367`). **Quit ist davon getrennt** geregelt:
   `QuitRequested` läuft nicht über `overlay_visible`, sondern über
   den Worker-Shutdown (Leitentscheidung 3) — Fenster weg, bevor der
   Prozess endet. Während `Transcribing`/`Injecting` kommt kein Pegel
   mehr → Waveform läuft sichtbar leer (Omarchy-Verhalten).
6. **DPI nur per Thread, mit definiertem Bootstrap (Sol Major 6,
   korrigiert durch Impl-Review Blocker 1):**
   `SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)`
   als **allererstes** auf dem Overlay-Thread, vor jeder Fenster-/
   Monitor-API; Fehlschlag (NULL) bricht den Overlay-Aufbau nichtfatal
   ab (Feature `Win32_UI_HiDpi`). Die DPI des Zielmonitors kommt vom
   **eigenen Fenster**: das noch versteckte Overlay per
   `SetWindowPos(…, SWP_NOACTIVATE)` klein in die Work-Area des
   Zielmonitors schieben, dann `GetDpiForWindow(hwnd)` lesen (0 →
   nichtfataler Abbruch), damit Layout/DIB rechnen, präsentieren,
   erst dann zeigen. `GetDpiForMonitor` wird **nicht** verwendet
   (liefert im prozessweit DPI-unawaren Mixed-Mode-Prozess nicht
   verlässlich die effektive Monitor-DPI); `GetDpiForWindow` eines
   **fremden** Fensters ebenso wenig (hängt an dessen Awareness).
   Während der Sichtbarkeit werden `WM_DPICHANGED` (Monitor aus dem
   Vorschlag, Geometrie neu aus `card_rect`, DIB/Layout neu),
   `WM_DISPLAYCHANGE` (Monitor neu ermitteln, bei ungültigem Monitor
   auf den nächstgelegenen aktiven klemmen) und `WM_SETTINGCHANGE`
   mit `SPI_SETWORKAREA` (Taskleiste verschoben/ausgeblendet →
   Re-Layout, Impl-Review Befund 6) behandelt (Sol Major 9).
   Prozessweite PMv2-Awareness (Manifest) bleibt bewusst ausgeklammert
   — eigenes Folgepaket (`hotkey_dialog.rs` müsste erst DPI-fest
   werden).
7. **Positionierung beim Einblenden, dann eingefroren** (außer den
   Display-Events aus Leitentscheidung 6): Bei `Show`
   `GetForegroundWindow()` → `MonitorFromWindow(MONITOR_DEFAULTTONEAREST)`
   → `GetMonitorInfoW().rcWork`; Karte unten mittig in der Work-Area
   (Referenz 400×72 px, 72 px Bodenabstand, skaliert mit
   DPI-Faktor/96). Kein Nachführen bei Fokuswechsel während der
   Aufnahme (das ist ohnehin der copy_only-Fall). Kein Fenster im
   Fokus (`NULL`) → Primärmonitor. **Session-Lock/RDP bewusst ohne
   Sonderbehandlung:** Beim Sperren wechselt Windows auf den Secure
   Desktop (User-Fenster dort unsichtbar), die Aufnahme endet spätestens
   am 60-s-Cap (SPEC §4.4 „verlorenes Release"); ein Gate deckt den
   Fall ab, mehr nicht.
8. **Pegel-Skala in dB, zeitbasierter Abfall (Sol Minor 12).**
   Balkenhöhe = `clamp((dBFS + 50) / 50, 0, 1)` (Anzeigebereich
   −50..0 dBFS); Pegel ≤ 0 oder nicht-endlich → direkt Stille-Floor
   (nie `log10(0)`). Peak-Hold fällt mit ~20 dB/s, berechnet über das
   **gemessene** `elapsed` seit dem letzten Frame (nicht pro Frame
   fix — Rendertakt kann jittern). Nebeneffekt: Das bekannte
   „Jabra-Mute"-Problem (RMS 0,0007) wird sofort sichtbar — flache
   Linie trotz Sprechens = Mikro stumm/Pegel zu niedrig.
9. **Rendering handgemalt ins DIB, keine neue Grafik-Dependency.**
   Abgerundete Karte (AA-Kanten über echtes Zwischenalpha,
   premultipliziert), Waveform als vertikal zentrierte Balken
   (Breite/Lücke ~3/2 px @96dpi, rechts neu, nach links scrollend),
   darunter schmales Pegelmeter mit Peak-Hold-Marke, links einfache
   Mikrofon-Glyphe aus Primitiven (Kapsel + Bügel + Fuß). Kein
   GDI+/Direct2D/DirectWrite; kein Text auf der Karte in v1.
10. **Config minimal:** neue Sektion `[overlay]` mit einzigem Schlüssel
    `enabled = true` (Default an, SPEC §8). `enabled = false` →
    OverlayWorker wird nicht gespawnt **und** der LevelTap ist `None`
    (Leitentscheidung 2 — echter Null-Kosten-Pfad). Position, Größe,
    Farben hartkodiert, bis ein Bedarf nachgewiesen ist.

## Arbeitspakete

Drei sequenzielle Gates (Sol Minor 14): erst A grün, dann B, dann C.

### ✅ WP-O1 — Pegel-Quelle (`src/audio/`) — Gate A

- `capture.rs`: `Option<LevelTap>` in `CpalAudioSource::new`,
  durchgereicht bis in die `impl_build!`-Closures (samt
  Stream-Generation); in `push_if_armed()` (innerhalb des
  Gate-Abschnitts, nach `active`-Check) pro Frame Kanalmittelwert →
  Betragspeak → Normierungs-Vertrag → generations-geprüfter CAS-Publish
  (Leitentscheidung 1). Reset-Matrix und Stuck-Producer-Regel aus
  Leitentscheidung 2.
- Tests (ohne Win32/cpal):
  - Normierung aller neun Formate inkl. `i*::MIN` (kein
    `abs()`-Overflow), unsigned-Mittelpunkt `2^(n-1)`;
  - NaN, ±Inf, Werte außerhalb `[-1, 1]`, `-0.0`, Subnormals → 0 bzw.
    geclampt (Sol Minor 11: Property zur Bitordnung **nur über der
    Nach-Normierungs-Domäne** kanonischer endlicher `0.0..=1.0`,
    Sonderfälle als gezielte Einzeltests);
  - Publish/Take-Semantik (Peak gewinnt, Take räumt ab, fremde
    Generation prallt ab, `active = false` unterbindet Rechnung);
    Kanalmittelwert vs. lautester Kanal (Stereo-Gleichlauf,
    Gegenphase → ~0, einseitiges Signal);
  - Barrieren-Test: kein Publish nach `disarm()`+`wait_idle()`;
    In-flight-Nachzügler (Barriere **im** betretenen
    `push_if_armed`-Abschnitt) publiziert nicht in die neue
    Generation;
  - dB-Mapping inkl. 0, Subnormal, 1.0; Peak-Hold-Abfall über
    variierende `elapsed`-Werte und lange Pausen.
- **Gate A:** `cargo test` grün; `--capture-test`-Lauf zeigt plausible
  Peaks beim Sprechen, 0 bei Stille; Log-/Debug-Nachweis, dass der
  deaktivierte Pfad (`None`) keine Pegelberechnung ausführt.

### ✅ WP-O2 — Overlay-Fenster (`src/overlay.rs`, Modul `windows`) — Gate B

- Fensterklasse `DiktierOverlay` (tolerant gegen
  `ERROR_CLASS_ALREADY_EXISTS`, `owns_class`-Muster), Stil und
  Rendering-Vertrag aus Leitentscheidung 4, DPI-Bootstrap aus
  Leitentscheidung 6, Positionslogik aus Leitentscheidung 7.
- `OverlayWorker` (in `daemon/workers.rs` neben `TrayWorker`) mit der
  Semantik aus Leitentscheidung 3 (Handshake, Fehlerpolitik,
  Command-Koaleszenz, Shutdown-Vorrang). Historie leert sich bei
  `Hide`.
- Layout/Zeichnen als reine Funktionen über einem
  `&mut [u8]`-BGRA-Puffer (testbar ohne Fenster): Kartenmaske mit
  runden Ecken (premultipliziertes Zwischenalpha), Balkenlayout aus
  der Historie, Meter + Peak-Hold, Glyphe;
  `card_rect(work_area, dpi) -> RECT`.
- **Debug-CLI `--overlay-test`** (Muster `--hotkey-test`/
  `--inject-test`): zeigt die Karte ~15 s mit Live-Pegel vom
  Default-Gerät (oder synthetischem Sinus-Sweep als Fallback) — macht
  Gate B ohne Daemon-Wiring abnehmbar (Sol Minor 14).
- Cargo: Feature `Win32_UI_HiDpi` ergänzen, API→Feature-Tabelle in
  `Cargo.toml` fortschreiben; `MonitorFromWindow`/`GetMonitorInfoW`
  liegen in `Win32_Graphics_Gdi` (beim Build verifizieren);
  `GetDpiForMonitor` beachten (shcore); fehlende Einzelkonstanten als
  lokale `const` (Projektkonvention).
- Tests: `card_rect` (Primär-/Zweitmonitor, 96/144 dpi), Balkenlayout
  (Historie kürzer/länger als Kartenbreite), Maskenränder (Ecke
  transparent, Kante opak, Premultiplikations-Invariante
  B,G,R ≤ A je Pixel).
- **Gate B (manuell, via `--overlay-test`):** Karte unten mittig,
  scharf bei 100 % und 150 %, auf dem Monitor des fokussierten
  Fensters; **Tippen in Notepad läuft beim Einblenden ununterbrochen
  weiter** (§4.2-Kernprobe); Klick durch die Karte erreicht ein
  Fremdprozess-Fenster darunter (Notepad/Browser); `WM_DPICHANGED`/
  Monitorwechsel während sichtbar crasht nicht.

### ✅ WP-O3 — Config und Daemon-Verdrahtung — Gate C

- `config.rs`: `[overlay] enabled = true` an den fünf bekannten
  Stellen (DEFAULT_TOML, `OVERLAY_KEYS`, `OverlayConfig`+Default,
  `RawOverlay`+Feld in `RawConfig`, Match-Arm in
  `collect_unknown_keys`); nichts zu clampen.
- `daemon/mod.rs`: LevelTap nur bei `enabled` erzeugen, an
  `AudioWorker::spawn` und `OverlayWorker::spawn` reichen; in
  `flush_presentation` `overlay_visible(runtime)` auswerten und
  `Show`/`Hide` nur bei Wechsel senden. Shutdown-Reihenfolge: Overlay
  vor Tray/Audio, Join-Timeout → `stuck`-Pfad.
- README-Abschnitt; `docs/windows-plan.md`: Folgepaket-Eintrag
  „Aufnahme-Indikator" auf ✅ mit Verweis hierher.
- Tests: `overlay_visible` über **alle** `AppState`-Varianten (inkl.
  `Injecting`); Config-Roundtrip (fehlende Sektion → Default an,
  `enabled = false` greift, unbekannter Key in `[overlay]` warnt).
- **Gate C:** Abnahme unten.

## Abnahme (Gates, auf diesem Rechner)

1. `cargo build --release`, `cargo test`, `cargo clippy` grün
   (`target-dev`, falls der Daemon läuft).
2. F9 halten in Notepad: Karte erscheint unten mittig auf dem
   Notepad-Monitor, Tippen läuft ununterbrochen weiter (§4.2),
   Waveform bewegt sich beim Sprechen, Meter + Peak-Hold plausibel,
   Stille = flache Linie.
3. Loslassen: Karte bleibt durch `transcribing` **und** `injecting`
   (Waveform läuft leer), verschwindet bei `idle`; Text landet wie
   bisher am Cursor.
4. Ausstiegs-/Störmatrix (Sol Minor 13): Tray-Klick-Aufnahme,
   60-s-Cap, Pause während der Aufnahme (Discard) mit sofortigem
   Neustart, schneller Tray-Toggle (Show→Hide→Show), Capture-Fehler
   während `Recording` (Gerät im laufenden Diktat trennen →
   Device-Lost, danach Reopen), Inject-Fehler (erhöhtes
   Notepad → copy_only), Beenden während sichtbarer Karte — Karte
   verschwindet in allen Fällen. Zombie-Prüfung über Fenster-/
   Thread-Existenz nach regulärem Quit (kein Explorer-Neustart nötig —
   das bleibt Tray-Gate).
5. Klick durch die Karte hindurch erreicht das Fremdprozess-Fenster
   darunter.
6. DPI/Monitor: 100 % und 150 % scharf und richtig positioniert;
   Zweitmonitor: Karte folgt dem fokussierten Fenster; Skalierung
   ändern, während die Karte sichtbar ist (`WM_DPICHANGED`); einmal
   Desktop sperren/entsperren während einer Aufnahme (kein Crash,
   Zustand konsistent nach Cap).
7. `[overlay] enabled = false`: kein Fenster, kein Overlay-Thread,
   keine Pegelberechnung im Callback, Verhalten wie heute.

## Nicht in diesem Paket (bewusst)

- Prozessweite DPI-Awareness (Manifest, PMv2) inkl. DPI-fester
  Hotkey-Dialog — eigenes Folgepaket.
- Text/Status auf der Karte, Theme-/Farbkonfiguration, Positionswahl,
  Fade-Animationen (`AnimateWindow`), Klick-Interaktion (bewusst
  durchklickbar).
- Session-Lock-/RDP-Sonderbehandlung (Begründung Leitentscheidung 7);
  Pegelanzeige im Tray-Tooltip; Gerätewahl-UI.
- Linux-Pendant (Plattform-Entscheidung 2026-08-27).

## Umsetzung

Delegation an Opus als **ein Paket mit drei sequenziellen Gates**
(WP-O1 → O2 → O3; eine frische Session, Briefing = dieser Plan +
SPEC §4.2/§4.5/§5.2 + Sol-Review). Zweit-Review durch Sol
(`gpt-5.6-sol` via copilot, effort medium) nach
`docs/reviews/impl-overlay-sol.md`; Orchestrator fährt die
Abnahme-Gates auf diesem Rechner selbst nach. Commit nach grünen
Gates; kein Push ohne Freigabe.
