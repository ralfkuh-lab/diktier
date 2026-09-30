Auftrag: WP2b aus docs/ultra-alltagstest-plan.md (v2) umsetzen: die Belegsammlung für den Alltagstest. Diktier, Rust, Windows-only. Basis ist der aktuelle Working Tree (`2d10ee0` plus uncommittete Pakete WP0, WP1 und WP2a, Version 0.5.0). Diese sind Vorgabe, nicht Arbeitsgegenstand.

Lies zuerst:
1. docs/SPEC.md (v1.10) §10, Absatz `DIKTIER_DEBUG_WAV` inklusive „Seit v1.10 einstellbar“. Verbindlich.
2. docs/ultra-alltagstest-plan.md: Leitentscheidung 5, WP2b, WP3b Schritte 4–5, „Umgang mit dem Review“ W1.
3. docs/reviews/plan-ultra-alltagstest-sol.md, W1.
4. Code: src/daemon/debug_wav.rs (`enabled`, `debug_dir`, `KEEP`, `write_recording`, `prune`, Tests), src/daemon/workers.rs (`dump_debug_wav`), src/daemon/mod.rs (Start, Logzeilen beim Start).

Umfang:
- `DIKTIER_DEBUG_WAV_KEEP` (ganze Zahl 1–5000, Default 10) und `DIKTIER_DEBUG_WAV_DIR` (absoluter Pfad, Default `%TEMP%\diktier`) werden **einmal** beim Daemon-Start gelesen und als Konfiguration durchgereicht, nicht bei jedem Dump neu aus der Umgebung. Ungültige Werte führen zu genau einer Warnzeile im Log und zum jeweiligen Default. Ungültig heißt hier: keine Zahl, außerhalb des Bereichs, leerer oder relativer Pfad.
- Ist der Dump an, nennt beim Start genau eine Logzeile den effektiven Zustand, z. B. `Debug-WAV an: <verzeichnis>, behalte <n>`, ohne Inhalte. Ist er aus, keine Zeile (oder eine knappe „aus“-Zeile, wenn das zum bestehenden Stil passt; im Bericht begründen).
- Der Ring (`prune`) gilt mit der konfigurierten Kapazität im konfigurierten Verzeichnis. Fremde Dateien bleiben unberührt, die Regeln für `.part` und das Muster bleiben unverändert. Das Verzeichnis wird bei Bedarf angelegt, wie heute.
- Tests ohne echte Umgebung: Parser der Werte als reine Funktion (Grenzen 0/1/5000/5001, Text, Leerraum, relativ, absolut), Ring mit kleiner Kapazität im eigenen Temp-Verzeichnis, fremde Dateien bleiben, Default-Pfad unverändert. Tests verändern keine Prozess-Umgebung. Wo das nicht zu vermeiden ist, serialisiert und wiederhergestellt; im Bericht begründen.
- README: Im Abschnitt Debug-WAV die beiden Variablen kurz nennen. Mehr nicht: den Abschnitt „Alltagstest“ schreibt ein anderes Paket.

Gates (Befehl und tatsächliche Summary-Zeile zitieren):
1. `cargo fmt --check`
2. `cargo clippy --all-targets -- -D warnings` ohne Meldung
3. `cargo test` grün

Regeln:
- Nicht committen.
- Parallel arbeitet ein anderer Agent (WP2c) an src/main.rs, scripts/compare-models.ps1, scripts/bench-models.ps1, LICENSES/, dem README-Abschnitt „Alltagstest“ und dem Bundle-Teil von scripts/release.ps1. Diese Dateien bzw. Abschnitte nicht anfassen. Im README nur den Debug-WAV-Abschnitt.
- Keine Änderungen an docs/ (außer deinem Bericht), testdata/, src/models.toml, src/download.rs, src/config.rs. Nichts in `%LOCALAPPDATA%\diktier` oder `%TEMP%\diktier` schreiben; installierte Version und laufenden Daemon nicht anfassen; keine Benutzervariablen setzen.
- Kein herdr, keine weiteren Panes.
- Rückfragen: Frage nach `D:\DEV\diktier\.herd\fragen\impl-ultra-wp2b.md`, Turn mit `RUECKFRAGE: D:\DEV\diktier\.herd\fragen\impl-ultra-wp2b.md` beenden.

Bericht nach docs/reviews/impl-ultra-wp2b-notes.md: Umgesetzt, Abweichungen und warum, Gate-Ausgaben wörtlich, Offen. TUI-Ausgabe knapp.
