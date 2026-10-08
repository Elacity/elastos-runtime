#!/usr/bin/env python3
"""Identify the guest recipe and reuse only its matching, intact image set."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parent.parent
INPUTS = (
    "scripts/build/build-browser-vm-rootfs.sh",
    "scripts/build/stage-browser-vm-target.sh",
    "scripts/build/prepare-browser-selkies.py",
    "scripts/build/browser-gst-python-smoke.py",
    "scripts/browser-vm-target-preflight.sh",
    "scripts/browser-selkies-control-service.mjs",
    "scripts/browser-vm-vz-transport-bootstrap.mjs",
    "scripts/browser-input-writer-gate.py",
    "third-party/selkies",
    "elastos/tools/browser-native-proxy-engine",
    "elastos/tools/browser-vm-runtime-relay",
    "elastos/tools/browser-vm-guest-control-bridge",
    "rust-toolchain.toml",
)
REQUIRED_FILES = INPUTS[:8] + ("rust-toolchain.toml",) + tuple(
    tool + "/" + name for tool in INPUTS[9:12] for name in ("Cargo.toml", "Cargo.lock", "src/main.rs")
)

DEFAULT_OPTIONS = {"target_platform": "linux-arm64", "rootfs_size": "8192M",
                   "debian_suite": "bookworm", "debian_mirror": "https://deb.debian.org/debian",
                   "cdp_timeout_ms": "20000"}


def identity(root=ROOT, source_ref=None, options=None):
    options = DEFAULT_OPTIONS.copy() if options is None else options
    if (not isinstance(options, dict) or set(options) != set(DEFAULT_OPTIONS)
            or any(not isinstance(value, str) or not value.strip() for value in options.values())
            or not options["cdp_timeout_ms"].isdigit() or int(options["cdp_timeout_ms"]) <= 0):
        raise ValueError("Guest recipe options are incomplete or invalid")
    if options.get("target_platform") != "linux-arm64":
        raise ValueError("The shared Browser guest requires linux-arm64")
    command = (["git", "ls-tree", "-r", "--name-only", source_ref, "--"] if source_ref
               else ["git", "ls-files", "--"])
    paths = subprocess.check_output(command + list(INPUTS), cwd=root, text=True).splitlines()
    records = {}
    for name in sorted(paths):
        if "/tests/" in name or name.endswith(("-test.py", ".test.mjs")):
            continue
        data = (subprocess.check_output(["git", "show", source_ref + ":" + name], cwd=root)
                if source_ref else (root / name).read_bytes())
        records[name] = hashlib.sha256(data).hexdigest()
    if any(name not in records for name in REQUIRED_FILES):
        raise ValueError("Guest recipe inputs are incomplete")
    result = {"schema": "elastos.browser.vm-image-inputs/v1", "options": options, "files": records}
    encoded = json.dumps(result, sort_keys=True, separators=(",", ":")).encode()
    return {**result, "sha256": hashlib.sha256(encoded).hexdigest()}


def reusable(image, inputs):
    receipt = image / "browser-vm-rootfs-manifest.json"
    if not receipt.is_file() or receipt.is_symlink():
        return False
    manifest = json.loads(receipt.read_bytes())
    if manifest.get("inputs_sha256") != inputs["sha256"]:
        return False
    if manifest.get("ok") is not True or manifest.get("target_platform") != "linux-arm64":
        raise ValueError("Matching guest inputs have an invalid build receipt")
    for name, record in (("rootfs.ext4", manifest), ("vmlinux", manifest["kernel"]),
                         ("initrd", manifest["initrd"])):
        path = image / name
        if not path.is_file() or path.is_symlink() or path.stat().st_size != record["size"]:
            raise ValueError("Matching guest image is missing or changed: " + name)
        digest = hashlib.sha256()
        with path.open("rb") as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(chunk)
        if digest.hexdigest() != record["sha256"]:
            raise ValueError("Matching guest image is changed: " + name)
    return True


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-ref")
    parser.add_argument("--target-platform", default="linux-arm64")
    parser.add_argument("--rootfs-size", default="8192M")
    parser.add_argument("--debian-suite", default="bookworm")
    parser.add_argument("--debian-mirror", default="https://deb.debian.org/debian")
    parser.add_argument("--image-dir", type=Path)
    args = parser.parse_args()
    try:
        options = {key: getattr(args, key) for key in (
            "target_platform", "rootfs_size", "debian_suite", "debian_mirror")}
        options["cdp_timeout_ms"] = os.environ.get("ELASTOS_BROWSER_VM_CDP_TIMEOUT_MS", "20000")
        inputs = identity(source_ref=args.source_ref, options=options)
        hit = args.image_dir is not None and reusable(args.image_dir, inputs)
        print(json.dumps(inputs, sort_keys=True))
        if args.image_dir is not None and not hit:
            raise SystemExit(2)
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        parser.exit(1, str(error) + "\n")
