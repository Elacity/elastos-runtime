// The ceremony half of the reader: what it does between asking to open a
// protected file and being given one.
//
// Opening is not a request that succeeds or fails once. Every first open of
// every protected item reaches a wallet that holds a rights-signature request,
// and the file arrives only after the person answers it. These tests drive that
// wait on a clock of their own, so the behaviour is checked without waiting
// through it.

import assert from "node:assert/strict";
import test from "node:test";

import {
  OPEN_PROGRESS_SCHEMA,
  createReaderController,
  readOpenProgress,
  waitingMessage,
} from "./reader.js";

const MINT = "a".repeat(64);
const HANDLE = "b".repeat(64);
const TOKEN = "home-token-1";

function progressPayload(overrides = {}) {
  return {
    status: "error",
    code: "library_error",
    message: "Runtime custody viewer release is pending exact Wallet approval",
    open_progress: {
      schema: OPEN_PROGRESS_SCHEMA,
      stage: "rights_approval",
      resumable: true,
      awaits_person: true,
      connector_id: "metamask",
      approval_request_id: "approval-7",
      expires_at: 1_000_290,
      ...overrides,
    },
  };
}

// ---------------------------------------------------------------------------
// Reading the typed state
// ---------------------------------------------------------------------------

test("the typed open state is read from the answer, never from its sentence", () => {
  const progress = readOpenProgress(progressPayload());
  assert.deepEqual(progress, {
    stage: "rights_approval",
    resumable: true,
    awaitsPerson: true,
    connectorId: "metamask",
    expiresAt: 1_000_290,
  });

  // Refused: a state this reader has no way to wait through is not waited
  // through, whatever the answer claims about it.
  assert.equal(readOpenProgress(progressPayload({ schema: "something.else/v1" })), null);
  assert.equal(readOpenProgress(progressPayload({ stage: "" })), null);
  assert.equal(readOpenProgress({ status: "error", message: "plain" }), null);
  assert.equal(readOpenProgress(null), null);
  assert.equal(
    readOpenProgress(progressPayload({ stage: "custody_recovery", resumable: true })).resumable,
    false,
    "an unknown stage carries no resume policy, so it is not resumable here",
  );

  // Refused as final, which is the one a reader must stop asking about.
  const denied = readOpenProgress(
    progressPayload({ stage: "denied", resumable: false, awaits_person: false }),
  );
  assert.equal(denied.resumable, false);
});

test("the waiting sentence names where to go, and is this reader's own wording", () => {
  const external = readOpenProgress(progressPayload());
  assert.match(waitingMessage(external), /metamask/);
  // Runtime's own sentence names internal machinery; none of it reaches the
  // page.
  assert.doesNotMatch(waitingMessage(external), /Runtime|custody|release/i);

  const noConnector = readOpenProgress(progressPayload({ connector_id: null }));
  assert.match(waitingMessage(noConnector), /your wallet/);

  // A managed account is waiting too, and asks the person for nothing.
  const managed = readOpenProgress(progressPayload({ awaits_person: false, connector_id: null }));
  assert.doesNotMatch(waitingMessage(managed), /Approve/);
});

// ---------------------------------------------------------------------------
// A document stub, only as wide as the controller actually reaches
// ---------------------------------------------------------------------------

function element(id) {
  return {
    id,
    textContent: "",
    dataset: {},
    hidden: false,
    disabled: false,
    dir: "",
    className: "",
    type: "",
    childNodes: [],
    listeners: {},
    focused: 0,
    get firstChild() {
      return this.childNodes[0] || null;
    },
    appendChild(child) {
      this.childNodes.push(child);
      return child;
    },
    removeChild(child) {
      this.childNodes = this.childNodes.filter((node) => node !== child);
      return child;
    },
    addEventListener(name, handler) {
      (this.listeners[name] ||= []).push(handler);
    },
    focus() {
      this.focused += 1;
    },
  };
}

function harness({ replies, now = 1_000_000 }) {
  const elements = new Map(
    [
      "reader-stage",
      "reader-status",
      "reader-kind",
      "reader-controls",
      "reader-previous",
      "reader-next",
      "reader-position",
    ].map((id) => [id, element(id)]),
  );
  const documentObject = {
    getElementById: (id) => elements.get(id),
    createElement: (tag) => element(`created-${tag}`),
  };
  const windowListeners = {};
  const windowObject = {
    addEventListener(name, handler) {
      (windowListeners[name] ||= []).push(handler);
    },
  };
  const timers = [];
  const requests = [];
  let clock = now;

  const controller = createReaderController({
    documentObject,
    windowObject,
    locationObject: { search: `?mint_id=${MINT}`, hash: `#home_token=${TOKEN}` },
    async fetchImpl(url, options) {
      requests.push({ url, body: JSON.parse(options.body) });
      const reply = replies.shift();
      if (!reply) throw new Error("no reply queued");
      return {
        ok: reply.ok !== false,
        text: async () => JSON.stringify(reply.payload),
      };
    },
    urlObject: { createObjectURL: () => "blob:x", revokeObjectURL() {} },
    render: () => async () => ({ release() {} }),
    nowSeconds: () => clock,
    setTimeoutImpl: (handler) => {
      timers.push(handler);
      return timers.length;
    },
    clearTimeoutImpl: () => {},
  });

  return {
    controller,
    requests,
    elements,
    windowListeners,
    stageText: () =>
      (elements.get("reader-stage").childNodes[0]?.childNodes || [])
        .map((node) => node.textContent)
        .join(" "),
    statusText: () => elements.get("reader-status").textContent,
    advance: (seconds) => {
      clock += seconds;
    },
    /** Runs every timer the controller set, the way a clock eventually would. */
    async tick() {
      const pending = timers.splice(0, timers.length);
      for (const handler of pending) handler();
      await new Promise((resolve) => setImmediate(resolve));
      await new Promise((resolve) => setImmediate(resolve));
    },
    pendingTimers: () => timers.length,
  };
}

function openReply() {
  return {
    payload: {
      status: "ok",
      data: {
        schema: "elastos.library.runtime-custody-viewer/v1",
        content_kind: "object",
        mint_id: MINT,
        viewer_session_handle: HANDLE,
        expires_at: 1_756_293_600,
        content_type: "text/plain",
        plaintext_bytes: 5,
        chunk_count: 1,
        chunk_plaintext_bytes: 1024 * 1024,
      },
    },
  };
}

function partReply() {
  return {
    payload: {
      status: "ok",
      data: {
        schema: "elastos.library.runtime-custody-viewer-part/v1",
        content_kind: "object",
        mint_id: MINT,
        viewer_session_handle: HANDLE,
        chunk_index: 0,
        encoding: "base64",
        data: Buffer.from("hello").toString("base64"),
      },
    },
  };
}

// ---------------------------------------------------------------------------
// Waiting through the ceremony
// ---------------------------------------------------------------------------

test("an open waiting on the wallet keeps waiting, and opens once the person answers", async () => {
  const world = harness({
    replies: [
      { ok: false, payload: progressPayload() },
      openReply(),
      partReply(),
    ],
  });

  await world.controller.open();

  // The person is told whose turn it is, and the file is not called
  // unavailable.
  assert.match(world.statusText(), /metamask/);
  assert.doesNotMatch(world.statusText(), /unavailable/i);
  assert.equal(world.controller.getState().failed, false);
  assert.equal(world.controller.getState().waiting, true);
  assert.equal(world.controller.getState().waitingStage, "rights_approval");
  assert.equal(world.pendingTimers(), 1);

  await world.tick();

  // The identical open is re-issued. Runtime resumes the attempt it already
  // has, so asking again is how the approval is collected.
  assert.equal(world.requests.length, 3);
  assert.deepEqual(world.requests[0].body, { mint_id: MINT });
  assert.deepEqual(world.requests[1].body, world.requests[0].body);
  assert.equal(world.requests[1].url, world.requests[0].url);

  const state = world.controller.getState();
  assert.equal(state.failed, false);
  assert.equal(state.waiting, false, "the waiting stops once the file is open");
  assert.equal(world.controller.getSession().contentType, "text/plain");
});

test("a refusal Runtime calls final stops at once and offers nothing to press", async () => {
  const world = harness({
    replies: [
      {
        ok: false,
        payload: {
          status: "error",
          code: "library_error",
          message: "Runtime custody open is denied before purchase",
          open_progress: {
            schema: OPEN_PROGRESS_SCHEMA,
            stage: "denied",
            resumable: false,
            awaits_person: false,
          },
        },
      },
    ],
  });

  await world.controller.open();

  const state = world.controller.getState();
  assert.equal(state.failed, true);
  assert.equal(state.waiting, false);
  assert.equal(state.retryOffered, false, "a decided answer gets no retry button");
  assert.equal(world.pendingTimers(), 0);
  assert.equal(world.requests.length, 1, "a decided answer is not asked again");
});

test("the waiting stops when the wallet request lapses, and then the person may ask again", async () => {
  const world = harness({
    replies: [
      { ok: false, payload: progressPayload() },
      { ok: false, payload: progressPayload() },
    ],
  });

  await world.controller.open();
  assert.equal(world.controller.getState().waiting, true);

  // The window Runtime sent closes while the person is away.
  world.advance(400);
  await world.tick();

  const state = world.controller.getState();
  assert.equal(state.failed, true);
  assert.equal(state.waiting, false);
  assert.equal(state.retryOffered, true, "asking again could still work, so it is offered");
  assert.equal(world.pendingTimers(), 0, "nothing is left ticking");
});

test("the page going away stops the waiting and leaves nothing ticking", async () => {
  const world = harness({ replies: [{ ok: false, payload: progressPayload() }] });

  await world.controller.open();
  assert.equal(world.controller.getState().waiting, true);

  world.controller.dispose();

  assert.equal(world.controller.getState().disposed, true);
  await world.tick();
  assert.equal(world.requests.length, 1, "a disposed page asks for nothing more");
});

test("a launch with nothing chosen explains where files come from, once", async () => {
  const world = harness({ replies: [] });
  const empty = createReaderController({
    documentObject: {
      getElementById: (id) => world.elements.get(id),
      createElement: (tag) => element(`created-${tag}`),
    },
    windowObject: { addEventListener() {} },
    locationObject: { search: "", hash: `#home_token=${TOKEN}` },
    fetchImpl: async () => {
      throw new Error("nothing should be asked for");
    },
    render: () => async () => ({ release() {} }),
  });

  await empty.open();

  assert.match(world.statusText(), /Library/);
  assert.doesNotMatch(world.statusText(), /unavailable/i);
  assert.equal(empty.getState().failed, false);
});
