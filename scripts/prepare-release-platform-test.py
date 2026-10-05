#!/usr/bin/env python3
"""Run the preparation worker with real packaging/receipts and fake native builds."""

import json
import base64
import hashlib
import io
import importlib.util
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch


SOURCE = Path(__file__).resolve().parent.parent
LINUX_ONLY = {"browser-engine-supervisor", "browser-native-proxy-engine", "browser-stream-bridge"}
APPS = [
    "home-cli", "home-gui", "home", "system", "wallet-metamask", "wallet-unisat",
    "wallet-walletconnect", "wallet", "browser", "documents", "library", "marketplace",
    "archive-manager", "inbox", "services", "people", "gba-emulator", "gba-ucity",
    "gba-nonogram", "chat-room", "assistant", "elacity-player",
]
MOCK_CARGO = r'''#!/usr/bin/env python3
import json, os, pathlib, struct, sys
args = sys.argv[1:]
with open(os.environ["MOCK_LOG"], "a") as log:
    log.write(json.dumps({"cwd": str(pathlib.Path.cwd()), "args": args,
                          "target_dir": os.environ.get("CARGO_TARGET_DIR")}) + "\n")
if args == ["--version"]:
    print("cargo fixture 1.91")
elif args[0] == "locate-project":
    print(pathlib.Path(args[args.index("--manifest-path") + 1]).resolve())
elif args[0] == "metadata":
    print(json.dumps({"target_directory": os.environ["MOCK_TARGET"]}))
else:
    assert args[0] == "build" and "--locked" in args, args
    if os.environ.get("FAIL_BUILD"):
        sys.exit(19)
    name = "elastos" if "--bin" in args else pathlib.Path.cwd().name
    cargo_target_dir = pathlib.Path(os.environ["CARGO_TARGET_DIR"])
    target = args[args.index("--target") + 1] if "--target" in args else ""
    output = cargo_target_dir / (f"{target}/release/{name}" if target else f"release/{name}")
    output.parent.mkdir(parents=True, exist_ok=True)
    header = bytearray(64)
    if target.endswith("linux-musl"):
        header[:7] = b"\x7fELF\x02\x01\x01"
        struct.pack_into("<HH", header, 16, 2, 62 if target.startswith("x86_64") else 183)
    else:
        header[:4] = b"\xcf\xfa\xed\xfe"
        struct.pack_into("<I", header, 4, 0x100000C)
        struct.pack_into("<I", header, 12, 2)
    output.write_bytes(bytes(header) + os.environ["ELASTOS_RELEASE_VERSION"].encode())
    output.chmod(0o755)
'''
MOCK_MEDIA_BUILD = r'''#!/usr/bin/env python3
import argparse, hashlib, json, os, pathlib, struct

SOURCES = {
    "ffmpeg-fixture.tar.xz": ("https://fixture.invalid/ffmpeg-fixture.tar.xz",
                              "0dd765da57f0dedab89eb7feefea3c51e65dac1fd40cbbdd15104f29c4d8ea42"),
    "x264-fixture.tar.bz2": ("https://fixture.invalid/x264-fixture.tar.bz2",
                            "c967c49130639e4fe5b560d3a38ea12855db6b7c41050d8ae21445c4d5afb031"),
}

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True, type=pathlib.Path)
    output = parser.parse_args().output
    host = (os.environ.get("MOCK_OS", "Darwin"), os.environ.get("MOCK_ARCH", "arm64"))
    platform = {("Darwin", "arm64"): "darwin-arm64", ("Linux", "x86_64"): "linux-amd64",
                ("Linux", "aarch64"): "linux-arm64"}[host]
    with open(os.environ["MOCK_MEDIA_LOG"], "a") as log:
        log.write(json.dumps({"output": str(output), "platform": platform}) + "\n")
    if os.environ.get("FAIL_MEDIA_BUILD"):
        raise SystemExit("fixture media build failure")
    for name in ("bin", "sources", "licenses"):
        (output / name).mkdir(parents=True, exist_ok=True)
    header = bytearray(64)
    if host[0] == "Linux":
        header[:7] = b"\x7fELF\x02\x01\x01"
        struct.pack_into("<HH", header, 16, 2, 62 if host[1] == "x86_64" else 183)
    else:
        header[:4] = b"\xcf\xfa\xed\xfe"
        struct.pack_into("<I", header, 4, 0x100000C)
        struct.pack_into("<I", header, 12, 2)
    for name in ("ffmpeg", "ffprobe"):
        binary = output / "bin" / name
        binary.write_bytes(bytes(header) + name.encode())
        binary.chmod(0o755)
    for name in SOURCES:
        (output / "sources" / name).write_bytes(f"synthetic {name.split('-')[0]} source archive\n".encode())
    recipe = pathlib.Path(__file__)
    wrapper = recipe.with_name("build-media-tools.sh")
    for source in (recipe, wrapper):
        (output / "sources" / source.name).write_bytes(source.read_bytes())
    (output / "BUILD.md").write_text("Synthetic media build for the actual-worker packaging fixture.\n")
    for name in ("FFmpeg-COPYING.GPLv2", "x264-COPYING"):
        (output / "licenses" / name).write_text("Synthetic license fixture.\n")
    info = {"schema": "elastos.media-tools-build/v1", "platform": platform,
            "sources": {name: {"url": url, "sha256": digest} for name, (url, digest) in SOURCES.items()},
            "recipe_sha256": sha(recipe), "wrapper_sha256": sha(wrapper), "compiler": "fixture compiler"}
    if host[0] == "Linux":
        license = output / "licenses/musl-COPYRIGHT"
        license.write_text("Synthetic musl license fixture.\n")
        info["musl"] = {"version": "fixture", "license_sha256": sha(license)}
    info["files"] = {str(p.relative_to(output)): {"sha256": sha(p), "size": p.stat().st_size}
                     for p in output.rglob("*") if p.is_file() and p.name != "build-info.json"}
    (output / "build-info.json").write_text(json.dumps(info))

if __name__ == "__main__":
    main()
'''


class PrepareWorkerTest(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix="release-worker-fixture-")
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name).resolve()
        self.repo = self.root / "source"
        scripts = self.repo / "scripts"
        scripts.mkdir(parents=True)
        for name in (
            "publish-release.sh", "prepare-release-platform.sh", "release-platform-input.py",
            "components-release-integrity-check.py", "check-versioning.sh", "build-media-tools.sh",
            "release-upstream-assets.py", "release-upstream-input.py",
        ):
            (scripts / name).write_bytes((SOURCE / "scripts" / name).read_bytes())
            (scripts / name).chmod(0o755)
        # The synthetic builder keeps source pins real to exercise archive admission.
        (scripts / "media-tools-build.py").write_text(MOCK_MEDIA_BUILD)
        # Fake binaries cannot establish static linking; only this audit is stubbed.
        (scripts / "audit-linux-runtime-portability.sh").write_text(
            '#!/bin/sh\necho "$*" >> "$MOCK_AUDIT_LOG"\n[ -z "${FAIL_AUDIT:-}" ]\n')
        (scripts / "audit-linux-runtime-portability.sh").chmod(0o755)
        publisher = (scripts / "publish-release.sh").read_text()
        self.native = list(dict.fromkeys(re.search(
            r"SUPPORT_BINARY_ASSETS=\((.*?)\n\)", publisher, re.S).group(1).split()))
        external = {}
        for name in self.native:
            capsule = self.repo / "capsules" / name
            capsule.mkdir(parents=True)
            (capsule / "Cargo.toml").write_text(f'[package]\nname="{name}"\nversion="0.1.0"\n')
            if name not in LINUX_ONLY:
                (capsule / "Cargo.lock").write_text("version = 4\n")
            platforms = {"linux-amd64": {"strategy": "source-build"},
                         "linux-arm64": {"strategy": "source-build"}}
            if name not in LINUX_ONLY:
                platforms["darwin-arm64"] = {"strategy": "source-build"}
            external[name] = {"install_path": f"bin/{name}", "platforms": platforms}
            if name == "shell":
                external[name]["provider_runtime"] = {}
                external[name]["capsule_metadata"] = {
                    "install_path": "capsules/shell", "platforms": {"*": self.stale_descriptor("metadata")}}
                (capsule / "capsule.json").write_text(json.dumps(
                    {"name": name, "role": "provider", "icon": "icons"}))
                (capsule / "icons").mkdir()
                for size in (32, 64, 128, 256):
                    (capsule / "icons" / f"icon-{size}.png").write_bytes(b"fixture icon")
        for name in APPS:
            capsule = self.repo / "capsules" / name
            (capsule / "browser").mkdir(parents=True)
            (capsule / "browser/index.html").write_text(f"tracked-{name}")
            (capsule / "capsule.json").write_text(json.dumps({
                "name": name, "type": "wasm", "runtime_abi": "elastos.runtime-projection/v1",
                "entrypoint": "browser/index.html"}))
            external[name] = {"install_path": f"capsules/{name}",
                              "platforms": {"*": self.stale_descriptor(name)}}
            if name == "home-cli":
                (capsule / "Cargo.toml").write_text('[package]\nname="home-cli"\nversion="0.1.0"\n')
                (capsule / "Cargo.lock").write_text("version = 4\n")
                external[name]["platforms"] = {
                    platform: self.stale_descriptor(f"{name}-{platform}")
                    for platform in ("linux-amd64", "linux-arm64", "darwin-arm64")
                }
        external["media-tools"] = {
            "install_path": "tools/media-tools",
            "platforms": {platform: {**self.stale_descriptor(f"media-tools-{platform}"),
                                      "extract_path": "media-tools"}
                          for platform in ("linux-amd64", "linux-arm64", "darwin-arm64")}}
        self.arm64_engine = self.root / "llama-b10516-bin-ubuntu22.04-arm64-cpu.tar.gz"
        header = bytearray(64)
        header[:7] = b"\x7fELF\x02\x01\x01"
        header[16:20] = (2).to_bytes(2, "little") + (183).to_bytes(2, "little")
        with tarfile.open(self.arm64_engine, "w:gz") as archive:
            for name in ("llama-server", "libllama.so"):
                member = tarfile.TarInfo(f"llama-b10516/{name}")
                member.mode = 0o755
                member.size = len(header)
                archive.addfile(member, io.BytesIO(header))
        external["llama-server"] = {"version": "b10516", "platforms": {"linux-arm64": {
            "release_path": self.arm64_engine.name,
            "checksum": "sha256:" + hashlib.sha256(self.arm64_engine.read_bytes()).hexdigest(),
            "size": self.arm64_engine.stat().st_size,
            "extract_path": "llama-b10516",
            "install_path": "libexec/llama.cpp/b10516/linux-arm64",
            "binary_path": "llama-server"}}}
        # Run the real upstream packaging helper with only tiny local pinned inputs.
        license_path = scripts / "fixture-license.txt"
        license_path.write_bytes(b"Synthetic upstream fixture licence\n")
        recipe = {"schema": "elastos.release-upstream-input/v1", "component": "llama-server",
                  "platform": "linux-arm64", "version": "0.0.10516", "format": "tar.gz",
                  "root": "llama-b10516", "entrypoint": "llama-server", "extract_path": "llama-b10516",
                  "install_path": "libexec/llama.cpp/b10516/linux-arm64", "binary_path": "llama-server",
                  "max_unpacked_bytes": 4096,
                  "source": {"path": "@llama-arm64-bundle", "checksum": "sha256:" + hashlib.sha256(self.arm64_engine.read_bytes()).hexdigest(),
                             "max_bytes": self.arm64_engine.stat().st_size},
                  "license": {"spdx_id": "MIT", "files": [{"name": "LICENSE", "source": {
                      "path": "scripts/fixture-license.txt", "checksum": "sha256:" + hashlib.sha256(license_path.read_bytes()).hexdigest(),
                      "max_bytes": license_path.stat().st_size}}]}}
        (scripts / "release-upstream-recipes.json").write_text(json.dumps({
            "schema": "elastos.release-upstream-recipes/v1", "recipes": [recipe]}))
        external["llama-server"]["platforms"]["linux-arm64"].update(
            strategy="source-build", cid="bafy-stale-fixture", url="https://example.invalid/stale-fixture")
        (self.repo / "components.json").write_text(json.dumps({
            "schema": "elastos.components/v1", "external": external,
            "profiles": {"home": {"components": ["home", "shell", "media-tools", "media-provider"]}}}))
        (self.repo / "elastos").mkdir()
        (self.repo / "elastos/Cargo.toml").write_text("[workspace]\n")
        (self.repo / "elastos/Cargo.lock").write_text("version = 4\n")
        (self.repo / ".gitignore").write_text("__pycache__/\nsecret.txt\n")
        mock = self.root / "mock"
        mock.mkdir()
        (mock / "cargo").write_text(MOCK_CARGO)
        (mock / "rustup").write_text(
            '#!/bin/sh\nprintf "aarch64-apple-darwin\\nx86_64-unknown-linux-musl\\naarch64-unknown-linux-musl\\n"\n')
        (mock / "rustc").write_text(
            '#!/bin/sh\n'
            'if [ "$1" = "-vV" ]; then\n'
            '  printf "release: fixture\\nhost: aarch64-apple-darwin\\n"\n'
            '  exit 0\n'
            'fi\n'
            'printf "rustc fixture 1.91\\n"\n')
        (mock / "uname").write_text(
            '#!/bin/sh\nif [ "$1" = -s ]; then echo "${MOCK_OS:-Darwin}"; '
            'else echo "${MOCK_ARCH:-arm64}"; fi\n')
        for path in mock.iterdir():
            path.chmod(0o755)
        self.env = {**os.environ, "PATH": str(mock) + os.pathsep + os.environ["PATH"],
                    "CARGO_TARGET_DIR": str(self.root / "native-cache"), "CARGO_BUILD_JOBS": "4",
                    "MOCK_TARGET": str(self.root / "resolved-cache"),
                    "MOCK_LOG": str(self.root / "cargo.log"),
                    "MOCK_MEDIA_LOG": str(self.root / "media.log"),
                    "MOCK_AUDIT_LOG": str(self.root / "audit.log"),
                    "ELASTOS_LLAMA_ARM64_BUNDLE": str(self.arm64_engine)}
        self.commit("fixture", init=True)
        (self.repo / "capsules/home/browser/secret.txt").write_text("ignored private input")

    @staticmethod
    def stale_descriptor(name):
        return {"cid": "old-cid", "url": "https://old.invalid/" + name,
                "release_path": "old-" + name + ".tar.gz", "checksum": "sha256:" + "0" * 64, "size": 0}

    def command(self, *args, env=None):
        return subprocess.run(args, cwd=self.repo, text=True, capture_output=True, env=env or self.env)

    def commit(self, message, init=False):
        commands = []
        if init:
            commands.extend([("git", "init", "-b", "fixture"), ("git", "config", "user.name", "Fixture"),
                             ("git", "config", "user.email", "fixture@invalid"),
                             ("git", "config", "commit.gpgsign", "false"),
                             ("git", "config", "gc.auto", "0"),
                             ("git", "config", "maintenance.auto", "false"),
                             ("git", "config", "gc.autoDetach", "false")])
        commands.extend([("git", "add", "."), ("git", "commit", "-m", message)])
        for command in commands:
            result = self.command(*command)
            self.assertEqual(result.returncode, 0, result.stderr)

    def prepare(self, name="prepared", env=None, version="0.7.1", reuse_support=None):
        output = self.root / name
        reuse = ["--reuse-support", str(reuse_support)] if reuse_support is not None else []
        result = self.command("/bin/bash", "scripts/prepare-release-platform.sh",
                              "--version", version, "--output", str(output), *reuse, env=env)
        self.assertEqual(list(self.root.glob(".release-platform.*")), [], "temporary sibling leaked")
        return output, result

    def qualified_support_input(self, name="m1", env=None):
        catalog = b'{"payload":{"schema":"elastos.model.catalog/v1","entries":[]}}\n'
        (self.repo / "model-catalog.json").write_bytes(catalog)
        template_path = self.repo / "components.json"
        template = json.loads(template_path.read_text())
        template["model_catalog"] = {
            "head_cid": "b" + base64.b32encode(b"\x01\x55\x12\x20" + hashlib.sha256(catalog).digest()).decode().lower().rstrip("="),
            "publisher_dids": ["did:key:z6MkgwHd2BCWe1jHMXPiR6H1q1RFPcv1YzhMbK5G1kBarbfe"],
        }
        template_path.write_text(json.dumps(template))
        self.commit("public catalogue fixture")
        output, result = self.prepare(name, env=env)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return output

    def clear_build_logs(self):
        for name in ("cargo.log", "media.log", "audit.log"):
            (self.root / name).unlink(missing_ok=True)

    def runtime_version_change(self):
        (self.repo / "elastos/runtime-fixture.txt").write_text("M2 Runtime source and embedded version")
        self.commit("M2 Runtime fixture")

    def test_reuse_support_changes_only_runtime_and_preserves_original_receipt(self):
        m1 = self.qualified_support_input()
        receipt_bytes = (m1 / "platform-input.json").read_bytes()
        original = json.loads(receipt_bytes)
        self.runtime_version_change()
        self.clear_build_logs()
        relative_input = os.path.relpath(m1, self.repo)
        m2, result = self.prepare("m2", version="0.7.2", reuse_support=relative_input)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        current = json.loads((m2 / "platform-input.json").read_text())
        self.assertEqual(current["version"], "0.7.2")
        self.assertNotEqual(current["source"]["commit"], original["source"]["commit"])
        self.assertEqual(current["source"]["commit"], self.command("git", "rev-parse", "HEAD").stdout.strip())
        self.assertEqual((m2 / "support-input.json").read_bytes(), receipt_bytes)
        self.assertEqual(current["support_origin"], {
            "receipt_path": "support-input.json", "sha256": hashlib.sha256(receipt_bytes).hexdigest()})
        self.assertEqual(current["files"]["support-input.json"]["sha256"], hashlib.sha256(receipt_bytes).hexdigest())
        self.assertEqual(current["files"]["support-input.json"]["size"], len(receipt_bytes))
        runtime = "artifacts/elastos-aarch64-darwin"
        self.assertNotEqual((m1 / runtime).read_bytes(), (m2 / runtime).read_bytes())
        self.assertTrue((m2 / runtime).read_bytes().endswith(b"0.7.2"))
        for name, record in original["files"].items():
            if name == runtime:
                continue
            self.assertEqual((m2 / name).read_bytes(), (m1 / name).read_bytes(), name)
            self.assertEqual(current["files"][name], record, name)
        self.assertEqual(current["omitted_platform_components"], original["omitted_platform_components"])
        self.assertEqual(current["build_command"][-2:], ["--reuse-support", "<input>"])
        calls = [json.loads(line) for line in (self.root / "cargo.log").read_text().splitlines()]
        builds = [entry for entry in calls if entry["args"][0] == "build"]
        self.assertEqual(len(builds), 1)
        self.assertIn("elastos-server", builds[0]["args"])
        self.assertEqual(builds[0]["args"][-2:], ["--bin", "elastos"])
        self.assertFalse((self.root / "media.log").exists())
        verified = self.command("python3", "scripts/release-platform-input.py", "verify", str(m2))
        self.assertEqual(verified.returncode, 0, verified.stderr)

    def test_reuse_support_refuses_before_cargo_and_cleans_owned_stage(self):
        m1 = self.qualified_support_input()
        original_template = (self.repo / "components.json").read_bytes()
        self.runtime_version_change()
        for concern in ("template", "platform", "artifact", "same-version"):
            with self.subTest(concern=concern):
                candidate = self.root / ("m1-" + concern)
                shutil.copytree(m1, candidate)
                version = "0.7.2"
                if concern == "template":
                    changed = json.loads(original_template)
                    changed["profiles"]["home"]["components"].append("another-component")
                    (self.repo / "components.json").write_text(json.dumps(changed))
                    self.commit("incompatible support template fixture")
                elif concern == "platform":
                    path = candidate / "platform-input.json"
                    receipt = json.loads(path.read_text())
                    receipt["platform"] = "x86_64-linux"
                    path.write_text(json.dumps(receipt))
                elif concern == "artifact":
                    (candidate / "artifacts/home.tar.gz").write_bytes(b"tampered public support")
                else:
                    version = "0.7.1"
                self.clear_build_logs()
                output, result = self.prepare("refused-" + concern, version=version, reuse_support=candidate)
                self.assertNotEqual(result.returncode, 0, result.stdout)
                self.assertFalse(output.exists())
                self.assertFalse((self.root / "cargo.log").exists(), result.stdout + result.stderr)
                self.assertFalse((self.root / "media.log").exists())
                self.assertFalse((self.root / "audit.log").exists())
                if concern == "template":
                    (self.repo / "components.json").write_bytes(original_template)
                    self.commit("restore exact support template fixture")

    def test_arm64_reuse_skips_external_bundle_and_support_lock_prerequisites(self):
        for name in LINUX_ONLY:
            (self.repo / "capsules" / name / "Cargo.lock").write_text("version = 4\n")
        self.commit("reviewed Linux fixture locks")
        env = {**self.env, "MOCK_OS": "Linux", "MOCK_ARCH": "aarch64"}
        m1 = self.qualified_support_input(env=env)
        for name in ["home-cli", *self.native]:
            (self.repo / "capsules" / name / "Cargo.lock").unlink(missing_ok=True)
        self.runtime_version_change()
        self.clear_build_logs()
        env = {key: value for key, value in env.items() if key != "ELASTOS_LLAMA_ARM64_BUNDLE"}
        m2, result = self.prepare("arm-m2", env=env, version="0.7.2", reuse_support=m1)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        for name in ("components.json", "components-template.json", "artifacts/model-catalog.json",
                     "artifacts/llama-server-linux-arm64.tar.gz"):
            self.assertEqual((m2 / name).read_bytes(), (m1 / name).read_bytes(), name)
        calls = [json.loads(line) for line in (self.root / "cargo.log").read_text().splitlines()]
        builds = [entry for entry in calls if entry["args"][0] == "build"]
        self.assertEqual(len(builds), 1)
        locations = [entry["args"][-1] for entry in calls if entry["args"][0] == "locate-project"]
        self.assertEqual(locations, ["elastos/Cargo.toml"])
        self.assertFalse((self.root / "media.log").exists())
        self.assertIn("--platform aarch64-linux", (self.root / "audit.log").read_text())

    def test_reuse_support_build_failure_preserves_m1_and_cleans_owned_stage(self):
        m1 = self.qualified_support_input()
        original = {str(path.relative_to(m1)): path.read_bytes() for path in m1.rglob("*") if path.is_file()}
        self.runtime_version_change()
        output, result = self.prepare("failed-m2", version="0.7.2", reuse_support=m1,
                                      env={**self.env, "FAIL_BUILD": "1"})
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(output.exists())
        self.assertEqual({str(path.relative_to(m1)): path.read_bytes() for path in m1.rglob("*") if path.is_file()}, original)

    def test_reuse_support_keeps_current_runtime_lock_prerequisite(self):
        m1 = self.qualified_support_input()
        (self.repo / "elastos/Cargo.lock").unlink()
        self.runtime_version_change()
        self.clear_build_logs()
        output, result = self.prepare("runtime-lock-refused", version="0.7.2", reuse_support=m1)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("elastos/Cargo.lock (elastos)", result.stderr)
        self.assertFalse(output.exists())
        calls = [json.loads(line)["args"][0] for line in (self.root / "cargo.log").read_text().splitlines()]
        self.assertNotIn("build", calls)
        self.assertFalse((self.root / "media.log").exists())

    def assert_media_archive(self, output, platform, setup_platform):
        manifest = json.loads((output / "components.json").read_text())
        descriptor = manifest["external"]["media-tools"]["platforms"][setup_platform]
        self.assertEqual(descriptor["install_path"], "tools/media-tools")
        self.assertEqual(descriptor["extract_path"], "media-tools")
        self.assertEqual(descriptor["release_path"], f"media-tools-{setup_platform}.tar.gz")
        archive_path = output / "artifacts" / descriptor["release_path"]
        with tarfile.open(archive_path) as archive:
            info = json.load(archive.extractfile("media-tools/build-info.json"))
            self.assertEqual(info["platform"], setup_platform)
            for name in ("ffmpeg", "ffprobe"):
                member = archive.getmember(f"media-tools/bin/{name}")
                self.assertTrue(member.isfile())
                self.assertEqual(member.mode & 0o111, 0o111)
            for name in ("media-tools-build.py", "build-media-tools.sh"):
                self.assertEqual(archive.extractfile(f"media-tools/sources/{name}").read(),
                                 (self.repo / "scripts" / name).read_bytes())
        result = self.command("python3", "-c",
                              "import runpy, sys; helper = runpy.run_path('scripts/release-platform-input.py'); "
                              "helper['check_archive'](helper['Path'](sys.argv[1]), 'media-tools', "
                              "media_platform=sys.argv[2])", str(archive_path), platform)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_arm64_rejects_unpinned_engine_before_build(self):
        wrong = self.root / "wrong-llama.tar.gz"
        wrong.write_bytes(b"different archive")
        env = {**self.env, "MOCK_OS": "Linux", "MOCK_ARCH": "aarch64",
               "ELASTOS_LLAMA_ARM64_BUNDLE": str(wrong)}
        output, result = self.prepare(env=env)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("checksum differs", result.stderr)
        self.assertFalse(output.exists())
        self.assertFalse((self.root / "cargo.log").exists())
        content = bytearray(self.arm64_engine.read_bytes())
        content[-1] ^= 1
        wrong.write_bytes(content)
        output, result = self.prepare("prepared-checksum", env=env)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("checksum differs", result.stderr)
        self.assertFalse(output.exists())
        self.assertFalse((self.root / "cargo.log").exists())

    def test_success_uses_real_packaging_and_receipt(self):
        output, result = self.prepare()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        receipt = json.loads((output / "platform-input.json").read_text())
        self.assertEqual(receipt["platform"], "aarch64-darwin")
        self.assertEqual(set(receipt["omitted_platform_components"]), LINUX_ONLY)
        manifest = json.loads((output / "components.json").read_text())
        self.assertEqual(manifest["capsules"], {})
        for info in (manifest["external"]["home"]["platforms"]["*"],
                     manifest["external"]["shell"]["capsule_metadata"]["platforms"]["*"]):
            self.assertNotIn("cid", info)
            self.assertNotIn("url", info)
        with tarfile.open(output / "artifacts/home.tar.gz") as archive:
            self.assertFalse(any("secret" in name for name in archive.getnames()))
            self.assertEqual(archive.extractfile("home/browser/index.html").read(), b"tracked-home")
        self.assertFalse((output / "artifacts/home-cli.tar.gz").exists())
        with tarfile.open(output / "artifacts/home-cli-darwin-arm64.tar.gz") as archive:
            renderer = archive.getmember("home-cli/bin/home-cli")
            self.assertTrue(renderer.isfile())
            self.assertEqual(renderer.mode & 0o111, 0o111)
            self.assertEqual(archive.extractfile(renderer).read()[:4], b"\xcf\xfa\xed\xfe")
        self.assert_media_archive(output, "aarch64-darwin", "darwin-arm64")
        self.assertEqual(json.loads((self.root / "media.log").read_text()), {
            "output": str(Path(self.env["CARGO_TARGET_DIR"]) / "media-tools/darwin-arm64"),
            "platform": "darwin-arm64"})
        result = self.command("python3", "scripts/release-platform-input.py", "verify", str(output))
        self.assertEqual(result.returncode, 0, result.stderr)
        commands = [json.loads(line) for line in (self.root / "cargo.log").read_text().splitlines()]
        builds = [entry for entry in commands if entry["args"][0] == "build"]
        self.assertEqual(len(builds), 2 + len(self.native) - len(LINUX_ONLY))
        for build in builds:
            self.assertIn("--locked", build["args"])
            self.assertNotIn("--target", build["args"])
            self.assertEqual(build["target_dir"], self.env["CARGO_TARGET_DIR"])
        before = (output / "platform-input.json").read_bytes()
        _, refused = self.prepare()
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn("Output already exists", refused.stderr)
        self.assertEqual((output / "platform-input.json").read_bytes(), before)

    def test_build_failure_removes_stage_and_preserves_missing_output(self):
        output, result = self.prepare(env={**self.env, "FAIL_BUILD": "1"})
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(output.exists())

    def test_symlink_parent_prepares_verified_input_at_canonical_path(self):
        parent = self.root / "canonical-output"
        parent.mkdir()
        alias = self.root / "output-alias"
        alias.symlink_to(parent, target_is_directory=True)
        output, result = self.prepare("output-alias/prepared")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        canonical = parent / "prepared"
        self.assertEqual(output.resolve(), canonical)
        verified = self.command("python3", "scripts/release-platform-input.py", "verify", str(canonical))
        self.assertEqual(verified.returncode, 0, verified.stdout + verified.stderr)
        self.assertEqual(list(parent.glob(".release-platform.*")), [])

    def test_symlink_parent_preserves_final_entry_and_checks_before_staging(self):
        parent = self.root / "canonical-output"
        parent.mkdir()
        (self.root / "output-alias").symlink_to(parent, target_is_directory=True)
        existing = parent / "existing"
        existing.mkdir()
        (existing / "owned.txt").write_text("preserve")
        (parent / "dangling").symlink_to(parent / "absent-target")
        for name in ("existing", "dangling"):
            with self.subTest(name=name):
                _, result = self.prepare("output-alias/" + name)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Output already exists", result.stderr)
                self.assertFalse((self.root / "cargo.log").exists())
                self.assertEqual(list(parent.glob(".release-platform.*")), [])
        self.assertEqual((existing / "owned.txt").read_text(), "preserve")
        self.assertTrue((parent / "dangling").is_symlink())
        (self.repo / "components.json").write_text("dirty")
        _, result = self.prepare("output-alias/new-parent/prepared")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Source checkout must be clean", result.stderr)
        self.assertFalse((parent / "new-parent").exists())
        self.assertFalse((self.root / "cargo.log").exists())

    def test_media_build_failure_removes_stage_and_preserves_missing_output(self):
        output, result = self.prepare(env={**self.env, "FAIL_MEDIA_BUILD": "1"})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("fixture media build failure", result.stderr)
        self.assertFalse(output.exists())

    def test_source_home_installs_renderer_after_copying_capsule_tree(self):
        source = (SOURCE / "scripts/setup-source-home.sh").read_text()
        names = ("cargo_target_root_for_manifest", "cargo_built_binary_path", "capsule_entrypoint",
                 "capsule_runtime_abi", "is_runtime_projection_capsule", "is_content_data_capsule",
                 "copy_capsule_tree", "install_app_capsules")
        functions = "\n".join(name + "() {" + source.split(name + "() {", 1)[1]
                              .split("\n}\n", 1)[0] + "\n}\n" for name in names)
        built = self.root / "built/release/home-cli"
        built.parent.mkdir(parents=True)
        built.write_bytes(b"rebuilt renderer")
        built.chmod(0o755)
        stale = self.repo / "capsules/home-cli/bin/home-cli"
        stale.parent.mkdir()
        stale.write_bytes(b"stale ignored source renderer")
        data = self.root / "source-home-data"
        env = {**self.env, "ROOT": str(self.repo), "DATA_DIR": str(data),
               "CARGO_TARGET_DIR": str(self.root / "built")}
        result = self.command("/bin/bash", "-euc", functions +
                              "\nAPP_CAPSULES=(home-cli)\ninstall_app_capsules\n", env=env)
        self.assertEqual(result.returncode, 0, result.stderr)
        installed = data / "capsules/home-cli/bin/home-cli"
        self.assertEqual(installed.read_bytes(), built.read_bytes())
        self.assertEqual(installed.stat().st_mode & 0o111, 0o111)
        self.assertFalse((data / "bin/home-cli").exists())

    def test_development_archives_carry_the_built_renderer(self):
        supplied = os.environ.get("ELASTOS_TEST_HOME_CLI_RENDERER")
        renderer = Path(supplied) if supplied else self.root / "renderer"
        if not supplied:
            renderer.write_bytes(b"renderer fixture")
            renderer.chmod(0o755)
        for script in ("home-frontdoor-smoke.sh", "local-carrier-setup-smoke.sh"):
            with self.subTest(script=script):
                source = (SOURCE / "scripts" / script).read_text()
                function = "def write_capsule_archive" + source.split("def write_capsule_archive", 1)[1].split(
                    '\nwrite_capsule_archive(', 1)[0]
                artifacts = self.root / script
                artifacts.mkdir()
                descriptor = {"release_path": "home-cli-darwin-arm64.tar.gz"}
                scope = {"json": json, "os": os, "pathlib": __import__("pathlib"),
                         "tarfile": tarfile, "hashlib": hashlib, "artifacts_dir": artifacts,
                         "platform_info": lambda name: descriptor}
                exec(compile(function, script, "exec"), scope)
                with patch.dict(os.environ, {"HOME_CLI_RENDERER": str(renderer)}):
                    scope["write_capsule_archive"]("home-cli", self.repo / "capsules/home-cli")
                archive = artifacts / descriptor["release_path"]
                self.assertEqual(descriptor["checksum"], "sha256:" + hashlib.sha256(archive.read_bytes()).hexdigest())
                installed = artifacts / "data/capsules"
                installed.mkdir(parents=True)
                result = self.command("tar", "xzf", str(archive), "-C", str(installed))
                self.assertEqual(result.returncode, 0, result.stderr)
                native = installed / "home-cli/bin/home-cli"
                self.assertEqual(native.read_bytes(), renderer.read_bytes())
                self.assertEqual(native.stat().st_mode & 0o111, 0o111)
                with patch.dict(os.environ, {"HOME_CLI_RENDERER": str(self.root / "missing-renderer")}):
                    with self.assertRaisesRegex(SystemExit, "missing built Home CLI renderer"):
                        scope["write_capsule_archive"]("home-cli", self.repo / "capsules/home-cli")

    def test_development_manifests_bind_the_managed_media_archive(self):
        archive = self.root / "media-tools.tar.gz"
        archive.write_bytes(b"already-validated native media archive")
        for script in ("home-frontdoor-smoke.sh", "local-carrier-setup-smoke.sh"):
            with self.subTest(script=script):
                source = (SOURCE / "scripts" / script).read_text()
                block = 'media_info = platform_info("media-tools")' + source.split(
                    'media_info = platform_info("media-tools")', 1)[1].split(
                    "\ndef write_capsule_archive", 1)[0].split(
                    '\ncache = pathlib.Path', 1)[0].split(
                    '\nif platform == "linux-arm64":', 1)[0]
                artifacts = self.root / script
                artifacts.mkdir()
                descriptor = {"release_path": "media-tools-darwin-arm64.tar.gz"}
                scope = {"os": os, "shutil": shutil, "hashlib": hashlib,
                         "artifacts_dir": artifacts, "platform_info": lambda name: descriptor}
                with patch.dict(os.environ, {"MEDIA_TOOLS_ARCHIVE": str(archive)}):
                    exec(compile(block, script, "exec"), scope)
                staged = artifacts / descriptor["release_path"]
                self.assertEqual(staged.read_bytes(), archive.read_bytes())
                self.assertEqual(descriptor["checksum"], "sha256:" + hashlib.sha256(archive.read_bytes()).hexdigest())
                self.assertEqual(descriptor["size"], archive.stat().st_size)
                with patch.dict(os.environ, {"MEDIA_TOOLS_ARCHIVE": str(self.root / "missing-archive")}):
                    with self.assertRaises(FileNotFoundError):
                        exec(compile(block, script, "exec"), scope)

    def test_arm64_carrier_fixture_stages_the_pinned_engine_archive(self):
        script = "local-carrier-setup-smoke.sh"
        source = (SOURCE / "scripts" / script).read_text()
        block = 'if platform == "linux-arm64":' + source.split(
            'if platform == "linux-arm64":', 1)[1].split("\ndef write_capsule_archive", 1)[0]
        artifacts = self.root / "carrier-artifacts"
        artifacts.mkdir()
        manifest = json.loads((self.repo / "components.json").read_text())
        descriptor = manifest["external"]["llama-server"]["platforms"]["linux-arm64"]
        recipes_path = self.repo / "scripts/release-upstream-recipes.json"
        recipes = json.loads(recipes_path.read_bytes())
        for item in recipes['recipes'][0]['license']['files']:
            item['source']['path'] = str(self.repo / item['source']['path'])
        recipes_path.write_text(json.dumps(recipes))
        spec = importlib.util.spec_from_file_location('carrier_fixture_upstream',
                                                    self.repo / "scripts/release-upstream-input.py")
        upstream = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(upstream)
        scope = {"platform": "linux-arm64", "artifacts_dir": artifacts,
                 "platform_info": lambda name: descriptor, "os": os, "pathlib": __import__("pathlib"),
                 "shutil": shutil, "hashlib": hashlib, "json": json, "tempfile": tempfile,
                 "root": self.repo, "cache": self.root / "upstream-cache", "upstream": upstream,
                 "manifest": manifest}
        with patch.dict(os.environ, {"ELASTOS_LLAMA_ARM64_BUNDLE": str(self.arm64_engine)}):
            exec(compile(block, script, "exec"), scope)
        staged = artifacts / descriptor["release_path"]
        self.assertEqual(descriptor['release_path'], 'llama-server-linux-arm64.tar.gz')
        self.assertEqual(descriptor['checksum'], 'sha256:' + hashlib.sha256(staged.read_bytes()).hexdigest())
        self.assertNotEqual(staged.read_bytes(), self.arm64_engine.read_bytes())
        with tarfile.open(staged) as capsule, tarfile.open(self.arm64_engine) as original:
            for name in ('llama-server', 'libllama.so'):
                path = 'llama-b10516/' + name
                self.assertEqual(capsule.extractfile(path).read(), original.extractfile(path).read())
            self.assertEqual(capsule.extractfile('llama-b10516/LICENSE').read(),
                             (self.repo / 'scripts/fixture-license.txt').read_bytes())
            metadata = json.load(capsule.extractfile('llama-b10516/capsule.json'))
            self.assertEqual((metadata['role'], metadata['type']), ('content', 'data'))
            self.assertIsNotNone(capsule.getmember('llama-b10516/PROVENANCE.json'))
        metadata = manifest['external']['llama-server']['capsule_metadata']['platforms']['linux-arm64']
        self.assertEqual(metadata['checksum'], descriptor['checksum'])
        self.assertEqual(metadata['size'], staged.stat().st_size)
        self.assertNotIn('strategy', descriptor)
        wrong = self.root / "wrong-arm64-engine.tar.gz"
        data = bytearray(self.arm64_engine.read_bytes())
        data[-1] ^= 1
        wrong.write_bytes(data)
        # A fresh cache exercises admission of the supplied bytes; a populated
        # cache correctly reuses the already verified input with the same pin.
        scope['cache'] = self.root / 'wrong-upstream-cache'
        with patch.dict(os.environ, {"ELASTOS_LLAMA_ARM64_BUNDLE": str(wrong)}):
            with self.assertRaisesRegex(ValueError, "upstream input checksum mismatch"):
                exec(compile(block, script, "exec"), scope)

    def test_demo_rejects_old_input_and_overlays_selected_platform(self):
        source = (SOURCE / "scripts/home-demo-local.sh").read_text()
        body = source.split('    "$SETUP_PLATFORM" <<\'PY\'\n', 1)[1].split("\nPY\n", 1)[0]
        installed = self.root / "installed-components.json"
        output = self.root / "demo-components.json"
        native_info = {"release_path": "home-cli-linux-amd64.tar.gz", "extract_path": "home-cli"}
        installed.write_text(json.dumps({"external": {
            "home-cli": {"platforms": {"*": {"release_path": "home-cli.tar.gz"}}},
            "shell": {"platforms": {"*": {"checksum": "sha256:published-shell"}}},
        }}))
        command = ["python3", "-", str(self.repo / "components.json"), str(installed), str(output), "linux-amd64"]
        result = subprocess.run(command, input=body, text=True, capture_output=True, env=self.env)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Published Home CLI input is incompatible", result.stderr)
        self.assertFalse(output.exists())
        data = json.loads(installed.read_text())
        data["external"]["home-cli"]["platforms"] = {"linux-amd64": native_info}
        installed.write_text(json.dumps(data))
        result = subprocess.run(command, input=body, text=True, capture_output=True, env=self.env)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing managed media tools", result.stderr)
        data["external"]["media-tools"] = {"platforms": {"linux-amd64": {
            "release_path": "media-tools-linux-amd64.tar.gz", "extract_path": "media-tools"}}}
        installed.write_text(json.dumps(data))
        result = subprocess.run(command, input=body, text=True, capture_output=True, env=self.env)
        self.assertEqual(result.returncode, 0, result.stderr)
        prepared = json.loads(output.read_text())
        self.assertEqual(prepared["external"]["shell"]["platforms"]["linux-amd64"],
                         {"checksum": "sha256:published-shell"})
        self.assertEqual(prepared["external"]["home-cli"]["platforms"]["linux-amd64"], native_info)

    def test_linux_missing_locks_stops_before_build(self):
        output, result = self.prepare(env={**self.env, "MOCK_OS": "Linux", "MOCK_ARCH": "x86_64"})
        self.assertNotEqual(result.returncode, 0)
        for name in LINUX_ONLY:
            self.assertIn(f"capsules/{name}/Cargo.lock ({name})", result.stderr)
        self.assertFalse(output.exists())
        calls = [json.loads(line)["args"][0] for line in (self.root / "cargo.log").read_text().splitlines()]
        self.assertNotIn("build", calls)

    def test_dirty_source_and_existing_symlink_refuse_before_build(self):
        (self.repo / "components.json").write_text("dirty")
        output, result = self.prepare()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Source checkout must be clean", result.stderr)
        self.assertFalse(output.exists())
        self.assertFalse((self.root / "cargo.log").exists())
        output.symlink_to(self.root / "absent-target")
        _, result = self.prepare()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Output already exists", result.stderr)
        self.assertTrue(output.is_symlink())

    def test_three_platform_inputs_admit_with_resolved_cache_and_linux_audit(self):
        for name in LINUX_ONLY:
            (self.repo / "capsules" / name / "Cargo.lock").write_text("version = 4\n")
        self.commit("reviewed fixture locks")
        inputs = []
        for platform, setup_platform, host_os, host_arch in (
            ("aarch64-darwin", "darwin-arm64", "Darwin", "arm64"),
            ("x86_64-linux", "linux-amd64", "Linux", "x86_64"),
            ("aarch64-linux", "linux-arm64", "Linux", "aarch64"),
        ):
            env = {**self.env, "MOCK_OS": host_os, "MOCK_ARCH": host_arch}
            del env["CARGO_TARGET_DIR"]
            output, result = self.prepare(platform, env=env)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertTrue((output / f"artifacts/elastos-{platform}").is_file())
            self.assert_media_archive(output, platform, setup_platform)
            if platform == "aarch64-linux":
                prepared = json.loads((output / "components.json").read_bytes())
                descriptor = prepared["external"]["llama-server"]["platforms"]["linux-arm64"]
                self.assertFalse(any(field in descriptor for field in ("strategy", "url", "cid")))
                with tarfile.open(self.arm64_engine) as original, tarfile.open(output / "artifacts" / descriptor["release_path"]) as capsule:
                    for name in ("llama-server", "libllama.so"):
                        relative = "llama-b10516/" + name
                        self.assertEqual(capsule.extractfile(relative).read(), original.extractfile(relative).read())
                        self.assertEqual(capsule.getmember(relative).mode, original.getmember(relative).mode)
                self.assertTrue((output / "upstream-recipes.json").is_file())
                self.assertTrue((output / "upstream-input.json").is_file())
            inputs.extend(["--input", f"{platform}={output}"])
        media_calls = [json.loads(line) for line in (self.root / "media.log").read_text().splitlines()]
        self.assertEqual(media_calls, [
            {"output": str(Path(self.env["MOCK_TARGET"]) / "media-tools" / platform), "platform": platform}
            for platform in ("darwin-arm64", "linux-amd64", "linux-arm64")])
        result = self.command("python3", "scripts/release-platform-input.py", "validate-inputs", *inputs)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("--platform x86_64-linux", (self.root / "audit.log").read_text())
        self.assertIn("--platform aarch64-linux", (self.root / "audit.log").read_text())
        calls = [json.loads(line) for line in (self.root / "cargo.log").read_text().splitlines()]
        self.assertTrue(any(entry["args"][0] == "metadata" for entry in calls))
        for call in calls:
            if call["args"][0] == "build":
                self.assertEqual(call["target_dir"], self.env["MOCK_TARGET"])
        failed, result = self.prepare("audit-refused", env={**env, "FAIL_AUDIT": "1"})
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(failed.exists())


if __name__ == "__main__":
    unittest.main()
