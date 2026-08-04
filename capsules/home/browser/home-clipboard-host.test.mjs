import assert from "node:assert/strict";
import test from "node:test";

import {
  HOME_CLIPBOARD_REQUEST_SCHEMA,
  HOME_CLIPBOARD_REQUEST_TYPE,
  MAX_HOME_CLIPBOARD_PURPOSE_CODE_UNITS,
  MAX_HOME_CLIPBOARD_REPLAY_IDS,
  MAX_HOME_CLIPBOARD_TEXT_UTF8_BYTES,
  createHomeClipboardFrameState,
  createHomeClipboardHost,
  createHomeClipboardPrompt,
} from "./home-clipboard-host.js";

function deferredPrompt() {
  let active = null;
  const requests = [];
  return {
    requests,
    request(request) {
      if (active) {
        const error = new Error("busy");
        error.code = "busy";
        return Promise.reject(error);
      }
      requests.push(request);
      return new Promise((resolve, reject) => {
        active = { request, resolve, reject };
      });
    },
    allow(value = true) {
      const pending = active;
      assert.ok(pending, "expected an active Home prompt");
      active = null;
      pending.resolve(value);
    },
    cancel(requestId, code = "cancelled") {
      if (!active || active.request.requestId !== requestId) {
        return false;
      }
      const pending = active;
      active = null;
      const error = new Error(code);
      error.code = code;
      pending.reject(error);
      return true;
    },
  };
}

function fakeTimers() {
  let nextId = 1;
  const timers = new Map();
  return {
    setTimeoutFn(callback, delay) {
      const id = nextId++;
      timers.set(id, { callback, delay });
      return id;
    },
    clearTimeoutFn(id) {
      timers.delete(id);
    },
    fireDelay(delay) {
      const entry = [...timers.entries()].find(([, timer]) => timer.delay === delay);
      assert.ok(entry, `expected a ${delay}ms timer`);
      timers.delete(entry[0]);
      entry[1].callback();
    },
    size() {
      return timers.size;
    },
  };
}

function fixture({
  targetId = "browser",
  clipboard = null,
  prompt = deferredPrompt(),
  timeoutMs = 50,
  now = () => 1_000,
  registerSource = true,
} = {}) {
  const timers = fakeTimers();
  const posts = [];
  const source = {
    postMessage(message, targetOrigin) {
      posts.push({ message, targetOrigin });
    },
  };
  const state = createHomeClipboardFrameState();
  let generationSequence = 0;
  const context = {
    kind: "app-frame",
    targetId,
    homeToken: `${targetId}-token`,
    origin: "null",
    parentOrigin: "https://home.example",
    source,
    clipboardState: state,
  };
  const host = createHomeClipboardHost({
    clipboard,
    prompt,
    cryptoRef: {
      randomUUID: () => {
        generationSequence += 1;
        return `${targetId}-generation-${generationSequence}`;
      },
    },
    now,
    setTimeoutFn: timers.setTimeoutFn,
    clearTimeoutFn: timers.clearTimeoutFn,
    timeoutMs,
  });
  if (registerSource) {
    host.resetFrame(state, context);
  }
  const event = (data, overrides = {}) => ({
    source,
    origin: "null",
    data,
    ...overrides,
  });
  const results = () =>
    posts.filter(({ message }) => message.type === "home:clipboard-result");
  return {
    context,
    event,
    host,
    posts,
    prompt,
    results,
    source,
    state,
    timers,
  };
}

function requestMessage(f, requestId, {
  operation = "write",
  purpose = "browser.text",
  text = "copy from Browser",
} = {}) {
  const message = {
    type: HOME_CLIPBOARD_REQUEST_TYPE,
    schema: HOME_CLIPBOARD_REQUEST_SCHEMA,
    requestId,
    homeToken: f.context.homeToken,
    parentOrigin: f.context.parentOrigin,
    generation: f.state.generation,
    operation,
    purpose,
    mime_type: "text/plain",
  };
  if (operation === "write") {
    message.text = text;
  }
  return message;
}

async function flushTasks() {
  await new Promise((resolve) => setImmediate(resolve));
}

test("visible Home action gates exact Browser write and read", async () => {
  const writes = [];
  const f = fixture({
    clipboard: {
      async writeText(text) {
        writes.push(text);
      },
      async readText() {
        return "paste into Browser";
      },
    },
  });

  const write = requestMessage(f, "write:1");
  assert.equal(f.host.handle(f.event(write), f.context, write), true);
  assert.deepEqual(writes, []);
  assert.deepEqual(f.prompt.requests, [{
    requestId: "write:1",
    targetId: "browser",
    operation: "write",
    purpose: "browser.text",
  }]);
  f.prompt.allow();
  await flushTasks();
  assert.deepEqual(writes, ["copy from Browser"]);
  assert.equal(f.results()[0].message.ok, true);

  const read = requestMessage(f, "read:1", {
    operation: "read",
    purpose: "browser.text",
  });
  f.host.handle(f.event(read), f.context, read);
  f.prompt.allow();
  await flushTasks();
  assert.equal(f.results()[1].message.text, "paste into Browser");
  assert.equal(f.state.inFlight, null);
  assert.equal(f.timers.size(), 0);
});

test("closed target and purpose policy accepts only approved writes", async () => {
  const cases = [
    ["wallet", "wallet.address", "0x1234"],
    ["wallet", "wallet.recovery-key", '{\n  "schema": "elastos.wallet.recovery-key/v1"\n}'],
    ["wallet-metamask", "wallet.address", "0x2345"],
    ["wallet-unisat", "wallet.address", "bc1qexample"],
    ["wallet-walletconnect", "wallet.address", "0x3456"],
    ["library", "resource.uri", "object:private/document-1"],
    ["library", "resource.identifier", "bafy-library-content"],
    ["documents", "resource.uri", "elastos://bafy-document"],
  ];
  for (const [targetId, purpose, text] of cases) {
    const writes = [];
    const f = fixture({
      targetId,
      clipboard: { async writeText(value) { writes.push(value); } },
    });
    const message = requestMessage(f, `${targetId}:1`, { purpose, text });
    f.host.handle(f.event(message), f.context, message);
    assert.equal(writes.length, 0);
    f.prompt.allow();
    await flushTasks();
    assert.deepEqual(writes, [text]);
    assert.equal(f.results()[0].message.targetId, targetId);
    assert.equal(f.results()[0].message.purpose, purpose);
  }
});

test("Recovery Key prompt classifies secret material without receiving its payload", async () => {
  function node() {
    const listeners = new Map();
    return {
      hidden: true,
      textContent: "",
      addEventListener(type, listener) {
        listeners.set(type, listener);
      },
      click() {
        listeners.get("click")?.();
      },
      setAttribute() {},
      focus() {},
    };
  }
  const root = node();
  const title = node();
  const copy = node();
  const allowButton = node();
  const cancelButton = node();
  const prompt = createHomeClipboardPrompt({
    root,
    title,
    copy,
    allowButton,
    cancelButton,
  });
  const secret = "private_key_hex:do-not-render";
  const decision = prompt.request({
    requestId: "secret:1",
    targetId: "wallet",
    operation: "write",
    purpose: "wallet.recovery-key",
    text: secret,
  });
  assert.match(title.textContent, /Recovery Key/);
  assert.match(copy.textContent, /secret material/);
  assert.equal(`${title.textContent}${copy.textContent}`.includes(secret), false);
  await assert.rejects(
    prompt.request({
      requestId: "concurrent:1",
      targetId: "browser",
      operation: "write",
      purpose: "browser.text",
    }),
    (error) => error?.code === "busy",
  );
  allowButton.click();
  assert.equal(await decision, true);
});

test("Library identifier prompt is explicit and does not receive its value", async () => {
  function node() {
    const listeners = new Map();
    return {
      hidden: true,
      textContent: "",
      addEventListener(type, listener) {
        listeners.set(type, listener);
      },
      click() {
        listeners.get("click")?.();
      },
      setAttribute() {},
      focus() {},
    };
  }
  const root = node();
  const title = node();
  const copy = node();
  const allowButton = node();
  const identifier = "bafy-do-not-render";
  const prompt = createHomeClipboardPrompt({
    root,
    title,
    copy,
    allowButton,
    cancelButton: node(),
  });
  const decision = prompt.request({
    requestId: "identifier:1",
    targetId: "library",
    operation: "write",
    purpose: "resource.identifier",
    text: identifier,
  });
  assert.match(title.textContent, /Library identifier/);
  assert.match(copy.textContent, /technical identifier/);
  assert.equal(`${title.textContent}${copy.textContent}`.includes(identifier), false);
  allowButton.click();
  assert.equal(await decision, true);
});

test("malformed, substituted, oversized, and unsupported requests fail closed", () => {
  const f = fixture();
  const oversized = "a".repeat(MAX_HOME_CLIPBOARD_TEXT_UTF8_BYTES + 1);
  const oversizedPurpose = "p".repeat(
    MAX_HOME_CLIPBOARD_PURPOSE_CODE_UNITS + 1,
  );
  const base = requestMessage(f, "bad:base");
  const messages = [
    { ...base, requestId: "bad:target", targetId: "wallet" },
    { ...base, requestId: "bad:token", homeToken: "substituted" },
    { ...base, requestId: "bad:origin", parentOrigin: "https://attacker.example" },
    { ...base, requestId: "bad:generation", generation: "clipboard:stale" },
    { ...base, requestId: "bad:schema", schema: "wrong/v1" },
    { ...base, requestId: "bad:purpose", purpose: "wallet.address" },
    { ...base, requestId: "bad:proto", purpose: "__proto__" },
    { ...base, requestId: "bad:constructor", purpose: "constructor" },
    { ...base, requestId: "bad:to-string", purpose: "toString" },
    { ...base, requestId: "bad:purpose-size", purpose: oversizedPurpose },
    { ...base, requestId: "bad:operation-size", operation: oversizedPurpose },
    { ...base, requestId: "bad:mime", mime_type: "text/html" },
    { ...base, requestId: "bad:oversized", text: oversized },
    { ...base, requestId: "bad:extra", extra: true },
  ];
  for (const message of messages) {
    f.host.handle(f.event(message), f.context, message);
  }
  assert.equal(f.prompt.requests.length, 0);
  assert.equal(f.results().length, messages.length);
  assert.ok(f.results().every(({ message }) => message.error === "malformed"));
  for (const requestId of [
    "bad:proto",
    "bad:constructor",
    "bad:to-string",
    "bad:purpose-size",
  ]) {
    const result = f.results().find(({ message }) =>
      message.requestId === requestId
    )?.message;
    assert.equal(result?.purpose, "invalid");
    assert.equal(JSON.stringify(result).includes(oversizedPurpose), false);
  }
  const oversizedOperationResult = f.results().find(({ message }) =>
    message.requestId === "bad:operation-size"
  ).message;
  assert.equal(oversizedOperationResult.operation, "write");
  assert.equal(oversizedOperationResult.purpose, "browser.text");
  assert.equal(
    JSON.stringify(oversizedOperationResult).includes(oversizedPurpose),
    false,
  );
  assert.equal(
    Object.hasOwn(
      f.results().find(({ message }) =>
        message.requestId === "bad:oversized"
      ).message,
      "text",
    ),
    false,
  );

  const wallet = fixture({ targetId: "wallet" });
  const deniedRead = requestMessage(wallet, "wallet:read", {
    operation: "read",
    purpose: "wallet.address",
  });
  const invalidAddress = requestMessage(wallet, "wallet:space", {
    purpose: "wallet.address",
    text: "0x12 34",
  });
  wallet.host.handle(wallet.event(deniedRead), wallet.context, deniedRead);
  wallet.host.handle(wallet.event(invalidAddress), wallet.context, invalidAddress);
  assert.equal(wallet.prompt.requests.length, 0);
  assert.equal(wallet.results().length, 2);

  const library = fixture({ targetId: "library" });
  const rawCid = requestMessage(library, "library:raw", {
    purpose: "resource.uri",
    text: "bafy-not-a-uri",
  });
  library.host.handle(library.event(rawCid), library.context, rawCid);
  assert.equal(library.results()[0].message.error, "malformed");
});

test("inherited targets and malformed envelopes are denied without throwing", () => {
  for (const targetId of ["__proto__", "constructor", "toString"]) {
    const f = fixture({ targetId, registerSource: false });
    assert.doesNotThrow(() => f.host.resetFrame(f.state, f.context));
    assert.equal(f.host.resetFrame(f.state, f.context), false);
    assert.equal(f.state.retired, true);
  }

  const f = fixture();
  for (const malformed of [null, undefined, "", [], 7]) {
    assert.doesNotThrow(() =>
      f.host.handle(f.event(malformed), f.context, malformed)
    );
    assert.equal(f.host.handle(f.event(malformed), f.context, malformed), false);
  }
  const inheritedEnvelope = Object.create({
    type: HOME_CLIPBOARD_REQUEST_TYPE,
    requestId: "inherited:1",
  });
  assert.equal(
    f.host.handle(
      f.event(inheritedEnvelope),
      f.context,
      inheritedEnvelope,
    ),
    false,
  );
  assert.equal(f.results().length, 0);
});

test("wrong source, wrong opaque origin, or unregistered frame has no authority", () => {
  const f = fixture({ registerSource: false });
  const message = requestMessage(f, "unregistered:1");
  f.host.handle(f.event(message), f.context, message);
  assert.equal(f.posts.length, 0);

  f.host.resetFrame(f.state, f.context);
  f.host.handle(f.event(message, { source: {} }), f.context, message);
  f.host.handle(
    f.event(message, { origin: "https://attacker.example" }),
    f.context,
    message,
  );
  assert.equal(f.prompt.requests.length, 0);
  assert.equal(f.results().length, 0);
});

test("concurrency, cancellation, timeout, and replay are deterministic", async () => {
  const f = fixture({
    clipboard: { async writeText() {} },
    timeoutMs: 25,
  });
  const first = requestMessage(f, "serial:1");
  const concurrent = requestMessage(f, "serial:2", { text: "second" });
  f.host.handle(f.event(first), f.context, first);
  f.host.handle(f.event(concurrent), f.context, concurrent);
  assert.equal(f.results()[0].message.error, "busy");
  f.prompt.allow();
  await flushTasks();
  assert.equal(f.results()[1].message.ok, true);
  f.host.handle(f.event(first), f.context, first);
  assert.equal(f.results()[2].message.error, "replay");

  const cancelled = requestMessage(f, "cancel:1");
  f.host.handle(f.event(cancelled), f.context, cancelled);
  f.prompt.allow(false);
  await flushTasks();
  assert.equal(f.results()[3].message.error, "cancelled");

  const timedOut = requestMessage(f, "timeout:1");
  f.host.handle(f.event(timedOut), f.context, timedOut);
  f.timers.fireDelay(25);
  await flushTasks();
  assert.equal(f.results()[4].message.error, "timeout");
  assert.equal(f.state.inFlight, null);
});

test("teardown clears text-free state, replay memory, and lifecycle generation", async () => {
  const f = fixture();
  const message = requestMessage(f, "retire:1", { text: "ephemeral" });
  f.host.handle(f.event(message), f.context, message);
  assert.equal(Object.hasOwn(f.state.inFlight, "text"), false);
  const generation = f.state.generation;
  f.host.retireFrame(f.state);
  await flushTasks();
  assert.equal(f.state.inFlight, null);
  assert.equal(f.state.replayIds.size, 0);
  assert.equal(f.state.retired, true);
  assert.equal(f.state.generation, "");
  assert.equal(f.results().length, 0);

  f.host.resetFrame(f.state, f.context);
  assert.equal(f.state.retired, false);
  assert.notEqual(f.state.generation, generation);
});

test("replay memory remains bounded", async () => {
  const f = fixture({ clipboard: { async writeText() {} } });
  for (let index = 0; index <= MAX_HOME_CLIPBOARD_REPLAY_IDS; index += 1) {
    const message = requestMessage(f, `bounded:${index}`, {
      text: String(index),
    });
    f.host.handle(f.event(message), f.context, message);
    f.prompt.allow();
    await flushTasks();
  }
  assert.equal(f.state.replayIds.size, MAX_HOME_CLIPBOARD_REPLAY_IDS);
  assert.equal(f.state.replayIds.has("bounded:0"), false);
});
