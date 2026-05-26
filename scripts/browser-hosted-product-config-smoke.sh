#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"
service_pid=""

cleanup() {
  if [[ -n "$service_pid" ]]; then
    kill "$service_pid" >/dev/null 2>&1 || true
  fi
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

cd "$repo_root"

control_socket="$tmp_dir/control.sock"
node - "$control_socket" <<'NODE' &
const fs = require("node:fs");
const http = require("node:http");
const socketPath = process.argv[2];
try { fs.unlinkSync(socketPath); } catch {}
const state = {
  pageId: "page:hosted-product-smoke",
  currentUrl: "",
  title: "Example",
  history: [],
  index: -1,
};
function pushHistory(url) {
  state.currentUrl = url;
  state.history = state.history.slice(0, state.index + 1);
  state.history.push(url);
  state.index = state.history.length - 1;
}
function browserState() {
  return {
    schema: "elastos.browser.page-status/v1",
    page_id: state.pageId,
    display_backend: "kasmvnc_webrtc",
    backend_class: "product_compositor",
    input_protocol: "selkies_v1",
    audio: true,
    video: true,
    direct_network: false,
    actual_url: state.currentUrl,
    title: state.title,
    can_go_back: state.index > 0,
    can_go_forward: state.index >= 0 && state.index < state.history.length - 1,
  };
}
function httpJson(res, status, body) {
  const data = Buffer.from(JSON.stringify(body));
  res.writeHead(status, {
    "content-type": "application/json",
    "content-length": data.length,
  });
  res.end(data);
}
const server = http.createServer((req, res) => {
  const url = new URL(req.url, "http://browser-engine");
  const pageMatch = url.pathname.match(/^\/pages\/([^/]+)\/(status|close|input)$/);
  const matchedPageId = pageMatch ? decodeURIComponent(pageMatch[1]) : "";
  const matchedOp = pageMatch ? pageMatch[2] : "";
  if (matchedPageId && matchedPageId !== state.pageId) {
    httpJson(res, 404, { error: "browser page not found" });
    return;
  }
  if (req.method === "GET" && matchedOp === "status") {
    httpJson(res, 200, browserState());
    return;
  }
  if (req.method === "POST" && matchedOp === "close") {
    httpJson(res, 200, { schema: "elastos.browser.close-result/v1", page_id: state.pageId, closed: true });
    return;
  }
  if (req.method === "POST" && matchedOp === "input") {
    const chunks = [];
    req.on("data", (chunk) => chunks.push(chunk));
    req.on("end", () => {
      const body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
      const command = body.event?.command;
      if (body.event?.type !== "browser_command") {
        httpJson(res, 200, { schema: "elastos.browser.input-result/v1", page_id: state.pageId, accepted: false });
        return;
      }
      if (command === "navigate") {
        pushHistory(String(body.event.url || ""));
      } else if (command === "back" && state.index > 0) {
        state.index -= 1;
        state.currentUrl = state.history[state.index];
      } else if (command === "forward" && state.index < state.history.length - 1) {
        state.index += 1;
        state.currentUrl = state.history[state.index];
      } else if (command !== "reload") {
        throw new Error(`unsupported command in smoke: ${command}`);
      }
      httpJson(res, 200, {
        schema: "elastos.browser.input-result/v1",
        page_id: state.pageId,
        accepted: true,
        actual_url: state.currentUrl,
        title: state.title,
        can_go_back: state.index > 0,
        can_go_forward: state.index >= 0 && state.index < state.history.length - 1,
        direct_network: false,
      });
    });
    return;
  }
  if (req.method !== "POST" || url.pathname !== "/pages") {
    httpJson(res, 404, { error: "not found" });
    return;
  }
  const chunks = [];
  req.on("data", (chunk) => chunks.push(chunk));
  req.on("end", () => {
    const body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    const launch = body.launch_request;
    state.history = [];
    state.index = -1;
    pushHistory(launch.url);
    if (body.schema !== "elastos.browser.hosted-product.open/v1") {
      throw new Error("wrong hosted product open schema");
    }
    httpJson(res, 200, {
      schema: "elastos.browser.engine.supervisor-result/v1",
      page_id: state.pageId,
      adapter: launch.adapter,
      engine: launch.engine,
      stream_id: launch.stream_id,
      actual_url: launch.url,
      title: "Example",
      network_mode: "runtime_net_only",
      direct_network: false,
      wallet_injection: false,
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
          sdp: "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=ElastOS Browser\r\nt=0 0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\nc=IN IP4 0.0.0.0\r\na=mid:0\r\na=sendonly\r\na=rtcp-mux\r\na=setup:actpass\r\na=rtpmap:96 VP8/90000\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\nc=IN IP4 0.0.0.0\r\na=mid:1\r\na=sendonly\r\na=rtcp-mux\r\na=setup:actpass\r\na=rtpmap:111 opus/48000/2\r\n",
        },
        display_backend: launch.engine === "hosted_remote_browser"
          ? "kasmvnc_webrtc"
          : "selkies_gstreamer_webrtc",
        backend_class: "product_compositor",
        audio: true,
        video: true,
        network_mode: "runtime_net_only",
        direct_network: false,
        signaling_url: "/api/apps/browser/pages/page%3Ahosted-product-smoke/webrtc",
      },
    });
  });
});
server.listen(socketPath);
NODE
service_pid="$!"
for _ in {1..100}; do
  [[ -S "$control_socket" ]] && break
  sleep 0.02
done

scripts/browser-hosted-product-target-preflight.sh \
  --out-dir "$tmp_dir/config" \
  --supervisor-program "$repo_root/scripts/browser-hosted-product-supervisor.mjs" \
  --control-socket "$control_socket" >/dev/null

node -e 'const fs=require("fs"); const c=JSON.parse(fs.readFileSync(process.argv[1],"utf8")); if(c.adapters?.[0]?.kind!=="selkies_gstreamer") throw new Error("wrong adapter kind"); if(!c.adapters[0].display_modes.includes("webrtc_remote_display")) throw new Error("missing display mode"); if(!c.adapters[0].supervisor.control_socket_path?.startsWith("/")) throw new Error("missing control socket");' \
  "$tmp_dir/config/browser-engine-adapter.json"

scripts/browser-hosted-product-target-preflight.sh \
  --out-dir "$tmp_dir/config-generic" \
  --supervisor-program "$repo_root/scripts/browser-hosted-product-supervisor.mjs" \
  --control-socket "$control_socket" \
  --engine-kind hosted_remote_browser \
  --display-backend kasmvnc_webrtc >/dev/null

scripts/browser-hosted-product-target-preflight.sh \
  --out-dir "$tmp_dir/config-kasmvnc-candidate" \
  --supervisor-program "$repo_root/scripts/browser-hosted-product-supervisor.mjs" \
  --control-socket "$control_socket" \
  --candidate kasmvnc >/dev/null

scripts/browser-hosted-product-navigation-smoke.sh \
  --adapter-config "$tmp_dir/config-generic/browser-engine-adapter.json" >/dev/null

node -e 'const fs=require("fs"); const c=JSON.parse(fs.readFileSync(process.argv[1],"utf8")); const a=c.adapters?.[0]; if(a?.kind!=="hosted_remote_browser") throw new Error("wrong generic adapter kind"); if(a.supervisor?.env?.ELASTOS_BROWSER_PRODUCT_DISPLAY_BACKEND!=="kasmvnc_webrtc") throw new Error("wrong generic display backend"); if(!a.display_modes.includes("webrtc_remote_display")) throw new Error("missing generic display mode");' \
  "$tmp_dir/config-generic/browser-engine-adapter.json"

node -e 'const fs=require("fs"); const c=JSON.parse(fs.readFileSync(process.argv[1],"utf8")); const a=c.adapters?.[0]; if(a?.id!=="kasmvnc-product") throw new Error("wrong KasmVNC candidate adapter id"); if(a?.kind!=="hosted_remote_browser") throw new Error("wrong KasmVNC candidate adapter kind"); if(a.supervisor?.env?.ELASTOS_BROWSER_PRODUCT_DISPLAY_BACKEND!=="kasmvnc_webrtc") throw new Error("wrong KasmVNC candidate display backend");' \
  "$tmp_dir/config-kasmvnc-candidate/browser-engine-adapter.json"

node scripts/browser-hosted-product-operator-config.mjs \
  --out-dir "$tmp_dir/config-browserbox" \
  --candidate browserbox \
  --supervisor-program "$repo_root/scripts/browser-hosted-product-supervisor.mjs" \
  --control-socket "$control_socket" >/dev/null

node -e 'const fs=require("fs"); const c=JSON.parse(fs.readFileSync(process.argv[1],"utf8")); const a=c.adapters?.[0]; if(a?.id!=="browserbox-product") throw new Error("wrong BrowserBox adapter id"); if(a?.kind!=="hosted_remote_browser") throw new Error("wrong BrowserBox adapter kind"); if(a.supervisor?.env?.ELASTOS_BROWSER_PRODUCT_DISPLAY_BACKEND!=="browserbox_webrtc") throw new Error("wrong BrowserBox display backend");' \
  "$tmp_dir/config-browserbox/browser-engine-adapter.json"

node scripts/browser-hosted-product-operator-config.mjs \
  --out-dir "$tmp_dir/config-kasm" \
  --candidate kasm-workspaces \
  --supervisor-program "$repo_root/scripts/browser-hosted-product-supervisor.mjs" \
  --control-socket "$control_socket" >/dev/null

node -e 'const fs=require("fs"); const c=JSON.parse(fs.readFileSync(process.argv[1],"utf8")); const a=c.adapters?.[0]; if(a?.id!=="kasm-workspaces-product") throw new Error("wrong Kasm adapter id"); if(a?.kind!=="hosted_remote_browser") throw new Error("wrong Kasm adapter kind"); if(a.supervisor?.env?.ELASTOS_BROWSER_PRODUCT_DISPLAY_BACKEND!=="kasm_workspaces_webrtc") throw new Error("wrong Kasm display backend");' \
  "$tmp_dir/config-kasm/browser-engine-adapter.json"

kill "$service_pid" >/dev/null 2>&1 || true
wait "$service_pid" >/dev/null 2>&1 || true
service_pid=""

bad_control_socket="$tmp_dir/kasm-url-only.sock"
node - "$bad_control_socket" <<'NODE' &
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
const server = http.createServer((req, res) => {
  const chunks = [];
  req.on("data", (chunk) => chunks.push(chunk));
  req.on("end", () => {
    if (req.method !== "POST" || req.url !== "/pages") {
      httpJson(res, 404, { error: "not found" });
      return;
    }
    const body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    const launch = body.launch_request;
    httpJson(res, 200, {
      schema: "elastos.browser.engine.supervisor-result/v1",
      page_id: "page:kasm-url-only",
      adapter: launch.adapter,
      engine: launch.engine,
      stream_id: launch.stream_id,
      actual_url: launch.url,
      title: "Kasm URL only",
      network_mode: "runtime_net_only",
      direct_network: false,
      wallet_injection: false,
      kasm_url: "https://kasm.example.invalid/#/go?kasm_url=https%3A%2F%2Fexample.com%2F"
    });
  });
});
server.listen(socketPath);
NODE
service_pid="$!"
for _ in {1..100}; do
  [[ -S "$bad_control_socket" ]] && break
  sleep 0.02
done

set +e
scripts/browser-hosted-product-target-preflight.sh \
  --out-dir "$tmp_dir/config-kasm-url-only" \
  --supervisor-program "$repo_root/scripts/browser-hosted-product-supervisor.mjs" \
  --control-socket "$bad_control_socket" \
  --candidate kasm-workspaces \
  >"$tmp_dir/kasm-url-only.out" \
  2>"$tmp_dir/kasm-url-only.err"
bad_status=$?
set -e
if [[ "$bad_status" -eq 0 ]]; then
  echo "hosted product preflight accepted a Kasm URL-only control service" >&2
  cat "$tmp_dir/kasm-url-only.out" >&2
  exit 1
fi
if ! grep -Eq "display[_ ]session|webrtc_remote_display|product_compositor" "$tmp_dir/kasm-url-only.err" "$tmp_dir/kasm-url-only.out"; then
  echo "Kasm URL-only rejection did not explain the missing product display session" >&2
  cat "$tmp_dir/kasm-url-only.out" >&2
  cat "$tmp_dir/kasm-url-only.err" >&2
  exit 1
fi

printf '%s\n' '{"schema":"elastos.browser.hosted-product-config-smoke/v1","ok":true,"selkies_config":true,"hosted_remote_browser_candidates":true,"navigation_contract":true,"kasm_url_only_rejected":true}'
