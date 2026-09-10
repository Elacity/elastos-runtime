#!/usr/bin/env python3
"""Run the preparation worker with real packaging/receipts and fake native builds."""

import json
import os
from pathlib import Path
import re
import subprocess
import tarfile
import tempfile
import unittest


SOURCE = Path(__file__).resolve().parent.parent
LINUX_ONLY = {"browser-engine-supervisor", "browser-native-proxy-engine", "browser-stream-bridge"}
APPS = [
    "home-cli", "home-gui", "home", "system", "wallet-metamask", "wallet-unisat",
    "wallet-walletconnect", "wallet", "browser", "documents", "library", "marketplace",
    "archive-manager", "inbox", "services", "people", "gba-emulator", "gba-ucity",
    "gba-nonogram", "chat-room", "assistant", "home-agent", "elacity-player",
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
    assert args[0] == "build" and "--locked" in args and "--target" in args, args
    if os.environ.get("FAIL_BUILD"):
        sys.exit(19)
    target = args[args.index("--target") + 1]
    name = "elastos" if "--bin" in args else pathlib.Path.cwd().name
    output = pathlib.Path(os.environ["CARGO_TARGET_DIR"]) / target / "release" / name
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


class PrepareWorkerTest(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix="release-worker-fixture-")
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        self.repo = self.root / "source"
        scripts = self.repo / "scripts"
        scripts.mkdir(parents=True)
        for name in (
            "publish-release.sh", "prepare-release-platform.sh", "release-platform-input.py",
            "components-release-integrity-check.py", "check-versioning.sh",
        ):
            (scripts / name).write_bytes((SOURCE / "scripts" / name).read_bytes())
            (scripts / name).chmod(0o755)
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
        (self.repo / "components.json").write_text(json.dumps({
            "schema": "elastos.components/v1", "external": external,
            "profiles": {"home": {"components": ["home", "shell"]}}}))
        (self.repo / "elastos").mkdir()
        (self.repo / "elastos/Cargo.toml").write_text("[workspace]\n")
        (self.repo / "elastos/Cargo.lock").write_text("version = 4\n")
        (self.repo / ".gitignore").write_text("__pycache__/\nsecret.txt\n")
        mock = self.root / "mock"
        mock.mkdir()
        (mock / "cargo").write_text(MOCK_CARGO)
        (mock / "rustup").write_text(
            '#!/bin/sh\nprintf "aarch64-apple-darwin\\nx86_64-unknown-linux-musl\\naarch64-unknown-linux-musl\\n"\n')
        (mock / "rustc").write_text('#!/bin/sh\nprintf "rustc fixture 1.91\\n"\n')
        (mock / "uname").write_text(
            '#!/bin/sh\nif [ "$1" = -s ]; then echo "${MOCK_OS:-Darwin}"; '
            'else echo "${MOCK_ARCH:-arm64}"; fi\n')
        for path in mock.iterdir():
            path.chmod(0o755)
        self.env = {**os.environ, "PATH": str(mock) + os.pathsep + os.environ["PATH"],
                    "CARGO_TARGET_DIR": str(self.root / "native-cache"), "CARGO_BUILD_JOBS": "4",
                    "MOCK_TARGET": str(self.root / "resolved-cache"),
                    "MOCK_LOG": str(self.root / "cargo.log"),
                    "MOCK_AUDIT_LOG": str(self.root / "audit.log")}
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
                             ("git", "config", "commit.gpgsign", "false")])
        commands.extend([("git", "add", "."), ("git", "commit", "-m", message)])
        for command in commands:
            result = self.command(*command)
            self.assertEqual(result.returncode, 0, result.stderr)

    def prepare(self, name="prepared", env=None):
        output = self.root / name
        result = self.command("/bin/bash", "scripts/prepare-release-platform.sh",
                              "--version", "0.7.1", "--output", str(output), env=env)
        self.assertEqual(list(self.root.glob(".release-platform.*")), [], "temporary sibling leaked")
        return output, result

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
        result = self.command("python3", "scripts/release-platform-input.py", "verify", str(output))
        self.assertEqual(result.returncode, 0, result.stderr)
        commands = [json.loads(line) for line in (self.root / "cargo.log").read_text().splitlines()]
        builds = [entry for entry in commands if entry["args"][0] == "build"]
        self.assertEqual(len(builds), 1 + len(self.native) - len(LINUX_ONLY))
        for build in builds:
            self.assertIn("--locked", build["args"])
            self.assertIn("aarch64-apple-darwin", build["args"])
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
        for platform, host_os, host_arch in (
            ("aarch64-darwin", "Darwin", "arm64"),
            ("x86_64-linux", "Linux", "x86_64"),
            ("aarch64-linux", "Linux", "aarch64"),
        ):
            env = {**self.env, "MOCK_OS": host_os, "MOCK_ARCH": host_arch}
            del env["CARGO_TARGET_DIR"]
            output, result = self.prepare(platform, env=env)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertTrue((output / f"artifacts/elastos-{platform}").is_file())
            inputs.extend(["--input", f"{platform}={output}"])
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
