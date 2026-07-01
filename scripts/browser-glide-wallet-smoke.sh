#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"
local_exit_pid=""

cleanup() {
  if [[ -n "${daemon_pid:-}" ]]; then
    kill "$daemon_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "$local_exit_pid" ]]; then
    kill "$local_exit_pid" >/dev/null 2>&1 || true
  fi
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

cd "$repo_root"

cargo build --quiet --manifest-path elastos/tools/browser-local-exit/Cargo.toml

local_exit_socket="$tmp_dir/local-exit.sock"
control_socket="$tmp_dir/playwright-control.sock"
profile_root="$tmp_dir/profile"

ELASTOS_BROWSER_LOCAL_EXIT_CONFIG="$(node - <<NODE
console.log(JSON.stringify({
  schema: "elastos.browser.local-exit.config/v1",
  relay_ipc_path: "$local_exit_socket",
  allowed_hosts: ["*"],
  allowed_schemes: ["tcp"],
  allowed_ports: [80, 443],
  address_family: process.env.BROWSER_SMOKE_ADDRESS_FAMILY || "prefer_ipv4",
  replace_existing_socket: true,
  allow_private_targets: false
}));
NODE
)" \
  elastos/tools/browser-local-exit/target/debug/browser-local-exit \
  >"$tmp_dir/local-exit.out" 2>"$tmp_dir/local-exit.err" &
local_exit_pid="$!"

for _ in {1..100}; do
  [[ -S "$local_exit_socket" ]] && break
  sleep 0.05
done
if [[ ! -S "$local_exit_socket" ]]; then
  cat "$tmp_dir/local-exit.err" >&2 || true
  exit 1
fi

engine_config="$tmp_dir/playwright-engine.json"
node - <<NODE >"$engine_config"
console.log(JSON.stringify({
  schema: "elastos.browser.playwright-engine.config/v1",
  control_socket_path: "$control_socket",
  local_exit_socket_path: "$local_exit_socket",
  profile_root: "$profile_root",
  allowed_hosts: ["*"],
  allowed_protocols: ["http", "https"],
  allowed_ports: [80, 443],
  viewport: { width: 1280, height: 900 },
  headless: true,
  launch_timeout_ms: 30000,
  ice_servers: []
}));
NODE

launch_request="$(node - <<'NODE'
console.log(JSON.stringify({
  schema: "elastos.browser.engine.launch-request/v1",
  adapter: "playwright-glide-smoke",
  engine: "chromium",
  stream_id: "stream:browser-glide-wallet-smoke",
  url: "https://glidefinance.io/",
  display_mode: "webrtc_remote_display",
  guarantee_level: "operator_rbi",
  network_mode: "runtime_net_only",
  direct_network: false,
  wallet_injection: false,
  viewport: { width: 1280, height: 900 },
  wallet: {
    accounts: [
      {
        account_id: "wallet:eip155:20:0x1111111111111111111111111111111111111111",
        chain_namespace: "eip155:20",
        address: "0x1111111111111111111111111111111111111111",
        label: "ESC Smoke"
      }
    ],
    default_chain_namespace: "eip155:20"
  }
}));
NODE
)"

result_json="$(ELASTOS_BROWSER_PLAYWRIGHT_ENGINE_CONFIG="$(cat "$engine_config")" \
  ELASTOS_BROWSER_ENGINE_REQUEST="$launch_request" \
  node elastos/tools/browser-playwright-engine/src/supervisor.mjs)"

RESULT_JSON="$result_json" CONTROL_SOCKET="$control_socket" node - <<'NODE'
const http = require("node:http");

const result = JSON.parse(process.env.RESULT_JSON);
const controlSocket = process.env.CONTROL_SOCKET;

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function request(method, path, body = null) {
  return new Promise((resolve, reject) => {
    const payload = body ? JSON.stringify(body) : "";
    const req = http.request({
      socketPath: controlSocket,
      path,
      method,
      headers: payload
        ? {
            "content-type": "application/json",
            "content-length": Buffer.byteLength(payload),
          }
        : undefined,
    }, (res) => {
      const chunks = [];
      res.on("data", (chunk) => chunks.push(chunk));
      res.on("end", () => {
        const text = Buffer.concat(chunks).toString("utf8");
        if (res.statusCode < 200 || res.statusCode >= 300) {
          reject(new Error(`${method} ${path} returned ${res.statusCode}: ${text}`));
          return;
        }
        try {
          resolve(JSON.parse(text));
        } catch (error) {
          reject(error);
        }
      });
    });
    req.on("error", reject);
    if (payload) {
      req.write(payload);
    }
    req.end();
  });
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function input(event) {
  await request("POST", `/pages/${encodeURIComponent(result.page_id)}/input`, {
    schema: "elastos.browser.input-event/v1",
    event,
  });
}

async function pageText() {
  const media = await request("GET", `/pages/${encodeURIComponent(result.page_id)}/media`);
  assert(media.direct_network === false, "Glide media/text probe reported direct network");
  return Array.isArray(media.frame_summaries)
    ? media.frame_summaries.map((frame) => `${frame.title || ""} ${frame.text_sample || ""}`).join(" ")
    : "";
}

(async () => {
  assert(result.schema === "elastos.browser.engine.supervisor-result/v1", "unexpected launch result schema");
  assert(result.actual_url === "https://glidefinance.io/", `unexpected Glide URL: ${result.actual_url}`);
  assert(result.network_mode === "runtime_net_only", "launch did not stay runtime_net_only");
  assert(result.direct_network === false, "launch reported direct network access");
  assert(result.wallet_bridge?.mode === "runtime_mediated_eip1193", "wallet bridge was not Runtime-mediated");
  assert(result.wallet_bridge?.default_chain_namespace === "eip155:20", "Glide smoke did not start on ESC");

  await sleep(5000);
  await input({ type: "click", x: 140, y: 190 });
  await sleep(1000);
  await input({ type: "click", x: 555, y: 449 });

  let text = "";
  for (let attempt = 0; attempt < 30; attempt += 1) {
    await sleep(1000);
    text = await pageText();
    if (/Connected with 0x11\.\.\.1111/i.test(text) || /0x\.\.\.1111/i.test(text)) {
      break;
    }
  }
  assert(/Connected with 0x11\.\.\.1111/i.test(text) || /0x\.\.\.1111/i.test(text), `Glide did not show the connected ESC account: ${text.slice(0, 500)}`);

  const status = await request("GET", "/status");
  if (Number.isInteger(status.pid) && status.pid > 1) {
    process.kill(status.pid, "SIGTERM");
  }
  console.log(JSON.stringify({
    ok: true,
    actual_url: result.actual_url,
    connected_account: "0x1111111111111111111111111111111111111111",
    chain_namespace: result.wallet_bridge.default_chain_namespace,
    direct_network: false,
  }));
})().catch((error) => {
  console.error(error.stack || error.message || String(error));
  process.exit(1);
});
NODE
