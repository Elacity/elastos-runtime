#!/usr/bin/env python3
"""Source and cache provenance regressions; compiler/media proof is separate."""
import bz2
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("builder", Path(__file__).with_name("media-tools-build.py"))
builder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(builder)


class SourcePinTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "upstream"
        self.repo.mkdir()
        self.git("init", "--quiet")
        (self.repo / "COPYING").write_text("fixture license\n")
        (self.repo / "configure").write_text("#!/bin/sh\nexit 0\n")
        (self.repo / "configure").chmod(0o755)
        self.git("add", "COPYING", "configure")
        self.git("-c", "commit.gpgsign=false", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                 "commit", "--quiet", "-m", "fixture")
        self.commit = self.git("rev-parse", "HEAD").decode().strip()
        self.tree = self.git("rev-parse", "HEAD^{tree}").decode().strip()
        self.archive = self.root / f"x264-{self.commit}.tar.bz2"
        source = self.git("-c", "tar.umask=0002", "archive", "--format=tar",
                          f"--prefix=x264-{self.commit}/", self.commit)
        self.digest = hashlib.sha256(bz2.compress(source)).hexdigest()
        for key, value in (("X264_COMMIT", self.commit), ("X264_TREE", self.tree)):
            pin = patch.object(builder, key, value)
            pin.start()
            self.addCleanup(pin.stop)

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.repo), *args])

    def fetch(self, digest=None):
        builder.fetch_source(self.archive, self.repo.as_uri(), digest or self.digest)

    def test_repeated_fetch_preserves_source_license_and_modes(self):
        self.fetch()
        first = self.archive.read_bytes()
        self.archive.unlink()
        self.fetch()
        self.assertEqual(first, self.archive.read_bytes())
        with tarfile.open(self.archive) as archive:
            prefix = f"x264-{self.commit}/"
            self.assertEqual(archive.extractfile(prefix + "COPYING").read(), b"fixture license\n")
            self.assertEqual(archive.extractfile(prefix + "configure").read(), b"#!/bin/sh\nexit 0\n")
            self.assertTrue(archive.getmember(prefix + "configure").mode & 0o111)
        self.assertFalse(list(self.root.glob(".x264-source-*")))

    def test_wrong_tree_stops_before_archive_creation(self):
        with patch.object(builder, "X264_TREE", "0" * 40):
            with self.assertRaisesRegex(ValueError, "commit or tree"):
                self.fetch()
        self.assertFalse(self.archive.exists())
        self.assertFalse(list(self.root.glob(".x264-source-*")))

    def test_wrong_archive_checksum_rejects(self):
        with self.assertRaisesRegex(ValueError, "Source checksum mismatch"):
            self.fetch("0" * 64)

    def test_downloaded_source_still_checks_bytes(self):
        source = self.root / "source.tar"
        source.write_bytes(b"original")
        dest = self.root / "download.tar"
        digest = hashlib.sha256(source.read_bytes()).hexdigest()
        builder.fetch_source(dest, source.as_uri(), digest)
        source.write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "Source checksum mismatch"):
            builder.fetch_source(dest, source.as_uri(), digest)


class CacheProofTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.sources = {"fixture.tar": ("https://example.invalid/fixture.tar", hashlib.sha256(b"source").hexdigest())}
        self.patcher = patch.object(builder, "SOURCES", self.sources)
        self.patcher.start()
        self.addCleanup(self.patcher.stop)
        for name in ("bin/ffmpeg", "bin/ffprobe", "BUILD.md", "licenses/FFmpeg-COPYING.GPLv2",
                     "licenses/x264-COPYING", "sources/media-tools-build.py", "sources/build-media-tools.sh",
                     "sources/fixture.tar"):
            p = self.root / name
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_bytes(b"source")
            p.chmod(0o755 if name.startswith("bin/") else 0o644)
        digest = hashlib.sha256(b"source").hexdigest()
        self.expected = {"schema": "elastos.media-tools-build/v1", "platform": "darwin-arm64",
                         "recipe_sha256": digest, "wrapper_sha256": digest}
        self.refresh_receipt()

    def refresh_receipt(self):
        info = dict(self.expected)
        info["files"] = {str(p.relative_to(self.root)): {"sha256": builder.sha(p), "size": p.stat().st_size}
                         for p in self.root.rglob("*") if p.is_file() and p.name != "build-info.json"}
        (self.root / "build-info.json").write_text(json.dumps(info))

    def test_complete_cache_passes(self):
        builder.verify(self.root, self.expected)

    def test_removing_license_and_receipt_entry_rejects(self):
        (self.root / "licenses/x264-COPYING").unlink()
        self.refresh_receipt()
        with self.assertRaisesRegex(ValueError, "inventory"):
            builder.verify(self.root, self.expected)

    def test_replacing_source_or_recipe_and_updating_receipt_rejects(self):
        for name in ("sources/fixture.tar", "sources/media-tools-build.py", "sources/build-media-tools.sh"):
            with self.subTest(file=name):
                p = self.root / name
                p.write_bytes(b"changed")
                self.refresh_receipt()
                with self.assertRaisesRegex(ValueError, "pinned|recipe"):
                    builder.verify(self.root, self.expected)
                p.write_bytes(b"source")

    def test_symlinked_receipt_or_empty_directory_rejects(self):
        receipt = self.root / "build-info.json"
        content = receipt.read_text()
        receipt.unlink()
        receipt.symlink_to("BUILD.md")
        with self.assertRaisesRegex(ValueError, "nonregular"):
            builder.verify(self.root, self.expected)
        receipt.unlink()
        receipt.write_text(content)
        (self.root / "empty").mkdir()
        (self.root / "link").symlink_to("empty", target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "nonregular"):
            builder.verify(self.root, self.expected)


if __name__ == "__main__":
    unittest.main()
