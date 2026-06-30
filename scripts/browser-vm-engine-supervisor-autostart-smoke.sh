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

cat > "$tmp_dir/fake-vz-launcher" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
if [[ -n "${ELASTOS_BROWSER_VM_AUTOSTART_PID_FILE:-}" ]]; then
  printf '%s\n' "$$" >> "$ELASTOS_BROWSER_VM_AUTOSTART_PID_FILE"
fi
exec "${ELASTOS_NODE_BIN:?}" - <<'NODE'
const fs = require("node:fs");
const path = require("node:path");
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
console.log(JSON.stringify({
  schema: "elastos.browser.engine.supervisor-result/v1",
  page_id: "page:vm-autostart-smoke",
  adapter: launch.adapter,
  engine: launch.engine,
  stream_id: launch.stream_id,
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
    ice_servers: JSON.parse(process.env.ELASTOS_BROWSER_VM_ICE_SERVERS_JSON || "[]"),
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
setInterval(() => {}, 1000);
NODE
SH
chmod 755 "$tmp_dir/fake-vz-launcher"

request_json="$("$node_bin" - <<'NODE'
console.log(JSON.stringify({
  schema: "elastos.browser.engine.launch-request/v1",
  adapter: "browser-vm-product",
  engine: "chromium_microvm",
  stream_id: "stream:vm-autostart-smoke",
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

"$node_bin" - <<'NODE' "$tmp_dir/result.json" "$control_log" "$control_socket" "$status_one"
const fs = require("node:fs");
const [resultPath, logPath, socketPath, statusPath] = process.argv.slice(2);
const result = JSON.parse(fs.readFileSync(resultPath, "utf8"));
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
const log = fs.readFileSync(logPath, "utf8");
if (!log.includes("elastos.browser.vm-control-service.ready/v1")) throw new Error("control service did not start");
if (!log.includes("\"event\":\"launch_ready\"")) throw new Error("control service did not launch page");
NODE

curl --unix-socket "$control_socket" -sS --max-time 3 \
  -H 'Content-Type: application/json' \
  -d '{"page_id":"page:vm-autostart-smoke"}' \
  http://localhost/shutdown >/dev/null

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

curl --unix-socket "$control_socket" -sS --max-time 3 \
  -H 'Content-Type: application/json' \
  -d '{"page_id":"page:vm-autostart-smoke"}' \
  http://localhost/shutdown >/dev/null

printf '%s\n' '# source-home wrapper revision for stale-process proof' >> "$tmp_dir/control-service"

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

printf '%s\n' '{"schema":"elastos.browser.vm-engine-supervisor-autostart-smoke/v1","ok":true}'
