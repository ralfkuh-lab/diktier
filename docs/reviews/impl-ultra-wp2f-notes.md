# WP2f — Nacharbeit zum Nachreview WP2e (Sol, F1–F6): Umsetzungsbericht

Stand: 2026-09-30. Auftrag: [impl-ultra-wp2f-prompt.md](impl-ultra-wp2f-prompt.md).
Grundlage: [impl-ultra-wp2-sol-2.md](impl-ultra-wp2-sol-2.md) (F1–F6),
[impl-ultra-wp2e-notes.md](impl-ultra-wp2e-notes.md) und
[../ultra-alltagstest-plan.md](../ultra-alltagstest-plan.md) v2.1. Basis war der
Working Tree auf `2d10ee0` mit WP0–WP2e (0.5.0).

Nichts committet. Geändert habe ich nur `scripts/` und `README.md`, dazu dieser
Bericht. **Kein Rust-Code**: `src/`, `Cargo.*`, `docs/` (außer diesem Bericht),
`testdata/`, `LICENSES/` und `src/models.toml` sind unberührt. Das Binary ist
unverändert (siehe Gate 1). Nichts wurde nach `%LOCALAPPDATA%\diktier`,
`%APPDATA%\diktier` oder `%TEMP%\diktier` geschrieben. Installierte Version und
Daemon blieben unberührt, kein Netz. Dieser Bericht enthält keine Transkripte.

## Übersicht

| Befund | Stand | Wo |
|---|---|---|
| F1 Thread-Provenienz | ✅ umgesetzt | `ultra-test-lib.ps1`, `bench-models.ps1`, `compare-models.ps1`, neu `toml-lib.ps1`, `toml-json.py` |
| F2 Speichern der Seite | ✅ umgesetzt | `compare-models.html` |
| F3 Reparse-Schutz je Schreibziel | ✅ umgesetzt, erweitert um Hardlinks | `ultra-test-lib.ps1`, `compare-models.ps1` |
| F4 »Unterschied nur Zahlenformat« | ✅ umgesetzt, Format 3 | `compare-models.ps1`, `compare-models.html` |
| F5 Dauer und Verlängerung | ✅ umgesetzt nach dem Vorschlag, mit Zusatzbedingungen | `compare-models.ps1`, `ultra-test-lib.ps1` (`Get-WallClockDays`) |
| F6 echte TOML-Prüfung | ✅ umgesetzt | `release.ps1`, neu `toml-lib.ps1`, `toml-json.py` |
| Suite | ✅ 33 → 48 Tests, 1 sichtbar übersprungen | `scripts/tests/Test-UltraScripts.ps1` |

## ✅ F1 — Threads aus der Config, die das Kind liest

- **Umgebung des Kindes.** Mit `-ModelRoot` bekommt `diktier.exe` jetzt
  `LOCALAPPDATA` **und** `APPDATA` auf die Testwurzel (`Set-ChildEnvironment`,
  für `--transcribe-list` und `--version`). Ohne `-ModelRoot` erbt das Kind
  beide unverändert, es gilt also das echte `%APPDATA%`.
- Das Kind bekommt die **aufgelöste** Testwurzel. Bisher ging der rohe
  Parameter durch, etwa das relative `.herd\wp2c-root`, und hing damit am
  Arbeitsverzeichnis des Prozesses.
- **Provenienz.** Der Config-Pfad ist `<APPDATA des Kindes>\diktier\config.toml`
  (`Get-ChildConfigPath`), genau der Pfad aus `config.rs::config_path`. Die
  Thread-Zahl ermittelt `Get-EffectiveThreads` so, wie `config.rs` sie auswertet:
  - Datei fehlt: 0. `load_from` legt dann `DEFAULT_TOML` mit `threads = 0` an
    und nimmt `Config::default()`. Das ist belegt, kein angenommener Default.
  - leer, kein `[engine]` oder kein `threads`: Default 0 (`#[serde(default)]`)
  - Ganzzahl: `clamp(0, CPUs)`, negativ wird also 0. Für das Protokoll zählt
    nur „0 oder nicht 0“.
  - **nicht belegt** (Threads `null`, Problem in `fehlende_belege`): kein
    Python ≥ 3.11, ungültiges TOML, `engine` ist keine Tabelle, `threads` ist
    keine Ganzzahl im i64-Bereich, `APPDATA` des Kindes ist leer
- Gelesen wird mit einem echten TOML-Parser (`tomllib` über
  `scripts\toml-json.py`), nicht per Regex. Der alte Regex hätte etwa
  `engine.threads = 4` oder `engine = { threads = 2 }` als 0 gemeldet; beides
  steht als Testfall in der Suite.
- **Vorher/Nachher.** Nach den Messläufen liest das Skript die Config erneut. Hat
  sich ihr Inhalt geändert oder ergibt sie eine andere Zahl, heißt es „nicht
  belegt“.
- `bench.json` nennt zusätzlich `provenienz.config` und `threads_quelle`.
- Tests:
  - Unit, 11 Config-Fälle: fehlt, leer, ohne `[engine]`, ohne `threads`, 0, 4,
    gepunktet, inline, negativ, Text, ungültig. Dazu „ohne Python → nicht
    belegt“.
  - Ende zu Ende mit `-ModelRoot`: Das echte `APPDATA` zeigt auf eine Config mit
    `threads = 4`, die Testwurzel hat keine. Ergebnis: Threads 0, Config-Pfad
    in der Testwurzel. Das Fake-`diktier` hält fest, dass es mit
    `APPDATA = LOCALAPPDATA = Testwurzel` lief. Umgekehrt (außen 0, Wurzel 4)
    wird der Wert gemeldet.
  - Ende zu Ende ohne `-ModelRoot`, mit `LOCALAPPDATA ≠ APPDATA` und
    widersprüchlichen Werten (0 unter LOCALAPPDATA, 4 unter APPDATA). Gemeldet
    wird 4 aus APPDATA; das Kind sah beide Pfade unverändert.

## ✅ F2 — „Gespeichert“ erst nach `close`

- Die Seite führt getrennt:
  - `changeGen`: jede Änderung zählt hoch
  - `savedGen`: die höchste Generation, deren `close()` in die **aktuelle**
    Datei erfolgreich war
  - `failedGen`: der letzte gescheiterte Schreibvorgang
  - `running`: laufende Schreibvorgänge
  - `fileEpoch`: wechselt mit der Datei; ein später fertiger Schreibvorgang in
    eine alte Datei meldet nichts als gespeichert
- Anzeige (`updateSaveInfo`): „gespeichert in urteile.json um …“ nur bei
  `savedGen === changeGen`. Sonst „wird gespeichert …“, „Änderungen noch nicht
  gespeichert …“ oder bei einem Fehler „NICHT gespeichert“. Die rote Meldung
  bleibt, bis ein späterer Stand erfolgreich geschrieben ist.
- `beforeunload` warnt, solange `changeGen !== savedGen` oder ein Schreibvorgang
  läuft. Das deckt ausstehende, laufende und gescheiterte Speicherungen ab,
  ebenso Änderungen ohne Urteilsdatei.
- Nach „Urteilsdatei öffnen“ gilt der geladene Stand als gespeichert. Nach
  „Zwischenstand löschen“ gibt es nichts mehr zu sichern (ausdrücklich
  verworfen); der Timer wird gestoppt.
- Test (headless Chrome, echter `vergleich.html`, Stub mit verzögertem und
  scheiterndem `close`). Die Warnung wird über ein synthetisches
  `beforeunload` mit `defaultPrevented` geprüft. Ablauf:
  1. Ohne Änderung: keine Warnung.
  2. `close` hängt: 0 Schreibvorgänge, keine Anzeige „gespeichert“, Warnung.
  3. Zweite Änderung während des Schreibens: Warnung.
  4. Erster `close` fertig: 1 Schreibvorgang, weiterhin nicht „gespeichert“,
     Warnung (der zweite Stand fehlt noch).
  5. Zweiter Schreibvorgang scheitert: „NICHT gespeichert“, Meldung
     „fehlgeschlagen“, Warnung.
  6. Dritte Änderung gelingt: „gespeichert in urteile.json …“, keine Warnung,
     die Meldung ist weg, die Datei enthält die dritte Notiz.

## ✅ F3 — Jedes Schreibziel vor dem Öffnen geprüft

- `Assert-WriteTarget` prüft vor jedem Öffnen:
  1. lexikalisch unter dem Schreibordner, wie bisher
  2. `Test-UnderEvaluationRoot` für den konkreten Pfad: jeder Bestandteil und
     die Zieldatei, falls vorhanden, dürfen kein Reparse-Point sein
- Das gilt für alle Schreibhelfer: `Write-Utf8*`, `New-WriteDir`,
  `Invoke-DiktierList` (stdout/stderr) und die Audio-Aliasse.
- Reparse-Erkennung über `File.GetAttributes`. Das folgt dem Link nicht und
  erkennt auch verwaiste Junctions und Symlinks. Nicht prüfbar (etwa Zugriff
  verweigert) gilt als unsicher. Vorher lief das über
  `Get-Item -ErrorAction SilentlyContinue`, das bei einem Fehler still
  „kein Link“ ergab.
- Geprüft wird jetzt von `<Wurzel>` (`%LOCALAPPDATA%` bzw. `-ModelRoot`) bis
  zum Ziel, also auch `…\diktier` und `…\diktier\ultra-test`. Bisher begann die
  Prüfung erst an der Auswertungswurzel.
- **Erweiterung über den Auftrag hinaus:** Eine vorhandene Zieldatei mit
  weiteren Hardlinks (`NumberOfLinks > 1`, `GetFileInformationByHandle`) wird
  nicht beschrieben. Ein Hardlink ist kein Reparse-Point, schreibt aber genauso
  nach außen. Er braucht keine Adminrechte und war damit der realistischere
  Weg im Test.
- `-Resolve` prüft `ergebnis.json`, `zusammenfassung.md` und `bericht.md`
  vorab. So bricht ein Link beim letzten Ziel ab, bevor das erste geschrieben
  ist.
- Tests:
  - Junction als innerer Pfadbestandteil nach außen: Abbruch, außen bleibt
    leer. Dasselbe mit verwaister Junction. Ein normales Ziel wird
    geschrieben.
  - `bericht.md` als Hardlink auf eine Datei außerhalb, Ende zu Ende über
    `-Resolve`: Abbruch mit „Hardlinks“, der Hash der externen Datei ist
    unverändert, `ergebnis.json` wurde nicht neu geschrieben.
  - **Datei-Symlink: übersprungen, mit sichtbarer Meldung.**
    `New-Item -ItemType SymbolicLink` scheitert hier mit „Administrator
    privilege required for this operation.“ (kein Entwicklermodus). Der Test
    versucht den Symlink bei jedem Lauf und läuft, wo er anlegbar ist. Die
    Regel selbst ist dieselbe wie bei der Junction (Reparse-Attribut am
    Zielpfad), die Zieldatei-Seite deckt der Hardlink-Test ab.
- **Grenze:** Zwischen Prüfung und Öffnen bleibt ein Zeitfenster (TOCTOU). Gegen
  vorab liegende Links schützt die Regel, gegen einen gleichzeitig arbeitenden
  Angreifer nicht.

## ✅ F4 — „Unterschied nur Zahlenformat“ als eigene Angabe

- Urteilsschema **Format 3**. Bei A/B ist `nur_zahlenformat` (bool) Pflicht.
  Bei „gleich“ und „unklar“ darf es nicht stehen.
- Kriterium 1: A/B mit `nur_zahlenformat: true` zählt als gleich
  (`K5_als_gleich`), **auch wenn beide Seiten gemeinsame andere Fehler haben**.
  Aus den Kategorien wird nichts mehr abgeleitet (`LoserOnlyK5` entfällt).
- Konsistenzregeln in `-Resolve`; ein Verstoß ist ein Schemafehler, dann wird
  nicht aufgelöst:
  - ja: Die schlechtere Seite hat K5, und die K1–K4-Mengen beider Seiten sind
    gleich. Unterscheiden sie sich, gibt es auch einen anderen Unterschied.
  - nein: Die schlechtere Seite hat mindestens ein K1–K4. Der bisherige
    K5-Fall mit „nein“ ist damit ein Widerspruch, kein stiller Sieg.
- Format 3 gilt einheitlich für `schluessel.json`, `vorbereitung.json` und
  `urteile.json` (F5 ändert die Vorbereitung ebenfalls). Auswertungsordner aus
  WP2e (Format 2) lehnt `-Resolve` mit „neu vorbereiten“ ab.
- Seite: je Paar die Radios „Unterschied nur Zahlenformat: ja / nein“, nur bei
  A/B aktiv. `pairDone` spiegelt die Konsistenzregeln, eine widersprüchliche
  Karte bleibt offen.
- Tests:
  - Sols Szenario: K4 auf beiden Seiten, K5 auf v3, Ultra gewählt, „ja“. Ergebnis
    U 0, entscheidbar 0, als gleich 1. Gegenprobe mit „nein“: U 1.
  - bisheriger K5-Fall mit „ja“ ergibt als gleich 1; mit „nein“ bricht er ab
  - Schemafehler: Angabe fehlt; „ja“ mit verschiedenen Kategorien; „ja“ ohne K5
    auf der schlechteren Seite; Angabe bei „gleich“; altes Format 2
  - Browser: Die Datei enthält `nur_zahlenformat`, P001 ist fertig.

## ✅ F5 — Dauer aus den Grenzen, Verlängerung nur mit 7-Tage-Mengenstand

**Wanduhr-Regel:**

- Die Dauer sind die Wanduhrtage **einer festen Zeitzone**
  (`Get-WallClockDays`). Beide UTC-Grenzen werden in die Zone umgerechnet, dann
  wird die Differenz der Ortszeiten genommen.
- `-Prepare` zählt in der lokalen Zone und speichert deren Windows-ID als
  `zeitraum.zeitzone`. Hier ist das `W. Europe Standard Time`.
- `-Resolve` rechnet mit den gespeicherten Grenzen und der gespeicherten Zone
  neu. Mi 08:00 MEZ bis Mi 08:00 MESZ sind 167 h, aber genau 7 Tage, also kein
  Kurzlauf.
- Fehlt die Zone oder ist sie unbekannt: kein Urteil.

**Prüfreihenfolge in `-Resolve`:**

1. verbindlich
2. Grenzen vorhanden
3. nicht verkehrt
4. abgelaufen
5. Zone bekannt
6. Dauer laut Grenzen genau 7 bzw. 11
7. `tage` gleich der berechneten Dauer, sonst „gespeicherte tage … widersprechen
   den Grenzen“
8. erstellt nach Ende
9. bei einer Verlängerung: die Grundlage

**Mengenstand und Verlängerung (Vorschlag übernommen):**

- Jede verbindliche 7-Tage-Vorbereitung schreibt `mengenstand_7_tage`, noch vor
  jedem Urteil:
  - vollständige Paare mit Sprache
  - Paare mit Textunterschied (Obergrenze der entscheidbaren Paare)
  - die Mindestmengen 300/50
  - `verlaengerung_zulaessig`

  Die Konsole zeigt ihn an.
- `-Prepare -Verlaengert` verlangt jetzt `-Grundlage <Auswertungsordner der
  7-Tage-Vorbereitung>` und prüft vor jedem Schreiben:
  - Grundlage im Format 3, verbindlich, keine Verlängerung
  - 7 Wanduhrtage laut Grenzen, dazu passendes `tage`
  - **gleicher Start**, gleiches Aufnahmeverzeichnis
  - erstellt **nach** Tag 7 und **vor** Ende der Verlängerung („vorab
    festgehalten“)
  - der Mengenstand passt zu ihren eigenen Zählungen
  - **mindestens eine der beiden Zahlen liegt unter ihrer Mindestmenge**

  Die Verlängerung hält Grundlage und Zahlen in `verlaengerung` fest.
- `-Resolve` prüft die Grundlage im selben Auswertungsbaum erneut. Sie muss
  also liegen bleiben. Zusätzlich muss sie älter sein als die Verlängerung, und
  die festgehaltenen Zahlen müssen zu ihr passen. Sonst: „Verlängerung
  unzulässig: …“, kein Urteil.

**Tests:**

- Wanduhrtage: Frühjahr 2026 und Herbst 2025 mit W.-Europe-Zone, UTC-Zählung
  dagegen, unbekannte Zone.
- `-Resolve` über die Zeitumstellung (167 h) ist verbindlich; dieselben Grenzen
  mit Zone `UTC` sind es nicht („statt 7 Tage“).
- Manipulierte Vorbereitung (erweitert):
  - Grenzen auf einen vergangenen Tag verkürzt, `tage = 7`: „statt 7 Tage“.
    Das ist Sols Fall, bisher verbindlich.
  - `tage = 3`: „widersprechen den Grenzen“
  - Zeitzone entfernt
  - läuft noch, vor Ablauf erstellt: wie bisher
- 7-Tage-Vorbereitung schreibt den Mengenstand (11 vollständige Paare,
  8 verschieden, zulässig).
- Verlängerung abgelehnt, und es entsteht kein Auswertungsordner:
  - ohne `-Grundlage`
  - mit einer Grundlage, die erst nach Ende der Verlängerung entstand
  - **trotz erreichter Mengen** (300/60)
- Zulässige Verlängerung (Grundlage an Tag 8) ist verbindlich. Wird die
  Grundlage nachträglich auf erreichte Mengen geändert, heißt es „Verlängerung
  unzulässig … Mindestmengen“. Ohne `verlaengerung`-Block heißt es „ohne
  7-Tage-Grundlage“.

**Folge der Vorab-Bedingung:** Legt Ralf die 7-Tage-Vorbereitung erst nach
Tag 11 an, ist keine verbindliche Verlängerung mehr möglich. Das Ergebnis ist
dann „nicht belegt“. Das ist streng, aber genau „vorab festgehalten“. Die
7-Tage-Vorbereitung selbst bleibt auflösbar. Verfehlt ihr Mengenstand die
Mindestmengen, ergibt Kriterium 1 dort ohnehin „nicht belegt“; zwischen beiden
Vorbereitungen lässt sich also nicht wählen.

## ✅ F6 — Echter TOML-Parser in `release.ps1`

- Neu:
  - `scripts\toml-json.py` (nur Standardbibliothek): liest mit `tomllib` und
    gibt typisiertes JSON aus (Tabellen als Paarlisten in Dateireihenfolge,
    Ganzzahlen als Text). Exit 0 gültig, 1 ungültig, 2 nicht lesbar, 3 Python
    zu alt.
  - `scripts\toml-lib.ps1`: `Invoke-RealToml` sucht `python`, dann `py -3`.
    Das JSON liest System.Text.Json, nicht `ConvertFrom-Json`, weil das
    ISO-Texte zu Datumswerten macht und Schlüssel ohne Groß-/Kleinschreibung
    vergleicht. Tabellen sind Ordinal-Dictionaries, Ganzzahlen BigInteger.
    Dazu `Compare-TomlValue`.
- `Assert-RealTomlAgrees` in `release.ps1`:
  - Die Datei muss laut `tomllib` gültig sein.
  - `tomllib` muss **genau dieselben Werte** lesen wie der Teilmengenparser:
    ganzes Dokument, Typen, case-sensitive Schlüssel.
  - Aufgerufen für `src\models.toml` **vor dem Build** und für die erzeugte
    `versions.toml` im Bundle-Gate.
  - Ohne Python ≥ 3.11 bricht das Skript mit klarer Meldung ab.
- Tests:
  - Das echte Manifest besteht.
  - Negativfixture mit führender Null (`bytes = 0…`): Die Teilmenge nimmt sie
    weiter an (`Get-ModelCatalog` besteht), die echte Prüfung bricht mit „kein
    gültiges TOML“ ab.
  - Dasselbe für eine nach Release-Muster erzeugte `versions.toml`, gültig und
    mit führender Null. `Assert-VersionsMatchManifest` besteht beide Male, die
    echte Prüfung fängt es.
  - ohne Python: Abbruch
  - `Compare-TomlValue` meldet Textabweichung (Groß/klein), Zahl und
    Zusatzschlüssel.
- Echte Negativprobe: `release.ps1 -SkipBuild -SkipInstaller` mit einem PATH
  ohne Python-Verzeichnisse und ohne `C:\Windows` (dort liegt `py.exe`):
  ```
  == Diktier 0.5.0 (win-x64), TargetDir=target
  Exception: D:\DEV\diktier\scripts\release.ps1:57
  Line |
    57 |      throw "release.ps1: $Message"
       |      ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
       | release.ps1: echte TOML-Prüfung von models.toml nicht möglich: kein Python >= 3.11 mit tomllib gefunden
       | (gesucht: python, py -3). Python 3.11 oder neuer installieren (tomllib).

  release exit=1
  zip unverändert: True
  ```

## Suite und Mutationsprobe

- `Test-UltraScripts.ps1`: 15 neue Tests. Dazu:
  - `Skip-Case` für sichtbar übersprungene Fälle; die Summary nennt sie.
  - `-Only <Muster>` als Entwicklungshilfe: filtert Tests, die übrigen zählen
    nicht. Setup-abhängige Blöcke sind abgesichert, damit ein Teillauf sein
    Temp-Verzeichnis nicht stehen lässt.
  - Das Fake-`diktier` schreibt `env-seen.json` mit seinen Benutzerpfaden.
- **Mutationsprobe** in einer Scratchpad-Kopie (danach gelöscht), mit acht
  wieder eingebauten Fehlern:
  - `APPDATA` nicht isoliert
  - `unsaved()` immer falsch
  - Reparse-Prüfung am Ziel und Hardlink-Prüfung entfernt
  - „nur Zahlenformat“ wieder aus den Kategorien abgeleitet
  - Dauer aus den Grenzen und Mengenstand ignoriert
  - echte TOML-Prüfung übersprungen

  Ergebnis: `Test-UltraScripts: 48 Tests, 37 ok, 11 fehlgeschlagen, 1
  übersprungen`. Es schlugen genau die zugehörigen Tests fehl: F1 (ModelRoot),
  F2, beide F3, F4 (gemeinsamer K4), drei F5-Tests, der W7-Manipulationstest
  und beide F6-Tests.
- Ein erster Mutationslauf war zu grob: Die F4-Mutation griff auf eine bei
  „gleich“ nicht gesetzte Variable zu und brach jeden Resolve. Er ist nicht
  gewertet.

## Gate-Ausgaben (wörtlich)

1. **`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
   `cargo test`: nicht ausgeführt, weil kein Rust-Code berührt ist.**
   - WP2f hat keine Datei unter `src/` und kein `Cargo.*` geändert.
   - F1 ließ sich ganz in den Skripten lösen: Die Umgebung des Kindes wird
     gesetzt, die Config genau wie in `config.rs` ausgewertet.
   - Beleg, dass das Binary gleich blieb: `release.ps1` in Gate 4 kompilierte
     nichts (`Finished … in 0.22s`), und
     ```
     D:\DEV\diktier\target\release\diktier.exe  2026-09-30 20:25:05  FD31774B42B03E15
     D:\DEV\diktier\dist\diktier-0.5.0-win-x64\diktier.exe  2026-09-30 20:25:05  FD31774B42B03E15
     ```
     Das ist der Build aus WP2e; WP2f hat nichts neu kompiliert. Der
     Manifest-Digest ist unverändert `9f88fb6d…`.
2. `pwsh -File scripts\tests\Test-UltraScripts.ps1`:
   ```
   Test-UltraScripts: 48 Tests, 48 ok, 0 fehlgeschlagen, 1 übersprungen
     SKIP F3: Datei-Symlink nach außen bricht Resolve vor dem Schreiben ab (Datei-Symlink nicht anlegbar ohne Adminrechte/Entwicklermodus: Administrator privilege required for this operation.. Abgedeckt über Junction (Pfadbestandteil) und Hardlink (Zieldatei).)
   suite exit=0
   Reste: 0
   ```
   Der Browser-Teil lief mit, headless Chrome. „Reste“ zählt die
   `diktier-ultra-tests-*` in `%TEMP%` nach dem Lauf.
3. `python scripts\tests\test_quantize_ultra.py` (Python 3.12.12), aus
   PowerShell aufgerufen. Die Leerzeile von unittest auf stderr zeigt
   PowerShell als `RemoteException`:
   ```
   Ran 11 tests in 0.204s
   System.Management.Automation.RemoteException
   OK
   python exit=0
   ```
4. `scripts\release.ps1 -SkipInstaller`:
   ```
   == Diktier 0.5.0 (win-x64), TargetDir=target
   == src\models.toml: gültiges TOML laut tomllib (python), gleich gelesen wie die Teilmenge
   == cargo build --release --locked (CARGO_TARGET_DIR=target)
       Finished `release` profile [optimized] target(s) in 0.22s
   == Manifest im Binary = src\models.toml (SHA-256 9f88fb6d6e471b78191e6fc3d84e67e90b07c9ba06135ecfc8dff38fa4693069), --version 0.5.0
   == Bundle D:\DEV\diktier\dist\diktier-0.5.0-win-x64
   == Modelle: parakeet-tdt-0.6b-v3-int8, parakeet-ultra-0.6b-int8-pc (Default parakeet-tdt-0.6b-v3-int8)
   == Bundle-Gate: versions.toml gegen src\models.toml
      versions.toml: gültiges TOML laut tomllib, gleich gelesen wie die Teilmenge
      ok parakeet-tdt-0.6b-v3-int8: huggingface revision=8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce, 4 Dateien
      ok parakeet-ultra-0.6b-int8-pc: github-release release_tag=model-parakeet-ultra-0.6b-int8-pc-r1, 3 Dateien
   == Selbstprüfung
   == Zip D:\DEV\diktier\dist\diktier-0.5.0-win-x64.zip
   == Installer übersprungen (-SkipInstaller)

   Bundle:  D:\DEV\diktier\dist\diktier-0.5.0-win-x64

   Zip:     D:\DEV\diktier\dist\diktier-0.5.0-win-x64.zip
     Größe:   8,2 MB
     SHA-256: 3130da4b3a65ab9758d0e8d41b9fcd1383fc0a27bfdf4883b078d74b35a6e395

   release exit=0
   ```
   Der Zip-Hash weicht vom ersten Lauf dieser Session ab (`ad00d611…`), weil
   das Bundle die geänderte README enthält.
5. **Echter Lauf.**
   - Aufruf: `compare-models.ps1 -Prepare -Explorativ -Exe
     dist\diktier-0.5.0-win-x64\diktier.exe -ModelRoot .herd\wp2c-root -WavDir
     .herd\wp2e-probe\wav -LogDir .herd\wp2e-probe -Seed 7`
   - Eingaben: die Probe-WAVs aus WP2e (8 mit Zeitstempel, `umbenannt.wav`) und
     deren synthetisches Log.
   - Danach statische Seitenprüfung und headless Chrome mit Datei-Stub, der
     `close` um 30 ms verzögert. Bedient wird alles per Klick; P001 ist das
     F4-Szenario (K4 auf beiden Seiten, K5 auf der schlechteren, „nur
     Zahlenformat: ja“), die übrigen „nein“ mit K4. Dazu eine Notiz.
   - Die zuletzt geschriebene Datei ist das simulierte `urteile.json` im
     Format 3, dann `-Resolve`.
   - Probe-Skript im Scratchpad, schreibt nur unter `.herd\`.
   ```
   ### Vorher: D:\DEV\diktier\.herd\wp2c-root\diktier\config.toml vorhanden: False; echte %APPDATA%\diktier\config.toml SHA-256 78841B220741…
   ### compare-models.ps1 -Prepare -Explorativ
   == Auswertungsordner D:\DEV\diktier\.herd\wp2c-root\diktier\ultra-test\auswertung\20260930-211701 (explorativ)
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
   == Seite D:\DEV\diktier\.herd\wp2c-root\diktier\ultra-test\auswertung\20260930-211701\vergleich.html
      3 Paare, 3 Stichprobe, 0 »Herr Präsident«, 0 »Ich«
      Die Seite fragt beim ersten Urteil nach der Urteilsdatei: D:\DEV\diktier\.herd\wp2c-root\diktier\ultra-test\auswertung\20260930-211701\urteile.json anlegen.
   prepare exit=0
   ### Nachher: D:\DEV\diktier\.herd\wp2c-root\diktier\config.toml vorhanden: True (vom Kind angelegt, APPDATA = Testwurzel); echte %APPDATA%\diktier\config.toml SHA-256 78841B220741…
   ### Seitenprüfung statisch
      Format schluessel/vorbereitung: 3/3, Zeitzone W. Europe Standard Time, mengenstand_7_tage keiner (explorativ)
      externe src/href: 0
      http(s)://: 0
      file:///: 0
      rec_ / lauf-: 0
      Modellschlüssel: 0
      Seed: 0
      < > & im JSON-Block: 0
      Aliasse bytegleich zum Original: 6/6 (hardlink 6)
      A/B-Texte bitgleich zu roh-<Modell>.jsonl laut schluessel.json: 3/3
   ### Seite headless (chrome), Bedienung mit Datei-Stub
      Browser exit 0; Fehler bei der Bedienung: 0
      Karten 6, davon fertig 6, Audio-Elemente 6, Fortschritt: Paare 3/3 · Stichprobe 3/3 · Herr Präsident 0/0 · Ich 0/0
      direkt nach dem letzten Klick: Anzeige 'nicht gespeichert', beforeunload warnt: True
      nach dem Schreiben: Anzeige 'gespeichert in urteile.json um 21:17:14', beforeunload warnt: False; Meldungen: ''
      Schreibvorgänge in die Urteilsdatei: 1
      Browser-Speicher mit Notiz: False, mit Transkript: False
      letzte Datei: format 3, vollstaendig True, Paare 3, Stichprobe 3, HP 0, Ich 0
   ### compare-models.ps1 -Resolve
   == Aufgelöst: D:\DEV\diktier\.herd\wp2c-root\diktier\ultra-test\auswertung\20260930-211701\zusammenfassung.md (nur Zahlen), D:\DEV\diktier\.herd\wp2c-root\diktier\ultra-test\auswertung\20260930-211701\bericht.md (lokal, mit Texten)
      explorativ, kein Urteil: Vorbereitung ist explorativ
      U 0, V 2, Quote 0.0 %, Mindestmengen nicht erfüllt, neue Klassen keine, Veto 0
      Halluzinationen »Herr Präsident« v3 0 / Ultra 0, »Ich« v3 0 / Ultra 0
      Kriterium 1: explorativ, kein Urteil · 2: explorativ, kein Urteil · 3: explorativ, kein Urteil · 6: explorativ, kein Urteil
   resolve exit=0
      ergebnis.json: format 3, verbindlich False, Grund 'Vorbereitung ist explorativ', U 0, V 2, K5_als_gleich 1, K4 Ultra-Seiten 3, v3-Seiten 1
      zusammenfassung.md: Transkripte 0, Notiz 0, .wav 0, rec_ 0
      bericht.md (lokal): Notiz enthalten 0
      Auswertungsordner: D:\DEV\diktier\.herd\wp2c-root\diktier\ultra-test\auswertung\20260930-211701
   ```
   Einordnung:
   - **F1 im echten Lauf:** Das echte Binary hat seine Config unter der
     Testwurzel angelegt (`APPDATA` isoliert). Die echte
     `%APPDATA%\diktier\config.toml` ist vorher und nachher gleich.
   - **F4 im echten Lauf:** P001 („nur Zahlenformat: ja“ trotz gemeinsamem K4)
     zählt als gleich (`K5_als_gleich 1`). Die beiden „nein“-Paare ergeben V 2.
     U/V spiegeln nur die simulierten Klicks, keine Qualitätsaussage.
   - **F2:** Direkt nach dem letzten Klick war der Picker noch nicht fertig,
     also „nicht gespeichert“ mit Warnung. Nach dem verzögerten `close` hieß es
     „gespeichert“ ohne Warnung.
   - **„bericht.md: Notiz enthalten 0“ ist ein Fehler der Probe, nicht des
     Skripts.** `bericht.md` maskiert Markdown-Zeichen, die Zeile lautet
     `- Notiz: Probe\-Notiz`, die Probe suchte unmaskiert. Nachgeprüft:
     `urteile.json` enthält `"notiz": "Probe-Notiz"`, `bericht.md` Zeile 12 die
     maskierte Notiz.

## Abweichungen und Entscheidungen

- **F1:**
  - Die Thread-Auswertung braucht Python ≥ 3.11 (`tomllib`) auch in
    `bench-models.ps1`. Ein Regex über TOML wäre genau die Unsicherheit, die
    „nicht belegt“ heißen soll. Python ist auf dem Build- und Messrechner
    derselbe (Vorgabe F6). Ohne Python ist Kriterium 4 „nicht belegt“, nie
    „erfüllt“.
  - Eine fehlende Config gilt als belegte 0, weil `config.rs` dann nachweislich
    die Default-Datei mit `threads = 0` anlegt und `Config::default()` nimmt.
    Die Vorher/Nachher-Prüfung bestätigt es.
  - Die obere Grenze (`available_parallelism`) bildet das Skript nur für die
    Anzeige nach (`ProcessorCount`); für das Protokoll zählt allein „0 oder
    nicht 0“.
- **F3:** Hardlink-Sperre und Prüfung ab `<Wurzel>` statt ab der
  Auswertungswurzel gehen über den Auftrag hinaus (Begründung oben).
  Datei-Symlink-Test übersprungen, weil hier ohne Adminrechte nicht anlegbar.
- **F4:** Feldname `nur_zahlenformat`. Die Konsistenzregeln („ja“ ⇒ K5 auf der
  schlechteren Seite und gleiche K1–K4, „nein“ ⇒ ein K1–K4 auf der
  schlechteren Seite) sind meine Auslegung. Sie verhindern, dass ein „ja“ einen
  echten Inhaltsunterschied verdeckt oder ein „nein“ einen reinen K5-Sieg.
- **F5:**
  - Vorschlag übernommen, zusätzlich verlangt: gleicher Start, gleiches
    Aufnahmeverzeichnis, Grundlage vor Ende der Verlängerung erstellt und älter
    als die Verlängerung, Zahlen in sich stimmig.
  - Die Zeitzone ist die lokale beim Vorbereiten; eine Vorbereitung auf einem
    Rechner mit anderer Zone zählt in dieser anderen Zone. Für Ralfs einen
    Rechner ist das gleich.
- **Format 3** für alle drei Dateien; WP2e-Ordner neu vorbereiten.

## 🔍 Offen

- **Plan-Nachtrag nötig (docs/ war gesperrt).** `ultra-alltagstest-plan.md`,
  Bewertungsprotokoll:
  - Die Angabe „Unterschied nur Zahlenformat“ bei A/B, samt der
    Konsistenzregeln.
  - Die Verlängerungsregel: Mengenstand nach 7 Tagen mit Paaren mit
    Textunterschied als Obergrenze; zulässig, wenn eine Zahl unter der
    Mindestmenge liegt; die 7-Tage-Vorbereitung vor Tag 11 anlegen und liegen
    lassen.
  - Die Wanduhr-/Zeitzonenregel.
  - Den WP2f-Status setzt der Orchestrator.
- **Datei-Symlink-Test** lief auf diesem Rechner nicht (keine Adminrechte bzw.
  kein Entwicklermodus). Er läuft automatisch mit, wo er anlegbar ist.
- **Python-Abhängigkeit:** `release.ps1` und das Thread-Urteil in
  `bench-models.ps1` brauchen `python` oder `py -3` mit Version ≥ 3.11. Hier
  gefunden: `python` 3.12.12 (uv). In der README steht es.
- **Reste unter `.herd\`:**
  - `.herd\wp2c-root\diktier\config.toml`: Default-Config, vom echten Binary in
    Gate 5 angelegt, weil `APPDATA` jetzt isoliert ist
  - `.herd\wp2c-root\diktier\ultra-test\auswertung\20260930-211701\`: Beleg von
    Gate 5, enthält Transkripte der Test-WAVs, nur lokal

  Ralf kann beides löschen. `dist\` wurde neu gebaut, die README hat sich
  geändert.
- Ein Nachreview von WP2f durch Sol steht aus.
