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
build_capsule decrypt-provider --features rail-stream,rail-mint,pdf-render
# dKMS consumer-open rail: key-provider (dkms backend) recovers the CEK 2-of-3 from the live
# quorum; dkms-authority is the node daemon; dkms-keygen derives the caller-identity vk the
# nodes allow-list. Built so a minted dKMS asset opens (double-click -> quorum recover -> render).
#
# DEV_MODE_GUARD_SPEC: this is a LOCAL DEV harness, so the node + gateway are built with the
# `dev-modes` feature (legacy-receipt fallback + Dev/ChainMock rights for offline testing). A
# PRODUCTION deploy builds these WITHOUT `dev-modes` (chain rights + wallet-signed grants only).
# key-provider keeps `--features key-authority-ref` (NOT dev-modes): the production dkms quorum
# path is compiled while the forgeable `reference` backend stays fenced out at selection, so the
# local gateway exercises the SAME secure key-release posture the production build ships.
build_capsule key-provider --features key-authority-ref
build_capsule dkms-authority --features dev-modes
build_capsule dkms-keygen
# Library object plane (v0.4.0): the provider-backed object model the new Library/Home use.
# Without it the gateway boots but Library object operations fail closed.
build_capsule object-provider

# v0.4.0 browser app capsules need compiled wasm entrypoints (gitignored like home.wasm).
# Opening Library/Marketplace/etc. from Home spawns a managed runtime that loads <name>.wasm.
echo "building v0.4.0 app capsule wasm (wasm32-wasip1) ..."
for c in library documents inbox system marketplace archive-manager wallet wallet-metamask wallet-unisat wallet-walletconnect; do
  cj="${CAPSULES}/${c}/capsule.json"
  [[ -f "$cj" ]] || continue
  entry=$(grep -oE '"entrypoint"[^,]*' "$cj" | sed -E 's/.*"entrypoint"[^"]*"([^"]+)".*/\1/')
  [[ -n "$entry" ]] || continue
  echo "  ${c} -> ${entry}"
  cargo build --quiet --manifest-path "${CAPSULES}/${c}/Cargo.toml" --target wasm32-wasip1 --release \
    || { echo "FAIL: could not build ${c} wasm" >&2; exit 1; }
  out=$(find "${CAPSULES}/${c}/target/wasm32-wasip1/release" -maxdepth 1 -name '*.wasm' 2>/dev/null | head -1)
  [[ -n "$out" ]] && cp "$out" "${CAPSULES}/${c}/${entry}"
done

echo "building media-authority helper ..."
cargo build --quiet --manifest-path "${REPO_ROOT}/scripts/dev/ddrm-media-authority/Cargo.toml" \
  || echo "WARN: media-authority helper build failed (owned-video playback seam) — mint still works"

echo "building the gateway (elastos-server) ..."
# DEV_MODE_GUARD_SPEC: built with `dev-modes` so this local harness can run any rights mode
# (Dev/ChainMock for offline tests) without the fail-closed startup guard refusing to boot. The
# user's `ELASTOS_DDRM_RIGHTS=chain` happy path works the same; a PRODUCTION gateway builds
# WITHOUT `dev-modes` (then `chain` is the only selectable rights mode — the guard enforces it).
if ! cargo build --quiet --manifest-path "${REPO_ROOT}/elastos/Cargo.toml" -p elastos-server --features dev-modes; then
  echo "FAIL: could not build elastos-server" >&2
  exit 1
fi
GATEWAY_BIN="${REPO_ROOT}/elastos/target/debug/elastos"
[[ -x "$GATEWAY_BIN" ]] || { echo "FAIL: gateway binary missing at ${GATEWAY_BIN}" >&2; exit 1; }
echo

# ── 3. provision the quorum the Create portal seals to ────────────────────────
# Two modes:
#   * LOCAL (default): provision/run a persistent 2-of-3 quorum on this box.
#   * REMOTE (ELASTOS_DKMS_REMOTE=1): seal + recover against the LIVE 3 geo nodes
#     (dkms0 mesh, tcp:10.66.66.x:9443) using ~/.elastos-dkms/dkms-authority.v2.json
#     + ~/.elastos-dkms/secrets/caller.seed. No local daemons are started — this
#     validates the real transport/crypto end-to-end against the sovereign quorum.
#     (This Mac must be on the dkms0 mesh to reach those endpoints.)
REMOTE="${ELASTOS_DKMS_REMOTE:-0}"
# Transport to the LIVE quorum, when validating against the real geo nodes (ELASTOS_DKMS_REMOTE=1):
#   * CARRIER (ELASTOS_DKMS_CARRIER=1) — PRODUCTION: reach each node by its `carrier:did:key:...`
#     identity over Carrier (iroh); a local `dkms-carrier-client` sidecar dials/relays/hole-punches.
#     NO WireGuard, NO dkms0 VPN, NO per-node enrollment. This is the path the geo nodes run bridges
#     for and the one we develop against.
#   * LEGACY WireGuard mesh (CARRIER unset) — DEPRECATED ROLLBACK ONLY: reach the nodes over
#     tcp:10.66.66.x:9443 on the dkms0 mesh; requires this Mac on the VPN. Kept as a fallback while
#     Carrier is proven; slated for removal. Prefer Carrier.
# (The zero-config default below is LOCAL — a self-contained 2-of-3 quorum on this box, no remote
# files needed; set ELASTOS_DKMS_CARRIER=1 to validate end-to-end against the live nodes.)
CARRIER="${ELASTOS_DKMS_CARRIER:-0}"
CARRIER_CLIENT_BIN="${REPO_ROOT}/scripts/dev/dkms-carrier-client/target/debug/dkms-carrier-client"
CARRIER_CLIENT_ADDR="${DKMS_CARRIER_CLIENT_ADDR:-127.0.0.1:9444}"
CARRIER_PID=""
[[ "$CARRIER" == "1" ]] && REMOTE=1
QUORUM_DIR="${DATA_DIR}/dkms"
QUORUM_JSON="${QUORUM_DIR}/quorum.json"
QUORUM_NODES="${QUORUM_DIR}/quorum-nodes.json"
OPEN_DESCRIPTOR="${QUORUM_DIR}/quorum-open.json"
KEY_PROVIDER_BIN="${CAPSULES}/key-provider/target/debug/key-provider"
DKMS_PIDS=()
QUORUM_OPEN_READY=0

if [[ "$REMOTE" == "1" ]]; then
  mkdir -p "$QUORUM_DIR" 2>/dev/null || true
  if [[ "$CARRIER" == "1" ]]; then
    REMOTE_DESC="${ELASTOS_DKMS_REMOTE_DESCRIPTOR:-${HOME}/.elastos-dkms/dkms-authority.carrier.json}"
  else
    REMOTE_DESC="${ELASTOS_DKMS_REMOTE_DESCRIPTOR:-${HOME}/.elastos-dkms/dkms-authority.v2.json}"
  fi
  REMOTE_SEED="${ELASTOS_DKMS_REMOTE_CALLER_SEED:-${HOME}/.elastos-dkms/secrets/caller.seed}"
  [[ -f "$REMOTE_DESC" ]] || { echo "FAIL: ELASTOS_DKMS_REMOTE=1 but descriptor missing: $REMOTE_DESC" >&2; exit 1; }
  [[ -f "$REMOTE_SEED" ]] || { echo "FAIL: ELASTOS_DKMS_REMOTE=1 but caller seed missing: $REMOTE_SEED" >&2; exit 1; }
  QUORUM_JSON="$REMOTE_DESC"       # the Create portal seals CEK shares to the live recipients
  OPEN_DESCRIPTOR="$REMOTE_DESC"   # the open path recovers 2-of-3 over the live endpoints
  CALLER_SEED_B64="$(tr -d '\r\n' < "$REMOTE_SEED")"
  QUORUM_OPEN_READY=1
  # Remote validation is the live-rail proof; default the rights gate to live on-chain (PC2 parity).
  export ELASTOS_DDRM_RIGHTS="${ELASTOS_DDRM_RIGHTS:-chain}"
  echo "dKMS REMOTE quorum — sealing + recovering against the LIVE 3 geo nodes:"
  python3 - "$REMOTE_DESC" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
th = d.get("threshold", {})
for i, n in enumerate(th.get("nodes", [])):
    print(f"    node{i}: {n.get('authority_endpoint')}")
print(f"    threshold: t={th.get('t')} n={len(th.get('nodes', []))}")
PY
  echo "  descriptor: ${REMOTE_DESC}"
  if [[ "$CARRIER" == "1" ]]; then
    # Bring up the local Carrier sidecar: key-provider dials each node's did:key through it
    # (relay/hole-punch, zero VPN). No dkms0 mesh required.
    echo "building dkms-carrier-client sidecar ..."
    if ! cargo build --quiet --manifest-path "${REPO_ROOT}/scripts/dev/dkms-carrier-client/Cargo.toml"; then
      echo "FAIL: could not build dkms-carrier-client" >&2; exit 1
    fi
    [[ -x "$CARRIER_CLIENT_BIN" ]] || { echo "FAIL: dkms-carrier-client missing at ${CARRIER_CLIENT_BIN}" >&2; exit 1; }
    # Pre-warm: hand the sidecar the node did:keys so it dials + keeps the quorum connections hot
    # from startup — the first open lands on a warm path instead of paying a cold cross-continent
    # dial per recover (the cause of the ~30s first open).
    WARM_DIDS="$(python3 - "$REMOTE_DESC" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
eps = [n.get("authority_endpoint", "") for n in d.get("threshold", {}).get("nodes", [])]
print(" ".join(e for e in eps if e))
PY
)"
    DKMS_CARRIER_CLIENT_LISTEN="$CARRIER_CLIENT_ADDR" DKMS_CARRIER_WARM_DIDS="$WARM_DIDS" "$CARRIER_CLIENT_BIN" \
      >"${QUORUM_DIR}/carrier-client.out" 2>"${QUORUM_DIR}/carrier-client.err" &
    CARRIER_PID="$!"
    for _ in $(seq 1 60); do grep -q '^ready' "${QUORUM_DIR}/carrier-client.out" 2>/dev/null && break; sleep 0.5; done
    if grep -q '^ready' "${QUORUM_DIR}/carrier-client.out" 2>/dev/null; then
      export DKMS_CARRIER_CLIENT_ADDR="$CARRIER_CLIENT_ADDR"
      echo "  Carrier transport LIVE — sidecar on ${CARRIER_CLIENT_ADDR}; key-provider reaches nodes by did:key (NO WireGuard)."
    else
      echo "FAIL: dkms-carrier-client did not become ready" >&2
      tail -5 "${QUORUM_DIR}/carrier-client.err" 2>/dev/null
      exit 1
    fi
  else
    echo "  LEGACY WireGuard mesh transport (DEPRECATED — prefer ELASTOS_DKMS_CARRIER=1):"
    echo "    this Mac must be on the dkms0 mesh (10.66.66.0/24) to reach those tcp: endpoints."
  fi
  echo
elif [[ -f "$QUORUM_JSON" && -f "$QUORUM_NODES" ]]; then
  echo "quorum descriptor present: ${QUORUM_JSON} (reusing)"
  echo
else
  echo "provisioning a fresh 2-of-3 quorum into ${QUORUM_DIR} ..."
  bash "${REPO_ROOT}/scripts/dev/ddrm-provision-quorum.sh" "${QUORUM_DIR}" || {
    echo "FAIL: quorum provisioning failed" >&2; exit 1; }
  echo
fi

# ── 3b. start the recovery daemons + assemble the OPEN descriptor + caller seed ───────────────
# Minting only needs the PUBLIC descriptor (sealing is local crypto). OPENING a minted dKMS asset
# needs the three secret-holding nodes RUNNING (the 2-of-3 recover) + a caller seed the nodes
# allow-list + an OPEN descriptor that carries each node's live endpoint. We bring the nodes up on
# their durable stores (same identities the mint sealed to), allow-list a fresh caller seed, and
# emit a v2 descriptor (top-level node-0 identity + threshold.nodes[] with endpoints) that serves
# BOTH the mint seal AND the key-provider(dkms) recover.
# (LOCAL mode only — in REMOTE mode the live geo nodes are already running and the
#  OPEN descriptor + caller seed were resolved above.)
if [[ "$REMOTE" != "1" ]]; then
NODE_BIN="${CAPSULES}/dkms-authority/target/debug/dkms-authority"
KEYGEN_BIN="${CAPSULES}/dkms-keygen/target/debug/dkms-keygen"
KEY_PROVIDER_BIN="${CAPSULES}/key-provider/target/debug/key-provider"
OPEN_DESCRIPTOR="${QUORUM_DIR}/quorum-open.json"
DKMS_PIDS=()
QUORUM_OPEN_READY=0
if [[ -x "$NODE_BIN" && -x "$KEYGEN_BIN" && -x "$KEY_PROVIDER_BIN" && -f "$QUORUM_NODES" ]]; then
  # A fresh per-boot caller identity the nodes allow-list (its vk is the allow entry; the seed is
  # the runtime-side secret the key-provider authenticates with — never written to the descriptor).
  CALLER_SEED_B64="$(head -c 32 /dev/urandom | base64 | tr -d '\n')"
  CALLER_VK="$("$KEYGEN_BIN" derive-vk --seed-b64 "$CALLER_SEED_B64" 2>/dev/null | tr -d '\n')"
  if [[ -z "$CALLER_VK" ]]; then
    echo "WARN: could not derive a caller vk (dkms-keygen) — quorum OPEN disabled (mint still works)"
  else
    # Read the private sidecar (store path + endpoint per node) and start each daemon on its
    # durable store, listening on its endpoint, allow-listing this caller. (No `mapfile` — macOS
    # ships bash 3.2; read the lines portably.)
    NODE_LINES=()
    while IFS= read -r ln; do [[ -n "$ln" ]] && NODE_LINES+=("$ln"); done < <(python3 - "$QUORUM_NODES" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
for n in d.get("nodes", []):
    print("\t".join([n["verifying_key_b64"], n["recipient_pub_b64"], n["key_store"], n["authority_endpoint"]]))
PY
)
    if [[ "${#NODE_LINES[@]}" -ne 3 ]]; then
      echo "WARN: quorum sidecar did not yield 3 nodes — quorum OPEN disabled (mint still works)"
    else
      # TRUSTLESS node-side authz (chain mode): give each local daemon its OWN read-only Base
      # capability so a wallet-signed AccessGrantV1 is authorized by the node itself (verify the
      # wallet signature + read `hasAccessByContentId` from Base) — a faithful local proxy for the
      # sovereign quorum, no dkms0 mesh needed. The browser's MetaMask grant flow is only offered
      # in chain mode (prepare-grant 400s otherwise), so dev/chain-mock keep the enrolled-caller
      # path and these vars stay unset (NodeChain::from_env -> None).
      NODE_CHAIN_ENV=()
      if [[ "${ELASTOS_DDRM_RIGHTS:-dev}" == "chain" ]]; then
        NODE_CHAIN_ENV=(
          "DKMS_CHAIN_RPC_POOL=${DKMS_CHAIN_RPC_POOL:-${ELASTOS_CHAIN_BASE_RPC:-https://mainnet.base.org}}"
          "DKMS_RIGHTS_CONTRACT=${ELASTOS_DDRM_RIGHTS_CONTRACT:-0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D}"
          "DKMS_RIGHTS_SELECTOR=${ELASTOS_DDRM_RIGHTS_SELECTOR:-0x54d42821}"
          "DKMS_CHAIN_ID=${ELASTOS_DDRM_CHAIN_ID:-8453}"
        )
        echo "  node-side trustless authz ENABLED for local daemons (DKMS_CHAIN_RPC_POOL set; nodes read Base themselves)"
      fi
      VKS=(); RECS=(); ENDPOINTS=()
      for ln in "${NODE_LINES[@]}"; do
        IFS=$'\t' read -r vk rec store endpoint <<<"$ln"
        VKS+=("$vk"); RECS+=("$rec"); ENDPOINTS+=("$endpoint")
        # `${#arr[@]}` is set -u-safe on an empty array (bash 3.2); expanding "${arr[@]}" is
        # only reached in the non-empty branch, so we never hit `env ""`.
        if [[ "${#NODE_CHAIN_ENV[@]}" -gt 0 ]]; then
          env "${NODE_CHAIN_ENV[@]}" \
          DKMS_AUTHORITY_LISTEN="$endpoint" \
          DKMS_AUTHORITY_KEY_STORE="$store" \
          DKMS_AUTHORITY_ALLOWED_CALLERS="$CALLER_VK" \
            "$NODE_BIN" >/dev/null 2>>"${QUORUM_DIR}/daemon.log" &
        else
          DKMS_AUTHORITY_LISTEN="$endpoint" \
          DKMS_AUTHORITY_KEY_STORE="$store" \
          DKMS_AUTHORITY_ALLOWED_CALLERS="$CALLER_VK" \
            "$NODE_BIN" >/dev/null 2>>"${QUORUM_DIR}/daemon.log" &
        fi
        DKMS_PIDS+=("$!")
      done
      # Wait for each unix socket to appear (the node clears+binds it on startup).
      for ep in "${ENDPOINTS[@]}"; do
        for _ in $(seq 1 200); do [[ -S "$ep" ]] && break; sleep 0.05; done
      done
      # Assemble the v2 OPEN descriptor (identities the mint sealed to + live endpoints).
      python3 - "$OPEN_DESCRIPTOR" "${VKS[@]}" "__SEP__" "${RECS[@]}" "__SEP__" "${ENDPOINTS[@]}" <<'PY'
import json, sys
args = sys.argv[1:]
out = args[0]; rest = args[1:]
groups, cur = [], []
for a in rest:
    if a == "__SEP__": groups.append(cur); cur = []
    else: cur.append(a)
groups.append(cur)
vks, recs, eps = groups
nodes = [{"verifying_key_b64": v, "recipient_pub_b64": r, "authority_endpoint": e}
         for v, r, e in zip(vks, recs, eps)]
desc = {
  "schema": "elastos.dkms.authority/v2",
  "verifying_key_b64": nodes[0]["verifying_key_b64"],
  "recipient_pub_b64": nodes[0]["recipient_pub_b64"],
  "authority_endpoint": nodes[0]["authority_endpoint"],
  "threshold": {"t": 2, "n": len(nodes), "nodes": nodes},
}
json.dump(desc, open(out, "w"), indent=2)
PY
      if [[ -f "$OPEN_DESCRIPTOR" ]]; then
        chmod 600 "$OPEN_DESCRIPTOR" 2>/dev/null || true
        QUORUM_OPEN_READY=1
        echo "dKMS recovery quorum LIVE — 3 node daemons up; OPEN descriptor: ${OPEN_DESCRIPTOR}"
      else
        echo "WARN: could not assemble the OPEN descriptor — quorum OPEN disabled (mint still works)"
      fi
    fi
  fi
else
  echo "WARN: key-provider/dkms-authority/dkms-keygen not built — quorum OPEN disabled (mint still works)"
fi
fi  # end LOCAL-quorum bring-up (skipped when ELASTOS_DKMS_REMOTE=1)
echo

# ── 4. dev overrides: point the gateway at the locally-built capsules ─────────
# find_installed_provider_binary checks ELASTOS_<NAME>_BIN first, and verify_provider_binary
# bypasses the signed-manifest check when that env points at the exact path (explicit dev trust).
cap_bin() { echo "${CAPSULES}/$1/target/debug/$1"; }
export ELASTOS_ENCRYPT_PROVIDER_BIN="$(cap_bin encrypt-provider)"
export ELASTOS_MEDIA_PROVIDER_BIN="$(cap_bin media-provider)"
# media-provider transcodes -> DASH (video/audio mint AND owned audio/video playback) and needs
# ffmpeg + a scratch dir. Wire its narrow operator config from the ffmpeg already checked on PATH
# above; the spawned media-provider inherits this env. ffprobe defaults to ffmpeg's sibling.
# Unset ⇒ the provider's `package` op fails closed ("ffmpeg_path not configured").
if command -v ffmpeg >/dev/null 2>&1; then
  MEDIA_SCRATCH="${DATA_DIR}/media-scratch"
  mkdir -p "$MEDIA_SCRATCH"
  export ELASTOS_MEDIA_PROVIDER_CONFIG="{\"ffmpeg_path\":\"$(command -v ffmpeg)\",\"scratch_dir\":\"${MEDIA_SCRATCH}\"}"
fi
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
# Opening an app from Home spawns a managed child runtime (`elastos serve`) that
# fail-closed VERIFIES every installed provider. On macOS the platform key resolves
# to "unknown-arm64", which has no manifest entry, so verification rejects providers
# that have no Rust source crate here (localhost/did/site/tunnel/webspace). These are
# genuine Mach-O arm64 binaries from the install; point the per-name override at them
# so the managed runtime trusts them WITHOUT manifest verification (explicit dev trust).
# The child runtime inherits this env (runtime_control spawns serve without env_clear).
INSTALLED_BIN="${DATA_DIR}/bin"
for p in localhost did site tunnel webspace; do
  bin_path="${INSTALLED_BIN}/${p}-provider"
  if [[ -x "$bin_path" ]]; then
    export "ELASTOS_$(echo "${p}_provider" | tr '[:lower:]-' '[:upper:]_')_BIN=$bin_path"
  fi
done
# The managed home runtime also hosts the desktop 'shell' capsule, which it verifies
# via verify_component_binary (same unknown-arm64 manifest miss). Point ELASTOS_SHELL_BIN
# at the installed Mach-O arm64 shell so the dev-override bypass trusts it.
[[ -x "${INSTALLED_BIN}/shell" ]] && export ELASTOS_SHELL_BIN="${INSTALLED_BIN}/shell"
MEDIA_AUTH_BIN="${REPO_ROOT}/scripts/dev/ddrm-media-authority/target/debug/ddrm-media-authority"
[[ -x "$MEDIA_AUTH_BIN" ]] && export ELASTOS_DDRM_MEDIA_AUTHORITY_BIN="$MEDIA_AUTH_BIN"

# ── AV forensic watermarking (dDRM AV Phase 5) ────────────────────────────────
# Enable per-buyer forensic variants: the mint boundary (encrypt-provider) emits {A,B} variants +
# a bias-committed manifest, and the serve helper (ddrm-media-authority) selects each buyer's
# variant from the wallet-signed grant. BOTH inherit this gateway env, so they share one bias
# MASTER — which MUST be (a) stable across restarts (an asset minted today still serves its
# committed variants tomorrow) and (b) identical on the mint + serve sides (else the manifest's
# bias commitment won't match and the open honestly falls back to the single encode). We persist
# one master per data-dir. Forensic marking needs the wallet-grant (chain) open path, so divergence
# only appears with ELASTOS_DDRM_RIGHTS=chain.
AV_MASTER_FILE="${QUORUM_DIR}/av-bias.master"
mkdir -p "$QUORUM_DIR" 2>/dev/null || true
if [[ ! -s "$AV_MASTER_FILE" ]]; then
  head -c 32 /dev/urandom | base64 | tr -d '\n' > "$AV_MASTER_FILE"
  chmod 600 "$AV_MASTER_FILE" 2>/dev/null || true
fi
export ELASTOS_AV_MASTER_B64="$(tr -d '\r\n' < "$AV_MASTER_FILE")"
export ELASTOS_AV_VARIANTS=1
# The Create portal reads the quorum here (also the default <data_dir>/dkms/quorum.json).
export ELASTOS_DKMS_QUORUM_DESCRIPTOR="$QUORUM_JSON"
# dKMS consumer-open (double-click a minted asset): the key-provider the helper drives + the LIVE
# OPEN descriptor (node endpoints) + the caller seed the running nodes allow-list. Set only when
# the recovery quorum came up — absent, opening a dKMS asset fails closed with a clear 503.
if [[ "$QUORUM_OPEN_READY" -eq 1 ]]; then
  export ELASTOS_DDRM_KEY_PROVIDER_BIN="$KEY_PROVIDER_BIN"
  export ELASTOS_DDRM_QUORUM_OPEN_DESCRIPTOR="$OPEN_DESCRIPTOR"
  export ELASTOS_DDRM_QUORUM_CALLER_SEED_B64="$CALLER_SEED_B64"

  # ── WARM key-provider daemon (Phase A: reuse node sessions across opens) ──────
  # Spawn ONE long-lived key-provider that self-inits the dKMS backend and listens on a Unix
  # socket. The per-open helper dials it (ELASTOS_DDRM_KEY_PROVIDER_SOCKET) and reuses the live
  # node handshake sessions, so every open after the first skips init+hello (~1–2.3s/node). It
  # inherits DKMS_CARRIER_CLIENT_ADDR, so it reaches nodes by did:key over the same Carrier path.
  # CRITICAL: KEY_PROVIDER_LISTEN is passed INLINE (child-only) — never exported — so a
  # fallback-spawned key-provider still runs in plain stdio mode. If the socket never appears we
  # simply DON'T advertise it: opens fall back to the per-open spawn (latency only, never access).
  KP_SOCK="${QUORUM_DIR}/key-provider.sock"
  rm -f "$KP_SOCK" 2>/dev/null || true
  KEY_PROVIDER_LISTEN="$KP_SOCK" \
  KEY_PROVIDER_DAEMON_DESCRIPTOR="$OPEN_DESCRIPTOR" \
  KEY_PROVIDER_DAEMON_CALLER_SEED_B64="$CALLER_SEED_B64" \
    "$KEY_PROVIDER_BIN" >"${QUORUM_DIR}/key-provider-daemon.out" 2>&1 &
  KP_DAEMON_PID=$!
  DKMS_PIDS+=("$KP_DAEMON_PID")
  for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15; do
    [[ -S "$KP_SOCK" ]] && break
    kill -0 "$KP_DAEMON_PID" 2>/dev/null || break
    sleep 0.2
  done
  if [[ -S "$KP_SOCK" ]]; then
    export ELASTOS_DDRM_KEY_PROVIDER_SOCKET="$KP_SOCK"
    echo "  warm key-provider daemon LIVE (pid ${KP_DAEMON_PID}) on ${KP_SOCK} — node sessions reused across opens"
  else
    echo "  WARN: warm key-provider daemon did not come up — opens fall back to per-open spawn (still works, slower)"
  fi
fi
# Default the rights/mint mode to dev (offline) unless the operator pinned one.
export ELASTOS_DDRM_RIGHTS="${ELASTOS_DDRM_RIGHTS:-dev}"

# Live on-chain rights (PC2 parity): when chain mode is selected, the rights gate drives the
# REAL chain-provider doing an eth_call of `hasAccessByContentId(address holder, bytes16 kid)`
# (selector 0x54d42821) against the Base AuthorityGateway (0x09dBe796…, the SAME contract PC2
# uses). It REQUIRES a Base RPC URL; default to the official public endpoint if the operator
# did not pin one. Contract/selector/chain-id default to the real Base values in
# rights_authority.rs and are overridable via ELASTOS_DDRM_RIGHTS_{CONTRACT,SELECTOR}/
# ELASTOS_DDRM_CHAIN_ID. The subject is the principal's linked EVM wallet (chain mode fails
# closed with 403 if none is linked) — override for testing with ELASTOS_DDRM_SUBJECT.
if [[ "$ELASTOS_DDRM_RIGHTS" == "chain" ]]; then
  export ELASTOS_CHAIN_BASE_RPC="${ELASTOS_CHAIN_BASE_RPC:-https://mainnet.base.org}"
fi

echo "AV forensic watermarking: ENABLED (ELASTOS_AV_VARIANTS=1)"
echo "  bias master: ${AV_MASTER_FILE} (persistent; shared by mint + serve)"
if [[ "${ELASTOS_DDRM_RIGHTS:-dev}" != "chain" ]]; then
  echo "  NOTE: forensic marking activates on the wallet-grant (chain) open path — run with"
  echo "        ELASTOS_DDRM_RIGHTS=chain to see per-buyer variants (else assets serve single-encode)."
fi
echo
echo "provider overrides:"
for v in ELASTOS_ENCRYPT_PROVIDER_BIN ELASTOS_MEDIA_PROVIDER_BIN ELASTOS_PUBLISH_PROVIDER_BIN \
         ELASTOS_CHAIN_PROVIDER_BIN ELASTOS_WALLET_PROVIDER_BIN ELASTOS_IPFS_PROVIDER_BIN \
         ELASTOS_RIGHTS_PROVIDER_BIN ELASTOS_DDRM_DECRYPT_BIN ELASTOS_OBJECT_PROVIDER_BIN; do
  printf '  %s=%s\n' "$v" "${!v}"
done
echo "  ELASTOS_DDRM_RIGHTS=${ELASTOS_DDRM_RIGHTS}"
if [[ "$ELASTOS_DDRM_RIGHTS" == "chain" ]]; then
  echo "  rights gate: LIVE on-chain hasAccessByContentId(address,bytes16) via chain-provider"
  echo "    contract = ${ELASTOS_DDRM_RIGHTS_CONTRACT:-0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D}  (Base AuthorityGateway)"
  echo "    selector = ${ELASTOS_DDRM_RIGHTS_SELECTOR:-0x54d42821}  chain_id = ${ELASTOS_DDRM_CHAIN_ID:-8453}"
  echo "    rpc      = ${ELASTOS_CHAIN_BASE_RPC}"
  echo "    subject  = ${ELASTOS_DDRM_SUBJECT:-the principal linked EVM wallet}"
  echo "    => an asset opens ONLY if that wallet currently holds access on-chain (sell/transfer revokes)"
elif [[ "$ELASTOS_DDRM_RIGHTS" == "chain-mock" ]]; then
  echo "  rights gate: chain-mock (real chain-provider vs in-process RPC; ELASTOS_DDRM_CHAIN_ACCESS=denied to fail closed)"
else
  echo "  rights gate: dev (local ownership attestation — NOT an on-chain token check)"
fi
if [[ "$QUORUM_OPEN_READY" -eq 1 ]]; then
  if [[ "$REMOTE" == "1" && "$CARRIER" == "1" ]]; then
    echo "  ELASTOS_DDRM_QUORUM_OPEN_DESCRIPTOR=${OPEN_DESCRIPTOR}  (LIVE 3 geo nodes — Carrier/did:key, no VPN)"
  elif [[ "$REMOTE" == "1" ]]; then
    echo "  ELASTOS_DDRM_QUORUM_OPEN_DESCRIPTOR=${OPEN_DESCRIPTOR}  (LIVE 3 geo nodes — dkms0 mesh)"
  else
    echo "  ELASTOS_DDRM_QUORUM_OPEN_DESCRIPTOR=${OPEN_DESCRIPTOR}  (3 node daemons live)"
  fi
else
  echo "  dKMS consumer-open: DISABLED (mint works; opening a minted asset will 503)"
fi
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
# Run in the FOREGROUND (not exec) so the recovery node daemons we spawned are reaped on exit.
cleanup_daemons() {
  for p in "${DKMS_PIDS[@]:-}"; do kill "$p" 2>/dev/null; done
  [[ -n "${CARRIER_PID:-}" ]] && kill "$CARRIER_PID" 2>/dev/null
  [[ -n "${KP_SOCK:-}" ]] && rm -f "$KP_SOCK" 2>/dev/null
}
trap cleanup_daemons EXIT INT TERM
"$GATEWAY_BIN" gateway --addr "$ADDR"
