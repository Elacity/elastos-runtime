import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { createHomeClipboardClient } from "../capsules/home/browser/home-clipboard-client.js";
import { createHomeClipboardHost, createHomeClipboardFrameState } from "../capsules/home/browser/home-clipboard-host.js";
import { createHomeNavigationClient } from "../capsules/home/browser/home-navigation-client.js";

const homeOrigin = "https://home.fixture";
const hostSource = readFileSync(new URL("../capsules/home/browser/home-shell-host.js", import.meta.url), "utf8");
function sourceFunction(name) {
  const start = hostSource.indexOf(`function ${name}(`);
  assert.ok(start >= 0, name);
  return hostSource.slice(hostSource.slice(start - 6, start) === "async " ? start - 6 : start, hostSource.indexOf("\n}", start) + 2);
}
function fixture(target = "chat-room", enabled = true) {
  const listeners = new Map(), timers = new Map(), frames = new Map(), commands = [];
  let next = 0, receive, launches = 0, refreshed = 0, currentToken = "old-frame", holdPrepare = false;
  const authority = { signed_in: true, principal_id: "alice", proof_binding_id: "proof:alice" };
  const shell = {}, home = { location: { origin: homeOrigin, href: homeOrigin },
    setTimeout(fn) { timers.set(++next, fn); return next; }, clearTimeout(id) { timers.delete(id); },
    addEventListener(type, fn) { if (type === "message") receive = fn; },
  };
  const frame = { postMessage(data) { for (const fn of listeners.get("message") || []) fn({ data, source: home, origin: homeOrigin }); } };
  const windowRef = { top: home, parent: shell, crypto: { randomUUID: () => `request-${++next}` },
    setTimeout: home.setTimeout, clearTimeout: home.clearTimeout,
    addEventListener(type, fn) { if (!listeners.has(type)) listeners.set(type, new Set()); listeners.get(type).add(fn); },
    removeEventListener(type, fn) { listeners.get(type)?.delete(fn); },
  };
  const message = (data, source = frame, origin = "null") => receive({ data, source, origin });
  home.postMessage = data => message(data);
  const old = { targetId: target, source: frame, origin: "null", expiresAt: 60_000 };
  frames.set("old-frame", old);
  const shellState = { currentSummary: { authority } };
  const receipt = command => message({ type: `home:frame-authority-${command.phase === "prepare" ? "renewed" : "committed"}`,
    homeToken: "shell-token", requestId: command.requestId, oldHomeToken: command.homeToken,
    freshHomeToken: "fresh-frame", ok: true }, shell);
  const context = vm.createContext({ console, URL, URLSearchParams, Promise, crypto: { randomUUID: () => `picker-${++next}` }, window: home,
    Date: class extends Date { static now() { return 59_000; } },
    shellState, launchedAppContexts: frames,
    activeShellFrame: { contentWindow: shell, dataset: { route: "/apps/home-gui/#home_token=shell-token" } },
    activeShellRoot: { dataset: { target: "home-gui" } },
    OPAQUE_CAPSULE_ORIGIN: "null", OPAQUE_FRAME_TARGET: "*", HOME_GUI_SHELL_ID: "home-gui",
    HOME_CLI_SHELL_ID: "home-cli", WALLET_CONNECTOR_EFFECT_TYPE: "wallet-effect", FORM_FACTOR_MESSAGE: "form-factor",
    BROWSER_AUTHORITY_RENEWAL_REQUEST_TYPE: "browser-request", BROWSER_AUTHORITY_RENEWAL_RESULT_TYPE: "browser-result",
    pendingBrowserAuthorityRenewals: new Map(),
    homeExternalLinkPrompt: { retire() {}, transfer() {} },
    homeClipboardHost: { handle: () => false, retireFrame() {}, resetFrame() {} },
    handleHomeWalletConnectorEffect: () => false,

    hasExactMessageKeys: (data, keys) => Object.keys(data).sort().join() === [...keys].sort().join(),
    requireHomeGuiActive() { assert.equal(authority.signed_in, true); },
    postToActiveShell(command) {
      commands.push(command);
      if (command.command === "renew-frame-authority" && (!holdPrepare || command.phase === "commit")) queueMicrotask(() => receipt(command));
      return true;
    },
    refreshHomeSession: async () => { refreshed++; }, refreshShellSummary: async () => shellState.currentSummary,
    isHomeAuthError: () => false,
    launchHomeTarget: async requested => {
      launches++;
      if (!authority.signed_in) throw Object.assign(new Error("Sign in"), { status: 401 });
      frames.set("fresh-frame", { targetId: requested, source: null, origin: "null", parentOrigin: homeOrigin, expiresAt: 120_000 });
      return { target: requested, attach_kind: "iframe", expires_at: 120,
        route: `/apps/${requested}/#home_token=fresh-frame` };
    },
  });
  for (const name of ["homeMessageContext", "homeLaunchTokenFromRoute", "retireLaunchedAppContext", "cancelFrameRenewal",
    "cancelBrowserAuthorityRenewalsForToken", "openLibraryPicker", "validPickerIdentity", "currentLibraryPicker", "settleLibraryPicker", "cancelLibraryPicker", "acceptLibraryPicker",
    "deliverMessageToHomeGuiTargetFrame", "settleFrameRenewal", "replyToShellRequest", "homeSummarySignedIn", "homeSummaryHasProofBoundSession",
    "renewExpiringFrames", "refreshSignedHomeSession"])
    vm.runInContext(sourceFunction(name), context);
  if (hostSource.includes("function transferFrameAssociations(")) vm.runInContext(sourceFunction("transferFrameAssociations"), context);
  const start = hostSource.indexOf('window.addEventListener("message", (event) => {');
  vm.runInContext(hostSource.slice(start, hostSource.indexOf("\n});", start) + 4), context);
  const client = createHomeNavigationClient({ homeToken: currentToken, homeOrigin, target, enabled, windowRef,
    onHomeToken(token) { currentToken = token; } });
  return { client, context, frames, message, receipt, commands, authority, frame, old, timers, home, windowRef,
    hold() { holdPrepare = true; }, token: () => currentToken, launches: () => launches, refreshed: () => refreshed,
    close() { context.retireLaunchedAppContext("old-frame"); },
  };
}
const settle = async () => { for (let i = 0; i < 8; i++) await Promise.resolve(); };

for (const target of ["chat-room", "people", "documents", "assistant", "library"]) {
  test(`Home refresh replaces an expiring open ${target} frame authority`, async () => {
    const f = fixture(target);
    await f.context.refreshSignedHomeSession(); await settle();
    assert.equal(f.refreshed(), 1);
    assert.equal(f.token(), "fresh-frame");
    assert.equal(f.frames.has("old-frame"), false);
    assert.equal(f.frames.get("fresh-frame").source, f.frame);
    assert.equal(f.launches(), 1);
    assert.equal(f.timers.size, 0);
  });
}

test("A stolen frame token, wrong origin or another target cannot request renewal", async () => {
  const f = fixture();
  const request = { type: "home:launch-target", homeToken: "old-frame", requestId: "renew-1", target: "chat-room", query: {} };
  f.message(request, {});
  f.message(request, f.frame, homeOrigin);
  f.message({ ...request, target: "documents" });
  await settle();
  assert.equal(f.launches(), 0);
  assert.equal(f.token(), "old-frame");
});

test("Home's own refused authority leaves the registered app unchanged", async () => {
  const f = fixture(); f.authority.signed_in = false;
  await assert.rejects(f.client.renewAuthority(), /Sign in to Home/);
  assert.equal(f.token(), "old-frame");
  assert.equal(f.frames.has("fresh-frame"), false);
});

for (const invalidation of ["close", "principal", "sign-out"]) {
  test(`${invalidation} during renewal rejects late receipts`, async () => {
    const f = fixture(); f.hold();
    const pending = f.client.renewAuthority();
    const rejected = assert.rejects(pending);
    assert.equal(f.client.renewAuthority(), pending);
    await settle();
    const command = f.commands.find(c => c.phase === "prepare");
    assert.ok(command);
    f.receipt({ ...command, requestId: "forged" });
    assert.equal(f.token(), "old-frame");
    if (invalidation === "close") f.close();
    if (invalidation === "principal") f.authority.principal_id = "bob";
    if (invalidation === "sign-out") f.authority.signed_in = false;
    f.receipt(command); await settle();
    for (const timer of [...f.timers.values()]) timer();
    await rejected;
    assert.equal(f.token(), "old-frame");
    assert.equal(f.frames.has("fresh-frame"), false);
  });
}

function pickerFixture(renewTarget, openerTarget = "chat-room") {
  const f = fixture(renewTarget, renewTarget !== "library");
  const opener = renewTarget === "library" ? { targetId: openerTarget, source: { postMessage(data) { delivered.push(data); } }, origin: "null" } : f.old;
  const delivered = [];
  if (opener === f.old) {
    const post = f.frame.postMessage;
    f.frame.postMessage = data => { delivered.push(data); post(data); };
  }
  const picker = renewTarget === "library" ? f.old : { targetId: "library", source: {}, origin: "null" };
  const openerToken = opener === f.old ? "old-frame" : "opener";
  const pickerToken = picker === f.old ? "old-frame" : "picker";
  f.frames.set(openerToken, opener); f.frames.set(pickerToken, picker);
  let record;
  if (openerTarget === "chat-room") {
    f.context.openTargetFromHomeGui = (target, { query }) => {
      assert.equal(target, "library");
      record = opener.pickerRequest;
      assert.equal(query.pickerRequestId, record.id);
      record.pickerToken = pickerToken;
      picker.pickerOwner = record;
    };
    f.context.openLibraryPicker({ homeToken: openerToken, source: opener.source, targetId: openerTarget },
      { requestId: "pick-request", documentNonce: "document-nonce", query: { mode: "attach", returnTarget: openerTarget } });
  } else {
    // Documents has no picker UI in this tree. Exercise association ownership
    // without adding a new Documents selection contract to a renewal fix.
    record = { id: "picker-id", requestId: "pick-request", documentNonce: "document-nonce",
      openerToken, opener, source: opener.source, pickerToken, phase: "ready" };
    opener.pickerRequest = record; picker.pickerOwner = record;
  }
  return { ...f, record, delivered, opener, picker };
}

for (const target of ["chat-room", "library"]) {
  test(`Library selection survives renewal of the ${target} frame`, async () => {
    const f = pickerFixture(target);
    await f.client.renewAuthority(); await settle();
    const fresh = f.frames.get("fresh-frame");
    assert.equal(f.context.currentLibraryPicker(f.record), true);
    if (target === "library") {
      assert.equal(f.record.pickerToken, "fresh-frame");
      assert.equal(fresh.pickerOwner, f.record);
      assert.equal(f.old.pickerOwner, null);
    } else {
      assert.equal(f.record.opener, fresh);
      assert.equal(f.record.openerToken, "fresh-frame");
      assert.equal(f.old.pickerRequest, null);
    }
    if (target === "library") {
      assert.equal(await f.context.deliverMessageToHomeGuiTargetFrame("chat-room", { type: "chat-room:attach-library-item" },
        { targetId: "library", homeToken: "old-frame", source: f.picker.source }, f.record.id), false);
    }
    const delivery = f.context.deliverMessageToHomeGuiTargetFrame("chat-room", { type: "chat-room:attach-library-item" },
      { targetId: "library", homeToken: f.record.pickerToken, source: f.picker.source }, f.record.id);
    const data = f.delivered.at(-1);
    assert.equal(data.pickerId, f.record.id);
    const reply = { ...data, type: "home:picker-accepted", accepted: true };
    assert.equal(f.context.acceptLibraryPicker({ homeToken: "old-frame", source: f.record.source }, reply), false);
    assert.equal(f.record.phase, "delivering");
    assert.equal(f.context.acceptLibraryPicker({ homeToken: f.record.openerToken, source: f.record.source }, reply), true);
    assert.equal(await delivery, true);
  });
}

for (const target of ["documents", "library"]) {
  test(`A Documents-owned picker record survives ${target} renewal`, async () => {
    const f = pickerFixture(target, "documents");
    await f.client.renewAuthority(); await settle();
    assert.equal(f.context.currentLibraryPicker(f.record), true);
    assert.equal(f.record.phase, "ready");
    assert.equal(f.record.openerToken, target === "documents" ? "fresh-frame" : "opener");
    assert.equal(f.record.pickerToken, target === "library" ? "fresh-frame" : "picker");
  });
}

for (const target of ["chat-room", "library"]) for (const closed of ["opener", "picker"]) {
  test(`Closing ${closed} cancels the picker during ${target} renewal`, async () => {
    const f = pickerFixture(target); f.hold();
    const pending = f.client.renewAuthority();
    const result = pending.catch(() => {});
    await settle();
    f.context.retireLaunchedAppContext(f.record[`${closed}Token`]);
    assert.equal(f.record.phase, "retired");
    const prepare = f.commands.find(command => command.phase === "prepare");
    f.receipt(prepare); await settle();
    for (const timer of [...f.timers.values()]) timer();
    await result;
    assert.equal(f.context.acceptLibraryPicker({ homeToken: f.record.openerToken, source: f.record.source }, {}), false);
    assert.equal(f.record.phase, "retired");
  });
}


test("Clipboard confirmation and replay state survive frame renewal", async () => {
  const f = fixture("chat-room");
  let allow;
  const writes = [];
  const host = createHomeClipboardHost({
    clipboard: { writeText: async text => writes.push(text) },
    prompt: { request: () => new Promise(resolve => { allow = resolve; }), cancel() {} },
    cryptoRef: { randomUUID: () => "clipboard-generation" },
    setTimeoutFn: f.home.setTimeout, clearTimeoutFn: f.home.clearTimeout,
  });
  f.context.homeClipboardHost = host;
  f.old.clipboardState = createHomeClipboardFrameState();
  f.old.parentOrigin = homeOrigin;
  const client = createHomeClipboardClient({ targetId: "chat-room", homeOrigin, homeToken: "old-frame",
    targetWindow: f.home, sourceWindow: f.windowRef, cryptoRef: { randomUUID: () => "copy-request" },
    setTimeoutFn: f.home.setTimeout, clearTimeoutFn: f.home.clearTimeout });
  client.start();
  const state = f.old.clipboardState;
  const generation = state.generation;
  const copied = client.writeText("hello", { purpose: "profile.id" });
  await settle();
  assert.equal(typeof allow, "function");
  await f.client.renewAuthority();
  client.setHomeToken("fresh-frame");
  assert.equal(f.frames.get("fresh-frame").clipboardState, state);
  assert.equal(f.old.clipboardState, null);
  assert.equal(state.generation, generation);
  assert.equal(state.retired, false);
  assert.equal(state.inFlight.homeToken, "fresh-frame");
  assert.equal(state.replayIds.has("copy-request"), true);
  allow(true);
  assert.equal(await copied, true);
  assert.deepEqual(writes, ["hello"]);
  client.teardown();
});

test("Pending Browser requests move ownership and retain their GUI reply identity", async () => {
  const f = fixture();
  const record = { requestId: "browser-request", oldHomeToken: "old-frame", frameToken: "old-frame", timeout: 0 };
  f.context.pendingBrowserAuthorityRenewals.set(record.requestId, record);
  await f.client.renewAuthority();
  assert.equal(f.context.pendingBrowserAuthorityRenewals.get(record.requestId), record);
  assert.equal(record.frameToken, "fresh-frame");
  assert.equal(record.oldHomeToken, "old-frame");
  f.context.retireLaunchedAppContext("fresh-frame");
  assert.equal(f.context.pendingBrowserAuthorityRenewals.size, 0);
});
