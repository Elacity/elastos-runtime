import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { randomUUID } from "node:crypto";
import { test } from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../capsules/system/browser/system.js", import.meta.url), "utf8");
const controller = source.slice(source.indexOf("function configureAiProvider() {"), source.indexOf("function configurePasskeyAccess() {"));

function fixture(connections = []) {
  const nodes = new Map();
  const element = () => ({
    value: "", hidden: false, disabled: false, dataset: {}, selectedOptions: [], children: [], handlers: {},
    focus() {},
    addEventListener(event, callback) { this.handlers[event] = callback; },
    append(...items) { this.children.push(...items); },
    replaceChildren(...items) { this.children = items; },
    querySelectorAll() { return []; },
  });
  const node = selector => {
    if (!nodes.has(selector)) nodes.set(selector, element());
    return nodes.get(selector);
  };
  const saved = new Map(), messages = [], attempts = [];
  let rejectActivation = true;
  const context = vm.createContext({
    crypto: { randomUUID },
    document: { querySelector: node, querySelectorAll: () => [], createElement: element },
    hasShellAccess: () => true,
    readText: value => typeof value === "string" ? value.trim() : "",
    setTextFields: (field, message) => { if (field === "ai-provider-state") messages.push(message); },
    shellHeaders: extra => extra || {}, publicSystemError: (_, fallback) => fallback,
    openCapsuleTarget: () => {},
    fetchJson: async (_url, init = {}) => {
      if (init.method !== "POST") return { connections };
      const body = JSON.parse(init.body);
      attempts.push(body);
      saved.set(body.id, body);
      if (rejectActivation) throw new Error("request failed: 409 provider error: selection_unavailable: model offer is not available");
      return { connections: [] };
    },
  });
  vm.runInContext(`${controller}\nconfigureAiProvider();`, context);
  return { node, saved, messages, attempts, activate: () => { rejectActivation = false; } };
}

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

test("Cancel after a failed Add restores provider choice and gives the next form a new identity", async () => {
  const f = fixture();
  f.node("#ai-provider-add").handlers.click();
  await f.node("#ai-provider-save").handlers.click();
  assert.equal(f.node("#ai-provider-kind").disabled, true);
  const firstId = f.attempts[0].id;
  f.node("#ai-provider-cancel").handlers.click();
  f.node("#ai-provider-add").handlers.click();
  assert.equal(f.node("#ai-provider-kind").disabled, false);
  f.node("#ai-provider-kind").value = "venice";
  await f.node("#ai-provider-save").handlers.click();
  assert.notEqual(f.attempts[1].id, firstId);
  assert.equal(f.attempts[1].provider, "venice");
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
