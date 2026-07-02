#!/usr/bin/env python3
import argparse
import json
import re
import sys
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


def audit_manifest(data, platforms):
    external = data.get("external") or {}
    errors = []

    if platforms:
        for name, component in sorted(external.items()):
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
        if not isinstance(component, dict):
            continue
        for platform, info in sorted((component.get("platforms") or {}).items()):
            error = checksum_error(name, platform, info)
            if error:
                errors.append(error)
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

    errors = audit_manifest(data, args.platform)
    if errors:
        print(f"[components-integrity] {path} has unstamped release artifacts:", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1

    platform_label = ", ".join(args.platform) if args.platform else "all platforms"
    print(f"[components-integrity] OK: {path} ({platform_label})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
