#!/usr/bin/env python3
"""Normalisierung und WER nach Spec §12 Phase 1."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

# Bindestriche zuerst durch Leerzeichen, dann restliche Interpunktion weg.
_DASHES = str.maketrans({
    "-": " ",
    "–": " ",
    "—": " ",
})
_PUNCT = str.maketrans("", "", ".,!?;:\"'„“”‚‘’»«")
_WS = re.compile(r"\s+")


def normalize(text: str) -> str:
    text = text.lower()
    text = text.translate(_DASHES)
    text = text.translate(_PUNCT)
    return _WS.sub(" ", text).strip()


def wer(reference: str, hypothesis: str) -> float:
    """Wort-Levenshtein / |Referenz|. Beide Seiten werden normalisiert."""
    ref = normalize(reference).split()
    hyp = normalize(hypothesis).split()
    if not ref:
        return 0.0 if not hyp else 1.0
    prev = list(range(len(hyp) + 1))
    for i, rtok in enumerate(ref, start=1):
        cur = [i]
        for j, htok in enumerate(hyp, start=1):
            ins = cur[j - 1] + 1
            delete = prev[j] + 1
            sub = prev[j - 1] + (rtok != htok)
            cur.append(min(ins, delete, sub))
        prev = cur
    return prev[-1] / len(ref)


def selftest() -> None:
    assert normalize("Hallo, Welt!") == "hallo welt"
    assert normalize("rust-daemon") == "rust daemon"
    assert normalize("Grüße, Öl, Spaß — Zeile") == "grüße öl spaß zeile"
    assert normalize("a–b") == "a b"
    assert normalize("„Zitat“ »x« ”y” ‚z‘ ‘w’") == "zitat x y z w"
    assert normalize("a.,!?;:\"'b") == "ab"
    assert wer("eins zwei drei", "eins zwei drei") == 0.0
    assert abs(wer("a b c", "a x c") - 1.0 / 3.0) < 1e-9
    assert abs(wer("a b", "a x b") - 0.5) < 1e-9  # Insertion
    assert abs(wer("a b c", "a c") - 1.0 / 3.0) < 1e-9  # Deletion
    assert wer("", "") == 0.0
    assert wer("", "x") == 1.0
    assert wer("x", "") == 1.0
    print("selftest ok", file=sys.stderr)


def _read_text(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except FileNotFoundError:
        print(f"Datei nicht gefunden: {path}", file=sys.stderr)
        raise SystemExit(1) from None
    except OSError as err:
        print(f"Datei unlesbar: {path}: {err}", file=sys.stderr)
        raise SystemExit(1) from None


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="normalize.py",
        description="Normalisierung und WER nach Spec §12 Phase 1.",
    )
    parser.add_argument(
        "--selftest",
        action="store_true",
        help="eingebaute Selbsttests ausführen",
    )
    parser.add_argument(
        "args",
        nargs="*",
        help="DATEI  oder  WER <datei-a> <datei-b>",
    )
    ns = parser.parse_args(argv)

    if ns.selftest:
        if ns.args:
            parser.error("--selftest verträgt keine weiteren Argumente")
        selftest()
        return 0

    if ns.args and ns.args[0] == "WER":
        if len(ns.args) != 3:
            parser.error("WER erwartet zwei Dateien: WER <datei-a> <datei-b>")
        a = _read_text(Path(ns.args[1]))
        b = _read_text(Path(ns.args[2]))
        print(f"{wer(a, b):.6f}")
        return 0

    if len(ns.args) != 1:
        parser.error("erwartet genau eine Datei (oder WER / --selftest)")
    text = _read_text(Path(ns.args[0]))
    sys.stdout.write(normalize(text) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
