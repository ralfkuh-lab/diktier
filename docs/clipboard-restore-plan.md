# Vollständiger Clipboard-Restore und Overlay-Hinweis (Plan, v2)

Stand: 2026-09-25, v2 nach dem Plan-Review durch Sol
([reviews/plan-clipboard-restore-sol.md](reviews/plan-clipboard-restore-sol.md),
Auftrag [reviews/plan-clipboard-restore-prompt.md](reviews/plan-clipboard-restore-prompt.md)).
Eingearbeitet sind alle sechs Blocker (B1–B6), die Hinweise H1–H11, die
Kleinigkeiten K1–K3 und die Entscheidungen zu den offenen Fragen F1–F3
(Abschnitt „Entscheidungen“). v1 vom selben Tag war der Entwurf ohne
Review.

Auftrag von Ralf am 2026-09-25: „die ganze Zwischenablage wird
gesichert“. Wenn etwas nicht gesichert oder wiederhergestellt werden
konnte, soll nach dem Diktat im Overlay kurz ein Hinweis stehen. Das
Tippen per Tastatur (`output.mode = "type"`) wird **nicht** gebaut, die
Option fliegt raus. Mit im Paket: Debug-Aufnahmen als Ring der letzten
zehn, als Hilfe für die „Herr Präsident“-Halluzination.

**Spec-Status:** Verbindlich ist SPEC v1.7. Betroffen sind §2
(Nicht-Ziel „Verlustfreies Clipboard-Restore für Nicht-Text“), §4.5
(Overlay), §7.1 (Snapshot/Restore), §7.3 (Hinweis bei Fokuswechsel),
§7.5 (`type`), §8 (`output.mode`) und §10 (Debug-WAV, Log-Inhalt).
**Erst der Spec-Nachtrag (WP0 → v1.8), dann der Code.** Unverändert
bleiben §4.2 (Fokusregel) und die Restore-Sicherheitsregeln aus §7.1
P5–P7: nie über eine fremde Änderung hinweg restaurieren, Restore nur
nach bedientem Read, 5-s-Fenster. Ein bedienter Read ist weiterhin eine
Heuristik und kein Beweis für ein Einfügen (H6).

## ✅ Ausgangslage (HEAD `56a6a4e`, 0.3.0)

### Snapshot nur als Unicode-Text

- `src/inject/windows.rs::read_open_clipboard` sichert ausschließlich
  `CF_UNICODETEXT`. Ohne dieses Format heißt das Ergebnis
  `ClipboardSnapshot::NonText`, und es gibt kein Restore-Versprechen
  (`RestoreDecision::NoPromise`, §7.1 P2).
- Liegt Text **mit** weiteren Formaten im Clipboard (HTML/RTF aus Word,
  Browser, Outlook; Excel-Zellen), gehen alle Formate außer dem Text
  verloren. Das Log meldet trotzdem `restore true (restored)`.
- Protokoll: `src/inject/protocol.rs` (`ClipboardSnapshot`,
  `RestoreSession`, `inject_paste`), Fake-Host `src/inject/fake.rs`,
  Tests in `src/inject/mod.rs`.

### Befund im Log (`%LOCALAPPDATA%\diktier\diktier.log`, Stand 2026-09-25)

| Ausgang | Anzahl |
|---|---|
| `restore true (restored)` | 1428 |
| `restore false (Nicht-Text-Clipboard konnte nicht restauriert werden)` | 33 (zuletzt Lauf 685, 2026-09-25) |
| `restore false (Einfügen nicht bestätigt …)` | 4 (alle 2026-09-10) |

Wie viele der 1428 „restored“ in Wahrheit Formate verloren haben, lässt
sich nicht zählen. Ralf vermutet für den jüngsten Fall einen Screenshot
in der Zwischenablage.

### Die versprochenen Hinweise erreichen den Nutzer nicht

§7.1 und §7.3 versprechen Tooltips („Nicht-Text-Clipboard konnte nicht
restauriert werden“, „Einfügen nicht bestätigt — Text liegt in der
Zwischenablage“, „Fokus geändert — Text liegt im Clipboard“). Im Code
kommt davon nichts beim Nutzer an. `InjectReport::Pasted`
(`src/state.rs`) trägt keine Restore-Details. `NoPromise` und
`NoReadTimeout` stehen nur in der Logzeile des Inject-Workers.
`InjectReport::CopyOnly` wird zu `LogEvent::CopyOnlyNotice`, also
ebenfalls nur zu einer Logzeile. Der Tray-Tooltip zeigt Zustand und
Fehlergrund, aber keine Hinweise.

### Weitere Befunde

- `output.mode = "type"` wird gelesen und validiert (`OutputMode::Type`),
  aber nirgends ausgewertet. Wer es einstellt, bekommt still Paste.
- `src/overlay.rs` zeichnet nur Primitive in einen premultiplizierten
  BGRA-Puffer. Die Sichtbarkeit ist zustandsgetrieben:
  `daemon::overlay_visible` → `flush_presentation` →
  `OverlayCmd::Show/Hide` → `overlay_loop`.
- `src/daemon/debug_wav.rs` schreibt genau eine Datei
  `%TEMP%\diktier\last_recording.wav`. Am 2026-09-25 ging so der Beleg
  für die „Herr Präsident“-Halluzination verloren (Lauf 703).
- Für `injecting` gibt es keinen Watchdog. Hängt der Inject-Worker, bleibt
  der Kern in `injecting` (Presses werden ignoriert), bis der Worker
  zurückkehrt. Quit meldet ihn als `stuck` und beendet trotzdem
  (`Daemon::shutdown`).

## Entscheidungen (Antworten auf Sol F1–F3)

- **F1 — Was heißt „vollständig“?** Vollständig heißt: **Alle
  Win32-auslesbaren Nutzdaten** sind wiederhergestellt, Byte für Byte, in
  der Originalreihenfolge. Das Einfügen funktioniert bestmöglich. **Nicht**
  vertraglich erhalten bleiben OLE-Objektsemantik (lebendes
  `IDataObject`, Paste-Link, virtuelle Dateien pro `lindex`, Rückmeldungen
  wie „Performed DropEffect“ an die Quelle) und die Owner-Identität
  (Excel-Laufrahmen). Diese Grenzen stehen in §2 und im README. Ein
  COM-Pfad (`OleGetClipboard` → eigenes `IDataObject` → `OleSetClipboard`)
  ist ein mögliches **Folgepaket**, wenn die Live-Gates zeigen, dass Ralf im
  Alltag daran scheitert (typisch: kopierte Outlook-Elemente).
- **F2 — Hängende Quelle.** Das Risiko wird akzeptiert und dokumentiert.
  Formate werden **nicht** vorsorglich ohne Render-Versuch verworfen. Es
  gibt keine API, die ein delayed gerendertes Format vorab erkennt, und
  pauschales Verwerfen träfe gerade die häufigen Office-Fälle. Die
  Absicherung: Das Zeitbudget ist ein weiches Limit für die **restlichen**
  Formate, Quit beendet den Prozess trotz hängendem Worker (heutiges
  `stuck`-Verhalten), und ein Gate mit blockierender Test-Quelle belegt das
  (G4 #11).
- **F3 — Diagnose-CLI.** `--clipboard-check` ist per Default **nur
  lesend**. Der destruktive Roundtrip braucht ausdrücklich `--roundtrip`
  und verweigert den Start, solange der Daemon läuft (WP2).

## Leitentscheidungen

1. **Formatmatrix statt Synthese-Annahmen (B1, B2, H1, H2).** Beim
   Snapshot werden im geöffneten Clipboard zuerst **alle IDs in
   Enumerationsreihenfolge** erfasst (`EnumClipboardFormats`). Ende der
   Liste heißt `0` **und** `GetLastError() == ERROR_SUCCESS`. Ein anderer
   Fehlercode ist ein Snapshot-Fehler, nicht `Empty`. Erst danach werden
   die Daten abgerufen. Synthetisierte Formate stehen laut Microsoft in der
   Enumeration unmittelbar hinter ihrer Quelle. Diktier unterscheidet sie
   deshalb **nicht** von explizit angebotenen, sondern behandelt jede ID
   nach ihrer Handle-Klasse:

   | Klasse | Formate | Sicherung | Restore | Ergebnis |
   |---|---|---|---|---|
   | HGLOBAL, bekannt | `CF_TEXT` 1, `CF_OEMTEXT` 7, `CF_DIB` 8, `CF_UNICODETEXT` 13, `CF_HDROP` 15, `CF_LOCALE` 16, `CF_DIBV5` 17, `CF_DSPTEXT` 0x81, alle weiteren Standard-IDs mit HGLOBAL laut „Standard Clipboard Formats“ | `GlobalSize`/`GlobalLock` → `Vec<u8>` | neues `GMEM_MOVEABLE` | gesichert, Bytes gleich |
   | HGLOBAL, registriert | `≥ 0xC000`, z. B. „HTML Format“, „Rich Text Format“, „PNG“, „Shell IDList Array“, „Preferred DropEffect“, „FileGroupDescriptorW“ | nur, wenn `GlobalSize` > 0 und `GlobalLock` gelingt (geprüfte Speicherklassifikation); sonst Verlust | neues `GMEM_MOVEABLE` | gesichert, Bytes gleich |
   | EMF | `CF_ENHMETAFILE` 14 | `GetEnhMetaFileBits` | `SetEnhMetaFileBits`; nach **fehlgeschlagenem** `SetClipboardData` `DeleteEnhMetaFile`, nach Erfolg nie | gesichert |
   | GDI, ersetzbar | `CF_BITMAP` 2, `CF_PALETTE` 9 (wenn `CF_DIB` oder `CF_DIBV5` gesichert); `CF_METAFILEPICT` 3 (wenn `CF_ENHMETAFILE` gesichert) | nicht gesichert | Windows synthetisiert aus dem gesicherten Format | **synthetisch ersetzt**: Log ja, Hinweis nein (Grenze in §18) |
   | GDI, nicht ersetzbar | dieselben IDs ohne gesichertes Gegenstück | — | — | Verlust |
   | OLE-intern | „DataObject“, „Ole Private Data“ | nicht kopiert (prozessgebundener Zeiger, OLE-Buchführung) | — | **OLE-Verweis entfällt**: Log ja, Hinweis nein (F1) |
   | nie kopierbar | `CF_OWNERDISPLAY` 0x80, `CF_DSPBITMAP` 0x82, `CF_DSPMETAFILEPICT` 0x83, `CF_DSPENHMETAFILE` 0x8E, `CF_PRIVATEFIRST..LAST` 0x200–0x2FF, `CF_GDIOBJFIRST..LAST` 0x300–0x3FF | — | — | Verlust (Freigabe-Semantik liegt beim Owner, H1) |
   | Lesefehler | `GetClipboardData == NULL`, Lock/Size scheitert, Budget erschöpft | — | — | Verlust |

   Alle HGLOBAL-Formate werden gesichert, **auch** die, die Windows
   synthetisiert haben könnte (`CF_TEXT`, `CF_OEMTEXT`, `CF_DIB` neben
   `CF_DIBV5`). Nach dem Restore liegen sie dann explizit vor, mit genau den
   Bytes, die eine einfügende Anwendung vorher bekommen hätte. Das kostet
   bei Screenshots Speicher (DIB + DIBV5), vermeidet aber jede Annahme
   darüber, welche Instanz das Original war. GDI-Handles werden **nie** per
   `GlobalLock` angefasst. Unbekannte IDs im Standardbereich (`< 0xC000`,
   nicht in der Matrix) zählen konservativ als Verlust. Der Implementierer
   gleicht die Matrix vor WP1 mit „Standard Clipboard Formats“ und
   „Clipboard Formats“ ab. Abweichungen gehen in den Bericht.

   **Nutzformate und Begleitformate (H4).** Begleitformate tragen allein
   keinen einfügbaren Inhalt: `CF_LOCALE`, „Preferred DropEffect“, „Shell
   Object Offsets“, die Verlaufs-/Cloud-Policy-Formate
   (`ExcludeClipboardContentFromMonitorProcessing`,
   `CanIncludeInClipboardHistory`, `CanUploadToCloudClipboard`). Alle
   anderen gesicherten Formate sind Nutzformate. Nur Nutzformate
   entscheiden über den Ausgang (Leitentscheidung 4).

2. **Budgets sind weiche Limits (B5).** `MAX_SNAPSHOT_BYTES = 128 MiB`
   für die Summe der gesicherten Bytes, `MAX_SNAPSHOT_TIME = 1 s`, geprüft
   nach jedem Format. Ein Format, das nicht mehr ins Byte-Budget passt,
   zählt als Verlust, die übrigen werden weiter versucht. Nach Ablauf der
   Zeit zählen alle restlichen Formate als Verlust. Beides ist
   **best effort**, keine harte Schranke. Ein einzelner `GetClipboardData`
   kann nicht abgebrochen werden, und kurzzeitig liegen `Vec` und neue
   Handles doppelt im Speicher (Spitze ≈ 2 × Budget). Ein 4K-Screenshot
   (DIB + DIBV5 ≈ 66 MB, eventuell plus PNG) passt. Für größere Monitore
   gilt das Ergebnis aus G3/G4 und nicht eine Zusage.

3. **Snapshot bleibt, wo er heute ist**, im Inject-Worker direkt vor
   `become_owner`, mit derselben Sequenz-Absicherung (`snapshot_seq`). Ein
   vorgezogener Snapshot beim Aufnahmestart ist nicht Teil des Pakets.
   Die Logzeile liefert die Snapshot-Dauer pro Lauf als Messgrundlage für
   ein mögliches Folgepaket. **Fokus (B6):** Weil der Snapshot jetzt
   merklich dauern kann, prüft `inject_paste` den Vordergrund zusätzlich
   **nach** `become_owner`, unmittelbar vor dem ersten Key-Event. Bei einem
   Wechsel gibt es nur `CopyOnly`, ohne jede Fensteraktivierung.

4. **Ausgänge des Snapshots.** Das plattformneutrale Protokoll kennt:
   - `Empty`: `CountClipboardFormats() == 0`. Restore = leeres Clipboard,
     wie heute.
   - `Formats { entries, lost, replaced, ole_dropped }`: mindestens ein
     **Nutzformat** gesichert. `lost` enthält die Verluste (ID und
     bereinigter Name, Grund, Phase „Sichern“), `replaced` und
     `ole_dropped` nur fürs Log. Restore-Versprechen wie heute.
   - `Unrestorable { lost }`: Formate vorhanden, aber kein Nutzformat
     gesichert, auch dann, wenn Begleitformate gesichert wurden. Kein
     Restore-Versprechen, das Transkript bleibt (heute `NoPromise`).

   Die Rohdaten bleiben plattformspezifisch hinter dem Host. Das Protokoll
   sieht nur Zähler, Bytes und Verlustlisten. Der Fake-Host bildet alle
   drei Ausgänge nach.

5. **Restore mit explizitem Ergebnis und Fallback (B3, H2, H3).** Neue
   Host-Methode `restore_snapshot(&Snapshot) -> RestoreResult` statt
   `set_serve_text`/`release_ownership` (beide haben heute keinen
   Fehler-Rückkanal). Ablauf:
   1. **Vor** `OpenClipboard`: alle HGLOBAL-Kopien und das EMF-Handle
      erzeugen, außerdem das Transkript als Fallback-HGLOBAL (plus
      Policy-Marker). Jedes noch nicht übergebene Handle liegt in einer
      RAII-Hülle (`GlobalFree` bzw. `DeleteEnhMetaFile` im `Drop`).
   2. `OpenClipboard`. Owner **und** Sequenz im geöffneten Clipboard gegen
      `expected_seq` prüfen. Weicht etwas ab: `CloseClipboard`, Ergebnis
      `Foreign`, alle Handles frei, **nichts** angefasst.
   3. Guard setzen, `EmptyClipboard`, dann `SetClipboardData` in
      Originalreihenfolge. Übergebene Handles verlassen die RAII-Hülle.
      Scheitert ein Aufruf, zählt das Format als Verlust in Phase
      „Wiederherstellen“, die übrigen werden weiter gesetzt.
   4. Ist danach **kein Nutzformat** platziert, wird noch im geöffneten
      Clipboard das Transkript-Fallback gesetzt. Ergebnis
      `RestoreFailed`, das Transkript liegt in der Zwischenablage. Scheitert
      auch das, ist das Ergebnis `Failed` („Zwischenablage leer —
      Transkript und vorheriger Inhalt verloren“). Das führt zu
      `InjectReport::Failed` → Tray `error`.
   5. `CloseClipboard` auf **jedem** Pfad, `GetLastError` sofort nach dem
      scheiternden Aufruf sichern.

   `RestoreResult` = `Restored` | `RestoredPartial { lost_save,
   lost_restore }` | `RestoreFailed` | `Foreign` | `Failed(err)`. Daraus
   werden die `RestoreDecision`s `Restored`, `RestoredPartial`,
   `RestoreFailed`, `ForeignOwner` (Rest wie heute). `RestoredPartial`
   entsteht, sobald `lost` aus dem Snapshot oder aus Phase
   „Wiederherstellen“ nicht leer ist. Die Entscheidungslogik
   (`RestoreSession::decide`, P5–P7) bleibt unverändert.
   `WM_RENDERALLFORMATS` betrifft nur noch ein offenes
   Transkript-Versprechen, denn Restore-Daten sind immer eager.

6. **Eigener Inhalt als Snapshot (H4).** `ClipboardState` hält die
   **zuletzt tatsächlich gesetzte** Eigentümer-Payload plus `delayed`:
   - nach `Restored`/`RestoredPartial`: genau die platzierten Formate
     (nicht die ursprüngliche Liste)
   - nach `RestoreFailed`, `NoReadTimeout`, `CopyOnly`, Quit-Materialisierung:
     das Transkript
   - nach Restore von `Empty`: nichts (`Empty`)

   Ist Diktier beim nächsten Snapshot noch Owner (Owner **und** Sequenz),
   dient diese Payload als Snapshot, ohne eigenes `GetClipboardData`, das
   einen eigenen `WM_RENDERFORMAT` auslösen und als Read zählen würde.
   `CopyOnly` bleibt ohne Snapshot und ohne Restore-Zusage.

7. **Verlaufs- und Cloud-Ausschluss (H6).** Transkript (Paste,
   `CopyOnly`, `NoReadTimeout`, Quit-Materialisierung, Fallback aus 5.4)
   **und** wiederhergestellter Inhalt bekommen das registrierte Format
   `ExcludeClipboardContentFromMonitorProcessing` mit einem kleinen
   HGLOBAL. Laut Microsoft („Clipboard Formats“, Abschnitt Cloud Clipboard
   and Clipboard History) genügt es für Windows-Verlauf **und** Cloud.
   Trug das Original eigene Policy-Formate (`CanIncludeInClipboardHistory`,
   `CanUploadToCloudClipboard`), werden sie mit restauriert. Der
   Ausschluss hat Vorrang, weil er beide überstimmt. Das ist gewollt, denn
   das Original steht schon vom ursprünglichen Kopieren im Verlauf.
   Für Drittanbieter-Manager (Ditto u. a.) gibt es keine Zusage. P7 bleibt
   eine Read-Heuristik, auch mit weniger Verlaufs-Reads.

8. **Hinweis im Overlay, zustandsgetrieben (H7, H8, K3).**

   | Ausgang | Zeile 1 | Zeile 2 |
   |---|---|---|
   | `Unrestorable` | Zwischenablage nicht gesichert | Vorheriger Inhalt wurde überschrieben |
   | `RestoreFailed` | Zwischenablage nicht wiederhergestellt | Vorheriger Inhalt wurde überschrieben |
   | `RestoredPartial`, nur Phase „Sichern“ | Zwischenablage teilweise wiederhergestellt | Nicht alle Formate ließen sich sichern |
   | `RestoredPartial`, mit Phase „Wiederherstellen“ | Zwischenablage teilweise wiederhergestellt | Nicht alle Formate ließen sich zurückschreiben |
   | `NoReadTimeout` | Einfügen nicht bestätigt | Text liegt in der Zwischenablage |
   | `CopyOnly(FocusChanged/FocusUnknown)` | Fokus gewechselt – nicht eingefügt | Text liegt in der Zwischenablage |
   | `CopyOnly(TrayClick)` | Text liegt in der Zwischenablage | Mit Strg+V einfügen |

   Kein Hinweis bei `Restored` (auch mit „synthetisch ersetzt“ oder
   „OLE-Verweis entfällt“), `ForeignOwner`, `Disabled` und bei leerem oder
   zu kurzem Transkript. `Failed` läuft über Tray `error`.

   **Kernzustand (normativ):**
   - `runtime.notice = Some((notice, runtime.now + NOTICE_DURATION))`,
     `NOTICE_DURATION = 3 s`, gesetzt **nur** bei `InjectFinished` des
     aktuellen Runs mit hinweispflichtigem Ausgang. `finish_run` ohne
     Inject (leeres Transkript) setzt nichts.
   - Gelöscht wird er bei Ablauf (`Tick`), bei **jedem** akzeptierten
     Aufnahmestart (Hotkey und TrayClick), bei `PauseToggle` in beide
     Richtungen, bei `QuitRequested`, beim Eintritt in `error` und bei
     jedem neuen Run.
   - Ansicht mit fester Priorität: `quitting` → `Hidden`; `error` →
     `Hidden`; `recording`/`transcribing`/`injecting` → `Level` (auch
     pausiert); `idle` mit Hinweis → `Notice(n)` (auch pausiert); sonst
     `Hidden`. (v2-Nachtrag nach WP3: `paused` verbirgt nichts mehr.)
     `overlay_visible` wird zu `overlay_view(runtime) -> OverlayView`.

   **Worker:** `OverlayCmd::View(OverlayView)` statt `Show`/`Hide`. Der
   Worker koalesziert auf die **letzte Ansicht** der Runde, `Shutdown` hat
   Vorrang. `Level → Notice → Level` wechselt nur den Inhalt, ohne
   `hide()` dazwischen und ohne Aktivierung. Ist die Karte beim Hinweis
   nicht sichtbar, positioniert sie sich wie beim Einblenden auf den
   Monitor des Vordergrundfensters. `WS_EX_NOACTIVATE`,
   `SW_SHOWNOACTIVATE`, `SWP_NOACTIVATE`, `WS_EX_TRANSPARENT` und
   `HTTRANSPARENT` gelten unverändert (§4.2).

   **Text:** eigenes Top-down-32-bit-DIB, schwarz initialisiert, GDI
   zeichnet Weiß mit Segoe UI, `ANTIALIASED_QUALITY` (Graustufen, kein
   ClearType), DPI-skaliert, Zeile 1 halbfett, `DT_END_ELLIPSIS` mit
   `DT_SINGLELINE` und fester Rechteckbreite. Die Luminanz (ein Kanal,
   0..255) ist die Deckungsmaske. `Canvas::blend_mask(mask, color)`
   mischt per Source-over in den premultiplizierten Puffer, reine
   Funktion mit Invariante `R, G, B ≤ A`. Die Deckung steigt unter Text,
   das ist korrekt. Links eine Warn-Glyphe in `WAVE_HOT_COLOR` aus
   Primitiven.

   **Fallback (B6):** Scheitert der Textaufbau (Font, DIB, `DrawTextW`),
   zeigt die Karte **nur die Warn-Glyphe** für 3 s (Primitive, kein GDI),
   Log-Warnung, und das Overlay bleibt aktiv. Ist das Overlay per Config
   aus (`[overlay] enabled = false`) oder zur Laufzeit ausgefallen, bleibt
   nur die Logzeile. Die Zusage „Hinweis sichtbar“ gilt ausdrücklich nur
   bei funktionsfähigem Overlay (§4.5).

   Der Mechanismus ist so gebaut, dass WP5 aus `silence-gate-plan.md`
   („nichts erkannt“) ihn später wiederverwenden kann. Nicht Teil dieses
   Pakets.

9. **Log ohne Inhalte (H9).** Pro Paste:
   `Clipboard-Snapshot: 9 Formate (7 gesichert, 1 ersetzt, 1 OLE), 66,4 MB,
   14 ms, verloren 0` und `restore partial (Sichern: 0x0080; Zurückschreiben:
   0xC0A1 "HTML Format")`. Formatnamen werden bereinigt: nur druckbares
   ASCII, höchstens 40 Zeichen, sonst `?`, in Anführungszeichen. Keine
   Blobs, keine Pfade aus `CF_HDROP`, keine Transkripte. Die
   Hinweis-Anzeige bekommt eine eigene Logzeile (`Hinweis: <Fall>`).

10. **`output.mode` erlaubt nur noch `"paste"` (H10).** `"type"` wird
    Fatal mit der Meldung `output.mode "type" gibt es nicht mehr — bitte
    "paste" eintragen oder die Zeile löschen`. Fehlt der Schlüssel, gilt
    `"paste"`. Default-Datei: `mode = "paste"   # v1 nur dieser Wert`.
    `OutputMode::Type` entfällt. Das ist ein bewusster Breaking Change für
    Configs mit `"type"`, dokumentiert in README und §18. Ralfs
    `config.toml` hat `"paste"` und bleibt gültig.

11. **Debug-WAV als Ring der letzten zehn (H9, K2).** Mit
    `DIKTIER_DEBUG_WAV=1` schreibt Diktier
    `%TEMP%\diktier\rec_<UTC bis ms>_lauf-<N>.wav`, z. B.
    `rec_2026-09-25T14-47-13-512Z_lauf-703.wav`. Die Laufnummer gibt der
    Audio-Worker an `dump_debug_wav` weiter, heute bekommt es nur die
    Samples. Temp-Datei mit eindeutigem Namen (`<ziel>.part`), dann
    Rename. Gezählt wird erst nach erfolgreichem Rename. Danach werden die
    ältesten Dateien des **genauen** Musters `rec_*_lauf-*.wav` über zehn
    gelöscht. Fremde Dateien und `.part`-Reste älter als eine Stunde
    werden nicht gezählt, `.part`-Reste werden entfernt. Die Altlast
    `last_recording.wav` wird beim ersten Dump einmalig gelöscht. Der Pfad
    steht weiter als eine Logzeile.

## Arbeitspakete

### ✅ WP0 — Spec-Nachtrag (SPEC v1.8) — **vor** WP1–WP4

Umgesetzt 2026-09-25 (Orchestrator): Kopfzeile, §2, §4.2 (Feedbackkanal
Overlay), §4.3 (TrayClick-Hinweis), §4.5 (Hinweiskarte), §7.1 mit neuem
§7.1.1, §7.3, §7.5, §8, §10, §18 #14.

- §2: „Verlustfreies Clipboard-Restore für Nicht-Text“ ersetzen durch
  die Grenzen aus F1 (OLE-Objektsemantik, virtuelle Dateien, Owner-
  Identität, nie kopierbare Formatklassen).
- §4.5: Hinweiskarte (Tabelle, 3 s, Kernzustand, Priorität, Fallback,
  Zusage nur bei funktionsfähigem Overlay).
- §7.1: P1/P2 neu (Formatmatrix, Nutz-/Begleitformate, Budgets als
  weiche Limits, drei Ausgänge, Restore-Ablauf mit Fallback,
  eigener Inhalt als Snapshot, Verlaufsausschluss). Tooltip-Verweise
  durch „Overlay-Hinweis (§4.5) + Logzeile“ ersetzen. P5–P8 unverändert,
  P7 ausdrücklich als Heuristik.
- §7.3: Tooltip „Fokus geändert“ ebenso; zusätzliche Fokusprüfung
  unmittelbar vor dem Chord.
- §7.5 streichen. §8: `output.mode` nur `"paste"`, Validierungstabelle.
- §10: Debug-WAV-Ring. Log darf Format-IDs, bereinigte Formatnamen,
  Größen und Dauern enthalten.
- §18: Entscheidung #14 mit Anlass (Befund vom 2026-09-25), F1–F3 und
  akzeptierten Restrisiken (hängende Quelle, synthetisch ersetzte
  GDI-Formate, OLE-Semantik, Owner-Identität, Breaking Change `type`).
- Kopfzeile v1.8 mit Verweis auf diesen Plan und das Review.

### ✅ WP1 — Mehrformat-Snapshot und -Restore (`src/inject/`)

Umgesetzt 2026-09-25 (Opus), Bericht [reviews/impl-clipboard-wp1-notes.md](reviews/impl-clipboard-wp1-notes.md) mit Nacharbeit 1 und 2; Review [reviews/impl-clipboard-wp1-sol.md](reviews/impl-clipboard-wp1-sol.md), alle Blocker behoben. Live-Tests `clipboard_live_*` gebaut, noch nicht gelaufen (G1).

- `protocol.rs`: Snapshot-Ausgänge (Leitentscheidung 4),
  `RestoreResult`/`RestoreDecision` (5), `inject_paste` mit
  `restore_snapshot` und zusätzlicher Fokusprüfung vor dem Chord (3).
- `windows.rs`: Snapshot nach 1/2/6, Restore nach 5, Ausschluss nach 7.
  Reine Hilfsfunktionen ohne Win32 und unit-getestet:
  Formatklassifikation (Matrix), Nutz-/Begleitformat, Budget-Abrechnung,
  Namensbereinigung. Jede neue `unsafe`-Stelle mit `// SAFETY:` wie im
  Bestand.
- `fake.rs`: Mehrformat-Inhalte, Lesefehler je Format, scheiterndes
  `SetClipboardData` je Format, fremder Copy zwischen Vorbereitung und
  `OpenClipboard`.
- Fake-Tests:
  - vollständiger Restore mit Reihenfolge
  - partiell in Phase „Sichern“ und in Phase „Wiederherstellen“
  - alle Nutzformate scheitern beim Setzen → Transkript-Fallback liegt
    (`RestoreFailed`)
  - auch der Fallback scheitert → `Failed`
  - nur Begleitformate gesichert → `Unrestorable`
  - `Empty` → leer
  - fremder Copy während der Vorbereitung → `Foreign`, nichts angefasst
  - fremde Änderung während der Wartezeit → nie restaurieren (Bestand,
    umgestellt)
  - eigener Inhalt als Snapshot in allen drei Folgen (nach Restore, nach
    `NoReadTimeout`, nach `CopyOnly`) ohne Read-Zählung
  - Byte-Budget und Zeitbudget → partiell
  - Fokuswechsel zwischen `become_owner` und Chord → `CopyOnly`
- Windows-Integrationstests (`#[ignore]`, manuell, seriell mit
  `--test-threads=1`). Sie überschreiben Ralfs Zwischenablage und laufen
  nur mit seiner ausdrücklichen Zustimmung. Ein eigenes Owner-Fenster mit
  Pump legt Fixtures ab: Unicode-Text + `CF_LOCALE` + `CF_DSPTEXT`, „HTML
  Format“, DIBV5, EMF, `CF_HDROP` + „Preferred DropEffect“, ein
  delayed gerendertes Format, ein Format mit `NULL`-Render. Jeweils
  Snapshot → Transkript → Restore → Vergleich: Formatliste
  (IDs, Reihenfolge) und Bytes je gesichertem Format, erwartete
  Verlust- und Ersetzt-Listen. Dazu ein Test mit einer Quelle, die
  `WM_RENDERFORMAT` 3 s blockiert (Beleg für B5: Dauer im Log,
  restliche Formate nach Zeitbudget verloren).

### ✅ WP2 — CLI `--clipboard-check`

Umgesetzt mit WP1 (gleicher Bericht); `--roundtrip` noch nicht gelaufen (G3).

- **Default nur lesend:** listet pro Format ID, bereinigten Namen,
  Klasse aus der Matrix (gesichert / synthetisch ersetzt / OLE-Verweis /
  Verlust mit Grund) und Größe, **nie Inhalte**. Sonst ändert sich
  nichts. Das Lesen kann delayed gerenderte Formate der Quelle
  anstoßen; das steht in `--help`.
- **`--clipboard-check --roundtrip`:** verweigert den Start, wenn der
  Daemon läuft (Single-Instance-Prüfung), und nennt dann den Grund.
  Sonst: Snapshot → Testtext setzen → Restore → erneut enumerieren und
  vergleichen (IDs, Reihenfolge, Bytes je gesichertem Format). Ausgabe
  „Nutzdaten byte-identisch“ bzw. die Abweichungen. Das ist ausdrücklich
  **keine** Aussage über OLE- oder Owner-Semantik. Bricht der Lauf nach
  `EmptyClipboard` ab, sagt die Ausgabe, was noch im Clipboard liegt.
- Exitcodes: 0 = alle Nutzformate gesichert (bzw. im Roundtrip
  byte-identisch), 3 = partiell oder `Unrestorable`, 1 = Fehler.

### ✅ WP3 — Overlay-Hinweis (`state.rs`, `daemon/`, `overlay.rs`)

Umgesetzt 2026-09-25 (Opus), Bericht [reviews/impl-clipboard-wp3-notes.md](reviews/impl-clipboard-wp3-notes.md). Entscheidung nach dem Bericht: `paused` verbirgt weder Pegel noch Hinweis (SPEC §4.5 angepasst, Umstellung in WP4).

Nach Leitentscheidung 8. Tests:
- `state.rs`: jede Tabellenzeile erzeugt den passenden Hinweis. Kein
  Hinweis bei `Restored`/`ForeignOwner`/`Disabled`/leerem Transkript.
  Ablauf nach 3 s per `Tick`. Löschen bei Hotkey-Press, TrayClick-Start
  (auch während Pause laut §4.3), Pause an/aus, Quit, `error`, neuem Run.
  Ein verspätetes `InjectFinished` eines alten Runs setzt keinen Hinweis.
- `daemon/mod.rs`: `overlay_view` für alle Zustände × Hinweis × paused ×
  quitting (Prioritätsliste).
- `workers.rs`: Koaleszenz auf die letzte Ansicht, `Shutdown` vorrangig,
  `Level → Notice → Level` ohne Hide.
- `overlay.rs`: `blend_mask` (Rand, Maske 0/255/Zwischenwerte,
  `RGB ≤ A`, Deckung steigt), Zwei-Zeilen-Layout bei 96/144/192 dpi und
  winziger Arbeitsfläche innerhalb der Karte, Glyphen-Fallback ohne
  Text.

### ✅ WP4 — `type` entfernen, Debug-WAV-Ring, Doku

Umgesetzt 2026-09-25 (Opus), Bericht [reviews/impl-clipboard-wp4-notes.md](reviews/impl-clipboard-wp4-notes.md).

### ✅ Schluss-Review und Nacharbeit

Sol-Schlussreview [reviews/impl-clipboard-final-sol.md](reviews/impl-clipboard-final-sol.md) (3 Blocker), Nacharbeit [reviews/impl-clipboard-final-fix-notes.md](reviews/impl-clipboard-final-fix-notes.md), Nachkontrolle [reviews/impl-clipboard-final-sol-2.md](reviews/impl-clipboard-final-sol-2.md) (2 Blocker), Nacharbeit 2 im selben Bericht; die Nacharbeit 2 hat der Orchestrator selbst gegengelesen (kein drittes Sol-Review). Stand: 503 Tests grün, stt-smoke grün. Live-Gates G1 (`clipboard_live_*`), G3, G4, G5 offen — mit Ralf in der Woche ab 2026-09-28.

- `config.rs`: Leitentscheidung 10. Tests: `"type"` → Fatal mit
  Meldung, `"paste"` ok, unbekannter Wert Fatal, fehlender Schlüssel =
  `"paste"`, Default-Datei.
- `debug_wav.rs`/`workers.rs`: Leitentscheidung 11. Tests: Dateiname aus
  Zeit und Lauf, Ring (nur eigenes Muster, älteste zuerst, genau zehn),
  `.part`-Reste, Altlast `last_recording.wav`, Schreibfehler zählt nicht.
- README: Troubleshooting „Zwischenablage“ (was wiederhergestellt wird,
  Grenzen aus F1, Hinweis im Overlay, `--clipboard-check`),
  Debug-WAV-Abschnitt, Breaking Change `output.mode`.
- Version **0.4.0** (Cargo, README-Versionszeile). `windows-plan.md`:
  Notiz wie bei WP6.

## Gates (Abnahme)

- **G1** `cargo fmt --check`, `cargo clippy --all-targets -- -D
  warnings`, `cargo test` grün. Die ignorierten Integrationstests aus WP1
  laufen einmal manuell grün, mit Ralfs Zustimmung
  (`cargo test -- --ignored --test-threads=1`).
- **G2** `stt-smoke` unverändert grün (Regressionsschutz).
- **G3** `--clipboard-check` (lesend) und `--roundtrip` bei beendetem
  Daemon, je Quelle ein Protokoll mit Formatliste und Ergebnis:
  1. Screenshot (Win+Shift+S) → alle Nutzformate, byte-identisch
  2. formatierter Text aus Word → dito, OLE-Verweis entfällt (Log)
  3. Zellbereich aus Excel → Ergebnis dokumentieren, Verluste benannt
  4. zwei Dateien im Explorer, **Strg+C** → byte-identisch inkl.
     „Preferred DropEffect“
  5. zwei Dateien im Explorer, **Strg+X** → byte-identisch
  6. reiner Text aus Notepad → byte-identisch
  7. Outlook-Mail in der Liste kopiert → Erwartung partiell oder
     `Unrestorable`, **nie** als vollständig abnehmen
  8. leeres Clipboard über den Test-Owner (`EmptyClipboard`,
     `CountClipboardFormats() == 0`) → `Empty`
- **G4** Daemon live (Ralf, mit 0.4.0 installiert), jeweils funktional:
  1. Screenshot kopieren, in Notepad diktieren, in Paint Strg+V → Bild
     erscheint.
  2. Word-Absatz kopieren, diktieren, in Word Strg+V → Formatierung
     erhalten.
  3. Excel-Zellen kopieren, diktieren, in Excel Strg+V → Zellen, nicht
     Text.
  4. Dateien im Explorer mit Strg+C, diktieren, im Zielordner Strg+V →
     Dateien kopiert, Quelle bleibt.
  5. Dateien mit Strg+X, diktieren, im Zielordner Strg+V → Ergebnis
     dokumentieren (verschoben oder kopiert). Kein Datenverlust an der
     Quelle.
  6. Win+V nach zwei Diktaten: kein Diktat-Eintrag, kein Duplikat.
  7. G3-Fall 7 (Outlook) im Daemon → Hinweiskarte erscheint 3 s.
     Währenddessen im Ziel weitertippen: Fokus bleibt, Eingabe läuft
     weiter.
  8. Während der Hinweis steht, sofort neu diktieren → die Pegelkarte
     ersetzt ihn ohne Flackern.
  9. Fokus während des Diktats wechseln → Hinweis „Fokus gewechselt“.
  10. Einfügen in ein erhöhtes Fenster (Admin-Terminal) → nach 5 s
      Hinweis „Einfügen nicht bestätigt“. Kommt stattdessen ein Restore,
      dokumentieren (Read-Heuristik, P7).
  11. Hängende Quelle (Test-Owner aus WP1 mit 3-s-Block als laufendes
      Hilfsprogramm): Diktat → Log zeigt Dauer und Verluste nach
      Zeitbudget, danach normales Diktat möglich. Quit während des
      Blocks → Prozess endet (Log `stuck: inject`).
  12. Logzeilen: Snapshot-Dauer und -Größe plausibel, keine Inhalte,
      Namen bereinigt.
- **G5** `DIKTIER_DEBUG_WAV=1`: zwölf kurze Diktate → genau zehn Dateien
  des neuen Musters, die zwei ältesten gelöscht, keine
  `last_recording.wav`, Namen passen zu den Logzeilen.

## Risiken

- **Hängende Quelle beim Rendern** (F2): `GetClipboardData` wartet
  synchron. Der Inject-Worker hängt mit, der Kern bleibt in `injecting`,
  neue Diktate werden bis dahin ignoriert. Heute gilt dasselbe für
  `CF_UNICODETEXT`, künftig für mehr Formate. Akzeptiert, Dauer im Log,
  Quit beendet trotzdem. Ein Watchdog für `injecting` ist ein mögliches
  Folgepaket.
- **Latenz.** Große Excel-Bereiche rendern viele Formate, bis zum
  Zeitbudget vor dem Paste. Erst messen, dann eventuell der vorgezogene
  Snapshot als Folgepaket.
- **Speicher.** Spitze ≈ 2 × 128 MiB während des Restores, danach so
  lange, wie Diktier den Inhalt als eigene Payload hält (6).
- **Owner- und OLE-Semantik** (F1): Excel verliert den Laufrahmen,
  Paste-Link und virtuelle Dateien gehen verloren, Cut/Move kann sich
  anders verhalten (G4 #5). Dokumentierte Grenze.
- **Synthetisch ersetzte GDI-Formate:** Bot eine Quelle `CF_BITMAP` oder
  `CF_PALETTE` mit anderen Daten als ihr DIB an, bekommt eine Anwendung
  nach dem Restore die synthetisierte Variante. Selten, dokumentiert,
  im Log sichtbar.
- **Drittanbieter-Clipboard-Manager** können Transkript und Restore
  trotz Ausschluss lesen und speichern (keine Zusage).
