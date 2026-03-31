#!/usr/bin/env bash
#
# ElastOS Installer — explicit web bootstrap, then Carrier-backed updates/setup
#
# Usage:
#   curl -fsSL https://<publisher-origin>/install.sh | bash
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
# Fails closed: if trust anchors are missing or OpenSSL doesn't support
# Ed25519 and --allow-unsigned is not passed, the installer exits.
#
# Dependencies: curl, python3, openssl (1.1.1+ for Ed25519), sha256sum|shasum
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
    if command -v sha256sum &>/dev/null; then
        actual=$(sha256sum "$file" | cut -d' ' -f1)
    elif command -v shasum &>/dev/null; then
        actual=$(shasum -a 256 "$file" | cut -d' ' -f1)
    else
        die "Neither sha256sum nor shasum found"
    fi
    if [[ "$actual" != "$expected" ]]; then
        die "SHA-256 mismatch!\n  Expected: ${expected}\n  Got:      ${actual}"
    fi
}

# Fetch a CID from IPFS gateways (tries each in order)
ipfs_fetch() {
    local cid="$1"
    local output="$2"
    local url
    for gw in "${GATEWAYS[@]}"; do
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
    python3 -c "
import json, sys
with open('${file}', 'r') as f:
    d = json.load(f)
try:
    v = ${expr}
    if v is None:
        sys.exit(0)
    print(v)
except (KeyError, TypeError, IndexError):
    sys.exit(0)
"
}

# Compact JSON payload (equivalent to jq -c '.payload')
json_payload() {
    local file="$1"
    python3 -c "
import json
with open('${file}', 'r') as f:
    d = json.load(f)
print(json.dumps(d['payload'], separators=(',', ':')))"
}

# Canonical sorted JSON (equivalent to jq -cS .)
json_canonical() {
    python3 -c "
import json, sys
d = json.load(sys.stdin)
print(json.dumps(d, separators=(',', ':'), sort_keys=True))"
}

# ── Ed25519 verification ─────────────────────────────────────────────

has_ed25519() {
    if openssl list -public-key-algorithms 2>/dev/null | grep -qi "ED25519"; then
        return 0
    fi
    openssl genpkey -algorithm ED25519 -out /dev/null >/dev/null 2>&1
}

decode_did_to_hex() {
    local did="$1"
    local multibase="${did#did:key:z}"
    [[ "$multibase" == "$did" ]] && die "Invalid DID format: $did"

    local raw_hex
    if command -v python3 &>/dev/null; then
        raw_hex=$(python3 -c "
import sys
ALPHABET = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz'
def b58decode(s):
    n = 0
    for c in s:
        n = n * 58 + ALPHABET.index(c)
    pad = len(s) - len(s.lstrip('1'))
    result = []
    while n > 0:
        result.append(n & 0xff)
        n >>= 8
    return bytes(pad) + bytes(reversed(result))
raw = b58decode('${multibase}')
if len(raw) != 34 or raw[0] != 0xed or raw[1] != 0x01:
    print('ERROR', file=sys.stderr)
    sys.exit(1)
print(raw[2:].hex())
") || die "Failed to decode DID"
    elif command -v perl &>/dev/null; then
        raw_hex=$(perl -e '
my @alpha = split //, "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
my %val; $val{$alpha[$_]} = $_ for 0..57;
my $s = "'"${multibase}"'";
use Math::BigInt;
my $n = Math::BigInt->new(0);
$n = $n * 58 + $val{$_} for split //, $s;
my $hex = $n->as_hex(); $hex =~ s/^0x//;
$hex = "0" . $hex if length($hex) % 2;
while (length($hex) < 68) { $hex = "00" . $hex; }
die "Bad multicodec" unless substr($hex, 0, 4) eq "ed01";
print substr($hex, 4);
') || die "Failed to decode DID"
    else
        die "Need python3 or perl for base58 decoding"
    fi

    echo "$raw_hex"
}

verify_signature() {
    local json_file="$1"
    local domain="$2"
    local expected_did="$3"

    if [[ "$ALLOW_UNSIGNED" = true ]]; then
        warn "Skipping signature verification (--allow-unsigned)"
        return 0
    fi

    if ! has_ed25519; then
        die "OpenSSL does not support Ed25519 on this system.\n  Install OpenSSL 1.1.1+ or pass --allow-unsigned (NOT recommended)."
    fi

    command -v xxd &>/dev/null || die "xxd required for signature verification"

    local payload sig_hex signer_did payload_signer_did
    payload=$(json_payload "$json_file")
    sig_hex=$(json_get "$json_file" 'd["signature"]')
    signer_did=$(json_get "$json_file" 'd["signer_did"]')
    payload_signer_did=$(json_get "$json_file" 'd.get("payload",{}).get("signer_did","")')

    if [[ "$signer_did" != "$expected_did" ]]; then
        die "Signer mismatch!\n  Expected: ${expected_did}\n  Got:      ${signer_did}"
    fi
    if [[ -n "$payload_signer_did" && "$payload_signer_did" != "$signer_did" ]]; then
        die "Payload/envelope signer mismatch!\n  Payload:  ${payload_signer_did}\n  Envelope: ${signer_did}"
    fi

    local canonical
    canonical=$(echo -n "$payload" | json_canonical)

    local digest_hex
    digest_hex=$(printf '%s\0%s' "$domain" "$canonical" | sha256sum | cut -d' ' -f1 2>/dev/null) \
        || digest_hex=$(printf '%s\0%s' "$domain" "$canonical" | shasum -a 256 | cut -d' ' -f1)

    local pubkey_hex
    pubkey_hex=$(decode_did_to_hex "$signer_did")

    local der_prefix="302a300506032b6570032100"
    local tmpdir
    tmpdir=$(mktemp -d)

    echo -n "${der_prefix}${pubkey_hex}" | xxd -r -p | \
        openssl pkey -inform DER -pubin -out "${tmpdir}/pubkey.pem" 2>/dev/null \
        || die "Failed to create PEM from public key"

    echo -n "$digest_hex" | xxd -r -p > "${tmpdir}/digest.bin"
    echo -n "$sig_hex" | xxd -r -p > "${tmpdir}/sig.bin"

    if openssl pkeyutl -verify -pubin -inkey "${tmpdir}/pubkey.pem" \
        -in "${tmpdir}/digest.bin" -sigfile "${tmpdir}/sig.bin" \
        -rawin 2>/dev/null; then
        rm -rf "$tmpdir"
        info "Signature verified"
    else
        rm -rf "$tmpdir"
        die "Signature verification FAILED"
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
            PUBLISHER_NODE_ID="$2"; shift 2 ;;
        --allow-unsigned) ALLOW_UNSIGNED=true; shift ;;
        --install-dir)
            [[ -z "${2:-}" ]] && die "Usage: --install-dir PATH"
            INSTALL_DIR="$2"; shift 2 ;;
        *) die "Unknown option: $1. Run --help for usage." ;;
    esac
done

if [[ ${#CLI_GATEWAYS[@]} -gt 0 ]]; then
    GATEWAYS=("${CLI_GATEWAYS[@]}")
elif [[ -n "${ELASTOS_IPFS_GATEWAYS:-}" ]]; then
    GATEWAYS=()
    IFS=', ' read -r -a ENV_GATEWAYS <<< "${ELASTOS_IPFS_GATEWAYS}"
    for gw in "${ENV_GATEWAYS[@]}"; do
        [[ -z "$gw" ]] && continue
        gw="${gw%/}"
        gw="${gw%/ipfs}"
        GATEWAYS+=("$gw")
    done
fi

# Prepend bootstrap publisher URL if available (it also serves /ipfs/<cid>/)
if [[ -n "$PUBLISHER_GATEWAY" ]]; then
    GATEWAYS=("${PUBLISHER_GATEWAY%/}" "${GATEWAYS[@]}")
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

if [[ "$ALLOW_UNSIGNED" != true ]]; then
    if ! has_ed25519; then
        die "OpenSSL does not support Ed25519 on this system.\n  Install OpenSSL 1.1.1+ or pass --allow-unsigned (NOT recommended).\n  Failing closed for your safety."
    fi
    command -v xxd &>/dev/null || die "xxd required for signature verification"
fi

echo ""
echo -e "${BOLD}ElastOS Installer${NC}"
echo ""

# ── Detect platform ──────────────────────────────────────────────────

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

info "Platform: ${PLATFORM}"

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

# ── Save Carrier contact + release metadata for `elastos upgrade` ────

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
# Normal app capsules (chat, notepad, etc.) are Carrier-only and rootless.
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
echo -e "  Open PC2 home:  elastos"
echo -e "  Check source:   elastos source show"
echo -e "  Check updates:  elastos update --check"
echo -e "  Optional chat:  elastos chat --nick $(whoami)"
echo -e "  Full help:      elastos --help"
echo ""
