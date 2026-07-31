#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"
selkies_pid=""
cdp_pid=""
proxy_pid=""
control_pid=""

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
  if [[ -n "$control_pid" ]]; then
    kill "$control_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "$cdp_pid" ]]; then
    kill "$cdp_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "$proxy_pid" ]]; then
    kill "$proxy_pid" >/dev/null 2>&1 || true
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
import path from "node:path";

const readyPath = process.argv[2];
const pongPath = process.argv[3];
const failedClosePath = process.argv[4];
const legacyTriggerPath = process.argv[5];
const legacyHelloPath = process.argv[6];
const legacyAnswerPath = process.argv[7];
const audioUnavailablePath = process.argv[8];
let failNextHello = true;

const videoOfferSdp = [
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
  "",
].join("\r\n");

const audioOfferSdp = [
  "v=0",
  "o=- 0 0 IN IP4 127.0.0.1",
  "s=ElastOS Browser Audio",
  "t=0 0",
  "m=audio 9 UDP/TLS/RTP/SAVPF 111",
  "c=IN IP4 0.0.0.0",
  "a=mid:0",
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

const server = http.createServer((req, res) => {
  if (req.method === "GET" && req.url === "/health") {
    res.writeHead(200, { "content-type": "text/plain" });
    res.end("OK\n");
    return;
  }
  res.writeHead(404, { "content-type": "text/plain" });
  res.end("not found\n");
});
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
  let closedForFailedHello = false;
  let legacyRawJson = false;
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
      if (closedForFailedHello) {
        return;
      }
      if (text.startsWith("HELLO client ")) {
        const meta = JSON.parse(text.slice("HELLO client ".length));
        if (meta.client_type !== "controller" || meta.client_slot !== 1) {
          socket.destroy();
          return;
        }
        if (fs.existsSync(legacyTriggerPath)) {
          fs.unlinkSync(legacyTriggerPath);
          closedForFailedHello = true;
          socket.end();
          return;
        }
        sendText(socket, "HELLO");
        if (failNextHello) {
          failNextHello = false;
          recordFailedClose = true;
          closedForFailedHello = true;
          socket.end();
          return;
        }
      } else if (text.startsWith("HELLO 1 ") || text.startsWith("HELLO 3 ")) {
        const prefix = text.startsWith("HELLO 1 ") ? "HELLO 1 " : "HELLO 3 ";
        const meta = JSON.parse(Buffer.from(text.slice(prefix.length), "base64").toString("utf8"));
        if (meta.res !== "1920x1080" || meta.scale !== 1) {
          socket.destroy();
          return;
        }
        if (prefix === "HELLO 3 " && fs.existsSync(audioUnavailablePath)) {
          fs.unlinkSync(audioUnavailablePath);
          socket.end();
          return;
        }
        legacyRawJson = true;
        fs.writeFileSync(legacyHelloPath, JSON.stringify(meta));
        sendText(socket, "HELLO");
        sendText(socket, JSON.stringify({ sdp: { type: "offer", sdp: prefix === "HELLO 3 " ? audioOfferSdp : videoOfferSdp } }));
      } else if (text === "SESSION server") {
        sendText(socket, "SESSION_OK server");
        sendText(socket, `server ${JSON.stringify({ sdp: { type: "offer", sdp: videoOfferSdp } })}`);
      } else if (text.startsWith("{") || text.startsWith("server ") || text.startsWith("1 ")) {
        const peer = text.startsWith("server ") ? "server" : text.startsWith("1 ") ? "1" : null;
        if (legacyRawJson && peer) {
          socket.destroy();
          return;
        }
        const body = JSON.parse(peer ? text.slice(peer.length + 1) : text);
        if (body.sdp?.type === "answer") {
          if (legacyRawJson && !peer) {
            fs.writeFileSync(legacyAnswerPath, "ok");
          }
          sendControl(socket, 0x9, "keepalive");
          const candidate = JSON.stringify({ ice: { candidate: "candidate:smoke 1 udp 1 127.0.0.1 9 typ host", sdpMid: "0", sdpMLineIndex: 0 } });
          sendText(socket, legacyRawJson ? candidate : `${peer || "server"} ${candidate}`);
        }
      }
    }
  });
});

server.listen(0, "127.0.0.1", () => {
  fs.writeFileSync(readyPath, JSON.stringify({ port: server.address().port }));
});
NODE

"$node_bin" "$tmp_dir/fake-selkies.mjs" \
  "$tmp_dir/fake-selkies-ready.json" \
  "$tmp_dir/fake-selkies-pong.txt" \
  "$tmp_dir/fake-selkies-failed-close.txt" \
  "$tmp_dir/fake-selkies-force-legacy.txt" \
  "$tmp_dir/fake-selkies-legacy-hello.json" \
  "$tmp_dir/fake-selkies-legacy-answer.txt" \
  "$tmp_dir/fake-selkies-audio-unavailable.txt" &
selkies_pid="$!"
for _ in {1..100}; do
  [[ -s "$tmp_dir/fake-selkies-ready.json" ]] && break
  sleep 0.02
done
[[ -s "$tmp_dir/fake-selkies-ready.json" ]]
selkies_port="$("$node_bin" -e 'console.log(JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).port)' "$tmp_dir/fake-selkies-ready.json")"

cat >"$tmp_dir/fake-cdp.mjs" <<'NODE'
import crypto from "node:crypto";
import http from "node:http";
import fs from "node:fs";
import path from "node:path";

const readyPath = process.argv[2];
const firstUrl = "https://example.com/";
const secondUrl = "https://example.com/?elastos-browser-nav-smoke=1";
const networkChangedUrl = "https://example.com/?network-changed=1";
const closedOnceUrl = "https://example.com/?closed-once=1";
const timeoutOnceUrl = "https://example.com/?timeout-once=1";
const netTimeoutOnceUrl = "https://example.com/?net-timeout-once=1";
const slowDomUrl = "https://example.com/?slow-dom=1";
const closedConnectionUrl = "https://docs.ela.city/";
const lateChromeErrorUrl = "https://docs-late.ela.city/";
const requested = {
  newTarget: "",
  navigated: "",
  initScript: "",
  insertedText: "",
  fileChooserIntercepted: false,
  uploadedFile: null,
  newTargetCount: 0,
  reloads: 0,
  networkChangedNavigations: 0,
  closedOnceNavigations: 0,
  timeoutOnceNavigations: 0,
  netTimeoutOnceNavigations: 0,
  lateChromeErrorNavigations: 0,
};
let historyEntries = [];
let currentIndex = -1;
let walletBindingInstalled = false;
let walletBridgeEventSent = false;

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
  if (req.method === "GET" && url.pathname === "/json/version") {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({
      Browser: "ElastOS Fake Chromium",
      webSocketDebuggerUrl: `ws://127.0.0.1:${server.address().port}/devtools/browser/smoke`,
    }));
    return;
  }
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
  if (req.method === "POST" && url.pathname === "/__fake/navigate") {
    const nextUrl = url.searchParams.get("url") || "";
      if (nextUrl !== firstUrl && nextUrl !== secondUrl && nextUrl !== networkChangedUrl && nextUrl !== slowDomUrl) {
        res.writeHead(400, { "content-type": "application/json" });
        res.end(JSON.stringify({ error: `wrong fake navigate URL: ${nextUrl}` }));
        return;
      }
      if (historyEntries.length === 0 || currentIndex < 0) {
        historyEntries = [{ id: 1, url: firstUrl, title: "Example Domain" }];
        currentIndex = 0;
      }
      historyEntries = historyEntries.slice(0, currentIndex + 1);
      historyEntries.push({ id: historyEntries.length + 1, url: nextUrl, title: "Example Domain" });
    currentIndex = historyEntries.length - 1;
    requested.navigated = nextUrl;
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({ ok: true, url: nextUrl }));
    return;
  }
  if (req.method !== "PUT" || url.pathname !== "/json/new") {
    res.writeHead(404, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: "not found" }));
    return;
  }
  const requestedUrl = url.search.slice(1);
  requested.newTarget = requestedUrl;
  requested.newTargetCount += 1;
  fs.writeFileSync(`${readyPath}.new-target-count`, String(requested.newTargetCount));
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
  const emitWalletBinding = (payload) => {
    sendText(socket, JSON.stringify({
      method: "Runtime.bindingCalled",
      params: {
        name: "__elastosBrowserWalletRuntime",
        payload: JSON.stringify(payload),
      },
    }));
  };
  socket.on("error", () => {});
  socket.on("data", (chunk) => {
    buffer = Buffer.concat([buffer, chunk]);
    for (;;) {
      const frame = readFrame(buffer);
      if (!frame) return;
      buffer = buffer.subarray(frame.consumed);
      if (frame.opcode !== 0x1) continue;
      const message = JSON.parse(frame.payload.toString("utf8"));
      if (message.method === "Page.enable") {
        sendText(socket, JSON.stringify({ id: message.id, result: {} }));
        sendText(socket, JSON.stringify({ method: "Page.domContentEventFired", params: { timestamp: 0 } }));
        sendText(socket, JSON.stringify({ method: "Page.loadEventFired", params: { timestamp: 0 } }));
      } else if (message.method === "Runtime.enable") {
        sendText(socket, JSON.stringify({ id: message.id, result: {} }));
      } else if (message.method === "DOM.enable") {
        sendText(socket, JSON.stringify({ id: message.id, result: {} }));
      } else if (message.method === "Runtime.addBinding") {
        if (message.params?.name === "__elastosBrowserWalletRuntime") {
          walletBindingInstalled = true;
          fs.writeFileSync(`${readyPath}.wallet-binding`, message.params.name);
        }
        sendText(socket, JSON.stringify({ id: message.id, result: {} }));
      } else if (message.method === "Emulation.setDeviceMetricsOverride") {
        requested.viewport = message.params || {};
        fs.writeFileSync(`${readyPath}.viewport`, JSON.stringify(requested.viewport));
        sendText(socket, JSON.stringify({ id: message.id, result: {} }));
      } else if (message.method === "Page.addScriptToEvaluateOnNewDocument") {
        requested.initScript = String(message.params?.source || "");
        fs.writeFileSync(`${readyPath}.init-script`, requested.initScript);
        walletBridgeEventSent = false;
        sendText(socket, JSON.stringify({ id: message.id, result: { identifier: "wallet-bridge-smoke" } }));
      } else if (message.method === "Runtime.evaluate") {
        const expression = String(message.params?.expression || "");
        if (expression.includes("globalThis[\"__elastosBrowserWalletRuntimeResult\"]")) {
          fs.appendFileSync(`${readyPath}.wallet-results`, `${expression}\n`);
          if (expression.includes("wallet:smoke-bridge")) {
            setTimeout(() => emitWalletBinding({
              id: "wallet:smoke-read",
              action: "post",
              operation: "read",
              body: {
                method: "eth_chainId",
                params: [],
              },
            }), 10);
          } else if (expression.includes("wallet:smoke-read")) {
            setTimeout(() => emitWalletBinding({
              id: "wallet:smoke-signature",
              action: "post",
              operation: "approval",
              body: {
                method: "personal_sign",
                params: [
                  "0x68656c6c6f",
                  "0x1111111111111111111111111111111111111111",
                ],
              },
            }), 10);
          } else if (expression.includes("wallet:smoke-signature")) {
            setTimeout(() => emitWalletBinding({
              id: "wallet:smoke-eth-sign",
              action: "post",
              operation: "approval",
              body: {
                method: "personal_sign",
                params: [
                  "0x6574682d7369676e",
                  "0x1111111111111111111111111111111111111111",
                ],
              },
            }), 10);
          } else if (expression.includes("wallet:smoke-eth-sign")) {
            setTimeout(() => emitWalletBinding({
              id: "wallet:smoke-typed-data",
              action: "post",
              operation: "approval",
              body: {
                method: "eth_signTypedData_v4",
                params: [
                  "0x1111111111111111111111111111111111111111",
                  {
                    domain: {
                      name: "ElastOS Browser Smoke",
                      version: "1",
                      chainId: 20,
                    },
                    primaryType: "Login",
                    types: {
                      EIP712Domain: [
                        { name: "name", type: "string" },
                        { name: "version", type: "string" },
                        { name: "chainId", type: "uint256" },
                      ],
                      Login: [{ name: "message", type: "string" }],
                    },
                    message: {
                      message: "typed-data-smoke",
                    },
                  },
                ],
              },
            }), 10);
          } else if (expression.includes("wallet:smoke-typed-data")) {
            setTimeout(() => emitWalletBinding({
              id: "wallet:smoke-tx-request",
              action: "post",
              operation: "transaction",
              body: {
                method: "eth_sendTransaction",
                params: [
                  {
                    from: "0x1111111111111111111111111111111111111111",
                    to: "0x2222222222222222222222222222222222222222",
                    value: "0x1",
                    data: "0x",
                  },
                ],
              },
            }), 10);
          } else if (expression.includes("wallet:smoke-tx-request")) {
            setTimeout(() => emitWalletBinding({
              id: "wallet:smoke-tx-status",
              action: "approvalStatus",
              request_id: "wallet-approval:tx-smoke",
            }), 10);
          } else if (expression.includes("wallet:smoke-tx-status")) {
            setTimeout(() => emitWalletBinding({
              id: "wallet:smoke-tx-broadcast",
              action: "post",
              operation: "transactionBroadcast",
              body: {
                request_id: "wallet-approval:tx-smoke",
              },
            }), 10);
          }
          sendText(socket, JSON.stringify({ id: message.id, result: { result: { type: "undefined" } } }));
      } else if (typeof message.params?.expression === "string" && message.params.expression.includes("JSON.stringify({ url: window.location.href")) {
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
      } else if (message.method === "Page.setInterceptFileChooserDialog") {
        requested.fileChooserIntercepted = message.params?.enabled === true;
        fs.writeFileSync(`${readyPath}.file-chooser-intercepted`, requested.fileChooserIntercepted ? "true" : "false");
        sendText(socket, JSON.stringify({ id: message.id, result: {} }));
      } else if (message.method === "Page.navigate") {
        requested.navigated = String(message.params?.url || "");
        if (requested.navigated !== firstUrl && requested.navigated !== secondUrl && requested.navigated !== networkChangedUrl && requested.navigated !== closedOnceUrl && requested.navigated !== timeoutOnceUrl && requested.navigated !== netTimeoutOnceUrl && requested.navigated !== slowDomUrl && requested.navigated !== closedConnectionUrl && requested.navigated !== lateChromeErrorUrl) {
          sendText(socket, JSON.stringify({ id: message.id, error: { message: `wrong navigate URL: ${requested.navigated}` } }));
          continue;
        }
          if (!requested.initScript.includes('"ethereum"') || !requested.initScript.includes("wallet_switchEthereumChain")) {
            sendText(socket, JSON.stringify({ id: message.id, error: { message: "wallet bridge init script was not installed before navigation" } }));
            continue;
          }
          if (requested.navigated === timeoutOnceUrl) {
            requested.timeoutOnceNavigations += 1;
            fs.writeFileSync(`${readyPath}.timeout-once-navigations`, String(requested.timeoutOnceNavigations));
            if (requested.timeoutOnceNavigations === 1) {
              continue;
            }
          }
          if (requested.navigated !== firstUrl && (historyEntries.length === 0 || currentIndex < 0)) {
            historyEntries = [{ id: 1, url: firstUrl, title: "Example Domain" }];
            currentIndex = 0;
          }
          historyEntries = historyEntries.slice(0, currentIndex + 1);
          historyEntries.push({ id: historyEntries.length + 1, url: requested.navigated, title: "Example Domain" });
        currentIndex = historyEntries.length - 1;
        if (requested.navigated === lateChromeErrorUrl) {
          requested.lateChromeErrorNavigations += 1;
          fs.writeFileSync(`${readyPath}.late-chrome-error-navigations`, String(requested.lateChromeErrorNavigations));
          if (requested.lateChromeErrorNavigations === 1) {
            historyEntries[currentIndex].url = "chrome-error://chromewebdata/";
            historyEntries[currentIndex].title = "This site can't be reached";
          }
        }
        if (requested.navigated === networkChangedUrl) {
          requested.networkChangedNavigations += 1;
          fs.writeFileSync(`${readyPath}.network-changed-navigations`, String(requested.networkChangedNavigations));
        }
        if (requested.navigated === closedOnceUrl) {
          requested.closedOnceNavigations += 1;
          fs.writeFileSync(`${readyPath}.closed-once-navigations`, String(requested.closedOnceNavigations));
          if (requested.closedOnceNavigations === 1) {
            sendText(socket, JSON.stringify({ id: message.id, result: { frameId: "frame-smoke", errorText: "net::ERR_CONNECTION_CLOSED" } }));
            continue;
          }
        }
        if (requested.navigated === netTimeoutOnceUrl) {
          requested.netTimeoutOnceNavigations += 1;
          fs.writeFileSync(`${readyPath}.net-timeout-once-navigations`, String(requested.netTimeoutOnceNavigations));
          if (requested.netTimeoutOnceNavigations === 1) {
            sendText(socket, JSON.stringify({ id: message.id, result: { frameId: "frame-smoke", errorText: "net::ERR_TIMED_OUT" } }));
            continue;
          }
        }
        if (requested.navigated === closedConnectionUrl) {
          sendText(socket, JSON.stringify({ id: message.id, result: { frameId: "frame-smoke", errorText: "net::ERR_CONNECTION_CLOSED" } }));
          continue;
        }
        sendText(socket, JSON.stringify({ id: message.id, result: { frameId: "frame-smoke" } }));
        if (requested.navigated === slowDomUrl) {
          setTimeout(() => {
            fs.writeFileSync(`${readyPath}.slow-dom-fired`, "ok");
            sendText(socket, JSON.stringify({ method: "Page.domContentEventFired", params: { timestamp: 5 } }));
            sendText(socket, JSON.stringify({ method: "Page.loadEventFired", params: { timestamp: 5 } }));
          }, 300);
          continue;
        }
        if (requested.navigated === networkChangedUrl && requested.networkChangedNavigations === 1) {
          sendText(socket, JSON.stringify({
            method: "Network.loadingFailed",
            params: {
              requestId: "network-changed-smoke",
              type: "Script",
              errorText: "net::ERR_NETWORK_CHANGED",
              canceled: false
            }
          }));
        }
        sendText(socket, JSON.stringify({ method: "Page.domContentEventFired", params: { timestamp: 1 } }));
        sendText(socket, JSON.stringify({ method: "Page.loadEventFired", params: { timestamp: 1 } }));
        if (
          !walletBridgeEventSent &&
          walletBindingInstalled &&
          requested.initScript.includes("wallet:eip155:20:0x1111111111111111111111111111111111111111")
        ) {
          walletBridgeEventSent = true;
          setTimeout(() => emitWalletBinding({
            id: "wallet:smoke-bridge",
            action: "bridge",
          }), 10);
        }
        } else if (message.method === "Page.getNavigationHistory") {
          if (requested.navigated === firstUrl) {
            historyEntries = [
              { id: 1, url: firstUrl, title: "Example Domain" },
              { id: 2, url: secondUrl, title: "Example Domain" },
            ];
            currentIndex = 0;
          } else if (requested.navigated === secondUrl) {
            historyEntries = [
              { id: 1, url: firstUrl, title: "Example Domain" },
              { id: 2, url: secondUrl, title: "Example Domain" },
            ];
            currentIndex = 1;
          }
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
        fs.writeFileSync(`${readyPath}.reloads`, String(requested.reloads));
        sendText(socket, JSON.stringify({ id: message.id, result: {} }));
        sendText(socket, JSON.stringify({ method: "Page.domContentEventFired", params: { timestamp: 2 } }));
        sendText(socket, JSON.stringify({ method: "Page.loadEventFired", params: { timestamp: 2 } }));
      } else if (message.method === "Input.insertText") {
        requested.insertedText = String(message.params?.text || "");
        fs.writeFileSync(`${readyPath}.inserted-text`, requested.insertedText);
        sendText(socket, JSON.stringify({ id: message.id, result: {} }));
      } else if (message.method === "DOM.resolveNode") {
        if (message.params?.backendNodeId !== 42) {
          sendText(socket, JSON.stringify({ id: message.id, error: { message: "wrong file chooser backend node" } }));
          continue;
        }
        sendText(socket, JSON.stringify({
          id: message.id,
          result: { object: { type: "object", objectId: "file-input-smoke" } }
        }));
      } else if (message.method === "DOM.setFileInputFiles") {
        if (message.params?.backendNodeId !== 42) {
          sendText(socket, JSON.stringify({ id: message.id, error: { message: "wrong file chooser backend node" } }));
          continue;
        }
        const files = Array.isArray(message.params?.files) ? message.params.files : [];
        const filePath = String(files[0] || "");
        if (!filePath || !fs.existsSync(filePath)) {
          sendText(socket, JSON.stringify({ id: message.id, error: { message: "uploaded file path does not exist" } }));
          continue;
        }
        requested.uploadedFile = {
          fileName: path.basename(filePath),
          mimeType: "image/png",
          size: fs.statSync(filePath).size,
          path: filePath,
        };
        fs.writeFileSync(`${readyPath}.uploaded-file`, JSON.stringify(requested.uploadedFile));
        sendText(socket, JSON.stringify({ id: message.id, result: {} }));
      } else if (message.method === "Runtime.callFunctionOn") {
        try {
          new Function(`return (${String(message.params?.functionDeclaration || "")});`);
        } catch (error) {
          sendText(socket, JSON.stringify({
            id: message.id,
            exceptionDetails: { text: error.message || "Runtime.callFunctionOn syntax error" }
          }));
          continue;
        }
        if (message.params?.objectId !== "file-input-smoke") {
          sendText(socket, JSON.stringify({ id: message.id, error: { message: "wrong file input object" } }));
          continue;
        }
        const uploadedFile = requested.uploadedFile || {
          fileName: "",
          mimeType: "",
          size: 0,
        };
        if (JSON.stringify(message.params?.arguments || []).includes("SGVsbG8gQnJvd3Nlcg")) {
          sendText(socket, JSON.stringify({ id: message.id, error: { message: "file bytes leaked into Runtime.callFunctionOn" } }));
          continue;
        }
        sendText(socket, JSON.stringify({
          id: message.id,
          result: {
            result: {
              type: "object",
              value: {
                ok: true,
                file_name: uploadedFile.fileName,
                type: uploadedFile.mimeType,
                size: uploadedFile.size
              }
            }
          }
        }));
      } else if (message.method === "Input.dispatchMouseEvent") {
        if (message.params?.type === "mouseReleased") {
          if (historyEntries.length === 0 || currentIndex < 0) {
            historyEntries = [{ id: 1, url: firstUrl, title: "Example Domain" }];
            currentIndex = 0;
          }
          historyEntries = historyEntries.slice(0, currentIndex + 1);
          historyEntries.push({
            id: historyEntries.length + 1,
            url: secondUrl,
            title: "Example Domain"
          });
          currentIndex = historyEntries.length - 1;
          requested.navigated = secondUrl;
          if (requested.fileChooserIntercepted) {
            sendText(socket, JSON.stringify({
              method: "Page.fileChooserOpened",
              params: {
                frameId: "frame-smoke",
                mode: "selectSingle",
                backendNodeId: 42
              }
            }));
          }
          sendText(socket, JSON.stringify({ id: message.id, result: {} }));
          sendText(socket, JSON.stringify({
            method: "Page.domContentEventFired",
            params: { timestamp: 4 }
          }));
        } else {
          sendText(socket, JSON.stringify({ id: message.id, result: {} }));
        }
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

"$node_bin" "$tmp_dir/fake-cdp.mjs" "$tmp_dir/fake-cdp-ready.json" &
cdp_pid="$!"
for _ in {1..100}; do
  [[ -s "$tmp_dir/fake-cdp-ready.json" ]] && break
  sleep 0.02
done
[[ -s "$tmp_dir/fake-cdp-ready.json" ]]
cdp_port="$("$node_bin" -e 'console.log(JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).port)' "$tmp_dir/fake-cdp-ready.json")"

read_new_target_count() {
  if [[ -s "$tmp_dir/fake-cdp-ready.json.new-target-count" ]]; then
    tr -d '\n' <"$tmp_dir/fake-cdp-ready.json.new-target-count"
  else
    printf '0'
  fi
}

cat >"$tmp_dir/fake-runtime-proxy.mjs" <<'NODE'
import http from "node:http";
import fs from "node:fs";

const readyPath = process.argv[2];
const requestsPath = process.argv[3];

function writeRequest(entry) {
  fs.appendFileSync(requestsPath, `${JSON.stringify(entry)}\n`);
}

const server = http.createServer((req, res) => {
  const chunks = [];
  req.on("data", (chunk) => chunks.push(Buffer.from(chunk)));
  req.on("end", () => {
    const bodyText = Buffer.concat(chunks).toString("utf8");
    const absoluteUrl = String(req.url || "");
    let target;
    try {
      target = new URL(absoluteUrl);
    } catch {
      res.writeHead(400, { "content-type": "application/json" });
      res.end(JSON.stringify({ error: `expected absolute proxy URL, got ${absoluteUrl}` }));
      return;
    }
    writeRequest({
      method: req.method,
      url: target.href,
      pathname: target.pathname,
      host: req.headers.host || "",
      origin: req.headers.origin || "",
      token: req.headers["x-elastos-home-token"] || "",
      body: bodyText ? JSON.parse(bodyText) : null,
    });
    if (req.headers.origin !== "null") {
      res.writeHead(400, { "content-type": "application/json" });
      res.end(JSON.stringify({ error: "Runtime wallet request requires exact Origin: null" }));
      return;
    }
    res.writeHead(200, { "content-type": "application/json" });
    if (req.method === "GET" && target.pathname === "/api/apps/browser/wallet/bridge") {
      res.end(JSON.stringify({
        schema: "elastos.browser.wallet-bridge/v1",
        accounts: [
          {
            account_id: "wallet:eip155:20:0x1111111111111111111111111111111111111111",
            chain_namespace: "eip155:20",
            address: "0x1111111111111111111111111111111111111111",
            label: "ESC Smoke",
          },
        ],
        default_chain_namespace: "eip155:20",
        default_account_id: "wallet:eip155:20:0x1111111111111111111111111111111111111111",
        bridge_url: "http://runtime.local/api/apps/browser/wallet/bridge",
        read_url: "http://runtime.local/api/apps/browser/wallet/read",
        approval_url: "http://runtime.local/api/apps/browser/wallet/request-signature",
        transaction_url: "http://runtime.local/api/apps/browser/wallet/request-transaction",
        transaction_broadcast_url: "http://runtime.local/api/apps/browser/wallet/broadcast-transaction",
        approval_status_url: "http://runtime.local/api/apps/browser/wallet/approvals",
        home_token: "wallet-token-smoke",
      }));
      return;
    }
    if (req.method === "POST" && target.pathname === "/api/apps/browser/wallet/read") {
      res.end(JSON.stringify({
        schema: "elastos.browser.wallet-read-result/v1",
        result: "0x14",
      }));
      return;
    }
    if (req.method === "POST" && target.pathname === "/api/apps/browser/wallet/request-signature") {
      const body = bodyText ? JSON.parse(bodyText) : {};
      const method = body.method === "eth_signTypedData_v4" ? "typed" : "personal";
      const message = Array.isArray(body.params) ? String(body.params[0] || "") : "";
      const requestId = method === "typed"
        ? "wallet-approval:typed-smoke"
        : message === "0x6574682d7369676e"
          ? "wallet-approval:eth-sign-smoke"
          : "wallet-approval:personal-smoke";
      res.end(JSON.stringify({
        schema: "elastos.browser.wallet-approval-result/v1",
        approval_request: {
          request_id: requestId,
          status: "pending",
          expires_at: Math.floor(Date.now() / 1000) + 600,
        },
        requires_wallet_approval: true,
      }));
      return;
    }
    if (req.method === "POST" && target.pathname === "/api/apps/browser/wallet/request-transaction") {
      res.end(JSON.stringify({
        schema: "elastos.browser.wallet-approval-result/v1",
        approval_request: {
          request_id: "wallet-approval:tx-smoke",
          status: "pending",
          expires_at: Math.floor(Date.now() / 1000) + 600,
        },
        requires_wallet_approval: true,
      }));
      return;
    }
    if (req.method === "GET" && target.pathname === "/api/apps/browser/wallet/approvals/wallet-approval%3Atx-smoke") {
      res.end(JSON.stringify({
        schema: "elastos.browser.wallet-approval-status/v1",
        request_id: "wallet-approval:tx-smoke",
        status: "completed",
        signed_transaction: "0x02f8",
        signed_result: {
          schema: "elastos.wallet.signed-transaction-result/v1",
          request_id: "wallet-approval:tx-smoke",
          method: "eth_sendTransaction",
          signed_transaction: "0x02f8",
          chain_namespace: "eip155:20",
        },
      }));
      return;
    }
    if (req.method === "POST" && target.pathname === "/api/apps/browser/wallet/broadcast-transaction") {
      res.end(JSON.stringify({
        schema: "elastos.browser.transaction-broadcast/v1",
        request_id: "wallet-approval:tx-smoke",
        transaction_hash: "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      }));
      return;
    }
    res.writeHead(404, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: `unexpected runtime proxy target: ${target.pathname}` }));
  });
});

server.listen(0, "127.0.0.1", () => {
  fs.writeFileSync(readyPath, JSON.stringify({ port: server.address().port }));
});
NODE

"$node_bin" "$tmp_dir/fake-runtime-proxy.mjs" \
  "$tmp_dir/fake-runtime-proxy-ready.json" \
  "$tmp_dir/fake-runtime-proxy-requests.jsonl" &
proxy_pid="$!"
for _ in {1..100}; do
  [[ -s "$tmp_dir/fake-runtime-proxy-ready.json" ]] && break
  sleep 0.02
done
[[ -s "$tmp_dir/fake-runtime-proxy-ready.json" ]]
runtime_proxy_port="$("$node_bin" -e 'console.log(JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).port)' "$tmp_dir/fake-runtime-proxy-ready.json")"

default_control_socket="$tmp_dir/control-default-auto.sock"
default_control_config="$("$node_bin" -e 'console.log(JSON.stringify({
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
    { urls: ["stun:stun.example.invalid:3478"] },
    { urls: ["turn:turn.example.invalid:3478"], username: "smoke-user", credential: "smoke-secret" }
  ],
  connect_timeout_ms: 2000,
  signal_timeout_ms: 2000
}))' "$default_control_socket" "$selkies_port" "$cdp_port")"
ELASTOS_BROWSER_SELKIES_CONTROL_CONFIG="$default_control_config" \
  scripts/browser-selkies-control-service.mjs >"$tmp_dir/control-default-auto.log" 2>&1 &
control_pid="$!"
for _ in {1..100}; do
  [[ -S "$default_control_socket" ]] && break
  sleep 0.02
done
[[ -S "$default_control_socket" ]]

default_status="$tmp_dir/default-auto-status.json"
curl --silent --show-error --fail \
  --unix-socket "$default_control_socket" \
  "http://browser-engine/status" >"$default_status"
"$node_bin" -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.signaling_protocol !== "auto") throw new Error(`default signaling protocol should be auto: ${JSON.stringify(response)}`);
' "$default_status"

curl --silent --show-error --fail \
  --unix-socket "$default_control_socket" \
  --request POST \
  "http://browser-engine/shutdown" >/dev/null
wait "$control_pid" 2>/dev/null || true
control_pid=""

control_socket="$tmp_dir/control.sock"
control_config="$("$node_bin" -e 'console.log(JSON.stringify({
  schema: "elastos.browser.selkies-control.config/v1",
  control_socket_path: process.argv[1],
  replace_existing_socket: true,
  selkies_ws_url: `ws://127.0.0.1:${process.argv[2]}/signaling`,
  runtime_fetch_proxy_url: `http://127.0.0.1:${process.argv[4]}`,
  signaling_protocol: "auto",
  browser_control: {
    kind: "cdp_http",
    endpoint: `http://127.0.0.1:${process.argv[3]}`,
    timeout_ms: 2000
  },
  ice_servers: [
    { urls: ["stun:stun.example.invalid:3478"] },
    { urls: ["turn:turn.example.invalid:3478"], username: "smoke-user", credential: "smoke-secret" }
  ],
  connect_timeout_ms: 2000,
  signal_timeout_ms: 2000
}))' "$control_socket" "$selkies_port" "$cdp_port" "$runtime_proxy_port")"
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
"$node_bin" -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.schema !== "elastos.browser.selkies-control.status/v1") throw new Error("wrong control status schema");
if (response.active_pages !== 0 || response.single_session !== true || response.direct_network !== false) throw new Error("wrong initial control status");
if (response.runtime_fetch_proxy_url !== `http://127.0.0.1:${process.argv[2]}`) throw new Error("Runtime fetch proxy URL was not exposed in status");
' "$initial_status" "$runtime_proxy_port"

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
    "guarantee_level": "operator_rbi",
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
      "default_chain_namespace": "eip155:20",
      "default_account_id": "wallet:eip155:20:0x1111111111111111111111111111111111111111",
      "bridge_url": "http://runtime.local/api/apps/browser/wallet/bridge",
      "read_url": "http://runtime.local/api/apps/browser/wallet/read",
      "approval_url": "http://runtime.local/api/apps/browser/wallet/request-signature",
      "transaction_url": "http://runtime.local/api/apps/browser/wallet/request-transaction",
      "transaction_broadcast_url": "http://runtime.local/api/apps/browser/wallet/broadcast-transaction",
      "approval_status_url": "http://runtime.local/api/apps/browser/wallet/approvals",
      "home_token": "wallet-token-smoke"
    }
  }
}
JSON
)"
if [[ "$failed_status" != "503" ]]; then
  cat "$failed_open_response" >&2 || true
  echo "expected intentionally failed hosted Selkies open to return 503, got $failed_status" >&2
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
"$node_bin" -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.active_pages !== 0) throw new Error("failed open leaked an active Selkies page");
' "$after_failed_status"

new_target_count_before_recovery="$(read_new_target_count)"
vm_guest_open_response="$tmp_dir/vm-guest-open-response.json"
if ! curl --silent --show-error --fail-with-body \
  --unix-socket "$control_socket" \
  --header "content-type: application/json" \
  --data @- \
  http://browser-engine/pages >"$vm_guest_open_response" <<'JSON'
{
  "schema": "elastos.browser.vm-guest.open/v1",
  "launch_request": {
    "schema": "elastos.browser.engine.launch-request/v1",
    "adapter": "browser-vm-product",
    "engine": "selkies_gstreamer",
    "stream_id": "vm-guest-webrtc-smoke",
    "url": "https://example.com/",
    "display_mode": "webrtc_remote_display",
    "guarantee_level": "mechanism_microvm",
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
  cat "$vm_guest_open_response" >&2 || true
  cat "$tmp_dir/control.log" >&2 || true
  exit 1
fi
"$node_bin" -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.schema !== "elastos.browser.engine.supervisor-result/v1") throw new Error("wrong VM guest supervisor schema");
if (response.adapter !== "browser-vm-product" || response.engine !== "selkies_gstreamer") throw new Error("VM guest Selkies service changed launch identity unexpectedly");
if (response.display_session?.mode !== "webrtc_remote_display") throw new Error("VM guest open did not return WebRTC display");
if (response.display_session?.media_transport !== "runtime_relay") throw new Error("VM guest display did not use Runtime media relay");
' "$vm_guest_open_response"
new_target_count_after_recovery="$(read_new_target_count)"
if [[ "$new_target_count_after_recovery" -le "$new_target_count_before_recovery" ]]; then
  echo "post-close Selkies recovery reused a stale CDP target instead of allocating a fresh target" >&2
  cat "$tmp_dir/control.log" >&2 || true
  exit 1
fi
vm_guest_page_id="$("$node_bin" -e 'const r=JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")); console.log(encodeURIComponent(r.page_id));' "$vm_guest_open_response")"
curl --silent --show-error --fail \
  --unix-socket "$control_socket" \
  --request POST \
  "http://browser-engine/pages/$vm_guest_page_id/close" >/dev/null

network_changed_open_response="$tmp_dir/network-changed-open-response.json"
if ! curl --silent --show-error --fail-with-body \
  --unix-socket "$control_socket" \
  --header "content-type: application/json" \
  --data @- \
  http://browser-engine/pages >"$network_changed_open_response" <<'JSON'
{
  "schema": "elastos.browser.hosted-product.open/v1",
  "launch_request": {
    "schema": "elastos.browser.engine.launch-request/v1",
    "adapter": "hosted-product",
    "engine": "selkies_gstreamer",
    "stream_id": "smoke-stream-network-changed",
    "url": "https://example.com/?network-changed=1",
    "display_mode": "webrtc_remote_display",
    "guarantee_level": "operator_rbi",
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
  cat "$network_changed_open_response" >&2 || true
  cat "$tmp_dir/control.log" >&2 || true
  exit 1
fi
"$node_bin" -e '
const fs = require("fs");
const response = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
const attempts = Number(fs.readFileSync(process.argv[2], "utf8"));
if (response.schema !== "elastos.browser.engine.supervisor-result/v1") throw new Error("wrong network-changed supervisor schema");
if (response.actual_url !== "https://example.com/?network-changed=1") throw new Error("network-changed repair changed the URL");
if (response.direct_network !== false || response.display_session?.network_mode !== "runtime_net_only") throw new Error("network-changed repair left the Runtime network boundary");
if (attempts < 2) throw new Error(`initial ERR_NETWORK_CHANGED must be repaired before open returns; attempts=${attempts}`);
' "$network_changed_open_response" "$tmp_dir/fake-cdp-ready.json.network-changed-navigations"
network_changed_page_id="$("$node_bin" -e 'const r=JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")); console.log(encodeURIComponent(r.page_id));' "$network_changed_open_response")"
curl --silent --show-error --fail \
  --unix-socket "$control_socket" \
  --request POST \
  "http://browser-engine/pages/$network_changed_page_id/close" >/dev/null

slow_dom_open_response="$tmp_dir/slow-dom-open-response.json"
if ! curl --silent --show-error --fail-with-body \
  --unix-socket "$control_socket" \
  --header "content-type: application/json" \
  --data @- \
  http://browser-engine/pages >"$slow_dom_open_response" <<'JSON'
{
  "schema": "elastos.browser.hosted-product.open/v1",
  "launch_request": {
    "schema": "elastos.browser.engine.launch-request/v1",
    "adapter": "hosted-product",
    "engine": "selkies_gstreamer",
    "stream_id": "smoke-stream-slow-dom",
    "url": "https://example.com/?slow-dom=1",
    "display_mode": "webrtc_remote_display",
    "guarantee_level": "operator_rbi",
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
  cat "$slow_dom_open_response" >&2 || true
  cat "$tmp_dir/control.log" >&2 || true
  exit 1
fi
"$node_bin" -e '
const fs = require("fs");
const response = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
if (response.schema !== "elastos.browser.engine.supervisor-result/v1") throw new Error("wrong slow-DOM supervisor schema");
if (response.actual_url !== "https://example.com/?slow-dom=1") throw new Error("slow-DOM open changed the URL");
if (!fs.existsSync(process.argv[2])) throw new Error("open returned before the requested navigation DOMContent event fired");
if (response.direct_network !== false || response.display_session?.network_mode !== "runtime_net_only") throw new Error("slow-DOM open left the Runtime network boundary");
' "$slow_dom_open_response" "$tmp_dir/fake-cdp-ready.json.slow-dom-fired"
slow_dom_page_id="$("$node_bin" -e 'const r=JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")); console.log(encodeURIComponent(r.page_id));' "$slow_dom_open_response")"
curl --silent --show-error --fail \
  --unix-socket "$control_socket" \
  --request POST \
  "http://browser-engine/pages/$slow_dom_page_id/close" >/dev/null

printf '1' >"$tmp_dir/fake-selkies-force-legacy.txt"
legacy_open_response="$tmp_dir/legacy-open-response.json"
if ! curl --silent --show-error --fail-with-body \
  --unix-socket "$control_socket" \
  --header "content-type: application/json" \
  --data @- \
  http://browser-engine/pages >"$legacy_open_response" <<'JSON'
{
  "schema": "elastos.browser.hosted-product.open/v1",
  "launch_request": {
    "schema": "elastos.browser.engine.launch-request/v1",
    "adapter": "hosted-product",
    "engine": "selkies_gstreamer",
    "stream_id": "smoke-stream-legacy",
    "url": "https://example.com/",
    "display_mode": "webrtc_remote_display",
    "guarantee_level": "operator_rbi",
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
  cat "$legacy_open_response" >&2 || true
  cat "$tmp_dir/control.log" >&2 || true
  exit 1
fi
"$node_bin" -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.schema !== "elastos.browser.engine.supervisor-result/v1") throw new Error("wrong legacy supervisor schema");
if (response.display_session?.backend_class !== "product_compositor") throw new Error("legacy signaling path did not return product compositor display");
if (response.display_session?.initial_offer?.schema !== "elastos.browser.webrtc-offer/v1") throw new Error("legacy signaling path did not return a WebRTC offer");
if (!response.display_session.initial_offer.sdp.includes("m=video")) throw new Error("legacy signaling offer missing video");
' "$legacy_open_response"
"$node_bin" -e '
const meta = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (meta.res !== "1920x1080" || meta.scale !== 1) throw new Error(`wrong legacy HELLO metadata: ${JSON.stringify(meta)}`);
' "$tmp_dir/fake-selkies-legacy-hello.json"
legacy_page_id="$("$node_bin" -e 'const r=JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")); console.log(encodeURIComponent(r.page_id));' "$legacy_open_response")"
legacy_answer_response="$tmp_dir/legacy-answer-response.json"
curl --silent --show-error --fail \
  --unix-socket "$control_socket" \
  --header "content-type: application/json" \
  --data @- \
  "http://browser-engine/pages/$legacy_page_id/webrtc" >"$legacy_answer_response" <<'JSON'
{
  "signal": {
    "schema": "elastos.browser.webrtc-answer/v1",
    "type": "answer",
    "sdp": "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=ElastOS Legacy Browser Answer\r\nt=0 0\r\n"
  }
}
JSON
"$node_bin" -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.schema !== "elastos.browser.webrtc-signal-ack/v1") throw new Error("wrong legacy answer ack schema");
if (response.type !== "answer" || response.accepted !== true) throw new Error("legacy answer was not accepted");
' "$legacy_answer_response"
for _ in {1..100}; do
  [[ -s "$tmp_dir/fake-selkies-legacy-answer.txt" ]] && break
  sleep 0.02
done
if [[ ! -s "$tmp_dir/fake-selkies-legacy-answer.txt" ]]; then
  echo "Selkies legacy signaling path did not send a raw JSON answer" >&2
  cat "$tmp_dir/control.log" >&2 || true
  exit 1
fi
sleep 0.1
legacy_candidate_response="$tmp_dir/legacy-candidate-response.json"
curl --silent --show-error --fail \
  --unix-socket "$control_socket" \
  --header "content-type: application/json" \
  --data @- \
  "http://browser-engine/pages/$legacy_page_id/webrtc" >"$legacy_candidate_response" <<'JSON'
{
  "signal": {
    "schema": "elastos.browser.webrtc-candidate/v1",
    "type": "candidate",
    "candidate": {
      "candidate": "candidate:browser-legacy-smoke 1 udp 1 127.0.0.1 9 typ host",
      "sdpMid": "0",
      "sdpMLineIndex": 0
    }
  }
}
JSON
"$node_bin" -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.schema !== "elastos.browser.webrtc-signal-ack/v1") throw new Error("wrong legacy candidate ack schema");
if (response.type !== "candidate" || response.accepted !== true) throw new Error("legacy candidate was not accepted");
if (response.candidates?.[0]?.candidate !== "candidate:smoke 1 udp 1 127.0.0.1 9 typ host") throw new Error("legacy raw JSON candidate was not returned in candidate ack");
' "$legacy_candidate_response"
curl --silent --show-error --fail \
  --unix-socket "$control_socket" \
  --request POST \
  "http://browser-engine/pages/$legacy_page_id/close" >/dev/null

printf '1' >"$tmp_dir/fake-selkies-audio-unavailable.txt"
audio_required_open_response="$tmp_dir/audio-required-open-response.json"
if curl --silent --show-error --fail-with-body \
  --unix-socket "$control_socket" \
  --header "content-type: application/json" \
  --data @- \
  http://browser-engine/pages >"$audio_required_open_response" <<'JSON'
{
  "schema": "elastos.browser.hosted-product.open/v1",
  "launch_request": {
    "schema": "elastos.browser.engine.launch-request/v1",
    "adapter": "hosted-product",
    "engine": "selkies_gstreamer",
    "stream_id": "smoke-stream-audio-required",
    "url": "https://example.com/",
    "display_mode": "webrtc_remote_display",
    "guarantee_level": "operator_rbi",
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
  echo "audio-unavailable product display launch unexpectedly succeeded" >&2
  cat "$audio_required_open_response" >&2 || true
  cat "$tmp_dir/control.log" >&2 || true
  exit 1
fi
"$node_bin" -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (!String(response.error || "").includes("Selkies audio")) {
  throw new Error(`audio-unavailable launch did not fail with a Selkies audio error: ${JSON.stringify(response)}`);
}
' "$audio_required_open_response"
rm -f "$tmp_dir/fake-selkies-audio-unavailable.txt"

: >"$tmp_dir/fake-runtime-proxy-requests.jsonl"
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
    "guarantee_level": "operator_rbi",
    "network_mode": "runtime_net_only",
    "direct_network": false,
    "viewport": {
      "width": 1281,
      "height": 721
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
      "default_chain_namespace": "eip155:20",
      "default_account_id": "wallet:eip155:20:0x1111111111111111111111111111111111111111",
      "bridge_url": "http://runtime.local/api/apps/browser/wallet/bridge",
      "read_url": "http://runtime.local/api/apps/browser/wallet/read",
      "approval_url": "http://runtime.local/api/apps/browser/wallet/request-signature",
      "transaction_url": "http://runtime.local/api/apps/browser/wallet/request-transaction",
      "transaction_broadcast_url": "http://runtime.local/api/apps/browser/wallet/broadcast-transaction",
      "approval_status_url": "http://runtime.local/api/apps/browser/wallet/approvals",
      "home_token": "wallet-token-smoke"
    }
  }
}
JSON
then
  cat "$open_response" >&2 || true
  cat "$tmp_dir/control.log" >&2 || true
  exit 1
fi

"$node_bin" -e '
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
if (display.media_transport !== "runtime_relay") throw new Error("Selkies display must report runtime_relay media transport");
if (display.offerer !== "engine") throw new Error("Selkies display must be engine-offer");
if (display.input !== "datachannel" || display.input_protocol !== "selkies_v1") throw new Error("Selkies display must declare datachannel selkies_v1 input");
if (display.width !== 1920 || display.height !== 1080) throw new Error("Selkies display must expose the fixed stream/input coordinate space");
if (response.view?.schema !== "elastos.browser.view/v1" || response.view.width !== 1280 || response.view.height !== 720) throw new Error("Selkies result must expose a matching Browser view");
if (display.initial_offer?.schema !== "elastos.browser.webrtc-offer/v1") throw new Error("missing initial offer");
if (!display.initial_offer.sdp.includes("m=video")) throw new Error("initial offer must include video");
if (display.audio_offer?.schema !== "elastos.browser.webrtc-offer/v1" || !display.audio_offer.sdp.includes("m=audio")) throw new Error("audio offer must include audio");
if (display.audio !== true || display.video !== true || display.direct_network !== false) throw new Error("wrong media/network flags");
if (display.ice_servers?.[0]?.urls?.[0] !== "stun:stun.example.invalid:3478") throw new Error("ICE servers were not propagated to display session");
' "$open_response"
"$node_bin" -e '
const initScript = require("fs").readFileSync(process.argv[1], "utf8");
for (const expected of [
  "walletApprovalPending",
  "waitForCachedWalletApproval",
  "approval_reuse",
  "approval_request",
]) {
  if (!initScript.includes(expected)) {
    throw new Error(`Browser wallet init script is missing approval coalescing marker: ${expected}`);
  }
}
' "$tmp_dir/fake-cdp-ready.json.init-script"
for _ in {1..100}; do
  proxy_request_count="$(wc -l <"$tmp_dir/fake-runtime-proxy-requests.jsonl" | tr -d ' ')"
  [[ "$proxy_request_count" -ge 8 ]] && break
  sleep 0.02
done
if [[ "$proxy_request_count" -lt 8 ]]; then
  echo "Runtime wallet proxy received only $proxy_request_count requests; expected at least 8" >&2
  exit 1
fi
"$node_bin" -e '
const fs = require("fs");
const lines = fs.readFileSync(process.argv[1], "utf8").trim().split(/\n/).filter(Boolean);
const requests = lines.map((line) => JSON.parse(line));
const seen = new Map(requests.map((request) => [request.pathname, request]));
for (const path of [
  "/api/apps/browser/wallet/bridge",
  "/api/apps/browser/wallet/read",
  "/api/apps/browser/wallet/request-transaction",
  "/api/apps/browser/wallet/approvals/wallet-approval%3Atx-smoke",
  "/api/apps/browser/wallet/broadcast-transaction",
]) {
  if (!seen.has(path)) {
    throw new Error(`Runtime wallet proxy did not receive ${path}: ${JSON.stringify(requests)}`);
  }
}
for (const request of requests) {
  if (request.origin !== "null") {
    throw new Error(`Runtime wallet proxy request did not carry exact Origin: null: ${JSON.stringify(request)}`);
  }
  if (request.token !== "wallet-token-smoke") {
    throw new Error(`Runtime wallet proxy request lost Home token: ${JSON.stringify(request)}`);
  }
  if (request.host !== "runtime.local") {
    throw new Error(`Runtime wallet proxy request was not absolute-form for runtime.local: ${JSON.stringify(request)}`);
  }
}
if (seen.get("/api/apps/browser/wallet/read").body?.method !== "eth_chainId") {
  throw new Error("Runtime wallet read request body was not delivered through the proxy");
}
const signatureRequests = requests.filter((request) => request.pathname === "/api/apps/browser/wallet/request-signature");
if (signatureRequests.length !== 3) {
  throw new Error(`Runtime wallet signature requests did not cover personal_sign, eth_sign, and typed-data: ${JSON.stringify(signatureRequests)}`);
}
const personalSign = signatureRequests.find((request) => request.body?.method === "personal_sign" && request.body?.params?.[0] === "0x68656c6c6f");
if (!personalSign) {
  throw new Error(`Runtime wallet personal_sign request body was not delivered through the proxy: ${JSON.stringify(signatureRequests)}`);
}
const normalizedEthSign = signatureRequests.find((request) => request.body?.method === "personal_sign" && request.body?.params?.[0] === "0x6574682d7369676e");
if (!normalizedEthSign || normalizedEthSign.body?.params?.[1] !== "0x1111111111111111111111111111111111111111") {
  throw new Error(`Runtime wallet eth_sign request was not normalized to personal_sign: ${JSON.stringify(signatureRequests)}`);
}
const typedData = signatureRequests.find((request) => request.body?.method === "eth_signTypedData_v4");
if (!typedData || typedData.body?.params?.[0] !== "0x1111111111111111111111111111111111111111" || typedData.body?.params?.[1]?.primaryType !== "Login") {
  throw new Error(`Runtime wallet typed-data request body was not delivered through the proxy: ${JSON.stringify(signatureRequests)}`);
}
if (seen.get("/api/apps/browser/wallet/request-transaction").body?.method !== "eth_sendTransaction") {
  throw new Error("Runtime wallet transaction request body was not delivered through the proxy");
}
if (seen.get("/api/apps/browser/wallet/broadcast-transaction").body?.request_id !== "wallet-approval:tx-smoke") {
  throw new Error("Runtime wallet transaction broadcast body was not delivered through the proxy");
}
' "$tmp_dir/fake-runtime-proxy-requests.jsonl"
"$node_bin" -e '
const viewport = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (viewport.width !== 1280 || viewport.height !== 720) throw new Error("initial CDP viewport did not preserve display aspect ratio");
if (viewport.deviceScaleFactor !== 1) throw new Error("initial CDP viewport did not apply one-to-one VM scale factor");
' "$tmp_dir/fake-cdp-ready.json.viewport"

page_id="$("$node_bin" -e 'const r=JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")); console.log(encodeURIComponent(r.page_id));' "$open_response")"

active_status="$tmp_dir/active-status.json"
curl --silent --show-error --fail \
  --unix-socket "$control_socket" \
  "http://browser-engine/status" >"$active_status"
"$node_bin" -e '
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
    "guarantee_level": "operator_rbi",
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
"$node_bin" -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
const oldPage = decodeURIComponent(process.argv[2]);
if (response.schema !== "elastos.browser.engine.supervisor-result/v1") throw new Error("wrong replacement supervisor schema");
if (!/^page:selkies-[0-9a-f]{16}$/.test(response.page_id)) throw new Error("replacement page id must be opaque");
if (response.page_id === oldPage) throw new Error("replacement open must allocate a fresh page id");
if (response.direct_network !== false || response.display_session?.network_mode !== "runtime_net_only") throw new Error("replacement lost Runtime network boundary");
' "$replacement_response" "$old_page_id"
preserved_status="$(curl --silent --show-error \
  --output "$tmp_dir/preserved-page-status.json" \
  --write-out "%{http_code}" \
  --unix-socket "$control_socket" \
  "http://browser-engine/pages/$old_page_id/status")"
if [[ "$preserved_status" != "200" ]]; then
  cat "$tmp_dir/preserved-page-status.json" >&2 || true
  echo "expected previous Selkies page to remain routable, got $preserved_status" >&2
  exit 1
fi
"$node_bin" -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
const oldPage = decodeURIComponent(process.argv[2]);
if (response.schema !== "elastos.browser.page-status/v1") throw new Error("wrong preserved page status schema");
if (response.page_id !== oldPage) throw new Error("preserved status returned the wrong page id");
' "$tmp_dir/preserved-page-status.json" "$old_page_id"
multi_status="$tmp_dir/multi-status.json"
curl --silent --show-error --fail \
  --unix-socket "$control_socket" \
  "http://browser-engine/status" >"$multi_status"
"$node_bin" -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.active_pages !== 2) throw new Error(`second Selkies page should keep both pages active: ${JSON.stringify(response)}`);
if (response.single_session !== false || response.single_vm_session !== true) throw new Error("Selkies status did not report multi-page/single-VM shape");
' "$multi_status"
page_id="$("$node_bin" -e 'const r=JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")); console.log(encodeURIComponent(r.page_id));' "$replacement_response")"

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
"$node_bin" -e '
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
"$node_bin" -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.schema !== "elastos.browser.webrtc-signal-ack/v1") throw new Error("wrong candidate ack schema");
if (response.type !== "candidate" || response.accepted !== true) throw new Error("candidate was not accepted");
if (response.candidates?.[0]?.candidate !== "candidate:smoke 1 udp 1 127.0.0.1 9 typ host") throw new Error("queued Selkies candidate was not returned in candidate ack");
' "$candidate_response"

reattach_answer_response="$tmp_dir/reattach-answer-response.json"
curl --silent --show-error --fail \
  --unix-socket "$control_socket" \
  --header "content-type: application/json" \
  --data @- \
  "http://browser-engine/pages/$page_id/webrtc" >"$reattach_answer_response" <<'JSON'
{
  "signal": {
    "schema": "elastos.browser.webrtc-answer/v1",
    "type": "answer",
    "sdp": "v=0\r\no=- 1 0 IN IP4 127.0.0.1\r\ns=ElastOS Browser Reattach Answer\r\nt=0 0\r\n"
  }
}
JSON
"$node_bin" -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.schema !== "elastos.browser.webrtc-signal-ack/v1") throw new Error("wrong reattach answer ack schema");
if (response.type !== "answer" || response.accepted !== true) throw new Error("reattach answer was not accepted");
if (response.candidates?.[0]?.candidate !== "candidate:smoke 1 udp 1 127.0.0.1 9 typ host") throw new Error("reattached Browser answer did not receive Selkies candidate history");
' "$reattach_answer_response"

status_response="$tmp_dir/status-response.json"
curl --silent --show-error --fail \
  --unix-socket "$control_socket" \
  "http://browser-engine/pages/$page_id/status" >"$status_response"
"$node_bin" -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.schema !== "elastos.browser.page-status/v1") throw new Error("wrong status schema");
if (response.state_source !== "cdp") throw new Error("full page status must be CDP-backed");
if (response.backend_class !== "product_compositor" || response.audio !== true || response.video !== true) throw new Error("wrong page status");
if (response.input_protocol !== "selkies_v1") throw new Error("status lost Selkies input protocol");
if (response.display_session?.media_transport !== "runtime_relay") throw new Error("status lost Runtime media relay proof");
const display = response.display_session || {};
const credentialedTurn = (display.ice_servers || []).some((server) => {
  const urls = Array.isArray(server.urls) ? server.urls : [server.urls].filter(Boolean);
  return urls.some((url) => /^turns?:/i.test(String(url || ""))) &&
    server.username_present === true &&
    server.credential_present === true &&
    Number(server.credential_length || 0) > 0;
});
if (!credentialedTurn) throw new Error("status lost redacted TURN credential proof");
const redactedDisplay = JSON.stringify(display);
if (redactedDisplay.includes("smoke-secret")) throw new Error("status leaked TURN credential");
if (redactedDisplay.includes("smoke-user")) throw new Error("status leaked TURN username");
' "$status_response"

curl --silent --show-error --fail \
  --request POST \
  "http://127.0.0.1:$cdp_port/__fake/navigate?url=https%3A%2F%2Fexample.com%2F%3Felastos-browser-nav-smoke%3D1" >/dev/null
fast_status_response="$tmp_dir/fast-status-response.json"
curl --silent --show-error --fail \
  --unix-socket "$control_socket" \
  "http://browser-engine/pages/$page_id/status?fast=1" >"$fast_status_response"
"$node_bin" -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.schema !== "elastos.browser.page-status/v1") throw new Error("wrong fast status schema");
if (response.state_source !== "cache") throw new Error("fast page status must be cache-backed");
if (response.actual_url !== "https://example.com/") throw new Error(`fast status unexpectedly refreshed CDP URL: ${JSON.stringify(response)}`);
' "$fast_status_response"
datachannel_status_response="$tmp_dir/datachannel-status-response.json"
curl --silent --show-error --fail \
  --unix-socket "$control_socket" \
  "http://browser-engine/pages/$page_id/status" >"$datachannel_status_response"
"$node_bin" -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.schema !== "elastos.browser.page-status/v1") throw new Error("wrong datachannel status schema");
if (response.state_source !== "cdp") throw new Error("full status after datachannel navigation must be CDP-backed");
if (response.actual_url !== "https://example.com/?elastos-browser-nav-smoke=1") throw new Error(`status did not refresh CDP URL after datachannel navigation: ${JSON.stringify(response)}`);
' "$datachannel_status_response"
curl --silent --show-error --fail \
  --request POST \
  "http://127.0.0.1:$cdp_port/__fake/navigate?url=https%3A%2F%2Fexample.com%2F" >/dev/null

pre_click_status_response="$tmp_dir/pre-click-status-response.json"
curl --silent --show-error --fail \
  --unix-socket "$control_socket" \
  "http://browser-engine/pages/$page_id/status" >"$pre_click_status_response"
"$node_bin" -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.actual_url !== "https://example.com/") throw new Error("pre-click status did not reset to the first URL");
' "$pre_click_status_response"

click_response="$tmp_dir/click-response.json"
curl --silent --show-error --fail \
  --unix-socket "$control_socket" \
  --header "content-type: application/json" \
  --data @- \
  "http://browser-engine/pages/$page_id/input" >"$click_response" <<'JSON'
{
  "event": {
    "type": "click",
    "x": 320,
    "y": 240
  }
}
JSON
"$node_bin" -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.schema !== "elastos.browser.input-result/v1") throw new Error("wrong click result schema");
if (response.accepted !== true || response.direct_network !== false) throw new Error("click was not accepted fail-closed through Runtime route");
if (response.actual_url !== "https://example.com/?elastos-browser-nav-smoke=1") throw new Error(`click navigation did not return fresh URL state: ${JSON.stringify(response)}`);
if (response.file_chooser?.pending !== true || response.file_chooser?.mode !== "selectSingle") throw new Error("click did not surface pending Runtime file chooser state");
' "$click_response"

file_upload_response="$tmp_dir/file-upload-response.json"
curl --silent --show-error --fail \
  --unix-socket "$control_socket" \
  --header "content-type: application/json" \
  --data @- \
  "http://browser-engine/pages/$page_id/input" >"$file_upload_response" <<'JSON'
{
  "event": {
    "type": "file_upload",
    "file_name": "avatar.png",
    "mime_type": "image/png",
    "content_base64": "SGVsbG8gQnJvd3Nlcg==",
    "object_uri": "elastos://object/documents/avatar.png"
  }
}
JSON
"$node_bin" -e '
const fs = require("fs");
const response = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
const uploaded = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
if (response.schema !== "elastos.browser.input-result/v1") throw new Error("wrong file_upload result schema");
if (response.accepted !== true || response.direct_network !== false) throw new Error("file_upload was not accepted through Runtime route");
if (response.file_chooser?.pending !== false) throw new Error("file_upload did not clear the pending file chooser");
if (response.file_upload?.file_name !== "avatar.png" || response.file_upload?.mime_type !== "image/png") throw new Error("file_upload did not return selected Library item metadata");
if (uploaded.fileName !== "avatar.png" || uploaded.mimeType !== "image/png" || uploaded.size !== 13) throw new Error(`CDP file target did not receive the Library item: ${JSON.stringify(uploaded)}`);
' "$file_upload_response" "$tmp_dir/fake-cdp-ready.json.uploaded-file"

curl --silent --show-error --fail \
  --request POST \
  "http://127.0.0.1:$cdp_port/__fake/navigate?url=https%3A%2F%2Fexample.com%2F" >/dev/null

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
"$node_bin" -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.schema !== "elastos.browser.input-result/v1") throw new Error("wrong resize result schema");
if (response.accepted !== true || response.direct_network !== false) throw new Error("resize was not accepted fail-closed through Runtime route");
if (response.width !== 992 || response.height !== 558) throw new Error("resize must return an aspect-preserving Browser viewport");
const viewport = JSON.parse(require("fs").readFileSync(process.argv[2], "utf8"));
if (viewport.width !== 992 || viewport.height !== 558) throw new Error("resize must update the CDP viewport with preserved display aspect");
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
"$node_bin" -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.schema !== "elastos.browser.input-result/v1") throw new Error("wrong navigate result schema");
if (response.accepted !== true || response.direct_network !== false) throw new Error("navigate was not accepted fail-closed through Runtime route");
if (response.actual_url !== "https://example.com/?elastos-browser-nav-smoke=1") throw new Error(`navigate did not update URL state: ${JSON.stringify(response)}`);
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
"$node_bin" -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.schema !== "elastos.browser.input-result/v1") throw new Error("wrong back result schema");
if (response.actual_url !== "https://example.com/") throw new Error(`back did not return to the first page: ${JSON.stringify(response)}`);
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
"$node_bin" -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.schema !== "elastos.browser.input-result/v1") throw new Error("wrong forward result schema");
if (response.actual_url !== "https://example.com/?elastos-browser-nav-smoke=1") throw new Error(`forward did not return to the second page: ${JSON.stringify(response)}`);
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
"$node_bin" -e '
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (response.schema !== "elastos.browser.input-result/v1") throw new Error("wrong input result schema");
if (response.accepted !== true || response.direct_network !== false) throw new Error("browser command was not accepted fail-closed through Runtime route");
if (response.actual_url !== "https://example.com/?elastos-browser-nav-smoke=1" || response.title !== "Example Domain") throw new Error("browser command did not return current page state");
' "$input_response"

closed_once_response="$tmp_dir/closed-once-response.json"
curl --silent --show-error --fail-with-body \
  --unix-socket "$control_socket" \
  --header "content-type: application/json" \
  --output "$closed_once_response" \
  --data @- \
  "http://browser-engine/pages/$page_id/input" <<'JSON'
{
  "event": {
    "type": "browser_command",
    "command": "navigate",
    "url": "https://example.com/?closed-once=1"
  }
}
JSON
"$node_bin" -e '
const fs = require("fs");
const response = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
const attempts = Number(fs.readFileSync(process.argv[2], "utf8"));
if (response.schema !== "elastos.browser.input-result/v1") throw new Error("wrong closed-once input result schema");
if (response.accepted !== true || response.direct_network !== false) throw new Error("closed-once navigation was not accepted through Runtime route");
if (response.actual_url !== "https://example.com/?closed-once=1") throw new Error(`closed-once navigation did not land on retry URL: ${JSON.stringify(response)}`);
if (attempts < 2) throw new Error(`closed-once navigation must retry after ERR_CONNECTION_CLOSED; attempts=${attempts}`);
' "$closed_once_response" "$tmp_dir/fake-cdp-ready.json.closed-once-navigations"

timeout_once_response="$tmp_dir/timeout-once-response.json"
curl --silent --show-error --fail-with-body \
  --unix-socket "$control_socket" \
  --header "content-type: application/json" \
  --output "$timeout_once_response" \
  --data @- \
  "http://browser-engine/pages/$page_id/input" <<'JSON'
{
  "event": {
    "type": "browser_command",
    "command": "navigate",
    "url": "https://example.com/?timeout-once=1"
  }
}
JSON
"$node_bin" -e '
const fs = require("fs");
const response = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
const attempts = Number(fs.readFileSync(process.argv[2], "utf8"));
if (response.schema !== "elastos.browser.input-result/v1") throw new Error("wrong timeout-once input result schema");
if (response.accepted !== true || response.direct_network !== false) throw new Error("timeout-once navigation was not accepted through Runtime route");
if (response.actual_url !== "https://example.com/?timeout-once=1") throw new Error(`timeout-once navigation did not land on retry URL: ${JSON.stringify(response)}`);
if (attempts < 2) throw new Error(`timeout-once navigation must retry after CDP Page.navigate timeout; attempts=${attempts}`);
' "$timeout_once_response" "$tmp_dir/fake-cdp-ready.json.timeout-once-navigations"

net_timeout_once_response="$tmp_dir/net-timeout-once-response.json"
curl --silent --show-error --fail-with-body \
  --unix-socket "$control_socket" \
  --header "content-type: application/json" \
  --output "$net_timeout_once_response" \
  --data @- \
  "http://browser-engine/pages/$page_id/input" <<'JSON'
{
  "event": {
    "type": "browser_command",
    "command": "navigate",
    "url": "https://example.com/?net-timeout-once=1"
  }
}
JSON
"$node_bin" -e '
const fs = require("fs");
const response = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
const attempts = Number(fs.readFileSync(process.argv[2], "utf8"));
if (response.schema !== "elastos.browser.input-result/v1") throw new Error("wrong net-timeout-once input result schema");
if (response.accepted !== true || response.direct_network !== false) throw new Error("net-timeout-once navigation was not accepted through Runtime route");
if (response.actual_url !== "https://example.com/?net-timeout-once=1") throw new Error(`net-timeout-once navigation did not land on retry URL: ${JSON.stringify(response)}`);
if (attempts < 2) throw new Error(`net-timeout-once navigation must retry after ERR_TIMED_OUT; attempts=${attempts}`);
' "$net_timeout_once_response" "$tmp_dir/fake-cdp-ready.json.net-timeout-once-navigations"

closed_connection_response="$tmp_dir/closed-connection-response.json"
closed_connection_status="$(
  curl --silent --show-error \
    --unix-socket "$control_socket" \
    --header "content-type: application/json" \
    --output "$closed_connection_response" \
    --write-out "%{http_code}" \
    --data @- \
    "http://browser-engine/pages/$page_id/input" <<'JSON'
{
  "event": {
    "type": "browser_command",
    "command": "navigate",
    "url": "https://docs.ela.city/"
  }
}
JSON
)"
"$node_bin" -e '
const status = Number(process.argv[2]);
const response = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (status < 400) throw new Error(`closed-connection navigation must fail so Browser can recover: ${status} ${JSON.stringify(response)}`);
if (!/ERR_CONNECTION_CLOSED/.test(String(response.error || ""))) throw new Error(`closed-connection navigation did not explain the CDP failure: ${JSON.stringify(response)}`);
' "$closed_connection_response" "$closed_connection_status"

late_chrome_error_target_count_before="$(read_new_target_count)"
late_chrome_error_response="$tmp_dir/late-chrome-error-response.json"
curl --silent --show-error --fail-with-body \
  --unix-socket "$control_socket" \
  --header "content-type: application/json" \
  --output "$late_chrome_error_response" \
  --data @- \
  "http://browser-engine/pages/$page_id/input" <<'JSON'
{
  "event": {
    "type": "browser_command",
    "command": "navigate",
    "url": "https://docs-late.ela.city/"
  }
}
JSON
late_chrome_error_target_count_after="$(read_new_target_count)"
"$node_bin" -e '
const fs = require("fs");
const response = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
const attempts = Number(fs.readFileSync(process.argv[2], "utf8"));
const beforeTargets = Number(process.argv[3]);
const afterTargets = Number(process.argv[4]);
if (response.schema !== "elastos.browser.input-result/v1") throw new Error("wrong late Chrome error input result schema");
if (response.accepted !== true || response.direct_network !== false) throw new Error("late Chrome error navigation was not accepted through Runtime route");
if (response.actual_url !== "https://docs-late.ela.city/") throw new Error(`late Chrome error navigation did not land on replacement target: ${JSON.stringify(response)}`);
if (attempts < 2) throw new Error(`late Chrome error navigation must retry on a fresh target; attempts=${attempts}`);
if (afterTargets <= beforeTargets) throw new Error(`late Chrome error navigation did not allocate a replacement target; before=${beforeTargets} after=${afterTargets}`);
' "$late_chrome_error_response" "$tmp_dir/fake-cdp-ready.json.late-chrome-error-navigations" "$late_chrome_error_target_count_before" "$late_chrome_error_target_count_after"

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
"$node_bin" -e '
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
