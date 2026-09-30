# WP2e — Nacharbeit zum Code-Review WP2 (Sol): Umsetzungsbericht

Stand: 2026-09-30. Auftrag: [impl-ultra-wp2e-prompt.md](impl-ultra-wp2e-prompt.md).
Grundlage: [impl-ultra-wp2-sol.md](impl-ultra-wp2-sol.md) (K1, W1–W9, L1–L3,
„Relevante Grenzen der vorhandenen Tests“),
[../ultra-alltagstest-plan.md](../ultra-alltagstest-plan.md) v2.1 und SPEC v1.10
§9/§10. Basis war der Working Tree auf `2d10ee0` mit WP0–WP2d (0.5.0).

Nichts committet. Geändert habe ich nur `src/`, `scripts/` und `README.md`. An
`docs/` (außer diesem Bericht), `testdata/`, `LICENSES/` und `src/models.toml`
habe ich nichts geändert. Nichts wurde nach `%LOCALAPPDATA%\diktier` oder
`%TEMP%\diktier` geschrieben. Die installierte Version und der laufende Daemon
blieben unberührt, Netz auf Ultra-URLs gab es keins. Dieser Bericht enthält
keine Transkripttexte.

## Übersicht

| Befund | Stand | Wo |
|---|---|---|
| K1 | ✅ umgesetzt | `scripts/quantize-ultra.py`, neu `scripts/tests/test_quantize_ultra.py` |
| W1 | ✅ umgesetzt | `src/daemon/debug_wav.rs`, `src/daemon/workers.rs` |
| W2/L3 | ✅ umgesetzt | `compare-models.html`, `ultra-test-lib.ps1`, `compare-models.ps1`, `bench-models.ps1` |
| W3 | ✅ umgesetzt | `src/transcribe_list.rs`, `ultra-test-lib.ps1` (`Assert-ExitMatchesRows`), beide Skripte |
| W4 | ✅ umgesetzt | `ultra-test-lib.ps1` (`Read-DaemonSessions`, `Get-Inventory`) |
| W5 | ✅ umgesetzt | `compare-models.html`, `compare-models.ps1` (Schema v2, Kriterium 2) |
| W6 | ✅ umgesetzt | `compare-models.ps1`, `compare-models.html` (Block 4 „Ich“) |
| W7 | ✅ umgesetzt | `compare-models.ps1` (`-Explorativ`, `-Verlaengert`, `Get-NonBindingReason`) |
| W8 | ✅ umgesetzt | `bench-models.ps1`, `ultra-test-lib.ps1` (`Get-Kriterium4`) |
| W9 | ✅ umgesetzt | `src/main.rs`, `src/download.rs`, `scripts/release.ps1` |
| L1 | ✅ umgesetzt | `src/main.rs` (`check_mode_options`) |
| L2 | ⚠️ teilweise | `scripts/release.ps1` (case-sensitive, COMPLETE/.part); die echte TOML-Gegenprüfung fehlt, siehe unten |
| Blindheit | ✅ umgesetzt | `compare-models.ps1` (`audio\<ID>.wav`) |
| Skript-Tests | ✅ umgesetzt | neu `scripts/tests/Test-UltraScripts.ps1` (33 Tests) |

## ✅ K1 — Quellverzeichnis beim Quantisieren

- Neu ist `check_paths(source, out)`. Es läuft vor jedem Seiteneffekt, also vor
  `log_environment`, dem Download und `mkdir`, und prüft die aufgelösten Pfade
  von Quelle, `--out`, `<out>\.quantize-tmp` und `<out>\.quantize-tmp\src`.
  - Überlappung in beide Richtungen, also gleich, darunter oder darüber.
    Verglichen wird mit `os.path.normcase`, unter Windows also ohne Groß- und
    Kleinschreibung.
  - Ist eine Bedingung verletzt, endet das Skript mit Exit 2
    (`FEHLER (Aufruf): …`).
- Ein vorgefundenes `.quantize-tmp` führt zu Exit 2 mit Hinweis. Es wird nicht
  gelöscht, denn es kann Analyse-Reste enthalten.
- Beim Cleanup gibt es die Flags `created_work` und `created_staged`. `finally`
  löscht nur, was dieser Lauf selbst angelegt hat. Das Staging legt jetzt
  `main()` an, `stage_sources` nicht mehr.
- Tests: `scripts/tests/test_quantize_ultra.py` mit 11 Tests, nur
  Standardbibliothek und Dummy-Dateien. `SOURCE`, `OUTPUT`, `log_environment`,
  `check_external_data` und `quantize` werden nur im Test ersetzt. Abgedeckt:
  - alle Überlappungsrichtungen und Groß-/Kleinschreibung
  - Sols Beispiel `--source <out>\.quantize-tmp\src --out <out>`
  - Quelle gleich `--out`, `--out` in der Quelle
  - vorhandenes `.quantize-tmp` bleibt samt Inhalt
  - **Fehler vor dem Anlegen des Arbeitsverzeichnisses**: `log_environment`
    wirft `ImportError`. Es wird nichts angelegt und nichts gelöscht.
  - falsche Quelldatei: nur das eigene Staging wird entfernt
  - Erfolg; abweichende Ausgabe bleibt zur Analyse, ohne Quellkopien
- Den Test legt `sys.dont_write_bytecode` fest, sonst entstünde
  `scripts\__pycache__`. Den Ordner aus dem ersten Lauf habe ich gelöscht.

## ✅ W1 — `last_recording.wav` nur im Default-Verzeichnis

- `DebugWavConfig` hat das neue Feld `legacy_cleanup`. `resolve` setzt es nur,
  wenn das Verzeichnis der Default ist, also bei fehlendem oder ungültigem
  `DIKTIER_DEBUG_WAV_DIR`. Ein ausdrücklich gewähltes Verzeichnis bekommt
  `false`, auch wenn es zufällig gleich dem Default ist. Gezählt wird die
  Herkunft, nicht der Pfad.
- `write_recording(&DebugWavConfig, …)` löscht die Altlast nur bei
  `legacy_cleanup`. Der Worker übergibt die Config.
- Test `a_chosen_dir_keeps_a_foreign_last_recording`:
  - Das Verzeichnis kommt aus `_DIR`, mit Kapazität 5000.
  - Darin liegen eine fremde `last_recording.wav` und drei fremde `.part`-Dateien
    (jung und über eine Stunde alt). Nach dem Dump sind alle noch da.
  - Gegenprobe: Ist dasselbe Verzeichnis der Default, wird die Altlast entfernt,
    die fremden `.part` bleiben.
- Die bisherigen Ring-Tests laufen über einen Test-Helfer mit
  `legacy_cleanup: true` wie bisher.

## ✅ W2/L3 — Urteile speichern und Auswertungswurzel

**Vergleichsseite** (`compare-models.html`):

- Beim ersten Urteil ruft die Seite `showSaveFilePicker` auf, vorgeschlagen ist
  `urteile.json`. Danach schreibt sie nach jeder Änderung über das Handle, mit
  400 ms Entprellung und hintereinander. Die Anzeige lautet
  „gespeichert in urteile.json um …“.
- Knöpfe:
  - „Urteilsdatei anlegen …“
  - „Urteilsdatei öffnen …“ zum Fortsetzen: prüft Auswertungs-ID und Format 2,
    sonst Fehler, und übernimmt den Stand.
  - „Zwischenstand löschen“: löst auch die Dateibindung, damit kein leerer Stand
    über die alte Datei geschrieben wird.
- Ohne File System Access API erscheint ein roter Fehler, die Dateiknöpfe sind
  gesperrt. Einen Download-Ersatz gibt es nicht mehr: `createObjectURL` und
  `a[download]` sind entfernt.
- localStorage (Format 2) enthält nur Urteilscodes: Urteil, Kategorien, K5,
  gravierend, „gleicher Fall wie“ (eine Paar-ID), Stichprobe und
  Gesprochen-Antworten. **Keine Notizen und keine Texte.**
- Sichtbare Fehler:
  - Schreiben in die Datei gescheitert: rot, Anzeige „NICHT gespeichert“
  - localStorage voll oder gesperrt: Warnung
  - beschädigter oder alter gespeicherter Stand: rot, und er wird **nicht**
    überschrieben, bis er ausdrücklich verworfen wird
  - Schließen mit ausstehendem Schreibvorgang: `beforeunload`

**Skripte:**

- Neu in `ultra-test-lib.ps1`:
  - `Get-EvaluationRoot`, also `<Wurzel>\diktier\ultra-test\auswertung`
  - `Test-UnderEvaluationRoot` bzw. `Assert-UnderEvaluationRoot`: voller Pfad,
    echt unterhalb der Wurzel, und kein Bestandteil zwischen Wurzel und Ziel
    (die Wurzel eingeschlossen) ist ein Reparse-Point
  - `Set-WriteRoot`: Alle Schreibhelfer (`Write-Utf8*`, `New-WriteDir`,
    `Invoke-DiktierList`, Audio-Aliasse) prüfen ihr Ziel selbst gegen diesen
    Ordner.
- Vor dem ersten Schreiben geprüft werden `-Resolve`, `-Judgments`,
  `-Evaluation` und `-List`. `<Wurzel>` ist `%LOCALAPPDATA%` oder `-ModelRoot`;
  mit `-ModelRoot` ist das die ausdrückliche Testwurzel.

## ✅ W3 — gescheiterter Warmup

- `run_batch`: Scheitert der Warmup, ist die Datei `error` mit voller
  Zeilenzahl (ohne `--runs` eine Zeile, mit `--runs n` n Zeilen mit `run`). Sie
  wird nicht gemessen, der Exit ist 1. Die nächste freigegebene Datei wärmt neu
  auf, danach geht es normal weiter. Damit gilt: Exit 1 ⇔ mindestens eine
  `error`-Zeile.
- Rust-Test `failed_warmup_marks_file_as_error`, der alte
  `failed_warmup_exits_1` ist ersetzt. Erster Engine-Aufruf Fehler, alle
  folgenden erfolgreich, mit und ohne `--runs 3`. Geprüft werden
  `error / rejected / text`, `infer_ms` null, `samples` gesetzt und `run` 1…n.
- `Assert-ExitMatchesRows` in der Bibliothek wird von beiden Skripten genutzt.
  Exit 1 ohne `error`-Zeile oder Exit 0 mit `error`-Zeile bricht ab.
  `bench-models.ps1` zählt die Warmup-Fehler als `error`-Zeilen und damit
  gegebenenfalls als Ultra-exklusiv.
- Gemeinsamer Regressionstest in der Suite (Fake-`diktier`, erste Ultra-Datei
  `error`, Rest `text`, Exit 1):
  - Die Vorbereitung weist „Fehler nur Ultra 1“ aus.
  - Der Benchmark zählt 9 Fehlerzeilen (3 × 3), 1 Ultra-exklusive Datei, und die
    Prüfung „keine Ultra-exklusiven Fehler“ ist `false`.
  - Der alte Widerspruch (Exit 1, alles `text`) bricht beide Skripte ab.

## ✅ W4 — Sitzungen

- `Read-DaemonSessions` erkennt beide echten Startzeilen: `startet (Daemon, …`
  und `startet (--foreground, …`.
- Zeilen vor der ersten Startzeile (Rotation) bilden eine eigene Sitzung „ohne
  Startzeile“. Sie wird nie mit einer anderen verschmolzen.
- Die Zuordnung von WAV zu Lauf ist ereignisbezogen:
  - Zuerst über die Dump-Zeile `DIKTIER_DEBUG_WAV: …\<name>` der Sitzung.
  - Sonst über die Zeit (letzte Startzeile ≤ WAV-Zeit) plus Laufnummer.
  - In der Sitzung ohne Startzeile und vor der ersten Startzeile gibt es keine
    Zeitzuordnung. Unbelegte Läufe und solche WAVs zählen dort als
    **mehrdeutig**, nicht als fehlend.
- Zeitgrenzen:
  - Eine WAV zählt nach ihrem Namenszeitstempel, also dem Aufnahmeende. Deshalb
    werden auch WAVs außerhalb des Zeitraums ihren Läufen zugeordnet: Ein Lauf,
    dessen Aufnahme knapp vor `From` endete und dessen Gate-Zeile danach steht,
    zählt nicht und ist auch nicht „fehlend“.
  - Ein Lauf ohne WAV zählt nach der Gate-Zeit. Liegt sie höchstens 2 min
    hinter `From` oder `To`, ist er „grenznah“ und wird eigens ausgewiesen.
- Tests:
  - zwei `--foreground`-Neustarts mit Laufnummern je ab 1: 6 erwartet, 5
    vorhanden, der fehlende gehört zur richtigen Sitzung
  - Daemon/Foreground gemischt, mit Dump-Zuordnung trotz verstellter Uhr
  - abgeschnittener Loganfang
  - WAV-Zeit gegen Gate-Zeit an beiden Grenzen, mit „grenznah“

## ✅ W5 — Urteilsschema und Kriterium 2

- Schema v2 (`urteile.json` und `schluessel.json` im Format 2), je Paar:

  ```json
  {"urteil": "A|B|gleich|unklar",
   "seiten": {"A": {"kategorien": ["K1","K3"], "k5": false, "gravierend": true},
              "B": {"kategorien": [], "k5": true}},
   "gleicher_fall_wie": "P003", "notiz": "…"}
  ```

  `gravierend` steht genau dann da, wenn K1 markiert ist. Für die Stichprobe
  gilt: `{"urteil": "stimmt"}` oder `{"urteil": "fehler", "kategorien": [...]}`.
- Kriterium 1:
  - U/V kommen aus dem Urteil. Ein Sieg zählt nur, wenn die schlechtere Seite
    mindestens eine Kategorie K1–K4 hat.
  - Hat sie nur K5, zählt das Paar als gleich (`K5_als_gleich`), wie bisher.
  - A/B ohne Kategorie und ohne K5 auf der schlechteren Seite ist unvollständig,
    dann wird nicht aufgelöst.
- Kriterium 2 genau nach Plan:
  - Ultra-exklusiv heißt: Die Kategorie steht auf der Ultra-Seite und nicht auf
    der v3-Seite desselben Paars.
  - „Gleicher Fall wie“-Ketten werden auf ihren Anfang zurückgeführt, ein Kreis
    ist ein Fehler. Je Kategorie zählt jede Fallgruppe einmal.
  - v3 „hat“ eine Kategorie, wenn sie auf einer v3-Paarseite **oder** in der
    Stichprobe vorkommt.
  - Veto ist ein Ultra-exklusives K1, das auf der Ultra-Seite als gravierend
    markiert ist, je Fallgruppe.
  - Nur als Zahl ausgewiesen, ohne Wirkung auf das Urteil: gemeinsames K1 mit
    gravierender Ultra-Seite.
- `zusammenfassung.md` hat eine Kategorien-Tabelle je Seite, Stichprobe und
  exklusive Fälle. Neu ist `ergebnis.json` mit nur Zahlen, als
  maschinenlesbarer Beleg für die Suite.
- Tests:
  - gemeinsamer Fehler: kein Veto, nicht exklusiv
  - gemeinsamer Stichprobenfehler, dazu die Gegenprobe über eine gemeinsame
    Paarseite
  - Wiederholung: drei Fälle ergeben eine Klasse; als eine Fallgruppe markiert
    entsteht keine
  - echtes exklusives Veto
  - K5 als gleich
  - Schemafehler (Sieger ohne Fehler, gravierend ohne K1, Selbstbezug, Kreis)

## ✅ W6 — „Ich“ vor einem Befehl

- Die Vorbereitung erkennt „beginnt mit dem Wort ‚Ich‘“ per
  `^\s*ich(?![\p{L}\p{N}])`, ohne Groß-/Kleinschreibung. In den neuen
  verdeckten Block 4 kommen nur Aufnahmen, bei denen **genau ein** Modell so
  beginnt (XOR). Der Block zeigt nur Audio und fragt „‚Ich‘ gesprochen: ja/nein“.
- Kriterium 3 wird für „Herr Präsident“ und „Ich“ getrennt bewertet
  (`kriterium_3_teile`). Jeder Teil gilt als „nicht erfüllt“, wenn Ultra
  häufiger halluziniert als v3. Kriterium 3 ist erfüllt, wenn beide Teile
  erfüllt sind. Ein gesprochenes „Ich“ zählt nicht.
- Test: je ein Ultra-Fall. Nicht gesprochen ergibt beide Teile „nicht erfüllt“,
  gesprochen ergibt „erfüllt“.

## ✅ W7 — verbindlich und explorativ

- `-Prepare -From -To [-Verlaengert]` ist verbindlich; `-From` und `-To` sind
  dort Pflicht (Parametersatz). Die Vorbereitung bricht vor jedem Schreiben ab,
  wenn:
  - der Zeitraum verkehrt ist
  - die Dauer nicht genau 7 bzw. 11 Tage (`-Verlaengert`) beträgt, gemessen in
    Wanduhrzeit, damit eine Zeitumstellung im Zeitraum nicht stört
  - das Ende in der Zukunft liegt
- `-Prepare -Explorativ` geht jederzeit, der Zeitraum ist optional.
- `vorbereitung.json` hält `modus`, `zeitraum.von/bis` (UTC), `tage` und
  `verlaengert` fest.
- `-Resolve` gibt Kriterien-Urteile (1, 2, 3, 6) nur aus, wenn alle folgenden
  Bedingungen gelten. Sonst steht bei jedem Kriterium „explorativ, kein Urteil“,
  mit Grund in `zusammenfassung.md`, `ergebnis.json` und auf der Konsole:
  - Modus verbindlich
  - Dauer 7 bzw. 11 Tage
  - Ende ≤ jetzt
  - Vorbereitung erstellt nach Ende
- WAVs ohne auswertbaren Zeitstempel, auch mit unmöglichem Datum, sind ein
  Inventurproblem, gezählt als `wavs_ohne_zeitstempel`. Sie kommen nicht in die
  Liste, in beiden Modi.
- Tests:
  - laufender, kurzer und verkehrter Zeitraum
  - verbindlich ohne `-To`
  - explorativ ohne Urteil
  - drei nachträgliche Manipulationen von `vorbereitung.json`: läuft noch,
    3 Tage, vor Ablauf erstellt
  - fremde Namen außerhalb des Zeitraums und ohne Zeitstempel

## ✅ W8 — Kriterium 4 nur mit festem Protokoll

- `Get-Kriterium4` (Bibliothek, reine Funktion) prüft in dieser Reihenfolge:
  1. Ohne 3 × 3 oder mit `-OhneDaemonPruefung`: „Funktionsprobe, kein Urteil“.
  2. Fehlt ein Beleg: „nicht belegt (…)“ mit Ursache.
  3. Ohne gepaarte Datei: „nicht belegt“.
  4. Sonst die vier Prüfungen.
- Provenienz, geprüft und in `bench.json`/`bench.md` protokolliert:
  - `--version` gleich `diktier 0.5.0`
  - SHA-256 von `lib\onnxruntime.dll` neben der Exe gegen
    `[onnxruntime] library_sha256` aus `versions.toml` im selben Ordner. Ohne
    `versions.toml` gilt „nicht belegt“.
  - `engine.threads` in `<Wurzel>\diktier\config.toml` ist 0. Ohne Datei gilt der
    Default 0.
- Hat ein Prozess keinen Peak (`GetProcessMemoryInfo` scheitert oder liefert 0),
  steht in `fehlende_belege` „Peak Working Set fehlt: Durchgang n Modell“, und
  das Urteil lautet „nicht belegt“.
- Neu ist der Schalter `-OhneDaemonPruefung` für Tests und Funktionsproben. Er
  ergibt nie ein Urteil.
- Tests: Die Urteilslogik läuft als Unit-Test über `Get-Kriterium4` (fehlender
  Peak, Provenienz, Kurzprobe, ohne Daemonprüfung). Die Provenienz läuft
  zusätzlich Ende zu Ende mit Fake: Version 0.4.1, fehlende `versions.toml`,
  `threads = 4`, und die Gegenprobe ist sauber.

## ✅ W9 — `--manifest-sha256`

- Das CLI gibt mit `--manifest-sha256` den SHA-256 der eingebetteten
  `models.toml`-Bytes aus, klein und hex (`download::manifest_sha256`).
  - Clap `exclusive`, jede weitere Option ergibt Exit 2.
  - Es lädt kein Modell und legt keine Config an.
  - Scheitert das Schreiben nach stdout, gibt es Exit 1 statt eines Panics.
- `release.ps1` vergleicht nach dem Build, auch bei `-SkipBuild`, mit
  `Get-FileHash src\models.toml` (`-ceq`). Zusätzlich muss `--version` gleich
  `diktier <Cargo-Version>` sein. Bei Abweichung bricht es vor dem Bundle ab.
- **Befund unterwegs:** `& diktier.exe --manifest-sha256` liefert in PowerShell
  nichts. `diktier.exe` ist ein Windows-Subsystem-Programm, PowerShell wartet
  nicht darauf, die Pipe schließt, und das Binary panickte mit „failed printing
  to stdout“. Deshalb ruft `release.ps1` es über `ProcessStartInfo` mit
  Umleitung und `WaitForExit` auf (`Invoke-ExeLines`). Mein Fake als `.cmd`
  hatte das zunächst verdeckt; der echte Build-Lauf (Gate 6/7) hat es gezeigt.
- Tests:
  - Rust: `manifest_sha256_matches_source_file` (Digest gleich der Quelldatei)
    und `manifest_sha256_is_exclusive_and_needs_nothing`
  - Suite: Abgleich mit Fake, mit anderem Manifest, Großbuchstaben und alter
    Version
  - Negativprobe echt: eine Kopie des installierten 0.4.1-Binaries mit
    `-SkipBuild`, siehe Gate 7

## ✅ L1 — modusabhängige Optionen vor jeder Aktion

- `check_mode_options(&cli)` ist der erste Schritt nach dem Parsen, noch vor
  `--manifest-sha256`, Autostart und allen Modi. `--runs` oder `--model` ohne
  Transkriptionsmodus ergeben Exit 2, ebenso ein unbekannter Schlüssel.
- Test `mode_options_are_checked_before_autostart`, nur geparst, nie
  `cli_main`: Der echte Autostart bleibt unberührt. Für beide Autostart-Schalter
  mit `--model nope`, v3, Ultra und `--runs 3` ist das Ergebnis `Err(2)`; die
  Schalter allein ergeben `Ok`.

## ⚠️ L2 — release.ps1 strikter

- Umgesetzt:
  - Im TOML-Leser und in der Manifestprüfung ist alles case-sensitive: `-ceq`,
    `-cne`, `-cmatch`, `-cnotcontains`, `switch -CaseSensitive -Exact`, Tabellen
    als `OrderedDictionary` mit `StringComparer.Ordinal`.
  - Die Muster enden auf `\z` statt `$`.
  - `Test-SafeComponent` entspricht `is_safe_component`, auch für die beiden
    Teile von `repository`.
  - Dateinamen `COMPLETE` und `*.part` werden abgelehnt, wie in `validate_model`.
- Negativfixtures in der Suite: aus dem echten `src\models.toml` im Temp
  abgeleitet.
  - `Default_Model`, `KEY`
  - `source = "GitHub-Release"`, `"Huggingface"`
  - großgeschriebener URL-Host, Revision und SHA
  - `COMPLETE`, `vocab.txt.part`
  - Das echte Manifest besteht.
- **Abweichung:** Eine echte TOML-Gegenprüfung (Sol: „statt ausschließlich
  Rücklesen mit demselben Teilmengenparser“) habe ich nicht gebaut, der Auftrag
  nennt sie nicht. Teilweise deckt W9 sie ab: Das Binary bestätigt, dass es genau
  die Bytes von `src\models.toml` eingebettet hat, und `cargo test` parst dieselbe
  Datei mit dem echten `toml`-Parser. `versions.toml` liest weiterhin nur der
  Teilmengenparser zurück.
- Damit die Suite die Funktionen laden kann, gibt es in `release.ps1` einen
  Dot-Source-Schutz (`if ($MyInvocation.InvocationName -eq ".") { return }`)
  hinter den Funktionen.

## ✅ Blindheit

- Audio-Aliasse in `auswertung\<stamp>\audio\<ID>.wav` (P…, S…, H…, I…).
  Zuerst wird ein Hardlink versucht, sonst eine Kopie.
- Die Seite verlinkt nur `audio/<ID>.wav`, relativ; `file:///`-Pfade sind raus.
- Nur `schluessel.json` enthält die Zuordnung, als `audio: {"P001.wav": {datei,
  art}}`.
- Suite und Gate 8 prüfen:
  - Die Seite enthält keine `rec_`, `lauf-`, Modellschlüssel, `file:///` und
    keinen Seed.
  - Jeder Alias ist bytegleich zum Original.

## ✅ Regressionssuite `scripts\tests\Test-UltraScripts.ps1`

- Voraussetzungen: PowerShell 7, kein Pester. Exit 1 bei einem Fehler.
- Fake-`diktier`: `diktier.cmd` ruft `fake-diktier.ps1` auf und liefert JSONL
  und Exitcodes laut `scenario.json`. Dazu kommen `--version`,
  `--manifest-sha256`, eine Dummy-`lib\onnxruntime.dll` und `versions.toml`.
- Alles liegt unter `%TEMP%\diktier-ultra-tests-<zufall>`, das die Suite
  anlegt und am Ende löscht (`-KeepTemp` zur Analyse). Jeder Skriptaufruf läuft
  mit `-ModelRoot` auf der Testwurzel.
- Browser-Teil mit headless Chrome oder Edge, falls vorhanden, sonst
  übersprungen (`-SkipBrowser`). Er nutzt die echte `vergleich.html` mit
  eingeschobenen Stubs und Bedienung und prüft:
  - Picker und fortlaufendes Schreiben
  - im Browser-Speicher nur Codes, ohne Notiz und ohne Transkript
  - ohne API: Fehler, kein Download
  - Schreibfehler sichtbar
  - beschädigter Stand sichtbar und nicht ersetzt
  - voller Speicher sichtbar
- **Mutationsprobe** in einer Scratchpad-Kopie mit vier wieder eingebauten
  Fehlern:
  - Startzeile nur `(Daemon`
  - Stichprobe zählt nicht für v3
  - keine Exit/Status-Prüfung im Benchmark
  - `COMPLETE` erlaubt

  Ergebnis: `Test-UltraScripts: 28 Tests, 23 ok, 5 fehlgeschlagen`. Es
  schlugen genau die zugehörigen Tests fehl: beide W4-Starttests, der
  Stichprobentest, der W3-Widerspruchstest und die L2-Fixtures. Das Original
  ist unverändert.

## Weitere Änderungen

- README, Abschnitt „Alltagstest (Ultra)“:
  - Urteilsdatei statt localStorage/Export
  - verbindlich/explorativ
  - Audio-Aliasse
  - Protokoll von Kriterium 4
  - Aufruf der Suite
- `release.ps1`: Der Kopfkommentar zu `-SkipBuild` nennt den Manifest-Abgleich.
  Die Datei bleibt bei BOM und CRLF.

## Gate-Ausgaben (wörtlich)

`cargo test` lief mit `TEMP`/`TMP` auf ein selbst angelegtes
`.herd\wp2e-tmp` (danach gelöscht), wie bei Sol, damit `tempfile` nicht ins
produktive `%TEMP%` schreibt.

1. `cargo fmt --check`: keine Ausgabe, `fmt exit=0`.
2. `cargo clippy --all-targets -- -D warnings`:
   ```
       Checking diktier v0.5.0 (D:\DEV\diktier)
       Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.34s
   clippy exit=0
   ```
3. `cargo test`:
   ```
   test result: ok. 554 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 2.00s
   cargo test exit=0
   ```
4. `pwsh -File scripts\tests\Test-UltraScripts.ps1`, mit normalem `%TEMP%`. Die
   Suite nutzt nur ihr eigenes Unterverzeichnis; danach waren 0 Reste
   `diktier-ultra-tests-*` vorhanden:
   ```
   Test-UltraScripts: 33 Tests, 33 ok, 0 fehlgeschlagen
   suite exit=0
   ```
5. `python scripts\tests\test_quantize_ultra.py` (Python 3.12.12):
   ```
   Ran 11 tests in 0.204s

   OK
   python exit=0
   ```
6. `$env:CARGO_TARGET_DIR="target-dev"; cargo build --release`, dann
   `target-dev\release\diktier.exe --manifest-sha256` über `ProcessStartInfo`:
   ```
      Compiling diktier v0.5.0 (D:\DEV\diktier)
       Finished `release` profile [optimized] target(s) in 14.11s
   build exit=0
   manifest exit=0
   binary: 9f88fb6d6e471b78191e6fc3d84e67e90b07c9ba06135ecfc8dff38fa4693069
   quelle: 9f88fb6d6e471b78191e6fc3d84e67e90b07c9ba06135ecfc8dff38fa4693069
   gleich: True
   version: diktier 0.5.0 (exit 0)
   ```
7. `scripts\release.ps1 -SkipInstaller`:
   ```
   == Diktier 0.5.0 (win-x64), TargetDir=target
   == cargo build --release --locked (CARGO_TARGET_DIR=target)
       Finished `release` profile [optimized] target(s) in 14.86s
   == Manifest im Binary = src\models.toml (SHA-256 9f88fb6d6e471b78191e6fc3d84e67e90b07c9ba06135ecfc8dff38fa4693069), --version 0.5.0
   == Bundle D:\DEV\diktier\dist\diktier-0.5.0-win-x64
   == Modelle: parakeet-tdt-0.6b-v3-int8, parakeet-ultra-0.6b-int8-pc (Default parakeet-tdt-0.6b-v3-int8)
   == Bundle-Gate: versions.toml gegen src\models.toml
      ok parakeet-tdt-0.6b-v3-int8: huggingface revision=8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce, 4 Dateien
      ok parakeet-ultra-0.6b-int8-pc: github-release release_tag=model-parakeet-ultra-0.6b-int8-pc-r1, 3 Dateien
   == Selbstprüfung
   == Zip D:\DEV\diktier\dist\diktier-0.5.0-win-x64.zip
   == Installer übersprungen (-SkipInstaller)
   ...
   release exit=0
   ```
   Negativprobe W9: Eine Kopie des installierten Binaries (ProductVersion
   0.4.1) lag unter `.herd\wp2e-skipbuild\release\` (danach gelöscht). Aufruf mit
   `-SkipBuild -SkipInstaller -TargetDir .herd\wp2e-skipbuild`:
   ```
        | release.ps1: D:\DEV\diktier\.herd\wp2e-skipbuild\release\diktier.exe --manifest-sha256 endete mit 2 (Binary vor
        | 0.5.0?)
   release exit=1
   zip unverändert: True
   ```
8. Echter Lauf mit `target-dev\release\diktier.exe` und
   `-ModelRoot .herd\wp2c-root`. Das Probe-Skript liegt im Scratchpad und
   schreibt nur unter `.herd\`.
   - Eingaben unter `.herd\wp2e-probe\`:
     - 8 WAVs als `rec_…_lauf-N.wav`: 5 aus `testdata\stt\`, 1 aus `local\`,
       2 aus `local\herr_praesident\`
     - `umbenannt.wav` ohne Zeitstempel
     - synthetisches `diktier.log` mit einer Daemon- und einer
       `--foreground`-Sitzung, Läufe je ab 1, Lauf 5 der zweiten Sitzung ohne WAV
   - Ablauf: Vorbereitung, statische Seitenprüfung, headless Chrome mit
     Datei-Stub (alle Urteile per Klick, eine Notiz; die zuletzt geschriebene
     Datei ist das simulierte `urteile.json` im Schema v2), dann `-Resolve`.
   ```
   ### compare-models.ps1 -Prepare -Explorativ
   == Auswertungsordner D:\DEV\diktier\.herd\wp2c-root\diktier\ultra-test\auswertung\20260930-203012 (explorativ)
   == Inventur (diktier.log, 2 Sitzungen, davon ohne Startzeile 0)
      erwartet 9, vorhanden 8, fehlend 1 (davon grenznah 0), mehrdeutige Läufe 0
      fehlt: Sitzung 2026-09-30T07:00:00Z (--foreground), Lauf 5
      WAVs ohne Logeintrag 0, mehrdeutig 0, mit Namenssuffix 0, außerhalb des Zeitraums 0
      WAVs ohne auswertbaren Zeitstempel (Inventurproblem, nicht gezählt) 1
   == parakeet-tdt-0.6b-v3-int8 über 8 Dateien
      Exitcode 0
   == parakeet-ultra-0.6b-int8-pc über 8 Dateien
      Exitcode 0
   == Paare: vollständig mit Sprache 6 (gleich 3, verschieden 3)
      ausgeschlossen: Gate-Ablehnung (beide) 2
   == Seite D:\DEV\diktier\.herd\wp2c-root\diktier\ultra-test\auswertung\20260930-203012\vergleich.html
      3 Paare, 3 Stichprobe, 0 »Herr Präsident«, 0 »Ich«
   prepare exit=0
   ### Seitenprüfung statisch
      externe src/href: 0
      http(s)://: 0
      file:///: 0
      rec_ / lauf-: 0
      Modellschlüssel: 0
      Seed: 0
      < > & im JSON-Block: 0
      Aliasse bytegleich zum Original: 6/6 (hardlink 6)
      Audio-URLs neutral (audio/<ID>.wav): 6/6
      A/B-Texte bitgleich zu roh-<Modell>.jsonl laut schluessel.json: 3/3
   ### Seite headless (Chrome), Bedienung mit Datei-Stub
      Chrome exit 0; Fehler bei der Bedienung: 0
      Karten 6, Audio-Elemente 6, Fortschritt: Paare 3/3 · Stichprobe 3/3 · Herr Präsident 0/0 · Ich 0/0
      Speicheranzeige: gespeichert in urteile.json um 20:30:24; Meldungen: ''
      Schreibvorgänge in die Urteilsdatei: 1
      Browser-Speicher mit Notiz: False, mit Transkript: False
      letzte Datei: format 2, vollstaendig True, Paare 3, Stichprobe 3, HP 0, Ich 0
   ### compare-models.ps1 -Resolve
      explorativ, kein Urteil: Vorbereitung ist explorativ
      U 1, V 2, Quote 33.3 %, Mindestmengen nicht erfüllt, neue Klassen keine, Veto 0
      Halluzinationen »Herr Präsident« v3 0 / Ultra 0, »Ich« v3 0 / Ultra 0
      Kriterium 1: explorativ, kein Urteil · 2: explorativ, kein Urteil · 3: explorativ, kein Urteil · 6: explorativ, kein Urteil
   resolve exit=0
      ergebnis.json: verbindlich False, Grund 'Vorbereitung ist explorativ', U 1, V 2, K4 Ultra-Seiten 2, v3-Seiten 1, K1 v3-Seiten 1
      zusammenfassung.md: Transkripte 0, Notiz 0, .wav 0, rec_ 0
      bericht.md (lokal): Notiz enthalten 1
   ```
   Die Pfadzeilen von `-Resolve` sind gekürzt, sonst ist die Ausgabe wörtlich.
   U/V spiegeln nur die simulierten Klicks (immer „A besser“), keine
   Qualitätsaussage. Nur ein Schreibvorgang, weil alle Klicks noch während der
   (gestubbten) Dateiauswahl fielen; das fortlaufende Schreiben belegt die Suite
   (≥ 2 Schreibvorgänge).

## Abweichungen und Entscheidungen

- **W3:** Nach einem gescheiterten Warmup wärmt die nächste freigegebene Datei
  erneut auf, statt kalt zu messen. Die betroffene Datei selbst wird nicht
  gemessen.
- **W4:** Die 2-min-Toleranz für „grenznah“ habe ich gesetzt. Sie ist
  Information, kein Ausschluss.
- **W7:**
  - Die verbindliche Vorbereitung weist laufende oder falsch lange Zeiträume
    schon selbst ab. `-Resolve` prüft zusätzlich „Vorbereitung erstellt nach
    Ende“: Eine vor Ablauf gezogene Inventur wäre unvollständig.
  - Dass die Verlängerung nur bei fehlenden Mindestmengen zulässig ist (F2),
    prüft das Skript nicht. `-Verlaengert` erlaubt einfach 11 Tage; die
    Begründung bleibt Ralfs Protokollpflicht.
- **W8:**
  - `diktier 0.5.0` ist im Skript fest verdrahtet (`$ExpectedVersion`), für
    eine spätere Version also anzupassen.
  - Threads liest das Skript aus der Config der benutzten Wurzel, ohne
    Umgebungsvariablen.
  - `-OhneDaemonPruefung` ist neu und nötig, damit Tests neben dem laufenden
    Daemon messen können. Die Option ergibt nie ein Urteil.
- **W2:**
  - Die Seite kann den Zielordner des Pickers nicht vorgeben (API-Grenze). Sie
    nennt ihn, und `-Resolve` lehnt eine Urteilsdatei außerhalb der
    Auswertungswurzel ab.
  - `-Judgments` muss unter der Wurzel liegen, nicht zwingend im selben
    Auswertungsordner.
- **W5:** Das Format ist jetzt 2. Auswertungsordner aus WP2c (Format 1, z. B.
  `.herd\wp2c-root\…\20260930-190500`) lehnt `-Resolve` mit dem Hinweis „neu
  vorbereiten“ ab.
- **Blindheit:** Hardlink und Kopie behalten die Dateizeit des Originals. In der
  Seite ist sie nicht zu sehen, in den Dateieigenschaften des Alias schon.
- **L2:** keine echte TOML-Gegenprüfung, siehe oben.

## 🔍 Offen

- **SPEC-Nachtrag nötig (docs/ war gesperrt):**
  - §10: „Die frühere `last_recording.wav` wird entfernt“ gilt jetzt nur im
    Default-Verzeichnis (W1).
  - §9: Ein gescheiterter Warmup macht die Datei zu `error` mit voller
    Zeilenzahl (W3).
  - §9 eventuell: `--manifest-sha256` endet bei Schreibfehler mit 1.
- **„Herr Präsident“ und „Ich“ im echten Lauf nicht ausgelöst:** Gate 8 hatte
  0 Fälle. Beide Blöcke sind nur über die Suite (Fake-Texte) belegt.
- **Echter Benchmark** mit beendetem Daemon und installiertem 0.5.0-Bundle steht
  aus (WP5). Das Bundle aus Gate 7 hat `versions.toml`, `target-dev\release\`
  hat keine; dort ergäbe sich „nicht belegt“.
- **Zeilenenden von `src\models.toml`:** Der Manifest-Digest hängt an den Bytes
  im Working Tree. Mit `core.autocrlf=true` hat ein frischer Checkout CRLF statt
  des heutigen LF. Innerhalb eines Checkouts ist das konsistent, weil Build und
  Vergleich dieselbe Datei nutzen; zwischen Checkouts ist der Digest nicht
  stabil.
- **Reste unter `.herd\`:**
  - `.herd\wp2e-probe\` (2,4 MB Kopien aus `testdata`)
  - der Auswertungsordner `.herd\wp2c-root\diktier\ultra-test\auswertung\20260930-203012\`
    (Gate-8-Beleg, enthält Transkripte der Test-WAVs, nur lokal)

  Ralf kann beides löschen.
- Ein Zweit-Review von WP2e durch Sol steht aus.
