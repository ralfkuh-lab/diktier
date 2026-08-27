# Gesamt-Review Phase 5: Windows-Dev-Milestone

## Kurzurteil

Hotkey, Tray, Named Mutex, Console-Attach und der normale End-to-end-Pfad sind für einen Windows-11-Dev-Milestone weitgehend schlüssig umgesetzt. Der Clipboard-Pfad hat jedoch noch Check-then-act-Rennen und destruktive Fehlerpfade, die fremden Clipboard-Inhalt oder das Transkript löschen können; außerdem umgeht `CTRL_CLOSE_EVENT` den regulären Clipboard-Cleanup. Vor Nutzung außerhalb des nachgewiesenen Happy Paths sollten diese Blocker behoben werden; die übrigen offenen Punkte sind für den Dev-Milestone nachrangig.

## Blocker

- **Fundstelle:** `src/inject/protocol.rs:323-341`, `src/inject/windows.rs:575-596`. **Problem:** Der Windows-Snapshot merkt zwar den Text, aber nicht seine Sequenz für die folgende Übernahme. Kopiert eine fremde Anwendung nach `snapshot_clipboard()`, aber vor `become_owner()`, leert Diktier deren neuen Inhalt und restauriert später den älteren Snapshot; das verletzt §7.1 Punkt 5 und kann fremde Daten verlieren. **Vorschlag:** Beim Snapshot die Windows-Sequenz merken und nach erfolgreichem `OpenClipboard`, unmittelbar vor `EmptyClipboard`, erneut vergleichen; bei Abweichung ohne Mutation abbrechen.

- **Fundstelle:** `src/inject/windows.rs:429-501,607-627,681-703`. **Problem:** Restore, Leer-Restore und Quit-Materialisierung prüfen zuerst `is_still_owner()` und öffnen/leeren das Clipboard erst danach. Ein fremder Copy zwischen Prüfung und `EmptyClipboard` wird damit überschrieben; der Guard unterdrückt anschließend sogar das dabei entstehende `WM_DESTROYCLIPBOARD`. **Vorschlag:** Owner und `expected_seq` innerhalb des geöffneten Clipboards direkt vor jeder destruktiven Übernahme erneut prüfen und bei Abweichung schließen, ohne `EmptyClipboard` aufzurufen; das muss für `set_serve_text`, `release_ownership`, `save_to_clipboard_manager` und `Drop` gelten.

- **Fundstelle:** `src/inject/windows.rs:467-502,518-553,607-617,697-703`. **Problem:** `fill_open_clipboard` ruft `EmptyClipboard` vor `GlobalAlloc` und vor dem falliblen `SetClipboardData` auf. Scheitert danach die eager Ablage, sind vorheriger Inhalt und Transkript bereits gelöscht; beim Restore verschluckt `set_serve_text` den Fehler, und `Drop` zerstört danach das Owner-Fenster. Beim Delayed-Pfad beweist `owner == hwnd` zudem nicht, dass `CF_UNICODETEXT` erfolgreich registriert wurde, weil `SetClipboardData(..., NULL)` immer `NULL` liefert. **Vorschlag:** Eager-Speicher vor `EmptyClipboard` vorbereiten, die Delayed-Registrierung vor dem Schließen über die tatsächliche Formatverfügbarkeit validieren und nach einem Fehler hinter `EmptyClipboard` mindestens den weiterhin gehaltenen Serve-/Transkripttext eager zurücklegen; Restore- und Quit-Fehler dürfen nicht still verworfen werden.

- **Fundstelle:** `src/daemon/signals.rs:52-75`, `src/daemon/mod.rs:423-446`. **Problem:** Bei `CTRL_CLOSE_EVENT` setzt der Handler nur das Flag und kehrt sofort zurück. Windows beendet einen Konsolenprozess nach Rückkehr aus diesem Handler, sodass der reguläre `save_targets`-Pfad typischerweise nicht mehr läuft; ein noch delayed gehaltenes Transkript verschwindet dann mit dem Clipboard-Fenster. **Vorschlag:** Wie in WP5 vorgesehen ein Cleanup-Ack-Event einführen, bei Close höchstens etwa drei Sekunden darauf warten und das Ack erst nach Clipboard-Materialisierung im regulären Shutdown setzen; Ctrl+C/Break bleiben nicht blockierend.

## Wichtig

- **Fundstelle:** `src/tray.rs:850-858`, `src/daemon/workers.rs:1086-1091`. **Problem:** `WM_ENDSESSION(TRUE)` legt nur ein `TrayEvent::Quit` in die lokale Queue und kehrt zurück. Der Daemon sieht es erst bei einem späteren `poll`; für Logoff/Shutdown ist damit nicht garantiert, dass die Clipboard-Materialisierung noch vor Prozessende beginnt. **Vorschlag:** Den Quit-Latch unmittelbar aus dem Session-Ende signalisieren und für diesen Pfad mindestens den Abschluss der Clipboard-Sicherung begrenzt koordinieren, ohne im Tray-`WndProc` auf dessen eigenen Thread-Join zu warten.

- **Fundstelle:** `src/daemon/signals.rs:82-91`. **Problem:** Scheitert `SetConsoleCtrlHandler`, läuft der Daemon nach einer stderr-Meldung weiter; Ctrl+C kann ihn dann hart beenden und denselben Delayed-Clipboard-Verlust auslösen. **Vorschlag:** Den Installationsfehler an den Aufrufer zurückgeben und den Foreground-Start als Laufzeitfehler beenden oder den fehlenden sauberen Signalpfad sichtbar in den Daemon-Fehlerzustand übernehmen.

- **Fundstelle:** `src/tray.rs:894-903`. **Problem:** Scheitert `NIM_ADD` nach `TaskbarCreated` einmal, bleibt der Daemon dauerhaft ohne Tray und damit ohne sichtbaren Quit-/Pause-Kanal; ein weiterer Versuch erfolgt erst nach dem nächsten Explorer-Neustart. **Vorschlag:** Einen begrenzten, verzögerten Wiederholungsversuch vorsehen und dauerhaften Verlust als klaren Tray-Fehler in den Daemon melden.

- **Fundstelle:** `src/single_instance.rs:1-340`, `src/tray.rs`, `src/autostart.rs`; `docs/reviews/impl-phase5-C-notes.md`, „Linux-Gate“. **Problem:** Die Linux-sichtbare Umbenennung und `cfg`-Aufteilung wurden nur auf Windows gebaut; ein konkreter Linux-Bruch ist beim Lesen nicht erkennbar, aber das verbindliche Mint-Gate fehlt. **Vorschlag:** Vor dem nächsten gemeinsamen Milestone mindestens `cargo check` und die Lock-/Tray-/Autostart-Tests auf Mint ausführen.

## Später

- **Fundstelle:** `src/hotkey.rs:1713-1721`; `docs/windows-plan.md`, „Nicht in dieser Phase“. **Problem:** Ein von Windows wegen `LowLevelHooksTimeout` still entfernter Hook bleibt unerkannt; Kommentar und Plan benennen das Restrisiko jetzt korrekt. **Vorschlag:** Den bereits geplanten Liveness-Watchdog vor einem Release ergänzen.

- **Fundstelle:** `src/single_instance.rs:412-421`. **Problem:** Kleinschreibung allein kanonisiert Windows-Pfade nicht; Junctions, `..`, 8.3- und UNC-Aliase können für dasselbe Downloadziel verschiedene Mutexe erzeugen. Der normale App-Pfad ist stabil und daher im Dev-Milestone nicht konkret gefährdet. **Vorschlag:** Vor einem Release den endgültigen Modellpfad normalisieren beziehungsweise die Identität über ein geöffnetes Verzeichnis ableiten.

- **Fundstelle:** `src/autostart.rs:151-166`; `docs/windows-plan.md` WP5/WP6. **Problem:** Die Startup-Ordner-`.cmd` erfüllt die aktuelle SPEC, kann beim Login aber kurz ein Konsolenfenster zeigen und bleibt eine Shell-Zwischenlösung. **Vorschlag:** Wie geplant auf eine `.lnk` per `IShellLinkW`/`IPersistFile` wechseln.

- **Fundstelle:** `src/main.rs:96-169`; `docs/reviews/impl-phase5-C-notes.md`, „Ausgabe geht verloren“. **Problem:** `cmd.exe` wartet bei einem GUI-Subsystem-Programm nicht zuverlässig und reicht einfache Umleitungen nicht wie bei einem Console-Binary durch; Attach und Exitcodes selbst funktionieren. **Vorschlag:** Das konkrete Aufrufmuster mit `Start-Process -Wait` beziehungsweise geeigneter Umleitung in WP6 dokumentieren.

- **Fundstelle:** `src/inject/windows.rs:595-596`; `docs/reviews/impl-phase5-B-notes.md`, „Gesperrter Bildschirm“. **Problem:** `GetForegroundWindow` kann beim gesperrten Desktop ein altes HWND liefern. Der beobachtete `OpenClipboard(ERROR_ACCESS_DENIED)` verhindert aktuell den Paste-Key, daher ist dies kein Dev-Blocker. **Vorschlag:** Vor Release den aktiven Input-Desktop explizit prüfen und Lock als Fokusverlust behandeln.

## Bestätigte Umsetzung der A-Blocker

- **A-Blocker 1 bestätigt:** `src/hotkey.rs:1347-1409,1539-1596` erzwingt die Queue, setzt bei Handshake-Fehler `cancel`, prüft es vor und nach der Hook-Installation und postet nach verspätetem `Ready::Up` gezielt `MSG_STOP`.
- **A-Blocker 2 bestätigt:** `src/hotkey.rs:1278-1316,1442-1450` wertet `UnhookWindowsHookEx` aus, nullt das Handle nur bei Erfolg und bestätigt Remove-Fehler über den Ack-Kanal.
- **A-Blocker 3 bestätigt:** `src/hotkey.rs:1520-1526,1602-1644` markiert das Backend nach Post-/Ack-Fehler irreversibel als defekt, stoppt den Thread und lässt keine späteren Commands mehr zu; verspätete Acks können keinen Folgebefehl bestätigen.
- **A-Blocker 4 bestätigt:** `src/daemon/workers.rs:984-995` meldet einen Resume-Registrierungsfehler als `Msg::HotkeyUnavailable` und beendet den Hotkey-Worker.
- **A-Blocker 5 bewusst offen:** `src/hotkey.rs:1713-1721` behauptet nicht mehr, Thread-/Kanal-Liveness erkenne einen still entfernten Hook, sondern nennt den fehlenden Watchdog ausdrücklich.
