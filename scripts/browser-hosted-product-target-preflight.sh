#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<'USAGE'
Usage:
  scripts/browser-hosted-product-target-preflight.sh \
    --out-dir /path/to/config-dir \
    --control-socket /absolute/path/to/product-control.sock

Options:
  --supervisor-program /absolute/path/to/browser-hosted-product-supervisor.mjs
      Default: scripts/browser-hosted-product-supervisor.mjs in this repo.
  --adapter-id <id>
      Default: hosted-product.
  --candidate <id>
      selkies|browserbox|kasm-workspaces|kasmvnc. Sets engine/display backend.
  --engine-kind <kind>
      Default: selkies_gstreamer. Use hosted_remote_browser for KasmVNC/BrowserBox-style spikes.
  --display-backend <backend>
      Default: selkies_gstreamer_webrtc.

This is the hosted Browser product preflight. It generates the Browser Engine
Adapter config and then runs the product-display gate against the configured
compositor control socket. It fails unless the target service returns a real
product_compositor WebRTC display session with audio=true, video=true, and
direct_network=false.
USAGE
}

out_dir=""
control_socket=""
supervisor_program="$repo_root/scripts/browser-hosted-product-supervisor.mjs"
adapter_id="hosted-product"
adapter_id_explicit="0"
candidate=""
engine_kind="selkies_gstreamer"
engine_kind_explicit="0"
display_backend="selkies_gstreamer_webrtc"
display_backend_explicit="0"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --out-dir)
      shift
      out_dir="${1:-}"
      ;;
    --control-socket)
      shift
      control_socket="${1:-}"
      ;;
    --supervisor-program)
      shift
      supervisor_program="${1:-}"
      ;;
    --adapter-id)
      shift
      adapter_id="${1:-}"
      adapter_id_explicit="1"
      ;;
    --candidate)
      shift
      candidate="${1:-}"
      ;;
    --engine-kind)
      shift
      engine_kind="${1:-}"
      engine_kind_explicit="1"
      ;;
    --display-backend)
      shift
      display_backend="${1:-}"
      display_backend_explicit="1"
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
  shift || true
done

if [[ -z "$out_dir" || -z "$control_socket" ]]; then
  usage >&2
  exit 1
fi

if [[ "$control_socket" != /* || "$control_socket" =~ [[:space:]] ]]; then
  echo "--control-socket must be an absolute Unix socket path without whitespace" >&2
  exit 1
fi

if [[ ! -S "$control_socket" ]]; then
  echo "hosted product control socket is not available: $control_socket" >&2
  exit 1
fi

if [[ "$supervisor_program" != /* || ! -x "$supervisor_program" ]]; then
  echo "--supervisor-program must be an absolute executable path: $supervisor_program" >&2
  exit 1
fi

cd "$repo_root"

config_args=(
  --out-dir "$out_dir"
  --supervisor-program "$supervisor_program"
  --control-socket "$control_socket"
)
if [[ "$adapter_id_explicit" == "1" ]]; then
  config_args+=(--adapter-id "$adapter_id")
fi
if [[ -n "$candidate" ]]; then
  config_args+=(--candidate "$candidate")
fi
if [[ "$engine_kind_explicit" == "1" ]]; then
  config_args+=(--engine-kind "$engine_kind")
fi
if [[ "$display_backend_explicit" == "1" ]]; then
  config_args+=(--display-backend "$display_backend")
fi

node scripts/browser-hosted-product-operator-config.mjs "${config_args[@]}" >/dev/null

scripts/browser-hosted-product-display-smoke.sh \
  --adapter-config "$out_dir/browser-engine-adapter.json"

node - <<'NODE' "$out_dir" "$control_socket" "$adapter_id"
const [outDir, controlSocket, adapterId] = process.argv.slice(2);
const config = require("fs").readFileSync(`${outDir}/browser-engine-adapter.json`, "utf8");
const adapter = JSON.parse(config).adapters[0];
console.log(JSON.stringify({
  ok: true,
  out_dir: outDir,
  control_socket: controlSocket,
  adapter_id: adapterId,
  engine: adapter.kind,
  display_backend: adapter.supervisor.env.ELASTOS_BROWSER_PRODUCT_DISPLAY_BACKEND,
  backend_class: "product_compositor",
  audio: true,
  video: true,
  direct_network: false,
}));
NODE
