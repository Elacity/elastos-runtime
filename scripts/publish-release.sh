#!/usr/bin/env bash
#
# Publish an ElastOS release — raw installer CIDs plus content-object sidecars.
# Prefer `elastos publish-release`; this script remains the low-level implementation.
#
# Usage:
#   ./scripts/publish-release.sh --version 0.10.0 --key path/to/release.key
#   ./scripts/publish-release.sh --version 0.10.0 --key release.key --channel canary
#   ./scripts/publish-release.sh --version 0.10.0 --key release.key --skip-build
#   ./scripts/publish-release.sh --version 0.10.0 --key release.key --capsules chat,chat-wasm
#   ./scripts/publish-release.sh --version 0.10.0 --key release.key --no-public-url
#   ./scripts/publish-release.sh --version 0.10.0 --key release.key --public-with-sudo
#   ./scripts/publish-release.sh --help
#
# Prerequisites: jq, sha256sum or shasum, ipfs-provider capsule binary, elastos binary
#
# Flow:
#   1. Build the elastos binary (and capsules only when rootfs artifacts are being rebuilt)
#   2. Build rootfs for each capsule (scripts/build/build-rootfs.sh)
#   3. Package each: capsule.json + rootfs.ext4 → <name>.capsule.tar.gz
#   4. ipfs-provider add each capsule artifact → get CIDs + sha256 + size
#   5. Build and publish direct share/open support assets
#   6. Generate components.json with CIDs + checksums
#   7. ipfs add elastos binary → binary CID
#   8. ipfs add components.json → components CID
#   9. Create release.json v1 (binary CID + components CID + shell CID)
#  10. Sign release.json + create release-head.json
#  11. Publish raw CIDs for installer compatibility plus release/head objects
#      with `_elastos_object.json` links for SmartWeb traversal
#

set -euo pipefail

BOLD='\033[1m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
RED='\033[0;31m'
DIM='\033[2m'
NC='\033[0m'

# Default publish scope: runtime core + chat demo (3 modes: native, microVM, WASM).
# ipfs-provider, availability-provider, wallet-provider, object-provider,
# content-block-graph-provider, drm-provider, rights-provider, key-provider,
# decrypt-provider, and tunnel-provider are supported direct command assets.
# They are not part of the managed user runtime, but fresh installs must
# provision them for share/open/public-share.
DEFAULT_CAPSULES=(
    shell
    localhost-provider
    chat
    chat-wasm
    did-provider
    tunnel-provider
)
CAPSULES=("${DEFAULT_CAPSULES[@]}")
REQUIRED_SUPPORTED_CAPSULES=(
    shell
    localhost-provider
    chat
    did-provider
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
    chain-provider
    wallet-provider
    object-provider
    content-block-graph-provider
    drm-provider
    rights-provider
    key-provider
    decrypt-provider
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

# Navigate to project root
cd "$(dirname "${BASH_SOURCE[0]}")/.."

# ── Help ──────────────────────────────────────────────────────────────

show_help() {
    echo ""
    echo -e "${BOLD}ElastOS Release Publisher (Capsule-Native)${NC}"
    echo ""
    echo -e "${BOLD}Usage:${NC}"
    echo "  ./scripts/publish-release.sh --version X.Y.Z --key path/to/release.key"
    echo ""
    echo -e "${BOLD}Required:${NC}"
    echo "  --version X.Y.Z    Runtime release version (see docs/VERSIONING.md)"
    echo "  --key PATH         Ed25519 signing key (hex-encoded, 32 bytes)"
    echo ""
    echo -e "${BOLD}Optional:${NC}"
    echo "  --ipfs-provider-bin PATH  Path to ipfs-provider binary (optional auto-detect)"
    echo "  --channel NAME     Release channel (stable | canary | jetson-test; default: stable)"
    echo "  --skip-build       Skip building binaries (use existing artifacts)"
    echo "  --skip-rootfs      Skip rootfs building (reuse existing .capsule.tar.gz and skip capsule rebuilds)"
    echo "  --capsules CSV     Override publish capsule list (default: demo capsule set)"
    echo "  --no-public-url    Skip auto-starting gateway+tunnel and URL emission"
    echo "  --public-with-sudo Start auto-public gateway+tunnel via sudo"
    echo "  --allow-signer-rotation  Allow signer DID to differ from the current canonical publisher signer"
    echo "  --gateway-addr     Gateway listen addr for auto-public URL (default: 127.0.0.1:8090)"
    echo "  --cross ARCH       Also cross-compile for ARCH (e.g., aarch64). Creates multi-platform release."
    echo "  --public-timeout   Seconds to wait for trycloudflare URL (default: 60)"
    echo "  --help             Show this help"
    echo ""
    echo -e "${BOLD}Output:${NC}"
    echo "  Publishes per-capsule artifacts + runtime binary as raw CIDs."
    echo "  Creates signed release.json/release-head.json plus content-object sidecars."
    echo "  Installer downloads only: binary + components.json (2 files)."
    echo "  Use --cross aarch64 when publishing from x86_64 for Jetson installs."
    echo ""
    echo -e "${BOLD}Trust model:${NC}"
    echo "  All artifacts signed with Ed25519. Release head is the installer's"
    echo "  trust root. Capsules downloaded on-demand by supervisor. Gateways"
    echo "  are transport only; signatures are trust."
    echo ""
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
    if [[ -n "${XDG_DATA_HOME:-}" ]]; then
        printf '%s\n' "${XDG_DATA_HOME%/}/elastos"
        return
    fi
    printf '%s\n' "${HOME}/.local/share/elastos"
}

discover_source_bootstrap_json() {
    local data_dir
    data_dir="$(default_elastos_data_dir)"
    DATA_DIR="${data_dir}" \
    COORDS_PATH="${ELASTOS_RUNTIME_COORDS_FILE:-${data_dir}/runtime-coords.json}" \
    EXPECTED_VERSION="${ELASTOS_SOURCE_EXPECTED_VERSION:-${VERSION:-}}" \
    python3 - <<'PY'
import json
import os
import pathlib
import urllib.request

coords_path = pathlib.Path(os.environ["COORDS_PATH"])
if not coords_path.exists():
    print("{}")
    raise SystemExit(0)

try:
    coords = json.loads(coords_path.read_text())
    api = coords["api_url"].rstrip("/")
    secret = coords["attach_secret"]
except Exception:
    print("{}")
    raise SystemExit(0)

version = ""
try:
    with urllib.request.urlopen(api + "/api/health", timeout=2) as resp:
        health = json.loads(resp.read().decode())
    version = health.get("version", "") or ""
except Exception:
    version = ""

def attach(scope: str) -> str:
    req = urllib.request.Request(
        api + "/api/auth/attach",
        data=json.dumps({"secret": secret, "scope": scope}).encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=5) as resp:
        body = json.loads(resp.read().decode())
    return body.get("token", "")

try:
    shell_token = attach("shell")
    if not shell_token:
        print("{}")
        raise SystemExit(0)
    req = urllib.request.Request(
        api + "/api/provider/peer/get_ticket",
        data=b"{}",
        headers={
            "Authorization": "Bearer " + shell_token,
            "Content-Type": "application/json",
        },
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=5) as resp:
        body = json.loads(resp.read().decode())
    data = body.get("data") or {}
    print(json.dumps({
        "ticket": data.get("ticket", ""),
        "node_id": data.get("node_id", ""),
        "version": version,
    }))
except Exception:
    print("{}")
PY
}

canonical_publisher_gateway() {
    printf '%s\n' "${ELASTOS_CANONICAL_PUBLISHER_GATEWAY:-https://elastos.elacitylabs.com}"
}

inspect_signer_did() {
    cargo run -q -p elastos-server --manifest-path "elastos/Cargo.toml" -- publish-release \
        --version "$VERSION" \
        --channel "$CHANNEL" \
        --key "$KEY_PATH" \
        --dry-run 2>/dev/null \
        | sed -n 's/^  Signer:[[:space:]]*//p' \
        | head -n1
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
            (cd "$capsule_dir" && "${CROSS_ENV[@]}" cross build --release --target "$target") >&2
        else
            (cd "$capsule_dir" && cargo build --release --target "$target") >&2
        fi
    else
        (cd "$capsule_dir" && cargo build --release) >&2
    fi

    binary=$(support_binary_build_path "$name" "$target")
    [[ -x "$binary" ]] || die "${name} binary missing after build for ${platform}: ${binary}"
    echo "$binary"
}

build_packaged_capsule_archive() {
    local platform="$1"
    local capsule_name="$2"
    local capsule_dir stage_root archive
    local required_files=()

    capsule_dir=$(resolve_capsule_dir "$capsule_name" || true)
    [[ -n "$capsule_dir" ]] || die "${capsule_name} source directory not found"
    [[ -f "${capsule_dir}/capsule.json" ]] || die "${capsule_name} capsule manifest not found at ${capsule_dir}/capsule.json"

    case "$capsule_name" in
        documents|library|inbox)
            required_files=(capsule.json index.html)
            ;;
        chat-wasm)
            required_files=(chat-stdio.wasm)
            ;;
        gba-emulator)
            required_files=(index.html emulator.js favicon.svg mgba.js mgba.wasm style.css)
            ;;
        gba-ucity)
            required_files=(ucity.gba)
            ;;
        *)
            die "Unsupported static capsule archive request: ${capsule_name}"
            ;;
    esac

    for file in "${required_files[@]}"; do
        [[ -f "${capsule_dir}/${file}" ]] || die "${capsule_name} asset missing at ${capsule_dir}/${file}"
    done

    stage_root="${TMPDIR}/support-assets-${platform}-${capsule_name}"
    archive="${TMPDIR}/support-assets-${platform}/${capsule_name}.tar.gz"
    rm -rf "$stage_root"
    mkdir -p "${stage_root}/${capsule_name}" "$(dirname "$archive")"
    for file in "${required_files[@]}"; do
        mkdir -p "${stage_root}/${capsule_name}/$(dirname "$file")"
        cp "${capsule_dir}/${file}" "${stage_root}/${capsule_name}/${file}"
    done
    tar -czf "$archive" -C "$stage_root" "$capsule_name"
    echo "$archive"
}

build_home_cli_archive() {
    local platform="$1"
    local home_cli_dir stage_root archive
    home_cli_dir=$(resolve_capsule_dir "home-cli" || true)
    [[ -n "$home_cli_dir" ]] || die "home-cli source directory not found"
    [[ -f "${home_cli_dir}/capsule.json" ]] || die "home-cli capsule manifest not found at ${home_cli_dir}/capsule.json"

    ensure_rust_target_installed "wasm32-wasip1"
    info "  Building home-cli (wasm32-wasip1)..." >&2
    (cd "$home_cli_dir" && cargo build --target wasm32-wasip1 --release) >&2
    [[ -f "${home_cli_dir}/target/wasm32-wasip1/release/home-cli.wasm" ]] || die "home-cli.wasm missing after build"

    stage_root="${TMPDIR}/support-assets-${platform}"
    archive="${stage_root}/home-cli.tar.gz"
    rm -rf "${stage_root}/home-cli"
    mkdir -p "${stage_root}/home-cli"
    cp "${home_cli_dir}/capsule.json" "${stage_root}/home-cli/"
    cp "${home_cli_dir}/target/wasm32-wasip1/release/home-cli.wasm" "${stage_root}/home-cli/"
    tar -czf "$archive" -C "$stage_root" home-cli
    echo "$archive"
}

build_browser_wasm_capsule_archive() {
    local platform="$1"
    local capsule_name="$2"
    local capsule_dir wasm_name stage_root archive

    capsule_dir=$(resolve_capsule_dir "$capsule_name" || true)
    [[ -n "$capsule_dir" ]] || die "${capsule_name} source directory not found"
    [[ -f "${capsule_dir}/capsule.json" ]] || die "${capsule_name} capsule manifest not found at ${capsule_dir}/capsule.json"
    [[ -d "${capsule_dir}/browser" ]] || die "${capsule_name} browser assets missing at ${capsule_dir}/browser"

    wasm_name="${capsule_name}.wasm"
    ensure_rust_target_installed "wasm32-wasip1"
    info "  Building ${capsule_name} (wasm32-wasip1)..." >&2
    (cd "$capsule_dir" && cargo build --target wasm32-wasip1 --release) >&2
    [[ -f "${capsule_dir}/target/wasm32-wasip1/release/${wasm_name}" ]] || die "${wasm_name} missing after build"

    stage_root="${TMPDIR}/support-assets-${platform}"
    archive="${stage_root}/${capsule_name}.tar.gz"
    rm -rf "${stage_root}/${capsule_name}"
    mkdir -p "${stage_root}/${capsule_name}"
    cp "${capsule_dir}/capsule.json" "${stage_root}/${capsule_name}/"
    cp "${capsule_dir}/target/wasm32-wasip1/release/${wasm_name}" "${stage_root}/${capsule_name}/"
    cp -R "${capsule_dir}/browser" "${stage_root}/${capsule_name}/browser"
    tar -czf "$archive" -C "$stage_root" "$capsule_name"
    echo "$archive"
}

build_chat_archive() {
    local platform="$1"
    local use_cross="${2:-false}"
    local artifacts_dir artifact stage_root archive

    if [[ "$use_cross" == true ]]; then
        artifacts_dir="${CROSS_ARTIFACTS_DIR:-}"
    else
        artifacts_dir="${ARTIFACTS_DIR:-}"
    fi

    [[ -n "$artifacts_dir" ]] || die "chat archive requested before rootfs artifacts were prepared"
    artifact="${artifacts_dir}/chat.capsule.tar.gz"
    [[ -f "$artifact" ]] || die "chat capsule artifact missing at ${artifact}"

    stage_root="${TMPDIR}/support-assets-${platform}"
    archive="${stage_root}/chat.tar.gz"
    rm -rf "${stage_root}/chat"
    mkdir -p "${stage_root}/chat"
    tar -xzf "$artifact" -C "${stage_root}/chat"
    [[ -f "${stage_root}/chat/capsule.json" ]] || die "chat archive staging missing capsule.json after extraction"
    [[ -f "${stage_root}/chat/rootfs.ext4" ]] || die "chat archive staging missing rootfs.ext4 after extraction"
    tar -czf "$archive" -C "$stage_root" chat
    echo "$archive"
}

record_direct_asset() {
    local updates_json="$1"
    local name="$2"
    local staged="$3"
    local install_path="$4"
    local release_path="$5"
    local extract_path="${6:-}"
    local cid checksum size

    cid=$(ipfs_add "$staged")
    checksum=$(sha256 "$staged")
    size=$(file_size "$staged")

    if [[ -n "$extract_path" ]]; then
        echo "$updates_json" | jq \
            --arg name "$name" \
            --arg cid "$cid" \
            --arg checksum "sha256:${checksum}" \
            --arg install_path "$install_path" \
            --arg extract_path "$extract_path" \
            --arg release_path "$release_path" \
            --argjson size "$size" \
            '.[$name] = {cid: $cid, checksum: $checksum, size: $size, install_path: $install_path, extract_path: $extract_path, release_path: $release_path}'
    else
        echo "$updates_json" | jq \
            --arg name "$name" \
            --arg cid "$cid" \
            --arg checksum "sha256:${checksum}" \
            --arg install_path "$install_path" \
            --arg release_path "$release_path" \
            --argjson size "$size" \
            '.[$name] = {cid: $cid, checksum: $checksum, size: $size, install_path: $install_path, release_path: $release_path}'
    fi
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

build_supported_direct_assets() {
    local platform="$1"
    local setup_platform="$2"
    local target="${3:-}"
    local use_cross="${4:-false}"
    local stage_dir updates_json name binary staged install_path release_path

    stage_dir="${TMPDIR}/supported-assets-${platform}"
    mkdir -p "$stage_dir"
    updates_json='{}'

    for name in "${SUPPORT_BINARY_ASSETS[@]}"; do
        binary=$(build_support_binary "$name" "$platform" "$target" "$use_cross")
        release_path="${name}-${setup_platform}"
        staged="${stage_dir}/${release_path}"
        cp "$binary" "$staged"
        install_path="bin/${name}"
        updates_json=$(record_direct_asset "$updates_json" "$name" "$staged" "$install_path" "$release_path")
    done

    local chat_archive chat_staged
    chat_archive=$(build_chat_archive "$platform" "$use_cross")
    release_path="chat-${setup_platform}.tar.gz"
    chat_staged="${stage_dir}/${release_path}"
    cp "$chat_archive" "$chat_staged"
    updates_json=$(record_direct_asset "$updates_json" "chat" "$chat_staged" "capsules/chat" "$release_path" "chat")

    stamp_direct_assets "$setup_platform" "$updates_json"
}

build_platform_independent_direct_assets() {
    local platform="$1"
    local stage_dir updates_json release_path
    local archive staged capsule

    stage_dir="${TMPDIR}/supported-assets-universal"
    mkdir -p "$stage_dir"
    updates_json='{}'

    for capsule in documents library inbox chat-wasm gba-emulator gba-ucity; do
        archive=$(build_packaged_capsule_archive "$platform" "$capsule")
        release_path="${capsule}.tar.gz"
        staged="${stage_dir}/${release_path}"
        cp "$archive" "$staged"
        updates_json=$(record_direct_asset "$updates_json" "$capsule" "$staged" "capsules/${capsule}" "$release_path" "$capsule")
    done

    local home_cli_archive home_cli_staged
    home_cli_archive=$(build_home_cli_archive "$platform")
    release_path="home-cli.tar.gz"
    home_cli_staged="${stage_dir}/${release_path}"
    cp "$home_cli_archive" "$home_cli_staged"
    updates_json=$(record_direct_asset "$updates_json" "home-cli" "$home_cli_staged" "capsules/home-cli" "$release_path" "home-cli")

    for capsule in home system chat-room; do
        archive=$(build_browser_wasm_capsule_archive "$platform" "$capsule")
        release_path="${capsule}.tar.gz"
        staged="${stage_dir}/${release_path}"
        cp "$archive" "$staged"
        updates_json=$(record_direct_asset "$updates_json" "$capsule" "$staged" "capsules/${capsule}" "$release_path" "$capsule")
    done

    stamp_direct_assets "*" "$updates_json"
}

merge_direct_assets() {
    jq -s '.[0] * .[1]' <(printf '%s\n' "$1") <(printf '%s\n' "$2")
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

# ── Parse args ────────────────────────────────────────────────────────

VERSION=""
KEY_PATH=""
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
STATE_DIR="${ELASTOS_PUBLISH_STATE_DIR:-.}"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --help|-h) show_help ;;
        --version)
            [[ -z "${2:-}" ]] && die "Usage: --version X.Y.Z"
            VERSION="$2"; shift 2 ;;
        --key)
            [[ -z "${2:-}" ]] && die "Usage: --key path/to/release.key"
            KEY_PATH="$2"; shift 2 ;;
        --ipfs-provider-bin)
            [[ -z "${2:-}" ]] && die "Usage: --ipfs-provider-bin PATH"
            IPFS_PROVIDER_BIN="$2"; shift 2 ;;
        --channel)
            [[ -z "${2:-}" ]] && die "Usage: --channel name"
            CHANNEL="$2"; shift 2 ;;
        --skip-build) SKIP_BUILD=true; shift ;;
        --skip-rootfs) SKIP_ROOTFS=true; shift ;;
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
        --capsules)
            [[ -z "${2:-}" ]] && die "Usage: --capsules name1,name2,..."
            IFS=',' read -r -a CAPSULES <<< "$2"
            shift 2 ;;
        *) die "Unknown option: $1. Run --help for usage." ;;
    esac
done

# ── Preflight ─────────────────────────────────────────────────────────

[[ -z "$VERSION" ]] && die "--version is required"
[[ -z "$KEY_PATH" ]] && die "--key is required (release signing must use an explicit key)"
[[ ! -f "$KEY_PATH" ]] && die "Key file not found: $KEY_PATH"
bash "./scripts/check-versioning.sh" "$VERSION"
if ! is_allowed_channel "$CHANNEL"; then
    die "Unsupported release channel '${CHANNEL}'. Allowed channels: ${ALLOWED_CHANNELS[*]}"
fi
mkdir -p "$STATE_DIR"
export ELASTOS_RELEASE_VERSION="$VERSION"

for cmd in jq python3 curl; do
    command -v "$cmd" &>/dev/null || die "Required tool not found: $cmd"
done

sha256 /dev/null &>/dev/null || die "No SHA-256 tool available"

if [[ -z "$IPFS_PROVIDER_BIN" ]]; then
    IPFS_PROVIDER_BIN=$(find_ipfs_provider_binary || true)
fi
[[ -z "$IPFS_PROVIDER_BIN" ]] && die "ipfs-provider binary not found. Build/install it first."
[[ ! -x "$IPFS_PROVIDER_BIN" ]] && die "ipfs-provider binary is not executable: $IPFS_PROVIDER_BIN"
export ELASTOS_IPFS_PROVIDER_BIN="$IPFS_PROVIDER_BIN"

# Resolve target dir from .cargo/config.toml (supports custom target-dir for WSL2 ext4 perf)
CARGO_TARGET_DIR=""
if [[ -f "elastos/.cargo/config.toml" ]]; then
    CARGO_TARGET_DIR=$(grep -E '^\s*target-dir\s*=' "elastos/.cargo/config.toml" 2>/dev/null \
        | head -1 | sed 's/.*=\s*"\(.*\)"/\1/' | sed "s|.*=\s*'\(.*\)'|\1|" | tr -d ' ' || true)
fi
if [[ -n "$CARGO_TARGET_DIR" ]]; then
    ELASTOS="${CARGO_TARGET_DIR}/release/elastos"
else
    ELASTOS="elastos/target/release/elastos"
fi

CANDIDATE_SIGNER_DID="$(inspect_signer_did)"
[[ -n "$CANDIDATE_SIGNER_DID" ]] || die "Failed to determine signer DID from ${KEY_PATH}"
CANONICAL_GATEWAY="$(canonical_publisher_gateway)"
CURRENT_CANONICAL_SIGNER_DID="$(fetch_canonical_signer_did "$CANONICAL_GATEWAY" || true)"
if [[ -n "$CURRENT_CANONICAL_SIGNER_DID" && "$ALLOW_SIGNER_ROTATION" != true && "$CANDIDATE_SIGNER_DID" != "$CURRENT_CANONICAL_SIGNER_DID" ]]; then
    die "Signer DID mismatch for canonical publisher.\n  Candidate: ${CANDIDATE_SIGNER_DID}\n  Canonical: ${CURRENT_CANONICAL_SIGNER_DID}\n  Gateway:   ${CANONICAL_GATEWAY}\nRe-run with --allow-signer-rotation only for an intentional trust-anchor rotation."
fi

echo ""
echo -e "${BOLD}ElastOS Release Publisher (Capsule-Native)${NC}"
echo -e "${DIM}  Version:  ${VERSION}${NC}"
echo -e "${DIM}  Channel:  ${CHANNEL}${NC}"
echo -e "${DIM}  IPFS provider: ${IPFS_PROVIDER_BIN}${NC}"
echo -e "${DIM}  Capsules: ${CAPSULES[*]}${NC}"
echo ""

# ── Step 1: Build runtime (and capsules only when needed) ────────────

if [ "$SKIP_BUILD" = true ]; then
    info "Skipping build (--skip-build)"
    [[ ! -f "$ELASTOS" ]] && die "No elastos binary at ${ELASTOS}. Build first or remove --skip-build."
else
    info "Building runtime..."
    (cd elastos && cargo build --workspace --release 2>&1)

    if [[ "$SKIP_ROOTFS" == true ]]; then
        info "Skipping capsule rebuilds (--skip-rootfs reuses existing capsule artifacts)"
    else
        info "Building publish capsules..."
        for capsule in "${CAPSULES[@]}"; do
            # chat-wasm: build from capsules/chat source, wasm32-wasip1 target
            if [[ "$capsule" == "chat-wasm" ]]; then
                info "  Building chat-wasm (wasm32-wasip1)..."
                (cd capsules/chat && cargo build --bin chat-stdio --target wasm32-wasip1 --no-default-features --release 2>&1)
                mkdir -p capsules/chat-wasm
                cp capsules/chat/target/wasm32-wasip1/release/chat-stdio.wasm capsules/chat-wasm/
                continue
            fi
            capsule_dir="$(resolve_capsule_dir "$capsule" || true)"
            if [[ -z "$capsule_dir" || ! -f "${capsule_dir}/Cargo.toml" ]]; then
                warn "No Cargo.toml for ${capsule}; skipping explicit build step"
                continue
            fi
            info "  Building ${capsule}..."
            (cd "$capsule_dir" && cargo build --release 2>&1)
        done
    fi
fi

[[ ! -f "$ELASTOS" ]] && die "elastos binary not found at ${ELASTOS}"

# ── Step 2: Detect platform ─────────────────────────────────────────

OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)

case "${ARCH}" in
    x86_64)  ARCH="x86_64" ;;
    aarch64) ARCH="aarch64" ;;
    arm64)   ARCH="aarch64" ;;
    *) die "Unsupported architecture: ${ARCH}" ;;
esac

case "${OS}" in
    linux)  PLATFORM="${ARCH}-linux" ;;
    darwin) PLATFORM="${ARCH}-darwin" ;;
    *) die "Unsupported OS: ${OS}" ;;
esac

info "Platform: ${PLATFORM}"

case "${PLATFORM}" in
    x86_64-linux)  SETUP_PLATFORM="linux-amd64" ;;
    aarch64-linux) SETUP_PLATFORM="linux-arm64" ;;
    x86_64-darwin) SETUP_PLATFORM="darwin-amd64" ;;
    aarch64-darwin) SETUP_PLATFORM="darwin-arm64" ;;
    *) die "Unsupported setup platform mapping for ${PLATFORM}" ;;
esac

HOST_DATA_DIR="$(default_elastos_data_dir)"

# Host musl target for rootfs compilation.
case "${ARCH}" in
    x86_64)  HOST_RUST_TARGET="x86_64-unknown-linux-musl" ;;
    aarch64) HOST_RUST_TARGET="aarch64-unknown-linux-musl" ;;
    *) die "No musl target for host arch: ${ARCH}" ;;
esac

# ── Cross-compilation setup ──────────────────────────────────────────

CROSS_PLATFORM=""
CROSS_RUST_TARGET=""
CROSS_SETUP_PLATFORM=""
CROSS_CACHE_DIR=""
CROSS_ENV=()

if [[ -n "$CROSS_ARCH" ]]; then
    case "${CROSS_ARCH}" in
        aarch64|arm64)
            CROSS_PLATFORM="aarch64-linux"
            CROSS_RUST_TARGET="aarch64-unknown-linux-musl"
            CROSS_SETUP_PLATFORM="linux-arm64"
            ;;
        x86_64)
            CROSS_PLATFORM="x86_64-linux"
            CROSS_RUST_TARGET="x86_64-unknown-linux-musl"
            CROSS_SETUP_PLATFORM="linux-amd64"
            ;;
        *) die "Unsupported cross architecture: ${CROSS_ARCH}" ;;
    esac

    CROSS_CACHE_DIR="${HOST_DATA_DIR}/cross/${CROSS_ARCH}"
    info "Cross-compilation target: ${CROSS_PLATFORM}"

    # Ensure Rust target is installed.
    if ! rustup target list --installed 2>/dev/null | grep -q "${CROSS_RUST_TARGET}"; then
        info "Installing Rust target ${CROSS_RUST_TARGET}..."
        rustup target add "${CROSS_RUST_TARGET}" || die "Failed to add Rust target"
    fi

    # Download cross-compilation prerequisites.
    setup_cross_prerequisites() {
        mkdir -p "${CROSS_CACHE_DIR}/bin" "${CROSS_CACHE_DIR}/lib"

        # 1. Busybox-static for target arch.
        if [[ ! -x "${CROSS_CACHE_DIR}/bin/busybox" ]]; then
            info "Downloading busybox-static for ${CROSS_ARCH}..."
            local bb_tmp="${CROSS_CACHE_DIR}/bin/.busybox-download"
            mkdir -p "${CROSS_CACHE_DIR}/bin"
            # Use Debian's static busybox package for reliable cross-arch binaries.
            local bb_url=""
            case "${CROSS_ARCH}" in
                aarch64) bb_url="https://busybox.net/downloads/binaries/1.35.0-x86_64-linux-musl/busybox" ;;
            esac
            # Try to extract from Docker image (most reliable for any arch).
            # Use create+cp instead of run — avoids "exec format error" when
            # the host can't execute cross-arch binaries (no QEMU binfmt).
            if command -v docker >/dev/null 2>&1; then
                local bb_ctr="elastos-bb-extract-$$"
                if docker create --platform "linux/${CROSS_ARCH}" --name "$bb_ctr" \
                        busybox:stable-musl /bin/true >/dev/null 2>&1; then
                    docker cp "${bb_ctr}:/bin/busybox" "${bb_tmp}" 2>/dev/null && \
                        chmod 755 "${bb_tmp}" && \
                        mv "${bb_tmp}" "${CROSS_CACHE_DIR}/bin/busybox" && \
                        info "  Got busybox from Docker (linux/${CROSS_ARCH})"
                    docker rm "$bb_ctr" >/dev/null 2>&1 || true
                    [[ -x "${CROSS_CACHE_DIR}/bin/busybox" ]] && return 0
                fi
                docker rm "$bb_ctr" >/dev/null 2>&1 || true
            fi
            # Fallback: try dpkg cross-arch package.
            if command -v dpkg >/dev/null 2>&1; then
                local deb_tmp
                deb_tmp="$(mktemp -d)"
                if apt-get download "busybox-static:${CROSS_ARCH}" -o Dir::Cache::Archives="${deb_tmp}" 2>/dev/null; then
                    local deb_file
                    deb_file="$(ls "${deb_tmp}"/busybox-static_*.deb 2>/dev/null | head -1)"
                    if [[ -n "$deb_file" ]]; then
                        dpkg-deb --fsys-tarfile "$deb_file" | tar -xf - -C "${deb_tmp}" ./bin/busybox 2>/dev/null
                        if [[ -f "${deb_tmp}/bin/busybox" ]]; then
                            mv "${deb_tmp}/bin/busybox" "${CROSS_CACHE_DIR}/bin/busybox"
                            chmod 755 "${CROSS_CACHE_DIR}/bin/busybox"
                            info "  Got busybox from dpkg (${CROSS_ARCH})"
                            rm -rf "${deb_tmp}"
                            return 0
                        fi
                    fi
                fi
                rm -rf "${deb_tmp}"
            fi
            die "Could not obtain busybox-static for ${CROSS_ARCH}.\n  Options:\n    1. Install Docker and retry\n    2. Manually place a static ${CROSS_ARCH} busybox at:\n       ${CROSS_CACHE_DIR}/bin/busybox"
        fi

        # 2. External tools required by the selected publish capsules for the
        # target arch. Do not prefetch the entire components.json external set
        # here: release publishes should stay focused on runtime-critical
        # assets, not optional demo models or unrelated operator extras.
        while IFS= read -r dep; do
            [[ -n "$dep" ]] || continue
            local dep_meta
            dep_meta="$(DEP_NAME="$dep" DEP_PLATFORM="$CROSS_SETUP_PLATFORM" python3 - <<'PY'
import json, os
name = os.environ["DEP_NAME"]
platform = os.environ["DEP_PLATFORM"]
with open("components.json") as f:
    data = json.load(f)
ext = data.get("external", {}).get(name, {})
plat = ext.get("platforms", {}).get(platform) or ext.get("platforms", {}).get("*") or {}
install_path = (plat.get("install_path") or ext.get("install_path") or "").strip()
url = (plat.get("url") or "").strip()
extract_path = (plat.get("extract_path") or "").strip()
strategy = (plat.get("strategy") or "").strip()
print(f"{install_path}|{url}|{extract_path}|{strategy}")
PY
            )"
            IFS='|' read -r install_rel dep_url extract_path strategy <<< "$dep_meta"
            [[ -z "$install_rel" ]] && continue
            [[ -e "${CROSS_CACHE_DIR}/${install_rel}" ]] && continue
            [[ "$strategy" == "source-build" || "$strategy" == "local-copy" ]] && continue  # Can't auto-download

            if [[ -n "$dep_url" ]]; then
                info "  Downloading ${dep} for ${CROSS_ARCH}..."
                local dl_tmp
                dl_tmp="$(mktemp -d)"
                local dl_file="${dl_tmp}/download"
                curl -fsSL "$dep_url" -o "$dl_file" || { warn "Failed to download ${dep}"; rm -rf "$dl_tmp"; continue; }

                local dest="${CROSS_CACHE_DIR}/${install_rel}"
                mkdir -p "$(dirname "$dest")"

                if [[ "$dep_url" == *.tar.gz || "$dep_url" == *.tgz ]]; then
                    tar -xzf "$dl_file" -C "$dl_tmp"
                    if [[ -n "$extract_path" && -f "${dl_tmp}/${extract_path}" ]]; then
                        cp "${dl_tmp}/${extract_path}" "$dest"
                        chmod 755 "$dest"
                    fi
                elif [[ "$dep_url" == *.gz ]]; then
                    gunzip -c "$dl_file" > "$dest"
                    chmod 755 "$dest"
                else
                    cp "$dl_file" "$dest"
                    chmod 755 "$dest"
                fi
                rm -rf "$dl_tmp"
                info "    -> ${dest}"
            fi
        done < <(SELECTED_CAPSULES="$(IFS=,; echo "${CAPSULES[*]}")" python3 - <<'PY'
import json
import os
from pathlib import Path

selected = [name for name in os.environ.get("SELECTED_CAPSULES", "").split(",") if name]
seen = set()

for name in selected:
    candidates = [
        Path("capsules") / name / "capsule.json",
        Path("elastos") / "capsules" / name / "capsule.json",
    ]
    capsule_json = next((path for path in candidates if path.is_file()), None)
    if capsule_json is None:
        continue
    data = json.loads(capsule_json.read_text(encoding="utf-8"))
    for req in data.get("requires", []):
        if not isinstance(req, dict):
            continue
        if req.get("kind") != "external":
            continue
        ext_name = (req.get("name") or "").strip()
        if ext_name and ext_name not in seen:
            seen.add(ext_name)
            print(ext_name)
PY
        )

        # 3. aarch64 dynamic linker and libc for Go binaries (kubo).
        if [[ ! -f "${CROSS_CACHE_DIR}/lib/ld-linux-aarch64.so.1" ]] && [[ "${CROSS_ARCH}" == "aarch64" ]]; then
            info "  Downloading aarch64 glibc for dynamic linking..."
            if command -v docker >/dev/null 2>&1; then
                # Extract libs from Debian image using create+cp (no execution).
                local lib_ctr="elastos-glibc-extract-$$"
                local lib_tmp
                lib_tmp="$(mktemp -d)"
                if docker create --platform linux/arm64 --name "$lib_ctr" \
                        debian:bookworm-slim /bin/true >/dev/null 2>&1; then
                    for lib_path in \
                        lib/aarch64-linux-gnu/libc.so.6 \
                        lib/aarch64-linux-gnu/libpthread.so.0 \
                        lib/aarch64-linux-gnu/libresolv.so.2 \
                        lib/aarch64-linux-gnu/libdl.so.2 \
                        lib/ld-linux-aarch64.so.1; do
                        mkdir -p "${lib_tmp}/$(dirname "$lib_path")"
                        docker cp "${lib_ctr}:/${lib_path}" "${lib_tmp}/${lib_path}" 2>/dev/null || true
                    done
                    docker rm "$lib_ctr" >/dev/null 2>&1 || true
                fi

                if [[ -d "${lib_tmp}/lib/aarch64-linux-gnu" ]]; then
                    mkdir -p "${CROSS_CACHE_DIR}/lib/aarch64-linux-gnu"
                    cp -a "${lib_tmp}/lib/aarch64-linux-gnu/"* "${CROSS_CACHE_DIR}/lib/aarch64-linux-gnu/"
                fi
                if [[ -f "${lib_tmp}/lib/ld-linux-aarch64.so.1" ]]; then
                    cp "${lib_tmp}/lib/ld-linux-aarch64.so.1" "${CROSS_CACHE_DIR}/lib/"
                fi
                rm -rf "$lib_tmp"
                info "    -> ${CROSS_CACHE_DIR}/lib/"
            else
                warn "Docker not available — skipping glibc download for ${CROSS_ARCH}."
                warn "Dynamically-linked external tools may not work in cross-compiled rootfs."
            fi
        fi
    }

    setup_cross_prerequisites
fi

if [[ "${PLATFORM}" == "x86_64-linux" && -z "${CROSS_ARCH}" ]]; then
    warn "Publishing host platform only (${PLATFORM})."
    warn "For Jetson installation from this release, rerun with: --cross aarch64"
fi

missing_supported_capsules=()
for capsule in "${REQUIRED_SUPPORTED_CAPSULES[@]}"; do
    found=false
    for selected in "${CAPSULES[@]}"; do
        if [[ "$selected" == "$capsule" ]]; then
            found=true
            break
        fi
    done
    if [[ "$found" != true ]]; then
        missing_supported_capsules+=("$capsule")
    fi
done
if [[ ${#missing_supported_capsules[@]} -gt 0 ]]; then
    die "Supported release would be incomplete. Missing required capsules: ${missing_supported_capsules[*]}"
fi

missing_external=()
for capsule in "${CAPSULES[@]}"; do
    capsule_dir="$(resolve_capsule_dir "$capsule" || true)"
    [[ -z "$capsule_dir" ]] && continue
    capsule_json="${capsule_dir}/capsule.json"
    [[ ! -f "$capsule_json" ]] && continue

    while IFS= read -r dep; do
        [[ -z "$dep" ]] && continue
        install_path="$(DEP_NAME="$dep" DEP_PLATFORM="$SETUP_PLATFORM" python3 - <<'PY'
import json, os
name = os.environ["DEP_NAME"]
platform = os.environ["DEP_PLATFORM"]
with open("components.json", "r", encoding="utf-8") as f:
    data = json.load(f)
ext = data.get("external", {}).get(name, {})
plat = ext.get("platforms", {}).get(platform) or ext.get("platforms", {}).get("*") or {}
print((plat.get("install_path") or ext.get("install_path") or "").strip())
PY
)"
        [[ -z "$install_path" ]] && die "External '${dep}' has no install_path in components.json for ${SETUP_PLATFORM}"
        [[ -e "${HOST_DATA_DIR}/${install_path}" ]] && continue
        missing_external+=("$dep")
    done < <(python3 - "$capsule_json" <<'PY'
import json, sys
path = sys.argv[1]
with open(path, "r", encoding="utf-8") as f:
    data = json.load(f)
for req in data.get("requires", []):
    if isinstance(req, dict) and req.get("kind") == "external" and isinstance(req.get("name"), str):
        print(req["name"])
PY
    )
done

if [[ ${#missing_external[@]} -gt 0 && "$SKIP_ROOTFS" != true ]]; then
    unique_external=$(printf '%s\n' "${missing_external[@]}" | sort -u | tr '\n' ',' | sed 's/,$//')
    die "Missing external components required for publish rootfs build (${SETUP_PLATFORM}): ${unique_external}\n  Run: elastos setup --with ${unique_external}"
fi

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

ARTIFACTS_DIR="${TMPDIR}/artifacts"
mkdir -p "$ARTIFACTS_DIR"
LOCAL_ARTIFACT_CACHE_DIR="artifacts"

# ── Step 3: Build rootfs + package capsule artifacts ─────────────────

if [ "$SKIP_ROOTFS" = true ]; then
    info "Skipping rootfs build (--skip-rootfs), using existing artifacts"
    MISSING_ARTIFACTS=()
    for capsule in "${CAPSULES[@]}"; do
        artifact="artifacts/${capsule}.capsule.tar.gz"
        if [[ -f "$artifact" ]]; then
            cp "$artifact" "${ARTIFACTS_DIR}/"
        else
            MISSING_ARTIFACTS+=("$capsule")
        fi
    done
    if [[ ${#MISSING_ARTIFACTS[@]} -gt 0 ]]; then
        die "Missing artifacts: ${MISSING_ARTIFACTS[*]}\n  Build with: ./scripts/build/build-rootfs.sh <name> --output artifacts/"
    fi

    # Cross artifacts from cached cross build.
    if [[ -n "$CROSS_ARCH" ]]; then
        CROSS_ARTIFACTS_DIR="${TMPDIR}/artifacts-${CROSS_ARCH}"
        mkdir -p "$CROSS_ARTIFACTS_DIR"
        MISSING_CROSS=()
        for capsule in "${CAPSULES[@]}"; do
            artifact="artifacts-${CROSS_ARCH}/${capsule}.capsule.tar.gz"
            if [[ -f "$artifact" ]]; then
                cp "$artifact" "${CROSS_ARTIFACTS_DIR}/"
            else
                MISSING_CROSS+=("$capsule")
            fi
        done
        if [[ ${#MISSING_CROSS[@]} -gt 0 ]]; then
            die "Missing cross artifacts (${CROSS_ARCH}): ${MISSING_CROSS[*]}\n  Build with: ./scripts/build/build-rootfs.sh <name> --target ${CROSS_RUST_TARGET} --output artifacts-${CROSS_ARCH}/"
        fi
    fi
else
    info "Building capsule artifacts (compile + sequential packaging)..."

    # Phase 1: Compile all capsule binaries sequentially (Cargo handles internal
    # parallelism; avoids lock contention from concurrent cargo invocations).
    info "  Compiling capsule binaries (${HOST_RUST_TARGET})..."
    for capsule in "${CAPSULES[@]}"; do
        capsule_dir="$(resolve_capsule_dir "$capsule" || true)"
        if [[ -n "$capsule_dir" && -f "${capsule_dir}/Cargo.toml" ]]; then
            info "    ${capsule} (host)..."
            (cd "$capsule_dir" && cargo build --release --target "$HOST_RUST_TARGET" 2>&1) \
                || die "Compile failed for ${capsule}"
        fi
    done

    # Pre-build vsock-proxy once (shared by all rootfs images).
    VSOCK_PROXY_DIR="elastos/tools/vsock-proxy"
    if [[ -f "${VSOCK_PROXY_DIR}/Cargo.toml" ]]; then
        info "    vsock-proxy (host)..."
        (cd "$VSOCK_PROXY_DIR" && cargo build --release --target "$HOST_RUST_TARGET" 2>&1) \
            || die "Compile failed for vsock-proxy"
    fi

    # Phase 2: Package capsule artifacts sequentially.
    # WASM capsules: tar capsule.json + .wasm directly (no rootfs).
    # MicroVM capsules: build rootfs via build-rootfs.sh one at a time.
    # This is intentionally boring: parallel rootfs packaging has proven flaky
    # under deadline pressure, while the sequential path is reproducible.
    info "  Packaging capsule artifacts..."
    ROOTFS_LOGS_DIR="${TMPDIR}/rootfs-logs"
    mkdir -p "$ROOTFS_LOGS_DIR"
    for capsule in "${CAPSULES[@]}"; do
        capsule_dir="$(resolve_capsule_dir "$capsule" || true)"
        # WASM capsule: package directly (no rootfs needed)
        if [[ -n "$capsule_dir" && -f "${capsule_dir}/capsule.json" ]]; then
            capsule_type=$(python3 -c "import json; print(json.load(open('${capsule_dir}/capsule.json')).get('type',''))" 2>/dev/null || true)
            if [[ "$capsule_type" == "wasm" ]]; then
                info "    ${capsule} (wasm package)..."
                tar -czf "${ARTIFACTS_DIR}/${capsule}.capsule.tar.gz" -C "$capsule_dir" . \
                    >"${ROOTFS_LOGS_DIR}/${capsule}.log" 2>&1
                continue
            fi
        fi
        info "    ${capsule} (rootfs)..."
        ./scripts/build/build-rootfs.sh "$capsule" --skip-compile --target "$HOST_RUST_TARGET" --output "$ARTIFACTS_DIR" \
            >"${ROOTFS_LOGS_DIR}/${capsule}.log" 2>&1 \
            || die "Rootfs build failed for ${capsule}. Check log: ${ROOTFS_LOGS_DIR}/${capsule}.log"
    done
    info "  All rootfs packages built."

    # Persist a local cache so subsequent runs can use --skip-rootfs quickly.
    mkdir -p "$LOCAL_ARTIFACT_CACHE_DIR"
    cp -f "${ARTIFACTS_DIR}"/*.capsule.tar.gz "$LOCAL_ARTIFACT_CACHE_DIR"/
    info "Cached artifacts to ./${LOCAL_ARTIFACT_CACHE_DIR}/"

    # Cross-compilation: compile then package in parallel for the cross target.
    if [[ -n "$CROSS_ARCH" ]]; then
        CROSS_ARTIFACTS_DIR="${TMPDIR}/artifacts-${CROSS_ARCH}"
        mkdir -p "$CROSS_ARTIFACTS_DIR"

        # When using `cross`, Docker containers don't have clang/lld from the
        # host .cargo/config.toml. Override the host linker to plain `cc`.
        CROSS_ENV=()
        if command -v cross >/dev/null 2>&1; then
            CROSS_ENV=(env CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=cc
                           CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS=)
        fi

        info "Compiling capsule binaries (${CROSS_PLATFORM})..."
        for capsule in "${CAPSULES[@]}"; do
            capsule_dir="$(resolve_capsule_dir "$capsule" || true)"
            if [[ -n "$capsule_dir" && -f "${capsule_dir}/Cargo.toml" ]]; then
                info "    ${capsule} (${CROSS_RUST_TARGET})..."
                if command -v cross >/dev/null 2>&1; then
                    (cd "$capsule_dir" && "${CROSS_ENV[@]}" cross build --release --target "$CROSS_RUST_TARGET" 2>&1) \
                        || die "Cross-compile failed for ${capsule}"
                else
                    (cd "$capsule_dir" && cargo build --release --target "$CROSS_RUST_TARGET" 2>&1) \
                        || die "Cross-compile failed for ${capsule}"
                fi
            fi
        done

        # Pre-build vsock-proxy for cross target.
        if [[ -f "${VSOCK_PROXY_DIR}/Cargo.toml" ]]; then
            info "    vsock-proxy (${CROSS_RUST_TARGET})..."
            if command -v cross >/dev/null 2>&1; then
                (cd "$VSOCK_PROXY_DIR" && "${CROSS_ENV[@]}" cross build --release --target "$CROSS_RUST_TARGET" 2>&1) \
                    || die "Cross-compile failed for vsock-proxy"
            else
                (cd "$VSOCK_PROXY_DIR" && cargo build --release --target "$CROSS_RUST_TARGET" 2>&1) \
                    || die "Cross-compile failed for vsock-proxy"
            fi
        fi

        info "  Packaging cross capsule artifacts sequentially..."
        CROSS_LOGS_DIR="${TMPDIR}/rootfs-logs-${CROSS_ARCH}"
        mkdir -p "$CROSS_LOGS_DIR"
        for capsule in "${CAPSULES[@]}"; do
            capsule_dir="$(resolve_capsule_dir "$capsule" || true)"
            # WASM capsules are platform-independent: package capsule.json + .wasm directly.
            if [[ -n "$capsule_dir" && -f "${capsule_dir}/capsule.json" ]]; then
                capsule_type=$(python3 -c "import json; print(json.load(open('${capsule_dir}/capsule.json')).get('type',''))" 2>/dev/null || true)
                if [[ "$capsule_type" == "wasm" ]]; then
                    info "    ${capsule} (wasm package, ${CROSS_PLATFORM})..."
                    tar -czf "${CROSS_ARTIFACTS_DIR}/${capsule}.capsule.tar.gz" -C "$capsule_dir" . \
                        >"${CROSS_LOGS_DIR}/${capsule}.log" 2>&1
                    continue
                fi
            fi
            info "    ${capsule} (${CROSS_RUST_TARGET})..."
            ./scripts/build/build-rootfs.sh "$capsule" --skip-compile --target "$CROSS_RUST_TARGET" --output "$CROSS_ARTIFACTS_DIR" \
                >"${CROSS_LOGS_DIR}/${capsule}.log" 2>&1 \
                || die "Cross rootfs build failed for ${capsule}. Check log: ${CROSS_LOGS_DIR}/${capsule}.log"
        done

        # Cache cross artifacts for --skip-rootfs on subsequent runs.
        mkdir -p "artifacts-${CROSS_ARCH}"
        cp -f "${CROSS_ARTIFACTS_DIR}"/*.capsule.tar.gz "artifacts-${CROSS_ARCH}/"
        info "Cached cross artifacts to ./artifacts-${CROSS_ARCH}/"
    fi
fi

# ── Step 4: Upload capsule artifacts to IPFS ─────────────────────────

# Helper: publish capsule artifacts for one platform in parallel, return capsule entries JSON.
publish_platform_capsules() {
    local art_dir="$1"
    local plat="$2"
    local upload_tmp="${TMPDIR}/uploads-${plat}"
    mkdir -p "$upload_tmp"

    # Launch all IPFS uploads in parallel, each writing result to a temp file.
    local pids=()
    for capsule in "${CAPSULES[@]}"; do
        artifact="${art_dir}/${capsule}.capsule.tar.gz"
        if [[ ! -f "$artifact" ]]; then
            warn "No artifact for ${capsule} (${plat}), skipping" >&2
            continue
        fi

        (
            cid=$(ipfs_add "$artifact")
            checksum=$(sha256 "$artifact")
            size=$(file_size "$artifact")
            echo -e "  ${GREEN}▶${NC}   ${capsule} [${plat}]: ${cid} (${size} bytes)" >&2
            # Write result as a single JSON line for merging.
            jq -nc --arg name "$capsule" --arg cid "$cid" --arg sha256 "$checksum" \
                --argjson size "$size" --arg platform "$plat" \
                '{($name): {cid: $cid, sha256: $sha256, size: $size, platforms: [$platform]}}' \
                > "${upload_tmp}/${capsule}.json"
        ) &
        pids+=("$!:${capsule}")
    done

    # Wait for all uploads; collect failures.
    local failed=()
    for entry in "${pids[@]}"; do
        local pid="${entry%%:*}"
        local name="${entry#*:}"
        if ! wait "$pid"; then
            failed+=("$name")
        fi
    done
    if [[ ${#failed[@]} -gt 0 ]]; then
        die "IPFS upload failed for: ${failed[*]} (${plat})"
    fi

    # Merge all per-capsule JSON files into one object.
    local entries="{}"
    for f in "${upload_tmp}"/*.json; do
        [[ -f "$f" ]] || continue
        entries=$(echo "$entries" "$(cat "$f")" | jq -s '.[0] * .[1]')
    done
    echo "$entries"
}

info "Publishing capsule artifacts to IPFS..."

# Host platform capsules.
CAPSULE_ENTRIES=$(publish_platform_capsules "$ARTIFACTS_DIR" "$PLATFORM")
SHELL_CID=$(echo "$CAPSULE_ENTRIES" | jq -r '.shell.cid // empty')
SHELL_SHA256=$(echo "$CAPSULE_ENTRIES" | jq -r '.shell.sha256 // empty')

# Cross platform capsules (if applicable).
CROSS_CAPSULE_ENTRIES="{}"
if [[ -n "$CROSS_ARCH" && -d "${CROSS_ARTIFACTS_DIR:-/nonexistent}" ]]; then
    info "Publishing cross-compiled capsule artifacts (${CROSS_PLATFORM})..."
    CROSS_CAPSULE_ENTRIES=$(publish_platform_capsules "$CROSS_ARTIFACTS_DIR" "$CROSS_PLATFORM")
fi

info "Publishing direct share/open support assets..."
UNIVERSAL_DIRECT_ASSETS=$(build_platform_independent_direct_assets "$PLATFORM")
HOST_PLATFORM_DIRECT_ASSETS=$(build_supported_direct_assets "$PLATFORM" "$SETUP_PLATFORM" "$HOST_RUST_TARGET" false)
HOST_DIRECT_ASSETS=$(merge_direct_assets "$HOST_PLATFORM_DIRECT_ASSETS" "$UNIVERSAL_DIRECT_ASSETS")
CROSS_DIRECT_ASSETS="{}"
if [[ -n "$CROSS_ARCH" ]]; then
    CROSS_PLATFORM_DIRECT_ASSETS=$(build_supported_direct_assets "$CROSS_PLATFORM" "$CROSS_SETUP_PLATFORM" "$CROSS_RUST_TARGET" true)
    CROSS_DIRECT_ASSETS=$(merge_direct_assets "$CROSS_PLATFORM_DIRECT_ASSETS" "$UNIVERSAL_DIRECT_ASSETS")
fi

# ── Step 5: Generate components.json with real CIDs ──────────────────

# Generate per-platform components.json (each platform has its own capsule CIDs).
generate_components_json() {
    local capsule_entries="$1"
    local direct_assets="$2"
    local external profiles
    external=$(jq '.external' components.json)
    profiles=$(jq '.profiles' components.json)
    jq -n \
        --arg schema "elastos.components/v1" \
        --argjson capsules "$capsule_entries" \
        --argjson external "$external" \
        --argjson profiles "$profiles" \
        --argjson direct "$direct_assets" \
        '{schema: $schema, capsules: $capsules, external: ($external * $direct.external), profiles: $profiles}'
}

info "Generating components.json..."
COMPONENTS_JSON=$(generate_components_json "$CAPSULE_ENTRIES" "$HOST_DIRECT_ASSETS")

echo "$COMPONENTS_JSON" > "${TMPDIR}/components.json"

COMPONENTS_SHA256=$(sha256 "${TMPDIR}/components.json")
COMPONENTS_SIZE=$(file_size "${TMPDIR}/components.json")

info "Publishing components.json to IPFS..."
COMPONENTS_CID=$(ipfs_add "${TMPDIR}/components.json")
info "Components CID: ${COMPONENTS_CID}"

# Cross platform components.json (if applicable).
CROSS_COMPONENTS_CID=""
CROSS_COMPONENTS_SHA256=""
CROSS_COMPONENTS_SIZE=""
CROSS_BINARY_CID=""
CROSS_BINARY_SHA256=""
CROSS_BINARY_SIZE=""

if [[ -n "$CROSS_ARCH" && "$CROSS_CAPSULE_ENTRIES" != "{}" ]]; then
    info "Generating components.json for ${CROSS_PLATFORM}..."
    CROSS_COMPONENTS_JSON=$(generate_components_json "$CROSS_CAPSULE_ENTRIES" "$CROSS_DIRECT_ASSETS")
    echo "$CROSS_COMPONENTS_JSON" > "${TMPDIR}/components-${CROSS_ARCH}.json"
    CROSS_COMPONENTS_SHA256=$(sha256 "${TMPDIR}/components-${CROSS_ARCH}.json")
    CROSS_COMPONENTS_SIZE=$(file_size "${TMPDIR}/components-${CROSS_ARCH}.json")

    info "Publishing components.json (${CROSS_PLATFORM}) to IPFS..."
    CROSS_COMPONENTS_CID=$(ipfs_add "${TMPDIR}/components-${CROSS_ARCH}.json")
    info "Components CID (${CROSS_PLATFORM}): ${CROSS_COMPONENTS_CID}"
fi

# ── Step 6: Upload elastos binary to IPFS ────────────────────────────

info "Publishing elastos binary to IPFS..."
# ipfs-provider add_path policy allows /tmp and ~/.local/share/elastos.
# Stage runtime binary into TMPDIR so publish works from any checkout path.
STAGED_ELASTOS="${TMPDIR}/elastos"
if [[ -n "$CARGO_TARGET_DIR" ]]; then
    HOST_MUSL_ELASTOS="${CARGO_TARGET_DIR}/${HOST_RUST_TARGET}/release/elastos"
else
    HOST_MUSL_ELASTOS="elastos/target/${HOST_RUST_TARGET}/release/elastos"
fi
if [[ "$PLATFORM" == *-linux ]]; then
    ensure_rust_target_installed "$HOST_RUST_TARGET"
    if [[ "$SKIP_BUILD" != true ]]; then
        if [[ ! -f "$HOST_MUSL_ELASTOS" ]] || ([[ -f "$ELASTOS" ]] && [[ "$HOST_MUSL_ELASTOS" -ot "$ELASTOS" ]]); then
            info "Building portable musl runtime binary for host (${PLATFORM})..."
            (cd elastos && cargo build --release --target "${HOST_RUST_TARGET}" -p elastos-server) \
                || die "Portable musl build failed for ${PLATFORM}"
        fi
    fi
    [[ -f "$HOST_MUSL_ELASTOS" ]] || die "Missing portable musl runtime binary for ${PLATFORM}: ${HOST_MUSL_ELASTOS}\nPublic Linux publish is fail-closed without it."
    assert_runtime_binary_embeds_release_version "$HOST_MUSL_ELASTOS" "host ${PLATFORM}" "$VERSION"
    ./scripts/audit-linux-runtime-portability.sh \
        --platform "$PLATFORM" \
        --binary "$HOST_MUSL_ELASTOS" \
        --label "host ${PLATFORM} runtime"
    info "Using portable musl runtime binary for host: ${HOST_MUSL_ELASTOS}"
    cp "$HOST_MUSL_ELASTOS" "$STAGED_ELASTOS"
else
    info "Using default host runtime binary: ${ELASTOS}"
    cp "$ELASTOS" "$STAGED_ELASTOS"
fi

BINARY_CID=$(ipfs_add "$STAGED_ELASTOS")
BINARY_SHA256=$(sha256 "$STAGED_ELASTOS")
BINARY_SIZE=$(file_size "$STAGED_ELASTOS")
info "Binary CID: ${BINARY_CID} (${BINARY_SIZE} bytes)"

# Cross-compile and publish runtime binary for cross platform.
if [[ -n "$CROSS_ARCH" ]]; then
    if [[ -n "$CARGO_TARGET_DIR" ]]; then
        CROSS_ELASTOS="${CARGO_TARGET_DIR}/${CROSS_RUST_TARGET}/release/elastos"
    else
        CROSS_ELASTOS="elastos/target/${CROSS_RUST_TARGET}/release/elastos"
    fi

    # Ensure CROSS_ENV is set (may have been skipped if --skip-rootfs was used).
    if [[ -z "${CROSS_ENV+x}" ]] && command -v cross >/dev/null 2>&1; then
        CROSS_ENV=(env CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=cc
                       CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS=)
    fi
    : "${CROSS_ENV:=()}"

    if [[ "$SKIP_BUILD" == true && -f "$CROSS_ELASTOS" ]]; then
        info "Using existing cross-compiled binary (--skip-build): ${CROSS_ELASTOS}"
    elif [[ "$SKIP_BUILD" == true ]]; then
        warn "Cross binary not found at ${CROSS_ELASTOS} (--skip-build). Skipping cross platform binary."
        CROSS_ELASTOS=""
    else
        info "Cross-compiling runtime binary for ${CROSS_PLATFORM}..."
        if command -v cross >/dev/null 2>&1; then
            (cd elastos && "${CROSS_ENV[@]}" cross build --release --target "${CROSS_RUST_TARGET}" -p elastos-server 2>&1) \
                || die "Cross-compilation of runtime binary failed"
        else
            (cd elastos && cargo build --release --target "${CROSS_RUST_TARGET}" -p elastos-server 2>&1) \
                || die "Cross-compilation of runtime binary failed.\n  Install 'cross': cargo install cross"
        fi
    fi

    if [[ -n "$CROSS_ELASTOS" && -f "$CROSS_ELASTOS" ]]; then
        assert_runtime_binary_embeds_release_version "$CROSS_ELASTOS" "cross ${CROSS_PLATFORM}" "$VERSION"
        ./scripts/audit-linux-runtime-portability.sh \
            --platform "$CROSS_PLATFORM" \
            --binary "$CROSS_ELASTOS" \
            --label "cross ${CROSS_PLATFORM} runtime"
        STAGED_CROSS="${TMPDIR}/elastos-${CROSS_ARCH}"
        cp "$CROSS_ELASTOS" "$STAGED_CROSS"
        CROSS_BINARY_CID=$(ipfs_add "$STAGED_CROSS")
        CROSS_BINARY_SHA256=$(sha256 "$STAGED_CROSS")
        CROSS_BINARY_SIZE=$(file_size "$STAGED_CROSS")
        info "Binary CID (${CROSS_PLATFORM}): ${CROSS_BINARY_CID} (${CROSS_BINARY_SIZE} bytes)"
    elif [[ -n "$CROSS_ELASTOS" ]]; then
        warn "Cross-compiled binary not found at ${CROSS_ELASTOS}"
    fi
fi

# ── Step 7: Create + sign release.json ───────────────────────────────

PREV_RELEASE_CID="null"
if [[ -f "${STATE_DIR}/last-release-cid" ]]; then
    PREV_RELEASE_CID="\"$(cat "${STATE_DIR}/last-release-cid")\""
fi

# Build the platforms object. Start with the host platform.
PLATFORMS_JSON=$(jq -n \
    --arg platform "$PLATFORM" \
    --arg bin_cid "$BINARY_CID" \
    --arg bin_sha256 "$BINARY_SHA256" \
    --argjson bin_size "$BINARY_SIZE" \
    --arg comp_cid "$COMPONENTS_CID" \
    --arg comp_sha256 "$COMPONENTS_SHA256" \
    --argjson comp_size "$COMPONENTS_SIZE" \
    '{
        ($platform): {
            binary: {cid: $bin_cid, sha256: $bin_sha256, size: $bin_size},
            components: {cid: $comp_cid, sha256: $comp_sha256, size: $comp_size}
        }
    }')

# Add cross platform entry if available.
if [[ -n "$CROSS_BINARY_CID" && -n "$CROSS_COMPONENTS_CID" ]]; then
    PLATFORMS_JSON=$(echo "$PLATFORMS_JSON" | jq \
        --arg platform "$CROSS_PLATFORM" \
        --arg bin_cid "$CROSS_BINARY_CID" \
        --arg bin_sha256 "$CROSS_BINARY_SHA256" \
        --argjson bin_size "$CROSS_BINARY_SIZE" \
        --arg comp_cid "$CROSS_COMPONENTS_CID" \
        --arg comp_sha256 "$CROSS_COMPONENTS_SHA256" \
        --argjson comp_size "$CROSS_COMPONENTS_SIZE" \
        '. + {
            ($platform): {
                binary: {cid: $bin_cid, sha256: $bin_sha256, size: $bin_size},
                components: {cid: $comp_cid, sha256: $comp_sha256, size: $comp_size}
            }
        }')
    info "Release includes: ${PLATFORM}, ${CROSS_PLATFORM}"
fi

RELEASE_PAYLOAD=$(jq -ncS \
    --arg schema "elastos.release/v1" \
    --arg channel "$CHANNEL" \
    --arg version "$VERSION" \
    --argjson released_at "$(now_unix)" \
    --argjson prev_release_cid "$PREV_RELEASE_CID" \
    --arg shell_cid "${SHELL_CID}" \
    --arg shell_sha256 "${SHELL_SHA256}" \
    --argjson platforms "$PLATFORMS_JSON" \
    '{
        schema: $schema,
        channel: $channel,
        version: $version,
        released_at: $released_at,
        prev_release_cid: $prev_release_cid,
        shell_cid: $shell_cid,
        shell_sha256: $shell_sha256,
        platforms: $platforms
    }')

info "Signing release payload..."
SIGN_OUTPUT=$(echo -n "$RELEASE_PAYLOAD" | "$ELASTOS" sign-payload --domain elastos.release.v1 --key "$KEY_PATH")
RELEASE_SIG=$(echo "$SIGN_OUTPUT" | jq -r '.signature')
SIGNER_DID=$(echo "$SIGN_OUTPUT" | jq -r '.signer_did')

RELEASE_JSON=$(jq -n \
    --argjson payload "$RELEASE_PAYLOAD" \
    --arg signature "$RELEASE_SIG" \
    --arg signer_did "$SIGNER_DID" \
    '{ payload: $payload, signature: $signature, signer_did: $signer_did }')

echo "$RELEASE_JSON" > "${TMPDIR}/release.json"

info "Publishing release.json to IPFS..."
RELEASE_CID=$(ipfs_add "${TMPDIR}/release.json")
info "Release CID: ${RELEASE_CID}"

RELEASE_LINK_ARGS=()
while IFS= read -r link; do
    [[ -n "$link" ]] && RELEASE_LINK_ARGS+=(--link "$link")
done < <(jq -rn \
    --argjson platforms "$PLATFORMS_JSON" \
    --argjson host_capsules "$CAPSULE_ENTRIES" \
    --argjson cross_capsules "$CROSS_CAPSULE_ENTRIES" \
    '
    (
    $platforms
        | to_entries[]
        | "binary.\(.key)=\(.value.binary.cid)",
          "components.\(.key)=\(.value.components.cid)"
    ),
    (
    $host_capsules
        | to_entries[]
        | select(.value.cid)
        | "capsule.\(.key).\(.value.platforms[0] // "host")=\(.value.cid)"
    ),
    (
    $cross_capsules
        | to_entries[]
        | select(.value.cid)
        | "capsule.\(.key).\(.value.platforms[0] // "cross")=\(.value.cid)"
    )
    ')
if [[ "$PREV_RELEASE_CID" != "null" && -f "${STATE_DIR}/last-release-cid" ]]; then
    RELEASE_LINK_ARGS+=(--link "previous.$CHANNEL=$(cat "${STATE_DIR}/last-release-cid")")
fi

info "Publishing release object manifest through content availability..."
RELEASE_OBJECT_CID=$(content_publish_object "${TMPDIR}/release.json" "release" "release.json" \
    --object-did "elastos://release/${CHANNEL}/${VERSION}" \
    --publisher-did "$SIGNER_DID" \
    "${RELEASE_LINK_ARGS[@]}")
info "Release object CID: ${RELEASE_OBJECT_CID}"

# ── Step 8: Create + sign release-head.json ──────────────────────────

PREV_HEAD_CID="null"
if [[ -f "${STATE_DIR}/last-release-head-cid" ]]; then
    PREV_HEAD_CID="\"$(cat "${STATE_DIR}/last-release-head-cid")\""
fi

HEAD_PAYLOAD=$(jq -ncS \
    --arg schema "elastos.release.head/v1" \
    --arg channel "$CHANNEL" \
    --arg latest_release_cid "$RELEASE_CID" \
    --arg release_object_cid "$RELEASE_OBJECT_CID" \
    --arg version "$VERSION" \
    --argjson updated_at "$(now_unix)" \
    --arg signer_did "$SIGNER_DID" \
    --argjson prev_head_cid "$PREV_HEAD_CID" \
    '{
        schema: $schema,
        channel: $channel,
        latest_release_cid: $latest_release_cid,
        release_object_cid: $release_object_cid,
        version: $version,
        updated_at: $updated_at,
        signer_did: $signer_did,
        prev_head_cid: $prev_head_cid
    }')

info "Signing release head..."
HEAD_SIGN_OUTPUT=$(echo -n "$HEAD_PAYLOAD" | "$ELASTOS" sign-payload --domain elastos.release.head.v1 --key "$KEY_PATH")
HEAD_SIG=$(echo "$HEAD_SIGN_OUTPUT" | jq -r '.signature')
HEAD_SIGNER_DID=$(echo "$HEAD_SIGN_OUTPUT" | jq -r '.signer_did')

HEAD_JSON=$(jq -n \
    --argjson payload "$HEAD_PAYLOAD" \
    --arg signature "$HEAD_SIG" \
    --arg signer_did "$HEAD_SIGNER_DID" \
    '{ payload: $payload, signature: $signature, signer_did: $signer_did }')

echo "$HEAD_JSON" > "${TMPDIR}/release-head.json"

info "Publishing release-head.json to IPFS..."
HEAD_CID=$(ipfs_add "${TMPDIR}/release-head.json")
info "Head CID: ${HEAD_CID}"

HEAD_LINK_ARGS=(
    --link "release=$RELEASE_CID"
    --link "release.object=$RELEASE_OBJECT_CID"
)
if [[ "$PREV_HEAD_CID" != "null" && -f "${STATE_DIR}/last-release-head-cid" ]]; then
    HEAD_LINK_ARGS+=(--link "previous.$CHANNEL=$(cat "${STATE_DIR}/last-release-head-cid")")
fi

info "Publishing release-head object manifest through content availability..."
HEAD_OBJECT_CID=$(content_publish_object "${TMPDIR}/release-head.json" "release" "release-head.json" \
    --object-did "elastos://release-head/${CHANNEL}" \
    --publisher-did "$HEAD_SIGNER_DID" \
    "${HEAD_LINK_ARGS[@]}")
info "Head object CID: ${HEAD_OBJECT_CID}"

# ── Step 8b: Publish IPNS name ─────────────────────────────────────────

IPNS_NAME=""
# Resolve Kubo API port from coord file (if Kubo is running via ipfs-provider)
KUBO_COORD_FILE="${HOST_DATA_DIR}/ipfs-coords.json"
KUBO_PORT=""
if [[ -f "$KUBO_COORD_FILE" ]]; then
    KUBO_PORT=$(jq -r '.api_port // empty' "$KUBO_COORD_FILE" 2>/dev/null || true)
fi

if [[ -n "$KUBO_PORT" ]]; then
    info "Publishing HEAD to IPNS (lifetime=8760h)..."
    IPNS_RESPONSE=$(curl -s -X POST "http://127.0.0.1:${KUBO_PORT}/api/v0/name/publish?arg=/ipfs/${HEAD_CID}&key=self&lifetime=8760h" 2>/dev/null || true)
    IPNS_NAME=$(echo "$IPNS_RESPONSE" | jq -r '.Name // empty' 2>/dev/null || true)
    if [[ -n "$IPNS_NAME" ]]; then
        info "IPNS name: ${IPNS_NAME}"
        echo -n "$IPNS_NAME" > "${STATE_DIR}/last-ipns-name"
    else
        warn "IPNS publish failed (Kubo may not be ready). Skipping."
        if [[ -n "$IPNS_RESPONSE" ]]; then
            warn "Response: ${IPNS_RESPONSE}"
        fi
    fi
elif command -v ipfs &>/dev/null && ipfs swarm peers &>/dev/null 2>&1; then
    info "Publishing HEAD to IPNS via system IPFS (lifetime=8760h)..."
    IPNS_NAME=$(ipfs name publish --lifetime=8760h "/ipfs/${HEAD_CID}" 2>/dev/null | grep -oP 'k[a-zA-Z0-9]+' | head -1 || true)
    if [[ -n "$IPNS_NAME" ]]; then
        info "IPNS name: ${IPNS_NAME}"
        echo -n "$IPNS_NAME" > "${STATE_DIR}/last-ipns-name"
    else
        warn "IPNS publish failed via system IPFS. Skipping."
    fi
else
    warn "No Kubo API or system IPFS available — skipping IPNS publish."
    warn "IPNS name can be published later with: ipfs name publish /ipfs/${HEAD_CID}"
fi

# ── Step 9: Publish installer bundle ──────────────────────────────────

info "Publishing installer bundle (install.sh) to IPFS..."
# Bake trust anchors into install.sh so users can just: curl ... | bash
STAMPED_INSTALL="${TMPDIR}/install.sh"
STAMPED_SOURCE_CONNECT_TICKET="${ELASTOS_SOURCE_CONNECT_TICKET:-}"
STAMPED_PUBLISHER_GATEWAY="${ELASTOS_PUBLISHER_GATEWAY:-}"
CANONICAL_PUBLISHER_GATEWAY="${ELASTOS_CANONICAL_PUBLISHER_GATEWAY:-https://elastos.elacitylabs.com}"
ALLOW_NO_BOOTSTRAP="${ELASTOS_ALLOW_NO_BOOTSTRAP:-}"
BOOTSTRAP_JSON="$(discover_source_bootstrap_json)"
BOOTSTRAP_VERSION="$(printf '%s' "$BOOTSTRAP_JSON" | jq -r '.version // empty' 2>/dev/null || true)"
if [[ -n "${VERSION:-}" ]]; then
    if [[ -z "$BOOTSTRAP_VERSION" && "$ALLOW_NO_BOOTSTRAP" != "1" ]]; then
        die "publish-release requires a trusted-source runtime with a visible health version. Refresh the canonical source runtime first."
    fi
    if [[ -n "$BOOTSTRAP_VERSION" && "$BOOTSTRAP_VERSION" != "$VERSION" && "$BOOTSTRAP_VERSION" != "${VERSION}-dev" ]]; then
        die "trusted-source runtime is stale (running ${BOOTSTRAP_VERSION}, expected ${VERSION} or ${VERSION}-dev). Refresh the canonical source runtime first."
    fi
fi
if [[ -z "$STAMPED_SOURCE_CONNECT_TICKET" ]]; then
    STAMPED_SOURCE_CONNECT_TICKET="$(printf '%s' "$BOOTSTRAP_JSON" | jq -r '.ticket // empty' 2>/dev/null || true)"
fi
# Get publisher's stable node ID for durable P2P connections
STAMPED_PUBLISHER_NODE_ID="${ELASTOS_PUBLISHER_NODE_ID:-}"
if [[ -z "$STAMPED_PUBLISHER_NODE_ID" ]]; then
    STAMPED_PUBLISHER_NODE_ID="$(printf '%s' "$BOOTSTRAP_JSON" | jq -r '.node_id // empty' 2>/dev/null || true)"
fi
if [[ -z "$STAMPED_PUBLISHER_NODE_ID" ]]; then
    STAMPED_PUBLISHER_NODE_ID=$("$ELASTOS" keys node-id 2>/dev/null || true)
fi
if [[ -n "$STAMPED_PUBLISHER_NODE_ID" ]]; then
    info "Publisher node ID: ${STAMPED_PUBLISHER_NODE_ID}"
fi
if [[ -z "$STAMPED_SOURCE_CONNECT_TICKET" && "$ALLOW_NO_BOOTSTRAP" != "1" ]]; then
    die "publish-release requires a live trusted-source Carrier ticket. Start or refresh a local ElastOS runtime first, or set ELASTOS_ALLOW_NO_BOOTSTRAP=1 only for local-only testing."
fi
if [[ -n "$STAMPED_SOURCE_CONNECT_TICKET" ]]; then
    info "Stamping trusted-source Carrier bootstrap ticket"
fi
STAMPED_IPNS_NAME="${IPNS_NAME:-}"
if [[ -z "$STAMPED_PUBLISHER_GATEWAY" ]]; then
    # Use the canonical public domain by default. Do not inherit stale local
    # install state or old nginx stamps when the public name has changed.
    if [[ -n "$CANONICAL_PUBLISHER_GATEWAY" ]]; then
        STAMPED_PUBLISHER_GATEWAY="${CANONICAL_PUBLISHER_GATEWAY%/}"
        info "Stamping canonical publisher gateway: ${STAMPED_PUBLISHER_GATEWAY}"
    fi
fi
if [[ -z "$STAMPED_PUBLISHER_GATEWAY" ]]; then
    die "publish-release requires a canonical publisher gateway. Set ELASTOS_PUBLISHER_GATEWAY=https://your-domain or ELASTOS_CANONICAL_PUBLISHER_GATEWAY=https://your-domain."
fi
sed -e "s|__HEAD_CID__|${HEAD_CID}|g" \
    -e "s|__MAINTAINER_DID__|${SIGNER_DID}|g" \
    -e "s|__SOURCE_CONNECT_TICKET__|${STAMPED_SOURCE_CONNECT_TICKET}|g" \
    -e "s|__PUBLISHER_GATEWAY__|${STAMPED_PUBLISHER_GATEWAY}|g" \
    -e "s|__PUBLISHER_NODE_ID__|${STAMPED_PUBLISHER_NODE_ID}|g" \
    -e "s|__IPNS_NAME__|${STAMPED_IPNS_NAME}|g" \
    scripts/install.sh > "$STAMPED_INSTALL"
if grep -Fq '__SOURCE_CONNECT_TICKET__' "$STAMPED_INSTALL"; then
    die "Rendered installer still contains an unresolved source bootstrap placeholder"
fi
if [[ -z "$STAMPED_SOURCE_CONNECT_TICKET" ]] && \
   grep -Fq 'SOURCE_CONNECT_TICKET="${ELASTOS_SOURCE_CONNECT_TICKET:-}"' "$STAMPED_INSTALL"; then
    die "Rendered installer is missing a stamped trusted-source Carrier ticket"
fi
INSTALL_SCRIPT_CID=$(content_publish_object "$STAMPED_INSTALL" "release" "install.sh" \
    --object-did "elastos://installer/${CHANNEL}/${VERSION}" \
    --publisher-did "$SIGNER_DID" \
    --link "head=$HEAD_CID" \
    --link "head.object=$HEAD_OBJECT_CID" \
    --link "release=$RELEASE_CID" \
    --link "release.object=$RELEASE_OBJECT_CID")
# Persist stamped install.sh and metadata so we can re-provide to IPFS network.
mkdir -p artifacts
cp "$STAMPED_INSTALL" artifacts/install.sh
cp "${TMPDIR}/release.json" artifacts/release.json
cp "${TMPDIR}/release-head.json" artifacts/release-head.json
cp "${TMPDIR}/components.json" artifacts/components-x86_64.json
[[ -f "${TMPDIR}/components-${CROSS_ARCH:-}.json" ]] && \
    cp "${TMPDIR}/components-${CROSS_ARCH}.json" "artifacts/components-${CROSS_ARCH}.json"
info "Installer CID: ${INSTALL_SCRIPT_CID}"

# Save release metadata to runtime-owned publisher state for gateway serving.
RUNTIME_DATA_DIR="${HOST_DATA_DIR}"
PUBLISHER_ROOT="${RUNTIME_DATA_DIR}/ElastOS/SystemServices/Publisher"
PUBLISHER_ARTIFACTS_DIR="${PUBLISHER_ROOT}/artifacts"
mkdir -p "${PUBLISHER_ARTIFACTS_DIR}"
cp "${TMPDIR}/release-head.json" "${PUBLISHER_ROOT}/release-head.json"
cp "${TMPDIR}/release.json" "${PUBLISHER_ROOT}/release.json"
cp "$STAMPED_INSTALL" "${PUBLISHER_ROOT}/install.sh"
# Save platform binaries for direct gateway serving
cp "${STAGED_ELASTOS}" "${PUBLISHER_ARTIFACTS_DIR}/elastos-${PLATFORM}"
if [[ -n "${CROSS_ELASTOS:-}" && -f "${CROSS_ELASTOS}" ]]; then
    cp "${CROSS_ELASTOS}" "${PUBLISHER_ARTIFACTS_DIR}/elastos-${CROSS_PLATFORM}"
fi
cp "${TMPDIR}/components.json" "${PUBLISHER_ARTIFACTS_DIR}/components-${PLATFORM}.json"
if [[ -n "${CROSS_PLATFORM:-}" && -f "${TMPDIR}/components-${CROSS_ARCH}.json" ]]; then
    cp "${TMPDIR}/components-${CROSS_ARCH}.json" "${PUBLISHER_ARTIFACTS_DIR}/components-${CROSS_PLATFORM}.json"
fi
# Copy first-party support assets for Carrier-served setup fetches.
for f in "${TMPDIR}/supported-assets-${PLATFORM}"/*; do
    [ -f "$f" ] || continue
    cp -f "$f" "${PUBLISHER_ARTIFACTS_DIR}/$(basename "$f")"
done
if [[ -n "${CROSS_PLATFORM:-}" ]]; then
    for f in "${TMPDIR}/supported-assets-${CROSS_PLATFORM}"/*; do
        [ -f "$f" ] || continue
        cp -f "$f" "${PUBLISHER_ARTIFACTS_DIR}/$(basename "$f")"
    done
fi
# Copy capsule artifacts for Carrier serving (platform-suffixed)
for f in "${ARTIFACTS_DIR}"/*.capsule.tar.gz; do
    [ -f "$f" ] || continue
    base=$(basename "$f" .capsule.tar.gz)
    cp -f "$f" "${PUBLISHER_ARTIFACTS_DIR}/${base}-${PLATFORM}.capsule.tar.gz"
done
if [[ -n "${CROSS_PLATFORM:-}" ]]; then
    CROSS_SRC="${TMPDIR}/artifacts-${CROSS_ARCH}"
    [ -d "$CROSS_SRC" ] || CROSS_SRC="artifacts-${CROSS_ARCH}"
    for f in "${CROSS_SRC}"/*.capsule.tar.gz; do
        [ -f "$f" ] || continue
        base=$(basename "$f" .capsule.tar.gz)
        cp -f "$f" "${PUBLISHER_ARTIFACTS_DIR}/${base}-${CROSS_PLATFORM}.capsule.tar.gz"
    done
fi
info "Saved release artifacts to ${PUBLISHER_ROOT} for gateway serving"

# ── Step 10: Start public gateway URL (best-effort) ──────────────────

PUBLIC_GATEWAY_URL=""
PUBLIC_INSTALL_URL=""
PUBLIC_GATEWAY_PID=""
PUBLIC_GATEWAY_LOG=""
PUBLIC_TUNNEL_PID=""

verify_public_gateway_handoff() {
    local gateway_url="$1"
    local install_url="$2"
    local tunnel_pid="$3"
    local settle_secs="${4:-8}"
    local deadline=$(( $(date +%s) + settle_secs ))

    while [[ $(date +%s) -lt $deadline ]]; do
        if [[ -n "$tunnel_pid" ]] && ! kill -0 "$tunnel_pid" 2>/dev/null; then
            return 1
        fi

        if ! curl -fsSI --max-time 10 "$install_url" >/dev/null 2>&1; then
            sleep 1
            continue
        fi

        if ! curl -fsSI --max-time 10 "${gateway_url}/release-head.json" >/dev/null 2>&1; then
            sleep 1
            continue
        fi

        sleep 1
    done

    if [[ -n "$tunnel_pid" ]] && ! kill -0 "$tunnel_pid" 2>/dev/null; then
        return 1
    fi

    curl -fsSI --max-time 10 "$install_url" >/dev/null 2>&1 &&
        curl -fsSI --max-time 10 "${gateway_url}/release-head.json" >/dev/null 2>&1
}

if [[ "${PUBLISH_PUBLIC_URL}" == true ]]; then
    # Only need cloudflared for public URL (no crosvm/vmlinux — lightweight gateway)
    CLOUDFLARED_BIN="${HOST_DATA_DIR}/bin/cloudflared"
    if [[ ! -x "$CLOUDFLARED_BIN" ]]; then
        info "Installing cloudflared via setup..."
        "$ELASTOS" setup --with cloudflared || true
    fi
    if [[ ! -x "$CLOUDFLARED_BIN" ]]; then
        warn "Skipping public URL step — cloudflared not found at ${CLOUDFLARED_BIN}"
        warn "Install with: elastos setup --with cloudflared"
        PUBLISH_PUBLIC_URL=false
    fi
fi

if [[ "${PUBLISH_PUBLIC_URL}" == true ]]; then
    info "Starting lightweight gateway + cloudflared tunnel..."
    LOG_DIR="${HOST_DATA_DIR}/logs"
    mkdir -p "${LOG_DIR}"
    SAFE_VERSION=$(printf '%s' "$VERSION" | tr -c 'A-Za-z0-9._-' '_')
    PUBLIC_GATEWAY_LOG="${LOG_DIR}/publish-gateway-${SAFE_VERSION}.log"

    # Kill stale gateway/cloudflared processes from previous publishes
    GATEWAY_PORT="${GATEWAY_ADDR##*:}"
    for stale_pid in $(lsof -ti :"$GATEWAY_PORT" 2>/dev/null || true); do
        info "Killing stale process on port ${GATEWAY_PORT} (pid ${stale_pid})"
        kill "$stale_pid" 2>/dev/null || true
    done
    # Kill any orphaned publish cloudflared tunnels
    pkill -f "cloudflared tunnel.*--no-autoupdate" 2>/dev/null || true
    # Kill only the old publish gateway bound to this exact port.
    # Do not kill unrelated `elastos gateway` instances.
    pkill -f "elastos gateway --addr ${GATEWAY_ADDR}" 2>/dev/null || true
    sleep 1

    # Seed runtime registry for the local gateway process.
    RUNTIME_DATA_DIR="${HOST_DATA_DIR}"
    mkdir -p "${RUNTIME_DATA_DIR}"
    cp "${TMPDIR}/components.json" "${RUNTIME_DATA_DIR}/components.json"

    # Start lightweight gateway (no VMs, no sudo, no CAP_NET_ADMIN).
    # Detach fully so the publish command can exit without tearing down
    # the advertised public handoff.
    setsid "$ELASTOS" gateway --addr "$GATEWAY_ADDR" \
        >"$PUBLIC_GATEWAY_LOG" 2>&1 < /dev/null &
    PUBLIC_GATEWAY_PID=$!
    sleep 1

    if ! kill -0 "$PUBLIC_GATEWAY_PID" 2>/dev/null; then
        warn "Lightweight gateway exited immediately. See log: ${PUBLIC_GATEWAY_LOG}"
        tail -n 10 "$PUBLIC_GATEWAY_LOG" 2>/dev/null || true
    else
        # Run cloudflared directly (not in a VM) to tunnel to the lightweight gateway
        GATEWAY_PORT="${GATEWAY_ADDR##*:}"
        CLOUDFLARED_LOG="${LOG_DIR}/publish-tunnel-${SAFE_VERSION}.log"
        setsid "$CLOUDFLARED_BIN" tunnel --url "http://127.0.0.1:${GATEWAY_PORT}" --no-autoupdate \
            >"$CLOUDFLARED_LOG" 2>&1 < /dev/null &
        TUNNEL_PID=$!
        PUBLIC_TUNNEL_PID="$TUNNEL_PID"

        deadline=$(( $(date +%s) + PUBLIC_URL_TIMEOUT ))
        while [[ $(date +%s) -lt $deadline ]]; do
            if ! kill -0 "$TUNNEL_PID" 2>/dev/null; then
                break
            fi

            # Parse cloudflared output for the trycloudflare URL
            if [[ -f "$CLOUDFLARED_LOG" ]]; then
                url=$(grep -Eo 'https://[A-Za-z0-9.-]+\.trycloudflare\.com' "$CLOUDFLARED_LOG" | tail -n1 || true)
                if [[ -n "$url" ]]; then
                    candidate_gateway="$url"
                    candidate_install="${candidate_gateway}/install.sh"
                    if verify_public_gateway_handoff "$candidate_gateway" "$candidate_install" "$TUNNEL_PID" 8; then
                        PUBLIC_GATEWAY_URL="$candidate_gateway"
                        PUBLIC_INSTALL_URL="$candidate_install"
                        break
                    fi
                    warn "Quick public URL appeared but did not stay live long enough to trust."
                    warn "Failing closed; no public URL will be advertised for this publish."
                    kill "$TUNNEL_PID" 2>/dev/null || true
                    kill "$PUBLIC_GATEWAY_PID" 2>/dev/null || true
                    PUBLIC_TUNNEL_PID=""
                    PUBLIC_GATEWAY_PID=""
                    PUBLIC_GATEWAY_URL=""
                    PUBLIC_INSTALL_URL=""
                    break
                fi
            fi
            sleep 1
        done

        if [[ -z "$PUBLIC_GATEWAY_URL" ]]; then
            warn "Public URL not detected within ${PUBLIC_URL_TIMEOUT}s."
            warn "Gateway log: ${PUBLIC_GATEWAY_LOG}"
            warn "Tunnel log:  ${CLOUDFLARED_LOG}"
            if [[ -f "$CLOUDFLARED_LOG" ]]; then
                warn "Last tunnel log lines:"
                tail -n 10 "$CLOUDFLARED_LOG" || true
            fi
            kill "$TUNNEL_PID" 2>/dev/null || true
            kill "$PUBLIC_GATEWAY_PID" 2>/dev/null || true
            PUBLIC_TUNNEL_PID=""
            PUBLIC_GATEWAY_PID=""
        fi
    fi
fi

# ── Step 11: Save state ───────────────────────────────────────────────

echo -n "$RELEASE_CID" > "${STATE_DIR}/last-release-cid"
echo -n "$HEAD_CID" > "${STATE_DIR}/last-release-head-cid"
echo -n "$INSTALL_SCRIPT_CID" > "${STATE_DIR}/last-install-script-cid"
echo -n "$RELEASE_OBJECT_CID" > "${STATE_DIR}/last-release-object-cid"
echo -n "$HEAD_OBJECT_CID" > "${STATE_DIR}/last-release-head-object-cid"
if [[ -n "$PUBLIC_INSTALL_URL" ]]; then
    echo -n "$PUBLIC_INSTALL_URL" > "${STATE_DIR}/last-public-install-url"
fi

# ── Step 12: Print results ────────────────────────────────────────────

echo ""
echo -e "${GREEN}${BOLD}Release published!${NC}"
echo ""
echo -e "  Version:      ${BOLD}${VERSION}${NC}"
echo -e "  Channel:      ${CHANNEL}"
if [[ -n "$CROSS_BINARY_CID" ]]; then
    echo -e "  Platforms:    ${PLATFORM}, ${CROSS_PLATFORM}"
else
    echo -e "  Platform:     ${PLATFORM}"
fi
echo ""
echo -e "  ${BOLD}${PLATFORM}:${NC}"
echo -e "    Binary:       elastos://${BINARY_CID}"
echo -e "    Components:   elastos://${COMPONENTS_CID}"
if [[ -n "$CROSS_BINARY_CID" ]]; then
    echo -e "  ${BOLD}${CROSS_PLATFORM}:${NC}"
    echo -e "    Binary:       elastos://${CROSS_BINARY_CID}"
    echo -e "    Components:   elastos://${CROSS_COMPONENTS_CID}"
fi
echo ""
[[ -n "${SHELL_CID:-}" ]] && echo -e "  Shell:        elastos://${SHELL_CID}"
echo -e "  Release:      elastos://${RELEASE_CID}"
echo -e "  Release obj:  elastos://${RELEASE_OBJECT_CID}"
echo -e "  Head:         elastos://${HEAD_CID}"
echo -e "  Head obj:     elastos://${HEAD_OBJECT_CID}"
echo -e "  Installer:    elastos://${INSTALL_SCRIPT_CID}"
echo -e "  Signer:       ${SIGNER_DID}"
if [[ -n "$IPNS_NAME" ]]; then
    echo -e "  IPNS:         ${IPNS_NAME}"
fi
echo ""
CANONICAL_PUBLIC_GATEWAY_URL=""
CANONICAL_PUBLIC_INSTALL_URL=""
if [[ -z "$PUBLIC_GATEWAY_URL" && -n "${STAMPED_PUBLISHER_GATEWAY:-}" && "${STAMPED_PUBLISHER_GATEWAY}" != "__PUBLISHER_GATEWAY__" ]]; then
    if curl -fsSI --max-time 10 "${STAMPED_PUBLISHER_GATEWAY}/install.sh" >/dev/null 2>&1 && \
       curl -fsSI --max-time 10 "${STAMPED_PUBLISHER_GATEWAY}/release-head.json" >/dev/null 2>&1; then
        CANONICAL_PUBLIC_GATEWAY_URL="${STAMPED_PUBLISHER_GATEWAY}"
        CANONICAL_PUBLIC_INSTALL_URL="${STAMPED_PUBLISHER_GATEWAY}/install.sh"
    fi
fi
if [[ -n "$PUBLIC_GATEWAY_URL" ]]; then
    echo -e "  Public Gate:  ${PUBLIC_GATEWAY_URL}"
    echo -e "  Install URL:  ${PUBLIC_INSTALL_URL}"
elif [[ -n "$CANONICAL_PUBLIC_GATEWAY_URL" ]]; then
    echo -e "  Public Gate:  ${CANONICAL_PUBLIC_GATEWAY_URL}"
    echo -e "  Install URL:  ${CANONICAL_PUBLIC_INSTALL_URL}"
fi
echo ""
if [[ -n "$PUBLIC_GATEWAY_URL" ]]; then
    echo -e "${BOLD}  Install (public gateway):${NC}"
    echo "    curl -fsSL ${PUBLIC_INSTALL_URL} | bash"
    echo ""
    echo -e "${BOLD}  Update from gateway (operator/debug path):${NC}"
    echo "    elastos update --no-p2p --gateway ${PUBLIC_GATEWAY_URL}"
    echo ""
elif [[ -n "$CANONICAL_PUBLIC_GATEWAY_URL" ]]; then
    echo -e "${BOLD}  Install (canonical gateway):${NC}"
    echo "    curl -fsSL ${CANONICAL_PUBLIC_INSTALL_URL} | bash"
    echo ""
    echo -e "${BOLD}  Update from gateway (operator/debug path):${NC}"
    echo "    elastos update --no-p2p --gateway ${CANONICAL_PUBLIC_GATEWAY_URL}"
    echo ""
else
    echo -e "${RED}${BOLD}  Public install: NOT AVAILABLE${NC}"
    echo "    No public gateway was established for this publish."
    echo "    This release is NOT shareable with external users."
    echo "    Fix: re-publish with a live gateway/tunnel, or use operator paths below."
    echo ""
    echo -e "${DIM}  Operator-only bootstrap metadata:${NC}"
    echo -e "${DIM}    INSTALLER_CID=${INSTALL_SCRIPT_CID}${NC}"
    echo -e "${DIM}    HEAD_CID=${HEAD_CID}${NC}"
    echo -e "${DIM}    INSTALL_SCRIPT_CID=${INSTALL_SCRIPT_CID}${NC}"
    echo ""
fi
echo -e "${BOLD}  Manual bootstrap (operator/debug only):${NC}"
echo "    curl -fsSL https://<explicit-gateway>/ipfs/${INSTALL_SCRIPT_CID}/install.sh | bash"
echo "    # Optional explicit anchors:"
echo "    #   --head-cid ${HEAD_CID} --maintainer-did ${SIGNER_DID}"
echo ""
echo -e "${BOLD}  Host Chat After Install:${NC}"
echo "    elastos setup"
echo "    elastos chat --nick host"
if [[ -n "$CROSS_BINARY_CID" ]]; then
    echo ""
    if [[ -n "$PUBLIC_INSTALL_URL" ]]; then
        echo -e "${BOLD}  Jetson Chat After Install:${NC}"
        echo "    curl -fsSL ${PUBLIC_INSTALL_URL} | bash"
        echo "    ~/.local/bin/elastos setup"
        echo "    ~/.local/bin/elastos chat --nick jetson"
        echo ""
        echo -e "${BOLD}  Jetson TUI Chat (microVM):${NC}"
        echo "    ~/.local/bin/elastos setup --profile chat"
        echo "    ~/.local/bin/elastos capsule chat --lifecycle interactive --interactive --config '{\"nick\":\"jetson\"}'"
    else
        echo -e "${DIM}  Jetson: requires public gateway for remote install${NC}"
    fi
fi
if [[ -n "$PUBLIC_GATEWAY_PID" ]]; then
    echo ""
    echo -e "${DIM}  Gateway process: PID ${PUBLIC_GATEWAY_PID}${NC}"
    echo -e "${DIM}  Gateway log:     ${PUBLIC_GATEWAY_LOG}${NC}"
    if [[ -n "$PUBLIC_TUNNEL_PID" ]]; then
        echo -e "${DIM}  Tunnel process:  PID ${PUBLIC_TUNNEL_PID}${NC}"
    fi
fi
echo ""
TOTAL_ARTIFACTS=${#CAPSULES[@]}
if [[ -n "$CROSS_BINARY_CID" ]]; then
    TOTAL_ARTIFACTS=$(( ${#CAPSULES[@]} * 2 ))
fi
echo -e "${DIM}  Capsule artifacts published: ${TOTAL_ARTIFACTS} (${#CAPSULES[@]} capsules × $([ -n "$CROSS_BINARY_CID" ] && echo "2 platforms" || echo "1 platform"))${NC}"
echo -e "${DIM}  Installer downloads: binary + components.json (2 files, platform-specific)${NC}"
echo -e "${DIM}  Native setup assets are stamped per platform in components.json${NC}"
echo -e "${DIM}  Browser/static/WASM capsule assets are stamped once under '*' in components.json${NC}"
echo -e "${DIM}  Capsules downloaded on-demand by supervisor${NC}"
echo ""

# ── Step 13: Provide to system IPFS (DHT advertisement) ──────────────

if command -v ipfs &>/dev/null && ipfs swarm peers &>/dev/null 2>&1; then
    PEER_COUNT=$(ipfs swarm peers 2>/dev/null | wc -l)
    if [[ "$PEER_COUNT" -gt 0 ]]; then
        info "System IPFS node has ${PEER_COUNT} peers — adding content for DHT advertisement..."
        # Add all artifacts to system IPFS
        for f in artifacts/*.capsule.tar.gz artifacts/*.json artifacts/install.sh; do
            [[ -f "$f" ]] && ipfs add -q "$f" >/dev/null 2>&1 || true
        done
        if [[ -d "artifacts-${CROSS_ARCH:-}" ]]; then
            for f in "artifacts-${CROSS_ARCH}"/*.capsule.tar.gz; do
                [[ -f "$f" ]] && ipfs add -q "$f" >/dev/null 2>&1 || true
            done
        fi
        # Add binaries
        ipfs add -q "$STAGED_ELASTOS" >/dev/null 2>&1 || true
        [[ -n "${STAGED_CROSS:-}" && -f "${STAGED_CROSS:-}" ]] && \
            ipfs add -q "$STAGED_CROSS" >/dev/null 2>&1 || true
        # Provide key CIDs to DHT
        for cid in "$RELEASE_CID" "$HEAD_CID" "$INSTALL_SCRIPT_CID" \
                   "$BINARY_CID" "$COMPONENTS_CID" "$SHELL_CID" \
                   ${CROSS_BINARY_CID:+"$CROSS_BINARY_CID"} \
                   ${CROSS_COMPONENTS_CID:+"$CROSS_COMPONENTS_CID"}; do
            ipfs routing provide "$cid" >/dev/null 2>&1 || true
        done
        info "Content provided to IPFS DHT (${PEER_COUNT} peers)"
    fi
fi
