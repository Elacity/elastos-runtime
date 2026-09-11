import assert from "node:assert/strict";
import { File } from "node:buffer";
import { createHash, webcrypto } from "node:crypto";
import test from "node:test";
import vm from "node:vm";
import { browserFileInputProbe, createBrowserJourneyFixture } from "./browser-journey-fixture.mjs";

const bytes = Uint8Array.from({ length: 65536 }, (_, i) => i % 251);
const expected = createHash("sha256").update(bytes).digest("hex");
const secret = "private-filename-and-path";
const plain = value => JSON.parse(JSON.stringify(value));
const options = { expected_sha256: expected };
const execute = (input, globals = {}) => vm.runInNewContext(`(${browserFileInputProbe.toString()})(input, options)`, {
  input, options, crypto: webcrypto, isSecureContext: true, performance, setTimeout, clearTimeout, ...globals,
});

test("one actual 64KiB File is read and hashed; an equal-size one-byte mutation fails the match", async () => {
  const good = await execute({ files: [new File([bytes], secret)] });
  assert.equal(good.size_bytes, 65536); assert.equal(good.sha256, expected);
  assert.equal(good.read_completed, true); assert.equal(good.hash_completed, true);
  assert.equal(good.measurement_ok, true); assert.equal(good.matches, true); assert.equal(good.ok, true); assert.equal(good.error, null);
  assert.ok(!JSON.stringify(good).includes(secret));
  const changed = bytes.slice(); changed[changed.length - 1] ^= 1;
  const mismatch = await execute({ files: [new File([changed], secret)] });
  assert.equal(mismatch.measurement_ok, true); assert.equal(mismatch.size_bytes, 65536);
  assert.notEqual(mismatch.sha256, expected); assert.equal(mismatch.matches, false); assert.equal(mismatch.ok, false);
});

test("selection and size are checked before reading or hashing; errors and metadata stay private", async () => {
  let reads = 0, hashes = 0;
  const file = size => ({ size, get name() { throw new Error(secret); }, get path() { throw new Error(secret); },
    arrayBuffer: async () => { reads++; return bytes.buffer; } });
  const crypto = { subtle: { digest: async () => { hashes++; return new ArrayBuffer(32); } } };
  for (const files of [[], [file(65536), file(65536)], [file(0)], [file(65535)], [file(65537)], [file(2 ** 40)]]) {
    const result = await execute({ files }, { crypto });
    assert.equal(result.ok, false); assert.equal(result.read_completed, false); assert.equal(result.sha256, null);
  }
  assert.equal(reads, 0); assert.equal(hashes, 0);
  const insecure = await execute({ files: [file(65536)] }, { crypto, isSecureContext: false });
  assert.equal(insecure.error, "hash_unavailable"); assert.equal(reads, 0);
  const readError = await execute({ files: [{ size: 65536, arrayBuffer: async () => { throw new Error(secret); } }] });
  assert.equal(readError.error, "read_failed"); assert.ok(!JSON.stringify(readError).includes(secret));
  const hashError = await execute({ files: [file(65536)] }, { crypto: { subtle: { digest: async () => { throw new Error(secret); } } } });
  assert.equal(hashError.error, "hash_failed"); assert.equal(hashError.read_completed, true);
  assert.equal(hashError.hash_completed, false); assert.ok(!JSON.stringify(hashError).includes(secret));
  const truncated = await execute({ files: [{ size: 65536, arrayBuffer: async () => new ArrayBuffer(1) }] }, { crypto });
  assert.equal(truncated.error, "invalid_size"); assert.equal(hashes, 0);
});

test("the single five-second deadline ignores late reads/digests and delayed timer dispatch", async () => {
  for (const stage of ["read", "digest", "late-clock"]) {
    let release, clock = 0, hashes = 0;
    const timers = new Set();
    const held = new Promise(resolve => { release = resolve; });
    const resultPromise = execute({ files: [{ size: 65536, arrayBuffer: () => stage === "read" || stage === "late-clock" ? held : Promise.resolve(bytes.buffer) }] }, {
      performance: { now: () => clock },
      setTimeout: (callback, ms) => { assert.equal(ms, 5000); timers.add(callback); return callback; },
      clearTimeout: callback => timers.delete(callback),
      crypto: { subtle: { digest: () => { hashes++; return stage === "digest" ? held : Promise.resolve(new ArrayBuffer(32)); } } },
    });
    await new Promise(setImmediate);
    clock = 5000;
    if (stage === "late-clock") release(bytes.buffer);
    else for (const callback of timers) callback();
    const result = await resultPromise, snapshot = plain(result);
    assert.equal(result.error, "file_timeout"); assert.equal(result.ok, false); assert.equal(result.hash_completed, false);
    assert.equal(result.read_completed, stage === "digest"); assert.equal(timers.size, 0);
    release(stage === "digest" ? new ArrayBuffer(32) : bytes.buffer);
    await new Promise(setImmediate);
    assert.deepEqual(plain(result), snapshot); assert.equal(hashes, stage === "digest" ? 1 : 0);
  }
});

async function fixture(t) {
  const server = createBrowserJourneyFixture();
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  t.after(async () => { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); });
  return `http://127.0.0.1:${server.address().port}`;
}
const event = measurement => ({ type: "file_input", page: "main", value: "", scroll_x: 0, scroll_y: 0,
  input_rect: { x: 32, y: 150, width: 480, height: 60 }, file_input: measurement });

test("served opt-in page handles input/change only once and sends only hash/size evidence", async t => {
  const base = await fixture(t), run = "file-page-run";
  const html = await fetch(`${base}/main?run=${run}&file=upload&sha256=${expected}`).then(r => r.text());
  assert.match(html, /<input id="journey-file" type="file">/);
  assert.ok(html.includes(`/nav?run=${run}&file=upload&sha256=${expected}`));
  const listeners = new Map(); let reads = 0;
  const inputFile = { files: [{ size: 65536, arrayBuffer: async () => { reads++; return bytes.buffer; } }],
    addEventListener: (kind, listener) => listeners.set(kind, listener), removeEventListener: kind => listeners.delete(kind) };
  const textInput = { value: "", getBoundingClientRect: () => event(null).input_rect, addEventListener: () => {} };
  const output = { textContent: "" };
  const sent = [];
  const context = vm.createContext({ crypto: webcrypto, isSecureContext: true, performance, setTimeout, clearTimeout,
    document: { querySelector: selector => ({ "#journey-input": textInput, "#journey-file": inputFile, "#journey-observation": output })[selector] },
    scrollX: 0, scrollY: 0, addEventListener: () => {}, requestAnimationFrame: fn => fn(),
    fetch: async (url, config) => { sent.push(JSON.parse(config.body)); return fetch(base + url, config); },
  });
  vm.runInContext(html.match(/<script>([\s\S]*)<\/script>/)[1], context);
  const input = listeners.get("input"), change = listeners.get("change");
  input(); change(); input();
  for (let i = 0; i < 100 && !sent.some(e => e.type === "file_input"); i++) await new Promise(setImmediate);
  await vm.runInContext("pending", context);
  assert.equal(reads, 1); assert.equal(listeners.size, 0);
  assert.equal(sent.filter(e => e.type === "file_input").length, 1);
  const receipt = await fetch(`${base}/receipt?run=${run}`).then(r => r.json());
  assert.equal(receipt.events.find(e => e.type === "file_input").file_input.sha256, expected);
  assert.ok(!JSON.stringify(receipt).includes(secret)); assert.match(output.textContent, /matches/);
});

test("file metadata is opt-in and immutable; receipt projection rejects false success and keeps existing bounds", async t => {
  const base = await fixture(t), run = "file-http-run";
  const path = `/main?run=${run}&file=upload&sha256=${expected}`;
  const normal = await fetch(`${base}/main?run=normal-file-run`).then(r => r.text());
  assert.ok(!normal.includes('id="journey-file"')); assert.ok(!normal.includes("browserFileInputProbe"));
  for (const suffix of ["&file=upload", `&sha256=${expected}`, "&file=download&sha256=x", `&file=upload&sha256=${expected.toUpperCase()}`,
    `&file=upload&file=upload&sha256=${expected}`, `&file=upload&sha256=${expected}&sha256=${expected}`]) {
    assert.equal((await fetch(`${base}/main?run=invalid-file-run${suffix}`)).status, 400);
  }
  assert.equal((await fetch(base + path)).status, 200);
  for (const suffix of ["", `&file=upload&sha256=${"0".repeat(64)}`]) {
    assert.equal((await fetch(`${base}/nav?run=${run}${suffix}`)).status, 409);
  }
  assert.equal((await fetch(`${base}/nav?run=${run}&file=upload&sha256=${expected}`)).status, 200);
  const good = await execute({ files: [new File([bytes], secret)] });
  const post = (measurement, id = run) => fetch(`${base}/events?run=${id}`, {
    method: "POST", headers: { "content-type": "application/json", origin: "https://foreign.invalid" },
    body: JSON.stringify(event(measurement)),
  });
  assert.equal((await post(good, "normal-file-run")).status, 400);
  for (const change of [{ expected_sha256: "0".repeat(64) }, { size_bytes: 65535 }, { read_completed: false },
    { hash_completed: false }, { measurement_ok: false }, { matches: false }, { sha256: null }, { elapsed_ms: 5000 },
    { error: secret }, { ok: false }]) assert.equal((await post({ ...good, ...change })).status, 400);
  const response = await post({ ...good, bytes: secret, name: secret, path: secret });
  assert.equal(response.status, 200); assert.equal(response.headers.get("access-control-allow-origin"), null);
  const changed = bytes.slice(); changed[0] ^= 1;
  assert.equal((await post(await execute({ files: [new File([changed], secret)] }))).status, 200);
  assert.equal((await post({ ...good, bytes: "x".repeat(5000) })).status, 413);
  const receipt = await fetch(`${base}/receipt?run=${run}`).then(r => r.json());
  assert.deepEqual(receipt.file_probe, { mode: "upload", expected_sha256: expected });
  assert.equal(receipt.events.length, 2); assert.equal(receipt.events[1].file_input.ok, false);
  assert.ok(!JSON.stringify(receipt).includes(secret));
  const normalReceipt = await fetch(`${base}/receipt?run=normal-file-run`).then(r => r.json());
  assert.equal(normalReceipt.file_probe, undefined);
});
