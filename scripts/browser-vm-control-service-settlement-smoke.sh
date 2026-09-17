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
  if [[ -n "${OWNER_PID_FILE:-}" && -f "${OWNER_PID_FILE:-}" ]]; then
    owner_pid="$(tr -d '[:space:]' < "$OWNER_PID_FILE")"
    if [[ "$owner_pid" =~ ^[0-9]+$ ]]; then
      kill "$owner_pid" >/dev/null 2>&1 || true
    fi
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
import { spawn } from "node:child_process";
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
if (process.env.POLLUTE_STDOUT === "1") {
  process.stdout.write("Allocated inode: 728\n");
}
if (process.env.HOLD_AFTER_POLLUTE === "1" && launch.transport_authority) {
  const writeHoldSettlement = () => {
    process.stderr.write(`${JSON.stringify({
      schema: "elastos.browser.vz-launch-settlement/v1",
      state: "terminal_post_effect_cleanup",
      message: "polluted stdout hold reaped",
      binding_hash: launch.transport_authority.binding_hash,
      generation: launch.transport_authority.generation,
      page_id: launch.transport_authority.page_id,
      vm_id: launch.transport_authority.vm_id,
      stream_id: launch.transport_authority.egress.stream_id,
      media_stream_id: launch.transport_authority.media.stream_id,
      effects: {
        session_directory: true,
        control_socket: true,
        ordinary_stream_bridge: true,
        media_stream_bridge: true,
        turn_process: true,
        supervisor_child: true,
        vm: true,
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
        vm_absent: true,
      },
    })}\n`);
    process.exit(0);
  };
  process.once("SIGTERM", writeHoldSettlement);
  setInterval(() => {}, 60_000);
} else {
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
if (process.env.LOG_STARTED_OWNER_HOLD === "1" && launch.transport_authority) {
  process.stdout.write("Allocated inode: 728\n");
  process.stderr.write(`${JSON.stringify({
    schema: "elastos.browser.media-diagnostic/v1",
    event: "turn_process_started",
    binding_hash: launch.transport_authority.binding_hash,
    generation: launch.transport_authority.generation,
    page_id: launch.transport_authority.page_id,
    vm_id: launch.transport_authority.vm_id,
    media_stream_id: launch.transport_authority.media.stream_id,
    ordinal: 0,
  })}\n`);
  const holder = spawn(process.execPath, ["-e", "setInterval(() => {}, 60000)"], {
    detached: true,
    stdio: "ignore",
  });
  holder.unref();
  if (process.env.OWNER_PID_FILE) {
    fs.writeFileSync(process.env.OWNER_PID_FILE, `${holder.pid}\n`, { mode: 0o600 });
  }
  process.exit(23);
}
if (process.env.MEDIA_DIAGNOSTIC_SMOKE === "1" && launch.transport_authority) {
  const diagnostic = {
    schema: "elastos.browser.media-diagnostic/v1",
    event: "turn_allocation_succeeded",
    binding_hash: launch.transport_authority.binding_hash,
    generation: launch.transport_authority.generation,
    page_id: launch.transport_authority.page_id,
    vm_id: launch.transport_authority.vm_id,
    media_stream_id: launch.transport_authority.media.stream_id,
    ordinal: 0,
  };
  process.stderr.write(`${JSON.stringify(diagnostic)}\n`);
  process.stderr.write(`${JSON.stringify({
    ...diagnostic,
    credential: "must-not-reach-control-service-log",
  })}\n`);
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
  const afterReady = process.env.TYPED_TRANSPORT_AFTER_READY;
  if (afterReady && launch.transport_authority) {
    const childAbsent = process.env.TYPED_TRANSPORT_CHILD_ABSENT !== "0";
    const delayedPorts = process.env.TYPED_TRANSPORT_DELAYED_PORTS === "1";
    const acted = afterReady !== "did_not_act";
    const profileDurability = process.env.TYPED_TRANSPORT_PROFILE_DURABILITY || "";
    process.stderr.write(`${JSON.stringify({
      schema: "elastos.browser.vz-launch-settlement/v1",
      state: afterReady,
      message: delayedPorts
        ? "injected delayed-port-only settlement"
        : `injected after-ready ${afterReady}`,
      binding_hash: launch.transport_authority.binding_hash,
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
        child_absent: childAbsent,
        supervisor_child_absent: childAbsent,
        control_socket_absent: true,
        route_absent: true,
        turn_listener_absent: delayedPorts ? false : true,
        turn_relay_ports_absent: delayedPorts ? false : true,
        ordinary_stream_bridge_absent: true,
        media_stream_bridge_absent: true,
        session_directory_absent: true,
        vm_absent: childAbsent,
      },
      ...(profileDurability
        ? { profile_durability: profileDurability }
        : {}),
    })}\n`);
    process.exit(1);
  }
});
}
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
if (/^[0-9a-f]{64}$/.test(process.env.CONFIG_FINGERPRINT || "")) {
  config.config_fingerprint = process.env.CONFIG_FINGERPRINT;
}
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
  TYPED_TRANSPORT_AFTER_READY="${TYPED_TRANSPORT_AFTER_READY:-}" \
  TYPED_TRANSPORT_CHILD_ABSENT="${TYPED_TRANSPORT_CHILD_ABSENT:-}" \
  TYPED_TRANSPORT_DELAYED_PORTS="${TYPED_TRANSPORT_DELAYED_PORTS:-}" \
  TYPED_TRANSPORT_PROFILE_DURABILITY="${TYPED_TRANSPORT_PROFILE_DURABILITY:-}" \
  TYPED_TRANSPORT_SUBSTITUTE="${TYPED_TRANSPORT_SUBSTITUTE:-}" \
  LOG_STARTED_OWNER_HOLD="${LOG_STARTED_OWNER_HOLD:-}" \
  OWNER_PID_FILE="${OWNER_PID_FILE:-}" \
  ELASTOS_BROWSER_VM_CONTROL_SERVICE_LOG="${ELASTOS_BROWSER_VM_CONTROL_SERVICE_LOG:-}" \
  MEDIA_DIAGNOSTIC_SMOKE="${MEDIA_DIAGNOSTIC_SMOKE:-}" \
  POLLUTE_STDOUT="${POLLUTE_STDOUT:-}" \
  HOLD_AFTER_POLLUTE="${HOLD_AFTER_POLLUTE:-}" \
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
import dgram from "node:dgram";
import fs from "node:fs";
import http from "node:http";
import net from "node:net";
import path from "node:path";

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

async function waitForReconcile(id, check, message) {
  const deadline = Date.now() + 5000;
  let last = null;
  while (Date.now() < deadline) {
    last = await reconcile(id);
    if (check(last)) return last;
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new Error(`${message}: ${JSON.stringify(last)}`);
}

function removeSessionDir(page) {
  const sessionDir = page?.isolation?.session_dir;
  if (typeof sessionDir === "string" && sessionDir.startsWith("/")) {
    fs.rmSync(sessionDir, { recursive: true, force: true });
  }
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
  let afterExitStatus = null;
  const exitDeadline = Date.now() + 5000;
  while (Date.now() < exitDeadline) {
    afterExitStatus = await request("GET", "/status");
    if (
      afterExitStatus.active_pages === 0 &&
      afterExitStatus.capacity_available === true &&
      afterExitStatus.pending_cleanup_pages === 1
    ) {
      break;
    }
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  if (
    afterExitStatus?.active_pages !== 0 ||
    afterExitStatus?.capacity_available !== true ||
    afterExitStatus?.pending_cleanup_pages !== 1
  ) {
    throw new Error(
      `launcher exit kept live capacity: ${JSON.stringify(afterExitStatus)}`,
    );
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
} else if (process.env.PHASE === "exit-then-reopen") {
  const first = await request("POST", "/pages", openBody(streamId));
  const live = await request("GET", "/status");
  if (
    live.active_pages !== 1 ||
    live.capacity_available !== false ||
    live.pending_cleanup_pages !== 0
  ) {
    throw new Error(`live page did not occupy capacity: ${JSON.stringify(live)}`);
  }
  process.kill(first.process.pid, "SIGTERM");
  await waitFor(
    () => !processAlive(first.process.pid),
    "exit-then-reopen child did not exit",
  );
  let afterExit = null;
  const exitDeadline = Date.now() + 5000;
  while (Date.now() < exitDeadline) {
    afterExit = await request("GET", "/status");
    if (
      afterExit.active_pages === 0 &&
      afterExit.capacity_available === true &&
      afterExit.pending_cleanup_pages === 1
    ) {
      break;
    }
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  if (
    afterExit?.active_pages !== 0 ||
    afterExit?.capacity_available !== true ||
    afterExit?.pending_cleanup_pages !== 1
  ) {
    throw new Error(
      `VM exit did not free live capacity: ${JSON.stringify(afterExit)}`,
    );
  }
  const pending = await reconcile(streamId);
  if (pending.state !== "cleanup_pending") {
    throw new Error(`VM exit synthesized terminal cleanup: ${JSON.stringify(pending)}`);
  }
  const closed = await request("POST", "/shutdown", closeBody(first));
  if (
    closed.terminal !== true ||
    Object.values(closed.effects || {}).some((value) => value !== true)
  ) {
    throw new Error(`post-exit close was not terminal: ${JSON.stringify(closed)}`);
  }
  const afterClose = await request("GET", "/status");
  if (
    afterClose.active_pages !== 0 ||
    afterClose.pending_cleanup_pages !== 0 ||
    afterClose.capacity_available !== true
  ) {
    throw new Error(`terminal close left pending capacity: ${JSON.stringify(afterClose)}`);
  }
  const reopenId = `${streamId}-reopen`;
  const second = await request("POST", "/pages", openBody(reopenId));
  const reopened = await request("GET", "/status");
  if (
    reopened.active_pages !== 1 ||
    reopened.capacity_available !== false ||
    second.page_id === first.page_id
  ) {
    throw new Error(`fresh open after VM exit failed: ${JSON.stringify({ second, reopened })}`);
  }
  const secondClose = await request("POST", "/shutdown", closeBody(second));
  if (secondClose.terminal !== true) {
    throw new Error(`reopened page close was not terminal: ${JSON.stringify(secondClose)}`);
  }
} else if (process.env.PHASE === "ungraceful-transport-close") {
  const first = await request("POST", "/pages", openBody(streamId));
  process.kill(first.process.pid, "SIGKILL");
  await waitFor(
    () => !processAlive(first.process.pid),
    "ungraceful-transport-close child did not exit",
  );
  let afterExit = null;
  const exitDeadline = Date.now() + 5000;
  while (Date.now() < exitDeadline) {
    afterExit = await request("GET", "/status");
    if (
      afterExit.active_pages === 0 &&
      afterExit.capacity_available === true &&
      afterExit.pending_cleanup_pages === 1
    ) {
      break;
    }
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  if (
    afterExit?.active_pages !== 0 ||
    afterExit?.capacity_available !== true ||
    afterExit?.pending_cleanup_pages !== 1
  ) {
    throw new Error(
      `ungraceful VM exit did not free live capacity: ${JSON.stringify(afterExit)}`,
    );
  }
  const pending = await reconcile(streamId);
  if (pending.state !== "cleanup_pending") {
    throw new Error(
      `ungraceful VM exit synthesized terminal cleanup: ${JSON.stringify(pending)}`,
    );
  }
  const closed = await requestRaw("POST", "/shutdown", closeBody(first));
  if (
    closed.status !== 400 ||
    !String(closed.body.error || "").includes("turn_process_absent")
  ) {
    throw new Error(
      `ungraceful post-exit close synthesized terminal TURN process absence: ${JSON.stringify(closed)}`,
    );
  }
  const afterClose = await request("GET", "/status");
  if (
    afterClose.active_pages !== 0 ||
    afterClose.pending_cleanup_pages !== 1 ||
    afterClose.capacity_available !== true
  ) {
    throw new Error(
      `ungraceful post-exit close released pending ownership: ${JSON.stringify(afterClose)}`,
    );
  }
} else if (process.env.PHASE === "exit-then-reopen-before-cleanup") {
  if (transportEnabled) {
    throw new Error("exit-then-reopen-before-cleanup is a local same-profile phase");
  }
  const first = await request("POST", "/pages", openBody(streamId));
  const live = await request("GET", "/status");
  if (
    live.active_pages !== 1 ||
    live.capacity_available !== false ||
    live.pending_cleanup_pages !== 0
  ) {
    throw new Error(`live page did not occupy capacity: ${JSON.stringify(live)}`);
  }
  process.kill(first.process.pid, "SIGTERM");
  await waitFor(
    () => !processAlive(first.process.pid),
    "exit-then-reopen-before-cleanup child did not exit",
  );
  let afterExit = null;
  const exitDeadline = Date.now() + 5000;
  while (Date.now() < exitDeadline) {
    afterExit = await request("GET", "/status");
    if (
      afterExit.active_pages === 0 &&
      afterExit.capacity_available === true &&
      afterExit.pending_cleanup_pages === 1 &&
      afterExit.pending_cleanup_page_ids?.[0] === first.page_id
    ) {
      break;
    }
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  if (
    afterExit?.active_pages !== 0 ||
    afterExit?.capacity_available !== true ||
    afterExit?.pending_cleanup_pages !== 1 ||
    afterExit?.pending_cleanup_page_ids?.[0] !== first.page_id
  ) {
    throw new Error(
      `VM exit did not free live capacity: ${JSON.stringify(afterExit)}`,
    );
  }
  const pending = await reconcile(streamId);
  if (pending.state !== "cleanup_pending") {
    throw new Error(`VM exit synthesized terminal cleanup: ${JSON.stringify(pending)}`);
  }
  const reopenId = `${streamId}-reopen`;
  const second = await request("POST", "/pages", openBody(reopenId));
  const overlapped = await request("GET", "/status");
  if (
    second.page_id === first.page_id ||
    overlapped.active_pages !== 1 ||
    overlapped.capacity_available !== false ||
    overlapped.pending_cleanup_pages !== 1 ||
    !overlapped.page_ids?.includes(second.page_id) ||
    overlapped.page_ids?.includes(first.page_id) ||
    !overlapped.pending_cleanup_page_ids?.includes(first.page_id) ||
    overlapped.pending_cleanup_page_ids?.includes(second.page_id)
  ) {
    throw new Error(
      `same-profile reopen revived the old pending page: ${JSON.stringify({ second, overlapped })}`,
    );
  }
  const oldStatus = await requestRaw(
    "GET",
    `/pages/${encodeURIComponent(first.page_id)}/status`,
  );
  if (
    oldStatus.status !== 404 ||
    !String(oldStatus.body.error || "").includes("cleanup is pending")
  ) {
    throw new Error(
      `old pending page did not stay cleanup-owned: ${JSON.stringify(oldStatus)}`,
    );
  }
  if (!processAlive(second.process.pid) || !fs.existsSync(second.control_socket_path)) {
    throw new Error("new same-profile page was not intact before old cleanup");
  }
  const closedOld = await request("POST", "/shutdown", closeBody(first));
  if (
    closedOld.terminal !== true ||
    Object.values(closedOld.effects || {}).some((value) => value !== true)
  ) {
    throw new Error(`old-owner cleanup was not terminal: ${JSON.stringify(closedOld)}`);
  }
  const afterOldClose = await request("GET", "/status");
  if (
    afterOldClose.active_pages !== 1 ||
    afterOldClose.capacity_available !== false ||
    afterOldClose.pending_cleanup_pages !== 0 ||
    !afterOldClose.page_ids?.includes(second.page_id) ||
    afterOldClose.page_ids?.includes(first.page_id)
  ) {
    throw new Error(
      `old-owner cleanup disturbed the new page: ${JSON.stringify(afterOldClose)}`,
    );
  }
  if (!processAlive(second.process.pid) || !fs.existsSync(second.control_socket_path)) {
    throw new Error("old-owner cleanup retired the new same-profile VM");
  }
  const terminalOld = await reconcile(streamId);
  if (terminalOld.state !== "terminal_post_effect_cleanup") {
    throw new Error(
      `old-owner cleanup did not persist terminal state: ${JSON.stringify(terminalOld)}`,
    );
  }
  const secondClose = await request("POST", "/shutdown", closeBody(second));
  if (secondClose.terminal !== true) {
    throw new Error(`new page close was not terminal: ${JSON.stringify(secondClose)}`);
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
} else if (process.env.PHASE === "verify-transport-restart") {
  const page = JSON.parse(fs.readFileSync(process.env.PAGE_FILE, "utf8"));
  issuedTransportAuthority = page.transport_authority || null;
  const pending = await reconcile(streamId);
  if (
    pending.state !== "cleanup_pending" ||
    pending.cleanup_binding?.page_id !== page.page_id ||
    pending.supervisor_result !== undefined
  ) {
    throw new Error(
      `transport restart did not retain only its exact durable cleanup binding: ${JSON.stringify(pending)}`,
    );
  }
  if (!processAlive(page.process.pid)) {
    throw new Error("transport restart lost its surviving owned launcher");
  }
  if (fs.existsSync(page.control_socket_path)) {
    fs.unlinkSync(page.control_socket_path);
  }
  const surviving = await requestRaw(
    "POST",
    "/shutdown",
    runtimeSerializedCloseBody(page),
  );
  if (
    surviving.status !== 400 ||
    !String(surviving.body.error || "").includes(
      "exact owned launcher unavailable",
    )
  ) {
    throw new Error(
      `surviving transport owner was synthesized terminal after restart: ${JSON.stringify(surviving)}`,
    );
  }
  if (!processAlive(page.process.pid)) {
    throw new Error("transport restart cleanup disturbed the surviving owned launcher");
  }
  if ((await reconcile(streamId)).state !== "cleanup_pending") {
    throw new Error("surviving transport owner did not remain pending");
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

  const unexpectedUdpRelay = dgram.createSocket({
    type: "udp4",
    reuseAddr: false,
  });
  await new Promise((resolve, reject) => {
    unexpectedUdpRelay.once("error", reject);
    unexpectedUdpRelay.bind(
      {
        address: page.transport_authority.turn.relay_host,
        port: page.transport_authority.turn.relay_port_min,
        exclusive: true,
      },
      resolve,
    );
  });
  const occupiedRelay = await requestRaw(
    "POST",
    "/shutdown",
    closeBody(page),
  );
  if (
    occupiedRelay.status !== 400 ||
    !String(occupiedRelay.body.error || "").includes(
      "turn_relay_ports_absent",
    )
  ) {
    throw new Error(
      `occupied UDP relay port did not retain cleanup ownership: ${JSON.stringify(occupiedRelay)}`,
    );
  }
  await new Promise((resolve) => unexpectedUdpRelay.close(resolve));

  const listenUnix = (path) =>
    new Promise((resolve, reject) => {
      try {
        fs.unlinkSync(path);
      } catch (error) {
        if (error?.code !== "ENOENT") {
          reject(error);
          return;
        }
      }
      const server = net.createServer();
      server.once("error", reject);
      server.listen(path, () => resolve(server));
    });
  const runtimeEgress = await listenUnix(
    page.transport_authority.egress.runtime_socket_path,
  );
  const runtimeMedia = await listenUnix(
    page.transport_authority.media.runtime_socket_path,
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
  if (
    !fs.existsSync(page.transport_authority.egress.runtime_socket_path) ||
    !fs.existsSync(page.transport_authority.media.runtime_socket_path)
  ) {
    throw new Error(
      "Runtime stream sockets remaining after owned launcher cleanup were unlinked by helper",
    );
  }
  await new Promise((resolve) => runtimeEgress.close(resolve));
  await new Promise((resolve) => runtimeMedia.close(resolve));
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
} else if (process.env.PHASE === "polluted-stdout-ready") {
  const page = await request("POST", "/pages", openBody(streamId));
  if (page.schema !== "elastos.browser.engine.supervisor-result/v1") {
    throw new Error(
      `polluted stdout hid the supervisor result: ${JSON.stringify(page)}`,
    );
  }
  const close = await request("POST", "/shutdown", closeBody(page));
  if (close.terminal !== true) {
    throw new Error(`polluted-stdout ready close was not terminal: ${JSON.stringify(close)}`);
  }
} else if (process.env.PHASE === "polluted-stdout-hold") {
  const failed = await requestRaw("POST", "/pages", openBody(streamId));
  if (
    failed.status !== 400 ||
    failed.body.launch_settlement_result?.state !==
      "terminal_post_effect_cleanup" ||
    failed.body.launch_settlement_result?.binding_hash !==
      issuedTransportAuthority.binding_hash
  ) {
    throw new Error(
      `polluted stdout hold did not reap a typed settlement: ${JSON.stringify(failed)}`,
    );
  }
  const durable = await reconcile(streamId);
  if (
    durable.state !== "terminal_post_effect_cleanup" ||
    durable.launch_settlement_result?.binding_hash !==
      issuedTransportAuthority.binding_hash
  ) {
    throw new Error(
      `polluted stdout hold settlement was not durable: ${JSON.stringify(durable)}`,
    );
  }
} else if (process.env.PHASE === "delayed-turn-child-present") {
  const page = await request("POST", "/pages", openBody(streamId));
  issuedTransportAuthority = page.transport_authority || null;
  if (process.env.PAGE_FILE) {
    fs.writeFileSync(process.env.PAGE_FILE, JSON.stringify(page));
  }
  await waitFor(
    () => !processAlive(page.process.pid),
    "after-ready child with child_absent=false did not exit",
  );
  const pending = await waitForReconcile(
    streamId,
    (record) =>
      record.state === "cleanup_pending" &&
      record.cleanup_binding?.page_id === page.page_id &&
      record.launch_settlement_result?.absence?.child_absent === false &&
      record.launch_settlement_result?.effects?.turn_process === true,
    "after-ready child_absent=false settlement was not durable",
  );
  removeSessionDir(page);
  const closed = await requestRaw(
    "POST",
    "/shutdown",
    runtimeSerializedCloseBody(page),
  );
  if (closed.status !== 400 || closed.body?.terminal === true) {
    throw new Error(
      `delayed turn with child_absent=false became terminal: ${JSON.stringify(closed)}`,
    );
  }
  const durable = await reconcile(streamId);
  if (
    durable.state !== "cleanup_pending" ||
    durable.launch_settlement_result?.absence?.child_absent !== false ||
    durable.launch_settlement_result?.binding_hash !==
      pending.launch_settlement_result?.binding_hash
  ) {
    throw new Error(
      `delayed turn with child_absent=false did not remain pending: ${JSON.stringify(durable)}`,
    );
  }
} else if (process.env.PHASE === "verify-delayed-turn-child-present-restart") {
  const page = JSON.parse(fs.readFileSync(process.env.PAGE_FILE, "utf8"));
  issuedTransportAuthority = page.transport_authority || null;
  removeSessionDir(page);
  const pending = await reconcile(streamId);
  if (
    pending.state !== "cleanup_pending" ||
    pending.cleanup_binding?.page_id !== page.page_id ||
    pending.launch_settlement_result?.absence?.child_absent !== false
  ) {
    throw new Error(
      `restart lost child_absent=false cleanup ownership: ${JSON.stringify(pending)}`,
    );
  }
  const closed = await requestRaw(
    "POST",
    "/shutdown",
    runtimeSerializedCloseBody(page),
  );
  if (closed.status !== 400 || closed.body?.terminal === true) {
    throw new Error(
      `restart delayed turn with child_absent=false became terminal: ${JSON.stringify(closed)}`,
    );
  }
  if ((await reconcile(streamId)).state !== "cleanup_pending") {
    throw new Error("restart delayed turn with child_absent=false did not remain pending");
  }
} else if (process.env.PHASE === "delayed-turn-port-only") {
  const page = await request("POST", "/pages", openBody(streamId));
  issuedTransportAuthority = page.transport_authority || null;
  if (process.env.PAGE_FILE) {
    fs.writeFileSync(process.env.PAGE_FILE, JSON.stringify(page));
  }
  await waitFor(
    () => !processAlive(page.process.pid),
    "delayed-port-only child did not exit",
  );
  await waitForReconcile(
    streamId,
    (record) =>
      record.state === "cleanup_pending" &&
      record.cleanup_binding?.page_id === page.page_id &&
      record.launch_settlement_result?.absence?.child_absent === true &&
      record.launch_settlement_result?.absence?.turn_listener_absent === false &&
      record.launch_settlement_result?.absence?.turn_relay_ports_absent === false &&
      (!process.env.TYPED_TRANSPORT_PROFILE_DURABILITY ||
        record.profile_durability ===
          process.env.TYPED_TRANSPORT_PROFILE_DURABILITY ||
        record.launch_settlement_result?.profile_durability ===
          process.env.TYPED_TRANSPORT_PROFILE_DURABILITY),
    "delayed-port-only settlement was not durable",
  );
} else if (process.env.PHASE === "verify-delayed-turn-port-only-restart") {
  const page = JSON.parse(fs.readFileSync(process.env.PAGE_FILE, "utf8"));
  issuedTransportAuthority = page.transport_authority || null;
  removeSessionDir(page);
  const pending = await reconcile(streamId);
  if (
    pending.state !== "cleanup_pending" ||
    pending.launch_settlement_result?.absence?.child_absent !== true ||
    pending.launch_settlement_result?.absence?.turn_listener_absent !== false
  ) {
    throw new Error(
      `restart lost delayed-port-only cleanup ownership: ${JSON.stringify(pending)}`,
    );
  }
  const closed = await request("POST", "/shutdown", runtimeSerializedCloseBody(page));
  if (
    closed.terminal !== true ||
    closed.delayed_turn_port_absence !== true
  ) {
    throw new Error(
      `delayed-port-only restart close was not delayed terminal: ${JSON.stringify(closed)}`,
    );
  }
  const durable = await reconcile(streamId);
  if (
    durable.state !== "terminal_post_effect_cleanup" ||
    durable.terminal_cleanup_receipt?.delayed_turn_port_absence !== true
  ) {
    throw new Error(
      `delayed-port-only restart close was not durable: ${JSON.stringify(durable)}`,
    );
  }
} else if (process.env.PHASE === "log-started-paths-gone-owner-alive") {
  const failed = await requestRaw("POST", "/pages", openBody(streamId));
  if (failed.status !== 400) {
    throw new Error(
      `logged started launch did not fail closed: ${JSON.stringify(failed)}`,
    );
  }
  const ownerPid = Number(
    String(fs.readFileSync(process.env.OWNER_PID_FILE, "utf8")).trim(),
  );
  if (!Number.isInteger(ownerPid) || !processAlive(ownerPid)) {
    throw new Error("logged started owner is not alive before reconcile");
  }
  const digest = String(issuedTransportAuthority.binding_hash)
    .replace(/^sha256:/, "")
    .toLowerCase();
  const segment = digest.slice(0, 32);
  const sessionDir = path.join(
    process.env.ELASTOS_BROWSER_VM_ROOT || "/tmp/evzs",
    `vz-${segment}`,
  );
  const socketDir = path.join(
    process.env.ELASTOS_BROWSER_VM_SOCKET_ROOT || "/tmp/evzrc",
    segment,
  );
  fs.mkdirSync(sessionDir, { recursive: true, mode: 0o700 });
  fs.mkdirSync(socketDir, { recursive: true, mode: 0o700 });
  fs.writeFileSync(path.join(socketDir, "c.sock"), "");
  fs.rmSync(sessionDir, { recursive: true, force: true });
  fs.rmSync(socketDir, { recursive: true, force: true });
  const durable = await reconcile(streamId);
  if (durable.state !== "cleanup_pending") {
    throw new Error(
      `logged started launch with a live owner became terminal: ${JSON.stringify(durable)}`,
    );
  }
  if (!processAlive(ownerPid)) {
    throw new Error("logged started owner was reaped during manufactured settlement");
  }
} else if (process.env.PHASE === "log-started-owner-exits-without-native-settlement") {
  const failed = await requestRaw("POST", "/pages", openBody(streamId));
  if (failed.status !== 400) {
    throw new Error(
      `logged started launch did not fail closed: ${JSON.stringify(failed)}`,
    );
  }
  const digest = String(issuedTransportAuthority.binding_hash)
    .replace(/^sha256:/, "")
    .toLowerCase();
  const segment = digest.slice(0, 32);
  const sessionDir = path.join(
    process.env.ELASTOS_BROWSER_VM_ROOT || "/tmp/evzs",
    `vz-${segment}`,
  );
  const socketDir = path.join(
    process.env.ELASTOS_BROWSER_VM_SOCKET_ROOT || "/tmp/evzrc",
    segment,
  );
  const controlSocket = path.join(socketDir, "c.sock");
  fs.mkdirSync(sessionDir, { recursive: true, mode: 0o700 });
  fs.mkdirSync(socketDir, { recursive: true, mode: 0o700 });
  fs.writeFileSync(
    path.join(socketDir, "owner.json"),
    `${JSON.stringify({
      schema: "elastos.browser.vz-socket-owner/v1",
      binding_hash: issuedTransportAuthority.binding_hash,
      generation: issuedTransportAuthority.generation,
      page_id: issuedTransportAuthority.page_id,
      vm_id: issuedTransportAuthority.vm_id,
      stream_id: issuedTransportAuthority.egress.stream_id,
      media_stream_id: issuedTransportAuthority.media.stream_id,
    })}\n`,
    { mode: 0o600 },
  );
  const holder = net.createServer();
  await new Promise((resolve, reject) => {
    holder.once("error", reject);
    holder.listen(controlSocket, resolve);
  });
  const live = await reconcile(streamId);
  if (
    live.state !== "cleanup_pending" ||
    live.launch_settlement_result?.state === "terminal_post_effect_cleanup"
  ) {
    throw new Error(
      `once-live owner manufactured a native settlement: ${JSON.stringify(live)}`,
    );
  }
  await new Promise((resolve) => holder.close(resolve));
  fs.rmSync(sessionDir, { recursive: true, force: true });
  fs.rmSync(socketDir, { recursive: true, force: true });
  const durable = await reconcile(streamId);
  if (
    durable.state !== "cleanup_pending" ||
    durable.launch_settlement_result?.state === "terminal_post_effect_cleanup"
  ) {
    throw new Error(
      `exited owner without a native settlement became terminal: ${JSON.stringify(durable)}`,
    );
  }
} else if (process.env.PHASE === "native-terminal-then-ordinary-close") {
  const page = await request("POST", "/pages", openBody(streamId));
  issuedTransportAuthority = page.transport_authority || null;
  await waitFor(
    () => !processAlive(page.process.pid),
    "native terminal child did not exit",
  );
  await waitForReconcile(
    streamId,
    (record) =>
      record.state === "terminal_post_effect_cleanup" &&
      record.launch_settlement_result?.state ===
        "terminal_post_effect_cleanup" &&
      record.launch_settlement_result?.absence?.child_absent === true &&
      record.terminal_cleanup_receipt === undefined &&
      (record.profile_durability === "failed" ||
        record.launch_settlement_result?.profile_durability === "failed"),
    "native terminal settlement was not durable before ordinary close",
  );
  const closed = await request("POST", "/shutdown", runtimeSerializedCloseBody(page));
  if (
    closed.terminal !== true ||
    closed.effects?.child_absent !== true ||
    closed.profile_durability !== "failed"
  ) {
    throw new Error(
      `ordinary close after native terminal lost failed durability or child absence: ${JSON.stringify(closed)}`,
    );
  }
  const durable = await reconcile(streamId);
  if (
    durable.state !== "terminal_post_effect_cleanup" ||
    durable.launch_settlement_result?.state !==
      "terminal_post_effect_cleanup" ||
    durable.profile_durability !== "failed" ||
    durable.terminal_cleanup_receipt?.effects?.child_absent !== true ||
    durable.terminal_cleanup_receipt?.profile_durability !== "failed"
  ) {
    throw new Error(
      `ordinary close after native terminal did not retain settlement and receipt: ${JSON.stringify(durable)}`,
    );
  }
} else if (process.env.PHASE === "verify-failed-flush-durability-restart") {
  const page = JSON.parse(fs.readFileSync(process.env.PAGE_FILE, "utf8"));
  issuedTransportAuthority = page.transport_authority || null;
  removeSessionDir(page);
  const pending = await reconcile(streamId);
  if (
    pending.state !== "cleanup_pending" ||
    pending.launch_settlement_result?.absence?.child_absent !== true ||
    (pending.profile_durability !== "failed" &&
      pending.launch_settlement_result?.profile_durability !== "failed")
  ) {
    throw new Error(
      `restart lost failed profile durability: ${JSON.stringify(pending)}`,
    );
  }
  const closed = await request("POST", "/shutdown", runtimeSerializedCloseBody(page));
  if (
    closed.terminal !== true ||
    closed.delayed_turn_port_absence !== true ||
    closed.profile_durability !== "failed"
  ) {
    throw new Error(
      `failed durability close lost the data-save failure: ${JSON.stringify(closed)}`,
    );
  }
  const durable = await reconcile(streamId);
  if (
    durable.state !== "terminal_post_effect_cleanup" ||
    durable.profile_durability !== "failed" ||
    durable.terminal_cleanup_receipt?.effects?.child_absent !== true
  ) {
    throw new Error(
      `failed durability reconcile lost truthful child absence: ${JSON.stringify(durable)}`,
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

reopen_socket="$tmp_dir/reopen-control.sock"
start_service "$reopen_socket" "" "reopen-service"
CONTROL_SOCKET="$reopen_socket" \
STREAM_ID="stream:settlement-exit-then-reopen" \
PHASE="exit-then-reopen" \
  "$node_bin" "$client"
stop_service

reopen_transport_socket="$tmp_dir/reopen-transport-control.sock"
start_service "$reopen_transport_socket" "" "reopen-transport-service"
TRANSPORT=1 \
CONTROL_SOCKET="$reopen_transport_socket" \
STREAM_ID="stream:settlement-exit-then-reopen-transport" \
PHASE="exit-then-reopen" \
  "$node_bin" "$client"
stop_service

ungraceful_transport_socket="$tmp_dir/ungraceful-transport-control.sock"
start_service "$ungraceful_transport_socket" "" "ungraceful-transport-service"
TRANSPORT=1 \
CONTROL_SOCKET="$ungraceful_transport_socket" \
STREAM_ID="stream:settlement-ungraceful-transport-close" \
PHASE="ungraceful-transport-close" \
  "$node_bin" "$client"
stop_service

reopen_before_cleanup_socket="$tmp_dir/reopen-before-cleanup-control.sock"
start_service "$reopen_before_cleanup_socket" "" "reopen-before-cleanup-service"
CONTROL_SOCKET="$reopen_before_cleanup_socket" \
STREAM_ID="stream:settlement-exit-then-reopen-before-cleanup" \
PHASE="exit-then-reopen-before-cleanup" \
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

transport_restart_socket="$tmp_dir/transport-restart-control.sock"
transport_restart_page="$tmp_dir/transport-restart-page.json"
start_service "$transport_restart_socket" "" "transport-restart-service-first"
TRANSPORT=1 \
CONTROL_SOCKET="$transport_restart_socket" \
STREAM_ID="stream:settlement-transport-restart" \
PAGE_FILE="$transport_restart_page" \
PHASE="open-for-restart" \
  "$node_bin" "$client"
kill -KILL "$service_pid"
wait "$service_pid" 2>/dev/null || true
service_pid=""
rm -f "$transport_restart_socket"
start_service "$transport_restart_socket" "" "transport-restart-service-second"
TRANSPORT=1 \
CONTROL_SOCKET="$transport_restart_socket" \
STREAM_ID="stream:settlement-transport-restart" \
PAGE_FILE="$transport_restart_page" \
PHASE="verify-transport-restart" \
  "$node_bin" "$client"
stop_service

fingerprint_socket="$tmp_dir/fingerprint-control.sock"
fingerprint_page="$tmp_dir/fingerprint-page.json"
start_service "$fingerprint_socket" "" "fingerprint-service-first"
CONTROL_SOCKET="$fingerprint_socket" \
STREAM_ID="stream:settlement-fingerprint-restart" \
PAGE_FILE="$fingerprint_page" \
PHASE="open-for-restart" \
  "$node_bin" "$client"
kill -KILL "$service_pid"
wait "$service_pid" 2>/dev/null || true
service_pid=""
rm -f "$fingerprint_socket"
CONFIG_FINGERPRINT="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" \
start_service "$fingerprint_socket" "" "fingerprint-service-second"
CONTROL_SOCKET="$fingerprint_socket" \
STREAM_ID="stream:settlement-fingerprint-restart" \
PAGE_FILE="$fingerprint_page" \
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
MEDIA_DIAGNOSTIC_SMOKE=1 \
  start_service "$transport_socket" "" "transport-service"
CONTROL_SOCKET="$transport_socket" \
STREAM_ID="stream:transport-settlement" \
JOURNAL_PATH="$transport_journal" \
PHASE="transport-cleanup" \
TRANSPORT=1 \
  "$node_bin" "$client"
stop_service
grep -q '"event":"media_diagnostic".*"diagnostic_event":"turn_allocation_succeeded"' \
  "$tmp_dir/transport-service.err"
if grep -q 'must-not-reach-control-service-log' "$tmp_dir/transport-service.err"; then
  echo "invalid Browser media diagnostic leaked child stderr into the control log" >&2
  exit 1
fi

pollute_ready_socket="$tmp_dir/pollute-ready-control.sock"
POLLUTE_STDOUT=1 \
  start_service "$pollute_ready_socket" "" "pollute-ready-service"
CONTROL_SOCKET="$pollute_ready_socket" \
STREAM_ID="stream:polluted-stdout-ready" \
PHASE="polluted-stdout-ready" \
TRANSPORT=1 \
  "$node_bin" "$client"
stop_service
unset POLLUTE_STDOUT

pollute_hold_socket="$tmp_dir/pollute-hold-control.sock"
pollute_hold_journal="${pollute_hold_socket}.launch-reconciliations.json"
POLLUTE_STDOUT=1 \
HOLD_AFTER_POLLUTE=1 \
  start_service "$pollute_hold_socket" "" "pollute-hold-service"
CONTROL_SOCKET="$pollute_hold_socket" \
STREAM_ID="stream:polluted-stdout-hold" \
JOURNAL_PATH="$pollute_hold_journal" \
PHASE="polluted-stdout-hold" \
TRANSPORT=1 \
  "$node_bin" "$client"
stop_service
unset POLLUTE_STDOUT
unset HOLD_AFTER_POLLUTE

delayed_child_socket="$tmp_dir/delayed-turn-child-present-control.sock"
delayed_child_journal="${delayed_child_socket}.launch-reconciliations.json"
delayed_child_page="$tmp_dir/delayed-turn-child-present-page.json"
TYPED_TRANSPORT_AFTER_READY="cleanup_pending" \
TYPED_TRANSPORT_CHILD_ABSENT="0" \
  start_service "$delayed_child_socket" "" "delayed-turn-child-present-service"
CONTROL_SOCKET="$delayed_child_socket" \
STREAM_ID="stream:delayed-turn-child-present" \
JOURNAL_PATH="$delayed_child_journal" \
PAGE_FILE="$delayed_child_page" \
PHASE="delayed-turn-child-present" \
TRANSPORT=1 \
  "$node_bin" "$client"
kill -KILL "$service_pid"
wait "$service_pid" 2>/dev/null || true
service_pid=""
rm -f "$delayed_child_socket"
TYPED_TRANSPORT_AFTER_READY="cleanup_pending" \
TYPED_TRANSPORT_CHILD_ABSENT="0" \
  start_service "$delayed_child_socket" "" "delayed-turn-child-present-restart-service"
CONTROL_SOCKET="$delayed_child_socket" \
STREAM_ID="stream:delayed-turn-child-present" \
JOURNAL_PATH="$delayed_child_journal" \
PAGE_FILE="$delayed_child_page" \
PHASE="verify-delayed-turn-child-present-restart" \
TRANSPORT=1 \
  "$node_bin" "$client"
stop_service
unset TYPED_TRANSPORT_AFTER_READY
unset TYPED_TRANSPORT_CHILD_ABSENT

delayed_port_socket="$tmp_dir/delayed-turn-port-only-control.sock"
delayed_port_journal="${delayed_port_socket}.launch-reconciliations.json"
delayed_port_page="$tmp_dir/delayed-turn-port-only-page.json"
TYPED_TRANSPORT_AFTER_READY="cleanup_pending" \
TYPED_TRANSPORT_DELAYED_PORTS="1" \
  start_service "$delayed_port_socket" "" "delayed-turn-port-only-service"
CONTROL_SOCKET="$delayed_port_socket" \
STREAM_ID="stream:delayed-turn-port-only" \
JOURNAL_PATH="$delayed_port_journal" \
PAGE_FILE="$delayed_port_page" \
PHASE="delayed-turn-port-only" \
TRANSPORT=1 \
  "$node_bin" "$client"
kill -KILL "$service_pid"
wait "$service_pid" 2>/dev/null || true
service_pid=""
rm -f "$delayed_port_socket"
TYPED_TRANSPORT_AFTER_READY="cleanup_pending" \
TYPED_TRANSPORT_DELAYED_PORTS="1" \
  start_service "$delayed_port_socket" "" "delayed-turn-port-only-restart-service"
CONTROL_SOCKET="$delayed_port_socket" \
STREAM_ID="stream:delayed-turn-port-only" \
JOURNAL_PATH="$delayed_port_journal" \
PAGE_FILE="$delayed_port_page" \
PHASE="verify-delayed-turn-port-only-restart" \
TRANSPORT=1 \
  "$node_bin" "$client"
stop_service
unset TYPED_TRANSPORT_AFTER_READY
unset TYPED_TRANSPORT_DELAYED_PORTS

failed_flush_socket="$tmp_dir/failed-flush-durability-control.sock"
failed_flush_journal="${failed_flush_socket}.launch-reconciliations.json"
failed_flush_page="$tmp_dir/failed-flush-durability-page.json"
TYPED_TRANSPORT_AFTER_READY="cleanup_pending" \
TYPED_TRANSPORT_DELAYED_PORTS="1" \
TYPED_TRANSPORT_PROFILE_DURABILITY="failed" \
  start_service "$failed_flush_socket" "" "failed-flush-durability-service"
CONTROL_SOCKET="$failed_flush_socket" \
STREAM_ID="stream:failed-flush-durability" \
JOURNAL_PATH="$failed_flush_journal" \
PAGE_FILE="$failed_flush_page" \
PHASE="delayed-turn-port-only" \
TRANSPORT=1 \
TYPED_TRANSPORT_PROFILE_DURABILITY="failed" \
  "$node_bin" "$client"
kill -KILL "$service_pid"
wait "$service_pid" 2>/dev/null || true
service_pid=""
rm -f "$failed_flush_socket"
TYPED_TRANSPORT_AFTER_READY="cleanup_pending" \
TYPED_TRANSPORT_DELAYED_PORTS="1" \
TYPED_TRANSPORT_PROFILE_DURABILITY="failed" \
  start_service "$failed_flush_socket" "" "failed-flush-durability-restart-service"
CONTROL_SOCKET="$failed_flush_socket" \
STREAM_ID="stream:failed-flush-durability" \
JOURNAL_PATH="$failed_flush_journal" \
PAGE_FILE="$failed_flush_page" \
PHASE="verify-failed-flush-durability-restart" \
TRANSPORT=1 \
TYPED_TRANSPORT_PROFILE_DURABILITY="failed" \
  "$node_bin" "$client"
stop_service
unset TYPED_TRANSPORT_AFTER_READY
unset TYPED_TRANSPORT_DELAYED_PORTS
unset TYPED_TRANSPORT_PROFILE_DURABILITY

native_close_socket="$tmp_dir/native-terminal-ordinary-close-control.sock"
native_close_journal="${native_close_socket}.launch-reconciliations.json"
TYPED_TRANSPORT_AFTER_READY="terminal_post_effect_cleanup" \
TYPED_TRANSPORT_PROFILE_DURABILITY="failed" \
  start_service "$native_close_socket" "" "native-terminal-ordinary-close-service"
CONTROL_SOCKET="$native_close_socket" \
STREAM_ID="stream:native-terminal-ordinary-close" \
JOURNAL_PATH="$native_close_journal" \
PHASE="native-terminal-then-ordinary-close" \
TRANSPORT=1 \
TYPED_TRANSPORT_PROFILE_DURABILITY="failed" \
  "$node_bin" "$client"
stop_service
unset TYPED_TRANSPORT_AFTER_READY
unset TYPED_TRANSPORT_PROFILE_DURABILITY

log_started_socket="$tmp_dir/log-started-owner-alive-control.sock"
log_started_journal="${log_started_socket}.launch-reconciliations.json"
log_started_owner="$tmp_dir/log-started-owner.pid"
LOG_STARTED_OWNER_HOLD=1 \
OWNER_PID_FILE="$log_started_owner" \
ELASTOS_BROWSER_VM_CONTROL_SERVICE_LOG="$tmp_dir/log-started-owner-alive-service.err" \
  start_service "$log_started_socket" "" "log-started-owner-alive-service"
CONTROL_SOCKET="$log_started_socket" \
STREAM_ID="stream:log-started-owner-alive" \
JOURNAL_PATH="$log_started_journal" \
OWNER_PID_FILE="$log_started_owner" \
PHASE="log-started-paths-gone-owner-alive" \
TRANSPORT=1 \
  "$node_bin" "$client"
stop_service
if [[ -f "$log_started_owner" ]]; then
  owner_pid="$(tr -d '[:space:]' < "$log_started_owner")"
  if [[ "$owner_pid" =~ ^[0-9]+$ ]]; then
    kill "$owner_pid" >/dev/null 2>&1 || true
  fi
fi
unset LOG_STARTED_OWNER_HOLD
unset OWNER_PID_FILE
unset ELASTOS_BROWSER_VM_CONTROL_SERVICE_LOG

log_exited_socket="$tmp_dir/log-started-owner-exited-control.sock"
log_exited_journal="${log_exited_socket}.launch-reconciliations.json"
LOG_STARTED_OWNER_HOLD=1 \
ELASTOS_BROWSER_VM_CONTROL_SERVICE_LOG="$tmp_dir/log-started-owner-exited-service.err" \
  start_service "$log_exited_socket" "" "log-started-owner-exited-service"
CONTROL_SOCKET="$log_exited_socket" \
STREAM_ID="stream:log-started-owner-exited" \
JOURNAL_PATH="$log_exited_journal" \
PHASE="log-started-owner-exits-without-native-settlement" \
TRANSPORT=1 \
  "$node_bin" "$client"
stop_service
unset LOG_STARTED_OWNER_HOLD
unset ELASTOS_BROWSER_VM_CONTROL_SERVICE_LOG

printf '%s\n' '{"schema":"elastos.browser.vm-control-service-settlement-smoke/v1","ok":true}'
