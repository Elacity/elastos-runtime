#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

node_bin="${ELASTOS_NODE_BIN:-}"
if [[ -z "$node_bin" ]]; then
  if command -v node >/dev/null 2>&1; then
    node_bin="$(command -v node)"
  elif [[ -x "/Users/anders/.elastos/node/node-v22.13.1-darwin-arm64/bin/node" ]]; then
    node_bin="/Users/anders/.elastos/node/node-v22.13.1-darwin-arm64/bin/node"
  else
    echo "node not found; set ELASTOS_NODE_BIN to run home-entropy-check.mjs" >&2
    exit 2
  fi
fi

echo "[capsule-inspector-act] server Inspector action tests"
(cd elastos && cargo test -p elastos-server inspect_action -- --nocapture)

echo "[capsule-inspector-act] runtime Inspect scope tests"
(cd elastos && cargo test -p elastos-runtime inspect -- --nocapture)

echo "[capsule-inspector-act] Home/Inspector entropy sentinel"
"$node_bin" scripts/home-entropy-check.mjs

printf '%s\n' '{"schema":"elastos.capsule-inspector-act-check/v1","ok":true}'
