#!/usr/bin/env python3
"""Validate frozen Debian inputs and Chromium archives before guest installation."""
import argparse
import hashlib
import json
from pathlib import Path
import re

LOCK = Path(__file__).with_name("browser-vm-debian-lock.json")
PYTHON_LOCK = Path(__file__).with_name("browser-vm-python-lock.json")


def load_lock(path=LOCK):
    lock = json.loads(path.read_text())
    if (lock.get("schema") != "elastos.browser.debian-lock/v1"
            or lock.get("architecture") != "arm64" or lock.get("suite") != "bookworm"
            or any(not re.fullmatch(r"\d{8}T\d{6}Z", lock.get(key, ""))
                   for key in ("debian_snapshot", "security_snapshot"))):
        raise ValueError("Invalid Browser Debian snapshot lock")
    packages = lock.get("packages", [])
    if {p["name"] for p in packages} != {"chromium", "chromium-common", "chromium-sandbox"} or len(packages) != 3:
        raise ValueError("Browser Chromium package set is incomplete")
    for package in packages:
        expected = f'pool/updates/main/c/chromium/{package["name"]}_{package["version"]}_arm64.deb'
        if (package["filename"] != expected or not re.fullmatch(r"[0-9A-Za-z.+~:-]+", package["version"])
                or not re.fullmatch(r"[0-9a-f]{64}", package["sha256"])
                or not isinstance(package["size"], int) or package["size"] <= 0):
            raise ValueError("Invalid Browser Chromium package record")
    return lock


def mirror(lock):
    return f'https://snapshot.debian.org/archive/debian/{lock["debian_snapshot"]}/'


def verify_cache(directory, lock):
    for package in lock["packages"]:
        archive = directory / Path(package["filename"]).name
        if archive.is_symlink() or not archive.is_file() or archive.stat().st_size != package["size"]:
            raise ValueError(f'Chromium archive size or identity differs: {package["name"]}')
        digest = hashlib.sha256()
        with archive.open("rb") as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(chunk)
        if digest.hexdigest() != package["sha256"]:
            raise ValueError(f'Chromium archive checksum differs: {package["name"]}')


def python_requirements(path=PYTHON_LOCK):
    lock = json.loads(path.read_text())
    packages = lock.get("packages", [])
    required = {"websockets", "basicauth", "gputil", "prometheus-client", "msgpack", "pynput", "psutil", "watchdog", "pillow", "python-xlib", "six", "evdev"}
    if (lock.get("schema") != "elastos.browser.python-lock/v1" or lock.get("python") != "3.11"
            or {p["name"].lower().replace("_", "-") for p in packages} != required or len(packages) != len(required)):
        raise ValueError("Browser Python dependency closure is incomplete")
    result = []
    for package in packages:
        hashes = sorted({f["sha256"] for f in package["files"]})
        if (not re.fullmatch(r"[A-Za-z0-9_-]+", package["name"])
                or not re.fullmatch(r"[0-9]+(?:\.[0-9]+)+", package["version"])
                or not hashes or any(not re.fullmatch(r"[0-9a-f]{64}", h) for h in hashes)):
            raise ValueError("Browser Python package pins are invalid")
        result.append(package["name"] + "==" + package["version"] + " " + " ".join("--hash=sha256:" + h for h in hashes))
    return "\n".join(result)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("validate", "sources", "packages", "verify-cache", "python-requirements"))
    parser.add_argument("values", nargs="*")
    args = parser.parse_args()
    try:
        lock = load_lock()
        if args.action == "validate":
            if args.values != [lock["suite"], mirror(lock)]:
                raise ValueError("Guest suite and mirror must match the committed Debian snapshot lock")
            python_requirements()
        elif args.action == "sources":
            print(f'deb [check-valid-until=no] {mirror(lock)} {lock["suite"]} main')
            print(f'deb [check-valid-until=no] https://snapshot.debian.org/archive/debian-security/{lock["security_snapshot"]}/ {lock["suite"]}-security main')
        elif args.action == "packages":
            print(" ".join(f'{p["name"]}={p["version"]}' for p in lock["packages"]))
        elif args.action == "python-requirements":
            print(python_requirements())
        elif len(args.values) == 1:
            verify_cache(Path(args.values[0]), lock)
        else:
            raise ValueError("verify-cache requires one archive directory")
    except (OSError, ValueError, KeyError, TypeError) as error:
        parser.exit(1, str(error) + "\n")


if __name__ == "__main__":
    main()
