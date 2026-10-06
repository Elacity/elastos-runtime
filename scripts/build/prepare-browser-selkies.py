#!/usr/bin/env python3
"""Verify pinned Selkies inputs and prepare an offline build tree."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


VENDOR = Path(__file__).resolve().parents[2] / "third-party/selkies"


def verify(vendor):
    manifest = json.loads((vendor / "provenance.json").read_text())
    files = manifest["files"]
    actual = set()
    for path in vendor.rglob("*"):
        if path.is_symlink():
            raise ValueError(f"Selkies symlink is forbidden: {path}")
        if path.is_file() and path != vendor / "provenance.json":
            actual.add(path.relative_to(vendor).as_posix())
    if actual != set(files):
        raise ValueError(f"Selkies file list mismatch: {actual ^ set(files)}")
    for name, record in files.items():
        if hashlib.sha256((vendor / name).read_bytes()).hexdigest() != record["sha256"]:
            raise ValueError(f"Selkies SHA-256 mismatch: {name}")
    return manifest


def prepare(vendor, destination):
    manifest = verify(vendor)
    # Apply the patches to a copy. The pinned upstream bytes stay unchanged.
    shutil.copytree(vendor / "upstream", destination)
    # Build outputs can be inside the host checkout. Keep git apply scoped to
    # this copy instead of discovering the enclosing repository.
    environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    environment["GIT_CEILING_DIRECTORIES"] = str(destination.resolve().parent)
    for name in manifest["patches"]:
        patch = (vendor / name).resolve()
        if name not in manifest["files"]:
            raise ValueError(f"Selkies patch is missing from provenance: {name}")
        subprocess.run(["git", "apply", "--check", str(patch)], cwd=destination, env=environment, check=True)
        subprocess.run(["git", "apply", str(patch)], cwd=destination, env=environment, check=True)
    for name, expected in manifest["patched_files"].items():
        if hashlib.sha256((destination / name).read_bytes()).hexdigest() != expected:
            raise ValueError(f"Selkies patched SHA-256 mismatch: {name}")
    for path in (destination / "src").rglob("*.py"):
        compile(path.read_bytes(), str(path), "exec")
    web = destination / "gst-web"
    shutil.copytree(destination / "addons/gst-web/src", web)
    # The upstream release installer substitutes date +%s. Use the source
    # identity for the same cache substitutions, so builds are reproducible.
    for name in ("app.js", "sw.js", "index.html"):
        path = web / name
        text = path.read_text()
        text = text.replace("CACHE_VERSION", manifest["web_cache_version"])
        if name == "index.html":
            text = text.replace("?ts=1\"", f'?ts={manifest["web_cache_version"]}"')
        path.write_text(text)
    # Ship license texts and source/patch identities with the guest installation.
    shutil.copytree(vendor, destination / "provenance")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--vendor", type=Path, default=VENDOR)
    parser.add_argument("--out-dir", type=Path)
    args = parser.parse_args()
    if args.out_dir is None:
        with tempfile.TemporaryDirectory(prefix="selkies-check-") as scratch:
            prepare(args.vendor, Path(scratch) / "source")
    else:
        prepare(args.vendor, args.out_dir)
    print("Selkies provenance and patches: PASS")


if __name__ == "__main__":
    main()
