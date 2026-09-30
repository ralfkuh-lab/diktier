Review-Auftrag: Plan „Alltagstest Parakeet Ultra mit Modell-Release“ für Diktier (Rust, Windows-only). Du bist ein delegierter Sub-Agent: nicht orchestrieren, nicht weiterdelegieren, nur dieses Review erledigen. Nur lesen und Befehle ausführen, keine Dateien außer deinem Bericht ändern.

Lies:
1. docs/ultra-alltagstest-plan.md (Gegenstand).
2. docs/reviews/spike-parakeet-ultra-notes.md (Grundlage, vor allem „Einschätzung“ und „Was ein Wechsel im Produkt bedeuten würde“).
3. docs/SPEC.md §6.2, §6.3, §8 (`[engine]`), §9, §10 (Log-Vertrag, `DIKTIER_DEBUG_WAV`).
4. Code, soweit der Plan darauf baut: src/models.toml, src/download.rs (Manifest, Download, Lock, `check_artifacts`, `COMPLETE`), src/config.rs (Validierung `engine.model`, Vorlage), src/engine.rs (`ParakeetTranscriber::load`), src/main.rs (`transcribe_wav`, Argumentparser), src/daemon/debug_wav.rs (`KEEP`, Ring), scripts/release.ps1 (`versions.toml` aus models.toml).

Prüffragen:
- Trägt der Plan die Frage, ob Ultra im Alltag besser ist? Sind die Abnahmekriterien messbar, vorab festgelegt und nicht manipulierbar? Fehlt ein Kriterium, ist eins unfair gegenüber v3 (Ralf diktiert live mit Ultra, v3 wird nur offline gerechnet)?
- Bleiben Log-Vertrag (§10: keine Transkripte) und Datenschutz gewahrt?
- Stimmen die Aussagen zum Code (Dateien, Zeilen, heutiges Verhalten)? Was übersieht der Plan beim Umbau des Manifests auf mehrere Modelle (release.ps1/versions.toml, stt-smoke, Tests, Download-Lock, Verzeichnisnamen, Tray/Fehlerzustände)?
- Modell-Release: Ist die Kette Reproduktion → Draft → Hash-Gegenprüfung → Veröffentlichung dicht? Was passiert, wenn GitHub die URL-Form oder die Weiterleitung ändert? Ist die Lizenzseite (CC-BY-4.0, Hinweis auf Änderungen) ausreichend? Welche Folgen hat es, immutable Releases für das ganze Repo einzuschalten?
- Rückweg: Kommt Ralf jederzeit sauber auf v3 zurück, auch wenn der Ultra-Download abbricht oder die Artefakte beschädigt sind?
- Reihenfolge und Zuschnitt der WPs: Ist etwas zu groß, fehlt ein Gate, ist etwas überflüssig?
- Die offenen Entscheidungen F1–F5: Gib zu jeder eine Empfehlung mit Begründung.

Keine Tests und keinen Build ausführen, nichts herunterladen, nichts im Repo oder in `%LOCALAPPDATA%`/`%TEMP%` ändern.

Bericht nach docs/reviews/plan-ultra-alltagstest-sol.md: Befunde nach Schwere (Blocker / wichtig / Hinweis), je mit Fundstelle (Plan-Abschnitt bzw. Datei:Zeile), Problem und konkretem Vorschlag, danach die Empfehlungen zu F1–F5. Ist ein Punkt in Ordnung, nicht auflisten. TUI-Ausgabe knapp.
