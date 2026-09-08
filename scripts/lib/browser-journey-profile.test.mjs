import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { browserProfileStorageProbe, createBrowserJourneyFixture } from "./browser-journey-fixture.mjs";

const marker = "profile-marker-1234";
// A request success and its transaction completion are controlled independently.
function storageFixture({ abortWrite = false, holdWrite = false, holdOpen = false, absent = false } = {}) {
  const state = { cookie: "", local: new Map(), database: !absent, value: undefined,
    puts: 0, gets: 0, creates: 0, closes: 0, localWrites: 0, cookieWrites: 0, aborted: 0 };
  let releaseWrite, releaseOpen, deadline;
  const document = {
    get cookie() { return state.cookie; },
    set cookie(v) { state.cookieWrites++; state.cookie = v.split(";")[0]; },
  };
  const db = {
    close() { state.closes++; },
    createObjectStore(name) { assert.equal(name, "markers"); state.creates++; },
    transaction(name, mode) {
      assert.equal(name, "markers"); assert.ok(["readwrite", "readonly"].includes(mode));
      const tx = { aborted: false, abort() { this.aborted = true; state.aborted++; queueMicrotask(() => this.onabort?.()); },
        objectStore: () => ({
          put(value, key) {
            assert.equal(mode, "readwrite"); assert.equal(key, "value"); state.puts++;
            const request = {};
            queueMicrotask(() => {
              request.onsuccess();
              const finish = () => {
                if (tx.aborted) return;
                if (abortWrite) { tx.abort(); return; }
                state.value = value; tx.oncomplete();
              };
              if (holdWrite) releaseWrite = finish; else queueMicrotask(finish);
            });
            return request;
          },
          get(key) {
            assert.equal(mode, "readonly"); assert.equal(key, "value"); state.gets++;
            const request = {};
            queueMicrotask(() => { request.result = state.value; request.onsuccess(); queueMicrotask(() => tx.oncomplete()); });
            return request;
          },
        }) };
      return tx;
    },
  };
  const sandbox = { document,
    localStorage: { setItem(k, v) { state.localWrites++; state.local.set(k, v); }, getItem: k => state.local.get(k) ?? null },
    indexedDB: { open(name, version) {
      assert.equal(name, "elastos-browser-profile-probe-v1"); assert.equal(version, 1);
      const request = { result: db, transaction: { abort() { state.aborted++; request.failed = true; } } };
      const finish = () => {
        if (!state.database) {
          request.onupgradeneeded();
          if (request.failed) { request.onerror(); return; }
          state.database = true;
        }
        request.onsuccess();
      };
      if (holdOpen) releaseOpen = finish; else queueMicrotask(finish);
      return request;
    } },
    setTimeout(fn, ms) { assert.equal(ms, 5000); deadline = fn; return fn; },
    clearTimeout(fn) { if (deadline === fn) deadline = null; },
  };
  const execute = (mode, value = marker) => vm.runInNewContext(
    `(${browserProfileStorageProbe.toString()})(options)`, { ...sandbox, options: { mode, marker: value } });
  return { state, sandbox, execute, commit: () => releaseWrite(), open: () => releaseOpen(), timeout: () => deadline() };
}
const plain = value => JSON.parse(JSON.stringify(value));

test("write waits for IndexedDB transaction commit, then a fresh read observes the same marker without writes", async () => {
  const f = storageFixture({ holdWrite: true, absent: true });
  let settled = false;
  const writing = f.execute("write").then(v => { settled = true; return v; });
  await new Promise(setImmediate);
  assert.equal(settled, false); assert.equal(f.state.gets, 0); assert.equal(f.state.value, undefined);
  f.commit(); const write = await writing;
  assert.equal(write.ok, true); assert.equal(write.indexed_db.write_request_succeeded, true);
  assert.equal(write.indexed_db.write_committed, true); assert.equal(write.indexed_db.read_completed, true);
  assert.equal(f.state.closes, 1); assert.equal(f.state.creates, 1);
  const before = { puts: f.state.puts, cookieWrites: f.state.cookieWrites, localWrites: f.state.localWrites, creates: f.state.creates };
  const read = await f.execute("read");
  assert.equal(read.ok, true); assert.equal(read.mode, "read");
  assert.equal(read.indexed_db.write_request_succeeded, false); assert.equal(read.indexed_db.write_committed, false);
  assert.equal(read.indexed_db.read_request_succeeded, true); assert.equal(read.indexed_db.read_completed, true);
  assert.deepEqual(Object.fromEntries(Object.keys(before).map(k => [k, f.state[k]])), before);
  assert.equal(f.state.closes, 2);
});
test("a successful put followed by abort or timeout never becomes committed evidence", async () => {
  const aborted = storageFixture({ abortWrite: true });
  const result = await aborted.execute("write");
  assert.equal(result.ok, false); assert.equal(result.error, "transaction_aborted");
  assert.equal(result.indexed_db.write_request_succeeded, true); assert.equal(result.indexed_db.write_committed, false);
  assert.equal(aborted.state.gets, 0); assert.equal(aborted.state.closes, 1);
  const timed = storageFixture({ holdWrite: true }), pending = timed.execute("write");
  await new Promise(setImmediate); timed.timeout(); const timeout = await pending; timed.commit();
  assert.equal(timeout.error, "storage_timeout"); assert.equal(timeout.indexed_db.write_committed, false);
  assert.equal(timed.state.aborted, 1); assert.equal(timed.state.value, undefined);
});
test("read before initialization leaves storage absent, and late open after timeout closes without creating a database", async () => {
  const f = storageFixture({ absent: true }), read = await f.execute("read");
  assert.equal(read.ok, false); assert.equal(read.error, "database_open_failed");
  assert.equal(f.state.database, false); assert.equal(f.state.creates + f.state.puts + f.state.localWrites + f.state.cookieWrites, 0);
  const late = storageFixture({ holdOpen: true, absent: true }), pending = late.execute("read");
  await new Promise(setImmediate); late.timeout(); const value = await pending; late.open();
  assert.equal(value.error, "storage_timeout"); assert.equal(late.state.database, false);
  const existing = storageFixture({ holdOpen: true }), open = existing.execute("read");
  await new Promise(setImmediate); existing.timeout(); await open; existing.open();
  assert.equal(existing.state.closes, 1); assert.equal(existing.state.puts, 0);
});
test("a different marker is a measured mismatch and stored values or exception messages stay private", async () => {
  const f = storageFixture(); await f.execute("write", "other-private-value");
  const read = await f.execute("read");
  assert.equal(read.measurement_ok, true); assert.equal(read.ok, false);
  for (const part of [read.cookie, read.local_storage, read.indexed_db]) {
    assert.equal(part.present, true); assert.equal(part.matches, false);
  }
  assert.equal(JSON.stringify(read).includes("other-private-value"), false);
  f.sandbox.localStorage.getItem = () => { throw new Error("private-user-data"); };
  const failed = await f.execute("read"); assert.equal(failed.error, "storage_unavailable");
  assert.equal(JSON.stringify(failed).includes("private-user-data"), false);
  await assert.rejects(f.execute("write", "<script>"), /invalid profile probe/);
});
test("the actual served page reports commit then read evidence through the existing event endpoint", { timeout: 5000 }, async () => {
  const server = createBrowserJourneyFixture(); await new Promise(r => server.listen(0, "127.0.0.1", r));
  const base = "http://127.0.0.1:" + server.address().port, f = storageFixture({ absent: true });
  const element = { value: "", addEventListener() {}, getBoundingClientRect: () => ({ x: 0, y: 0, width: 100, height: 20 }) };
  f.sandbox.document.querySelector = () => element;
  try {
    for (const mode of [null, "write", "read"]) {
      const run = "served-page-" + (mode || "normal");
      const response = await fetch(base + `/main?run=${run}` + (mode ? `&profile=${mode}&marker=${marker}` : ""));
      const script = (await response.text()).match(/<script>([\s\S]*)<\/script>/)[1];
      let posted;
      const reported = new Promise(resolve => { posted = resolve; });
      new vm.Script(script).runInNewContext({ ...f.sandbox, scrollX: 0, scrollY: 0, addEventListener() {},
        requestAnimationFrame: fn => fn(), fetch: async (path, options) => {
          const result = await fetch(base + path, options);
          if (JSON.parse(options.body).type === (mode ? "profile_storage" : "load")) posted(result.status);
          return result;
        } });
      assert.equal(await reported, 200);
      const receipt = await fetch(base + "/receipt?run=" + run).then(r => r.json());
      if (!mode) {
        assert.equal(receipt.events.length, 1); assert.equal(receipt.profile_probe, undefined);
        assert.equal(f.state.cookieWrites + f.state.localWrites + f.state.puts + f.state.gets, 0);
      } else {
        const proof = receipt.events.find(e => e.type === "profile_storage").profile_storage;
        assert.equal(proof.mode, mode); assert.equal(proof.marker, marker); assert.equal(proof.ok, true);
        assert.equal(proof.indexed_db.write_committed, mode === "write");
        assert.equal(proof.indexed_db.read_completed, true);
      }
    }
    assert.equal(f.state.cookieWrites, 1); assert.equal(f.state.localWrites, 1); assert.equal(f.state.puts, 1);
    assert.equal(f.state.closes, 2);
  } finally { server.closeAllConnections(); await new Promise(r => server.close(r)); }
});
test("HTTP profile opt-in binds mode and marker per run, filters receipts and preserves normal fixture boundaries", async () => {
  const server = createBrowserJourneyFixture(); await new Promise(r => server.listen(0, "127.0.0.1", r));
  const base = "http://127.0.0.1:" + server.address().port;
  const event = { type: "profile_storage", page: "main", value: "", scroll_x: 0, scroll_y: 0,
    input_rect: { x: 0, y: 0, width: 100, height: 20 } };
  const post = (run, proof) => fetch(base + "/events?run=" + run, {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ ...event, profile_storage: proof }) });
  try {
    const normal = await fetch(base + "/main?run=normal-run-123"), html = await normal.text();
    assert.equal(normal.headers.get("access-control-allow-origin"), null);
    assert.doesNotMatch(html, /indexedDB|elastos-browser-profile-probe/);
    assert.match(html, /\/nav\?run=normal-run-123"/);
    for (const query of ["profile=read", "marker=" + marker, "profile=clear&marker=" + marker,
      "profile=write&marker=%3Cscript%3E", "profile=write&profile=read&marker=" + marker]) {
      assert.equal((await fetch(base + "/main?run=invalid-run-123&" + query)).status, 400);
    }
    for (const mode of ["write", "read"]) {
      const response = await fetch(base + `/main?run=${mode}-run-123&profile=${mode}&marker=${marker}`);
      const body = await response.text(); assert.equal(response.status, 200);
      new vm.Script(body.match(/<script>([\s\S]*)<\/script>/)[1]);
      assert.match(body, new RegExp(`/nav\\?run=${mode}-run-123&profile=${mode}&marker=${marker}`));
      assert.equal(response.headers.get("access-control-allow-origin"), null);
    }
    assert.equal((await fetch(base + `/main?run=write-run-123&profile=read&marker=${marker}`)).status, 409);
    assert.equal((await fetch(base + "/main?run=write-run-123&profile=write&marker=different-marker")).status, 409);
    assert.equal((await fetch(base + "/nav?run=write-run-123")).status, 409);
    const f = storageFixture(), write = plain(await f.execute("write")), read = plain(await f.execute("read"));
    assert.equal((await post("normal-run-123", write)).status, 400);
    assert.equal((await post("write-run-123", read)).status, 400);
    for (const mutate of [p => { p.marker = "different-marker"; }, p => { p.indexed_db.write_committed = false; },
      p => { p.indexed_db.write_request_succeeded = false; }, p => { p.indexed_db.read_completed = false; },
      p => { p.measurement_ok = false; }, p => { p.error = "private-error"; }]) {
      const proof = structuredClone(write); mutate(proof); assert.equal((await post("write-run-123", proof)).status, 400);
    }
    write.secret = "discard-me"; write.cookie.value = "private-cookie";
    assert.equal((await post("write-run-123", write)).status, 200);
    assert.equal((await post("read-run-123", read)).status, 200);
    const receipt = await fetch(base + "/receipt?run=write-run-123").then(r => r.json());
    assert.deepEqual(receipt.profile_probe, { mode: "write", marker });
    assert.equal(receipt.events[0].profile_storage.indexed_db.write_committed, true);
    assert.doesNotMatch(JSON.stringify(receipt), /discard-me|private-cookie/);
    assert.equal((await fetch(base + "/receipt?run=unseen-run-123")).status, 404);
    const preflight = await fetch(base + "/events?run=write-run-123", { method: "OPTIONS", headers: { Origin: "https://foreign.example" } });
    assert.equal(preflight.headers.get("access-control-allow-origin"), null);
    assert.equal(preflight.status, 404);
    const abort = plain(await storageFixture({ abortWrite: true }).execute("write"));
    assert.equal((await post("write-run-123", abort)).status, 200);
  } finally { server.closeAllConnections(); await new Promise(r => server.close(r)); }
});
