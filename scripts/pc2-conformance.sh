#!/usr/bin/env bash
#
# Executable PC2 cross-implementation conformance check.
#
# Decrypts ElastOS's committed golden vectors using PC2 `ddrm-decrypt`'s REAL code
# and asserts byte-for-byte parity. Two vector families:
#   - classical envelope vectors: parity at TWO layers —
#       * primitive: envelope ECDH unwrap + CENC AES-128-CTR sample decrypt;
#       * session (the rail carrier path): PC2's PUBLIC session API
#         `session::unwrap_envelope` -> `media::decrypt_segment` — the same
#         entrypoints the production decrypt runtime calls — proving our Option-A
#         carrier is wire-compatible with PC2's session model, not just its crypto.
#     Each layer also checks negative parity: a tampered carrier fails closed in PC2.
#   - producer round-trips (no envelope; CEK captured as rail stand-in): the fMP4
#     segment encrypt-provider's REAL in-boundary engine emitted is decrypted by
#     PC2's `mp4box::parse_segment` + `cenc::decrypt_samples` to the producer's
#     exact bytes (multi-sample + subsample shapes), with a wrong-CEK key-bound
#     check — proving PC2 can consume our PRODUCER's output, not only our consumer.
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
  "$VECTOR_DIR/classical_cenc.json"             # v3 random IV (single sample)
  "$VECTOR_DIR/classical_cenc_v2.json"          # v2 fixed IV (derived from eph pubkey)
  "$VECTOR_DIR/classical_cenc_multisample.json" # 3 samples, per-sample IV
  "$VECTOR_DIR/classical_cenc_subsample.json"   # subsample clear+encrypted ranges
  "$VECTOR_DIR/classical_cenc_initseg.json"     # 16-byte IV via init-segment tenc
  # Producer round-trips: segments emitted by encrypt-provider's REAL in-boundary
  # engine, decrypted by PC2 (proves PC2 consumes our producer's output).
  "$VECTOR_DIR/roundtrip_multisample_encrypt_to_decrypt.json" # 4 samples, per-sample IV
  "$VECTOR_DIR/roundtrip_subsample_encrypt_to_decrypt.json"   # 16B clear leader + enc body
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
