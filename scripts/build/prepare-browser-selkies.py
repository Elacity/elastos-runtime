#!/usr/bin/env python3
"""Verify pinned Selkies inputs and prepare an offline build tree."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile


VENDOR = Path(__file__).resolve().parents[2] / "third-party/selkies"


def font_name_fields(data):
    """Read copyright and license fields directly from the OpenType name table."""
    names = {0: "copyright", 1: "family", 2: "subfamily", 5: "version",
             13: "license_description", 14: "license_url"}
    fields = {name: [] for name in names.values()}
    for index in range(struct.unpack_from(">H", data, 4)[0]):
        tag, _, offset, length = struct.unpack_from(">4sIII", data, 12 + index * 16)
        if tag != b"name":
            continue
        table = data[offset:offset + length]
        if len(table) != length:
            raise ValueError("Truncated font name table")
        _, count, strings = struct.unpack_from(">HHH", table)
        for record in range(count):
            platform, encoding, _, name, size, start = struct.unpack_from(">6H", table, 6 + record * 12)
            if name not in names:
                continue
            raw = table[strings + start:strings + start + size]
            if len(raw) != size:
                raise ValueError("Truncated font name string")
            if platform == 0 or (platform == 3 and encoding in (0, 1, 10)):
                text = raw.decode("utf-16be")
            elif platform == 1 and encoding == 0:
                text = raw.decode("mac_roman")
            else:
                raise ValueError(f"Unsupported font name encoding: {platform}/{encoding}")
            if text not in fields[names[name]]:
                fields[names[name]].append(text)
        return fields
    raise ValueError("Font name table is missing")


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
    if set(manifest["fonts"]) != {name for name in files if name.endswith(".ttf")}:
        raise ValueError("Selkies font metadata list mismatch")
    for name, record in manifest["fonts"].items():
        if font_name_fields((vendor / name).read_bytes()) != record["name_fields"]:
            raise ValueError(f"Selkies font name metadata mismatch: {name}")
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
