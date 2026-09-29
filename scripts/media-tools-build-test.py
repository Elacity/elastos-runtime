#!/usr/bin/env python3
"""Cache provenance regressions; actual compiler/media proof is separate."""
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("builder", Path(__file__).with_name("media-tools-build.py"))
builder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(builder)


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
