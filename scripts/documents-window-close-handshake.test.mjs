#!/usr/bin/env node

import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import vm from "node:vm";

const shellWindowsSource = fs.readFileSync(
  new URL("../capsules/home-gui/browser/shell-windows.js", import.meta.url),
  "utf8",
);
const documentsSource = fs.readFileSync(
  new URL("../capsules/documents/browser/index.html", import.meta.url),
  "utf8",
);

function extractFunction(source, name) {
  const markers = [`async function ${name}(`, `function ${name}(`];
  const start = markers
    .map((marker) => source.indexOf(marker))
    .filter((index) => index >= 0)
    .sort((left, right) => left - right)[0];
  assert.notEqual(start, undefined, `${name} function not found`);
  const parametersOpen = source.indexOf("(", start);
  let parameterDepth = 0;
  let parametersClose = -1;
  for (let index = parametersOpen; index < source.length; index += 1) {
    if (source[index] === "(") parameterDepth += 1;
    if (source[index] === ")") parameterDepth -= 1;
    if (parameterDepth === 0) {
      parametersClose = index;
      break;
    }
  }
  const open = source.indexOf("{", parametersClose);
  let depth = 0;
  for (let index = open; index < source.length; index += 1) {
    if (source[index] === "{") depth += 1;
    if (source[index] === "}") depth -= 1;
    if (depth === 0) return source.slice(start, index + 1);
  }
  throw new Error(`${name} function body is not balanced`);
}

function stringConstant(source, name) {
  const match = source.match(new RegExp(`const ${name} =\\s*"([^"]+)";`));
  assert.ok(match, `${name} constant not found`);
  return match[1];
}

function numericConstant(source, name) {
  const match = source.match(new RegExp(`const ${name} = ([0-9_]+);`));
  assert.ok(match, `${name} constant not found`);
  return Number(match[1].replaceAll("_", ""));
}

const shellHarnessSource = [
  "hasExactKeys",
  "documentsWindowCloseContext",
  "browserWindowCloseRequestId",
  "settleDocumentsWindowClose",
  "handleDocumentsWindowCloseResult",
  "requestDocumentsWindowClose",
  "closeWindow",
]
  .map((name) => extractFunction(shellWindowsSource, name))
  .join("\n");

const documentsHarnessSource = [
  "hasExactKeys",
  "clearAutosaveTimer",
  "scheduleAutosave",
  "chooseInCapsule",
  "confirmInCapsule",
  "closeConfirmModal",
  "sameHomeWindowCloseTarget",
  "suspendAutosaveForHomeWindowClose",
  "resumeAutosaveForHomeWindowClose",
  "requestHomeWindowCloseDecision",
  "postHomeWindowCloseResult",
  "handleHomeWindowCloseRequest",
]
  .map((name) => extractFunction(documentsSource, name))
  .join("\n");

const documentsSaveHarnessSource = [
  "clearAutosaveTimer",
  "scheduleAutosave",
  "documentIdentity",
  "isCurrentSessionTarget",
  "isCurrentDocumentTarget",
  "isLibraryFileProjection",
  "isDraftDocument",
  "chooseInCapsule",
  "closeConfirmModal",
  "sameHomeWindowCloseTarget",
  "suspendAutosaveForHomeWindowClose",
  "resumeAutosaveForHomeWindowClose",
  "applySavedDocumentState",
  "saveCurrent",
  "requestHomeWindowCloseDecision",
]
  .map((name) => extractFunction(documentsSource, name))
  .join("\n");

function createShellHarness() {
  const posts = [];
  const removed = [];
  const timers = new Map();
  let nextTimerId = 1;
  let nextRequestId = 0;
  const frameWindow = {
    postMessage(message, origin) {
      posts.push({ message, origin });
    },
  };
  const frame = {
    dataset: {
      route: "/apps/documents/?view=write#home_token=documents-close-token",
    },
    getAttribute(name) {
      return name === "src" ? this.dataset.route : "";
    },
    contentWindow: frameWindow,
  };
  const node = {
    querySelector(selector) {
      if (selector === ".window-frame") return frame;
      return null;
    },
    dataset: {},
  };
  const entry = {
    id: "documents-window-1",
    targetId: "documents",
    node,
  };
  const shellState = {
    windows: new Map([[entry.id, entry]]),
  };
  const context = vm.createContext({
    DOCUMENTS_WINDOW_CLOSE_TIMEOUT_MS: numericConstant(
      shellWindowsSource,
      "DOCUMENTS_WINDOW_CLOSE_TIMEOUT_MS",
    ),
    DOCUMENTS_WINDOW_CLOSE_REQUEST_TYPE: stringConstant(
      shellWindowsSource,
      "DOCUMENTS_WINDOW_CLOSE_REQUEST_TYPE",
    ),
    DOCUMENTS_WINDOW_CLOSE_RESULT_TYPE: stringConstant(
      shellWindowsSource,
      "DOCUMENTS_WINDOW_CLOSE_RESULT_TYPE",
    ),
    OPAQUE_CAPSULE_ORIGIN: "null",
    URL,
    URLSearchParams,
    pendingDocumentsWindowCloses: new Map(),
    shellState,
    removeWindowEntries(entries) {
      removed.push(entries.map((candidate) => candidate.id));
      for (const candidate of entries) {
        shellState.windows.delete(candidate.id);
      }
      return true;
    },
    window: {
      location: { href: "http://localhost:61380/apps/home/" },
      crypto: {
        randomUUID() {
          nextRequestId += 1;
          return `documents-close-request-${nextRequestId}`;
        },
      },
      setTimeout(callback, delay) {
        const id = nextTimerId++;
        timers.set(id, { callback, delay });
        return id;
      },
      clearTimeout(id) {
        timers.delete(id);
      },
    },
  });
  vm.runInContext(
    `${shellHarnessSource}\nthis.api = { closeWindow, handleDocumentsWindowCloseResult };`,
    context,
  );
  return {
    entry,
    posts,
    removed,
    shellState,
    frame,
    frameWindow,
    timerCount() {
      return timers.size;
    },
    closeWindow() {
      return context.api.closeWindow(entry.id);
    },
    deliver(message, options = {}) {
      return context.api.handleDocumentsWindowCloseResult({
        origin: options.origin || "null",
        source: options.source || frameWindow,
        data: message,
      });
    },
  };
}

function createDocumentsSaveHarness() {
  const timers = new Map();
  let nextTimerId = 1;
  let providerDeferred = deferredPromise();
  let providerCalls = 0;
  const state = {
    current: {
      doc_did: "doc-1",
      title: "Audit",
      body: "Original body",
      file_name: "audit.md",
      working_copy_uri: "localhost://ElastOS/Documents/doc-1",
      document_uri: "localhost://ElastOS/Documents/doc-1",
      latest_published_cid: null,
      publish_history: [],
    },
    currentSessionId: 1,
    mode: "shell",
    dirty: true,
    pendingConfirmation: null,
  };
  const elements = {
    titleInput: { value: "Audit" },
    editor: { value: "Original body" },
    confirmModal: {
      classList: { add() {}, remove() {} },
      setAttribute() {},
      querySelector() {
        return {
          setAttribute() {},
          removeAttribute() {},
        };
      },
    },
    confirmTitle: { textContent: "", hidden: false },
    confirmMessage: { textContent: "" },
    confirmCancel: { focus() {} },
    confirmSecondaryAction: { hidden: true, textContent: "" },
    confirmAction: {
      textContent: "",
      classList: { toggle() {}, remove() {} },
      focus() {},
    },
  };
  const context = vm.createContext({
    state,
    elements,
    autosaveTimerId: 0,
    autosaveQueued: false,
    pendingHomeWindowCloseTarget: null,
    saveInFlight: null,
    window: {
      setTimeout(callback, delay) {
        const id = nextTimerId++;
        timers.set(id, { callback, delay });
        return id;
      },
      clearTimeout(id) {
        timers.delete(id);
      },
    },
    reportSaveFailure() {},
    setStatus() {},
    scheduleStatusClear() {},
    clearStatus() {},
    upsertDocumentListItem() {},
    renderDocumentsList() {},
    renderCurrentDocument() {},
    refreshPreviewFromEditor() {},
    renderHistory() {},
    setDirty(value) {
      state.dirty = value === true;
    },
    documentsProviderApi(action) {
      assert.equal(action, "save");
      providerCalls += 1;
      return providerDeferred.promise;
    },
    libraryObjectApi() {
      throw new Error("unexpected library save");
    },
    utf8ToBase64(value) {
      return value;
    },
  });
  vm.runInContext(
    `${documentsSaveHarnessSource}\nthis.api = { scheduleAutosave, saveCurrent, requestHomeWindowCloseDecision, closeConfirmModal };`,
    context,
  );
  return {
    timers,
    elements,
    state,
    scheduleAutosave() {
      return context.api.scheduleAutosave();
    },
    saveCurrent(options) {
      return context.api.saveCurrent(options);
    },
    requestCloseDecision() {
      return context.api.requestHomeWindowCloseDecision({ sessionId: 1 });
    },
    respond(result) {
      context.api.closeConfirmModal(result);
    },
    resolveSave() {
      providerDeferred.resolve({
        document: {
          doc_did: "doc-1",
          title: "Audit",
          body: "Original body",
          file_name: "audit.md",
        },
      });
    },
    providerCalls: () => providerCalls,
    hasTimer(delay) {
      return [...timers.values()].some((timer) => timer.delay === delay);
    },
    fireTimerByDelay(delay) {
      const timer = [...timers.entries()].find(([, value]) => value.delay === delay);
      assert.ok(timer, `expected timer ${delay}`);
      timers.delete(timer[0]);
      return timer[1].callback();
    },
  };
}

function deferredPromise() {
  let resolve;
  let reject;
  const promise = new Promise((nextResolve, nextReject) => {
    resolve = nextResolve;
    reject = nextReject;
  });
  return { promise, resolve, reject };
}

function createDocumentsHarness({
  dirty = false,
  currentSessionId = 1,
  savePlan = "success",
  queuedAutosave = false,
} = {}) {
  const posted = [];
  const focused = [];
  const timers = new Map();
  let nextTimerId = 1;
  let reportedSaveFailure = 0;
  let saveCalls = 0;
  let saveInFlightControl = null;
  const state = {
    homeToken: "documents-close-token",
    current: { doc_did: "doc-1" },
    currentSessionId,
    mode: "shell",
    dirty,
    pendingConfirmation: null,
  };
  const elements = {
    confirmModal: {
      classList: { add() {}, remove() {} },
      setAttribute() {},
      querySelector() {
        return {
          setAttribute() {},
          removeAttribute() {},
        };
      },
    },
    confirmTitle: { textContent: "", hidden: false },
    confirmMessage: { textContent: "" },
    confirmCancel: { focus() { focused.push("cancel"); } },
    confirmSecondaryAction: { hidden: true, textContent: "" },
    confirmAction: {
      textContent: "",
      classList: { toggle() {}, remove() {} },
      focus() { focused.push("confirm"); },
    },
  };
  const parent = {
    postMessage(message, origin) {
      posted.push({ message, origin });
    },
  };
  const context = vm.createContext({
    DOCUMENTS_WINDOW_CLOSE_REQUEST_TYPE: stringConstant(
      documentsSource,
      "DOCUMENTS_WINDOW_CLOSE_REQUEST_TYPE",
    ),
    DOCUMENTS_WINDOW_CLOSE_RESULT_TYPE: stringConstant(
      documentsSource,
      "DOCUMENTS_WINDOW_CLOSE_RESULT_TYPE",
    ),
    state,
    elements,
    autosaveTimerId: 0,
    autosaveQueued: queuedAutosave,
    pendingHomeWindowCloseTarget: null,
    saveInFlight: null,
    reportSaveFailure() {
      reportedSaveFailure += 1;
    },
    saveCurrent: async () => {
      saveCalls += 1;
      if (savePlan === "failure") {
        throw new Error("save failed");
      }
      if (savePlan === "dirty_after_save") {
        state.dirty = true;
        return;
      }
      state.dirty = false;
    },
    window: {
      parent,
      setTimeout(callback, delay) {
        const id = nextTimerId++;
        timers.set(id, { callback, delay });
        return id;
      },
      clearTimeout(id) {
        timers.delete(id);
      },
    },
  });
  vm.runInContext(
    `${documentsHarnessSource}\nthis.api = { scheduleAutosave, closeConfirmModal, handleHomeWindowCloseRequest };`,
    context,
  );
  if (savePlan === "in_flight_success" || savePlan === "in_flight_failure") {
    saveInFlightControl = deferredPromise();
    context.saveInFlight = saveInFlightControl.promise;
  }
  return {
    focused,
    posted,
    timers,
    state,
    saveCalls: () => saveCalls,
    reportedSaveFailure: () => reportedSaveFailure,
    async request(requestId = "documents-close-request-1") {
      const pending = context.api.handleHomeWindowCloseRequest({
        origin: "null",
        source: parent,
        data: {
          type: stringConstant(
            documentsSource,
            "DOCUMENTS_WINDOW_CLOSE_REQUEST_TYPE",
          ),
          requestId,
          homeToken: state.homeToken,
        },
      });
      await Promise.resolve();
      return pending;
    },
    scheduleAutosave() {
      return context.api.scheduleAutosave();
    },
    respond(result) {
      context.api.closeConfirmModal(result);
    },
    fireTimerByDelay(delay) {
      const timer = [...timers.entries()].find(([, value]) => value.delay === delay);
      assert.ok(timer, `expected timer ${delay}`);
      timers.delete(timer[0]);
      return timer[1].callback();
    },
    resolveSaveInFlight() {
      assert.ok(saveInFlightControl, "missing in-flight save");
      state.dirty = false;
      saveInFlightControl.resolve();
    },
    rejectSaveInFlight() {
      assert.ok(saveInFlightControl, "missing in-flight save");
      saveInFlightControl.reject(new Error("save failed"));
    },
  };
}

test("Home keeps the Documents window until an accepted close request settles", async () => {
  const harness = createShellHarness();
  const close = harness.closeWindow();
  assert.equal(harness.posts.length, 1);
  assert.deepEqual(
    { ...harness.posts[0].message },
    {
      type: stringConstant(
        shellWindowsSource,
        "DOCUMENTS_WINDOW_CLOSE_REQUEST_TYPE",
      ),
      requestId: "documents-close-request-1",
      homeToken: "documents-close-token",
    },
  );
  assert.equal(harness.posts[0].origin, "*");
  assert.equal(harness.closeWindow(), close);
  harness.deliver(
    {
      type: stringConstant(
        shellWindowsSource,
        "DOCUMENTS_WINDOW_CLOSE_RESULT_TYPE",
      ),
      requestId: "documents-close-request-1",
      homeToken: "documents-close-token",
      state: "pending",
      ok: false,
      reason: "awaiting_decision",
      sessionId: 7,
    },
    { source: { postMessage() {} } },
  );
  assert.equal(harness.shellState.windows.has(harness.entry.id), true);
  assert.equal(harness.timerCount(), 1);
  harness.deliver({
    type: stringConstant(
      shellWindowsSource,
      "DOCUMENTS_WINDOW_CLOSE_RESULT_TYPE",
    ),
    requestId: "documents-close-request-1",
    homeToken: "documents-close-token",
    state: "pending",
    ok: false,
    reason: "awaiting_decision",
    sessionId: 7,
  });
  assert.equal(harness.timerCount(), 0);
  harness.deliver({
    type: stringConstant(
      shellWindowsSource,
      "DOCUMENTS_WINDOW_CLOSE_RESULT_TYPE",
    ),
    requestId: "documents-close-request-1",
    homeToken: "documents-close-token",
    state: "terminal",
    ok: true,
    reason: "",
    sessionId: 8,
  });
  assert.equal(harness.shellState.windows.has(harness.entry.id), true);
  harness.deliver({
    type: stringConstant(
      shellWindowsSource,
      "DOCUMENTS_WINDOW_CLOSE_RESULT_TYPE",
    ),
    requestId: "documents-close-request-1",
    homeToken: "documents-close-token",
    state: "terminal",
    ok: true,
    reason: "",
    sessionId: 7,
  });
  assert.equal(await close, true);
  assert.equal(harness.shellState.windows.has(harness.entry.id), false);
  assert.equal(harness.removed.length, 1);
  assert.equal(harness.removed[0].length, 1);
  assert.equal(harness.removed[0][0], harness.entry.id);
});

test("Home keeps the Documents window open when the request resolves on a stale frame or retokened route", async () => {
  const harness = createShellHarness();
  const close = harness.closeWindow();
  harness.deliver({
    type: stringConstant(
      shellWindowsSource,
      "DOCUMENTS_WINDOW_CLOSE_RESULT_TYPE",
    ),
    requestId: "documents-close-request-1",
    homeToken: "documents-close-token",
    state: "pending",
    ok: false,
    reason: "awaiting_decision",
    sessionId: 3,
  });
  harness.frame.dataset.route = "/apps/documents/?view=write#home_token=documents-close-token-2";
  harness.deliver({
    type: stringConstant(
      shellWindowsSource,
      "DOCUMENTS_WINDOW_CLOSE_RESULT_TYPE",
    ),
    requestId: "documents-close-request-1",
    homeToken: "documents-close-token",
    state: "terminal",
    ok: true,
    reason: "",
    sessionId: 3,
  });
  assert.equal(await close, false);
  assert.equal(harness.shellState.windows.has(harness.entry.id), true);
});

test("Home keeps the Documents window open when the capsule cancels closing", async () => {
  const harness = createShellHarness();
  const close = harness.closeWindow();
  harness.deliver({
    type: stringConstant(
      shellWindowsSource,
      "DOCUMENTS_WINDOW_CLOSE_RESULT_TYPE",
    ),
    requestId: "documents-close-request-1",
    homeToken: "documents-close-token",
    state: "pending",
    ok: false,
    reason: "awaiting_decision",
    sessionId: 2,
  });
  harness.deliver({
    type: stringConstant(
      shellWindowsSource,
      "DOCUMENTS_WINDOW_CLOSE_RESULT_TYPE",
    ),
    requestId: "documents-close-request-1",
    homeToken: "documents-close-token",
    state: "terminal",
    ok: false,
    reason: "cancelled",
    sessionId: 2,
  });
  assert.equal(await close, false);
  assert.equal(harness.shellState.windows.has(harness.entry.id), true);
});

test("dirty Documents discard stays pending beyond the autosave window and then discards", async () => {
  const harness = createDocumentsHarness({ dirty: true });
  harness.scheduleAutosave();
  assert.equal(
    [...harness.timers.values()].some((timer) => timer.delay === 900),
    true,
  );
  const pending = harness.request();
  assert.equal(
    [...harness.timers.values()].some((timer) => timer.delay === 900),
    false,
  );
  assert.deepEqual(
    { ...harness.posted[0].message },
    {
      type: stringConstant(
        documentsSource,
        "DOCUMENTS_WINDOW_CLOSE_RESULT_TYPE",
      ),
      requestId: "documents-close-request-1",
      homeToken: "documents-close-token",
      state: "pending",
      ok: false,
      reason: "awaiting_decision",
      sessionId: 1,
    },
  );
  harness.fireTimerByDelay(10);
  assert.deepEqual(harness.focused, ["cancel"]);
  harness.respond("secondary");
  await pending;
  assert.equal(harness.saveCalls(), 0);
  assert.equal(harness.posted.at(-1).message.ok, true);
  assert.equal(harness.posted.at(-1).message.reason, "discarded");
});

test("dirty Documents cancel restores the exact autosave timer", async () => {
  const harness = createDocumentsHarness({ dirty: true });
  harness.scheduleAutosave();
  const pending = harness.request();
  harness.respond("cancel");
  await pending;
  assert.equal(harness.posted.at(-1).message.ok, false);
  assert.equal(harness.posted.at(-1).message.reason, "cancelled");
  assert.equal(
    [...harness.timers.values()].some((timer) => timer.delay === 900),
    true,
  );
});

test("dirty Documents close saves before closing", async () => {
  const harness = createDocumentsHarness({ dirty: true });
  const pending = harness.request();
  harness.respond("confirm");
  await pending;
  assert.equal(harness.saveCalls(), 1);
  assert.equal(harness.posted.at(-1).message.ok, true);
  assert.equal(harness.posted.at(-1).message.reason, "");
});

test("dirty Documents close keeps the window open when save fails or remains dirty", async () => {
  const failureHarness = createDocumentsHarness({ dirty: true, savePlan: "failure" });
  const failed = failureHarness.request();
  failureHarness.respond("confirm");
  await failed;
  assert.equal(failureHarness.saveCalls(), 1);
  assert.equal(failureHarness.reportedSaveFailure(), 1);
  assert.equal(failureHarness.posted.at(-1).message.ok, false);
  assert.equal(failureHarness.posted.at(-1).message.reason, "save_failed");

  const incompleteHarness = createDocumentsHarness({
    dirty: true,
    savePlan: "dirty_after_save",
  });
  const incomplete = incompleteHarness.request();
  incompleteHarness.respond("confirm");
  await incomplete;
  assert.equal(incompleteHarness.posted.at(-1).message.ok, false);
  assert.equal(incompleteHarness.posted.at(-1).message.reason, "save_incomplete");
});

test("in-flight saves settle before close and stale document changes fail closed", async () => {
  const saveHarness = createDocumentsHarness({
    dirty: true,
    savePlan: "in_flight_success",
    queuedAutosave: true,
  });
  const waiting = saveHarness.request();
  assert.equal(saveHarness.focused.length, 0);
  assert.equal(
    [...saveHarness.timers.values()].some((timer) => timer.delay === 900),
    false,
  );
  saveHarness.resolveSaveInFlight();
  await waiting;
  assert.equal(saveHarness.saveCalls(), 0);
  assert.equal(saveHarness.posted.at(-1).message.ok, true);

  const staleAfterSaveHarness = createDocumentsHarness({
    dirty: true,
    savePlan: "in_flight_success",
  });
  const staleAfterSave = staleAfterSaveHarness.request();
  staleAfterSaveHarness.state.currentSessionId = 2;
  staleAfterSaveHarness.resolveSaveInFlight();
  await staleAfterSave;
  assert.equal(staleAfterSaveHarness.posted.at(-1).message.ok, false);
  assert.equal(staleAfterSaveHarness.posted.at(-1).message.reason, "stale_request");

  const staleHarness = createDocumentsHarness({ dirty: true });
  const stale = staleHarness.request();
  staleHarness.state.currentSessionId = 2;
  staleHarness.respond("secondary");
  await stale;
  assert.equal(staleHarness.posted.at(-1).message.ok, false);
  assert.equal(staleHarness.posted.at(-1).message.reason, "stale_request");
});

test("real save requeue paths stay suspended while the close decision is open", async () => {
  const harness = createDocumentsSaveHarness();
  harness.scheduleAutosave();
  assert.equal(harness.hasTimer(900), true);

  const firstSave = harness.saveCurrent({ quiet: true, autosave: true });
  void harness.saveCurrent({ quiet: true, autosave: true });
  assert.equal(harness.providerCalls(), 1);

  harness.elements.editor.value = "Changed while saving";
  harness.state.dirty = true;
  const decision = harness.requestCloseDecision();
  assert.equal(harness.hasTimer(900), false);

  harness.resolveSave();
  await firstSave;
  await Promise.resolve();
  assert.equal(harness.hasTimer(900), false);

  harness.respond("cancel");
  await decision;
  assert.equal(harness.hasTimer(900), true);
});
