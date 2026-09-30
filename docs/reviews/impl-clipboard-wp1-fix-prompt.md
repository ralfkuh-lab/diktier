Nacharbeit 2 zu WP1/WP2 nach dem Implementierungs-Review durch Sol: docs/reviews/impl-clipboard-wp1-sol.md. Lies es zuerst ganz. Gleiche Regeln wie im ursprünglichen Auftrag (docs/reviews/impl-clipboard-wp1-prompt.md): nicht committen, nichts ausführen, was Ralfs Zwischenablage verändert (keine `clipboard_live_*`, kein `--roundtrip`), kein herdr.

**Wichtig:** Parallel arbeitet ein anderer Agent an WP3 in src/state.rs, src/daemon/mod.rs, src/daemon/workers.rs und src/overlay.rs. Du änderst **nur** src/inject/* und src/main.rs. Braucht ein Punkt eine Änderung in workers.rs, beschreibst du sie im Bericht und setzt sie nicht um.

Umsetzen:

1. **Blocker 1:** Scheitert im Snapshot das `OpenClipboard`, wird trotzdem `GetClipboardSequenceNumber()` gelesen (geht ohne geöffnetes Clipboard) und als `snapshot_seq` gesetzt. `take_clipboard` prüft dann wie üblich im geöffneten Clipboard. Ein fremder Copy dazwischen → `TakeFailure::Foreign` → nichts überschrieben, Inject-Fehler wie bei jeder fremden Änderung vor der Übernahme. Fake-Test: Snapshot-Open scheitert → fremder Copy → Take-Open gelingt → fremder Inhalt bleibt, `Err`.

2. **Blocker 2 an der Ursache:** Ein offenes Delayed-Rendering-Versprechen des Transkripts darf nicht länger leben als nötig.
   - Nach dem Ende der Restore-Wartezeit mit einem Ausgang **ohne** Restore (`NoReadTimeout`, `NoPromise`, `Disabled`) und noch eigenem Clipboard materialisiert `inject_paste` das Transkript sofort eager (mit Marker, Guard, `expect`-Sequenz wie `materialize`).
   - `copy_only` setzt das Transkript direkt eager statt delayed. Dort wird kein Read gebraucht.
   - Scheitert die Materialisierung (Clipboard blockiert), bleibt das Versprechen offen. Eine Log-Warnung über `eprintln!` wie im Bestand, und der bisherige Quit-Pfad bleibt die zweite Chance.
   - Ein fremder Copy dazwischen → nichts tun.
   - Tests (Fake): nach `NoReadTimeout`/`NoPromise`/`CopyOnly` ist der Inhalt eager (`delayed == false` bzw. das Fake-Äquivalent), die Sequenz fortgeschrieben, `still_owner` wahr. Ein Folgediktat sieht die eigene Payload ohne Read-Zählung. Fremder Copy vor der Materialisierung → unverändert.
   - Prüfe, ob `WM_RENDERFORMAT` danach noch für irgendeinen Pfad nötig ist (nur Paste bis zur Entscheidung), und halte das im Kommentar fest.

3. **Blocker 3:** `clipboard_roundtrip` gibt bei einer gescheiterten Nachprüfung (`read_raw` scheitert nach dem Restore) einen eigenen Ausgang zurück: Platzierungsergebnis des Restores + „aktueller Inhalt nicht abfragbar: <Fehler>“. `main.rs` gibt beides aus, Exit 1.

4. **Hinweis read_raw:** Im Roundtrip nach dem Restore nur die im Snapshot **gesicherten** IDs byteweise lesen, mit denselben Budgets. Weitere IDs nur als Metadaten enumerieren (erscheinen als „zusätzlich“). Test für ein im Snapshot übersprungenes großes Format.

5. **Hinweis Marker:** Scheitert `RegisterClipboardFormatW` für den Verlaufsausschluss, eine Warnung auf stderr beim Anlegen des Sinks. Scheitert die Allokation pro Aufruf, ebenfalls eine Warnung. Kein Abbruch. Zusätzlich im `ClipboardReport` ein Feld `history_excluded: bool` (für die Logzeile, die setzt WP4 in workers.rs um).

6. **Hinweis Fehlerpfade:** Fake-Schalter und Tests für `EmptyClipboard` scheitert ohne Sequenzänderung (→ `RestoreFailed`, Transkript liegt) und mit Sequenzänderung (→ `Err`), und für einen Marker-Ausfall (Restore trotzdem ok, `history_excluded == false`).

7. **Hinweis Live-Tests:** `assert_identical` prüft zusätzlich, dass jede als „ersetzt“ gemeldete ID nach dem Restore in der Formatliste auftaucht (synthetisiert). Weiterhin nicht ausführen.

8. **Kleinigkeit Schranke:** `MAX_ENUMERATED_FORMATS` auf die ID-Domäne anheben (Schranke 0x10000) und wiederholte IDs erkennen (Wiederholung = Ende mit Snapshot-Fehler). Test mit einer wiederholten ID im Fake bzw. in der reinen Funktion.

9. **Kleinigkeit CLI:** Überschrift „alle auslesbaren Nutzdaten gesichert“ statt „alle Formate gesichert“.

Nicht umsetzen, nur im Bericht notieren (landet in WP4, workers.rs): Snapshot-Report auch bei `CopyOnly` nach dem Snapshot loggen, `history_excluded` in der Logzeile, Quit-Logzeile bei gescheiterter Sicherung als eindeutige Warnung („Transkript beim Beenden nicht gesichert“).

Gates 1–4 wie im ursprünglichen Auftrag, Befehl und Summary wörtlich. Bericht: Abschnitt „Nacharbeit 2“ in docs/reviews/impl-clipboard-wp1-notes.md, je Punkt Umsetzung mit Datei/Funktion und die Gate-Summaries. TUI-Ausgabe knapp.
