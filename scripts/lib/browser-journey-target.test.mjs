import assert from "node:assert/strict";
import { createServer } from "node:http";
import test from "node:test";
import { createHash } from "node:crypto";
import { browserJourneyTargetConfig, readBrowserJourneyHealth, readBrowserJourneyReceipt,
  browserJourneyEngineChoice, browserJourneyEngineRoute, browserJourneyFixtureUrl,
  browserJourneyProfileStorage, browserJourneyProfileBinding, browserJourneyProfilePair } from "./browser-journey-target.mjs";

const profileEnv = { HOME_VIRTUAL_AUTH_BROWSER_CONTROLLED_JOURNEY: "1", HOME_VIRTUAL_AUTH_BROWSER_REQUIRE_VZ_TRANSPORT: "1",
  HOME_VIRTUAL_AUTH_BROWSER_PROFILE_MODE: "write", HOME_VIRTUAL_AUTH_BROWSER_PROFILE_MARKER: "profile-marker-123" };
const storage = mode => ({ schema: "elastos.browser.profile-storage/v1", mode, marker: profileEnv.HOME_VIRTUAL_AUTH_BROWSER_PROFILE_MARKER,
  measurement_ok: true, ok: true, error: null, cookie: { present: true, matches: true }, local_storage: { present: true, matches: true },
  indexed_db: { present: true, matches: true, write_request_succeeded: mode === "write", write_committed: mode === "write",
    read_request_succeeded: true, read_completed: true } });

test("profile flags are explicit and every fixture URL preserves phase, marker and run", () => {
  const run = "profile-run-123";
  assert.equal(browserJourneyFixtureUrl(browserJourneyTargetConfig(), run, "main"), `http://localhost:61511/main?run=${run}`);
  for (const mode of ["write", "read"]) {
    const config = browserJourneyTargetConfig({ ...profileEnv, HOME_VIRTUAL_AUTH_BROWSER_PROFILE_MODE: mode });
    for (const page of ["main", "nav", "main"]) {
      const url = new URL(browserJourneyFixtureUrl(config, run, page, { media: true, qualification: true }));
      assert.equal(url.origin, config.origin); assert.equal(url.pathname, `/${page}`);
      assert.deepEqual([...url.searchParams], [["run", run], ["media", "1"], ["qualification", "1"],
        ["profile", mode], ["marker", profileEnv.HOME_VIRTUAL_AUTH_BROWSER_PROFILE_MARKER]]);
    }
  }
  for (const change of [{ HOME_VIRTUAL_AUTH_BROWSER_PROFILE_MODE: "" }, { HOME_VIRTUAL_AUTH_BROWSER_PROFILE_MODE: "reset" },
    { HOME_VIRTUAL_AUTH_BROWSER_PROFILE_MARKER: "" }, { HOME_VIRTUAL_AUTH_BROWSER_PROFILE_MARKER: "short" },
    { HOME_VIRTUAL_AUTH_BROWSER_PROFILE_MARKER: "x".repeat(65) }, { HOME_VIRTUAL_AUTH_BROWSER_PROFILE_MARKER: "bad&marker" },
    { HOME_VIRTUAL_AUTH_BROWSER_CONTROLLED_JOURNEY: "0" }, { HOME_VIRTUAL_AUTH_BROWSER_REQUIRE_VZ_TRANSPORT: "0" },
    { HOME_VIRTUAL_AUTH_BROWSER_PROFILE_RESET: "1" }]) assert.throws(() => browserJourneyTargetConfig({ ...profileEnv, ...change }));
});

test("profile receipts require real measured storage completion and retain earlier failure", () => {
  for (const mode of ["write", "read"]) {
    const config = browserJourneyTargetConfig({ ...profileEnv, HOME_VIRTUAL_AUTH_BROWSER_PROFILE_MODE: mode });
    const receipt = { schema: "elastos.browser.journey-receipt/v1", run: "profile-run-123", profile_probe: config.profile,
      events: [{ type: "profile_storage", page: "main", sequence: 2, profile_storage: storage(mode) }] };
    assert.equal(browserJourneyProfileStorage(config, receipt.run, receipt, "main").sequence, 2);
    assert.equal(browserJourneyProfileStorage(config, receipt.run, receipt, "nav"), null);
    assert.equal(browserJourneyProfileStorage(config, receipt.run, receipt, "main", 2), null);
    const changes = [r => { r.run = "other-run"; }, r => { r.profile_probe = { ...config.profile, mode: "wrong" }; },
      r => { r.events[0].profile_storage.marker = "other-marker"; }, r => { r.events[0].profile_storage.measurement_ok = false; },
      r => { r.events[0].profile_storage.error = "transaction_aborted"; }, r => { r.events[0].profile_storage.cookie.matches = false; },
      r => { r.events[0].profile_storage.local_storage.matches = false; }, r => { r.events[0].profile_storage.indexed_db.matches = false; },
      r => { r.events[0].profile_storage.indexed_db.read_completed = false; },
      r => { r.events[0].profile_storage.indexed_db.read_request_succeeded = false; },
      r => { r.events[0].profile_storage.indexed_db.write_committed = mode !== "write"; }];
    for (const change of changes) {
      const bad = structuredClone(receipt); change(bad);
      bad.events.push({ ...structuredClone(receipt.events[0]), sequence: 3 });
      assert.throws(() => browserJourneyProfileStorage(config, receipt.run, bad, "main"), error => {
        assert.equal(error.details.profile_receipt, bad); return true;
      });
    }
  }
});

test("profile binding uses accepted Runtime principal, lifecycle profile and actual Engine; phase pair rejects changed identity", () => {
  const hash = s => createHash("sha256").update(s).digest("hex"), short = s => `sha256:${hash(s).slice(0, 16)}`;
  const principal = "person:local:test123", pageId = "profile-page-123", config = browserJourneyTargetConfig(profileEnv);
  const token = Buffer.from(JSON.stringify({ signer_did: "did:key:runtime123", payload: {
    schema: "elastos.home.launch-token/v4", principal_id: principal, launch_context: { executable_actor: "browser" } } })).toString("base64url");
  const summary = { schema: "elastos.browser.runtime/v1", principal_id: principal, sessions: {
    lifecycle: { schema: "elastos.browser.lifecycle-status/v1", sessions: [{ phase: "ACTIVE_SESSION", page_id: short(pageId),
      principal_id: short(principal), profile_key_hash: short(`profile-${hash(principal)}`) }] },
    recoverable_page: { schema: "elastos.browser.recoverable-page/v1", state: "active", page_id: pageId,
      engine_page: { page_id: pageId, adapter: "browser-vm-test", provider: "browser-engine" },
      service_selection: { schema: "elastos.browser.service-selection/v1", engine_id: "" } } } };
  const binding = browserJourneyProfileBinding(config, summary, pageId, "http://localhost:8090/apps/browser/", token);
  assert.equal(binding.principal_sha256, hash(principal)); assert.equal(binding.runtime_signer_sha256, hash("did:key:runtime123"));
  assert.equal(binding.profile_key_hash, summary.sessions.lifecycle.sessions[0].profile_key_hash);
  assert.equal(binding.engine_id, "", "Automatic keeps its actual empty Runtime selection");
  assert.ok(!JSON.stringify(binding).includes(principal)); assert.ok(!JSON.stringify(binding).includes(token));
  for (const change of [s => { s.principal_id = "other-principal"; }, s => { s.sessions.lifecycle.sessions[0].profile_key_hash = short("other"); },
    s => { s.sessions.lifecycle.sessions[0].phase = "RETIRING"; }, s => { s.sessions.lifecycle.sessions.push(s.sessions.lifecycle.sessions[0]); },
    s => { s.sessions.lifecycle.sessions[0].principal_id = short("other"); }, s => { s.sessions.recoverable_page.page_id = "other"; },
    s => { delete s.sessions.recoverable_page.service_selection.engine_id; },
    s => { s.sessions.recoverable_page.service_selection.engine_id = null; },
    s => { s.sessions.recoverable_page.engine_page.adapter = ""; },
    s => { s.sessions.recoverable_page.engine_page.provider = ""; }]) {
    const bad = structuredClone(summary); change(bad);
    assert.throws(() => browserJourneyProfileBinding(config, bad, pageId, "http://localhost:8090", token));
  }
  const explicit = { ...config, engineId: "browser-vm-test" };
  assert.throws(() => browserJourneyProfileBinding(explicit, summary, pageId, "http://localhost:8090", token), /Engine selection mismatch/);
  const selected = structuredClone(summary);
  selected.sessions.recoverable_page.service_selection.engine_id = explicit.engineId;
  assert.equal(browserJourneyProfileBinding(explicit, selected, pageId, "http://localhost:8090", token).engine_id, explicit.engineId);
  const write = { schema: "elastos.browser.profile-journey/v1", mode: "write", marker: config.profile.marker,
    run: "write-run-123", complete: true, binding, started_at: 1000, completed_at: 2000 };
  const read = { ...write, mode: "read", run: "read-run-123", started_at: 2001, completed_at: 3000 };
  assert.equal(browserJourneyProfilePair(write, read).read_run, read.run);
  assert.equal(browserJourneyProfilePair(write, read).binding.engine_id, "");
  for (const missing of [null, undefined]) {
    const bad = { ...binding, engine_id: missing };
    assert.throws(() => browserJourneyProfilePair({ ...write, binding: bad }, { ...read, binding: bad }), /missing identity/);
  }
  for (const key of Object.keys(binding)) {
    assert.throws(() => browserJourneyProfilePair(write, { ...read, binding: { ...binding, [key]: "different" } }));
  }
  for (const change of [{ mode: "write" }, { marker: "different-marker" }, { run: write.run }, { complete: false }, { binding: null },
    { binding: {} }, { started_at: 1999 }, { completed_at: null }]) {
    assert.throws(() => browserJourneyProfilePair(write, { ...read, ...change }));
  }
});

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
