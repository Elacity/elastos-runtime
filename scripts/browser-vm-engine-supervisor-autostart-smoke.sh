#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"

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
export ELASTOS_NODE_BIN="$node_bin"
launcher_pid_file="$tmp_dir/fake-launcher.pids"
foreign_server_pid=""
foreign_target_pid=""
unresponsive_server_pid=""

cleanup() {
  local service_pid=""
  if [[ -S "$tmp_dir/browser-vm-control.sock" ]]; then
    service_pid="$(curl --unix-socket "$tmp_dir/browser-vm-control.sock" -sS --max-time 3 \
      http://localhost/status 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("pid",""))' 2>/dev/null || true)"
    curl --unix-socket "$tmp_dir/browser-vm-control.sock" -sS --max-time 3 \
      -H 'Content-Type: application/json' \
      -d '{"page_id":"page:vm-autostart-smoke"}' \
      http://localhost/shutdown >/dev/null 2>&1 || true
    for _ in 1 2 3 4 5 6 7 8 9 10; do
      active_pages="$(curl --unix-socket "$tmp_dir/browser-vm-control.sock" -sS --max-time 1 \
        http://localhost/status 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("active_pages",""))' 2>/dev/null || true)"
      [[ "$active_pages" == "0" ]] && break
      sleep 0.1
    done
    if [[ -n "$service_pid" ]]; then
      kill "$service_pid" >/dev/null 2>&1 || true
      for _ in 1 2 3 4 5 6 7 8 9 10; do
        kill -0 "$service_pid" >/dev/null 2>&1 || break
        sleep 0.1
      done
      kill -KILL "$service_pid" >/dev/null 2>&1 || true
    fi
  fi
  if [[ -f "$launcher_pid_file" ]]; then
    sort -u "$launcher_pid_file" | while IFS= read -r pid; do
      [[ "$pid" =~ ^[0-9]+$ ]] || continue
      [[ "$pid" == "$$" ]] && continue
      kill "$pid" >/dev/null 2>&1 || true
    done
    sleep 0.1
    sort -u "$launcher_pid_file" | while IFS= read -r pid; do
      [[ "$pid" =~ ^[0-9]+$ ]] || continue
      [[ "$pid" == "$$" ]] && continue
      kill -KILL "$pid" >/dev/null 2>&1 || true
    done
  fi
  if [[ -n "$foreign_server_pid" ]]; then
    kill "$foreign_server_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "$foreign_target_pid" ]]; then
    kill "$foreign_target_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "$unresponsive_server_pid" ]]; then
    kill "$unresponsive_server_pid" >/dev/null 2>&1 || true
  fi
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

cd "$repo_root"

control_socket="$tmp_dir/browser-vm-control.sock"
control_log="$tmp_dir/browser-vm-control.log"
vm_root="$tmp_dir/sessions"
data_dir="$tmp_dir/data"
mkdir -p "$data_dir/bin" "$vm_root"

cat > "$tmp_dir/control-service" <<SH
#!/usr/bin/env bash
set -euo pipefail
exec "$node_bin" "$repo_root/scripts/browser-vm-control-service.mjs"
SH
chmod 755 "$tmp_dir/control-service"

cat > "$tmp_dir/fake-vz-launcher.cjs" <<'NODE'
const fs = require("node:fs");
const path = require("node:path");
if (process.env.ELASTOS_BROWSER_VM_AUTOSTART_PID_FILE) {
  fs.appendFileSync(
    process.env.ELASTOS_BROWSER_VM_AUTOSTART_PID_FILE,
    `${process.pid}\n`,
  );
}
const raw = fs.readFileSync(0, "utf8").trim() || process.env.ELASTOS_BROWSER_VM_OPEN_REQUEST;
const body = JSON.parse(raw);
const launch = body.launch_request;
if (body.profile?.schema !== "elastos.browser.profile/v1"
    || body.profile?.storage !== "principal_owned_profile_disk"
    || body.profile?.storage_posture !== "principal_owned_reset_scoped_unprotected"
    || body.profile?.protected_storage !== false
    || body.profile?.encrypted !== false
    || body.profile?.recoverable !== false
    || body.profile?.recovery !== "not_recovery_kit_packaged"
    || body.profile?.public_uri !== "localhost://Users/self/BrowserProfiles/default/profile.ext4"
    || !/^profile-[0-9a-f]{64}$/.test(body.profile?.profile_key || "")
    || !String(body.profile?.disk_path || "").endsWith("/BrowserProfiles/default/profile.ext4")) {
  throw new Error("VM open request must carry the Runtime Browser profile descriptor");
}
if (
  Boolean(launch.transport_authority) !== Boolean(launch.transport_secret) ||
  Boolean(launch.transport_authority) !== Boolean(launch.page_id) ||
  Boolean(launch.transport_authority) !== Boolean(launch.vm_id)
) {
  throw new Error("VM open request carried an incomplete VZ transport binding");
}
const transportReceipt = launch.transport_authority
  ? {
      schema: "elastos.browser.vz-transport-effect-receipt/v1",
      binding_hash: launch.transport_authority.binding_hash,
      generation: launch.transport_authority.generation,
      page_id: launch.transport_authority.page_id,
      vm_id: launch.transport_authority.vm_id,
      expires_at_unix_ms: launch.transport_authority.expires_at_unix_ms,
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
console.log(JSON.stringify({
  schema: "elastos.browser.engine.supervisor-result/v1",
  page_id: launch.page_id || "page:vm-autostart-smoke",
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
  title: "ElastOS Browser VM Autostart Smoke",
  network_mode: "runtime_net_only",
  direct_network: false,
  wallet_injection: false,
  control_socket_path: "/tmp/elastos-browser-vm-autostart-page.sock",
  isolated_session: true,
  isolation: {
    schema: "elastos.browser.engine.isolation/v1",
    kind: "per_launch_vm_target",
    session_dir: "/tmp/elastos-browser-vm-autostart-smoke",
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
      sdp: "v=0\r\ns=ElastOS Browser VM Autostart Smoke\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n",
    },
    audio_offer: {
      schema: "elastos.browser.webrtc-offer/v1",
      type: "offer",
      sdp: "v=0\r\ns=ElastOS Browser VM Autostart Smoke Audio\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n",
    },
    ...(launch.transport_authority
      ? { ice_connection_policy: "engine_relay_only" }
      : {
          ice_servers: JSON.parse(
            process.env.ELASTOS_BROWSER_VM_ICE_SERVERS_JSON || "[]",
          ),
        }),
    display_backend: "vm_selkies_gstreamer_webrtc",
    backend_class: "product_compositor",
    media_transport: "runtime_relay",
    audio: true,
    video: true,
    network_mode: "runtime_net_only",
    direct_network: false,
    signaling_url: "/api/apps/browser/pages/page%3Avm-autostart-smoke/webrtc",
  },
}));
process.once("SIGTERM", () => process.exit(0));
process.once("SIGINT", () => process.exit(0));
setInterval(() => {}, 1000);
NODE
cat > "$tmp_dir/fake-vz-launcher" <<SH
#!/usr/bin/env bash
set -euo pipefail
exec "$node_bin" "$tmp_dir/fake-vz-launcher.cjs"
SH
chmod 755 "$tmp_dir/fake-vz-launcher"

request_json="$("$node_bin" - <<'NODE'
console.log(JSON.stringify({
  schema: "elastos.browser.engine.launch-request/v1",
  adapter: "browser-vm-product",
  engine: "chromium_microvm",
  stream_id: "stream:vm-autostart-smoke",
  lifecycle_generation: "sha256:stream:vm-autostart-smoke",
  url: "https://ela.city/",
  network_mode: "runtime_net_only",
  direct_network: false,
  wallet_injection: false,
  display_mode: "webrtc_remote_display",
  guarantee_level: "mechanism_microvm",
  profile: {
    schema: "elastos.browser.profile/v1",
    scope: "active_principal",
    storage: "principal_owned_profile_disk",
    storage_posture: "principal_owned_reset_scoped_unprotected",
    protected_storage: false,
    encrypted: false,
    recoverable: false,
    recovery: "not_recovery_kit_packaged",
    uri: "localhost://Users/0123456789ab/BrowserProfiles/default/profile.ext4",
    public_uri: "localhost://Users/self/BrowserProfiles/default/profile.ext4",
    profile_key: "profile-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    disk_path: "/tmp/elastos-browser-vm-autostart/BrowserProfiles/default/profile.ext4",
    reset: "whole_profile",
  },
  relay_ipc: {
    schema: "elastos.browser.relay-ipc/v1",
    kind: "unix_socket",
    path: "/tmp/elastos-browser-vm-autostart-relay.sock",
  },
}));
NODE
)"

request_json_two="$(BASE_REQUEST="$request_json" NEXT_STREAM="stream:vm-autostart-smoke-two" "$node_bin" -e '
const request = JSON.parse(process.env.BASE_REQUEST);
request.stream_id = process.env.NEXT_STREAM;
request.lifecycle_generation = `sha256:${request.stream_id}`;
process.stdout.write(JSON.stringify(request));
')"
request_json_three="$(BASE_REQUEST="$request_json" NEXT_STREAM="stream:vm-autostart-smoke-three" "$node_bin" -e '
const request = JSON.parse(process.env.BASE_REQUEST);
request.stream_id = process.env.NEXT_STREAM;
request.lifecycle_generation = `sha256:${request.stream_id}`;
process.stdout.write(JSON.stringify(request));
')"
transport_request_path="$tmp_dir/private-stdin-request.json"
(
  umask 077
  BASE_REQUEST="$request_json" "$node_bin" > "$transport_request_path" <<'NODE'
const crypto = require("node:crypto");

const canonicalJson = (value) => {
  if (Array.isArray(value)) return value.map(canonicalJson);
  if (value && typeof value === "object") {
    return Object.keys(value)
      .sort()
      .reduce((out, key) => {
        out[key] = canonicalJson(value[key]);
        return out;
      }, {});
  }
  return value;
};
const sha256Label = (value) =>
  `sha256:${crypto.createHash("sha256").update(value).digest("hex")}`;
const request = JSON.parse(process.env.BASE_REQUEST);
request.stream_id = "stream:vm-private-stdin-smoke";
request.lifecycle_generation = sha256Label(
  Buffer.from("vm-private-stdin-generation"),
);
request.principal_id = "person:local:vm-private-stdin-smoke";
request.target = "tls://private-stdin.invalid:443";
request.page_id = "page:vm-private-stdin-smoke";
request.vm_id = "vm:vm-private-stdin-smoke";
const expiresAtUnixMs = (Math.floor(Date.now() / 1000) + 300) * 1000;
const username = `${expiresAtUnixMs / 1000}:private-stdin-smoke`;
const authSecret = crypto.randomBytes(32).toString("base64url");
const credential = crypto
  .createHmac("sha1", authSecret)
  .update(username)
  .digest("base64");
const authority = {
  schema: "elastos.browser.vz-transport-authority/v1",
  generation: request.lifecycle_generation,
  page_id: request.page_id,
  vm_id: request.vm_id,
  principal_id: request.principal_id,
  egress: {
    schema: "elastos.browser.vz-transport-stream/v1",
    stream_id: request.stream_id,
    target: request.target,
    runtime_socket_path: "/tmp/vm-private-stdin-egress.sock",
    vsock_port: 19091,
  },
  media: {
    schema: "elastos.browser.vz-transport-stream/v1",
    stream_id: "stream:vm-private-stdin-media",
    target: "tcp://127.0.0.1:49961",
    runtime_socket_path: "/tmp/vm-private-stdin-media.sock",
    vsock_port: 19094,
  },
  turn: {
    schema: "elastos.browser.vz-turn-authority/v1",
    guest_url: "turn:127.0.0.1:3478?transport=tcp",
    guest_host: "127.0.0.1",
    guest_port: 3478,
    listen_host: "127.0.0.1",
    listen_port: 49961,
    advertised_host: "127.0.0.1",
    relay_host: "127.0.0.1",
    relay_port_min: 49962,
    relay_port_max: 49965,
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
request.transport_authority = authority;
request.transport_secret = {
  schema: "elastos.browser.vz-transport-secret/v1",
  binding_hash: authority.binding_hash,
  credential,
  auth_secret: authSecret,
};
process.stdout.write(JSON.stringify(request));
NODE
)

startup_lock="${control_socket}.start.lock"
printf '%s\n' '{}' > "$startup_lock"
python3 -c 'import os,sys,time; os.utime(sys.argv[1], (time.time()-30, time.time()-30))' "$startup_lock"

close_page() {
  local result_file="$1"
  local cleanup_body
  cleanup_body="$("$node_bin" - <<'NODE' "$result_file" "$control_socket"
const fs = require("node:fs");
const [resultPath, shutdownSocketPath] = process.argv.slice(2);
const page = JSON.parse(fs.readFileSync(resultPath, "utf8"));
process.stdout.write(JSON.stringify({
  page_id: page.page_id,
  force_retire_vm: true,
  runtime_cleanup: {
    schema: "elastos.browser.engine-cleanup-binding/v2",
    page_id: page.page_id,
    generation:
      page.transport_authority?.generation || `sha256:${page.stream_id}`,
    stream_id: page.stream_id,
    adapter: page.adapter,
    engine: page.engine,
    display_mode: "webrtc_remote_display",
    guarantee_level: "mechanism_microvm",
    principal_id: page.transport_authority?.principal_id || null,
    control_socket_path: page.control_socket_path,
    shutdown_socket_path: shutdownSocketPath,
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
  },
}));
NODE
)"
  local cleanup_response
  if ! cleanup_response="$(curl --fail-with-body --unix-socket "$control_socket" -sS --max-time 3 \
      -H 'Content-Type: application/json' \
      -d "$cleanup_body" \
      http://localhost/shutdown)"; then
    printf '%s\n' "$cleanup_response" >&2
    return 1
  fi
}

startup_lock_identity="$tmp_dir/startup-lock-identity.json"
"$node_bin" - <<'NODE' "$startup_lock" "$startup_lock_identity"
const fs = require("node:fs");
const [lockPath, identityPath] = process.argv.slice(2);
const stat = fs.lstatSync(lockPath);
fs.writeFileSync(identityPath, JSON.stringify({
  dev: stat.dev,
  ino: stat.ino,
  contents: fs.readFileSync(lockPath, "utf8"),
}));
NODE

set +e
ELASTOS_BROWSER_VM_PREWARM_CONTROL_SERVICE="1" \
ELASTOS_BROWSER_VM_CONTROL_SOCKET="$control_socket" \
ELASTOS_BROWSER_VM_CONTROL_SERVICE="$tmp_dir/control-service" \
ELASTOS_BROWSER_VM_CONTROL_LAUNCHER="$tmp_dir/fake-vz-launcher" \
ELASTOS_BROWSER_VM_CONTROL_LOG="$control_log" \
ELASTOS_BROWSER_VM_DATA_DIR="$data_dir" \
ELASTOS_BROWSER_VM_ROOT="$vm_root" \
ELASTOS_BROWSER_VM_AUTOSTART_PID_FILE="$launcher_pid_file" \
ELASTOS_BROWSER_VM_PLATFORM="darwin-arm64" \
ELASTOS_BROWSER_VM_CONTROL_MAX_ACTIVE_PAGES="1" \
ELASTOS_BROWSER_VM_IDLE_KEEPALIVE_MS="45000" \
ELASTOS_BROWSER_VM_ICE_SERVERS_JSON='[{"urls":["turn:192.0.2.10:3478?transport=udp"],"username":"first-turn","credential":"first-secret"}]' \
  "$node_bin" "$repo_root/scripts/browser-vm-engine-supervisor.mjs" \
    > "$tmp_dir/prewarm-blocked.json" 2> "$tmp_dir/prewarm-blocked.err"
blocked_status=$?
set -e
if [[ "$blocked_status" -eq 0 ]] ||
   ! grep -q '"code":"control_service_startup_busy"' "$tmp_dir/prewarm-blocked.err"; then
  cat "$tmp_dir/prewarm-blocked.err" >&2 || true
  echo "existing startup lock did not fail closed" >&2
  exit 1
fi
"$node_bin" - <<'NODE' "$startup_lock" "$startup_lock_identity"
const fs = require("node:fs");
const [lockPath, identityPath] = process.argv.slice(2);
const expected = JSON.parse(fs.readFileSync(identityPath, "utf8"));
const stat = fs.lstatSync(lockPath);
if (
  stat.dev !== expected.dev ||
  stat.ino !== expected.ino ||
  fs.readFileSync(lockPath, "utf8") !== expected.contents
) {
  throw new Error("fail-closed startup changed the existing lock identity");
}
NODE
rm -f "$startup_lock"

ELASTOS_BROWSER_VM_PREWARM_CONTROL_SERVICE="1" \
ELASTOS_BROWSER_VM_CONTROL_SOCKET="$control_socket" \
ELASTOS_BROWSER_VM_CONTROL_SERVICE="$tmp_dir/control-service" \
ELASTOS_BROWSER_VM_CONTROL_LAUNCHER="$tmp_dir/fake-vz-launcher" \
ELASTOS_BROWSER_VM_CONTROL_LOG="$control_log" \
ELASTOS_BROWSER_VM_DATA_DIR="$data_dir" \
ELASTOS_BROWSER_VM_ROOT="$vm_root" \
ELASTOS_BROWSER_VM_AUTOSTART_PID_FILE="$launcher_pid_file" \
ELASTOS_BROWSER_VM_PLATFORM="darwin-arm64" \
ELASTOS_BROWSER_VM_CONTROL_MAX_ACTIVE_PAGES="1" \
ELASTOS_BROWSER_VM_IDLE_KEEPALIVE_MS="45000" \
ELASTOS_BROWSER_VM_ICE_SERVERS_JSON='[{"urls":["turn:192.0.2.10:3478?transport=udp"],"username":"first-turn","credential":"first-secret"}]' \
  "$node_bin" "$repo_root/scripts/browser-vm-engine-supervisor.mjs" > "$tmp_dir/prewarm.json"
if [[ -e "$startup_lock" ]]; then
  echo "startup lock was not released after successful acquisition" >&2
  exit 1
fi

"$node_bin" - <<'NODE' "$tmp_dir/prewarm.json" "$control_socket"
const fs = require("node:fs");
const [prewarmPath, socketPath] = process.argv.slice(2);
const result = JSON.parse(fs.readFileSync(prewarmPath, "utf8"));
if (result.schema !== "elastos.browser.vm-engine-prewarm/v1" || result.ok !== true) throw new Error("wrong prewarm schema");
if (result.control_socket_path !== socketPath) throw new Error("prewarm returned wrong control socket");
if (result.control_status?.max_active_pages !== 1) throw new Error("prewarm did not configure max active pages");
if (result.control_status?.idle_vm_keepalive_ms !== 45000) throw new Error("prewarm did not configure idle VM keepalive");
if (result.control_status?.hibernation_mode !== "off") throw new Error("prewarm should not claim hibernation unless enabled");
if (result.network_mode !== "runtime_net_only" || result.direct_network !== false) throw new Error("prewarm leaked network authority");
NODE

env -u ELASTOS_BROWSER_ENGINE_REQUEST \
    -u ELASTOS_BROWSER_VM_PREWARM_CONTROL_SERVICE \
    ELASTOS_BROWSER_VM_CONTROL_SOCKET="$control_socket" \
    ELASTOS_BROWSER_VM_CONTROL_SERVICE="$tmp_dir/control-service" \
    ELASTOS_BROWSER_VM_CONTROL_LAUNCHER="$tmp_dir/fake-vz-launcher" \
    ELASTOS_BROWSER_VM_CONTROL_LOG="$control_log" \
    ELASTOS_BROWSER_VM_DATA_DIR="$data_dir" \
    ELASTOS_BROWSER_VM_ROOT="$vm_root" \
    ELASTOS_BROWSER_VM_AUTOSTART_PID_FILE="$launcher_pid_file" \
    ELASTOS_BROWSER_VM_PLATFORM="darwin-arm64" \
    ELASTOS_BROWSER_VM_CONTROL_MAX_ACTIVE_PAGES="1" \
    ELASTOS_BROWSER_VM_IDLE_KEEPALIVE_MS="45000" \
    ELASTOS_BROWSER_VM_ICE_SERVERS_JSON='[{"urls":["turn:192.0.2.10:3478?transport=udp"],"username":"first-turn","credential":"first-secret"}]' \
    "$node_bin" "$repo_root/scripts/browser-vm-engine-supervisor.mjs" \
    < "$transport_request_path" \
    > "$tmp_dir/private-stdin-result.json"

"$node_bin" - <<'NODE' \
  "$tmp_dir/private-stdin-result.json" \
  "$control_log" \
  "$transport_request_path"
const fs = require("node:fs");
const [resultPath, logPath, requestPath] = process.argv.slice(2);
const result = JSON.parse(fs.readFileSync(resultPath, "utf8"));
const request = JSON.parse(fs.readFileSync(requestPath, "utf8"));
if ((fs.statSync(requestPath).mode & 0o777) !== 0o600) {
  throw new Error("private stdin request fixture is not owner-only");
}
if (
  result.page_id !== request.page_id ||
  result.vm_id !== request.vm_id ||
  JSON.stringify(result.transport_authority) !==
    JSON.stringify(request.transport_authority) ||
  result.transport_receipt?.binding_hash !==
    request.transport_authority.binding_hash ||
  result.transport_receipt?.generation !== request.lifecycle_generation ||
  result.transport_receipt?.terminal !== true
) {
  throw new Error("private stdin launch did not preserve the exact VZ binding");
}
if (
  result.display_session?.ice_connection_policy !== "engine_relay_only" ||
  Object.prototype.hasOwnProperty.call(
    result.display_session || {},
    "ice_servers",
  )
) {
  throw new Error("private stdin launch exposed a non-engine-owned ICE policy");
}
const evidence = `${fs.readFileSync(resultPath, "utf8")}\n${fs.readFileSync(logPath, "utf8")}`;
for (const secret of [
  request.transport_secret.credential,
  request.transport_secret.auth_secret,
]) {
  if (evidence.includes(secret)) {
    throw new Error("private VZ transport secret reached result or control log");
  }
}
NODE

close_page "$tmp_dir/private-stdin-result.json"

printf '%s\n' "$request_json" | \
  ELASTOS_BROWSER_ENGINE_REQUEST="$request_json" \
  ELASTOS_BROWSER_VM_PREWARM_CONTROL_SERVICE="1" \
  ELASTOS_BROWSER_VM_CONTROL_SOCKET="$control_socket" \
  ELASTOS_BROWSER_VM_CONTROL_SERVICE="$tmp_dir/control-service" \
  ELASTOS_BROWSER_VM_CONTROL_LAUNCHER="$tmp_dir/fake-vz-launcher" \
  ELASTOS_BROWSER_VM_CONTROL_LOG="$control_log" \
  ELASTOS_BROWSER_VM_DATA_DIR="$data_dir" \
  ELASTOS_BROWSER_VM_ROOT="$vm_root" \
  ELASTOS_BROWSER_VM_AUTOSTART_PID_FILE="$launcher_pid_file" \
  ELASTOS_BROWSER_VM_PLATFORM="darwin-arm64" \
  ELASTOS_BROWSER_VM_CONTROL_MAX_ACTIVE_PAGES="1" \
  ELASTOS_BROWSER_VM_IDLE_KEEPALIVE_MS="45000" \
  ELASTOS_BROWSER_VM_ICE_SERVERS_JSON='[{"urls":["turn:192.0.2.10:3478?transport=udp"],"username":"first-turn","credential":"first-secret"}]' \
  "$node_bin" "$repo_root/scripts/browser-vm-engine-supervisor.mjs" > "$tmp_dir/result.json"

status_one="$tmp_dir/status-one.json"
curl --unix-socket "$control_socket" -sS --max-time 3 \
  http://localhost/status > "$status_one"

"$node_bin" - <<'NODE' "$tmp_dir/result.json" "$control_log" "$control_socket" "$status_one" "$tmp_dir/prewarm.json"
const fs = require("node:fs");
const [resultPath, logPath, socketPath, statusPath, prewarmPath] = process.argv.slice(2);
const result = JSON.parse(fs.readFileSync(resultPath, "utf8"));
const prewarm = JSON.parse(fs.readFileSync(prewarmPath, "utf8"));
if (result.schema !== "elastos.browser.engine.supervisor-result/v1") throw new Error("wrong result schema");
if (result.page_id !== "page:vm-autostart-smoke") throw new Error("wrong page id");
if (result.engine !== "chromium_microvm") throw new Error("wrong engine");
if (result.network_mode !== "runtime_net_only" || result.direct_network !== false) throw new Error("wrong network authority");
if (result.display_session?.media_transport !== "runtime_relay") throw new Error("display is not Runtime-relayed");
if (!JSON.stringify(result.display_session?.ice_servers || []).includes("first-turn")) throw new Error("first control service did not inherit first ICE config");
if (!fs.existsSync(socketPath)) throw new Error("control socket was not created");
const status = JSON.parse(fs.readFileSync(statusPath, "utf8"));
if (!Number.isInteger(status.pid) || status.pid <= 1) throw new Error("control service status must expose pid");
if (!Date.parse(status.started_at || "")) throw new Error("control service status must expose started_at");
if (!Number.isFinite(Number(status.uptime_ms)) || Number(status.uptime_ms) < 0) throw new Error("control service status must expose uptime_ms");
if (!/^[0-9a-f]{64}$/.test(status.config_fingerprint || "")) throw new Error("control service status must expose config fingerprint");
if (status.idle_vm_keepalive_ms !== 45000) throw new Error("control service status must expose idle VM keepalive");
if (status.hibernation_mode !== "off") throw new Error("control service status must expose hibernation mode");
if (status.pid !== prewarm.control_status?.pid) throw new Error("real launch replaced the canonical prewarmed control service");
const log = fs.readFileSync(logPath, "utf8");
if (!log.includes("elastos.browser.vm-control-service.ready/v1")) throw new Error("control service did not start");
if (!log.includes("\"event\":\"launch_ready\"")) throw new Error("control service did not launch page");
NODE

set +e
printf '%s\n' "$request_json_two" | \
  ELASTOS_BROWSER_ENGINE_REQUEST="$request_json_two" \
  ELASTOS_BROWSER_VM_CONTROL_SOCKET="$control_socket" \
  ELASTOS_BROWSER_VM_CONTROL_SERVICE="$tmp_dir/control-service" \
  ELASTOS_BROWSER_VM_CONTROL_LAUNCHER="$tmp_dir/fake-vz-launcher" \
  ELASTOS_BROWSER_VM_CONTROL_LOG="$control_log" \
  ELASTOS_BROWSER_VM_DATA_DIR="$data_dir" \
  ELASTOS_BROWSER_VM_ROOT="$vm_root" \
  ELASTOS_BROWSER_VM_AUTOSTART_PID_FILE="$launcher_pid_file" \
  ELASTOS_BROWSER_VM_PLATFORM="darwin-arm64" \
  ELASTOS_BROWSER_VM_CONTROL_MAX_ACTIVE_PAGES="1" \
  ELASTOS_BROWSER_VM_IDLE_KEEPALIVE_MS="45000" \
  ELASTOS_BROWSER_VM_ICE_SERVERS_JSON='[{"urls":["turn:192.0.2.20:3478?transport=udp"],"username":"second-turn","credential":"second-secret"}]' \
  "$node_bin" "$repo_root/scripts/browser-vm-engine-supervisor.mjs" \
  > "$tmp_dir/active-replacement.out" 2> "$tmp_dir/active-replacement.err"
active_replacement_status="$?"
set -e
if [[ "$active_replacement_status" -eq 0 ]]; then
  echo "mismatched control service was replaced while it owned an active VM" >&2
  exit 1
fi
"$node_bin" - <<'NODE' "$tmp_dir/active-replacement.err" "$status_one" "$control_socket"
const fs = require("node:fs");
const http = require("node:http");
const [errorPath, statusPath, socketPath] = process.argv.slice(2);
const lines = fs.readFileSync(errorPath, "utf8").trim().split(/\r?\n/);
const error = JSON.parse(lines.at(-1));
if (error.schema !== "elastos.browser.engine.launch-error/v1" || error.code !== "resources_in_use") {
  throw new Error(`active replacement did not return typed resources_in_use: ${JSON.stringify(error)}`);
}
const expected = JSON.parse(fs.readFileSync(statusPath, "utf8"));
const request = http.request({ socketPath, path: "/status", method: "GET" }, (response) => {
  const chunks = [];
  response.on("data", (chunk) => chunks.push(chunk));
  response.on("end", () => {
    const current = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    if (current.pid !== expected.pid || current.active_pages !== 1) {
      throw new Error(`active generation changed during replacement rejection: ${JSON.stringify(current)}`);
    }
  });
});
request.on("error", (error) => { throw error; });
request.end();
NODE

close_page "$tmp_dir/result.json"

printf '%s\n' "$request_json_two" | \
  ELASTOS_BROWSER_ENGINE_REQUEST="$request_json_two" \
  ELASTOS_BROWSER_VM_CONTROL_SOCKET="$control_socket" \
  ELASTOS_BROWSER_VM_CONTROL_SERVICE="$tmp_dir/control-service" \
  ELASTOS_BROWSER_VM_CONTROL_LAUNCHER="$tmp_dir/fake-vz-launcher" \
  ELASTOS_BROWSER_VM_CONTROL_LOG="$control_log" \
  ELASTOS_BROWSER_VM_DATA_DIR="$data_dir" \
  ELASTOS_BROWSER_VM_ROOT="$vm_root" \
  ELASTOS_BROWSER_VM_AUTOSTART_PID_FILE="$launcher_pid_file" \
  ELASTOS_BROWSER_VM_PLATFORM="darwin-arm64" \
  ELASTOS_BROWSER_VM_CONTROL_MAX_ACTIVE_PAGES="1" \
  ELASTOS_BROWSER_VM_IDLE_KEEPALIVE_MS="45000" \
  ELASTOS_BROWSER_VM_ICE_SERVERS_JSON='[{"urls":["turn:192.0.2.20:3478?transport=udp"],"username":"second-turn","credential":"second-secret"}]' \
  "$node_bin" "$repo_root/scripts/browser-vm-engine-supervisor.mjs" > "$tmp_dir/result-two.json"

status_two="$tmp_dir/status-two.json"
curl --unix-socket "$control_socket" -sS --max-time 3 \
  http://localhost/status > "$status_two"

"$node_bin" - <<'NODE' "$tmp_dir/result-two.json" "$status_one" "$status_two"
const fs = require("node:fs");
const [resultPath, statusOnePath, statusTwoPath] = process.argv.slice(2);
const result = JSON.parse(fs.readFileSync(resultPath, "utf8"));
const statusOne = JSON.parse(fs.readFileSync(statusOnePath, "utf8"));
const statusTwo = JSON.parse(fs.readFileSync(statusTwoPath, "utf8"));
if (!JSON.stringify(result.display_session?.ice_servers || []).includes("second-turn")) {
  throw new Error("second control service did not inherit changed ICE config");
}
if (statusOne.config_fingerprint === statusTwo.config_fingerprint) {
  throw new Error("changed VM control config must restart with a new fingerprint");
}
if (statusOne.pid === statusTwo.pid) {
  throw new Error("changed VM control config must replace the stale control service process");
}
NODE

close_page "$tmp_dir/result-two.json"

printf '%s\n' '# source-home wrapper revision for stale-process proof' >> "$tmp_dir/control-service"

printf '%s\n' "$request_json_three" | \
  ELASTOS_BROWSER_ENGINE_REQUEST="$request_json_three" \
  ELASTOS_BROWSER_VM_CONTROL_SOCKET="$control_socket" \
  ELASTOS_BROWSER_VM_CONTROL_SERVICE="$tmp_dir/control-service" \
  ELASTOS_BROWSER_VM_CONTROL_LAUNCHER="$tmp_dir/fake-vz-launcher" \
  ELASTOS_BROWSER_VM_CONTROL_LOG="$control_log" \
  ELASTOS_BROWSER_VM_DATA_DIR="$data_dir" \
  ELASTOS_BROWSER_VM_ROOT="$vm_root" \
  ELASTOS_BROWSER_VM_AUTOSTART_PID_FILE="$launcher_pid_file" \
  ELASTOS_BROWSER_VM_PLATFORM="darwin-arm64" \
  ELASTOS_BROWSER_VM_CONTROL_MAX_ACTIVE_PAGES="1" \
  ELASTOS_BROWSER_VM_IDLE_KEEPALIVE_MS="45000" \
  ELASTOS_BROWSER_VM_ICE_SERVERS_JSON='[{"urls":["turn:192.0.2.20:3478?transport=udp"],"username":"second-turn","credential":"second-secret"}]' \
  "$node_bin" "$repo_root/scripts/browser-vm-engine-supervisor.mjs" > "$tmp_dir/result-three.json"

status_three="$tmp_dir/status-three.json"
curl --unix-socket "$control_socket" -sS --max-time 3 \
  http://localhost/status > "$status_three"

"$node_bin" - <<'NODE' "$status_two" "$status_three"
const fs = require("node:fs");
const [statusTwoPath, statusThreePath] = process.argv.slice(2);
const statusTwo = JSON.parse(fs.readFileSync(statusTwoPath, "utf8"));
const statusThree = JSON.parse(fs.readFileSync(statusThreePath, "utf8"));
if (statusTwo.config_fingerprint === statusThree.config_fingerprint) {
  throw new Error("changed VM control helper contents must restart with a new fingerprint");
}
if (statusTwo.pid === statusThree.pid) {
  throw new Error("changed VM control helper contents must replace the stale control service process");
}
NODE

close_page "$tmp_dir/result-three.json"
service_pid_three="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$status_three")"
kill -TERM "$service_pid_three"
for _ in {1..100}; do
  [[ ! -S "$control_socket" ]] && break
  sleep 0.05
done
if [[ -S "$control_socket" ]]; then
  echo "owned control service did not remove its socket during shutdown" >&2
  exit 1
fi

sleep 30 &
foreign_target_pid="$!"
cat > "$tmp_dir/foreign-control-service.mjs" <<'NODE'
import fs from "node:fs";
import http from "node:http";
const socketPath = process.env.FOREIGN_CONTROL_SOCKET;
try { fs.unlinkSync(socketPath); } catch {}
const server = http.createServer((request, response) => {
  if (request.method === "GET" && request.url === "/status") {
    response.writeHead(200, { "content-type": "application/json" });
    response.end(JSON.stringify({
      schema: "elastos.browser.vm-control-service.status/v1",
      ok: true,
      pid: Number(process.env.FOREIGN_TARGET_PID),
      started_at: "2026-07-27T00:00:00.000Z",
      config_fingerprint: "f".repeat(64),
      active_pages: 0,
      active_vms: 0,
      warm_vms: 0,
      pending_launches: 0,
    }));
    return;
  }
  response.writeHead(404, { "content-type": "application/json" });
  response.end(JSON.stringify({ error: "foreign service has no owned shutdown contract" }));
});
server.listen(socketPath);
NODE
FOREIGN_CONTROL_SOCKET="$control_socket" FOREIGN_TARGET_PID="$foreign_target_pid" \
  "$node_bin" "$tmp_dir/foreign-control-service.mjs" &
foreign_server_pid="$!"
for _ in {1..100}; do
  [[ -S "$control_socket" ]] && break
  sleep 0.05
done
if [[ ! -S "$control_socket" ]]; then
  echo "foreign substitution fixture did not create its socket" >&2
  exit 1
fi

set +e
printf '%s\n' "$request_json" | \
  ELASTOS_BROWSER_ENGINE_REQUEST="$request_json" \
  ELASTOS_BROWSER_VM_CONTROL_SOCKET="$control_socket" \
  ELASTOS_BROWSER_VM_CONTROL_SERVICE="$tmp_dir/control-service" \
  ELASTOS_BROWSER_VM_CONTROL_LAUNCHER="$tmp_dir/fake-vz-launcher" \
  ELASTOS_BROWSER_VM_CONTROL_LOG="$control_log" \
  ELASTOS_BROWSER_VM_DATA_DIR="$data_dir" \
  ELASTOS_BROWSER_VM_ROOT="$vm_root" \
  ELASTOS_BROWSER_VM_AUTOSTART_PID_FILE="$launcher_pid_file" \
  ELASTOS_BROWSER_VM_PLATFORM="darwin-arm64" \
  ELASTOS_BROWSER_VM_CONTROL_MAX_ACTIVE_PAGES="1" \
  ELASTOS_BROWSER_VM_IDLE_KEEPALIVE_MS="45000" \
  ELASTOS_BROWSER_VM_ICE_SERVERS_JSON='[{"urls":["turn:192.0.2.20:3478?transport=udp"],"username":"second-turn","credential":"second-secret"}]' \
  "$node_bin" "$repo_root/scripts/browser-vm-engine-supervisor.mjs" \
  > "$tmp_dir/foreign-substitution.out" 2> "$tmp_dir/foreign-substitution.err"
foreign_substitution_status="$?"
set -e
if [[ "$foreign_substitution_status" -eq 0 ]]; then
  echo "foreign control-service substitution was accepted" >&2
  exit 1
fi
"$node_bin" - <<'NODE' "$tmp_dir/foreign-substitution.err"
const fs = require("node:fs");
const lines = fs.readFileSync(process.argv[2], "utf8").trim().split(/\r?\n/);
const error = JSON.parse(lines.at(-1));
if (
  error.schema !== "elastos.browser.engine.launch-error/v1" ||
  error.code !== "control_service_substitution"
) {
  throw new Error(`foreign substitution did not fail with a typed rejection: ${JSON.stringify(error)}`);
}
NODE
if ! kill -0 "$foreign_target_pid" >/dev/null 2>&1; then
  echo "foreign status PID was signaled during substitution rejection" >&2
  exit 1
fi
if ! kill -0 "$foreign_server_pid" >/dev/null 2>&1 || [[ ! -S "$control_socket" ]]; then
  echo "foreign control socket was unlinked or replaced" >&2
  exit 1
fi

kill "$foreign_server_pid"
wait "$foreign_server_pid" 2>/dev/null || true
foreign_server_pid=""
rm "$control_socket"

cat > "$tmp_dir/unresponsive-control-service.mjs" <<'NODE'
import net from "node:net";
const server = net.createServer((socket) => {
  socket.on("error", () => {});
});
server.listen(process.env.UNRESPONSIVE_CONTROL_SOCKET);
NODE
UNRESPONSIVE_CONTROL_SOCKET="$control_socket" \
  "$node_bin" "$tmp_dir/unresponsive-control-service.mjs" &
unresponsive_server_pid="$!"
for _ in {1..100}; do
  [[ -S "$control_socket" ]] && break
  sleep 0.05
done
if [[ ! -S "$control_socket" ]]; then
  echo "unresponsive control-service fixture did not create its socket" >&2
  exit 1
fi

set +e
printf '%s\n' "$request_json" | \
  ELASTOS_BROWSER_ENGINE_REQUEST="$request_json" \
  ELASTOS_BROWSER_VM_CONTROL_SOCKET="$control_socket" \
  ELASTOS_BROWSER_VM_CONTROL_SERVICE="$tmp_dir/control-service" \
  ELASTOS_BROWSER_VM_CONTROL_LAUNCHER="$tmp_dir/fake-vz-launcher" \
  ELASTOS_BROWSER_VM_CONTROL_LOG="$control_log" \
  ELASTOS_BROWSER_VM_DATA_DIR="$data_dir" \
  ELASTOS_BROWSER_VM_ROOT="$vm_root" \
  ELASTOS_BROWSER_VM_AUTOSTART_PID_FILE="$launcher_pid_file" \
  ELASTOS_BROWSER_VM_PLATFORM="darwin-arm64" \
  ELASTOS_BROWSER_VM_CONTROL_MAX_ACTIVE_PAGES="1" \
  ELASTOS_BROWSER_VM_IDLE_KEEPALIVE_MS="45000" \
  "$node_bin" "$repo_root/scripts/browser-vm-engine-supervisor.mjs" \
  > "$tmp_dir/unresponsive.out" 2> "$tmp_dir/unresponsive.err"
unresponsive_status="$?"
set -e
if [[ "$unresponsive_status" -eq 0 ]]; then
  echo "unresponsive control-service substitution was accepted" >&2
  exit 1
fi
"$node_bin" - <<'NODE' "$tmp_dir/unresponsive.err"
const fs = require("node:fs");
const lines = fs.readFileSync(process.argv[2], "utf8").trim().split(/\r?\n/);
const error = JSON.parse(lines.at(-1));
if (
  error.schema !== "elastos.browser.engine.launch-error/v1" ||
  error.code !== "control_service_unverified"
) {
  throw new Error(`unresponsive service did not fail closed: ${JSON.stringify(error)}`);
}
NODE
if ! kill -0 "$unresponsive_server_pid" >/dev/null 2>&1 || [[ ! -S "$control_socket" ]]; then
  echo "unresponsive control socket was unlinked or replaced" >&2
  exit 1
fi

printf '%s\n' '{"schema":"elastos.browser.vm-engine-supervisor-autostart-smoke/v1","ok":true}'
