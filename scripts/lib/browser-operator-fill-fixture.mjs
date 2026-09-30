// Source-only Engine fixture: executes the unchanged dispatcher and its fixed
// isolated-world functions. DOM/CDP transport are simulated; no browser starts.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

export function createFillFixture() {
  const source = readFileSync(new URL("../browser-selkies-control-service.mjs", import.meta.url), "utf8");
  const block = (start, end) => {
    const a = source.indexOf(start), b = source.indexOf(end, a);
    assert.ok(a >= 0 && b > a); return source.slice(a, b);
  };
  const root = { activeElement: null }, calls = [], gates = [];
  let now = 10, hook = null, preventDelete = false;
  class Input {
    constructor() { Object.assign(this, { type: "text", value: "prefilled", isConnected: true, readOnly: false,
      disabled: false, inert: false, selectionStart: 0, selectionEnd: 0, visible: true }); }
    matches() { return this.disabled; }
    closest() { return this.inert; }
    getBoundingClientRect() { return { width: this.visible ? 100 : 0, height: 20 }; }
    getRootNode() { return root; }
    focus() { root.activeElement = this; }
    select() { this.selectionStart = 0; this.selectionEnd = this.value.length; }
  }
  class Textarea extends Input {}
  const field = new Input();
  const realm = vm.createContext({ HTMLInputElement: Input, HTMLTextAreaElement: Textarea,
    getComputedStyle: () => ({ visibility: "visible", display: "block" }) });
  const frame = { id: "frame", loaderId: "loader", url: "https://fixture.invalid/form" };
  const snapshot = { id: "b".repeat(32), expires: 30010, backendNodes: [{ backendDOMNodeId: 7, frameId: "frame" }] };
  const page = { pageId: "page:existing", closed: false, browserPage: { debugger_url: "ws://engine-private/page",
    _inspection: { generation: "a".repeat(32), snapshot, binding: "frame:loader:https://fixture.invalid/form" } } };
  const cdp = { closed: false, async request(method, params = {}) {
    calls.push({ method, params });
    let result = {};
    if (method === "Page.getFrameTree") result = { frameTree: { frame } };
    if (method === "DOM.describeNode") result = { node: { backendNodeId: 7, nodeName: "INPUT" } };
    if (method === "Page.createIsolatedWorld") {
      assert.equal(params.frameId, frame.id); assert.equal(params.grantUniveralAccess, false);
      result = { executionContextId: 42 };
    }
    if (method === "DOM.resolveNode") {
      assert.equal(params.backendNodeId, 7); assert.equal(params.executionContextId, 42);
      result = { object: { objectId: "exact-field" } };
    }
    if (method === "Runtime.callFunctionOn") {
      assert.equal(params.objectId, "exact-field");
      assert.ok(gates.some(g => g.command === "begin"), "native hold precedes focus/selection");
      const fn = vm.runInContext(`(${params.functionDeclaration})`, realm);
      result = { result: { value: fn.apply(field, (params.arguments || []).map(a => a.value)) } };
    }
    if (method === "Input.insertText" || (method === "Input.dispatchKeyEvent" && params.type === "keyDown" && !preventDelete)) {
      assert.equal(root.activeElement, field);
      const replacement = params.text || "";
      field.value = field.value.slice(0, field.selectionStart) + replacement + field.value.slice(field.selectionEnd);
      field.selectionEnd = field.selectionStart += replacement.length;
    }
    if (hook) await hook(method, params);
    return result;
  } };
  page.browserPage._cdp = cdp;
  const context = vm.createContext({ Buffer, console, setTimeout, clearTimeout, performance: { now: () => now },
    withBrowserCdp: async (browser, _timeout, callback, options) => {
      assert.equal(browser, page.browserPage); assert.equal(options.retryAction, false); return callback(cdp);
    } });
  vm.runInContext(block("function withTimeout(", "\nfunction createBrowserFileChooserState") +
    block("const OPERATOR_ID =", "async function withBrowserCdp("), context);
  context.gate = async (command, id) => { gates.push({ command, id }); return { active: command !== "release" }; };
  vm.runInContext("browserInputWriterGate = gate", context);
  const lease = { command: "acquire", admission_id: "c".repeat(32), document_generation: "a".repeat(32),
    actions: ["click", "fill"], duration_ms: 30000 };
  return { field, page, calls, gates, frame, snapshot,
    acquire: () => context.browserOperatorLease(page, lease, () => !page.closed),
    input: event => context.browserRefInput(page, event, () => !page.closed),
    revoke: () => context.browserOperatorLease(page, { command: "release", admission_id: lease.admission_id }, () => true),
    hook: fn => { hook = fn; }, preventDelete: () => { preventDelete = true; }, advance: ms => { now += ms; },
    close: () => { page.closed = true; if (page.operatorLease) { clearTimeout(page.operatorLease.timer); page.operatorLease.active = false; } } };
}
