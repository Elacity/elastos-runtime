#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"
pid_a=""
pid_b=""
last_pid=""

cleanup() {
  if [[ -n "$pid_a" ]]; then
    kill "$pid_a" >/dev/null 2>&1 || true
  fi
  if [[ -n "$pid_b" ]]; then
    kill "$pid_b" >/dev/null 2>&1 || true
  fi
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

cd "$repo_root"

service_home="${ELASTOS_BROWSER_SERVICE_HOME:-$tmp_dir/service-home}"
service_xdg_data_home="${ELASTOS_BROWSER_SERVICE_XDG_DATA_HOME:-$service_home/xdg-data}"
browser_program="${ELASTOS_BROWSER_SELKIES_BROWSER_PROGRAM:-}"
if [[ -z "$browser_program" || ! -x "$browser_program" ]]; then
  echo "ELASTOS_BROWSER_SELKIES_BROWSER_PROGRAM must point at an executable Chromium binary" >&2
  exit 2
fi
cargo_target_dir="${ELASTOS_BROWSER_SELKIES_CARGO_TARGET_DIR:-$tmp_dir/cargo-target}"

CARGO_TARGET_DIR="$cargo_target_dir" cargo build --quiet --manifest-path elastos/tools/browser-local-exit/Cargo.toml
CARGO_TARGET_DIR="$cargo_target_dir" cargo build --quiet --manifest-path elastos/tools/browser-native-proxy-engine/Cargo.toml

make_request() {
  local stream_id="$1"
  local url="$2"
  local principal_id="$3"
  node - "$stream_id" "$url" "$principal_id" <<'NODE'
const [streamId, url, principalId] = process.argv.slice(2);
console.log(JSON.stringify({
  schema: "elastos.browser.engine.launch-request/v1",
  adapter: "hosted-product",
  stream_id: streamId,
  principal_id: principalId,
  engine: "selkies_gstreamer",
  display_mode: "webrtc_remote_display",
  guarantee_level: "operator_rbi",
  network_mode: "runtime_net_only",
  direct_network: false,
  wallet_injection: false,
  url
}));
NODE
}

launch_session() {
  local name="$1"
  local stream_id="$2"
  local url="$3"
  local principal_id="$4"
  local stdout_path="$tmp_dir/$name.stdout.json"
  local stderr_path="$tmp_dir/$name.stderr.log"
  (
    HOME="$service_home" \
    XDG_DATA_HOME="$service_xdg_data_home" \
    ELASTOS_BROWSER_ENGINE_REQUEST="$(make_request "$stream_id" "$url" "$principal_id")" \
    ELASTOS_BROWSER_PROFILE_ROOT="$tmp_dir/profiles" \
    ELASTOS_BROWSER_SELKIES_BROWSER_PROGRAM="$browser_program" \
    ELASTOS_BROWSER_SELKIES_CARGO_TARGET_DIR="$cargo_target_dir" \
    ELASTOS_BROWSER_SELKIES_TARGET_IMAGE="${ELASTOS_BROWSER_SELKIES_TARGET_IMAGE:-elastos/browser-selkies-runtime-target:dev}" \
    ELASTOS_BROWSER_SELKIES_ICE_SERVER="${ELASTOS_BROWSER_SELKIES_ICE_SERVER:-stun:stun.l.google.com:19302}" \
    ELASTOS_BROWSER_SELKIES_WIDTH="${ELASTOS_BROWSER_SELKIES_WIDTH:-1920}" \
    ELASTOS_BROWSER_SELKIES_HEIGHT="${ELASTOS_BROWSER_SELKIES_HEIGHT:-1080}" \
    ELASTOS_BROWSER_SELKIES_FRAMERATE="${ELASTOS_BROWSER_SELKIES_FRAMERATE:-30}" \
    ELASTOS_BROWSER_SELKIES_VIDEO_BITRATE="${ELASTOS_BROWSER_SELKIES_VIDEO_BITRATE:-16}" \
    ELASTOS_BROWSER_SELKIES_H264_CRF="${ELASTOS_BROWSER_SELKIES_H264_CRF:-23}" \
    ELASTOS_BROWSER_SELKIES_RESOLUTION_MODE="${ELASTOS_BROWSER_SELKIES_RESOLUTION_MODE:-dynamic}" \
    ELASTOS_BROWSER_PER_LAUNCH_STARTUP_TIMEOUT_MS="${ELASTOS_BROWSER_PER_LAUNCH_STARTUP_TIMEOUT_MS:-90000}" \
    node scripts/browser-per-launch-selkies-supervisor.mjs
  ) >"$stdout_path" 2>"$stderr_path" &
  last_pid="$!"
}

launch_session a "stream:per-launch-smoke:a" "https://example.com/" "person:local:per-launch-smoke-a"
pid_a="$last_pid"
launch_session b "stream:per-launch-smoke:b" "https://example.org/" "person:local:per-launch-smoke-b"
pid_b="$last_pid"

wait "$pid_a" || {
  echo "first per-launch Browser session failed" >&2
  sed -n '1,220p' "$tmp_dir/a.stderr.log" >&2 || true
  exit 1
}
pid_a=""
wait "$pid_b" || {
  echo "second per-launch Browser session failed" >&2
  sed -n '1,220p' "$tmp_dir/b.stderr.log" >&2 || true
  exit 1
}
pid_b=""

node - "$tmp_dir/a.stdout.json" "$tmp_dir/b.stdout.json" <<'NODE'
const fs = require("node:fs");
const http = require("node:http");
const path = require("node:path");
const [aPath, bPath] = process.argv.slice(2);

function readResult(file) {
  const raw = fs.readFileSync(file, "utf8").trim();
  if (!raw) throw new Error(`${file} is empty`);
  const line = raw.split(/\n/).filter(Boolean).at(-1);
  return JSON.parse(line);
}

function readTargetReceipt(sessionDir) {
  const raw = fs.readFileSync(`${sessionDir}/target.stdout.log`, "utf8").trim();
  const line = raw.split(/\n/).filter(Boolean).findLast((entry) => entry.includes("elastos.browser.selkies-runtime-exit-target/v1"));
  if (!line) throw new Error(`${sessionDir}/target.stdout.log did not include target receipt`);
  return JSON.parse(line);
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function unixJson(method, socketPath, requestPath, body) {
  const payload = body ? Buffer.from(JSON.stringify(body)) : undefined;
  return new Promise((resolve, reject) => {
    const req = http.request(
      {
        socketPath,
        path: requestPath,
        method,
        timeout: 15000,
        headers: payload
          ? {
              "content-type": "application/json",
              "content-length": payload.length,
            }
          : undefined,
      },
      (res) => {
        const chunks = [];
        res.on("data", (chunk) => chunks.push(chunk));
        res.on("end", () => {
          const text = Buffer.concat(chunks).toString("utf8");
          let parsed = {};
          try {
            parsed = text ? JSON.parse(text) : {};
          } catch (error) {
            reject(new Error(`${requestPath} returned non-JSON: ${error.message}`));
            return;
          }
          if ((res.statusCode || 500) < 200 || (res.statusCode || 500) >= 300) {
            reject(new Error(`${requestPath} returned ${res.statusCode}: ${parsed.error || text}`));
            return;
          }
          resolve(parsed);
        });
      },
    );
    req.on("timeout", () => req.destroy(new Error(`${requestPath} timed out`)));
    req.on("error", reject);
    if (payload) req.end(payload);
    else req.end();
  });
}

async function validateSession(result, expectedUrl) {
  assert(result.schema === "elastos.browser.engine.supervisor-result/v1", "invalid supervisor-result schema");
  assert(/^page:/.test(result.page_id || ""), "missing page_id");
  assert(result.isolated_session === true, "session is not isolated");
  assert(result.control_socket_path && result.control_socket_path.startsWith("/"), "missing page-scoped control socket");
  assert(result.network_mode === "runtime_net_only", "session did not report runtime_net_only");
  assert(result.direct_network === false, "session did not report direct_network=false");
  assert(result.wallet_injection === false, "session reported wallet injection authority");
  assert(result.display_session?.mode === "webrtc_remote_display", "session did not return WebRTC display");
  assert(result.display_session?.backend_class === "product_compositor", "session is not a product compositor");
  assert(result.display_session?.audio === true && result.display_session?.video === true, "session did not expose audio+video");

  const status = await unixJson("GET", result.control_socket_path, "/status");
  assert(status.schema === "elastos.browser.selkies-control.status/v1", "invalid control status schema");
  assert(status.active_pages === 1, "control session must own exactly one active page");
  assert(status.page_ids?.includes(result.page_id), "control session does not own returned page_id");

  const pageStatus = await unixJson("GET", result.control_socket_path, `/pages/${encodeURIComponent(result.page_id)}/status`);
  assert(pageStatus.schema === "elastos.browser.page-status/v1", "invalid page status schema");
  assert(pageStatus.page_id === result.page_id, "page status returned mismatched page_id");
  assert(pageStatus.direct_network === false, "page status did not report direct_network=false");
  assert(typeof pageStatus.actual_url === "string" && pageStatus.actual_url.startsWith(expectedUrl), "page did not navigate to expected URL");
}

(async () => {
  const a = readResult(aPath);
  const b = readResult(bPath);
  assert(a.page_id !== b.page_id, "per-launch smoke returned duplicate page ids");
  assert(a.control_socket_path !== b.control_socket_path, "per-launch smoke reused a control socket");
  assert(a.isolation?.session_dir !== b.isolation?.session_dir, "per-launch smoke reused an isolation directory");
  const targetA = readTargetReceipt(a.isolation.session_dir);
  const targetB = readTargetReceipt(b.isolation.session_dir);
  assert(targetA.profile_persistent === true, "first target did not report persistent profile");
  assert(targetB.profile_persistent === true, "second target did not report persistent profile");
  assert(targetA.profile_dir.startsWith(`${process.env.TMPDIR || "/tmp"}`) || targetA.profile_dir.includes("/profiles/"), "first target profile dir is not under smoke profile root");
  assert(/^profile-[0-9a-f]{64}$/.test(path.basename(targetA.profile_dir)), "first target profile dir must use a full non-reversible profile key");
  assert(/^profile-[0-9a-f]{64}$/.test(path.basename(targetB.profile_dir)), "second target profile dir must use a full non-reversible profile key");
  assert(targetA.profile_dir !== targetB.profile_dir, "distinct smoke principals reused one Browser profile directory");

  await validateSession(a, "https://example.com/");
  await validateSession(b, "https://example.org/");

  const shutdownA = await unixJson("POST", a.control_socket_path, "/shutdown", {});
  const shutdownB = await unixJson("POST", b.control_socket_path, "/shutdown", {});
  assert(shutdownA.schema === "elastos.browser.selkies-control.shutdown/v1", "first shutdown schema mismatch");
  assert(shutdownB.schema === "elastos.browser.selkies-control.shutdown/v1", "second shutdown schema mismatch");

  console.log(JSON.stringify({
    schema: "elastos.browser.per-launch-selkies-supervisor-smoke/v1",
    ok: true,
    sessions: [
      { page_id: a.page_id, control_socket_path: a.control_socket_path, session_dir: a.isolation.session_dir },
      { page_id: b.page_id, control_socket_path: b.control_socket_path, session_dir: b.isolation.session_dir },
    ],
  }));
})().catch((error) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
});
NODE
