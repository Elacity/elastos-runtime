// ELASTOS_BROWSER_EXECUTABLE=/path/to/chromium node --test scripts/browser-chromium-loopback-proxy.test.mjs
// Uses the existing Playwright dependency and temporary headless browser profiles.
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { readFileSync } from "node:fs";
import http from "node:http";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";

const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");
const stage = readFileSync(new URL("./build/stage-browser-vm-target.sh", import.meta.url), "utf8");
const guestProxyArgs = [...stage.matchAll(/"(--(?:proxy-server|proxy-bypass-list|host-resolver-rules)=[^"]+)"/g)]
  .map(match => match[1]);

function listen(server) {
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      server.off("error", reject);
      resolve(server.address().port);
    });
  });
}

function respond(response, status, title) {
  response.writeHead(status, {
    "content-type": "text/html; charset=utf-8", "cache-control": "no-store", "connection": "close",
  });
  response.end(`<!doctype html><title>${title}</title><link rel="icon" href="data:,"><h1>${title}</h1>`);
}

async function withFixture(run) {
  const state = { receipts: [], directHits: 0 };
  const sockets = new Set();
  const direct = http.createServer((_request, response) => {
    state.directHits++;
    respond(response, 200, "Direct connection trap");
  });
  const proxy = http.createServer((request, response) => {
    let target;
    try { target = new URL(request.url); } catch { respond(response, 400, "Absolute proxy URL required"); return; }
    const approved = request.method === "GET" && target.protocol === "http:"
      && ["localhost", "127.0.0.1"].includes(target.hostname)
      && target.port === String(state.destinationPort) && !target.username && !target.password;
    if (state.receipts.length >= 128) { respond(response, 429, "Receipt limit"); return; }
    state.receipts.push({ target: target.href, approved });
    // The test proxy serves fixed pages. It opens no upstream connections.
    respond(response, approved ? 200 : 403, approved ? "Proxy approved" : "Proxy denied");
  });
  proxy.on("connect", (_request, socket) => socket.destroy());
  const servers = [direct, proxy];
  for (const server of servers) {
    server.on("connection", socket => {
      sockets.add(socket);
      socket.once("close", () => sockets.delete(socket));
    });
  }
  try {
    state.destinationPort = await listen(direct);
    state.proxyPort = await listen(proxy);
    return await run({ state, servers, proxyUrl: `http://127.0.0.1:${state.proxyPort}` });
  } finally {
    const closed = servers.map(server => new Promise(resolve => server.close(resolve)));
    for (const socket of sockets) socket.destroy();
    await Promise.all(closed);
  }
}

test("browser launch failure releases both fixture listeners", async () => {
  let servers;
  await assert.rejects(withFixture(async fixture => {
    servers = fixture.servers;
    await chromium.launch({
      executablePath: path.join(tmpdir(), randomUUID(), "missing-chromium"),
      headless: true, timeout: 2000,
    });
  }), /executable.*doesn.t exist/i);
  assert.ok(servers.every(server => !server.listening));
});

test("guest Chromium proxy flags send loopback destinations through proxy policy", async t => {
  assert.ok(guestProxyArgs.includes("--proxy-server={proxy_url}"));
  assert.ok(guestProxyArgs.includes("--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1"));
  for (const fixed of [false, true]) {
    await t.test(fixed ? "guest flags enforce proxy policy" : "old flags reproduce implicit loopback bypass", async t => {
      await withFixture(async fixture => {
        const { state, proxyUrl } = fixture;
        let browser;
        try {
          browser = await chromium.launch({
            executablePath: process.env.ELASTOS_BROWSER_EXECUTABLE || undefined,
            headless: true, timeout: 15_000,
            // Playwright's proxy option changes bypass behavior. Use raw guest arguments.
            args: [
              ...guestProxyArgs.filter(arg => fixed || !arg.startsWith("--proxy-bypass-list="))
                .map(arg => arg.replace("{proxy_url}", proxyUrl)),
              "--disable-background-networking", "--disable-component-update", "--disable-default-apps",
              "--disable-quic", "--no-first-run",
            ],
          });
          const page = await browser.newPage();
          const results = [];
          for (const host of fixed ? ["localhost", "127.0.0.1"] : ["localhost"]) {
            const target = `http://${host}:${state.destinationPort}/main`;
            const start = state.receipts.length;
            const directBefore = state.directHits;
            let error;
            const response = await page.goto(target, { waitUntil: "domcontentloaded", timeout: 10_000 })
              .catch(cause => { error = cause.message.split("\n")[0]; });
            const proxied = state.receipts.slice(start).some(receipt => receipt.target === target && receipt.approved);
            assert.equal(proxied, fixed, target);
            if (fixed) {
              assert.equal(error, undefined, error);
              assert.equal(response.status(), 200);
              assert.equal(await page.title(), "Proxy approved");
              assert.equal(state.directHits, directBefore, "approved navigation bypassed the proxy");
            } else {
              assert.match(error || "", /ERR_NAME_NOT_RESOLVED/);
            }
            results.push({ host, proxied, error: error || null });
          }
          if (fixed) {
            for (const host of ["localhost", "127.0.0.1", "unapproved.invalid"]) {
              // Test host and port rejection separately against the exact grant.
              const port = host === "unapproved.invalid" ? state.destinationPort : state.proxyPort;
              const target = `http://${host}:${port}/unapproved`;
              const start = state.receipts.length;
              const response = await page.goto(target, { waitUntil: "domcontentloaded", timeout: 10_000 });
              assert.equal(response.status(), 403);
              assert.equal(await page.title(), "Proxy denied");
              assert.ok(state.receipts.slice(start).some(receipt => receipt.target === target && !receipt.approved));
            }
            assert.equal(state.directHits, 0, "a navigation reached the direct connection trap");
          }
          t.diagnostic(JSON.stringify({ browser_version: browser.version(), guest_flags: fixed, results }));
        } finally {
          await browser?.close();
        }
      });
    });
  }
});
