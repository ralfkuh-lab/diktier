# Umsetzungsnotizen: Clipboard-Restore WP3 (Hinweiskarte im Overlay)

Stand 2026-09-25, Implementierer Claude Opus 5.5. Basis: aktueller Working Tree
mit den uncommitteten WP1/WP2-Ergebnissen. Nicht committet. Kein Daemon
gestartet, keine installierte Version angefasst, keine `clipboard_live_*`-Tests
und nichts, was die Zwischenablage verändert. Kein Fenster geöffnet.

Geänderte Dateien:

- `src/state.rs`: Kern, `Notice`, Tests
- `src/daemon/mod.rs`: `OverlayView`, `overlay_view`, `flush_presentation`, Tests
- `src/daemon/workers.rs`: `restore_notice`, `OverlayCmd::View`, Worker, Tests
- `src/daemon/logging.rs`: Logzeile `Hinweis: …`
- `src/daemon/dispatch.rs`: ein Test auf `Pasted { notice: None }` umgestellt
- `src/overlay.rs`: reine Zeichenfunktionen und Tests
- `src/overlay/windows.rs`: GDI-Text, Inhaltswechsel und Tests
- `src/main.rs`: nur `--overlay-test` von `window.show()` auf `window.show_level()` umgestellt
- `Cargo.toml` / `Cargo.lock`: dev-dependency `png = "0.18"`

Nicht angefasst: `src/inject/*` (vor und nach `cargo fmt` per md5 geprüft),
SPEC, Plan, README, Cargo-Version, `config.rs`, `debug_wav.rs`.

## Umgesetzt

### Kern (`src/state.rs`)

- `Notice` mit sieben Fällen nach der Tabelle in SPEC §4.5. `title()` und
  `detail()` liefern die Tabellentexte wörtlich, mit Gedankenstrich „–“ in
  „Fokus gewechselt – nicht eingefügt“. `key()` ist der Kurzname für das Log.
  `Notice::for_copy(CopyReason)` bildet alle `CopyOnly`-Gründe ab:
  - `TrayClickPath` → `TrayCopy`
  - `FocusChanged` und `FocusUnknown` → `FocusChanged`, wie in der
    SPEC-Zeile „Fokuswechsel/-verlust“
- `NOTICE_DURATION = 3 s` und `Runtime::notice: Option<(Notice, Duration)>`.
  Der Ablauf zählt auf der Kern-Uhr: `runtime.now + NOTICE_DURATION`.
- `InjectReport::Pasted { notice: Option<Notice> }`.
- **Setzen** nur im Zweig `InjectFinished` des **aktuellen** Laufs, erst nach
  `finish_run` (`set_notice`). `Pasted { notice: Some }` und jedes `CopyOnly`
  setzen einen Hinweis. `finish_run` ohne Inject setzt nichts, also weder bei
  leerem noch bei zu kurzem Transkript. Ein verspätetes `InjectFinished` landet
  wie bisher in `stale` und setzt nichts.
- **Löschen**:
  - Ablauf in `Tick` bei `now >= until`, ohne Tray-Update und ohne Log
  - `start_recording`, also jeder akzeptierte Hotkey- und TrayClick-Start, auch
    in der Pause
  - `PauseToggle` in beide Richtungen
  - `QuitRequested`
  - `enter_error`
  - jeder neue Lauf: `finish_run`, `RetryRequested`, `start_recording`
- Neues `LogEvent::Notice { notice }` beim Setzen. Die Logzeile lautet
  `Hinweis: <key> — <Zeile 1> / <Zeile 2>`, zum Beispiel
  `Hinweis: no-read-timeout — Einfügen nicht bestätigt / Text liegt in der Zwischenablage`.
  Sie enthält nur die festen Kartentexte, keine Inhalte (§10).

### Inject-Worker (`src/daemon/workers.rs`)

`restore_notice(RestoreDecision) -> Option<Notice>` leitet den Hinweis aus dem
WP1-Ausgang ab:

| `RestoreDecision` | Hinweis |
|---|---|
| `NoPromise` (`Unrestorable`) | `ClipboardNotSaved` |
| `RestoreFailed` | `ClipboardNotRestored` |
| `RestoredPartial { lost_on_restore: false }` | `PartialSave` |
| `RestoredPartial { lost_on_restore: true }` | `PartialRestore` |
| `NoReadTimeout` | `PasteUnconfirmed` |
| `Restored`, `ForeignOwner`, `Disabled` | keiner |
| `Wait`, `Restore` | keiner (Zwischenstände, nie im Ausgang) |

Der Paste-Zweig schickt `InjectReport::Pasted { notice: restore_notice(restore) }`.
Die `CopyOnly`-Zweige bleiben unverändert, den Hinweis dazu bildet der Kern.
Aus `src/inject/` hat nichts gefehlt.

### Ansicht (`src/daemon/mod.rs`)

- `OverlayView { Hidden, Level, Notice(Notice) }` und
  `overlay_view(runtime)` ersetzen `overlay_visible`. Die Priorität steht unter
  Abweichung 1.
- `flush_presentation` hält `overlay_shown: OverlayView` und sendet
  `OverlayCmd::View(..)` nur bei einem Wechsel.

### Overlay-Worker (`src/daemon/workers.rs`)

- `OverlayCmd = View(OverlayView) | Shutdown`, dazu
  `OverlayWorker::set_view`. `drain_overlay_commands` koalesziert auf die
  **letzte** Ansicht der Runde. `Shutdown` und ein abgerissener Kanal haben
  Vorrang und verwerfen alle Ansichten.
- `apply_overlay_view` über den kleinen Trait `OverlaySurface`
  (`show_level`, `show_notice`, `hide`). Das Fenster und der Test-Fake
  implementieren ihn.
  - `hide()` wird **nur** für `Hidden` aufgerufen.
  - `Level ↔ Notice` tauscht nur den Inhalt.
  - Eine unveränderte Ansicht fasst das Fenster nicht an.
- `overlay_loop` merkt sich die aktuelle Ansicht. Bei einem Übergang von
  unsichtbar nach sichtbar loggt er `Overlay sichtbar: …` wie bisher.
  Scheitern Fensterbau oder Anzeige, schaltet er das Overlay wie bisher ab.
  Nach jeder Runde holt er `take_text_warning()` ab und loggt die Warnung
  `Hinweistext nicht darstellbar, nur Warn-Glyphe: …`.

### Zeichnen (`src/overlay.rs`, rein)

- `Canvas::blend_mask(mask, color) -> bool`: ein Byte je Pixel, top-down,
  genau `width × height`, sonst `false` und nichts gezeichnet.
  - Gemischt wird per Source-over über das bestehende `blend`, also
    premultipliziert, mit Klemmung `RGB ≤ A`.
  - Die Deckung steigt unter Text.
- `notice_layout(width, height, dpi) -> NoticeLayout { card, glyph, title, detail }`
  - Glyphe links, 24 px Referenz
  - zwei Zeilen fester Breite, 20 + 2 + 18 px, als Block vertikal zentriert
  - alles DPI-skaliert und mit `clip` auf die Karte beschnitten, damit bei
    winziger Arbeitsfläche nichts negativ wird oder herausläuft
- `draw_warning_glyph`: gleichseitiges Dreieck in `WAVE_HOT_COLOR`, geglättet
  per 4×4-Überabtastung, mit Ausrufezeichen (Strich und Punkt) in
  `NOTICE_MARK_COLOR`, der deckenden Kartenfarbe. Nur Primitive, kein GDI.
- `draw_notice_card(canvas, dpi, text: Option<&[u8]>)`: Karte, Glyphe und,
  wenn vorhanden, die Maske in `NOTICE_TEXT_COLOR`. `None` ist der
  Glyphen-Fallback.

### Fenster (`src/overlay/windows.rs`)

- `render_notice_text(width, height, dpi, title, detail) -> Result<Vec<u8>, _>`:
  - eigenes Top-down-32-bpp-DIB über die bestehende `Surface`, ausdrücklich
    schwarz gefüllt
  - `CreateFontW` mit `ANTIALIASED_QUALITY` in DPI-skalierter Zeichenhöhe
    (14 bzw. 13 px Referenz)
  - Zeile 1 in „Segoe UI Semibold“ mit `FW_SEMIBOLD`, Zeile 2 in „Segoe UI“ mit
    `FW_NORMAL`
  - `SetBkMode(TRANSPARENT)`, dann
    `DrawTextW(DT_SINGLELINE | DT_END_ELLIPSIS | DT_LEFT | DT_VCENTER | DT_NOPREFIX)`
    in die festen Zeilenrechtecke
  - `GdiFlush`, dann Luminanz (Rec. 601, ganzzahlig) → Maske
  - Zeile 1 schreibt GDI weiß, Zeile 2 in Grau `NOTICE_DETAIL_LEVEL = 178`.
    Die Maske dämpft Zeile 2 dadurch von selbst, es bleibt bei einer Farbe.
  - RAII: `Font` gibt im `Drop` per `DeleteObject` frei, `FontSelection`
    selektiert den ursprünglichen Font auf jedem Pfad zurück, bevor die Fonts
    fallen. Alle Fehler (Font, DIB, `SelectObject`, `SetTextColor`,
    `SetBkMode`, `DrawTextW`) enden als `Err`. Jede neue `unsafe`-Stelle trägt
    ein `// SAFETY:`.
- `OverlayWindow` kennt jetzt `Content::Level | Content::Notice { title, detail, mask }`.
  - `show_level()`:
    - unsichtbar: Einblenden wie bisher (`appear`: Monitor des
      Vordergrundfensters, DPI-Probe mit `HWND_TOPMOST` + `SWP_NOACTIVATE`,
      erster Frame, dann `SW_SHOWNOACTIVATE`)
    - aus dem Hinweis heraus: nur Inhaltswechsel mit leerer Waveform, ohne
      `ShowWindow`, ohne Umpositionieren
  - `show_notice(title, detail)`:
    - sichtbar: Maske bauen, dann `present` per `UpdateLayeredWindow`
    - unsichtbar: `appear` wie beim Pegel
    - scheitert der Text: `mask = None` (nur Glyphe) und Warnung für den
      Worker. Das ist **kein** Fehler, das Overlay bleibt aktiv.
  - `frame()` zeichnet im Hinweis-Modus nur nach einer Layoutänderung neu
    (`WM_DPICHANGED`, `WM_DISPLAYCHANGE`, `SPI_SETWORKAREA`) und rastert dann
    auch den Text neu. `apply_pending_layout` liefert dafür jetzt `bool`.
    Der Pegel-Modus läuft unverändert.
- Fokusregel §4.2: Es gibt keine neue Fenster- oder Fokus-API. Unverändert
  gelten `WS_EX_NOACTIVATE | WS_EX_TRANSPARENT`, `HTTRANSPARENT`,
  `SW_SHOWNOACTIVATE` und `SWP_NOACTIVATE`. Das Topmost-Band wird bei jedem
  Einblenden neu behauptet, auch beim Hinweis aus dem Unsichtbaren. Der
  Textaufbau braucht kein Fenster (Memory-DC).

### Tests

- `state.rs`, Tests 71–80:
  - jede Tabellenzeile inklusive `FocusUnknown` und TrayClick, mit
    `now + 3 s`, genau einer Logzeile und `UpdateTray` zuletzt
  - Texte wörtlich wie in der SPEC
  - kein Hinweis bei `Pasted { notice: None }`, leerem und zu kurzem
    Transkript (Hotkey und TrayClick) und bei Inject-Fehler
  - Ablauf: 2999 ms steht der Hinweis noch, nach 3000 ms ist er weg, ohne
    Effekte
  - Löschen:
    - Hotkey-Press; ein ignorierter Press löscht nicht
    - TrayClick-Start, auch pausiert (Hinweis aus einem Tray-Diktat während
      der Pause)
    - Pause an und aus
    - Quit und `error`
    - der nächste Lauf erbt nichts
  - verspätetes `InjectFinished`, auch mitten in der nächsten Aufnahme
  - Test 50 um die neue Logzeile ergänzt
- `daemon/mod.rs`:
  - `overlay_view` über 8 Hinweiszustände (keiner plus sieben) × 11 Zustände
    × paused × quitting, also 352 Kombinationen, gegen eine unabhängig
    formulierte Prioritätsliste
  - dazu die Eckpunkte
  - ein Durchlauf über den echten Kern:
    `Hidden → Level → Notice → Level → Notice → Hidden`, ohne `Hidden`
    zwischen Pegel und Hinweis
- `workers.rs`:
  - `restore_notice` für alle zehn `RestoreDecision`s
  - Koaleszenz auf die letzte Ansicht, auch mit Hinweisen
  - `Shutdown` hat Vorrang, ein abgerissener Kanal wirkt wie `Shutdown`
  - `Level → Notice → Level → Level → Notice → Hidden → Notice` am Fake:
    genau ein `hide`, und zwar nur für `Hidden`; die doppelte Ansicht bleibt
    ohne Aufruf
  - ein Fensterfehler kommt beim Worker an
- `overlay.rs`:
  - `blend_mask`: Maske 0 lässt das Pixel stehen, 255 deckt voll,
    128 anteilig; Randpixel links oben und rechts unten; eine falsch
    dimensionierte Maske zeichnet nichts
  - über der Karte steigt die Deckung monoton, `RGB ≤ A` für alle 256
    Maskenwerte
  - Zwei-Zeilen-Layout bei 96/144/192 dpi: Innenabstand, Reihenfolge, feste
    Breite, Zentrierung
  - winzige Arbeitsfläche (vier Fälle bis 1×1): nichts außerhalb, volle
    Maske bleibt premultipliziert
  - Glyphen-Fallback: orange Glyphe, Ausrufezeichen, Textzeilen nur
    Kartenfarbe; mit Maske Text
  - Glyphe bleibt in ihrer Fläche, auch am Rand und bei entarteter Fläche
- `overlay/windows.rs` (GDI ohne Fenster, laufen normal mit):
  - Text liegt nur in den beiden Zeilenrechtecken; Zeile 1 erreicht mindestens
    250, Zeile 2 höchstens ihr Grau; Graustufen-Kanten vorhanden (96/144/192)
  - überlanger Text bleibt innerhalb der Zeilenbreite
- `logging.rs`: die neue Logzeile im Beschreibungs-Test.

Hilfstest (ignoriert): `overlay::windows::tests::notice_card_png_snapshots`.

## Abweichungen und Widersprüche

1. **Priorität der Ansicht bei `paused` (Widerspruch in der SPEC).** §4.5
   (v1.8) und der Plan listen `quitting`, `error`, `paused` → verborgen **vor**
   `recording`/`transcribing`/`injecting` → Pegel. Wörtlich genommen
   verschwände die Pegelkarte bei einer TrayClick-Aufnahme während der Pause.
   §4.3 lässt diese Aufnahme ausdrücklich zu. Das widerspricht:
   - §4.5 Absatz 1: „Während `recording`, `transcribing` und `injecting` zeigt
     …“
   - dem bisherigen, getesteten Verhalten: „Der Pausezustand ändert daran
     nichts“

   Umgesetzt ist deshalb: `quitting` → `error` → aktive Zustände (Pegel, auch
   pausiert) → `paused` → Hinweis in `idle` → verborgen. `paused` verbirgt
   damit nur noch den Hinweis. **Bitte bestätigen**, sonst genügt es, in
   `overlay_view` eine Zeile umzustellen und den Test anzupassen.
2. **Hinweis aus einem Tray-Diktat während der Pause.** Nach der SPEC ist er
   verborgen (`paused` → verborgen). Der Kernzustand setzt ihn trotzdem, und
   die Logzeile erscheint. Gezeigt wird dem Nutzer nichts, obwohl „Text liegt
   in der Zwischenablage – Mit Strg+V einfügen“ gerade da hilfreich wäre.
   Umgesetzt ist die SPEC. Ob der Hinweis auch pausiert erscheinen soll, ist
   eine Owner-Entscheidung.
3. **Logzeile beim Setzen, nicht beim Zeigen.** Der Auftrag sagt „je gezeigtem
   Hinweis“. Die Zeile kommt aus dem Kern (`LogEvent::Notice`) und erscheint
   beim Setzen. Nur so entsteht sie auch bei `[overlay] enabled = false` oder
   ausgefallenem Overlay, wie Plan und SPEC es verlangen („sonst bleibt die
   Logzeile“). Folgen:
   - Ein Hinweis, den ein Press in derselben Event-Runde sofort wieder löscht,
     wird geloggt, aber nie gezeigt.
   - Beim `copy_only` stehen zwei Zeilen im Log, die bisherige
     `Text liegt in der Zwischenablage (…)` und die neue `Hinweis: …`.
4. **Zeile 2 gedämpft über die Maske.** Zeile 2 schreibt GDI in Grau (178)
   statt Weiß. Mit einer Maske und einer Farbe wirkt sie dadurch gedämpfter.
   Plan und SPEC verlangen das nicht, sie verbieten es auch nicht.
5. **Schrift für Zeile 1: „Segoe UI Semibold“** statt „Segoe UI“ mit
   `FW_SEMIBOLD`. GDI führt Semibold als eigene Familie. So kommt die echte
   Schriftdatei zum Zuge statt eines künstlichen Fettdrucks. Die PNGs zeigen
   das erwartete Schriftbild.
6. **`--overlay-test`** ruft jetzt `show_level()` auf, weil `show()` in
   `show_level()`/`show_notice()` aufgeteilt ist. Das ist die einzige Zeile in
   `main.rs`.
7. **dev-dependency `png = "0.18"`**, erlaubt laut Auftrag. `0.18.1` statt
   `0.17`: `0.17` hätte `bitflags 1.x` zusätzlich gezogen und bestehende
   Lock-Einträge umbenannt. Mit `0.18` ist der `Cargo.lock` rein additiv
   (neun neue Pakete, nur im Testbuild).

## Gate-Ausgaben (wörtlich)

1. `cargo fmt --check` → keine Ausgabe, `exit 0`. Vorher hat `cargo fmt` fünf
   Test-Asserts umgebrochen; `src/inject/*`, `main.rs`, `config.rs` und
   `debug_wav.rs` sind laut md5 unverändert.
2. `cargo clippy --all-targets -- -D warnings` →
   `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 5.37s`,
   keine Meldung
3. `cargo test` →
   `test result: ok. 461 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 0.28s`
   (10 ignoriert: `stt_smoke_fixtures`, die acht `clipboard_live_*`, der
   PNG-Hilfstest)
4. `cargo test -- --ignored stt_smoke_fixtures` →
   `running 1 test` /
   `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 470 filtered out; finished in 19.97s`
5. `cargo build --release` →
   `Finished \`release\` profile [optimized] target(s) in 16.27s`

Hilfstest `cargo test notice_card_png_snapshots -- --ignored` →
`test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 470 filtered out; finished in 1.20s`.
Er legt je Datei alle sieben Hinweise und als letzte Zeile den
Glyphen-Fallback ab, über Mittelgrau komponiert:

- `target/overlay-notice/notice-96dpi.png` (416×648)
- `target/overlay-notice/notice-144dpi.png`
- `target/overlay-notice/notice-192dpi.png`

Sichtprüfung bei 96 und 144 dpi: Beide Zeilen passen ohne Ellipse, auch die
längste („… ließen sich zurückschreiben“), die Umlaute sind korrekt, die
Glyphe ist orange mit dunklem Ausrufezeichen.

### Nachprüfung nach WP1-Nacharbeit 2

Der Orchestrator hat gemeldet, dass sich `src/inject/*` und `src/main.rs`
geändert haben und `workers.rs` nachformatiert wurde. Danach sind alle Gates
auf dem neuen Stand noch einmal gelaufen. Mein Stand ist erhalten:
`show_level()` in `main.rs`, `restore_notice` und `OverlayCmd::View` in
`workers.rs`.

1. `cargo fmt --check` → keine Ausgabe, `exit 0`
2. `cargo clippy --all-targets -- -D warnings` →
   `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 0.27s`,
   keine Meldung
3. `cargo test` →
   `test result: ok. 461 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 0.25s`
4. `cargo test -- --ignored stt_smoke_fixtures` → `running 1 test` /
   `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 470 filtered out; finished in 17.30s`
5. `cargo build --release` →
   `Finished \`release\` profile [optimized] target(s) in 0.20s`

Den PNG-Hilfstest habe ich neu erzeugt, er ist wieder grün.

## Offen und Hinweise für den Live-Test G4

- **G4 #7 (Outlook), #9 (Fokus), #10 (Admin-Terminal):** Pro Fall erwartet
  das Log eine Zeile `Hinweis: <key> — …`, und die Karte steht 3 s. Bei #10
  zeigt die Karte während der 5 s Wartezeit den Pegel (`injecting`, die
  Waveform läuft leer), danach ohne Lücke den Hinweis. Die 3 s zählen ab
  `InjectFinished`.
- **G4 #8 (sofort neu diktieren):** Der Übergang Hinweis → Pegel ruft kein
  `hide()` auf, und die Karte bleibt an ihrer Position (Monitor des vorigen
  Diktats). Das Umpositionieren passiert nur beim Einblenden aus dem
  Unsichtbaren. Wandert das neue Diktat auf einen anderen Monitor, während
  der Hinweis noch steht, bleibt die Karte auf dem alten. Das folgt dem
  „eingefroren“-Vertrag aus dem Overlay-Plan, beim Test aber darauf achten.
- **Fokusprobe:** Während der Hinweis steht, in Notepad weitertippen und durch
  die Karte klicken. Es gibt keinen neuen Fensterpfad. Bitte trotzdem prüfen,
  weil der Hinweis auch aus dem Unsichtbaren einblenden kann (etwa nach einem
  Tray-Diktat in einem anderen Fenster).
- **Tray-Diktat:** Nach dem zweiten Klick erscheint „Text liegt in der
  Zwischenablage / Mit Strg+V einfügen“. Während der Pause erscheint er nicht,
  siehe Abweichung 2.
- **DPI-Wechsel während des Hinweises:** Karte und Text sollten neu gerastert
  in der richtigen Größe stehen.
- **Glyphen-Fallback:** Live kaum auszulösen, er würde einen GDI-Fehler
  brauchen. Belegt ist er über die reinen Tests und die letzte PNG-Zeile.
- **Nicht geprüft:** Verhalten mit Hochkontrast oder anderer Systemschrift.
  Die Schrift ist hart „Segoe UI“, fehlt sie, wählt der GDI-Mapper Ersatz.
- **Offene Owner-Entscheidungen:** Abweichungen 1 und 2.
