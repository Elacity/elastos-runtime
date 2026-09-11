#!/usr/bin/env python3
"""Verify safe rejection before raw boxed calls; native proof runs in Linux."""

import importlib
from pathlib import Path
import runpy
from types import ModuleType, SimpleNamespace
import unittest
from unittest.mock import patch


HERE = Path(__file__).parent
VERIFY = runpy.run_path(str(HERE / "browser-gst-python-smoke.py"))["verify"]


class GstPythonSmokeTests(unittest.TestCase):
    def exercise(self, *, missing=None, bad_structure=False, bad_fraction=False, module_error=None):
        calls = []
        class Structure:
            __module__ = "gi.overrides.Gst"
            def __init__(self, name):
                calls.append("structure")
                self.name, self.values = name, {}
            def set_value(self, key, value):
                self.values[key] = value
            def get_name(self):
                return "wrong" if bad_structure else self.name
            def get_value(self, key):
                return self.values[key]
        class Fraction:
            __module__ = "gi.overrides.Gst"
            def __init__(self, num, denom):
                calls.append("fraction")
                self.num, self.denom = num, 0 if bad_fraction else denom
        class Caps(Structure):
            def get_structure(self, index):
                return self
        if missing:
            {"structure": Structure, "fraction": Fraction}[missing].__module__ = "gi.repository.Gst"
        gst = SimpleNamespace(Structure=Structure, Fraction=Fraction,
            Caps=SimpleNamespace(from_string=Caps), init=lambda _: calls.append("init"),
            version_string=lambda: "test-only")
        gi = ModuleType("gi")
        gi.require_version = lambda *_: None
        gi.__version__ = "test-only"
        repository = ModuleType("gi.repository")
        repository.Gst = gst
        repository.GstWebRTC = repository.GstSdp = repository.GstRtp = None
        def import_module(name):
            self.assertEqual(name, "selkies_gstreamer.gstwebrtc_app")
            calls.append("module")
            if module_error:
                raise module_error
            return SimpleNamespace(GSTWebRTCApp=lambda **kw: calls.append(("app", kw)))
        self.calls = calls
        with patch.dict("sys.modules", {"gi": gi, "gi.repository": repository}), \
                patch.object(importlib, "import_module", side_effect=import_module):
            return VERIFY()

    def test_missing_override_rejects_before_any_boxed_constructor(self):
        for missing in ("structure", "fraction"):
            with self.subTest(missing=missing), self.assertRaisesRegex(RuntimeError, "python3-gst-1.0"):
                self.exercise(missing=missing)
            self.assertEqual(self.calls, [])

    def test_broken_structure_round_trip_rejects(self):
        with self.assertRaisesRegex(RuntimeError, "Structure"):
            self.exercise(bad_structure=True)
        self.assertNotIn("module", self.calls)

    def test_broken_fraction_round_trip_rejects(self):
        with self.assertRaisesRegex(RuntimeError, "Fraction"):
            self.exercise(bad_fraction=True)
        self.assertNotIn("module", self.calls)

    def test_real_module_import_failure_is_preserved(self):
        error = ImportError("missing real Selkies dependency")
        with self.assertRaises(ImportError) as caught:
            self.exercise(module_error=error)
        self.assertIs(caught.exception, error)

    def test_success_requires_actual_module_initialization(self):
        result = self.exercise()
        self.assertTrue(result["ok"])
        self.assertIn(("app", {"encoder": "openh264enc", "stun_servers": [], "turn_servers": []}), self.calls)

    def test_builder_checks_bindings_before_creating_image(self):
        source = (HERE / "build-browser-vm-rootfs.sh").read_text()
        packages = source.split("apt-get install --no-install-recommends", 1)[1].split('"$KERNEL_PACKAGE"', 1)[0]
        self.assertRegex(packages, r"(?m)^  python3-gst-1\.0 \\\s*$")
        self.assertLess(source.index("browser-gst-python-smoke.py"), source.index('as_root "$mke2fs_bin"'))


if __name__ == "__main__":
    unittest.main()
