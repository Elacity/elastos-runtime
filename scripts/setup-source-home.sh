#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
    cat <<'EOF'
Usage:
  scripts/setup-source-home.sh

Builds and installs source-based runtime providers into the Home data directory
resolved from HOME/XDG_DATA_HOME.

Configure tool paths with:
  ELASTOS_CARGO_BIN
  ELASTOS_NODE_BIN
  ELASTOS_DEBUGFS_BIN
  ELASTOS_COLLABORATION_STARTUP_MODE (configured|isolated)
  ELASTOS_COLLABORATION_STARTUP_CONFIG_INPUT
  ELASTOS_BROWSER_NATIVE_PROXY_BIN
  ELASTOS_BROWSER_VM_RUNTIME_RELAY_BIN
  ELASTOS_BROWSER_VM_GUEST_CONTROL_BRIDGE_BIN
  ELASTOS_BROWSER_VM_ARTIFACT_DATA_DIR
  ELASTOS_BROWSER_VM_BACKUP_RETENTION (must be 1)

To target a non-default runtime root, set HOME or XDG_DATA_HOME before running.
ELASTOS_DATA_DIR is intentionally not accepted as a gateway data-root override.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    usage
    exit 0
fi

if [[ "$#" -gt 0 ]]; then
    echo "unknown argument: $1" >&2
    usage >&2
    exit 2
fi

detect_platform() {
    case "$(uname -s)-$(uname -m)" in
        Linux-x86_64) printf '%s\n' "linux-amd64" ;;
        Linux-aarch64|Linux-arm64) printf '%s\n' "linux-arm64" ;;
        Darwin-arm64) printf '%s\n' "darwin-arm64" ;;
        *)
            echo "Unsupported source-home platform: $(uname -s)-$(uname -m)" >&2
            exit 1
            ;;
    esac
}

default_data_dir() {
    case "$(uname -s)" in
        Darwin) printf '%s\n' "${HOME}/Library/Application Support/elastos" ;;
        Linux) printf '%s\n' "${XDG_DATA_HOME:-${HOME}/.local/share}/elastos" ;;
        *)
            echo "Unsupported OS: $(uname -s)" >&2
            exit 1
            ;;
    esac
}

cargo_target_root_for_manifest() {
    local manifest_path="$1"
    local manifest_dir
    manifest_dir="$(cd "$(dirname "${manifest_path}")" && pwd)"
    if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
        if [[ "${CARGO_TARGET_DIR}" = /* ]]; then
            printf '%s\n' "${CARGO_TARGET_DIR}"
        else
            printf '%s\n' "${ROOT}/${CARGO_TARGET_DIR}"
        fi
    elif [[ "${manifest_dir}" == "${ROOT}/elastos"* ]] && ! grep -q '^\[workspace\]' "${manifest_path}"; then
        printf '%s\n' "${ROOT}/elastos/target"
    else
        printf '%s\n' "${manifest_dir}/target"
    fi
}

cargo_built_binary_path() {
    local manifest_path="$1"
    local profile="$2"
    local binary="$3"
    printf '%s\n' "$(cargo_target_root_for_manifest "${manifest_path}")/${profile}/${binary}"
}

find_cargo() {
    if [[ -n "${ELASTOS_CARGO_BIN:-}" && -x "${ELASTOS_CARGO_BIN}" ]]; then
        printf '%s\n' "${ELASTOS_CARGO_BIN}"
        return
    fi
    if command -v cargo >/dev/null 2>&1; then
        command -v cargo
        return
    fi
    if [[ -x "${HOME}/.cargo/bin/cargo" ]]; then
        printf '%s\n' "${HOME}/.cargo/bin/cargo"
        return
    fi
    local candidate
    candidate="$(find "${HOME}/.rustup/toolchains" -path '*/bin/cargo' -type f 2>/dev/null | sort | tail -n 1 || true)"
    if [[ -n "$candidate" && -x "$candidate" ]]; then
        printf '%s\n' "$candidate"
        return
    fi
    echo "cargo not found. Install Rust or add cargo to PATH." >&2
    exit 1
}

require_supported_rust() {
    local rustc_bin="$1"
    local output
    local version
    local major
    local minor

    output="$(cd "$ROOT" && "$rustc_bin" --version)"
    version="$(printf '%s\n' "$output" | awk '{print $2}')"
    if [[ ! "$version" =~ ^([0-9]+)\.([0-9]+)(\.[0-9]+)?([+-].*)?$ ]]; then
        echo "Could not parse Rust version from: $output" >&2
        exit 1
    fi
    major="${BASH_REMATCH[1]}"
    minor="${BASH_REMATCH[2]}"
    if (( major < 1 || (major == 1 && minor < 91) )); then
        echo "Rust 1.91 or newer is required; found ${version}." >&2
        exit 1
    fi
}

find_node() {
    if [[ -n "${ELASTOS_NODE_BIN:-}" && -x "${ELASTOS_NODE_BIN}" ]]; then
        printf '%s\n' "${ELASTOS_NODE_BIN}"
        return
    fi
    if command -v node >/dev/null 2>&1; then
        command -v node
        return
    fi
    for candidate in \
        /opt/homebrew/bin/node \
        /usr/local/bin/node
    do
        if [[ -x "$candidate" ]]; then
            printf '%s\n' "$candidate"
            return
        fi
    done
    local candidate
    candidate="$(find "${HOME}/.elastos/node" -path '*/bin/node' -type f 2>/dev/null | sort | tail -n 1 || true)"
    if [[ -n "$candidate" && -x "$candidate" ]]; then
        printf '%s\n' "$candidate"
        return
    fi
    candidate="$(find "${HOME}/.nvm/versions/node" -path '*/bin/node' -type f 2>/dev/null | sort | tail -n 1 || true)"
    if [[ -n "$candidate" && -x "$candidate" ]]; then
        printf '%s\n' "$candidate"
        return
    fi
    echo "node not found. Install Node or set ELASTOS_NODE_BIN to an executable node binary." >&2
    exit 1
}

find_debugfs() {
    if [[ -n "${ELASTOS_DEBUGFS_BIN:-}" && -x "${ELASTOS_DEBUGFS_BIN}" ]]; then
        printf '%s\n' "${ELASTOS_DEBUGFS_BIN}"
        return
    fi
    if command -v debugfs >/dev/null 2>&1; then
        command -v debugfs
        return
    fi
    for candidate in \
        /opt/homebrew/opt/e2fsprogs/sbin/debugfs \
        /usr/local/opt/e2fsprogs/sbin/debugfs \
        /usr/sbin/debugfs \
        /sbin/debugfs
    do
        if [[ -x "$candidate" ]]; then
            printf '%s\n' "$candidate"
            return
        fi
    done
}

find_turnserver() {
    if [[ -n "${ELASTOS_BROWSER_VM_TURNSERVER_BIN:-}" && -x "${ELASTOS_BROWSER_VM_TURNSERVER_BIN}" ]]; then
        printf '%s\n' "${ELASTOS_BROWSER_VM_TURNSERVER_BIN}"
        return
    fi
    if [[ -n "${ELASTOS_TURNSERVER_BIN:-}" && -x "${ELASTOS_TURNSERVER_BIN}" ]]; then
        printf '%s\n' "${ELASTOS_TURNSERVER_BIN}"
        return
    fi
    if command -v turnserver >/dev/null 2>&1; then
        command -v turnserver
        return
    fi
    for candidate in \
        /usr/bin/turnserver \
        /usr/local/bin/turnserver \
        /opt/homebrew/bin/turnserver
    do
        if [[ -x "$candidate" ]]; then
            printf '%s\n' "$candidate"
            return
        fi
    done
    return 1
}

find_vz_turn_program() {
    if [[ -n "${ELASTOS_BROWSER_VM_TURN_PROGRAM:-}" ]]; then
        if [[ ! -x "${ELASTOS_BROWSER_VM_TURN_PROGRAM}" ]]; then
            echo "ELASTOS_BROWSER_VM_TURN_PROGRAM is not executable: ${ELASTOS_BROWSER_VM_TURN_PROGRAM}" >&2
            return 2
        fi
        printf '%s\n' "${ELASTOS_BROWSER_VM_TURN_PROGRAM}"
        return
    fi
    if command -v turnserver >/dev/null 2>&1; then
        command -v turnserver
        return
    fi
    local candidate
    for candidate in \
        /usr/bin/turnserver \
        /usr/local/bin/turnserver \
        /opt/homebrew/bin/turnserver
    do
        if [[ -x "$candidate" ]]; then
            printf '%s\n' "$candidate"
            return
        fi
    done
    return 1
}

browser_vm_target_platform() {
    case "$(uname -s)-$(uname -m)" in
        Linux-x86_64) printf '%s\n' "linux-amd64" ;;
        Linux-aarch64|Linux-arm64|Darwin-arm64) printf '%s\n' "linux-arm64" ;;
        *) return 1 ;;
    esac
}

resolve_browser_vm_native_proxy_source() {
    local target_platform="$1"
    local candidate

    if [[ -n "${ELASTOS_BROWSER_NATIVE_PROXY_BIN:-}" ]]; then
        if [[ ! -x "${ELASTOS_BROWSER_NATIVE_PROXY_BIN}" ]]; then
            echo "ELASTOS_BROWSER_NATIVE_PROXY_BIN is not executable: ${ELASTOS_BROWSER_NATIVE_PROXY_BIN}" >&2
            return 2
        fi
        printf '%s\n' "${ELASTOS_BROWSER_NATIVE_PROXY_BIN}"
        return
    fi

    case "$target_platform" in
        linux-arm64)
            for candidate in \
                "${ROOT}/elastos/tools/browser-native-proxy-engine/target/aarch64-unknown-linux-musl/release/browser-native-proxy-engine" \
                "${ROOT}/elastos/tools/browser-native-proxy-engine/target/aarch64-unknown-linux-gnu/release/browser-native-proxy-engine"
            do
                if [[ -x "$candidate" ]]; then
                    printf '%s\n' "$candidate"
                    return
                fi
            done
            ;;
        linux-amd64)
            for candidate in \
                "${ROOT}/elastos/tools/browser-native-proxy-engine/target/x86_64-unknown-linux-musl/release/browser-native-proxy-engine" \
                "${ROOT}/elastos/tools/browser-native-proxy-engine/target/x86_64-unknown-linux-gnu/release/browser-native-proxy-engine"
            do
                if [[ -x "$candidate" ]]; then
                    printf '%s\n' "$candidate"
                    return
                fi
            done
            ;;
    esac
    return 1
}

browser_vm_guest_rust_target() {
    case "$1" in
        linux-arm64) printf '%s\n' "aarch64-unknown-linux-musl" ;;
        linux-amd64) printf '%s\n' "x86_64-unknown-linux-musl" ;;
        *) return 1 ;;
    esac
}

build_browser_vm_guest_helper() {
    local manifest="$1"
    local rust_target="$2"
    local linker_env=""
    local linker=""

    case "$rust_target" in
        aarch64-unknown-linux-musl)
            linker_env="CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER"
            ;;
        x86_64-unknown-linux-musl)
            linker_env="CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER"
            ;;
        *)
            echo "unsupported Browser VM guest Rust target: $rust_target" >&2
            exit 1
            ;;
    esac

    if [[ "$(uname -s)" == "Darwin" && -z "${!linker_env:-}" ]]; then
        local rustc_bin
        local rust_sysroot
        rustc_bin="$(command -v rustc || true)"
        if [[ -z "$rustc_bin" ]]; then
            echo "rustc not found; Browser VM guest helper cross-build cannot locate rust-lld" >&2
            exit 1
        fi
        rust_sysroot="$("$rustc_bin" --print sysroot)"
        linker="$(find "$rust_sysroot" -type f -name rust-lld -perm -111 -print -quit 2>/dev/null || true)"
        if [[ -z "$linker" ]]; then
            echo "rust-lld not found in $rust_sysroot; Browser VM guest helper cross-build is unavailable" >&2
            exit 1
        fi
        env "$linker_env=$linker" "$CARGO_BIN" build --quiet \
            --manifest-path "$manifest" \
            --target "$rust_target" \
            --release
        return
    fi

    "$CARGO_BIN" build --quiet \
        --manifest-path "$manifest" \
        --target "$rust_target" \
        --release
}

build_browser_vm_guest_helpers() {
    local target_platform
    local rust_target
    target_platform="$(browser_vm_target_platform || true)"
    rust_target="$(browser_vm_guest_rust_target "$target_platform" || true)"
    if [[ -z "$rust_target" ]]; then
        echo "Browser VM guest helper build is unsupported on ${target_platform:-this platform}" >&2
        exit 1
    fi
    ensure_rust_target_installed "$rust_target"

    echo "[setup-source-home] build Browser VM guest relay helpers"
    build_browser_vm_guest_helper \
        "${ROOT}/elastos/tools/browser-vm-runtime-relay/Cargo.toml" \
        "$rust_target"
    build_browser_vm_guest_helper \
        "${ROOT}/elastos/tools/browser-vm-guest-control-bridge/Cargo.toml" \
        "$rust_target"
}

resolve_browser_vm_guest_helper_source() {
    local label="$1"
    local env_name="$2"
    local crate_name="$3"
    local binary_name="$4"
    local target_platform="$5"
    local explicit="${!env_name:-}"
    local rust_target
    local candidate
    local manifest

    if [[ -n "$explicit" ]]; then
        if [[ ! -x "$explicit" ]]; then
            echo "$env_name is not executable: $explicit" >&2
            return 2
        fi
        printf '%s\n' "$explicit"
        return
    fi

    rust_target="$(browser_vm_guest_rust_target "$target_platform" || true)"
    if [[ -z "$rust_target" ]]; then
        return 1
    fi
    manifest="${ROOT}/elastos/tools/${crate_name}/Cargo.toml"
    candidate="$(cargo_target_root_for_manifest "$manifest")/${rust_target}/release/${binary_name}"
    if [[ ! -x "$candidate" ]]; then
        echo "$label source build is missing: $candidate" >&2
        return 1
    fi
    printf '%s\n' "$candidate"
}

validate_linux_guest_binary() {
    local label="$1"
    local path="$2"
    local target_platform="$3"
    python3 - "$label" "$path" "$target_platform" <<'PY'
import pathlib
import struct
import sys

label, path, target_platform = sys.argv[1:]
data = pathlib.Path(path).read_bytes()[:20]
if len(data) < 20 or data[:4] != b"\x7fELF":
    raise SystemExit(f"{label} must be a Linux ELF guest binary for {target_platform}")
if data[5] == 1:
    endian = "<"
elif data[5] == 2:
    endian = ">"
else:
    raise SystemExit(f"{label} has an invalid ELF data encoding")
machine = struct.unpack(endian + "H", data[18:20])[0]
expected = {
    "linux-amd64": 62,
    "linux-arm64": 183,
}[target_platform]
if machine != expected:
    names = {62: "x86_64", 183: "aarch64"}
    got = names.get(machine, f"e_machine={machine}")
    want = names[expected]
    raise SystemExit(f"{label} must be {want} for {target_platform}, got {got}")
PY
}

configure_rust_toolchain_env() {
    local cargo_bin="$1"
    local cargo_home_guess
    local home_guess

    if [[ -z "${CARGO_HOME:-}" && "$cargo_bin" == */.cargo/bin/cargo ]]; then
        cargo_home_guess="${cargo_bin%/.cargo/bin/cargo}/.cargo"
        if [[ -d "$cargo_home_guess" ]]; then
            export CARGO_HOME="$cargo_home_guess"
        fi
    fi

    if [[ -z "${RUSTUP_HOME:-}" && -n "${CARGO_HOME:-}" && "$(basename "$CARGO_HOME")" == ".cargo" ]]; then
        home_guess="$(dirname "$CARGO_HOME")"
        if [[ -d "${home_guess}/.rustup" ]]; then
            export RUSTUP_HOME="${home_guess}/.rustup"
        fi
    fi
}

clone_or_copy_file() {
    local source="$1"
    local dest="$2"
    cp -c "$source" "$dest" 2>/dev/null ||
        cp --reflink=auto -p "$source" "$dest" 2>/dev/null ||
        cp -p "$source" "$dest"
    touch "$dest"
}

browser_vm_backup_retention() {
    local retention="${ELASTOS_BROWSER_VM_BACKUP_RETENTION:-1}"
    if [[ "$retention" != "1" ]]; then
        echo "ELASTOS_BROWSER_VM_BACKUP_RETENTION must be 1" >&2
        exit 2
    fi
    printf '%s\n' "$retention"
}

file_mtime() {
    case "$(uname -s)" in
        Darwin) stat -f '%m' "$1" ;;
        Linux) stat -c '%Y' "$1" ;;
        *) return 1 ;;
    esac
}

prune_file_backups() {
    local source="$1"
    local keep="$2"
    local candidate
    local oldest_index
    local oldest_mtime
    local candidate_mtime
    local index
    local backups=()

    for candidate in "${source}".before-*; do
        if [[ -f "$candidate" && ! -L "$candidate" ]]; then
            backups+=("$candidate")
        fi
    done
    while (( ${#backups[@]} > keep )); do
        oldest_index=0
        oldest_mtime="$(file_mtime "${backups[0]}")"
        for (( index = 1; index < ${#backups[@]}; index += 1 )); do
            candidate_mtime="$(file_mtime "${backups[$index]}")"
            if (( candidate_mtime < oldest_mtime )); then
                oldest_index="$index"
                oldest_mtime="$candidate_mtime"
            fi
        done
        candidate="${backups[$oldest_index]}"
        rm -f -- "$candidate"
        echo "[setup-source-home] pruned stale Browser VM backup: ${candidate}"
        unset "backups[$oldest_index]"
        backups=("${backups[@]}")
    done
}

require_minimum_free_space() {
    local path="$1"
    # A full cold setup measures roughly 10 GiB across the workspace release
    # build, the capsule builds, the cargo registry, and the installed data
    # root; 16 GiB leaves headroom for architecture variance, staging, and
    # the VM backup set. The gate is absolute because a percentage scales
    # with volume size and demands space setup never uses on large disks.
    local minimum_gib=16
    local minimum_kib=$((minimum_gib * 1024 * 1024))
    local available
    local available_gib

    available="$(df -Pk "$path" | awk 'NR == 2 {print $4}')"
    if [[ ! "$available" =~ ^[0-9]+$ ]]; then
        echo "Could not determine free space for source-home volume: ${path}" >&2
        exit 1
    fi
    available_gib=$((available / 1024 / 1024))
    if (( available < minimum_kib )); then
        echo "Source-home setup requires at least ${minimum_gib} GiB free space; ${path} has ${available_gib} GiB." >&2
        echo "Reconcile worktrees, Cargo targets, VM state, and rollback artifacts before building." >&2
        exit 1
    fi
    echo "[setup-source-home] free-space gate: ${available_gib} GiB available"
}

DEFAULT_DATA_DIR="$(default_data_dir)"
if [[ -n "${ELASTOS_DATA_DIR:-}" && "${ELASTOS_DATA_DIR}" != "${DEFAULT_DATA_DIR}" ]]; then
    echo "ELASTOS_DATA_DIR is not a gateway data-root override." >&2
    echo "Set HOME or XDG_DATA_HOME before running this script so dirs::data_dir resolves to the intended runtime root." >&2
    echo "Expected data dir from current environment: ${DEFAULT_DATA_DIR}" >&2
    exit 1
fi
DATA_DIR="${DEFAULT_DATA_DIR}"
PLATFORM="$(detect_platform)"
CARGO_BIN="$(find_cargo)"
NODE_BIN="$(find_node)"
BROWSER_VM_ROOTFS_BACKUP=""
SOURCE_HOME_KUBO_INSTALLED="0"
configure_rust_toolchain_env "$CARGO_BIN"
export PATH="$(dirname "$CARGO_BIN"):$(dirname "$NODE_BIN"):${PATH}"
require_supported_rust "$(command -v rustc)"

provider_runtime_names() {
    COMPONENTS_SRC="${ROOT}/components.json" python3 - <<'PY'
import json
import os
import pathlib

manifest = json.loads(pathlib.Path(os.environ["COMPONENTS_SRC"]).read_text())
for name, component in manifest.get("external", {}).items():
    runtime = component.get("provider_runtime")
    if not isinstance(runtime, dict):
        continue
    if runtime.get("role") != "provider":
        raise SystemExit(f"{name} provider_runtime.role must be provider")
    if runtime.get("substrate") != "native":
        raise SystemExit(f"{name} provider_runtime.substrate must be native")
    if runtime.get("runtime_abi") != "elastos.provider-stdio/v1":
        raise SystemExit(f"{name} provider_runtime.runtime_abi must be elastos.provider-stdio/v1")
    if runtime.get("execution") != "native-provider":
        raise SystemExit(f"{name} provider_runtime.execution must be native-provider")
    provides = runtime.get("provides")
    if not isinstance(provides, str) or not provides:
        raise SystemExit(f"{name} provider_runtime.provides must be a non-empty string")
    runtime_only = runtime.get("runtime_only", False)
    if not isinstance(runtime_only, bool):
        raise SystemExit(f"{name} provider_runtime.runtime_only must be a boolean")
    if runtime_only and (
        provides.startswith("-")
        or provides.endswith("-")
        or any(ch not in "abcdefghijklmnopqrstuvwxyz0123456789-" for ch in provides)
    ):
        raise SystemExit(f"{name} provider_runtime.provides must be a lowercase Runtime target")
    print(name)
PY
}

source_home_helper_binary_names() {
    printf '%s\n' "browser-local-exit"
    if [[ "$(uname -s)" == "Linux" ]]; then
        printf '%s\n' \
            browser-engine-supervisor \
            browser-native-proxy-engine \
            browser-stream-bridge
    fi
}

source_home_binary_names() {
    provider_runtime_names
    source_home_helper_binary_names
}

SOURCE_HOME_BINARY_NAMES_JSON="$(source_home_binary_names | python3 -c 'import json,sys; print(json.dumps([line.strip() for line in sys.stdin if line.strip()]))')"

APP_CAPSULES=(
    home-cli
    home-gui
    home
    system
    services
    people
    browser
    documents
    library
    elacity-player
    marketplace
    archive-manager
    inbox
    assistant
    wallet
    wallet-metamask
    wallet-unisat
    wallet-walletconnect
    gba-emulator
    gba-ucity
    gba-nonogram
    chat-room
)

APP_CAPSULES_JSON="$(printf '%s\n' "${APP_CAPSULES[@]}" | python3 -c 'import json,sys; print(json.dumps([line.strip() for line in sys.stdin if line.strip()]))')"

# Historical source-home app trees that this installer owns but no longer installs.
# Keep this list narrow: unrelated or user-installed capsule directories are preserved.
RETIRED_SOURCE_HOME_CAPSULES=(
    chat-wasm
    gba-engine-provider
)

RETIRED_SOURCE_HOME_PROVIDER_BINARIES=(
    gba-engine-provider
    ai-provider
    llama-provider
)

collaboration_startup_config_destination() {
    printf '%s\n' "${DATA_DIR}/collaboration-network-v1.json"
}

collaboration_startup_mode() {
    case "${ELASTOS_COLLABORATION_STARTUP_MODE:-}" in
        configured|isolated) printf '%s\n' "${ELASTOS_COLLABORATION_STARTUP_MODE}" ;;
        "")
            echo "ELASTOS_COLLABORATION_STARTUP_MODE must be set to configured or isolated." >&2
            exit 1
            ;;
        *)
            echo "unsupported ELASTOS_COLLABORATION_STARTUP_MODE: ${ELASTOS_COLLABORATION_STARTUP_MODE}" >&2
            exit 1
            ;;
    esac
}

validate_collaboration_startup_mode() {
    local mode
    mode="$(collaboration_startup_mode)"
    local input_path="${ELASTOS_COLLABORATION_STARTUP_CONFIG_INPUT:-}"
    local destination
    destination="$(collaboration_startup_config_destination)"
    case "$mode" in
        configured)
            if [[ -z "$input_path" ]]; then
                echo "configured collaboration setup requires ELASTOS_COLLABORATION_STARTUP_CONFIG_INPUT." >&2
                exit 1
            fi
            ;;
        isolated)
            if [[ -n "$input_path" ]]; then
                echo "isolated collaboration setup rejects ELASTOS_COLLABORATION_STARTUP_CONFIG_INPUT." >&2
                exit 1
            fi
            if [[ -e "$destination" || -L "$destination" ]]; then
                echo "isolated collaboration setup refuses an existing startup config: ${destination}" >&2
                exit 1
            fi
            ;;
    esac
}

verify_collaboration_startup_config_input() {
    local mode
    mode="$(collaboration_startup_mode)"
    if [[ "$mode" != "configured" ]]; then
        return 0
    fi
    local input_path="${ELASTOS_COLLABORATION_STARTUP_CONFIG_INPUT:-}"
    if [[ ! -f "$input_path" ]]; then
        echo "collaboration startup config input is unavailable: ${input_path}" >&2
        exit 1
    fi
    local elastos_bin
    elastos_bin="$(cargo_built_binary_path "${ROOT}/elastos/Cargo.toml" release elastos)"
    "$elastos_bin" collaboration-config verify --input "$input_path" >/dev/null
}

install_collaboration_startup_config() {
    local mode
    mode="$(collaboration_startup_mode)"
    if [[ "$mode" != "configured" ]]; then
        return 0
    fi
    local input_path="${ELASTOS_COLLABORATION_STARTUP_CONFIG_INPUT:-}"
    local destination
    destination="$(collaboration_startup_config_destination)"
    mkdir -p "${DATA_DIR}"
    if [[ -e "$destination" || -L "$destination" ]]; then
        if [[ -f "$destination" && ! -L "$destination" ]] && cmp -s "$input_path" "$destination"; then
            chmod 600 "$destination"
            echo "[setup-source-home] preserve identical collaboration startup config: ${destination}"
            return 0
        fi
        echo "refusing to replace collaboration startup config with different bytes: ${destination}" >&2
        exit 1
    fi
    local temp_path
    temp_path="$(mktemp "${DATA_DIR}/.collaboration-network-v1.XXXXXX.tmp")"
    chmod 600 "$temp_path"
    cat "$input_path" >"$temp_path"
    chmod 600 "$temp_path"
    if ! cmp -s "$input_path" "$temp_path"; then
        rm -f "$temp_path"
        echo "collaboration startup config install changed bytes unexpectedly" >&2
        exit 1
    fi
    if ! ln "$temp_path" "$destination"; then
        rm -f "$temp_path"
        echo "failed to install collaboration startup config: ${destination}" >&2
        exit 1
    fi
    rm -f "$temp_path"
    chmod 600 "$destination"
    echo "[setup-source-home] installed collaboration startup config: ${destination}"
}

ensure_owner_only_data_dir() {
    mkdir -p "${DATA_DIR}"
    chmod 700 "${DATA_DIR}"
}

capsule_entrypoint() {
    local manifest="$1"
    python3 - "$manifest" <<'PY'
import json
import pathlib
import sys

manifest = json.loads(pathlib.Path(sys.argv[1]).read_text())
entrypoint = manifest.get("entrypoint")
if not entrypoint:
    raise SystemExit(f"{sys.argv[1]} missing entrypoint")
print(entrypoint)
PY
}

capsule_runtime_abi() {
    local manifest="$1"
    python3 - "$manifest" <<'PY'
import json
import pathlib
import sys

manifest = json.loads(pathlib.Path(sys.argv[1]).read_text())
print(manifest.get("runtime_abi", ""))
PY
}

ensure_rust_target_installed() {
    local target="$1"
    if command -v rustup >/dev/null 2>&1 && ! rustup target list --installed 2>/dev/null | grep -qx "$target"; then
        rustup target add "$target"
    fi
}

build_component_capsule() {
    local src="$1"

    ensure_rust_target_installed "wasm32-unknown-unknown"
    CARGO_BIN="$CARGO_BIN" "${ROOT}/scripts/build-component-capsule.sh" "$src"
}

is_runtime_projection_capsule() {
    [[ "$1" == "elastos.runtime-projection/v1" ]]
}

is_content_data_capsule() {
    local manifest="$1"
    python3 - "$manifest" <<'PY'
import json
import pathlib
import sys

manifest = json.loads(pathlib.Path(sys.argv[1]).read_text())
print("yes" if manifest.get("role") == "content" and manifest.get("type") == "data" else "no")
PY
}

copy_capsule_tree() {
    local src="$1"
    local dest="$2"

    mkdir -p "$(dirname "$dest")"
    if command -v rsync >/dev/null 2>&1; then
        rsync -a --delete --exclude target "$src/" "$dest/"
        return
    fi

    rm -rf "$dest"
    mkdir -p "$dest"
    (cd "$src" && tar --exclude './target' -cf - .) | (cd "$dest" && tar -xf -)
}

install_app_capsules() {
    local capsule src dest entrypoint runtime_abi built_entrypoint

    mkdir -p "${DATA_DIR}/capsules"
    for capsule in "${APP_CAPSULES[@]}"; do
        src="${ROOT}/capsules/${capsule}"
        dest="${DATA_DIR}/capsules/${capsule}"
        entrypoint="$(capsule_entrypoint "${src}/capsule.json")"
        runtime_abi="$(capsule_runtime_abi "${src}/capsule.json")"
        if [[ "$runtime_abi" == "elastos.component/v1" ]] || is_runtime_projection_capsule "$runtime_abi" || [[ "$(is_content_data_capsule "${src}/capsule.json")" == "yes" ]]; then
            built_entrypoint="${src}/${entrypoint}"
        else
            echo "${capsule} uses unsupported runtime_abi '${runtime_abi:-unset}'; first-party product capsules must use elastos.component/v1, elastos.runtime-projection/v1, or role=content type=data" >&2
            exit 1
        fi

        if [[ ! -f "$built_entrypoint" ]]; then
            echo "${capsule} entrypoint missing after build: ${built_entrypoint}" >&2
            exit 1
        fi

        # Generated source materializations are obsolete once source-home installs
        # the canonical capsule tree and manifest-selected entrypoint.
        rm -rf "${DATA_DIR}/dev-capsules/${capsule}"
        copy_capsule_tree "$src" "$dest"
        if is_runtime_projection_capsule "$runtime_abi"; then
            find "$dest" -maxdepth 1 -type f -name '*.wasm' -delete
        fi
        mkdir -p "${dest}/$(dirname "$entrypoint")"
        install -m 644 "$built_entrypoint" "${dest}/${entrypoint}"
    done
}

sign_browser_vz_supervisor() {
    local binary="$1"

    if [[ "$(uname -s)" != "Darwin" ]]; then
        return
    fi
    "${ROOT}/scripts/dev/sign-elastos-vz/sign.sh" "$binary"
}

refresh_browser_vm_rootfs_file() {
    local rootfs="$1"
    local source="$2"
    local guest_path="$3"
    local mode="$4"
    local label="$5"
    local debugfs="$6"
    local current_copy
    local staged_source
    local updated_copy
    local commands_file
    current_copy="$(mktemp)"
    staged_source="$(mktemp)"
    updated_copy="$(mktemp)"
    commands_file="$(mktemp)"
    cp "$source" "$staged_source"

    if "$debugfs" -R "cat ${guest_path}" "$rootfs" > "$current_copy" 2>/dev/null &&
        cmp -s "$source" "$current_copy"; then
        rm -f "$current_copy" "$staged_source" "$updated_copy" "$commands_file"
        return
    fi

    if [[ -z "$BROWSER_VM_ROOTFS_BACKUP" ]]; then
        BROWSER_VM_ROOTFS_BACKUP="${rootfs}.before-${label}-$(date -u +%Y%m%dT%H%M%SZ)"
        clone_or_copy_file "$rootfs" "$BROWSER_VM_ROOTFS_BACKUP"
    fi

    cat > "$commands_file" <<EOF
rm ${guest_path}
write $staged_source ${guest_path}
set_inode_field ${guest_path} mode ${mode}
EOF
    "$debugfs" -w -f "$commands_file" "$rootfs" >/dev/null
    "$debugfs" -R "cat ${guest_path}" "$rootfs" > "$updated_copy" 2>/dev/null
    if ! cmp -s "$source" "$updated_copy"; then
        echo "Browser VM rootfs ${label} refresh did not verify; backup kept at ${BROWSER_VM_ROOTFS_BACKUP}" >&2
        exit 1
    fi
    rm -f "$current_copy" "$staged_source" "$updated_copy" "$commands_file"
    echo "[setup-source-home] refreshed Browser VM rootfs ${label}: ${rootfs}"
}

extract_browser_vm_selkies_start() {
    local target="$1"
    python3 - "${ROOT}/scripts/build/stage-browser-vm-target.sh" "$target" <<'PY'
import pathlib
import sys

stage_script = pathlib.Path(sys.argv[1])
target = pathlib.Path(sys.argv[2])
text = stage_script.read_text()
start_marker = 'cat > "$staging_dir/opt/elastos/bin/browser-vm-selkies-start" <<\'SH\'\n'
end_marker = '\nSH\nchmod 755 "$staging_dir/opt/elastos/bin/browser-vm-selkies-start"'
try:
    start = text.index(start_marker) + len(start_marker)
    end = text.index(end_marker, start)
except ValueError as exc:
    raise SystemExit(f"could not extract browser-vm-selkies-start from stage script: {exc}")
target.write_text(text[start:end])
PY
}

extract_browser_vm_init() {
    local target="$1"
    python3 - "${ROOT}/scripts/build/stage-browser-vm-target.sh" "$target" <<'PY'
import pathlib
import sys

stage_script = pathlib.Path(sys.argv[1])
target = pathlib.Path(sys.argv[2])
text = stage_script.read_text()
start_marker = 'cat > "$staging_dir/opt/elastos/bin/browser-vm-init" <<\'SH\'\n'
end_marker = '\nSH\nchmod 755 "$staging_dir/opt/elastos/bin/browser-vm-init"'
try:
    start = text.index(start_marker) + len(start_marker)
    end = text.index(end_marker, start)
except ValueError as exc:
    raise SystemExit(f"could not extract browser-vm-init from stage script: {exc}")
target.write_text(text[start:end])
PY
}

patch_browser_vm_selkies_app_source() {
    local target="$1"
    python3 - "$target" <<'PY'
from pathlib import Path
import re
import sys

path = Path(sys.argv[1])
text = path.read_text()
marker = '        self.webrtcbin.set_property("latency", 0)\n'
relay_patch = '''        elastos_ice_transport_policy = os.environ.get("ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY", "").strip().lower()
        if elastos_ice_transport_policy:
            if elastos_ice_transport_policy not in ("all", "relay"):
                raise GSTWebRTCAppError("ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY must be all or relay")
            try:
                policy_value = getattr(GstWebRTC.WebRTCICETransportPolicy, elastos_ice_transport_policy.upper())
            except AttributeError:
                policy_value = elastos_ice_transport_policy
            self.webrtcbin.set_property("ice-transport-policy", policy_value)
            logger.info("using ICE transport policy: %s", elastos_ice_transport_policy)
'''
turn_marker = '''        if self.turn_servers:
            for i, turn_server in enumerate(self.turn_servers):
                logger.info("updating TURN server")
                if i == 0:
                    self.webrtcbin.set_property("turn-server", turn_server)
                else:
                    self.webrtcbin.emit("add-turn-server", turn_server)
'''
turn_relay_patch = '''        if elastos_ice_transport_policy:
            self.webrtcbin.set_property("ice-transport-policy", policy_value)
            logger.info("confirmed ICE transport policy after TURN setup: %s", elastos_ice_transport_policy)
'''
if "elastos_ice_transport_policy" not in text:
    if marker not in text:
        raise SystemExit("Selkies relay policy patch target not found")
    text = text.replace(marker, marker + relay_patch, 1)
if "confirmed ICE transport policy after TURN setup" not in text:
    if turn_marker not in text:
        raise SystemExit("Selkies TURN relay policy patch target not found")
    text = text.replace(turn_marker, turn_marker + turn_relay_patch, 1)
if "confirmed ICE transport policy after TURN setup" not in text:
    raise SystemExit("Selkies relay policy patch incomplete")
ice_log_marker = '        logger.debug("received ICE candidate: %d %s", mlineindex, candidate)\n'
ice_log_patch = '        logger.info("emitting ICE candidate: %d %s", mlineindex, candidate)\n'
if ice_log_patch not in text:
    if ice_log_marker not in text:
        raise SystemExit("Selkies ICE candidate log patch target not found")
    text = text.replace(ice_log_marker, ice_log_patch, 1)
rtp_extensions_marker = '''        rtp_id_iteration = 0
        return_result = True
'''
previous_rtp_extensions_patch = '''        # Selkies 1.6.1 RTP header extensions are unstable in the ElastOS
        # combined audio/video product session. Runtime TURN plus NACK/FIR keeps
        # the media path reliable without mutating RTP extension caps here.
        return True
'''
if previous_rtp_extensions_patch in text:
    text = text.replace(previous_rtp_extensions_patch, rtp_extensions_marker, 1)
elif rtp_extensions_marker not in text:
    raise SystemExit("Selkies RTP extension patch removal target not found")
opusenc_member_marker = "        self.rtpgccbwe = None\n"
opusenc_member_patch = "        self.rtpgccbwe = None\n        self.opusenc = None\n"
if opusenc_member_patch not in text:
    if opusenc_member_marker not in text:
        raise SystemExit("Selkies opusenc member patch target not found")
    text = text.replace(opusenc_member_marker, opusenc_member_patch, 1)
video_only_start = """        if audio_only:
            self.build_audio_pipeline()
        else:
            self.build_video_pipeline()
"""
audio_video_start = """        if audio_only:
            self.build_audio_pipeline()
        else:
            self.build_video_pipeline()
            self.build_audio_pipeline()
"""
audio_video_pattern = (
    r"        if audio_only:\n"
    r"            self\.build_audio_pipeline\(\)\n"
    r"        else:\n"
    r"            self\.build_video_pipeline\(\)\n"
    r"(?:            self\.build_audio_pipeline\(\)\n)+"
)
text, pipeline_replacements = re.subn(audio_video_pattern, video_only_start, text, count=1)
if pipeline_replacements == 0 and video_only_start not in text:
    raise SystemExit("Selkies video/audio split pipeline patch target not found")
audio_extension_block = """        # Add WebRTC RTP extensions
        extensions_return = self.rtp_add_extensions(rtpopuspay, audio=True)
        if not extensions_return:
            logger.warning("WebRTC RTP extension configuration failed with audio, this may lead to suboptimal performance")
"""
previous_audio_extension_patch = """        # Selkies 1.6.1 can corrupt combined audio/video SDP when audio RTP
        # header extensions are attached. Keep the product session audio track
        # simple and let WebRTC/NACK handle media recovery through Runtime TURN.
        extensions_return = True
"""
audio_extension_patch = """        # Selkies 1.6.1 audio RTP header extensions are fragile in the split
        # product audio peer. Keep the audio track
        # simple and let WebRTC/NACK handle media recovery through Runtime TURN.
        extensions_return = True
"""
if audio_extension_block in text:
    text = text.replace(audio_extension_block, audio_extension_patch, 1)
elif previous_audio_extension_patch in text:
    text = text.replace(previous_audio_extension_patch, audio_extension_patch, 1)
elif "Selkies 1.6.1 audio RTP header extensions are fragile" not in text:
    raise SystemExit("Selkies audio RTP extension patch target not found")
pulsesrc_named = '        pulsesrc = Gst.ElementFactory.make("pulsesrc", "pulsesrc")\n'
pulsesrc_unnamed = '        pulsesrc = Gst.ElementFactory.make("pulsesrc")\n'
pulsesrc_device = '        pulsesrc.set_property("device", "auto_null.monitor")\n'
if pulsesrc_named in text:
    text = text.replace(pulsesrc_named, pulsesrc_unnamed, 1)
elif pulsesrc_unnamed not in text:
    raise SystemExit("Selkies pulsesrc patch target not found")
text = re.sub(r'^[ \t]*pulsesrc\.set_property\("device", .*\)\n', '', text, flags=re.MULTILINE)
text = text.replace(pulsesrc_unnamed, pulsesrc_unnamed + pulsesrc_device, 1)
opusenc_named = '        opusenc = Gst.ElementFactory.make("opusenc", "opusenc")\n'
opusenc_unnamed = '        opusenc = Gst.ElementFactory.make("opusenc")\n'
if opusenc_named in text:
    text = text.replace(opusenc_named, opusenc_unnamed, 1)
elif opusenc_unnamed not in text:
    raise SystemExit("Selkies opusenc patch target not found")
opusenc_bitrate_marker = '        opusenc.set_property("bitrate", self.audio_bitrate)\n'
opusenc_bitrate_patch = '        opusenc.set_property("bitrate", self.audio_bitrate)\n        self.opusenc = opusenc\n'
if opusenc_bitrate_patch not in text:
    if opusenc_bitrate_marker not in text:
        raise SystemExit("Selkies opusenc reference patch target not found")
    text = text.replace(opusenc_bitrate_marker, opusenc_bitrate_patch, 1)
opusenc_update_block = """            element = Gst.Bin.get_by_name(self.pipeline, "opusenc")
            element.set_property("bitrate", bitrate)
"""
opusenc_update_patch = """            element = self.opusenc or Gst.Bin.get_by_name(self.pipeline, "opusenc")
            if element is None:
                raise GSTWebRTCAppError("Audio encoder is unavailable")
            element.set_property("bitrate", bitrate)
"""
if opusenc_update_block in text:
    text = text.replace(opusenc_update_block, opusenc_update_patch, 1)
elif opusenc_update_patch not in text:
    raise SystemExit("Selkies audio bitrate update patch target not found")
audio_queue_named = '        rtpopuspay_queue = Gst.ElementFactory.make("queue", "rtpopuspay_queue")\n'
audio_queue_unnamed = '        rtpopuspay_queue = Gst.ElementFactory.make("queue")\n'
if audio_queue_named in text:
    text = text.replace(audio_queue_named, audio_queue_unnamed, 1)
elif audio_queue_unnamed not in text:
    raise SystemExit("Selkies audio queue patch target not found")
audio_add_block = """        # Add all elements to the pipeline.
        pipeline_elements = [pulsesrc, pulsesrc_capsfilter, opusenc, rtpopuspay, rtpopuspay_queue, rtpopuspay_capsfilter]

        for pipeline_element in pipeline_elements:
            self.pipeline.add(pipeline_element)
"""
audio_add_strict_block = """        # Add all elements to the pipeline.
        pipeline_elements = [pulsesrc, pulsesrc_capsfilter, opusenc, rtpopuspay, rtpopuspay_queue, rtpopuspay_capsfilter]

        for pipeline_element in pipeline_elements:
            if pipeline_element is None:
                raise GSTWebRTCAppError("Audio pipeline element is unavailable")
            if not self.pipeline.add(pipeline_element):
                raise GSTWebRTCAppError("Failed to add {} to pipeline".format(pipeline_element.get_name()))
"""
audio_add_patch = """        # Add all elements to the pipeline.
        pipeline_elements = [pulsesrc, pulsesrc_capsfilter, opusenc, rtpopuspay, rtpopuspay_queue, rtpopuspay_capsfilter]

        for pipeline_element in pipeline_elements:
            if pipeline_element is None:
                raise GSTWebRTCAppError("Audio pipeline element is unavailable")
            self.pipeline.add(pipeline_element)
"""
if audio_add_strict_block in text:
    text = text.replace(audio_add_strict_block, audio_add_patch, 1)
elif audio_add_block in text:
    text = text.replace(audio_add_block, audio_add_patch, 1)
elif audio_add_patch not in text:
    raise SystemExit("Selkies audio pipeline add patch target not found")
audio_offer_marker = '        logger.info("{} pipeline started".format("audio" if audio_only else "video"))\n'
audio_offer_patch = """        logger.info("{} pipeline started".format("audio" if audio_only else "video"))
        if audio_only:
            logger.info("forcing audio SDP offer for split product audio peer")
            self.__on_negotiation_needed(self.webrtcbin)
"""
if audio_offer_patch not in text:
    if audio_offer_marker not in text:
        raise SystemExit("Selkies split audio offer patch target not found")
    text = text.replace(audio_offer_marker, audio_offer_patch, 1)
if "Failed to add {} to pipeline" in text:
    raise SystemExit("Selkies audio pipeline still contains the obsolete strict add check")
if "emitting ICE candidate" not in text:
    raise SystemExit("Selkies must log outbound ICE candidates at info level")
path.write_text(text)
PY
}

extract_browser_vm_selkies_app() {
    local rootfs="$1"
    local target="$2"
    local debugfs="$3"
    local guest_path="/usr/local/lib/python3.11/dist-packages/selkies_gstreamer/gstwebrtc_app.py"

    if ! "$debugfs" -R "cat ${guest_path}" "$rootfs" > "$target" 2>/dev/null; then
        echo "Browser VM rootfs Selkies gstwebrtc_app.py was not found: ${rootfs}" >&2
        exit 1
    fi
    patch_browser_vm_selkies_app_source "$target"
}

write_browser_vm_target_manifest() {
    local target="$1"
    cat > "$target" <<'JSON'
{
  "schema": "elastos.browser.vm-target/v1",
  "engine": "chromium_microvm",
  "network_mode": "runtime_net_only",
  "direct_network": false,
  "wallet_injection": false,
  "media_transport": "runtime_relay",
  "display_mode": "webrtc_remote_display",
  "guarantee_level": "mechanism_microvm",
  "display_backend": "vm_selkies_gstreamer_webrtc",
  "runtime_exit_transport": "vsock_relay",
  "control_transport": "vsock_relay",
  "control_port": 19092
}
JSON
}

refresh_browser_vm_native_helpers() {
    local rootfs="$1"
    local debugfs="$2"
    local target_platform="$3"
    local runtime_relay_source
    local guest_control_bridge_source

    runtime_relay_source="$(resolve_browser_vm_guest_helper_source \
        "browser-vm-runtime-relay" \
        "ELASTOS_BROWSER_VM_RUNTIME_RELAY_BIN" \
        "browser-vm-runtime-relay" \
        "browser-vm-runtime-relay" \
        "$target_platform")"
    guest_control_bridge_source="$(resolve_browser_vm_guest_helper_source \
        "browser-vm-guest-control-bridge" \
        "ELASTOS_BROWSER_VM_GUEST_CONTROL_BRIDGE_BIN" \
        "browser-vm-guest-control-bridge" \
        "browser-vm-guest-control-bridge" \
        "$target_platform")"
    validate_linux_guest_binary "browser-vm-runtime-relay" \
        "$runtime_relay_source" "$target_platform"
    validate_linux_guest_binary "browser-vm-guest-control-bridge" \
        "$guest_control_bridge_source" "$target_platform"
    refresh_browser_vm_rootfs_file "$rootfs" "$runtime_relay_source" \
        "/opt/elastos/bin/browser-vm-runtime-relay" "0100755" \
        "runtime-relay" "$debugfs"
    refresh_browser_vm_rootfs_file "$rootfs" "$guest_control_bridge_source" \
        "/opt/elastos/bin/browser-vm-guest-control-bridge" "0100755" \
        "guest-control-bridge" "$debugfs"
}

refresh_browser_vm_rootfs_files() {
    local rootfs="${ELASTOS_BROWSER_VM_ROOTFS:-${DATA_DIR}/browser-vm/rootfs.ext4}"
    local control_source="${ROOT}/scripts/browser-selkies-control-service.mjs"
    local vz_transport_bootstrap_source="${ROOT}/scripts/browser-vm-vz-transport-bootstrap.mjs"

    if [[ ! -f "$rootfs" ]]; then
        return
    fi

    local debugfs
    debugfs="$(find_debugfs || true)"
    if [[ -z "$debugfs" ]]; then
        echo "Browser VM rootfs exists but debugfs was not found; install e2fsprogs or set ELASTOS_DEBUGFS_BIN so setup-source-home can refresh VM guest files." >&2
        exit 1
    fi

    local target_platform
    local native_proxy_source=""
    target_platform="$(browser_vm_target_platform || true)"
    if [[ -z "$target_platform" ]]; then
        echo "Browser VM rootfs helper refresh is unsupported on this platform" >&2
        exit 1
    fi
    if [[ -n "$target_platform" ]]; then
        if ! native_proxy_source="$(resolve_browser_vm_native_proxy_source "$target_platform")"; then
            if [[ -n "${ELASTOS_BROWSER_NATIVE_PROXY_BIN:-}" ]]; then
                exit 1
            fi
            native_proxy_source=""
        fi
    fi

    local init_source
    local selkies_start_source
    local selkies_app_source
    local manifest_source
    init_source="$(mktemp)"
    selkies_start_source="$(mktemp)"
    selkies_app_source="$(mktemp)"
    manifest_source="$(mktemp)"
    extract_browser_vm_init "$init_source"
    extract_browser_vm_selkies_start "$selkies_start_source"
    extract_browser_vm_selkies_app "$rootfs" "$selkies_app_source" "$debugfs"
    write_browser_vm_target_manifest "$manifest_source"
    refresh_browser_vm_rootfs_file "$rootfs" "$manifest_source" \
        "/etc/elastos/browser-vm-target.json" "0100644" \
        "target-manifest" "$debugfs"
    refresh_browser_vm_rootfs_file "$rootfs" "$control_source" \
        "/opt/elastos/bin/browser-selkies-control-service.mjs" "0100644" \
        "control-service" "$debugfs"
    refresh_browser_vm_rootfs_file "$rootfs" "$vz_transport_bootstrap_source" \
        "/opt/elastos/bin/browser-vm-vz-transport-bootstrap.mjs" "0100755" \
        "vz-transport-bootstrap" "$debugfs"
    refresh_browser_vm_native_helpers "$rootfs" "$debugfs" "$target_platform"
    refresh_browser_vm_rootfs_file "$rootfs" "$init_source" \
        "/opt/elastos/bin/browser-vm-init" "0100755" \
        "vm-init" "$debugfs"
    refresh_browser_vm_rootfs_file "$rootfs" "$selkies_app_source" \
        "/usr/local/lib/python3.11/dist-packages/selkies_gstreamer/gstwebrtc_app.py" "0100644" \
        "selkies-gstwebrtc-app" "$debugfs"
    "$debugfs" -w -R "rm /usr/local/lib/python3.11/dist-packages/selkies_gstreamer/__pycache__/gstwebrtc_app.cpython-311.pyc" \
        "$rootfs" >/dev/null 2>&1 || true
    refresh_browser_vm_rootfs_file "$rootfs" "$selkies_start_source" \
        "/opt/elastos/bin/browser-vm-selkies-start" "0100755" \
        "selkies-start" "$debugfs"
    if [[ -n "$native_proxy_source" ]]; then
        validate_linux_guest_binary "browser-native-proxy-engine" \
            "$native_proxy_source" "$target_platform"
        refresh_browser_vm_rootfs_file "$rootfs" "$native_proxy_source" \
            "/opt/elastos/bin/browser-native-proxy-engine" "0100755" \
            "native-proxy" "$debugfs"
    fi
    rm -f "$init_source" "$selkies_start_source" "$selkies_app_source" "$manifest_source"
    prune_file_backups "$rootfs" "$(browser_vm_backup_retention)"
}

resolve_existing_symlink_target() {
    local path="$1"
    local target
    if [[ ! -L "$path" ]]; then
        printf '%s\n' "$path"
        return
    fi
    target="$(readlink "$path")"
    if [[ "$target" == /* ]]; then
        printf '%s\n' "$target"
    else
        printf '%s/%s\n' "$(cd "$(dirname "$path")" && pwd -P)" "$target"
    fi
}

refresh_browser_vm_initrd_path() {
    local requested_initrd="$1"
    local initrd
    local source="${ROOT}/scripts/browser-selkies-control-service.mjs"

    if [[ ! -f "$requested_initrd" ]]; then
        return
    fi
    initrd="$(resolve_existing_symlink_target "$requested_initrd")"
    if ! command -v cpio >/dev/null 2>&1; then
        echo "Browser VM initrd exists but cpio was not found; install cpio so setup-source-home can refresh browser-selkies-control-service.mjs." >&2
        exit 1
    fi
    if ! command -v gzip >/dev/null 2>&1; then
        echo "Browser VM initrd exists but gzip was not found; install gzip so setup-source-home can refresh browser-selkies-control-service.mjs." >&2
        exit 1
    fi

    local work_dir
    work_dir="$(mktemp -d)"
    gzip -dc "$initrd" | (cd "$work_dir" && cpio -id --quiet)
    mkdir -p "$work_dir/bin"

    if [[ -f "$work_dir/bin/browser-selkies-control-service.mjs" ]] &&
        cmp -s "$source" "$work_dir/bin/browser-selkies-control-service.mjs"; then
        rm -rf "$work_dir"
        return
    fi

    install -m 644 "$source" "$work_dir/bin/browser-selkies-control-service.mjs"
    local backup="${initrd}.before-selkies-control-$(date -u +%Y%m%dT%H%M%SZ)"
    clone_or_copy_file "$initrd" "$backup"
    (cd "$work_dir" && find . -print0 | cpio --null -o --format=newc 2>/dev/null | gzip -9) > "${initrd}.new"
    mv "${initrd}.new" "$initrd"

    local verify_dir
    verify_dir="$(mktemp -d)"
    gzip -dc "$initrd" | (cd "$verify_dir" && cpio -id --quiet)
    if ! cmp -s "$source" "$verify_dir/bin/browser-selkies-control-service.mjs"; then
        echo "Browser VM initrd control-service refresh did not verify; backup kept at ${backup}" >&2
        exit 1
    fi
    rm -rf "$work_dir" "$verify_dir"
    prune_file_backups "$initrd" "$(browser_vm_backup_retention)"
    if [[ "$requested_initrd" != "$initrd" ]]; then
        echo "[setup-source-home] refreshed Browser VM initrd control service: ${requested_initrd} -> ${initrd}"
    else
        echo "[setup-source-home] refreshed Browser VM initrd control service: ${initrd}"
    fi
}

refresh_browser_vm_initrd_control_service() {
    local initrds=()

    if [[ -n "${ELASTOS_BROWSER_VM_INITRD:-}" ]]; then
        initrds+=("${ELASTOS_BROWSER_VM_INITRD}")
    fi
    if [[ -n "${ELASTOS_BROWSER_VM_INITRAMFS:-}" ]]; then
        initrds+=("${ELASTOS_BROWSER_VM_INITRAMFS}")
    fi
    if [[ "${#initrds[@]}" -eq 0 ]]; then
        initrds+=("${DATA_DIR}/browser-vm/initrd" "${DATA_DIR}/bin/initrd")
    fi

    local seen=":"
    local initrd
    for initrd in "${initrds[@]}"; do
        if [[ "$seen" == *":${initrd}:"* ]]; then
            continue
        fi
        seen="${seen}${initrd}:"
        refresh_browser_vm_initrd_path "$initrd"
    done
}

install_browser_runtime_helpers() {
    echo "[setup-source-home] install Browser runtime helper scripts"
    mkdir -p "${DATA_DIR}/bin" "${DATA_DIR}/scripts"
    install -m 644 "${ROOT}/scripts/browser-selkies-control-service.mjs" \
        "${DATA_DIR}/scripts/browser-selkies-control-service.mjs"
    install -m 755 "${ROOT}/scripts/browser-source-home-config.mjs" \
        "${DATA_DIR}/scripts/browser-source-home-config.mjs"
    install -m 755 "${ROOT}/scripts/browser-runtime-turn.mjs" \
        "${DATA_DIR}/scripts/browser-runtime-turn.mjs"
    install -m 755 "${ROOT}/scripts/setup-source-home-browser-artifacts.sh" \
        "${DATA_DIR}/scripts/setup-source-home-browser-artifacts.sh"
    install -m 755 "${ROOT}/scripts/browser-vm-engine-supervisor.mjs" \
        "${DATA_DIR}/bin/browser-vm-engine-supervisor.mjs"
    install -m 755 "${ROOT}/scripts/browser-vm-control-service.mjs" \
        "${DATA_DIR}/bin/browser-vm-control-service.mjs"
    install -m 755 "${ROOT}/scripts/browser-vm-remote-vz-launcher.mjs" \
        "${DATA_DIR}/bin/browser-vm-remote-vz-launcher.mjs"
    install -m 755 "${ROOT}/scripts/browser-vm-vz-transport-bootstrap.mjs" \
        "${DATA_DIR}/scripts/browser-vm-vz-transport-bootstrap.mjs"
    install -m 755 "${ROOT}/scripts/browser-vm-local-crosvm-launcher.mjs" \
        "${DATA_DIR}/bin/browser-vm-local-crosvm-launcher.mjs"
    install -m 755 "${ROOT}/scripts/browser-vm-prepare-rootfs-pool.mjs" \
        "${DATA_DIR}/scripts/browser-vm-prepare-rootfs-pool.mjs"
    install -m 755 "${ROOT}/scripts/browser-vm-engine-preflight.sh" \
        "${DATA_DIR}/scripts/browser-vm-engine-preflight.sh"
    install -m 755 "${ROOT}/scripts/browser-vm-artifact-preflight.sh" \
        "${DATA_DIR}/scripts/browser-vm-artifact-preflight.sh"
    install -m 755 "${ROOT}/scripts/browser-vm-target-preflight.sh" \
        "${DATA_DIR}/scripts/browser-vm-target-preflight.sh"
    cat > "${DATA_DIR}/bin/browser-vm-engine-supervisor" <<EOF
#!/usr/bin/env bash
set -euo pipefail
exec "${NODE_BIN}" "${DATA_DIR}/bin/browser-vm-engine-supervisor.mjs" "\$@"
EOF
    chmod 755 "${DATA_DIR}/bin/browser-vm-engine-supervisor"
    cat > "${DATA_DIR}/bin/browser-vm-control-service" <<EOF
#!/usr/bin/env bash
set -euo pipefail
exec "${NODE_BIN}" "${DATA_DIR}/bin/browser-vm-control-service.mjs" "\$@"
EOF
    chmod 755 "${DATA_DIR}/bin/browser-vm-control-service"
    cat > "${DATA_DIR}/bin/browser-vm-remote-vz-launcher" <<EOF
#!/usr/bin/env bash
set -euo pipefail
exec "${NODE_BIN}" "${DATA_DIR}/bin/browser-vm-remote-vz-launcher.mjs" "\$@"
EOF
    chmod 755 "${DATA_DIR}/bin/browser-vm-remote-vz-launcher"
    cat > "${DATA_DIR}/bin/browser-vm-local-crosvm-launcher" <<EOF
#!/usr/bin/env bash
set -euo pipefail
exec "${NODE_BIN}" "${DATA_DIR}/bin/browser-vm-local-crosvm-launcher.mjs" "\$@"
EOF
    chmod 755 "${DATA_DIR}/bin/browser-vm-local-crosvm-launcher"
    cat > "${DATA_DIR}/bin/browser-vm-prepare-rootfs-pool" <<EOF
#!/usr/bin/env bash
set -euo pipefail
exec "${NODE_BIN}" "${DATA_DIR}/scripts/browser-vm-prepare-rootfs-pool.mjs" "\$@"
EOF
    chmod 755 "${DATA_DIR}/bin/browser-vm-prepare-rootfs-pool"

    if [[ "$(uname -s)" == "Darwin" ]]; then
        local vz_supervisor="${DATA_DIR}/bin/browser-vz-engine-supervisor"
        local vz_release_bin
        local vz_debug_bin
        vz_release_bin="$(cargo_built_binary_path "${ROOT}/elastos/Cargo.toml" release browser-vz-engine-supervisor)"
        vz_debug_bin="$(cargo_built_binary_path "${ROOT}/elastos/Cargo.toml" debug browser-vz-engine-supervisor)"
        if [[ -x "${vz_release_bin}" ]]; then
            install -m 755 "${vz_release_bin}" \
                "${vz_supervisor}"
        elif [[ -x "${vz_debug_bin}" ]]; then
            install -m 755 "${vz_debug_bin}" \
                "${vz_supervisor}"
        fi
        if [[ -x "${vz_supervisor}" ]]; then
            sign_browser_vz_supervisor "${vz_supervisor}"
        fi
    fi
    "${ROOT}/scripts/setup-source-home-browser-artifacts.sh" \
        --data-dir "${DATA_DIR}" \
        --platform "${PLATFORM}"
    refresh_browser_vm_initrd_control_service
    refresh_browser_vm_rootfs_files
}

start_browser_runtime_turn() {
    local mode="${SETUP_SOURCE_HOME_RUNTIME_TURN:-auto}"
    if [[ "$mode" == "0" ]]; then
        echo "[setup-source-home] skip Browser runtime TURN: SETUP_SOURCE_HOME_RUNTIME_TURN=0"
        return
    fi
    if [[ "$mode" != "auto" && "$mode" != "1" ]]; then
        echo "SETUP_SOURCE_HOME_RUNTIME_TURN must be auto, 1, or 0" >&2
        exit 2
    fi
    if has_remote_browser_vm_control_config; then
        echo "[setup-source-home] skip Browser runtime TURN relay: remote Browser VM control is preserved"
        return
    fi
    if [[ "$PLATFORM" == "darwin-arm64" ]]; then
        local vz_turn_program
        vz_turn_program="$(find_vz_turn_program || true)"
        if [[ -z "$vz_turn_program" ]]; then
            echo "turnserver was not found; install coturn or set ELASTOS_BROWSER_VM_TURN_PROGRAM for launch-owned VZ TURN." >&2
            exit 1
        fi
        export ELASTOS_BROWSER_VM_TURN_PROGRAM="$vz_turn_program"
        echo "[setup-source-home] configure launch-owned Browser VZ TURN"
        return
    fi
    if [[ -n "${ELASTOS_BROWSER_VM_ICE_SERVER:-}" || -n "${ELASTOS_BROWSER_VM_ICE_SERVERS_JSON:-}" ]]; then
        return
    fi
    local default_runtime_turn_env="${DATA_DIR}/runtime-turn/turn-credentials.env"
    if [[ -n "${ELASTOS_BROWSER_RUNTIME_TURN_ENV:-}" && "${ELASTOS_BROWSER_RUNTIME_TURN_ENV}" != "${default_runtime_turn_env}" ]]; then
        if [[ ! -f "${ELASTOS_BROWSER_RUNTIME_TURN_ENV}" ]]; then
            echo "ELASTOS_BROWSER_RUNTIME_TURN_ENV does not exist: ${ELASTOS_BROWSER_RUNTIME_TURN_ENV}" >&2
            exit 2
        fi
        echo "[setup-source-home] use existing Browser runtime TURN env: ${ELASTOS_BROWSER_RUNTIME_TURN_ENV}"
        return
    fi
    if [[ "$PLATFORM" != linux-* && "$PLATFORM" != "darwin-arm64" ]]; then
        return
    fi
    local turnserver_bin
    turnserver_bin="$(find_turnserver || true)"
    if [[ -z "$turnserver_bin" ]]; then
        if [[ "$mode" == "1" ]]; then
            echo "turnserver was not found; install coturn, set ELASTOS_BROWSER_VM_TURNSERVER_BIN, or set SETUP_SOURCE_HOME_RUNTIME_TURN=0 to skip Browser VM TURN setup." >&2
            exit 1
        fi
        echo "[setup-source-home] skip Browser runtime TURN relay: turnserver not found"
        return
    fi
    export ELASTOS_BROWSER_VM_TURNSERVER_BIN="$turnserver_bin"
    echo "[setup-source-home] start Browser runtime TURN relay"
    "${NODE_BIN}" "${ROOT}/scripts/browser-runtime-turn.mjs" \
        --turnserver "$turnserver_bin" \
        --data-dir "${DATA_DIR}"
    export ELASTOS_BROWSER_RUNTIME_TURN_ENV="${DATA_DIR}/runtime-turn/turn-credentials.env"
}

existing_remote_browser_vm_config() {
    local config="${DATA_DIR}/config/browser-engine-adapter.json"
    if [[ ! -f "$config" ]]; then
        return
    fi
    python3 - "$config" <<'PY'
import json
import os
import pathlib
import sys

config = pathlib.Path(sys.argv[1])
try:
    data = json.loads(config.read_text())
except Exception:
    raise SystemExit(0)

adapters = data.get("adapters")
if not isinstance(adapters, list):
    raise SystemExit(0)

adapter = next((entry for entry in adapters if entry.get("id") == "browser-vm-product"), None)
if not isinstance(adapter, dict):
    raise SystemExit(0)
supervisor = adapter.get("supervisor")
if not isinstance(supervisor, dict):
    raise SystemExit(0)
env = supervisor.get("env")
if not isinstance(env, dict):
    env = {}

launcher = str(env.get("ELASTOS_BROWSER_VM_CONTROL_LAUNCHER") or "")
if not os.path.basename(launcher).startswith("browser-vm-remote-vz-launcher"):
    raise SystemExit(0)

values = {
    "control_socket": supervisor.get("control_socket_path") or env.get("ELASTOS_BROWSER_VM_CONTROL_SOCKET"),
    "control_launcher": launcher,
    "remote_vz_data_dir": env.get("ELASTOS_BROWSER_REMOTE_VZ_DATA_DIR"),
    "status_probe_timeout_ms": env.get("ELASTOS_BROWSER_VM_CONTROL_STATUS_PROBE_TIMEOUT_MS"),
    "debug_hold_on_open_error_ms": env.get("ELASTOS_BROWSER_VM_DEBUG_HOLD_ON_OPEN_ERROR_MS"),
}
for key, value in values.items():
    if not isinstance(value, str) or not value:
        continue
    if any(ch in value for ch in "\t\r\n\0"):
        continue
    print(f"{key}\t{value}")
PY
}

has_remote_browser_vm_control_config() {
    local launcher="${ELASTOS_BROWSER_VM_CONTROL_LAUNCHER:-}"
    if [[ -n "$launcher" && "$(basename "$launcher")" == browser-vm-remote-vz-launcher* ]]; then
        return 0
    fi
    local key value
    while IFS=$'\t' read -r key value; do
        if [[ "$key" == "control_launcher" ]]; then
            return 0
        fi
    done < <(existing_remote_browser_vm_config)
    return 1
}

install_browser_source_home_config() {
    echo "[setup-source-home] write Browser source-home provider config"
    local preserved_control_socket=""
    local preserved_control_launcher=""
    local preserved_remote_vz_data_dir=""
    local preserved_status_probe_timeout_ms=""
    local preserved_debug_hold_on_open_error_ms=""
    local key value
    if [[ -z "${ELASTOS_BROWSER_VM_CONTROL_SOCKET:-}" && -z "${ELASTOS_BROWSER_VM_CONTROL_LAUNCHER:-}" ]]; then
        while IFS=$'\t' read -r key value; do
            case "$key" in
                control_socket) preserved_control_socket="$value" ;;
                control_launcher) preserved_control_launcher="$value" ;;
                remote_vz_data_dir) preserved_remote_vz_data_dir="$value" ;;
                status_probe_timeout_ms) preserved_status_probe_timeout_ms="$value" ;;
                debug_hold_on_open_error_ms) preserved_debug_hold_on_open_error_ms="$value" ;;
            esac
        done < <(existing_remote_browser_vm_config)
        if [[ -n "$preserved_control_socket" && -n "$preserved_control_launcher" ]]; then
            echo "[setup-source-home] preserve existing remote Browser VM control config"
        fi
    fi
    local control_socket="${ELASTOS_BROWSER_VM_CONTROL_SOCKET:-$preserved_control_socket}"
    local control_launcher="${ELASTOS_BROWSER_VM_CONTROL_LAUNCHER:-$preserved_control_launcher}"
    local args=(
        "${NODE_BIN}" "${ROOT}/scripts/browser-source-home-config.mjs"
        --data-dir "${DATA_DIR}"
        --platform "${PLATFORM}"
    )
    args+=(--vm-supervisor "${DATA_DIR}/bin/browser-vm-engine-supervisor")
    if [[ -n "$control_socket" ]]; then
        args+=(--vm-control-socket "$control_socket")
    fi
    if [[ -n "$control_launcher" ]]; then
        args+=(--vm-control-launcher "$control_launcher")
    fi
    if [[ -n "${ELASTOS_BROWSER_VM_ROOTFS:-}" ]]; then
        args+=(--vm-rootfs "${ELASTOS_BROWSER_VM_ROOTFS}")
    fi
    local env_args=()
    if [[ -n "$preserved_status_probe_timeout_ms" && -z "${ELASTOS_BROWSER_VM_CONTROL_STATUS_PROBE_TIMEOUT_MS:-}" ]]; then
        env_args+=("ELASTOS_BROWSER_VM_CONTROL_STATUS_PROBE_TIMEOUT_MS=$preserved_status_probe_timeout_ms")
    fi
    if [[ -n "$preserved_debug_hold_on_open_error_ms" && -z "${ELASTOS_BROWSER_VM_DEBUG_HOLD_ON_OPEN_ERROR_MS:-}" ]]; then
        env_args+=("ELASTOS_BROWSER_VM_DEBUG_HOLD_ON_OPEN_ERROR_MS=$preserved_debug_hold_on_open_error_ms")
    fi
    if [[ -n "$preserved_remote_vz_data_dir" && -z "${ELASTOS_BROWSER_REMOTE_VZ_DATA_DIR:-}" ]]; then
        env_args+=("ELASTOS_BROWSER_REMOTE_VZ_DATA_DIR=$preserved_remote_vz_data_dir")
    fi
    if [[ "${#env_args[@]}" -gt 0 ]]; then
        env "${env_args[@]}" "${args[@]}"
    else
        "${args[@]}"
    fi
}

stamp_source_home_components_manifest() {
    COMPONENTS_SRC="${ROOT}/components.json" \
    COMPONENTS_DEST="${DATA_DIR}/components.json" \
    DATA_DIR="${DATA_DIR}" \
    SETUP_PLATFORM="${PLATFORM}" \
    SOURCE_HOME_BINARY_NAMES_JSON="${SOURCE_HOME_BINARY_NAMES_JSON}" \
    APP_CAPSULES_JSON="${APP_CAPSULES_JSON}" \
    SOURCE_HOME_KUBO_INSTALLED="${SOURCE_HOME_KUBO_INSTALLED}" \
    python3 - <<'PY'
import hashlib
import json
import os
import pathlib

components_src = pathlib.Path(os.environ["COMPONENTS_SRC"])
components_dest = pathlib.Path(os.environ["COMPONENTS_DEST"])
data_dir = pathlib.Path(os.environ["DATA_DIR"])
platform = os.environ["SETUP_PLATFORM"]

manifest = json.loads(components_src.read_text())
host_components = [
    "shell",
    *json.loads(os.environ["SOURCE_HOME_BINARY_NAMES_JSON"]),
]
source_home_components = list(
    dict.fromkeys([*host_components, *json.loads(os.environ["APP_CAPSULES_JSON"])])
)
if os.environ["SOURCE_HOME_KUBO_INSTALLED"] == "1":
    kubo = data_dir / "bin" / "kubo"
    if not kubo.is_file():
        raise SystemExit("successful Kubo setup did not install bin/kubo")
    source_home_components.append("kubo")

for name in host_components:
    platforms = manifest["external"][name].setdefault("platforms", {})
    info = platforms.get(platform)
    if info is None:
        raise SystemExit(f"{name} has no {platform} platform entry in components.json")
    binary = data_dir / "bin" / name
    data = binary.read_bytes()
    info["checksum"] = "sha256:" + hashlib.sha256(data).hexdigest()
    info["size"] = len(data)
    info["install_path"] = f"bin/{name}"
    info.setdefault("release_path", f"{name}-{platform}")

manifest.setdefault("profiles", {})["source-home"] = {
    "description": "Components built and installed by setup-source-home.sh",
    "components": source_home_components,
}

components_dest.parent.mkdir(parents=True, exist_ok=True)
components_dest.write_text(json.dumps(manifest, indent=2) + "\n")
PY
}

stamp_source_home_capsule_artifacts_manifest() {
    local args=()
    local capsule
    for capsule in "${APP_CAPSULES[@]}"; do
        args+=(--capsule "$capsule")
    done
    for capsule in "${RETIRED_SOURCE_HOME_CAPSULES[@]}"; do
        args+=(--retired-capsule "$capsule")
    done
    while IFS= read -r capsule; do
        if [[ -f "${ROOT}/capsules/${capsule}/capsule.json" ]]; then
            args+=(--capsule "$capsule")
        fi
    done < <(provider_runtime_names)
    python3 "${ROOT}/scripts/stamp-source-home-capsule-metadata.py" \
        --components "${DATA_DIR}/components.json" \
        --data-dir "${DATA_DIR}" \
        --root "${ROOT}" \
        --platform "${PLATFORM}" \
        --managed-state "${DATA_DIR}/receipts/source-home-capsules.json" \
        "${args[@]}"
}

prepare_media_provider_prerequisite() {
    echo "[setup-source-home] prepare media-provider prerequisite"
    HOME="${HOME}" \
    ELASTOS_COMPONENTS_MANIFEST="${ROOT}/components.json" \
        "$(cargo_built_binary_path "${ROOT}/elastos/Cargo.toml" release elastos)" \
        setup --with media-provider --prerequisites-only
}

install_content_publish_backend() {
    local mode="${SETUP_SOURCE_HOME_INSTALL_KUBO:-auto}"

    if [[ "$mode" == "0" ]]; then
        echo "[setup-source-home] skip Kubo install: SETUP_SOURCE_HOME_INSTALL_KUBO=0"
        return
    fi
    if [[ "$mode" != "1" && "$PLATFORM" != "darwin-arm64" ]]; then
        return
    fi

    echo "[setup-source-home] install Kubo for Library/Documents publish"
    HOME="${HOME}" \
    ELASTOS_COMPONENTS_MANIFEST="${DATA_DIR}/components.json" \
        "$(cargo_built_binary_path "${ROOT}/elastos/Cargo.toml" release elastos)" setup --with kubo
    if [[ ! -f "${DATA_DIR}/bin/kubo" || ! -x "${DATA_DIR}/bin/kubo" ]]; then
        echo "Kubo setup succeeded without an installed executable: ${DATA_DIR}/bin/kubo" >&2
        exit 1
    fi
    SOURCE_HOME_KUBO_INSTALLED="1"
}

echo "[setup-source-home] repo: ${ROOT}"
echo "[setup-source-home] data dir: ${DATA_DIR}"
echo "[setup-source-home] platform: ${PLATFORM}"
echo "[setup-source-home] cargo: ${CARGO_BIN}"
echo "[setup-source-home] node: ${NODE_BIN}"
echo "[setup-source-home] cargo home: ${CARGO_HOME:-<default>}"
echo "[setup-source-home] rustup home: ${RUSTUP_HOME:-<default>}"

ensure_owner_only_data_dir
validate_collaboration_startup_mode
if [[ "${SETUP_SOURCE_HOME_CONFIG_ONLY:-0}" == "1" ]]; then
    if [[ "$(collaboration_startup_mode)" == "configured" ]]; then
        echo "configured collaboration setup requires full setup-source-home mode." >&2
        exit 1
    fi
    install_browser_source_home_config
    echo "[setup-source-home] config-only artifacts installed"
    exit 0
fi

browser_vm_backup_retention >/dev/null
require_minimum_free_space "${ROOT}"
require_minimum_free_space "${DATA_DIR}"

CONFIG_TOML="${DATA_DIR}/config.toml"
touch "${CONFIG_TOML}"
if ! grep -Eq '^[[:space:]]*dev_mode[[:space:]]*=' "${CONFIG_TOML}"; then
    printf '\ndev_mode = true\n' >> "${CONFIG_TOML}"
fi
if ! grep -Eq '^[[:space:]]*trusted_keys[[:space:]]*=' "${CONFIG_TOML}"; then
    printf 'trusted_keys = []\n' >> "${CONFIG_TOML}"
fi

echo "[setup-source-home] build runtime server"
"$CARGO_BIN" build --manifest-path "${ROOT}/elastos/Cargo.toml" --release -p elastos-server
verify_collaboration_startup_config_input
if [[ "$PLATFORM" == "darwin-arm64" ]]; then
    echo "[setup-source-home] build Browser VZ engine supervisor"
    "$CARGO_BIN" build --manifest-path "${ROOT}/elastos/Cargo.toml" --release -p elastos-vz --bin browser-vz-engine-supervisor
fi
build_browser_vm_guest_helpers

source_home_binary_manifest_path() {
    local name="$1"
    local candidate
    for candidate in \
        "${ROOT}/capsules/${name}/Cargo.toml" \
        "${ROOT}/elastos/capsules/${name}/Cargo.toml" \
        "${ROOT}/elastos/tools/${name}/Cargo.toml"
    do
        if [[ -f "$candidate" ]]; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done
    echo "source-home binary manifest not found for ${name}" >&2
    exit 1
}

echo "[setup-source-home] build native provider binaries"
"$CARGO_BIN" build --manifest-path "${ROOT}/elastos/capsules/shell/Cargo.toml" --release
source_home_binary_names | while IFS= read -r provider; do
    "$CARGO_BIN" build --manifest-path "$(source_home_binary_manifest_path "${provider}")" --release
done

echo "[setup-source-home] build Home CLI native renderer"
"$CARGO_BIN" build --manifest-path "${ROOT}/capsules/home-cli/Cargo.toml" --release --bin home-cli

echo "[setup-source-home] build app WASM capsules"
for capsule in "${APP_CAPSULES[@]}"; do
    entrypoint="$(capsule_entrypoint "${ROOT}/capsules/${capsule}/capsule.json")"
    runtime_abi="$(capsule_runtime_abi "${ROOT}/capsules/${capsule}/capsule.json")"
    if [[ "$runtime_abi" == "elastos.component/v1" ]]; then
        build_component_capsule "${ROOT}/capsules/${capsule}"
    elif is_runtime_projection_capsule "$runtime_abi"; then
        test -f "${ROOT}/capsules/${capsule}/${entrypoint}" || {
            echo "${capsule} projection entrypoint missing: ${ROOT}/capsules/${capsule}/${entrypoint}" >&2
            exit 1
        }
    elif [[ "$(is_content_data_capsule "${ROOT}/capsules/${capsule}/capsule.json")" == "yes" ]]; then
        test -f "${ROOT}/capsules/${capsule}/${entrypoint}" || {
            echo "${capsule} content entrypoint missing: ${ROOT}/capsules/${capsule}/${entrypoint}" >&2
            exit 1
        }
    else
        echo "${capsule} uses unsupported runtime_abi '${runtime_abi:-unset}'; first-party product capsules must use elastos.component/v1, elastos.runtime-projection/v1, or role=content type=data" >&2
        exit 1
    fi
done

prepare_media_provider_prerequisite

echo "[setup-source-home] install native providers and stamp manifest"
mkdir -p "${DATA_DIR}/bin"
install -m 755 "$(cargo_built_binary_path "${ROOT}/elastos/Cargo.toml" release shell)" "${DATA_DIR}/bin/shell"
install -m 755 "$(cargo_built_binary_path "${ROOT}/capsules/home-cli/Cargo.toml" release home-cli)" "${DATA_DIR}/bin/home-cli"
source_home_binary_names | while IFS= read -r provider; do
    install -m 755 "$(cargo_built_binary_path "$(source_home_binary_manifest_path "${provider}")" release "${provider}")" "${DATA_DIR}/bin/${provider}"
done
stamp_source_home_components_manifest

echo "[setup-source-home] install content publish backend before final manifest stamp"
install_content_publish_backend

echo "[setup-source-home] finalize source-home component selection"
stamp_source_home_components_manifest

echo "[setup-source-home] install app capsules and manifest entrypoints"
install_app_capsules
for retired_provider in "${RETIRED_SOURCE_HOME_PROVIDER_BINARIES[@]}"; do
    rm -f "${DATA_DIR}/bin/${retired_provider}"
done
stamp_source_home_capsule_artifacts_manifest
python3 "${ROOT}/scripts/components-release-integrity-check.py" \
    --manifest "${DATA_DIR}/components.json" \
    --platform "${PLATFORM}" \
    --profile source-home \
    --source-root "${ROOT}" \
    --source-home-data-dir "${DATA_DIR}"
install_browser_runtime_helpers
start_browser_runtime_turn
install_browser_source_home_config
install_collaboration_startup_config
# On Linux cargo hardlinks target/release/elastos to its deps/ twin, and the
# installer's artifact gate rejects any multi-link source (hardlink-swap
# defense, st_nlink must be 1). Hand it a private single-link copy staged
# next to the built binary instead of relaxing the gate.
built_runtime="$(cargo_built_binary_path "${ROOT}/elastos/Cargo.toml" release elastos)"
(
    runtime_stage_dir="$(mktemp -d "$(dirname "${built_runtime}")/install-stage.XXXXXX")"
    trap 'rm -rf "${runtime_stage_dir}"' EXIT
    install -m 0700 "${built_runtime}" "${runtime_stage_dir}/elastos"
    python3 "${ROOT}/scripts/install-source-home-runtime.py" \
        --source-root "${ROOT}" \
        --data-dir "${DATA_DIR}" \
        --built-runtime "${runtime_stage_dir}/elastos" \
        --platform "${PLATFORM}"
)

cat <<EOF
[setup-source-home] artifacts installed; offline principal-root upgrade and restart are required before readiness

Use the platform source-home restart script. It stops the Runtime, performs the
canonical principal-root upgrade, and starts Home only after the readiness gate
passes.

Direct gateway startup remains fail-closed while a configured protected root
contains declared plaintext or an incomplete migration journal.
EOF
