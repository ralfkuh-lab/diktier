Du bist ein delegierter Sub-Agent: nicht orchestrieren, nicht weiterdelegieren — nur diese Aufgabe erledigen.

Aufgabe: Review des Plans für einen relativen Silence-Gate in Diktier (Rust, lokales Push-to-Talk-Diktiertool auf Windows, STT mit parakeet-rs). Stand: Commit 5315181.

Lies in dieser Reihenfolge:
1. docs/silence-gate-plan.md (der zu prüfende Plan, v1)
2. src/engine.rs — heutiger Gate (`silence_gate`, `SilenceGate`, `rms_f32`, `max_window_rms`, `longest_loud_run_secs`, `transcribe_pcm`) und die Tests im Modul
3. src/daemon/workers.rs, Funktion `engine_loop` (Aufrufer im Daemon)
4. src/main.rs, Funktionen `transcribe_wav` und `record_test` (CLI-Aufrufer)
5. docs/SPEC.md §6.4, §10, §12 (Phase 1, Absatz zum RMS-Silence-Gate) und die Tabelle in §18
6. docs/SPIKES.md, Zeile „RMS-Silence-Gate“ in der Phase-2-Tabelle (Kalibrierungsherkunft)
7. testdata/stt/README.md

Kontext: Der heutige Gate arbeitet mit einer absoluten RMS-Schwelle (0,0075). Am 2026-09-21 lieferte das Headset über Stunden ~15 dB weniger Pegel; verständliche Diktate wurden als Stille verworfen. Messung: Parakeet erkennt alltag.wav auch um 22 dB abgesenkt wortidentisch. Der Plan ersetzt die absolute Stufe durch ein Verhältnis zum Grundrauschen (Regeln A–D). Die Zahlen im Plan stammen aus Python-Messungen über die Testdateien; du kannst sie mit einem eigenen Skript nachrechnen, musst aber nicht.

Prüfe kritisch — Fokus auf Signalverarbeitung, Testbarkeit und Spec-Treue, nicht auf Stil:
- Sind die Regeln A–D in sich schlüssig und vollständig? Gibt es Eingaben, bei denen keine Regel greift oder zwei widersprüchlich greifen?
- Floor-Schätzung als 10. Perzentil der 250-ms-Fenster-RMS: Robustheit bei kurzen Aufnahmen (0,25–2 s, also 1–8 Fenster), bei Aufnahmen ohne Pause, bei Aufnahmen, die mit Sprache beginnen. Bessere Schätzer (Minimum, Median der leisesten k Fenster, gleitendes Minimum)?
- Regel C (ABS_FLOOR 0,0003 ≈ −70 dBFS) gegen digitale Null, Quantisierungsrauschen von 16-bit-Quellen (die WAV-Fixtures sind 16 bit, der Live-Pfad ist f32 aus cpal) und Geräte mit hartem Noise-Gate im Treiber (liefern exakt 0 in Pausen, dann Sprache) — kippt Regel C oder D bei solchen Geräten?
- Regel B als Ja-Pfad: bringt sie gegenüber D nur Geschwindigkeit, oder auch Verhalten, das D nicht hätte? Regressionsrisiko gegenüber heute.
- Halluzinationsschutz: der Plan öffnet die Engine für leise Signale mit ≥ 1,5 s Lauf bei +12 dB. Welche Nicht-Sprache erfüllt das realistisch (Tippen, Lüfter-Anlauf, Stuhl, Atmen ins Headset)? Ist die Live-Gate-Liste im Plan dafür ausreichend, was fehlt?
- Testplan: reichen die synthetischen Fixtures (skalierte alltag.wav) als Beleg, oder braucht der Plan echte leise Aufnahmen vor der Umsetzung? Welche Unit-Tests fehlen?
- Log-Erweiterung (`DeadInput`, `NoRelativeRun`): §10-konform? Fehlt eine Angabe, die man zur Nachkalibrierung im Betrieb bräuchte?
- Spec-Nachtrag (WP4): reicht der Umfang, ist §6.4 der richtige Ort?
- Fehlende Arbeitspakete, falsche Reihenfolge, unrealistischer Umfang.

Schreibe das Ergebnis als Markdown nach docs/reviews/plan-silence-gate-astra.md mit dieser Struktur:
- Kurzurteil (3 Sätze)
- Blocker (müssen vor Implementierung in den Plan)
- Wichtige Hinweise (sollten rein)
- Kleinigkeiten
- Offene Fragen an den Autor
Jeder Punkt: Fundstelle (Datei/Abschnitt), Problem, konkreter Vorschlag. Keine Umformulierung des Plans, keine Code-Änderungen an src/. Wenn du zum Nachrechnen ein Skript brauchst, lege es nur unter .herd/ ab. Halte die TUI-Ausgabe knapp; das Ergebnis zählt nur aus der Datei.
