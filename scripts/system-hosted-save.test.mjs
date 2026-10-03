import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { randomUUID } from "node:crypto";
import { test } from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../capsules/system/browser/system.js", import.meta.url), "utf8");
const controller = source.slice(source.indexOf("function configureAiProvider() {"), source.indexOf("function configurePasskeyAccess() {"));

function fixture(connections = [], guest = false, staged = []) {
  const nodes = new Map();
  const element = () => ({
    value: "", hidden: false, disabled: false, dataset: {}, selectedOptions: [], children: [], handlers: {},
    focus() {},
    addEventListener(event, callback) { this.handlers[event] = callback; },
    append(...items) { this.children.push(...items); },
    after() {},
    replaceChildren(...items) { this.children = items; },
    querySelectorAll() { return []; },
  });
  const node = selector => {
    if (!nodes.has(selector)) nodes.set(selector, element());
    return nodes.get(selector);
  };
  const saved = new Map(), messages = [], attempts = [], discarded = [];
  let stagedConnections = staged.slice();
  let rejectActivation = true;
  let consentPending = false;
  const context = vm.createContext({
    crypto: { randomUUID },
    document: { querySelector: node, querySelectorAll: () => [], createElement: element },
    hasShellAccess: () => true,
    readText: value => typeof value === "string" ? value.trim() : "",
    setTextFields: (field, message) => { if (field === "ai-provider-state") messages.push(message); },
    shellHeaders: extra => extra || {}, publicSystemError: (_, fallback) => fallback,
    hostedProviderValidationError: () => "This Home could not check the key. Try again.",
    openCapsuleTarget: () => {},
    fetchJson: async (_url, init = {}) => {
      if (init.method === "DELETE" && _url.endsWith("/staged")) {
        const { id } = JSON.parse(init.body);
        discarded.push(id);
        stagedConnections = stagedConnections.filter(entry => entry.id !== id);
        return { discarded: true };
      }
      if (init.method !== "POST") {
        if (guest) throw new Error("request failed: 403 admin passkey required");
        return { connections, staged_connections: stagedConnections };
      }
      const body = JSON.parse(init.body);
      attempts.push(body);
      if (consentPending) {
        if (!stagedConnections.some(entry => entry.id === body.id)) {
          stagedConnections.push({ id: body.id, provider: body.provider, has_saved_model: false });
        }
        throw new Error("request failed: 400 Approve this hosted connection in Inbox, then check the key again.");
      }
      saved.set(body.id, body);
      if (rejectActivation) throw new Error("request failed: 409 provider error: selection_unavailable: model offer is not available");
      return { connections: [] };
    },
  });
  vm.runInContext(`${controller}\nconfigureAiProvider();`, context);
  return { node, saved, messages, attempts, discarded,
    activate: () => { rejectActivation = false; }, pending: () => { consentPending = true; } };
}

test("guest setup does not keep an entered key or offer an unusable Add form", async () => {
  const f = fixture([], true);
  f.node("#ai-provider-key").value = "guest-key";
  await new Promise(setImmediate);
  assert.equal(f.node("#ai-provider-key").value, "");
  assert.equal(f.node("#ai-provider-add").hidden, true);
  assert.equal(f.node("#ai-provider-form").hidden, true);
  assert.equal(f.node("#approval-lens").hidden, true);
  assert.match(f.messages.at(-1), /not available for this account/);
  assert.equal(f.attempts.length, 0);
});

test("hosted Save keeps one identity and entered key across activation failure and retry", async () => {
  const f = fixture();
  f.node("#ai-provider-add").handlers.click();
  f.node("#ai-provider-name").value = "Jev";
  f.node("#ai-provider-model").value = "typesafe/jev-1.13";
  f.node("#ai-provider-key").value = "fixture-key";
  await f.node("#ai-provider-save").handlers.click();
  const first = f.attempts[0];
  assert.match(first.id, /^model:hosted-[a-f0-9]{32}$/);
  assert.equal(f.node("#ai-provider-key").value, "fixture-key");
  assert.equal(f.node("#ai-provider-form").hidden, false);
  assert.match(f.messages.at(-1), /saved.*waiting.*Select Save/);
  f.activate();
  await f.node("#ai-provider-save").handlers.click();
  assert.equal(f.saved.size, 1);
  assert.equal(f.attempts[1].id, first.id);
  assert.equal(f.node("#ai-provider-key").value, "");
  assert.equal(f.node("#ai-provider-form").hidden, true);
  assert.equal(f.messages.at(-1), "This hosted model is saved on this Home.");
  f.node("#ai-provider-add").handlers.click();
  await f.node("#ai-provider-save").handlers.click();
  assert.notEqual(f.attempts[2].id, first.id);
});

test("hosted key feedback separates paused egress, Home authority, and provider rejection", () => {
  const functions = source.slice(
    source.indexOf("function publicSystemError("),
    source.indexOf("function showError("),
  );
  const context = vm.createContext({ readText: value => typeof value === "string" ? value.trim() : "" });
  vm.runInContext(functions, context);
  const message = detail => vm.runInContext(
    `hostedProviderValidationError(new Error(${JSON.stringify(detail)}))`, context,
  );
  assert.equal(
    message("request failed: 400 Hosted external HTTPS is paused until Runtime network authority is available."),
    "External HTTPS is paused on this Home. The key has not been checked.",
  );
  assert.equal(message("request failed: 403 admin passkey required"), "Sign in as the Home admin to check provider keys.");
  assert.equal(message("request failed: 403 home launch token expired"), "This Home could not check the key. Try again.");
  assert.equal(message("request failed: 400 invalid Venice key"), "The provider could not validate this key.");
  assert.match(message("request failed: 400 The hosted connection request was denied. Try again after the decision window."), /was denied/);
  assert.match(message("request failed: 400 Hosted access was denied or ended. Review Inbox."), /denied or ended/);
  assert.match(message("request failed: 400 Hosted HTTPS was ended on this Home. Start a new connection check in Inbox."), /was ended/);
  assert.match(message("request failed: 400 Runtime blocked this hosted route. Review the connection configuration."), /blocked this hosted route/);
  assert.match(message("request failed: 400 Hosted HTTPS could not reach the host. Check the network and try again."), /network/);
  assert.equal(message("request failed: 502 Bad Gateway"), "This Home could not check the key. Try again.");
  assert(!source.includes('publicSystemError(error, "This key is invalid.")'));
});

test("Cancel after a failed Add restores provider choice and gives the next form a new identity", async () => {
  const f = fixture();
  f.pending();
  f.node("#ai-provider-add").handlers.click();
  await f.node("#ai-provider-save").handlers.click();
  assert.equal(f.node("#ai-provider-kind").disabled, true);
  const firstId = f.attempts[0].id;
  f.node("#ai-provider-cancel").handlers.click();
  assert.equal(f.node("#ai-provider-instances").children.length, 1);
  f.node("#ai-provider-add").handlers.click();
  assert.equal(f.node("#ai-provider-kind").disabled, false);
  f.node("#ai-provider-kind").value = "venice";
  await f.node("#ai-provider-save").handlers.click();
  assert.notEqual(f.attempts[1].id, firstId);
  assert.equal(f.attempts[1].provider, "venice");
  f.node("#ai-provider-cancel").handlers.click();
  const stagedCard = f.node("#ai-provider-instances").children[0];
  await stagedCard.children[2].handlers.click();
  assert.deepEqual(f.discarded, [firstId]);
});

test("staged key survives reload and Close; Discard clears only the staged setup", async () => {
  const id = `model:hosted-${"a".repeat(32)}`;
  const f = fixture([], false, [{ id, provider: "openrouter", has_saved_model: false }]);
  await new Promise(setImmediate);
  const card = f.node("#ai-provider-instances").children[0];
  card.children[1].handlers.click();
  assert.equal(f.node("#ai-provider-cancel").textContent, "Close; keep staged key");
  f.node("#ai-provider-cancel").handlers.click();
  assert.equal(f.discarded.length, 0);
  assert.equal(f.node("#ai-provider-instances").children.length, 1);
  await card.children[2].handlers.click();
  assert.deepEqual(f.discarded, [id]);
  assert.equal(f.node("#ai-provider-instances").children.length, 0);
});

test("discarding a staged key change keeps the saved model card", async () => {
  const id = `model:hosted-${"b".repeat(32)}`;
  const connection = { id, name: "Saved", provider: "openrouter", connected: true,
    selected_model: "fixture/model", operation: "text.generate" };
  const f = fixture([connection], false, [{ id, provider: "openrouter", has_saved_model: true }]);
  await new Promise(setImmediate);
  const stagedCard = f.node("#ai-provider-instances").children[1];
  await stagedCard.children[2].handlers.click();
  assert.deepEqual(f.discarded, [id]);
  assert.equal(f.node("#ai-provider-instances").children.length, 1);
  assert.match(f.messages.at(-1), /saved model key remains/);
});

test("Edit uses the existing identity with a blank key and explains server-side key retention", async () => {
  const id = `model:hosted-${"a".repeat(32)}`;
  const f = fixture([{ id, name: "Text model", provider: "openrouter", connected: true,
    selected_model: "openai/gpt-4o-mini", operation: "text.generate" }]);
  await new Promise(setImmediate);
  const card = f.node("#ai-provider-instances").children[0];
  card.children[2].children.find(button => button.textContent === "Edit").handlers.click();
  assert.match(f.messages.at(-1), /Leave the API key blank to keep the stored key/);
  assert.equal(f.node("#ai-provider-kind").disabled, true);
  assert.equal(f.node("#ai-provider-key").value, "");
  f.activate();
  await f.node("#ai-provider-save").handlers.click();
  assert.equal(f.attempts[0].id, id);
  assert.equal(f.attempts[0].api_key, "");
  assert.equal(f.attempts[0].model, "openai/gpt-4o-mini");
});


test("hosted handoff replaces the previous selection with the exact requested offer", () => {
  const entry = readFileSync(new URL("../capsules/assistant/browser/home-agent.js", import.meta.url), "utf8");
  const apply = entry.slice(entry.indexOf("function applyLaunchQuery("), entry.indexOf("function raiseRoom("));
  let selected = "model:previous";
  let saves = 0;
  const context = vm.createContext({
    validModelCid: () => false,
    selectLiveOffer: id => { selected = id; },
    scheduleAgentWorkspacePersist: () => saves++,
    selectSession: () => {}, window: {},
  });
  vm.runInContext(`${apply}\napplyLaunchQuery({ offer_id: "model:requested" });`, context);
  assert.equal(selected, "model:requested");
  assert.equal(saves, 1);
  vm.runInContext('applyLaunchQuery({ offer_id: "model:unavailable" });', context);
  assert.equal(selected, "model:unavailable");
});

const updateFunctions = source.slice(source.indexOf("function configureRuntimeUpdate() {"), source.indexOf("function readQueryParam("));

function updateFixture(overrides = {}) {
  const fields = new Map(), hiddenFields = new Map(), timers = new Map(), listeners = new Map();
  const posts = [], approvals = [];
  let timerId = 0;
  const button = { hidden: true, disabled: true, addEventListener(type, handler) { this[type] = handler; } };
  const panel = { hidden: true }, status = { textContent: "" }, top = {};
  const context = vm.createContext({
    AbortController,
    runtimeUpdateNode: panel, runtimeUpdateButton: button, runtimeUpdateStatusNode: status,
    runtimeUpdate: undefined, runtimeUpdatePending: null, runtimeUpdateBusy: false,
    runtimeUpdateActive: true, runtimeUpdateTimer: 0, runtimeUpdateReconnects: 0,
    systemSummaryInFlight: null, homeParentOrigin: "https://home.example",
    document: { hidden: false, addEventListener: (type, handler) => listeners.set(`document:${type}`, handler) },
    window: {
      top, crypto: { randomUUID },
      addEventListener: (type, handler) => listeners.set(type, handler),
      setTimeout: (handler, delay) => { timers.set(++timerId, { handler, delay }); return timerId; },
      clearTimeout: id => timers.delete(id),
    },
    readText: value => typeof value === "string" ? value.trim() : "",
    setTextFields: (field, value) => fields.set(field, value),
    setHiddenFields: (field, value) => hiddenFields.set(field, value),
    shellHeaders: extra => ({ "x-elastos-home-token": "system-token", ...extra }),
    requestPasskeyStepUp: async (operation, intent) => { approvals.push({ operation, intent: JSON.parse(JSON.stringify(intent)) }); return "approved-exact-choice"; },
    fetchJson: async (url, init) => { posts.push({ url, ...init }); return {}; },
    refreshSystemSummary: async () => {},
    ...overrides,
  });
  vm.runInContext(updateFunctions, context);
  const offer = {
    configured: true, available: true, can_apply: true, source_name: "publisher",
    channel: "stable", publisher: "did:key:verified", publisher_did: "did:key:verified",
    current_version: "1.0.0", new_version: "1.1.0", head_cid: "head-exact", release_cid: "release-exact",
    changes: ["A signed change.", "Another signed change."], controller: { phase: "ready", id: null },
  };
  context.configureRuntimeUpdate();
  context.renderRuntimeUpdate(offer);
  return { context, offer, button, panel, status, fields, hiddenFields, timers, listeners, posts, approvals, top };
}

test("Home update requires a verified complete offer and a controller that can apply", () => {
  const f = updateFixture();
  assert.equal(f.button.hidden, false);
  assert.equal(f.fields.get("update-current-version"), "1.0.0");
  assert.equal(f.fields.get("update-new-version"), "1.1.0");
  assert.equal(f.fields.get("update-publisher"), "did:key:verified");
  assert.equal(f.fields.get("update-changes"), "A signed change.\nAnother signed change.");
  for (const refusal of [null, { configured: false }, { available: false }, { can_apply: false },
    { configured: "true" }, { publisher_did: "" }, { release_cid: "" }, { controller: null }, { controller: { phase: "restarting" } }]) {
    f.context.renderRuntimeUpdate(refusal === null ? null : { ...f.offer, ...refusal });
    assert.equal(f.button.hidden, true, JSON.stringify(refusal));
    assert.equal(f.button.disabled, true);
  }
  for (const phase of ["ready", "updated", "restored", "failed"]) {
    f.context.renderRuntimeUpdate({ ...f.offer, controller: { phase } });
    assert.equal(f.button.hidden, false, phase);
  }
  f.context.renderRuntimeUpdate({ ...f.offer, changes: [] });
  assert.equal(f.fields.get("update-changes"), "Publisher has not supplied change notes.");
  f.context.renderRuntimeUpdate({ ...f.offer, available: false, message: "Home is up to date." });
  assert.equal(f.hiddenFields.get("update-changes"), true);
  assert.equal(f.status.textContent, "Home is up to date.");
  f.context.renderRuntimeUpdate({ configured: true, available: false, can_apply: false,
    controller: { phase: "restarting", current_version: "1.0.0", new_version: "1.1.0" } });
  assert.equal(f.hiddenFields.get("update-offer"), false, "reopened System keeps durable progress versions");
  assert.equal(f.fields.get("update-current-version"), "1.0.0");
  assert.equal(f.fields.get("update-new-version"), "1.1.0");
  assert.equal(f.hiddenFields.get("update-publisher-row"), true, "progress does not invent a publisher");
  f.context.renderRuntimeUpdate(null);
  assert.equal(f.timers.size, 0, "source Home without an installed controller has no update polling");
});

test("Home update freezes one exact passkey intent and retries a lost response with identical bytes", async () => {
  let approve;
  let attempt = 0;
  const f = updateFixture({ requestPasskeyStepUp: (operation, intent) => {
    f.approvals.push({ operation, intent: JSON.parse(JSON.stringify(intent)) });
    return new Promise(resolve => { approve = resolve; });
  } });
  f.context.fetchJson = async (url, init) => {
    f.posts.push({ url, ...init });
    if (++attempt === 1) throw new TypeError("response lost");
    return {};
  };
  const apply = f.context.onRuntimeUpdateApply();
  await f.context.onRuntimeUpdateApply();
  assert.equal(f.approvals.length, 1);
  assert.equal(f.posts.length, 0, "approval precedes mutation");
  assert.equal(f.button.disabled, true);
  f.context.renderRuntimeUpdate({ ...f.offer, new_version: "1.2.0", release_cid: "later-release" });
  approve("approved-exact-choice");
  await apply;
  assert.equal(f.posts.length, 2);
  assert.equal(f.posts[0].url, "/api/apps/system/summary");
  assert.equal(f.posts[0].method, "POST");
  assert.equal(f.posts[0].body, f.posts[1].body);
  assert.equal(f.approvals[0].operation, "system.update.apply");
  const intent = f.approvals[0].intent;
  assert.match(intent.request_id, /^[a-f0-9]{32}$/);
  assert.deepEqual(Object.keys(intent).sort(), ["action", "request_id", "source_name", "channel", "publisher_did", "current_version", "new_version", "head_cid", "release_cid"].sort());
  assert.deepEqual(JSON.parse(f.posts[0].body), { ...intent, step_up_token: "approved-exact-choice" });
  assert.equal(intent.release_cid, "release-exact");
  assert.equal(intent.new_version, "1.1.0");
  assert.equal(f.fields.get("update-new-version"), "1.1.0", "progress retains the approved choice");
  await f.context.onRuntimeUpdateApply();
  assert.equal(f.approvals.length, 1, "queued choice has one action");
  f.context.renderRuntimeUpdate({ ...f.offer, controller: { phase: "updated", id: "another-choice", message: "Other result" } });
  assert(f.context.runtimeUpdatePending, "another request result cannot retire this choice");
  f.context.renderRuntimeUpdate({ ...f.offer, available: false, controller: { phase: "updated", id: intent.request_id, message: "Home is up to date." } });
  assert.equal(f.context.runtimeUpdatePending, null);
  assert.equal(f.status.textContent, "Home is up to date.");
});

test("Home update keeps an ambiguous submission pending and stops bounded progress recovery", async () => {
  for (const status of [undefined, 503]) {
    const f = updateFixture();
    const bodies = [];
    f.context.fetchJson = async (_url, init) => {
      bodies.push(init.body);
      const error = new TypeError("connection closed"); error.status = status; throw error;
    };
    await f.context.onRuntimeUpdateApply();
    assert(f.context.runtimeUpdatePending);
    assert.equal(bodies.length, 2);
    assert.equal(bodies[0], bodies[1]);
    assert.equal(f.button.disabled, true);
    assert.match(f.status.textContent, /Reconnecting/);
    f.context.systemSummaryInFlight = Promise.resolve();
    const polls = f.context.runtimeUpdatePending.polls;
    await f.context.pollRuntimeUpdate();
    assert.equal(f.context.runtimeUpdatePending.polls, polls, "overlapping refresh does not consume recovery attempts");
    f.context.systemSummaryInFlight = null;
    f.context.runtimeUpdatePending.polls = 89;
    const before = f.timers.size;
    await f.context.pollRuntimeUpdate();
    assert.equal(f.timers.size, before, "exhausted recovery adds no timer");
    assert.match(f.status.textContent, /has not completed/);
  }
});

test("Home update cancellation and definitive refusal each allow a fresh approved choice", async () => {
  const f = updateFixture({ requestPasskeyStepUp: async () => { throw new Error("passkey cancelled"); } });
  await f.context.onRuntimeUpdateApply();
  assert.equal(f.posts.length, 0);
  assert.equal(f.context.runtimeUpdatePending, null);
  f.context.requestPasskeyStepUp = async (operation, intent) => { f.approvals.push({ operation, intent }); return "approved"; };
  f.context.fetchJson = async () => { const error = new Error("private failure detail"); error.status = 409; throw error; };
  await f.context.onRuntimeUpdateApply();
  const first = f.approvals[0].intent.request_id;
  assert.equal(f.context.runtimeUpdatePending, null);
  assert(!f.status.textContent.includes("private"));
  await f.context.onRuntimeUpdateApply();
  assert.notEqual(f.approvals[1].intent.request_id, first);
});

test("Home update polling binds host messages, stops at auth refusal, and closes with its document", async () => {
  const f = updateFixture();
  const message = { type: "elastos:runtime-events", schema: "elastos.home.runtime-events/v1", events: [] };
  const timer = f.context.runtimeUpdateTimer;
  f.listeners.get("message")({ source: {}, origin: "https://home.example", data: message });
  assert.equal(f.context.runtimeUpdateTimer, timer);
  f.listeners.get("message")({ source: f.top, origin: "https://foreign.example", data: message });
  assert.equal(f.context.runtimeUpdateTimer, timer);
  f.listeners.get("message")({ source: f.top, origin: "https://home.example", data: message });
  assert.equal(f.timers.get(f.context.runtimeUpdateTimer).delay, 0);
  f.context.renderRuntimeUpdate({ ...f.offer, can_apply: false, controller: { phase: "restarting" } });
  assert.equal(f.timers.get(f.context.runtimeUpdateTimer).delay, 2000);
  f.context.refreshSystemSummary = async () => { const error = new Error("expired"); error.status = 403; throw error; };
  f.timers.clear();
  await f.context.pollRuntimeUpdate();
  assert.equal(f.timers.size, 0);
  assert.match(f.status.textContent, /access has changed/);
  assert.equal(f.button.hidden, true);
  f.listeners.get("pagehide")();
  f.context.scheduleRuntimeUpdateRefresh();
  assert.equal(f.timers.size, 0);
});

test("Home update summary refreshes share one bounded request", async () => {
  const f = updateFixture();
  let finish, reads = 0;
  f.context.fetchJson = () => { reads++; return new Promise(resolve => { finish = resolve; }); };
  f.context.renderSystemSummary = value => { f.context.renderRuntimeUpdate(value.runtime_update); };
  const refresh = source.slice(source.indexOf("async function refreshSystemSummary() {"), source.indexOf("async function fetchJson("));
  vm.runInContext(refresh, f.context);
  const first = f.context.refreshSystemSummary();
  const second = f.context.refreshSystemSummary();
  assert.equal(reads, 1);
  assert([...f.timers.values()].some(timer => timer.delay === 10_000));
  finish({ runtime_update: f.offer });
  await Promise.all([first, second]);
  assert.equal(f.context.systemSummaryInFlight, null);
  assert(![...f.timers.values()].some(timer => timer.delay === 10_000));
  f.context.fetchJson = (_url, init) => new Promise((_resolve, reject) => {
    init.signal.addEventListener("abort", () => reject(new Error("request deadline")));
  });
  const hung = f.context.refreshSystemSummary();
  const deadline = [...f.timers.values()].find(timer => timer.delay === 10_000);
  deadline.handler();
  await assert.rejects(hung, /request deadline/);
  assert.equal(f.context.systemSummaryInFlight, null, "timed-out request releases the next refresh");
  assert(![...f.timers.values()].some(timer => timer.delay === 10_000));
});

test("System startup and recovery stay ready while the background Carrier check is pending or offline", async () => {
  const boot = source.slice(source.indexOf("async function boot() {"), source.indexOf("function configureHomeRecoverySave() {"));
  for (const checking of [true, false]) {
    const calls = [];
    const top = {};
    const context = vm.createContext({
      hasShellAccess: () => true,
      requestedSettingsTab: "account", initialRecoveryState: null,
      window: { parent: top, top }, homeClipboard: { start: () => false },
      refreshSystemSummary: async () => {
        calls.push(checking ? "summary-checking" : "summary-offline");
      },
      refreshActiveShell: async () => calls.push("shell"),
      refreshAccountList: async () => calls.push("accounts"),
      refreshRecoveryStatus: async () => calls.push("recovery"),
      refreshChainNetworks: async () => calls.push("chains"),
      refreshCapsuleCatalog: async () => calls.push("catalogue"),
    });
    for (const name of ["configureSettingsTabs", "configureSettingsSearch", "activateSettingsTab",
      "configureAppearanceEditor", "configureAppearancePreferences", "configureGuestAccess",
      "configureAiProvider", "configurePasskeyAccess", "configureRecoveryAccess", "configureChainAccess",
      "configureActiveShell", "configureCapsuleCatalog", "configureTechnicalDetails", "configureDeviceDidCopy",
      "configureRuntimeUpdate", "configureHomeRecoverySave"]) context[name] = () => {};
    vm.runInContext(boot, context);
    await context.boot();
    await context.initialRecoveryState;
    assert.deepEqual(calls, [checking ? "summary-checking" : "summary-offline",
      "shell", "accounts", "recovery", "chains", "catalogue"]);
  }
});
