# Diktier — Spike-Protokoll

Vorlage. Nichts abhaken, das nicht auf der genannten Maschine gelaufen ist.

## Maschinen

| Rolle | OS | CPU | RAM | Ort |
|---|---|---|---|---|
| Langsames Zeit-Gate | Omarchy / Arch, dieses Gerät | i7-4500U (Haswell, AVX2) | | dieses System |
| Büro-Laptop | | | | *namentlich eintragen* |
| Windows 10 | 22H2 x64 | | | |
| Windows 11 | x64 | | | |
| Mint 22 | Cinnamon **X11** x86_64 | | | |

Peak-RSS-Ziel: ≤ 2 GiB mit geladenem Default-Modell.

## Artefakte

SHA-256 und Bytes siehe `docs/SPEC.md` §6.3. Gemessene Werte hierher kopieren.

## Phase 1 — STT

Crate-/ORT-Version:

| Datei | WER Voxtype | WER Diktier | Δ | Fachwörter | Halluzination | Zeit warm (Median, 5) |
|---|---|---|---|---|---|---|
| Alltag | | | | | | |
| Fachwörter | | | | | | |
| Zahlen/Umlaute | | | | | | |
| Stille → leer | | | | | | |
| Rauschen → leer | | | | | | |

Gate: Spec §12 Phase 1.

## Phase 2 — Inject / Capture

Pflichtmatrix und Einzelfälle: Spec §12 Phase 2. Pro Zelle: pass/fail + Fenster-ID vor/nach.

## Phase 2b — Tray

Spec §12 Phase 2b.

## Phase 3 / 4

Spec §12.
