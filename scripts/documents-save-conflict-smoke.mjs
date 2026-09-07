import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../capsules/documents/browser/index.html", import.meta.url), "utf8");
const functionSource = (name) => {
  const start = source.search(new RegExp(`(?:async )?function ${name}\\(`));
  assert.ok(start >= 0, `missing ${name}`);
  return source.slice(start, source.indexOf("\n}", start) + 2);
};
const deferred = () => { let resolve; const promise = new Promise((done) => { resolve = done; }); return { promise, resolve }; };
const version = (document) => createHash("sha256").update(JSON.stringify([document.doc_did, document.title, document.body])).digest("hex");

function backend() {
  const requests = [], docs = new Map();
  let creates = 0;
  const put = (document) => { const value = { ...document, revision: version(document) }; docs.set(value.doc_did, value); return { ...value }; };
  put({ doc_did: "did:one", title: "Initial", body: "Body", file_name: "one.md" });
  const request = async (op, payload) => {
    requests.push({ op, ...payload });
    if (op === "get") return { document: { ...docs.get(payload.doc_did) } };
    if (op === "create") return { document: put({ doc_did: `did:created-${++creates}`, title: payload.title || "Untitled", body: "" }) };
    if (op === "save") {
      const old = docs.get(payload.doc_did);
      if (!payload.if_revision || payload.if_revision !== old.revision) throw Object.assign(new Error("Document changed in another window."), { code: "revision_conflict" });
      return { document: put({ ...old, title: payload.title.trim() || "Untitled", body: payload.body }) };
    }
    if (op === "save_as") {
      if ([...docs.values()].some((doc) => doc.file_name === payload.file_name)) throw new Error("destination already exists");
      return { document: put({ doc_did: `did:created-${++creates}`, title: payload.title, body: payload.body, file_name: payload.file_name }) };
    }
    throw new Error(`unexpected ${op}`);
  };
  return { docs, requests, request, put };
}

function editor(server, current = server.docs.get("did:one")) {
  const context = vm.createContext({
    state: { current: { ...current }, currentSessionId: 1, dirty: true, documents: [], mode: "shell", homeToken: "fixture-token" },
    elements: { titleInput: { value: current.title }, editor: { value: current.body } },
    saveInFlight: null, autosaveQueued: false, pendingHomeWindowCloseTarget: null,
    saveRecovery: null, documentSelectionSequence: 0, queuedSaveTarget: null, savedWorkingCopy: { ...current },
    isLibraryFileProjection: () => false, isDraftDocument: (doc) => doc.doc_did.startsWith("draft:"),
    documentIdentity: (doc) => doc.doc_did,
    documentsProviderApi: (...args) => server.request(...args),
    clearAutosaveTimer() {}, setStatus() {}, clearStatus() {}, scheduleStatusClear() {},
    renderDocumentsList() {}, renderCurrentDocument() { context.state.dirty = false; context.elements.titleInput.value = context.state.current.title; context.elements.editor.value = context.state.current.body; }, refreshPreviewFromEditor() {}, renderHistory() {},
    upsertDocumentListItem() {}, reportSaveFailure() {}, scheduleAutosave() {},
    setDirty(value) { context.state.dirty = value; },
    confirmInCapsule: async () => false,
    chooseInCapsule: async () => "cancel",
  });
  vm.runInContext(["isCurrentSessionTarget", "isCurrentDocumentTarget", "resetPendingSaveIntent", "replaceCurrentDocument", "applySavedDocumentState", "requireSavedWorkingCopy", "saveCurrent", "selectDocument", "saveAsCurrent"].map(functionSource).join("\n"), context);
  return context;
}

test("two windows cannot overwrite a changed title or body", async () => {
  for (const field of ["title", "body"]) {
    const server = backend(), first = editor(server), second = editor(server);
    first.elements[field === "title" ? "titleInput" : "editor"].value = "Winner";
    await first.saveCurrent();
    second.elements.editor.value = "Loser";
    await assert.rejects(second.saveCurrent());
    assert.equal(server.docs.get("did:one")[field], "Winner");
    assert.equal(second.elements.editor.value, "Loser");
    assert.equal(second.state.dirty, true);
  }
});

test("lost first save reply retains the exact created document and never repeats create", async () => {
  const server = backend(), app = editor(server, { doc_did: "draft:one", title: "Draft", body: "Keep me" });
  const request = server.request;
  server.request = async (op, payload) => { const result = await request(op, payload); if (op === "save") throw new Error("reply lost"); return result; };
  await assert.rejects(app.saveCurrent());
  assert.equal(app.state.current.doc_did, "did:created-1");
  await assert.rejects(app.saveCurrent({ autosave: true }));
  assert.equal(server.requests.filter((value) => value.op === "create").length, 1);
  assert.equal(app.elements.editor.value, "Keep me");
});

test("a stale GET cannot replace a newer document selection", async () => {
  const server = backend(), app = editor(server), held = deferred();
  app.state.dirty = false;
  server.put({ doc_did: "did:two", title: "Second", body: "Two" });
  server.put({ doc_did: "did:three", title: "Third", body: "Three" });
  const request = server.request;
  server.request = async (op, payload) => { const result = await request(op, payload); if (payload.doc_did === "did:two") await held.promise; return result; };
  const first = app.selectDocument("did:two");
  await app.selectDocument("did:three");
  held.resolve();
  await first;
  assert.equal(app.state.current.doc_did, "did:three");
});

test("unknown create pauses autosave and explicit copy warns of duplicate risk", async () => {
  const server = backend(), app = editor(server, { doc_did: "draft:one", title: "Draft", body: "Keep me" });
  const request = server.request;
  server.request = async (op, payload) => { const result = await request(op, payload); if (op === "create") throw new Error("create reply lost"); return result; };
  await assert.rejects(app.saveCurrent());
  assert.equal(app.state.current.doc_did, "draft:one");
  await assert.rejects(app.saveCurrent({ autosave: true }));
  assert.equal(server.requests.filter(({ op }) => op === "create").length, 1);
  app.chooseInCapsule = async (options) => { assert.match(options.message, /duplicate/); return "secondary"; };
  server.request = request;
  await app.saveCurrent();
  assert.equal(app.state.current.doc_did, "did:created-2");
  assert.equal(server.docs.get("did:created-2").body, "Keep me");
});

test("unknown create can refresh the list without assigning an unobserved ID", async () => {
  const server = backend(), app = editor(server, { doc_did: "draft:one", title: "Draft", body: "Keep me" });
  const request = server.request;
  server.request = async (op, payload) => {
    if (op === "summary") return { documents: [...server.docs.values()] };
    const result = await request(op, payload);
    if (op === "create") throw new Error("create reply lost");
    return result;
  };
  await assert.rejects(app.saveCurrent());
  app.chooseInCapsule = async () => "confirm";
  await assert.rejects(app.saveCurrent(), /Select the stored document/);
  assert.equal(app.state.current.doc_did, "draft:one");
  assert.equal(app.elements.editor.value, "Keep me");
  assert.ok(app.state.documents.some(({ doc_did }) => doc_did === "did:created-1"));
  assert.equal(server.requests.filter(({ op }) => op === "create").length, 1);
});

test("lost save reply reconciles exact saved state and keeps newer edits", async () => {
  const server = backend(), app = editor(server);
  app.elements.editor.value = "Sent body";
  const request = server.request;
  server.request = async (op, payload) => { const result = await request(op, payload); if (op === "save") throw new Error("save reply lost"); return result; };
  await assert.rejects(app.saveCurrent());
  app.elements.editor.value = "Newer local body";
  server.request = request;
  await app.saveCurrent();
  assert.equal(server.requests.filter(({ op }) => op === "save").length, 1);
  assert.equal(app.elements.editor.value, "Newer local body");
  assert.equal(app.state.dirty, true);
  assert.equal(app.savedWorkingCopy.body, "Sent body");
  await app.saveCurrent();
  assert.equal(server.docs.get("did:one").body, "Newer local body");
});

test("unchanged GET permits only explicit conditional retry; mixed outcome preserves draft", async () => {
  for (const mixed of [false, true]) {
    const server = backend(), app = editor(server), request = server.request;
    app.elements.titleInput.value = "New title";
    app.elements.editor.value = "New body";
    server.request = async (op, payload) => {
      if (op === "save") {
        if (mixed) server.put({ ...server.docs.get("did:one"), body: payload.body });
        throw new Error("write outcome unknown");
      }
      return request(op, payload);
    };
    await assert.rejects(app.saveCurrent());
    server.request = request;
    await assert.rejects(app.saveCurrent({ autosave: true }));
    if (mixed) {
      await assert.rejects(app.saveCurrent(), /stored document changed/);
      assert.equal(server.docs.get("did:one").title, "Initial");
      assert.equal(app.elements.titleInput.value, "New title");
      assert.equal(app.state.dirty, true);
    } else {
      await app.saveCurrent();
      assert.equal(server.docs.get("did:one").title, "New title");
    }
  }
});

test("Rust trim/default title contract also reconciles lost replies and long titles", async () => {
  const rust = readFileSync(new URL("../elastos/crates/elastos-server/src/documents.rs", import.meta.url), "utf8");
  assert.match(rust, /DOCUMENTS_DEFAULT_TITLE: &str = "Untitled"/);
  assert.match(rust, /fn documents_normalize_title[\s\S]*?\.map\(str::trim\)[\s\S]*?\.unwrap_or\(default_title\)[\s\S]*?\.to_string\(\)/);
  for (const title of ["", "   ", "  Trim me  ", "Long".repeat(40)]) {
    const server = backend(), app = editor(server), request = server.request;
    app.elements.titleInput.value = title;
    server.request = async (op, payload) => { const result = await request(op, payload); if (op === "save") throw new Error("lost"); return result; };
    await assert.rejects(app.saveCurrent());
    server.request = request;
    await app.saveCurrent();
    assert.equal(app.state.dirty, false);
    assert.equal(app.state.current.title, title.trim() || "Untitled");
    assert.equal(server.requests.filter(({ op }) => op === "save").length, 1);
  }
});

test("late create reply cannot bind or save over a newer selection", async () => {
  const server = backend(), app = editor(server, { doc_did: "draft:one", title: "Draft", body: "Draft body" }), held = deferred();
  const request = server.request;
  server.request = async (op, payload) => { const result = await request(op, payload); if (op === "create") await held.promise; return result; };
  const saving = app.saveCurrent();
  app.replaceCurrentDocument({ ...server.docs.get("did:one") });
  app.elements.titleInput.value = "New selected draft";
  app.elements.editor.value = "Keep selected";
  held.resolve();
  await saving;
  assert.equal(app.state.current.doc_did, "did:one");
  assert.equal(app.elements.editor.value, "Keep selected");
  assert.equal(server.requests.filter(({ op }) => op === "save").length, 0);
});

test("selection GET keeps edits made while awaiting replies", async () => {
  const server = backend(), app = editor(server), held = deferred(), request = server.request;
  server.put({ doc_did: "did:two", title: "Two", body: "Two" });
  app.state.dirty = false;
  server.request = async (op, payload) => { const result = await request(op, payload); if (op === "get") await held.promise; return result; };
  const selecting = app.selectDocument("did:two");
  app.elements.editor.value = "Edited during selection";
  app.state.dirty = true;
  held.resolve();
  await selecting;
  assert.equal(app.state.current.doc_did, "did:one");
  assert.equal(app.elements.editor.value, "Edited during selection");
});

test("late save recovery GET cannot replace or write into a newer selection", async () => {
  const server = backend(), app = editor(server), request = server.request, held = deferred();
  server.request = async (op, payload) => { if (op === "save") throw new Error("unknown"); return request(op, payload); };
  await assert.rejects(app.saveCurrent());
  server.request = async (op, payload) => { const result = await request(op, payload); if (op === "get") await held.promise; return result; };
  const recovering = app.saveCurrent();
  const next = server.put({ doc_did: "did:two", title: "Two", body: "Two" });
  app.replaceCurrentDocument(next);
  app.elements.editor.value = "New document edits";
  held.resolve();
  await recovering;
  assert.equal(app.state.current.doc_did, "did:two");
  assert.equal(app.elements.editor.value, "New document edits");
  assert.equal(server.requests.filter(({ op }) => op === "save").length, 0);
});

test("Save As collision preserves existing bytes and delayed copy preserves newer edits", async () => {
  const server = backend(), app = editor(server), request = server.request, held = deferred();
  app.elements.editor.value = "Draft copy";
  await assert.rejects(app.saveAsCurrent("Copy", "one.md"), /already exists/);
  assert.equal(server.docs.get("did:one").body, "Body");
  assert.equal(app.elements.editor.value, "Draft copy");
  server.request = async (op, payload) => { const result = await request(op, payload); if (op === "save_as") await held.promise; return result; };
  const saving = app.saveAsCurrent("Copy", "copy.md");
  app.elements.editor.value = "Newer than copy";
  held.resolve();
  await saving;
  assert.equal(app.state.current.doc_did, "did:one");
  assert.equal(app.elements.editor.value, "Newer than copy");
  assert.equal(server.docs.get("did:created-1").body, "Draft copy");
});

test("queued saves remain bound to the selected session", async () => {
  const server = backend(), app = editor(server), held = deferred(), request = server.request;
  server.put({ doc_did: "did:two", title: "Two", body: "Two" });
  server.put({ doc_did: "did:three", title: "Three", body: "Three" });
  server.request = async (op, payload) => { const result = await request(op, payload); if (op === "save" && payload.doc_did === "did:one") await held.promise; return result; };
  const saving = app.saveCurrent();
  app.replaceCurrentDocument({ ...server.docs.get("did:two") });
  app.elements.titleInput.value = "Two";
  app.elements.editor.value = "Queued two";
  const queued = app.saveCurrent();
  app.replaceCurrentDocument({ ...server.docs.get("did:three") });
  app.elements.titleInput.value = "Three";
  app.elements.editor.value = "Keep three";
  held.resolve();
  await Promise.all([saving, queued]);
  assert.equal(server.requests.filter(({ op }) => op === "save").length, 1);
  assert.equal(app.state.current.doc_did, "did:three");
  assert.equal(app.elements.editor.value, "Keep three");
});
