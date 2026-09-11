import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import { test } from "node:test";
import { createWalletRequests } from "../capsules/wallet/browser/wallet-requests.js";

const read = path => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
const windows = read("capsules/home-gui/browser/shell-windows.js");
function fn(source, name) {
  const start = source.search(new RegExp(`(?:async )?function ${name}\\(`));
  assert(start >= 0, name);
  const indent = source.slice(source.lastIndexOf("\n", start) + 1, start);
  return source.slice(start, source.indexOf(`\n${indent}}`, start) + indent.length + 2);
}
const tick = () => new Promise(setImmediate);
const queryFor = target => target === "inbox" ? { notification_id: "notification-two" } : { wallet_request: "wallet-request-two" };

function host(target, pending = false) {
  const messages = [], listeners = new Set(), timers = new Map();
  let serial = 0, launches = 0, release;
  const gate = pending ? new Promise(resolve => { release = resolve; }) : Promise.resolve();
  const frame = { dataset: { route: `/apps/${target}/#home_token=token` },
    contentWindow: { postMessage(data) { messages.push(data); } },
    addEventListener() {}, removeEventListener() {} };
  const entry = { id: target, targetId: target, draft: "intentional draft", node: { querySelector: () => frame } };
  const state = { windows: new Map(pending ? [] : [[target, entry]]) };
  const c = vm.createContext({ shellState: state, SYSTEM_APP_ID: "system", pendingWindowLaunches: new Map(),
    targetById: () => ({ window_policy: "single" }), topBrowserWindowEntryForTarget: () => state.windows.get(target),
    focusWindow() {}, browserLaunchAuthority: () => ({ homeToken: "token" }),
    async createBrowserTargetWindow() { launches++; await gate; state.windows.set(target, entry); return entry; },
    window: { crypto: { randomUUID: () => `request-${++serial}` },
      addEventListener: (_type, listener) => listeners.add(listener), removeEventListener: (_type, listener) => listeners.delete(listener),
      setTimeout: callback => { timers.set(serial, callback); return serial; }, clearTimeout: id => timers.delete(id) } });
  const functions = ["isSingleWindowTarget", "launchBrowserTargetWindow", "hasExactKeys",
    "requestSelectionWindow", "currentSelectionWindowFrame", "probeSelectionWindow", "handleSelectionWindowMessage", "finishSelectionWindowRequest"];
  vm.runInContext(functions.filter(name => windows.includes(`function ${name}(`)).map(name => fn(windows, name)).join("\n"), c);
  function event(data, overrides = {}) {
    for (const listener of [...listeners]) listener({ source: frame.contentWindow, origin: "null", data: { homeToken: "token", ...data }, ...overrides });
  }
  function ready(overrides = {}) {
    const probe = messages.at(-1);
    event({ type: `elastos.${target}.window-ready.result/v1`, requestId: probe.requestId, documentNonce: "document-one", ...overrides });
  }
  function result(overrides = {}) {
    const command = messages.at(-1);
    event({ type: `elastos.${target}.window.result/v1`, requestId: command.requestId, documentNonce: "document-one", ok: true, ...overrides });
  }
  return { c, entry, frame, state, messages, event, ready, result, timers, release, launches: () => launches };
}

for (const target of ["inbox", "wallet"]) {
  test(`${target}: already-open exact selection waits for current document acknowledgement`, async () => {
    const f = host(target); let settled = false;
    const p = f.c.launchBrowserTargetWindow(target, { query: queryFor(target) }).then(() => { settled = true; });
    await tick();
    assert.equal(f.messages.length, 1, "existing single window must receive a readiness probe");
    assert.equal(settled, false);
    const probe = f.messages[0];
    for (const overrides of [{ source: {} }, { origin: "https://wrong.example" }]) {
      f.event({ type: `elastos.${target}.window-ready.result/v1`, requestId: probe.requestId, documentNonce: "document-one" }, overrides);
    }
    f.ready({ homeToken: "wrong" });
    assert.equal(f.messages.length, 1);
    f.ready();
    const command = f.messages.at(-1);
    assert.equal(command.type, `elastos:${target}-chrome-command`);
    assert.deepEqual(JSON.parse(JSON.stringify(command.query)), queryFor(target));
    f.result({ documentNonce: "old-document" }); await tick(); assert.equal(settled, false);
    f.result(); await p;
    assert.equal(f.entry.draft, "intentional draft");
    assert.equal(f.launches(), 0);
    assert.equal(f.timers.size, 0);
  });
  test(`${target}: pending ordinary launch receives later selection once`, async () => {
    const f = host(target, true);
    const ordinary = f.c.launchBrowserTargetWindow(target);
    const selected = f.c.launchBrowserTargetWindow(target, { query: queryFor(target) });
    assert.equal(f.launches(), 1); f.release(); await ordinary; await tick();
    assert.equal(f.messages.length, 1, "pending-launch selection must not be lost");
    f.ready(); f.result(); await selected;
    const count = f.messages.length;
    await f.c.launchBrowserTargetWindow(target);
    assert.equal(f.messages.length, count, "ordinary Open preserves review and draft without navigation");
    assert.equal(f.launches(), 1);
  });
  test(`${target}: stale frame cannot acknowledge and unsupported query cannot select`, async () => {
    const f = host(target);
    await assert.rejects(f.c.launchBrowserTargetWindow(target, { query: { ...queryFor(target), approve: "true" } }));
    const p = f.c.launchBrowserTargetWindow(target, { query: queryFor(target) });
    await tick(); f.ready(); f.frame.dataset.route += "-replaced"; f.result();
    await assert.rejects(p);
    assert.equal(f.entry.draft, "intentional draft");
  });
}
test("People ordinary single reuse leaves the current profile draft alone", async () => {
  const f = host("people");
  await f.c.launchBrowserTargetWindow("people"); await f.c.launchBrowserTargetWindow("people");
  assert.equal(f.launches(), 0); assert.equal(f.messages.length, 0); assert.equal(f.entry.draft, "intentional draft");
  const people = read("capsules/people/browser/people.js");
  assert.deepEqual([...people.matchAll(/launchParams\.get\("([^"]+)"\)/g)].map(m => m[1]), ["home_origin"]);
  const input = { value: "edited name", placeholder: "" };
  const c = vm.createContext({ objectValue: value => value || {}, readText: value => String(value || ""),
    profileDraftDirty: true, profileDraftValue: "edited name", document: { activeElement: null }, profileInput: input,
    profileTitle: null, profileDescription: null, profileSubmit: null, profileForm: null });
  vm.runInContext(fn(people, "renderProfile"), c);
  c.renderProfile({ profile_readiness: { schema: "elastos.profile.readiness/v1", status: "missing" }, profile_setup_display_name: "suggestion" });
  assert.equal(input.value, "edited name");
  c.profileDraftValue = "";
  for (let i = 0; i < 2; i++) c.renderProfile({ profile_readiness: { schema: "elastos.profile.readiness/v1", status: "missing" }, profile_setup_display_name: "suggestion" });
  assert.equal(input.value, "", "unfocused deliberately blank draft survives repeated refresh");
  const pending = host("people", true);
  const first = pending.c.launchBrowserTargetWindow("people");
  const second = pending.c.launchBrowserTargetWindow("people");
  pending.release(); await Promise.all([first, second]);
  assert.equal(pending.launches(), 1); assert.equal(pending.entry.draft, "intentional draft");
});

function receiver(target) {
  const source = read(target === "inbox" ? "capsules/inbox/browser/index.html" : "capsules/wallet/browser/wallet.js");
  const listeners = new Map(), messages = [], calls = [], renders = [], statuses = [];
  let release, fail, clockOffset = 0;
  const summary = { notifications: { entries: [{ id: "notification-one" }, { id: "notification-two" }] },
    wallet_approvals: { approval_requests: [{ request_id: "wallet-request-two" }] } };
  const pending = new Promise((resolve, reject) => { release = () => resolve(summary); fail = reject; });
  const parent = { postMessage(data) { messages.push(data); } };
  const state = { homeToken: "token", refreshInFlight: null, entries: [], selectedId: "notification-one", requestedSelectionId: "" };
  const modal = { hidden: true, value: "intentional draft" };
  const refresh = { disabled: false };
  const c = vm.createContext({
    state, presentation: "window", elements: { refresh },
    activeHomeToken: "token", reviewWalletRequestId: "wallet-request-one",
    modalNode: modal, heroNode: null, accountsSectionNode: null,
    document: { querySelector: () => null },
    readQueryParam: () => "", Date: { now: () => Date.now() + clockOffset },
    api(path) { calls.push({ path, method: "GET" }); return pending; },
    fetchJson(path) { calls.push({ path, method: "GET" }); return pending; }, shellHeaders: () => ({}),
    refreshWalletState() { assert.fail("selection must not repaint during its read"); },
    setFilter(filter) { state.filter = filter; renders.push(state.requestedSelectionId); },
    renderWalletReviewRequests() { renders.push(vm.runInContext("reviewWalletRequestId", c)); },
    setStatus(text) { statuses.push(text); }, showStatus(text) { statuses.push(text); },
    window: { parent, top: {}, crypto: { randomUUID: () => "document-one" },
      addEventListener(type, listener) { listeners.set(type, listener); } },
  });
  const name = target === "inbox" ? "configureInboxWindowSelection" : "configureWalletWindowSelection";
  const dependencies = target === "inbox" ? fn(source, "hasExactKeys") : fn(source, "walletSelectionBlocked");
  vm.runInContext(dependencies + "\n" + fn(source, name), c); c[name]();
  const data = { type: `elastos:${target}-chrome-command`, cmd: target === "inbox" ? "select-notification" : "review-request",
    homeToken: "token", requestId: "request-1", documentNonce: "document-one", sequence: 1,
    expiresAt: Date.now() + 15000, query: queryFor(target) };
  return { c, messages, calls, renders, statuses, state, modal, refresh, release, fail, data,
    expire() { clockOffset += 15001; }, unload() { listeners.get("pagehide")(); },
    block() { if (target === "inbox") refresh.disabled = true; else modal.hidden = false; },
    event(data, overrides = {}) { return listeners.get("message")({ data, source: parent, origin: "null", ...overrides }); } };
}

for (const target of ["inbox", "wallet"]) {
  test(`${target}: actual host command selects exact review after read-only load and acknowledges`, async () => {
    const h = host(target), r = receiver(target);
    const p = h.c.launchBrowserTargetWindow(target, { query: queryFor(target) }); await tick();
    await r.event(h.messages[0]); h.event(r.messages.at(-1));
    const delivery = r.event(h.messages.at(-1)); await tick();
    assert.equal(r.renders.length, 0); assert.equal(r.calls.length, 1);
    r.release(); await delivery; h.event(r.messages.at(-1)); await p;
    assert.deepEqual(r.renders, [Object.values(queryFor(target))[0]]);
    assert(r.calls.every(call => call.method === "GET"), "selection does not approve, mark read or dismiss");
    assert.equal(r.modal.value, "intentional draft");
    const count = r.messages.length; await r.event(h.messages.at(-1));
    assert.equal(r.messages.length, count, "duplicate sequence is ignored");
  });
  for (const invalid of ["source", "origin", "token", "document", "query", "expired", "extra"]) {
    test(`${target}: denies ${invalid} selection without reading or changing state`, async () => {
      const r = receiver(target); const data = { ...r.data }; const overrides = {};
      if (invalid === "source") overrides.source = {};
      if (invalid === "origin") overrides.origin = "https://wrong.example";
      if (invalid === "token") data.homeToken = "wrong";
      if (invalid === "document") data.documentNonce = "retired-document";
      if (invalid === "query") data.query = { ...data.query, approve: true };
      if (invalid === "expired") data.expiresAt = Date.now() - 1;
      if (invalid === "extra") data.approve = true;
      await r.event(data, overrides);
      assert.equal(r.calls.length, 0); assert.equal(r.renders.length, 0);
      assert(!r.messages.some(m => m.ok === true));
    });
  }
  for (const boundary of ["dialog", "dialog-during-read", "unload", "token-change", "timeout", "failed-read"]) {
    test(`${target}: preserves review/draft across ${boundary}`, async () => {
      const r = receiver(target); if (boundary === "dialog") r.block();
      const p = r.event(r.data); await tick();
      if (boundary === "dialog-during-read") r.block();
      if (boundary === "unload") r.unload();
      if (boundary === "token-change") { r.state.homeToken = "changed"; r.c.activeHomeToken = "changed"; }
      if (boundary === "timeout") r.expire();
      if (boundary === "failed-read") r.fail(new Error("read failed")); else r.release();
      await p;
      assert.equal(r.renders.length, 0); assert.equal(r.modal.value, "intentional draft");
      assert(!r.messages.some(m => m.ok === true));
      if (boundary === "dialog" || boundary === "dialog-during-read") assert.match(r.statuses.at(-1), /Finish the current review/);
    });
  }
  test(`${target}: two pending selections are serialized on one current document`, async () => {
    const f = host(target, true);
    const first = f.c.launchBrowserTargetWindow(target, { query: queryFor(target) });
    const nextQuery = Object.fromEntries(Object.keys(queryFor(target)).map(key => [key, "next-request"]));
    const second = f.c.launchBrowserTargetWindow(target, { query: nextQuery });
    f.release(); await tick(); f.ready(); f.result(); await first; await tick();
    f.ready(); assert.deepEqual(JSON.parse(JSON.stringify(f.messages.at(-1).query)), nextQuery);
    assert.equal(f.messages.at(-1).sequence, 2); f.result(); await second;
    assert.equal(f.launches(), 1);
  });
}

test("Wallet selection cannot repaint a real approval started during its summary read", async () => {
  const r = receiver("wallet");
  const savedDocument = globalThis.document, savedWindow = globalThis.window;
  class Element {
    children = []; dataset = {}; disabled = false; hidden = false;
    classList = { toggle() {} };
    append(...nodes) { this.children.push(...nodes); }
    replaceChildren(...nodes) { this.children = nodes; }
    scrollIntoView() {}
    closest(selector) { return selector === "[data-wallet-request-managed-approve]" && this.dataset.walletRequestManagedApprove ? this : null; }
  }
  const tree = node => [node, ...node.children.flatMap(tree)];
  const requestsNode = new Element();
  let cancelPasskey;
  const passkey = new Promise((_resolve, reject) => { cancelPasskey = reject; });
  const effects = [];
  try {
    globalThis.document = { createElement: () => new Element() };
    globalThis.window = { setTimeout: fn => { fn(); } };
    const requests = createWalletRequests({
      requestsNode, fetchJson: (...args) => { effects.push(args); throw Error("unexpected effect"); },
      requestPasskeyStepUp: () => passkey, shellHeaders: () => ({}), showStatus() {},
      refreshWalletState: () => assert.fail("cancelled approval cannot refresh"), notifyHomeSummaryChanged() {}, openApprovalMethod() {},
    });
    const source = read("capsules/wallet/browser/wallet.js");
    const initial = { wallet_accounts: { accounts: [], default_accounts: [] }, wallet_approvals: {
      approval_requests: [{ request_id: "wallet-request-one", status: "pending", proof_type: "managed_evm", method: "personal_sign" }] } };
    Object.assign(r.c, { currentRequests: [], renderRequests: requests.renderRequests, pendingWalletRequests: requests.pendingWalletRequests,
      readText: value => String(value || ""), buildViewAccounts: () => [], loadPrices: async () => ({}), loadBalanceRows: async () => [],
      renderHero() {}, renderHeroAccount() {}, renderAccounts() {}, renderMethods() {}, renderActivity() {}, renderApprovalsBadge() {}, updateFlowButtons() {} });
    vm.runInContext(["loadWalletState", "renderAll", "renderWalletReviewRequests"].map(name => fn(source, name)).join("\n"), r.c);
    const heldFetch = r.c.fetchJson; r.c.fetchJson = async () => initial;
    await r.c.loadWalletState(); // Real startup load/render creates the original approval button.
    r.c.fetchJson = heldFetch;
    r.c.document.querySelector = () => tree(requestsNode).find(node => node.disabled) || null;
    const card = requestsNode.children[0];
    const button = tree(card).find(node => node.dataset.walletRequestManagedApprove);
    assert(button);
    const selection = r.event(r.data); await tick();
    const approval = requests.onRequestClick({ target: button }); await tick();
    assert.equal(button.disabled, true, "actual passkey approval holds its button busy");
    r.release(); await selection;
    assert.equal(requestsNode.children[0], card, "selection read never replaces the active review");
    assert.equal(button.disabled, true);
    assert.equal(vm.runInContext("reviewWalletRequestId", r.c), "wallet-request-one");
    assert.equal(r.messages.at(-1).ok, false);
    assert.match(r.statuses.at(-1), /Finish the current review/);
    assert.equal(effects.length, 0);
    cancelPasskey(Error("fixture cancelled")); await approval;
    assert.equal(button.disabled, false); assert.equal(effects.length, 0);
  } finally { globalThis.document = savedDocument; globalThis.window = savedWindow; }
});

test("Wallet unavailable selection stays exact through later renders", () => {
  const shown = [], statuses = [];
  const c = vm.createContext({ currentRequests: [{ request_id: "different", status: "pending" }], reviewWalletRequestId: "missing",
    pendingWalletRequests: values => values, readText: value => value,
    renderRequests(values, id) { shown.push({ ids: values.map(value => value.request_id), id }); return values.length > 0; },
    showStatus(text) { statuses.push(text); } });
  vm.runInContext(fn(read("capsules/wallet/browser/wallet.js"), "renderWalletReviewRequests"), c);
  c.renderWalletReviewRequests(); c.renderWalletReviewRequests();
  assert.deepEqual(JSON.parse(JSON.stringify(shown)), [{ ids: [], id: "missing" }, { ids: [], id: "missing" }]);
  assert.match(statuses.at(-1), /unavailable/);
  c.currentRequests.push({ request_id: "missing", status: "pending" }); c.renderWalletReviewRequests();
  assert.deepEqual(JSON.parse(JSON.stringify(shown.at(-1))), { ids: ["missing"], id: "missing" });
});

test("selection refusal keeps current-window feedback rather than app-start error UI", () => {
  const c = vm.createContext({ renderSystemErrorWindow() { assert.fail("must preserve the current app"); } });
  vm.runInContext(fn(windows, "renderTargetLaunchError"), c);
  c.renderTargetLaunchError("wallet", { selectionOnly: true });
});
