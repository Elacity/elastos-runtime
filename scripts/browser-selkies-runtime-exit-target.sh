#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<'USAGE'
Usage:
  scripts/browser-selkies-runtime-exit-target.sh --out-dir /tmp/elastos-browser-target [options]

Starts the hosted Browser product target:
  browser-local-exit -> browser-native-proxy-engine -> Chromium
  Selkies/GStreamer WebRTC compositor -> browser-selkies-control-service
  browser-engine-adapter config for Runtime

Options:
  --browser-program /path/to/chromium      Default: Playwright Chromium
  --adapter-id <id>                        Default: hosted-product
  --allowed-hosts <csv>                    Default: *
  --allowed-ports <csv>                    Default: 80,443
  --address-family <policy>                Default: prefer_ipv4
  --selkies-basic-auth-user <user>         Default: ubuntu
  --selkies-basic-auth-password <password> Default: generated per run
  --ice-server <stun|turn|turns URL>       Repeatable WebRTC ICE server for clients
  --ice-username <user>                    Optional TURN username
  --ice-credential <secret>                Optional TURN credential
  --selkies-encoder <encoder>              Default: x264enc
  --selkies-framerate <fps>                Default: 30
  --selkies-video-bitrate <mbps>           Default: 16
  --selkies-h264-crf <value>               Default: 23
  --target-image <image>                   Prebuilt Selkies Runtime target image
  --local-exit-bin /path/to/browser-local-exit
  --native-proxy-bin /path/to/browser-native-proxy-engine
  --profile-dir /path/to/profile           Persistent Chromium profile directory
  --verify-url <url>                       Default: https://example.com/
  --verify                                Verify page load + product display, then keep running
  --cleanup-after-verify                  With --verify, stop everything after successful verification
  --timeout-seconds <seconds>              Default: 300

The target requires Docker and runs the Selkies service on host networking with
loopback-only CDP and Selkies endpoints. Browser traffic goes through
browser-native-proxy-engine into browser-local-exit over a private Unix relay.
USAGE
}

out_dir=""
browser_program=""
adapter_id="hosted-product"
allowed_hosts="*"
allowed_ports="80,443"
address_family="prefer_ipv4"
selkies_auth_user="ubuntu"
selkies_auth_password=""
ice_servers=()
ice_username=""
ice_credential=""
selkies_encoder="x264enc"
selkies_framerate="30"
selkies_video_bitrate="16"
selkies_h264_crf="23"
selkies_width="1920"
selkies_height="1080"
selkies_display="${ELASTOS_BROWSER_SELKIES_DISPLAY:-:$((40 + ($$ % 50)))}"
target_image=""
local_exit_bin="${ELASTOS_BROWSER_LOCAL_EXIT_BIN:-}"
native_proxy_bin="${ELASTOS_BROWSER_NATIVE_PROXY_BIN:-}"
profile_dir=""
verify_url="https://example.com/"
verify=0
cleanup_after_verify=0
timeout_seconds="${ELASTOS_BROWSER_SELKIES_SMOKE_TIMEOUT_SECONDS:-300}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --out-dir)
      out_dir="${2:-}"
      shift 2
      ;;
    --browser-program)
      browser_program="${2:-}"
      shift 2
      ;;
    --adapter-id)
      adapter_id="${2:-}"
      shift 2
      ;;
    --allowed-hosts)
      allowed_hosts="${2:-}"
      shift 2
      ;;
    --allowed-ports)
      allowed_ports="${2:-}"
      shift 2
      ;;
    --address-family)
      address_family="${2:-}"
      shift 2
      ;;
    --selkies-basic-auth-user)
      selkies_auth_user="${2:-}"
      shift 2
      ;;
    --selkies-basic-auth-password)
      selkies_auth_password="${2:-}"
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
    --selkies-encoder)
      selkies_encoder="${2:-}"
      shift 2
      ;;
    --selkies-framerate)
      selkies_framerate="${2:-}"
      shift 2
      ;;
    --selkies-video-bitrate)
      selkies_video_bitrate="${2:-}"
      shift 2
      ;;
    --selkies-h264-crf)
      selkies_h264_crf="${2:-}"
      shift 2
      ;;
    --target-image)
      target_image="${2:-}"
      shift 2
      ;;
    --local-exit-bin)
      local_exit_bin="${2:-}"
      shift 2
      ;;
    --native-proxy-bin)
      native_proxy_bin="${2:-}"
      shift 2
      ;;
    --profile-dir)
      profile_dir="${2:-}"
      shift 2
      ;;
    --verify-url)
      verify_url="${2:-}"
      shift 2
      ;;
    --verify)
      verify=1
      shift
      ;;
    --cleanup-after-verify)
      cleanup_after_verify=1
      shift
      ;;
    --timeout-seconds)
      timeout_seconds="${2:-}"
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

if [[ -z "$out_dir" ]]; then
  usage >&2
  exit 2
fi
if ! [[ "$timeout_seconds" =~ ^[0-9]+$ ]] || (( timeout_seconds < 30 || timeout_seconds > 1800 )); then
  echo "--timeout-seconds must be 30..1800" >&2
  exit 2
fi
case "$address_family" in
  system|prefer_ipv4|prefer_ipv6|ipv4_only|ipv6_only) ;;
  *) echo "--address-family must be system, prefer_ipv4, prefer_ipv6, ipv4_only, or ipv6_only" >&2; exit 2 ;;
esac
if [[ ! "$adapter_id" =~ ^[A-Za-z0-9:_-]+$ ]]; then
  echo "--adapter-id must be a safe identifier" >&2
  exit 2
fi
if [[ "$selkies_auth_user" =~ [$'\r\n\0'] ]]; then
  echo "--selkies-basic-auth-user must not contain control characters" >&2
  exit 2
fi
if [[ -n "$selkies_auth_password" && "$selkies_auth_password" =~ [$'\r\n\0'] ]]; then
  echo "--selkies-basic-auth-password must not contain control characters" >&2
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
case "$selkies_encoder" in
  x264enc|x264enc-striped|jpeg) ;;
  *) echo "--selkies-encoder must be x264enc, x264enc-striped, or jpeg" >&2; exit 2 ;;
esac
for numeric_value in \
  "selkies-framerate:$selkies_framerate" \
  "selkies-video-bitrate:$selkies_video_bitrate" \
  "selkies-h264-crf:$selkies_h264_crf"; do
  name="${numeric_value%%:*}"
  value="${numeric_value#*:}"
  if ! [[ "$value" =~ ^[0-9]+$ ]]; then
    echo "--$name must be a positive integer" >&2
    exit 2
  fi
done
if (( selkies_framerate < 8 || selkies_framerate > 165 )); then
  echo "--selkies-framerate must be 8..165" >&2
  exit 2
fi
if (( selkies_video_bitrate < 1 || selkies_video_bitrate > 100 )); then
  echo "--selkies-video-bitrate must be 1..100" >&2
  exit 2
fi
if (( selkies_h264_crf < 5 || selkies_h264_crf > 50 )); then
  echo "--selkies-h264-crf must be 5..50" >&2
  exit 2
fi
cd "$repo_root"

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required for Selkies Runtime Exit target" >&2
  exit 1
fi

if [[ -z "$browser_program" ]]; then
  browser_program="$(
    cd elastos/tools/browser-playwright-engine
    node - <<'NODE'
import { chromium } from 'playwright';
console.log(chromium.executablePath());
NODE
  )"
fi
if [[ ! -x "$browser_program" ]]; then
  echo "Chromium binary is not executable: $browser_program" >&2
  exit 2
fi
if [[ -z "$selkies_auth_password" ]]; then
  selkies_auth_password="$(node -e 'console.log(require("node:crypto").randomBytes(18).toString("base64url"))')"
fi
if [[ ! "$selkies_display" =~ ^:[0-9]+$ ]]; then
  echo "ELASTOS_BROWSER_SELKIES_DISPLAY must look like :43" >&2
  exit 2
fi
selkies_display_number="${selkies_display#:}"

out_dir="$(mkdir -p "$out_dir" && cd "$out_dir" && pwd)"
if [[ -z "$profile_dir" ]]; then
  profile_dir="$out_dir/chromium-profile"
fi
if [[ "$profile_dir" != /* || "$profile_dir" =~ [$'\r\n\0'] ]]; then
  echo "--profile-dir must be an absolute path without control characters" >&2
  exit 2
fi
profile_dir="$(mkdir -p "$profile_dir" && cd "$profile_dir" && pwd)"
chmod 700 "$profile_dir" >/dev/null 2>&1 || true
profile_lock_file="$profile_dir/.elastos-profile.lock"
exec 8>"$profile_lock_file"
if ! flock -n 8; then
  echo "Browser profile is already active in another Runtime Browser session: $profile_dir" >&2
  exit 1
fi
lock_file="$out_dir/.target.lock"
exec 9>"$lock_file"
if ! flock -n 9; then
  echo "Selkies Runtime Exit target is already running for $out_dir" >&2
  exit 1
fi
cargo_target_dir="${ELASTOS_BROWSER_SELKIES_CARGO_TARGET_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/elastos/browser-selkies-cargo-target}"
mkdir -p "$cargo_target_dir"
if [[ -z "$local_exit_bin" ]]; then
  if [[ -x "$repo_root/bin/browser-local-exit" ]]; then
    local_exit_bin="$repo_root/bin/browser-local-exit"
  else
    local_exit_bin="$cargo_target_dir/debug/browser-local-exit"
  fi
fi
if [[ -z "$native_proxy_bin" ]]; then
  if [[ -x "$repo_root/bin/browser-native-proxy-engine" ]]; then
    native_proxy_bin="$repo_root/bin/browser-native-proxy-engine"
  else
    native_proxy_bin="$cargo_target_dir/debug/browser-native-proxy-engine"
  fi
fi
relay_socket="$out_dir/local-exit.sock"
control_socket="$out_dir/selkies-control.sock"
local_exit_log="$out_dir/local-exit.out"
local_exit_err="$out_dir/local-exit.err"
control_log="$out_dir/selkies-control.log"
container_profile_dir="/var/lib/elastos-browser-profile"
container_name="elastos-selkies-runtime-exit-target-$$"
wheel_container=""
local_exit_pid=""
control_pid=""
terminating=0

dump_diagnostics() {
  echo "Selkies Runtime Exit target diagnostics follow" >&2
  docker ps --filter name="$container_name" >&2 || true
  docker logs --tail 120 "$container_name" >&2 || true
  docker exec "$container_name" bash -lc 'tail -220 /tmp/selkies-current.log /tmp/native-proxy-engine.log /tmp/chromium-current.log /tmp/Xvfb-current.log /tmp/pipewire-current.log /tmp/pipewire-pulse-current.log /tmp/wireplumber-current.log /tmp/selkies-install.log 2>/dev/null || true' >&2 || true
  sed -n '1,180p' "$local_exit_log" "$local_exit_err" "$control_log" 2>/dev/null >&2 || true
}

cleanup() {
  local status="$?"
  if [[ "$status" != "0" && "$terminating" != "1" ]]; then
    dump_diagnostics
  fi
  if [[ -n "$control_pid" ]]; then
    kill "$control_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "$local_exit_pid" ]]; then
    kill "$local_exit_pid" >/dev/null 2>&1 || true
  fi
  docker rm -f "$container_name" >/dev/null 2>&1 || true
  if [[ -n "$wheel_container" ]]; then
    docker rm "$wheel_container" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT
trap 'if [[ "${ELASTOS_BROWSER_DUMP_DIAGNOSTICS_ON_TERM:-0}" == "1" ]]; then dump_diagnostics; fi; terminating=1; exit 143' TERM INT

if [[ ! -x "$local_exit_bin" || ! -x "$native_proxy_bin" ]]; then
  cargo_bin="${CARGO:-}"
  if [[ -z "$cargo_bin" ]]; then
    cargo_bin="$(command -v cargo 2>/dev/null || true)"
  fi
  if [[ -z "$cargo_bin" ]]; then
    runtime_user_home="$(getent passwd "$(id -un)" | cut -d: -f6 || true)"
    for candidate in \
      "${CARGO_HOME:-}/bin/cargo" \
      "$repo_root/.cargo/bin/cargo" \
      "${runtime_user_home:-}/.cargo/bin/cargo" \
      "/usr/local/cargo/bin/cargo" \
      "/root/.cargo/bin/cargo"; do
      if [[ -x "$candidate" ]]; then
        cargo_bin="$candidate"
        break
      fi
    done
  fi
  if [[ -z "$cargo_bin" ]]; then
    echo "cargo not found and Browser helper binaries are unavailable: $local_exit_bin $native_proxy_bin" >&2
    exit 127
  fi
  cargo_bin_dir="$(dirname "$cargo_bin")"
  PATH="$cargo_bin_dir:$PATH" CARGO_TARGET_DIR="$cargo_target_dir" "$cargo_bin" build --quiet --manifest-path elastos/tools/browser-local-exit/Cargo.toml
  PATH="$cargo_bin_dir:$PATH" CARGO_TARGET_DIR="$cargo_target_dir" "$cargo_bin" build --quiet --manifest-path elastos/tools/browser-native-proxy-engine/Cargo.toml
fi

browser_dir="$(dirname "$browser_program")"
selkies_port="$(node -e 'const net=require("node:net"); const s=net.createServer(); s.listen(0,"127.0.0.1",()=>{const p=s.address().port; s.close(()=>console.log(p));});')"
cdp_port="$(node -e 'const net=require("node:net"); const s=net.createServer(); s.listen(0,"127.0.0.1",()=>{const p=s.address().port; s.close(()=>console.log(p));});')"
docker_image="${target_image:-ghcr.io/selkies-project/selkies-gstreamer/gst-py-example:main-ubuntu24.04}"
wheel_mount_args=()
container_bootstrap_commands=": > /tmp/apt.log
: > /tmp/apt-install.log
: > /tmp/selkies-install.log"

rm -f "$relay_socket" "$control_socket"

ELASTOS_BROWSER_LOCAL_EXIT_CONFIG="$(node - <<NODE
const allowedPorts = "$allowed_ports".split(",").map((value) => Number(value.trim())).filter(Boolean);
console.log(JSON.stringify({
  schema: "elastos.browser.local-exit.config/v1",
  relay_ipc_path: "$relay_socket",
  allowed_hosts: "$allowed_hosts".split(",").map((value) => value.trim()).filter(Boolean),
  allowed_schemes: ["tcp", "tls"],
  allowed_ports: allowedPorts,
  address_family: "$address_family",
  allow_private_targets: false,
  replace_existing_socket: true
}));
NODE
)" \
  "$local_exit_bin" \
  >"$local_exit_log" 2>"$local_exit_err" &
local_exit_pid="$!"
for _ in {1..100}; do
  [[ -S "$relay_socket" ]] && break
  sleep 0.05
done
if [[ ! -S "$relay_socket" ]]; then
  echo "browser-local-exit did not create relay socket" >&2
  exit 1
fi

if [[ -z "$target_image" ]]; then
  wheel_container="$(docker create ghcr.io/selkies-project/selkies/py-build:main)"
  docker cp "$wheel_container:/opt/pypi/dist/selkies-0.0.0.dev0-py3-none-any.whl" "$out_dir/selkies-0.0.0.dev0-py3-none-any.whl"
  docker rm "$wheel_container" >/dev/null
  wheel_container=""
  wheel_mount_args=(-v "$out_dir/selkies-0.0.0.dev0-py3-none-any.whl:/tmp/selkies-0.0.0.dev0-py3-none-any.whl:ro")
  container_bootstrap_commands="export DEBIAN_FRONTEND=noninteractive
sudo-root apt-get update >/tmp/apt.log
sudo-root apt-get install --no-install-recommends -y libxkbcommon-dev libnss3 libnspr4 >/tmp/apt-install.log
python3 -m pip install --break-system-packages --no-cache-dir /tmp/selkies-0.0.0.dev0-py3-none-any.whl >/tmp/selkies-install.log"
fi

docker run --rm -d \
  --name "$container_name" \
  --network=host \
  --shm-size=1g \
  -v "$out_dir:$out_dir" \
  "${wheel_mount_args[@]}" \
  -v "$browser_dir:/opt/elastos-chromium:ro" \
  -v "$native_proxy_bin:/opt/elastos-browser-native-proxy-engine:ro" \
  -v "$profile_dir:$container_profile_dir" \
  --entrypoint bash \
  "$docker_image" \
  -lc "
set -e
$container_bootstrap_commands
export DISPLAY=$selkies_display
export XDG_RUNTIME_DIR=/tmp/runtime-ubuntu
export PIPEWIRE_RUNTIME_DIR=\$XDG_RUNTIME_DIR
export PULSE_RUNTIME_PATH=\$XDG_RUNTIME_DIR/pulse
export PULSE_SERVER=unix:\$PULSE_RUNTIME_PATH/native
mkdir -p \"\$XDG_RUNTIME_DIR\" \"\$PULSE_RUNTIME_PATH\" \"$container_profile_dir\"
mkdir -p /tmp/.X11-unix
chmod 1777 /tmp/.X11-unix
rm -f /tmp/.X11-unix/X$selkies_display_number /tmp/.X$selkies_display_number-lock
/usr/bin/Xvfb \"\$DISPLAY\" -screen 0 ${selkies_width}x${selkies_height}x24 +extension COMPOSITE +extension DAMAGE +extension GLX +extension RANDR +extension RENDER +extension MIT-SHM +extension XFIXES +extension XTEST -nolisten tcp -ac -noreset >/tmp/Xvfb-current.log 2>&1 &
until [ -S /tmp/.X11-unix/X$selkies_display_number ]; do sleep .1; done
(dbus-run-session -- /usr/bin/pipewire >/tmp/pipewire-current.log 2>&1 &)
(dbus-run-session -- /usr/bin/wireplumber >/tmp/wireplumber-current.log 2>&1 &)
(dbus-run-session -- /usr/bin/pipewire-pulse >/tmp/pipewire-pulse-current.log 2>&1 &)
for attempt in \$(seq 1 80); do
  if command -v pw-cli >/dev/null 2>&1 && pw-cli ls Node 2>/dev/null | grep -q 'node.name = \"output\"'; then
    break
  fi
  if command -v pw-cli >/dev/null 2>&1 && pw-cli create-node adapter '{ factory.name=support.null-audio-sink node.name=output node.description=output media.class=Audio/Sink object.linger=true audio.position=[FL FR] }' >/tmp/pipewire-null-sink.log 2>&1; then
    break
  fi
  sleep 0.25
done
ELASTOS_BROWSER_NATIVE_PROXY_ENGINE_CONFIG='{
  \"schema\":\"elastos.browser.native-proxy-engine.config/v1\",
  \"browser_program\":\"/opt/elastos-chromium/chrome\",
  \"relay_ipc_path\":\"$relay_socket\",
  \"startup_grace_ms\":1000,
  \"browser_args\":[
    \"--proxy-server={proxy_url}\",
    \"--proxy-bypass-list=<-loopback>\",
    \"--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1\",
    \"--no-sandbox\",
    \"--disable-dev-shm-usage\",
    \"--no-first-run\",
    \"--disable-background-networking\",
    \"--disable-component-update\",
    \"--disable-default-apps\",
    \"--disable-infobars\",
    \"--disable-sync\",
    \"--disable-quic\",
    \"--autoplay-policy=no-user-gesture-required\",
    \"--kiosk\",
    \"--start-fullscreen\",
    \"--window-position=0,0\",
    \"--window-size=$selkies_width,$selkies_height\",
    \"--force-device-scale-factor=1\",
    \"--app=about:blank\",
    \"--user-data-dir=$container_profile_dir\",
    \"--remote-debugging-address=127.0.0.1\",
    \"--remote-debugging-port=$cdp_port\"
  ]
}' \
ELASTOS_BROWSER_ENGINE_URL='about:blank' \
ELASTOS_BROWSER_ENGINE_STREAM_ID='stream:selkies-runtime-exit-target' \
  /opt/elastos-browser-native-proxy-engine >/tmp/native-proxy-engine.log 2>/tmp/chromium-current.log &
python3 -m selkies \
  --addr=127.0.0.1 \
  --port=$selkies_port \
  --mode=webrtc \
  --audio-enabled=true \
  --microphone-enabled=false \
  --gamepad-enabled=false \
  --clipboard-enabled=true \
  --file-transfers=none \
  --basic-auth-user '$selkies_auth_user' \
  --basic-auth-password '$selkies_auth_password' \
  --encoder='$selkies_encoder' \
  --framerate=$selkies_framerate \
  --video-bitrate=$selkies_video_bitrate \
  --h264-crf=$selkies_h264_crf \
  --h264-streaming-mode=true \
  --enable-resize=false \
  --use-paint-over-quality=true \
  --paint-over-jpeg-quality=95 \
  --manual-width=$selkies_width \
  --manual-height=$selkies_height \
  --is-manual-resolution-mode=true >/tmp/selkies-current.log 2>&1
" >/dev/null

for ((attempt = 0; attempt < timeout_seconds * 10; attempt += 1)); do
  if curl -fsS -m 1 "http://127.0.0.1:$cdp_port/json/version" >/dev/null 2>&1 \
    && curl -sS -m 1 -i "http://127.0.0.1:$selkies_port/health" 2>/dev/null | grep -Eq 'HTTP/[0-9.]+ (200|401)'; then
    break
  fi
  sleep 0.1
done
if ! curl -fsS -m 2 "http://127.0.0.1:$cdp_port/json/version" >/dev/null 2>&1; then
  echo "Chromium CDP endpoint did not become reachable within ${timeout_seconds}s" >&2
  exit 1
fi
if ! curl -sS -m 2 -i "http://127.0.0.1:$selkies_port/health" 2>/dev/null | grep -Eq 'HTTP/[0-9.]+ (200|401)'; then
  echo "Selkies endpoint did not become reachable within ${timeout_seconds}s" >&2
  exit 1
fi

for _ in {1..600}; do
  if docker exec "$container_name" bash -lc 'test -S /tmp/selkies_js0.sock && test -S /tmp/selkies_event1000.sock' >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done
if ! docker exec "$container_name" bash -lc 'test -S /tmp/selkies_js0.sock && test -S /tmp/selkies_event1000.sock' >/dev/null 2>&1; then
  echo "Selkies did not create session sockets" >&2
  exit 1
fi
sleep 5

ice_servers_json="$(node -e 'console.log(JSON.stringify(process.argv.slice(1)))' "${ice_servers[@]}")"
control_config="$(node -e '
const [controlSocket, selkiesPort, cdpPort, authUser, authPassword, iceServersRaw, iceUsername, iceCredential, targetContainerName] = process.argv.slice(1);
const iceUrls = JSON.parse(iceServersRaw);
if (!Array.isArray(iceUrls) || iceUrls.length > 8) {
  throw new Error("--ice-server may be repeated at most 8 times");
}
for (const url of iceUrls) {
  if (typeof url !== "string" || !/^(stun|turns?):/i.test(url.trim()) || /[\r\n\0]/.test(url) || url.trim().length > 512) {
    throw new Error("--ice-server must be a stun:, turn:, or turns: URL without control characters");
  }
}
if (!/^elastos-selkies-runtime-exit-target-[0-9]+$/.test(targetContainerName)) {
  throw new Error("invalid target container name");
}
const config = {
  schema: "elastos.browser.selkies-control.config/v1",
  control_socket_path: controlSocket,
  replace_existing_socket: true,
  target_container_name: targetContainerName,
  selkies_ws_url: `ws://127.0.0.1:${selkiesPort}/webrtc/signaling`,
  browser_control: {
    kind: "cdp_http",
    endpoint: `http://127.0.0.1:${cdpPort}`,
    timeout_ms: 5000
  },
  basic_auth: {
    user: authUser,
    password: authPassword
  },
  display_surface: {
    stream_width: Number("$selkies_width"),
    stream_height: Number("$selkies_height"),
    css_width: Number("$selkies_width"),
    css_height: Number("$selkies_height")
  },
  connect_timeout_ms: 30000,
  signal_timeout_ms: 30000
};
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
' "$control_socket" "$selkies_port" "$cdp_port" "$selkies_auth_user" "$selkies_auth_password" "$ice_servers_json" "$ice_username" "$ice_credential" "$container_name")"
ELASTOS_BROWSER_SELKIES_CONTROL_CONFIG="$control_config" \
  scripts/browser-selkies-control-service.mjs >"$control_log" 2>&1 &
control_pid="$!"
for _ in {1..200}; do
  [[ -S "$control_socket" ]] && break
  sleep 0.025
done
if [[ ! -S "$control_socket" ]]; then
  echo "Selkies control service did not create socket" >&2
  exit 1
fi

node scripts/browser-hosted-product-operator-config.mjs \
  --out-dir "$out_dir" \
  --adapter-id "$adapter_id" \
  --supervisor-program "$repo_root/scripts/browser-hosted-product-supervisor.mjs" \
  --control-socket "$control_socket" >/dev/null

if (( verify )); then
  VERIFY_URL="$verify_url" DEBUG_PORT="$cdp_port" node --input-type=module - <<'NODE'
import playwright from './elastos/tools/browser-playwright-engine/node_modules/playwright/index.js';
const { chromium } = playwright;
const browser = await chromium.connectOverCDP(`http://127.0.0.1:${process.env.DEBUG_PORT}`);
const context = browser.contexts()[0] || await browser.newContext();
const page = context.pages()[0] || await context.newPage();
await page.goto(process.env.VERIFY_URL, { waitUntil: 'load', timeout: 30000 });
const title = await page.title();
const text = await page.locator('body').innerText({ timeout: 5000 }).catch(() => '');
await browser.close().catch(() => {});
if (!title && !text) {
  throw new Error(`Runtime Exit page load produced no title/body for ${process.env.VERIFY_URL}`);
}
NODE
  scripts/browser-hosted-product-webrtc-smoke.sh \
    --adapter-config "$out_dir/browser-engine-adapter.json" \
    --url "$verify_url" >/dev/null
fi

profile_dir_json="$(node -e 'process.stdout.write(JSON.stringify(process.argv[1]))' "$profile_dir")"
node - <<NODE
console.log(JSON.stringify({
  ok: true,
  schema: "elastos.browser.selkies-runtime-exit-target/v1",
  out_dir: "$out_dir",
  browser_engine_adapter_config: "$out_dir/browser-engine-adapter.json",
  control_socket: "$control_socket",
  relay_socket: "$relay_socket",
  selkies_ws_url: "ws://127.0.0.1:$selkies_port/webrtc/signaling",
  browser_cdp_endpoint: "http://127.0.0.1:$cdp_port",
  container_name: "$container_name",
  adapter_id: "$adapter_id",
  display_backend: "selkies_gstreamer_webrtc",
  backend_class: "product_compositor",
  audio: true,
  video: true,
  direct_network: false,
  runtime_exit: true,
  profile_persistent: true,
  profile_dir: $profile_dir_json,
  ice_servers_configured: Boolean($([[ ${#ice_servers[@]} -gt 0 ]] && echo true || echo false)),
  selkies_encoder: "$selkies_encoder",
  selkies_framerate: Number("$selkies_framerate"),
  selkies_video_bitrate_mbps: Number("$selkies_video_bitrate"),
  selkies_resolution: "$selkies_width" + "x" + "$selkies_height",
  selkies_resolution_mode: "fixed",
  target_image: "$docker_image",
  prebuilt_target_image: Boolean("$target_image"),
  verified: Boolean($verify)
}));
NODE

if (( verify && cleanup_after_verify )); then
  exit 0
fi

while docker ps --format '{{.Names}}' | grep -Fxq "$container_name"; do
  if ! kill -0 "$local_exit_pid" >/dev/null 2>&1; then
    echo "browser-local-exit exited" >&2
    exit 1
  fi
  if ! kill -0 "$control_pid" >/dev/null 2>&1; then
    echo "Selkies control service exited" >&2
    exit 1
  fi
  sleep 2
done
echo "Selkies target container exited" >&2
exit 1
