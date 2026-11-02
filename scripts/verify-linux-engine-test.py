#!/usr/bin/env python3
"""Inert engine archives exercise the real Linux dependency verifier."""
import importlib.util
import io
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("engine", Path(__file__).with_name("verify-linux-engine.py"))
engine = importlib.util.module_from_spec(spec)
spec.loader.exec_module(engine)


class EngineDependencyTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.bundle = self.root / "llama-b10516"
        self.bundle.mkdir()
        for name in ("llama-server", "libllama.so", "libggml.so"):
            (self.bundle / name).write_bytes(b"\x7fELFinert fixture")
        self.fault = None
        self.ldd_paths = []

    def inspect(self, args, **kwargs):
        name = Path(args[-1]).name
        self.assertEqual(kwargs["env"]["LC_ALL"], "C")
        self.assertNotIn("LD_LIBRARY_PATH", kwargs["env"])
        self.assertNotIn("LD_PRELOAD", kwargs["env"])
        if args[0] == "ldd":
            self.ldd_paths.append(name)
            return "libm.so.6 => not found" if self.fault == "unresolved" and name == "libggml.so" else "resolved libraries"
        if args[1] == "-h":
            return "Class: ELF64\nMachine: Advanced Micro Devices X86-64\n"
        if args[1] == "-d":
            library = "libgomp.so.1" if self.fault == "libgomp" and name == "libggml.so" else "libc.so.6"
            return f"(RUNPATH) [$ORIGIN]\n(NEEDED) [{library}]\n"
        return "GLIBC_2.36" if self.fault == "glibc" else "GLIBC_2.35"

    def verify(self):
        with patch.dict(os.environ, {"LD_LIBRARY_PATH": "/host", "LD_PRELOAD": "/host/lib.so"}), \
             patch.object(engine.subprocess, "check_output", side_effect=self.inspect), patch("builtins.print"):
            return engine.verify_bundle(self.bundle, "amd64")

    def test_every_executable_and_shared_library_is_checked(self):
        self.assertEqual(self.verify(), 3)
        self.assertEqual(set(self.ldd_paths), {"llama-server", "libllama.so", "libggml.so"})

    def test_host_installed_libgomp_cannot_hide_an_unbundled_dependency(self):
        self.fault = "libgomp"
        with self.assertRaisesRegex(ValueError, "libggml.so: libgomp.so.1"):
            self.verify()

    def test_unresolved_library_in_a_non_entrypoint_member_is_rejected(self):
        self.fault = "unresolved"
        with self.assertRaisesRegex(ValueError, "unresolved engine dependency: libggml.so"):
            self.verify()

    def test_supported_glibc_baseline_is_required(self):
        self.fault = "glibc"
        with self.assertRaisesRegex(ValueError, "GLIBC_2.36"):
            self.verify()

    def test_archive_members_are_checked_with_the_same_gate(self):
        archive = self.root / "engine.tar.gz"
        with tarfile.open(archive, "w:gz", format=tarfile.GNU_FORMAT) as target:
            target.add(self.bundle, arcname=self.bundle.name)
        with patch.object(engine.subprocess, "check_output", side_effect=self.inspect), patch("builtins.print"):
            self.assertEqual(engine.verify_archive(archive, self.bundle.name, "amd64"), 3)
            self.fault = "libgomp"
            with self.assertRaisesRegex(ValueError, "libgomp.so.1"):
                engine.verify_archive(archive, self.bundle.name, "amd64")

    def test_archive_escape_is_rejected_before_inspection(self):
        archive = self.root / "engine.tar.gz"
        with tarfile.open(archive, "w:gz", format=tarfile.GNU_FORMAT) as target:
            member = tarfile.TarInfo("../llama-server")
            member.size = 4
            target.addfile(member, io.BytesIO(b"\x7fELF"))
        with patch.object(engine.subprocess, "check_output") as inspect, self.assertRaises(ValueError):
            engine.verify_archive(archive, self.bundle.name, "amd64")
        inspect.assert_not_called()

    def test_ldd_execution_failure_is_rejected(self):
        with patch.object(engine.subprocess, "check_output", side_effect=subprocess.CalledProcessError(1, "ldd")):
            with self.assertRaises(subprocess.CalledProcessError):
                engine.verify_bundle(self.bundle, "amd64")

    def test_symlink_outside_bundle_is_rejected(self):
        (self.bundle / "libmissing.so").symlink_to("/host/libmissing.so")
        with self.assertRaisesRegex(ValueError, "unsafe engine symlink"):
            self.verify()


if __name__ == "__main__":
    unittest.main()
