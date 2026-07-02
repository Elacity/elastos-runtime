#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"
local_exit_pid=""

cleanup() {
  if [[ -n "${daemon_pid:-}" ]]; then
    kill "$daemon_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "$local_exit_pid" ]]; then
    kill "$local_exit_pid" >/dev/null 2>&1 || true
  fi
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

cd "$repo_root"

smoke_url="${BROWSER_SMOKE_URL:-https://example.com/}"
smoke_allowed_hosts="${BROWSER_SMOKE_ALLOWED_HOSTS:-}"
smoke_principal_id="${BROWSER_SMOKE_PRINCIPAL_ID:-person:local:browser-runtime-proxy-smoke}"
smoke_host="$(SMOKE_URL="$smoke_url" node - <<'NODE'
const value = process.env.SMOKE_URL;
const parsed = new URL(value);
console.log(parsed.hostname);
NODE
)"
if [[ -z "$smoke_allowed_hosts" ]]; then
  smoke_allowed_hosts="$smoke_host"
fi
smoke_expected_url="$(SMOKE_URL="$smoke_url" node - <<'NODE'
console.log(new URL(process.env.SMOKE_URL).toString());
NODE
)"

cargo build --quiet --manifest-path elastos/tools/browser-local-exit/Cargo.toml

local_exit_socket="$tmp_dir/local-exit.sock"
control_socket="$tmp_dir/playwright-control.sock"
profile_root="$tmp_dir/profile"

ELASTOS_BROWSER_LOCAL_EXIT_CONFIG="$(node - <<NODE
const allowedHosts = "$smoke_allowed_hosts".split(",").map((value) => value.trim()).filter(Boolean);
const config = {
  schema: "elastos.browser.local-exit.config/v1",
  relay_ipc_path: "$local_exit_socket",
  allowed_hosts: allowedHosts,
  allowed_schemes: ["tcp"],
  allowed_ports: [80, 443],
  address_family: process.env.BROWSER_SMOKE_ADDRESS_FAMILY || "prefer_ipv4",
  replace_existing_socket: true,
  allow_private_targets: false
};
if (process.env.BROWSER_SMOKE_UPSTREAM_HTTP_PROXY) {
  config.upstream_http_proxy = {
    url: process.env.BROWSER_SMOKE_UPSTREAM_HTTP_PROXY,
  };
  if (process.env.BROWSER_SMOKE_UPSTREAM_PROXY_AUTHORIZATION) {
    config.upstream_http_proxy.authorization_header = process.env.BROWSER_SMOKE_UPSTREAM_PROXY_AUTHORIZATION;
  }
}
console.log(JSON.stringify(config));
NODE
)" \
  elastos/tools/browser-local-exit/target/debug/browser-local-exit \
  >"$tmp_dir/local-exit.out" 2>"$tmp_dir/local-exit.err" &
local_exit_pid="$!"

for _ in {1..100}; do
  [[ -S "$local_exit_socket" ]] && break
  sleep 0.05
done
if [[ ! -S "$local_exit_socket" ]]; then
  echo "browser-local-exit did not create $local_exit_socket" >&2
  cat "$tmp_dir/local-exit.err" >&2 || true
  exit 1
fi

engine_config="$tmp_dir/playwright-engine.json"
node - <<NODE >"$engine_config"
const allowedHosts = "$smoke_allowed_hosts".split(",").map((value) => value.trim()).filter(Boolean);
console.log(JSON.stringify({
  schema: "elastos.browser.playwright-engine.config/v1",
  control_socket_path: "$control_socket",
  local_exit_socket_path: "$local_exit_socket",
  profile_root: "$profile_root",
  allowed_hosts: allowedHosts,
  allowed_protocols: ["http", "https"],
  allowed_ports: [80, 443],
  viewport: { width: 1024, height: 720 },
  headless: process.env.BROWSER_SMOKE_HEADLESS === "0" ? false : true,
  launch_timeout_ms: 30000,
  ice_servers: []
}));
NODE

launch_request="$(BROWSER_SMOKE_PRINCIPAL_ID="$smoke_principal_id" node - <<'NODE'
const request = {
  schema: "elastos.browser.engine.launch-request/v1",
  adapter: "playwright-smoke",
  engine: "chromium",
  stream_id: "stream:browser-runtime-proxy-smoke",
  principal_id: process.env.BROWSER_SMOKE_PRINCIPAL_ID,
  url: process.env.BROWSER_SMOKE_URL || "https://example.com/",
  display_mode: "webrtc_remote_display",
  guarantee_level: "diagnostic",
  network_mode: "runtime_net_only",
  direct_network: false,
  wallet_injection: false,
  viewport: { width: 1024, height: 720 }
};
if (process.env.BROWSER_SMOKE_REFERER) {
  request.referer = process.env.BROWSER_SMOKE_REFERER;
}
console.log(JSON.stringify(request));
NODE
)"

result_json="$(ELASTOS_BROWSER_PLAYWRIGHT_ENGINE_CONFIG="$(cat "$engine_config")" \
  ELASTOS_BROWSER_ENGINE_REQUEST="$launch_request" \
  node elastos/tools/browser-playwright-engine/src/supervisor.mjs)"

RESULT_JSON="$result_json" CONTROL_SOCKET="$control_socket" LOCAL_EXIT_ERR="$tmp_dir/local-exit.err" BROWSER_SMOKE_EXPECTED_URL="$smoke_expected_url" BROWSER_SMOKE_PRINCIPAL_ID="$smoke_principal_id" BROWSER_SMOKE_REQUIRE_MEDIA="${BROWSER_SMOKE_REQUIRE_MEDIA:-0}" node - <<'NODE'
const fs = require("node:fs");
const http = require("node:http");

const result = JSON.parse(process.env.RESULT_JSON);
const controlSocket = process.env.CONTROL_SOCKET;
const localExitErr = process.env.LOCAL_EXIT_ERR;
const expectedUrl = new URL(process.env.BROWSER_SMOKE_EXPECTED_URL).toString();
const expectedPrincipalId = process.env.BROWSER_SMOKE_PRINCIPAL_ID;
const requireMedia = process.env.BROWSER_SMOKE_REQUIRE_MEDIA === "1";

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function localExitRelayOpenLogs() {
  return fs.readFileSync(localExitErr, "utf8")
    .split(/\r?\n/)
    .filter(Boolean)
    .map((line) => {
      try {
        return JSON.parse(line);
      } catch {
        return null;
      }
    })
    .filter((entry) => entry?.schema === "elastos.browser.local-exit.relay-open/v1");
}

function control(path) {
  return new Promise((resolve, reject) => {
    const req = http.request({ socketPath: controlSocket, path, method: "GET" }, (res) => {
      const chunks = [];
      res.on("data", (chunk) => chunks.push(chunk));
      res.on("end", () => {
        const body = Buffer.concat(chunks).toString("utf8");
        if (res.statusCode < 200 || res.statusCode >= 300) {
          reject(new Error(`GET ${path} returned ${res.statusCode}: ${body}`));
          return;
        }
        try {
          resolve(JSON.parse(body));
        } catch (error) {
          reject(error);
        }
      });
    });
    req.on("error", reject);
    req.end();
  });
}

function controlPost(path, body) {
  return new Promise((resolve, reject) => {
    const payload = JSON.stringify(body);
    const req = http.request({
      socketPath: controlSocket,
      path,
      method: "POST",
      headers: {
        "content-type": "application/json",
        "content-length": Buffer.byteLength(payload),
      },
    }, (res) => {
      const chunks = [];
      res.on("data", (chunk) => chunks.push(chunk));
      res.on("end", () => {
        const text = Buffer.concat(chunks).toString("utf8");
        if (res.statusCode < 200 || res.statusCode >= 300) {
          reject(new Error(`POST ${path} returned ${res.statusCode}: ${text}`));
          return;
        }
        try {
          resolve(JSON.parse(text));
        } catch (error) {
          reject(error);
        }
      });
    });
    req.on("error", reject);
    req.end(payload);
  });
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function assertMediaPlayback(pageId) {
  const mediaPath = `/pages/${encodeURIComponent(pageId)}/media`;
  const inputPath = `/pages/${encodeURIComponent(pageId)}/input`;
  let first = null;
  let last = null;
  let lastMedia = null;
  await controlPost(inputPath, {
    schema: "elastos.browser.input-event/v1",
    event: { type: "click", x: 512, y: 360 },
  }).catch(() => {});
  for (let attempt = 0; attempt < 24; attempt += 1) {
    await sleep(1000);
    const media = await control(mediaPath);
    lastMedia = media;
    const frameText = Array.isArray(media.frame_summaries)
      ? media.frame_summaries.map((frame) => `${frame.title || ""} ${frame.text_sample || ""}`).join(" ")
      : "";
    if (/not a bot|kein bot|confirm you.?re not a bot|sign in to confirm/i.test(frameText)) {
      throw new Error(`YouTube upstream bot challenge on selected Browser Exit: ${frameText.slice(0, 320)}`);
    }
    assert(media.schema === "elastos.browser.media-status/v1", "unexpected media status schema");
    assert(media.direct_network === false, "media status reported direct network access");
    const video = Array.isArray(media.elements)
      ? media.elements.find((element) => element.tag === "video" && element.ready_state >= 2)
      : null;
    if (!video) {
      continue;
    }
    if (!first) {
      first = video;
      if (video.paused) {
        await controlPost(inputPath, {
          schema: "elastos.browser.input-event/v1",
          event: { type: "key", key: "k" },
        }).catch(() => {});
      }
    }
    last = video;
    const timeDelta = Number(last.current_time || 0) - Number(first.current_time || 0);
    const videoDelta = Number(last.video_decoded_bytes || 0) - Number(first.video_decoded_bytes || 0);
    const audioDelta = Number(last.audio_decoded_bytes || 0) - Number(first.audio_decoded_bytes || 0);
    if (timeDelta >= 2 && videoDelta > 0 && audioDelta > 0 && last.paused === false && last.muted === false) {
      return {
        ok: true,
        current_time_delta: timeDelta,
        video_decoded_delta: videoDelta,
        audio_decoded_delta: audioDelta,
        muted: last.muted,
        volume: last.volume,
      };
    }
  }
  throw new Error(`media playback did not reach stable video+audio decode: ${JSON.stringify({ first, last, lastMedia })}`);
}

(async () => {
  assert(result.schema === "elastos.browser.engine.supervisor-result/v1", "unexpected launch result schema");
  assert(new URL(result.actual_url).toString() === expectedUrl, `unexpected actual_url: ${result.actual_url}`);
  assert(result.network_mode === "runtime_net_only", "launch did not stay runtime_net_only");
  assert(result.direct_network === false, "launch reported direct network access");
  assert(result.display_session?.schema === "elastos.browser.display-session/v1", "missing display session");
  assert(result.display_session.mode === "webrtc_remote_display", "launch did not return WebRTC display");
  assert(result.display_session.input === "datachannel", "WebRTC display must advertise datachannel input");
  assert(result.display_session.display_backend === "cdp_screencast_i420", "current proof backend must be explicit");
  assert(result.display_session.backend_class === "proof_surface", "current proof must not masquerade as final compositor");
  assert(result.display_session.audio === false, "current proof must not advertise audio until real capture exists");

  const status = await control("/status");
  assert(status.schema === "elastos.browser.playwright-engine.status/v1", "unexpected daemon status schema");
  assert(status.direct_network === false, "daemon reported direct network access");
  assert(status.runtime_proxy?.mode === "http_connect", "runtime proxy mode is not http_connect");
  assert(status.runtime_proxy?.direct_network === false, "runtime proxy reported direct network access");

  const page = await control(`/pages/${encodeURIComponent(result.page_id)}/status`);
  assert(page.schema === "elastos.browser.page-status/v1", "unexpected page status schema");
  assert(page.page_id === result.page_id, "page status returned the wrong page");
  assert(page.display_backend === "cdp_screencast_i420", "page status must expose current display backend");
  assert(page.backend_class === "proof_surface", "page status must mark current backend as proof_surface");
  assert(page.proof_surface === true, "page status must mark proof_surface true for CDP screencast");
  assert(page.direct_network === false, "page reported direct network access");
  assert(new URL(page.actual_url).toString() === expectedUrl, `unexpected page status URL: ${page.actual_url}`);
  assert(typeof page.can_go_back === "boolean", "page status must expose engine back-navigation state");
  assert(typeof page.can_go_forward === "boolean", "page status must expose engine forward-navigation state");

  const relayOpenLogs = localExitRelayOpenLogs();
  assert(relayOpenLogs.some((entry) => entry.principal_id === expectedPrincipalId), "local Exit relay-open did not preserve the launch principal_id");
  assert(relayOpenLogs.every((entry) => entry.direct_network === false), "local Exit relay-open log must stay runtime_net_only");

  const reload = await controlPost(`/pages/${encodeURIComponent(result.page_id)}/input`, {
    schema: "elastos.browser.input-event/v1",
    event: { type: "browser_command", command: "reload" },
  });
  assert(reload.schema === "elastos.browser.input-result/v1", "browser command did not return input-result");
  assert(new URL(reload.actual_url).toString() === expectedUrl, `reload changed URL unexpectedly: ${reload.actual_url}`);
  assert(typeof reload.can_go_back === "boolean", "browser command result must expose engine back-navigation state");
  assert(typeof reload.can_go_forward === "boolean", "browser command result must expose engine forward-navigation state");

  const media = requireMedia ? await assertMediaPlayback(result.page_id) : null;

  console.log(JSON.stringify({
    ok: true,
    page_id: result.page_id,
    actual_url: result.actual_url,
    display_backend: page.display_backend,
    audio: result.display_session.audio,
    runtime_proxy: status.runtime_proxy.mode,
    principal_id: expectedPrincipalId,
    media
  }));

  if (Number.isInteger(status.pid) && status.pid > 1) {
    process.kill(status.pid, "SIGTERM");
  }
})().catch((error) => {
  console.error(error.stack || error.message || String(error));
  process.exit(1);
});
NODE
