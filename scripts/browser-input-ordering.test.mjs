import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { setImmediate as nextTurn } from "node:timers/promises";
import test from "node:test";
import vm from "node:vm";
import { runtimePageOwner, sameRuntimePageOwner } from "../capsules/browser/browser/browser-page-cleanup.js";
import { selkiesMessagesForInput } from "../capsules/browser/browser/browser-input.js";
import { createBrowserClipboardBridge } from "../capsules/browser/browser/browser-clipboard.js";

// An explicit ref runs the unchanged behavioral gate as a negative control.
const source = process.env.BROWSER_INPUT_ORDERING_SOURCE_REF
  ? execFileSync("git", ["show", `${process.env.BROWSER_INPUT_ORDERING_SOURCE_REF}:capsules/browser/browser/browser.js`],
    { cwd: new URL("../", import.meta.url), encoding: "utf8" })
  : readFileSync(new URL("../capsules/browser/browser/browser.js", import.meta.url), "utf8");
const sourceHash = createHash("sha256").update(source).digest("hex");
function declaration(name) {
  const start = source.search(new RegExp(`(?:async )?function ${name}\\(`));
  const end = source.slice(start).search(/\n\}(?=\n|$)/);
  assert.ok(start >= 0 && end > 0, `missing actual function ${name}`);
  return source.slice(start, start + end + 2);
}
function deferred() {
  let resolve, reject;
  const promise = new Promise((ok, fail) => { resolve = ok; reject = fail; });
  return { promise, resolve, reject };
}
const text = value => ({ type: "paste_text", text: value });
const plain = value => JSON.parse(JSON.stringify(value));
function assertCanceled(results) {
  for (const result of results) {
    assert.equal(result.status, "rejected", "current-owner dependent input must report cancellation");
    assert.equal(result.error.message, "Browser input was canceled after a failed operation.");
  }
}

function harness() {
  const requests = [], sends = [], mutations = [], failures = [], effects = [];
  const state = vm.createContext({
    currentPage: { page_id: "page:original", title: "Original", actual_url: "https://fixture.invalid/nav?run=ordering",
      runtime_cleanup: { schema: "elastos.browser.cleanup-handle/v1", id: "cleanup:original" },
      display_session: { input: "datachannel", input_protocol: "selkies_v1" } },
    currentPageGeneration: 1, currentDisplayMode: "webrtc_remote_display", currentView: { width: 1280, height: 720 },
    libraryPickerRequest: null, runtimePageOwner, sameRuntimePageOwner, selkiesMessagesForInput,
    PAGE_STATUS_AFTER_INPUT_DELAY_MS: 1, PAGE_STATUS_AFTER_SCROLL_DELAY_MS: 1,
    remoteDisplay: {
      inputChannelOpen: () => true,
      // This observes synchronous dispatch, not a guest-side data-channel ack.
      sendInputMessages: messages => sends.push(...plain(messages)),
    },
    fetchJson: (path, options) => {
      const wait = deferred(), event = plain(options.body.event);
      const request = { path, event,
        ack: (response = { accepted: true }) => { effects.push(event.text ?? event.type); wait.resolve(response); },
        reject: wait.reject };
      requests.push(request);
      return wait.promise;
    },
    recoverMissingRuntimePage: error => { failures.push(error); return false; },
    handleFileChooserFromStatus: response => mutations.push(["chooser", response]),
    syncViewFromResponse: response => mutations.push(["view", response]),
    syncBrowserLocation: (...args) => mutations.push(["location", ...args]),
    focusRemoteInput: () => mutations.push(["focus"]),
    schedulePageStatusRefresh: options => mutations.push(["refresh", options]),
  });
  const names = ["currentRuntimePageOwner", "currentInputTransport", "currentInputProtocol", "encodeDatachannelInput", "sendBrowserInput"];
  // The old implementation remains executable so the same behavioral tests can
  // record a red baseline, rather than failing merely on a missing new symbol.
  if (source.includes("function dispatchBrowserInput(")) names.push("dispatchBrowserInput");
  const queue = source.match(/^let browserInputQueue = [^\n]+;$/m)?.[0] || "";
  const cap = source.match(/^const MAX_PENDING_BROWSER_INPUTS = [0-9_]+;$/m)?.[0] || "";
  vm.runInContext(queue + "\n" + cap + "\n" + names.map(declaration).join("\n"), state);
  const submit = (event, options) => Promise.resolve(state.sendBrowserInput(event, options))
    .then(value => ({ status: "fulfilled", value }), error => ({ status: "rejected", error }));
  const changeOwner = kind => {
    state.currentPage = { ...state.currentPage, runtime_cleanup: { ...state.currentPage.runtime_cleanup } };
    if (kind === "generation") state.currentPageGeneration++;
    if (kind === "page") state.currentPage.page_id = "page:replacement";
    if (kind === "cleanup") state.currentPage.runtime_cleanup.id = "cleanup:replacement";
    return plain(state.currentPage);
  };
  return { state, requests, sends, mutations, failures, effects, submit, changeOwner };
}

test("rapid r/e/d waits for each Runtime ack and preserves the observed recovered suffix", async t => {
  t.diagnostic(`browser.js sha256:${sourceHash}`);
  const h = harness(), result = ["r", "e", "d"].map(value => h.submit(text(value)));
  await nextTurn();
  assert.deepEqual(h.requests.map(request => request.event.text), ["r"], "e/d must remain queued while r is unresolved");
  for (let index = 0; index < 3; index++) {
    assert.equal(h.requests.length, index + 1);
    h.requests[index].ack();
    await nextTurn();
  }
  assert.deepEqual((await Promise.all(result)).map(value => value.status), ["fulfilled", "fulfilled", "fulfilled"]);
  assert.equal("Browser-fixture-recove" + h.effects.join(""), "Browser-fixture-recovered");
  assert.deepEqual(h.sends, []);
});

for (const [name, event, messages] of [
  ["Backspace", { type: "key", key: "Backspace" }, ["kd,65288", "ku,65288"]],
  ["Enter", { type: "key", key: "Enter" }, ["kd,65293", "ku,65293"]],
]) test(`${name} dispatch waits behind unresolved Runtime text`, async () => {
  const h = harness(), first = h.submit(text("r")), second = h.submit(event);
  await nextTurn();
  assert.equal(h.requests.length, 1);
  assert.deepEqual(h.sends, []);
  h.requests[0].ack();
  const results = await Promise.all([first, second]);
  assert.ok(results.every(result => result.status === "fulfilled"));
  assert.deepEqual(h.sends, messages);
});

for (const events of [
  [{ type: "click", x: 15, y: 20 }, text("B")],
  [text("B"), { type: "click", x: 15, y: 20 }],
]) test(`${events[0].type} completion precedes ${events[1].type} at the Engine`, async () => {
  const h = harness(), results = events.map(event => h.submit(event));
  await nextTurn();
  assert.deepEqual(h.requests.map(request => request.event), [events[0]]);
  assert.deepEqual(h.sends, [], "a local data-channel send cannot acknowledge remote focus");
  h.requests[0].ack(); await nextTurn();
  assert.deepEqual(h.requests.map(request => request.event), events);
  h.requests[1].ack();
  assert.ok((await Promise.all(results)).every(result => result.status === "fulfilled"));
  assert.deepEqual(h.effects, events.map(event => event.text ?? event.type));
});

test("uncertain click delivery cancels queued text instead of inserting into an unknown focus", async () => {
  const h = harness(), click = h.submit({ type: "click", x: 15, y: 20 }), typed = h.submit(text("B"));
  await nextTurn();
  assert.equal(h.requests.length, 1); assert.equal(h.requests[0].event.type, "click");
  h.requests[0].reject(new TypeError("click acknowledgment unavailable"));
  assert.equal((await click).status, "rejected"); assertCanceled([await typed]);
  assert.equal(h.requests.length, 1); assert.deepEqual(h.sends, []); assert.deepEqual(h.effects, []);
});

test("multiple Runtime text and paste operations each await their predecessor", async () => {
  const h = harness(), results = ["r", "pasted text", "d"].map(value => h.submit(text(value)));
  await nextTurn();
  assert.equal(h.requests.length, 1);
  h.requests[0].ack(); await nextTurn();
  assert.equal(h.requests.length, 2);
  assert.equal(h.requests[1].event.text, "pasted text");
  h.requests[1].ack(); await nextTurn();
  assert.equal(h.requests.length, 3);
  h.requests[2].ack();
  assert.ok((await Promise.all(results)).every(result => result.status === "fulfilled"));
  assert.deepEqual(h.effects, ["r", "pasted text", "d"]);
});

for (const change of ["generation", "page", "cleanup"]) {
  for (const outcome of ["success", "error"]) {
    test(`${change} change discards waiting input and ignores late ${outcome}`, async () => {
      const h = harness(), first = h.submit(text("r"));
      await nextTurn();
      const queuedText = h.submit(text("e"));
      const queuedKey = h.submit({ type: "key", key: "Backspace" });
      await nextTurn();
      const replacement = h.changeOwner(change);
      if (outcome === "success") h.requests[0].ack({ accepted: true, actual_url: "https://stale.invalid/", title: "Stale" });
      else h.requests[0].reject(Object.assign(new Error("old owner transport failed"), { status: 404 }));
      await nextTurn();
      assert.equal(h.requests.length, 1, "waiting text must not reach the old or replacement owner");
      assert.deepEqual(h.sends, [], "waiting edit key must not reach the replacement display");
      const results = await Promise.all([first, queuedText, queuedKey]);
      assert.ok(results.every(result => result.status === "fulfilled"), "stale failures must not escape to the new viewer");
      assert.deepEqual(plain(h.state.currentPage), replacement);
      assert.deepEqual(h.mutations, []);
      assert.deepEqual(h.failures, []);
      const fresh = h.submit(text("fresh"));
      await nextTurn();
      assert.equal(h.requests.length, 2);
      assert.ok(h.requests[1].path.includes(encodeURIComponent(replacement.page_id)));
      h.requests[1].ack();
      assert.equal((await fresh).status, "fulfilled");
    });
  }
}

test("closing the page discards queued input and late response mutations", async () => {
  const h = harness(), first = h.submit(text("r"));
  await nextTurn();
  const waiting = h.submit(text("e"));
  h.state.currentPage = null;
  h.requests[0].ack({ accepted: true, actual_url: "https://stale.invalid/" });
  await nextTurn();
  assert.equal(h.requests.length, 1);
  assert.ok((await Promise.all([first, waiting])).every(result => result.status === "fulfilled"));
  assert.equal(h.state.currentPage, null);
  assert.deepEqual(h.mutations, []);
});

for (const outcome of ["success", "error"]) test(`late old-owner ${outcome} leaves the replacement owner's active queue intact`, async () => {
  const h = harness(), old = h.submit(text("old"));
  await nextTurn();
  const abandoned = h.submit(text("abandoned"));
  h.changeOwner("generation");
  const fresh = h.submit(text("fresh")), following = h.submit(text("following"));
  await nextTurn();
  assert.deepEqual(h.requests.map(request => request.event.text), ["old", "fresh"]);
  if (outcome === "success") h.requests[0].ack({ accepted: true, actual_url: "https://stale.invalid/" });
  else h.requests[0].reject(new TypeError("old uncertain delivery"));
  await Promise.all([old, abandoned]);
  assert.deepEqual(h.mutations, []);
  assert.deepEqual(h.failures, []);
  assert.equal(h.requests.length, 2);
  h.requests[1].ack(); await nextTurn();
  assert.deepEqual(h.requests.map(request => request.event.text), ["old", "fresh", "following"]);
  h.requests[2].ack();
  assert.ok((await Promise.all([fresh, following])).every(result => result.status === "fulfilled"));
});

test("uncertain dispatch failure abandons queued dependents without replay; fresh input can proceed", async () => {
  const h = harness(), first = h.submit(text("r"));
  await nextTurn();
  const dependents = [h.submit(text("e")), h.submit(text("d")), h.submit({ type: "key", key: "Enter" }),
    h.submit({ type: "click", x: 15, y: 20 })];
  await nextTurn();
  assert.equal(h.requests.length, 1);
  const error = new TypeError("uncertain Runtime response");
  h.requests[0].reject(error);
  assert.equal((await first).error, error);
  assertCanceled(await Promise.all(dependents));
  assert.equal(h.requests.length, 1);
  assert.deepEqual(h.sends, []);
  assert.deepEqual(h.effects, []);
  assert.deepEqual(h.mutations, []);
  const fresh = h.submit(text("fresh"));
  await nextTurn();
  assert.deepEqual(h.requests.map(request => request.event.text), ["r", "fresh"]);
  h.requests[1].ack();
  assert.equal((await fresh).status, "fulfilled");
});

test("known missing page starts cleanup once and rejects original failure while abandoning queued input", async () => {
  const h = harness(), cleanupCalls = [], originalOwner = plain(h.state.currentRuntimePageOwner());
  h.state.recoverMissingRuntimePage = (error, message) => {
    cleanupCalls.push({ error, message });
    // Cleanup has started asynchronously; it has not cleared the current owner.
    return true;
  };
  const first = h.submit(text("r"));
  await nextTurn();
  const dependents = [h.submit(text("e")), h.submit(text("d")), h.submit({ type: "key", key: "Enter" }),
    h.submit({ type: "click", x: 15, y: 20 })];
  await nextTurn();
  assert.equal(h.requests.length, 1);
  const error = Object.assign(new Error("browser session is not active"), { status: 404 });
  h.requests[0].reject(error);
  const result = await first;
  assert.equal(result.status, "rejected");
  assert.equal(result.error, error, "recognizing a missing page must not swallow the dispatch failure");
  assertCanceled(await Promise.all(dependents));
  assert.equal(cleanupCalls.length, 1);
  assert.equal(cleanupCalls[0].error, error);
  assert.deepEqual(plain(h.state.currentRuntimePageOwner()), originalOwner);
  assert.equal(h.requests.length, 1);
  assert.deepEqual(h.sends, []);
  assert.deepEqual(h.effects, []);
  assert.deepEqual(h.mutations, []);
});

test("synchronous data-channel failure also abandons inputs already behind it", async () => {
  const h = harness(), error = new Error("input channel closed");
  h.state.remoteDisplay.sendInputMessages = () => { throw error; };
  const first = h.submit({ type: "key", key: "Enter" }), waiting = h.submit(text("e"));
  assert.equal((await first).error, error);
  await nextTurn();
  assert.equal(h.requests.length, 0);
  assertCanceled([await waiting]);
  const fresh = h.submit(text("fresh"));
  await nextTurn();
  assert.equal(h.requests.length, 1);
  h.requests[0].ack();
  assert.equal((await fresh).status, "fulfilled");
});

for (const outcome of ["success", "failure"]) test(`actual clipboard bridge ${outcome === "failure" ? "cannot continue a canceled queued copy" : "reads and writes after an acknowledged queued copy"}`, async () => {
  const h = harness(), timers = new Map(), scheduled = [], inputs = [], hostWrites = [];
  let nextTimer = 0;
  const bridge = createBrowserClipboardBridge({
    getCurrentPage: () => h.state.currentPage,
    sendBrowserInput: (event, options) => {
      inputs.push(event.type);
      // Delegate to the actual extracted FIFO and dispatch functions. Returning
      // a synthetic successful copy here would hide the canceled-copy defect.
      return h.state.sendBrowserInput(event, options);
    },
    friendlyOpenError: error => error.message,
    showStatus: () => {},
    writeHostClipboardTextFn: async (value, options) => { hostWrites.push({ value, options }); },
    createClipboardRequestIdFn: () => "clipboard:ordering-test",
    setTimeoutFn: (callback, delay) => {
      const id = ++nextTimer;
      scheduled.push(delay); timers.set(id, { callback, delay }); return id;
    },
    clearTimeoutFn: id => timers.delete(id),
  });
  try {
    const first = h.submit(text("r"));
    await nextTurn();
    const copy = bridge.copyRemoteClipboardToHost()
      .then(value => ({ status: "fulfilled", value }), error => ({ status: "rejected", error }));
    await nextTurn();
    assert.deepEqual(inputs, ["key_combo"]);
    assert.deepEqual(h.sends, [], "Ctrl+C must wait behind Runtime text");
    if (outcome === "failure") h.requests[0].reject(new TypeError("uncertain Runtime text delivery"));
    else h.requests[0].ack();
    await first;
    const result = await copy;
    // Run any actual delayed clipboard read, then supply a valid remote reply.
    // A falsely successful canceled copy would start a fresh read and host write.
    for (const [id, timer] of [...timers]) {
      if (timer.delay !== 150) continue;
      timers.delete(id); timer.callback();
    }
    await nextTurn();
    await bridge.handleRemoteInputChannelMessage({ data: JSON.stringify({
      type: "clipboard-msg", data: { mime_type: "text/plain", content: btoa("fixture clipboard") },
    }) });
    if (outcome === "failure") {
      assertCanceled([result]);
      assert.deepEqual(inputs, ["key_combo"], "canceled copy must not submit clipboard_read on a fresh queue");
      assert.ok(!scheduled.includes(150), "canceled copy must not schedule a clipboard read");
      assert.deepEqual(h.sends, []);
      assert.deepEqual(hostWrites, []);
    } else {
      assert.equal(result.status, "fulfilled");
      assert.deepEqual(inputs, ["key_combo", "clipboard_read"]);
      assert.deepEqual(h.sends, ["kd,65507", "kd,99", "ku,99", "ku,65507", "cr"]);
      assert.deepEqual(hostWrites, [{ value: "fixture clipboard", options: { requestId: "clipboard:ordering-test" } }]);
    }
    assert.equal(timers.size, 0);
    assert.equal(h.requests.length, 1);
  } finally {
    bridge.teardownRemoteClipboard();
  }
});

test("128 pending inputs are bounded; overflow preserves accepted order and capacity returns after ack", async () => {
  const h = harness(), accepted = Array.from({ length: 128 }, (_, index) => `input-${index}`);
  const results = accepted.map(value => h.submit(text(value)));
  const overflow = h.submit(text("overflow"));
  await nextTurn();
  assert.equal(h.requests.length, 1, "the cap must not introduce concurrent dispatch");
  const rejected = await overflow;
  assert.equal(rejected.status, "rejected");
  assert.equal(rejected.error.message, "Browser input is busy. Check the page before typing again.");
  h.requests[0].ack(); await nextTurn();
  const replacement = h.submit(text("replacement"));
  await nextTurn();
  assert.equal(h.requests.length, 2, "new capacity still joins the existing tail");
  for (let index = 1; index <= accepted.length; index++) {
    assert.equal(h.requests.length, index + 1);
    h.requests[index].ack(); await nextTurn();
  }
  assert.ok((await Promise.all([...results, replacement])).every(result => result.status === "fulfilled"));
  assert.deepEqual(h.effects, [...accepted, "replacement"]);
  assert.deepEqual(h.requests.map(request => request.event.text), [...accepted, "replacement"]);
});
