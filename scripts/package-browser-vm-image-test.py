#!/usr/bin/env python3
"""Source-only image packaging tests: real archives, explicitly mocked preflight.

Tiny payloads are not ext4 images and do not qualify a host or Browser Engine.
The preflight mock checks fixture hashes and the real host-to-guest mapping;
native preflight, guest filesystems and installed delivery retain their own tests.
"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location(
    "browser_image_packager", Path(__file__).with_name("package-browser-vm-image.py")
)
packager = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(packager)
FILES = ("rootfs.ext4", "vmlinux", "initrd", "browser-vm-rootfs-manifest.json")


def sha(data):
    return hashlib.sha256(data).hexdigest()



_GUEST_SPEC = importlib.util.spec_from_file_location("guest_inputs", Path(__file__).with_name("browser-vm-image-inputs.py"))
guest_inputs = importlib.util.module_from_spec(_GUEST_SPEC)
_GUEST_SPEC.loader.exec_module(guest_inputs)

class BrowserImagePackageTest(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.root = Path(scratch.name).resolve()
        self.image = self.root / "image"
        self.image.mkdir()
        self.archive = self.root / "release" / "browser-image.tar.gz"
        self.metadata = self.root / "release" / "components-overlay.json"
        self.platform = "darwin-arm64"
        self.payloads = {
            "rootfs.ext4": b"fixture rootfs bytes",
            "vmlinux": b"fixture kernel bytes",
            "initrd": b"fixture initrd bytes",
        }
        for name, payload in self.payloads.items():
            (self.image / name).write_bytes(payload)
        self.receipt = {
            "schema": "elastos.browser.vm-rootfs-build/v1", "ok": True,
                   "inputs_sha256": guest_inputs.identity()["sha256"],
                   "recipe_options": guest_inputs.identity()["options"],
            # Mac ARM64 runs the Linux ARM64 guest, as artifact preflight requires.
            "target_platform": "linux-arm64",
            "size": len(self.payloads["rootfs.ext4"]),
            "sha256": sha(self.payloads["rootfs.ext4"]),
            "rootfs": "/operator/private/rootfs.ext4",
            "build_workspace": "/operator/private/source",
            "kernel": {"size": len(self.payloads["vmlinux"]),
                       "sha256": sha(self.payloads["vmlinux"]),
                       "path": "/operator/private/vmlinux"},
            "initrd": {"size": len(self.payloads["initrd"]),
                       "sha256": sha(self.payloads["initrd"]),
                       "path": "/operator/private/initrd"},
            "preflight": {
                "ok": True, "audio_default_ready": True,
                "manifest": {"schema": "elastos.browser.vm-target/v1",
                             "runtime_network_only": True},
                "missing": [], "manifest_errors": [], "script_errors": [],
                "required": {"init": {"ok": True, "path": "/operator/private/init"}},
                "optional_audio": {"audio": {"ok": True, "path": "/operator/private/audio"}},
                "path": "/operator/private/preflight",
            },
        }
        self.write_receipt()
        self.preflight = patch.object(packager.subprocess, "run", side_effect=self.verify_fixture)
        self.mock_preflight = self.preflight.start()
        self.addCleanup(self.preflight.stop)

    def write_receipt(self):
        (self.image / FILES[-1]).write_text(json.dumps(self.receipt))

    def verify_fixture(self, command, *, env, capture_output, text, timeout):
        self.assertEqual(Path(command[0]).name, "browser-vm-artifact-preflight.sh")
        self.assertEqual(command[1:], ["--verify-image-set"])
        self.assertTrue(capture_output and text)
        self.assertEqual(timeout, 600)
        self.assertEqual(env["ELASTOS_BROWSER_VM_PLATFORM"], self.platform)
        self.assertNotIn("ELASTOS_BROWSER_VM_STAGED_ROOTFS", env)
        self.assertEqual(env["ELASTOS_BROWSER_VM_ROOTFS"], str(self.image / "rootfs.ext4"))
        self.assertEqual(env["ELASTOS_BROWSER_VM_KERNEL"], str(self.image / "vmlinux"))
        init_key = "ELASTOS_BROWSER_VM_INITRAMFS" if self.platform == "darwin-arm64" else "ELASTOS_BROWSER_VM_INITRD"
        self.assertEqual(env[init_key], str(self.image / "initrd"))
        receipt = json.loads((self.image / FILES[-1]).read_bytes())
        expected_guest = "linux-amd64" if self.platform == "linux-amd64" else "linux-arm64"
        valid = receipt["target_platform"] == expected_guest and receipt["ok"] is True
        for name, entry in (("rootfs.ext4", receipt), ("vmlinux", receipt["kernel"]), ("initrd", receipt["initrd"])):
            data = (self.image / name).read_bytes()
            valid = valid and entry["sha256"] == sha(data) and entry["size"] == len(data)
        return subprocess.CompletedProcess(command, 0 if valid else 1,
            json.dumps({"schema": "elastos.browser.vm-image-set/v1", "ok": valid}), "")

    def package(self, archive=None, metadata=None):
        return packager.package(self.image, self.platform, archive or self.archive,
                                metadata or self.metadata, "browser/image.tar.gz")

    def outputs(self):
        return {path: (path.read_bytes(), path.stat().st_dev, path.stat().st_ino)
                for path in (self.archive, self.metadata) if path.exists()}

    def existing_outputs(self):
        self.archive.parent.mkdir()
        self.archive.write_bytes(b"existing archive")
        self.metadata.write_bytes(b"existing metadata")
        return self.outputs()

    def test_one_guest_archive_is_identical_for_mac_and_jetson(self):
        self.package()
        checksum = sha(self.archive.read_bytes())
        self.archive = self.root / "jetson.tar.gz"
        self.metadata = self.root / "jetson.json"
        self.platform = "linux-arm64"
        self.package()
        self.assertEqual(sha(self.archive.read_bytes()), checksum)

    def test_x86_local_image_is_refused(self):
        self.platform = "linux-amd64"
        with self.assertRaises(ValueError):
            self.package()
        self.assertFalse(self.archive.exists())

    def test_matching_input_cache_reuses_only_unchanged_payloads(self):
        inputs = {"sha256": self.receipt["inputs_sha256"]}
        self.assertTrue(guest_inputs.reusable(self.image, inputs))
        self.assertFalse(guest_inputs.reusable(self.image, {"sha256": "0" * 64}))
        (self.image / "vmlinux").write_bytes(b"changed kernel")
        with self.assertRaises(ValueError):
            guest_inputs.reusable(self.image, inputs)

    def test_recipe_options_must_be_complete_and_arm64(self):
        for options in ({}, {**guest_inputs.DEFAULT_OPTIONS, "target_platform": "linux-amd64"},
                        {**guest_inputs.DEFAULT_OPTIONS, "cdp_timeout_ms": "0"}):
            with self.subTest(options=options), self.assertRaises(ValueError):
                guest_inputs.identity(options=options)

    def test_complete_portable_archive_hash_size_and_deterministic_reuse(self):
        for platform in ("darwin-arm64", "linux-arm64"):
            with self.subTest(platform=platform):
                self.platform = platform
                self.receipt["target_platform"] = "linux-amd64" if platform == "linux-amd64" else "linux-arm64"
                self.write_receipt()
                self.archive = self.root / platform / "image.tar.gz"
                self.metadata = self.root / platform / "components.json"
                with patch.dict(os.environ, {"ELASTOS_BROWSER_VM_STAGED_ROOTFS": "/foreign/staged"}):
                    overlay = self.package()
                content = self.archive.read_bytes()
                info = overlay["external"]["browser-vm-image"]["platforms"][platform]
                self.assertEqual(info, {"install_path": "browser-vm/image-set", "extract_path": "browser-vm-image",
                    "strategy": "browser-vm-image", "release_path": "browser/image.tar.gz",
                    "checksum": "sha256:" + sha(content), "size": len(content)})
                self.assertEqual(json.loads(self.metadata.read_bytes()), overlay)
                self.assertEqual(content[4:8], bytes(4), "gzip time must be reproducible")
                with tarfile.open(self.archive, "r:gz") as archive:
                    self.assertEqual(archive.getnames(), ["browser-vm-image/" + name for name in sorted(FILES)])
                    for entry in archive.getmembers():
                        self.assertTrue(entry.isfile())
                        self.assertEqual((entry.mode, entry.mtime, entry.uid, entry.gid), (0o644, 0, 0, 0))
                    for name, payload in self.payloads.items():
                        self.assertEqual(archive.extractfile("browser-vm-image/" + name).read(), payload)
                    portable_bytes = archive.extractfile("browser-vm-image/" + FILES[-1]).read()
                self.assertNotIn(b"/operator/private", portable_bytes)
                portable = json.loads(portable_bytes)
                self.assertEqual(set(portable), {"schema", "ok", "target_platform", "size", "sha256", "kernel", "initrd", "preflight", "inputs_sha256", "recipe_options"})
                self.assertEqual(portable["preflight"]["required"], {"init": {"ok": True}})
                before = self.outputs()
                self.assertEqual(self.package(), overlay)
                self.assertEqual(self.outputs(), before, "reuse retains both output inodes and bytes")
                other_archive = self.archive.with_name("repeated.tar.gz")
                other_metadata = self.metadata.with_name("repeated.json")
                self.assertEqual(self.package(other_archive, other_metadata), overlay)
                self.assertEqual(other_archive.read_bytes(), content)
                self.assertEqual(other_metadata.read_bytes(), self.metadata.read_bytes())

    def test_missing_or_linked_source_preserves_existing_outputs(self):
        before = self.existing_outputs()
        for name in FILES:
            source = self.image / name
            payload = source.read_bytes()
            for linked in (False, True):
                with self.subTest(name=name, linked=linked):
                    source.unlink()
                    if linked:
                        foreign = self.root / (name + ".foreign")
                        foreign.write_bytes(payload)
                        source.symlink_to(foreign)
                    try:
                        with self.assertRaisesRegex(ValueError, "four regular source files"):
                            self.package()
                        self.assertEqual(self.outputs(), before)
                    finally:
                        source.unlink(missing_ok=True)
                        source.write_bytes(payload)
        self.mock_preflight.assert_not_called()

    def test_corrupt_or_wrong_guest_source_fails_verification_and_preserves_outputs(self):
        before = self.existing_outputs()
        for name in self.payloads:
            with self.subTest(name=name):
                (self.image / name).write_bytes(b"corrupt")
                try:
                    with self.assertRaisesRegex(ValueError, "failed verification"):
                        self.package()
                    self.assertEqual(self.outputs(), before)
                finally:
                    (self.image / name).write_bytes(self.payloads[name])
        self.receipt["target_platform"] = "darwin-arm64"
        self.write_receipt()
        with self.assertRaisesRegex(ValueError, "failed verification"):
            self.package()
        self.assertEqual(self.outputs(), before)

    def test_source_substitution_during_archive_read_is_refused(self):
        original_addfile = tarfile.TarFile.addfile
        for name, expected in self.payloads.items():
            with self.subTest(name=name):
                self.archive = self.root / "substitution" / (name + ".tar.gz")
                self.metadata = self.root / "substitution" / (name + ".json")
                source = self.image / name

                def substitute(archive, entry, reader=None):
                    if entry.name == "browser-vm-image/" + name:
                        # A concurrent build changes the opened inode while tar reads it,
                        # then restores pathname bytes before the final source scan.
                        source.write_bytes(b"X" * len(expected))
                        try:
                            return original_addfile(archive, entry, reader)
                        finally:
                            source.write_bytes(expected)
                    return original_addfile(archive, entry, reader)

                with patch.object(tarfile.TarFile, "addfile", substitute):
                    try:
                        self.package()
                    except ValueError as error:
                        self.assertRegex(str(error), "source image changed|hash|checksum")
                    else:
                        with tarfile.open(self.archive, "r:gz") as archive:
                            actual = archive.extractfile("browser-vm-image/" + name).read()
                        self.assertEqual(actual, b"X" * len(expected))
                        self.fail(f"packager published substituted {name} bytes: {sha(actual)} != {sha(expected)}")
                self.assertFalse(self.archive.exists())
                self.assertFalse(self.metadata.exists())
                self.assertEqual(source.read_bytes(), expected)

    def test_conflicting_output_metadata_preserves_both_outputs(self):
        self.package()
        self.metadata.write_bytes(b"foreign metadata")
        before = self.outputs()
        with self.assertRaisesRegex(ValueError, "outputs already exist"):
            self.package()
        self.assertEqual(self.outputs(), before)

    def test_publication_race_never_overwrites_foreign_archive(self):
        original_link = os.link

        def race(source, destination):
            if Path(destination) == self.archive:
                self.archive.write_bytes(b"foreign archive")
            return original_link(source, destination)

        with patch.object(packager.os, "link", race):
            with self.assertRaises(FileExistsError):
                self.package()
        self.assertEqual(self.archive.read_bytes(), b"foreign archive")
        self.assertFalse(self.metadata.exists())

    def test_metadata_race_removes_only_this_invocations_new_archive(self):
        original_link = os.link
        for replace_archive in (False, True):
            with self.subTest(replace_archive=replace_archive):
                self.archive.unlink(missing_ok=True)
                self.metadata.unlink(missing_ok=True)

                def race(source, destination):
                    if Path(destination) == self.metadata:
                        self.metadata.write_bytes(b"foreign metadata")
                        if replace_archive:
                            self.archive.unlink()
                            self.archive.write_bytes(b"replacement foreign archive")
                    return original_link(source, destination)

                with patch.object(packager.os, "link", race):
                    with self.assertRaises(FileExistsError):
                        self.package()
                self.assertEqual(self.metadata.read_bytes(), b"foreign metadata")
                if replace_archive:
                    self.assertEqual(self.archive.read_bytes(), b"replacement foreign archive")
                else:
                    self.assertFalse(self.archive.exists())


class GuestInputIdentityTest(unittest.TestCase):
    def test_only_guest_inputs_and_recipe_options_invalidate_reuse(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            subprocess.run(["git", "init", "--quiet", root], check=True)
            for name in guest_inputs.REQUIRED_FILES:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(b"guest input fixture")
            subprocess.run(["git", "-C", root, "add", "."], check=True)
            before = guest_inputs.identity(root=root)["sha256"]
            runtime = root / "elastos/crates/elastos-server/src/setup.rs"
            runtime.parent.mkdir(parents=True)
            runtime.write_bytes(b"Runtime changes separately")
            self.assertEqual(guest_inputs.identity(root=root)["sha256"], before)
            (root / guest_inputs.REQUIRED_FILES[0]).write_bytes(b"changed guest builder")
            self.assertNotEqual(guest_inputs.identity(root=root)["sha256"], before)
            options = {**guest_inputs.DEFAULT_OPTIONS, "rootfs_size": "16384M"}
            self.assertNotEqual(guest_inputs.identity(root=root, options=options)["sha256"],
                                guest_inputs.identity(root=root)["sha256"])


if __name__ == "__main__":
    unittest.main()
