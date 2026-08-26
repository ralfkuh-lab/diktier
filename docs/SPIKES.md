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

## Phase 2b — Tray

Spec §12 Phase 2b.

## Phase 3 / 4

Spec §12.
