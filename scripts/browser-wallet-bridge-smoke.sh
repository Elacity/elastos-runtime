#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"
local_exit_pid=""
site_pid=""

cleanup() {
  if [[ -n "${daemon_pid:-}" ]]; then
    kill "$daemon_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "$site_pid" ]]; then
    kill "$site_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "$local_exit_pid" ]]; then
    kill "$local_exit_pid" >/dev/null 2>&1 || true
  fi
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

cd "$repo_root"

cargo build --quiet --manifest-path elastos/tools/browser-local-exit/Cargo.toml

site_port="$((48000 + RANDOM % 10000))"
local_exit_socket="$tmp_dir/local-exit.sock"
control_socket="$tmp_dir/playwright-control.sock"
profile_root="$tmp_dir/profile"

SITE_PORT="$site_port" node --input-type=module - <<'NODE' &
import http from 'node:http';

const html = `<!doctype html>
<meta charset="utf-8">
<title>wallet-pending</title>
<script>
(async () => {
  const out = {};
  try {
    out.initialChain = await window.ethereum.request({ method: "eth_chainId" });
    out.initialAccounts = await window.ethereum.request({ method: "eth_requestAccounts" });
    out.initialNet = await window.ethereum.request({ method: "net_version" });
    await window.ethereum.request({ method: "wallet_switchEthereumChain", params: [{ chainId: "0x2105" }] });
    out.switchedChain = await window.ethereum.request({ method: "eth_chainId" });
    out.switchedAccounts = await window.ethereum.request({ method: "eth_accounts" });
    out.selectedAddress = window.ethereum.selectedAddress;
    out.networkVersion = window.ethereum.networkVersion;
    document.title = "wallet-ok:" + btoa(JSON.stringify(out));
  } catch (error) {
    document.title = "wallet-error:" + btoa(JSON.stringify({ message: String(error && error.message || error) }));
  }
})();
</script>
<body>Browser wallet bridge smoke</body>`;

const server = http.createServer((req, res) => {
  res.writeHead(200, {
    'content-type': 'text/html; charset=utf-8',
    'cache-control': 'no-store',
  });
  res.end(html);
});
server.listen(Number(process.env.SITE_PORT), '127.0.0.1');
NODE
site_pid="$!"

for _ in {1..100}; do
  if curl -fsS "http://127.0.0.1:$site_port/" >/dev/null 2>&1; then
    break
  fi
  sleep 0.05
done

ELASTOS_BROWSER_LOCAL_EXIT_CONFIG="$(node - <<NODE
console.log(JSON.stringify({
  schema: "elastos.browser.local-exit.config/v1",
  relay_ipc_path: "$local_exit_socket",
  allowed_hosts: ["127.0.0.1"],
  allowed_schemes: ["tcp"],
  allowed_ports: [$site_port],
  address_family: "ipv4_only",
  allow_private_targets: true,
  replace_existing_socket: true
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
  allowed_hosts: ["127.0.0.1"],
  allowed_protocols: ["http"],
  allowed_ports: [$site_port],
  viewport: { width: 800, height: 600 },
  headless: true,
  launch_timeout_ms: 30000,
  ice_servers: []
}));
NODE

launch_request="$(node - <<NODE
console.log(JSON.stringify({
  schema: "elastos.browser.engine.launch-request/v1",
  adapter: "playwright-wallet-smoke",
  engine: "chromium",
  stream_id: "stream:browser-wallet-bridge-smoke",
  url: "http://127.0.0.1:$site_port/",
  display_mode: "webrtc_remote_display",
  guarantee_level: "operator_rbi",
  network_mode: "runtime_net_only",
  direct_network: false,
  wallet_injection: false,
  viewport: { width: 800, height: 600 },
  wallet: {
    accounts: [
      {
        account_id: "wallet:eip155:20:0x1111111111111111111111111111111111111111",
        chain_namespace: "eip155:20",
        address: "0x1111111111111111111111111111111111111111",
        label: "ESC Smoke"
      },
      {
        account_id: "wallet:eip155:8453:0x2222222222222222222222222222222222222222",
        chain_namespace: "eip155:8453",
        address: "0x2222222222222222222222222222222222222222",
        label: "Base Smoke"
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

function control(path) {
  return new Promise((resolve, reject) => {
    const req = http.request({ socketPath: controlSocket, path, method: "GET" }, (res) => {
      const chunks = [];
      res.on("data", (chunk) => chunks.push(chunk));
      res.on("end", () => {
        const body = Buffer.concat(chunks).toString("utf8");
        if (res.statusCode < 200 || res.statusCode >= 300) {
          reject(new Error(`GET ${path} returned ${res.statusCode}: ${body}`));
          return;
        }
        try {
          resolve(JSON.parse(body));
        } catch (error) {
          reject(error);
        }
      });
    });
    req.on("error", reject);
    req.end();
  });
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function pageTitle() {
  const media = await control(`/pages/${encodeURIComponent(result.page_id)}/media`);
  const summaries = Array.isArray(media.frame_summaries) ? media.frame_summaries : [];
  const titled = summaries.find((summary) => typeof summary.title === "string" && summary.title.startsWith("wallet-"));
  return titled?.title || summaries[0]?.title || "";
}

(async () => {
  assert(result.schema === "elastos.browser.engine.supervisor-result/v1", "unexpected launch result schema");
  assert(result.network_mode === "runtime_net_only", "launch did not stay runtime_net_only");
  assert(result.direct_network === false, "launch reported direct network access");
  assert(result.wallet_bridge?.mode === "runtime_mediated_eip1193", "wallet bridge was not runtime mediated");
  assert(result.wallet_bridge?.accounts === 2, "wallet bridge did not expose the fixture accounts");
  let title = "";
  for (let attempt = 0; attempt < 30; attempt += 1) {
    title = await pageTitle();
    if (title.startsWith("wallet-")) {
      break;
    }
    await sleep(250);
  }
  assert(typeof title === "string" && title.length > 0, "wallet smoke did not produce a page title");
  if (title.startsWith("wallet-error:")) {
    const error = JSON.parse(Buffer.from(title.slice("wallet-error:".length), "base64").toString("utf8"));
    throw new Error(`wallet bridge page failed: ${JSON.stringify(error)}`);
  }
  assert(title.startsWith("wallet-ok:"), `wallet smoke did not complete: ${title}`);
  const payload = JSON.parse(Buffer.from(title.slice("wallet-ok:".length), "base64").toString("utf8"));
  assert(payload.initialChain === "0x14", `expected ESC initial chain 0x14, got ${payload.initialChain}`);
  assert(payload.initialNet === "20", `expected ESC net_version 20, got ${payload.initialNet}`);
  assert(payload.initialAccounts?.[0] === "0x1111111111111111111111111111111111111111", "initial account mismatch");
  assert(payload.switchedChain === "0x2105", `expected Base chain 0x2105, got ${payload.switchedChain}`);
  assert(payload.networkVersion === "8453", `expected Base networkVersion 8453, got ${payload.networkVersion}`);
  assert(payload.switchedAccounts?.[0] === "0x2222222222222222222222222222222222222222", "switched account mismatch");
  assert(payload.selectedAddress === "0x2222222222222222222222222222222222222222", "selectedAddress did not track switched account");
  const status = await control("/status");
  if (Number.isInteger(status.pid) && status.pid > 1) {
    process.kill(status.pid, "SIGTERM");
  }
  console.log(JSON.stringify({
    ok: true,
    initial_chain: payload.initialChain,
    switched_chain: payload.switchedChain,
    initial_account: payload.initialAccounts[0],
    switched_account: payload.switchedAccounts[0],
    direct_network: false,
  }));
})().catch((error) => {
  console.error(error.stack || error.message || String(error));
  process.exit(1);
});
NODE
