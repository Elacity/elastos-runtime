import assert from "node:assert/strict";
import { createHash, webcrypto } from "node:crypto";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../capsules/gba-emulator/browser/emulator.js", import.meta.url), "utf8")
  .replace(/^import .*\n/gm, "").split("async function loadInstalledGames()")[0];
const revision = (bytes) => `"${createHash("sha256").update(bytes).digest("hex")}"`;
const deferred = () => { let resolve; const promise = new Promise((done) => { resolve = done; }); return { promise, resolve }; };

function storage() {
  const files = new Map();
  const requests = [];
  const fetch = async (url, options = {}) => {
    requests.push({ url, ...options });
    const old = files.get(url);
    if (options.method !== "PUT") return old
      ? new Response(old, { headers: { etag: revision(old) } }) : new Response(null, { status: 404 });
    if ((options.headers?.["If-None-Match"] === "*" && old)
      || (options.headers?.["If-Match"] && (!old || options.headers["If-Match"] !== revision(old)))) {
      return new Response(null, { status: 412 });
    }
    files.set(url, new Uint8Array(options.body));
    return new Response(null, { status: 204, headers: { etag: revision(options.body) } });
  };
  return { files, requests, fetch };
}

function player(store) {
  const elements = new Map();
  const timers = new Map();
  let timerId = 0;
  const element = (id) => {
    if (!elements.has(id)) elements.set(id, { dataset: {}, classList: { toggle() {}, remove() {}, add() {} },
      querySelector() { return this; }, setAttribute() {}, focus() {} });
    return elements.get(id);
  };
  const game = { save: new Uint8Array([1]), loads: [], states: new Map(),
    getSave() { return this.save; }, paused: false, pauseGame() { this.paused = true; }, resumeGame() { this.paused = false; }, setVolume() {}, setFastForwardMultiplier() {},
    loadGame(path) { this.loads.push(path); return true; }, saveState() { return true; }, loadState() { return true; },
  };
  game.FS = { writeFile: (path, bytes) => game.states.set(path, new Uint8Array(bytes)),
    readFile: (path) => game.states.get(path), unlink: (path) => game.states.delete(path), analyzePath: () => ({ exists: false }) };
  const window = { location: { search: "", hash: "#home_token=fixture" },
    setInterval(fn) { timers.set(++timerId, fn); return timerId; }, clearInterval(id) { timers.delete(id); },
    setTimeout, clearTimeout, confirm: () => false };
  window.self = window.top = window;
  const context = vm.createContext({ window, document: { body: { dataset: {} }, getElementById: element },
    createHomeNavigationClient: () => ({ setQuery() {} }),
    URLSearchParams, Uint8Array, crypto: webcrypto, console, fetch: (...args) => store.fetch(...args), game });
  vm.runInContext(source + `
    loadEngine = async () => (engine = game);
    readGame = async (request) => ({ bytes: new Uint8Array([request.rom || 1]), fileName: "game.gba" });
    startInputLoop = () => {};
    globalThis.api = { openGame, persistSave, saveState, loadState, refreshStateSlots, recoverSave };
  `, context);
  return { ...context.api, game, context, window, timers, elements };
}

test("two windows cannot overwrite the same missing save; loser keeps its game and stops autosave", async () => {
  const store = storage();
  const first = player(store), second = player(store);
  await first.openGame({ capsule: "game" });
  await second.openGame({ capsule: "game" });
  first.game.save = new Uint8Array([7]);
  second.game.save = new Uint8Array([9]);
  await first.persistSave();
  await assert.rejects(second.persistSave(), /changed|another window/i);
  assert.deepEqual([...store.files.values()][0], new Uint8Array([7]));
  assert.deepEqual(second.game.save, new Uint8Array([9]));
  assert.equal(second.timers.size, 0);
  const puts = store.requests.filter((request) => request.method === "PUT");
  assert.equal(puts[0].headers["If-None-Match"], "*");
  await assert.rejects(second.persistSave());
  assert.equal(store.requests.filter((request) => request.method === "PUT").length, 2);
});

test("existing save revision survives restart; stale writer cannot replace winner", async () => {
  const store = storage();
  const first = player(store);
  await first.openGame({ capsule: "game" });
  await first.persistSave();
  const restarted = player(store);
  await restarted.openGame({ capsule: "game" });
  restarted.game.save = new Uint8Array([2]);
  await restarted.persistSave();
  first.game.save = new Uint8Array([3]);
  await assert.rejects(first.persistSave());
  assert.deepEqual([...store.files.values()][0], new Uint8Array([2]));
  assert.ok(store.requests.filter((request) => request.method === "PUT")[1].headers["If-Match"]);
});

test("failed initial read is not absence and preserves the current live game", async () => {
  const store = storage();
  const app = player(store);
  await app.openGame({ capsule: "game" });
  const loads = app.game.loads.length;
  const fetch = store.fetch;
  store.fetch = (url, options) => url.includes("/other/") ? Promise.resolve(new Response(null, { status: 503 })) : fetch(url, options);
  await assert.rejects(app.openGame({ capsule: "other", rom: 2 }));
  assert.equal(app.game.loads.length, loads);
  assert.equal(vm.runInContext("gameLoaded", app.context), true);
  assert.equal(store.requests.filter((request) => request.method === "PUT" && request.url.includes("/other/")).length, 0);
});

test("lost PUT reply stops blind retries while preserving the acknowledged-or-uncertain stored bytes", async () => {
  const store = storage();
  const app = player(store);
  await app.openGame({ capsule: "game" });
  const fetch = store.fetch;
  store.fetch = async (url, options) => { const response = await fetch(url, options); if (options?.method === "PUT") throw new Error("connection lost"); return response; };
  await assert.rejects(app.persistSave());
  await assert.rejects(app.persistSave());
  assert.equal(store.requests.filter((request) => request.method === "PUT").length, 1);
  assert.equal(app.timers.size, 0);
  assert.deepEqual(app.game.save, new Uint8Array([1]));
});

test("overlapping saves use the confirmed revision and capture newer live progress", async () => {
  const store = storage(), app = player(store);
  await app.openGame({ capsule: "game" });
  const release = deferred(), sent = deferred(), fetch = store.fetch;
  store.fetch = async (url, options) => {
    const response = await fetch(url, options);
    if (options?.method === "PUT" && !sent.done) { sent.done = true; sent.resolve(); await release.promise; }
    return response;
  };
  const first = app.persistSave();
  await sent.promise;
  app.game.save = new Uint8Array([2]);
  const second = app.persistSave();
  assert.equal(store.requests.filter((request) => request.method === "PUT").length, 1);
  release.resolve();
  await Promise.all([first, second]);
  const puts = store.requests.filter((request) => request.method === "PUT");
  assert.equal(puts.length, 2);
  assert.equal(puts[1].headers["If-Match"], revision(new Uint8Array([1])));
  assert.deepEqual([...store.files.values()][0], new Uint8Array([2]));
});

test("unresolved save blocks another ROM without replacing the live game", async () => {
  const store = storage(), app = player(store), winner = player(store);
  await app.openGame({ capsule: "game" });
  await winner.openGame({ capsule: "game" });
  await winner.persistSave();
  app.game.save = new Uint8Array([3]);
  await assert.rejects(app.openGame({ capsule: "other", rom: 2 }));
  assert.equal(app.game.loads.length, 1);
  assert.deepEqual(app.game.save, new Uint8Array([3]));
  await assert.rejects(app.openGame({ capsule: "other", rom: 2 }));
  assert.equal(app.game.loads.length, 1);
});

test("reopening the same ROM reads the final saved revision, not its earlier snapshot", async () => {
  const store = storage(), app = player(store);
  await app.openGame({ capsule: "game" });
  await app.persistSave();
  app.game.save = new Uint8Array([4]);
  await app.openGame({ capsule: "game" });
  assert.deepEqual(vm.runInContext("saveSession.save.bytes", app.context), new Uint8Array([4]));
  app.game.save = new Uint8Array([5]);
  await app.persistSave();
  assert.deepEqual([...store.files.values()][0], new Uint8Array([5]));
});

test("a slower previous ROM read cannot replace the newer selected game", async () => {
  const store = storage(), app = player(store), held = deferred(), reached = deferred(), fetch = store.fetch;
  store.fetch = async (url, options) => {
    if (url.includes("/slow/")) { reached.resolve(); await held.promise; }
    return fetch(url, options);
  };
  const slow = app.openGame({ capsule: "slow", rom: 1 });
  await reached.promise;
  await app.openGame({ capsule: "newer", rom: 2 });
  held.resolve();
  await slow;
  assert.equal(app.game.loads.length, 1);
  assert.equal(vm.runInContext("activeStorageCapsule", app.context), "newer");
});

test("state conflicts preserve winner bytes and the losing live game", async () => {
  const store = storage(), first = player(store), second = player(store);
  for (const app of [first, second]) {
    await app.openGame({ capsule: "game" });
    app.game.saveState = (slot) => { app.game.states.set(vm.runInContext(`saveSession.statePaths[${slot}]`, app.context), app.game.save); return true; };
  }
  first.game.save = new Uint8Array([7]);
  second.game.save = new Uint8Array([9]);
  await first.saveState(1);
  await assert.rejects(second.saveState(1), /changed/i);
  assert.deepEqual([...store.files.values()][0], new Uint8Array([7]));
  assert.deepEqual(second.game.save, new Uint8Array([9]));
  assert.equal(second.timers.size, 0);
});

test("failed state read cannot become a create precondition", async () => {
  const store = storage(), app = player(store), fetch = store.fetch;
  store.fetch = (url, options) => url.includes("/state/") ? Promise.resolve(new Response(null, { status: 503 })) : fetch(url, options);
  await app.openGame({ capsule: "game" });
  await assert.rejects(app.saveState(1), /read/i);
  assert.equal(store.requests.filter((request) => request.method === "PUT").length, 0);
  assert.equal(app.elements.get("slot-status1").textContent, "Unavailable");
});

test("malformed GET and successful-PUT revisions fail closed", async () => {
  const store = storage(), app = player(store);
  const fetch = store.fetch;
  store.fetch = async (url, options) => options?.method === "PUT"
    ? new Response(null, { status: 204 }) : fetch(url, options);
  await app.openGame({ capsule: "game" });
  await assert.rejects(app.persistSave(), /uncertain/i);
  assert.equal(app.timers.size, 0);
  store.fetch = async () => new Response(new Uint8Array([1]), { headers: { etag: "W/weak" } });
  await assert.rejects(player(store).openGame({ capsule: "game" }), /revision/i);
});

test("uncertain committed save reconciles exact bytes before newer progress resumes", async () => {
  const store = storage(), app = player(store), fetch = store.fetch;
  await app.openGame({ capsule: "game" });
  store.fetch = async (url, options) => { const result = await fetch(url, options); if (options?.method === "PUT") throw new Error("lost reply"); return result; };
  await assert.rejects(app.persistSave());
  app.game.save = new Uint8Array([9]);
  store.fetch = fetch;
  await app.recoverSave();
  assert.deepEqual(app.game.save, new Uint8Array([9]));
  assert.equal(store.requests.filter((request) => request.method === "PUT").length, 1);
  assert.equal(app.timers.size, 1);
  await app.persistSave();
  assert.deepEqual([...store.files.values()][0], new Uint8Array([9]));
});

test("uncertain uncommitted save reconciles its original absence before a conditional retry", async () => {
  const store = storage(), app = player(store), fetch = store.fetch;
  await app.openGame({ capsule: "game" });
  store.fetch = async (url, options) => { if (options?.method === "PUT") throw new Error("not sent"); return fetch(url, options); };
  await assert.rejects(app.persistSave());
  store.fetch = fetch;
  app.game.save = new Uint8Array([8]);
  await app.recoverSave();
  await app.persistSave();
  const put = store.requests.find((request) => request.method === "PUT");
  assert.equal(put.headers["If-None-Match"], "*");
  assert.deepEqual([...store.files.values()][0], new Uint8Array([8]));
});

test("conflict recovery Cancel preserves live progress and stored winner; explicit consent loads stored save", async () => {
  const store = storage(), app = player(store), winner = player(store);
  await app.openGame({ capsule: "game" });
  await winner.openGame({ capsule: "game" });
  winner.game.save = new Uint8Array([7]);
  await winner.persistSave();
  app.game.save = new Uint8Array([9]);
  await assert.rejects(app.persistSave());
  await app.recoverSave();
  assert.deepEqual(app.game.save, new Uint8Array([9]));
  assert.equal(app.game.loads.length, 1);
  assert.equal(app.timers.size, 0);
  app.window.confirm = () => true;
  await app.recoverSave();
  assert.equal(app.game.loads.length, 2);
  const savePath = vm.runInContext("saveSession.savePath", app.context);
  assert.deepEqual(app.game.states.get(savePath), new Uint8Array([7]));
  assert.equal(app.timers.size, 1);
  assert.equal(store.requests.filter((request) => request.method === "PUT").length, 2);
  assert.deepEqual([...store.files.values()][0], new Uint8Array([7]));
});

test("reconciliation read failure cannot authorize retry or discard", async () => {
  const store = storage(), app = player(store);
  await app.openGame({ capsule: "game" });
  store.fetch = async () => { throw new Error("offline"); };
  await assert.rejects(app.persistSave());
  await app.recoverSave();
  assert.equal(app.timers.size, 0);
  assert.equal(app.game.loads.length, 1);
  await assert.rejects(app.persistSave());
});

test("reconciliation reply from a retired window session cannot resume saves or load a game", async () => {
  const store = storage(), app = player(store), fetch = store.fetch;
  await app.openGame({ capsule: "game" });
  store.fetch = async () => { throw new Error("offline"); };
  await assert.rejects(app.persistSave());
  const held = deferred(), reached = deferred();
  store.fetch = async (...args) => { reached.resolve(); await held.promise; return fetch(...args); };
  const recovery = app.recoverSave();
  await reached.promise;
  vm.runInContext("saveSession = null", app.context);
  held.resolve();
  await recovery;
  assert.equal(app.timers.size, 0);
  assert.equal(app.game.loads.length, 1);
});

test("superseded switch resumes the live engine when newer ROM loading fails", async () => {
  const store = storage(), app = player(store), fetch = store.fetch;
  await app.openGame({ capsule: "game" });
  const held = deferred(), reached = deferred();
  store.fetch = async (url, options) => {
    if (options?.method === "PUT") { reached.resolve(); await held.promise; }
    if (url.includes("/failed/")) throw new Error("ROM unavailable");
    return fetch(url, options);
  };
  const first = app.openGame({ capsule: "first", rom: 2 });
  await reached.promise;
  assert.equal(app.game.paused, true);
  await assert.rejects(app.openGame({ capsule: "failed", rom: 3 }));
  held.resolve();
  await first;
  assert.equal(app.game.paused, false);
  assert.equal(app.game.loads.length, 1);
});

test("late state read for a retired game never loads into a new game", async () => {
  const store = storage(), app = player(store);
  await app.openGame({ capsule: "game" });
  let loads = 0;
  app.game.loadState = () => { loads += 1; return true; };
  const held = deferred(), reached = deferred();
  store.fetch = async () => { reached.resolve(); await held.promise; return new Response(new Uint8Array([7]), { headers: { etag: revision(new Uint8Array([7])) } }); };
  const loading = app.loadState(1);
  await reached.promise;
  vm.runInContext("saveSession = null", app.context);
  held.resolve();
  await assert.rejects(loading, /game changed/i);
  assert.equal(loads, 0);
});
