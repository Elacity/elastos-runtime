#!/usr/bin/env node
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { once } from "node:events";
import { readFile } from "node:fs/promises";
import { extname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium, brave } from "./system-uiux-fixture.mjs";

// Studio is a mode control only while the Runtime advertises an image or video
// offer. The real Assistant capsule runs against fixture Runtime responses to
// prove that a late offers read never moves the person away from the chat they
// are using.
const root = resolve(fileURLToPath(new URL("../", import.meta.url)));
const schema = "elastos.assistant.workspace/v2";
const workspacePath = "/api/apps/assistant/workspace-v2";
const textOffer = { id: "offer:text-fixture", title: "Text fixture", operation: "text.generate",
  input_modalities: ["text/plain"], output_modalities: ["text/plain"] };
const imageOffer = { id: "offer:image-fixture", title: "Image fixture", operation: "image.generate",
  input_modalities: ["application/json"], output_modalities: ["application/json"] };
const offersFor = { text: [textOffer], image: [textOffer, imageOffer] };
const modeNotice = "Finish or stop the current run before changing mode.";

const session = (id, title, mode, extra = {}) => ({ id, title, mode, messages: [], composerDraft: { text: "", parts: [] }, ...extra });
const workspace = (sessions, activeSessionId, sessionMode) => ({
  v: 1, activeSessionId, sessionMode, liveOfferId: "", selectedModelCid: null,
  composerDraft: { text: "", parts: [] }, sessions, projects: [], usageTurns: [],
});
const studioRoom = () => session("studio-room", "Studio room", "studio", { studio: { studioDraft: "A lighthouse at dusk" } });
const plainChat = () => session("plain-chat", "Plain chat", "chat");

// Per-scenario Runtime state: `offers` is text | image | fail | hold (answer later
// through release()); `runs` maps a run id to open (no events yet) or complete.
const fixture = { stored: null, offers: "text", held: [], runs: {}, calls: [] };
const errors = [], pageErrors = [];
function reset({ document, offers, runs = {} }) {
  fixture.stored = { schema, revision: 1, document: structuredClone(document) };
  fixture.offers = offers; fixture.held = []; fixture.runs = runs; fixture.calls = [];
}
function release(offers) {
  fixture.offers = offers;
  for (const respond of fixture.held.splice(0)) respond();
}
const server = createServer(async (req, res) => {
  const url = new URL(req.url, "http://fixture");
  const headers = {
    "access-control-allow-origin": "null",
    "access-control-allow-headers": "content-type,x-elastos-home-token",
    "access-control-allow-methods": "GET,POST,PUT,OPTIONS",
    "cache-control": "no-store",
  };
  const json = (value, status = 200) => {
    res.writeHead(status, { ...headers, "content-type": "application/json" });
    res.end(JSON.stringify(value));
  };
  try {
    if (req.method === "OPTIONS") { res.writeHead(204, headers); res.end(); return; }
    if (url.pathname === "/") {
      const origin = `http://127.0.0.1:${server.address().port}`;
      res.writeHead(200, { "content-type": "text/html" });
      res.end(`<!doctype html><style>html,body,iframe{width:100%;height:100%;margin:0;border:0}</style>
        <iframe title="Canonical Assistant" sandbox="allow-scripts allow-forms allow-modals" src="/apps/assistant/?home_origin=${encodeURIComponent(origin)}#home_token=fixture"></iframe>
        <script>addEventListener('message',event=>{if(event.data?.type==='home-agent:ready'){
          event.source.postMessage({type:'home-agent:open'},'*');
          event.source.postMessage({type:'home-agent:shelf-handover',on:true},'*');}});</script>`);
      return;
    }
    if (url.pathname.startsWith("/api/")) {
      assert.equal(req.headers["x-elastos-home-token"], "fixture");
      const chunks = []; for await (const chunk of req) chunks.push(chunk);
      const body = Buffer.concat(chunks).toString("utf8");
      const input = body ? JSON.parse(body) : null;
      fixture.calls.push({ method: req.method, path: url.pathname, input });
      if (url.pathname === workspacePath) {
        if (req.method === "GET") { json(fixture.stored); return; }
        assert.equal(req.method, "PUT");
        if (input.if_revision !== fixture.stored.revision) { json({ error: "workspace revision conflict" }, 409); return; }
        fixture.stored = { schema, revision: fixture.stored.revision + 1, document: structuredClone(input.document) };
        json(fixture.stored); return;
      }
      if (url.pathname === "/api/provider/model/offers_list") {
        const answer = () => fixture.offers === "fail"
          ? json({ status: "error", code: "provider_unavailable", message: "Model provider unavailable." }, 503)
          : json({ offers: offersFor[fixture.offers] });
        if (fixture.offers === "hold") fixture.held.push(answer); else answer();
        return;
      }
      if (url.pathname === "/api/capsules/catalog") {
        json({ schema: "elastos.capsules.catalog/v1", model_catalog_state: "verified", capsules: [] }); return;
      }
      if (url.pathname === "/api/provider/model/runs_get") { json({ run_id: input.run_id }); return; }
      if (url.pathname === "/api/provider/model/runs_events") {
        const after = Number(input.after_sequence) || 0;
        assert.ok(fixture.runs[input.run_id], `unknown run ${input.run_id}`);
        if (fixture.runs[input.run_id] === "open") { json({ events: [], next_cursor: after, has_more: false }); return; }
        json({ events: after >= 1 ? [] : [{ sequence: 1, kind: "completed", data: { output_retained: false } }], next_cursor: Math.max(after, 1), has_more: false });
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
    const bytes = await readFile(path);
    res.writeHead(200, { ...headers, "content-type": ({ ".html": "text/html", ".js": "text/javascript",
      ".mjs": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".woff2": "font/woff2" })[extname(path)] || "application/octet-stream" });
    res.end(bytes);
  } catch (error) { errors.push(String(error)); json({ error: "fixture failure" }, 500); }
});

async function until(predicate, label) {
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline) {
    if (await predicate()) return;
    await new Promise(resolve => setTimeout(resolve, 25));
  }
  throw new Error(`Timed out: ${label}`);
}
const settle = () => new Promise(resolve => setTimeout(resolve, 600));
const called = path => fixture.calls.some(call => call.path.endsWith(path));

let browser, page, stage = "launch";
try {
  server.listen(0, "127.0.0.1"); await once(server, "listening");
  browser = await chromium.launch({ executablePath: brave, headless: true });
  const open = async () => {
    if (page) await page.close();
    page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
    page.on("pageerror", error => pageErrors.push(`${stage}: ${error}`));
    await page.goto(`http://127.0.0.1:${server.address().port}/`);
    const frame = page.frameLocator("iframe");
    await frame.locator("#agent-composer-input").waitFor({ state: "visible" });
    return {
      frame,
      composer: frame.locator("#agent-composer-input"),
      studioRow: frame.locator('button[data-assistant-mode="studio"]'),
      panel: frame.locator("#assistant-studio"),
      notice: frame.locator("#assistant-workspace-notice"),
      mode: () => frame.locator("body").getAttribute("data-assistant-mode"),
      activeSession: () => frame.locator(".agent-harness-session.is-active").getAttribute("data-session-id"),
      chooseSession: title => frame.locator(".agent-harness-session-btn").filter({ hasText: title }).click(),
    };
  };
  const expectChatKept = async (ui, sessionId) => {
    await settle();
    assert.equal(await ui.mode(), "chat", `${stage}: chat stays`);
    assert.equal(await ui.panel.isVisible(), false, `${stage}: Studio panel stays closed`);
    assert.equal(await ui.composer.isVisible(), true, `${stage}: chat composer stays visible`);
    assert.equal(await ui.activeSession(), sessionId, `${stage}: selected session stays`);
  };

  stage = "saved Studio reopens once its offer arrives";
  reset({ document: workspace([studioRoom(), plainChat()], "studio-room", "studio"), offers: "hold" });
  let ui = await open();
  await until(() => fixture.held.length > 0, "boot offers read held");
  assert.equal(await ui.mode(), "chat", "no offer confirmed yet: chat opens");
  assert.equal(await ui.studioRow.isVisible(), false, "Studio row hidden until an offer is advertised");
  assert.equal(await ui.panel.isVisible(), false);
  release("image");
  await ui.studioRow.waitFor({ state: "visible" });
  await until(() => ui.mode().then(mode => mode === "studio"), "saved Studio reopened");
  assert.equal(await ui.panel.isVisible(), true);
  assert.equal(await ui.studioRow.getAttribute("aria-pressed"), "true");
  assert.equal(await ui.frame.locator("#studio-draft").inputValue(), "A lighthouse at dusk", "the saved Studio draft is the one reopened");
  await ui.studioRow.click();
  assert.equal(await ui.mode(), "chat", "the Studio row toggles back to Chat");
  assert.equal(await ui.studioRow.isVisible(), true, "Studio stays offered after leaving it");

  stage = "chat selected before the offers response keeps that chat";
  reset({ document: workspace([studioRoom(), plainChat()], "studio-room", "studio"), offers: "hold" });
  ui = await open();
  await until(() => fixture.held.length > 0, "boot offers read held");
  await ui.chooseSession("Plain chat");
  await until(() => ui.activeSession().then(id => id === "plain-chat"), "Plain chat selected");
  release("image");
  await ui.studioRow.waitFor({ state: "visible" });
  await expectChatKept(ui, "plain-chat");

  stage = "typing before the offers response keeps the composer";
  reset({ document: workspace([studioRoom(), plainChat()], "studio-room", "studio"), offers: "hold" });
  ui = await open();
  await until(() => fixture.held.length > 0, "boot offers read held");
  await ui.composer.fill("Started writing in chat");
  release("image");
  await ui.studioRow.waitFor({ state: "visible" });
  await expectChatKept(ui, "studio-room");
  assert.equal(await ui.composer.inputValue(), "Started writing in chat");

  stage = "a turn still running when the offers arrive keeps chat";
  const streaming = studioRoom();
  streaming.messages = [{ id: "message-0", role: "user", text: "Keep going" }];
  streaming.lastTurn = { turnId: "turn-open", providerRunId: "run-open", createRequestId: "request-open", state: "streaming", startedAt: Date.now() - 1000 };
  reset({ document: workspace([streaming, plainChat()], "studio-room", "studio"), offers: "hold", runs: { "run-open": "open" } });
  ui = await open();
  await until(() => fixture.held.length > 0 && called("/runs_get") && called("/runs_events"), "turn resumed and offers read held");
  release("image");
  await ui.studioRow.waitFor({ state: "visible" });
  await expectChatKept(ui, "studio-room");
  await ui.studioRow.click();
  assert.equal(await ui.notice.textContent(), modeNotice, "mode change waits for the running turn");
  assert.equal(await ui.mode(), "chat");
  assert.equal(pageErrors.length, 0, pageErrors.join("\n"));

  assert.deepEqual(errors, []);
  assert.deepEqual(pageErrors, []);
  console.log("PASS Studio mode control: offered reopen bound to its session and intent");
} catch (error) {
  console.error(`Studio mode smoke failed at ${stage}`);
  console.error(JSON.stringify({ errors, pageErrors, calls: fixture.calls.map(call => call.path) }, null, 1));
  throw error;
} finally {
  await browser?.close();
  server.close();
}
