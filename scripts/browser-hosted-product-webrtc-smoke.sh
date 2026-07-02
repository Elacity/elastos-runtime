#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<'USAGE'
Usage:
  scripts/browser-hosted-product-webrtc-smoke.sh \
    --adapter-config /path/to/browser-engine-adapter.json \
    [--cdp-endpoint http://127.0.0.1:PORT] \
    [--relay-ipc-path /tmp/elastos-browser-local-exit.sock] \
    [--hold-ms 0] \
    [--resize-width 0 --resize-height 0] \
    [--require-media] \
    [--url https://example.com/] \
    [--timeout-ms 30000]

Launches Browser Engine Adapter with the given hosted product config, answers
the engine-created Selkies WebRTC offer with a real Node WebRTC client, and
fails unless audio track, video track, datachannel input, and connected ICE are
all observed.
USAGE
}

adapter_config=""
cdp_endpoint=""
relay_ipc_path=""
require_media=0
url="https://example.com/"
timeout_ms="30000"
hold_ms="0"
resize_width="0"
resize_height="0"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --adapter-config)
      adapter_config="${2:-}"
      shift 2
      ;;
    --cdp-endpoint)
      cdp_endpoint="${2:-}"
      shift 2
      ;;
    --relay-ipc-path)
      relay_ipc_path="${2:-}"
      shift 2
      ;;
    --require-media)
      require_media=1
      shift
      ;;
    --hold-ms)
      hold_ms="${2:-}"
      shift 2
      ;;
    --resize-width)
      resize_width="${2:-}"
      shift 2
      ;;
    --resize-height)
      resize_height="${2:-}"
      shift 2
      ;;
    --url)
      url="${2:-}"
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

cd "$repo_root"
cargo build --quiet --manifest-path capsules/browser-engine-adapter/Cargo.toml
adapter_bin="${CARGO_TARGET_DIR:-capsules/browser-engine-adapter/target}/debug/browser-engine-adapter"

args=(
  --adapter-config "$adapter_config"
  --adapter-bin "$adapter_bin"
  --url "$url"
  --timeout-ms "$timeout_ms"
  --hold-ms "$hold_ms"
  --resize-width "$resize_width"
  --resize-height "$resize_height"
)
if [[ -n "$relay_ipc_path" ]]; then
  args+=(--relay-ipc-path "$relay_ipc_path")
fi
if [[ "$require_media" == "1" ]]; then
  args+=(--require-media --cdp-endpoint "$cdp_endpoint")
fi

node scripts/browser-hosted-product-webrtc-smoke.mjs \
  "${args[@]}"
