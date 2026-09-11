#!/usr/bin/env bash
# Prepare unsigned native release inputs. Publishing is a separate operation.
set -euo pipefail

CALLER_DIR="$PWD"
SOURCE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$SOURCE_ROOT/scripts/publish-release.sh"

usage() {
    cat <<'EOF'
Usage: scripts/prepare-release-platform.sh --version X.Y.Z --output DIR

Build local release inputs on Linux x86_64/ARM64 or macOS ARM64 from a clean
checkout. DIR must be absent. Use a directory outside the checkout, or one
whose temporary sibling is ignored by Git. CARGO_TARGET_DIR selects the native
build cache; otherwise Cargo resolves it. CARGO_BUILD_JOBS defaults to 4 (max 4).
The output contains artifacts/, draft components.json and platform-input.json.
Generic provider microVM rootfs and Browser substrate acceptance are separate.
EOF
}

VERSION=""
OUTPUT=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --version|--output)
            [[ $# -ge 2 && -n "$2" ]] || die "Missing value for $1"
            if [[ "$1" == --version ]]; then VERSION="$2"; else OUTPUT="$2"; fi
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
print(os.path.abspath(os.path.join(sys.argv[1], sys.argv[2])))
PY
)
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

# Resolve exact/alias/* platform support through the existing integrity checker.
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

# locate-project reads workspace ownership without resolving or generating locks.
missing_locks=()
for name in elastos home-cli "${SUPPORT_BINARY_ASSETS[@]}"; do
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
python3 - "$CARGO_TARGET_DIR" "$STAGING" <<'PY'
import pathlib, shutil, sys
for value in sys.argv[1:]:
    path = pathlib.Path(value).resolve()
    while not path.exists():
        path = path.parent
    usage = shutil.disk_usage(path)
    if usage.free * 10 < usage.total:
        raise SystemExit(f"At least 10% free space is required on the volume for {value}")
PY
export CARGO_TARGET_DIR ELASTOS_RELEASE_VERSION="$VERSION"
rustup target list --installed | grep -Fxq "$TARGET" || die "Required Rust target is not installed: $TARGET"
RELEASE_PREPARE_LOCKED=true
RELEASE_PREPARE_SOURCE_COMMIT="$SOURCE_COMMIT"
SKIP_BUILD=false
ARTIFACTS_DIR=""
info "Preparing ${PLATFORM} from ${SOURCE_COMMIT} (native cache: ${CARGO_TARGET_DIR})"
(cd elastos && cargo build --locked --release --target "$TARGET" -p elastos-server --bin elastos)
RUNTIME="$CARGO_TARGET_DIR/$TARGET/release/elastos"
[[ -x "$RUNTIME" ]] || die "Built Runtime is missing: $RUNTIME"
if [[ "$PLATFORM" == *-linux ]]; then
    scripts/audit-linux-runtime-portability.sh --platform "$PLATFORM" --binary "$RUNTIME" --label "prepared native Runtime"
fi
assert_runtime_binary_embeds_release_version "$RUNTIME" "$PLATFORM" "$VERSION"
cp "$RUNTIME" "$STAGING/artifacts/elastos-$PLATFORM"
NATIVE_ASSETS=$(build_supported_direct_assets "$PLATFORM" "$SETUP_PLATFORM" "$TARGET")
APP_ASSETS=$(build_platform_independent_direct_assets "$PLATFORM")
PROVIDER_METADATA=$(build_platform_independent_provider_capsule_metadata_assets)
DIRECT_ASSETS=$(merge_direct_assets "$(merge_direct_assets "$NATIVE_ASSETS" "$APP_ASSETS")" "$PROVIDER_METADATA")
generate_components_json '{}' "$DIRECT_ASSETS" > "$WORK_DIR/components-merged.json"
printf '%s\n' "$NATIVE_ASSETS" "$APP_ASSETS" "$PROVIDER_METADATA" | jq -s '.' > "$WORK_DIR/direct-assets.json"

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
        for key, info in metadata.get("platforms", {}).items():
            if "release_path" in info and "cid" not in info:
                target["capsule_metadata"]["platforms"][key] = info
pathlib.Path(sys.argv[2]).write_text(json.dumps(draft, indent=2) + "\n")
PY
for asset in "$TMPDIR/supported-assets-$PLATFORM"/* \
    "$TMPDIR/supported-assets-universal"/* \
    "$TMPDIR/supported-provider-contracts-universal"/*; do
    [[ -f "$asset" ]] || continue
    cp "$asset" "$STAGING/artifacts/"
done
python3 scripts/release-platform-input.py record \
    --root "$STAGING" --version "$VERSION" --platform "$PLATFORM" --target "$TARGET" \
    --source-commit "$SOURCE_COMMIT" --source-tree "$SOURCE_TREE" \
    --omissions-json "$WORK_DIR/omissions.json"
[[ ! -e "$OUTPUT" && ! -L "$OUTPUT" ]] || die "Output appeared during preparation: $OUTPUT"
mv "$STAGING" "$OUTPUT"
info "Prepared local native release inputs: $OUTPUT"
