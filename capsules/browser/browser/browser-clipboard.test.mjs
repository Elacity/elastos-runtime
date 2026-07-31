import assert from "node:assert/strict";
import test from "node:test";

import {
  CLIPBOARD_COPY_INTENT_TIMEOUT_MS,
  MAX_CLIPBOARD_CHUNK_COUNT,
  MAX_CLIPBOARD_ENCODED_BYTES,
  MAX_CLIPBOARD_ENCODED_CHUNK_BYTES,
  MAX_CLIPBOARD_TEXT_UTF8_BYTES,
  assertBoundedClipboardText,
  clipboardTextUtf8Bytes,
  createBrowserClipboardBridge,
  decodeClipboardBase64Text,
} from "./browser-clipboard.js";

function fakeTimers() {
  let nextId = 1;
  const timers = new Map();
  return {
    clearTimeoutFn(id) {
      timers.delete(id);
    },
    fireDelay(delay) {
      const timer = [...timers.entries()].find(([, entry]) => entry.delay === delay);
      assert.ok(timer, `expected a ${delay}ms timer`);
      timers.delete(timer[0]);
      timer[1].callback();
    },
    pendingDelays() {
      return [...timers.values()].map(({ delay }) => delay);
    },
    setTimeoutFn(callback, delay) {
      const id = nextId++;
      timers.set(id, { callback, delay });
      return id;
    },
  };
}

function bridgeFixture(overrides = {}) {
  const hostWrites = [];
  const sends = [];
  const statuses = [];
  const cancellations = [];
  const timers = fakeTimers();
  let currentPage = { page_id: "page-1" };
  let requestSerial = 0;
  const bridge = createBrowserClipboardBridge({
    cancelHostClipboardRequestFn: (requestId) => {
      cancellations.push(requestId);
      return true;
    },
    createClipboardRequestIdFn: () => {
      requestSerial += 1;
      return `copy:${requestSerial}`;
    },
    friendlyOpenError: (error) => error?.message || String(error),
    getCurrentPage: () => currentPage,
    sendBrowserInput: async (...args) => {
      sends.push(args);
    },
    showStatus: (...args) => {
      statuses.push(args);
    },
    writeHostClipboardTextFn: async (...args) => {
      hostWrites.push(args);
    },
    setTimeoutFn: timers.setTimeoutFn,
    clearTimeoutFn: timers.clearTimeoutFn,
    ...overrides,
  });
  return {
    bridge,
    cancellations,
    hostWrites,
    sends,
    setCurrentPage(page) {
      currentPage = page;
    },
    statuses,
    timers,
  };
}

function base64Text(text) {
  return Buffer.from(text, "utf8").toString("base64");
}

async function deliver(bridge, message) {
  return bridge.handleRemoteInputChannelMessage({
    data: JSON.stringify(message),
  });
}

function startMessage(mimeType = "text/plain") {
  return {
    type: "clipboard-msg-start",
    data: { mime_type: mimeType },
  };
}

function dataMessage(content) {
  return {
    type: "clipboard-msg-data",
    data: { content },
  };
}

function endMessage(data = undefined) {
  return {
    type: "clipboard-msg-end",
    ...(data === undefined ? {} : { data }),
  };
}

function completeMessage(content, mimeType = "text/plain") {
  return {
    type: "clipboard-msg",
    data: {
      mime_type: mimeType,
      content,
    },
  };
}

async function beginCopy(fixture) {
  await fixture.bridge.copyRemoteClipboardToHost();
  fixture.timers.fireDelay(150);
  await Promise.resolve();
}

test("accepts exactly 65,536 UTF-8 bytes and rejects one byte more", () => {
  const exact = "😀".repeat(MAX_CLIPBOARD_TEXT_UTF8_BYTES / 4);
  const oversized = `${exact}a`;

  assert.equal(clipboardTextUtf8Bytes(exact), MAX_CLIPBOARD_TEXT_UTF8_BYTES);
  assert.equal(
    clipboardTextUtf8Bytes(oversized),
    MAX_CLIPBOARD_TEXT_UTF8_BYTES + 1,
  );
  assert.equal(assertBoundedClipboardText(exact), exact);
  assert.throws(
    () => assertBoundedClipboardText(oversized),
    /65,536 UTF-8 bytes/,
  );
});

test("canonical base64 decoding is strict and bounded", () => {
  assert.equal(decodeClipboardBase64Text(base64Text("plain text")), "plain text");
  assert.throws(() => decodeClipboardBase64Text("%%%="), /canonical bounded base64/);
  assert.throws(() => decodeClipboardBase64Text("YR=="), /canonical bounded base64/);
  assert.throws(
    () => decodeClipboardBase64Text(Buffer.from([0xc3, 0x28]).toString("base64")),
    /encoded data was not valid/,
  );
});

test("unsolicited remote Clipboard content cannot write the host Clipboard", async () => {
  const f = bridgeFixture();
  await deliver(f.bridge, completeMessage(base64Text("unsolicited")));
  await deliver(f.bridge, startMessage());
  await deliver(f.bridge, dataMessage(base64Text("unsolicited chunks")));
  await deliver(f.bridge, endMessage());

  assert.deepEqual(f.hostWrites, []);
  assert.deepEqual(f.statuses, []);
});

test("explicit local copy binds remote content to one exact Home request", async () => {
  const f = bridgeFixture();
  await f.bridge.copyRemoteClipboardToHost();
  await deliver(f.bridge, completeMessage(base64Text("stale before read")));
  assert.deepEqual(f.hostWrites, []);
  f.timers.fireDelay(150);
  await Promise.resolve();
  assert.deepEqual(f.sends, [
    [
      { type: "key_combo", keysyms: [65507, 99] },
      { history: "replace" },
    ],
    [
      { type: "clipboard_read" },
      { focus: false, history: "replace" },
    ],
  ]);

  await deliver(f.bridge, completeMessage(base64Text("remote clipboard")));
  assert.deepEqual(f.hostWrites, [
    ["remote clipboard", { requestId: "copy:1" }],
  ]);
  assert.deepEqual(f.statuses, [
    ["Copied from Browser.", { sticky: false }],
  ]);

  await deliver(f.bridge, completeMessage(base64Text("second unsolicited")));
  assert.equal(f.hostWrites.length, 1);
});

test("copy success is reported only after the matching Home request succeeds", async () => {
  let resolveWrite;
  const write = new Promise((resolve) => {
    resolveWrite = resolve;
  });
  const f = bridgeFixture({
    writeHostClipboardTextFn: () => write,
  });
  await beginCopy(f);
  const delivery = deliver(f.bridge, completeMessage(base64Text("pending")));
  await Promise.resolve();
  assert.deepEqual(f.statuses, []);
  resolveWrite(true);
  await delivery;
  assert.deepEqual(f.statuses, [
    ["Copied from Browser.", { sticky: false }],
  ]);
});

test("concurrent local copy intent is rejected without duplicate guest effects", async () => {
  const f = bridgeFixture();
  await f.bridge.copyRemoteClipboardToHost();
  await assert.rejects(
    f.bridge.copyRemoteClipboardToHost(),
    /already pending/,
  );
  assert.equal(f.sends.length, 1);
});

test("replacement page ownership rejects stale remote Clipboard content", async () => {
  const f = bridgeFixture();
  await beginCopy(f);
  f.setCurrentPage({ page_id: "page-2" });
  await deliver(f.bridge, completeMessage(base64Text("stale")));
  assert.deepEqual(f.hostWrites, []);
});

test("copy intent timeout clears state and allows a later request", async () => {
  const f = bridgeFixture();
  await f.bridge.copyRemoteClipboardToHost();
  f.timers.fireDelay(CLIPBOARD_COPY_INTENT_TIMEOUT_MS);
  assert.deepEqual(f.statuses, [
    ["Browser copy timed out.", { sticky: true }],
  ]);
  await f.bridge.copyRemoteClipboardToHost();
  assert.equal(f.sends.length, 2);
});

test("malformed, oversized, non-text, and invalid UTF-8 messages cannot write", async () => {
  const messages = [
    completeMessage("%%%="),
    completeMessage("A".repeat(MAX_CLIPBOARD_ENCODED_BYTES + 1)),
    completeMessage(base64Text("<b>html</b>"), "text/html"),
    completeMessage(Buffer.from([0xc3, 0x28]).toString("base64")),
  ];
  for (const message of messages) {
    const f = bridgeFixture();
    await beginCopy(f);
    await deliver(f.bridge, message);
    assert.deepEqual(f.hostWrites, []);
    f.bridge.teardownRemoteClipboard();
  }
});

test("bounded chunk assembly writes once for an explicit local copy", async () => {
  const encoded = base64Text("chunked clipboard ✅");
  const f = bridgeFixture();
  await beginCopy(f);
  await deliver(f.bridge, startMessage());
  await deliver(f.bridge, dataMessage(encoded.slice(0, 5)));
  await deliver(f.bridge, dataMessage(encoded.slice(5)));
  await deliver(f.bridge, endMessage());

  assert.deepEqual(f.hostWrites, [
    ["chunked clipboard ✅", { requestId: "copy:1" }],
  ]);
  await deliver(f.bridge, endMessage());
  assert.equal(f.hostWrites.length, 1);
});

test("chunk count, per-chunk bytes, total bytes, and MIME remain bounded", async () => {
  const f = bridgeFixture();
  await beginCopy(f);
  await deliver(f.bridge, startMessage("text/html"));
  await deliver(f.bridge, dataMessage(base64Text("html")));
  await deliver(f.bridge, endMessage());

  await deliver(f.bridge, startMessage());
  await deliver(
    f.bridge,
    dataMessage("A".repeat(MAX_CLIPBOARD_ENCODED_CHUNK_BYTES + 1)),
  );
  await deliver(f.bridge, endMessage());

  await deliver(f.bridge, startMessage());
  const first = "A".repeat(MAX_CLIPBOARD_ENCODED_CHUNK_BYTES);
  const overflow = "A".repeat(
    MAX_CLIPBOARD_ENCODED_BYTES -
      MAX_CLIPBOARD_ENCODED_CHUNK_BYTES +
      1,
  );
  await deliver(f.bridge, dataMessage(first));
  await deliver(f.bridge, dataMessage(overflow));
  await deliver(f.bridge, endMessage());

  await deliver(f.bridge, startMessage());
  for (let index = 0; index <= MAX_CLIPBOARD_CHUNK_COUNT; index += 1) {
    await deliver(f.bridge, dataMessage("A"));
  }
  await deliver(f.bridge, endMessage());
  assert.deepEqual(f.hostWrites, []);
});

test("teardown clears assembly and cancels an active Home write", async () => {
  let resolveWrite;
  const write = new Promise((resolve) => {
    resolveWrite = resolve;
  });
  const f = bridgeFixture({
    writeHostClipboardTextFn: () => write,
  });
  await beginCopy(f);
  const delivery = deliver(f.bridge, completeMessage(base64Text("ephemeral")));
  await Promise.resolve();
  f.bridge.teardownRemoteClipboard();
  assert.deepEqual(f.cancellations, ["copy:1"]);
  resolveWrite(true);
  await delivery;
  assert.deepEqual(f.statuses, []);
});

test("host paste remains Runtime-mediated and bounded", async () => {
  const exact = "é".repeat(MAX_CLIPBOARD_TEXT_UTF8_BYTES / 2);
  const oversized = `${exact}a`;
  const f = bridgeFixture();

  await f.bridge.pasteHostClipboardIntoRemote(exact);
  assert.deepEqual(f.sends, [
    [
      { type: "paste_text", text: exact },
      { history: "replace" },
    ],
  ]);
  await assert.rejects(
    f.bridge.pasteHostClipboardIntoRemote(oversized),
    /65,536 UTF-8 bytes/,
  );
  assert.equal(f.sends.length, 1);
});
