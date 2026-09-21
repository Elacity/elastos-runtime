#!/usr/bin/env node
/* Canonical Assistant capsule gate.

   Encodes what the capsule owes the Runtime and Home GUI, through these contracts:
   - inference runs only through the typed model contract (offers_list,
     runs_create with a typed input, runs_events by after_sequence, runs_cancel);
     no offer is named in source, no ping, no mock provider, no mock reply,
     no surface the Runtime does not back (tool grants, plan, workbench,
     speculative model downloads or mock Studio);
   - the workspace is a Runtime object behind the capsule's launch token,
     revisioned, never browser storage;
   - Home GUI owns the morph and the place; the capsule speaks to Home only by
     message and never reaches into Home's DOM.
   The pure contract module is exercised directly. */

import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";

const root = new URL("../", import.meta.url);
const read = (path) => readFileSync(new URL(path, root), "utf8");
const capsuleDir = new URL("capsules/assistant/browser/", root);
const capsuleScripts = readdirSync(capsuleDir)
  .filter((name) => name.endsWith(".js"))
  .map((name) => [name, readFileSync(new URL(name, capsuleDir), "utf8")]);

const agentLive = read("capsules/assistant/browser/agent-live.js");
const agentStream = read("capsules/assistant/browser/agent-stream.js");
const agentHarness = read("capsules/assistant/browser/agent-harness.js");
const harnessHost = read("capsules/assistant/browser/harness-host.js");
const entry = read("capsules/assistant/browser/home-agent.js");
const indexHtml = read("capsules/assistant/browser/index.html");
const manifest = JSON.parse(read("capsules/assistant/capsule.json"));
const components = JSON.parse(read("components.json"));
const gateway = read("elastos/crates/elastos-server/src/api/gateway.rs");
const gatewayWorkspace = read("elastos/crates/elastos-server/src/api/gateway_assistant_workspace_v2.rs");
const gatewayAssistant = read("elastos/crates/elastos-server/src/api/gateway_assistant.rs");
const homeFace = read("capsules/home-gui/browser/shell-assistant-face.js");
const homeWindows = read("capsules/home-gui/browser/shell-windows.js");
const homeSurface = read("capsules/home-gui/browser/shell-surface.js");
const homeGui = read("capsules/home-gui/browser/home-gui.js");
const shellStages = read("capsules/home-gui/browser/shell-stages.js");
const localCarrierSetup = read("scripts/local-carrier-setup-smoke.sh");

/* ---- typed model contract, in source ------------------------------------- */

for (const [name, source] of capsuleScripts) {
  assert.ok(
    !/offer:[a-z0-9-]+:[a-z0-9-]+/i.test(source),
    `${name} names a model offer; offers come from offers_list`,
  );
  assert.ok(!/localStorage|sessionStorage|indexedDB/.test(source), `${name} uses browser storage`);
  assert.ok(!/\/api\/provider\/model\/ping|"ping"/.test(source), `${name} pings the provider`);
  assert.ok(!/mock-agent-provider|agent-grants\.js|agent-studio\.js/.test(source), `${name} imports a removed module`);
  assert.ok(
    !/\/api\/apps\/home\/(agent|permissions|machine)/.test(source),
    `${name} calls a route this Runtime does not serve`,
  );
  assert.ok(!/\bMOCK_[A-Z_]+\b|getMockTurn|startMockStream/.test(source), `${name} carries mock replies`);
}
const capsuleFiles = readdirSync(capsuleDir);
for (const gone of ["mock-agent-provider.js", "agent-grants.js", "agent-studio.js", "agent-tip.js"]) {
  assert.ok(!capsuleFiles.includes(gone), `${gone} is out of the capsule`);
}

assert.ok(agentLive.includes('from "./model-contract.js"'), "agent-live speaks the typed contract module");
assert.ok(
  readFileSync(new URL("assistant.js", capsuleDir), "utf8").includes('from "./model-contract.js"'),
  "Assistant Chat and Studio decode events through the typed contract module",
);
assert.ok(agentLive.includes("after_sequence: afterSequence"), "runs_events is polled by after_sequence");
assert.ok(agentLive.includes("textRunCreateBody({ offer, messages, requestId: createRequestId })"));
assert.ok(agentLive.includes('modelRunCall("runs_cancel", { run_id: runId, request_id: newRequestId() })'));
assert.ok(!agentLive.includes("agent-run-cursor"), "the URUX cursor helper is gone");

const turnStart = agentStream.slice(
  agentStream.indexOf("export function startTurnForPrompt("),
  agentStream.indexOf("\n}\n", agentStream.indexOf("export function startTurnForPrompt(")),
);
assert.ok(turnStart.includes("startLiveTurnForPrompt(userText)"));
assert.ok(!turnStart.includes("startMockStream"), "no mock reply when no model is live");
assert.ok(!agentStream.includes("Preview mock"), "live failures are reported, never replaced by a mock");
assert.ok(agentStream.includes("NO_MODEL_OFFER_STATUS"), "the no-offer state is an honest status line");
assert.ok(agentLive.includes("selectedModelOffer(models.map"), "the model menu resolves exact current offer and optional content identity");

/* ---- workspace: a Runtime object ----------------------------------------- */

assert.ok(harnessHost.includes('const WORKSPACE_URL = "/api/apps/assistant/workspace-v2"'));
assert.ok(harnessHost.includes('const WORKSPACE_SCHEMA = "elastos.assistant.workspace/v2"'));
assert.ok(harnessHost.includes("if_revision: workspaceRevision"), "writes carry the revision they saw");
assert.ok(harnessHost.includes("error?.status === 409"), "a revision conflict reloads, never overwrites");
assert.ok(harnessHost.includes("if (workspaceRevision === null) {"), "no write before the read");
assert.ok(entry.includes("loadAgentWorkspace()"), "the entry reads the workspace on boot");
assert.ok(
  entry.indexOf("loadAgentWorkspace()") < entry.indexOf('postToHome({ type: "home-agent:ready" })'),
  "ready is posted after the workspace is read",
);
assert.equal(
  (gateway.match(/\/api\/apps\/assistant\/workspace-v2/g) || []).length,
  1,
  "one workspace route",
);
assert.ok(gateway.includes("gateway_assistant::principal_root_protected_object_inventory(localhost_root)"));
assert.ok(gatewayAssistant.includes("principal_root_protected_object_inventory("));
assert.ok(gatewayWorkspace.includes('const RELATIVE_PATH: &str = ".AppData/ElastOS/Assistant/workspace-v2.json"'));
assert.ok(gatewayWorkspace.includes('require_home_launch_token_context(&state.data_dir, &headers, "assistant")'));
assert.ok(gatewayWorkspace.includes("write_protected_principal_root_object("));
assert.ok(gatewayWorkspace.includes("#[serde(deny_unknown_fields)]"));
assert.ok(harnessHost.includes("migrateLegacyWorkspaces(saved.legacy)"), "legacy sources enter the canonical workspace");
assert.ok(harnessHost.includes("mergeWorkspaceDocuments(lastSnapshot || {}, local, remote || {})"), "conflicts retain both writers");

/* ---- manifest and install ------------------------------------------------ */

assert.equal(manifest.name, "assistant");
const methods = manifest.interfaces.flatMap((i) => i.methods.map((m) => m.operation)).sort();
assert.deepEqual(methods, ["offers_list", "reclaim", "retention", "runs_cancel", "runs_create", "runs_events", "runs_get"]);
assert.ok(components.external?.assistant, "components.json installs Assistant");
assert.ok(!components.external?.["home-agent"], "the legacy app is retired from installation");
assert.ok(
  localCarrierSetup.includes('ASSISTANT_CAPSULE_DIR="${REPO_ROOT}/capsules/assistant"') &&
    localCarrierSetup.includes('"assistant": pathlib.Path(os.environ["ASSISTANT_CAPSULE_DIR"])') &&
    localCarrierSetup.includes('"${DATA_DIR}/capsules/assistant/browser/index.html"'),
  "the local Carrier setup fixture stages and verifies canonical Assistant",
);

/* ---- ownership split with Home GUI ---------------------------------------- */

assert.ok(homeFace.includes('const TARGET_ID = "assistant"'));
assert.ok(
  homeFace.includes("function onAssistantFrameDocumentLoad(") &&
    homeFace.includes('frame?.addEventListener("load", onAssistantFrameDocumentLoad)') &&
    homeFace.includes("frameReady = false"),
  "a replaced Assistant document must handshake ready again so Home can raise the room",
);
assert.ok(
  homeWindows.includes('const HOME_AGENT_TARGET_ID = "assistant"') &&
    (homeWindows.match(/targetId === HOME_AGENT_TARGET_ID/g) || []).length === 3 &&
    (homeWindows.match(/showAssistantFace\((?:normalizedLaunchQuery\(options\.query\))?\);/g) || []).length === 2,
  "canonical and legacy target activation and session restore stay on one Assistant face",
);
assert.ok(
  /export function openSelectedLauncherTarget\(\)[\s\S]{0,180}openTarget\(shellState\.selectedLauncherTargetId\)/.test(homeSurface) &&
    /if \(action === "open-target" \|\| action === "open-target-new-window"\)[\s\S]{0,220}openTarget\(shellState\.contextMenuTarget\.targetId,/.test(homeSurface) &&
    /function attachTargetIconInteractions\(node, targetId, source\)[\s\S]{0,2600}handleTaskbarTargetClick\(targetId\)/.test(homeSurface) &&
    /function attachTargetIconInteractions\(node, targetId, source\)[\s\S]{0,2600}openTarget\(targetId\)/.test(homeSurface),
  "desktop, launcher, taskbar, keyboard, and context-menu activation must converge on Home target activation",
);
assert.ok(homeFace.includes("event.source !== frame.contentWindow"), "Home pins Agent messages to its frame");
assert.ok(!/document\.querySelector\(/.test(harnessHost), "the host seam never reaches Home's DOM");
assert.ok(harnessHost.includes("window.parent.postMessage(message, \"*\")"), "Home is reached by message");
assert.ok(entry.includes("event.source === window.parent"), "messages are accepted from Home only");
assert.ok(
  !capsuleScripts.some(([, source]) => source.includes("home-agent:menubar-reveal")) &&
    !homeFace.includes("home-agent:menubar-reveal") &&
    shellStages.includes('classList.add("stage-menubar-reveal")'),
  "the existing Home stage owns menubar reveal without a second Agent seam",
);
assert.ok(
  agentStream.includes('data-open-artifact="1"') &&
    agentStream.includes('type: "home-agent:open-viewer"') &&
    homeFace.includes('openHomeGuiTargetWithPayload("documents", payload)') &&
    homeGui.includes("bindShellSurfaceDom({ openHomeGuiTargetWithPayload })"),
  "the visible code action uses Home's existing Documents delivery path",
);
assert.ok(
  agentStream.includes('data-open-browser-url="${href}"') &&
    agentHarness.includes('type: "home-agent:open-browser"') &&
    homeFace.includes('openTarget("browser", { query: { url } })') &&
    !agentStream.includes('target="_blank"'),
  "HTTP links use the source-pinned Home Browser handoff instead of capsule popups",
);
assert.ok(!agentStream.includes("asHtml"), "appendMessage has no unused HTML option");
assert.ok(!agentStream.includes("body.innerHTML = text"), "appendMessage has no raw HTML branch");
assert.ok(!indexHtml.includes('data-sidebar-nav="usage"'), "Usage nav is gone");
assert.ok(!indexHtml.includes('data-sidebar-nav="studio"'), "Studio uses the shared mode controls, not a second sidebar");
for (const mode of ["chat", "build", "studio"]) {
  assert.ok(indexHtml.includes(`data-assistant-mode="${mode}"`), `${mode} is a canonical Assistant mode`);
}
assert.ok(!indexHtml.includes('class="assistant-modes"'), "Mode controls live in the sidebar, not a bar above the transcript");
assert.ok(!indexHtml.includes("assistant-copy-conversation"), "Whole-conversation copy is gone; per-message copy stays in the transcript");
assert.equal((indexHtml.match(/id="agent-harness-sidebar"/g) || []).length, 1, "one Assistant sidebar");
const modes = read("capsules/assistant/browser/assistant-modes.js");
assert.ok(modes.includes("MODE_CAPABILITY") && modes.includes("eligibleStudioOffers("), "Mode controls show only when the Runtime backs them (UI ≠ authority)");
assert.ok(modes.includes('from "./assistant.js"') && modes.includes("studioOnly: true"), "Studio uses the typed controller");
for (const theatre of [
  "data-workbench",
  "agent-approve-menu",
  "data-tool-mode",
  "data-segment=",
  "data-plan-markdown",
  "data-models-discover",
  "data-model-download",
  'data-configure-section="overview"',
  'data-configure-section="machine"',
  'data-configure-section="tools"',
  'data-configure-section="runtime"',
  'data-configure-section="permissions"',
  "data-agent-preview",
]) {
  assert.ok(!indexHtml.includes(theatre), `index.html still carries ${theatre}`);
}
assert.ok(!/preview|mock/i.test(indexHtml), "index.html carries no preview or mock copy");
/* One Shelf: the capsule's pill is the composer and nothing else. The dock row,
   the Apps launcher and the Space pager belong to Home GUI, as does the morph. */
for (const shelf of ["taskbar-sortable", "launcher", "space-pager", "agent-shelf-toggle", "shelf-face-apps"]) {
  assert.ok(!indexHtml.includes(shelf), `index.html carries Home's ${shelf}`);
}
const agentShelf = read("capsules/assistant/browser/agent-shelf.js");
assert.ok(
  !/data-agent-morph|agentMorph|flipTaskbarGeometry|launcher/i.test(agentShelf),
  "the capsule runs no Shelf morph of its own; it asks Home to leave and Home runs the one morph",
);
assert.ok(agentShelf.includes("export function leaveAgentRoom()") && agentShelf.includes("setActiveStage(desktopStageId())"));
/* Every way out of the room asks Home: the composer's Home chip, Esc, and the
   sidebar's Home row. */
assert.ok(indexHtml.includes('id="agent-shelf-flip-back"'), "the composer keeps its Home chip");
assert.ok(indexHtml.includes('placeholder="Message Assistant"'), "composer placeholder is route-neutral");
assert.ok(!indexHtml.includes("Ask on this machine"), "composer must not claim this machine");
assert.ok(!indexHtml.includes("agent-think-btn"), "composer has no Think chip");
assert.ok(indexHtml.includes("Show reasoning on replies"), "reasoning disclosure stays in Prompt settings");
assert.equal((indexHtml.match(/id="agent-model-picker"/g) || []).length, 1, "one composer model selector");
assert.ok(indexHtml.includes('aria-haspopup="listbox"'), "the model trigger opens a listbox");
assert.ok(!agentHarness.includes("cost unknown"), "composer trigger omits unknown cost");
assert.ok(
  /closest\?\.\("#agent-shelf-flip-back"\)\)\s*\{\s*event\.preventDefault\(\);\s*leaveAgentRoom\(\);/.test(agentShelf),
  "the composer's Home chip asks Home to leave",
);
assert.ok(/dismiss: \(\) => leaveAgentRoom\(\)/.test(agentShelf), "Esc asks Home to leave");
assert.ok(
  /closest\?\.\("#agent-harness-home"\)[\s\S]{0,400}leaveAgentRoom\(\);/.test(read("capsules/assistant/browser/agent-harness.js")),
  "the sidebar Home row asks Home to leave",
);
const configureSections = read("capsules/assistant/browser/agent-configure.js");
assert.ok(configureSections.includes('export const CONFIGURE_SECTIONS = new Set(["models", "prompt"]);'), "Settings shows only what the Runtime backs");
assert.ok(!/Planning weekend|calm weekend/.test(read("capsules/assistant/browser/agent-harness.js")), "no seeded conversation");

/* ---- the pure contract module -------------------------------------------- */

const contract = await import(new URL("model-contract.js", capsuleDir));
const homeMessageContract = await import(
  new URL("capsules/home-gui/browser/home-agent-message-contract.js", root)
);

const codeBytes = Buffer.from("const answer = 42;", "utf8").toString("base64");
const viewerMessage = {
  type: "home-agent:open-viewer",
  request: {
    target: "documents",
    title: "javascript snippet",
    kind: "code",
    query: { view: "read" },
    deliver: {
      type: "documents:open-chat-attachment",
      attachmentId: "code-123",
      fileName: "snippet.js",
      mimeType: "text/plain",
      dataUrl: `data:text/plain;base64,${codeBytes}`,
    },
  },
};
assert.deepEqual(
  homeMessageContract.normalizeHomeAgentViewerPayload(viewerMessage),
  viewerMessage.request.deliver,
);
for (const invalid of [
  { ...viewerMessage, extra: true },
  { ...viewerMessage, request: { ...viewerMessage.request, target: "library" } },
  {
    ...viewerMessage,
    request: {
      ...viewerMessage.request,
      deliver: { ...viewerMessage.request.deliver, fileName: "../secret" },
    },
  },
  {
    ...viewerMessage,
    request: {
      ...viewerMessage.request,
      deliver: { ...viewerMessage.request.deliver, dataUrl: "data:text/plain;base64,***=" },
    },
  },
  {
    ...viewerMessage,
    request: {
      ...viewerMessage.request,
      deliver: { ...viewerMessage.request.deliver, extra: true },
    },
  },
]) {
  assert.equal(homeMessageContract.normalizeHomeAgentViewerPayload(invalid), null);
}
assert.equal(
  homeMessageContract.normalizeHomeAgentViewerPayload({
    ...viewerMessage,
    request: {
      ...viewerMessage.request,
      deliver: {
        ...viewerMessage.request.deliver,
        dataUrl: `data:text/plain;base64,${"A".repeat(350_000)}`,
      },
    },
  }),
  null,
);
assert.equal(
  homeMessageContract.normalizeHomeAgentBrowserUrl({
    type: "home-agent:open-browser",
    url: "https://example.com/docs?q=agent",
  }),
  "https://example.com/docs?q=agent",
);
for (const invalid of [
  { type: "home-agent:open-browser", url: "ftp://example.com/file" },
  { type: "home-agent:open-browser", url: "https://user@example.com/" },
  { type: "home-agent:open-browser", url: " https://example.com/" },
  { type: "home-agent:open-browser", url: "https://example.com/", extra: true },
  { type: "home-agent:open-browser", url: `https://example.com/${"x".repeat(2_048)}` },
]) {
  assert.equal(homeMessageContract.normalizeHomeAgentBrowserUrl(invalid), "");
}

const offersPayload = {
  status: "ok",
  data: {
    offers: [
      {
        id: "offer-a",
        title: "Local chat",
        operation: "text.generate",
        input_modalities: ["text/plain"],
        output_modalities: ["text/plain"],
        stream_output: true,
      },
      {
        id: "offer-img",
        title: "Images",
        operation: "image.generate",
        input_modalities: ["text/plain"],
        output_modalities: ["application/json"],
      },
      { id: "", title: "broken", operation: "text.generate", input_modalities: ["text/plain"], output_modalities: ["text/plain"] },
      null,
    ],
  },
};
const textOffers = contract.eligibleTextOffers(offersPayload);
assert.deepEqual(textOffers.map((o) => o.id), ["offer-a"]);
assert.deepEqual(contract.eligibleTextOffers({ offers: offersPayload.data.offers }).map((o) => o.id), ["offer-a"]);
assert.deepEqual(contract.eligibleTextOffers(null), []);
const rows = contract.textOfferRows(textOffers);
const localFacts = contract.offerSelectionFacts(textOffers[0]);
assert.deepEqual(rows, [
  {
    id: "live:offer-a",
    offerId: "offer-a",
    operation: "text.generate",
    label: "Local chat",
    detail: "This Home · local",
    streamOutput: true,
    selectionFacts: localFacts,
  },
]);
assert.equal(localFacts.requestedModel, "Local chat");
assert.equal(localFacts.resolvedModel, "");
assert.equal(localFacts.provider, "this Home");
assert.equal(localFacts.execution, "this Home");
assert.equal(localFacts.privacy, "");
assert.equal(localFacts.cost, "");
assert.equal(localFacts.fallback, "");
assert.equal(localFacts.routeSubtitle, "This Home · local");
assert.equal(rows[0].detail, "This Home · local");
assert.equal(localFacts.detailRows.find((row) => row.term === "Cost"), undefined);

const hostedOffer = {
  id: "offer-hosted",
  title: "Hosted chat",
  operation: "text.generate",
  input_modalities: ["text/plain"],
  output_modalities: ["text/plain"],
  stream_output: true,
  policy: {
    concurrency_limit: 1,
    input_bytes_limit: 4096,
    runtime_ms_limit: 30000,
  },
  hosted: {
    placement: "hosted",
    backend_provider_label: "Fixture Provider",
    selection_mode: "pinned",
    requested_selector: "gpt-test",
    privacy_policy_ref: "fixture:privacy:v1",
    terms_ref: "fixture:terms:v1",
    provider_request_policy: "single_dispatch_no_retry",
    upstream_routing_fallback_assertion: "operator_asserted_disabled",
  },
};
const hostedFacts = contract.offerSelectionFacts(hostedOffer);
assert.equal(hostedFacts.requestedModel, "gpt-test");
assert.equal(hostedFacts.resolvedModel, "");
assert.equal(hostedFacts.provider, "Fixture Provider");
assert.equal(hostedFacts.privacy, "fixture:privacy:v1");
assert.equal(hostedFacts.cost, "");
assert.equal(hostedFacts.fallback, "operator_asserted_disabled");
assert.match(hostedFacts.limits, /1 run at a time/);
assert.equal(hostedFacts.execution, "hosted via Fixture Provider");
assert.equal(hostedFacts.routeSubtitle, "Fixture Provider · hosted");
assert.equal(hostedFacts.payer, "this Home");
assert.equal(hostedFacts.detailRows.find((row) => row.term === "Payer")?.value, "This Home");
assert.equal(hostedFacts.detailRows.find((row) => row.term === "Processor")?.value, "Fixture Provider");
assert.equal(hostedFacts.detailRows.find((row) => row.term === "Prompt destination")?.value, "Fixture Provider");
assert.equal(hostedFacts.detailRows.find((row) => row.term === "Availability")?.value, "Hosted");
assert.equal(hostedFacts.detailRows.find((row) => row.term === "Cost"), undefined);
const hostedResolved = contract.offerSelectionFacts(hostedOffer, {
  resolved_model: { status: "reported", value: "gpt-test-resolved" },
  cost: { status: "reported", value: { value: "0.02", unit: "USD" } },
});
assert.equal(hostedResolved.resolvedModel, "gpt-test-resolved");
assert.equal(hostedResolved.cost, "0.02 USD");
assert.equal(
  hostedResolved.detailRows.find((row) => row.term === "Cost")?.value,
  "0.02 USD",
);
assert.equal(
  contract.offerSelectionFacts(hostedOffer, { cost: { status: "unknown" } }).cost,
  "Not reported",
);
assert.equal(contract.textOfferRows([hostedOffer])[0].detail, "Fixture Provider · hosted");

const remoteOffer = {
  id: "offer-remote",
  title: "qwen3-5-9b-q4-k-m-local",
  operation: "text.generate",
  input_modalities: ["text/plain"],
  output_modalities: ["text/plain"],
  stream_output: true,
  remote_service: { display_name: "MA2 Mac guest's AI model", grant_id: "grant-remote" },
};
const remoteFacts = contract.offerSelectionFacts(remoteOffer);
assert.equal(remoteFacts.requestedModel, "qwen3-5-9b-q4-k-m-local");
assert.equal(remoteFacts.resolvedModel, "");
assert.equal(remoteFacts.provider, "MA2 Mac guest's AI model");
assert.equal(remoteFacts.execution, "via MA2 Mac guest's AI model");
assert.equal(remoteFacts.privacy, "");
assert.equal(remoteFacts.cost, "");
assert.equal(remoteFacts.fallback, "");
assert.equal(remoteFacts.routeSubtitle, "via MA2 Mac guest's AI model");
assert.equal(contract.offerRouteKind(remoteOffer), "remote");
const remoteHostedOffer = {
  id: "remote:grant:model:openrouter",
  title: "OpenRouter",
  operation: "text.generate",
  input_modalities: ["text/plain"],
  output_modalities: ["text/plain"],
  stream_output: true,
  hosted: {
    placement: "hosted",
    backend_provider_label: "OpenRouter",
    requested_selector: "fixture/model",
    payer: "this Home",
    intermediary: "this Home",
  },
  remote_service: { display_name: "Owner Home", grant_id: "grant-hosted" },
};
const remoteHostedFacts = contract.offerSelectionFacts(remoteHostedOffer);
assert.equal(contract.offerRouteKind(remoteHostedOffer), "remote_hosted");
assert.equal(remoteHostedFacts.intermediary, "Owner Home");
assert.equal(remoteHostedFacts.processor, "OpenRouter");
assert.equal(remoteHostedFacts.payer, "this Home");
assert.equal(remoteHostedFacts.execution, "via Owner Home; OpenRouter; payer this Home");
assert.equal(contract.textOfferRows([remoteHostedOffer])[0].detail, "via Owner Home");
const remoteRows = contract.textOfferRows([remoteOffer, hostedOffer], {
  "offer-hosted": {
    resolved_model: { status: "reported", value: "stale-hosted" },
    cost: { status: "reported", value: "9 USD" },
  },
});
assert.equal(remoteRows[0].selectionFacts.resolvedModel, "");
assert.equal(remoteRows[0].selectionFacts.cost, "");
assert.equal(remoteRows[1].selectionFacts.resolvedModel, "stale-hosted");

const messages = [
  { role: "system", content: "Be brief." },
  { role: "user", content: "Hi" },
  { role: "assistant", content: "Hello." },
  { role: "user", content: "  " },
  { role: "user", content: "Plan my day" },
];
assert.equal(
  contract.transcriptPrompt(messages),
  "Be brief.\n\nUser: Hi\n\nAssistant: Hello.\n\nUser: Plan my day\n\nAssistant:",
);
const body = contract.textRunCreateBody({ offer: rows[0], messages, requestId: "req-1" });
assert.deepEqual(body, {
  offer_id: "offer-a",
  operation: "text.generate",
  request_id: "req-1",
  input: { schema: "elastos.model.input.text/v1", prompt: contract.transcriptPrompt(messages) },
});
assert.throws(() => contract.textRunCreateBody({ offer: null, messages, requestId: "r" }), /no text model offer/);
assert.throws(() => contract.textRunCreateBody({ offer: rows[0], messages, requestId: "" }), /request id/);

const page1 = contract.applyRunEventsPage(
  {
    events: [
      { sequence: 1, kind: "prepared", data: {} },
      { sequence: 2, kind: "text_delta", data: { text: "Hel" } },
      { sequence: 3, kind: "text_delta", data: { text: "lo" } },
    ],
    next_cursor: 3,
    has_more: true,
  },
  0,
);
assert.deepEqual(page1, {
  nextCursor: 3,
  hasMore: true,
  textDeltas: ["Hel", "lo"],
  terminal: null,
  studioProgress: null,
  backendReport: null,
});

const page2 = contract.applyRunEventsPage(
  {
    events: [
      { sequence: 4, kind: "output", data: { schema: "elastos.model.output.text/v1", text: "Hello" }, terminal: true },
    ],
    next_cursor: 4,
    has_more: false,
  },
  3,
);
assert.equal(page2.terminal.status, "completed");
assert.equal(contract.terminalOutputText(page2.terminal.output), "Hello");
assert.equal(contract.terminalOutputText({ schema: "other", text: "x" }), "");
const reported = contract.applyRunEventsPage(
  {
    events: [
      {
        sequence: 1,
        kind: "completed",
        terminal: true,
        data: { schema: "elastos.model.output.text/v1", text: "Hi" },
        backend_report: {
          schema: "elastos.model.backend-report/v1",
          resolved_model: { status: "reported", value: "gpt-test-resolved" },
          usage: { status: "unknown" },
          cost: { status: "reported", value: { value: "0.01", unit: "USD" } },
        },
      },
    ],
    next_cursor: 1,
    has_more: false,
  },
  0,
);
assert.equal(reported.backendReport.resolved_model.value, "gpt-test-resolved");

const prunedCompleted = contract.applyRunEventsPage(
  {
    events: [{ sequence: 3, kind: "completed", terminal: true, data: { output_retained: false } }],
    next_cursor: 3,
    has_more: false,
    output_retained: false,
  },
  2,
);
assert.equal(prunedCompleted.terminal.status, "completed");
assert.equal(prunedCompleted.terminal.outputRetained, false);
assert.equal(prunedCompleted.terminal.output, null);

const failed = contract.applyRunEventsPage(
  { events: [{ sequence: 1, kind: "failed", data: { code: "backend_down", message: "no backend" }, terminal: true }], next_cursor: 1 },
  0,
);
assert.deepEqual(failed.terminal, { status: "failed", output: null, error: { code: "backend_down", message: "no backend" } });

assert.deepEqual(
  contract.applyRunEventsPage({ events: [], next_cursor: 5 }, 5),
  { nextCursor: 5, hasMore: false, textDeltas: [], terminal: null, studioProgress: null, backendReport: null },
);
assert.throws(() => contract.applyRunEventsPage({ events: [], next_cursor: 2 }, 5), /cursor went backwards/);
assert.deepEqual(
  contract.applyRunEventsPage(
    {
      events: [
        { sequence: 2, kind: "text_delta", data: { text: "a" } },
        { sequence: 2, kind: "text_delta", data: { text: "b" } },
      ],
      next_cursor: 2,
    },
    0,
  ),
  { nextCursor: 2, hasMore: false, textDeltas: ["a"], terminal: null, studioProgress: null, backendReport: null },
);
assert.deepEqual(
  contract.applyRunEventsPage(
    { events: [{ sequence: 1, kind: "text_delta", data: { text: "a" } }], next_cursor: 1 },
    1,
  ),
  { nextCursor: 1, hasMore: false, textDeltas: [], terminal: null, studioProgress: null, backendReport: null },
);
const replayedProgress = contract.applyRunEventsPage(
  {
    events: [
      { sequence: 1, kind: "progress", data: { phase: "rendering", completed: 1, total: 4 } },
      { sequence: 1, kind: "progress", data: { phase: "rendering", completed: 1, total: 4 } },
      { sequence: 2, kind: "progress", data: { phase: "rendering", completed: 2, total: 4 } },
    ],
    next_cursor: 2,
  },
  0,
);
assert.deepEqual(replayedProgress.studioProgress, { phase: "rendering", completed: 2, total: 4 });
assert.throws(
  () =>
    contract.applyRunEventsPage(
      { events: [{ sequence: 1, kind: "progress", data: { phase: "rendering", completed: 3, total: 2 } }], next_cursor: 1 },
      0,
    ),
  /studio progress/,
);
assert.throws(
  () =>
    contract.applyRunEventsPage(
      {
        events: [
          { sequence: 2, kind: "text_delta", data: { text: "B" } },
          { sequence: 1, kind: "text_delta", data: { text: "A" } },
        ],
        next_cursor: 2,
      },
      0,
    ),
  /out of order/,
);
assert.throws(
  () => contract.applyRunEventsPage({ events: [{ sequence: 7, kind: "text_delta", data: { text: "a" } }], next_cursor: 6 }, 0),
  /behind last sequence/,
);
assert.throws(() => contract.applyRunEventsPage({ events: "nope" }, 0), /malformed/);
assert.throws(() => contract.applyRunEventsPage(null, 0), /malformed/);

console.log("canonical Assistant shell smoke: ok");
