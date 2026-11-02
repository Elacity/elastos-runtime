#!/usr/bin/env python3
"""Check every ELF member in a Linux model engine bundle or release archive."""
import argparse
import importlib.util
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import tarfile
import tempfile


def verify_bundle(root, architecture):
    root = Path(root).resolve(strict=True)
    machine, loader = {"arm64": ("AArch64", "ld-linux-aarch64.so.1"),
                       "amd64": ("Advanced Micro Devices X86-64", "ld-linux-x86-64.so.2")}[architecture]
    limits = {"GLIBC": (2, 35), "GLIBCXX": (3, 4, 30), "CXXABI": (1, 3, 13)}
    host_libraries = {"libc.so.6", "libm.so.6", "libstdc++.so.6", "libgcc_s.so.1", loader}
    env = {key: value for key, value in os.environ.items()
           if key not in ("LD_LIBRARY_PATH", "LD_PRELOAD", "LD_AUDIT")}
    env["LC_ALL"] = "C"
    count = 0
    for path in sorted(root.rglob("*")):
        mode = path.lstat().st_mode
        if stat.S_ISLNK(mode):
            target = Path(os.readlink(path))
            if target.is_absolute() or ".." in target.parts or not path.exists() or not path.resolve().is_relative_to(root):
                raise ValueError(f"unsafe engine symlink: {path.name}")
            continue
        if stat.S_ISDIR(mode):
            continue
        if not stat.S_ISREG(mode) or path.stat().st_nlink != 1:
            raise ValueError(f"unsupported engine file: {path.name}")
        with path.open("rb") as stream:
            if stream.read(4) != b"\x7fELF":
                continue
        count += 1
        def read(*args):
            return subprocess.check_output([*args, str(path)], text=True, env=env,
                                           stderr=subprocess.STDOUT, timeout=30)
        header = read("readelf", "-h")
        if not re.search(r"Class:\s+ELF64", header) or not re.search(r"Machine:\s+" + re.escape(machine) + r"\s*$", header, re.M):
            raise ValueError(f"wrong engine architecture: {path.name}")
        dynamic = read("readelf", "-d")
        if not re.search(r"\(RUNPATH\).*\[\$ORIGIN\]", dynamic):
            raise ValueError(f"engine requires $ORIGIN RUNPATH: {path.name}")
        for library in re.findall(r"\(NEEDED\).*\[(.+?)\]", dynamic):
            if Path(library).name != library or (library not in host_libraries and not (path.parent / library).is_file()):
                raise ValueError(f"unbundled engine dependency: {path.name}: {library}")
        versions = read("readelf", "--version-info")
        if "GLIBC_PRIVATE" in versions:
            raise ValueError(f"private engine symbol baseline: {path.name}")
        for family, number in re.findall(r"\b(GLIBCXX|GLIBC|CXXABI)_([0-9.]+)", versions):
            if tuple(map(int, number.split("."))) > limits[family]:
                raise ValueError(f"engine symbol baseline: {path.name}: {family}_{number}")
        linked = read("ldd")
        if "not found" in linked:
            raise ValueError(f"unresolved engine dependency: {path.name}: {linked.strip()}")
        print(path.relative_to(root), "\n" + dynamic + versions + linked)
    if count < 2 or not (root / "llama-server").is_file():
        raise ValueError("engine and shared libraries required")
    print(f"PASS: {architecture} engine; all {count} ELF files have bundled dependencies and supported symbols")
    return count


def verify_archive(path, root, architecture):
    spec = importlib.util.spec_from_file_location("upstream", Path(__file__).with_name("release-upstream-input.py"))
    upstream = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(upstream)
    members = upstream.archive_members(path, "tar.gz", root, 2 * 1024**3, source_archive=True)
    with tempfile.TemporaryDirectory(prefix="engine-verify-") as temporary:
        destination = Path(temporary)
        upstream.disk_gate(destination, sum(info["size"] for info in members.values()))
        with tarfile.open(path, "r:gz") as archive:
            for name, info in members.items():
                target = destination / name
                target.parent.mkdir(parents=True, exist_ok=True)
                with archive.extractfile(info["source"]) as source, target.open("xb") as output:
                    shutil.copyfileobj(source, output)
                target.chmod(info["mode"])
        return verify_bundle(destination / root, architecture)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", choices=("linux-arm64", "linux-amd64"), required=True)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--bundle", type=Path)
    source.add_argument("--archive", type=Path)
    parser.add_argument("--root", default="llama-b10516")
    args = parser.parse_args()
    for tool in ("readelf", "ldd"):
        if shutil.which(tool) is None:
            parser.error(f"engine verification requires {tool}")
    if args.archive:
        verify_archive(args.archive, args.root, args.platform.removeprefix("linux-"))
    else:
        verify_bundle(args.bundle, args.platform.removeprefix("linux-"))


if __name__ == "__main__":
    main()
