#!/usr/bin/env node
// When the session ends without asking (expired, refused, locked), Home's
// host rescues unsaved Documents text before it blanks the desktop, keeps it
// in memory for the same person, and offers it back after sign-in.

import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import vm from "node:vm";

const hostSource = fs.readFileSync(new URL("../capsules/home/browser/home-shell-host.js", import.meta.url), "utf8");

function extractFunction(source, name) {
  const start = [`async function ${name}(`, `function ${name}(`]
    .map((marker) => source.indexOf(marker)).filter((index) => index >= 0).sort((a, b) => a - b)[0];
  assert.notEqual(start, undefined, `${name} function not found`);
  let depth = 0;
  let close = -1;
  for (let index = source.indexOf("(", start); index < source.length; index += 1) {
    if (source[index] === "(") depth += 1;
    if (source[index] === ")") depth -= 1;
    if (depth === 0) { close = index; break; }
  }
  const open = source.indexOf("{", close);
  depth = 0;
  for (let index = open; index < source.length; index += 1) {
    if (source[index] === "{") depth += 1;
    if (source[index] === "}") depth -= 1;
    if (depth === 0) return source.slice(start, index + 1);
  }
  throw new Error(`${name} is not balanced`);
}

const hostFunctions = [
  "hasExactMessageKeys", "homeSummarySignedIn", "currentHomePrincipalId", "validDocumentsDraft",
  "askDocumentsWindow", "acceptDocumentsDraftReply", "collectDocumentsDrafts", "rescueDocumentsDrafts",
  "restoreDocumentsDrafts", "showHostAuthGate",
].map((name) => extractFunction(hostSource, name)).join("\n");

const draft = (overrides = {}) => ({
  title: "Beta", body: "# Beta\n\nUnsaved words", doc_did: "doc-beta", library_object_uri: "",
  revision: "a".repeat(64), file_name: "beta.md", ...overrides,
});

function host({ principal = "person:one", windows = [] } = {}) {
  let serial = 0;
  const events = [];
  const delivered = [];
  const context = vm.createContext({
    OPAQUE_FRAME_TARGET: "*",
    DOCUMENTS_DRAFT_COLLECT_MS: 30,
    DOCUMENTS_DRAFT_RESTORE_MS: 200,
    MAX_RESCUED_DRAFT_CHARS: 1024,
    pendingDocumentsDraftRequests: new Map(),
    rescuedDocumentsDrafts: null,
    documentsDraftRescue: null,
    launchedAppContexts: new Map(),
    shellState: { currentSummary: { authority: { signed_in: true }, browser_state: { principal_id: principal } } },
    window: { setTimeout, clearTimeout, crypto: { randomUUID: () => `request-${++serial}` } },
    enterHostAuthGate() {
      events.push({ gate: true, held: JSON.parse(JSON.stringify(context.rescuedDocumentsDrafts)) });
      context.launchedAppContexts.clear();
    },
    currentSignedProfileDisplayName: () => "",
    showHomeUnlock: () => Promise.resolve(),
    hideHostBootMask() {},
    boot: async () => {},
    profileReadinessActionTarget: () => "",
    activateDesktopShell: async () => {},
    openTargetFromHomeGui: async () => {},
    restoreReply: true,
    async openHomeGuiTargetWithPayload(target, payload) {
      delivered.push({ target, payload });
      const documentsWindow = { homeToken: "restored-window", targetId: "documents", source: {} };
      context.launchedAppContexts.set(documentsWindow.homeToken, documentsWindow);
      setTimeout(() => context.acceptDocumentsDraftReply(
        { kind: "app-frame", targetId: "documents", homeToken: documentsWindow.homeToken },
        { type: "documents:draft-restored", requestId: payload.requestId, homeToken: documentsWindow.homeToken,
          ok: context.restoreReply },
      ), 1);
      return true;
    },
  });
  vm.runInContext(hostFunctions, context);
  for (const [index, { reply, targetId = "documents" }] of windows.entries()) {
    const homeToken = `window-${index}`;
    context.launchedAppContexts.set(homeToken, {
      homeToken, targetId, origin: "null",
      source: {
        postMessage(message, origin) {
          assert.equal(origin, "*");
          if (reply === undefined) return; // A window that never answers.
          setTimeout(() => context.acceptDocumentsDraftReply(
            { kind: "app-frame", targetId, homeToken },
            { type: "documents:draft", requestId: message.requestId, homeToken, draft: reply },
          ), 1);
        },
      },
    });
  }
  return { context, events, delivered };
}

test("an unasked session end rescues unsaved Documents text before it blanks the desktop", async () => {
  const { context, events } = host({ windows: [
    { reply: draft() },
    { reply: null },
    { reply: undefined },
    { reply: { ...draft(), extra: "field" } },
    { reply: draft({ body: "x".repeat(2048) }) },
    { reply: draft({ title: "Not Documents" }), targetId: "library" },
  ] });
  await context.showHostAuthGate({ presentation: "prompt" });
  assert.equal(events.length, 1);
  assert.deepEqual(events[0].held, { principalId: "person:one", drafts: [draft()] },
    "only the valid unsaved Documents draft is held when the desktop blanks");
  assert.equal(context.pendingDocumentsDraftRequests.size, 0, "no request is left waiting");
});

test("rescued text comes back to the same person, one draft at a time, then is let go", async () => {
  const { context, delivered } = host({ windows: [{ reply: draft() }, { reply: draft({ doc_did: "", title: "New" }) }] });
  await context.showHostAuthGate({ presentation: "prompt" });
  await context.restoreDocumentsDrafts();
  assert.deepEqual(delivered.map(({ target, payload }) => [target, payload.type, payload.draft.title]),
    [["documents", "documents:restore-draft", "Beta"], ["documents", "documents:restore-draft", "New"]]);
  assert.equal(context.rescuedDocumentsDrafts, null);
});

test("a failed restore keeps the text for the next try", async () => {
  const { context, delivered } = host({ windows: [{ reply: draft() }] });
  await context.showHostAuthGate({ presentation: "prompt" });
  context.restoreReply = false;
  await context.restoreDocumentsDrafts();
  assert.equal(delivered.length, 1);
  assert.deepEqual(JSON.parse(JSON.stringify(context.rescuedDocumentsDrafts.drafts)), [draft()]);
  context.restoreReply = true;
  await context.restoreDocumentsDrafts();
  assert.equal(context.rescuedDocumentsDrafts, null);
});

test("another person signing in never sees the rescued text", async () => {
  const { context, delivered } = host({ windows: [{ reply: draft() }] });
  await context.showHostAuthGate({ presentation: "prompt" });
  context.shellState.currentSummary = { authority: { signed_in: true }, browser_state: { principal_id: "person:two" } };
  await context.restoreDocumentsDrafts();
  assert.equal(delivered.length, 0);
  assert.equal(context.rescuedDocumentsDrafts, null);
});
