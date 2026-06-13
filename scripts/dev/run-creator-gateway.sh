#!/usr/bin/env bash
#
# Bring up the Create-portal gateway on macOS/Linux, ready to MINT (media + non-media).
#
# This is the one-command operator path for testing the dDRM Create flow end to end. It:
#   1. checks the external tools the spine needs (ffmpeg/ffprobe for media, kubo/ipfs for content);
#   2. builds every provider binary the mint spine calls, with the right features;
#   3. provisions a persistent 2-of-3 dKMS quorum descriptor the Create portal seals to;
#   4. exports the per-binary dev overrides (ELASTOS_<NAME>_BIN) so the gateway trusts the
#      locally-built capsules WITHOUT an installed signed manifest (macOS has none);
#   5. launches `elastos gateway` at 127.0.0.1:8090.
#
# Then open http://localhost:8090/apps/home/ (NOT 127.0.0.1 — WebAuthn rejects bare IPs),
# sign in, and use the Create portal. Minting seals to the quorum (pure local crypto, no node
# need run) and hands MetaMask an UNSIGNED mint for YOU to sign — the runtime never signs.
#
# Usage:  scripts/dev/run-creator-gateway.sh [--addr 127.0.0.1:8090] [--data-dir DIR]
# Env passthrough honored: ELASTOS_DDRM_RIGHTS (dev|chain-mock|chain), ELASTOS_DDRM_CHAIN_ID, etc.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CAPSULES="${REPO_ROOT}/capsules"
ADDR="127.0.0.1:8090"
DATA_DIR=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --addr) ADDR="${2:-}"; shift 2 ;;
    --addr=*) ADDR="${1#*=}"; shift ;;
    --data-dir) DATA_DIR="${2:-}"; shift 2 ;;
    --data-dir=*) DATA_DIR="${1#*=}"; shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

default_data_dir() {
  case "$(uname -s)" in
    Darwin) echo "${HOME}/Library/Application Support/elastos" ;;
    *)      echo "${ELASTOS_DATA_DIR:-${HOME}/.elastos}" ;;
  esac
}
[[ -z "$DATA_DIR" ]] && DATA_DIR="$(default_data_dir)"

echo "=============================================================="
echo " ElastOS Create-portal gateway — build + provision + launch"
echo "=============================================================="
echo "repo:     ${REPO_ROOT}"
echo "data dir: ${DATA_DIR}"
echo "addr:     ${ADDR}"
echo

# ── 1. external tools ─────────────────────────────────────────────────────────
missing_tool=0
if ! command -v ffmpeg >/dev/null 2>&1 || ! command -v ffprobe >/dev/null 2>&1; then
  echo "WARN: ffmpeg/ffprobe not on PATH — MEDIA (video/audio) mint will fail. Install: brew install ffmpeg"
  missing_tool=1
fi
if ! command -v ipfs >/dev/null 2>&1; then
  echo "WARN: kubo (ipfs) not on PATH — content publish will fail (no asset/metadata CID). Install: brew install ipfs"
  missing_tool=1
fi
[[ $missing_tool -eq 0 ]] && echo "external tools OK (ffmpeg, ffprobe, ipfs)"
echo

# ── 2. build the provider binaries + the gateway ──────────────────────────────
build_capsule() {
  local pkg="$1"; shift
  echo "building ${pkg} $* ..."
  if ! cargo build --quiet --manifest-path "${CAPSULES}/${pkg}/Cargo.toml" "$@"; then
    echo "FAIL: could not build ${pkg}" >&2
    exit 1
  fi
}

# Mint spine: encrypt (escrow seal), media (DASH package), publish (unsigned mint), chain
# (Base calldata), wallet (owner-signature request), ipfs (Kubo-backed content plane).
build_capsule encrypt-provider --features escrow
build_capsule media-provider
build_capsule publish-provider
build_capsule chain-provider
build_capsule wallet-provider
build_capsule ipfs-provider
# Playback rail (for opening a minted asset later): rights gate + decrypt boundary + the
# local media-authority helper. Not needed to MINT, built so the open path is ready too.
build_capsule rights-provider --features chain-rights
build_capsule decrypt-provider --features rail-stream,rail-mint
# Library object plane (v0.4.0): the provider-backed object model the new Library/Home use.
# Without it the gateway boots but Library object operations fail closed.
build_capsule object-provider

echo "building media-authority helper ..."
cargo build --quiet --manifest-path "${REPO_ROOT}/scripts/dev/ddrm-media-authority/Cargo.toml" \
  || echo "WARN: media-authority helper build failed (owned-video playback seam) — mint still works"

echo "building the gateway (elastos-server) ..."
if ! cargo build --quiet --manifest-path "${REPO_ROOT}/elastos/Cargo.toml" -p elastos-server; then
  echo "FAIL: could not build elastos-server" >&2
  exit 1
fi
GATEWAY_BIN="${REPO_ROOT}/elastos/target/debug/elastos"
[[ -x "$GATEWAY_BIN" ]] || { echo "FAIL: gateway binary missing at ${GATEWAY_BIN}" >&2; exit 1; }
echo

# ── 3. provision the persistent 2-of-3 quorum (the Create portal seals to it) ──
QUORUM_JSON="${DATA_DIR}/dkms/quorum.json"
if [[ -f "$QUORUM_JSON" ]]; then
  echo "quorum descriptor present: ${QUORUM_JSON} (reusing)"
else
  echo "provisioning a fresh 2-of-3 quorum into ${DATA_DIR}/dkms ..."
  bash "${REPO_ROOT}/scripts/dev/ddrm-provision-quorum.sh" "${DATA_DIR}/dkms" || {
    echo "FAIL: quorum provisioning failed" >&2; exit 1; }
fi
echo

# ── 4. dev overrides: point the gateway at the locally-built capsules ─────────
# find_installed_provider_binary checks ELASTOS_<NAME>_BIN first, and verify_provider_binary
# bypasses the signed-manifest check when that env points at the exact path (explicit dev trust).
cap_bin() { echo "${CAPSULES}/$1/target/debug/$1"; }
export ELASTOS_ENCRYPT_PROVIDER_BIN="$(cap_bin encrypt-provider)"
export ELASTOS_MEDIA_PROVIDER_BIN="$(cap_bin media-provider)"
export ELASTOS_PUBLISH_PROVIDER_BIN="$(cap_bin publish-provider)"
export ELASTOS_CHAIN_PROVIDER_BIN="$(cap_bin chain-provider)"
export ELASTOS_WALLET_PROVIDER_BIN="$(cap_bin wallet-provider)"
export ELASTOS_IPFS_PROVIDER_BIN="$(cap_bin ipfs-provider)"
export ELASTOS_RIGHTS_PROVIDER_BIN="$(cap_bin rights-provider)"
# decrypt-provider in two roles: the registered sub-provider (ELASTOS_DECRYPT_PROVIDER_BIN,
# resolved by server_infra) AND the media_authority playback rail (ELASTOS_DDRM_DECRYPT_BIN,
# spawned directly by the open route). Same binary, two lookups — set both.
export ELASTOS_DECRYPT_PROVIDER_BIN="$(cap_bin decrypt-provider)"
export ELASTOS_DDRM_DECRYPT_BIN="$(cap_bin decrypt-provider)"
# v0.4.0 Library object plane.
export ELASTOS_OBJECT_PROVIDER_BIN="$(cap_bin object-provider)"
MEDIA_AUTH_BIN="${REPO_ROOT}/scripts/dev/ddrm-media-authority/target/debug/ddrm-media-authority"
[[ -x "$MEDIA_AUTH_BIN" ]] && export ELASTOS_DDRM_MEDIA_AUTHORITY_BIN="$MEDIA_AUTH_BIN"
# The Create portal reads the quorum here (also the default <data_dir>/dkms/quorum.json).
export ELASTOS_DKMS_QUORUM_DESCRIPTOR="$QUORUM_JSON"
# Default the rights/mint mode to dev (offline) unless the operator pinned one.
export ELASTOS_DDRM_RIGHTS="${ELASTOS_DDRM_RIGHTS:-dev}"

echo "provider overrides:"
for v in ELASTOS_ENCRYPT_PROVIDER_BIN ELASTOS_MEDIA_PROVIDER_BIN ELASTOS_PUBLISH_PROVIDER_BIN \
         ELASTOS_CHAIN_PROVIDER_BIN ELASTOS_WALLET_PROVIDER_BIN ELASTOS_IPFS_PROVIDER_BIN \
         ELASTOS_RIGHTS_PROVIDER_BIN ELASTOS_DDRM_DECRYPT_BIN ELASTOS_OBJECT_PROVIDER_BIN; do
  printf '  %s=%s\n' "$v" "${!v}"
done
echo "  ELASTOS_DDRM_RIGHTS=${ELASTOS_DDRM_RIGHTS}"
echo

# ── 5. free the port / host lock, then launch ─────────────────────────────────
PORT="${ADDR##*:}"
if lsof -nP -iTCP:"${PORT}" -sTCP:LISTEN >/dev/null 2>&1; then
  echo "WARN: ${PORT} is in use — the ElastOS desktop app or a prior gateway may hold the host lock."
  echo "      Quit ElastOS.app (osascript -e 'quit app \"ElastOS\"') and/or: lsof -ti tcp:${PORT} | xargs kill"
fi

echo "=============================================================="
echo " launching gateway — open http://localhost:${PORT}/apps/home/"
echo " (use 'localhost', NOT 127.0.0.1 — WebAuthn rejects bare IPs)"
echo "=============================================================="
# The `gateway` command uses the platform default data dir (dirs::data_dir()/elastos); the
# Create portal finds the quorum via ELASTOS_DKMS_QUORUM_DESCRIPTOR regardless of --data-dir.
exec "$GATEWAY_BIN" gateway --addr "$ADDR"
