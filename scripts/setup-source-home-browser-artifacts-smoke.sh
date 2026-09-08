#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec python3 - "$repo_root" <<'PY'
import copy
import hashlib
import json
import os
from pathlib import Path
import re
import runpy
import shutil
import subprocess
import sys
import tempfile
from unittest import mock

repo = Path(sys.argv[1])
checks = []
# This is an artifact ownership fixture. Real ext4 inspection is covered by
# browser-vm-artifact-preflight-smoke.sh; these bytes are never launched.
contract = {"schema": "elastos.browser.vm-target/v1", "engine": "chromium_microvm",
            "network_mode": "runtime_net_only", "direct_network": False, "wallet_injection": False,
            "media_transport": "runtime_relay", "display_mode": "webrtc_remote_display",
            "guarantee_level": "mechanism_microvm", "display_backend": "vm_selkies_gstreamer_webrtc",
            "runtime_exit_transport": "vsock_relay", "control_transport": "vsock_relay", "control_port": 19092}
required = "manifest init native_proxy runtime_relay guest_control_bridge control_service selkies_start node chromium xvfb python3 gst_inspect".split()
audio = "pipewire pipewire_pulse wireplumber pw_cli".split()
preflight = {"ok": True, "required": {n: {"ok": True} for n in required},
             "optional_audio": {n: {"ok": True} for n in audio}, "audio_default_ready": True,
             "missing": [], "manifest_errors": [], "script_errors": [], "manifest": contract}


def sha(p):
    return hashlib.sha256(p.read_bytes()).hexdigest()


def make_set(source, target="linux-arm64"):
    (source / "browser-vm").mkdir(parents=True)
    (source / "bin").mkdir()
    for rel, contents in [("browser-vm/rootfs.ext4", b"fixture-image"), ("bin/vmlinux", b"fixture-kernel"),
                          ("bin/initrd", b"fixture-initrd"), ("browser-vm/initrd", b"fixture-initrd"),
                          ("bin/crosvm", b"fixture-crosvm")]:
        (source / rel).write_bytes(contents)
    image = source / "browser-vm/rootfs.ext4"
    manifest = {"schema": "elastos.browser.vm-rootfs-build/v1", "ok": True,
                "target_platform": target, "size": image.stat().st_size, "sha256": sha(image),
                "preflight": copy.deepcopy(preflight)}
    for name, rel in [("kernel", "bin/vmlinux"), ("initrd", "bin/initrd")]:
        p = source / rel
        manifest[name] = {"size": p.stat().st_size, "sha256": sha(p)}
    (source / "browser-vm/browser-vm-rootfs-manifest.json").write_text(json.dumps(manifest))
    return manifest


with tempfile.TemporaryDirectory(prefix="browser-image-ownership-") as temp:
    root = Path(temp)
    env = {k:v for k,v in os.environ.items() if not k.startswith("ELASTOS_BROWSER_VM_")}
    env["ELASTOS_DEBUGFS_BIN"] = str(root / "absent-debugfs")
    script = repo / "scripts/setup-source-home-browser-artifacts.sh"

    def run(name, data, source=None, platform="darwin-arm64", ok=True):
        command = [str(script), "--data-dir", str(data), "--platform", platform]
        if source is not None:
            command += ["--artifact-data-dir", str(source)]
        p = subprocess.run(command, env=env, capture_output=True, text=True, timeout=20)
        result = json.loads(p.stdout)
        assert (p.returncode == 0) is ok, (name, result, p.stderr)
        checks.append(name)
        return result

    for platform in ["darwin-arm64", "linux-arm64", "linux-amd64"]:
        source = root / platform
        make_set(source, "linux-amd64" if platform == "linux-amd64" else "linux-arm64")
        before = {str(p.relative_to(source)): sha(p) for p in source.rglob("*") if p.is_file()}
        managed = source / "managed-runtimes/test/home/data"
        result = run(platform + " complete set", managed, platform=platform)
        assert result["image_set_verified"] and result["linked"] == (4 if platform == "darwin-arm64" else 5), result
        for rel in ["browser-vm/rootfs.ext4", "browser-vm/browser-vm-rootfs-manifest.json", "bin/vmlinux",
                    "bin/initrd" if platform == "darwin-arm64" else "browser-vm/initrd"]:
            assert (managed / rel).is_symlink() and (managed / rel).resolve() == (source / rel).resolve(), (platform, rel)
        assert (managed / "bin/crosvm").exists() is platform.startswith("linux-")
        if platform.startswith("linux-"):
            assert not (managed / "bin/initrd").exists(), "Linux setup uses its own initrd path"
        assert run(platform + " repeated setup", managed, platform=platform)["linked"] == 0
        initrd_key = "ELASTOS_BROWSER_VM_INITRAMFS" if platform == "darwin-arm64" else "ELASTOS_BROWSER_VM_INITRD"
        foreign_key = "ELASTOS_BROWSER_VM_INITRD" if platform == "darwin-arm64" else "ELASTOS_BROWSER_VM_INITRAMFS"
        checked = subprocess.run([str(repo / "scripts/browser-vm-artifact-preflight.sh"), "--verify-image-set"],
            env={**env, "ELASTOS_BROWSER_VM_DATA_DIR": str(managed), "ELASTOS_BROWSER_VM_PLATFORM": platform,
                 foreign_key: str(root / "wrong-host-initrd")}, capture_output=True, text=True, timeout=15)
        assert checked.returncode == 0, (platform, checked.stdout, checked.stderr)
        checks.append(platform + " launcher initrd path")

        assert all(sha(source / rel) == digest for rel,digest in before.items()), "shared source changed"

    source = root / "darwin-arm64"
    manifest = source / "browser-vm/browser-vm-rootfs-manifest.json"
    original_manifest = manifest.read_bytes()
    partial = root / "partial"
    manifest.unlink()
    run("missing manifest creates zero links", partial, source, ok=False)
    assert not partial.exists()
    manifest.write_bytes(original_manifest)
    mixed = root / "mixed"
    (mixed / "bin").mkdir(parents=True)
    (mixed / "bin/vmlinux").write_bytes(b"other-kernel")
    run("mismatched existing kernel is preserved", mixed, source, ok=False)
    assert (mixed / "bin/vmlinux").read_bytes() == b"other-kernel"
    assert not (mixed / "browser-vm").exists()
    image = source / "browser-vm/rootfs.ext4"
    old = image.read_bytes()
    image.write_bytes(b"changed-image")
    run("changed image rejected", root / "changed", source, ok=False)
    image.write_bytes(old)
    run("wrong architecture rejected", root / "wrong-arch", source, platform="linux-amd64", ok=False)
    broken = root / "broken"
    (broken / "browser-vm").mkdir(parents=True)
    (broken / "browser-vm/browser-vm-rootfs-manifest.json").symlink_to(root / "absent")
    run("broken destination preserves all files", broken, source, ok=False)
    assert not (broken / "bin").exists()
    assert not (broken / "browser-vm/rootfs.ext4").exists()
    run("missing explicit artifact store is actionable", root / "missing-explicit", root / "absent-store", ok=False)
    unavailable = run("no local image permits a viewer Runtime", root / "viewer")
    assert unavailable["missing"] == 4 and not unavailable["image_set_verified"]
    assert unavailable["image_preparation"] == "required" and "browser-vm-image" in unavailable["detail"]

    # Package only tiny fixture bytes, then inspect the release component overlay.
    # The parent-owned production image store is outside this test's scope.
    package = runpy.run_path(str(repo / "scripts/package-browser-vm-image.py"))["package"]
    package_source = root / "package-source"
    package_source.mkdir()
    for name, rel in [("rootfs.ext4", "browser-vm/rootfs.ext4"),
                      ("browser-vm-rootfs-manifest.json", "browser-vm/browser-vm-rootfs-manifest.json"),
                      ("vmlinux", "bin/vmlinux"), ("initrd", "bin/initrd")]:
        shutil.copyfile(source / rel, package_source / name)
    archive = root / "release/image.tar.gz"
    metadata = root / "release/components.json"
    saved_debugfs = os.environ.get("ELASTOS_DEBUGFS_BIN")
    os.environ["ELASTOS_DEBUGFS_BIN"] = env["ELASTOS_DEBUGFS_BIN"]
    try:
        overlay = package(package_source, "darwin-arm64", archive, metadata, "test/image.tar.gz")
        before_package = archive.read_bytes(), metadata.read_bytes()
        package(package_source, "darwin-arm64", archive, metadata, "test/image.tar.gz")
        assert (archive.read_bytes(), metadata.read_bytes()) == before_package
        platform_info = overlay["external"]["browser-vm-image"]["platforms"]["darwin-arm64"]
        assert platform_info["checksum"] == "sha256:" + sha(archive)
        assert platform_info["size"] == archive.stat().st_size
        assert platform_info["install_path"] == "browser-vm/image-set"
        checks.append("image release archive and manifest are reproducible and hash-bound")
        try:
            package(package_source, "darwin-arm64", archive, metadata, "test/another-release.tar.gz")
            raise AssertionError("existing package pair was overwritten")
        except ValueError as error:
            assert "immutable" in str(error)
        assert (archive.read_bytes(), metadata.read_bytes()) == before_package
        checks.append("new release metadata preserves the immutable previous output pair")

        next_archive, next_metadata = root / "next/image.tar.gz", root / "next/components.json"
        original_link = os.link
        def fail_metadata_link(source, destination, *args, **kwargs):
            if Path(destination) == next_metadata.resolve():
                raise OSError("injected metadata publication failure")
            return original_link(source, destination, *args, **kwargs)
        with mock.patch("os.link", side_effect=fail_metadata_link):
            try:
                package(package_source, "darwin-arm64", next_archive, next_metadata, "test/image.tar.gz")
                raise AssertionError("metadata publication failure was ignored")
            except OSError as error:
                assert "injected" in str(error)
        assert not next_archive.exists() and not next_metadata.exists()
        assert not list(next_archive.parent.glob(".browser-image-*"))
        assert (archive.read_bytes(), metadata.read_bytes()) == before_package
        checks.append("failed metadata publication cleans the new archive and preserves previous outputs")

        original_write = Path.write_bytes
        def fail_metadata_stage(destination, data):
            if destination.name == "components.json" and destination.parent.name.startswith(".browser-image-metadata-"):
                raise OSError("injected metadata staging failure")
            return original_write(destination, data)
        with mock.patch.object(Path, "write_bytes", fail_metadata_stage):
            try:
                package(package_source, "darwin-arm64", next_archive, next_metadata, "test/image.tar.gz")
                raise AssertionError("metadata staging failure was ignored")
            except OSError as error:
                assert "injected" in str(error)
        assert not next_archive.exists() and not next_metadata.exists()
        assert not list(next_archive.parent.glob(".browser-image-*"))
        checks.append("metadata staging failure leaves no published or temporary output")
        for name in ["rootfs.ext4", "vmlinux", "initrd", "browser-vm-rootfs-manifest.json"]:
            artifact = package_source / name
            original = artifact.read_bytes()
            artifact.write_bytes(b"corrupt")
            try:
                package(package_source, "darwin-arm64", archive, metadata, "test/image.tar.gz")
                raise AssertionError("corrupt package source was accepted: " + name)
            except ValueError:
                pass
            assert (archive.read_bytes(), metadata.read_bytes()) == before_package
            artifact.write_bytes(original)
            checks.append("release packaging preserves existing outputs after corrupt " + name)
        for platform, release_path in [("darwin-amd64", "test/image.tar.gz"),
                                       ("linux-amd64", "test/image.tar.gz"),
                                       ("darwin-arm64", "../image.tar.gz")]:
            try:
                package(package_source, platform, archive, metadata, release_path)
                raise AssertionError("invalid release package was accepted")
            except ValueError:
                pass
        checks.append("release packaging rejects wrong architecture and unsafe release paths")
    finally:
        if saved_debugfs is None:
            os.environ.pop("ELASTOS_DEBUGFS_BIN", None)
        else:
            os.environ["ELASTOS_DEBUGFS_BIN"] = saved_debugfs

    # Exercise the actual source-home helper installation. Older setup modified
    # rootfs/initrd here, including a shared store reached through symlinks.
    text = (repo / "scripts/setup-source-home.sh").read_text()
    functions = text.split('echo "[setup-source-home] repo:')[0]
    functions = re.sub(r'^SOURCE_HOME_BINARY_NAMES_JSON=.*\n', '', functions, flags=re.M)
    definitions = root / "setup-functions.sh"
    definitions.write_text(functions)
    installed = source / "managed-runtimes/full-helper-install/data"
    before = {rel: sha(source / rel) for rel in ["browser-vm/rootfs.ext4", "browser-vm/initrd",
                                                "bin/vmlinux", "browser-vm/browser-vm-rootfs-manifest.json"]}
    shell = 'source "$DEFINITIONS"\nROOT="$SOURCE"\nDATA_DIR="$DEST"\nPLATFORM=linux-arm64\nNODE_BIN="$NODE"\ninstall_browser_runtime_helpers\n'
    p = subprocess.run(["bash", "-c", shell], env={**env, "DEFINITIONS": str(definitions),
                       "SOURCE": str(repo), "DEST": str(installed), "NODE": shutil.which("node")},
                       capture_output=True, text=True, timeout=30)
    assert p.returncode == 0, (p.stdout, p.stderr)
    assert all(sha(source / rel) == digest for rel,digest in before.items())
    assert (installed / "browser-vm/browser-vm-rootfs-manifest.json").is_file()
    assert not list(source.glob("**/*.before-*"))
    checks.append("actual helper setup preserves shared image, initrd and receipt")

print(json.dumps({"schema": "elastos.setup-source-home.browser-artifacts-smoke/v1", "ok": True,
                  "checks": checks, "count": len(checks)}))
PY
