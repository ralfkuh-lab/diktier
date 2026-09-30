Auftrag: WP2f, Nacharbeit aus dem Nachreview docs/reviews/impl-ultra-wp2-sol-2.md (Befunde F1–F6). Diktier, Windows-only. Basis ist der aktuelle Working Tree (0.5.0, WP0–WP2e uncommittet).

Lies zuerst:
1. docs/reviews/impl-ultra-wp2-sol-2.md, vollständig.
2. docs/reviews/impl-ultra-wp2e-notes.md (Aufbau der Skripte und Suite).
3. docs/ultra-alltagstest-plan.md: Bewertungsprotokoll, Abnahmekriterien, Datenschutz.

Umzusetzen. Wo Sol Alternativen nennt, gilt die Entscheidung hier:
- **F1:** `-ModelRoot` isoliert **beide** Pfade des Kindprozesses, `LOCALAPPDATA` und `APPDATA`, auf die Testwurzel. Die Provenienz liest die Config genau dort, wo Rust sie liest (`<APPDATA>\diktier\config.toml` des Kindes). Die effektive Thread-Zahl wird so ermittelt, wie `config.rs` sie auswertet: fehlender Schlüssel = Default 0. Bei Unsicherheit heißt es „nicht belegt“, nicht „Default“. Ohne `-ModelRoot` gilt das echte `%APPDATA%`. Regressionstest mit unterschiedlichem APPDATA und LOCALAPPDATA und widersprüchlichen Thread-Werten.
- **F2:** Die Seite verfolgt Änderungsgeneration und gespeicherte Generation getrennt. „Gespeichert“ zeigt sie erst nach erfolgreichem `close` des aktuellen Stands. `beforeunload` warnt bei jeder ungespeicherten, laufenden oder fehlgeschlagenen Speicherung. Tests mit verzögertem `close`, einer zweiten Änderung während des Schreibens und einem Schreibfehler danach.
- **F3:** Jeder konkrete Zielpfad wird vor dem Öffnen gegen die Reparse-Regel geprüft: die Zieldatei, falls vorhanden, und alle Pfadbestandteile. Das gilt zusätzlich zur lexikalischen Grenze. Regressionstest mit einem inneren Datei-Link nach außen: Abbruch, die externe Datei bleibt unverändert. Kann ein Test ohne Adminrechte keinen Symlink anlegen, nimm eine Junction oder überspringe ihn mit sichtbarer Meldung; im Bericht begründen.
- **F4:** Das Urteilsschema erfasst bei A/B ausdrücklich „Unterschied nur Zahlenformat“ als eigene Angabe, statt es aus den Kategorien abzuleiten. Solche Paare zählen in Kriterium 1 als „gleich“, auch wenn beide Seiten gemeinsame andere Fehler haben. Schema-Version erhöhen. Tests mit dem Szenario aus F4 und dem bisherigen K5-Fall.
- **F5:** `-Resolve` berechnet die Dauer aus den gespeicherten Grenzen, mit einer festgelegten Regel für Wanduhr und Zeitzone (Sommer-/Winterzeit darf nicht als Kurzlauf gelten), und sperrt Widersprüche zu `tage`. Die Verlängerung wird nur akzeptiert, wenn ein vorab festgehaltener 7-Tage-Mengenstand vorliegt, der die Mindestmengen nicht erreicht: `-Prepare` im verbindlichen Modus nach 7 Tagen schreibt ihn, bevor verlängert wird. Die Mengen (Paare mit Sprache, entscheidbare Paare) lassen sich dabei nicht ohne Urteile bestimmen. Vorschlag: Der Stand hält die vollständigen Paare mit Sprache und die Paare mit Textunterschied fest, als Obergrenze für entscheidbare Paare. Eine Verlängerung ist zulässig, wenn eine dieser Zahlen unter der Mindestmenge liegt. Wählst du eine andere belastbare Regel, begründe sie im Bericht. Tests: widersprüchliche Grenzen, unzulässige Verlängerung trotz erreichter Mengen, Zeitumstellung.
- **F6:** `release.ps1` prüft `src\models.toml` und die erzeugte `versions.toml` zusätzlich mit einem echten TOML-Parser. Vorgabe: Python 3.11+ `tomllib`, ist auf dem Build-Host vorhanden. Fehlt Python, bricht das Skript mit klarer Meldung ab, es gibt keinen stillen Verzicht. Negativfixture mit führender Null in der Suite.

Gates (Befehl und tatsächliche Summary-Zeile zitieren):
1. `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` (nur falls Rust-Code berührt ist; sonst begründen, warum nicht nötig)
2. `pwsh -File scripts\tests\Test-UltraScripts.ps1` grün, mit den neuen Tests
3. `python scripts\tests\test_quantize_ultra.py` weiter grün
4. `scripts\release.ps1 -SkipInstaller` grün, mit TOML-Prüfung
5. Ein echter Durchlauf `compare-models.ps1 -Prepare -Explorativ` mit `-ModelRoot .herd\wp2c-root` über einige WAVs, dazu `-Resolve` mit simuliertem `urteile.json` im neuen Schema. Die Seite headless prüfen, wie in WP2e.

Regeln:
- Nicht committen.
- Keine Änderungen an docs/ (außer deinem Bericht), testdata/, LICENSES/, src/models.toml. Rust-Code nur, wenn für F1 unumgänglich, und dann mit Begründung. Das Binary soll sich möglichst nicht ändern, weil es als 0.5.0 veröffentlicht wird.
- Nichts in `%LOCALAPPDATA%\diktier`, `%APPDATA%\diktier` oder `%TEMP%\diktier` schreiben, nur unter `.herd\` oder in selbst angelegten Temp-Verzeichnissen. Installierte Version und laufenden Daemon nicht anfassen, kein Netz.
- Große Dateien nie komplett in den Speicher laden. Tests nur mit kleinen Fakes.
- Kein herdr, keine weiteren Panes.
- Rückfragen: Frage nach `D:\DEV\diktier\.herd\fragen\impl-ultra-wp2f.md`, Turn mit `RUECKFRAGE: D:\DEV\diktier\.herd\fragen\impl-ultra-wp2f.md` beenden.

Bericht nach docs/reviews/impl-ultra-wp2f-notes.md: je Befund umgesetzt/abweichend, Gate-Ausgaben wörtlich, Offen. TUI-Ausgabe knapp.
