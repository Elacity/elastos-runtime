#!/usr/bin/env node

import { readFileSync } from "node:fs";

const root = new URL("../", import.meta.url);
const surfaces = [
  "home",
  "home-gui",
  "home-cli",
  "system",
  "marketplace",
  "people",
  "services",
  "inbox",
  "library",
  "documents",
  "wallet",
  "browser",
];
const bannedPublicCopy = /\b(runtime mirror|permissioned runtime|projection|schema|derived facts?|runtime facts?|capsules?|providers?|capabilit(?:y|ies)|affordances?|authority boundary|provider boundary|gate preview|runtime-owned|host-loaded|structured home intents?|provider operation|launch token|hostcall)\b/i;

function read(path) {
  return readFileSync(new URL(path, root), "utf8");
}

function fail(message, details = "") {
  console.error(`FAIL public-copy entropy: ${message}`);
  if (details) console.error(details);
  process.exit(1);
}

function assert(condition, message, details = "") {
  if (!condition) fail(message, details);
}

function assertPlain(label, value) {
  const text = String(value || "").trim();
  assert(!bannedPublicCopy.test(text), `${label} contains internal narration`, text);
}

function collectPublicDescriptions(value, path = "manifest", output = []) {
  if (!value || typeof value !== "object") return output;
  if (Array.isArray(value)) {
    value.forEach((entry, index) => collectPublicDescriptions(entry, `${path}[${index}]`, output));
    return output;
  }
  for (const [key, entry] of Object.entries(value)) {
    if ((key === "description" || key === "summary") && typeof entry === "string") {
      output.push([`${path}.${key}`, entry]);
    }
    collectPublicDescriptions(entry, `${path}.${key}`, output);
  }
  return output;
}

for (const surface of surfaces) {
  const manifest = JSON.parse(read(`capsules/${surface}/capsule.json`));
  for (const [path, value] of collectPublicDescriptions(manifest, surface)) {
    assertPlain(path, value);
  }
}

const commandContract = JSON.parse(read("capsules/home-cli/browser/commands.json"));
const homeCliMain = [
  "main.rs",
  "runtime_io.rs",
  "line_views.rs",
  "tui_state.rs",
  "tui_render.rs",
  "view_models.rs",
].map((file) => read(`capsules/home-cli/src/${file}`)).join("\n");
for (const command of commandContract.commands || []) {
  if (command.name === "debug") continue;
  assertPlain(`Home CLI ${command.name} summary`, command.summary);
  assertPlain(`Home CLI ${command.name} description`, command.description);
}
for (const control of commandContract.controls || []) {
  assertPlain(`Home CLI ${control.key} control`, control.description);
}
assert(
  !homeCliMain.includes('println!("  capsules  {}"')
    && homeCliMain.includes('"  Status    {}"')
    && homeCliMain.includes('"  Requests  {}"'),
  "Home CLI Wallet restored capsule inventory in ordinary output",
);

function visibleHtml(path, { stripTechnicalDetails = false } = {}) {
  let html = read(path)
    .replace(/<script\b[^>]*>[\s\S]*?<\/script>/gi, " ")
    .replace(/<style\b[^>]*>[\s\S]*?<\/style>/gi, " ");
  if (stripTechnicalDetails) {
    html = html.replace(/<details\b[^>]*id=["']technical-details["'][^>]*>[\s\S]*?<\/details>/gi, " ");
  }
  return html
    .replace(/<[^>]+>/g, " ")
    .replace(/&amp;/g, "&")
    .replace(/&nbsp;/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

for (const [path, options] of [
  ["capsules/home/browser/index.html", {}],
  ["capsules/home-gui/browser/index.html", {}],
  ["capsules/home-cli/browser/index.html", {}],
  ["capsules/system/browser/index.html", { stripTechnicalDetails: true }],
  ["capsules/marketplace/browser/index.html", {}],
  ["capsules/people/browser/index.html", {}],
  ["capsules/services/browser/index.html", {}],
  ["capsules/inbox/browser/index.html", {}],
  ["capsules/library/browser/index.html", {}],
  ["capsules/documents/browser/index.html", {}],
  ["capsules/wallet/browser/index.html", {}],
  ["capsules/browser/browser/index.html", {}],
]) {
  assertPlain(path, visibleHtml(path, options));
}

assertPlain(
  "Home GUI notification template",
  visibleHtml("capsules/home-gui/browser/home-gui-template.html"),
);

const marketplaceHtml = read("capsules/marketplace/browser/index.html");
const marketplaceJs = read("capsules/marketplace/browser/marketplace.js");
assert(
  !/Staff Picks|Featured Apps|Popular Apps/.test(marketplaceHtml + marketplaceJs),
  "Marketplace restored fabricated or duplicate recommendation sections",
);
assert(
  marketplaceJs.includes("<summary class=\"modal-section-title\">Technical details</summary>"),
  "Marketplace technical fields must remain behind Technical details",
);
assert(
  !marketplaceJs.includes("No provider authority declared")
    && !marketplaceJs.includes("Installed Runtime apps")
    && !marketplaceJs.includes("Marketplace catalog unavailable")
    && marketplaceJs.includes("runtime|capsules?|providers?|projection|schema|derived facts?|boundary")
    && marketplaceJs.includes("function publicTitle(capsule)")
    && marketplaceJs.includes('role === "provider" ? "provider"')
    && marketplaceJs.includes('if (role === "provider") return `${title} service for apps on this Home.`;')
    && marketplaceJs.includes("technicalDependencies")
    && !marketplaceJs.includes("showInstallPending")
    && !marketplaceJs.includes("Install pending"),
  "Marketplace restored internal empty or error copy",
);

const homeRuntime = read("elastos/crates/elastos-server/src/api/gateway_home_runtime.rs");
const catalogReadModel = read("elastos/crates/elastos-server/src/api/gateway_capsule_catalog/read_model.rs");
assert(
  homeRuntime.includes('CHAT_ROOM_CAPSULE_ID => "Send messages and join conversations."')
    && homeRuntime.includes(
      'MARKETPLACE_CAPSULE_ID => {\n            "Discover and open apps, viewers, and content on this device."',
    )
    && homeRuntime.includes('SERVICES_CAPSULE_ID => "Sharing".to_string()')
    && homeRuntime.includes(
      'SERVICES_CAPSULE_ID => {\n            "Share Browser Engine and Browser Exit services with people."',
    )
    && homeRuntime.includes('INBOX_CAPSULE_ID => "Review messages, requests, and approvals."')
    && homeRuntime.includes('BROWSER_CAPSULE_ID => "Browse websites from this device."')
    && !homeRuntime.includes("Open web sites through the ElastOS Browser boundary")
    && !homeRuntime.includes("Browse installed capsules, providers, viewers, and content")
    && !homeRuntime.includes("Browse installed apps, services, viewers, and content.")
    && !homeRuntime.includes("Manage Browser Exit Node sharing and subscriptions."),
  "Runtime restored internal app descriptions used by public catalogs",
);
assert(
  catalogReadModel.includes('"object-provider" => Some("Storage")')
    && catalogReadModel.includes('"browser-engine-adapter" => Some("Browser Engine")')
    && catalogReadModel.includes('"did-provider" => Some("Identity")')
    && catalogReadModel.includes('"net-provider" => Some("Network")')
    && catalogReadModel.includes('"wallet-provider" => Some("Wallet Security")'),
  "Runtime public catalog must expose service names separately from technical capsule ids",
);

const systemHtml = read("capsules/system/browser/index.html");
const systemJs = read("capsules/system/browser/system.js");
assert(
  systemHtml.includes('<details id="technical-details"')
    && !systemHtml.includes('<details id="technical-details" open'),
  "System Technical Details must exist and stay closed by default",
);
assert(
  !systemHtml.includes("Capsules and providers")
    && !systemHtml.includes("Capability-scoped object surfaces")
    && !systemHtml.includes("Configured chain providers"),
  "System restored internal narration outside Technical Details",
);
assert(
  !systemHtml.includes('data-settings="storage"')
    && !systemJs.includes("renderWebspaceDetails")
    && !systemJs.includes('empty.textContent = "No capsules or providers discovered."')
    && systemJs.includes("function publicSystemError(")
    && systemJs.includes('publicSystemError(error, "System could not be loaded.")')
    && !systemJs.includes("actions.push(description.replace")
    && systemJs.includes('return dependency ? catalogTitle(dependency) : "";')
    && systemJs.includes('{ role: "provider", label: "Background services" }'),
  "System must keep app discovery human-facing and technical inspection explicit",
);

const libraryDialog = read("capsules/library/browser/src/dialog.js");
const libraryActions = read("capsules/library/browser/src/actions.js");
const libraryApp = read("capsules/library/browser/src/app.js");
const libraryRender = read("capsules/library/browser/src/render.js");
assert(
  libraryDialog.includes('data-tab="technical">Technical</div>')
    && !libraryDialog.includes('data-tab="runtime">Runtime</div>')
    && !libraryDialog.includes("Visibility Contract"),
  "Library properties must keep implementation fields under Technical",
);
assert(
  !libraryActions.includes("provider-owned location")
    && !libraryActions.includes(" object${")
    && !libraryActions.includes("Runtime provider")
    && !libraryActions.includes("selected objects")
    && !libraryActions.includes("Every object in Trash")
    && libraryApp.includes('"Choose an item for Browser."')
    && libraryApp.includes('"Choose an item for Chat."')
    && !libraryApp.includes("Choose an object")
    && libraryRender.includes('title: "No matching items"')
    && libraryRender.includes('title: "This space is empty"')
    && libraryRender.includes('${visible} item${visible === 1 ? "" : "s"}')
    && libraryRender.includes('elements.currentTitle.textContent || "Library"')
    && !libraryRender.includes("shortUri(state.currentUri)")
    && !libraryRender.includes("Localhost is your signed local object space"),
  "Library restored implementation nouns in ordinary actions",
);
assert(
  (libraryDialog.match(/<summary>Technical details<\/summary>/g) || []).length >= 3
    && !libraryDialog.includes("Runtime checked this object")
    && !libraryDialog.includes("Choose a Runtime share policy")
    && !libraryDialog.includes("Recipient-scoped access requires Runtime recipient proof"),
  "Library share flows must keep implementation details collapsed",
);

const homeHost = read("capsules/home/browser/home-shell-host.js");
const homeWindows = read("capsules/home-gui/browser/shell-windows.js");
const peopleApp = read("capsules/people/browser/people.js");
assert(
  !homeHost.includes("Home event channel returned an invalid schema")
    && !homeHost.includes("Switching shells requires an explicit shell launch token")
    && !homeHost.includes("return error.message || String(error)"),
  "Home restored raw contract errors",
);
assert(
  !homeWindows.includes("Home asked the runtime to open this item")
    && !homeWindows.includes('subjectLabel: "Item ID"')
    && !homeWindows.includes("/api/apps/people/")
    && peopleApp.includes("function publicError(error, fallback)"),
  "Home GUI restored raw launch or People errors",
);

const services = read("capsules/services/browser/services.js");
assert(
  !services.includes("provider grant")
    && !services.includes("private route ticket")
    && !services.includes("missing an offer id")
    && services.includes("function publicServicesError(")
    && services.includes('publicServicesError(text, "Sharing could not be updated.")'),
  "Services restored provider implementation copy",
);

const inbox = read("capsules/inbox/browser/index.html");
const documents = read("capsules/documents/browser/index.html");
const walletCreate = read("capsules/wallet/browser/wallet-create-account-flow.js");
const walletSend = read("capsules/wallet/browser/wallet-send-flow.js");
const walletRender = read("capsules/wallet/browser/wallet-render.js");
const walletFlows = read("capsules/wallet/browser/wallet-flows.js");
assert(inbox.includes("publicInboxText"), "Inbox must sanitize request and error copy");
assert(documents.includes("readableDocumentsError") && !documents.includes("Documents provider request failed") && !documents.includes('escapeHtml(error.message || "Could not load documents.")'), "Documents must sanitize provider errors");
assert(!walletCreate.includes("Chains are provider routes") && !walletCreate.includes('placeholder=\'{"schema"'), "Wallet must not expose route or schema instructions");
assert(!walletSend.includes("chain-provider") && !walletSend.includes("from this Runtime"), "Wallet send must use device-level copy");
assert(walletRender.includes("export function publicWalletText(") && walletFlows.includes("publicWalletText(message)"), "Wallet must sanitize status and modal errors");
assert(read("capsules/library/browser/src/app.js").includes("function publicLibraryText("), "Library must sanitize ordinary action errors");

const browserStatus = read("capsules/browser/browser/browser-status.js");
const browserJs = read("capsules/browser/browser/browser.js");
const browserDisplay = read("capsules/browser/browser/browser-remote-display.js");
assert(!browserStatus.includes("Browser failed closed") && browserStatus.includes("public") === false, "Browser must not show fail-closed implementation narration");
assert(!browserJs.includes("Browser Engine Adapter returned an invalid") && !browserDisplay.includes("Browser Engine Adapter returned an invalid"), "Browser must not show adapter contract failures");

const homeVisual = read("scripts/home-camofox-smoke.mjs");
const systemVisual = read("scripts/system-camofox-smoke.mjs");
const passkeyVisual = read("scripts/home-passkey-virtual-auth-smoke.mjs");
for (const surface of ["home-cli", "system", "marketplace", "services", "inbox", "library", "documents", "wallet", "browser"]) {
  assert(homeVisual.includes(`\"/apps/${surface}/\"`), `visual empty/error coverage is missing ${surface}`);
}
assert(
  homeVisual.includes('"ordinary-app-copy-stays-plain-and-heading-unique"')
    && homeVisual.includes('"home-cli-copy-stays-plain"')
    && homeVisual.includes('"browser"')
    && systemVisual.includes('state.panelLabels.includes("Accounts")')
    && systemVisual.includes('!state.fieldLabels.includes("Documents")')
    && passkeyVisual.includes("async function checkHomePublicCopy(page)")
    && passkeyVisual.includes('"Home GUI exposed implementation copy"')
    && passkeyVisual.includes('"Home GUI rendered duplicate visible headings"'),
  "visual public-copy checks are stale or incomplete",
);

console.log("PASS public-copy entropy check");
