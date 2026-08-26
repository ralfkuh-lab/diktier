# Review: Diktier Spec v1.2 (Claude, Orchestrator)

Review-Stand: 2026-08-26. Geprüft: `docs/SPEC.md` v1.2 vollständig, nach
Einarbeitung von `spec-codex.md` und `spec-agy.md`; inklusive
Integrationsprüfung §17 gegen den Fließtext (stichprobenhaft, keine
Abweichung gefunden).

## Kurzfazit

- Die Spec ist in ungewöhnlich gutem Zustand: scharfe, falsifizierbare
  Gates, disziplinierter Scope, beide Vor-Reviews korrekt integriert.
- Nichts blockiert Phase 0/1.
- **Ein Befund muss vor Phase 2 in die Spec** (H1): Das spezifizierte
  Restore-Verhalten enthält einen Datenverlust-Pfad, der der eigenen
  Phase-2-Erwartung („erhöhtes Notepad → Text im Clipboard“)
  widerspricht. Beide Vor-Reviewer haben ihn übersehen.

## Befunde

### H1 — Restore-nach-Timeout löscht das Transkript bei stillem Paste-Fehlschlag

- **Schwere:** Hoch (Datenverlust; Spec widerspricht eigenem Gate)
- **Stelle:** §7.1 Punkt 6; §10; §12 Phase 2
- **Problem:** §7.1 erlaubt Restore „nach mindestens einem bedienten
  Request **oder nach Timeout** ohne Ownership-Verlust“. Der Oder-Zweig
  ist ein Loch: Bei UIPI (erhöhtes Notepad), einem vom Ziel verschluckten
  Chord oder einem falsch geratenen Paste-Shortcut wird das Clipboard nie
  gelesen — der Timeout läuft ab, Ownership/Sequenznummer unverändert,
  Restore überschreibt das Transkript. Ergebnis: nichts gepastet,
  Transkript weg, kein Fehler sichtbar. Das widerspricht direkt der
  Phase-2-Erwartung „erhöhtes Notepad → Text im Clipboard“. Zweite
  Ausprägung: Liest eine langsame App erst nach
  `restore_clipboard_delay_ms`, pastet sie unter Windows den **alten**
  Inhalt.
- **Vorschlag (asymmetrische Regel):** Ohne mindestens einen bedienten
  Read **kein** Restore — Transkript bleibt im Clipboard. Das
  Windows-Analogon zum X11-`SelectionRequest` ist Delayed Rendering
  (`SetClipboardData(CF_UNICODETEXT, NULL)` → `WM_RENDERFORMAT` beim
  tatsächlichen Paste). Clipboard-Manager/Win+V erzeugen
  False-Positive-Reads — dann verhält es sich wie heute; False Negatives
  (weggewischtes Transkript) sind ausgeschlossen. Strikt sicherer.

### H2 — Die PTT-Taste darf die fokussierte Anwendung nie erreichen

- **Schwere:** Hoch
- **Stelle:** §3; §4.4; §12 Phase 2
- **Problem:** Nirgends steht, dass F9 verschluckt wird. `XGrabKey` tut
  das implizit, aber der Windows-`WH_KEYBOARD_LL`-Hook reicht die Taste
  durch, wenn er nicht explizit non-zero zurückgibt — dann bekommt
  VS Code bei jedem Diktat ein F9 = Breakpoint-Toggle.
- **Vorschlag:** Normativ: „Der PTT-Hotkey erreicht die fokussierte
  Anwendung nie.“ Phase-2-Testfall: F9 halten in VS Code → kein
  Breakpoint, kein Zeichen.

### M1 — Modifier-„Wiederherstellen“ riskiert hängende Modifier

- **Schwere:** Mittel
- **Stelle:** §7.1
- **Problem:** Restore ist unbedingt formuliert. Hat der User Shift
  zwischenzeitlich physisch losgelassen, erzeugt das synthetische Down
  einen stuck modifier (logisch gedrückt, physisch nicht).
- **Vorschlag:** Restore nur, wenn die Taste zum Restore-Zeitpunkt
  physisch noch gehalten ist (`GetAsyncKeyState`/`XQueryKeymap`), sonst
  gar nicht. Nicht-Restaurieren heilt sich selbst, Falsch-Restaurieren
  nicht.

### M2 — Gesperrter Desktop: CaptureContext wird auf dem Locker erfasst

- **Schwere:** Mittel (potenziell sicherheitsrelevant)
- **Stelle:** §4.4; §7.3
- **Problem:** Beim verlorenen Release (Desktop gesperrt) feuert der Cap
  während der Sperre — `target_window_id` ist der Locker. Unter X11 ist
  der Screensaver ein normales Fenster: Locker == Locker beim Inject,
  Fokuscheck besteht, `Ctrl+V` geht in den Unlock-Passwortdialog.
- **Vorschlag:** (a) Fenster-ID zusätzlich beim Press erfassen;
  Press-ID ≠ Release-ID → `copy_only` (deckt auch Fensterwechsel während
  der Aufnahme ab). (b) „Kennung nicht ermittelbar (NULL, Secure
  Desktop) = Fokusverlust.“ Beides übernehmen.

### M3 — `transcribing` hat keinen Watchdog; Haswell-Worst-Case

- **Schwere:** Mittel
- **Stelle:** §5.2; §12 Phase 1
- **Problem:** Zeit-Gate erlaubt RTF 2 auf Haswell → 60-s-Diktat ≈ 2 min
  `transcribing` (als Design ok). Aber: Hängt ORT, bleibt der Prozess
  für immer in `transcribing` — Hotkey tot, einziger Ausweg Kill. Das
  5-s-Timeout existiert nur beim Beenden.
- **Vorschlag:** Watchdog max(30 s, 5 × Audiolänge) → `error`, Engine neu
  initialisieren, Retry beim nächsten Press; verspätete Ergebnisse eines
  verworfenen Laufs nie injizieren. Zusätzlich Peak-RSS im Spike auch
  mit einer 60-s-Datei messen.

### M4 — `conhost` in der Ctrl+Shift+V-Liste ist falsch

- **Schwere:** Mittel
- **Stelle:** §7.2
- **Problem:** Windows Terminal bindet Ctrl+V **und** Ctrl+Shift+V auf
  Paste. Klassisches `conhost` kennt Ctrl+Shift+V nicht; dort pastet
  Ctrl+V (Console-Setting bzw. PSReadLine). Auf Win10 läuft PowerShell
  oft conhost-gehostet — ein Pflichtfall der Phase-2-Matrix.
- **Vorschlag:** Ctrl+Shift+V nur für `WindowsTerminal.exe`; `conhost` →
  `ctrl_v`.

### M5 — TrayClick-Stop: Fokus liegt eventuell auf Panel/Taskbar

- **Schwere:** Mittel
- **Stelle:** §4.3; §7.3
- **Problem:** Beim Toggle-Stop per Linksklick ist unter Windows
  womöglich die Taskbar das Vordergrundfenster — `target_window_id` =
  Taskbar; der Fallback-Pfad endet de facto in copy_only oder pastet ins
  Leere. Unter Cinnamon nehmen Panel-Icons meist keinen Fokus.
- **Vorschlag:** TrayClick-Diktate deterministisch als `copy_only`
  spezifizieren (der User ist ohnehin an der Maus) — ein Sonderpfad
  weniger.

### N — Kleinere Punkte

1. **Stille-Gate hängt allein am Modell** (§12): Halluziniert Parakeet
   auf Stille/Rauschen, gälte `parakeet-rs` als „gescheitert“, obwohl ein
   dokumentierter RMS-Silence-Gate in Diktier das sauber löst. Explizit
   erlauben.
2. **Artefaktquelle für Phase 1** (§6.3): Die immutable HF-URL entsteht
   erst im Spike — explizit erlauben, dass Phase 1 mit der Kopie des
   Omarchy-Golden-Sets läuft (Hashes müssen stimmen).
3. **„bauen oder dort testen“** (§3): glibc-Symbolversionierung macht das
   „oder“ hohl — Mint-22-Basis (VM/Container/CI) als Build-Umgebung
   verbindlich machen.
4. **Ein-Writer-Regel vs. CLI** (§10): CLI-Modi loggen nur stderr, nie
   `diktier.log` (Daemon kann parallel laufen).
5. **Restaurierte X11-Selection** (§7.1): nach Restore bis zum
   Ownership-Verlust weiter bedienen; bei Quit verschwindet sie
   (X11-Natur) — ein Satz genügt.
6. **„60-s-Cap“** einmal klarstellen: Cap = `audio.max_duration_secs`
   (1..=60).
7. **`leading_space = true`**: führendes Leerzeichen in leeren Feldern;
   in Shells mit `HISTCONTROL=ignorespace` fällt der Befehl aus der
   History. Default ok, aber dokumentieren.

## So lassen

Fokusregel, Plaintext-only-Restore, Wayland-/Elevation-Ausschluss, Golden
Set mit Hashes, falsifizierbare Gates mit benannten Maschinen,
Phasenreihenfolge STT → Inject → Daemon, Trennung
Audio-Callback/Worker, §5.1-Verträge als v2-Vorbereitung ohne Vorwegbau.

## Ergebnis

H1 + H2 vor Phase 2 in die Spec (H1 ändert §7.1 substanziell), M1–M5 und
N als v1.3-Redaktion in einem Rutsch. Phase 0/1 können sofort starten.
Eingearbeitet als Spec v1.3, Entscheidungen in §18.
