import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readFileSync, mkdirSync } from "node:fs";
import http from "node:http";
import path from "node:path";
import test from "node:test";

const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");
const root = new URL("../", import.meta.url);

test("Browser paints a status before scripts, through preparation and actionable failures", async () => {
  let releaseSummary, failure, opened;
  let summaryReady = new Promise(resolve => { releaseSummary = resolve; });
  const server = http.createServer(async (request, response) => {
    const url = new URL(request.url, "http://localhost");
    response.setHeader("cache-control", "no-store");
    if (url.pathname === "/api/apps/browser/summary") {
      await summaryReady;
      response.setHeader("content-type", "application/json");
      response.end(JSON.stringify({ engine_adapter: { adapters: [] },
        sessions: { schema: "elastos.browser.session-capacity/v1", status: "configured", fresh_start_allowed: true, recoverable_page: null } }));
    } else if (url.pathname === "/api/apps/browser/open") {
      opened++;
      response.setHeader("content-type", "application/json");
      response.end(JSON.stringify({ schema: "elastos.browser.open-accepted/v1", status_url: "/api/apps/browser/open/test" }));
    } else if (url.pathname === "/api/apps/browser/open/test") {
      response.setHeader("content-type", "application/json");
      response.end(JSON.stringify({ schema: "elastos.browser.open-status/v1",
        status: failure ? "failed" : "pending", error: failure }));
    } else {
      const match = url.pathname.match(/^\/apps\/(browser|home)\/(.*)$/);
      if (!match || match[2].includes("..")) { response.writeHead(404).end(); return; }
      const name = match[2] || "index.html";
      try {
        const bytes = readFileSync(new URL(`capsules/${match[1]}/browser/${name}`, root));
        response.setHeader("content-type", name.endsWith(".js") ? "text/javascript" : name.endsWith(".css") ? "text/css" : "text/html");
        response.end(bytes);
      } catch { response.writeHead(404).end(); }
    }
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const origin = `http://127.0.0.1:${server.address().port}`;
  const browser = await chromium.launch({ headless: true });
  try {
    for (const viewport of [{ width: 1280, height: 800 }, { width: 390, height: 844 }]) {
      const context = await browser.newContext({ viewport, javaScriptEnabled: false });
      const page = await context.newPage();
      await page.goto(`${origin}/apps/browser/`);
      const status = page.locator("#browser-status");
      assert.match(await status.innerText(), /checking its Engine.*retry/);
      assert.equal(await status.evaluate(node => getComputedStyle(node).opacity), "1");
      if (process.env.RUNNER_TEMP) {
        const dir = path.join(process.env.RUNNER_TEMP, "browser-status-screenshots");
        mkdirSync(dir, { recursive: true });
        await page.screenshot({ path: path.join(dir, `browser-startup-${viewport.width}.png`) });
      }
      await context.close();
    }
    for (const [stage, reason, code, expected] of [
      ["engine_compatibility", "", "engine_not_found", /Engine is unavailable.*Choose/],
      ["engine_readiness", "artifact_invalid", "", /files need repair.*Prepare/],
      ["engine_readiness", "host_unsupported", "", /virtualization is unavailable.*choose/i],
    ]) {
      failure = null; opened = 0;
      summaryReady = new Promise(resolve => { releaseSummary = resolve; });
      const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
      await page.goto(`${origin}/apps/browser/#home_token=isolated-test-token`);
      const status = page.locator("#browser-status");
      await page.waitForFunction(() => document.querySelector("#browser-status")?.dataset.visible === "true");
      assert.match(await status.innerText(), /checking its Engine/);
      assert.equal(opened, 0);
      releaseSummary();
      await page.waitForFunction(() => document.querySelector("#browser-status")?.textContent.includes("Browser is preparing"));
      assert.equal(opened, 1);
      failure = { schema: "elastos.browser.open-error/v1", http_status: 503, stage, reason, code,
        outcome: { schema: "elastos.browser.open-outcome/v1", state: "terminal_pre_effect_failure",
          effects: { page_acquired: false, vm_acquired: false, stream_acquired: false } } };
      await page.waitForFunction(() => document.querySelector("#browser-url")?.disabled === false);
      assert.match(await status.innerText(), expected);
      assert.equal(await status.evaluate(node => getComputedStyle(node).opacity), "1");
      const box = await status.boundingBox();
      assert.ok(box.width <= 390 && box.x >= 0, "the complete action stays inside a phone view");
      assert.equal(opened, 1, "a preparation failure does not allocate a replacement page");
      await page.close();
    }
  } finally {
    releaseSummary();
    await browser.close();
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
  }
});
