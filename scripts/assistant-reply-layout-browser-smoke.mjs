#!/usr/bin/env node
// Replies stay readable at the edges of the Assistant room. The real
// Assistant capsule runs opaque-sandboxed inside an opaque Home GUI frame,
// as installed. Numbered lists of ten or more items keep every marker whole
// ("10.", "20."), in a saved reply and in one that streamed in, and while a
// turn runs its status line ("Working out the best approach…") and then the
// newest reply sit fully above the composer, not behind it.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { once } from "node:events";
import { readFile } from "node:fs/promises";
import { extname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "./system-uiux-fixture.mjs";

const root = resolve(fileURLToPath(new URL("../", import.meta.url)));
const schema = "elastos.assistant.workspace/v2";
const homeToken = "assistant-layout-fixture";
const textOffer = { id: "offer:text-fixture", title: "Text fixture", operation: "text.generate",
  input_modalities: ["text/plain"], output_modalities: ["text/plain"] };
const numbered = (count, label) => Array.from({ length: count }, (_, i) => `${i + 1}. ${label} ${i + 1}`).join("\n");
const filler = "A sentence that fills the room so the transcript scrolls. ".repeat(12);
const savedList = `Here are twelve:\n\n${numbered(12, "Saved step")}`;
const streamedList = `Here are twenty-five:\n\n${numbered(25, "Streamed step")}`;

const history = [];
for (let i = 0; i < 4; i += 1) {
  history.push({ role: "user", text: `Question ${i + 1}`, modelText: `Question ${i + 1}` });
  history.push({ role: "agent", text: i === 3 ? savedList : filler });
}
history.push({ role: "user", text: "List twenty-five steps", modelText: "List twenty-five steps" });
// A turn still running when the room opens: the Assistant resumes it, shows
// its status while nothing has arrived, then streams the reply.
const busyRoom = {
  v: 1, activeSessionId: "layout-room", sessionMode: "chat", liveOfferId: textOffer.id, selectedModelCid: null,
  composerDraft: { text: "", parts: [] }, projects: [], usageTurns: [],
  sessions: [{ id: "layout-room", title: "Layout room", mode: "chat", composerDraft: { text: "", parts: [] },
    messages: history,
    lastTurn: { turnId: "turn-layout", providerRunId: "run-layout", createRequestId: "request-layout",
      state: "streaming", startedAt: Date.now() - 1000 } }],
};

const fixture = { stored: { schema, revision: 1, document: busyRoom }, events: [] };
const errors = [];
const server = createServer(async (req, res) => {
  const url = new URL(req.url, "http://fixture");
  const origin = `http://127.0.0.1:${server.address().port}`;
  const headers = {
    "access-control-allow-origin": "null",
    "access-control-allow-headers": "content-type,x-elastos-home-token",
    "access-control-allow-methods": "GET,POST,PUT,OPTIONS",
    "cache-control": "no-store",
  };
  const send = (status, type, body) => { res.writeHead(status, { ...headers, "content-type": type }); res.end(body); };
  const json = (value, status = 200) => send(status, "application/json", JSON.stringify(value));
  try {
    if (req.method === "OPTIONS") { res.writeHead(204, headers); res.end(); return; }
    if (url.pathname === "/") {
      send(200, "text/html", `<!doctype html><style>html,body,iframe{width:100%;height:100%;margin:0;border:0}</style>
        <iframe title="Home GUI" sandbox="allow-scripts" src="/gui?home_origin=${encodeURIComponent(origin)}"></iframe>`);
      return;
    }
    if (url.pathname === "/gui") {
      // Home GUI: hosts the Assistant frame and raises its room once ready.
      const homeOrigin = url.searchParams.get("home_origin");
      send(200, "text/html", `<!doctype html><style>html,body,iframe{width:100%;height:100%;margin:0;border:0}</style>
        <iframe title="Assistant" sandbox="allow-downloads allow-forms allow-modals allow-pointer-lock allow-scripts" allow="autoplay; fullscreen"
          src="/apps/assistant/?home_origin=${encodeURIComponent(homeOrigin)}#home_token=${homeToken}"></iframe>
        <script>addEventListener('message',event=>{if(event.data?.type==='home-agent:ready'){
          event.source.postMessage({type:'home-agent:open'},'*');
          event.source.postMessage({type:'home-agent:shelf-handover',on:true},'*');}});</script>`);
      return;
    }
    if (url.pathname.startsWith("/api/")) {
      assert.equal(req.headers["x-elastos-home-token"], homeToken);
      const chunks = []; for await (const chunk of req) chunks.push(chunk);
      const body = Buffer.concat(chunks).toString("utf8");
      const input = body ? JSON.parse(body) : null;
      if (url.pathname === "/api/apps/assistant/workspace-v2") {
        if (req.method === "GET") { json(fixture.stored); return; }
        assert.equal(req.method, "PUT");
        if (input.if_revision !== fixture.stored.revision) { json({ error: "workspace revision conflict" }, 409); return; }
        fixture.stored = { schema, revision: fixture.stored.revision + 1, document: input.document };
        json(fixture.stored); return;
      }
      if (url.pathname === "/api/provider/model/offers_list") { json({ offers: [textOffer] }); return; }
      if (url.pathname === "/api/capsules/catalog") {
        json({ schema: "elastos.capsules.catalog/v1", model_catalog_state: "verified", capsules: [] }); return;
      }
      if (url.pathname === "/api/provider/model/runs_get") { json({ run_id: input.run_id }); return; }
      if (url.pathname === "/api/provider/model/runs_events") {
        assert.equal(input.run_id, "run-layout");
        const after = Number(input.after_sequence) || 0;
        const events = fixture.events.filter(event => event.sequence > after);
        json({ events, next_cursor: Math.max(after, ...events.map(event => event.sequence)), has_more: false });
        return;
      }
      throw new Error(`Unexpected API operation: ${req.method} ${url.pathname}`);
    }
    if (url.pathname === "/favicon.ico") { res.writeHead(204); res.end(); return; }
    const match = url.pathname.match(/^\/apps\/(assistant|home)\/(.*)$/);
    assert.ok(match, `Unexpected asset: ${url.pathname}`);
    const base = resolve(root, "capsules", match[1], "browser");
    const path = resolve(base, match[2] || "index.html");
    assert.ok(path.startsWith(`${base}/`));
    send(200, ({ ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript", ".css": "text/css",
      ".svg": "image/svg+xml", ".woff2": "font/woff2" })[extname(path)] || "application/octet-stream", await readFile(path));
  } catch (error) { errors.push(String(error)); json({ error: "fixture failure" }, 500); }
});

/* Every marker of every numbered reply list, measured against the nearest
   box that clips it: an outside marker ends where its item's content starts
   and is as wide as its text ("25. "). */
function markerReport(selector) {
  const clipLeft = (el) => {
    let left = -Infinity;
    for (let node = el.parentElement; node; node = node.parentElement) {
      const style = getComputedStyle(node);
      const contain = style.contain;
      if (style.overflowX !== "visible" || /paint|content|strict/.test(contain)) {
        left = Math.max(left, node.getBoundingClientRect().left + parseFloat(style.borderLeftWidth));
      }
    }
    return left;
  };
  const probe = document.createElement("span");
  probe.style.cssText = "position:absolute;visibility:hidden;white-space:pre";
  document.body.append(probe);
  const lists = [...document.querySelectorAll(selector)];
  const report = lists.map((list) => {
    const items = [...list.children].filter((item) => item.tagName === "LI");
    let worst = Infinity;
    items.forEach((item, index) => {
      const style = getComputedStyle(item, "::marker");
      probe.style.font = getComputedStyle(item).font;
      probe.style.fontVariantNumeric = style.fontVariantNumeric;
      probe.textContent = `${(Number(list.getAttribute("start")) || 1) + index}. `;
      const markerLeft = item.getBoundingClientRect().left - probe.getBoundingClientRect().width;
      worst = Math.min(worst, markerLeft - clipLeft(list));
    });
    return { items: items.length, worstRoomPx: Number(worst.toFixed(1)) };
  });
  probe.remove();
  return report;
}

/* How far the newest block in the transcript sits above the composer's top
   edge (negative: it is behind the composer). */
function newestClearance(selector) {
  const scroller = document.querySelector("#agent-harness-stream");
  const composer = document.querySelector(".taskbar.is-agent-face");
  const column = document.querySelector("#agent-harness-stream-column");
  const blocks = [...column.children].filter((el) => !el.hidden && el.getBoundingClientRect().height > 0);
  const newest = blocks.at(-1);
  return {
    newest: newest.matches(selector),
    text: newest.textContent.trim().slice(-40),
    abovePx: Math.round(composer.getBoundingClientRect().top - newest.getBoundingClientRect().bottom),
    atEnd: scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight <= 1,
  };
}

let browser, page, stage = "launch";
const pageErrors = [];
try {
  server.listen(0, "127.0.0.1"); await once(server, "listening");
  browser = await chromium.launch({ headless: true, ...(process.env.BRAVE_BIN ? { executablePath: process.env.BRAVE_BIN } : {}) });
  page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  page.on("pageerror", error => pageErrors.push(`${stage}: ${error}`));
  await page.goto(`http://127.0.0.1:${server.address().port}/`);
  const room = page.frameLocator("iframe").frameLocator("iframe");
  await room.locator(".agent-msg").nth(history.length - 1).waitFor({ state: "attached" });
  const frame = page.frames().find(candidate => candidate.url().includes("/apps/assistant/"));

  const input = room.locator("#agent-composer-input");
  // Polls until the newest block is at the end and fully above the composer,
  // for up to 5 s; then asserts on the last measurement.
  const expectNewestAboveComposer = async (selector, label) => {
    const deadline = Date.now() + 5000;
    let seen = await frame.evaluate(newestClearance, selector);
    while (!(seen.newest && seen.atEnd && seen.abovePx >= 0) && Date.now() < deadline) {
      await page.waitForTimeout(50);
      seen = await frame.evaluate(newestClearance, selector);
    }
    assert.ok(seen.newest && seen.atEnd, `${label} is the newest block, at the end: ${JSON.stringify(seen)}`);
    assert.ok(seen.abovePx >= 0, `${label} sits fully above the composer: ${JSON.stringify(seen)}`);
    return seen.abovePx;
  };
  // A long follow-up being typed grows the composer upwards; wait for the
  // growth so the next measurement cannot pass on the old composer.
  const composerHeight = () => frame.evaluate(() => document.querySelector(".taskbar.is-agent-face").getBoundingClientRect().height);
  const typeLongFollowUp = async () => {
    const before = await composerHeight();
    await input.fill(Array.from({ length: 6 }, (_, i) => `Follow-up line ${i + 1}`).join("\n"));
    await frame.waitForFunction((height) =>
      document.querySelector(".taskbar.is-agent-face").getBoundingClientRect().height > height, before, { timeout: 5000 });
  };
  const seen = {};

  stage = "status line of a running turn";
  await room.locator(".agent-progress:not([hidden])").filter({ hasText: "Working out the best approach" })
    .waitFor({ timeout: 10000 });
  seen.status = await expectNewestAboveComposer(".agent-progress", "the status line");
  await typeLongFollowUp();
  seen.statusTyping = await expectNewestAboveComposer(".agent-progress", "the status line, while a follow-up is typed");
  await input.fill("");

  stage = "streamed reply";
  fixture.events = [
    { sequence: 1, kind: "text_delta", data: { text: streamedList } },
    { sequence: 2, kind: "completed", data: {} },
  ];
  await room.locator(".agent-msg-agent.is-streaming").waitFor({ state: "detached", timeout: 15000 });
  await room.locator(".agent-msg-agent").last().filter({ hasText: "Streamed step 25" }).waitFor({ timeout: 5000 });
  seen.reply = await expectNewestAboveComposer(".agent-msg-agent", "the newest reply");
  await typeLongFollowUp();
  seen.replyTyping = await expectNewestAboveComposer(".agent-msg-agent", "the newest reply, while a follow-up is typed");

  stage = "numbered list markers";
  const markers = await frame.evaluate(markerReport, ".agent-msg-agent ol.agent-md-list");
  assert.deepEqual(markers.map(list => list.items), [12, 25], `saved and streamed lists render: ${JSON.stringify(markers)}`);
  for (const list of markers) {
    assert.ok(list.worstRoomPx >= 0, `every marker of a ${list.items}-item list is whole: ${JSON.stringify(markers)}`);
  }
  assert.deepEqual(errors, []);
  assert.deepEqual(pageErrors, []);
  console.log(`PASS Assistant reply layout: px above the composer ${JSON.stringify(seen)}; list markers whole (${markers.map(list => `${list.items} items, ${list.worstRoomPx}px spare`).join("; ")})`);
} catch (error) {
  console.error(`Assistant reply layout smoke failed at ${stage}`);
  console.error(JSON.stringify({ errors, pageErrors }, null, 1));
  throw error;
} finally {
  await browser?.close();
  server.close();
}
