Auftrag: WP2a aus docs/ultra-alltagstest-plan.md (v2) umsetzen: Mehrmodell-Vertrag für 0.5.0. Diktier, Rust, Windows-only. Basis ist der aktuelle Working Tree (Commit `2d10ee0` plus uncommittete Doku: SPEC v1.10, Plan, WP0-Ergebnisse). Die Doku ist Vorgabe, nicht Arbeitsgegenstand.

Lies zuerst:
1. docs/SPEC.md (v1.10): §6.2, §6.3 inkl. „Ultra-Artefakte (v1.10)“, „Unveränderliche URL“ und „Prüfumfang“, §8 `[engine]`, §11 `versions.toml`, §18 #16. Verbindlich; weicht der Plan ab, gilt die SPEC, Widersprüche in den Bericht.
2. docs/ultra-alltagstest-plan.md: Ausgangslage, Leitentscheidungen 1–3, WP2a, „Umgang mit dem Review“ (B3, W3, W8).
3. docs/reviews/plan-ultra-alltagstest-sol.md, Abschnitte B3, W3 und W8 (dort Fundstellen im Code).
4. Code: src/models.toml, src/download.rs, src/config.rs (Validierung `engine.model`, Vorlage), src/engine.rs (`ParakeetTranscriber::load`, `model_artifacts`, stt-smoke), src/daemon/mod.rs (Modellauswahl, Download, Tray-Zustände), src/daemon/workers.rs, src/single_instance.rs (Download-Sperre), src/tray.rs, scripts/release.ps1 (`versions.toml`).

Umfang:
- `models.toml` wird auf `[[models]]` mit `key` und `[[models.files]]` umgestellt. Der v3-Eintrag bleibt in allen Werten byte-gleich, URLs inklusive. Neu ist der Ultra-Eintrag nach SPEC §6.3 mit URLs `https://github.com/ralfkuh-lab/diktier-models/releases/download/model-parakeet-ultra-0.6b-int8-pc-r1/<datei>`, ohne `config.json`. Wenn es für `versions.toml` nötig ist, bekommt jedes Modell Herkunftsfelder (z. B. `source`, `revision` bzw. `release_tag`), sonst nicht.
- `load_manifest(key)`, bzw. eine Manifest-Liste plus Auswahl, mit einem sauberen Fehler bei unbekanntem Schlüssel. **Genau ein** aus der Config gewählter Eintrag wird an alle Verbraucher durchgereicht: Daemon (Verzeichnis, Download, Engine, Tray), `ParakeetTranscriber::load`, `model_artifacts(key)`. Kein Verbraucher wählt selbst ein Manifest, und es gibt keinen stillen v3-Fallback.
- config.rs: Die Validierung prüft gegen die Manifest-Schlüssel statt gegen `DEFAULT_MODEL`. Ein unbekannter Schlüssel bleibt fatal, die Meldung nennt die erlaubten Werte. `DEFAULT_MODEL` bleibt v3. Die Vorlage schreibt weiter den Default und bekommt einen Kommentar mit dem zweiten Wert (SPEC §8).
- scripts/release.ps1: Alle Manifestmodelle werden strukturiert gelesen, nicht per Regex auf den ersten Treffer. `versions.toml` bekommt `default_model` und je Modell einen Block nach SPEC §11. Nichts wird aus der Reihenfolge oder aus URL-Mustern erraten. Dazu ein Bundle-Gate im Skript, das die erzeugte TOML zurückliest und gegen `models.toml` prüft; bei Abweichung bricht das Skript ab. Den Kommentar „vier Artefakte“ modellbezogen machen.
- Tests (ohne echte große Dateien):
  - Manifest: beide Dateisätze vollständig, eindeutige Schlüssel, Verzeichnisnamen sicher (kein `..`, keine Trenner), v3-Werte einschließlich URLs unverändert, Ultra-Werte nach SPEC-Tabelle.
  - Config: beide Werte ok, fehlender Schlüssel gleich Default, unbekannt fatal mit Liste in der Meldung.
  - Downloader mit kleinen Fakes für einen Drei-Datei-Satz ohne `config.json`.
  - Daemon- und Tray-Verdrahtung parametrisiert für beide Schlüssel, soweit ohne Win32 prüfbar: Auswahl, `downloading → loading → idle`, Fehlerpfad ohne scharfen Hotkey und ohne Fallback.
  - Download-Sperre bzw. Verzeichnisse beider Modelle getrennt.
  - v3-Golden-Set-Test und `stt_smoke_fixtures` bleiben beim v3-Schlüssel.
- Version 0.5.0 in Cargo.toml (Cargo.lock zieht der Build nach). README und `LICENSES` gehören **nicht** in dieses Paket (WP2c).

Gates (Befehl und tatsächliche Summary-Zeile zitieren):
1. `cargo fmt --check`
2. `cargo clippy --all-targets -- -D warnings` ohne Meldung
3. `cargo test` grün
4. `cargo test -- --ignored stt_smoke_fixtures` grün (v3)
5. `$env:CARGO_TARGET_DIR="target-dev"; cargo build --release` grün
6. `scripts\release.ps1 -NoInstaller` (oder der passende Schalter laut Skriptkopf) läuft mit dem Bundle-Gate grün. Die erzeugte `versions.toml` steht wörtlich im Bericht. Das Skript baut nach `target`; der laufende Daemon läuft aus `%LOCALAPPDATA%\Programs\Diktier` und ist nicht betroffen.

Regeln:
- Nicht committen, nicht pushen.
- Keine Änderungen an docs/ (außer deinem Bericht), testdata/, LICENSES/, `scripts/quantize-ultra*`. Nichts in `%LOCALAPPDATA%\diktier` oder `%TEMP%\diktier` schreiben. Installierte Version und laufenden Daemon nicht anfassen. Kein Netzwerkzugriff auf die Ultra-URLs, das Release existiert noch nicht.
- Kein herdr, keine weiteren Panes.
- Rückfragen: Frage nach `D:\DEV\diktier\.herd\fragen\impl-ultra-wp2a.md`, Turn mit `RUECKFRAGE: D:\DEV\diktier\.herd\fragen\impl-ultra-wp2a.md` beenden.

Bericht nach docs/reviews/impl-ultra-wp2a-notes.md: Umgesetzt (je Verbraucher, wie das Manifest ankommt), Abweichungen und warum, Gate-Ausgaben wörtlich, Offen. TUI-Ausgabe knapp; das Ergebnis zählt nur aus Bericht und `git diff`.
