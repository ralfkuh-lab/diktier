# Review: Relativer Silence-Gate, Plan v1

Prüfstand: Commit `5315181`, `docs/silence-gate-plan.md` v1 vom 2026-09-21.
Review anhand von Plan, Engine samt Tests, Daemon-/CLI-Aufrufern, SPEC,
SPIKES und Testdaten-Dokumentation; keine erneute Parakeet-/Live-Messung.
Die Gegenbeispiele unten sind konstruierte Fenster-RMS-Folgen, keine
Behauptungen über den Inhalt der WAV-Fixtures.

## Kurzurteil

Der relative Zusatzpfad ist eine sinnvolle Antwort auf den Pegelabfall, aber der Plan ist in dieser Form noch nicht implementierungsreif.
Ein Null-Floor kann Einzelklicks samt digitaler Stille als langen Sprachlauf durchlassen, und der Ersatz des bisherigen absoluten Laufkriteriums kann bislang akzeptierte Signale verwerfen.
Vor der Umsetzung müssen Null-Floor-Behandlung, exakte Fenster-/Perzentilsemantik, die Regressionsentscheidung und eine verbindliche Kalibrierungs- und Abnahmematrix ergänzt werden.

## Blocker (müssen vor Implementierung in den Plan)

### B1 — Null-Floor hebelt den Halluzinationsschutz aus

**Fundstelle:** `docs/silence-gate-plan.md`, „Zielverhalten“, Regeln C/D; WP2.

**Problem:** Bei `floor = 0` wird die D-Schwelle ebenfalls null und
`RMS >= 0` trifft auch auf jedes vollständig stille Fenster zu.
Beispiel: 10 s digitale Null plus ein 250-ms-Fenster mit RMS `0,001`.
Gesamt-RMS ≈ `0,000156`, daher kein B; max. Fenster `0,001`, daher kein C;
D zählt alle 10,25 s als Lauf und ruft die Engine auf. Das ist gerade
der Fall eines Treiber-Noise-Gates mit einem kurzen Öffnen/Klick.
Reine Nullen werden zwar von C abgefangen, Nullen plus ein ausreichend
großes Ereignis aber nicht. Sehr kleine positive Floors können entsprechend
Quantisierungs-/Restgeräusche statt Sprache zum Lauf verbinden.

**Vorschlag:** Eine explizite, positiv begrenzte Aktivitätsschwelle für D
festlegen, beispielsweise `max(floor * ratio, MIN_ACTIVE_RMS)`, und deren
Untergrenze anhand leiser Sprache und Nicht-Sprache kalibrieren.
Nur `>` statt `>=` oder ein Maschinen-Epsilon löst das Restgeräuschproblem
nicht. `ABS_FLOOR` nicht ungeprüft als Floor vor der Multiplikation
verwenden: Das ergäbe bereits eine Aktivitätsschwelle von ca. `0,001194`
und verändert die zugesagten leisen Sprachläufe.
Pflichttests: reine Null; Null plus ein Klick; zwei Klicks mit Nullpause;
Null plus ausreichend lange leise Sprache; dieselben Fälle mit einem
LSB Restpegel. Nullen dürfen nie einen Lauf verlängern oder verbinden.

### B2 — „Keine Regression gegen heute“ gilt nur für B, nicht für den ganzen Gate

**Fundstelle:** Plan, Regel B, Regel D, WP2 „bestehende bleiben grün“;
`src/engine.rs`, `silence_gate` und `longest_loud_run_secs`.

**Problem:** Heute wird bei Gesamt-RMS unter `0,0075` auch ein absolut
lauter Lauf von mindestens 2 s akzeptiert. Dieser Ja-Pfad verschwindet.
Konkretes Gegenbeispiel mit vollständig ausgerichteten Fenstern:
8 s RMS `0,003`, danach 2 s RMS `0,010`.
Gesamt-RMS ist ca. `0,00522`; heute passiert das Signal wegen des
2-s-Laufs über `0,0075`. Künftig ist der Floor `0,003`, die D-Schwelle
ca. `0,01194`, also höher als jedes Fenster: Das Signal wird verworfen.
Das Gegenbeispiel hängt nicht von der üblichen Perzentilinterpolation ab.
Der bestehende Test mit Floor `0,001` und Sprache `0,02` deckt es nicht ab.

**Vorschlag:** Dieses Gegenbeispiel als Regressionstest aufnehmen und
die Entscheidung vor WP2 festhalten: Entweder den bisherigen absoluten
2-s-Ja-Pfad zusätzlich zu D erhalten, oder die gezielte Verschärfung bei
höherem Grundpegel ausdrücklich freigeben und mit Sprachfällen belegen.
B ist zudem nicht bloß eine Optimierung: Gleichmäßig laute Sprache,
kurze laute Diktate und auch gleichmäßig lautes Rauschen passieren B,
obwohl D mangels Kontrast oder Laufdauer ablehnen würde.
Diese Semantik muss erhalten beziehungsweise bewusst geändert werden;
„sicher laut“ ist keine Feststellung von Sprache.

### B3 — Perzentil und Fensterdauer sind noch keine eindeutige Spezifikation

**Fundstelle:** Plan, Floor-Definition, Regel D und WP2
`window_rms(pcm) -> Vec<f32>` / `longest_run_secs(windows, thr)`;
`src/engine.rs`, bisherige Fensterhelfer.

**Problem:** „10. Perzentil, mindestens das Minimum“ definiert weder
Indexwahl noch Interpolation. Bei acht Fenstern mit RMS
`[0,0001, 0,002, …, 0,002]` ergibt Nearest Rank den Floor `0,0001`
und 1,75 s über +12 dB. Lineare Interpolation mit Index `0,1 * (n-1)`
ergibt dagegen `0,00143`: Kein Fenster überschreitet die Schwelle.
Beides sind gebräuchliche Perzentildefinitionen.
Außerdem berücksichtigt der heutige Code ein letztes Teilfenster und
zählt dessen tatsächliche Samples. Ein `Vec<f32>` allein verliert diese
Dauerinformation; sechs Fenster können weniger als 1,5 s Audio umfassen.
Ein winziges leises Restfenster darf auch nicht unbemerkt genauso viel
Einfluss auf den Floor bekommen wie ein volles 250-ms-Fenster.

**Vorschlag:** Fensterstart, nicht überlappende Schrittweite,
Teilfensterbehandlung für Floor/max/Lauf und die genaue Quantilformel
einschließlich `n = 1…8` normativ festlegen. Laufdauer in Samples zählen
und Fensterlängen beziehungsweise die PCM-Gesamtlänge verfügbar halten.
Die Messwerte mit genau dieser Definition reproduzieren.
Grenztests müssen 3.999/4.000 Samples, 23.999/24.000 Samples Lauf,
einzelne Restsamples und relativ zum Fensterraster verschobene Ereignisse
abdecken. Ohne diese Festlegung sind Python-Kalibrierung und Rust-Abnahme
nicht verlässlich vergleichbar.

### B4 — Verbindliche Nicht-Sprach- und Echtaufnahme-Abnahme fehlt

**Fundstelle:** Plan, WP1, „Gates“ 3/4 und „Risiken und offene Fragen“;
`docs/SPEC.md` §12 Phase 1; `docs/SPIKES.md`, Phase-2-Zeile
„RMS-Silence-Gate“.

**Problem:** Fünf Versuche im stillen Raum prüfen nicht den neu geöffneten
D-Pfad. 250-ms-RMS kann wiederholte kurze Tastaturimpulse über mehrere
Fenster zu einem durchgehenden Lauf zusammenfassen. Ebenso können ein
Lüfter-Anlauf nach ruhigem Vorlauf, Reiben am Headset/Kabel, Stuhlrollen
oder anhaltendes Atmen/Wind am Mikrofon +12 dB für 1,5 s erreichen.
„Lüfter heben den Floor selbst“ gilt nicht, wenn ausreichend ruhige
Fenster im Puffer verbleiben. Das bisherige Halluzinationsbeispiel entstand
live; skalierte Sprache belegt dessen Vermeidung nicht.
Nur einen Tipp-Test im Risikotext zu erwähnen, ist keine verbindliche
Abnahme, und +15 dB/2 s sind ohne Gegenmessung keine gesicherte Reparatur.

**Vorschlag:** Vor der produktiven Umsetzung ein kleines WP0
„Kalibrierungsentscheidung“ einplanen: echte leise Sprachaufnahmen über
den Windows-Capture-Pfad sowie die genannten Nicht-Sprachklassen,
jeweils mit/ohne ruhigen Vorlauf, mit festgehaltenem Gerät/DSP-Zustand.
Die exakte historische Jabra-Störung muss dafür nicht wiederkehren;
repräsentative reale leise Aufnahmen sind aber nicht optional.
Ein Messprototyp darf der Datensammlung dienen.
Für die Abnahme getrennt festlegen: Welche Fälle müssen ohne
Engine-Aufruf verworfen werden, welche dürfen die Engine erreichen,
aber müssen textlos bleiben? Je Fall Gate-Entscheidung, Engine-Aufruf
und Ergebnislänge erfassen; ein bloß leeres Ergebnis beweist keinen
funktionierenden Gate. Für jede Schwellenänderung auch alle leisen
Positivfälle erneut prüfen. Das Ziel „keine Halluzination“ bleibt
SPEC-konform; fünf stille Versuche allein belegen es nicht ausreichend.

## Wichtige Hinweise (sollten rein)

### W1 — Ein unteres Quantil ist ohne Rauschfenster kein Rauschschätzer

**Fundstelle:** Plan, „Verhältnis statt Absolutwert“, Vorbehalt,
Regel D und Risiko „Sehr kurze Diktate“.

**Problem:** Ein einzelnes positives Fenster hat `floor = max`;
ohne B erreicht es niemals +12 dB. Bei wenigen Fenstern ist das
10. Perzentil vor allem ein einzelner Ordnungswert oder dessen
Interpolation. Bei pausenlosem Sprechen kann es auch bei längeren
Diktaten leise Sprachanteile statt Rauschen schätzen. Mit Sprache
beginnen ist nicht automatisch schädlich, solange später ausreichend
rauschdominierte Fenster vorkommen; ein fester „Anfang = Rauschen“-Ansatz
wäre hingegen falsch. Die dokumentierte Lücke endet nicht bei 1,5 s:
Auch längere leise, gleichmäßige Äußerungen können an D scheitern.

**Vorschlag:** Den Zielanspruch auf belegte Bedingungen begrenzen und
kurze, pausenlose, sofort beginnende sowie durch Pausen unterbrochene
Sprache getrennt messen. Minimum ist empfindlicher gegen einen einzigen
Dropout/Nullblock; Median der leisesten `k` Fenster dämpft einzelne
Ausreißer, benötigt aber eine Definition von `k` und genügend echte
Rauschfenster; ein gleitendes Minimum benötigt Zeitkonstante und Reset
und kann ebenfalls an Nullen hängen. Keiner dieser Schätzer schafft
Rauschbeobachtungen, die in einer Aufnahme fehlen.
Vorläufig das exakte Quantil plus definierte Absicherung bevorzugen,
statt einen komplexeren Schätzer ohne Gegenbelege einzuführen.
Festlegen, ob fehlender Kontrast bei solchen Diktaten eine akzeptierte
Grenze bleibt oder einen gesondert zu validierenden Ja-Pfad braucht.

### W2 — C erkennt niedrigen Pegel, kein totes Gerät

**Fundstelle:** Plan, Regel C und `DeadInput`; `docs/SPEC.md` §6.4/§10.

**Problem:** Für endliche RMS-Werte gilt `floor <= max`; die zusätzliche
Floor-Bedingung in C ist daher redundant. `0,0003` entspricht etwa
−70,46 dBFS beziehungsweise 9,8 LSB bei normiertem 16-bit-PCM.
Ein LSB ist ca. `0,0000305`; idealisiertes gleichverteiltes
Quantisierungsrauschen hat RMS ca. `0,00000881`.
C fängt dieses kleine Rauschen ab, setzt aber zugleich eine reale
absolute Empfindlichkeitsgrenze: Unterhalb davon wird auch Sprache
verworfen. f32-Transport garantiert weder analoge Auflösung noch
Abwesenheit geräteinterner Quantisierung/DSP.
Ein solches Signal beweist keinen Device-Ausfall; dafür hat §10 einen
anderen Recovery-Pfad.

**Vorschlag:** C als „Pegel unter numerischer/kalibrierter Untergrenze“
beschreiben und die Grenze als Ausnahme von „pegelunabhängig“ benennen.
Einen neutralen Grund wie `BelowAbsoluteFloor` verwenden oder
`DeadInput` ausdrücklich ohne Gerätefehler-/Recovery-Semantik definieren.
Tests knapp unter/an/über `ABS_FLOOR`, für 16-bit-Quantisierung und
unquantisierte f32-Signale aufnehmen. Die C-Entscheidung löst B1 nicht.

### W3 — Tests müssen den Gate-Vertrag und den echten WER-Vertrag prüfen

**Fundstelle:** Plan, WP1/WP2 und Gate 2; `src/engine.rs`, Modultests
und `stt_smoke_fixtures`; `testdata/stt/normalize.py`, `README.md`.

**Problem:** Der heutige ignored Smoke-Test prüft bei Sprache nur
„nicht leer“, nicht WER oder Wortidentität. Der Aufrufzähler für
Stille/Rauschen existiert im normalen Stub-Test, nicht im Parakeet-Smoke.
„WER-Puffer wie alltag.wav“ beschreibt daher neue Prüflogik, nicht nur
zwei zusätzliche Array-Einträge. Digitale Absenkung erhält den SNR
näherungsweise, bildet aber Sprache, die relativ zu gleichbleibendem
Rauschen leiser wird, nicht nach; erneute 16-bit-Quantisierung kann
Fenster-RMS und Schwellenübertritte verändern.

**Vorschlag:** WP2 explizit um normalisierten Textvergleich/WER anhand
der vorhandenen Normalisierung und der dokumentierten Voxtype-Baseline
ergänzen. Kein versehentlich zusätzlich aufgeschlagener 0,05-Puffer
auf den bereits schlechteren Diktier-Ausgangswert.
Für No-Call-Belege einen zählenden Transcriber verwenden; positive
Fälle müssen ebenfalls den tatsächlich erfolgten Aufruf belegen.
Zusätzlich zu B1–B3 vorsehen:

- B exakt unter/an/über `0,0075`, auch bei kurzer Aufnahme und Rauschen.
- D exakt an +12 dB, knapp darunter/darüber; getrennte Läufe dürfen nicht addiert werden.
- Gleichmäßiges leises Rauschen, wechselnder Pegel, Null-/Beinahe-Null-Floor und kurze starke Impulse.
- Alle drei Sprach-Fixtures abgesenkt, nicht nur eine Stimme/Äußerung in „Alltag“; geschnittene Kurzfälle mit eigener Referenz.
- Skalierungsreihen als f32 und als wieder quantisiertes 16-bit-PCM, mit klar abgegrenztem Gültigkeitsbereich wegen B/C.
- Erwartete neue Gate-Varianten und aussagekräftige Display-Ausgaben statt alter `BelowThreshold`-/`NoLoudRun`-Assertions.

Die vorhandenen Tests können in ihrer Verhaltensabsicht bestehen
bleiben, aber nicht sämtlich unverändert. Die expliziten Falltests
sollten ohne Modell laufen; WER und Live-Capture sind ergänzende Gates.

### W4 — Die neuen Messwerte sind §10-konform, zur Kalibrierung aber unvollständig

**Fundstelle:** Plan, „Log-Grund“, WP3 und Gate 3;
`src/daemon/workers.rs::engine_loop`; `src/main.rs::record_test`.

**Problem:** Aggregierte Pegel/Dauern verletzen §10 nicht.
Unklar bleibt, ob `ratio_db` die konfigurierte Marge oder einen
gemessenen Abstand meint; bei Floor null ist der gemessene Abstand
nicht endlich. Aufnahme-/Fensteranzahl, tatsächliche D-Schwelle und
eine angewendete Untergrenze fehlen. Vier Nachkommastellen wie im
heutigen Display sind bei Floors um `0,0001` zu grob.
Der Daemon protokolliert den Gate-Grund nur bei Ablehnung, nicht bei
neu durchgelassenem Nicht-Sprachsignal. Aus diesem Log allein lassen
sich alternative Perzentile oder +15-dB-Läufe nicht rekonstruieren.
WP3 nennt nur `--transcribe-wav`; `record_test` hat denselben alten
Bool-Pfad und liefert den in Gate 3 verlangten Grund noch nicht.

**Vorschlag:** Beide CLI-Pfade ausdrücklich aufnehmen; sie bleiben bei
stderr, nicht beim Daemon-Logfile. Begrenzte numerische Diagnose auch
für akzeptierte Fälle vorsehen: B/absoluter Lauf/D, Sampleanzahl,
Fensteranzahl, RMS/Floor/max, effektive Schwelle, angewandte Untergrenze,
Laufdauer und konfigurierte Marge mit genügend Präzision.
`ratio_db` eindeutig benennen; Nullfälle ohne erfundene endliche
Messwerte darstellen. Geräte-/Capture-Kontext und Run-Zuordnung
sollten korrelierbar sein. Offline-Nachkalibrierung verlangt weiterhin
bewusst gesicherte lokale Fixtures; das Standardlog ist kein Ersatz
für sie. Kein Audio, Text oder Fenstertitel ins Log.

### W5 — Spec-Freigabe und Messbeleg gehören vor die produktive Änderung

**Fundstelle:** Plan, WP4, WP5 und Reihenfolge; SPEC §6.4, §12, §18;
SPIKES Phase 2; Testdaten-README.

**Problem:** §6.4 ist der richtige normative Ort, aber ein Nachtrag erst
als viertes Arbeitspaket lässt die Verhaltensentscheidung zeitlich offen.
Vier Konstanten allein definieren den Algorithmus wegen B1–B3 nicht.
Die historische SPIKES-Zeile ist der bisherige Kalibrierungsbeleg;
ein Verweis allein auf einen v1-Plan macht daraus noch keinen neuen
Abnahmebeleg. Das Testdaten-README nennt außerdem einen WER-Vergleich
ohne den in §12/§18 erlaubten +0,05-Puffer.

**Vorschlag:** Nach Kalibrierungsentscheidung den Spec-Vertrag
freigeben, dann produktiv implementieren. §6.4 um exakte
Fenster-/Quantil-/Grenzsemantik, Fallbacks, bekannte Kurz-/Pausenlos-Lücken
und „Gate-Ablehnung = leeres Ergebnis ohne Engine-Inferenz“ ergänzen;
„Sprache“ nicht als Klassifikationsgarantie formulieren.
§12 und §18 wie geplant verknüpfen und in SPIKES einen datierten neuen
Mess-/Abnahmeblock ergänzen, ohne den historischen Befund zu überschreiben.
README-Herkunft und WER-Definition angleichen.
WP5 bleibt sinnvoll separat: Seine Präsentationsänderung betrifft
§4.5 und darf kein verdecktes Zusatzkriterium für WP1–4 sein.
Der Rust-Umbau ist klein; reale Kalibrierung und ein echter WER-Smoke
sind die zusätzlich einzuplanende Arbeit, keine bloßen Doku-Zeilen.

## Kleinigkeiten

### K1 — Priorität und Eingabedomäne ausdrücklich festhalten

**Fundstelle:** Plan, „Regeln in Reihenfolge“, A–D; WP2 `noise_floor`.

**Problem:** Für endliche PCM-Werte und einen definierten Floor ist die
Entscheidung vollständig: A/B/C sind frühe Ausgänge, D besitzt einen
expliziten Nein-Zweig. B und C können nicht gleichzeitig zutreffen,
weil max. Fenster-RMS mindestens so groß wie Gesamt-RMS ist.
C und das rohe D-Prädikat treffen bei Null hingegen beide zu; durch
Priorität ist das kein widersprüchliches Ergebnis, sondern B1 bleibt
erst bei gemischten Puffern bestehen. NaN/Inf sind noch undefiniert,
insbesondere bei Sortierung der RMS-Werte.

**Vorschlag:** „Erste zutreffende Regel entscheidet“ und die endliche
Eingabedomäne explizit machen. Falls diese nicht bereits beim
Audio-Eingang garantiert wird, nichtendliche Samples als gemeldeten
Eingabefehler behandeln und testen, nicht als stillen normalen Gate-Fall.
Keine eigene neue Signalbereinigung im Gate einführen.

### K2 — „Einmal berechnen“ gilt heute nicht für den ganzen Aufrufpfad

**Fundstelle:** Plan, WP2; `src/daemon/workers.rs::engine_loop`
und `src/engine.rs::transcribe_pcm`; `src/main.rs::transcribe_wav`.

**Problem:** Der Daemon ruft den Gate für die Logzeile und anschließend
noch einmal in `transcribe_pcm` auf; die WAV-CLI wiederholt ihn auch bei
warmen Läufen. Eine lokale Fensterliste beseitigt nur Mehrfachscans
innerhalb eines einzelnen Gate-Aufrufs.

**Vorschlag:** Den Anspruch entsprechend eingrenzen oder ein einmal
ermitteltes Gate-Ergebnis samt Messwerten im Aufrufer wiederverwenden.
Dabei den geprüften `transcribe_pcm`-Vertrag nicht durch ungesicherte
direkte Engine-Aufrufe unterlaufen. Keine große Architekturänderung
allein für diese kleine Optimierung.

### K3 — Dokumentationsstand und Reproduzierbarkeit präzisieren

**Fundstelle:** Plan, WP1/WP4; SPEC-Kopf; `testdata/stt/normalize.py`.

**Problem:** SPEC enthält bereits den v1.5-Nachtrag, trägt im Titel
aber noch v1.4; der Plan verweist sachlich zu Recht auf v1.5.
`normalize.py` normalisiert Text und berechnet WER, nicht Audio.
„Deterministisch, 16 bit, ohne Dithering“ lässt die Rundungsregel
der neuen WAV-Fixtures offen.

**Vorschlag:** Bei WP4 Titel und Versionshistorie gemeinsam aktualisieren.
Ein separates Audio-Absenkskript bevorzugen und Faktor, Rundung,
Sättigung, Originaldatei und reproduzierbare Hashes dokumentieren.
Synthetische Ableitungen eindeutig von echten Capture-Aufnahmen trennen.

## Offene Fragen an den Autor

### F1 — Welche Garantie soll „pegelunabhängig“ tatsächlich geben?

**Fundstelle:** Plan, Zielverhalten und Kurz-Diktat-Risiko.

**Problem:** Kurz, pausenlos, geringer Sprach-Rausch-Abstand und C
begrenzen die Aussage unterschiedlich; „heute ebenfalls verworfen“
beantwortet nicht, ob das Ziel damit erreicht wird.

**Konkreter Vorschlag / Frage:** Als zugesagten Bereich zunächst
„ausreichend lange Sprache mit belegtem Kontrast, oberhalb definierter
Aktivitätsuntergrenze“ festlegen. Sollen leise Ein-Wort-Diktate und
pausenlose 2-s-Diktate ausdrücklich außerhalb dieses Pakets bleiben?

### F2 — Ist Erhalt aller bisherigen Ja-Entscheidungen Pflicht?

**Fundstelle:** Plan, Regel B „keine Regression“, WP2.

**Problem:** Das B2-Gegenbeispiel wird bislang weder als gewünschte
Verschärfung noch als verbotene Regression behandelt.

**Konkreter Vorschlag / Frage:** Den bisherigen 2-s-Absolutpfad zunächst
erhalten. Falls nicht: Welche belegten Fehlannahmen sollen durch seine
Entfernung verhindert werden, und welche Sprachverluste sind freigegeben?

### F3 — Welche reale Aufnahmematrix ist vor Freigabe verfügbar?

**Fundstelle:** Plan, optionale Jabra-Fixture und Gates 3/4.

**Problem:** Der verlorene Lauf 83 ist kein reproduzierbarer Positivfall;
aus max. Fenster und Gesamt-RMS lässt sich sein D-Lauf nicht ableiten.

**Konkreter Vorschlag / Frage:** Reale leise Jabra-Aufnahmen und möglichst
ein zweites Windows-Mikrofon mit dokumentiertem DSP-/Noise-Gate-Zustand
einplanen. Wer nimmt die in B4 genannten Positiv-/Negativfälle ab, und
welche davon werden lokal als dauerhafte Regressionen gesichert?

### F4 — Was gilt im Live-Gate als nachgewiesener Erfolg?

**Fundstelle:** Plan, Gate 3 „normales Diktat → eingefügt“, „beides aus dem Log“.

**Problem:** Ein §10-konformes Zahlenlog kann Inferenz und Inject-Status
belegen, aber nicht die inhaltliche Korrektheit des eingefügten Diktats.
`--record-test` injiziert überhaupt nicht, sondern schreibt das
Transkript nach stdout.

**Konkreter Vorschlag / Frage:** CLI-Capture-Abnahme und Daemon-End-to-End-
Abnahme trennen; im Daemon ein leises, nicht nur normal lautes Diktat
vorsehen. Ist ein manueller Vergleich im Zielprogramm mit dokumentiertem
Pass/Fail vorgesehen, während das Log ausschließlich Messwerte behält?
