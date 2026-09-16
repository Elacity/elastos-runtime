#!/usr/bin/env python3
"""Check publisher platform selection and local exports without publishing."""

import os
import hashlib
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import tarfile
import unicodedata
import unittest


PUBLISHER = Path(__file__).resolve().with_name("publish-release.sh")
CHECKER = PUBLISHER.with_name("components-release-integrity-check.py")
spec = importlib.util.spec_from_file_location("components_integrity", CHECKER)
integrity = importlib.util.module_from_spec(spec)
spec.loader.exec_module(integrity)

RELEASE_PLATFORMS = {"x86_64-linux": "linux-amd64", "aarch64-linux": "linux-arm64", "aarch64-darwin": "darwin-arm64"}
PUBLICATION_COMMANDS = ("elastos", "ipfs-provider", "ipfs", "curl", "cargo", "cloudflared", "git")


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def publication_fixture(prepared, salt=b"", version="0.7.1", channel="stable"):
    """Write a tiny three-platform publication set and return the served files it advertises."""
    artifacts = prepared / "artifacts"
    artifacts.mkdir(parents=True)
    files = {"home.tar.gz": b"universal app" + salt, "shell-capsule-metadata.tar.gz": b"provider metadata" + salt}
    for platform, setup in RELEASE_PLATFORMS.items():
        files[f"elastos-{platform}"] = b"runtime " + platform.encode() + salt
        files[f"shell-{setup}"] = b"native shell " + platform.encode() + salt
        files[f"shell-{platform}.capsule.tar.gz"] = b"shell capsule " + platform.encode() + salt

    def descriptor(name, install_path, extract_path=None):
        record = {"release_path": name, "install_path": install_path, "cid": "bafy-" + name,
                  "checksum": "sha256:" + sha256(files[name]), "size": len(files[name])}
        if extract_path:
            record["extract_path"] = extract_path
        return record

    platforms = {}
    for platform, setup in RELEASE_PLATFORMS.items():
        capsule = files[f"shell-{platform}.capsule.tar.gz"]
        manifest = {
            "schema": "elastos.components/v1",
            "capsules": {"shell": {"cid": "bafy-shell", "sha256": sha256(capsule), "size": len(capsule),
                                   "platforms": [platform]}},
            "external": {
                "home": {"platforms": {"*": descriptor("home.tar.gz", "capsules/home", "home")}},
                "shell": {"provider_runtime": {},
                          "platforms": {setup: descriptor(f"shell-{setup}", "bin/shell")},
                          "capsule_metadata": {"install_path": "capsules/shell", "platforms": {
                              "*": descriptor("shell-capsule-metadata.tar.gz", "capsules/shell", "shell")}}},
                "kubo": {"platforms": {"*": {"url": "https://example.invalid/kubo.tar.gz", "install_path": "bin/ipfs",
                                             "checksum": "sha256:" + "d" * 64, "size": 100}}},
            },
            "profiles": {"home": {"components": ["home", "shell", "kubo"]}},
        }
        files[f"components-{platform}.json"] = json.dumps(manifest, indent=2).encode()
        platforms[platform] = {
            "binary": {"cid": "bafy-" + platform, "sha256": sha256(files[f"elastos-{platform}"]),
                       "size": len(files[f"elastos-{platform}"])},
            "components": {"cid": "bafy-components-" + platform, "sha256": sha256(files[f"components-{platform}.json"]),
                           "size": len(files[f"components-{platform}.json"])},
        }
    for name, data in files.items():
        (artifacts / name).write_bytes(data)
    release = json.dumps({"payload": {"schema": "elastos.release/v1", "channel": channel, "version": version,
                                      "released_at": 1, "prev_release_cid": None, "platforms": platforms},
                          "signature": "fixture", "signer_did": "did:key:fixture"}).encode()
    head = json.dumps({"payload": {"schema": "elastos.release.head/v1", "channel": channel, "version": version,
                                   "latest_release_cid": "bafy-release", "release_sha256": sha256(release),
                                   "signer_did": "did:key:fixture"},
                       "signature": "fixture", "signer_did": "did:key:fixture"}).encode()
    install = b"#!/bin/sh\necho install" + salt + b"\n"
    (prepared / "release.json").write_bytes(release)
    (prepared / "release-head.json").write_bytes(head)
    (prepared / "install.sh").write_bytes(install)
    served = {"release.json": release, "release-head.json": head, "install.sh": install}
    served.update({"artifacts/" + name: data for name, data in files.items()})
    return served


def rebind_head(prepared, **payload_changes):
    """Rebind release-head.json to the current release.json bytes, then apply payload edits."""
    head = json.loads((prepared / "release-head.json").read_bytes())
    head["payload"]["release_sha256"] = sha256((prepared / "release.json").read_bytes())
    head["payload"].update(payload_changes)
    (prepared / "release-head.json").write_bytes(json.dumps(head).encode())


def rewrite_release(prepared, edit):
    release = json.loads((prepared / "release.json").read_bytes())
    edit(release)
    (prepared / "release.json").write_bytes(json.dumps(release).encode())
    rebind_head(prepared)


def snapshot(root):
    """Byte-level view of a tree without following links, so unchanged means identical."""
    view = {}

    def visit(directory):
        for entry in sorted(directory.iterdir()):
            key = str(entry.relative_to(root))
            if entry.is_symlink():
                view[key] = ("link", os.readlink(entry))
            elif entry.is_dir():
                view[key] = ("dir",)
                visit(entry)
            else:
                view[key] = ("file", entry.read_bytes())

    if root.is_dir() and not root.is_symlink():
        visit(root)
    return view


def served_view(files):
    view = {"artifacts": ("dir",)}
    view.update({name: ("file", data) for name, data in files.items()})
    return view


class PlatformArtifactExportTest(unittest.TestCase):
    def test_source_provider_metadata_archives_pass_admission(self):
        repo = PUBLISHER.parent.parent
        external = json.loads((repo / "components.json").read_text())["external"]
        sources = {}
        for name, component in external.items():
            if not isinstance(component.get("provider_runtime"), dict):
                continue
            for parent in (repo / "capsules", repo / "elastos/capsules"):
                if (parent / name / "capsule.json").is_file():
                    sources[name] = parent / name
                    break
        spec = importlib.util.spec_from_file_location(
            "platform_input", PUBLISHER.with_name("release-platform-input.py"))
        admission = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(admission)
        with tempfile.TemporaryDirectory() as root:
            result = subprocess.run(
                ["bash", "-euc", 'source "$1"; build_platform_independent_provider_capsule_metadata_assets',
                 "provider-metadata-test", str(PUBLISHER)], cwd=repo,
                env={**os.environ, "TMPDIR": root, "RELEASE_PREPARE_SOURCE_COMMIT": ""},
                capture_output=True, text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            records = json.loads(result.stdout)["external"]
            self.assertEqual(set(records), set(sources))
            for name, source in sources.items():
                with self.subTest(provider=name):
                    descriptor = records[name]["capsule_metadata"]["platforms"]["*"]
                    archive_path = Path(root) / "supported-provider-contracts-universal" / descriptor["release_path"]
                    admission.check_archive(archive_path, name, provider=True)
                    self.assertEqual(descriptor["checksum"], "sha256:" + hashlib.sha256(archive_path.read_bytes()).hexdigest())
                    self.assertEqual(descriptor["size"], archive_path.stat().st_size)
                    manifest = json.loads((source / "capsule.json").read_text())
                    members = ["capsule.json"] + [f'{manifest["icon"]}/icon-{size}.png' for size in (32, 64, 128, 256)]
                    with tarfile.open(archive_path) as archive:
                        for member in members:
                            self.assertEqual(archive.extractfile(f"{name}/{member}").read(), (source / member).read_bytes())

    def test_direct_asset_publication_attaches_cids_after_preparation(self):
        source = PUBLISHER.read_text()
        function = "publish_direct_assets() {" + source.split(
            "publish_direct_assets() {", 1
        )[1].split("\n}\n", 1)[0] + "\n}\n"
        for failure in (None, "upload", "missing", "duplicate", "unsafe"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as root:
                root = Path(root)
                for directory, filename in (
                    ("supported-assets-aarch64-darwin", "shell-darwin-arm64"),
                    ("supported-assets-universal", "home.tar.gz"),
                    ("supported-provider-contracts-universal", "shell-metadata.tar.gz"),
                ):
                    (root / directory).mkdir()
                    (root / directory / filename).write_bytes(b"asset")
                data = {"external": {
                    "home": {"platforms": {"*": {"release_path": "home.tar.gz", "size": 5}}},
                    "shell": {
                        "platforms": {"darwin-arm64": {"release_path": "shell-darwin-arm64", "size": 5}},
                        "capsule_metadata": {"platforms": {"*": {"release_path": "shell-metadata.tar.gz", "size": 5}}},
                    },
                }}
                if failure == "missing":
                    (root / "supported-assets-universal/home.tar.gz").unlink()
                if failure == "duplicate":
                    (root / "supported-assets-aarch64-darwin/home.tar.gz").write_bytes(b"asset")
                if failure == "unsafe":
                    data["external"]["home"]["platforms"]["*"]["release_path"] = "../escape"
                result = subprocess.run(["bash", "-euc", function + '''
die() { echo "$*" >&2; exit 1; }
ipfs_add() {
    printf '%s\\n' "$(basename "$1")" >> "$TEST_UPLOAD"
    [[ "$TEST_FAILURE" != upload ]] || return 93
    printf 'cid-%s\\n' "$(basename "$1")"
}
result=$(publish_direct_assets "$TEST_DATA" aarch64-darwin)
printf '%s\\n' "$result"
'''], env={**os.environ, "TMPDIR": str(root), "TEST_FAILURE": failure or "",
           "TEST_DATA": json.dumps(data), "TEST_UPLOAD": str(root / "uploaded")},
                    capture_output=True, text=True)
                if failure:
                    self.assertNotEqual(result.returncode, 0)
                else:
                    self.assertEqual(result.returncode, 0, result.stderr)
                    received = json.loads(result.stdout)
                    for name, component in received["external"].items():
                        for descriptor in component["platforms"].values():
                            self.assertEqual(descriptor.pop("cid"), "cid-" + descriptor["release_path"])
                        if "capsule_metadata" in component:
                            descriptor = component["capsule_metadata"]["platforms"]["*"]
                            self.assertEqual(descriptor.pop("cid"), "cid-shell-metadata.tar.gz")
                    self.assertEqual(received, data)
                    self.assertEqual(len((root / "uploaded").read_text().splitlines()), 3)

    def test_failed_asset_preparation_stops_before_any_upload(self):
        source = PUBLISHER.read_text()
        start = source.index('info "Preparing direct share/open support assets..."')
        phase = source[start:source.index("# ── Step 5:", start)]
        for failure in ("apps", "metadata", "native", "cross"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as root:
                result = subprocess.run(["bash", "-euc", '''
info() { :; }
build_platform_independent_direct_assets() { [[ "$TEST_FAILURE" != apps ]] || return 93; echo '{}'; }
build_platform_independent_provider_capsule_metadata_assets() { [[ "$TEST_FAILURE" != metadata ]] || return 93; echo '{}'; }
build_supported_direct_assets() {
    [[ "$TEST_FAILURE" != native ]] || return 93
    [[ "$TEST_FAILURE" != cross || "$1" != aarch64-linux ]] || return 93
    echo '{}'
}
merge_direct_assets() { echo '{}'; }
publish_platform_capsules() { touch "$TEST_UPLOAD"; echo '{}'; }
publish_direct_assets() { touch "$TEST_UPLOAD"; echo '{}'; }
''' + phase], env={**os.environ, "TEST_FAILURE": failure, "TEST_UPLOAD": str(Path(root) / "uploaded"),
                  "PLATFORM": "aarch64-darwin", "SETUP_PLATFORM": "darwin-arm64",
                  "NATIVE_RUST_TARGET": "aarch64-apple-darwin", "CROSS_ARCH": "aarch64",
                  "CROSS_PLATFORM": "aarch64-linux", "CROSS_SETUP_PLATFORM": "linux-arm64",
                  "CROSS_RUST_TARGET": "aarch64-unknown-linux-musl", "ARTIFACTS_DIR": root,
                  "CROSS_ARTIFACTS_DIR": root}, capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0, result.stderr)
                self.assertFalse((Path(root) / "uploaded").exists())

    def test_fresh_projection_packaging_and_asset_records_do_not_publish(self):
        source = PUBLISHER.read_text()
        names = ("capsule_manifest_field", "copy_clean_capsule_tree", "copy_release_source_file", "create_capsule_tar",
                 "stage_wasm_capsule", "build_packaged_capsule_archive",
                 "record_direct_asset", "record_provider_capsule_metadata_asset",
                 "sha256", "file_size")
        functions = "\n".join(name + "() {" + source.split(name + "() {", 1)[1].split(
            "\n}\n", 1)[0] + "\n}\n" for name in names)
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            capsule = root / "source"
            (capsule / "browser").mkdir(parents=True)
            (capsule / "browser/index.html").write_text("<title>Home</title>")
            (capsule / "capsule.json").write_text(json.dumps({
                "type": "wasm", "entrypoint": "browser/index.html",
                "runtime_abi": "elastos.runtime-projection/v1",
            }))
            result = subprocess.run(["bash", "-euc", functions + '''
die() { echo "$*" >&2; exit 1; }
info() { :; }
resolve_capsule_dir() { echo "$TEST_SOURCE"; }
ipfs_add() { touch "$TEST_UPLOAD"; echo unexpected-upload; }
archive=$(build_packaged_capsule_archive aarch64-darwin home)
record_direct_asset '{}' home "$archive" capsules/home home.tar.gz home > "$TMPDIR/app.json"
record_provider_capsule_metadata_asset '{}' provider "$archive" capsules/provider provider.tar.gz provider > "$TMPDIR/provider.json"
'''], env={**os.environ, "TMPDIR": str(root), "TEST_SOURCE": str(capsule),
           "TEST_UPLOAD": str(root / "uploaded")}, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stderr, "")
            self.assertFalse((root / "uploaded").exists(), "packaging called IPFS")
            app = json.loads((root / "app.json").read_text())["home"]
            metadata = json.loads((root / "provider.json").read_text())["external"]["provider"]["capsule_metadata"]["platforms"]["*"]
            for record in (app, metadata):
                self.assertNotIn("cid", record)
                self.assertGreater(record["size"], 0)
                self.assertRegex(record["checksum"], r"^sha256:[0-9a-f]{64}$")
            with tarfile.open(root / "support-assets-aarch64-darwin/home.tar.gz") as archive:
                self.assertEqual(archive.extractfile("home/browser/index.html").read(), b"<title>Home</title>")
            # A command substitution on stock Bash disables implicit errexit;
            # the builder must return an archive-write failure explicitly.
            result = subprocess.run(["bash", "-euc", functions + '''
die() { echo "$*" >&2; exit 1; }
info() { :; }
resolve_capsule_dir() { echo "$TEST_SOURCE"; }
create_capsule_tar() { return 93; }
archive=$(build_packaged_capsule_archive aarch64-darwin home)
touch "$TEST_FALSE_SUCCESS"
'''], env={**os.environ, "TMPDIR": str(root), "TEST_SOURCE": str(capsule),
           "TEST_FALSE_SUCCESS": str(root / "false-success")}, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse((root / "false-success").exists())

    def test_capsule_archive_is_portable_and_reproducible(self):
        source = PUBLISHER.read_text()
        function = "create_capsule_tar() {" + source.split(
            "create_capsule_tar() {", 1
        )[1].split("\n}\n", 1)[0] + "\n}\n"
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            stage = root / "staged apps"
            app = stage / "example"
            app.mkdir(parents=True)
            (app / "empty").mkdir()
            (app / "café with spaces.txt").write_text("content\n")
            (app / "launch").write_text("#!/bin/sh\nexit 0\n")
            (app / "launch").chmod(0o700)
            (app / "alias").symlink_to("café with spaces.txt")
            os.link(app / "launch", app / "launch-copy")
            archives = [root / "first.tar.gz", root / "different name.tar.gz"]
            def pack(path):
                result = subprocess.run(
                    ["bash", "-euc", function + 'create_capsule_tar "$1" "$2" example',
                     "archive-test", str(path), str(stage)],
                    capture_output=True, text=True,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
            pack(archives[0])
            # An equivalent checkout can have different ownership, times, umask
            # permissions and inode sharing; none should alter universal bytes.
            (app / "launch-copy").unlink()
            (app / "launch-copy").write_bytes((app / "launch").read_bytes())
            for path in [app, *app.rglob("*")]:
                if path.is_symlink():
                    continue
                os.utime(path, (1234567890, 1234567890))
                path.chmod(0o755 if path.is_dir() or path.name.startswith("launch") else 0o600)
            pack(archives[1])
            self.assertEqual(archives[0].read_bytes(), archives[1].read_bytes())
            with tarfile.open(archives[0]) as archive:
                entries = archive.getmembers()
                self.assertEqual([e.name for e in entries], sorted(e.name for e in entries))
                self.assertEqual({e.name for e in entries}, {
                    "example", "example/empty", "example/café with spaces.txt",
                    "example/launch", "example/launch-copy", "example/alias",
                })
                for entry in entries:
                    self.assertEqual((entry.uid, entry.gid, entry.mtime), (0, 0, 0))
                    self.assertEqual((entry.uname, entry.gname), ("", ""))
                self.assertEqual(archive.getmember("example/launch").mode, 0o755)
                self.assertEqual(archive.getmember("example/café with spaces.txt").mode, 0o644)
                self.assertTrue(archive.getmember("example/launch-copy").isfile())
                self.assertTrue(archive.getmember("example/alias").issym())
            extracted = root / "extracted"
            extracted.mkdir()
            subprocess.run(["tar", "-xzf", str(archives[0]), "-C", str(extracted)], check=True)
            self.assertEqual((extracted / "example/café with spaces.txt").read_text(), "content\n")
            self.assertEqual(unicodedata.normalize("NFC", os.readlink(extracted / "example/alias")),
                             "café with spaces.txt")
            self.assertEqual((extracted / "example/alias").read_text(), "content\n")
            self.assertTrue(os.access(extracted / "example/launch", os.X_OK))

    def test_low_level_dry_run_rejects_before_side_effects(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            commands = root / "commands"
            commands.mkdir()
            effects = root / "effects"
            for command in ("mktemp", "mkdir", "curl", "cargo", "ipfs-provider", "git"):
                stub = commands / command
                stub.write_text('#!/bin/sh\nprintf "%s\\n" "$0" >> "$TEST_EFFECT_LOG"\nexit 93\n')
                stub.chmod(0o755)
            result = subprocess.run(
                ["bash", str(PUBLISHER), "--version", "0.7.1", "--dry-run"],
                env={**os.environ, "PATH": str(commands) + os.pathsep + os.environ["PATH"],
                     "TEST_EFFECT_LOG": str(effects), "ELASTOS_PUBLISH_STATE_DIR": str(root / "state")},
                capture_output=True, text=True,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(effects.exists(), result.stderr)
            self.assertFalse((root / "state").exists())
            self.assertIn("elastos publish-release --dry-run", result.stderr)

    def test_artifact_gate_stops_before_release_signing(self):
        source = PUBLISHER.read_text()
        function = "stage_release_artifacts() {" + source.split(
            "stage_release_artifacts() {", 1
        )[1].split("\n}\n", 1)[0] + "\n}\n"
        start = source.index("# Assemble the files advertised by this release")
        gate = source[start:source.index("# End build-mode preparation.", start)]
        for failure in (None, "missing", "stale", "runtime", "manifest",
                        "cross-valid", "cross-missing", "cross-runtime", "cross-manifest", "cross-app"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as root:
                root = Path(root)
                app = b"app bytes"
                runtime = b"runtime"
                manifest = json.dumps({"external": {"home": {"platforms": {"*": {
                    "release_path": "home.tar.gz", "size": len(app),
                    "checksum": "sha256:" + hashlib.sha256(app).hexdigest(),
                }}}}}).encode()
                (root / "elastos").write_bytes(runtime)
                (root / "components.json").write_bytes(manifest)
                cross = bool(failure and failure.startswith("cross-"))
                cross_runtime = b"cross runtime"
                cross_manifest = manifest
                if cross:
                    (root / "cross-elastos").write_bytes(cross_runtime)
                    if failure == "cross-app":
                        other = json.loads(manifest)
                        other["external"]["home"]["platforms"]["*"]["checksum"] = "sha256:" + "0" * 64
                        cross_manifest = json.dumps(other).encode()
                    if failure != "cross-missing":
                        (root / "components-aarch64-linux.json").write_bytes(cross_manifest)
                universal = root / "supported-assets-universal"
                universal.mkdir()
                if failure != "missing":
                    (universal / "home.tar.gz").write_bytes(b"bad bytes" if failure == "stale" else app)
                result = subprocess.run(
                    ["bash", "-euc", '''
die() { echo "$*" >&2; exit 1; }
sha256() { python3 -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())' "$1"; }
''' + function + gate + 'touch "$TEST_SIGN_MARKER"'],
                    cwd=PUBLISHER.parent.parent,
                    env={
                        **os.environ,
                        "TMPDIR": str(root),
                        "TEST_SIGN_MARKER": str(root / "signing-reached"),
                        "STAGED_ELASTOS": str(root / "elastos"),
                        "ARTIFACTS_DIR": str(root / "artifacts"),
                        "PLATFORM": "aarch64-darwin", "SETUP_PLATFORM": "darwin-arm64",
                        "CROSS_PLATFORM": "aarch64-linux" if cross else "",
                        "CROSS_SETUP_PLATFORM": "linux-arm64" if cross else "",
                        "CROSS_ELASTOS": str(root / "cross-elastos") if cross else "",
                        "CROSS_ARCH": "aarch64" if cross else "",
                        "CROSS_BINARY_SHA256": "0" * 64 if failure == "cross-runtime" else hashlib.sha256(cross_runtime).hexdigest(),
                        "CROSS_COMPONENTS_SHA256": "0" * 64 if failure == "cross-manifest" else hashlib.sha256(cross_manifest).hexdigest(),
                        "BINARY_SHA256": "0" * 64 if failure == "runtime" else hashlib.sha256(runtime).hexdigest(),
                        "COMPONENTS_SHA256": "0" * 64 if failure == "manifest" else hashlib.sha256(manifest).hexdigest(),
                    },
                    capture_output=True, text=True,
                )
                if failure in (None, "cross-valid"):
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertTrue((root / "signing-reached").exists())
                else:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertFalse((root / "signing-reached").exists())
                    expected_error = {
                        "missing": "home.tar.gz", "stale": "artifact checksum mismatch",
                        "runtime": "Staged runtime differs", "manifest": "Staged components differ",
                        "cross-runtime": "Staged runtime differs", "cross-manifest": "Staged components differ",
                        "cross-missing": "components-aarch64-linux.json", "cross-app": "artifact checksum mismatch",
                    }[failure]
                    self.assertIn(expected_error, result.stderr)

    def test_advertised_provider_and_capsule_bytes_are_checked(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            payload = b"abc"
            checksum = "sha256:" + hashlib.sha256(payload).hexdigest()
            def descriptor(path):
                return {"release_path": path, "checksum": checksum, "size": len(payload)}
            metadata = descriptor("provider-metadata.tar.gz")
            data = {
                "external": {"provider": {
                    "platforms": {"linux-arm64": descriptor("provider")},
                    "capsule_metadata": {"platforms": {"*": metadata}},
                }},
                "capsules": {"shell": {
                    "platforms": ["aarch64-linux"], "sha256": checksum.split(":")[1], "size": 3,
                }},
            }
            files = ("provider", "provider-metadata.tar.gz", "shell-aarch64-linux.capsule.tar.gz")
            for name in files:
                (root / name).write_bytes(payload)
            audit = lambda: integrity.audit_release_artifacts(data, ["linux-arm64"], root)
            self.assertEqual(audit(), [])
            for bad in (None, [], "aarch64-linux", [False]):
                data["capsules"]["shell"]["platforms"] = bad
                self.assertTrue(audit(), bad)
            data["capsules"]["shell"]["platforms"] = ["aarch64-linux"]
            for name in files:
                (root / name).unlink()
                self.assertTrue(audit(), name)
                (root / name).write_bytes(b"def")
                self.assertTrue(audit(), name)
                (root / name).write_bytes(payload)
            for bad in ("../escape", "/absolute", "nested/../escape", "bad\\path"):
                metadata["release_path"] = bad
                self.assertTrue(audit(), bad)
            metadata["release_path"] = "provider-metadata.tar.gz"
            for size in (True, 0, -1, 4):
                metadata["size"] = size
                self.assertTrue(audit(), size)
            metadata["size"] = 3
            (root / "provider-metadata.tar.gz").unlink()
            (root / "provider-metadata.tar.gz").symlink_to(root / "provider")
            self.assertTrue(audit())
            (root / "provider-metadata.tar.gz").unlink()
            (root / "provider-metadata.tar.gz").mkdir()
            self.assertTrue(audit())
            self.assertTrue(integrity.audit_release_artifacts(data, [], root))

    def test_served_artifact_stage_includes_universal_apps(self):
        source = PUBLISHER.read_text()
        function = "stage_release_artifacts() {" + source.split(
            "stage_release_artifacts() {", 1
        )[1].split("\n}\n", 1)[0] + "\n}\n"
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            expected = {
                "elastos-aarch64-darwin": b"mac runtime",
                "elastos-aarch64-linux": b"linux runtime",
                "components-aarch64-darwin.json": b"mac components",
                "components-aarch64-linux.json": b"linux components",
                "home.tar.gz": b"universal app",
                "ipfs-provider-darwin-arm64": b"native mac provider",
                "ipfs-provider-linux-arm64": b"native linux provider",
                "ipfs-provider-capsule-metadata.tar.gz": b"provider metadata",
                "shell-aarch64-darwin.capsule.tar.gz": b"mac capsule",
                "shell-aarch64-linux.capsule.tar.gz": b"linux capsule",
            }
            inputs = {
                "elastos": "elastos-aarch64-darwin",
                "cross-elastos": "elastos-aarch64-linux",
                "components.json": "components-aarch64-darwin.json",
                "components-aarch64-linux.json": "components-aarch64-linux.json",
                "supported-assets-universal/home.tar.gz": "home.tar.gz",
                "supported-assets-aarch64-darwin/ipfs-provider-darwin-arm64": "ipfs-provider-darwin-arm64",
                "supported-assets-aarch64-linux/ipfs-provider-linux-arm64": "ipfs-provider-linux-arm64",
                "supported-provider-contracts-universal/ipfs-provider-capsule-metadata.tar.gz": "ipfs-provider-capsule-metadata.tar.gz",
                "artifacts/shell.capsule.tar.gz": "shell-aarch64-darwin.capsule.tar.gz",
                "artifacts-aarch64/shell.capsule.tar.gz": "shell-aarch64-linux.capsule.tar.gz",
            }
            for path, artifact in inputs.items():
                path = root / path
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(expected[artifact])
            subprocess.run(
                ["bash", "-euc", function + 'stage_release_artifacts "$TEST_DEST"'],
                cwd=root,
                env={
                    **os.environ,
                    "TMPDIR": str(root),
                    "TEST_DEST": str(root / "served"),
                    "STAGED_ELASTOS": str(root / "elastos"),
                    "CROSS_ELASTOS": str(root / "cross-elastos"),
                    "ARTIFACTS_DIR": str(root / "artifacts"),
                    "PLATFORM": "aarch64-darwin",
                    "CROSS_PLATFORM": "aarch64-linux",
                    "CROSS_ARCH": "aarch64",
                },
                check=True, capture_output=True, text=True,
            )
            self.assertEqual(
                {p.name: p.read_bytes() for p in (root / "served").iterdir()}, expected
            )

    def test_native_tools_use_host_os_while_guests_stay_linux(self):
        source = PUBLISHER.read_text()
        start = source.index("OS=$(uname -s")
        end = source.index("# ── Cross-compilation setup", start)
        platform_setup = source[start:end]
        native_call = next(
            line for line in source.splitlines()
            if line.startswith("HOST_PLATFORM_DIRECT_ASSETS=$(build_supported_direct_assets")
        )
        for system, machine, release_platform, setup_platform, native_target, guest_target in (
            ("Linux", "x86_64", "x86_64-linux", "linux-amd64", "x86_64-unknown-linux-musl", "x86_64-unknown-linux-musl"),
            ("Linux", "aarch64", "aarch64-linux", "linux-arm64", "aarch64-unknown-linux-musl", "aarch64-unknown-linux-musl"),
            ("Darwin", "arm64", "aarch64-darwin", "darwin-arm64", "aarch64-apple-darwin", "aarch64-unknown-linux-musl"),
        ):
            with self.subTest(system=system, machine=machine):
                result = subprocess.run(
                    ["bash", "-euc", '''
uname() { case "$1" in -s) echo "$TEST_SYSTEM";; -m) echo "$TEST_MACHINE";; *) return 1;; esac; }
info() { :; }
die() { echo "$*" >&2; exit 1; }
default_elastos_data_dir() { echo unused; }
build_supported_direct_assets() { printf '%s|%s|%s|%s' "$@"; }
''' + platform_setup + native_call + '''
printf '%s\\n' "$HOST_PLATFORM_DIRECT_ASSETS" "${GUEST_RUST_TARGET:-${HOST_RUST_TARGET}}"
'''],
                    env={**os.environ, "TEST_SYSTEM": system, "TEST_MACHINE": machine},
                    check=True,
                    capture_output=True,
                    text=True,
                )
                self.assertEqual(result.stdout.splitlines(), [
                    f"{release_platform}|{setup_platform}|{native_target}|false",
                    guest_target,
                ])

    def test_host_and_cross_manifests_keep_their_platform_names(self):
        source = PUBLISHER.read_text()
        start = source.index("# Persist stamped install.sh and metadata")
        end = source.index('info "Installer CID:', start)
        export = source[start:end]
        for host, cross in (
            ("x86_64-linux", "aarch64-linux"),
            ("aarch64-darwin", "aarch64-linux"),
            ("aarch64-linux", ""),
        ):
            with self.subTest(host=host, cross=cross), tempfile.TemporaryDirectory() as root:
                root = Path(root)
                inputs = root / "inputs"
                inputs.mkdir()
                for name in ("install.sh", "release.json", "release-head.json"):
                    (inputs / name).write_text(name)
                prepared = inputs / "publication-artifacts"
                prepared.mkdir()
                (prepared / f"components-{host}.json").write_text(f"host:{host}")
                if cross:
                    (prepared / f"components-{cross}.json").write_text(f"cross:{cross}")
                    # Prepare both names so this test detects the output naming
                    # defect independently of the temporary input naming change.
                    for name in (cross, cross.split("-")[0]):
                        (inputs / f"components-{name}.json").write_text(f"cross:{cross}")
                subprocess.run(
                    ["bash", "-euc", export + "\n:"],
                    cwd=root,
                    env={
                        **os.environ,
                        "TMPDIR": str(inputs),
                        "PREPARED_ARTIFACTS_DIR": str(prepared),
                        "STAMPED_INSTALL": str(inputs / "install.sh"),
                        "PLATFORM": host,
                        "CROSS_PLATFORM": cross,
                        "CROSS_ARCH": cross.split("-")[0],
                    },
                    check=True,
                    capture_output=True,
                    text=True,
                )
                outputs = root / "artifacts"
                expected = {"install.sh", "release.json", "release-head.json", f"components-{host}.json"}
                if cross:
                    expected.add(f"components-{cross}.json")
                self.assertEqual({p.name for p in outputs.iterdir()}, expected)
                self.assertEqual((outputs / f"components-{host}.json").read_text(), f"host:{host}")
                if cross:
                    self.assertEqual((outputs / f"components-{cross}.json").read_text(), f"cross:{cross}")

    def run_publication_export(self, prepared, publisher_root, stubs=""):
        """Export a prepared set into a disposable Publisher root with real commands stubbed out."""
        with tempfile.TemporaryDirectory() as commands:
            commands = Path(commands)
            effects = commands / "effects"
            for command in PUBLICATION_COMMANDS:
                stub = commands / command
                stub.write_text('#!/bin/sh\nprintf "%s\\n" "$0" >> "$TEST_EFFECT_LOG"\nexit 93\n')
                stub.chmod(0o755)
            result = subprocess.run(
                ["bash", "-euc", 'source "$1"; shift\n' + stubs + '''
export_release_publication "$1" "$2/release-head.json" "$2/release.json" "$2/install.sh" "$2/artifacts"
''', "publication-test", str(PUBLISHER), str(publisher_root), str(prepared)],
                cwd=PUBLISHER.parent.parent,
                env={**os.environ, "PATH": str(commands) + os.pathsep + os.environ["PATH"],
                     "TEST_EFFECT_LOG": str(effects), "LC_ALL": "C"},
                capture_output=True, text=True,
            )
            self.assertFalse(effects.exists(), "publication export invoked a real publication command")
        return result

    def test_publication_export_promotes_exact_advertised_set_head_last(self):
        source = PUBLISHER.read_text()
        start = source.index("# Save release metadata to runtime-owned publisher state")
        call_site = source[start:source.index('info "Saved release artifacts', start)]
        self.assertIn('export_release_publication "$PUBLISHER_ROOT"', call_site)
        self.assertNotIn("cp ", call_site)
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            publisher = root / "data/ElastOS/SystemServices/Publisher"
            previous = publication_fixture(root / "previous", salt=b" previous")
            result = self.run_publication_export(root / "previous", publisher)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(snapshot(publisher), served_view(previous))
            expected = publication_fixture(root / "prepared", salt=b" next")
            moves = root / "moves"
            for attempt in ("publish", "identical retry"):
                with self.subTest(attempt=attempt):
                    moves.unlink(missing_ok=True)
                    result = self.run_publication_export(root / "prepared", publisher,
                        'mv() { printf "%s\\n" "${@: -1}" >> "$TEST_MOVES"; command mv "$@"; }\nexport TEST_MOVES="'
                        + str(moves) + '"\n')
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(result.stderr, "")
                    self.assertEqual(snapshot(publisher), served_view(expected))
                    order = [os.path.relpath(line, publisher) for line in moves.read_text().splitlines()]
                    artifacts = sorted(name for name in expected if name.startswith("artifacts/"))
                    self.assertEqual(order, artifacts + ["install.sh", "release.json", "release-head.json"])

    def test_publication_export_rejects_incomplete_or_mismatched_sets_before_mutation(self):
        def replace(name, data):
            return lambda prepared: (prepared / name).write_bytes(data)

        def corrupt(name):
            def edit(prepared):
                path = prepared / "artifacts" / name
                path.write_bytes(bytes(len(path.read_bytes())))
            return edit

        def grow(name):
            def edit(prepared):
                path = prepared / "artifacts" / name
                path.write_bytes(path.read_bytes() + b"!")
            return edit

        def unknown_platform(release):
            release["payload"]["platforms"]["riscv64-linux"] = release["payload"]["platforms"]["x86_64-linux"]

        def no_platforms(release):
            release["payload"]["platforms"] = {}

        def unsigned(release):
            del release["signature"]

        cases = {
            "missing-app": ((lambda p: (p / "artifacts/home.tar.gz").unlink()), "home.tar.gz"),
            "missing-runtime": ((lambda p: (p / "artifacts/elastos-aarch64-linux").unlink()), "elastos-aarch64-linux"),
            "missing-components": ((lambda p: (p / "artifacts/components-x86_64-linux.json").unlink()),
                                   "components-x86_64-linux.json"),
            "missing-install": ((lambda p: (p / "install.sh").unlink()), "install.sh must be a regular file"),
            "empty-install": (replace("install.sh", b""), "install.sh is empty"),
            "malformed-head": (replace("release-head.json", b"{"), "release-head.json is not valid JSON"),
            "malformed-release": (replace("release.json", b"not json"), "release.json is not valid JSON"),
            "wrong-head-schema": (replace("release-head.json", json.dumps(
                {"payload": {"schema": "elastos.release/v1"}, "signature": "s", "signer_did": "d"}).encode()),
                "release-head.json is not a elastos.release.head/v1 envelope"),
            "unsigned-release": ((lambda p: rewrite_release(p, unsigned)), "release.json is missing its signature"),
            "stale-head-binding": ((lambda p: rebind_head(p, release_sha256="0" * 64)), "does not bind"),
            "head-version": ((lambda p: rebind_head(p, version="0.7.2")), "version differ"),
            "head-channel": ((lambda p: rebind_head(p, channel="canary")), "channel differ"),
            "runtime-hash": (corrupt("elastos-x86_64-linux"), "advertised binary differs from its bytes"),
            "runtime-size": (grow("elastos-aarch64-darwin"), "advertised binary differs from its bytes"),
            "components-hash": (corrupt("components-aarch64-linux.json"), "advertised components differs"),
            "app-hash": (corrupt("home.tar.gz"), "artifact checksum mismatch"),
            "app-size": (grow("home.tar.gz"), "artifact size mismatch"),
            "native-hash": (corrupt("shell-linux-arm64"), "artifact checksum mismatch"),
            "capsule-hash": (corrupt("shell-aarch64-darwin.capsule.tar.gz"), "artifact checksum mismatch"),
            "provider-metadata-size": (grow("shell-capsule-metadata.tar.gz"), "artifact size mismatch"),
            "unadvertised-extra": (replace("artifacts/elastos-riscv64-linux", b"stray"), "not advertised"),
            "unknown-platform": ((lambda p: rewrite_release(p, unknown_platform)), "unknown platform"),
            "no-platforms": ((lambda p: rewrite_release(p, no_platforms)), "advertises no platforms"),
        }
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            publisher = root / "publisher"
            publication_fixture(root / "previous", salt=b" previous")
            self.assertEqual(self.run_publication_export(root / "previous", publisher).returncode, 0)
            before = snapshot(publisher)
            for name, (mutate, message) in cases.items():
                with self.subTest(failure=name):
                    prepared = root / f"prepared-{name}"
                    publication_fixture(prepared, salt=b" next")
                    mutate(prepared)
                    result = self.run_publication_export(prepared, publisher)
                    self.assertNotEqual(result.returncode, 0, name)
                    self.assertIn("Release publication set rejected", result.stderr)
                    self.assertIn(message, result.stderr)
                    self.assertEqual(snapshot(publisher), before)

    def test_publication_export_rejects_unsafe_paths_and_collisions_before_mutation(self):
        def link_artifact(prepared, publisher, outside):
            path = prepared / "artifacts/home.tar.gz"
            outside.write_bytes(path.read_bytes())
            path.unlink()
            path.symlink_to(outside)

        def link_artifact_dir(prepared, publisher, outside):
            (prepared / "artifacts").rename(prepared / "artifacts-real")
            (prepared / "artifacts").symlink_to(prepared / "artifacts-real")

        def link_release(prepared, publisher, outside):
            outside.write_bytes((prepared / "release.json").read_bytes())
            (prepared / "release.json").unlink()
            (prepared / "release.json").symlink_to(outside)

        def nested_dir(prepared, publisher, outside):
            (prepared / "artifacts/nested").mkdir()

        def hidden_file(prepared, publisher, outside):
            (prepared / "artifacts/.hidden").write_bytes(b"hidden")

        def release_dir(prepared, publisher, outside):
            (publisher / "release.json").unlink()
            (publisher / "release.json").mkdir()

        def head_link(prepared, publisher, outside):
            outside.write_bytes(b"outside head")
            (publisher / "release-head.json").unlink()
            (publisher / "release-head.json").symlink_to(outside)

        def artifacts_file(prepared, publisher, outside):
            shutil.rmtree(publisher / "artifacts")
            (publisher / "artifacts").write_bytes(b"not a directory")

        def artifacts_link(prepared, publisher, outside):
            (publisher / "artifacts").rename(publisher / "elsewhere")
            (publisher / "artifacts").symlink_to(publisher / "elsewhere")

        def artifact_dir_collision(prepared, publisher, outside):
            (publisher / "artifacts/home.tar.gz").unlink()
            (publisher / "artifacts/home.tar.gz").mkdir()

        def root_file(prepared, publisher, outside):
            shutil.rmtree(publisher)
            publisher.write_bytes(b"not a directory")

        cases = {
            "symlink-artifact": (link_artifact, "plain regular files"),
            "symlink-artifact-dir": (link_artifact_dir, "artifact directory must be a regular directory"),
            "symlink-release": (link_release, "release.json must be a regular file"),
            "nested-directory": (nested_dir, "plain regular files"),
            "hidden-file": (hidden_file, "plain regular files"),
            "destination-release-dir": (release_dir, "Publisher destination must be a regular file or absent"),
            "destination-head-link": (head_link, "Publisher destination must be a regular file or absent"),
            "destination-artifacts-file": (artifacts_file, "Publisher artifacts path must be a regular directory"),
            "destination-artifacts-link": (artifacts_link, "Publisher artifacts path must be a regular directory"),
            "destination-artifact-dir": (artifact_dir_collision, "Publisher destination must be a regular file or absent"),
            "destination-root-file": (root_file, "Publisher root is not a directory"),
        }
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            for name, (arrange, message) in cases.items():
                with self.subTest(failure=name):
                    publisher = root / f"publisher-{name}"
                    previous = root / f"previous-{name}"
                    publication_fixture(previous, salt=b" previous")
                    self.assertEqual(self.run_publication_export(previous, publisher).returncode, 0)
                    prepared = root / f"prepared-{name}"
                    publication_fixture(prepared, salt=b" next")
                    outside = root / f"outside-{name}"
                    arrange(prepared, publisher, outside)
                    untouched = lambda: {k: v for k, v in snapshot(root).items() if not k.startswith(f"prepared-{name}")}
                    before = untouched()
                    result = self.run_publication_export(prepared, publisher)
                    self.assertNotEqual(result.returncode, 0, name)
                    self.assertIn(message, result.stderr)
                    self.assertEqual(untouched(), before)

    def test_publication_staging_failure_keeps_publication_and_foreign_scratch(self):
        stubs = {
            "copy-error": 'cp() { case "$1" in */release.json) return 93;; esac; command cp "$@"; }\n',
            "short-write": 'cp() { case "$1" in */elastos-aarch64-linux) printf short > "$2";; *) command cp "$@";; esac; }\n',
        }
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            publisher = root / "publisher"
            publication_fixture(root / "previous", salt=b" previous")
            self.assertEqual(self.run_publication_export(root / "previous", publisher).returncode, 0)
            foreign = publisher / ".publish-release.foreign/staged"
            foreign.mkdir(parents=True)
            (foreign / "release.json").write_bytes(b"another attempt")
            before = snapshot(publisher)
            publication_fixture(root / "prepared", salt=b" next")
            for failure, stub in stubs.items():
                with self.subTest(failure=failure):
                    result = self.run_publication_export(root / "prepared", publisher, stub)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("the current publication is unchanged", result.stderr)
                    self.assertIn(".publish-release.foreign", result.stdout)
                    self.assertEqual(snapshot(publisher), before)

    def test_publication_promotion_failure_restores_previous_and_never_advertises_new_head(self):
        boundaries = ("artifacts/components-aarch64-darwin.json", "artifacts/elastos-aarch64-linux",
                      "artifacts/shell-x86_64-linux.capsule.tar.gz", "install.sh", "release.json", "release-head.json")
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            publisher = root / "publisher"
            publication_fixture(root / "previous", salt=b" previous")
            self.assertEqual(self.run_publication_export(root / "previous", publisher).returncode, 0)
            before = snapshot(publisher)
            expected = served_view(publication_fixture(root / "prepared", salt=b" next"))
            for boundary in boundaries:
                with self.subTest(boundary=boundary):
                    result = self.run_publication_export(root / "prepared", publisher,
                        'mv() { case "$2" in */staged/%s) return 93;; esac; command mv "$@"; }\n' % boundary)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("the previous publication was restored", result.stderr)
                    self.assertEqual(snapshot(publisher), before)
            with self.subTest(boundary="hard link of the current file"):
                result = self.run_publication_export(root / "prepared", publisher, 'ln() { return 93; }\n')
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("the previous publication was restored", result.stderr)
                self.assertEqual(snapshot(publisher), before)
            with self.subTest(boundary="restore itself fails"):
                # The attempt keeps its scratch, which still holds the previous bytes it could not put back.
                result = self.run_publication_export(root / "prepared", publisher,
                    'mv() { case "$2" in */staged/release.json|*/previous/artifacts/home.tar.gz) return 93;; esac;'
                    ' command mv "$@"; }\n')
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("could not restore every previous file", result.stderr)
                after = snapshot(publisher)
                scratch = sorted(key for key in after if key.startswith(".publish-release."))
                self.assertTrue(scratch)
                kept = [key for key in scratch if key.endswith("/previous/artifacts/home.tar.gz")]
                self.assertEqual([after[key] for key in kept], [before["artifacts/home.tar.gz"]])
                self.assertEqual({key: value for key, value in after.items() if key not in scratch},
                                 {**before, "artifacts/home.tar.gz": expected["artifacts/home.tar.gz"]})
                for key in scratch[::-1]:
                    path = publisher / key
                    path.rmdir() if path.is_dir() else path.unlink()
                shutil.copyfile(root / "previous/artifacts/home.tar.gz", publisher / "artifacts/home.tar.gz")
                self.assertEqual(snapshot(publisher), before)
            # A first publication has no previous files; a failed promotion removes what it created.
            fresh = root / "fresh"
            result = self.run_publication_export(root / "prepared", fresh,
                'mv() { case "$2" in */staged/release.json) return 93;; esac; command mv "$@"; }\n')
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(snapshot(fresh), {"artifacts": ("dir",)})

    def test_promotion_records_pending_change_before_rename(self):
        # Bookkeeping that fails after a rename must not hide the replaced file
        # from restore; recording first keeps every replaced path restorable.
        pending = 'printf() { if [[ "${2-}" == %s ]]; then return 93; fi; builtin printf "$@"; }\n'
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            publisher = root / "publisher"
            publication_fixture(root / "previous", salt=b" previous")
            self.assertEqual(self.run_publication_export(root / "previous", publisher).returncode, 0)
            before = snapshot(publisher)
            expected = served_view(publication_fixture(root / "prepared", salt=b" next"))
            for relative in ("artifacts/home.tar.gz", "release.json", "release-head.json"):
                with self.subTest(pending_record=relative):
                    result = self.run_publication_export(root / "prepared", publisher, pending % relative)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("the previous publication was restored", result.stderr)
                    self.assertEqual(snapshot(publisher), before)
            with self.subTest(pending_record="restore of an earlier file fails"):
                result = self.run_publication_export(root / "prepared", publisher, pending % "release.json" +
                    'mv() { case "$2" in */previous/artifacts/home.tar.gz) return 93;; esac; command mv "$@"; }\n')
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("could not restore every previous file", result.stderr)
                after = snapshot(publisher)
                scratch = sorted(key for key in after if key.startswith(".publish-release."))
                kept = [key for key in scratch if key.endswith("/previous/artifacts/home.tar.gz")]
                self.assertEqual([after[key] for key in kept], [before["artifacts/home.tar.gz"]])
                self.assertEqual({key: value for key, value in after.items() if key not in scratch},
                                 {**before, "artifacts/home.tar.gz": expected["artifacts/home.tar.gz"]})
                self.assertEqual(after["release-head.json"], before["release-head.json"])

    def test_killed_promotion_leaves_old_head_over_mixed_artifacts_until_retry(self):
        # A shell error rolls back; a killed process does not. Per-file renames are
        # atomic, so the observable state is the promotion order cut at the kill:
        # replaced paths hold complete new bytes, the rest hold the previous bytes,
        # and the previous head stays advertised. A reader holding that head fails
        # closed on replaced artifact checksums until an identical retry completes.
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            publisher = root / "publisher"
            previous = served_view(publication_fixture(root / "previous", salt=b" previous"))
            expected = served_view(publication_fixture(root / "prepared", salt=b" next"))
            order = sorted(name for name in expected if name.startswith("artifacts/")) + [
                "install.sh", "release.json", "release-head.json"]
            for boundary in ("artifacts/elastos-aarch64-linux", "release-head.json"):
                with self.subTest(boundary=boundary):
                    shutil.rmtree(publisher, ignore_errors=True)
                    self.assertEqual(self.run_publication_export(root / "previous", publisher).returncode, 0)
                    result = self.run_publication_export(root / "prepared", publisher,
                        'mv() { case "$2" in */staged/%s) kill -9 $$;; esac; command mv "$@"; }\n' % boundary)
                    self.assertEqual(result.returncode, -9)
                    replaced = set(order[:order.index(boundary)])
                    after = snapshot(publisher)
                    scratch = {key for key in after if key.startswith(".publish-release.")}
                    self.assertTrue(scratch, "the killed attempt leaves its staging behind")
                    self.assertEqual({key: value for key, value in after.items() if key not in scratch},
                                     {key: (expected if key in replaced else previous)[key] for key in previous})
                    self.assertEqual(after["release-head.json"], previous["release-head.json"])
                    result = self.run_publication_export(root / "prepared", publisher)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertIn("interrupted publication attempt remains", result.stdout)
                    after = snapshot(publisher)
                    self.assertEqual({key: value for key, value in after.items() if key not in scratch}, expected)
                    self.assertEqual({key for key in after if key.startswith(".publish-release.")}, scratch)


if __name__ == "__main__":
    unittest.main()
