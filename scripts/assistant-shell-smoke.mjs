#!/usr/bin/env node

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const textEncoder = new TextEncoder();
const homeClipboardClientUrl = new URL(
  "../capsules/home/browser/home-clipboard-client.js",
  import.meta.url,
).href;
const katexModuleUrl = new URL(
  "../capsules/assistant/browser/vendor/katex/katex.mjs",
  import.meta.url,
).href;
const originalSource = readFileSync(
  new URL("../capsules/assistant/browser/assistant.js", import.meta.url),
  "utf8",
);
const originalIndexSource = readFileSync(
  new URL("../capsules/assistant/browser/index.html", import.meta.url),
  "utf8",
);
const source = `${originalSource
  .replace(
    '"/apps/home/home-clipboard-client.js?v=home-20260726a"',
    JSON.stringify(homeClipboardClientUrl),
  )
  .replace(
    '"./vendor/katex/katex.mjs"',
    JSON.stringify(katexModuleUrl),
  )}
export { renderMarkdown, renderMessageBody };
`;
const assistantModule = await import(
  `data:text/javascript,${encodeURIComponent(source)}`,
);

function offer(id, title, operation = "text.generate") {
  return {
    id,
    title,
    operation,
    input_modalities: ["text/plain"],
    output_modalities: ["text/plain"],
  };
}

function studioOffer(id, title, operation = "image.generate") {
  return {
    id,
    title,
    operation,
    input_modalities: ["application/json"],
    output_modalities: ["application/json"],
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
  const clipboardWrites = [];
  const fetch = createFetchFixture(options);
  const timers = new Map();
  const timerOrder = [];
  let timerId = 0;
  let uuidCounter = 0;
  let nowMs = 0;
  const clipboard = {
    config: null,
    startCount: 0,
    async onWrite(text, writeOptions) {
      if (typeof options.onClipboardWrite === "function") {
        return options.onClipboardWrite(text, writeOptions);
      }
      if (options.clipboardError) {
        throw options.clipboardError;
      }
      return true;
    },
  };
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
    homeClipboardClientFactory(config) {
      clipboard.config = config;
      return {
        start() {
          clipboard.startCount += 1;
          if (config.targetWindow && typeof config.targetWindow.postMessage === "function") {
            config.targetWindow.postMessage(
              {
                type: "home:app-ready",
                homeToken: config.homeToken,
              },
              config.homeOrigin,
            );
          }
        },
        async writeText(text, writeOptions) {
          clipboardWrites.push({ text, writeOptions });
          return clipboard.onWrite(text, writeOptions);
        },
      };
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
    clipboard,
    clipboardWrites,
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
  const rendered = assistantModule.renderMessageBody(
    {
      role: "assistant",
      content: [
        "# Heading",
        "",
        "Paragraph with **bold**, *italic*, `code`, and [Docs](https://example.com/docs).",
        "",
        "- one",
        "- two",
        "",
        "> quoted",
        "",
        "| Name | Value |",
        "| --- | --- |",
        "| alpha | 1 |",
        "",
        "Inline math $x^2$.",
        "",
        "$$",
        "\\frac{1}{2}",
        "$$",
        "",
        "```js",
        "const value = 1;",
        "```",
      ].join("\n"),
    },
    {},
  );
  assert.match(rendered, /assistant-md-h assistant-md-h1/);
  assert.match(rendered, /assistant-md-list/);
  assert.match(rendered, /assistant-md-quote/);
  assert.match(rendered, /assistant-md-table/);
  assert.match(rendered, /assistant-md-code/);
  assert.match(rendered, /assistant-md-inline/);
  assert.match(rendered, /class="katex"/);
  assert.equal(/<a\b|href=|target=/.test(rendered), false);
  assert.match(rendered, /assistant-md-link/);
  assert.match(rendered, /assistant-md-link-url/);
}

{
  const rendered = assistantModule.renderMessageBody(
    {
      role: "assistant",
      content: "| Name | Value |\n| --- | --- |\n| alpha | 1 |",
    },
    {},
  );
  assert.match(rendered, /assistant-md-table/);
  assert.match(rendered, /<th>Name<\/th>/);
  assert.match(rendered, /<td>1<\/td>/);
}

{
  const rendered = assistantModule.renderMessageBody(
    {
      role: "assistant",
      content: "Pipe text stays plain: alpha | beta | gamma",
    },
    {},
  );
  assert.match(rendered, /assistant-md-p/);
  assert.equal(rendered.includes("assistant-md-table"), false);
  assert.match(rendered, /alpha \| beta \| gamma/);
}

{
  const rendered = assistantModule.renderMessageBody(
    {
      role: "assistant",
      content: 'Unsafe <img src=x onerror=1> and <script>alert(1)</script> stay inert.',
    },
    {},
  );
  assert.equal(rendered.includes("<img"), false);
  assert.equal(rendered.includes("<script"), false);
  assert.match(rendered, /&lt;img src=x onerror=1&gt;/);
  assert.match(rendered, /&lt;script&gt;alert\(1\)&lt;\/script&gt;/);
}

{
  const rendered = assistantModule.renderMessageBody(
    {
      role: "assistant",
      content:
        "Literal `*bold* $x^2$ [AT&T](https://example.com?a=1&b=2)` and math $a*b$.",
    },
    {},
  );
  assert.match(
    rendered,
    /<code class="assistant-md-inline">\*bold\* \$x\^2\$ \[AT&amp;T\]\(https:\/\/example\.com\?a=1&amp;b=2\)<\/code>/,
  );
  assert.match(rendered, /annotation encoding="application\/x-tex">a\*b<\/annotation>/);
  assert.match(rendered, /assistant-md-math/);
  assert.equal(rendered.includes("AT&amp;amp;T"), false);
  assert.equal(rendered.includes("a=1&amp;amp;b=2"), false);
}

{
  const rendered = assistantModule.renderMessageBody(
    {
      role: "assistant",
      content: "Entity stays literal in math $&amp;lt;$.",
    },
    {},
  );
  assert.match(rendered, /assistant-md-math-raw/);
  assert.match(rendered, /&amp;amp;amp;lt;/);
  assert.equal(rendered.includes("&amp;amp;lt;"), false);
  assert.equal(rendered.includes('class="katex"'), false);
}

{
  const rendered = assistantModule.renderMessageBody(
    {
      role: "assistant",
      content: "Malformed math $\\badcommand{}$ still shows as text.",
    },
    {},
  );
  assert.match(rendered, /assistant-md-math-raw/);
  assert.match(rendered, /\\badcommand\{\}/);
}

{
  const rendered = assistantModule.renderMessageBody(
    {
      role: "assistant",
      content: "$$\n\\frac{1}{2}",
    },
    {},
  );
  assert.match(rendered, /\$\$/);
  assert.equal(rendered.includes("assistant-md-math-block"), false);
}

{
  const rendered = assistantModule.renderMessageBody(
    {
      role: "assistant",
      content: "\\[\n\\frac{1}{2}",
    },
    { streaming: true },
  );
  assert.match(rendered, /\\\[/);
  assert.equal(rendered.includes("assistant-md-math-block"), false);
}

{
  const rendered = assistantModule.renderMessageBody(
    {
      role: "user",
      content: "# Keep this plain\n<script>alert(1)</script>\n`code`",
    },
    {},
  );
  assert.equal(/assistant-md-h|assistant-md-code|class="katex"/.test(rendered), false);
  assert.match(rendered, /&lt;script&gt;alert\(1\)&lt;\/script&gt;/);
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
  const { app, clipboard, clipboardWrites } = await buildApp({
    workspace: {
      schema: "elastos.assistant.workspace/v1",
      revision: 1,
      sessions: [
        {
          id: "session-1",
          title: "Saved chat",
          mode: "chat",
          pinned: false,
          messages: [
            { role: "user", content: "Outline the next patch." },
            {
              role: "assistant",
              content: "Start with the smallest safe edit.",
              run_id:
                "run:sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            },
            { role: "system", content: "hidden system detail" },
          ],
        },
      ],
      draft: "",
      selected_offer_id: "offer:text-1",
    },
  });
  assert.equal(clipboard.config.targetId, "assistant");
  assert.equal(await app.copyTranscript(), true);
  assert.equal(clipboardWrites.length, 1);
  assert.deepEqual(clipboardWrites[0].writeOptions, {
    purpose: "transcript.markdown",
  });
  assert.equal(
    clipboardWrites[0].text,
    [
      "# Assistant transcript",
      "",
      "## User",
      "",
      "Outline the next patch.",
      "",
      "## Assistant",
      "",
      "Start with the smallest safe edit.",
    ].join("\n"),
  );
  assert.equal(/run:sha256:|offer:text-1|revision|hidden system detail/.test(clipboardWrites[0].text), false);
  assert.equal(app.snapshot().copyStatusMessage, "Transcript copied.");
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
  const unavailableError = new Error("not ready");
  unavailableError.code = "unavailable";
  const { app, clipboardWrites } = await buildApp({
    workspace: {
      schema: "elastos.assistant.workspace/v1",
      revision: 1,
      sessions: [
        {
          id: "session-1",
          title: "Saved chat",
          mode: "chat",
          pinned: false,
          messages: [{ role: "user", content: "Retry later." }],
        },
      ],
      draft: "",
      selected_offer_id: "offer:text-1",
    },
    clipboardError: unavailableError,
  });
  assert.equal(await app.copyTranscript(), false);
  assert.equal(clipboardWrites.length, 1);
  assert.equal(app.snapshot().copyStatusMessage, "Clipboard unavailable.");
}

{
  const hugeMessage = "🙂".repeat(2048);
  const { app, clipboardWrites } = await buildApp({
    workspace: {
      schema: "elastos.assistant.workspace/v1",
      revision: 1,
      sessions: [
        {
          id: "session-1",
          title: "Saved chat",
          mode: "chat",
          pinned: false,
          messages: Array.from({ length: 16 }, (_, index) => ({
            role: index % 2 === 0 ? "user" : "assistant",
            content: hugeMessage,
          })),
        },
      ],
      draft: "",
      selected_offer_id: "offer:text-1",
    },
  });
  assert.equal(await app.copyTranscript(), true);
  assert.equal(clipboardWrites.length, 1);
  assert.equal(
    textEncoder.encode(clipboardWrites[0].text).length <= 65_536,
    true,
  );
  assert.match(
    clipboardWrites[0].text,
    /> Note: Transcript truncated to fit the trusted Home Clipboard limit\.$/,
  );
  assert.equal(clipboardWrites[0].text.includes("\ufffd"), false);
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
  const { app, clipboardWrites } = await buildApp();
  app.setDraft("Do not copy partial output.");
  await app.sendDraft();
  assert.equal(app.snapshot().copyDisabled, true);
  assert.equal(await app.copyTranscript(), false);
  assert.equal(clipboardWrites.length, 0);
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
    homeClipboardClientFactory(config) {
      return {
        start() {
          config.targetWindow?.postMessage(
            {
              type: "home:app-ready",
              homeToken: config.homeToken,
            },
            config.homeOrigin,
          );
        },
        async writeText() {
          return true;
        },
      };
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

{
  const { app } = await buildApp({
    offers: [
      {
        id: "offer:legacy-text",
        title: "Legacy text",
        operation: "text.generate",
        input_modalities: ["text"],
        output_modalities: ["text"],
      },
    ],
  });
  assert.equal(app.snapshot().offersReady.length, 0);
  assert.equal(app.snapshot().statusMessage, "No model offers available.");
}

{
  const { app } = await buildApp({
    workspace: {
      schema: "elastos.assistant.workspace/v1",
      revision: 1,
      sessions: [
        {
          id: "session-1",
          title: "Saved chat",
          mode: "chat",
          pinned: false,
          messages: [{ role: "user", content: "Visible chat history." }],
        },
      ],
      draft: "",
      selected_offer_id: "offer:text-1",
    },
  });
  app.setSessionMode("studio");
  const view = app.snapshot();
  assert.equal(view.currentMode, "studio");
  assert.equal(view.copyHidden, true);
  assert.equal(view.copyDisabled, true);
  assert.equal(view.studioUnavailable, true);
}

{
  const { app, fetch } = await buildApp({
    offers: [offer("offer:text-1", "Fast text"), studioOffer("offer:vision-1", "Vision", "image.generate")],
    createResponse: {
      status: "error",
      code: "provider_unavailable",
      message: "Model provider unavailable.",
    },
    workspace: {
      schema: "elastos.assistant.workspace/v1",
      revision: 7,
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
  });
  const before = app.snapshot();
  app.setSessionMode("studio");
  app.setDraft("Poster prompt");
  const sent = await app.sendDraft();
  const after = app.snapshot();
  assert.equal(sent, false);
  assert.equal(after.draft, "Poster prompt");
  assert.equal(after.workspaceRevision, before.workspaceRevision);
  assert.equal(after.workspaceVersion, before.workspaceVersion);
  assert.equal(fetch.savedBodies.length, 0);
  assert.equal(
    fetch.fetchCalls.filter(([url]) => url === "/api/provider/model/runs_create").length,
    1,
  );
}

{
  const runId =
    "run:sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
  const { app, fetch } = await buildApp({
    offers: [studioOffer("offer:vision-1", "Vision", "image.generate")],
    createResponse: {
      status: "ok",
      data: {
        schema: "elastos.model.run/v1",
        run_id: runId,
        offer_id: "offer:vision-1",
        operation: "image.generate",
        status: "running",
        sequence_cursor: 0,
      },
    },
  });
  app.setSessionMode("studio");
  app.setDraft("Design a quiet poster.");
  assert.equal(await app.sendDraft(), true);
  const createCall = fetch.fetchCalls.find(
    ([url]) => url === "/api/provider/model/runs_create",
  );
  assert.ok(createCall);
  assert.deepEqual(JSON.parse(createCall[1].body).input, {
    schema: "elastos.model.input.image/v1",
    prompt: "Design a quiet poster.",
  });
}

{
  const runId =
    "run:sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
  const { app, fetch } = await buildApp({
    offers: [studioOffer("offer:motion-1", "Motion", "video.generate")],
    createResponse: {
      status: "ok",
      data: {
        schema: "elastos.model.run/v1",
        run_id: runId,
        offer_id: "offer:motion-1",
        operation: "video.generate",
        status: "running",
        sequence_cursor: 0,
      },
    },
  });
  app.setSessionMode("studio");
  app.setDraft("Generate a short teaser.");
  assert.equal(await app.sendDraft(), true);
  const createCall = fetch.fetchCalls.find(
    ([url]) => url === "/api/provider/model/runs_create",
  );
  assert.ok(createCall);
  assert.deepEqual(JSON.parse(createCall[1].body).input, {
    schema: "elastos.model.input.video/v1",
    prompt: "Generate a short teaser.",
  });
}

{
  const runId =
    "run:sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
  const { app, flushAsync, flushTimers } = await buildApp({
    offers: [studioOffer("offer:vision-2", "Vision", "image.generate")],
    createResponse: {
      status: "ok",
      data: {
        schema: "elastos.model.run/v1",
        run_id: runId,
        offer_id: "offer:vision-2",
        operation: "image.generate",
        status: "running",
        sequence_cursor: 0,
      },
    },
    eventPages: [
      {
        status: "ok",
        data: {
          schema: "elastos.model.run-events/v1",
          run_id: runId,
          next_cursor: 1,
          has_more: false,
          events: [
            {
              schema: "elastos.model.run-event/v1",
              sequence: 1,
              kind: "progress",
              terminal: false,
              data: { phase: "rendering", completed: 1, total: 4 },
            },
          ],
        },
      },
    ],
  });
  app.setSessionMode("studio");
  app.setDraft("Create cover art.");
  await app.sendDraft();
  await flushAsync();
  await flushTimers();
  assert.deepEqual(app.snapshot().studioProgress, {
    phase: "rendering",
    completed: 1,
    total: 4,
  });
}

{
  const runId =
    "run:sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
  const { app, flushAsync, flushTimers } = await buildApp({
    offers: [studioOffer("offer:motion-2", "Motion", "video.generate")],
    createResponse: {
      status: "ok",
      data: {
        schema: "elastos.model.run/v1",
        run_id: runId,
        offer_id: "offer:motion-2",
        operation: "video.generate",
        status: "running",
        sequence_cursor: 0,
      },
    },
    eventPages: [
      {
        status: "ok",
        data: {
          schema: "elastos.model.run-events/v1",
          run_id: runId,
          next_cursor: 2,
          has_more: true,
          events: [
            {
              schema: "elastos.model.run-event/v1",
              sequence: 1,
              kind: "progress",
              terminal: false,
              data: { phase: "rendering", completed: 2, total: 5 },
            },
            {
              schema: "elastos.model.run-event/v1",
              sequence: 2,
              kind: "output",
              terminal: true,
              data: {
                schema: "elastos.model.output.content/v1",
                resource_id: "result-1",
              },
            },
          ],
        },
      },
    ],
  });
  app.setSessionMode("studio");
  app.setDraft("Create a trailer.");
  await app.sendDraft();
  await flushAsync();
  await flushTimers();
  const view = app.snapshot();
  assert.equal(view.statusMessage, "");
  assert.equal(view.currentSession.messages.length, 0);
  assert.deepEqual(view.studioResult, {
    mediaLabel: "Video",
    schema: "elastos.model.output.content/v1",
    resourceId: "result-1",
  });
}

{
  const runId =
    "run:sha256:abababababababababababababababababababababababababababababababab";
  const longResourceId = "a".repeat(4097);
  const { app, flushAsync, flushTimers } = await buildApp({
    offers: [studioOffer("offer:vision-3", "Vision", "image.generate")],
    createResponse: {
      status: "ok",
      data: {
        schema: "elastos.model.run/v1",
        run_id: runId,
        offer_id: "offer:vision-3",
        operation: "image.generate",
        status: "running",
        sequence_cursor: 0,
      },
    },
    eventPages: [
      {
        status: "ok",
        data: {
          schema: "elastos.model.run-events/v1",
          run_id: runId,
          next_cursor: 1,
          has_more: false,
          events: [
            {
              schema: "elastos.model.run-event/v1",
              sequence: 1,
              kind: "output",
              terminal: true,
              data: {
                schema: "elastos.model.output.content/v1",
                resource_id: longResourceId,
              },
            },
          ],
        },
      },
    ],
  });
  app.setSessionMode("studio");
  app.setDraft("Reject overlong result.");
  await app.sendDraft();
  await flushAsync();
  await flushTimers();
  assert.equal(app.snapshot().statusMessage, "Model provider unavailable.");
  assert.equal(app.snapshot().studioResult, null);
}

{
  const runId =
    "run:sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
  const { app, flushAsync, flushTimers } = await buildApp({
    offers: [studioOffer("offer:vision-object", "Vision", "image.generate")],
    createResponse: {
      status: "ok",
      data: {
        schema: "elastos.model.run/v1",
        run_id: runId,
        offer_id: "offer:vision-object",
        operation: "image.generate",
        status: "running",
        sequence_cursor: 0,
      },
    },
    eventPages: [
      {
        status: "ok",
        data: {
          schema: "elastos.model.run-events/v1",
          run_id: runId,
          next_cursor: 1,
          has_more: false,
          events: [
            {
              schema: "elastos.model.run-event/v1",
              sequence: 1,
              kind: "output",
              terminal: true,
              data: {
                schema: "elastos.model.output.object/v1",
                resource_id: "object:studio-result-1",
              },
            },
          ],
        },
      },
    ],
  });
  app.setSessionMode("studio");
  app.setDraft("Reject object output.");
  await app.sendDraft();
  await flushAsync();
  await flushTimers();
  assert.equal(app.snapshot().statusMessage, "Model provider unavailable.");
  assert.equal(app.snapshot().studioResult, null);
}

{
  const runId =
    "run:sha256:bcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbc";
  const { app, fetch } = await buildApp({
    offers: [studioOffer("offer:vision-4", "Vision", "image.generate")],
    createResponse: {
      status: "ok",
      data: {
        schema: "elastos.model.run/v1",
        run_id: runId,
        offer_id: "offer:vision-4",
        operation: "image.generate",
        status: "running",
        sequence_cursor: 0,
      },
    },
    cancelResponse: {
      status: "ok",
      data: {
        schema: "elastos.model.run/v1",
        run_id: runId,
        offer_id: "offer:vision-4",
        operation: "image.generate",
        status: "cancelled",
        sequence_cursor: 1,
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
  });
  app.setSessionMode("studio");
  app.setDraft("Cancel the studio run.");
  await app.sendDraft();
  assert.equal(await app.stopRun(), true);
  assert.equal(
    fetch.fetchCalls.filter(([url]) => url === "/api/provider/model/runs_cancel").length,
    1,
  );
  assert.equal(app.snapshot().statusMessage, "Model run cancelled.");
}

{
  const runId =
    "run:sha256:cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";
  const { app } = await buildApp({
    offers: [
      offer("offer:text-1", "Fast text"),
      studioOffer("offer:vision-lock", "Vision", "image.generate"),
    ],
    createResponse: {
      status: "ok",
      data: {
        schema: "elastos.model.run/v1",
        run_id: runId,
        offer_id: "offer:vision-lock",
        operation: "image.generate",
        status: "running",
        sequence_cursor: 0,
      },
    },
  });
  app.setSessionMode("studio");
  app.setDraft("Keep the surface locked.");
  assert.equal(await app.sendDraft(), true);
  assert.equal(app.snapshot().modeSwitchDisabled, true);
  assert.equal(app.setSessionMode("chat"), false);
  assert.equal(app.snapshot().currentMode, "studio");
}

{
  const runId =
    "run:sha256:edededededededededededededededededededededededededededededededed";
  const { app, flushAsync, flushTimers } = await buildApp({
    offers: [
      offer("offer:text-1", "Fast text"),
      studioOffer("offer:vision-sticky", "Vision", "image.generate"),
    ],
    createResponse: {
      status: "ok",
      data: {
        schema: "elastos.model.run/v1",
        run_id: runId,
        offer_id: "offer:vision-sticky",
        operation: "image.generate",
        status: "running",
        sequence_cursor: 0,
      },
    },
    eventPages: [
      {
        status: "ok",
        data: {
          schema: "elastos.model.run-events/v1",
          run_id: runId,
          next_cursor: 1,
          has_more: false,
          events: [
            {
              schema: "elastos.model.run-event/v1",
              sequence: 1,
              kind: "output",
              terminal: true,
              data: {
                schema: "elastos.model.output.content/v1",
                resource_id: "studio-result-1",
              },
            },
          ],
        },
      },
    ],
  });
  app.setSessionMode("studio");
  app.setDraft("Keep the result visible.");
  await app.sendDraft();
  await flushAsync();
  await flushTimers();
  assert.equal(app.snapshot().studioResult?.resourceId, "studio-result-1");
  assert.equal(app.setSessionMode("chat"), true);
  assert.equal(app.snapshot().studioResult?.resourceId, "studio-result-1");
  assert.equal(app.setSessionMode("studio"), true);
  assert.equal(app.snapshot().studioResult?.resourceId, "studio-result-1");
}

{
  const runId =
    "run:sha256:efefefefefefefefefefefefefefefefefefefefefefefefefefefefefefefef";
  const { app, flushAsync, flushTimers } = await buildApp({
    offers: [studioOffer("offer:vision-prefix", "Vision", "image.generate")],
    createResponse: {
      status: "ok",
      data: {
        schema: "elastos.model.run/v1",
        run_id: runId,
        offer_id: "offer:vision-prefix",
        operation: "image.generate",
        status: "running",
        sequence_cursor: 0,
      },
    },
    eventPages: [
      {
        status: "ok",
        data: {
          schema: "elastos.model.run-events/v1",
          run_id: runId,
          next_cursor: 1,
          has_more: false,
          events: [
            {
              schema: "elastos.model.run-event/v1",
              sequence: 1,
              kind: "output",
              terminal: true,
              data: {
                schema: "elastos.model.output.content/v1",
                resource_id: "",
              },
            },
          ],
        },
      },
    ],
  });
  app.setSessionMode("studio");
  app.setDraft("Reject an empty resource identifier.");
  await app.sendDraft();
  await flushAsync();
  await flushTimers();
  assert.equal(app.snapshot().statusMessage, "Model provider unavailable.");
  assert.equal(app.snapshot().studioResult, null);
}

{
  const runId =
    "run:sha256:f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0";
  const { app, flushAsync, flushTimers } = await buildApp({
    offers: [studioOffer("offer:vision-extra", "Vision", "image.generate")],
    createResponse: {
      status: "ok",
      data: {
        schema: "elastos.model.run/v1",
        run_id: runId,
        offer_id: "offer:vision-extra",
        operation: "image.generate",
        status: "running",
        sequence_cursor: 0,
      },
    },
    eventPages: [
      {
        status: "ok",
        data: {
          schema: "elastos.model.run-events/v1",
          run_id: runId,
          next_cursor: 1,
          has_more: false,
          events: [
            {
              schema: "elastos.model.run-event/v1",
              sequence: 1,
              kind: "output",
              terminal: true,
              data: {
                schema: "elastos.model.output.content/v1",
                resource_id: "studio-result-2",
                extra: "nope",
              },
            },
          ],
        },
      },
    ],
  });
  app.setSessionMode("studio");
  app.setDraft("Reject extra fields.");
  await app.sendDraft();
  await flushAsync();
  await flushTimers();
  assert.equal(app.snapshot().statusMessage, "Model provider unavailable.");
  assert.equal(app.snapshot().studioResult, null);
}

assert(!/\blocalStorage\b|\bsessionStorage\b|\bindexedDB\b/.test(originalSource));
assert(!/navigator\.clipboard|execCommand/.test(originalSource));
assert(originalSource.includes('"/apps/home/home-clipboard-client.js?v=home-20260726a"'));
assert(originalSource.includes('"./vendor/katex/katex.mjs"'));
assert(originalIndexSource.includes('./vendor/katex/katex.min.css'));
assert(originalSource.includes("copyNode.hidden = view.copyHidden;"));
assert(originalSource.includes("chatNode.disabled = view.modeSwitchDisabled;"));
assert(originalSource.includes("buildNode.disabled = view.modeSwitchDisabled;"));
assert(originalSource.includes("studioNode.disabled = view.modeSwitchDisabled;"));
assert(!/MODEL_OBJECT_OUTPUT_SCHEMA|elastos:\/\/object\//.test(originalSource));
assert(!/home:clipboard-request|home:clipboard-ready|home:clipboard-result|home:clipboard-cancel/.test(originalSource));
assert(!/target="_blank"|window\.open\(/.test(originalSource));
assert(
  !/(["'](?:runtime_binding|principal_id|session_id|grant_id|backend_url|api_key|bearer)["']\s*:|\b(?:runtime_binding|principal_id|session_id|grant_id|backend_url|api_key|bearer)\s*:)/i.test(originalSource),
);
assert(!/typed text runs only|Build mode uses the same typed text runs/i.test(originalSource));
assert(!/run:sha256:|offer:text-|workspace revision/i.test(originalIndexSource));
