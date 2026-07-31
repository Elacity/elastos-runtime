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
const pageId = launch.page_id || `page:settlement-${suffix}`;
const controlSocketPath = `${proofDir}/${suffix}.sock`;
const pidPath = `${proofDir}/${suffix}.pid`;
const typedFailure = process.env.TYPED_TRANSPORT_FAILURE;
if (typedFailure && launch.transport_authority) {
  const acted = typedFailure !== "did_not_act";
  const terminal = typedFailure !== "cleanup_pending";
  process.stderr.write(`${JSON.stringify({
    schema: "elastos.browser.vz-launch-settlement/v1",
    state: typedFailure,
    message: `injected ${typedFailure}`,
    binding_hash: process.env.TYPED_TRANSPORT_SUBSTITUTE
      ? `sha256:${"f".repeat(64)}`
      : launch.transport_authority.binding_hash,
    generation: launch.transport_authority.generation,
    page_id: launch.transport_authority.page_id,
    vm_id: launch.transport_authority.vm_id,
    stream_id: launch.transport_authority.egress.stream_id,
    media_stream_id: launch.transport_authority.media.stream_id,
    effects: {
      session_directory: acted,
      control_socket: acted,
      ordinary_stream_bridge: acted,
      media_stream_bridge: acted,
      turn_process: acted,
      supervisor_child: acted,
      vm: acted,
    },
    absence: {
      child_absent: true,
      supervisor_child_absent: true,
      control_socket_absent: true,
      route_absent: true,
      turn_listener_absent: true,
      turn_relay_ports_absent: true,
      ordinary_stream_bridge_absent: true,
      media_stream_bridge_absent: true,
      session_directory_absent: true,
      vm_absent: terminal,
    },
  })}\n`);
  process.exit(24);
}
if (process.env.LAUNCH_MARKER_PATH) {
  fs.appendFileSync(process.env.LAUNCH_MARKER_PATH, `${launch.stream_id}\n`);
}
fs.writeFileSync(pidPath, `${process.pid}\n`, { mode: 0o600 });
if (process.env.FAIL_TRANSPORT_LAUNCH === "1" && launch.transport_authority) {
  process.exit(23);
}

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
  const transportReceipt = launch.transport_authority
    ? {
        schema: "elastos.browser.vz-transport-effect-receipt/v1",
        binding_hash: launch.transport_authority.binding_hash,
        generation: launch.transport_authority.generation,
        page_id: launch.transport_authority.page_id,
        vm_id: launch.transport_authority.vm_id,
        expires_at_unix_ms:
          launch.transport_authority.expires_at_unix_ms,
        terminal: true,
        effects: {
          vz_network_devices_zero: true,
          guest_bootstrap_validated: true,
          guest_loopback_only: true,
          guest_interfaces: ["lo"],
          guest_default_route_absent: true,
          guest_direct_network_absent: true,
          ordinary_stream_fixed_target: true,
          media_stream_fixed_target: true,
          turn_launch_owned: true,
          turn_listener_loopback: true,
          hibernation_disabled: true,
        },
      }
    : undefined;
  process.stdout.write(`${JSON.stringify({
    schema: "elastos.browser.engine.supervisor-result/v1",
    page_id: pageId,
    adapter: launch.adapter,
    engine: launch.engine,
    stream_id: launch.stream_id,
    ...(launch.transport_authority
      ? {
          vm_id: launch.vm_id,
          transport_authority: launch.transport_authority,
          transport_receipt: transportReceipt,
        }
      : {}),
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
      ...(launch.transport_authority
        ? { ice_connection_policy: "engine_relay_only" }
        : {}),
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
  FAIL_TRANSPORT_LAUNCH="${FAIL_TRANSPORT_LAUNCH:-}" \
  TYPED_TRANSPORT_FAILURE="${TYPED_TRANSPORT_FAILURE:-}" \
  TYPED_TRANSPORT_SUBSTITUTE="${TYPED_TRANSPORT_SUBSTITUTE:-}" \
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
import net from "node:net";

const socketPath = process.env.CONTROL_SOCKET;
const streamId = process.env.STREAM_ID;
const principalId = "person:local:vm-settlement-smoke";
const transportEnabled = process.env.TRANSPORT === "1";
let issuedTransportSecret = null;
let issuedTransportAuthority = null;

function canonicalJson(value) {
  if (Array.isArray(value)) return value.map(canonicalJson);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((key) => [key, canonicalJson(value[key])]),
    );
  }
  return value;
}

function sha256Label(value) {
  return `sha256:${crypto.createHash("sha256").update(value).digest("hex")}`;
}

function generation(id) {
  return transportEnabled
    ? sha256Label(Buffer.from(id))
    : `sha256:${id}`;
}

function transportBinding(id) {
  const expiresAtUnixMs = (Math.floor(Date.now() / 1000) + 300) * 1000;
  const authSecret = crypto.randomBytes(32).toString("base64url");
  const username = `${expiresAtUnixMs / 1000}:settlement`;
  const credential = crypto
    .createHmac("sha1", authSecret)
    .update(username)
    .digest("base64");
  const authority = {
    schema: "elastos.browser.vz-transport-authority/v1",
    generation: generation(id),
    page_id: `page:vz-${sha256Label(Buffer.from(id)).slice(7, 23)}`,
    vm_id: `vm:vz-${sha256Label(Buffer.from(`${id}:vm`)).slice(7, 23)}`,
    principal_id: principalId,
    egress: {
      schema: "elastos.browser.vz-transport-stream/v1",
      stream_id: id,
      target: "tls://settlement.invalid:443",
      runtime_socket_path: "/tmp/vz-settlement-egress.sock",
      vsock_port: 19091,
    },
    media: {
      schema: "elastos.browser.vz-transport-stream/v1",
      stream_id: `stream:vz-media-${sha256Label(Buffer.from(id)).slice(7, 23)}`,
      target: "tcp://127.0.0.1:49991",
      runtime_socket_path: "/tmp/vz-settlement-media.sock",
      vsock_port: 19094,
    },
    turn: {
      schema: "elastos.browser.vz-turn-authority/v1",
      guest_url: "turn:127.0.0.1:3478?transport=tcp",
      guest_host: "127.0.0.1",
      guest_port: 3478,
      listen_host: "127.0.0.1",
      listen_port: 49991,
      advertised_host: "127.0.0.1",
      relay_host: "127.0.0.1",
      relay_port_min: 49992,
      relay_port_max: 49995,
      protocols: ["turn", "tcp"],
      username,
      credential_hash: sha256Label(Buffer.from(credential)),
      auth_secret_hash: sha256Label(Buffer.from(authSecret)),
    },
    bootstrap_vsock_port: 19093,
    expires_at_unix_ms: expiresAtUnixMs,
  };
  authority.binding_hash = sha256Label(
    Buffer.from(JSON.stringify(canonicalJson(authority))),
  );
  issuedTransportAuthority = authority;
  issuedTransportSecret = { credential, auth_secret: authSecret };
  return {
    page_id: authority.page_id,
    vm_id: authority.vm_id,
    transport_authority: authority,
    transport_secret: {
      schema: "elastos.browser.vz-transport-secret/v1",
      binding_hash: authority.binding_hash,
      credential,
      auth_secret: authSecret,
    },
  };
}

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
  const transport = transportEnabled ? transportBinding(id) : {};
  return {
    schema: "elastos.browser.vm-engine.open/v1",
    launch_request: {
      schema: "elastos.browser.engine.launch-request/v1",
      adapter: "browser-vm-product",
      engine: "chromium_microvm",
      url: "https://settlement.invalid/",
      stream_id: id,
      lifecycle_generation: generation(id),
      target: "tls://settlement.invalid:443",
      principal_id: principalId,
      profile,
      network_mode: "runtime_net_only",
      direct_network: false,
      wallet_injection: false,
      display_mode: "webrtc_remote_display",
      guarantee_level: "mechanism_microvm",
      ...transport,
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
    generation: generation(page.stream_id),
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
    control_service: page.control_service,
    process: page.process,
    ...(page.transport_authority
      ? {
          transport_authority: page.transport_authority,
          transport_receipt: page.transport_receipt,
        }
      : {}),
  };
}

function closeBody(page) {
  return {
    page_id: page.page_id,
    force_retire_vm: true,
    runtime_cleanup: cleanupBinding(page),
  };
}

function runtimeSerializedCloseBody(page) {
  const body = closeBody(page);
  const serialized = structuredClone(body);
  serialized.runtime_cleanup.isolation = canonicalJson(
    serialized.runtime_cleanup.isolation,
  );
  serialized.runtime_cleanup.process = canonicalJson(
    serialized.runtime_cleanup.process,
  );
  if (
    JSON.stringify(body.runtime_cleanup.isolation) ===
      JSON.stringify(serialized.runtime_cleanup.isolation) ||
    JSON.stringify(body.runtime_cleanup.process) ===
      JSON.stringify(serialized.runtime_cleanup.process)
  ) {
    throw new Error("Runtime serialization fixture did not reorder both bindings");
  }
  return JSON.parse(JSON.stringify(serialized));
}

const reconcile = (id) =>
  request("POST", "/launches/reconcile", {
    schema: "elastos.browser.vm-control-service.reconcile-launch/v1",
    lifecycle_generation: generation(id),
    stream_id: id,
    ...(transportEnabled
      ? { transport_authority: issuedTransportAuthority }
      : {}),
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

async function requireBindingRejected(page, label, mutate) {
  const body = runtimeSerializedCloseBody(page);
  mutate(body.runtime_cleanup);
  const rejected = await requestRaw("POST", "/shutdown", body);
  if (rejected.status !== 400) {
    throw new Error(
      `${label} cleanup binding was accepted: ${JSON.stringify(rejected)}`,
    );
  }
  const status = await request("GET", "/status");
  if (
    status.active_pages !== 1 ||
    status.active_vms !== 1 ||
    !processAlive(page.process.pid) ||
    !fs.existsSync(page.control_socket_path)
  ) {
    throw new Error(
      `${label} cleanup binding released its exact owner: ${JSON.stringify(status)}`,
    );
  }
  const retained = await reconcile(streamId);
  if (
    retained.state !== "effect_acquired" ||
    retained.supervisor_result?.page_id !== page.page_id
  ) {
    throw new Error(
      `${label} cleanup binding lost durable effect ownership: ${JSON.stringify(retained)}`,
    );
  }
}

if (process.env.PHASE === "binding-equality") {
  const page = await request("POST", "/pages", openBody(streamId));
  await requireBindingRejected(page, "changed isolation", (binding) => {
    binding.isolation.session_dir += "-substitute";
  });
  await requireBindingRejected(page, "missing isolation", (binding) => {
    delete binding.isolation.kind;
  });
  await requireBindingRejected(page, "additional isolation", (binding) => {
    binding.isolation.unexpected = true;
  });
  await requireBindingRejected(page, "changed process", (binding) => {
    binding.process.pid += 1;
  });
  await requireBindingRejected(page, "missing process", (binding) => {
    delete binding.process.stream_bridge_pid;
  });
  await requireBindingRejected(page, "additional process", (binding) => {
    binding.process.unexpected = true;
  });
  const terminal = await request(
    "POST",
    "/shutdown",
    runtimeSerializedCloseBody(page),
  );
  if (
    terminal.terminal !== true ||
    Object.values(terminal.effects || {}).some((value) => value !== true)
  ) {
    throw new Error(
      `semantically exact serialized binding was not terminal: ${JSON.stringify(terminal)}`,
    );
  }
  const durable = await reconcile(streamId);
  if (durable.state !== "terminal_post_effect_cleanup") {
    throw new Error(
      `serialized cleanup did not persist terminal state: ${JSON.stringify(durable)}`,
    );
  }
} else if (process.env.PHASE === "cleanup-retry") {
  const page = await request("POST", "/pages", openBody(streamId));
  const first = await requestRaw(
    "POST",
    "/shutdown",
    runtimeSerializedCloseBody(page),
  );
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
    pending.cleanup_binding?.page_id !== page.page_id ||
    pending.supervisor_result !== undefined
  ) {
    throw new Error(`restart did not retain only its exact durable cleanup binding: ${JSON.stringify(pending)}`);
  }
  const first = await requestRaw(
    "POST",
    "/shutdown",
    runtimeSerializedCloseBody(page),
  );
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
  const stillIndeterminate = await requestRaw(
    "POST",
    "/shutdown",
    runtimeSerializedCloseBody(page),
  );
  if (
    stillIndeterminate.status !== 400 ||
    !String(stillIndeterminate.body.error || "").includes(
      "exact owned launcher unavailable",
    )
  ) {
    throw new Error(
      `stale process identity synthesized terminal cleanup: ${JSON.stringify(stillIndeterminate)}`,
    );
  }
  if ((await reconcile(streamId)).state !== "cleanup_pending") {
    throw new Error("stale process identity did not remain pending");
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
} else if (process.env.PHASE === "transport-cleanup") {
  const page = await request("POST", "/pages", openBody(streamId));
  if (
    page.page_id !== page.transport_authority?.page_id ||
    page.transport_receipt?.terminal !== true ||
    page.transport_receipt?.effects?.vz_network_devices_zero !== true
  ) {
    throw new Error(
      `transport launch did not echo its exact effect receipt: ${JSON.stringify(page)}`,
    );
  }
  const journalBefore = fs.readFileSync(process.env.JOURNAL_PATH, "utf8");
  if (
    journalBefore.includes(
      page.transport_authority.turn.credential_hash,
    ) === false ||
    journalBefore.includes('"auth_secret":') ||
    journalBefore.includes('"transport_secret":') ||
    journalBefore.includes(issuedTransportSecret.credential) ||
    journalBefore.includes(issuedTransportSecret.auth_secret)
  ) {
    throw new Error("transport journal secret posture is invalid");
  }

  const malformedClose = structuredClone(closeBody(page));
  malformedClose.runtime_cleanup.transport_receipt.effects.turn_launch_owned =
    false;
  const rejected = await requestRaw(
    "POST",
    "/shutdown",
    malformedClose,
  );
  if (
    rejected.status !== 400 ||
    !String(rejected.body.error || "").includes(
      "transport effect receipt",
    )
  ) {
    throw new Error(
      `malformed transport cleanup receipt did not fail closed: ${JSON.stringify(rejected)}`,
    );
  }
  const retained = await request("GET", "/status");
  if (
    retained.active_pages !== 1 ||
    !processAlive(page.process.pid) ||
    !fs.existsSync(page.control_socket_path)
  ) {
    throw new Error(
      `malformed transport receipt released cleanup ownership: ${JSON.stringify(retained)}`,
    );
  }

  const unexpectedTurnListener = net.createServer();
  await new Promise((resolve, reject) => {
    unexpectedTurnListener.once("error", reject);
    unexpectedTurnListener.listen(
      page.transport_authority.turn.listen_port,
      page.transport_authority.turn.listen_host,
      resolve,
    );
  });
  const indeterminate = await requestRaw(
    "POST",
    "/shutdown",
    closeBody(page),
  );
  if (
    indeterminate.status !== 400 ||
    !String(indeterminate.body.error || "").includes(
      "turn_listener_absent",
    )
  ) {
    throw new Error(
      `live TURN listener did not retain cleanup ownership: ${JSON.stringify(indeterminate)}`,
    );
  }
  await new Promise((resolve) =>
    unexpectedTurnListener.close(resolve),
  );

  const terminal = await request("POST", "/shutdown", closeBody(page));
  const requiredEffects = [
    "transport_session_absent",
    "turn_process_absent",
    "turn_listener_absent",
    "turn_relay_ports_absent",
    "ordinary_vsock_bridge_absent",
    "media_vsock_bridge_absent",
    "bootstrap_vsock_bridge_absent",
    "hibernation_state_absent",
  ];
  if (
    terminal.terminal !== true ||
    requiredEffects.some((key) => terminal.effects?.[key] !== true)
  ) {
    throw new Error(
      `transport cleanup was not terminal: ${JSON.stringify(terminal)}`,
    );
  }
  const durable = await reconcile(streamId);
  if (
    durable.state !== "terminal_post_effect_cleanup" ||
    durable.terminal_cleanup_receipt?.schema !==
      "elastos.browser.supervisor-cleanup-result/v2" ||
    requiredEffects.some(
      (key) =>
        durable.terminal_cleanup_receipt?.effects?.[key] !== true,
    ) ||
    durable.terminal_cleanup_receipt?.binding?.transport_authority
      ?.binding_hash !== page.transport_authority.binding_hash
  ) {
    throw new Error(
      `transport terminal cleanup was not durable: ${JSON.stringify(durable)}`,
    );
  }
  const journalAfter = fs.readFileSync(process.env.JOURNAL_PATH, "utf8");
  if (
    journalAfter.includes('"auth_secret":') ||
    journalAfter.includes('"transport_secret":')
  ) {
    throw new Error("transport terminal journal persisted a launch secret");
  }
} else if (process.env.PHASE === "transport-launch-failure") {
  const failed = await requestRaw("POST", "/pages", openBody(streamId));
  if (
    failed.status !== 400 ||
    !String(failed.body.error || "").includes(
      "persistent launcher exited before readiness",
    )
  ) {
    throw new Error(
      `injected transport launcher failure was not surfaced: ${JSON.stringify(failed)}`,
    );
  }
  const pending = await reconcile(streamId);
  if (
    pending.state !== "cleanup_pending" ||
    pending.launch.lifecycle_generation !== generation(streamId) ||
    pending.transport_authority?.binding_hash !==
      issuedTransportAuthority.binding_hash
  ) {
    throw new Error(
      `dispatched transport failure did not retain exact cleanup ownership: ${JSON.stringify(pending)}`,
    );
  }
  const journal = fs.readFileSync(process.env.JOURNAL_PATH, "utf8");
  if (
    !journal.includes(issuedTransportAuthority.binding_hash) ||
    journal.includes(issuedTransportSecret.credential) ||
    journal.includes(issuedTransportSecret.auth_secret)
  ) {
    throw new Error(
      "dispatched transport failure journal lost its binding or persisted a secret",
    );
  }
} else if (process.env.PHASE === "typed-transport-failure") {
  const expected = process.env.EXPECTED_SETTLEMENT;
  const failed = await requestRaw("POST", "/pages", openBody(streamId));
  const settlement = failed.body.launch_settlement_result;
  if (
    failed.status !== 400 ||
    settlement?.schema !==
      "elastos.browser.vz-launch-settlement/v1" ||
    settlement.state !== expected ||
    settlement.binding_hash !==
      issuedTransportAuthority.binding_hash ||
    settlement.generation !==
      issuedTransportAuthority.generation ||
    settlement.page_id !== issuedTransportAuthority.page_id ||
    settlement.vm_id !== issuedTransportAuthority.vm_id ||
    settlement.stream_id !==
      issuedTransportAuthority.egress.stream_id ||
    settlement.media_stream_id !==
      issuedTransportAuthority.media.stream_id
  ) {
    throw new Error(
      `typed transport failure was not propagated exactly: ${JSON.stringify(failed)}`,
    );
  }
  const durable = await reconcile(streamId);
  if (
    durable.state !== expected ||
    durable.launch_settlement_result?.binding_hash !==
      issuedTransportAuthority.binding_hash
  ) {
    throw new Error(
      `typed transport settlement was not durable: ${JSON.stringify(durable)}`,
    );
  }
  const journal = fs.readFileSync(process.env.JOURNAL_PATH, "utf8");
  if (
    journal.includes(issuedTransportSecret.credential) ||
    journal.includes(issuedTransportSecret.auth_secret)
  ) {
    throw new Error("typed transport settlement persisted a private secret");
  }
} else if (process.env.PHASE === "verify-typed-restart") {
  const expected = process.env.EXPECTED_SETTLEMENT;
  const persisted = JSON.parse(
    fs.readFileSync(process.env.JOURNAL_PATH, "utf8"),
  ).records.find(
    (record) =>
      record.launch?.lifecycle_generation === generation(streamId) &&
      record.launch?.stream_id === streamId,
  );
  issuedTransportAuthority =
    persisted?.launch?.transport_authority || null;
  if (!issuedTransportAuthority) {
    throw new Error("typed transport restart lost its exact authority");
  }
  const durable = await reconcile(streamId);
  if (
    durable.state !== expected ||
    durable.launch_settlement_result?.state !== expected ||
    durable.launch_settlement_result?.binding_hash !==
      issuedTransportAuthority.binding_hash ||
    durable.launch_settlement_result?.generation !==
      issuedTransportAuthority.generation
  ) {
    throw new Error(
      `typed transport settlement did not survive restart: ${JSON.stringify(durable)}`,
    );
  }
} else if (process.env.PHASE === "substituted-transport-failure") {
  const failed = await requestRaw("POST", "/pages", openBody(streamId));
  if (
    failed.status !== 400 ||
    failed.body.launch_settlement_result !== undefined
  ) {
    throw new Error(
      `substituted transport settlement was adopted: ${JSON.stringify(failed)}`,
    );
  }
  const durable = await reconcile(streamId);
  if (
    durable.state !== "cleanup_pending" ||
    durable.launch_settlement_result !== undefined
  ) {
    throw new Error(
      `substituted transport settlement escaped cleanup ownership: ${JSON.stringify(durable)}`,
    );
  }
} else {
  throw new Error(`unknown settlement phase: ${process.env.PHASE}`);
}
NODE

binding_socket="$tmp_dir/binding-control.sock"
start_service "$binding_socket" "" "binding-service"
CONTROL_SOCKET="$binding_socket" \
STREAM_ID="stream:settlement-binding-equality" \
PHASE="binding-equality" \
  "$node_bin" "$client"
stop_service

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

transport_failure_socket="$tmp_dir/transport-failure-control.sock"
transport_failure_journal="${transport_failure_socket}.launch-reconciliations.json"
FAIL_TRANSPORT_LAUNCH=1 \
  start_service "$transport_failure_socket" "" "transport-failure-service"
CONTROL_SOCKET="$transport_failure_socket" \
STREAM_ID="stream:transport-launch-failure" \
JOURNAL_PATH="$transport_failure_journal" \
PHASE="transport-launch-failure" \
TRANSPORT=1 \
  "$node_bin" "$client"
stop_service
unset FAIL_TRANSPORT_LAUNCH

for typed_settlement in did_not_act cleanup_pending terminal_post_effect_cleanup; do
  typed_socket="$tmp_dir/typed-${typed_settlement}-control.sock"
  typed_journal="${typed_socket}.launch-reconciliations.json"
  TYPED_TRANSPORT_FAILURE="$typed_settlement" \
    start_service "$typed_socket" "" "typed-${typed_settlement}-service"
  CONTROL_SOCKET="$typed_socket" \
  STREAM_ID="stream:typed-${typed_settlement}" \
  JOURNAL_PATH="$typed_journal" \
  PHASE="typed-transport-failure" \
  EXPECTED_SETTLEMENT="$typed_settlement" \
  TRANSPORT=1 \
    "$node_bin" "$client"
  stop_service
  start_service \
    "$typed_socket" \
    "" \
    "typed-${typed_settlement}-restart-service"
  CONTROL_SOCKET="$typed_socket" \
  STREAM_ID="stream:typed-${typed_settlement}" \
  JOURNAL_PATH="$typed_journal" \
  PHASE="verify-typed-restart" \
  EXPECTED_SETTLEMENT="$typed_settlement" \
  TRANSPORT=1 \
    "$node_bin" "$client"
  stop_service
done
unset TYPED_TRANSPORT_FAILURE

substituted_socket="$tmp_dir/substituted-transport-control.sock"
substituted_journal="${substituted_socket}.launch-reconciliations.json"
TYPED_TRANSPORT_FAILURE="terminal_post_effect_cleanup" \
TYPED_TRANSPORT_SUBSTITUTE=1 \
  start_service "$substituted_socket" "" "substituted-transport-service"
CONTROL_SOCKET="$substituted_socket" \
STREAM_ID="stream:substituted-transport" \
JOURNAL_PATH="$substituted_journal" \
PHASE="substituted-transport-failure" \
TRANSPORT=1 \
  "$node_bin" "$client"
stop_service
unset TYPED_TRANSPORT_FAILURE
unset TYPED_TRANSPORT_SUBSTITUTE

transport_socket="$tmp_dir/transport-control.sock"
transport_journal="${transport_socket}.launch-reconciliations.json"
start_service "$transport_socket" "" "transport-service"
CONTROL_SOCKET="$transport_socket" \
STREAM_ID="stream:transport-settlement" \
JOURNAL_PATH="$transport_journal" \
PHASE="transport-cleanup" \
TRANSPORT=1 \
  "$node_bin" "$client"
stop_service

printf '%s\n' '{"schema":"elastos.browser.vm-control-service-settlement-smoke/v1","ok":true}'
