#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
    cat <<'EOF'
Usage:
  scripts/browser-vm-target-refresh.sh [options]

Refresh deployed Browser VM host helper scripts after verifying guest parity with a
reviewed source checkout without running a full Rust/WASM source-home setup.

Options:
  --source-dir PATH       Source checkout. Default: this repository.
  --data-dir PATH         ElastOS data dir. Default: platform data dir.
  --initrd PATH           Initrd to verify. May be passed more than once.
                          Default: existing browser-vm/initrd and bin/initrd.
  --rootfs PATH           Rootfs ext4 to verify. Default: browser-vm/rootfs.ext4.
  --guest-control-bridge-bin PATH
                          Optional prebuilt Linux guest-control bridge binary to
                          compare with the rootfs.
  --backup-dir PATH       Backup directory. Default: data-dir/backups/browser-vm-target-refresh-<timestamp>-<pid>.
  --node-bin PATH         Node executable for refreshed wrapper scripts. Default: auto-detect.
  --verify-only           Report whether target files match; do not write.
  --help, -h              Show this help.

The script preserves initrd/rootfs bytes and their build receipt. Guest changes
require a rebuilt image set. It creates backups before changed installed host helpers
and retains one default backup set. ELASTOS_BROWSER_VM_BACKUP_RETENTION must
be 1. Finish target closeout by running:

  scripts/jetson-browser-runtime-audit.mjs \
    --host <target-host> \
    --user <target-user> \
    --data-dir <target-elastos-data-dir> \
    --source-dir <target-source-checkout> \
    --require-parity
EOF
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

find_node() {
    if [[ -n "${ELASTOS_NODE_BIN:-}" && -x "${ELASTOS_NODE_BIN}" ]]; then
        printf '%s\n' "${ELASTOS_NODE_BIN}"
        return
    fi
    if command -v node >/dev/null 2>&1; then
        command -v node
        return
    fi
    for candidate in /opt/homebrew/bin/node /usr/local/bin/node; do
        if [[ -x "$candidate" ]]; then
            printf '%s\n' "$candidate"
            return
        fi
    done
    echo "node not found. Install Node or pass --node-bin." >&2
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

backup_file() {
    local path="$1"
    local label="$2"
    local backup="$BACKUP_DIR/${label}.before"
    mkdir -p "$BACKUP_DIR"
    cp -c "$path" "$backup" 2>/dev/null ||
        cp --reflink=auto --sparse=always -p "$path" "$backup" 2>/dev/null ||
        cp -p "$path" "$backup"
    touch "$backup"
    printf '%s\n' "$backup"
}

backup_retention() {
    local retention="${ELASTOS_BROWSER_VM_BACKUP_RETENTION:-1}"
    if [[ "$retention" != "1" ]]; then
        echo "ELASTOS_BROWSER_VM_BACKUP_RETENTION must be 1" >&2
        exit 2
    fi
    printf '%s\n' "$retention"
}

prune_default_backup_dirs() {
    local keep
    local candidate
    local remove_index
    local index
    local LC_ALL=C
    local backups=()

    keep="$(backup_retention)"
    for candidate in "$DATA_DIR"/backups/browser-vm-target-refresh-*; do
        if [[ -d "$candidate" && ! -L "$candidate" ]]; then
            backups+=("$candidate")
        fi
    done
    while (( ${#backups[@]} > keep )); do
        remove_index=-1
        for (( index = 0; index < ${#backups[@]}; index += 1 )); do
            if [[ "${backups[$index]}" != "$BACKUP_DIR" ]]; then
                remove_index="$index"
                break
            fi
        done
        if (( remove_index < 0 )); then
            break
        fi
        candidate="${backups[$remove_index]}"
        rm -rf -- "$candidate"
        echo "[browser-vm-target-refresh] pruned stale backup: ${candidate}"
        unset "backups[$remove_index]"
        backups=("${backups[@]}")
    done
}

install_with_backup() {
    local source="$1"
    local target="$2"
    local mode="$3"
    local label="$4"

    if [[ -f "$target" ]] && cmp -s "$source" "$target"; then
        echo "[browser-vm-target-refresh] unchanged installed helper: $target"
        return
    fi
    if [[ "$VERIFY_ONLY" == "1" ]]; then
        echo "[browser-vm-target-refresh] drift installed helper: $target"
        DRIFT=1
        return
    fi

    mkdir -p "$(dirname "$target")"
    if [[ -e "$target" || -L "$target" ]]; then
        backup_file "$target" "$label" >/dev/null
    fi
    install -m "$mode" "$source" "$target"
    if ! cmp -s "$source" "$target"; then
        echo "installed helper refresh did not verify: $target" >&2
        exit 1
    fi
    echo "[browser-vm-target-refresh] refreshed installed helper: $target"
}

write_node_wrapper() {
    local target="$1"
    local script="$2"
    local label="$3"
    local tmp
    tmp="$(mktemp)"
    cat > "$tmp" <<EOF
#!/usr/bin/env bash
set -euo pipefail
exec "${NODE_BIN}" "${script}" "\$@"
EOF
    install_with_backup "$tmp" "$target" 755 "$label"
    rm -f "$tmp"
}

initrd_helper_matches() {
    local initrd="$1"
    local source="$2"
    local tmp_dir
    tmp_dir="$(mktemp -d)"
    gzip -dc "$initrd" | (cd "$tmp_dir" && cpio -id --quiet bin/browser-selkies-control-service.mjs)
    cmp -s "$source" "$tmp_dir/bin/browser-selkies-control-service.mjs"
    local status=$?
    rm -rf "$tmp_dir"
    return "$status"
}

check_initrd() {
    local requested_initrd="$1"
    local source="$2"
    local initrd

    if [[ ! -f "$requested_initrd" ]]; then
        return
    fi
    command -v gzip >/dev/null 2>&1 || { echo "gzip not found" >&2; exit 1; }
    command -v cpio >/dev/null 2>&1 || { echo "cpio not found" >&2; exit 1; }

    initrd="$(resolve_existing_symlink_target "$requested_initrd")"
    if initrd_helper_matches "$initrd" "$source"; then
        echo "[browser-vm-target-refresh] unchanged initrd helper: $requested_initrd"
        return
    fi
    echo "[browser-vm-target-refresh] guest initrd requires a rebuilt image set: $requested_initrd"
    DRIFT=1
}

rootfs_helper_matches() {
    local rootfs="$1"
    local source="$2"
    local guest_path="$3"
    local debugfs="$4"
    local tmp
    tmp="$(mktemp)"
    "$debugfs" -R "cat ${guest_path}" "$rootfs" > "$tmp" 2>/dev/null
    cmp -s "$source" "$tmp"
    local status=$?
    rm -f "$tmp"
    return "$status"
}

rootfs_helper_contains() {
    local rootfs="$1"
    local guest_path="$2"
    local snippet="$3"
    local debugfs="$4"
    local tmp
    tmp="$(mktemp)"
    "$debugfs" -R "cat ${guest_path}" "$rootfs" > "$tmp" 2>/dev/null || {
        rm -f "$tmp"
        return 1
    }
    LC_ALL=C grep -aFq "$snippet" "$tmp"
    local status=$?
    rm -f "$tmp"
    return "$status"
}

verify_rootfs_guest_control_bridge_contract() {
    local rootfs="$1"
    local debugfs="$2"
    local guest_path="/opt/elastos/bin/browser-vm-guest-control-bridge"
    local missing=0
    for snippet in \
        "elastos.browser.vm-guest-control-bridge.config/v1" \
        "control_socket_ready_timeout_ms" \
        "control_request_timeout_ms"
    do
        if ! rootfs_helper_contains "$rootfs" "$guest_path" "$snippet" "$debugfs"; then
            echo "[browser-vm-target-refresh] stale rootfs helper: $guest_path missing $snippet" >&2
            DRIFT=1
            missing=1
        fi
    done
    if [[ "$missing" == "1" && "$VERIFY_ONLY" != "1" ]]; then
        echo "rootfs guest-control bridge is stale; rebuild the image set with the current Linux guest bridge binary" >&2
        exit 1
    fi
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

extract_browser_vm_selkies_start() {
    local source_dir="$1"
    local target="$2"
    python3 - "$source_dir/scripts/build/stage-browser-vm-target.sh" "$target" <<'PY'
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
    local source_dir="$1"
    local target="$2"
    python3 - "$source_dir/scripts/build/stage-browser-vm-target.sh" "$target" <<'PY'
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

check_rootfs_file() {
    local rootfs="$1"
    local source="$2"
    local guest_path="$3"
    local debugfs="$4"

    if rootfs_helper_matches "$rootfs" "$source" "$guest_path" "$debugfs"; then
        echo "[browser-vm-target-refresh] unchanged rootfs helper: $guest_path"
        return
    fi
    echo "[browser-vm-target-refresh] guest file requires a rebuilt image set: $guest_path"
    DRIFT=1
}

check_rootfs_guest_files() {
    local requested_rootfs="$1"
    local source="$2"
    local rootfs
    local debugfs
    local init_source
    local selkies_start_source
    local manifest_source

    if [[ ! -f "$requested_rootfs" ]]; then
        return
    fi
    debugfs="$(find_debugfs || true)"
    if [[ -z "$debugfs" ]]; then
        echo "debugfs not found. Install e2fsprogs or set ELASTOS_DEBUGFS_BIN." >&2
        exit 1
    fi

    rootfs="$(resolve_existing_symlink_target "$requested_rootfs")"
    init_source="$(mktemp)"
    selkies_start_source="$(mktemp)"
    manifest_source="$(mktemp)"
    extract_browser_vm_init "$SOURCE_DIR" "$init_source"
    extract_browser_vm_selkies_start "$SOURCE_DIR" "$selkies_start_source"
    write_browser_vm_target_manifest "$manifest_source"

    check_rootfs_file "$rootfs" "$manifest_source" \
        "/etc/elastos/browser-vm-target.json" "$debugfs"
    check_rootfs_file "$rootfs" "$source" \
        "/opt/elastos/bin/browser-selkies-control-service.mjs" "$debugfs"
    check_rootfs_file "$rootfs" "$init_source" \
        "/opt/elastos/bin/browser-vm-init" "$debugfs"
    check_rootfs_file "$rootfs" "$selkies_start_source" \
        "/opt/elastos/bin/browser-vm-selkies-start" "$debugfs"
    if [[ -n "$GUEST_CONTROL_BRIDGE_BIN" ]]; then
        check_rootfs_file "$rootfs" "$GUEST_CONTROL_BRIDGE_BIN" \
            "/opt/elastos/bin/browser-vm-guest-control-bridge" "$debugfs"
    fi
    verify_rootfs_guest_control_bridge_contract "$rootfs" "$debugfs"
    rm -f "$init_source" "$selkies_start_source" "$manifest_source"
}

SOURCE_DIR="$ROOT"
DATA_DIR="$(default_data_dir)"
BACKUP_DIR=""
DEFAULT_BACKUP_DIR=0
NODE_BIN=""
VERIFY_ONLY=0
DRIFT=0
INITRDS=()
ROOTFS=""
ROOTFS_BACKUP_PATH=""
GUEST_CONTROL_BRIDGE_BIN="${ELASTOS_BROWSER_VM_GUEST_CONTROL_BRIDGE_BIN:-}"

while [[ "$#" -gt 0 ]]; do
    case "$1" in
        --source-dir)
            [[ "$#" -ge 2 ]] || { echo "--source-dir requires PATH" >&2; exit 2; }
            SOURCE_DIR="$2"
            shift 2
            ;;
        --data-dir)
            [[ "$#" -ge 2 ]] || { echo "--data-dir requires PATH" >&2; exit 2; }
            DATA_DIR="$2"
            shift 2
            ;;
        --initrd)
            [[ "$#" -ge 2 ]] || { echo "--initrd requires PATH" >&2; exit 2; }
            INITRDS+=("$2")
            shift 2
            ;;
        --rootfs)
            [[ "$#" -ge 2 ]] || { echo "--rootfs requires PATH" >&2; exit 2; }
            ROOTFS="$2"
            shift 2
            ;;
        --guest-control-bridge-bin)
            [[ "$#" -ge 2 ]] || { echo "--guest-control-bridge-bin requires PATH" >&2; exit 2; }
            GUEST_CONTROL_BRIDGE_BIN="$2"
            shift 2
            ;;
        --backup-dir)
            [[ "$#" -ge 2 ]] || { echo "--backup-dir requires PATH" >&2; exit 2; }
            BACKUP_DIR="$2"
            shift 2
            ;;
        --node-bin)
            [[ "$#" -ge 2 ]] || { echo "--node-bin requires PATH" >&2; exit 2; }
            NODE_BIN="$2"
            shift 2
            ;;
        --verify-only)
            VERIFY_ONLY=1
            shift
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            echo "unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

SOURCE_DIR="$(cd "$SOURCE_DIR" && pwd -P)"
backup_retention >/dev/null
if [[ -z "$BACKUP_DIR" ]]; then
    BACKUP_DIR="$DATA_DIR/backups/browser-vm-target-refresh-$(date -u +%Y%m%dT%H%M%SZ)-$$"
    DEFAULT_BACKUP_DIR=1
fi
if [[ -z "$NODE_BIN" ]]; then
    NODE_BIN="$(find_node)"
fi
if [[ ! -x "$NODE_BIN" ]]; then
    echo "node executable is not executable: $NODE_BIN" >&2
    exit 1
fi
if [[ -n "$GUEST_CONTROL_BRIDGE_BIN" ]]; then
    if [[ ! -f "$GUEST_CONTROL_BRIDGE_BIN" ]]; then
        echo "guest control bridge binary is missing: $GUEST_CONTROL_BRIDGE_BIN" >&2
        exit 1
    fi
    GUEST_CONTROL_BRIDGE_BIN="$(cd "$(dirname "$GUEST_CONTROL_BRIDGE_BIN")" && pwd -P)/$(basename "$GUEST_CONTROL_BRIDGE_BIN")"
fi
if [[ -z "$ROOTFS" ]]; then
    ROOTFS="$DATA_DIR/browser-vm/rootfs.ext4"
fi
if [[ "${#INITRDS[@]}" -eq 0 ]]; then
    [[ -f "$DATA_DIR/browser-vm/initrd" ]] && INITRDS+=("$DATA_DIR/browser-vm/initrd")
    [[ -f "$DATA_DIR/bin/initrd" ]] && INITRDS+=("$DATA_DIR/bin/initrd")
fi

selkies_source="$SOURCE_DIR/scripts/browser-selkies-control-service.mjs"
for required in \
    "$SOURCE_DIR/scripts/browser-selkies-control-service.mjs" \
    "$SOURCE_DIR/scripts/browser-source-home-config.mjs" \
    "$SOURCE_DIR/scripts/browser-runtime-turn.mjs" \
    "$SOURCE_DIR/scripts/browser-vm-prepare-rootfs-pool.mjs" \
    "$SOURCE_DIR/scripts/browser-vm-engine-preflight.sh" \
    "$SOURCE_DIR/scripts/browser-vm-artifact-preflight.sh" \
    "$SOURCE_DIR/scripts/browser-vm-target-preflight.sh" \
    "$SOURCE_DIR/scripts/browser-vm-engine-supervisor.mjs" \
    "$SOURCE_DIR/scripts/browser-vm-control-service.mjs" \
    "$SOURCE_DIR/scripts/browser-vm-remote-vz-launcher.mjs" \
    "$SOURCE_DIR/scripts/browser-vm-local-crosvm-launcher.mjs"
do
    if [[ ! -f "$required" ]]; then
        echo "required source helper missing: $required" >&2
        exit 1
    fi
done

echo "[browser-vm-target-refresh] source: $SOURCE_DIR"
echo "[browser-vm-target-refresh] data: $DATA_DIR"
echo "[browser-vm-target-refresh] backup: $BACKUP_DIR"
echo "[browser-vm-target-refresh] mode: $([[ "$VERIFY_ONLY" == "1" ]] && echo verify-only || echo write)"

if [[ -f "$ROOTFS" ]]; then
    ELASTOS_BROWSER_VM_DATA_DIR="$DATA_DIR" \
    ELASTOS_BROWSER_VM_ROOTFS="$ROOTFS" \
        "$SOURCE_DIR/scripts/browser-vm-artifact-preflight.sh" --verify-image-set
fi

if [[ "${#INITRDS[@]}" -gt 0 ]]; then
    for initrd in "${INITRDS[@]}"; do
        check_initrd "$initrd" "$selkies_source"
    done
fi
check_rootfs_guest_files "$ROOTFS" "$selkies_source"

if [[ "$DRIFT" == "1" ]]; then
    echo "Browser guest files differ from source. Rebuild and install the complete image set before refreshing host helpers." >&2
    exit 1
fi

install_with_backup "$selkies_source" \
    "$DATA_DIR/scripts/browser-selkies-control-service.mjs" 644 \
    "browser-selkies-control-service.mjs"
install_with_backup "$SOURCE_DIR/scripts/browser-source-home-config.mjs" \
    "$DATA_DIR/scripts/browser-source-home-config.mjs" 755 \
    "browser-source-home-config.mjs"
install_with_backup "$SOURCE_DIR/scripts/browser-runtime-turn.mjs" \
    "$DATA_DIR/scripts/browser-runtime-turn.mjs" 755 \
    "browser-runtime-turn.mjs"
install_with_backup "$SOURCE_DIR/scripts/browser-vm-prepare-rootfs-pool.mjs" \
    "$DATA_DIR/scripts/browser-vm-prepare-rootfs-pool.mjs" 755 \
    "browser-vm-prepare-rootfs-pool.mjs"
install_with_backup "$SOURCE_DIR/scripts/browser-vm-engine-preflight.sh" \
    "$DATA_DIR/scripts/browser-vm-engine-preflight.sh" 755 \
    "browser-vm-engine-preflight.sh"
install_with_backup "$SOURCE_DIR/scripts/browser-vm-artifact-preflight.sh" \
    "$DATA_DIR/scripts/browser-vm-artifact-preflight.sh" 755 \
    "browser-vm-artifact-preflight.sh"
install_with_backup "$SOURCE_DIR/scripts/browser-vm-target-preflight.sh" \
    "$DATA_DIR/scripts/browser-vm-target-preflight.sh" 755 \
    "browser-vm-target-preflight.sh"
install_with_backup "$SOURCE_DIR/scripts/browser-vm-engine-supervisor.mjs" \
    "$DATA_DIR/bin/browser-vm-engine-supervisor.mjs" 755 \
    "browser-vm-engine-supervisor.mjs"
install_with_backup "$SOURCE_DIR/scripts/browser-vm-control-service.mjs" \
    "$DATA_DIR/bin/browser-vm-control-service.mjs" 755 \
    "browser-vm-control-service.mjs"
install_with_backup "$SOURCE_DIR/scripts/browser-vm-remote-vz-launcher.mjs" \
    "$DATA_DIR/bin/browser-vm-remote-vz-launcher.mjs" 755 \
    "browser-vm-remote-vz-launcher.mjs"
install_with_backup "$SOURCE_DIR/scripts/browser-vm-local-crosvm-launcher.mjs" \
    "$DATA_DIR/bin/browser-vm-local-crosvm-launcher.mjs" 755 \
    "browser-vm-local-crosvm-launcher.mjs"

write_node_wrapper "$DATA_DIR/bin/browser-vm-engine-supervisor" \
    "$DATA_DIR/bin/browser-vm-engine-supervisor.mjs" \
    "browser-vm-engine-supervisor"
write_node_wrapper "$DATA_DIR/bin/browser-vm-control-service" \
    "$DATA_DIR/bin/browser-vm-control-service.mjs" \
    "browser-vm-control-service"
write_node_wrapper "$DATA_DIR/bin/browser-vm-remote-vz-launcher" \
    "$DATA_DIR/bin/browser-vm-remote-vz-launcher.mjs" \
    "browser-vm-remote-vz-launcher"
write_node_wrapper "$DATA_DIR/bin/browser-vm-local-crosvm-launcher" \
    "$DATA_DIR/bin/browser-vm-local-crosvm-launcher.mjs" \
    "browser-vm-local-crosvm-launcher"
write_node_wrapper "$DATA_DIR/bin/browser-vm-prepare-rootfs-pool" \
    "$DATA_DIR/scripts/browser-vm-prepare-rootfs-pool.mjs" \
    "browser-vm-prepare-rootfs-pool"

if [[ "$VERIFY_ONLY" == "1" && "$DRIFT" == "1" ]]; then
    echo "[browser-vm-target-refresh] verify-only found drift"
    exit 1
fi

if [[ "$VERIFY_ONLY" != "1" && "$DEFAULT_BACKUP_DIR" == "1" ]]; then
    prune_default_backup_dirs
fi

echo "[browser-vm-target-refresh] done"
