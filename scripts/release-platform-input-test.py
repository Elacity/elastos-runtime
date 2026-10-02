#!/usr/bin/env python3
"""Exercise unsigned input admission without builds, uploads or signing."""

import base64
import copy
import hashlib
import importlib.util
import io
import json
import os
import platform
import shutil
from pathlib import Path
import struct
import subprocess
import sys
import tarfile
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch


spec = importlib.util.spec_from_file_location("platform_input", Path(__file__).with_name("release-platform-input.py"))
inputs = importlib.util.module_from_spec(spec)
spec.loader.exec_module(inputs)

signer_spec = importlib.util.spec_from_file_location("release_signer_fixture", Path(__file__).with_name("release-signer.py"))
signer = importlib.util.module_from_spec(signer_spec)
sys.modules[signer_spec.name] = signer
signer_spec.loader.exec_module(signer)


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

    def test_input_staging_and_cid_attachment_preserve_three_platforms(self):
        pinned = {"platforms": {"*": {"url": "https://example.invalid/kubo-v1.tar.gz",
                   "checksum": "sha256:" + "d" * 64, "size": 100, "install_path": "bin/ipfs"}}}
        self.template["external"]["kubo"] = pinned
        self.template["profiles"]["home"]["components"].append("kubo")
        self.write_json(inputs.SOURCE_ROOT / "components.json", self.template)
        for root in self.bundles.values():
            manifest = json.loads((root / "components.json").read_text())
            manifest["external"]["kubo"] = copy.deepcopy(pinned)
            manifest["profiles"] = copy.deepcopy(self.template["profiles"])
            self.write_json(root / "components.json", manifest)
            self.write_json(root / "components-template.json", self.template)
            self.refresh(root)
        stage = self.root / "publication"
        record = inputs.stage_inputs(self.values(), "0.7.1", stage)
        self.assertEqual(len(record["files"]), 8)
        inputs.verify_staged_inputs(stage)
        merged = json.loads((stage / "components.json").read_text())
        self.assertEqual(set(merged["external"]["shell"]["platforms"]),
                         {value[0] for value in inputs.PLATFORMS.values()})
        self.assertEqual(merged["capsules"], {})
        self.assertEqual(merged["profiles"], self.template["profiles"])
        cids = self.root / "cids.json"
        self.write_json(cids, {name: "bafy" + entry["sha256"] for name, entry in record["files"].items()})
        inputs.attach_input_cids(stage, cids)
        outputs = [(stage / "artifacts" / f"components-{platform}.json").read_bytes()
                   for platform in inputs.PLATFORMS]
        self.assertEqual(len(set(outputs)), 1)
        final = json.loads(outputs[0])
        self.assertEqual(final["external"]["kubo"], pinned)
        for setup, _, _ in inputs.PLATFORMS.values():
            descriptor = final["external"]["shell"]["platforms"][setup]
            self.assertTrue(descriptor["cid"].startswith("bafy"))
            self.assertEqual(descriptor["size"], 64)
        self.assertNotIn("cid", merged["external"]["home"]["platforms"]["*"])

    def test_input_staging_rejects_version_existing_output_and_changed_bytes(self):
        stage = self.root / "publication"
        with self.assertRaisesRegex(ValueError, "requested release"):
            inputs.stage_inputs(self.values(), "0.7.2", stage)
        self.assertFalse(stage.exists())
        stage.symlink_to(self.root / "absent")
        with self.assertRaisesRegex(ValueError, "already exists"):
            inputs.stage_inputs(self.values(), "0.7.1", stage)
        self.assertTrue(stage.is_symlink())
        stage.unlink()
        inputs.stage_inputs(self.values(), "0.7.1", stage)
        (stage / "artifacts/home.tar.gz").write_bytes(b"changed after admission")
        cids = self.root / "cids.json"
        self.write_json(cids, {})
        with self.assertRaisesRegex(ValueError, "staged input differs"):
            inputs.attach_input_cids(stage, cids)
        self.assertFalse(list((stage / "artifacts").glob("components-*.json")))

    def actual_source_fixture(self):
        source = self.root / "actual-source"
        (source / "scripts").mkdir(parents=True)
        (source / "elastos").mkdir()
        (source / ".gitignore").write_text("__pycache__/\n")
        (source / "elastos/Cargo.lock").write_text("version = 4\n")
        self.write_json(source / "components.json", self.template)
        for name in ("release-platform-input.py", "components-release-integrity-check.py",
                     "publish-release.sh", "check-versioning.sh", "install.sh"):
            shutil.copyfile(Path(__file__).with_name(name), source / "scripts" / name)
        for args in (("init", "-q"), ("add", "."),
                     ("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                      "commit", "-qm", "controlled publisher fixture")):
            subprocess.run(["git", *args], cwd=source, check=True, capture_output=True)
        commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=source, text=True).strip()
        tree = subprocess.check_output(["git", "rev-parse", "HEAD^{tree}"], cwd=source, text=True).strip()
        for root in self.bundles.values():
            receipt = json.loads((root / "platform-input.json").read_text())
            receipt["source"] = {"clean": True, "commit": commit, "tree": tree,
                                  "lockfiles": {"elastos/Cargo.lock": inputs.digest(source / "elastos/Cargo.lock")}}
            self.write_json(root / "platform-input.json", receipt)
        return source

    def test_actual_publisher_import_success_and_effect_boundaries(self):
        source = self.actual_source_fixture()
        publisher = (source / "scripts/publish-release.sh").read_text()
        payload = "RELEASE_PAYLOAD=" + publisher.split("\nRELEASE_PAYLOAD=", 1)[1].split(
            '\ninfo "Signing release payload', 1)[0]
        for failure in ("", "tamper", "upload"):
            with self.subTest(failure=failure):
                work = self.root / ("publisher-" + (failure or "success"))
                work.mkdir()
                body = r'''source "$1"
VERSION=0.7.1
TMPDIR="$2/work"
mkdir "$TMPDIR"
PLATFORM=aarch64-darwin
failure="$3"
shift 3
stage_platform_inputs "$TMPDIR/staged" "$@"
if [[ "$failure" == tamper ]]; then printf bad > "$TMPDIR/staged/artifacts/home.tar.gz"; fi
ipfs_add() {
    printf '%s\n' "$(basename "$1")" >> "$TMPDIR/uploads"
    [[ "$failure" != upload ]] || return 93
    printf 'bafy%s\n' "$(sha256 "$1")"
}
publish_prepared_platform_inputs "$TMPDIR/staged"
printf '%s\n' "$PLATFORMS_JSON" > "$TMPDIR/platforms.json"
printf '%s\n' "$RELEASE_SOURCE_JSON" > "$TMPDIR/source.json"
printf 'later signing boundary reached\n' > "$TMPDIR/later-effect"
'''
                body = body.replace("printf 'later signing boundary reached",
                    'CHANNEL=stable\nPREV_RELEASE_CID=null\n' + payload +
                    "\nprintf '%s\\n' \"$RELEASE_PAYLOAD\" > \"$TMPDIR/release-payload.json\"\n" +
                    "printf 'later signing boundary reached")
                result = subprocess.run(["/bin/bash", "-euc", body, "publisher-fixture",
                                         str(source / "scripts/publish-release.sh"), str(work), failure,
                                         *self.values()], capture_output=True, text=True)
                prepared = work / "work"
                if failure:
                    self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertFalse((prepared / "later-effect").exists())
                    if failure == "tamper":
                        self.assertFalse((prepared / "uploads").exists())
                    else:
                        self.assertEqual(len((prepared / "uploads").read_text().splitlines()), 1)
                else:
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    platforms = json.loads((prepared / "platforms.json").read_text())
                    self.assertEqual(set(platforms), set(inputs.PLATFORMS))
                    self.assertEqual(len((prepared / "uploads").read_text().splitlines()), 9)
                    self.assertEqual(len({json.dumps(p["components"], sort_keys=True) for p in platforms.values()}), 1)
                    for platform, descriptor in platforms.items():
                        self.assertEqual(descriptor["binary"]["sha256"], inputs.digest(
                            self.bundles[platform] / "artifacts" / f"elastos-{platform}"))
                    public_source = json.loads((prepared / "source.json").read_text())
                    self.assertEqual(set(public_source), {"commit", "tree"})
                    release = json.loads((prepared / "release-payload.json").read_text())
                    self.assertEqual(release["source"], public_source)
                    self.assertEqual(set(release["platforms"]), set(inputs.PLATFORMS))
                    self.assertNotIn("shell_cid", release)
                    self.assertNotIn("shell_sha256", release)
                    self.assertTrue((prepared / "later-effect").exists())

    def pin_catalogue(self):
        """Pin a signed catalogue snapshot in the template and every bundle, as the candidate source does."""
        catalog = b'{"payload":{"schema":"elastos.model.catalog/v1","entries":[]},"signature":"00","signer_did":"did:key:z6Mkfixture"}\n'
        pin = {"head_cid": inputs.catalog_head_cid(catalog), "publisher_dids": ["did:key:z6Mkfixture"]}
        self.template["model_catalog"] = pin
        self.write_json(inputs.SOURCE_ROOT / "components.json", self.template)
        for root in self.bundles.values():
            manifest = json.loads((root / "components.json").read_text())
            manifest["model_catalog"] = pin
            self.write_json(root / "components.json", manifest)
            self.write_json(root / "components-template.json", self.template)
            (root / "artifacts/model-catalog.json").write_bytes(catalog)
            self.refresh(root)
        return pin

    def test_default_admission_rejects_a_single_platform_input(self):
        darwin = [f"aarch64-darwin={self.bundles['aarch64-darwin']}"]
        stage = self.root / "publication"
        with self.assertRaisesRegex(ValueError, "all three platforms"):
            inputs.validate_inputs(darwin)
        with self.assertRaisesRegex(ValueError, "all three platforms"):
            inputs.stage_inputs(darwin, "0.7.1", stage)
        self.assertFalse(stage.exists())

    def test_stable_admission_accepts_darwin_and_x86_64_linux(self):
        pin = self.pin_catalogue()
        values = [f"aarch64-darwin={self.bundles['aarch64-darwin']}",
                  f"x86_64-linux={self.bundles['x86_64-linux']}"]
        self.assertEqual(set(inputs.validate_inputs(values, "0.7.1")),
                         {"aarch64-darwin", "x86_64-linux"})
        stage = self.root / "publication"
        record = inputs.stage_inputs(values, "0.7.1", stage)
        self.assertEqual(record["platforms"], ["aarch64-darwin", "x86_64-linux"])
        inputs.verify_staged_inputs(stage)
        merged = json.loads((stage / "components.json").read_text())
        self.assertEqual(merged["model_catalog"], pin)
        self.assertEqual(set(merged["external"]["shell"]["platforms"]),
                         {"darwin-arm64", "linux-amd64"})
        self.assertNotIn("linux-arm64", merged["external"]["shell"]["platforms"])
        cids = self.root / "cids.json"
        self.write_json(cids, {name: "bafy" + entry["sha256"] for name, entry in record["files"].items()})
        inputs.attach_input_cids(stage, cids)
        generated = sorted(path.name for path in (stage / "artifacts").glob("components-*.json"))
        self.assertEqual(generated, ["components-aarch64-darwin.json", "components-x86_64-linux.json"])
        final = json.loads((stage / "artifacts/components-x86_64-linux.json").read_bytes())
        self.assertEqual(set(final["external"]["shell"]["platforms"]),
                         {"darwin-arm64", "linux-amd64"})
        self.assertNotIn("linux-arm64", final["external"]["shell"]["platforms"])

    def test_mac_preview_admits_one_input_and_keeps_catalogue_and_selected_platform(self):
        pin = self.pin_catalogue()
        darwin = [f"aarch64-darwin={self.bundles['aarch64-darwin']}"]
        linux = [f"x86_64-linux={self.bundles['x86_64-linux']}"]
        self.assertEqual(set(inputs.validate_inputs(darwin, "0.7.1", "aarch64-darwin")), {"aarch64-darwin"})
        for values, preview, message in [
                (darwin, "x86_64-linux", "outside the selected publication"),
                (darwin + linux, "aarch64-darwin", "outside the selected publication"),
                ([], "aarch64-darwin", "exactly one aarch64-darwin input"),
                (darwin, "riscv64-linux", "not a release platform")]:
            with self.subTest(preview=preview, count=len(values)):
                with self.assertRaisesRegex(ValueError, message):
                    inputs.validate_inputs(values, "0.7.1", preview)
        stage = self.root / "publication"
        record = inputs.stage_inputs(darwin, "0.7.1", stage, "aarch64-darwin")
        self.assertEqual(record["platforms"], ["aarch64-darwin"])
        self.assertEqual(set(record["files"]), {"elastos-aarch64-darwin", "shell-darwin-arm64", "home.tar.gz",
                                                "shell-metadata.tar.gz", "model-catalog.json"})
        self.assertEqual(json.loads((stage / "assembly.json").read_text())["platforms"], ["aarch64-darwin"])
        inputs.verify_staged_inputs(stage, preview_platform="aarch64-darwin")
        with self.assertRaisesRegex(ValueError, "all three platforms"):
            inputs.verify_staged_inputs(stage)
        with self.assertRaisesRegex(ValueError, "not the x86_64-linux preview"):
            inputs.verify_staged_inputs(stage, preview_platform="x86_64-linux")
        merged = json.loads((stage / "components.json").read_text())
        self.assertEqual(merged["model_catalog"], pin)
        self.assertEqual(set(merged["external"]["shell"]["platforms"]), {"darwin-arm64"})
        cids = self.root / "cids.json"
        self.write_json(cids, {name: "bafy" + entry["sha256"] for name, entry in record["files"].items()})
        with self.assertRaisesRegex(ValueError, "all three platforms"):
            inputs.attach_input_cids(stage, cids)
        inputs.attach_input_cids(stage, cids, "aarch64-darwin")
        generated = sorted(path.name for path in (stage / "artifacts").glob("components-*.json"))
        self.assertEqual(generated, ["components-aarch64-darwin.json"])
        final = json.loads((stage / "artifacts/components-aarch64-darwin.json").read_text())
        self.assertEqual(final["model_catalog"], pin)
        self.assertTrue(final["external"]["shell"]["platforms"]["darwin-arm64"]["cid"].startswith("bafy"))
        self.assertEqual((stage / "artifacts/model-catalog.json").read_bytes(),
                         (self.bundles["aarch64-darwin"] / "artifacts/model-catalog.json").read_bytes())
        # A tampered input is refused at admission, before any staging output exists.
        binary_path = self.bundles["aarch64-darwin"] / "artifacts/elastos-aarch64-darwin"
        binary_path.write_bytes(binary_path.read_bytes()[:-1] + b"!")
        with self.assertRaisesRegex(ValueError, "differs from receipt"):
            inputs.stage_inputs(darwin, "0.7.1", self.root / "publication-2", "aarch64-darwin")
        self.assertFalse((self.root / "publication-2").exists())

    def test_actual_publisher_preview_publishes_only_the_selected_platform(self):
        pin = self.pin_catalogue()
        source = self.actual_source_fixture()
        publisher = (source / "scripts/publish-release.sh").read_text()
        payload = "RELEASE_PAYLOAD=" + publisher.split("\nRELEASE_PAYLOAD=", 1)[1].split(
            '\ninfo "Signing release payload', 1)[0]
        work = self.root / "publisher-preview"
        work.mkdir()
        body = r'''source "$1"
VERSION=0.7.1
CHANNEL=canary
TMPDIR="$2/work"
mkdir "$TMPDIR"
PLATFORM=aarch64-darwin
PREVIEW_PLATFORM=aarch64-darwin
PREVIEW_ARGS=(--preview-platform aarch64-darwin)
shift 2
stage_platform_inputs "$TMPDIR/staged" "$@"
ipfs_add() {
    printf '%s\n' "$(basename "$1")" >> "$TMPDIR/uploads"
    printf 'bafy%s\n' "$(sha256 "$1")"
}
publish_prepared_platform_inputs "$TMPDIR/staged"
printf '%s\n' "$PLATFORMS_JSON" > "$TMPDIR/platforms.json"
PREV_RELEASE_CID=null
''' + payload + r'''
printf '%s\n' "$RELEASE_PAYLOAD" > "$TMPDIR/release-payload.json"
'''
        result = subprocess.run(["/bin/bash", "-euc", body, "publisher-fixture",
                                 str(source / "scripts/publish-release.sh"), str(work),
                                 f"aarch64-darwin={self.bundles['aarch64-darwin']}"],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        prepared = work / "work"
        platforms = json.loads((prepared / "platforms.json").read_text())
        self.assertEqual(set(platforms), {"aarch64-darwin"})
        self.assertEqual(sorted((prepared / "uploads").read_text().splitlines()),
                         ["components-aarch64-darwin.json", "elastos-aarch64-darwin", "home.tar.gz",
                          "model-catalog.json", "shell-darwin-arm64", "shell-metadata.tar.gz"])
        release = json.loads((prepared / "release-payload.json").read_text())
        self.assertEqual((release["channel"], set(release["platforms"])), ("canary", {"aarch64-darwin"}))
        self.assertEqual(json.loads((prepared / "components.json").read_text())["model_catalog"], pin)
        self.assertEqual(platforms["aarch64-darwin"]["binary"]["sha256"],
                         inputs.digest(self.bundles["aarch64-darwin"] / "artifacts/elastos-aarch64-darwin"))

    def test_actual_publisher_preview_rejects_channel_input_count_and_host(self):
        source = self.actual_source_fixture()
        darwin = f"aarch64-darwin={os.path.relpath(self.bundles['aarch64-darwin'], self.root)}"
        linux = f"x86_64-linux={os.path.relpath(self.bundles['x86_64-linux'], self.root)}"
        base = ["/bin/bash", str(source / "scripts/publish-release.sh"), "--version", "0.7.1",
                "--key", str(self.root / "missing-key"), "--preview-platform", "aarch64-darwin"]
        state = self.root / "publisher-state"
        cases = [
            ("stable channel", ["--channel", "stable", "--platform-input", darwin], "requires --channel canary"),
            ("no input", ["--channel", "canary"], "exactly one --platform-input aarch64-darwin=DIR"),
            ("two inputs", ["--channel", "canary", "--platform-input", darwin, "--platform-input", linux],
             "exactly one --platform-input aarch64-darwin=DIR"),
            ("other platform input", ["--channel", "canary", "--platform-input", linux],
             "exactly one --platform-input aarch64-darwin=DIR"),
        ]
        if platform.system() != "Darwin":
            cases.append(("wrong host", ["--channel", "canary", "--platform-input", darwin],
                          "must be published from a aarch64-darwin host"))
        for name, options, message in cases:
            with self.subTest(case=name):
                result = subprocess.run(base + options, cwd=self.root,
                                        env={**os.environ, "ELASTOS_PUBLISH_STATE_DIR": str(state)},
                                        capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(message, result.stderr)
                self.assertFalse(state.exists())

    def test_actual_publisher_rejects_input_before_state_or_key_inspection(self):
        source = self.actual_source_fixture()
        state = self.root / "publisher-state"
        command = ["/bin/bash", str(source / "scripts/publish-release.sh"),
                   "--version", "0.7.1", "--key", str(self.root / "missing-key")]
        for platform, root in self.bundles.items():
            command.extend(["--platform-input", f"{platform}={os.path.relpath(root, self.root)}"])
        for failure in ("key", "conflict", "corrupt"):
            with self.subTest(failure=failure):
                if failure == "corrupt":
                    (self.bundles["aarch64-darwin"] / "artifacts/home.tar.gz").write_bytes(b"stale")
                result = subprocess.run(command + (["--skip-build"] if failure == "conflict" else []),
                                        cwd=self.root, env={**os.environ, "ELASTOS_PUBLISH_STATE_DIR": str(state)},
                                        capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0)
                expected = {"key": "Key file not found", "conflict": "conflicts with", "corrupt": "differs from receipt"}[failure]
                self.assertIn(expected, result.stderr)
                self.assertFalse(state.exists())

    def test_input_snapshot_and_copy_races_preserve_absent_output(self):
        stage = self.root / "publication"
        original_merge = inputs.merged_input_components
        changed = self.bundles["aarch64-darwin"] / "components.json"
        original_bytes = changed.read_bytes()
        def mutate_manifest(*args):
            data = json.loads(original_bytes)
            data["external"]["home"]["platforms"]["*"]["url"] = "https://changed.invalid/archive"
            self.write_json(changed, data)
            return original_merge(*args)
        with patch.object(inputs, "merged_input_components", side_effect=mutate_manifest):
            with self.assertRaisesRegex(ValueError, "manifest changed after admission"):
                inputs.stage_inputs(self.values(), "0.7.1", stage)
        self.assertFalse(stage.exists())
        changed.write_bytes(original_bytes)
        original_copy = inputs.shutil.copyfile
        def corrupt_copy(source, destination):
            original_copy(source, destination)
            destination.write_bytes(b"changed during copy")
        with patch.object(inputs.shutil, "copyfile", side_effect=corrupt_copy):
            with self.assertRaisesRegex(ValueError, "changed while staging"):
                inputs.stage_inputs(self.values(), "0.7.1", stage)
        self.assertFalse(stage.exists())
        self.assertFalse(list(self.root.glob(".platform-import-*")))

    def test_staged_inventory_and_generated_retry_are_exact(self):
        stage = self.root / "publication"
        record = inputs.stage_inputs(self.values(), "0.7.1", stage)
        extra = stage / "artifacts/unexpected"
        extra.write_text("unrelated bytes")
        with self.assertRaisesRegex(ValueError, "inventory differs"):
            inputs.verify_staged_inputs(stage)
        extra.unlink()
        cids = self.root / "cids.json"
        self.write_json(cids, {name: "bafy" + entry["sha256"] for name, entry in record["files"].items()})
        inputs.attach_input_cids(stage, cids)
        inputs.attach_input_cids(stage, cids)
        generated = stage / "artifacts/components-aarch64-darwin.json"
        generated.write_text("conflicting prior output")
        with self.assertRaisesRegex(ValueError, "existing generated components differ"):
            inputs.attach_input_cids(stage, cids)
        self.assertEqual(generated.read_text(), "conflicting prior output")

    def test_input_cid_attachment_requires_complete_upload_results(self):
        stage = self.root / "publication"
        record = inputs.stage_inputs(self.values(), "0.7.1", stage)
        cids = self.root / "cids.json"
        for values in ({}, {name: "" for name in record["files"]},
                       {name: "CID with space" for name in record["files"]}):
            self.write_json(cids, values)
            with self.assertRaisesRegex(ValueError, "every admitted artifact"):
                inputs.attach_input_cids(stage, cids)
            self.assertFalse(list((stage / "artifacts").glob("components-*.json")))

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
        for case in (values[:1], values + values[:1], [values[0].replace("x86_64-linux=", "aarch64-linux="), *values[1:]]):
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

    def test_media_tools_requires_native_pair_and_exact_source_bundle(self):
        platform = "aarch64-darwin"
        root = self.bundles[platform]
        scripts = inputs.SOURCE_ROOT / "scripts"
        scripts.mkdir()
        sources = {name: ("https://source.invalid/" + name, hashlib.sha256(payload).hexdigest())
                   for name, payload in (("ffmpeg.tar.xz", b"pinned ffmpeg"), ("x264.tar.bz2", b"pinned x264"))}
        recipe = ("SOURCES = " + repr(sources) + "\n").encode()
        wrapper = b"#!/bin/sh\nexec python3 media-tools-build.py \"$@\"\n"
        (scripts / "media-tools-build.py").write_bytes(recipe)
        (scripts / "build-media-tools.sh").write_bytes(wrapper)
        path = root / "artifacts/media-tools-darwin-arm64.tar.gz"
        template = json.loads((root / "components-template.json").read_text())
        component = {"install_path": "tools/media-tools", "platforms": {"darwin-arm64": {
            "install_path": "tools/media-tools", "release_path": path.name, "extract_path": "media-tools"}}}
        template["external"]["media-tools"] = component
        self.write_json(root / "components-template.json", template)
        manifest = json.loads((root / "components.json").read_text())
        manifest["external"]["media-tools"] = copy.deepcopy(component)
        for case in ("valid", "missing-pair", "wrong-cpu", "nonexec", "link", "source-mismatch",
                     "missing-license", "recipe-mismatch", "metadata-mismatch"):
            payloads = {"bin/ffmpeg": binary(platform), "bin/ffprobe": binary(platform),
                        "sources/ffmpeg.tar.xz": b"pinned ffmpeg", "sources/x264.tar.bz2": b"pinned x264",
                        "sources/media-tools-build.py": recipe, "sources/build-media-tools.sh": wrapper,
                        "licenses/FFmpeg-COPYING.GPLv2": b"GPL", "licenses/x264-COPYING": b"GPL",
                        "BUILD.md": b"Build from the included source"}
            if case == "missing-pair":
                del payloads["bin/ffprobe"]
            if case == "missing-license":
                del payloads["licenses/x264-COPYING"]
            if case == "source-mismatch":
                payloads["sources/ffmpeg.tar.xz"] = b"different source"
            if case == "recipe-mismatch":
                payloads["sources/media-tools-build.py"] = b"different recipe"
            if case == "wrong-cpu":
                payloads["bin/ffprobe"] = binary("x86_64-linux")
            info = {"schema": "elastos.media-tools-build/v1", "platform": "darwin-arm64",
                    "sources": {name: {"url": url, "sha256": sha} for name, (url, sha) in sources.items()},
                    "recipe_sha256": hashlib.sha256(recipe).hexdigest(),
                    "wrapper_sha256": hashlib.sha256(wrapper).hexdigest(), "compiler": "fixture compiler",
                    "files": {name: {"sha256": hashlib.sha256(payload).hexdigest(), "size": len(payload)}
                              for name, payload in payloads.items()}}
            if case == "metadata-mismatch":
                info["files"]["bin/ffmpeg"]["sha256"] = "0" * 64
            payloads["build-info.json"] = json.dumps(info).encode()
            with tarfile.open(path, "w:gz") as archive:
                for name, payload in payloads.items():
                    entry = tarfile.TarInfo("media-tools/" + name)
                    entry.mode = 0o755 if name.startswith("bin/") else 0o644
                    if case == "nonexec" and name == "bin/ffprobe":
                        entry.mode = 0o644
                    if case == "link" and name == "bin/ffprobe":
                        entry.type = tarfile.SYMTYPE
                        entry.linkname = "ffmpeg"
                        archive.addfile(entry)
                    else:
                        entry.size = len(payload)
                        archive.addfile(entry, io.BytesIO(payload))
            descriptor = manifest["external"]["media-tools"]["platforms"]["darwin-arm64"]
            descriptor.update(checksum="sha256:" + inputs.digest(path), size=path.stat().st_size)
            self.write_json(root / "components.json", manifest)
            self.refresh(root)
            with self.subTest(case=case):
                if case == "valid":
                    inputs.verify(root)
                else:
                    with self.assertRaises(ValueError):
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

    def test_arm64_engine_archive_requires_native_executable_and_libraries(self):
        path = self.root / "llama-arm64.tar.gz"

        def write_archive(server, library):
            with tarfile.open(path, "w:gz") as archive:
                for name, payload in (("llama-b10516/llama-server", server),
                                      ("llama-b10516/libllama.so", library)):
                    if payload is None:
                        continue
                    entry = tarfile.TarInfo(name)
                    entry.size = len(payload)
                    entry.mode = 0o755
                    archive.addfile(entry, io.BytesIO(payload))

        arm64 = binary("aarch64-linux")
        x86 = binary("x86_64-linux")
        write_archive(arm64, arm64)
        inputs.check_archive(path, "llama-b10516", engine_platform="aarch64-linux")
        for server, library in ((x86, arm64), (arm64, x86), (None, arm64), (arm64, None)):
            with self.subTest(server=server is not None, library=library == arm64):
                write_archive(server, library)
                with self.assertRaises(ValueError):
                    inputs.check_archive(path, "llama-b10516", engine_platform="aarch64-linux")
        path.write_bytes(archive_bytes([
            ("other/llama-server", arm64, None),
            ("llama-b10516/llama-server", arm64, None),
            ("llama-b10516/libllama.so", arm64, None),
        ]))
        with self.assertRaisesRegex(ValueError, "escapes its archive root"):
            inputs.check_archive(path, "llama-b10516", engine_platform="aarch64-linux")

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

    def signing_fixture(self):
        platform = "aarch64-darwin"
        stage = self.root / "signing-stage"
        record = inputs.stage_inputs([f"{platform}={self.bundles[platform]}"], "0.7.1", stage,
                                     preview_platform=platform)
        cids_path = self.root / "signing-cids.json"
        cids = {name: signer.raw_cid((stage / "artifacts" / name).read_bytes())
                for name in record["files"]}
        self.write_json(cids_path, cids)
        inputs.attach_input_cids(stage, cids_path, preview_platform=platform)
        components = f"components-{platform}.json"
        cids[components] = signer.raw_cid((stage / "artifacts" / components).read_bytes())
        self.write_json(cids_path, cids)
        # RFC 8032 public verification key only; no signing seed or backend.
        did = signer.public_did(bytes.fromhex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"))
        stamps = {"MAINTAINER_DID": did, "SOURCE_CONNECT_TICKET": "public-ticket",
                  "PUBLISHER_NODE_ID": "public-node", "PUBLISHER_GATEWAY": "https://staging.invalid",
                  "IPNS_NAME": ""}
        stamps_path = self.root / "signing-stamps.json"
        self.write_json(stamps_path, stamps)
        template = ("#!/bin/sh\n" + "\n".join(f'{name}="__{name}__"'
                    for name in sorted(signer.STAMPS | {"HEAD_CID"})) + "\n").encode()
        blob_oid = hashlib.sha1(b"blob " + str(len(template)).encode() + b"\0" + template).hexdigest()
        for context in (
                patch.object(inputs, "installer_source_blob", return_value=(blob_oid, template)),
                patch.object(inputs.shutil, "disk_usage", return_value=SimpleNamespace(total=100 * 1024**3, free=50 * 1024**3))):
            context.start()
            self.addCleanup(context.stop)
        return SimpleNamespace(stage=stage, cids=cids, cids_path=cids_path, stamps=stamps,
                               stamps_path=stamps_path, template=template, blob_oid=blob_oid,
                               output=self.root / "unsigned-input", platform=platform)

    def prepare_signing_fixture(self, fixture, **overrides):
        options = {"channel": "canary", "output": fixture.output, "preview_platform": fixture.platform}
        options.update(overrides)
        return inputs.signing_input(fixture.stage, fixture.cids_path, fixture.stamps_path, **options)

    def test_unsigned_mac_canary_handoff_binds_exact_files_installer_and_source(self):
        fixture = self.signing_fixture()
        manifest = self.prepare_signing_fixture(fixture)
        rendered = signer.render_installer(fixture.template, fixture.stamps, fixture.stamps["MAINTAINER_DID"])
        self.assertIn(b'HEAD_CID=""', rendered)
        self.assertEqual(manifest["source"], {"commit": "a" * 40, "tree": "b" * 40})
        self.assertEqual((manifest["version"], manifest["channel"]), ("0.7.1", "canary"))
        self.assertEqual(manifest["installer"], {"blob_oid": fixture.blob_oid, "stamps": fixture.stamps})
        self.assertEqual(manifest["release"]["source"], manifest["source"])
        self.assertEqual(manifest["release"]["installer_sha256"], signer.sha256(rendered))
        self.assertEqual(manifest["release"]["released_at"], manifest["head"]["updated_at"])
        self.assertEqual(set(manifest["release"]["platforms"]), {fixture.platform})
        self.assertIsNone(manifest["release"]["prev_release_cid"])
        self.assertIsNone(manifest["head"]["prev_head_cid"])
        self.assertEqual((fixture.output / "signing-input.json").read_bytes(), signer.json_bytes(manifest))
        self.assertEqual(set(path.name for path in fixture.output.iterdir()), set(fixture.cids) | {"signing-input.json"})
        for name, cid in fixture.cids.items():
            source = fixture.stage / "artifacts" / name
            copied = fixture.output / name
            self.assertEqual(copied.read_bytes(), source.read_bytes())
            record = inputs.file_record(source)
            self.assertEqual(manifest["files"][name], {"cid": cid, "sha256": record["sha256"], "size": record["size"]})
            self.assertEqual(copied.stat().st_mode & 0o777, 0o400)
        for kind, name in (("binary", f"elastos-{fixture.platform}"),
                           ("components", f"components-{fixture.platform}.json")):
            self.assertEqual(manifest["release"]["platforms"][fixture.platform][kind], manifest["files"][name])
        # Admit the exported data through the production signer, with public
        # source responses only. This stops before constructing any backend.
        commit, tree, scripts = "a" * 40, "b" * 40, "c" * 40
        prefix = f"/repos/{signer.REPOSITORY}"
        api = {
            f"{prefix}/git/ref/tags/v0.7.1": {"ref": "refs/tags/v0.7.1", "object": {"type": "commit", "sha": commit}},
            f"{prefix}/git/commits/{commit}": {"sha": commit, "tree": {"sha": tree}},
            f"{prefix}/git/ref/heads/main": {"ref": "refs/heads/main", "object": {"type": "commit", "sha": commit}},
            f"{prefix}/compare/{commit}...{commit}?per_page=1": {"status": "identical", "ahead_by": 0, "behind_by": 0,
                "base_commit": {"sha": commit}, "merge_base_commit": {"sha": commit}},
            f"{prefix}/git/trees/{tree}": {"sha": tree, "truncated": False, "tree": [{"path": "scripts", "type": "tree", "mode": "040000", "sha": scripts}]},
            f"{prefix}/git/trees/{scripts}": {"sha": scripts, "truncated": False, "tree": [{"path": "install.sh", "type": "blob", "mode": "100755", "sha": fixture.blob_oid}]},
            f"{prefix}/git/blobs/{fixture.blob_oid}": {"sha": fixture.blob_oid, "encoding": "base64", "size": len(fixture.template),
                "content": base64.b64encode(fixture.template).decode()},
        }
        policy = {"repository": signer.REPOSITORY, "tag": "v0.7.1", "tag_oid": commit, "commit": commit, "tree": tree,
                  "version": "0.7.1", "channel": "canary", "publisher_did": fixture.stamps["MAINTAINER_DID"],
                  "manifest_sha256": signer.sha256(signer.json_bytes(manifest)),
                  "max_file_bytes": 1024 * 1024, "max_snapshot_bytes": 8 * 1024 * 1024}
        snapshot = (self.root / "custodian-public-snapshot").resolve()
        snapshot.mkdir(mode=0o700)
        prepared = signer.prepare(policy, fixture.output.resolve(), "signing-input.json", api.__getitem__, snapshot)
        self.assertEqual(prepared.release, signer.json_bytes(manifest["release"]))
        self.assertEqual(dict(prepared.files)["install.sh"].read_bytes(), rendered)

    def test_unsigned_handoff_refuses_existing_and_symlink_output(self):
        fixture = self.signing_fixture()
        fixture.output.mkdir()
        marker = fixture.output / "keep"
        marker.write_bytes(b"existing public bytes")
        with self.assertRaisesRegex(ValueError, "already exists"):
            self.prepare_signing_fixture(fixture)
        self.assertEqual(marker.read_bytes(), b"existing public bytes")
        dangling = self.root / "output-link"
        target = self.root / "absent-output"
        dangling.symlink_to(target)
        with self.assertRaisesRegex(ValueError, "already exists"):
            self.prepare_signing_fixture(fixture, output=dangling)
        self.assertTrue(dangling.is_symlink())
        self.assertFalse(target.exists())

    def test_unsigned_handoff_refuses_changed_artifacts_and_missing_cid_map(self):
        fixture = self.signing_fixture()
        with self.assertRaises(FileNotFoundError):
            inputs.signing_input(fixture.stage, self.root / "missing-cids.json", fixture.stamps_path,
                                 "canary", fixture.output, fixture.platform)
        missing = dict(fixture.cids)
        del missing[f"elastos-{fixture.platform}"]
        self.write_json(fixture.cids_path, missing)
        with self.assertRaisesRegex(ValueError, "complete artifact set"):
            self.prepare_signing_fixture(fixture)
        self.write_json(fixture.cids_path, fixture.cids)
        (fixture.stage / "artifacts/home.tar.gz").write_bytes(b"changed public bytes")
        with self.assertRaisesRegex(ValueError, "differs from admitted bytes"):
            self.prepare_signing_fixture(fixture)
        self.assertFalse(fixture.output.exists())

    def test_unsigned_handoff_refuses_raw_cid_mismatch_and_unsafe_stamps(self):
        fixture = self.signing_fixture()
        changed = dict(fixture.cids)
        changed[f"elastos-{fixture.platform}"] = signer.raw_cid(b"different public bytes")
        self.write_json(fixture.cids_path, changed)
        with self.assertRaisesRegex(ValueError, "raw CID differs"):
            self.prepare_signing_fixture(fixture)
        self.write_json(fixture.cids_path, fixture.cids)
        for stamps in ({**fixture.stamps, "SOURCE_CONNECT_TICKET": "$(touch marker)"},
                       {**fixture.stamps, "PUBLISHER_GATEWAY": "http://staging.invalid"},
                       {**fixture.stamps, "MAINTAINER_DID": "did:key:invalid"},
                       {**fixture.stamps, "HEAD_CID": "circular-head"}):
            self.write_json(fixture.stamps_path, stamps)
            with self.subTest(stamps=stamps), self.assertRaises(ValueError):
                self.prepare_signing_fixture(fixture)
            self.assertFalse(fixture.output.exists())

    def test_unsigned_handoff_refuses_rehashed_generated_components(self):
        fixture = self.signing_fixture()
        admitted = (fixture.stage / "components.json").read_bytes()
        generated = fixture.stage / "artifacts" / f"components-{fixture.platform}.json"
        components = json.loads(generated.read_bytes())
        components["external"]["shell"]["platforms"]["darwin-arm64"]["install_path"] = "bin/unapproved"
        self.write_json(generated, components)
        changed = dict(fixture.cids)
        changed[generated.name] = signer.raw_cid(generated.read_bytes())
        self.write_json(fixture.cids_path, changed)
        with patch.object(inputs.shutil, "copyfile", side_effect=AssertionError("tampered components copied")):
            with self.assertRaisesRegex(ValueError, "generated components differ"):
                self.prepare_signing_fixture(fixture)
        self.assertFalse(fixture.output.exists())
        self.assertEqual((fixture.stage / "components.json").read_bytes(), admitted)

    def test_unsigned_stage_refuses_15_percent_floor_before_any_copy(self):
        output = self.root / "low-disk-stage"
        platform = "aarch64-darwin"
        with patch.object(inputs.shutil, "disk_usage", return_value=SimpleNamespace(total=100_000, free=14_000)), \
                patch.object(inputs.shutil, "copyfile", side_effect=AssertionError("low-disk stage copied")):
            with self.assertRaisesRegex(ValueError, "15%"):
                inputs.stage_inputs([f"{platform}={self.bundles[platform]}"], "0.7.1", output,
                                    preview_platform=platform)
        self.assertFalse(output.exists())
        self.assertFalse(list(self.root.glob(".platform-import-*")))

    def shell_signing_fixture(self):
        scratch = self.root / "shell-signing"
        scratch.mkdir()
        state = self.root / "shell-state"
        state.mkdir()
        previous = {"last-release-cid": signer.unixfs_metadata_cid(b"previous public release"),
                    "last-release-head-cid": signer.unixfs_metadata_cid(b"previous public head")}
        for name, cid in previous.items():
            (state / name).write_text(cid)
        cids = {"elastos-aarch64-darwin": signer.raw_cid(b"public binary"),
                "home.tar.gz": signer.raw_cid(b"public app archive")}
        self.write_json(scratch / "input-cids.json", cids)
        components_cid = signer.raw_cid(b"public generated components")
        platforms = self.root / "shell-platforms.json"
        self.write_json(platforms, {"aarch64-darwin": {
            "binary": {"cid": cids["elastos-aarch64-darwin"]}, "components": {"cid": components_cid}}})
        return SimpleNamespace(scratch=scratch, state=state, previous=previous, cids=cids,
                               components_cid=components_cid, platforms=platforms,
                               capture=self.root / "python-arguments", output=self.root / "unsigned-shell-output")

    def run_shell_signing_fixture(self, fixture, ticket="public-ticket", node="public-node"):
        body = r'''source "$1"
TMPDIR="$2"
STATE_DIR="$3"
PLATFORMS_JSON=$(cat "$4")
CAPTURE="$5"
CHANNEL=canary
PREVIEW_ARGS=(--preview-platform aarch64-darwin)
KEY_PATH="$6/key-must-stay-absent"
discover_source_bootstrap_json() { echo unexpected-bootstrap-discovery >&2; return 94; }
inspect_signer_did() { echo unexpected-key-inspection >&2; return 95; }
python3() { printf '%s\n' "$@" > "$CAPTURE"; }
prepare_release_signing_input "$6/native-inputs" "$7" "$8"
'''
        did = signer.public_did(bytes.fromhex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"))
        result = subprocess.run(["/bin/bash", "-euc", body, "public-signing-helper",
                                 str(Path(__file__).with_name("publish-release.sh")), str(fixture.scratch),
                                 str(fixture.state), str(fixture.platforms), str(fixture.capture),
                                 str(self.root), str(fixture.output), did], capture_output=True, text=True,
                                env={**os.environ, "ELASTOS_SOURCE_CONNECT_TICKET": ticket,
                                     "ELASTOS_PUBLISHER_NODE_ID": node,
                                     "ELASTOS_PUBLISHER_GATEWAY": "https://staging.invalid/",
                                     "ELASTOS_IPNS_NAME": "public-ipns"})
        return did, result

    def test_shell_unsigned_helper_passes_complete_public_inputs_without_signing(self):
        fixture = self.shell_signing_fixture()
        before = {path.name: path.read_bytes() for path in fixture.state.iterdir()}
        did, result = self.run_shell_signing_fixture(fixture)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(json.loads((fixture.scratch / "signing-cids.json").read_bytes()),
                         {**fixture.cids, "components-aarch64-darwin.json": fixture.components_cid})
        self.assertEqual(json.loads((fixture.scratch / "signing-stamps.json").read_bytes()),
                         {"MAINTAINER_DID": did, "SOURCE_CONNECT_TICKET": "public-ticket",
                          "PUBLISHER_NODE_ID": "public-node", "PUBLISHER_GATEWAY": "https://staging.invalid",
                          "IPNS_NAME": "public-ipns"})
        arguments = fixture.capture.read_text().splitlines()
        self.assertEqual(arguments, ["scripts/release-platform-input.py", "signing-input", str(self.root / "native-inputs"),
                         "--cids", str(fixture.scratch / "signing-cids.json"),
                         "--stamps", str(fixture.scratch / "signing-stamps.json"), "--channel", "canary",
                         "--output", str(fixture.output), "--preview-platform", "aarch64-darwin",
                         "--prev-release-cid", fixture.previous["last-release-cid"],
                         "--prev-head-cid", fixture.previous["last-release-head-cid"]])
        self.assertNotIn("--key", arguments)
        self.assertFalse((self.root / "key-must-stay-absent").exists())
        self.assertFalse(fixture.output.exists())
        self.assertEqual({path.name: path.read_bytes() for path in fixture.state.iterdir()}, before)

    def test_shell_unsigned_helper_refuses_unpaired_bootstrap_before_python(self):
        fixture = self.shell_signing_fixture()
        before = {path.name: path.read_bytes() for path in fixture.state.iterdir()}
        for ticket, node in (("public-ticket", ""), ("", "public-node")):
            with self.subTest(ticket=ticket, node=node):
                _, result = self.run_shell_signing_fixture(fixture, ticket, node)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("ticket and node from one publisher", result.stderr)
                self.assertNotIn("unexpected-bootstrap-discovery", result.stderr)
                self.assertFalse(fixture.capture.exists())
                self.assertFalse((fixture.scratch / "signing-stamps.json").exists())
                self.assertFalse(fixture.output.exists())
                self.assertEqual({path.name: path.read_bytes() for path in fixture.state.iterdir()}, before)

    def test_unsigned_handoff_refuses_changed_copy_and_removes_its_scratch(self):
        fixture = self.signing_fixture()
        original_copy = inputs.shutil.copyfile
        def changed_copy(source, destination):
            original_copy(source, destination)
            Path(destination).write_bytes(b"changed while copying public fixture")
        with patch.object(inputs.shutil, "copyfile", side_effect=changed_copy):
            with self.assertRaisesRegex(ValueError, "artifact changed while preparing"):
                self.prepare_signing_fixture(fixture)
        self.assertFalse(fixture.output.exists())
        self.assertFalse(list(self.root.glob(".signing-input-*")))

    def test_unsigned_handoff_refuses_low_disk_before_copy_and_stable_preview(self):
        fixture = self.signing_fixture()
        with self.assertRaisesRegex(ValueError, "preview requires canary"):
            self.prepare_signing_fixture(fixture, channel="stable")
        with patch.object(inputs.shutil, "disk_usage", return_value=SimpleNamespace(total=100_000, free=15_000)), \
                patch.object(inputs.shutil, "copyfile", side_effect=AssertionError("low-disk input copied")):
            with self.assertRaisesRegex(ValueError, "15 percent"):
                self.prepare_signing_fixture(fixture)
        self.assertFalse(fixture.output.exists())
        self.assertFalse(list(self.root.glob(".signing-input-*")))


class SourceRecordTest(unittest.TestCase):
    def test_installer_blob_uses_selected_commit_and_validates_inert_bytes(self):
        template = b"#!/bin/sh\n# public installer Git blob\n"
        blob = hashlib.sha1(b"blob " + str(len(template)).encode() + b"\0" + template).hexdigest()
        source = {"commit": "a" * 40, "tree": "b" * 40}
        with patch.object(inputs, "run", return_value=blob) as run, \
                patch.object(inputs.subprocess, "check_output", return_value=template) as read:
            self.assertEqual(inputs.installer_source_blob(source), (blob, template))
            run.assert_called_once_with("git", "rev-parse", source["commit"] + ":scripts/install.sh")
            read.assert_called_once_with(["git", "cat-file", "blob", blob], cwd=inputs.SOURCE_ROOT)
        for invalid in ("e" * 40, "malformed"):
            with self.subTest(blob=invalid), patch.object(inputs, "run", return_value=invalid), \
                    patch.object(inputs.subprocess, "check_output", return_value=template), \
                    self.assertRaisesRegex(ValueError, "source blob differs"):
                inputs.installer_source_blob(source)

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


class ReleaseSignerInputTest(unittest.TestCase):
    def test_custodian_refuses_hostile_inputs_without_running_candidates(self):
        # CI's existing release-input gate runs these public-data/fake-backend
        # cases with the same isolated interpreter flags as the signer.
        script = Path(__file__).with_name("release-signer-test.py").resolve()
        result = subprocess.run(
            [sys.executable, "-I", "-S", str(script)],
            cwd=script.parent.parent, capture_output=True, text=True, timeout=90,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
