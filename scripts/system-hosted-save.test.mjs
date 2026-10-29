import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { randomUUID } from "node:crypto";
import { test } from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../capsules/system/browser/system.js", import.meta.url), "utf8");
const controller = source.slice(source.indexOf("function configureAiProvider() {"), source.indexOf("function configurePasskeyAccess() {"));
const keyCheckOutcomes = source.slice(source.indexOf("const HOSTED_KEY_CHECK_PENDING ="), source.indexOf("function showError("));
const homeRuntimeEvents = source.match(/^function homeRuntimeEventsFrom\([\s\S]*?^}$/m)?.[0] ?? "";
const HOME_ORIGIN = "https://home.example";

function fixture(connections = [], guest = false, staged = [], status = {}) {
  const nodes = new Map();
  const element = () => ({
    value: "", hidden: false, disabled: false, dataset: {}, selectedOptions: [], children: [], handlers: {},
    focus() {}, contains() { return false; },
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
  const saved = new Map(), messages = [], attempts = [], discarded = [], reads = [], revoked = [];
  const timers = new Map(), listeners = new Map(), top = {};
  let timerId = 0, gate = null;
  let stagedConnections = staged.slice();
  let rejectActivation = true;
  let consentPending = false, rejectKey = false, failDiscard = false, failReads = false;
  const stateNode = element();
  const document = {
    hidden: false, querySelector: node, createElement: element,
    querySelectorAll: selector => selector === '[data-field="ai-provider-state"]' ? [stateNode] : [],
    addEventListener: (type, handler) => listeners.set(`document:${type}`, handler),
  };
  const context = vm.createContext({
    crypto: { randomUUID }, document, homeParentOrigin: HOME_ORIGIN,
    window: {
      top,
      addEventListener: (type, handler) => listeners.set(type, handler),
      setTimeout: handler => { timers.set(++timerId, handler); return timerId; },
    },
    hasShellAccess: () => true,
    readText: value => typeof value === "string" ? value.trim() : "",
    setTextFields: (field, message) => { if (field === "ai-provider-state") messages.push(message); },
    shellHeaders: extra => extra || {}, publicSystemError: (_, fallback) => fallback,
    openCapsuleTarget: () => {},
    fetchJson: async (_url, init = {}) => {
      if (init.method && gate) await gate.promise;
      if (_url.endsWith("/approval-lens/revoke")) {
        revoked.push(JSON.parse(init.body).id);
        return { approval: "ended" };
      }
      if (init.method === "DELETE" && _url.endsWith("/staged")) {
        if (failDiscard) throw new Error("request failed: 500 staged hosted connection unavailable");
        const { id } = JSON.parse(init.body);
        discarded.push(id);
        stagedConnections = stagedConnections.filter(entry => entry.id !== id);
        return { discarded: true };
      }
      if (init.method !== "POST") {
        reads.push(_url);
        if (guest) throw new Error("request failed: 403 admin passkey required");
        if (failReads) throw new Error("request failed: 503 service unavailable");
        // Each read is a fresh reply, as over HTTP.
        return structuredClone({ ...status, connections, staged_connections: stagedConnections });
      }
      const body = JSON.parse(init.body);
      attempts.push(body);
      if (rejectKey && _url.endsWith("/validate")) {
        // Runtime stages the checked key (typed, earlier staged, or saved) before the provider answers.
        if (!stagedConnections.some(entry => entry.id === body.id)) {
          stagedConnections.push({ id: body.id, provider: body.provider,
            has_saved_model: connections.some(entry => entry.id === body.id) });
        }
        throw new Error("request failed: 400 invalid OpenRouter key");
      }
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
  vm.runInContext(`${keyCheckOutcomes}\n${homeRuntimeEvents}\n${controller}\nconfigureAiProvider();`, context);
  const runTimers = async () => {
    for (const [id, handler] of [...timers]) {
      timers.delete(id);
      await handler();
    }
    await new Promise(setImmediate);
  };
  const hold = () => {
    let release;
    gate = { promise: new Promise(resolve => { release = resolve; }) };
    return () => { gate = null; release(); };
  };
  return { node, saved, messages, attempts, discarded, stateNode, reads, revoked, document, top, runTimers, hold,
    message: event => listeners.get("message")(event),
    shown: () => listeners.get("document:visibilitychange")(),
    activate: () => { rejectActivation = false; }, pending: () => { consentPending = true; },
    rejectKey: () => { rejectKey = true; }, failDiscard: () => { failDiscard = true; },
    failReads: () => { failReads = true; },
    staged: () => stagedConnections };
}

test("saved hosted connections keep management controls while Inbox owns access", async () => {
  const f = fixture([{
    id: "model:hosted-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", name: "My hosted model",
    provider: "openrouter", processor_label: "OpenRouter", selected_model: "fixture/model",
    connected: true, operation: "text.generate", egress_state: "consent_required",
  }], false, [], { hosted_external_https: "consent_required" });
  await new Promise(setImmediate);
  const card = f.node("#ai-provider-instances").children[0];
  const controls = [];
  const collect = element => {
    controls.push(element.textContent);
    for (const child of element.children) collect(child);
  };
  collect(card);
  assert.ok(controls.includes("My hosted model"));
  assert.ok(controls.includes("Needs Inbox review"));
  assert.ok(!controls.some(text => /^Instance: /.test(text || "")));
  assert.ok(controls.includes("Disconnect"));
  assert.ok(controls.includes("Edit"));
  assert.equal(f.node("#ai-provider-egress-note").hidden, true);
  assert.equal(f.node("#ai-provider-egress-note").textContent, "");
  assert.equal(f.attempts.length, 0);
});

test("saved hosted card labels only states that need attention", async () => {
  const card = (id, fields) => ({ id: `model:hosted-${id.repeat(32)}`, name: id, provider: "openrouter",
    processor_label: "OpenRouter", selected_model: "fixture/model", connected: true, operation: "text.generate", ...fields });
  const f = fixture([
    card("a", { egress_approval_state: "ended", egress_state: "consent_required" }),
    card("b", { egress_state: "paused" }),
    card("c", { egress_state: "ready", approval_state: "approved", share_enabled: true }),
  ]);
  await new Promise(setImmediate);
  const labels = f.node("#ai-provider-instances").children
    .map(node => node.children[0].children.map(child => child.textContent));
  assert.deepEqual(labels, [["Hosted approval ended"], ["External HTTPS paused"], []]);
  // Services is hidden, so a card shared by an earlier version offers no way into it.
  const shared = f.node("#ai-provider-instances").children[2];
  assert.deepEqual([...shared.children[2].children, ...shared.children[3].children[1].children].map(node => node.textContent),
    ["Use in Assistant", "Edit", "End hosted approval", "Disconnect"]);
});

test("Inbox approval prompts are neutral; real failures are errors", async () => {
  const f = fixture();
  f.pending();
  f.node("#ai-provider-add").handlers.click();
  await f.node("#ai-provider-validate").handlers.click();
  assert.equal(f.messages.at(-1), "Approve this connection in Inbox, then check the key again.");
  assert.equal(f.stateNode.dataset.tone, "");
  await f.node("#ai-provider-save").handlers.click();
  assert.equal(f.messages.at(-1), "Approve this connection in Inbox, then select Save again.");
  assert.equal(f.stateNode.dataset.tone, "");
  const g = fixture();
  g.node("#ai-provider-add").handlers.click();
  await g.node("#ai-provider-validate").handlers.click();
  assert.equal(g.messages.at(-1), "This Home could not check the key. Try again.");
  assert.equal(g.stateNode.dataset.tone, "error");
});

test("guest setup does not keep an entered key or offer an unusable Add form", async () => {
  const f = fixture([], true);
  f.node("#ai-provider-key").value = "guest-key";
  await new Promise(setImmediate);
  assert.equal(f.node("#ai-provider-key").value, "");
  assert.equal(f.node("#ai-provider-add").hidden, true);
  assert.equal(f.node("#ai-provider-form").hidden, true);
  assert.match(f.messages.at(-1), /not available for this account/);
  assert.equal(f.attempts.length, 0);
});

test("System Models offers no Approval Lens evaluator choice", () => {
  const page = readFileSync(new URL("../capsules/system/browser/index.html", import.meta.url), "utf8");
  const styles = readFileSync(new URL("../capsules/system/browser/style.css", import.meta.url), "utf8");
  for (const text of [page, source, styles]) {
    assert.doesNotMatch(text, /approval-lens(?!\/revoke)|Approval Lens|evaluator/i);
  }
  // Ending a hosted approval still uses the shared revoke route.
  assert.match(source, /\/api\/apps\/system\/approval-lens\/revoke/);
});

test("hosted Save keeps one identity and entered key across activation failure and retry", async () => {
  const f = fixture();
  f.node("#ai-provider-add").handlers.click();
  f.node("#ai-provider-name").value = "Chat";
  f.node("#ai-provider-model").value = "fixture/model";
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

test("hosted key check names each known Runtime outcome and hides unknown detail", () => {
  const context = vm.createContext({ readText: value => typeof value === "string" ? value.trim() : "" });
  vm.runInContext(keyCheckOutcomes, context);
  const message = detail => vm.runInContext(
    `hostedProviderValidationError(new Error(${JSON.stringify(detail)}))`, context,
  );
  // Each Runtime response below is copied from elastos-server api/gateway_home_system_ai_provider.rs.
  const outcomes = [
    ["request failed: 400 Approve this hosted connection in Inbox, then check the key again.",
      "Approve this connection in Inbox, then check the key again."],
    ["request failed: 400 The hosted connection request was denied. Try again after the decision window.",
      "This connection was denied in Inbox. Try again later."],
    ["request failed: 400 Hosted access was denied or ended. Review Inbox.",
      "This connection was denied or ended. Review Inbox."],
    ["request failed: 400 Hosted HTTPS was ended on this Home. Start a new connection check in Inbox.",
      "Hosted access was ended. Check the key again to ask in Inbox."],
    ["request failed: 400 Runtime blocked this hosted route. Review the connection configuration.",
      "This Home blocked the route to the host."],
    ["request failed: 400 Hosted HTTPS could not reach the host. Check the network and try again.",
      "Could not reach the host. Check the network and try again."],
    ["request failed: 400 invalid Venice key", "The provider rejected this key."],
    ["request failed: 400 invalid OpenRouter key", "The provider rejected this key."],
    ["request failed: 500 Venice validation unavailable", "The provider sent an unexpected response. Try again later."],
    ["request failed: 500 OpenRouter validation unavailable", "The provider sent an unexpected response. Try again later."],
    ["request failed: 403 admin passkey required", "Sign in with the Home admin passkey to check keys."],
    ["request failed: 403 proof-bound passkey session required", "Sign in with the Home admin passkey to check keys."],
    ["request failed: 400 Hosted external HTTPS is paused until Runtime network authority is available.",
      "External HTTPS is paused on this Home."],
  ];
  for (const [detail, expected] of outcomes) assert.equal(message(detail), expected, detail);
  const generic = "This Home could not check the key. Try again.";
  for (const detail of [
    "request failed: 400 This Home could not check the hosted connection. Try again.",
    "request failed: 403 home launch token expired",
    "request failed: 502 Bad Gateway",
    "request failed: 500 OpenRouter validation transport unset",
    "request failed: 400 invalid AI provider",
    "request failed: 500 error sending request for url (https://api.venice.ai/api/v1/models?key=sk-secret)",
    "",
  ]) assert.equal(message(detail), generic, detail);
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
  assert.equal(f.node("#ai-provider-cancel").textContent, "Close");
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
  assert.equal(f.messages.at(-1), "Staged key removed.");
});

test("Edit uses the existing identity with a blank key and marks the key field as optional", async () => {
  const id = `model:hosted-${"a".repeat(32)}`;
  const f = fixture([{ id, name: "Text model", provider: "openrouter", connected: true,
    selected_model: "openai/gpt-4o-mini", operation: "text.generate" }]);
  await new Promise(setImmediate);
  const card = f.node("#ai-provider-instances").children[0];
  card.children[2].children.find(button => button.textContent === "Edit").handlers.click();
  assert.equal(f.messages.at(-1), "");
  assert.equal(f.node("#ai-provider-key").placeholder, "Leave blank to keep the stored key");
  assert.equal(f.node("#ai-provider-kind").disabled, true);
  assert.equal(f.node("#ai-provider-key").value, "");
  f.activate();
  await f.node("#ai-provider-save").handlers.click();
  assert.equal(f.attempts[0].id, id);
  assert.equal(f.attempts[0].api_key, "");
  assert.equal(f.attempts[0].model, "openai/gpt-4o-mini");
});

const veniceCard = () => ({ id: `model:hosted-${"c".repeat(32)}`, name: "Venice", provider: "venice",
  processor_label: "Venice", selected_model: "venice/model", connected: true, operation: "text.generate",
  egress_state: "consent_required", egress_approval_state: "none" });
const APPROVED = { egress_approval_state: "approved", egress_state: "ready" };
const ENDED = { egress_approval_state: "none", egress_state: "consent_required" };
const homeEvents = (...events) => ({ type: "elastos:runtime-events", schema: "elastos.home.runtime-events/v1", events });
const INBOX_CHANGED = homeEvents({ scope: "home", kind: "home.summary.changed" }, { scope: "inbox", kind: "inbox.changed" });
// Card children: title, detail, actions, "Details and access" (summary, access controls).
const accessControls = f => f.node("#ai-provider-instances").children[0].children[3].children[1].children;
const accessLabels = f => accessControls(f).map(node => node.textContent);

test("an Inbox decision reaches open System through Home's inbox event or System being shown again", async () => {
  const venice = veniceCard();
  const f = fixture([venice]);
  await new Promise(setImmediate);
  assert.deepEqual(accessLabels(f), ["Disconnect"]);
  const reads = f.reads.length;
  Object.assign(venice, APPROVED); // The person approves the new request in Inbox.
  f.message({ source: f.top, origin: "https://foreign.example", data: INBOX_CHANGED });
  f.message({ source: {}, origin: HOME_ORIGIN, data: INBOX_CHANGED });
  f.message({ source: f.top, origin: HOME_ORIGIN, data: { ...INBOX_CHANGED, schema: "elastos.other/v1" } });
  f.message({ source: f.top, origin: HOME_ORIGIN,
    data: homeEvents({ scope: "chat-room", kind: "chat-room.changed" }, { scope: "people", kind: "people.changed" }) });
  await f.runTimers();
  assert.equal(f.reads.length, reads, "foreign, framed-sibling, malformed and unrelated messages do not read");
  assert.deepEqual(accessLabels(f), ["Disconnect"]);
  f.message({ source: f.top, origin: HOME_ORIGIN, data: INBOX_CHANGED });
  f.message({ source: f.top, origin: HOME_ORIGIN, data: homeEvents({ scope: "inbox", kind: "inbox.changed" }) });
  await f.runTimers();
  assert.equal(f.reads.length, reads + 1, "a burst of Inbox events shares one read");
  assert.deepEqual(accessLabels(f), ["End hosted approval", "Disconnect"]);
  Object.assign(venice, ENDED); // Ended in another window while System was hidden.
  f.document.hidden = true;
  f.shown();
  f.message({ source: f.top, origin: HOME_ORIGIN, data: INBOX_CHANGED });
  await f.runTimers();
  assert.equal(f.reads.length, reads + 1, "a hidden System does not read");
  f.document.hidden = false;
  f.shown();
  await f.runTimers();
  assert.equal(f.reads.length, reads + 2);
  assert.deepEqual(accessLabels(f), ["Disconnect"]);
});

test("a live refresh waits for the person's open form and for the page's own request", async () => {
  const venice = veniceCard();
  const f = fixture([venice]);
  await new Promise(setImmediate);
  const reads = f.reads.length;
  f.node("#ai-provider-add").handlers.click();
  f.node("#ai-provider-name").value = "Typed name";
  f.node("#ai-provider-key").value = "typed-key";
  Object.assign(venice, APPROVED);
  f.message({ source: f.top, origin: HOME_ORIGIN, data: INBOX_CHANGED });
  f.shown();
  await f.runTimers();
  assert.equal(f.reads.length, reads, "nothing is read or rebuilt under the open form");
  assert.equal(f.node("#ai-provider-form").hidden, false);
  assert.equal(f.node("#ai-provider-name").value, "Typed name");
  assert.equal(f.node("#ai-provider-key").value, "typed-key");
  f.node("#ai-provider-cancel").handlers.click();
  await f.runTimers();
  assert.equal(f.reads.length, reads + 1, "closing the form catches up once");
  assert.deepEqual(accessLabels(f), ["End hosted approval", "Disconnect"]);

  const card = f.node("#ai-provider-instances").children[0];
  const end = accessControls(f).find(node => node.textContent === "End hosted approval");
  const release = f.hold();
  const ending = end.handlers.click();
  Object.assign(venice, ENDED);
  f.message({ source: f.top, origin: HOME_ORIGIN, data: INBOX_CHANGED });
  await f.runTimers();
  assert.equal(f.reads.length, reads + 1, "no read while End is in flight");
  assert.equal(f.node("#ai-provider-instances").children[0], card, "the card End acts on is not rebuilt");
  assert.equal(end.disabled, true);
  release();
  await ending;
  await f.runTimers();
  assert.deepEqual(f.revoked, [venice.id]);
  assert.equal(f.reads.length, reads + 3, "End reads once, then the held Inbox event reads once");
  assert.deepEqual(accessLabels(f), ["Disconnect"]);
});

test("a key the provider rejects is not kept staged; a check waiting for Inbox keeps its key behind Close", async () => {
  const f = fixture();
  f.rejectKey();
  f.node("#ai-provider-add").handlers.click();
  f.node("#ai-provider-key").value = "test";
  await f.node("#ai-provider-validate").handlers.click();
  assert.equal(f.messages.at(-1), "The provider rejected this key.");
  assert.equal(f.stateNode.dataset.tone, "error");
  assert.deepEqual(f.discarded, [f.attempts[0].id], "the rejected key is discarded through the staged-key path");
  assert.equal(f.node("#ai-provider-form").hidden, false, "the form stays open for another key");
  assert.equal(f.node("#ai-provider-cancel").textContent, "Cancel");
  assert.equal(f.node("#ai-provider-kind").disabled, false, "the next key check starts a new connection");
  await f.node("#ai-provider-validate").handlers.click();
  assert.notEqual(f.attempts[1].id, f.attempts[0].id);

  const g = fixture();
  g.pending();
  g.node("#ai-provider-add").handlers.click();
  await g.node("#ai-provider-validate").handlers.click();
  assert.equal(g.messages.at(-1), "Approve this connection in Inbox, then check the key again.");
  assert.deepEqual(g.discarded, []);
  assert.equal(g.node("#ai-provider-cancel").textContent, "Close");
});

test("a key the provider rejects while editing a saved connection is discarded; Save with a blank field cannot send it", async () => {
  const saved = { ...veniceCard(), ...APPROVED, provider: "openrouter", processor_label: "OpenRouter" };
  const f = fixture([saved]);
  await new Promise(setImmediate);
  f.node("#ai-provider-instances").children[0].children[2].children.find(node => node.textContent === "Edit").handlers.click();
  f.rejectKey();
  f.node("#ai-provider-key").value = "rejected-key";
  await f.node("#ai-provider-validate").handlers.click();
  assert.equal(f.messages.at(-1), "The provider rejected this key.");
  assert.deepEqual(f.discarded, [saved.id]);
  assert.deepEqual(f.staged(), []);
  assert.equal(f.node("#ai-provider-cancel").textContent, "Cancel");
  f.node("#ai-provider-key").value = "";
  f.activate();
  await f.node("#ai-provider-save").handlers.click();
  const save = f.attempts.at(-1);
  assert.equal(save.id, saved.id, "Edit keeps its saved connection");
  assert.equal(save.api_key, "");
  assert.deepEqual(f.staged(), [], "no rejected key is left for a blank-field Save to send");
});

test("a blank-field check of the saved key that the provider rejects removes nothing", async () => {
  const saved = { ...veniceCard(), ...APPROVED, provider: "openrouter", processor_label: "OpenRouter" };
  const f = fixture([saved]);
  await new Promise(setImmediate);
  f.node("#ai-provider-instances").children[0].children[2].children.find(node => node.textContent === "Edit").handlers.click();
  f.rejectKey();
  await f.node("#ai-provider-validate").handlers.click();
  assert.deepEqual(f.discarded, []);
  assert.equal(f.messages.at(-1), "The provider rejected this key.");
  assert.equal(f.stateNode.dataset.tone, "error");
});

test("a rejected key that cannot be removed is reported and stays on its card", async () => {
  const f = fixture();
  f.rejectKey();
  f.failDiscard();
  f.node("#ai-provider-add").handlers.click();
  f.node("#ai-provider-key").value = "test";
  await f.node("#ai-provider-validate").handlers.click();
  const id = f.attempts[0].id;
  assert.equal(f.messages.at(-1), "The rejected key could not be removed. Select Close, then Discard staged key on its card.");
  assert.equal(f.stateNode.dataset.tone, "error");
  assert.equal(f.node("#ai-provider-cancel").textContent, "Close", "the form keeps the staged connection");
  assert.equal(f.node("#ai-provider-kind").disabled, true);
  f.node("#ai-provider-cancel").handlers.click();
  const card = f.node("#ai-provider-instances").children[0];
  assert.equal(card.children[0].textContent, "openrouter · Key check needed");
  assert.equal(card.children[2].textContent, "Discard staged key");
  assert.deepEqual(f.staged().map(entry => entry.id), [id]);
});

test("a rejected key that cannot be removed while status cannot load points to a Home reload", async () => {
  const f = fixture();
  await new Promise(setImmediate);
  f.rejectKey();
  f.failDiscard();
  f.node("#ai-provider-add").handlers.click();
  f.node("#ai-provider-key").value = "test";
  f.failReads();
  await f.node("#ai-provider-validate").handlers.click();
  assert.equal(f.messages.at(-1), "The rejected key could not be removed. Reload Home, then select Discard staged key on its card.");
  assert.equal(f.stateNode.dataset.tone, "error");
  assert.equal(f.node("#ai-provider-cancel").textContent, "Cancel", "the shown status has no staged card yet");
  assert.deepEqual(f.staged().map(entry => entry.id), [f.attempts[0].id]);
});

test("a live refresh that changes the cards clears an earlier success line; a neutral line stays", async () => {
  const venice = veniceCard();
  const stagedId = `model:hosted-${"d".repeat(32)}`;
  const f = fixture([venice], false, [{ id: stagedId, provider: "openrouter", has_saved_model: false }]);
  await new Promise(setImmediate);
  const stagedCard = f.node("#ai-provider-instances").children[1];
  await stagedCard.children[2].handlers.click();
  assert.equal(f.messages.at(-1), "Staged key removed.");
  f.message({ source: f.top, origin: HOME_ORIGIN, data: INBOX_CHANGED });
  await f.runTimers();
  assert.equal(f.messages.at(-1), "Staged key removed.", "an unchanged card list keeps the line");
  Object.assign(venice, APPROVED);
  f.message({ source: f.top, origin: HOME_ORIGIN, data: INBOX_CHANGED });
  await f.runTimers();
  assert.deepEqual(accessLabels(f), ["End hosted approval", "Disconnect"]);
  assert.equal(f.messages.at(-1), "", "the success line no longer describes the cards");

  const other = { ...veniceCard(), ...APPROVED };
  const g = fixture([other]);
  await new Promise(setImmediate);
  await accessControls(g).find(node => node.textContent === "End hosted approval").handlers.click();
  const ended = "Hosted approval ended. A later request needs Inbox review again.";
  assert.equal(g.messages.at(-1), ended);
  Object.assign(other, ENDED, { egress_approval_state: "pending" });
  g.message({ source: g.top, origin: HOME_ORIGIN, data: INBOX_CHANGED });
  await g.runTimers();
  assert.equal(g.reads.length, 3, "the Inbox request changed the card");
  assert.deepEqual(accessLabels(g), ["End hosted approval", "Disconnect"]);
  assert.equal(g.messages.at(-1), ended, "a neutral line stays");
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

const updateFunctions = source.slice(source.indexOf("function configureRuntimeUpdate() {"), source.indexOf("function readQueryParam("));
const settleUpdateTasks = () => new Promise(resolve => setImmediate(resolve));

function updateFixture(overrides = {}) {
  const fields = new Map(), hiddenFields = new Map(), timers = new Map(), listeners = new Map();
  const posts = [], approvals = [];
  let timerId = 0, now = 0;
  const button = { hidden: true, disabled: true, addEventListener(type, handler) { this[type] = handler; } };
  const panel = { hidden: true }, status = { textContent: "" }, top = {};
  const context = vm.createContext({
    AbortController,
    runtimeUpdateNode: panel, runtimeUpdateButton: button, runtimeUpdateStatusNode: status,
    runtimeUpdate: undefined, runtimeUpdatePending: null, runtimeUpdateBusy: false,
    runtimeUpdateActive: true, runtimeUpdateTimer: 0, runtimeUpdateReconnects: 0,
    runtimeUpdateRequests: new Set(), runtimeUpdateInProgressSeen: false, runtimeUpdateFeedback: "",
    RUNTIME_UPDATE_RESTART_TEXT: source.match(/RUNTIME_UPDATE_RESTART_TEXT = "([^"]+)"/)?.[1],
    RUNTIME_UPDATE_RESTART_POLLS: Number(source.match(/RUNTIME_UPDATE_RESTART_POLLS = (\d+)/)?.[1]),
    systemSummaryInFlight: null, homeParentOrigin: "https://home.example",
    document: { hidden: false, addEventListener: (type, handler) => listeners.set(`document:${type}`, handler) },
    window: {
      top, crypto: { randomUUID },
      addEventListener: (type, handler) => listeners.set(type, handler),
      setTimeout: (handler, delay) => { timers.set(++timerId, { handler, delay, due: now + delay }); return timerId; },
      clearTimeout: id => timers.delete(id),
      location: { reloads: 0, reload() { this.reloads += 1; } },
    },
    readText: value => typeof value === "string" ? value.trim() : "",
    setTextFields: (field, value) => fields.set(field, value),
    setHiddenFields: (field, value) => hiddenFields.set(field, value),
    shellHeaders: extra => ({ "x-elastos-home-token": "system-token", ...extra }),
    requestPasskeyStepUp: async (operation, intent) => { approvals.push({ operation, intent: JSON.parse(JSON.stringify(intent)) }); return "approved-exact-choice"; },
    fetchJson: async (url, init) => {
      posts.push({ url, ...init });
      return init.method === "POST" ? { id: JSON.parse(init.body).request_id, phase: "queued" } : {};
    },
    refreshSystemSummary: async () => {},
    ...overrides,
  });
  vm.runInContext(updateFunctions, context);
  const offer = {
    configured: true, available: true, can_apply: true, source_name: "publisher",
    channel: "stable", publisher: "did:key:verified", publisher_did: "did:key:verified",
    current_version: "1.0.0", new_version: "1.1.0", head_cid: "head-exact", release_cid: "release-exact",
    changes: ["A signed change.", "Another signed change."], controller: { phase: "ready", id: null },
  };
  context.configureRuntimeUpdate();
  context.renderRuntimeUpdate(offer);
  const advance = async ms => {
    const target = now + ms;
    for (;;) {
      const next = [...timers].filter(([, timer]) => timer.due <= target).sort((a, b) => a[1].due - b[1].due)[0];
      if (!next) break;
      timers.delete(next[0]);
      now = next[1].due;
      await next[1].handler();
      await Promise.resolve();
    }
    now = target;
  };
  return { context, offer, button, panel, status, fields, hiddenFields, timers, listeners, posts, approvals, top, advance };
}

test("Home update requires a verified complete offer and a controller that can apply", () => {
  const f = updateFixture();
  assert.equal(f.button.hidden, false);
  assert.equal(f.fields.get("update-current-version"), "1.0.0");
  assert.equal(f.fields.get("update-new-version"), "1.1.0");
  assert.equal(f.fields.get("update-publisher"), "did:key:verified");
  assert.equal(f.fields.get("update-changes"), "A signed change.\nAnother signed change.");
  for (const refusal of [null, { configured: false }, { available: false }, { can_apply: false },
    { configured: "true" }, { publisher_did: "" }, { release_cid: "" }, { controller: null }, { controller: { phase: "restarting" } }]) {
    f.context.renderRuntimeUpdate(refusal === null ? null : { ...f.offer, ...refusal });
    assert.equal(f.button.hidden, true, JSON.stringify(refusal));
    assert.equal(f.button.disabled, true);
  }
  for (const phase of ["ready", "updated", "restored", "failed"]) {
    f.context.renderRuntimeUpdate({ ...f.offer, controller: { phase } });
    assert.equal(f.button.hidden, false, phase);
  }
  f.context.renderRuntimeUpdate({ ...f.offer, changes: [] });
  assert.equal(f.fields.get("update-changes"), "Publisher has not supplied change notes.");
  f.context.renderRuntimeUpdate({ ...f.offer, available: false, message: "Home is up to date." });
  assert.equal(f.hiddenFields.get("update-changes"), true);
  assert.equal(f.status.textContent, "Home is up to date.");
  f.context.renderRuntimeUpdate({ configured: true, available: false, can_apply: false,
    controller: { phase: "restarting", current_version: "1.0.0", new_version: "1.1.0" } });
  assert.equal(f.hiddenFields.get("update-offer"), false, "reopened System keeps durable progress versions");
  assert.equal(f.fields.get("update-current-version"), "1.0.0");
  assert.equal(f.fields.get("update-new-version"), "1.1.0");
  assert.equal(f.hiddenFields.get("update-publisher-row"), true, "progress does not invent a publisher");
  f.context.renderRuntimeUpdate(null);
  assert.equal(f.timers.size, 0, "source Home without an installed controller has no update polling");
});

test("guest System summary keeps update controls and discovery polling hidden", async () => {
  const f = updateFixture();
  f.context.renderRuntimeUpdate(null);
  assert.equal(f.panel.hidden, true);
  assert.equal(f.button.hidden, true);
  assert.equal(f.button.disabled, true);
  await f.context.onRuntimeUpdateApply();
  await f.advance(60_000);
  assert.equal(f.approvals.length, 0);
  assert.equal(f.posts.length, 0);
  assert.equal(f.timers.size, 0);
});

test("Home update freezes one exact passkey intent and retries a lost response with identical bytes", async () => {
  let approve;
  let attempt = 0;
  const f = updateFixture({ requestPasskeyStepUp: (operation, intent) => {
    f.approvals.push({ operation, intent: JSON.parse(JSON.stringify(intent)) });
    return new Promise(resolve => { approve = resolve; });
  } });
  f.context.fetchJson = async (url, init) => {
    f.posts.push({ url, ...init });
    if (++attempt === 1) throw new TypeError("response lost");
    return { id: JSON.parse(init.body).request_id, phase: "queued" };
  };
  const apply = f.context.onRuntimeUpdateApply();
  await f.context.onRuntimeUpdateApply();
  assert.equal(f.approvals.length, 1);
  assert.equal(f.posts.length, 0, "approval precedes mutation");
  assert.equal(f.button.disabled, true);
  f.context.renderRuntimeUpdate({ ...f.offer, new_version: "1.2.0", release_cid: "later-release" });
  approve("approved-exact-choice");
  await apply;
  assert.equal(f.posts.length, 2);
  assert.equal(f.posts[0].url, "/api/apps/system/summary");
  assert.equal(f.posts[0].method, "POST");
  assert.equal(f.posts[0].body, f.posts[1].body);
  assert.equal(f.approvals[0].operation, "system.update.apply");
  const intent = f.approvals[0].intent;
  assert.match(intent.request_id, /^[a-f0-9]{32}$/);
  assert.deepEqual(Object.keys(intent).sort(), ["action", "request_id", "source_name", "channel", "publisher_did", "current_version", "new_version", "head_cid", "release_cid"].sort());
  assert.deepEqual(JSON.parse(f.posts[0].body), { ...intent, step_up_token: "approved-exact-choice" });
  assert.equal(intent.release_cid, "release-exact");
  assert.equal(intent.new_version, "1.1.0");
  assert.equal(f.fields.get("update-new-version"), "1.1.0", "progress retains the approved choice");
  await f.context.onRuntimeUpdateApply();
  assert.equal(f.approvals.length, 1, "queued choice has one action");
  f.context.renderRuntimeUpdate({ ...f.offer, controller: { phase: "updated", id: "another-choice", message: "Other result" } });
  assert(f.context.runtimeUpdatePending, "another request result cannot retire this choice");
  f.context.renderRuntimeUpdate({ ...f.offer, available: false, controller: { phase: "updated", id: intent.request_id, message: "Home is up to date." } });
  assert.equal(f.context.runtimeUpdatePending, null);
  assert.equal(f.status.textContent, "Home is up to date.");
  assert.equal(f.context.window.location.reloads, 1, "the finished update loads the new System");
});

test("System reloaded by its update says to reload Home; opened again it does not", () => {
  const navigation = type => ({ performance: { getEntriesByType: kind => kind === "navigation" ? [{ type }] : [] } });
  const updated = { phase: "updated", id: "a".repeat(32), new_version: "1.1.0", message: "Home is up to date." };
  const reloaded = updateFixture(navigation("reload"));
  reloaded.context.renderRuntimeUpdate({ ...reloaded.offer, available: false, current_version: "1.1.0", controller: updated });
  assert.equal(reloaded.status.textContent, "Updated to 1.1.0. Reload Home to finish.");
  assert.equal(reloaded.context.window.location.reloads, 0, "System never reloads Home or itself for it");
  for (const [overrides, controller, text] of [
    [navigation("navigate"), updated, "Home is up to date."],
    [{}, updated, "Home is up to date."],
    [navigation("reload"), { ...updated, phase: "restored", message: "The previous Runtime is ready." }, "The previous Runtime is ready."],
    [navigation("reload"), { ...updated, phase: "failed", message: "The update failed." }, "The update failed."],
  ]) {
    const f = updateFixture(overrides);
    f.context.renderRuntimeUpdate({ ...f.offer, available: false, controller });
    assert.equal(f.status.textContent, text, controller.phase);
  }
});

test("Home update keeps an ambiguous submission pending and stops bounded progress recovery", async () => {
  for (const status of [undefined, 503]) {
    const f = updateFixture();
    const bodies = [];
    f.context.fetchJson = async (_url, init) => {
      bodies.push(init.body);
      const error = new TypeError("connection closed"); error.status = status; throw error;
    };
    await f.context.onRuntimeUpdateApply();
    assert(f.context.runtimeUpdatePending);
    assert.equal(bodies.length, 2);
    assert.equal(bodies[0], bodies[1]);
    assert.equal(f.button.disabled, true);
    assert.match(f.status.textContent, /Waiting for Home to confirm/);
    assert(!f.status.textContent.includes("restarting"));
    f.context.systemSummaryInFlight = Promise.resolve();
    const polls = f.context.runtimeUpdatePending.polls;
    await f.context.pollRuntimeUpdate();
    assert.equal(f.context.runtimeUpdatePending.polls, polls, "overlapping refresh does not consume recovery attempts");
    f.context.systemSummaryInFlight = null;
    f.context.runtimeUpdatePending.polls = 89;
    const before = f.timers.size;
    await f.context.pollRuntimeUpdate();
    assert.equal(f.timers.size, before, "exhausted recovery adds no timer");
    assert.match(f.status.textContent, /has not confirmed/);
    assert.match(f.status.textContent, /Open System from Home again/);
  }
});

test("Home update cancellation and definitive refusal each allow a fresh approved choice", async () => {
  const f = updateFixture({ requestPasskeyStepUp: async () => { throw new Error("passkey cancelled"); } });
  await f.context.onRuntimeUpdateApply();
  assert.equal(f.posts.length, 0);
  assert.equal(f.context.runtimeUpdatePending, null);
  f.context.requestPasskeyStepUp = async (operation, intent) => { f.approvals.push({ operation, intent }); return "approved"; };
  f.context.fetchJson = async () => { const error = new Error("private failure detail"); error.status = 409; throw error; };
  await f.context.onRuntimeUpdateApply();
  const first = f.approvals[0].intent.request_id;
  assert.equal(f.context.runtimeUpdatePending, null);
  assert(!f.status.textContent.includes("private"));
  await f.context.onRuntimeUpdateApply();
  assert.notEqual(f.approvals[1].intent.request_id, first);
});

test("slow Carrier admission stays neutral beyond thirty seconds and can still refuse the choice", async () => {
  const f = updateFixture();
  let refuse;
  f.context.fetchJson = (_url, init) => {
    f.posts.push(init);
    return new Promise((_resolve, reject) => { refuse = reject; });
  };
  const applying = f.context.onRuntimeUpdateApply();
  await Promise.resolve();
  await f.advance(35_000);
  assert.equal(f.posts.length, 1, "slow admission retains one submission");
  assert.equal(f.posts[0].signal.aborted, false);
  assert([...f.timers.values()].some(timer => timer.delay === 60_000));
  assert.equal(f.context.runtimeUpdatePending.dispatched, false);
  f.context.renderRuntimeUpdate({ ...f.offer, controller: { id: "another-request", phase: "restarting" } });
  assert.match(f.status.textContent, /Checking the update with Home/);
  assert(!f.status.textContent.includes("restarting"), "another controller request cannot prove this restart");
  const error = new Error("signed choice refused"); error.status = 409;
  refuse(error);
  await applying;
  assert.equal(f.posts.length, 1, "a definite refusal does not replay");
  assert.equal(f.context.runtimeUpdatePending, null);
  assert.equal(f.context.runtimeUpdateRequests.size, 0);
  assert.match(f.status.textContent, /could not start/);
  assert.equal(f.button.disabled, true, "the other active request controls action availability");
});

test("matching queue confirmation and controller state prove progress after slow admission", async () => {
  const f = updateFixture();
  let confirm;
  f.context.fetchJson = (_url, init) => {
    f.posts.push(init);
    return new Promise(resolve => { confirm = resolve; });
  };
  const applying = f.context.onRuntimeUpdateApply();
  await Promise.resolve();
  await f.advance(35_000);
  const id = f.context.runtimeUpdatePending.intent.request_id;
  confirm({ id, phase: "queued" });
  await applying;
  assert.equal(f.context.runtimeUpdatePending.dispatched, true);
  assert.match(f.status.textContent, /update is queued/);
  assert(!f.status.textContent.includes("restarting"));
  f.context.renderRuntimeUpdate({ ...f.offer, controller: { id, phase: "staging" } });
  assert.match(f.status.textContent, /Checking the signed update/);
  f.context.refreshSystemSummary = async () => { throw new TypeError("response lost"); };
  await f.context.pollRuntimeUpdate();
  assert.equal(f.status.textContent, "Restarting to finish the update. Home is usually back within two minutes.");
  f.context.renderRuntimeUpdate({ ...f.offer, controller: { id, phase: "restarting" } });
  assert.equal(f.status.textContent, "Restarting to finish the update. Home is usually back within two minutes.");
  await f.context.pollRuntimeUpdate();
  assert.equal(f.status.textContent, "Restarting to finish the update. Home is usually back within two minutes.");
});

test("only a matching queue reply confirms dispatch and an ambiguous deadline preserves the exact retry", async () => {
  const f = updateFixture();
  f.context.fetchJson = async (_url, init) => {
    f.posts.push(init);
    return { id: "unrelated-request", phase: "queued" };
  };
  await f.context.onRuntimeUpdateApply();
  assert.equal(f.posts.length, 2);
  assert.equal(f.posts[0].body, f.posts[1].body);
  assert.equal(f.context.runtimeUpdatePending.dispatched, false);
  assert.match(f.status.textContent, /Waiting for Home to confirm/);
  const g = updateFixture();
  g.context.fetchJson = (_url, init) => {
    g.posts.push(init);
    return new Promise((_resolve, reject) => {
      init.signal.addEventListener("abort", () => reject(new Error("admission deadline")));
    });
  };
  const applying = g.context.onRuntimeUpdateApply();
  await settleUpdateTasks();
  for (let attempt = 0; attempt < 2; attempt++) {
    const deadline = [...g.timers.values()].find(timer => timer.delay === 60_000);
    assert(deadline, "each exact submission has a sixty-second deadline");
    deadline.handler();
    await settleUpdateTasks();
  }
  await applying;
  assert.equal(g.posts.length, 2);
  assert.equal(g.posts[0].body, g.posts[1].body);
  assert.equal(g.approvals.length, 1);
  assert.equal(g.context.runtimeUpdateRequests.size, 0);
  assert.equal(g.context.runtimeUpdatePending.dispatched, false);
  assert.match(g.status.textContent, /Waiting for Home to confirm/);
});

test("closing System aborts its owned submission and summary requests without a replay", async () => {
  const f = updateFixture();
  f.context.fetchJson = (_url, init) => {
    f.posts.push(init);
    return new Promise((_resolve, reject) => {
      init.signal.addEventListener("abort", () => reject(new Error("document closed")));
    });
  };
  const applying = f.context.onRuntimeUpdateApply();
  await settleUpdateTasks();
  const reading = f.context.fetchRuntimeUpdateJson({ headers: {} });
  const readClosed = assert.rejects(reading, /document closed/);
  assert.equal(f.context.runtimeUpdateRequests.size, 2);
  f.listeners.get("pagehide")();
  await Promise.all([applying, readClosed]);
  assert.equal(f.posts.length, 2, "one POST and one GET stop with their document");
  assert(f.posts.every(request => request.signal.aborted));
  assert.equal(f.context.runtimeUpdateRequests.size, 0);
  assert.equal(f.timers.size, 0, "request deadlines and polling timers are released");
  await f.context.pollRuntimeUpdate();
  assert.equal(f.posts.length, 2, "a stale poll cannot restart a closed document");
  const g = updateFixture({ requestPasskeyStepUp: () => new Promise(resolve => { g.approve = resolve; }) });
  const approving = g.context.onRuntimeUpdateApply();
  g.listeners.get("pagehide")();
  g.approve("approved");
  await approving;
  assert.equal(g.posts.length, 0, "late passkey approval cannot submit after page close");
});

test("Home update polling binds host messages, stops at auth refusal, and closes with its document", async () => {
  const f = updateFixture();
  const message = { type: "elastos:runtime-events", schema: "elastos.home.runtime-events/v1", events: [] };
  const timer = f.context.runtimeUpdateTimer;
  f.listeners.get("message")({ source: {}, origin: "https://home.example", data: message });
  assert.equal(f.context.runtimeUpdateTimer, timer);
  f.listeners.get("message")({ source: f.top, origin: "https://foreign.example", data: message });
  assert.equal(f.context.runtimeUpdateTimer, timer);
  f.listeners.get("message")({ source: f.top, origin: "https://home.example", data: message });
  assert.equal(f.timers.get(f.context.runtimeUpdateTimer).delay, 0);
  f.context.renderRuntimeUpdate({ ...f.offer, can_apply: false, controller: { phase: "ready" } });
  assert.equal(f.timers.get(f.context.runtimeUpdateTimer).delay, 5000);
  f.context.refreshSystemSummary = async () => { const error = new Error("expired"); error.status = 403; throw error; };
  f.timers.clear();
  await f.context.pollRuntimeUpdate();
  assert.equal(f.timers.size, 0);
  assert.match(f.status.textContent, /access has changed/);
  assert.equal(f.button.hidden, true);
  f.listeners.get("pagehide")();
  f.context.scheduleRuntimeUpdateRefresh();
  assert.equal(f.timers.size, 0);
});

test("System explains the restart wait, outlasts the controller restart budget, and reloads once when Home is back", async () => {
  for (const back of ["updated", "restored"]) {
    const f = updateFixture();
    await f.context.onRuntimeUpdateApply();
    const id = f.context.runtimeUpdatePending.intent.request_id;
    f.context.renderRuntimeUpdate({ ...f.offer, controller: { id, phase: "restarting" } });
    assert.equal(f.status.textContent, "Restarting to finish the update. Home is usually back within two minutes.");
    assert.equal(f.timers.get(f.context.runtimeUpdateTimer).delay, 2000);
    f.context.refreshSystemSummary = async () => { throw new TypeError("connection refused"); };
    await f.advance(130_000);
    assert.equal(f.status.textContent, "Restarting to finish the update. Home is usually back within two minutes.");
    assert(f.timers.size > 0, "System still polls after the 120 s controller restart budget");
    assert.equal(f.context.window.location.reloads, 0);
    f.context.refreshSystemSummary = async () => f.context.renderRuntimeUpdate(
      { ...f.offer, available: false, controller: { id, phase: back, message: "Home is up to date." } });
    await f.advance(2_000);
    assert.equal(f.context.window.location.reloads, 1);
    assert.equal(f.timers.size, 0, "the reloading page stops polling");
    const fresh = updateFixture();
    fresh.context.renderRuntimeUpdate({ ...fresh.offer, controller: { id, phase: back, message: "Home is up to date." } });
    assert.equal(fresh.context.window.location.reloads, 0, "the reloaded page does not reload again");
  }
  const failed = updateFixture();
  await failed.context.onRuntimeUpdateApply();
  const id = failed.context.runtimeUpdatePending.intent.request_id;
  failed.context.renderRuntimeUpdate({ ...failed.offer, controller: { id, phase: "restarting" } });
  failed.context.renderRuntimeUpdate({ ...failed.offer, controller: { id, phase: "failed", message: "The update failed." } });
  assert.equal(failed.context.window.location.reloads, 0, "a failed update keeps the current page");
  assert.equal(failed.status.textContent, "The update failed.");
});

test("the restart gets the full polling budget however long the download took", async () => {
  const f = updateFixture();
  await f.context.onRuntimeUpdateApply();
  const id = f.context.runtimeUpdatePending.intent.request_id;
  f.context.refreshSystemSummary = async () => f.context.renderRuntimeUpdate(
    { ...f.offer, controller: { id, phase: "downloading", message: "Downloading the update (88 MB)." } });
  await f.advance(600_000);
  f.context.refreshSystemSummary = async () => { throw new TypeError("connection refused"); };
  await f.advance(130_000);
  assert(f.timers.size > 0, "a long download does not shorten the restart wait");
  assert.equal(f.status.textContent, "Restarting to finish the update. Home is usually back within two minutes.");
});

test("once an update is in progress, the update finishing or a session refusal reloads once; failed never does", async () => {
  const refused = () => { throw new TypeError("connection refused"); };
  const status = code => () => { const error = new Error("session"); error.status = code; throw error; };
  const back = (f, id, phase) => () => f.context.renderRuntimeUpdate(
    { ...f.offer, available: false, controller: { id, phase, message: "Home is up to date." } });
  for (const own of [true, false]) {
    for (const [seen, steps, reloads] of [
      ["downloading", f => [status(401)], 1],
      ["downloading", f => [status(403)], 1],
      ["verifying", (f, id) => [back(f, id, "updated")], 1],
      ["downloading", f => [refused, refused, status(401)], 1],
      ["restarting", (f, id) => [refused, back(f, id, "restored")], 1],
      ["staging", (f, id) => [back(f, id, "failed"), status(401)], 0],
    ]) {
      const f = updateFixture();
      let id = "another-tab-request";
      if (own) {
        await f.context.onRuntimeUpdateApply();
        id = f.context.runtimeUpdatePending.intent.request_id;
      }
      f.context.renderRuntimeUpdate({ ...f.offer, controller: { id, phase: seen } });
      const label = `${own ? "own" : "seen"} ${seen} -> ${steps(f, id).length} steps -> ${reloads}`;
      for (const step of steps(f, id)) {
        f.context.refreshSystemSummary = async () => step();
        f.timers.clear();
        await f.context.pollRuntimeUpdate();
        if (step === refused) assert.equal(f.status.textContent, "Restarting to finish the update. Home is usually back within two minutes.", label);
        if (step === refused) assert.equal(f.timers.get(f.context.runtimeUpdateTimer).delay, 2000, label);
      }
      assert.equal(f.context.window.location.reloads, reloads, label);
      if (reloads) assert.equal(f.timers.size, 0, label);
      f.context.renderRuntimeUpdate({ ...f.offer, controller: { id, phase: "updated" } });
      await f.context.pollRuntimeUpdate();
      assert.equal(f.context.window.location.reloads, reloads, `at most one reload: ${label}`);
      assert.equal(f.posts.filter(post => post.method === "POST").length, own ? 1 : 0, `no resend: ${label}`);
    }
  }
  const fresh = updateFixture();
  fresh.context.refreshSystemSummary = async () => status(401)();
  await fresh.context.pollRuntimeUpdate();
  assert.match(fresh.status.textContent, /access has changed/, "a page that saw no update in progress keeps the dead-end message");
  fresh.context.renderRuntimeUpdate({ ...fresh.offer, controller: { id: "earlier", phase: "updated" } });
  assert.equal(fresh.context.window.location.reloads, 0, "nor does an earlier update's result reload it");
  const queued = updateFixture();
  await queued.context.onRuntimeUpdateApply();
  queued.context.renderRuntimeUpdate({ ...queued.offer, controller: { id: "earlier-update", phase: "updated" } });
  assert.equal(queued.context.window.location.reloads, 0, "an earlier update's result does not reload a queued request");
});

test("an auth refusal right after a restart reloads once instead of a dead end", async () => {
  const f = updateFixture();
  await f.context.onRuntimeUpdateApply();
  const id = f.context.runtimeUpdatePending.intent.request_id;
  f.context.renderRuntimeUpdate({ ...f.offer, controller: { id, phase: "restarting" } });
  f.context.refreshSystemSummary = async () => { const error = new Error("session"); error.status = 401; throw error; };
  f.timers.clear();
  await f.context.pollRuntimeUpdate();
  assert.equal(f.context.window.location.reloads, 1);
  assert.equal(f.timers.size, 0);
  assert.equal(f.posts.filter(post => post.method === "POST").length, 1, "the reload replays no update request");
});

test("Home update summary refreshes share one bounded request", async () => {
  const f = updateFixture();
  let finish, reads = 0, communityReads = 0, presenceReads = 0;
  f.context.fetchJson = () => { reads++; return new Promise(resolve => { finish = resolve; }); };
  f.context.refreshCommunityMembership = async () => { communityReads++; };
  f.context.refreshCommunityPresence = async () => { presenceReads++; };
  const unavailable = [];
  f.context.showCommunityUnavailable = error => unavailable.push(error.message);
  f.context.renderSystemSummary = value => { f.context.renderRuntimeUpdate(value.runtime_update); };
  const refresh = source.slice(source.indexOf("async function refreshSystemSummary() {"), source.indexOf("async function fetchJson("));
  vm.runInContext(refresh, f.context);
  const first = f.context.refreshSystemSummary();
  const second = f.context.refreshSystemSummary();
  assert.equal(reads, 1);
  assert([...f.timers.values()].some(timer => timer.delay === 10_000));
  finish({ runtime_update: f.offer });
  await Promise.all([first, second]);
  assert.equal(communityReads, 1, "one shared summary refresh also reads current Community membership once");
  assert.equal(presenceReads, 1, "and the person's own Community choice once");
  assert.equal(f.context.systemSummaryInFlight, null);
  assert(![...f.timers.values()].some(timer => timer.delay === 10_000));
  f.context.fetchJson = (_url, init) => new Promise((_resolve, reject) => {
    init.signal.addEventListener("abort", () => reject(new Error("request deadline")));
  });
  const hung = f.context.refreshSystemSummary();
  const deadline = [...f.timers.values()].find(timer => timer.delay === 10_000);
  deadline.handler();
  await assert.rejects(hung, /request deadline/);
  assert.deepEqual(unavailable, ["request deadline"], "a failed summary shows Community as unavailable instead of hiding it");
  assert.equal(f.context.systemSummaryInFlight, null, "timed-out request releases the next refresh");
  assert(![...f.timers.values()].some(timer => timer.delay === 10_000));
});

test("System startup and recovery stay ready while the background Carrier check is pending or offline", async () => {
  const boot = source.slice(source.indexOf("async function boot() {"), source.indexOf("function configureHomeRecoverySave() {"));
  for (const checking of [true, false]) {
    const calls = [];
    const top = {};
    const context = vm.createContext({
      hasShellAccess: () => true,
      requestedSettingsTab: "account", initialRecoveryState: null,
      window: { parent: top, top }, homeClipboard: { start: () => false },
      refreshSystemSummary: async () => {
        calls.push(checking ? "summary-checking" : "summary-offline");
      },
      refreshActiveShell: async () => calls.push("shell"),
      refreshAccountList: async () => calls.push("accounts"),
      refreshRecoveryStatus: async () => calls.push("recovery"),
      refreshChainNetworks: async () => calls.push("chains"),
    });
    for (const name of ["configureSettingsTabs", "configureSettingsSearch", "activateSettingsTab",
      "configureAppearanceEditor", "configureAppearancePreferences", "configureGuestAccess", "configureCommunityMembership", "configureCommunityPresence",
      "configureAiProvider", "configurePasskeyAccess", "configureRecoveryAccess", "configureChainAccess",
      "configureActiveShell", "configureTechnicalDetails", "configureDeviceDidCopy",
      "configureRuntimeUpdate", "configureHomeRecoverySave"]) context[name] = () => {};
    vm.runInContext(boot, context);
    await context.boot();
    await context.initialRecoveryState;
    assert.deepEqual(calls, [checking ? "summary-checking" : "summary-offline",
      "shell", "accounts", "recovery", "chains"]);
  }
});

test("System shows matching download and verification progress while Home stays connected", async () => {
  const f = updateFixture();
  await f.context.onRuntimeUpdateApply();
  const id = f.context.runtimeUpdatePending.intent.request_id;
  for (const [phase, message] of [
    ["downloading", "Downloading the update (88 MB). Home restarts when the update is ready."],
    ["verifying", "Verifying the update. Home restarts when the update is ready."],
  ]) {
    f.context.renderRuntimeUpdate({ ...f.offer, controller: { id, phase, message } });
    assert.equal(f.status.textContent, message);
    assert.equal(f.context.runtimeUpdatePending.phase, phase);
    assert.equal(f.button.disabled, true);
    const reopened = updateFixture();
    reopened.context.renderRuntimeUpdate({ ...reopened.offer, controller: { id, phase, message } });
    assert.equal(reopened.status.textContent, message);
    assert.equal(reopened.button.disabled, true);
  }
  const message = "Home could not reach the update source. Your current version is unchanged. Connect to the internet and select Update again.";
  f.context.renderRuntimeUpdate({ ...f.offer, controller: { id, phase: "failed", message } });
  assert.equal(f.status.textContent, message);
  assert.equal(f.context.runtimeUpdatePending, null);
  assert.equal(f.button.disabled, false);
});

test("System cancellation explains the unchanged Home and keeps private errors out of update copy", async () => {
  for (const error of [Object.assign(new Error("private detail"), { name: "NotAllowedError" }),
    new Error("Passkey verification was cancelled."), new Error("The operation timed out or was not allowed.")]) {
    const f = updateFixture({ requestPasskeyStepUp: async () => { throw error; } });
    await f.context.onRuntimeUpdateApply();
    assert.equal(f.status.textContent, "You cancelled the update. Home is unchanged.");
    assert.equal(f.posts.length, 0);
    assert.equal(f.button.disabled, false);
    // The next refresh (#392) keeps it readable; only the next Update replaces it.
    f.context.renderRuntimeUpdate(f.offer);
    assert.equal(f.status.textContent, "You cancelled the update. Home is unchanged.");
    f.context.requestPasskeyStepUp = () => new Promise(() => {});
    f.context.onRuntimeUpdateApply();
    assert.equal(f.status.textContent, "Verify your passkey to start the update.");
  }
  const f = updateFixture({ requestPasskeyStepUp: async () => { throw new Error("private signing path"); } });
  await f.context.onRuntimeUpdateApply();
  assert.match(f.status.textContent, /verification failed/);
  assert(!f.status.textContent.includes("private"));
});

test("System explains an unreachable source or another tab's update before queueing and allows a new choice", async () => {
  for (const [status, text] of [
    [424, "Home could not reach the update source. Your current version is unchanged. Connect to the internet and select Update again."],
    [423, "Another update was started on this Home. Wait for it to finish, then check the update again."],
    [410, "This approval is no longer valid. Check for the update again and approve it once more."],
  ]) {
    const f = updateFixture();
    f.context.fetchJson = async (_url, init) => {
      f.posts.push(init);
      const error = new Error("private server detail"); error.status = status; throw error;
    };
    await f.context.onRuntimeUpdateApply();
    assert.equal(f.status.textContent, text);
    assert.equal(f.posts.length, 1, "a definite refusal is not resubmitted");
    assert.equal(f.context.runtimeUpdatePending, null);
    assert.equal(f.button.disabled, false);
    assert.equal(f.approvals.length, 1);
  }
});

test("a slow update keeps polling while Home reports the approved download", async () => {
  const f = updateFixture();
  await f.context.onRuntimeUpdateApply();
  const id = f.context.runtimeUpdatePending.intent.request_id;
  const message = "Downloading the update (88 MB). Home restarts when the update is ready.";
  f.context.refreshSystemSummary = async () => f.context.renderRuntimeUpdate({ ...f.offer, controller: { id, phase: "downloading", message } });
  for (let poll = 0; poll < 100; poll += 1) await f.context.pollRuntimeUpdate();
  assert.equal(f.status.textContent, message);
  assert.equal(f.context.runtimeUpdatePending.polls, 0);
  assert.equal(f.button.disabled, true);
});

function communityFixture(role = "admin") {
  const section = { hidden: true }, button = { disabled: true, textContent: "" };
  const fields = new Map(), hidden = new Map(), requests = [], timers = new Map();
  let nextTimer = 0;
  const context = vm.createContext({
    communitySection: section, communityButton: button,
    currentAccess: { role }, hasShellAccess: () => true,
    readText: value => typeof value === "string" ? value.trim() : "",
    setTextFields: (key, value) => fields.set(key, value),
    setHiddenFields: (key, value) => hidden.set(key, value),
    shellHeaders: value => value || {}, publicSystemError: (_, fallback) => fallback,
    AbortController, runtimeUpdateRequests: new Set(),
    window: { setTimeout: (handler, delay) => { const id = ++nextTimer; timers.set(id, { handler, delay }); return id; }, clearTimeout: id => timers.delete(id) },
    fetchJson: async (path, init) => {
      requests.push({ path, ...init });
      return { schema: "elastos.community-membership/v1", configured: true, joined: false, detail: "Left" };
    },
  });
  const start = source.indexOf("function renderCommunityMembership(");
  const end = source.indexOf("async function onGuestRegistrationChange", start);
  const fetchStart = source.indexOf("async function fetchRuntimeUpdateJson(");
  const fetchEnd = source.indexOf("async function onRuntimeUpdateApply(", fetchStart);
  vm.runInContext(`let communityMembership = null; let communityBusy = false; ${source.slice(start, end)} ${source.slice(fetchStart, fetchEnd)}`, context);
  context.renderCommunityMembership({ schema: "elastos.community-membership/v1", configured: true, joined: true, detail: "Joined" });
  return { context, section, button, fields, hidden, requests, timers };
}

test("Community uses one owner request and a visible Rejoin control", async () => {
  const f = communityFixture();
  let finish;
  f.context.fetchJson = async (path, init) => {
    f.requests.push({ path, ...init });
    return new Promise(resolve => { finish = resolve; });
  };
  const pending = f.context.updateCommunityMembership();
  assert.equal(f.button.disabled, true);
  await f.context.updateCommunityMembership();
  assert.equal(f.requests.length, 1);
  assert.equal(f.requests[0].path, "/api/apps/system/community");
  assert.equal(JSON.parse(f.requests[0].body).joined, false);
  assert.equal([...f.timers.values()][0].delay, 10_000, "the local owner choice has a bounded response wait");
  finish({ schema: "elastos.community-membership/v1", configured: true, joined: false, detail: "Left" });
  await pending;
  assert.equal(f.button.textContent, "Rejoin Community");
  assert.equal(f.button.disabled, false);
  assert.equal(f.fields.get("community-detail"), "Left");
  assert.equal(f.section.hidden, false);
  assert.equal(f.timers.size, 0);
});

test("Community guests cannot dispatch and refused owner changes retain the last accepted state", async () => {
  const guest = communityFixture("guest");
  await guest.context.updateCommunityMembership();
  assert.equal(guest.requests.length, 0);
  assert.equal(guest.button.disabled, true);
  const owner = communityFixture();
  owner.context.fetchJson = async () => { throw new Error("request failed: 401"); };
  await owner.context.updateCommunityMembership();
  assert.equal(owner.button.textContent, "Leave Community");
  assert.equal(owner.button.disabled, false);
  assert.equal(owner.fields.get("community-detail"), "Joined");
  assert.equal(owner.hidden.get("community-status"), false);
});

test("Community load failure is visible when configured and stays hidden on a Home without Community", async () => {
  const none = communityFixture();
  none.context.renderCommunityMembership({ schema: "elastos.community-membership/v1", configured: false, joined: false });
  none.context.fetchJson = async () => { throw new Error("request failed: 500"); };
  await none.context.refreshCommunityMembership();
  none.context.showCommunityUnavailable(new Error("summary failed"));
  assert.equal(none.section.hidden, true, "a confirmed unconfigured Home stays hidden after a later failure");
  assert.equal(none.hidden.get("community-status"), undefined);

  const f = communityFixture();
  f.context.fetchJson = async () => { throw new Error("request failed: 500"); };
  await f.context.refreshCommunityMembership();
  assert.equal(f.section.hidden, false);
  assert.equal(f.hidden.get("community-status"), false);
  assert.equal(f.fields.get("community-status"), "Community setting is unavailable.");
  assert.equal(f.button.disabled, true);
});

test("Community missing configuration stays hidden and malformed state cannot enable its control", () => {
  const f = communityFixture();
  f.context.renderCommunityMembership({ schema: "elastos.community-membership/v1", configured: false, joined: false });
  assert.equal(f.section.hidden, true);
  assert.equal(f.button.disabled, true);
  assert.throws(() => f.context.renderCommunityMembership({ schema: "foreign", configured: true, joined: true }));
  assert.equal(f.button.disabled, true);
});

test("Show me in Community offers three modes, sets only the caller's own and reverts on refusal", async () => {
  const row = { hidden: true }, section = { hidden: true };
  const option = mode => {
    const attrs = new Map([["aria-checked", "false"]]), classes = new Set();
    return {
      dataset: { presenceMode: mode }, disabled: true, classes,
      classList: { toggle: (name, on) => (on ? classes.add(name) : classes.delete(name)) },
      setAttribute: (key, value) => attrs.set(key, value), getAttribute: key => attrs.get(key),
    };
  };
  const options = ["always", "while_open", "hidden"].map(option);
  const checked = () => options.filter(o => o.getAttribute("aria-checked") === "true").map(o => o.dataset.presenceMode);
  const fields = new Map(), hidden = new Map(), requests = [];
  let reply = { schema: "elastos.community-presence/v1", configured: true, joined: true, community_presence: "requested" };
  const context = vm.createContext({
    communitySection: section, communityPresenceRow: row, communityPresenceOptions: options, hasShellAccess: () => true,
    setTextFields: (key, value) => fields.set(key, value),
    setHiddenFields: (key, value) => hidden.set(key, value),
    shellHeaders: value => value || {}, publicSystemError: (_, fallback) => fallback,
    AbortController, runtimeUpdateRequests: new Set(),
    window: { setTimeout: () => 1, clearTimeout: () => {} },
    fetchJson: async (path, init) => { requests.push({ path, ...init }); return reply; },
  });
  const start = source.indexOf("function configureCommunityPresence(");
  const end = source.indexOf("function configureAiProvider(", start);
  const fetchStart = source.indexOf("async function fetchRuntimeUpdateJson(");
  const fetchEnd = source.indexOf("async function onRuntimeUpdateApply(", fetchStart);
  vm.runInContext(`let communityPresenceBusy = false; var communityMembership = null; ${source.slice(start, end)} ${source.slice(fetchStart, fetchEnd)}`, context);

  await context.refreshCommunityPresence();
  assert.equal(row.hidden, false);
  assert.deepEqual(checked(), [], "an open request shows no choice until the person makes one");
  assert.ok(options.every(o => !o.disabled));

  reply = { ...reply, community_presence: "while_open" };
  await context.updateCommunityPresence("while_open");
  const post = requests.at(-1);
  assert.equal(post.path, "/api/apps/system/community/presence");
  assert.deepEqual(JSON.parse(post.body), { mode: "while_open" });
  assert.deepEqual(checked(), ["while_open"]);
  assert.ok(options.find(o => o.dataset.presenceMode === "while_open").classes.has("active"));

  context.fetchJson = async () => { throw new Error("request failed: 409"); };
  await context.updateCommunityPresence("always");
  assert.deepEqual(checked(), ["while_open"], "a refused change keeps the last accepted choice");
  assert.equal(hidden.get("community-presence-status"), false);

  context.fetchJson = async () => ({ ...reply, community_presence: "refused" });
  await context.refreshCommunityPresence();
  assert.deepEqual(checked(), ["hidden"], "Never in Inbox shows as Hidden");

  context.fetchJson = async () => ({ ...reply, joined: false, community_presence: "always" });
  await context.refreshCommunityPresence();
  assert.ok(options.every(o => o.disabled), "a Home that left Community cannot change it until it rejoins");

  row.hidden = true;
  context.fetchJson = async () => ({ ...reply, configured: false });
  await context.refreshCommunityPresence();
  assert.equal(row.hidden, true, "a Home without Community keeps the row hidden");
  assert.equal(section.hidden, true);
  context.fetchJson = async () => { throw new Error("request failed: 500"); };
  await context.refreshCommunityPresence();
  assert.equal(section.hidden, false, "a presence load failure is visible");
  assert.equal(row.hidden, false);
  assert.equal(hidden.get("community-presence-status"), false);
  assert.ok(options.every(o => o.disabled));

  section.hidden = true; row.hidden = true;
  context.communityMembership = { configured: false };
  let reads = 0;
  context.fetchJson = async () => { reads++; throw new Error("request failed: 500"); };
  await context.refreshCommunityPresence();
  assert.equal(reads, 0, "a Home without Community never reads presence");
  assert.equal(section.hidden, true, "and a presence failure cannot reveal it");
});
