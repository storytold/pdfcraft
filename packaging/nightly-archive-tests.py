#!/usr/bin/env python3
"""Synthetic app-owned archive/output tests; no native build or executable launch."""
import gzip
import importlib.util
import os
import runpy
import sys
from unittest import mock
from pathlib import Path
import tarfile
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("pdfcraft_nightly_archive", HERE / "nightly-archive.py")
ARCHIVE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ARCHIVE)


class ArchivePolicy(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name).resolve()
        previous = Path.cwd()
        os.chdir(self.root)
        self.addCleanup(os.chdir, previous)
        self.stage = self.root / "stage"
        self.stage.mkdir()
        self.output = self.root / "nightly.tar.gz"
        self.required = {"pdfcraft", "pdfcraft-nightly", "pdfcraft-nightly.png", "README.md", "LICENSE-MIT", "LICENSE-APACHE", "NOTICE", "ATTRIBUTION.md"}
        for name in self.required:
            (self.stage / name).write_bytes(b"generated fixture: " + name.encode())
        (self.stage / "pdfcraft").chmod(0o755)
        (self.stage / "pdfcraft-nightly").chmod(0o755)

    def test_flat_regular_ustar_with_all_notices_and_the_launcher(self):
        (self.stage / "OFL-Font.txt").write_text("generated fixture license", encoding="utf-8")
        ARCHIVE.linux_archive(self.stage, self.output)
        with tarfile.open(self.output) as archive:
            self.assertEqual({m.name for m in archive}, self.required | {"OFL-Font.txt"})
            self.assertTrue(all(m.isfile() and not m.pax_headers for m in archive))
            self.assertEqual(archive.extractfile("pdfcraft").read(), (self.stage / "pdfcraft").read_bytes())
            self.assertEqual(archive.extractfile("pdfcraft-nightly").read(), (self.stage / "pdfcraft-nightly").read_bytes())
        with gzip.open(self.output, "rb") as stream:
            while (header := stream.read(512)) != bytes(512):
                self.assertEqual(len(header), 512)
                self.assertIn(header[156:157], {b"0", b"\0"})
                self.assertEqual(header[257:263], b"ustar\0")
                size = int(header[124:136].rstrip(b"\0 ").lstrip(b" ") or b"0", 8)
                stream.read(((size + 511) // 512) * 512)

    def test_each_required_member_is_required_before_output(self):
        for name in self.required:
            with self.subTest(name=name):
                content = (self.stage / name).read_bytes()
                (self.stage / name).unlink()
                with self.assertRaisesRegex(ValueError, "required"):
                    ARCHIVE.linux_archive(self.stage, self.output)
                self.assertFalse(self.output.exists())
                (self.stage / name).write_bytes(content)

    def test_unexpected_cli_portable_or_installer_member_refuses(self):
        for name in ["pdfcraft-cli", "portable.txt", "pdfcraft.desktop", "models"]:
            with self.subTest(name=name):
                bad = self.stage / name
                bad.write_bytes(b"generated fixture")
                with self.assertRaisesRegex(ValueError, "unexpected"):
                    ARCHIVE.linux_archive(self.stage, self.output)
                self.assertFalse(self.output.exists())
                bad.unlink()

    def test_symlink_refuses_before_output(self):
        icon = self.stage / "pdfcraft-nightly.png"
        icon.unlink()
        icon.symlink_to(self.stage / "README.md")
        with self.assertRaisesRegex(ValueError, "regular file|symlinks or aliases"):
            ARCHIVE.linux_archive(self.stage, self.output)
        self.assertFalse(self.output.exists())

    def test_directory_refuses_before_output(self):
        icon = self.stage / "pdfcraft-nightly.png"
        icon.unlink()
        icon.mkdir()
        with self.assertRaisesRegex(ValueError, "regular file"):
            ARCHIVE.linux_archive(self.stage, self.output)
        self.assertFalse(self.output.exists())

    def test_long_notice_refuses_before_output(self):
        (self.stage / ("OFL-" + "x" * 100 + ".txt")).write_bytes(b"fixture")
        with self.assertRaisesRegex(ValueError, "short regular"):
            ARCHIVE.linux_archive(self.stage, self.output)
        self.assertFalse(self.output.exists())

    def test_case_colliding_notices_refuse_before_output(self):
        for name in ["OFL-Font.txt", "OFL-font.txt"]:
            (self.stage / name).write_bytes(b"fixture")
        with self.assertRaisesRegex(ValueError, "case-colliding"):
            ARCHIVE.linux_archive(self.stage, self.output)
        self.assertFalse(self.output.exists())

    def test_cli_dispatches_only_the_supported_archive_kind(self):
        helper = HERE / "nightly-archive.py"
        with mock.patch.object(sys, "argv", [str(helper), "linux", str(self.stage), str(self.output)]):
            runpy.run_path(str(helper), run_name="__main__")
        with tarfile.open(self.output) as archive:
            self.assertEqual({member.name for member in archive}, self.required)
        refused = self.root / "refused.tar.gz"
        for args in ([str(helper)], [str(helper), "unknown", str(self.stage), str(refused)]):
            with self.subTest(args=args), mock.patch.object(sys, "argv", args):
                with self.assertRaises(SystemExit):
                    runpy.run_path(str(helper), run_name="__main__")
                self.assertFalse(refused.exists())

    def test_existing_archive_is_unchanged(self):
        self.output.write_bytes(b"owned earlier artifact")
        with self.assertRaises(FileExistsError):
            ARCHIVE.linux_archive(self.stage, self.output)
        self.assertEqual(self.output.read_bytes(), b"owned earlier artifact")

    def test_archive_output_outside_build_root_refuses(self):
        with tempfile.TemporaryDirectory() as external:
            output = Path(external) / "nightly.tar.gz"
            with self.assertRaises(ValueError):
                ARCHIVE.linux_archive(self.stage, output)
            self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
