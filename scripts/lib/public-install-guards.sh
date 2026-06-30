#!/usr/bin/env bash

guard_branch_binary_requires_checksummed_public_manifest() {
    local manifest_path="$1"
    local label="$2"

    if [[ -z "${ELASTOS_BIN_OVERRIDE:-}" ]]; then
        return 0
    fi
    if [[ ! -f "${manifest_path}" ]]; then
        echo "${label} branch binary override cannot find installed components manifest: ${manifest_path}" >&2
        exit 1
    fi

    MANIFEST_PATH="${manifest_path}" LABEL="${label}" python3 - <<'PY'
import json
import os
import pathlib
import platform
import sys

manifest_path = pathlib.Path(os.environ["MANIFEST_PATH"])
label = os.environ["LABEL"]
machine = platform.machine()
system = platform.system()

if system == "Linux" and machine == "x86_64":
    setup_platform = "linux-amd64"
elif system == "Linux" and machine in {"aarch64", "arm64"}:
    setup_platform = "linux-arm64"
elif system == "Darwin" and machine == "arm64":
    setup_platform = "darwin-arm64"
else:
    print(
        f"{label} branch binary override guard does not know this platform: {system}-{machine}",
        file=sys.stderr,
    )
    sys.exit(1)

manifest = json.loads(manifest_path.read_text())
profiles = manifest.get("profiles") or {}
if "home" not in profiles:
    available = ", ".join(sorted(profiles.keys())[:12]) or "<none>"
    print(
        f"{label} branch binary override requires installer-selected release metadata with the current 'home' setup profile.",
        file=sys.stderr,
    )
    print(
        f"{label} {manifest_path} profiles are not 0.5.0-compatible for this smoke: {available}",
        file=sys.stderr,
    )
    print(
        f"{label} stage or publish a 0.5.0 candidate gateway and rerun with ELASTOS_PUBLISHER_GATEWAY=<url>, or use scripts/local-carrier-setup-smoke.sh for source/local Carrier setup proof.",
        file=sys.stderr,
    )
    sys.exit(1)

missing = []
for name, component in sorted((manifest.get("external") or {}).items()):
    platforms = component.get("platforms") or {}
    info = platforms.get(setup_platform) or platforms.get("*")
    if not isinstance(info, dict):
        continue
    if info.get("strategy") in {"source-build", "local-copy"}:
        continue
    has_release_artifact = any(
        str(info.get(field) or "").strip()
        for field in ("release_path", "cid", "url")
    )
    if not has_release_artifact:
        continue
    checksum = str(info.get("checksum") or "").strip()
    if not (checksum.startswith("sha256:") or checksum.startswith("sha512:")):
        artifact = info.get("release_path") or info.get("cid") or info.get("url") or "<unknown>"
        missing.append(f"{name} ({artifact})")

if missing:
    examples = ", ".join(missing[:8])
    suffix = "" if len(missing) <= 8 else f", ... +{len(missing) - 8} more"
    print(
        f"{label} branch binary override requires checksummed installer-selected release metadata.",
        file=sys.stderr,
    )
    print(
        f"{label} {manifest_path} has {len(missing)} fetched component(s) for {setup_platform} without sha256/sha512 checksums: {examples}{suffix}",
        file=sys.stderr,
    )
    print(
        f"{label} rerun against a staged/published gateway with checksummed artifacts, or use scripts/local-carrier-setup-smoke.sh for source/local Carrier setup proof.",
        file=sys.stderr,
    )
    sys.exit(1)
PY
}
