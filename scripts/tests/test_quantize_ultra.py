#!/usr/bin/env python
"""Negativtests fuer scripts/quantize-ultra.py (Code-Review WP2, K1).

Ohne echte Quantisierung und ohne Netz: kleine Dummy-Dateien in einem eigenen
Temp-Verzeichnis. Die Sollwerte SOURCE/OUTPUT, die Umgebungspruefung und die
Quantisierung werden nur hier im Test ersetzt; das Skript selbst bleibt
unveraendert.

Aufruf (Python >= 3.10, nur Standardbibliothek):

    python scripts/tests/test_quantize_ultra.py -v
"""

import contextlib
import hashlib
import importlib.util
import io
import os
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "quantize-ultra.py"
# Kein __pycache__ neben dem Skript anlegen.
sys.dont_write_bytecode = True


def load_module():
    spec = importlib.util.spec_from_file_location("quantize_ultra", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


# Kleine Stellvertreter der vier Quelldateien.
DUMMY = {
    "encoder-model.onnx": b"encoder",
    "encoder-model.onnx.data": b"gewichte" * 16,
    "decoder_joint-model.onnx": b"decoder",
    "vocab.txt": b"a\nb\n",
}


def snapshot(directory: Path) -> dict:
    """Alle Dateien unter directory mit Inhalt, relativ benannt."""
    return {
        str(p.relative_to(directory)): p.read_bytes()
        for p in sorted(directory.rglob("*"))
        if p.is_file()
    }


class QuantizeTests(unittest.TestCase):
    def setUp(self):
        self.q = load_module()
        self.tmp = Path(tempfile.mkdtemp(prefix="diktier-quantize-test-"))
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        # Nur im Test: Sollwerte der Dummy-Dateien, keine Umgebungspruefung,
        # keine echte Quantisierung.
        self.q.SOURCE = {n: (len(d), sha(d)) for n, d in DUMMY.items()}
        out = {
            "encoder-model.int8.onnx": b"q:" + DUMMY["encoder-model.onnx"],
            "decoder_joint-model.int8.onnx": b"q:" + DUMMY["decoder_joint-model.onnx"],
            "vocab.txt": DUMMY["vocab.txt"],
        }
        self.q.OUTPUT = {n: (len(d), sha(d)) for n, d in out.items()}
        self.q.log_environment = lambda: None
        self.q.check_external_data = lambda encoder: None

        def fake_quantize(source, work):
            for src_name, dst_name in self.q.QUANTIZE:
                (work / dst_name).write_bytes(b"q:" + (source / src_name).read_bytes())
            shutil.copyfile(source / "vocab.txt", work / "vocab.txt")

        self.q.quantize = fake_quantize

    def make_source(self, directory: Path) -> dict:
        directory.mkdir(parents=True, exist_ok=True)
        for name, data in DUMMY.items():
            (directory / name).write_bytes(data)
        return snapshot(directory)

    def run_main(self, *args) -> tuple[int, str]:
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            code = self.q.main([str(a) for a in args])
        return code, buf.getvalue()

    # ---------------------------------------------------------- Pruefung

    def test_overlap_in_both_directions_is_rejected(self):
        out = self.tmp / "export"
        cases = {
            "gleich --out": out,
            "Quelle unter --out": out / "quelle",
            "Quelle gleich .quantize-tmp": out / ".quantize-tmp",
            "Quelle gleich Staging": out / ".quantize-tmp" / "src",
            "Quelle unter Staging": out / ".quantize-tmp" / "src" / "tief",
            "--out unter Quelle": self.tmp,
        }
        for label, source in cases.items():
            with self.subTest(label):
                with self.assertRaises(self.q.UsageError):
                    self.q.check_paths(source, out)

    def test_case_variants_count_as_the_same_path_on_windows(self):
        if os.name != "nt":
            self.skipTest("nur Windows: Pfade ohne Gross-/Kleinschreibung")
        out = self.tmp / "Export"
        source = self.tmp / "EXPORT" / ".QUANTIZE-TMP" / "SRC"
        with self.assertRaises(self.q.UsageError):
            self.q.check_paths(source, out)

    def test_separate_directories_pass(self):
        work, staged = self.q.check_paths(self.tmp / "quelle", self.tmp / "export")
        self.assertEqual(work, (self.tmp / "export" / ".quantize-tmp").resolve())
        self.assertEqual(staged, work / "src")
        # Geschwister mit gemeinsamem Namensanfang sind keine Ueberlappung.
        self.q.check_paths(self.tmp / "export-quelle", self.tmp / "export")

    # ------------------------------------------------------ Ende zu Ende

    def test_sol_example_source_under_quantize_tmp_stays_intact(self):
        """K1-Beispiel: --source <out>\\.quantize-tmp\\src --out <out>."""
        out = self.tmp / "export"
        source = out / ".quantize-tmp" / "src"
        before = self.make_source(source)
        code, text = self.run_main("--source", source, "--out", out)
        self.assertEqual(code, 2, text)
        self.assertIn("ueberlappen", text)
        self.assertEqual(snapshot(source), before)

    def test_source_equal_to_out_stays_intact(self):
        source = self.tmp / "export"
        before = self.make_source(source)
        code, text = self.run_main("--source", source, "--out", source)
        self.assertEqual(code, 2, text)
        self.assertEqual(snapshot(source), before)

    def test_out_inside_source_is_rejected_before_anything(self):
        source = self.tmp / "quelle"
        before = self.make_source(source)
        code, text = self.run_main("--source", source, "--out", source / "export")
        self.assertEqual(code, 2, text)
        self.assertFalse((source / "export").exists())
        self.assertEqual(snapshot(source), before)

    def test_existing_quantize_tmp_is_not_deleted(self):
        source = self.tmp / "quelle"
        out = self.tmp / "export"
        self.make_source(source)
        leftover = out / ".quantize-tmp" / "encoder-model.int8.onnx"
        leftover.parent.mkdir(parents=True)
        leftover.write_bytes(b"Analyse-Rest")
        code, text = self.run_main("--source", source, "--out", out)
        self.assertEqual(code, 2, text)
        self.assertIn("existiert schon", text)
        self.assertEqual(leftover.read_bytes(), b"Analyse-Rest")

    def test_failure_before_the_work_dir_exists_deletes_nothing(self):
        """Scheitert log_environment (etwa fehlendes onnx), gibt es noch kein
        Arbeitsverzeichnis: nichts wird angelegt, nichts geloescht."""
        source = self.tmp / "quelle"
        out = self.tmp / "export"
        before = self.make_source(source)

        def broken():
            raise ImportError("onnx fehlt")

        self.q.log_environment = broken
        with self.assertRaises(ImportError):
            self.run_main("--source", source, "--out", out)
        self.assertFalse((out / ".quantize-tmp").exists())
        self.assertEqual(snapshot(source), before)

    def test_bad_source_file_cleans_only_own_staging(self):
        source = self.tmp / "quelle"
        out = self.tmp / "export"
        self.make_source(source)
        (source / "vocab.txt").write_bytes(b"anders\n")
        before = snapshot(source)
        code, text = self.run_main("--source", source, "--out", out)
        self.assertEqual(code, 1, text)
        self.assertIn("FEHL", text)
        self.assertFalse((out / ".quantize-tmp").exists(), "eigenes Staging bleibt liegen")
        self.assertEqual(snapshot(source), before)

    def test_success_moves_output_and_leaves_source(self):
        source = self.tmp / "quelle"
        out = self.tmp / "export"
        before = self.make_source(source)
        code, text = self.run_main("--source", source, "--out", out)
        self.assertEqual(code, 0, text)
        self.assertEqual(sorted(p.name for p in out.iterdir()), sorted(self.q.OUTPUT))
        self.assertEqual(snapshot(source), before)

    def test_wrong_output_stays_for_analysis(self):
        source = self.tmp / "quelle"
        out = self.tmp / "export"
        before = self.make_source(source)
        name = "vocab.txt"
        size, _ = self.q.OUTPUT[name]
        self.q.OUTPUT[name] = (size, "0" * 64)
        code, text = self.run_main("--source", source, "--out", out)
        self.assertEqual(code, 1, text)
        work = out / ".quantize-tmp"
        self.assertTrue((work / name).exists())
        self.assertFalse((work / "src").exists(), "Quellkopien bleiben liegen")
        self.assertEqual(snapshot(source), before)


if __name__ == "__main__":
    sys.exit(0 if unittest.main(exit=False).result.wasSuccessful() else 1)
