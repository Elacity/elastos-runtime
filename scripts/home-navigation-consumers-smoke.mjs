import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import { webcrypto } from "node:crypto";
import { createHomeNavigationClient } from "../capsules/home/browser/home-navigation-client.js";

export function navigationClientFixture({ homeToken, homeOrigin, parent, top, enabled = true }) {
  const listeners = new Map();
  const windowRef = { parent, top, crypto: webcrypto,
    addEventListener(type, callback) { listeners.set(type, callback); } };
  const client = createHomeNavigationClient({ homeToken, homeOrigin, enabled, windowRef });
  return { client, dispatch: (type, event = {}) => listeners.get(type)?.(event) };
}

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
export function selectionFunction(source, name) {
  const match = source.match(new RegExp(`^([ \\t]*)(?:async )?function ${name}\\(`, "m"));
  assert.ok(match, `missing selection function ${name}`);
  const end = source.indexOf(`\n${match[1]}}`, match.index);
  return source.slice(match.index, end + match[1].length + 2);
}

// Actual settled producers. The Home regression connects these to its real
// admission/relay/session fixture and fresh launch hook.
export async function exerciseNavigationConsumers(makeClient) {
  const reports = {};
  const client = (target) => {
    const transport = makeClient?.(target);
    return { setQuery(query) { reports[target] = JSON.parse(JSON.stringify(query)); transport?.setQuery(query); } };
  };
  const library = vm.createContext({
    state: { currentUri: "localhost://Users/fixture/Documents/B", loadSeq: 0, mode: "browse",
      folderCache: new Map(), selectedUris: new Set(), objects: [] }, perf: {},
    homeNavigation: client("library"),
    providerApi: async (_op, { uri }) => ({ object: { uri, kind: "directory" }, objects: [] }),
    cacheFolderListing: () => ({}), setObjects() {}, setStatus() {}, setFolderStatus() {}, renderAll() {}, runInitialObjectAction() {},
    isDirectory: object => object?.kind === "directory",
  });
  vm.runInContext(selectionFunction(read("capsules/library/browser/src/app.js"), "loadCurrentFolder"), library);
  await library.loadCurrentFolder();
  assert.deepEqual(reports.library, { uri: library.state.currentUri });
  const folderB = library.state.currentUri;
  let resolveOldList;
  const normalList = library.providerApi;
  library.providerApi = (_op, { uri }) => new Promise(resolve => { resolveOldList = () => resolve({ object: { uri, kind: "directory" }, objects: [] }); });
  library.state.currentUri = "localhost://Users/fixture/Documents/old";
  const oldList = library.loadCurrentFolder();
  library.providerApi = normalList;
  library.state.currentUri = folderB;
  await library.loadCurrentFolder();
  resolveOldList(); await oldList;
  library.providerApi = async () => { throw new Error("list failed"); };
  await assert.rejects(library.loadCurrentFolder(), /list failed/);
  library.providerApi = normalList;
  library.state.mode = "attach";
  library.state.currentUri = "localhost://Users/fixture/picker";
  await library.loadCurrentFolder();
  assert.deepEqual(reports.library, { uri: folderB }, "stale/failed/picker list changed settled folder");

  const archive = vm.createContext({
    homeToken: "fixture", objectUri: "localhost://Users/fixture/Documents/B.zip", archiveSelectionSequence: 0,
    homeNavigation: client("archive-manager"), state: { entries: [], selectedEntries: new Set(), policy: null },
    URL, window: { location: { origin: "https://home.fixture" } },
    fetch: async () => ({ ok: true, json: async () => ({ data: { object: { uri: archive.objectUri }, entries: [] } }) }),
    render() {}, renderEntries() {}, parseArchiveSupport: () => null,
    document: { querySelector: () => ({}) }, parentUri: () => "localhost://Users/fixture",
    showWorkspace() {}, async hydrateDestinationRoots() {}, announceHomeChrome() {},
  });
  const archiveSource = read("capsules/archive-manager/browser/index.html");
  vm.runInContext(["openLibraryObject", "hydrateStatOnly", "hydrateArchiveEntries"].map(name => selectionFunction(archiveSource, name)).join("\n"), archive);
  await archive.openLibraryObject({ uri: archive.objectUri });
  assert.deepEqual(reports["archive-manager"], { objectUri: archive.objectUri });
  const archiveB = archive.objectUri;
  let releaseArchive;
  const normalArchiveFetch = archive.fetch;
  archive.fetch = async () => { await new Promise(resolve => { releaseArchive = resolve; }); return { ok: true, json: async () => ({ data: { object: { uri: "localhost://Users/fixture/old.zip" }, entries: [] } }) }; };
  const oldArchive = archive.openLibraryObject({ uri: "localhost://Users/fixture/old.zip" });
  for (let turn = 0; turn < 20 && !releaseArchive; turn += 1) await Promise.resolve();
  assert.ok(releaseArchive, "Archive stat did not start");
  archive.fetch = normalArchiveFetch;
  await archive.openLibraryObject({ uri: archiveB });
  releaseArchive(); await oldArchive;
  archive.fetch = async () => ({ ok: false, json: async () => ({ error: "stat failed" }) });
  await assert.rejects(archive.openLibraryObject({ uri: "localhost://Users/fixture/fail.zip" }), /stat failed/);
  for (const failure of ["list", "malformed"]) {
    archive.fetch = async (url) => ({ ok: !url.includes("entries=true") || failure !== "list",
      json: async () => ({ data: { object: { uri: archive.objectUri }, ...(failure === "malformed" ? {} : { entries: [] }) } }) });
    await assert.rejects(archive.openLibraryObject({ uri: "localhost://Users/fixture/fail.zip" }), /could not be loaded/);
  }
  releaseArchive = null;
  archive.fetch = async (url) => {
    const uri = new URL(url, "https://home.fixture").searchParams.get("uri");
    if (url.includes("entries=true")) await new Promise(resolve => { releaseArchive = resolve; });
    return { ok: true, json: async () => ({ data: { object: { uri }, entries: [] } }) };
  };
  const oldEntries = archive.openLibraryObject({ uri: "localhost://Users/fixture/old.zip" });
  for (let turn = 0; turn < 20 && !releaseArchive; turn += 1) await Promise.resolve();
  assert.ok(releaseArchive, "Archive entries did not start");
  archive.fetch = normalArchiveFetch;
  await archive.openLibraryObject({ uri: archiveB });
  releaseArchive();
  assert.equal(await oldEntries, false, "Superseded Archive selection claimed acceptance");
  assert.deepEqual(reports["archive-manager"], { objectUri: archiveB });

  const gba = vm.createContext({ homeToken: "fixture", openSequence: 0, saveSession: null,
    homeNavigation: client("gba-emulator"),
    readGame: async () => ({ bytes: new Uint8Array([1]), fileName: "B.gba" }),
    loadEngine: async () => ({ FS: { writeFile() {}, unlink() {} }, loadGame: () => true,
      setVolume() {}, setFastForwardMultiplier() {}, pauseGame() {}, resumeGame() {} }),
    sha256Hex: async () => "rom-b", storageCapsuleForRequest: request => request.capsule,
    readStoredData: async () => ({ bytes: null }), saveUrl: () => "save", statePath: () => "state", stateUrl: () => "state",
    assertSaveSession() {}, persistSave() {},
    showStatus() {}, saveRecoveryButton: {}, emptyState: {}, powerLed: { classList: { remove() {} } },
    fastForwardButton: { classList: { remove() {} } }, document: {}, canvas: { focus() {} },
    setGameControlsEnabled() {}, syncPauseButton() {}, refreshStateSlots() {}, startSaveLifecycle() {}, startInputLoop() {},
  });
  vm.runInContext(selectionFunction(read("capsules/gba-emulator/browser/emulator.js"), "openGame"), gba);
  await gba.openGame({ capsule: "gba-nonogram" });
  assert.deepEqual(reports["gba-emulator"], { capsule: "gba-nonogram" });
  gba.persistSave = async () => {};
  const goodEngine = await gba.loadEngine();
  gba.loadEngine = async () => ({ ...goodEngine, loadGame: () => false });
  await assert.rejects(gba.openGame({ capsule: "rejected-game" }), /engine rejected/);
  assert.deepEqual(reports["gba-emulator"], { capsule: "gba-nonogram" });
  gba.loadEngine = async () => goodEngine;
  await gba.openGame({ objectUri: "localhost://Users/fixture/Games/My%2Fgame #1.gba" });
  assert.deepEqual(reports["gba-emulator"], { objectUri: "localhost://Users/fixture/Games/My%2Fgame #1.gba" });
  gba.saveSession.engine.pauseGame = () => {};
  await gba.openGame({ capsule: "gba-nonogram" });
  let releaseGame;
  const normalGameRead = gba.readGame;
  gba.readGame = async () => { await new Promise(resolve => { releaseGame = resolve; }); return { bytes: new Uint8Array([2]), fileName: "old.gba" }; };
  const oldGame = gba.openGame({ capsule: "old-game" });
  gba.readGame = normalGameRead;
  gba.saveSession.engine.pauseGame = () => {};
  await gba.openGame({ capsule: "gba-nonogram" });
  releaseGame(); await oldGame;
  gba.readGame = async () => { throw new Error("game denied"); };
  await assert.rejects(gba.openGame({ capsule: "unavailable" }), /game denied/);
  gba.readGame = normalGameRead;
  gba.saveSession.engine.pauseGame = () => {};
  gba.persistSave = async () => { throw new Error("save conflict"); };
  await assert.rejects(gba.openGame({ capsule: "conflict-game" }), /save conflict/);
  assert.deepEqual(reports["gba-emulator"], { capsule: "gba-nonogram" });

  // Rust owns the guarded Chat selection. This exact bridge consumes its JSON;
  // the native regression covers the producer with current/stale guards.
  const chatSource = read("capsules/chat-room/browser/index.html");
  const chat = vm.createContext({ globalThis: {}, homeNavigation: client("chat-room") });
  const bridge = chatSource.match(/globalThis\.elastosChatNavigation =[^;]+;/);
  assert.ok(bridge, "Chat must connect the actual Rust selector to Home navigation");
  vm.runInContext(bridge[0], chat);
  chat.globalThis.elastosChatNavigation({ conversation_id: "direct:sha256:conversation-b" });
  assert.deepEqual(reports["chat-room"], { conversation_id: "direct:sha256:conversation-b" });
  chat.globalThis.elastosChatNavigation({});
  assert.deepEqual(reports["chat-room"], {}, "explicit default retained direct selection");
  chat.globalThis.elastosChatNavigation({ conversation_id: "direct:sha256:conversation-b" });
  return { reports, library, archive, gba, chat };
}

if (process.argv[1] === new URL(import.meta.url).pathname) {
  await exerciseNavigationConsumers();
  console.log("[home-navigation-consumers] four settled selectors: PASS");
}
