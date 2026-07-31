#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d /tmp/elastos-vm-persistent.XXXXXX)"
service_pid=""

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
  if [[ -n "$service_pid" ]]; then
    kill "$service_pid" >/dev/null 2>&1 || true
  fi
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

control_socket="$tmp_dir/browser-vm-control.sock"
fake_launcher="$tmp_dir/fake-persistent-vm-launcher.mjs"
proof_dir="$tmp_dir/p"
mkdir -p "$proof_dir"

cat > "$fake_launcher" <<'NODE'
#!/usr/bin/env node
import fs from "node:fs";
import http from "node:http";

const proofDir = process.env.PERSISTENT_LAUNCHER_PROOF_DIR;
const chunks = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
const launch = body.launch_request;
const suffix = launch.stream_id.replace(/[^A-Za-z0-9_-]/g, "_");
const pageId = `page:vm-persistent-smoke-${suffix}`;
const controlSocketPath = `${proofDir}/${suffix}.sock`;
const pages = new Set([pageId]);

fs.writeFileSync(`${proofDir}/${suffix}.started`, JSON.stringify({
  schema: "elastos.browser.vm-persistent-launcher-started/v1",
  stream_id: launch.stream_id,
  page_id: pageId,
}));

process.on("SIGTERM", () => {
  fs.writeFileSync(`${proofDir}/${suffix}.json`, JSON.stringify({
    schema: "elastos.browser.vm-persistent-launcher-proof/v1",
    terminated: true,
    stream_id: launch.stream_id,
    page_id: pageId,
    remaining_pages: [...pages],
  }));
  process.exit(0);
});

function pageResult(pageLaunch) {
  const resultPageId = `page:vm-persistent-smoke-${pageLaunch.stream_id.replace(/[^A-Za-z0-9_-]/g, "_")}`;
  return {
  schema: "elastos.browser.engine.supervisor-result/v1",
  page_id: resultPageId,
  adapter: pageLaunch.adapter,
  engine: pageLaunch.engine,
  stream_id: pageLaunch.stream_id,
  actual_url: pageLaunch.url,
  title: "Browser VM Persistent Launcher Smoke",
  network_mode: "runtime_net_only",
  direct_network: false,
  wallet_injection: false,
  control_socket_path: controlSocketPath,
  isolated_session: true,
  isolation: {
    schema: "elastos.browser.engine.isolation/v1",
    kind: "per_launch_vm_target",
    session_dir: "/tmp/elastos-browser-vm-sessions/vm-persistent-smoke",
  },
  process: {
    pid: process.pid,
    stream_bridge_pid: null,
  },
  display_session: {
    schema: "elastos.browser.display-session/v1",
    session_id: `display:${pageLaunch.stream_id}`,
    mode: "webrtc_remote_display",
    input: "datachannel",
    width: 1280,
    height: 720,
    offerer: "engine",
    initial_offer: {
      schema: "elastos.browser.webrtc-offer/v1",
      type: "offer",
      sdp: "v=0\r\ns=Browser VM Persistent Launcher Smoke\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n",
    },
    audio_offer: {
      schema: "elastos.browser.webrtc-offer/v1",
      type: "offer",
      sdp: "v=0\r\ns=Browser VM Persistent Launcher Smoke\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n",
    },
    display_backend: "vm_selkies_gstreamer_webrtc",
    backend_class: "product_compositor",
    media_transport: "runtime_relay",
    audio: true,
    video: true,
    network_mode: "runtime_net_only",
    direct_network: false,
    signaling_url: `/api/apps/browser/pages/${encodeURIComponent(resultPageId)}/webrtc`,
  },
  };
}

function readBody(req) {
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

function sendJson(res, status, body) {
  const bytes = Buffer.from(JSON.stringify(body));
  res.writeHead(status, {
    "content-type": "application/json",
    "content-length": bytes.length,
  });
  res.end(bytes);
}

const server = http.createServer(async (req, res) => {
  try {
    const url = new URL(req.url, "http://browser-vm");
    if (req.method === "POST" && url.pathname === "/pages") {
      const request = await readBody(req);
      if (request.schema !== "elastos.browser.vm-guest.open/v1") {
        sendJson(res, 400, { error: "wrong guest open schema" });
        return;
      }
      const pageLaunch = request.launch_request || {};
      const failPath = `${proofDir}/fail-next-guest-open`;
      if (fs.existsSync(failPath)) {
        fs.unlinkSync(failPath);
        sendJson(res, 503, { error: "guest open intentionally failed" });
        return;
      }
      if (pageLaunch.engine !== "selkies_gstreamer") {
        sendJson(res, 400, { error: "guest launch must use selkies_gstreamer" });
        return;
      }
      const result = pageResult(pageLaunch);
      pages.add(result.page_id);
      fs.writeFileSync(`${proofDir}/${pageLaunch.stream_id.replace(/[^A-Za-z0-9_-]/g, "_")}.guest-open`, JSON.stringify({
        schema: "elastos.browser.vm-persistent-guest-open-proof/v1",
        stream_id: pageLaunch.stream_id,
        page_id: result.page_id,
      }));
      sendJson(res, 200, result);
      return;
    }
    const closeMatch = url.pathname.match(/^\/pages\/([^/]+)\/close$/);
    if (req.method === "POST" && closeMatch) {
      const closePageId = decodeURIComponent(closeMatch[1]);
      const failPath = `${proofDir}/fail-next-guest-close`;
      if (fs.existsSync(failPath)) {
        fs.unlinkSync(failPath);
        sendJson(res, 404, { error: "browser page not found during close" });
        return;
      }
      pages.delete(closePageId);
      fs.writeFileSync(`${proofDir}/${closePageId.replace(/[^A-Za-z0-9_-]/g, "_")}.closed`, "ok\n");
      sendJson(res, 200, { schema: "elastos.browser.close-result/v1", page_id: closePageId, closed: true });
      return;
    }
    const inputMatch = url.pathname.match(/^\/pages\/([^/]+)\/input$/);
    if (req.method === "POST" && inputMatch) {
      const inputPageId = decodeURIComponent(inputMatch[1]);
      if (!pages.has(inputPageId)) {
        sendJson(res, 404, { error: "browser page not found" });
        return;
      }
      const input = await readBody(req);
      fs.writeFileSync(`${proofDir}/${inputPageId.replace(/[^A-Za-z0-9_-]/g, "_")}.input`, JSON.stringify(input));
      sendJson(res, 200, {
        schema: "elastos.browser.input-result/v1",
        page_id: inputPageId,
        accepted: true,
        direct_network: false,
      });
      return;
    }
    if (req.method === "GET" && url.pathname === "/logs") {
      sendJson(res, 200, {
        schema: "elastos.browser.selkies-control.logs/v1",
        logs: {
          "browser-vm-selkies.log": {
            present: true,
            bytes: 64,
            tail: "using ICE transport policy: relay\nupdating TURN server\n",
          },
        },
      });
      return;
    }
    const readMatch = url.pathname.match(/^\/pages\/([^/]+)\/(status|diagnostics)$/);
    if (req.method === "GET" && readMatch) {
      const readPageId = decodeURIComponent(readMatch[1]);
      if (!pages.has(readPageId)) {
        sendJson(res, 404, { error: "browser page not found" });
        return;
      }
      if (readMatch[2] === "status") {
        sendJson(res, 200, {
          schema: "elastos.browser.page-status/v1",
          page_id: readPageId,
          actual_url: "https://example.com/",
          title: "Browser VM Persistent Launcher Smoke",
          direct_network: false,
        });
        return;
      }
      sendJson(res, 200, {
        schema: "elastos.browser.page-diagnostics/v1",
        page_id: readPageId,
        url: "https://example.com/",
        title: "Browser VM Persistent Launcher Smoke",
        has_ethereum: true,
        direct_network: false,
      });
      return;
    }
    sendJson(res, 404, { error: "not found" });
  } catch (error) {
    sendJson(res, 500, { error: error instanceof Error ? error.message : String(error) });
  }
});

try {
  fs.unlinkSync(controlSocketPath);
} catch {}
server.listen(controlSocketPath, () => {
  console.log(JSON.stringify(pageResult(launch)));
});

setInterval(() => {}, 1000);
NODE
chmod 755 "$fake_launcher"

config_json="$(python3 - <<PY
import json
print(json.dumps({
    "schema": "elastos.browser.vm-control-service.config/v1",
    "control_socket_path": "$control_socket",
    "launcher_program": "$fake_launcher",
    "replace_existing_socket": True,
    "persistent_launcher": True,
    "max_active_pages": 1,
    "idle_vm_keepalive_ms": 2000,
    "reuse_idle_vms": True,
    "hibernation_mode": "vz_save_restore",
    "launch_timeout_ms": 30000,
    "shutdown_timeout_ms": 5000,
}))
PY
)"

ELASTOS_BROWSER_VM_CONTROL_SERVICE_CONFIG="$config_json" \
PERSISTENT_LAUNCHER_PROOF_DIR="$proof_dir" \
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

CONTROL_SOCKET="$control_socket" PROOF_DIR="$proof_dir" "$node_bin" - <<'NODE'
const http = require("node:http");
const fs = require("node:fs");
const socketPath = process.env.CONTROL_SOCKET;
const proofDir = process.env.PROOF_DIR;

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
        if (res.statusCode < 200 || res.statusCode >= 300) reject(new Error(parsed.error || `status ${res.statusCode}`));
        else resolve(parsed);
      });
    });
    req.on("error", reject);
    req.end(bytes);
  });
}

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

function openBody(streamId, overrides = {}) {
  const profile = {
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
    profile_key: overrides.profileKey || "profile-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    disk_path: overrides.profileDiskPath || "/tmp/elastos-browser-profile-test/BrowserProfiles/default/profile.ext4",
    reset: "whole_profile",
  };
  return {
    schema: "elastos.browser.vm-engine.open/v1",
    launch_request: {
      schema: "elastos.browser.engine.launch-request/v1",
      adapter: "browser-vm-product",
      engine: "chromium_microvm",
      url: "https://example.com/",
      stream_id: streamId,
      lifecycle_generation: `sha256:${streamId}`,
      target: "tls://example.com:443",
      principal_id: overrides.principalId || "person:local:vm-persistent-smoke",
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
    principal_id: page.stream_id.includes("principal-b")
      ? "person:local:vm-persistent-smoke-b"
      : "person:local:vm-persistent-smoke",
    control_socket_path: page.control_socket_path,
    shutdown_socket_path: socketPath,
    isolated_session: true,
    isolation: page.isolation,
    process: page.process,
  };
}

function shutdownBody(page) {
  return {
    page_id: page.page_id,
    runtime_cleanup: cleanupBinding(page),
    force_retire_vm: true,
  };
}

(async () => {
  const launch = await request("POST", "/pages", openBody("stream:vm-persistent-smoke"));
  if (launch.schema !== "elastos.browser.engine.supervisor-result/v1") throw new Error("wrong launch schema");
  if (launch.isolation?.kind !== "per_launch_vm_target") throw new Error("wrong isolation kind");

  const replay = await requestRaw("POST", "/pages", openBody("stream:vm-persistent-smoke"));
  if (replay.statusCode !== 400 || !String(replay.body.error || "").includes("identity already exist")) {
    throw new Error(`completed lifecycle identity replay did not fail closed: ${JSON.stringify(replay)}`);
  }
  const afterReplay = await request("GET", "/status");
  if (
    afterReplay.active_pages !== 1 ||
    afterReplay.active_vms !== 1 ||
    afterReplay.page_ids?.[0] !== launch.page_id ||
    afterReplay.active_stream_ids?.[0] !== launch.stream_id
  ) {
    throw new Error(`completed replay changed the healthy owner: ${JSON.stringify(afterReplay)}`);
  }

  const pageStatus = await request("GET", `/pages/${encodeURIComponent(launch.page_id)}/status`);
  if (pageStatus.page_id !== launch.page_id || pageStatus.actual_url !== "https://example.com/" || pageStatus.direct_network !== false) {
    throw new Error(`outer VM control did not proxy guest page status: ${JSON.stringify(pageStatus)}`);
  }
  const pageDiagnostics = await request("GET", `/pages/${encodeURIComponent(launch.page_id)}/diagnostics`);
  if (pageDiagnostics.schema !== "elastos.browser.page-diagnostics/v1" || pageDiagnostics.has_ethereum !== true || pageDiagnostics.direct_network !== false) {
    throw new Error(`outer VM control did not proxy guest page diagnostics: ${JSON.stringify(pageDiagnostics)}`);
  }
  const pageLogs = await request("GET", `/pages/${encodeURIComponent(launch.page_id)}/logs`);
  if (pageLogs.schema !== "elastos.browser.selkies-control.logs/v1" || pageLogs.logs?.["browser-vm-selkies.log"]?.present !== true) {
    throw new Error(`outer VM control did not proxy guest page logs: ${JSON.stringify(pageLogs)}`);
  }
  const pageInput = await request("POST", `/pages/${encodeURIComponent(launch.page_id)}/input`, {
    event: { type: "click", x: 10, y: 20 },
  });
  if (pageInput.schema !== "elastos.browser.input-result/v1" || pageInput.accepted !== true || pageInput.direct_network !== false) {
    throw new Error(`outer VM control did not proxy guest page input: ${JSON.stringify(pageInput)}`);
  }

  const launchCleanup = shutdownBody(launch);
  const shutdownLaunch = await request("POST", "/shutdown", launchCleanup);
  if (
    shutdownLaunch.schema !== "elastos.browser.supervisor-cleanup-result/v2" ||
    shutdownLaunch.terminal !== true ||
    Object.values(shutdownLaunch.effects || {}).some((value) => value !== true)
  ) {
    throw new Error(`explicit close did not prove exact terminal cleanup: ${JSON.stringify(shutdownLaunch)}`);
  }
  const second = await request("POST", "/pages", openBody("stream:vm-persistent-second-smoke"));
  if (second.page_id === launch.page_id) throw new Error("second stream reused first page id");
  if (second.stream_id !== "stream:vm-persistent-second-smoke") throw new Error("second stream launch returned wrong stream");
  if (second.engine !== "chromium_microvm") throw new Error("second stream lost VM supervisor identity");
  if (second.control_socket_path === launch.control_socket_path) throw new Error("terminal cleanup reused the retired VM control socket");

  const status = await request("GET", "/status");
  if (status.active_pages !== 1 || status.max_active_pages !== 1 || status.capacity_available !== false) {
    throw new Error(`wrong single-page persistent status: ${JSON.stringify(status)}`);
  }
  if (status.active_vms !== 1) throw new Error(`same-profile replacement must keep one active VM: ${JSON.stringify(status)}`);
  if (status.hibernation_mode !== "vz_save_restore") throw new Error(`persistent smoke should report hibernation mode: ${JSON.stringify(status)}`);

  const shutdownSecond = await request(
    "POST",
    "/shutdown",
    shutdownBody(second),
  );
  if (
    shutdownSecond.schema !== "elastos.browser.supervisor-cleanup-result/v2" ||
    shutdownSecond.terminal !== true
  ) {
    throw new Error(`second shutdown did not return terminal proof: ${JSON.stringify(shutdownSecond)}`);
  }
  const warmStatus = await request("GET", "/status");
  if (warmStatus.active_pages !== 0 || warmStatus.active_vms !== 0 || warmStatus.warm_vms !== 0 || warmStatus.capacity_available !== true) {
    throw new Error(`closed page retained VM ownership: ${JSON.stringify(warmStatus)}`);
  }
  if (warmStatus.idle_vm_keepalive_ms !== 2000) {
    throw new Error(`status did not expose idle VM keepalive: ${JSON.stringify(warmStatus)}`);
  }
  if (warmStatus.reuse_idle_vms !== true) {
    throw new Error(`status did not expose explicit idle VM reuse opt-in: ${JSON.stringify(warmStatus)}`);
  }
  if (warmStatus.hibernation_mode !== "vz_save_restore") {
    throw new Error(`warm VM status did not expose hibernation mode: ${JSON.stringify(warmStatus)}`);
  }
  if ((warmStatus.lifecycle?.sessions || []).length !== 0) {
    throw new Error(`terminal cleanup retained lifecycle sessions: ${JSON.stringify(warmStatus.lifecycle)}`);
  }

  const routeChanged = await request("POST", "/pages", openBody("remote-carrier:seed-node-linux:seed-smoke:1"));
  if (routeChanged.stream_id !== "remote-carrier:seed-node-linux:seed-smoke:1") {
    throw new Error(`route-change launch returned wrong stream: ${JSON.stringify(routeChanged)}`);
  }
  if (routeChanged.control_socket_path === second.control_socket_path) {
    throw new Error("same-profile route change reused a terminally closed VM control socket");
  }
  const routeStatus = await request("GET", "/status");
  if (routeStatus.active_pages !== 1 || routeStatus.active_vms !== 1 || routeStatus.warm_vms !== 0) {
    throw new Error(`same-profile route change did not reuse idle VM cleanly: ${JSON.stringify(routeStatus)}`);
  }
  const routeLifecycle = routeStatus.lifecycle?.sessions?.find((session) => session.warm_vm === false);
  if (!routeLifecycle || routeLifecycle.phase !== "ACTIVE_SESSION" || !String(routeLifecycle.exit_id || "").startsWith("remote-carrier:sha256:")) {
    throw new Error(`same-profile route change did not expose the remote exit as page routing state: ${JSON.stringify(routeStatus.lifecycle)}`);
  }
  const shutdownRouteChanged = await request(
    "POST",
    "/shutdown",
    shutdownBody(routeChanged),
  );
  if (
    shutdownRouteChanged.schema !== "elastos.browser.supervisor-cleanup-result/v2" ||
    shutdownRouteChanged.terminal !== true
  ) {
    throw new Error(`route-changed shutdown did not prove terminal cleanup: ${JSON.stringify(shutdownRouteChanged)}`);
  }
  const afterRouteWarmStatus = await request("GET", "/status");
  if (afterRouteWarmStatus.active_pages !== 0 || afterRouteWarmStatus.active_vms !== 0 || afterRouteWarmStatus.warm_vms !== 0) {
    throw new Error(`route-changed shutdown retained VM state: ${JSON.stringify(afterRouteWarmStatus)}`);
  }

  const principalChanged = await request("POST", "/pages", openBody("stream:vm-persistent-principal-b-smoke", {
    principalId: "person:local:vm-persistent-smoke-b",
    profileKey: "profile-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    profileDiskPath: "/tmp/elastos-browser-profile-test-b/BrowserProfiles/default/profile.ext4",
  }));
  if (principalChanged.control_socket_path === second.control_socket_path) {
    throw new Error("different principal/profile reused the previous warm VM control socket");
  }
  const principalChangedStatus = await request("GET", "/status");
  if (principalChangedStatus.active_pages !== 1 || principalChangedStatus.active_vms !== 1 || principalChangedStatus.warm_vms !== 0) {
    throw new Error(`different principal/profile launch retained a non-reusable idle VM: ${JSON.stringify(principalChangedStatus)}`);
  }
  const retiredProofPath = `${proofDir}/stream_vm-persistent-second-smoke.json`;
  for (let attempt = 0; attempt < 100 && !fs.existsSync(retiredProofPath); attempt += 1) {
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  if (!fs.existsSync(retiredProofPath)) {
    throw new Error("different principal/profile launch did not terminate the previous idle VM");
  }
  const shutdownPrincipalChanged = await request(
    "POST",
    "/shutdown",
    shutdownBody(principalChanged),
  );
  if (
    shutdownPrincipalChanged.schema !== "elastos.browser.supervisor-cleanup-result/v2" ||
    shutdownPrincipalChanged.terminal !== true
  ) {
    throw new Error(`principal-changed shutdown did not prove terminal cleanup: ${JSON.stringify(shutdownPrincipalChanged)}`);
  }

  await new Promise((resolve) => setTimeout(resolve, 2600));
  const idleStatus = await request("GET", "/status");
  if (idleStatus.active_pages !== 0 || idleStatus.active_vms !== 0 || idleStatus.warm_vms !== 0) {
    throw new Error(`idle VM keepalive did not expire cleanly: ${JSON.stringify(idleStatus)}`);
  }

  const closeFailurePage = await request(
    "POST",
    "/pages",
    openBody("stream:vm-persistent-close-failure-smoke"),
  );
  fs.writeFileSync(`${proofDir}/fail-next-guest-close`, "fail\n");
  const closeFailureCleanup = shutdownBody(closeFailurePage);
  const forcedClose = await request("POST", "/shutdown", closeFailureCleanup);
  if (
    forcedClose.schema !== "elastos.browser.supervisor-cleanup-result/v2" ||
    forcedClose.terminal !== true ||
    forcedClose.forced_vm_retirement !== true
  ) {
    throw new Error(`failed guest close did not force terminal VM retirement: ${JSON.stringify(forcedClose)}`);
  }
  const afterForcedClose = await request("GET", "/status");
  if (
    afterForcedClose.active_pages !== 0 ||
    afterForcedClose.active_vms !== 0 ||
    afterForcedClose.warm_vms !== 0
  ) {
    throw new Error(`failed guest close leaked reusable state: ${JSON.stringify(afterForcedClose)}`);
  }
  const staleClose = await request("POST", "/shutdown", closeFailureCleanup);
  if (
    staleClose.schema !== "elastos.browser.supervisor-cleanup-result/v2" ||
    staleClose.terminal !== true ||
    staleClose.already_absent !== true
  ) {
    throw new Error(`typed already-absent close did not remain terminal: ${JSON.stringify(staleClose)}`);
  }
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
NODE

if [[ ! -f "$proof_dir/stream_vm-persistent-second-smoke.started" ]]; then
  echo "same-profile second page did not spawn a fresh persistent launcher after terminal cleanup" >&2
  exit 1
fi

for proof in \
  "page_vm-persistent-smoke-stream_vm-persistent-smoke.closed" \
  "page_vm-persistent-smoke-stream_vm-persistent-second-smoke.closed" \
  "page_vm-persistent-smoke-remote-carrier_seed-node-linux_seed-smoke_1.closed"; do
  proof_path="$proof_dir/$proof"
  for _ in {1..100}; do
    [[ -f "$proof_path" ]] && break
    sleep 0.05
  done
  [[ -f "$proof_path" ]] || {
    echo "missing shared VM proof $proof" >&2
    exit 1
  }
done

for suffix in stream_vm-persistent-smoke; do
  proof_path="$proof_dir/${suffix}.json"
  for _ in {1..100}; do
    [[ -f "$proof_path" ]] && break
    sleep 0.05
  done
  [[ -f "$proof_path" ]] || {
    echo "persistent launcher was not terminated for $suffix" >&2
    exit 1
  }
done

for suffix in stream_vm-persistent-second-smoke; do
  proof_path="$proof_dir/${suffix}.json"
  for _ in {1..100}; do
    [[ -f "$proof_path" ]] && break
    sleep 0.05
  done
  [[ -f "$proof_path" ]] || {
    echo "replacement persistent launcher was not terminated for $suffix" >&2
    exit 1
  }
done

for suffix in stream_vm-persistent-principal-b-smoke; do
  proof_path="$proof_dir/${suffix}.json"
  for _ in {1..100}; do
    [[ -f "$proof_path" ]] && break
    sleep 0.05
  done
  [[ -f "$proof_path" ]] || {
    echo "principal-changed persistent launcher was not terminated for $suffix" >&2
    exit 1
  }
done

for suffix in stream_vm-persistent-close-failure-smoke; do
  proof_path="$proof_dir/${suffix}.json"
  for _ in {1..100}; do
    [[ -f "$proof_path" ]] && break
    sleep 0.05
  done
  [[ -f "$proof_path" ]] || {
    echo "failed close did not terminate persistent launcher for $suffix" >&2
    exit 1
  }
done

PROOF_DIR="$proof_dir" "$node_bin" - <<'NODE'
const fs = require("node:fs");
for (const [file, streamId] of [
  ["stream_vm-persistent-smoke.json", "stream:vm-persistent-smoke"],
  ["stream_vm-persistent-second-smoke.json", "stream:vm-persistent-second-smoke"],
  ["stream_vm-persistent-principal-b-smoke.json", "stream:vm-persistent-principal-b-smoke"],
  ["stream_vm-persistent-close-failure-smoke.json", "stream:vm-persistent-close-failure-smoke"],
]) {
  const proof = JSON.parse(fs.readFileSync(`${process.env.PROOF_DIR}/${file}`, "utf8"));
  if (proof.schema !== "elastos.browser.vm-persistent-launcher-proof/v1") throw new Error("wrong proof schema");
  if (proof.terminated !== true) throw new Error("persistent launcher was not terminated");
  if (proof.stream_id !== streamId) throw new Error("wrong stream id");
}
NODE

kill "$service_pid" >/dev/null 2>&1 || true
wait "$service_pid" 2>/dev/null || true
service_pid=""

reuse_failure_control_socket="$tmp_dir/browser-vm-control-reuse-failure.sock"
reuse_failure_proof_dir="$tmp_dir/p-reuse-failure"
mkdir -p "$reuse_failure_proof_dir"

reuse_failure_config_json="$(python3 - <<PY
import json
print(json.dumps({
    "schema": "elastos.browser.vm-control-service.config/v1",
    "control_socket_path": "$reuse_failure_control_socket",
    "launcher_program": "$fake_launcher",
    "persistent_launcher": True,
    "max_active_pages": 2,
    "idle_vm_keepalive_ms": 2000,
    "reuse_idle_vms": True,
    "launch_timeout_ms": 30000,
    "shutdown_timeout_ms": 5000,
}))
PY
)"

ELASTOS_BROWSER_VM_CONTROL_SERVICE_CONFIG="$reuse_failure_config_json" \
PERSISTENT_LAUNCHER_PROOF_DIR="$reuse_failure_proof_dir" \
  "$node_bin" "$repo_root/scripts/browser-vm-control-service.mjs" > "$tmp_dir/service-reuse-failure.out" 2> "$tmp_dir/service-reuse-failure.err" &
service_pid="$!"

for _ in {1..100}; do
  [[ -S "$reuse_failure_control_socket" ]] && break
  sleep 0.05
done
if [[ ! -S "$reuse_failure_control_socket" ]]; then
  cat "$tmp_dir/service-reuse-failure.err" >&2 || true
  exit 1
fi

CONTROL_SOCKET="$reuse_failure_control_socket" \
PROOF_DIR="$reuse_failure_proof_dir" \
  "$node_bin" - <<'NODE'
const fs = require("node:fs");
const http = require("node:http");
const socketPath = process.env.CONTROL_SOCKET;
const proofDir = process.env.PROOF_DIR;

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
    throw new Error(response.body.error || response.body.message || `status ${response.statusCode}`);
  }
  return response.body;
}

const profile = {
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
  profile_key: "profile-cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
  disk_path: "/tmp/elastos-browser-profile-reuse-failure/BrowserProfiles/default/profile.ext4",
  reset: "whole_profile",
};

const openBody = (streamId) => ({
  schema: "elastos.browser.vm-engine.open/v1",
  launch_request: {
    schema: "elastos.browser.engine.launch-request/v1",
    adapter: "browser-vm-product",
    engine: "chromium_microvm",
    url: "https://example.com/",
    stream_id: streamId,
    lifecycle_generation: `sha256:${streamId}`,
    target: "tls://example.com:443",
    principal_id: "person:local:vm-persistent-reuse-failure",
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
});

const reconcile = (streamId) =>
  request("POST", "/launches/reconcile", {
    schema: "elastos.browser.vm-control-service.reconcile-launch/v1",
    lifecycle_generation: `sha256:${streamId}`,
    stream_id: streamId,
  });

const cleanupBody = (page) => ({
  page_id: page.page_id,
  force_retire_vm: true,
  runtime_cleanup: {
    schema: "elastos.browser.engine-cleanup-binding/v2",
    page_id: page.page_id,
    generation: `sha256:${page.stream_id}`,
    stream_id: page.stream_id,
    adapter: page.adapter,
    engine: page.engine,
    display_mode: "webrtc_remote_display",
    guarantee_level: "mechanism_microvm",
    principal_id: "person:local:vm-persistent-reuse-failure",
    control_socket_path: page.control_socket_path,
    shutdown_socket_path: socketPath,
    isolated_session: true,
    isolation: page.isolation,
    process: page.process,
  },
});

(async () => {
  const first = await request(
    "POST",
    "/pages",
    openBody("stream:vm-reuse-failure-owner"),
  );
  fs.writeFileSync(`${proofDir}/fail-next-guest-open`, "fail\n");
  const failed = await requestRaw(
    "POST",
    "/pages",
    openBody("stream:vm-reuse-failure-pending"),
  );
  if (
    failed.statusCode !== 400 ||
    !String(failed.body.error || "").includes("guest open intentionally failed")
  ) {
    throw new Error(`reused guest-open failure changed: ${JSON.stringify(failed)}`);
  }
  const pending = await reconcile("stream:vm-reuse-failure-pending");
  if (
    pending.state !== "cleanup_pending" ||
    pending.effects?.page_acquired !== null ||
    pending.effects?.vm_acquired !== null
  ) {
    throw new Error(`unreaped reused failure was not retained: ${JSON.stringify(pending)}`);
  }
  const stillOwned = await request("GET", "/status");
  if (stillOwned.active_pages !== 1 || stillOwned.active_vms !== 1) {
    throw new Error(`cleanup failure disturbed the healthy page owner: ${JSON.stringify(stillOwned)}`);
  }
  await request("POST", "/shutdown", cleanupBody(first));
  const terminal = await reconcile("stream:vm-reuse-failure-pending");
  if (
    terminal.state !== "terminal_post_effect_cleanup" ||
    terminal.effects?.page_acquired !== true ||
    terminal.effects?.vm_acquired !== true
  ) {
    throw new Error(`exact VM reap did not terminally settle reused failure: ${JSON.stringify(terminal)}`);
  }
  const replacement = await request(
    "POST",
    "/pages",
    openBody("stream:vm-reuse-failure-replacement"),
  );
  if (replacement.stream_id !== "stream:vm-reuse-failure-replacement") {
    throw new Error(`replacement remained blocked after terminal proof: ${JSON.stringify(replacement)}`);
  }
  await request("POST", "/shutdown", cleanupBody(replacement));
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
NODE

kill "$service_pid" >/dev/null 2>&1 || true
wait "$service_pid" 2>/dev/null || true
service_pid=""

no_reuse_control_socket="$tmp_dir/browser-vm-control-no-reuse.sock"
no_reuse_proof_dir="$tmp_dir/p-no-reuse"
mkdir -p "$no_reuse_proof_dir"

no_reuse_config_json="$(python3 - <<PY
import json
print(json.dumps({
    "schema": "elastos.browser.vm-control-service.config/v1",
    "control_socket_path": "$no_reuse_control_socket",
    "launcher_program": "$fake_launcher",
    "replace_existing_socket": True,
    "persistent_launcher": True,
    "max_active_pages": 1,
    "idle_vm_keepalive_ms": 2000,
    "launch_timeout_ms": 30000,
    "shutdown_timeout_ms": 5000,
}))
PY
)"

ELASTOS_BROWSER_VM_CONTROL_SERVICE_CONFIG="$no_reuse_config_json" \
PERSISTENT_LAUNCHER_PROOF_DIR="$no_reuse_proof_dir" \
  "$node_bin" "$repo_root/scripts/browser-vm-control-service.mjs" > "$tmp_dir/service-no-reuse.out" 2> "$tmp_dir/service-no-reuse.err" &
service_pid="$!"

for _ in {1..100}; do
  [[ -S "$no_reuse_control_socket" ]] && break
  sleep 0.05
done
if [[ ! -S "$no_reuse_control_socket" ]]; then
  cat "$tmp_dir/service-no-reuse.err" >&2 || true
  exit 1
fi

CONTROL_SOCKET="$no_reuse_control_socket" PROOF_DIR="$no_reuse_proof_dir" "$node_bin" - <<'NODE'
const http = require("node:http");
const fs = require("node:fs");
const socketPath = process.env.CONTROL_SOCKET;
const proofDir = process.env.PROOF_DIR;

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
        if (res.statusCode < 200 || res.statusCode >= 300) reject(new Error(parsed.error || `status ${res.statusCode}`));
        else resolve(parsed);
      });
    });
    req.on("error", reject);
    req.end(bytes);
  });
}

function openBody(streamId) {
  const profile = {
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
    disk_path: "/tmp/elastos-browser-profile-test/BrowserProfiles/default/profile.ext4",
    reset: "whole_profile",
  };
  return {
    schema: "elastos.browser.vm-engine.open/v1",
    launch_request: {
      schema: "elastos.browser.engine.launch-request/v1",
      adapter: "browser-vm-product",
      engine: "chromium_microvm",
      url: "https://example.com/",
      stream_id: streamId,
      lifecycle_generation: `sha256:${streamId}`,
      target: "tls://example.com:443",
      principal_id: "person:local:vm-persistent-no-reuse-smoke",
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

function shutdownBody(page) {
  return {
    page_id: page.page_id,
    runtime_cleanup: {
      schema: "elastos.browser.engine-cleanup-binding/v2",
      page_id: page.page_id,
      generation: `sha256:${page.stream_id}`,
      stream_id: page.stream_id,
      adapter: page.adapter,
      engine: page.engine,
      display_mode: "webrtc_remote_display",
      guarantee_level: "mechanism_microvm",
      principal_id: "person:local:vm-persistent-no-reuse-smoke",
      control_socket_path: page.control_socket_path,
      shutdown_socket_path: socketPath,
      isolated_session: true,
      isolation: page.isolation,
      process: page.process,
    },
    force_retire_vm: true,
  };
}

(async () => {
  const launch = await request("POST", "/pages", openBody("stream:vm-no-reuse-smoke"));
  const shutdown = await request("POST", "/shutdown", shutdownBody(launch));
  if (
    shutdown.schema !== "elastos.browser.supervisor-cleanup-result/v2" ||
    shutdown.terminal !== true ||
    Object.values(shutdown.effects || {}).some((value) => value !== true)
  ) {
    throw new Error(`terminal cleanup proof was incomplete: ${JSON.stringify(shutdown)}`);
  }
  const status = await request("GET", "/status");
  if (status.active_pages !== 0 || status.active_vms !== 0 || status.warm_vms !== 0) {
    throw new Error(`idle reuse disabled leaked warm VM state: ${JSON.stringify(status)}`);
  }
  if (status.idle_vm_keepalive_ms !== 2000 || status.reuse_idle_vms !== false) {
    throw new Error(`status did not expose fail-closed idle reuse state: ${JSON.stringify(status)}`);
  }
  const proofPath = `${proofDir}/stream_vm-no-reuse-smoke.json`;
  for (let attempt = 0; attempt < 100 && !fs.existsSync(proofPath); attempt += 1) {
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  if (!fs.existsSync(proofPath)) {
    throw new Error("no-reuse shutdown did not terminate the persistent launcher");
  }
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
NODE

kill "$service_pid" >/dev/null 2>&1 || true
wait "$service_pid" 2>/dev/null || true
service_pid=""

invalid_control_socket="$tmp_dir/browser-vm-control-invalid.sock"
invalid_launcher="$tmp_dir/fake-invalid-persistent-vm-launcher.mjs"
invalid_proof_path="$tmp_dir/persistent-launcher-invalid-proof.json"

cat > "$invalid_launcher" <<'NODE'
#!/usr/bin/env node
import fs from "node:fs";

const proofPath = process.env.PERSISTENT_LAUNCHER_PROOF;

process.on("SIGTERM", () => {
  fs.writeFileSync(proofPath, JSON.stringify({
    schema: "elastos.browser.vm-persistent-launcher-invalid-proof/v1",
    terminated: true,
  }));
  process.exit(0);
});

console.log("{not-json");
setInterval(() => {}, 1000);
NODE
chmod 755 "$invalid_launcher"

invalid_config_json="$(python3 - <<PY
import json
print(json.dumps({
    "schema": "elastos.browser.vm-control-service.config/v1",
    "control_socket_path": "$invalid_control_socket",
    "launcher_program": "$invalid_launcher",
    "replace_existing_socket": True,
    "persistent_launcher": True,
    "launch_timeout_ms": 30000,
    "shutdown_timeout_ms": 5000,
}))
PY
)"

ELASTOS_BROWSER_VM_CONTROL_SERVICE_CONFIG="$invalid_config_json" \
PERSISTENT_LAUNCHER_PROOF="$invalid_proof_path" \
  "$node_bin" "$repo_root/scripts/browser-vm-control-service.mjs" > "$tmp_dir/service-invalid.out" 2> "$tmp_dir/service-invalid.err" &
service_pid="$!"

for _ in {1..100}; do
  [[ -S "$invalid_control_socket" ]] && break
  sleep 0.05
done
if [[ ! -S "$invalid_control_socket" ]]; then
  cat "$tmp_dir/service-invalid.err" >&2 || true
  exit 1
fi

CONTROL_SOCKET="$invalid_control_socket" "$node_bin" - <<'NODE'
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
        resolve({ statusCode: res.statusCode, body: parsed });
      });
    });
    req.on("error", reject);
    req.end(bytes);
  });
}

(async () => {
  const response = await request("POST", "/pages", {
    schema: "elastos.browser.vm-engine.open/v1",
    launch_request: {
      schema: "elastos.browser.engine.launch-request/v1",
      adapter: "browser-vm-product",
      engine: "chromium_microvm",
      url: "https://example.com/",
      stream_id: "stream:vm-persistent-invalid-smoke",
      lifecycle_generation: "sha256:vm-persistent-invalid-smoke",
      target: "tls://example.com:443",
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
  });
  if (response.statusCode !== 400) throw new Error(`expected 400, got ${response.statusCode}`);
  if (!String(response.body.error || "").includes("Browser VM launcher output is not JSON")) {
    throw new Error(`wrong error: ${JSON.stringify(response.body)}`);
  }
  const reconciliation = await request("POST", "/launches/reconcile", {
    schema: "elastos.browser.vm-control-service.reconcile-launch/v1",
    lifecycle_generation: "sha256:vm-persistent-invalid-smoke",
    stream_id: "stream:vm-persistent-invalid-smoke",
  });
  if (
    reconciliation.statusCode !== 200 ||
    reconciliation.body.state !== "terminal_post_effect_cleanup"
  ) {
    throw new Error(`invalid launcher was released before exact reap: ${JSON.stringify(reconciliation)}`);
  }
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
NODE

for _ in {1..100}; do
  [[ -f "$invalid_proof_path" ]] && break
  sleep 0.05
done
[[ -f "$invalid_proof_path" ]] || {
  echo "invalid persistent launcher was not terminated" >&2
  exit 1
}

PROOF_PATH="$invalid_proof_path" "$node_bin" - <<'NODE'
const fs = require("node:fs");
const proof = JSON.parse(fs.readFileSync(process.env.PROOF_PATH, "utf8"));
if (proof.schema !== "elastos.browser.vm-persistent-launcher-invalid-proof/v1") throw new Error("wrong invalid proof schema");
if (proof.terminated !== true) throw new Error("invalid persistent launcher was not terminated");
NODE

kill "$service_pid" >/dev/null 2>&1 || true
wait "$service_pid" 2>/dev/null || true
service_pid=""

timeout_control_socket="$tmp_dir/browser-vm-control-timeout.sock"
timeout_launcher="$tmp_dir/fake-timeout-persistent-vm-launcher.mjs"

cat > "$timeout_launcher" <<'NODE'
#!/usr/bin/env node
console.error("browser-vz-engine-supervisor stage=open_guest_page_start");
setInterval(() => {}, 1000);
NODE
chmod 755 "$timeout_launcher"

timeout_config_json="$(python3 - <<PY
import json
print(json.dumps({
    "schema": "elastos.browser.vm-control-service.config/v1",
    "control_socket_path": "$timeout_control_socket",
    "launcher_program": "$timeout_launcher",
    "replace_existing_socket": True,
    "persistent_launcher": True,
    "launch_timeout_ms": 1000,
    "shutdown_timeout_ms": 1000,
}))
PY
)"

ELASTOS_BROWSER_VM_CONTROL_SERVICE_CONFIG="$timeout_config_json" \
  "$node_bin" "$repo_root/scripts/browser-vm-control-service.mjs" > "$tmp_dir/service-timeout.out" 2> "$tmp_dir/service-timeout.err" &
service_pid="$!"

for _ in {1..100}; do
  [[ -S "$timeout_control_socket" ]] && break
  sleep 0.05
done
if [[ ! -S "$timeout_control_socket" ]]; then
  cat "$tmp_dir/service-timeout.err" >&2 || true
  exit 1
fi

CONTROL_SOCKET="$timeout_control_socket" "$node_bin" - <<'NODE'
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
        resolve({ statusCode: res.statusCode, body: parsed });
      });
    });
    req.on("error", reject);
    req.end(bytes);
  });
}

(async () => {
  const response = await request("POST", "/pages", {
    schema: "elastos.browser.vm-engine.open/v1",
    launch_request: {
      schema: "elastos.browser.engine.launch-request/v1",
      adapter: "browser-vm-product",
      engine: "chromium_microvm",
      url: "https://example.com/",
      stream_id: "stream:vm-persistent-timeout-smoke",
      lifecycle_generation: "sha256:vm-persistent-timeout-smoke",
      target: "tls://example.com:443",
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
  });
  if (response.statusCode !== 400) throw new Error(`expected timeout 400, got ${response.statusCode}`);
  const error = String(response.body.error || "");
  if (!error.includes("Browser VM persistent launcher timed out")) {
    throw new Error(`wrong timeout error: ${JSON.stringify(response.body)}`);
  }
  if (!error.includes("stage=open_guest_page_start")) {
    throw new Error(`timeout error did not include launcher stderr: ${JSON.stringify(response.body)}`);
  }
  const reconciliation = await request("POST", "/launches/reconcile", {
    schema: "elastos.browser.vm-control-service.reconcile-launch/v1",
    lifecycle_generation: "sha256:vm-persistent-timeout-smoke",
    stream_id: "stream:vm-persistent-timeout-smoke",
  });
  if (
    reconciliation.statusCode !== 200 ||
    reconciliation.body.state !== "terminal_post_effect_cleanup"
  ) {
    throw new Error(`timed-out launcher was released before exact reap: ${JSON.stringify(reconciliation)}`);
  }
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
NODE

kill "$service_pid" >/dev/null 2>&1 || true
wait "$service_pid" 2>/dev/null || true
service_pid=""

abort_control_socket="$tmp_dir/browser-vm-control-abort.sock"
abort_launcher="$tmp_dir/fake-abort-persistent-vm-launcher.mjs"
abort_proof_path="$tmp_dir/persistent-launcher-abort-proof.json"
abort_ready_path="$tmp_dir/persistent-launcher-abort-ready"

cat > "$abort_launcher" <<'NODE'
#!/usr/bin/env node
import fs from "node:fs";

const proofPath = process.env.PERSISTENT_LAUNCHER_PROOF;
const readyPath = process.env.PERSISTENT_LAUNCHER_READY;
const chunks = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
const launch = body.launch_request;

process.on("SIGTERM", () => {
  fs.writeFileSync(proofPath, JSON.stringify({
    schema: "elastos.browser.vm-persistent-launcher-abort-proof/v1",
    terminated: true,
    stream_id: launch.stream_id,
  }));
  process.exit(0);
});

console.error("browser-vz-engine-supervisor stage=waiting_for_guest_ready");
fs.writeFileSync(readyPath, "ready\n");
setInterval(() => {}, 1000);
NODE
chmod 755 "$abort_launcher"

abort_config_json="$(python3 - <<PY
import json
print(json.dumps({
    "schema": "elastos.browser.vm-control-service.config/v1",
    "control_socket_path": "$abort_control_socket",
    "launcher_program": "$abort_launcher",
    "replace_existing_socket": True,
    "persistent_launcher": True,
    "launch_timeout_ms": 30000,
    "shutdown_timeout_ms": 1000,
}))
PY
)"

ELASTOS_BROWSER_VM_CONTROL_SERVICE_CONFIG="$abort_config_json" \
PERSISTENT_LAUNCHER_PROOF="$abort_proof_path" \
PERSISTENT_LAUNCHER_READY="$abort_ready_path" \
  "$node_bin" "$repo_root/scripts/browser-vm-control-service.mjs" > "$tmp_dir/service-abort.out" 2> "$tmp_dir/service-abort.err" &
service_pid="$!"

for _ in {1..100}; do
  [[ -S "$abort_control_socket" ]] && break
  sleep 0.05
done
if [[ ! -S "$abort_control_socket" ]]; then
  cat "$tmp_dir/service-abort.err" >&2 || true
  exit 1
fi

CONTROL_SOCKET="$abort_control_socket" READY_PATH="$abort_ready_path" "$node_bin" - <<'NODE'
const http = require("node:http");
const fs = require("node:fs");
const socketPath = process.env.CONTROL_SOCKET;
const readyPath = process.env.READY_PATH;

function openBody() {
  return {
    schema: "elastos.browser.vm-engine.open/v1",
    launch_request: {
      schema: "elastos.browser.engine.launch-request/v1",
      adapter: "browser-vm-product",
      engine: "chromium_microvm",
      url: "https://example.com/",
      stream_id: "stream:vm-persistent-abort-smoke",
      lifecycle_generation: "sha256:vm-persistent-abort-smoke",
      target: "tls://example.com:443",
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
  };
}

function getStatus() {
  return new Promise((resolve, reject) => {
    const req = http.request({ socketPath, path: "/status", method: "GET" }, (res) => {
      const chunks = [];
      res.on("data", (chunk) => chunks.push(chunk));
      res.on("end", () => {
        try {
          resolve(JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}"));
        } catch (error) {
          reject(error);
        }
      });
    });
    req.on("error", reject);
    req.end();
  });
}

function reconcile() {
  const bytes = Buffer.from(JSON.stringify({
    schema: "elastos.browser.vm-control-service.reconcile-launch/v1",
    lifecycle_generation: "sha256:vm-persistent-abort-smoke",
    stream_id: "stream:vm-persistent-abort-smoke",
  }));
  return new Promise((resolve, reject) => {
    const req = http.request({
      socketPath,
      path: "/launches/reconcile",
      method: "POST",
      headers: {
        "content-type": "application/json",
        "content-length": bytes.length,
      },
    }, (res) => {
      const chunks = [];
      res.on("data", (chunk) => chunks.push(chunk));
      res.on("end", () => {
        try {
          resolve(JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}"));
        } catch (error) {
          reject(error);
        }
      });
    });
    req.on("error", reject);
    req.end(bytes);
  });
}

async function waitForStatus(predicate, label) {
  const deadline = Date.now() + 5000;
  let lastStatus = null;
  while (Date.now() < deadline) {
    lastStatus = await getStatus();
    if (predicate(lastStatus)) return lastStatus;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`${label}: ${JSON.stringify(lastStatus)}`);
}

async function waitForFile(file, label) {
  const deadline = Date.now() + 5000;
  while (Date.now() < deadline) {
    if (fs.existsSync(file)) return;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(label);
}

(async () => {
  const bytes = Buffer.from(JSON.stringify(openBody()));
  const req = http.request({
    socketPath,
    path: "/pages",
    method: "POST",
    headers: {
      "content-type": "application/json",
      "content-length": bytes.length,
    },
  });
  req.on("error", () => {});
  req.end(bytes);

  await waitForStatus((status) => status.pending_launches === 1, "pending launch did not appear");
  await waitForFile(readyPath, "persistent abort launcher did not become ready");
  req.destroy(new Error("client canceled launch"));
  const settled = await waitForStatus((status) => status.pending_launches === 0, "pending launch did not clear after client cancel");
  if (settled.active_pages !== 0) throw new Error(`aborted launch left active pages: ${JSON.stringify(settled)}`);
  const reconciliation = await reconcile();
  if (reconciliation.state !== "terminal_post_effect_cleanup") {
    throw new Error(`canceled launcher was released before exact reap: ${JSON.stringify(reconciliation)}`);
  }
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
NODE

for _ in {1..100}; do
  [[ -f "$abort_proof_path" ]] && break
  sleep 0.05
done
[[ -f "$abort_proof_path" ]] || {
  echo "aborted persistent launcher was not terminated" >&2
  exit 1
}

PROOF_PATH="$abort_proof_path" "$node_bin" - <<'NODE'
const fs = require("node:fs");
const proof = JSON.parse(fs.readFileSync(process.env.PROOF_PATH, "utf8"));
if (proof.schema !== "elastos.browser.vm-persistent-launcher-abort-proof/v1") throw new Error("wrong abort proof schema");
if (proof.terminated !== true) throw new Error("aborted persistent launcher was not terminated");
if (proof.stream_id !== "stream:vm-persistent-abort-smoke") throw new Error("wrong abort stream id");
NODE

kill "$service_pid" >/dev/null 2>&1 || true
wait "$service_pid" 2>/dev/null || true
service_pid=""

printf '%s\n' '{"schema":"elastos.browser.vm-control-service-persistent-smoke/v1","ok":true}'
