import test from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readFile, readdir } from "node:fs/promises";
import { resolve, join } from "node:path";
import { pathToFileURL } from "node:url";
import { createHash } from "node:crypto";
import { once } from "node:events";
import { createBrowserOperatorClient } from "./browser-operator-client.mjs";
import { createPlaywrightOperatorAdapter, PLAYWRIGHT_OPERATOR_CLIENT } from "./browser-operator-client-playwright.mjs";

// Explicit dependency input: the unchanged npm package, without browser binaries.
assert.ok(process.env.BROWSER_OPERATOR_PLAYWRIGHT_CORE, "Set BROWSER_OPERATOR_PLAYWRIGHT_CORE to the pinned playwright-core package directory");
const sdkPath = resolve(process.env.BROWSER_OPERATOR_PLAYWRIGHT_CORE);
const metadata = JSON.parse(await readFile(join(sdkPath, "package.json"), "utf8"));
assert.equal(metadata.name, PLAYWRIGHT_OPERATOR_CLIENT.package);
assert.equal(metadata.version, PLAYWRIGHT_OPERATOR_CLIENT.version);
const { chromium } = await import(pathToFileURL(join(sdkPath, "index.mjs")));
const requireSDK = createRequire(join(sdkPath, "package.json"));
const { wsServer: WebSocketServer, ws: WebSocket } = requireSDK("./lib/utilsBundle.js");
const accessKey = "playwright_source_fixture_key";
const generation = "a".repeat(32), snapshotId = "b".repeat(32);
const pageId = "invited-runtime-page", admissionId = "invited-admission";
const headers = { authorization: `Bearer ${accessKey}`, "x-elastos-playwright-client": "1.59.1" };
const rawHeaders = { ...headers, "x-playwright-browser": "chromium" };

test("every SDK file matches the unchanged registry tarball tree", async () => {
  const paths = [];
  async function walk(directory, prefix = "") {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const relative = `${prefix}${entry.name}`;
      if (entry.isDirectory()) await walk(join(directory, entry.name), `${relative}/`);
      else { assert.equal(entry.isFile(), true); paths.push(relative); }
    }
  }
  await walk(sdkPath);
  const hash = createHash("sha256");
  for (const path of paths.sort()) hash.update(`${path}\0${createHash("sha256").update(await readFile(join(sdkPath, path))).digest("hex")}\n`);
  assert.equal(hash.digest("hex"), PLAYWRIGHT_OPERATOR_CLIENT.treeSha256);
});

function fixture() {
  const state = { value: "initial", status: "active", generation, snapshotId, calls: [], effects: [],
    pageOpen: true, writer: true, detachCount: 0, loseInputReply: false, detachUncertain: false };
  const admission = () => ({ schema: "elastos.browser.operator-admission/v1", request_id: admissionId,
    status: state.status, capability: "fixture-write-grant", inspection_capability: "fixture-read-grant",
    writer_acquired: state.writer });
  const fetchImpl = async (url, options) => {
    options.signal.throwIfAborted();
    assert.equal(options.headers.Authorization, "Bearer operator-session");
    assert.equal(options.headers["x-elastos-home-token"], undefined);
    assert.equal(new URL(url).origin, "https://runtime.invalid");
    const path = new URL(url).pathname;
    const prefix = `/api/apps/browser/pages/${pageId}`;
    state.calls.push({ path, method: options.method });
    if (path === `${prefix}/operator-requests/${admissionId}`) {
      if (state.beforeStatus) await state.beforeStatus(options.signal);
      return Response.json(admission());
    }
    if (path === `${prefix}/operator-requests/${admissionId}/detach`) {
      state.detachCount++;
      if (state.detachUncertain) return Response.json({ code: "operator_native_effect_pending" }, { status: 409 });
      state.status = "revoked"; state.writer = false;
      return Response.json(admission());
    }
    if (state.status !== "active") return Response.json({ code: "operator_revoked" }, { status: 403 });
    if (path === `${prefix}/operator-requests/${admissionId}/inspect`) {
      assert.equal(options.headers["x-elastos-capability"], "fixture-read-grant");
      return Response.json({ schema: "elastos.browser.inspect-result/v1", page_id: pageId,
        snapshot_id: state.snapshotId, document_generation: state.generation, truncated: false, next_cursor: null,
        nodes: [{ ref: `${state.snapshotId}:0`, role: "textbox", name: "Message", description: "", value: state.value }] });
    }
    if (path === `${prefix}/input`) {
      assert.equal(options.headers["x-elastos-capability"], "fixture-write-grant");
      const { event } = JSON.parse(options.body);
      assert.equal(event.admission_id, admissionId);
      assert.equal(event.action, "fill");
      if (event.document_generation !== state.generation || event.ref !== `${state.snapshotId}:0`)
        return Response.json({ code: "stale_inspection" }, { status: 409 });
      state.effects.push(event); state.value = event.text;
      if (state.inputStarted) state.inputStarted();
      if (state.loseInputReply) return new Promise((_, reject) => {
        options.signal.addEventListener("abort", () => reject(options.signal.reason), { once: true });
      });
      return Response.json({ schema: "elastos.browser.ref-input-result/v1", page_id: pageId,
        admission_id: admissionId, document_generation: event.document_generation, request_id: event.request_id,
        accepted: true, outcome: "completed" });
    }
    assert.fail(`unexpected Runtime route ${path}`);
  };
  const client = createBrowserOperatorClient({ origin: "https://runtime.invalid", sessionToken: "operator-session",
    pageId, admissionId, fetchImpl, timeoutMs: 500 });
  return { state, client };
}

async function setup(t, options = {}) {
  const { state, client } = fixture();
  let clock = 0;
  const adapter = createPlaywrightOperatorAdapter({ client, accessKey, now: () => clock });
  const wire = [], rejectionCodes = [];
  const server = new WebSocketServer({ host: "127.0.0.1", port: 0, maxPayload: 16384, perMessageDeflate: false });
  server.on("connection", (socket, request) => {
    let connection;
    try {
      connection = adapter.connect({ headers: request.headers,
        send: raw => { wire.push(JSON.parse(raw)); socket.send(raw); }, close: () => socket.close() });
    } catch (error) { rejectionCodes.push(error.code); socket.close(1008, error.code); return; }
    socket.on("message", (raw, binary) => { void connection.receive(binary ? null : raw.toString()); });
    socket.on("close", () => { void connection.disconnect(); });
    socket.on("error", () => { void connection.disconnect(); });
  });
  await once(server, "listening");
  const endpoint = `ws://127.0.0.1:${server.address().port}`;
  let browser;
  t.after(async () => {
    await browser?.close();
    await adapter.disconnect();
    for (const socket of server.clients) socket.terminate();
    await new Promise(resolve => server.close(resolve));
  });
  if (options.connect !== false) browser = await chromium.connect(endpoint, { headers, timeout: 2000 });
  return { state, client, adapter, wire, rejectionCodes, endpoint, browser,
    page: browser?.contexts()[0]?.pages()[0], advance: ms => { clock += ms; } };
}

test("unchanged pinned SDK connects, fills, clears and disconnects with the Runtime page preserved", async t => {
  const f = await setup(t);
  assert.equal(f.browser.constructor.name, "Browser");
  assert.equal(f.browser.contexts().length, 1);
  assert.equal(f.browser.contexts()[0].pages().length, 1);
  assert.equal(f.page.constructor.name, "Page");
  assert.equal(f.page.mainFrame().constructor.name, "Frame");
  assert.equal(f.adapter.pageId, pageId);
  const snapshot = await f.adapter.inspect();
  const locator = f.page.locator(snapshot.nodes[0].selector);
  assert.equal(locator.constructor.name, "Locator");
  await locator.fill("replacement");
  assert.equal((await f.adapter.inspect()).nodes[0].value, "replacement");
  await locator.clear();
  assert.equal((await f.adapter.inspect()).nodes[0].value, "");
  assert.deepEqual(f.state.effects.map(e => e.text), ["replacement", ""]);
  assert.equal(new Set(f.state.effects.map(e => e.request_id)).size, 2);
  await f.browser.close();
  assert.deepEqual(await f.adapter.closed, { detached: true, page_closed: false });
  assert.equal(f.state.pageOpen, true);
  assert.equal(f.state.writer, false);
  assert.equal(f.state.detachCount, 1);
  assert.doesNotMatch(JSON.stringify(f.wire), /operator-session|fixture-write-grant|fixture-read-grant/);
});

test("ordinary selectors and selector compositions never reach Runtime input", async t => {
  const f = await setup(t);
  const selector = (await f.adapter.inspect()).nodes[0].selector;
  for (const locator of [f.page.locator("input"), f.page.getByRole("textbox", { name: "Message" }),
    f.page.locator(selector).first(), f.page.locator(selector).filter({ hasText: "Message" }),
    f.page.locator(selector.replace(generation, "c".repeat(32)))])
    await assert.rejects(() => locator.fill("x"), /stale_inspection/);
  assert.equal(f.state.effects.length, 0);
});

test("inspection refresh, expiry and Runtime generation checks fence old references", async t => {
  const f = await setup(t);
  const old = f.page.locator((await f.adapter.inspect()).nodes[0].selector);
  f.state.snapshotId = "c".repeat(32);
  const current = f.page.locator((await f.adapter.inspect()).nodes[0].selector);
  await assert.rejects(() => old.fill("old"), /stale_inspection/);
  f.advance(30000);
  await assert.rejects(() => current.fill("expired"), /stale_inspection/);
  const next = f.page.locator((await f.adapter.inspect()).nodes[0].selector);
  f.state.generation = "d".repeat(32);
  await assert.rejects(() => next.fill("wrong document"), /operator_reconciliation_required: request_id=[a-f0-9]{32}/);
  await assert.rejects(() => next.clear(), /operator_reconciliation_required/);
  assert.equal(f.state.effects.length, 0);
});

test("owner revocation rejects the next real-client fill and inspection", async t => {
  const f = await setup(t);
  const locator = f.page.locator((await f.adapter.inspect()).nodes[0].selector);
  f.state.status = "revoked";
  await assert.rejects(() => locator.fill("revoked"), /operator_revoked/);
  await assert.rejects(() => f.adapter.inspect(), /operator_revoked/);
  assert.equal(f.state.effects.length, 0);
});

test("real-client timeout and concurrent request cannot replay an uncertain native effect", async t => {
  const f = await setup(t);
  const locator = f.page.locator((await f.adapter.inspect()).nodes[0].selector);
  let started;
  const inputStarted = new Promise(resolve => { started = resolve; });
  f.state.inputStarted = started; f.state.loseInputReply = true;
  const first = assert.rejects(() => locator.fill("one effect", { timeout: 60 }), /operator_reconciliation_required: request_id=[a-f0-9]{32}/);
  await inputStarted;
  await assert.rejects(() => locator.clear(), /adapter_busy/);
  await first;
  await assert.rejects(() => locator.fill("retry"), /operator_reconciliation_required/);
  assert.equal(f.state.effects.length, 1);
});

test("disconnect aborts an outstanding write, releases once and exposes unconfirmed release", async t => {
  const f = await setup(t);
  const locator = f.page.locator((await f.adapter.inspect()).nodes[0].selector);
  let started;
  const inputStarted = new Promise(resolve => { started = resolve; });
  f.state.inputStarted = started; f.state.loseInputReply = true; f.state.detachUncertain = true;
  const first = assert.rejects(() => locator.fill("one effect"), /closed/);
  await inputStarted;
  await f.browser.close(); await first;
  const release = await f.adapter.closed;
  assert.equal(release.detached, false);
  assert.equal(release.requires_reconciliation, true);
  assert.equal(release.request_id, f.state.effects[0].request_id);
  assert.equal(release.code, "operator_native_effect_pending");
  assert.deepEqual(await f.adapter.disconnect(), release);
  assert.equal(f.state.detachCount, 1);
  assert.equal(f.state.effects.length, 1);
  assert.equal(f.state.pageOpen, true);
  await assert.rejects(() => f.adapter.inspect(), /operator_detached/);
});

test("deadline during grant acquisition prevents input dispatch", async t => {
  const f = await setup(t);
  const locator = f.page.locator((await f.adapter.inspect()).nodes[0].selector);
  f.state.beforeStatus = signal => new Promise((_, reject) => {
    signal.addEventListener("abort", () => reject(signal.reason), { once: true });
  });
  await assert.rejects(() => locator.fill("too late", { timeout: 20 }), /operator_transport_failed/);
  f.state.beforeStatus = null;
  assert.equal(f.state.effects.length, 0);
});

test("actual SDK rejects foreign authority, version and network exposure before admission reads", async t => {
  const f = await setup(t, { connect: false });
  for (const options of [
    { headers: { ...headers, authorization: "Bearer wrong-key" } },
    { headers: { ...headers, "x-elastos-home-token": "owner-credential" } },
    { headers: { ...headers, cookie: "owner-cookie" } },
    { headers: { ...headers, "x-elastos-playwright-client": "1.60.0" } },
    { headers, exposeNetwork: "*" },
  ]) await assert.rejects(() => chromium.connect(f.endpoint, { ...options, timeout: 1000 }));
  assert.deepEqual(f.rejectionCodes, ["adapter_authority_required", "adapter_authority_required", "adapter_authority_required",
    "adapter_client_version_unsupported", "capability_unsupported"]);
  assert.equal(f.state.calls.length, 0);
});

test("a second real SDK connection cannot take over the active adapter", async t => {
  const f = await setup(t);
  await assert.rejects(() => chromium.connect(f.endpoint, { headers, timeout: 1000 }));
  assert.deepEqual(f.rejectionCodes, ["adapter_connection_unavailable"]);
  const locator = f.page.locator((await f.adapter.inspect()).nodes[0].selector);
  await locator.fill("first operator remains");
  assert.equal(f.state.effects.length, 1);
});

test("raw duplicate IDs and malformed messages close only the operator transport", async t => {
  for (const mutation of [message => message, () => "invalid json", () => "x".repeat(16385)]) {
    const f = await setup(t, { connect: false });
    const socket = new WebSocket(f.endpoint, { headers: rawHeaders });
    await once(socket, "open");
    const initialized = new Promise(resolve => socket.on("message", raw => {
      const message = JSON.parse(raw.toString());
      if (message.id === 1) resolve(message);
    }));
    const initialize = JSON.stringify({ id: 1, guid: "", method: "initialize", params: { sdkLanguage: "javascript" } });
    socket.send(initialize); assert.ok((await initialized).result);
    const disconnected = once(socket, "close");
    socket.send(mutation(initialize));
    await disconnected;
    assert.equal((await f.adapter.closed).detached, true);
    assert.equal(f.state.pageOpen, true);
    assert.equal(f.state.effects.length, 0);
  }
});

test("unsupported SDK methods fail explicitly before Runtime dispatch", async t => {
  const f = await setup(t);
  const locator = f.page.locator((await f.adapter.inspect()).nodes[0].selector);
  const calls = f.state.calls.length;
  for (const operation of [
    () => locator.inputValue(), () => locator.click(), () => locator.isVisible(), () => locator.fill("x", { force: true }),
    () => f.page.evaluate(() => document.title), () => f.page.goto("https://example.invalid"),
    () => f.browser.newContext(), () => f.browser.contexts()[0].newPage(),
    () => f.page.screenshot(), () => f.page.context().newCDPSession(f.page),
    () => f.page.request.get("https://example.invalid"), () => f.page.close(), () => f.page.context().close(),
  ]) await assert.rejects(operation, /capability_unsupported/);
  assert.equal(f.state.calls.length, calls);
  assert.equal(f.state.pageOpen, true);
});
