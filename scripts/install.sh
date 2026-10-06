#!/usr/bin/env bash
#
# ElastOS Installer — signed web bootstrap, then Carrier-backed updates/setup
#
# Usage:
#   curl -fsSL https://<publisher-origin>/install.sh | bash
#
#   ELASTOS_HEAD_CID=QmXyz ELASTOS_MAINTAINER_DID=did:key:z6Mk... \
#     curl -fsSL https://<explicit-gateway>/ipfs/<installer-cid>/install.sh | bash
#
#   ./scripts/install.sh --head-cid QmXyz...
#   ./scripts/install.sh --head-cid QmXyz... --maintainer-did did:key:z6Mk...
#   ./scripts/install.sh --help
#
# Required (one of):
#   ELASTOS_HEAD_CID env var   or   --head-cid <CID>
#   ELASTOS_MAINTAINER_DID env var   or   --maintainer-did <did:key:...>
#
# Trust anchors can be provided via env vars or CLI flags. In the canonical
# bootstrap flow, they should already be stamped into install.sh.
#
# Downloads the signed release head, release envelope, and Runtime binary.
# Runtime setup fetches component metadata over Carrier.
#
# After bootstrap, setup installs the Home profile and opens browser Home.
# Use --install-only for automated provisioning or other profiles.
#
# Trust model:
#   1. Bootstrap over the stamped publisher URL (or explicit operator/debug CID gateway)
#   2. Verify Ed25519 signature against pinned MAINTAINER_DID
#   3. Follow latest_release_cid to release.json
#   4. Verify release signature
#   5. Download the Runtime binary and verify SHA-256
#   6. Runtime's installation writer admits and installs the Runtime, the verified
#      envelopes and the stamped Carrier source in one journaled transaction
#   7. Runtime setup fetches signed metadata and installs Home over Carrier
#
# Fails closed if trust anchors or signature verification fail.
#
# Dependencies: curl, python3 (stdlib only), sha256sum|shasum
#

if [[ "${BASH_SOURCE[0]:-$0}" == "$0" ]]; then
set -euo pipefail

# ── Trust anchors (baked in by publish-release.sh) ────────────────────
# These placeholders are replaced with real values at publish time.
# Override via env vars or CLI flags if needed.

MAINTAINER_DID="${ELASTOS_MAINTAINER_DID:-__MAINTAINER_DID__}"
MAINTAINER_DID_PLACEHOLDER="__MAINTAINER""_DID__"
if [[ "$MAINTAINER_DID" == "$MAINTAINER_DID_PLACEHOLDER" ]]; then
    MAINTAINER_DID=""
fi
HEAD_CID="${ELASTOS_HEAD_CID:-__HEAD_CID__}"
HEAD_CID_PLACEHOLDER="__HEAD""_CID__"
if [[ "$HEAD_CID" == "$HEAD_CID_PLACEHOLDER" ]]; then
    HEAD_CID=""
fi
SOURCE_CONNECT_TICKET="${ELASTOS_SOURCE_CONNECT_TICKET:-__SOURCE_CONNECT_TICKET__}"
SOURCE_CONNECT_TICKET_PLACEHOLDER="__SOURCE""_CONNECT_TICKET__"
if [[ "$SOURCE_CONNECT_TICKET" == "$SOURCE_CONNECT_TICKET_PLACEHOLDER" ]]; then
    SOURCE_CONNECT_TICKET=""
fi
SOURCE_CONNECT_TICKET_EXPLICIT=false
if [[ -n "${ELASTOS_SOURCE_CONNECT_TICKET:-}" && -n "$SOURCE_CONNECT_TICKET" ]]; then
    SOURCE_CONNECT_TICKET_EXPLICIT=true
fi
PUBLISHER_GATEWAY="${ELASTOS_PUBLISHER_GATEWAY:-__PUBLISHER_GATEWAY__}"
PUBLISHER_GATEWAY_PLACEHOLDER="__PUBLISHER""_GATEWAY__"
if [[ "$PUBLISHER_GATEWAY" == "$PUBLISHER_GATEWAY_PLACEHOLDER" ]]; then
    PUBLISHER_GATEWAY=""
fi
PUBLISHER_NODE_ID="${ELASTOS_PUBLISHER_NODE_ID:-__PUBLISHER_NODE_ID__}"
PUBLISHER_NODE_ID_PLACEHOLDER="__PUBLISHER""_NODE_ID__"
if [[ "$PUBLISHER_NODE_ID" == "$PUBLISHER_NODE_ID_PLACEHOLDER" ]]; then
    PUBLISHER_NODE_ID=""
fi
PUBLISHER_NODE_ID_EXPLICIT=false
if [[ -n "${ELASTOS_PUBLISHER_NODE_ID:-}" && -n "$PUBLISHER_NODE_ID" ]]; then
    PUBLISHER_NODE_ID_EXPLICIT=true
fi
IPNS_NAME="${ELASTOS_IPNS_NAME:-__IPNS_NAME__}"
IPNS_NAME_PLACEHOLDER="__IPNS""_NAME__"
if [[ "$IPNS_NAME" == "$IPNS_NAME_PLACEHOLDER" ]]; then
    IPNS_NAME=""
fi

# Explicit IPFS gateways for operator/debug bootstrap only.
# Override with:
#   1) --gateway <url> (repeatable, takes highest priority)
#   2) ELASTOS_IPFS_GATEWAYS="https://a,https://b"
GATEWAYS=()
CLI_GATEWAYS=()
LAST_SUCCESS_GATEWAY=""
ALLOWED_CHANNELS=("stable" "canary" "jetson-test")
BINARY_DOWNLOAD_MAX_TIME="${ELASTOS_BINARY_DOWNLOAD_MAX_TIME:-1800}"
BINARY_DOWNLOAD_RETRY_COUNT="${ELASTOS_BINARY_DOWNLOAD_RETRY_COUNT:-10}"
BINARY_DOWNLOAD_RETRY_DELAY="${ELASTOS_BINARY_DOWNLOAD_RETRY_DELAY:-2}"
BINARY_DOWNLOAD_CONNECT_TIMEOUT="${ELASTOS_BINARY_DOWNLOAD_CONNECT_TIMEOUT:-15}"
BINARY_DOWNLOAD_SPEED_LIMIT="${ELASTOS_BINARY_DOWNLOAD_SPEED_LIMIT:-1024}"
BINARY_DOWNLOAD_SPEED_TIME="${ELASTOS_BINARY_DOWNLOAD_SPEED_TIME:-60}"

# ── Colors ────────────────────────────────────────────────────────────

BOLD='\033[1m'
DIM='\033[2m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
RED='\033[0;31m'
ACCENT='\033[38;5;208m'
NC='\033[0m'

# ── Help ──────────────────────────────────────────────────────────────

show_help() {
    echo ""
    echo -e "${BOLD}ElastOS Installer${NC}"
    echo "  Release lookup: Linux x86_64/aarch64 and macOS Apple silicon."
    echo "  The signed release determines which platforms have downloads."
    echo ""
    echo -e "${BOLD}Usage:${NC}"
    echo "  curl -fsSL https://<publisher-origin>/install.sh | bash"
    echo "  curl -fsSL https://<explicit-gateway>/ipfs/<installer-cid>/install.sh | bash   # operator/debug only"
    echo "  ./scripts/install.sh [options]"
    echo ""
    echo -e "${BOLD}Options:${NC}"
    echo "  --head-cid CID       Override bootstrap head CID"
    echo "  --maintainer-did DID Override maintainer DID trust anchor"
    echo "  --gateway URL        IPFS gateway base URL (repeatable, operator/debug bootstrap)"
    echo "  --publisher-gateway URL  Bootstrap publisher URL (stamped for normal installs)"
    echo "  --publisher-node-id ID   Publisher P2P node ID (for durable Carrier link)"
    echo "  --install-dir PATH    Binary install directory (default: ~/.local/bin)"
    echo "  --install-only        Install Runtime without setup or opening Home"
    echo "  --help                Show this help"
    echo ""
    echo -e "${BOLD}What gets installed:${NC}"
    echo "  ~/.local/bin/elastos                     Runtime binary"
    echo "  Verified release metadata and the trusted Carrier source in the Runtime data directory"
    echo ""
    echo -e "${BOLD}After installation:${NC}"
    echo "  Runtime setup fetches signed metadata and Home components over Carrier."
    echo "  Setup then opens Home in your browser."
    echo "  Keep the terminal open while using Home; Ctrl+C stops it."
    echo "  Without an interactive terminal, the installer prints the launch command."
    echo ""
    echo -e "${BOLD}Trust model:${NC}"
    echo "  All artifacts signed with Ed25519. install.sh is the explicit"
    echo "  web bootstrap. After install, first-party setup/update use the"
    echo "  trusted source over Carrier by default."
    echo "  Fails closed if signatures can't be verified."
    echo ""
    echo -e "${BOLD}Release channels:${NC}"
    echo "  stable, canary, jetson-test"
    echo ""
    exit 0
}
fi

# ── Helpers ───────────────────────────────────────────────────────────

die()  { echo -e "${RED:-}Error:${NC:-} $*" >&2; exit 1; }

# Colour, the banner and the step layout are for a person at a terminal.
# Pipes, CI, NO_COLOR and dumb terminals get plain [OK] and [WARN] lines.
installer_rich_output() {
    [[ -t 1 && -z "${NO_COLOR:-}" && -z "${CI:-}" && "${TERM:-dumb}" != dumb ]]
}

installer_select_output() {
    if installer_rich_output; then
        INSTALLER_RICH=true
    else
        INSTALLER_RICH=false
        BOLD='' DIM='' GREEN='' YELLOW='' RED='' NC='' ACCENT=''
    fi
}

INSTALLER_STEPS=5

show_banner() {
    if [[ "${INSTALLER_RICH:-false}" != true ]]; then
        echo "ElastOS Installer"
        return 0
    fi
    local line
    echo ""
    # shellcheck disable=SC1003  # the art's trailing backslashes are literal
    for line in \
        '    _____ _           _    ___  ____' \
        '   | ____| | __ _ ___| |_ / _ \/ ___|' \
        '   |  _| | |/ _` / __| __| | | \___ \' \
        '   | |___| | (_| \__ \ |_| |_| |___) |' \
        '   |_____|_|\__,_|___/\__|\___/|____/'; do
        printf '%b%s%b\n' "${ACCENT:-}" "$line" "${NC:-}"
    done
    echo ""
    echo -e "   ${BOLD:-}ElastOS Installer${NC:-}  ${DIM:-}signed release · verified before anything changes${NC:-}"
}

step() {
    if [[ "${INSTALLER_RICH:-false}" == true ]]; then
        echo ""
        echo -e "  ${ACCENT:-}[$1/${INSTALLER_STEPS}]${NC:-} ${BOLD:-}$2${NC:-}"
    else
        echo "[$1/${INSTALLER_STEPS}] $2"
    fi
}

info() {
    if [[ "${INSTALLER_RICH:-false}" == true ]]; then
        echo -e "        ${DIM:-}$*${NC:-}"
    else
        echo -e "  ... $*"
    fi
}

ok() {
    if [[ "${INSTALLER_RICH:-false}" == true ]]; then
        echo -e "        ${GREEN:-}✓${NC:-} $*"
    else
        echo -e "  [OK] $*"
    fi
}

warn() {
    if [[ "${INSTALLER_RICH:-false}" == true ]]; then
        echo -e "        ${YELLOW:-}!${NC:-} $*"
    else
        echo -e "  [WARN] $*"
    fi
}

format_elapsed() {
    local seconds="$1"
    if (( seconds < 60 )); then
        printf '%s s\n' "$seconds"
    else
        printf '%s min %s s\n' "$((seconds / 60))" "$((seconds % 60))"
    fi
}

# Free space, in MB, on the volume that holds a path's nearest existing parent.
free_space_mb() {
    local path="$1"
    while [[ ! -e "$path" && "$path" != / ]]; do
        path="$(dirname "$path")"
    done
    df -Pk "$path" 2>/dev/null | awk 'NR == 2 { print int($4 / 1024) }'
}

platform_label() {
    case "$1" in
        aarch64-darwin)
            local version
            version="$(sw_vers -productVersion 2>/dev/null || true)"
            printf 'macOS%s on Apple silicon\n' "${version:+ $version}"
            ;;
        x86_64-linux) printf 'Linux on x86-64\n' ;;
        aarch64-linux) printf 'Linux on ARM64\n' ;;
        *) printf '%s\n' "$1" ;;
    esac
}

# Reads the installed version from the trusted source record without running
# the installed binary. Empty when this computer has no ElastOS installation.
installed_version() {
    local sources="$1/sources.json" version
    [[ -f "$sources" ]] || return 0
    version="$(json_get "$sources" \
        '(lambda v: v if isinstance(v, str) else None)(d["sources"][0]["installed_version"])' \
        2>/dev/null || true)"
    if [[ "$version" =~ ^[0-9A-Za-z.+-]{1,64}$ ]]; then
        printf '%s\n' "$version"
    fi
}

short_did() {
    local did="$1"
    if (( ${#did} > 24 )); then
        printf '%s…%s\n' "${did:0:14}" "${did: -6}"
    else
        printf '%s\n' "$did"
    fi
}

installer_data_dir() {
    local home_dir="$1"
    local xdg_data_home="${2:-}"
    [[ "$home_dir" == /* ]] || die "Home directory must be an absolute path"
    case "$(uname -s)" in
        Darwin) printf '%s\n' "${home_dir%/}/Library/Application Support/elastos" ;;
        Linux)
            if [[ "$xdg_data_home" == /* ]]; then
                printf '%s\n' "${xdg_data_home%/}/elastos"
            else
                printf '%s\n' "${home_dir%/}/.local/share/elastos"
            fi
            ;;
        *) die "Unsupported OS: $(uname -s)" ;;
    esac
}

detect_platform() {
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
        darwin)
            [[ "$ARCH" == aarch64 ]] || die "macOS release lookup requires Apple silicon"
            PLATFORM="aarch64-darwin"
            ;;
        *) die "Unsupported OS: ${OS}" ;;
    esac
}

validate_explicit_source_bootstrap_pair() {
    if [[ "$SOURCE_CONNECT_TICKET_EXPLICIT" == true && "$PUBLISHER_NODE_ID_EXPLICIT" != true ]] ||
       [[ "$SOURCE_CONNECT_TICKET_EXPLICIT" != true && "$PUBLISHER_NODE_ID_EXPLICIT" == true ]]; then
        die "trusted-source Carrier bootstrap overrides are atomic; set both ELASTOS_SOURCE_CONNECT_TICKET and ELASTOS_PUBLISHER_NODE_ID, or neither"
    fi
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

sha256_check() {
    local file="$1"
    local expected="$2"
    local actual
    actual=$(sha256_file "$file")
    if [[ "$actual" != "$expected" ]]; then
        die "SHA-256 mismatch!\n  Expected: ${expected}\n  Got:      ${actual}"
    fi
}

sha256_file() {
    local file="$1"
    if command -v sha256sum &>/dev/null; then
        sha256sum "$file" | cut -d' ' -f1
    elif command -v shasum &>/dev/null; then
        shasum -a 256 "$file" | cut -d' ' -f1
    else
        die "Neither sha256sum nor shasum found"
    fi
}

installer_runtime_control() {
    # All callers stop verified processes before changing the selected install.
    # Smoke cleanup can disable the binary scan when using a shared branch binary.
    python3 - "$@" <<'PY_RUNTIME_CONTROL'
import json
import os
from pathlib import Path
import signal
import stat
import subprocess
import sys
import time

PREFIXES = ("serve", "gateway", "room open")


def process_snapshot(pid):
    if type(pid) is not int or not 1 < pid < 2**31:
        raise ValueError("Runtime PID must be a positive process ID greater than one")
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return None
    except PermissionError:
        raise ValueError("Runtime process belongs to another owner")
    result = subprocess.run(
        ["ps", "-ww", "-p", str(pid), "-o", "uid=", "-o", "lstart=", "-o", "stat=", "-o", "command="],
        stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        env=dict(os.environ, LC_ALL="C"), text=True, check=False,
    )
    if result.returncode:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return None
        raise ValueError("Runtime process identity is unavailable")
    parts = result.stdout.strip().split(None, 7)
    if len(parts) != 8 or not parts[0].isdigit():
        raise ValueError("Runtime process identity is ambiguous")
    if int(parts[0]) != os.geteuid():
        raise ValueError("Runtime process belongs to another owner")
    if parts[6].startswith("Z"):
        return None
    return (" ".join(parts[1:6]), parts[7])


def matches_command(snapshot, binary, prefixes=PREFIXES):
    for prefix in prefixes:
        command = binary + " " + prefix
        if snapshot[1] == command or snapshot[1].startswith(command + " "):
            return True
    return False


def stop_owned_process(pid, expected, binary, prefixes=PREFIXES):
    if not matches_command(expected, binary, prefixes):
        raise ValueError("Runtime process does not match the selected binary and command")
    for sig in (signal.SIGTERM, signal.SIGKILL):
        current = process_snapshot(pid)
        if current is None:
            return
        if current != expected:
            raise ValueError("Runtime process identity changed; preserved the new process")
        try:
            os.kill(pid, sig)
        except ProcessLookupError:
            return
        for _ in range(20):
            current = process_snapshot(pid)
            if current is None:
                return
            if current != expected:
                raise ValueError("Runtime process identity changed; preserved the new process")
            time.sleep(0.1)
    raise ValueError("Runtime process did not stop; preserved its state")


def read_coords(path):
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    except FileNotFoundError:
        return None
    with os.fdopen(descriptor, "rb") as source:
        metadata = os.fstat(source.fileno())
        if (not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.geteuid()
                or metadata.st_mode & 0o077):
            raise ValueError("Runtime coordinates require an owner-only regular file")
        data = source.read(65537)
    if len(data) > 65536:
        raise ValueError("Runtime coordinates exceed the size limit")
    value = json.loads(data)
    pid = value.get("pid")
    if type(pid) is not int or not 1 < pid < 2**31:
        raise ValueError("Runtime coordinates contain an invalid PID")
    return (pid, data, metadata.st_dev, metadata.st_ino, metadata.st_mtime)


def remove_dead_coords(path, recorded):
    current = read_coords(path)
    if current is None:
        return
    if current != recorded or process_snapshot(recorded[0]) is not None:
        raise ValueError("Runtime coordinates changed; preserved the new state")
    path.unlink()


def selected_processes(binary):
    result = subprocess.run(
        ["ps", "-ww", "-u", str(os.geteuid()), "-o", "pid=", "-o", "command="],
        stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        env=dict(os.environ, LC_ALL="C"), text=True, check=False,
    )
    if result.returncode:
        raise ValueError("Cannot inspect processes for the selected installation")
    found = {}
    for line in result.stdout.splitlines():
        fields = line.strip().split(None, 1)
        if len(fields) != 2 or not fields[0].isdigit():
            continue
        if not matches_command(("", fields[1]), binary):
            if matches_command(("", fields[1]), os.path.basename(binary)):
                raise ValueError("A Runtime command has an ambiguous binary path; close it and retry")
            continue
        pid = int(fields[0])
        snapshot = process_snapshot(pid)
        if snapshot is not None:
            if not matches_command(snapshot, binary):
                raise ValueError("Runtime process changed during inspection")
            found[pid] = snapshot
    return found


def descendant_processes(parents):
    if not parents:
        return {}
    result = subprocess.run(
        ["ps", "-axo", "pid=", "-o", "ppid="],
        stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        env=dict(os.environ, LC_ALL="C"), text=True, check=False,
    )
    if result.returncode:
        raise ValueError("Cannot inspect Runtime child processes")
    relationships = []
    for line in result.stdout.splitlines():
        fields = line.split()
        if len(fields) != 2 or not all(field.isdigit() for field in fields):
            raise ValueError("Runtime child process ownership is ambiguous")
        relationships.append(tuple(map(int, fields)))
    found = {}
    family = set(parents)
    while True:
        children = {pid for pid, parent in relationships if parent in family and pid not in family}
        if not children:
            return found
        for pid in children:
            snapshot = process_snapshot(pid)
            if snapshot is not None:
                found[pid] = snapshot
        family.update(children)


def wait_for_descendants(children):
    # One overall deadline, independent of the number of captured children.
    deadline = time.monotonic() + 2
    pending = dict(children)
    while pending:
        for pid, expected in list(pending.items()):
            current = process_snapshot(pid)
            if current is None:
                del pending[pid]
            elif current != expected:
                raise ValueError("Runtime child identity changed; preserved its state")
        if not pending:
            return
        if time.monotonic() >= deadline:
            raise ValueError("Runtime child remains active; preserved its state")
        time.sleep(0.1)


def stop_installation(data_dir, binary, scan_binary):
    binary = os.path.abspath(binary)
    data_dir = Path(data_dir)
    records = []
    selected = {}
    # Validate every recorded owner before sending the first signal.
    for name in ("runtime-coords.json", "home-runtime-coords.json", "gateway-runtime-coords.json"):
        path = data_dir / name
        recorded = read_coords(path)
        if recorded is None:
            continue
        records.append((path, recorded))
        pid = recorded[0]
        snapshot = process_snapshot(pid)
        if snapshot is not None:
            if not matches_command(snapshot, binary):
                raise ValueError("Recorded Runtime is foreign or ambiguous; preserved its process and state")
            # ps lstart and this Python process use the same local timezone.
            # Its second precision rejects definite PID reuse; coordinates do
            # not yet carry a process birth identity for finer comparisons.
            started = time.mktime(time.strptime(snapshot[0], "%a %b %d %H:%M:%S %Y"))
            if started > int(recorded[4]):
                raise ValueError("Runtime process started after its ownership record; preserved its process and state")
            selected[pid] = snapshot
    if scan_binary:
        for pid, snapshot in selected_processes(binary).items():
            if pid not in selected:
                raise ValueError("A process using this binary has no ownership record in the selected data directory; close it and retry")
            if selected[pid] != snapshot:
                raise ValueError("Runtime process identity changed during inspection")
    children = descendant_processes(selected)
    for pid, snapshot in selected.items():
        stop_owned_process(pid, snapshot, binary)
    wait_for_descendants(children)
    if scan_binary and selected_processes(binary):
        raise ValueError("A new Runtime started during cleanup; preserved its state")
    for path, recorded in records:
        remove_dead_coords(path, recorded)


if __name__ == "__main__":
    try:
        if len(sys.argv) != 4 or sys.argv[3] not in ("true", "false"):
            raise ValueError("Expected data directory, Runtime binary and binary-scan flag")
        stop_installation(sys.argv[1], sys.argv[2], sys.argv[3] == "true")
    except (ValueError, OSError, TypeError, AttributeError) as error:
        print("Runtime cleanup stopped: " + str(error), file=sys.stderr)
        sys.exit(1)
PY_RUNTIME_CONTROL
}

# Fetch a CID from IPFS gateways (tries each in order)
ipfs_fetch() {
    local cid="$1"
    local output="$2"
    local url
    for gw in ${GATEWAYS[@]+"${GATEWAYS[@]}"}; do
        url="${gw}/ipfs/${cid}"
        if curl -fsSL --max-time 30 -o "$output" "$url" 2>/dev/null; then
            LAST_SUCCESS_GATEWAY="$gw"
            return 0
        fi
    done
    die "Failed to fetch CID ${cid} from any gateway"
}

# Extract a value from a JSON file using python3 (replaces jq dependency).
# Usage: json_get <file> <python-expression>
#   json_get release.json 'd["payload"]["schema"]'
#   json_get release.json 'd["payload"]["platforms"]["aarch64-linux"]["binary"]["cid"]'
json_get() {
    local file="$1"
    local expr="$2"
    python3 - "$file" "$expr" <<'PY'
import json, sys
with open(sys.argv[1], 'r', encoding='utf-8') as f:
    d = json.load(f)
try:
    v = eval(sys.argv[2], {"d": d})
    if v is None:
        sys.exit(0)
    print(v)
except (KeyError, TypeError, IndexError):
    sys.exit(0)
PY
}

# ── Ed25519 verification ─────────────────────────────────────────────

verify_signature() {
    local json_file="$1"
    local domain="$2"
    local expected_did="$3"

    if ! python3 - "$json_file" "$domain" "$expected_did" <<'PY_ED25519'
# RFC 8032 sections 5.1.3, 5.1.4 and 5.1.7:
# https://www.rfc-editor.org/rfc/rfc8032.html#section-5.1
# Verification only: all curve operations below use public data.
import hashlib
import json
import re
import sys

FIELD = 2**255 - 19
ORDER = 2**252 + 27742317777372353535851937790883648493
D = -121665 * pow(121666, FIELD - 2, FIELD) % FIELD
SQRT_MINUS_ONE = pow(2, (FIELD - 1) // 4, FIELD)
IDENTITY = (0, 1, 1, 0)


def point_add(left, right):
    x1, y1, z1, t1 = left
    x2, y2, z2, t2 = right
    a = (y1 - x1) * (y2 - x2) % FIELD
    b = (y1 + x1) * (y2 + x2) % FIELD
    c = 2 * D * t1 * t2 % FIELD
    d = 2 * z1 * z2 % FIELD
    e, f, g, h = b - a, d - c, d + c, b + a
    return (e * f % FIELD, g * h % FIELD, f * g % FIELD, e * h % FIELD)


def point_mul(scalar, point):
    result = IDENTITY
    while scalar:
        if scalar & 1:
            result = point_add(result, point)
        point = point_add(point, point)
        scalar >>= 1
    return result


def point_equal(left, right):
    return ((left[0] * right[2] - right[0] * left[2]) % FIELD == 0
            and (left[1] * right[2] - right[1] * left[2]) % FIELD == 0)


def decode_point(encoded):
    if len(encoded) != 32:
        raise ValueError("Ed25519 point must contain 32 bytes")
    packed = int.from_bytes(encoded, "little")
    y, sign = packed & (2**255 - 1), packed >> 255
    if y >= FIELD:
        raise ValueError("Noncanonical Ed25519 point")
    y_squared = y * y % FIELD
    x_squared = (y_squared - 1) * pow(D * y_squared + 1, FIELD - 2, FIELD) % FIELD
    x = pow(x_squared, (FIELD + 3) // 8, FIELD)
    if (x * x - x_squared) % FIELD:
        x = x * SQRT_MINUS_ONE % FIELD
    if (x * x - x_squared) % FIELD or (x == 0 and sign):
        raise ValueError("Invalid Ed25519 point")
    if x & 1 != sign:
        x = FIELD - x
    return (x, y, 1, x * y % FIELD)


BASE = decode_point(bytes.fromhex("58" + "66" * 31))


def verify_ed25519(public_key, message, signature):
    if len(signature) != 64:
        raise ValueError("Ed25519 signature must contain 64 bytes")
    public = decode_point(public_key)
    r_point = decode_point(signature[:32])
    scalar = int.from_bytes(signature[32:], "little")
    if scalar >= ORDER:
        raise ValueError("Noncanonical Ed25519 scalar")
    # The installer accepts canonical points and the strict verification
    # equation. Reject small-order keys and R values, including the identity.
    if any(point_equal(point_mul(8, point), IDENTITY) for point in (public, r_point)):
        raise ValueError("Small-order Ed25519 point")
    challenge = int.from_bytes(
        hashlib.sha512(signature[:32] + public_key + message).digest(), "little"
    ) % ORDER
    if not point_equal(point_mul(scalar, BASE), point_add(r_point, point_mul(challenge, public))):
        raise ValueError("Ed25519 signature does not match")


def decode_did_key(did):
    if not isinstance(did, str) or not did.startswith("did:key:z"):
        raise ValueError("Expected an Ed25519 did:key with base58btc encoding")
    encoded = did[len("did:key:z"):]
    if not 1 <= len(encoded) <= 64:
        raise ValueError("Invalid DID key length")
    alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
    number = 0
    for char in encoded:
        number = number * 58 + alphabet.index(char)
    raw = (b"\0" * (len(encoded) - len(encoded.lstrip("1")))
           + number.to_bytes((number.bit_length() + 7) // 8, "big"))
    if len(raw) != 34 or raw[:2] != b"\xed\x01":
        raise ValueError("DID key must use the Ed25519 multicodec")
    return raw[2:]


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("Duplicate JSON field: " + key)
        result[key] = value
    return result


def verify_envelope(json_file, domain, expected_did):
    with open(json_file, "r", encoding="utf-8") as source:
        envelope = json.load(source, object_pairs_hook=unique_object)
    signer = envelope["signer_did"]
    if not expected_did or signer != expected_did:
        raise ValueError("Envelope signer differs from the pinned maintainer DID")
    payload = envelope["payload"]
    if not isinstance(payload, dict):
        raise ValueError("Release payload must be a JSON object")
    if "signer_did" in payload and payload["signer_did"] != signer:
        raise ValueError("Payload and envelope signer differ")
    signature = envelope["signature"]
    if not isinstance(signature, str) or not re.fullmatch(r"[0-9a-fA-F]{128}", signature):
        raise ValueError("Signature must contain 64 hex-encoded bytes")
    # Matches the publisher's compact sorted UTF-8 JSON and Runtime's
    # SHA256(domain + NUL + payload), signed with ordinary Ed25519.
    canonical = json.dumps(payload, separators=(",", ":"), sort_keys=True,
                           ensure_ascii=False, allow_nan=False).encode("utf-8")
    digest = hashlib.sha256(domain.encode("utf-8") + b"\0" + canonical).digest()
    verify_ed25519(decode_did_key(signer), digest, bytes.fromhex(signature))


if __name__ == "__main__":
    try:
        verify_envelope(*sys.argv[1:])
    except (ValueError, TypeError, KeyError, OSError, AttributeError) as error:
        print("Signature verification failed: " + str(error), file=sys.stderr)
        sys.exit(1)
PY_ED25519
    then
        die "Signature verification FAILED"
    fi
    ok "Signature verified"
}

validate_release_identity() {
    # Both envelopes are verified before this check, on every transport.
    # The signed digest binds exact envelope bytes; the CID stays content identity.
    if ! python3 - "$1" "$2" <<'PY_RELEASE_IDENTITY'
import hashlib
import json
import re
import sys

try:
    with open(sys.argv[1], encoding="utf-8") as source:
        head = json.load(source)["payload"]
    with open(sys.argv[2], "rb") as source:
        release_bytes = source.read()
    release = json.loads(release_bytes)["payload"]
    if head.get("schema") != "elastos.release.head/v1" or release.get("schema") != "elastos.release/v1":
        raise ValueError("Unexpected release schema")
    expected = head.get("release_sha256")
    if not isinstance(expected, str) or not re.fullmatch(r"[0-9a-f]{64}", expected):
        raise ValueError("Release head requires a lowercase SHA-256 envelope binding; ask the publisher to update its metadata")
    if hashlib.sha256(release_bytes).hexdigest() != expected:
        raise ValueError("Release envelope differs from the signed head")
    core = r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    if not re.fullmatch(core + r"(-(alpha|beta|rc)\.(0|[1-9][0-9]*))?(\+[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?", str(head.get("version", ""))):
        raise ValueError("Invalid signed release version; expected X.Y.Z or X.Y.Z-{alpha|beta|rc}.N")
    for field in ("version", "channel"):
        value = head.get(field)
        if not isinstance(value, str) or not value or release.get(field) != value:
            raise ValueError("Release head and release " + field + " must match")
except (ValueError, TypeError, KeyError, OSError, AttributeError) as error:
    print("Release identity check failed: " + str(error), file=sys.stderr)
    sys.exit(1)
PY_RELEASE_IDENTITY
    then
        die "Release binding or metadata mismatch; retry after the publisher finishes updating"
    fi
}

# A path a person can paste into a shell: ~/... under HOME, quoted elsewhere.
display_path() {
    local path="$1" quoted
    quoted="$(printf '%q' "$path")"
    if [[ "$quoted" == "$path" && -n "${HOME:-}" && "$path" == "${HOME%/}"/* ]]; then
        printf '%s/%s' '~' "${path#"${HOME%/}"/}"
    else
        printf '%s' "$quoted"
    fi
}

runtime_command() {
    local runtime_bin="$1"
    if [[ "$(command -v elastos 2>/dev/null || true)" == "$runtime_bin" ]]; then
        printf 'elastos'
    else
        display_path "$runtime_bin"
    fi
}

show_ready() {
    local runtime_bin="$1" command elapsed
    command="$(runtime_command "$runtime_bin")"
    elapsed="$(format_elapsed $((SECONDS - ${INSTALL_STARTED_AT:-0})))"
    echo ""
    echo -e "  ${GREEN}${BOLD}ElastOS ${RELEASE_VERSION:-} is ready${NC} ${DIM}(took ${elapsed})${NC}"
    printf '    %b%-12s%b %s\n' "$DIM" "Open Home" "$NC" "${command} home --browser"
    printf '    %b%-12s%b %s\n' "$DIM" "Update" "$NC" "${command} update"
    if [[ "$command" != elastos ]]; then
        local quoted path_entry
        quoted="$(printf '%q' "$INSTALL_DIR")"
        if [[ "$quoted" == "$INSTALL_DIR" && -n "${HOME:-}" && "$INSTALL_DIR" == "${HOME%/}"/* ]]; then
            path_entry="\"\$HOME/${INSTALL_DIR#"${HOME%/}"/}:\$PATH\""
        else
            path_entry="${quoted}:\"\$PATH\""
        fi
        printf '    %b%-12s%b %s\n' "$DIM" "Add to PATH" "$NC" "export PATH=${path_entry}"
    fi
    echo ""
}

# Setup leaves the curl pipe unread. Browser Home runs in the controlling
# terminal; non-interactive provisioning prints the command for a later launch.
finish_install() {
    local runtime_bin="${INSTALL_DIR}/elastos"
    if [[ "$INSTALL_ONLY" == true || "$INSTALL_ONLY" == 1 ]]; then
        info "Runtime installed: ${runtime_bin}"
        return 0
    fi
    step 5 "Set up Home"
    info "Setting up Home..."
    "$runtime_bin" setup </dev/null || return $?
    show_ready "$runtime_bin"
    if ( : </dev/tty ) 2>/dev/null && [[ -t 1 ]]; then
        info "Opening Home..."
        "$runtime_bin" home --browser </dev/tty || return $?
    else
        info "Home is installed. Open it from a terminal:"
        printf '  %q home --browser\n' "$runtime_bin"
    fi
}

# ── Parse args ────────────────────────────────────────────────────────

# Repo smoke/publisher helpers source these definitions inside a subshell.
# The downloaded installer remains one self-contained script.
if [[ "${BASH_SOURCE[0]:-$0}" != "$0" ]]; then
    return 0
fi

INSTALL_DIR="${HOME}/.local/bin"
INSTALL_ONLY="${ELASTOS_INSTALL_ONLY:-false}"
INSTALL_STARTED_AT=$SECONDS
installer_select_output

while [[ $# -gt 0 ]]; do
    case "$1" in
        --help|-h) show_help ;;
        --head-cid)
            [[ -z "${2:-}" ]] && die "Usage: --head-cid CID"
            HEAD_CID="$2"; shift 2 ;;
        --maintainer-did)
            [[ -z "${2:-}" ]] && die "Usage: --maintainer-did did:key:z6Mk..."
            MAINTAINER_DID="$2"; shift 2 ;;
        --gateway)
            [[ -z "${2:-}" ]] && die "Usage: --gateway https://<gateway>"
            gw="${2%/}"
            gw="${gw%/ipfs}"
            CLI_GATEWAYS+=("$gw"); shift 2 ;;
        --publisher-gateway)
            [[ -z "${2:-}" ]] && die "Usage: --publisher-gateway https://<url>"
            PUBLISHER_GATEWAY="${2%/}"; shift 2 ;;
        --publisher-node-id)
            [[ -z "${2:-}" ]] && die "Usage: --publisher-node-id <node-id>"
            PUBLISHER_NODE_ID="$2"; PUBLISHER_NODE_ID_EXPLICIT=true; shift 2 ;;
        --install-only) INSTALL_ONLY=true; shift ;;
        --install-dir)
            [[ -z "${2:-}" ]] && die "Usage: --install-dir PATH"
            INSTALL_DIR="$2"; shift 2 ;;
        *) die "Unknown option: $1. Run --help for usage." ;;
    esac
done

if [[ "$INSTALL_ONLY" == true || "$INSTALL_ONLY" == 1 ]]; then
    INSTALLER_STEPS=4
fi

show_banner
step 1 "Check this computer"
detect_platform
ok "Platform: $(platform_label "$PLATFORM") (${PLATFORM})"

validate_explicit_source_bootstrap_pair

if [[ ${#CLI_GATEWAYS[@]} -gt 0 ]]; then
    GATEWAYS=("${CLI_GATEWAYS[@]}")
elif [[ -n "${ELASTOS_IPFS_GATEWAYS:-}" ]]; then
    GATEWAYS=()
    IFS=', ' read -r -a ENV_GATEWAYS <<< "${ELASTOS_IPFS_GATEWAYS}"
    for gw in ${ENV_GATEWAYS[@]+"${ENV_GATEWAYS[@]}"}; do
        [[ -z "$gw" ]] && continue
        gw="${gw%/}"
        gw="${gw%/ipfs}"
        GATEWAYS+=("$gw")
    done
fi

# Prepend bootstrap publisher URL if available (it also serves /ipfs/<cid>/)
if [[ -n "$PUBLISHER_GATEWAY" ]]; then
    if [[ ${#GATEWAYS[@]} -gt 0 ]]; then
        GATEWAYS=("${PUBLISHER_GATEWAY%/}" "${GATEWAYS[@]}")
    else
        GATEWAYS=("${PUBLISHER_GATEWAY%/}")
    fi
fi

if [[ -z "$PUBLISHER_GATEWAY" && ${#GATEWAYS[@]} -eq 0 ]]; then
    die "No bootstrap publisher URL configured and no explicit IPFS gateways provided.\n  Use the canonical publisher install URL, or pass --gateway <url> for operator/debug bootstrap."
fi

# ── Validate trust anchors ────────────────────────────────────────────

if [[ -z "$HEAD_CID" && -z "$PUBLISHER_GATEWAY" ]]; then
    die "No bootstrap publisher URL and no HEAD_CID. Either:\n  1. Set ELASTOS_PUBLISHER_GATEWAY env var, or\n  2. Set ELASTOS_HEAD_CID env var, or\n  3. Pass --head-cid <CID>."
fi

if [[ -z "$MAINTAINER_DID" ]]; then
    die "MAINTAINER_DID not set. Either:\n  1. Set ELASTOS_MAINTAINER_DID env var, or\n  2. Pass --maintainer-did <did:key:...>."
fi
# The DID reaches the terminal before its signature check, so only base58 did:key text passes.
if [[ ! "$MAINTAINER_DID" =~ ^did:key:z[1-9A-HJ-NP-Za-km-z]{1,128}$ ]]; then
    die "MAINTAINER_DID must be a did:key value (did:key:z6Mk...)."
fi

# ── Preflight ─────────────────────────────────────────────────────────

for cmd in curl python3; do
    command -v "$cmd" &>/dev/null || die "Required tool not found: $cmd"
done

if ! command -v sha256sum &>/dev/null && ! command -v shasum &>/dev/null; then
    die "Neither sha256sum nor shasum found"
fi
ok "Tools: curl, python3 and SHA-256 are available"

DATA_DIR="$(installer_data_dir "$HOME" "${XDG_DATA_HOME:-}")"
PREVIOUS_VERSION="$(installed_version "$DATA_DIR")"
EXISTING_INSTALL=false
if [[ -n "$PREVIOUS_VERSION" ]]; then
    EXISTING_INSTALL=true
    ok "Existing installation: ElastOS ${PREVIOUS_VERSION}"
elif [[ -e "${INSTALL_DIR}/elastos" || -e "${DATA_DIR}/sources.json" ]]; then
    EXISTING_INSTALL=true
    ok "Existing installation found"
else
    ok "Fresh install: no ElastOS installation found"
fi

FREE_MB="$(free_space_mb "$DATA_DIR" || true)"
if [[ "$FREE_MB" =~ ^[0-9]+$ ]]; then
    if (( FREE_MB < 1024 )); then
        ok "Disk: ${FREE_MB} MB free"
    else
        ok "Disk: $((FREE_MB / 1024)) GB free"
    fi
fi

# ── Fetch + verify release head ──────────────────────────────────────

step 2 "Verify the release"
info "Maintainer DID: ${MAINTAINER_DID}"

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

# Publisher gateway is the source of truth. No fallback.
if [[ -n "$PUBLISHER_GATEWAY" ]]; then
    PG="${PUBLISHER_GATEWAY%/}"
    info "Fetching release head from ${PG}"
    curl -fsSL --max-time 30 -o "${TMPDIR}/release-head.json" "${PG}/release-head.json" \
        || die "Publisher gateway unreachable: ${PG}/release-head.json"
else
    # No bootstrap publisher URL — use CID-based fetch (operator/debug bootstrap only)
    [[ -z "$HEAD_CID" ]] && die "No bootstrap publisher URL and no HEAD_CID configured"
    info "Fetching release head by CID: ${HEAD_CID} (bootstrap mode)"
    ipfs_fetch "$HEAD_CID" "${TMPDIR}/release-head.json"
fi

HEAD_SCHEMA=$(json_get "${TMPDIR}/release-head.json" 'd["payload"]["schema"]') \
    || die "Invalid release-head.json format"
[[ "$HEAD_SCHEMA" != "elastos.release.head/v1" ]] && \
    die "Unexpected head schema: ${HEAD_SCHEMA}"

info "Verifying release head signature..."
verify_signature "${TMPDIR}/release-head.json" "elastos.release.head.v1" "$MAINTAINER_DID"

RELEASE_CID=$(json_get "${TMPDIR}/release-head.json" 'd["payload"]["latest_release_cid"]')
RELEASE_VERSION=$(json_get "${TMPDIR}/release-head.json" 'd["payload"]["version"]')
RELEASE_CHANNEL=$(json_get "${TMPDIR}/release-head.json" 'd["payload"].get("channel","stable")')
if ! is_allowed_channel "$RELEASE_CHANNEL"; then
    die "Unsupported release channel '${RELEASE_CHANNEL}' in release-head.json. Allowed channels: ${ALLOWED_CHANNELS[*]}"
fi
ok "Release: ElastOS ${RELEASE_VERSION} (${RELEASE_CHANNEL}), signed by $(short_did "$MAINTAINER_DID")"
if [[ "$PREVIOUS_VERSION" == "$RELEASE_VERSION" ]]; then
    info "Reinstalling the installed version"
elif [[ -n "$PREVIOUS_VERSION" ]]; then
    info "Update: ${PREVIOUS_VERSION} → ${RELEASE_VERSION}"
fi

# ── Fetch + verify release.json ──────────────────────────────────────

if [[ -n "$PUBLISHER_GATEWAY" ]]; then
    info "Fetching release bootstrap metadata from publisher URL"
    curl -fsSL --max-time 30 -o "${TMPDIR}/release.json" "${PG}/release.json" \
        || die "Publisher gateway unreachable: ${PG}/release.json"
else
    info "Fetching release by CID: ${RELEASE_CID} (bootstrap mode)"
    ipfs_fetch "$RELEASE_CID" "${TMPDIR}/release.json"
fi

RELEASE_SCHEMA=$(json_get "${TMPDIR}/release.json" 'd["payload"]["schema"]') \
    || die "Invalid release.json format"
[[ "$RELEASE_SCHEMA" != "elastos.release/v1" ]] && \
    die "Unexpected release schema: ${RELEASE_SCHEMA}"

info "Verifying release signature..."
verify_signature "${TMPDIR}/release.json" "elastos.release.v1" "$MAINTAINER_DID"
validate_release_identity "${TMPDIR}/release-head.json" "${TMPDIR}/release.json"

# ── Extract platform info ────────────────────────────────────────────

BINARY_CID=$(json_get "${TMPDIR}/release.json" "d['payload']['platforms']['${PLATFORM}']['binary']['cid']")
BINARY_SHA256=$(json_get "${TMPDIR}/release.json" "d['payload']['platforms']['${PLATFORM}']['binary']['sha256']")

if [[ -z "$BINARY_CID" ]]; then
    AVAILABLE=$(json_get "${TMPDIR}/release.json" "', '.join(d['payload'].get('platforms',{}).keys())")
    die "No release available for platform: ${PLATFORM}\n  Available: ${AVAILABLE:-none}"
fi

# ── Download + verify binary ─────────────────────────────────────────

step 3 "Download ElastOS Runtime"
BINARY_SIZE=$(json_get "${TMPDIR}/release.json" "d['payload']['platforms']['${PLATFORM}']['binary'].get('size')")
BINARY_SIZE_LABEL=""
if [[ "$BINARY_SIZE" =~ ^[0-9]+$ ]] && (( BINARY_SIZE >= 1048576 )); then
    BINARY_SIZE_LABEL=" ($((BINARY_SIZE / 1048576)) MB)"
fi

if [[ -n "$PUBLISHER_GATEWAY" ]]; then
    info "Downloading binary${BINARY_SIZE_LABEL} from ${PG}"
    CURL_BINARY_FLAGS=(-fL)
    if [[ -t 2 ]]; then
        CURL_BINARY_FLAGS+=(--progress-bar)
    else
        CURL_BINARY_FLAGS+=(-sS)
    fi
    curl "${CURL_BINARY_FLAGS[@]}" \
        --retry "${BINARY_DOWNLOAD_RETRY_COUNT}" \
        --retry-all-errors \
        --retry-delay "${BINARY_DOWNLOAD_RETRY_DELAY}" \
        --connect-timeout "${BINARY_DOWNLOAD_CONNECT_TIMEOUT}" \
        --speed-limit "${BINARY_DOWNLOAD_SPEED_LIMIT}" \
        --speed-time "${BINARY_DOWNLOAD_SPEED_TIME}" \
        --max-time "${BINARY_DOWNLOAD_MAX_TIME}" \
        -o "${TMPDIR}/elastos" "${PG}/artifacts/elastos-${PLATFORM}" \
        || die "Failed to download binary from ${PG}/artifacts/elastos-${PLATFORM}"
else
    info "Downloading binary${BINARY_SIZE_LABEL} by CID: ${BINARY_CID} (bootstrap mode)"
    ipfs_fetch "$BINARY_CID" "${TMPDIR}/elastos"
fi

info "Verifying binary SHA-256..."
sha256_check "${TMPDIR}/elastos" "$BINARY_SHA256"

# ── Admit the verified release with the shared installation writer ──

[[ "$INSTALL_DIR" == /* ]] || INSTALL_DIR="${PWD}/${INSTALL_DIR}"
mkdir -p "$INSTALL_DIR"

# New Runtime data is private; preserve the mode of an existing installation.
(umask 077; mkdir -p "$DATA_DIR")

TMP_INSTALL_BIN="$(mktemp "${INSTALL_DIR}/.elastos.install.XXXXXX")"
trap 'rm -rf "$TMPDIR" "$TMP_INSTALL_BIN"' EXIT
cp "${TMPDIR}/elastos" "${TMP_INSTALL_BIN}"
chmod +x "${TMP_INSTALL_BIN}"

# The staged executable must report the exact release version on stdout with
# empty stderr before it can write this installation.
STAGED_VERSION_STATUS=0
STAGED_VERSION_STDERR_PATH="${TMPDIR}/elastos-version.stderr"
STAGED_VERSION_OUTPUT="$("${TMP_INSTALL_BIN}" --version 2>"${STAGED_VERSION_STDERR_PATH}")" || STAGED_VERSION_STATUS=$?
STAGED_VERSION_ERROR="$(cat "${STAGED_VERSION_STDERR_PATH}")"
if [[ "${STAGED_VERSION_STATUS}" -ne 0 ]]; then
    die "Downloaded binary failed its version check (exit ${STAGED_VERSION_STATUS}); the current installation was preserved\n  Output: ${STAGED_VERSION_OUTPUT:-<no output>}\n  Stderr: ${STAGED_VERSION_ERROR:-<no output>}"
fi
if [[ "${STAGED_VERSION_OUTPUT}" != "elastos ${RELEASE_VERSION}" || -s "${STAGED_VERSION_STDERR_PATH}" ]]; then
    die "Downloaded binary version mismatch; the current installation was preserved\n  Expected: ${RELEASE_VERSION}\n  Got:      ${STAGED_VERSION_OUTPUT:-<no output>}\n  Stderr: ${STAGED_VERSION_ERROR:-<no output>}"
fi
ok "Checksum and version match the signed release"

# ── Carrier contact + release metadata for `elastos upgrade` ─────────

SIGNER_DID=$(json_get "${TMPDIR}/release-head.json" 'd["signer_did"]')

SOURCES_PATH="${TMPDIR}/sources.json"
PUBLISHER_HASH=$(SIGNER_DID="${SIGNER_DID}" python3 - <<'PY'
import hashlib
import os
publisher = os.environ["SIGNER_DID"]
print(hashlib.sha256(publisher.encode("utf-8")).hexdigest()[:32])
PY
)
PUBLISHER_DID="${SIGNER_DID}" \
PUBLISHER_GATEWAY="${PUBLISHER_GATEWAY}" \
INSTALLED_VERSION="${RELEASE_VERSION}" \
INSTALLED_HEAD_CID="${HEAD_CID}" \
INSTALLED_CHANNEL="${RELEASE_CHANNEL}" \
INSTALLED_BINARY_PATH="${INSTALL_DIR}/elastos" \
SOURCE_CONNECT_TICKET="${SOURCE_CONNECT_TICKET}" \
PUBLISHER_NODE_ID="${PUBLISHER_NODE_ID}" \
IPNS_NAME="${IPNS_NAME}" \
SOURCES_PATH="${SOURCES_PATH}" \
python3 - <<'PY'
import json
import os
import hashlib

publisher_gw = os.environ.get("PUBLISHER_GATEWAY", "").strip()
gateways = [publisher_gw] if publisher_gw else []
raw_channel = os.environ["INSTALLED_CHANNEL"] or "stable"
channel = "".join(
    ch if ch.isalnum() or ch in "-_" else "-"
    for ch in raw_channel
) or "stable"
publisher_did = os.environ["PUBLISHER_DID"]
publisher_hash = hashlib.sha256(publisher_did.encode("utf-8")).hexdigest()[:32]
discovery_uri = f"elastos://source/{channel}/{publisher_hash}"
connect_ticket = os.environ.get("SOURCE_CONNECT_TICKET", "")
publisher_node_id = os.environ.get("PUBLISHER_NODE_ID", "")
ipns_name = os.environ.get("IPNS_NAME", "")

sources = {
    "schema": "elastos.trusted-sources/v1",
    "default_source": "default",
    "sources": [
        {
            "name": "default",
            "publisher_dids": [publisher_did],
            "channel": channel,
            "discovery_uri": discovery_uri,
            "connect_ticket": connect_ticket,
            "publisher_node_id": publisher_node_id,
            "ipns_name": ipns_name,
            "gateways": gateways,
            "install_path": os.environ["INSTALLED_BINARY_PATH"],
            "installed_version": os.environ["INSTALLED_VERSION"],
            "head_cid": os.environ["INSTALLED_HEAD_CID"],
        }
    ],
}

with open(os.environ["SOURCES_PATH"], "w", encoding="utf-8") as f:
    json.dump(sources, f, indent=2)
    f.write("\n")
PY

# The writer refuses an older release, another channel, a pending Home
# update and a concurrent writer before this installer stops Runtime.
INSTALL_RELEASE=("${TMP_INSTALL_BIN}" install-release --data-dir "$DATA_DIR"
    --binary "${INSTALL_DIR}/elastos" --candidate "${TMP_INSTALL_BIN}" "$SOURCES_PATH"
    "${TMPDIR}/release-head.json" "${TMPDIR}/release.json")
step 4 "Install"
"${INSTALL_RELEASE[@]}" --check || die "This installation was not changed"

info "Stopping verified Runtime processes for this installation so the new version can be installed..."
installer_runtime_control "$DATA_DIR" "${INSTALL_DIR}/elastos" true \
    || die "Close this installation's Runtime and retry; its existing files were preserved"
if [[ "${EXISTING_INSTALL:-true}" == true ]]; then
    info "Open Home again after installation to reconnect."
fi

# One journaled transaction replaces the Runtime, trusted sources and the
# verified release pair; an interrupted run restores the previous set.
info "Installing binary to $(display_path "${INSTALL_DIR}/elastos")..."
"${INSTALL_RELEASE[@]}" || die "The previous installation was preserved"
ok "Installed ElastOS ${RELEASE_VERSION} to $(display_path "${INSTALL_DIR}/elastos")"

PRINCIPAL_ROOT_BACKUP_DIR="${DATA_DIR}/backups/principal-root-upgrade-$(date -u +%s)-$$"
info "Verifying and upgrading configured protected roots while Runtime is stopped..."
"${INSTALL_DIR}/elastos" principal-root-upgrade \
    --data-dir "${DATA_DIR}" \
    --backup-dir "${PRINCIPAL_ROOT_BACKUP_DIR}"
ok "Saved trusted source config to $(display_path "${DATA_DIR}/sources.json")"
ok "Saved verified release inputs for Runtime setup and updates"

# ── Complete installation ─────────────────────────────────────────────

finish_install
