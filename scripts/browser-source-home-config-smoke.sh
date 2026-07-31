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

cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

cd "$repo_root"

node_bin="$(find_node || true)"
if [[ -z "$node_bin" ]]; then
  echo "node not found. Install Node or set ELASTOS_NODE_BIN to an executable node binary." >&2
  exit 2
fi

rejected_output="$tmp_dir/rejected.out"
if "$node_bin" scripts/browser-source-home-config.mjs \
  --data-dir "$tmp_dir/rejected-data" \
  --platform linux-amd64 \
  --out-dir "$tmp_dir/rejected-config" \
  --engine-mode hosted-proof >"$rejected_output" 2>&1; then
  cat "$rejected_output"
  echo "expected hosted-proof mode to be rejected by source-home Browser config" >&2
  exit 1
fi
grep -q -- "unknown option: --engine-mode" "$rejected_output"

ELASTOS_BROWSER_VM_ICE_SERVER="turn:turn.example.invalid:3478?transport=tcp" \
ELASTOS_BROWSER_VM_ICE_USERNAME="source-home-user" \
ELASTOS_BROWSER_VM_ICE_CREDENTIAL="source-home-secret" \
ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY="relay" \
ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4="10.44.0.1" \
ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4="10.44.0.2" \
ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX="24" \
ELASTOS_BROWSER_VM_TRACE="1" \
ELASTOS_BROWSER_VM_CONTROL_STATUS_PROBE_TIMEOUT_MS="3000" \
ELASTOS_BROWSER_VM_DEBUG_HOLD_ON_OPEN_ERROR_MS="60000" \
ELASTOS_BROWSER_VM_TURNSERVER_BIN="/tmp/elastos-test-turnserver" \
  "$node_bin" scripts/browser-source-home-config.mjs \
    --data-dir "$tmp_dir/vm-data" \
    --platform linux-amd64 \
    --out-dir "$tmp_dir/vm-config" \
    --vm-control-socket "$tmp_dir/vm-control.sock" >/dev/null

ELASTOS_BROWSER_VM_ICE_SERVERS_JSON='[{"urls":["turn:192.168.65.1:3478?transport=udp"],"username":"mac-user","credential":"mac-secret"}]' \
ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY="relay" \
ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4="192.168.65.1" \
ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4="192.168.65.2" \
ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX="24" \
ELASTOS_BROWSER_VM_TURNSERVER_BIN="/tmp/elastos-local-turnserver" \
ELASTOS_BROWSER_REMOTE_VZ_TURN_PROGRAM="/opt/homebrew/bin/turnserver" \
ELASTOS_BROWSER_VZ_TURN_ADVERTISED_HOST="192.168.65.1" \
ELASTOS_BROWSER_VZ_TURN_RELAY_HOST="192.168.65.1" \
"$node_bin" scripts/browser-source-home-config.mjs \
  --data-dir "$tmp_dir/mac-vm-data" \
  --platform darwin-arm64 \
  --out-dir "$tmp_dir/mac-vm-config" \
  --vm-control-socket "$tmp_dir/mac-vm-control.sock" \
  --vm-control-launcher "$tmp_dir/browser-vm-remote-vz-launcher" >/dev/null

ELASTOS_BROWSER_VM_ICE_SERVER="turn:viewer.example.invalid:3478?transport=tcp" \
ELASTOS_BROWSER_VM_ICE_USERNAME="viewer-user" \
ELASTOS_BROWSER_VM_ICE_CREDENTIAL="viewer-secret" \
ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY="relay" \
ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4="10.99.0.1" \
ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4="10.99.0.2" \
ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX="24" \
ELASTOS_BROWSER_VM_TURNSERVER_BIN="/tmp/elastos-local-turnserver" \
ELASTOS_BROWSER_REMOTE_VZ_DATA_DIR="$tmp_dir/mac-provider-data" \
ELASTOS_BROWSER_REMOTE_VZ_TURN_PROGRAM="/opt/homebrew/bin/turnserver" \
ELASTOS_BROWSER_VZ_TURN_ADVERTISED_HOST="192.168.65.1" \
ELASTOS_BROWSER_VZ_TURN_RELAY_HOST="192.168.65.1" \
"$node_bin" scripts/browser-source-home-config.mjs \
  --data-dir "$tmp_dir/linux-remote-vz-data" \
  --platform linux-amd64 \
  --out-dir "$tmp_dir/linux-remote-vz-config" \
  --vm-control-launcher "$tmp_dir/browser-vm-remote-vz-launcher" >/dev/null

"$node_bin" scripts/browser-source-home-config.mjs \
  --data-dir "$tmp_dir/default-mac-data" \
  --platform darwin-arm64 \
  --out-dir "$tmp_dir/default-mac-config" >/dev/null

mkdir -p "$tmp_dir/preserve-config"
cat > "$tmp_dir/preserve-config/exit-provider.json" <<'JSON'
{
  "timeout_secs": 10,
  "backends": [],
  "remote_carrier_exits": [{
    "id": "seed-node",
    "grant_id": "operator-grant:seed-node:mac",
    "peer_did": "did:key:z6Mkseed",
    "carrier_service": "elastos://exit/open_stream",
    "connect_ticket": "private-ticket-redacted-in-reports",
    "allowed_principals": ["person:local:test"],
    "allowed_hosts": ["*"],
    "allowed_schemes": ["tcp", "tls"],
    "allowed_ports": [80, 443],
    "max_active_streams": 2,
    "max_active_streams_per_principal": 1
  }]
}
JSON
preserve_out="$tmp_dir/preserve-config.out"
"$node_bin" scripts/browser-source-home-config.mjs \
  --data-dir "$tmp_dir/preserve-data" \
  --platform darwin-arm64 \
  --out-dir "$tmp_dir/preserve-config" >"$preserve_out"

mkdir -p "$tmp_dir/turn-home/runtime-turn"
cat > "$tmp_dir/turn-home/runtime-turn/turn-credentials.env" <<'EOF'
ELASTOS_BROWSER_VM_ICE_SERVERS_JSON=[{"urls":["turn:192.168.66.1:3478?transport=udp","turn:192.168.66.1:3478?transport=tcp"],"username":"runtime-turn-user","credential":"runtime-turn-secret"}]
ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY=relay
ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4=192.168.66.1
ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4=192.168.66.2
ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX=24
EOF

HOME="$tmp_dir/turn-home" \
"$node_bin" scripts/browser-source-home-config.mjs \
  --data-dir "$tmp_dir/turn-data" \
  --platform darwin-arm64 \
  --out-dir "$tmp_dir/turn-config" >/dev/null

mkdir -p "$tmp_dir/linux-turn-data/runtime-turn"
cat > "$tmp_dir/linux-turn-data/runtime-turn/turn-credentials.env" <<'EOF'
ELASTOS_BROWSER_VM_ICE_SERVERS_JSON=[{"urls":["turn:{host_ip}:{turn_port}?transport=udp","turn:{host_ip}:{turn_port}?transport=tcp"],"username":"linux-runtime-turn-user","credential":"linux-runtime-turn-secret"}]
ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY=relay
EOF

"$node_bin" scripts/browser-source-home-config.mjs \
  --data-dir "$tmp_dir/linux-turn-data" \
  --platform linux-arm64 \
  --out-dir "$tmp_dir/linux-turn-config" >/dev/null

"$node_bin" - "$tmp_dir/vm-config/browser-engine-adapter.json" "$tmp_dir/vm-config/exit-provider.json" "$tmp_dir/vm-config/browser-local-exit.json" "$tmp_dir/mac-vm-config/browser-engine-adapter.json" "$tmp_dir/mac-vm-config/browser-vz-vsock-transport.json" "$tmp_dir/linux-remote-vz-config/browser-engine-adapter.json" "$tmp_dir/linux-remote-vz-config/browser-vz-vsock-transport.json" "$tmp_dir/default-mac-config/browser-engine-adapter.json" "$tmp_dir/default-mac-config/browser-vz-vsock-transport.json" "$tmp_dir/turn-config/browser-engine-adapter.json" "$tmp_dir/linux-turn-config/browser-engine-adapter.json" "$tmp_dir/preserve-config/exit-provider.json" "$preserve_out" <<'NODE'
const fs = require("node:fs");
const [vmAdapterPath, vmExitPath, vmLocalExitPath, macVmAdapterPath, macVzTransportPath, linuxRemoteVmAdapterPath, linuxRemoteVzTransportPath, defaultMacVmAdapterPath, defaultMacVzTransportPath, turnAdapterPath, linuxTurnAdapterPath, preserveExitPath, preserveOutPath] = process.argv.slice(2);
const vmAdapter = JSON.parse(fs.readFileSync(vmAdapterPath, "utf8"));
const vmExit = JSON.parse(fs.readFileSync(vmExitPath, "utf8"));
const vmLocalExit = JSON.parse(fs.readFileSync(vmLocalExitPath, "utf8"));
const macVmAdapter = JSON.parse(fs.readFileSync(macVmAdapterPath, "utf8"));
const macVzTransport = JSON.parse(fs.readFileSync(macVzTransportPath, "utf8"));
const linuxRemoteVmAdapter = JSON.parse(fs.readFileSync(linuxRemoteVmAdapterPath, "utf8"));
const linuxRemoteVzTransport = JSON.parse(fs.readFileSync(linuxRemoteVzTransportPath, "utf8"));
const defaultMacVmAdapter = JSON.parse(fs.readFileSync(defaultMacVmAdapterPath, "utf8"));
const defaultMacVzTransport = JSON.parse(fs.readFileSync(defaultMacVzTransportPath, "utf8"));
const turnAdapter = JSON.parse(fs.readFileSync(turnAdapterPath, "utf8"));
const linuxTurnAdapter = JSON.parse(fs.readFileSync(linuxTurnAdapterPath, "utf8"));
const preserveExit = JSON.parse(fs.readFileSync(preserveExitPath, "utf8"));
const preserveReceipt = JSON.parse(fs.readFileSync(preserveOutPath, "utf8"));
function assertNoRemoteVzLocalTurnEnv(env, label) {
  for (const key of [
    "ELASTOS_BROWSER_VM_ICE_SERVER",
    "ELASTOS_BROWSER_VM_ICE_SERVERS_JSON",
    "ELASTOS_BROWSER_VM_ICE_USERNAME",
    "ELASTOS_BROWSER_VM_ICE_CREDENTIAL",
    "ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY",
    "ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4",
    "ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4",
    "ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX",
    "ELASTOS_BROWSER_VM_TURNSERVER_BIN",
  ]) {
    if (Object.prototype.hasOwnProperty.call(env || {}, key)) {
      throw new Error(`${label} must not inherit local ${key}`);
    }
  }
}
const adapter = vmAdapter.adapters?.[0];
if (vmAdapter.max_active_sessions !== 4) {
  throw new Error("VM Browser source-home config must advertise multiple isolated Browser VM sessions");
}
if (adapter?.id !== "browser-vm-product") throw new Error("wrong VM adapter id");
if (adapter?.kind !== "chromium_microvm") throw new Error("wrong VM engine");
if (adapter?.network_mode !== "runtime_net_only") throw new Error("VM Browser must use Runtime-only networking");
if (JSON.stringify(adapter?.display_modes) !== JSON.stringify(["webrtc_remote_display"])) {
  throw new Error("VM Browser source-home config must advertise only the WebRTC product display mode");
}
if (Object.prototype.hasOwnProperty.call(vmAdapter, "preferred_display_mode")) {
  throw new Error("VM Browser adapter config must not include non-provider preferred_display_mode fields");
}
const expectedVmControlSocket = vmAdapterPath.replace(/\/vm-config\/browser-engine-adapter\.json$/, "/vm-control.sock");
if (adapter?.supervisor?.control_socket_path !== expectedVmControlSocket) {
  throw new Error("VM adapter supervisor must expose global control_socket_path for shutdown");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_DATA_DIR !== vmAdapterPath.replace(/\/vm-config\/browser-engine-adapter\.json$/, "/vm-data")) {
  throw new Error("VM adapter must pass the source-home data dir to the supervisor");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_ROOT !== vmAdapterPath.replace(/\/vm-config\/browser-engine-adapter\.json$/, "/vm-data/bvm")) {
  throw new Error("Linux source-home Browser config must keep VM sessions under the ElastOS data dir");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_PROFILE_DISK_ROOT) {
  throw new Error("VM adapter must receive Browser profile disks from Runtime launch descriptors, not a global profile root env");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_PROFILE_ROOT) {
  throw new Error("VM adapter must not use the old hosted Browser profile root env");
}
if (adapter?.supervisor?.timeout_ms !== 240000) {
  throw new Error("VM adapter must fail slow launch attempts inside the source-home startup budget");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_ENGINE_TIMEOUT_MS !== "225000") {
  throw new Error("VM adapter must keep the supervisor control request timeout aligned with the source-home launch timeout");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_CONTROL_LAUNCH_TIMEOUT_MS !== "210000") {
  throw new Error("VM adapter must let the VM control service report guest readiness failures before the supervisor times out");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_CONTROL_PROXY_REQUEST_TIMEOUT_MS !== "120000") {
  throw new Error("VM adapter must keep the VZ host control proxy request timeout aligned with guest command latency");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_CONTROL_READY_TIMEOUT_MS !== "120000") {
  throw new Error("VM adapter must keep the guest control readiness budget bounded");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_EGRESS_MAX_SESSIONS !== "16") {
  throw new Error("VM adapter must bound pre-opened Runtime egress streams while keeping enough slots for Browser pages");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_CONTROL_MAX_ACTIVE_PAGES !== "1") {
  throw new Error("VM adapter must pass the single active Browser VM page contract to the control service");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_IDLE_KEEPALIVE_MS !== "0") {
  throw new Error("Linux VM adapter must not retain warm crosvm sessions until reuse has a display-health proof");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_REUSE_IDLE_VMS !== "0") {
  throw new Error("Linux VM adapter must fail closed on profile-keyed idle VM reuse");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_HIBERNATION !== "0") {
  throw new Error("Linux source-home Browser config must not claim VZ hibernation");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_PREWARM_CONTROL_SERVICE !== "1") {
  throw new Error("VM adapter must prewarm the Browser VM control service during Runtime startup");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_CONTROL_LAUNCHER !== vmAdapterPath.replace(/\/vm-config\/browser-engine-adapter\.json$/, "/vm-data/bin/browser-vm-local-crosvm-launcher")) {
  throw new Error("Linux source-home Browser config must default to the local crosvm VM control launcher");
}
if (!adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_ROOTFS_POOL_DIR?.endsWith("/vm-data/browser-vm/rootfs-pool")) {
  throw new Error("Linux source-home Browser config must use a prepared rootfs pool");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_ROOTFS_COPY_MODE !== "pool-required") {
  throw new Error("Linux source-home Browser config must require prepared rootfs pool copies");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_ROOTFS_POOL_REFILL_COUNT !== "2") {
  throw new Error("Linux source-home Browser config must request prepared rootfs pool refill after launch");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_ROOTFS_POOL_REFILL_SCRIPT !== vmAdapterPath.replace(/\/vm-config\/browser-engine-adapter\.json$/, "/vm-data/bin/browser-vm-prepare-rootfs-pool")) {
  throw new Error("Linux source-home Browser config must point at the rootfs pool refill wrapper");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_ICE_SERVER !== "turn:turn.example.invalid:3478?transport=tcp") {
  throw new Error("VM source-home config must carry operator TURN relay config to the supervisor env");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_ICE_USERNAME !== "source-home-user") {
  throw new Error("VM source-home config must carry TURN usernames to the supervisor env");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_ICE_CREDENTIAL !== "source-home-secret") {
  throw new Error("VM source-home config must carry TURN credentials to the supervisor env");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY !== "relay") {
  throw new Error("VM source-home config must keep WebRTC on relay-only ICE policy");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4 !== "10.44.0.1") {
  throw new Error("VM source-home config must carry operator media relay host IPv4 to the supervisor env");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4 !== "10.44.0.2") {
  throw new Error("VM source-home config must carry operator media relay guest IPv4 to the supervisor env");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX !== "24") {
  throw new Error("VM source-home config must carry operator media relay prefix to the supervisor env");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_TRACE !== "1") {
  throw new Error("VM source-home config must carry operator diagnostic trace to the supervisor env");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_CONTROL_STATUS_PROBE_TIMEOUT_MS !== "3000") {
  throw new Error("VM source-home config must carry guest-control status probe diagnostics to the supervisor env");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_DEBUG_HOLD_ON_OPEN_ERROR_MS !== "60000") {
  throw new Error("VM source-home config must carry open-error debug hold diagnostics to the supervisor env");
}
if (adapter?.supervisor?.env?.ELASTOS_BROWSER_VM_TURNSERVER_BIN !== "/tmp/elastos-test-turnserver") {
  throw new Error("VM source-home config must carry the canonical turnserver binary env to the supervisor env");
}
if (vmExit.backends?.[0]?.adapter_ipc?.kind !== "unix_socket") throw new Error("VM missing adapter_ipc socket");
if (vmExit.backends?.[0]?.relay_ipc?.kind !== "unix_socket") throw new Error("VM missing relay_ipc socket");
if (vmExit.backends?.[0]?.adapter_ipc?.path === vmExit.backends?.[0]?.relay_ipc?.path) {
  throw new Error("VM adapter_ipc and relay_ipc sockets must differ");
}
if (vmExit.backends?.[0]?.allow_private_targets !== false) {
  throw new Error("VM Exit must block private targets by default");
}
const vmPrivateTargets = vmExit.backends?.[0]?.allowed_private_targets || [];
if (vmPrivateTargets.length !== 1 || vmPrivateTargets[0].host !== "localhost") {
  throw new Error("VM Exit must allow only the explicit Runtime gateway private target");
}
if (!vmPrivateTargets[0].ports?.includes(61180) || !vmPrivateTargets[0].ports?.includes(8090)) {
  throw new Error("VM Exit Runtime gateway private target must include Mac staging and local source-home ports");
}
if (vmLocalExit.schema !== "elastos.browser.local-exit.config/v1") throw new Error("VM missing browser-local-exit config");
if (vmLocalExit.relay_ipc_path !== vmExit.backends?.[0]?.relay_ipc?.path) {
  throw new Error("VM browser-local-exit relay socket must match exit-provider relay_ipc");
}
if (vmLocalExit.allow_private_targets !== false) throw new Error("VM local Exit must block private targets by default");
if (JSON.stringify(vmLocalExit.allowed_private_targets) !== JSON.stringify(vmPrivateTargets)) {
  throw new Error("VM local Exit private target policy must match Exit provider");
}
const macAdapter = macVmAdapter.adapters?.[0];
if (macAdapter?.id !== "browser-vm-product") throw new Error("wrong Mac VM adapter id");
if (macAdapter?.kind !== "chromium_microvm") throw new Error("wrong Mac VM engine");
if (macAdapter?.supervisor?.env?.ELASTOS_BROWSER_VM_PLATFORM !== "darwin-arm64") {
  throw new Error("Mac source-home Browser config must use the Darwin VM platform");
}
if (
  macVzTransport.schema !==
    "elastos.browser.vz-transport-config/v1" ||
  macVzTransport.enabled !== true ||
  macVzTransport.turn_listen_host !== "127.0.0.1" ||
  macVzTransport.turn_advertised_host !== "192.168.65.1" ||
  macVzTransport.turn_relay_host !== "192.168.65.1" ||
  macVzTransport.guest_turn_host !== "127.0.0.1" ||
  new Set([
    macVzTransport.bootstrap_vsock_port,
    macVzTransport.egress_vsock_port,
    macVzTransport.media_vsock_port,
  ]).size !== 3
) {
  throw new Error("Mac source-home VZ transport authority config is invalid");
}
if ((fs.statSync(macVzTransportPath).mode & 0o077) !== 0) {
  throw new Error("Mac source-home VZ transport config must be owner-only");
}
const macVzTransportBytes = fs.readFileSync(macVzTransportPath, "utf8");
if (
  macVzTransportBytes.includes("mac-secret") ||
  macVzTransportBytes.includes("source-home-secret")
) {
  throw new Error("Mac source-home VZ transport config persisted a shared TURN secret");
}
if (
  macAdapter?.supervisor?.env?.ELASTOS_BROWSER_REMOTE_VZ_TURN_PROGRAM !==
  "/opt/homebrew/bin/turnserver"
) {
  throw new Error("Remote VZ source-home config must bind the remote TURN program path");
}
if ("ELASTOS_BROWSER_VM_HIBERNATION" in (macAdapter?.supervisor?.env || {})) {
  throw new Error("Mac source-home VZ transport config must not carry legacy hibernation configuration");
}
if (macAdapter?.supervisor?.env?.ELASTOS_BROWSER_VM_IDLE_KEEPALIVE_MS !== "300000") {
  throw new Error("Mac VZ source-home Browser config may keep same-principal Browser VMs warm briefly");
}
if (macAdapter?.supervisor?.env?.ELASTOS_BROWSER_VM_REUSE_IDLE_VMS !== "1") {
  throw new Error("Mac VZ source-home Browser config must explicitly opt in to profile-keyed idle VM reuse");
}
for (const legacyKey of [
  "ELASTOS_BROWSER_VM_HIBERNATION_DIR",
  "ELASTOS_BROWSER_VM_HIBERNATION_MAX_ENTRIES",
  "ELASTOS_BROWSER_VM_HIBERNATION_MAX_AGE_SECS",
]) {
  if (legacyKey in (macAdapter?.supervisor?.env || {})) {
    throw new Error(`Mac source-home VZ transport config must omit ${legacyKey}`);
  }
}
if (macAdapter?.supervisor?.env?.ELASTOS_BROWSER_VM_CONTROL_LAUNCHER !== macVmAdapterPath.replace(/\/mac-vm-config\/browser-engine-adapter\.json$/, "/browser-vm-remote-vz-launcher")) {
  throw new Error("Mac source-home Browser config must pass the VZ control launcher through the VM supervisor");
}
if (macAdapter?.supervisor?.env?.ELASTOS_BROWSER_VM_CONTROL_LAUNCH_TIMEOUT_MS !== "210000") {
  throw new Error("Remote VZ source-home Browser config must keep the outer control launch budget bounded");
}
if (macAdapter?.supervisor?.env?.ELASTOS_BROWSER_VM_CONTROL_PROXY_REQUEST_TIMEOUT_MS !== "120000") {
  throw new Error("Remote VZ source-home Browser config must keep command proxy requests from timing out before Browser input returns");
}
if (macAdapter?.supervisor?.env?.ELASTOS_BROWSER_REMOTE_VZ_LAUNCH_TIMEOUT_MS !== "180000") {
  throw new Error("Remote VZ source-home Browser config must pin the remote launcher budget instead of inheriting ambient env");
}
if (Object.prototype.hasOwnProperty.call(macAdapter?.supervisor?.env || {}, "ELASTOS_BROWSER_VM_CONTROL_READY_TIMEOUT_MS")) {
  throw new Error("Remote VZ source-home Browser config must let the remote launcher derive an inner guest-ready margin");
}
assertNoRemoteVzLocalTurnEnv(macAdapter?.supervisor?.env, "Remote VZ source-home Browser config");
const linuxRemoteAdapter = linuxRemoteVmAdapter.adapters?.[0];
if (linuxRemoteAdapter?.supervisor?.env?.ELASTOS_BROWSER_VM_PLATFORM !== "linux-amd64") {
  throw new Error("Linux remote VZ source-home Browser config must preserve the local host platform");
}
if (linuxRemoteAdapter?.supervisor?.env?.ELASTOS_BROWSER_VM_CONTROL_LAUNCHER !== linuxRemoteVmAdapterPath.replace(/\/linux-remote-vz-config\/browser-engine-adapter\.json$/, "/browser-vm-remote-vz-launcher")) {
  throw new Error("Linux remote VZ source-home Browser config must pass the remote VZ control launcher through the VM supervisor");
}
if (linuxRemoteAdapter?.supervisor?.env?.ELASTOS_BROWSER_VM_ROOT !== "/tmp/evzs") {
  throw new Error("Linux remote VZ source-home Browser config must use the remote VZ VM root");
}
if (Object.prototype.hasOwnProperty.call(linuxRemoteAdapter?.supervisor?.env || {}, "ELASTOS_BROWSER_VM_ROOTFS_POOL_DIR")) {
  throw new Error("Linux remote VZ source-home Browser config must not inherit local crosvm rootfs-pool env");
}
if (linuxRemoteAdapter?.supervisor?.env?.ELASTOS_BROWSER_REMOTE_VZ_LAUNCH_TIMEOUT_MS !== "180000") {
  throw new Error("Linux remote VZ source-home Browser config must pin the remote launcher budget instead of inheriting ambient env");
}
if (linuxRemoteAdapter?.supervisor?.env?.ELASTOS_BROWSER_REMOTE_VZ_DATA_DIR !== linuxRemoteVmAdapterPath.replace(/\/linux-remote-vz-config\/browser-engine-adapter\.json$/, "/mac-provider-data")) {
  throw new Error("Linux remote VZ source-home Browser config must pin the remote provider data dir instead of falling back to the Mac default install");
}
if (
  linuxRemoteAdapter?.supervisor?.env?.ELASTOS_BROWSER_REMOTE_VZ_TURN_PROGRAM !==
  "/opt/homebrew/bin/turnserver"
) {
  throw new Error("Linux remote VZ config must bind the remote TURN program path");
}
if (linuxRemoteAdapter?.supervisor?.env?.ELASTOS_BROWSER_VM_CONTROL_PROXY_REQUEST_TIMEOUT_MS !== "120000") {
  throw new Error("Linux remote VZ source-home Browser config must keep command proxy requests from timing out before Browser input returns");
}
if (Object.prototype.hasOwnProperty.call(linuxRemoteAdapter?.supervisor?.env || {}, "ELASTOS_BROWSER_VM_CONTROL_READY_TIMEOUT_MS")) {
  throw new Error("Linux remote VZ source-home Browser config must let the remote launcher derive an inner guest-ready margin");
}
assertNoRemoteVzLocalTurnEnv(linuxRemoteAdapter?.supervisor?.env, "Linux remote VZ source-home Browser config");
if (
  linuxRemoteVzTransport.schema !==
    "elastos.browser.vz-transport-config/v1" ||
  linuxRemoteVzTransport.turn_advertised_host !== "192.168.65.1"
) {
  throw new Error("Linux remote VZ source-home config must issue VZ transport authority");
}
const defaultMacAdapter = defaultMacVmAdapter.adapters?.[0];
if (defaultMacAdapter?.supervisor?.control_socket_path !== "/tmp/elastos-browser-vm-control-darwin-arm64.sock") {
  throw new Error("Mac source-home Browser config must declare a default VM control socket");
}
if (defaultMacAdapter?.supervisor?.env?.ELASTOS_BROWSER_VM_CONTROL_SOCKET !== "/tmp/elastos-browser-vm-control-darwin-arm64.sock") {
  throw new Error("Mac source-home Browser supervisor must receive the default VM control socket");
}
if (defaultMacAdapter?.supervisor?.env?.ELASTOS_BROWSER_VM_CONTROL_READY_TIMEOUT_MS !== "120000") {
  throw new Error("Local VZ source-home Browser config must keep the guest control readiness budget bounded");
}
if (defaultMacAdapter?.supervisor?.env?.ELASTOS_BROWSER_VM_CONTROL_PROXY_REQUEST_TIMEOUT_MS !== "120000") {
  throw new Error("Local VZ source-home Browser config must keep command proxy requests from timing out before Browser input returns");
}
if (defaultMacAdapter?.supervisor?.env?.ELASTOS_BROWSER_VM_CONTROL_LAUNCHER !== defaultMacVmAdapterPath.replace(/\/default-mac-config\/browser-engine-adapter\.json$/, "/default-mac-data/bin/browser-vz-engine-supervisor")) {
  throw new Error("Mac source-home Browser config must default to the local VZ control launcher");
}
if (
  defaultMacVzTransport.schema !==
    "elastos.browser.vz-transport-config/v1" ||
  defaultMacVzTransport.turn_advertised_host !== "127.0.0.1" ||
  "ELASTOS_BROWSER_VM_HIBERNATION" in
    (defaultMacAdapter?.supervisor?.env || {})
) {
  throw new Error("Local Mac source-home Browser config must use no-NIC VZ transport by default");
}
const turnEnv = turnAdapter.adapters?.[0]?.supervisor?.env || {};
assertNoRemoteVzLocalTurnEnv(
  turnEnv,
  "Local Mac no-NIC VZ source-home Browser config",
);
const linuxTurnEnv = linuxTurnAdapter.adapters?.[0]?.supervisor?.env || {};
if (!linuxTurnEnv.ELASTOS_BROWSER_VM_ICE_SERVERS_JSON?.includes("{host_ip}")) {
  throw new Error("Linux source-home Browser config must preserve per-session TURN host placeholder");
}
if (!linuxTurnEnv.ELASTOS_BROWSER_VM_ICE_SERVERS_JSON?.includes("{turn_port}")) {
  throw new Error("Linux source-home Browser config must preserve per-session TURN port placeholder");
}
if (!linuxTurnEnv.ELASTOS_BROWSER_VM_ICE_SERVERS_JSON?.includes("linux-runtime-turn-user")) {
  throw new Error("Linux source-home Browser config must load runtime TURN credentials from data dir");
}
if (linuxTurnEnv.ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY !== "relay") {
  throw new Error("Linux source-home Browser config must carry relay-only ICE policy from runtime TURN credentials");
}
if (preserveExit.backends?.length !== 1) {
  throw new Error("source-home config must refresh the local Exit backend while preserving remote grants");
}
if (preserveExit.remote_carrier_exits?.length !== 1 ||
    preserveExit.remote_carrier_exits[0].id !== "seed-node" ||
    preserveExit.remote_carrier_exits[0].connect_ticket !== "private-ticket-redacted-in-reports") {
  throw new Error("source-home config must preserve existing remote Carrier Exit grants");
}
if (preserveReceipt.remote_carrier_exit_count !== 1) {
  throw new Error("source-home config receipt must report preserved remote Carrier Exit grants");
}
NODE

printf '%s\n' '{"schema":"elastos.browser.source-home-config-smoke/v1","ok":true}'
