import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import test from "node:test";

const source = fs.readFileSync(new URL("./browser.js", import.meta.url), "utf8");
const start = source.indexOf("async function navigateAddress(");
const end = source.indexOf("\nasync function navigateBrowser(", start);
function harness() {
  const pending = [], statusNode = { dataset: {}, firstChild: null };
  const context = vm.createContext({
    normalizeUrl: value => value, currentBrowserUrl: () => "https://example.test/",
    sameBrowserStreamTarget: () => true, clearAddressDraft() {},
    currentPage: { page_id: "page:one" }, selectedBrowserEngineId: "engine:one",
    currentBrowserEngineId: "engine:one", selectedRemoteExitId: "exit:one", currentRemoteExitId: "exit:one",
    isBrowserErrorUrl: () => false, addressInput: { value: "", blur() {} }, setLoading() {},
    visibleAddressForUrl: value => value, statusNode, statusTimer: 0, window: { clearTimeout() {} },
    showStatus(message) { statusNode.firstChild = { textContent: message }; statusNode.dataset.visible = "true"; },
    sendBrowserInput: () => new Promise((resolve, reject) => pending.push({ resolve, reject })),
    startPageStatusPolling() {}, friendlyOpenError: error => error.message,
    requestRuntimeOpen: () => { throw new Error("Existing page must be retained"); },
  });
  vm.runInContext(source.slice(start, end), context);
  return { pending, statusNode, navigate: url => context.navigateAddress(url),
    message: text => context.showStatus(text) };
}

test("accepted navigation clears its progress while retaining the same page", async () => {
  const h = harness(), action = h.navigate("https://example.test/next");
  assert.equal(h.statusNode.dataset.visible, "true");
  h.pending[0].resolve({ accepted: true }); await action;
  assert.equal(h.statusNode.dataset.visible, "false");
});
test("an earlier navigation cannot clear a later navigation or failure message", async () => {
  const h = harness(), first = h.navigate("https://example.test/one"), second = h.navigate("https://example.test/two");
  const later = h.statusNode.firstChild;
  h.pending[0].resolve({ accepted: true }); await first;
  assert.equal(h.statusNode.firstChild, later); assert.equal(h.statusNode.dataset.visible, "true");
  h.message("Connection interrupted"); h.pending[1].resolve({ accepted: true }); await second;
  assert.equal(h.statusNode.firstChild.textContent, "Connection interrupted");
  assert.equal(h.statusNode.dataset.visible, "true");
});
test("failed navigation retains its actionable error", async () => {
  const h = harness(), action = h.navigate("https://example.test/next");
  h.pending[0].reject(new Error("Exit permission expired"));
  await assert.rejects(action, /Exit permission expired/);
  assert.equal(h.statusNode.firstChild.textContent, "Exit permission expired");
  assert.equal(h.statusNode.dataset.visible, "true");
});
