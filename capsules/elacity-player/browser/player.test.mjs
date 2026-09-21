// What the player does between asking to open protected media and being given
// it, plus the parsing it refuses along the way.
//
// Playing is not a request that succeeds or fails once. Every first open of
// every protected item reaches a wallet that holds a rights-signature request,
// and the media arrives only after the person answers it. These tests drive
// that wait on a clock of their own, so the behaviour is checked without
// waiting through it.

import assert from "node:assert/strict";
import test from "node:test";

import {
  OPEN_PROGRESS_SCHEMA,
  PRESENTATION_CONTROLS_ONLY,
  PRESENTATION_IMAGE_TRACK,
  buildViewerMimeType,
  createPlayerController,
  parseViewerOpenData,
  parseViewerPartData,
  presentationFor,
  readLaunchContext,
  readOpenProgress,
  renditionHasImageTrack,
  waitingMessage,
} from "./player.js";

const MINT = "a".repeat(64);
const HANDLE = "b".repeat(64);
const TOKEN = "home-token-1";

function openData(overrides = {}) {
  return {
    schema: "elastos.library.runtime-custody-viewer/v1",
    content_kind: "media",
    mint_id: MINT,
    viewer_session_handle: HANDLE,
    expires_at: 1_756_293_600,
    mime_type: "video/mp4",
    codecs: "avc1.640028,mp4a.40.2",
    has_init_segment: true,
    segment_count: 2,
    ...overrides,
  };
}

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
// The session answer
// ---------------------------------------------------------------------------

test("the launch context comes from the query and the hash, never the other way round", () => {
  assert.deepEqual(readLaunchContext({ search: `?mint_id=${MINT}`, hash: `#home_token=${TOKEN}` }), {
    mintId: MINT,
    homeToken: TOKEN,
  });
  // A token in the query is ignored: it must never travel anywhere a server
  // writes down.
  assert.deepEqual(readLaunchContext({ search: `?home_token=${TOKEN}`, hash: `#mint_id=${MINT}` }), {
    mintId: "",
    homeToken: "",
  });
});

test("a media session is read whole, and anything else is refused", () => {
  const session = parseViewerOpenData(openData(), MINT);
  assert.equal(session.segmentCount, 2);
  assert.equal(buildViewerMimeType(session.mimeType, session.codecs), 'video/mp4; codecs="avc1.640028,mp4a.40.2"');

  // A file session's geometry is a run of equal parts, not a segment ladder,
  // so it is refused here rather than misread field by field.
  assert.throws(() => parseViewerOpenData(openData({ content_kind: "object" }), MINT));
  // Another item's session, and an answer carrying a field this player does
  // not know, are both refused.
  assert.throws(() => parseViewerOpenData(openData({ mint_id: "c".repeat(64) }), MINT));
  assert.throws(() => parseViewerOpenData({ ...openData(), extra: 1 }, MINT));
  assert.throws(() => parseViewerOpenData(openData({ has_init_segment: false }), MINT));
  assert.throws(() => parseViewerOpenData(openData({ segment_count: 0 }), MINT));
});

test("a part is refused unless it belongs to this exact session", () => {
  const part = {
    schema: "elastos.library.runtime-custody-viewer-part/v1",
    mint_id: MINT,
    viewer_session_handle: HANDLE,
    encoding: "base64",
    data: Buffer.from("segment").toString("base64"),
  };
  assert.equal(parseViewerPartData(part, MINT, HANDLE).length, 7);
  assert.throws(() => parseViewerPartData(part, MINT, "d".repeat(64)));
  assert.throws(() => parseViewerPartData({ ...part, encoding: "hex" }, MINT, HANDLE));
  assert.throws(() => parseViewerPartData({ ...part, data: "" }, MINT, HANDLE));
});

test("presentation follows the rendition, and stands in for no picture it does not have", () => {
  assert.ok(renditionHasImageTrack("avc1.640028,mp4a.40.2"));
  assert.ok(!renditionHasImageTrack("mp4a.40.2"));
  assert.equal(presentationFor({ codecs: "avc1.640028" }, ""), PRESENTATION_IMAGE_TRACK);
  // Sound alone, with no poster: the frame collapses to its controls rather
  // than showing something invented.
  assert.equal(presentationFor({ codecs: "mp4a.40.2" }, ""), PRESENTATION_CONTROLS_ONLY);
});

// ---------------------------------------------------------------------------
// Reading the typed state
// ---------------------------------------------------------------------------

test("the typed open state is read from the answer, never from its sentence", () => {
  assert.deepEqual(readOpenProgress(progressPayload()), {
    stage: "rights_approval",
    resumable: true,
    awaitsPerson: true,
    connectorId: "metamask",
    expiresAt: 1_000_290,
  });
  assert.equal(readOpenProgress(progressPayload({ schema: "something.else/v1" })), null);
  assert.equal(readOpenProgress({ status: "error", message: "plain" }), null);
  assert.equal(
    readOpenProgress(progressPayload({ stage: "custody_recovery" })).resumable,
    false,
    "an unknown stage carries no resume policy, so it is not resumable here",
  );
});

test("the waiting sentence names where to go, and is this player's own wording", () => {
  const external = readOpenProgress(progressPayload());
  assert.match(waitingMessage(external), /metamask/);
  assert.doesNotMatch(waitingMessage(external), /Runtime|custody|release/i);
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
    type: "",
    className: "",
    childNodes: [],
    listeners: {},
    focused: 0,
    classList: { toggle() {} },
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
    removeAttribute() {},
    getAttribute() {
      return "";
    },
    focus() {
      this.focused += 1;
    },
    load() {},
    pause() {},
    play: async () => {},
  };
}

class FakeMediaSource {
  constructor() {
    this.readyState = "open";
    this.buffers = [];
    this.ended = 0;
  }

  addEventListener(name, handler) {
    // The engine opens the source on a later turn; one microtask is enough to
    // reproduce that ordering here.
    if (name === "sourceopen") queueMicrotask(handler);
  }

  addSourceBuffer() {
    const buffer = {
      appendBuffer: (bytes) => {
        this.buffers.push(bytes);
        for (const handler of buffer.updateEnd) handler();
      },
      updateEnd: [],
      addEventListener(name, handler) {
        if (name === "updateend") buffer.updateEnd.push(handler);
      },
      removeEventListener() {},
    };
    return buffer;
  }

  endOfStream() {
    this.ended += 1;
    this.readyState = "ended";
  }

  static isTypeSupported() {
    return true;
  }
}

function harness({ replies, now = 1_000_000, search = `?mint_id=${MINT}` }) {
  const elements = new Map(
    ["player-video", "player-status", "player-overlay", "player-overlay-text"].map((id) => [
      id,
      element(id),
    ]),
  );
  const timers = [];
  const requests = [];
  const revoked = [];
  let clock = now;
  let sources = [];

  const controller = createPlayerController({
    documentObject: {
      getElementById: (id) => elements.get(id),
      createElement: (tag) => element(`created-${tag}`),
    },
    windowObject: { addEventListener() {} },
    locationObject: { search, hash: `#home_token=${TOKEN}` },
    async fetchImpl(url, options) {
      requests.push({ url, body: JSON.parse(options.body) });
      const reply = replies.shift();
      if (!reply) throw new Error("no reply queued");
      return { ok: reply.ok !== false, text: async () => JSON.stringify(reply.payload) };
    },
    mediaSourceClass: class extends FakeMediaSource {
      constructor() {
        super();
        sources.push(this);
      }
    },
    urlObject: {
      createObjectURL: () => "blob:media",
      revokeObjectURL: (value) => revoked.push(value),
    },
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
    revoked,
    sources: () => sources,
    overlayText: () => elements.get("player-overlay-text").textContent,
    overlayChildren: () => elements.get("player-overlay").childNodes,
    advance: (seconds) => {
      clock += seconds;
    },
    async tick() {
      const pending = timers.splice(0, timers.length);
      for (const handler of pending) handler();
      for (let turn = 0; turn < 4; turn += 1) {
        await new Promise((resolve) => setImmediate(resolve));
      }
    },
    pendingTimers: () => timers.length,
  };
}

function partReply(text) {
  return {
    payload: {
      status: "ok",
      data: {
        schema: "elastos.library.runtime-custody-viewer-part/v1",
        mint_id: MINT,
        viewer_session_handle: HANDLE,
        encoding: "base64",
        data: Buffer.from(text).toString("base64"),
      },
    },
  };
}

// ---------------------------------------------------------------------------
// Waiting through the ceremony
// ---------------------------------------------------------------------------

test("an open waiting on the wallet keeps waiting, and plays once the person answers", async () => {
  const world = harness({
    replies: [
      { ok: false, payload: progressPayload() },
      { payload: { status: "ok", data: openData() } },
      partReply("init"),
      partReply("seg0"),
      partReply("seg1"),
    ],
  });

  await world.controller.startPlayback();

  assert.match(world.overlayText(), /metamask/);
  assert.doesNotMatch(world.overlayText(), /unavailable/i);
  assert.equal(world.controller.getState().failed, false);
  assert.equal(world.controller.getState().waiting, true);
  assert.equal(world.pendingTimers(), 1);

  await world.tick();

  // The identical open is re-issued, so Runtime resumes the attempt it has.
  assert.deepEqual(world.requests[0].body, { mint_id: MINT });
  assert.deepEqual(world.requests[1].body, world.requests[0].body);
  assert.equal(world.controller.getState().waiting, false);
  assert.equal(world.controller.getSession().segmentCount, 2);
  // Init segment, segment 0, segment 1.
  assert.equal(world.sources()[0].buffers.length, 3);
  assert.equal(world.sources()[0].ended, 1);
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

  await world.controller.startPlayback();

  const state = world.controller.getState();
  assert.equal(state.failed, true);
  assert.equal(state.waiting, false);
  assert.equal(state.retryOffered, false);
  assert.equal(world.requests.length, 1, "a decided answer is not asked again");
});

test("the waiting stops when the wallet request lapses, and then the person may ask again", async () => {
  const world = harness({
    replies: [
      { ok: false, payload: progressPayload() },
      { ok: false, payload: progressPayload() },
    ],
  });

  await world.controller.startPlayback();
  assert.equal(world.controller.getState().waiting, true);

  world.advance(400);
  await world.tick();

  const state = world.controller.getState();
  assert.equal(state.failed, true);
  assert.equal(state.retryOffered, true);
  assert.equal(world.overlayChildren().length, 1, "one control, not a second sentence");
  assert.equal(world.pendingTimers(), 0);
});

test("the page going away gives back the media source and the object URL", async () => {
  const world = harness({
    replies: [
      { payload: { status: "ok", data: openData({ segment_count: 1 }) } },
      partReply("init"),
      partReply("seg0"),
      { payload: { status: "ok", data: { closed: true } } },
    ],
  });

  await world.controller.startPlayback();
  assert.equal(world.sources()[0].buffers.length, 2);

  world.controller.dispose();

  assert.equal(world.controller.getState().disposed, true);
  assert.deepEqual(world.revoked, ["blob:media"], "the object URL is given back exactly once");
  // The video element firing an error as its source is torn down repaints
  // nothing: a page that has gone away has nothing to show.
  assert.equal(world.controller.getState().failed, false);
});

test("a launch with nothing chosen explains where media comes from, once", async () => {
  const world = harness({ replies: [], search: "" });

  await world.controller.startPlayback();

  assert.match(world.overlayText(), /Library/);
  assert.doesNotMatch(world.overlayText(), /unavailable/i);
  assert.equal(world.requests.length, 0, "nothing is asked for");
  assert.equal(world.controller.getState().failed, false);
});
