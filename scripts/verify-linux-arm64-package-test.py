#!/usr/bin/env python3
"""Compile-free fixtures for the published ARM64 package compatibility gate."""
import importlib.util
import io
from pathlib import Path
import struct
import tarfile
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("gate", Path(__file__).with_name("verify-linux-arm64-package.py"))
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


def executable(version="GLIBC_2.35", machine=183):
    strings = b"\0libc.so.6\0" + version.encode() + b"\0"
    needs = struct.pack("<HHIII", 1, 1, 1, 16, 0)
    needs += struct.pack("<IHHII", 0, 0, 2, 11, 0)
    blob = bytearray(64)
    blob[:7] = b"\x7fELF\x02\x01\x01"
    struct.pack_into("<HH", blob, 16, 3, machine)
    struct.pack_into("<Q", blob, 40, 64)
    struct.pack_into("<HH", blob, 58, 64, 3)
    blob += bytes(64)  # null section
    blob += struct.pack("<IIQQQQIIQQ", 0, 3, 0, 0, 256, len(strings), 0, 0, 1, 0)
    blob += struct.pack("<IIQQQQIIQQ", 0, 0x6FFFFFFE, 0, 0, 256 + len(strings), len(needs), 1, 1, 4, 0)
    return bytes(blob) + strings + needs


def package(path, contents):
    with tarfile.open(path, "w:gz") as archive:
        for name, data in contents:
            entry = tarfile.TarInfo(gate.PACKAGE_ROOT + "/" + name)
            entry.mode = 0o755
            entry.size = len(data)
            archive.addfile(entry, io.BytesIO(data))


class CompatibilityTests(unittest.TestCase):
    def test_target_baseline_and_older_versions(self):
        for version in ("GLIBC_2.17", "GLIBC_2.34", "GLIBC_2.35"):
            self.assertEqual(gate.glibc_requirements(executable(version)), [version])

    def test_known_ci_regression_is_rejected(self):
        for version in ("GLIBC_2.38", "GLIBC_2.39", "GLIBC_2.100"):
            with self.assertRaisesRegex(ValueError, "target supports GLIBC_2.35"):
                gate.glibc_requirements(executable(version))

    def test_private_glibc_requirement_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "unsupported libc"):
            gate.glibc_requirements(executable("GLIBC_PRIVATE"))

    def test_debug_strings_are_not_version_requirements(self):
        self.assertEqual(gate.glibc_requirements(executable() + b"GLIBC_2.99\0"), ["GLIBC_2.35"])

    def test_other_architecture_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "AArch64"):
            gate.glibc_requirements(executable(machine=62))

    def test_version_record_counts_and_chain_termination_are_checked(self):
        for offset, value in ((192 + 44, 0), (192 + 44, 2),
                              (len(executable()) - 20, 16), (len(executable()) - 4, 16)):
            blob = bytearray(executable("GLIBC_2.39"))
            struct.pack_into("<I", blob, offset, value)
            with self.assertRaisesRegex(ValueError, "version-needs"):
                gate.glibc_requirements(blob)

    def test_special_or_shared_write_permissions_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "package.tar.gz"
            for mode in (0o4755, 0o775, 0o757, 0o777):
                with tarfile.open(path, "w:gz") as archive:
                    entry = tarfile.TarInfo(gate.PACKAGE_ROOT + "/elastos")
                    entry.mode = mode
                    entry.size = len(executable())
                    archive.addfile(entry, io.BytesIO(executable()))
                with self.assertRaisesRegex(ValueError, "unexpected package member"):
                    gate.verify_package(path)

    def test_archive_resource_limits(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "package.tar.gz"
            package(path, [("elastos", executable()), ("model-provider", executable())])
            for limit in ("MAX_EXECUTABLE_BYTES", "MAX_PACKAGE_BYTES", "MAX_EXECUTABLES"):
                with patch.object(gate, limit, 1), self.assertRaises(ValueError):
                    gate.verify_package(path)

    def test_every_provider_is_checked(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "package.tar.gz"
            package(path, [("elastos", executable()), ("model-provider", executable()),
                           ("chain-provider", executable("GLIBC_2.39"))])
            with self.assertRaisesRegex(ValueError, "chain-provider: requires GLIBC_2.39"):
                gate.verify_package(path)

    def test_package_hash_receipt_and_required_members(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "package.tar.gz"
            package(path, [("elastos", executable()), ("model-provider", executable())])
            receipt = gate.verify_package(path)
            self.assertEqual(set(receipt["executables"]), {"elastos", "model-provider"})
            self.assertEqual(len(receipt["package_sha256"]), 64)
            package(path, [("elastos", executable())])
            with self.assertRaisesRegex(ValueError, "requires Runtime and model-provider"):
                gate.verify_package(path)

    def test_duplicate_and_malformed_members_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "package.tar.gz"
            for contents in ([("elastos", executable())] * 2,
                             [("elastos", executable()[:-5])],
                             [("elastos", b"not ELF")],
                             [("../elastos", executable())]):
                package(path, contents)
                with self.assertRaises(ValueError):
                    gate.verify_package(path)


if __name__ == "__main__":
    unittest.main()
