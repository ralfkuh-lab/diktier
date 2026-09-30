Auftrag: WP2e, Nacharbeit aus dem Code-Review docs/reviews/impl-ultra-wp2-sol.md (GPT-6.1 Sol) für den Ultra-Alltagstest. Diktier, Rust, Windows-only. Basis ist der aktuelle Working Tree (`2d10ee0` plus uncommittete Pakete WP0–WP2c, Version 0.5.0).

Lies zuerst:
1. docs/reviews/impl-ultra-wp2-sol.md, **vollständig**: alle Befunde K1, W1–W9, L1–L3 und der Abschnitt „Relevante Grenzen der vorhandenen Tests“.
2. docs/ultra-alltagstest-plan.md in der aktuellen Fassung. Neu (v2.1) und verbindlich sind „Bewertungsprotokoll“ (Urteil für **beide** Seiten mit Kategorien, „gleicher Fall wie“, „Ich“-Regel, verbindlicher Lauf), „Abnahmekriterien“ 2 und 3 (neu gefasst) und „Datenschutz“ („Urteile speichern“).
3. docs/SPEC.md v1.10 §9 (neu: `--manifest-sha256`; modusabhängige Optionen außerhalb ihres Modus sind Exit 2) und §10.
4. Die Berichte docs/reviews/impl-ultra-wp0-notes.md, impl-ultra-wp2a/b/c-notes.md, damit du die Struktur kennst.

Umzusetzen, alle Befunde. Wo Sol Alternativen nennt, gilt die Entscheidung hier:
- **K1** `scripts/quantize-ultra.py`: Vor jedem Seiteneffekt die aufgelösten Pfade von Quelle, `--out`, `.quantize-tmp` und Staging gegen Überlappung in **beide** Richtungen prüfen, sonst Exit 2. Cleanup nur für Verzeichnisse, die dieser Lauf selbst angelegt hat. Ein vorgefundenes `.quantize-tmp` wird nicht ungeprüft gelöscht, sondern führt zu Exit 2 mit Hinweis. Dazu Negativtests für die Szenarien aus K1, auch einen Fehler vor dem Anlegen des Arbeitsverzeichnisses, ohne echte Quantisierung (z. B. mit kleinen Dummy-Dateien und abgeschaltetem Hash-Sollwert nur im Test, oder durch Aufruf der Prüffunktion).
- **W1:** Die Bereinigung von `last_recording.wav` läuft nur im Default-Verzeichnis `<TEMP>\diktier`, nie in einem über `DIKTIER_DEBUG_WAV_DIR` gewählten. Test wie in W1 beschrieben.
- **W2/L3** nach Plan „Urteile speichern“:
  - Die Seite verlangt beim ersten Speichern per `showSaveFilePicker` eine Datei (vorgeschlagener Name `urteile.json`). Danach schreibt sie bei jeder Änderung über das Handle dorthin.
  - Ohne File-System-Access-API gibt es eine sichtbare Fehlermeldung, keinen Download-Ersatz.
  - localStorage enthält nur Urteilscodes, keine Notizen, keine Texte.
  - Speicherfehler sind sichtbar, beschädigter gespeicherter Stand ebenso (kein stilles Ersetzen).
  - `-Resolve`, `-Judgments`, `-Evaluation` und alle Schreibziele der Skripte liegen nur unter `<Wurzel>\diktier\ultra-test\auswertung\`, sonst Abbruch vor dem ersten Schreiben. `<Wurzel>` ist `%LOCALAPPDATA%` oder `-ModelRoot`.
- **W3:** Ein gescheiterter Warmup wird für die betroffene Datei maschinenlesbar als `error` ausgewiesen, mit gleichbleibender Zeilenzahl. Beide Skripte prüfen Exitcode und Status konsistent, bench zählt diese Fehler. Dazu ein gemeinsamer Regressionstest.
- **W4:** Beide Startzeilen-Formate erkennen (`(Daemon` und `(--foreground`). Sitzungen ohne Startzeile, etwa durch Rotation, als mehrdeutig ausweisen statt sie zu verschmelzen. Die Zeitgrenzen gemäß W4 behandeln (WAV-Zeit gegen Gate-Zeit). Tests wie in W4.
- **W5:** Das Urteilsschema folgt dem neuen Protokoll:
  - je Paar ein Urteil
  - je Seite (A/B) die Kategorien K1–K4 als Mehrfachauswahl, dazu das Flag K5 und bei K1 „gravierend“
  - das optionale Feld „gleicher Fall wie P…“

  `-Resolve` rechnet Kriterium 2 genau nach Plan (exklusiv = Kategorie auf der Ultra-Seite, nicht auf der v3-Seite desselben Paars; Wiederholungen zählen einmal; v3-Kategorien aus Paaren **und** Stichprobe; Veto = exklusives gravierendes K1 auf der Ultra-Seite). Kriterium 1 (U/V) ergibt sich aus dem Urteil, K5 wie bisher. Tests: gemeinsamer Fehler, gemeinsamer Stichprobenfehler, Wiederholung, echtes exklusives Veto.
- **W6:** Ein zusätzlicher verdeckter Audio-Block für Aufnahmen, bei denen **genau ein** Modell mit dem Wort „Ich“ beginnt, mit der Frage „Ich gesprochen ja/nein“. Kriterium 3 bewertet „Herr Präsident“ und „Ich“ getrennt.
- **W7:** `-Prepare` hat einen verbindlichen Modus mit Pflichtangaben `-From`/`-To`, der Start und Ende festhält. Daneben gibt es `-Explorativ` ohne Urteil. `-Resolve` gibt nur im verbindlichen Modus und nach Ablauf (Ende ≤ jetzt, Dauer 7 Tage oder 11 mit `-Verlaengert`) Kriterien-Urteile aus, sonst „explorativ, kein Urteil“. WAVs ohne zuordenbaren Zeitstempel sind Inventurproblem und zählen nicht zur Grundgesamtheit. Tests wie in W7.
- **W8:** Ein Urteil zu Kriterium 4 gibt es nur mit dem festen Protokoll (3 Durchgänge × 3 Runs, wechselnde Reihenfolge). Geprüft und protokolliert werden: `--version` des Binaries = 0.5.0, SHA-256 der gebündelten `lib\onnxruntime.dll` gegen `versions.toml` im selben Ordner (falls vorhanden, sonst „nicht belegt“), Threads = Default. Jeder Prozess braucht einen Peak-Wert, sonst „nicht belegt“. Kurzproben bleiben möglich, heißen aber „Funktionsprobe, kein Urteil“.
- **W9** + SPEC §9: neuer CLI-Modus `--manifest-sha256` (SHA-256 der eingebetteten `models.toml`-Bytes, kein Modell-Laden, keine Config-Anlage). `release.ps1` vergleicht ihn nach dem Build bzw. bei `-SkipBuild` mit dem SHA-256 von `src\models.toml`. Bei Abweichung Abbruch. Mit Test.
- **L1:** Modusabhängige Optionen (`--model`, `--runs`) werden vor jeder Aktion validiert, auch vor Autostart. Parser-Tests ohne echte Autostart-Aktion.
- **L2:** Die Prüfungen in release.ps1 werden case-sensitive (`-ceq`, `-cmatch`, `switch -CaseSensitive`) und bekommen die Gegenstücke zur Rust-Prüfung (`COMPLETE`/`.part`). Dazu Negativfixtures.
- **Blindheit** (Sol, Tabelle „A/B“): Audio in der Vergleichsseite nur über neutrale Aliasse im Auswertungsordner (z. B. `audio\P001.wav`, als Hardlink, sonst Kopie), damit Zeitstempel und Laufnummer nicht sichtbar sind. Die Zuordnung Alias → Original steht nur in `schluessel.json`.
- **Tests der Skripte:** eine dauerhaft ausführbare Suite `scripts\tests\Test-UltraScripts.ps1` (PowerShell 7, ohne Pester-Pflicht, Exit ≠ 0 bei Fehler). Sie deckt die Skriptbefunde oben mit kleinen Fakes ab: ein Fake-`diktier` über `-Exe`, etwa eine `.cmd`, die feste JSONL und Exitcodes liefert, keine echten Modelle. Die Suite schreibt nur unter einem Temp-Verzeichnis, das sie selbst anlegt und löscht.

Gates (Befehl und tatsächliche Summary-Zeile zitieren):
1. `cargo fmt --check`
2. `cargo clippy --all-targets -- -D warnings` ohne Meldung
3. `cargo test` grün
4. `pwsh -File scripts\tests\Test-UltraScripts.ps1` grün
5. Negativtests von `quantize-ultra.py` grün (Befehl und Ausgabe)
6. `$env:CARGO_TARGET_DIR="target-dev"; cargo build --release`, dann `target-dev\release\diktier.exe --manifest-sha256` gleich dem SHA-256 von `src\models.toml`
7. `scripts\release.ps1 -SkipInstaller` grün, mit dem neuen Manifest-Abgleich
8. Ein echter Durchlauf von `compare-models.ps1 -Prepare -Explorativ` mit `-ModelRoot .herd\wp2c-root` über 6–10 WAVs, dazu `-Resolve` mit einem simulierten `urteile.json` im neuen Schema, beides grün. Die Vergleichsseite headless prüfen, wie in WP2c.

Regeln:
- Nicht committen.
- Keine Änderungen an docs/ (außer deinem Bericht), testdata/, LICENSES/, src/models.toml.
- Nichts in `%LOCALAPPDATA%\diktier` oder `%TEMP%\diktier` schreiben, nur unter `.herd\` oder in selbst angelegten Temp-Verzeichnissen. Installierte Version und laufenden Daemon nicht anfassen, kein Netz auf Ultra-URLs.
- **Große Dateien** (Modelle) nie komplett in den Speicher laden oder per Array bearbeiten. Kürzen oder Beschädigen für Tests nur per Kopie und `FileStream.SetLength`. Ralf arbeitet parallel am Rechner.
- Kein herdr, keine weiteren Panes.
- Rückfragen: Frage nach `D:\DEV\diktier\.herd\fragen\impl-ultra-wp2e.md`, Turn mit `RUECKFRAGE: D:\DEV\diktier\.herd\fragen\impl-ultra-wp2e.md` beenden.

Bericht nach docs/reviews/impl-ultra-wp2e-notes.md: je Befund umgesetzt/abweichend mit Begründung, Gate-Ausgaben wörtlich (ohne Transkripttexte aus `testdata\stt\local`), Offen. TUI-Ausgabe knapp.
