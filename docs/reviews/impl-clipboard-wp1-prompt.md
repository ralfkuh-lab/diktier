Auftrag: Mehrformat-Clipboard-Snapshot/-Restore (WP1) und die Diagnose-CLI `--clipboard-check` (WP2) umsetzen. Diktier, Rust, Windows-only. Basis: HEAD 56a6a4e auf main plus uncommittete Doku-Änderungen (docs/SPEC.md v1.8, docs/clipboard-restore-plan.md v2, docs/reviews/plan-clipboard-restore-*.md). Diese Dateien sind Vorgabe, nicht Arbeitsgegenstand.

Lies zuerst:
1. docs/clipboard-restore-plan.md (v2) komplett, insbesondere „Entscheidungen“, Leitentscheidungen 1–7 und 9, WP1, WP2, Gates G1/G3.
2. docs/SPEC.md §7.1 mit dem neuen §7.1.1, §7.3, §10, §18 #14. Das ist die verbindliche Quelle. Weicht der Plan davon ab, gilt die SPEC. Widersprüche meldest du im Bericht, statt sie still aufzulösen.
3. docs/reviews/plan-clipboard-restore-sol.md (Hintergrund der Entscheidungen, Microsoft-Doku-Verweise).
4. src/inject/protocol.rs, src/inject/windows.rs, src/inject/mod.rs, src/inject/fake.rs, src/daemon/workers.rs (Inject-Worker, Logzeile `Paste …`), src/main.rs (CLI-Modi, `--inject-test` als Vorbild), src/single_instance.rs.

Umfang:
- WP1 nach Plan: Formatmatrix (Leitentscheidung 1) als reine, unit-getestete Funktion. Nutz- und Begleitformate. Budgets als weiche Limits (2). Zusätzliche Fokusprüfung nach `become_owner`, unmittelbar vor dem ersten Key-Event (3). Snapshot-Ausgänge `Empty`/`Formats`/`Unrestorable` (4). `restore_snapshot` mit Vorab-Allokation vor `OpenClipboard`, RAII-Hüllen, erneuter Owner/Sequenz-Prüfung, Transkript-Fallback und `RestoreResult` (5). Eigene Payload als Snapshot (6). `ExcludeClipboardContentFromMonitorProcessing` auf allen Transkript-Pfaden und beim Restore (7). Logzeilen mit bereinigten Namen (9).
- `RestoreDecision` bekommt `Restored`, `RestoredPartial` (mit Unterscheidung Verlust beim Sichern / beim Zurückschreiben), `RestoreFailed`. `InjectOutcome::Pasted` transportiert, was WP3 später für den Hinweis braucht: Ausgang plus die Frage „auch beim Zurückschreiben verloren?“. `InjectReport` in src/state.rs und die Hinweislogik fasst du **nicht** an, das ist WP3. Der Inject-Worker loggt nur die neue Information.
- Alle Fake-Tests aus WP1. Die Windows-Integrationstests aus WP1 legst du an, mit `#[ignore]`, Namenspräfix `clipboard_live_`, und dem Kommentar, dass sie die echte Zwischenablage überschreiben. **Du führst sie nicht aus**, auch nicht einzeln: Sie überschreiben Ralfs Zwischenablage, während er am Rechner arbeitet. Sie müssen aber kompilieren (`cargo test --no-run`).
- WP2: `--clipboard-check` (nur lesend, Default) und `--clipboard-check --roundtrip` (verweigert bei laufendem Daemon; Single-Instance-Mechanik wiederverwenden, ohne die Daemon-Instanz zu stören). Exitcodes 0/3/1, `--help`-Text mit Warnhinweisen. README ist WP4, fass sie nicht an.
- Jede neue `unsafe`-Stelle mit `// SAFETY:` im Stil des Bestands. Kein neues Crate ohne Not. Braucht windows-sys ein zusätzliches Feature (z. B. für `GetEnhMetaFileBits`), ist das in Ordnung; nenne es im Bericht.
- Die Synthese- und Handle-Annahmen der Matrix gleichst du mit der Microsoft-Doku ab („Standard Clipboard Formats“, „Clipboard Formats“, `EnumClipboardFormats`, `SetClipboardData`, `GetClipboardData`). Abweichungen und offene Punkte mit Quelle in den Bericht.

Gates:
1. `cargo fmt --check`
2. `cargo clippy --all-targets -- -D warnings` ohne Meldung
3. `cargo test` grün; `cargo test --no-run` baut auch die ignorierten `clipboard_live_*`-Tests
4. `cargo test -- --ignored stt_smoke_fixtures` grün (Regressionsschutz)
5. `cargo build --release`, dann `target/release/diktier.exe --clipboard-check` **nur lesend** einmal gegen den aktuellen Inhalt ausführen und die Ausgabe (ohne Inhalte, sie enthält ohnehin keine) in den Bericht übernehmen. `--roundtrip` nicht ausführen.

Zitiere für jedes Gate den Befehl und die tatsächliche Summary-Zeile.

Regeln:
- Nicht committen.
- Keine Änderungen an docs/SPEC.md, docs/clipboard-restore-plan.md, docs/reviews/* (außer deinem Bericht), README.md, Cargo-Version, src/state.rs, src/overlay.rs, src/config.rs, src/daemon/debug_wav.rs.
- Keine Fenster öffnen, die den Fokus nehmen. Kein Aufruf, der Ralfs Zwischenablage verändert (keine ignorierten Clipboard-Tests, kein `--roundtrip`, kein manuelles Setzen).
- Kein herdr, keine weiteren Panes. Interne Sub-Agenten für Recherche sind erlaubt.
- Scheiterst du zweimal an derselben Stelle, beschreib sie im Bericht und mach mit dem Rest weiter.

Bericht nach docs/reviews/impl-clipboard-wp1-notes.md:
- Umgesetzt (je Leitentscheidung kurz, mit Datei/Funktion)
- Abweichungen vom Plan oder der SPEC und warum
- Doku-Abgleich der Formatmatrix (Quelle je Punkt)
- Gate-Ausgaben wörtlich
- Offen / Risiken / Hinweise für WP3 (welche Felder `InjectOutcome` jetzt trägt)

Halte die TUI-Ausgabe knapp. Das Ergebnis zählt nur aus Bericht und `git diff`.
