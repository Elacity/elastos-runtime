import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../capsules/chat-room/browser/chat-chrome.js", import.meta.url), "utf8");

function loadChat() {
  const input = { value: "Keep my Community draft", disabled: false, listeners: new Map(),
    addEventListener(type, callback) { this.listeners.set(type, callback); } };
  class FakeButton {}
  const send = Object.assign(new FakeButton(), { disabled: false });
  const status = { hidden: true, textContent: "", dataset: {} };
  const row = { textContent: "A retained Community message" };
  const history = { children: [row], scrollTop: 37 };
  const submits = [];
  const nodes = new Map([
    ["message-input", input], ["send-button", send], ["community-status", status],
    ["message-list", history],
    ["composer-form", { querySelector() { return null; },
      requestSubmit(button) { submits.push(button); } }],
  ]);
  const document = { documentElement: { dataset: { elFormFactor: "desktop" } },
    getElementById: id => nodes.get(id) || null };
  const window = { location: { search: "", hash: "" }, addEventListener() {},
    parent: { postMessage() {} }, top: { postMessage() {} } };
  const context = vm.createContext({ URLSearchParams, document, window,
    HTMLElement: Object, HTMLButtonElement: FakeButton });
  vm.runInContext(source, context);
  const render = context.elastosChatRenderCommunity;
  assert.equal(typeof render, "function", "Rust's Community renderer is available");
  const keydown = input.listeners.get("keydown");
  assert.equal(typeof keydown, "function", "the actual desktop Enter handler is connected");
  return { input, send, status, history, row, submits, render,
    pressEnter() {
      keydown({ key: "Enter", shiftKey: false, altKey: false, ctrlKey: false,
        metaKey: false, isComposing: false, keyCode: 13, preventDefault() {} });
    } };
}

function checkState(chat, state, label, enabled) {
  assert.equal(chat.status.dataset.state, state);
  assert.equal(chat.status.textContent, label);
  assert.equal(chat.status.hidden, state === "live");
  assert.equal(chat.input.disabled, !enabled);
  assert.equal(chat.send.disabled, !enabled);
  assert.equal(chat.input.value, "Keep my Community draft");
  assert.deepEqual(chat.history.children, [chat.row]);
  assert.equal(chat.history.children[0], chat.row);
  assert.equal(chat.row.textContent, "A retained Community message");
  assert.equal(chat.history.scrollTop, 37);
}

test("not joined shows its state and blocks Community Send while keeping history", () => {
  const chat = loadChat();
  chat.render("not_joined", true, true);
  checkState(chat, "not_joined", "Community not joined", false);
  chat.pressEnter();
  assert.deepEqual(chat.submits, []);
});

test("Leave keeps Community history and draft, and leaves Direct controls alone", () => {
  const chat = loadChat();
  chat.render("live", true, true);
  checkState(chat, "live", "Community live", true);
  chat.render("left", true, true);
  checkState(chat, "left", "Left Community", false);
  chat.pressEnter();
  assert.deepEqual(chat.submits, []);
  chat.input.disabled = true;
  chat.send.disabled = false;
  chat.render("left", false, false);
  assert.equal(chat.input.disabled, true);
  assert.equal(chat.send.disabled, false);
  assert.equal(chat.input.value, "Keep my Community draft");
  assert.equal(chat.history.children[0], chat.row);
});

test("joined but unreachable shows the failure and blocks Community Send", () => {
  const chat = loadChat();
  chat.render("joined_unreachable", true, true);
  checkState(chat, "joined_unreachable", "Community unreachable", false);
  chat.pressEnter();
  assert.deepEqual(chat.submits, []);
});

test("live enables one Enter submission while the base Send guard still applies", () => {
  const chat = loadChat();
  chat.render("live", true, true);
  checkState(chat, "live", "Community live", true);
  chat.pressEnter();
  assert.deepEqual(chat.submits, [chat.send]);
  chat.render("live", true, false);
  checkState(chat, "live", "Community live", false);
  chat.pressEnter();
  assert.deepEqual(chat.submits, [chat.send]);
});
