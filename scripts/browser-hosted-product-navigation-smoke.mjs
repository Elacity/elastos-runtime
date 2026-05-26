#!/usr/bin/env node
import { spawn } from "node:child_process";
import fs from "node:fs";
import { createInterface } from "node:readline";
import process from "node:process";

function usage() {
  console.error(`Usage:
  scripts/browser-hosted-product-navigation-smoke.mjs \\
    --adapter-config /path/to/browser-engine-adapter.json \\
    [--adapter-bin capsules/browser-engine-adapter/target/debug/browser-engine-adapter] \\
    [--first-url https://example.com/] \\
    [--second-url https://example.com/?elastos-browser-nav-smoke=1] \\
    [--cdp-endpoint http://127.0.0.1:PORT] \\
    [--timeout-ms 30000]
`);
}

function parseArgs(argv) {
  const args = {
    adapterBin: "capsules/browser-engine-adapter/target/debug/browser-engine-adapter",
    adapterConfig: "",
    firstUrl: "https://example.com/",
    secondUrl: "https://example.com/?elastos-browser-nav-smoke=1",
    cdpEndpoint: "",
    timeoutMs: 30_000,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--adapter-bin") {
      args.adapterBin = argv[++index] || "";
    } else if (arg === "--adapter-config") {
      args.adapterConfig = argv[++index] || "";
    } else if (arg === "--first-url") {
      args.firstUrl = argv[++index] || "";
    } else if (arg === "--second-url") {
      args.secondUrl = argv[++index] || "";
    } else if (arg === "--cdp-endpoint") {
      args.cdpEndpoint = argv[++index] || "";
    } else if (arg === "--timeout-ms") {
      args.timeoutMs = Number(argv[++index] || "0");
    } else if (arg === "--help" || arg === "-h") {
      usage();
      process.exit(0);
    } else {
      throw new Error(`unknown argument: ${arg}`);
    }
  }
  if (!args.adapterConfig) {
    throw new Error("--adapter-config is required");
  }
  if (!/^https?:\/\//.test(args.firstUrl) || !/^https?:\/\//.test(args.secondUrl)) {
    throw new Error("--first-url and --second-url must use http or https");
  }
  if (args.cdpEndpoint && !/^https?:\/\/(127\.0\.0\.1|localhost):[0-9]+\/?$/.test(args.cdpEndpoint)) {
    throw new Error("--cdp-endpoint must be a loopback HTTP endpoint");
  }
  if (!Number.isInteger(args.timeoutMs) || args.timeoutMs < 5_000 || args.timeoutMs > 120_000) {
    throw new Error("--timeout-ms must be 5000..120000");
  }
  return args;
}

class AdapterClient {
  constructor(adapterBin) {
    this.child = spawn(adapterBin, [], { stdio: ["pipe", "pipe", "pipe"] });
    this.pending = [];
    this.stderr = "";
    this.closed = false;
    createInterface({ input: this.child.stdout }).on("line", (line) => this.handleLine(line));
    this.child.stderr.on("data", (chunk) => {
      this.stderr += chunk.toString("utf8");
      process.stderr.write(chunk);
    });
    this.child.on("exit", (code, signal) => {
      this.closed = true;
      for (const pending of this.pending.splice(0)) {
        pending.reject(new Error(`browser-engine-adapter exited with ${signal || code}`));
      }
    });
  }

  handleLine(line) {
    const pending = this.pending.shift();
    if (!pending) {
      return;
    }
    try {
      pending.resolve(JSON.parse(line));
    } catch {
      pending.reject(new Error(`adapter returned non-JSON line: ${line}`));
    }
  }

  request(payload) {
    if (this.closed) {
      return Promise.reject(new Error("browser-engine-adapter is closed"));
    }
    return new Promise((resolve, reject) => {
      this.pending.push({ resolve, reject });
      this.child.stdin.write(`${JSON.stringify(payload)}\n`);
    });
  }

  async close(pageId) {
    if (!this.closed && pageId) {
      await this.request({ op: "close_page", page_id: pageId }).catch(() => {});
    }
    if (!this.closed) {
      await this.request({ op: "shutdown" }).catch(() => {});
      this.child.stdin.end();
      this.child.kill();
    }
  }
}

function expectOk(response, label) {
  if (!response || response.status !== "ok") {
    throw new Error(`${label} failed: ${response?.code || "unknown"} ${response?.message || ""}`);
  }
  return response.data || {};
}

function streamSessionFor(url) {
  const target = new URL(url);
  const port = target.port || (target.protocol === "https:" ? "443" : "80");
  const scheme = target.protocol === "https:" ? "tls" : "tcp";
  return {
    schema: "elastos.exit.stream-session/v1",
    stream_id: "stream:hosted-product-navigation-smoke",
    target: `${scheme}://${target.hostname}:${port}`,
    byte_transport: "adapter_ipc",
    adapter_ipc: {
      schema: "elastos.adapter-ipc/v1",
      kind: "unix_socket",
      path: "/tmp/elastos-browser-product-navigation-smoke-adapter.sock",
      stream_id: "stream:hosted-product-navigation-smoke",
      runtime_stream_path: "/tmp/elastos-browser-product-navigation-smoke-runtime.sock",
    },
  };
}

async function waitForStatus(adapter, pageId, predicate, timeoutMs, label) {
  const startedAt = Date.now();
  let lastStatus = null;
  while (Date.now() - startedAt <= timeoutMs) {
    lastStatus = expectOk(
      await adapter.request({
        op: "page_status",
        page_id: pageId,
        principal_id: "person:local:hosted-product-navigation-smoke",
      }),
      "page_status",
    );
    if (predicate(lastStatus)) {
      return lastStatus;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`timed out waiting for ${label}: ${JSON.stringify(lastStatus)}`);
}

async function command(adapter, pageId, event, label) {
  const result = expectOk(
    await adapter.request({
      op: "input",
      page_id: pageId,
      principal_id: "person:local:hosted-product-navigation-smoke",
      event,
    }),
    label,
  );
  if (result.schema !== "elastos.browser.input-result/v1" || result.accepted !== true || result.direct_network !== false) {
    throw new Error(`${label} returned invalid input result: ${JSON.stringify(result)}`);
  }
  return result;
}

async function runPopupPolicyCheck(args, adapter, pageId) {
  if (!args.cdpEndpoint) {
    return null;
  }
  const playwright = await import("../elastos/tools/browser-playwright-engine/node_modules/playwright/index.js");
  const { chromium } = playwright.default || playwright;
  const browser = await chromium.connectOverCDP(args.cdpEndpoint);
  try {
    const deadline = Date.now() + args.timeoutMs;
    let page = null;
    while (Date.now() < deadline && !page) {
      for (const candidate of browser.contexts().flatMap((context) => context.pages())) {
        if (candidate.url() === args.firstUrl) {
          page = candidate;
          break;
        }
      }
      if (!page) {
        await new Promise((resolve) => setTimeout(resolve, 250));
      }
    }
    if (!page) {
      throw new Error("popup policy check could not find the launched page");
    }
    const context = page.context();
    const pageCountBefore = context.pages().length;
    await page.evaluate((url) => {
      window.open(url, "_blank");
    }, args.secondUrl);
    await page.waitForURL(args.secondUrl, { timeout: args.timeoutMs });
    const pageCountAfter = context.pages().length;
    if (pageCountAfter !== pageCountBefore) {
      throw new Error(`popup policy created hidden page target: ${pageCountBefore} -> ${pageCountAfter}`);
    }
    const popupStatus = await waitForStatus(
      adapter,
      pageId,
      (status) => status.actual_url === args.secondUrl && status.can_go_back === true,
      args.timeoutMs,
      "popup in-place status",
    );
    await command(adapter, pageId, { type: "browser_command", command: "back" }, "popup back");
    await waitForStatus(
      adapter,
      pageId,
      (status) => status.actual_url === args.firstUrl && status.can_go_forward === true,
      args.timeoutMs,
      "popup back status",
    );
    return {
      page_count_before: pageCountBefore,
      page_count_after: pageCountAfter,
      actual_url: popupStatus.actual_url,
    };
  } finally {
    await browser.close().catch(() => {});
  }
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const config = JSON.parse(fs.readFileSync(args.adapterConfig, "utf8"));
  const adapter = new AdapterClient(args.adapterBin);
  let pageId = "";
  try {
    expectOk(await adapter.request({ op: "init", config }), "init");
    const launchedPage = expectOk(
      await adapter.request({
        op: "launch",
        url: args.firstUrl,
        stream_session: streamSessionFor(args.firstUrl),
        principal_id: "person:local:hosted-product-navigation-smoke",
        reason: "verify hosted product navigation",
        display_mode: "webrtc_remote_display",
        viewport: { width: 1280, height: 720 },
      }),
      "launch",
    );
    pageId = launchedPage.page_id;
    const session = launchedPage.display_session || {};
    if (
      session.schema !== "elastos.browser.display-session/v1" ||
      session.mode !== "webrtc_remote_display" ||
      session.backend_class !== "product_compositor" ||
      session.direct_network !== false
    ) {
      throw new Error(`unexpected hosted display session: ${JSON.stringify(session)}`);
    }

    await waitForStatus(
      adapter,
      pageId,
      (status) => status.actual_url === args.firstUrl,
      args.timeoutMs,
      "initial page status",
    );
    const popup_policy = await runPopupPolicyCheck(args, adapter, pageId);
    await command(
      adapter,
      pageId,
      { type: "browser_command", command: "navigate", url: args.secondUrl },
      "navigate",
    );
    const afterNavigate = await waitForStatus(
      adapter,
      pageId,
      (status) => status.actual_url === args.secondUrl && status.can_go_back === true,
      args.timeoutMs,
      "navigate status",
    );
    await command(adapter, pageId, { type: "browser_command", command: "back" }, "back");
    const afterBack = await waitForStatus(
      adapter,
      pageId,
      (status) => status.actual_url === args.firstUrl && status.can_go_forward === true,
      args.timeoutMs,
      "back status",
    );
    await command(adapter, pageId, { type: "browser_command", command: "forward" }, "forward");
    const afterForward = await waitForStatus(
      adapter,
      pageId,
      (status) => status.actual_url === args.secondUrl && status.can_go_back === true,
      args.timeoutMs,
      "forward status",
    );
    const afterReload = await command(adapter, pageId, { type: "browser_command", command: "reload" }, "reload");
    if (afterReload.actual_url !== args.secondUrl) {
      throw new Error(`reload changed URL unexpectedly: ${JSON.stringify(afterReload)}`);
    }

    console.log(JSON.stringify({
      ok: true,
      schema: "elastos.browser.hosted-product-navigation-smoke/v1",
      page_id: pageId,
      display_backend: session.display_backend,
      backend_class: session.backend_class,
      first_url: args.firstUrl,
      second_url: args.secondUrl,
      after_navigate: {
        actual_url: afterNavigate.actual_url,
        can_go_back: afterNavigate.can_go_back,
        can_go_forward: afterNavigate.can_go_forward,
      },
      after_back: {
        actual_url: afterBack.actual_url,
        can_go_back: afterBack.can_go_back,
        can_go_forward: afterBack.can_go_forward,
      },
      after_forward: {
        actual_url: afterForward.actual_url,
        can_go_back: afterForward.can_go_back,
        can_go_forward: afterForward.can_go_forward,
      },
      popup_policy,
      direct_network: false,
    }));
  } finally {
    await adapter.close(pageId);
  }
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
});
