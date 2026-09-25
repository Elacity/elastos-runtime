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
