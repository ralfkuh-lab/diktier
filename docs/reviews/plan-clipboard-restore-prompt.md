Du bist ein delegierter Sub-Agent: nicht orchestrieren, nicht weiterdelegieren — nur diese Aufgabe erledigen.

Aufgabe: Review des Plans für einen vollständigen Clipboard-Restore mit Overlay-Hinweis in Diktier (Rust, lokales Push-to-Talk-Diktiertool, ausschließlich Windows 10 22H2+/11 x64). Stand: Commit 56a6a4e plus die noch nicht committete Datei docs/clipboard-restore-plan.md.

Lies in dieser Reihenfolge:
1. docs/clipboard-restore-plan.md (der zu prüfende Plan, v1)
2. src/inject/protocol.rs — `ClipboardSnapshot`, `RestoreSession`, `inject_paste`, `ClipboardHost`
3. src/inject/windows.rs — `read_open_clipboard`, `snapshot_clipboard`, `take_clipboard`, `fill_open_clipboard`, `set_serve_text`, `release_ownership`, WndProc-Handler (`on_render_format`, `on_render_all_formats`, `on_destroy_clipboard`)
4. src/inject/mod.rs — `RestoreDecision`, `InjectOutcome`, die Tests
5. src/state.rs — `InjectReport`, Behandlung von `Event::InjectFinished`, `finish_run`
6. src/daemon/mod.rs — `overlay_visible`, `flush_presentation`; src/daemon/workers.rs — Inject-Worker (`InjectCmd::Paste`), `overlay_loop`, `drain_overlay_commands`
7. src/overlay.rs — Kopfkommentar, Rendering-Vertrag, `draw_card`
8. docs/SPEC.md §2, §4.2, §4.5, §7.1–§7.5, §8, §10

Kontext: Heute sichert Diktier vor dem Einfügen nur `CF_UNICODETEXT`. Screenshots, Dateien, HTML/RTF und Excel-Zellen gehen verloren (Log: 33× „Nicht-Text-Clipboard konnte nicht restauriert werden“). Die in der SPEC versprochenen Tooltips kommen beim Nutzer nie an, sie stehen nur im Log. Der Plan sichert alle Formate, stellt sie eager wieder her, schließt Transkript und Restore aus dem Win+V-Verlauf aus und zeigt bei Verlust eine 3-s-Hinweiskarte im Overlay.

Prüfe kritisch — Fokus auf Win32-Korrektheit, Datenverlust, Rennen und Fokusregel, nicht auf Stil:
- Formatklassen (Leitentscheidung 1): Sind Synthese-Paare, übersprungene und verlorene Formate korrekt und vollständig? Was ist mit `CF_DSPTEXT`, `CF_LOCALE`, `CF_HDROP`-Begleitformaten („Preferred DropEffect“, „Shell IDList Array“, „FileGroupDescriptorW“/„FileContents“), OLE-Formaten („Embed Source“, „Object Descriptor“, „Link Source“, „Native“, „OwnerLink“)? Liefert `GetClipboardData` für OLE-Quellen (OleSetClipboard) HGLOBALs auch für TYMED_ISTREAM/ISTORAGE-Formate?
- „DataObject“/„Ole Private Data“ überspringen: richtig, oder beeinflusst das `OleGetClipboard` bei Zielanwendungen?
- Enumerationsreihenfolge vs. synthetisierte Formate: Liefert `EnumClipboardFormats` synthetisierte Formate mit, und an welcher Position? Folgen für die Restore-Reihenfolge?
- Restore (Leitentscheidung 5/6): Vorab-Allokation, Guard, `expect`-Sequenz, Freigabe nicht platzierter Handles, EMF-Handle-Eigentum. Passt das zu `take_clipboard`/`WM_DESTROYCLIPBOARD`-Guard und zum Delayed-Rendering-Pfad? Rennen zwischen Snapshot und `EmptyClipboard`?
- Eigener Inhalt als Snapshot (Leitentscheidung 6): korrekt für alle Folgen (Restore → nächstes Diktat; NoReadTimeout → nächstes Diktat; CopyOnly → nächstes Diktat)?
- Verlaufsausschluss (Leitentscheidung 7): Reicht `ExcludeClipboardContentFromMonitorProcessing`, oder braucht es `CanIncludeInClipboardHistory`/`CanUploadToCloudClipboard`? Wirkung auf die Read-Erkennung aus §7.1 P7 (Delayed Rendering, `WM_RENDERFORMAT` als Read) — kann der Ausschluss Reads verhindern, die heute ein echtes Einfügen belegen?
- Budgets (128 MiB / 1 s) und die Entscheidung gegen einen vorgezogenen Snapshot: vertretbar? Blockade-Risiko durch hängende Quellen?
- Overlay-Hinweis (Leitentscheidung 8): Zustandsmodell (`runtime.notice`, `overlay_view`, Tick-Ablauf), alle Abbruchpfade (Pause-Discard, Quit, `error`, schnelle Folge-Diktate, TrayClick), Koaleszenz im Worker. Fokusregel §4.2 unverletzt? GDI-Text in premultipliziertes Layered Window: ist der Masken-Ansatz korrekt (Gamma, Alpha)?
- Log (Leitentscheidung 9): §10-konform, reicht es zur Diagnose?
- `output.mode` (Leitentscheidung 10) und Debug-WAV-Ring (11): Randfälle.
- Gates und Tests: Fehlt etwas, um Datenverlust sicher auszuschließen? Ist der `#[ignore]`-Integrationstest sinnvoll?
- Fehlende Arbeitspakete, falsche Reihenfolge, unrealistischer Umfang.
- Die Fragen im Abschnitt „Fragen an das Plan-Review (Sol)“ bitte ausdrücklich beantworten.

Wo du dich auf Microsoft-Doku stützt, nenne die Seite. Wo du etwas nicht sicher weißt, sag das, statt zu raten.

Schreibe das Ergebnis als Markdown nach docs/reviews/plan-clipboard-restore-sol.md mit dieser Struktur:
- Kurzurteil (3 Sätze)
- Blocker (müssen vor Implementierung in den Plan)
- Wichtige Hinweise (sollten rein)
- Kleinigkeiten
- Antworten auf die Fragen 1–6 des Plans
- Offene Fragen an den Autor
Jeder Punkt: Fundstelle (Datei/Abschnitt), Problem, konkreter Vorschlag. Keine Umformulierung des Plans, keine Code-Änderungen an src/ oder docs/ außer der Review-Datei. Halte die TUI-Ausgabe knapp; das Ergebnis zählt nur aus der Datei.
