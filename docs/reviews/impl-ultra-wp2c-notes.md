# WP2c — Vergleichswerkzeug, Leistungsmessung, Doku (0.5.0, Teil 3): Umsetzungsbericht

Stand: 2026-09-30. Auftrag: [impl-ultra-wp2c-prompt.md](impl-ultra-wp2c-prompt.md).
Vorgabe waren SPEC v1.10 (§9, §10, §6.2/§6.3) und
[../ultra-alltagstest-plan.md](../ultra-alltagstest-plan.md) (Leitentscheidungen
5–7, Bewertungsprotokoll, Abnahmekriterien, Datenschutz, WP2c, WP5; Sol B1, B2,
W2, W7). Basis war der Working Tree auf `2d10ee0` mit WP0, WP1, WP2a und WP2b.
Kein Commit, kein Push. Im Bericht stehen keine Transkripttexte.

## ✅ Umgesetzt

### CLI (`src/main.rs`, neu `src/transcribe_list.rs`)

- `--transcribe-list <LISTE>` liest UTF-8 mit einer WAV je Zeile. Ein BOM am
  Anfang, CRLF, Leerzeilen und Leerraum an den Rändern werden ignoriert. Ist die
  Liste nicht lesbar, endet der Aufruf mit Exit 1. Kein UTF-8 oder keine einzige
  Datei ergibt Exit 2.
- `--model <SCHLÜSSEL>` gilt für `--transcribe-wav` und `--transcribe-list`.
  Geprüft wird gegen `download::model_keys()`, und zwar vor Config und Datei. Ein
  unbekannter Schlüssel ergibt Exit 2 mit der Liste der erlaubten Werte.
  `--model` ohne Transkriptionsmodus ergibt ebenfalls Exit 2. Die Config wird nur
  gelesen, nie geschrieben; `--model` ersetzt `engine.model` nur für diesen
  Aufruf.
- `--runs` gilt jetzt für beide Modi. Es ist ein `Option<u32>`, damit
  `--transcribe-list` erkennt, ob `run` in die Zeile gehört. `--transcribe-wav`
  nimmt ohne `--runs` wie bisher 1.
- Clap-Konflikte: `--transcribe-list` schließt `--transcribe-wav`,
  `--gate-analyze`, `--clipboard-check`, Autostart und alle Spikes aus.
- Batch (`transcribe_list::run_batch`, generisch über Loader und Writer, dadurch
  mit Stub-Engine testbar):
  - Die Dateien laufen in Listenreihenfolge. Je Datei wird gelesen, der Gate
    gerechnet und der Report nach stderr geschrieben (`<datei>: Gate: …`).
  - Das Modell lädt erst, wenn die erste Datei den Gate passiert, und nur
    einmal. Danach folgt ein ungezählter Warmup auf dieser Datei.
  - Gemessen wird `transcribe_pcm`, also dieselbe Stelle mit Vorlauf-Stille wie
    Daemon und `--transcribe-wav`.
  - JSONL über `serde_json` in der Reihenfolge `file, status, text, infer_ms,
    samples`, mit `--runs` dazu `run`.
  - Nach jeder Zeile wird geflusht, damit fertige Zeilen einen Abbruch
    überstehen.
  - Ist eine Datei `error`, gibt es Exit 1, die übrigen Dateien laufen
    trotzdem.
- `serde_json = "1"` ist jetzt direkte Abhängigkeit. 1.0.151 stand schon
  transitiv über parakeet-rs im Lock; der Lock bekommt nur den Eintrag in der
  `diktier`-Abhängigkeitsliste.
- `--transcribe-wav` ohne `--model` und ohne `--runs` ist unverändert, siehe
  Gate 5.

Tests (15 neue, alle ohne echtes Modell):

- `transcribe_list::tests`:
  - `list_ignores_blank_lines_bom_and_crlf`
  - `list_file_errors_have_exit_codes`: fehlt → 1, leer → 2, Latin-1 → 2,
    Umlaute im Pfad ok.
  - `states_text_rejected_error`: alle drei Zustände, Feldreihenfolge,
    Diagnose mit Dateibezug und ohne Text, ein Laden, Warmup plus Lauf.
  - `jsonl_escapes_quotes_tab_newline_and_keeps_umlauts`: `\"`, `\t`, `\n`,
    `\\`, Umlaute bleiben lesbar, eine Zeile, Roundtrip gleich.
  - `engine_error_mid_batch_exits_1_with_complete_output`: text / error /
    text, Exit 1.
  - `runs_emit_numbered_lines_per_file_after_one_warmup`: 3 Dateien × 3 Zeilen,
    `run` 1…3, genau 1 Warmup.
  - `only_rejected_files_never_load_the_model`: Der Loader würde scheitern,
    wird aber nie gerufen.
  - `failed_model_load_marks_speech_as_error_once`
  - `empty_engine_output_is_text_not_rejected`
  - `failed_warmup_exits_1`
- `main.rs`:
  - `model_without_transcription_mode_exits_2`
  - `unknown_model_key_exits_2`: auch bei falscher Groß-/Kleinschreibung.
  - `transcribe_list_parser_conflicts_exit_2`
  - `transcribe_list_unreadable_or_empty_list`
  - `transcribe_list_with_only_rejected_files_needs_no_model`: Ende zu Ende mit
    `--model` Ultra und `--runs 2` ohne installiertes Ultra. Dazu
    `--transcribe-wav --model` Ultra auf Stille.
  - `runs_without_transcribe_wav_exits_2` erweitert um `--runs 0`.

### `scripts/compare-models.ps1` (+ `compare-models.html`, `ultra-test-lib.ps1`)

- **Ausgabe** fest unter `<Wurzel>\diktier\ultra-test\auswertung\<yyyyMMdd-HHmmss>\`.
  `<Wurzel>` ist `%LOCALAPPDATA%` oder `-ModelRoot`. Ein vorhandener Ordner
  bricht ab.
- **`-Prepare`:**
  - **Inventur:** Erwartet sind die `Lauf N: Gate:`-Zeilen aus `diktier.log.1`
    und `diktier.log` im Zeitraum `-From`/`-To` (optional, lokale Zeit ohne
    Zone). Weil die Laufnummer je Daemonstart neu beginnt, ist der Schlüssel
    `(letzte Startzeile „diktier … startet (Daemon“ ≤ t, Lauf)`. Vorhanden sind
    die `rec_<UTC>_lauf-<N>[-<k>].wav` im Ring. Ausgegeben werden erwartet,
    vorhanden und fehlend (mit Sitzung und Laufnummer), dazu WAVs ohne
    Logeintrag, ohne Laufnummer und mit Namenssuffix.
  - Bytegleiche WAVs (SHA-256) werden nur einmal gewertet, Grund „Duplikat“.
  - `liste.txt` wird als UTF-8 ohne BOM geschrieben. Dann läuft je Modell
    `diktier.exe --transcribe-list … --model <key>` als `Process` mit
    UTF-8-Umleitung: stdout nach `roh-<key>.jsonl`, stderr nach
    `diagnose-<key>.txt`.
  - Nur Exit 0 und 1 werden weiterverarbeitet, alles andere bricht ab. Exit 1
    ohne `error`-Zeile oder Exit 0 mit `error`-Zeile bricht ebenfalls ab.
  - Das JSONL wird gegen die Liste geprüft: Zeilenzahl, Reihenfolge, `file`,
    Status.
  - Paare: Weiter gehen nur Dateien mit `text` bei beiden Modellen und
    mindestens einem nicht leeren Text. Ausschlüsse werden mit Grund gezählt:
    Duplikat, Fehler v3/Ultra/beide, Gate-Ablehnung beide, Gate uneinheitlich,
    beide leer.
  - Zufall aus `System.Random(seed)`. Ohne `-Seed` wird ein Seed kryptographisch
    gezogen. Gemischt werden Reihenfolge der Paare, A/B je Paar, die Stichprobe
    von 20 gleichen Ausgaben und die Reihenfolge der „Herr Präsident“-Fälle.
  - Seed und Zuordnung stehen **nur** in `schluessel.json`. Die Seite enthält
    weder Modellnamen noch Seed; geprüft, siehe unten.
  - `vorbereitung.json` enthält Inventur, Exitcodes, Ausschlüsse und Mengen.
- **`vergleich.html`:** Die Vorlage `scripts/compare-models.html` bekommt die
  Daten als JSON-Block, erzeugt mit `ConvertTo-Json -EscapeHandling
  EscapeHtml`. Damit steht `< > & ' "` als `\uXXXX` im Block, und kein Text
  kann den `<script>` verlassen.
  - Im DOM landen Texte nur über `textContent`/`append`, nie über `innerHTML`.
  - CSP per Meta: `default-src 'none'`, nur Inline-Skript und -Stil, `media-src
    file:`. Keine externe Ressource.
  - Block 1: A/B-Texte mit LCS-Wortdiff, Audio-Player (`file:///…`, per
    `Uri.AbsoluteUri` kodiert), Urteil A / B / gleich / unklar. Bei A/B sind
    Kategorie K1–K5 und „gravierend“ ja/nein Pflicht, eine Notiz ist optional.
  - Block 2: Stichprobe mit Text, Audio und „stimmt / Fehler (Kategorie)“.
  - Block 3: Audio und „gesprochen ja/nein“.
  - Der Zwischenstand liegt im localStorage unter einem Schlüssel je
    Auswertung. Es gibt den Knopf „Zwischenstand löschen“ und eine
    Fortschrittsanzeige.
  - Der Export schreibt `urteile.json` über `showSaveFilePicker`, sonst als
    Download. Die Seite nennt den Auswertungsordner als Ziel.
- **`-Resolve <ordner> -Judgments <urteile.json> [-Ziffern ja|nein|offen]`:**
  - Aufgelöst wird nur, wenn die Auswertungs-ID passt und alle Urteile
    vollständig sind: jedes Paar, bei A/B mit Kategorie und „gravierend“, jede
    Stichprobe, jeder „Herr Präsident“-Fall.
  - Kriterium 1: Die Mindestmengen sind ≥ 300 vollständige Paare mit Sprache
    und ≥ 50 entscheidbare Paare. Gezählt wird U/V ohne K5, K5 zählt als
    gleich. Dazu die Quote mit 95-%-Wilson-Intervall. Das Ergebnis ist
    erfüllt, nicht erfüllt oder nicht belegt.
  - Kriterium 2: Kategorien der schlechteren Seite je Modell. „Neue
    Fehlerklasse“ heißt K1–K3 mit Ultra ≥ 3 und v3 = 0. Veto ist, wenn Ultra
    mit K1 verliert und der Fall gravierend ist.
  - Kriterium 3: Halluzinationen je Modell, also Text beginnt mit „Herr
    Präsident“ und es war laut Audio nicht gesprochen. 0 gegen 0 steht als
    „keine Verschlechterung beobachtet“ da.
  - Die Stichprobe gleicher Ausgaben wird nach Kategorien gezählt.
  - Kriterium 6 kommt aus `-Ziffern`, Default `offen`.
  - `zusammenfassung.md` enthält nur Zahlen, ohne Transkripte, Pfade oder
    Notizen.
  - `bericht.md` ist lokal und enthält Texte und Notizen. Sie sind als Daten
    maskiert: Markdown-Metazeichen mit `\`, Zeilenumbruch als ⏎, Tab als ⇥.
- Konsole: nur Zahlen und Pfade. stderr von `diktier.exe` (Gate-Reports, ohne
  Text) geht in den Auswertungsordner.

### `scripts/bench-models.ps1`

- Ist ein `diktier`-Prozess aktiv, bricht das Skript mit PID und Hinweis ab
  (Exit 3). Es beendet nichts.
- Aufruf: `-Evaluation <auswertungsordner>` mit dessen `liste.txt` (oder
  `-List`), `-Exe`, `-ModelRoot`, `-Runs 3`, `-Passes 3`.
- Je Durchgang und Modell läuft ein eigener Prozess `--transcribe-list --runs
  3`. Die Reihenfolge wechselt: v3→Ultra, Ultra→v3, v3→Ultra.
- Gewertet wird `infer_ms / (samples/16000)` nur über Dateien, bei denen beide
  Modelle in **allen** Läufen `text` liefern. Daraus Median und p95
  (Nearest-Rank).
- Fehler: `error`-Zeilen je Modell und Ultra-exklusive Fehlerdateien.
- Peak Working Set je Prozess.
- Die Prüfungen nach Kriterium 4 (Median ≤ +10 %, p95 ≤ +20 %, keine
  Ultra-exklusiven Fehler, Peak ≤ 2 GiB) landen in `bench.json` und `bench.md`
  (nur Zahlen). Die Roh-JSONL liegen in `<auswertungsordner>\bench-<zeit>\`.

### Modellwurzel für Tests (`-ModelRoot`, ohne Produktänderung)

`Invoke-DiktierList` setzt `LOCALAPPDATA` nur in `ProcessStartInfo.Environment`
des Kindprozesses. Beide Skripte leiten ihre Default-Pfade von derselben Wurzel
ab. Dazu gehören Aufnahmen, Log und Auswertung. Mit `-ModelRoot` bleibt so das
echte `%LOCALAPPDATA%\diktier` unberührt. Vorab wird geprüft, ob beide
Modellverzeichnisse existieren.

### README, release.ps1

- README, neuer Abschnitt „Alltagstest (Ultra)“:
  - Umschalten per `[engine] model`, Rückweg auf v3
  - die drei Variablen aus Leitentscheidung 5 und dass der Daemon sie nur beim
    Start liest
  - Ablage unter `%LOCALAPPDATA%\diktier\ultra-test\`, mit Datenschutzhinweis
    (auch zum localStorage)
  - Aufrufe von `compare-models.ps1` und `bench-models.ps1`
  - Lizenznennung mit der Kette und dem Verweis auf die NOTICE
- README, weitere Stellen: Ein Satz in „Konfiguration“ lautete „Das
  Sprachmodell ist fest“. Er verweist jetzt auf `[engine] model`. Im Abschnitt
  „Lizenz“ steht eine Ultra-Zeile. Den Debug-WAV-Abschnitt habe ich nicht
  angefasst.
- `scripts/release.ps1`: `LICENSES\NOTICE-parakeet-ultra.md` ist
  Pflichtdatei der Selbstprüfung, die v3-NOTICE bleibt. Der Kopfkommentar nennt
  beide NOTICEs. Die Datei bleibt bei BOM und CRLF.
  - Negativprobe: Die Selbstprüfungsschleife lief aus dem Skript heraus gegen
    eine Bundle-Kopie ohne Ultra-NOTICE (Scratchpad). Ergebnis:
    `release.ps1: Bundle unvollständig: LICENSES\NOTICE-parakeet-ultra.md`.

### Beleg mit echtem Lauf (Binary `target-dev\release\diktier.exe`)

**Temp-Wurzel** `.herd\wp2c-root\` (git-ausgeschlossen über `.git/info/exclude`):

- `diktier\models\parakeet-tdt-0.6b-v3-int8\` ist eine Kopie des installierten
  Verzeichnisses. Das Original wurde nur gelesen.
- `diktier\models\parakeet-ultra-0.6b-int8-pc\` enthält Encoder, Decoder und
  `vocab.txt` aus `.herd\model-release\model-parakeet-ultra-0.6b-int8-pc-r1\`,
  ohne NOTICE und SHA256SUMS. Dazu kommt `COMPLETE` mit
  `parakeet-ultra-0.6b-int8-pc\n` (UTF-8, LF), wie `download::write_marker` es
  schreibt.
- SHA-256 nach dem Kopieren: `2cc01c15…`, `afcb9459…`, `d5854467…`.

**Probe für `-Prepare`:**

- 10 WAVs, als `rec_<UTC>_lauf-<N>.wav` benannt:
  - aus `testdata\stt\`: alltag (zweimal, bytegleich), zahlen_umlaute,
    fachwoerter, stille, rauschen
  - aus `testdata\stt\local\`: 09_normal_referenz
  - aus `local\herr_praesident\`: drei Fälle
- Ein Fall liegt vor dem Zeitraum und wird nicht mitgezählt.
- Das synthetische `diktier.log` hat zwei Sitzungen. Die Laufnummern beginnen
  in jeder Sitzung neu, Sitzung 1 Lauf 7 hat keine WAV.

Konsolenausgabe:

```
== Inventur (diktier.log)
   erwartet 10, vorhanden 9, fehlend 1
   fehlt: Sitzung 2026-10-01T08:00:00Z, Lauf 7
   WAVs ohne Logeintrag 0, ohne Laufnummer im Namen 0, mit Namenssuffix 0
== parakeet-tdt-0.6b-v3-int8 über 9 Dateien
   Exitcode 0
== parakeet-ultra-0.6b-int8-pc über 9 Dateien
   Exitcode 0
== Paare: vollständig mit Sprache 6 (gleich 3, verschieden 3)
   ausgeschlossen: Gate-Ablehnung (beide) 2
   ausgeschlossen: Duplikat (bytegleiche WAV) 1
== Seite D:\DEV\diktier\.herd\wp2c-root\diktier\ultra-test\auswertung\20260930-190500\vergleich.html
   3 Paare, 3 Stichprobe, 0 »Herr Präsident«
```

**Prüfung der HTML-Seite** (sie öffnet sich nicht automatisch):

1. Struktur und Einbettung, Python-Skript im Scratchpad:
   - Tags balanciert (`html.parser`), Doctype vorhanden
   - 0 externe `src`/`href`, 0 `http(s)://`
   - Im JSON-Block steht kein `<`, `>` oder `&`.
   - Jeder A/B-Text ist bitgleich zu `roh-<A/B-Modell>.jsonl` laut
     `schluessel.json`, die Stichprobentexte sind bei beiden Modellen gleich.
   - Modellnamen und Seed kommen in der Seite nicht vor, alle Audio-URLs sind
     `file:///`.
   - `node --check` auf das Seitenskript ist ok.
2. Headless Chrome (`--dump-dom`, eigenes Profil im Scratchpad):
   - Die echte Seite rendert 6 Karten, 6 Audio-Elemente und die
     Fortschrittszeile `Paare 0/3 · Stichprobe 0/3 · Herr Präsident 0/0`.
   - Injektionsprobe mit gleichem Escaping und einem Text aus
     `</script><script>…</script><img onerror=…>`, Anführungszeichen,
     Zeilenumbruch, `*`, `|` und `#`: kein `<img>` im DOM, Titel unverändert,
     der Text steht als `&lt;/script&gt;…` im DOM, 2 Diff-Spans.
3. Bedienung simuliert: Headless Chrome mit einem angehängten Testskript.
   - Es setzt Radio- und Select-Werte per `change`-Event und ruft das
     `exportDoc()` der Seite.
   - Der Export war vollständig (`vollstaendig: true`).
   - Nach einem erneuten `loadState()` aus dem localStorage war der Stand
     gleich.
   - Dieser Export ist das **simulierte `urteile.json`**: P001 A/K1/gravierend
     mit Notiz, P002 B/K4, P003 gleich; S01 stimmt, S02 Fehler K2, S03 stimmt.

**`-Resolve`** mit diesem `urteile.json`:

```
   U 2, V 0, Quote 100.0 %, Mindestmengen nicht erfüllt
   Kriterium 1: nicht belegt · 2: erfüllt · 3: erfüllt (0 gegen 0: keine Verschlechterung beobachtet) · 6: offen (Ralfs ausdrückliches Ja steht aus)
```

- Die Wilson-Grenzen für 2/2 sind 34,2 % bis 100 %.
- Abgebrochen wird, wie gewollt, bei fehlendem Paar-Urteil, bei fremder
  Auswertungs-ID und bei A ohne Kategorie.
- Simulierter Sonderfall in einer Kopie des Ordners:
  - P003 geht an v3 mit K1 gravierend, P002 wird K5.
  - Dazu kommt ein künstlicher „Herr Präsident“-Eintrag in `schluessel.json`:
    nur v3, nicht gesprochen.
  - Ergebnis: U 1, V 1, K5 als gleich 1, Veto 1, Kriterium 2 „nicht
    erfüllt“, Halluzinationen v3 1 / Ultra 0, Kriterium 3 „erfüllt“, mit
    `-Ziffern ja` Kriterium 6 „erfüllt“.
- Maschinell geprüft: Kein Transkript und keine Notiz steht in
  `zusammenfassung.md`, auch kein Pfad und kein `.wav`. Die Notiz steht im
  `bericht.md` maskiert.

**`bench-models.ps1`:**

- Echter Aufruf:
  ```
  bench-models.ps1: diktier.exe läuft (PID 13084).
  Für eine faire Messung den Daemon über das Tray-Menü beenden und neu aufrufen. Das Skript beendet ihn nicht.
  ```
  Exit 3. Der Daemon lief, das Skript hat wie verlangt abgebrochen, ich habe
  ihn nicht angefasst.
- Den Messpfad habe ich mit einer Scratchpad-Kopie des Skripts belegt. In
  dieser Kopie war nur die Daemonprüfung ersetzt, alles andere
  unverändert, mit `-ModelRoot`:
  ```
     Durchgang 1 parakeet-tdt-0.6b-v3-int8: Exit 0, 14,4 s, Peak WS 883 MiB
     Durchgang 1 parakeet-ultra-0.6b-int8-pc: Exit 0, 13,8 s, Peak WS 982 MiB
     Durchgang 2 parakeet-ultra-0.6b-int8-pc: Exit 0, 14,1 s, Peak WS 983 MiB
     Durchgang 2 parakeet-tdt-0.6b-v3-int8: Exit 0, 14,3 s, Peak WS 883 MiB
     Durchgang 3 parakeet-tdt-0.6b-v3-int8: Exit 0, 14,1 s, Peak WS 883 MiB
     Durchgang 3 parakeet-ultra-0.6b-int8-pc: Exit 0, 14,0 s, Peak WS 983 MiB
     Median 54.0 → 53.5 ms/s (-0.9 %), p95 60.1 → 58.4 ms/s (-2.8 %), Fehler 0/0, Peak 883/983 MiB
     Kriterium 4: erfüllt
  ```
  9 Dateien, davon 7 mit Text bei beiden Modellen, 63 Messungen je Modell.
- **Die Zahlen sind nicht aussagekräftig:** Der Daemon lief parallel, und die
  Menge ist winzig. Sie belegen nur, dass Messpfad, Paarung, Statistik und
  Speichermessung funktionieren.

## Abweichungen und warum

- **`--transcribe-wav --model` gibt weiter nur Text aus, kein JSONL.** SPEC §9
  legt JSONL nur für `--transcribe-list` fest. Der Satz „`--transcribe-wav`
  ohne `--model` gibt wie bisher nur den Text aus“ sagt nicht, was mit
  `--model` kommt. Ich habe `--model` als reinen Modell-Override gelesen, für
  den Einzeltest am schnellsten brauchbar. Soll es dort JSONL geben, ist das
  eine kleine Änderung.
- **`--runs n` ergibt genau n Zeilen je Datei, auch bei `rejected` und
  `error`.** So hängt die Zeilenzahl nicht vom Status ab, und Skripte können
  sie prüfen (`Read-DiktierJsonl`). SPEC: „n Zeilen je Datei“.
- **Nullwerte:** `infer_ms` ist bei `rejected`/`error` `null`. `samples` ist
  `null`, wenn die Datei nicht lesbar war. `infer_ms` misst den Aufruf
  `transcribe_pcm`, also Gate-Nachrechnung, Vorlauf und Engine, auf µs
  gerundet. Der Gate-Anteil ist gegenüber der Inferenz vernachlässigbar und für
  beide Modelle gleich.
- **Warmup scheitert → Exit 1**, auch wenn die Messläufe danach gelingen. Ein
  scheiternder Warmup ist ein Engine-Fehler des Laufs.
- **Leere Liste → Exit 2** (Bedienfehler) statt einer leeren Ausgabe mit 0.
- **Peak Working Set:** `Process.PeakWorkingSet64` liefert unter .NET nach
  Prozessende 0; geprüft mit einem Testprozess. Deshalb rufe ich
  `GetProcessMemoryInfo` auf dem noch offenen Prozesshandle auf; das ergab für
  denselben Testprozess 278 MiB. Der Wert entspricht `PeakWorkingSetSize` aus
  `PROCESS_MEMORY_COUNTERS`.
- **`-ModelRoot` ersetzt die ganze `LOCALAPPDATA`-Wurzel**, nicht nur das
  Modellverzeichnis. Daran hängen auch die Default-Pfade der Skripte. Nur so
  schreibt ein Probelauf nichts unter das echte `%LOCALAPPDATA%\diktier`.
- **PowerShell 7 ist Pflicht** (`#Requires -Version 7.0`). Gebraucht werden
  `ConvertTo-Json -EscapeHandling`, `ConvertFrom-Json -AsHashtable` und
  `ProcessStartInfo.ArgumentList`.
- **Anführungszeichen in den Skripten:** PowerShell wertet „ und “ als
  String-Begrenzer. In `.ps1`-Texten stehen deshalb » «. Die HTML-Seite nutzt
  „ “.
- **Block 3 zeigt nur Audio**, keine Texte: Die Frage ist, ob gesprochen wurde.
  Ohne Texte verrät der Block nicht, welches Modell die Phrase hatte.
- **Erkennung „Herr Präsident“:** Der getrimmte Text beginnt mit „Herr
  Präsident“, Groß-/Kleinschreibung egal. Gezählt werden nur Dateien, die nicht
  aus anderem Grund ausgeschlossen sind (Paar vollständig). Eine Datei mit
  `error` bei einem Modell fällt hier heraus.
- **Duplikate** sind bytegleiche WAVs, per SHA-256 erkannt.
  `bench-models.ps1` misst die Liste wie gegeben, ein Duplikat also doppelt;
  für die Latenz ist das unerheblich.
- **Kriterium 6** lässt sich nicht aus den Urteilen ableiten. Es kommt als
  Parameter `-Ziffern ja|nein|offen`, und die K5-Anzahl steht daneben.
- **bench-Abbruch mit Exit 3** unterscheidet den verweigerten Start von einem
  Fehler (1) und einer Bedienfrage (2).
- **Belegumfang bench:** Der echte Lauf hat abgebrochen, weil der Daemon lief.
  Der Messpfad ist deshalb nur über die Scratchpad-Kopie ohne Daemonprüfung
  belegt, siehe oben.
- **„Herr Präsident“ im echten Lauf nicht auslösbar:** Mit 0.5.0 und 300 ms
  Vorlauf-Stille beginnt bei keinem Modell ein Text mit „Herr Präsident“. Das
  gilt für die zehn `herr_praesident`-Fixtures einzeln (je 12 Zeilen, alle
  `text`). Block 3 und die Halluzinationszählung sind deshalb nur über den
  simulierten Eintrag und die Injektionsprobe belegt.

## Gate-Ausgaben (wörtlich)

1. `cargo fmt --check`: keine Ausgabe, `fmt exit=0`.
2. `cargo clippy --all-targets -- -D warnings`:
   ```
       Checking diktier v0.5.0 (D:\DEV\diktier)
       Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.42s
   ```
   Keine Meldung, `clippy exit=0`.
3. `cargo test`:
   ```
   test result: ok. 550 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 2.02s
   ```
4. `$env:CARGO_TARGET_DIR="target-dev"; cargo build --release`: Der Build
   nach der letzten Rust-Änderung ergab
   ```
      Compiling diktier v0.5.0 (D:\DEV\diktier)
       Finished `release` profile [optimized] target(s) in 16.80s
   ```
   Die Wiederholung vor Gate 5 ergab `Finished `release` profile [optimized]
   target(s) in 0.22s`, `build exit=0`. Alle Skriptbelege oben liefen mit
   diesem Binary (ProductVersion 0.5.0).
5. `--transcribe-wav testdata\stt\alltag.wav`, stdout per `Start-Process
   -RedirectStandardOutput`:
   ```
   dev: exit 0, stdout 139 B, sha256 D346DA30D6A66B36
   inst: exit 0, stdout 139 B, sha256 D346DA30D6A66B36
   byte-gleich: True
   ```
   Installiert ist 0.4.1 unter `%LOCALAPPDATA%\Programs\Diktier\`.
6. `scripts\release.ps1 -SkipInstaller`:
   ```
   == Diktier 0.5.0 (win-x64), TargetDir=target
   == cargo build --release --locked (CARGO_TARGET_DIR=target)
   == Bundle D:\DEV\diktier\dist\diktier-0.5.0-win-x64
   == Modelle: parakeet-tdt-0.6b-v3-int8, parakeet-ultra-0.6b-int8-pc (Default parakeet-tdt-0.6b-v3-int8)
   == Bundle-Gate: versions.toml gegen src\models.toml
      ok parakeet-tdt-0.6b-v3-int8: huggingface revision=8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce, 4 Dateien
      ok parakeet-ultra-0.6b-int8-pc: github-release release_tag=model-parakeet-ultra-0.6b-int8-pc-r1, 3 Dateien
   == Selbstprüfung
   == Zip D:\DEV\diktier\dist\diktier-0.5.0-win-x64.zip
   == Installer übersprungen (-SkipInstaller)
   ```
   `release exit=0`. Die Datei `LICENSES\NOTICE-parakeet-ultra.md` liegt im
   Bundle (`Test-Path` True). Die Negativprobe steht oben.

## 🔍 Offen

- **„Ich“ vor einem Befehl** (Bewertungsprotokoll, „Halluzinationen“) zählt das
  Werkzeug nicht. Automatisch ist es nicht sicher zu erkennen. Ohne eigene
  Frage in der Seite bleibt es ein Fall für Kategorie K1 und die Notiz. Soll es
  einen eigenen Block bekommen?
- **`--transcribe-wav --model`:** Text oder JSONL, siehe Abweichungen.
- **Echter bench-Lauf** mit beendetem Daemon: gehört zu WP2d/WP5.
- **`.herd\wp2c-root\`** belegt rund 1,4 GB mit den Modellkopien. Löschen oder
  für WP2d weiterverwenden.
- Die Skripte nutzen zwei Logzeilen als Vertrag: „diktier … startet (Daemon“
  und „Lauf N: Gate:“. Ändert sich ihr Wortlaut, stimmt die Inventur nicht
  mehr. Ein Test dafür existiert nicht.
