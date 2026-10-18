import assert from "node:assert/strict";
import { createServer } from "node:http";
import { once } from "node:events";
import { readFile } from "node:fs/promises";
import { extname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { chromium } from "./system-uiux-fixture.mjs";

// The real Assistant capsule in Chromium; only Runtime responses are fixtures.
const root = resolve(fileURLToPath(new URL("../", import.meta.url)));
const schema = "elastos.assistant.workspace/v2";
const offerId = "offer:edit-fixture";
const types = { ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml" };
const until = async (predicate, label) => {
  for (const deadline = Date.now() + 5000; !predicate();) {
    assert.ok(Date.now() < deadline, `Timed out: ${label}`);
    await new Promise(done => setTimeout(done, 25));
  }
};

/** Run N answers "Reply N"; `firstReply` holds run 1's events until it resolves.
    `eventsPage(after_sequence)`, when given, serves every run's events pages instead. */
async function openAssistant(t, { firstReply = null, eventsPage = null } = {}) {
  let stored = { schema, revision: 0, document: { v: 1, liveOfferId: offerId, sessions: [] } };
  const creates = [], errors = [], cancels = [];
  const server = createServer(async (req, res) => {
    const url = new URL(req.url, "http://fixture");
    const headers = { "access-control-allow-origin": "null", "access-control-allow-headers": "content-type,x-elastos-home-token",
      "access-control-allow-methods": "GET,POST,PUT,OPTIONS", "cache-control": "no-store" };
    const json = value => { res.writeHead(200, { ...headers, "content-type": "application/json" }); res.end(JSON.stringify(value)); };
    try {
      if (req.method === "OPTIONS") { res.writeHead(204, headers); res.end(); return; }
      if (url.pathname === "/") {
        const origin = `http://127.0.0.1:${server.address().port}`;
        res.writeHead(200, { "content-type": "text/html" });
        res.end(`<!doctype html><style>html,body,iframe{width:100%;height:100%;margin:0;border:0}</style>
          <iframe sandbox="allow-scripts allow-forms allow-modals" src="/apps/assistant/?home_origin=${encodeURIComponent(origin)}#home_token=fixture"></iframe>
          <script>addEventListener('message',event=>{if(event.data?.type==='home-agent:ready'){
            event.source.postMessage({type:'home-agent:open'},'*');
            event.source.postMessage({type:'home-agent:shelf-handover',on:true},'*');}});</script>`);
        return;
      }
      if (url.pathname.startsWith("/api/")) {
        const chunks = []; for await (const chunk of req) chunks.push(chunk);
        const body = chunks.length ? JSON.parse(Buffer.concat(chunks).toString("utf8")) : null;
        if (url.pathname === "/api/apps/assistant/workspace-v2") {
          if (req.method === "PUT") {
            assert.equal(body.if_revision, stored.revision);
            stored = { schema, revision: stored.revision + 1, document: body.document };
          }
          json(stored); return;
        }
        if (url.pathname === "/api/provider/model/offers_list") {
          json({ offers: [{ id: offerId, title: "Edit fixture", operation: "text.generate",
            context: { context_window_tokens: 8192, max_output_tokens: 1024 },
            input_modalities: ["text/plain"], output_modalities: ["text/plain"] }] }); return;
        }
        if (url.pathname === "/api/capsules/catalog") { json({ schema: "elastos.capsules.catalog/v1", capsules: [] }); return; }
        if (url.pathname === "/api/provider/model/runs_create") {
          creates.push(body);
          json({ status: "ok", data: { schema: "elastos.model.run/v1", run_id: `run:sha256:${String(creates.length).repeat(64)}`,
            offer_id: offerId, operation: "text.generate", status: "running", sequence_cursor: 0 } }); return;
        }
        if (url.pathname === "/api/provider/model/runs_cancel") {
          cancels.push(body);
          json({ status: "ok", data: { schema: "elastos.model.run/v1", run_id: body.run_id, status: "cancelling" } }); return;
        }
        if (url.pathname === "/api/provider/model/runs_events") {
          if (eventsPage) {
            json({ status: "ok", data: { schema: "elastos.model.run-events/v1", run_id: body.run_id,
              ...await eventsPage(body.after_sequence) } }); return;
          }
          const run = body.run_id.at(-1);
          if (run === "1") await firstReply;
          json({ status: "ok", data: { schema: "elastos.model.run-events/v1", run_id: body.run_id, next_cursor: 2, has_more: false,
            events: [{ sequence: 1, kind: "text_delta", data: { text: `Reply ${run}` } },
              { sequence: 2, kind: "completed", terminal: true, data: {} }] } }); return;
        }
        throw new Error(`Unexpected API operation: ${req.method} ${url.pathname}`);
      }
      const match = url.pathname.match(/^\/apps\/(assistant|home)\/(.*)$/);
      assert.ok(match, `Unexpected asset: ${url.pathname}`);
      const base = resolve(root, "capsules", match[1], "browser");
      const path = resolve(base, match[2] || "index.html");
      assert.ok(path.startsWith(`${base}/`));
      const bytes = await readFile(path);
      // The Runtime's app-document sandbox (browser_capsules.rs
      // shell_content_security_policy): no allow-modals, so confirm() is ignored.
      const csp = match[1] === "assistant" && path === resolve(base, "index.html")
        ? { "content-security-policy": "sandbox allow-scripts allow-forms allow-downloads" } : {};
      res.writeHead(200, { ...headers, ...csp, "content-type": types[extname(path)] || "application/octet-stream" });
      res.end(bytes);
    } catch (error) { errors.push(String(error)); res.writeHead(500); res.end(); }
  });
  server.listen(0, "127.0.0.1"); await once(server, "listening");
  const browser = await chromium.launch({ headless: true });
  t.after(async () => {
    await browser.close();
    server.closeAllConnections();
    await new Promise(done => server.close(done));
  });
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  page.on("pageerror", error => errors.push(String(error)));
  page.on("dialog", dialog => { errors.push(`browser dialog: ${dialog.message()}`); void dialog.dismiss(); });
  await page.goto(`http://127.0.0.1:${server.address().port}/`);
  const frame = page.frameLocator("iframe");
  const composer = frame.locator("#agent-composer-input");
  await composer.waitFor({ state: "visible" });
  const capsuleFrame = page.frames().find(item => item.url().includes("/apps/assistant/"));
  assert.equal(await capsuleFrame.evaluate(() => window.confirm("probe")), false,
    "as on a real Home, the Assistant document cannot open browser dialogs");
  const userRows = frame.locator(".agent-msg-user");
  const replies = frame.locator(".agent-msg-agent .agent-msg-body");
  // DOM text: rows use content-visibility:auto, so rendered text can be empty.
  const texts = async locator => (await locator.allTextContents()).map(text => text.trim());
  const saved = () => JSON.stringify(stored.document.sessions?.[0]?.messages?.map(message => [message.role, message.text]));
  return {
    page, frame, composer, userRows, replies, creates, cancels, errors,
    userTexts: () => texts(userRows.locator(".agent-msg-body")),
    replyTexts: () => texts(replies),
    send: async (question, reply) => {
      await composer.fill(question);
      await composer.press("Enter");
      if (reply) await replies.filter({ hasText: reply }).waitFor();
    },
    startEdit: async index => {
      const row = userRows.nth(index);
      await row.hover();
      await row.getByRole("button", { name: "Edit and resubmit" }).click();
      await row.locator("[data-edit-form]").waitFor({ state: "visible", timeout: 5000 });
      return row.locator("[data-edit-form]");
    },
    resubmit: async (form, text, reply) => {
      await form.locator("textarea").fill(text);
      await form.getByRole("button", { name: "Save & submit" }).click();
      await replies.filter({ hasText: reply }).waitFor();
    },
    savedTranscript: async expected => {
      await until(() => saved() === JSON.stringify(expected), "the workspace keeps the transcript");
    },
  };
}

test("Edit on a message sent in this view replaces that turn onward, then resubmits; Cancel and Delete keep indices right", async t => {
  const app = await openAssistant(t);
  await app.send("First question", "Reply 1");
  await app.send("Second question", "Reply 2");
  await app.send("Third question", "Reply 3");

  // The rows were appended as sent; nothing re-rendered the transcript yet.
  assert.equal(await (await app.startEdit(0)).locator("textarea").inputValue(), "First question");
  const second = await app.startEdit(1);
  assert.equal(await second.locator("textarea").inputValue(), "Second question");
  await second.getByRole("button", { name: "Cancel" }).click();
  assert.equal(await app.frame.locator("[data-edit-form]").count(), 0, "Cancel closes the editors");
  assert.deepEqual(await app.userTexts(), ["First question", "Second question", "Third question"]);
  assert.deepEqual(await app.replyTexts(), ["Reply 1", "Reply 2", "Reply 3"], "Cancel keeps the replies");
  assert.equal(app.creates.length, 3, "Cancel does not resubmit");

  // Editing an earlier turn drops it and every later turn and reply.
  await app.resubmit(await app.startEdit(1), "Edited question", "Reply 4");
  assert.deepEqual(await app.userTexts(), ["First question", "Edited question"]);
  assert.deepEqual(await app.replyTexts(), ["Reply 1", "Reply 4"]);
  assert.equal(app.creates.length, 4, "Save & submit sends one new run");
  let prompt = app.creates[3].input.prompt;
  assert.ok(["First question", "Reply 1", "Edited question"].every(text => prompt.includes(text)), prompt);
  assert.ok(!/Second question|Third question|Reply 2|Reply 3/.test(prompt), "the resubmitted run drops the replaced turns");
  await app.savedTranscript([["user", "First question"], ["agent", "Reply 1"], ["user", "Edited question"], ["agent", "Reply 4"]]);

  // Delete asks in the row (Escape and Cancel keep the message), then shifts
  // the later messages; Edit then acts on the message shown.
  const firstReply = app.frame.locator(".agent-msg-agent").filter({ hasText: "Reply 1" });
  const question = firstReply.getByRole("group", { name: "Delete this message?" });
  await firstReply.hover();
  await firstReply.getByRole("button", { name: "Delete" }).click();
  await question.waitFor({ state: "visible", timeout: 5000 });
  await app.page.keyboard.press("Escape");
  await question.waitFor({ state: "detached", timeout: 5000 });
  await firstReply.hover();
  await firstReply.getByRole("button", { name: "Delete" }).click();
  await question.getByRole("button", { name: "Cancel" }).click();
  assert.equal(await question.count(), 0);
  assert.deepEqual(await app.replyTexts(), ["Reply 1", "Reply 4"], "Escape and Cancel keep the reply");
  await firstReply.hover();
  await firstReply.getByRole("button", { name: "Delete" }).click();
  await question.getByRole("button", { name: "Delete" }).click();
  await app.savedTranscript([["user", "First question"], ["user", "Edited question"], ["agent", "Reply 4"]]);
  assert.deepEqual(await app.replyTexts(), ["Reply 4"]);
  const edited = await app.startEdit(1);
  assert.equal(await edited.locator("textarea").inputValue(), "Edited question");
  await app.resubmit(edited, "Final question", "Reply 5");
  assert.deepEqual(await app.userTexts(), ["First question", "Final question"]);
  assert.deepEqual(await app.replyTexts(), ["Reply 5"]);
  prompt = app.creates[4].input.prompt;
  assert.ok(["First question", "Final question"].every(text => prompt.includes(text)), prompt);
  assert.ok(!/Edited question|Reply 1|Reply 4/.test(prompt), prompt);
  await app.savedTranscript([["user", "First question"], ["user", "Final question"], ["agent", "Reply 5"]]);

  // Rename, Edit tags and Import JSON used prompt()/alert(), which this sandbox
  // ignores; they stay out of the chat menu until they work in the page.
  await app.frame.getByRole("button", { name: /^Chat actions for / }).first().click();
  const menuItems = app.frame.getByRole("menuitem");
  await menuItems.first().waitFor();
  const items = await menuItems.allTextContents();
  assert.ok(items.includes("Delete"), items.join(", "));
  for (const hidden of ["Rename", "Edit tags…", "Import JSON…"]) assert.ok(!items.includes(hidden), `${hidden} is not offered`);
  assert.deepEqual(app.errors, []);
});

test("Edit on a queued follow-up sent in this view resubmits that message", async t => {
  let release;
  const app = await openAssistant(t, { firstReply: new Promise(done => { release = done; }) });
  t.after(() => release());
  await app.send("First question");
  await until(() => app.creates.length === 1, "first run created");
  // While a reply streams the composer's Enter is Stop, so the composer cannot
  // queue; queue through the capsule's own module instance instead.
  const capsule = app.page.frames().find(frame => frame.url().includes("/apps/assistant/"));
  await capsule.evaluate(async () => (await import("/apps/assistant/agent-stream.js"))
    .enqueueFollowUp("Second question", { displayText: "Second question" }));
  await app.frame.locator(".agent-queue-item-text").filter({ hasText: "Second question" }).waitFor();
  release();
  await app.replies.filter({ hasText: "Reply 2" }).waitFor();
  assert.deepEqual(await app.userTexts(), ["First question", "Second question"]);

  const queued = await app.startEdit(1);
  assert.equal(await queued.locator("textarea").inputValue(), "Second question");
  await app.resubmit(queued, "Edited follow-up", "Reply 3");
  assert.deepEqual(await app.userTexts(), ["First question", "Edited follow-up"]);
  assert.deepEqual(await app.replyTexts(), ["Reply 1", "Reply 3"]);
  assert.ok(!/Second question|Reply 2/.test(app.creates[2].input.prompt), app.creates[2].input.prompt);
  await app.savedTranscript([["user", "First question"], ["agent", "Reply 1"], ["user", "Edited follow-up"], ["agent", "Reply 3"]]);
  assert.deepEqual(app.errors, []);
});

test("A reply that has just streamed offers Delete and Regenerate without a redraw", async t => {
  const app = await openAssistant(t);
  await app.send("First question", "Reply 1");
  const reply = text => app.frame.locator(".agent-msg-agent").filter({ hasText: text });
  // A marker survives only while the transcript is not redrawn.
  await reply("Reply 1").evaluate(row => { row.dataset.fixtureStreamed = "1"; });
  await app.send("Second question", "Reply 2");
  await reply("Reply 2").evaluate(row => { row.dataset.fixtureStreamed = "1"; });
  await until(() => app.creates.length === 2, "second run created");
  for (const text of ["Reply 1", "Reply 2"]) {
    const row = reply(text);
    assert.equal(await row.getAttribute("data-fixture-streamed"), "1", `${text} is the streamed row`);
    await row.hover();
    await row.getByRole("button", { name: "Delete" }).waitFor({ state: "attached", timeout: 5000 });
    assert.ok(await row.getByRole("button", { name: "Regenerate reply" }).isEnabled(), `${text} offers Regenerate`);
  }
  const question = reply("Reply 1").getByRole("group", { name: "Delete this message?" });
  await reply("Reply 1").getByRole("button", { name: "Delete" }).click();
  await question.getByRole("button", { name: "Delete" }).click();
  await app.savedTranscript([["user", "First question"], ["user", "Second question"], ["agent", "Reply 2"]]);
  assert.deepEqual(await app.replyTexts(), ["Reply 2"], "Delete removes the reply it was offered on");
  assert.deepEqual(app.errors, []);
});

test("A streamed reply whose provider keeps no copy still offers Delete and Regenerate", async t => {
  const app = await openAssistant(t, { eventsPage: async () => ({ next_cursor: 2, has_more: false, events: [
    { sequence: 1, kind: "text_delta", data: { text: "Shown once" } },
    { sequence: 2, kind: "completed", terminal: true, data: { output_retained: false } }] }) });
  await app.send("Question", "Shown once");
  const row = app.frame.locator(".agent-msg-agent").filter({ hasText: "Shown once" });
  await row.hover();
  await row.getByRole("button", { name: "Delete" }).waitFor({ state: "attached", timeout: 5000 });
  assert.ok(await row.getByRole("button", { name: "Regenerate reply" }).isEnabled());
  assert.equal(await row.getAttribute("data-msg-index"), "1");
  assert.deepEqual(app.errors, []);
});

test("A reply stopped before its run settles still offers Delete and Regenerate", async t => {
  let release;
  const settled = new Promise(done => { release = done; });
  const app = await openAssistant(t, { eventsPage: async after => {
    if (after === 0) return { next_cursor: 1, has_more: false,
      events: [{ sequence: 1, kind: "text_delta", data: { text: "Partial words" } }] };
    await settled;
    return { next_cursor: 2, has_more: false,
      events: [{ sequence: 2, kind: "settlement_unknown", terminal: true, data: {} }] };
  } });
  t.after(() => release());
  await app.send("Question", "Partial words");
  const capsule = app.page.frames().find(frame => frame.url().includes("/apps/assistant/"));
  await capsule.evaluate(async () => { void (await import("/apps/assistant/agent-live.js")).abortLiveChatStream(); });
  await until(() => app.cancels.length === 1, "Stop sends one cancel");
  release();
  const row = app.frame.locator(".agent-msg-agent").filter({ hasText: "Partial words" });
  await row.hover();
  await row.getByRole("button", { name: "Delete" }).waitFor({ state: "attached", timeout: 5000 });
  assert.equal(await row.getAttribute("data-msg-index"), "1");
  await app.frame.locator('.agent-msg-agent .agent-msg-action[data-regenerate]:not([disabled])').waitFor({ state: "attached", timeout: 5000 });
  assert.equal(await app.frame.locator(".agent-msg-agent").count(), 1, "one row for the saved partial");
  assert.deepEqual(app.errors, []);
});

/** A long streamed reply (paragraphs, code block); page `hold` waits on `gate`. */
function longReply({ hold = -1, gate = null } = {}) {
  const parts = [];
  for (let i = 1; i <= 8; i += 1) parts.push(`Paragraph ${i}. ${"The reply keeps going with more words. ".repeat(8)}\n\n`);
  parts.push("\n```js\n");
  for (let i = 1; i <= 12; i += 1) parts.push(`const line${i} = ${i};\n`);
  parts.push("```\n\nLast paragraph of the reply.\n");
  return async after => {
    const index = Number(after);
    if (index === hold) await gate;
    const last = index + 1 === parts.length;
    return { next_cursor: index + (last ? 2 : 1), has_more: !last, events: [
      { sequence: index + 1, kind: "text_delta", data: { text: parts[index] } },
      ...(last ? [{ sequence: index + 2, kind: "completed", terminal: true, data: {} }] : [])] };
  };
}

const scrollState = frame => frame.locator("#agent-harness-stream").evaluate(scroller => {
  const actions = [...document.querySelectorAll(".agent-msg-agent .agent-msg-actions")].at(-1);
  const jump = document.querySelector("[data-agent-jump-latest]");
  return {
    scrollTop: scroller.scrollTop,
    fromBottom: scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight,
    actionsBottom: actions?.getBoundingClientRect().bottom ?? null,
    composerTop: document.querySelector(".taskbar").getBoundingClientRect().top,
    jumpVisible: !!jump && !jump.hidden && jump.getBoundingClientRect().height > 0,
  };
});
/** Two frames: the pin after layout (and any wrong late scroll) has run. */
const settled = frame => frame.locator("#agent-harness-stream").evaluate(() =>
  new Promise(done => requestAnimationFrame(() => requestAnimationFrame(done))));
const presented = frame => frame.locator('.agent-msg-agent[data-stream-phase="presentation_done"]').waitFor({ state: "attached", timeout: 10000 });

test("A long streamed reply ends with its actions above the composer and the transcript at the bottom", async t => {
  const app = await openAssistant(t, { eventsPage: longReply() });
  await app.composer.fill("Write a long answer");
  await app.composer.press("Enter");
  await presented(app.frame);
  await settled(app.frame);
  const end = await scrollState(app.frame);
  assert.ok(end.actionsBottom !== null, "the reply has its actions row");
  assert.ok(end.actionsBottom <= end.composerTop,
    `the actions row (bottom ${end.actionsBottom}) is above the composer (top ${end.composerTop})`);
  assert.ok(end.fromBottom < 4, `the transcript is at the bottom (${end.fromBottom}px left)`);
  assert.deepEqual(app.errors, []);
});

test("Scrolling up while a reply streams keeps the reader's place and offers Jump to latest", async t => {
  let release;
  const gate = new Promise(done => { release = done; });
  const app = await openAssistant(t, { eventsPage: longReply({ hold: 6, gate }) });
  await app.composer.fill("Write a long answer");
  await app.composer.press("Enter");
  await app.frame.locator(".agent-msg-agent .agent-msg-body", { hasText: "Paragraph 6" }).waitFor();
  const box = await app.frame.locator("#agent-harness-stream").boundingBox();
  await app.page.mouse.move(box.x + box.width / 2, box.y + box.height / 3);
  await app.page.mouse.wheel(0, -400);
  // mouse.wheel returns before the scroll finishes: wait until it stops moving.
  for (let last = -1, now = (await scrollState(app.frame)).scrollTop; now !== last;) {
    await settled(app.frame);
    [last, now] = [now, (await scrollState(app.frame)).scrollTop];
  }
  const held = await scrollState(app.frame);
  release();
  await presented(app.frame);
  await settled(app.frame);
  const end = await scrollState(app.frame);
  assert.ok(Math.abs(end.scrollTop - held.scrollTop) < 2,
    `the reader's place is kept (${held.scrollTop} then ${end.scrollTop})`);
  assert.ok(end.fromBottom > 100, "the transcript did not jump to the end");
  assert.ok(end.jumpVisible, "Jump to latest is offered");
  assert.deepEqual(app.errors, []);
});
