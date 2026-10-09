import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { createHomeNavigationClient } from "../capsules/home/browser/home-navigation-client.js";

const homeOrigin = "https://home.fixture";
const read = path => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
const launched = token => ({ target: "chat-room", attach_kind: "iframe", launch_status: "launched",
  route: `/apps/chat-room/?home_origin=${encodeURIComponent(homeOrigin)}#home_token=${token}` });

function chatFixture() {
  const listeners = new Map(), timers = new Map(), messages = [], clipboard = [];
  let sequence = 0;
  const top = { postMessage: (data, origin) => messages.push({ data, origin }) };
  const parent = {};
  const window = { top, parent, location: { search: `?home_origin=${encodeURIComponent(homeOrigin)}`, hash: "#home_token=old" },
    crypto: { randomUUID: () => `request-${++sequence}` },
    addEventListener(type, fn) { if (!listeners.has(type)) listeners.set(type, new Set()); listeners.get(type).add(fn); },
    removeEventListener(type, fn) { listeners.get(type)?.delete(fn); },
    dispatchEvent(event) { for (const fn of listeners.get(event.type) || []) fn(event); },
    setTimeout(fn) { const id = ++sequence; timers.set(id, fn); return id; },
    clearTimeout(id) { timers.delete(id); },
  };
  const context = vm.createContext({ window, URL, URLSearchParams, Event, init() {},
    createHomeNavigationClient: options => createHomeNavigationClient({ ...options, windowRef: window }),
    createHomeClipboardClient(options) { const client = { token: options.homeToken, started: false, retired: false,
      start() { this.started = true; }, teardown() { this.retired = true; }, writeText() {} };
      clipboard.push(client); return client; },
  });
  const source = read("capsules/chat-room/browser/chat-room.js")
    .replace(/^\s*import[\s\S]*?;/gm, "").replaceAll("import.meta.url", JSON.stringify("https://home.fixture/apps/chat-room/chat-room.js"));
  vm.runInContext(source, context);
  const respond = (requestId, result, extra = {}, source = top, origin = homeOrigin) =>
    window.dispatchEvent({ type: "message", source, origin,
      data: { type: "home:shell-response", requestId, result, ...extra } });
  return { context, window, top, parent, timers, messages, clipboard, respond };
}

test("Reconnect uses one visible action and only the Home response renews the same document", async () => {
  const f = chatFixture();
  assert.equal(f.messages.length, 0, "startup must leave renewal to the user");
  f.context.elastosChatNavigation({ conversation_id: "direct:a" });
  const pending = f.context.elastosChatReconnect();
  assert.equal(f.context.elastosChatReconnect(), pending, "two clicks share one Home launch");
  assert.equal(f.messages.length, 1);
  const request = f.messages[0];
  assert.equal(request.origin, homeOrigin);
  assert.deepEqual(JSON.parse(JSON.stringify(request.data)), { type: "home:launch-target", requestId: request.data.requestId,
    target: "chat-room", query: { conversation_id: "direct:a" }, homeToken: "old" });
  f.respond(request.data.requestId, launched("forged"), {}, {}, homeOrigin);
  f.respond(request.data.requestId, launched("forged"), {}, f.top, "https://evil.fixture");
  f.respond("wrong-request", launched("forged"));
  assert.equal(f.context.elastosChatHomeToken(), "old");
  assert.equal(f.timers.size, 1);
  f.respond(request.data.requestId, launched("fresh"));
  assert.equal(await pending, "fresh");
  assert.equal(f.context.elastosChatHomeToken(), "fresh");
  assert.equal(f.clipboard[0].retired, true);
  assert.equal(f.clipboard[1].token, "fresh");
  assert.equal(f.clipboard[1].started, true);
  assert.equal(f.timers.size, 0);
  assert.equal(f.messages.at(-1).data.type, "home:app-ready");
  // The actual navigation client keeps its selector and document nonce while
  // requiring a new GUI probe under the renewed token.
  f.window.dispatchEvent({ type: "message", source: f.parent, origin: "null",
    data: { type: "elastos.home.navigation.request/v1", requestId: "probe-old", homeToken: "old" } });
  const count = f.messages.length;
  f.window.dispatchEvent({ type: "message", source: f.parent, origin: "null",
    data: { type: "elastos.home.navigation.request/v1", requestId: "probe-fresh", homeToken: "fresh" } });
  assert.equal(f.messages.length, count + 1);
  assert.equal(f.messages.at(-1).data.query.conversation_id, "direct:a");
});

test("Expired Home and refused launches keep the old authority available for a user retry", async () => {
  for (const reply of [{ error: "expired", status: 401 }, { result: { ...launched("fresh"), target: "browser" } },
    { result: { ...launched("fresh"), route: "https://evil.fixture/apps/chat-room/#home_token=fresh" } }]) {
    const f = chatFixture();
    const pending = f.context.elastosChatReconnect();
    const rejection = assert.rejects(pending, /Sign in to Home|invalid Chat launch/);
    f.respond(f.messages[0].data.requestId, reply.result, reply);
    await rejection;
    assert.equal(f.context.elastosChatHomeToken(), "old");
    assert.equal(f.clipboard.length, 1);
    assert.equal(f.clipboard[0].retired, false);
    assert.equal(f.timers.size, 0);
    const retry = f.context.elastosChatReconnect();
    f.respond(f.messages.at(-1).data.requestId, launched("retry"));
    assert.equal(await retry, "retry");
  }
});

test("A missing Home reply expires one request and permits an explicit retry", async () => {
  const f = chatFixture();
  const pending = f.context.elastosChatReconnect();
  const rejection = assert.rejects(pending, /Home did not answer/);
  const oldRequestId = f.messages[0].data.requestId;
  [...f.timers.values()][0]();
  await rejection;
  assert.equal(f.timers.size, 0);
  assert.equal(f.context.elastosChatHomeToken(), "old");
  f.respond(oldRequestId, launched("late"));
  assert.equal(f.context.elastosChatHomeToken(), "old", "expired request must ignore a late Home reply");
  const retry = f.context.elastosChatReconnect();
  f.respond(f.messages.at(-1).data.requestId, launched("retry"));
  assert.equal(await retry, "retry");
});

function sourceFunction(source, name) {
  const start = source.indexOf(`function ${name}(`);
  assert.ok(start >= 0, name);
  return source.slice(start, source.indexOf("\n}", start) + 2);
}

test("Home GUI accepts only the open Chat entry and keeps its iframe document", () => {
  const oldRoute = launched("old").route;
  const frame = { contentWindow: {}, src: oldRoute, dataset: { route: oldRoute }, getAttribute: () => oldRoute };
  const entry = { targetId: "chat-room", node: { querySelector: () => frame }, homeNavigation: { old: true } };
  const probes = [], receipts = [];
  const context = vm.createContext({ URL, URLSearchParams, Date, homeOrigin,
    window: { location: { href: `${homeOrigin}/apps/home-gui/` } }, shellState: { windows: new Map([["chat", entry]]) },
    probeHomeNavigation: (value, force) => probes.push([value, force]),
    postToHome: value => receipts.push(value),
    hasExactKeys: (value, keys) => Object.keys(value).sort().join(",") === keys.sort().join(","),
  });
  const gui = read("capsules/home-gui/browser/home-gui.js");
  for (const name of ["homeLaunchTokenFromRoute", "homeGuiWindowEntryForToken", "renewHomeGuiChatWindowAuthority"])
    vm.runInContext(sourceFunction(gui, name), context);
  vm.runInContext(sourceFunction(read("capsules/home-gui/browser/home-gui-shell.js"), "handleGuiCommand"), context);
  const command = { type: "home:gui-command", command: "renew-chat-authority", requestId: "renew-chat",
    phase: "prepare", homeToken: "old", launched: launched("fresh"), expiresAt: Date.now() + 1000 };
  assert.equal(context.handleGuiCommand({ ...command, expiresAt: Date.now() - 1 }), false);
  assert.equal(receipts.at(-1).ok, false);
  assert.equal(frame.dataset.route, oldRoute);
  assert.equal(context.renewHomeGuiChatWindowAuthority("old", { ...launched("fresh"), target: "browser" }, homeOrigin), false);
  assert.equal(context.renewHomeGuiChatWindowAuthority("old", { ...launched("fresh"), route: "https://evil.fixture/apps/chat-room/#home_token=fresh" }, homeOrigin), false);
  assert.equal(context.handleGuiCommand(command), true);
  assert.equal(receipts.at(-1).ok, true);
  assert.equal(receipts.at(-1).freshHomeToken, "fresh");
  assert.equal(frame.dataset.route, oldRoute, "an unaccepted prepare receipt must preserve retry lookup");
  assert.equal(probes.length, 0);
  assert.equal(context.handleGuiCommand({ ...command, phase: "commit" }), true);
  assert.equal(receipts.at(-1).type, "home:chat-authority-committed");
  assert.equal(receipts.at(-1).ok, true);
  assert.equal(frame.dataset.route, launched("fresh").route);
  assert.equal(frame.src, oldRoute, "renewal must retain the same iframe document and drafts");
  assert.equal(entry.homeNavigation, null);
  assert.equal(probes.length, 1);
  assert.equal(context.renewHomeGuiChatWindowAuthority("old", launched("later"), homeOrigin), false);
  const closedCommand = { ...command, homeToken: "fresh", launched: launched("later") };
  assert.equal(context.handleGuiCommand(closedCommand), true);
  assert.equal(receipts.at(-1).type, "home:chat-authority-renewed");
  context.shellState.windows.clear();
  assert.equal(context.handleGuiCommand({ ...closedCommand, phase: "commit" }), false);
  assert.equal(receipts.at(-1).type, "home:chat-authority-committed");
  assert.equal(receipts.at(-1).ok, false, "a closed Chat entry must refuse the commit");
  assert.equal(context.renewHomeGuiChatWindowAuthority("fresh", launched("later"), homeOrigin), false);
});
