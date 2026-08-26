# Review: Diktier Spec v1.1 (Kreuz-Review)

Review-Stand: 2026-08-26. Geprüft wurde `docs/SPEC.md` (Fassung v1.1) unter Einbeziehung des Vor-Reviews `docs/reviews/spec-codex.md` und der Vorab-Entscheidungen in §17.

---

## 1. Kurzfazit

- Die Spec v1.1 ist im Kern **weitgehend tragfähig und deutlich geschärft**, aber **noch nicht frei von technischen Fallstricken**.
- Die von Codex eingeforderten Härtungen (Golden-Set-Hashes, Falsifizierbarkeit der Gates, Plaintext-Beschränkung beim Clipboard, Fokusprüfung, Ausschluss von Wayland und Windows-Elevation) wurden sinnvoll integriert und verhindern ein Ausufern des v1-Scopes.
- **Was Codex übersehen hat:**
  1. **Single-Instance-Kollision:** Der Daemon-Lock sperrt CLI-Subcommands wie `--install-autostart` und `--remove-autostart` aus, wenn Diktier bereits läuft.
  2. **Unvollständiger Fallback:** `transcribe-rs` benötigt zwingend `nemo128.onnx`, das im Golden Set (§6.3) und in `models.toml` fehlt – der definierte Fallback-Pfad ist damit nicht lauffähig.
  3. **Win32-Hook vs. Tray-Eventloop:** `WH_KEYBOARD_LL` erfordert zwingend eine eigene Win32-Message-Pump, da `betrayer` den Hook bei modalen Menüs aushängen würde (`LowLevelHooksTimeout`).
  4. **X11-Selection-Deadlock:** Ein synchroner Sleep im Clipboard-Restore blockiert das Beantworten von `SelectionRequest`-Events auf X11.
  5. **Audio Real-Time Safety:** `cpal`-Callbacks dürfen kein `rubato`-Resampling oder Heap-Allokationen direkt im Audio-Thread ausführen.
- **Was Codex überzogen hat:** Die Forderung nach einer künstlichen WER-Toleranz (`+0,05`) bei identischen ONNX-INT8-Modellen sowie überkomplexe Generationen-Logik für das Clipboard auf X11.
- Nach Behebung der untenstehenden Blocker und Hoch-Befunde kann Phase 1 unmittelbar gestartet werden.

---

## 2. Befunde

### B1 — CLI-Subcommands (`--install-autostart`, etc.) kollidieren mit dem Single-Instance Daemon-Lock

- **Schwere:** Blocker
- **Stelle:** §5.3 (Single-Instance), §9 (Autostart und CLI), §12 Phase 3
- **Problem:** §5.3 legt fest, dass ein zweiter Prozessstart via Named Mutex (Windows) bzw. `flock` (Linux) erkannt wird, eine Notify-Meldung an den Tray sendet und mit Exitcode 0 beendet wird. Wenn diese Prüfung vor dem Parsen der CLI-Argumente stattfindet, schlagen Wartungsbefehle wie `diktier --install-autostart` oder `diktier --remove-autostart` stillschweigend fehl, wenn Diktier bereits im Hintergrund als Daemon läuft.
- **Vorschlag:** In §5.3 und §9 klarstellen: „CLI-Befehle (`--install-autostart`, `--remove-autostart`, `--help`, `--version`) laufen **vor** der Single-Instance-Prüfung und fordern weder Mutex noch `flock` an. Nur der Daemon-Modus (`diktier` bzw. `diktier --foreground`) beansprucht die Single-Instance-Sperre.“

---

### B2 — Fallback-Engine `transcribe-rs` erfordert Artefakt (`nemo128.onnx`), das im Golden Set fehlt

- **Schwere:** Blocker
- **Stelle:** §6.1 (Runtime), §6.3 (Artefakte), §12 Phase 1 (STT-Spike)
- **Problem:** §6.1 definiert: Scheitert `parakeet-rs`, wird `transcribe-rs` mit zusätzlichem `nemo128.onnx` durch dasselbe Gate geschickt. §6.3 und `models.toml` definieren jedoch nur die vier Dateien aus Voxtype (Encoder, Decoder-Joint, Vocab, Config). Weder Hash, Dateigröße noch Download-Quelle für `nemo128.onnx` sind spezifiziert. Tritt der Fallback ein, scheitert Phase 1 sofort an fehlenden Artefaktspezifikationen.
- **Vorschlag:** In §6.1 und §6.3 präzisieren: Entweder wird `nemo128.onnx` (SHA-256, Bytes, Hugging-Face-URL) als optionales Fallback-Artefakt in die Spec und `models.toml` aufgenommen, oder die Spec legt fest: „Phase 1 evaluiert primär `parakeet-rs`. Scheitert dieses, ist vor dem Start des `transcribe-rs`-Spikes die Bereitstellung des passenden `nemo128.onnx`-Artefakts in `models.toml` durch den Orchestrator nachzupflegen.“

---

### H1 — Windows Low-Level-Hook (`WH_KEYBOARD_LL`) erfordert dedizierten Message-Pump-Thread

- **Schwere:** Hoch
- **Stelle:** §3 (Plattformtabelle), §4.3 (Tray), §4.4 (Default-Hotkey), §5 (Architektur)
- **Problem:** Ein Windows-Low-Level-Tastaturhook (`WH_KEYBOARD_LL` via `SetWindowsHookExW`) leitet Tastenanschläge synchron in den Thread weiter, der den Hook installiert hat, und benötigt eine aktive Win32-Message-Loop (`GetMessage` / `DispatchMessage`). Läuft der Hook auf dem Thread von `betrayer` (Tray), wird die Message-Loop blockiert, sobald der Nutzer ein Kontextmenü öffnet oder die UI blockiert. Windows hängt den Hook bei Überschreiten von `LowLevelHooksTimeout` (Standard: 200–1000 ms) kommentarlos aus.
- **Vorschlag:** In §5 explizit die Windows-Thread-Architektur vorschreiben: „Unter Windows betreibt das Hotkey-Modul einen separaten Worker-Thread mit einer minimalen Win32-Message-Loop (`GetMessageW`), der ausschließlich den `WH_KEYBOARD_LL`-Hook hält, Down/Up-Events entprellt und über einen Channel an die zentrale State-Machine sendet. Völlige Entkopplung vom Tray-Thread (`betrayer`).“

---

### H2 — X11-Selection-Handling: Synchroner Sleep im Inject blockiert Event-Loop und zerstört Paste

- **Schwere:** Hoch
- **Stelle:** §7.1 (Clipboard + Paste), §12 Phase 2
- **Problem:** Auf X11 (ICCCM) ist die Zwischenablage kein globaler Speicherbereich, sondern eine Selection (`CLIPBOARD`). Nach dem Setzen des Transkripts und dem Senden von `Ctrl+Shift+V` / `Ctrl+V` sendet die Zielanwendung ein `SelectionRequest`-Event an Diktier. Wenn Diktier `restore_clipboard_delay_ms` (200 ms) als synchrones `std::thread::sleep` auf demselben Thread ausführt, der die X11-Connection hält, kann Diktier keine `SelectionRequest`-Events beantworten. Die Zielanwendung wartet, läuft in ein Timeout oder fügt nichts ein. Wenn Diktier danach sofort die alte Selection restauriert, wird der alte Text statt des Transkripts gepastet.
- **Vorschlag:** In §7.1 ergänzen: „Auf X11 muss der Selection-Owner während der Wartezeit aktiv eingehende `SelectionRequest`-Events abarbeiten. Der Restore-Timer läuft asynchron (z. B. im State-Machine-Loop). Die alte Selection wird erst freigegeben/überschrieben, wenn mindestens ein erfolgreicher Request bedient wurde oder das Timeout (`restore_clipboard_delay_ms`) ohne fremden Ownership-Verlust abgelaufen ist.“

---

### H3 — Terminal-Shortcut-Erkennung bricht auf Standard-X11-Terminals (`xterm` / Mint)

- **Schwere:** Hoch
- **Stelle:** §7.2 (Paste-Shortcut), §12 Phase 2 (Pflichtmatrix)
- **Problem:** §7.2 ordnet unter Linux X11 `Ctrl+Shift+V` einer festen Liste zu (GNOME Terminal, Xfce, Tilix, Alacritty, Kitty, Ghostty) und nutzt für alle anderen `Ctrl+V`. In §12 Phase 2 wird als Mint-Terminalmatrix „mitgeliefertes Terminal (gnome-terminal oder xterm)“ verlangt. `xterm` (sowie schlanke Terminals wie `rxvt`, `st`, etc.) unterstützen weder `Ctrl+V` noch `Ctrl+Shift+V` für Paste, sondern erwarten zwingend `Shift+Insert`.
- **Vorschlag:** In §7.2 die `auto`-Regel für X11 präzisieren: „`Ctrl+Shift+V` für moderne VTE/Freedesktop-Terminals (`gnome-terminal`, `xfce4-terminal`, `alacritty`, `kitty`, `ghostty`, `tilix`); `Shift+Insert` für `xterm`, `uxterm` und generische X11-Terminals; `Ctrl+V` für normale GUI-Fenster.“ In §12 Phase 2 klarstellen, dass unter Mint 22 Cinnamon standardmäßig `gnome-terminal` (bzw. das abgeleitete Mint-Terminal) geprüft wird.

---

### H4 — Fehlende Bereinigung aktiver Tastatur-Modifier vor Key-Injection

- **Schwere:** Hoch
- **Stelle:** §7.1, §7.2
- **Problem:** Wenn der Anwender die PTT-Taste loslässt und Diktier `Ctrl+V` bzw. `Ctrl+Shift+V` via `SendInput` (Windows) oder `XTest` (Linux) simuliert, können physisch noch Modifier aktiv sein (z. B. `Shift` beim schnellen Diktieren, `Alt`, oder `CapsLock`). Ein simuliertes `Ctrl+V` bei gedrückter `Shift`-Taste wird vom Zielprogramm als `Ctrl+Shift+V` interpretiert; bei gedrückter `Alt`-Taste als `Ctrl+Alt+V`.
- **Vorschlag:** In §7.1 ergänzen: „Vor dem Senden des Paste-Shortcuts stellt die Inject-Schicht sicher, dass keine störenden Modifier-Tasten (insbesondere `Shift`, `Alt`, `Super`/`Win`) aktiv sind; ggf. werden vorübergehend Up-Events für störende Modifier gesendet und nach dem Paste wiederhergestellt.“

---

### H5 — Audio-Callback Real-Time Violation durch `rubato`-Resampling und Allokationen

- **Schwere:** Hoch
- **Stelle:** §5 (Architektur), §6.4 (Audio)
- **Problem:** §6.4 fordert Konvertierung nach f32, Mittelung von Mehrkanal-Audio und Resampling mit `rubato` auf 16 kHz. Führt man dies direkt im `cpal`-Stream-Callback aus, drohen Audio-Glöckchen, Buffer-Underruns (XRuns) und Knackser, da `rubato` (FFT/Sinc-Algorithmen) rechenintensiv ist und Speicherallokationen durchführt.
- **Vorschlag:** In §6.4 verbindlich vorgeben: „Der `cpal`-Audio-Callback ist strikt lock-free und allokationsfrei. Er schiebt rohe Samples direkt in einen Ringpuffer (`ringbuf` / lock-free SPSC). Kanal-Mittelung, f32-Skalierung und Resampling via `rubato` finden ausschließlich auf dem separaten Audio-/Transkriptions-Worker-Thread statt.“

---

### M1 — Log-Kappung durch In-Place-Truncate ist I/O-ineffizient und crashanfällig

- **Schwere:** Mittel
- **Stelle:** §10 (Fehler, Recovery, Logs)
- **Problem:** „Vor einem Eintrag, der 2 MiB überschreiten würde: atomar auf die letzten 1 MiB vollständiger UTF-8-Zeilen kürzen, dann schreiben.“ Dies vor jedem Log-Write zu prüfen und die Datei im laufenden Betrieb einzulesen, zeilenweise zu kürzen und neu zu schreiben, erzeugt unnötigen I/O-Overhead und birgt bei einem Crash während des Schreibens das Risiko von Dateikorruption.
- **Vorschlag:** Auf Standard-File-Rotation umstellen: „Erreicht `diktier.log` 2 MiB, wird sie atomar zu `diktier.log.1` (bzw. `.old`) rotiert und eine neue `diktier.log` begonnen (maximal 1 Backup-Datei). Dies ist O(1), absturzsicher und erfordert kein zeilenweises In-Memory-Kürzen.“

---

### M2 — Zustands-Desynchronisation bei Mischung von PTT (`F9`) und Tray-Klick (Toggle)

- **Schwere:** Mittel
- **Stelle:** §4.3 (Tray), §5.2 (State-Machine)
- **Problem:** §4.3 regelt: „Während `recording` durch PTT wird Linksklick ignoriert.“ Es fehlt die inverse Regel: Wenn eine Aufnahme per Tray-Linksklick (Toggle) gestartet wurde, wie reagiert das System auf `F9 Press` bzw. `F9 Release`? Ein `F9 Release` könnte eine per Klick gestartete Aufnahme vorzeitig und unbemerkt beenden.
- **Vorschlag:** In §5.2 den Zustand `recording` durch den Auslöser typisieren: `recording(Source::Hotkey)` vs. `recording(Source::TrayClick)`. Regel ergänzen: „Befindet sich das System in `recording(TrayClick)`, werden `F9 Press`- und `Release`-Events ignoriert (Log-Info). Ein erneuter Tray-Klick oder das 60-s-Cap stoppen die Aufnahme.“

---

### M3 — Definition von `target_window_id` für Fokusprüfung präzisieren

- **Schwere:** Mittel
- **Stelle:** §5.1 (Module/Verträge), §7.3 (Fokus bei Inject), §12 Phase 2
- **Problem:** `target_window_id` ist nicht plattformspezifisch definiert. Unter Windows liefert `GetForegroundWindow()` das Top-Level-Fenster-Handle (`HWND`), während `GetFocus()` nur thread-lokal funktioniert. Unter X11 liefert `_NET_ACTIVE_WINDOW` auf dem Root-Window das aktive Top-Level-Window. Würde man versehentlich Child-Control-Handles prüfen, würden Fokusprüfungen bei Web-Apps oder Multi-Prozess-Editoren (VS Code) fehlschlagen.
- **Vorschlag:** In §7.3 festlegen: „`target_window_id` ist strikt das native Top-Level-Vordergrundfenster: unter Windows `HWND` via `GetForegroundWindow()`, unter Linux X11 `Window` via `_NET_ACTIVE_WINDOW`. Fokuswechsel innerhalb von Child-Controls oder Tabs desselben Top-Level-Fensters gelten nicht als Fokusverlust.“

---

### M4 — Naming und dynamisches Laden der Linux-ORT-Bibliothek (`libonnxruntime.so`)

- **Schwere:** Mittel
- **Stelle:** §11 (Verteilung)
- **Problem:** §11 nennt `lib/libonnxruntime.so.<abi>`. Offizielle Linux-Releases von Microsoft enthalten `libonnxruntime.so.1.x.y` und einen Symlink `libonnxruntime.so`. Tarballs/Zip-Archive können Symlinks je nach Packprogramm verlieren oder Windows-Inkompatibilitäten beim Entpacken erzeugen. Wenn `ort::init_from` einen festen Namen erwartet, muss dieser exakt definiert sein.
- **Vorschlag:** In §11 den Pfad verbindlich auf `lib/libonnxruntime.so` (bzw. auf Windows `lib/onnxruntime.dll`) festlegen. Release-Skripte kopieren die Shared Library unter diesen eindeutigen Namen ins `lib/`-Verzeichnis, ohne auf symbolische Links angewiesen zu sein.

---

### N1 — Widerspruch bei `DIKTIER_DEBUG_WAV=1` („überschreibt nichts“ vs. „nur letzte Datei“)

- **Schwere:** Niedrig
- **Stelle:** §10 (Fehler, Recovery, Logs)
- **Problem:** „... überschreibt nichts, ... Nur die letzte Datei behalten.“ Wenn nichts überschrieben wird, wächst das Verzeichnis; wenn nur die letzte Datei behalten wird, muss überschrieben oder alte gelöscht werden.
- **Vorschlag:** Formulierung bereinigen zu: „Schreibt nach `$TEMP/diktier-$USER/last_recording.wav` (bzw. Windows `%TEMP%\diktier\last_recording.wav`) mit Rechten `0600`. Jeder neue Aufnahmevorgang mit aktiver Debug-Flag überschreibt diese Datei atomar, sodass genau ein Audio-Dump auf der Festplatte existiert.“

---

### N2 — Fehlende Normalisierungs-Spezifikation für das STT-WER-Gate

- **Schwere:** Niedrig
- **Stelle:** §12 Phase 1 (STT-Spike)
- **Problem:** „nach dokumentierter Unicode-/Whitespace-/Interpunktionsnormalisierung“ lässt offen, wie Zahlen (Ziffern vs. Zahlwörter wie „42“ vs. „zweiundvierzig“) und Groß-/Kleinschreibung im Referenztext behandelt werden.
- **Vorschlag:** In Phase 1 ein einfaches, deterministisches Normalisierungsskript (in Rust oder Bash) festlegen: Lowercase, Entfernen von Interpunktion `[.,!?;:\-–—"']`, Reduktion mehrfacher Whitespaces. Referenztexte in `testdata/stt/` werden vorab exakt an die Parakeet-Zahlenausgabe angepasst.

---

## 3. Liste „So lassen“ (inkl. Codex-Würdigung)

Folgende Punkte in v1.1 sind explizit richtig und sollten **nicht** angetastet werden:

1. **Ausschluss von Wayland aus v1 (§2, §3, §17 #6):**
   *Begründung:* Codex lag hier vollkommen richtig. Virtual Keyboard Protocols und Global Hotkeys unter Wayland (Muffin/Cinnamon) ohne Root/uinput/evdev-Gruppenberechtigungen sind für v1 ein unkalkulierbares Risiko. Die Beschränkung auf Linux Mint 22 Cinnamon X11 ist pragmatisch und rettet den Zeitplan.
2. **Ausschluss von Windows-Elevation / UIPI (§2, §7.1, §17 #10):**
   *Begründung:* Diktier soll ohne Adminrechte laufen. Ein Injizieren in Administrator-Terminals/Notepads via `SendInput` scheitert an Windows UIPI. Das definierte Verhalten (Text bleibt im Clipboard, Tray meldet Infotext) ist die einzig saubere Lösung.
3. **Clipboard-Restore nur für Unicode-Plaintext (§7.1, §17 #8):**
   *Begründung:* Der Versuch, Rich-Text, HTML, Bilder oder OLE-Formate verlustfrei über Rust-Clipboard-Crates zu sichern und wiederherzustellen, ist extrem fehleranfällig. Die transparente Degradation (kein Restore bei Nicht-Text, Text bleibt im Clipboard) ist für v1 völlig ausreichend.
4. **Fokus-Verlust-Regel (§7.3, §17 #7):**
   *Begründung:* Wenn während der Transkription das Fenster gewechselt wird, darf Diktier **keinesfalls** blind in das neue Fenster pasten (Gefahr von Passworteingaben, Fehlinjektionen). Abbruch des Pastes und Belassen des Transkripts im Clipboard ist absolut sicher.
5. **Wegfall von `language` in Config (§6.2, §17 #4):**
   *Begründung:* Da `parakeet-tdt-0.6b-v3-int8` multilingual mit integrierter Spracherkennung arbeitet und kein wirksames Language-Enforcement-API in `parakeet-rs` existiert, verhindert das Entfernen aus der Config falsche Erwartungen.
6. **Golden Set Hashes (§6.3, §17 #1):**
   *Begründung:* Das Verankern der vier SHA-256-Hashes gegen das reale Omarchy-Voxtype-Setup ist der beste Schutz gegen stille Regressions- und Modellabweichungen.
7. **Kein Preview-Dialog, kein Whisper, kein OSD in v1 (§2):**
   *Begründung:* Der Fokus auf PTT und schmalen Daemon-Betrieb schützt vor Feature-Creep.

---

## 4. Offene Fragen an den Orchestrator vor Phase 1

1. **Fallback-Artefakt `nemo128.onnx`:**
   Soll `nemo128.onnx` (für den theoretischen `transcribe-rs`-Fallback) bereits jetzt mit SHA-256-Hash in `models.toml` und §6.3 aufgenommen werden, oder wird dieser Pfad erst angefasst, falls `parakeet-rs` in Phase 1 tatsächlich scheitert? *(Empfehlung: In der Spec belassen, aber erst bei Fail von `parakeet-rs` spezifizieren)*.
2. **Standard-Terminal unter Linux Mint 22:**
   Ist bestätigt, dass für das Mint-Inject-Gate in Phase 2 primär das Standard-Desktop-Terminal `gnome-terminal` (bzw. das Mint-Xed-Terminal) getestet wird, bei dem `Ctrl+Shift+V` der Standard ist?
3. **Lock-Free Ringbuffer für cpal:**
   Wird die Trennung zwischen cpal-Callback (nur Lock-Free Push in Ringpuffer) und Resampling-Worker (`rubato`) hiermit als Architekturvorgabe für §6.4 bestätigt?
4. **Log-Rotation statt In-Place-Truncate:**
   Darf der Implementierer in Phase 3/4 eine bewährte 2-Dateien-Rotation (`diktier.log` / `diktier.log.1`) anstelle des zeilenweisen In-Memory-Truncates umsetzen?

---

## Fazit & Freigabe-Empfehlung

Die Spec v1.1 ist konzeptionell exzellent und hat die strategischen Risiken erfolgreich isoliert. Werden die beiden **Blocker B1 (CLI-Lock-Bypass)** und **B2 (Fallback-Artefakt-Klarstellung)** sowie die **Threading-/X11-Hinweise (H1, H2, H5)** im Hinterkopf behalten bzw. redaktionell nachgeführt, ist die Spec **vollständig implementierungsreif für Phase 0 und Phase 1 (STT-Spike)**.
