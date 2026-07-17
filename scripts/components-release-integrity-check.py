#!/usr/bin/env python3
import argparse
import hashlib
import json
import re
import sys
import tempfile
from pathlib import Path


ALIASES = {
    "linux-amd64": ("x86_64-linux",),
    "x86_64-linux": ("linux-amd64",),
    "linux-arm64": ("aarch64-linux",),
    "aarch64-linux": ("linux-arm64",),
    "darwin-arm64": ("macos-arm64",),
    "macos-arm64": ("darwin-arm64",),
}

CHECKSUM_RE = re.compile(r"^(sha256:[0-9a-fA-F]{64}|sha512:[0-9a-fA-F]{128})$")
DEV_STRATEGIES = {"source-build", "local-copy"}
FETCH_FIELDS = ("release_path", "cid", "url")


def non_empty(value):
    return isinstance(value, str) and value.strip() != ""


def requires_checksum(info):
    if not isinstance(info, dict):
        return False
    if info.get("strategy") in DEV_STRATEGIES:
        return False
    return any(non_empty(info.get(field)) for field in FETCH_FIELDS)


def checksum_error(name, platform, info):
    if not requires_checksum(info):
        return None
    checksum = info.get("checksum")
    artifact = next(
        (info.get(field) for field in FETCH_FIELDS if non_empty(info.get(field))),
        "<unknown artifact>",
    )
    if not non_empty(checksum):
        return f"{name} {platform} {artifact}: missing checksum"
    if not CHECKSUM_RE.match(checksum):
        return f"{name} {platform} {artifact}: unsupported checksum format {checksum!r}"
    return None


def resolve_platform_info(component, platform):
    platforms = component.get("platforms") or {}
    keys = (platform, *ALIASES.get(platform, ()), "*")
    for key in keys:
        info = platforms.get(key)
        if isinstance(info, dict):
            return key, info
    return None, None


def selected_profile_components(data, profiles):
    if not profiles:
        return None
    selected = set()
    available = data.get("profiles") or {}
    for profile in profiles:
        entry = available.get(profile)
        if not isinstance(entry, dict):
            raise ValueError(f"profile {profile!r} not found in components manifest")
        components = entry.get("components") or []
        if not isinstance(components, list):
            raise ValueError(f"profile {profile!r} components must be a list")
        selected.update(component for component in components if isinstance(component, str))
    return selected


def stamped_source_home_capsules(data, selected_components):
    stamped = set()
    capsules = data.get("capsules") or {}
    for name, entry in sorted(capsules.items()):
        if selected_components is not None and name not in selected_components:
            continue
        if not isinstance(entry, dict):
            continue
        if entry.get("install_path") != f"capsules/{name}":
            continue
        if not non_empty(entry.get("entrypoint")):
            continue
        if not CHECKSUM_RE.match(str(entry.get("entrypoint_sha256") or "")):
            continue
        stamped.add(name)
    return stamped


def audit_manifest(data, platforms, selected_components=None, source_home_capsules=None):
    external = data.get("external") or {}
    errors = []
    source_home_capsules = source_home_capsules or set()

    if platforms:
        for name, component in sorted(external.items()):
            if selected_components is not None and name not in selected_components:
                continue
            if name in source_home_capsules:
                continue
            if not isinstance(component, dict):
                continue
            for platform in platforms:
                resolved_key, info = resolve_platform_info(component, platform)
                if info is None:
                    continue
                label = platform if resolved_key == platform else f"{platform} via {resolved_key}"
                error = checksum_error(name, label, info)
                if error:
                    errors.append(error)
        return errors

    for name, component in sorted(external.items()):
        if selected_components is not None and name not in selected_components:
            continue
        if name in source_home_capsules:
            continue
        if not isinstance(component, dict):
            continue
        for platform, info in sorted((component.get("platforms") or {}).items()):
            error = checksum_error(name, platform, info)
            if error:
                errors.append(error)
    return errors


def file_sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return "sha256:" + digest.hexdigest()


def browser_assets(root):
    browser_root = root / "browser"
    if not browser_root.is_dir():
        return []
    assets = []
    for path in sorted(browser_root.rglob("*")):
        if not path.is_file():
            continue
        parts = set(path.relative_to(root).parts)
        if parts.intersection({"target", "node_modules", ".git"}) or path.name == ".DS_Store":
            continue
        assets.append(
            {
                "path": path.relative_to(root).as_posix(),
                "sha256": file_sha256(path),
                "size": path.stat().st_size,
            }
        )
    return assets


AUDITED_RUNTIME_ABIS = {"elastos.component/v1", "elastos.runtime-projection/v1"}


def is_audited_source_capsule(manifest):
    if manifest.get("runtime_abi") in AUDITED_RUNTIME_ABIS:
        return True
    return manifest.get("role") == "content" and manifest.get("type") == "data"


def audit_capsule_artifact_metadata(
    data,
    source_root,
    selected_components=None,
    installed_data_dir=None,
    require_selected=False,
):
    if source_root is None:
        return []

    errors = []
    source_root = Path(source_root)
    installed_data_dir = Path(installed_data_dir) if installed_data_dir else None
    capsules = data.get("capsules") or {}
    names = sorted(selected_components) if require_selected and selected_components else sorted(capsules)
    for name in names:
        entry = capsules.get(name)
        capsule_root = source_root / "capsules" / name
        manifest_path = capsule_root / "capsule.json"
        if not manifest_path.is_file():
            continue
        try:
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        except Exception as exc:
            errors.append(f"{name}: failed to read source manifest: {exc}")
            continue
        if not is_audited_source_capsule(manifest):
            continue
        if not isinstance(entry, dict):
            errors.append(f"{name}: installed components manifest missing capsule metadata")
            continue

        metadata_root = capsule_root
        installed_root = None
        installed_manifest = None
        if installed_data_dir is not None:
            installed_root = installed_data_dir / "capsules" / name
            installed_manifest_path = installed_root / "capsule.json"
            if not installed_manifest_path.is_file():
                errors.append(f"{name}: installed capsule manifest missing at {installed_manifest_path}")
                continue
            try:
                installed_manifest = json.loads(
                    installed_manifest_path.read_text(encoding="utf-8")
                )
            except Exception as exc:
                errors.append(f"{name}: failed to read installed manifest: {exc}")
                continue
            metadata_root = installed_root

        expected_component_fields = {
            "install_path": f"capsules/{name}",
            "entrypoint": manifest.get("entrypoint"),
            "runtime_abi": manifest.get("runtime_abi"),
            "execution": manifest.get("execution"),
            "bus_contract": manifest.get("bus_contract"),
            "wit_world_sha256": manifest.get("wit_world_sha256"),
            "projections": manifest.get("projections") or [],
        }
        if manifest.get("role") == "content" and manifest.get("type") == "data":
            expected_component_fields["role"] = "content"
            expected_component_fields["type"] = "data"
            expected_component_fields["viewer"] = manifest.get("viewer")
        expected_manifest_fields = {
            field: expected
            for field, expected in expected_component_fields.items()
            if field != "install_path"
        }
        for field, expected in expected_component_fields.items():
            if entry.get(field) != expected:
                errors.append(f"{name}: capsule metadata {field} mismatch")
        for field, expected in expected_manifest_fields.items():
            actual = installed_manifest.get(field) if installed_manifest is not None else None
            if field == "projections" and actual is None:
                actual = []
            if installed_manifest is not None and actual != expected:
                errors.append(f"{name}: installed manifest {field} mismatch")

        entrypoint = manifest.get("entrypoint")
        if not entrypoint:
            errors.append(f"{name}: source manifest missing entrypoint")
            continue
        entrypoint_path = capsule_root / entrypoint
        if not entrypoint_path.is_file():
            errors.append(f"{name}: source capsule entrypoint missing at {entrypoint_path}")
            continue
        metadata_entrypoint_path = metadata_root / entrypoint
        if not metadata_entrypoint_path.is_file():
            errors.append(f"{name}: installed capsule entrypoint missing at {metadata_entrypoint_path}")
            continue
        source_entrypoint_sha = file_sha256(entrypoint_path)
        entrypoint_sha = file_sha256(metadata_entrypoint_path)
        if source_entrypoint_sha != entrypoint_sha:
            errors.append(f"{name}: source/installed entrypoint mismatch")
        entrypoint_size = metadata_entrypoint_path.stat().st_size
        if entry.get("entrypoint_sha256") != entrypoint_sha:
            errors.append(f"{name}: entrypoint_sha256 mismatch")
        if entry.get("entrypoint_size") != entrypoint_size:
            errors.append(f"{name}: entrypoint_size mismatch")

        source_assets = browser_assets(capsule_root)
        expected_assets = browser_assets(metadata_root)
        if installed_data_dir is not None and source_assets != expected_assets:
            errors.append(f"{name}: source/installed browser_assets mismatch")
        if entry.get("browser_assets", []) != expected_assets:
            errors.append(f"{name}: browser_assets metadata mismatch")

    return errors


def run_self_test():
    good_hash = "a" * 64
    good_hash_512 = "b" * 128
    manifest = {
        "external": {
            "good": {
                "platforms": {
                    "linux-amd64": {
                        "release_path": "good-linux-amd64",
                        "checksum": f"sha256:{good_hash}",
                    },
                    "linux-arm64": {
                        "release_path": "good-linux-arm64",
                        "checksum": f"sha512:{good_hash_512}",
                    },
                }
            },
            "source": {
                "platforms": {
                    "linux-amd64": {
                        "strategy": "source-build",
                        "release_path": "source-linux-amd64",
                    }
                }
            },
            "local": {
                "platforms": {
                    "linux-amd64": {
                        "strategy": "local-copy",
                        "release_path": "local-linux-amd64",
                    }
                }
            },
            "missing-other-platform": {
                "platforms": {
                    "linux-arm64": {"release_path": "missing-linux-arm64"}
                }
            },
            "bad-format": {
                "platforms": {
                    "*": {"release_path": "bad.tar.gz", "checksum": "sha256:not-a-real-hash"}
                }
            },
        }
    }

    current_platform_errors = audit_manifest(manifest, ["linux-amd64"])
    if current_platform_errors != [
        "bad-format linux-amd64 via * bad.tar.gz: unsupported checksum format 'sha256:not-a-real-hash'"
    ]:
        raise AssertionError(current_platform_errors)

    all_errors = audit_manifest(manifest, [])
    expected_all = [
        "bad-format * bad.tar.gz: unsupported checksum format 'sha256:not-a-real-hash'",
        "missing-other-platform linux-arm64 missing-linux-arm64: missing checksum",
    ]
    if all_errors != expected_all:
        raise AssertionError(all_errors)

    clean_manifest = {
        "external": {
            "good": {
                "platforms": {
                    "x86_64-linux": {
                        "cid": "bafy-good",
                        "checksum": f"sha256:{good_hash}",
                    }
                }
            }
        }
    }
    if audit_manifest(clean_manifest, ["linux-amd64"]):
        raise AssertionError("alias-resolved stamped manifest should pass")

    profiled_manifest = {
        "external": {
            "used-provider": {
                "platforms": {"linux-amd64": {"release_path": "used-provider"}}
            },
            "unused-provider": {
                "platforms": {"linux-amd64": {"release_path": "unused-provider"}}
            },
        },
        "profiles": {"home": {"components": ["used-provider"]}},
    }
    selected = selected_profile_components(profiled_manifest, ["home"])
    if selected != {"used-provider"}:
        raise AssertionError(selected)
    if audit_manifest(profiled_manifest, ["linux-amd64"], selected) != [
        "used-provider linux-amd64 used-provider: missing checksum"
    ]:
        raise AssertionError("profile filtering must only check selected components")

    if audit_capsule_artifact_metadata({"capsules": {}}, None):
        raise AssertionError("capsule artifact metadata audit should be opt-in")

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        capsule_root = root / "capsules" / "component-demo"
        (capsule_root / "browser").mkdir(parents=True)
        manifest = {
            "schema": "elastos.capsule/v1",
            "name": "component-demo",
            "version": "0.1.0",
            "description": "Component demo",
            "author": "elastos",
            "role": "app",
            "type": "wasm",
            "runtime_abi": "elastos.component/v1",
            "bus_contract": "elastos:bus@v1",
            "wit_world_sha256": "c" * 64,
            "execution": "component",
            "projections": ["cli", "facts"],
            "entrypoint": "component-demo.component.wasm",
        }
        (capsule_root / "capsule.json").write_text(json.dumps(manifest), encoding="utf-8")
        (capsule_root / "component-demo.component.wasm").write_bytes(b"component")
        (capsule_root / "browser/index.html").write_text("<main></main>", encoding="utf-8")
        good = {
            "capsules": {
                "component-demo": {
                    "cid": "bafy-demo",
                    "sha256": "a" * 64,
                    "size": 123,
                    "platforms": ["linux-amd64"],
                    "install_path": "capsules/component-demo",
                    "entrypoint": "component-demo.component.wasm",
                    "entrypoint_sha256": file_sha256(
                        capsule_root / "component-demo.component.wasm"
                    ),
                    "entrypoint_size": len(b"component"),
                    "runtime_abi": "elastos.component/v1",
                    "execution": "component",
                    "bus_contract": "elastos:bus@v1",
                    "wit_world_sha256": "c" * 64,
                    "projections": ["cli", "facts"],
                    "browser_assets": browser_assets(capsule_root),
                }
            }
        }
        if audit_capsule_artifact_metadata(good, root):
            raise AssertionError("matching capsule artifact metadata should pass")
        source_home_manifest = {
            **good,
            "external": {
                "component-demo": {
                    "platforms": {
                        "*": {
                            "release_path": "component-demo.tar.gz",
                            "extract_path": "component-demo",
                        }
                    }
                }
            },
            "profiles": {"home": {"components": ["component-demo"]}},
        }
        selected = selected_profile_components(source_home_manifest, ["home"])
        source_home_stamped = stamped_source_home_capsules(source_home_manifest, selected)
        if source_home_stamped != {"component-demo"}:
            raise AssertionError(source_home_stamped)
        if audit_manifest(
            source_home_manifest,
            ["linux-amd64"],
            selected,
            source_home_stamped,
        ):
            raise AssertionError("source-home stamped capsule should not require release archive checksum")
        bad = json.loads(json.dumps(good))
        bad["capsules"]["component-demo"]["entrypoint_sha256"] = "sha256:" + "0" * 64
        if audit_capsule_artifact_metadata(bad, root) != [
            "component-demo: entrypoint_sha256 mismatch"
        ]:
            raise AssertionError("capsule entrypoint mismatch should be reported")

        projection_root = root / "capsules" / "projection-demo"
        (projection_root / "browser").mkdir(parents=True)
        projection_manifest = {
            "schema": "elastos.capsule/v1",
            "name": "projection-demo",
            "version": "0.1.0",
            "description": "Projection demo",
            "author": "elastos",
            "role": "app",
            "type": "wasm",
            "runtime_abi": "elastos.runtime-projection/v1",
            "bus_contract": "elastos.runtime-projection/v1",
            "execution": "web-projection",
            "projections": ["web", "facts"],
            "entrypoint": "browser/index.html",
        }
        (projection_root / "capsule.json").write_text(
            json.dumps(projection_manifest), encoding="utf-8"
        )
        (projection_root / "browser/index.html").write_text(
            "<main>projection</main>", encoding="utf-8"
        )
        projection_good = {
            "capsules": {
                "projection-demo": {
                    "cid": "",
                    "sha256": "",
                    "size": 0,
                    "platforms": ["linux-amd64"],
                    "install_path": "capsules/projection-demo",
                    "entrypoint": "browser/index.html",
                    "entrypoint_sha256": file_sha256(
                        projection_root / "browser/index.html"
                    ),
                    "entrypoint_size": len("<main>projection</main>"),
                    "runtime_abi": "elastos.runtime-projection/v1",
                    "execution": "web-projection",
                    "bus_contract": "elastos.runtime-projection/v1",
                    "wit_world_sha256": None,
                    "projections": ["web", "facts"],
                    "browser_assets": browser_assets(projection_root),
                }
            }
        }
        if audit_capsule_artifact_metadata(projection_good, root):
            raise AssertionError("matching projection metadata should pass")

        content_root = root / "capsules" / "gba-demo"
        content_root.mkdir(parents=True)
        content_manifest = {
            "schema": "elastos.capsule/v1",
            "name": "gba-demo",
            "version": "0.1.0",
            "description": "Content demo",
            "author": "elastos",
            "role": "content",
            "type": "data",
            "entrypoint": "demo.gba",
            "viewer": "gba-emulator",
        }
        (content_root / "capsule.json").write_text(
            json.dumps(content_manifest), encoding="utf-8"
        )
        (content_root / "demo.gba").write_bytes(b"rom")
        content_good = {
            "capsules": {
                "gba-demo": {
                    "cid": "",
                    "sha256": "",
                    "size": 0,
                    "platforms": ["linux-amd64"],
                    "install_path": "capsules/gba-demo",
                    "entrypoint": "demo.gba",
                    "entrypoint_sha256": file_sha256(content_root / "demo.gba"),
                    "entrypoint_size": len(b"rom"),
                    "runtime_abi": None,
                    "execution": None,
                    "bus_contract": None,
                    "wit_world_sha256": None,
                    "projections": [],
                    "role": "content",
                    "type": "data",
                    "viewer": "gba-emulator",
                    "browser_assets": [],
                }
            }
        }
        if audit_capsule_artifact_metadata(content_good, root):
            raise AssertionError("matching content/data metadata should pass")


def parse_args(argv):
    parser = argparse.ArgumentParser(
        description="Fail release manifests that fetch artifacts without sha256/sha512 checksums."
    )
    parser.add_argument("--manifest", default="components.json", help="components.json path")
    parser.add_argument(
        "--platform",
        action="append",
        default=[],
        help="Check the effective component entries for this setup platform. May be repeated.",
    )
    parser.add_argument(
        "--source-root",
        help="Repository root used to verify capsule artifact metadata in the manifest.",
    )
    parser.add_argument(
        "--profile",
        action="append",
        default=[],
        help="Only check components selected by this setup profile. May be repeated.",
    )
    parser.add_argument(
        "--source-home-data-dir",
        help="Installed source-home data dir used to verify stamped capsule parity.",
    )
    parser.add_argument("--self-test", action="store_true", help="run built-in checker tests")
    return parser.parse_args(argv)


def main(argv):
    args = parse_args(argv)
    if args.self_test:
        run_self_test()
        print("[components-integrity] self-test OK")
        return 0

    path = Path(args.manifest)
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except Exception as exc:
        print(f"[components-integrity] failed to read {path}: {exc}", file=sys.stderr)
        return 2

    try:
        selected_components = selected_profile_components(data, args.profile)
    except ValueError as exc:
        print(f"[components-integrity] {exc}", file=sys.stderr)
        return 2

    source_home_capsules = set()
    if args.source_home_data_dir:
        source_home_capsules = stamped_source_home_capsules(data, selected_components)

    errors = audit_manifest(
        data,
        args.platform,
        selected_components,
        source_home_capsules,
    )
    errors.extend(
        audit_capsule_artifact_metadata(
            data,
            args.source_root,
            selected_components=selected_components,
            installed_data_dir=args.source_home_data_dir,
            require_selected=bool(args.source_home_data_dir and selected_components),
        )
    )
    if errors:
        print(f"[components-integrity] {path} has integrity errors:", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1

    platform_label = ", ".join(args.platform) if args.platform else "all platforms"
    profile_label = ", ".join(args.profile) if args.profile else "all profiles"
    print(f"[components-integrity] OK: {path} ({platform_label}; {profile_label})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
