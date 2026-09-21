# Umsetzung Regel B3 und Version 0.3.0

Auftrag: [impl-silence-gate-b3-prompt.md](impl-silence-gate-b3-prompt.md),
Vertrag SPEC §6.4/§18 #13 (v1.7), Kalibrierung
[../SPIKES.md](../SPIKES.md) („Kalibrierung relativer Silence-Gate“).
Basis: Commit `a74519a`. Nicht committet; SPEC, Plan, SPIKES und die
übrigen Reviews unverändert.

## ✅ Regel B3 im Gate (`src/engine.rs`)

- `pub const QUIET_SPEECH_RMS: f32 = 0.004;` (≈ −48 dBFS) mit
  Kalibrierungsbeleg im Rustdoc; die Laufdauer ist dieselbe Konstante wie
  bei B2 (`MIN_SPEECH_RUN_ABS_SECS`, 2,0 s).
- `SpeechRule::B3` neu; `decide()` prüft in der Reihenfolge
  A → B1 → B2 → **B3** → C → D. Grund-Text: `Regel B3: Lauf ≥ 2.00 s über
  0.00400`.
- `GateMetrics::longest_quiet_run_secs` (Lauf über `QUIET_SPEECH_RMS`,
  `break_below = ABS_FLOOR` wie bei den anderen Läufen) wird immer
  berechnet — auch wenn eine frühere Regel entscheidet.
- `Display` ergänzt um `Lauf 0.004 X.XX s` zwischen `Lauf abs` und
  `Lauf rel`. Der Log-Vertrag aus §6.4 bleibt eine Zeile pro Aufnahme.
- Rustdoc an `silence_gate` gibt die neue Regelreihenfolge und die
  angepassten Grenzen wieder (gleichmäßiges Geräusch zwischen 0,004 und
  0,0075 über ≥ 2 s erreicht jetzt die Engine — Schutz ist dort die
  Engine selbst, §18 #13).

### Entscheidungen

- **B3 zählt Läufe mit `break_below = ABS_FLOOR`**, wie B2 und D. Da
  `QUIET_SPEECH_RMS > ABS_FLOOR` ist das wirkungslos, aber einheitlich —
  eine Sonderbehandlung hätte nur eine zweite Lesart geschaffen.
- **Kein eigener Ablehnungsgrund** für B3: greift die Regel nicht, fällt
  der Fall nach C/D durch und die Ablehnung nennt weiter `Regel C` bzw.
  `Regel D`. Das entspricht §6.4 („die erste zutreffende entscheidet“).
- **`longest_quiet_run_secs` auch bei früher Entscheidung** — dieselbe
  Begründung wie bei den bestehenden Metriken (Astra W4:
  Nachkalibrierbarkeit aus dem Betriebslog).

## ✅ Tests

Fünf neue Fälle (Abschnitt „Regel B3“), alle über `transcribe_pcm` mit
`CountingStub`:

- `rule_b3_admits_quiet_speech_without_a_pause` — Nachbau „Aufnahme 07“:
  1,5 s @ 0,003 + 4 s @ 0,0055 + 1,5 s @ 0,003. Belegt im selben Test,
  dass B2 nicht greift (Lauf abs 0,00 s) **und** D keinen Kontrast findet
  (Schwelle D 0,0119 > max. Fenster) → `SpeechRule::B3`.
- `rule_b3_needs_two_full_seconds` — 1,75 s @ 0,0055: in Stille
  entscheidet D (Lauf rel 1,75 s), ohne Kontrast (floor = 0,003) bleibt
  es leer.
- `rule_b3_run_is_exact_at_32000_samples` — 31.999 Samples leer,
  32.000 Samples B3.
- `rule_b3_does_not_shadow_b2` — Astras Gegenbeispiel 8 s @ 0,003 +
  2 s @ 0,010 erfüllt B2 und B3; die Reihenfolge entscheidet für B2.
- `rule_b3_ignores_a_one_second_disturbance` — 1,0 s @ 0,03 in digitaler
  Null (Stuhl/Kabel bzw. Klick, SPIKES ≤ 1,0 s) bleibt leer.

Bestehende Tests gegengeprüft, drei Anpassungen:

- `rule_b2_needs_two_full_seconds` (Astras B2-Gegenbeispiel mit 1,75 s
  @ 0,010): **verifiziert — B3 greift nicht**, 1,75 s < 2,0 s. Als
  Assertion auf `longest_quiet_run_secs` festgehalten, Erwartung
  unverändert `NoRelativeRun`.
- `rule_d_admits_quiet_speech_after_silence` und die 1-LSB-Variante in
  `rule_d_same_cases_with_one_lsb_residual` benutzten 0,004 — exakt die
  neue B3-Schwelle. Der D-Fall läuft jetzt mit 0,0035; der 0,004-Fall
  bleibt als expliziter B3-Nachweis im ersten Test stehen.
- `display_carries_every_measurement` und `rule_names_appear_in_the_report`
  um einen B3-Fall erweitert, D-Fall auf 0,0035 gezogen; das Display
  prüft zusätzlich `Lauf 0.004 X.XX s`.

`stt_smoke_fixtures` unverändert gültig: `rauschen.wav` hat 1,00 s über
0,004 und bleibt abgelehnt, `alltag_-22db.wav` 0,25 s (D deckt).

## ✅ `--gate-analyze`

Je Datei eine Zeile direkt unter dem Report — vor der Margen-Tabelle und
unabhängig davon, ob ein volles Fenster existiert:

```
  absolut 0.0040 / 0.0075: Lauf 4.50 s / 1.00 s (B3/B2 ab 2.0 s)
```

## ✅ Version 0.3.0

- `Cargo.toml` 0.2.2 → 0.3.0, `Cargo.lock` von `cargo build` nachgezogen.
- README-Versionszeile auf `v0.3.0` inklusive Release-Tag im Link.
- README-Troubleshooting, Gate-Eintrag: **Seit 0.3.0** misst der Gate
  relativ zum Grundrauschen, und `Regel B3` lässt leises Sprechen ohne
  Pause durch (2 s am Stück über 0,004). Hinweis: im Eintrag stand bisher
  **keine** Version — „0.2.2“ kam dort nicht vor —, die Angabe wurde also
  ergänzt, nicht ersetzt. Der `--gate-analyze`-Eintrag nennt die neue
  absolute Zeile.
- `docs/windows-plan.md`, WP6: Satz zu 0.3.0 (relativer Gate + B3, Verweis
  auf SPIKES) hinter der 0.2.2-Notiz.

## Gates (wörtlich)

```
> cargo fmt --check
(kein Output, Exit 0)

> cargo clippy --all-targets
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.93s
(keine Meldung)

> cargo test
test result: ok. 383 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.25s

> cargo test -- --ignored stt_smoke_fixtures
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 383 filtered out; finished in 17.17s

> cargo build --release
    Finished `release` profile [optimized] target(s) in 13.21s
```

stt-smoke (Debug, `--nocapture`) — die beiden abgelehnten und die sieben
Sprachdateien:

```
stille.wav: leer (Regel D: kein aktiver Lauf >= 1.50 s) — … Lauf 0.004 0.00 s, Lauf rel 0.00 s
rauschen.wav: leer (Regel D: kein aktiver Lauf >= 1.50 s) — … Lauf 0.004 1.00 s, Lauf rel 1.00 s
alltag.wav: WER 0.0476 — Engine (Regel B1: Gesamt-RMS ≥ 0.00750)
fachwoerter.wav: WER 0.1000 — Engine (Regel B1: Gesamt-RMS ≥ 0.00750)
zahlen_umlaute.wav: WER 0.1333 — Engine (Regel B1: Gesamt-RMS ≥ 0.00750)
alltag_-16db.wav: WER 0.0476 — Engine (Regel D: Lauf ≥ 1.50 s über Schwelle D)
alltag_-22db.wav: WER 0.0476 — Engine (Regel D: Lauf ≥ 1.50 s über Schwelle D)
fachwoerter_-16db.wav: WER 0.1000 — Engine (Regel D: Lauf ≥ 1.50 s über Schwelle D)
zahlen_umlaute_-16db.wav: WER 0.1333 — Engine (Regel D: Lauf ≥ 1.50 s über Schwelle D)
```

Die WER-Werte sind die heutigen Baselines, unverändert gegenüber dem
Vorgängerlauf; die abgesenkten Dateien bleiben innerhalb des
+0,05-Puffers.

## `--gate-analyze` auf allen Aufnahmen (Release)

`target/release/diktier.exe --gate-analyze testdata/stt/local/*.wav testdata/stt/*.wav`,
jeweils die Entscheidungszeile:

| Datei | Entscheidung |
|---|---|
| `local/00_normal_jabra.wav` | Engine (Regel B1: Gesamt-RMS ≥ 0.00750) |
| `local/01_tippen.wav` | leer (Regel D: kein aktiver Lauf ≥ 1.50 s) |
| `local/02_atmen.wav` | leer (Regel D: kein aktiver Lauf ≥ 1.50 s) |
| `local/03_stuhl_kabel.wav` | Engine (Regel B1: Gesamt-RMS ≥ 0.00750) |
| `local/04_stille.wav` | leer (Regel C: max. Fenster unter 0.00030) |
| `local/05_leise_20pct.wav` | Engine (**Regel B3**: Lauf ≥ 2.00 s über 0.00400) |
| `local/06_leise_vorlauf.wav` | Engine (**Regel B3**: Lauf ≥ 2.00 s über 0.00400) |
| `local/07_leise_sofort.wav` | Engine (**Regel B3**: Lauf ≥ 2.00 s über 0.00400) |
| `local/07b_leise_sofort_mit_nullvorlauf.wav` | Engine (**Regel B3**: Lauf ≥ 2.00 s über 0.00400) |
| `local/08_einwort.wav` | leer (Regel D: kein aktiver Lauf ≥ 1.50 s) |
| `local/08b_einwort_mit_nullvorlauf.wav` | leer (Regel D: kein aktiver Lauf ≥ 1.50 s) |
| `local/09_normal_referenz.wav` | Engine (Regel B1: Gesamt-RMS ≥ 0.00750) |
| `alltag.wav` | Engine (Regel B1: Gesamt-RMS ≥ 0.00750) |
| `alltag_-16db.wav` | Engine (Regel D: Lauf ≥ 1.50 s über Schwelle D) |
| `alltag_-22db.wav` | Engine (Regel D: Lauf ≥ 1.50 s über Schwelle D) |
| `fachwoerter.wav` | Engine (Regel B1: Gesamt-RMS ≥ 0.00750) |
| `fachwoerter_-16db.wav` | Engine (Regel D: Lauf ≥ 1.50 s über Schwelle D) |
| `rauschen.wav` | leer (Regel D: kein aktiver Lauf ≥ 1.50 s) |
| `stille.wav` | leer (Regel D: kein aktiver Lauf ≥ 1.50 s) |
| `zahlen_umlaute.wav` | Engine (Regel B1: Gesamt-RMS ≥ 0.00750) |
| `zahlen_umlaute_-16db.wav` | Engine (Regel D: Lauf ≥ 1.50 s über Schwelle D) |

Die Erwartung aus dem Auftrag ist erfüllt, mit **einer Abweichung in der
Benennung**: 05 und 06 werden jetzt von **B3** statt von D angenommen
(05: Lauf 4,25 s über 0,004; 06: 4,00 s). Beide waren vorher schon
Ja-Fälle — es ändert sich nur, welche Regel zuerst zutrifft, eine direkte
Folge der Reihenfolge B3 vor D aus §6.4. Die Engine läuft wie bisher, die
Läufe über 0,004 stehen jetzt in der `absolut`-Zeile.

Neu belegt: 07 (Lauf 4,50 s über 0,004, Schwelle D 0,01173 > max. Fenster
0,00967) kommt ausschließlich über B3 durch — genau der Fall, für den die
Regel eingeführt wurde. 07b mit Null-Vorlauf ebenso (dort hätte auch D
gereicht: Lauf rel 6,75 s).

## 🔍 Offen

- **Nicht committet** (Auftrag). Geänderte Dateien: `src/engine.rs`,
  `src/main.rs`, `Cargo.toml`, `Cargo.lock`, `README.md`,
  `docs/windows-plan.md`, dazu dieser Bericht und der Auftrag.
- **Live-Abnahme im Daemon** (Hotkey, leises Diktat ohne Pause) steht
  aus; geprüft ist bisher nur der CLI-Pfad mit den WAVs.
- **Release 0.3.0** (Tag, `scripts/release.ps1`, Setup) ist nicht
  angefasst — die Versionszeile im README verweist auf einen Tag, den es
  erst nach der Veröffentlichung gibt (wie zuvor bei 0.2.2).
- **Restrisiko aus §18 #13** bleibt wie beschrieben: ein gleichmäßiges
  Geräusch zwischen 0,004 und 0,0075 über ≥ 2 s erreicht die Engine.
  `03_stuhl_kabel.wav` läuft schon heute über B1 in die Engine und liefert
  leer; ein B3-Gegenbeispiel aus echten Aufnahmen gibt es nicht.
