#!/usr/bin/env python3
import argparse
import hashlib
import json
import os
import re
import shutil
import tempfile
from pathlib import Path


def sha256_file(path):
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return "sha256:" + digest.hexdigest()


def browser_assets(capsule_root):
    browser_root = capsule_root / "browser"
    if not browser_root.is_dir():
        return []
    assets = []
    for path in sorted(browser_root.rglob("*")):
        if not path.is_file():
            continue
        rel = path.relative_to(capsule_root)
        parts = set(rel.parts)
        if parts.intersection({"target", "node_modules", ".git"}) or path.name == ".DS_Store":
            continue
        assets.append(
            {
                "path": rel.as_posix(),
                "sha256": sha256_file(path),
                "size": path.stat().st_size,
            }
        )
    return assets


STAMPS_RUNTIME_ABIS = {"elastos.component/v1", "elastos.runtime-projection/v1"}
MANAGED_STATE_SCHEMA = "elastos.source-home.managed-capsules/v1"
SAFE_CAPSULE_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")


def stamps_source_capsule(manifest):
    if manifest.get("runtime_abi") in STAMPS_RUNTIME_ABIS:
        return True
    return manifest.get("role") == "content" and manifest.get("type") == "data"


def atomic_write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        mode="w", encoding="utf-8", dir=path.parent, delete=False
    ) as handle:
        json.dump(value, handle, indent=2)
        handle.write("\n")
        staged = Path(handle.name)
    os.replace(staged, path)


def atomic_copy_file(source, target):
    target.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode="wb", dir=target.parent, delete=False) as handle:
        handle.write(source.read_bytes())
        staged = Path(handle.name)
    os.replace(staged, target)


def validated_capsule_names(names, label):
    values = sorted(set(names))
    for name in values:
        if not SAFE_CAPSULE_NAME.fullmatch(name):
            raise SystemExit(f"unsafe {label} capsule name: {name!r}")
    return values


def load_managed_capsules(path):
    if not path.is_file():
        return []
    try:
        state = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise SystemExit(f"invalid source-home managed capsule state {path}: {error}")
    if state.get("schema") != MANAGED_STATE_SCHEMA:
        raise SystemExit(f"unsupported source-home managed capsule state: {path}")
    capsules = state.get("capsules")
    if not isinstance(capsules, list) or not all(isinstance(name, str) for name in capsules):
        raise SystemExit(f"invalid source-home managed capsule list: {path}")
    return validated_capsule_names(capsules, "previously managed")


def source_capsule_names(root):
    capsules_root = root / "capsules"
    if not capsules_root.is_dir():
        return []
    return validated_capsule_names(
        [
            path.name
            for path in capsules_root.iterdir()
            if path.is_dir() and (path / "capsule.json").is_file()
        ],
        "first-party source",
    )


def remove_managed_inactive_capsules(
    data_dir, active_capsules, retired_capsules, managed_state_path
):
    active = set(validated_capsule_names(active_capsules, "active"))
    previous = set(load_managed_capsules(managed_state_path))
    retired = set(validated_capsule_names(retired_capsules, "retired"))
    inactive = sorted((previous | retired) - active)
    capsules_root = data_dir / "capsules"
    removed = []
    for name in inactive:
        target = capsules_root / name
        if not target.exists() and not target.is_symlink():
            continue
        if target.is_symlink() or not target.is_dir():
            raise SystemExit(f"refusing to remove unsafe managed capsule path: {target}")
        shutil.rmtree(target)
        removed.append(name)
    return removed


def materialize_provider_contracts(data_dir, root, capsules):
    for name in capsules:
        source_manifest_path = root / "capsules" / name / "capsule.json"
        source_manifest = json.loads(source_manifest_path.read_text(encoding="utf-8"))
        if source_manifest.get("role") != "provider":
            continue
        installed_root = data_dir / "capsules" / name
        if installed_root.is_symlink() or (
            installed_root.exists() and not installed_root.is_dir()
        ):
            raise SystemExit(
                f"refusing to replace unsafe provider contract path: {installed_root}"
            )
        if installed_root.is_dir():
            shutil.rmtree(installed_root)
        installed_root.mkdir(parents=True)
        atomic_copy_file(source_manifest_path, installed_root / "capsule.json")
        copy_declared_icon_set(source_manifest_path.parent, installed_root, source_manifest)


ICON_SIZES = (32, 64, 128, 256)


def copy_declared_icon_set(source_root, installed_root, manifest):
    """A provider owns its icon like an app does; the contract dir carries the
    declared `icon-{32,64,128,256}.png` set so the gateway can serve it."""
    icon_dir = str(manifest.get("icon") or "").strip().strip("/")
    if not icon_dir:
        return
    if ".." in Path(icon_dir).parts:
        raise SystemExit(f"{manifest.get('name')} icon path escapes the capsule: {icon_dir!r}")
    for size in ICON_SIZES:
        source = source_root / icon_dir / f"icon-{size}.png"
        if not source.is_file():
            raise SystemExit(f"{manifest.get('name')} declares icon {icon_dir!r} but {source.name} is missing")
        atomic_copy_file(source, installed_root / icon_dir / f"icon-{size}.png")


def stamp_component_capsules(components_path, data_dir, root, platform, capsules):
    manifest = json.loads(components_path.read_text())
    capsule_entries = manifest.setdefault("capsules", {})

    for name in capsules:
        source_manifest_path = root / "capsules" / name / "capsule.json"
        installed_root = data_dir / "capsules" / name
        installed_manifest_path = installed_root / "capsule.json"
        if not source_manifest_path.is_file() or not installed_manifest_path.is_file():
            continue
        source_manifest = json.loads(source_manifest_path.read_text())
        if not stamps_source_capsule(source_manifest):
            continue
        installed_manifest = json.loads(installed_manifest_path.read_text())
        entrypoint = installed_manifest["entrypoint"]
        built_entrypoint = root / "capsules" / name / entrypoint
        installed_entrypoint = installed_root / entrypoint
        if not built_entrypoint.is_file():
            raise SystemExit(f"{name} source capsule entrypoint missing: {built_entrypoint}")
        if not installed_entrypoint.is_file():
            raise SystemExit(f"{name} installed capsule entrypoint missing: {installed_entrypoint}")
        built_hash = sha256_file(built_entrypoint)
        installed_hash = sha256_file(installed_entrypoint)
        if built_hash != installed_hash:
            raise SystemExit(
                f"{name} built/installed entrypoint mismatch: {built_hash} != {installed_hash}"
            )

        entry = capsule_entries.setdefault(name, {})
        entry.setdefault("cid", "")
        entry.setdefault("sha256", "")
        entry.setdefault("size", 0)
        platforms = set(entry.get("platforms") or [])
        platforms.add(platform)
        entry["platforms"] = sorted(platforms)
        entry["install_path"] = f"capsules/{name}"
        entry["entrypoint"] = entrypoint
        entry["entrypoint_sha256"] = installed_hash
        entry["entrypoint_size"] = installed_entrypoint.stat().st_size
        entry["runtime_abi"] = installed_manifest.get("runtime_abi")
        entry["execution"] = installed_manifest.get("execution")
        entry["bus_contract"] = installed_manifest.get("bus_contract")
        entry["wit_world_sha256"] = installed_manifest.get("wit_world_sha256")
        entry["projections"] = installed_manifest.get("projections") or []
        if installed_manifest.get("role") == "content" and installed_manifest.get("type") == "data":
            entry["role"] = "content"
            entry["type"] = "data"
            entry["viewer"] = installed_manifest.get("viewer")
        entry["browser_assets"] = browser_assets(installed_root)

    atomic_write_json(components_path, manifest)


def finalize_source_home_capsules(
    components_path,
    data_dir,
    root,
    platform,
    capsules,
    retired_capsules,
    managed_state_path,
):
    active = validated_capsule_names(capsules, "active")
    first_party = set(source_capsule_names(root))
    missing = sorted(set(active) - first_party)
    if missing:
        raise SystemExit(
            "active source-home capsule manifests are missing: " + ", ".join(missing)
        )
    retired = sorted(set(retired_capsules) | (first_party - set(active)))
    removed = remove_managed_inactive_capsules(
        data_dir, active, retired, managed_state_path
    )
    materialize_provider_contracts(data_dir, root, active)
    stamp_component_capsules(components_path, data_dir, root, platform, active)
    atomic_write_json(
        managed_state_path,
        {"schema": MANAGED_STATE_SCHEMA, "capsules": active},
    )
    return removed


def parse_args():
    parser = argparse.ArgumentParser(
        description="Finalize installed source-home capsule metadata in components.json."
    )
    parser.add_argument("--components", required=True, type=Path)
    parser.add_argument("--data-dir", required=True, type=Path)
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--platform", required=True)
    parser.add_argument("--capsule", action="append", default=[])
    parser.add_argument("--retired-capsule", action="append", default=[])
    parser.add_argument("--managed-state", type=Path)
    return parser.parse_args()


def main():
    args = parse_args()
    if args.managed_state is None:
        stamp_component_capsules(
            args.components,
            args.data_dir,
            args.root,
            args.platform,
            args.capsule,
        )
        return
    removed = finalize_source_home_capsules(
        components_path=args.components,
        data_dir=args.data_dir,
        root=args.root,
        platform=args.platform,
        capsules=args.capsule,
        retired_capsules=args.retired_capsule,
        managed_state_path=args.managed_state,
    )
    if removed:
        print("[source-home] removed managed inactive capsules: " + ", ".join(removed))


if __name__ == "__main__":
    main()
