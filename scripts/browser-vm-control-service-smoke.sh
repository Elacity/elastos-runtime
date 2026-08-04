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
    try {
      fs.unlinkSync(socketPath);
    } catch {}
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
import fs from "node:fs";
const chunks = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
const launch = body.launch_request;
if (launch.url.includes("fail-before-readiness.invalid")) {
  fs.writeFileSync(process.env.FAILURE_LAUNCHER_PID_FILE, `${process.pid}\n`);
  console.log("{not-json");
  setInterval(() => {}, 1000);
  await new Promise(() => {});
}
if (launch.url.includes("resources-in-use.invalid")) {
  console.error(JSON.stringify({
    schema: "elastos.browser.engine.launch-error/v1",
    code: "resources_in_use",
    message: "simulated exact Browser VM resource lease conflict",
  }));
  process.exit(73);
}
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
  process: {
    pid: process.pid,
    stream_bridge_pid: null,
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
setInterval(() => {}, 1000);
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
    "persistent_launcher": True,
    "max_active_pages": 1,
    "launch_timeout_ms": 30000,
    "shutdown_timeout_ms": 2000,
}))
PY
)"

failure_launcher_pid_file="$tmp_dir/failure-launcher.pid"
FAKE_GUEST_CONTROL_SOCKET="$guest_control_socket" \
FAILURE_LAUNCHER_PID_FILE="$failure_launcher_pid_file" \
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

live_launcher_pid_file="$tmp_dir/live-launcher.pid"
CONTROL_SOCKET="$control_socket" LIVE_LAUNCHER_PID_FILE="$live_launcher_pid_file" FAILURE_LAUNCHER_PID_FILE="$failure_launcher_pid_file" "$node_bin" - <<'NODE'
const fs = require("node:fs");
const http = require("node:http");
const socketPath = process.env.CONTROL_SOCKET;

function requestRaw(method, path, body) {
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
        resolve({ statusCode: res.statusCode, body: parsed });
      });
    });
    req.on("error", reject);
    req.end(bytes);
  });
}

async function request(method, path, body) {
  const response = await requestRaw(method, path, body);
  if (response.statusCode < 200 || response.statusCode >= 300) {
    throw new Error(response.body.error || `status ${response.statusCode}`);
  }
  return response.body;
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
      lifecycle_generation: `sha256:${streamId}`,
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
  const reconcile = (streamId) =>
    request("POST", "/launches/reconcile", {
      schema: "elastos.browser.vm-control-service.reconcile-launch/v1",
      lifecycle_generation: `sha256:${streamId}`,
      stream_id: streamId,
    });
  const status = await request("GET", "/status");
  if (status.schema !== "elastos.browser.vm-control-service.status/v1") throw new Error("wrong status schema");
  if (!Number.isInteger(status.pid) || status.pid <= 1) throw new Error("status must expose pid");
  if (!Date.parse(status.started_at || "")) throw new Error("status must expose started_at");
  if (!Number.isFinite(Number(status.uptime_ms)) || Number(status.uptime_ms) < 0) throw new Error("status must expose uptime_ms");
  if (status.hibernation_mode !== "off") throw new Error(`status must report hibernation mode: ${JSON.stringify(status)}`);
  const failed = await requestRaw(
    "POST",
    "/pages",
    openRequest(
      "https://fail-before-readiness.invalid/",
      "stream:pre-readiness-failure",
    ),
  );
  if (
    failed.statusCode !== 400 ||
    !String(failed.body.error || "").includes("output is not JSON")
  ) {
    throw new Error(`pre-readiness failure was not terminal: ${JSON.stringify(failed)}`);
  }
  const failedPid = Number(fs.readFileSync(process.env.FAILURE_LAUNCHER_PID_FILE, "utf8").trim());
  try {
    process.kill(failedPid, 0);
    throw new Error(`pre-readiness launcher ${failedPid} remained alive`);
  } catch (error) {
    if (error?.code !== "ESRCH") throw error;
  }
  const statusAfterFailedReadiness = await request("GET", "/status");
  if (
    statusAfterFailedReadiness.active_pages !== 0 ||
    statusAfterFailedReadiness.active_vms !== 0 ||
    statusAfterFailedReadiness.pending_launches !== 0
  ) {
    throw new Error(`pre-readiness failure left lifecycle effects: ${JSON.stringify(statusAfterFailedReadiness)}`);
  }
  const failedReconciliation = await reconcile("stream:pre-readiness-failure");
  if (
    failedReconciliation.state !== "terminal_post_effect_cleanup" ||
    failedReconciliation.effects?.page_acquired !== true ||
    failedReconciliation.effects?.vm_acquired !== true
  ) {
    throw new Error(`reaped pre-readiness failure was not terminal: ${JSON.stringify(failedReconciliation)}`);
  }
  const resourcesInUse = await requestRaw(
    "POST",
    "/pages",
    openRequest(
      "https://resources-in-use.invalid/",
      "stream:resources-in-use",
    ),
  );
  if (
    resourcesInUse.statusCode !== 409 ||
    resourcesInUse.body.code !== "resources_in_use"
  ) {
    throw new Error(`typed resources_in_use failure changed: ${JSON.stringify(resourcesInUse)}`);
  }
  const resourcesReconciliation = await reconcile("stream:resources-in-use");
  if (
    resourcesReconciliation.state !== "terminal_post_effect_cleanup" ||
    resourcesReconciliation.effects?.page_acquired !== true ||
    resourcesReconciliation.effects?.vm_acquired !== true
  ) {
    throw new Error(`reaped resources_in_use launch remained stranded: ${JSON.stringify(resourcesReconciliation)}`);
  }
  const unknownReconciliation = await reconcile("stream:unknown-launch");
  if (unknownReconciliation.state !== "indeterminate") {
    throw new Error(`unknown launch synthesized an effect claim: ${JSON.stringify(unknownReconciliation)}`);
  }
  const launch = await request("POST", "/pages", openRequest("https://example.com/"));
  if (launch.schema !== "elastos.browser.engine.supervisor-result/v1") throw new Error("wrong launch schema");
  if (launch.engine !== "chromium_microvm") throw new Error("wrong engine");
  if (
    launch.control_service?.schema !==
      "elastos.browser.vm-control-service.identity/v1" ||
    !/^service:[0-9a-f]{64}$/.test(
      launch.control_service?.service_id || "",
    ) ||
    launch.control_service?.control_socket_path !== socketPath
  ) {
    throw new Error(`launch lacks exact selected control-service identity: ${JSON.stringify(launch)}`);
  }
  if (
    launch.process?.schema !== "elastos.browser.host-process-binding/v1" ||
    !/^process:[0-9a-f]{64}$/.test(launch.process?.ownership_id || "") ||
    !Number.isInteger(launch.process?.pid) ||
    launch.process.pid <= 1
  ) {
    throw new Error(`launch lacks exact owned host-process binding: ${JSON.stringify(launch)}`);
  }
  if (launch.display_session.media_transport !== "runtime_relay") throw new Error("missing runtime relay media transport");
  if (launch.display_session.audio !== true || launch.display_session.video !== true) throw new Error("split audio/video offers did not normalize to audio+video");
  const activeReconciliation = await reconcile(launch.stream_id);
  if (
    activeReconciliation.state !== "effect_acquired" ||
    activeReconciliation.supervisor_result?.page_id !== launch.page_id
  ) {
    throw new Error(`active launch did not reconcile to its exact effect: ${JSON.stringify(activeReconciliation)}`);
  }
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
  const replay = await requestRaw("POST", "/pages", openRequest("https://example.org/"));
  if (replay.statusCode !== 400 || !String(replay.body.error || "").includes("identity already exist")) {
    throw new Error(`same lifecycle identity was accepted twice: ${JSON.stringify(replay)}`);
  }
  const conflict = await requestRaw("POST", "/pages", openRequest("https://example.net/", "stream:other-vm-control-smoke"));
  if (conflict.statusCode !== 400 || !String(conflict.body.error || "").includes("active page capacity reached")) {
    throw new Error(`conflicting stream did not fail closed at capacity: ${JSON.stringify(conflict)}`);
  }
  const statusAfterConflict = await request("GET", "/status");
  if (
    statusAfterConflict.active_pages !== 1 ||
    statusAfterConflict.active_stream_ids?.[0] !== launch.stream_id ||
    statusAfterConflict.page_ids?.[0] !== launch.page_id
  ) {
    throw new Error(`capacity conflict changed the healthy owner: ${JSON.stringify(statusAfterConflict)}`);
  }
  const conflictReconciliation = await reconcile("stream:other-vm-control-smoke");
  if (
    conflictReconciliation.state !== "did_not_act" ||
    conflictReconciliation.effects?.page_acquired !== false ||
    conflictReconciliation.effects?.vm_acquired !== false
  ) {
    throw new Error(`capacity rejection did not preserve a DidNotAct proof: ${JSON.stringify(conflictReconciliation)}`);
  }
  const cleanupBinding = (page, generation = `sha256:${page.stream_id}`) => ({
    schema: "elastos.browser.engine-cleanup-binding/v2",
    page_id: page.page_id,
    generation,
    stream_id: page.stream_id,
    adapter: page.adapter,
    engine: page.engine,
    display_mode: "webrtc_remote_display",
    guarantee_level: "mechanism_microvm",
    principal_id: "person:local:vm-control-smoke",
    control_socket_path: page.control_socket_path,
    shutdown_socket_path: socketPath,
    isolated_session: true,
    isolation: page.isolation,
    control_service: page.control_service,
    process: page.process,
  });
  const cleanup = cleanupBinding(launch);
  const shutdown = await request("POST", "/shutdown", {
    page_id: launch.page_id,
    runtime_cleanup: cleanup,
    force_retire_vm: true,
  });
  if (
    shutdown.schema !== "elastos.browser.supervisor-cleanup-result/v2" ||
    shutdown.terminal !== true ||
    Object.values(shutdown.effects || {}).some((value) => value !== true)
  ) {
    throw new Error(`shutdown did not return exact terminal proof: ${JSON.stringify(shutdown)}`);
  }
  const terminalReconciliation = await reconcile(launch.stream_id);
  if (
    terminalReconciliation.state !== "terminal_post_effect_cleanup" ||
    terminalReconciliation.effects?.page_acquired !== true ||
    terminalReconciliation.effects?.vm_acquired !== true
  ) {
    throw new Error(`terminal cleanup did not preserve reconciliation proof: ${JSON.stringify(terminalReconciliation)}`);
  }

  const liveChildBinding = {
    ...cleanup,
    page_id: "page:missing-map-live-child",
    generation: "sha256:missing-map-live-child",
    control_socket_path: `${cleanup.control_socket_path}.missing`,
    process: {
      ...cleanup.process,
      ownership_id: `process:${"f".repeat(64)}`,
      pid: process.pid,
    },
  };
  const indeterminate = await requestRaw("POST", "/shutdown", {
    page_id: liveChildBinding.page_id,
    runtime_cleanup: liveChildBinding,
    force_retire_vm: true,
  });
  if (
    indeterminate.statusCode !== 400 ||
    !String(indeterminate.body.error || "").includes("exact durable VM effect")
  ) {
    throw new Error(`unknown cleanup binding was not rejected: ${JSON.stringify(indeterminate)}`);
  }

  const alreadyAbsent = await request("POST", "/shutdown", {
    page_id: launch.page_id,
    runtime_cleanup: cleanup,
    force_retire_vm: true,
  });
  if (
    alreadyAbsent.terminal !== true ||
    alreadyAbsent.already_absent !== true
  ) {
    throw new Error(`typed already-absent proof failed: ${JSON.stringify(alreadyAbsent)}`);
  }
  const next = await request("POST", "/pages", openRequest("https://example.net/", "stream:next-vm-control-smoke"));
  if (next.actual_url !== "https://example.net/" || next.stream_id !== "stream:next-vm-control-smoke") {
    throw new Error(`explicit close did not permit the next lifecycle: ${JSON.stringify(next)}`);
  }
  if (!Number.isInteger(next.process?.pid) || next.process.pid <= 1) {
    throw new Error(`persistent launcher did not report its owned pid: ${JSON.stringify(next)}`);
  }
  fs.writeFileSync(process.env.LIVE_LAUNCHER_PID_FILE, `${next.process.pid}\n`);
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
NODE

if [[ ! -s "$live_launcher_pid_file" ]]; then
  echo "control-service smoke did not record a live launcher child" >&2
  exit 1
fi
live_launcher_pid="$(tr -d '[:space:]' < "$live_launcher_pid_file")"
kill -TERM "$service_pid"
for _ in {1..100}; do
  if ! kill -0 "$service_pid" >/dev/null 2>&1; then
    break
  fi
  sleep 0.05
done
if kill -0 "$service_pid" >/dev/null 2>&1; then
  echo "control service did not exit after SIGTERM" >&2
  exit 1
fi
wait "$service_pid"
service_pid=""
if [[ -S "$control_socket" ]]; then
  echo "control service removed neither its socket nor its lifecycle effects" >&2
  exit 1
fi
if kill -0 "$live_launcher_pid" >/dev/null 2>&1; then
  echo "control service exited before its owned launcher child was reaped" >&2
  exit 1
fi

restart_launcher_pid_file="$tmp_dir/restart-launcher.pid"
FAKE_GUEST_CONTROL_SOCKET="$guest_control_socket" \
FAILURE_LAUNCHER_PID_FILE="$failure_launcher_pid_file" \
ELASTOS_BROWSER_VM_CONTROL_SERVICE_CONFIG="$config_json" \
  "$node_bin" "$repo_root/scripts/browser-vm-control-service.mjs" > "$tmp_dir/service-restart.out" 2> "$tmp_dir/service-restart.err" &
service_pid="$!"

for _ in {1..100}; do
  [[ -S "$control_socket" ]] && break
  sleep 0.05
done
if [[ ! -S "$control_socket" ]]; then
  cat "$tmp_dir/service-restart.err" >&2 || true
  exit 1
fi

CONTROL_SOCKET="$control_socket" \
RECONCILIATION_JOURNAL="${control_socket}.launch-reconciliations.json" \
RESTART_LAUNCHER_PID_FILE="$restart_launcher_pid_file" \
  "$node_bin" - <<'NODE'
const fs = require("node:fs");
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
        if (res.statusCode < 200 || res.statusCode >= 300) {
          reject(new Error(parsed.error || parsed.message || `status ${res.statusCode}`));
        } else {
          resolve(parsed);
        }
      });
    });
    req.on("error", reject);
    req.end(bytes);
  });
}

const reconcile = (streamId) =>
  request("POST", "/launches/reconcile", {
    schema: "elastos.browser.vm-control-service.reconcile-launch/v1",
    lifecycle_generation: `sha256:${streamId}`,
    stream_id: streamId,
  });

const openRequest = (streamId) => ({
  schema: "elastos.browser.vm-engine.open/v1",
  launch_request: {
    schema: "elastos.browser.engine.launch-request/v1",
    adapter: "browser-vm-product",
    engine: "chromium_microvm",
    url: "https://replacement-after-terminal.invalid/",
    stream_id: streamId,
    lifecycle_generation: `sha256:${streamId}`,
    target: "tls://replacement-after-terminal.invalid:443",
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

(async () => {
  const journalStat = fs.statSync(process.env.RECONCILIATION_JOURNAL);
  if ((journalStat.mode & 0o077) !== 0) {
    throw new Error(`reconciliation journal is not owner-only: ${(journalStat.mode & 0o777).toString(8)}`);
  }
  for (const streamId of [
    "stream:pre-readiness-failure",
    "stream:resources-in-use",
    "stream:vm-control-smoke",
  ]) {
    const proof = await reconcile(streamId);
    if (proof.state !== "terminal_post_effect_cleanup") {
      throw new Error(`terminal proof did not survive restart for ${streamId}: ${JSON.stringify(proof)}`);
    }
  }
  const interrupted = await reconcile("stream:next-vm-control-smoke");
  if (
    interrupted.state !== "cleanup_pending" ||
    interrupted.cleanup_binding?.stream_id !== "stream:next-vm-control-smoke"
  ) {
    throw new Error(`restart synthesized terminal cleanup for an interrupted effect: ${JSON.stringify(interrupted)}`);
  }
  const didNotAct = await reconcile("stream:other-vm-control-smoke");
  if (
    didNotAct.state !== "did_not_act" ||
    didNotAct.effects?.page_acquired !== false ||
    didNotAct.effects?.vm_acquired !== false
  ) {
    throw new Error(`DidNotAct proof did not survive restart: ${JSON.stringify(didNotAct)}`);
  }
  const replacement = await request(
    "POST",
    "/pages",
    openRequest("stream:restart-replacement"),
  );
  if (
    replacement.stream_id !== "stream:restart-replacement" ||
    !Number.isInteger(replacement.process?.pid)
  ) {
    throw new Error(`independent replacement launch failed: ${JSON.stringify(replacement)}`);
  }
  fs.writeFileSync(process.env.RESTART_LAUNCHER_PID_FILE, `${replacement.process.pid}\n`);
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
NODE

restart_launcher_pid="$(tr -d '[:space:]' < "$restart_launcher_pid_file")"
kill -TERM "$service_pid"
for _ in {1..100}; do
  if ! kill -0 "$service_pid" >/dev/null 2>&1; then
    break
  fi
  sleep 0.05
done
wait "$service_pid"
service_pid=""
if kill -0 "$restart_launcher_pid" >/dev/null 2>&1; then
  echo "restarted control service exited before reaping its replacement launcher" >&2
  exit 1
fi

printf '%s\n' '{"schema":"elastos.browser.vm-control-service-smoke/v1","ok":true}'
