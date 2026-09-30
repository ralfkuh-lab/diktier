Auftrag: WP2c aus docs/ultra-alltagstest-plan.md (v2) umsetzen: Vergleichswerkzeug, Leistungsmessung, Doku. Diktier, Rust, Windows-only. Basis ist der aktuelle Working Tree (`2d10ee0` plus uncommittete Pakete WP0, WP1 und WP2a, Version 0.5.0). Diese sind Vorgabe, nicht Arbeitsgegenstand.

Lies zuerst:
1. docs/SPEC.md (v1.10): §9 „Entwickler-Modi“ (`--transcribe-list`, `--model`, JSONL mit `run`, Exitcodes), §10 (Log-Vertrag, Debug-WAV inklusive v1.10), §6.2/§6.3. Verbindlich.
2. docs/ultra-alltagstest-plan.md: Leitentscheidungen 5–7, „Bewertungsprotokoll“, „Abnahmekriterien“, „Datenschutz“, WP2c, WP5, „Umgang mit dem Review“ (B1, B2, W2, W7).
3. docs/reviews/plan-ultra-alltagstest-sol.md: B1, B2, W2, W7.
4. docs/reviews/impl-ultra-wp2a-notes.md (wie das Manifest jetzt gewählt wird: `SelectedModel`, `engine::model_artifacts(key)`).
5. Code: src/main.rs (Argumentparser, `transcribe_wav`, `record_test`), src/engine.rs (`transcribe_pcm`, Gate-Report), src/audio/mod.rs (`read_wav_16k_mono`), scripts/release.ps1 (Selbstprüfung der Bundle-Dateien), README.md, LICENSES/.

Umfang:
- **CLI nach SPEC §9:**
  - `--transcribe-list <liste>`: UTF-8, eine WAV je Zeile, Leerzeilen ignorieren.
  - `--model <schlüssel>` für `--transcribe-wav` und `--transcribe-list`. Nur Manifest-Schlüssel, die Config auf Platte bleibt unverändert, ein unbekannter Schlüssel ergibt Exit 2.
  - Das Modell wird einmal geladen, und nur, wenn mindestens eine Datei den Gate passiert. Ein Warmup, ungezählt.
  - JSONL auf stdout, je Datei eine Zeile `{"file","status","text","infer_ms","samples"}`, mit `--runs n` n Zeilen mit `run`. Der Status ist `text`, `rejected` oder `error`; bei `rejected` und `error` ist `text` leer. Für JSON-Escaping einen vorhandenen Serializer nehmen, nicht selbst bauen.
  - Diagnose (Gate-Report, Fehlerursache) steht textfrei auf stderr, mit Dateibezug.
  - Exitcode 1, sobald eine Datei `error` hat. Die übrigen Dateien werden trotzdem verarbeitet.
  - `--transcribe-wav <datei>` ohne `--model` und ohne `--runs` bleibt byte-gleich zum bisherigen Verhalten.
  - Tests: Parser (Kombinationen und Konflikte, `--model` ohne Transkriptionsmodus → Exit 2), Liste mit Leerzeilen, JSONL-Escaping (Anführungszeichen, Tab, Zeilenumbruch, Umlaute), Zustände `rejected`/`error`/`text` mit Stub-Engine, Fehler mitten im Batch → Exit 1 mit vollständiger Ausgabe, `--runs`, kein Modell-Laden bei nur abgelehnten Dateien.
- **`scripts/compare-models.ps1`**, Ausgabe fest unter `%LOCALAPPDATA%\diktier\ultra-test\auswertung\<zeitstempel>\`, nie ins Repo:
  - `-Prepare`:
    - Inventur: WAVs im Ring-Verzeichnis (Parameter, Default aus SPEC §10 bzw. dem Testverzeichnis `%LOCALAPPDATA%\diktier\ultra-test\wav`) gegen die Läufe laut `diktier.log` bzw. `diktier.log.1` im Zeitraum. Das Ergebnis ist eine Zahl: erwartet, vorhanden, fehlend, mit Laufnummern.
    - `diktier.exe --transcribe-list` für beide Modelle. Exitcodes prüfen, nur vollständige Paare mit `text` bei beiden Modellen weiterverarbeiten, Ausschlüsse mit Grund zählen.
    - Paare mit Textunterschied: A/B-Zuordnung zufällig. Der Seed kommt protokolliert in eine getrennte Schlüsseldatei.
    - Eine selbstenthaltene lokale HTML-Seite ohne Netz. Je Paar: A- und B-Text mit Wortdiff, Audio-Player auf die WAV (file-URL), Urteil (A / B / gleich / unklar), bei A/B die Kategorie K1–K5 des Fehlers der schlechteren Seite und „gravierend“ (ja/nein), optional eine Notiz. Zwischenstand per localStorage, Export per Knopf als `urteile.json`.
    - Ein zweiter Block: eine zufällige Stichprobe von 20 Aufnahmen mit gleicher Ausgabe, mit Audio und Frage „stimmt / Fehler (Kategorie)“.
    - Ein dritter Block: alle Texte, die bei einem der Modelle mit „Herr Präsident“ beginnen, mit Audio und Frage „gesprochen ja/nein“.
    - Texte in HTML sicher maskiert.
  - `-Resolve <auswertungsordner> -Judgments <urteile.json>`: löst auf und rechnet die Kennzahlen nach „Abnahmekriterien“ 1–3 und 6 (U, V, Quote mit 95-%-Wilson-Intervall ohne K5, Kategorien je Modell, Ultra-exklusive Fälle je Kategorie, Veto-Fälle, Halluzinationen je Modell, Stichprobe gleicher Ausgaben, Mindestmengen erfüllt ja/nein).
    - Zwei Ausgaben: ein lokaler Detailbericht **mit** Texten im Auswertungsordner und eine `zusammenfassung.md` **ohne** Transkripte, Pfade oder Notizen, nur Zahlen, als Vorlage für das Repo.
  - Kein Text aus dem Vergleich auf stderr, in Logs oder außerhalb des Auswertungsordners.
- **`scripts/bench-models.ps1`** für Kriterium 4:
  - gleiche WAV-Liste, dasselbe `diktier.exe`
  - je Modell ein eigener Prozess mit `--transcribe-list --runs 3`
  - Reihenfolge der Modelle wechselnd über drei Durchgänge
  - Peak Working Set je Prozess (`PeakWorkingSet64` nach Ende)
  - Ergebnis: Median und p95 von `infer_ms` je Sekunde Audio (Samples/16000) je Modell, Fehler je Modell, Peak Working Set
  - Prüft vorher, dass kein `diktier.exe`-Daemon läuft, und bricht sonst mit Hinweis ab (beendet ihn nicht selbst). Ausgabe unter demselben Auswertungsordner.
- **Modellwurzel für Tests:** Die CLI findet Modelle unter `%LOCALAPPDATA%\diktier\models\<key>`. Für Tests und das spätere Integrationsgate genügt es, `LOCALAPPDATA` nur für den Kindprozess auf ein Temp-Verzeichnis zu setzen. Das sollen beide Skripte als Parameter `-ModelRoot` unterstützen, ohne Produktänderung. Belege mit einem echten Lauf:
  - Temp-Wurzel unter `.herd\wp2c-root\` (lokal, git-ausgeschlossen).
  - Das installierte v3-Verzeichnis als Kopie, nur lesen.
  - Ultra als Kopie aus `.herd\model-release\model-parakeet-ultra-0.6b-int8-pc-r1\`, ohne NOTICE und SHA256SUMS, mit einer `COMPLETE`, wie sie der Downloader schreibt; wie genau, siehe download.rs.
  - `-Prepare` über 6–10 WAVs aus `testdata\stt\` und `testdata\stt\local\herr_praesident\`.
  - Ein simuliertes `urteile.json` für `-Resolve` und ein kurzer `bench-models.ps1`-Lauf.
  - Die HTML-Seite öffnet sich im Browser nicht automatisch. Beschreib, wie du sie geprüft hast (z. B. HTML-Validierung und Stichprobe der Einbettung).
- **README:** Abschnitt „Alltagstest (Ultra)“. Inhalt:
  - Umschalten per `engine.model`
  - die drei Umgebungsvariablen aus Leitentscheidung 5 und dass der Daemon sie beim Start liest
  - Rückweg auf v3
  - wo Aufnahmen und Auswertung liegen, mit Datenschutzhinweis
  - Aufruf von `compare-models.ps1` und `bench-models.ps1`
  - Lizenznennung für Ultra mit Verweis auf `LICENSES/NOTICE-parakeet-ultra.md`

  Den Debug-WAV-Abschnitt ergänzt ein anderes Paket (WP2b), dort nichts ändern.
- **scripts/release.ps1:** `LICENSES\NOTICE-parakeet-ultra.md` als Pflichtdatei in die Bundle-Selbstprüfung aufnehmen. Die v3-Notice bleibt Pflicht.

Gates (Befehl und tatsächliche Summary-Zeile zitieren):
1. `cargo fmt --check`
2. `cargo clippy --all-targets -- -D warnings` ohne Meldung
3. `cargo test` grün
4. `$env:CARGO_TARGET_DIR="target-dev"; cargo build --release` grün, und der Skriptbeleg oben läuft mit diesem Binary
5. `target-dev\release\diktier.exe --transcribe-wav testdata\stt\alltag.wav` gibt byte-gleich dasselbe aus wie `%LOCALAPPDATA%\Programs\Diktier\diktier.exe --transcribe-wav testdata\stt\alltag.wav` (stdout)
6. `scripts\release.ps1 -SkipInstaller` grün, inklusive Selbstprüfung mit der Ultra-NOTICE

Regeln:
- Nicht committen.
- Parallel arbeitet ein anderer Agent (WP2b) an src/daemon/debug_wav.rs, src/daemon/workers.rs, src/daemon/mod.rs und dem README-Abschnitt Debug-WAV. Diese nicht anfassen.
- Keine Änderungen an docs/ (außer deinem Bericht), testdata/, src/models.toml, src/download.rs, src/config.rs, bestehenden LICENSES-Dateien und `scripts/quantize-ultra*`. Nichts in `%LOCALAPPDATA%\diktier` oder `%TEMP%\diktier` schreiben, außer über eine `-ModelRoot`-Temp-Wurzel unter `.herd\`. Installierte Version und laufenden Daemon nicht anfassen (bench-models also nur mit `-ModelRoot`, und wenn der Daemon läuft, wie verlangt abbrechen und das so berichten). Kein Netzzugriff auf die Ultra-URLs.
- Kein herdr, keine weiteren Panes.
- Rückfragen: Frage nach `D:\DEV\diktier\.herd\fragen\impl-ultra-wp2c.md`, Turn mit `RUECKFRAGE: D:\DEV\diktier\.herd\fragen\impl-ultra-wp2c.md` beenden.

Bericht nach docs/reviews/impl-ultra-wp2c-notes.md: Umgesetzt, Abweichungen und warum, Gate-Ausgaben wörtlich (ohne Transkripttexte aus `testdata\stt\local`), Offen. TUI-Ausgabe knapp.
