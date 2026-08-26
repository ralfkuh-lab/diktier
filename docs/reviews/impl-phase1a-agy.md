# Review: Diktier Phase 1a — STT-Engine-Infrastruktur

Reviewer: **agy**  
Datum: 2026-08-26  
Verbindliche Referenz: `docs/SPEC.md` (v1.3), `AGENTS.md`  
Geprüfter Scope: Uncommitteter Diff gegen HEAD (`.gitignore`, `Cargo.toml`, `Cargo.lock`, `src/audio.rs`, `src/download.rs`, `src/engine.rs`, `src/main.rs`, `docs/SPIKES.md`) sowie neue Dateien `scripts/fetch-ort.sh`, `testdata/stt/normalize.py`, `testdata/stt/README.md`.

---

## 1. Kurzfazit

Die Implementierung von Phase 1a ist von **hoher technischer Qualität** und setzt das Fundament der STT-Engine (`parakeet-rs`, `ort`, Audio-Pipeline, Golden-Set-Validierung) strukturiert, performant und im Wesentlichen spec-treu um.

### Positive Ergebnisse & Gate-Erfüllung
- **Inferenz & Performance (§3, §12):** Auf dem Referenzsystem (Ryzen 9 5900HX, Mint 22 Cinnamon) transkribiert das Release-Binary eine 60-s-Audioaufnahme in **~3,7 s** bei einem gemessenen Peak-RSS von **1,39 GiB** (Ziel: ≤ 2 GiB). Warm-Inferenz bei 10 s Audio liegt bei **~0,48 s** (Median, 5 Läufe; Ziel: ≤ 5 s).
- **Stille & Rauschen (§12):** Sowohl `stille.wav` als auch `rauschen.wav` erzeugen deterministisch leere Transkripte. Kein RMS-Gate erforderlich.
- **< 250-ms-Regel (§6.4):** Mit `MIN_SAMPLES_16KHZ = 4000` exakt auf Sample-Ebene ohne Engine-Aufruf implementiert und unit-getestet.
- **Exitcode-Vertrag (§9):** Sauber eingehalten (`0` bei Erfolg/Stille, `1` bei fatalen Modell-/Laufzeitfehlern, `2` bei ungültigem CLI-Aufruf oder fehlerhaften WAVs).
- **Abhängigkeiten & Pinning (§6.1):** `parakeet-rs = "=0.3.7"`, `ort = "=2.0.0-rc.13"` mit expliziten Feature-Flags (`load-dynamic`, `api-28`, `default-features = false`), ONNX Runtime 1.28.0 mit SHA-256 gepinnt.

### Wesentliche Kritikpunkte & Handlungsbedarf
1. **ORT-Auflösung scheitert im Dev-Workflow (B1):** `resolve_ort_lib()` sucht nur `exe_dir/lib` und `exe_dir/../lib`. Bei `cargo run` oder Start aus `target/debug` bzw. `target/release` schlägt die Suche ohne manuelle gesetzte Umgebungsvariable `DIKTIER_ORT_LIB` fehl. Dies verletzt das Portable-Start-Gebot aus §11.
2. **`OnceLock`-Vergiftung bei Fehler (B2):** `ensure_ort_initialized()` speichert `Err`-Zustände permanent im `OnceLock`. Ein State-Machine-Retry (§5.2/§10) nach behobenem Fehler schlägt ohne Prozessneustart fehl.
3. **`normalize.py` Bindestrich-Logik verfälscht WER (B3):** Das ersatzlose Löschen von `-` erzeugt Kunstwörter (`rustdaemon`), während Parakeet getrennt ausgibt (`Rust Demon`), was die gemessene WER bei Fachwörtern künstlich auf 35,3 % aufbläst.
4. **Windows-Hygiene & Smoke-Test (B4, B5):** `scripts/fetch-ort.sh` ist rein Linux-spezifisch; im Smoke-Test ist `libonnxruntime.so` hartcodiert; der Smoke-Test prüft nur 1 von 5 Pflicht-Dateien.

Es gibt **keine Blocker**, die den Architekturansatz infrage stellen, jedoch drei **Hoch**- und vier **Mittel**-Befunde, die vor dem Übergang zu Phase 2 behoben werden sollten.

---

## 2. Befunde

### B1 — `resolve_ort_lib()` scheitert im regulären Cargo-Entwicklungs-Workflow (`target/debug` und `target/release`)

- **Schwere:** Hoch
- **Stelle:** `src/engine.rs:152-160`
- **Problem:**
  `resolve_ort_lib()` sucht die native ONNX-Runtime-Bibliothek über folgende Kandidaten:
  ```rust
  let candidates = [
      exe_dir.join("lib").join(name),
      exe_dir.join("..").join("lib").join(name),
  ];
  ```
  Nach Ausführung von `scripts/fetch-ort.sh` liegt die Bibliothek unter `<repo>/lib/libonnxruntime.so`.
  Wird die Anwendung regulär per `cargo run -- --transcribe-wav ...` oder direkt als `./target/debug/diktier` bzw. `./target/release/diktier` ausgeführt, ist `exe_dir` gleich `<repo>/target/debug` bzw. `<repo>/target/release`.
  Die Kandidatensuche prüft nur:
  1. `<repo>/target/debug/lib/libonnxruntime.so` (bzw. `target/release/lib/...`)
  2. `<repo>/target/lib/libonnxruntime.so`
  Beide Pfade existieren nicht. Jeder reguläre Aufruf nach dem Fetch-Skript scheitert mit `EngineError::Ort`, es sei denn, man setzt manuell die Umgebungsvariable `DIKTIER_ORT_LIB`.
  Dies widerspricht Spec §11:
  - *„Laden ausschließlich über ort::init_from relativ zu current_exe().“*
  - *„Release-Gate: (...) ORT-Umgebungsvariablen entfernen, STT laden.“*
  - *„Portable Start aus Ordner muss gehen. cargo build --release ist der Dev-Weg.“*
- **Vorschlag:**
  Kandidatenliste um den relativen Repo-Root-Pfad erweitern:
  ```rust
  let candidates = [
      exe_dir.join("lib").join(name),              // Bundle: <bundle>/lib/
      exe_dir.join("..").join("lib").join(name),      // Prefix: bin/ -> ../lib/
      exe_dir.join("../..").join("lib").join(name),  // Cargo: target/debug/ -> ../../lib/
  ];
  ```
  Damit funktioniert der Start im Release-Bundle, in System-Präfixen und bei Cargo-Builds transparent ohne Umgebungsvariablen.

---

### B2 — `ensure_ort_initialized()` friert Fehlerzustand (`Err`) in `OnceLock` dauerhaft ein

- **Schwere:** Hoch
- **Stelle:** `src/engine.rs:184-196`
- **Problem:**
  `ensure_ort_initialized()` speichert das Ergebnis der Initialisierung in einer `OnceLock`:
  ```rust
  static INIT: OnceLock<Result<PathBuf, String>> = OnceLock::new();
  match INIT.get_or_init(|| {
      let path = resolve_ort_lib().map_err(|e| e.to_string())?;
      let _ = ort::init_from(&path)
          .map_err(|e| e.to_string())?
          .with_telemetry(false)
          .commit();
      Ok(path)
  }) {
      Ok(_) => Ok(()),
      Err(msg) => Err(EngineError::Ort(msg.clone())),
  }
  ```
  Schlägt der erste Aufruf fehl (z. B. weil die Bibliothek beim Start noch fehlt, der Download noch läuft oder ein Pfad temporär falsch war), wird das `Err(msg)` permanent in `INIT` gespeichert.
  Spec §5.2 und §10 definieren ein klares Recovery- und Retry-Verhalten:
  - `error + Retry/Neustart → starting`
  - *„Download/ORT/Modell fatal → Neustart oder expliziter Retry“*
  Bei einem späteren Retry im laufenden Prozess (z. B. durch Tray-Interaktion oder nach Abschluss des Downloads) wird die Initialisierungsclosure nie wieder aufgerufen. `ensure_ort_initialized()` liefert dauerhaft denselben Fehler zurück.
- **Vorschlag:**
  Die `OnceLock` darf nur im Erfolgsfall (`Ok`) belegt werden:
  ```rust
  static INIT: OnceLock<PathBuf> = OnceLock::new();
  if INIT.get().is_some() {
      return Ok(());
  }
  static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
  let _guard = LOCK.lock().unwrap();
  if INIT.get().is_some() {
      return Ok(());
  }
  let path = resolve_ort_lib().map_err(|e| EngineError::Ort(e.to_string()))?;
  let _ = ort::init_from(&path)
      .map_err(|e| EngineError::Ort(e.to_string()))?
      .with_telemetry(false)
      .commit();
  let _ = INIT.set(path);
  Ok(())
  ```

---

### B3 — `normalize.py` löscht Bindestriche ersatzlos und verfälscht WER bei Komposita

- **Schwere:** Hoch
- **Stelle:** `testdata/stt/normalize.py:11-18`, `testdata/stt/fachwoerter.ref.txt:1`
- **Problem:**
  In `normalize.py` ist die Interpunktion definiert als:
  ```python
  _PUNCT = str.maketrans("", "", ".,!?;:-–—\"'")
  ```
  `str.translate(_PUNCT)` löscht Bindestriche `-` ersatzlos.
  In `testdata/stt/fachwoerter.ref.txt` stehen die normgerechten deutschen Schreibweisen:
  `Der Rust-Daemon lädt das ONNX-Modell über die Runtime, danach schreibt der Worker-Thread das Transkript in die Zwischenablage.`
  Durch das ersatzlose Löschen entstehen die Token:
  `rustdaemon`, `onnxmodell`, `workerthread`.
  Parakeet transkribiert gesprochene Sprache hingegen als getrennte Wörter:
  `Rust Demon`, `ONNX Modell`, `Worker Thread`.
  Beim Wort-Levenshtein-Vergleich (`wer()`) zählt jedes dieser 3 Komposita als 1 Substitution + 1 Insertion (Distanz 2 statt 0 bzw. 1). Dadurch schnellt die errechnete WER auf **35,29 %** (6 Fehler bei 17 Referenzwörtern), obwohl die Erkennung inhaltlich exakt ist.
  Spec §12 fordert: *„Referenztexte nutzen dieselbe Zahlenschreibweise wie Parakeet (Ziffern vs. Wort)“* mit dem Ziel einer unverzerrten Evaluation gegenüber Voxtype.
  Gleiches gilt für `testdata/stt/zahlen_umlaute.ref.txt`: `Am dreiundzwanzigsten März` (Ordinalwort) vs. Parakeet-Ausgabe `Am dreiundzwanzig. März` (Kardinalzahl mit Punkt), was nach Punkt-Entfernung zu `dreiundzwanzig` wird.
- **Vorschlag:**
  Entweder in `normalize.py` Bindestriche und Gedankenstriche vor der Satzzeichenbereinigung in Leerzeichen umwandeln (`text = text.replace("-", " ").replace("–", " ").replace("—", " ")`), oder in den `.ref.txt`-Dateien die Worttrennung an Parakeets Tokenisierung anpassen (`Rust Daemon`, `ONNX Modell`, `Worker Thread`).

---

### B4 — Fehlende Windows-Unterstützung in `scripts/fetch-ort.sh` und hartcodierte Dateiendung im Test

- **Schwere:** Mittel
- **Stelle:** `scripts/fetch-ort.sh:1-39`, `src/engine.rs:260-261`
- **Problem:**
  1. `scripts/fetch-ort.sh` lädt fest das Linux-Archiv `onnxruntime-linux-x64-1.28.0.tgz` und extrahiert `libonnxruntime.so`. Für Windows (`onnxruntime-win-x64-1.28.0.zip` → `lib/onnxruntime.dll`) gibt es weder ein Skript (z. B. `scripts/fetch-ort.ps1`) noch einen dokumentierten Abruf.
  2. Im Smoke-Test `stt_smoke_silence_is_empty` (`src/engine.rs:260-261`) ist der Dateiname hardcodiert:
     ```rust
     let repo_lib = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
         .join("lib")
         .join("libonnxruntime.so");
     ```
     Unter Windows schlägt dieser Pfad fehl, weil dort `onnxruntime.dll` erwartet wird. Die bereits vorhandene Hilfsfunktion `ort_lib_filename()` wurde hier nicht verwendet.
- **Vorschlag:**
  1. In `src/engine.rs:261` `.join(ort_lib_filename())` verwenden.
  2. Ein äquivalentes PowerShell-Skript `scripts/fetch-ort.ps1` mit SHA-256-Pinning für das Windows-x64-Zip bereitstellen.

---

### B5 — `stt-smoke`-Test vernachlässigt Pflicht-Gates aus Spec §12 & §13

- **Schwere:** Mittel
- **Stelle:** `src/engine.rs:250-288`
- **Problem:**
  Spec §13 verlangt:
  - *„stt-smoke mit echtem Modell: #[ignore] im normalen cargo test, Pflicht in Phase 1 auf beiden OS.“*
  - Spec §12 verlangt: *„Stille, Raumrauschen, < 250 ms → leer“* und die Evaluation der drei Sprach-WAVs.
  Die einzige Testfunktion `stt_smoke_silence_is_empty` prüft jedoch ausschließlich `stille.wav`.
  Es fehlen Smoke-Tests für:
  - `rauschen.wav` (Verifikation, dass Raumrauschen leer bleibt)
  - `alltag.wav`, `fachwoerter.wav`, `zahlen_umlaute.wav` (Verifikation, dass Transkription nicht-leer und plausibel ist)
  - Puffer `< 250 ms` gegen das echte Modell.
- **Vorschlag:**
  Einen strukturierten Testlauf über alle 5 Testdateien in `testdata/stt/` aufbauen, der Stille/Rauschen auf Leere und die Sprach-WAVs auf erfolgreiche Erkennung verifiziert.

---

### B6 — Robustheitsmängel und unvollständige Selbsttests in `normalize.py`

- **Schwere:** Mittel
- **Stelle:** `testdata/stt/normalize.py:40-82`
- **Problem:**
  1. **Absturz bei `--help`:** `python3 normalize.py --help` fängt den Schalter nicht ab, versucht die Datei `--help` zu lesen und stürzt mit `FileNotFoundError` ab.
  2. **Absturz bei Stdin für WER:** `python3 normalize.py WER ref.txt -` stürzt mit `FileNotFoundError: [Errno 2] No such file or directory: '-'` ab.
  3. **Fehlende Fehlerbehandlung:** Ungültige oder nicht lesbare Pfade werfen rohe Python-Tracebacks statt sauberer CLI-Fehlermeldungen.
  4. **Typografische Satzzeichen fehlen:** `_PUNCT` enthält nur ASCII `"` und `'`, ignoriert aber deutsche typografische Anführungszeichen (`„`, `“`, `”`, `‚`, `‘`, `’`, `»`, `«`).
  5. **`selftest()` unvollständig:** Getestet werden nur `,` und `!`. Die Satzzeichen `.`, `?`, `;`, `:`, `-`, `–`, `"`, `'` sind ungetestet. Bei `wer()` fehlen Tests für Insertionen, Deletionen und leere Strings (`wer("", "")`, `wer("a", "")`).
- **Vorschlag:**
  CLI-Handling robust gestalten (Argument-Parsing mit `--help`, Stdin-Support via `-`), deutsche Unicode-Satzzeichen in `_PUNCT` ergänzen und `selftest()` um alle Randfälle erweitern.

---

### B7 — Testlücken in Audio- und CLI-Validierung (§6.4, §9, §13)

- **Schwere:** Mittel
- **Stelle:** `src/audio.rs:96-128`, `src/engine.rs:207-248`, `src/main.rs:197-238`
- **Problem:**
  1. In `src/audio.rs` gibt es nur 2 Tests (`stub_capture_returns_empty_16khz`, `wav_rejects_wrong_rate`). Es fehlen Unit-Tests für:
     - Ablehnung von Stereo-Dateien (`channels != 1`)
     - Ablehnung unzulässiger Bit-Tiefen (z. B. 24-Bit Int, 8-Bit Int, 64-Bit Float)
     - Positivtest für 16-Bit Integer PCM (Prüfung der f32-Skalierung im Bereich `[-1.0, 1.0]`)
     - Positivtest für 32-Bit Float PCM.
  2. In `src/engine.rs` fehlen Unit-Tests für `resolve_ort_lib()` (Verhalten mit gesetzter, fehlender und ungültiger Umgebungsvariable) sowie für Modell-Mismatch in `ParakeetTranscriber::load`.
  3. In `src/main.rs` fehlen Tests für `--transcribe-wav` mit nicht-existierender Datei (Exitcode 2) und formatwidrigen WAVs (Exitcode 2).
- **Vorschlag:**
  Die genannten Negativ- und Positivtests in die jeweiligen Testmodule einfügen.

---

### B8 — Unsichere Modifikation von Umgebungsvariablen in Tests

- **Schwere:** Niedrig
- **Stelle:** `src/engine.rs:264`
- **Problem:**
  In `stt_smoke_silence_is_empty` wird `unsafe { std::env::set_var("DIKTIER_ORT_LIB", &repo_lib) }` verwendet. In multithreaded Testumgebungen ist das Mutieren globaler Prozess-Umgebungsvariablen unsicher und führt potenziell zu Data-Races mit parallel laufenden Tests.
- **Vorschlag:**
  Nach Behebung von Befund B1 (Repo-Root-Auflösung in `resolve_ort_lib`) entfällt `set_var` im Test ersatzlos.

---

### B9 — Fehlende Sanity-Prüfung auf `NaN` / `Inf` bei Float-WAVs

- **Schwere:** Niedrig
- **Stelle:** `src/audio.rs:88-92`
- **Problem:**
  Beim Lesen von 32-Bit Float-WAVs werden Samples direkt übernommen. Nicht-finite Werte (`NaN`, `+Inf`, `-Inf`) werden ungeprüft an die ONNX Runtime durchgereicht und können dort zu unkontrolliertem Verhalten im Decoder führen.
- **Vorschlag:**
  Bei Float-WAVs vor der Rückgabe `v.is_finite()` prüfen und bei ungültigen Werten mit `AudioError::Wav("Ungültiges Float-Sample (NaN/Inf)")` ablehnen.

---

### B10 — Hugging-Face-URLs in `models.toml` nachtragen (Vorbereitung Phase 3)

- **Schwere:** Niedrig
- **Stelle:** `src/models.toml:11, 17, 23, 29`
- **Problem:**
  Die URLs in `src/models.toml` sind aktuell noch leere Platzhalter `url = ""`. In `docs/SPIKES.md` wurde die Herkunfts-Revision nun verbindlich dokumentiert (`istupakov/parakeet-tdt-0.6b-v3-onnx` Revision `8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce`).
- **Vorschlag:**
  Die vier immutable URLs (`https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce/<dateiname>`) in `models.toml` eintragen, bevor Phase 3 begonnen wird.

---

## 3. Fazit und Freigabe-Empfehlung

Die Umsetzung der Phase 1a erfüllt die funktionalen und qualitativen Kernanforderungen der Spec v1.3. Die Inferenz- und Speichermessungen liegen deutlich innerhalb der vorgegebenen Budgets.

**Empfohlene nächste Schritte vor Start von Phase 1b / Phase 2:**
1. Behebung von **B1** (Suche in `../../lib`), **B2** (`OnceLock`-Retry-Sicherheit) und **B3** (`normalize.py` Bindestrich-Logik).
2. Vervollständigung der Unit- und Smoke-Tests (**B5**, **B7**).
3. Bereitstellung des Windows-Download-Pfads (**B4**).
4. Ausführung des Voxtype-Vergleichslaufs auf Omarchy zur Schließung der offenen WER-Spalten in `docs/SPIKES.md`.
