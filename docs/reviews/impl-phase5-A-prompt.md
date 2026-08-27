Du bist ein delegierter Sub-Agent: nicht orchestrieren, nicht weiterdelegieren — nur diese Aufgabe erledigen.

Aufgabe: Code-Review von Paket A der Windows-Portierung (WP1 Cargo/cfg + WP2 Hotkey via WH_KEYBOARD_LL). Der Code liegt uncommittet im Working-Tree.

Lies:
1. `git diff HEAD -- Cargo.toml Cargo.lock src/hotkey.rs` (das ist das Review-Objekt)
2. docs/reviews/impl-phase5-A-notes.md (Implementierer-Notizen inkl. Plan-Abweichungen)
3. docs/windows-plan.md WP1/WP2 und Leitentscheidungen; docs/SPEC.md §4.4, §5 (Windows-Hotkey-Absatz), §5.2, §10
4. Dein eigener Plan-Review docs/reviews/plan-phase5-sol.md, Abschnitte zu WP1/WP2 — prüfe, ob jeder Blocker/Hinweis umgesetzt ist.
5. src/daemon/workers.rs, Hotkey-Worker (Grab/Ungrab-Aufrufe), zur Prüfung der Integration.

Prüfe kritisch, Fokus Win32-Korrektheit und Nebenläufigkeit:
- Hook-Thread-Lebenszyklus: Queue-Erzwingung, Handshake-Timeouts, was passiert bei Timeout (Zombie-Thread? doppelter Hook?), Stop/Join, Drop während laufendem Callback.
- Hook-Proc: Rückgabewerte, CallNextHookEx-Pfade, nCode<0, injected-Flags, WM_SYSKEYDOWN/UP, Auto-Repeat, Release nach Modifierwechsel, accepted_down-Reset bei Ungrab (bleibt dann ein Key „hängen" in der App?), Unwind-Sicherheit über die FFI-Grenze, thread_local-Korrektheit.
- Modifier-Vergleich: L/R-Zusammenfassung, Win-Taste, AltGr (VK_RMENU + VK_LCONTROL!), Lock-Tasten.
- VK-Mapping-Vollständigkeit vs. Config-Validierung in src/config.rs (welche Keys akzeptiert die Config, die das Mapping nicht kennt?).
- Cargo: Features minimal und ausreichend; Lock sauber.
- SAFETY-Kommentare inhaltlich richtig?
- Tests: decken sie die Kernfälle ab, sind sie deterministisch?

Schreibe das Ergebnis nach docs/reviews/impl-phase5-A-sol.md: Kurzurteil (3 Sätze) · Blocker · Wichtige Hinweise · Kleinigkeiten · Bewertung der Plan-Abweichungen aus den Notizen. Je Punkt: Fundstelle (Datei:Zeile), Problem, konkreter Vorschlag. Keine Code-Änderungen. TUI-Ausgabe knapp halten.
