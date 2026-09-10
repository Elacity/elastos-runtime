#!/usr/bin/env python3
"""Exercise unsigned input admission without builds, uploads or signing."""

import copy
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import struct
import subprocess
import tarfile
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch


spec = importlib.util.spec_from_file_location("platform_input", Path(__file__).with_name("release-platform-input.py"))
inputs = importlib.util.module_from_spec(spec)
spec.loader.exec_module(inputs)


def binary(platform):
    data = bytearray(64)
    if platform.endswith("-linux"):
        data[:7] = b"\x7fELF\x02\x01\x01"
        struct.pack_into("<HH", data, 16, 2, inputs.PLATFORMS[platform][2])
    else:
        data[:4] = b"\xcf\xfa\xed\xfe"
        struct.pack_into("<I", data, 4, inputs.PLATFORMS[platform][2])
        struct.pack_into("<I", data, 12, 2)
    return bytes(data)


def archive_bytes(entries=None):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w:gz") as archive:
        for name, payload, target in entries or [("home/index.html", b"Home", None)]:
            info = tarfile.TarInfo(name)
            if target is not None:
                info.type = tarfile.SYMTYPE
                info.linkname = target
                archive.addfile(info)
            else:
                info.size = len(payload)
                archive.addfile(info, io.BytesIO(payload))
    return output.getvalue()


class PlatformInputTest(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        self.app = archive_bytes()
        self.metadata = archive_bytes([
            ("shell/capsule.json", json.dumps({"name": "shell", "role": "provider", "icon": "icon"}).encode(), None),
            *[(f"shell/icon/icon-{size}.png", b"icon bytes", None) for size in inputs.integrity.PROVIDER_ICON_SIZES],
        ])
        self.template = {"schema": "elastos.components/v1", "capsules": {},
                         "profiles": {"home": {"components": ["home", "shell"]}},
                         "external": {
                             "home": {"platforms": {"*": {"install_path": "capsules/home", "release_path": "home.tar.gz"}}},
                             "shell": {"provider_runtime": {}, "platforms": {
                                 setup: {"install_path": "bin/shell", "release_path": f"shell-{setup}"}
                                 for setup, _, _ in inputs.PLATFORMS.values()}},
                         }}
        self.bundles = {p: self.make_bundle(p) for p in inputs.PLATFORMS}
        # The coordinator's reviewed source is a separate fixture from inputs.
        source_root = self.root / "source"
        source_root.mkdir()
        self.write_json(source_root / "components.json", self.template)
        source = json.loads((next(iter(self.bundles.values())) / "platform-input.json").read_text())["source"]
        for context in (patch.object(inputs, "SOURCE_ROOT", source_root),
                        patch.object(inputs, "source_identity", return_value=source)):
            context.start()
            self.addCleanup(context.stop)

    def write_json(self, path, data):
        path.write_text(json.dumps(data, sort_keys=True))

    def refresh(self, root):
        receipt_path = root / "platform-input.json"
        receipt = json.loads(receipt_path.read_text())
        receipt["files"] = {str(path.relative_to(root)): inputs.file_record(path)
                            for path in root.rglob("*") if path.is_file() and path != receipt_path}
        self.write_json(receipt_path, receipt)

    def make_bundle(self, platform):
        root = self.root / platform
        artifact_dir = root / "artifacts"
        artifact_dir.mkdir(parents=True)
        runtime = artifact_dir / f"elastos-{platform}"
        runtime.write_bytes(binary(platform))
        runtime.chmod(0o755)
        shell = artifact_dir / f"shell-{inputs.PLATFORMS[platform][0]}"
        shell.write_bytes(binary(platform))
        shell.chmod(0o755)
        (artifact_dir / "home.tar.gz").write_bytes(self.app)
        (artifact_dir / "shell-metadata.tar.gz").write_bytes(self.metadata)
        def descriptor(path, install, extract=None):
            result = {"release_path": path, "install_path": install,
                      "checksum": "sha256:" + inputs.digest(artifact_dir / path),
                      "size": (artifact_dir / path).stat().st_size}
            if extract:
                result["extract_path"] = extract
            return result
        manifest = copy.deepcopy(self.template)
        manifest["external"]["home"]["platforms"] = {"*": descriptor("home.tar.gz", "capsules/home", "home")}
        manifest["external"]["shell"]["platforms"] = {
            inputs.PLATFORMS[platform][0]: descriptor(shell.name, "bin/shell")}
        manifest["external"]["shell"]["capsule_metadata"] = {
            "install_path": "capsules/shell", "platforms": {
                "*": descriptor("shell-metadata.tar.gz", "capsules/shell", "shell")}}
        self.write_json(root / "components.json", manifest)
        self.write_json(root / "components-template.json", self.template)
        self.write_json(root / "platform-input.json", {
            "schema": inputs.SCHEMA, "source": {"commit": "a" * 40, "tree": "b" * 40,
                "clean": True, "lockfiles": {"elastos/Cargo.lock": "c" * 64}},
            "version": "0.7.1", "platform": platform, "target": inputs.PLATFORMS[platform][1],
            "omitted_platform_components": [], "tools": {"rustc": "test-rustc", "cargo": "test-cargo"},
        })
        self.refresh(root)
        return root

    def values(self):
        return [f"{platform}={root}" for platform, root in self.bundles.items()]

    def test_three_matching_platform_inputs_pass(self):
        self.assertEqual(set(inputs.validate_inputs(self.values())), set(inputs.PLATFORMS))

    def test_runtime_only_provider_metadata_follows_source_contract(self):
        root = self.bundles["aarch64-darwin"]
        template = json.loads((root / "components-template.json").read_text())
        manifest = json.loads((root / "components.json").read_text())
        del manifest["external"]["shell"]["capsule_metadata"]
        (root / "artifacts/shell-metadata.tar.gz").unlink()
        for value in (False, True, 1, "true"):
            template["external"]["shell"]["provider_runtime"]["runtime_only"] = value
            manifest["external"]["shell"]["provider_runtime"]["runtime_only"] = value
            self.write_json(root / "components-template.json", template)
            self.write_json(root / "components.json", manifest)
            self.refresh(root)
            with self.subTest(runtime_only=value):
                if value is True:
                    inputs.verify(root)
                else:
                    with self.assertRaisesRegex(ValueError, "provider capsule metadata is missing"):
                        inputs.verify(root)
        template["external"]["shell"]["provider_runtime"]["runtime_only"] = False
        manifest["external"]["shell"]["provider_runtime"]["runtime_only"] = True
        self.write_json(root / "components-template.json", template)
        self.write_json(root / "components.json", manifest)
        self.refresh(root)
        with self.assertRaisesRegex(ValueError, "component contract differs from source template"):
            inputs.verify(root)

    def test_missing_duplicate_or_wrong_platform_labels_reject(self):
        values = self.values()
        for case in (values[:2], values + values[:1], [values[0].replace("x86_64-linux=", "aarch64-linux="), *values[1:]]):
            with self.subTest(case=case), self.assertRaises(ValueError):
                inputs.validate_inputs(case)

    def test_source_version_toolchain_and_template_disagreement_reject(self):
        root = self.bundles["aarch64-darwin"]
        original = json.loads((root / "platform-input.json").read_text())
        for field, value in (("version", "0.7.2"), ("source", {**original["source"], "tree": "d" * 40}),
                             ("tools", {"rustc": "other", "cargo": "test-cargo"})):
            with self.subTest(field=field):
                self.write_json(root / "platform-input.json", {**original, field: value})
                with self.assertRaisesRegex(ValueError, "mismatch"):
                    inputs.validate_inputs(self.values())
        self.write_json(root / "platform-input.json", original)
        template = copy.deepcopy(self.template)
        template["note"] = "different source template"
        self.write_json(root / "components-template.json", template)
        self.refresh(root)
        with self.assertRaisesRegex(ValueError, "mismatch"):
            inputs.validate_inputs(self.values())

    def test_changed_missing_extra_and_symlink_files_reject(self):
        root = self.bundles["aarch64-darwin"]
        app = root / "artifacts/home.tar.gz"
        app.write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "differs from receipt"):
            inputs.verify(root)
        app.unlink()
        with self.assertRaisesRegex(ValueError, "inventory"):
            inputs.verify(root)
        app.symlink_to(root / "components.json")
        with self.assertRaisesRegex(ValueError, "symlink"):
            inputs.verify(root)
        app.unlink()
        app.write_bytes(self.app)
        (root / "artifacts/platform-input.json").write_text("extra")
        with self.assertRaisesRegex(ValueError, "inventory"):
            inputs.verify(root)

    def test_rehashed_wrong_architecture_or_nonexecutable_reject(self):
        root = self.bundles["aarch64-darwin"]
        path = root / "artifacts/elastos-aarch64-darwin"
        path.write_bytes(binary("aarch64-linux"))
        self.refresh(root)
        with self.assertRaisesRegex(ValueError, "expected executable"):
            inputs.verify(root)
        path.write_bytes(binary("aarch64-darwin"))
        path.chmod(0o644)
        self.refresh(root)
        with self.assertRaisesRegex(ValueError, "expected executable"):
            inputs.verify(root)

    def test_home_cli_requires_a_delivered_native_renderer(self):
        platform = "aarch64-darwin"
        root = self.bundles[platform]
        path = root / "artifacts/home-cli-darwin-arm64.tar.gz"
        template = json.loads((root / "components-template.json").read_text())
        template["external"]["home-cli"] = {
            "install_path": "capsules/home-cli", "platforms": {"darwin-arm64": {
                "install_path": "capsules/home-cli", "release_path": path.name,
                "extract_path": "home-cli"}}}
        self.write_json(root / "components-template.json", template)
        manifest = json.loads((root / "components.json").read_text())
        manifest["external"]["home-cli"] = copy.deepcopy(template["external"]["home-cli"])
        for case in ("valid", "missing", "wrong-os", "wrong-cpu", "nonexec", "symlink"):
            with tarfile.open(path, "w:gz") as archive:
                contract = tarfile.TarInfo("home-cli/capsule.json")
                payload = b'{"name":"home-cli"}'
                contract.size = len(payload)
                archive.addfile(contract, io.BytesIO(payload))
                if case != "missing":
                    info = tarfile.TarInfo("home-cli/bin/home-cli")
                    info.mode = 0o644 if case == "nonexec" else 0o755
                    payload = binary("aarch64-linux" if case == "wrong-os" else platform)
                    if case == "wrong-cpu":
                        payload = bytearray(payload)
                        struct.pack_into("<I", payload, 4, 0x01000007)
                    if case == "symlink":
                        info.type = tarfile.SYMTYPE
                        info.linkname = "../other"
                        archive.addfile(info)
                    else:
                        info.size = len(payload)
                        archive.addfile(info, io.BytesIO(payload))
            descriptor = manifest["external"]["home-cli"]["platforms"]["darwin-arm64"]
            descriptor.update(checksum="sha256:" + inputs.digest(path), size=path.stat().st_size)
            self.write_json(root / "components.json", manifest)
            self.refresh(root)
            with self.subTest(case=case):
                if case == "valid":
                    inputs.verify(root)
                else:
                    with self.assertRaisesRegex(ValueError, "renderer|expected executable"):
                        inputs.verify(root)

    def test_unsafe_archive_members_and_links_reject(self):
        path = self.root / "bad.tar.gz"
        for entries in (
            [("../escape", b"bad", None)], [("/absolute", b"bad", None)],
            [("home/a", b"a", None), ("home/a", b"b", None)],
            [("home/link", b"", "../../escape")],
            [("home/link", b"", "dir"), ("home/link/child", b"bad", None)],
            [("home/link/child", b"bad", None), ("home/link", b"", "dir")],
            [("home/bin", b"file", None), ("home/bin/renderer", b"child", None)],
            [("home/bin/renderer", b"child", None), ("home/bin", b"file", None)],
        ):
            with self.subTest(entries=entries):
                path.write_bytes(archive_bytes(entries))
                with self.assertRaises(ValueError):
                    inputs.check_archive(path)

    def test_supported_component_cannot_be_omitted(self):
        root = self.bundles["aarch64-darwin"]
        receipt = json.loads((root / "platform-input.json").read_text())
        receipt["omitted_platform_components"] = ["shell"]
        self.write_json(root / "platform-input.json", receipt)
        with self.assertRaisesRegex(ValueError, "not platform-absent"):
            inputs.verify(root)

    def test_provider_metadata_and_install_contract_are_required(self):
        root = self.bundles["aarch64-darwin"]
        original = json.loads((root / "components.json").read_text())
        changed = copy.deepcopy(original)
        del changed["external"]["shell"]["capsule_metadata"]
        self.write_json(root / "components.json", changed)
        self.refresh(root)
        with self.assertRaisesRegex(ValueError, "metadata is missing"):
            inputs.verify(root)
        changed = copy.deepcopy(original)
        changed["external"]["shell"]["platforms"]["darwin-arm64"]["install_path"] = "bin/other"
        self.write_json(root / "components.json", changed)
        self.refresh(root)
        with self.assertRaisesRegex(ValueError, "install path"):
            inputs.verify(root)

    def test_boolean_sizes_and_missing_tool_identity_reject(self):
        root = self.bundles["aarch64-darwin"]
        original = json.loads((root / "platform-input.json").read_text())
        changed = copy.deepcopy(original)
        changed["files"]["components.json"]["size"] = True
        self.write_json(root / "platform-input.json", changed)
        with self.assertRaisesRegex(ValueError, "invalid artifact receipt"):
            inputs.verify(root)
        self.write_json(root / "platform-input.json", {**original, "tools": {}})
        with self.assertRaisesRegex(ValueError, "build tool"):
            inputs.verify(root)

    def test_rehashed_universal_conflict_rejects(self):
        root = self.bundles["aarch64-darwin"]
        path = root / "artifacts/home.tar.gz"
        path.write_bytes(archive_bytes([("home/index.html", b"Different app", None)]))
        manifest = json.loads((root / "components.json").read_text())
        descriptor = manifest["external"]["home"]["platforms"]["*"]
        descriptor.update(size=path.stat().st_size, checksum="sha256:" + inputs.digest(path))
        self.write_json(root / "components.json", manifest)
        self.refresh(root)
        with self.assertRaisesRegex(ValueError, "conflicting universal asset"):
            inputs.validate_inputs(self.values())

    def test_source_local_assets_cannot_become_url_only(self):
        root = self.bundles["aarch64-darwin"]
        manifest = json.loads((root / "components.json").read_text())
        descriptor = manifest["external"]["home"]["platforms"]["*"]
        del descriptor["release_path"]
        descriptor["url"] = "https://example.invalid/home.tar.gz"
        self.write_json(root / "components.json", manifest)
        self.refresh(root)
        with self.assertRaisesRegex(ValueError, "needs a local artifact"):
            inputs.verify(root)

    def test_wrong_extract_root_and_missing_provider_contract_reject(self):
        path = self.root / "contract.tar.gz"
        path.write_bytes(self.app)
        with self.assertRaisesRegex(ValueError, "extraction root"):
            inputs.check_archive(path, "shell", provider=True)
        path.write_bytes(archive_bytes([("shell/index.html", b"Other content", None)]))
        with self.assertRaisesRegex(ValueError, "provider capsule contract"):
            inputs.check_archive(path, "shell", provider=True)
        path.write_bytes(archive_bytes([("shell/capsule.json", b'{"name":"shell","role":"provider","icon":"icon"}', None)]))
        with self.assertRaisesRegex(ValueError, "missing provider icon"):
            inputs.check_archive(path, "shell", provider=True)
        path.write_bytes(archive_bytes([("shell/capsule.json", b'{"name":"other","role":"provider","icon":"icon"}', None)]))
        with self.assertRaisesRegex(ValueError, "capsule name"):
            inputs.check_archive(path, "shell", provider=True)

    def test_unselected_platform_changes_reject(self):
        root = self.bundles["aarch64-darwin"]
        manifest = json.loads((root / "components.json").read_text())
        manifest["external"]["shell"]["platforms"]["linux-arm64"] = {"install_path": "other"}
        self.write_json(root / "components.json", manifest)
        self.refresh(root)
        with self.assertRaisesRegex(ValueError, "unselected platform"):
            inputs.verify(root)

    def test_matching_forged_source_claims_do_not_replace_candidate_bindings(self):
        for root in self.bundles.values():
            receipt = json.loads((root / "platform-input.json").read_text())
            receipt["source"]["lockfiles"]["elastos/Cargo.lock"] = "e" * 64
            self.write_json(root / "platform-input.json", receipt)
        with self.assertRaisesRegex(ValueError, "candidate checkout"):
            inputs.validate_inputs(self.values())


class SourceRecordTest(unittest.TestCase):
    def test_dangling_receipt_link_is_rejected_before_writing(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            stage = root / "stage"
            stage.mkdir()
            outside = root / "outside.json"
            (stage / "platform-input.json").symlink_to(outside)
            with self.assertRaisesRegex(ValueError, "new, regular staging"):
                inputs.record(SimpleNamespace(root=stage))
            self.assertFalse(outside.exists())
            self.assertTrue((stage / "platform-input.json").is_symlink())

    def test_record_binds_real_git_source_and_rejects_changes(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            source = root / "source"
            source.mkdir()
            template = {"schema": "elastos.components/v1", "capsules": {},
                        "profiles": {"home": {"components": []}}, "external": {}}
            (source / "components.json").write_text(json.dumps(template))
            (source / "Cargo.lock").write_text("version = 3\n")
            def git(*args):
                return subprocess.check_output(["git", *args], cwd=source, text=True).strip()
            git("init", "--quiet")
            git("add", ".")
            git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                "commit", "--quiet", "-m", "source fixture")
            commit, tree = git("rev-parse", "HEAD"), git("rev-parse", "HEAD^{tree}")
            stage = root / "stage"
            (stage / "artifacts").mkdir(parents=True)
            (stage / "components.json").write_text(json.dumps(template))
            runtime = stage / "artifacts/elastos-aarch64-darwin"
            runtime.write_bytes(binary("aarch64-darwin"))
            runtime.chmod(0o755)
            omissions = root / "omissions.json"
            omissions.write_text("[]")
            args = SimpleNamespace(root=stage, version="0.7.1", platform="aarch64-darwin",
                                   target="aarch64-apple-darwin", source_commit=commit,
                                   source_tree=tree, omissions_json=omissions)
            original_run = inputs.run
            def run(*args):
                return "fixture-tool" if args[0] in ("rustc", "cargo") else original_run(*args)
            with patch.object(inputs, "SOURCE_ROOT", source), patch.object(inputs, "run", side_effect=run):
                inputs.record(args)
                receipt = inputs.verify(stage)
                self.assertEqual(receipt["source"]["commit"], commit)
                self.assertEqual(receipt["source"]["tree"], tree)
                self.assertEqual(receipt["source"]["lockfiles"]["Cargo.lock"], inputs.digest(source / "Cargo.lock"))
                (source / "Cargo.lock").write_text("changed\n")
                with self.assertRaisesRegex(ValueError, "clean"):
                    inputs.source_identity(commit, tree)
                git("checkout", "--", "Cargo.lock")
                with self.assertRaisesRegex(ValueError, "identity changed"):
                    inputs.source_identity("e" * 40, tree)


if __name__ == "__main__":
    unittest.main()
