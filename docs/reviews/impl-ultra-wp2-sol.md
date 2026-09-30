# Review WP2a–WP2c — GPT-6.1 Sol

Stand: 2026-09-30, Working Tree 0.5.0 gegen `HEAD 2d10ee0`.
Auftrag: `docs\reviews\impl-ultra-wp2-review-prompt.md`.
Grundlagen: SPEC v1.10, die angegebenen Abschnitte des Alltagstest-Plans und
die Berichte WP0, WP2a, WP2b, WP2c und WP2d.

**Ergebnis:** Modellwahl und Rust-Grundpfade sind schlüssig; die ausgeführten
Rust-Gates sind grün. Noch keine Freigabe für den verbindlichen Alltagstest:
ein kritischer Befund bei der Quellverzeichnis-Garantie, neun wichtige
Befunde bei Datenerhalt, Datenschutz, Auswertung und Release-Nachweis.
Die Befunde sind statische, konkrete Codepfad-Nachweise; die Fehlerszenarien
wurden wegen des eingeschränkten Ausführungsauftrags nicht separat ausgeführt.
Keine Implementierung geändert, kein Commit, keine Weiterdelegation.

## Befunde — kritisch

### K1 — Quantisierungs-Cleanup kann das ausdrücklich nur lesbare Quellverzeichnis löschen

**Stellen:** `scripts\quantize-ultra.py:291–310,334–339`.

Der Überlappungsschutz verbietet nur `out == source` und ein Ziel *unterhalb*
der Quelle. Er erlaubt dagegen beispielsweise
`--source D:\scratch\export\.quantize-tmp\src --out D:\scratch\export`.
Damit liegt die Quelle unter dem Verzeichnis, das Zeile 310 vor der
Staging-Kopie rekursiv löscht. Noch problematischer: Scheitert bereits
`log_environment()`, entfernt das `finally` in Zeile 337 trotzdem das
vorhandene `staged`, obwohl dieser Lauf es gar nicht angelegt hat. Bei dem
Beispiel ist `staged` die Quelle selbst.

**Folge:** Verlust der vier Quelldateien, nicht bloß ein fehlgeschlagener
Export. Die Garantie „Das Quellverzeichnis wird nur gelesen“ gilt nicht für
alle akzeptierten Aufrufe.

**Vorschlag:** Vor jedem Seiteneffekt die aufgelösten Quell-, Arbeits- und
Staging-Pfade auf diese Überlappung prüfen. Cleanup ausschließlich für
Verzeichnisse, die der aktuelle Lauf selbst erfolgreich angelegt hat;
vorhandene Arbeitsverzeichnisse nicht ungeprüft entfernen. Negativtests
für Quelle gleich/unter `.quantize-tmp`, insbesondere für einen Fehler
vor dem Anlegen des Arbeitsverzeichnisses.

## Befunde — wichtig

### W1 — Ein frei gewähltes Debug-Verzeichnis verliert eine fremde `last_recording.wav`

**Stellen:** `src\daemon\debug_wav.rs:167–177,359–363,459–495`.

Nach einem erfolgreichen Dump wird unabhängig von `keep` immer
`dir\last_recording.wav` entfernt. Seit `_DIR` konfigurierbar ist, kann `dir`
ein bereits benutztes Aufnahmeverzeichnis sein. Dort ist diese Datei nicht
automatisch die Diktier-Altlast aus `%TEMP%\diktier`. Auch mit Kapazität 5000
und nur einer neuen Ringdatei wird sie gelöscht; die Kapazitätsprüfung in
`prune` schützt diesen nachfolgenden Löschaufruf nicht.

**Vorschlag:** Die Legacy-Bereinigung auf den historischen Default-Ordner
beschränken oder Eigentümerschaft nachweisen. Test: frei gewählter Ordner mit
fremder `last_recording.wav`, jungen/alten fremden `.part`-Dateien und
Kapazität 5000; die fremde Legacy-Datei muss bleiben. Die Löschlogik für
Dateien außerhalb des exakten Ring-/Part-Musters ist ansonsten eng begrenzt.

### W2 — Der zugesicherte Ablageort wird beim Export und bei späteren Skriptaufrufen nicht eingehalten

**Stellen:** `scripts\compare-models.html:80–89,191–193,259–280`;
`scripts\compare-models.ps1:456–463,649–692`;
`scripts\bench-models.ps1:68–77,89–91`.

Eine Notiz wird sofort mit dem Zustand in Browser-`localStorage` geschrieben,
also außerhalb des Auswertungsordners. Der Export erlaubt ein beliebiges
Picker-Ziel und fällt bei fehlendem/fehlgeschlagenem Picker auf einen
Browser-Download zurück. Bei normalem Download-Ziel steht die Notiz dann
bereits in `Downloads`; „bitte verschieben“ macht die erste Kopie nicht
rückgängig. Zudem akzeptieren `-Resolve` und `-Evaluation` jeden existierenden
Ordner. Eine ins Repo kopierte Vorbereitung kann dort einen
transkripthaltigen `bericht.md` beziehungsweise neue Roh-JSONL erzeugen.
Die nachträgliche Warnung für externe `-Judgments` ist keine Sperre.

**Vorschlag:** Native Schreibziele vor dem ersten Schreiben gegen die
erlaubte Auswertungswurzel prüfen, mit ausdrücklich getrenntem Testmodus für
isolierte Wurzeln. Kein automatischer Downloads-Fallback für Notizen;
Zwischenstände in einer bewusst gewählten Auswertungsdatei statt im
Browserprofil speichern. Falls Browserablage ausdrücklich gewollt bleibt,
ist das eine genehmigungspflichtige Abweichung vom Datenschutzvertrag,
nicht nur ein README-Hinweis. Die regulär erzeugte `zusammenfassung.md`
enthält dagegen statisch keine Transkripte oder Notizen.

### W3 — Warmup-Fehler sind im JSONL unsichtbar und können im Benchmark als bestanden gelten

**Stellen:** `src\transcribe_list.rs:187–192,216–229,573–601`;
`scripts\compare-models.ps1:279–285`;
`scripts\bench-models.ps1:94–108,135–149`.

Ein einmaliger Engine-Fehler beim Warmup setzt Exit 1, lässt die Engine aber
geladen und schreibt bei danach erfolgreicher Messung ausschließlich
`status: text`. Der bestehende Test `failed_warmup_exits_1` bestätigt genau
diese Kombination. `compare-models.ps1` verwirft sie als „Exit 1 ohne Datei
mit error“, sodass die Vorbereitung trotz vollständiger JSONL nicht
verarbeitbar ist. `bench-models.ps1` akzeptiert Exit 1 dagegen ohne diese
Gegenprüfung und zählt ausschließlich `error`-Zeilen: Ein Ultra-exklusiver
Warmup-Fehler kann mit null Fehlern und `Kriterium 4: erfüllt` enden.

**Vorschlag:** Einen gescheiterten Warmup für die betroffene Datei
maschinenlesbar als `error` ausweisen, unter Beibehaltung der vereinbarten
Zeilenzahl; spätere Dateien dürfen weiterlaufen. Beide Skripte müssen
Exitcode und Fehlerzustände konsistent validieren. Gemeinsamer Regressionstest:
erstes Engine-Ergebnis Fehler, alle folgenden erfolgreich; weder
Vorbereitung noch Benchmark dürfen daraus einen fehlerfreien Lauf machen.

### W4 — Inventur verschmilzt Sitzungen nach `--foreground`-Neustarts

**Stellen:** `scripts\compare-models.ps1:120–139,144–176`;
`src\daemon\mod.rs:222–230`.

`Read-DaemonLog` erkennt nur Startzeilen mit `(Daemon`. Der echte Daemon
schreibt bei `--foreground` jedoch `(--foreground, Modell …)`. Startet
nach einer normalen Sitzung eine Foreground-Sitzung und beginnt deren
Laufnummer wieder bei 1, werden beide Läufe auf denselben Schlüssel aus
altem Startzeitpunkt und Laufnummer abgebildet. Die Hashtables überschreiben
die Einträge. Zwei erwartete Aufnahmen können als eine gezählt werden;
eine fehlende Aufnahme kann durch die Datei der anderen Sitzung verdeckt sein.

**Vorschlag:** Beide tatsächlichen Startformate erkennen und Sitzung
ereignisbezogen zuordnen. Bei fehlender Startzeile infolge Rotation
Mehrdeutigkeit ausweisen, statt alle unbekannten Sitzungen unter `?|N`
zusammenzuführen. Tests mit zwei Foreground-Neustarts, gemischten
Daemon-/Foreground-Starts und abgeschnittenem Loganfang. An den Zeitraumgrenzen
zusätzlich beachten: Die WAV trägt das Aufnahmeende, die Gate-Zeile kommt
erst nach der Inferenz (`workers.rs:321–327`); diese Zeiten sind nicht identisch.

### W5 — Kriterium 2 wird aus „schlechter“ statt aus nachgewiesen „exklusiv und unabhängig“ berechnet

**Stellen:** `scripts\compare-models.html:62,175–196`;
`scripts\compare-models.ps1:519–547,570–575,620`.

Gespeichert wird nur die Kategorie der schlechteren Seite. Jede Niederlage
wird anschließend als exklusiver Fehler dieses Modells behandelt. Das reicht
für die vorgegebene Exklusivität nicht: Sind beide Texte in K1 fehlerhaft,
Ultra aber stärker, kann ein gravierendes K1-Urteil ein „Ultra-exklusives“
Veto erzeugen, obwohl v3 ebenfalls den betreffenden Fehler hat.
Gemeinsame K1–K3-Fehler aus der Stichprobe werden ausschließlich in `$sErr`
gezählt und gehen nicht in die Frage ein, ob v3 diese Kategorie irgendwo hat.
Auch Unabhängigkeit wird nicht gespeichert; byteverschiedene Wiederholungen
desselben Falls zählen automatisch bis zur Dreiergrenze. Der Satz „prüft
Ralf im lokalen Bericht“ korrigiert das bereits definitive `$k2` nicht.

**Vorschlag:** „Exklusiv“ und unabhängige Fallgruppen vor der Auflösung
nachvollziehbar erfassen beziehungsweise bis zur entsprechenden Prüfung
Kriterium 2 offen lassen. Gemeinsame Fehler gesondert berücksichtigen.
Das braucht eine ausdrücklich festgelegte Präzisierung des
Bewertungsprotokolls: Die bisherige einzelne Kategorie der schlechteren
Seite liefert diese Information grundsätzlich nicht. Tests für gemeinsame
Fehler, gemeinsamen Stichprobenfehler, wiederholten Fall und ein wirklich
exklusives gravierendes K1-Veto.

### W6 — „Ich“ vor einem Befehl fehlt vollständig im Halluzinationsnachweis

**Stellen:** `scripts\compare-models.ps1:107,220–222,305–308,550–564`;
`scripts\compare-models.html:68–70`.

Der Plan verlangt neben „Herr Präsident“ ausdrücklich dieselbe Prüfung
für ein ungesprochenes „Ich“ vor einem Befehl. Vorbereitung, Urteilsformular
und Kriterium 3 unterstützen nur „Herr Präsident“. Sind etwa alle
Herr-Präsident-Zähler null, Ultra erzeugt aber mehrfach ein zusätzliches
„Ich“, meldet das Skript trotzdem Kriterium 3 als erfüllt.

**Vorschlag:** Beide vorab benannten Halluzinationstypen als getrennte,
verdeckte Audiofragen und Zähler abbilden. Ein tatsächlich gesprochenes
„Ich“ darf nicht als Halluzination zählen; Kriterium 3 muss beide Ergebnisse
berücksichtigen. Den bisher engen Typ nicht still zu einer allgemeinen
StartsWith-„Ich“-Strafe umdeuten.

### W7 — Auswertung kann vor Ablauf und mit Aufnahmen außerhalb des Zeitraums positiv ausfallen

**Stellen:** `scripts\compare-models.ps1:77–81,162–182,239–240,378–382,536–547`.

`From`/`To` sind optional, werden nicht auf Reihenfolge, abgeschlossenen
Zeitraum oder die festgelegten sieben Tage geprüft. `Resolve` entscheidet
allein anhand der Mengen und Siegquote. Bei häufigem Diktieren können 300
Paare/50 Entscheidungen schon am ersten Tag ein „erfüllt“ erzeugen, obwohl
vor Ablauf ausdrücklich nicht ausgewertet werden darf. Eine einmalige
Verlängerung nur wegen fehlender Mindestmengen ist ebenfalls nicht abgebildet.
Unabhängig davon werden WAVs ohne passendes Namensmuster stets in die Liste
aufgenommen, selbst mit `From`/`To`: Eine alte, umbenannte Aufnahme kann
dadurch die Mindestmenge und Quote des Testzeitraums verändern.

**Vorschlag:** Verbindlichen Start und Abschluss/zulässige Verlängerung
in der Vorbereitung festhalten und vor der Bewertung prüfen. Explorative
Aufrufe dürfen existieren, aber keine verbindliche Abnahme behaupten.
Zeitlich nicht zuordenbare WAVs nur als Inventurproblem/Ausschluss
ausweisen, nicht ungeprüft zur Grundgesamtheit zählen. Tests mit
kurzem/laufendem/verkehrtem Zeitraum und fremden Namen außerhalb der Periode.

### W8 — Kriterium 4 kann mit unvollständigem Messprotokoll als erfüllt ausgegeben werden

**Stellen:** `scripts\bench-models.ps1:49–50,84–96,123–149,151–165`;
`scripts\ultra-test-lib.ps1:112–118`.

Ein Aufruf mit `-Passes 1 -Runs 1` kann denselben verbindlichen
„Kriterium 4: erfüllt“-Text liefern wie das festgelegte Drei-Durchgang-Protokoll.
Es gibt keine Prüfung des geforderten 0.5.0-Binaries und der gebündelten
ORT-Version; `ProductVersion` wird nur protokolliert. Außerdem werden fehlende
Peak-Messungen kommentarlos herausgefiltert. Scheitert `GetProcessMemoryInfo`
in einem Durchgang und sind die übrigen Peaks klein, kann das Speicher-Gate
trotz unbewerteten Durchgangs bestehen.

**Vorschlag:** Abnahme und kurze Funktionsprobe unterscheiden; nur unter
dem festgelegten Protokoll ein Kriterium-4-Urteil ausgeben. Binary-/ORT-Stand
und effektive Threadkonfiguration als Messprovenienz prüfen/festhalten.
Jeder Prozess braucht eine erfolgreiche Peak-Messung, sonst „nicht belegt“
mit Fehlerursache. Die Fehlerauswertung aus W3 gehört zusätzlich dazu.
Median, Nearest-Rank-p95, Audio-Normierung und die wechselnde
Modellreihenfolge sind für gültige vollständige Messdaten statisch korrekt.

### W9 — Bundle-Gate prüft die Quellen, nicht das tatsächlich eingebettete Manifest

**Stellen:** `scripts\release.ps1:323–333,352–359,418–424`.

Mit `-SkipBuild` kann ein altes beziehungsweise anders gebautes Binary
gebündelt werden. `versions.toml` wird aus dem aktuellen `src\models.toml`
erzeugt und gegen denselben aktuell geparsten Katalog geprüft; der Gate ist
dadurch auch bei abweichendem Manifest in `diktier.exe` grün.
Beispiel: vorhandenes 0.4.1-Binary, Quellen schon 0.5.0 mit Ultra,
`-SkipBuild -SkipInstaller`. Die Metadaten beschreiben beide Modelle und
0.5.0, ohne die behauptete Übereinstimmung mit dem eingebauten Manifest
nachzuweisen. Das ist eine Grenze des neu zugesicherten Bundle-Gates;
die alte SkipBuild-Option allein ist nicht der neue Befund.

**Vorschlag:** Das Binary ohne Modellladen seine Version und einen Digest
beziehungsweise den Katalog des eingebetteten Manifests ausgeben lassen
und dagegen prüfen, oder SkipBuild an überprüfte Build-Provenienz binden.
Ein Versionsvergleich allein reicht für zwei unterschiedliche Builds
derselben Version nicht. Pflichtdateien einschließlich beider NOTICEs
sind ansonsten aufgenommen.

## Befunde — Kleinigkeit

### L1 — Autostart umgeht die neue `--model`-Validierung

**Stellen:** `src\main.rs:271–298`.

`--install-autostart --model nope` beziehungsweise
`--remove-autostart --model nope` wird von Clap angenommen und führt die
Autostart-Aktion vor der Modus-/Schlüsselprüfung aus. Statt des geforderten
Bedienfehlers wird der neue Schalter ignoriert; auch `--runs` erreicht
diese Prüfung nicht. **Vorschlag:** Modusabhängige Optionen vor sämtlichen
Aktionen validieren oder direkt als Clap-Anforderung ausdrücken.
Parser-Test ohne echte Autostart-Aktion.

### L2 — TOML-/Manifestprüfung des Release-Skripts ist weniger strikt als die Rust-Prüfung

**Stellen:** `scripts\release.ps1:104–149,176–207`;
`src\download.rs:101–132,244–264`.

PowerShell-Vergleiche, Regex und `switch` sind hier standardmäßig
case-insensitive. Beispielsweise akzeptiert das Skript eine passend
umgeschriebene, großgeschriebene URL beziehungsweise `source =
"GitHub-Release"`, während Rust exakte URLs und den kleingeschriebenen
Serde-Enumwert verlangt. Hinzu kommen fehlende Gegenstücke zur Rust-Prüfung
für `COMPLETE`/`.part`-Dateinamen. Das aktuelle Manifest ist korrekt;
die Unterschiede betreffen den behaupteten Fehlerschutz bei späteren
Änderungen. **Vorschlag:** Case-sensitive Prüfung und gemeinsame
Negativfixtures; eine echte TOML-Gegenprüfung statt ausschließlich
Rücklesen mit demselben Teilmengenparser.

### L3 — Fehlgeschlagene Speicherung von Urteilen bleibt unsichtbar

**Stellen:** `scripts\compare-models.html:80–90,285–290`.

Bei gesperrtem/vollem `localStorage` tut die Seite weiter so, als wäre der
Zwischenstand gespeichert. Nach Schließen/Neuladen fehlen die Urteile;
auch beschädigte gespeicherte Daten werden still durch einen leeren Stand
ersetzt. **Vorschlag:** Sichtbare Speicherwarnung und Exportaufforderung,
keine stille Erfolgsdarstellung. Bei der Behebung von W2 den Fehlerpfad
des ersetzenden Dateispeichers ebenso explizit behandeln.

## Statisch geprüft — übrige Bereiche

| Bereich | Ergebnis |
|---|---|
| Auswahl/Daemon/Worker/Engine/Tray | Ein `SelectedModel` in `run_locked`; Startprüfung, Download und Engine bekommen diesen Wert beziehungsweise seinen Clone, Tray dessen Schlüssel. Kein stiller v3-Fallback gefunden. Wiederladen nach Watchdog benutzt dieselbe Auswahl. |
| CLI-Modellauswahl | Override wird in beiden Transkriptionsmodi benutzt; Batch wählt/lädt erst beim ersten angenommenen WAV und höchstens einmal. Der normale Einzel-WAV-Pfad ohne Override bleibt in Verhalten und Textausgabe erhalten. Das Modell selbst wird nicht in die Config zurückgeschrieben. `config::load()` kann wie vorher eine fehlende Default-Config anlegen; die pauschale Formulierung „nur lesen, nie schreiben“ im Implementiererbericht ist insofern zu weit. |
| v3/Ultra-Artefakte | v3-Dateinamen, Größen, Hashes und URL-Strings im Diff unverändert; nicht die TOML-Serialisierung selbst, deren Tabellenstruktur geändert wurde. Ultra: drei Dateien, feste Größen/Hashes genau wie SPEC §6.3, kein `config.json`. |
| Manifest-URLs | Exakte Ableitung aus strukturierter Herkunft verhindert CDN-/Redirect-Ziele und `releases/latest/download`; HF braucht einen vollen Commit. Tatsächlichen GitHub-Immutable-Status kann der lokale Parser nicht beweisen; das bleibt WP3a. Keine Ersatzquelle. |
| Download | Gemeinsamer per-user Lock vor jedem Download, getrennte Modellpfade. Größe und SHA vor Rename, Abschlussmarker zuletzt, korrekte vorhandene Dateien werden wiederverwendet. Fehler-/Abbruchpfade löschen nur die betreffende Part-Datei; andere Modellverzeichnisse werden nicht gewählt. Startprüfung nur Existenz/Größe entspricht jetzt ausdrücklich der Spec. |
| Config | Unbekannter **Modellwert** fatal mit beiden erlaubten Werten, Fehler-Tray ohne Hotkey und ohne vorgetäuschtes v3. Vorlage/Default korrekt. Unbekannte Config-**Feldnamen** bleiben nach §8 Warnung statt fatal. Lokaler v3-Rückweg ist im Code nicht vom Ultra-Verzeichnis oder Netz abhängig. |
| Belegsammlung | Die drei Diktier-Variablen werden einmal im Daemonstart gelesen und als Konfiguration durchgereicht. 1–5000, Defaults, Warnungen und An-/Aus-Startzeilen stimmen; keine Transkriptinhalte. Ringmuster/Datum/Part-Alter sowie Rename ohne Ersetzen bleiben erhalten; Ausnahme W1. |
| JSONL | `serde_json` übernimmt Escaping; `run` 1…n nur bei explizitem `--runs`, n Zeilen auch für Ablehnungen/Fehler. Leere Engineausgabe bleibt `text`. Ladefehler werden nicht als v3-Erfolg ersetzt. Ausnahme Warmup W3. |
| A/B | Paarreihenfolge und A/B unabhängig zufällig; IDs werden erst danach vergeben. Seite enthält keine Zuordnung, keinen Seed, keine Modell-Inferenzzeiten oder modellabhängigen Audio-Links. Originale `rec_<UTC>_lauf-N`-Pfade verraten aber weiterhin Aufnahmezeit/Lauf und ermöglichen Wiedererkennen einer live gesehenen Ultra-Ausgabe; das ist zusätzliche Reidentifizierbarkeit innerhalb der bereits eingeräumten Teilblindheit. Neutrale Audio-Aliasse würden sie verringern. |
| Statistik/HTML | Wilson-Formel korrekt; K5 nicht als Sieg und nicht in U+V; 300/50 und U≥2V numerisch korrekt. Keine externe HTML-Ressource, EscapeHtml beim Einbetten, Textknoten statt HTML-Ausführung. Grenzen der Abnahmelogik: W5–W8. |
| Prozessaufrufe | stdout und stderr werden beide mit `ReadToEndAsync()` gestartet, bevor gewartet wird; kein erkennbarer gegenseitiger Pipe-Deadlock. Texte werden nicht an die Konsole weitergereicht. |
| Release | Alle Modelle strukturiert gelesen/geschrieben, Default gegengeprüft, Größen/Hashes und Quellenfelder zurückverglichen, beide NOTICEs Pflicht. Kein „erstes Modell“-Regex mehr. Einschränkungen W9/L2. |
| Quantisierung/Lock | Getrennte verifizierte Quellkopie im regulären Pfad, vier Quell-Sollwerte, External-Data-Prüfung, festes QInt8/per-channel-Rezept, feste Ausgangshashes vor Promotion. Hashabweichung stoppt ohne neue Sollwerte. Python-/Paketstand und Hash-Lock dokumentiert. Bei Netz-/IO-/ORT-Ausnahmen erfolgt ein nicht erfolgreicher Python-Abbruch; die Downloadfunktion entfernt `.part` nur bei ihrer expliziten Hash-/Größenabweichung, nicht bei jeder Ausnahme. Kein falscher Erfolgsmarker, aber solche Reste sind zusätzlich ungetestet. Kritischer Sonderpfad K1. |

## Selbst ausgeführt

`TEMP` und `TMP` für den Cargo-Prozess auf das bereits vorhandene
`D:\DEV\diktier\target` umgeleitet, damit die `tempfile`-Tests nicht im
produktiven `%TEMP%` anlegen. Anschließend:

```text
cargo test
test result: ok. 550 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 2.02s
cargo test exit=0

cargo clippy --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.51s
Exitcode 0
```

Zusätzlich lesend: Working-Tree-Status, Basiscommit, die auftragsbezogenen
Diffs und `git diff --check -- src scripts Cargo.toml` (Exit 0; nur
Git-Hinweise zu künftigen CRLF-Konvertierungen).
Nicht ausgeführt: ignorierte Tests/stt-smoke, Release-Build oder
Release-Skript, PowerShell-Auswertung gegen echte oder synthetische Daten,
Quantisierung, Modellladen oder Downloads. Keine installierte Version und
keinen laufenden Daemon bedient.

## Übernommen aus Implementiererberichten — nicht selbst wiederholt

- WP0: bitgleiche lokale Quantisierung, Quell-/Output-Negativproben,
  Hash-Lock-Installationsprobe und Prüfung des Staging-`SHA256SUMS`.
  Vollständiger Großdatei-Download und abweichende Quantisierungsausgabe
  waren dort ausdrücklich nicht getestet. Recipe-Commit in der NOTICE
  sowie NOTICE-Zeilenenden bleiben Veröffentlichungsthemen.
- WP2a/WP2c: v3-stt-smoke, Release-Build/Bundle-Gate und synthetische
  Negativproben des Release-Lesers; HTML-/Browser-/Escape- und Urteilsproben.
  Das sind dokumentierte Ergebnisse, keine in diesem Review selbst
  ausgeführten Gates.
- WP2c: Der echte Benchmark verweigerte den Start wegen des laufenden
  Daemons. Der Messpfad wurde nur in einer Scratchpad-Kopie mit
  ausgeschalteter Daemonprüfung durchlaufen; ausdrücklich kein gültiger
  Kriterium-4-Leistungsnachweis.
- WP2d: reale Batch-Transkription mit beiden Modellen einschließlich f32,
  Stille/Rauschen und lokalem CLI-Rückweg bei gekürztem Ultra-Encoder.
  Daemon-Config-Rückweg und anonymes Transport-Gate stehen weiterhin in
  WP3b beziehungsweise WP3a.

## Relevante Grenzen der vorhandenen Tests

Die grünen 550 Tests belegen nicht die Skript-/Browserverträge. Dort gibt es
im Working Tree keine entsprechende dauerhaft ausführbare Regression-Suite;
die Notizen beschreiben Ad-hoc-Proben. Besonders zu ergänzen sind die
Fehlerszenarien K1/W1–W9/L1–L3 mit kleinen Fakes, ohne reale Modelle.

Die Wiring-Probe benutzt zwar echte State-/Download-Prüffunktionen, verdrahtet
`load_model` und Tray aber selbst; sie kann eine spätere falsche Übergabe
in `EngineWorker::spawn` oder `TrayWorker::spawn` nicht unmittelbar erkennen.
Aktuell ist diese Produktionsverdrahtung statisch korrekt. Echtes ORT-Laden
bleibt im selbst ausgeführten Testlauf unter den ignorierten Tests.

Zusätzlich fehlen Dauerproben für Teil-Download/Abbruch eines Modells bei
gleichzeitig vollständigem Nachbarmodell, für Win32-Ladefehler im echten
Worker und für vollständige Benchmark-Provenienz/fehlende Peak-Werte.
Die allgemeinen Download-Abbruchtests und die separaten Modellpfadtests
decken jeweils Teile davon ab, nicht die gesamte Kombination.
