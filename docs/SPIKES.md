# Diktier — Spike-Protokoll

Vorlage. Nichts abhaken, das nicht auf der genannten Maschine gelaufen ist.

## Maschinen

| Rolle | OS | CPU | RAM | Ort |
|---|---|---|---|---|
| Langsames Zeit-Gate | Omarchy / Arch | i7-4500U (Haswell, AVX2) | | Rechner „omarchy“ |
| Schnelles Zeit-Gate + Mint-22-Zielplattform | Mint 22.3 Cinnamon X11 x86_64 | Ryzen 9 5900HX (8C/16T) | 30 GiB | ralf-Legion-S7-15ACH6 |
| Windows 10 | 22H2 x64 | | | |
| Windows 11 | x64 | | | |

Peak-RSS-Ziel: ≤ 2 GiB mit geladenem Default-Modell; zusätzlich mit
einer 60-s-Datei messen (Spec §12 Phase 1).

## Artefakte

SHA-256 und Bytes siehe `docs/SPEC.md` §6.3. Gemessene Werte hierher kopieren.

## Phase 1 — STT

Crate-/ORT-Version: `parakeet-rs =0.3.7`, `ort =2.0.0-rc.13` (load-dynamic,
api-28), ONNX Runtime CPU **1.28.0** linux-x64 (scripts/fetch-ort.sh,
SHA-256 im Skript gepinnt). Threads: Runtime-Default (Config `threads = 0`).
Artefakte: Golden Set §6.3, per SHA-256 verifiziert von
`istupakov/parakeet-tdt-0.6b-v3-onnx` Revision
`8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce`.

Diktier-Seite gemessen 2026-08-26 auf ralf-Legion-S7-15ACH6 (Release-Build,
DIKTIER_ORT_LIB auf Repo-`lib/`). Methode Zeit: 5 CLI-Läufe je Datei, die
CLI weist Modellladen (~1,9 s) und Inferenz getrennt aus; Median Inferenz.

Voxtype-Referenz: 2026-08-26 auf „omarchy“ (i7-4500U), `voxtype 0.7.5`,
`voxtype transcribe --engine parakeet`, Artefakt-Hashes vor dem Lauf per
sha256sum verifiziert (= Golden Set §6.3). Handgezählte WER (wortgetreue
Referenz, Bindestrich = Wortgrenze); maschinelle Bestätigung nach dem
normalize.py-Fix (Kreuz-Review B3) nachtragen.

| Datei | WER Voxtype | WER Diktier | Δ | Fachwörter | Halluzination | Zeit warm (Median, 5) |
|---|---|---|---|---|---|---|
| Alltag (12 s, leise Aufnahme) | 0/21 = 0 % | 1/21 ≈ 4,8 % („Werstadt“ statt „Werkstatt“) | +1 Wort | | keine | 0,563 s |
| Fachwörter (12 s) | 1/20 = 5 % („Demon“) | 1/20 = 5 % — Text **identisch** zu Voxtype | 0 | ONNX/Runtime/Worker-Thread/Transkript korrekt | keine | 0,57 s |
| Zahlen/Umlaute (12 s) | 2/15 ≈ 13,3 % | 2/15 ≈ 13,3 % — Text **identisch** zu Voxtype | 0 | Jörg/Björn/Umlaute korrekt | keine | 0,56 s |
| Stille → leer (6 s) | leer ✓ | **leer** ✓ | 0 | | keine | 0,30 s |
| Rauschen → leer (9 s) | leer ✓ | **leer** ✓ | 0 | | keine | 0,43 s |

Voxtype-Zeiten auf Haswell (aus den Logs): Modellladen 2,6–2,8 s, Inferenz
1,85–1,97 s für 12 s Audio (RTF ≈ 0,16). **Diktier auf Haswell**
(Mint-Release-Bundle nach /tmp/diktier-bundle, Modell-Symlink,
`--runs 5`): Laden 2,47 s, Inferenz 2,23–2,26 s für 12 s Audio
(RTF ≈ 0,19, Median 2,241 s) — Haswell-Gate (10 s ≤ 20 s) klar erfüllt;
Transkript identisch zum Mint-Lauf. Haswell-RSS nicht gemessen (kein
GNU time auf Omarchy); RSS-Werte siehe oben (Mint).

**Befund „Werstadt“ (einziger Text-Unterschied, 4/5 Dateien wortidentisch):**
Beide Tools nutzen byte-identische Artefakte, aber verschiedene
Mel-Frontends: Voxtype `src/transcribe/fbank.rs` rechnet Kaldi-Style
(Samples × 32768 auf int16-Range, eigene Pre-emphasis), `parakeet-rs 0.3.7`
`src/audio.rs` rechnet NeMo-Style ([-1,1], Pre-emphasis auf normalisiertem
Signal, log-zero-guard 2^-24 wie im NeMo-Training). Numerisch verschiedene
Features → bei der bewusst leisen Aufnahme kippt genau ein Token.
Threads 1/2/4/8 ändern nichts (getestet). Kein Diktier-Defekt; die
Spec-Annahme aus §17 #19 („dieselben Artefakte ⇒ identische Pipeline“)
war zu stark. Entscheidung zum Gate: siehe Spec §18 (Nachtrag).

Peak-RSS (Release, /usr/bin/time -v): 12-s-Datei **1,08 GiB**; 60-s-Datei
(Konkatenation der Sprach-WAVs, nur für RSS) **1,39 GiB** — Ziel ≤ 2 GiB ✓.

Notizen:
- `rauschen.wav` um die erste Sekunde gekürzt (Enter-Klick der Aufnahme).
- `alltag.wav` ist leise (Peak 13 %) — bewusst als harter Fall belassen;
  das WER-Gate ist relativ zu Voxtype auf derselben Datei.
- Digitale Null-Samples (0,5 s) ergaben in einem Vorabtest „Yeah.“ —
  echte Raumstille/-rauschen sind leer. RMS-Gate (Spec §12) bisher NICHT
  nötig.
- WER-Spalten offen bis zum Voxtype-Referenzlauf auf „omarchy“
  (`voxtype transcribe <wav>`, CLI vorhanden).

Gate: Spec §12 Phase 1 — **BESTANDEN** mit dem wiederhergestellten
+0,05-Puffer (Spec §18 #11, Owner-Entscheidung 2026-08-26): 4/5 Dateien
text-identisch zu Voxtype, „Alltag“ 4,8 % vs. 0 % (innerhalb +5 %).
Maschinelle WER-Bestätigung (normalize.py nach B3-Fix): alltag
Diktier 0,0476 / Voxtype 0,0000 (Δ +4,76 % ≤ +5 %), fachwoerter beide
0,0500, zahlen_umlaute beide 0,1333. Haswell-Zeiten: siehe oben.
**Phase 1 vollständig.**

## Phase 2 — Inject / Capture

Linux-Zeile der Pflichtmatrix: live abgenommen 2026-08-26 auf
ralf-Legion-S7-15ACH6 (Orchestrator, xdotool/wmctrl-gesteuert,
Spike-CLI `--inject-test` / `--hotkey-test` / `--record-test`).
Gate-Text «Grüße, Öl, Spaß — Zeile 1\nZeile 2», Byte-Vergleich per diff.

| Fall | Ergebnis |
|---|---|
| xed | pass — byte-exakt, `ctrl_v`, Fenster-ID vor/nach identisch (0x6600325) |
| gnome-terminal | pass — `ctrl_shift_v` via WM_CLASS, byte-exakt inkl. Zeilen, kein `^V` |
| VSCodium (statt VS Code, Owner-Entscheidung) | pass — `ctrl_v`, byte-exakt (Home-Datei; Flatpak sieht /tmp nicht — Testmethodik, kein Produktproblem) |
| Fokuswechsel während Transkription | pass — copy_only, Transkript bleibt im Clipboard |
| Kein Read → kein Restore (§7.1 P7) | pass — Transkript bleibt |
| Fremder Owner während Wartezeit | pass — kein Restore („fremder Clipboard-Inhalt bleibt“) |
| Clipboard-Restore | pass in der Serve-Phase (restored_served=1, Inhalt „ALTER-INHALT-42“) |
| PTT F9 Press/Release | pass — `global-hotkey` 0.8.0; 2,5-s-Halten mit X-Autorepeat → genau 1 Press/Release |
| Registrierungskonflikt | pass — „HotKey already registered“ sauber gemeldet |
| Capture nativ | pass — 48 kHz / I32 / Stereo → Downmix → rubato → 16 k; Längenbilanz exakt (630784/3), overflow=0 |
| End-to-End Lautsprecher→Mikro | pass — alltag.wav über Raumakustik nahezu fehlerfrei transkribiert |
| RMS-Silence-Gate | Schwelle 0,0075 (≈ −42,5 dBFS); stille.wav rms=0,00119 → leer ohne Engine; alltag.wav rms=0,02145 → transkribiert; Anlass: Live-Halluzination „Ich bin jetzt wohl weiter zu machen.“ bei stillem Raum (vor dem Gate) |

Erkenntnisse:
- `csd-clipboard` (Cinnamon) stellt nach Owner-Exit seinen letzten Fetch
  wieder her; ein still restauriertes Clipboard geht damit beim
  Prozessende verloren. Daemon-Quit-Pfad (Phase 3) MUSS das
  `CLIPBOARD_MANAGER`/`SAVE_TARGETS`-Protokoll bedienen.
- Windows-Zeilen der Matrix: offen (eigene Etappe, Rechner/VMs nötig).
- Kreuz-Review 2a+2b: docs/reviews/impl-phase2-codex.md / -agy.md.
  Konsolidiertes 14-Punkte-Fixpaket umgesetzt (u. a. SelectionClear-Drain
  + Ownership-Timestamp, finale Fokusprüfung vor Key-Event, Cookie-Checks
  vor Read-Zählung, Consumer-only-SPSC, Hotkey-Handshake, leading_space,
  fensterbasiertes RMS-Gate, TIMESTAMP-Target/Latin-1-STRING, INCR-Grenzen).
  Vertagt mit Code-Verankerung: codex H4 (nichtblockierender Paste) =
  Phase-3-Pflicht; ICCCM MULTIPLE = dokumentierte v1-Lücke.
  Live-Regression nach Fixpaket: xed byte-exakt, Restore im Grace-Fenster
  („ALTER-INHALT-99“, restored_served=1), PTT 1/1 entprellt, Capture
  48 kHz/I32/Stereo overflow=0. 89 Unit-Tests + stt-smoke grün.
  **Phase 2 (Linux) vollständig.**

## Phase 2b — Tray-Smoke

2026-08-26, Cinnamon 22.3: `betrayer`-TrayBackend, SNI-Item im Panel
sichtbar (Sichtprüfung Screenshot), Zustandsrotation
starting→…→paused mit Tooltip „<zustand> — <modellschlüssel>“,
D-Bus-Eventpfad, sauberes Quit (Exit 0, kein Prozess-Leak). 99 Unit-Tests.
Offen: Panel-Neustart-Fall (invasiv, bei Gelegenheit im Alltag) und
Menü-Sichtprüfung durch den User. Kreuz-Review: zusammen mit Phase 3.

## Phase 3 — Daemon (Linux)

2026-08-27, ralf-Legion-S7-15ACH6. TDD-Etappe (3a Tests-zuerst mit
Orchestrator-Abnahme, 3b Implementierung, 3c Wiring, 3d Infrastruktur),
Owner-Entscheidungen per Remote Control. Details + Live-Belege:
`docs/reviews/impl-phase3-context.md`.

Gate §12 Phase 3 — alle Fälle live bestanden:
- Kalter Start MIT echtem Modell-Download (650 MB von HF, .part→SHA→
  atomar→COMPLETE), danach Diktat.
- F9-PTT-Diktat landet in xed (mehrfach, auch nach Pause/Resume und mit
  konfigurierter Ausweichtaste F8).
- Parallelstart Exit 0 (Doppel-Lock beide Orte, beide Richtungen),
  kill -9 → sofortiger Neustart.
- Beenden über Tray und SIGTERM ≤ 5 s, auch während Inferenz
  (Quit-Latch: kein Inject nach Quit), keine Zombies.
- Log-Rotation 2 MiB → .1 live, Ein-Writer belegt, Privacy-Grep sauber.
- Autostart idempotent, Leerzeichen-Pfad, Exec-Quoting, %-Escaping.

Kreuz-Review (impl-phase3-codex.md / -agy.md): 3×Hoch + 4×Mittel (codex),
2×Mittel + Rest niedrig (agy) — 11-Punkte-Fixpaket umgesetzt und live
regressiert (271 Tests). Wichtigste Fixes: SPSC-In-flight-Zähler,
Quit-Prioritäts-Latch, HotkeyConfig bis ins Backend + Grab-Freigabe bei
Pause (Manager-Drop).

Testumgebungs-Notiz: Ein nächtlicher Screensaver-Lock ließ XTEST-Events
ins Leere laufen (Fehlalarm, per Referenz-Build ausgeschlossen;
loginctl unlock-session).

## Phase 4 — Politur/Release (Linux)

2026-08-27. `scripts/release.sh` → `dist/diktier-0.1.0-linux-x64.tar.gz`
(13,9 MB): diktier + lib/libonnxruntime.so + LICENSES/ (MIT, CC-BY-4.0,
NOTICE-parakeet, ORT, THIRD-PARTY) + versions.toml + Bundle-README
(Owner-Entscheidung; §11-Layout ergänzt).

Release-Gate §11 in sauberem ubuntu:24.04-Container bestanden: Tarball in
leeres Verzeichnis, ORT-Env unset, Modell read-only gemountet →
--transcribe-wav liefert das identische Transkript wie der Host, Exit 0;
`ldd` ohne „not found“. Einzige Audio-Systemabhängigkeit: libasound.so.2
(cpal-pulseaudio spricht den Pulse-Socket über einen reinen
Rust-Protokoll-Stack, kein libpulse-Link — nachgemessen via readelf/nm).

Verschobene-Binary-Fall (§12): Bundle A → install → nach B verschieben →
erneutes install aktualisiert den Eintrag. Portabler Start aus fremdem
cwd ok. Idempotenz von release.sh belegt, shellcheck sauber.

**Linux-Stand komplett: Phasen 0–4 bestanden.** Offen: Windows-Etappe
(Pflichtmatrix Win10/11, fetch-ort.ps1-Verifikation, Named Mutex,
Windows-Autostart, release.ps1).

## Kalibrierung relativer Silence-Gate (Windows, 2026-09-21)

Anlass und Plan: `docs/silence-gate-plan.md` (v2), SPEC v1.6 §6.4.
Werkzeug: `diktier --gate-analyze` (Release-Build aus `f0563c1`), Aufnahmen
über den laufenden Daemon 0.2.1 mit `DIKTIER_DEBUG_WAV=1` (Jabra Evolve2 40,
48 kHz → 16 kHz, Büro-Laptop), Dateien lokal unter `testdata/stt/local/`
(gitignored, nicht im Repo). „Alt“ = Gate 0.2.1/0.2.2 (absolut), „Neu“ =
Regeln A/B1/B2/C/D. Laptop-Mikrofon nicht aufgenommen (Owner: „ohne 10“).

| # | Aufnahme | Pegel | Dauer | RMS | max. Fenster | floor | Alt | Neu | Engine |
|---|---|---|---|---|---|---|---|---|---|
| 00 | normales Diktat (18 s) | 44 % | 18,4 s | 0,01334 | 0,02799 | 0,00011 | Engine | B1 | Text ok |
| 01 | Tippen, kein Wort | 44 % | 8,7 s | 0,00039 | 0,00077 | 0,00017 | leer | D leer | — |
| 02 | Atmen ins Mikro | 44 % | 5,8 s | 0,00153 | 0,00314 | 0,00019 | leer | D leer (Lauf 0,75 s) | — |
| 03 | Stuhl rollen, Kabel reiben | 44 % | 11,5 s | 0,00792 | 0,04567 | 0,00007 | Engine | B1 | **leer** (0,70 s, keine Halluzination) |
| 04 | stiller Raum | 44 % | 14,4 s | 0,00005 | 0,00029 | 0,00002 | leer | C leer | — |
| 05 | Referenzsatz, normal gesprochen | **20 %** | 8,0 s | 0,00697 | 0,01120 | 0,00082 | **leer** | **D** (Lauf 4,25 s) | WER 0,0476 (= Original) |
| 06 | 3 s Stille, dann Referenzsatz | 20 % | 10,9 s | 0,00497 | 0,01244 | 0,00002 | **leer** | **D** (Lauf 4,00 s) | WER 0,0476 |
| 07 | Referenzsatz sofort, pausenlos | 20 % | 7,1 s | 0,00675 | 0,00967 | 0,00295 | leer | **D leer** (Schwelle 0,0117 > max) | mit 3 s Null-Vorlauf: WER 0,0476 |
| 08 | ein Wort („Schreibtisch“) | 20 % | 1,9 s | 0,00616 | 0,01099 | 0,00002 | leer | D leer (Lauf 1,00 s) | — |
| 09 | Referenzsatz, Gegenprobe | 44 % | 7,9 s | 0,01050 | 0,01838 | 0,00072 | Engine | B1 | WER 0,0476 |

Befunde:

- **Der Bug ist reproduziert und behoben:** 05 und 06 sind der Fall vom
  Morgen — alter Gate leer, neuer Gate Regel D, Engine wortgleich zur
  Referenz in voller Lautstärke. Reserve: Lauf 4,0–4,25 s gegen 1,5 s
  gefordert; bei allen Margen +10/+12/+15 dB identisch.
- **Negativfälle sicher:** Tippen und Stille kommen nicht über die
  Aktivitätsgrenze (Jabra-DSP liefert in Pausen nahezu Null → Regel C
  greift knapp, max. Fenster 0,00029 < 0,0003), Atmen erreicht 0,75 s,
  die Hälfte der geforderten Laufdauer. Stuhl/Kabel ist lauter als die
  alte Schwelle (B1, wie heute) — die Engine liefert darauf leer.
- **Bekannte Grenze bestätigt (Plan F1):** 07 (pausenlos, sofort) und 08
  (Einzelwort) bleiben leer. Bei 07 ist das Grundrauschen der Aufnahme die
  leise Sprache selbst; bei 08 ist der Lauf mit 1,0 s gleich lang wie der
  Klick in `rauschen.wav` und die Stuhlgeräusche.
- **Kandidat für 07 — zusätzlicher absoluter Pfad „B3: Lauf ≥ 2,0 s über
  0,004“** (−48 dBFS), längste Läufe über 0,004 je Datei: 07 4,50 s, 05
  4,25 s, 06 4,00 s, 00 2,50 s · Stuhl/Kabel 1,00 s, `rauschen.wav` 1,00 s,
  Atmen 0,00 s, Tippen/Stille 0,00 s, `alltag_-16db` 1,00 s (D deckt),
  `alltag_-22db` 0,25 s (D deckt). Trennung 1,0 s gegen 4,0 s. Nicht
  umgesetzt — Owner-Entscheidung offen (Spec-Änderung §6.4 → v1.7).
- **Hinweis zur Phase-1-Zeile oben:** für `fachwoerter.wav` liefern
  `normalize.py` und die Rust-Portierung auf dem heutigen Transkript WER
  0,1000 („Rust Demon“, „ONX Modell“ = 2/20), nicht 0,0500. Der historische
  Eintrag bleibt stehen; der Smoke-Test vergleicht relativ zur unabgesenkten
  Datei und ist davon unberührt.

## Vorlauf-Stille gegen „Herr Präsident“ (Windows, 2026-09-30)

Anlass: Diktate beginnen gelegentlich mit einem erfundenen „Herr
Präsident.“. Belegfälle aus dem Debug-WAV-Ring, gesichert unter
`testdata/stt/local/herr_praesident/` (lokal, nie committen, Übersicht in
`FAELLE.md`): Lauf 523 (reproduziert direkt), Lauf 545 (live 180 Zeichen,
nachgestellt 164; Differenz = „Herr Präsident. “), Lauf 547 (zusätzlich
„Ich schaue“ statt „Schau“).

- **Debug-WAV nicht bitgenau:** Der Ring speichert 16-bit, die Engine
  bekommt f32. Lauf 545 zeigt die Phrase nur mit ±1 LSB Zufallsrauschen
  (8 von 8 Seeds). Das Modell kippt also schon bei Rauschen um −90 dBFS im
  Vorlauf. Deshalb v1.9: Debug-WAV als 32-bit-Float.
- **Kein Zustand zwischen Aufrufen:** `parakeet-rs` 0.3.7 setzt den
  Decoder-Zustand pro Aufruf auf null, der Feature-Cache hält nur
  Filterbank und FFT-Plan.
- **Schnitt- und Stille-Varianten (Lauf 523):** 0,10/0,15 s abschneiden →
  Phrase bleibt; ab 0,20 s abschneiden → weg; 0,1–1,0 s Nullen voranstellen
  → weg, obwohl das Geräusch erhalten bleibt.
- **Matrix** (`--transcribe-wav` der installierten 0.4.0, 0/200/300/500 ms
  Nullen, jede Datei roh und mit ±1-LSB-Dither Seed 0): 7 Fixtures, 5
  Kalibrierungsaufnahmen, 12 Ring-Aufnahmen.

  | Stille | WER-Summe roh | WER-Summe Dither | „Herr Präsident“ (ohne 527) |
  |---|---|---|---|
  | 0 ms | 75,2 | 53,8 | roh 2, Dither 3 |
  | 200 ms | 48,8 | 65,5 | 0 |
  | 300 ms | 51,2 | 51,2 | 0 |
  | 500 ms | 75,5 | 60,7 | 0 |

  WER-Summe = Summe der Einzel-WER in Prozent über die 12 Dateien mit
  Referenz (je Wortfehler 4,8 bzw. 5,0/6,7). Lauf 527 enthält „Herr
  Präsident“ wirklich (gesprochen). Die übrigen Unterschiede sind einzelne
  Wortkipper in beide Richtungen („Werkstatt“/„Werstadt“, „lassen“/
  „schlossen“), wie sie auch der Dither allein erzeugt. Entscheidung:
  300 ms (SPEC §6.4, §18 #15).
- **Nachprüfung mit dem 0.4.1-Build** (`target-dev`, 300 ms eingebaut):
  Lauf 545 mit 24 Dither-Seeds → 23 sauber, Seed 4 zeigt weiterhin „Herr
  Präsident.“ (ohne Stille waren es 8 von 8). Seed 4 mit zusätzlich 200,
  500 oder 1000 ms Stille oder mit genulltem Vorlauf bis 0,85 s → sauber.
  Die Stille senkt die Rate also stark, beseitigt die Phrase aber nicht
  garantiert. 300 ms bleibt (Matrix oben).
- **Grenze:** Das vorangestellte „Ich“ in Lauf 547 bleibt bei jeder
  Vorlauf-Variante (abgeschnitten, genullt, 0,3–1,0 s Stille).
