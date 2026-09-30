# Nachreview WP2f — GPT-6.1 Sol

Stand: 2026-09-30, Working Tree 0.5.0 auf `HEAD 2d10ee0`.
Grundlage: `impl-ultra-wp2-review3-prompt.md`, WP2f-Bericht, vorheriges
Sol-Nachreview und Alltagstest-Plan v2.2.

**Ergebnis: F1–F6 sind behoben. Die Auswertungswerkzeuge können für WP5
abgenommen werden.** Kein neuer blockierender Befund; eine Kleinigkeit
betrifft die Portabilität der Regressionstests, nicht die Auswertung auf
diesem Rechner.

## Status F1–F6

| Befund | Status | Begründung |
|---|---|---|
| F1 — tatsächliche Config/Threads | **behoben** | `ultra-test-lib.ps1:101–175,332,363`: beide Benutzerpfade werden bei `-ModelRoot` auf die aufgelöste Testwurzel gesetzt; ohne Override wird APPDATA verwendet. Echter TOML-Parser, Typ-/Bereichsprüfung und explizit unbelegte Ergebnisse statt geratenem Default. `bench-models.ps1:137–144,183–195` prüft zusätzlich den Config-Stand nach den Messungen. Gegenläufige APPDATA-/LOCALAPPDATA-Fixtures und fehlender Parser sind getestet. |
| F2 — laufende/gescheiterte Speicherung | **behoben** | `compare-models.html:175–207,264–308,556–559`: Änderungs-, Speicher- und Fehlergeneration sowie laufende Queue getrennt; gespeichert erst nach erfolgreichem `close`; veraltete Datei-Epochen ändern den aktuellen Speicherstand nicht. Unpersistierte Änderungen und laufende Vorgänge lösen die Schließwarnung aus. Browser-Test prüft verzögertes `close`, zweite Änderung, Fehler und Wiederherstellung. |
| F3 — tatsächliches Schreibziel | **behoben** | `ultra-test-lib.ps1:192–244,270–294`: Reparse-Prüfung je konkretem Ziel und allen relevanten Pfadbestandteilen; nicht prüfbare Pfade werden gesperrt. Vorhandene mehrfach verlinkte Zieldateien werden zusätzlich abgelehnt. `compare-models.ps1:688–690` prüft alle drei Resolve-Ausgaben vorab. Junction-, verwaiste-Junction- und Hardlink-Tests sind grün; Datei-Symlink-Test sichtbar übersprungen, siehe Gates. |
| F4 — K5 mit gemeinsamem Fehler | **behoben** | `compare-models.ps1:739–763,810–819`, `compare-models.html:418–428,448–455`: explizites Pflichtfeld `nur_zahlenformat` bei A/B; identische Konsistenzregeln in Browser und Resolver. „Ja“ zählt unabhängig von gemeinsamen Kategorien als gleich. Format 2 wird abgelehnt. Gemeinsames K4 plus K5, Gegenprobe „nein“ und widersprüchliche Angaben sind getestet; entspricht v2.2. |
| F5 — Dauer/Verlängerungsgrund | **behoben** | `ultra-test-lib.ps1:656–668`, `compare-models.ps1:259–293,427–440,642–676`: Dauer aus UTC-Grenzen in gespeicherter Zeitzone, Abgleich mit `tage`; Verlängerung braucht eine rechtzeitig erstellte Siebentage-Grundlage mit gleichem Start/Aufnahmeverzeichnis und fehlender Menge. Resolve prüft sie erneut. Ein-/siebentägige Manipulation, Zeitumstellung und unzulässige/zulässige Verlängerung sind getestet. Die Obergrenze „Paare mit Textunterschied“ statt späterer A/B-Zahl ist ausdrücklich durch v2.2 genehmigt. |
| F6 — unabhängige TOML-Prüfung | **behoben** | `release.ps1:323–330,401–404,522–524`: Quellenmanifest vor dem Build und erzeugte `versions.toml` mit `tomllib` prüfen und vollständig gegen die Teilmenge vergleichen. `toml-json.py`/`toml-lib.ps1` erhalten Schlüssel, Typen und Integer ohne JSON-Umdeutung. Ohne geeigneten Parser expliziter Abbruch. Führende Null in beiden Dateien, Wertabweichungen und fehlender Parser sind getestet. |

## Neue Befunde nach Schwere

### Kleinigkeit G1 — Tests setzen den Interpreter-Namen und mindestens vier CPUs voraus

**Stellen:** `scripts\tests\Test-UltraScripts.ps1:1062,1075,1119,1148,1194,1208`;
`scripts\ultra-test-lib.ps1:174`.

Die F6-Tests verlangen als Rückgabe exakt `"python"`, obwohl der neue
Parser ausdrücklich auch `py -3` unterstützt. Auf einem gültig eingerichteten
Windows mit Python 3.11+ nur über den Launcher läuft die Prüfung korrekt,
die Suite meldet aber Fehler. Ebenso erwarten die neuen Thread-Fixtures
unbedingt 4, während der Helfer auf `Environment.ProcessorCount` begrenzt:
auf einer 1-/2-CPU-VM scheitern sie trotz korrektem Ergebnis.

**Vorschlag:** Einen erfolgreichen unterstützten Parser prüfen, nicht
zwingend dessen Namen; Thread-Erwartungen an die dokumentierte Begrenzung
anpassen. Statischer Nachweis, keine alternative Rechnerumgebung ausgeführt.
Auf diesem Rechner trat keiner dieser Fehler auf.

## Selbst ausgeführte Gates

Beide Befehle mit
`TEMP=TMP=D:\DEV\diktier\.herd\review-wp2-sol-3-temp`:

```text
pwsh -File scripts\tests\Test-UltraScripts.ps1
Test-UltraScripts: 48 Tests, 48 ok, 0 fehlgeschlagen, 1 übersprungen
Test-UltraScripts exit=0

python scripts\tests\test_quantize_ultra.py
Ran 11 tests in 0.171s

OK
test_quantize_ultra exit=0
```

Die sechs Browser-Tests liefen mit. Übersprungen wurde nur der direkte
Datei-Symlink-Test mangels Adminrechten/Entwicklermodus; das Anlegen meldete
„Administrator privilege required for this operation.“ Das ist kein
bestandener Symlink-Test. Junction/Reparse-Pfadprüfung und Hardlink-Zieldatei
wurden tatsächlich ausgeführt.

## Abnahmegrenzen

**WP5-Code-Abnahme: ja.** Die sechs ursprünglichen Restszenarien sind im
aktuellen Code geschlossen und durch passende Regressionstests gestützt.
G1 blockiert weder den jetzigen grünen Lauf noch die produktive Auswertung.

Das ist keine Modellentscheidung und kein Ersatz für reale Leistungs-,
Betriebs- und Rückweg-Gates oder Ralfs Urteile. Die strengere Verlängerung
muss praktisch eingeplant werden: Siebentage-Grundlage vor Ende von Tag 11
anlegen und aufbewahren. Der dokumentierte TOCTOU-Rest des Pfadschutzes gegen
gleichzeitiges Austauschen von Links bleibt; vorab vorhandene Links werden
gesperrt.

Nur diesen Bericht erstellt, keine Implementierung geändert, keine
Weiterdelegation und kein Commit. Keine Modelle geladen, kein Netz,
kein Schreiben in die echten APPDATA-/LOCALAPPDATA-Verzeichnisse. Echte
Release-/Modellläufe aus dem WP2f-Bericht wurden nicht erneut ausgeführt.
