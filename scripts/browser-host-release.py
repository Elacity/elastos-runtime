#!/usr/bin/env python3
"""Stage first-party Browser scripts as checksum-bound release support files.

Pass native components explicitly only after their native producers staged them.
The handoff cannot replace a script source declared by the selected role.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat

PLATFORMS = ("darwin-arm64", "linux-arm64", "linux-amd64")


def filename(value):
    return isinstance(value, str) and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", value) is not None


def helper_path(value, parents):
    if not isinstance(value, str):
        return False
    parts = value.split("/")
    return len(parts) == 2 and parts[0] in parents and filename(parts[1])


def script_sources(template, platform, native_components=()):
    if platform not in PLATFORMS:
        raise ValueError("Unsupported Browser helper platform")
    profiles = template.get("profiles") if isinstance(template, dict) else None
    profile = profiles.get("browser-host") if isinstance(profiles, dict) else None
    names = profile.get("components") if isinstance(profile, dict) else None
    if (not isinstance(names, list) or not names or not all(filename(name) for name in names)
            or len(set(names)) != len(names)):
        raise ValueError("Browser host role requires unique component names")
    if (not isinstance(native_components, (tuple, list))
            or not all(filename(name) for name in native_components)
            or len(set(native_components)) != len(native_components)
            or not set(native_components) <= set(names)):
        raise ValueError("Native handoff requires unique selected Browser host components")
    native_components = set(native_components)
    external = template.get("external")
    if not isinstance(external, dict):
        raise ValueError("Browser host role requires external components")
    result = {}
    outputs, installs = set(), set()
    for name in names:
        component = external.get(name)
        platforms = component.get("platforms") if isinstance(component, dict) else None
        if not isinstance(platforms, dict):
            raise ValueError("Browser host component requires platform metadata: " + name)
        if platform not in platforms:
            if name in native_components:
                raise ValueError("Native Browser handoff is unavailable for " + platform + ": " + name)
            continue
        info = platforms[platform]
        if not isinstance(info, dict):
            raise ValueError("Browser helper platform metadata must be an object")
        if "source" not in info:
            if name not in native_components:
                raise ValueError("Browser component requires a script source or explicit native handoff: " + name)
            continue
        if name in native_components:
            raise ValueError("Native Browser handoff cannot replace a script source: " + name)
        source = info["source"]
        if not helper_path(source, ("scripts",)):
            raise ValueError("Browser script source must be a file in scripts/")
        install = info.get("install_path", component.get("install_path", ""))
        if not helper_path(install, ("bin", "scripts")):
            raise ValueError("Browser script install path must be in bin/ or scripts/")
        release = info.get("release_path")
        if not filename(release):
            raise ValueError("Browser helper release path must be a filename")
        if release in outputs:
            raise ValueError("Duplicate Browser helper release path: " + release)
        if install in installs:
            raise ValueError("Duplicate Browser helper install path: " + install)
        outputs.add(release)
        installs.add(install)
        result[name] = source
    if not result and not native_components:
        raise ValueError("Browser host role has no script helpers for " + platform)
    return result


def stage(root, platform, output, native_components=()):
    template = json.loads((root / "components.json").read_text())
    sources = script_sources(template, platform, native_components)
    if output.is_symlink() or (output.exists() and not output.is_dir()):
        raise ValueError("Browser helper output must be a directory")
    if (root / "scripts").is_symlink():
        raise ValueError("Browser helper source directory must be a real directory")
    prepared = []
    external = {}
    # Admit every source and destination before publishing the first helper.
    for name, source in sources.items():
        component = template["external"][name]
        info = component["platforms"][platform]
        release = info["release_path"]
        destination = output / release
        if destination.exists() or destination.is_symlink():
            raise ValueError("Browser helper output already exists: " + release)
        original = root / source
        try:
            with os.fdopen(os.open(original, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK), "rb") as source_file:
                if not stat.S_ISREG(os.fstat(source_file.fileno()).st_mode):
                    raise ValueError("Browser helper source must be a regular file")
                data = source_file.read()
        except OSError as error:
            raise ValueError("Browser helper source must be a regular file: " + source) from error
        prepared.append((destination, data))
        external[name] = {"platforms": {platform: {
            "release_path": release, "install_path": info.get("install_path", component.get("install_path")),
            "checksum": "sha256:" + hashlib.sha256(data).hexdigest(), "size": len(data),
        }}}
    output.mkdir(parents=True, exist_ok=True)
    created = []
    try:
        for destination, data in prepared:
            with destination.open("xb") as target:
                identity = os.fstat(target.fileno())
                created.append((destination, identity.st_dev, identity.st_ino))
                target.write(data)
                os.fchmod(target.fileno(), 0o755)
    except BaseException:
        for destination, device, inode in created:
            try:
                current = destination.lstat()
            except FileNotFoundError:
                continue
            if stat.S_ISREG(current.st_mode) and (current.st_dev, current.st_ino) == (device, inode):
                destination.unlink()
        raise
    return {"external": external}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parent.parent)
    parser.add_argument("--platform", choices=PLATFORMS, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--native-component", action="append", default=[],
                        help="Selected component already staged by its native producer (repeatable)")
    args = parser.parse_args()
    print(json.dumps(stage(args.root, args.platform, args.output, args.native_component)))
