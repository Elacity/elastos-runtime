#!/usr/bin/env bash
set -euo pipefail

repo_root="${ELASTOS_BROWSER_SELKIES_REPO:-/opt/elastos-runtime}"
out_dir="${ELASTOS_BROWSER_SELKIES_OUT_DIR:-/tmp/elastos-browser-selkies-live}"
target_image="${ELASTOS_BROWSER_SELKIES_TARGET_IMAGE:-elastos/browser-selkies-runtime-target:dev}"
browser_program="${ELASTOS_BROWSER_SELKIES_BROWSER_PROGRAM:-}"
allowed_hosts="${ELASTOS_BROWSER_SELKIES_ALLOWED_HOSTS:-*}"
allowed_ports="${ELASTOS_BROWSER_SELKIES_ALLOWED_PORTS:-80,443}"
address_family="${ELASTOS_BROWSER_SELKIES_ADDRESS_FAMILY:-prefer_ipv4}"
auth_user="${ELASTOS_BROWSER_SELKIES_BASIC_AUTH_USER:-ubuntu}"
auth_password="${ELASTOS_BROWSER_SELKIES_BASIC_AUTH_PASSWORD:-}"
ice_servers_csv="${ELASTOS_BROWSER_SELKIES_ICE_SERVERS:-}"
ice_username="${ELASTOS_BROWSER_SELKIES_ICE_USERNAME:-}"
ice_credential="${ELASTOS_BROWSER_SELKIES_ICE_CREDENTIAL:-}"
encoder="${ELASTOS_BROWSER_SELKIES_ENCODER:-x264enc}"
framerate="${ELASTOS_BROWSER_SELKIES_FRAMERATE:-30}"
video_bitrate="${ELASTOS_BROWSER_SELKIES_VIDEO_BITRATE:-16}"
h264_crf="${ELASTOS_BROWSER_SELKIES_H264_CRF:-23}"
verify_url="${ELASTOS_BROWSER_SELKIES_VERIFY_URL:-https://example.com/}"

if [[ ! -d "$repo_root" ]]; then
  echo "ELASTOS_BROWSER_SELKIES_REPO does not exist: $repo_root" >&2
  exit 2
fi

cd "$repo_root"

args=(
  --out-dir "$out_dir"
  --target-image "$target_image"
  --adapter-id hosted-product
  --allowed-hosts "$allowed_hosts"
  --allowed-ports "$allowed_ports"
  --address-family "$address_family"
  --selkies-basic-auth-user "$auth_user"
  --selkies-encoder "$encoder"
  --selkies-framerate "$framerate"
  --selkies-video-bitrate "$video_bitrate"
  --selkies-h264-crf "$h264_crf"
  --verify-url "$verify_url"
)

if [[ -n "$browser_program" ]]; then
  args+=(--browser-program "$browser_program")
fi
if [[ -n "$auth_password" ]]; then
  args+=(--selkies-basic-auth-password "$auth_password")
fi
if [[ -n "$ice_servers_csv" ]]; then
  IFS=',' read -r -a ice_servers <<<"$ice_servers_csv"
  for server in "${ice_servers[@]}"; do
    server="${server#"${server%%[![:space:]]*}"}"
    server="${server%"${server##*[![:space:]]}"}"
    [[ -n "$server" ]] && args+=(--ice-server "$server")
  done
fi
if [[ -n "$ice_username" ]]; then
  args+=(--ice-username "$ice_username")
fi
if [[ -n "$ice_credential" ]]; then
  args+=(--ice-credential "$ice_credential")
fi

exec scripts/browser-selkies-runtime-exit-target.sh "${args[@]}"
