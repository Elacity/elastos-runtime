#!/usr/bin/env bash
# Prepare unsigned native release inputs. Publishing is a separate operation.
set -euo pipefail

CALLER_DIR="$PWD"
SOURCE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$SOURCE_ROOT/scripts/publish-release.sh"

usage() {
    cat <<'EOF'
Usage: scripts/prepare-release-platform.sh --version X.Y.Z --output DIR [--reuse-support M1_DIR]
       [--browser-vm-image PATH --browser-vm-image-sha256 HEX]
       [--browser-vm-image-set DIR]

Build local release inputs on Linux x86_64/ARM64 or macOS ARM64 from a clean
checkout. DIR must be absent. Use a directory outside the checkout, or one
whose temporary sibling is ignored by Git. CARGO_TARGET_DIR selects the native
build cache; otherwise Cargo resolves it. CARGO_BUILD_JOBS defaults to 4 (max 4).
The output contains artifacts/, draft components.json and platform-input.json.
Browser images are operator-built inputs, supplied as a local package and SHA-256.
--browser-vm-image-set packages the verified rootfs/kernel/initrd set with the
release producer. Reuse the existing verified set when its guest inputs match.
Fresh support preparation requires the package for this host platform. It is
verified before Cargo runs and enters the signed release with the support assets.
--reuse-support retains the admitted image. Build and qualify guest images on
the native target outside CI; CI keeps the existing fast source and package tests.
Generic provider microVM rootfs and Browser substrate acceptance are separate.
Fresh Linux ARM64 support requires ELASTOS_LLAMA_ARM64_BUNDLE to name the reviewed b10516
archive. The archive is verified against its build recipe before any build.
Use --reuse-support to build a new Runtime while keeping the exact support,
capsules, catalogue and component template from a qualified native input.
The input must use this platform and a different version. Its bytes and template
are verified before Cargo runs; the output records their original receipt.
EOF
}

VERSION=""
OUTPUT=""
REUSE_SUPPORT=""
BROWSER_VM_IMAGE=""
BROWSER_VM_IMAGE_SHA256=""
BROWSER_VM_IMAGE_SET=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --version|--output|--reuse-support|--browser-vm-image|--browser-vm-image-sha256|--browser-vm-image-set)
            [[ $# -ge 2 && -n "$2" ]] || die "Missing value for $1"
            case "$1" in
                --version) VERSION="$2" ;;
                --output) OUTPUT="$2" ;;
                --reuse-support) REUSE_SUPPORT="$2" ;;
                --browser-vm-image) BROWSER_VM_IMAGE="$2" ;;
                --browser-vm-image-sha256) BROWSER_VM_IMAGE_SHA256="$2" ;;
                --browser-vm-image-set) BROWSER_VM_IMAGE_SET="$2" ;;
            esac
            shift 2
            ;;
        --help|-h) usage; exit 0 ;;
        *) die "Unknown argument: $1" ;;
    esac
done
[[ -n "$VERSION" && -n "$OUTPUT" ]] || { usage >&2; exit 2; }
for tool in git python3 jq cargo rustc rustup tar; do
    command -v "$tool" >/dev/null 2>&1 || die "Required tool not found: $tool"
done
scripts/check-versioning.sh "$VERSION"
OUTPUT=$(python3 - "$CALLER_DIR" "$OUTPUT" <<'PY'
import os, sys
from pathlib import Path
path = Path(os.path.abspath(os.path.join(sys.argv[1], sys.argv[2])))
# Canonicalize existing parent aliases while keeping the final output entry
# intact for the existing-directory and dangling-symlink refusal below.
print(path.parent.resolve() / path.name)
PY
)
if [[ -n "$BROWSER_VM_IMAGE" ]]; then
    BROWSER_VM_IMAGE=$(python3 - "$CALLER_DIR" "$BROWSER_VM_IMAGE" <<'PYTHON'
import os, sys
print(os.path.abspath(os.path.join(sys.argv[1], sys.argv[2])))
PYTHON
)
fi
if [[ -n "$BROWSER_VM_IMAGE_SET" ]]; then
    BROWSER_VM_IMAGE_SET=$(python3 - "$CALLER_DIR" "$BROWSER_VM_IMAGE_SET" <<'PYTHON'
import os, sys
print(os.path.abspath(os.path.join(sys.argv[1], sys.argv[2])))
PYTHON
)
fi
if [[ -n "$REUSE_SUPPORT" ]]; then
    REUSE_SUPPORT=$(python3 - "$CALLER_DIR" "$REUSE_SUPPORT" <<'PY'
import os, sys
print(os.path.abspath(os.path.join(sys.argv[1], sys.argv[2])))
PY
)
fi
[[ ! -e "$OUTPUT" && ! -L "$OUTPUT" ]] || die "Output already exists: $OUTPUT"
SOURCE_COMMIT=$(git rev-parse HEAD)
SOURCE_TREE=$(git rev-parse 'HEAD^{tree}')
[[ -z "$(git status --porcelain --untracked-files=normal)" ]] || die "Source checkout must be clean"

case "$(uname -s):$(uname -m)" in
    Linux:x86_64) PLATFORM=x86_64-linux; SETUP_PLATFORM=linux-amd64; TARGET=x86_64-unknown-linux-musl ;;
    Linux:aarch64|Linux:arm64) PLATFORM=aarch64-linux; SETUP_PLATFORM=linux-arm64; TARGET=aarch64-unknown-linux-musl ;;
    Darwin:arm64|Darwin:aarch64) PLATFORM=aarch64-darwin; SETUP_PLATFORM=darwin-arm64; TARGET=aarch64-apple-darwin ;;
    *) die "Native preparation supports Linux x86_64/ARM64 and macOS ARM64" ;;
esac
if [[ -z "$REUSE_SUPPORT" && "$SETUP_PLATFORM" == linux-arm64 ]]; then
    [[ -n "${ELASTOS_LLAMA_ARM64_BUNDLE:-}" ]] || die "ELASTOS_LLAMA_ARM64_BUNDLE is required for Linux ARM64"
    python3 - "$ELASTOS_LLAMA_ARM64_BUNDLE" <<'PY'
import hashlib, json, pathlib, sys
source = pathlib.Path(sys.argv[1])
recipes = json.loads(pathlib.Path("scripts/release-upstream-recipes.json").read_text())["recipes"]
info = next(item["source"] for item in recipes if item["component"] == "llama-server" and item["platform"] == "linux-arm64")
if not source.is_file() or source.is_symlink() or source.stat().st_size > info["max_bytes"]:
    raise SystemExit("ARM64 llama-server bundle is missing or has the wrong size")
digest = hashlib.sha256(source.read_bytes()).hexdigest()
if "sha256:" + digest != info["checksum"]:
    raise SystemExit("ARM64 llama-server bundle checksum differs from components.json")
PY
fi
if [[ -n "$REUSE_SUPPORT" && ( -n "$BROWSER_VM_IMAGE" || -n "$BROWSER_VM_IMAGE_SHA256" || -n "$BROWSER_VM_IMAGE_SET" ) ]]; then
    die "--reuse-support retains its image; supply Browser image inputs only for fresh support"
fi
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
case "$CARGO_BUILD_JOBS" in 1|2|3|4) ;; *) die "CARGO_BUILD_JOBS must be between 1 and 4" ;; esac

umask 077
mkdir -p "$(dirname "$OUTPUT")"
WORK_DIR=$(mktemp -d "$(dirname "$OUTPUT")/.release-platform.XXXXXX")
trap 'rm -rf "$WORK_DIR"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
STAGING="$WORK_DIR/output"
TMPDIR="$WORK_DIR/build"
mkdir -p "$STAGING/artifacts" "$TMPDIR"
[[ -z "$(git status --porcelain --untracked-files=normal)" ]] || die "Temporary output must be outside the checkout or ignored by Git"

# Reuse qualified bytes or resolve native support through the integrity checker.
REUSE_SUPPORT_ARGS=()
if [[ -n "$REUSE_SUPPORT" ]]; then
    python3 scripts/release-platform-input.py copy-support \
        --input "$REUSE_SUPPORT" --root "$STAGING" --platform "$PLATFORM" --version "$VERSION"
    python3 - "$STAGING/support-input.json" "$WORK_DIR/omissions.json" <<'PY'
import json, pathlib, sys
receipt = json.loads(pathlib.Path(sys.argv[1]).read_text())
pathlib.Path(sys.argv[2]).write_text(json.dumps(receipt["omitted_platform_components"]) + "\n")
PY
    SUPPORT_BINARY_ASSETS=()
    REUSE_SUPPORT_ARGS=(--reuse-support)
else
python3 - "$SETUP_PLATFORM" "$WORK_DIR" "${SUPPORT_BINARY_ASSETS[@]}" <<'PY'
import importlib.util, json, pathlib, sys
spec = importlib.util.spec_from_file_location("integrity", "scripts/components-release-integrity-check.py")
integrity = importlib.util.module_from_spec(spec)
spec.loader.exec_module(integrity)
platform, work = sys.argv[1], pathlib.Path(sys.argv[2])
external = json.loads(pathlib.Path("components.json").read_text())["external"]
selected, omitted = [], []
for name in dict.fromkeys(sys.argv[3:]):
    if name not in external:
        raise SystemExit(f"Support asset is absent from components template: {name}")
    _, info = integrity.resolve_platform_info(external[name], platform)
    (selected if info is not None else omitted).append(name)
(work / "native-assets.txt").write_text("".join(name + "\n" for name in selected))
(work / "omissions.json").write_text(json.dumps(omitted) + "\n")
for name in omitted:
    print(f"Platform-absent helper: {name}", file=sys.stderr)
PY
SUPPORT_BINARY_ASSETS=()
while IFS= read -r name; do SUPPORT_BINARY_ASSETS+=("$name"); done < "$WORK_DIR/native-assets.txt"
fi

if [[ -z "$REUSE_SUPPORT" ]]; then
    if [[ -n "$BROWSER_VM_IMAGE_SET" ]]; then
        [[ -z "$BROWSER_VM_IMAGE" && -z "$BROWSER_VM_IMAGE_SHA256" ]] || die "Select an image set or a pinned image package"
        BROWSER_VM_IMAGE="$WORK_DIR/browser-image.tar.gz"
        python3 "$SOURCE_ROOT/scripts/package-browser-vm-image.py" \
            --image-dir "$BROWSER_VM_IMAGE_SET" --platform "$SETUP_PLATFORM" \
            --archive "$BROWSER_VM_IMAGE" --manifest-output "$WORK_DIR/packaged-browser-image.json" \
            --release-path "browser-vm-image-${SETUP_PLATFORM}.tar.gz"
        BROWSER_VM_IMAGE_SHA256=$(python3 - "$BROWSER_VM_IMAGE" <<'PYTHON'
import hashlib, pathlib, sys
with pathlib.Path(sys.argv[1]).open('rb') as stream:
    digest = hashlib.sha256()
    for chunk in iter(lambda: stream.read(1024 * 1024), b''): digest.update(chunk)
print(digest.hexdigest())
PYTHON
)
    fi
    python3 scripts/release-platform-input.py stage-browser-image \
        --root "$STAGING" --platform "$PLATFORM" \
        --package "$BROWSER_VM_IMAGE" \
        --sha256 "$BROWSER_VM_IMAGE_SHA256" --output "$WORK_DIR/browser-image.json"
fi

# locate-project reads workspace ownership without resolving or generating locks.
missing_locks=()
LOCK_COMPONENTS=(elastos)
if [[ -z "$REUSE_SUPPORT" ]]; then
    LOCK_COMPONENTS+=(home-cli ${SUPPORT_BINARY_ASSETS[@]+"${SUPPORT_BINARY_ASSETS[@]}"})
fi
for name in "${LOCK_COMPONENTS[@]}"; do
    if [[ "$name" == elastos ]]; then
        capsule_dir=elastos
    else
        capsule_dir=$(resolve_capsule_dir "$name") || die "Source directory missing: $name"
    fi
    workspace_manifest=$(cargo locate-project --workspace --message-format plain --manifest-path "$capsule_dir/Cargo.toml")
    lockfile=$(python3 - "$workspace_manifest" <<'PY'
import os, sys
print(os.path.relpath(os.path.join(os.path.dirname(sys.argv[1]), "Cargo.lock")))
PY
)
    if [[ ! -f "$lockfile" ]] || ! git cat-file -e "${SOURCE_COMMIT}:${lockfile}" 2>/dev/null; then
        missing_locks+=("$lockfile ($name)")
    fi
done
if [[ ${#missing_locks[@]} -gt 0 ]]; then
    printf 'Missing tracked lockfile prerequisite: %s\n' "${missing_locks[@]}" >&2
    die "Prepare reviewed lockfiles before native release preparation"
fi

if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
    CARGO_TARGET_DIR=$(python3 - "$CALLER_DIR" "$CARGO_TARGET_DIR" <<'PY'
import os, sys
print(os.path.abspath(os.path.join(sys.argv[1], sys.argv[2])))
PY
)
else
    CARGO_TARGET_DIR=$(cd elastos && cargo metadata --locked --offline --no-deps --format-version 1 | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')
fi
[[ "$CARGO_TARGET_DIR" == /* ]] || die "Cargo target directory must resolve to an absolute path"
export CARGO_TARGET_DIR ELASTOS_RELEASE_VERSION="$VERSION"
rustup target list --installed | grep -Fxq "$TARGET" || die "Required Rust target is not installed: $TARGET"
# Native macOS uses the same Cargo cache as normal source-home builds. Linux
# keeps its explicit musl target; the receipt still records the actual target.
BUILD_TARGET="$TARGET"
if [[ "$PLATFORM" == aarch64-darwin ]]; then
    [[ "$(rustc -vV | sed -n 's/^host: //p')" == "$TARGET" ]] || die "Rust host differs from native Mac target"
    BUILD_TARGET=""
fi
BUILD_TARGET_ARGS=()
[[ -z "$BUILD_TARGET" ]] || BUILD_TARGET_ARGS=(--target "$BUILD_TARGET")
RELEASE_PREPARE_LOCKED=true
RELEASE_PREPARE_SOURCE_COMMIT="$SOURCE_COMMIT"
SKIP_BUILD=false
ARTIFACTS_DIR=""
info "Preparing ${PLATFORM} from ${SOURCE_COMMIT} (native cache: ${CARGO_TARGET_DIR})"
(cd elastos && cargo build --locked --release ${BUILD_TARGET_ARGS[@]+"${BUILD_TARGET_ARGS[@]}"} -p elastos-server --bin elastos)
RUNTIME="$CARGO_TARGET_DIR/${BUILD_TARGET:+$BUILD_TARGET/}release/elastos"
[[ -x "$RUNTIME" ]] || die "Built Runtime is missing: $RUNTIME"
if [[ "$PLATFORM" == *-linux ]]; then
    scripts/audit-linux-runtime-portability.sh --platform "$PLATFORM" --binary "$RUNTIME" --label "prepared native Runtime"
fi
assert_runtime_binary_embeds_release_version "$RUNTIME" "$PLATFORM" "$VERSION"
cp "$RUNTIME" "$STAGING/artifacts/elastos-$PLATFORM"
if [[ -z "$REUSE_SUPPORT" ]]; then
NATIVE_ASSETS=$(build_supported_direct_assets "$PLATFORM" "$SETUP_PLATFORM" "$BUILD_TARGET")
APP_ASSETS=$(build_platform_independent_direct_assets "$PLATFORM")
PROVIDER_METADATA=$(build_platform_independent_provider_capsule_metadata_assets)
UPSTREAM_ASSETS=$(build_upstream_direct_assets "$PLATFORM" "$SETUP_PLATFORM")
# Script wrappers are staged only after every selected native producer completes.
# The stager checks source bindings; final admission checks all referenced bytes.
printf '%s\n' "$NATIVE_ASSETS" "$UPSTREAM_ASSETS" | jq -s '.[0] * .[1]' > "$WORK_DIR/browser-native.json"
BROWSER_HOST_ASSETS=$(python3 - "$SOURCE_ROOT" "$SETUP_PLATFORM" "$TMPDIR/supported-assets-$PLATFORM" "$WORK_DIR/browser-native.json" <<'PYTHON'
import importlib.util, json, pathlib, sys
root, platform, output, native_file = pathlib.Path(sys.argv[1]), sys.argv[2], pathlib.Path(sys.argv[3]), pathlib.Path(sys.argv[4])
spec = importlib.util.spec_from_file_location("browser_host", root / "scripts/browser-host-release.py")
module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
template = json.loads((root / "components.json").read_text())
native_assets = json.loads(native_file.read_text())["external"]
native = []
for name in template.get("profiles", {}).get("browser-host", {}).get("components", []):
    info = template["external"][name].get("platforms", {}).get(platform)
    if info is None or "source" in info: continue
    staged = native_assets.get(name, {}).get("platforms", {}).get(platform)
    if not staged or not staged.get("checksum") or not staged.get("size"):
        raise ValueError("Browser native producer did not complete: " + name)
    native.append(name)
print(json.dumps(module.stage(root, platform, output, native)))
PYTHON
)
DIRECT_ASSETS=$(merge_direct_assets "$(merge_direct_assets "$NATIVE_ASSETS" "$APP_ASSETS")" "$PROVIDER_METADATA")
DIRECT_ASSETS=$(merge_direct_assets "$DIRECT_ASSETS" "$UPSTREAM_ASSETS")
DIRECT_ASSETS=$(merge_direct_assets "$DIRECT_ASSETS" "$BROWSER_HOST_ASSETS")
BROWSER_IMAGE_ASSETS=$(cat "$WORK_DIR/browser-image.json")
DIRECT_ASSETS=$(merge_direct_assets "$DIRECT_ASSETS" "$BROWSER_IMAGE_ASSETS")
generate_components_json '{}' "$DIRECT_ASSETS" > "$WORK_DIR/components-merged.json"
printf '%s\n' "$NATIVE_ASSETS" "$APP_ASSETS" "$PROVIDER_METADATA" "$UPSTREAM_ASSETS" "$BROWSER_HOST_ASSETS" "$BROWSER_IMAGE_ASSETS" | jq -s '.' > "$WORK_DIR/direct-assets.json"

# Replace each built descriptor, including universal provider metadata, as a unit.
# A recursive template merge must not retain an old CID or URL for new bytes.
python3 - "$WORK_DIR" "$STAGING/components.json" <<'PY'
import json, pathlib, sys
work = pathlib.Path(sys.argv[1])
draft = json.loads((work / "components-merged.json").read_text())
for direct in json.loads((work / "direct-assets.json").read_text()):
    for name, component in direct["external"].items():
        target = draft["external"][name]
        for key, info in component.get("platforms", {}).items():
            if "release_path" in info and "cid" not in info:
                target["platforms"][key] = info
        metadata = component.get("capsule_metadata", {})
        if metadata:
            target_metadata = target.setdefault("capsule_metadata", {"platforms": {}})
            target_metadata.update({key: value for key, value in metadata.items() if key != "platforms"})
        for key, info in metadata.get("platforms", {}).items():
            if "release_path" in info and "cid" not in info:
                target_metadata.setdefault("platforms", {})[key] = info
pathlib.Path(sys.argv[2]).write_text(json.dumps(draft, indent=2) + "\n")
PY
for asset in "$TMPDIR/supported-assets-$PLATFORM"/* \
    "$TMPDIR/supported-assets-universal"/* \
    "$TMPDIR/supported-provider-contracts-universal"/* \
    "$TMPDIR/supported-upstream-assets-$PLATFORM"/*.tar.gz; do
    [[ -f "$asset" ]] || continue
    [[ ! -L "$asset" ]] || die "Prepared artifact must be a regular owned file: $asset"
    destination="$STAGING/artifacts/$(basename "$asset")"
    [[ ! -e "$destination" && ! -L "$destination" ]] || die "Prepared artifact names must be distinct"
    # Build output and stage share a volume; avoid copying the payloads.
    mv "$asset" "$destination"
done
cp "$TMPDIR/supported-upstream-assets-$PLATFORM/upstream-input.json" "$STAGING/upstream-input.json"
cp scripts/release-upstream-recipes.json "$STAGING/upstream-recipes.json"
if [[ -f "$SOURCE_ROOT/model-catalog.json" ]]; then
    cp "$SOURCE_ROOT/model-catalog.json" "$STAGING/artifacts/model-catalog.json"
fi
fi
python3 scripts/release-platform-input.py record \
    --root "$STAGING" --version "$VERSION" --platform "$PLATFORM" --target "$TARGET" \
    --source-commit "$SOURCE_COMMIT" --source-tree "$SOURCE_TREE" \
    --omissions-json "$WORK_DIR/omissions.json" ${REUSE_SUPPORT_ARGS[@]+"${REUSE_SUPPORT_ARGS[@]}"}
[[ ! -e "$OUTPUT" && ! -L "$OUTPUT" ]] || die "Output appeared during preparation: $OUTPUT"
mv "$STAGING" "$OUTPUT"
info "Prepared local native release inputs: $OUTPUT"
