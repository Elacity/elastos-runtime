#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"
container_name="elastos-current-selkies-smoke-$$"
wheel_container=""
cdp_pid=""
timeout_seconds="${ELASTOS_BROWSER_SELKIES_SMOKE_TIMEOUT_SECONDS:-300}"

cleanup() {
  local status="$?"
  if [[ "$status" != "0" ]]; then
    echo "current Selkies wheel smoke failed; diagnostics follow" >&2
    docker ps --filter name="$container_name" >&2 || true
    docker logs --tail 120 "$container_name" >&2 || true
    docker exec "$container_name" bash -lc 'tail -160 /tmp/selkies-current.log /tmp/Xvfb-current.log /tmp/pipewire-current.log /tmp/pipewire-pulse-current.log /tmp/wireplumber-current.log /tmp/selkies-install.log 2>/dev/null || true' >&2 || true
    if [[ -d "$tmp_dir/preflight" ]]; then
      find "$tmp_dir/preflight" -maxdepth 3 -type f -print -exec sed -n "1,160p" {} \; >&2 || true
    fi
  fi
  if [[ -n "$cdp_pid" ]]; then
    kill "$cdp_pid" >/dev/null 2>&1 || true
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
  echo "docker is required for current Selkies wheel smoke" >&2
  exit 1
fi

port="$(node -e '
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
  --entrypoint bash \
  ghcr.io/selkies-project/selkies-gstreamer/gst-py-example:main-ubuntu24.04 \
  -lc "
set -e
export DEBIAN_FRONTEND=noninteractive
sudo-root apt-get update >/tmp/apt.log
sudo-root apt-get install --no-install-recommends -y libxkbcommon-dev >/tmp/apt-install.log
python3 -m pip install --break-system-packages --no-cache-dir /tmp/selkies-0.0.0.dev0-py3-none-any.whl >/tmp/selkies-install.log
export DISPLAY=:43
export XDG_RUNTIME_DIR=/tmp/runtime-ubuntu
export PIPEWIRE_RUNTIME_DIR=\$XDG_RUNTIME_DIR
export PULSE_RUNTIME_PATH=\$XDG_RUNTIME_DIR/pulse
export PULSE_SERVER=unix:\$PULSE_RUNTIME_PATH/native
mkdir -p \"\$XDG_RUNTIME_DIR\" \"\$PULSE_RUNTIME_PATH\"
/usr/bin/Xvfb \"\$DISPLAY\" -screen 0 1920x1080x24 +extension COMPOSITE +extension DAMAGE +extension GLX +extension RANDR +extension RENDER +extension MIT-SHM +extension XFIXES +extension XTEST -nolisten tcp -ac -noreset >/tmp/Xvfb-current.log 2>&1 &
until [ -S /tmp/.X11-unix/X43 ]; do sleep .1; done
(dbus-run-session -- /usr/bin/pipewire >/tmp/pipewire-current.log 2>&1 &)
(dbus-run-session -- /usr/bin/wireplumber >/tmp/wireplumber-current.log 2>&1 &)
(dbus-run-session -- /usr/bin/pipewire-pulse >/tmp/pipewire-pulse-current.log 2>&1 &)
python3 -m selkies \
  --addr=127.0.0.1 \
  --port=$port \
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

if ! [[ "$timeout_seconds" =~ ^[0-9]+$ ]] || (( timeout_seconds < 30 || timeout_seconds > 1800 )); then
  echo "ELASTOS_BROWSER_SELKIES_SMOKE_TIMEOUT_SECONDS must be 30..1800" >&2
  exit 2
fi

for ((attempt = 0; attempt < timeout_seconds * 10; attempt += 1)); do
  if curl -sS -m 1 -i "http://127.0.0.1:$port/health" 2>/dev/null | grep -Eq 'HTTP/[0-9.]+ (200|401)'; then
    break
  fi
  sleep 0.1
done
if ! curl -sS -m 2 -i "http://127.0.0.1:$port/health" 2>/dev/null | grep -Eq 'HTTP/[0-9.]+ (200|401)'; then
  echo "current Selkies wheel did not become reachable within ${timeout_seconds}s" >&2
  exit 1
fi

for _ in {1..600}; do
  if docker exec "$container_name" bash -lc 'test -S /tmp/selkies_js0.sock && test -S /tmp/selkies_event1000.sock' >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done
if ! docker exec "$container_name" bash -lc 'test -S /tmp/selkies_js0.sock && test -S /tmp/selkies_event1000.sock' >/dev/null 2>&1; then
  echo "current Selkies wheel did not create session sockets" >&2
  exit 1
fi
# The HTTP endpoint can become reachable before the internal Selkies server peer
# has registered with the signaling server. Without this short settle window the
# first controller can be accepted and then closed before SESSION_OK.
sleep 5

cat >"$tmp_dir/fake-cdp.mjs" <<'NODE'
import http from "node:http";
import fs from "node:fs";

const readyPath = process.argv[2];
const server = http.createServer((req, res) => {
  const url = new URL(req.url, "http://127.0.0.1");
  if (req.method !== "PUT" || url.pathname !== "/json/new") {
    res.writeHead(404, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: "not found" }));
    return;
  }
  const requestedUrl = decodeURIComponent(url.search.slice(1));
  res.writeHead(200, { "content-type": "application/json" });
  res.end(JSON.stringify({
    id: "target-smoke",
    type: "page",
    title: "Example Domain",
    url: requestedUrl,
    webSocketDebuggerUrl: "ws://127.0.0.1/devtools/page/target-smoke"
  }));
});

server.listen(0, "127.0.0.1", () => {
  fs.writeFileSync(readyPath, JSON.stringify({ port: server.address().port }));
});
NODE

node "$tmp_dir/fake-cdp.mjs" "$tmp_dir/fake-cdp-ready.json" >"$tmp_dir/fake-cdp.log" 2>&1 &
cdp_pid="$!"
for _ in {1..100}; do
  [[ -s "$tmp_dir/fake-cdp-ready.json" ]] && break
  sleep 0.02
done
if [[ ! -s "$tmp_dir/fake-cdp-ready.json" ]]; then
  echo "fake CDP endpoint did not start" >&2
  cat "$tmp_dir/fake-cdp.log" >&2 || true
  exit 1
fi
cdp_port="$(node -e 'console.log(JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")).port)' "$tmp_dir/fake-cdp-ready.json")"

scripts/browser-selkies-target-preflight.sh \
  --out-dir "$tmp_dir/preflight" \
  --control-socket "$tmp_dir/preflight/control.sock" \
  --selkies-ws-url "ws://127.0.0.1:$port/webrtc/signaling" \
  --browser-cdp-endpoint "http://127.0.0.1:$cdp_port" \
  --selkies-basic-auth-user ubuntu \
  --selkies-basic-auth-password mypasswd

node - <<'NODE' "$port"
const [port] = process.argv.slice(2);
console.log(JSON.stringify({
  ok: true,
  schema: "elastos.browser.selkies-current-wheel-smoke/v1",
  selkies_ws_url: `ws://127.0.0.1:${port}/webrtc/signaling`,
  display_backend: "selkies_gstreamer_webrtc",
  backend_class: "product_compositor",
  audio: true,
  video: true,
  direct_network: false
}));
NODE
