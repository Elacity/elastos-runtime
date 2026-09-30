#!/usr/bin/env python3
"""Reject release executables that require a newer libc than the Jetson host."""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import tarfile

PACKAGE_ROOT = "elastos-runtime-linux-arm64"
GLIBC_MAX = (2, 35)


def glibc_requirements(blob):
    if len(blob) < 64 or blob[:7] != b"\x7fELF\x02\x01\x01":
        raise ValueError("expected a little-endian ELF64 executable")
    kind, machine = struct.unpack_from("<HH", blob, 16)
    if kind not in (2, 3) or machine != 183:
        raise ValueError("expected an AArch64 executable")
    offset = struct.unpack_from("<Q", blob, 40)[0]
    size, count = struct.unpack_from("<HH", blob, 58)
    if size != 64 or not count or offset + size * count > len(blob):
        raise ValueError("invalid ELF section table")
    sections = [struct.unpack_from("<IIQQQQIIQQ", blob, offset + i * size)
                for i in range(count)]

    def section_data(section):
        start, length = section[4:6]
        if start + length > len(blob):
            raise ValueError("truncated ELF section")
        return blob[start:start + length]

    requirements = set()
    for section in sections:
        if section[1] != 0x6FFFFFFE:  # SHT_GNU_verneed, not strings in code/debug data
            continue
        if section[6] >= count or sections[section[6]][1] != 3:
            raise ValueError("invalid version-needs string table")
        strings = section_data(sections[section[6]])
        raw = section_data(section)
        cursor = 0
        for record in range(section[7]):
            version, entries, _, aux, following = struct.unpack_from("<HHIII", raw, cursor)
            if version != 1 or not entries:
                raise ValueError("invalid version-needs record")
            child = cursor + aux
            for entry in range(entries):
                _, _, _, name, next_aux = struct.unpack_from("<IHHII", raw, child)
                value = strings[name:strings.index(b"\0", name)].decode("ascii")
                if value.startswith("GLIBC_"):
                    requirements.add(value)
                if entry + 1 < entries and next_aux < 16:
                    raise ValueError("invalid version-needs auxiliary chain")
                child += next_aux
            if record + 1 < section[7] and following < 16:
                raise ValueError("invalid version-needs chain")
            cursor += following
    for value in requirements:
        try:
            version = tuple(int(part) for part in value.removeprefix("GLIBC_").split("."))
        except ValueError:
            raise ValueError(f"unsupported libc requirement: {value}") from None
        if version > GLIBC_MAX:
            raise ValueError(f"requires {value}; target supports GLIBC_2.35")
    return sorted(requirements)


def verify_package(path):
    members = {}
    with tarfile.open(path, "r:gz") as archive:
        for item in archive:
            if item.isdir() and item.name.rstrip("/") == PACKAGE_ROOT:
                continue
            parts = item.name.split("/")
            if (len(parts) != 2 or parts[0] != PACKAGE_ROOT or parts[1] in ("", ".", "..")
                    or not item.isfile() or not item.mode & 0o100 or parts[1] in members):
                raise ValueError(f"unexpected package member: {item.name}")
            blob = archive.extractfile(item).read()
            try:
                versions = glibc_requirements(blob)
            except (ValueError, struct.error, UnicodeError) as error:
                raise ValueError(f"{item.name}: {error}") from error
            members[parts[1]] = {"size": len(blob), "sha256": hashlib.sha256(blob).hexdigest(),
                                 "glibc_requirements": versions}
    if not {"elastos", "model-provider"}.issubset(members):
        raise ValueError("package requires Runtime and model-provider executables")
    return {"schema": "elastos.linux-arm64-package/v1", "architecture": "aarch64",
            "glibc_max": "2.35", "package_sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            "package_size": path.stat().st_size, "executables": members}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package", type=Path)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--source-tree", required=True)
    parser.add_argument("--receipt", required=True, type=Path)
    args = parser.parse_args()
    try:
        receipt = verify_package(args.package)
    except (OSError, ValueError, tarfile.TarError) as error:
        parser.exit(1, f"ARM64 package rejected: {error}\n")
    receipt.update(source_commit=args.source_commit, source_tree=args.source_tree)
    args.receipt.write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")
    print(f"Verified {len(receipt['executables'])} AArch64 executables against glibc 2.35")


if __name__ == "__main__":
    main()
