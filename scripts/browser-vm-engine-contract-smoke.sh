#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"
service_pid=""
node_bin="${ELASTOS_NODE_BIN:-}"
if [[ -z "$node_bin" ]]; then
  node_bin="$(command -v node 2>/dev/null || true)"
fi
if [[ ! -x "$node_bin" ]]; then
  echo "node not found. Set ELASTOS_NODE_BIN to an executable node binary." >&2
  exit 2
fi
export PATH="$(dirname "$node_bin"):$PATH"

cleanup() {
  if [[ -n "$service_pid" ]]; then
    kill "$service_pid" >/dev/null 2>&1 || true
    wait "$service_pid" 2>/dev/null || true
  fi
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

cd "$repo_root"

control_socket="$tmp_dir/browser-vm-control.sock"
"$node_bin" - "$control_socket" <<'NODE' &
const fs = require("node:fs");
const http = require("node:http");
const socketPath = process.argv[2];
const controlService = {
  schema: "elastos.browser.vm-control-service.identity/v1",
  service_id: `service:${"c".repeat(64)}`,
  control_socket_path: socketPath,
  config_fingerprint: null,
};
try { fs.unlinkSync(socketPath); } catch {}

function json(res, status, body) {
  const data = Buffer.from(JSON.stringify(body));
  res.writeHead(status, { "content-type": "application/json", "content-length": data.length });
  res.end(data);
}

const server = http.createServer((req, res) => {
  if (req.method === "GET" && req.url === "/status") {
    json(res, 200, {
      schema: "elastos.browser.vm-control-service.status/v1",
      ok: true,
      control_service: controlService,
      active_pages: 0,
    });
    return;
  }
  if (req.method === "POST" && req.url === "/shutdown") {
    const chunks = [];
    req.on("data", (chunk) => chunks.push(chunk));
    req.on("end", () => {
      const body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
      const binding = body.runtime_cleanup;
      json(res, 200, {
        schema: "elastos.browser.supervisor-cleanup-result/v2",
        page_id: binding.page_id,
        generation: binding.generation,
        binding,
        terminal: true,
        effects: {
          page_absent: true,
          child_absent: true,
          vm_absent: true,
          route_absent: true,
          socket_absent: true,
        },
      });
    });
    return;
  }
  if (req.method !== "POST" || req.url !== "/pages") {
    json(res, 404, { error: "not found" });
    return;
  }
  const chunks = [];
  req.on("data", (chunk) => chunks.push(chunk));
  req.on("end", () => {
    const body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    if (body.schema !== "elastos.browser.vm-engine.open/v1") {
      json(res, 400, { error: "wrong request schema" });
      return;
    }
    const launch = body.launch_request;
    if (launch.engine !== "chromium_microvm" || body.requirements?.substrate !== "microvm") {
      json(res, 400, { error: "wrong VM engine request" });
      return;
    }
    if (launch.url === "https://did-not-act.invalid/") {
      json(res, 503, {
        error: "injected exact VZ pre-effect failure",
        launch_settlement_result: {
          schema: "elastos.browser.vz-launch-settlement/v1",
          state: "did_not_act",
          message: "injected exact VZ pre-effect failure",
          binding_hash: launch.transport_authority.binding_hash,
          generation: launch.lifecycle_generation,
          page_id: launch.page_id,
          vm_id: launch.vm_id,
          stream_id: launch.stream_id,
          media_stream_id: launch.transport_authority.media.stream_id,
          effects: {
            session_directory: false,
            control_socket: false,
            ordinary_stream_bridge: false,
            media_stream_bridge: false,
            turn_process: false,
            supervisor_child: false,
            vm: false,
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
        },
      });
      return;
    }
    json(res, 200, {
      schema: "elastos.browser.engine.supervisor-result/v1",
      page_id: "page:vm-contract-smoke",
      adapter: launch.adapter,
      engine: launch.engine,
      stream_id: launch.stream_id,
      actual_url: launch.url,
      title: "ElastOS Browser VM Contract Smoke",
      network_mode: "runtime_net_only",
      direct_network: false,
      wallet_injection: false,
      control_socket_path: socketPath,
      isolated_session: true,
      isolation: {
        schema: "elastos.browser.engine.isolation/v1",
        kind: "per_launch_vm_target",
        session_dir: "/tmp/elastos-browser-vm-sessions/vm-contract-smoke",
      },
      control_service: controlService,
      process: {
        schema: "elastos.browser.host-process-binding/v1",
        ownership_id: `process:${"d".repeat(64)}`,
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
          sdp: "v=0\r\ns=ElastOS Browser VM\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n",
        },
        display_backend: "vm_selkies_gstreamer_webrtc",
        backend_class: "product_compositor",
        media_transport: "runtime_relay",
        audio: true,
        video: true,
        network_mode: "runtime_net_only",
        direct_network: false,
        signaling_url: "/api/apps/browser/pages/page%3Avm-contract-smoke/webrtc",
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

"$node_bin" scripts/browser-source-home-config.mjs \
  --data-dir "$tmp_dir/data" \
  --platform linux-amd64 \
  --out-dir "$tmp_dir/config" \
  --vm-supervisor "$repo_root/scripts/browser-vm-engine-supervisor.mjs" \
  --vm-control-socket "$control_socket" >/dev/null

cargo build --quiet --manifest-path capsules/browser-engine-adapter/Cargo.toml
adapter_bin="${CARGO_TARGET_DIR:-capsules/browser-engine-adapter/target}/debug/browser-engine-adapter"

request_json="$(CONFIG_PATH="$tmp_dir/config/browser-engine-adapter.json" CONTROL_SOCKET="$control_socket" CONTROL_PROCESS_PID="$service_pid" "$node_bin" - <<'NODE'
const fs = require("node:fs");
const config = JSON.parse(fs.readFileSync(process.env.CONFIG_PATH, "utf8"));
const streamSession = {
  schema: "elastos.exit.stream-session/v1",
  stream_id: "stream:vm-contract-smoke",
  target: "tls://example.com:443",
  byte_transport: "adapter_ipc",
  adapter_ipc: {
    schema: "elastos.adapter-ipc/v1",
    kind: "unix_socket",
    path: "/tmp/elastos-browser-vm-contract-adapter.sock",
    stream_id: "stream:vm-contract-smoke",
    runtime_stream_path: "/tmp/elastos-browser-vm-contract-runtime.sock",
  },
};
const browserProfile = {
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
  disk_path: "/tmp/elastos-browser-vm-contract/BrowserProfiles/default/profile.ext4",
  reset: "whole_profile",
};
console.log(JSON.stringify({ op: "init", config }));
console.log(JSON.stringify({
  op: "launch",
  url: "https://example.com/",
  stream_session: streamSession,
  lifecycle_generation: "sha256:vm-contract-smoke",
  profile: browserProfile,
  principal_id: "person:local:browser-vm-contract-smoke",
  reason: "verify Browser VM engine contract",
  display_mode: "webrtc_remote_display",
  guarantee_level: "mechanism_microvm",
  viewport: { width: 1280, height: 720 },
}));
console.log(JSON.stringify({
  op: "close_page",
  page_id: "page:vm-contract-smoke",
  principal_id: "person:local:browser-vm-contract-smoke",
  runtime_cleanup: {
    schema: "elastos.browser.engine-cleanup-binding/v2",
    page_id: "page:vm-contract-smoke",
    generation: "sha256:vm-contract-smoke",
    stream_id: "stream:vm-contract-smoke",
    adapter: "browser-vm-product",
    engine: "chromium_microvm",
    display_mode: "webrtc_remote_display",
    guarantee_level: "mechanism_microvm",
    principal_id: "person:local:browser-vm-contract-smoke",
    control_socket_path: process.env.CONTROL_SOCKET,
    shutdown_socket_path: process.env.CONTROL_SOCKET,
    isolated_session: true,
    isolation: {
      schema: "elastos.browser.engine.isolation/v1",
      kind: "per_launch_vm_target",
      session_dir: "/tmp/elastos-browser-vm-sessions/vm-contract-smoke",
    },
    control_service: {
      schema: "elastos.browser.vm-control-service.identity/v1",
      service_id: `service:${"c".repeat(64)}`,
      control_socket_path: process.env.CONTROL_SOCKET,
      config_fingerprint: null,
    },
    process: {
      schema: "elastos.browser.host-process-binding/v1",
      ownership_id: `process:${"d".repeat(64)}`,
      pid: Number(process.env.CONTROL_PROCESS_PID),
      stream_bridge_pid: null,
    },
  },
}));
console.log(JSON.stringify({ op: "shutdown" }));
NODE
)"

output="$(printf '%s\n' "$request_json" | \
  ELASTOS_BROWSER_VM_DATA_DIR="$tmp_dir/Application Support/elastos" \
  ELASTOS_BROWSER_VM_ROOT="$tmp_dir/vm-root" \
  "$adapter_bin")"

OUTPUT="$output" "$node_bin" - <<'NODE'
const lines = process.env.OUTPUT.split(/\r?\n/).filter(Boolean).map((line) => JSON.parse(line));
const launch = lines.find((line) => line.status === "ok" && line.data?.schema === "elastos.browser.engine.page/v1");
if (!launch) throw new Error(`missing launch result: ${process.env.OUTPUT}`);
if (launch.data.adapter !== "browser-vm-product") throw new Error("wrong adapter");
if (launch.data.engine !== "chromium_microvm") throw new Error("wrong engine");
if (launch.data.display_session?.display_backend !== "vm_selkies_gstreamer_webrtc") throw new Error("wrong display backend");
if (launch.data.display_session?.backend_class !== "product_compositor") throw new Error("not product compositor");
if (launch.data.display_session?.audio !== true || launch.data.display_session?.video !== true) throw new Error("VM product display did not advertise audio+video");
if (launch.data.display_session?.media_transport !== "runtime_relay") throw new Error("VM display is not Runtime-relayed");
if (launch.data.direct_network !== false || launch.data.wallet_injection !== false) throw new Error("authority proof failed");
if (launch.data.engine_control !== "page_scoped") throw new Error("VM control is not page-scoped");
if (launch.data.isolated_engine_session !== true) throw new Error("VM session is not isolated");
if (launch.data.isolation?.kind !== "per_launch_vm_target") throw new Error("VM isolation proof is missing");
const close = lines.find((line) => line.status === "ok" && line.data?.schema === "elastos.browser.engine-cleanup-result/v2");
if (!close) throw new Error("missing close result");
if (close.data.terminal !== true || Object.values(close.data.effects || {}).some((value) => value !== true)) {
  throw new Error("close did not return exact terminal VM cleanup proof");
}
NODE

settlement_request_json="$(CONFIG_PATH="$tmp_dir/config/browser-engine-adapter.json" "$node_bin" - <<'NODE'
const crypto = require("node:crypto");
const fs = require("node:fs");
const config = JSON.parse(fs.readFileSync(process.env.CONFIG_PATH, "utf8"));
const expiresAtUnixSecs = Math.floor(Date.now() / 1000) + 300;
const expiresAtUnixMs = expiresAtUnixSecs * 1000;
const generation = `sha256:${"a".repeat(64)}`;
const streamId = "stream:vm-contract-did-not-act";
const pageId = "page:vz-vm-contract-did-not-act";
const vmId = "vm:vz-vm-contract-did-not-act";
const principalId = "person:local:vm-contract-did-not-act";
const username = `${expiresAtUnixSecs}:vm-contract-did-not-act`;
const authSecret = "vm-contract-did-not-act-auth-secret";
const credential = crypto.createHmac("sha1", authSecret).update(username).digest("base64");
const sha256Label = (value) =>
  `sha256:${crypto.createHash("sha256").update(value).digest("hex")}`;
const canonical = (value) => {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object") {
    return Object.keys(value).sort().reduce((out, key) => {
      out[key] = canonical(value[key]);
      return out;
    }, {});
  }
  return value;
};
const authority = {
  schema: "elastos.browser.vz-transport-authority/v1",
  generation,
  page_id: pageId,
  vm_id: vmId,
  principal_id: principalId,
  egress: {
    schema: "elastos.browser.vz-transport-stream/v1",
    stream_id: streamId,
    target: "tls://did-not-act.invalid:443",
    runtime_socket_path: "/tmp/elastos-browser-vm-contract-runtime.sock",
    vsock_port: 19091,
  },
  media: {
    schema: "elastos.browser.vz-transport-stream/v1",
    stream_id: "stream:media-vm-contract-did-not-act",
    target: "tcp://127.0.0.1:49160",
    runtime_socket_path: "/tmp/elastos-browser-vm-contract-media.sock",
    vsock_port: 19094,
  },
  turn: {
    schema: "elastos.browser.vz-turn-authority/v1",
    guest_url: "turn:127.0.0.1:3478?transport=tcp",
    guest_host: "127.0.0.1",
    guest_port: 3478,
    listen_host: "127.0.0.1",
    listen_port: 49160,
    advertised_host: "192.0.2.10",
    relay_host: "192.0.2.10",
    relay_port_min: 55000,
    relay_port_max: 55019,
    protocols: ["turn", "tcp"],
    username,
    credential_hash: sha256Label(credential),
    auth_secret_hash: sha256Label(authSecret),
  },
  bootstrap_vsock_port: 19093,
  expires_at_unix_ms: expiresAtUnixMs,
};
authority.binding_hash = sha256Label(JSON.stringify(canonical(authority)));
const secret = {
  schema: "elastos.browser.vz-transport-secret/v1",
  binding_hash: authority.binding_hash,
  credential,
  auth_secret: authSecret,
};
const streamSession = {
  schema: "elastos.exit.stream-session/v1",
  stream_id: streamId,
  target: "tls://did-not-act.invalid:443",
  byte_transport: "adapter_ipc",
  adapter_ipc: {
    schema: "elastos.adapter-ipc/v1",
    kind: "unix_socket",
    path: "/tmp/elastos-browser-vm-contract-adapter.sock",
    stream_id: streamId,
    runtime_stream_path: authority.egress.runtime_socket_path,
  },
};
const browserProfile = {
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
  disk_path: "/tmp/elastos-browser-vm-contract/BrowserProfiles/default/profile.ext4",
  reset: "whole_profile",
};
console.log(JSON.stringify({ op: "init", config }));
console.log(JSON.stringify({
  op: "launch",
  url: "https://did-not-act.invalid/",
  stream_session: streamSession,
  lifecycle_generation: generation,
  profile: browserProfile,
  adapter_id: "browser-vm-product",
  principal_id: principalId,
  reason: "verify exact VZ DidNotAct propagation",
  display_mode: "webrtc_remote_display",
  guarantee_level: "mechanism_microvm",
  page_id: pageId,
  vm_id: vmId,
  transport_authority: authority,
  transport_secret: secret,
}));
console.log(JSON.stringify({ op: "shutdown" }));
NODE
)"

settlement_output="$(printf '%s\n' "$settlement_request_json" | \
  ELASTOS_BROWSER_VM_DATA_DIR="$tmp_dir/Application Support/elastos" \
  ELASTOS_BROWSER_VM_ROOT="$tmp_dir/vm-root" \
  "$adapter_bin")"

OUTPUT="$settlement_output" "$node_bin" - <<'NODE'
const lines = process.env.OUTPUT.split(/\r?\n/).filter(Boolean).map((line) => JSON.parse(line));
const failure = lines.find((line) => line.status === "error" && line.launch_settlement_result);
if (!failure) throw new Error(`missing exact VZ settlement result: ${process.env.OUTPUT}`);
if (failure.adapter !== "browser-vm-product") throw new Error("selected Adapter identity was lost");
if (failure.launch_settlement_result.schema !== "elastos.browser.vz-launch-settlement/v1") {
  throw new Error("wrong VZ settlement schema");
}
if (failure.launch_settlement_result.state !== "did_not_act") {
  throw new Error("VZ pre-effect classification changed");
}
if (Object.values(failure.launch_settlement_result.effects || {}).some(Boolean)) {
  throw new Error("VZ DidNotAct settlement claimed an effect");
}
if (Object.values(failure.launch_settlement_result.absence || {}).some((value) => value !== true)) {
  throw new Error("VZ DidNotAct settlement omitted absence proof");
}
if (/credential|auth_secret|transport_secret/.test(process.env.OUTPUT)) {
  throw new Error("private VZ transport material escaped the Adapter");
}
NODE

if [[ -d "$tmp_dir/vm-root" ]] && find "$tmp_dir/vm-root" -mindepth 1 -print -quit | grep -q .; then
  echo "Browser VM wrapper left an unowned session directory" >&2
  find "$tmp_dir/vm-root" -mindepth 1 -maxdepth 2 -print >&2
  exit 1
fi

printf '%s\n' '{"schema":"elastos.browser.vm-engine-contract-smoke/v1","ok":true}'
