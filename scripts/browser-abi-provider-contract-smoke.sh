#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

find_node() {
  if [[ -n "${ELASTOS_NODE_BIN:-}" && -x "${ELASTOS_NODE_BIN}" ]]; then
    printf '%s\n' "${ELASTOS_NODE_BIN}"
    return 0
  fi
  if command -v node >/dev/null 2>&1; then
    command -v node
    return 0
  fi
  local bundled="${HOME}/.elastos/node/node-v22.13.1-darwin-arm64/bin/node"
  if [[ -x "$bundled" ]]; then
    printf '%s\n' "$bundled"
    return 0
  fi
  return 1
}

node_bin="$(find_node || true)"
if [[ -z "$node_bin" ]]; then
  echo "node not found. Install Node or set ELASTOS_NODE_BIN to an executable node binary." >&2
  exit 2
fi

browser_js=(capsules/browser/browser/*.js)
if [[ ! -e "${browser_js[0]}" ]]; then
  echo "no Browser app JavaScript files found" >&2
  exit 1
fi

for file in "${browser_js[@]}"; do
  "$node_bin" --check "$file"
done

cargo test --manifest-path capsules/browser-engine-adapter/Cargo.toml
cargo test --manifest-path capsules/net-provider/Cargo.toml
cargo test --manifest-path capsules/exit-provider/Cargo.toml

(cd elastos && cargo test -p elastos-server browser --lib)

printf '{"schema":"elastos.browser.abi-provider-contract-smoke/v1","ok":true}\n'
