#!/usr/bin/env python3
"""Exercise unsigned input admission without builds, uploads or signing."""

import copy
import hashlib
import importlib.util
import io
import json
import os
import shutil
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
