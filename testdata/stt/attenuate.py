#!/usr/bin/env python3
"""Digitale Absenkung einer 16-bit-WAV (silence-gate-plan WP1).

Multipliziert jedes Sample mit ``10^(-dB/20)``, rundet kaufmännisch
(halbe Werte vom Nullpunkt weg) auf die nächste Ganzzahl und sättigt auf
den i16-Bereich. Kein Dithering, keine Filter, keine Normalisierung —
dieselbe Eingabe ergibt byteweise dieselbe Ausgabe. Zum Beleg druckt das
Skript den SHA-256 der geschriebenen Datei.

Eingabe muss 16-bit-Integer-PCM sein (Rate und Kanalzahl werden
unverändert übernommen; die STT-Fixtures sind 16 kHz mono).

    python3 testdata/stt/attenuate.py testdata/stt/alltag.wav 16
    python3 testdata/stt/attenuate.py --selftest
"""

from __future__ import annotations

import argparse
import array
import hashlib
import math
import sys
import wave
from pathlib import Path

I16_MIN = -32768
I16_MAX = 32767


def factor(db: float) -> float:
    """Linearer Faktor zu einer Absenkung um ``db`` Dezibel."""
    return 10.0 ** (-db / 20.0)


def round_half_away_from_zero(x: float) -> int:
    """Kaufmännisch runden: 0,5 → 1, −0,5 → −1 (Pythons round() rundet zur geraden Zahl)."""
    if x >= 0.0:
        return math.floor(x + 0.5)
    return math.ceil(x - 0.5)


def scale_sample(value: int, f: float) -> int:
    return max(I16_MIN, min(I16_MAX, round_half_away_from_zero(value * f)))


def attenuate_samples(samples: array.array, db: float) -> array.array:
    f = factor(db)
    return array.array("h", (scale_sample(v, f) for v in samples))


def read_i16_wav(path: Path) -> tuple[array.array, wave._wave_params]:
    with wave.open(str(path), "rb") as src:
        params = src.getparams()
        if params.sampwidth != 2:
            raise SystemExit(
                f"{path}: {params.sampwidth * 8}-bit PCM, erwartet 16-bit Integer"
            )
        if params.comptype != "NONE":
            raise SystemExit(f"{path}: komprimiert ({params.comptype}), erwartet PCM")
        raw = src.readframes(params.nframes)
    samples = array.array("h")
    samples.frombytes(raw)
    if sys.byteorder == "big":  # WAV ist little-endian
        samples.byteswap()
    return samples, params


def write_i16_wav(path: Path, samples: array.array, params: "wave._wave_params") -> None:
    payload = array.array("h", samples)
    if sys.byteorder == "big":
        payload.byteswap()
    with wave.open(str(path), "wb") as dst:
        dst.setnchannels(params.nchannels)
        dst.setsampwidth(2)
        dst.setframerate(params.framerate)
        dst.writeframes(payload.tobytes())


def sha256_of(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 16), b""):
            digest.update(block)
    return digest.hexdigest()


def default_output(src: Path, db: float) -> Path:
    db_text = f"{db:g}".replace(".", "_")
    return src.with_name(f"{src.stem}_-{db_text}db{src.suffix}")


def attenuate_file(src: Path, db: float, dst: Path) -> str:
    samples, params = read_i16_wav(src)
    write_i16_wav(dst, attenuate_samples(samples, db), params)
    return sha256_of(dst)


def selftest() -> None:
    assert round_half_away_from_zero(0.5) == 1
    assert round_half_away_from_zero(1.5) == 2
    assert round_half_away_from_zero(2.5) == 3  # round() ergäbe hier 2
    assert round_half_away_from_zero(-0.5) == -1
    assert round_half_away_from_zero(-2.5) == -3
    assert round_half_away_from_zero(0.49) == 0
    assert abs(factor(0.0) - 1.0) < 1e-12
    assert abs(factor(6.0206) - 0.5) < 1e-6
    assert abs(factor(16.0) - 0.15848931924611134) < 1e-15
    assert abs(factor(22.0) - 0.07943282347242814) < 1e-15
    # Sättigung: die Untergrenze bleibt im i16-Bereich, hier ohne Absenkung.
    assert scale_sample(I16_MIN, 1.0) == I16_MIN
    assert scale_sample(I16_MIN, 2.0) == I16_MIN
    assert scale_sample(I16_MAX, 2.0) == I16_MAX
    # −16 dB auf bekannte Werte.
    f16 = factor(16.0)
    assert scale_sample(32767, f16) == round_half_away_from_zero(32767 * f16) == 5193
    assert scale_sample(-32768, f16) == -5193
    assert scale_sample(3, f16) == 0  # 0,475 → 0
    assert scale_sample(4, f16) == 1  # 0,634 → 1
    assert attenuate_samples(array.array("h", [100, -100, 0]), 0.0).tolist() == [
        100,
        -100,
        0,
    ]
    print("selftest ok", file=sys.stderr)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="attenuate.py",
        description="16-bit-WAV deterministisch um <dB> absenken (silence-gate-plan WP1).",
    )
    parser.add_argument("--selftest", action="store_true", help="eingebaute Selbsttests")
    parser.add_argument("wav", nargs="?", type=Path, help="Eingabe (16-bit PCM)")
    parser.add_argument("db", nargs="?", type=float, help="Absenkung in dB (> 0)")
    parser.add_argument(
        "out",
        nargs="?",
        type=Path,
        help="Ausgabe (Default: <name>_-<db>db.wav neben der Eingabe)",
    )
    ns = parser.parse_args(argv)

    if ns.selftest:
        if ns.wav is not None:
            parser.error("--selftest verträgt keine weiteren Argumente")
        selftest()
        return 0

    if ns.wav is None or ns.db is None:
        parser.error("erwartet <datei.wav> <db> [ausgabe.wav] (oder --selftest)")
    if ns.db <= 0:
        parser.error("db muss > 0 sein (das Skript senkt ab)")
    if not ns.wav.is_file():
        print(f"Datei nicht gefunden: {ns.wav}", file=sys.stderr)
        return 1

    dst = ns.out or default_output(ns.wav, ns.db)
    digest = attenuate_file(ns.wav, ns.db, dst)
    print(f"{dst}  -{ns.db:g} dB  Faktor {factor(ns.db):.17g}")
    print(f"sha256  {digest}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
