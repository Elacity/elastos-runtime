#!/usr/bin/env python3
"""Package names for hosts that already have a native release build."""

import json
import os
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/package-release-binaries.sh"


def package_name(system, machine):
    with tempfile.TemporaryDirectory() as directory:
        shim = Path(directory) / "uname"
        shim.write_text(
            "#!/bin/sh\n"
            f'SYSTEM={system!r}\nMACHINE={machine!r}\n'
            'case "$1" in -s) printf "%s\\n" "$SYSTEM" ;; *) printf "%s\\n" "$MACHINE" ;; esac\n'
        )
        shim.chmod(shim.stat().st_mode | stat.S_IEXEC)
        env = os.environ.copy()
        env["PATH"] = directory + os.pathsep + env["PATH"]
        return subprocess.run(
            ["bash", str(SCRIPT), "--print-platform"],
            cwd=ROOT, env=env, text=True, capture_output=True, timeout=15)


class PackageReleaseNameTest(unittest.TestCase):
    def test_native_hosts_use_the_release_platform_names(self):
        expected = {
            ("Linux", "x86_64"): "linux-amd64",
            ("Linux", "aarch64"): "linux-arm64",
            ("Linux", "arm64"): "linux-arm64",
            ("Darwin", "arm64"): "darwin-arm64",
            ("Darwin", "x86_64"): "darwin-amd64",
        }
        for host, name in expected.items():
            with self.subTest(host=host):
                result = package_name(*host)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.strip(), name)

    def test_intel_package_key_is_the_release_id_the_signer_refuses(self):
        platforms = json.loads((ROOT / "scripts/release-platforms.json").read_text())
        self.assertEqual(platforms["darwin-amd64"], "x86_64-darwin")
        signer = (ROOT / "scripts/release-signer.py").read_text()
        self.assertIn(
            'set(platforms) <= {"x86_64-linux", "aarch64-linux", "aarch64-darwin"}, "release platforms refused"',
            signer,
        )
        result = package_name("Darwin", "x86_64")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "darwin-amd64")

    def test_other_darwin_hosts_stop(self):
        result = package_name("Darwin", "i386")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Unsupported release platform: Darwin-i386", result.stderr)


if __name__ == "__main__":
    unittest.main()
