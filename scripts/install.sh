#!/usr/bin/env bash
#
# ElastOS Installer — Linux preview web bootstrap, then Carrier-backed updates/setup
#
# Usage:
#   curl -fsSL https://<publisher-origin>/install.sh | bash   # Linux x86_64/aarch64 preview
#
#   ELASTOS_HEAD_CID=QmXyz ELASTOS_MAINTAINER_DID=did:key:z6Mk... \
#     curl -fsSL https://<explicit-gateway>/ipfs/<installer-cid>/install.sh | bash
#
#   ./scripts/install.sh --head-cid QmXyz...
#   ./scripts/install.sh --head-cid QmXyz... --maintainer-did did:key:z6Mk...
#   ./scripts/install.sh --head-cid QmXyz... --allow-unsigned
#   ./scripts/install.sh --help
#
# Required (one of):
#   ELASTOS_HEAD_CID env var   or   --head-cid <CID>
#   ELASTOS_MAINTAINER_DID env var   or   --maintainer-did <did:key:...>
#   (or --allow-unsigned to skip sig check)
#
# Trust anchors can be provided via env vars or CLI flags. In the canonical
# bootstrap flow, they should already be stamped into install.sh.
#
# Downloads exactly 2 files:
#   1. elastos binary → ~/.local/bin/elastos
#   2. components.json → ${XDG_DATA_HOME:-~/.local/share}/elastos/components.json
#
# Capsules are NOT pre-installed. They are downloaded on-demand by the
# supervisor when a command needs them (e.g., `elastos chat` downloads
# chat + its provider dependencies automatically).
#
# Trust model:
#   1. Bootstrap over the stamped publisher URL (or explicit operator/debug CID gateway)
#   2. Verify Ed25519 signature against pinned MAINTAINER_DID
#   3. Follow latest_release_cid to release.json
#   4. Verify release signature
#   5. Download binary + components.json, verify SHA-256
#   6. Install to ~/.local/bin/elastos + ${XDG_DATA_HOME:-~/.local/share}/elastos/
#   7. Save trusted-source Carrier metadata for later `setup` and `update`
#
# Fails closed if trust anchors or signature verification fail, unless the
# operator explicitly selects --allow-unsigned.
#
# Dependencies: curl, python3 (stdlib only), sha256sum|shasum
#

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
NC='\033[0m'

# ── Help ──────────────────────────────────────────────────────────────

show_help() {
    echo ""
    echo -e "${BOLD}ElastOS Installer${NC}"
    echo "  Current public install preview: Linux x86_64/aarch64."
    echo "  macOS uses source-home staging for now; see docs/MAC.md."
    echo ""
    echo -e "${BOLD}Usage:${NC}"
    echo "  curl -fsSL https://<publisher-origin>/install.sh | bash   # Linux x86_64/aarch64 preview"
    echo "  curl -fsSL https://<explicit-gateway>/ipfs/<installer-cid>/install.sh | bash   # operator/debug only"
    echo "  ./scripts/install.sh [options]"
    echo ""
    echo -e "${BOLD}Options:${NC}"
    echo "  --head-cid CID       Override bootstrap head CID"
    echo "  --maintainer-did DID Override maintainer DID trust anchor"
    echo "  --gateway URL        IPFS gateway base URL (repeatable, operator/debug bootstrap)"
    echo "  --publisher-gateway URL  Bootstrap publisher URL (stamped for normal installs)"
    echo "  --publisher-node-id ID   Publisher P2P node ID (for durable Carrier link)"
    echo "  --allow-unsigned      Skip signature verification (NOT recommended)"
    echo "  --install-dir PATH    Binary install directory (default: ~/.local/bin)"
    echo "  --help                Show this help"
    echo ""
    echo -e "${BOLD}What gets installed:${NC}"
    echo "  ~/.local/bin/elastos                     Runtime binary"
    echo "  \${XDG_DATA_HOME:-~/.local/share}/elastos/components.json   Capsule registry"
    echo ""
    echo -e "${BOLD}What does NOT get installed:${NC}"
    echo "  Capsules are downloaded on-demand when you run commands."
    echo "  Example: 'elastos chat' auto-downloads chat + providers."
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

# ── Helpers ───────────────────────────────────────────────────────────

die()  { echo -e "${RED}Error:${NC} $*" >&2; exit 1; }
info() { echo -e "  ${GREEN}▶${NC} $*"; }
warn() { echo -e "  ${YELLOW}!${NC} $*"; }

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
        *) die "Unsupported OS: ${OS}. Current public install preview is Linux-only." ;;
    esac
}

validate_explicit_source_bootstrap_pair() {
    if [[ "$SOURCE_CONNECT_TICKET_EXPLICIT" == true && "$PUBLISHER_NODE_ID_EXPLICIT" != true ]] ||
       [[ "$SOURCE_CONNECT_TICKET_EXPLICIT" != true && "$PUBLISHER_NODE_ID_EXPLICIT" == true ]]; then
        die "trusted-source Carrier bootstrap overrides are atomic; set both ELASTOS_SOURCE_CONNECT_TICKET and ELASTOS_PUBLISHER_NODE_ID, or neither"
    fi
}

refresh_source_bootstrap_from_publisher() {
    [[ -n "$PUBLISHER_GATEWAY" ]] || return 0
    if [[ "$SOURCE_CONNECT_TICKET_EXPLICIT" == true && "$PUBLISHER_NODE_ID_EXPLICIT" == true ]]; then
        return 0
    fi

    local bootstrap_url parsed
    bootstrap_url="${PUBLISHER_GATEWAY%/}/.well-known/elastos/carrier-bootstrap.json?role=publisher"
    if ! parsed=$(curl -fsSL --max-time 10 "$bootstrap_url" | python3 -c '
import json
import sys

try:
    data = json.load(sys.stdin)
except Exception:
    raise SystemExit(1)

if data.get("schema") != "elastos.carrier.bootstrap/v1":
    raise SystemExit(1)
if data.get("role") != "publisher":
    raise SystemExit(1)
ticket = (data.get("ticket") or "").strip()
node_id = (data.get("node_id") or "").strip()
if not ticket or not node_id:
    raise SystemExit(1)
print(ticket)
print(node_id)
'); then
        warn "Could not refresh trusted-source Carrier bootstrap from ${bootstrap_url}; using stamped source route"
        return 0
    fi

    local refreshed=() line
    while IFS= read -r line; do
        refreshed+=("$line")
    done <<<"$parsed"
    if [[ "${SOURCE_CONNECT_TICKET_EXPLICIT}" != true ]]; then
        SOURCE_CONNECT_TICKET="${refreshed[0]:-}"
    fi
    if [[ "${PUBLISHER_NODE_ID_EXPLICIT}" != true ]]; then
        PUBLISHER_NODE_ID="${refreshed[1]:-}"
    fi
    if [[ -n "$SOURCE_CONNECT_TICKET" && -n "$PUBLISHER_NODE_ID" ]]; then
        info "Refreshed trusted-source Carrier bootstrap from publisher gateway"
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

stop_stale_runtime_if_needed() {
    local coords_path="$1"
    local label="$2"
    local expected_sha="$3"
    local pid=""
    local running_sha=""

    [[ -f "$coords_path" ]] || return 0

    read -r pid running_sha < <(python3 - "$coords_path" <<'PY'
import json
import sys

path = sys.argv[1]
try:
    data = json.load(open(path, "r", encoding="utf-8"))
except Exception:
    print("")
    sys.exit(0)
pid = data.get("pid", "")
sha = data.get("binary_sha256", "")
print(f"{pid} {sha}")
PY
    )

    if [[ -z "$pid" ]]; then
        rm -f "$coords_path"
        return 0
    fi

    if [[ ! -d "/proc/${pid}" ]]; then
        rm -f "$coords_path"
        return 0
    fi

    if [[ -n "$running_sha" && "$running_sha" == "$expected_sha" ]]; then
        return 0
    fi

    info "Stopping stale ${label} (pid ${pid}) so the new install starts cleanly"
    kill "${pid}" 2>/dev/null || true
    for _ in 1 2 3 4 5 6 7 8 9 10; do
        [[ ! -d "/proc/${pid}" ]] && break
        sleep 0.2
    done
    if [[ -d "/proc/${pid}" ]]; then
        kill -9 "${pid}" 2>/dev/null || true
    fi
    rm -f "$coords_path"
}

stop_stale_installed_elastos_processes() {
    local label="$1"
    local expected_sha="$2"
    shift 2
    local binary="${INSTALL_DIR}/elastos"
    local pid=""
    local running_sha=""

    [[ -x "$binary" ]] || return 0

    while IFS= read -r pid; do
        [[ -n "$pid" ]] || continue
        [[ -d "/proc/${pid}" ]] || continue
        running_sha=$(sha256_file "/proc/${pid}/exe" 2>/dev/null || true)
        if [[ -n "$running_sha" && "$running_sha" == "$expected_sha" ]]; then
            continue
        fi
        info "Stopping stale ${label} (pid ${pid}) so the new install starts cleanly"
        kill "${pid}" 2>/dev/null || true
        for _ in 1 2 3 4 5 6 7 8 9 10; do
            [[ ! -d "/proc/${pid}" ]] && break
            sleep 0.2
        done
        if [[ -d "/proc/${pid}" ]]; then
            kill -9 "${pid}" 2>/dev/null || true
        fi
    done < <(python3 - "$binary" "$@" <<'PY'
import os
import sys

binary = os.path.realpath(sys.argv[1])
expected_args = sys.argv[2:]

for pid in os.listdir("/proc"):
    if not pid.isdigit():
        continue
    try:
        raw = open(f"/proc/{pid}/cmdline", "rb").read().split(b"\0")
    except Exception:
        continue
    raw = [item.decode("utf-8", "ignore") for item in raw if item]
    if not raw:
        continue
    try:
        exe = os.path.realpath(raw[0])
    except Exception:
        continue
    if exe != binary:
        continue
    if raw[1:1 + len(expected_args)] == expected_args:
        print(pid)
PY
    )
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

    if [[ "$ALLOW_UNSIGNED" = true ]]; then
        warn "Skipping signature verification (--allow-unsigned)"
        return 0
    fi

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
    info "Signature verified"
}

validate_release_identity() {
    # Both envelopes are verified before this check. A publisher URL can serve
    # documents from different publications while its files are being replaced.
    if ! python3 - "$1" "$2" <<'PY_RELEASE_IDENTITY'
import json
import sys

try:
    with open(sys.argv[1], encoding="utf-8") as source:
        head = json.load(source)["payload"]
    with open(sys.argv[2], encoding="utf-8") as source:
        release = json.load(source)["payload"]
    if head.get("schema") != "elastos.release.head/v1" or release.get("schema") != "elastos.release/v1":
        raise ValueError("Unexpected release schema")
    for field in ("version", "channel"):
        value = head.get(field)
        if not isinstance(value, str) or not value or release.get(field) != value:
            raise ValueError("Release head and release " + field + " must match")
except (ValueError, TypeError, KeyError, OSError, AttributeError) as error:
    print("Release identity check failed: " + str(error), file=sys.stderr)
    sys.exit(1)
PY_RELEASE_IDENTITY
    then
        die "Release schema, version or channel mismatch; retry after the publisher finishes updating"
    fi
}

# ── Parse args ────────────────────────────────────────────────────────

ALLOW_UNSIGNED=false
INSTALL_DIR="${HOME}/.local/bin"

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
        --allow-unsigned) ALLOW_UNSIGNED=true; shift ;;
        --install-dir)
            [[ -z "${2:-}" ]] && die "Usage: --install-dir PATH"
            INSTALL_DIR="$2"; shift 2 ;;
        *) die "Unknown option: $1. Run --help for usage." ;;
    esac
done

detect_platform
info "Platform: ${PLATFORM}"

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

if [[ "$ALLOW_UNSIGNED" != true && -z "$MAINTAINER_DID" ]]; then
    die "MAINTAINER_DID not set. Either:\n  1. Set ELASTOS_MAINTAINER_DID env var, or\n  2. Pass --maintainer-did <did:key:...>.\n  For unsigned install: pass --allow-unsigned"
fi

# ── Preflight ─────────────────────────────────────────────────────────

for cmd in curl python3; do
    command -v "$cmd" &>/dev/null || die "Required tool not found: $cmd"
done

if ! command -v sha256sum &>/dev/null && ! command -v shasum &>/dev/null; then
    die "Neither sha256sum nor shasum found"
fi

echo ""
echo -e "${BOLD}ElastOS Installer${NC}"
echo ""

# ── Fetch + verify release head ──────────────────────────────────────

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
info "Version: ${RELEASE_VERSION}"

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
COMPONENTS_CID=$(json_get "${TMPDIR}/release.json" "d['payload']['platforms']['${PLATFORM}']['components']['cid']")
COMPONENTS_SHA256=$(json_get "${TMPDIR}/release.json" "d['payload']['platforms']['${PLATFORM}']['components']['sha256']")

if [[ -z "$BINARY_CID" ]]; then
    AVAILABLE=$(json_get "${TMPDIR}/release.json" "', '.join(d['payload'].get('platforms',{}).keys())")
    die "No release available for platform: ${PLATFORM}\n  Available: ${AVAILABLE:-none}"
fi

# ── Download + verify binary ─────────────────────────────────────────

if [[ -n "$PUBLISHER_GATEWAY" ]]; then
    info "Downloading binary from bootstrap publisher URL"
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
    info "Downloading binary by CID: ${BINARY_CID} (bootstrap mode)"
    ipfs_fetch "$BINARY_CID" "${TMPDIR}/elastos"
fi

info "Verifying binary SHA-256..."
sha256_check "${TMPDIR}/elastos" "$BINARY_SHA256"

# ── Download + verify components.json ────────────────────────────────

if [[ -n "$PUBLISHER_GATEWAY" ]]; then
    info "Downloading components.json from bootstrap publisher URL"
    curl -fsSL --max-time 30 -o "${TMPDIR}/components.json" "${PG}/artifacts/components-${PLATFORM}.json" \
        || die "Failed to download components from ${PG}/artifacts/components-${PLATFORM}.json"
else
    info "Downloading components.json by CID: ${COMPONENTS_CID} (bootstrap mode)"
    ipfs_fetch "$COMPONENTS_CID" "${TMPDIR}/components.json"
fi

info "Verifying components.json SHA-256..."
sha256_check "${TMPDIR}/components.json" "$COMPONENTS_SHA256"

# ── Install (2 files) ────────────────────────────────────────────────

info "Installing binary to ${INSTALL_DIR}/elastos..."
mkdir -p "$INSTALL_DIR"
TMP_INSTALL_BIN="${INSTALL_DIR}/.elastos.install.tmp"
cp "${TMPDIR}/elastos" "${TMP_INSTALL_BIN}"
chmod +x "${TMP_INSTALL_BIN}"
mv -f "${TMP_INSTALL_BIN}" "${INSTALL_DIR}/elastos"

INSTALLED_VERSION_OUTPUT="$("${INSTALL_DIR}/elastos" --version 2>&1 || true)"
if ! printf '%s' "${INSTALLED_VERSION_OUTPUT}" | grep -Fq "${RELEASE_VERSION}"; then
    die "Installed binary version mismatch at ${INSTALL_DIR}/elastos\n  Expected: ${RELEASE_VERSION}\n  Got:      ${INSTALLED_VERSION_OUTPUT:-<no output>}"
fi

DATA_DIR="${XDG_DATA_HOME:-${HOME}/.local/share}/elastos"
mkdir -p "$DATA_DIR"

# Evict stale cached capsules when components.json changes (CID mismatch).
# This forces the supervisor to re-download updated capsule binaries on demand.
OLD_COMPONENTS="${DATA_DIR}/components.json"
if [[ -f "$OLD_COMPONENTS" ]]; then
    CHANGED_CAPSULES=$(python3 - "$OLD_COMPONENTS" "${TMPDIR}/components.json" <<'PY'
import json, sys
try:
    old = json.load(open(sys.argv[1]))
    new = json.load(open(sys.argv[2]))
    for name, entry in new.get("capsules", {}).items():
        old_entry = old.get("capsules", {}).get(name, {})
        if old_entry.get("cid") != entry.get("cid"):
            print(name)
except Exception:
    pass
PY
    )
    CAPSULE_CACHE="${DATA_DIR}/capsules"
    for cname in $CHANGED_CAPSULES; do
        if [[ -d "${CAPSULE_CACHE}/${cname}" ]]; then
            info "Evicting stale capsule cache: ${cname}"
            rm -rf "${CAPSULE_CACHE}/${cname}"
        fi
    done
fi

info "Installing components.json to ${DATA_DIR}/..."
cp "${TMPDIR}/components.json" "${DATA_DIR}/components.json"

stop_stale_runtime_if_needed "${DATA_DIR}/runtime-coords.json" "runtime" "${BINARY_SHA256}"
stop_stale_runtime_if_needed "${DATA_DIR}/home-runtime-coords.json" "Home runtime" "${BINARY_SHA256}"
stop_stale_installed_elastos_processes "Room gateway" "${BINARY_SHA256}" room open
stop_stale_installed_elastos_processes "gateway" "${BINARY_SHA256}" gateway

PRINCIPAL_ROOT_BACKUP_DIR="${DATA_DIR}/backups/principal-root-upgrade-$(date -u +%s)-$$"
info "Verifying and upgrading configured protected roots while Runtime is stopped..."
"${INSTALL_DIR}/elastos" principal-root-upgrade \
    --data-dir "${DATA_DIR}" \
    --backup-dir "${PRINCIPAL_ROOT_BACKUP_DIR}"

# ── Save Carrier contact + release metadata for `elastos upgrade` ────

refresh_source_bootstrap_from_publisher

SIGNER_DID=$(json_get "${TMPDIR}/release-head.json" 'd["signer_did"]')

SOURCES_PATH="${DATA_DIR}/sources.json"
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
info "Saved trusted source config to ${DATA_DIR}/sources.json"

PUBLISHER_ROOT="${DATA_DIR}/ElastOS/SystemServices/Publisher"
mkdir -p "${PUBLISHER_ROOT}"
cp "${TMPDIR}/release-head.json" "${PUBLISHER_ROOT}/release-head.json"
cp "${TMPDIR}/release.json" "${PUBLISHER_ROOT}/release.json"
info "Saved publisher metadata for future upgrades"

# ── Guest-network compatibility mode (optional) ─────────────────────
# Normal app capsules (chat, Documents, etc.) are Carrier-only and rootless.
# CAP_NET_ADMIN belongs only to explicit guest-network capsules, mediated by
# the runtime. Do NOT print sudo suggestions for normal installs.

# ── Done ──────────────────────────────────────────────────────────────

echo ""
echo -e "${GREEN}${BOLD}ElastOS ${RELEASE_VERSION} installed!${NC}"
echo ""
echo -e "  ${INSTALL_DIR}/elastos"
echo ""

if ! echo "$PATH" | tr ':' '\n' | grep -qx "$INSTALL_DIR"; then
    echo -e "  Add to your PATH:"
    echo ""
    echo "    export PATH=\"${INSTALL_DIR}:\$PATH\""
    echo ""
fi

echo -e "  Setup home:     elastos setup"
echo -e "  Open Home:      elastos"
echo -e "  Check source:   elastos source show"
echo -e "  Check updates:  elastos update --check"
echo -e "  Optional chat:  elastos chat --nick $(whoami)"
echo -e "  Full help:      elastos --help"
echo ""
