Du bist ein delegierter Sub-Agent: nicht orchestrieren, nicht weiterdelegieren — nur diese Aufgabe erledigen.

Aufgabe: Review des Plans für die Windows-Portierung von Diktier (Rust, Push-to-Talk-Diktiertool).

Lies in dieser Reihenfolge:
1. docs/windows-plan.md (der zu prüfende Plan)
2. docs/SPEC.md — insbesondere §3, §4.3, §4.4, §5, §5.3, §7, §9, §10, §11
3. Die betroffenen Verträge im Code: src/hotkey.rs (Trait HotkeyBackend, Debounce, Modul linux als Referenz), src/inject/protocol.rs (ClipboardHost, inject_paste, RestoreSession), src/inject/mod.rs (OutputSink), src/tray.rs (TrayBackend, IconSet im Modul linux), src/single_instance.rs, src/daemon/signals.rs, src/autostart.rs, src/main.rs (Subsystem/Spike-Modi).

Prüfe kritisch — mit Fokus auf Win32-Korrektheit, nicht auf Stil:
- Widerspricht der Plan an irgendeiner Stelle der SPEC? Wo bleibt er hinter ihr zurück, ohne es zu benennen?
- Threading: Message-Pumps, WH_KEYBOARD_LL-Thread, Clipboard-Owner-Fenster, Zugriff auf HWNDs/HICONs aus fremden Threads, GetForegroundWindow/AttachThreadInput, SendInput vs. Hook (LLKHF_INJECTED), TrackPopupMenu-Fallen.
- Clipboard-Protokoll: Delayed Rendering + WM_RENDERFORMAT als „Read"-Zähler — funktioniert das mit Win+V-History, Clipboard-Managern, Terminals, Electron/VS Code? Wo ist das Restore unsicher? GetClipboardSequenceNumber-Semantik.
- Single-Instance mit Named Mutex; Signale (SetConsoleCtrlHandler, WM_ENDSESSION); windows_subsystem + AttachConsole und die Folgen für --foreground und die Spike-Modi.
- Autostart: Registry-Run-Key vs. Startup-Ordner-.cmd — Bewertung und Empfehlung.
- Fehlende Arbeitspakete, falsche Reihenfolge, unrealistischer Umfang, konkrete API-Fehler (falsche Konstanten, fehlende windows-sys-Features).

Schreibe das Ergebnis als Markdown nach docs/reviews/plan-phase5-sol.md mit dieser Struktur:
- Kurzurteil (3 Sätze)
- Blocker (müssen vor Implementierung in den Plan)
- Wichtige Hinweise (sollten rein)
- Kleinigkeiten
- Offene Fragen an den Autor
Jeder Punkt: Fundstelle (Datei/Abschnitt), Problem, konkreter Vorschlag. Keine Umformulierung des Plans, keine Code-Änderungen. Halte die TUI-Ausgabe knapp; das Ergebnis zählt nur aus der Datei.
