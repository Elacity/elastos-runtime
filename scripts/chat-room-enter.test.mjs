import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../capsules/chat-room/browser/chat-chrome.js", import.meta.url), "utf8");

const DESKTOP = { screen: { width: 1440, height: 900 }, coarse: false };
const PHONE = { screen: { width: 390, height: 844 }, coarse: true };
const PHONE_SIDEWAYS = { screen: { width: 844, height: 390 }, coarse: true };

function makeElement(id) {
  const listeners = new Map();
  return {
    id, hidden: false, disabled: false, dataset: {}, style: {}, value: "", scrollHeight: 34,
    listeners,
    addEventListener(type, callback) { listeners.set(type, callback); },
    removeEventListener() {},
    setAttribute() {}, getAttribute() { return null; }, toggleAttribute() {},
    classList: { add() {}, remove() {}, toggle() {}, contains() { return false; } },
    querySelector() { return null; }, querySelectorAll() { return []; },
    closest() { return null; }, contains() { return false; }, focus() {},
  };
}

function loadChat({ screen, coarse, formFactor }) {
  const nodes = new Map();
  const get = (id) => {
    if (!nodes.has(id)) {
      nodes.set(id, makeElement(id));
    }
    return nodes.get(id);
  };
  class FakeButton {}
  const send = Object.assign(new FakeButton(), makeElement("send-button"));
  nodes.set("send-button", send);
  const submits = [];
  get("composer-form").requestSubmit = (button) => submits.push(button);
  const document = {
    documentElement: { dataset: formFactor ? { elFormFactor: formFactor } : {} },
    body: makeElement("body"),
    getElementById: get,
    querySelector() { return null; }, querySelectorAll() { return []; },
    addEventListener() {},
  };
  const window = {
    location: { search: "", hash: "" },
    screen,
    matchMedia: (query) => ({ matches: coarse && /pointer: coarse|hover: none/.test(query) }),
    addEventListener() {},
    parent: { postMessage() {} },
    top: { postMessage() {} },
    document,
  };
  const context = {
    URLSearchParams, console, document, window,
    HTMLElement: Object, HTMLTextAreaElement: Object, HTMLButtonElement: FakeButton,
    MutationObserver: class { observe() {} },
    requestAnimationFrame: (callback) => callback(), setTimeout, clearTimeout,
  };
  context.globalThis = context;
  vm.createContext(context);
  vm.runInContext(source, context);
  const keydown = get("message-input").listeners.get("keydown");
  assert.ok(keydown, "the composer listens for keydown");
  const press = (extra = {}) => {
    let prevented = false;
    keydown({
      key: "Enter", shiftKey: false, altKey: false, ctrlKey: false, metaKey: false,
      isComposing: false, keyCode: 13, preventDefault() { prevented = true; }, ...extra,
    });
    return prevented;
  };
  return { press, submits, send };
}

test("Enter sends through the Send button on a desktop screen", () => {
  const chat = loadChat(DESKTOP);
  assert.equal(chat.press(), true);
  assert.deepEqual(chat.submits, [chat.send]);
});

test("Shift+Enter, IME Enter and a disabled Send keep the message", () => {
  const chat = loadChat(DESKTOP);
  chat.press({ shiftKey: true });
  chat.press({ isComposing: true });
  chat.press({ keyCode: 229 });
  chat.send.disabled = true;
  chat.press();
  assert.equal(chat.submits.length, 0);
});

test("a phone's Return adds a line before the shell names the form factor", () => {
  for (const device of [PHONE, PHONE_SIDEWAYS]) {
    const chat = loadChat(device);
    assert.equal(chat.press(), false, JSON.stringify(device.screen));
    assert.equal(chat.submits.length, 0, JSON.stringify(device.screen));
  }
});

test("the shell's form factor decides once it is set", () => {
  const phone = loadChat({ ...DESKTOP, formFactor: "phone" });
  assert.equal(phone.press(), false);
  assert.equal(phone.submits.length, 0);
  const desktop = loadChat({ ...PHONE, formFactor: "desktop" });
  assert.equal(desktop.press(), true);
  assert.equal(desktop.submits.length, 1);
});
