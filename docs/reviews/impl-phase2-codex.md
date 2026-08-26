# Review Phase 2a+2b — X11-Inject + Capture-Pipeline (codex)

Datum: 2026-08-26  
Verbindliche Referenz: `docs/SPEC.md` v1.3  
Geprüfter Scope: vollständiger uncommitteter Working-Tree-Diff gegen `HEAD`,
einschließlich `Cargo.lock`, `src/engine.rs` und der neuen Module unter
`src/audio/` und `src/inject/`.

## Kurzfazit

Die Grundrichtung ist gut: Linux-Abhängigkeiten sind sauber gegatet, der
`cpal`-Daten-Callback selbst ist allokations-, lock- und syscallfrei, der
Downmix liegt außerhalb des Callbacks, und `rubato::FftFixedIn` verarbeitet
den Restblock plus einen Flush-Block. Die bereits bestandene Live-Abnahme
wurde nicht wiederholt.

Der Stand ist trotzdem noch nicht für Phase 3 freizugeben. Besonders kritisch
sind mehrere X11-Races: Ein ungepumpter `SelectionClear` kann einen fremden
Clipboard-Inhalt beim nächsten Diktat durch einen veralteten Snapshot ersetzen;
die Fokusprüfung liegt vor einem bis zu Sekunden dauernden Clipboard-Snapshot;
und ein `SelectionRequest` wird als bestätigter Read gezählt, bevor die
X11-Requests auf Protokollerfolg geprüft wurden. Das synchrone Restore-Protokoll
blockiert außerdem seinen Aufrufer bis zu fünf Sekunden und ist damit nicht der
in §7 verlangte State-Machine-Timer.

Im Audio-Pfad ist der Callback zeilenweise betrachtet realtime-tauglich. Die
Sicherheit bricht jedoch am Stop-/Overflow-Rand: `pause()`-Fehler werden
ignoriert, während Producer und Consumer des Overwrite-Rings nicht sicher
parallel laufen dürfen. Die Tests prüfen nur phasengetrennte, sequentielle
Zugriffe. Beim Hotkey können X11-Verbindungsfehler als erfolgreich registrierter,
aber toter Stub-/Global-Backend-Pfad erscheinen; ein Start-Timeout kann durch
das anschließende unbeschränkte `join()` dennoch hängen.

Verifikation auf diesem Linux-Host:

- `cargo build --locked`: erfolgreich.
- `cargo test --locked`: 74 bestanden, 1 modellabhängiger Smoke-Test ignoriert.
- `cargo clippy --locked --all-targets --all-features -- -D warnings`:
  erfolgreich.
- Ein Windows-Cross-Check war nicht möglich; installiert ist nur
  `x86_64-unknown-linux-gnu`.

## Befunde

### H1 — Ein verzögertes `SelectionClear` kann fremden Clipboard-Inhalt überschreiben

- **Schwere:** Hoch
- **Stelle:** `src/inject/linux.rs:187-200`, `src/inject/linux.rs:475-498`,
  `src/inject/linux.rs:523-545`, `src/inject/protocol.rs:294-309`
- **Problem:** `snapshot_clipboard()` vertraut bei `self.we_own == true` blind
  auf `self.serve`. `we_own` wird aber erst beim Pumpen eines
  `SelectionClear` zurückgesetzt. Übernimmt ein anderer Client zwischen zwei
  Diktaten das Clipboard, während Diktier keine Events pumpt, liegt das
  `SelectionClear` nur in der Queue. Das nächste Diktat snapshotet dann den
  alten Diktier-Text, übernimmt die Selection erneut und restauriert später
  diesen veralteten Text statt des fremden Inhalts. Genau der laut §7.1
  verbotene Fremdänderungsfall wird damit außerhalb des aktiven Restore-Loops
  nicht erkannt.

  Zusätzlich besteht beim Restore eines ursprünglich leeren Clipboards ein
  TOCTOU: Nach dem erfolgreichen `still_owner()` kann ein fremder Owner
  übernehmen, bevor `release_ownership()` mit `CURRENT_TIME` läuft. Dieser
  spätere Request kann dann den inzwischen fremden Owner wieder auf `NONE`
  setzen. Ein boolesches `we_own` ist keine belastbare X11-Generation.
- **Vorschlag:** Vor jedem Snapshot zuerst alle Selection-Events drainieren und
  den tatsächlichen Owner synchron prüfen; `we_own` bei jeder negativen
  Owner-Prüfung aktualisieren. Den Server-Zeitstempel der eigenen Übernahme
  speichern und beim bedingten Freigeben verwenden, sodass eine nachfolgende
  fremde Ownership den alten Request serverseitig verwirft. Die Fake-Tests um
  „ungepumpter SelectionClear vor neuem Paste“ und „Takeover zwischen letzter
  Owner-Prüfung und Empty-Restore“ erweitern.

### H2 — Die Fokusprüfung ist beim tatsächlichen Paste bereits veraltet

- **Schwere:** Hoch
- **Stelle:** `src/inject/protocol.rs:268-292`,
  `src/inject/linux.rs:304-418`
- **Problem:** `current_window()` wird nur am Anfang von `inject_paste()`
  geprüft. Danach folgen `WM_CLASS`-Abfrage und Clipboard-Snapshot. Der
  Snapshot kann 2 × 400 ms dauern; ein `INCR`-Transfer wartet zusätzlich bis
  zu einer Sekunde. Wechselt der Nutzer in diesem Fenster den Fokus, wird der
  Paste-Chord dennoch an das neue Fenster geschickt. Damit gilt beim
  eigentlichen Inject nicht mehr `start == target == current` (§7.3), obwohl
  der zurückgegebene `InjectOutcome` noch das alte Fenster meldet.
- **Vorschlag:** Unmittelbar vor dem ersten synthetischen Key-Event erneut
  `_NET_ACTIVE_WINDOW` abfragen und bei jeder Abweichung auf `copy_only`
  wechseln. Das Snapshotting darf vor dieser finalen Prüfung liegen. Ein Fake
  muss einen Fokuswechsel während `snapshot_clipboard()` auslösen.

### H3 — Ein X11-Request wird zu früh als bestätigter Clipboard-Read gezählt

- **Schwere:** Hoch
- **Stelle:** `src/inject/linux.rs:225-297`,
  `src/inject/linux.rs:523-532`, `src/inject/protocol.rs:294-318`
- **Problem:** Für `change_property8/32` und `send_event` wird nur geprüft, ob
  der Request lokal serialisiert werden konnte. Die jeweiligen `VoidCookie`s
  werden nicht mit `check()` synchron auf `BadWindow`, `BadAtom` usw. geprüft.
  Trotzdem liefert `handle_selection_request()` bei Daten-Targets `true`, der
  Read-Zähler steigt und der alte Clipboard-Inhalt darf restauriert werden.
  Ein zwischen Request und Antwort zerstörter Requestor reicht für diesen
  Fehlerpfad.

  `still_owner()` verschluckt außerdem jeden Reply-/Connection-Fehler und
  übersetzt ihn in `false`. Ein Verbindungsabbruch wird so als harmlose fremde
  Ownership statt als `InjectError` behandelt, obwohl der Transkriptinhalt auf
  dieser Verbindung nicht mehr bereitgestellt werden kann.
- **Vorschlag:** Property-Write und `SelectionNotify` checked ausführen und
  erst danach einen Read bestätigen. Owner-Abfragen müssen
  `Result<bool, InjectError>` liefern; Connection-/Reply-Fehler dürfen nicht
  mit Ownership-Verlust zusammenfallen. Fakes für fehlschlagenden
  Property-Write, Notify und Connection-Abbruch ergänzen.

### H4 — Der Restore-Loop blockiert den Aufrufer statt als State-Machine-Timer zu laufen

- **Schwere:** Hoch
- **Stelle:** `src/inject/protocol.rs:262-319`,
  `src/inject/protocol.rs:355-383`, `src/inject/linux.rs:594-607`
- **Problem:** `OutputSink::paste()` kehrt erst nach Mindestdelay oder dem
  5-s-Read-Timeout zurück. Zwar werden währenddessen X11-Events in 5-ms-Slices
  bedient, der Thread schläft und kann aber keine Tray-, Quit-, Hotkey- oder
  State-Machine-Nachricht verarbeiten. Das ist nicht der in §7.1 Punkt 6
  verlangte Timer in der State-Machine und wird spätestens beim Phase-3-Wiring
  zu einer sichtbaren Event-Loop-Blockade. Quit kann auf diesem Pfad bis zu
  fünf Sekunden nicht bearbeitet werden.
- **Vorschlag:** Paste-Start und Restore-Fortschritt als nichtblockierende
  Session modellieren. Den X11-FD zusammen mit State-/Quit-Kanälen pollen und
  Delay sowie Read-Deadline als Timer-Events behandeln. Der Output-Pfad muss
  zwischenzeitlich auf Quit reagieren und die Selection bis Ownership-Verlust
  weiter bedienen.

### H5 — Der Overwrite-Ring ist bei echter Producer-/Consumer-Überlappung nicht speichersicher

- **Schwere:** Hoch
- **Stelle:** `src/audio/spsc.rs:56-94`,
  `src/audio/capture.rs:298-325`
- **Problem:** Producer und Consumer schreiben beide `read`. Bei vollem Ring
  kann der Producer `read` vorschieben und anschließend einen Slot
  überschreiben, den der Consumer bereits mit einem alten `read`-Wert liest.
  Neben verlorenen Pointer-Updates entsteht damit ein gleichzeitiger
  nichtatomarer Read/Write auf demselben `UnsafeCell<T>` — Undefined Behavior.
  Bei Stereo kann ein paralleles `pop()` zudem die Frame-Ausrichtung verlieren.

  Die aktuelle Einbindung versucht, den Consumer erst nach `stream.pause()`
  zu starten, ignoriert aber das Ergebnis von `pause()`. CPAL dokumentiert
  ausdrücklich, dass Pause scheitern kann. Dann läuft der Callback während
  `drain_f32()`, Downmix und Resampling weiter; auch ein späteres `reset()` kann
  mit ihm kollidieren. Die sequentiellen Ringtests beweisen diesen Rand nicht.
- **Vorschlag:** Einen bewährten SPSC-Ring verwenden, bei dem ausschließlich
  der Consumer den Read-Cursor schreibt. Für diesen begrenzten Capture-Puffer
  ist „neuestes Frame bei voll verwerfen und Overflow zählen“ sicherer als
  producerseitiges Überschreiben des ältesten Frames. `pause()`-Fehler
  propagieren oder den Stream zuerst sicher droppen/joinen, bevor gelesen oder
  resettet wird. Den Überlappungsfall mit Loom/Miri bzw. einem dafür geeigneten
  Ring-Crate abdecken.

### H6 — Hotkey-Verbindungsfehler können PTT lautlos deaktivieren oder Shutdown hängen lassen

- **Schwere:** Hoch
- **Stelle:** `src/hotkey.rs:128-147`, `src/hotkey.rs:196-224`,
  `src/hotkey.rs:259-338`, `src/hotkey.rs:386-393`
- **Problem:** Beim gepinnten `global-hotkey 0.8.0` startet
  `GlobalHotKeyManager::new()` den X11-Thread asynchron und liefert bereits
  `Ok`. Scheitert dessen X11-Verbindung früh, ignoriert die Dependency den
  fehlgeschlagenen Channel-Send/Recv in `register()` und kann ebenfalls `Ok`
  liefern. Der Fallback wird nur bei `try_new()` gewählt und ist damit in
  diesem Fall unerreichbar: Diktier meldet einen registrierten, aber toten
  Hotkey.

  Im eigenen XGrabKey-Fallback beendet ein späterer Fehler von
  `poll_for_event()` lediglich die innere `while let Ok(...)`-Schleife; der
  Thread schläft danach weiter und hält den Event-Sender offen. `poll()` sieht
  weder Event noch Fehler. Beim Start-Timeout wird zwar `Shutdown` gesendet,
  direkt danach aber unbegrenzt `join()` aufgerufen; hängt der Thread gerade in
  X11-Connect oder einem Reply, hebt das den 2-s-Timeout wieder auf. Die Antwort
  von `xkb_per_client_flags(DETECTABLE_AUTO_REPEAT)` wird zudem verworfen; bei
  Ablehnung erzeugt der Fallback Release/Press-Paare durch Auto-Repeat.
- **Vorschlag:** Einen expliziten Ready-/Error-Handshake bis nach erfolgreichem
  Register verwenden, alle X11-ConnectionErrors über einen Fehlerkanal
  propagieren und bei Poll-Fehler den Sender droppen. Start und Shutdown dürfen
  nicht durch ein unbeschränktes Join den eigenen Timeout neutralisieren.
  Detectable-Auto-Repeat muss bestätigt werden; andernfalls Release/Press mit
  gleichem Zeitstempel als Repeat filtern.

### M1 — Große X11-Transfers sind weder beim Snapshot noch beim Serven robust

- **Schwere:** Mittel
- **Stelle:** `src/inject/linux.rs:304-418`,
  `src/inject/linux.rs:249-274`
- **Problem:** Ein direkter Property-Transfer wird auf 256 KiB begrenzt, aber
  `bytes_after` wird ignoriert. Ein zulässiger größerer Nicht-INCR-Transfer
  wird daher still abgeschnitten und als vollständiger Snapshot restauriert.
  Beim INCR-Empfang gelten eine gemeinsame 1-s-Deadline und 1-MiB-Grenze. Nach
  Timeout/Grenze wird das Protokoll nicht sauber beendet; der anschließende
  `STRING`-Fallback verwendet dieselbe `DIKTIER_SELECTION`-Property, während
  der erste Owner noch UTF8-INCR-Chunks liefern kann. Dadurch können zwei
  Transfers kollidieren.

  In Senderichtung wird immer ein einzelnes `change_property8` mit dem
  gesamten Text verwendet. Überschreitet er die maximale X-Request-Größe,
  gibt es kein INCR-Protokoll. Das 60-s-Limit macht diesen Fall selten, ist
  aber keine im Code erzwungene Byte-Grenze (`copy_only`/`--inject-test`
  akzeptieren beliebige Texte).
- **Vorschlag:** `bytes_after` vollständig nachlesen oder ablehnen; INCR mit
  per-chunk Deadline, konsistentem Typ/Format und sauberer
  Abbruch-/Drain-Strategie implementieren. Fallback-Transfers benötigen
  getrennte Properties oder müssen strikt serialisiert werden. Ausgehend vom
  Server-Maximum ab einer definierten Schwelle auch als Owner INCR senden und
  mit großen direkten sowie inkrementellen Payloads testen.

### M2 — Das Selection-Backend erfüllt die verpflichtenden ICCCM-Targets und Kodierungen nicht vollständig

- **Schwere:** Mittel
- **Stelle:** `src/inject/linux.rs:24-33`, `src/inject/linux.rs:225-275`
- **Problem:** `TARGETS` nennt weder `TIMESTAMP` noch `MULTIPLE`, und beide
  standardisierten Requests werden verweigert. Für das Target `STRING` werden
  die UTF-8-Bytes von `self.serve` als Typ `STRING` ausgeliefert; `STRING` ist
  jedoch Latin-1 und korrumpiert damit Umlaute bei Legacy-Requestors. Das
  Backend speichert auch keinen belastbaren Selection-Zeitstempel, was den
  Ownership-Race aus H1 begünstigt.
- **Vorschlag:** Acquisition-Timestamp erfassen und `TIMESTAMP` beantworten,
  `MULTIPLE` korrekt abarbeiten und nur tatsächlich unterstützte Targets
  annoncieren. `STRING` entweder korrekt nach Latin-1 konvertieren (bei nicht
  darstellbaren Zeichen ablehnen) oder nicht anbieten; Unicode bleibt über
  `UTF8_STRING`.

### M3 — Modifier-Restore und Fehler-Cleanup sind nicht race-/fehlerfest

- **Schwere:** Mittel
- **Stelle:** `src/inject/protocol.rs:385-440`,
  `src/inject/linux.rs:547-592`
- **Problem:** Die Chord-Funktion besteht aus vielen falliblen Schritten mit
  frühem `?`. Scheitert ein Schritt nach einem synthetischen Down, gibt es
  keinen best-effort Cleanup; Ctrl, Shift, V oder Insert können logisch unten
  bleiben. Umgekehrt können zuvor gelöste physische Modifier unvollständig
  behandelt werden.

  Auch der Restore ist ein TOCTOU: `query_modifiers()` entscheidet, dass ein
  Modifier noch gehalten wird; löst der Nutzer ihn vor dem folgenden
  `key_down()`, kann gerade dieses synthetische Down nach dem realen Release
  einen hängenden Modifier erzeugen. Zudem bildet `XQueryKeymap` den logischen
  Core-Zustand nach den XTEST-Up-Events ab, nicht unabhängig davon den
  Hardwarezustand; der Fake-Test mit `synthetic_affects_physical = false`
  modelliert daher nicht zwingend den realen X-Server.
- **Vorschlag:** Chord-Senden als Guard/Transaktion mit garantiertem
  best-effort Key-Up für alle selbst gedrückten Tasten implementieren. Für
  X11 die reale Modifier-Semantik in einem eigenen Spike einschließlich
  „Release genau zwischen Query und Restore“ prüfen; wenn ein sicherer
  physischer Nachweis nicht möglich ist, auf synthetisches Restore verzichten
  statt einen hängenden Modifier zu riskieren.

### M4 — CPAL deckt nur vier der nativen PCM-Formate ab

- **Schwere:** Mittel
- **Stelle:** `src/audio/capture.rs:139-169`, `src/audio/convert.rs:3-43`
- **Problem:** Unterstützt werden nur `I16`, `U16`, `I32` und `F32`. CPAL
  0.18.2 kennt zusätzlich unter anderem `I8`, `I24`, `I64`, `U8`, `U24`,
  `U32`, `U64` und `F64`. Liefert `default_input_config()` eines davon, wird
  das native Gerät abgelehnt, obwohl §6.4 das Öffnen in seiner nativen
  Sampledarstellung und die Integer-/Float-Konvertierung verlangt. Auffällig
  ist, dass `u8_to_f32()` existiert, aber nie verdrahtet ist.
- **Vorschlag:** Alle von CPAL angebotenen linearen Integer-/Floatformate
  typisiert anbinden und außerhalb des Callbacks skaliert nach f32
  konvertieren; DSD explizit als nicht unterstütztes Nicht-PCM-Format melden.
  Randwerte jedes Formats testen.

### M5 — `output.leading_space` wird ignoriert

- **Schwere:** Mittel
- **Stelle:** `src/inject/protocol.rs:262-319`, `src/config.rs:132`,
  `src/config.rs:190`
- **Problem:** §7.4 verlangt bei Default `leading_space = true` ein führendes
  Leerzeichen. Der Inject-Pfad benutzt aus `OutputConfig` nur Restore- und
  Shortcut-Felder; sowohl Paste als auch Fokusverlust-`copy_only` übernehmen
  den Text unverändert. Der Default hat damit keine Wirkung.
- **Vorschlag:** Die Ausgabe einmal zentral vor `paste`/`copy_only` formatieren
  und beide Pfade mit `leading_space = true/false`, leerem Text und bereits
  vorhandenem Leerzeichen testen.

### M6 — Audio-Worker-, Device-lost- und Shutdown-Verträge sind automatisiert nicht belegt

- **Schwere:** Mittel
- **Stelle:** `src/audio/capture.rs:181-187`,
  `src/audio/capture.rs:289-347`, `src/main.rs:343-435`, §5, §6.4, §13
- **Problem:** Der Phase-2-CLI-Pfad führt `stop()`, alle Konvertierungen,
  `rubato` und danach Modellladen/Inferenz synchron auf dem Main-Thread aus.
  Für einen isolierten Spike ist das ausführbar, belegt aber nicht den
  verbindlichen Audio-/Transkriptions-Worker oder dessen Quit-/Join-Verhalten.
  Es gibt keinen Fake für `pause()`-Fehler, Device-lost während einer Aufnahme,
  Reopen-Erfolg/-Fehler oder Quit während Capture/Resample. `lost` wird erst
  beim nächsten `start()` ausgewertet; `stop()` kann trotz vorherigem
  Device-lost einen Teilpuffer als normale Aufnahme liefern.
- **Vorschlag:** Vor Phase 3 einen Worker mit explizitem Stop-/Quit-Protokoll
  und begrenztem Join definieren. `AudioSource` bzw. CPAL hinter einen Fake
  stellen, der Device-lost, Xrun, Pause-Fehler und blockierten Worker
  deterministisch injiziert. Festlegen und testen, ob ein bei Device-lost
  abgebrochener Teilpuffer verworfen oder transkribiert wird.

### M7 — Die Fakes decken die entscheidenden asynchronen Races nicht ab

- **Schwere:** Mittel
- **Stelle:** `src/inject/fake.rs:21-25`,
  `src/inject/fake.rs:117-139`, `src/audio/spsc.rs:97-140`, §13
- **Problem:** Der Clipboard-Fake kennt nur einen erfolgreichen Datenrequest
  und eine vollständig verarbeitete Fremdübernahme. Er kann keine queued
  `SelectionClear`, Requestor-Zerstörung, ConnectionError, Focusänderung
  während Snapshot/Shortcut, getrennte TARGETS-/Datenrequests oder
  INCR-Chunks darstellen. Die Ringtests sind rein sequentiell. Daher laufen
  alle 74 Tests trotz H1–H6 grün.
- **Vorschlag:** Den Fake ereignis- und fehlerfähig machen und für jede der
  oben beschriebenen Reihenfolgen einen Regressionstest anlegen. Für den
  unsafe Ring reicht ein normaler Threadtest nicht als Beweis; Loom/Miri oder
  der Ersatz durch eine etablierte SPSC-Implementierung ist vorzuziehen.

### N1 — Der Ganzpuffer-RMS kann kurze leise Sprache in langer Stille verwerfen

- **Schwere:** Niedrig
- **Stelle:** `src/engine.rs:22-37`, `src/engine.rs:77-88`
- **Problem:** Die bestandene WAV-Regression belegt die Schwelle für genau die
  vorhandenen Dateien. Da RMS über die gesamte Aufnahme berechnet wird, kann
  ein kurzer leiser Sprachanteil durch viele Sekunden Stille unter `0.0075`
  verdünnt werden. Das betrifft gerade lange PTT-Haltezeiten und ist in den
  Fixtures nicht abgedeckt.
- **Vorschlag:** Gate fensterweise (z. B. Maximum/Anteil über 20–250-ms-Fenster)
  auswerten und Regressionen „kurze leise Sprache + lange Stille“ sowie
  ausschließliches Raumrauschen ergänzen, ohne die bereits abgenommene
  Rauschschwelle aufzugeben.

