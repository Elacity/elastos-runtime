#!/usr/bin/env node
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readFileSync, statfsSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { randomUUID } from "node:crypto";
import { MODEL_TEXT_INPUT_SCHEMA } from "../capsules/assistant/browser/model-contract.js";
const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");
const [base, evidence, data] = process.argv.slice(2);
const { cid } = JSON.parse(readFileSync(join(evidence, "package.json")));
const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const page = await context.newPage();
const cdp = await context.newCDPSession(page);
await cdp.send("WebAuthn.enable");
await cdp.send("WebAuthn.addVirtualAuthenticator", { options: {
  protocol: "ctap2", transport: "internal", hasResidentKey: true,
  hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true,
} });
let stage = "sign_in";
const results = {};
const disk = {};
function observeDisk() {
  const usage = statfsSync(data);
  return { capacity_bytes: usage.blocks * usage.bsize, available_bytes: usage.bavail * usage.bsize };
}
async function app(target) {
  const shell = page.frameLocator("#active-shell-frame");
  const [control, surface] = {
    marketplace: ['.taskbar-item[data-target="marketplace"]', 'section.window[data-target="marketplace"] iframe.window-frame'],
    assistant: ["#assistant-toggle", "#assistant-space-frame"],
  }[target];
  await shell.locator(control).click();
  const iframe = shell.locator(surface).last();
  await iframe.waitFor({ state: "visible", timeout: 30000 });
  const frame = await (await iframe.elementHandle()).contentFrame();
  await frame.waitForURL(url => url.pathname.startsWith(`/apps/${target}/`));
  return frame;
}
async function request(frame, path, body) {
  return frame.evaluate(async ({ path, body }) => {
    const token = new URLSearchParams(location.hash.slice(1)).get("home_token");
    if (!token) throw new Error("missing capsule launch authority");
    const response = await fetch(path, { method: body ? "POST" : "GET",
      headers: { "x-elastos-home-token": token, ...(body ? { "content-type": "application/json" } : {}) },
      ...(body ? { body: JSON.stringify(body) } : {}), signal: AbortSignal.timeout(35000) });
    if (!response.ok) {
      const failure = await response.json().catch(() => ({}));
      // Retain the public protocol code; response text can contain operator data.
      const code = typeof failure.code === "string" && /^[a-z][a-z0-9_]{0,63}$/.test(failure.code) ? failure.code : "unavailable";
      throw new Error(`Runtime request failed: ${path} ${response.status} code=${code}`);
    }
    return response.json();
  }, { path, body });
}
try {
  assert((await page.goto(`${base}/home/`, { waitUntil: "domcontentloaded" })).ok(), "installed Home frontdoor responds successfully");
  await page.locator("#home-unlock-name").waitFor({ state: "visible", timeout: 60000 });
  await page.locator("#home-unlock-name").fill("CI journey");
  const completed = page.waitForResponse(response => response.url().endsWith("/api/auth/passkey/register/complete"));
  await page.locator("#home-unlock-primary").click();
  assert((await completed).ok(), "new isolated Home passkey registration");
  await page.waitForFunction(() => document.body.dataset.homeStatus === "ready" && document.body.dataset.homeAuthority === "signed", null, { timeout: 60000 });
  const shell = page.frameLocator("#active-shell-frame");
  await shell.locator("#desktop").waitFor({ state: "visible", timeout: 30000 });
  const setupReminder = shell.locator("#setup-sheet");
  if (await setupReminder.isVisible()) {
    await shell.locator("#setup-sheet-close").click();
    await setupReminder.waitFor({ state: "hidden", timeout: 5000 });
  }
  await page.screenshot({ path: join(evidence, "home-desktop.png"), fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  await shell.locator("#desktop").waitFor({ state: "visible" });
  await page.screenshot({ path: join(evidence, "home-phone.png"), fullPage: true });
  results.home_screenshots = "passed";
  await page.setViewportSize({ width: 1440, height: 900 });
  stage = "model_package_admission";
  const marketplace = await app("marketplace");
  const interfaces = await request(marketplace, "/api/capsules/interfaces");
  const content = interfaces.interfaces.find(row => row.capsule === "marketplace" && row.bindings.some(b => b.method === "content.use" && b.executable));
  assert(content, "Marketplace declares Content Use");
  disk.before_content_use = observeDisk();
  const used = await request(marketplace, "/api/capsules/interfaces/invoke", {
    capsule: "marketplace", interface: content.interface.id, method: "content.use",
    request_id: `ci-use-${randomUUID()}`, input: { cid },
  });
  assert.equal(used.status, "ok");
  let offer;
  const admissionDeadline = Date.now() + 180000;
  while (Date.now() < admissionDeadline) {
    const catalog = await request(marketplace, "/api/capsules/catalog");
    const row = catalog.capsules.find(row => row.cid === cid);
    if (row?.model_runtime?.dispatch_ready) { offer = row.model_runtime.offer_id; break; }
    assert(!["failed", "cancelled"].includes(row?.model_runtime?.preparation?.state), "model preparation settles successfully");
    await new Promise(resolve => setTimeout(resolve, 500));
  }
  assert(offer, "installed Runtime admits the pinned package and publishes its offer");
  results.model_package_admission = "passed";
  stage = "installed_runtime_reply";
  const assistant = await app("assistant");
  const created = await request(assistant, "/api/provider/model/runs_create", {
    offer_id: offer, operation: "text.generate", request_id: `ci-reply-${randomUUID()}`,
    input: { schema: MODEL_TEXT_INPUT_SCHEMA, prompt: "Say hello in one sentence." },
  });
  assert.equal(created.status, "ok");
  const run_id = created.data.run_id;
  assert(run_id);
  let terminal;
  const replyDeadline = Date.now() + 150000;
  while (Date.now() < replyDeadline) {
    const got = await request(assistant, "/api/provider/model/runs_get", { run_id, request_id: `ci-get-${randomUUID()}` });
    if (["completed", "failed", "cancelled", "settlement_unknown"].includes(got.data.status)) { terminal = got.data; break; }
    await new Promise(resolve => setTimeout(resolve, 250));
  }
  assert.equal(terminal?.status, "completed", `installed Runtime reply outcome: ${terminal?.terminal?.error?.code || "deadline"}`);
  const text = terminal.terminal.output.text;
  assert.equal(typeof text, "string");
  assert(text.trim().length > 0);
  results.installed_runtime_reply = "passed";
  writeFileSync(join(evidence, "home-journey.json"), JSON.stringify({ results, disk, reply_characters: text.length, model_cid: cid }, null, 2));
} catch (error) {
  results[stage] = "failed";
  disk.at_failure = observeDisk();
  writeFileSync(join(evidence, "home-journey.json"), JSON.stringify({ results, disk, failure_stage: stage, failure: error.message }, null, 2));
  await page.screenshot({ path: join(evidence, "home-failure.png"), fullPage: true }).catch(() => {});
  throw error;
} finally {
  await browser.close();
}
