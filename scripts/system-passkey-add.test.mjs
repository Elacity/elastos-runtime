import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import vm from "node:vm";

// System's real Add passkey code: the owner-only button, the request to Home
// and the one plain sentence it shows for each result.
const source = readFileSync(new URL("../capsules/system/browser/system.js", import.meta.url), "utf8");
const slice = (start, end) => {
  const from = source.indexOf(start);
  const to = source.indexOf(end, from);
  assert.ok(from >= 0 && to > from, `system.js keeps ${start}`);
  return source.slice(from, to);
};
const controller = [
  slice("async function onPasskeyAdd() {", "async function refreshRecoveryStatus() {"),
  slice("function showPasskeyStatus(message, tone) {", "function setGuestRegistrationControlState() {"),
  slice("function hasExactKeys(value, expectedKeys) {", "function hostedProviderValidationError(value) {"),
].join("\n");

const homeOrigin = "http://localhost:61180";

function fixture({ shellAccess = true } = {}) {
  const element = () => ({
    hidden: false,
    disabled: false,
    textContent: "",
    dataset: {},
    classList: { toggle() {} },
    handlers: {},
    addEventListener(type, callback) { this.handlers[type] = callback; },
  });
  const posted = [];
  const timers = [];
  const listeners = new Set();
  const top = {
    postMessage(message, origin) { posted.push({ message, origin }); },
  };
  const calls = { refreshed: 0, notified: 0 };
  const context = vm.createContext({
    passkeyAddButton: element(),
    passkeyStatusNode: element(),
    currentAccess: {},
    apiHomeToken: "system-launch",
    homeParentOrigin: homeOrigin,
    hasShellAccess: () => shellAccess,
    setGuestRegistrationControlState() {},
    refreshAccountList: async () => { calls.refreshed += 1; },
    notifyHomeSummaryChanged: () => { calls.notified += 1; },
    window: {
      top,
      crypto: { randomUUID: () => "add-request-1" },
      setTimeout: (callback) => { timers.push(callback); return timers.length; },
      clearTimeout: () => {},
      addEventListener: (type, listener) => { if (type === "message") listeners.add(listener); },
      removeEventListener: (type, listener) => { if (type === "message") listeners.delete(listener); },
    },
  });
  vm.runInContext(controller, context);
  const reply = (data, { origin = homeOrigin, source = top } = {}) => {
    for (const listener of [...listeners]) listener({ data, origin, source });
  };
  return { context, posted, timers, listeners, reply, calls, top };
}

const settle = () => new Promise((resolve) => setImmediate(resolve));

test("only an owner with System open from Home sees Add passkey", () => {
  const owner = fixture();
  owner.context.setAccessPolicy({ role: "admin" });
  assert.equal(owner.context.passkeyAddButton.hidden, false);
  owner.context.setAccessPolicy({ role: "guest" });
  assert.equal(owner.context.passkeyAddButton.hidden, true);

  const detached = fixture({ shellAccess: false });
  detached.context.setAccessPolicy({ role: "admin" });
  assert.equal(detached.context.passkeyAddButton.hidden, true);
});

test("Add passkey asks Home with the System launch and reports the new passkey", async () => {
  const f = fixture();
  const done = f.context.onPasskeyAdd();
  assert.equal(f.context.passkeyAddButton.disabled, true, "one add at a time");
  assert.deepEqual(JSON.parse(JSON.stringify(f.posted)), [{
    message: { type: "elastos.home.passkey-add.request/v1", requestId: "add-request-1", homeToken: "system-launch" },
    origin: homeOrigin,
  }]);
  f.reply({ type: "elastos.home.passkey-add.result/v1", requestId: "add-request-1", role: "admin" });
  await done;
  assert.equal(f.context.passkeyStatusNode.textContent, "Passkey added. You can sign in with it now.");
  assert.equal(f.context.passkeyStatusNode.dataset.tone, "success");
  assert.equal(f.calls.refreshed, 1);
  assert.equal(f.calls.notified, 1);
  assert.equal(f.context.passkeyAddButton.disabled, false);
  assert.equal(f.listeners.size, 0);
});

test("Home's plain sentence is shown when the add fails", async () => {
  const f = fixture();
  const done = f.context.onPasskeyAdd();
  f.reply({
    type: "elastos.home.passkey-add.result/v1",
    requestId: "add-request-1",
    error: "This device already has this passkey. Use another device or security key.",
  });
  await done;
  assert.equal(
    f.context.passkeyStatusNode.textContent,
    "This device already has this passkey. Use another device or security key.",
  );
  assert.equal(f.context.passkeyStatusNode.dataset.tone, "error");
  assert.equal(f.calls.refreshed, 0);
});

test("System listens only to Home's answer to its own request", async () => {
  const f = fixture();
  const done = f.context.onPasskeyAdd();
  const result = { type: "elastos.home.passkey-add.result/v1", requestId: "add-request-1", role: "admin" };
  f.reply(result, { origin: "http://localhost:9999" });
  f.reply(result, { source: {} });
  f.reply({ ...result, requestId: "someone-else" });
  await settle();
  assert.equal(f.listeners.size, 1, "still waiting for Home");
  f.reply({ ...result, extra: true });
  await done;
  assert.equal(f.context.passkeyStatusNode.textContent, "Home did not add the passkey. Select Add passkey to try again.");
  assert.equal(f.calls.refreshed, 0);
});

test("an add that Home never answers times out with a next step", async () => {
  const f = fixture();
  const done = f.context.onPasskeyAdd();
  assert.equal(f.timers.length, 1);
  f.timers[0]();
  await done;
  assert.equal(
    f.context.passkeyStatusNode.textContent,
    "Adding the passkey timed out. Select Add passkey to try again.",
  );
  assert.equal(f.listeners.size, 0);
  assert.equal(f.context.passkeyAddButton.disabled, false);
});

test("System outside Home cannot add a passkey", async () => {
  const f = fixture();
  f.context.window.top = f.context.window;
  await f.context.onPasskeyAdd();
  assert.equal(f.posted.length, 0);
  assert.equal(f.context.passkeyStatusNode.textContent, "Open System from Home to add a passkey.");
});
