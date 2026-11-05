import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createServer } from "node:http";
import { resolve, extname } from "node:path";
import { pathToFileURL } from "node:url";
import test from "node:test";
import { checkArtifacts } from "./check-chat-ui-artifacts.mjs";

const root = resolve(new URL("..", import.meta.url).pathname);
const read = path => readFileSync(resolve(root, path), "utf8");
assert.ok(process.env.BROWSER_OPERATOR_PLAYWRIGHT_CORE,
  "Run ./scripts/test-behaviour.sh inputs and use the pinned BROWSER_OPERATOR_PLAYWRIGHT_CORE input");
const sdkPath = resolve(process.env.BROWSER_OPERATOR_PLAYWRIGHT_CORE);
const metadata = JSON.parse(readFileSync(resolve(sdkPath, "package.json"), "utf8"));
assert.equal(metadata.name, "playwright-core");
assert.equal(metadata.version, "1.59.1");
const { chromium } = await import(pathToFileURL(resolve(sdkPath, "index.mjs")).href);
const serverSecret = "provider-secret-sk-session-start-fixture";
const boundedStartFailure = "Chat session bootstrap failed. Reopen Chat from Home.";
async function fixture(t, direct = false, { startFault = null } = {}) {
  checkArtifacts(); // Execute the committed WASM only when its source stamp matches.
  const state = { now: 0, tokenExpiry: 60, roomExpiry: 60, fault: null, startFault,
    polls: [], accepted: 0, starts: 0, renewals: 0, errors: [] };
  const transport = { configured: true, available: true, community_joined: true };
  const poll = () => ({ room_slug: "chat-room", display_name: "Owner", latest_seq: 41,
    participants: [], objects: [], transport });
  const server = createServer(async (req, res) => {
    const url = new URL(req.url, "http://fixture");
    const json = (status, value) => { res.writeHead(status, { "content-type": "application/json" }); res.end(JSON.stringify(value)); };
    if (url.pathname === "/") {
      res.writeHead(200, { "content-type": "text/html" });
      res.end(`<iframe src="/apps/chat-room/?home_origin=${encodeURIComponent(origin)}${direct ? "&conversation_id=direct:fixture" : ""}#home_token=old-frame"></iframe>
        <script>window.renewals=0;addEventListener('message',async e=>{
          if(e.source!==document.querySelector('iframe').contentWindow||e.data.type!=='home:launch-target')return;
          window.renewals++;await fetch('/fixture/renew');
          e.source.postMessage({type:'home:shell-response',requestId:e.data.requestId,result:{target:'chat-room',attach_kind:'iframe',launch_status:'launched',route:'/apps/chat-room/#home_token=fresh-frame-'+window.renewals}},location.origin);
        });</script>`);
      return;
    }
    if (url.pathname === "/fixture/renew") { state.renewals++; state.tokenExpiry = state.now + 60; json(200, {}); return; }
    if (url.pathname.startsWith("/api/")) {
      let body = ""; for await (const chunk of req) body += chunk;
      if (url.pathname.endsWith("/poll") || url.pathname.endsWith("/messages")) {
        const request = { ...JSON.parse(body || "{}"), token: req.headers["x-elastos-home-token"], now: state.now };
        state.polls.push(request);
        if (state.fault === "network") { req.socket.destroy(); return; }
        if (state.fault) { json(state.fault, { error: `request failed: ${state.fault}` }); return; }
        if (state.now >= state.tokenExpiry) { res.writeHead(403); res.end("Gateway request requires an admitted host and caller"); return; }
        if (state.now >= state.roomExpiry) { json(401, { error: "invalid or expired session for configured Chat" }); return; }
        state.accepted++;
        json(200, url.pathname.endsWith("/messages") ? { conversation_id: "direct:fixture", messages: [
          { message_id: "message:kept", direction: "incoming", text: "Kept history", created_at: 1, delivery_state: "received" },
        ] } : poll()); return;
      }
      if (url.pathname.endsWith("/session/start")) {
        state.starts++;
        if (state.startFault) { json(state.startFault, { error: serverSecret }); return; }
        state.roomExpiry = state.now + 60; json(200, { poll: poll(), expires_at: state.roomExpiry }); return;
      }
      if (url.pathname.endsWith("/summary")) {
        json(200, { room_slug: "chat-room", pending_count: 0, active_session_count: 1, transport }); return;
      }
      if (url.pathname.endsWith("/conversations")) { json(200, { conversations: direct ? [{ conversation_id: "direct:fixture", display_name: "Contact" }] : [], community_unread: false }); return; }
      json(200, {}); return;
    }
    const relative = url.pathname.replace(/^\/apps\//, "capsules/").replace(/^(capsules\/[^/]+)\//, "$1/browser/");
    const file = resolve(root, relative + (url.pathname.endsWith("/") ? "index.html" : ""));
    if (!file.startsWith(`${root}/capsules/`)) { res.writeHead(404); res.end(); return; }
    try {
      res.writeHead(200, { "content-type": ({ ".js": "text/javascript", ".wasm": "application/wasm", ".html": "text/html", ".css": "text/css" })[extname(file)] || "application/octet-stream" });
      res.end(readFileSync(file));
    } catch { res.end(); }
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const origin = `http://127.0.0.1:${server.address().port}`;
  let browser;
  t.after(async () => { await browser?.close(); server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); });
  browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  await page.addInitScript(secret => {
    window.sessionStartSecretLeaks = [];
    new MutationObserver(() => {
      if (document.documentElement?.outerHTML.includes(secret)) window.sessionStartSecretLeaks.push(true);
    }).observe(document, { subtree: true, childList: true, characterData: true, attributes: true });
  }, serverSecret);
  page.on("pageerror", error => state.errors.push(error.message));
  await page.route("**/*", route => new URL(route.request().url()).origin === origin ? route.continue() : route.abort());
  await page.clock.install({ time: new Date(0) });
  await page.goto(origin);
  const frame = page.frames().find(frame => frame.url().includes("/apps/chat-room/"));
  if (startFault) {
    await frame.waitForFunction(() => document.body.textContent.includes("session bootstrap") || document.body.textContent.includes("provider-secret"));
    return { state, page, frame };
  }
  if (direct) await frame.getByText("Kept history", { exact: true }).waitFor();
  else await frame.waitForSelector('body[data-room-session-active="true"]', { state: "attached", timeout: 10000 });
  await frame.locator("#message-input").fill("Keep this draft");
  const tick = async (count = 1) => {
    for (let i = 0; i < count; i++) {
      state.now += 1;
      await page.clock.runFor(1100);
      // Let real HTTP requests settle between virtual timer ticks.
      await new Promise(resolve => setTimeout(resolve, 40));
    }
  };
  await tick(2);
  assert.ok(state.polls.length > 0, "the actual WASM is polling before the fault");
  if (!direct) assert.equal(state.polls.at(-1).since, 41);
  assert.deepEqual(state.errors, []);
  return { state, page, frame, tick };
}

for (const path of ["initial bootstrap", "room renewal", "Home recovery"]) {
  test(`Chat bounds session-start server errors during ${path}`, { timeout: 30000 }, async t => {
    const { state, frame, tick } = await fixture(t, false,
      path === "initial bootstrap" ? { startFault: 503 } : {});
    if (tick) {
      const starts = state.starts;
      state.startFault = 503;
      if (path === "room renewal") state.now = 31;
      else state.fault = 401;
      await tick(2);
      state.fault = null;
      await tick(35);
      assert.ok(state.starts > starts, "the session-start call site reached the failing server");
    }
    assert.ok(state.starts > 0);
    const ui = await frame.locator("html").evaluate(element => element.outerHTML);
    assert.ok(!ui.includes(serverSecret), "server response secret must never enter rendered UI state");
    assert.deepEqual(await frame.evaluate(() => window.sessionStartSecretLeaks), [],
      "all rendered UI transitions keep the server body private");
    assert.ok(ui.includes(boundedStartFailure), "the call site renders only the bounded failure detail");
    assert.deepEqual(state.errors, []);
  });
}

for (const direct of [false, true]) for (const failure of (direct
  ? ["frame expiry", "401", "network", "503"]
  : ["frame expiry", "401", "room expiry", "network", "503"])) {
  test(`${direct ? "Direct" : "Community"} Chat resumes polling without a click after ${failure}`, { timeout: 30000 }, async t => {
    const { state, frame, tick } = await fixture(t, direct);
    const before = state.polls.length;
    if (failure === "frame expiry") { state.now = 61; state.roomExpiry = 1000; }
    else if (failure === "room expiry") { state.now = 61; state.tokenExpiry = 1000; }
    else state.fault = failure === "401" ? 401 : failure === "503" ? 503 : "network";
    await tick(2);
    assert.ok(state.polls.length > before, "the poll actually reached the fault");
    const atFailure = state.polls.length;
    const acceptedAtFailure = state.accepted;
    state.fault = null;
    await tick(35); // More than the proposed maximum retry delay; no user event.
    const observation = { before, atFailure, after: state.polls.length, acceptedAfterFault: state.accepted - acceptedAtFailure, starts: state.starts,
      renewals: state.renewals, active: await frame.locator("body").getAttribute("data-room-session-active"),
      status: await frame.locator("#status-badge").textContent(),
      draft: await frame.locator("#message-input").inputValue() };
    t.diagnostic(JSON.stringify(observation));
    const automaticPolls = state.polls.length;
    if (!direct && observation.active === "false") {
      // Positive control: the same document and healthy backend work after the
      // existing button is used. The failing assertion below tests automation.
      await frame.locator("#reconnect-button").evaluate(button => button.click());
      await tick(3);
      assert.equal(await frame.locator("body").getAttribute("data-room-session-active"), "true",
        "fixture must recover through the existing user action");
      assert.ok(state.polls.length > automaticPolls);
      t.diagnostic(`manual Reconnect control: active=true, polls=${state.polls.length}, renewals=${state.renewals}`);
    }
    assert.ok(automaticPolls > atFailure, `polling stayed stopped: ${JSON.stringify(observation)}`);
    assert.ok(observation.acceptedAfterFault > 0, `no successful poll after recovery: ${JSON.stringify(observation)}`);
    if (!direct) assert.equal(observation.active, "true");
    assert.equal(observation.draft, "Keep this draft");
    if (!direct) assert.equal(state.polls.at(-1).since, 41, "recovery keeps the cursor");
    else assert.equal(await frame.getByText("Kept history", { exact: true }).count(), 1, "recovery keeps history without duplicates");
    if (failure === "frame expiry") assert.ok(state.renewals > 0);
    if (failure === "room expiry") assert.ok(state.starts > 1);
  });
}


test("A long outage bounds retries, shows Reconnecting and resumes on online", { timeout: 30000 }, async t => {
  const { state, frame, tick } = await fixture(t);
  state.fault = 503;
  state.tokenExpiry = state.roomExpiry = 1000; // Isolate transport recovery from authority expiry.
  const before = state.polls.length;
  await tick(90);
  const attempts = state.polls.length - before;
  assert.ok(attempts >= 5 && attempts <= 12, `bounded retries: ${attempts}`);
  assert.equal(await frame.locator("#status-badge").textContent(), "Reconnecting");
  state.fault = null;
  const accepted = state.accepted;
  await frame.evaluate(() => dispatchEvent(new Event("online")));
  await tick(2);
  assert.ok(state.accepted > accepted);
  assert.equal(await frame.locator("#message-input").inputValue(), "Keep this draft");
});
