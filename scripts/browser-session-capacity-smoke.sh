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

export HOME_URL="${HOME_URL:-http://localhost:8090/apps/home/}"
export HOME_VIRTUAL_AUTH_BROWSER=1
export HOME_VIRTUAL_AUTH_BROWSER_SUMMARY=1
export HOME_VIRTUAL_AUTH_BROWSER_OPEN=1
export HOME_VIRTUAL_AUTH_BROWSER_OPEN_CONCURRENT="${HOME_VIRTUAL_AUTH_BROWSER_OPEN_CONCURRENT:-1}"
export HOME_VIRTUAL_AUTH_BROWSER_OPEN_HOLD_MS="${HOME_VIRTUAL_AUTH_BROWSER_OPEN_HOLD_MS:-30000}"
export HOME_VIRTUAL_AUTH_BROWSER_EXPECT_CAPACITY_REJECTION="${HOME_VIRTUAL_AUTH_BROWSER_EXPECT_CAPACITY_REJECTION:-1}"
export HOME_VIRTUAL_AUTH_BROWSER_OPEN_URLS="${HOME_VIRTUAL_AUTH_BROWSER_OPEN_URLS:-https://example.com/,https://example.org/}"

"$node_bin" scripts/home-passkey-virtual-auth-smoke.mjs
