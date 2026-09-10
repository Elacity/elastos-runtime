#!/usr/bin/env python3
"""Record and verify unsigned native build inputs. This tool never publishes."""

import argparse
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
from pathlib import Path, PurePosixPath
import platform as host_platform
import posixpath
import re
import stat
import struct
import subprocess
import sys
import tarfile


SCRIPT_ROOT = Path(__file__).resolve().parent
SOURCE_ROOT = SCRIPT_ROOT.parent
SCHEMA = "elastos.release-platform-input/v1"
PLATFORMS = {
    "x86_64-linux": ("linux-amd64", "x86_64-unknown-linux-musl", 62),
    "aarch64-linux": ("linux-arm64", "aarch64-unknown-linux-musl", 183),
    "aarch64-darwin": ("darwin-arm64", "aarch64-apple-darwin", 0x100000C),
}
spec = importlib.util.spec_from_file_location(
    "component_integrity", SCRIPT_ROOT / "components-release-integrity-check.py")
integrity = importlib.util.module_from_spec(spec)
spec.loader.exec_module(integrity)


def run(*args):
    return subprocess.check_output(args, cwd=SOURCE_ROOT, text=True).strip()


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def regular_file(root, relative):
    if (not isinstance(relative, str) or "\\" in relative
            or any(part in {"", ".", ".."} for part in relative.split("/"))):
        raise ValueError(f"unsafe artifact path: {relative!r}")
    path = root
    for part in relative.split("/"):
        path = path / part
        if path.is_symlink():
            raise ValueError(f"symlink in artifact path: {relative}")
    if not stat.S_ISREG(path.stat().st_mode):
        raise ValueError(f"artifact is not a regular file: {relative}")
    return path


def file_record(path):
    return {"sha256": digest(path), "size": path.stat().st_size,
            "executable": bool(path.stat().st_mode & 0o111)}


def check_version(version):
    if not isinstance(version, str) or not version:
        raise ValueError("missing release version")
    result = subprocess.run(["bash", str(SCRIPT_ROOT / "check-versioning.sh"), version],
                            capture_output=True, text=True)
    if result.returncode:
        raise ValueError(result.stderr.strip())


def check_binary(path, platform):
    with path.open("rb") as source:
        header = source.read(64)
    machine = PLATFORMS[platform][2]
    if platform.endswith("-linux"):
        valid = (len(header) == 64 and header[:7] == b"\x7fELF\x02\x01\x01"
                 and struct.unpack_from("<H", header, 16)[0] in (2, 3)
                 and struct.unpack_from("<H", header, 18)[0] == machine)
    else:
        valid = (len(header) >= 32 and header[:4] == b"\xcf\xfa\xed\xfe"
                 and struct.unpack_from("<I", header, 4)[0] == machine
                 and struct.unpack_from("<I", header, 12)[0] == 2)
    if not valid or not path.stat().st_mode & 0o111:
        raise ValueError(f"{path.name}: expected executable for {platform}")


def check_archive(path, extract_path=None, provider=False):
    seen = set()
    regular = set()
    links = set()
    contract = None
    with tarfile.open(path, "r|gz") as archive:
        for entry in archive:
            name = entry.name.rstrip("/")
            if (not name or "\\" in name or PurePosixPath(name).is_absolute()
                    or any(p in {"", ".", ".."} for p in name.split("/"))
                    or name in seen):
                raise ValueError(f"{path.name}: unsafe or duplicate archive member {name!r}")
            if any(str(parent) in links for parent in PurePosixPath(name).parents):
                raise ValueError(f"{path.name}: archive member beneath a link: {name}")
            if entry.issym():
                target = entry.linkname
                resolved = posixpath.normpath(posixpath.join(posixpath.dirname(name), target))
                if ("\\" in target or target.startswith("/") or resolved == ".."
                        or resolved.startswith("../") or resolved.split("/")[0] != name.split("/")[0]):
                    raise ValueError(f"{path.name}: archive link escapes capsule: {name}")
                links.add(name)
                if any(other.startswith(name + "/") for other in seen):
                    raise ValueError(f"{path.name}: link replaces archive parent: {name}")
            elif not (entry.isfile() or entry.isdir()):
                raise ValueError(f"{path.name}: unsupported archive entry type: {name}")
            if entry.isfile():
                regular.add(name)
            if provider and name == f"{extract_path}/capsule.json":
                if not entry.isfile() or entry.size > 1024 * 1024:
                    raise ValueError(f"{path.name}: invalid provider capsule manifest")
                contract = json.load(archive.extractfile(entry))
            seen.add(name)
    if not seen:
        raise ValueError(f"{path.name}: empty app archive")
    if extract_path is not None:
        if (not isinstance(extract_path, str) or not extract_path or "\\" in extract_path
                or any(p in {"", ".", ".."} for p in extract_path.split("/"))
                or extract_path in links
                or not any(n == extract_path or n.startswith(extract_path + "/") for n in seen)):
            raise ValueError(f"{path.name}: extraction root is missing or unsafe: {extract_path}")
    if provider:
        if not isinstance(contract, dict) or contract.get("role") != "provider":
            raise ValueError(f"{path.name}: provider capsule contract is missing")
        if contract.get("name") != extract_path:
            raise ValueError(f"{path.name}: provider capsule name differs from its component")
        icon = contract.get("icon")
        if (not isinstance(icon, str) or not icon or "\\" in icon
                or any(p in {"", ".", ".."} for p in icon.split("/"))):
            raise ValueError(f"{path.name}: invalid provider icon directory")
        for size in integrity.PROVIDER_ICON_SIZES:
            name = f"{extract_path}/{icon}/icon-{size}.png"
            if name not in regular:
                raise ValueError(f"{path.name}: missing provider icon {size}")


def check_contents(root, platform, omissions):
    setup_platform = PLATFORMS[platform][0]
    manifest = json.loads(regular_file(root, "components.json").read_text())
    template = json.loads(regular_file(root, "components-template.json").read_text())
    if manifest.get("schema") != "elastos.components/v1" or manifest.get("capsules") != {}:
        raise ValueError("native preparation requires a v1 manifest without generic VM capsules")
    if manifest.get("profiles") != template.get("profiles"):
        raise ValueError("prepared profiles differ from source template")
    if set(manifest.get("external", {})) != set(template.get("external", {})):
        raise ValueError("prepared component inventory differs from source template")
    if (not isinstance(omissions, list) or any(not isinstance(n, str) for n in omissions)
            or len(set(omissions)) != len(omissions)):
        raise ValueError("platform omissions must be unique component names")
    for name in omissions:
        component = template["external"].get(name)
        if component is None or integrity.resolve_platform_info(component, setup_platform)[1] is not None:
            raise ValueError(f"{name}: omission is not platform-absent in source template")
    errors = integrity.audit_manifest(manifest, [setup_platform])
    errors += integrity.audit_release_artifacts(manifest, [setup_platform], root / "artifacts")
    if errors:
        raise ValueError("; ".join(errors))
    referenced = {f"elastos-{platform}"}
    for name, component in manifest["external"].items():
        contract = lambda value: {k: v for k, v in value.items() if k not in ("platforms", "capsule_metadata")}
        if contract(component) != contract(template["external"][name]):
            raise ValueError(f"{name}: component contract differs from source template")
        original_component = template["external"][name]
        _, original_info = integrity.resolve_platform_info(original_component, setup_platform)
        selected_key, prepared_info = integrity.resolve_platform_info(component, setup_platform)
        if original_info is not None and prepared_info is None:
            raise ValueError(f"{name}: prepared platform is missing")
        if original_info is None and prepared_info is not None:
            raise ValueError(f"{name}: preparation added a platform absent from source")
        for key, value in component.get("platforms", {}).items():
            if key != selected_key and original_component.get("platforms", {}).get(key) != value:
                raise ValueError(f"{name}: unselected platform contract changed: {key}")
        if original_info is not None:
            if original_info.get("release_path") and not prepared_info.get("release_path"):
                raise ValueError(f"{name}: source-local component needs a local artifact")
            if not original_info.get("release_path") and original_info.get("url") and prepared_info != original_info:
                raise ValueError(f"{name}: external dependency differs from pinned source template")
        if original_info is not None and isinstance(component.get("provider_runtime"), dict):
            metadata = component.get("capsule_metadata")
            if not isinstance(metadata, dict) or integrity.resolve_platform_info(metadata, setup_platform)[1] is None:
                raise ValueError(f"{name}: provider capsule metadata is missing")
        entries = [component]
        if isinstance(component.get("capsule_metadata"), dict):
            entries.append(component["capsule_metadata"])
        for entry in entries:
            selected_key, info = integrity.resolve_platform_info(entry, setup_platform)
            is_provider_metadata = entry is not component
            if is_provider_metadata:
                if info is None or not info.get("release_path") or info.get("extract_path") != name:
                    raise ValueError(f"{name}: provider metadata needs a local archive rooted at its capsule name")
                if set(entry.get("platforms", {})) != {selected_key}:
                    raise ValueError(f"{name}: provider metadata includes an unselected platform")
            if info is None or not info.get("release_path"):
                continue
            if any(key in info for key in ("cid", "url", "strategy")):
                raise ValueError(f"{name}: prepared local descriptor contains a transport or build strategy")
            relative = info["release_path"]
            referenced.add(relative)
            path = regular_file(root / "artifacts", relative)
            expected_install = f"capsules/{name}" if entry is not component else None
            if expected_install is None:
                _, original = integrity.resolve_platform_info(template["external"][name], setup_platform)
                expected_install = (original or {}).get("install_path", component.get("install_path"))
            if info.get("install_path", entry.get("install_path")) != expected_install:
                raise ValueError(f"{name}: prepared install path differs from source contract")
            if info.get("install_path", entry.get("install_path", "")).startswith("bin/"):
                check_binary(path, platform)
            elif info.get("extract_path"):
                check_archive(path, info["extract_path"], provider=is_provider_metadata)
            elif expected_install and expected_install.startswith("capsules/"):
                raise ValueError(f"{name}: capsule artifact needs an extraction path")
    check_binary(regular_file(root / "artifacts", f"elastos-{platform}"), platform)
    actual = {str(path.relative_to(root / "artifacts"))
              for path in (root / "artifacts").rglob("*") if not path.is_dir() or path.is_symlink()}
    if actual != referenced:
        raise ValueError(f"artifact inventory mismatch: {sorted(actual ^ referenced)}")
    return manifest, template


def source_identity(commit, tree):
    if not re.fullmatch(r"[0-9a-f]{40}", commit) or not re.fullmatch(r"[0-9a-f]{40}", tree):
        raise ValueError("source commit and tree must be full Git object names")
    if run("git", "rev-parse", "HEAD") != commit or run("git", "rev-parse", "HEAD^{tree}") != tree:
        raise ValueError("source identity changed during preparation")
    if run("git", "status", "--porcelain", "--untracked-files=normal"):
        raise ValueError("source checkout must be clean")
    locks = [p for p in run("git", "ls-files").splitlines() if p.endswith("Cargo.lock")]
    return {"commit": commit, "tree": tree, "clean": True,
            "lockfiles": {p: digest(SOURCE_ROOT / p) for p in locks}}


def record(args):
    root = args.root.resolve()
    if (args.root.is_symlink() or not root.is_dir()
            or (root / "platform-input.json").exists()
            or (root / "platform-input.json").is_symlink()):
        raise ValueError("record requires a new, regular staging directory")
    source = source_identity(args.source_commit, args.source_tree)
    if args.target != PLATFORMS[args.platform][1]:
        raise ValueError("native target does not match platform")
    check_version(args.version)
    template = root / "components-template.json"
    if template.exists() or template.is_symlink():
        raise ValueError("template export already exists")
    template.write_bytes((SOURCE_ROOT / "components.json").read_bytes())
    omissions = json.loads(args.omissions_json.read_text())
    check_contents(root, args.platform, omissions)
    paths = [root / "components.json", template, *sorted((root / "artifacts").rglob("*"))]
    files = {str(p.relative_to(root)): file_record(regular_file(root, str(p.relative_to(root))))
             for p in paths if not p.is_dir() or p.is_symlink()}
    if source_identity(args.source_commit, args.source_tree) != source:
        raise ValueError("source inputs changed during preparation")
    receipt = {"schema": SCHEMA, "created_at": datetime.now(timezone.utc).isoformat(),
               "source": source, "version": args.version, "platform": args.platform,
               "target": args.target, "omitted_platform_components": omissions,
               "tools": {"rustc": run("rustc", "--version"), "cargo": run("cargo", "--version"),
                         "python": host_platform.python_version()},
               "build_command": ["scripts/prepare-release-platform.sh", "--version", args.version,
                                 "--output", "<output>"],
               "files": files,
               "scope": "unsigned native preparation; external prerequisites and installed journeys require acceptance"}
    receipt_path = root / "platform-input.json"
    receipt_path.write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")
    try:
        verify(root)
    except Exception:
        receipt_path.unlink()
        raise
    print(f"Recorded {args.platform}: {len(files)} files from {args.source_commit}")


def verify(root):
    if root.is_symlink() or not root.is_dir():
        raise ValueError("input root must be a regular directory")
    receipt = json.loads(regular_file(root, "platform-input.json").read_text())
    platform = receipt.get("platform")
    if receipt.get("schema") != SCHEMA or platform not in PLATFORMS:
        raise ValueError("unsupported platform input")
    if receipt.get("target") != PLATFORMS[platform][1]:
        raise ValueError("native target does not match platform")
    source = receipt.get("source", {})
    if (source.get("clean") is not True
            or any(not re.fullmatch(r"[0-9a-f]{40}", str(source.get(k, ""))) for k in ("commit", "tree"))):
        raise ValueError("invalid source binding")
    files = receipt.get("files")
    if not isinstance(files, dict) or not files:
        raise ValueError("missing artifact receipt")
    check_version(receipt.get("version"))
    if not isinstance(receipt.get("tools"), dict) or any(
            not isinstance(receipt["tools"].get(tool), str) or not receipt["tools"][tool]
            for tool in ("cargo", "rustc")):
        raise ValueError("missing build tool identity")
    if not isinstance(source.get("lockfiles"), dict) or not source["lockfiles"]:
        raise ValueError("missing lockfile bindings")
    if any(not re.fullmatch(r"[0-9a-f]{64}", str(value)) for value in source["lockfiles"].values()):
        raise ValueError("invalid lockfile binding")
    actual = {str(path.relative_to(root)) for path in root.rglob("*")
              if (not path.is_dir() or path.is_symlink()) and path != root / "platform-input.json"}
    if actual != set(files):
        raise ValueError("input file inventory differs from receipt")
    for relative, expected in files.items():
        if (not isinstance(expected, dict) or type(expected.get("size")) is not int
                or expected["size"] <= 0 or type(expected.get("executable")) is not bool):
            raise ValueError(f"invalid artifact receipt: {relative}")
        if file_record(regular_file(root, relative)) != expected:
            raise ValueError(f"input artifact differs from receipt: {relative}")
    check_contents(root, platform, receipt.get("omitted_platform_components"))
    return receipt


def validate_inputs(values):
    inputs = {}
    for value in values:
        name, sep, path = value.partition("=")
        if not sep or name not in PLATFORMS or name in inputs:
            raise ValueError("inputs require each full release platform exactly once")
        receipt = verify(Path(path))
        if receipt["platform"] != name:
            raise ValueError("input label differs from receipt platform")
        manifest = json.loads((Path(path) / "components.json").read_text())
        for component_name in manifest["profiles"]["home"]["components"]:
            component = manifest["external"].get(component_name)
            if component is None:
                raise ValueError(f"{name}: required Home component is absent: {component_name}")
            _, info = integrity.resolve_platform_info(component, PLATFORMS[name][0])
            if info is None:
                if component_name not in receipt["omitted_platform_components"]:
                    raise ValueError(f"{name}: unrecorded platform omission: {component_name}")
            elif info.get("strategy") in integrity.DEV_STRATEGIES:
                raise ValueError(f"{name}: required Home component needs a distributable artifact: {component_name}")
            elif not any(info.get(key) for key in ("release_path", "url")):
                raise ValueError(f"{name}: required Home component has no prepared delivery path: {component_name}")
        inputs[name] = receipt
    if set(inputs) != set(PLATFORMS):
        raise ValueError("candidate input requires all three platforms")
    first = next(iter(inputs.values()))
    # Admission runs from the reviewed candidate checkout. Agreement between
    # unsigned workers is insufficient to establish the selected source tree.
    expected_source = source_identity(first["source"]["commit"], first["source"]["tree"])
    if first["source"] != expected_source:
        raise ValueError("input source/lockfiles differ from the candidate checkout")
    expected_template = digest(SOURCE_ROOT / "components.json")
    if first["files"]["components-template.json"]["sha256"] != expected_template:
        raise ValueError("input template differs from the candidate checkout")
    for receipt in inputs.values():
        if (receipt["source"] != first["source"] or receipt["version"] != first["version"]
                or receipt["files"]["components-template.json"] != first["files"]["components-template.json"]
                or any(receipt["tools"].get(t) != first["tools"].get(t) for t in ("rustc", "cargo"))):
            raise ValueError("platform source/version/template/toolchain mismatch")
    # Same-named local files are universal assets; native names include platform.
    shared = {}
    for receipt in inputs.values():
        for path, info in receipt["files"].items():
            if path.startswith("artifacts/"):
                if path in shared and shared[path] != info:
                    raise ValueError(f"conflicting universal asset: {path}")
                shared[path] = info
    return inputs


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    create = commands.add_parser("record")
    for name in ("root", "omissions-json"):
        create.add_argument("--" + name, type=Path, required=True)
    for name in ("version", "target", "source-commit", "source-tree"):
        create.add_argument("--" + name, required=True)
    create.add_argument("--platform", choices=PLATFORMS, required=True)
    check = commands.add_parser("verify")
    check.add_argument("root", type=Path)
    combined = commands.add_parser("validate-inputs")
    combined.add_argument("--input", action="append", required=True)
    args = parser.parse_args()
    try:
        if args.command == "record":
            record(args)
        elif args.command == "verify":
            print(json.dumps(verify(args.root), sort_keys=True))
        else:
            receipts = validate_inputs(args.input)
            print(f"Verified source and local bytes for {len(receipts)} platform inputs; publication and installed acceptance remain separate.")
    except (ValueError, OSError, KeyError, TypeError, tarfile.TarError, subprocess.CalledProcessError) as exc:
        parser.exit(1, f"Error: {exc}\n")


if __name__ == "__main__":
    main()
