# WP0 Ultra-Artefakt reproduzieren — Bericht (2026-09-30)

Auftrag: [impl-ultra-wp0-prompt.md](impl-ultra-wp0-prompt.md), Plan
[../ultra-alltagstest-plan.md](../ultra-alltagstest-plan.md) (v2), Stand
`2d10ee0`. Nichts committet, nichts gepusht, nichts veröffentlicht.

## ✅ Umgesetzt

- **`scripts/quantize-ultra.py`**
  - Quelle: die vier Dateien aus `altunenes/parakeet-rs@4d2a8bc7…/parakeet-ultra/`. Größe und SHA-256 stehen fest im Skript (`SOURCE`) und stimmen mit den LFS-oids der HF-API überein.
  - `--source <dir>` nimmt vorhandene Dateien. Ohne `--source` lädt das Skript nach `--download-dir` (Default `<out>.source`). Gültige Dateien dort nutzt es wieder; ungültige lädt es neu, über `.part` und Rename, mit Hashprüfung beim Schreiben.
  - **Das Quellverzeichnis wird nur gelesen.** Alle vier Quelldateien werden nach `<out>/.quantize-tmp/src` kopiert, Größe und SHA-256 werden dabei an der Kopie geprüft, quantisiert wird aus der Kopie. Die Kopien werden in einem `finally` immer entfernt. `--out` im oder gleich dem Quellverzeichnis ergibt Exit 2.
  - Prüfung, dass der Encoder seine External Data genau aus `encoder-model.onnx.data` daneben zieht.
  - `quantize_dynamic(weight_type=QuantType.QInt8, per_channel=True)`, Default-Operatortypen, für Decoder und Encoder. `vocab.txt` wird unverändert kopiert.
  - Ausgabe gegen feste Größen und Hashes (`OUTPUT`). Bei Abweichung Exit 1, die abweichende Ausgabe bleibt in `.quantize-tmp`, es gibt keine neuen Sollwerte. Nur bei Erfolg wandern die drei Dateien nach `--out`.
  - Protokolliert werden Python-Version, Implementierung und Pfad, Plattform, onnxruntime, onnx und numpy. Weicht Python, ORT oder onnx vom Lock ab, gibt es eine Warnung; entscheidend bleibt die Ausgabeprüfung.
  - Exitcodes: 0 ok, 1 Prüf- oder Laufzeitfehler, 2 Aufruf.
- **`scripts/quantize-ultra.requirements.txt`**: per `uv pip compile --generate-hashes` für CPython 3.12 unter Windows x86_64. Enthalten: onnxruntime 1.30.0, onnx 1.23.1, flatbuffers 25.12.19, ml-dtypes 0.6.0, numpy 2.5.3, packaging 26.3, protobuf 7.36.2 und typing-extensions 4.16.0. Die Python-Version (3.12.12) und die Erzeugungszeile stehen im Kopf. Der Paketsatz ist exakt der, unter dem der Spike quantisiert hat: Die uv-Cache-Umgebung des Spikes von 15:23 ist Python 3.12.12 mit denselben acht Paketen und Versionen.
- **`LICENSES/NOTICE-parakeet-ultra.md`** nach der WP0-Checkliste:
  - Herkunftskette mit Link und Revision je Stufe
  - CC-BY-4.0 mit Link auf den Legalcode
  - Hinweise der Quellen, wörtlich
  - Hinweis, dass die Quellen das Artefakt nicht billigen
  - Änderungshinweis mit Unterscheidung zwischen früheren und eigenen Änderungen
  - Haftungsausschluss mit Verweis auf §5
  - Rezept sowie Hashes der Quellen und des Artefakts

  Die v3-Notice ist unverändert.
- **Staging** `.herd/model-release/model-parakeet-ultra-0.6b-int8-pc-r1/`: die drei Laufzeitdateien, `NOTICE-parakeet-ultra.md` (Kopie aus `LICENSES/`) und `SHA256SUMS` über alle vier:

  ```
  2cc01c15a08d6976ca9ebe97739d15890f3088cfedd3a4aa4d969ba7a1702038 *encoder-model.int8.onnx
  afcb9459250ab5c2e48e657d852501c233e8b7f5daed1894a2a7101d21165a5e *decoder_joint-model.int8.onnx
  d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d *vocab.txt
  9cb8289d323ce9edf9658aa95cf27944e4575b1323a31444a3bba5602bb58afa *NOTICE-parakeet-ultra.md
  ```

### Herkunft, ermittelt über die HF-API (2026-09-30)

| Stufe | Revision | Wie ermittelt |
|---|---|---|
| NVIDIA `parakeet-tdt-0.6b-v3` | `541d1f99c6b0c3cd0b11a95167540bb8edefd82b` | Moondream nennt keine Revision. `main` steht laut Commit-Liste seit 2026-08-05 auf diesem Commit, also auch bei der Erstellung von `moondream/parakeet-ultra` (2026-09-22). Das ist ein Schluss aus Daten, keine Angabe von Moondream. So steht es auch in der NOTICE. |
| Moondream `parakeet-ultra` | `73175eb7aeb0d82f1e2a6b53b3aabc10a90bcd0b` | altunenes nennt keine Revision. Das Repo hat genau einen Commit (2026-09-23T00:03Z), vor dem altunenes-Commit (2026-09-23T21:44Z). |
| altunenes `parakeet-rs`, `parakeet-ultra/` | `4d2a8bc71f5c896ec40faa59732e6716295edaf2` | fest vorgegeben |

## Gelieferte Hinweise der Quellen (wörtlich)

Zu §3(a)(1) der CC-BY-4.0: Keine der drei Modellkarten liefert einen
**Copyright-Vermerk** oder einen **eigenen Haftungsausschluss**. Geliefert
werden Urhebernennung, Lizenzverweis und Änderungshinweise:

**altunenes**, `parakeet-ultra/README.md` @ `4d2a8bc7…`. Front-Matter
`license: cc-by-4.0`, `base_model: moondream/parakeet-ultra`. Zitiert:

> The model and its weights are Moondream's, built on NVIDIA's. This folder only converts them to
> ONNX; it is not an official release from either.

> ## Changes from the original
>
> - Converted from Hugging Face Transformers safetensors to ONNX.
> - Moondream's small voice-activity head, used by their Photon runtime to split long audio, is not
>   included.

> ## License
>
> [CC-BY-4.0](https://creativecommons.org/licenses/by/4.0/), the same as the original. Parakeet Ultra by
> [Moondream](https://huggingface.co/moondream), based on parakeet-tdt-0.6b-v3 by
> [NVIDIA](https://huggingface.co/nvidia).

**Moondream**, `README.md` @ `73175eb7…`. Front-Matter `license: cc-by-4.0`.
Abschnitt „Notes“:

> - Based on [parakeet-tdt-0.6b-v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3) by NVIDIA. Languages,
>   tokenizer and output conventions (punctuation, casing, numerals) are the original's.

> - License is CC-BY-4.0, same as the original.

**NVIDIA**, `README.md` @ `541d1f99…`. Front-Matter `license: cc-by-4.0`.
Abschnitt „License/Terms of Use“, gleichlautend in der Zeile „Licensing“:

> GOVERNING TERMS: Use of this model is governed by the [CC-BY-4.0](https://creativecommons.org/licenses/by/4.0/legalcode.en) license.

Alle diese Stellen stehen wörtlich in der NOTICE. Nicht übernommen habe ich
Benchmarks, Nutzungsbeispiele und die NVIDIA-Abschnitte zu Privacy, Safety und
Trustworthy AI; das sind keine Lizenzhinweise.

## Abweichungen

1. **Aufruf ohne `uv run --with-requirements`.** Der Auftrag nennt ihn als Beispiel. Er prüft die Hashes nicht: Eine Kopie des Locks mit einem verfälschten flatbuffers-Hash installierte ohne Fehler, auch mit `--no-cache`:

   ```
   $ uv run --no-project --no-config --no-cache --python 3.12.12 --with-requirements badhash.txt python -c "print('lief')"
   …
   Installed 8 packages in 1.23s
   lief
   EXIT 0
   ```

   `uv pip sync --require-hashes` bricht dagegen ab:

   ```
   $ uv pip sync --no-config --require-hashes badhash.txt   (VIRTUAL_ENV=venv-bad)
         Expected:
           sha256:0000f50c427838bb021c2d66a3d1168e9d199b0607e6329399f04846d42e20b4

         Computed:
           sha256:7634f50c427838bb021c2d66a3d1168e9d199b0607e6329399f04846d42e20b4
   EXIT 1
   ```

   Die Aufrufzeile im Skriptkopf und in der NOTICE lautet deshalb:

   ```
   uv venv --no-config --python 3.12.12 <venv>
   uv pip sync --no-config --require-hashes --python <venv> scripts/quantize-ultra.requirements.txt
   <venv>\Scripts\python scripts/quantize-ultra.py --source models\spike\ultra-fp32 --out <dir>
   ```

2. **Staging-Kopie der Quelle, nach dem Hinweis des Orchestrators.** `quantize_dynamic` schreibt bei Pfad-Eingabe `<modell>-inferred.onnx` neben das Modell und löscht die Datei nur bei Erfolg (`onnxruntime/quantization/quant_utils.py:1238–1244`).
   - Die erste Fassung quantisierte direkt aus `--source`. Ein abgebrochener Lauf (siehe 3.) hinterließ `models\spike\ultra-fp32\encoder-model-inferred.onnx` (88.167.843 B). Die Datei ist entfernt, und nur diese.
   - Die Quelle ist wieder unverändert: vier Dateien, mtimes von 15:14–15:22, Linkzahl je 1.
   - Das Skript kopiert jetzt nach `<out>/.quantize-tmp/src`, siehe oben. Das kostet einmal 2,6 GB Platz und einige Sekunden.
3. **Selbst verursachter Fehlschlag, behoben.** Für den Negativtest hatte ich Hardlinks auf die Quelldateien angelegt, während Gate 1 lief. onnx 1.23.1 verweigert External Data mit mehreren Hardlinks („… but it has multiple hard links, indicating a potential hardlink attack.“), und der Lauf endete mit Exit 1.
   - Die Hardlinks sind entfernt.
   - Die Negativtests arbeiten jetzt mit echten Kopien.
   - Durch die Staging-Kopie (2.) kann ein Hardlink auf die Quelle das Skript nicht mehr stören.
4. **Zusätzlich zum Auftrag:** Prüfung der External-Data-Verweise, Schutz gegen `--out` in der Quelle, ein Download-Test (unten) und die Prüfung, dass beide int8-Dateien ohne External Data sind (je 0 externe Initializer, Producer `onnx.quantize 0.1.0`).

## Gate-Ausgaben (wörtlich)

Die Warnungen von onnxruntime („Please consider to run pre-processing …“,
„Inference failed or unsupported type to quantize for tensor …“) sind
gekürzt. Sie erschienen im Spike genauso (`.herd/spike-ultra/quant-ultra-pc.log`).

### Gate 1: Lauf aus `models\spike\ultra-fp32`, bitgleich zum Spike

Endgültige Skriptfassung, Umgebung frisch per `uv venv` und `uv pip sync --require-hashes`.
Von uv sind die Fortschrittszeilen (Resolved, Downloading, Prepared) weggelassen:

```
PS> uv venv --no-config --python 3.12.12 .herd\venv-quantize-ultra
Using CPython 3.12.12
EXIT venv 0
PS> uv pip sync --no-config --require-hashes --python .herd\venv-quantize-ultra scripts/quantize-ultra.requirements.txt
 + flatbuffers==25.12.19
 + ml-dtypes==0.6.0
 + numpy==2.5.3
 + onnx==1.23.1
 + onnxruntime==1.30.0
 + packaging==26.3
 + protobuf==7.36.2
 + typing-extensions==4.16.0
EXIT sync 0
PS> .herd\venv-quantize-ultra\Scripts\python scripts/quantize-ultra.py --source models\spike\ultra-fp32 --out .herd\model-release\model-parakeet-ultra-0.6b-int8-pc-r1
Python 3.12.12 (CPython, D:\DEV\diktier\.herd\venv-quantize-ultra\Scripts\python.exe)
Plattform Windows-11-10.0.22631-SP0 AMD64
onnxruntime 1.30.0, onnx 1.23.1, numpy 2.5.3
Quelle: D:\DEV\diktier\models\spike\ultra-fp32 (vorhanden, altunenes/parakeet-rs@4d2a8bc71f5c896ec40faa59732e6716295edaf2)
Kopiere und pruefe Quelldateien nach D:\DEV\diktier\.herd\model-release\model-parakeet-ultra-0.6b-int8-pc-r1\.quantize-tmp\src:
  ok   encoder-model.onnx  87857063 B  76f835e57d62d82f1485c7a84706782e44a123a69f4efa86ed3b4ad56e236051
  ok   encoder-model.onnx.data  2435420160 B  6aeb9438f1f45dafc17d27c61a12bc406c0c2ccb8c17219aeeb3f898c283a8e6
  ok   decoder_joint-model.onnx  72520894 B  a5911fe202e8fba44251fce252a6c9c7c0a7c724c882a13f81d96611fa2d7ccb
  ok   vocab.txt  93939 B  d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d
Quantisiere (QInt8, per_channel=True):
  decoder_joint-model.int8.onnx 18300628 B in 2 s
  encoder-model.int8.onnx 700507227 B in 47 s
  vocab.txt kopiert
Pruefe Ausgabe:
  ok   encoder-model.int8.onnx  700507227 B  2cc01c15a08d6976ca9ebe97739d15890f3088cfedd3a4aa4d969ba7a1702038
  ok   decoder_joint-model.int8.onnx  18300628 B  afcb9459250ab5c2e48e657d852501c233e8b7f5daed1894a2a7101d21165a5e
  ok   vocab.txt  93939 B  d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d
OK: 3 Dateien in D:\DEV\diktier\.herd\model-release\model-parakeet-ultra-0.6b-int8-pc-r1
EXIT 0

$ for f in encoder-model.int8.onnx decoder_joint-model.int8.onnx vocab.txt; do cmp $R/$f models/spike/ultra-int8-pc/$f && echo "cmp identisch: $f"; done
cmp identisch: encoder-model.int8.onnx
cmp identisch: decoder_joint-model.int8.onnx
cmp identisch: vocab.txt
```

Danach enthält `models\spike\ultra-fp32\` nur die vier Quelldateien, unverändert.
Unter `%TEMP%` liegen keine `ort.quant.*`-Verzeichnisse, und
`.quantize-tmp` ist weg.

Die erste Fassung (mit `uv run`, ohne Staging) war ebenfalls grün und
bitgleich. Die zweite scheiterte an meinen Hardlinks (Abweichung 3).

### Gate 2: Negativtests mit manipulierten Kopien (`.herd\wp0-negtest\`, danach gelöscht)

a) `decoder_joint-model.onnx`: ein Bit an Offset 1.000.000 gekippt, Größe gleich:

```
PS> .herd\venv-quantize-ultra\Scripts\python scripts/quantize-ultra.py --source .herd\wp0-negtest\src-hash --out .herd\wp0-negtest\out
…
Kopiere und pruefe Quelldateien nach D:\DEV\diktier\.herd\wp0-negtest\out\.quantize-tmp\src:
  ok   encoder-model.onnx  87857063 B  76f835e57d62d82f1485c7a84706782e44a123a69f4efa86ed3b4ad56e236051
  ok   encoder-model.onnx.data  2435420160 B  6aeb9438f1f45dafc17d27c61a12bc406c0c2ccb8c17219aeeb3f898c283a8e6
  FEHL D:\DEV\diktier\.herd\wp0-negtest\src-hash\decoder_joint-model.onnx: SHA-256 a4600ed037348bc60251f5ccb4b748eac6706b920846ead39612a1784acccab3 (72520894 B), erwartet a5911fe202e8fba44251fce252a6c9c7c0a7c724c882a13f81d96611fa2d7ccb (72520894 B)
  ok   vocab.txt  93939 B  d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d
FEHLER: Quelle: 1 Datei(en) weichen ab

EXIT 1
```

b) `vocab.txt`: um ein Byte gekürzt:

```
  FEHL D:\DEV\diktier\.herd\wp0-negtest\src-hash\vocab.txt: Groesse 93938 B, erwartet 93939 B
FEHLER: Quelle: 1 Datei(en) weichen ab

EXIT 1
Inhalt out: 0
```

c) `--out` im Quellverzeichnis:

```
quantize-ultra.py: error: --out darf nicht im Quellverzeichnis liegen

EXIT guard 2
x existiert: False
```

Download-Pfad, zusätzlich: Das Download-Verzeichnis enthielt die drei großen
Dateien und das gekürzte `vocab.txt`.

```
PS> .herd\venv-quantize-ultra\Scripts\python scripts/quantize-ultra.py --download-dir .herd\wp0-negtest\src-hash --out .herd\wp0-negtest\out-dl
Quelle: https://huggingface.co/altunenes/parakeet-rs/resolve/4d2a8bc71f5c896ec40faa59732e6716295edaf2/parakeet-ultra -> D:\DEV\diktier\.herd\wp0-negtest\src-hash
  vorhanden und gueltig: encoder-model.onnx
  vorhanden und gueltig: encoder-model.onnx.data
  vorhanden und gueltig: decoder_joint-model.onnx
  ungueltig, lade neu: D:\DEV\diktier\.herd\wp0-negtest\src-hash\vocab.txt: Groesse 93938 B, erwartet 93939 B
  lade https://huggingface.co/altunenes/parakeet-rs/resolve/4d2a8bc71f5c896ec40faa59732e6716295edaf2/parakeet-ultra/vocab.txt
  geladen vocab.txt 93939 B in 0 s
…
  ok   encoder-model.int8.onnx  700507227 B  2cc01c15a08d6976ca9ebe97739d15890f3088cfedd3a4aa4d969ba7a1702038
  ok   decoder_joint-model.int8.onnx  18300628 B  afcb9459250ab5c2e48e657d852501c233e8b7f5daed1894a2a7101d21165a5e
  ok   vocab.txt  93939 B  d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d
OK: 3 Dateien in D:\DEV\diktier\.herd\wp0-negtest\out-dl
EXIT 0
```

### Gate 3: `sha256sum -c SHA256SUMS` im Staging-Verzeichnis

```
$ sha256sum -c SHA256SUMS
encoder-model.int8.onnx: OK
decoder_joint-model.int8.onnx: OK
vocab.txt: OK
NOTICE-parakeet-ultra.md: OK
EXIT 0
```

## 🔍 Offen

- **Vollständiger Download der drei großen Dateien ungetestet.** Geprüft sind der Download von `vocab.txt` und die Wiederverwendung. Die LFS-Dateien liefen über denselben Codepfad (`urllib`, Redirect auf das HF-CDN); dass das für 2,4 GB durchläuft, ist nicht gemessen.
- **Kein Test für eine abweichende Ausgabe.** Dafür bräuchte es eine andere ORT-Version. Der Pfad nutzt dieselbe Prüffunktion wie die Quelle.
- **NOTICE, Stufe 4:** Die Zeile „Commit des Rezepts: wird beim Release eingetragen“ muss vor dem Upload in WP3a den Commit bekommen, der Skript und Lock enthält. Danach ändert sich der NOTICE-Hash; `SHA256SUMS` muss dann neu erzeugt werden.
- **Zeilenenden der NOTICE:** Das Repo hat `core.autocrlf=true` und keine `.gitattributes`. Eine frische Windows-Checkout-Kopie von `LICENSES/NOTICE-parakeet-ultra.md` hätte CRLF und damit einen anderen Hash als das Release-Asset (LF). Für WP3a heißt das: das Asset aus dem Staging hochladen, nicht aus einem Checkout, oder `.gitattributes` festlegen (Entscheidung Orchestrator/Ralf).
- **`SHA256SUMS`** hat das GNU-Binärformat (`<hash> *<datei>`), wie es `sha256sum` unter Git Bash schreibt. `sha256sum -c` akzeptiert es auf allen Plattformen.
- **Lokal liegen geblieben** (git-ausgeschlossen): `.herd\venv-quantize-ultra\` (gepinnte Umgebung, wiederverwendbar) und das Staging-Verzeichnis. Die Negativtest-Kopien sind gelöscht.
- **Fremde Änderungen im Arbeitsbaum:** `git status` zeigt geänderte Dateien in `src/`, `src/models.toml` und `docs/SPEC.md` sowie ein neues `docs/reviews/impl-ultra-wp2a-prompt.md`. Die stammen nicht aus diesem Paket; ich habe sie nicht angefasst.
