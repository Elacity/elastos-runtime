import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const playerModule = await import(
  new URL("../capsules/elacity-player/browser/player.js", import.meta.url)
);

const {
  MAX_VIEWER_MEDIA_PART_BYTES,
  MAX_VIEWER_SEGMENT_COUNT,
  createPlayerController,
  parseViewerOpenData,
  parseViewerPartData,
} = playerModule;

function createDeferred() {
  let resolve;
  let reject;
  const promise = new Promise((nextResolve, nextReject) => {
    resolve = nextResolve;
    reject = nextReject;
  });
  return { promise, resolve, reject };
}

async function flushMicrotasks(rounds = 8) {
  for (let index = 0; index < rounds; index += 1) {
    await Promise.resolve();
  }
}

async function waitForMicrotaskCondition(check, rounds = 32) {
  for (let index = 0; index < rounds; index += 1) {
    if (check()) {
      return;
    }
    await Promise.resolve();
  }
  assert.ok(check(), "expected condition to become true");
}

function jsonResponse(payload, options = {}) {
  return {
    ok: options.ok ?? true,
    async text() {
      return JSON.stringify(payload);
    },
  };
}

function base64(bytes) {
  return Buffer.from(bytes).toString("base64");
}

class FakeSourceBuffer extends EventTarget {
  constructor(recordedBuffers, failOnAppend = false) {
    super();
    this.recordedBuffers = recordedBuffers;
    this.failOnAppend = failOnAppend;
  }

  appendBuffer(bytes) {
    if (this.failOnAppend) {
      queueMicrotask(() => this.dispatchEvent(new Event("error")));
      return;
    }
    this.recordedBuffers.push(Array.from(bytes));
    queueMicrotask(() => this.dispatchEvent(new Event("updateend")));
  }
}

class FakeMediaSource extends EventTarget {
  static supported = new Set();

  static isTypeSupported(mimeType) {
    return this.supported.has(mimeType);
  }

  constructor(recordedBuffers, failOnAppend = false) {
    super();
    this.recordedBuffers = recordedBuffers;
    this.failOnAppend = failOnAppend;
    this.sourceBuffer = null;
    this.ended = false;
  }

  addSourceBuffer(mimeType) {
    this.mimeType = mimeType;
    this.sourceBuffer = new FakeSourceBuffer(this.recordedBuffers, this.failOnAppend);
    return this.sourceBuffer;
  }

  endOfStream() {
    this.ended = true;
  }
}

class FakeVideo extends EventTarget {
  constructor() {
    super();
    this.src = "";
    this.playCalls = 0;
    this.pauseCalls = 0;
    this.loadCalls = 0;
  }

  async play() {
    this.playCalls += 1;
  }

  pause() {
    this.pauseCalls += 1;
  }

  load() {
    this.loadCalls += 1;
  }

  removeAttribute(name) {
    if (name === "src") {
      this.src = "";
    }
  }
}

function createDom() {
  const video = new FakeVideo();
  const status = { textContent: "", dataset: {} };
  const overlay = { hidden: false };
  const overlayText = { textContent: "" };
  const nodes = new Map([
    ["player-video", video],
    ["player-status", status],
    ["player-overlay", overlay],
    ["player-overlay-text", overlayText],
  ]);
  return {
    video,
    status,
    overlay,
    overlayText,
    documentObject: {
      getElementById(id) {
        return nodes.get(id);
      },
    },
  };
}

function createWindowObject() {
  const listeners = new Map();
  return {
    addEventListener(type, listener) {
      listeners.set(type, listener);
    },
    dispatch(type) {
      listeners.get(type)?.(new Event(type));
    },
  };
}

function createUrlObject(factory) {
  return {
    createObjectURL(mediaSource) {
      queueMicrotask(() => mediaSource.dispatchEvent(new Event("sourceopen")));
      return factory(mediaSource);
    },
    revokeObjectURL() {},
  };
}

function createMediaSourceClass(recordedBuffers, failOnAppend = false) {
  return class extends FakeMediaSource {
    static supported = FakeMediaSource.supported;

    constructor() {
      super(recordedBuffers, failOnAppend);
    }
  };
}

function createFetchHarness(responses) {
  const requests = [];
  return {
    requests,
    fetchImpl: async (url, options = {}) => {
      const op = String(url).split("/").pop();
      const body = JSON.parse(options.body);
      requests.push({ url, op, body, keepalive: options.keepalive === true });
      const next = responses.shift();
      if (!next) {
        throw new Error(`unexpected request for ${op}`);
      }
      return typeof next === "function" ? next({ op, body, options }) : next;
    },
  };
}

test("player opens, reads ordered media parts, and closes once on ended", async () => {
  const mintId = "ab".repeat(32);
  const handle = "cd".repeat(32);
  const openData = {
    schema: "elastos.library.runtime-custody-viewer/v1",
    mint_id: mintId,
    viewer_session_handle: handle,
    expires_at: 123,
    mime_type: "video/mp4",
    codecs: "avc1.640028",
    has_init_segment: true,
    segment_count: 2,
  };
  const part = (bytes) => ({
    schema: "elastos.library.runtime-custody-viewer-part/v1",
    mint_id: mintId,
    viewer_session_handle: handle,
    encoding: "base64",
    data: base64(bytes),
  });
  const finalSegment = createDeferred();
  const fetchHarness = createFetchHarness([
    jsonResponse({ status: "ok", data: openData }),
    jsonResponse({ status: "ok", data: part([1, 2, 3]) }),
    jsonResponse({ status: "ok", data: part([4, 5]) }),
    () => finalSegment.promise,
    jsonResponse({
      status: "ok",
      data: {
        schema: "elastos.library.runtime-custody-viewer/v1",
        mint_id: mintId,
        viewer_session_handle: handle,
        close_result: "closed",
      },
    }),
  ]);
  const recordedBuffers = [];
  const dom = createDom();
  const windowObject = createWindowObject();
  const MediaSourceClass = createMediaSourceClass(recordedBuffers);
  MediaSourceClass.supported.add('video/mp4; codecs="avc1.640028"');
  const controller = createPlayerController({
    documentObject: dom.documentObject,
    windowObject,
    locationObject: {
      search: `?mint_id=${mintId}`,
      hash: "#home_token=token-1",
    },
    fetchImpl: fetchHarness.fetchImpl,
    mediaSourceClass: MediaSourceClass,
    urlObject: createUrlObject(() => "blob:player"),
  });

  const playbackPromise = controller.startPlayback();
  await waitForMicrotaskCondition(
    () =>
      fetchHarness.requests.length === 4
      && dom.overlay.hidden === true
      && dom.status.textContent === "Playing"
      && dom.video.playCalls === 1,
  );

  assert.equal(dom.overlay.hidden, true);
  assert.equal(dom.status.textContent, "Playing");
  assert.equal(dom.video.playCalls, 1);
  assert.deepEqual(recordedBuffers, [[1, 2, 3], [4, 5]]);

  finalSegment.resolve(jsonResponse({ status: "ok", data: part([6, 7]) }));
  await playbackPromise;
  dom.video.dispatchEvent(new Event("ended"));
  await flushMicrotasks();

  assert.deepEqual(recordedBuffers, [[1, 2, 3], [4, 5], [6, 7]]);
  assert.equal(dom.overlay.hidden, true);
  assert.equal(dom.status.textContent, "Playing");
  assert.equal(dom.video.playCalls, 1);
  assert.deepEqual(
    fetchHarness.requests.map(({ op, body }) => ({ op, body })),
    [
      { op: "open_viewer", body: { mint_id: mintId } },
      {
        op: "read_viewer",
        body: { mint_id: mintId, viewer_session_handle: handle },
      },
      {
        op: "read_viewer",
        body: { mint_id: mintId, viewer_session_handle: handle, segment_index: 0 },
      },
      {
        op: "read_viewer",
        body: { mint_id: mintId, viewer_session_handle: handle, segment_index: 1 },
      },
      {
        op: "close_viewer",
        body: { mint_id: mintId, viewer_session_handle: handle },
      },
    ],
  );
});

test("player sends one keepalive close on pagehide", async () => {
  const mintId = "ef".repeat(32);
  const handle = "01".repeat(32);
  const fetchHarness = createFetchHarness([
    jsonResponse({
      status: "ok",
      data: {
        schema: "elastos.library.runtime-custody-viewer/v1",
        mint_id: mintId,
        viewer_session_handle: handle,
        expires_at: 123,
        mime_type: "video/mp4",
        codecs: "avc1.640028",
        has_init_segment: true,
        segment_count: 1,
      },
    }),
    jsonResponse({
      status: "ok",
      data: {
        schema: "elastos.library.runtime-custody-viewer-part/v1",
        mint_id: mintId,
        viewer_session_handle: handle,
        encoding: "base64",
        data: base64([9]),
      },
    }),
    jsonResponse({
      status: "ok",
      data: {
        schema: "elastos.library.runtime-custody-viewer-part/v1",
        mint_id: mintId,
        viewer_session_handle: handle,
        encoding: "base64",
        data: base64([8]),
      },
    }),
    jsonResponse({
      status: "ok",
      data: {
        schema: "elastos.library.runtime-custody-viewer/v1",
        mint_id: mintId,
        viewer_session_handle: handle,
        close_result: "closed",
      },
    }),
  ]);
  const dom = createDom();
  const windowObject = createWindowObject();
  const MediaSourceClass = createMediaSourceClass([]);
  MediaSourceClass.supported.add('video/mp4; codecs="avc1.640028"');
  const controller = createPlayerController({
    documentObject: dom.documentObject,
    windowObject,
    locationObject: {
      search: `?mint_id=${mintId}`,
      hash: "#home_token=token-2",
    },
    fetchImpl: fetchHarness.fetchImpl,
    mediaSourceClass: MediaSourceClass,
    urlObject: createUrlObject(() => "blob:player"),
  });

  await controller.startPlayback();
  windowObject.dispatch("pagehide");
  dom.video.dispatchEvent(new Event("ended"));
  await controller.closeViewer();

  assert.equal(
    fetchHarness.requests.filter((request) => request.op === "close_viewer").length,
    1,
  );
  assert.equal(
    fetchHarness.requests.find((request) => request.op === "close_viewer")?.keepalive,
    true,
  );
});

test("player keeps the first visible state when quiet close fails", async () => {
  const mintId = "90".repeat(32);
  const handle = "ab".repeat(32);
  const fetchHarness = createFetchHarness([
    jsonResponse({
      status: "ok",
      data: {
        schema: "elastos.library.runtime-custody-viewer/v1",
        mint_id: mintId,
        viewer_session_handle: handle,
        expires_at: 123,
        mime_type: "video/mp4",
        codecs: "avc1.640028",
        has_init_segment: true,
        segment_count: 1,
      },
    }),
    jsonResponse({
      status: "ok",
      data: {
        schema: "elastos.library.runtime-custody-viewer-part/v1",
        mint_id: mintId,
        viewer_session_handle: handle,
        encoding: "base64",
        data: base64([1]),
      },
    }),
    jsonResponse({
      status: "ok",
      data: {
        schema: "elastos.library.runtime-custody-viewer-part/v1",
        mint_id: mintId,
        viewer_session_handle: handle,
        encoding: "base64",
        data: base64([2]),
      },
    }),
    jsonResponse({ status: "error", message: "private close detail" }),
  ]);
  const dom = createDom();
  const windowObject = createWindowObject();
  const MediaSourceClass = createMediaSourceClass([]);
  MediaSourceClass.supported.add('video/mp4; codecs="avc1.640028"');
  const controller = createPlayerController({
    documentObject: dom.documentObject,
    windowObject,
    locationObject: {
      search: `?mint_id=${mintId}`,
      hash: "#home_token=token-quiet",
    },
    fetchImpl: fetchHarness.fetchImpl,
    mediaSourceClass: MediaSourceClass,
    urlObject: createUrlObject(() => "blob:quiet"),
  });

  const unhandled = [];
  const onUnhandled = (error) => unhandled.push(error);
  process.on("unhandledRejection", onUnhandled);
  try {
    await controller.startPlayback();
    windowObject.dispatch("pagehide");
    dom.video.dispatchEvent(new Event("ended"));
    await flushMicrotasks();
  } finally {
    process.off("unhandledRejection", onUnhandled);
  }

  assert.equal(dom.status.textContent, "Playing");
  assert.equal(unhandled.length, 0);
  assert.equal(
    fetchHarness.requests.filter((request) => request.op === "close_viewer").length,
    1,
  );
});

test("player fails closed on malformed part data and closes once", async () => {
  const mintId = "12".repeat(32);
  const handle = "34".repeat(32);
  const fetchHarness = createFetchHarness([
    jsonResponse({
      status: "ok",
      data: {
        schema: "elastos.library.runtime-custody-viewer/v1",
        mint_id: mintId,
        viewer_session_handle: handle,
        expires_at: 123,
        mime_type: "video/mp4",
        codecs: "avc1.640028",
        has_init_segment: true,
        segment_count: 1,
      },
    }),
    jsonResponse({
      status: "ok",
      data: {
        schema: "elastos.library.runtime-custody-viewer-part/v1",
        mint_id: mintId,
        viewer_session_handle: handle,
        encoding: "base64",
        data: "*",
      },
    }),
    jsonResponse({
      status: "ok",
      data: {
        schema: "elastos.library.runtime-custody-viewer/v1",
        mint_id: mintId,
        viewer_session_handle: handle,
        close_result: "closed",
      },
    }),
  ]);
  const dom = createDom();
  const MediaSourceClass = createMediaSourceClass([]);
  MediaSourceClass.supported.add('video/mp4; codecs="avc1.640028"');
  const controller = createPlayerController({
    documentObject: dom.documentObject,
    windowObject: createWindowObject(),
    locationObject: {
      search: `?mint_id=${mintId}`,
      hash: "#home_token=token-3",
    },
    fetchImpl: fetchHarness.fetchImpl,
    mediaSourceClass: MediaSourceClass,
    urlObject: createUrlObject(() => "blob:player"),
  });

  await controller.startPlayback();

  assert.equal(dom.overlay.hidden, false);
  assert.equal(dom.status.dataset.state, "error");
  assert.match(dom.status.textContent, /Video data is unavailable|Protected video is unavailable/);
  assert.equal(
    fetchHarness.requests.filter((request) => request.op === "close_viewer").length,
    1,
  );
});

test("player parser rejects malformed viewer data", async () => {
  const protectContractsSource = await readFile(
    new URL(
      "../elastos/crates/elastos-protected-content-provider-contracts/src/protect.rs",
      import.meta.url,
    ),
    "utf8",
  );
  const decryptContractsSource = await readFile(
    new URL(
      "../elastos/crates/elastos-protected-content-provider-contracts/src/decrypt.rs",
      import.meta.url,
    ),
    "utf8",
  );
  const maxSegmentsMatch = protectContractsSource.match(
    /pub const MAX_PROTECT_MEDIA_SEGMENTS_V1: u32 = (\d+);/,
  );
  const maxPartBytesMatch = decryptContractsSource.match(
    /pub const MAX_VIEWER_MEDIA_PART_BYTES_V1: usize = (\d+)\s*\*\s*1024\s*\*\s*1024;/,
  );
  assert.equal(
    Number(maxSegmentsMatch?.[1] || 0),
    MAX_VIEWER_SEGMENT_COUNT,
    "player segment bound must track the protected-content contract",
  );
  assert.equal(
    Number(maxPartBytesMatch?.[1] || 0) * 1024 * 1024,
    MAX_VIEWER_MEDIA_PART_BYTES,
    "player part-size bound must track the protected-content contract",
  );
  assert.throws(
    () =>
      parseViewerOpenData(
        {
          schema: "elastos.library.runtime-custody-viewer/v1",
          mint_id: "ab".repeat(32),
          viewer_session_handle: "cd".repeat(32),
          expires_at: 1,
          mime_type: "video/mp4",
          codecs: "avc1.640028",
          has_init_segment: true,
        },
        "ab".repeat(32),
      ),
    /Viewer response is unavailable/,
  );
  assert.throws(
    () =>
      parseViewerOpenData(
        {
          schema: "elastos.library.runtime-custody-viewer/v1",
          mint_id: "ab".repeat(32),
          viewer_session_handle: "cd".repeat(32),
          expires_at: 1,
          mime_type: "video/mp4",
          codecs: "avc1.640028",
          has_init_segment: true,
          segment_count: MAX_VIEWER_SEGMENT_COUNT + 1,
        },
        "ab".repeat(32),
      ),
    /Viewer response is unavailable/,
  );
  assert.throws(
    () =>
      parseViewerPartData(
        {
          schema: "elastos.library.runtime-custody-viewer-part/v1",
          mint_id: "ab".repeat(32),
          viewer_session_handle: "cd".repeat(32),
          encoding: "utf8",
          data: "abc",
        },
        "ab".repeat(32),
        "cd".repeat(32),
      ),
    /Video data is unavailable/,
  );
  assert.throws(
    () =>
      parseViewerPartData(
        {
          schema: "elastos.library.runtime-custody-viewer-part/v1",
          mint_id: "ab".repeat(32),
          viewer_session_handle: "cd".repeat(32),
          encoding: "base64",
          data: "AA",
        },
        "ab".repeat(32),
        "cd".repeat(32),
      ),
    /Video data is unavailable/,
  );
  assert.throws(
    () =>
      parseViewerPartData(
        {
          schema: "elastos.library.runtime-custody-viewer-part/v1",
          mint_id: "ab".repeat(32),
          viewer_session_handle: "cd".repeat(32),
          encoding: "base64",
          data: "AA=A",
        },
        "ab".repeat(32),
        "cd".repeat(32),
      ),
    /Video data is unavailable/,
  );
  const oversizedPartData = Buffer.alloc(MAX_VIEWER_MEDIA_PART_BYTES + 1, 7).toString("base64");
  assert.throws(
    () =>
      parseViewerPartData(
        {
          schema: "elastos.library.runtime-custody-viewer-part/v1",
          mint_id: "ab".repeat(32),
          viewer_session_handle: "cd".repeat(32),
          encoding: "base64",
          data: oversizedPartData,
        },
        "ab".repeat(32),
        "cd".repeat(32),
      ),
    /Video data is unavailable/,
  );
});

test("player source keeps only typed provider routes", async () => {
  const source = await readFile(
    new URL("../capsules/elacity-player/browser/player.js", import.meta.url),
    "utf8",
  );
  assert(!source.includes("/api/viewers/"), "player must keep the typed provider route");
  assert(!source.includes("principal_id"), "player must not send caller authority fields");
  assert(!source.includes("proof_binding_id"), "player must not send caller authority fields");
  assert(!source.includes("grant_id"), "player must not send caller authority fields");
});

test("player preserves the first failure and closes once under repeated media errors", async () => {
  const mintId = "56".repeat(32);
  const handle = "78".repeat(32);
  const fetchHarness = createFetchHarness([
    jsonResponse({
      status: "ok",
      data: {
        schema: "elastos.library.runtime-custody-viewer/v1",
        mint_id: mintId,
        viewer_session_handle: handle,
        expires_at: 123,
        mime_type: "video/mp4",
        codecs: "avc1.640028",
        has_init_segment: true,
        segment_count: 1,
      },
    }),
    jsonResponse({
      status: "ok",
      data: {
        schema: "elastos.library.runtime-custody-viewer-part/v1",
        mint_id: mintId,
        viewer_session_handle: handle,
        encoding: "base64",
        data: base64([1]),
      },
    }),
    jsonResponse({
      status: "ok",
      data: {
        schema: "elastos.library.runtime-custody-viewer-part/v1",
        mint_id: mintId,
        viewer_session_handle: handle,
        encoding: "base64",
        data: base64([2]),
      },
    }),
    jsonResponse({
      status: "ok",
      data: {
        schema: "elastos.library.runtime-custody-viewer/v1",
        mint_id: mintId,
        viewer_session_handle: handle,
        close_result: "closed",
      },
    }),
  ]);
  const dom = createDom();
  const windowObject = createWindowObject();
  const MediaSourceClass = createMediaSourceClass([], true);
  MediaSourceClass.supported.add('video/mp4; codecs="avc1.640028"');
  const controller = createPlayerController({
    documentObject: dom.documentObject,
    windowObject,
    locationObject: {
      search: `?mint_id=${mintId}`,
      hash: "#home_token=token-4",
    },
    fetchImpl: fetchHarness.fetchImpl,
    mediaSourceClass: MediaSourceClass,
    urlObject: createUrlObject(() => "blob:player"),
  });

  await controller.startPlayback();
  dom.video.dispatchEvent(new Event("error"));
  dom.video.dispatchEvent(new Event("error"));
  await Promise.resolve();

  assert.equal(dom.status.textContent, "Video data is unavailable.");
  assert.equal(
    fetchHarness.requests.filter((request) => request.op === "close_viewer").length,
    1,
  );
});
