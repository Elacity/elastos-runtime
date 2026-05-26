#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<'USAGE'
Usage:
  scripts/browser-hosted-provider-candidate-smoke.sh \
    --adapter-config /path/to/browser-engine-adapter.json \
    --cdp-endpoint http://127.0.0.1:PORT \
    [--media-url https://www.w3schools.com/html/mov_bbb.mp4] \
    [--hold-ms 60000] \
    [--resize-width 1000 --resize-height 700] \
    [--timeout-ms 120000]

Runs the hosted Browser provider acceptance sequence against any provider behind
the Browser Engine Adapter contract. This is the comparison gate for Selkies,
KasmVNC/Workspaces, BrowserBox, or another hosted remote browser candidate.

The candidate must prove:
  - product compositor display contract, not CDP/JPEG proof frames
  - WebRTC audio track, video track, datachannel input, connected ICE
  - remote compositor resize, so page scale can match the visible window
  - Runtime/provider navigation commands: address navigate, back, forward, reload
  - controlled media playback with decoded audio/video bytes and quality floor
  - Runtime-mediated EIP-1193 wallet bridge
  - Glide connect-wallet flow with direct_network=false
USAGE
}

adapter_config=""
cdp_endpoint=""
media_url="https://www.w3schools.com/html/mov_bbb.mp4"
hold_ms="60000"
resize_width="1000"
resize_height="700"
timeout_ms="120000"

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
    --media-url)
      media_url="${2:-}"
      shift 2
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

if [[ -z "$adapter_config" || -z "$cdp_endpoint" ]]; then
  usage >&2
  exit 1
fi
if [[ ! -f "$adapter_config" ]]; then
  echo "--adapter-config does not exist: $adapter_config" >&2
  exit 1
fi
if [[ ! "$cdp_endpoint" =~ ^https?://127\.0\.0\.1:[0-9]+/?$ && ! "$cdp_endpoint" =~ ^https?://localhost:[0-9]+/?$ ]]; then
  echo "--cdp-endpoint must be an operator-private loopback HTTP endpoint" >&2
  exit 1
fi
if [[ ! "$media_url" =~ ^https?:// ]]; then
  echo "--media-url must use http or https" >&2
  exit 1
fi
if [[ ! "$hold_ms" =~ ^[0-9]+$ || "$hold_ms" -lt 0 || "$hold_ms" -gt 300000 ]]; then
  echo "--hold-ms must be an integer from 0 to 300000" >&2
  exit 1
fi
if [[ ! "$resize_width" =~ ^[0-9]+$ || ! "$resize_height" =~ ^[0-9]+$ || "$resize_width" -lt 320 || "$resize_height" -lt 240 ]]; then
  echo "--resize-width/--resize-height must be integers at least 320x240" >&2
  exit 1
fi
if [[ ! "$timeout_ms" =~ ^[0-9]+$ || "$timeout_ms" -lt 5000 || "$timeout_ms" -gt 120000 ]]; then
  echo "--timeout-ms must be an integer from 5000 to 120000" >&2
  exit 1
fi

cd "$repo_root"

scripts/browser-hosted-product-display-smoke.sh \
  --adapter-config "$adapter_config" >/dev/null

media_result="$(
  scripts/browser-hosted-product-webrtc-smoke.sh \
    --adapter-config "$adapter_config" \
    --cdp-endpoint "$cdp_endpoint" \
    --require-media \
    --hold-ms "$hold_ms" \
    --resize-width "$resize_width" \
    --resize-height "$resize_height" \
    --url "$media_url" \
    --timeout-ms "$timeout_ms"
)"

navigation_result="$(
  scripts/browser-hosted-product-navigation-smoke.sh \
    --adapter-config "$adapter_config" \
    --timeout-ms "$timeout_ms"
)"

wallet_result="$(
  scripts/browser-hosted-product-wallet-smoke.sh \
    --adapter-config "$adapter_config" \
    --cdp-endpoint "$cdp_endpoint"
)"

glide_result="$(
  scripts/browser-hosted-product-glide-wallet-smoke.sh \
    --adapter-config "$adapter_config" \
    --cdp-endpoint "$cdp_endpoint"
)"

MEDIA_RESULT="$media_result" NAVIGATION_RESULT="$navigation_result" WALLET_RESULT="$wallet_result" GLIDE_RESULT="$glide_result" \
  node - <<'NODE'
function parse(name) {
  const raw = process.env[name] || "";
  const line = raw.trim().split(/\r?\n/).filter(Boolean).at(-1);
  if (!line) throw new Error(`${name} did not produce JSON output`);
  return JSON.parse(line);
}

const media = parse("MEDIA_RESULT");
const navigation = parse("NAVIGATION_RESULT");
const wallet = parse("WALLET_RESULT");
const glide = parse("GLIDE_RESULT");
if (media.ok !== true || navigation.ok !== true || wallet.ok !== true || glide.ok !== true) {
  throw new Error("hosted provider candidate gate failed");
}
console.log(JSON.stringify({
  ok: true,
  schema: "elastos.browser.hosted-provider-candidate-smoke/v1",
  display_backend: media.display_backend,
  backend_class: media.backend_class,
  audio_track: media.audio_track,
  video_track: media.video_track,
  datachannel_input: media.datachannel_input,
  held_ms: media.held_ms,
  quality_gate: media.quality_gate,
  resize_gate: media.resize_gate,
  navigation: {
    first_url: navigation.first_url,
    second_url: navigation.second_url,
    can_go_back_after_navigate: navigation.after_navigate?.can_go_back === true,
    can_go_forward_after_back: navigation.after_back?.can_go_forward === true,
  },
  wallet_bridge: wallet.wallet_bridge,
  glide_connected_account: glide.connected_account,
  direct_network: false,
}));
NODE
