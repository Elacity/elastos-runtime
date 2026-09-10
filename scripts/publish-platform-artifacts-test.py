#!/usr/bin/env python3
"""Check publisher platform selection and local exports without publishing."""

import os
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest


PUBLISHER = Path(__file__).resolve().with_name("publish-release.sh")
CHECKER = PUBLISHER.with_name("components-release-integrity-check.py")
spec = importlib.util.spec_from_file_location("components_integrity", CHECKER)
integrity = importlib.util.module_from_spec(spec)
spec.loader.exec_module(integrity)


class PlatformArtifactExportTest(unittest.TestCase):
    def test_artifact_gate_stops_before_release_signing(self):
        source = PUBLISHER.read_text()
        function = "stage_release_artifacts() {" + source.split(
            "stage_release_artifacts() {", 1
        )[1].split("\n}\n", 1)[0] + "\n}\n"
        start = source.index('PREPARED_ARTIFACTS_DIR="')
        gate = source[start:source.index("# ── Step 7:", start)]
        for failure in (None, "missing", "stale", "runtime", "manifest",
                        "cross-valid", "cross-missing", "cross-runtime", "cross-manifest", "cross-app"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as root:
                root = Path(root)
                app = b"app bytes"
                runtime = b"runtime"
                manifest = json.dumps({"external": {"home": {"platforms": {"*": {
                    "release_path": "home.tar.gz", "size": len(app),
                    "checksum": "sha256:" + hashlib.sha256(app).hexdigest(),
                }}}}}).encode()
                (root / "elastos").write_bytes(runtime)
                (root / "components.json").write_bytes(manifest)
                cross = bool(failure and failure.startswith("cross-"))
                cross_runtime = b"cross runtime"
                cross_manifest = manifest
                if cross:
                    (root / "cross-elastos").write_bytes(cross_runtime)
                    if failure == "cross-app":
                        other = json.loads(manifest)
                        other["external"]["home"]["platforms"]["*"]["checksum"] = "sha256:" + "0" * 64
                        cross_manifest = json.dumps(other).encode()
                    if failure != "cross-missing":
                        (root / "components-aarch64-linux.json").write_bytes(cross_manifest)
                universal = root / "supported-assets-universal"
                universal.mkdir()
                if failure != "missing":
                    (universal / "home.tar.gz").write_bytes(b"bad bytes" if failure == "stale" else app)
                result = subprocess.run(
                    ["bash", "-euc", '''
die() { echo "$*" >&2; exit 1; }
sha256() { python3 -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())' "$1"; }
''' + function + gate + 'touch "$TEST_SIGN_MARKER"'],
                    cwd=PUBLISHER.parent.parent,
                    env={
                        **os.environ,
                        "TMPDIR": str(root),
                        "TEST_SIGN_MARKER": str(root / "signing-reached"),
                        "STAGED_ELASTOS": str(root / "elastos"),
                        "ARTIFACTS_DIR": str(root / "artifacts"),
                        "PLATFORM": "aarch64-darwin", "SETUP_PLATFORM": "darwin-arm64",
                        "CROSS_PLATFORM": "aarch64-linux" if cross else "",
                        "CROSS_SETUP_PLATFORM": "linux-arm64" if cross else "",
                        "CROSS_ELASTOS": str(root / "cross-elastos") if cross else "",
                        "CROSS_ARCH": "aarch64" if cross else "",
                        "CROSS_BINARY_SHA256": "0" * 64 if failure == "cross-runtime" else hashlib.sha256(cross_runtime).hexdigest(),
                        "CROSS_COMPONENTS_SHA256": "0" * 64 if failure == "cross-manifest" else hashlib.sha256(cross_manifest).hexdigest(),
                        "BINARY_SHA256": "0" * 64 if failure == "runtime" else hashlib.sha256(runtime).hexdigest(),
                        "COMPONENTS_SHA256": "0" * 64 if failure == "manifest" else hashlib.sha256(manifest).hexdigest(),
                    },
                    capture_output=True, text=True,
                )
                if failure in (None, "cross-valid"):
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertTrue((root / "signing-reached").exists())
                else:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertFalse((root / "signing-reached").exists())
                    expected_error = {
                        "missing": "home.tar.gz", "stale": "artifact checksum mismatch",
                        "runtime": "Staged runtime differs", "manifest": "Staged components differ",
                        "cross-runtime": "Staged runtime differs", "cross-manifest": "Staged components differ",
                        "cross-missing": "components-aarch64-linux.json", "cross-app": "artifact checksum mismatch",
                    }[failure]
                    self.assertIn(expected_error, result.stderr)

    def test_advertised_provider_and_capsule_bytes_are_checked(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            payload = b"abc"
            checksum = "sha256:" + hashlib.sha256(payload).hexdigest()
            def descriptor(path):
                return {"release_path": path, "checksum": checksum, "size": len(payload)}
            metadata = descriptor("provider-metadata.tar.gz")
            data = {
                "external": {"provider": {
                    "platforms": {"linux-arm64": descriptor("provider")},
                    "capsule_metadata": {"platforms": {"*": metadata}},
                }},
                "capsules": {"shell": {
                    "platforms": ["aarch64-linux"], "sha256": checksum.split(":")[1], "size": 3,
                }},
            }
            files = ("provider", "provider-metadata.tar.gz", "shell-aarch64-linux.capsule.tar.gz")
            for name in files:
                (root / name).write_bytes(payload)
            audit = lambda: integrity.audit_release_artifacts(data, ["linux-arm64"], root)
            self.assertEqual(audit(), [])
            for bad in (None, [], "aarch64-linux", [False]):
                data["capsules"]["shell"]["platforms"] = bad
                self.assertTrue(audit(), bad)
            data["capsules"]["shell"]["platforms"] = ["aarch64-linux"]
            for name in files:
                (root / name).unlink()
                self.assertTrue(audit(), name)
                (root / name).write_bytes(b"def")
                self.assertTrue(audit(), name)
                (root / name).write_bytes(payload)
            for bad in ("../escape", "/absolute", "nested/../escape", "bad\\path"):
                metadata["release_path"] = bad
                self.assertTrue(audit(), bad)
            metadata["release_path"] = "provider-metadata.tar.gz"
            for size in (True, 0, -1, 4):
                metadata["size"] = size
                self.assertTrue(audit(), size)
            metadata["size"] = 3
            (root / "provider-metadata.tar.gz").unlink()
            (root / "provider-metadata.tar.gz").symlink_to(root / "provider")
            self.assertTrue(audit())
            (root / "provider-metadata.tar.gz").unlink()
            (root / "provider-metadata.tar.gz").mkdir()
            self.assertTrue(audit())
            self.assertTrue(integrity.audit_release_artifacts(data, [], root))

    def test_served_artifact_stage_includes_universal_apps(self):
        source = PUBLISHER.read_text()
        function = "stage_release_artifacts() {" + source.split(
            "stage_release_artifacts() {", 1
        )[1].split("\n}\n", 1)[0] + "\n}\n"
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            expected = {
                "elastos-aarch64-darwin": b"mac runtime",
                "elastos-aarch64-linux": b"linux runtime",
                "components-aarch64-darwin.json": b"mac components",
                "components-aarch64-linux.json": b"linux components",
                "home.tar.gz": b"universal app",
                "ipfs-provider-darwin-arm64": b"native mac provider",
                "ipfs-provider-linux-arm64": b"native linux provider",
                "ipfs-provider-capsule-metadata.tar.gz": b"provider metadata",
                "shell-aarch64-darwin.capsule.tar.gz": b"mac capsule",
                "shell-aarch64-linux.capsule.tar.gz": b"linux capsule",
            }
            inputs = {
                "elastos": "elastos-aarch64-darwin",
                "cross-elastos": "elastos-aarch64-linux",
                "components.json": "components-aarch64-darwin.json",
                "components-aarch64-linux.json": "components-aarch64-linux.json",
                "supported-assets-universal/home.tar.gz": "home.tar.gz",
                "supported-assets-aarch64-darwin/ipfs-provider-darwin-arm64": "ipfs-provider-darwin-arm64",
                "supported-assets-aarch64-linux/ipfs-provider-linux-arm64": "ipfs-provider-linux-arm64",
                "supported-provider-contracts-universal/ipfs-provider-capsule-metadata.tar.gz": "ipfs-provider-capsule-metadata.tar.gz",
                "artifacts/shell.capsule.tar.gz": "shell-aarch64-darwin.capsule.tar.gz",
                "artifacts-aarch64/shell.capsule.tar.gz": "shell-aarch64-linux.capsule.tar.gz",
            }
            for path, artifact in inputs.items():
                path = root / path
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(expected[artifact])
            subprocess.run(
                ["bash", "-euc", function + 'stage_release_artifacts "$TEST_DEST"'],
                cwd=root,
                env={
                    **os.environ,
                    "TMPDIR": str(root),
                    "TEST_DEST": str(root / "served"),
                    "STAGED_ELASTOS": str(root / "elastos"),
                    "CROSS_ELASTOS": str(root / "cross-elastos"),
                    "ARTIFACTS_DIR": str(root / "artifacts"),
                    "PLATFORM": "aarch64-darwin",
                    "CROSS_PLATFORM": "aarch64-linux",
                    "CROSS_ARCH": "aarch64",
                },
                check=True, capture_output=True, text=True,
            )
            self.assertEqual(
                {p.name: p.read_bytes() for p in (root / "served").iterdir()}, expected
            )

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
