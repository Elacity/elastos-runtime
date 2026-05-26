#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<'USAGE'
Usage:
  scripts/browser-hosted-product-glide-wallet-smoke.sh \
    --adapter-config /path/to/browser-engine-adapter.json \
    --cdp-endpoint http://127.0.0.1:PORT

Launches Glide through the hosted product Browser adapter, connects the
Runtime-mediated EIP-1193 provider from inside the real Glide UI, and verifies
that Glide renders the connected ESC account without direct network authority.
USAGE
}

adapter_config=""
cdp_endpoint=""
url="https://glidefinance.io/"

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
    stream_id: "stream:hosted-product-glide-wallet-smoke",
    target: `${scheme}://${target.hostname}:${port}`,
    byte_transport: "adapter_ipc",
    adapter_ipc: {
      schema: "elastos.adapter-ipc/v1",
      kind: "unix_socket",
      path: "/tmp/elastos-browser-product-glide-wallet-smoke-adapter.sock",
      stream_id: "stream:hosted-product-glide-wallet-smoke",
      runtime_stream_path: "/tmp/elastos-browser-product-glide-wallet-smoke-runtime.sock",
    },
  };
}

async function clickFirst(page, locators, label) {
  let lastError = null;
  for (const locator of locators) {
    const count = await locator.count().catch(() => 0);
    for (let index = 0; index < Math.min(count, 12); index += 1) {
      const item = locator.nth(index);
      const visible = await item.isVisible({ timeout: 1000 }).catch(() => false);
      if (!visible) {
        continue;
      }
      try {
        await item.click({ timeout: 5000 });
        return;
      } catch (error) {
        lastError = error;
      }
    }
    try {
      await locator.click({ timeout: 5000 });
      return;
    } catch (error) {
      lastError = error;
    }
  }
  throw new Error(`could not click ${label}: ${lastError?.message || "not found"}`);
}

async function connectedAccountText(page, attempts = 1) {
  let text = "";
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    if (attempt > 0) {
      await page.waitForTimeout(1000);
    }
    text = await page.locator("body").innerText({ timeout: 5000 }).catch(() => "");
    if (/0x11\.\.\.1111/i.test(text) || /0x1111/i.test(text) || /1111/i.test(text)) {
      return text;
    }
  }
  return text;
}

const adapter = new AdapterClient();
let pageId = "";
let browser = null;
try {
  const config = JSON.parse(fs.readFileSync(process.env.ADAPTER_CONFIG, "utf8"));
  expectOk(await adapter.request({ op: "init", config }), "init");
  const launched = expectOk(
    await adapter.request({
      op: "launch",
      url: process.env.TARGET_URL,
      stream_session: streamSessionFor(process.env.TARGET_URL),
      principal_id: "person:local:hosted-product-glide-wallet-smoke",
      reason: "verify hosted product Glide wallet flow",
      display_mode: "webrtc_remote_display",
      viewport: { width: 1365, height: 900 },
      wallet: {
        accounts: [
          {
            account_id: "wallet:eip155:20:0x1111111111111111111111111111111111111111",
            chain_namespace: "eip155:20",
            address: "0x1111111111111111111111111111111111111111",
            label: "ESC Smoke",
          },
        ],
        default_chain_namespace: "eip155:20",
      },
    }),
    "launch",
  );
  pageId = launched.page_id;
  if (launched.direct_network !== false) {
    throw new Error("hosted Glide launch reported direct network authority");
  }
  if (launched.wallet_bridge?.mode !== "runtime_mediated_eip1193") {
    throw new Error(`missing wallet bridge receipt: ${JSON.stringify(launched.wallet_bridge)}`);
  }
  if (launched.wallet_bridge?.signing !== "approval_required") {
    throw new Error(`hosted wallet bridge must route signing through Runtime approval: ${JSON.stringify(launched.wallet_bridge)}`);
  }

  const { chromium } = playwright;
  browser = await chromium.connectOverCDP(process.env.CDP_ENDPOINT);
  const deadline = Date.now() + 45_000;
  let page = null;
  while (Date.now() < deadline && !page) {
    for (const candidate of browser.contexts().flatMap((context) => context.pages())) {
      const hasEthereum = await candidate.evaluate(() => Boolean(globalThis.ethereum)).catch(() => false);
      if (candidate.url().startsWith(process.env.TARGET_URL) && hasEthereum) {
        page = candidate;
        break;
      }
    }
    if (!page) {
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
  }
  if (!page) {
    throw new Error("no hosted Glide page had the Runtime-mediated wallet bridge");
  }

  await page.bringToFront().catch(() => {});
  await page.waitForLoadState("domcontentloaded", { timeout: 20_000 }).catch(() => {});
  await page.waitForTimeout(5000);
  let text = await connectedAccountText(page, 5);
  if (!(/0x11\.\.\.1111/i.test(text) || /0x1111/i.test(text) || /1111/i.test(text))) {
    await clickFirst(
      page,
      [
        page.getByRole("button", { name: /^connect wallet$/i }),
        page.locator("button").filter({ hasText: /^connect wallet$/i }),
        page.getByText(/^connect wallet$/i),
        page.locator("button").filter({ hasText: /^connect$/i }),
      ],
      "Connect Wallet",
    );
    await page.waitForTimeout(1000);
    await clickFirst(
      page,
      [
        page.getByRole("button", { name: /metamask|browser wallet|injected/i }),
        page.getByText(/metamask/i),
        page.getByText(/browser wallet/i),
        page.locator("button").filter({ hasText: /metamask|browser wallet|injected/i }),
      ],
      "wallet connector",
    );
    text = await connectedAccountText(page, 30);
  }
  if (!(/0x11\.\.\.1111/i.test(text) || /0x1111/i.test(text) || /1111/i.test(text))) {
    throw new Error(`Glide did not render the connected ESC account: ${text.slice(0, 800)}`);
  }

  console.log(JSON.stringify({
    ok: true,
    schema: "elastos.browser.hosted-product-glide-wallet-smoke/v1",
    page_id: pageId,
    actual_url: page.url(),
    connected_account: "0x1111111111111111111111111111111111111111",
    wallet_bridge: launched.wallet_bridge,
    direct_network: false,
  }));
} finally {
  await browser?.close().catch(() => {});
  await adapter.close(pageId);
}
NODE
