# Parakeet Ultra (int8 per-channel) — Modellartefakte

Das Modellartefakt `parakeet-ultra-0.6b-int8-pc` (Release-Tag
`model-parakeet-ultra-0.6b-int8-pc-r1`) ist eine int8-Quantisierung von
**Moondream Parakeet Ultra**, einer nachtrainierten Fassung von **NVIDIA Parakeet
TDT 0.6B v3** (25 Sprachen, Auto-Detect). Es besteht aus drei Dateien:
`encoder-model.int8.onnx`, `decoder_joint-model.int8.onnx` und `vocab.txt`.

Lizenz: [Creative Commons Attribution 4.0 International](https://creativecommons.org/licenses/by/4.0/legalcode.en)
(CC-BY-4.0). Im Diktier-Bundle liegt der Lizenztext als `CC-BY-4.0.txt` daneben.

## Herkunftskette

| Stufe | Urheber | Werk | Revision |
|---|---|---|---|
| 1. Ursprungsmodell | NVIDIA | [`nvidia/parakeet-tdt-0.6b-v3`](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3) | von Moondream nicht angegeben; `main` steht seit 2026-08-05 auf `541d1f99c6b0c3cd0b11a95167540bb8edefd82b`, also auch bei Erscheinen von Parakeet Ultra (2026-09-22) |
| 2. Post-Training | Moondream | [`moondream/parakeet-ultra`](https://huggingface.co/moondream/parakeet-ultra) | von altunenes nicht angegeben; das Repository hat genau einen Commit, `73175eb7aeb0d82f1e2a6b53b3aabc10a90bcd0b` (2026-09-23) |
| 3. ONNX-Export | altunenes | [`altunenes/parakeet-rs`, Ordner `parakeet-ultra/`](https://huggingface.co/altunenes/parakeet-rs/tree/4d2a8bc71f5c896ec40faa59732e6716295edaf2/parakeet-ultra) | `4d2a8bc71f5c896ec40faa59732e6716295edaf2` |
| 4. int8-Quantisierung | Diktier-Projekt | [`ralfkuh-lab/diktier`](https://github.com/ralfkuh-lab/diktier), `scripts/quantize-ultra.py` | Commit des Rezepts: [`f6ae94f`](https://github.com/ralfkuh-lab/diktier/commit/f6ae94f1bfec55a692f4a4bdf6e473390d3a5ccd) |

Keine dieser Stufen ist eine offizielle Veröffentlichung einer früheren Stufe.
NVIDIA, Moondream und altunenes haben dieses Artefakt weder erstellt noch
geprüft, und sie billigen oder unterstützen es nicht.

## Hinweise der Quellen (wörtlich erhalten)

Aus der Modellkarte von `altunenes/parakeet-rs`, `parakeet-ultra/README.md` an
Revision `4d2a8bc7…`:

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

Aus der Modellkarte von `moondream/parakeet-ultra` an Revision `73175eb7…`:

> - Based on [parakeet-tdt-0.6b-v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3) by NVIDIA. Languages,
>   tokenizer and output conventions (punctuation, casing, numerals) are the original's.

> - License is CC-BY-4.0, same as the original.

Aus der Modellkarte von `nvidia/parakeet-tdt-0.6b-v3` an Revision `541d1f99…`:

> GOVERNING TERMS: Use of this model is governed by the [CC-BY-4.0](https://creativecommons.org/licenses/by/4.0/legalcode.en) license.

Einen Copyright-Vermerk oder einen eigenen Haftungsausschluss liefern die drei
Modellkarten nicht mit.

## Änderungen

- **Frühere Änderungen (Stufe 3, altunenes):** Umwandlung der
  Transformers-Safetensors in ONNX (fp32); der Voice-Activity-Kopf von Moondream
  fehlt. Siehe Zitat oben.
- **Diese Stufe (4):** Die Gewichte von Encoder und Decoder/Joint sind dynamisch
  nach int8 quantisiert, mit
  `onnxruntime.quantization.quantize_dynamic(weight_type=QuantType.QInt8, per_channel=True)`
  und Default-Operatortypen. Der Encoder ist dadurch eine einzige Datei ohne
  External Data. `vocab.txt` ist unverändert. Architektur und Tokenizer sind
  unverändert, weiter trainiert wurde nichts. Die Quantisierung verändert die
  Ausgaben gegenüber dem fp32-Export.

## Haftungsausschluss

Das Artefakt wird so, wie es ist, und so weit wie verfügbar bereitgestellt,
ohne Zusicherungen oder Gewährleistungen jeder Art, ausdrücklich, stillschweigend
oder gesetzlich, und ohne Haftung, soweit gesetzlich zulässig. Das entspricht
Section 5 (Disclaimer of Warranties and Limitation of Liability) der CC-BY-4.0,
die für alle Stufen gilt. Transkripte können fehlerhaft sein.

## Rezept

Umgebung: CPython 3.12.12, Windows x86_64, onnxruntime 1.30.0, onnx 1.23.1 und
die transitiven Pakete laut `scripts/quantize-ultra.requirements.txt` (mit
Hashes gepinnt).

```
uv venv --no-config --python 3.12.12 <venv>
uv pip sync --no-config --require-hashes --python <venv> scripts/quantize-ultra.requirements.txt
<venv>\Scripts\python scripts/quantize-ultra.py --out <dir>
```

Das Skript lädt die vier Quelldateien von
`https://huggingface.co/altunenes/parakeet-rs/resolve/4d2a8bc71f5c896ec40faa59732e6716295edaf2/parakeet-ultra/<datei>`
(oder nimmt sie mit `--source <dir>`), prüft Größe und SHA-256, quantisiert und
prüft die Ausgabe gegen die Sollwerte unten. Bei Abweichung bricht es ab.

### Quelldateien

| Datei | Bytes | SHA-256 |
|---|---:|---|
| `encoder-model.onnx` | 87857063 | `76f835e57d62d82f1485c7a84706782e44a123a69f4efa86ed3b4ad56e236051` |
| `encoder-model.onnx.data` | 2435420160 | `6aeb9438f1f45dafc17d27c61a12bc406c0c2ccb8c17219aeeb3f898c283a8e6` |
| `decoder_joint-model.onnx` | 72520894 | `a5911fe202e8fba44251fce252a6c9c7c0a7c724c882a13f81d96611fa2d7ccb` |
| `vocab.txt` | 93939 | `d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d` |

### Artefakt

| Datei | Bytes | SHA-256 |
|---|---:|---|
| `encoder-model.int8.onnx` | 700507227 | `2cc01c15a08d6976ca9ebe97739d15890f3088cfedd3a4aa4d969ba7a1702038` |
| `decoder_joint-model.int8.onnx` | 18300628 | `afcb9459250ab5c2e48e657d852501c233e8b7f5daed1894a2a7101d21165a5e` |
| `vocab.txt` | 93939 | `d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d` |
