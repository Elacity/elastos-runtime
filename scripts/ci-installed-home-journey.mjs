#!/usr/bin/env node
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readFileSync, statfsSync, writeFileSync } from "node:fs";
import { join } from "node:path";
const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");
const [base, evidence, data, mode] = process.argv.slice(2);
assert(!mode || mode === "--home-only", "supported installed journey mode required");
const cid = mode ? null : JSON.parse(readFileSync(join(evidence, "package.json"))).cid;
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
async function projection(frame, path) {
  return frame.evaluate(async path => {
    const token = new URLSearchParams(location.hash.slice(1)).get("home_token");
    if (!token) throw new Error("missing capsule launch authority");
    const response = await fetch(path, { headers: { "x-elastos-home-token": token }, signal: AbortSignal.timeout(35000) });
    if (!response.ok) {
      const failure = await response.json().catch(() => ({}));
      // Retain the public protocol code; response text can contain operator data.
      const code = typeof failure.code === "string" && /^[a-z][a-z0-9_]{0,63}$/.test(failure.code) ? failure.code : "unavailable";
      throw new Error(`Runtime request failed: ${path} ${response.status} code=${code}`);
    }
    return response.json();
  }, path);
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
  if (mode === "--home-only") {
    const marketplace = await app("marketplace");
    const catalog = await projection(marketplace, "/api/capsules/catalog");
    assert.equal(catalog.model_catalog_state, "unconfigured", "fresh ordinary Home needs no model catalogue");
    results.engine_absent_home = "passed";
    writeFileSync(join(evidence, "home-journey.json"), JSON.stringify({ results, disk, journey: "fresh Home with optional model engine absent" }, null, 2));
  } else {
    stage = "model_package_admission";
    const marketplace = await app("marketplace");
    const initialCatalog = await projection(marketplace, "/api/capsules/catalog");
    assert.equal(initialCatalog.model_catalog_state, "verified");
    const initialModel = initialCatalog.capsules.find(row => row.cid === cid);
    assert.equal(initialModel?.signature_state, "catalog-signature-verified");
    assert.equal(initialModel?.model_runtime?.dispatch_ready, false, "fresh Home starts before model admission");
    await marketplace.locator(`[data-action="model-detail"][data-app="model:${cid}"]`).click();
    const get = marketplace.locator('[data-model-control="use"]');
    await get.waitFor({ state: "visible", timeout: 30000 });
    assert.equal((await get.textContent()).trim(), "Get");
    disk.before_content_use = observeDisk();
    const useResponse = page.waitForResponse(response => {
      if (!response.url().endsWith("/api/capsules/interfaces/invoke") || response.request().frame() !== marketplace) return false;
      const body = response.request().postDataJSON();
      return body?.capsule === "marketplace" && body?.method === "content.use" && body?.input?.cid === cid;
    }, { timeout: 60000 });
    useResponse.catch(() => {});
    await get.click();
    const usedResponse = await useResponse;
    assert(usedResponse.ok(), "Marketplace Get invokes Content Use successfully");
    assert.equal((await usedResponse.json()).status, "ok");
    const openAssistant = marketplace.locator('[data-model-control="open-assistant"]');
    await openAssistant.waitFor({ state: "visible", timeout: 180000 });
    const catalog = await projection(marketplace, "/api/capsules/catalog");
    const readyModel = catalog.capsules.find(row => row.cid === cid);
    assert.equal(readyModel?.model_runtime?.dispatch_ready, true);
    const offer = readyModel.model_runtime.offer_id;
    assert(/^model:[0-9a-f]{64}$/.test(offer));
    results.model_package_admission = "passed";
    await page.screenshot({ path: join(evidence, "model-ready.png"), fullPage: true });
    stage = "installed_runtime_reply";
    await openAssistant.click();
    const assistantElement = page.frameLocator("#active-shell-frame").locator("#assistant-space-frame");
    await assistantElement.waitFor({ state: "visible", timeout: 30000 });
    const assistant = await (await assistantElement.elementHandle()).contentFrame();
    await assistant.waitForURL(url => url.pathname.startsWith("/apps/assistant/"));
    await assistant.locator("#agent-model-picker").click();
    const selection = assistant.locator(`#agent-model-menu [role="option"][data-model-cid="${cid}"]`);
    await selection.waitFor({ state: "visible", timeout: 30000 });
    assert.equal(await selection.getAttribute("data-live-offer-id"), offer);
    await selection.click();
    await assistant.locator("#agent-model-menu").waitFor({ state: "hidden", timeout: 40000 });
    await assistant.locator('[data-sidebar-nav="configure"]').click();
    await assistant.locator('[data-configure-section="prompt"]').click();
    await assistant.locator("[data-agent-max-tokens]").fill("16");
    await assistant.locator("[data-page-close]").click();
    const createdResponse = page.waitForResponse(response => {
      if (!response.url().endsWith("/api/provider/model/runs_create") || response.request().frame() !== assistant) return false;
      const body = response.request().postDataJSON();
      return body?.offer_id === offer && body?.operation === "text.generate";
    }, { timeout: 30000 });
    createdResponse.catch(() => {});
    const terminalResponse = page.waitForResponse(async response => {
      if (!response.url().endsWith("/api/provider/model/runs_events") || response.request().frame() !== assistant) return false;
      const created = await (await createdResponse).json();
      if (response.request().postDataJSON()?.run_id !== created.data?.run_id) return false;
      const body = await response.json();
      return body.data?.events?.some(event => ["completed", "failed", "cancelled", "settlement_unknown"].includes(event.kind));
    }, { timeout: 150000 });
    terminalResponse.catch(() => {});
    await assistant.locator("#agent-composer-input").fill("Say hello in one sentence.");
    await assistant.locator("#agent-composer-send").click();
    const acceptedResponse = await createdResponse;
    assert(acceptedResponse.ok(), "Assistant submits its normal model request");
    const request = acceptedResponse.request().postDataJSON();
    assert.equal(request.offer_id, offer);
    assert.equal(request.operation, "text.generate");
    assert.equal(request.input?.max_output_tokens, 16, "normal Assistant controls bound the greeting");
    const assistantToken = await assistant.evaluate(() => new URLSearchParams(location.hash.slice(1)).get("home_token"));
    assert(Boolean(assistantToken) && acceptedResponse.request().headers()["x-elastos-home-token"] === assistantToken, "Assistant uses its own launch authority");
    const created = await acceptedResponse.json();
    assert.equal(created.status, "ok");
    assert(created.data?.run_id);
    const finishedResponse = await terminalResponse;
    assert(finishedResponse.ok());
    const finished = await finishedResponse.json();
    const terminal = finished.data.events.find(event => ["completed", "failed", "cancelled", "settlement_unknown"].includes(event.kind));
    assert.equal(terminal.kind, "completed", "installed Runtime completes the visible Assistant turn");
    await assistant.locator('.agent-msg-agent [data-regenerate]:enabled').last().waitFor({ state: "visible", timeout: 10000 });
    const text = (await assistant.locator(".agent-msg-agent .agent-msg-body").last().innerText()).trim();
    assert(text.length > 0, "Assistant displays the real local reply");
    results.installed_runtime_reply = "passed";
    await page.screenshot({ path: join(evidence, "assistant-reply-desktop.png"), fullPage: true });
    await page.setViewportSize({ width: 390, height: 844 });
    const phoneReply = assistant.locator(".agent-msg-agent .agent-msg-body").last();
    await phoneReply.scrollIntoViewIfNeeded();
    const bounds = await phoneReply.boundingBox();
    assert(bounds && bounds.x >= 0 && bounds.y >= 0 && bounds.x + bounds.width <= 390 && bounds.y + bounds.height <= 844, "phone viewport displays the local reply");
    assert.equal((await phoneReply.innerText()).trim(), text);
    await page.screenshot({ path: join(evidence, "assistant-reply-phone.png"), fullPage: true });
    writeFileSync(join(evidence, "home-journey.json"), JSON.stringify({
      results, disk, reply_characters: text.length, model_cid: cid, offer_id: offer,
      max_output_tokens: request.input.max_output_tokens, journey: "Marketplace Get to visible Assistant reply",
    }, null, 2));
  }
} catch (error) {
  results[stage] = "failed";
  disk.at_failure = observeDisk();
  writeFileSync(join(evidence, "home-journey.json"), JSON.stringify({ results, disk, failure_stage: stage, failure: error.message }, null, 2));
  await page.screenshot({ path: join(evidence, "home-failure.png"), fullPage: true }).catch(() => {});
  throw error;
} finally {
  await browser.close();
}
