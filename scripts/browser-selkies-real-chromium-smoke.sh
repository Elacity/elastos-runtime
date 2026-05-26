#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"
container_name="elastos-selkies-chromium-smoke-$$"
wheel_container=""
timeout_seconds="${ELASTOS_BROWSER_SELKIES_SMOKE_TIMEOUT_SECONDS:-300}"

cleanup() {
  local status="$?"
  if [[ "$status" != "0" ]]; then
    echo "Selkies real-Chromium smoke failed; diagnostics follow" >&2
    docker ps --filter name="$container_name" >&2 || true
    docker logs --tail 120 "$container_name" >&2 || true
    docker exec "$container_name" bash -lc 'tail -180 /tmp/selkies-current.log /tmp/chromium-current.log /tmp/Xvfb-current.log /tmp/pipewire-current.log /tmp/pipewire-pulse-current.log /tmp/wireplumber-current.log /tmp/selkies-install.log 2>/dev/null || true' >&2 || true
    if [[ -d "$tmp_dir/preflight" ]]; then
      find "$tmp_dir/preflight" -maxdepth 3 -type f -print -exec sed -n "1,180p" {} \; >&2 || true
    fi
  fi
  docker rm -f "$container_name" >/dev/null 2>&1 || true
  if [[ -n "$wheel_container" ]]; then
    docker rm "$wheel_container" >/dev/null 2>&1 || true
  fi
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

cd "$repo_root"

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required for Selkies real-Chromium smoke" >&2
  exit 1
fi
if ! [[ "$timeout_seconds" =~ ^[0-9]+$ ]] || (( timeout_seconds < 30 || timeout_seconds > 1800 )); then
  echo "ELASTOS_BROWSER_SELKIES_SMOKE_TIMEOUT_SECONDS must be 30..1800" >&2
  exit 2
fi

browser_program="${BROWSER_SELKIES_CHROMIUM_PROGRAM:-}"
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
browser_dir="$(dirname "$browser_program")"

selkies_port="$(node -e '
const net = require("node:net");
const server = net.createServer();
server.listen(0, "127.0.0.1", () => {
  const port = server.address().port;
  server.close(() => console.log(port));
});
')"
cdp_port="$(node -e '
const net = require("node:net");
const server = net.createServer();
server.listen(0, "127.0.0.1", () => {
  const port = server.address().port;
  server.close(() => console.log(port));
});
')"

wheel_container="$(docker create ghcr.io/selkies-project/selkies/py-build:main)"
docker cp "$wheel_container:/opt/pypi/dist/selkies-0.0.0.dev0-py3-none-any.whl" "$tmp_dir/selkies-0.0.0.dev0-py3-none-any.whl"
docker rm "$wheel_container" >/dev/null
wheel_container=""

docker run --rm -d \
  --name "$container_name" \
  --network=host \
  --shm-size=1g \
  -v "$tmp_dir/selkies-0.0.0.dev0-py3-none-any.whl:/tmp/selkies-0.0.0.dev0-py3-none-any.whl:ro" \
  -v "$browser_dir:/opt/elastos-chromium:ro" \
  --entrypoint bash \
  ghcr.io/selkies-project/selkies-gstreamer/gst-py-example:main-ubuntu24.04 \
  -lc "
set -e
export DEBIAN_FRONTEND=noninteractive
sudo-root apt-get update >/tmp/apt.log
sudo-root apt-get install --no-install-recommends -y libxkbcommon-dev libnss3 libnspr4 >/tmp/apt-install.log
python3 -m pip install --break-system-packages --no-cache-dir /tmp/selkies-0.0.0.dev0-py3-none-any.whl >/tmp/selkies-install.log
export DISPLAY=:43
export XDG_RUNTIME_DIR=/tmp/runtime-ubuntu
export PIPEWIRE_RUNTIME_DIR=\$XDG_RUNTIME_DIR
export PULSE_RUNTIME_PATH=\$XDG_RUNTIME_DIR/pulse
export PULSE_SERVER=unix:\$PULSE_RUNTIME_PATH/native
mkdir -p \"\$XDG_RUNTIME_DIR\" \"\$PULSE_RUNTIME_PATH\" /tmp/chromium-profile
/usr/bin/Xvfb \"\$DISPLAY\" -screen 0 1920x1080x24 +extension COMPOSITE +extension DAMAGE +extension GLX +extension RANDR +extension RENDER +extension MIT-SHM +extension XFIXES +extension XTEST -nolisten tcp -ac -noreset >/tmp/Xvfb-current.log 2>&1 &
until [ -S /tmp/.X11-unix/X43 ]; do sleep .1; done
(dbus-run-session -- /usr/bin/pipewire >/tmp/pipewire-current.log 2>&1 &)
(dbus-run-session -- /usr/bin/wireplumber >/tmp/wireplumber-current.log 2>&1 &)
(dbus-run-session -- /usr/bin/pipewire-pulse >/tmp/pipewire-pulse-current.log 2>&1 &)
/opt/elastos-chromium/chrome \
  --no-sandbox \
  --disable-dev-shm-usage \
  --no-first-run \
  --disable-background-networking \
  --disable-component-update \
  --disable-default-apps \
  --disable-sync \
  --disable-quic \
  --autoplay-policy=no-user-gesture-required \
  --user-data-dir=/tmp/chromium-profile \
  --host-resolver-rules='MAP * ~NOTFOUND, EXCLUDE 127.0.0.1' \
  --remote-debugging-address=127.0.0.1 \
  --remote-debugging-port=$cdp_port \
  about:blank >/tmp/chromium-current.log 2>&1 &
python3 -m selkies \
  --addr=127.0.0.1 \
  --port=$selkies_port \
  --mode=webrtc \
  --audio-enabled=true \
  --microphone-enabled=false \
  --gamepad-enabled=false \
  --clipboard-enabled=true \
  --file-transfers=none \
  --encoder=jpeg \
  --framerate=30 \
  --video-bitrate=8 \
  --manual-width=1280 \
  --manual-height=720 \
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

scripts/browser-selkies-target-preflight.sh \
  --out-dir "$tmp_dir/preflight" \
  --control-socket "$tmp_dir/preflight/control.sock" \
  --selkies-ws-url "ws://127.0.0.1:$selkies_port/webrtc/signaling" \
  --browser-cdp-endpoint "http://127.0.0.1:$cdp_port" \
  --selkies-basic-auth-user ubuntu \
  --selkies-basic-auth-password mypasswd

node - <<'NODE' "$selkies_port" "$cdp_port"
const [selkiesPort, cdpPort] = process.argv.slice(2);
console.log(JSON.stringify({
  ok: true,
  schema: "elastos.browser.selkies-real-chromium-smoke/v1",
  selkies_ws_url: `ws://127.0.0.1:${selkiesPort}/webrtc/signaling`,
  browser_cdp_endpoint: `http://127.0.0.1:${cdpPort}`,
  display_backend: "selkies_gstreamer_webrtc",
  backend_class: "product_compositor",
  audio: true,
  video: true,
  direct_network: false,
  browser_control: "real_chromium_cdp"
}));
NODE
