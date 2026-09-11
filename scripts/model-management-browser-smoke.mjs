#!/usr/bin/env node
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, resolve } from "node:path";
import { once } from "node:events";
import { chromium, brave, inertSystemApiResponse, makeSystemSummary, makeAppearanceRecord } from "./system-uiux-fixture.mjs";

const root = resolve(new URL("../", import.meta.url).pathname);
const cid = `bafybei${"a".repeat(52)}`;
const operation = "model-operation-1";
const calls = [];
let phase = "unprepared";
let kept = false;
let activationPending = false;
let dispatchPending = false;
let failure = false;
let delayStatus = null;
let delayUse = null;
let delayRetention = null;
let wrongIdentity = false;
let executable = true;
let selectedCid = cid;
let trust = "verified";
let loseUseResponse = false;
let malformedRuntime = false;
let mismatchRetention = false;
let delayCatalog = null;
const fixtureErrors = [];
function preparation() {
  return phase === "unprepared" ? null : { operation_id: operation, cid: selectedCid, state: phase,
    total_bytes: 1024, completed_bytes: phase === "capacity_pending" ? 0 : ["admitted", "reclaimed"].includes(phase) ? 1024 : 256,
    cancel_requested: false, admitted: phase === "admitted", activation_pending: activationPending };
}
function runtime() {
  return { admitted: phase === "admitted", kept, dispatch_ready: phase === "admitted" && !dispatchPending,
    offer_id: malformedRuntime ? "wrong" : phase === "admitted" && !dispatchPending ? `model:${"b".repeat(64)}` : null, preparation: preparation() };
}
function catalog() {
  return { schema: "elastos.capsules.catalog/v1", model_catalog_state: trust, capsules: [
    { name: "people", title: "People", role: "app", installed: true, launchable: true, interfaces: [], dependencies: [] },
    { name: "fixture-model", title: "Fixture model", role: "content", source: "signed-model-catalog",
      installed: false, launchable: false, cid: selectedCid, publisher_did: "did:key:zFixturePublisher", content_size_bytes: 1024,
      signature_state: "catalog-signature-verified", model_content: { format: "gguf" }, model_runtime: runtime() },
  ] };
}
const manifests = Object.fromEntries(await Promise.all(["marketplace", "system"].map(async name =>
  [name, JSON.parse(await readFile(resolve(root, `capsules/${name}/capsule.json`), "utf8"))])));
const server = createServer(async (req, res) => {
  const url = new URL(req.url, "http://fixture");
  const headers = { "access-control-allow-origin": "null", "access-control-allow-headers": "content-type,x-elastos-home-token", "access-control-allow-methods": "GET,POST,OPTIONS" };
  const json = (value, status = 200) => { res.writeHead(status, { ...headers, "content-type": "application/json" }); res.end(JSON.stringify(value)); };
  try {
    if (req.method === "OPTIONS") { res.writeHead(204, headers); res.end(); return; }
    if (url.pathname === "/") {
      res.setHeader("content-type", "text/html");
      res.end(`<iframe style="border:0;width:100%;height:95vh" sandbox="allow-scripts allow-forms" src="/apps/${url.searchParams.get("app")}/?home_origin=${encodeURIComponent(`http://127.0.0.1:${server.address().port}`)}#home_token=fixture-token"></iframe>`); return;
    }
    if (url.pathname === "/api/capsules/catalog") {
      const value = catalog();
      if (delayCatalog) { delayCatalog(() => json(value)); return; }
      json(value); return;
    }
    if (url.pathname === "/api/capsules/interfaces") {
      json({ interfaces: Object.entries(manifests).flatMap(([capsule, m]) => m.interfaces.map(i => ({ capsule, interface: i,
        bindings: i.methods.map(method => ({ method: method.id, executable })) }))) }); return;
    }
    if (url.pathname === "/api/capsules/interfaces/invoke") {
      let body = ""; for await (const chunk of req) body += chunk;
      const input = JSON.parse(body); calls.push(input);
      assert.equal(req.headers["x-elastos-home-token"], "fixture-token");
      assert.deepEqual(Object.keys(input).sort(), ["capsule", "input", "interface", "method", "request_id"]);
      const iface = manifests[input.capsule]?.interfaces.find(i => i.id === input.interface);
      const method = iface?.methods.find(m => m.id === input.method);
      assert.ok(method, "real capsule must declare invoked method");
      assert.equal(method.resource, "elastos://capsules/*");
      assert.equal(method.approval, "runtime_policy");
      assert.equal(method.operation, input.method.split(".")[1]);
      assert.ok(["use", "status", "cancel", "retention"].includes(method.operation));
      assert.equal(method.risk, method.operation === "status" ? "read" : "write");
      if (failure) { json({ error: "private runtime provider path /secret" }, 503); return; }
      if (method.operation === "use") { assert.deepEqual(input.input, { cid }); phase = "preparing"; }
      if (method.operation === "use" && loseUseResponse) { req.socket.destroy(); return; }
      if (["status", "cancel"].includes(method.operation)) assert.deepEqual(input.input, { operation_id: operation });
      if (method.operation === "cancel") { phase = "cancelled"; kept = false; }
      if (method.operation === "retention") { assert.deepEqual(input.input, { cid, keep: !kept }); kept = input.input.keep; }
      const readiness = runtime();
      const facts = { admitted: readiness.admitted, kept: readiness.kept, dispatch_ready: readiness.dispatch_ready, offer_id: readiness.offer_id };
      const output = method.operation === "retention" ? { cid, ...facts } : { ...preparation(), ...facts };
      if (method.operation === "retention" && mismatchRetention) {
        output.kept = !input.input.keep;
        output.dispatch_ready = false; output.offer_id = null;
      }
      const result = { schema: "elastos.capsules.invoke-result/v1", status: "ok", capsule: input.capsule, interface: input.interface, method: input.method,
        request_id: wrongIdentity ? "other-request" : input.request_id, request_binding: {}, output };
      const reply = () => json(result);
      if (method.operation === "status" && delayStatus) { delayStatus(reply); return; }
      if (method.operation === "use" && delayUse) { delayUse(reply); return; }
      if (method.operation === "retention" && delayRetention) { delayRetention(reply, output); return; }
      reply(); return;
    }
    if (["/api/apps/home/summary", "/api/apps/system/summary"].includes(url.pathname)) { json(makeSystemSummary(makeAppearanceRecord())); return; }
    if (url.pathname === "/api/provider/object/list_runtime_custody") { json({ schema: "elastos.library.runtime-custody-listings/v1", listings: [], truncated: false }); return; }
    if (url.pathname.startsWith("/api/")) { json(inertSystemApiResponse(url.pathname) || {}); return; }
    // The fixture serves each capsule's real browser tree, including vendored helpers.
    const match = url.pathname.match(/^\/apps\/([^/]+)\/(.*)$/);
    if (!match) { res.writeHead(404); res.end(); return; }
    const asset = resolve(root, "capsules", match[1], "browser", match[2] || "index.html");
    assert.ok(asset.startsWith(`${root}/capsules/`));
    const bytes = await readFile(asset);
    res.writeHead(200, { ...headers, "content-type": ({ ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript", ".css": "text/css" })[extname(asset)] || "application/octet-stream" }); res.end(bytes);
  } catch (error) { fixtureErrors.push(String(error)); json({ fixture_error: String(error) }, 500); }
});
server.listen(0, "127.0.0.1"); await once(server, "listening");
const browser = await chromium.launch({ executablePath: brave, headless: true });
let activePage;
try {
  for (const app of ["marketplace", "system"]) {
    phase = "unprepared"; kept = false; failure = false; calls.length = 0;
    selectedCid = cid; executable = true; trust = "verified";
    const page = await browser.newPage({ viewport: { width: 1100, height: 800 } });
    activePage = page;
    const pageErrors = [];
    page.on("pageerror", error => pageErrors.push(String(error)));
    await page.goto(`http://127.0.0.1:${server.address().port}/?app=${app}`);
    const frame = page.frameLocator("iframe");
    if (app === "marketplace") {
      for (const title of ["Installed", "Apps", "Models"]) {
        await frame.locator("#store-sections").getByRole("heading", { name: title, exact: true }).waitFor();
      }
      for (const title of ["Apps", "Installed"]) {
        await frame.locator(`[data-destination="${title.toLowerCase()}"]`).first().click();
        await frame.locator("#store-sections").getByText("People", { exact: true }).waitFor();
        assert.equal(await frame.getByRole("heading", { name: title, exact: true }).count(), 1, `${title} has one page title`);
        assert.equal(await frame.locator("#store-sections .store-section-head").count(), 0, `${title} omits the repeated section heading`);
      }
    }
    await frame.getByRole("button", { name: "Models", exact: true }).click({ timeout: 3000 });
    if (app === "marketplace") {
      await frame.getByRole("button", { name: "Details", exact: true }).waitFor();
      assert.equal(await frame.getByRole("heading", { name: "Models", exact: true }).count(), 1, "Models has one page title");
      assert.equal(await frame.locator("#store-sections .store-section-head").count(), 0, "Models omits the repeated section heading");
      await frame.getByRole("button", { name: "Details", exact: true }).click();
    }
    await (app === "marketplace" ? frame.locator(".modal-title") : frame.getByRole("heading", { name: "Fixture model", exact: true })).waitFor();
    assert.match(await frame.locator("body").textContent(), /did:key:zFixturePublisher/);
    phase = "capacity_pending";
    await frame.getByRole("button", { name: "Refresh models" }).click();
    await frame.getByText("Waiting for local capacity…", { exact: true }).waitFor();
    assert.equal(await frame.getByRole("button", { name: "Cancel preparation" }).count(), 1);
    assert.equal(calls.filter(c => c.method === "content.use").length, 0, "pending status never starts another Use");
    phase = "reclaimed";
    await frame.getByRole("button", { name: "Refresh models" }).click();
    await frame.getByText("Model removed from local cache.", { exact: true }).waitFor();
    assert.equal(await frame.getByRole("button", { name: "Use", exact: true }).isEnabled(), true);
    assert.equal(await frame.getByRole("checkbox", { name: "Keep on this device" }).isEnabled(), false);
    phase = "unprepared";
    await frame.getByRole("button", { name: "Refresh models" }).click();
    await frame.getByText("Ready to prepare", { exact: true }).waitFor();
    let releaseUse;
    const useReceived = new Promise(resolveUse => { delayUse = reply => { releaseUse = reply; resolveUse(); }; });
    await frame.getByRole("button", { name: "Use", exact: true }).evaluate(button => { button.click(); button.click(); });
    await useReceived;
    assert.equal(calls.filter(c => c.method === "content.use").length, 1, "double-click dispatches once");
    delayUse = null; releaseUse();
    await frame.getByRole("button", { name: "Cancel preparation" }).waitFor();
    assert.equal(calls.filter(c => c.method === "content.use").length, 1);
    await frame.getByRole("checkbox", { name: "Keep on this device" }).check();
    await frame.getByText("Keep choice saved for this preparation.", { exact: true }).waitFor();
    await frame.getByRole("button", { name: "Refresh models" }).click();
    await frame.locator('[data-model-management] input:enabled:checked').waitFor();
    assert.equal(calls.filter(c => c.method === "content.use").length, 1, "retention never starts another preparation");
    await frame.getByRole("button", { name: "Cancel preparation" }).click();
    await frame.getByRole("button", { name: "Retry", exact: true }).waitFor();
    assert.equal(await frame.getByRole("checkbox", { name: "Keep on this device" }).isChecked(), false, "cancelled attempt releases pending choice");
    await frame.getByRole("button", { name: "Retry", exact: true }).click();
    await frame.getByRole("button", { name: "Cancel preparation" }).waitFor();
    phase = "admitted"; activationPending = app === "marketplace"; dispatchPending = true;
    await frame.getByText("Available on this device. Model service unavailable.", { exact: true }).waitFor();
    const activationUses = calls.filter(c => c.method === "content.use").length;
    assert.equal(await frame.getByRole("button", { name: "Cancel preparation" }).count(), 0, "admitted content is not cancellable");
    activationPending = false; dispatchPending = false;
    await frame.locator("[data-model-management] p").filter({ hasText: /^Available on this device$/ }).waitFor();
    assert.equal(calls.filter(c => c.method === "content.use").length, activationUses, "activation readiness uses status without another Use");
    const readyStatuses = calls.filter(c => c.method === "content.status").length;
    await page.waitForTimeout(1750);
    assert.equal(calls.filter(c => c.method === "content.status").length, readyStatuses, "ready state stops polling");
    await frame.getByRole("checkbox", { name: "Keep on this device" }).check();
    await frame.locator('[data-model-management] input:enabled:checked').waitFor();
    await frame.getByRole("checkbox", { name: "Keep on this device" }).uncheck();
    await frame.locator('[data-model-management] input:enabled:not(:checked)').waitFor();
    phase = "preparing";
    await frame.getByRole("button", { name: "Refresh models" }).click();
    await frame.getByRole("button", { name: "Cancel preparation" }).waitFor();
    const usesBeforeRetentionRace = calls.filter(c => c.method === "content.use").length;
    const statusesBeforeRetentionRace = calls.filter(c => c.method === "content.status").length;
    let releaseRetention;
    const retentionReceived = new Promise(resolveRetention => {
      delayRetention = (reply, output) => { releaseRetention = reply; resolveRetention(output); };
    });
    // Click does not wait for the checked state while the test holds the ack.
    await frame.getByRole("checkbox", { name: "Keep on this device" }).click();
    const pendingRetentionAck = await retentionReceived;
    assert.equal(pendingRetentionAck.admitted, false, "retention acknowledgement captures the pending preparation");
    assert.equal(pendingRetentionAck.kept, true);
    phase = "admitted";
    delayRetention = null; releaseRetention();
    await frame.locator("[data-model-management] p").filter({ hasText: /^Available on this device$/ }).waitFor();
    await frame.locator('[data-model-management] input:enabled:checked').waitFor();
    assert.equal(await frame.getByRole("button", { name: "Cancel preparation" }).count(), 0, "fresh admission replaces the pending preparation after retention settles");
    assert.equal(calls.filter(c => c.method === "content.use").length, usesBeforeRetentionRace, "admission during retention does not dispatch another Use");
    assert.equal(calls.filter(c => c.method === "content.status").length, statusesBeforeRetentionRace, "retention reads fresh admission before another status poll can hide stale state");
    await frame.getByRole("checkbox", { name: "Keep on this device" }).uncheck();
    await frame.locator('[data-model-management] input:enabled:not(:checked)').waitFor();
    failure = true;
    await frame.getByRole("checkbox", { name: "Keep on this device" }).click();
    await frame.getByText("Model action could not be confirmed. Refresh to check its status.", { exact: true }).waitFor();
    assert.doesNotMatch(await frame.locator("body").innerText(), /\/secret|private runtime provider/);
    failure = false;
    await frame.getByRole("button", { name: "Refresh models" }).click();
    await frame.locator('[data-model-management] input:enabled').waitFor();
    mismatchRetention = true;
    await frame.getByRole("checkbox", { name: "Keep on this device" }).click();
    await frame.getByText("Model action could not be confirmed. Refresh to check its status.", { exact: true }).waitFor();
    assert.equal(await frame.getByRole("checkbox", { name: "Keep on this device" }).isChecked(), false, "invalid retention reply leaves prior state intact");
    assert.equal(await frame.getByText("Current status unavailable", { exact: true }).count(), 1);
    mismatchRetention = false;
    await frame.getByRole("button", { name: "Refresh models" }).click();
    await frame.locator('[data-model-management] input:enabled:checked').waitFor();
    assert.equal(await frame.getByRole("button", { name: "Refresh models" }).evaluate(n => n === document.activeElement), true, "refresh retains keyboard focus");
    const bounds = async () => {
      assert.equal(await frame.locator("html").evaluate(n => n.scrollWidth <= n.clientWidth + 1), true, `${app} page clips`);
      assert.equal(await frame.locator("[data-model-management]").evaluate(n => n.scrollWidth <= n.clientWidth + 1), true, `${app} model view clips`);
    };
    await bounds();
    if (process.env.MODEL_UI_SCREENSHOTS) await page.screenshot({ path: `${process.env.MODEL_UI_SCREENSHOTS}/${app}-models-desktop.png` });
    await page.setViewportSize({ width: 390, height: 740 });
    await bounds();
    await frame.getByRole("checkbox", { name: "Keep on this device" }).scrollIntoViewIfNeeded();
    if (process.env.MODEL_UI_SCREENSHOTS) await page.screenshot({ path: `${process.env.MODEL_UI_SCREENSHOTS}/${app}-models-narrow.png` });

    await frame.locator("html").evaluate(node => node.dataset.elTheme = "light");
    await bounds();
    if (process.env.MODEL_UI_SCREENSHOTS) await page.screenshot({ path: `${process.env.MODEL_UI_SCREENSHOTS}/${app}-models-light.png` });
    await frame.locator("html").evaluate(node => node.dataset.elTheme = "dark");
    await page.setViewportSize({ width: 1100, height: 800 });
    phase = "unprepared"; kept = false;
    await frame.getByRole("button", { name: "Refresh models" }).click();
    await frame.getByRole("button", { name: "Use", exact: true }).waitFor();
    let releaseCatalog;
    const catalogReceived = new Promise(resolveCatalog => { delayCatalog = reply => { releaseCatalog = reply; resolveCatalog(); }; });
    const beforeRefreshUses = calls.filter(c => c.method === "content.use").length;
    await frame.getByRole("button", { name: "Refresh models" }).click();
    await catalogReceived;
    assert.equal(await frame.getByRole("button", { name: "Use", exact: true }).isDisabled(), true);
    await frame.getByRole("button", { name: "Use", exact: true }).evaluate(n => n.click());
    assert.equal(calls.filter(c => c.method === "content.use").length, beforeRefreshUses, "catalog refresh blocks old-model mutation");
    delayCatalog = null; releaseCatalog();
    await frame.locator('[data-model-control="use"]:enabled').waitFor();
    loseUseResponse = true;
    const usesBeforeLoss = calls.filter(c => c.method === "content.use").length;
    await frame.getByRole("button", { name: "Use", exact: true }).click();
    await frame.getByText("Model action could not be confirmed. Refresh to check its status.", { exact: true }).waitFor();
    assert.equal(await frame.getByRole("button", { name: "Retry", exact: true }).isDisabled(), true);
    loseUseResponse = false;
    await frame.getByRole("button", { name: "Refresh models" }).click();
    await frame.getByRole("button", { name: "Cancel preparation" }).waitFor();
    const lostRequests = calls.filter(c => c.method === "content.use").slice(usesBeforeLoss);
    assert.ok(lostRequests.length > 0);
    assert.equal(new Set(lostRequests.map(c => c.request_id)).size, 1, "transport retries and reconciliation preserve the same Use identity");
    phase = "unprepared"; kept = false;
    await frame.getByRole("button", { name: "Refresh models" }).click();
    await frame.getByRole("button", { name: "Use", exact: true }).waitFor();
    wrongIdentity = true;
    await frame.getByRole("button", { name: "Use", exact: true }).click();
    await frame.getByText("Model action could not be confirmed. Refresh to check its status.", { exact: true }).waitFor();
    assert.equal(await frame.getByRole("button", { name: "Cancel preparation" }).count(), 0, "mismatched response cannot install an operation");
    wrongIdentity = false;
    executable = false;
    await frame.getByRole("button", { name: "Refresh models" }).click();
    await frame.getByText("Models are unavailable. Check your connection and trusted catalog, then retry.", { exact: true }).waitFor();
    assert.equal(await frame.getByRole("button", { name: "Use", exact: true }).count(), 0);
    executable = true; trust = "unavailable";
    await frame.getByRole("button", { name: "Refresh models" }).click();
    await frame.getByText("Models are unavailable. Check your connection and trusted catalog, then retry.", { exact: true }).waitFor();
    trust = "verified"; malformedRuntime = true;
    await frame.getByRole("button", { name: "Refresh models" }).click();
    await frame.getByText("Models are unavailable. Check your connection and trusted catalog, then retry.", { exact: true }).waitFor();
    malformedRuntime = false; phase = "preparing";
    await frame.locator("body").evaluate(() => {
      const nativeFetch = window.fetch;
      let delivered;
      window.modelStatusDelivered = new Promise(resolveDelivery => { delivered = resolveDelivery; });
      // Let one real status reply survive abort, proving identity guards after delivery.
      window.fetch = async (url, options) => {
        if (url !== "/api/capsules/interfaces/invoke" || JSON.parse(options?.body || "{}").method !== "content.status") {
          return nativeFetch(url, options);
        }
        window.fetch = nativeFetch;
        const response = await nativeFetch(url, { ...options, signal: undefined });
        const readText = response.text.bind(response);
        response.text = async () => {
          const text = await readText();
          // A task runs after the request/invoke/poll continuation microtasks settle.
          setTimeout(() => delivered(JSON.parse(text).output), 0);
          return text;
        };
        return response;
      };
    });
    let releaseStatus;
    const statusReceived = new Promise(resolveStatus => { delayStatus = reply => { releaseStatus = reply; resolveStatus(); }; });
    await frame.getByRole("button", { name: "Refresh models" }).click();
    await statusReceived;
    await frame.locator("body").evaluate(() => {
      Object.defineProperty(document, "hidden", { configurable: true, value: true });
      document.dispatchEvent(new Event("visibilitychange"));
    });
    const hiddenCount = calls.filter(c => c.method === "content.status").length;
    await new Promise(resolveTick => setTimeout(resolveTick, 1750));
    assert.equal(calls.filter(c => c.method === "content.status").length, hiddenCount, "hidden document stops polling");
    const leave = async () => {
      if (app === "marketplace") await frame.getByRole("button", { name: "Close", exact: true }).first().click();
      await frame.getByRole("button", { name: app === "marketplace" ? "Discover" : "About", exact: true }).click();
    };
    await leave();
    const statusCount = calls.filter(c => c.method === "content.status").length;
    await new Promise(resolveTick => setTimeout(resolveTick, 1750));
    assert.equal(calls.filter(c => c.method === "content.status").length, statusCount, "hidden surface stops polling");
    if (app === "marketplace") {
      await frame.getByText("People", { exact: true }).first().waitFor();
      await frame.getByRole("button", { name: "Media", exact: true }).click();
      assert.match(await frame.locator("#store-sections").innerText(), /No|media/i);
    }
    selectedCid = `bafybei${"b".repeat(52)}`; phase = "unprepared";
    await frame.locator("body").evaluate(() => { delete document.hidden; document.dispatchEvent(new Event("visibilitychange")); });
    if (app === "marketplace") {
      const refreshed = page.waitForResponse(r => new URL(r.url()).pathname === "/api/capsules/catalog");
      await frame.locator("body").evaluate(() => window.dispatchEvent(new MessageEvent("message", {
        origin: "null", source: window.parent, data: { type: "elastos:menu-command", cmd: "refresh" },
      })));
      await refreshed;
      await frame.getByRole("button", { name: "Models", exact: true }).click();
      await frame.locator(`[data-app="model:${selectedCid}"]`).first().waitFor({ state: "attached" });
    }
    await frame.getByRole("button", { name: "Models", exact: true }).click();
    if (app === "marketplace") await frame.getByRole("button", { name: "Details", exact: true }).click();
    if (app === "system") await frame.getByText("Content details", { exact: true }).click();
    if (app === "marketplace") await frame.getByText("Technical details", { exact: true }).click();
    await frame.locator(app === "marketplace" ? ".model-content-identity" : ".model-identity").filter({ hasText: selectedCid }).waitFor();
    delayStatus = null; releaseStatus();
    const deliveredStatus = await frame.locator("body").evaluate(() => window.modelStatusDelivered);
    assert.equal(deliveredStatus.cid, cid, "old status actually reaches the consumer after the CID switch");
    assert.equal(deliveredStatus.operation_id, operation);
    assert.equal(await frame.getByRole("button", { name: "Cancel preparation" }).count(), 0, "late old operation cannot replace new identity");
    assert.match(await frame.locator(app === "marketplace" ? ".model-content-identity" : ".model-identity").innerText(), new RegExp(selectedCid));
    if (app === "marketplace") {
      await frame.getByRole("button", { name: "Use", exact: true }).waitFor();
      const detailCid = selectedCid;
      await frame.locator("[data-model-management]").evaluate(node => {
        window.fixtureOpenModelDetail = node;
        window.fixturePreviousModelUse = node.querySelector('[data-model-control="use"]');
      });
      const mutationsBeforeRotation = calls.filter(c => c.method !== "content.status").length;
      selectedCid = `bafybei${"c".repeat(52)}`;
      await frame.getByRole("button", { name: "Refresh models" }).click();
      await frame.getByText("No verified model is available. Ask your administrator to configure a trusted catalog.", { exact: true }).waitFor();
      assert.equal(await frame.locator("[data-model-management]").evaluate(node => node === window.fixtureOpenModelDetail), true, "CID rotation leaves the same detail controller mounted");
      assert.match(await frame.locator(".model-content-identity").innerText(), new RegExp(detailCid), "the open detail keeps its original CID");
      assert.equal(await frame.locator('[data-model-control="use"], [data-model-control="keep"], [data-model-control="cancel"]').count(), 0, "the previous detail exposes no actions for the replacement CID");
      const staleMutationAttempts = await frame.locator("body").evaluate(() => {
        const originalFetch = window.fetch;
        let attempts = 0;
        window.fetch = (...args) => {
          if (args[0] === "/api/capsules/interfaces/invoke") attempts++;
          return originalFetch(...args);
        };
        try { window.fixturePreviousModelUse.click(); } finally { window.fetch = originalFetch; }
        return attempts;
      });
      assert.equal(staleMutationAttempts, 0, "even a detached previous Use control cannot dispatch after CID rotation");
      assert.equal(calls.filter(c => c.method !== "content.status").length, mutationsBeforeRotation, "CID rotation performs no model mutation");
    }
    assert.deepEqual(pageErrors, []);
    await page.close();
  }
  assert.deepEqual(fixtureErrors, []);
  console.log("PASS Marketplace/System model management browser smoke");
} catch (error) {
  if (process.env.MODEL_UI_SCREENSHOTS && activePage && !activePage.isClosed()) {
    await activePage.screenshot({ path: `${process.env.MODEL_UI_SCREENSHOTS}/failure.png` });
    console.error((await activePage.locator("iframe").contentFrame().locator("body").innerText()).slice(0, 8000));
  }
  throw error;
} finally { await browser.close(); server.closeAllConnections(); await new Promise(resolveClose => server.close(resolveClose)); }
