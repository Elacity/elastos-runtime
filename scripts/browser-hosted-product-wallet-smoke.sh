#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<'USAGE'
Usage:
  scripts/browser-hosted-product-wallet-smoke.sh \
    --adapter-config /path/to/browser-engine-adapter.json \
    --cdp-endpoint http://127.0.0.1:PORT \
    [--url https://example.com/]

Launches the hosted product Browser adapter with fixture EVM accounts, then
uses the operator-private CDP endpoint only to verify the remote page received
the constrained Runtime-mediated EIP-1193 bridge.
USAGE
}

adapter_config=""
cdp_endpoint=""
url="https://example.com/"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --adapter-config)
      adapter_config="${2:-}"
      shift 2
      ;;
    --cdp-endpoint)
      cdp_endpoint="${2:-}"
      shift 2
      ;;
    --url)
      url="${2:-}"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [[ -z "$adapter_config" || -z "$cdp_endpoint" ]]; then
  usage >&2
  exit 1
fi
if [[ ! -f "$adapter_config" ]]; then
  echo "--adapter-config does not exist: $adapter_config" >&2
  exit 1
fi
if [[ ! "$cdp_endpoint" =~ ^https?://127\.0\.0\.1:[0-9]+/?$ && ! "$cdp_endpoint" =~ ^https?://localhost:[0-9]+/?$ ]]; then
  echo "--cdp-endpoint must be an operator-private loopback HTTP endpoint" >&2
  exit 1
fi
if [[ ! "$url" =~ ^https?:// ]]; then
  echo "--url must use http or https" >&2
  exit 1
fi

cd "$repo_root"
cargo build --quiet --manifest-path capsules/browser-engine-adapter/Cargo.toml
adapter_bin="${CARGO_TARGET_DIR:-capsules/browser-engine-adapter/target}/debug/browser-engine-adapter"

ADAPTER_BIN="$adapter_bin" ADAPTER_CONFIG="$adapter_config" CDP_ENDPOINT="$cdp_endpoint" TARGET_URL="$url" \
  node --input-type=module - <<'NODE'
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import fs from "node:fs";
import playwright from "./elastos/tools/browser-playwright-engine/node_modules/playwright/index.js";

class AdapterClient {
  constructor() {
    this.child = spawn(process.env.ADAPTER_BIN, [], {
      stdio: ["pipe", "pipe", "pipe"],
    });
    this.pending = [];
    createInterface({ input: this.child.stdout }).on("line", (line) => {
      const pending = this.pending.shift();
      if (!pending) return;
      try {
        pending.resolve(JSON.parse(line));
      } catch (error) {
        pending.reject(error);
      }
    });
    this.child.stderr.on("data", (chunk) => process.stderr.write(chunk));
    this.child.on("exit", (code, signal) => {
      for (const pending of this.pending.splice(0)) {
        pending.reject(new Error(`browser-engine-adapter exited with ${signal || code}`));
      }
    });
  }

  request(payload) {
    return new Promise((resolve, reject) => {
      this.pending.push({ resolve, reject });
      this.child.stdin.write(`${JSON.stringify(payload)}\n`);
    });
  }

  async close(pageId) {
    if (pageId) {
      await this.request({ op: "close_page", page_id: pageId }).catch(() => {});
    }
    await this.request({ op: "shutdown" }).catch(() => {});
    this.child.stdin.end();
    this.child.kill();
  }
}

function expectOk(response, label) {
  if (!response || response.status !== "ok") {
    throw new Error(`${label} failed: ${response?.code || "unknown"} ${response?.message || ""}`);
  }
  return response.data || {};
}

function streamSessionFor(targetUrl) {
  const target = new URL(targetUrl);
  const port = target.port || (target.protocol === "https:" ? "443" : "80");
  const scheme = target.protocol === "https:" ? "tls" : "tcp";
  return {
    schema: "elastos.exit.stream-session/v1",
    stream_id: "stream:hosted-product-wallet-smoke",
    target: `${scheme}://${target.hostname}:${port}`,
    byte_transport: "adapter_ipc",
    adapter_ipc: {
      schema: "elastos.adapter-ipc/v1",
      kind: "unix_socket",
      path: "/tmp/elastos-browser-product-wallet-smoke-adapter.sock",
      stream_id: "stream:hosted-product-wallet-smoke",
      runtime_stream_path: "/tmp/elastos-browser-product-wallet-smoke-runtime.sock",
    },
  };
}

const adapter = new AdapterClient();
let pageId = "";
try {
  const config = JSON.parse(fs.readFileSync(process.env.ADAPTER_CONFIG, "utf8"));
  expectOk(await adapter.request({ op: "init", config }), "init");
  const launched = expectOk(
    await adapter.request({
      op: "launch",
      url: process.env.TARGET_URL,
      stream_session: streamSessionFor(process.env.TARGET_URL),
      principal_id: "person:local:hosted-product-wallet-smoke",
      reason: "verify hosted product wallet bridge",
      display_mode: "webrtc_remote_display",
      viewport: { width: 1280, height: 720 },
      wallet: {
        accounts: [
          {
            account_id: "wallet:eip155:20:0x1111111111111111111111111111111111111111",
            chain_namespace: "eip155:20",
            address: "0x1111111111111111111111111111111111111111",
            label: "ESC Smoke",
          },
          {
            account_id: "wallet:eip155:8453:0x2222222222222222222222222222222222222222",
            chain_namespace: "eip155:8453",
            address: "0x2222222222222222222222222222222222222222",
            label: "Base Smoke",
          },
        ],
        default_chain_namespace: "eip155:20",
      },
    }),
    "launch",
  );
  pageId = launched.page_id;
  if (launched.wallet_bridge?.mode !== "runtime_mediated_eip1193") {
    throw new Error(`missing wallet bridge receipt: ${JSON.stringify(launched.wallet_bridge)}`);
  }
  if (launched.wallet_bridge?.signing !== "approval_required") {
    throw new Error(`hosted wallet bridge must route signing through Runtime approval: ${JSON.stringify(launched.wallet_bridge)}`);
  }
  const { chromium } = playwright;
  const browser = await chromium.connectOverCDP(process.env.CDP_ENDPOINT);
  const pages = browser.contexts().flatMap((context) => context.pages());
  let page = null;
  for (const candidate of pages) {
    const hasEthereum = await candidate.evaluate(() => Boolean(globalThis.ethereum)).catch(() => false);
    if (candidate.url() === process.env.TARGET_URL && hasEthereum) {
      page = candidate;
      break;
    }
  }
  if (!page) {
    throw new Error(`no launched page had the hosted wallet bridge: ${JSON.stringify(pages.map((item) => item.url()))}`);
  }
	  const payload = await page.evaluate(async () => {
    const out = {
      hasEthereum: Boolean(globalThis.ethereum),
      isMetaMask: Boolean(globalThis.ethereum?.isMetaMask),
      providers: Array.isArray(globalThis.ethereum?.providers) ? globalThis.ethereum.providers.length : 0,
      eip6963: false,
    };
    globalThis.addEventListener("eip6963:announceProvider", (event) => {
      out.eip6963 = Boolean(event.detail?.provider?.isElastOS && event.detail?.info?.name === "ElastOS Wallet");
    }, { once: true });
    globalThis.dispatchEvent(new Event("eip6963:requestProvider"));
    out.chain = await globalThis.ethereum.request({ method: "eth_chainId" });
    out.accounts = await globalThis.ethereum.request({ method: "eth_requestAccounts" });
    out.coinbase = await globalThis.ethereum.request({ method: "eth_coinbase" });
    out.permissions = await globalThis.ethereum.request({ method: "wallet_getPermissions" });
    await globalThis.ethereum.request({
      method: "wallet_switchEthereumChain",
      params: [{ chainId: "0x2105" }],
    });
    out.switchedChain = await globalThis.ethereum.request({ method: "eth_chainId" });
    out.switchedAccounts = await globalThis.ethereum.request({ method: "eth_accounts" });
    await globalThis.ethereum.request({
      method: "wallet_addEthereumChain",
      params: [{ chainId: "0x14", chainName: "Elastos Smart Chain" }],
    });
    out.addedEscChain = await globalThis.ethereum.request({ method: "eth_chainId" });
    out.addedEscAccounts = await globalThis.ethereum.request({ method: "eth_accounts" });
	    return out;
	  });
	  const navigateResult = expectOk(
	    await adapter.request({
	      op: "input",
	      page_id: pageId,
	      event: { type: "browser_command", command: "navigate", url: "https://ela.city/home" },
	      principal_id: "person:local:hosted-product-wallet-smoke",
	    }),
	    "command navigate",
	  );
	  await page.waitForLoadState("domcontentloaded", { timeout: 20_000 }).catch(() => {});
	  const afterNavigate = await page.evaluate(async () => ({
	    url: location.href,
	    hasEthereum: Boolean(globalThis.ethereum),
	    isMetaMask: Boolean(globalThis.ethereum?.isMetaMask),
	    isElastOS: Boolean(globalThis.ethereum?.isElastOS),
	    providers: Array.isArray(globalThis.ethereum?.providers) ? globalThis.ethereum.providers.length : 0,
	    chain: globalThis.ethereum ? await globalThis.ethereum.request({ method: "eth_chainId" }) : null,
	    accounts: globalThis.ethereum ? await globalThis.ethereum.request({ method: "eth_accounts" }) : [],
	  }));
	  const pageReloaded = page.waitForNavigation({ waitUntil: "domcontentloaded", timeout: 20_000 }).catch(() => null);
	  await page.evaluate(() => {
	    location.reload();
	    return true;
	  }).catch(() => {});
	  await pageReloaded;
	  await page.waitForTimeout(1000);
	  const afterPageReload = await page.evaluate(async () => ({
	    url: location.href,
	    hasEthereum: Boolean(globalThis.ethereum),
	    isMetaMask: Boolean(globalThis.ethereum?.isMetaMask),
	    isElastOS: Boolean(globalThis.ethereum?.isElastOS),
	    providers: Array.isArray(globalThis.ethereum?.providers) ? globalThis.ethereum.providers.length : 0,
	    chain: globalThis.ethereum ? await globalThis.ethereum.request({ method: "eth_chainId" }) : null,
	    accounts: globalThis.ethereum ? await globalThis.ethereum.request({ method: "eth_accounts" }) : [],
	  }));
	  await browser.close().catch(() => {});
  if (!payload.hasEthereum || !payload.isMetaMask || payload.providers < 1) {
    throw new Error(`wallet provider missing: ${JSON.stringify(payload)}`);
  }
  if (!payload.eip6963 || payload.coinbase !== "0x1111111111111111111111111111111111111111" || payload.permissions?.[0]?.parentCapability !== "eth_accounts") {
    throw new Error(`wallet provider compatibility surface missing: ${JSON.stringify(payload)}`);
  }
  if (payload.chain !== "0x14" || payload.accounts?.[0] !== "0x1111111111111111111111111111111111111111") {
    throw new Error(`ESC account mismatch: ${JSON.stringify(payload)}`);
  }
  if (payload.switchedChain !== "0x2105" || payload.switchedAccounts?.[0] !== "0x2222222222222222222222222222222222222222") {
    throw new Error(`Base switch mismatch: ${JSON.stringify(payload)}`);
  }
	  if (payload.addedEscChain !== "0x14" || payload.addedEscAccounts?.[0] !== "0x1111111111111111111111111111111111111111") {
	    throw new Error(`ESC add-chain switch mismatch: ${JSON.stringify(payload)}`);
	  }
	  if (
	    navigateResult?.actual_url !== "https://ela.city/home" ||
	    !afterNavigate.hasEthereum ||
	    afterNavigate.chain !== "0x14" ||
	    afterNavigate.accounts?.[0] !== "0x1111111111111111111111111111111111111111"
	  ) {
	    throw new Error(`wallet bridge did not survive Runtime command navigation: ${JSON.stringify({ navigateResult, afterNavigate })}`);
	  }
	  if (
	    !afterPageReload.hasEthereum ||
	    afterPageReload.chain !== "0x14" ||
	    afterPageReload.accounts?.[0] !== "0x1111111111111111111111111111111111111111"
	  ) {
	    throw new Error(`wallet bridge did not survive page-initiated reload: ${JSON.stringify(afterPageReload)}`);
	  }
	  console.log(JSON.stringify({
    ok: true,
    schema: "elastos.browser.hosted-product-wallet-smoke/v1",
    page_id: pageId,
	    wallet_bridge: launched.wallet_bridge,
	    payload,
	    after_navigate: afterNavigate,
	    after_page_reload: afterPageReload,
	    direct_network: false,
	  }));
} finally {
  await adapter.close(pageId);
}
NODE
