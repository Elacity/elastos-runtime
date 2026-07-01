#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

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
export PATH="$(dirname "$node_bin"):${PATH}"

usage() {
  cat <<'USAGE'
Usage:
  scripts/browser-hosted-product-navigation-smoke.sh \
    --adapter-config /path/to/browser-engine-adapter.json \
    [--first-url https://example.com/] \
    [--second-url https://example.com/?elastos-browser-nav-smoke=1] \
    [--guarantee-level operator_rbi|mechanism_microvm] \
    [--relay-ipc-path /path/to/browser-exit-relay.sock] \
    [--timeout-ms 30000]

Launches a hosted Browser product adapter and verifies that address navigation,
back, forward, and reload are applied through the Runtime/provider input route
while preserving the product_compositor contract and direct_network=false.
USAGE
}

adapter_config=""
first_url="https://example.com/"
second_url="https://example.com/?elastos-browser-nav-smoke=1"
guarantee_level="operator_rbi"
relay_ipc_path=""
timeout_ms="30000"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --adapter-config)
      adapter_config="${2:-}"
      shift 2
      ;;
    --first-url)
      first_url="${2:-}"
      shift 2
      ;;
    --second-url)
      second_url="${2:-}"
      shift 2
      ;;
    --guarantee-level)
      guarantee_level="${2:-}"
      shift 2
      ;;
    --relay-ipc-path)
      relay_ipc_path="${2:-}"
      shift 2
      ;;
    --timeout-ms)
      timeout_ms="${2:-}"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [[ -z "$adapter_config" ]]; then
  usage >&2
  exit 1
fi
if [[ ! -f "$adapter_config" ]]; then
  echo "--adapter-config does not exist: $adapter_config" >&2
  exit 1
fi
if [[ ! "$first_url" =~ ^https?:// || ! "$second_url" =~ ^https?:// ]]; then
  echo "--first-url and --second-url must use http or https" >&2
  exit 1
fi
if [[ ! "$timeout_ms" =~ ^[0-9]+$ || "$timeout_ms" -lt 5000 || "$timeout_ms" -gt 120000 ]]; then
  echo "--timeout-ms must be an integer from 5000 to 120000" >&2
  exit 1
fi
if [[ "$guarantee_level" != "operator_rbi" && "$guarantee_level" != "mechanism_microvm" ]]; then
  echo "--guarantee-level must be operator_rbi or mechanism_microvm" >&2
  exit 1
fi
if [[ -n "$relay_ipc_path" && ! -S "$relay_ipc_path" ]]; then
  echo "--relay-ipc-path must be a Unix socket" >&2
  exit 1
fi

cd "$repo_root"
cargo build --quiet --manifest-path capsules/browser-engine-adapter/Cargo.toml
adapter_bin="${CARGO_TARGET_DIR:-capsules/browser-engine-adapter/target}/debug/browser-engine-adapter"

if [[ -n "$relay_ipc_path" ]]; then
  "$node_bin" scripts/browser-hosted-product-navigation-smoke.mjs \
    --adapter-config "$adapter_config" \
    --adapter-bin "$adapter_bin" \
    --first-url "$first_url" \
    --second-url "$second_url" \
    --guarantee-level "$guarantee_level" \
    --relay-ipc-path "$relay_ipc_path" \
    --timeout-ms "$timeout_ms"
else
  "$node_bin" scripts/browser-hosted-product-navigation-smoke.mjs \
    --adapter-config "$adapter_config" \
    --adapter-bin "$adapter_bin" \
    --first-url "$first_url" \
    --second-url "$second_url" \
    --guarantee-level "$guarantee_level" \
    --timeout-ms "$timeout_ms"
fi
