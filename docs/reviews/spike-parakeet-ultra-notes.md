# Spike: Parakeet Ultra gegen Parakeet v3 (2026-09-30)

Auftrag: [spike-parakeet-ultra-prompt.md](spike-parakeet-ultra-prompt.md). Stand
Diktier: Commit 2d10ee0 (0.4.1). Am Produkt ist nichts geändert, nichts committet.
Rohdaten: [`.herd/spike-ultra/results.tsv`](../../.herd/spike-ultra/results.tsv)
(lokal, git-ausgeschlossen).

## Kurzfazit

- **Ultra ist auf diesen Diktaten besser, aber nur, wenn man es per-channel nach
  int8 quantisiert.** Mit der Methode, nach der v3-int8 entstanden ist (per-tensor,
  QUInt8), verliert Ultra seinen Vorsprung und liegt auf dem Niveau von v3-int8.
  Mit per-channel/QInt8 (`ultra-int8-pc`) liegt es fast auf fp32-Niveau. Ein Teil
  des Gewinns kommt allerdings von der Quantisierung selbst. Werden Ziffern als
  korrekt gewertet, kommt v3 per-channel auf 4 statt 9 Wortfehler (Ultra
  per-channel: 2). Roh liegt v3 per-channel bei 53,3 statt 51,2. Vor allem wird
  v3 per-channel 2,6× langsamer und scheidet damit aus.
- **„Herr Präsident“ ist eine Eigenheit der v3-int8-Gewichte, kein Problem des
  Modells an sich.** Ohne Vorlauf-Stille (Zusatzmessung, Stand vor 0.4.1) zeigt
  v3-int8 die Phrase bei 523, 547 und in 48 von 48 Dither-Varianten von 545 und
  547. v3-fp32, v3-int8-pc und alle drei Ultra-Varianten zeigen sie **nie**. Mit
  300 ms Vorlauf-Stille: v3-int8 1/24 (Seed 4, wie in SPIKES), alle anderen 0/24.
- **Das „Ich schaue“ in Lauf 547 bleibt bei allen sechs Varianten** (roh und
  24/24 Seeds). Ultra löst das nicht.
- **Kosten von `ultra-int8-pc` gegenüber Produktion:** gleiche Latenz (Ø 640 statt
  659 ms, RTF 0,053 statt 0,055), gleiche Ladezeit (~2 s), +175 MiB Peak Working
  Set (1199 statt 1024 MiB), +48 MB Download (719 statt 670 MB).
- **Einschätzung:** Ein Wechsel lohnt sich, und zwar nur auf `ultra-int8-pc`. Die
  Belegbasis ist aber schmal (drei Sprachaufnahmen mit Referenz, dazu fünf
  Kalibrierungsaufnahmen desselben Satzes). Vor einem Wechsel sollte ein
  Alltagstest stehen. Details unter „Einschätzung“.

## Aufbau

### Modelle

Quelle: Hugging Face `altunenes/parakeet-rs`, festgelegte Revision
`4d2a8bc71f5c896ec40faa59732e6716295edaf2` (main am 2026-09-30). Alle Dateien nach
dem Download geprüft. Die LFS-Dateien stimmen im SHA-256 mit dem LFS-oid überein.
`vocab.txt` liegt nicht in LFS; der Git-Blob-Hash stimmt
(`fc43e1c7…`), und der SHA-256 ist bei beiden Ordnern gleich dem
Produktions-`vocab.txt` (`d5854467…`). Ultra hat also denselben Tokenizer.

| Label | Herkunft | Dateien (Bytes) |
|---|---|---|
| `v3-int8` | Produktion, `%LOCALAPPDATA%\diktier\models\parakeet-tdt-0.6b-v3-int8\`, nur gelesen | Encoder 652.183.999, Decoder 18.202.004 |
| `v3-fp32` | `tdt/` | `encoder-model.onnx` 41.770.866 + `.data` 2.435.420.160, Decoder 72.520.893 |
| `ultra-fp32` | `parakeet-ultra/` | `encoder-model.onnx` 87.857.063 + `.data` 2.435.420.160, Decoder 72.520.894 |
| `ultra-int8` | selbst quantisiert aus `ultra-fp32`, Methode wie v3-int8 (per-tensor, QUInt8) | Encoder 698.288.996, Decoder 18.202.004 |
| `ultra-int8-pc` | selbst quantisiert aus `ultra-fp32`, per-channel, QInt8 | Encoder 700.507.227, Decoder 18.300.628 |
| `v3-int8-pc` | Gegenprobe: `v3-fp32` genauso per-channel/QInt8 quantisiert | Encoder 654.033.351, Decoder 18.300.628 |

`nemo128.onnx` wird von parakeet-rs 0.3.7 nicht gebraucht (`ParakeetTDT` rechnet die
Features selbst) und ist nicht geladen. Alles liegt unter
`D:\DEV\diktier\models\spike\` (gitignoriert).

SHA-256 der selbst erzeugten Dateien:

| Datei | SHA-256 |
|---|---|
| `ultra-int8/encoder-model.int8.onnx` | `a0c31c52d4d851e751cb7278349d31ca0475bc27f1287fb1640a350820afcb6a` |
| `ultra-int8/decoder_joint-model.int8.onnx` | `2276a335d4c8dc48686e931475a634595954956427d485830e1214c0d1a18d07` |
| `ultra-int8-pc/encoder-model.int8.onnx` | `2cc01c15a08d6976ca9ebe97739d15890f3088cfedd3a4aa4d969ba7a1702038` |
| `ultra-int8-pc/decoder_joint-model.int8.onnx` | `afcb9459250ab5c2e48e657d852501c233e8b7f5daed1894a2a7101d21165a5e` |
| `v3-int8-pc/encoder-model.int8.onnx` | `c8a9708e2deafbf1c56d499672987fbc22500ac5ad6e3e43f0b80f34a8a26faa` |
| `v3-int8-pc/decoder_joint-model.int8.onnx` | `05892ef89093e7dabcf23b83b4123089ff00231f6556b1b67f4350eaf454b5e5` |

### Messwerkzeug

Eigene Crate `.herd/spike-ultra/` mit denselben Pins wie Diktier: `parakeet-rs =0.3.7`
(Features `cpu`, `api-28`, `load-dynamic`) und `ort =2.0.0-rc.13` (`load-dynamic`,
`api-28`). Diktiers `Cargo.lock` wurde als Startpunkt kopiert; `ort`, `ort-sys`,
`ndarray`, `tokenizers`, `rustfft` und `realfft` lösen auf dieselben Versionen auf.
Die ONNX Runtime ist eine Kopie der installierten `onnxruntime.dll` 1.28.0
(SHA-256 `18370c37…`, identisch mit `versions.toml`). Geladen wird sie per
`ort::init_from(..).with_telemetry(false).commit()` wie in `ensure_ort_initialized`.

Je WAV liest das Werkzeug 16-bit-Samples als `i16 / 32768`, wie `read_wav_16k_mono`
mit `convert::i16_to_f32`. Es stellt 4800 Nullen voran wie `transcribe_pcm` und ruft
`transcribe_samples(pcm, 16000, 1, None)` auf. Die Engine ist `ExecutionConfig`
Default (intra 4, inter 1), entspricht also `engine.threads = 0`. Das Modell wird
einmal geladen, dann folgt ein ungezählter Warmup-Lauf. Den Silence-Gate rechnet
das Werkzeug nicht; alle Dateien der Matrix mit Sprache würden ihn passieren.

Jedes Modell lief die ganze Matrix (72 Dateien) zweimal in eigenen Prozessen (A,
B), nacheinander und ohne Parallellast. **Die Texte waren bei allen sechs Modellen
in A und B identisch (0 Abweichungen)**, die Inferenz ist also deterministisch.
Leistungswerte sind über A und B gemittelt.

Die Dither-Varianten (545 und 547, Seeds 0–23) erzeugt `make_lists.py` exakt nach
Auftrag: `random.Random(seed)`, je Sample `+choice((-1, 0, 1))`, auf i16
gesättigt. Plausibilitätsprüfung gegen SPIKES 2026-09-30: v3-int8 zeigt mit 300 ms
genau bei Seed 4 „Herr Präsident.“ (SPIKES: 23 sauber, Seed 4 nicht), und die
WER-Summe über die 11 Referenzdateien ist 51,2 (SPIKES bei 300 ms: 51,2). Das
Werkzeug rechnet also wie Diktier.

**Zusatzmessungen, nicht im Auftrag:**

1. Ring und Dither-Varianten **ohne** Vorlauf-Stille (`SPIKE_LEAD_IN=0`). Mit
   300 ms war „Herr Präsident“ zu selten (1/24 gegen 0/24), um Modelle zu
   unterscheiden.
2. Die per-channel-Quantisierungen `ultra-int8-pc` und `v3-int8-pc`. Sie waren
   nötig, weil `ultra-int8` nach Produktionsmethode schlechter abschnitt als
   `ultra-fp32` und sonst unklar geblieben wäre, ob der Verlust an Ultra oder an der
   Quantisierung liegt.

## WER je Datei (in %, `normalize.py` `wer`)

Fixtures gegen ihre Referenz, Kalibrierung 05/06/07/09 gegen `alltag.ref.txt`.
Eine Wortabweichung entspricht 4,8 % (alltag, 21 Wörter), 5,0 % (fachwoerter, 20)
bzw. 6,7 % (zahlen_umlaute, 15).

| Datei | v3-int8 | v3-int8-pc | v3-fp32 | ultra-int8 | ultra-int8-pc | ultra-fp32 |
|---|---:|---:|---:|---:|---:|---:|
| alltag | 0,0 | 0,0 | 0,0 | 4,8 | 0,0 | 0,0 |
| alltag_-16db | 0,0 | 0,0 | 0,0 | 4,8 | 0,0 | 0,0 |
| alltag_-22db | 0,0 | 0,0 | 0,0 | 4,8 | 4,8 | 0,0 |
| fachwoerter | 5,0 | 10,0 | 10,0 | 5,0 | 0,0 | 0,0 |
| fachwoerter_-16db | 10,0 | 10,0 | 10,0 | 5,0 | 5,0 | 5,0 |
| zahlen_umlaute | 13,3 | 20,0 | 13,3 | 13,3 | 13,3 | 13,3 |
| zahlen_umlaute_-16db | 13,3 | 13,3 | 13,3 | 13,3 | 13,3 | 13,3 |
| 05_leise_20pct | 4,8 | 0,0 | 0,0 | 0,0 | 0,0 | 0,0 |
| 06_leise_vorlauf | 0,0 | 0,0 | 0,0 | 0,0 | 0,0 | 0,0 |
| 07_leise_sofort | 4,8 | 0,0 | 0,0 | 4,8 | 0,0 | 0,0 |
| 09_normal_referenz | 0,0 | 0,0 | 0,0 | 0,0 | 0,0 | 0,0 |
| **Summe 7 Fixtures** | 41,7 | 53,3 | 46,7 | 51,0 | 36,4 | **31,7** |
| **Summe alle 11** | 51,2 | 53,3 | 46,7 | 55,7 | 36,4 | **31,7** |
| Summe 11, Ziffern als korrekt | 51,2 | 20,0 | 40,0 | 42,4 | 9,8 | **5,0** |
| Wortfehler 11, Ziffern als korrekt | 9 | 4 | 7 | 8 | 2 | 1 |

**Zur Zahlenzeile:** Die Referenz schreibt „dreiundzwanzigsten … zweihundertfünfzig“
aus. Alle Ultra-Varianten und `v3-int8-pc` schreiben ganz oder teilweise „23.“ und
„250“. Die Normalisierung zählt das als zwei Fehler, obwohl der Inhalt stimmt.
v3-int8 dagegen schreibt „dunundzwanzig … zweihundertfünzig“, das sind echte
Fehler. Die Zeile „Ziffern als korrekt“ ersetzt vor dem Vergleich „23.“, „250“
und „€“ durch die ausgeschriebene Form; sonst ändert sie nichts. Für das Produkt
heißt das: **Ultra schreibt Zahlen eher als Ziffern.** Das ändert das Verhalten
für den Nutzer, und ob es gewünscht ist, entscheidet Ralf.

**Aussagekraft:** alltag/-16db/-22db sind dieselbe Aufnahme, ebenso
fachwoerter/-16db und zahlen/-16db; 05/06/07/09 sprechen denselben Satz. Die
Unterschiede bestehen aus 1 bis 8 Wörtern. „Werkstatt/Werstadt“ kippt nach SPIKES
auch schon allein durch Dither. Die Richtung ist konsistent, der Umfang ist klein.

## „Herr Präsident“

Gezählt wird ein Text, der mit „Herr Präsident“ beginnt. Lauf 527 enthält die
Phrase gesprochen mitten im Satz („… wieder das Herr Präsident Problem.“), alle
Modelle transkribieren ihn gleich; er zählt nicht mit.

| Modell | Ring roh (11, ohne 527), 300 ms | 545 × 24 Seeds, 300 ms | Ring roh, **ohne** Vorlauf | 545 × 24, **ohne** | 547 × 24, **ohne** |
|---|---:|---:|---:|---:|---:|
| v3-int8 | 0 | **1** (Seed 4) | **2** (523, 547) | **24** | **24** |
| v3-int8-pc | 0 | 0 | 0 | 0 | 0 |
| v3-fp32 | 0 | 0 | 0 | 0 | 0 |
| ultra-int8 | 0 | 0 | 0 | 0 | 0 |
| ultra-int8-pc | 0 | 0 | 0 | 0 | 0 |
| ultra-fp32 | 0 | 0 | 0 | 0 | 0 |

Mit 300 ms zeigen 547 × 24 Seeds bei keinem Modell „Herr Präsident“.

Ohne Vorlauf lässt v3-int8 bei 12 der 24 Seeds von 547 zusätzlich den ganzen
ersten Satz weg:

> Herr Präsident. Ich meine, dass ich da in den letzten Tagen irgendwas gelesen habe, aber ich bin mir nicht mehr sicher, wo das war.

statt

> Herr Präsident. Ich schaue auch mal ganz kurz im Internet nach, ob es irgendwelche Updates zu Paraket gibt. Ich meine, dass …

**Befund:** Die Anfälligkeit sitzt in den per-tensor quantisierten v3-Gewichten.
Dieselbe Architektur in fp32 und dieselben v3-Gewichte per-channel quantisiert
sind auf diesen Aufnahmen immun. Ultra ist es in allen drei Varianten, auch mit
der Produktionsmethode. Die Grundlage sind allerdings nur drei Belegaufnahmen
(523, 545, 547). Die Dither-Varianten sind Varianten derselben Aufnahme, keine
unabhängigen Stichproben.

## „Ich“ bei Lauf 547

Gesagt ist „Schau auch mal …“. Alle sechs Modelle liefern „Ich schaue auch mal …“,
roh und bei 24/24 Seeds, mit und ohne Vorlauf-Stille. Das „Ich“ ist also keine
Quantisierungs- und keine Modellfrage zwischen v3 und Ultra. Ultra hilft hier nicht.

## Leistung

Inferenzzeit je Datei ohne Warmup, Mittel über die 7 Fixtures × 2 Läufe (Audio
7–12 s). RTF = Σ Inferenzzeit / Σ Audiodauer (ohne die 300 ms Vorlauf).
Peak Working Set per `GetProcessMemoryInfo` am Prozessende, Maximum aus A/B.
Ladezeit ist `ParakeetTDT::from_pretrained`, Lauf A/B.

| Modell | Laden A/B [s] | Inferenz Ø [ms] | Inferenz max [ms] | RTF | Ø über alle 72 [ms] | Peak Working Set [MiB] | Download [MB] |
|---|---:|---:|---:|---:|---:|---:|---:|
| v3-int8 | 2,2 / 2,0 | 659 | 689 | 0,055 | 642 | 1024 | 670,5 |
| v3-int8-pc | 2,1 / 2,0 | **1731** | 1792 | 0,144 | 1641 | 1027 | 672,4 |
| v3-fp32 | 6,4 / 3,0 | 805 | 872 | 0,067 | 773 | 2675 | 2549,8 |
| ultra-int8 | 2,0 / 2,0 | 659 | 718 | 0,055 | 639 | 1195 | 716,6 |
| ultra-int8-pc | 2,1 / 2,2 | 640 | 687 | 0,053 | 614 | 1199 | 718,9 |
| ultra-fp32 | 7,5 / 3,3 | 818 | 908 | 0,068 | 804 | 2977 | 2595,9 |

Download = Encoder + Decoder + `vocab.txt` (+ 97 Byte `config.json` bei v3-int8).
Die fp32-Ladezeit A ist ein Kaltstart aus dem Dateicache (erster Lesezugriff auf
2,4 GB), B ein Warmstart.

- **v3-int8-pc ist 2,6× langsamer, ultra-int8-pc nicht.** Die plausible, aber
  nicht verifizierte Erklärung: Der v3-Encoder hat 77 `ConvInteger`-Knoten, die per
  QInt8 vorzeichenbehaftete Gewichte bekommen. Ultra hat nur 29, weil der
  Ultra-Export 48 Pointwise-Convolutions als `MatMul` enthält (siehe unten). Ein
  ORT-Profil liegt nicht vor.
- **Ultra braucht +171 bis +175 MiB RAM.** Der Ultra-Graph enthält 82 MiB
  Inline-Konstanten statt 39 MiB (s. u.). Das erklärt einen Teil davon, aber nicht
  alles; den Rest habe ich nicht aufgeschlüsselt.

## Auffällige Textunterschiede (wörtlich, Lauf A, 300 ms)

Die Ring-Aufnahmen haben keine Referenz. Die Einordnung in Klammern ist meine
Lesart, keine Messung.

**fachwoerter** (Ref.: „Der Rust-Daemon lädt das ONNX-Modell über die Runtime,
danach schreibt der Worker-Thread …“)
- v3-int8: „Der Rust Daemon lädt das **UNNX** Modell über die Runtime.“
- v3-fp32: „Der Rust-**Deemon** lädt das **ONX**-Modell …“
- ultra-fp32: „Der Rust-Daemon lädt das ONNX-Modell …“
- ultra-int8: „Der Rust Daemon lädt das **ONX** Modell …“
- ultra-int8-pc: „Der Rust Daemon lädt das ONNX-Modell …“

**zahlen_umlaute** (Ref.: „Am dreiundzwanzigsten März überweise ich
zweihundertfünfzig Euro …“)
- v3-int8: „Am **dunundzwanzig** März überweise ich **zweihundertfünzig** Euro.“
- v3-fp32: „Am **dundzwariten** März überweise ich 250 Euro …“
- v3-int8-pc: „Am 23. März überweise ich 250 **€**.“
- ultra-int8: „Am **dundzwanzig.** März überweise ich 250 Euro, …“
- ultra-int8-pc, ultra-fp32: „Am 23. März überweise ich 250 Euro.“

**alltag** (alle drei Pegel): ultra-int8 „danach die **Werstadt** aufgeräumt“, alle
anderen „Werkstatt“ (ultra-int8-pc nur bei −22 dB „Werstadt“). 05 und 07:
v3-int8 „Werstadt“, ultra-fp32 und ultra-int8-pc „Werkstatt“.

**Lauf 523** („… macht seinen Branch fertig …“)
- v3-int8: „… Sebastian macht seinen **Branch** fertig und **erstell** einen eigenen Pull Request.“
- v3-fp32, ultra-fp32, ultra-int8-pc: „… seinen **Brunch** fertig und erstellt …“
- ultra-int8: „… seinen **Branch** fertig und erstellt …“

**Lauf 529**
- v3-int8: „Den **Brand** lassen wir noch liegen …“
- v3-fp32, v3-int8-pc: „Den **Brand schlassen** wir noch liegen …“
- ultra-fp32: „Den **Bransch** lassen wir …“
- ultra-int8: „Den **Branch** lassen wir …“
- ultra-int8-pc: „Den **Bahnschlassen** wir noch liegen …“ (klar schlechter)

**Lauf 533**
- v3-int8: „In der List View, … ist, glaube ich, schon eine Spalte Hess Document vorhanden.“
- ultra-fp32: „In der Listview, … schon eine Spalte Hess-Dokument vorhanden.“
- ultra-int8-pc: „In der List View, … ist glaube ich schon eine **Spaltehess** Document vorhanden.“ (schlechter)

**Lauf 537:** ultra-fp32 und ultra-int8 glätten die Selbstkorrektur. Aus „dass wir
**die Spalten, die verfügbaren Spalten** da irgendwie freigeben müssen“
(v3-int8, v3-fp32, beide -pc) wird „dass wir **die verfügbaren Spalten** da
irgendwie freigeben müssen“. Falls so gesprochen, ist das eine Auslassung.

**Lauf 543:** v3-fp32 „für dieses **WorkItem** … die anderen **WorKitems**“, alle
anderen „Work Item(s)“.

**Lauf 545**, ultra-int8 roh: „wie du **das** mir gerade erklärt hast“. In 23 der
24 Dither-Varianten steht wie bei allen anderen Modellen „wie du **es** mir …“.

Die Läufe 527, 539 und 541 sind bei allen Modellen identisch.

**Bilanz Ring:** Kein Modell ist auf dem Ring durchgehend besser. ultra-int8-pc
produziert zwei hässliche Verschmelzungen („Bahnschlassen“, „Spaltehess“), die
v3-int8 nicht hat. Dafür hat v3-int8 „erstell“. Auf dem Ring ist der Vorsprung von
Ultra nicht belegt.

## Wie Ultra-int8 entstanden ist

1. **Methode der Produktion ermittelt**, weil keine README sie nennt
   (istupakov-Modellkarte: nur NeMo-Export, keine Quantisierung). Dazu habe ich die
   Graphen von v3-int8 mit `onnx` inspiziert: Producer `onnx.quantize 0.1.0`,
   Metadaten `onnx.infer = onnxruntime.quant`, `DynamicQuantizeLinear` +
   `MatMulInteger`/`ConvInteger`, im Decoder `DynamicQuantizeLSTM` und quantisiertes
   `Gather`. Die Gewichte sind **UINT8** mit skalarem Scale/Zero-Point, also
   per-tensor.
2. **Methode reproduziert:** `onnxruntime.quantization.quantize_dynamic(src, dst,
   weight_type=QuantType.QUInt8, per_channel=False)` mit Default-Operatortypen,
   onnxruntime 1.30.0 und onnx 1.23.1, isoliert per `uv run --no-project --with
   onnxruntime --with onnx`. Angewandt auf `v3-fp32` ergibt das **bitgleich** die
   Produktionsdateien (Encoder `6139d2fa…`, Decoder `eea7483e…`). Die Methode ist
   damit exakt belegt (Skript `.herd/spike-ultra/quantize.py`).
3. **`ultra-int8`** = dieselbe Methode auf `ultra-fp32`.
4. **`ultra-int8-pc`** = `quantize_dynamic(..., weight_type=QuantType.QInt8,
   per_channel=True)` (Skript `quantize_pc.py`), die im Auftrag genannte
   Ersatzmethode. Eine zweite Quantisierung ergab bitgleiche Dateien; das Ergebnis
   ist reproduzierbar.

**Warum Ultra-int8 größer ist:** Der Ultra-Export ist anders gebaut als der v3-Export
(4954 statt 4491 Knoten): 337 statt 289 `MatMul`, 29 statt 77 `Conv` und 82 statt
39 MiB Konstanten inline. Die Gewichte in der `.data`-Datei sind gleich groß
(2.435.420.160 Byte). Die Konstanten werden nicht quantisiert, daher +46 MB Encoder.
Die README von Ultra nennt NeMos `model.export()` „wie bei `tdt/`“. Die
NeMo-/Torch-Version war offenbar eine andere; das habe ich nicht weiter geprüft.

## Einschätzung

**Ein Wechsel lohnt sich, mit `ultra-int8-pc`. Nicht mit `ultra-int8` nach
Produktionsmethode und nicht mit fp32.**

Belege dafür:
- **Qualität:** 2 statt 9 Wortfehler auf den 11 Referenzdateien, Ziffern als korrekt
  gewertet; roh 36,4 statt 51,2. Die Fehler von v3-int8 bei „ONNX“, „Daemon“ und
  den Zahlwörtern verschwinden. Das passt zu Moondreams eigener Angabe für
  Deutsch (FLEURS 4,13 → 3,61 % WER, Modellkarte `moondream/parakeet-ultra`,
  nicht nachgemessen). Ein Teil des Gewinns kommt von per-channel selbst (v3
  per-channel: 4 Wortfehler), die Variante ist für v3 aber zu langsam.
- **Robustheit gegen „Herr Präsident“:** Ohne Vorlauf-Stille zeigt v3-int8 die
  Phrase bei 50 von 59 Dateien (11 Ring + 48 Dither), ultra-int8-pc bei 0. Damit wäre die 300-ms-Stille
  nicht mehr der einzige Schutz.
- **Kosten:** Latenz und Ladezeit gleich, +175 MiB RAM, +48 MB Download.

Dagegen oder einschränkend:
- **Kleine Stichprobe:** drei Referenzaufnahmen plus ein Kalibrierungssatz. Auf dem
  Ring ohne Referenz gibt es keinen klaren Sieger, und ultra-int8-pc hat dort zwei
  auffällige Verschmelzungen.
- **Ziffern statt Zahlwörter** ändern das Verhalten für den Nutzer.
- **„Ich schaue“ bleibt.**
- **Das per-channel-Ergebnis ist fragil:** Dieselbe Quantisierung macht v3 2,6×
  langsamer, weil sie vom Graphaufbau abhängt. Die Leistung muss nach jedem neuen
  Ultra-Export neu gemessen werden.
- **Kein Upstream-Artefakt:** `ultra-int8-pc` gibt es nur aus eigener Erzeugung
  (siehe nächster Abschnitt).

**Empfehlung:** Vor einer Spec-Änderung einen Alltagstest mit `ultra-int8-pc` von
etwa ein bis zwei Wochen machen. Der f32-Debug-WAV-Ring aus 0.4.1 liefert dafür
bitgenaue Aufnahmen, die sich danach gegen beide Modelle nachrechnen lassen. Dafür
bräuchte es einen Weg, das Modell im Produkt zu tauschen. Das ist bereits eine
Produktänderung und nicht Teil dieses Spikes. Hält sich der Eindruck, dann wechseln.
Ohne Alltagstest ist die Datenlage für eine Umstellung des einzigen v1-Modells dünn.
Einen Rückschritt gegenüber v3-int8 zeigt sie aber nirgends, abgesehen von den
beiden Ring-Verschmelzungen.

`v3-fp32` und `ultra-fp32` scheiden aus: 2,5 GB Download, 2,7–3,0 GB RAM, +22–24 % Latenz.

## Was ein Wechsel im Produkt bedeuten würde (nicht umgesetzt)

- **Artefakte und Hosting:** Für `ultra-int8-pc` gibt es keine unveränderliche
  Upstream-URL. §6.3 verlangt `resolve/<git-commit>/…`. Mögliche Wege:
  (a) die zwei int8-Dateien in einem eigenen HF-Repo veröffentlichen, mit
  CC-BY-4.0-Nennung von Moondream und NVIDIA, und per Commit pinnen;
  (b) eines der Drittanbieter-Repos nutzen, die es laut HF-Suche gibt (z. B.
  `mldecode/parakeet-ultra-onnx-int8`, `Olicorne/parakeet-tdt-0.6b-v3-ultra-onnx`).
  Deren Inhalt, Methode und Layout habe ich **nicht** geprüft; sie bräuchten
  dieselbe Messung. Beim Nutzer quantisieren scheidet aus, denn das braucht Python.
- **`src/models.toml`:** neuer `key`, Encoder 700.507.227 B / `2cc01c15…`,
  Decoder 18.300.628 B / `afcb9459…`, `vocab.txt` unverändert (93.939 B /
  `d5854467…`). `config.json` (97 B) gibt es im Ultra-Ordner nicht. parakeet-rs
  liest es nicht (es sucht nur `vocab.txt` und die ONNX-Namen), daher ist zu
  entscheiden: streichen oder das v3-Exemplar weiterführen. Die Dateinamen
  `encoder-model.int8.onnx` / `decoder_joint-model.int8.onnx` bleiben, dadurch
  findet `find_encoder` sie ohne Codeänderung.
- **Schlüssel und Config:** Ein neuer Schlüssel (z. B. `parakeet-ultra-0.6b-int8`,
  Name offen) ändert `DEFAULT_MODEL`, die Config-Vorlage und die Tests in
  `download.rs`/`engine.rs`/`tray.rs`. **Achtung:** `config.rs` lehnt jeden Schlüssel
  außer `DEFAULT_MODEL` fatal ab. Ralfs `%APPDATA%\diktier\config.toml` enthält
  `model = "parakeet-tdt-0.6b-v3-int8"` ausdrücklich, und die Vorlage schreibt die
  Zeile mit. Nach einem Update ohne Migration wäre jede bestehende Config ein
  Fatal. Nötig wäre also ein Alias oder eine Migration mit Hinweis, wie bei
  `output.mode = "type"` in v1.8. Denselben Schlüssel mit getauschten Dateien
  weiterzuführen wäre schlechter, denn `check_artifacts` meldet dann
  `SizeMismatch` auf dem alten Verzeichnis statt eines sauberen Neudownloads.
- **Download und Verzeichnis:** neues `%LOCALAPPDATA%\diktier\models\<key>\` mit
  ~719 MB Erstdownload. Das alte Verzeichnis (670 MB) bliebe liegen; ob es
  aufgeräumt wird, ist offen.
- **`check_artifacts`:** Am Code ändert sich nichts, er prüft Existenz und Größe
  aus dem Manifest. `verify_artifacts_sha256` und der Download-Pfad arbeiten
  ebenfalls manifestgetrieben.
- **SPEC §6.2:** Die Tabelle bekommt den neuen Schlüssel als Default und einziges
  v1-Modell. Weiter 25 Sprachen und Auto-Detect laut Modellkarte.
- **SPEC §6.3:** Der Grundsatz „Golden Set byte-identisch zu Voxtype“ fällt weg,
  denn für Ultra gibt es keine Voxtype-Referenz. Neu festzuhalten wären Quelle
  (`moondream/parakeet-ultra` → `altunenes/parakeet-rs` Revision `4d2a8bc7…`),
  Quantisierungsrezept (ORT-Version, `QInt8`, `per_channel=True`), Hosting-Commit,
  Größen und Hashes. Das Phase-1-Gate „gegen Voxtype“ (§12, §18 #11) bräuchte
  eine neue Bezugsgröße, etwa v3-int8 auf den Fixtures.
- **SPEC §6.4 Vorlauf-Stille:** kann bleiben, sie kostet nichts. Ob sie mit Ultra
  noch nötig ist, zeigt diese Messung nur für drei Aufnahmen; ich würde sie nicht
  streichen.
- **Weitere Stellen:** `versions.toml` (`[model]` Repository/Revision), README
  und `LICENSES/` (Nennung Moondream), stt-smoke-Baselines (Zahlen als Ziffern)
  und Moondreams Hinweis, dass der VAD-Kopf fehlt. Diktier nutzt ihn nicht.

## Grenzen dieses Spikes

- Kein Silence-Gate im Messwerkzeug. Die Matrix enthält nur Sprache, 06/07 laufen
  im Produkt über D bzw. B3.
- Ring-Aufnahmen sind 16-bit, live bekommt die Engine f32. Mit dem Dither wird
  das angenähert, wie in SPIKES.
- Nur die Ersatzmethode (QInt8 per-channel) ist gemessen, keine weiteren Varianten
  (QUInt8 per-channel, statische Quantisierung, fp16).
- Nur dieser Laptop, parakeet-rs-Default mit 4 Threads.

## Dateien

Unter `.herd/spike-ultra/` (lokal, git-ausgeschlossen):

- `results.tsv`: alle Läufe; `rep` = A/B mit 300 ms Vorlauf, `nolead` ohne
  Vorlauf. Spalten: Modell, Lauf, Gruppe, Datei, Audiodauer, Inferenzzeit, WER,
  „Herr Präsident“-Flag, Text.
- `summary.md`: Tabellen und alle Textunterschiede, maschinell erzeugt.
- `src/main.rs`, `Cargo.toml`, `Cargo.lock`, `lib/onnxruntime.dll`: Messwerkzeug.
- `download.sh`, `make_lists.py`, `wavs.txt`, `wavs_ring.txt`, `dither/`,
  `quantize.py`, `quantize_pc.py`, `run_all.sh`, `analyze.py`,
  `analyze_extra.py`, `inspect_diff.py`, `out/`.

Modelle unter `D:\DEV\diktier\models\spike\` (gitignoriert, 7,4 GiB):
`v3-fp32`, `ultra-fp32`, `ultra-int8`, `ultra-int8-pc`, `v3-int8-pc` und
`v3-int8-selbst` (bitgleiche Reproduktion der Produktion, nur für den
Methodennachweis).
