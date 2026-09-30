# Nachreview WP2e — GPT-6.1 Sol

Stand: 2026-09-30, Working Tree 0.5.0 auf `HEAD 2d10ee0`.
Auftrag: `docs\reviews\impl-ultra-wp2-review2-prompt.md`.
Grundlagen: erstes Sol-Review, WP2e-Umsetzungsbericht, Alltagstest-Plan v2.1
und SPEC v1.10 §9.

**Ergebnis:** 10 der 14 geprüften Punkte sind behoben, vier teilweise.
Der kritische Quellverlust K1 ist behoben. Es bleiben wichtige Lücken bei
Ablagegrenzen, Messprovenienz und verbindlicher Auswertung; neu hinzu kommen
ein Verlustfenster beim Browser-Speichern und eine K5-Fehlzählung bei
gemeinsamen Fehlern. Die drei angeforderten Testsuiten sind grün.

**WP3a kann aus enger Veröffentlichungssicht mit Ralfs Go starten:** Die
offenen Befunde betreffen überwiegend die spätere Bewertung, nicht die
unveränderlichen Modellartefakte oder deren aktuelle Download-Adressen.
Das ist **keine vollständige Abnahme von WP2e und keine Freigabe der
verbindlichen WP5-Auswertung**. Einzelheiten und Grenzen stehen unten.

Nur dieser Bericht wurde dauerhaft erstellt. Keine Implementierung geändert,
keine Weiterdelegation, kein Commit, keine Modelle geladen, kein Netzaufruf,
kein Eingriff in Installation oder Daemon.

## Status der ursprünglichen Befunde

Die Bewertung von W2/L3 berücksichtigt die ausdrücklich geänderte Regel:
Urteilscodes im Browser sind jetzt erlaubt, Notizen dort weiterhin nicht.
Die ursprüngliche, bewusst gewählte Picker-Datei wird nicht erneut als
unerlaubter Automatismus bewertet.

| Befund | Status | Begründung am jetzigen Code |
|---|---|---|
| K1 — Quantisierungs-Cleanup/Quelle | **behoben** | `scripts\quantize-ultra.py:284–321,329–410`: aufgelöste Pfade werden vor Seiteneffekten in beiden Überlappungsrichtungen geprüft; vorhandenes Arbeitsverzeichnis wird abgelehnt; Cleanup hängt an den Eigentumsflags. Die 11 Python-Tests decken insbesondere das ursprüngliche Quellverlust-Beispiel und den frühen Importfehler ab. |
| W1 — fremde `last_recording.wav` | **behoben** | `src\daemon\debug_wav.rs:143–158,382–384`: ein ausdrücklich gewähltes Verzeichnis hat `legacy_cleanup = false`. `workers.rs:721–724` übergibt die vollständige Config. Der neue Rust-Test ab Zeile 945 prüft auch fremde `.part`-Dateien und die Default-Gegenprobe. |
| W2 — Ablageort/Export | **teilweise** | Browser speichert keine Notizen mehr in localStorage; kein Download-Ersatz; native Eingabeordner und explizite Urteils-/Listenpfade werden gegen die Auswertungswurzel geprüft. Die einzelnen Schreibziele werden jedoch nur lexikalisch geprüft: ein vorhandener Datei-Symlink im erlaubten Ordner kann sensible Ausgabe nach außen umleiten. Siehe F3. |
| W3 — Warmup/Exit/JSONL | **behoben** | `src\transcribe_list.rs:215–230`: gescheiterter Warmup erzeugt für die gesamte betroffene Datei `error`; nächste freigegebene Datei wärmt erneut auf. `ultra-test-lib.ps1:247–256` prüft Exit gegen Zeilenstatus; beide Skripte verwenden den Helfer. Rust- und Skript-Regressionstests sind grün. |
| W4 — Foreground-Sitzungen/Inventur | **behoben** | `scripts\ultra-test-lib.ps1:269–340,350–450`: beide Startformen werden erkannt, Sitzungen ereignisbezogen getrennt, Dump-Zeilen zur Zuordnung benutzt. Abgeschnittener Anfang, doppelte Laufnummern und grenznahe Gate-Zeiten werden gesondert ausgewiesen. Die vier Inventurtests sind grün. |
| W5 — Exklusivität/Unabhängigkeit in Kriterium 2 | **behoben** | `scripts\compare-models.ps1:580–650,671–715`: Kategorien beider Seiten; Exklusivität nach v2.1 durch Kategorie-Differenz; gemeinsame Stichprobenfehler zählen für v3; Fallketten werden zusammengeführt, Kreise abgelehnt; Veto nur für exklusives gravierendes K1. Die entsprechenden positiven und negativen Tests sind grün. F4 betrifft eine andere, neue Wechselwirkung mit Kriterium 1. |
| W6 — „Ich“ | **behoben** | Vorbereitung erzeugt den neuen Audio-Block nur bei genau einem „Ich“-Beginn mit Wortgrenze; Browser und Resolver unterstützen beide Halluzinationstypen. `compare-models.ps1:720–738` berechnet getrennte Zähler und das gemeinsame Kriterium. Gesprochen/nicht gesprochen wird getestet. |
| W7 — Zeitraum/verbindlich | **teilweise** | Normale Vorbereitung verlangt einen abgeschlossenen Zeitraum von 7 bzw. 11 Wanduhrtagen; explorative Vorbereitung erhält kein verbindliches Kriterienurteil; nicht datierbare WAVs zählen nicht. Resolve vertraut aber dem separaten Feld `tage`, statt die Dauer aus den Grenzen zu prüfen. Auch der zulässige Verlängerungsgrund bleibt ungeprüft. Siehe F5. |
| W8 — vollständiges Benchmark-Protokoll | **teilweise** | 3 × 3, Daemonprüfung, Version 0.5.0, DLL-Hash gegen Bundle und ein Peak je Prozess sind jetzt erforderlich; Kurzproben liefern kein Urteil. Die Thread-Provenienz liest jedoch die falsche Config und kann den Default behaupten, obwohl die Engine anders läuft. Siehe F1. |
| W9 — eingebettetes Manifest | **behoben** | `src\download.rs:300–302`, `src\main.rs:91–94,285–292`: exklusiver, config-/modellfreier Digest-Modus. `scripts\release.ps1:338–351,403–404` prüft Digest und Version auch bei `-SkipBuild`, vor Änderungen am Bundle. Rust-Test vergleicht eingebettete mit Dateibytes; Fake-Tests prüfen Abweichungen. |
| L1 — Optionen vor Autostart | **behoben** | `src\main.rs:277–281,393–407`: Validierung unmittelbar nach Clap und vor jeder Aktion. Parser-/Validierungstest ab Zeile 1614 führt selbst keinen Autostart aus. |
| L2 — strengere TOML-/Manifestprüfung | **teilweise** | Ordinale Schlüssel, case-sensitive Vergleiche und Quelle/URL/Hash-Prüfungen sowie `COMPLETE`/`.part`-Sperren sind vorhanden und getestet. Unabhängige TOML-Gegenprüfung fehlt weiterhin; Digest-Gleichheit ersetzt sie nicht. Konkrete verbleibende Parserabweichung in F6. |
| L3 — lautlose Browser-Speicherfehler | **behoben** | `scripts\compare-models.html:132–160,246–250`: Lesen, beschädigter Stand, volle/gesperrte Speicherung und Dateischreibfehler werden sichtbar behandelt. Die fünf Browser-Tests liefen tatsächlich mit. F2 ist ein neues Lebenszyklusproblem, kein weiterhin verschluckter localStorage-Fehler. |
| Blindheit — Audio-Pfade | **behoben** | `compare-models.ps1:220–228,329–341`: neutrale relative Audio-Aliasse, Zuordnung getrennt im Schlüssel. Die Seite enthält keine ursprünglichen Aufnahme-/Laufnamen, Modellschlüssel oder den Seed; Alias-Bytegleichheit wird getestet. Die im Plan offen benannte Restbefangenheit durch Live-Nutzung und Zahlenformat bleibt bestehen. |

## Neue Befunde und konkrete Restszenarien

Keine neuen kritischen Befunde. F1, F3, F5 und F6 konkretisieren die
teilweise behobenen Punkte; F2 und F4 sind neue Fehler beziehungsweise
Wechselwirkungen der WP2e-Änderungen.

### Wichtig

### F1 — Thread-Nachweis liest LOCALAPPDATA, die Engine liest APPDATA

**Bezug:** Rest W8.

**Stellen:** `scripts\bench-models.ps1:128–141,149`;
`scripts\ultra-test-lib.ps1:166`; `src\config.rs:356–364`;
`src\main.rs:430–443,523–536`.

`Get-Provenance` liest `<Root>\diktier\config.toml`. `Root` ist hier
LOCALAPPDATA beziehungsweise `-ModelRoot`. Rust liest dagegen
`%APPDATA%\diktier\config.toml`; genau diese Config liefert dem
Transkriptionsmodus `engine.threads`. Der Prozesshelfer überschreibt bei
`-ModelRoot` nur LOCALAPPDATA, nicht APPDATA.

**Szenario:** In der echten APPDATA-Config steht `threads = 4`; im geprüften
LOCALAPPDATA-Pfad existiert keine Config. Provenienz meldet Threads 0 ohne
Problem, die Messung läuft tatsächlich mit 4. Bei ansonsten vollständigen
Belegen kann Kriterium 4 „erfüllt“ melden. Umgekehrt kann eine unbenutzte
Config unter der Modellwurzel einen gültigen Lauf unnötig sperren.
Eine reale CLI unter `-ModelRoot` ist außerdem hinsichtlich der Config nicht
vollständig isoliert: Fehlt die echte APPDATA-Datei, kann `config::load`
sie dort anlegen.

**Vorschlag:** Die effektive Config des Kindes als Quelle verwenden und
die Test-Config ausdrücklich isolieren. Pfad und effektive Threadzahl
sollten derselben Auswertung wie in Rust folgen, nicht einem Regex auf
einer anderen Datei. Regressionstest mit verschiedenem APPDATA und
LOCALAPPDATA sowie widersprüchlichen Threadwerten.

**Testlücke:** `scripts\tests\Test-UltraScripts.ps1:744–772` schreibt die
Fixture gerade an den falschen Modellwurzel-Pfad; das Fake-Binary liest
keine Rust-Config. Dieser grüne Test belegt den produktiven Pfad nicht.
DLL-Hashabgleich und Versionsabfrage sind davon unabhängig umgesetzt.

### F2 — Schließen während eines Dateischreibvorgangs verliert die letzten Notizen ohne Warnung

**Bezug:** neuer Browser-Persistenzfehler.

**Stellen:** `scripts\compare-models.html:232–257,494–496`.

`writeNow` setzt `pending = false`, **bevor** der Vorgang in `writeChain`
wartet beziehungsweise `createWritable`, `write` und `close` beendet sind.
`beforeunload` warnt ausschließlich bei `pending`. Es gibt keinen gesonderten
Zustand für laufende/aufgestaute oder fehlgeschlagene Dateischreibvorgänge.

**Szenario:** Ralf bearbeitet eine Notiz. Nach 400 ms läuft der Timer an;
der Dateischluss dauert noch oder eine ältere Speicherung blockiert die
Queue. Ralf lädt die Seite neu beziehungsweise schließt sie. Es erscheint
keine Warnung, obwohl der neue Stand noch nicht dauerhaft in der Datei ist.
Gerade die Notiz kann danach nicht aus localStorage wiederhergestellt
werden, weil dort absichtlich nur Codes liegen. Die bisherige grüne
„gespeichert“-Anzeige muss währenddessen nicht zurückgenommen werden.

**Nachweis:** Den unveränderten `writeNow`- und `beforeunload`-Code isoliert
mit einem verzögerten Fake-Dateischluss ausgeführt, ohne Browserprofil
oder echte Datei:

```text
Ausstehender Dateischluss: true; pending=false; beforeunload warnt: false
```

**Vorschlag:** Änderungs-/Persistenzgeneration und laufende Queue getrennt
verfolgen. Erst nach erfolgreichem `close` des aktuellen Standes als
gespeichert markieren; bei neuen, laufenden oder fehlgeschlagenen
unpersistierten Änderungen warnen. Test mit verzögertem `close`, einer
zweiten Änderung während des Schreibens und nachfolgendem Schreibfehler.
Die bestehenden Browser-Stubs schreiben sofort und decken das Fenster nicht ab.

### F3 — Reparse-Schutz endet am Ordner, nicht am tatsächlichen Schreibziel

**Bezug:** Rest W2.

**Stellen:** `scripts\ultra-test-lib.ps1:81–118,121–132`;
`scripts\compare-models.ps1:549–553,889`.

`Set-WriteRoot` validiert den Auswertungsordner einschließlich seiner
Pfadbestandteile. `Assert-WriteTarget` prüft danach nur den normalisierten
String gegen diesen Ordner. Der vorhandene Reparse-Check wird für den
konkreten Dateipfad nicht erneut benutzt.

**Szenario:** Der erlaubte Auswertungsordner ist ein normaler Ordner,
enthält aber bereits einen Datei-Symlink `bericht.md` auf eine Datei
außerhalb der Auswertungswurzel, etwa in einem synchronisierten Verzeichnis.
Resolve akzeptiert den Ordner; `Write-Utf8Lines` akzeptiert den lexikalisch
darunterliegenden Namen. `WriteAllLines` folgt dem Link und schreibt
Transkripte und Notizen außerhalb der zugesicherten Ablage.
Dafür ist kein Rennen nötig; ein vorab vorhandener Link reicht.

**Vorschlag:** Jeden konkreten Zielpfad einschließlich vorhandener
Datei-/Verzeichnisbestandteile vor dem Öffnen gegen die Reparse-Regel prüfen,
zusätzlich zur lexikalischen WriteRoot-Grenze. Regressionstest mit einem
erlaubten echten Ordner und einem nach außen zeigenden inneren Datei-Link;
die externe Datei muss unverändert bleiben.

**Nachweisgrenze:** Statischer Schreibpfad-Nachweis; kein externer
Symlink und keine sensible Ausgabe wurden im Review angelegt.

### F4 — Gemeinsamer Fehler macht einen reinen Zahlenformat-Sieg zum Erkennungsgewinn

**Bezug:** neue Wechselwirkung des beidseitigen Schemas mit Kriterium 1.

**Stellen:** `scripts\compare-models.ps1:588–600,651–664`;
`docs\ultra-alltagstest-plan.md:152–155,170–172`.

`LoserOnlyK5` wird ausschließlich daraus abgeleitet, dass die schlechtere
Seite **keine** K1–K4-Kategorie enthält. Das neue Schema erlaubt und verlangt
aber auch die Erfassung gemeinsamer Fehler.

**Szenario:** Beide Ausgaben enthalten denselben Schreibfehler, also K4
auf A und B. Ihr einziger Unterschied ist die Zahlendarstellung bei
gleichem Wert; Ralf bevorzugt Ultras Ziffern, wählt die Ultra-Seite und
markiert K5 auf der anderen Seite. Dieses gültig darstellbare Urteil
passiert die Prüfung. Wegen des gemeinsamen K4 ist `LoserOnlyK5 = false`;
Ultra bekommt U und ein entscheidbares Paar, statt „gleich/K5“.
Viele solche Paare können sowohl die Mindestmenge 50 als auch U ≥ 2·V
unzulässig beeinflussen. Der Plan schließt reine K5-Gewinne ausdrücklich aus.

**Vorschlag:** Den Grund der A/B-Präferenz beziehungsweise „Unterschied
nur Zahlenformat“ ausdrücklich erfassen und bei Kriterium 1 auswerten.
Nicht einfach alle identischen Kategorie-Sets als K5 behandeln: Zwei
unterschiedlich schwere Inhaltsfehler können derselben Kategorie angehören.
Das gemeinsame K4-plus-reines-K5-Szenario zusätzlich zum bestehenden
„K5 allein auf der schlechteren Seite“-Test prüfen.

**Nachweisgrenze:** Statische Auswertung des akzeptierten Schemafalls;
keine realen Transkripte beurteilt.

### F5 — Resolve prüft nicht die tatsächliche Dauer; Verlängerung bleibt ohne Grundnachweis

**Bezug:** Rest W7.

**Stellen:** `scripts\compare-models.ps1:195–216,521–534`.

Die Vorbereitung prüft die Dauer korrekt. Bei Resolve wird dagegen nur
`zeitraum.tage` mit 7/11 verglichen. Die eingelesenen Grenzen werden nur auf
Reihenfolge und abgeschlossenes Ende geprüft; die tatsächliche Differenz
wird nicht mit dem gespeicherten Tageswert abgeglichen.

**Szenario:** In `vorbereitung.json` werden die Grenzen nachträglich auf
einen vergangenen eintägigen Zeitraum geändert, `tage = 7` bleibt stehen.
`Get-NonBindingReason` akzeptiert ihn weiterhin als verbindlich. Die neue
Mutationssuite prüft bei „statt 7 Tage“ nur die Änderung von `tage`, nicht
diesen widersprüchlichen Fall.

**Nachweis:** Die unveränderte Funktion und ihre reinen Helfer wurden
isoliert aus den PowerShell-ASTs geladen, ohne Skript-Einstiegspunkt:

```text
Tatsaechlicher Zeitraum: 1 Tag; gespeicherte tage: 7; Ablehnungsgrund: ''; verbindlich: True
```

Unabhängig davon schaltet `-Verlaengert` jeden passenden elf Tage langen
Zeitraum frei. Es gibt weder einen Siebentage-Mindestmengenstand noch
einen anderen geprüften Nachweis, dass die Verlängerung tatsächlich wegen
fehlender Mengen und nicht wegen einer verfehlten Quote erfolgte.
Diese ursprüngliche W7-Teilanforderung ist weiterhin Operator-Vertrauen.

**Vorschlag:** Dauer aus den gespeicherten Grenzen mit nachvollziehbarer
Wanduhr-/Zeitzonenregel herleiten und Inkonsistenzen sperren. Für die
Verlängerung den zulässigen Siebentage-Mengenstand beziehungsweise einen
vorab festgehaltenen Grund abbilden. Ergänzende Tests für widersprüchliche
Grenzen/Tageszahl und eine unzulässige Verlängerung trotz erreichter
Mindestmengen. Zeitumstellung dabei nicht versehentlich als Kurzlauf werten.

### Kleinigkeit / verbleibende Release-Prüfgrenze

### F6 — Der Teilmengenparser akzeptiert weiter ungültiges TOML

**Bezug:** Rest L2; kein Fehler im jetzigen Manifest.

**Stellen:** `scripts\release.ps1:92–93,182,276–277,338–351`;
`src\download.rs:300–302`.

Die neue Case-Sensitivität behebt die konkreten früheren Namens-/URL-Fälle.
Eine unabhängige Syntaxprüfung ist weiterhin nicht vorhanden. Beispielsweise
akzeptiert `Read-TomlValue` `0700507227` als Dezimalzahl 700507227, obwohl
TOML führende Nullen für solche Integer verbietet.

**Nachweis:** Die Release-Funktionen per vorgesehenem Dot-Source-Schutz
geladen und den Wert mit Pythons Standardbibliotheks-TOML-Parser verglichen:

```text
Release-TOML-Teilmenge: bytes=700507227 akzeptiert
tomllib.TOMLDecodeError: Expected newline or end of document after a statement (at line 1, column 10)
tomllib probe exit=1
```

**Szenario:** Bei einer späteren Manifeständerung wird einer Bytes-Zahl
eine führende Null vorangestellt. Release-Parser und Rücklesen können
weiterhin passieren. Auch der Digest-Gate kann passieren, wenn das Binary
genau diese ungültigen Bytes enthält: `--manifest-sha256` parst absichtlich
keinen Katalog. Ein Release-Build allein führt die grünen Manifest-Unit-Tests
nicht aus. Die App scheitert dann beim eigentlichen TOML-Lesen.

**Vorschlag:** Quellenmanifest und erzeugte `versions.toml` unabhängig
mit einem echten TOML-Parser prüfen, einschließlich dieser Negativfixture.
Die aktuellen Tests zeigen, dass das jetzige Quellenmanifest gültig ist;
sie ersetzen keine Prüfung der tatsächlich erzeugten Release-Metadaten.

## Selbst ausgeführte Gates

Alle drei angeforderten Befehle mit:

```text
TEMP=TMP=D:\DEV\diktier\.herd\review-wp2-sol-2-temp
```

`cargo test`:

```text
test result: ok. 554 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 2.01s
cargo test exit=0
```

`pwsh -File scripts\tests\Test-UltraScripts.ps1`:

```text
Test-UltraScripts: 33 Tests, 33 ok, 0 fehlgeschlagen
Test-UltraScripts exit=0
```

Die Browser-Tests wurden nicht übersprungen. Prozesse, Modelle und
Dateizugriffe der Skript-Tests sind Fakes beziehungsweise isolierte
Testdaten; sie belegen nicht die produktive Config-Auflösung aus F1.

`python scripts\tests\test_quantize_ultra.py`:

```text
Ran 11 tests in 0.185s

OK
test_quantize_ultra exit=0
```

Zusätzlich nur die oben dokumentierten kleinen, isolierten Proben zu
Schreibqueue, Zeitraumfunktion und TOML-Zahl; keine Modell-/Netz-/Release-
Ausführung. Das absichtliche `tomllib`-Negativergebnis ist keine fehlgeschlagene
Projekt-Testsuite.

Die echten Release-, SkipBuild-, Modell- und Integrationsläufe aus dem
WP2e-Bericht sind **dessen Nachweise**, nicht von mir erneut ausgeführte
Gates. Im Nachreview wurden kein Bundle gebaut, keine Assets veröffentlicht
und keine anonymen Download-Adressen geprüft.

## Urteil zu WP3a

**Ja, WP3a kann mit dem vorgesehenen ausdrücklichen Go aus Code-Sicht
beginnen; nein zu einer pauschalen Freigabe aller WP2e-Werkzeuge.**

Für die eigentliche Modellveröffentlichung sehe ich am jetzigen Stand keinen
neuen Blocker: Quellschutz im Quantisierer ist repariert, Modellwahl bleibt
explizit, v3 bleibt Default, und der Release-Abgleich schützt gegen ein
abweichendes eingebettetes Manifest. Das jetzige Manifest wird durch die
ausgeführten Rust-Tests geprüft; F6 betrifft den Schutz bei späteren
Änderungen, nicht einen nachgewiesen ungültigen aktuellen Katalog.

Das Go ersetzt weder die übrigen Schritte aus WP3a noch insbesondere
Immutable-/Asset-Rückleseprüfung und anonymes Transport-Gate. Diese konnten
und sollten in diesem read-only Review nicht ausgeführt werden.

**Vor der verbindlichen Bewertung müssen F1–F5 geschlossen sein.**
Vor produktiver Nutzung der Vergleichsseite insbesondere F2/F3, vor
Kriterium-1-/Zeitraumurteilen F4/F5 und vor dem Leistungsurteil F1.
Offene Auswertungsfehler sind kein Grund, die bereits korrekten
Modellartefakte zu verändern; sie sind aber ein Grund, WP2e nicht als
vollständig abgenommen und spätere positive Kriterien nicht als belastbar
zu behandeln.
