#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
node_bin="${NODE:-node}"
tmp_dir="$(mktemp -d "/tmp/elastos-vm-settlement.XXXXXX")"
proof_dir="$tmp_dir/proof"
mkdir -p "$proof_dir"
service_pid=""

cleanup() {
  if [[ -n "$service_pid" ]]; then
    kill "$service_pid" >/dev/null 2>&1 || true
    wait "$service_pid" 2>/dev/null || true
  fi
  shopt -s nullglob
  for pid_file in "$proof_dir"/*.pid; do
    child_pid="$(tr -d '[:space:]' < "$pid_file")"
    if [[ "$child_pid" =~ ^[0-9]+$ ]]; then
      kill "$child_pid" >/dev/null 2>&1 || true
    fi
  done
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

fake_launcher="$tmp_dir/fake-settlement-launcher.mjs"
cat > "$fake_launcher" <<'NODE'
#!/usr/bin/env node
import fs from "node:fs";
import http from "node:http";

const chunks = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
const launch = body.launch_request;
const proofDir = process.env.PERSISTENT_LAUNCHER_PROOF_DIR;
const suffix = launch.stream_id.replace(/[^A-Za-z0-9_-]/g, "_");
const pageId = `page:settlement-${suffix}`;
const controlSocketPath = `${proofDir}/${suffix}.sock`;
const pidPath = `${proofDir}/${suffix}.pid`;
if (process.env.LAUNCH_MARKER_PATH) {
  fs.appendFileSync(process.env.LAUNCH_MARKER_PATH, `${launch.stream_id}\n`);
}
fs.writeFileSync(pidPath, `${process.pid}\n`, { mode: 0o600 });

const sendJson = (res, status, value) => {
  const bytes = Buffer.from(JSON.stringify(value));
  res.writeHead(status, {
    "content-type": "application/json",
    "content-length": bytes.length,
  });
  res.end(bytes);
};

const guest = http.createServer((req, res) => {
  const url = new URL(req.url, "http://browser-vm-guest");
  const close = url.pathname.match(/^\/pages\/([^/]+)\/close$/);
  if (req.method === "POST" && close) {
    sendJson(res, 200, {
      schema: "elastos.browser.close-result/v1",
      page_id: decodeURIComponent(close[1]),
      closed: true,
    });
    return;
  }
  sendJson(res, 404, { error: "browser page not found" });
});

const terminate = () => {
  guest.close(() => process.exit(0));
  setTimeout(() => process.exit(0), 100).unref();
};
process.once("SIGTERM", terminate);
process.once("SIGINT", terminate);

guest.listen(controlSocketPath, () => {
  process.stdout.write(`${JSON.stringify({
    schema: "elastos.browser.engine.supervisor-result/v1",
    page_id: pageId,
    adapter: launch.adapter,
    engine: launch.engine,
    stream_id: launch.stream_id,
    actual_url: launch.url,
    title: "Browser VM Settlement Smoke",
    network_mode: "runtime_net_only",
    direct_network: false,
    wallet_injection: false,
    control_socket_path: controlSocketPath,
    isolated_session: true,
    isolation: {
      schema: "elastos.browser.engine.isolation/v1",
      kind: "per_launch_vm_target",
      session_dir: `/tmp/elastos-browser-vm-sessions/${suffix}`,
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
        sdp: "v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n",
      },
      audio_offer: {
        schema: "elastos.browser.webrtc-offer/v1",
        type: "offer",
        sdp: "v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n",
      },
      display_backend: "vm_selkies_gstreamer_webrtc",
      backend_class: "product_compositor",
      media_transport: "runtime_relay",
      audio: true,
      video: true,
      network_mode: "runtime_net_only",
      direct_network: false,
    },
  })}\n`);
});
NODE
chmod +x "$fake_launcher"

flaky_shutdown="$tmp_dir/flaky-shutdown.mjs"
cat > "$flaky_shutdown" <<'NODE'
#!/usr/bin/env node
import fs from "node:fs";

const statePath = process.env.SHUTDOWN_STATE_FILE;
if (!fs.existsSync(statePath)) {
  fs.writeFileSync(statePath, "failed\n", { mode: 0o600 });
  process.exit(17);
}
fs.appendFileSync(statePath, "succeeded\n");
NODE
chmod +x "$flaky_shutdown"

config_json() {
  local socket_path="$1"
  local shutdown_program="${2:-}"
  "$node_bin" - "$socket_path" "$fake_launcher" "$shutdown_program" <<'NODE'
const [socketPath, launcher, shutdownProgram] = process.argv.slice(2);
const config = {
  schema: "elastos.browser.vm-control-service.config/v1",
  control_socket_path: socketPath,
  launcher_program: launcher,
  launcher_args: [],
  persistent_launcher: true,
  max_active_pages: 1,
  reuse_idle_vms: false,
  idle_vm_keepalive_ms: 0,
  launch_timeout_ms: 5000,
  shutdown_timeout_ms: 1000,
};
if (shutdownProgram) config.shutdown_program = shutdownProgram;
process.stdout.write(JSON.stringify(config));
NODE
}

start_service() {
  local socket_path="$1"
  local shutdown_program="${2:-}"
  local label="$3"
  local launch_marker="${4:-}"
  local config
  config="$(config_json "$socket_path" "$shutdown_program")"
  ELASTOS_BROWSER_VM_CONTROL_SERVICE_CONFIG="$config" \
  PERSISTENT_LAUNCHER_PROOF_DIR="$proof_dir" \
  SHUTDOWN_STATE_FILE="$tmp_dir/shutdown-state" \
  LAUNCH_MARKER_PATH="$launch_marker" \
    "$node_bin" "$repo_root/scripts/browser-vm-control-service.mjs" \
      > "$tmp_dir/${label}.out" 2> "$tmp_dir/${label}.err" &
  service_pid=$!
  for _ in {1..100}; do
    [[ -S "$socket_path" ]] && return
    sleep 0.05
  done
  cat "$tmp_dir/${label}.err" >&2 || true
  echo "Browser VM settlement service did not become ready: $label" >&2
  exit 1
}

stop_service() {
  kill "$service_pid" >/dev/null 2>&1 || true
  wait "$service_pid" 2>/dev/null || true
  service_pid=""
}

client="$tmp_dir/settlement-client.mjs"
cat > "$client" <<'NODE'
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";

const socketPath = process.env.CONTROL_SOCKET;
const streamId = process.env.STREAM_ID;
const principalId = "person:local:vm-settlement-smoke";

function requestRaw(method, route, body) {
  const bytes = body ? Buffer.from(JSON.stringify(body)) : Buffer.alloc(0);
  return new Promise((resolve, reject) => {
    const req = http.request({
      socketPath,
      path: route,
      method,
      headers: {
        "content-type": "application/json",
        "content-length": bytes.length,
      },
    }, (res) => {
      const chunks = [];
      res.on("data", (chunk) => chunks.push(chunk));
      res.on("end", () => {
        resolve({
          status: res.statusCode,
          body: JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}"),
        });
      });
    });
    req.on("error", reject);
    req.end(bytes);
  });
}

async function request(method, route, body) {
  const response = await requestRaw(method, route, body);
  if (response.status < 200 || response.status >= 300) {
    throw new Error(response.body.error || response.body.message || `status ${response.status}`);
  }
  return response.body;
}

function openBody(id) {
  const profile = {
    schema: "elastos.browser.profile-descriptor/v1",
    principal_id: principalId,
    profile_key: "profile-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    disk_path: "/tmp/elastos-browser-settlement/profile.ext4",
    reset: "whole_profile",
  };
  return {
    schema: "elastos.browser.vm-engine.open/v1",
    launch_request: {
      schema: "elastos.browser.engine.launch-request/v1",
      adapter: "browser-vm-product",
      engine: "chromium_microvm",
      url: "https://settlement.invalid/",
      stream_id: id,
      lifecycle_generation: `sha256:${id}`,
      target: "tls://settlement.invalid:443",
      principal_id: principalId,
      profile,
      network_mode: "runtime_net_only",
      direct_network: false,
      wallet_injection: false,
      display_mode: "webrtc_remote_display",
      guarantee_level: "mechanism_microvm",
    },
    requirements: {
      substrate: "microvm",
      display_mode: "webrtc_remote_display",
      guarantee_level: "mechanism_microvm",
      backend_class: "product_compositor",
      network_mode: "runtime_net_only",
      direct_network: false,
    },
    profile,
  };
}

function cleanupBinding(page) {
  return {
    schema: "elastos.browser.engine-cleanup-binding/v2",
    page_id: page.page_id,
    generation: `sha256:${page.stream_id}`,
    stream_id: page.stream_id,
    adapter: page.adapter,
    engine: page.engine,
    display_mode: "webrtc_remote_display",
    guarantee_level: "mechanism_microvm",
    principal_id: principalId,
    control_socket_path: page.control_socket_path,
    shutdown_socket_path: socketPath,
    isolated_session: true,
    isolation: page.isolation,
    process: page.process,
  };
}

function closeBody(page) {
  return {
    page_id: page.page_id,
    force_retire_vm: true,
    runtime_cleanup: cleanupBinding(page),
  };
}

const reconcile = (id) =>
  request("POST", "/launches/reconcile", {
    schema: "elastos.browser.vm-control-service.reconcile-launch/v1",
    lifecycle_generation: `sha256:${id}`,
    stream_id: id,
  });

const processAlive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error?.code !== "ESRCH";
  }
};

async function waitFor(predicate, message) {
  const deadline = Date.now() + 5000;
  while (Date.now() < deadline) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new Error(message);
}

if (process.env.PHASE === "cleanup-retry") {
  const page = await request("POST", "/pages", openBody(streamId));
  const first = await requestRaw("POST", "/shutdown", closeBody(page));
  if (
    first.status !== 400 ||
    !String(first.body.error || "").includes("cleanup failed")
  ) {
    throw new Error(`first cleanup did not fail closed: ${JSON.stringify(first)}`);
  }
  const status = await request("GET", "/status");
  if (status.active_pages !== 1 || status.active_vms !== 1) {
    throw new Error(`failed cleanup released its owner: ${JSON.stringify(status)}`);
  }
  const pending = await reconcile(streamId);
  if (pending.state !== "cleanup_pending") {
    throw new Error(`failed cleanup was not pending: ${JSON.stringify(pending)}`);
  }
  if (!processAlive(page.process.pid) || !fs.existsSync(page.control_socket_path)) {
    throw new Error("failed cleanup lost its surviving exact child or socket");
  }
  const blockedRead = await requestRaw(
    "GET",
    `/pages/${encodeURIComponent(page.page_id)}/status`,
  );
  if (
    blockedRead.status !== 404 ||
    !String(blockedRead.body.error || "").includes("cleanup is pending")
  ) {
    throw new Error(`pending page remained routable: ${JSON.stringify(blockedRead)}`);
  }
  process.kill(page.process.pid, "SIGTERM");
  await waitFor(
    () => !processAlive(page.process.pid),
    "failed-cleanup fixture child did not exit",
  );
  const afterExit = await reconcile(streamId);
  if (afterExit.state !== "cleanup_pending") {
    throw new Error(`launcher exit synthesized terminal cleanup: ${JSON.stringify(afterExit)}`);
  }
  const second = await request("POST", "/shutdown", closeBody(page));
  if (
    second.terminal !== true ||
    Object.values(second.effects || {}).some((value) => value !== true)
  ) {
    throw new Error(`cleanup retry was not terminal: ${JSON.stringify(second)}`);
  }
  await waitFor(
    () => !processAlive(page.process.pid) && !fs.existsSync(page.control_socket_path),
    "cleanup retry did not reap its exact child and socket",
  );
  const terminal = await reconcile(streamId);
  if (terminal.state !== "terminal_post_effect_cleanup") {
    throw new Error(`cleanup retry did not persist terminal state: ${JSON.stringify(terminal)}`);
  }
  const alreadyAbsent = await request("POST", "/shutdown", closeBody(page));
  if (alreadyAbsent.terminal !== true || alreadyAbsent.already_absent !== true) {
    throw new Error(`terminal retry lost already-absent proof: ${JSON.stringify(alreadyAbsent)}`);
  }
} else if (process.env.PHASE === "open-for-restart") {
  const page = await request("POST", "/pages", openBody(streamId));
  fs.writeFileSync(process.env.PAGE_FILE, JSON.stringify(page), { mode: 0o600 });
} else if (process.env.PHASE === "verify-restart") {
  const page = JSON.parse(fs.readFileSync(process.env.PAGE_FILE, "utf8"));
  const pending = await reconcile(streamId);
  if (
    pending.state !== "cleanup_pending" ||
    pending.cleanup_binding?.page_id !== page.page_id
  ) {
    throw new Error(`restart did not retain exact pending ownership: ${JSON.stringify(pending)}`);
  }
  const first = await requestRaw("POST", "/shutdown", closeBody(page));
  if (
    first.status !== 400 ||
    !String(first.body.error || "").includes("indeterminate after service restart")
  ) {
    throw new Error(`surviving restart resource was synthesized terminal: ${JSON.stringify(first)}`);
  }
  if (!processAlive(page.process.pid) || !fs.existsSync(page.control_socket_path)) {
    throw new Error("restart cleanup disturbed the surviving exact resource");
  }
  if ((await reconcile(streamId)).state !== "cleanup_pending") {
    throw new Error("restart cleanup did not remain pending");
  }
  process.kill(page.process.pid, "SIGTERM");
  await waitFor(
    () => !processAlive(page.process.pid),
    "test fixture child did not exit",
  );
  if (fs.existsSync(page.control_socket_path)) {
    fs.unlinkSync(page.control_socket_path);
  }
  const terminal = await request("POST", "/shutdown", closeBody(page));
  if (terminal.terminal !== true || terminal.already_absent !== true) {
    throw new Error(`exact absence did not settle restart cleanup: ${JSON.stringify(terminal)}`);
  }
  if ((await reconcile(streamId)).state !== "terminal_post_effect_cleanup") {
    throw new Error("restart cleanup did not persist terminal state");
  }
} else if (process.env.PHASE === "capacity") {
  const before = fs.readFileSync(process.env.JOURNAL_PATH);
  const beforeHash = crypto.createHash("sha256").update(before).digest("hex");
  const response = await requestRaw("POST", "/pages", openBody(streamId));
  if (
    response.status !== 400 ||
    response.body.code !== "reconciliation_capacity_exhausted"
  ) {
    throw new Error(`unresolved capacity did not reject predispatch: ${JSON.stringify(response)}`);
  }
  const after = fs.readFileSync(process.env.JOURNAL_PATH);
  const afterHash = crypto.createHash("sha256").update(after).digest("hex");
  const journal = JSON.parse(after);
  if (
    beforeHash !== afterHash ||
    journal.records.length !== 128 ||
    journal.records.some((record) => record.state !== "cleanup_pending") ||
    journal.records.some((record) => record.launch.stream_id === streamId)
  ) {
    throw new Error("capacity rejection evicted or changed an unresolved record");
  }
  if (fs.existsSync(process.env.LAUNCH_MARKER_PATH)) {
    throw new Error("capacity rejection dispatched the launcher");
  }
} else {
  throw new Error(`unknown settlement phase: ${process.env.PHASE}`);
}
NODE

retry_socket="$tmp_dir/retry-control.sock"
start_service "$retry_socket" "$flaky_shutdown" "retry-service"
CONTROL_SOCKET="$retry_socket" \
STREAM_ID="stream:settlement-cleanup-retry" \
PHASE="cleanup-retry" \
  "$node_bin" "$client"
stop_service

restart_socket="$tmp_dir/restart-control.sock"
restart_page="$tmp_dir/restart-page.json"
start_service "$restart_socket" "" "restart-service-first"
CONTROL_SOCKET="$restart_socket" \
STREAM_ID="stream:settlement-restart" \
PAGE_FILE="$restart_page" \
PHASE="open-for-restart" \
  "$node_bin" "$client"
kill -KILL "$service_pid"
wait "$service_pid" 2>/dev/null || true
service_pid=""
rm -f "$restart_socket"
start_service "$restart_socket" "" "restart-service-second"
CONTROL_SOCKET="$restart_socket" \
STREAM_ID="stream:settlement-restart" \
PAGE_FILE="$restart_page" \
PHASE="verify-restart" \
  "$node_bin" "$client"
stop_service

capacity_socket="$tmp_dir/capacity-control.sock"
capacity_journal="${capacity_socket}.launch-reconciliations.json"
launch_marker="$tmp_dir/capacity-launcher-ran"
JOURNAL_PATH="$capacity_journal" "$node_bin" - <<'NODE'
import fs from "node:fs";

const records = Array.from({ length: 128 }, (_, index) => {
  const suffix = String(index).padStart(3, "0");
  return {
    schema: "elastos.browser.vm-control-service.launch-reconciliation/v1",
    state: "cleanup_pending",
    launch: {
      adapter: "browser-vm-product",
      engine: "chromium_microvm",
      lifecycle_generation: `generation:pending-${suffix}`,
      stream_id: `stream:pending-${suffix}`,
      principal_id: null,
      display_mode: "webrtc_remote_display",
      guarantee_level: "mechanism_microvm",
    },
    updated_at: "2026-07-27T00:00:00.000Z",
    effects: {
      page_acquired: null,
      vm_acquired: null,
    },
  };
});
fs.writeFileSync(process.env.JOURNAL_PATH, JSON.stringify({
  schema: "elastos.browser.vm-control-service.launch-reconciliations/v1",
  records,
}), { mode: 0o600 });
NODE

start_service "$capacity_socket" "" "capacity-service-first" "$launch_marker"
CONTROL_SOCKET="$capacity_socket" \
STREAM_ID="stream:capacity-overflow-129" \
JOURNAL_PATH="$capacity_journal" \
LAUNCH_MARKER_PATH="$launch_marker" \
PHASE="capacity" \
  "$node_bin" "$client"
stop_service
start_service "$capacity_socket" "" "capacity-service-second" "$launch_marker"
CONTROL_SOCKET="$capacity_socket" \
STREAM_ID="stream:capacity-overflow-130" \
JOURNAL_PATH="$capacity_journal" \
LAUNCH_MARKER_PATH="$launch_marker" \
PHASE="capacity" \
  "$node_bin" "$client"
stop_service

printf '%s\n' '{"schema":"elastos.browser.vm-control-service-settlement-smoke/v1","ok":true}'
