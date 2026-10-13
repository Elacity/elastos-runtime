#!/usr/bin/env node
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { appendFileSync, readFileSync, statfsSync, writeFileSync, renameSync } from "node:fs";
import { join } from "node:path";
import { applyRunEventsPage, terminalOutputText } from "../capsules/assistant/browser/model-contract.js";
import { publicJourneyFailure } from "./ci-installed-home-failure.mjs";
import { openInstalledAssistant, returnInstalledHome } from "./ci-installed-home-navigation.mjs";
const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");
const [base, evidence, data, mode, resourceControl] = process.argv.slice(2);
assert(!mode || ["--home-only", "--memory-refusal", "--resource-lifecycle", "--model-repair"].includes(mode), "supported installed journey mode required");
assert(Boolean(resourceControl) === ["--memory-refusal", "--resource-lifecycle", "--model-repair"].includes(mode));
const fixture = JSON.parse(readFileSync(join(evidence, "package.json")));
const cid = fixture.cid;
// A holder Get reads the package over Carrier in 64 KiB bounded reads; a same-Home Get is local.
const readyMs = fixture.carrier_holder === true ? 15 * 60 * 1000 : 180000;
let browser, browserOpening, browserClosing, page;
let journeyFailed = false;
function closeBrowser() {
  browserClosing ??= (async () => {
    const owned = browser ?? await browserOpening;
    await owned?.close();
    if (resourceControl) writeFileSync(join(resourceControl, "browser-stopped.json"), JSON.stringify({ closed: true }), { flag: "wx", mode: 0o600 });
  })();
  return browserClosing;
}
if (resourceControl) {
  for (const [name, code] of [["SIGTERM", 143], ["SIGINT", 130]]) {
    process.once(name, () => { closeBrowser().then(() => process.exit(code), () => process.exit(1)); });
  }
}
let stage = "journey";
let subcheck = null;
const results = {};
const disk = {};
let resourceSequence = 0;
const resourceTurns = [];
const modelRepair = {};
async function resourceAction(action) {
  const id = ++resourceSequence;
  const path = join(resourceControl, "request.json");
  writeFileSync(path + ".next", JSON.stringify({ id, action }), { flag: "wx", mode: 0o600 });
  renameSync(path + ".next", path);
  const deadline = Date.now() + (action === "restart_get" ? 150000 : action === "idle_stopped" ? 100000 : 25000);
  while (Date.now() < deadline) {
    let response;
    try { response = JSON.parse(readFileSync(join(resourceControl, "response.json"), "utf8")); }
    catch (error) { if (error.code !== "ENOENT") throw error; }
    if (response?.id === id) {
      assert.equal(response.status, "passed", "owned finite-resource stage passes");
      return response;
    }
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  assert.fail("owned finite-resource stage deadline");
}
async function configureTokens(assistant, tokens) {
  await assistant.locator('[data-sidebar-nav="configure"]').click();
  await assistant.locator('[data-configure-section="prompt"]').click();
  await assistant.locator("[data-agent-max-tokens]").fill(String(tokens));
  await assistant.locator("[data-page-close]").click();
}
async function visibleFailure(assistant, message) {
  const status = assistant.locator("[data-agent-stream-status]");
  await status.waitFor({ state: "visible", timeout: 10000 });
  await assistant.waitForFunction(message =>
    document.querySelector("[data-agent-stream-status]")?.textContent === message,
  message, { timeout: 10000 });
}
async function selectModel(assistant, offer, modelCid = cid) {
  await assistant.locator("#agent-model-picker").waitFor({ state: "visible", timeout: 30000 });
  await assistant.locator("#agent-model-picker").click();
  const option = assistant.locator(`#agent-model-menu [role="option"][data-model-cid="${modelCid}"]`);
  await option.waitFor({ state: "visible", timeout: 30000 });
  assert.equal(await option.getAttribute("data-live-offer-id"), offer);
  await option.click();
  await assistant.locator("#agent-model-menu").waitFor({ state: "hidden", timeout: 30000 });
}
async function startResourceTurn(assistant, offer, prompt, tokens) {
  const ownerPage = assistant.page();
  const response = ownerPage.waitForResponse(response => {
    if (!response.url().endsWith("/api/provider/model/runs_create") || response.request().frame() !== assistant) return false;
    const body = response.request().postDataJSON();
    return body?.offer_id === offer && body?.operation === "text.generate";
  }, { timeout: 30000 });
  response.catch(() => {});
  const created = response.then(async accepted => {
    assert(accepted.ok());
    const body = accepted.request().postDataJSON();
    assert.equal(body.input?.max_output_tokens, tokens);
    assert(/^model-v1:[1-9][0-9]*:[A-Za-z0-9_.:-]{1,128}$/.test(body.request_id));
    const token = await assistant.evaluate(() => new URLSearchParams(location.hash.slice(1)).get("home_token"));
    assert(token && accepted.request().headers()["x-elastos-home-token"] === token);
    const value = await accepted.json();
    assert.equal(value.status, "ok");
    assert(value.data?.run_id);
    return value.data;
  });
  created.catch(() => {});
  let resolve, reject, timer;
  const terminal = new Promise((yes, no) => { resolve = yes; reject = no; });
  terminal.catch(() => {});
  const listener = async response => {
    try {
      if (!response.url().endsWith("/api/provider/model/runs_events") || response.request().frame() !== assistant) return;
      const run = await created;
      const request = response.request().postDataJSON();
      if (request?.run_id !== run.run_id) return;
      assert(response.ok());
      const body = await response.json();
      const final = applyRunEventsPage(body.data, request.after_sequence).terminal;
      if (final) resolve(final);
    } catch (error) { reject(error); }
  };
  // Capture events before Send; a fast terminal page may precede create parsing.
  ownerPage.on("response", listener);
  timer = setTimeout(() => reject(new Error("finite model turn deadline")), 150000);
  const settled = terminal.finally(() => { clearTimeout(timer); ownerPage.off("response", listener); });
  settled.catch(() => {});
  created.then(run => { if (run.terminal) resolve(run.terminal); }, reject);
  try {
    await assistant.locator("#agent-composer-input").fill(prompt);
    await assistant.locator('#agent-composer-send[data-mode="send"]').click();
    const run = await created;
    return { run, terminal: settled };
  } catch (error) { reject(error); throw error; }
}
async function resourceReply(assistant, offer, label) {
  await assistant.locator('#agent-composer-send[data-mode="send"]').waitFor({ state: "visible", timeout: 10000 });
  await configureTokens(assistant, 16);
  const turn = await startResourceTurn(assistant, offer, "Say hello in one sentence.", 16);
  const final = await turn.terminal;
  assert.equal(final.status, "completed");
  assert(final.outputRetained !== false && terminalOutputText(final.output).trim());
  await assistant.waitForFunction(() => {
    const body = [...document.querySelectorAll(".agent-msg-agent .agent-msg-body")].at(-1);
    return body?.closest(".agent-msg-agent")?.dataset.streamPhase === "presentation_done" && /\S/.test(body?.innerText ?? "");
  }, null, { timeout: 10000 });
  resourceTurns.push({ stage: label, run_id: turn.run.run_id, status: final.status });
  await page.screenshot({ path: join(evidence, `${mode === "--model-repair" ? "repair" : "finite"}-${label}.png`), fullPage: true });
  return turn;
}
async function resourceLifecycle(assistant, offer, context) {
  stage = "finite_resource_lifecycle";
  subcheck = "owner_configure";
  await page.setViewportSize({ width: 1440, height: 900 });
  await assistant.locator("#agent-harness-new-chat").click();
  await configureTokens(assistant, 1024);
  const longPrompt = "Write a numbered list from 1 to 1000. Each item must say hello and give one detail. Continue the list without an introduction or conclusion.";
  subcheck = "contender_launch";
  const secondPage = await context.newPage();
  try {
    // Each Home launches its own Assistant and completes the Shell's room handoff.
    const second = await openInstalledAssistant(secondPage, `${base}/home/`);
    subcheck = "contender_configure";
    await second.locator("#agent-harness-new-chat").waitFor({ state: "visible" });
    await second.locator("#agent-harness-new-chat").click();
    await selectModel(second, offer);
    await configureTokens(second, 16);
    subcheck = "owner_start";
    const owner = await startResourceTurn(assistant, offer, longPrompt, 1024);
    assert(!owner.run.terminal && ["prepared", "running"].includes(owner.run.status));
    await resourceAction("engine_running");
    subcheck = "busy";
    const busy = await startResourceTurn(second, offer, "Say hello.", 16);
    const refused = await busy.terminal;
    assert.equal(refused.status, "failed");
    assert.equal(refused.error?.code, "model_busy");
    await visibleFailure(second, "Model is busy.");
    await secondPage.screenshot({ path: join(evidence, "finite-model-busy.png"), fullPage: true });
    resourceTurns.push({ stage: "busy", run_id: busy.run.run_id, status: refused.status, code: refused.error.code });
    results.model_busy = "passed";
    subcheck = "stop";
    const cancelled = page.waitForResponse(response => {
      if (!response.url().endsWith("/api/provider/model/runs_cancel") || response.request().frame() !== assistant) return false;
      return response.request().postDataJSON()?.run_id === owner.run.run_id;
    }, { timeout: 30000 });
    cancelled.catch(() => {});
    await assistant.locator('#agent-composer-send[data-mode="stop"]').click();
    assert((await cancelled).ok());
    const stopped = await owner.terminal;
    assert.equal(stopped.status, "cancelled", "Stop confirms the exact live run's settlement");
    await resourceAction("engine_stopped");
    resourceTurns.push({ stage: "stop", run_id: owner.run.run_id, status: stopped.status });
    results.model_stop = "passed";
  } finally { await secondPage.close(); }
  subcheck = "stop_recovery";
  await resourceReply(assistant, offer, "stop_recovery");
  results.model_stop_recovery = "passed";
  subcheck = "idle";
  const warm = await resourceAction("engine_running");
  await resourceAction("idle_stopped");
  results.model_idle_release = "passed";
  subcheck = "idle_recovery";
  await resourceReply(assistant, offer, "idle_recovery");
  const restarted = await resourceAction("engine_running");
  assert(warm.engine_pid !== restarted.engine_pid || warm.engine_start_ticks !== restarted.engine_start_ticks);
  results.model_idle_recovery = "passed";
  subcheck = "crash";
  await assistant.locator("#agent-harness-new-chat").click();
  await configureTokens(assistant, 1024);
  const crashing = await startResourceTurn(assistant, offer, longPrompt, 1024);
  assert(!crashing.run.terminal);
  await assistant.waitForFunction(() => /\S/.test([...document.querySelectorAll(".agent-msg-agent .agent-msg-body")].at(-1)?.innerText ?? ""), null, { timeout: 60000 });
  const doomed = await resourceAction("engine_running");
  await resourceAction("crash_engine");
  const failed = await crashing.terminal;
  assert.equal(failed.status, "failed");
  assert.equal(failed.error?.message, "model backend transport was interrupted");
  await visibleFailure(assistant, "Model run failed: model backend transport was interrupted");
  await resourceAction("engine_stopped");
  resourceTurns.push({ stage: "engine_crash", run_id: crashing.run.run_id, status: failed.status, code: failed.error?.code });
  results.model_engine_crash = "passed";
  subcheck = "crash_recovery";
  await resourceReply(assistant, offer, "crash_recovery");
  const recovered = await resourceAction("engine_running");
  assert(doomed.engine_pid !== recovered.engine_pid || doomed.engine_start_ticks !== recovered.engine_start_ticks);
  results.model_crash_recovery = "passed";
  stage = "finite_home_recovery";
  subcheck = null;
  await returnInstalledHome(page, assistant, evidence);
  results.finite_home_recovery = "passed";
}

async function repairJourney(assistant, offer) {
  assert(fixture.model_repair && fixture.carrier_holder && fixture.models.length === 2);
  const other = fixture.models[1];
  assert.notEqual(other.cid, cid);
  assert.notEqual(other.model_sha256, fixture.model_sha256);
  await page.setViewportSize({ width: 1440, height: 900 });
  stage = "model_stop_get_different";
  subcheck = "stop_real_stream";
  await assistant.locator("#agent-harness-new-chat").click();
  await configureTokens(assistant, 1024);
  const owner = await startResourceTurn(assistant, offer,
    "Write a numbered list from 1 to 1000. Each item must say hello and give one detail. Continue the list without an introduction or conclusion.", 1024);
  assert(!owner.run.terminal);
  await resourceAction("engine_running");
  await assistant.waitForFunction(() => /\S/.test([...document.querySelectorAll(".agent-msg-agent .agent-msg-body")].at(-1)?.innerText ?? ""), null, { timeout: 60000 });
  const cancelled = page.waitForResponse(response => response.url().endsWith("/api/provider/model/runs_cancel") &&
    response.request().frame() === assistant && response.request().postDataJSON()?.run_id === owner.run.run_id, { timeout: 30000 });
  cancelled.catch(() => {});
  await assistant.locator('#agent-composer-send[data-mode="stop"]').click();
  assert((await cancelled).ok());
  assert.equal((await owner.terminal).status, "cancelled");
  const closed = await resourceAction("engine_stopped");
  assert(closed.whole_engine_exit && closed.runtime_unchanged);
  modelRepair.stop = { run_id: owner.run.run_id, status: "cancelled", production_guard: true, whole_engine_exit: true };
  await page.screenshot({ path: join(evidence, "repair-stop.png"), fullPage: true });
  await returnInstalledHome(page, assistant, evidence);
  let marketplace = await app("marketplace");
  await showModelDetail(marketplace, other.cid);
  subcheck = "get_other_model";
  await clickModelUse(marketplace, other.cid, "Get");
  const second = await readyModel(marketplace, other.cid);
  assert.notEqual(second.model_runtime.offer_id, offer);
  await page.screenshot({ path: join(evidence, "repair-qwen-ready.png"), fullPage: true });
  const nextAssistant = await openInstalledAssistant(page, `${base}/home/`);
  await nextAssistant.locator("#agent-harness-new-chat").click();
  await selectModel(nextAssistant, second.model_runtime.offer_id, other.cid);
  subcheck = "other_model_reply";
  const replied = await resourceReply(nextAssistant, second.model_runtime.offer_id, "qwen-reply");
  const previous = await nextAssistant.evaluate(async runId => {
    const token = new URLSearchParams(location.hash.slice(1)).get("home_token");
    const response = await fetch("/api/provider/model/runs_get", { method: "POST", headers: {
      "x-elastos-home-token": token, "Content-Type": "application/json",
    }, signal: AbortSignal.timeout(35000), body: JSON.stringify({ run_id: runId, request_id: `model-v1:${Date.now()}:${crypto.randomUUID()}` }) });
    return { ok: response.ok, payload: await response.json() };
  }, owner.run.run_id);
  assert(previous.ok && previous.payload.status === "ok");
  assert.equal(previous.payload.data.run_id, owner.run.run_id);
  assert.equal(previous.payload.data.offer_id, offer);
  assert.equal(previous.payload.data.status, "cancelled");
  assert.equal(previous.payload.data.terminal?.status, "cancelled");
  assert(previous.payload.data.terminal.output == null);
  modelRepair.stop.journal_after_different_reply = "cancelled";
  modelRepair.different_model = { cid: other.cid, model_sha256: other.model_sha256, offer_id: second.model_runtime.offer_id,
    run_id: replied.run.run_id, status: "completed", runtime_restart_before_get: false };
  results.model_stop_get_different = "passed";
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({ path: join(evidence, "repair-qwen-reply-phone.png"), fullPage: true });
  await page.setViewportSize({ width: 1440, height: 900 });
  await returnInstalledHome(page, nextAssistant, evidence);
  marketplace = await app("marketplace");
  stage = "model_interrupted_get_retry";
  subcheck = "fresh_get_precondition";
  // Reset only this isolated model through Runtime's normal scoped reclaim.
  // The two user-facing Get/Retry actions below still use Marketplace controls.
  const reclaimed = await marketplace.evaluate(async modelCid => {
    const token = new URLSearchParams(location.hash.slice(1)).get("home_token");
    const headers = { "x-elastos-home-token": token, "Content-Type": "application/json" };
    const registry = await (await fetch("/api/capsules/interfaces", { headers, signal: AbortSignal.timeout(35000) })).json();
    const entry = registry.interfaces.find(entry => entry.capsule === "marketplace" && entry.interface.methods.some(method => method.id === "content.reclaim"));
    if (!entry) throw new Error("scoped reclaim interface unavailable");
    const response = await fetch("/api/capsules/interfaces/invoke", { method: "POST", headers, signal: AbortSignal.timeout(35000), body: JSON.stringify({
      capsule: "marketplace", interface: entry.interface.id, method: "content.reclaim", request_id: `model-v1:${Date.now()}:${crypto.randomUUID()}`, input: { cid: modelCid },
    }) });
    return { ok: response.ok, status: (await response.json()).status };
  }, other.cid);
  assert(reclaimed.ok && reclaimed.status === "ok");
  await showModelDetail(marketplace, other.cid);
  subcheck = "partial_get";
  await clickModelUse(marketplace, other.cid, "Get");
  await marketplace.waitForFunction(() => {
    const progress = document.querySelector('progress[aria-label="Preparation progress"]');
    return progress && progress.value > 0 && progress.value < progress.max;
  }, null, { timeout: 60000 });
  await page.screenshot({ path: join(evidence, "repair-partial-get.png"), fullPage: true });
  const restarted = await resourceAction("restart_get");
  assert(restarted.home_restarted && restarted.weights_bytes > 0);
  modelRepair.interrupted_get = { kept_weights_bytes: restarted.weights_bytes, kept_weights_prefix_sha256: restarted.weights_prefix_sha256 };
  subcheck = "restarted_home";
  assert((await page.goto(`${base}/home/`, { waitUntil: "domcontentloaded" })).ok());
  await page.waitForFunction(() => document.body.dataset.homeStatus === "ready" || document.querySelector("#home-unlock")?.hidden === false, null, { timeout: 60000 });
  if (await page.locator("#home-unlock").isVisible()) await page.locator("#home-unlock-primary").click();
  await page.waitForFunction(() => document.body.dataset.homeStatus === "ready" && document.body.dataset.homeAuthority === "signed", null, { timeout: 60000 });
  const shell = page.frameLocator("#active-shell-frame");
  await shell.locator("#desktop").waitFor({ state: "visible", timeout: 60000 });
  marketplace = await app("marketplace");
  await showModelDetail(marketplace, other.cid);
  await marketplace.getByText("The download stopped before it finished. Retry.", { exact: true }).waitFor({ state: "visible", timeout: 60000 });
  await page.screenshot({ path: join(evidence, "repair-restart-retry.png"), fullPage: true });
  subcheck = "retry_retained_get";
  const retry = await clickModelUse(marketplace, other.cid, "Retry");
  modelRepair.retry_operation_id = retry.output.operation_id;
  await marketplace.waitForFunction(kept => {
    const progress = document.querySelector('progress[aria-label="Preparation progress"]');
    return progress?.value >= kept || Boolean(document.querySelector('[data-model-control="open-assistant"]'));
  }, restarted.weights_bytes, { timeout: 60000 });
  await page.screenshot({ path: join(evidence, "repair-resumed-get.png"), fullPage: true });
  const final = await readyModel(marketplace, other.cid);
  assert(final.model_runtime.dispatch_ready);
  await page.screenshot({ path: join(evidence, "repair-retry-ready.png"), fullPage: true });
  results.model_interrupted_get_retry = "passed";
}

async function showModelDetail(marketplace, modelCid) {
  const close = marketplace.locator('[data-action="close-detail"]').first();
  if (await close.isVisible()) await close.click();
  await marketplace.locator(`[data-action="model-detail"][data-app="model:${modelCid}"]`).click();
  await marketplace.locator('[data-model-control="refresh"]').click();
}

async function clickModelUse(marketplace, modelCid, label) {
  const use = marketplace.locator('[data-model-control="use"]');
  await use.waitFor({ state: "visible", timeout: 30000 });
  assert.equal((await use.textContent()).trim(), label);
  const response = page.waitForResponse(response => {
    if (!response.url().endsWith("/api/capsules/interfaces/invoke") || response.request().frame() !== marketplace) return false;
    const body = response.request().postDataJSON();
    return body?.capsule === "marketplace" && body?.method === "content.use" && body?.input?.cid === modelCid;
  }, { timeout: 60000 });
  response.catch(() => {});
  await use.click();
  const accepted = await response;
  assert(accepted.ok());
  const value = await accepted.json();
  assert.equal(value.status, "ok");
  return value;
}

async function readyModel(marketplace, modelCid) {
  await marketplace.locator('[data-model-control="open-assistant"]').waitFor({ state: "visible", timeout: readyMs });
  const model = (await projection(marketplace, "/api/capsules/catalog")).capsules.find(row => row.cid === modelCid);
  assert.equal(model?.model_runtime?.dispatch_ready, true);
  return model;
}
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
  browserOpening = chromium.launch({ headless: true, handleSIGTERM: !resourceControl, handleSIGINT: !resourceControl });
  browser = await browserOpening;
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
  if (resourceControl) {
    assert.equal(initialModel?.model_content?.minimum_memory_mb, 512);
    assert.equal(initialModel?.model_memory_fit, "fits", "signed minimum fits; dynamic run admission remains separate");
  }
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
    if (mode === "--memory-refusal") {
      stage = "memory_refusal";
      const refused = await startResourceTurn(assistant, offer, "Say hello in one sentence.", 16);
      const terminal = await refused.terminal;
      assert.equal(terminal.status, "failed");
      assert.equal(terminal.error?.code, "model_memory_unavailable");
      assert.equal(terminal.error?.class, "context_rejected");
      const message = "Not enough free memory for this model. Close other apps or choose a smaller model.";
      assert.equal(terminal.error?.message, message);
      await visibleFailure(assistant, `Model run failed: ${message}`);
      await page.screenshot({ path: join(evidence, "finite-memory-refusal.png"), fullPage: true });
      resourceTurns.push({ stage: "memory_refusal", run_id: refused.run.run_id, status: terminal.status, code: terminal.error.code });
      results.memory_refusal = "passed";
      stage = "finite_home_recovery";
      await returnInstalledHome(page, assistant, evidence);
      results.finite_home_recovery = "passed";
      writeFileSync(join(evidence, "home-journey.json"), JSON.stringify({
        results, disk, model_cid: cid, offer_id: offer, max_output_tokens: 16, resource_turns: resourceTurns,
        journey: "signed Get succeeds; dynamic memory admission refuses Assistant Send; Home stays usable",
      }, null, 2));
    } else {
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
    subcheck = null;
    if (mode === "--resource-lifecycle") await resourceLifecycle(assistant, offer, context);
    if (mode === "--model-repair") await repairJourney(assistant, offer);
    writeFileSync(join(evidence, "home-journey.json"), JSON.stringify({
      results, disk, reply_characters: text.length, model_cid: cid, offer_id: offer,
      max_output_tokens: request.input.max_output_tokens, resource_turns: resourceTurns, ...(mode === "--model-repair" ? { model_repair: modelRepair } : {}),
      journey: mode === "--model-repair" ? "Real reply, Stop, Get a different pinned model and reply; interrupted Get, Home restart and retained-prefix Retry"
        : mode === "--resource-lifecycle" ? "Marketplace Get, reply, Busy, Stop, idle closure and crash recovery" : "Marketplace Get to visible Assistant reply",
    }, null, 2));
    }
  }
} catch (error) {
  journeyFailed = true;
  results[stage] = "failed";
  disk.at_failure = observeDisk();
  const failure = publicJourneyFailure(error, stage, subcheck);
  writeFileSync(join(data, "journey-browser.private.log"), String(error?.stack ?? error), { mode: 0o600 });
  writeFileSync(join(evidence, "home-journey.json"), JSON.stringify({ results, disk, resource_turns: resourceTurns, ...(mode === "--model-repair" ? { model_repair: modelRepair } : {}), ...failure }, null, 2));
  await page?.screenshot({ path: join(evidence, "home-failure.png"), fullPage: true }).catch(() => {});
  throw new Error(`Installed Home journey failed: stage=${failure.failure_stage} code=${failure.failure}${failure.failure_check ? ` check=${failure.failure_check}` : ""}`);
} finally {
  try { await closeBrowser(); }
  catch (error) {
    try { appendFileSync(join(data, "journey-browser.private.log"), `\nBrowser cleanup failed:\n${String(error?.stack ?? error)}`, { mode: 0o600 }); }
    catch { /* The fixed public failure still applies if private logging fails. */ }
    try {
      const path = join(evidence, "home-journey.json");
      const receipt = JSON.parse(readFileSync(path, "utf8"));
      receipt.results.process_cleanup = "failed";
      if (!journeyFailed) Object.assign(receipt, publicJourneyFailure(error, stage, subcheck));
      writeFileSync(path, JSON.stringify(receipt, null, 2));
    } catch { /* A missing receipt cannot satisfy the installed proof. */ }
    if (!journeyFailed) throw new Error("Installed Home journey failed: stage=journey code=journey_failed");
  }
}
