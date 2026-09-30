# WP2a — Mehrmodell-Vertrag (0.5.0, Teil 1): Umsetzungsbericht

Stand: 2026-09-30. Auftrag: [impl-ultra-wp2a-prompt.md](impl-ultra-wp2a-prompt.md).
Vorgabe waren SPEC v1.10 (§6.2, §6.3, §8, §11, §18 #16) und
[../ultra-alltagstest-plan.md](../ultra-alltagstest-plan.md) (WP2a; Sol B3, W3
und W8). Basis war der Working Tree auf `2d10ee0`. Es gibt keinen Commit und
keinen Push.

## ✅ Umgesetzt

### Manifest `src/models.toml`

- Das Manifest steht jetzt auf `[[models]]` mit `[[models.files]]`. Dazu kommt
  ein Top-Level-Feld `default_model = "parakeet-tdt-0.6b-v3-int8"`.
- Jedes Modell hat Herkunftsfelder: `source` (`huggingface` |
  `github-release`), `repository` und je nach Quelle `revision` oder
  `release_tag`. Ohne diese Felder ließe sich `versions.toml` nicht bauen, ohne
  Werte aus URLs zu raten.
- Der v3-Eintrag ist in allen Werten gleich geblieben, URLs eingeschlossen. Das
  habe ich mit `tomllib` gegen `git show HEAD:src/models.toml` geprüft:
  `v3 files identisch (name/bytes/sha256/url): True 4`.
- Ultra folgt der SPEC-Tabelle in §6.3: drei Dateien, kein `config.json`, URLs
  `https://github.com/ralfkuh-lab/diktier-models/releases/download/model-parakeet-ultra-0.6b-int8-pc-r1/<datei>`.
- Die Prüfung beim Parsen (`download::parse_catalog`) bricht ab, wenn eine
  dieser Regeln verletzt ist:
  - Schlüssel sind eindeutig.
  - Schlüssel, Dateinamen, Repo-Teile und Tag sind sichere Pfadbestandteile:
    nur `[A-Za-z0-9._-]`, alphanumerisch am Anfang, kein `..`.
  - Dateinamen sind nicht `COMPLETE` und enden nicht auf `.part`.
  - `bytes > 0`, und `sha256` besteht aus 64 Hex-Zeichen.
  - Eine HF-Revision ist ein voller 40-stelliger Commit.
  - Jede Datei-URL ist **genau** die URL, die aus der Herkunft folgt. Damit
    fallen Redirect-Ziele und `latest` durch.
  - `default_model` steht unter den Modellen.
  - Unbekannte Felder sind nicht erlaubt (`deny_unknown_fields`).

### Auswahl: einmal gewählt, danach nur durchgereicht

- `download::load_manifest(key)` gibt bei einem unbekannten Schlüssel
  `DownloadError::UnknownModel` zurück, mit der Meldung
  `Modellschlüssel "x" ist unbekannt (erlaubt: "parakeet-tdt-0.6b-v3-int8", "parakeet-ultra-0.6b-int8-pc")`.
  Ein Ersatzmodell gibt es nicht.
- Neu ist `download::SelectedModel`: Manifest und Verzeichnis
  `%LOCALAPPDATA%\diktier\models\<key>\`. Es entsteht über
  `SelectedModel::select(key)` und liefert `key()`, `manifest()`, `dir()` und
  `check()` (Existenz und Größe). Der geparste Katalog ist per `OnceLock`
  zwischengespeichert. `model_keys()`, `default_model_key()` und
  `allowed_models_hint()` bedienen Config und Tests.

So kommt das Manifest bei den Verbrauchern an:

| Verbraucher | Wie das Manifest ankommt |
|---|---|
| Daemon (`daemon/mod.rs::run_locked`) | Die **einzige** Auswahlstelle des Laufs ist `SelectedModel::select(&config.engine.model)`. `UnknownModel` → `config_error_mode` (Tray `error`, kein Hotkey), andere Manifestfehler → Exit 1. Das Ergebnis liegt in `Daemon.model`. |
| Startprüfung (`Actors::check_artifacts`) | Die neue freie Funktion `artifacts_checked(&self.model, …)` ruft `model.check()` auf. |
| Download (`DownloadWorker::spawn(run, self.model.clone(), …)`) | `download_loop` holt nur noch den Lock-Pfad und übergibt an `workers::run_download(run, &model, transport, lock_path, …)`. Der Download läuft in `model.dir()` mit `model.manifest()`. Die Funktion ist ohne Thread und mit austauschbarem Transport testbar. |
| Engine (`EngineWorker::spawn(self.model.clone(), …)`) | `ParakeetTranscriber::load(&SelectedModel, threads)` wählt nichts mehr selbst. Der alte Abgleich `manifest.key == key` entfällt, geprüft wird `model.check()`, danach `from_pretrained(model.dir())`. Die Logzeile `Modell geladen in … s (<key>)` bleibt unverändert, das Gate in WP3b stützt sich darauf. |
| Tray | `TrayWorker::spawn(model.key().to_string(), …)`. Im Configfehler-Modus steht im Tooltip jetzt `kein Modell` statt v3, siehe Abweichungen. |
| `engine::model_artifacts(key)` | Gibt `SelectedModel` zurück (vorher ein Tupel mit dem einzigen Manifest) und nimmt genau den übergebenen Schlüssel. Ist er unbekannt, ist das ein `EngineError::Artifacts` mit der Liste der erlaubten Werte. |
| CLI `--transcribe-wav`, `--record-test` (`main.rs`) | `engine::model_artifacts(&config.engine.model)` → `ParakeetTranscriber::load(&model, threads)`. `--model` gehört zu WP2c. |
| stt-smoke | `model_artifacts(DEFAULT_MODEL)`. Prüfung, SHA-Vollcheck und Laden laufen über **dasselbe** `SelectedModel`. Vorher wurde für Prüfung und Laden getrennt gewählt. |

### Config (`src/config.rs`)

- `engine.model` wird gegen `download::model_keys()` geprüft. Ein unbekannter
  Schlüssel bleibt fatal, mit der Meldung
  `ungültiges engine.model "x" (erlaubt: "parakeet-tdt-0.6b-v3-int8", "parakeet-ultra-0.6b-int8-pc")`.
- `DEFAULT_MODEL` bleibt v3. Ein Test hält es gleich `default_model` im
  Manifest.
- Die Vorlage wie SPEC §8:
  `model = "parakeet-tdt-0.6b-v3-int8"   # v1.10 auch "parakeet-ultra-0.6b-int8-pc"`.

### `scripts/release.ps1`

- Neu ist ein strenger Leser für die TOML-Teilmenge (`Read-TomlSubset`). Er
  kennt Kommentare, `[t]`, `[[l]]`, `[[l.u]]` sowie Text, Ganzzahl, Bool und
  einzeilige Textlisten. Alles andere bricht ab.
- `Get-ModelCatalog` liest alle Modelle strukturiert, mit denselben Regeln wie
  die Rust-Seite (URL folgt aus der Herkunft, sichere Namen, Eindeutigkeit,
  Default vorhanden). Regex auf den ersten Treffer und HF-Ableitung aus der URL
  entfallen.
- Zusätzlich gleicht das Skript `default_model` mit
  `pub const DEFAULT_MODEL` in `src/config.rs` ab, weil das Release-Skript
  ohne `cargo test` läuft.
- `versions.toml` bekommt `default_model` auf oberster Ebene und je Modell
  einen `[[models]]`-Block: `key`, `source`, `repository`, `revision` bzw.
  `release_tag`, dazu `[[models.files]]` mit `name`, `bytes` und `sha256`. Der
  alte `[model]`-Block und der Kommentar „vier Artefakte“ sind weg; der neue
  Kommentar bezieht sich auf das jeweilige Modell.
- **Bundle-Gate** `Assert-VersionsMatchManifest`: Es liest die erzeugte Datei
  zurück und vergleicht Feld für Feld mit dem Katalog (Default, Anzahl, je
  Schlüssel Quelle, Repo, Revision/Tag, Dateien, Größen und Hashes, keine
  fremden Felder, kein alter `[model]`-Block, `[app].version`). Bei jeder
  Abweichung bricht es ab. Der Skriptkopf beschreibt das.
- Negativprobe der Skriptfunktionen, per AST aus `release.ps1` geladen, im
  Scratchpad und nicht im Repo:
  - Diese Abweichungen hat das Gate erkannt und abgebrochen: falscher Hash,
    falsche Größe, falscher Tag, falsche Revision, falscher Default, falsche
    App-Version, fehlendes Ultra, fehlende Datei, Extra-Feld, alter
    `[model]`-Block, nicht unterstützte Syntax.
  - Ebenso die Manifest-Fehler: URL `latest` statt Tag, doppelter Schlüssel,
    `../x`, Default fehlt, Tag und Revision zugleich.
- Unabhängige Gegenprobe der echten `versions.toml` mit Pythons `tomllib`
  gegen `models.toml`: `True`.

### Tests (ohne echte große Dateien)

- **Manifest** (`download.rs`):
  - `golden_set_matches_spec`: v3 samt URLs.
  - `ultra_set_matches_spec`
  - `catalog_origin_is_structured_per_model`
  - `catalog_keys_are_unique_and_default_is_v3`
  - `model_dirs_are_safe_and_separate`
  - `unknown_key_is_an_error_naming_the_allowed_keys`
  - `parse_catalog_accepts_the_mini_catalog`
  - `parse_catalog_rejects_broken_entries`: 19 Negativfälle.
  - `github_origin_requires_the_canonical_release_url`
- **Config:**
  - `fatal_invalid_engine_model`, erweitert: Die Meldung nennt beide Werte.
  - `both_manifest_models_are_accepted`
  - `missing_engine_model_is_the_default`
  - `default_model_is_the_manifest_default`
  - `default_file_writes_v3_and_names_the_other_model`
- **Downloader:**
  - `three_file_set_without_config_json_downloads_completely`: echte
    Ultra-Namen und -URLs, Fake-Inhalte.
- **Download-Sperre:**
  - `shared_download_lock_with_separate_model_dirs`: Die gemeinsame Sperre
    bleibt (Sol W3). Lädt v3, bekommt Ultra `Busy`. Danach hat jedes Modell
    sein eigenes Verzeichnis, keines berührt das andere.
- **Engine:**
  - `model_artifacts_rejects_unknown_model_key`: ersetzt
    `load_rejects_unknown_model_key`.
  - `model_artifacts_selects_the_requested_key`
- **Daemon/Tray** (neues Testmodul `src/daemon/model_wiring_tests.rs`): Der
  echte Kern (`transition` + `drive`) treibt eine Probe. Für Prüfung und
  Download nimmt sie dieselben Funktionen wie der Daemon
  (`artifacts_checked`, `run_download`). Jeder Test läuft für beide Schlüssel:
  - `each_model_downloads_loads_and_idles_on_its_own_path`: Tray
    `starting → downloading → loading → idle`, Hotkey scharf. Nur die eigenen
    URLs werden geholt, obwohl der Fake auch das andere Modell liefern würde.
    Geladen wird genau `(key, root\key)`, `COMPLETE` enthält den Schlüssel.
    Das andere Verzeichnis entsteht nicht, der Tooltip lautet `idle — <key>`.
    Der zweite Start geht `starting → loading → idle` ohne Netz.
  - `a_failed_download_is_fatal_without_fallback_to_the_other_model`: Das
    andere Modell liegt vollständig daneben. Ergebnis ist `error` mit
    `ModelDownload`, Hotkey und Tray-Klick bleiben aus. Es wird nichts geladen
    und nur die eigene erste URL angefragt. Das andere Verzeichnis bleibt
    bytegleich.
  - `a_wrong_size_in_the_selected_dir_triggers_its_own_download`
- Der v3-Golden-Set-Test und `stt_smoke_fixtures` bleiben beim v3-Schlüssel.

### Version

- `Cargo.toml` steht auf 0.5.0. `Cargo.lock` hat der Build nachgezogen (eine
  Zeile). README und `LICENSES/` habe ich nicht angefasst.

## Abweichungen und warum

- **`default_model` im Manifest.** SPEC §11 verlangt `default_model` in
  `versions.toml`, strukturiert und nicht erraten. Das Release-Skript braucht
  dafür eine lesbare Quelle. Ich habe `models.toml` als Quelle gewählt statt
  eines Regex auf Rust-Quelltext. `DEFAULT_MODEL` in `config.rs` bleibt
  bestehen, weil Defaults und die Vorlage es als `&'static str` brauchen.
  Gleich gehalten wird es von einem Unit-Test und zusätzlich vom Skript.
- **Herkunft nur im Katalog.** `ArtifactManifest` bleibt `{ key, files }`, so
  wie Download, Engine und die Fakes es brauchen. Die Herkunftsfelder stehen
  im privaten `RawModel`, prüfen dort die URLs und werden vom Release-Skript
  aus der TOML gelesen.
- **Tray im Configfehler-Modus.** Dort stand bisher der v3-Schlüssel im
  Tooltip, auch wenn `engine.model` falsch war. Das deutet einen v3-Fallback
  an, den es nicht gibt. Jetzt steht dort `kein Modell`
  (`error — kein Modell — <Meldung>`).
- **Randfall ohne `%LOCALAPPDATA%`.** Das Modellverzeichnis wird jetzt einmal
  beim Start bestimmt. Fehlt die Variable, endet `run_locked` mit Exit 1 und
  Logzeile. Vorher kam dieser Fall erst über `downloading` in den
  Tray-`error`. Praktisch kommt man nicht so weit, denn die Instanz- und
  Logpfade vor `run_locked` brauchen dieselbe Variable.
- **`release.ps1`-Schalter.** Der Auftrag nennt `-NoInstaller`, das Skript
  heißt laut Kopf `-SkipInstaller`. Genutzt habe ich `-SkipInstaller`.
- **Zeilenenden.** `cargo fmt` hat `src/daemon/mod.rs` im Working Tree von CRLF
  auf LF gestellt. Der Index ist LF, `git diff` zeigt nur die inhaltlichen
  Änderungen. `scripts/release.ps1` bleibt bei CRLF mit BOM.
- **Widersprüche SPEC ↔ Plan:** keine gefunden. Der Plan nennt „Version 0.5.0“
  unter WP2c, der Auftrag unter WP2a. Ich bin dem Auftrag gefolgt; README und
  `LICENSES` bleiben WP2c.

## Gate-Ausgaben (wörtlich)

1. `cargo fmt --check`: keine Ausgabe, Exitcode 0.
2. `cargo clippy --all-targets -- -D warnings`:
   ```
      Compiling diktier v0.5.0 (D:\DEV\diktier)
       Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.54s
   ```
   Keine Meldung, Exitcode 0.
3. `cargo test`:
   ```
   test result: ok. 526 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 1.99s
   ```
4. `cargo test -- --ignored stt_smoke_fixtures` (v3):
   ```
   test engine::tests::stt_smoke_fixtures ... ok
   test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 535 filtered out; finished in 17.76s
   ```
5. `$env:CARGO_TARGET_DIR="target-dev"; cargo build --release`:
   ```
      Compiling diktier v0.5.0 (D:\DEV\diktier)
       Finished `release` profile [optimized] target(s) in 20.88s
   ```
6. `scripts\release.ps1 -SkipInstaller` (baut nach `target`), Exitcode 0:
   ```
   == Diktier 0.5.0 (win-x64), TargetDir=target
   == cargo build --release --locked (CARGO_TARGET_DIR=target)
       Finished `release` profile [optimized] target(s) in 20.26s
   == Bundle D:\DEV\diktier\dist\diktier-0.5.0-win-x64
   == Modelle: parakeet-tdt-0.6b-v3-int8, parakeet-ultra-0.6b-int8-pc (Default parakeet-tdt-0.6b-v3-int8)
   == Bundle-Gate: versions.toml gegen src\models.toml
      ok parakeet-tdt-0.6b-v3-int8: huggingface revision=8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce, 4 Dateien
      ok parakeet-ultra-0.6b-int8-pc: github-release release_tag=model-parakeet-ultra-0.6b-int8-pc-r1, 3 Dateien
   == Selbstprüfung
   == Zip D:\DEV\diktier\dist\diktier-0.5.0-win-x64.zip
   == Installer übersprungen (-SkipInstaller)

   Bundle:  D:\DEV\diktier\dist\diktier-0.5.0-win-x64

   Zip:     D:\DEV\diktier\dist\diktier-0.5.0-win-x64.zip
     Größe:   8,2 MB
     SHA-256: ce796801466dc786354f03a6c8c83c6e38e54fd4b3cb4c45ca70a0229724d346
   ```

Die erzeugte `dist\diktier-0.5.0-win-x64\versions.toml`:

```toml
# Von scripts\release.ps1 erzeugt (Spec §11). Nicht von Hand pflegen.

# Default für engine.model (Spec §6.2, §8).
default_model = "parakeet-tdt-0.6b-v3-int8"

[app]
name = "diktier"
version = "0.5.0"
platform = "win-x64"
target = "x86_64-pc-windows-msvc"

[onnxruntime]
# CPU-Release von microsoft/onnxruntime, geladen über scripts\fetch-ort.ps1.
# ABI: C-API 1.28 (ort-Feature api-28), Laden per ort::init_from aus lib\.
version = "1.28.0"
abi = "api-28"
build = "onnxruntime-win-x64-1.28.0 (CPU, offizielles GitHub-Release)"
# Diese Builds setzen mindestens SSE4.2/AVX2 voraus — Haswell aufwärts (§11).
zip_sha256 = "abef733dacbe2f571547a7150b479b5cb9cc0df22f96c24983a42cadb1b4f8bc"
library_sha256 = "18370c375f07357fa5874344a9d9ac17e6b6fe1eb18b1dd209d79483b4470257"

# Je Manifestmodell ein Block; Dateien, Größen und SHA-256 aus src\models.toml
# (Spec §6.3). Wählbar über engine.model, Default steht in default_model.
[[models]]
key = "parakeet-tdt-0.6b-v3-int8"
source = "huggingface"
repository = "istupakov/parakeet-tdt-0.6b-v3-onnx"
revision = "8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce"

[[models.files]]
name = "encoder-model.int8.onnx"
bytes = 652183999
sha256 = "6139d2fa7e1b086097b277c7149725edbab89cc7c7ae64b23c741be4055aff09"

[[models.files]]
name = "decoder_joint-model.int8.onnx"
bytes = 18202004
sha256 = "eea7483ee3d1a30375daedc8ed83e3960c91b098812127a0d99d1c8977667a70"

[[models.files]]
name = "vocab.txt"
bytes = 93939
sha256 = "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d"

[[models.files]]
name = "config.json"
bytes = 97
sha256 = "666903c76b9798caf2c210afd4f6cd60b08a8dbf9800ec8d7a3bc0d2148ac466"

[[models]]
key = "parakeet-ultra-0.6b-int8-pc"
source = "github-release"
repository = "ralfkuh-lab/diktier-models"
release_tag = "model-parakeet-ultra-0.6b-int8-pc-r1"

[[models.files]]
name = "encoder-model.int8.onnx"
bytes = 700507227
sha256 = "2cc01c15a08d6976ca9ebe97739d15890f3088cfedd3a4aa4d969ba7a1702038"

[[models.files]]
name = "decoder_joint-model.int8.onnx"
bytes = 18300628
sha256 = "afcb9459250ab5c2e48e657d852501c233e8b7f5daed1894a2a7101d21165a5e"

[[models.files]]
name = "vocab.txt"
bytes = 93939
sha256 = "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d"

[crates]
# Aufgelöste Versionen aus Cargo.lock — die Pins stehen in Cargo.toml.
parakeet-rs = "0.3.7"
ort = "2.0.0-rc.13"
cpal = "0.18.2"
rubato = "0.16.2"
ureq = "3.4.0"
rustls = "0.23.43"
ring = "0.17.14"
windows-sys = ["0.52.0", "0.61.2"]
clap = "4.6.6"
serde = "1.0.229"
toml = ["0.8.23", "1.1.4+spec-1.1.0"]
toml_edit = ["0.22.27", "0.25.13+spec-1.1.0"]
sha2 = "0.10.9"
thiserror = ["1.0.69", "2.0.20"]
hound = "3.5.1"

[toolchain]
rustc = "rustc 1.95.0 (59807616e 2026-04-14)"
cargo = "cargo 1.95.0 (f2d3ce0bd 2026-03-21)"

[build_host]
os = "Microsoft Windows NT 10.0.22631.0 (Microsoft Windows 11 Enterprise)"
```

## 🔍 Offen

- **NOTICE im Bundle.** `release.ps1` kopiert `LICENSES\*` per Platzhalter.
  Deshalb liegt die `LICENSES/NOTICE-parakeet-ultra.md` des parallelen
  WP0-Agenten schon in diesem Test-Bundle unter `dist\`. In die Selbstprüfung
  als Pflichtdatei gehört sie mit WP2c. Die Datei selbst habe ich nicht
  angefasst.
- **Ultra-Laden ist nicht belegt.** Echtes Laden und Transkribieren mit Ultra
  (fehlendes `config.json`, f32-WAV, Stille und Rauschen) ist hier nicht
  geprüft, denn es gibt keine Artefakte im Produktpfad und keinen Netzzugriff.
  Das ist WP2d (lokales Ultra-Integrationsgate).
- **Kein Ladefehler-Test.** Ein Ladefehler (`ModelLoadFailed`) ist in der
  Wiring-Probe nicht eigens getestet, weil `parakeet-rs` ohne ONNX nicht
  scheitern kann. Die Probe lädt nur, wenn `check()` besteht. Der Kernpfad
  `ModelLoadFailed → error ohne Hotkey` steht unverändert in `state.rs`
  (`model_load_failure_is_fatal_error_without_hotkey`).
- **`--model` fehlt noch.** `--transcribe-list` und `--model` aus SPEC §9
  (v1.10) gehören zu WP2c, hier nicht umgesetzt.
- **Review steht aus.** Das Code-Review durch Sol ist für WP2a–d gemeinsam
  vorgesehen (Plan WP2d).
