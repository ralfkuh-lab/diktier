#!/usr/bin/env python
"""Erzeugt das Modellartefakt parakeet-ultra-0.6b-int8-pc reproduzierbar (WP0).

Rezept laut docs/ultra-alltagstest-plan.md, Leitentscheidung 4:

1. Quelle: vier Dateien aus Hugging Face `altunenes/parakeet-rs`, Revision
   4d2a8bc71f5c896ec40faa59732e6716295edaf2, Ordner `parakeet-ultra/`
   (ONNX-Export von Moondream Parakeet Ultra). Groesse und SHA-256 stehen
   unten fest (SOURCE).
2. onnxruntime.quantization.quantize_dynamic(weight_type=QInt8,
   per_channel=True), Default-Operatortypen, fuer Encoder und Decoder.
3. vocab.txt wird unveraendert kopiert.
4. Die Ausgabe wird gegen feste Groessen und Hashes geprueft (OUTPUT). Bei
   Abweichung endet das Skript mit Exit 1. Neue Sollwerte werden nie
   nachgezogen: andere Bytes heissen neuer Modellschluessel.

Aufruf, isoliert und mit gepinnter Umgebung (scripts/quantize-ultra.requirements.txt),
<venv> ist ein beliebiges neues Verzeichnis:

    uv venv --no-config --python 3.12.12 <venv>
    uv pip sync --no-config --require-hashes --python <venv> scripts/quantize-ultra.requirements.txt
    <venv>\\Scripts\\python scripts/quantize-ultra.py --source models\\spike\\ultra-fp32 --out <dir>

Bewusst nicht `uv run --with-requirements`: uv 0.10.3 installiert dort auch
bei falschen Hashes ohne Fehler; `uv pip sync --require-hashes` bricht ab.

Ohne --source laedt das Skript die Quelldateien von Hugging Face nach
--download-dir (Default: <out>.source) und prueft sie ebenfalls; vorhandene,
gueltige Dateien dort werden wiederverwendet.

Das Quellverzeichnis wird nur gelesen. quantize_dynamic schreibt bei
Pfad-Eingabe ein `<modell>-inferred.onnx` neben das Modell und loescht es nur
bei Erfolg. Deshalb kopiert das Skript die Quelldateien zuerst nach
<out>/.quantize-tmp/src (~2,6 GB), prueft dabei Groesse und SHA-256 der
Kopie und quantisiert aus ihr. Die Kopien werden danach immer entfernt.

Schutz des Quellverzeichnisses (Code-Review WP2, K1), vor jedem Seiteneffekt:

- Quelle, --out, <out>/.quantize-tmp und <out>/.quantize-tmp/src duerfen sich
  in keiner Richtung ueberlappen (gleich, darunter oder darueber; aufgeloeste
  Pfade, unter Windows ohne Gross-/Kleinschreibung). Sonst Exit 2.
- Ein vorgefundenes <out>/.quantize-tmp wird nicht geloescht: Exit 2 mit
  Hinweis (es kann Analyse-Reste eines frueheren Laufs enthalten).
- Aufgeraeumt werden nur Verzeichnisse, die dieser Lauf selbst angelegt hat.

Exitcodes: 0 = Ausgabe stimmt, 1 = Pruef- oder Laufzeitfehler, 2 = Aufruf.
"""

import argparse
import hashlib
import os
import platform
import shutil
import sys
import time
import urllib.request
from pathlib import Path

REVISION = "4d2a8bc71f5c896ec40faa59732e6716295edaf2"
BASE_URL = f"https://huggingface.co/altunenes/parakeet-rs/resolve/{REVISION}/parakeet-ultra"

# Quelldateien: Name -> (Bytes, SHA-256). LFS-Dateien laut LFS-oid, vocab.txt
# gleich dem Produktions-vocab.txt von v3 (Spike 2026-09-30).
SOURCE = {
    "encoder-model.onnx": (
        87857063,
        "76f835e57d62d82f1485c7a84706782e44a123a69f4efa86ed3b4ad56e236051",
    ),
    "encoder-model.onnx.data": (
        2435420160,
        "6aeb9438f1f45dafc17d27c61a12bc406c0c2ccb8c17219aeeb3f898c283a8e6",
    ),
    "decoder_joint-model.onnx": (
        72520894,
        "a5911fe202e8fba44251fce252a6c9c7c0a7c724c882a13f81d96611fa2d7ccb",
    ),
    "vocab.txt": (
        93939,
        "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
    ),
}

# Ausgabe: Name -> (Bytes, SHA-256), bitgleich zu models\spike\ultra-int8-pc\.
OUTPUT = {
    "encoder-model.int8.onnx": (
        700507227,
        "2cc01c15a08d6976ca9ebe97739d15890f3088cfedd3a4aa4d969ba7a1702038",
    ),
    "decoder_joint-model.int8.onnx": (
        18300628,
        "afcb9459250ab5c2e48e657d852501c233e8b7f5daed1894a2a7101d21165a5e",
    ),
    "vocab.txt": (
        93939,
        "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
    ),
}

# Quelle -> Ziel der Quantisierung
QUANTIZE = (
    ("decoder_joint-model.onnx", "decoder_joint-model.int8.onnx"),
    ("encoder-model.onnx", "encoder-model.int8.onnx"),
)

# Stand, unter dem die Sollwerte entstanden sind (Lockfile)
EXPECTED_ENV = {"python": "3.12.12", "onnxruntime": "1.30.0", "onnx": "1.23.1"}

CHUNK = 8 * 1024 * 1024


class CheckError(Exception):
    pass


class UsageError(Exception):
    """Aufruf- oder Pfadfehler vor jedem Seiteneffekt (Exit 2)."""


def log(msg: str) -> None:
    print(msg, flush=True)


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while True:
            block = f.read(CHUNK)
            if not block:
                break
            h.update(block)
    return h.hexdigest()


def check_file(path: Path, size: int, sha: str) -> None:
    """Existenz, Groesse, SHA-256. Wirft CheckError mit klarer Meldung."""
    if not path.is_file():
        raise CheckError(f"{path}: fehlt")
    actual_size = path.stat().st_size
    if actual_size != size:
        raise CheckError(f"{path}: Groesse {actual_size} B, erwartet {size} B")
    actual_sha = sha256_file(path)
    if actual_sha != sha:
        raise CheckError(f"{path}: SHA-256 {actual_sha}, erwartet {sha}")


def check_set(directory: Path, expected: dict, label: str) -> None:
    errors = []
    for name, (size, sha) in expected.items():
        try:
            check_file(directory / name, size, sha)
            log(f"  ok   {name}  {size} B  {sha}")
        except CheckError as e:
            log(f"  FEHL {e}")
            errors.append(str(e))
    if errors:
        raise CheckError(f"{label}: {len(errors)} Datei(en) weichen ab")


def copy_checked(src: Path, dst: Path, size: int, sha: str) -> None:
    """Kopiert src nach dst und prueft dabei Groesse und SHA-256 der Kopie."""
    if not src.is_file():
        raise CheckError(f"{src}: fehlt")
    actual_size = src.stat().st_size
    if actual_size != size:
        raise CheckError(f"{src}: Groesse {actual_size} B, erwartet {size} B")
    h = hashlib.sha256()
    n = 0
    with open(src, "rb") as fin, open(dst, "wb") as fout:
        while True:
            block = fin.read(CHUNK)
            if not block:
                break
            fout.write(block)
            h.update(block)
            n += len(block)
    if n != size or h.hexdigest() != sha:
        dst.unlink()
        raise CheckError(f"{src}: SHA-256 {h.hexdigest()} ({n} B), erwartet {sha} ({size} B)")


def stage_sources(source: Path, staged: Path) -> None:
    """Kopiert alle Quelldateien geprueft in das (leere) staged; meldet alle Abweichungen."""
    errors = []
    for name, (size, sha) in SOURCE.items():
        try:
            copy_checked(source / name, staged / name, size, sha)
            log(f"  ok   {name}  {size} B  {sha}")
        except CheckError as e:
            log(f"  FEHL {e}")
            errors.append(str(e))
    if errors:
        raise CheckError(f"Quelle: {len(errors)} Datei(en) weichen ab")


def download(dest_dir: Path) -> None:
    """Laedt fehlende oder ungueltige Quelldateien, prueft beim Schreiben."""
    dest_dir.mkdir(parents=True, exist_ok=True)
    for name, (size, sha) in SOURCE.items():
        target = dest_dir / name
        if target.is_file():
            try:
                check_file(target, size, sha)
                log(f"  vorhanden und gueltig: {name}")
                continue
            except CheckError as e:
                log(f"  ungueltig, lade neu: {e}")
        url = f"{BASE_URL}/{name}"
        part = dest_dir / (name + ".part")
        log(f"  lade {url}")
        h = hashlib.sha256()
        n = 0
        t = time.time()
        with urllib.request.urlopen(url) as resp, open(part, "wb") as out:
            while True:
                block = resp.read(CHUNK)
                if not block:
                    break
                out.write(block)
                h.update(block)
                n += len(block)
        if n != size or h.hexdigest() != sha:
            part.unlink(missing_ok=True)
            raise CheckError(
                f"Download {name}: {n} B / {h.hexdigest()}, erwartet {size} B / {sha}"
            )
        os.replace(part, target)
        log(f"  geladen {name} {n} B in {time.time() - t:.0f} s")


def check_external_data(encoder: Path) -> None:
    """Der Encoder muss seine Gewichte aus encoder-model.onnx.data daneben ziehen."""
    import onnx
    from onnx.external_data_helper import uses_external_data

    model = onnx.load(str(encoder), load_external_data=False)
    locations = set()
    for tensor in model.graph.initializer:
        if uses_external_data(tensor):
            for entry in tensor.external_data:
                if entry.key == "location":
                    locations.add(entry.value)
    if locations != {"encoder-model.onnx.data"}:
        raise CheckError(
            f"{encoder}: External-Data-Verweise {sorted(locations)}, "
            "erwartet ['encoder-model.onnx.data']"
        )


def log_environment() -> None:
    import numpy
    import onnx
    import onnxruntime

    actual = {
        "python": platform.python_version(),
        "onnxruntime": onnxruntime.__version__,
        "onnx": onnx.__version__,
    }
    log(f"Python {actual['python']} ({platform.python_implementation()}, {sys.executable})")
    log(f"Plattform {platform.platform()} {platform.machine()}")
    log(f"onnxruntime {actual['onnxruntime']}, onnx {actual['onnx']}, numpy {numpy.__version__}")
    for key, want in EXPECTED_ENV.items():
        if actual[key] != want:
            log(f"WARNUNG: {key} {actual[key]} statt {want} (Lockfile); die Ausgabepruefung entscheidet")


def quantize(source: Path, work: Path) -> None:
    from onnxruntime.quantization import QuantType, quantize_dynamic

    for src_name, dst_name in QUANTIZE:
        t = time.time()
        quantize_dynamic(
            str(source / src_name),
            str(work / dst_name),
            weight_type=QuantType.QInt8,
            per_channel=True,
        )
        log(f"  {dst_name} {(work / dst_name).stat().st_size} B in {time.time() - t:.0f} s")
    shutil.copyfile(source / "vocab.txt", work / "vocab.txt")
    log("  vocab.txt kopiert")


def _key(path: Path) -> str:
    """Vergleichsform eines aufgeloesten Pfads (Windows: ohne Gross/klein)."""
    return os.path.normcase(os.path.normpath(str(path)))


def overlaps(a: Path, b: Path) -> bool:
    """a und b sind gleich oder eins liegt unter dem anderen."""
    ka, kb = _key(a), _key(b)
    if ka == kb:
        return True
    return any(_key(p) == kb for p in a.parents) or any(_key(p) == ka for p in b.parents)


def check_paths(source: Path, out: Path) -> tuple[Path, Path]:
    """Prueft die aufgeloesten Pfade vor jedem Seiteneffekt (K1).

    Rueckgabe: (work, staged). Wirft UsageError bei Ueberlappung der Quelle
    mit --out, .quantize-tmp oder Staging (beide Richtungen) und bei einem
    schon vorhandenen .quantize-tmp.
    """
    source = source.resolve()
    out = out.resolve()
    work = out / ".quantize-tmp"
    staged = work / "src"
    for label, path in (("--out", out), (".quantize-tmp", work), ("Staging", staged)):
        if overlaps(source, path):
            raise UsageError(
                f"Quelle {source} und {label} {path} ueberlappen sich; "
                "Quelle und Ausgabe muessen getrennte Verzeichnisse sein"
            )
    if os.path.lexists(work):
        raise UsageError(
            f"{work} existiert schon (Rest eines frueheren Laufs?). Es wird nicht "
            "geloescht: Inhalt pruefen, von Hand entfernen und neu starten"
        )
    return work, staged


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Erzeugt parakeet-ultra-0.6b-int8-pc aus altunenes/parakeet-rs@"
        + REVISION[:8]
    )
    parser.add_argument(
        "--source",
        type=Path,
        help="Verzeichnis mit den vier Quelldateien (sonst Download von Hugging Face)",
    )
    parser.add_argument("--out", type=Path, required=True, help="Zielverzeichnis")
    parser.add_argument(
        "--download-dir",
        type=Path,
        help="Ablage fuer den Download ohne --source (Default: <out>.source)",
    )
    args = parser.parse_args(argv)
    if args.source and args.download_dir:
        parser.error("--download-dir nur ohne --source")

    out = args.out.resolve()
    if args.source:
        source = args.source.resolve()
    else:
        source = (args.download_dir or Path(str(out) + ".source")).resolve()
    try:
        work, staged = check_paths(source, out)
    except UsageError as e:
        log(f"FEHLER (Aufruf): {e}")
        return 2
    # Nur was dieser Lauf selbst angelegt hat, wird aufgeraeumt.
    created_work = False
    created_staged = False
    try:
        log_environment()

        if args.source:
            log(f"Quelle: {source} (vorhanden, altunenes/parakeet-rs@{REVISION})")
        else:
            log(f"Quelle: {BASE_URL} -> {source}")
            download(source)

        work.mkdir(parents=True)
        created_work = True
        staged.mkdir()
        created_staged = True
        log(f"Kopiere und pruefe Quelldateien nach {staged}:")
        stage_sources(source, staged)
        check_external_data(staged / "encoder-model.onnx")

        log("Quantisiere (QInt8, per_channel=True):")
        quantize(staged, work)
        shutil.rmtree(staged)

        log("Pruefe Ausgabe:")
        try:
            check_set(work, OUTPUT, "Ausgabe")
        except CheckError:
            log(f"Abweichende Ausgabe bleibt zur Analyse in {work}")
            raise
        for name in OUTPUT:
            os.replace(work / name, out / name)
        work.rmdir()
        log(f"OK: {len(OUTPUT)} Dateien in {out}")
        return 0
    except CheckError as e:
        log(f"FEHLER: {e}")
        return 1
    finally:
        # Die Quellkopien (~2,6 GB) nie liegen lassen, auch nicht nach Fehlern;
        # aber nur die eigenen: ein vorgefundenes Verzeichnis bleibt (K1).
        if created_staged and staged.exists():
            shutil.rmtree(staged)
        if created_work and work.exists() and not any(work.iterdir()):
            work.rmdir()


if __name__ == "__main__":
    sys.exit(main())
