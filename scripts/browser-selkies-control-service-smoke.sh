#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"
selkies_pid=""
cdp_pid=""
control_pid=""

cleanup() {
  if [[ -n "$control_pid" ]]; then
    kill "$control_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "$cdp_pid" ]]; then
    kill "$cdp_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "$selkies_pid" ]]; then
    kill "$selkies_pid" >/dev/null 2>&1 || true
  fi
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

cd "$repo_root"

cat >"$tmp_dir/fake-selkies.mjs" <<'NODE'
import crypto from "node:crypto";
import http from "node:http";
import fs from "node:fs";

const readyPath = process.argv[2];
const pongPath = process.argv[3];
const failedClosePath = process.argv[4];
let failNextSession = true;

const offerSdp = [
  "v=0",
  "o=- 0 0 IN IP4 127.0.0.1",
  "s=ElastOS Browser",
  "t=0 0",
  "m=video 9 UDP/TLS/RTP/SAVPF 96",
  "c=IN IP4 0.0.0.0",
  "a=mid:0",
  "a=sendonly",
  "a=rtcp-mux",
  "a=setup:actpass",
  "a=rtpmap:96 VP8/90000",
  "m=audio 9 UDP/TLS/RTP/SAVPF 111",
  "c=IN IP4 0.0.0.0",
  "a=mid:1",
  "a=sendonly",
  "a=rtcp-mux",
  "a=setup:actpass",
  "a=rtpmap:111 opus/48000/2",
  "",
].join("\r\n");

function sendText(socket, text) {
  const payload = Buffer.from(text);
  const header = [];
  header.push(0x81);
  if (payload.length < 126) {
    header.push(payload.length);
  } else {
    header.push(126, (payload.length >> 8) & 0xff, payload.length & 0xff);
  }
  socket.write(Buffer.concat([Buffer.from(header), payload]));
}

function sendControl(socket, opcode, text) {
  const payload = Buffer.from(text);
  if (payload.length > 125) {
    throw new Error("control payload too large for smoke");
  }
  socket.write(Buffer.concat([Buffer.from([0x80 | opcode, payload.length]), payload]));
}

function readFrame(buffer) {
  if (buffer.length < 2) return null;
  const opcode = buffer[0] & 0x0f;
  const masked = (buffer[1] & 0x80) !== 0;
  let length = buffer[1] & 0x7f;
  let offset = 2;
  if (length === 126) {
    if (buffer.length < offset + 2) return null;
    length = buffer.readUInt16BE(offset);
    offset += 2;
  } else if (length === 127) {
    throw new Error("frame too large for smoke");
  }
  let mask;
  if (masked) {
    if (buffer.length < offset + 4) return null;
    mask = buffer.subarray(offset, offset + 4);
    offset += 4;
  }
  if (buffer.length < offset + length) return null;
  const payload = Buffer.from(buffer.subarray(offset, offset + length));
  if (mask) {
    for (let index = 0; index < payload.length; index += 1) {
      payload[index] ^= mask[index % 4];
    }
  }
  return { opcode, masked, payload, consumed: offset + length };
}

const server = http.createServer();
server.on("upgrade", (req, socket) => {
  const key = req.headers["sec-websocket-key"];
  const accept = crypto
    .createHash("sha1")
    .update(`${key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`)
    .digest("base64");
  socket.write([
    "HTTP/1.1 101 Switching Protocols",
    "Upgrade: websocket",
    "Connection: Upgrade",
    `Sec-WebSocket-Accept: ${accept}`,
    "",
    "",
  ].join("\r\n"));

  let buffer = Buffer.alloc(0);
  let recordFailedClose = false;
  let failedCloseRecorded = false;
  const recordClosed = () => {
    if (recordFailedClose && !failedCloseRecorded) {
      failedCloseRecorded = true;
      fs.writeFileSync(failedClosePath, "ok");
    }
  };
  socket.on("end", recordClosed);
  socket.on("close", recordClosed);
  socket.on("data", (chunk) => {
    buffer = Buffer.concat([buffer, chunk]);
    for (;;) {
      const frame = readFrame(buffer);
      if (!frame) return;
      buffer = buffer.subarray(frame.consumed);
      if (frame.opcode === 0xa) {
        if (!frame.masked || frame.payload.toString("utf8") !== "keepalive") {
          socket.destroy();
          return;
        }
        fs.writeFileSync(pongPath, "ok");
        continue;
      }
      if (frame.opcode === 0x8) {
        recordClosed();
        socket.end();
        return;
      }
      if (frame.opcode !== 0x1) continue;
      const text = frame.payload.toString("utf8");
      if (text.startsWith("HELLO client ")) {
        sendText(socket, "HELLO");
      } else if (text === "SESSION server") {
        if (failNextSession) {
          failNextSession = false;
          recordFailedClose = true;
          sendText(socket, "ERROR no-server-yet");
          return;
        }
        sendText(socket, "SESSION_OK server-peer-1");
        sendText(socket, `server-peer-1 ${JSON.stringify({ sdp: { type: "offer", sdp: offerSdp } })}`);
      } else if (text.startsWith("server-peer-1 ")) {
        const body = JSON.parse(text.slice("server-peer-1 ".length));
        if (body.sdp?.type === "answer") {
          sendControl(socket, 0x9, "keepalive");
          sendText(socket, `server-peer-1 ${JSON.stringify({ ice: { candidate: "candidate:smoke 1 udp 1 127.0.0.1 9 typ host", sdpMid: "0", sdpMLineIndex: 0 } })}`);
        }
      }
    }
  });
});

server.listen(0, "127.0.0.1", () => {
  fs.writeFileSync(readyPath, JSON.stringify({ port: server.address().port }));
});
NODE

node "$tmp_dir/fake-selkies.mjs" "$tmp_dir/fake-selkies-ready.json" "$tmp_dir/fake-selkies-pong.txt" "$tmp_dir/fake-selkies-failed-close.txt" &
selkies_pid="$!"
for _ in {1..100}; do
  [[ -s "$tmp_dir/fake-selkies-ready.json" ]] && break
  sleep 0.02
done
[[ -s "$tmp_dir/fake-selkies-ready.json" ]]
selkies_port="$(node -e 'console.log(JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).port)' "$tmp_dir/fake-selkies-ready.json")"

cat >"$tmp_dir/fake-cdp.mjs" <<'NODE'
import crypto from "node:crypto";
import http from "node:http";
import fs from "node:fs";

const readyPath = process.argv[2];
const firstUrl = "https://example.com/";
const secondUrl = "https://example.com/?elastos-browser-nav-smoke=1";
const requested = {
  newTarget: "",
  navigated: "",
  initScript: "",
  insertedText: "",
  reloads: 0,
};
let historyEntries = [];
let currentIndex = -1;

function sendText(socket, text) {
  const payload = Buffer.from(text);
  const header = [];
  header.push(0x81);
  if (payload.length < 126) {
    header.push(payload.length);
  } else {
    header.push(126, (payload.length >> 8) & 0xff, payload.length & 0xff);
  }
  socket.write(Buffer.concat([Buffer.from(header), payload]));
}

function readFrame(buffer) {
  if (buffer.length < 2) return null;
  const opcode = buffer[0] & 0x0f;
  const masked = (buffer[1] & 0x80) !== 0;
  let length = buffer[1] & 0x7f;
  let offset = 2;
  if (length === 126) {
    if (buffer.length < offset + 2) return null;
    length = buffer.readUInt16BE(offset);
    offset += 2;
  } else if (length === 127) {
    throw new Error("frame too large for smoke");
  }
  let mask;
  if (masked) {
    if (buffer.length < offset + 4) return null;
    mask = buffer.subarray(offset, offset + 4);
    offset += 4;
  }
  if (buffer.length < offset + length) return null;
  const payload = Buffer.from(buffer.subarray(offset, offset + length));
  if (mask) {
    for (let index = 0; index < payload.length; index += 1) {
      payload[index] ^= mask[index % 4];
    }
  }
  return { opcode, payload, consumed: offset + length };
}

const server = http.createServer((req, res) => {
  const url = new URL(req.url, "http://127.0.0.1");
  if (req.method === "GET" && url.pathname === "/json/list") {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify([{
      id: "target-smoke",
      type: "page",
      title: "",
      url: historyEntries[currentIndex]?.url || "about:blank",
      webSocketDebuggerUrl: `ws://127.0.0.1:${server.address().port}/devtools/page/target-smoke`
    }]));
    return;
  }
  if (req.method === "GET" && url.pathname === "/json/activate/target-smoke") {
    res.writeHead(200, { "content-type": "text/plain" });
    res.end("Target activated");
    return;
  }
  if (req.method !== "PUT" || url.pathname !== "/json/new") {
    res.writeHead(404, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: "not found" }));
    return;
  }
  const requestedUrl = url.search.slice(1);
  requested.newTarget = requestedUrl;
  if (requestedUrl !== "about:blank") {
    res.writeHead(400, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: `wrong URL: ${requestedUrl}` }));
    return;
  }
  res.writeHead(200, { "content-type": "application/json" });
  res.end(JSON.stringify({
    id: "target-smoke",
    type: "page",
    title: "",
    url: requestedUrl,
    webSocketDebuggerUrl: `ws://127.0.0.1:${server.address().port}/devtools/page/target-smoke`
  }));
});

server.on("upgrade", (req, socket) => {
  if (req.url !== "/devtools/page/target-smoke") {
    socket.destroy();
    return;
  }
  const key = req.headers["sec-websocket-key"];
  const accept = crypto
    .createHash("sha1")
    .update(`${key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`)
    .digest("base64");
  socket.write([
    "HTTP/1.1 101 Switching Protocols",
    "Upgrade: websocket",
    "Connection: Upgrade",
    `Sec-WebSocket-Accept: ${accept}`,
    "",
    "",
  ].join("\r\n"));

  let buffer = Buffer.alloc(0);
  socket.on("data", (chunk) => {
    buffer = Buffer.concat([buffer, chunk]);
    for (;;) {
      const frame = readFrame(buffer);
      if (!frame) return;
      buffer = buffer.subarray(frame.consumed);
      if (frame.opcode !== 0x1) continue;
      const message = JSON.parse(frame.payload.toString("utf8"));
      if (message.method === "Page.enable" || message.method === "Runtime.enable") {
        sendText(socket, JSON.stringify({ id: message.id, result: {} }));
      } else if (message.method === "Emulation.setDeviceMetricsOverride") {
        requested.viewport = message.params || {};
        fs.writeFileSync(`${readyPath}.viewport`, JSON.stringify(requested.viewport));
        sendText(socket, JSON.stringify({ id: message.id, result: {} }));
      } else if (message.method === "Page.addScriptToEvaluateOnNewDocument") {
        requested.initScript = String(message.params?.source || "");
        sendText(socket, JSON.stringify({ id: message.id, result: { identifier: "wallet-bridge-smoke" } }));
      } else if (message.method === "Runtime.evaluate") {
      if (typeof message.params?.expression === "string" && message.params.expression.includes("JSON.stringify({ url: window.location.href")) {
          const currentUrl = historyEntries[currentIndex]?.url || requested.navigated || firstUrl;
          sendText(socket, JSON.stringify({
            id: message.id,
            result: { result: { type: "string", value: JSON.stringify({ url: currentUrl, title: "Example Domain" }) } }
          }));
        } else {
          try {
            new Function(String(message.params?.expression || ""));
          } catch (error) {
            sendText(socket, JSON.stringify({
              id: message.id,
              exceptionDetails: { text: error.message || "Runtime.evaluate syntax error" }
            }));
            continue;
          }
          sendText(socket, JSON.stringify({ id: message.id, result: { result: { type: "undefined" } } }));
        }
      } else if (message.method === "Page.navigate") {
        requested.navigated = String(message.params?.url || "");
        if (requested.navigated !== firstUrl && requested.navigated !== secondUrl) {
          sendText(socket, JSON.stringify({ id: message.id, error: { message: `wrong navigate URL: ${requested.navigated}` } }));
          continue;
        }
        if (!requested.initScript.includes('"ethereum"') || !requested.initScript.includes("wallet_switchEthereumChain")) {
          sendText(socket, JSON.stringify({ id: message.id, error: { message: "wallet bridge init script was not installed before navigation" } }));
          continue;
        }
        historyEntries = historyEntries.slice(0, currentIndex + 1);
        historyEntries.push({ id: historyEntries.length + 1, url: requested.navigated, title: "Example Domain" });
        currentIndex = historyEntries.length - 1;
        sendText(socket, JSON.stringify({ id: message.id, result: { frameId: "frame-smoke" } }));
        sendText(socket, JSON.stringify({ method: "Page.domContentEventFired", params: { timestamp: 1 } }));
      } else if (message.method === "Page.getNavigationHistory") {
        sendText(socket, JSON.stringify({
          id: message.id,
          result: {
            currentIndex,
            entries: historyEntries
          }
        }));
      } else if (message.method === "Page.navigateToHistoryEntry") {
        const entryId = Number(message.params?.entryId);
        const nextIndex = historyEntries.findIndex((entry) => entry.id === entryId);
        if (nextIndex >= 0) {
          currentIndex = nextIndex;
          requested.navigated = historyEntries[currentIndex].url;
        }
        sendText(socket, JSON.stringify({ id: message.id, result: {} }));
        sendText(socket, JSON.stringify({ method: "Page.domContentEventFired", params: { timestamp: 3 } }));
      } else if (message.method === "Page.reload") {
        requested.reloads += 1;
        sendText(socket, JSON.stringify({ id: message.id, result: {} }));
        sendText(socket, JSON.stringify({ method: "Page.domContentEventFired", params: { timestamp: 2 } }));
      } else if (message.method === "Input.insertText") {
        requested.insertedText = String(message.params?.text || "");
        fs.writeFileSync(`${readyPath}.inserted-text`, requested.insertedText);
        sendText(socket, JSON.stringify({ id: message.id, result: {} }));
      } else {
        sendText(socket, JSON.stringify({ id: message.id, result: {} }));
      }
    }
  });
});

server.listen(0, "127.0.0.1", () => {
  fs.writeFileSync(readyPath, JSON.stringify({ port: server.address().port }));
});
NODE

node "$tmp_dir/fake-cdp.mjs" "$tmp_dir/fake-cdp-ready.json" &
cdp_pid="$!"
for _ in {1..100}; do
  [[ -s "$tmp_dir/fake-cdp-ready.json" ]] && break
  sleep 0.02
done
[[ -s "$tmp_dir/fake-cdp-ready.json" ]]
cdp_port="$(node -e 'console.log(JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).port)' "$tmp_dir/fake-cdp-ready.json")"

control_socket="$tmp_dir/control.sock"
control_config="$(node -e 'console.log(JSON.stringify({
  schema: "elastos.browser.selkies-control.config/v1",
  control_socket_path: process.argv[1],
  replace_existing_socket: true,
  selkies_ws_url: `ws://127.0.0.1:${process.argv[2]}/signaling`,
  browser_control: {
    kind: "cdp_http",
    endpoint: `http://127.0.0.1:${process.argv[3]}`,
    timeout_ms: 2000
  },
  ice_servers: [
    { urls: ["stun:stun.example.invalid:3478"] }
  ],
  connect_timeout_ms: 2000,
  signal_timeout_ms: 2000
}))' "$control_socket" "$selkies_port" "$cdp_port")"
ELASTOS_BROWSER_SELKIES_CONTROL_CONFIG="$control_config" \
  scripts/browser-selkies-control-service.mjs >"$tmp_dir/control.log" 2>&1 &
control_pid="$!"
for _ in {1..100}; do
  [[ -S "$control_socket" ]] && break
  sleep 0.02
done
[[ -S "$control_socket" ]]

initial_status="$tmp_dir/initial-status.json"
curl --silent --show-error --fail \
  --unix-socket "$control_socket" \
  "http://browser-engine/status" >"$initial_status"
node -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.schema !== "elastos.browser.selkies-control.status/v1") throw new Error("wrong control status schema");
if (response.active_pages !== 0 || response.single_session !== true || response.direct_network !== false) throw new Error("wrong initial control status");
' "$initial_status"

failed_open_response="$tmp_dir/failed-open-response.json"
failed_status="$(curl --silent --show-error \
  --output "$failed_open_response" \
  --write-out "%{http_code}" \
  --unix-socket "$control_socket" \
  --header "content-type: application/json" \
  --data @- \
  http://browser-engine/pages <<'JSON'
{
  "schema": "elastos.browser.hosted-product.open/v1",
  "launch_request": {
    "schema": "elastos.browser.engine.launch-request/v1",
    "adapter": "hosted-product",
    "engine": "selkies_gstreamer",
    "stream_id": "smoke-stream-failed",
    "url": "https://example.com/",
    "display_mode": "webrtc_remote_display",
    "network_mode": "runtime_net_only",
    "direct_network": false,
    "wallet": {
      "accounts": [
        {
          "account_id": "wallet:eip155:20:0x1111111111111111111111111111111111111111",
          "chain_namespace": "eip155:20",
          "address": "0x1111111111111111111111111111111111111111",
          "label": "ESC Smoke"
        }
      ],
      "default_chain_namespace": "eip155:20"
    }
  }
}
JSON
)"
if [[ "$failed_status" != "500" ]]; then
  cat "$failed_open_response" >&2 || true
  echo "expected intentionally failed hosted Selkies open to return 500, got $failed_status" >&2
  exit 1
fi
for _ in {1..150}; do
  [[ -s "$tmp_dir/fake-selkies-failed-close.txt" ]] && break
  sleep 0.02
done
if [[ ! -s "$tmp_dir/fake-selkies-failed-close.txt" ]]; then
  echo "Selkies control bridge did not close the WebSocket after failed open" >&2
  cat "$failed_open_response" >&2 || true
  cat "$tmp_dir/control.log" >&2 || true
  exit 1
fi
after_failed_status="$tmp_dir/after-failed-status.json"
curl --silent --show-error --fail \
  --unix-socket "$control_socket" \
  "http://browser-engine/status" >"$after_failed_status"
node -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.active_pages !== 0) throw new Error("failed open leaked an active Selkies page");
' "$after_failed_status"

open_response="$tmp_dir/open-response.json"
if ! curl --silent --show-error --fail-with-body \
  --unix-socket "$control_socket" \
  --header "content-type: application/json" \
  --data @- \
  http://browser-engine/pages >"$open_response" <<'JSON'
{
  "schema": "elastos.browser.hosted-product.open/v1",
  "launch_request": {
    "schema": "elastos.browser.engine.launch-request/v1",
    "adapter": "hosted-product",
    "engine": "selkies_gstreamer",
    "stream_id": "smoke-stream",
    "url": "https://example.com/",
    "display_mode": "webrtc_remote_display",
    "network_mode": "runtime_net_only",
    "direct_network": false,
    "viewport": {
      "width": 1280,
      "height": 720
    },
    "wallet": {
      "accounts": [
        {
          "account_id": "wallet:eip155:20:0x1111111111111111111111111111111111111111",
          "chain_namespace": "eip155:20",
          "address": "0x1111111111111111111111111111111111111111",
          "label": "ESC Smoke"
        },
        {
          "account_id": "wallet:eip155:8453:0x2222222222222222222222222222222222222222",
          "chain_namespace": "eip155:8453",
          "address": "0x2222222222222222222222222222222222222222",
          "label": "Base Smoke"
        }
      ],
      "default_chain_namespace": "eip155:20"
    }
  }
}
JSON
then
  cat "$open_response" >&2 || true
  cat "$tmp_dir/control.log" >&2 || true
  exit 1
fi

node -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
const crypto = require("crypto");
if (response.schema !== "elastos.browser.engine.supervisor-result/v1") throw new Error("wrong supervisor schema");
if (!/^page:selkies-[0-9a-f]{16}$/.test(response.page_id)) throw new Error("page id must be an opaque Selkies session id");
const deterministic = `page:selkies-${crypto.createHash("sha256").update("https://example.com/\0smoke-stream").digest("hex").slice(0, 16)}`;
if (response.page_id === deterministic) throw new Error("page id must be launch-unique, not URL/stream deterministic");
if (response.actual_url !== "https://example.com/") throw new Error("CDP navigation did not set actual_url");
if (response.title !== "Example Domain") throw new Error("CDP navigation did not set title");
if (response.wallet_bridge?.mode !== "runtime_mediated_eip1193") throw new Error("missing Runtime-mediated wallet bridge receipt");
if (response.wallet_bridge?.accounts !== 2) throw new Error("wallet bridge did not keep fixture accounts");
if (response.wallet_bridge?.default_chain_namespace !== "eip155:20") throw new Error("wallet bridge lost the default chain namespace");
const display = response.display_session;
if (display.backend_class !== "product_compositor") throw new Error("not a product compositor");
if (display.display_backend !== "selkies_gstreamer_webrtc") throw new Error("wrong display backend");
if (display.offerer !== "engine") throw new Error("Selkies display must be engine-offer");
if (display.input !== "datachannel" || display.input_protocol !== "selkies_v1") throw new Error("Selkies display must declare datachannel selkies_v1 input");
if (display.width !== 1920 || display.height !== 1080) throw new Error("Selkies display must expose the fixed stream/input coordinate space");
if (response.view?.schema !== "elastos.browser.view/v1" || response.view.width !== 1280 || response.view.height !== 720) throw new Error("Selkies result must expose a matching Browser view");
if (display.initial_offer?.schema !== "elastos.browser.webrtc-offer/v1") throw new Error("missing initial offer");
if (!display.initial_offer.sdp.includes("m=video") || !display.initial_offer.sdp.includes("m=audio")) throw new Error("initial offer must include audio and video");
if (display.audio !== true || display.video !== true || display.direct_network !== false) throw new Error("wrong media/network flags");
if (display.ice_servers?.[0]?.urls?.[0] !== "stun:stun.example.invalid:3478") throw new Error("ICE servers were not propagated to display session");
' "$open_response"
node -e '
const viewport = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (viewport.width !== 1280 || viewport.height !== 720) throw new Error("initial CDP viewport did not match launch viewport");
if (viewport.deviceScaleFactor !== 1.5) throw new Error("initial CDP viewport did not apply normal-browser scale factor");
' "$tmp_dir/fake-cdp-ready.json.viewport"

page_id="$(node -e 'const r=JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")); console.log(encodeURIComponent(r.page_id));' "$open_response")"

active_status="$tmp_dir/active-status.json"
curl --silent --show-error --fail \
  --unix-socket "$control_socket" \
  "http://browser-engine/status" >"$active_status"
node -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.active_pages !== 1) throw new Error("control status did not report active page");
if (!Array.isArray(response.page_ids) || response.page_ids.length !== 1) throw new Error("control status did not report active page id");
' "$active_status"

old_page_id="$page_id"
replacement_response="$tmp_dir/replacement-response.json"
if ! curl --silent --show-error --fail-with-body \
  --unix-socket "$control_socket" \
  --header "content-type: application/json" \
  --data @- \
  http://browser-engine/pages >"$replacement_response" <<'JSON'
{
  "schema": "elastos.browser.hosted-product.open/v1",
  "launch_request": {
    "schema": "elastos.browser.engine.launch-request/v1",
    "adapter": "hosted-product",
    "engine": "selkies_gstreamer",
    "stream_id": "smoke-stream-second",
    "url": "https://example.com/",
    "display_mode": "webrtc_remote_display",
    "network_mode": "runtime_net_only",
    "direct_network": false,
    "viewport": {
      "width": 1280,
      "height": 720
    }
  }
}
JSON
then
  cat "$replacement_response" >&2 || true
  cat "$tmp_dir/control.log" >&2 || true
  exit 1
fi
node -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
const oldPage = decodeURIComponent(process.argv[2]);
if (response.schema !== "elastos.browser.engine.supervisor-result/v1") throw new Error("wrong replacement supervisor schema");
if (!/^page:selkies-[0-9a-f]{16}$/.test(response.page_id)) throw new Error("replacement page id must be opaque");
if (response.page_id === oldPage) throw new Error("replacement open must allocate a fresh page id");
if (response.direct_network !== false || response.display_session?.network_mode !== "runtime_net_only") throw new Error("replacement lost Runtime network boundary");
' "$replacement_response" "$old_page_id"
stale_status="$(curl --silent --show-error \
  --output "$tmp_dir/stale-page-status.json" \
  --write-out "%{http_code}" \
  --unix-socket "$control_socket" \
  "http://browser-engine/pages/$old_page_id/status")"
if [[ "$stale_status" != "404" ]]; then
  cat "$tmp_dir/stale-page-status.json" >&2 || true
  echo "expected superseded Selkies page to be closed, got $stale_status" >&2
  exit 1
fi
page_id="$(node -e 'const r=JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")); console.log(encodeURIComponent(r.page_id));' "$replacement_response")"

answer_response="$tmp_dir/answer-response.json"
curl --silent --show-error --fail \
  --unix-socket "$control_socket" \
  --header "content-type: application/json" \
  --data @- \
  "http://browser-engine/pages/$page_id/webrtc" >"$answer_response" <<'JSON'
{
  "signal": {
    "schema": "elastos.browser.webrtc-answer/v1",
    "type": "answer",
    "sdp": "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=ElastOS Browser Answer\r\nt=0 0\r\n"
  }
}
JSON
node -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.schema !== "elastos.browser.webrtc-signal-ack/v1") throw new Error("wrong answer ack schema");
if (response.type !== "answer" || response.accepted !== true) throw new Error("answer was not accepted");
if (!Array.isArray(response.candidates)) throw new Error("answer ack must carry queued candidates");
' "$answer_response"
for _ in {1..100}; do
  [[ -s "$tmp_dir/fake-selkies-pong.txt" ]] && break
  sleep 0.02
done
if [[ ! -s "$tmp_dir/fake-selkies-pong.txt" ]]; then
  echo "Selkies control client did not answer WebSocket ping with a masked pong" >&2
  cat "$tmp_dir/control.log" >&2 || true
  exit 1
fi

candidate_response="$tmp_dir/candidate-response.json"
curl --silent --show-error --fail \
  --unix-socket "$control_socket" \
  --header "content-type: application/json" \
  --data @- \
  "http://browser-engine/pages/$page_id/webrtc" >"$candidate_response" <<'JSON'
{
  "signal": {
    "schema": "elastos.browser.webrtc-candidate/v1",
    "type": "candidate",
    "candidate": {
      "candidate": "candidate:browser-smoke 1 udp 1 127.0.0.1 9 typ host",
      "sdpMid": "0",
      "sdpMLineIndex": 0
    }
  }
}
JSON
node -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.schema !== "elastos.browser.webrtc-signal-ack/v1") throw new Error("wrong candidate ack schema");
if (response.type !== "candidate" || response.accepted !== true) throw new Error("candidate was not accepted");
' "$candidate_response"

status_response="$tmp_dir/status-response.json"
curl --silent --show-error --fail \
  --unix-socket "$control_socket" \
  "http://browser-engine/pages/$page_id/status" >"$status_response"
node -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.schema !== "elastos.browser.page-status/v1") throw new Error("wrong status schema");
if (response.backend_class !== "product_compositor" || response.audio !== true || response.video !== true) throw new Error("wrong page status");
if (response.input_protocol !== "selkies_v1") throw new Error("status lost Selkies input protocol");
' "$status_response"

resize_response="$tmp_dir/resize-response.json"
curl --silent --show-error --fail \
  --unix-socket "$control_socket" \
  --header "content-type: application/json" \
  --data @- \
  "http://browser-engine/pages/$page_id/input" >"$resize_response" <<'JSON'
{
  "event": {
    "type": "resize",
    "viewport": {
      "width": 1000,
      "height": 700
    }
  }
}
JSON
node -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.schema !== "elastos.browser.input-result/v1") throw new Error("wrong resize result schema");
if (response.accepted !== true || response.direct_network !== false) throw new Error("resize was not accepted fail-closed through Runtime route");
if (response.width !== 1000 || response.height !== 700) throw new Error("resize must return the requested Browser viewport");
const viewport = JSON.parse(require("fs").readFileSync(process.argv[2], "utf8"));
if (viewport.width !== 1000 || viewport.height !== 700) throw new Error("resize must update the CDP viewport");
' "$resize_response" "$tmp_dir/fake-cdp-ready.json.viewport"

navigate_response="$tmp_dir/navigate-response.json"
curl --silent --show-error --fail \
  --unix-socket "$control_socket" \
  --header "content-type: application/json" \
  --data @- \
  "http://browser-engine/pages/$page_id/input" >"$navigate_response" <<'JSON'
{
  "event": {
    "type": "browser_command",
    "command": "navigate",
    "url": "https://example.com/?elastos-browser-nav-smoke=1"
  }
}
JSON
node -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.schema !== "elastos.browser.input-result/v1") throw new Error("wrong navigate result schema");
if (response.accepted !== true || response.direct_network !== false) throw new Error("navigate was not accepted fail-closed through Runtime route");
if (response.actual_url !== "https://example.com/?elastos-browser-nav-smoke=1" || response.can_go_back !== true) throw new Error("navigate did not update URL/history state");
' "$navigate_response"

back_response="$tmp_dir/back-response.json"
curl --silent --show-error --fail \
  --unix-socket "$control_socket" \
  --header "content-type: application/json" \
  --data @- \
  "http://browser-engine/pages/$page_id/input" >"$back_response" <<'JSON'
{
  "event": {
    "type": "browser_command",
    "command": "back"
  }
}
JSON
node -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.schema !== "elastos.browser.input-result/v1") throw new Error("wrong back result schema");
if (response.actual_url !== "https://example.com/" || response.can_go_forward !== true) throw new Error("back did not return to the first page");
' "$back_response"

forward_response="$tmp_dir/forward-response.json"
curl --silent --show-error --fail \
  --unix-socket "$control_socket" \
  --header "content-type: application/json" \
  --data @- \
  "http://browser-engine/pages/$page_id/input" >"$forward_response" <<'JSON'
{
  "event": {
    "type": "browser_command",
    "command": "forward"
  }
}
JSON
node -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.schema !== "elastos.browser.input-result/v1") throw new Error("wrong forward result schema");
if (response.actual_url !== "https://example.com/?elastos-browser-nav-smoke=1" || response.can_go_back !== true) throw new Error("forward did not return to the second page");
' "$forward_response"

input_response="$tmp_dir/input-response.json"
curl --silent --show-error --fail \
  --unix-socket "$control_socket" \
  --header "content-type: application/json" \
  --data @- \
  "http://browser-engine/pages/$page_id/input" >"$input_response" <<'JSON'
{
  "event": {
    "type": "browser_command",
    "command": "reload"
  }
}
JSON
node -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.schema !== "elastos.browser.input-result/v1") throw new Error("wrong input result schema");
if (response.accepted !== true || response.direct_network !== false) throw new Error("browser command was not accepted fail-closed through Runtime route");
if (response.actual_url !== "https://example.com/?elastos-browser-nav-smoke=1" || response.title !== "Example Domain") throw new Error("browser command did not return current page state");
' "$input_response"

paste_response="$tmp_dir/paste-response.json"
curl --silent --show-error --fail \
  --unix-socket "$control_socket" \
  --header "content-type: application/json" \
  --data @- \
  "http://browser-engine/pages/$page_id/input" >"$paste_response" <<'JSON'
{
  "event": {
    "type": "paste_text",
    "text": "Paste Text 123"
  }
}
JSON
node -e '
const fs = require("fs");
const response = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
const inserted = fs.readFileSync(process.argv[2], "utf8");
if (response.schema !== "elastos.browser.input-result/v1") throw new Error("wrong paste result schema");
if (response.accepted !== true || response.direct_network !== false) throw new Error("paste_text was not accepted through Runtime route");
if (inserted !== "Paste Text 123") throw new Error(`paste_text did not use CDP Input.insertText: ${inserted}`);
' "$paste_response" "$tmp_dir/fake-cdp-ready.json.inserted-text"

scripts/browser-selkies-target-preflight.sh \
  --out-dir "$tmp_dir/target-preflight" \
  --control-socket "$tmp_dir/target-preflight.sock" \
  --selkies-ws-url "ws://127.0.0.1:$selkies_port/signaling" \
  --browser-cdp-endpoint "http://127.0.0.1:$cdp_port" \
  --ice-server "stun:stun.example.invalid:3478" >/dev/null
