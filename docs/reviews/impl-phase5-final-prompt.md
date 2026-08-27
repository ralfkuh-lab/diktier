Du bist ein delegierter Sub-Agent: nicht orchestrieren, nicht weiterdelegieren — nur diese Aufgabe erledigen.

Aufgabe: EIN Gesamt-Review des Windows-Dev-Milestones von Diktier, Commit HEAD (6f7ca8d) gegen HEAD~1 — Pakete B (Inject) und C (Tray, Single-Instance, Signale, GUI-Subsystem, Autostart) sowie das Fixpaket zu A (Hotkey), das nach deinem Review impl-phase5-A-sol.md entstand.

Kontext vom Auftraggeber: Dieses Review ist bewusst das einzige Zweit-Review für B+C. Ziel ist ein LAUFFÄHIGER Dev-Milestone auf einem Windows-11-Rechner — kein Release. End-to-end funktioniert es dort bereits (F9 → Aufnahme → Paste ctrl_v, reads 1, restore true). Bitte streng priorisieren: Was kann Daten verlieren, hängen, crashen oder Fremd-Anwendungen stören? Randfälle, die nur Politur sind, gehören in „Später", nicht in „Blocker". Keine Testabstraktions- oder Architekturvorschläge.

Lies:
1. `git show HEAD --stat` und `git diff HEAD~1 HEAD -- src/` (Review-Objekt); die Doku-Dateien nur als Kontext.
2. docs/reviews/impl-phase5-B-notes.md und impl-phase5-C-notes.md (Abweichungen/offene Punkte der Implementierer); impl-phase5-A-notes.md Abschnitt „Fixpaket nach Sol-Review".
3. docs/windows-plan.md WP3–WP5; docs/SPEC.md §4.2, §4.3, §5.3, §7, §9, §10.

Schwerpunkte:
- src/inject/windows.rs: Clipboard-Ownership/Generation (`expected_seq`), Delayed Rendering + WM_RENDERFORMAT/WM_RENDERALLFORMATS, Restore-Bedingungen (kann ein fremder Clipboard-Inhalt überschrieben werden?), Pump-Dauerbetrieb, SendInput-Fehlerpfad, GlobalAlloc/Lock-Korrektheit, Drop-Verhalten (eager render), Unwind über WndProc.
- src/tray.rs windows: Version-4-Callbacks, Menüpfad mit SetForegroundWindow gemäß SPEC §4.2-Ausnahme, TaskbarCreated, HICON/HBITMAP-Leaks, Drop/Thread-Affinität, WM_ENDSESSION.
- src/single_instance.rs / download.rs: Mutex-Semantik, Handle-Lebensdauer, Verhalten der cfg-geteilten Tests; Linux-Pfad unverändert? (Achtung: FileLock → PathLock umbenannt — prüfe, ob der Linux-Code noch kompiliert; wir können hier kein Linux-Check fahren.)
- src/main.rs: windows_subsystem + AttachConsole-Reihenfolge, Exitcodes; src/daemon/signals.rs: SetConsoleCtrlHandler.
- src/hotkey.rs Fixpaket: sind deine Blocker 1–4 korrekt umgesetzt? Blocker 5 (Watchdog) ist bewusst zurückgestellt — nur bestätigen, dass der Kommentar jetzt ehrlich ist.

Ergebnis nach docs/reviews/impl-phase5-final-sol.md: Kurzurteil (3 Sätze) · Blocker (nur: Datenverlust, Hänger, Crash, Störung fremder Apps, Linux-Bruch) · Wichtig · Später · Bestätigte Umsetzung der A-Blocker. Je Punkt: Datei:Zeile, Problem, knapper Vorschlag. Keine Code-Änderungen. TUI-Ausgabe knapp.
