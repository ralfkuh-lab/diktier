# Umsetzungsnotizen: Clipboard-Restore WP1 + WP2

Stand 2026-09-25, Implementierer Claude Opus 5.5. Basis: HEAD `56a6a4e` plus
die uncommitteten Vorgaben (SPEC v1.8, `docs/clipboard-restore-plan.md` v2,
Sol-Review). Nicht committet. Die ignorierten `clipboard_live_*`-Tests und
`--roundtrip` wurden **nicht** ausgeführt.

Geänderte Dateien: `src/inject/formats.rs` (neu), `src/inject/protocol.rs`,
`src/inject/mod.rs`, `src/inject/fake.rs`, `src/inject/windows.rs`,
`src/daemon/workers.rs` (nur die Logzeilen), `src/main.rs` (CLI, Spike-Ausgabe),
`Cargo.toml` (nur Kommentar, keine Version und kein Feature).
Nicht angefasst: SPEC, Plan, README, `state.rs`, `overlay.rs`, `config.rs`,
`debug_wav.rs`.

## Umgesetzt

### Leitentscheidung 1: Formatmatrix, Nutz- und Begleitformate

- `src/inject/formats.rs::classify(id, raw_name) -> FormatClass` ist die Matrix
  als reine Funktion. Klassen:
  - `Data(Global)`: 1, 4, 5, 6, 7, 8, 11, 12, 13, 15, 16, 17, 0x81 und alle
    registrierten IDs ab 0xC000
  - `Data(Emf)`: 14
  - `Gdi(Dib)`: 2 und 9
  - `Gdi(Emf)`: 3
  - `OleInternal`: „DataObject“ und „Ole Private Data“, Vergleich
    case-insensitiv
  - `NeverCopyable`: 0x80, 0x82, 0x83, 0x8E, 0x200–0x2FF und 0x300–0x3FF
  - `UnknownStandard`: alles andere unter 0xC000, darunter `CF_PENDATA`
- `formats::is_companion` erkennt die Begleitformate: `CF_LOCALE`, „Preferred
  DropEffect“, „Shell Object Offsets“ und die drei Verlaufs- und Cloud-Formate.
- `formats::collect(entries, max_bytes, max_time, elapsed, read)` rechnet
  Matrix und Budget ab. Die Win32-Seite und der Fake teilen sich diese
  Funktion, die Fake-Tests prüfen also die produktive Klassifikation.
  - Nur `Data`-Klassen werden gelesen. GDI-, OLE-, nie kopierbare und
    unbekannte IDs lösen kein `GetClipboardData` aus.
  - Ob ein GDI-Format ersetzt werden kann, wird erst nach allen Lesevorgängen
    entschieden. Ein DIB, das in der Enumeration **hinter** `CF_BITMAP` steht,
    zählt deshalb mit.
- Win32: `windows.rs::enumerate_open_clipboard` erfasst zuerst alle IDs. Vor
  jedem Aufruf steht `SetLastError(ERROR_SUCCESS)`. Das Ende ist `0` mit
  `ERROR_SUCCESS`, jeder andere Code ist ein Fehler. Eine Schranke von 4096
  Formaten verhindert eine Endlosschleife.
- Die Rohnamen liefert `registered_name` (`GetClipboardFormatNameW`, nur ab
  0xC000). Danach liest `read_open_clipboard` → `collect` → `read_format`
  mit `copy_global` bzw. `copy_emf`.
- `copy_global` kopiert nur, wenn `GlobalSize > 0` ist und `GlobalLock`
  gelingt. GDI-Handles erreichen diese Funktion nie.
- Leer ist nur `CountClipboardFormats() == 0` mit `ERROR_SUCCESS`
  (`read_open_clipboard`).

### Leitentscheidung 2: Budgets als weiche Limits

- `formats::MAX_SNAPSHOT_BYTES` = 128 MiB, `MAX_SNAPSHOT_TIME` = 1 s.
- `collect` prüft die Zeit vor jedem weiteren Datenformat. Nach Ablauf zählen
  alle restlichen Formate als `TimeBudget`. Ein Format, dessen Anforderung
  schon lief, bleibt gesichert.
- Das Byte-Budget prüft der Leser **vor** dem Kopieren gegen `remaining`, also
  nach `GlobalSize` bzw. der Größenabfrage von `GetEnhMetaFileBits`. Das Format
  zählt dann als `ByteBudget`, die übrigen werden weiter versucht. Liefert ein
  Leser trotzdem mehr, rechnet `collect` das ebenfalls ab.
- Die Rohdaten liegen als `Rc<Vec<u8>>` (`windows.rs::RawFormat`). Stash,
  eigene Payload und Restore teilen sich so dieselben Bytes, und der Speicher
  wird nicht ein drittes Mal belegt.

### Leitentscheidung 3: zusätzliche Fokusprüfung

- `protocol.rs::inject_paste_inner` prüft den Vordergrund jetzt dreimal: vor
  dem Snapshot, danach (wie bisher) und neu **nach** `become_owner`,
  unmittelbar vor `send_paste_shortcut`.
- Bei einem Wechsel gibt es nur `CopyOnly`. Es gibt kein Key-Event und keine
  Aktivierung, das Transkript liegt schon im Clipboard.

### Leitentscheidung 4: Snapshot-Ausgänge

- `protocol.rs::ClipboardSnapshot { kind: SnapshotKind, report: SnapshotReport }`
  mit den Ausgängen `Empty`, `Formats` und `Unrestorable`
  (`formats::snapshot_kind`). `Formats` verlangt mindestens ein gesichertes
  Nutzformat.
- Der `SnapshotReport` enthält eine Zeile je enumeriertem Format
  (`FormatRow`: ID, bereinigter Name und Ausgang `Saved{bytes,useful}`,
  `Replaced`, `OleDropped` oder `Lost(reason)`), dazu `duration` und `own`.
  `lost()`, `replaced()` und `ole_dropped()` werden daraus abgeleitet.
- Die Rohdaten bleiben im Host (`Win32OutputSink::stash`, `FakeHost::stash`).

### Leitentscheidung 5: `restore_snapshot` und `RestoreResult`

- Neue Trait-Methoden `restore_snapshot(&snapshot, transcript) -> RestoreResult`
  und `discard_snapshot()`. `set_serve_text` und `release_ownership` sind weg.
- Ablauf in `windows.rs::Win32OutputSink::restore_snapshot`:
  1. Vor `OpenClipboard` werden alle Kopien erzeugt (`prepare`: HGLOBAL über
     `alloc_bytes`, EMF über `SetEnhMetaFileBits`), dazu das Fallback
     (`alloc_utf16`) und der Marker. Alle Handles liegen in den RAII-Hüllen
     `OwnedGlobal` (`GlobalFree`) und `OwnedEmf` (`DeleteEnhMetaFile`).
     Scheitert eine Kopie, zählt das Format als Verlust `AllocFailed` in der
     Phase „Wiederherstellen“.
  2. `open_clipboard`, dann werden `owned`, Owner und Sequenz gegen
     `expected_seq` geprüft. Bei einer Abweichung: `CloseClipboard`, Ergebnis
     `Foreign`, und die Handles gibt `Drop` frei.
  3. Guard setzen, dann `restore_open_clipboard`: `EmptyClipboard`, danach
     `place()` je Format in Originalreihenfolge. `place` sichert
     `GetLastError` direkt nach dem Fehlschlag und gibt das Handle erst danach
     frei. Ein gescheitertes Format zählt als `SetFailed(code)`.
  4. Liegt kein Nutzformat, wird noch im geöffneten Clipboard das
     Transkript-Fallback gesetzt (`Placement::Transcript` → `RestoreFailed`).
     Scheitert auch das, entsteht `Placement::Lost` → `RestoreResult::Failed`
     („Zwischenablage leer — Transkript und vorheriger Inhalt verloren“).
  5. `CloseClipboard` läuft auf jedem Pfad.
- `RestoreResult`: `Restored`, `RestoredPartial { lost_save, lost_restore }`,
  `RestoreFailed { lost_restore }`, `Foreign` und `Failed(InjectError)`.
  `inject_paste` macht aus `Failed` ein `Err` (→ `InjectReport::Failed`, Tray
  `error`).
- Neue `RestoreDecision`s (`mod.rs`): `Restored`,
  `RestoredPartial { lost_on_restore: bool }` und `RestoreFailed`.
  - `Restore` bleibt der Zwischenstand von `RestoreSession::decide`. Die Logik
    P5–P7 ist unverändert, `Restore` steht nie im Outcome.
  - `is_restored()` ist wahr für `Restored` und `RestoredPartial`.
- Der `WM_RENDERFORMAT`-Pfad bedient weiter nur `CF_UNICODETEXT`, denn die
  Restore-Daten gehen immer eager zurück. `on_render_format` und
  `on_render_all_formats` nutzen jetzt ebenfalls `place()`.

### Leitentscheidung 6: eigener Inhalt als Snapshot

- `ClipboardState::payload: Payload` mit den Werten `Transcript`,
  `Formats(Vec<RawFormat>)` und `Empty`.
  - `take_clipboard` setzt ihn auf `Transcript`. Das deckt Paste, `CopyOnly`
    und Quit-Materialisierung ab.
  - `restore_snapshot` setzt ihn auf die platzierten Formate, bei
    `RestoreFailed` auf `Transcript` und nach dem Restore von `Empty` auf
    `Empty`.
  - `forget_ownership` leert ihn und gibt so den Speicher frei.
- Ist Diktier noch Owner (Owner und Sequenz), liefert
  `snapshot_clipboard` → `ClipboardState::own_snapshot` den Snapshot, ohne
  `GetClipboardData` aufzurufen. Das Transkript wird dabei aus `serve`
  gebildet.
- Zusatz: `take_clipboard` setzt `state.reads` nach jeder erfolgreichen
  Übernahme auf 0. Grund: Hält Diktier das Clipboard noch, obwohl die
  Sequenz fremd ist, kann ein fremder Snapshot einen eigenen Render
  auslösen. Dieser Render darf nicht als Read des neuen Transkripts zählen
  (P7).

### Leitentscheidung 7: Verlaufs- und Cloud-Ausschluss

- `Win32OutputSink::new` registriert `ExcludeClipboardContentFromMonitorProcessing`
  (`RegisterClipboardFormatW`) → `marker`. `marker_handle()` erzeugt dafür
  ein DWORD 0 als HGLOBAL.
- Den Marker setzen: `fill_open_clipboard` (Transkript delayed und eager, also
  Paste, `CopyOnly`, `NoReadTimeout` und Quit-Materialisierung),
  `restore_open_clipboard` (restaurierter Inhalt und Fallback) sowie
  `place_marker`.
- Trug das Original den Ausschluss schon, kommt er mit zurück und wird nicht
  doppelt gesetzt. `CanIncludeInClipboardHistory` und
  `CanUploadToCloudClipboard` kommen byte-gleich zurück. Laut Microsoft
  überstimmt der Ausschluss beide.
- Das Restore eines leeren Snapshots bekommt keinen Marker, sonst wäre das
  Clipboard nicht leer.

### Leitentscheidung 9: Logzeilen

- `formats::sanitize_name`: nur druckbares ASCII, höchstens 40 Zeichen, alles
  andere wird zu `?`. Auch `"` wird zu `?`, damit die Anführungszeichen im Log
  eindeutig bleiben.
- `FormatRef`-Display: `0x0080` bzw. `0xC0A1 "HTML Format"`.
- `formats::snapshot_log_line`, zum Beispiel: `Clipboard-Snapshot: 4 Formate
  (1 gesichert, 1 ersetzt, 1 OLE), 2,0 kB, 14 ms, verloren 1 · verloren:
  0x0080 [nie kopierbar] · ersetzt: 0x0002 · OLE: 0xC010 "DataObject"`.
  `· eigener Inhalt` kennzeichnet einen Snapshot der eigenen Payload.
- `inject::restore_log` liefert den `restore …`-Teil: `true (restored)`,
  `partial (Sichern: …; Zurückschreiben: …)` bzw. `false (<Grund>)`.
- `workers.rs` schreibt je Paste zwei Zeilen: die Snapshot-Zeile und die
  Paste-Zeile mit `restore_log`. `InjectReport` ist unverändert.

### WP2: `--clipboard-check`

- `main.rs::clipboard_check` → `inject::clipboard_check()`. Diese Funktion
  nutzt einen eigenen Sink und `snapshot_clipboard` und verwirft danach den
  Stash.
- Die Ausgabe (`print_snapshot`) zeigt je Format Nummer, ID, Name,
  Klasse und Größe, nie Inhalte. Standard-IDs erscheinen mit Konstantennamen
  (`formats::standard_name`).
- Exitcode (`snapshot_exit_code`): 0 bei leer oder ganz ohne Verlust, 3 bei
  einem Verlust oder `Unrestorable`, 1 bei einem Fehler.
- `--roundtrip` (`requires = "clipboard_check"`) → `main.rs::clipboard_roundtrip`.
  - Die Funktion fragt `single_instance::acquire_instance_lock` an. Bei
    `Busy` verweigert sie mit Begründung und Exit 1. `CreateMutexW` schließt
    dabei nur das eigene Handle, der Daemon bleibt unberührt. Bei `Held`
    bleibt die Sperre für die ganze Laufzeit bestehen.
  - Danach folgt `inject::clipboard_roundtrip()`: Snapshot → Testtext →
    `restore_snapshot` → `read_raw` (ohne Budget und ohne Abkürzung über die
    eigene Payload) → `formats::compare_roundtrip`. Verglichen werden IDs,
    Reihenfolge und Bytes, zusätzliche Formate werden gemeldet.
  - Ein `Unrestorable`-Inhalt wird nicht angefasst (`NotAttempted`, Exit 3).
  - Nach einem Abbruch oder `RestoreFailed` zeigt die Ausgabe, was jetzt im
    Clipboard liegt.
- Der `--help`-Text warnt: Das Lesen kann bei der Quelle verzögert gerenderte
  Formate anstoßen, der Check soll nicht während eines Diktats laufen,
  `--roundtrip` überschreibt die Zwischenablage, und es gibt keine OLE- oder
  Owner-Aussage. Ein Test prüft diesen Text (`clipboard_check_help_warns`).

### Tests

- `formats.rs`: 18 reine Tests. Sie decken die Matrix ab (alle Klassen und
  Grenzen der Bereiche), Begleitformate, Namensbereinigung, Reihenfolge,
  keine Reads für Nicht-Datenformate, EMF und METAFILEPICT, das Byte-Budget
  (überspringen und weitermachen, Grenze inklusiv), das Zeitbudget,
  `snapshot_kind`, die Logzeilen und `compare_roundtrip`.
- `mod.rs`: alle Fake-Tests aus WP1.
  - vollständiger Restore mit Reihenfolge und Bytes
  - partiell in der Phase „Sichern“ und in der Phase „Wiederherstellen“
  - alle Nutzformate scheitern → `RestoreFailed` mit Transkript
  - auch das Fallback scheitert → `Err`
  - nur Begleitformate → `NoPromise`
  - `Empty` → leeres Clipboard ohne Marker
  - fremder Copy während der Vorbereitung → `ForeignOwner`, nichts angefasst
  - fremde Änderung während der Wartezeit, auf Mehrformat-Inhalt umgestellt
  - eigener Inhalt als Snapshot nach Restore, `NoReadTimeout` und `CopyOnly`,
    jeweils mit `snapshot_reads` unverändert und nur dem Script-Read
  - Byte- und Zeitbudget → partiell
  - Fokuswechsel zwischen `become_owner` und Chord → `CopyOnly` ohne
    Key-Event, auch bei `NULL`-Fenster
  - `restore_log`
- Die bestehenden Tests sind umgestellt: `Restore` → `Restored`,
  `with_takeover_before_release` → `with_foreign_copy_before_restore`.
- `windows.rs::tests::live`: acht Tests `clipboard_live_*` mit `#[ignore]` und
  dem Kommentar, dass sie die echte Zwischenablage überschreiben.
  - Ein eigener Fixture-Owner (`FixtureOwner`, Message-only, `fixture_proc`
    auf demselben Thread) legt ab: Text + `CF_LOCALE` + `CF_DSPTEXT`,
    „HTML Format“, DIBV5, EMF, `CF_HDROP` + „Preferred DropEffect“, ein
    Delayed- und ein `NULL`-Render, eine 3-s-Blockade sowie `Empty`.
  - Geprüft werden jeweils die Formatliste, die Reihenfolge, die Bytes, die
    Verluste und die Ersetzungen.
  - Die Tests sind nur gebaut, nicht ausgeführt.

## Abweichungen vom Plan und von der SPEC

1. **Ort der reinen Funktionen.** Der Plan sieht die Hilfsfunktionen in
   `windows.rs` vor. Sie stehen stattdessen im neuen `src/inject/formats.rs`
   (ohne Win32). Nur so kann der Fake dieselbe Matrix und dieselbe
   Budget-Abrechnung nutzen.
2. **Form des Snapshots.** Statt `Formats { entries, lost, replaced,
   ole_dropped }` gibt es eine Struktur `{ kind, report }` mit einer Zeile je
   Format. `lost`, `replaced` und `ole_dropped` sind Methoden. Inhaltlich
   ist das dasselbe, und die Reihenfolge je Format bleibt erhalten (für die
   CLI).
3. **SYLK, DIF, TIFF, RIFF und WAVE als HGLOBAL, `CF_PENDATA` als Verlust.**
   Die SPEC sagt „weitere Standard-IDs mit HGLOBAL laut Standard Clipboard
   Formats“. Die Microsoft-Seite nennt für diese sechs IDs aber gar keinen
   Handle-Typ (siehe Doku-Abgleich). Ich habe die fünf Datenformate
   aufgenommen: Für sie gilt die allgemeine `GMEM_MOVEABLE`-Regel, und
   gelesen wird ohnehin nur nach geprüfter Speicherklassifikation.
   `CF_PENDATA` gilt konservativ als unbekannt. **Bitte bestätigen oder
   korrigieren.**
4. **`CF_GDIOBJFIRST..LAST` bleibt Verlust**, wie SPEC und Plan es vorgeben.
   Microsoft nennt für diesen Bereich ausdrücklich „a handle allocated by the
   GlobalAlloc function with the GMEM_MOVEABLE flag“. Die Aussagen zur
   Freigabe widersprechen sich allerdings zwischen zwei Seiten. Kopierbar wäre
   der Bereich wohl, aber die SPEC ist verbindlich, deshalb habe ich es nicht
   geändert.
5. **`RestoreFailed` auch ohne `EmptyClipboard`.** Die SPEC kennt
   `RestoreFailed` nur für den Fall „kein Nutzformat platziert“. Ich verwende
   es zusätzlich, wenn **vor** dem Leeren etwas scheitert:
   - `OpenClipboard` bleibt nach allen Versuchen erfolglos, und wir sind noch
     Owner (sonst `Foreign`)
   - das Fallback lässt sich nicht allozieren
   - `EmptyClipboard` scheitert, und die Sequenz ist unverändert

   In allen drei Fällen ist nichts angefasst, das Transkript liegt noch, und
   „nicht wiederhergestellt“ trifft zu. Tray `error` wäre für einen
   vorübergehend blockierten Clipboard-Zugriff zu grob. Scheitert
   `EmptyClipboard` und hat sich die Sequenz **geändert**, ist der Zustand
   unbekannt → `Failed`.
6. **Beim Fallback bleiben gesetzte Begleitformate stehen.** Liegt kein
   Nutzformat, wird das Transkript neben schon platzierte Begleitformate
   gesetzt, etwa `CF_LOCALE` oder „Preferred DropEffect“. Ein zweites
   `EmptyClipboard` habe ich bewusst weggelassen. Allein tragen diese Formate
   keinen Inhalt.
7. **Logformat.** Die Verlustlisten tragen den Grund in eckigen Klammern
   (`0x0080 [nie kopierbar]`). Das Plan-Beispiel hat keinen Grund, §10 erlaubt
   ihn. Die Snapshot-Zeile listet zusätzlich die Verluste, die Ersetzungen und
   die OLE-Formate.
8. **Exitcodes der CLI.** Exit 0 heißt „kein einziger Verlust“. Auch ein
   verlorenes Begleitformat führt also zu 3, weil „alle Nutzformate
   gesichert“ ohne Verlust von Begleitformaten nicht eindeutig messbar ist.
   Ein verweigerter Roundtrip wegen laufendem Daemon gibt Exit 1
   („1 = Fehler“ laut Plan). §9 der SPEC nennt dagegen `2` für
   Bedienfehler, und weder `--clipboard-check` noch Exit 3 stehen in §9.
   **Die SPEC braucht einen Nachtrag in §9** (Orchestrator/WP4).
9. **Kein Snapshot-Log bei `CopyOnly` nach dem Snapshot.** Nur
   `InjectOutcome::Pasted` trägt den `ClipboardReport` (Auftrag). Kippt der
   Fokus nach dem Snapshot, wird der Snapshot verworfen und nicht geloggt.

Weitere Widersprüche zwischen Plan und SPEC habe ich nicht gefunden.

## Doku-Abgleich der Formatmatrix

Die Seiten wurden am 2026-09-25 von einem Recherche-Sub-Agenten per WebFetch
geholt. Die Zitate hat er wörtlich wiedergegeben.

| Punkt | Befund | Quelle |
|---|---|---|
| Handle-Typen CF_DIB/DIBV5 | „A memory object containing a BITMAPINFO …“ bzw. „… BITMAPV5HEADER …“ → HGLOBAL ✔ | [Standard Clipboard Formats](https://learn.microsoft.com/en-us/windows/win32/dataxchg/standard-clipboard-formats) |
| CF_LOCALE | „The data is a handle (HGLOBAL) to the locale identifier (LCID)“ ✔ | ebd. |
| CF_HDROP | „A handle to type HDROP“, das ist in der Praxis ein HGLOBAL mit `DROPFILES`; kein ausdrückliches „HGLOBAL“ | ebd. |
| CF_TEXT, OEMTEXT, UNICODETEXT, DSPTEXT | Die Formatseite nennt keinen Handle-Typ. Die Freigabetabelle führt sie unter `GlobalFree` → HGLOBAL ✔ | [Clipboard Operations](https://learn.microsoft.com/en-us/windows/win32/dataxchg/clipboard-operations) |
| CF_SYLK, DIF, TIFF, RIFF, WAVE, PENDATA | **Kein Handle-Typ genannt**, auch nicht in der Freigabetabelle. Es bleibt nur die allgemeine Regel „If the hMem parameter identifies a memory object, the object must have been allocated … with the GMEM_MOVEABLE flag“ → Abweichung 3 | Standard Clipboard Formats; [SetClipboardData](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setclipboarddata) |
| CF_BITMAP, PALETTE, METAFILEPICT | HBITMAP, „Handle to a color palette“, METAFILEPICT mit HMETAFILE; Freigabe über DeleteObject bzw. DeleteMetaFile → GDI, nie `GlobalLock` ✔ | Standard Clipboard Formats; Clipboard Operations |
| CF_DSPBITMAP, DSPMETAFILEPICT, DSPENHMETAFILE | Freigabe über DeleteObject bzw. DeleteMetaFile → GDI → Verlust ✔ | Clipboard Operations |
| CF_OWNERDISPLAY | „The hMem parameter must be NULL.“ → Verlust ✔ | Standard Clipboard Formats |
| CF_PRIVATEFIRST..LAST | „… not freed automatically; the clipboard owner must free such handles“ → Verlust ✔ | ebd. |
| CF_GDIOBJFIRST..LAST | „… not a handle to a GDI object, but is a handle allocated by the GlobalAlloc function with the GMEM_MOVEABLE flag“. Die Freigabe widerspricht sich zwischen den Seiten → Abweichung 4 | Standard Clipboard Formats; [Clipboard Formats](https://learn.microsoft.com/en-us/windows/win32/dataxchg/clipboard-formats) |
| Synthese | DIB → BITMAP, PALETTE, DIBV5; DIBV5 → BITMAP, DIB, PALETTE; ENHMETAFILE → METAFILEPICT; die Textformate untereinander. „Ersetzbar“ in der Matrix ist damit gedeckt ✔ | Clipboard Formats |
| Synthese: Reihenfolge | „the system first enumerates the format that is on the clipboard, followed by the formats to which it can be converted“ ✔ | Clipboard Formats; [EnumClipboardFormats](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-enumclipboardformats) |
| Synthese: Zeitpunkt | Aus CF_BITMAP werden DIB und DIBV5 „as soon as the clipboard is closed“ gerendert, alle anderen Umwandlungen „upon demand“. DIBV5 → DIB kann in sRGB umrechnen. Deshalb werden alle HGLOBAL-Varianten gesichert, auch synthetisierte | Clipboard Formats |
| Reihenfolge allgemein | „clipboard formats are enumerated in the order they are placed on the clipboard“ → Restore in Originalreihenfolge ✔ | Clipboard Formats |
| Ende der Enumeration | „If there are no more clipboard formats to enumerate, the return value is zero. In this case, the GetLastError function returns the value ERROR_SUCCESS.“ ✔ | EnumClipboardFormats |
| CountClipboardFormats | Ob synthetisierte Formate mitzählen, ist **nicht dokumentiert**. Verwendet wird nur `== 0` | [CountClipboardFormats](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-countclipboardformats) |
| SetClipboardData: Eigentum | Nach Erfolg gehören die Daten dem System („may not write to or free the data“). Nach einem Fehlschlag ist das Eigentum **nicht dokumentiert**. Umgesetzt wie im Bestand: Diktier gibt das Handle selbst frei (RAII) | SetClipboardData |
| GetClipboardData | „The clipboard controls the handle … must not free the handle nor leave it locked … must not use the handle after … CloseClipboard“ → sofort kopieren, `GlobalUnlock` ✔ | [GetClipboardData](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getclipboarddata) |
| EMF | `GetEnhMetaFileBits`: „If lpbBuffer is NULL, the function returns the size“, 0 bei Fehler. Das Handle aus `SetEnhMetaFileBits` wird mit `DeleteEnhMetaFile` gelöscht → nur nach gescheitertem `SetClipboardData` ✔. Die Freigabetabelle nennt für CF_ENHMETAFILE „DeleteMetaFile“, das ist vermutlich eine Ungenauigkeit der Doku | [GetEnhMetaFileBits](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/nf-wingdi-getenhmetafilebits), [SetEnhMetaFileBits](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/nf-wingdi-setenhmetafilebits) |
| Verlaufsausschluss | „Place any data on the clipboard in this format to prevent all clipboard formats being included in the clipboard history or synchronized to the user's other devices.“ → beliebige Daten, ein Format für beides ✔ | Clipboard Formats |
| CF_LOCALE, automatisch | „if it contains CF_TEXT data but no CF_LOCALE data, the system automatically sets the CF_LOCALE format“. Nach dem Restore liegt CF_LOCALE explizit vor, wenn das Original es enumerierte | Standard Clipboard Formats |

Offen, weil in der Doku nicht beantwortet: ob `GetClipboardData` für ein
registriertes Format je etwas anderes als ein HGLOBAL liefern kann. Die
Speicherprüfung (`GlobalSize > 0`, `GlobalLock`) ist die Absicherung, wie im
Plan gefordert.

windows-sys: **Kein neues Feature nötig.**
- `EnumClipboardFormats`, `GetClipboardFormatNameW` und
  `RegisterClipboardFormatW` stehen in `Win32_System_DataExchange`.
- `GetEnhMetaFileBits`, `SetEnhMetaFileBits`, `DeleteEnhMetaFile` und
  `HENHMETAFILE` stehen in `Win32_Graphics_Gdi`. Die Live-Tests brauchen
  daraus zusätzlich `CreateEnhMetaFileW`, `CloseEnhMetaFile` und `Rectangle`.
- `SetLastError` und `ERROR_SUCCESS` stehen in `Win32_Foundation`.

Alle Features sind schon aktiv. Im Cargo.toml-Kommentar ist das ergänzt.

## Gate-Ausgaben (wörtlich)

1. `cargo fmt --check` → keine Ausgabe, `exit 0`
2. `cargo clippy --all-targets -- -D warnings` →
   `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 2.39s`, keine Meldung, `exit 0`
3. `cargo test` →
   `test result: ok. 421 passed; 0 failed; 9 ignored; 0 measured; 0 filtered out; finished in 0.28s`
   (9 ignoriert: `stt_smoke_fixtures` und die acht `clipboard_live_*`)
   `cargo test --no-run` →
   `Finished \`test\` profile [unoptimized + debuginfo] target(s) in 0.21s` /
   `Executable unittests src\main.rs (target\debug\deps\diktier-c0414b08052f6d51.exe)`, `exit 0`.
   `cargo test -- --list --ignored` führt alle acht
   `inject::windows::tests::live::clipboard_live_*` als `test` auf, sie sind
   also im Testbinary gebaut.
4. `cargo test -- --ignored stt_smoke_fixtures` →
   `running 1 test` /
   `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 429 filtered out; finished in 17.35s`
5. `cargo build --release` →
   `Finished \`release\` profile [optimized] target(s) in 15.37s`, dann
   `target/release/diktier.exe --clipboard-check` (nur lesend, einmal):

   ```
   Zwischenablage: 4 Formate · alle Formate gesichert
      #  ID      Name                                        Klasse                        Größe
      1  0x000D  CF_UNICODETEXT                              gesichert (Nutzformat)        68 B
      2  0x0010  CF_LOCALE                                   gesichert (Begleitformat)     4 B
      3  0x0001  CF_TEXT                                     gesichert (Nutzformat)        34 B
      4  0x0007  CF_OEMTEXT                                  gesichert (Nutzformat)        34 B
   Gesichert: 140 B in 4 Formaten, 0 ms
   exit 0
   ```

   `--roundtrip` wurde nicht ausgeführt.

Zum Vergleich: Der letzte Commit (`56a6a4e`) meldet 383 grüne Tests. Jetzt
sind es 421 plus 9 ignorierte.

## Offen, Risiken, Hinweise für WP3

**Was `InjectOutcome::Pasted` jetzt trägt** (`src/inject/mod.rs`):

- `restore: RestoreDecision`. Das ist der Endausgang und reicht WP3 für
  die Hinweistabelle:
  - `NoPromise` → „Zwischenablage nicht gesichert“ (`Unrestorable`)
  - `RestoreFailed` → „Zwischenablage nicht wiederhergestellt“
  - `RestoredPartial { lost_on_restore: false }` → „Nicht alle Formate ließen
    sich sichern“
  - `RestoredPartial { lost_on_restore: true }` → „Nicht alle Formate ließen
    sich zurückschreiben“
  - `NoReadTimeout` → „Einfügen nicht bestätigt“
  - kein Hinweis bei `Restored`, `ForeignOwner` und `Disabled`
- `restored: bool` = `restore.is_restored()`, wie bisher.
- `clipboard: ClipboardReport { snapshot: SnapshotReport, lost_restore }`,
  nur fürs Log. `lost_save()` wird aus dem Snapshot abgeleitet.
- `CopyOnly { reason }` ist unverändert. Der neue B6-Fall (Fokus nach
  `become_owner`) liefert ebenfalls `CopyOnly(FocusChanged/FocusUnknown)`.
- `RestoreResult::Failed` kommt als `Err(InjectError)` aus `paste()`.
  Der Worker macht daraus wie bisher `InjectReport::Failed`.

WP3 muss `InjectReport::Pasted` in `state.rs` um diesen Ausgang erweitern.
Im Worker ist die Stelle `workers.rs` im Paste-Zweig, derzeit
`InjectReport::Pasted` ohne Felder.

**Risiken und Hinweise:**

- **Snapshot-Fehler bricht den Paste ab (Bestand, jetzt mehr Fehlerquellen).**
  Scheitert `CountClipboardFormats` oder `EnumClipboardFormats` mit einem
  echten Fehlercode, oder bleibt `OpenClipboard` im Snapshot erfolglos,
  liefert `snapshot_clipboard` ein `Err`. `inject_paste` bricht dann **vor**
  `become_owner` ab: Tray `error`, das Transkript liegt **nicht** im
  Clipboard. Das Verhalten für `OpenClipboard` war schon vorher so. Die SPEC
  (§7.1.1) sagt nur „Snapshot-Fehler“ und legt keine Folge fest. Zu
  überlegen ist ein Rückfall auf `Unrestorable`: kein Versprechen, aber das
  Transkript wird eingefügt. → Owner-Entscheidung.
- **Hängende Quelle** (F2, akzeptiert): `GetClipboardData` blockiert den
  Inject-Worker. Das Zeitbudget greift erst nach der Rückkehr. Belegen soll
  das `clipboard_live_blocking_source_hits_the_time_budget` (nicht
  ausgeführt).
- **Live-Tests ungeprüft.** Die acht `clipboard_live_*` sind nur kompiliert.
  Unsicher sind vor allem zwei Annahmen:
  - Enumeriert Windows zu einem DIBV5-Fixture wirklich `CF_BITMAP`
    (`clipboard_live_dibv5`)?
  - Enumeriert Windows zu einem EMF wirklich `CF_METAFILEPICT`
    (`clipboard_live_emf`)?

  Beides folgt aus der Synthesetabelle, ist aber nicht gemessen. Laufen
  lassen nur mit Ralfs Zustimmung:
  `cargo test clipboard_live_ -- --ignored --test-threads=1`
- **Byte-Gleichheit und `GlobalSize`.** Gesichert wird die volle
  `GlobalSize`, die laut Microsoft größer sein kann als die angeforderte
  Größe. Zurück kommt ein Block genau dieser Größe. `compare_roundtrip`
  verlangt exakte Gleichheit. Rundet Windows den neuen Block anders auf,
  meldet der Roundtrip „Bytes weichen ab (vorher X, nachher Y)“ mit
  unterschiedlichen Größen, obwohl die Nutzdaten gleich sind. Ob das
  vorkommt, zeigt erst G3.
- **`--clipboard-check` während eines Diktats.** Das Lesen des delayed
  Transkripts im Daemon zählt dort als Read (P7). Der `--help`-Text warnt
  davor, technisch verhindert wird es nicht.
- **README (WP4)** braucht den Abschnitt zu `--clipboard-check`, den
  Exitcodes und den Grenzen, **SPEC §9** die CLI-Zeile und Exit 3
  (Abweichung 8).
- **Log-Auswertung.** `restore true (restored)` bleibt wörtlich erhalten.
  Neu sind `restore partial (…)` und `restore false (Zwischenablage nicht
  wiederhergestellt …)`. Der frühere Text „Nicht-Text-Clipboard konnte nicht
  restauriert werden“ heißt jetzt „Zwischenablage nicht gesichert —
  Transkript liegt in der Zwischenablage“ (`NoPromise`).

## Nacharbeit (2026-09-25)

Auftrag des Orchestrators nach dem Bericht. Akzeptiert sind die Abweichungen
3, 4, 5, 6, 7 und 9. SPEC §9 (Abweichung 8) zieht der Orchestrator selbst
nach, die Exitcodes bleiben 0/3/1. Es gelten dieselben Regeln wie zuvor:
nicht committet, keine Aufrufe, die die Zwischenablage verändern, keine
`clipboard_live_*` ausgeführt.

### Snapshot-Fehler bricht den Paste nicht mehr ab

Der Risikopunkt „Snapshot-Fehler bricht den Paste ab“ weiter oben ist damit
erledigt.

- `formats::LossReason::SnapshotFailed(code)` ist neu, im Log und in der CLI
  erscheint es als `Snapshot-Fehler <code>`.
- `SnapshotReport::failed(code, duration)` erzeugt eine einzige Verlustzeile
  ohne Format-ID (`0x0000`). `SnapshotReport::failure()` liefert den Code
  zurück.
- `protocol::ClipboardSnapshot::failed` = `Unrestorable` mit genau diesem
  Report.
- Win32 (`windows.rs::snapshot_clipboard`):
  - **`OpenClipboard` scheitert:** `try_open_clipboard` liefert den Code des
    letzten Versuchs. Ergebnis ist `ClipboardSnapshot::failed`, und
    `snapshot_seq` wird auf `None` gesetzt: Ohne Versprechen gibt es nichts
    zu schützen, die Übernahme prüft wie bei `copy_only` gegen nichts.
    Danach entscheidet `become_owner` wie bisher; scheitert es auch, bleibt
    es `Err`. `open_clipboard` ist jetzt eine dünne Hülle um
    `try_open_clipboard`.
  - **`CountClipboardFormats`/`EnumClipboardFormats` mit echtem Fehlercode:**
    `read_open_clipboard` und `enumerate_open_clipboard` liefern jetzt
    `Err(u32)`. `CloseClipboard` läuft wie bisher, `snapshot_seq` ist die im
    geöffneten Clipboard gelesene Sequenz, das Ergebnis ist
    `ClipboardSnapshot::failed`. Eine Enumeration über der Schranke von 4096
    Formaten meldet Code `0`. `read_raw` (Roundtrip) macht aus dem Code
    wieder ein `InjectError`.
- Der Paste läuft danach normal weiter: `NoPromise`, das Transkript bleibt
  liegen. Die Snapshot-Logzeile zeigt
  `verloren: 0x0000 [Snapshot-Fehler <code>]`.
- CLI (`main.rs::snapshot_exit_code`): Ein gescheiterter Snapshot ist im
  Daemon `Unrestorable`, für die Diagnose aber ein Fehler → **Exit 1**, nicht
  3. Das gilt auch für den Roundtrip-Zweig `NotAttempted`, der jetzt
  `snapshot_exit_code` nutzt. Die Codes bleiben 0/3/1.
- Fake: `with_snapshot_failure(code)` und `with_failing_become_owner()`. Neue
  Tests in `mod.rs`:
  - `enumeration_error_is_unrestorable_and_pastes_anyway` (1418): `NoPromise`,
    ein Verlust `SnapshotFailed(1418)`, der Chord wird gesendet, das
    Transkript liegt, Logzeile mit `[Snapshot-Fehler 1418]`
  - `open_failure_in_snapshot_is_unrestorable` (5): `NoPromise`, Transkript mit
    Verlaufsausschluss
  - `open_failure_in_snapshot_and_become_owner_is_an_error`: `Err`, kein
    Key-Event, der alte Inhalt bleibt
- Test in `formats.rs`: `failed_snapshot_report_names_the_code`.

### `compare_roundtrip`: `GlobalSize`-Aufrundung

Der Risikopunkt „Byte-Gleichheit und `GlobalSize`“ weiter oben ist damit
erledigt.

- `formats::same_payload(saved, got)`: Die Bytes gelten als gleich, wenn der
  neue Block mindestens so lang ist wie der gesicherte und dessen volle alte
  Länge als Präfix trägt. Ist er kürzer oder weicht er im Präfix ab, bleibt
  es eine Abweichung („Bytes weichen ab: … (vorher X, nachher Y)“).
- Test `roundtrip_bytes_allow_global_size_rounding`: gleich, aufgerundet,
  leer, kürzer, abweichend, verschoben, dazu `compare_roundtrip` mit
  aufgerundetem und mit kürzerem Block.
- Die Live-Tests nutzen `compare_roundtrip` über `assert_identical` und
  übernehmen die Regel damit automatisch.

### Gates nach der Nacharbeit (wörtlich)

1. `cargo fmt --check` → keine Ausgabe, `exit 0`
2. `cargo clippy --all-targets -- -D warnings` →
   `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 2.44s`, keine Meldung, `exit 0`
3. `cargo test` →
   `test result: ok. 426 passed; 0 failed; 9 ignored; 0 measured; 0 filtered out; finished in 0.27s`
   `cargo test --no-run` →
   `Finished \`test\` profile [unoptimized + debuginfo] target(s) in 0.21s` /
   `Executable unittests src\main.rs (target\debug\deps\diktier-c0414b08052f6d51.exe)`, `exit 0`.
   `cargo test -- --list --ignored` zählt 8 `clipboard_live_*`.
4. `cargo test -- --ignored stt_smoke_fixtures` →
   `running 1 test` /
   `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 434 filtered out; finished in 17.49s`

Gate 5 war nicht Teil der Nacharbeit und lief nicht erneut. Die Tests sind
von 421 auf 426 gestiegen: 3 Fake-Tests und 2 in `formats.rs`.

## Nacharbeit 2 (2026-09-25, nach dem Implementierungs-Review durch Sol)

Grundlage ist `docs/reviews/impl-clipboard-wp1-sol.md`. Es gelten dieselben
Regeln wie im ursprünglichen Auftrag: nicht committet, keine Aufrufe, die die
Zwischenablage verändern (kein `clipboard_live_*`, kein `--roundtrip`), kein
herdr. Geändert habe ich nur `src/inject/*` und `src/main.rs`.

**Vorfall:** Ein `cargo fmt -- <dateien>` hat die ganze Crate formatiert und
dabei auch `src/daemon/workers.rs` (WP3, parallel in Arbeit) umformatiert.
Das waren nur Whitespace- und Umbruchänderungen am damaligen WP3-Zwischenstand,
inhaltlich habe ich dort nichts geändert. Der WP3-Agent sollte das wissen.

### 1. Blocker 1: fremder Copy nach gescheitertem Snapshot-Open

- `windows.rs::snapshot_clipboard`: Scheitert `try_open_clipboard`, wird
  trotzdem `GetClipboardSequenceNumber()` gelesen. Das geht ohne geöffnetes
  Clipboard. Der Wert landet in `snapshot_seq`.
- `become_owner` bzw. `copy_transcript` → `take_clipboard` vergleichen ihn wie
  üblich im geöffneten Clipboard. Ein fremder Copy dazwischen führt zu
  `TakeFailure::Foreign`: nichts wird überschrieben, und der Aufrufer bekommt
  den Inject-Fehler „Clipboard zwischenzeitlich fremd geändert“.
- Fake: `take_transcript` prüft die beim Snapshot beobachtete Generation
  (`snapshot_generation`, wird wie `snapshot_seq` verbraucht). Neuer Schalter
  `with_foreign_copy_before_become_owner`.
- Test `foreign_copy_after_failed_snapshot_open_is_not_overwritten`: Der
  Snapshot-Open scheitert, dann kommt ein fremder Copy, dann gelingt der
  Take-Open. Ergebnis: `Err`, der fremde Inhalt bleibt, kein Chord.

### 2. Blocker 2: offenes Delayed-Rendering-Versprechen, an der Ursache

- Neue Trait-Methoden in `protocol.rs::ClipboardHost`:
  - `copy_transcript(text)` setzt das Transkript eager.
  - `materialize_transcript()` macht ein offenes Versprechen eager. Wenn
    Diktier nicht mehr Owner oder der Text schon eager ist, passiert nichts.
  - `history_excluded()` sagt, ob der eigene Inhalt den Verlaufsausschluss
    trägt (siehe Punkt 5).
- `protocol.rs::inject_paste_inner`:
  - Die beiden `CopyOnly`-Pfade vor `become_owner` rufen `copy_transcript` auf
    und setzen damit eager.
  - Der Fokuswechsel nach `become_owner` (vor dem Chord) ruft
    `materialize_transcript` auf.
  - `NoReadTimeout` und eigenes Clipboard: sofort `materialize_transcript`.
  - `NoPromise` und `Disabled` sind **eine bewusste Abweichung vom Wortlaut**:
    Beide Entscheidungen fallen schon *vor* dem ersten Read, also direkt nach
    dem Chord. Ein sofortiges `EmptyClipboard` + `SetClipboardData` würde mit
    dem `OpenClipboard` des Ziels kollidieren, das gerade einfügt, und der
    Paste könnte scheitern. Deshalb wartet `wait_for_first_read` bis zum
    ersten bedienten Read, einem fremden Copy oder dem Ende des
    5-s-Fensters. Ein Read rendert ohnehin eager (`delayed = false`), dann
    ist nichts mehr zu tun. Sonst folgt `materialize_transcript`. Die
    Entscheidung selbst ändert sich dadurch nicht.
  - Folge: Bei `NoPromise`/`Disabled` **ohne** Read ist der Inject-Worker bis
    zu 5 s belegt. Das entspricht dem Verhalten im Restore-Fall, früher kehrte
    er in diesem Fall sofort zurück. **Bitte bestätigen.**
- `windows.rs`:
  - `copy_transcript` = `take_clipboard(text, Fill::Eager, snapshot_seq.take())`.
  - `OutputSink::copy_only` nutzt jetzt `copy_transcript` statt
    `become_owner`, also eager ohne Versprechen.
  - `materialize_transcript` ruft den bestehenden `materialize` auf: Marker,
    Guard und `expect` = `expected_seq`, genau wie im Quit-Pfad.
    - `TakeFailure::Foreign`: nichts tun, der fremde Inhalt bleibt.
    - `TakeFailure::Failed`: Warnung per `eprintln!`. Scheiterte
      `OpenClipboard`, ist nichts angefasst, das Versprechen bleibt offen, und
      der Quit-Pfad (`save_to_clipboard_manager`, `Drop`) versucht es erneut.
- **Wo `WM_RENDERFORMAT` noch gebraucht wird**, steht als Kommentar im
  Modulkopf von `windows.rs`: nur auf dem Paste-Pfad, von `become_owner` bis
  zur Restore-Entscheidung. Dort ist es die Read-Heuristik aus §7.1 P7. Alle
  anderen Pfade setzen eager. `WM_RENDERALLFORMATS` und der Quit-Pfad bleiben
  die letzte Chance für ein Versprechen, das trotz allem offen ist.
- Fake:
  - neues Feld `FakeClipboard::delayed`. Ein Render bzw. `SelectionRequest`
    setzt es auf `false`, jede eager Mutation ebenfalls.
  - neuer Zähler `materializations`
  - neue Schalter `with_foreign_copy_before_materialize` und
    `with_failing_materialize`
- Tests:
  - `no_read_timeout_materializes_the_transcript`: eager, Generation
    fortgeschrieben (`still_owner`), Marker gesetzt, Folgediktat mit eigener
    Payload ohne `snapshot_reads` und nur mit dem Script-Read.
  - `no_promise_materializes_after_the_read_window`: ohne Read eager erst
    nach 5 s. Mit Read ist nichts nötig, und das Fenster läuft nicht aus.
  - `disabled_restore_materializes_too`
  - `copy_only_sets_the_transcript_eager`: direkt eager, ohne
    Materialisierung; gilt auch für den Fokuswechsel nach dem Snapshot.
  - `focus_change_after_become_owner_materializes`
  - `foreign_copy_before_materialization_is_left_alone`
  - `failed_materialization_keeps_the_promise`
- **Nicht umgesetzt (WP4, `workers.rs`):** Der Quit-Pfad soll eine gescheiterte
  Sicherung als eindeutige Warnung loggen („Transkript beim Beenden nicht
  gesichert“) statt als `SAVE_TARGETS: … → Timeout`. Weiterer Vorschlag: Eine
  Sicherung, die an `OpenClipboard` scheitert, vor dem Schließen des
  Owner-Fensters einmal mit kurzer Wartezeit wiederholen. Durch die
  Materialisierung ist dieser Fall jetzt deutlich seltener; er entsteht nur
  noch, wenn beide Versuche am blockierten Clipboard scheitern.

### 3. Blocker 3: Roundtrip ohne Zustandsauskunft

- Neues Feld `windows.rs::Roundtrip::after_error: Option<String>`.
- Scheitert die Nachprüfung (`read_after`) nach dem Restore, gibt
  `clipboard_roundtrip` das Platzierungsergebnis (`restore`,
  `lost_restore`) **und** den Fehler zurück, statt ihn mit `?` weiterzureichen.
  Dasselbe gilt für den Zweig `NotAttempted`.
- `main.rs::clipboard_roundtrip` gibt dann aus:
  `Roundtrip: <Platzierung> — aktueller Inhalt nicht abfragbar: <Fehler>`,
  Exit 1. Den Text für die Platzierung liefert `roundtrip_placement`.

### 4. Hinweis `read_raw`: Nachprüfung nur für gesicherte IDs, mit Budget

- `formats::read_saved(entries, saved, max_bytes, max_time, elapsed, read)`:
  Nur die im Snapshot gesicherten IDs der Datenklassen werden gelesen, mit
  denselben Budgets. Alle anderen IDs erscheinen nur als Metadaten (`None`)
  und damit bei Bedarf als „zusätzlich“. Was das Budget nicht hergibt, bleibt
  `None` → „nicht lesbar“.
- `windows.rs::read_raw` heißt jetzt `read_after(saved)` und nutzt
  `read_saved`. Der Roundtrip übergibt die gesicherten IDs, `NotAttempted`
  eine leere Liste.
- Test `read_saved_only_reads_saved_ids_within_budget`: Ein beim Snapshot
  übersprungenes großes DIB wird nicht angefordert und ergibt keine
  Abweichung. Das Byte- und das Zeitbudget greifen.
- Restrisiko: Liegt ein Snapshot knapp an 128 MiB und rundet `GlobalSize` die
  neuen Blöcke auf, kann das letzte Format „nicht lesbar“ melden. Das wäre
  eine falsch-positive Abweichung, kein Datenverlust.

### 5. Hinweis Marker

- `Win32OutputSink::new`: Scheitert `RegisterClipboardFormatW`, erscheint auf
  stderr die Warnung „Verlaufsausschluss nicht registrierbar (Win32-Fehler …)
  — Transkript und Restore können in Win+V/Cloud landen“. Der Sink wird
  trotzdem angelegt.
- `marker_handle`: Scheitert die Allokation, gibt es eine Warnung je Aufruf.
- `place_marker` gibt jetzt `bool` zurück und warnt bei einem gescheiterten
  `SetClipboardData`.
- `ClipboardState::excluded` wird gesetzt von:
  - `take_clipboard`, über `fill_open_clipboard` → `Ok(bool)`
  - dem Restore, über `Placement::Formats(_, bool)` bzw.
    `Placement::Transcript(bool)`. Hat das Original den Ausschluss schon
    getragen, zählt das als gesetzt.
- Neues Feld `ClipboardReport::history_excluded: bool`. Es wird am Ende von
  `inject_paste` über `ClipboardHost::history_excluded` gefüllt, also nach
  einer eventuellen Materialisierung.
  - Bedeutung: Der eigene Inhalt trägt den Ausschluss.
  - `true` auch dann, wenn kein eigener Inhalt mehr liegt (fremder Copy,
    leeres Clipboard).
- Einschränkung: `eprintln!` erreicht im Daemon ohne Konsole kein Log. Das ist
  im Bestand genauso. Sichtbar wird der Ausfall erst über `history_excluded`
  in der Logzeile (WP4).
- **Nicht umgesetzt (WP4, `workers.rs`):** `history_excluded` in die
  Paste-Logzeile aufnehmen.

### 6. Hinweis Fehlerpfade

- Neue Fake-Schalter `with_failing_empty(sequence_changes)` und
  `with_failing_marker()`. Sie bilden `Placement::NotEmptied` bzw.
  `place_marker == false` aus `windows.rs` nach.
- Tests:
  - `empty_clipboard_failure_without_sequence_change_keeps_the_transcript`:
    `RestoreFailed`, das Transkript liegt, Diktier ist weiter Owner.
  - `empty_clipboard_failure_with_sequence_change_is_an_error`: `Err`.
  - `marker_failure_still_restores`: `Restored`,
    `history_excluded == false`. Gegenprobe ohne Ausfall: `true`.

### 7. Hinweis Live-Tests

- `windows.rs::tests::live::Outcome::assert_identical` prüft zusätzlich, dass
  jede als „ersetzt“ gemeldete ID nach dem Restore in der Formatliste
  auftaucht, also synthetisiert verfügbar ist.
- Die Live-Tests nutzen jetzt `read_after(saved)` statt `read_raw`.
- Die Tests sind nur kompiliert, nicht ausgeführt.

### 8. Kleinigkeit Schranke

- `formats::MAX_ENUMERATED_FORMATS = 0x10000`, also die ID-Domäne. Die alte
  lokale Schranke von 4096 in `windows.rs` ist entfernt.
- `formats::enumerate_ids(next)` ist die Enumeration als reine Funktion, mit
  Erkennung wiederholter IDs per `HashSet`. Eine Wiederholung oder das
  Überschreiten der Schranke gibt `Err(ENUM_INCONSISTENT)` (= 0) zurück, im
  Log erscheint das als `Snapshot-Fehler 0`.
- `windows.rs::enumerate_open_clipboard` nutzt diese Funktion und holt die
  Namen danach, bei weiter geöffnetem Clipboard.
- Tests:
  - `enumeration_stops_at_the_end_and_on_errors`
  - `repeated_ids_end_the_enumeration_with_an_error`: Zyklus 13 → 16 → 13,
    Selbstverweis und 5000 IDs ohne Schranke.

### 9. Kleinigkeit CLI

- `main.rs::print_snapshot`: Die Überschrift lautet jetzt „alle auslesbaren
  Nutzdaten gesichert“.

### Weiter offen für WP4 (`workers.rs`, nicht umgesetzt)

- den Snapshot-Report auch bei `CopyOnly` nach dem Snapshot loggen (heute
  verwirft `inject_paste` ihn; nötig ist dafür ein optionaler Report in
  `InjectOutcome::CopyOnly` oder ein eigener Rückkanal)
- `ClipboardReport::history_excluded` in der Paste-Logzeile
- die Quit-Logzeile bei gescheiterter Sicherung als eindeutige Warnung
  („Transkript beim Beenden nicht gesichert“), siehe Punkt 2

### Gates nach Nacharbeit 2 (wörtlich)

Während der Arbeit kompilierte der Arbeitsbaum zeitweise nicht, wegen
WP3-Zwischenständen in `src/daemon/workers.rs`, `src/overlay/*` und dem
Overlay-Spike in `main.rs`. Ich habe die Gates deshalb **zweimal** gefahren.

**A. Isolierte Kopie.** Sie besteht aus HEAD `56a6a4e` per `git archive`,
dazu:
- `src/inject/*` und `src/main.rs` aus dem Arbeitsbaum
- `workers.rs` = HEAD plus nur der WP1-Hunk mit den Logzeilen
- im Overlay-Spike `show_level()` → `show()` wie in HEAD
- `Cargo.toml` aus HEAD

Eigenes `CARGO_TARGET_DIR` im Scratchpad, `onnxruntime.dll` nach
`debug/lib` kopiert. Diese Kopie prüft WP1/WP2 ohne WP3-Einfluss.

1. `cargo fmt --check` → keine Ausgabe, `exit 0`
2. `cargo clippy --all-targets -- -D warnings` →
   `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 0.26s`, keine Meldung, `exit 0`
3. `cargo test` →
   `test result: ok. 440 passed; 0 failed; 9 ignored; 0 measured; 0 filtered out; finished in 0.24s`
   `cargo test --no-run` →
   `Finished \`test\` profile [unoptimized + debuginfo] target(s) in 0.24s`, `exit 0`.
   `cargo test -- --list --ignored` zählt 8 `clipboard_live_*`.
4. `cargo test -- --ignored stt_smoke_fixtures` →
   `running 1 test` / `test engine::tests::stt_smoke_fixtures ... ok` /
   `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 448 filtered out; finished in 18.24s`

**B. Arbeitsbaum** (WP1/WP2 zusammen mit dem WP3-Stand zum Zeitpunkt des
Laufs):

1. `cargo fmt --check` → `exit 0`
2. `cargo clippy --all-targets -- -D warnings` →
   `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 0.29s`, `exit 0`
3. `cargo test` →
   `test result: ok. 461 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 0.29s`
   (10 ignoriert: 8 `clipboard_live_*`, `stt_smoke_fixtures` und ein
   WP3-PNG-Hilfstest)
   `cargo test --no-run` →
   `Finished \`test\` profile [unoptimized + debuginfo] target(s) in 0.21s`, `exit 0`
4. `cargo test -- --ignored stt_smoke_fixtures` →
   `running 1 test` /
   `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 470 filtered out; finished in 19.69s`

WP1/WP2 hat 14 neue Tests, von 426 auf 440 in der isolierten Kopie:
- 11 Fake-Tests in `mod.rs`
- 3 in `formats.rs` (Enumeration ×2, `read_saved`)

Gate 5 war nicht Teil der Nacharbeit.
