#!/usr/bin/env node
import assert from "node:assert/strict";
import { mkdirSync, readFileSync } from "node:fs";
import vm from "node:vm";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

const source = readFileSync(new URL("../capsules/home/browser/shell-auth.js", import.meta.url), "utf8")
  .replace(/^import\s*\{[\s\S]*?\}\s*from\s*"[^"]+";\n/, "")
  .replace(/export /g, "");
const key = "elastos.home.pending-registration/v1";
const grant = { schema: "elastos.auth.passkey.verify/v2", principal_id: "owner",
  session_id: "one-session", home_token: "one-grant" };
const tick = () => new Promise(resolve => setImmediate(resolve));

function fixture({ guest = false, purpose = "create", terminalLoss = false } = {}) {
  const values = new Map();
  const requests = [];
  const writes = [];
  let verified = false, acknowledged = false, creates = 0, gets = 0, now = 1000000;
  let writeFailure = false, readFailure = false, removeFailure = false;
  let beforeBegin = () => {}, afterBegin = () => {}, afterCreate = () => {};
  let responseMode = "lost";
  let ownerClaimed = false;
  const intent = purpose === "create" ? { purpose, public_name: "Owner" } : { purpose };
  const storage = {
    getItem(name) { if (readFailure) throw Error("storage denied"); return values.get(name) ?? null; },
    setItem(name, value) { if (writeFailure) throw Error("storage full"); values.set(name, String(value)); writes.push(String(value)); },
    removeItem(name) { if (removeFailure) throw Error("storage denied"); values.delete(name); },
  };
  function reload() {
    const elements = new Map();
    const element = id => {
      if (!elements.has(id)) {
        const listeners = new Map();
        elements.set(id, {
          value: "", hidden: false, checked: false, dataset: {}, textContent: "", disabled: false,
          style: { removeProperty() {}, setProperty() {} },
          classList: { add() {}, remove() {} }, setAttribute() {},
          addEventListener(type, callback) { listeners.set(type, callback); },
          emit(type) { if (!this.disabled) listeners.get(type)?.({ target: this }); },
          focus() { this.focused = true; },
        });
      }
      return elements.get(id);
    };
    let opened = null;
    const context = vm.createContext({
      document: { querySelector: element, getElementById: id => element("#" + id), body: element("body") },
      window: { PublicKeyCredential: {}, location: { protocol: "https:" }, sessionStorage: storage, atob, btoa,
        clearTimeout() {}, setTimeout() {}, setInterval() { return 1; }, clearInterval() {} },
      navigator: { credentials: { async create(options) {
        creates++;
        assert.equal(options.publicKey.user.name, purpose === "create" ? "Owner" : "ElastOS Home");
        await afterCreate();
        return { id: "credential", rawId: new Uint8Array([1]).buffer, type: "public-key",
          response: { clientDataJSON: new Uint8Array([2]).buffer, attestationObject: new Uint8Array([3]).buffer } };
      }, async get() {
        gets++;
        return { id: "existing", rawId: new Uint8Array([1]).buffer, type: "public-key",
          response: { clientDataJSON: new Uint8Array([2]).buffer, authenticatorData: new Uint8Array([3]).buffer,
            signature: new Uint8Array([4]).buffer, userHandle: null } };
      } } },
      Date: class extends Date { static now() { return now; } }, Uint8Array, ArrayBuffer, TextEncoder, atob, btoa,
      setHomeAuthorityToken() {}, clearHomeAuthorityToken() {},
      async fetchJson(path, options = {}) {
        requests.push({ path, options });
        if (path.endsWith("/status")) return { registered: guest || verified,
          owner_setup_pending: !guest && verified && !terminalLoss && !acknowledged, guest_registration_enabled: guest };
        if (path.includes("/authenticate/")) return path.endsWith("/begin")
          ? { options: { publicKey: { challenge: "AQ", allowCredentials: [] } } } : grant;
        if (path.endsWith("/begin")) {
          assert.equal(options.headers["x-elastos-owner-enrollment"], guest || ownerClaimed ? undefined : "operator-secret");
          beforeBegin(options);
          assert.equal(options.method, "POST");
          assert.equal(options.headers["content-type"], "application/json");
          assert.deepEqual(JSON.parse(options.body), { intent });
          ownerClaimed = true;
          afterBegin();
          return { schema: "elastos.auth.passkey.register.begin/v1", ceremony_id: "ceremony",
            options: verified ? null : { publicKey: { challenge: "AQ", user: { id: "AQ" }, excludeCredentials: [] } } };
        }
        assert.ok(path.endsWith("/complete"));
        const request = JSON.parse(options.body);
        assert.deepEqual(request.intent, intent);
        assert.deepEqual(Object.keys(request).sort(), Object.hasOwn(request, "response")
          ? ["ceremony_id", "intent", "response"] : ["ceremony_id", "intent"]);
        if (!Object.hasOwn(request, "response")) assert.equal(verified && !guest, true);
        assert.equal(request.ceremony_id, "ceremony");
        assert.equal(options.headers["content-type"], "application/json");
        verified = true;
        if (responseMode === "lost") throw Error("response lost after verification");
        if (responseMode === "rejected") { const error = Error("completion rejected"); error.status = 422; throw error; }
        if (responseMode === "malformed") return {};
        acknowledged = true;
        return grant;
      },
    });
    vm.runInContext(source + "\nglobalThis.fixture = { showHomeUnlock, bindHomeUnlock, runPasskeyCreate };", context);
    const api = context.fixture;
    api.bindHomeUnlock();
    const onOpen = (response, flow) => { opened = JSON.parse(JSON.stringify({ response, flow })); };
    return { element, api, opened: () => opened, async show() { await api.showHomeUnlock(onOpen); },
      choose() {
        if (guest && !values.has(key)) element("#home-unlock-secondary").emit("click");
        element("#home-unlock-name").value = "Owner";
        element("#home-owner-token").value = "operator-secret";
        const radio = element("#home-enrollment-" + purpose);
        radio.checked = true;
        radio.emit("change");
      } };
  }
  return { reload, values, requests, writes, intent, creates: () => creates, gets: () => gets, advance: delta => { now += delta; },
    setWriteFailure: value => { writeFailure = value; }, setReadFailure: value => { readFailure = value; },
    setRemoveFailure: value => { removeFailure = value; }, afterBegin: fn => { afterBegin = fn; },
    beforeBegin: fn => { beforeBegin = fn; },
    afterCreate: fn => { afterCreate = fn; }, response: mode => { responseMode = mode; } };
}

for (const guest of [false, true]) for (const purpose of ["create", "recover"]) for (const terminalLoss of [false, true]) {
  const f = fixture({ guest, purpose, terminalLoss });
  let home = f.reload();
  await home.show();
  home.choose();
  assert.equal(home.element("#home-unlock-name").hidden, purpose === "recover");
  f.afterBegin(() => { home.element("#home-unlock-name").value = "Changed after begin"; });
  home.element("#home-unlock-primary").emit("click");
  await tick();
  assert.match(home.element("#home-unlock-status").textContent, /response lost/);
  assert.equal(f.creates(), 1);
  const saved = f.values.get(key);
  const completion = f.requests.find(r => r.path.endsWith("/complete")).options.body;
  assert.ok(!/operator-secret|one-grant|home_token|session_id/.test(saved));
  assert.deepEqual(JSON.parse(saved).intent, f.intent);
  f.advance(600000);
  f.response("ok");
  home = f.reload();
  await home.show();
  assert.equal(home.element("#home-unlock-primary").textContent, "Resume setup");
  assert.equal(home.element("#home-unlock-title").textContent, "Resume setup");
  assert.equal(home.element("#home-unlock-secondary").hidden, false);
  assert.equal(home.element("#home-unlock-secondary").textContent, "Back to sign in");
  home.element("#home-unlock-primary").emit("click");
  await tick();
  assert.equal(f.creates(), 1);
  assert.equal(f.requests.filter(r => r.path.endsWith("/begin")).length, 1);
  assert.equal(f.requests.filter(r => r.path.endsWith("/complete"))[1].options.body, completion);
  assert.deepEqual(home.opened(), { response: grant, flow: { enrollmentPurpose: purpose } });
  assert.equal(f.values.size, 0);
  assert.equal(new Set(f.writes.map(raw => JSON.parse(raw).expires_at)).size, 1, "retry extended retention");
}

for (const invalid of ["", "bad/name", "bad\\name", "bad\u0000name", "bad\u0085name", "bad\u009fname", "x".repeat(65), "é".repeat(33)]) {
  const f = fixture(), home = f.reload();
  await home.show(); home.choose();
  home.element("#home-unlock-name").value = invalid;
  await assert.rejects(home.api.runPasskeyCreate(), /display name/);
  assert.equal(f.creates(), 0);
  assert.equal(f.requests.filter(r => r.path.endsWith("/begin")).length, 0);
}

for (const guest of [false, true]) for (const purpose of ["create", "recover"]) {
  const f = fixture({ guest, purpose }), original = f.reload();
  await original.show(); original.choose();
  f.afterCreate(() => new Promise(() => {}));
  void original.api.runPasskeyCreate();
  await tick();
  const pending = JSON.parse(f.values.get(key));
  assert.equal(pending.ceremony_id, "ceremony");
  assert.equal(pending.response, null);
  assert.equal(f.requests.filter(r => r.path.endsWith("/complete")).length, 0);
  f.advance(600000); f.afterCreate(() => {}); f.response("ok");
  const resumed = f.reload(); await resumed.show();
  assert.equal(f.requests.filter(r => r.path.endsWith("/begin")).length, 1, "reload must wait for explicit Resume");
  await resumed.api.runPasskeyCreate();
  assert.equal(f.requests.filter(r => r.path.endsWith("/begin")).length, 2);
  assert.equal(f.creates(), 2, "only explicit Resume may start the second prompt");
  assert.deepEqual(resumed.opened(), { response: grant, flow: { enrollmentPurpose: purpose } });
  assert.equal(new Set(f.writes.map(raw => JSON.parse(raw).expires_at)).size, 1);
}

for (const guest of [false, true]) {
  const f = fixture({ guest }), home = f.reload();
  await home.show(); home.choose();
  home.element("#home-unlock-name").value = "Person";
  f.beforeBegin(options => {
    assert.equal(JSON.parse(options.body).intent.public_name, "Person");
    const error = Error("Choose a public display name"); error.status = 422; throw error;
  });
  await assert.rejects(home.api.runPasskeyCreate(), { message: "Choose a different display name." });
  assert.equal(f.creates(), 0);
  assert.equal(f.values.size, 0, "definitive pre-begin input rejection releases only local unverified setup");
  assert.equal(home.element("#home-unlock-name").disabled, false);
  assert.equal(home.element("#home-enrollment-recover").disabled, false);
  home.element("#home-unlock-name").value = "Owner";
  // A rejected begin did not establish a claim; the operator can re-enter it.
  if (!guest) {
    assert.equal(home.element("#home-owner-token").value, "", "rejected begin must clear the entered secret");
    home.element("#home-owner-token").value = "operator-secret";
  }
  f.beforeBegin(() => {}); f.response("ok");
  await home.api.runPasskeyCreate();
  assert.equal(f.creates(), 1);
  assert.deepEqual(home.opened(), { response: grant, flow: { enrollmentPurpose: "create" } });
}

for (const guest of [false, true]) for (const status of [undefined, 403, 500]) {
  const f = fixture({ guest }), home = f.reload();
  await home.show(); home.choose();
  f.afterBegin(() => { const error = Error("begin reply unavailable"); error.status = status; throw error; });
  await assert.rejects(home.api.runPasskeyCreate(), /begin reply unavailable/);
  const pending = JSON.parse(f.values.get(key));
  f.advance(600000); f.afterBegin(() => {}); f.response("ok");
  const resumed = f.reload(); await resumed.show();
  await resumed.api.runPasskeyCreate();
  assert.equal(f.creates(), 1);
  assert.deepEqual(f.requests.filter(r => r.path.endsWith("/begin")).map(r => JSON.parse(r.options.body)),
    [{ intent: pending.intent }, { intent: pending.intent }]);
  assert.equal(new Set(f.writes.map(raw => JSON.parse(raw).expires_at)).size, 1);
}

for (const raw of ["{", "x".repeat(65537), JSON.stringify({ schema: key, home_token: "forbidden" })]) {
  const f = fixture(); f.values.set(key, raw);
  const home = f.reload(); await home.show();
  assert.match(home.element("#home-unlock-status").textContent, /invalid/);
  assert.equal(home.element("#home-enrollment-dismiss").hidden, false);
  await assert.rejects(home.api.runPasskeyCreate(), /invalid/);
  assert.equal(f.creates(), 0);
  assert.equal(f.values.get(key), raw);
  home.element("#home-enrollment-dismiss").emit("click"); await tick();
  assert.equal(f.values.size, 0);
}

{
  const f = fixture(), home = f.reload();
  await home.show(); home.choose();
  await assert.rejects(home.api.runPasskeyCreate(), /response lost/);
  const saved = f.values.get(key);
  f.advance(12 * 60 * 60 * 1000 + 1);
  const next = f.reload(); await next.show();
  assert.match(next.element("#home-unlock-status").textContent, /expired/);
  await assert.rejects(next.api.runPasskeyCreate(), /expired/);
  assert.equal(f.creates(), 1);
  assert.equal(f.values.get(key), saved);
  next.element("#home-enrollment-dismiss").emit("click"); await tick();
  assert.equal(f.values.size, 0);
  next.element("#home-unlock-name").value = "Owner";
  f.response("ok");
  await next.api.runPasskeyCreate();
  assert.equal(f.creates(), 1, "dismissed expired retry created another owner credential");
  assert.deepEqual(next.opened(), { response: grant, flow: { enrollmentPurpose: "create" } });
}

for (const guest of [false, true]) for (const reason of ["cancelled", "timed out"]) {
  const f = fixture({ guest }), home = f.reload();
  await home.show(); home.choose();
  f.afterCreate(() => { const error = Error(reason); error.name = "NotAllowedError"; throw error; });
  await assert.rejects(home.api.runPasskeyCreate(), new RegExp(reason));
  const pending = JSON.parse(f.values.get(key));
  assert.equal(pending.ceremony_id, null);
  assert.equal(pending.response, null);
  assert.equal(f.requests.filter(r => r.path.endsWith("/complete")).length, 0);
  f.afterCreate(() => {});
  await assert.rejects(home.api.runPasskeyCreate(), /response lost/);
  assert.equal(f.creates(), 2);
  assert.equal(new Set(f.writes.map(raw => JSON.parse(raw).expires_at)).size, 1);
}
{
  const f = fixture(), home = f.reload();
  await home.show(); home.choose();
  await assert.rejects(home.api.runPasskeyCreate(), /response lost/);
  const pending = JSON.parse(f.values.get(key));
  pending.response = null;
  f.values.set(key, JSON.stringify(pending));
  f.response("ok");
  const next = f.reload(); await next.show();
  await next.api.runPasskeyCreate();
  assert.equal(f.creates(), 1, "verified owner resume created another passkey");
}
{
  const f = fixture({ purpose: "recover" }), home = f.reload();
  await home.show(); home.choose();
  assert.match(home.element("#home-unlock-copy").textContent, /Recovery Kit/);
  const create = home.element("#home-enrollment-create");
  create.checked = true; create.emit("change");
  assert.equal(home.element("#home-unlock-name").hidden, false);
  assert.doesNotMatch(home.element("#home-unlock-copy").textContent, /Recovery Kit/);
}
for (const field of ["ceremony", "id", "rawId", "clientDataJson", "attestationObject", "name", "c1-start", "c1-end", "extra"]) {
  const f = fixture(), home = f.reload();
  await home.show(); home.choose();
  await assert.rejects(home.api.runPasskeyCreate(), /response lost/);
  const pending = JSON.parse(f.values.get(key));
  if (field === "ceremony") pending.ceremony_id = "x".repeat(129);
  else if (field === "id" || field === "rawId") pending.response[field] = "x".repeat(8193);
  else if (field === "name") pending.intent.public_name = "é".repeat(33);
  else if (field === "c1-start") pending.intent.public_name = "bad\u0080name";
  else if (field === "c1-end") pending.intent.public_name = "bad\u009fname";
  else if (field === "extra") pending.operator_claim = "forbidden";
  else pending.response.response[field] = "x".repeat(field === "clientDataJson" ? 4097 : 32769);
  f.values.set(key, JSON.stringify(pending));
  const next = f.reload(); await next.show();
  await assert.rejects(next.api.runPasskeyCreate(), /invalid/);
  assert.equal(f.creates(), 1);
  assert.equal(f.requests.filter(r => r.path.endsWith("/complete")).length, 1);
}
for (const guest of [false, true]) {
  const f = fixture({ guest }), home = f.reload();
  await home.show(); home.choose(); f.response("rejected");
  await assert.rejects(home.api.runPasskeyCreate(), /completion rejected/);
  const saved = f.values.get(key);
  assert.ok(JSON.parse(saved).response, "completion rejection discarded the credential response");
  const completion = f.requests.find(r => r.path.endsWith("/complete")).options.body;
  f.advance(600000); f.response("ok");
  const resumed = f.reload(); await resumed.show(); await resumed.api.runPasskeyCreate();
  assert.equal(f.creates(), 1);
  assert.equal(f.requests.filter(r => r.path.endsWith("/begin")).length, 1);
  assert.equal(f.requests.filter(r => r.path.endsWith("/complete"))[1].options.body, completion);
  assert.equal(new Set(f.writes.map(raw => JSON.parse(raw).expires_at)).size, 1);
}
for (const stage of ["before-begin", "after-create"]) {
  const f = fixture(), home = f.reload();
  await home.show(); home.choose();
  if (stage === "before-begin") f.setWriteFailure(true);
  else f.afterCreate(() => f.setWriteFailure(true));
  await assert.rejects(home.api.runPasskeyCreate(), /cannot save/);
  assert.equal(f.creates(), stage === "before-begin" ? 0 : 1);
  assert.equal(f.requests.filter(r => r.path.endsWith("/complete")).length, 0);
  f.setWriteFailure(false); f.afterCreate(() => {});
  await assert.rejects(home.api.runPasskeyCreate(), /response lost/);
  assert.equal(f.creates(), 1);
}
{
  const f = fixture(), home = f.reload();
  await home.show(); home.choose(); f.setReadFailure(true);
  await assert.rejects(home.api.runPasskeyCreate(), /cannot read/);
  assert.equal(f.creates(), 0);
}
{
  const f = fixture(), home = f.reload();
  await home.show(); home.choose(); f.response("malformed");
  await assert.rejects(home.api.runPasskeyCreate(), /invalid completion/);
  assert.equal(f.values.size, 1);
  f.response("ok"); f.setRemoveFailure(true);
  await assert.rejects(home.api.runPasskeyCreate(), /cannot clear/);
  assert.equal(f.creates(), 1);
  f.setRemoveFailure(false); await home.api.runPasskeyCreate();
  assert.equal(f.creates(), 1);
  assert.equal(f.values.size, 0);
}
{
  const f = fixture({ guest: true }), home = f.reload();
  await home.show();
  home.element("#home-unlock-secondary").emit("click");
  assert.equal(home.element("#home-enrollment-choice").hidden, false);
  home.element("#home-unlock-secondary").emit("click");
  assert.equal(home.element("#home-enrollment-choice").hidden, true);
  assert.equal(home.element("#home-unlock-primary").textContent, "Use passkey");
  home.element("#home-unlock-primary").emit("click"); await tick();
  assert.equal(f.gets(), 1);
  assert.equal(f.creates(), 0);
}

// Execute the host's actual boot and unlock callback with effects recorded.
const hostSource = readFileSync(new URL("../capsules/home/browser/home-shell-host.js", import.meta.url), "utf8");
const section = (text, start, end) => {
  const a = text.indexOf(start), b = text.indexOf(end, a);
  assert.ok(a >= 0 && b > a);
  return text.slice(a, b);
};
{
  const f = fixture({ purpose: "recover" }), page = f.reload();
  await page.show(); page.choose();
  await assert.rejects(page.api.runPasskeyCreate(), /response lost/);
  const pending = f.values.get(key);
  const calls = [];
  let unlock, currentAccount = "different-signed-in-account";
  const context = vm.createContext({
    document: { body: { dataset: {} } }, console,
    activeShellBootHintTarget: () => "", isHomeAuthError: () => false,
    async refreshHomeSession() { calls.push("refresh:" + currentAccount); return { principal_id: currentAccount }; },
    async refreshShellSummary() { return { signed_in: true }; }, homeSummarySignedIn: () => true,
    async fetchJson(path) { assert.equal(path, "/api/apps/home/runtime/ensure"); calls.push("ensure"); },
    hideHomeUnlock() { calls.push("hide"); }, startShellTimers() {},
    enterHostAuthGate() {}, currentSignedProfileDisplayName: () => "", hideHostBootMask() {},
    showHomeUnlock(callback) { unlock = callback; calls.push("unlock"); },
    profileReadinessActionTarget() { calls.push("readiness"); return "people"; },
    async activateDesktopShell() { calls.push("desktop"); },
    async openTargetFromHomeGui(target, options) { calls.push({ target, options }); },
  });
  vm.runInContext(section(hostSource, "async function boot()", "function startShellTimers()")
    + section(hostSource, "async function showHostAuthGate", "async function launchHomeTarget"), context);
  await context.boot();
  assert.equal(currentAccount, "different-signed-in-account");
  assert.deepEqual(calls, ["refresh:different-signed-in-account", "ensure", "hide"]);
  assert.equal(f.values.get(key), pending, "ordinary signed-in boot changed pending registration");
  assert.equal(f.creates(), 1);
  assert.equal(f.requests.filter(r => r.path.endsWith("/complete")).length, 1);
  await context.showHostAuthGate();
  calls.length = 0;
  await unlock(grant, { enrollmentPurpose: "recover" });
  assert.deepEqual(JSON.parse(JSON.stringify(calls.slice(-2))), ["desktop",
    { target: "system", options: { query: { settings: "security", recovery: "import" } } }]);
  assert.ok(!calls.includes("readiness"));
  calls.length = 0;
  const pendingProfile = { ...grant, profile_readiness: { schema: "elastos.profile.readiness/v1", status: "setup_required" } };
  context.profileReadinessActionTarget = () => { calls.push("readiness"); return "system"; };
  await unlock(pendingProfile, { enrollmentPurpose: "create" });
  assert.ok(calls.includes("readiness"));
  assert.deepEqual(JSON.parse(JSON.stringify(calls.at(-1))),
    { target: "system", options: { query: { settings: "security" } } },
    "interrupted Create must keep its setup/export path");
  calls.length = 0;
  await unlock(pendingProfile);
  assert.deepEqual(JSON.parse(JSON.stringify(calls.at(-1))),
    { target: "system", options: { query: { settings: "security", recovery: "import" } } });
}
{
  const systemSource = readFileSync(new URL("../capsules/system/browser/system.js", import.meta.url), "utf8");
  const focused = [];
  const context = vm.createContext({ requestedSettingsActionFocused: false, requestedSettingsTab: "security",
    readQueryParam: () => "import", readText: value => value,
    recoveryImportInput: { disabled: false, focus() { focused.push("import"); } },
    recoveryDownloadButton: { disabled: false, focus() { focused.push("download"); } },
    document: { querySelector: () => ({ dataset: { settings: "security" } }) },
  });
  vm.runInContext(section(systemSource, "function focusRequestedSettingsAction()", "function hasShellAccess()"), context);
  context.focusRequestedSettingsAction(); context.focusRequestedSettingsAction();
  assert.deepEqual(focused, ["import"]);
  context.requestedSettingsActionFocused = false;
  context.document.querySelector = () => ({ dataset: { settings: "about" } });
  context.focusRequestedSettingsAction();
  assert.deepEqual(focused, ["import"], "late recovery focus stole active tab focus");
}
console.log("PASS Home owner/guest Create/Recover, exact reload retry, storage, host navigation and account boundaries");

if (process.argv.includes("--browser")) {
  const configured = process.env.ELASTOS_PLAYWRIGHT_MODULE;
  const moduleUrl = configured
    ? (configured.startsWith("file:") ? configured : pathToFileURL(resolve(configured)).href)
    : new URL("../elastos/tools/browser-playwright-engine/node_modules/playwright/index.js", import.meta.url).href;
  const imported = await import(moduleUrl);
  const { chromium } = imported.default || imported;
  const browser = await chromium.launch({
    headless: true,
    executablePath: process.env.ELASTOS_BROWSER_EXECUTABLE || undefined,
    args: ["--disable-background-networking", "--disable-component-update", "--no-first-run"],
  });
  try {
    const page = await browser.newPage({ reducedMotion: "reduce" });
    const errors = [];
    const statusRequests = [];
    page.on("pageerror", error => errors.push(error.message));
    const assets = new Map([
      ["index.html", "text/html"], ["style.css", "text/css"],
      ["shell-auth.js", "text/javascript"], ["shell-core.js", "text/javascript"],
      ["home-boot-hint.js", "text/javascript"], ["manifest.webmanifest", "application/manifest+json"],
      ["elastos-logo.svg", "image/svg+xml"], ["elastos-home-icon.svg", "image/svg+xml"],
      ["elastos-home-icon-192.png", "image/png"],
    ]);
    await page.route("**/*", async route => {
      const url = new URL(route.request().url());
      assert.equal(url.origin, "https://home-enrollment.test");
      if (url.pathname === "/api/auth/passkey/status") {
        assert.equal(route.request().method(), "GET");
        statusRequests.push(url.pathname);
        return route.fulfill({ json: { registered: false, guest_registration_enabled: false } });
      }
      const name = url.pathname.slice("/apps/home/".length);
      assert(url.pathname.startsWith("/apps/home/"));
      if (name === "home-shell-host.js") {
        return route.fulfill({ contentType: "text/javascript", body: `
          import { bindHomeUnlock, showHomeUnlock } from "./shell-auth.js?v=home-20260802a";
          document.querySelector("#home-shell-boot-mask").hidden = true;
          bindHomeUnlock();
          await showHomeUnlock(() => { throw new Error("Unexpected enrollment dispatch"); });
        ` });
      }
      assert(assets.has(name), "unexpected fixture request: " + url.pathname);
      return route.fulfill({ contentType: assets.get(name),
        body: readFileSync(new URL("../capsules/home/browser/" + name, import.meta.url)) });
    });
    for (const [width, height] of [[1280, 900], [390, 844], [320, 568]]) {
      await page.setViewportSize({ width, height });
      await page.goto("https://home-enrollment.test/apps/home/index.html");
      const dialog = page.getByRole("dialog", { name: "Set up Home", exact: true });
      await dialog.waitFor({ state: "visible" });
      assert.equal(await dialog.getAttribute("aria-describedby"), "home-unlock-copy");
      const create = page.getByRole("radio", { name: "Create account", exact: true });
      const recover = page.getByRole("radio", { name: "Recover account", exact: true });
      await create.focus();
      for (const purpose of ["create", "recover"]) {
        if (purpose === "recover") await create.press("ArrowDown");
        const radio = purpose === "create" ? create : recover;
        assert(await radio.isChecked(), purpose + " native radio selection");
        assert(await radio.evaluate(el => el === document.activeElement));
        assert.equal(await page.getByLabel("Your display name", { exact: true }).isVisible(), purpose === "create");
        await dialog.evaluate(el => { el.scrollTop = 0; });
        const bounds = await dialog.evaluate(el => ({
          width: innerWidth, height: innerHeight,
          boxes: [el, ...el.querySelectorAll("#home-enrollment-choice label")].map(node => {
            const { x, y, width, height } = node.getBoundingClientRect();
            return { x, y, width, height };
          }),
        }));
        for (const box of bounds.boxes) {
          assert(box.x >= 0 && box.y >= 0 && box.x + box.width <= width && box.y + box.height <= height,
            JSON.stringify({ purpose, width, height, box }));
        }
        for (const box of bounds.boxes.slice(1)) assert(box.height >= 44 && box.width >= 44);
        if (width === 390 && process.env.ELASTOS_OWNER_ENROLLMENT_SCREENSHOT_DIR) {
          const directory = resolve(process.env.ELASTOS_OWNER_ENROLLMENT_SCREENSHOT_DIR);
          mkdirSync(directory, { recursive: true });
          await page.screenshot({ path: resolve(directory, `home-enrollment-${purpose}-390.png`) });
        }
        const controls = await dialog.locator("input, button").evaluateAll(elements => elements
          .filter(el => el.getClientRects().length && !el.disabled && (el.type !== "radio" || el.checked))
          .map(el => el.id));
        assert(controls.includes("home-owner-token"), "HTTPS fixture covers the operator field");
        assert(controls.includes("home-unlock-primary"));
        await radio.focus();
        for (const [index, id] of controls.entries()) {
          if (index) await page.keyboard.press("Tab");
          const control = page.locator("#" + id);
          assert(await control.evaluate(el => el === document.activeElement), "native Tab must reach " + id);
          await control.scrollIntoViewIfNeeded();
          const visible = await control.evaluate(el => {
            const box = el.getBoundingClientRect();
            const card = el.closest('[role="dialog"]').getBoundingClientRect();
            return box.left >= Math.max(0, card.left) && box.right <= Math.min(innerWidth, card.right)
              && box.top >= Math.max(0, card.top) && box.bottom <= Math.min(innerHeight, card.bottom);
          });
          assert(visible, id + " must be reachable inside card and viewport");
        }
      }
      await recover.press("ArrowUp");
      assert(await create.isChecked());
      assert(await page.getByLabel("Your display name", { exact: true }).isVisible());
    }
    assert.equal(statusRequests.length, 3);
    assert.deepEqual(errors, []);
    console.log("PASS isolated browser: six Create/Recover views, accessible dialog, native radios, 44px targets, scrollable keyboard controls, viewport bounds, zero page errors");
  } finally {
    await browser.close();
  }
}
