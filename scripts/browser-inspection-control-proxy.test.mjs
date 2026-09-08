import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("./browser-vm-control-service.mjs", import.meta.url), "utf8");
function declaration(name) {
  const start = source.search(new RegExp(`(?:async )?function ${name}\\(`));
  const end = source.slice(start).search(/\n\}(?=\n|$)/);
  assert.ok(start >= 0 && end > 0, name);
  return source.slice(start, start + end + 2);
}
const start = source.indexOf("      const inspectionMatch =");
const end = source.indexOf("      const pageReadMatch =", start);
assert.ok(start > 0 && end > start);

async function proxyHarness(t) {
  const scratch = mkdtempSync(path.join(tmpdir(), "browser-inspect-"));
  const socket = path.join(scratch, "guest.sock");
  const requests = [];
  let respond = (_req, res) => res.end(JSON.stringify({ page_id: "page:owned" }));
  const guest = http.createServer(async (req, res) => {
    let body = ""; for await (const chunk of req) body += chunk;
    requests.push({ method: req.method, path: req.url, body: body ? JSON.parse(body) : null });
    respond(req, res);
  });
  const activePages = new Map([["page:owned", { page: { control_socket_path: socket } }]]);
  const context = vm.createContext({ http, Buffer, Error, AbortController, setTimeout, clearTimeout,
    activePages, activeVms: new Map() });
  vm.runInContext(["safeId", "validateAbsolutePath", "readJsonBody", "requestJsonOverUnix",
    "browserDisplayControlError", "browserInspectionControlError", "activePageGuestControl", "proxyGuestPageInspect"]
    .map(declaration).join("\n"), context);
  const handler = vm.runInContext(`(async function(req, url, sendJson) { ${source.slice(start, end)} })`, context);
  const proxy = http.createServer((req, res) => {
    handler(req, new URL(req.url, "http://localhost"), (status, result) => {
      res.writeHead(status, { "content-type": "application/json" }); res.end(JSON.stringify(result));
    }).catch(error => { res.writeHead(500); res.end(error.message); });
  });
  t.after(async () => {
    for (const server of [proxy, guest]) {
      server.closeAllConnections();
      if (server.listening) await new Promise(resolve => server.close(resolve));
    }
    rmSync(scratch, { recursive: true, force: true });
  });
  await new Promise(resolve => guest.listen(socket, resolve));
  await new Promise(resolve => proxy.listen(0, "127.0.0.1", resolve));
  const endpoint = `http://127.0.0.1:${proxy.address().port}/pages/page%3Aowned/inspect`;
  return { requests, activePages, respond: fn => { respond = fn; },
    read: (body = null) => fetch(endpoint, body === null ? {} : { method: "POST", body: JSON.stringify(body) }),
  };
}

test("inspection uses the acquired page Unix path for GET discovery and bounded POST", async t => {
  const h = await proxyHarness(t);
  assert.equal((await h.read()).status, 200);
  const body = { schema: "elastos.browser.inspect-request/v1", limit: 3 };
  assert.equal((await h.read(body)).status, 200);
  assert.deepEqual(h.requests, [
    { method: "GET", path: "/pages/page%3Aowned/inspect", body: null },
    { method: "POST", path: "/pages/page%3Aowned/inspect", body },
  ]);
  h.activePages.get("page:owned").cleanup_pending = true;
  assert.equal((await h.read()).status, 503);
  h.activePages.delete("page:owned");
  assert.equal((await h.read()).status, 503);
  assert.equal(h.requests.length, 2);
});

test("inspection preserves only typed guest errors and bounds response bytes", async t => {
  const h = await proxyHarness(t);
  for (const [code, status] of Object.entries({ invalid_inspection: 400, inspection_unsupported: 501,
    stale_inspection: 409, inspection_busy: 409, inspection_owner_changed: 409, inspection_failed: 503 })) {
    h.respond((_req, res) => { res.writeHead(status); res.end(JSON.stringify({ code, error: "private detail" })); });
    const response = await h.read();
    assert.equal(response.status, status);
    assert.deepEqual(await response.json(), { code, error: "Browser page inspection could not complete." });
  }
  h.respond((_req, res) => { res.writeHead(404); res.end('{}'); });
  assert.equal((await h.read()).status, 501); // Older guests do not implement discovery.
  for (const body of ['private malformed JSON', JSON.stringify({ content: "x".repeat(32769) })]) {
    h.respond((_req, res) => res.end(body));
    const response = await h.read();
    assert.equal(response.status, 503);
    assert.equal((await response.json()).code, "inspection_failed");
  }
});

for (const change of ["close", "replace", "retire"]) {
  test(`inspection rejects ${change} during a delayed control read`, async t => {
    const h = await proxyHarness(t);
    for (const failed of [false, true]) {
      let release, entered;
      const ready = new Promise(resolve => { entered = resolve; });
      const original = h.activePages.get("page:owned");
      h.respond((_req, res) => {
        release = () => { res.writeHead(failed ? 503 : 200); res.end(JSON.stringify({ page_id: "page:owned", content: "late content" })); };
        entered();
      });
      const pending = h.read(); await ready;
      if (change === "close") h.activePages.delete("page:owned");
      if (change === "replace") h.activePages.set("page:owned", { ...original });
      if (change === "retire") original.cleanup_pending = true;
      release();
      const response = await pending;
      assert.equal(response.status, 409);
      assert.deepEqual(await response.json(), { code: "inspection_owner_changed", error: "Browser page inspection could not complete." });
      delete original.cleanup_pending;
      h.activePages.set("page:owned", original);
    }
  });
}

test("an inspection read has an absolute deadline even while the guest keeps sending bytes", async t => {
  const h = await proxyHarness(t);
  let guestClosed;
  const closed = new Promise(resolve => { guestClosed = resolve; });
  h.respond((_req, res) => {
    res.write('{"pending":"');
    const timer = setInterval(() => res.write("x"), 30);
    res.on("close", () => { clearInterval(timer); guestClosed(); });
  });
  const started = performance.now();
  const response = await h.read();
  assert.equal(response.status, 503);
  assert.equal((await response.json()).code, "inspection_failed");
  assert.ok(performance.now() - started < 3000);
  await closed;
  assert.equal(h.activePages.size, 1);
});
