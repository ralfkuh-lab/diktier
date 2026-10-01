# Alltagstest Parakeet Ultra mit Modell-Release (Plan, v2)

Stand: 2026-09-30. Diese v2 entstand nach dem Plan-Review durch GPT-6.1 Sol
([reviews/plan-ultra-alltagstest-sol.md](reviews/plan-ultra-alltagstest-sol.md),
Auftrag [reviews/plan-ultra-alltagstest-prompt.md](reviews/plan-ultra-alltagstest-prompt.md)).
Eingearbeitet sind die Blocker B1–B3, W1–W8 und H1–H3; wo v2 vom Vorschlag
abweicht, steht es unter „Umgang mit dem Review“. v1 vom selben Tag war der
Entwurf ohne Review. Auftrag von Ralf: „plane den Alltagstest mit dem
Modell-Release“.

Anlass: Der Spike vom 2026-09-30
([reviews/spike-parakeet-ultra-notes.md](reviews/spike-parakeet-ultra-notes.md))
hat `ultra-int8-pc` zum Favoriten gemacht. Das ist Moondreams Parakeet Ultra,
selbst per-channel nach int8 quantisiert. Auf den Referenzdateien kommt es auf
2 statt 9 Wortfehler, bei gleicher Latenz, +175 MiB RAM und 719 statt 670 MB.
„Herr Präsident“ tritt dort nie auf, auch ohne Vorlauf-Stille. Die Datenbasis
ist klein, und auf dem Ring gibt es keinen klaren Sieger. Deshalb kommt erst ein
Alltagstest mit vorab festgelegtem Bewertungsprotokoll, dann die Entscheidung.

**Spec-Status:** Verbindlich ist SPEC v1.9. Betroffen sind §6.2, §6.3, §8, §9,
§10 und §11. Die Reihenfolge ist: erst der Spec-Nachtrag (WP1, → v1.10), dann
Code.

**Nicht Ziel:** der endgültige Wechsel des Default-Modells. Er ist ein eigenes
Paket nach der Auswertung.

**Geltungsbereich des Ergebnisses:** Ralfs Windows-Alltag mit der Jabra, eine
Person, ein Rechner. Befangenheit bleibt, denn Ralf erlebt Ultra live. Das
Protokoll mindert sie, beseitigt sie aber nicht.

## 🔍 Ausgangslage (HEAD `2d10ee0`, 0.4.1)

- `src/models.toml` beschreibt **ein** Modell und wird per `include_str!`
  eingebaut (`download.rs:26`). Mehrere Verbraucher holen sich unabhängig
  voneinander „das“ Manifest: der Daemon für Verzeichnis, Download, Engine und
  Tray (`daemon/mod.rs`), `ParakeetTranscriber::load` (Vergleich
  `manifest.key == key`) und `model_artifacts` (stt-smoke, auf `DEFAULT_MODEL`
  verdrahtet).
- `config.rs:460` lehnt jeden `engine.model` außer `DEFAULT_MODEL` fatal ab.
  Ralfs `config.toml` nennt den v3-Schlüssel ausdrücklich.
- `check_artifacts` prüft beim Start nur Existenz und Größe, keinen SHA-256, und
  ignoriert `COMPLETE`. Die volle Hashprüfung läuft nur beim Download. Eine gleich
  große, beschädigte Datei löst deshalb keinen Reparatur-Download aus.
- Die Download-Sperre ist ein pfadbasierter `Local\`-Mutex (`single_instance.rs`).
- `scripts/release.ps1` liest per Regex den **ersten** `key` und die erste `url`
  aus `models.toml`, erzeugt genau einen `[model]`-Block in `versions.toml` und
  leitet Repository und Revision aus einer HF-`resolve`-URL ab.
- Debug-WAV: nur bei `DIKTIER_DEBUG_WAV=1` (bei Ralf als Benutzervariable
  gesetzt). Ring `KEEP = 10` fest in `%TEMP%\diktier`. Seit 0.4.1 als f32,
  bitgenau nachrechenbar.
- `--transcribe-wav <datei>` nimmt eine Datei und das Config-Modell und lädt es
  bei jedem Aufruf neu.
- Das Log enthält je Lauf `Lauf N: Gate: …` und `Inferenz x s, N Zeichen`, das
  zweite ohne Laufnummer. Text steht nie im Log (§10). Die Startzeile nennt den
  Modellschlüssel. Erfolgsmeldung des Downloads: „Modellartefakte vollständig und
  geprüft“.
- Ralf diktiert laut `diktier.log.1` rund 75-mal pro Arbeitstag (1728 Läufe vom
  2026-08-27 bis 2026-09-28).
- Das Repo `ralfkuh-lab/diktier` ist öffentlich. `main` steht auf GitHub bei
  `56a6a4e` (0.3.0); `d9902dd` (0.4.0) und `2d10ee0` (0.4.1) sind lokal.
- Aus dem Spike: `models\spike\ultra-int8-pc\` und
  `.herd/spike-ultra/quantize_pc.py` (lokal).

## Umgang mit dem Review

| Befund | Umgang |
|---|---|
| B1 Qualitätsentscheidung | Übernommen: Bewertungsprotokoll mit festem Zeitraum, Mindestmengen, verdecktem A/B, Fehlerkategorien und Veto (Abschnitt „Bewertungsprotokoll“). **Angepasst:** Audio wird nur bei Unsicherheit angehört, nicht bei jedem Paar. Die Stichprobe gleicher Ausgaben umfasst 20 Aufnahmen. |
| B2 Latenz | Übernommen: gepaarte Offline-Messung auf denselben WAVs mit demselben Binary, dazu p95, Fehler und Peak Working Set ≤ 2 GiB (SPEC §3). Die alten Logs gelten nur als Kontext. Ein Live-v3-Kontrollblock entfällt; Live-Latenz ist ein UX-Eindruck aus den Notizen. |
| B3 release.ps1 | Übernommen in WP2a. |
| W1 Belegsammlung | Übernommen, **angepasst:** Statt täglich zu kopieren schreibt der Ring während des Tests direkt in ein Verzeichnis außerhalb von `%TEMP%` (`DIKTIER_DEBUG_WAV_DIR`). Die Startzeile loggt Verzeichnis und Kapazität. Die Inventur vergleicht die Läufe laut Log mit den Dateien. |
| W2 Datenschutz | Übernommen: Texte, Zuordnung und Notizen liegen nur lokal. Im Repo landen nur aggregierte Zahlen und von Ralf freigegebene Beispiele. |
| W3 Modellwahl/Rückweg | Übernommen: ein gewähltes Manifest durch alle Verbraucher, vorher ein v3-Vollcheck, Rückweg-Gate. |
| W4 Transport | Übernommen: Draft authentifiziert zurücklesen, nach der Veröffentlichung ein anonymes Transport-Gate mit dem echten `HttpTransport` in ein frisches Verzeichnis, **vor** der Umstellung. |
| W5 Lizenz | Übernommen: Checkliste für die NOTICE über die ganze Herkunftskette. |
| W6 Immutability | Übernommen als eigene Entscheidung F6, mit Empfehlung eines separaten Modell-Repos. |
| W7 CLI | Übernommen: Dateiliste als Eingabe, JSONL-Ausgabe, eindeutige Zustände und Exitcodes. |
| W8 Ultra-Gate | Übernommen: lokales Ultra-Integrationsgate vor dem Push; v3-Golden-Set und smoke bleiben unverändert. |
| H1–H3 | Übernommen: Gate-Formulierung, vollständiger Dateisatz im Rezept, feinere WPs. |

## Leitentscheidungen

1. **Zwei freigegebene Schlüssel, Default bleibt v3.** Das Manifest beschreibt
   beide Modelle. `engine.model` akzeptiert genau diese beiden; jeder andere
   Schlüssel bleibt fatal, ohne Fallback. **Ein** aus der Config gewähltes
   Manifest wird an alle Verbraucher durchgereicht (Daemon, Download, Engine,
   Tray, `model_artifacts`). Umschalten und Zurückschalten heißt: Config ändern,
   Daemon neu starten. Beide Verzeichnisse bleiben nebeneinander.
2. **Schlüssel `parakeet-ultra-0.6b-int8-pc`, ein Artefaktvertrag pro
   Schlüssel.** Ändern sich Bytes (neues Rezept, neuer Export), gibt es einen neuen
   Schlüssel, nie andere Dateien unter demselben.
3. **Modell-Release auf GitHub**, getrennt von App-Releases, mit dem Tag
   `model-parakeet-ultra-0.6b-int8-pc-r1`, immutable, nicht „Latest“. Wo es
   liegt (dieses Repo oder ein eigenes Modell-Repo), entscheidet F6. Die URLs
   sind kanonische `…/releases/download/<tag>/<datei>`-Adressen, keine
   Redirect-Ziele. Die Integrität sichern Größe und SHA-256 aus dem Manifest,
   die Verfügbarkeit ist nicht garantiert. Fällt die Quelle aus, gibt es einen
   verständlichen Fehler, und der Rückweg ist v3.
4. **Reproduzierbares Artefakt.** Das Rezept liegt im Repo
   (`scripts/quantize-ultra.py`). Es zieht alle vier Quelldateien aus
   `altunenes/parakeet-rs` an Revision
   `4d2a8bc71f5c896ec40faa59732e6716295edaf2`, Ordner `parakeet-ultra/`:
   `encoder-model.onnx`, `encoder-model.onnx.data`, `decoder_joint-model.onnx`
   und `vocab.txt`, jeweils mit Größe und SHA-256 geprüft. Danach
   `quantize_dynamic(weight_type=QInt8, per_channel=True)` unter gepinntem
   Python und gepinnten Paketen (Lock). Die Ausgabe wird gegen feste Größen und
   Hashes geprüft: Encoder `2cc01c15…`, Decoder `afcb9459…`, `vocab.txt`
   unverändert `d5854467…`. Bei Abweichung ist Schluss; neue Sollwerte werden
   nicht nachgezogen.
5. **Sammlung außerhalb von `%TEMP%`.** Für den Test gelten
   `DIKTIER_DEBUG_WAV=1`, `DIKTIER_DEBUG_WAV_KEEP=5000` und
   `DIKTIER_DEBUG_WAV_DIR=%LOCALAPPDATA%\diktier\ultra-test\wav`. Die
   Startzeile loggt „Debug-WAV an: <Verzeichnis>, behalte <n>“, ohne Text. Bei
   ~750 Diktaten à ~10 s in zwei Wochen kommen rund 480 MB zusammen.
6. **Vergleichswerkzeug.** Die CLI bekommt `--transcribe-list <datei>` (eine
   WAV pro Zeile) plus `--model <key>`. Das Modell wird einmal geladen, und nur
   dann, wenn mindestens eine Aufnahme den Gate passiert. Ausgabe als JSONL auf
   stdout, eine Zeile pro Datei:
   `{"file":…,"status":"text|rejected|error","text":…,"infer_ms":…,"samples":…}`.
   Diagnose steht textfrei auf stderr. Der Exitcode ist ≠ 0, sobald eine Datei
   `error` hat. `--transcribe-wav <datei>` ohne `--model` bleibt wie heute.
   `scripts/compare-models.ps1` baut daraus den verdeckten Vergleich
   (Bewertungsprotokoll).
7. **Live läuft Ultra, verglichen wird hinterher verdeckt.** Die v3-Seite
   entsteht offline aus denselben bitgenauen WAVs.

## Bewertungsprotokoll (vor dem ersten Testdiktat festgeschrieben)

- **Zeitraum:** 7 Kalendertage ab Umstellung (WP3b; Ralf 2026-09-30: „ein
  bisschen kürzer“, er diktiert dafür häufiger). Eine einmalige Verlängerung um
  4 Tage gibt es nur, wenn die Mindestmengen fehlen, nie wegen einer knapp
  verfehlten Quote. Vor Ablauf wird nichts ausgewertet. Reicht es danach immer noch nicht, heißt das
  Ergebnis „nicht belegt“.
- **Mindestmengen:** ≥ 300 Aufnahmen mit Sprache und vollständigem Paar (beide
  Modelle `text`), davon ≥ 50 entscheidbare Paare (Urteil A oder B).
- **Grundgesamtheit:** alle Aufnahmen des Zeitraums laut Inventur. Ausschlüsse
  (Gate-Ablehnung bei beiden, Fehler, Duplikate) sind mit Grund gezählt.
- **Verdeckter Vergleich:** Das Skript nimmt nur Paare mit Textunterschied.
  Je Paar legt es A/B zufällig fest und speichert die Zuordnung getrennt in einer
  lokalen Schlüsseldatei. Die Ansicht zeigt beide Texte mit Wortdiff und einem
  Link auf die WAV. Ralf trägt pro Paar ein Urteil (A / B / gleich / unklar)
  ein und markiert **für beide Seiten** die Fehlerkategorien, die sie enthalten
  (keine oder mehrere von K1–K4, dazu „nur Zahlenformat“ K5). Bei K1 kommt je
  Seite „gravierend“ ja/nein dazu. Ein optionales Feld „gleicher Fall wie
  P…“ markiert Wiederholungen desselben Fehlers (v2.1 nach Code-Review
  W5). Bei A/B gibt es zusätzlich die Angabe „Unterschied nur Zahlenformat“
  (ja/nein). Bei „ja“ zählt das Paar in Kriterium 1 als gleich, auch wenn beide
  Seiten gemeinsame andere Fehler haben. Konsistenzregeln: „ja“ verlangt K5 auf
  der schlechteren Seite und gleiche K1–K4 auf beiden Seiten; „nein“ verlangt
  mindestens ein K1–K4 auf der schlechteren Seite (v2.2 nach Nachreview F4). Aufgelöst wird erst, wenn alle Urteile gespeichert sind. Dazu kommt eine zufällige Stichprobe
  von 20 Aufnahmen mit **gleicher** Ausgabe: Ralf prüft, ob sie stimmen, weil
  gemeinsame Fehler sonst unsichtbar blieben. Weil Ultra Zahlen als Ziffern
  schreibt, ist der Vergleich nur teilweise blind. Das steht so im Ergebnis.
- **Kategorien:** K1 Inhalt falsch oder ausgelassen · K2 Zahl-/Datumswert falsch
  · K3 Wortverschmelzung/-trennung · K4 Schreibweise/Interpunktion · K5 nur
  Zahlenformat (Ziffer gegen Wort bei gleichem Wert). **K5 zählt nie als
  Erkennungsgewinn**; solche Paare gelten als „gleich“ und gehen in die eigene
  Frage „Ziffern“ ein.
- **Halluzinationen:** Pro Modell wird gezählt, wie oft ein Text mit „Herr
  Präsident“ beginnt, ohne dass es gesprochen war. Ralf prüft das am Audio.
  Beim „Ich“ vor einem Befehl kommen nur Aufnahmen in die Audioprüfung, bei
  denen **genau ein** Modell mit dem Wort „Ich“ beginnt. Beginnen beide gleich,
  macht das für den Vergleich keinen Unterschied (v2.1, Code-Review W6).
- **Verbindlicher Lauf:** Die Vorbereitung hält Start und Ende des Zeitraums
  fest. Verbindlich ausgewertet wird nur nach Ablauf (7 Tage bzw. 11 mit der
  Verlängerung). Frühere Aufrufe sind explorativ und liefern kein Urteil. WAVs
  ohne zuordenbaren Zeitstempel gelten als Inventurproblem und zählen nicht zur
  Grundgesamtheit (Code-Review W7).
- **Verlängerung** (v2.2, Nachreview F5): Nach 7 Tagen wird verbindlich
  vorbereitet. Dieser Mengenstand bleibt liegen. Er hält die vollständigen Paare
  mit Sprache fest und die Paare mit Textunterschied als Obergrenze der
  entscheidbaren Paare. Verlängert werden darf nur, wenn eine der beiden Zahlen
  unter ihrer Mindestmenge liegt (300 bzw. 50). Die Dauer rechnet das Skript
  aus den gespeicherten Grenzen in lokaler Wanduhrzeit, mit der Zeitzone beim
  Vorbereiten; eine Zeitumstellung verkürzt den Lauf nicht.

## Abnahmekriterien für einen Wechsel

Ultra wird Default, wenn **alle** gelten:

1. Mindestmengen erreicht. Über die entscheidbaren Paare ohne K5 gewinnt
   Ultra mindestens doppelt so oft wie v3 (`U ≥ 2·V`). Siegquote mit
   95-%-Intervall angeben.
2. **Keine neue Fehlerklasse:** Ultra-exklusiv ist eine Kategorie in einem
   Paar, wenn Ralf sie auf der Ultra-Seite markiert hat und nicht auf der
   v3-Seite. Fälle, die Ralf als „gleicher Fall wie …“ markiert, zählen einmal.
   Eine Klasse ist entstanden, wenn es ≥ 3 solcher Fälle derselben Kategorie
   K1–K3 gibt und v3 diese Kategorie weder in einem Paar noch in der Stichprobe
   gleicher Ausgaben hat. Gemeinsame Fehler der Stichprobe zählen für beide
   Modelle.
   **Veto:** ein einzelner Ultra-exklusiver K1-Fall, den Ralf auf der
   Ultra-Seite als gravierend markiert hat, etwa ein verlorener Satz, der auf
   der v3-Seite nicht fehlt. Ralf urteilt verdeckt.
3. Halluzinationen: Ultra nicht häufiger als v3, getrennt für „Herr
   Präsident“ und für das „Ich“ vor einem Befehl. 0 gegen 0 heißt nur „keine
   Verschlechterung beobachtet“.
4. **Leistung**, gepaart offline (WP5) auf denselben WAVs, mit demselben
   0.5.0-Binary, der gebündelten ORT-DLL, denselben Threads und beendetem Daemon.
   Je Modell ein Warmup, drei Durchgänge in wechselnder Reihenfolge:
   - Median Inferenz je Sekunde Audio ≤ +10 % gegenüber v3
   - p95 ≤ +20 % gegenüber v3
   - keine Ultra-exklusiven Fehler
   - Peak Working Set ≤ 2 GiB
5. **Betrieb:** Im Testzeitraum gab es keinen Ultra-exklusiven Fehlerzustand
   (Watchdog, Engine-Fehler), und das Rückweg-Gate aus WP3b ist bestanden.
6. **Ziffern:** Ralf sagt ausdrücklich Ja zur Zahlenschreibweise von Ultra.
   ✅ **Erteilt am 2026-10-01:** „Dass Zahlen als Ziffern ausgegeben werden, ist
   auch genau das, was ich möchte. Es nervt mich ehrlich gesagt ziemlich, dass
   bei der Diktatfunktion bisher immer die Zahlen ausgeschrieben wurden.
   Besonders bei Release-Nummern … wie zum Beispiel 2026.1 von Icaros.“ Für
   `-Resolve` heißt das `-Ziffern ja`. Paare mit „Unterschied nur
   Zahlenformat“ zählen in Kriterium 1 weiter als gleich (K5 ist nie ein
   Erkennungsgewinn); die Vorliebe wirkt nur über Kriterium 6.

Sonst: kein Wechsel oder „nicht belegt“.

## Datenschutz

- Audio, Rohtexte (JSONL), Gegenüberstellung, Schlüsseldatei und Notizen liegen
  ausschließlich unter `%LOCALAPPDATA%\diktier\ultra-test\` (nicht synchronisiert,
  außerhalb des Repos). Nichts davon kommt ins Repo, in `diktier.log` oder auf
  stderr.
- `docs/reviews/ultra-alltagstest-auswertung.md` und `docs/SPIKES.md` enthalten
  nur Kennzahlen und von Ralf einzeln freigegebene, anonymisierte Beispiele.
- Nach der Entscheidung legt Ralf fest, was gelöscht wird. Vorschlag: Audio und
  Texte löschen, Kennzahlen behalten.
- Das Vergleichsskript behandelt Texte als Daten: Markdown und Zeilenumbrüche
  werden maskiert, Ausgabe ist UTF-8.
- **Urteile speichern** (v2.1, Code-Review W2/L3): Die Vergleichsseite
  speichert die Urteile direkt in eine Datei, die Ralf beim ersten Speichern im
  Auswertungsordner anlegt (File System Access API), und schreibt fortlaufend
  dorthin. Einen automatischen Download-Ordner als Ausweg gibt es nicht; ohne
  Dateizugriff zeigt die Seite einen Fehler. Im Browser-Speicher liegen höchstens
  die Urteilscodes (A/B/Kategorien), **keine** Notizen. Ein Fehler beim Speichern
  ist sichtbar. `-Resolve` und die Messskripte akzeptieren nur Ordner und Dateien
  unter der Auswertungswurzel (bzw. der ausdrücklichen Testwurzel).

## Arbeitspakete

### ✅ WP0 — Artefakt lokal reproduzieren (keine Außenwirkung)

- `scripts/quantize-ultra.py` nach Leitentscheidung 4 mit Lockfile. Python- und
  Paketstand werden protokolliert.
- `NOTICE-parakeet-ultra.md` mit dieser Checkliste:
  - NVIDIA parakeet-tdt-0.6b-v3 als Ursprung
  - Moondream als Post-Training
  - altunenes als ONNX-Export (ohne VAD-Kopf)
  - eigene int8-per-channel-Quantisierung
  - je Stufe Link und Revision
  - CC-BY-4.0 mit Lizenzlink
  - erhaltene Hinweise der Quellen
  - Haftungsausschluss
  - Rezept und Hashes

  Dazu `SHA256SUMS`.
- Gate: Die drei Laufzeitartefakte sind bitgleich zu den Spike-Dateien.

### ✅ WP1 — Spec-Nachtrag (SPEC v1.10)

- §6.2: zwei Schlüssel. v3 ist Default, Ultra ist Testmodell. Unbekannt bleibt
  fatal.
- §6.3: Das v3-Golden-Set bleibt unverändert. Neu ist der Ultra-Artefaktsatz
  (Herkunft, Rezept, Größen, Hashes, Tag). Die URL-Regel wird verallgemeinert:
  unveränderlich heißt HF-Commit oder immutable GitHub-Release. Die Startprüfung
  bleibt Existenz und Größe, der Download prüft den vollen Hash; so steht es
  jetzt ausdrücklich da.
- §8: `[engine] model`, zwei Werte. §9: `--transcribe-list`, `--model`, JSONL
  und Exitcodes. §10: `DIKTIER_DEBUG_WAV_KEEP` (1–5000),
  `DIKTIER_DEBUG_WAV_DIR`, die Startzeile. §11: `versions.toml` mit
  Default und je einem Block pro Modell. §18 #16.

### ✅ WP2a — Mehrmodell-Vertrag (0.5.0, Teil 1)

- `models.toml` mit `[[models]]`. `load_manifest(key)`. Ein ausgewähltes Manifest
  läuft durch Daemon, Download, Engine, Tray und `model_artifacts(key)`.
- Config-Validierung gegen die Manifest-Schlüssel. Die Meldung nennt die
  erlaubten Werte.
- `release.ps1`: Alle Modelle werden strukturiert gelesen. `versions.toml` bekommt
  `default_model` und je Modell Quelle, Revision bzw. Tag, Dateien, Größen und
  Hashes. Nichts wird aus der Reihenfolge oder per HF-Regex erraten. Ein
  Bundle-Gate liest die erzeugte TOML ein und vergleicht sie mit dem Manifest.
- Tests:
  - Manifest: beide Dateisätze, eindeutige Schlüssel, sichere Verzeichnisnamen,
    v3-Werte einschließlich URLs byte-gleich.
  - Downloader mit kleinen Fakes für einen Drei-Datei-Satz.
  - Daemon/Tray parametrisiert für beide Schlüssel: `downloading → loading →
    idle`, Fehler ohne scharfen Hotkey und ohne heimlichen v3-Fallback.
  - Download-Sperre mit getrennten Verzeichnissen.
- v3-Golden-Set-Test und stt-smoke bleiben beim v3-Schlüssel.

### ✅ WP2b — Belegsammlung (0.5.0, Teil 2)

- `DIKTIER_DEBUG_WAV_KEEP` und `DIKTIER_DEBUG_WAV_DIR` werden beim Start gelesen.
  Ungültige Werte ergeben eine Warnung und den Default. Die Startzeile nennt den
  effektiven Zustand.
- Tests: Grenzen, ungültige Werte, Ring im eigenen Verzeichnis, fremde Dateien
  bleiben unberührt.

### ✅ WP2c — Vergleichswerkzeug (0.5.0, Teil 3)

- CLI nach Leitentscheidung 6, mit Parser- und Batch-Tests: Gate-Ablehnung, leer,
  Fehler mitten im Batch, Exitcode, `--runs`.
- `scripts/compare-models.ps1`:
  - Eingaben sind zwei JSONL-Läufe.
  - Es prüft Exitcodes und vollständige Paare.
  - Es baut den verdeckten Vergleich, die Schlüsseldatei und die Stichprobe
    gleicher Ausgaben.
  - Nach dem Urteil löst es auf und rechnet die Kennzahlen.
  - Ausgabeort ist fest `%LOCALAPPDATA%\diktier\ultra-test\`.
- `scripts/bench-models.ps1` für Kriterium 4, das Peak Working Set je Lauf
  eingeschlossen.
- README-Abschnitt „Alltagstest“ mit Umschalten, Rückweg, Sammlung und
  Auswertung. Die Ultra-NOTICE kommt ins Bundle nach `LICENSES/`, die v3-NOTICE
  bleibt. Version 0.5.0.

### ✅ WP2d — Lokales Ultra-Integrationsgate (vor jedem Push)

- Die Ultra-Artefakte aus WP0 werden in ein frisches Modellverzeichnis kopiert.
  0.5.0-Release-Build, gebündelte ORT 1.28.0, Produktionsthreads.
- `--transcribe-list` über Fixtures, Stille, Rauschen, die gesicherten Fälle und
  eine f32-WAV mit beiden Modellen. Stille und Rauschen bleiben leer, Sprache
  kommt an. Die Zahlen werden getrennt bewertet.
- Rückweg lokal: ein Ultra-Verzeichnis mit falscher Größe gibt einen sauberen
  Fehler; Config zurück auf v3 → läuft ohne Netz.
- Dazu die Gates `fmt`, `clippy -D warnings`, `cargo test`, stt-smoke (v3),
  Bundle-Gate. Anschließend das Code-Review durch Sol über WP2a–d.

### ✅ WP2e — Nacharbeit Code-Review

Befunde aus [reviews/impl-ultra-wp2-sol.md](reviews/impl-ultra-wp2-sol.md)
(K1, W1–W9, L1–L3), Bericht
[reviews/impl-ultra-wp2e-notes.md](reviews/impl-ultra-wp2e-notes.md),
Nachreview [reviews/impl-ultra-wp2-sol-2.md](reviews/impl-ultra-wp2-sol-2.md):
10 von 14 behoben. WP3a kann aus Code-Sicht starten; die Reste betreffen nur die
Auswertung.

### ✅ WP2f — Reste aus dem Nachreview (F1–F6, vor WP5)

Thread-Provenienz, Speichern der Seite, Reparse-Schutz der Schreibziele,
Zahlenformat im Urteil, Dauer und Verlängerung, echte TOML-Prüfung. Betrifft
nur die Auswertungswerkzeuge und `release.ps1`, nicht das Binary. Muss vor der
verbindlichen Auswertung (WP5) abgeschlossen und nachgeprüft sein. Umgesetzt
laut [reviews/impl-ultra-wp2f-notes.md](reviews/impl-ultra-wp2f-notes.md).
Nachreview [reviews/impl-ultra-wp2-sol-3.md](reviews/impl-ultra-wp2-sol-3.md):
F1–F6 behoben, Werkzeuge für WP5 abgenommen. Offen ist nur die Kleinigkeit G1
(Tests setzen `python` und ≥ 4 CPUs voraus), bewusst nicht behoben. **Praktisch
wichtig:** Die 7-Tage-Vorbereitung vor dem Ende von Tag 11 anlegen und
aufbewahren. Neu vorausgesetzt ist Python ≥ 3.11 für
`release.ps1` und das Thread-Urteil in `bench-models.ps1`.

### ✅ WP3a — Veröffentlichen (einzeln mit Ralfs Go)

- ✅ Schritt 1 (2026-10-01): [f6ae94f](https://github.com/ralfkuh-lab/diktier/commit/f6ae94f1bfec55a692f4a4bdf6e473390d3a5ccd)
  (0.5.0), [5657e12](https://github.com/ralfkuh-lab/diktier/commit/5657e123b417e8a7b337ea15d4288f6231f2f793)
  (NOTICE mit Rezept-Commit) und [357235d](https://github.com/ralfkuh-lab/diktier/commit/357235d1a39b50e9f47315cc8cd2730856227ea4)
  (TODO) sind auf `main` gepusht.
- ✅ Schritt 2: Ralf hat `ralfkuh-lab/diktier-models` angelegt, öffentlich, mit
  Immutable Releases, und `fsrakul` mit Write eingeladen. Die Einladung ist
  angenommen. Der erste Commit mit README und NOTICE ist
  [8ddc6a2](https://github.com/ralfkuh-lab/diktier-models/commit/8ddc6a2c64b25c74e5582085f85c6b194d1d5902).
- ✅ Schritte 3–4: Der Entwurf zeigt auf `8ddc6a2`. Fünf Assets mit Status
  `uploaded`, Größen wie im Manifest. Authentifiziert per Asset-API
  zurückgelesen, `sha256sum -c` grün, `SHA256SUMS` identisch zum Staging.
  Veröffentlicht am 2026-10-01T09:03:35Z mit `make_latest=false`, die API meldet
  `immutable: true`. Die öffentlichen URLs entsprechen exakt dem Manifest; anonym
  antworten sie mit 302 auf `release-assets.githubusercontent.com`.
- ✅ Schritt 5, abweichend vom Plan zusammen mit WP3b: Das anonyme
  Transport-Gate war der erste Ultra-Start des produktiven Daemons ohne Token,
  in ein frisches Modellverzeichnis. Ergebnis: „Modellartefakte vollständig und
  geprüft (79.7 s)“, danach „Modell geladen in 2.218 s
  (parakeet-ultra-0.6b-int8-pc)“. Die Abweichung ist vertretbar, weil v3
  unverändert und hashgeprüft auf der Platte lag und der Rückweg nur einen
  Neustart kostet.

### ✅ WP3b — Umstellen (mit Ralfs Go)

Stand 2026-10-01:

- ✅ Gesichert sind Logs, Umgebung (`DIKTIER_DEBUG_WAV=1`, `_KEEP` und `_DIR`
  vorher leer) und `config.toml` in `%LOCALAPPDATA%\diktier\ultra-test\vorher\`.
- ✅ Der v3-Vollcheck ist grün, 0.5.0 ist installiert.
- ✅ Umgestellt um 09:04 UTC: `DIKTIER_DEBUG_WAV_KEEP=5000` und
  `DIKTIER_DEBUG_WAV_DIR=%LOCALAPPDATA%\diktier\ultra-test\wav` als
  Benutzervariablen und in der Startumgebung, `engine.model =
  "parakeet-ultra-0.6b-int8-pc"`. Die Startzeile lautet „Debug-WAV an:
  …\ultra-test\wav, behalte 5000“.
- ✅ Rückweg-Gate live um 09:05 UTC: v3 lädt in 2,6 s ohne Netz, danach zurück
  auf Ultra (2,2 s). Die Config steht auf Ultra.
- ✅ Probediktat (Ralfs erstes Diktat) um 09:22:25Z, Lauf 1: Gate B1,
  Inferenz 0,539 s, eingefügt. `rec_2026-10-01T09-22-25-500Z_lauf-1.wav` liegt im
  Testverzeichnis, 16 kHz mono 32-bit-Float (WAVE_FORMAT_EXTENSIBLE), Größe
  passend zu 151200 Samples.
- **Testbeginn:** 2026-10-01T09:05:54Z (11:05:54 Ortszeit), der letzte
  Ultra-Start. Das Ende für `-Prepare` ist 2026-10-08 11:05:54 Ortszeit, bei
  Verlängerung 2026-10-12.

### ⏳ WP4 — Testlauf (7 Tage, Ralf)

- Normal diktieren. Auffälliges (UX, Latenz, Ziffern) kurz mit Uhrzeit in
  `%LOCALAPPDATA%\diktier\ultra-test\notizen.md` festhalten.
- Abbruch jederzeit: Config zurück auf v3, Daemon neu starten.
- Kein Kopieren nötig, der Ring liegt außerhalb von `%TEMP%`.

### 🔍 WP5 — Auswertung und Entscheidung

- Inventur: Läufe laut Log gegen WAVs; Lücken werden ausgewiesen.
- Beide Modelle über alle WAVs (`--transcribe-list`), dann der verdeckte
  Vergleich. Ralf urteilt, danach die Auflösung.
- `bench-models.ps1` für Kriterium 4.
- Bericht (nur Kennzahlen) nach `docs/reviews/ultra-alltagstest-auswertung.md`,
  dazu ein Block in `docs/SPIKES.md`. Ergebnis: Wechsel, kein Wechsel oder nicht
  belegt.
- Umgebung zurücksetzen (Variablen, Ringgröße) und über die Löschung
  entscheiden.

## Nach dem Test (eigenes Paket)

- **Wechsel:**
  - Default auf Ultra. Der v3-Schlüssel bleibt gültig, damit bestehende Configs
    nicht fatal werden.
  - SPEC §6.2/§6.3 neu fassen (der Golden-Set-Bezug auf Voxtype entfällt).
  - stt-smoke-Baselines für Ultra.
  - §18.
- **Kein Wechsel:** Die Config geht zurück auf v3. Ob der Ultra-Schlüssel
  bleibt, entscheidet Ralf. Das Release bleibt bestehen, wird aber nicht mehr
  referenziert.

## Risiken

- **Verfügbarkeit:** Immutable schützt Bytes und Tag. Das Release lässt sich
  trotzdem löschen, und der Tagname ist dann gesperrt. Der Rückweg auf v3 bleibt
  immer lokal möglich.
- **Transport:** Die URL-Form von GitHub kann sich ändern. WP3a Schritt 5 prüft
  den heutigen Stand. Ein automatischer Mirror oder ein „Latest“-Fallback ist
  nicht vorgesehen.
- **Befangenheit:** Wegen Live-Nutzung und Ziffern ist der Vergleich nur
  teilweise blind; das Ergebnis sagt es so.
- **Kleine Unterschiede:** Einzelne Wörter kippen schon durch 1 LSB Rauschen.
  Deshalb zählen Mindestmengen und Summen, nicht Einzelfälle.
- **Umfang:** 0.5.0 ist ein größeres Paket. Die Teilpakete WP2a–d sind einzeln
  abnehmbar.

## ✅ Entscheidungen (Ralf, 2026-09-30)

„Ja, mach alles so wie vorgeschlagen, nur die Testdauer mach ein bisschen
kürzer.“ Damit gilt:

- **F1:** Schlüssel `parakeet-ultra-0.6b-int8-pc`, ein Artefaktvertrag pro
  Schlüssel.
- **F2:** 7 Tage, einmal +4 nur bei fehlenden Mindestmengen. Die Mindestmengen
  bleiben unverändert.
- **F3:** verdecktes A/B wie im Protokoll.
- **F4:** `main` in WP3a pushen, keine historischen App-Releases nebenbei.
- **F5:** Kriterien 1–6.
- **F6:** eigenes öffentliches Repo `ralfkuh-lab/diktier-models`, dort
  immutable. Das App-Repo bleibt unverändert.

Die Schritte mit Außenwirkung in WP3a/WP3b (Repo anlegen, Push, Release,
Umstellen der Config und Umgebung) bleiben einzeln freigabepflichtig.
