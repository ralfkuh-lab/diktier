Auftrag: WP0 aus docs/ultra-alltagstest-plan.md (v2) umsetzen, also das Ultra-int8-per-channel-Artefakt reproduzierbar lokal erzeugen. Diktier, Windows-only, Stand `2d10ee0`. Nichts wird veröffentlicht, nichts gepusht.

Lies zuerst:
1. docs/ultra-alltagstest-plan.md: Leitentscheidungen 2–4, WP0, „Umgang mit dem Review“ (W5, H2) und „Entscheidungen“ (F6: Modell-Repo `ralfkuh-lab/diktier-models`, Tag `model-parakeet-ultra-0.6b-int8-pc-r1`).
2. docs/reviews/spike-parakeet-ultra-notes.md, „Wie Ultra-int8 entstanden ist“ und die Hash-Tabellen.
3. Die Spike-Skripte `.herd/spike-ultra/quantize_pc.py`, `.herd/spike-ultra/download.sh` (lokal).
4. LICENSES/ (bestehende v3-Notice als Stilvorlage) und die Modellkarten der Kette: https://huggingface.co/altunenes/parakeet-rs/blob/4d2a8bc71f5c896ec40faa59732e6716295edaf2/parakeet-ultra/README.md, https://huggingface.co/moondream/parakeet-ultra, https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3, https://creativecommons.org/licenses/by/4.0/legalcode.en (§3(a)).

Umfang:
- `scripts/quantize-ultra.py`:
  - Quelle sind die vier Dateien aus `altunenes/parakeet-rs` an Revision `4d2a8bc71f5c896ec40faa59732e6716295edaf2`, Ordner `parakeet-ultra/`: `encoder-model.onnx`, `encoder-model.onnx.data`, `decoder_joint-model.onnx`, `vocab.txt`. Größe und SHA-256 stehen fest im Skript.
  - Mit `--source <dir>` nimmt es vorhandene Dateien, zum Beispiel `models\spike\ultra-fp32`, und prüft sie. Ohne `--source` lädt es von `https://huggingface.co/altunenes/parakeet-rs/resolve/<revision>/parakeet-ultra/<datei>` und prüft ebenfalls.
  - Quantisierung: `quantize_dynamic(weight_type=QInt8, per_channel=True)` für Encoder und Decoder. Die External-Data-Datei muss neben dem Encoder liegen.
  - Ausgabe nach `--out <dir>`: `encoder-model.int8.onnx`, `decoder_joint-model.int8.onnx` und `vocab.txt` (unverändert kopiert).
  - Prüfung der Ausgabe gegen feste Größen und Hashes (Encoder 700507227 B `2cc01c15a08d6976ca9ebe97739d15890f3088cfedd3a4aa4d969ba7a1702038`, Decoder 18300628 B `afcb9459250ab5c2e48e657d852501c233e8b7f5daed1894a2a7101d21165a5e`, vocab 93939 B `d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d`). Bei Abweichung Exit ≠ 0, keine neuen Sollwerte.
  - Das Skript protokolliert Python-, onnx- und onnxruntime-Version.
- Reproduzierbare Umgebung: `scripts/quantize-ultra.requirements.txt`, mit Hashes gepinnt (z. B. per `uv pip compile --generate-hashes`), onnxruntime 1.30.0 und onnx 1.23.1 samt transitiven Paketen. Dazu die Python-Version. Aufrufzeile im Skriptkopf, isoliert, z. B. `uv run --no-project --python <ver> --with-requirements scripts/quantize-ultra.requirements.txt python scripts/quantize-ultra.py …`.
- `LICENSES/NOTICE-parakeet-ultra.md` nach der Checkliste in WP0: NVIDIA als Ursprung, Moondream als Post-Training, altunenes als ONNX-Export (VAD-Kopf fehlt), eigene Quantisierung. Je Stufe Link und Revision bzw. Commit, soweit ermittelbar. CC-BY-4.0 mit Link. Hinweise, die die Quellen mitliefern, bleiben erhalten; was sie liefern, steht wörtlich in deinem Bericht. Änderungshinweis, Haftungsausschluss, Rezept und Hashes. Die bestehende v3-Notice bleibt unverändert.
- Release-Staging unter `.herd/model-release/model-parakeet-ultra-0.6b-int8-pc-r1/` (lokal, git-ausgeschlossen): die drei Laufzeitdateien, `NOTICE-parakeet-ultra.md` und `SHA256SUMS` im `sha256sum`-Format über alle vier.

Gates (Befehl und tatsächliche Ausgabe zitieren):
1. Das Skript mit `--source models\spike\ultra-fp32 --out .herd\model-release\…` läuft grün, die Ausgabe ist bitgleich zu `models\spike\ultra-int8-pc\` (per `cmp` oder Hash).
2. Negativtest: eine manipulierte Kopie einer Quelldatei in einem Temp-Verzeichnis ergibt Exit ≠ 0 mit klarer Meldung.
3. `sha256sum -c SHA256SUMS` im Staging-Verzeichnis ist grün.

Regeln:
- Nicht committen, nicht pushen, nichts auf GitHub oder Hugging Face veröffentlichen.
- Keine Änderungen an src/, Cargo.*, docs/ (außer deinem Bericht), testdata/, bestehenden Dateien unter LICENSES/. Nichts in `%LOCALAPPDATA%\diktier` und `%TEMP%\diktier`; keinen laufenden Daemon und keine installierte Version anfassen.
- Keine globalen Python-Installationen, nur isoliert per `uv`.
- Kein herdr, keine weiteren Panes.
- Rückfragen: Frage nach `D:\DEV\diktier\.herd\fragen\impl-ultra-wp0.md`, Turn mit `RUECKFRAGE: D:\DEV\diktier\.herd\fragen\impl-ultra-wp0.md` beenden.

Bericht nach docs/reviews/impl-ultra-wp0-notes.md: Umgesetzt, gelieferte Hinweise der Quellen (wörtlich), Abweichungen, Gate-Ausgaben wörtlich, Offen. TUI-Ausgabe knapp.
