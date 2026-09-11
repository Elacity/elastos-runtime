#!/usr/bin/env node
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { once } from "node:events";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { extname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium, brave } from "./system-uiux-fixture.mjs";

// Render the real canonical capsule. Only Runtime storage/provider responses
// are fixtures; migration, session selection and save recovery use the client.
const root = resolve(fileURLToPath(new URL("../", import.meta.url)));
const schema = "elastos.assistant.workspace/v2";
const workspacePath = "/api/apps/assistant/workspace-v2";
const cid = `bafybei${"a".repeat(52)}`;
const offerId = "offer:qwen-installed-fixture";
const messages = Array.from({ length: 64 }, (_, index) => ({
  id: `message-${index}`, role: index % 2 ? "assistant" : "user",
  content: `Message ${index}: ${"世界🙂 café ".repeat(700)} END-${index}`,
  request_id: `retained-request-${index}`, futureMetadata: { original: index },
}));
assert.ok(Buffer.byteLength(messages[0].content) > 8192);
const legacy = {
  assistant: {
    schema: "elastos.assistant.workspace/v1", revision: 7,
    sessions: [{ id: "collision", title: "Assistant complete history", messages }],
    draft: "Assistant draft: mañana 世界", selected_offer_id: offerId, selected_model_cid: cid,
  },
  homeAgent: {
    schema: "elastos.home-agent.workspace/v1", revision: 11,
    document: {
      v: 1, activeSessionId: "collision", sessionMode: "chat",
      liveOfferId: offerId, selectedModelCid: cid,
      sessions: [{ id: "collision", title: "Sash imported conversation", messages: [
        { id: "message-0", role: "user", text: "Sash history survives" },
        { id: "grant-0", role: "grant", text: "Historical grant record survives", decision: "approved" },
        { id: "tool-0", role: "tool", text: "Historical tool result survives" },
        { id: "system-0", role: "system", text: "Historical system context survives" },
      ] }],
      composerDraft: { text: "Home Agent draft: editable 🧭", parts: [] },
    },
  },
  homeSessionAgent: {
    v: 1, activeSessionId: "collision",
    sessions: [{ id: "collision", title: "Home session imported conversation", messages: [
      { id: "message-0", role: "user", text: "Third store history survives" },
    ] }],
    composerDraft: { text: "Home session draft: preserved 🌱", parts: [] },
  },
};
const initial = { schema, revision: 0, document: {}, legacy, migration_revision: `sha256:${"1".repeat(64)}` };
let stored = null;
let conflicts = 0;
const calls = [], errors = [], pageErrors = [];
const clone = value => structuredClone(value);
const server = createServer(async (req, res) => {
  const url = new URL(req.url, "http://fixture");
  const headers = {
    "access-control-allow-origin": "null",
    "access-control-allow-headers": "content-type,x-elastos-home-token",
    "access-control-allow-methods": "GET,POST,PUT,OPTIONS",
    "cache-control": "no-store",
  };
  const json = (value, status = 200) => {
    res.writeHead(status, { ...headers, "content-type": "application/json" });
    res.end(JSON.stringify(value));
  };
  try {
    if (req.method === "OPTIONS") { res.writeHead(204, headers); res.end(); return; }
    if (url.pathname === "/") {
      const origin = `http://127.0.0.1:${server.address().port}`;
      res.writeHead(200, { "content-type": "text/html" });
      res.end(`<!doctype html><style>html,body,iframe{width:100%;height:100%;margin:0;border:0}</style>
        <iframe title="Canonical Assistant" sandbox="allow-scripts allow-forms allow-modals" src="/apps/assistant/?home_origin=${encodeURIComponent(origin)}#home_token=fixture"></iframe>
        <script>addEventListener('message',event=>{if(event.data?.type==='home-agent:ready'){
          event.source.postMessage({type:'home-agent:open'},'*');
          event.source.postMessage({type:'home-agent:shelf-handover',on:true},'*');}});</script>`);
      return;
    }
    if (url.pathname.startsWith("/api/")) {
      assert.equal(req.headers["x-elastos-home-token"], "fixture");
      const chunks = []; for await (const chunk of req) chunks.push(chunk);
      const body = Buffer.concat(chunks).toString("utf8");
      const input = body ? JSON.parse(body) : null;
      calls.push({ method: req.method, path: url.pathname, revision: input?.if_revision });
      if (url.pathname === workspacePath) {
        if (req.method === "GET") { json(stored || initial); return; }
        assert.equal(req.method, "PUT");
        assert.equal(input.schema, schema);
        if (input.if_revision !== (stored?.revision ?? 0)) {
          conflicts++; json({ error: "workspace revision conflict" }, 409); return;
        }
        if (!stored) assert.equal(input.migration_revision, initial.migration_revision);
        stored = { schema, revision: (stored?.revision ?? 0) + 1, document: clone(input.document) };
        json(stored); return;
      }
      if (url.pathname === "/api/provider/model/offers_list") {
        json({ offers: [{ id: offerId, title: "Qwen installed fixture", operation: "text.generate",
          input_modalities: ["text/plain"], output_modalities: ["text/plain"] }] }); return;
      }
      if (url.pathname === "/api/capsules/catalog") {
        json({ schema: "elastos.capsules.catalog/v1", model_catalog_state: "verified", capsules: [{
          source: "signed-model-catalog", role: "content", title: "Qwen installed fixture", cid,
          installed: false, launchable: false, signature_state: "catalog-signature-verified",
          model_runtime: { admitted: true, dispatch_ready: true, offer_id: offerId },
        }] }); return;
      }
      throw new Error(`Unexpected API operation: ${req.method} ${url.pathname}`);
    }
    if (url.pathname === "/favicon.ico") { res.writeHead(204); res.end(); return; }
    const match = url.pathname.match(/^\/apps\/(assistant|home)\/(.*)$/);
    assert.ok(match, `Unexpected asset: ${url.pathname}`);
    const base = resolve(root, "capsules", match[1], "browser");
    const path = resolve(base, match[2] || "index.html");
    assert.ok(path.startsWith(`${base}/`));
    const bytes = await readFile(path);
    res.writeHead(200, { ...headers, "content-type": ({ ".html": "text/html", ".js": "text/javascript",
      ".mjs": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".woff2": "font/woff2" })[extname(path)] || "application/octet-stream" });
    res.end(bytes);
  } catch (error) { errors.push(String(error)); json({ error: "fixture failure" }, 500); }
});

async function until(predicate, label) {
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline) {
    if (predicate()) return;
    await new Promise(resolve => setTimeout(resolve, 25));
  }
  throw new Error(`Timed out: ${label}`);
}
function assertComplete(document) {
  assert.equal(new Set(document.sessions.map(item => item.id)).size, document.sessions.length);
  const imported = document.sessions.find(item => item.title === "Assistant complete history");
  assert.equal(imported.messages.length, 64);
  imported.messages.forEach((message, index) => {
    assert.ok(message.text === messages[index].content, `Exact text changed at message ${index}`);
    assert.ok(message.content === messages[index].content, `Exact content changed at message ${index}`);
    assert.equal(message.request_id, messages[index].request_id);
    assert.deepEqual(message.futureMetadata, messages[index].futureMetadata);
  });
  for (const [source, snapshot] of Object.entries(legacy)) {
    assert.deepEqual(document.legacyImports[source].snapshot, snapshot);
  }
  assert.equal(document.liveOfferId, offerId);
  assert.equal(document.selectedModelCid, cid);
}

let browser, page, stage = "launch";
const ownedProof = !process.env.ASSISTANT_CONVERGENCE_PROOF_DIR;
const proofDir = ownedProof
  ? await mkdtemp(resolve(tmpdir(), "assistant-convergence-"))
  : resolve(process.env.ASSISTANT_CONVERGENCE_PROOF_DIR);
await mkdir(proofDir, { recursive: true });
try {
  server.listen(0, "127.0.0.1"); await once(server, "listening");
  browser = await chromium.launch({ executablePath: brave, headless: true });
  const openPage = async () => {
    const next = await browser.newPage({ viewport: { width: 1280, height: 900 } });
    next.on("pageerror", error => pageErrors.push(String(error)));
    await next.goto(`http://127.0.0.1:${server.address().port}/`);
    await next.frameLocator("iframe").locator("#agent-composer-input").waitFor({ state: "visible" });
    return next;
  };
  page = await openPage();
  let frame = page.frameLocator("iframe");
  const choose = async (target, title) => {
    await target.locator(".agent-harness-session-btn").filter({ hasText: title }).click();
  };
  stage = "migration and one sidebar";
  await until(() => stored?.document?.legacyImports, "canonical migration save");
  assertComplete(stored.document);
  assert.equal(await frame.locator("#agent-harness-sidebar").count(), 1);
  assert.equal(await frame.locator("#assistant-sidebar").count(), 0);
  for (const title of ["Assistant complete history", "Sash imported conversation", "Home session imported conversation",
    "Assistant saved draft", "Home Agent saved draft", "Home saved draft"]) {
    assert.equal(await frame.locator(".agent-harness-session-btn").filter({ hasText: title }).count(), 1, title);
  }
  stage = "complete rendered history";
  await choose(frame, "Assistant complete history");
  await frame.locator("#agent-harness-stream-column").getByText("END-63", {exact:false}).first().waitFor({state:"attached"});
  const transcript = await frame.locator("#agent-harness-stream-column").textContent();
  for (let index = 0; index < 64; index++) assert.ok(transcript.includes(`END-${index}`), `Missing rendered message ${index}`);
  await choose(frame, "Sash imported conversation");
  for (const role of ["grant", "tool", "system"]) {
    await frame.locator(`#agent-harness-stream-column .agent-msg-${role}`).getByText(`Historical ${role}`, {exact:false}).waitFor({state:"attached"});
  }
  stage = "editable imported drafts";
  for (const [title, text] of [["Assistant saved draft", legacy.assistant.draft],
    ["Home saved draft", legacy.homeSessionAgent.composerDraft.text],
    ["Home Agent saved draft", legacy.homeAgent.document.composerDraft.text]]) {
    await choose(frame, title);
    assert.equal(await frame.locator("#agent-composer-input").inputValue(), text, title);
  }
  const edited = "Edited imported draft: café 世界 🧭";
  await frame.locator("#agent-composer-input").fill(edited);
  await until(() => stored.document.composerDraft?.text === edited, "edited draft persisted");
  stage = "reload and exact selected model";
  await page.reload(); frame = page.frameLocator("iframe");
  await frame.locator("#agent-composer-input").waitFor({ state: "visible" });
  assert.equal(await frame.locator("#agent-composer-input").inputValue(), edited);
  await frame.locator("#agent-model-picker").click();
  await frame.locator(`#agent-model-menu [data-model-cid="${cid}"][aria-selected="true"]`).waitFor({ state: "visible" });
  assert.equal(await frame.locator("#agent-model-menu").getByRole("option", { name: "Qwen installed fixture", exact: true }).count(), 1);
  await frame.locator("#agent-model-picker").click();
  stage = "Chat Build Studio controls";
  for (const mode of ["build", "studio", "chat"]) {
    const button = frame.locator(`button[data-assistant-mode="${mode}"]`);
    await button.click();
    assert.equal(await button.getAttribute("aria-pressed"), "true");
    assert.equal(await frame.locator("#assistant-studio").isVisible(), mode === "studio");
  }
  // Let preceding real UI writes settle, then open a second stale writer.
  await until(() => stored.document.sessionMode === "chat", "mode save");
  const second = await openPage();
  const secondFrame = second.frameLocator("iframe");
  page = second; // Capture the active conflict UI if this phase fails.
  stage = "two-page CAS draft conflict";
  const firstDraft = "Window one keeps this draft 🪟";
  const secondDraft = "Window two keeps this draft 🌍";
  await frame.locator("#agent-composer-input").fill(firstDraft);
  await until(() => stored.document.composerDraft?.text === firstDraft, "first window save");
  await secondFrame.locator("#agent-composer-input").fill(secondDraft);
  await until(() => conflicts > 0 && stored.document.composerDraft?.text === secondDraft
    && stored.document.sessions.some(item => item.composerDraft?.text === firstDraft), "both conflicting drafts saved");
  const recovered = stored.document.sessions.find(item => item.composerDraft?.text === firstDraft && item.recoveryIdentity);
  assert.ok(recovered, "Conflict must have a visible recovery entry");
  await choose(secondFrame, recovered.title);
  assert.equal(await secondFrame.locator("#agent-composer-input").inputValue(), firstDraft);
  assertComplete(stored.document);
  assert.equal(calls.filter(call => call.path.includes("/runs_")).length, 0, "Opening, migration, modes and recovery must not invoke runs");
  assert.deepEqual(errors, []);
  assert.deepEqual(pageErrors, []);
  console.log("PASS canonical Assistant rendered migration, complete history, editable drafts, model selection, modes and two-page CAS recovery");
  if (ownedProof) await rm(proofDir, { recursive: true, force: true });
} catch (error) {
  const report = { stage, error: String(error.stack || error), calls, errors, pageErrors, conflicts, revision: stored?.revision };
  await writeFile(resolve(proofDir, "first-failure.json"), JSON.stringify(report, null, 2));
  if (page) await page.screenshot({ path: resolve(proofDir, "first-failure.png"), fullPage: true }).catch(() => {});
  console.error(`Assistant convergence failed at ${stage}. Proof: ${proofDir}`);
  throw error;
} finally {
  await browser?.close();
  server.closeAllConnections();
  if (server.listening) await new Promise(resolve => server.close(resolve));
}
