import test from "node:test";
import assert from "node:assert/strict";
import { migrateLegacyWorkspaces, mergeWorkspaceDocuments } from "../capsules/assistant/browser/workspace-transition.js";

const cid = `bafybei${"a".repeat(52)}`;
function legacyFixture() {
  const text = "界".repeat(2730) + "ab"; // Exactly 8192 UTF-8 bytes.
  const assistant = { schema: "elastos.assistant.workspace/v1", revision: 4,
    selected_offer_id: "chosen", selected_model_cid: cid, draft: "Assistant draft",
    future: { secretToKeep: "unknown field" }, sessions: [{ id: "same", title: "A".repeat(160), mode: "build", pinned: true,
      messages: Array.from({ length: 64 }, (_, i) => ({ id: `m${i}`, role: ["user", "assistant", "system", "tool"][i % 4],
        content: text, run_id: `run-${i}`, futureMessage: i })) }] };
  const homeAgent = { schema: "elastos.home-agent.workspace/v1", revision: 8, document: {
    v: 1, activeSessionId: "same", liveOfferId: "chosen", selectedModelCid: cid,
    projects: [{ id: "project", title: "Project", extra: true }],
    composerDraft: { text: "Home Agent draft", parts: [{ id: "attachment", text, uri: "localhost://Users/user/notes", authority: "untrusted_content", extra: "retain" }] },
    sessions: [{ id: "same", projectId: "project", title: "Sash chat", archived: true, branchId: "branch", messages: [
      { id: "m", branchId: "branch", role: "user", text, parts: [{ uri: "localhost://notes", text, extra: true }] },
      { id: "m2", parentId: "m", branchId: "branch", role: "grant", summary: "Pending", args: { exact: true }, requestId: "original-grant-request" },
    ], lastTurn: { providerRunId: "original-run", createRequestId: "original-request", turnId: "turn", state: "settlement_unknown" } }],
  } };
  const homeSessionAgent = { v: 1, activeSessionId: "same", workbenchTab: "plan", plan: "Legacy plan", toolMode: "ask",
    projects: [{ id: "project", title: "Legacy project" }], composerDraft: { text: "Old Home draft", parts: [] },
    sessions: [{ id: "same", title: "Legacy chat", projectId: "project", forkedFrom: "parent", messages: [{ id: "m", role: "agent", text: "Earlier" }] }] };
  return { assistant, homeAgent, homeSessionAgent };
}

test("imports all stores, full history, unknown fields and each draft without mutating originals", () => {
  const legacy = legacyFixture();
  const before = structuredClone(legacy);
  const doc = migrateLegacyWorkspaces(legacy);
  assert.deepEqual(legacy, before);
  for (const source of Object.keys(legacy)) assert.deepEqual(doc.legacyImports[source].snapshot, legacy[source]);
  const assistant = doc.sessions.find(s => s.legacyOrigin.source === "assistant" && s.legacyOrigin.id === "same");
  assert.equal(assistant.messages.length, 64);
  assert.equal(Buffer.byteLength(assistant.messages[0].text), 8192);
  assert.equal(assistant.title.length, 160);
  assert.equal(assistant.messages[1].role, "agent");
  assert.equal(assistant.messages[1].legacyOrigin.role, "assistant");
  assert.equal(assistant.messages[2].role, "system");
  assert.equal(assistant.messages[3].role, "tool");
  assert.equal(assistant.messages[63].run_id, "run-63");
  assert.equal(assistant.messages[63].futureMessage, 63);
  const sash = doc.sessions.find(s => s.legacyOrigin.source === "homeAgent" && s.legacyOrigin.id === "same");
  assert.equal(sash.archived, true);
  assert.equal(sash.messages[1].role, "grant");
  assert.equal(sash.messages[1].requestId, "original-grant-request");
  assert.deepEqual(sash.lastTurn, { ...legacy.homeAgent.document.sessions[0].lastTurn, actorCapsule: "home-agent" });
  assert.equal(sash.messages[1].parentId, sash.messages[0].id);
  assert.equal(sash.messages[1].branchId, sash.branchId);
  assert.equal(doc.projects.find(p => p.id === sash.projectId).extra, true);
  assert.equal(new Set(doc.sessions.map(s => s.id)).size, doc.sessions.length);
  assert.equal(new Set(doc.projects.map(p => p.id)).size, doc.projects.length);
  const drafts = doc.sessions.filter(s => s.composerDraft);
  assert.deepEqual(drafts.map(s => s.composerDraft.text).sort(), ["Assistant draft", "Home Agent draft", "Old Home draft"].sort());
  assert.ok(drafts.every(s => !s.archived && s.pinned && !s.messages.length));
  assert.deepEqual(drafts.find(s => s.legacyOrigin.source === "homeAgent").composerDraft.parts, legacy.homeAgent.document.composerDraft.parts);
  assert.equal(doc.liveOfferId, "chosen");
  assert.equal(doc.selectedModelCid, cid);
});

test("repeat import is deterministic and does not restore deleted or edited imported work", () => {
  const legacy = legacyFixture();
  const doc = migrateLegacyWorkspaces(legacy);
  assert.deepEqual(migrateLegacyWorkspaces(legacy), doc);
  assert.deepEqual(migrateLegacyWorkspaces(legacy, doc), doc);
  doc.sessions.shift();
  doc.sessions[0].title = "Edited";
  const changedLegacy = structuredClone(legacy);
  changedLegacy.assistant.draft = "Changed elsewhere";
  assert.deepEqual(migrateLegacyWorkspaces(changedLegacy, doc), doc);
});

test("imported turns retain their original actor and exact provider identities", () => {
  const legacy = legacyFixture();
  const turn = { providerRunId: "old-run", createRequestId: "old-request", turnId: "old-turn", state: "streaming", custom: { kept: true } };
  for (const [source, doc] of [["assistant", legacy.assistant], ["homeAgent", legacy.homeAgent.document], ["homeSessionAgent", legacy.homeSessionAgent]]) {
    doc.sessions[0].lastTurn = structuredClone(turn);
    doc.sessions[0].messages[0].turn = structuredClone(turn);
  }
  const canonical = migrateLegacyWorkspaces(legacy);
  for (const source of Object.keys(legacy)) {
    const session = canonical.sessions.find(item => item.legacyOrigin.source === source && item.legacyOrigin.id === "same");
    const expected = { ...turn, state: "settlement_unknown", actorCapsule: source === "assistant" ? "assistant" : "home-agent" };
    assert.deepEqual(session.lastTurn, expected);
    assert.deepEqual(session.messages[0].turn, expected);
    assert.deepEqual(canonical.legacyImports[source].snapshot, legacy[source]);
  }
});

test("duplicate legacy IDs receive distinct deterministic IDs; providers keep exact identities", () => {
  const source = { v: 1, sessions: [
    { id: "duplicate", messages: [{ id: "m", role: "agent", text: "one", run_id: "run" }, { id: "m", role: "agent", text: "two" }] },
    { id: "duplicate", messages: [] },
  ] };
  const doc = migrateLegacyWorkspaces({ homeSessionAgent: source });
  assert.notEqual(doc.sessions[0].id, doc.sessions[1].id);
  assert.notEqual(doc.sessions[0].messages[0].id, doc.sessions[0].messages[1].id);
  assert.equal(doc.sessions[0].messages[0].run_id, "run");
});

test("source IDs preserve long and unusual identity without breaking quoted UI selectors", () => {
  const id = `quotes\"\\${"x".repeat(120)}\ud800`;
  const source = { v: 1, projects: [{ id, title: "Project" }], sessions: [{ id, projectId: id, messages: [] }] };
  const doc = migrateLegacyWorkspaces({ homeSessionAgent: source });
  assert.equal(doc.sessions[0].legacyOrigin.id, id);
  assert.equal(doc.sessions[0].projectId, doc.projects[0].id);
  assert.doesNotMatch(doc.projects[0].id, /["\\]/);
  assert.ok(doc.sessions[0].id.length > 120);
});

test("three-way merge retains disjoint session, nested setting, and new-record changes", () => {
  const base = { v: 1, settings: { a: 1, b: 2 }, sessions: [{ id: "s", title: "Old", pinned: false, messages: [] }], projects: [] };
  const local = structuredClone(base), remote = structuredClone(base);
  local.settings.a = 3; local.sessions[0].title = "Local title";
  remote.settings.b = 4; remote.sessions[0].pinned = true; remote.sessions.push({ id: "remote", messages: [] });
  const result = mergeWorkspaceDocuments(base, local, remote);
  assert.equal(result.requiresReview, false);
  assert.deepEqual(result.document.settings, { a: 3, b: 4 });
  assert.equal(result.document.sessions[0].title, "Local title");
  assert.equal(result.document.sessions[0].pinned, true);
  assert.equal(result.document.sessions.length, 2);
  assert.equal(base.sessions[0].title, "Old");
});

test("same-session conflict keeps both complete histories in visible deterministic copies", () => {
  const base = { activeSessionId: "s", sessions: [{ id: "s", title: "Chat", messages: [{ id: "m", role: "agent", text: "base" }] }], projects: [] };
  const local = structuredClone(base), remote = structuredClone(base);
  local.sessions[0].messages.push({ id: "l", role: "user", text: "local" });
  local.sessions[0].lastTurn = { providerRunId: "keep-run", createRequestId: "keep-request" };
  remote.sessions[0].messages.push({ id: "r", role: "user", text: "remote" });
  const result = mergeWorkspaceDocuments(base, local, remote);
  assert.equal(result.requiresReview, true);
  assert.deepEqual(result.document.sessions.find(s => s.id === "s"), remote.sessions[0]);
  const recovered = result.document.sessions.find(s => s.id !== "s");
  assert.deepEqual(recovered.messages, local.sessions[0].messages);
  assert.deepEqual(recovered.lastTurn, { ...local.sessions[0].lastTurn, attachmentAllowed: false });
  assert.equal(result.document.activeSessionId, recovered.id);
  assert.equal(recovered.archived, false);
  assert.match(recovered.title, /recovered changes/);
  assert.deepEqual(mergeWorkspaceDocuments(base, local, remote), result);
  assert.deepEqual(mergeWorkspaceDocuments(base, result.document, remote).document, result.document);
});

test("delete/edit conflicts preserve edited history; uncontested deletion stays deleted", () => {
  const base = { sessions: [{ id: "s", title: "Chat", messages: [] }], projects: [] };
  const edited = structuredClone(base); edited.sessions[0].messages.push({ text: "Saved work" });
  const deleted = { sessions: [], projects: [] };
  assert.equal(mergeWorkspaceDocuments(base, deleted, edited).document.sessions[0].messages[0].text, "Saved work");
  assert.equal(mergeWorkspaceDocuments(base, edited, deleted).document.sessions[0].messages[0].text, "Saved work");
  assert.deepEqual(mergeWorkspaceDocuments(base, deleted, base).document.sessions, []);
});

test("conflicting workspace drafts retain both values and a visible recovery entry", () => {
  const base = { composerDraft: { text: "base", parts: [] }, sessions: [], projects: [] };
  const local = structuredClone(base), remote = structuredClone(base);
  local.composerDraft.text = "my unsaved draft";
  remote.composerDraft.text = "other saved draft";
  const result = mergeWorkspaceDocuments(base, local, remote);
  assert.equal(result.requiresReview, true);
  assert.equal(result.document.composerDraft.text, "my unsaved draft");
  assert.equal(result.conflicts[0].remote, "other saved draft");
  assert.match(result.document.sessions[0].messages[0].text, /other saved draft/);
  assert.deepEqual(result.document.sessions[0].composerDraft, remote.composerDraft);
  assert.equal(result.document.sessions[0].archived, false);
});

test("duplicate canonical record IDs fail explicitly instead of silently dropping a record", () => {
  assert.throws(() => mergeWorkspaceDocuments({}, { sessions: [{ id: "same" }, { id: "same" }] }, {}), /Duplicate sessions IDs/);
});
