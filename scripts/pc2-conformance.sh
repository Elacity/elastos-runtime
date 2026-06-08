#!/usr/bin/env bash
#
# Executable PC2 cross-implementation conformance check.
#
# Decrypts ElastOS's committed classical golden vector
# (capsules/decrypt-provider/tests/vectors/classical_cenc.json) using PC2
# `ddrm-decrypt`'s REAL code — envelope ECDH unwrap + CENC AES-128-CTR sample
# decrypt — and asserts byte-for-byte parity (recovered CEK and plaintext).
#
# This makes the "byte-compatible with PC2 ddrm-decrypt" claim *executable*: the
# two independent implementations are run against the same bytes.
#
#   - PASS (exit 0): PC2 recovered the same CEK and plaintext from our vector.
#   - FAIL (exit 1): a genuine divergence — the wire contract drifted.
#   - SKIP (exit 0): the PC2 repo is not present (so the default chain is never
#                    broken on machines without the reference checkout).
#
# Override the PC2 location with PC2_REPO=/path/to/pc2-node.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/.." && pwd)"
VECTOR_DIR="$REPO_ROOT/capsules/decrypt-provider/tests/vectors"
# Classical vectors to cross-check (both PC2-supported envelope versions).
VECTORS=(
  "$VECTOR_DIR/classical_cenc.json"     # v3 random IV
  "$VECTOR_DIR/classical_cenc_v2.json"  # v2 fixed IV (derived from eph pubkey)
)
PC2_REPO="${PC2_REPO:-/Users/sash/Documents/Cursor/pc2.net/pc2-node}"
DDRM="$PC2_REPO/crates/ddrm-decrypt"

if [ ! -f "$DDRM/Cargo.toml" ]; then
  echo "SKIP: PC2 ddrm-decrypt not found at $DDRM"
  echo "      (set PC2_REPO=/path/to/pc2-node to enable the cross-impl check)."
  exit 0
fi
for vec in "${VECTORS[@]}"; do
  if [ ! -f "$vec" ]; then
    echo "FAIL: classical golden vector missing at $vec"
    exit 1
  fi
done

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/src"
sed "s#__PC2_DDRM_PATH__#${DDRM}#g" "$HERE/pc2-conformance/Cargo.toml.in" > "$WORK/Cargo.toml"
cp "$HERE/pc2-conformance/driver.rs" "$WORK/src/main.rs"

echo "PC2 cross-impl conformance — decrypting classical vectors with PC2 ddrm-decrypt:"
echo "  PC2 ddrm-decrypt: $DDRM"
cargo run --quiet --manifest-path "$WORK/Cargo.toml" -- "${VECTORS[@]}"
