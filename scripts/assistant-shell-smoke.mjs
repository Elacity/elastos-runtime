#!/usr/bin/env node

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const textEncoder = new TextEncoder();
const source = readFileSync(
  new URL("../capsules/assistant/browser/assistant.js", import.meta.url),
  "utf8",
);
const assistantModule = await import(
  `data:text/javascript,${encodeURIComponent(source)}`,
);

function offer(id, title, operation = "text.generate") {
  return {
    id,
    title,
    operation,
    input_modalities: ["text"],
    output_modalities: ["text"],
  };
}

function createFetchFixture({
  offers = [offer("offer:text-1", "Fast text")],
  workspace = {
    schema: "elastos.assistant.workspace/v1",
    revision: 0,
    sessions: [],
    draft: "",
    selected_offer_id: null,
  },
  createResponse = {
    status: "ok",
    data: {
      schema: "elastos.model.run/v1",
      run_id:
        "run:sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      offer_id: "offer:text-1",
      operation: "text.generate",
      status: "running",
      sequence_cursor: 0,
    },
  },
  eventPages = [],
  cancelResponse = null,
  putStatuses = [],
  unavailable = false,
} = {}) {
  const fetchCalls = [];
  const savedBodies = [];
  let workspaceState = structuredClone(workspace);
  let eventIndex = 0;
  let putIndex = 0;

  function jsonResponse(entry, fallback) {
    if (entry && typeof entry === "object" && "ok" in entry) {
      return {
        ok: Boolean(entry.ok),
        status: Number(entry.status ?? (entry.ok ? 200 : 500)),
        async json() {
          return structuredClone(entry.payload ?? fallback);
        },
      };
    }
    return {
      ok: true,
      status: 200,
      async json() {
        return structuredClone(entry ?? fallback);
      },
    };
  }

  async function fetchFn(url, init = {}) {
    fetchCalls.push([url, init]);
    if (unavailable && url === "/api/provider/model/offers_list") {
      return {
        ok: false,
        status: 503,
        async json() {
          return {
            status: "error",
            code: "provider_unavailable",
            message: "Model provider unavailable.",
          };
        },
      };
    }
    if (url === "/api/provider/model/offers_list") {
      return {
        ok: true,
        status: 200,
        async json() {
          return { status: "ok", data: { offers } };
        },
      };
    }
    if (url === "/api/apps/assistant/workspace" && init.method === "GET") {
      return {
        ok: true,
        status: 200,
        async json() {
          return structuredClone(workspaceState);
        },
      };
    }
    if (url === "/api/apps/assistant/workspace" && init.method === "POST") {
      const parsed = JSON.parse(init.body);
      savedBodies.push(parsed);
      const status = putStatuses[putIndex++] ?? 200;
      if (status === 409) {
        return {
          ok: false,
          status: 409,
          async json() {
            return {
              status: "error",
              code: "conflict",
              message: "assistant workspace revision conflict",
            };
          },
        };
      }
      workspaceState = {
        schema: parsed.schema,
        revision: parsed.if_revision + 1,
        sessions: parsed.sessions,
        draft: parsed.draft,
        selected_offer_id: parsed.selected_offer_id,
      };
      return {
        ok: true,
        status: 200,
        async json() {
          return structuredClone(workspaceState);
        },
      };
    }
    if (url === "/api/provider/model/runs_create") {
      return {
        ok: true,
        status: 200,
        async json() {
          return structuredClone(createResponse);
        },
      };
    }
    if (url === "/api/provider/model/runs_events") {
      return jsonResponse(
        eventPages[eventIndex++],
        {
          status: "ok",
          data: {
            schema: "elastos.model.run-events/v1",
            run_id: createResponse.data.run_id,
            next_cursor: 0,
            has_more: false,
            events: [],
          },
        },
      );
    }
    if (url === "/api/provider/model/runs_cancel") {
      return {
        ok: true,
        status: 200,
        async json() {
          return structuredClone(
            cancelResponse ?? {
              status: "ok",
              data: {
                schema: "elastos.model.run/v1",
                run_id: createResponse.data.run_id,
                offer_id: "offer:text-1",
                operation: "text.generate",
                status: "cancelled",
                sequence_cursor: 2,
                terminal: {
                  status: "cancelled",
                  error: {
                    class: "cancelled",
                    code: "cancelled",
                    message: "Model run cancelled.",
                  },
                },
              },
            },
          );
        },
      };
    }
    throw new Error(`Unexpected fetch ${url}`);
  }

  return { fetchFn, fetchCalls, savedBodies };
}

async function buildApp(options = {}) {
  const posts = [];
  const fetch = createFetchFixture(options);
  const timers = new Map();
  const timerOrder = [];
  let timerId = 0;
  let uuidCounter = 0;
  let nowMs = 0;
  const app = assistantModule.createAssistantApp({
    homeToken: "token-1",
    homeOrigin: "null",
    fetchFn: fetch.fetchFn,
    cryptoRef: {
      randomUUID() {
        uuidCounter += 1;
        return `uuid-${uuidCounter}`;
      },
    },
    nowFn() {
      return nowMs;
    },
    setTimeoutFn(callback, delay = 0) {
      timerId += 1;
      timers.set(timerId, { callback, delay });
      timerOrder.push(timerId);
      return timerId;
    },
    clearTimeoutFn(id) {
      timers.delete(id);
    },
    targetWindow: {
      postMessage(message, origin) {
        posts.push({ message, origin });
      },
    },
  });
  await app.initialize();
  async function flushAsync(turns = 8) {
    for (let index = 0; index < turns; index += 1) {
      await Promise.resolve();
    }
  }
  async function flushTimers(limit = 16) {
    let count = 0;
    while (timerOrder.length && count < limit) {
      const id = timerOrder.shift();
      const timer = timers.get(id);
      if (!timer) {
        continue;
      }
      timers.delete(id);
      nowMs += Number(timer.delay) || 0;
      await timer.callback();
      await flushAsync();
      count += 1;
    }
  }
  return {
    app,
    posts,
    fetch,
    flushAsync,
    flushTimers,
    pendingTimerCount() {
      return timers.size;
    },
  };
}

{
  const { app, posts, fetch } = await buildApp({ offers: [] });
  assert.equal(posts.length, 1);
  assert.equal(posts[0].origin, "null");
  assert.equal(posts[0].message.type, "home:app-ready");
  assert.equal(posts[0].message.homeToken, "token-1");
  assert.equal(fetch.fetchCalls[0][0], "/api/provider/model/offers_list");
  assert.equal(fetch.fetchCalls[1][0], "/api/apps/assistant/workspace");
  assert.equal(app.snapshot().statusMessage, "No model offers available.");
}

{
  const { app } = await buildApp({ unavailable: true });
  assert.equal(app.snapshot().statusMessage, "Model provider unavailable.");
}

{
  const { app, fetch, flushAsync, flushTimers } = await buildApp({
    workspace: {
      schema: "elastos.assistant.workspace/v1",
      revision: 4,
      sessions: [
        {
          id: "session-1",
          title: "Saved chat",
          mode: "chat",
          pinned: false,
          messages: [],
        },
      ],
      draft: "Saved draft",
      selected_offer_id: "offer:text-1",
    },
    eventPages: [
      {
        status: "ok",
        data: {
          schema: "elastos.model.run-events/v1",
          run_id:
            "run:sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
          next_cursor: 1,
          has_more: true,
          events: [
            {
              schema: "elastos.model.run-event/v1",
              sequence: 1,
              kind: "text_delta",
              terminal: false,
              data: { text: "Hello " },
            },
          ],
        },
      },
      {
        status: "ok",
        data: {
          schema: "elastos.model.run-events/v1",
          run_id:
            "run:sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
          next_cursor: 2,
          has_more: false,
          events: [
            {
              schema: "elastos.model.run-event/v1",
              sequence: 2,
              kind: "output",
              terminal: true,
              data: {
                schema: "elastos.model.output.text/v1",
                text: "Hello world",
              },
            },
          ],
        },
      },
    ],
  });
  assert.equal(app.snapshot().draft, "Saved draft");
  app.setDraft("Write a clean recap.");
  const sent = await app.sendDraft();
  assert.equal(sent, true);
  await flushAsync();
  await flushTimers();
  const createCall = fetch.fetchCalls.find(
    ([url]) => url === "/api/provider/model/runs_create",
  );
  assert.ok(createCall);
  const createBody = JSON.parse(createCall[1].body);
  assert.deepEqual(createBody.input, {
    schema: "elastos.model.input.text/v1",
    prompt: "Write a clean recap.",
  });
  assert.equal(createBody.offer_id, "offer:text-1");
  assert.equal(createBody.operation, "text.generate");
  assert.match(createBody.request_id, /^uuid-/);
  const messages = app.snapshot().currentSession.messages;
  assert.equal(messages.at(-1).content, "Hello world");
}

{
  const longPrompt = "x".repeat(8 * 1024 + 64);
  const { app, fetch } = await buildApp();
  app.setDraft(longPrompt);
  const sent = await app.sendDraft();
  assert.equal(sent, true);
  const createCall = fetch.fetchCalls.find(
    ([url]) => url === "/api/provider/model/runs_create",
  );
  assert.ok(createCall);
  const createBody = JSON.parse(createCall[1].body);
  const userMessage = app.snapshot().currentSession.messages.find(
    (message) => message.role === "user",
  );
  assert.equal(createBody.input.prompt.length, 8 * 1024);
  assert.equal(userMessage.content.length, 8 * 1024);
  assert.equal(createBody.input.prompt, userMessage.content);
}

{
  const longPrompt = `${"🙂".repeat(2048)}trim-me`;
  const { app, fetch } = await buildApp();
  app.setDraft(longPrompt);
  const sent = await app.sendDraft();
  assert.equal(sent, true);
  const createCall = fetch.fetchCalls.find(
    ([url]) => url === "/api/provider/model/runs_create",
  );
  assert.ok(createCall);
  const createBody = JSON.parse(createCall[1].body);
  const userMessage = app.snapshot().currentSession.messages.find(
    (message) => message.role === "user",
  );
  assert.equal(textEncoder.encode(createBody.input.prompt).length, 8 * 1024);
  assert.equal(createBody.input.prompt, "🙂".repeat(2048));
  assert.equal(createBody.input.prompt, userMessage.content);
}

{
  const { app, fetch, flushAsync, flushTimers, pendingTimerCount } = await buildApp({
    eventPages: [
      {
        ok: false,
        status: 503,
        payload: {
          status: "error",
          code: "provider_unavailable",
          message: "Model provider unavailable.",
        },
      },
      {
        ok: false,
        status: 503,
        payload: {
          status: "error",
          code: "provider_unavailable",
          message: "Model provider unavailable.",
        },
      },
      {
        ok: false,
        status: 503,
        payload: {
          status: "error",
          code: "provider_unavailable",
          message: "Model provider unavailable.",
        },
      },
      {
        ok: false,
        status: 503,
        payload: {
          status: "error",
          code: "provider_unavailable",
          message: "Model provider unavailable.",
        },
      },
      {
        ok: false,
        status: 503,
        payload: {
          status: "error",
          code: "provider_unavailable",
          message: "Model provider unavailable.",
        },
      },
    ],
  });
  app.setDraft("Retry within bounds.");
  await app.sendDraft();
  await flushAsync();
  await flushTimers(12);
  assert.equal(
    fetch.fetchCalls.filter(([url]) => url === "/api/provider/model/runs_events").length,
    4,
  );
  assert.equal(app.snapshot().statusMessage, "Model provider unavailable.");
  assert.equal(app.snapshot().activeRun?.terminal, false);
  assert.equal(pendingTimerCount(), 0);
}

{
  const longEventPages = Array.from({ length: 9 }, (_, index) => ({
    status: "ok",
    data: {
      schema: "elastos.model.run-events/v1",
      run_id:
        "run:sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      next_cursor: index + 1,
      has_more: index < 8,
      events: [
        {
          schema: "elastos.model.run-event/v1",
          sequence: index + 1,
          kind: index < 8 ? "text_delta" : "output",
          terminal: index === 8,
          data:
            index < 8
              ? { text: String(index + 1) }
              : {
                  schema: "elastos.model.output.text/v1",
                  text: "12345678 done",
                },
        },
      ],
    },
  }));
  const { app, fetch, flushAsync, flushTimers, pendingTimerCount } = await buildApp({
    eventPages: longEventPages,
  });
  app.setDraft("Yield after bounded immediate pages.");
  await app.sendDraft();
  await flushAsync();
  await flushTimers(16);
  assert.equal(app.snapshot().currentSession.messages.at(-1).content, "12345678 done");
  assert.equal(
    fetch.fetchCalls.filter(([url]) => url === "/api/provider/model/runs_events").length,
    9,
  );
  assert.equal(pendingTimerCount(), 0);
}

{
  const { app, fetch, flushAsync, flushTimers, pendingTimerCount } = await buildApp({
    eventPages: [
      {
        status: "ok",
        data: {
          schema: "elastos.model.run-events/v1",
          run_id:
            "run:sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
          next_cursor: 1,
          has_more: true,
          events: [
            {
              schema: "elastos.model.run-event/v1",
              sequence: 1,
              kind: "text_delta",
              terminal: false,
              data: { text: "Hello " },
            },
          ],
        },
      },
      {
        status: "ok",
        data: {
          schema: "elastos.model.run-events/v1",
          run_id:
            "run:sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
          next_cursor: 1,
          has_more: true,
          events: [
            {
              schema: "elastos.model.run-event/v1",
              sequence: 1,
              kind: "text_delta",
              terminal: false,
              data: { text: "Hello " },
            },
          ],
        },
      },
    ],
  });
  app.setDraft("Do not duplicate text.");
  await app.sendDraft();
  await flushAsync();
  await flushTimers(8);
  const assistantMessages = app
    .snapshot()
    .currentSession.messages.filter((message) => message.role === "assistant");
  assert.equal(assistantMessages.length, 1);
  assert.equal(assistantMessages[0].content, "Hello ");
  assert.equal(app.snapshot().statusMessage, "Model provider unavailable.");
  assert.equal(
    fetch.fetchCalls.filter(([url]) => url === "/api/provider/model/runs_events").length,
    2,
  );
  assert.equal(pendingTimerCount(), 0);
}

{
  const posts = [];
  const timers = new Map();
  const timerOrder = [];
  let timerId = 0;
  let nowMs = 0;
  const savedBodies = [];
  let workspaceRevision = 4;
  let resolveFirstSave = null;
  const app = assistantModule.createAssistantApp({
    homeToken: "token-1",
    homeOrigin: "null",
    cryptoRef: {
      randomUUID() {
        return "uuid-1";
      },
    },
    nowFn() {
      return nowMs;
    },
    setTimeoutFn(callback, delay = 0) {
      timerId += 1;
      timers.set(timerId, { callback, delay });
      timerOrder.push(timerId);
      return timerId;
    },
    clearTimeoutFn(id) {
      timers.delete(id);
    },
    targetWindow: {
      postMessage(message, origin) {
        posts.push({ message, origin });
      },
    },
    fetchFn(url, init = {}) {
      if (url === "/api/provider/model/offers_list") {
        return Promise.resolve({
          ok: true,
          status: 200,
          async json() {
            return { status: "ok", data: { offers: [offer("offer:text-1", "Fast text")] } };
          },
        });
      }
      if (url === "/api/apps/assistant/workspace" && init.method === "GET") {
        return Promise.resolve({
          ok: true,
          status: 200,
          async json() {
            return {
              schema: "elastos.assistant.workspace/v1",
              revision: workspaceRevision,
              sessions: [
                {
                  id: "session-1",
                  title: "Saved",
                  mode: "chat",
                  pinned: false,
                  messages: [],
                },
              ],
              draft: "",
              selected_offer_id: "offer:text-1",
            };
          },
        });
      }
      if (url === "/api/apps/assistant/workspace" && init.method === "POST") {
        const parsed = JSON.parse(init.body);
        savedBodies.push(parsed);
        if (savedBodies.length === 1) {
          return new Promise((resolve) => {
            resolveFirstSave = () => {
              workspaceRevision = parsed.if_revision + 1;
              resolve({
                ok: true,
                status: 200,
                async json() {
                  return {
                    schema: "elastos.assistant.workspace/v1",
                    revision: workspaceRevision,
                    sessions: parsed.sessions,
                    draft: parsed.draft,
                    selected_offer_id: parsed.selected_offer_id,
                  };
                },
              });
            };
          });
        }
        workspaceRevision = parsed.if_revision + 1;
        return Promise.resolve({
          ok: true,
          status: 200,
          async json() {
            return {
              schema: "elastos.assistant.workspace/v1",
              revision: workspaceRevision,
              sessions: parsed.sessions,
              draft: parsed.draft,
              selected_offer_id: parsed.selected_offer_id,
            };
          },
        });
      }
      throw new Error(`Unexpected fetch ${url}`);
    },
  });
  await app.initialize();
  app.setDraft("first draft");
  {
    const id = timerOrder.shift();
    const timer = timers.get(id);
    timers.delete(id);
    nowMs += Number(timer.delay) || 0;
    const inFlight = timer.callback();
    await Promise.resolve();
    app.setDraft("second draft");
    resolveFirstSave();
    await Promise.resolve();
    await Promise.resolve();
    await inFlight;
  }
  while (timerOrder.length) {
    const id = timerOrder.shift();
    const timer = timers.get(id);
    if (!timer) {
      continue;
    }
    timers.delete(id);
    nowMs += Number(timer.delay) || 0;
    await timer.callback();
  }
  assert.equal(posts.length, 1);
  assert.equal(savedBodies.length, 2);
  assert.equal(savedBodies[0].draft, "first draft");
  assert.equal(savedBodies[1].draft, "second draft");
  assert.equal(app.snapshot().draft, "second draft");
  assert.equal(app.snapshot().conflictMessage, "");
}

{
  const { app, fetch, flushAsync, flushTimers } = await buildApp({
    eventPages: [
      {
        status: "ok",
        data: {
          schema: "elastos.model.run-events/v1",
          run_id:
            "run:sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
          next_cursor: 1,
          has_more: false,
          events: [
            {
              schema: "elastos.model.run-event/v1",
              sequence: 1,
              kind: "failed",
              terminal: true,
              data: {
                class: "backend_failed",
                code: "provider_error",
                message: "Provider failed.",
              },
            },
          ],
        },
      },
    ],
  });
  app.setDraft("Fail cleanly.");
  await app.sendDraft();
  await flushAsync();
  await flushTimers();
  assert.equal(app.snapshot().statusMessage, "Provider failed.");

  const { app: cancelledApp, fetch: cancelledFetch } = await buildApp();
  cancelledApp.setDraft("Cancel cleanly.");
  await cancelledApp.sendDraft();
  const stopped = await cancelledApp.stopRun();
  assert.equal(stopped, true);
  assert.equal(
    cancelledFetch.fetchCalls.filter(
      ([url]) => url === "/api/provider/model/runs_cancel",
    ).length,
    1,
  );
  assert.equal(cancelledApp.snapshot().statusMessage, "Model run cancelled.");

  const { app: unknownApp, flushAsync: flushUnknownAsync, flushTimers: flushUnknown } = await buildApp({
    eventPages: [
      {
        status: "ok",
        data: {
          schema: "elastos.model.run-events/v1",
          run_id:
            "run:sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
          next_cursor: 1,
          has_more: false,
          events: [
            {
              schema: "elastos.model.run-event/v1",
              sequence: 1,
              kind: "settlement_unknown",
              terminal: true,
              data: {
                class: "settlement_unknown",
                code: "settlement_unknown",
                message: "Settlement unknown.",
              },
            },
          ],
        },
      },
    ],
  });
  unknownApp.setDraft("Unknown cleanly.");
  await unknownApp.sendDraft();
  await flushUnknownAsync();
  await flushUnknown();
  assert.equal(unknownApp.snapshot().statusMessage, "Settlement unknown.");
}

{
  const { app, fetch, flushTimers } = await buildApp({
    putStatuses: [200, 409],
    workspace: {
      schema: "elastos.assistant.workspace/v1",
      revision: 2,
      sessions: [
        {
          id: "session-1",
          title: "Alpha",
          mode: "chat",
          pinned: false,
          messages: [{ role: "user", content: "alpha body" }],
        },
        {
          id: "session-2",
          title: "Beta",
          mode: "build",
          pinned: false,
          messages: [{ role: "assistant", content: "beta body" }],
        },
      ],
      draft: "",
      selected_offer_id: "offer:text-1",
    },
  });
  app.togglePinSession("session-2");
  await flushTimers();
  assert.equal(fetch.savedBodies[0].sessions[1].pinned, true);
  app.setSearchQuery("beta");
  assert.equal(app.snapshot().filteredSessions.length, 1);
  assert.equal(app.snapshot().filteredSessions[0].id, "session-2");
  app.setDraft("conflict");
  await flushTimers();
  assert.equal(
    app.snapshot().conflictMessage,
    "Workspace changed elsewhere. Reloaded the latest saved state.",
  );
}

assert(!/\blocalStorage\b|\bsessionStorage\b|\bindexedDB\b/.test(source));
assert(!/navigator\.clipboard|execCommand/.test(source));
assert(
  !/(["'](?:runtime_binding|principal_id|session_id|grant_id|backend_url|api_key|bearer)["']\s*:|\b(?:runtime_binding|principal_id|session_id|grant_id|backend_url|api_key|bearer)\s*:)/i.test(source),
);
assert(!/typed text runs only|Build mode uses the same typed text runs/i.test(source));
