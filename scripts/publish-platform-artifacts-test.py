#!/usr/bin/env python3
"""Check publisher platform selection and local exports without publishing."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


PUBLISHER = Path(__file__).resolve().with_name("publish-release.sh")


class PlatformArtifactExportTest(unittest.TestCase):
    def test_native_tools_use_host_os_while_guests_stay_linux(self):
        source = PUBLISHER.read_text()
        start = source.index("OS=$(uname -s")
        end = source.index("# ── Cross-compilation setup", start)
        platform_setup = source[start:end]
        native_call = next(
            line for line in source.splitlines()
            if line.startswith("HOST_PLATFORM_DIRECT_ASSETS=$(build_supported_direct_assets")
        )
        for system, machine, release_platform, setup_platform, native_target, guest_target in (
            ("Linux", "x86_64", "x86_64-linux", "linux-amd64", "x86_64-unknown-linux-musl", "x86_64-unknown-linux-musl"),
            ("Linux", "aarch64", "aarch64-linux", "linux-arm64", "aarch64-unknown-linux-musl", "aarch64-unknown-linux-musl"),
            ("Darwin", "arm64", "aarch64-darwin", "darwin-arm64", "aarch64-apple-darwin", "aarch64-unknown-linux-musl"),
        ):
            with self.subTest(system=system, machine=machine):
                result = subprocess.run(
                    ["bash", "-euc", '''
uname() { case "$1" in -s) echo "$TEST_SYSTEM";; -m) echo "$TEST_MACHINE";; *) return 1;; esac; }
info() { :; }
die() { echo "$*" >&2; exit 1; }
default_elastos_data_dir() { echo unused; }
build_supported_direct_assets() { printf '%s|%s|%s|%s' "$@"; }
''' + platform_setup + native_call + '''
printf '%s\\n' "$HOST_PLATFORM_DIRECT_ASSETS" "${GUEST_RUST_TARGET:-${HOST_RUST_TARGET}}"
'''],
                    env={**os.environ, "TEST_SYSTEM": system, "TEST_MACHINE": machine},
                    check=True,
                    capture_output=True,
                    text=True,
                )
                self.assertEqual(result.stdout.splitlines(), [
                    f"{release_platform}|{setup_platform}|{native_target}|false",
                    guest_target,
                ])

    def test_host_and_cross_manifests_keep_their_platform_names(self):
        source = PUBLISHER.read_text()
        start = source.index("# Persist stamped install.sh and metadata")
        end = source.index('info "Installer CID:', start)
        export = source[start:end]
        for host, cross in (
            ("x86_64-linux", "aarch64-linux"),
            ("aarch64-darwin", "aarch64-linux"),
            ("aarch64-linux", ""),
        ):
            with self.subTest(host=host, cross=cross), tempfile.TemporaryDirectory() as root:
                root = Path(root)
                inputs = root / "inputs"
                inputs.mkdir()
                for name in ("install.sh", "release.json", "release-head.json"):
                    (inputs / name).write_text(name)
                (inputs / "components.json").write_text(f"host:{host}")
                if cross:
                    # Prepare both names so this test detects the output naming
                    # defect independently of the temporary input naming change.
                    for name in (cross, cross.split("-")[0]):
                        (inputs / f"components-{name}.json").write_text(f"cross:{cross}")
                subprocess.run(
                    ["bash", "-euc", export + "\n:"],
                    cwd=root,
                    env={
                        **os.environ,
                        "TMPDIR": str(inputs),
                        "STAMPED_INSTALL": str(inputs / "install.sh"),
                        "PLATFORM": host,
                        "CROSS_PLATFORM": cross,
                        "CROSS_ARCH": cross.split("-")[0],
                    },
                    check=True,
                    capture_output=True,
                    text=True,
                )
                outputs = root / "artifacts"
                expected = {"install.sh", "release.json", "release-head.json", f"components-{host}.json"}
                if cross:
                    expected.add(f"components-{cross}.json")
                self.assertEqual({p.name for p in outputs.iterdir()}, expected)
                self.assertEqual((outputs / f"components-{host}.json").read_text(), f"host:{host}")
                if cross:
                    self.assertEqual((outputs / f"components-{cross}.json").read_text(), f"cross:{cross}")


if __name__ == "__main__":
    unittest.main()
