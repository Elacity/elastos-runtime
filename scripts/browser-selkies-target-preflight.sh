#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out_dir=""
control_socket=""
selkies_ws_url=""
browser_cdp_endpoint=""
supervisor_program="$repo_root/scripts/browser-hosted-product-supervisor.mjs"
timeout_ms="30000"
selkies_basic_auth_user=""
selkies_basic_auth_password=""
ice_servers=()
ice_username=""
ice_credential=""
control_pid=""

usage() {
  cat <<USAGE
Usage:
  scripts/browser-selkies-target-preflight.sh \\
    --out-dir /tmp/elastos-browser-product \\
    --control-socket /tmp/elastos-browser-product/selkies-control.sock \\
    --selkies-ws-url ws://127.0.0.1:8081/ws \\
    --browser-cdp-endpoint http://127.0.0.1:9222

Optional:
  --selkies-basic-auth-user ubuntu \\
  --selkies-basic-auth-password mypasswd
  --ice-server stun:stun.example.invalid:3478 \\
  --ice-server turns:turn.example.com:5349 \\
  --ice-username user \\
  --ice-credential secret

This script does not launch Selkies or Chromium. It verifies an already-running
operator Selkies/GStreamer product target by starting the ElastOS Selkies
control bridge and running the hosted product display preflight through the
Browser Engine Adapter contract.
USAGE
}

cleanup() {
  if [[ -n "$control_pid" ]]; then
    kill "$control_pid" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

while [[ $# -gt 0 ]]; do
  case "$1" in
    --out-dir)
      out_dir="${2:-}"
      shift 2
      ;;
    --control-socket)
      control_socket="${2:-}"
      shift 2
      ;;
    --selkies-ws-url)
      selkies_ws_url="${2:-}"
      shift 2
      ;;
    --browser-cdp-endpoint)
      browser_cdp_endpoint="${2:-}"
      shift 2
      ;;
    --selkies-basic-auth-user)
      selkies_basic_auth_user="${2:-}"
      shift 2
      ;;
    --selkies-basic-auth-password)
      selkies_basic_auth_password="${2:-}"
      shift 2
      ;;
    --ice-server)
      ice_servers+=("${2:-}")
      shift 2
      ;;
    --ice-username)
      ice_username="${2:-}"
      shift 2
      ;;
    --ice-credential)
      ice_credential="${2:-}"
      shift 2
      ;;
    --supervisor-program)
      supervisor_program="${2:-}"
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
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -z "$out_dir" || -z "$control_socket" || -z "$selkies_ws_url" || -z "$browser_cdp_endpoint" ]]; then
  usage >&2
  exit 2
fi
if [[ -n "$selkies_basic_auth_user" && -z "$selkies_basic_auth_password" ]]; then
  echo "--selkies-basic-auth-password is required when --selkies-basic-auth-user is provided" >&2
  exit 2
fi
if [[ -z "$selkies_basic_auth_user" && -n "$selkies_basic_auth_password" ]]; then
  echo "--selkies-basic-auth-user is required when --selkies-basic-auth-password is provided" >&2
  exit 2
fi
if [[ ${#ice_servers[@]} -eq 0 && ( -n "$ice_username" || -n "$ice_credential" ) ]]; then
  echo "--ice-server is required when ICE credentials are provided" >&2
  exit 2
fi
if [[ -n "$ice_username" && -z "$ice_credential" ]]; then
  echo "--ice-credential is required when --ice-username is provided" >&2
  exit 2
fi
if [[ -z "$ice_username" && -n "$ice_credential" ]]; then
  echo "--ice-username is required when --ice-credential is provided" >&2
  exit 2
fi
case "$control_socket" in
  /*) ;;
  *) echo "--control-socket must be an absolute path" >&2; exit 2 ;;
esac

mkdir -p "$out_dir"
rm -f "$control_socket"

ice_servers_json="$(node -e 'console.log(JSON.stringify(process.argv.slice(1)))' "${ice_servers[@]}")"
control_config="$(node -e '
const [controlSocket, selkiesWsUrl, browserCdpEndpoint, timeoutRaw, basicAuthUser, basicAuthPassword, iceServersRaw, iceUsername, iceCredential] = process.argv.slice(1);
const ws = new URL(selkiesWsUrl);
if (!["ws:", "wss:"].includes(ws.protocol)) throw new Error("--selkies-ws-url must use ws or wss");
const cdp = new URL(browserCdpEndpoint);
if (!["http:", "https:"].includes(cdp.protocol)) throw new Error("--browser-cdp-endpoint must use http or https");
if (!["127.0.0.1", "::1", "localhost"].includes(cdp.hostname)) {
  throw new Error("--browser-cdp-endpoint must be loopback/private to the operator service");
}
const timeoutMs = Number(timeoutRaw);
if (!Number.isInteger(timeoutMs) || timeoutMs < 1000 || timeoutMs > 300000) {
  throw new Error("--timeout-ms must be an integer from 1000 to 300000");
}
const iceUrls = JSON.parse(iceServersRaw);
if (!Array.isArray(iceUrls) || iceUrls.length > 8) {
  throw new Error("--ice-server may be repeated at most 8 times");
}
for (const url of iceUrls) {
  if (typeof url !== "string" || !/^(stun|turns?):/i.test(url.trim()) || /[\r\n\0]/.test(url) || url.trim().length > 512) {
    throw new Error("--ice-server must be a stun:, turn:, or turns: URL without control characters");
  }
}
const config = {
  schema: "elastos.browser.selkies-control.config/v1",
  control_socket_path: controlSocket,
  replace_existing_socket: true,
  selkies_ws_url: selkiesWsUrl,
  browser_control: {
    kind: "cdp_http",
    endpoint: browserCdpEndpoint,
    timeout_ms: Math.min(timeoutMs, 10000)
  },
  connect_timeout_ms: timeoutMs,
  signal_timeout_ms: timeoutMs
};
if (basicAuthUser && basicAuthPassword) {
  if (/[\r\n\0]/.test(basicAuthUser) || /[\r\n\0]/.test(basicAuthPassword)) {
    throw new Error("Selkies basic auth credentials must not contain control characters");
  }
  config.basic_auth = { user: basicAuthUser, password: basicAuthPassword };
}
if (iceUrls.length > 0) {
  const iceServer = { urls: iceUrls.map((url) => url.trim()) };
  if (iceUsername || iceCredential) {
    if (!iceUsername || !iceCredential || /[\r\n\0]/.test(iceUsername) || /[\r\n\0]/.test(iceCredential)) {
      throw new Error("ICE username and credential must be provided together without control characters");
    }
    iceServer.username = iceUsername;
    iceServer.credential = iceCredential;
  }
  config.ice_servers = [iceServer];
}
console.log(JSON.stringify(config));
' "$control_socket" "$selkies_ws_url" "$browser_cdp_endpoint" "$timeout_ms" "$selkies_basic_auth_user" "$selkies_basic_auth_password" "$ice_servers_json" "$ice_username" "$ice_credential")"

ELASTOS_BROWSER_SELKIES_CONTROL_CONFIG="$control_config" \
  "$repo_root/scripts/browser-selkies-control-service.mjs" >"$out_dir/selkies-control.log" 2>&1 &
control_pid="$!"

for _ in {1..200}; do
  [[ -S "$control_socket" ]] && break
  sleep 0.025
done
if [[ ! -S "$control_socket" ]]; then
  echo "Selkies control bridge did not create its control socket" >&2
  sed -n '1,120p' "$out_dir/selkies-control.log" >&2 || true
  exit 1
fi

"$repo_root/scripts/browser-hosted-product-target-preflight.sh" \
  --out-dir "$out_dir/config" \
  --supervisor-program "$supervisor_program" \
  --control-socket "$control_socket"
