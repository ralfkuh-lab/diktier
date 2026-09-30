"""Echter TOML-Parser für die PowerShell-Skripte (WP2f, Nachreview F1/F6).

Liest eine TOML-Datei mit `tomllib` (Standardbibliothek ab Python 3.11) und
gibt sie als JSON mit Typangabe auf stdout aus, damit PowerShell nichts
umdeuten kann (ConvertFrom-Json macht aus ISO-Texten Datumswerte und
vergleicht Schlüssel ohne Groß-/Kleinschreibung):

    Tabelle   {"table": [["schlüssel", <wert>], ...]}   (Reihenfolge der Datei)
    Liste     {"array": [<wert>, ...]}
    Text      {"string": "..."}
    Ganzzahl  {"int": "123"}                            (als Text, ohne Rundung)
    Wahrheit  {"bool": true}
    sonst     {"other": "<typname>", "value": "<str()>"}

Exitcodes: 0 gültig, 1 ungültiges TOML (Meldung auf stderr), 2 Aufruf oder
Datei nicht lesbar, 3 Python älter als 3.11 (kein tomllib).
"""

import json
import sys

try:
    import tomllib
except ImportError:  # Python < 3.11
    print("toml-json.py: tomllib fehlt (Python >= 3.11 nötig)", file=sys.stderr)
    sys.exit(3)


def tag(value):
    # bool vor int prüfen: bool ist in Python eine Unterklasse von int.
    if isinstance(value, bool):
        return {"bool": value}
    if isinstance(value, int):
        return {"int": str(value)}
    if isinstance(value, str):
        return {"string": value}
    if isinstance(value, dict):
        return {"table": [[k, tag(v)] for k, v in value.items()]}
    if isinstance(value, list):
        return {"array": [tag(v) for v in value]}
    return {"other": type(value).__name__, "value": str(value)}


def main(argv):
    if len(argv) != 2:
        print("Aufruf: toml-json.py <datei.toml>", file=sys.stderr)
        return 2
    try:
        with open(argv[1], "rb") as f:
            data = tomllib.load(f)
    except (tomllib.TOMLDecodeError, UnicodeDecodeError) as e:
        print(f"{type(e).__name__}: {e}", file=sys.stderr)
        return 1
    except OSError as e:
        print(f"nicht lesbar: {e}", file=sys.stderr)
        return 2
    sys.stdout.write(json.dumps(tag(data), ensure_ascii=True))
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
