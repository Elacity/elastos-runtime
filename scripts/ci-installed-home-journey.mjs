#!/usr/bin/env node
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readFileSync, statfsSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { applyRunEventsPage, terminalOutputText } from "../capsules/assistant/browser/model-contract.js";
import { publicJourneyFailure } from "./ci-installed-home-failure.mjs";
const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");
const [base, evidence, data, mode] = process.argv.slice(2);
assert(!mode || mode === "--home-only", "supported installed journey mode required");
const fixture = JSON.parse(readFileSync(join(evidence, "package.json")));
const cid = fixture.cid;
// A holder Get reads the package over Carrier in 64 KiB bounded reads; a same-Home Get is local.
const readyMs = fixture.carrier_holder === true ? 15 * 60 * 1000 : 180000;
let browser, page;
let stage = "journey";
let subcheck = null;
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
  browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  page = await context.newPage();
  const cdp = await context.newCDPSession(page);
  await cdp.send("WebAuthn.enable");
  await cdp.send("WebAuthn.addVirtualAuthenticator", { options: {
    protocol: "ctap2", transport: "internal", hasResidentKey: true,
    hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true,
  } });
  stage = "sign_in";
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
  if (mode === "--home-only") {
    stage = "engine_absent_refusal";
    const recovery = "This source Home needs its local model engine. Install the engine through source setup, then Retry.";
    // Get returns before model preparation finishes. The first refusal uses
    // the same bounded preparation window as the model-ready journey.
    await marketplace.getByText(recovery, { exact: true }).waitFor({ state: "visible", timeout: readyMs });
    const unavailable = (await projection(marketplace, "/api/capsules/catalog")).capsules.find(row => row.cid === cid)?.model_runtime;
    assert.equal(unavailable?.admitted, true, "engine refusal preserves admitted content");
    assert.equal(unavailable?.dispatch_ready, false);
    assert.equal(unavailable?.dispatch_unavailable_reason, "source_engine_required");
    assert.equal(unavailable?.offer_id, null);
    assert.equal(await marketplace.locator('[data-model-control="open-assistant"]').count(), 0);
    assert.equal((await get.textContent()).trim(), "Retry");
    await page.screenshot({ path: join(evidence, "engine-absent-refusal.png"), fullPage: true });
    const retried = page.waitForResponse(response => {
      if (!response.url().endsWith("/api/capsules/interfaces/invoke") || response.request().frame() !== marketplace) return false;
      const body = response.request().postDataJSON();
      return body?.capsule === "marketplace" && body?.method === "content.use" && body?.input?.cid === cid;
    }, { timeout: 60000 });
    retried.catch(() => {});
    await get.click();
    const retryResponse = await retried;
    assert(retryResponse.ok());
    assert.equal((await retryResponse.json()).status, "ok");
    await marketplace.getByText(recovery, { exact: true }).waitFor({ state: "visible", timeout: 90000 });
    const retained = (await projection(marketplace, "/api/capsules/catalog")).capsules.find(row => row.cid === cid)?.model_runtime;
    assert.equal(retained?.admitted, true);
    assert.equal(retained?.dispatch_ready, false);
    assert.equal(retained?.dispatch_unavailable_reason, "source_engine_required");
    const assistant = await app("assistant");
    await assistant.locator("#agent-composer-input").waitFor({ state: "visible", timeout: 30000 });
    assert.equal(await page.getAttribute("body", "data-home-status"), "ready");
    results.engine_absent_refusal = "passed";
    results.engine_absent_home = "passed";
    writeFileSync(join(evidence, "home-journey.json"), JSON.stringify({
      results, disk, model_cid: cid, dispatch_unavailable_reason: retained.dispatch_unavailable_reason,
      journey: "signed Get with optional engine absent, clear Use refusal, Retry and usable Home",
    }, null, 2));
  } else {
    const openAssistant = marketplace.locator('[data-model-control="open-assistant"]');
    await openAssistant.waitFor({ state: "visible", timeout: readyMs });
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
    await assistant.locator('#agent-model-picker[aria-label^="Model: "]').waitFor({ state: "visible", timeout: 30000 });
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
      const request = response.request().postDataJSON();
      if (request?.run_id !== created.data?.run_id) return false;
      const body = await response.json();
      return Boolean(applyRunEventsPage(body.data, request.after_sequence).terminal);
    }, { timeout: 150000 });
    terminalResponse.catch(() => {});
    await assistant.locator("#agent-composer-input").fill("Say hello in one sentence.");
    await assistant.locator("#agent-composer-send").click();
    const acceptedResponse = await createdResponse;
    assert(acceptedResponse.ok(), "Assistant submits its normal model request");
    const request = acceptedResponse.request().postDataJSON();
    assert.equal(request.offer_id, offer);
    assert.equal(request.operation, "text.generate");
    subcheck = "request_cap";
    assert.equal(request.input?.max_output_tokens, 16, "normal Assistant controls bound the greeting");
    subcheck = "request_authority";
    const assistantToken = await assistant.evaluate(() => new URLSearchParams(location.hash.slice(1)).get("home_token"));
    assert(Boolean(assistantToken) && acceptedResponse.request().headers()["x-elastos-home-token"] === assistantToken, "Assistant uses its own launch authority");
    subcheck = null;
    const created = await acceptedResponse.json();
    assert.equal(created.status, "ok");
    assert(created.data?.run_id);
    subcheck = "terminal_status";
    const finishedResponse = await terminalResponse;
    assert(finishedResponse.ok());
    const finished = await finishedResponse.json();
    const terminal = applyRunEventsPage(finished.data, finishedResponse.request().postDataJSON().after_sequence).terminal;
    assert.equal(terminal?.status, "completed", "installed Runtime completes the visible Assistant turn");
    subcheck = "terminal_output";
    assert(terminal.outputRetained !== false && terminalOutputText(terminal.output).trim().length > 0, "Runtime retains the real local reply");
    subcheck = "reply_display";
    await assistant.locator('.agent-msg-agent [data-regenerate]:enabled').last().waitFor({ state: "visible", timeout: 10000 });
    const reply = assistant.locator(".agent-msg-agent .agent-msg-body").last();
    await reply.waitFor({ state: "visible", timeout: 10000 });
    await assistant.waitForFunction(() => {
      const body = [...document.querySelectorAll(".agent-msg-agent .agent-msg-body")].at(-1);
      return body?.closest(".agent-msg-agent")?.dataset.streamPhase === "presentation_done" &&
        /\S/.test(body?.innerText ?? "");
    }, null, { timeout: 10000 });
    const text = (await reply.innerText()).trim();
    assert(text.length > 0, "Assistant displays the real local reply");
    results.installed_runtime_reply = "passed";
    await page.screenshot({ path: join(evidence, "assistant-reply-desktop.png"), fullPage: true });
    subcheck = "reply_bounds";
    await page.setViewportSize({ width: 390, height: 844 });
    await assistant.waitForFunction(() =>
      !document.body.classList.contains("agent-harness-drawer-open") &&
      document.querySelector("#agent-harness-drawer-open")?.getAttribute("aria-expanded") === "false" &&
      document.querySelector("#agent-harness-sidebar")?.getBoundingClientRect().right <= 0);
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
  const failure = publicJourneyFailure(error, stage, subcheck);
  writeFileSync(join(data, "journey-browser.private.log"), String(error?.stack ?? error), { mode: 0o600 });
  writeFileSync(join(evidence, "home-journey.json"), JSON.stringify({ results, disk, ...failure }, null, 2));
  await page?.screenshot({ path: join(evidence, "home-failure.png"), fullPage: true }).catch(() => {});
  throw new Error(`Installed Home journey failed: stage=${failure.failure_stage} code=${failure.failure}${failure.failure_check ? ` check=${failure.failure_check}` : ""}`);
} finally {
  await browser?.close();
}
