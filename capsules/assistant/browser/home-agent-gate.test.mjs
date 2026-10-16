import test from "node:test";
import assert from "node:assert/strict";
import { createHomeMessageGate } from "./home-agent-gate.js";
import { createAssistantFrameReadiness } from "../../home-gui/browser/home-agent-message-contract.js";

const open = (model) => ({ type: "home-agent:open", query: { model } });
const handover = (on) => ({ type: "home-agent:shelf-handover", on });

/* The Assistant document as far as the gate sees it: applying open raises the
   room (once, as the real handler guards) and focuses the composer. */
function assistantDocument() {
  const composer = { readOnly: false, value: "", focused: false };
  const applied = [];
  let raised = false;
  const gate = createHomeMessageGate({
    composer,
    apply(message) {
      applied.push(message);
      if (message.type === "home-agent:open" && !raised) {
        raised = true;
        composer.focused = true;
      }
    },
  });
  const type = (text) => { if (!composer.readOnly) composer.value = text; };
  const readWorkspace = (draft) => { composer.value = draft; gate.initialized(); };
  return { composer, applied, gate, type, readWorkspace, raised: () => raised };
}

test("open and handover before the workspace read are held, then applied once in order", () => {
  const doc = assistantDocument();
  doc.gate.receive(open("a"));
  doc.gate.receive(handover(false));
  doc.gate.receive(open("b"));
  doc.gate.receive(handover(true));
  assert.deepEqual(doc.applied, [], "nothing applies before initialization");
  assert.equal(doc.raised(), false, "the room is not raised");
  assert.equal(doc.composer.focused, false, "the composer is not focused");
  doc.type("too early");
  assert.equal(doc.composer.value, "", "the composer cannot be edited before initialization");
  doc.readWorkspace("saved draft");
  assert.deepEqual(doc.applied, [open("b"), handover(true)], "the latest open, then the latest handover");
  assert.equal(doc.raised(), true);
  doc.type("typed after open");
  assert.equal(doc.composer.value, "typed after open", "the snapshot cannot replace text typed after the room opens");
  doc.gate.initialized();
  assert.equal(doc.applied.length, 2, "initialization applies held messages once");
  doc.gate.receive(handover(false));
  assert.deepEqual(doc.applied.at(-1), handover(false), "after initialization messages apply directly");
});

test("a close before initialization drops the held open and handover", () => {
  const doc = assistantDocument();
  doc.gate.receive(open("a"));
  doc.gate.receive(handover(true));
  doc.gate.receive({ type: "home-agent:close" });
  doc.readWorkspace("");
  assert.deepEqual(doc.applied, [], "a closed room is not raised later");
  assert.equal(doc.composer.readOnly, false);
});

test("legacy ready, then hello(B), open and handover, then ready(B): B raises only after its read", () => {
  const home = createAssistantFrameReadiness();
  const doc = assistantDocument();
  const docId = "b".repeat(32);
  const sendOpen = () => { doc.gate.receive(open("m")); doc.gate.receive(handover(true)); };
  // Document A's queued legacy ready reaches Home after navigation committed to B.
  assert.equal(home.receive({ type: "home-agent:ready" }), "show");
  sendOpen(); // Home's open/handover now address B.
  assert.equal(home.receive({ type: "home-agent:hello", docId }), "hide");
  assert.equal(doc.raised(), false, "B is not raised by messages meant for A");
  assert.equal(doc.composer.focused, false);
  doc.readWorkspace("B draft");
  assert.equal(doc.raised(), true, "B raises after its workspace read");
  assert.equal(home.receive({ type: "home-agent:ready", docId }), "show");
  sendOpen(); // Home answers B's ready with open/handover again.
  assert.equal(doc.applied.filter((m) => m.type === "home-agent:open").length, 2);
  assert.equal(doc.raised(), true, "the room was raised once");
  doc.type("hello");
  assert.equal(doc.composer.value, "hello");
});
