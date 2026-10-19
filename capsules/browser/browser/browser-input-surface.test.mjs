import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import vm from 'node:vm';
import { execFileSync } from "node:child_process";
import { plainTextFromClipboardEvent } from "./browser-input-surface.js";
const sourceRef = process.env.BROWSER_INPUT_SURFACE_SOURCE_REF;
const { bindBrowserInputSurface } = sourceRef
  ? await import(`data:text/javascript,${encodeURIComponent(execFileSync("git", ["show",
    `${sourceRef}:capsules/browser/browser/browser-input-surface.js`],
  { cwd: new URL("../../../", import.meta.url), encoding: "utf8" }))}`)
  : await import("./browser-input-surface.js");
import { runtimePageOwner, sameRuntimePageOwner } from "./browser-page-cleanup.js";
// The encoder must follow an explicit historical source too: otherwise a
// negative control could silently exercise the fixed working-tree encoder.
const inputSourceRef = process.env.BROWSER_INPUT_COORDINATE_SOURCE_REF || sourceRef;
const { selkiesMessagesForInput } = inputSourceRef
  ? await import(`data:text/javascript,${encodeURIComponent(execFileSync("git", ["show",
    `${inputSourceRef}:capsules/browser/browser/browser-input.js`],
  { cwd: new URL("../../../", import.meta.url), encoding: "utf8" }))}`)
  : await import("./browser-input.js");
// Execute the actual Browser binding, owner predicate, FIFO and encoder. The
// fixture replaces DOM/timers and transport effects, not their source contract.
const source = sourceRef
  ? execFileSync("git", ["show", `${sourceRef}:capsules/browser/browser/browser.js`],
    { cwd: new URL("../../../", import.meta.url), encoding: "utf8" })
  : readFileSync(new URL("./browser.js", import.meta.url), "utf8");
function declaration(name) {
  const start = source.search(new RegExp(`(?:async )?function ${name}\\(`));
  const end = source.slice(start).search(/\n\}(?=\n|$)/);
  assert.ok(start >= 0 && end > 0, name);
  return source.slice(start, start + end + 2);
}
function target() {
  const listeners = new Map();
  return { listeners, hidden: false, videoWidth: 640, videoHeight: 360,
    addEventListener(type, fn) { listeners.set(type, [...listeners.get(type) || [], fn]); },
    focus() {}, setSelectionRange() {}, closest() { return null; }, contains() { return false; },
    capturedPointers: new Set(),
    setPointerCapture(id) { this.capturedPointers.add(id); },
    hasPointerCapture(id) { return this.capturedPointers.has(id); },
    releasePointerCapture(id) {
      this.capturedPointers.delete(id);
      for (const listener of listeners.get("lostpointercapture") || []) listener({ pointerId: id });
    },
    getBoundingClientRect() { return { left: 0, top: 0, width: 640, height: 360 }; },
    ownerDocument: { defaultView: { getComputedStyle: () => ({ objectFit: 'fill' }) } },
  };
}
const plain = x => JSON.parse(JSON.stringify(x));
function harness(t) {
  const previousWindow = globalThis.window, previousDocument = globalThis.document;
  const timers = new Map(), inputs = [], wire = [], errors = [], requests = [];
  let now = 0, nextTimer = 0;
  const panel = target();
  globalThis.window = Object.assign(target(), {
    setTimeout(callback, delay) { timers.set(++nextTimer, { at: now + delay, callback }); return nextTimer; },
    clearTimeout(id) { timers.delete(id); },
  });
  globalThis.document = target();
  t.after(() => { globalThis.window = previousWindow; globalThis.document = previousDocument; });
  const state = vm.createContext({
    currentPage: { page_id: 'page:original', runtime_cleanup: { schema: 'elastos.browser.cleanup-handle/v1', id: 'cleanup:original' },
      display_session: { input: 'datachannel', input_protocol: 'selkies_v1', display_generation: 'display:original' } },
    currentPageGeneration: 1, currentDisplayMode: 'webrtc_remote_display', currentView: { width: 640, height: 360 },
    currentDisplayInput: 'datachannel', currentDisplayInputProtocol: 'selkies_v1',
    unloadCleanupStarted: false, homeWindowCloseInFlight: false, homeWindowTerminalCloseConfirmed: false,
    pendingHomeWindowCloseDelivery: null, cleanupPending: false,
    runtimePageCleanup: { status() { return state.cleanupPending; } },
    runtimePageOwner, sameRuntimePageOwner, selkiesMessagesForInput, libraryPickerRequest: null,
    PAGE_STATUS_AFTER_INPUT_DELAY_MS: 1, PAGE_STATUS_AFTER_SCROLL_DELAY_MS: 1,
    remoteDisplay: { inputChannelOpen: () => true, sendInputMessages: messages => wire.push(...plain(messages)) },
    fetchJson: async (path, options) => { requests.push({ path, body: plain(options.body) }); return { accepted: true }; },
    recoverMissingRuntimePage() {}, handleFileChooserFromStatus() {}, syncViewFromResponse() {}, syncBrowserLocation() {},
    focusRemoteInput() {}, schedulePageStatusRefresh() {},
  });
  const names = ['currentRuntimePageOwner','runtimeViewerOwnerActive','currentInputTransport','currentInputProtocol','encodeDatachannelInput','sendBrowserInput','dispatchBrowserInput'];
  vm.runInContext(source.match(/^let browserInputQueue = [^\n]+;$/m)[0] + '\n' + source.match(/^const MAX_PENDING_BROWSER_INPUTS = [0-9_]+;$/m)[0] + '\n' + names.map(declaration).join('\n'), state);
  const actualSendBrowserInput = state.sendBrowserInput;
  Object.assign(state, {
    bindBrowserInputSurface, renderPanel: panel, remoteVideo: target(), keyboardCapture: target(),
    sendBrowserInput: (event, options) => {
      inputs.push({ at: now, owner: plain(state.currentRuntimePageOwner()), event });
      return actualSendBrowserInput(event, options);
    },
    copyRemoteClipboardToHost: async () => {}, pasteHostClipboardIntoRemote: async () => {},
    homeClipboard: { readText: async () => "" },
    friendlyOpenError: error => error.message, showStatus: message => errors.push(message),
    unlockRemoteAudioFromGesture() {},
  });
  const binding = source.match(/^bindBrowserInputSurface\(\{[\s\S]*?\n\}\);/m);
  assert.ok(binding, "actual Browser surface binding must remain executable");
  vm.runInContext(binding[0], state);
  async function settle() { for (let i = 0; i < 16; i++) await Promise.resolve(); }
  async function advance(to) {
    for (;;) {
      const next = [...timers.entries()].filter(([, timer]) => timer.at <= to).sort((a,b) => a[1].at - b[1].at)[0];
      if (!next) break;
      timers.delete(next[0]); now = next[1].at; next[1].callback(); await settle();
    }
    now = to; await settle();
  }
  function emit(type, event = {}, eventTarget = panel) {
    for (const listener of eventTarget.listeners.get(type) || []) listener({ target: eventTarget, clientX: 20, clientY: 30, deltaX: 0, deltaY: 10,
      ctrlKey: false, metaKey: false, altKey: false, preventDefault() {}, stopPropagation() {}, ...event });
  }
  function replace(kind) {
    state.currentPage = { ...state.currentPage, runtime_cleanup: { ...state.currentPage.runtime_cleanup } };
    if (kind === 'page') state.currentPage.page_id = 'page:replacement';
    if (kind === 'generation') state.currentPageGeneration++;
    if (kind === 'cleanup') state.currentPage.runtime_cleanup.id = 'cleanup:replacement';
  }
  return { state, timers, inputs, wire, errors, requests, emit, replace, advance, settle };
}
test('unchanged owner keeps 80 ms trailing batch, raw axes, latest point and real encoded steps', async t => {
  t.diagnostic('actual browser.js SHA-256 ' + createHash('sha256').update(source).digest('hex'));
  const h = harness(t);
  h.emit('wheel', { deltaX: 2, deltaY: 8 }); await h.advance(40);
  h.replace('metadata'); h.state.currentPage.display_session = { ...h.state.currentPage.display_session, display_generation: 'display:renewed' };
  h.emit('wheel', { deltaX: 3, deltaY: 8, clientX: 25 }); await h.advance(119);
  assert.equal(h.inputs.length, 0); await h.advance(120);
  assert.deepEqual(h.inputs.map(x => [x.at, x.event]), [[120, { type: 'wheel', x: 25, y: 30, delta_x: 5, delta_y: 16 }]]);
  assert.deepEqual(h.wire, ['m,25,30,8,1', 'm,25,30,0,0']);
});
for (const kind of ['page','generation','cleanup']) test(`deferred wheel never reaches replacement ${kind}`, async t => {
  const h = harness(t); h.emit('wheel'); h.replace(kind); await h.advance(80);
  assert.deepEqual(h.wire, []); assert.deepEqual(h.requests, []);
});
for (const flag of ['unloadCleanupStarted','homeWindowCloseInFlight','homeWindowTerminalCloseConfirmed','cleanupPending','pendingHomeWindowCloseDelivery']) test(`pending wheel stops while ${flag}`, async t => {
  const h = harness(t); h.emit('wheel'); h.state[flag] = flag === 'pendingHomeWindowCloseDelivery' ? { owner: h.state.currentRuntimePageOwner() } : true;
  await h.advance(80); assert.deepEqual(h.wire, []);
});
test('replacement owner gets only its own new wheel delta', async t => {
  const h = harness(t); h.emit('wheel', { deltaY: 80 }); await h.advance(10); h.replace('generation');
  h.emit('wheel', { deltaY: 80 }); await h.advance(90);
  assert.deepEqual(h.wire, ['m,20,30,8,1','m,20,30,0,0']);
});
test('touch gesture cannot cross owners through move/up or synthetic click', async t => {
  const h = harness(t); h.emit('pointerdown', { pointerType: 'touch', pointerId: 1 }); h.replace('generation');
  h.emit('pointermove', { pointerId: 1, clientY: 10 }); h.emit('pointerup', { pointerId: 1 }); h.emit('click'); await h.advance(80);
  assert.deepEqual(h.wire, []); assert.deepEqual(h.requests, []);
});
test('old-owner tap cannot click the replacement page', async t => {
  const h = harness(t); h.emit('pointerdown', { pointerType: 'touch', pointerId: 1 }); h.replace('cleanup');
  h.emit('pointerup', { pointerId: 1 }); h.emit('click'); await h.settle();
  assert.deepEqual(h.requests, []);
});
test('fresh owner requires a new touch gesture, then preserves existing motion', async t => {
  const h = harness(t); h.emit('pointerdown', { pointerType: 'touch', pointerId: 1 }); h.replace('generation');
  h.emit('pointermove', { pointerId: 1, clientY: 10 }); h.emit('pointerup', { pointerId: 1 });
  h.emit('pointerdown', { pointerType: 'touch', pointerId: 2, clientY: 30 });
  h.emit('pointermove', { pointerId: 2, clientY: 20 }); h.emit('pointerup', { pointerId: 2 }); await h.advance(80);
  assert.deepEqual(h.inputs.map(x => x.event.delta_y), [10]);
});
test('missing valid Runtime owner allocates no deferred input', async t => {
  const h = harness(t); h.state.currentPage = { page_id: 'page:unowned' }; h.emit('wheel');
  h.emit('pointerdown', { pointerType: 'touch', pointerId: 1 });
  h.emit('pointermove', { pointerId: 1, clientY: 10 });
  h.emit('pointerup', { pointerId: 1 });
  h.emit('click');
  assert.equal(h.timers.size, 0); await h.advance(80);
  assert.deepEqual(h.wire, []); assert.deepEqual(h.requests, []);
});

function clipboardEvent(types, values = {}) {
  return {
    clipboardData: {
      types,
      getData(type) {
        return values[type] ?? "";
      },
    },
  };
}

test("explicit ClipboardEvent accepts only text/plain", () => {
  assert.equal(
    plainTextFromClipboardEvent(
      clipboardEvent(["text/plain"], { "text/plain": "paste me" }),
    ),
    "paste me",
  );
  assert.equal(
    plainTextFromClipboardEvent(
      clipboardEvent(["text/html"], { "text/html": "<b>no</b>" }),
    ),
    "",
  );
  assert.equal(
    plainTextFromClipboardEvent(
      clipboardEvent(["text"], { text: "alias is not accepted" }),
    ),
    "",
  );
});

test("missing or malformed ClipboardEvent data is rejected", () => {
  assert.equal(plainTextFromClipboardEvent(null), "");
  assert.equal(plainTextFromClipboardEvent({ clipboardData: {} }), "");
  assert.equal(
    plainTextFromClipboardEvent({
      clipboardData: {
        types: ["text/plain"],
        getData() {
          return null;
        },
      },
    }),
    "",
  );
});


test("a retired timer callback leaves the replacement batch intact", async t => {
  const h = harness(t);
  h.emit("wheel", { deltaY: 80 });
  const retired = [...h.timers.values()][0].callback;
  await h.advance(10);
  h.replace("generation");
  h.emit("wheel", { deltaY: 160 });
  retired();
  await h.settle();
  assert.deepEqual(h.wire, []);
  await h.advance(90);
  assert.deepEqual(h.wire, ["m,20,30,8,2", "m,20,30,0,0"]);
});

test("a late old-owner send failure leaves new-owner status and wheel untouched", async t => {
  const h = harness(t);
  const send = h.state.remoteDisplay.sendInputMessages;
  h.state.remoteDisplay.sendInputMessages = () => {
    queueMicrotask(() => {
      h.replace("generation");
      h.emit("wheel", { deltaY: 160 });
    });
    h.state.remoteDisplay.sendInputMessages = send;
    throw new Error("old channel failed");
  };
  h.emit("wheel");
  await h.advance(80);
  assert.deepEqual(h.errors, []);
  await h.advance(160);
  assert.deepEqual(h.wire, ["m,20,30,8,2", "m,20,30,0,0"]);
});

test("current-owner uncertainty is reported once and fresh input can proceed", async t => {
  const h = harness(t);
  const send = h.state.remoteDisplay.sendInputMessages;
  h.state.remoteDisplay.sendInputMessages = () => { throw new Error("channel failed"); };
  h.emit("wheel");
  await h.advance(80);
  assert.deepEqual(h.errors, ["channel failed"]);
  h.state.remoteDisplay.sendInputMessages = send;
  h.emit("wheel", { deltaY: 160 });
  await h.advance(160);
  assert.deepEqual(h.wire, ["m,20,30,8,2", "m,20,30,0,0"]);
});

for (const flag of ["homeWindowCloseInFlight", "unloadCleanupStarted"]) {
  test(`an active touch cannot move or tap while ${flag}`, async t => {
    const h = harness(t);
    h.emit("pointerdown", { pointerType: "touch", pointerId: 1 });
    h.state[flag] = true;
    h.emit("pointermove", { pointerId: 1, clientY: 10 });
    h.emit("pointerup", { pointerId: 1 });
    h.emit("click");
    await h.advance(80);
    assert.deepEqual(h.wire, []);
    assert.deepEqual(h.requests, []);
  });
}

test("same-owner tap uses the existing Runtime click route once", async t => {
  const h = harness(t);
  h.emit("pointerdown", { pointerType: "pen", pointerId: 1 });
  h.replace("metadata");
  h.emit("pointerup", { pointerId: 1 });
  h.emit("click");
  await h.settle();
  assert.deepEqual(h.requests, [{ path: "/api/apps/browser/pages/page%3Aoriginal/input",
    body: { event: { type: "click", x: 20, y: 30 } } }]);
  assert.deepEqual(h.wire, []);
});

test("overlapping pointers retain the current owner's first gesture", async t => {
  const h = harness(t);
  h.emit("pointerdown", { pointerType: "touch", pointerId: 1 });
  h.emit("pointerdown", { pointerType: "touch", pointerId: 2 });
  h.emit("pointermove", { pointerId: 2, clientY: 10 });
  h.emit("pointerup", { pointerId: 2 });
  h.emit("pointermove", { pointerId: 1, clientY: 20 });
  h.emit("pointerup", { pointerId: 1 });
  await h.advance(80);
  assert.deepEqual(h.inputs.map(input => input.event.delta_y), [10]);
  assert.deepEqual(h.requests, []);
});

for (const kind of ["page", "generation", "cleanup"]) {
  test(`retired ${kind} touch cannot block a new pointer without its old release`, async t => {
    const h = harness(t);
    h.emit("pointerdown", { pointerType: "touch", pointerId: 1 });
    h.replace(kind);
    h.emit("pointerdown", { pointerType: "touch", pointerId: 2 });
    assert.deepEqual([...h.state.renderPanel.capturedPointers], [2]);
    // A late old release and its synthetic click belong to the retired owner.
    h.emit("pointerup", { pointerId: 1 });
    h.emit("click");
    h.emit("pointermove", { pointerId: 2, clientY: 20 });
    h.emit("pointerup", { pointerId: 2 });
    await h.advance(80);
    assert.deepEqual(h.inputs.map(input => input.event.delta_y), [10]);
    assert.deepEqual(h.wire, ["m,20,20,8,1", "m,20,20,0,0"]);
    assert.deepEqual(h.requests, []);
  });
}

for (const replaceOwner of [false, true]) {
  test(`lost pointer capture permits a fresh ${replaceOwner ? "replacement" : "same"} owner's gesture`, async t => {
    const h = harness(t);
    h.emit("pointerdown", { pointerType: "pen", pointerId: 1 });
    if (replaceOwner) h.replace("generation");
    h.emit("lostpointercapture", { pointerId: 1 });
    h.emit("click");
    h.emit("pointerdown", { pointerType: "pen", pointerId: 2 });
    h.emit("pointermove", { pointerId: 2, clientY: 20 });
    h.emit("pointerup", { pointerId: 2 });
    await h.advance(80);
    assert.deepEqual(h.inputs.map(input => input.event.delta_y), [10]);
    assert.deepEqual(h.requests, []);
  });
}

test("a move from a retired owner releases its tracked pointer immediately", async t => {
  const h = harness(t);
  h.emit("pointerdown", { pointerType: "touch", pointerId: 1 });
  h.replace("generation");
  h.emit("pointermove", { pointerId: 1, clientY: 20 });
  assert.deepEqual([...h.state.renderPanel.capturedPointers], []);
  h.emit("click");
  await h.advance(80);
  assert.deepEqual(h.inputs, []);
  assert.deepEqual(h.requests, []);
});

for (const pointerType of ["touch", "pen"]) {
  for (const retirement of ["move", "capture loss", "replacement pointer"]) {
    test(`delayed ${pointerType} release after ${retirement} cannot click the replacement owner`, async t => {
      let wallClock = 1_000;
      t.mock.method(Date, "now", () => wallClock);
      const h = harness(t);
      h.emit("pointerdown", { pointerType, pointerId: 1 });
      h.replace("generation");
      if (retirement === "move") h.emit("pointermove", { pointerType, pointerId: 1, clientY: 20 });
      if (retirement === "capture loss") h.emit("lostpointercapture", { pointerType, pointerId: 1 });
      if (retirement === "replacement pointer") h.emit("pointerdown", { pointerType, pointerId: 2 });
      wallClock = 2_000;
      h.emit("pointerup", { pointerType, pointerId: 1 });
      // Provenance protects both click surfaces even after the renewed legacy
      // fallback expires. A later callback cannot acquire the current owner.
      wallClock = 3_000;
      h.emit("click", { pointerType, pointerId: 1 });
      h.emit("click", { pointerType, pointerId: 1 }, h.state.remoteVideo);
      await h.settle();
      assert.deepEqual(h.requests, []);
      assert.deepEqual(h.wire, []);
      if (retirement !== "replacement pointer") h.emit("pointerdown", { pointerType, pointerId: 2 });
      h.emit("pointerup", { pointerType, pointerId: 2 });
      h.emit("click", { pointerType, pointerId: 2 });
      await h.settle();
      assert.deepEqual(h.requests, [{ path: "/api/apps/browser/pages/page%3Aoriginal/input",
        body: { event: { type: "click", x: 20, y: 30 } } }], "the fresh owner's tap dispatches once");
    });
  }

  test(`delayed retired ${pointerType} release renews only the legacy click fallback`, async t => {
    let wallClock = 1_000;
    t.mock.method(Date, "now", () => wallClock);
    const h = harness(t);
    h.emit("pointerdown", { pointerType, pointerId: 1 });
    h.replace("generation");
    h.emit("pointermove", { pointerType, pointerId: 1, clientY: 20 });
    wallClock = 2_000;
    h.emit("pointerup", { pointerType, pointerId: 1 });
    h.emit("click");
    await h.settle();
    assert.deepEqual(h.requests, [], "legacy compatibility click follows the late release");
    h.emit("click", { pointerType: "mouse", pointerId: 9 });
    await h.settle();
    assert.equal(h.requests.length, 1, "a fresh known mouse is not the retired touch");
    wallClock = 3_000;
    h.emit("click", { sourceCapabilities: { firesTouchEvents: true } });
    await h.settle();
    assert.equal(h.requests.length, 1, "legacy touch provenance stays bound after the fallback expires");
  });
}

test("losing an unrelated pointer's capture keeps the current gesture", async t => {
  const h = harness(t);
  h.emit("pointerdown", { pointerType: "touch", pointerId: 1 });
  h.emit("lostpointercapture", { pointerId: 2 });
  h.emit("pointermove", { pointerId: 1, clientY: 20 });
  h.emit("pointerup", { pointerId: 1 });
  await h.advance(80);
  assert.deepEqual(h.inputs.map(input => input.event.delta_y), [10]);
});

test("canceling the tracked pointer permits a fresh owner's gesture", async t => {
  const h = harness(t);
  h.emit("pointerdown", { pointerType: "pen", pointerId: 1 });
  h.replace("generation");
  h.emit("pointercancel", { pointerId: 1 });
  h.emit("pointerdown", { pointerType: "pen", pointerId: 2 });
  h.emit("pointermove", { pointerId: 2, clientY: 20 });
  h.emit("pointerup", { pointerId: 2 });
  await h.advance(80);
  assert.deepEqual(h.inputs.map(input => input.event.delta_y), [10]);
  assert.deepEqual(h.wire, ["m,20,20,8,1", "m,20,20,0,0"]);
});

const controlSource = readFileSync(new URL("../../../scripts/browser-selkies-control-service.mjs", import.meta.url), "utf8");
function controlDeclaration(name) {
  const start = controlSource.search(new RegExp(`(?:async )?function ${name}\\(`));
  const end = controlSource.slice(start).search(/\n\}(?=\n|$)/);
  assert.ok(start >= 0 && end > 0, name);
  return controlSource.slice(start, start + end + 2);
}
const provider = vm.createContext({});
vm.runInContext(["finiteCoordinate", "dispatchBrowserInputEvent"].map(controlDeclaration).join("\n"), provider);

for (const [clientX, clientY] of [[0, 0], [0, 30], [20, 0], [20, 30]]) {
  test(`actual surface and FIFO preserve wheel point (${clientX}, ${clientY}) in Selkies`, async t => {
    const h = harness(t);
    h.emit("wheel", { clientX, clientY, deltaX: 11, deltaY: -81 });
    await h.advance(80);
    assert.deepEqual(h.inputs.map(input => input.event), [{ type: "wheel", x: clientX, y: clientY, delta_x: 11, delta_y: -81 }]);
    assert.deepEqual(h.wire, [`m,${clientX},${clientY},16,2`, `m,${clientX},${clientY},0,0`]);
    assert.deepEqual(h.requests, []);
  });
  test(`actual Runtime route and CDP adapter retain wheel point (${clientX}, ${clientY})`, async t => {
    const h = harness(t), cdp = [], requests = [];
    h.state.currentPage.display_session.input = "runtime_route";
    h.state.currentDisplayInput = "runtime_route";
    h.state.fetchJson = async (path, options) => {
      requests.push({ path, event: plain(options.body.event) });
      await provider.dispatchBrowserInputEvent({}, options.body.event, 100,
        { request: async (method, params) => cdp.push({ method, params: plain(params) }) });
      return { accepted: true };
    };
    h.emit("wheel", { clientX, clientY, deltaX: 11, deltaY: -81 });
    await h.advance(80);
    assert.deepEqual(requests, [{ path: "/api/apps/browser/pages/page%3Aoriginal/input",
      event: { type: "wheel", x: clientX, y: clientY, delta_x: 11, delta_y: -81 } }]);
    assert.deepEqual(cdp, [
      { method: "Input.setIgnoreInputEvents", params: { ignore: false } },
      { method: "Input.dispatchMouseEvent", params: { type: "mouseWheel", x: clientX, y: clientY, deltaX: 11, deltaY: -81 } },
    ]);
    assert.deepEqual(h.wire, []);
  });
}

test("scaled contained video maps letterboxed host points to decoded wheel edges", async t => {
  const h = harness(t), video = h.state.remoteVideo;
  video.videoWidth = 1920;
  video.videoHeight = 1080;
  video.getBoundingClientRect = () => ({ left: 40, top: 70, width: 1000, height: 800 });
  video.ownerDocument.defaultView.getComputedStyle = () => ({ objectFit: "contain" });
  h.state.currentView = { width: 1920, height: 1080 };
  // The contained image is 1000 x 562.5, inset 118.75 from the host's top.
  const points = [[40, 188.75, 0, 0], [40, 245, 0, 108], [90, 188.75, 96, 0]];
  for (const [index, [clientX, clientY]] of points.entries()) {
    h.emit("wheel", { clientX, clientY, deltaX: 0, deltaY: 80 });
    await h.advance((index + 1) * 80);
  }
  assert.deepEqual(h.inputs.map(input => [input.event.x, input.event.y]),
    points.map(([, , x, y]) => [x, y]));
  assert.deepEqual(h.wire, points.flatMap(([, , x, y]) => [`m,${x},${y},8,1`, `m,${x},${y},0,0`]));
  assert.deepEqual(h.requests, []);
});

function wheelGeometry(h, kind) {
  const video = h.state.remoteVideo;
  video.hidden = kind === "hidden";
  video.ownerDocument.defaultView.getComputedStyle = () => ({ objectFit: "contain" });
  const rect = kind === "zero-width" ? { left: 0, top: 0, width: 0, height: 360 }
    : kind === "zero-height" ? { left: 0, top: 0, width: 640, height: 0 }
    : kind === "top" || kind === "bottom" ? { left: 40, top: 70, width: 640, height: 480 }
    : { left: 40, top: 70, width: 800, height: 360 };
  video.getBoundingClientRect = () => rect;
  return {
    top: { clientX: 360, clientY: 129 },
    bottom: { clientX: 360, clientY: 491 },
    left: { clientX: 119, clientY: 250 },
    right: { clientX: 761, clientY: 250 },
  }[kind] || { clientX: 20, clientY: 30 };
}

for (const route of ["datachannel", "runtime_route"]) {
  for (const geometry of ["hidden", "zero-width", "zero-height", "top", "bottom", "left", "right"]) {
    for (const pending of [false, true]) {
      test(`${route} ignores ${geometry} wheel input${pending ? " without disturbing valid pending movement" : ""}`, async t => {
        const h = harness(t);
        h.state.currentPage.display_session.input = route;
        h.state.currentDisplayInput = route;
        if (pending) {
          h.emit("wheel", { clientX: 0, clientY: 30, deltaX: 11, deltaY: -81 });
          await h.advance(40);
        }
        const timersBefore = [...h.timers.entries()];
        let prevented = 0, focused = 0;
        h.state.keyboardCapture.focus = () => focused++;
        h.emit("wheel", { ...wheelGeometry(h, geometry), deltaX: 300, deltaY: 900,
          preventDefault() { prevented++; } });
        const timersAfter = [...h.timers.entries()];
        await h.advance(79);
        assert.deepEqual(h.inputs, []);
        await h.advance(80);
        const event = { type: "wheel", x: 0, y: 30, delta_x: 11, delta_y: -81 };
        assert.deepEqual(h.inputs.map(input => [input.at, input.event]), pending ? [[80, event]] : []);
        assert.deepEqual(h.wire, pending && route === "datachannel" ? ["m,0,30,16,2", "m,0,30,0,0"] : []);
        assert.deepEqual(h.requests, pending && route === "runtime_route" ? [{
          path: "/api/apps/browser/pages/page%3Aoriginal/input", body: { event },
        }] : []);
        assert.deepEqual(timersAfter, timersBefore, "valid batch keeps its original deadline");
        assert.equal(prevented, 0, "unaddressable input leaves outer UI behavior available");
        assert.equal(focused, 0, "unaddressable input leaves keyboard focus alone");
        await h.advance(200);
        assert.equal(h.inputs.length, pending ? 1 : 0, "ignored input leaves no later batch");
        assert.equal(h.timers.size, 0);
      });
    }
  }

  test(`${route} accepts contained video edges and interior after a margin gesture`, async t => {
    const h = harness(t);
    h.state.currentPage.display_session.input = route;
    h.state.currentDisplayInput = route;
    h.emit("wheel", wheelGeometry(h, "top"));
    let prevented = 0, focused = 0;
    h.state.keyboardCapture.focus = () => focused++;
    // The decoded image occupies x=40..680 and y=130..490 inside this element.
    const points = [[40, 130, 0, 0], [680, 490, 640, 360], [360, 310, 320, 180]];
    for (const [index, [clientX, clientY]] of points.entries()) {
      h.emit("wheel", { clientX, clientY, deltaX: 0, deltaY: 80, preventDefault() { prevented++; } });
      await h.advance((index + 1) * 80);
    }
    const events = points.map(([, , x, y]) => ({ type: "wheel", x, y, delta_x: 0, delta_y: 80 }));
    assert.deepEqual(h.inputs.map(input => input.event), events);
    assert.equal(prevented, points.length);
    assert.equal(focused, points.length);
    assert.deepEqual(h.wire, route === "datachannel"
      ? points.flatMap(([, , x, y]) => [`m,${x},${y},8,1`, `m,${x},${y},0,0`]) : []);
    assert.deepEqual(h.requests, route === "runtime_route" ? events.map(event => ({
      path: "/api/apps/browser/pages/page%3Aoriginal/input", body: { event },
    })) : []);
  });
}
