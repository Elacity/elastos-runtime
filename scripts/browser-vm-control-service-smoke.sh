#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"
service_pid=""
guest_pid=""

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

cleanup() {
  if [[ -n "$guest_pid" ]]; then
    kill "$guest_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "$service_pid" ]]; then
    kill "$service_pid" >/dev/null 2>&1 || true
  fi
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

control_socket="$tmp_dir/browser-vm-control.sock"
guest_control_socket="$tmp_dir/browser-vm-guest-control.sock"
fake_launcher="$tmp_dir/fake-vm-launcher.mjs"
fake_guest_control="$tmp_dir/fake-guest-control.mjs"

cat > "$fake_guest_control" <<'NODE'
#!/usr/bin/env node
import fs from "node:fs";
import http from "node:http";

const socketPath = process.env.FAKE_GUEST_CONTROL_SOCKET;
if (!socketPath) {
  console.error("FAKE_GUEST_CONTROL_SOCKET is required");
  process.exit(2);
}
try {
  fs.unlinkSync(socketPath);
} catch {}

function readJson(req) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    req.on("data", (chunk) => chunks.push(chunk));
    req.on("end", () => {
      try {
        resolve(JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}"));
      } catch (error) {
        reject(error);
      }
    });
    req.on("error", reject);
  });
}

const server = http.createServer(async (req, res) => {
  const path = new URL(req.url, "http://browser-engine").pathname;
  const webrtc = path.match(/^\/pages\/([^/]+)\/webrtc$/);
  const close = path.match(/^\/pages\/([^/]+)\/close$/);
  if (req.method === "POST" && webrtc) {
    const body = await readJson(req);
    const signal = body.signal || {};
    const channel = body.channel || "video";
    const type = signal.type || signal.schema?.split(".").at(-1)?.replace("/v1", "") || "signal";
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({
      schema: "elastos.browser.webrtc-signal-ack/v1",
      type,
      channel,
      page_id: decodeURIComponent(webrtc[1]),
    }));
    return;
  }
  if (req.method === "POST" && close) {
    await readJson(req);
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({
      schema: "elastos.browser.page-close/v1",
      ok: true,
      page_id: decodeURIComponent(close[1]),
    }));
    return;
  }
  res.writeHead(404, { "content-type": "application/json" });
  res.end(JSON.stringify({ error: "not found" }));
});

server.listen(socketPath, () => {
  process.stdout.write(JSON.stringify({
    schema: "elastos.browser.fake-guest-control.ready/v1",
    socket_path: socketPath,
  }) + "\n");
});
NODE
chmod 755 "$fake_guest_control"

cat > "$fake_launcher" <<'NODE'
#!/usr/bin/env node
const chunks = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
const launch = body.launch_request;
const pageId = `page:vm-control-service-smoke-${launch.stream_id.replace(/[^A-Za-z0-9_-]/g, "_")}`;
console.log(JSON.stringify({
  schema: "elastos.browser.engine.supervisor-result/v1",
  page_id: pageId,
  adapter: launch.adapter,
  engine: launch.engine,
  stream_id: launch.stream_id,
  actual_url: launch.url,
  title: "Browser VM Control Service Smoke",
  network_mode: "runtime_net_only",
  direct_network: false,
  wallet_injection: false,
  control_socket_path: process.env.FAKE_GUEST_CONTROL_SOCKET,
  isolated_session: true,
  isolation: {
    schema: "elastos.browser.engine.isolation/v1",
    kind: "per_launch_vm_target",
    session_dir: "/tmp/elastos-browser-vm-sessions/vm-control-service-smoke",
  },
  display_session: {
    schema: "elastos.browser.display-session/v1",
    session_id: `display:${launch.stream_id}`,
    mode: "webrtc_remote_display",
    input: "datachannel",
    width: 1280,
    height: 720,
    offerer: "engine",
    initial_offer: {
      schema: "elastos.browser.webrtc-offer/v1",
      type: "offer",
      sdp: "v=0\r\ns=Browser VM Control Service Smoke\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n",
    },
    audio_offer: {
      schema: "elastos.browser.webrtc-offer/v1",
      type: "offer",
      sdp: "v=0\r\ns=Browser VM Control Service Smoke Audio\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n",
    },
    display_backend: "vm_selkies_gstreamer_webrtc",
    backend_class: "product_compositor",
    media_transport: "runtime_relay",
    audio: true,
    video: true,
    network_mode: "runtime_net_only",
    direct_network: false,
    signaling_url: `/api/apps/browser/pages/${encodeURIComponent(pageId)}/webrtc`,
  },
}));
NODE
chmod 755 "$fake_launcher"

FAKE_GUEST_CONTROL_SOCKET="$guest_control_socket" \
  "$node_bin" "$fake_guest_control" > "$tmp_dir/guest.out" 2> "$tmp_dir/guest.err" &
guest_pid="$!"

for _ in {1..100}; do
  [[ -S "$guest_control_socket" ]] && break
  sleep 0.05
done
if [[ ! -S "$guest_control_socket" ]]; then
  cat "$tmp_dir/guest.err" >&2 || true
  exit 1
fi

config_json="$(python3 - <<PY
import json
print(json.dumps({
    "schema": "elastos.browser.vm-control-service.config/v1",
    "control_socket_path": "$control_socket",
    "launcher_program": "$fake_launcher",
    "replace_existing_socket": True,
    "max_active_pages": 1,
    "launch_timeout_ms": 30000,
}))
PY
)"

FAKE_GUEST_CONTROL_SOCKET="$guest_control_socket" \
ELASTOS_BROWSER_VM_CONTROL_SERVICE_CONFIG="$config_json" \
  "$node_bin" "$repo_root/scripts/browser-vm-control-service.mjs" > "$tmp_dir/service.out" 2> "$tmp_dir/service.err" &
service_pid="$!"

for _ in {1..100}; do
  [[ -S "$control_socket" ]] && break
  sleep 0.05
done
if [[ ! -S "$control_socket" ]]; then
  cat "$tmp_dir/service.err" >&2 || true
  exit 1
fi

CONTROL_SOCKET="$control_socket" "$node_bin" - <<'NODE'
const http = require("node:http");
const socketPath = process.env.CONTROL_SOCKET;

function request(method, path, body) {
  const bytes = body ? Buffer.from(JSON.stringify(body)) : Buffer.alloc(0);
  return new Promise((resolve, reject) => {
    const req = http.request({
      socketPath,
      path,
      method,
      headers: {
        "content-type": "application/json",
        "content-length": bytes.length,
      },
    }, (res) => {
      const chunks = [];
      res.on("data", (chunk) => chunks.push(chunk));
      res.on("end", () => {
        const parsed = JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}");
        if (res.statusCode < 200 || res.statusCode >= 300) reject(new Error(parsed.error || `status ${res.statusCode}`));
        else resolve(parsed);
      });
    });
    req.on("error", reject);
    req.end(bytes);
  });
}

(async () => {
  const openRequest = (url, streamId = "stream:vm-control-smoke") => ({
    schema: "elastos.browser.vm-engine.open/v1",
    launch_request: {
      schema: "elastos.browser.engine.launch-request/v1",
      adapter: "browser-vm-product",
      engine: "chromium_microvm",
      url,
      stream_id: streamId,
      target: "tls://example.com:443",
      principal_id: "person:local:vm-control-smoke",
      network_mode: "runtime_net_only",
      direct_network: false,
      wallet_injection: false,
      display_mode: "webrtc_remote_display",
      guarantee_level: "mechanism_microvm",
    },
    requirements: {
      substrate: "microvm",
      display_mode: "webrtc_remote_display",
      backend_class: "product_compositor",
      network_mode: "runtime_net_only",
      direct_network: false,
    },
  });
  const status = await request("GET", "/status");
  if (status.schema !== "elastos.browser.vm-control-service.status/v1") throw new Error("wrong status schema");
  if (!Number.isInteger(status.pid) || status.pid <= 1) throw new Error("status must expose pid");
  if (!Date.parse(status.started_at || "")) throw new Error("status must expose started_at");
  if (!Number.isFinite(Number(status.uptime_ms)) || Number(status.uptime_ms) < 0) throw new Error("status must expose uptime_ms");
  if (status.hibernation_mode !== "off") throw new Error(`status must report hibernation mode: ${JSON.stringify(status)}`);
  const launch = await request("POST", "/pages", openRequest("https://example.com/"));
  if (launch.schema !== "elastos.browser.engine.supervisor-result/v1") throw new Error("wrong launch schema");
  if (launch.engine !== "chromium_microvm") throw new Error("wrong engine");
  if (launch.display_session.media_transport !== "runtime_relay") throw new Error("missing runtime relay media transport");
  if (launch.display_session.audio !== true || launch.display_session.video !== true) throw new Error("split audio/video offers did not normalize to audio+video");
  const webrtc = await request("POST", `/pages/${encodeURIComponent(launch.page_id)}/webrtc`, {
    signal: {
      schema: "elastos.browser.webrtc-answer/v1",
      type: "answer",
      sdp: "v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n",
    },
    channel: "video",
  });
  if (webrtc.schema !== "elastos.browser.webrtc-signal-ack/v1" || webrtc.type !== "answer" || webrtc.channel !== "video") {
    throw new Error(`WebRTC proxy did not return signal ack: ${JSON.stringify(webrtc)}`);
  }
  const replaced = await request("POST", "/pages", openRequest("https://example.org/"));
  if (replaced.actual_url !== "https://example.org/") throw new Error("same-stream replacement did not launch the new URL");
  if (replaced.page_id !== launch.page_id) throw new Error("same-stream replacement should preserve the active page identity contract");
  const replacedStream = await request("POST", "/pages", openRequest("https://example.net/", "stream:other-vm-control-smoke"));
  if (replacedStream.actual_url !== "https://example.net/") throw new Error("second stream did not launch the new URL");
  if (replacedStream.stream_id !== "stream:other-vm-control-smoke") throw new Error("second stream returned the wrong stream");
  if (replacedStream.page_id === replaced.page_id) throw new Error("different stream must get a replacement page identity");
  const statusAfterReplacement = await request("GET", "/status");
  if (statusAfterReplacement.active_pages !== 1 || statusAfterReplacement.max_active_pages !== 1 || statusAfterReplacement.capacity_available !== false) {
    throw new Error(`single-page replacement did not preserve one active page: ${JSON.stringify(statusAfterReplacement)}`);
  }
  const shutdown = await request("POST", "/shutdown", { page_id: replacedStream.page_id });
  if (shutdown.schema !== "elastos.browser.vm-engine.shutdown/v1" || shutdown.ok !== true) throw new Error("shutdown failed");
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
NODE

printf '%s\n' '{"schema":"elastos.browser.vm-control-service-smoke/v1","ok":true}'
