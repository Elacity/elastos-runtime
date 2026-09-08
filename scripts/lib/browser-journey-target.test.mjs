import assert from "node:assert/strict";
import { createServer } from "node:http";
import test from "node:test";
import { browserJourneyTargetConfig, readBrowserJourneyHealth, readBrowserJourneyReceipt,
  browserJourneyEngineChoice, browserJourneyEngineRoute } from "./browser-journey-target.mjs";

test("local fixture defaults stay local; product and admin origins require explicit remote opt-in", () => {
  assert.deepEqual(browserJourneyTargetConfig(), { origin: "http://localhost:61511",
    adminOrigin: "http://localhost:61511", engineId: "", allowRemote: false });
  for (const key of ["HOME_VIRTUAL_AUTH_BROWSER_FIXTURE_ORIGIN", "HOME_VIRTUAL_AUTH_BROWSER_FIXTURE_ADMIN_ORIGIN"]) {
    for (const value of ["http://fixture.example:61512", "https://fixture.example"]) {
      assert.throws(() => browserJourneyTargetConfig({ [key]: value }), /non-loopback/);
      assert.throws(() => browserJourneyTargetConfig({ [key]: value, HOME_VIRTUAL_AUTH_BROWSER_ALLOW_REMOTE_FIXTURE: "true" }));
      assert.equal(browserJourneyTargetConfig({ [key]: value,
        HOME_VIRTUAL_AUTH_BROWSER_ALLOW_REMOTE_FIXTURE: "1" })[key.endsWith("ADMIN_ORIGIN") ? "adminOrigin" : "origin"], value);
    }
    for (const value of ["file:///tmp/fixture", "http://user:secret@localhost:61511", "http://localhost:61511/path",
      "http://localhost:61511/?run=x", "http://localhost:61511/#x", "http://localhost:61511/a/..",
      " http://localhost:61511", "http://LOCALHOST:61511", "http://localhost:61511?", "http://localhost:61511#"]) {
      assert.throws(() => browserJourneyTargetConfig({ [key]: value, HOME_VIRTUAL_AUTH_BROWSER_ALLOW_REMOTE_FIXTURE: "1" }),
        /exact HTTP\(S\) origin/, value);
    }
  }
  assert.equal(browserJourneyTargetConfig({ HOME_VIRTUAL_AUTH_BROWSER_FIXTURE_ORIGIN: "http://[::1]:61511/" }).origin,
    "http://[::1]:61511");
  for (const engineId of ["with spaces", "a".repeat(129), "engine?token=x", "engine/id"]) {
    assert.throws(() => browserJourneyTargetConfig({ HOME_VIRTUAL_AUTH_BROWSER_ENGINE_ID: engineId }), /exact Runtime identifier/);
  }
});

test("health and run receipts use the read-only admin origin, omit credentials and reject redirects", async () => {
  const seen = [], run = "fixture-run";
  const server = createServer((req, res) => {
    seen.push({ method: req.method, url: req.url, cookie: req.headers.cookie, authorization: req.headers.authorization });
    if (req.url === "/receipt?run=redirect-run") { res.writeHead(302, { location: "/health" }); res.end(); return; }
    res.setHeader("content-type", "application/json");
    res.end(JSON.stringify(req.url === "/health" ? { schema: "elastos.browser.journey-fixture/v1", ok: true } :
      { schema: "elastos.browser.journey-receipt/v1", run, events: [{ type: "load", page: "main" }] }));
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const config = browserJourneyTargetConfig({ HOME_VIRTUAL_AUTH_BROWSER_FIXTURE_ORIGIN: "http://fixture.example:61512",
    HOME_VIRTUAL_AUTH_BROWSER_FIXTURE_ADMIN_ORIGIN: `http://127.0.0.1:${server.address().port}`,
    HOME_VIRTUAL_AUTH_BROWSER_ALLOW_REMOTE_FIXTURE: "1" });
  try {
    assert.equal((await readBrowserJourneyHealth(config)).ok, true);
    assert.equal((await readBrowserJourneyReceipt(config, run)).events.length, 1);
    await assert.rejects(readBrowserJourneyReceipt(config, "other-run"), /receipt failed/);
    await assert.rejects(readBrowserJourneyReceipt(config, "redirect-run"), /fetch failed/);
    assert.deepEqual(seen.map(row => row.url), ["/health", "/receipt?run=fixture-run", "/receipt?run=other-run", "/receipt?run=redirect-run"]);
    assert.ok(seen.every(row => row.method === "GET" && !row.cookie && !row.authorization));
    const signal = new AbortController().signal;
    await readBrowserJourneyReceipt(config, run, { signal, fetchImpl: async (url, options) => {
      assert.equal(url, `${config.adminOrigin}/receipt?run=${run}`);
      assert.equal(options.signal, signal); assert.equal(options.credentials, "omit"); assert.equal(options.redirect, "error");
      return { ok: true, json: async () => ({ schema: "elastos.browser.journey-receipt/v1", run, events: [] }) };
    } });
  } finally { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); }
});

test("fixture reads preserve health schema and exact receipt run binding", async () => {
  const config = browserJourneyTargetConfig();
  const response = (body, ok = true, status = 200) => async () => ({ ok, status, json: async () => body });
  for (const [body, ok] of [[{ schema: "wrong", ok: true }, true],
    [{ schema: "elastos.browser.journey-fixture/v1", ok: false }, true],
    [{ schema: "elastos.browser.journey-fixture/v1", ok: true }, false]]) {
    await assert.rejects(readBrowserJourneyHealth(config, response(body, ok)), /fixture is unavailable/);
  }
  const receipt = { schema: "elastos.browser.journey-receipt/v1", run: "exact-run", events: [] };
  for (const body of [{ ...receipt, schema: "wrong" }, { ...receipt, run: "another-run" }, { ...receipt, events: null }]) {
    await assert.rejects(readBrowserJourneyReceipt(config, "exact-run", { fetchImpl: response(body) }), /receipt failed/);
  }
  await assert.rejects(readBrowserJourneyReceipt(config, "exact-run", { fetchImpl: response(receipt, false, 503) }));
  assert.deepEqual(await readBrowserJourneyReceipt(config, "exact-run", {
    fetchImpl: response({ error: "unknown run" }, false, 404) }), receipt);
});

test("approved Engine selection binds the active Runtime page and concrete adapter, not just the UI request", () => {
  const engineId = "remote-engine-test", adapterId = "browser-vm-test";
  const adapter = { id: engineId, direct_network: false, wallet_injection: false };
  const summary = { engine_adapter: { adapters: [adapter], remote_services: { offers: [{ state: "approved",
    launch_available: true, selectable_adapters: [adapter], adapters: [{ ...adapter, id: adapterId }] }] } } };
  const choice = browserJourneyEngineChoice(summary, engineId);
  assert.deepEqual(choice, { engineId, adapterId });
  const owner = { schema: "elastos.browser.recoverable-page/v1", state: "active", page_id: "page-one",
    engine_page: { page_id: "page-one", adapter: adapterId, provider: "browser-engine" },
    service_selection: { schema: "elastos.browser.service-selection/v1", engine_id: engineId } };
  assert.deepEqual(browserJourneyEngineRoute({ sessions: { recoverable_page: owner } }, choice, "page-one"),
    { page_id: "page-one", engine_id: engineId, adapter_id: adapterId, provider: "browser-engine" });
  for (const mutate of [s => { s.engine_adapter.adapters = []; }, s => { s.engine_adapter.adapters.push(adapter); },
    s => { s.engine_adapter.adapters[0].direct_network = true; }, s => { s.engine_adapter.adapters[0].wallet_injection = true; },
    s => { s.engine_adapter.remote_services.offers[0].state = "expired"; },
    s => { s.engine_adapter.remote_services.offers[0].launch_available = false; },
    s => { s.engine_adapter.remote_services.offers.push(s.engine_adapter.remote_services.offers[0]); },
    s => { s.engine_adapter.remote_services.offers[0].adapters = []; }]) {
    const bad = structuredClone(summary); mutate(bad); assert.throws(() => browserJourneyEngineChoice(bad, engineId));
  }
  for (const mutate of [o => { o.state = "closing"; }, o => { o.page_id = "another-page"; },
    o => { o.engine_page.page_id = "another-page"; }, o => { o.engine_page.adapter = "another-adapter"; },
    o => { o.service_selection.engine_id = "another-grant"; }, o => { delete o.service_selection; },
    o => { o.service_selection.schema = "wrong"; }, o => { o.schema = "wrong"; }]) {
    const bad = structuredClone(owner); mutate(bad);
    assert.throws(() => browserJourneyEngineRoute({ sessions: { recoverable_page: bad } }, choice, "page-one"), /different Engine or page/);
  }
  assert.throws(() => browserJourneyEngineRoute(summary, choice, "page-one"), /different Engine or page/);
  assert.deepEqual(browserJourneyEngineChoice({ engine_adapter: { adapters: [{ ...adapter, id: adapterId }] } }, adapterId),
    { engineId: adapterId, adapterId });
});
