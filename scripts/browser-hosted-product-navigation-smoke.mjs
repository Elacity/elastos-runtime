#!/usr/bin/env node
import { spawn } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import { createInterface } from "node:readline";
import process from "node:process";

const SMOKE_RUN_ID = crypto.randomBytes(8).toString("hex");
const SMOKE_STREAM_ID = `stream:hosted-product-navigation-smoke:${SMOKE_RUN_ID}`;
const SMOKE_PRINCIPAL_ID = "person:local:hosted-product-navigation-smoke";
const RUNTIME_STREAM_PATH = `/tmp/elastos-browser-product-navigation-smoke-${SMOKE_RUN_ID}-runtime.sock`;
const ADAPTER_IPC_PATH = `/tmp/elastos-browser-product-navigation-smoke-${SMOKE_RUN_ID}-adapter.sock`;

function usage() {
  console.error(`Usage:
  scripts/browser-hosted-product-navigation-smoke.mjs \\
    --adapter-config /path/to/browser-engine-adapter.json \\
    [--adapter-bin capsules/browser-engine-adapter/target/debug/browser-engine-adapter] \\
    [--relay-ipc-path /path/to/browser-exit-relay.sock] \\
    [--first-url https://example.com/] \\
    [--second-url https://example.com/?elastos-browser-nav-smoke=1] \\
    [--guarantee-level operator_rbi|mechanism_microvm] \\
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
    guaranteeLevel: "operator_rbi",
    cdpEndpoint: "",
    relayIpcPath: "",
    timeoutMs: 30_000,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--adapter-bin") {
      args.adapterBin = argv[++index] || "";
    } else if (arg === "--adapter-config") {
      args.adapterConfig = argv[++index] || "";
    } else if (arg === "--relay-ipc-path") {
      args.relayIpcPath = argv[++index] || "";
    } else if (arg === "--first-url") {
      args.firstUrl = argv[++index] || "";
    } else if (arg === "--second-url") {
      args.secondUrl = argv[++index] || "";
    } else if (arg === "--guarantee-level") {
      args.guaranteeLevel = argv[++index] || "";
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
  if (args.relayIpcPath && !args.relayIpcPath.startsWith("/")) {
    throw new Error("--relay-ipc-path must be absolute when provided");
  }
  if (args.relayIpcPath && !fs.statSync(args.relayIpcPath).isSocket()) {
    throw new Error("--relay-ipc-path must point to a Unix socket");
  }
  if (!["operator_rbi", "mechanism_microvm"].includes(args.guaranteeLevel)) {
    throw new Error("--guarantee-level must be operator_rbi or mechanism_microvm");
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

  async close(pageId, principalId) {
    let closeError = null;
    try {
      if (!this.closed && pageId) {
        expectOk(
          await this.request({ op: "close_page", page_id: pageId, principal_id: principalId }),
          "close_page",
        );
      }
    } catch (error) {
      closeError = error;
    } finally {
      if (!this.closed) {
        await this.request({ op: "shutdown" }).catch(() => {});
        this.child.stdin.end();
        this.child.kill();
      }
    }
    if (closeError) {
      throw closeError;
    }
  }
}

function expectOk(response, label) {
  if (!response || response.status !== "ok") {
    throw new Error(`${label} failed: ${response?.code || "unknown"} ${response?.message || ""}`);
  }
  return response.data || {};
}

function streamSessionFor(url, relayIpcPath = "") {
  const target = new URL(url);
  const port = target.port || (target.protocol === "https:" ? "443" : "80");
  const scheme = target.protocol === "https:" ? "tls" : "tcp";
  const session = {
    schema: "elastos.exit.stream-session/v1",
    stream_id: SMOKE_STREAM_ID,
    target: `${scheme}://${target.hostname}:${port}`,
    byte_transport: "adapter_ipc",
    adapter_ipc: {
      schema: "elastos.adapter-ipc/v1",
      kind: "unix_socket",
      path: ADAPTER_IPC_PATH,
      stream_id: SMOKE_STREAM_ID,
      runtime_stream_path: RUNTIME_STREAM_PATH,
    },
  };
  if (relayIpcPath) {
    session.relay_ipc = {
      schema: "elastos.exit.relay-ipc/v1",
      kind: "unix_socket",
      path: relayIpcPath,
      stream_id: session.stream_id,
    };
  }
  return session;
}

async function startRuntimeStreamForwarder(relayIpcPath) {
  if (!relayIpcPath) {
    return null;
  }
  try {
    const existing = fs.lstatSync(RUNTIME_STREAM_PATH);
    if (!existing.isSocket()) {
      throw new Error(`refusing to replace non-socket runtime stream path: ${RUNTIME_STREAM_PATH}`);
    }
    fs.unlinkSync(RUNTIME_STREAM_PATH);
  } catch (error) {
    if (error?.code !== "ENOENT") {
      throw error;
    }
  }
  const sockets = new Set();
  const server = net.createServer((client) => {
    const relay = net.createConnection(relayIpcPath);
    sockets.add(client);
    sockets.add(relay);
    const untrack = (socket) => {
      sockets.delete(socket);
    };
    const abort = () => {
      sockets.delete(client);
      sockets.delete(relay);
      client.destroy();
      relay.destroy();
    };
    client.on("error", abort);
    relay.on("error", abort);
    client.on("close", () => untrack(client));
    relay.on("close", () => untrack(relay));
    client.pipe(relay);
    relay.pipe(client);
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(RUNTIME_STREAM_PATH, () => {
      server.off("error", reject);
      resolve();
    });
  });
  return async () => {
    for (const socket of sockets) {
      socket.destroy();
    }
    await new Promise((resolve) => server.close(resolve));
    fs.rmSync(RUNTIME_STREAM_PATH, { force: true });
  };
}

function browserProfile() {
  const principalRoot = `smoke-${SMOKE_RUN_ID}`;
  const profileKey = `profile-${crypto.randomBytes(32).toString("hex")}`;
  return {
    schema: "elastos.browser.profile/v1",
    scope: "active_principal",
    storage: "principal_owned_profile_disk",
    storage_posture: "principal_owned_reset_scoped_unprotected",
    protected_storage: false,
    encrypted: false,
    recoverable: false,
    recovery: "not_recovery_kit_packaged",
    uri: `localhost://Users/${principalRoot}/BrowserProfiles/default/profile.ext4`,
    public_uri: "localhost://Users/self/BrowserProfiles/default/profile.ext4",
    profile_key: profileKey,
    disk_path: `/tmp/elastos-browser-hosted-product-navigation/${principalRoot}/BrowserProfiles/default/profile.ext4`,
    reset: "whole_profile",
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
        principal_id: SMOKE_PRINCIPAL_ID,
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
      principal_id: SMOKE_PRINCIPAL_ID,
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
  const closeRuntimeStreamForwarder = await startRuntimeStreamForwarder(args.relayIpcPath);
  let pageId = "";
  let runError = null;
  try {
    expectOk(await adapter.request({ op: "init", config }), "init");
    const launchedPage = expectOk(
      await adapter.request({
        op: "launch",
        url: args.firstUrl,
        stream_session: streamSessionFor(args.firstUrl, args.relayIpcPath),
        profile: browserProfile(),
        principal_id: SMOKE_PRINCIPAL_ID,
        reason: "verify hosted product navigation",
        display_mode: "webrtc_remote_display",
        guarantee_level: args.guaranteeLevel,
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
      guarantee_level: args.guaranteeLevel,
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
  } catch (error) {
    runError = error;
    throw error;
  } finally {
    try {
      await adapter.close(pageId, SMOKE_PRINCIPAL_ID);
    } catch (error) {
      if (!runError) {
        throw error;
      }
      console.error(`cleanup warning: ${error instanceof Error ? error.message : String(error)}`);
    }
    if (closeRuntimeStreamForwarder) {
      await closeRuntimeStreamForwarder();
    }
  }
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
});
