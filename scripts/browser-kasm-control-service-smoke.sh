#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d /tmp/elastos-browser-kasm-control-smoke-XXXXXX)"
pids=()

cleanup() {
  for pid in "${pids[@]}"; do
    kill "$pid" >/dev/null 2>&1 || true
  done
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

wait_for_file() {
  local file="$1"
  for _ in {1..100}; do
    [[ -s "$file" ]] && return 0
    sleep 0.02
  done
  echo "timed out waiting for $file" >&2
  return 1
}

wait_for_socket() {
  local socket="$1"
  for _ in {1..100}; do
    [[ -S "$socket" ]] && return 0
    sleep 0.02
  done
  echo "timed out waiting for socket $socket" >&2
  return 1
}

kasm_api_info="$tmp_dir/kasm-api.json"
kasm_api_state="$tmp_dir/kasm-api-state.json"
node - "$kasm_api_info" "$kasm_api_state" <<'NODE' &
const fs = require("node:fs");
const http = require("node:http");
const infoPath = process.argv[2];
const statePath = process.argv[3];
const state = { request: 0, status: 0, delete: 0 };
const writeState = () => fs.writeFileSync(statePath, JSON.stringify(state));
function httpJson(res, status, body) {
  const data = Buffer.from(JSON.stringify(body));
  res.writeHead(status, {
    "content-type": "application/json",
    "content-length": data.length,
  });
  res.end(data);
}
function readBody(req) {
  return new Promise((resolve) => {
    const chunks = [];
    req.on("data", (chunk) => chunks.push(chunk));
    req.on("end", () => resolve(JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}")));
  });
}
const server = http.createServer(async (req, res) => {
  const body = await readBody(req);
  if (body.api_key !== "api-key" || body.api_key_secret !== "api-secret") {
    httpJson(res, 403, { error_message: "bad credentials" });
    return;
  }
  if (req.url === "/api/public/request_kasm") {
    state.request += 1;
    writeState();
    if (body.allow_kasm_audio !== true || body.kasm_audio_default_on !== true) {
      httpJson(res, 400, { error_message: "audio flags missing" });
      return;
    }
    httpJson(res, 200, { kasm_id: "kasm-smoke" });
    return;
  }
  if (req.url === "/api/public/get_kasm_status") {
    state.status += 1;
    writeState();
    httpJson(res, 200, {
      kasm: {
        kasm_id: "kasm-smoke",
        operational_status: "running",
        kasm_url: "https://kasm.example.invalid/#/go?kasm_url=https%3A%2F%2Fexample.com%2F",
      },
    });
    return;
  }
  if (req.url === "/api/public/delete_kasm") {
    state.delete += 1;
    writeState();
    httpJson(res, 200, { deleted: true });
    return;
  }
  httpJson(res, 404, { error: "not found" });
});
writeState();
server.listen(0, "127.0.0.1", () => {
  const address = server.address();
  fs.writeFileSync(infoPath, JSON.stringify({ url: `http://127.0.0.1:${address.port}` }));
});
NODE
pids+=("$!")
wait_for_file "$kasm_api_info"
kasm_base_url="$(node -e 'console.log(JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).url)' "$kasm_api_info")"

launch_request='{"schema":"elastos.browser.hosted-product.open/v1","launch_request":{"schema":"elastos.browser.engine.launch-request/v1","adapter":"kasm-workspaces-product","engine":"hosted_remote_browser","stream_id":"stream:kasm:smoke","display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi","network_mode":"runtime_net_only","direct_network":false,"wallet_injection":false,"url":"https://example.com/","viewport":{"width":1280,"height":720}},"requirements":{"display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi","backend_class":"product_compositor","audio":true,"video":true,"network_mode":"runtime_net_only","direct_network":false}}'

control_no_bridge="$tmp_dir/kasm-no-bridge.sock"
ELASTOS_BROWSER_KASM_CONTROL_CONFIG="$(node -e '
  console.log(JSON.stringify({
    schema: "elastos.browser.kasm-control.config/v1",
    control_socket_path: process.argv[1],
    replace_existing_socket: true,
    kasm_base_url: process.argv[2],
    api_key: "api-key",
    api_key_secret: "api-secret",
    user_id: "user-smoke",
    image_id: "image-smoke"
  }))
' "$control_no_bridge" "$kasm_base_url")" \
  node "$repo_root/scripts/browser-kasm-control-service.mjs" >"$tmp_dir/no-bridge.log" 2>"$tmp_dir/no-bridge.err" &
pids+=("$!")
wait_for_socket "$control_no_bridge"

set +e
curl --silent --show-error --unix-socket "$control_no_bridge" \
  -H 'content-type: application/json' \
  --data "$launch_request" \
  http://browser-engine/pages >"$tmp_dir/no-bridge-response.json"
no_bridge_status=$?
set -e
if [[ "$no_bridge_status" -ne 0 ]]; then
  echo "curl to no-bridge Kasm control service failed" >&2
  cat "$tmp_dir/no-bridge-response.json" >&2
  exit 1
fi
node -e '
  const fs = require("node:fs");
  const response = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const state = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
  if (response.error !== "kasm_product_display_bridge_required") throw new Error("Kasm URL-only path did not fail closed");
  if (state.request !== 0 || state.status !== 0 || state.delete !== 0) throw new Error("Kasm API was called before product display bridge was configured");
' "$tmp_dir/no-bridge-response.json" "$kasm_api_state"

bridge_socket="$tmp_dir/kasm-display-bridge.sock"
node - "$bridge_socket" <<'NODE' &
const fs = require("node:fs");
const http = require("node:http");
const socketPath = process.argv[2];
try { fs.unlinkSync(socketPath); } catch {}
function httpJson(res, status, body) {
  const data = Buffer.from(JSON.stringify(body));
  res.writeHead(status, {
    "content-type": "application/json",
    "content-length": data.length,
  });
  res.end(data);
}
function readBody(req) {
  return new Promise((resolve) => {
    const chunks = [];
    req.on("data", (chunk) => chunks.push(chunk));
    req.on("end", () => resolve(JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}")));
  });
}
const pageId = "page:kasm-product-smoke";
const server = http.createServer(async (req, res) => {
  if (req.method === "POST" && req.url === "/pages") {
    const body = await readBody(req);
    if (body.schema !== "elastos.browser.kasm-display-bridge.open/v1") throw new Error("wrong bridge schema");
    if (!body.kasm_session?.kasm_url) throw new Error("missing internal Kasm session URL");
    const launch = body.launch_request;
    httpJson(res, 200, {
      schema: "elastos.browser.engine.supervisor-result/v1",
      page_id: pageId,
      adapter: launch.adapter,
      engine: launch.engine,
      stream_id: launch.stream_id,
      actual_url: launch.url,
      title: "Kasm Product Smoke",
      network_mode: "runtime_net_only",
      direct_network: false,
      wallet_injection: false,
      display_session: {
        schema: "elastos.browser.display-session/v1",
        session_id: `display:${launch.stream_id}`,
        mode: "webrtc_remote_display",
        input: "datachannel",
        input_protocol: "kasm_v1",
        width: 1280,
        height: 720,
        offerer: "engine",
        initial_offer: {
          schema: "elastos.browser.webrtc-offer/v1",
          type: "offer",
          sdp: "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=Kasm Smoke\r\nt=0 0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\nc=IN IP4 0.0.0.0\r\na=mid:0\r\na=sendonly\r\na=rtcp-mux\r\na=setup:actpass\r\na=rtpmap:96 VP8/90000\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\nc=IN IP4 0.0.0.0\r\na=mid:1\r\na=sendonly\r\na=rtcp-mux\r\na=setup:actpass\r\na=rtpmap:111 opus/48000/2\r\n",
        },
        display_backend: "kasm_workspaces_webrtc",
        backend_class: "product_compositor",
        audio: true,
        video: true,
        network_mode: "runtime_net_only",
        direct_network: false,
        signaling_url: "/api/apps/browser/pages/page%3Akasm-product-smoke/webrtc",
      },
    });
    return;
  }
  if (req.method === "GET" && req.url === `/pages/${encodeURIComponent(pageId)}/status`) {
    httpJson(res, 200, {
      schema: "elastos.browser.page-status/v1",
      page_id: pageId,
      display_backend: "kasm_workspaces_webrtc",
      backend_class: "product_compositor",
      audio: true,
      video: true,
      direct_network: false,
    });
    return;
  }
  if (req.method === "POST" && req.url === `/pages/${encodeURIComponent(pageId)}/close`) {
    await readBody(req);
    httpJson(res, 200, { schema: "elastos.browser.close-result/v1", page_id: pageId, closed: true });
    return;
  }
  httpJson(res, 200, { schema: "elastos.browser.bridge-smoke/v1", page_id: pageId, accepted: true });
});
server.listen(socketPath);
NODE
pids+=("$!")
wait_for_socket "$bridge_socket"

control_with_bridge="$tmp_dir/kasm-with-bridge.sock"
ELASTOS_BROWSER_KASM_CONTROL_CONFIG="$(node -e '
  console.log(JSON.stringify({
    schema: "elastos.browser.kasm-control.config/v1",
    control_socket_path: process.argv[1],
    replace_existing_socket: true,
    kasm_base_url: process.argv[2],
    api_key: "api-key",
    api_key_secret: "api-secret",
    user_id: "user-smoke",
    image_id: "image-smoke",
    product_display_bridge_socket: process.argv[3]
  }))
' "$control_with_bridge" "$kasm_base_url" "$bridge_socket")" \
  node "$repo_root/scripts/browser-kasm-control-service.mjs" >"$tmp_dir/with-bridge.log" 2>"$tmp_dir/with-bridge.err" &
pids+=("$!")
wait_for_socket "$control_with_bridge"

scripts/browser-hosted-product-target-preflight.sh \
  --out-dir "$tmp_dir/config-kasm" \
  --supervisor-program "$repo_root/scripts/browser-hosted-product-supervisor.mjs" \
  --control-socket "$control_with_bridge" \
  --candidate kasm-workspaces >/dev/null

node -e '
  const fs = require("node:fs");
  const state = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (state.request !== 1 || state.status !== 1) throw new Error(`Kasm API lifecycle was not exercised: ${JSON.stringify(state)}`);
' "$kasm_api_state"

curl --silent --show-error --unix-socket "$control_with_bridge" \
  -H 'content-type: application/json' \
  --data '{}' \
  "http://browser-engine/pages/page%3Akasm-product-smoke/close" >/dev/null

node -e '
  const fs = require("node:fs");
  const state = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (state.delete !== 1) throw new Error(`Kasm session was not deleted on close: ${JSON.stringify(state)}`);
' "$kasm_api_state"

printf '{"schema":"elastos.browser.kasm-control-smoke/v1","ok":true,"url_only_rejected_before_api":true,"product_bridge_preflight_passed":true,"delete_called_on_close":true}\n'
