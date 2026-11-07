#!/usr/bin/env bash
# Rebuild the runtime and check every localhost authority branch on a fresh home.
set -euo pipefail
umask 077

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
LAUNCH="$ROOT/.cursor/skills/verify-elastos/scripts/launch.sh"
CLEANUP="$ROOT/.cursor/skills/verify-elastos/scripts/cleanup.sh"
MATRIX="$SCRIPT_DIR/matrix.py"
ENV_FILE=""

cleanup() {
  if [[ -n "$ENV_FILE" && -f "$ENV_FILE" ]]; then
    "$CLEANUP" "$ENV_FILE" || true
  fi
}
trap cleanup EXIT

cd "$ROOT"
cargo build -p elastos-server --release --manifest-path elastos/Cargo.toml
ENV_FILE="$("$LAUNCH")"
RUN_ID="$(basename "$(dirname "$ENV_FILE")")"
OUT="$ROOT/artifacts/verify-localhost-authority/$RUN_ID/matrix.txt"
mkdir -p "$(dirname "$OUT")"
python3 "$MATRIX" "$ENV_FILE" "$OUT"
echo "matrix transcript $OUT"
