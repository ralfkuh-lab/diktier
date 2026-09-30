# Review: Alltagstest Parakeet Ultra mit Modell-Release

Stand: 2026-09-30. Gegenstand: `docs\ultra-alltagstest-plan.md`, v1,
Codebasis `2d10ee0` / 0.4.1, SPEC v1.9.

**Urteil:** Vor Umsetzung nacharbeiten. Drei Blocker betreffen die
Entscheidungsgrundlage und das Release-Bundle. Der Test kann sonst einen
Default-Wechsel rechtfertigen, ohne einen belastbaren Alltagsvorteil zu belegen.

Nur lesende Untersuchung von Plan, Spike, Spec und relevanten Codepfaden;
ergänzend öffentliche GitHub-Dokumentation und Modellkarten. Keine Tests,
Builds, Modell-/Asset-Downloads, Installation oder Laufzeitänderungen.
Geändert wurde ausschließlich dieser Bericht.

## Blocker

### B1 — Qualitätsentscheidung ist nicht ausreichend vorab festgelegt

**Fundstelle:** Plan, Leitentscheidung 7, WP4/WP5 und Abnahmekriterien 1, 2, 5;
Spike, Abschnitte „Bilanz Ring“ und „Einschätzung“.

**Problem:** Eine Quote ohne Mindestmenge erlaubt einen Wechsel schon nach
zwei Ultra-Siegen gegen einen v3-Sieg; bei null entscheidbaren Unterschieden
ist die Regel nicht eindeutig. „Systematische Fehlerklasse“ und „stört in der
Praxis nicht“ werden erst nach Sichtung ausgelegt. Dauer und Verlängerung sind
nicht fixiert. Das ermöglicht nachträgliche Auswahl günstiger Zeiträume und
Bewertungsmaßstäbe. Ein kleines Rechtschreibplus und eine gravierende
Auslassung zählen außerdem jeweils als ein gewonnenes Diktat.

Bei der späteren Bewertung hat Ralf die Ultra-Ausgabe live bereits gesehen
und kann während des Tests seine Sprechweise anpassen. Das verzerrt vor allem
ein offenes Urteil; zufällige Spalten allein
machen den Versuch deshalb nicht vollständig blind. Ein Wortdiff zeigt
nicht, was tatsächlich gesprochen wurde. Gleiche, aber falsche Ausgaben
fehlen in der geplanten Ansicht ganz. Die Frage „besser“ bleibt so teilweise
eine Präferenz für eine bekannte Ausgabe statt für richtige Erkennung.

**Vorschlag:** Vor dem ersten Testdiktat ein kleines Bewertungsprotokoll
festschreiben:

- Fester Zeitraum und vollständige Aufnahmeliste; Ausschlüsse mit Grund,
  Duplikate und Wiederholungen gesondert ausweisen. Verlängerung nur wegen
  unzureichender Datenmenge, nicht wegen einer knapp verfehlten Quote.
- Als konkrete, noch freizugebende Schwelle: 14 Tage, mindestens 300
  auswertbare Sprachaufnahmen und 50 entscheidbare Paare; `U >= 2 * V`.
  Null entscheidbare Paare oder unterschrittene Mindestmengen bedeuten
  „nicht belegt“, nicht „bestanden“. Unsicherheit der Siegquote mit angeben.
- Pro Aufnahme verdeckte A/B-Zuordnung, gespeicherte Zufallszuordnung und
  Audiozugriff; Urteil vor Auflösung speichern. Ralf darf „unklar“ wählen.
  Eine vorab gezogene Stichprobe gleicher Ausgaben ebenfalls anhören.
- Fehler vorab kategorisieren: Inhalt/Auslassung, Zahl-/Datumswert,
  Wortverschmelzung, reine Schreibweise/Interpunktion. Zahlenformat nicht
  automatisch als Erkennungsgewinn werten. Kritische Inhaltsfehler separat
  zählen; als Klassenregel etwa drei unabhängige Ultra-exklusive Fälle
  derselben Kategorie, zusätzlich eine Einzelfall-Vetoregel für gravierende
  Inhaltsfehler.
- Zahlenschreibweise braucht am Ende ein ausdrückliches Ja; das alternative
  „stört nicht“ streichen.

Die Grenzwerte sind Empfehlungen, keine aus dem kleinen Spike ableitbaren
statistischen Garantien. Die verbleibende Live-Befangenheit im Ergebnis
benennen und den Schluss auf Ralfs Windows-Alltag begrenzen.

### B2 — Historische v3-Latenz ist kein fairer Vergleichspartner

**Fundstelle:** Plan, Abnahmekriterium 4;
`src\daemon\workers.rs:313-327`, `src\main.rs:406-422`;
SPEC §3 und §5.2.

**Problem:** Historische v3-Läufe seit August und neue Ultra-Läufe unterscheiden
sich in Audiodauer, Last, Energiezustand und Softwarestand. Insbesondere kam
die Vorlauf-Stille erst mit 0.4.1 hinzu. Der Median von Zeit/Audiodauer beseitigt
diese Unterschiede nicht. Der Daemon misst außerdem auch erfolgreiche
Gate-Ablehnungen als nahezu null Sekunden „Inferenz“; solche Zeilen dürfen
nicht als Engine-Latenz eingehen. Die Inferenzzeile benutzt `log.info`, nicht
`log.run`, und trägt selbst keine Laufnummer.

Nur der Median kann lange Ausreißer, den 5-s-Watchdog, Modellladeprobleme und
Speichermehrbedarf verdecken. Offline läuft ein ungezählter Warmup, im
Daemonpfad nicht. Ein historisches Live-Median gegen neue Offline-Messungen
wäre daher ebenfalls unfair.

**Vorschlag:** Primäres Leistungsgate auf denselben archivierten WAVs, demselben
0.5.0-Binary, derselben gebündelten ORT-DLL und denselben Threads messen.
Je Modell gleicher Warmup und mehrere serielle Durchgänge mit wechselnder
Modellreihenfolge, Daemon für diese Messung beenden. Nur angenommene
Sprachaufnahmen mit vollständigem Messpaar berücksichtigen; Audio-ID,
Samples und Zeiten maschinenlesbar zuordnen. Mediangrenze von +10 % kann
bleiben. Zusätzlich vorab p95-/Ausreißergrenze, Watchdog-/Fehlergrenze und
Peak-Working-Set-Gate definieren, etwa SPEC-Ziel <= 2 GiB.

Live-Notizen bleiben ein eigener UX-Indikator. Wenn Live-Latenz entscheidend
sein soll, einen kurzen v3-Kontrollblock unter demselben Softwarestand
vorsehen. Die alten Logs höchstens als Kontext verwenden; beide
Rotationsdateien vorab sichern, nicht nur `diktier.log`.

### B3 — Mehrmodell-Manifest bricht den Versionsvertrag des Bundles

**Fundstelle:** Plan, WP2 „Manifest“, „Gates“ und WP3 Schritt 4;
`scripts\release.ps1:111-115,155-159`; SPEC §11.

**Problem:** `release.ps1` ist in WP2 nicht als Änderungsfläche genannt.
Es liest per Regex den ersten `key` und die erste `url`, erzeugt genau
`[model]` und extrahiert Repository/Revision ausschließlich aus
HF-`resolve`-URLs. Mit zwei Modellen wird Ultra im Bundle unterschlagen.
Steht Ultra zuerst, landen bei den beiden HF-Ersetzungen unveränderte
GitHub-URLs in Repository und Revision. Das Skript kann dabei erfolgreich
laufen; ein Release-Build entdeckt den semantischen Fehler nicht.

**Vorschlag:** Release-Skript ausdrücklich in WP2 aufnehmen. Alle
Manifestmodelle strukturiert lesen bzw. aus einem gemeinsamen Export
übernehmen; `versions.toml` um Default und modellbezogene Einträge mit
Quelle, HF-Commit bzw. Release-Tag, Dateinamen, Größen und Hashes erweitern.
Nicht den Default aus der Reihenfolge ableiten und GitHub-Herkunft nicht
durch HF-Regex erraten. Ein eigenes Bundle-Gate muss die erzeugte TOML
einlesen und beide Einträge gegen das eingebettete Manifest prüfen.
Die Angabe „vier Artefakte“ im Skript ebenfalls modellbezogen machen.

## Wichtig

### W1 — Die Belegsammlung ist nicht gegen fehlende oder verlorene WAVs abgesichert

**Fundstelle:** Plan, Leitentscheidung 5, WP3 Schritt 5, WP4 und Risiko „Speicher“;
`src\daemon\debug_wav.rs:49-52,329-371`;
`src\daemon\workers.rs:655-662,690-701`; SPEC §10.

**Problem:** WP3 setzt nur `DIKTIER_DEBUG_WAV_KEEP`; geschrieben wird
weiterhin ausschließlich bei `DIKTIER_DEBUG_WAV=1`. Ein bereits gesetzter
Schalter wird stillschweigend vorausgesetzt. Eine neue Benutzervariable
aktualisiert nicht automatisch die Umgebung der startenden PowerShell oder
eines anderen bestehenden Elternprozesses; Neustart allein garantiert
deshalb den neuen Wert nicht. Das Gate prüft keine tatsächlich erzeugte
f32-WAV oder angewandte Ringgröße.

Kopieren erst am Testende verliert Daten durch Temp-Bereinigung, Überschreiten
der Ringgröße oder Dump-Fehler. Ungültiges KEEP fällt auf zehn zurück; auch
ein zwischenzeitlicher Start mit Default kann den bisherigen großen Ring
beschneiden. Die Behauptung „geht nichts verloren“ berücksichtigt dies
nicht. Ein Verlust bevorzugt unbemerkt die übrig gebliebenen Diktate.

**Vorschlag:** Beide Schalter und ihre vorherigen Werte vor WP3 aufnehmen;
gewünschte Werte ausdrücklich auch in der Umgebung des gestarteten Prozesses
setzen. Startlog nennt nur Aktivierung und effektive Kapazität, keinen Text.
Gate: Probediktat erzeugt f32/16-kHz/mono-Datei, deren Aufnahme-ID im Log
steht; ein isolierter Ringtest belegt die konfigurierte Grenze.

Mindestens täglich und vor jedem Abbruch/neuen Start in das lokale Archiv
kopieren; erfolgreiche Dumps mit Größe/SHA-256 inventarisieren und erwartete
gegen gesicherte Aufnahmen zählen. Fehler/Lücken im Ergebnis ausweisen;
ein Wechsel darf nicht auf einer unbemerkt unvollständigen Teilmenge beruhen.
Speicherbudget nach tatsächlicher Audiodauer statt nur Dateizahl überwachen.
Nach dem Test die vorherigen Umgebungswerte wiederherstellen.

### W2 — Datenschutz ist nur für WAVs, nicht für die neue Textgegenüberstellung definiert

**Fundstelle:** Plan, Leitentscheidung 6, WP2 „Skript“ und WP5;
`.gitignore:24`, SPEC §10, `src\main.rs:253-255`.

**Problem:** Für die Markdown-Gegenüberstellung ist kein zwingender
Ausgabeort festgelegt. Sie enthält vollständige Diktatinhalte, ebenso
temporäre stdout-Dateien und Freitexturteile. WP5 sieht dagegen einen
Bericht im öffentlichen Repo vor. „WAVs nie hochladen“ schützt nicht vor
versehentlich übernommenen Texten oder Notizen. Gitignore schützt nur den
genannten lokalen Baum und ist keine Zugriffsschutz-/Aufbewahrungsregel.

**Vorschlag:** Rohtexte, Gegenüberstellung, Audio, Zuordnungsschlüssel und
Notizen ausschließlich unter `testdata\stt\local\ultra-test\` oder einem
explizit lokalen, nicht synchronisierten Benutzerverzeichnis speichern.
Das Vergleichsskript muss einen sicheren Default haben und darf Inhalte
nicht zusätzlich als Diagnose nach stderr oder in `diktier.log` schreiben.
Markdown-Metazeichen und Zeilenumbrüche als Daten behandeln.

In `docs\reviews\ultra-alltagstest-auswertung.md` und `docs\SPIKES.md`
nur aggregierte Kennzahlen und redaktionell freigegebene, anonymisierte
Befunde; keine automatisch übernommenen Transkriptzeilen, Audios oder
personen-/kundenbezogenen Notizen. Lokale ACLs, Ausschluss von Sync/Upload
und Lösch-/Aufbewahrungsentscheidung für Text und Audio gemeinsam festlegen.
CLI-Textausgabe bleibt ein expliziter lokaler Werkzeugpfad, keine Ausnahme
für Transkripte im Daemonlog.

### W3 — Modellwahl und Rückweg müssen durch den ganzen Daemonpfad verdrahtet werden

**Fundstelle:** Plan, Ausgangslage, WP2 „Download“, WP4 „Abbruch“;
`src\daemon\mod.rs:198-216,240-244,641-648,746-782`;
`src\engine.rs:541-552,656-660`;
`src\download.rs:102-129,263-277`;
`src\single_instance.rs:106-143`.

**Problem:** Außer `config.rs` und `ParakeetTranscriber::load` wählen der
Daemon und `model_artifacts` heute separat das einzige Manifest. Der Daemon
vergleicht anschließend gegen genau diesen Schlüssel und nutzt das Manifest
für Verzeichnis, Download, Engine und Tray. WP2 benennt diese Verbraucher
nicht ausdrücklich; ein isolierter Ultra-Fake-Download belegt ihre gemeinsame
Auswahl nicht.

Die Ausgangslage kann zudem als SHA-Prüfung bei jedem Laden gelesen werden.
Tatsächlich prüft `check_artifacts` nur Existenz/Größe und ignoriert `COMPLETE`.
Eine gleich große beschädigte ONNX-Datei löst daher keinen Reparaturdownload
aus; Laden kann scheitern oder trotz veränderter Daten gelingen. Neustart
allein repariert diese Klasse nicht. Für einen garantiert netzunabhängigen
Rückweg muss v3 vorab vollständig und hashgeprüft vorhanden sein.

**Vorschlag:** Ein gewähltes Manifest vom Config-Schlüssel bis Tray/Worker
durchreichen; auch `model_artifacts(key)` muss dasselbe Modell selektieren.
Unbekannt bleibt Config-Fatal ohne Modell-Fallback. Startprüfung,
Download-Vollprüfung und Rolle des Markers ausdrücklich unterscheiden.
Ein lokaler Vollcheck vor dem Test und eine dokumentierte, gezielte
Reparatur des betroffenen Modellverzeichnisses reichen; keine ungeplante
SHA-Prüfung bei jedem Kaltstart verlangen.

Vor Ultra-Start v3 vollständig hashprüfen und einen v3-Probelauf absolvieren.
Rückweg-Gate mit abgebrochenem Ultra-Download, falscher Größe und falschem
Hash bzw. gleich großer beschädigter Datei: alten Daemon vollständig beenden,
Config auf v3 zurücksetzen, neu starten, ohne Ultra-Netzzugriff diktieren.
v3-Dateien und Config sonst unangetastet lassen.

Die vorhandene gemeinsame Download-Sperre bewusst beibehalten und
Busy/Abbruch/Freigabe sowie getrennte Modellverzeichnisse testen. Sie ist im
Windows-Code ein pfadbasierter `Local\`-Mutex, keine Marker-/Sperrdatei und
keine rechnerweite Sperre über alle interaktiven Sessions. `COMPLETE` weder
als Lock noch als alleinigen Integritätsnachweis verwenden.

### W4 — Draft-Hashprüfung ersetzt nicht den Test der veröffentlichten Produkt-URL

**Fundstelle:** Plan, Leitentscheidung 3, WP3 Schritte 3-5, Risiko „Download von
GitHub“; `src\download.rs:428-459`.

**Problem:** Draft-Assets sind nicht über die anonyme Produkt-URL allgemein
verfügbar. Die vorgeschlagene Download-Gegenprüfung braucht einen benannten
authentifizierten Weg. Eine erfolgreich über `gh`/GitHub-API heruntergeladene
Datei belegt noch nicht den produktiven `ureq`-Transport ohne Token.
Der Plan prüft diesen erst nach Installation und Umschalten von Ralfs
funktionierendem Daemon.

Ändert GitHub URL-Form, Redirect-Ziel oder Redirect-Verhalten, schützt der
Hash zwar vor falschen Bytes, nicht vor Unverfügbarkeit. Ein Release-Asset
ist unveränderlich, aber weder seine Erreichbarkeit noch ein bestimmter
CDN-Hostname sind damit garantiert.

**Vorschlag:** Die Kette ausdrücklich teilen:

1. Reproduktion mit feststehenden erwarteten Hashes; vollständige Assetliste
   einschließlich `vocab.txt`, Notice und Prüfsummendatei.
2. Draft auf einen festgehaltenen Commit mit Rezept/Notice beziehen.
   Uploads abschließen, tatsächliche Assetnamen/Größen und `uploaded`-Status
   prüfen; authentifiziert per Asset-ID/API oder geeignetem `gh`-Aufruf
   zurücklesen und gegen die vorher feststehenden Hashes prüfen.
3. Veröffentlichung mit ausdrücklich `make_latest=false` bzw. `--latest=false`.
   Danach Immutable-Status und publizierte Asset-URLs kontrollieren.
4. Vor Installation/Umschalten frisches isoliertes Modellverzeichnis mit dem
   echten `HttpTransport`, ohne GitHub-Token und ohne Cache, aus den im
   Binary eingebetteten URLs befüllen; Größe/Hash und Laden prüfen.

Canonical GitHub-Release-URLs verwenden, keine kurzlebigen signierten
Redirect-URLs speichern und kein festes `objects.githubusercontent.com`
als Voraussetzung einbauen. Bei späterem URL-Ausfall verständlicher Fehler
mit v3-Rückweg; geänderte Quellen erst nach kontrollierter Manifest-/App-
Aktualisierung, weiterhin dieselbe Hashprüfung. Kein automatischer
„Latest“- oder ungeprüfter Mirror-Fallback.

### W5 — Lizenz-Nennung muss die gelieferten Hinweise der gesamten Herkunftskette erhalten

**Fundstelle:** Plan, Leitentscheidung 3, WP0 „NOTICE.md“, WP2 `LICENSES/`,
Risiko „Lizenz“; `LICENSES\NOTICE-parakeet.md:3-11`;
CC-BY-4.0 §3(a)(1); gepinnte Ultra-ONNX-Modellkarte [S4].

**Problem:** Namen plus Lizenzlink und eigener Quantisierungshinweis sind
keine vollständige Checkliste. CC-BY verlangt auch die Erhaltung gelieferter
Attributions-/Copyright-/Lizenz-/Haftungshinweise und Hinweise auf vorherige
Änderungen. Der ONNX-Export nennt ausdrücklich die Konvertierung und das
Weglassen des VAD-Kopfs; bloß „modifiziert (int8 per-channel)“ beschreibt die
Änderungskette nicht vollständig. Die konkrete Notice existiert noch nicht,
ihre Vollständigkeit ist daher nicht geprüft.

**Vorschlag:** In WP0 als Veröffentlichungsgate die Hinweise der gepinnten
Quelle und der verlinkten Ursprungsmodelle prüfen und, soweit geliefert,
erhalten. NVIDIA als Ursprung, Moondream als Post-Training, altunenes als
ONNX-Export und eigene Quantisierung mit Modell-/Quelllinks benennen;
fehlenden VAD-Kopf und frühere Konvertierung nennen. Lizenztext oder
gültigen Lizenzlink sowie Herkunftsrevision und Rezept zuordnen.
Dieselbe Ultra-Notice im App-Bundle ausliefern und die v3-Notice behalten.
Releasebeschreibung auf die Notice verweisen lassen; Attribution nicht
ausschließlich in editierbaren Release Notes ablegen.

### W6 — Repo-weite Immutability braucht einen vollständigen App-Release-Workflow

**Fundstelle:** Plan, WP3 Schritte 1-3 und Risiko „Immutable ist endgültig“;
GitHub-Dokumentation [S1], [S2].

**Problem:** Die Folge „fehlerhafte Datei nur durch neue Version“ ist zu
schmal. Nach Veröffentlichung dürfen auch keine weiteren Assets angehängt
werden; Zip, Installer, Lizenzen und Prüfsummen müssen bei künftigen
App-Releases vorher vollständig im Draft liegen. Der Tag ist an den Commit
gebunden und kann während des Releases weder verschoben noch gelöscht werden.
Die WP3-Reihenfolge lässt optionale App-Releases bereits vor dem Einschalten
entstehen; diese würden nicht nachträglich immutable.

„Immutable ist endgültig“ bzw. „Release bleibt bestehen“ ist auch keine
Verfügbarkeitsgarantie: GitHub erlaubt das Löschen des ganzen Releases.
Der geschützte Tagname darf danach nicht wiederverwendet werden.
Titel/Release Notes bleiben editierbar.

**Vorschlag:** Bewusste Zustimmung zur Repo-weiten Wirkung getrennt von
Modellveröffentlichung einholen. Vor optionalen App-Veröffentlichungen
einschalten, sofern diese ebenfalls immutable sein sollen. Den Ablauf
„bauen -> alle Assets/Prüfsummen prüfen -> Draft befüllen -> veröffentlichen“
für App-Releases dokumentieren. Bestehende Releases ausdrücklich
ausnehmen, korrigierte Modelle mit neuem Tag veröffentlichen und die
Bedeutung von Immutability auf Bytes/Tag statt dauerhafte Verfügbarkeit
begrenzen. Ein separates Modellrepo nur dann erwägen, wenn Ralf die
App-Release-Regel bewusst nicht übernehmen will.

### W7 — Vergleichs-CLI ist für die geplante Datenmenge noch nicht spezifiziert

**Fundstelle:** Plan, Leitentscheidung 6 und WP2 „CLI“/„Skript“;
`src\main.rs:60-71,262-267,353-425`.

**Problem:** Bis zu 2000 absolute WAV-Pfade als Argumente passen nicht
zuverlässig in Windows' Prozess-Kommandozeile von maximal 32.767 Zeichen.
„Alle Dateien, Modell einmal laden“ ist mit ausschließlich variadischen
Argumenten deshalb nicht durchgehend erfüllbar.

Außerdem fehlen eindeutige Verträge für Einzeldatei mit `--model`, `--runs`,
Gate-Ablehnung, Fehler mitten im Batch sowie die Zuordnung der stderr-
Gate-/Zeitzeilen. Leertext nach Gate-Ablehnung darf nicht als fehlgeschlagene
Datei interpretiert werden; Prozessfehler darf das Skript nicht zu einer
erfolgreichen Teilgegenüberstellung machen. TSV ist ohne Escape-Regel für
Tabs/Zeilenumbrüche im Transkript kein belastbares Maschinenformat.

**Vorschlag:** Dateiliste oder Ordner als zusätzlichen Eingabepfad festlegen,
damit beliebig viele WAVs mit einmaligem Modellladen verarbeitet werden.
`--model` nur zusammen mit Transkriptionsmodus zulassen; Config auf Platte
nie umschreiben. Ausgabeformat für jeden Aufruf eindeutig festlegen,
Legacy-Einzeldatei ohne Override erhalten.

Ein stabiles lokales Maschinenformat bzw. exakt spezifiziertes TSV-Escaping
mit Datei-ID und getrennten, textfreien Diagnose-/Messdaten verwenden.
Gate-rejected, erfolgreich leer und Fehler unterscheiden; Modell/Warmup
nur laden, wenn mindestens eine Aufnahme den Gate passiert. Bei Batchfehler
nicht null zurückgeben; Vergleichsskript kontrolliert Exitcodes, vollständige
Paare, UTF-8, Dateireihenfolge und Markdown-Escaping. `--runs` gibt weiter
einen Text pro Datei, aber eindeutig zuordenbare Messwiederholungen aus.
Parser-, Batch- und Skripttests dafür in WP2 aufnehmen.

### W8 — Ein grünes v3-smoke-Gate beweist noch keine Produktfähigkeit von Ultra

**Fundstelle:** Plan, WP2 „Tests“/„Gates“ und WP3 Schritt 5;
`src\engine.rs:1577-1580,1605-1619`;
`src\download.rs:470-503`;
`src\tray.rs:1029-1213`.

**Problem:** `stt_smoke_fixtures` ist ignoriert und ausdrücklich auf
`DEFAULT_MODEL` verdrahtet; normales `cargo test` deckt es nicht ab.
WP2 fordert nur das unveränderte v3-Gate. Der Spike benutzte ein separates
Werkzeug, nicht die neue Mehrmodell-Produktverdrahtung. Ein einziges
Ultra-Probediktat nach Veröffentlichung deckt Gate-Ablehnung, f32-Replay,
Warmup/Batch, Fehlerzustände und das fehlende `config.json` nicht ab.
Auch der geplante Ultra-Fake-Transport-Test darf keine hunderte MB großen
Realdateien voraussetzen.

**Vorschlag:** v3-Golden-Set-Test und v3-smoke ausdrücklich beim bisherigen
Schlüssel halten; `model_artifacts` modellbezogen machen, nicht die
v3-Referenzen durch Ultra-Ausgaben ersetzen. Zusätzlich vor dem Push ein
Ultra-Integrationsgate mit reproduzierten lokalen Artefakten, gebündelter
ORT 1.28.0, Produktionsthreads, f32-WAV, Sprache/Stille/Rauschen und
300-ms-Vorlauf durchführen. Zahlen erwartungsgemäß getrennt bewerten.

Manifesttests prüfen beide vollständigen Dateisätze, eindeutige Schlüssel
und getrennte sichere Verzeichnisnamen; Golden-v3-Werte einschließlich URLs
bleiben unverändert. Downloader mit kleinen Drei-Dateien-Fakes und
separatem Test des echten Ultra-Manifests prüfen. Parametrisierte
Daemon-/Tray-Proben belegen für beide Modelle Auswahl sowie
`downloading -> loading -> idle` und Fehler ohne scharfen Hotkey oder
heimlichen v3-Fallback. Das spätere öffentliche Transportgate aus W4
bleibt ein eigener Schritt.

## Hinweis

### H1 — WP3 verlangt eine Logzeile, die es heute nicht gibt

**Fundstelle:** Plan, WP3 Schritt 5;
`src\download.rs:413-420`, `src\daemon\workers.rs:430-436`.

**Problem:** `COMPLETE` wird als Datei geschrieben, nicht als solche geloggt.
Die Erfolgsmeldung lautet „Modellartefakte vollständig und geprüft“.
„Das Log zeigt ... COMPLETE“ ist daher ohne zusätzliche Änderung kein
ausführbares Gate.

**Vorschlag:** Gate auf vorhandene Erfolgsmeldung, korrekten Markerinhalt
im ausgewählten Verzeichnis und anschließende Modell-Ladezeile formulieren.
Den Marker nicht mit der Startprüfung oder SHA-Vollprüfung verwechseln.

### H2 — Reproduktionsrezept muss den gesamten Release-Dateisatz benennen

**Fundstelle:** Plan, Leitentscheidung 4 und WP0 „drei Eingaben“;
Spike, Tabelle „Modelle“; gepinnte ONNX-Modellkarte [S4].

**Problem:** Die drei Quantisierungseingaben sind Encoder-ONNX, dessen
External-Data-Datei und Decoder-ONNX. Zusätzlich braucht das Release
`vocab.txt`; dessen gepinnter Download und Eingangsprüfung sind in WP0
nicht ausdrücklich beschrieben. Zwei Paketpins legen auch Python und
transitive Abhängigkeiten nicht vollständig fest, was spätere
Reproduktionsfehler schwerer diagnostizierbar macht.

**Vorschlag:** Alle vier Quellpfade mit Revision/Größe/SHA-256 explizit
aufführen, External Data vor Quantisierung korrekt daneben ablegen und
`vocab.txt` unverändert übernehmen. Alle drei Laufzeitartefakte gegen
feststehende Ausgabegrößen/Hashes prüfen. Python-/Umgebungsstand und
aufgelöste Pakete protokollieren bzw. mit Lock reproduzierbar machen.
Bei Hashabweichung stoppen, nicht neue Sollhashes passend zum Ergebnis
erfinden.

### H3 — WP2 und WP3 verbinden zu viele unabhängig abnehmbare Änderungen

**Fundstelle:** Plan, WP2, WP3 und F4.

**Problem:** Manifest-/Daemon-Umbau, Batch-CLI, Auswertungsskript, Ring,
Lizenzen und Packaging haben zusammen nur ein abschließendes Gate.
WP3 mischt historische App-Releases, Modellveröffentlichung, Installation
und private Testkonfiguration. Ein Fehler spät im Ablauf erschwert die
Zuordnung und blockiert unnötig unabhängige Teile.

**Vorschlag:** WP2 in kleine abnehmbare Teilpakete schneiden:
Mehrmodell-Vertrag einschließlich Bundle und v3-Rückweg; dann
Sammlung/Archivierung; dann CLI/Vergleich. WP3 getrennt in Draft-Prüfung,
Veröffentlichung mit anonymem Transportgate und erst danach lokale
Installation/Umschaltung. Historische App-Releases sind für den
Alltagstest nicht erforderlich und gehören nicht in dessen kritischen Pfad.
WP0 kann als lokales Artefakt-Vorbereitungspaket vor dem Spec-Nachtrag
bleiben; Produktänderungen erst nach WP1.

## Empfehlungen zu F1-F5

| Entscheidung | Empfehlung | Begründung |
|---|---|---|
| **F1: Schlüssel** | `parakeet-ultra-0.6b-int8-pc` übernehmen, aber einmaliger Artefaktvertrag pro Schlüssel. | Name trennt Testmodell und v3-Verzeichnis. `r1` steht im Release-Tag; sobald Bytes/Hashes eines späteren Rezepts oder Exports wechseln, neuen Modellschlüssel oder explizite versionierte Artefaktidentität vergeben. Nicht denselben Schlüssel still mit anderen Dateien belegen. |
| **F2: Dauer** | Zwei feste Wochen; Mindestmenge wie B1 vorab freigeben. | Eine Woche kann überwiegend dieselben Tätigkeiten/Wörter abdecken. Bei zu wenig Daten einmal vorher festgelegte Verlängerung, etwa sieben Tage; weiter unzureichend bedeutet „nicht belegt“. Nicht verlängern, bis die Siegquote passt. |
| **F3: blind/offen** | Verdecktes A/B pro Aufnahme, Urteil nach Audio vor Auflösung. | Mindert Präferenz für das gerade live benutzte Ultra und Auswahlbias; gespeicherte Zuordnung hält die Auswertung reproduzierbar. Ziffern können das Modell verraten, daher nur teilweise blind nennen. Live-Notizen getrennt als UX-Belege behandeln. |
| **F4: Push/App-Releases** | `main` nach lokalen Gates und explizitem Go pushen; v0.4.0/v0.4.1 nicht nebenbei nachveröffentlichen. | Quellstand mit Rezept, Spec, Notices und eingebettetem Manifest muss vor Modellveröffentlichung nachvollziehbar sein. Historische App-Releases sind unnötige Außenwirkung. Wenn gewünscht, getrennt auf exakte alte Commits bauen und mit komplettem Draft-Workflow veröffentlichen; nicht aktuelle 0.5.0-Binaries unter alte Tags hängen. |
| **F5: Kriterien** | Nicht unverändert übernehmen: B1/B2 ersetzen, Sammlung/Vollständigkeit und Betriebsstabilität ergänzen. | Siegquote braucht Mindestmengen/Schwereklassen, Latenz gepaarte Messungen, Zahlenformat ausdrückliche Akzeptanz. „Herr Präsident“ anhand vollständiger gleicher Aufnahmen und Audio verifizieren, gesprochene Phrase ausschließen; 0 gegen 0 belegt nur keine beobachtete Verschlechterung. Keine neue systematische Regression, keine Ultra-exklusiven fatalen Betriebsfehler, nachgewiesener v3-Rückweg und messbares Speicher-/Ausreißerbudget als zusätzliche Gates. |

## Quellen für externe Aussagen

- **[S1]** GitHub, [Immutable releases](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases):
  Asset-/Tag-Schutz, weiterhin editierbare Metadaten, Löschung und
  ausgeschlossene Wiederverwendung des Tags.
- **[S2]** GitHub, [Preventing changes to your releases](https://docs.github.com/en/code-security/how-tos/secure-your-supply-chain/establish-provenance-and-integrity/prevent-release-changes):
  Repo-Einstellung betrifft nur zukünftige Releases.
- **[S3]** GitHub, [Release assets API](https://docs.github.com/en/rest/releases/assets)
  und [Managing releases](https://docs.github.com/en/repositories/releasing-projects-on-github/managing-releases-in-a-repository):
  Asset-ID, Download per API mit 200/302, Assetstatus und Draft-Workflow.
- **[S4]** [Ultra-ONNX-Modellkarte an der gepinnten Revision](https://huggingface.co/altunenes/parakeet-rs/blob/4d2a8bc71f5c896ec40faa59732e6716295edaf2/parakeet-ultra/README.md):
  Dateisatz, Herkunft, ONNX-Konvertierung, fehlender VAD-Kopf und Lizenz.
- **[S5]** [CC-BY-4.0, Legal Code §3(a)](https://creativecommons.org/licenses/by/4.0/legalcode.en):
  Attribution, Erhaltung gelieferter Hinweise und bisheriger Änderungen.
