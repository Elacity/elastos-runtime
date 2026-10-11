#!/usr/bin/env node
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readFileSync, statfsSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { applyRunEventsPage, terminalOutputText } from "../capsules/assistant/browser/model-contract.js";
import { publicJourneyFailure } from "./ci-installed-home-failure.mjs";
const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");
const [base, evidence, data, mode, restoreBase] = process.argv.slice(2);
assert(!mode || mode === "--home-only", "supported installed journey mode required");
// The Home-only journey also restores its Recovery Kit on a second fresh Home.
assert.equal(Boolean(restoreBase), mode === "--home-only", "restore Home required exactly for the Home-only journey");
const PROFILE_NAME = "CI journey";
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
const signedHome = () => document.body.dataset.homeStatus === "ready" && document.body.dataset.homeAuthority === "signed";
async function virtualPasskey(target) {
  const cdp = await target.context().newCDPSession(target);
  await cdp.send("WebAuthn.enable");
  const { authenticatorId } = await cdp.send("WebAuthn.addVirtualAuthenticator", { options: {
    protocol: "ctap2", transport: "internal", hasResidentKey: true,
    hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true,
  } });
  return { cdp, authenticatorId };
}
// The same passkey on another device. A passkey's counter only moves forward,
// so the copy continues from the source's count.
async function copyPasskey(from, to) {
  const { credentials } = await from.cdp.send("WebAuthn.getCredentials", { authenticatorId: from.authenticatorId });
  assert.equal(credentials.length, 1, "one Home passkey");
  await to.cdp.send("WebAuthn.clearCredentials", { authenticatorId: to.authenticatorId });
  for (const credential of credentials) {
    await to.cdp.send("WebAuthn.addCredential", { authenticatorId: to.authenticatorId,
      credential: { ...credential, signCount: credential.signCount + 1 } });
  }
}
async function signInWithPasskey(target) {
  const unlock = target.locator("#home-unlock-person:visible, #home-unlock-primary:visible").first();
  await unlock.waitFor({ state: "visible", timeout: 60000 });
  const verified = target.waitForResponse(response => new URL(response.url()).pathname === "/api/auth/passkey/authenticate/complete", { timeout: 60000 });
  await unlock.click();
  assert((await verified).ok(), "passkey sign-in");
  await target.waitForFunction(signedHome, null, { timeout: 60000 });
}
// Reads Home the way Home does: renew the browser's sign-in, then the summary.
async function homeSession(target) {
  return target.evaluate(async () => {
    const refreshed = await fetch("/api/auth/sessions/refresh", { method: "POST", credentials: "same-origin", signal: AbortSignal.timeout(30000) });
    if (!refreshed.ok) return { status: refreshed.status };
    const token = (await refreshed.json()).home_token;
    const response = await fetch("/api/apps/home/summary", { headers: { "x-elastos-home-token": token }, signal: AbortSignal.timeout(30000) });
    return { status: response.status, summary: response.ok ? await response.json() : null };
  });
}
async function activeSystem(shell) {
  const iframe = shell.locator('section.window.window-active[data-target="system"] iframe.window-frame');
  await iframe.waitFor({ state: "visible", timeout: 30000 });
  const frame = await (await iframe.elementHandle()).contentFrame();
  await frame.waitForURL(url => url.pathname.startsWith("/apps/system/"));
  return frame;
}
async function openSystem(shell) {
  await shell.locator("#toolbar-home").click();
  await shell.locator("#identity-menu-system").click();
  return activeSystem(shell);
}
// Fresh install to finished setup: Profile and Recovery Kit, sign in again,
// manage signed-in devices, then restore the Kit on a second fresh Home.
async function accountJourney(passkey, shell) {
  stage = "recovery_kit";
  const sheet = shell.locator("#setup-sheet");
  await sheet.waitFor({ state: "visible", timeout: 30000 });
  const act = shell.locator("#setup-sheet-recovery");
  const profilePending = (await act.textContent()).trim() === "Finish setup";
  const downloaded = page.waitForEvent("download", { timeout: 120000 });
  downloaded.catch(() => {});
  await act.click();
  if (profilePending) {
    const system = await activeSystem(shell);
    await system.locator("#recovery-profile-name").fill(PROFILE_NAME);
    await system.locator("#recovery-download").click();
  }
  const download = await downloaded;
  // The Kit holds secrets: keep it in memory only and delete the browser copy.
  const kit = readFileSync(await download.path());
  await download.delete();
  const bundle = JSON.parse(kit);
  assert.equal(bundle.schema, "elastos.full-recovery-bundle/v1");
  const profileDid = bundle.people_identity?.profile_authority_bundle?.signed_profile?.payload?.profile_did;
  assert.match(profileDid ?? "", /^did:key:z/, "Recovery Kit carries the signed Profile");
  await sheet.waitFor({ state: "hidden", timeout: 30000 });
  const saved = await homeSession(page);
  assert.equal(saved.summary?.identity?.profile_readiness?.status, "ready");
  assert.equal(saved.summary?.identity?.recovery_readiness?.status, "ready");
  results.recovery_kit = "passed";

  stage = "sign_in_again";
  await shell.locator("#toolbar-home").click();
  await shell.locator("#identity-menu-sign-out").click();
  await signInWithPasskey(page);
  await shell.locator("#desktop").waitFor({ state: "visible", timeout: 30000 });
  const again = await homeSession(page);
  assert.equal(again.summary?.authority?.signed_in, true);
  assert.equal(again.summary?.identity?.profile?.display_name, PROFILE_NAME);
  assert.equal(again.summary?.identity?.recovery_readiness?.status, "ready");
  assert.equal(await sheet.isVisible(), false, "finished setup stays finished after signing in again");
  results.sign_in_again = "passed";

  stage = "signed_in_devices";
  const phone = await browser.newContext({ viewport: { width: 390, height: 844 } });
  try {
    const phonePage = await phone.newPage();
    const phonePasskey = await virtualPasskey(phonePage);
    await copyPasskey(passkey, phonePasskey);
    assert((await phonePage.goto(`${base}/home/`, { waitUntil: "domcontentloaded" })).ok());
    await signInWithPasskey(phonePage);
    const system = await openSystem(shell);
    const rows = system.locator("#session-list .session-row");
    await rows.nth(1).waitFor({ timeout: 30000 });
    assert.equal(await rows.count(), 2, "this computer and the phone");
    assert.equal(await system.locator('#session-list .session-row[data-current="true"]').count(), 1);
    assert.match(await rows.first().textContent(), /Chrome on /, "each sign-in names its browser");
    await page.setViewportSize({ width: 390, height: 844 });
    const signOut = system.locator("#session-list .session-revoke");
    const others = system.locator("#session-revoke-others");
    for (const target of [signOut, others]) {
      await target.scrollIntoViewIfNeeded();
      const box = await target.boundingBox();
      assert(box && box.width >= 44 && box.height >= 44, "phone-sized controls are 44 px targets");
    }
    await page.screenshot({ path: join(evidence, "signed-in-devices-phone.png"), fullPage: true });
    await page.setViewportSize({ width: 1440, height: 900 });
    await signOut.click();
    await rows.nth(1).waitFor({ state: "detached", timeout: 30000 });
    assert.notEqual((await homeSession(phonePage)).status, 200, "a signed-out device stops working at once");
    await phonePage.reload({ waitUntil: "domcontentloaded" });
    await signInWithPasskey(phonePage);
    await system.locator('.settings-sidebar-item[data-settings="account"]').click();
    await rows.nth(1).waitFor({ timeout: 30000 });
    await others.click();
    await rows.nth(1).waitFor({ state: "detached", timeout: 30000 });
    assert.equal(await others.isVisible(), false);
    assert.notEqual((await homeSession(phonePage)).status, 200, "Sign out everywhere else ends the phone sign-in");
    assert.equal((await homeSession(page)).status, 200, "this computer stays signed in");
    await copyPasskey(phonePasskey, passkey);
  } finally {
    await phone.close();
  }
  results.signed_in_devices = "passed";

  stage = "restore_on_new_computer";
  const second = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  try {
    const restored = await second.newPage();
    await virtualPasskey(restored);
    assert((await restored.goto(`${restoreBase}/home/`, { waitUntil: "domcontentloaded" })).ok(), "second Home responds");
    await restored.locator("#home-enrollment-recover").check({ timeout: 60000 });
    await restored.locator("#home-recovery-file").setInputFiles({ name: "recovery-kit.json", mimeType: "application/json", buffer: kit });
    const primary = restored.locator("#home-unlock-primary");
    await restored.waitForFunction(() => document.querySelector("#home-unlock-primary")?.textContent.trim() === "Continue with passkey", null, { timeout: 30000 });
    const imported = restored.waitForResponse(response => new URL(response.url()).pathname === "/api/auth/recovery/full-import", { timeout: 120000 });
    await primary.click();
    const response = await imported;
    assert(response.ok(), "Recovery Kit import");
    const outcome = await response.json();
    assert.equal(outcome.people_identity_restore?.status, "restored");
    assert.equal(outcome.people_identity_restore?.profile_did, profileDid, "the second Home has the same Profile DID");
    await restored.waitForFunction(signedHome, null, { timeout: 60000 });
    const summary = (await homeSession(restored)).summary;
    assert.equal(summary?.identity?.profile?.display_name, PROFILE_NAME);
    assert.equal(summary?.identity?.profile_readiness?.status, "ready");
    await restored.screenshot({ path: join(evidence, "restored-home.png"), fullPage: true });
  } finally {
    kit.fill(0);
    await second.close();
  }
  results.restore_on_new_computer = "passed";
}
try {
  browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  page = await context.newPage();
  const passkey = await virtualPasskey(page);
  stage = "sign_in";
  assert((await page.goto(`${base}/home/`, { waitUntil: "domcontentloaded" })).ok(), "installed Home frontdoor responds successfully");
  await page.locator("#home-unlock-name").waitFor({ state: "visible", timeout: 60000 });
  await page.locator("#home-unlock-name").fill(PROFILE_NAME);
  const completed = page.waitForResponse(response => response.url().endsWith("/api/auth/passkey/register/complete"));
  await page.locator("#home-unlock-primary").click();
  assert((await completed).ok(), "new isolated Home passkey registration");
  await page.waitForFunction(signedHome, null, { timeout: 60000 });
  const shell = page.frameLocator("#active-shell-frame");
  await shell.locator("#desktop").waitFor({ state: "visible", timeout: 30000 });
  if (mode === "--home-only") {
    await accountJourney(passkey, shell);
  } else {
    const setupReminder = shell.locator("#setup-sheet");
    if (await setupReminder.isVisible()) {
      await shell.locator("#setup-sheet-close").click();
      await setupReminder.waitFor({ state: "hidden", timeout: 5000 });
    }
  }
  stage = "journey";
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
