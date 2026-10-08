#!/usr/bin/env bash
#
# Prepare unsigned native release inputs for the separate custodian signer.
# Source this file to use its local builders and publication exporter. The
# canonical `elastos publish-release` validates signed inputs before export.
#
# Usage:
#   ./scripts/publish-release.sh --version X.Y.Z --prepare-only DIR \
#       --publisher-did did:key:... --platform-input PLATFORM=DIR [...]
#   ./scripts/publish-release.sh --help
#
# Prerequisites: jq, python3, sha256sum or shasum, ipfs-provider capsule binary.
# Preparation admits native inputs, imports their bytes and writes unsigned
# signing input. The custodian owns signatures; Runtime owns publication.
#

set -euo pipefail

BOLD='\033[1m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
RED='\033[0;31m'
DIM='\033[2m'
NC='\033[0m'

# Default publish scope: runtime core + first-party Home app surface.
# Wallet/Browser surfaces require chain-provider and wallet-provider authority.
# Demo-only capsules such as GBA are published by passing an
# explicit --capsules list or through the Rust `demo` publish profile.
# availability-provider, drm-provider, rights-provider, key-provider,
# decrypt-provider, and tunnel-provider are supported direct command assets
# outside the default managed Home. Runtime-only protected providers are
# support assets selected by the component profile rather than capsule publish.
DEFAULT_CAPSULES=(
    shell
    localhost-provider
    did-provider
    chain-provider
    net-provider
    exit-provider
    browser-engine-adapter
    webspace-provider
    wallet-provider
    object-provider
    content-block-graph-provider
    ipfs-provider
    home-cli
    home-gui
    home
    system
    services
    people
    wallet-metamask
    wallet-unisat
    wallet-walletconnect
    wallet
    browser
    documents
    library
    marketplace
    archive-manager
    inbox
    chat-room
    assistant
    elacity-player
    model-provider
)
CAPSULES=("${DEFAULT_CAPSULES[@]}")
REQUIRED_SUPPORTED_CAPSULES=(
    shell
    localhost-provider
    did-provider
    chain-provider
    net-provider
    exit-provider
    browser-engine-adapter
    webspace-provider
    wallet-provider
    object-provider
    content-block-graph-provider
    ipfs-provider
    home-cli
    home-gui
    home
    system
    services
    people
    wallet-metamask
    wallet-unisat
    wallet-walletconnect
    wallet
    browser
    documents
    library
    marketplace
    archive-manager
    inbox
    chat-room
    assistant
    elacity-player
    model-provider
)
SUPPORT_BINARY_ASSETS=(
    shell
    localhost-provider
    did-provider
    net-provider
    exit-provider
    browser-engine-adapter
    browser-engine-supervisor
    browser-native-proxy-engine
    browser-stream-bridge
    browser-local-exit
    webspace-provider
    object-provider
    chain-provider
    wallet-provider
    object-provider
    content-block-graph-provider
    drm-provider
    rights-provider
    key-provider
    decrypt-provider
    protected-content-protect-provider
    media-provider
    model-provider
    custody-provider
    protected-content-decrypt-provider
    ipfs-provider
    availability-provider
    operator-drive-adapter
    site-provider
    tunnel-provider
)
ALLOWED_CHANNELS=(
    stable
    canary
    jetson-test
)

# Navigate to project root; relative CLI paths belong to the original caller.
PUBLISH_CALLER_DIR="$PWD"
cd "$(dirname "${BASH_SOURCE[0]}")/.."

# ── Help ──────────────────────────────────────────────────────────────

show_help() {
    echo "ElastOS unsigned release preparation"
    echo "Usage: ./scripts/publish-release.sh --version X.Y.Z --prepare-only DIR"
    echo "       --publisher-did DID --platform-input PLATFORM=DIR [--platform-input ...]"
    echo ""
    echo "Required:"
    echo "  --version X.Y.Z    Runtime release version (see docs/VERSIONING.md)"
    echo "  --prepare-only DIR New unsigned input directory for the custodian signer"
    echo "  --publisher-did DID Public signer DID for unsigned preparation"
    echo "  --platform-input PLATFORM=DIR Reviewed native input (repeat for each platform)"
    echo ""
    echo "Optional:"
    echo "  --ipfs-provider-bin PATH Path to the ipfs-provider binary"
    echo "  --channel NAME     stable | canary | jetson-test (default: stable)"
    echo "  --preview-platform PLATFORM One canary input from that platform's host"
    echo "  --help             Show this help"
    echo ""
    echo "The custodian signs the unsigned output separately."
    echo "Publish the signed set with elastos publish-release."
    echo "Read-only planning: elastos publish-release --version X.Y.Z --dry-run"
    exit 0
}

# ── Helpers ───────────────────────────────────────────────────────────

die()  { echo -e "${RED}Error:${NC} $*" >&2; exit 1; }
info() { echo -e "  ${GREEN}▶${NC} $*"; }
warn() { echo -e "  ${YELLOW}!${NC} $*"; }

default_elastos_data_dir() {
    if [[ -n "${ELASTOS_HOST_DATA_DIR:-}" ]]; then
        printf '%s\n' "${ELASTOS_HOST_DATA_DIR}"
        return
    fi
    if [[ -n "${ELASTOS_DATA_DIR:-}" ]]; then
        printf '%s\n' "${ELASTOS_DATA_DIR}"
        return
    fi
    (
        # The publisher enters the repository root before defining helpers.
        source scripts/install.sh
        installer_data_dir "$HOME" "${XDG_DATA_HOME:-}"
    )
}

discover_source_bootstrap_json() {
    local data_dir helper
    data_dir="$(default_elastos_data_dir)"
    helper="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/discover-source-bootstrap.py"
    DATA_DIR="${data_dir}" \
    COORDS_PATH="${ELASTOS_RUNTIME_COORDS_FILE:-${data_dir}/runtime-coords.json}" \
    PYTHONDONTWRITEBYTECODE=1 \
    python3 "$helper"
}

canonical_publisher_gateway() {
    printf '%s\n' "${ELASTOS_CANONICAL_PUBLISHER_GATEWAY:-https://elastos.elacitylabs.com}"
}

fetch_canonical_signer_did() {
    local gateway="$1"
    curl -fsSL --max-time 20 "${gateway%/}/release-head.json" \
        | jq -r '.signer_did // empty'
}

is_allowed_channel() {
    local needle="$1"
    local channel
    for channel in "${ALLOWED_CHANNELS[@]}"; do
        if [[ "$needle" == "$channel" ]]; then
            return 0
        fi
    done
    return 1
}

sha256() {
    if command -v sha256sum &>/dev/null; then
        sha256sum "$1" | cut -d' ' -f1
    elif command -v shasum &>/dev/null; then
        shasum -a 256 "$1" | cut -d' ' -f1
    else
        die "Neither sha256sum nor shasum found"
    fi
}

file_size() {
    wc -c < "$1" | tr -d ' '
}

ensure_rust_target_installed() {
    local target="$1"
    if ! rustup target list --installed 2>/dev/null | grep -qx "$target"; then
        info "Installing Rust target ${target}..."
        rustup target add "$target" || die "Failed to add Rust target ${target}"
    fi
}

resolve_capsule_dir() {
    local capsule="$1"
    if [[ -f "capsules/${capsule}/Cargo.toml" ]] || [[ -f "capsules/${capsule}/capsule.json" ]]; then
        echo "capsules/${capsule}"
        return 0
    fi
    if [[ -f "elastos/capsules/${capsule}/Cargo.toml" ]] || [[ -f "elastos/capsules/${capsule}/capsule.json" ]]; then
        echo "elastos/capsules/${capsule}"
        return 0
    fi
    if [[ -f "elastos/tools/${capsule}/Cargo.toml" ]]; then
        echo "elastos/tools/${capsule}"
        return 0
    fi
    return 1
}

ipfs_add() {
    local file="$1"
    local absolute
    absolute=$(abs_path "$file")

    local req response status code message cid
    req=$(jq -nc --arg path "$absolute" '{op:"add_path", path:$path, pin:true}')
    response=$(printf '%s\n%s\n' '{"op":"init","config":{}}' "$req" | "$IPFS_PROVIDER_BIN" | tail -n1)

    status=$(echo "$response" | jq -r '.status // empty' 2>/dev/null || true)
    if [[ "$status" != "ok" ]]; then
        code=$(echo "$response" | jq -r '.code // "unknown_error"' 2>/dev/null || echo "unknown_error")
        message=$(echo "$response" | jq -r '.message // "unknown error"' 2>/dev/null || echo "unknown error")
        die "ipfs-provider add failed for ${file} [${code}]: ${message}"
    fi

    cid=$(echo "$response" | jq -r '.data.cid // empty')
    [[ -z "$cid" ]] && die "ipfs-provider returned no CID for ${file}"
    echo "$cid"
}

abs_path() {
    local path="$1"
    if [[ "$path" = /* ]]; then
        echo "$path"
    else
        local dir base
        dir=$(dirname "$path")
        base=$(basename "$path")
        echo "$(cd "$dir" && pwd)/$base"
    fi
}

content_publish_object() {
    local src_file="$1"
    local kind="$2"
    local entry_name="$3"
    shift 3
    [[ -f "$src_file" ]] || die "File not found for content object publish: ${src_file}"

    "$ELASTOS" content publish-object "$src_file" \
        --kind "$kind" \
        --entry-name "$entry_name" \
        "$@"
}

find_ipfs_provider_binary() {
    local candidates=()
    local data_dir
    data_dir="$(default_elastos_data_dir)"

    if [[ -n "${ELASTOS_IPFS_PROVIDER_BIN:-}" ]]; then
        candidates+=("${ELASTOS_IPFS_PROVIDER_BIN}")
    fi

    if [[ -n "${ELASTOS_CAPSULE_BIN_DIR:-}" ]]; then
        candidates+=("${ELASTOS_CAPSULE_BIN_DIR}/ipfs-provider")
    fi

    candidates+=(
        "capsules/ipfs-provider/target/release/ipfs-provider"
        "${data_dir}/bin/ipfs-provider"
    )

    local cmd_path
    cmd_path=$(command -v ipfs-provider 2>/dev/null || true)
    if [[ -n "$cmd_path" ]]; then
        candidates+=("$cmd_path")
    fi

    local path
    for path in "${candidates[@]}"; do
        if [[ -x "$path" ]]; then
            echo "$path"
            return 0
        fi
    done
    return 1
}

now_unix() {
    date +%s
}

resolve_component_meta() {
    local component="$1"
    local platform="$2"
    COMPONENT_NAME="$component" COMPONENT_PLATFORM="$platform" python3 - <<'PY'
import json, os
name = os.environ["COMPONENT_NAME"]
platform = os.environ["COMPONENT_PLATFORM"]
with open("components.json", "r", encoding="utf-8") as f:
    data = json.load(f)
entry = data.get("external", {}).get(name, {})
plat = entry.get("platforms", {}).get(platform) or entry.get("platforms", {}).get("*") or {}
install_path = (plat.get("install_path") or entry.get("install_path") or "").strip()
strategy = (plat.get("strategy") or "").strip()
note = (plat.get("note") or "").strip().replace("\n", " ")
print(f"{install_path}|{strategy}|{note}")
PY
}

component_full_path() {
    local component="$1"
    local platform="$2"
    local meta install_rel
    meta=$(resolve_component_meta "$component" "$platform")
    IFS='|' read -r install_rel _ _ <<< "$meta"
    [[ -n "$install_rel" ]] || return 1
    echo "${HOST_DATA_DIR}/${install_rel}"
    return 0
}

assert_runtime_binary_embeds_release_version() {
    local binary_path="$1"
    local platform_label="$2"
    local expected_version="$3"

    python3 - "$binary_path" "$platform_label" "$expected_version" <<'PY'
import pathlib
import sys

binary_path = pathlib.Path(sys.argv[1])
platform_label = sys.argv[2]
expected_version = sys.argv[3]
blob = binary_path.read_bytes()
expected = expected_version.encode('utf-8')
expected_dev = f"{expected_version}-dev".encode('utf-8')

if expected_dev in blob:
    raise SystemExit(
        f"{platform_label} runtime binary embeds {expected_version}-dev; rebuild without --skip-build or rebuild with ELASTOS_RELEASE_VERSION={expected_version}."
    )
if expected not in blob:
    raise SystemExit(
        f"{platform_label} runtime binary does not embed expected release version {expected_version}."
    )
PY
}

support_binary_build_path() {
    local name="$1"
    local target="${2:-}"
    local capsule_dir candidate
    capsule_dir=$(resolve_capsule_dir "$name" || true)
    [[ -n "$capsule_dir" ]] || return 1

    if [[ "${RELEASE_PREPARE_LOCKED:-false}" == true && -n "${CARGO_TARGET_DIR:-}" ]]; then
        printf '%s/%srelease/%s\n' "$CARGO_TARGET_DIR" "${target:+${target}/}" "$name"
        return 0
    fi

    # Workspace members (under elastos/capsules/) compile to the workspace
    # root target dir, not the capsule's own target dir.
    local paths=()
    if [[ -n "$target" ]]; then
        paths+=("${capsule_dir}/target/${target}/release/${name}")
        paths+=("elastos/target/${target}/release/${name}")
    else
        paths+=("${capsule_dir}/target/release/${name}")
        paths+=("elastos/target/release/${name}")
    fi

    for candidate in "${paths[@]}"; do
        if [[ -x "$candidate" ]]; then
            echo "$candidate"
            return 0
        fi
    done

    # Return the first path for error messaging even if it doesn't exist
    echo "${paths[0]}"
}

build_support_binary() {
    local name="$1"
    local platform="$2"
    local target="${3:-}"
    local use_cross="${4:-false}"
    local capsule_dir binary
    local build_args=(build --release)
    if [[ "${RELEASE_PREPARE_LOCKED:-false}" == true ]]; then
        build_args+=(--locked)
    fi
    capsule_dir=$(resolve_capsule_dir "$name" || true)
    [[ -n "$capsule_dir" ]] || die "Source directory not found for support asset '${name}'"
    binary=$(support_binary_build_path "$name" "$target")

    if [[ "$SKIP_BUILD" == true ]]; then
        [[ -x "$binary" ]] || die "Missing ${name} binary for ${platform}: ${binary}. Build it first or rerun without --skip-build."
        echo "$binary"
        return 0
    fi

    info "  Building ${name} (${platform})..." >&2
    if [[ -n "$target" ]]; then
        if [[ "$use_cross" == true ]] && command -v cross >/dev/null 2>&1; then
            (cd "$capsule_dir" && "${CROSS_ENV[@]}" cross "${build_args[@]}" --target "$target") >&2 || return
        else
            (cd "$capsule_dir" && cargo "${build_args[@]}" --target "$target") >&2 || return
        fi
    else
        (cd "$capsule_dir" && cargo "${build_args[@]}") >&2 || return
    fi

    binary=$(support_binary_build_path "$name" "$target")
    [[ -x "$binary" ]] || die "${name} binary missing after build for ${platform}: ${binary}"
    echo "$binary"
}

build_packaged_capsule_archive() {
    local platform="$1"
    local capsule_name="$2"
    local native_renderer="${3:-}"
    local capsule_dir capsule_type entrypoint stage_root archive

    capsule_dir=$(resolve_capsule_dir "$capsule_name" || true)
    [[ -n "$capsule_dir" ]] || die "${capsule_name} source directory not found"
    [[ -f "${capsule_dir}/capsule.json" ]] || die "${capsule_name} capsule manifest not found at ${capsule_dir}/capsule.json"

    capsule_type=$(capsule_manifest_field "$capsule_name" "type")
    entrypoint=$(capsule_manifest_field "$capsule_name" "entrypoint")
    stage_root="${TMPDIR}/support-assets-${platform}-${capsule_name}"
    archive="${TMPDIR}/support-assets-${platform}/${capsule_name}.tar.gz"
    rm -rf "$stage_root" || return
    mkdir -p "${stage_root}/${capsule_name}" "$(dirname "$archive")" || return

    case "$capsule_type" in
        data)
            copy_clean_capsule_tree "$capsule_dir" "${stage_root}/${capsule_name}" || return
            [[ -f "${stage_root}/${capsule_name}/${entrypoint}" ]] || die "${capsule_name} data entrypoint missing after packaging: ${entrypoint}"
            ;;
        wasm|web-projection)
            stage_wasm_capsule "$capsule_name" "$capsule_dir" "${stage_root}/${capsule_name}" || return
            ;;
        *)
            die "Unsupported packaged app capsule type for ${capsule_name}: ${capsule_type}"
            ;;
    esac

    if [[ -n "$native_renderer" ]]; then
        [[ "$capsule_name" == home-cli && -x "$native_renderer" && ! -L "$native_renderer" ]] \
            || die "Home CLI packaging requires a built native renderer"
        mkdir -p "${stage_root}/${capsule_name}/bin" || return
        install -m 755 "$native_renderer" "${stage_root}/${capsule_name}/bin/home-cli" || return
    fi
    create_capsule_tar "$archive" "$stage_root" "$capsule_name" || return
    echo "$archive"
}

provider_capsule_names() {
    python3 - <<'PY'
import json
from pathlib import Path

root = Path(".")
components = json.loads(Path("components.json").read_text(encoding="utf-8"))
for name, component in sorted((components.get("external") or {}).items()):
    if not isinstance(component.get("provider_runtime"), dict):
        continue
    capsule_dir = None
    for candidate in (root / "capsules" / name, root / "elastos" / "capsules" / name):
        if (candidate / "capsule.json").is_file():
            capsule_dir = candidate
            break
    if capsule_dir is None:
        continue
    manifest = json.loads((capsule_dir / "capsule.json").read_text(encoding="utf-8"))
    honest = (manifest.get("type"), manifest.get("execution"), manifest.get("runtime_abi"), manifest.get("entrypoint")) == ("native-provider", "native-provider", "elastos.provider-stdio/v1", name)
    legacy = manifest.get("type") in ("wasm", "microvm") and manifest.get("execution") is None and manifest.get("runtime_abi") is None and manifest.get("entrypoint") == "rootfs.ext4"
    if manifest.get("role") != "provider" or not (honest or legacy):
        raise SystemExit(f"{name} capsule manifest must describe native-provider execution")
    icon_dir = str(manifest.get("icon") or "").strip().strip("/")
    if not icon_dir:
        raise SystemExit(f"{name} provider capsule icon path is missing")
    if ".." in Path(icon_dir).parts:
        raise SystemExit(f"{name} provider capsule icon path escapes the capsule")
    print(name)
PY
}

build_packaged_provider_capsule_metadata_archive() {
    local capsule_name="$1"
    local capsule_dir icon_dir stage_root archive_dir archive size source

    capsule_dir=$(resolve_capsule_dir "$capsule_name" || true)
    [[ -n "$capsule_dir" ]] || die "${capsule_name} source directory not found"
    [[ -f "${capsule_dir}/capsule.json" ]] || die "${capsule_name} capsule manifest not found at ${capsule_dir}/capsule.json"

    [[ "$(capsule_manifest_field "$capsule_name" "role")" == "provider" ]] \
        || die "${capsule_name} capsule manifest role must be provider"
    icon_dir=$(capsule_manifest_field "$capsule_name" "icon")
    [[ -n "$icon_dir" ]] || die "${capsule_name} provider capsule icon path is missing"

    stage_root="${TMPDIR}/provider-contract-${capsule_name}"
    archive_dir="${TMPDIR}/supported-provider-contract-archives"
    archive="${archive_dir}/${capsule_name}-capsule-metadata.tar.gz"
    rm -rf "$stage_root" || return
    mkdir -p "${stage_root}/${capsule_name}" "$archive_dir" || return

    copy_release_source_file "${capsule_dir}/capsule.json" "${stage_root}/${capsule_name}/capsule.json" || return
    for size in 32 64 128 256; do
        source="${capsule_dir}/${icon_dir}/icon-${size}.png"
        [[ -f "$source" ]] || die "${capsule_name} provider capsule icon missing: ${icon_dir}/icon-${size}.png"
        mkdir -p "${stage_root}/${capsule_name}/${icon_dir}" || return
        copy_release_source_file "$source" "${stage_root}/${capsule_name}/${icon_dir}/icon-${size}.png" || return
    done

    create_capsule_tar "$archive" "$stage_root" "$capsule_name" || return
    echo "$archive"
}

capsule_manifest_field() {
    local capsule_name="$1"
    local field="$2"
    local capsule_dir
    capsule_dir=$(resolve_capsule_dir "$capsule_name" || true)
    [[ -n "$capsule_dir" ]] || die "${capsule_name} source directory not found"
    python3 - "$capsule_dir/capsule.json" "$field" <<'PY'
import json
import sys
path, field = sys.argv[1], sys.argv[2]
with open(path, "r", encoding="utf-8") as f:
    value = json.load(f).get(field, "")
print(value if isinstance(value, str) else "")
PY
}

copy_clean_capsule_tree() {
    local src="$1"
    local dest="$2"
    mkdir -p "$dest"
    if [[ -n "${RELEASE_PREPARE_SOURCE_COMMIT:-}" ]]; then
        local prefix archive status
        prefix=$(git -C "$src" rev-parse --show-prefix) || return
        # macOS bsdtar stops reading at the end-of-archive marker, so a pipe can
        # SIGPIPE git archive while it writes padding (exit 141 under pipefail).
        archive=$(mktemp "${TMPDIR:-/tmp}/capsule-tree.XXXXXX") || return
        git archive -o "$archive" "${RELEASE_PREPARE_SOURCE_COMMIT}:${prefix%/}" &&
            tar -xf "$archive" -C "$dest"
        status=$?
        rm -f "$archive"
        return "$status"
    fi
    tar \
        --exclude='./target' \
        --exclude='./node_modules' \
        --exclude='./.git' \
        --exclude='./.DS_Store' \
        -cf - -C "$src" . | tar -xf - -C "$dest"
}

copy_release_source_file() {
    local src="$1"
    local dest="$2"
    if [[ -n "${RELEASE_PREPARE_SOURCE_COMMIT:-}" ]]; then
        git show "${RELEASE_PREPARE_SOURCE_COMMIT}:${src}" > "$dest"
    else
        cp "$src" "$dest"
    fi
}

create_capsule_tar() {
    local archive="$1"
    local stage_root="$2"
    local capsule_name="$3"
    python3 - "$archive" "$stage_root" "$capsule_name" <<'PY'
import gzip
from pathlib import Path
import sys
import tarfile

archive, stage_root, capsule_name = sys.argv[1:]

def normalize(info):
    # Equivalent checkouts can share inodes differently. Store regular files
    # independently, while preserving explicit symbolic links.
    if info.islnk():
        info.type = tarfile.REGTYPE
        info.linkname = ""
        info.size = (Path(stage_root) / info.name).stat().st_size
    info.uid = info.gid = info.mtime = 0
    info.uname = info.gname = ""
    info.pax_headers = {}
    info.mode = 0o777 if info.issym() else (
        0o755 if info.isdir() or info.mode & 0o111 else 0o644
    )
    return info

# tarfile sorts recursive entries. The gzip header must also be independent
# of the output filename and wall clock, on both macOS and Linux.
with open(archive, "wb") as output:
    with gzip.GzipFile(filename="", mode="wb", fileobj=output, mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as package:
            package.add(Path(stage_root) / capsule_name, arcname=capsule_name, filter=normalize)
PY
}

stage_wasm_capsule() {
    local capsule_name="$1"
    local capsule_dir="$2"
    local dest="$3"
    local entrypoint runtime_abi built_wasm candidates candidate

    entrypoint=$(capsule_manifest_field "$capsule_name" "entrypoint")
    [[ -n "$entrypoint" ]] || die "${capsule_name} capsule manifest missing entrypoint"
    runtime_abi=$(capsule_manifest_field "$capsule_name" "runtime_abi")
    local capsule_type execution
    capsule_type=$(capsule_manifest_field "$capsule_name" "type")
    execution=$(capsule_manifest_field "$capsule_name" "execution")
    if [[ "$runtime_abi" == "elastos.component/v1" && ( "$capsule_type" != "wasm" || "$execution" != "component" ) ]] ||
       [[ "$runtime_abi" == "elastos.runtime-projection/v1" && ( ( "$capsule_type" != "wasm" && "$capsule_type" != "web-projection" ) || "$execution" != "web-projection" ) ]]; then
        die "${capsule_name} type contradicts its execution ABI"
    fi

    if [[ "$runtime_abi" == "elastos.component/v1" ]]; then
        ensure_rust_target_installed "wasm32-unknown-unknown" || return
        info "  Building ${capsule_name} Component..." >&2
        scripts/build-component-capsule.sh "$capsule_dir" >&2 || return
    elif [[ "$runtime_abi" == "elastos.runtime-projection/v1" ]]; then
        info "  Using ${capsule_name} Runtime projection from source..." >&2
    else
        die "${capsule_name} uses unsupported runtime_abi '${runtime_abi:-unset}'"
    fi

    candidates=("${capsule_dir}/${entrypoint}")
    built_wasm=""
    for candidate in "${candidates[@]}"; do
        if [[ -f "$candidate" ]]; then
            built_wasm="$candidate"
            break
        fi
    done
    [[ -n "$built_wasm" ]] || die "${capsule_name} entrypoint missing after build: ${entrypoint}"

    mkdir -p "$(dirname "${dest}/${entrypoint}")" || return
    copy_release_source_file "${capsule_dir}/capsule.json" "$dest/capsule.json" || return
    if [[ -d "${capsule_dir}/browser" ]]; then
        mkdir -p "${dest}/browser" || return
        copy_clean_capsule_tree "${capsule_dir}/browser" "${dest}/browser" || return
    fi
    if [[ "$runtime_abi" == "elastos.runtime-projection/v1" ]]; then
        copy_release_source_file "$built_wasm" "${dest}/${entrypoint}" || return
    else
        cp "$built_wasm" "${dest}/${entrypoint}" || return
    fi
}

record_direct_asset() {
    local updates_json="$1"
    local name="$2"
    local staged="$3"
    local install_path="$4"
    local release_path="$5"
    local extract_path="${6:-}"
    local checksum size

    checksum=$(sha256 "$staged") || return
    size=$(file_size "$staged") || return

    if [[ -n "$extract_path" ]]; then
        echo "$updates_json" | jq \
            --arg name "$name" \
            --arg checksum "sha256:${checksum}" \
            --arg install_path "$install_path" \
            --arg extract_path "$extract_path" \
            --arg release_path "$release_path" \
            --argjson size "$size" \
            '.[$name] = {checksum: $checksum, size: $size, install_path: $install_path, extract_path: $extract_path, release_path: $release_path}'
    else
        echo "$updates_json" | jq \
            --arg name "$name" \
            --arg checksum "sha256:${checksum}" \
            --arg install_path "$install_path" \
            --arg release_path "$release_path" \
            --argjson size "$size" \
            '.[$name] = {checksum: $checksum, size: $size, install_path: $install_path, release_path: $release_path}'
    fi
}

record_provider_capsule_metadata_asset() {
    local updates_json="$1"
    local name="$2"
    local staged="$3"
    local install_path="$4"
    local release_path="$5"
    local extract_path="$6"
    local checksum size

    checksum=$(sha256 "$staged") || return
    size=$(file_size "$staged") || return

    echo "$updates_json" | jq \
        --arg name "$name" \
        --arg checksum "sha256:${checksum}" \
        --arg install_path "$install_path" \
        --arg extract_path "$extract_path" \
        --arg release_path "$release_path" \
        --argjson size "$size" \
        '.external[$name].capsule_metadata = {
            install_path: $install_path,
            platforms: {
                "*": {
                    checksum: $checksum,
                    size: $size,
                    install_path: $install_path,
                    extract_path: $extract_path,
                    release_path: $release_path
                }
            }
        }'
}

stamp_direct_assets() {
    local platform_key="$1"
    local updates_json="$2"

    DIRECT_SETUP_PLATFORM="$platform_key" DIRECT_UPDATES_JSON="$updates_json" python3 - <<'PY'
import copy
import json
import os

platform = os.environ["DIRECT_SETUP_PLATFORM"]
updates = json.loads(os.environ["DIRECT_UPDATES_JSON"])
with open("components.json", "r", encoding="utf-8") as f:
    data = json.load(f)

external = {}
for name, platform_meta in updates.items():
    if name not in data.get("external", {}):
        raise SystemExit(f"Missing external component definition for {name} in components.json")
    component = copy.deepcopy(data["external"][name])
    component["platforms"] = {platform: platform_meta}
    external[name] = component

print(json.dumps({"external": external}))
PY
}

build_packaged_media_tools_archive() {
    local setup_platform="$1"
    local host_platform cache target_root stage_root archive
    case "$(uname -s):$(uname -m)" in
        Darwin:arm64|Darwin:aarch64) host_platform=darwin-arm64 ;;
        Linux:x86_64) host_platform=linux-amd64 ;;
        Linux:aarch64|Linux:arm64) host_platform=linux-arm64 ;;
        *) die "Unsupported native media-tools build platform" ;;
    esac
    [[ "$host_platform" == "$setup_platform" ]] \
        || die "Prepare media-tools on ${setup_platform}; the media builder requires a native host"
    target_root="${CARGO_TARGET_DIR:-}"
    if [[ -z "$target_root" ]]; then
        target_root=$(cd elastos && cargo metadata --locked --offline --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])') || return
    fi
    cache="${target_root}/media-tools/${setup_platform}"
    scripts/build-media-tools.sh --output "$cache" >&2 || return
    stage_root="${TMPDIR}/media-tools-${setup_platform}"
    archive="${TMPDIR}/media-tools-${setup_platform}.tar.gz"
    mkdir -p "$stage_root" || return
    cp -R "$cache" "${stage_root}/media-tools" || return
    create_capsule_tar "$archive" "$stage_root" media-tools || return
    echo "$archive"
}

build_supported_direct_assets() {
    local platform="$1"
    local setup_platform="$2"
    local target="${3:-}"
    local use_cross="${4:-false}"
    local stage_dir updates_json name binary staged install_path release_path archive

    stage_dir="${TMPDIR}/supported-assets-${platform}"
    mkdir -p "$stage_dir" || return
    updates_json='{}'

    archive=$(build_packaged_media_tools_archive "$setup_platform") || return
    release_path="media-tools-${setup_platform}.tar.gz"
    staged="${stage_dir}/${release_path}"
    cp "$archive" "$staged" || return
    updates_json=$(record_direct_asset "$updates_json" media-tools "$staged" "tools/media-tools" "$release_path" media-tools) || return

    for name in "${SUPPORT_BINARY_ASSETS[@]}"; do
        binary=$(build_support_binary "$name" "$platform" "$target" "$use_cross") || return
        release_path="${name}-${setup_platform}"
        staged="${stage_dir}/${release_path}"
        cp "$binary" "$staged" || return
        install_path="bin/${name}"
        updates_json=$(record_direct_asset "$updates_json" "$name" "$staged" "$install_path" "$release_path") || return
    done

    # Home CLI remains one capsule; its terminal renderer makes this archive native.
    binary=$(build_support_binary home-cli "$platform" "$target" "$use_cross") || return
    archive=$(build_packaged_capsule_archive "$platform" home-cli "$binary") || return
    release_path="home-cli-${setup_platform}.tar.gz"
    staged="${stage_dir}/${release_path}"
    cp "$archive" "$staged" || return
    updates_json=$(record_direct_asset "$updates_json" home-cli "$staged" "capsules/home-cli" "$release_path" home-cli) || return

    if [[ "$setup_platform" == darwin-arm64 ]] && jq -e '.external["browser-vz-engine-supervisor"]' components.json >/dev/null; then
        # The source Home and release use the same native supervisor and entitlement.
        if [[ "$SKIP_BUILD" != true ]]; then
            (cd elastos && cargo build --locked --release -p elastos-vz --bin browser-vz-engine-supervisor) >&2 || return
        fi
        staged="$stage_dir/browser-vz-engine-supervisor-darwin-arm64"
        cp "${CARGO_TARGET_DIR:-elastos/target}/release/browser-vz-engine-supervisor" "$staged" || return
        scripts/dev/sign-elastos-vz/sign.sh "$staged" >&2 || return
        updates_json=$(record_direct_asset "$updates_json" browser-vz-engine-supervisor "$staged" bin/browser-vz-engine-supervisor browser-vz-engine-supervisor-darwin-arm64) || return
    fi

    stamp_direct_assets "$setup_platform" "$updates_json"
}

stage_release_artifacts() {
    local artifact_dir="$1"
    local f base CROSS_SRC
    mkdir -p "$artifact_dir"
    # Save platform binaries for direct gateway serving
    cp "${STAGED_ELASTOS}" "${artifact_dir}/elastos-${PLATFORM}"
    if [[ -n "${CROSS_ELASTOS:-}" && -f "${CROSS_ELASTOS}" ]]; then
        cp "${CROSS_ELASTOS}" "${artifact_dir}/elastos-${CROSS_PLATFORM}"
    fi
    cp "${TMPDIR}/components.json" "${artifact_dir}/components-${PLATFORM}.json"
    if [[ -n "${CROSS_PLATFORM:-}" && -f "${TMPDIR}/components-${CROSS_PLATFORM}.json" ]]; then
        cp "${TMPDIR}/components-${CROSS_PLATFORM}.json" "${artifact_dir}/components-${CROSS_PLATFORM}.json"
    fi
    # Copy first-party support assets for Carrier-served setup fetches.
    for f in "${TMPDIR}/supported-assets-${PLATFORM}"/* \
        "${TMPDIR}/supported-assets-universal"/*; do
        [ -f "$f" ] || continue
        cp -f "$f" "${artifact_dir}/$(basename "$f")"
    done
    for f in "${TMPDIR}/supported-provider-contracts-universal"/*; do
        [ -f "$f" ] || continue
        cp -f "$f" "${artifact_dir}/$(basename "$f")"
    done
    if [[ -n "${CROSS_PLATFORM:-}" ]]; then
        for f in "${TMPDIR}/supported-assets-${CROSS_PLATFORM}"/*; do
            [ -f "$f" ] || continue
            cp -f "$f" "${artifact_dir}/$(basename "$f")"
        done
    fi
    # Copy capsule artifacts for Carrier serving (platform-suffixed)
    for f in "${ARTIFACTS_DIR}"/*.capsule.tar.gz; do
        [ -f "$f" ] || continue
        base=$(basename "$f" .capsule.tar.gz)
        cp -f "$f" "${artifact_dir}/${base}-${PLATFORM}.capsule.tar.gz"
    done
    if [[ -f model-catalog.json ]]; then
        cp -f model-catalog.json "${artifact_dir}/model-catalog.json"
    fi
    if [[ -n "${CROSS_PLATFORM:-}" ]]; then
        CROSS_SRC="${TMPDIR}/artifacts-${CROSS_ARCH}"
        [ -d "$CROSS_SRC" ] || CROSS_SRC="artifacts-${CROSS_ARCH}"
        for f in "${CROSS_SRC}"/*.capsule.tar.gz; do
            [ -f "$f" ] || continue
            base=$(basename "$f" .capsule.tar.gz)
            cp -f "$f" "${artifact_dir}/${base}-${CROSS_PLATFORM}.capsule.tar.gz"
        done
    fi
}

build_platform_independent_direct_assets() {
    local platform="$1"
    local stage_dir updates_json release_path
    local archive staged capsule

    stage_dir="${TMPDIR}/supported-assets-universal"
    mkdir -p "$stage_dir" || return
    updates_json='{}'

    for capsule in \
        home-gui \
        home \
        system \
        wallet-metamask \
        wallet-unisat \
        wallet-walletconnect \
        wallet \
        browser \
        documents \
        library \
        marketplace \
        archive-manager \
        inbox \
        services \
        people \
        gba-emulator \
        gba-ucity \
        gba-nonogram \
        chat-room \
        assistant \
        elacity-player; do
        if [[ -n "${ARTIFACTS_DIR:-}" && -f "${ARTIFACTS_DIR}/${capsule}.capsule.tar.gz" ]]; then
            archive="${ARTIFACTS_DIR}/${capsule}.capsule.tar.gz"
        else
            archive=$(build_packaged_capsule_archive "$platform" "$capsule") || return
        fi
        release_path="${capsule}.tar.gz"
        staged="${stage_dir}/${release_path}"
        cp "$archive" "$staged" || return
        updates_json=$(record_direct_asset "$updates_json" "$capsule" "$staged" "capsules/${capsule}" "$release_path" "$capsule") || return
    done

    stamp_direct_assets "*" "$updates_json"
}

build_platform_independent_provider_capsule_metadata_assets() {
    local stage_dir updates_json provider archive staged release_path providers

    stage_dir="${TMPDIR}/supported-provider-contracts-universal"
    mkdir -p "$stage_dir" || return
    updates_json='{}'

    providers=$(provider_capsule_names) || return
    while IFS= read -r provider; do
        [[ -n "$provider" ]] || continue
        archive=$(build_packaged_provider_capsule_metadata_archive "$provider") || return
        release_path="${provider}-capsule-metadata.tar.gz"
        staged="${stage_dir}/${release_path}"
        cp "$archive" "$staged" || return
        updates_json=$(
            record_provider_capsule_metadata_asset \
                "$updates_json" \
                "$provider" \
                "$staged" \
                "capsules/${provider}" \
                "$release_path" \
                "$provider"
        ) || return
    done <<< "$providers"

    echo "$updates_json"
}
# Attach transport identities only after local asset preparation succeeds.
# Builders above return unsigned descriptors with no placeholder CIDs.
# Upstream software is fetched only by this build worker. Installed clients use
# the same signed release-path/CID contract as every first-party component.
build_upstream_direct_assets() {
    local platform="$1" setup_platform="$2"
    local output="$TMPDIR/supported-upstream-assets-$platform"
    local cache="${ELASTOS_RELEASE_UPSTREAM_CACHE:-$TMPDIR/upstream-cache}"
    local args=()
    if [[ -n "${ELASTOS_LLAMA_ARM64_BUNDLE:-}" ]]; then
        args+=(--llama-arm64-bundle "$ELASTOS_LLAMA_ARM64_BUNDLE")
    fi
    python3 scripts/release-upstream-assets.py --platform "$setup_platform" \
        --cache "$cache" --output "$output" ${args[@]+"${args[@]}"}
}

publish_direct_assets() {
    local updates_json="$1"
    local platform="$2"
    local release_paths release_path staged candidate cid count
    release_paths=$(printf '%s\n' "$updates_json" | jq -r \
        '[.. | objects | .release_path? // empty] | unique[]') || return
    while IFS= read -r release_path; do
        [[ -n "$release_path" ]] || continue
        case "$release_path" in
            */*|*\\*|.|..) die "Invalid direct asset filename: $release_path" ;;
        esac
        staged=""
        count=0
        for candidate in \
            "${TMPDIR}/supported-assets-${platform}/${release_path}" \
            "${TMPDIR}/supported-assets-universal/${release_path}" \
            "${TMPDIR}/supported-provider-contracts-universal/${release_path}" \
            "${TMPDIR}/supported-upstream-assets-${platform}/${release_path}"; do
            if [[ -f "$candidate" && ! -L "$candidate" ]]; then
                staged="$candidate"
                count=$((count + 1))
            fi
        done
        [[ "$count" == 1 ]] || die "Expected one staged direct asset: $release_path (found $count)"
        cid=$(ipfs_add "$staged") || return
        updates_json=$(printf '%s\n' "$updates_json" | jq \
            --arg path "$release_path" --arg cid "$cid" \
            'walk(if type == "object" and .release_path? == $path then .cid = $cid else . end)') || return
    done <<< "$release_paths"
    printf '%s\n' "$updates_json"
}

merge_direct_assets() {
    jq -s '.[0] * .[1]' <(printf '%s\n' "$1") <(printf '%s\n' "$2")
}

# The native preparation worker uses the same template and profile merge.
generate_components_json() {
    local capsule_entries="$1"
    local direct_assets="$2"
    local external profiles catalog
    external=$(jq '.external' components.json)
    profiles=$(jq '.profiles' components.json)
    catalog=$(jq -c '.model_catalog // null' components.json)
    jq -n \
        --arg schema "elastos.components/v1" \
        --argjson capsules "$capsule_entries" \
        --argjson external "$external" \
        --argjson profiles "$profiles" \
        --argjson direct "$direct_assets" \
        --argjson catalog "$catalog" \
        '{schema: $schema, capsules: $capsules, external: ($external * $direct.external), profiles: $profiles} + (if $catalog == null then {} else {model_catalog: $catalog} end)'
}

runtime_tunnel_url() {
    local coords_file
    coords_file="$(default_elastos_data_dir)/runtime-coords.json"
    [[ -f "$coords_file" ]] || return 1

    local api token response url
    api=$(jq -r '.api_url // empty' "$coords_file" 2>/dev/null || true)
    token=$(jq -r '.shell_token // empty' "$coords_file" 2>/dev/null || true)
    [[ -n "$api" && -n "$token" ]] || return 1

    response=$(curl -fsS --max-time 3 \
        -H "Authorization: Bearer ${token}" \
        -H "Content-Type: application/json" \
        -X POST \
        -d '{}' \
        "${api}/api/provider/tunnel/status" 2>/dev/null || true)
    [[ -n "$response" ]] || return 1

    url=$(echo "$response" | jq -r '.data.url // empty' 2>/dev/null || true)
    [[ -n "$url" ]] || return 1
    echo "$url"
    return 0
}

# Input admission and byte staging happen before signing, uploads or state writes.
stage_platform_inputs() {
    local output="$1"
    shift
    local value args=()
    for value in "$@"; do args+=(--input "$value"); done
    python3 scripts/release-platform-input.py stage-inputs \
        --version "$VERSION" --output "$output" "${args[@]}" ${PREVIEW_ARGS[@]+"${PREVIEW_ARGS[@]}"}
}

publish_prepared_platform_inputs() {
    local root="$1"
    local path cid components_cid components_sha components_size
    python3 scripts/release-platform-input.py verify-staged "$root" ${PREVIEW_ARGS[@]+"${PREVIEW_ARGS[@]}"} || return
    : > "${TMPDIR}/input-cids.jsonl"
    while IFS= read -r path; do
        cid=$(ipfs_add "$root/artifacts/$path") || return
        jq -nc --arg path "$path" --arg cid "$cid" '{($path): $cid}' >> "${TMPDIR}/input-cids.jsonl" || return
    done < <(jq -r '.files | keys[]' "$root/assembly.json")
    jq -s 'add' "${TMPDIR}/input-cids.jsonl" > "${TMPDIR}/input-cids.json" || return
    python3 scripts/release-platform-input.py attach-cids "$root" \
        --cids "${TMPDIR}/input-cids.json" ${PREVIEW_ARGS[@]+"${PREVIEW_ARGS[@]}"} || return
    PREPARED_ARTIFACTS_DIR="$root/artifacts"
    components_cid=$(ipfs_add "$PREPARED_ARTIFACTS_DIR/components-${PLATFORM}.json") || return
    components_sha=$(sha256 "$PREPARED_ARTIFACTS_DIR/components-${PLATFORM}.json") || return
    components_size=$(file_size "$PREPARED_ARTIFACTS_DIR/components-${PLATFORM}.json") || return
    PLATFORMS_JSON=$(jq -n --slurpfile assembly "$root/assembly.json" \
        --slurpfile cids "${TMPDIR}/input-cids.json" \
        --arg cid "$components_cid" --arg sha "$components_sha" --argjson size "$components_size" '
        $assembly[0] as $a | $cids[0] as $c | reduce $a.platforms[] as $p ({};
        .[$p] = {binary: {cid: $c["elastos-"+$p], sha256: $a.files["elastos-"+$p].sha256,
                         size: $a.files["elastos-"+$p].size},
                 components: {cid: $cid, sha256: $sha, size: $size}})') || return
    RELEASE_SOURCE_JSON=$(jq -c '.source | {commit,tree}' "$root/assembly.json") || return
    STAGED_ELASTOS="$PREPARED_ARTIFACTS_DIR/elastos-${PLATFORM}"
    cp "$PREPARED_ARTIFACTS_DIR/components-${PLATFORM}.json" "${TMPDIR}/components.json" || return
    BINARY_CID=$(printf '%s' "$PLATFORMS_JSON" | jq -r --arg p "$PLATFORM" '.[$p].binary.cid')
    BINARY_SHA256=$(sha256 "$STAGED_ELASTOS")
    BINARY_SIZE=$(file_size "$STAGED_ELASTOS")
    COMPONENTS_CID="$components_cid"
    COMPONENTS_SHA256="$components_sha"
    COMPONENTS_SIZE="$components_size"
    CAPSULE_ENTRIES='{}'
    CROSS_CAPSULE_ENTRIES='{}'
    CROSS_PLATFORM='' CROSS_BINARY_CID='' CROSS_COMPONENTS_CID=''
    SHELL_CID='' SHELL_SHA256=''
}

prepare_release_signing_input() {
    local root="$1" output="$2" publisher="$3" bootstrap ticket node gateway ipns
    local args=() prev
    ticket="${ELASTOS_SOURCE_CONNECT_TICKET:-}"
    node="${ELASTOS_PUBLISHER_NODE_ID:-}"
    if [[ -n "$ticket" || -n "$node" ]]; then
        [[ -n "$ticket" && -n "$node" ]] || die "unsigned preparation requires the Carrier ticket and node from one publisher"
    else
        bootstrap=$(discover_source_bootstrap_json) || return
        ticket=$(printf '%s' "$bootstrap" | jq -r '.ticket // empty')
        node=$(printf '%s' "$bootstrap" | jq -r '.node_id // empty')
    fi
    [[ -n "$ticket" && -n "$node" ]] || die "unsigned preparation requires a Publisher Carrier ticket/node pair"
    gateway="${ELASTOS_PUBLISHER_GATEWAY:-$(canonical_publisher_gateway)}"
    ipns="${ELASTOS_IPNS_NAME:-}"
    jq -nc --arg did "$publisher" --arg ticket "$ticket" --arg node "$node" \
        --arg gateway "${gateway%/}" --arg ipns "$ipns" \
        '{MAINTAINER_DID:$did,SOURCE_CONNECT_TICKET:$ticket,PUBLISHER_NODE_ID:$node,PUBLISHER_GATEWAY:$gateway,IPNS_NAME:$ipns}' \
        > "${TMPDIR}/signing-stamps.json" || return
    jq --argjson platforms "$PLATFORMS_JSON" '
        reduce ($platforms | to_entries[]) as $p (. ;
            .["components-"+$p.key+".json"] = $p.value.components.cid)' \
        "${TMPDIR}/input-cids.json" > "${TMPDIR}/signing-cids.json" || return
    for prev in release head; do
        local path="${STATE_DIR}/last-release-cid" option=--prev-release-cid field=last_release_cid value=""
        if [[ "$prev" == head ]]; then path="${STATE_DIR}/last-release-head-cid"; option=--prev-head-cid; field=last_head_cid; fi
        if [[ -e "${STATE_DIR}/publish-state.json" || -L "${STATE_DIR}/publish-state.json" ]]; then
            [[ -f "${STATE_DIR}/publish-state.json" && ! -L "${STATE_DIR}/publish-state.json" ]] \
                || die "publish-state.json must be a regular receipt"
            value=$(jq -r --arg field "$field" '
                if type != "object" then error("publication receipt must be an object")
                elif .[$field] == null then ""
                elif (.[$field] | type) == "string" and .[$field] != "" then .[$field]
                else error("publication receipt CID must be a nonempty string") end' \
                "${STATE_DIR}/publish-state.json") || return
        elif [[ -f "$path" ]]; then
            value=$(cat "$path") || return
        fi
        [[ -z "$value" ]] || args+=("$option" "$value")
    done
    python3 scripts/release-platform-input.py signing-input "$root" \
        --cids "${TMPDIR}/signing-cids.json" --stamps "${TMPDIR}/signing-stamps.json" \
        --channel "$CHANNEL" --output "$output" \
        ${PREVIEW_ARGS[@]+"${PREVIEW_ARGS[@]}"} ${args[@]+"${args[@]}"}
}

# The Publisher root serves stable paths: release-head.json, release.json,
# install.sh and artifacts/<name>. A prepared set is checked against its own
# signed metadata, staged beside the current publication, then promoted by
# per-file rename with release-head.json last, so a reader sees the previous
# head until every advertised byte is in place. Renames are atomic per file,
# not per release: a reader that already holds the previous head can still meet
# replaced artifact bytes at a stable path and fails closed on its checksum.
verify_release_publication_set() {
    local head="$1" release="$2" install="$3" artifact_dir="$4" state="$5"
    python3 - "$head" "$release" "$install" "$artifact_dir" "$state" <<'PY'
import hashlib
import importlib.util
import json
import stat
import sys
from pathlib import Path

head_path, release_path, install_path, artifact_root = (Path(p) for p in sys.argv[1:5])
state_path = Path(sys.argv[5])
spec = importlib.util.spec_from_file_location(
    "components_integrity", "scripts/components-release-integrity-check.py")
integrity = importlib.util.module_from_spec(spec)
spec.loader.exec_module(integrity)


def regular(path, label):
    if path.is_symlink() or not path.exists() or not stat.S_ISREG(path.stat().st_mode):
        raise ValueError(f"{label} must be a regular file: {path}")
    data = path.read_bytes()
    if not data:
        raise ValueError(f"{label} is empty: {path}")
    return data


def envelope(data, schema, label):
    try:
        value = json.loads(data)
    except ValueError as exc:
        raise ValueError(f"{label} is not valid JSON: {exc}")
    payload = value.get("payload") if isinstance(value, dict) else None
    if not isinstance(payload, dict) or payload.get("schema") != schema:
        raise ValueError(f"{label} is not a {schema} envelope")
    if any(not isinstance(value.get(k), str) or not value[k] for k in ("signature", "signer_did")):
        raise ValueError(f"{label} is missing its signature or signer")
    return payload


def digest(data):
    return hashlib.sha256(data).hexdigest()


try:
    release_bytes = regular(release_path, "release.json")
    head = envelope(regular(head_path, "release-head.json"), "elastos.release.head/v1", "release-head.json")
    release = envelope(release_bytes, "elastos.release/v1", "release.json")
    state = json.loads(regular(state_path, "publish-state.json"))
    if not isinstance(state, dict):
        raise ValueError("publish-state.json must be an object")
    signer = json.loads(release_bytes)["signer_did"]
    if state.get("publisher_did") != signer or json.loads(head_path.read_bytes())["signer_did"] != signer:
        raise ValueError("publish-state.json publisher_did differs from the signed set")
    if state.get("last_release_cid") != head.get("latest_release_cid") or state.get("last_version") != release.get("version"):
        raise ValueError("publish-state.json release receipt differs from the signed set")
    if not isinstance(state.get("last_head_cid"), str) or not state["last_head_cid"]:
        raise ValueError("publish-state.json requires last_head_cid")
    if type(state.get("last_published_at")) is not int or state["last_published_at"] < 0:
        raise ValueError("publish-state.json requires last_published_at")
    regular(install_path, "install.sh")
    if head.get("release_sha256") != digest(release_bytes):
        raise ValueError("release-head.json does not bind these release.json bytes")
    for field in ("version", "channel"):
        if not isinstance(head.get(field), str) or not head[field] or release.get(field) != head[field]:
            raise ValueError(f"release-head.json and release.json {field} differ")
    if artifact_root.is_symlink() or not artifact_root.is_dir():
        raise ValueError(f"artifact directory must be a regular directory: {artifact_root}")
    present = {}
    for entry in artifact_root.iterdir():
        if (entry.name.startswith(".") or "\\" in entry.name or entry.is_symlink()
                or not stat.S_ISREG(entry.lstat().st_mode)):
            raise ValueError(f"artifact entries must be plain regular files: {entry.name}")
        present[entry.name] = entry
    platforms = release.get("platforms")
    if not isinstance(platforms, dict) or not platforms:
        raise ValueError("release.json advertises no platforms")
    referenced = set()
    errors = []
    for platform, descriptor in sorted(platforms.items()):
        setup = next((s for s, r in integrity.RELEASE_PLATFORMS.items() if r == platform), None)
        if setup is None:
            raise ValueError(f"release.json advertises an unknown platform: {platform}")
        for kind, name in (("binary", f"elastos-{platform}"), ("components", f"components-{platform}.json")):
            if name not in present:
                raise ValueError(f"advertised artifact is missing: {name}")
            data = present[name].read_bytes()
            info = descriptor.get(kind) if isinstance(descriptor, dict) else None
            if not isinstance(info, dict) or info.get("sha256") != digest(data) or info.get("size") != len(data):
                raise ValueError(f"advertised {kind} differs from its bytes: {name}")
            referenced.add(name)
        try:
            manifest = json.loads(present[f"components-{platform}.json"].read_bytes())
        except ValueError as exc:
            raise ValueError(f"components-{platform}.json is not valid JSON: {exc}")
        errors += integrity.audit_release_artifacts(manifest, [setup], artifact_root)
        for component in (manifest.get("external") or {}).values():
            if not isinstance(component, dict):
                continue
            for entry in (component, component.get("capsule_metadata")):
                if isinstance(entry, dict):
                    _, info = integrity.resolve_platform_info(entry, setup)
                    if isinstance(info, dict) and isinstance(info.get("release_path"), str):
                        referenced.add(info["release_path"])
        for name, entry in (manifest.get("capsules") or {}).items():
            if isinstance(entry, dict) and platform in (entry.get("platforms") or []):
                referenced.add(f"{name}-{platform}.capsule.tar.gz")
    if errors:
        raise ValueError("; ".join(errors))
    extra = sorted(set(present) - referenced)
    pin = None
    for name in present:
        if name.startswith("components-") and name.endswith(".json"):
            try:
                candidate = json.loads(present[name].read_bytes()).get("model_catalog")
            except ValueError as exc:
                raise ValueError(f"{name} is not valid JSON: {exc}")
            if pin is None:
                pin = candidate
            elif pin != candidate:
                raise ValueError("prepared components disagree on model_catalog")
    if isinstance(pin, dict):
        head = pin.get("head_cid")
        if not isinstance(head, str) or not head:
            raise ValueError("model_catalog.head_cid is required")
        if "model-catalog.json" not in present:
            raise ValueError("advertised model catalog pin is missing model-catalog.json")
        data = present["model-catalog.json"].read_bytes()
        digest = hashlib.sha256(data).digest()
        actual = "b" + __import__("base64").b32encode(b"\x01\x55\x12\x20" + digest).decode("ascii").lower().rstrip("=")
        if actual != head:
            raise ValueError(f"model-catalog.json head {actual} does not match pin {head}")
        referenced.add("model-catalog.json")
        extra = sorted(set(present) - referenced)
    if extra:
        raise ValueError(f"prepared artifacts are not advertised by this release: {extra}")
except (OSError, ValueError, TypeError, AttributeError) as exc:
    raise SystemExit(f"Release publication set rejected: {exc}")
PY
}

reject_release_publication_collision() {
    if [[ -L "$1" ]] || [[ -e "$1" && ! -f "$1" ]]; then
        die "Publisher destination must be a regular file or absent: $1"
    fi
}

check_release_publication_destinations() {
    local publisher_root="$1" artifact_dir="$2"
    local source destination
    if [[ -L "$publisher_root" ]] || [[ -e "$publisher_root" && ! -d "$publisher_root" ]]; then
        die "Publisher root is not a directory: ${publisher_root}"
    fi
    if [[ -L "${publisher_root}/artifacts" ]] || \
       [[ -e "${publisher_root}/artifacts" && ! -d "${publisher_root}/artifacts" ]]; then
        die "Publisher artifacts path must be a regular directory: ${publisher_root}/artifacts"
    fi
    for source in "${artifact_dir}"/*; do
        reject_release_publication_collision "${publisher_root}/artifacts/$(basename "$source")"
    done
    for destination in install.sh release.json publish-state.json release-head.json; do
        reject_release_publication_collision "${publisher_root}/${destination}"
    done
}

copy_release_publication_file() {
    cp "$1" "$2" || return
    cmp -s "$1" "$2"
}

verify_release_publication_space() {
    python3 - "$@" <<'PY'
import shutil
import sys
from pathlib import Path

root, head, release, install, artifacts, state = (Path(path) for path in sys.argv[1:])
required = sum(path.stat().st_size for path in artifacts.iterdir())
required += sum(path.stat().st_size for path in (head, release, install, state))
previous_head = root / "release-head.json"
if previous_head.is_file():
    required += previous_head.stat().st_size
while not root.exists():
    root = root.parent
disk = shutil.disk_usage(root)
if disk.free < required:
    raise SystemExit("Release publication staging needs more free space than the volume has")
PY
}

stage_release_publication() {
    local scratch="$1" head="$2" release="$3" install="$4" artifact_dir="$5" state="$6"
    local publisher_root="$7"
    local source
    mkdir -p "${scratch}/staged/artifacts" "${scratch}/previous/artifacts" || return
    for source in "${artifact_dir}"/*; do
        copy_release_publication_file "$source" "${scratch}/staged/artifacts/$(basename "$source")" || return
    done
    copy_release_publication_file "$install" "${scratch}/staged/install.sh" || return
    copy_release_publication_file "$release" "${scratch}/staged/release.json" || return
    copy_release_publication_file "$state" "${scratch}/staged/publish-state.json" || return
    copy_release_publication_file "$head" "${scratch}/staged/release-head.json" || return
    if [[ -f "${publisher_root}/release-head.json" ]]; then
        copy_release_publication_file "${publisher_root}/release-head.json" "${scratch}/recovered-head.json" || return
    fi
}

# The current file keeps a hard link under the attempt's scratch so a failed
# rename can put it back without a moment where the path is absent. The path
# is recorded as pending before its rename, so restore always sees a replaced
# file even when bookkeeping fails afterwards.
promote_release_publication_file() {
    local scratch="$1" relative="$2" destination="$3"
    if [[ -f "$destination" ]]; then
        ln "$destination" "${scratch}/previous/${relative}" || return
    fi
    printf '%s\n' "$relative" >> "${scratch}/promoted" || return
    mv -f "${scratch}/staged/${relative}" "$destination"
}

promote_release_publication() {
    local scratch="$1" publisher_root="$2"
    local staged relative
    : > "${scratch}/promoted" || return
    for staged in "${scratch}/staged/artifacts"/* "${scratch}/staged/install.sh" \
        "${scratch}/staged/release.json" "${scratch}/staged/publish-state.json" "${scratch}/staged/release-head.json"; do
        relative="${staged#"${scratch}/staged/"}"
        promote_release_publication_file "$scratch" "$relative" "${publisher_root}/${relative}" || return
    done
}

restore_release_publication() {
    local scratch="$1" publisher_root="$2"
    local relative previous status=0
    [[ -f "${scratch}/promoted" ]] || return 0
    while IFS= read -r relative; do
        previous="${scratch}/previous/${relative}"
        if [[ -f "$previous" ]]; then
            # A pending path whose rename never ran still shares the saved inode.
            if [[ ! "$previous" -ef "${publisher_root}/${relative}" ]]; then
                mv -f "$previous" "${publisher_root}/${relative}" || status=1
            fi
        else
            rm -f "${publisher_root}/${relative}" || status=1
        fi
    done < "${scratch}/promoted"
    [[ "$status" -eq 0 ]] || return "$status"
    # Remove every backup link before the final head activates the restored set.
    # A fresh inode makes a refused gateway snapshot run admission again.
    rm -rf "${scratch}/previous" || return
    if [[ -f "${scratch}/recovered-head.json" ]]; then
        cmp -s "${scratch}/recovered-head.json" "${publisher_root}/release-head.json" || return
        mv -f "${scratch}/recovered-head.json" "${publisher_root}/release-head.json" || return
    fi
    return 0
}

export_release_publication() {
    local publisher_root="$1" head="$2" release="$3" install="$4" artifact_dir="$5" state="$6"
    local scratch stale
    verify_release_publication_set "$head" "$release" "$install" "$artifact_dir" "$state" || return
    check_release_publication_destinations "$publisher_root" "$artifact_dir" || return
    verify_release_publication_space "$publisher_root" "$head" "$release" "$install" "$artifact_dir" "$state" || return
    mkdir -p "${publisher_root}/artifacts" || return
    for stale in "${publisher_root}"/.publish-release.*; do
        if [[ -e "$stale" ]]; then
            warn "Staging from an interrupted publication attempt remains: ${stale}"
        fi
    done
    scratch=$(mktemp -d "${publisher_root}/.publish-release.XXXXXX") || return
    if ! stage_release_publication "$scratch" "$head" "$release" "$install" "$artifact_dir" "$state" "$publisher_root"; then
        rm -rf "$scratch"
        die "Failed to stage the release publication set; the current publication is unchanged"
    fi
    if ! promote_release_publication "$scratch" "$publisher_root"; then
        if restore_release_publication "$scratch" "$publisher_root" && rm -rf "$scratch"; then
            die "Failed to promote the release publication set; the previous publication was restored"
        fi
        die "Failed to promote the release publication set and could not complete recovery; inspect ${scratch}"
    fi
    rm -rf "$scratch"
}

# Sourcing exposes local builders without invoking the publisher.
if [[ "${BASH_SOURCE[0]}" != "$0" ]]; then
    return 0
fi

# ── Parse args ────────────────────────────────────────────────────────

VERSION=""
KEY_PATH=""
PREPARE_OUTPUT=""
PREPARE_PUBLISHER_DID=""
IPFS_PROVIDER_BIN=""
CHANNEL="stable"
SKIP_BUILD=false
SKIP_ROOTFS=false
PUBLISH_PUBLIC_URL=true
PUBLIC_WITH_SUDO=false
ALLOW_SIGNER_ROTATION=false
GATEWAY_ADDR="127.0.0.1:8090"
PUBLIC_URL_TIMEOUT=60
CROSS_ARCH=""
PLATFORM_INPUTS=()
PREVIEW_PLATFORM=""
CAPSULES_EXPLICIT=false
STATE_DIR="${ELASTOS_PUBLISH_STATE_DIR:-.}"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --help|-h) show_help ;;
        --version)
            [[ -z "${2:-}" ]] && die "Usage: --version X.Y.Z"
            VERSION="$2"; shift 2 ;;
        --key|--key=*)
            die "Shell release signing is retired; use --prepare-only with public inputs and the separate custodian signer" ;;
        --prepare-only)
            [[ -n "${2:-}" ]] || die "Usage: --prepare-only DIR"
            PREPARE_OUTPUT="$2"
            [[ "$PREPARE_OUTPUT" == /* ]] || PREPARE_OUTPUT="$PUBLISH_CALLER_DIR/$PREPARE_OUTPUT"
            shift 2 ;;
        --publisher-did)
            [[ -n "${2:-}" ]] || die "Usage: --publisher-did DID"
            PREPARE_PUBLISHER_DID="$2"; shift 2 ;;
        --ipfs-provider-bin)
            [[ -z "${2:-}" ]] && die "Usage: --ipfs-provider-bin PATH"
            IPFS_PROVIDER_BIN="$2"; shift 2 ;;
        --channel)
            [[ -z "${2:-}" ]] && die "Usage: --channel name"
            CHANNEL="$2"; shift 2 ;;
        --skip-build) SKIP_BUILD=true; shift ;;
        --skip-rootfs) SKIP_ROOTFS=true; shift ;;
        --dry-run) die "Use elastos publish-release --dry-run for a read-only plan. This shell supports unsigned --prepare-only inputs." ;;
        --no-public-url) PUBLISH_PUBLIC_URL=false; shift ;;
        --public-with-sudo) PUBLIC_WITH_SUDO=true; shift ;;
        --allow-signer-rotation) ALLOW_SIGNER_ROTATION=true; shift ;;
        --gateway-addr)
            [[ -z "${2:-}" ]] && die "Usage: --gateway-addr HOST:PORT"
            GATEWAY_ADDR="$2"; shift 2 ;;
        --public-timeout)
            [[ -z "${2:-}" ]] && die "Usage: --public-timeout SECONDS"
            PUBLIC_URL_TIMEOUT="$2"; shift 2 ;;
        --cross)
            [[ -z "${2:-}" ]] && die "Usage: --cross ARCH (e.g., aarch64)"
            CROSS_ARCH="$2"; shift 2 ;;
        --platform-input)
            [[ -z "${2:-}" ]] && die "Usage: --platform-input PLATFORM=DIR"
            input_name="${2%%=*}"
            input_path="${2#*=}"
            [[ "$2" == *=* && -n "$input_path" ]] || die "Usage: --platform-input PLATFORM=DIR"
            [[ "$input_path" == /* ]] || input_path="$PUBLISH_CALLER_DIR/$input_path"
            PLATFORM_INPUTS+=("${input_name}=${input_path}"); shift 2 ;;
        --preview-platform)
            [[ -z "${2:-}" ]] && die "Usage: --preview-platform PLATFORM"
            PREVIEW_PLATFORM="$2"; shift 2 ;;
        --capsules)
            CAPSULES_EXPLICIT=true
            [[ -z "${2:-}" ]] && die "Usage: --capsules name1,name2,..."
            IFS=',' read -r -a CAPSULES <<< "$2"
            shift 2 ;;
        *) die "Unknown option: $1. Run --help for usage." ;;
    esac
done

# ── Preflight ─────────────────────────────────────────────────────────

[[ -n "$PREPARE_OUTPUT" ]] || die "Shell publication is retired; use --prepare-only and publish the signed set with elastos publish-release"

[[ -z "$VERSION" ]] && die "--version is required"
bash "./scripts/check-versioning.sh" "$VERSION"
if ! is_allowed_channel "$CHANNEL"; then
    die "Unsupported release channel '${CHANNEL}'. Allowed channels: ${ALLOWED_CHANNELS[*]}"
fi
export ELASTOS_RELEASE_VERSION="$VERSION"

for cmd in jq python3 curl; do
    command -v "$cmd" &>/dev/null || die "Required tool not found: $cmd"
done

sha256 /dev/null &>/dev/null || die "No SHA-256 tool available"

if [[ -n "$PREPARE_OUTPUT" ]]; then
    [[ -z "$KEY_PATH" && "$ALLOW_SIGNER_ROTATION" == false ]] || die "unsigned preparation accepts public inputs only; the custodian owns signing and signer changes"
    [[ ${#PLATFORM_INPUTS[@]} -gt 0 && -n "$PREPARE_PUBLISHER_DID" ]] || die "unsigned preparation requires --platform-input and --publisher-did"
    [[ ! -e "$PREPARE_OUTPUT" && ! -L "$PREPARE_OUTPUT" && -d "$(dirname "$PREPARE_OUTPUT")" ]] || die "unsigned output must be a new directory under an existing parent"
    PUBLISH_PUBLIC_URL=false
    PUBLIC_WITH_SUDO=false
elif [[ -n "$PREPARE_PUBLISHER_DID" ]]; then
    die "--publisher-did requires --prepare-only"
fi

if [[ ${#PLATFORM_INPUTS[@]} -gt 0 ]]; then
    [[ "$SKIP_BUILD" == false && "$SKIP_ROOTFS" == false && -z "$CROSS_ARCH" && "$CAPSULES_EXPLICIT" == false ]] \
        || die "--platform-input conflicts with --skip-build, --skip-rootfs, --cross and --capsules"
fi
# A preview prepares one explicitly named native input on the canary channel
# from that platform's own host. Stable admission keeps every supplied native
# input when at least two release platforms are present.
PREVIEW_ARGS=()
if [[ -n "$PREVIEW_PLATFORM" ]]; then
    [[ "$CHANNEL" == canary ]] || die "--preview-platform requires --channel canary"
    [[ ${#PLATFORM_INPUTS[@]} -eq 1 && "${PLATFORM_INPUTS[0]%%=*}" == "$PREVIEW_PLATFORM" ]] \
        || die "--preview-platform ${PREVIEW_PLATFORM} requires exactly one --platform-input ${PREVIEW_PLATFORM}=DIR"
    PREVIEW_ARGS=(--preview-platform "$PREVIEW_PLATFORM")
fi
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT
PREPARED_INPUT_ROOT=""
if [[ ${#PLATFORM_INPUTS[@]} -gt 0 ]]; then
    PREPARED_INPUT_ROOT="${TMPDIR}/native-inputs"
    case "$(uname -s):$(uname -m)" in
        Linux:x86_64) PLATFORM=x86_64-linux; SETUP_PLATFORM=linux-amd64 ;;
        Linux:aarch64|Linux:arm64) PLATFORM=aarch64-linux; SETUP_PLATFORM=linux-arm64 ;;
        Darwin:arm64|Darwin:aarch64) PLATFORM=aarch64-darwin; SETUP_PLATFORM=darwin-arm64 ;;
        *) die "Prepared input publication requires a supported native coordinator" ;;
    esac
    if [[ -n "$PREVIEW_PLATFORM" && "$PLATFORM" != "$PREVIEW_PLATFORM" ]]; then
        die "--preview-platform ${PREVIEW_PLATFORM} must be published from a ${PREVIEW_PLATFORM} host (this host is ${PLATFORM})"
    fi
    stage_platform_inputs "$PREPARED_INPUT_ROOT" "${PLATFORM_INPUTS[@]}"
    jq -e --arg p "$PLATFORM" '.platforms | index($p)' "$PREPARED_INPUT_ROOT/assembly.json" >/dev/null \
        || die "prepared inputs do not include this host platform: ${PLATFORM}"
    ELASTOS="$PREPARED_INPUT_ROOT/artifacts/elastos-${PLATFORM}"
    HOST_DATA_DIR="$(default_elastos_data_dir)"
    if [[ -z "$IPFS_PROVIDER_BIN" ]]; then
        IPFS_PROVIDER_BIN="$PREPARED_INPUT_ROOT/artifacts/ipfs-provider-${SETUP_PLATFORM}"
    fi
fi

if [[ -z "$IPFS_PROVIDER_BIN" ]]; then
    IPFS_PROVIDER_BIN=$(find_ipfs_provider_binary || true)
fi
[[ -z "$IPFS_PROVIDER_BIN" ]] && die "ipfs-provider binary not found. Build/install it first."
[[ ! -x "$IPFS_PROVIDER_BIN" ]] && die "ipfs-provider binary is not executable: $IPFS_PROVIDER_BIN"
export ELASTOS_IPFS_PROVIDER_BIN="$IPFS_PROVIDER_BIN"


echo ""
echo -e "${BOLD}ElastOS unsigned release preparation${NC}"
echo -e "${DIM}  Version:  ${VERSION}${NC}"
echo -e "${DIM}  Channel:  ${CHANNEL}${NC}"
echo -e "${DIM}  IPFS provider: ${IPFS_PROVIDER_BIN}${NC}"
echo -e "${DIM}  Capsules: ${CAPSULES[*]}${NC}"
echo ""

info "Importing the admitted native inputs..."
publish_prepared_platform_inputs "$PREPARED_INPUT_ROOT"
prepare_release_signing_input "$PREPARED_INPUT_ROOT" "$PREPARE_OUTPUT" "$PREPARE_PUBLISHER_DID"
info "Unsigned input is ready for operator approval and the separately installed custodian signer."
