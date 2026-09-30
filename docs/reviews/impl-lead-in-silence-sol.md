# Review: Vorlauf-Stille und Float-Debug-WAV (SPEC v1.9)

Stand: 2026-09-30. Reviewer: Copilot / GPT-6.1 Sol.
Auftrag: `impl-lead-in-silence-review-prompt.md`.

## Ergebnis und Befunde nach Schwere

**Im vereinbarten Umfang ist nichts zu beanstanden: keine kritischen,
wichtigen oder kleinen Implementierungsfehler nachgewiesen.** Die
Vorlauf-Stille entsteht genau einmal pro freigegebenem Engine-Aufruf;
Gate, Report und Ergebnisdauer bleiben auf dem Original. Die Debug-WAV
speichert die Capture-Samples unveraendert als Float32, ohne Vorlauf.
Die verbleibenden Testgrenzen sind unten getrennt aufgefuehrt.

Geprueft wurde der aktuelle Working Tree, nicht nur HEAD. Der gesamte Diff
von `src\engine.rs` gehoert zum Review. In `src\daemon\debug_wav.rs` wurden
das Modul-Doc, `write_wav` und die beiden beauftragten WAV-Tests bewertet.
Clipboard-Restore und die Ring-Implementierung des uncommitteten
0.4.0-Stands wurden nicht bewertet; die Ring-Tests wurden wie verlangt
mit ausgefuehrt. Weitere Dateien wurden nur zur Pruefung der Aufrufpfade,
des Float-Lesers und der Dauerverbraucher gelesen.

Referenzhinweis: Der im Auftrag als SPEC Abschnitt 18 #15 bezeichnete
Entscheidungstext zur Vorlauf-Stille steht im vorliegenden Dokument
tatsaechlich in Abschnitt 17, `docs\SPEC.md:1014`; Abschnitt 18 endet mit
#14. Bewertet wurde der inhaltlich passende Text sowie Abschnitt 6.4
(`docs\SPEC.md:490-506`) und Abschnitt 10 (`docs\SPEC.md:814-824`).
Das ist keine Abweichung der Implementierung.

## Statisch geprueft

### Engine-Pfade: Stille genau einmal

| Pfad | Beleg | Bewertung |
|---|---|---|
| Daemon | `src\daemon\workers.rs:661-665`, `src\daemon\mod.rs:1001-1006,826-850`, `src\daemon\workers.rs:308-315` | Dump und Nachrichten enthalten die Originalsamples. Der EngineWorker uebergibt diese genau einmal an `transcribe_pcm`. `EngineWorker::transcribe` in `mod.rs:845` ist der Nachrichtenversand, kein direkter Aufruf des Parakeet-Transcribers. |
| `--transcribe-wav` | `src\main.rs:368-386,405-425` | Vorpruefung und Report auf dem Original. Warmup und jeder gemessene Lauf rufen getrennt `transcribe_pcm(&pcm)` auf; keiner verwendet den gepaddeten Puffer des vorherigen Laufs. |
| `--record-test` | `src\main.rs:944-969` | Vorpruefung auf `captured.samples`, danach `transcribe_pcm` mit demselben Original. |
| stt-smoke | `src\engine.rs:695-700,1527-1544,1571-1576` | Alle Inferenzpfade gehen ueber `transcribe_pcm`. `CountingEngine` delegiert den bereits gepaddeten Puffer nur an den inneren Transcriber, ohne erneut zu padden. |

Eine Suche ueber die Rust-Dateien des Repositories zeigt keine weiteren
realen Modell-Aufrufer. Der direkte Aufruf in
`src\engine.rs:858` ist ausschliesslich der Test des `StubTranscriber`,
nicht ein Parakeet-Pfad. Der eigentliche Bibliotheksaufruf in
`src\engine.rs:568-574` fuegt keine Stille hinzu.

### Gate, Report, Dauer und Fehler

- `src\engine.rs:506-523`: `silence_gate` bekommt den Originalslice.
  Ablehnung kehrt vor der neuen Padding-Allokation zurueck; die schon
  vorher vorhandenen Gate-internen Allokationen sind davon unabhaengig.
  Erst bei Freigabe entstehen 4800 positive Nullen und eine bitgleiche
  Kopie des Originals. Der Originalslice ist nur geliehen und wird nicht
  veraendert. Der Report wird nicht aus dem neuen Puffer berechnet.
- `src\engine.rs:517-522`: Jeder erfolgreiche Engine-Rueckgabewert bekommt
  `Timing.duration = Originalsamplezahl / ENGINE_RATE`, auch wenn die
  Engine selbst keinen oder einen falschen Timing-Wert liefert oder
  einen leeren Text zurueckgibt. `ENGINE_RATE` ist 16000
  (`src\audio\mod.rs:20`), 4800 Samples sind damit exakt 300 ms.
  Parakeet kennt die Padding-Laenge nicht und liefert `timing: None`
  (`src\engine.rs:576-581`).
- Im Fehlerfall bleibt `Err(EngineError)` erhalten; es gibt dann kein
  `Transcription`-Objekt und deshalb auch keine Ergebnisdauer. Der
  separat zurueckgegebene Report enthaelt weiterhin Originalsamplezahl
  und Originaldauer. Der Daemon protokolliert ihn vor der Aufteilung in
  Erfolg und Fehler (`src\daemon\workers.rs:314-337`).
  Ablehnung behaelt das bisherige `Transcription::empty()` mit
  `timing: None`.
- Keine produktive Stelle liest `Transcription.timing`; die einzigen
  Leser liegen in Engine-Tests. Der Watchdog verwendet die aus der
  Originalsamplezahl erzeugte `AudioInfo.duration`
  (`src\daemon\mod.rs:1001-1006,1065-1066`,
  `src\state.rs:748-752`), nicht das Transcription-Timing.
  Gate-Logzeilen verwenden den Originalreport. Die Inferenz-Logzeilen
  messen reale Laufzeit per `Instant`, keine Audio- oder Padding-Laenge.
  Dass das Padding die reale Rechenzeit beeinflussen kann, ist kein
  Widerspruch zum Vertrag fuer die Audiodauer.

### Debug-WAV und Float-Leser

- `src\daemon\debug_wav.rs:16-17,312-326`: Header ist mono, 16000 Hz,
  32 bit, `SampleFormat::Float`. Jedes `f32` wird direkt an Hound
  uebergeben, ohne Clamp, Skalierung oder Rundung. Der gepruefte
  Roundtrip-Test bestaetigt die Bits einschliesslich `-0.0`, sehr kleiner
  Werte, `0.99999` sowie `1.5` und `-2.0`.
- `src\daemon\workers.rs:661-665`: Der Dump leiht `captured.samples`;
  danach wird derselbe Vec in `Msg::Audio` verschoben. Auf dem weiteren
  Weg zur Engine findet keine Samplebearbeitung statt. Die WAV ist
  deshalb bitgleich zum Eingang von `transcribe_pcm`, nicht zum um
  4800 Nullen erweiterten Eingang des inneren Transcribers. Genau diese
  Unterscheidung verlangt SPEC Abschnitt 10.
- `src\audio\mod.rs:105-123`: Der Float32-Zweig akzeptiert endliche Werte
  ausserhalb von [-1, 1] unveraendert. NaN und beide Unendlichkeiten
  ergeben ausdruecklich `AudioError::Format`, keine Bereinigung.
  Der Writer validiert Endlichkeit dagegen nicht und kann solche Bits
  archivieren. Das ist keine widerspruechliche Audioverarbeitung:
  Nicht-endliche Samples sind auch beim direkten PCM-Eingang
  ungueltig und werden vom Gate ohne Engine-Aufruf abgelehnt
  (`src\engine.rs:377-385,1246-1255`). Ein Dump mit NaN/Inf ist also
  archiviert, aber absichtlich kein akzeptierter CLI-Roundtrip; beim
  CLI-Lesen entsteht ein Formatfehler statt eines InvalidInput-Reports.
  Die Bitgleichheit gilt beim Schreiben auch ohne eine Zusage, dass
  jeder ungueltige Float-Bitwert anschliessend transkribierbar ist.

### Testabdeckung und nicht gefangene Fehlerbilder

Die neuen Tests pruefen 4800 positive Null-Bitwerte, Gesamtlaenge,
bitgleichen Originalteil, vollstaendige Reportgleichheit,
Originaldauer statt absichtlich falscher Stub-Dauer und ausbleibende
Engine-Aufrufe bei Ablehnung (`src\engine.rs:775-853`).
Die bestehenden Gate-Tests pruefen ueber `expect_speech` auch die
gepaddingte Laenge und Aufrufzahl fuer die anderen Annahmeregeln
(`src\engine.rs:731-741`). Die WAV-Tests pruefen Format und
bitgleichen Roundtrip (`src\daemon\debug_wav.rs:564-621`).

Folgende Grenzen sind **Testverbesserungen, keine nachgewiesenen Bugs**:

| Stelle | Nicht sicher gefangenes Fehlerszenario | Vorschlag |
|---|---|---|
| `src\engine.rs:801-838` | Der explizite Dauervergleich verwendet genau 3 s. Eine kuenftige Ganzsekunden-Trunkierung koennte bestehen; der bitgenaue Puffer-/Reportvergleich ist dort nur fuer B1 ausgefuehrt. | Zusaetzlich z. B. 48001 Samples und einen B2/B3/D-Fall mit RecordingStub pruefen. |
| `src\engine.rs:761-775` | Der Fehlerfall prueft Entscheidung und vorhandene Metrics, nicht den gesamten Originalreport oder den tatsaechlich empfangenen Fehlerfall-Puffer. Eine spaetere Sonderbehandlung koennte falsche Samplezahlen/Metrics zurueckgeben. | Fehlerschlagenden RecordingStub verwenden und Report gegen `silence_gate(&pcm)` sowie Pufferbits vergleichen. |
| `src\main.rs:405-415,968`, `src\daemon\workers.rs:314,661-665` | Ein spaeterer Wrapper koennte `transcribe_pcm` umgehen, zuvor selbst padden oder die Samples zwischen Dump und Engine veraendern; isolierte Engine- und WAV-Tests blieben gruen. | Modellfreie Wiring-Tests mit injizierbarem RecordingTranscriber und Vergleich des Dump-Inhalts mit dessen Originalteil ergaenzen. Im aktuellen Code statisch ausgeschlossen. |
| `src\daemon\debug_wav.rs:588-621`, `src\audio\mod.rs:279-297` | Der neue Writer-Roundtrip-Test enthaelt keine NaN/Inf-Bits; die Leser-Ablehnung ist separat getestet. Eine spaetere Normalisierung nicht-endlicher Werte allein im Dump-Writer bliebe unbemerkt. | Optional NaN-Payload und +/-Inf direkt mit Hound bitweise zuruecklesen und separat die Ablehnung durch `read_wav_16k_mono` pruefen. |

Die Tests ohne Modell belegen nicht, dass "Herr Praesident" in realer
Inferenz verschwindet. Auch der vorhandene stt-smoke prueft die
abgesenkten Fixtures relativ zur jeweils im selben Lauf gemessenen
Baseline, nicht eine absolute WER-Grenze oder die gesicherten
Halluzinationsfaelle (`src\engine.rs:1549-1566`). Diese Modellwirkung
bleibt in diesem Review ein uebernommener Messbeleg.

## Selbst ausgefuehrte Tests

Alle folgenden Befehle endeten mit Exitcode 0.

`cargo test engine::tests`:

```text
test result: ok. 39 passed; 0 failed; 1 ignored; 0 measured; 475 filtered out; finished in 0.18s
```

`stt_smoke_fixtures` blieb dabei wie verlangt ignoriert.

`cargo test daemon::debug_wav`:

```text
test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 498 filtered out; finished in 0.03s
```

Zusaetzlich gezielt fuer die Float-Leser-Prueffrage:
`cargo test audio::tests::wav_f32`:

```text
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 513 filtered out; finished in 0.00s
```

Dabei liefen `wav_f32_roundtrip` und `wav_f32_rejects_nan_and_inf`.
Keine Modell-Inferenz, kein Release-Build, kein Start oder Eingriff in
den laufenden/installierten Daemon. Die Tests verwenden eigene
tempfile-Verzeichnisse; `%TEMP%\diktier` wurde nicht angefasst.

## Uebernommene Aussagen, nicht selbst verifiziert

Aus `impl-lead-in-silence-notes.md` stammen:

- `cargo fmt --check` und Clippy ohne Warnungen; voller Testlauf:
  `505 passed; 0 failed; 10 ignored`.
- Modell-Gate `stt_smoke_fixtures`: `1 passed; 0 failed`, samt den dort
  aufgefuehrten sieben WER-Messwerten und dem bestandenen relativen
  WER-Puffer. Dieses Gate wurde hier ausdruecklich nicht ausgefuehrt.
- Release-Build und Versionsausgabe `diktier 0.4.1`; Transkript von Lauf
  523 ohne "Herr Praesident", Originalreport mit 159360 Samples sowie
  weiter bestehendes vorangestelltes "Ich" in Lauf 547. Keine eigene
  Reproduktion dieser Aussagen.

Die Matrix mit 0/200/300/500 ms aus dem letzten Abschnitt von
`docs\SPIKES.md` wurde als Begruendung der verbindlichen 300-ms-Vorgabe
gelesen, nicht nachgemessen. README, Versionsdateien und Release/Installation
waren nicht Gegenstand dieses begrenzten Reviews.
