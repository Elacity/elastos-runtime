#!/bin/bash
# Build a minimal rootfs image for a capsule.
#
# Usage: ./scripts/build/build-rootfs.sh <capsule-name> [--output <dir>]
#
# Produces: <output>/<capsule-name>.capsule.tar.gz
#   containing: capsule.json, rootfs.ext4, checksums.sha256
#
# Requires: mke2fs (e2fsprogs), cargo, busybox-static

set -euo pipefail

die() { echo "Error: $*" >&2; exit 1; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

CAPSULE_NAME="${1:?Usage: build-rootfs.sh <capsule-name> [--target <rust-target>] [--output <dir>] [--skip-compile]}"
OUTPUT_DIR="${PROJECT_ROOT}/artifacts"
CROSS_TARGET=""
SKIP_COMPILE=false

# Parse optional args
shift
while [[ $# -gt 0 ]]; do
    case "$1" in
        --output)
            mkdir -p "$2"
            OUTPUT_DIR="$(cd "$2" && pwd)"
            shift 2
            ;;
        --target)
            CROSS_TARGET="$2"
            shift 2
            ;;
        --skip-compile)
            SKIP_COMPILE=true
            shift
            ;;
        *)
            die "Unknown option: $1"
            ;;
    esac
done

# ── Locate capsule source ───────────────────────────────────────────
CAPSULE_DIR=""
CAPSULE_JSON=""
for candidate in \
    "${PROJECT_ROOT}/elastos/capsules/${CAPSULE_NAME}" \
    "${PROJECT_ROOT}/capsules/${CAPSULE_NAME}"; do
    if [ -f "${candidate}/capsule.json" ]; then
        CAPSULE_DIR="$candidate"
        CAPSULE_JSON="${candidate}/capsule.json"
        break
    fi
done

if [ -z "$CAPSULE_DIR" ]; then
    die "capsule '${CAPSULE_NAME}' not found (searched: elastos/capsules/, capsules/)"
fi

echo "=== Building rootfs for '${CAPSULE_NAME}' ==="
echo "  Source: ${CAPSULE_DIR}"
echo "  Output: ${OUTPUT_DIR}"

# ── Detect target architecture ───────────────────────────────────────
HOST_ARCH="$(uname -m)"
CROSS_COMPILING=false

if [ -n "$CROSS_TARGET" ]; then
    RUST_TARGET="$CROSS_TARGET"
    case "$CROSS_TARGET" in
        aarch64-*)
            PLATFORM_KEY="linux-arm64"
            TARGET_ARCH="aarch64"
            ;;
        x86_64-*)
            PLATFORM_KEY="linux-amd64"
            TARGET_ARCH="x86_64"
            ;;
        *) die "unsupported cross target: $CROSS_TARGET" ;;
    esac
    if [ "$TARGET_ARCH" != "$HOST_ARCH" ]; then
        CROSS_COMPILING=true
    fi
else
    case "$HOST_ARCH" in
        x86_64)
            RUST_TARGET="x86_64-unknown-linux-musl"
            PLATFORM_KEY="linux-amd64"
            TARGET_ARCH="x86_64"
            ;;
        aarch64)
            RUST_TARGET="aarch64-unknown-linux-musl"
            PLATFORM_KEY="linux-arm64"
            TARGET_ARCH="aarch64"
            ;;
        *)  die "unsupported architecture: $HOST_ARCH" ;;
    esac
fi

# Cross-compilation asset cache (populated by publish-release.sh --cross).
CROSS_CACHE_DIR="${HOME}/.local/share/elastos/cross/${TARGET_ARCH}"

# ── Resolve cargo target-dir from workspace config ────────────────
CARGO_TARGET_DIR=""
if [[ -f "${PROJECT_ROOT}/elastos/.cargo/config.toml" ]]; then
    CARGO_TARGET_DIR=$(grep -E '^\s*target-dir\s*=' "${PROJECT_ROOT}/elastos/.cargo/config.toml" 2>/dev/null \
        | head -1 | sed 's/.*=\s*"\(.*\)"/\1/' | sed "s|.*=\s*'\(.*\)'|\1|" | tr -d ' ' || true)
fi

COMPONENTS_JSON="${PROJECT_ROOT}/components.json"
[ -f "${COMPONENTS_JSON}" ] || die "components.json not found at ${COMPONENTS_JSON}"

# Source of external dependencies on host. Prefer the invoking user's data dir
# when running via sudo so we reuse artifacts from `elastos setup`.
HOST_HOME="${HOME}"
if [ -n "${SUDO_USER:-}" ] && [ "${SUDO_USER}" != "root" ]; then
    HOST_HOME="$(getent passwd "${SUDO_USER}" | cut -d: -f6)"
fi
HOST_DATA_DIR="${ELASTOS_HOST_DATA_DIR:-${HOST_HOME}/.local/share/elastos}"

# ── Build the capsule binary (static-linked via musl — required for VM rootfs) ─
if [ "$SKIP_COMPILE" = true ]; then
    echo "  Skipping compile (--skip-compile), expecting pre-built binary..."
else
    echo "  Building ${CAPSULE_NAME} (${RUST_TARGET})..."
    if [ -f "${CAPSULE_DIR}/Cargo.toml" ]; then
        if [ "$CROSS_COMPILING" = true ] && command -v cross >/dev/null 2>&1; then
            echo "  Using 'cross' for cross-compilation..."
            (cd "${CAPSULE_DIR}" && cross build --release --target "${RUST_TARGET}" 2>&1) || {
                die "cross build failed for ${CAPSULE_NAME}"
            }
        else
            (cd "${CAPSULE_DIR}" && cargo build --release --target "${RUST_TARGET}" 2>&1) || {
                if [ "$CROSS_COMPILING" = true ]; then
                    die "cross-compilation failed for ${CAPSULE_NAME}. Either:\n  1. Install 'cross': cargo install cross (uses Docker)\n  2. Install linker: apt install gcc-aarch64-linux-gnu\n     Then: export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=aarch64-linux-gnu-gcc"
                else
                    die "musl build failed for ${CAPSULE_NAME}. Install musl target: rustup target add ${RUST_TARGET}"
                fi
            }
        fi
    fi
fi

# ── Determine binary path ────────────────────────────────────────────
BINARY_PATH=""
BINARY_NAMES=("${CAPSULE_NAME}")
CARGO_MANIFEST="${CAPSULE_DIR}/Cargo.toml"
if [ -f "${CARGO_MANIFEST}" ]; then
    TOML_BIN_NAMES="$(
        python3 - <<'PY' "${CARGO_MANIFEST}" 2>/dev/null || true
import sys
import re

manifest = sys.argv[1]
names = []
try:
    import tomllib  # py3.11+
    with open(manifest, "rb") as f:
        data = tomllib.load(f)
    for b in data.get("bin", []):
        name = b.get("name")
        if isinstance(name, str) and name:
            names.append(name)
except Exception:
    # Fallback parser for Python versions without tomllib.
    in_bin = False
    try:
        with open(manifest, "r", encoding="utf-8") as f:
            for raw in f:
                line = raw.strip()
                if line.startswith("[[") and line.endswith("]]"):
                    in_bin = (line == "[[bin]]")
                    continue
                if in_bin:
                    m = re.match(r'^name\s*=\s*"([^"]+)"', line)
                    if m:
                        names.append(m.group(1))
    except Exception:
        pass

print(" ".join(names))
PY
    )"
    if [ -n "${TOML_BIN_NAMES}" ]; then
        # Prefer explicit Cargo [[bin]] names first.
        BINARY_NAMES=()
        for n in ${TOML_BIN_NAMES}; do
            BINARY_NAMES+=("${n}")
        done
        BINARY_NAMES+=("${CAPSULE_NAME}")
    fi
fi

for bin_name in "${BINARY_NAMES[@]}"; do
    # Prefer target-specific artifacts first (musl static), then non-target fallbacks.
    # Build candidate list, prepending custom cargo target-dir if configured.
    BINARY_CANDIDATES=()
    if [[ -n "$CARGO_TARGET_DIR" ]]; then
        BINARY_CANDIDATES+=("${CARGO_TARGET_DIR}/${RUST_TARGET}/release/${bin_name}")
        BINARY_CANDIDATES+=("${CARGO_TARGET_DIR}/release/${bin_name}")
    fi
    BINARY_CANDIDATES+=(
        "${CAPSULE_DIR}/target/${RUST_TARGET}/release/${bin_name}"
        "${PROJECT_ROOT}/elastos/target/${RUST_TARGET}/release/${bin_name}"
        "${PROJECT_ROOT}/target/${RUST_TARGET}/release/${bin_name}"
        "${CAPSULE_DIR}/target/release/${bin_name}"
        "${PROJECT_ROOT}/elastos/target/release/${bin_name}"
        "${PROJECT_ROOT}/target/release/${bin_name}"
    )
    for candidate in "${BINARY_CANDIDATES[@]}"; do
        if [ -f "$candidate" ]; then
            BINARY_PATH="$candidate"
            break 2
        fi
    done
done

if [ -z "${BINARY_PATH}" ]; then
    die "binary for '${CAPSULE_NAME}' not found after build (candidates: ${BINARY_NAMES[*]})"
fi

# VM rootfs has no shared libc. Enforce static binary to avoid immediate boot-time exits.
if command -v file >/dev/null 2>&1; then
    if file "${BINARY_PATH}" | grep -q "dynamically linked"; then
        die "selected binary is dynamically linked (${BINARY_PATH}); expected static musl artifact for ${RUST_TARGET}"
    fi
fi

echo "  Binary: ${BINARY_PATH}"

command -v mke2fs >/dev/null 2>&1 \
    || die "mke2fs not found. Install e2fsprogs (e.g. apt install e2fsprogs)"

# ── Resolve external requirements to materialize inside guest ────────
REQUIRES="$(
    CAPSULE_JSON_PATH="${CAPSULE_JSON}" python3 - <<'PY'
import json
import os
import sys

path = os.environ["CAPSULE_JSON_PATH"]
with open(path, "r", encoding="utf-8") as f:
    data = json.load(f)

if "dependencies" in data:
    print("ERROR: legacy 'dependencies' field is not supported; use typed 'requires'")
    sys.exit(2)

requires = data.get("requires", [])
if not isinstance(requires, list):
    print("ERROR: 'requires' must be a list")
    sys.exit(2)

pairs = []
for item in requires:
    if not isinstance(item, dict):
        print("ERROR: each require entry must be an object")
        sys.exit(2)
    name = item.get("name")
    kind = item.get("kind")
    if not isinstance(name, str) or not name:
        print("ERROR: require.name must be a non-empty string")
        sys.exit(2)
    if kind not in ("capsule", "external"):
        print(f"ERROR: require.kind for '{name}' must be 'capsule' or 'external'")
        sys.exit(2)
    pairs.append(f"{kind}:{name}")

print(" ".join(pairs))
PY
)"
PROVIDES="$(python3 -c "import json; print(json.load(open('${CAPSULE_JSON}')).get('provides',''))" 2>/dev/null || echo "")"

DEPENDENCY_ENTRIES=()
DEPENDENCY_BYTES=0

for req in ${REQUIRES}; do
    REQ_KIND="${req%%:*}"
    dep="${req#*:}"

    if [ "${REQ_KIND}" = "capsule" ]; then
        echo "  Skipping capsule requirement '${dep}' (resolved by supervisor at runtime)"
        continue
    fi
    if [ "${REQ_KIND}" != "external" ]; then
        die "invalid requirement kind '${REQ_KIND}' for '${dep}'"
    fi

    DEP_META="$(
        DEP_NAME="${dep}" DEP_PLATFORM="${PLATFORM_KEY}" COMPONENTS_JSON="${COMPONENTS_JSON}" python3 - <<'PY'
import json
import os
name = os.environ["DEP_NAME"]
platform = os.environ["DEP_PLATFORM"]
path = os.environ["COMPONENTS_JSON"]
with open(path, "r", encoding="utf-8") as f:
    data = json.load(f)

external = data.get("external", {})
capsules = data.get("capsules", {})

if name in external:
    ext = external.get(name, {})
    plat = ext.get("platforms", {}).get(platform) or ext.get("platforms", {}).get("*") or {}
    install = plat.get("install_path") or ext.get("install_path") or ""
    print(f"external|{install}")
elif name in capsules:
    print("capsule|")
else:
    print("unknown|")
PY
    )"

    DEP_KIND="${DEP_META%%|*}"
    DEP_INSTALL_PATH="${DEP_META#*|}"

    case "${DEP_KIND}" in
        capsule)
            die "requirement '${dep}' declared as external but found as capsule in components.json"
            ;;
        unknown)
            die "external requirement '${dep}' is unknown (not found in components.json external)"
            ;;
        external)
            if [ -z "${DEP_INSTALL_PATH}" ]; then
                die "external requirement '${dep}' has no install_path in components.json for platform ${PLATFORM_KEY}"
            fi
            ;;
        *)
            die "external requirement '${dep}' resolved to invalid kind '${DEP_KIND}'"
            ;;
    esac

    if [ "$CROSS_COMPILING" = true ]; then
        SRC_PATH="${CROSS_CACHE_DIR}/${DEP_INSTALL_PATH}"
    else
        SRC_PATH="${HOST_DATA_DIR}/${DEP_INSTALL_PATH}"
    fi
    if [ ! -e "${SRC_PATH}" ]; then
        if [ "$CROSS_COMPILING" = true ]; then
            die "external requirement '${dep}' for ${TARGET_ARCH} missing at ${SRC_PATH}.\n  Run: ./scripts/publish-release.sh --cross ${TARGET_ARCH}"
        else
            die "external requirement '${dep}' missing at ${SRC_PATH} (run: elastos setup --with ${dep})"
        fi
    fi

    DEP_BYTES="$(du -sb "${SRC_PATH}" | cut -f1)"
    DEPENDENCY_BYTES=$((DEPENDENCY_BYTES + DEP_BYTES))
    DEPENDENCY_ENTRIES+=("${dep}|${DEP_INSTALL_PATH}|${SRC_PATH}")
done

# ── Create and populate rootfs (rootless via mke2fs -d) ──────────────
WORK_DIR="$(mktemp -d)"
trap "rm -rf ${WORK_DIR}" EXIT

# Size rootfs based on dependency footprint (plus fixed 256MB slack).
BASE_BYTES=$((64 * 1024 * 1024))
SLACK_BYTES=$((256 * 1024 * 1024))
ROOTFS_BYTES=$((BASE_BYTES + DEPENDENCY_BYTES + SLACK_BYTES))
ROOTFS_SIZE_MB=$(((ROOTFS_BYTES + 1024 * 1024 - 1) / (1024 * 1024)))
ROOTFS="${WORK_DIR}/rootfs.ext4"
STAGING_DIR="${WORK_DIR}/staging"

echo "  Creating rootfs (${ROOTFS_SIZE_MB} MB)..."
mkdir -p "${STAGING_DIR}"

# Create minimal filesystem
mkdir -p "${STAGING_DIR}"/{bin,sbin,usr/bin,usr/sbin,lib,etc,tmp,var,proc,sys,dev}
mkdir -p "${STAGING_DIR}/opt/elastos"

# ── Install busybox (provides /bin/sh, mount, cat, ip for init) ──
BUSYBOX_PATH=""

if [ "$CROSS_COMPILING" = true ]; then
    # Cross mode: busybox must be for the TARGET architecture.
    if [ -x "${CROSS_CACHE_DIR}/bin/busybox" ]; then
        BUSYBOX_PATH="${CROSS_CACHE_DIR}/bin/busybox"
    fi
    if [ -z "$BUSYBOX_PATH" ]; then
            die "busybox-static for ${TARGET_ARCH} not found at ${CROSS_CACHE_DIR}/bin/busybox.\n  Run: ./scripts/publish-release.sh --cross ${TARGET_ARCH}\n  Or manually place a static ${TARGET_ARCH} busybox binary there."
    fi
else
    for candidate in /bin/busybox /usr/bin/busybox; do
        if [ -x "$candidate" ] && "$candidate" --list >/dev/null 2>&1; then
            BUSYBOX_PATH="$candidate"
            break
        fi
    done

    # Also check if host has busybox-static installed
    if [ -z "$BUSYBOX_PATH" ] && command -v busybox >/dev/null 2>&1; then
        BUSYBOX_PATH="$(command -v busybox)"
    fi

    if [ -z "$BUSYBOX_PATH" ]; then
        die "busybox not found on host. Install: apt install busybox-static"
    fi

    # Verify busybox has the applets we need. Avoid a pipefail+grep -q
    # false negative here: grep may exit early after a match, which can
    # make the producer report SIGPIPE and intermittently fail the check.
    BUSYBOX_APPLETS="$("$BUSYBOX_PATH" --list 2>/dev/null || true)"
    REQUIRED_BUSYBOX_APPLETS=(sh mount mkdir cat echo ip stty sed sleep ls)
    MISSING_BUSYBOX_APPLETS=()
    for applet in "${REQUIRED_BUSYBOX_APPLETS[@]}"; do
        if ! grep -qx "$applet" <<<"$BUSYBOX_APPLETS"; then
            MISSING_BUSYBOX_APPLETS+=("$applet")
        fi
    done
    if [ "${#MISSING_BUSYBOX_APPLETS[@]}" -ne 0 ]; then
        die "busybox at $BUSYBOX_PATH is missing required applets: ${MISSING_BUSYBOX_APPLETS[*]}"
    fi

    # Verify busybox is statically linked
    if command -v file >/dev/null 2>&1; then
        if file "$BUSYBOX_PATH" | grep -q "dynamically linked"; then
            die "busybox at $BUSYBOX_PATH is dynamically linked — rootfs has no shared libs. Install: apt install busybox-static"
        fi
    fi
fi

cp "$BUSYBOX_PATH" "${STAGING_DIR}/bin/busybox"
chmod 755 "${STAGING_DIR}/bin/busybox"

# Create symlinks for applets used by init
for applet in sh mount mkdir cat echo ip stty sed sleep ls; do
    ln -sf busybox "${STAGING_DIR}/bin/${applet}"
done
echo "  Installed busybox from $BUSYBOX_PATH"

# Copy the capsule binary
cp "${BINARY_PATH}" "${STAGING_DIR}/usr/bin/${CAPSULE_NAME}"
chmod 755 "${STAGING_DIR}/usr/bin/${CAPSULE_NAME}"
echo "  Copied binary: ${BINARY_PATH}"

# Bundle a host CA trust store so native guest tools can validate HTTPS
# endpoints (for example cloudflared against api.trycloudflare.com).
HOST_CA_BUNDLE=""
for candidate in \
    /etc/ssl/certs/ca-certificates.crt \
    /etc/pki/tls/certs/ca-bundle.crt \
    /etc/ssl/cert.pem; do
    if [ -f "${candidate}" ]; then
        HOST_CA_BUNDLE="${candidate}"
        break
    fi
done

if [ -z "${HOST_CA_BUNDLE}" ]; then
    die "host CA bundle not found. Install ca-certificates on the host"
fi

mkdir -p "${STAGING_DIR}/etc/ssl/certs"
cp "${HOST_CA_BUNDLE}" "${STAGING_DIR}/etc/ssl/certs/ca-certificates.crt"
ln -sf /etc/ssl/certs/ca-certificates.crt "${STAGING_DIR}/etc/ssl/cert.pem"
echo "  Installed CA bundle from ${HOST_CA_BUNDLE}"

# Materialize declared external dependencies into a canonical guest path.
# Provider capsules read ELASTOS_DATA_DIR=/opt/elastos inside the guest.
for entry in "${DEPENDENCY_ENTRIES[@]}"; do
    IFS='|' read -r dep dep_install src_path <<< "${entry}"
    dest_path="${STAGING_DIR}/opt/elastos/${dep_install}"
    mkdir -p "$(dirname "${dest_path}")"

    if [ -d "${src_path}" ]; then
        cp -a "${src_path}" "${dest_path}"
    else
        cp "${src_path}" "${dest_path}"
    fi

    # Executable dependencies become directly invokable from /usr/bin.
    if [[ "${dep_install}" == bin/* ]] && [ -f "${dest_path}" ]; then
        chmod 755 "${dest_path}" || true
        ln -sf "/opt/elastos/${dep_install}" "${STAGING_DIR}/usr/bin/$(basename "${dep_install}")"
    fi

    # Dynamically-linked external binaries (e.g. kubo/Go) need their shared
    # libraries bundled since the minimal rootfs has no libc.
    if [ "$CROSS_COMPILING" = true ]; then
        # Cross mode: host ldd gives wrong libraries. Use pre-populated libs
        # from the cross cache if available.
        if [ -f "${dest_path}" ] && file "${dest_path}" 2>/dev/null | grep -q "dynamically linked"; then
            if [ -d "${CROSS_CACHE_DIR}/lib" ]; then
                echo "  Bundling cross-compiled libraries for '${dep}'..."
                # Copy all libs from cross cache — these are target-arch libraries.
                cp -a "${CROSS_CACHE_DIR}/lib/." "${STAGING_DIR}/lib/" 2>/dev/null || true
                # Also handle lib64 if present.
                if [ -d "${CROSS_CACHE_DIR}/lib64" ]; then
                    mkdir -p "${STAGING_DIR}/lib64"
                    cp -a "${CROSS_CACHE_DIR}/lib64/." "${STAGING_DIR}/lib64/" 2>/dev/null || true
                fi
            else
                echo "  Warning: '${dep}' is dynamically linked but no cross libs at ${CROSS_CACHE_DIR}/lib"
            fi
        fi
    elif [ -f "${dest_path}" ] && command -v ldd >/dev/null 2>&1; then
        if file "${dest_path}" 2>/dev/null | grep -q "dynamically linked"; then
            echo "  Bundling shared libraries for '${dep}'..."
            ldd "${dest_path}" 2>/dev/null | while read -r line; do
                # Parse ldd output: "libfoo.so.1 => /lib/x86_64-linux-gnu/libfoo.so.1 (0x...)"
                lib_path=$(echo "$line" | grep -oP '=> \K/[^\s]+' || true)
                if [ -n "${lib_path}" ] && [ -f "${lib_path}" ]; then
                    lib_dir=$(dirname "${lib_path}")
                    mkdir -p "${STAGING_DIR}${lib_dir}"
                    cp -n "${lib_path}" "${STAGING_DIR}${lib_path}" 2>/dev/null || true
                fi
                # Also handle the dynamic linker line: "/lib64/ld-linux-x86-64.so.2 (0x...)"
                interp=$(echo "$line" | grep -oP '^\s*\K/[^\s]+' || true)
                if [ -n "${interp}" ] && [ -f "${interp}" ] && [[ "${interp}" == */ld-* ]]; then
                    interp_dir=$(dirname "${interp}")
                    mkdir -p "${STAGING_DIR}${interp_dir}"
                    cp -n "${interp}" "${STAGING_DIR}${interp}" 2>/dev/null || true
                fi
            done
        fi
    fi

    echo "  Bundled dependency '${dep}' -> /opt/elastos/${dep_install}"
done

# Bundle the guest-side bridge helper in capsules that need the common
# provider/API bridge. The binary name is still `vsock-proxy`, but the active
# Current path uses it for explicit guest-network compatibility, not as a
# correctness-critical host-vsock bridge.
VSOCK_PROXY_DIR="${PROJECT_ROOT}/elastos/tools/vsock-proxy"
if [ ! -f "${VSOCK_PROXY_DIR}/Cargo.toml" ]; then
    die "vsock-proxy tool source not found at ${VSOCK_PROXY_DIR}"
fi

if [ "$SKIP_COMPILE" = true ]; then
    echo "  Skipping vsock-proxy compile (--skip-compile)..."
else
    echo "  Building Carrier guest bridge helper (${RUST_TARGET})..."
    if [ "$CROSS_COMPILING" = true ] && command -v cross >/dev/null 2>&1; then
        (cd "${VSOCK_PROXY_DIR}" && cross build --release --target "${RUST_TARGET}" 2>&1) || {
            die "cross build failed for vsock-proxy (${RUST_TARGET})"
        }
    else
        (cd "${VSOCK_PROXY_DIR}" && cargo build --release --target "${RUST_TARGET}" 2>&1) || {
            die "musl build failed for vsock-proxy (${RUST_TARGET})"
        }
    fi
fi

VSOCK_PROXY_BIN=""
VSOCK_CANDIDATES=()
if [[ -n "$CARGO_TARGET_DIR" ]]; then
    VSOCK_CANDIDATES+=("${CARGO_TARGET_DIR}/${RUST_TARGET}/release/vsock-proxy")
    VSOCK_CANDIDATES+=("${CARGO_TARGET_DIR}/release/vsock-proxy")
fi
VSOCK_CANDIDATES+=(
    "${VSOCK_PROXY_DIR}/target/${RUST_TARGET}/release/vsock-proxy"
    "${VSOCK_PROXY_DIR}/target/release/vsock-proxy"
)
for candidate in "${VSOCK_CANDIDATES[@]}"; do
    if [ -f "${candidate}" ]; then
        VSOCK_PROXY_BIN="${candidate}"
        break
    fi
done
if [ -z "${VSOCK_PROXY_BIN}" ]; then
    die "Carrier guest bridge helper binary not found after build"
fi

cp "${VSOCK_PROXY_BIN}" "${STAGING_DIR}/usr/bin/vsock-proxy"
chmod 755 "${STAGING_DIR}/usr/bin/vsock-proxy"
echo "  Bundled Carrier guest bridge helper"

# Create init script — ElastOS Capsule Runtime v1
# CAPSULE_NAME is expanded at build time,
# shell vars use \$ to stay literal in the generated script.
cat > "${STAGING_DIR}/init" << INIT_EOF
#!/bin/sh
# ElastOS Capsule Runtime v1 — common init for all capsules.
# Transport and guest networking are Carrier implementation details. Capsules
# should rely on provider contracts, not on raw interfaces or host topology.
mount -t proc proc /proc 2>/dev/null || true
mount -t sysfs sys /sys 2>/dev/null || true
mount -t devtmpfs dev /dev 2>/dev/null || true

# Raise kernel thread/pid limits for capsule binaries (tokio, ureq, etc.)
# Writing to /proc/sys is more reliable than ulimit in busybox.
echo 4096 > /proc/sys/kernel/threads-max 2>/dev/null || true
echo 8192 > /proc/sys/kernel/pid_max 2>/dev/null || true
echo 1000000 > /proc/sys/vm/max_map_count 2>/dev/null || true
ulimit -u 4096 2>/dev/null || true
ulimit -n 4096 2>/dev/null || true

# Bring up loopback early. Any additional guest networking is configured by
# Carrier during capsule launch and should not be treated as an app contract.
if [ -x /bin/ip ]; then
    /bin/ip link set lo up 2>/dev/null || true
fi

# Read kernel params → env vars
ELASTOS_DATA_DIR="/opt/elastos"
TERM_COLS=""
TERM_ROWS=""
HOST_TERM=""
ELASTOS_GUEST_IP=""
ELASTOS_HOST_IP=""
ELASTOS_PREFIX_LEN=""
ELASTOS_NET_IFACE=""
ELASTOS_DNS=""
for param in \$(cat /proc/cmdline); do
    case "\$param" in
        elastos.token=*)   export ELASTOS_TOKEN="\${param#elastos.token=}" ;;
        elastos.api=*)     export ELASTOS_API="\${param#elastos.api=}" ;;
        elastos.command=*) export ELASTOS_COMMAND_B64="\${param#elastos.command=}" ;;
        elastos.data_dir=*) export ELASTOS_DATA_DIR="\${param#elastos.data_dir=}" ;;
        elastos.provider_port=*) export ELASTOS_PROVIDER_PORT="\${param#elastos.provider_port=}" ;;
        elastos.vsock_api_port=*) export ELASTOS_VSOCK_API_PORT="\${param#elastos.vsock_api_port=}" ;;
        elastos.carrier_serial=*) export ELASTOS_CARRIER_SERIAL="\${param#elastos.carrier_serial=}" ;;
        elastos.carrier_path=*) export ELASTOS_CARRIER_PATH="\${param#elastos.carrier_path=}" ;;
        elastos.capsule_args=*) ELASTOS_CAPSULE_ARGS_B64="\${param#elastos.capsule_args=}" ;;
        elastos.guest_ip=*) ELASTOS_GUEST_IP="\${param#elastos.guest_ip=}" ;;
        elastos.host_ip=*) ELASTOS_HOST_IP="\${param#elastos.host_ip=}" ;;
        elastos.prefix_len=*) ELASTOS_PREFIX_LEN="\${param#elastos.prefix_len=}" ;;
        elastos.net_iface=*) ELASTOS_NET_IFACE="\${param#elastos.net_iface=}" ;;
        elastos.dns=*) ELASTOS_DNS="\${param#elastos.dns=}" ;;
        elastos.term_cols=*) TERM_COLS="\${param#elastos.term_cols=}" ;;
        elastos.term_rows=*) TERM_ROWS="\${param#elastos.term_rows=}" ;;
        elastos.term=*) HOST_TERM="\${param#elastos.term=}" ;;
    esac
done
export ELASTOS_DATA_DIR
export PATH="\${ELASTOS_DATA_DIR}/bin:/usr/bin:/bin"
export SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
export SSL_CERT_DIR=/etc/ssl/certs
export CURL_CA_BUNDLE=/etc/ssl/certs/ca-certificates.crt

# Optional guest-network TAP — configure eth0 only when boot args request it.
# No iptables, no internet — guest-network capsules can only reach the host runtime.
if [ -x /bin/ip ] && [ -n "\${ELASTOS_GUEST_IP}" ] && [ -n "\${ELASTOS_HOST_IP}" ] && [ -n "\${ELASTOS_PREFIX_LEN}" ]; then
    IFACE="\${ELASTOS_NET_IFACE:-eth0}"
    /bin/ip link set "\${IFACE}" up 2>/dev/null || true
    /bin/ip addr add "\${ELASTOS_GUEST_IP}/\${ELASTOS_PREFIX_LEN}" dev "\${IFACE}" 2>/dev/null || true
    /bin/ip route add default via "\${ELASTOS_HOST_IP}" dev "\${IFACE}" 2>/dev/null || true
fi

# Provider capsules in guest-network mode expose JSON protocol over TCP.
if [ -n "\${ELASTOS_PROVIDER_PORT:-}" ] && [ -x /usr/bin/vsock-proxy ]; then
    exec /usr/bin/vsock-proxy provider-tcp "\${ELASTOS_PROVIDER_PORT}" /usr/bin/${CAPSULE_NAME}
fi

# Non-provider capsules: run directly with inherited stdio.
# crosvm serial connects stdin/stdout to the host terminal.
# Use host TERM from boot args (passed by supervisor); fall back to linux.
export TERM="\${HOST_TERM:-linux}"
export LINES="\${TERM_ROWS:-24}"
export COLUMNS="\${TERM_COLS:-80}"

# Make userspace logs deterministic across hosts. Some crosvm/guest console
# combinations surface kernel logs reliably but do not preserve PID 1 stderr
# the way we expect. Bind stdout/stderr explicitly to the console before
# launching the capsule so interactive failures are visible.
if [ -c /dev/console ]; then
    exec >/dev/console 2>&1
fi

# Configure the serial console for TUI use:
# 1. raw -echo: disable kernel line-discipline processing and echo so
#    crossterm receives raw bytes and controls all output itself.
#    Without this, the kernel echoes keystrokes AND crossterm echoes them
#    → duplicated/mangled input ("first letter keeps changing").
# 2. rows/cols: TIOCSWINSZ — serial ports default to (0,0).
# crosvm uses 16550 UART (ttyS0) on all architectures.
if [ -c /dev/ttyS0 ]; then
    stty raw -echo rows "\$LINES" cols "\$COLUMNS" -F /dev/ttyS0 2>/dev/null || true
elif [ -c /dev/ttyAMA0 ]; then
    stty raw -echo rows "\$LINES" cols "\$COLUMNS" -F /dev/ttyAMA0 2>/dev/null || true
fi

# Carrier bridge serial port: raw mode, no echo, no line discipline.
# Without this, the kernel echoes SDK JSON back to the bridge and corrupts
# the second+ request (first works because echo hasn't arrived yet).
if [ -n "\$ELASTOS_CARRIER_PATH" ] && [ -c "\$ELASTOS_CARRIER_PATH" ]; then
    stty raw -echo -F "\$ELASTOS_CARRIER_PATH" 2>/dev/null || true
fi

echo "init: capsule=${CAPSULE_NAME} api=\${ELASTOS_API:-unset} provider_port=\${ELASTOS_PROVIDER_PORT:-unset}" >&2

# Decode capsule arguments from base64-encoded boot arg (newline-separated).
CAPSULE_ARGV=""
if [ -n "\${ELASTOS_CAPSULE_ARGS_B64:-}" ]; then
    CAPSULE_ARGV=\$(echo "\$ELASTOS_CAPSULE_ARGS_B64" | base64 -d 2>/dev/null || true)
fi

# Launch the capsule binary. If args were provided, convert newline-separated
# decoded string into positional parameters via set --.
if [ -n "\$CAPSULE_ARGV" ]; then
    echo "init: launching /usr/bin/${CAPSULE_NAME} with args" >&2
    # Split decoded newline-separated args into positional parameters.
    # set -f disables pathname expansion so metacharacters (*, ?, [)
    # pass through as literal strings.
    _old_ifs="\$IFS"
    set -f
    IFS='
'
    set -- \$CAPSULE_ARGV
    IFS="\$_old_ifs"
    set +f
    /usr/bin/${CAPSULE_NAME} "\$@"
else
    echo "init: launching /usr/bin/${CAPSULE_NAME}" >&2
    /usr/bin/${CAPSULE_NAME}
fi
status=\$?
echo "init: /usr/bin/${CAPSULE_NAME} exited with status=\$status" >&2
exit \$status
INIT_EOF
chmod 755 "${STAGING_DIR}/init"

# Verify the rootfs is populated
if [ ! -f "${STAGING_DIR}/bin/busybox" ]; then
    die "rootfs population failed — busybox not present in rootfs"
fi
if [ ! -f "${STAGING_DIR}/usr/bin/${CAPSULE_NAME}" ]; then
    die "rootfs population failed — binary not present in rootfs"
fi
if [ ! -f "${STAGING_DIR}/init" ]; then
    die "rootfs population failed — init script not present in rootfs"
fi

# Build rootfs image from staging tree (no loop mount / root required)
mke2fs -q -t ext4 -d "${STAGING_DIR}" -F "${ROOTFS}" "${ROOTFS_SIZE_MB}M" \
    || die "mke2fs failed while creating rootfs image"
echo "  Rootfs image created and verified"

# ── Package the artifact ─────────────────────────────────────────────
mkdir -p "${OUTPUT_DIR}"
ARTIFACT_DIR="${WORK_DIR}/artifact"
mkdir -p "${ARTIFACT_DIR}"

cp "${CAPSULE_JSON}" "${ARTIFACT_DIR}/capsule.json"
cp "${ROOTFS}" "${ARTIFACT_DIR}/rootfs.ext4"

# Carrier-plane capsules also need the raw binary for host execution.
IS_CARRIER="$(python3 -c "import json; d=json.load(open('${CAPSULE_JSON}')); print('true' if d.get('permissions',{}).get('carrier') else '')" 2>/dev/null || echo "")"
ARTIFACT_FILES="capsule.json rootfs.ext4"
if [ -n "${IS_CARRIER}" ] && [ -f "${BINARY_PATH}" ]; then
    cp "${BINARY_PATH}" "${ARTIFACT_DIR}/${CAPSULE_NAME}"
    chmod +x "${ARTIFACT_DIR}/${CAPSULE_NAME}"
    ARTIFACT_FILES="${ARTIFACT_FILES} ${CAPSULE_NAME}"
    echo "  Bundled native binary for carrier-plane capsule"
fi

# Generate checksums
(cd "${ARTIFACT_DIR}" && sha256sum ${ARTIFACT_FILES} > checksums.sha256)

# Create tarball
ARTIFACT="${OUTPUT_DIR}/${CAPSULE_NAME}.capsule.tar.gz"
(cd "${ARTIFACT_DIR}" && tar czf "${ARTIFACT}" ${ARTIFACT_FILES} checksums.sha256)

echo "  Artifact: ${ARTIFACT}"
echo "  Size: $(du -h "${ARTIFACT}" | cut -f1)"
echo "=== Done ==="
