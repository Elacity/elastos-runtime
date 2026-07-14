#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
node_bin="${ELASTOS_NODE_BIN:-}"
if [[ -z "$node_bin" ]]; then
    node_bin="$(command -v node 2>/dev/null || true)"
fi
if [[ -z "$node_bin" || ! -x "$node_bin" ]]; then
    echo "node not found. Install Node or set ELASTOS_NODE_BIN to an executable node binary." >&2
    exit 1
fi

echo "[gba-demo] verify the portable, lazy capsule engine"
"$node_bin" "$ROOT/scripts/normalize-gba-engine-imports.mjs"
"$node_bin" --check "$ROOT/capsules/gba-emulator/browser/emulator.js"
"$node_bin" "$ROOT/scripts/gba-projection-smoke.mjs"
if [[ -n "${ELASTOS_GBA_HOME_TOKEN:-}" ]]; then
    "$node_bin" "$ROOT/scripts/gba-live-smoke.mjs"
fi

echo "[gba-demo] verify Runtime ROM, Library, save, and authorization routes"
(
    cd "$ROOT/elastos"
    cargo test -p elastos-server gateway_tests::gba -- --nocapture
)

test -s "$ROOT/capsules/gba-emulator/browser/mgba.js"
test -s "$ROOT/capsules/gba-emulator/browser/mgba.wasm"
test ! -e "$ROOT/capsules/gba-engine-provider"

echo "[gba-demo] OK portable capsule"
