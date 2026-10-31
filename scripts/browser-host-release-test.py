#!/usr/bin/env python3
"""Small fixtures for the Browser host helper release producer."""
import importlib.util
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.dont_write_bytecode = True
SOURCE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("browser_host_release", SOURCE / "browser-host-release.py")
producer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(producer)


class BrowserHostReleaseTest(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name) / "source"
        self.root.mkdir()
        (self.root / "scripts").mkdir()
        self.output = Path(self.scratch.name) / "output"
        self.output.mkdir()
        (self.output / "keep").write_bytes(b"existing release input")
        self.template = {
            "profiles": {"browser-host": {"components": ["control", "module"]}},
            "external": {
                name: {"install_path": install, "platforms": {
                    "darwin-arm64": {"source": source, "release_path": release}}}
                for name, install, source, release in [
                    ("control", "bin/browser-vm-control-service", "scripts/control.sh", "control-darwin-arm64"),
                    ("module", "bin/browser-vm-control-service.mjs", "scripts/control.mjs", "module-darwin-arm64"),
                ]
            },
        }
        (self.root / "scripts/control.sh").write_bytes(b"#!/bin/sh\nexec ./control.mjs \"$@\"\n")
        (self.root / "scripts/control.mjs").write_bytes(b"export const control = true;\n")
        self.write_template()

    def write_template(self):
        (self.root / "components.json").write_text(json.dumps(self.template))

    def snapshot(self):
        return {path.name: ("link", os.readlink(path)) if path.is_symlink()
                else ("dir",) if path.is_dir()
                else ("file", path.read_bytes(), path.stat().st_mode)
                for path in self.output.iterdir()}

    def assert_refused(self, platform="darwin-arm64", native_components=()):
        self.write_template()
        before = self.snapshot()
        with self.assertRaises(ValueError):
            producer.stage(self.root, platform, self.output, native_components)
        self.assertEqual(self.snapshot(), before)

    def test_stages_exact_bytes_and_effective_platform_binding(self):
        self.template["external"]["module"]["platforms"]["darwin-arm64"]["install_path"] = "scripts/control.mjs"
        del self.template["external"]["module"]["install_path"]
        self.template["profiles"]["browser-host"]["components"].extend(["native", "linux-only"])
        self.template["external"]["native"] = {"platforms": {"darwin-arm64": {"release_path": "native"}}}
        self.template["external"]["linux-only"] = {"platforms": {"linux-arm64": {"source": "scripts/linux.sh"}}}
        self.write_template()
        result = producer.stage(self.root, "darwin-arm64", self.output, ["native"])
        self.assertEqual(set(result["external"]), {"control", "module"})
        for name, source in [("control", "control.sh"), ("module", "control.mjs")]:
            self.assertEqual(set(result["external"][name]["platforms"]), {"darwin-arm64"})
            info = result["external"][name]["platforms"]["darwin-arm64"]
            data = (self.root / "scripts" / source).read_bytes()
            self.assertEqual((self.output / info["release_path"]).read_bytes(), data)
            self.assertEqual(info["checksum"], "sha256:" + hashlib.sha256(data).hexdigest())
            self.assertEqual(info["size"], len(data))
            self.assertEqual((self.output / info["release_path"]).stat().st_mode & 0o777, 0o755)
        self.assertEqual(result["external"]["module"]["platforms"]["darwin-arm64"]["install_path"], "scripts/control.mjs")
        self.assertEqual((self.output / "keep").read_bytes(), b"existing release input")

    def test_supported_platforms_remain_distinct(self):
        for platform in producer.PLATFORMS:
            with self.subTest(platform=platform):
                template = copy.deepcopy(self.template)
                for component in template["external"].values():
                    component["platforms"] = {platform: component["platforms"]["darwin-arm64"]}
                output = self.output / platform
                (self.root / "components.json").write_text(json.dumps(template))
                result = producer.stage(self.root, platform, output)
                self.assertEqual(set(result["external"]["control"]["platforms"]), {platform})

    def test_role_metadata_is_required(self):
        original = copy.deepcopy(self.template)
        for profiles in [None, {}, {"browser-host": None}, {"browser-host": {}},
                         {"browser-host": {"components": []}},
                         {"browser-host": {"components": "control"}},
                         {"browser-host": {"components": ["control", "control"]}},
                         {"browser-host": {"components": ["unknown"]}}]:
            with self.subTest(profiles=profiles):
                self.template = {**copy.deepcopy(original), "profiles": profiles}
                self.assert_refused()

    def test_unsupported_platform_is_refused(self):
        self.assert_refused("darwin-amd64")

    def test_missing_selected_source_requires_explicit_native_handoff(self):
        del self.template["external"]["module"]["platforms"]["darwin-arm64"]["source"]
        self.assert_refused()

    def test_invalid_native_handoff_is_refused(self):
        self.template["profiles"]["browser-host"]["components"].extend(["native", "linux-only"])
        self.template["external"]["native"] = {"platforms": {"darwin-arm64": {"release_path": "native"}}}
        self.template["external"]["linux-only"] = {"platforms": {"linux-arm64": {"release_path": "linux-only"}}}
        for handoff in [["unknown"], ["native", "native"], ["native", "linux-only"], ["module"], "native", [None]]:
            with self.subTest(handoff=handoff):
                self.assert_refused(native_components=handoff)

    def test_native_handoff_cannot_suppress_malformed_script_source(self):
        self.template["external"]["module"]["platforms"]["darwin-arm64"]["source"] = None
        self.assert_refused(native_components=["module"])

    def test_native_only_role_has_an_explicit_completed_handoff(self):
        self.template["profiles"]["browser-host"]["components"] = ["native"]
        self.template["external"]["native"] = {"platforms": {"darwin-arm64": {"release_path": "native"}}}
        self.write_template()
        before = self.snapshot()
        result = producer.stage(self.root, "darwin-arm64", self.output, ["native"])
        self.assertEqual(result, {"external": {}})
        self.assertEqual(self.snapshot(), before)

    def test_cli_requires_and_accepts_explicit_native_handoff(self):
        self.template["profiles"]["browser-host"]["components"].append("native")
        self.template["external"]["native"] = {"platforms": {"darwin-arm64": {"release_path": "native"}}}
        self.write_template()
        command = [sys.executable, "-B", str(SOURCE / "browser-host-release.py"), "--root", str(self.root),
                   "--platform", "darwin-arm64", "--output", str(self.output)]
        before = self.snapshot()
        refused = subprocess.run(command, capture_output=True, text=True, timeout=5)
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn("explicit native handoff", refused.stderr)
        self.assertEqual(self.snapshot(), before)
        accepted = subprocess.run(command + ["--native-component", "native"], capture_output=True, text=True, timeout=5)
        self.assertEqual(accepted.returncode, 0, accepted.stderr)
        self.assertEqual(set(json.loads(accepted.stdout)["external"]), {"control", "module"})

    def test_malformed_helper_paths_are_refused(self):
        original = copy.deepcopy(self.template)
        cases = {
            "source": [None, 1, "", "/scripts/x", "scripts/../x", "scripts//x", "scripts/x/", "scripts/a\\b"],
            "release_path": [None, 1, "", ".", "..", "/x", "a/b", "a\\b", "x\n"],
            "install_path": [None, 1, "", "bin/..", "bin/.", "bin//x", "/bin/x", "other/x", "scripts/a\\b"],
        }
        for field, values in cases.items():
            for value in values:
                with self.subTest(field=field, value=value):
                    self.template = copy.deepcopy(original)
                    self.template["external"]["module"]["platforms"]["darwin-arm64"][field] = value
                    self.assert_refused()

    def test_duplicate_release_and_install_paths_are_refused(self):
        original = copy.deepcopy(self.template)
        for field, value in [("release_path", "control-darwin-arm64"), ("install_path", "bin/browser-vm-control-service")]:
            with self.subTest(field=field):
                self.template = copy.deepcopy(original)
                self.template["external"]["module"]["platforms"]["darwin-arm64"][field] = value
                self.assert_refused()

    def test_missing_later_source_publishes_nothing(self):
        (self.root / "scripts/control.mjs").unlink()
        self.assert_refused()

    def test_linked_or_nonregular_later_source_is_refused(self):
        source = self.root / "scripts/control.mjs"
        for kind in ["link", "directory", "fifo"]:
            with self.subTest(kind=kind):
                source.unlink()
                if kind == "link":
                    source.symlink_to("control.sh")
                elif kind == "directory":
                    source.mkdir()
                else:
                    os.mkfifo(source)
                self.assert_refused()
                if source.is_dir():
                    source.rmdir()
                else:
                    source.unlink()
                source.write_bytes(b"restored module")

    def test_linked_source_directory_is_refused(self):
        scripts = self.root / "scripts"
        scripts.rename(self.root / "elsewhere")
        scripts.symlink_to("elsewhere", target_is_directory=True)
        self.assert_refused()

    def test_later_existing_output_is_preserved(self):
        later = self.output / "module-darwin-arm64"
        for kind in ["file", "directory", "dangling-link"]:
            with self.subTest(kind=kind):
                if kind == "file":
                    later.write_bytes(b"prior artifact")
                elif kind == "directory":
                    later.mkdir()
                else:
                    later.symlink_to("absent")
                self.assert_refused()
                if later.is_dir():
                    later.rmdir()
                else:
                    later.unlink()

    def test_refusal_creates_no_output_directory(self):
        self.output = Path(self.scratch.name) / "absent-output"
        (self.root / "scripts/control.mjs").unlink()
        with self.assertRaises(ValueError):
            producer.stage(self.root, "darwin-arm64", self.output)
        self.assertFalse(self.output.exists())

    def test_write_failure_removes_only_new_helpers(self):
        original_open = Path.open
        def failing_open(path, *args, **kwargs):
            if path == self.output / "module-darwin-arm64":
                raise OSError("fixture write failure")
            return original_open(path, *args, **kwargs)
        before = self.snapshot()
        with mock.patch.object(Path, "open", failing_open), self.assertRaisesRegex(OSError, "fixture write failure"):
            producer.stage(self.root, "darwin-arm64", self.output)
        self.assertEqual(self.snapshot(), before)

    def test_write_failure_keeps_foreign_replacement(self):
        original_open = Path.open
        first = self.output / "control-darwin-arm64"
        replacement = Path(self.scratch.name) / "replacement"
        replacement.write_bytes(b"foreign replacement")
        def failing_open(path, *args, **kwargs):
            if path == self.output / "module-darwin-arm64":
                replacement.replace(first)
                raise OSError("fixture write failure after replacement")
            return original_open(path, *args, **kwargs)
        with mock.patch.object(Path, "open", failing_open), self.assertRaisesRegex(OSError, "after replacement"):
            producer.stage(self.root, "darwin-arm64", self.output)
        self.assertEqual(first.read_bytes(), b"foreign replacement")
        self.assertEqual((self.output / "keep").read_bytes(), b"existing release input")
        self.assertFalse((self.output / "module-darwin-arm64").exists())

    def test_root_wrapper_relocation_and_argument_forwarding(self):
        bin_dir = Path(self.scratch.name) / "relocated/bin"
        bin_dir.mkdir(parents=True)
        wrapper = bin_dir / "browser-vm-control-service"
        shutil.copyfile(SOURCE / "browser-host-node-launcher.sh", wrapper)
        wrapper.chmod(0o755)
        module = bin_dir / "browser-vm-control-service.mjs"
        module.write_bytes(b"fixture module")
        node = bin_dir / "node"
        node.write_text("#!" + sys.executable + "\nimport json,sys\nprint(json.dumps(sys.argv[1:]))\n")
        node.chmod(0o755)
        result = subprocess.run([str(wrapper), "space argument", "--flag", ""], capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), [str(module), "space argument", "--flag", ""])
        self.assertEqual(module.read_bytes(), b"fixture module")

    def test_root_wrapper_missing_managed_node_refuses_ambient_node(self):
        bin_dir = Path(self.scratch.name) / "relocated/bin"
        bin_dir.mkdir(parents=True)
        wrapper = bin_dir / "browser-vm-control-service"
        shutil.copyfile(SOURCE / "browser-host-node-launcher.sh", wrapper)
        wrapper.chmod(0o755)
        trap_dir = Path(self.scratch.name) / "ambient"
        trap_dir.mkdir()
        marker = Path(self.scratch.name) / "ambient-used"
        trap = trap_dir / "node"
        trap.write_text("#!" + sys.executable + "\nfrom pathlib import Path\nPath(" + repr(str(marker)) + ").write_text('used')\n")
        trap.chmod(0o755)
        result = subprocess.run([str(wrapper)], env={**os.environ, "PATH": str(trap_dir) + os.pathsep + os.environ.get("PATH", "")},
                                capture_output=True, text=True, timeout=5)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("managed Node", result.stderr)
        self.assertFalse(marker.exists())


if __name__ == "__main__":
    unittest.main()
