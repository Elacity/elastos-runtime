import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import test from "node:test";
import { createLibraryActions } from "../capsules/library/browser/src/actions.js";
import { createLibraryRuntime } from "../capsules/library/browser/src/api.js";

const host = readFileSync(new URL("../capsules/home/browser/home-shell-host.js", import.meta.url), "utf8");
function fn(name) {
  const start = host.search(new RegExp(`(?:async )?function ${name}\\(`));
  assert(start >= 0, name);
  return host.slice(start, host.indexOf("\n}", start) + 2);
}
function fixture() {
  const messages = [];
  const source = (name) => ({ postMessage: (data) => messages.push({ name, data }) });
  const opener = { targetId: "browser", source: source("opener"), origin: "null" };
  const other = { targetId: "browser", source: source("other"), origin: "null" };
  const record = { id: "picker-1", requestId: "chooser-1", documentNonce: "document-1", openerToken: "opener", pickerToken: "picker", opener, source: opener.source, phase: "ready" };
  opener.pickerRequest = record;
  const picker = { targetId: "library", source: source("picker"), origin: "null", pickerOwner: record };
  const contexts = new Map([["opener", opener], ["other", other], ["picker", picker]]);
  const timers = new Map();
  const sandbox = vm.createContext({
    launchedAppContexts: contexts, requireHomeGuiActive() {},
    window: { setTimeout: (f) => { timers.set(f, f); return f; }, clearTimeout: (f) => timers.delete(f) },
    OPAQUE_FRAME_TARGET: "*", crypto: { randomUUID: () => "delivery-1" },
    openTargetFromHomeGui: async (target, options) => messages.push({ name: "launch", target, options }),
  });
  for (const name of ["currentLibraryPicker", "settleLibraryPicker", "cancelLibraryPicker", "validPickerIdentity", "openLibraryPicker", "acceptLibraryPicker", "deliverMessageToHomeGuiTargetFrame"]) {
    if (host.includes(`function ${name}(`)) vm.runInContext(fn(name), sandbox);
  }
  return { sandbox, contexts, opener, other, record, picker, messages, timers,
    context: { kind: "app-frame", targetId: "library", homeToken: "picker", source: picker.source } };
}

test("picker delivers to its opener, never the newest matching Browser window", async () => {
  const f = fixture();
  const pending = f.sandbox.deliverMessageToHomeGuiTargetFrame("browser", { type: "browser:file-picker-selection" }, f.context, "picker-1");
  assert.equal(f.messages[0]?.name, "opener");
  f.sandbox.acceptLibraryPicker({ homeToken: "opener", source: f.opener.source }, {
    pickerId: "picker-1", requestId: "chooser-1", documentNonce: "document-1", deliveryId: "delivery-1", accepted: true,
  });
  assert.equal(await pending, true);
});

const ack = (changes = {}) => ({ pickerId: "picker-1", requestId: "chooser-1", documentNonce: "document-1", deliveryId: "delivery-1", accepted: true, ...changes });
for (const [name, changes] of Object.entries({ wrong_picker: { pickerId: "other" }, wrong_request: { requestId: "other" }, wrong_document: { documentNonce: "old" }, wrong_delivery: { deliveryId: "old" }, malformed_acceptance: { accepted: "yes" } })) {
  test(`Home rejects ${name} acknowledgement`, async () => {
    const f = fixture();
    const pending = f.sandbox.deliverMessageToHomeGuiTargetFrame("browser", { type: "browser:file-picker-selection" }, f.context, "picker-1");
    assert.equal(f.sandbox.acceptLibraryPicker({ homeToken: "opener", source: f.opener.source }, ack(changes)), false);
    assert.equal(f.record.phase, "delivering");
    f.sandbox.settleLibraryPicker(f.record, false);
    assert.equal(await pending, false);
  });
}

test("wrong source, newer window and duplicate acknowledgements have no authority", async () => {
  const f = fixture();
  const pending = f.sandbox.deliverMessageToHomeGuiTargetFrame("browser", { type: "browser:file-picker-selection" }, f.context, "picker-1");
  assert.equal(f.sandbox.acceptLibraryPicker({ homeToken: "opener", source: f.other.source }, ack()), false);
  assert.equal(f.sandbox.acceptLibraryPicker({ homeToken: "other", source: f.other.source }, ack()), false);
  assert.equal(await f.sandbox.deliverMessageToHomeGuiTargetFrame("browser", { type: "browser:file-picker-selection" }, f.context, "picker-1"), false);
  assert.equal(f.messages.length, 1);
  assert.equal(f.sandbox.acceptLibraryPicker({ homeToken: "opener", source: f.opener.source }, ack()), true);
  assert.equal(await pending, true);
  assert.equal(f.sandbox.acceptLibraryPicker({ homeToken: "opener", source: f.opener.source }, ack()), false);
});

for (const retire of ["opener", "picker"]) {
  test(`${retire} document retirement cancels in-flight delivery`, async () => {
    const f = fixture();
    const pending = f.sandbox.deliverMessageToHomeGuiTargetFrame("browser", { type: "browser:file-picker-selection" }, f.context, "picker-1");
    f.sandbox.cancelLibraryPicker(f[retire]);
    assert.equal(await pending, false);
    assert.equal(f.sandbox.acceptLibraryPicker({ homeToken: "opener", source: f.opener.source }, ack()), false);
  });
}

test("a replaced opener or picker cannot settle an old request", async () => {
  for (const token of ["opener", "picker"]) {
    const f = fixture();
    const pending = f.sandbox.deliverMessageToHomeGuiTargetFrame("browser", { type: "browser:file-picker-selection" }, f.context, "picker-1");
    f.contexts.set(token, { ...f.contexts.get(token), pickerOwner: null });
    assert.equal(f.sandbox.acceptLibraryPicker({ homeToken: "opener", source: f.opener.source }, ack()), token === "picker");
    f.sandbox.settleLibraryPicker(f.record, false);
    assert.equal(await pending, false);
  }
});

test("two Browser openers retain independent picker requests; same-opener requests supersede", async () => {
  const f = fixture();
  const request = (id) => ({ requestId: id, documentNonce: "document-2", query: { mode: "attach", returnTarget: "browser" } });
  await f.sandbox.openLibraryPicker({ targetId: "browser", homeToken: "other", source: f.other.source }, request("chooser:2"));
  assert.equal(f.opener.pickerRequest, f.record);
  const second = f.other.pickerRequest;
  await f.sandbox.openLibraryPicker({ targetId: "browser", homeToken: "other", source: f.other.source }, request("chooser:3"));
  assert.equal(second.phase, "retired");
  assert.equal(f.opener.pickerRequest.phase, "ready");
  assert.equal(f.messages.filter((item) => item.name === "launch").length, 2);
  await f.sandbox.openLibraryPicker({ targetId: "browser", homeToken: "other", source: f.other.source }, request("chooser:3"));
  assert.equal(f.messages.length, 2, "duplicate open does not create a second picker");
});

test("unbound Library and wrong payload never fall back to a matching window", async () => {
  const f = fixture();
  for (const [target, payload, id] of [["browser", { type: "wrong" }, "picker-1"], ["chat-room", { type: "chat-room:attach-library-item" }, "picker-1"], ["browser", { type: "browser:file-picker-selection" }, "old"]]) {
    assert.equal(await f.sandbox.deliverMessageToHomeGuiTargetFrame(target, payload, f.context, id), false);
  }
  f.picker.pickerOwner = null;
  assert.equal(await f.sandbox.deliverMessageToHomeGuiTargetFrame("browser", { type: "browser:file-picker-selection" }, f.context, "picker-1"), false);
  assert.equal(f.messages.length, 0);
});

test("delivery timeout denies acceptance and late acknowledgement stays terminal", async () => {
  const f = fixture();
  const pending = f.sandbox.deliverMessageToHomeGuiTargetFrame("browser", { type: "browser:file-picker-selection" }, f.context, "picker-1");
  [...f.timers.values()][0]();
  assert.equal(await pending, false);
  assert.equal(f.sandbox.acceptLibraryPicker({ homeToken: "opener", source: f.opener.source }, ack()), false);
});

test("Library API waits for its exact Home response and rejects sibling or stale replies", async () => {
  const previous = globalThis.window;
  const messages = [];
  const listeners = new Map();
  const top = { postMessage: (message) => messages.push(message) };
  globalThis.window = { top, location: { search: "?home_origin=https://home.test&pickerRequestId=picker-1" },
    addEventListener: (type, listener) => listeners.set(type, listener), removeEventListener: (type) => listeners.delete(type),
    setTimeout: () => 1, clearTimeout() {} };
  try {
    const api = createLibraryRuntime({ getHomeToken: () => "picker-token" });
    let settled = false;
    const pending = api.deliverToTarget("browser", { type: "browser:file-picker-selection" }).then((result) => { settled = true; return result; });
    assert.equal(await api.deliverToTarget("browser", {}), false);
    const receive = listeners.get("message");
    const data = { type: "home:shell-response", requestId: messages[0].requestId, result: { accepted: true } };
    receive({ source: {}, origin: "https://home.test", data });
    receive({ source: top, origin: "null", data });
    receive({ source: top, origin: "https://home.test", data: { ...data, requestId: "old" } });
    await Promise.resolve();
    assert.equal(settled, false);
    receive({ source: top, origin: "https://home.test", data });
    assert.equal(await pending, true);
    assert.equal(listeners.size, 0);
  } finally { globalThis.window = previous; }
});

test("fresh Runtime picker launch binds only the live opener request", async () => {
  for (const stale of [false, true]) {
    const f = fixture();
    delete f.record.pickerToken;
    let finishLaunch;
    let issued;
    Object.assign(f.sandbox, {
      window: { ...f.sandbox.window, location: { origin: "https://home.test" } },
      fetchJson: async (url, options) => {
        assert.equal(url, "/api/apps/home/launch");
        issued = JSON.parse(options.body);
        return new Promise((resolve) => { finishLaunch = resolve; });
      },
      rememberLaunchedAppContext: () => f.contexts.set("fresh-picker", { targetId: "library" }),
      homeLaunchTokenFromRoute: () => "fresh-picker",
      retireLaunchedAppContext: (token) => f.contexts.delete(token),
    });
    vm.runInContext(fn("launchHomeTarget"), f.sandbox);
    const launch = f.sandbox.launchHomeTarget("library", { mode: "attach", returnTarget: "browser", pickerRequestId: "picker-1" });
    assert.deepEqual(issued, { target: "library", query: { mode: "attach", returnTarget: "browser", pickerRequestId: "picker-1", home_origin: "https://home.test" } });
    if (stale) f.sandbox.cancelLibraryPicker(f.opener);
    finishLaunch({ target: "library", attach_kind: "iframe", route: "fixture" });
    if (stale) {
      await assert.rejects(launch, /no longer available/);
      assert(!f.contexts.has("fresh-picker"));
    } else {
      await launch;
      assert.equal(f.contexts.get("fresh-picker").pickerOwner, f.record);
      assert.equal(f.record.pickerToken, "fresh-picker");
    }
  }
});

function archiveFixture() {
  const source = readFileSync(new URL("../capsules/archive-manager/browser/index.html", import.meta.url), "utf8");
  const messages = [];
  const opens = [];
  const menus = [];
  const top = { postMessage: (data) => messages.push(data) };
  const parent = {};
  const request = { id: "archive-request", consumed: false };
  const context = vm.createContext({
    window: { top, parent }, homeParentOrigin: "https://home.test", homeToken: "archive-token",
    libraryPickerDocumentNonce: "archive-document", libraryPickerRequest: request,
    openLibraryObject: async (object) => { opens.push(object); },
    handleHomeMenuCommand: (command) => menus.push(command), state: {}, renderEntries() {},
  });
  for (const name of ["isTrustedHomeMessage", "acceptLibraryPickerObject", "handleTrustedHomeMessage"]) {
    const start = source.search(new RegExp(`(?:async )?function ${name}\\(`));
    vm.runInContext(source.slice(start, source.indexOf("\n    }", start) + 6), context);
  }
  const data = { type: "archive:open-library-object", pickerId: "picker-archive", requestId: request.id, documentNonce: "archive-document", deliveryId: "delivery", object: { uri: "localhost://fixture/exact.zip" } };
  return { context, top, parent, data, messages, opens, menus };
}

test("Archive accepts only the exact top-Home picker and retains separate parent menus", async () => {
  const f = archiveFixture();
  for (const event of [{ source: {}, origin: "https://home.test" }, { source: f.parent, origin: "null" }, { source: f.top, origin: "https://wrong.test" }]) {
    f.context.handleTrustedHomeMessage({ ...event, data: f.data });
  }
  assert.equal(f.opens.length, 0);
  f.context.handleTrustedHomeMessage({ source: f.parent, origin: "null", data: { type: "elastos:menu-command", cmd: "open-archive" } });
  assert.deepEqual(f.menus, ["open-archive"]);
  f.context.handleTrustedHomeMessage({ source: f.top, origin: "https://home.test", data: f.data });
  await new Promise(setImmediate);
  assert.deepEqual(f.opens, [f.data.object]);
  assert.equal(f.messages[0].accepted, true);
});

test("Archive rejects old documents, duplicate deliveries and failed opens", async () => {
  for (const mode of ["document", "request", "failure", "duplicate", "stale-open"]) {
    const f = archiveFixture();
    if (mode === "document") f.data.documentNonce = "old";
    if (mode === "request") f.data.requestId = "old";
    if (mode === "failure") f.context.openLibraryObject = async () => { throw new Error("fixture"); };
    if (mode === "stale-open") f.context.openLibraryObject = async () => false;
    if (mode === "duplicate") assert.equal(await f.context.acceptLibraryPickerObject(f.data), true);
    assert.equal(await f.context.acceptLibraryPickerObject(f.data), false);
    assert.equal(f.opens.length, mode === "duplicate" ? 1 : 0);
  }
});

test("Library stays open until the receiver accepts", async () => {
  let accept;
  const delivery = new Promise((resolve) => { accept = resolve; });
  let closes = 0;
  const actions = createLibraryActions({
    state: { returnTarget: "browser" }, setStatus() {},
    downloadObjectRaw: async () => ({ blob: new Blob(["file"]) }),
    deliverToTarget: () => delivery, closeSelf: () => { closes += 1; },
  });
  const oldWindow = globalThis.window;
  globalThis.window = { setTimeout: (fn) => fn() };
  try {
    const pending = actions.attachObject({ uri: "localhost://fixture/file", name: "file" });
    await new Promise(setImmediate);
    assert.equal(closes, 0, "posting is not acceptance");
    accept(true);
    await pending;
    assert.equal(closes, 1);
  } finally { globalThis.window = oldWindow; }
});
