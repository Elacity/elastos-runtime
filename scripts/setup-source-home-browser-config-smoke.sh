#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

find_node() {
  if [[ -n "${ELASTOS_NODE_BIN:-}" && -x "${ELASTOS_NODE_BIN}" ]]; then
    printf '%s\n' "${ELASTOS_NODE_BIN}"
    return
  fi
  if command -v node >/dev/null 2>&1; then
    command -v node
    return
  fi
  return 1
}

node_bin="$(find_node || true)"
if [[ -z "$node_bin" ]]; then
  echo "node not found. Install Node or set ELASTOS_NODE_BIN to an executable node binary." >&2
  exit 2
fi

case "$(uname -s)" in
  Darwin) data_dir="$tmp_dir/home/Library/Application Support/elastos" ;;
  Linux) data_dir="$tmp_dir/xdg/elastos" ;;
  *) echo "unsupported setup-source-home smoke host: $(uname -s)" >&2; exit 2 ;;
esac
mkdir -p "$data_dir/config"
shared_turn_env="$tmp_dir/shared-turn.env"
cat > "$shared_turn_env" <<'EOF'
ELASTOS_BROWSER_VM_ICE_SERVERS_JSON=[{"urls":["turn:192.168.65.1:3478?transport=udp"],"username":"elastos-browser","credential":"shared-runtime-turn-secret"}]
ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY=relay
ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4=192.168.65.1
ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4=192.168.65.2
ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX=24
EOF
cat > "$data_dir/config/browser-engine-adapter.json" <<JSON
{
  "max_active_sessions": 1,
  "adapters": [{
    "id": "browser-vm-product",
    "kind": "chromium_microvm",
    "network_mode": "runtime_net_only",
    "display_modes": ["webrtc_remote_display"],
    "supervisor": {
      "program": "$data_dir/bin/browser-vm-engine-supervisor",
      "timeout_ms": 180000,
      "control_socket_path": "/tmp/existing-remote-browser-vm.sock",
      "env": {
        "ELASTOS_BROWSER_VM_CONTROL_SOCKET": "/tmp/existing-remote-browser-vm.sock",
        "ELASTOS_BROWSER_VM_CONTROL_LAUNCHER": "$data_dir/bin/browser-vm-remote-vz-launcher",
        "ELASTOS_BROWSER_VM_CONTROL_STATUS_PROBE_TIMEOUT_MS": "4321",
        "ELASTOS_BROWSER_VM_DEBUG_HOLD_ON_OPEN_ERROR_MS": "8765"
      }
    }
  }]
}
JSON

HOME="$tmp_dir/home" \
XDG_DATA_HOME="$tmp_dir/xdg" \
ELASTOS_NODE_BIN="$node_bin" \
ELASTOS_BROWSER_RUNTIME_TURN_ENV="$shared_turn_env" \
ELASTOS_BROWSER_VZ_TURN_ADVERTISED_HOST="192.168.65.1" \
ELASTOS_BROWSER_VZ_TURN_RELAY_HOST="192.168.65.1" \
SETUP_SOURCE_HOME_CONFIG_ONLY=1 \
  "$repo_root/scripts/setup-source-home.sh" >"$tmp_dir/setup-source-home.log"

"$node_bin" - "$data_dir/config/browser-engine-adapter.json" "$data_dir" <<'NODE'
const fs = require("node:fs");
const [configPath, dataDir] = process.argv.slice(2);
const config = JSON.parse(fs.readFileSync(configPath, "utf8"));
const adapter = config.adapters?.find((entry) => entry.id === "browser-vm-product");
if (!adapter) throw new Error("browser-vm-product adapter missing");
const supervisor = adapter.supervisor || {};
const env = supervisor.env || {};
if (supervisor.control_socket_path !== "/tmp/existing-remote-browser-vm.sock") {
  throw new Error("setup-source-home did not preserve the existing remote control socket");
}
if (env.ELASTOS_BROWSER_VM_CONTROL_SOCKET !== "/tmp/existing-remote-browser-vm.sock") {
  throw new Error("setup-source-home did not preserve the remote control socket env");
}
if (env.ELASTOS_BROWSER_VM_CONTROL_LAUNCHER !== `${dataDir}/bin/browser-vm-remote-vz-launcher`) {
  throw new Error("setup-source-home did not preserve the remote VZ launcher");
}
if (env.ELASTOS_BROWSER_VM_ROOT !== "/tmp/evzs") {
  throw new Error("remote VZ setup must keep the VM root under the remote host default");
}
if (Object.prototype.hasOwnProperty.call(env, "ELASTOS_BROWSER_VM_ROOTFS_POOL_DIR")) {
  throw new Error("remote VZ setup must not inherit local crosvm rootfs pool env");
}
if (Object.prototype.hasOwnProperty.call(env, "ELASTOS_BROWSER_VM_CONTROL_READY_TIMEOUT_MS")) {
  throw new Error("remote VZ setup must let the remote launcher derive guest-ready timing");
}
if (env.ELASTOS_BROWSER_VM_CONTROL_STATUS_PROBE_TIMEOUT_MS !== "4321") {
  throw new Error("setup-source-home did not preserve control status probe diagnostics");
}
if (env.ELASTOS_BROWSER_VM_DEBUG_HOLD_ON_OPEN_ERROR_MS !== "8765") {
  throw new Error("setup-source-home did not preserve open-error diagnostics");
}
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
  if (Object.prototype.hasOwnProperty.call(env, key)) {
    throw new Error(`remote VZ setup must not inherit local ${key}`);
  }
}
NODE

"$node_bin" - "$data_dir/config/browser-vz-vsock-transport.json" <<'NODE'
const fs = require("node:fs");
const config = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
if (
  config.schema !== "elastos.browser.vz-transport-config/v1" ||
  config.turn_advertised_host !== "192.168.65.1" ||
  config.turn_relay_host !== "192.168.65.1"
) {
  throw new Error("remote VZ setup did not issue exact no-NIC transport config");
}
if ((fs.statSync(process.argv[2]).mode & 0o077) !== 0) {
  throw new Error("remote VZ setup transport config is not owner-only");
}
NODE

if [[ -e "$data_dir/runtime-turn/turn-credentials.env" ]]; then
  echo "setup-source-home should use the explicit shared runtime TURN env instead of creating a local one" >&2
  exit 1
fi
grep -q "use existing Browser runtime TURN env" "$repo_root/scripts/setup-source-home.sh"

printf '%s\n' '{"schema":"elastos.setup-source-home.browser-config-smoke/v1","ok":true}'
