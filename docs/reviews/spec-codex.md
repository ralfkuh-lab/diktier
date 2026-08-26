# Review: Diktier Spec v1

Review-Stand: 2026-08-26. Geprüft wurde `docs/SPEC.md` vollständig; die dort festgelegten Nicht-Ziele werden nicht in Frage gestellt.

## Kurzfazit

- Die Produktidee und die grobe Reihenfolge STT → Inject → Daemon sind tragfähig.
- Die Spec ist in der aktuellen Fassung jedoch noch nicht implementierungsreif.
- Blockierend sind vor allem zwei nicht falsifizierbare Gates: „dieselbe Liga“ bei STT und „Paste kommt an“ beim Inject.
- `parakeet-rs` kann die vorhandenen Joint-INT8-Dateien grundsätzlich laden und ist die richtige erste Wahl.
- Crate-, ORT-ABI-, Modell- und Artefaktversionen müssen vor dem Spike exakt gepinnt werden.
- CPU-ORT ist für Windows x64 und Mint x64 plausibel, das portable DLL/`.so`-Layout ist aber nicht festgelegt.
- Clipboard-Paste ist für Umlaute und Layoutunabhängigkeit richtiger als Zeichen-Simulation.
- Clipboard-Restore ist derzeit datenverlustgefährlich, weil Formate, Ownership und konkurrierende Änderungen undefiniert sind.
- Terminal-Erkennung, Paste-Chord und Wechsel des Fokus während der Transkription brauchen normative Regeln.
- Windows-Injection in erhöhte Prozesse funktioniert ohne gleiche Integritätsstufe nicht und muss als Grenze benannt werden.
- Die State-Machine bildet Laden, Download, Pause, Fehler-Recovery und konkurrierende Bedienung nicht ab.
- Wayland sollte aus der v1-Supportmatrix entfernt oder mit eigenen überprüfbaren Best-effort-Kriterien isoliert werden.
- Nach den unten genannten Spec-Änderungen sind Phase 1 und Phase 2 mit überschaubarem Risiko sinnvoll startbar.

## Befunde

### B1 — Das STT-Gate ist nicht reproduzierbar oder falsifizierbar

- **Schwere:** Blocker
- **Stelle:** §12, Phase 1 — STT-Spike; außerdem §1 Qualitätsziel
- **Problem:** „dieselbe Liga“, „Fachwörter nicht schlechter“ und „deutlich unter 10 s“ lassen unterschiedliche Bewertungen desselben Ergebnisses zu. Eine einzelne 5–15-s-WAV trennt Frontend-, Decoder- und Sprachqualitätsfehler nicht zuverlässig. Warm-up, Threadzahl, CPU, ORT-Version, Normalisierung und Referenztext sind nicht festgelegt. Damit kann ein fehlgeschlagener Spike als bestanden interpretiert werden. Die Stille-WAV prüft außerdem weder leises Rauschen noch die `< 250 ms`-Regel.
- **Vorschlag:** Den Gate-Text ersetzen durch: „`testdata/stt/` enthält mindestens drei selbst gesprochene deutsche Äußerungen (Alltag, Fachwörter, Zahlen/Umlaute) mit wortgetreuem Referenztext sowie je eine echte Stille- und Raumrauschdatei. Voxtype und Diktier verwenden byte-identische Modellartefakte. Nach dokumentierter Unicode-/Whitespace-/Interpunktionsnormalisierung gilt pro Sprachdatei `WER(Diktier, Referenz) <= WER(Voxtype, Referenz) + 0,05`; kein Diktier-Ergebnis darf einen nicht gesprochenen Satz enthalten, und die vorab markierten Fachwörter müssen mindestens so oft korrekt sein wie bei Voxtype. Stille, Raumrauschen und `< 250 ms` ergeben leer. Zeitmessung: Median aus fünf warmen Läufen, Modellladen separat; 10 s Audio <= 5 s auf dem benannten Büro-Laptop und <= 20 s auf der benannten Haswell-Maschine. `docs/SPIKES.md` protokolliert CPU, RAM, OS, Crate-/ORT-Version, Threadzahl, Artefakt-SHA256, Rohtexte, normalisierte Texte und Zeiten.“

### B2 — Das Inject-Gate definiert weder Erfolg noch die echte Zielmatrix ausreichend

- **Schwere:** Blocker
- **Stelle:** §7; §12, Phase 2 — Inject-Spike
- **Problem:** Ein erfolgreicher Tastatur-API-Aufruf beweist nicht, dass die Zielanwendung den Clipboard-Inhalt übernommen hat. „beide Zielplattformen“ ist bei Windows 10/11 und Mint X11/Wayland mehrdeutig; „ein Terminal“ ist nicht repräsentativ. Fokus, Zwischenablage und Text werden nur manuell beobachtet, aber erwartete Start-/Endwerte und Negativfälle fehlen. Damit ist das erklärte „v1 ist tot“-Gate nicht belastbar.
- **Vorschlag:** In §12 eine feste Matrix aufnehmen: Windows 10 x64 und Windows 11 x64 jeweils Notepad, VS Code, Windows Terminal/PowerShell; Mint 22 Cinnamon X11 jeweils xed, VS Code und das mit Mint ausgelieferte Terminal. Pro Fall stehen in `docs/SPIKES.md`: Fensterkennung vor/nach Paste identisch; exakter Text `Grüße, Öl, Spaß — Zeile 1\nZeile 2`; vorhandener Text-Clipboardwert nach spätestens 1 s wiederhergestellt; absichtlicher API-Fehler lässt das Transkript im Clipboard; Fokuswechsel während der Transkription folgt der in H5 festgelegten Regel; kein `^V`. Bestehen bedeutet exakte Zeichen- und Zeilengleichheit in jedem Pflichtfall. Wayland wird nur geprüft, falls es nach H1 im Scope bleibt.

### H1 — Wayland ist zugleich Zielzeile, Best-effort und ungegateter Sonderpfad

- **Schwere:** Hoch
- **Stelle:** §3 Plattformtabelle; §2; §12 Phase 2
- **Problem:** Mint/Cinnamon X11 ist der beschriebene Alltag, Wayland führt aber mit evdev, `wtype`, `dotool` und `ydotool` vier zusätzliche Sicherheits-, Berechtigungs- und Paketpfade ein. Wayland erlaubt virtuelle Tastaturen nur mit Compositor-Unterstützung bzw. Autorisierung; `global-hotkey` unterstützt laut Upstream nur X11. `ydotool`/evdev kann uinput-/Gruppen- oder Daemon-Setup benötigen und passt schlecht zu „kein Admin im Normalbetrieb“. Für diesen Best-effort-Pfad gibt es kein Release-Gate und keine definierte Degradation.
- **Vorschlag:** Für v1 die Tabellenzeile ersetzen durch „Linux Mint 22, Cinnamon, X11 — unterstützt“ und unter Nicht-Ziele „Cinnamon/Wayland (v1 nicht unterstützt; Programm startet mit verständlichem Fehler und ohne Hotkey)“ ergänzen. Falls Wayland unbedingt stehen bleibt: separat als experimentell, opt-in, ohne Release-Blockade spezifizieren und pro Compositor genau einen Hotkey- und Inject-Weg samt Installationsvoraussetzungen nennen.

### H2 — Die zentrale State-Machine ist unvollständig

- **Schwere:** Hoch
- **Stelle:** §4.1–4.3; §5 und §5.1; §13
- **Problem:** `idle | recording | transcribing | error` enthält weder `starting/downloading/loading` noch `paused`. Unbestimmt sind Press während `transcribing`, Press während Modellladen, Pause während Aufnahme, Beenden während Inferenz, Click während Transkription, wiederholte Press-Events, Release nach dem 60-s-Cap und Recovery aus `error`. Die Aussage „idle = geladen, bereit“ widerspricht „Aufnahme startet, auch wenn das Modell noch lädt“.
- **Vorschlag:** In §5 eine normative Übergangstabelle ergänzen. Mindestens: `starting → downloading → loading → idle`; `idle + Press → recording`; `recording + Release|Timeout → transcribing`; `transcribing + Erfolg → idle`; `transcribing + leer → idle`; jeder Betriebszustand + fataler Fehler → `error`; `paused` als orthogonales Flag oder eigener Zustand. Weitere Regeln: Press außerhalb `idle` wird mit Log-Warnung ignoriert; Auto-Repeat wird entprellt; nach 60-s-Timeout wird genau einmal transkribiert und die folgende Release ignoriert; Pause stoppt eine laufende Aufnahme ohne Transkription oder ist währenddessen deaktiviert — eine Variante verbindlich wählen; `error` nennt je Fehlerklasse den Retry-Auslöser.

### H3 — Clipboard-Restore kann fremde neue Daten überschreiben

- **Schwere:** Hoch
- **Stelle:** §7.1; §13 Inject-Tests
- **Problem:** Nach pauschal 200 ms wird blind der alte Wert zurückgeschrieben. Kopiert der Nutzer oder eine andere Anwendung in diesem Fenster etwas Neues, würde Diktier diese neue Zwischenablage überschreiben. Auf X11 ist das Clipboard eine asynchrone Selection mit einem Owner; ein fixer Delay beweist nicht, dass die Zielanwendung die Daten schon angefordert hat. Auf Windows existieren Sequenznummern und delayed-rendered Formate. Diese Race-Semantik fehlt vollständig.
- **Vorschlag:** In §7.1 ergänzen: „Diktier merkt nach dem Setzen die eigene Clipboard-Generation/Selection-Ownership. Restore erfolgt nur, wenn Diktier noch Owner ist bzw. die Windows-Clipboard-Sequenz seit dem eigenen Setzen unverändert blieb. Bei einer fremden Änderung wird niemals restauriert. Der Delay ist eine Mindestwartezeit, keine Erfolgserkennung. Diktier hält die X11-Selection mindestens bis zum Restore/Ownership-Verlust bedienbar.“ Das Gate muss einen konkurrierenden Copy-Vorgang innerhalb der Wartezeit prüfen und dessen Inhalt danach unverändert erwarten.

### H4 — „Clipboard-Inhalt merken“ ist hinsichtlich Formaten unentscheidbar

- **Schwere:** Hoch
- **Stelle:** §7.1; §8 `restore_clipboard`
- **Problem:** Zwischenablagen enthalten nicht nur Text, sondern Bilder, Dateilisten, HTML/RTF und anwendungseigene bzw. verzögert gerenderte Formate. Rust-Clipboard-Crates sichern typischerweise nicht verlustfrei „den Inhalt“. Der jetzige Wortlaut verspricht mehr, als der vorgeschlagene Weg garantiert, und kann ein Bild oder Rich Text zerstören.
- **Vorschlag:** Den v1-Vertrag ausdrücklich begrenzen oder erweitern. Konkrete schmale Variante: „Restore ist in v1 nur für Unicode-Plaintext garantiert. Ist vor dem Inject kein Unicode-Text snapshotbar, wird nicht so getan, als sei Restore möglich: nach Paste bleibt das Transkript im Clipboard und der Tooltip meldet `Nicht-Text-Clipboard konnte nicht restauriert werden`.“ Alternativ muss die Spec pro Plattform alle zu konservierenden Formate samt Tests aufzählen. Zusätzlich im Phase-2-Gate Text, leeres Clipboard und mindestens einen Nicht-Text-Inhalt prüfen.

### H5 — Das Ziel bei zwischenzeitlichem Fokuswechsel ist nicht definiert

- **Schwere:** Hoch
- **Stelle:** §1; §4.1–4.2; §7
- **Problem:** Inferenz kann Sekunden dauern. Wechselt der Nutzer danach in ein Passwortfeld, Terminal oder anderes Dokument, ist unklar, ob Diktier in das bei Press, bei Release oder bei Inject fokussierte Fenster schreibt. Das ursprüngliche Fenster wieder zu aktivieren würde die Fokusregel verletzen; blind in das neue zu pasten ist gefährlich.
- **Vorschlag:** In §7 normativ festlegen: „Beim Aufnahmeende wird die native Kennung des Vordergrundfensters erfasst. Vor Inject muss dieselbe Kennung noch Vordergrund sein. Andernfalls wird kein Paste-Key gesendet, das Transkript bleibt im Clipboard und der Tray-Tooltip meldet `Fokus geändert — Text liegt im Clipboard`. Diktier aktiviert niemals selbst ein Fenster.“ Phase 2 muss genau diesen Fall testen.

### H6 — Windows-Injection ist ohne Admin nicht in alle Fenster möglich

- **Schwere:** Hoch
- **Stelle:** §3 „kein Admin“; §7.1; §12 Phase 2
- **Problem:** `SendInput` unterliegt UIPI und darf nur in Prozesse gleicher oder niedrigerer Integritätsstufe injizieren. Ein normal gestartetes Diktier kann daher nicht verlässlich in ein als Administrator gestartetes Terminal oder Programm pasten; der Rückgabewert erklärt UIPI zudem nicht eindeutig. „Windows 10+“ ohne Einschränkung verspricht aktuell zu viel.
- **Vorschlag:** In §3/§7 ergänzen: „Normale, nicht erhöhte Desktop-Anwendungen sind unterstützt. Inject in erhöhte/UAC-Prozesse ist ohne erhöhtes Diktier nicht unterstützt; Diktier fordert keine Elevation an. Wenn `SendInput` nicht alle Key-Events annimmt, bleibt das Transkript im Clipboard und es erscheint die generische Inject-Fehlermeldung.“ Als Negativtest ein erhöhtes Notepad/Terminal aufnehmen; erwartet wird kein Fokuswechsel und Text im Clipboard, nicht erfolgreicher Paste.

### H7 — Terminal-Paste und „bekannte Terminals“ sind nicht spezifiziert

- **Schwere:** Hoch
- **Stelle:** §7.1; §12 Phase 2
- **Problem:** `Ctrl+V`, `Ctrl+Shift+V` und `Shift+Insert` unterscheiden sich zwischen Terminal, Shell, Terminalprofil und Anwendung. Die Spec benennt weder Erkennungsmerkmale noch Fallback und verwechselt Zielprozess und sichtbares Fenster teilweise (z. B. Windows Terminal hostet PowerShell). Fehlklassifikation führt zu `^V` oder keinem Paste. Umlaute selbst sind beim Unicode-Clipboard layoutunabhängig; nur der simulierte Shortcut muss als physische/virtuelle Taste robust sein.
- **Vorschlag:** `output.paste_shortcut = "auto" | "ctrl_v" | "ctrl_shift_v" | "shift_insert"` ergänzen. Für `auto` eine verbindliche, kurze Windows-Prozess-/Fensterklassen- und X11-`WM_CLASS`-Liste definieren; unbekannte Ziele verwenden `Ctrl+V`. Im Gate alle Pflichtterminals plus manuelle Overrides prüfen. Wenn robuste Auto-Erkennung im Spike scheitert, ist der v1-Fallback `shift_insert` als dokumentierter Terminal-Override, nicht Zeichen-für-Zeichen-Tippen.

### H8 — Modellquelle, Dateisatz und Downloadtransaktion sind nicht festgelegt

- **Schwere:** Hoch
- **Stelle:** §6.1–6.3; §11
- **Problem:** „dieselben Hugging-Face-/Voxtype-Artefakte“ ist keine eindeutige Quelle. Repository, Revision/Commit, URL, Größe, Dateiname, SHA256 und Lizenz fehlen. Der lokal installierte Voxtype-INT8-Satz enthält `encoder-model.int8.onnx`, `decoder_joint-model.int8.onnx`, `vocab.txt` und `config.json`; `transcribe-rs` verlangt zusätzlich `nemo128.onnx`. Ein abgebrochener Download kann derzeit als vorhandenes Modell gelten. Mehrere Starts könnten parallel in dasselbe Verzeichnis schreiben.
- **Vorschlag:** `models.toml` verbindlich machen: pro Engine+Modell exakte immutable `resolve/<commit>/...`-URLs, Dateinamen, Bytes, SHA256 und Lizenz/Notice. Für den Default zunächst die SHA256 der tatsächlich mit Voxtype verglichenen vier Dateien einfrieren (auf dem Review-System: Encoder `6139d2fa…aff09`, Decoder `eea7483e…7a70`, Vocab `d5854467…c35d`, Config `666903c7…c466`). Download je Datei nach `<name>.part`, Hash und Größe prüfen, erst danach atomar umbenennen; ein vollständiger Marker wird zuletzt geschrieben. Ein per-user Download-Lock verhindert Parallelität. Hashfehler löscht nur die `.part`-Datei, meldet `error` und wird erst nach explizitem Retry/Neustart erneut versucht.

### H9 — `parakeet-rs` und ORT sind nicht exakt genug gepinnt; der Fallback ist kein Gate

- **Schwere:** Hoch
- **Stelle:** §6.1; §11; §12 Phase 1
- **Problem:** `parakeet-rs` ist jung und hängt derzeit an einem ORT-2.0-Release-Candidate/API-Feature. Das Upstream-API kann sich innerhalb der genannten Major-/Minor-Angabe ändern. Die Spec sagt nicht, ob `load-dynamic` verwendet wird, welche ORT-API-Version die ausgelieferte Library erfüllen muss oder wann genau auf `transcribe-rs` gewechselt wird. Dessen aktueller 0.3-Zweig bezeichnet sich selbst als große, noch stabilisierende Migration und benötigt den zusätzlichen Preprocessor `nemo128.onnx`.
- **Vorschlag:** In §6.1 festlegen: „Phase 1 pinnt `parakeet-rs`, `ort`/`ort-sys` und ORT-Binary exakt (`=` bzw. `Cargo.lock`) und aktiviert CPU plus `load-dynamic`. Vor jeder Session initialisiert die App ORT aus dem Bundlepfad. `parakeet-rs` gilt als gescheitert, wenn auf einer Pflichtplattform Modellladen, alle Qualitätsfälle, Stille oder Zeitlimit fehlschlagen. Dann wird exakt eine gepinnte `transcribe-rs`-Version mit `nemo128.onnx` durch dasselbe Gate geschickt. Nur wenn auch diese scheitert, darf sherpa-onnx mit eigenem Artefaktsatz und identischem Qualitätsgate geprüft werden.“ Kein selbst geschriebener TDT-Decoder bleibt richtig ausgeschlossen.

### H10 — Das ORT-DLL/`.so`-Layout ist auf Linux nicht ausführbar spezifiziert

- **Schwere:** Hoch
- **Stelle:** §11 Verteilung; §12 Phase 1 und Phase 4
- **Problem:** „`.so` neben der Binary / über Paket; Spike entscheidet“ lässt die zentrale portable Eigenschaft offen. Ein Linux-Linker sucht nicht allgemein im Executable-Verzeichnis. „Ein Binary“ widerspricht außerdem dem Windows-ZIP mit DLL. Ein Systempaket würde Versions- und Installationsabhängigkeit sowie eventuell Adminbedarf einführen. Es fehlt ein Test ohne Entwicklerumgebung und ohne `LD_LIBRARY_PATH`/`PATH`-Zufall.
- **Vorschlag:** Für beide OS ein Bundleformat festlegen, z. B. `diktier[.exe]` plus `lib/onnxruntime.dll` bzw. `lib/libonnxruntime.so.<major>` und Notices. Mit `ort/load-dynamic` wird vor jeder ORT-Nutzung der absolute Pfad relativ zu `current_exe()` via `ort::init_from` geladen; kein PATH und kein systemweites ORT. Die exakte ORT-Version/API muss im Manifest stehen. Release-Gate: ZIP/Tarball in ein neues Verzeichnis einer sauberen Win10-, Win11- und Mint-22-VM entpacken, sämtliche ORT-Umgebungsvariablen entfernen und STT starten. Wortlaut „ein Binary“ zu „ein Anwendungsprozess/eine ausführbare Datei, plus gebündelte Runtime-Library und heruntergeladenes Modell“ korrigieren.

### H11 — Hotkey-Backend und PTT-Release-Semantik sind nicht verbindlich

- **Schwere:** Hoch
- **Stelle:** §3; §4.4; §5; §12 Phase 2
- **Problem:** Die Tabelle fordert Windows „nativer Low-Level-Hook“, während §3 für Linux `global-hotkey` nennt. `global-hotkey` ist X11-only und seine Press/Release- sowie Eventloop-Eigenschaften sind versionsabhängig; bloßes Registrieren eines Shortcuts reicht für PTT nicht. Nicht geregelt sind verlorene Releases bei Fokus-/Desktopwechsel, Auto-Repeat, Hotkey-Registrierungskonflikte und Verhalten bei gesperrtem Desktop. F9 „gewinnt“ nur, wenn die Registrierung gelingt.
- **Vorschlag:** Backends festschreiben: Windows über `windows`-API mit nachweisbaren Down/Up-Ereignissen (Low-Level-Hook oder exakt gegatete RegisterHotKey/GetAsyncKeyState-Lösung), Mint X11 zunächst exakt gepinntes `global-hotkey`. Phase 2 misst Press/Release, 60-s-Cap, Auto-Repeat und Registrierungsfehler. Falls `global-hotkey` auf Mint ein Release verliert oder den Tray-Eventloop blockiert, ist der spezifizierte Fallback `x11rb` mit `XGrabKey`/KeyPress/KeyRelease hinter dem `HotkeyBackend`-Trait; kein evdev für X11. Registrierungsfehler führen zu `error`, lassen aber Tray-Click aktiv.

### H12 — Single-Instance per Lock-Datei ist race- und crashanfällig

- **Schwere:** Hoch
- **Stelle:** §5 Single-Instance; §12 Phase 3
- **Problem:** Die Existenz einer Datei ist kein belastbarer Prozess-Lock; nach Crash bleibt sie liegen, und zwei Starts können gleichzeitig prüfen. `$XDG_RUNTIME_DIR` kann fehlen. „bringt den laufenden Prozess in den Vordergrund“ widerspricht der Fokusregel und ist bei einer Tray-App ohne Fenster inhaltlich unklar. Ein Balloon/Log ist kein Vordergrundprozess.
- **Vorschlag:** Text ersetzen durch: „Windows verwendet einen per-user Named Mutex; Linux einen gehaltenen advisory lock (`flock`/äquivalent) im sicheren Runtime-Verzeichnis, mit Fallback unter dem per-user State-Verzeichnis. Die Datei darf liegen bleiben; allein der gehaltene Lock zählt. Der zweite Prozess sendet optional über eine kleine lokale IPC-Nachricht `notify`, aktiviert kein Fenster, beendet sich mit dokumentiertem Exitcode 0 und schreibt eine kurze Meldung nach stderr. Keine PID wird ungeprüft beendet.“ Gate um Parallelstart und Neustart nach hart beendetem ersten Prozess erweitern.

### H13 — Audioformat und `cpal`-Backend sind zu optimistisch beschrieben

- **Schwere:** Hoch
- **Stelle:** §3; §5; §6.4; §12 Phase 2
- **Problem:** `sample_rate = 16000` klingt wie eine Geräteanforderung, obwohl viele Geräte nur 44,1/48 kHz und unterschiedliche Sampleformate/Kanalzahlen anbieten. Die Spec regelt Auswahl einer unterstützten Konfiguration, Downmix, Samplekonvertierung, Resampler-Flush, Geräteverlust und Default-Device-Wechsel nicht. Der STT-Spike mit bereits passender WAV prüft den echten Capture-/Resample-Pfad überhaupt nicht. `cpal` erreicht auf Linux je nach Version/Features ALSA, Pulse oder PipeWire unterschiedlich; „PipeWire/Pulse via cpal“ ist ohne Features/Version keine Garantie. Linearer Resampler kann die Qualitätsreferenz unbemerkt verändern.
- **Vorschlag:** In §6.4 festlegen: „Gerät wird in seiner unterstützten nativen Rate und Sampledarstellung geöffnet; alle Integer-/Floatformate werden begrenzt nach f32 konvertiert, Mehrkanal wird definiert gemittelt, dann mit gepinntem `rubato` auf exakt 16 kHz resampled und beim Stop vollständig geflusht. `audio.sample_rate` ist die Engine-Zielrate und in v1 nur 16000 zulässig.“ Für Mint zuerst die passenden nativen `cpal`-Pulse/PipeWire-Features pinnen; falls deren Spike scheitert, ist der plattformspezifische Backend-Fallback hinter `AudioSource` (`windows`/WASAPI bzw. eine gepinnte Pulse/PipeWire-Bindung), nicht ein Shell-Programm. Phase 2 muss echte 44,1- und 48-kHz-Geräte bzw. Fixtures, Stereo, Device-lost und Reopen testen.

### H14 — Das Modellmenü verspricht ungeprüfte Varianten und eine möglicherweise wirkungslose Spracheinstellung

- **Schwere:** Hoch
- **Stelle:** §6.2; §8
- **Problem:** Der Default ist der einzige für das Produktziel relevante und gegatete Datensatz. Die unquantisierte TDT-Variante hat einen anderen Dateisatz (einschließlich externer ONNX-Daten), und `parakeet-unified-en-0.6b` ist ein anderer Modelltyp/API-Pfad. Trotzdem erscheinen beide wie gleichwertig konfigurierbar, ohne eigene Manifest- oder Gates. `language = "de"` darf laut Text ignoriert werden; dann behauptet die Config eine Wirkung, die nicht existiert. „Unbekanntes Modell: nicht starten“ widerspricht §8 „ungültige Werte: Default“.
- **Vorschlag:** In v1 nur `parakeet-tdt-0.6b-v3-int8` als freigegebenen Schlüssel führen. Weitere Modelle erst nach eigenem Manifest plus Phase-1-Gate aufnehmen. `language` für TDT v1 entweder entfernen und ausschließlich Auto dokumentieren oder beim Start mit Warnung sichtbar auf `auto` normalisieren; nie still ignorieren. Validierungsregel präzisieren: unbekanntes Modell ist fataler Configfehler mit Tray `error`, kein Default-Fallback.

### M1 — Config-Fehler fallen teilweise gefährlich auf Defaults zurück

- **Schwere:** Mittel
- **Stelle:** §8; §13
- **Problem:** Ein TOML-Syntaxfehler oder Tippfehler bei Hotkey/Output kann still F9 und Paste aktivieren. Unbekannte Keys sind als Testfall genannt, ihr erwartetes Ergebnis fehlt. Bereichsgrenzen für Delay, Threads, Dauer und Modifier sind undefiniert. „Defaults schreiben“ ist ohne atomisches Schreiben bei Crash riskant.
- **Vorschlag:** Drei Klassen definieren: Syntaxfehler sowie ungültige `hotkey.key`, `output.mode` und `engine.model` → kein Hotkey/keine Aufnahme, Tray `error`; unbekannte Keys → ignorieren plus Warnung; harmlose numerische Werte → definierte Grenzen (`max_duration_secs 1..=60`, `restore_clipboard_delay_ms 0..=5000`, `threads 0..=<logische CPUs oder festgelegtes Maximum>`) und Warnung. Defaults über temporäre Datei plus Rename schreiben. Tests müssen pro Klasse Zustand, Log und effektiven Wert prüfen.

### M2 — Fehlerzustände haben keine Recovery- und Datenhaltungsregeln

- **Schwere:** Mittel
- **Stelle:** §4.3; §6.3; §10; §13
- **Problem:** Für „Mic/Modell/Inject kaputt“ ist nicht festgelegt, ob der Hotkey deaktiviert wird, wie Retry erfolgt, ob ein vorliegendes Transkript erhalten bleibt oder ob ein temporärer Mic-Ausfall permanent `error` macht. Eine Desktop-Notification kann nur Zusatz sein; wenn Tray-Aufbau selbst scheitert, fehlt der sichtbare Kanal.
- **Vorschlag:** Fehlerklassen ergänzen: Download/Modell/ORT fatal bis Neustart oder explizitem Retry; Hotkeyfehler lässt Click-to-record aktiv; Micfehler wird beim nächsten Startversuch einmal neu geöffnet; Injectfehler bewahrt Text im Clipboard; Trayfehler beendet den Prozess mit stderr+Log statt kopflos weiterzulaufen. Für jede Klasse Übergang, Retry, Tooltip und Text-/Audio-Lebensdauer festlegen und in State-Tests abbilden.

### M3 — `betrayer` ist sinnvoll, aber noch kein abgesicherter Plattformvertrag

- **Schwere:** Mittel
- **Stelle:** §4.3; §5; §12 Phase 3
- **Problem:** `betrayer` 0.4.x benötigt unter Windows einen laufenden Eventloop auf demselben Thread und nutzt unter Linux StatusNotifierItem/DBus mit eigenem Thread. Upstream nennt begrenzte Menüfunktionalität und mögliche AppIndicator-Abhängigkeiten. Ob Linksklick, dynamische Icons/Tooltips und nicht klickbare Statuszeile unter Cinnamon exakt funktionieren, ist ungeprüft. Das ist nicht kritisch für Phase 1, kann Phase 3 aber spät blockieren.
- **Vorschlag:** Vor Phase 3 einen kleinen Tray-Gatepunkt ergänzen: exakt gepinnte Version, Windows 10/11 und Mint Cinnamon; Links-/Rechtsklick getrennt; Tooltip/Icon-Update; Pause/Resume; Explorer-/Panel-Neustart; sauberer Quit; kein Fokuswechsel. Fallback im Spec: Windows `Shell_NotifyIconW` über `windows`, Linux `ksni`, beide hinter `TrayBackend`. `tray-icon` bleibt nur letzter Fallback, falls die akzeptierte Linux-Paketabhängigkeit ausdrücklich neu entschieden wird.

### M4 — Release-Matrix, CPU-Baseline und Paketinhalt sind nicht vollständig

- **Schwere:** Mittel
- **Stelle:** §3; §11; §12 Phase 4
- **Problem:** Windows 10/11 werden gemeinsam genannt, aber die Gates verlangen nur „Windows“. Mint-Version, Architektur, glibc-Baseline, CPU-Instruktionssatz und maximale RAM-Nutzung fehlen. Ein auf dem Entwicklungsrechner gebautes Linux-Binary kann auf Mint wegen glibc/CPU scheitern. Modell und ORT-Lizenzen/Notices fehlen im ZIP-Layout.
- **Vorschlag:** Supportmatrix festschreiben: Windows 10 22H2 x64, Windows 11 aktuell x64, Mint 22.x Cinnamon X11 x86_64; Mindest-CPU AVX2, falls der ORT-Build dies tatsächlich verlangt. Release in der ältesten Zielumgebung bauen oder dort testen, `ldd`/Dependency-Check ohne `not found`, Peak-RSS protokollieren und ein RAM-Ziel festlegen. Bundle enthält App, ORT, `LICENSES/` und Versionsmanifest; Modelllizenz/-quelle wird beim Download und in README genannt.

### M5 — Autostart und Konsolenmodus sind nicht eindeutig

- **Schwere:** Mittel
- **Stelle:** §9
- **Problem:** „ein Binary“ soll interaktiv stderr zeigen und aus Autostart ohne Konsole laufen. Unter Windows ist Console-vs-Windows-Subsystem eine Buildentscheidung bzw. erfordert bewusstes Attach-Verhalten. Nicht geregelt sind bestehende/veraltete Links, Pfade mit Leerzeichen, Umzug eines portablen Ordners, idempotente Installation und Exitcodes.
- **Vorschlag:** CLI-Vertrag ergänzen: Install/Remove idempotent; absolute, korrekt gequotete `current_exe()`-Pfade; vorhandener eigener Eintrag wird aktualisiert; fremder Eintrag nie gelöscht; standardisierte Exitcodes. Windows-Buildmodus und Verhalten von `--foreground` konkret festlegen. Phase 4 testet Pfad mit Leerzeichen, zweimaliges Install/Remove und verschobenen Ordner (definierter Fehler oder Update durch erneutes Installieren).

### M6 — Tests decken zentrale Invarianten der State- und Clipboard-Logik nicht ab

- **Schwere:** Mittel
- **Stelle:** §13
- **Problem:** Es fehlen Press während Transkription, Auto-Repeat, Pause/Resume, Timeout+spätes Release, Modellladen, Downloadabbruch/Hashfehler, Fokuswechsel, Clipboard-Fremdänderung, Injectfehler und Recovery. „Wo CI kein ORT hat“ ist vermeidbar, wenn Engine und Modellloader getrennt sowie ein kleiner ORT-Smoke-Test als Artefaktjob geführt werden.
- **Vorschlag:** Die automatischen Tests um die genannten Übergänge und Fake-Backends erweitern. Download mit lokalem Fake-Transport testen: Abbruch, falsche Größe, falscher Hash, atomarer Abschluss, Parallelstart. Clipboard-Fake führt Generationen/Ownership und Fremdänderung. Ein separater, explizit gestarteter `stt-smoke` bleibt modellabhängig; er darf im normalen Unit-Test-Lauf ignoriert sein, muss aber in `docs/SPIKES.md` auf beiden OS bestanden haben.

### M7 — v2-Erweiterbarkeit ist als Wunsch, nicht als Schnittstellenvertrag formuliert

- **Schwere:** Mittel
- **Stelle:** §5.1; §14
- **Problem:** `Transcriber` ist eine gute Grenze, aber v2 braucht auch ein Ergebnisobjekt, einen Zielkontext und eine Ausgabeentscheidung. Wenn v1 sofort `String → inject()` koppelt, erzwingt der Review-Dialog später einen Umbau. HTTP ist Nicht-Ziel und muss dafür nicht vorbereitet werden; Preview darf jedoch nicht in Audio/Engine hineinwachsen.
- **Vorschlag:** In §14 nur abstrakte Verträge ergänzen: `Transcription { text, language?, timing? }`, `CaptureContext { target_window_id, ended_at }`, und ein `OutputSink`/Command, das `paste`, `copy_only` oder später `review` wählen kann. v1 instanziiert ausschließlich `paste/copy_only`; kein Dialog- oder HTTP-Code. Damit bleiben Engine, State und Inject trennbar, ohne v2 vorwegzubauen.

### N1 — Log-Kappen per Truncate kann den wichtigsten Fehler löschen

- **Schwere:** Niedrig
- **Stelle:** §10
- **Problem:** „kappen bei 2 MB“ sagt nicht, ob vor oder nach dem aktuellen Eintrag und auf welche Größe. Komplettes Truncate löscht genau die Historie, die einen seltenen Crash erklärt; parallele Writer stderr+Datei sind nicht geregelt.
- **Vorschlag:** Festlegen: Beim nächsten Start oder vor einem neuen Eintrag über 2 MiB atomar auf die letzten 1 MiB vollständiger UTF-8-Zeilen kürzen, dann neuen Eintrag schreiben; nur ein Logger-Writer besitzt die Datei. Alternativ eine Rotation `diktier.log`/`diktier.log.1` zulassen. Transkripte, Clipboard-Inhalte und Hotkey-Ziel-Fenstertitel bleiben aus Logs entfernt.

### N2 — Das Debug-WAV-Verhalten braucht Lebensdauer und Berechtigungen

- **Schwere:** Niedrig
- **Stelle:** §10
- **Problem:** `DIKTIER_DEBUG_WAV=1` schreibt potentiell personenbezogene Sprache in ein gemeinsames Temp-Verzeichnis, ohne Dateirechte, Namen, Überschreiben oder Löschregel. „Audio nie auf Disk, außer …“ ist zwar klar gemeint, aber operativ zu offen.
- **Vorschlag:** Ergänzen: Debug-WAV ist ausschließlich explizit per Prozessumgebung aktiv, erhält einen zufälligen Namen in einem per-user Temp-Unterverzeichnis mit restriktiven Rechten, überschreibt nichts, wird im Log mit Pfad angekündigt und nicht automatisch hochgeladen. Eine Aufbewahrungsregel festlegen (z. B. maximal die letzte Datei oder keine automatische Löschung, aber deutlich dokumentiert).

## So lassen

- Den Scope v1 klein halten: kein Preview-Dialog, Whisper, HTTP-Server, Streaming, OSD oder Wortersetzungen.
- `parakeet-rs` als erste Wahl und den Decoder nicht in Diktier selbst implementieren.
- Das Joint-ONNX-INT8-Modell `parakeet-tdt-0.6b-v3-int8` als Qualitätsanker gegen Voxtype verwenden.
- CPU-first ohne CUDA-Pflicht und Haswell-/Büro-PC als ausdrücklich zu prüfenden Designpunkt.
- Modell resident halten; Modellladen darf niemals den Audio-Callback blockieren.
- Clipboard-Paste statt Zeichen-für-Zeichen-Tippen als Default. Das ist gerade für `äöüÄÖÜß`, Layouts und tote Tasten die richtige Entscheidung.
- `output.mode = "type"` höchstens optional und ohne v1-Garantie lassen; bei Spike-Misserfolg nicht weiterverfolgen.
- Kein fokusnehmendes Fenster auf dem PTT-Pfad; Fehler nur über Tray/Notification/Log.
- F9 ohne Modifier als bewussten Default beibehalten, solange Konflikt/Registrierungsfehler sichtbar und die Config vor dem Alltagseinsatz änderbar ist.
- 60 s harte Obergrenze und `< 250 ms` nicht transkribieren beibehalten.
- Keine Transkripte loggen und Audio standardmäßig nie persistieren.
- `Cargo.lock` für die Anwendung committen und Crates/ORT zusätzlich exakt pinnen.
- `Transcriber`-, `inject`- und `hotkey`-Abstraktionen sowie zentrale Queue/State-Machine beibehalten.
- Autostart opt-in und ohne Adminrechte lassen.
- Phase 1 vor Inject und beide Spikes vor Tray/Daemon erzwingen.

## Offene Fragen an den Orchestrator vor Phase 1

1. Welche exakte Hugging-Face-Repository-Revision und welche vier SHA256-Werte sind die verbindliche Voxtype-Referenz? Sollen die auf dem Omarchy-Rechner vorhandenen Artefakte oben zum Golden Set erklärt werden?
2. Welche konkreten Rechner bilden „aktueller Büro-Laptop“ und „Haswell-Klasse“, und welches Peak-RAM-Limit gilt für v1?
3. Darf das Qualitätsgate von einer auf mindestens drei gesprochenen WAVs erweiterten Testmenge ausgehen, oder muss es bei genau einer Sprach-WAV bleiben?
4. Soll `language = "de"` aus v1 entfernt werden, solange `parakeet-rs` für TDT nur Auto-Erkennung anbietet, oder ist ein sichtbarer No-op akzeptiert?
5. Sollen unquantisierte TDT und Unified-English aus der v1-Modellliste entfernt werden, bis sie je ein eigenes Gate bestanden haben?
6. Ist Cinnamon/Wayland ausdrücklich außerhalb des v1-Supportvertrags? Falls nein: welcher konkrete Cinnamon-/Muffin-Stand und welche erlaubte einmalige Einrichtung gelten?
7. Was ist die gewünschte Fokusregel bei Fensterwechsel während der Inferenz: der hier empfohlene Abbruch mit Text im Clipboard oder Paste in das dann aktuelle Fenster?
8. Muss Clipboard-Restore Nicht-Text- und Rich-Text-Formate verlustfrei erhalten, oder darf v1 die Garantie explizit auf Unicode-Plaintext begrenzen?
9. Ist ein konfigurierbarer Terminal-Paste-Shortcut akzeptiert, wenn Auto-Erkennung nicht auf allen Pflichtterminals robust ist?
10. Gilt Inject in erhöhte Windows-Prozesse ausdrücklich als nicht unterstützt?
11. Soll ORT auf beiden Plattformen verbindlich als private, relativ zum Executable dynamisch geladene Library gebündelt werden, statt Linux-Systempakete zuzulassen?
12. Welcher genaue Windows-Toolchain-/CRT- und Linux-glibc-Baseline-Build ist für die Releaseartefakte vorgesehen?

## Technische Belege für die risikoreichen Punkte

- [`parakeet-rs` README](https://github.com/altunenes/parakeet-rs): TDT, 16-kHz-Audio, Joint-Decoder-Dateisatz und CPU/ORT-Grundlage.
- [`parakeet-rs` Cargo.toml](https://github.com/altunenes/parakeet-rs/blob/master/Cargo.toml): aktuelle ORT-RC-Abhängigkeit sowie `load-dynamic`-Feature; deshalb exakt pinnen.
- [`transcribe-rs` README](https://github.com/cjpais/transcribe-rs): benötigtes `nemo128.onnx`, ONNX-Feature und derzeitiger 0.3-Migrationshinweis.
- [`global-hotkey` README](https://github.com/tauri-apps/global-hotkey): Linux nur X11 und Eventloop-Anforderungen.
- [`betrayer`-Dokumentation](https://docs.rs/betrayer/latest/betrayer/): Windows-Eventloop und Linux-StatusNotifierItem/DBus-Annahmen.
- [`ort::init_from`-Dokumentation](https://docs.rs/ort/latest/ort/environment/index.html): explizites Laden einer privaten DLL/`.so` über absoluten Anwendungspfad.
- [Microsoft `SendInput`](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput): UIPI-Grenze und nicht eindeutig diagnostizierbare Blockade.
- [Microsoft Clipboard-Verwendung](https://learn.microsoft.com/en-us/windows/win32/dataxchg/using-the-clipboard): Clipboard-Sequenznummern und delayed rendering.
- [X11 ICCCM, Selections/Clipboard](https://www.x.org/docs/ICCCM/icccm.pdf): asynchrone Selection-Ownership statt eines einfachen globalen Werts.
- [`cpal` README](https://docs.rs/crate/cpal/latest/source/README.md): Backend-/Feature-Unterschiede für ALSA, PulseAudio und PipeWire.
