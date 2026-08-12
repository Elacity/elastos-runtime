import assert from "node:assert/strict";
import test from "node:test";

import {
  HOME_CLIPBOARD_CLIENT_TIMEOUT_MS,
  MAX_HOME_CLIPBOARD_TEXT_UTF8_BYTES,
  createHomeClipboardClient,
} from "./home-clipboard-client.js";
import {
  MAX_HOME_CLIPBOARD_PURPOSE_CODE_UNITS,
} from "./home-clipboard-protocol.js";

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

function fixture(targetId = "browser", { autoReady = true } = {}) {
  const posts = [];
  const listeners = new Map();
  const timers = fakeTimers();
  const sourceWindow = {
    addEventListener(type, listener) {
      listeners.set(type, listener);
    },
    removeEventListener(type, listener) {
      if (listeners.get(type) === listener) {
        listeners.delete(type);
      }
    },
  };
  const targetWindow = {
    postMessage(message, targetOrigin) {
      posts.push({ message, targetOrigin });
    },
  };
  let nextId = 0;
  const client = createHomeClipboardClient({
    targetId,
    homeOrigin: "https://home.example",
    homeToken: `${targetId}-token`,
    targetWindow,
    sourceWindow,
    cryptoRef: {
      randomUUID() {
        nextId += 1;
        return `request-${nextId}`;
      },
    },
    setTimeoutFn: timers.setTimeoutFn,
    clearTimeoutFn: timers.clearTimeoutFn,
  });
  const event = (data, overrides = {}) => ({
    source: targetWindow,
    origin: "https://home.example",
    data,
    ...overrides,
  });
  const ready = (overrides = {}) => ({
    type: "home:clipboard-ready",
    schema: "elastos.home.clipboard.ready/v1",
    targetId,
    homeToken: `${targetId}-token`,
    parentOrigin: "https://home.example",
    generation: "clipboard:generation-1",
    ...overrides,
  });
  const result = (requestId, operation, purpose, fields = {}) => ({
    type: "home:clipboard-result",
    schema: "elastos.home.clipboard.result/v1",
    requestId,
    targetId,
    homeToken: `${targetId}-token`,
    parentOrigin: "https://home.example",
    generation: "clipboard:generation-1",
    operation,
    purpose,
    ok: true,
    ...fields,
  });
  client.start();
  if (autoReady) {
    client.handleMessage(event(ready()));
  }
  return {
    client,
    event,
    listeners,
    posts,
    ready,
    result,
    sourceWindow,
    targetWindow,
    timers,
  };
}

test("client registers before requesting and never asserts its target", async () => {
  const f = fixture();
  assert.deepEqual(f.posts[0], {
    targetOrigin: "https://home.example",
    message: { type: "home:app-ready", homeToken: "browser-token" },
  });
  const write = f.client.writeText("Browser status");
  assert.deepEqual(f.posts[1].message, {
    type: "home:clipboard-request",
    schema: "elastos.home.clipboard.request/v1",
    requestId: "request-1",
    homeToken: "browser-token",
    parentOrigin: "https://home.example",
    generation: "clipboard:generation-1",
    operation: "write",
    purpose: "browser.text",
    mime_type: "text/plain",
    text: "Browser status",
  });
  assert.equal(Object.hasOwn(f.posts[1].message, "targetId"), false);
  f.client.handleMessage(
    f.event(f.result("request-1", "write", "browser.text")),
  );
  assert.equal(await write, true);
  assert.equal(f.timers.size(), 0);
});

test("first-party write re-registers when readiness is missing and still fails closed without it", async () => {
  const readyMissing = fixture("wallet", { autoReady: false });
  const write = readyMissing.client.writeText("0x1950", {
    purpose: "wallet.address",
  });
  assert.deepEqual(
    readyMissing.posts.map(({ message }) => message.type),
    ["home:app-ready", "home:app-ready"],
  );
  readyMissing.client.handleMessage(readyMissing.event(readyMissing.ready()));
  await Promise.resolve();
  assert.equal(
    readyMissing.posts.at(-1).message.type,
    "home:clipboard-request",
  );
  readyMissing.client.handleMessage(
    readyMissing.event(
      readyMissing.result("request-1", "write", "wallet.address"),
    ),
  );
  assert.equal(await write, true);
  assert.equal(readyMissing.timers.size(), 0);

  const unavailable = fixture("wallet", { autoReady: false });
  const unavailableWrite = unavailable.client.writeText("0x1950", {
    purpose: "wallet.address",
  });
  unavailable.timers.fireDelay(HOME_CLIPBOARD_CLIENT_TIMEOUT_MS);
  await assert.rejects(unavailableWrite, /unavailable/);
  assert.equal(unavailable.posts.length, 2);
});

test("only Browser can read and exact matching text/plain result settles it", async () => {
  const f = fixture();
  const read = f.client.readText();
  f.client.handleMessage(
    f.event(f.result("request-1", "read", "browser.text", {
      mime_type: "text/plain",
      text: "paste me",
    })),
  );
  assert.equal(await read, "paste me");

  const wallet = fixture("wallet");
  await assert.rejects(
    wallet.client.readText({ purpose: "wallet.address" }),
    /denied/,
  );
  assert.equal(wallet.posts.length, 1);
});

test("client accepts the closed first-party write purposes", async () => {
  const cases = [
    ["wallet", "wallet.address", "0x1234"],
    ["wallet", "wallet.recovery-key", '{"secret":"bounded"}'],
    ["wallet-metamask", "wallet.address", "0x2345"],
    ["wallet-unisat", "wallet.address", "bc1qexample"],
    ["wallet-walletconnect", "wallet.address", "0x3456"],
    ["library", "resource.uri", "object:private/document-1"],
    ["library", "resource.identifier", "bafy-library-content"],
    ["documents", "resource.uri", "elastos://bafy-document"],
  ];
  for (const [targetId, purpose, text] of cases) {
    const f = fixture(targetId);
    const write = f.client.writeText(text, { purpose });
    f.client.handleMessage(
      f.event(f.result("request-1", "write", purpose)),
    );
    assert.equal(await write, true);
  }
});

test("wrong source, origin, token, target, generation, or shape fails closed", async () => {
  const ignored = fixture();
  const ignoredWrite = ignored.client.writeText("copy");
  const success = ignored.result("request-1", "write", "browser.text");
  assert.equal(
    ignored.client.handleMessage(ignored.event(success, { source: {} })),
    false,
  );
  assert.equal(
    ignored.client.handleMessage(
      ignored.event(success, { origin: "https://attacker.example" }),
    ),
    false,
  );
  ignored.client.handleMessage(ignored.event(success));
  assert.equal(await ignoredWrite, true);

  for (const fields of [
    { targetId: "wallet" },
    { homeToken: "substituted" },
    { parentOrigin: "https://attacker.example" },
    { generation: "clipboard:stale" },
    { purpose: "wallet.address" },
    { schema: "wrong/v1" },
    { extra: true },
  ]) {
    const f = fixture();
    const write = f.client.writeText("copy");
    f.client.handleMessage(
      f.event({ ...f.result("request-1", "write", "browser.text"), ...fields }),
    );
    await assert.rejects(write, /invalid Clipboard result/);
  }
});

test("concurrency, payload bounds, timeout, cancellation, and teardown are closed", async () => {
  const f = fixture();
  const first = f.client.writeText("first", { requestId: "copy:1" });
  await assert.rejects(f.client.readText(), /already active/);
  f.client.cancel("copy:1");
  await assert.rejects(first, /cancelled/);
  assert.equal(f.posts.at(-1).message.type, "home:clipboard-cancel");

  const oversized = "a".repeat(MAX_HOME_CLIPBOARD_TEXT_UTF8_BYTES + 1);
  await assert.rejects(
    f.client.writeText(oversized),
    /denied/,
  );
  await assert.rejects(
    f.client.writeText("not a uri", { purpose: "resource.uri" }),
    /denied/,
  );
  await assert.rejects(
    f.client.writeText("copy", {
      purpose: "p".repeat(MAX_HOME_CLIPBOARD_PURPOSE_CODE_UNITS + 1),
    }),
    /denied/,
  );

  const read = f.client.readText();
  f.timers.fireDelay(HOME_CLIPBOARD_CLIENT_TIMEOUT_MS);
  await assert.rejects(read, /timed out/);

  const pending = f.client.writeText("ephemeral");
  f.client.teardown();
  await assert.rejects(pending, /retired/);
  assert.equal(f.posts.at(-1).message.type, "home:clipboard-retire");
  assert.equal(f.client.canRequest(), false);
  assert.equal(f.listeners.has("message"), false);
});

test("inherited targets and purposes never become Clipboard authority", async () => {
  for (const inheritedName of ["__proto__", "constructor", "toString"]) {
    const target = fixture(inheritedName);
    assert.equal(target.client.canRequest(), false);
    assert.equal(target.posts.length, 0);
    await assert.rejects(
      target.client.writeText("copy", { purpose: "browser.text" }),
      /unavailable/,
    );

    const purpose = fixture();
    await assert.rejects(
      purpose.client.writeText("copy", { purpose: inheritedName }),
      /denied/,
    );
    assert.equal(purpose.posts.length, 1);
  }
});

test("substituted lifecycle readiness cannot replace an active generation", async () => {
  const f = fixture();
  const write = f.client.writeText("ephemeral");
  f.client.handleMessage(
    f.event(f.ready({ generation: "clipboard:generation-2" })),
  );
  await assert.rejects(write, /lifecycle changed/);
  assert.equal(f.client.canRequest(), true);
});
