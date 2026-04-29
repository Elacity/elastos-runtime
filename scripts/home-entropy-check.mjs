#!/usr/bin/env node

import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = new URL("../", import.meta.url);
const repoRootPath = fileURLToPath(repoRoot);

function read(path) {
  return readFileSync(new URL(path, repoRoot), "utf8");
}

function readBytes(path) {
  return readFileSync(new URL(path, repoRoot));
}

function pngDimensions(path) {
  const bytes = readBytes(path);
  const pngSignature = "89504e470d0a1a0a";
  assert(bytes.subarray(0, 8).toString("hex") === pngSignature, `${path} must be a PNG image`);
  return {
    width: bytes.readUInt32BE(16),
    height: bytes.readUInt32BE(20),
  };
}

function assertPngDimensions(path, width, height) {
  const dimensions = pngDimensions(path);
  assert(
    dimensions.width === width && dimensions.height === height,
    `${path} must have exact PNG dimensions`,
    dimensions,
  );
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function assert(condition, message, details = undefined) {
  if (!condition) {
    const suffix = details ? `\n${JSON.stringify(details, null, 2)}` : "";
    throw new Error(`${message}${suffix}`);
  }
}

function listMarkdownFiles(dir = repoRootPath) {
  const entries = readdirSync(dir, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    if (entry.name === ".git" || entry.name === "target" || entry.name === "node_modules") {
      continue;
    }
    const full = resolve(dir, entry.name);
    if (entry.isDirectory()) {
      files.push(...listMarkdownFiles(full));
    } else if (entry.isFile() && entry.name.endsWith(".md")) {
      files.push(full);
    }
  }
  return files;
}

function assertMarkdownLocalLinksResolve() {
  const failures = [];
  for (const file of listMarkdownFiles()) {
    const source = readFileSync(file, "utf8");
    const relativeFile = file.slice(repoRootPath.length);
    for (const match of source.matchAll(/\[[^\]\n]+\]\(([^)]+)\)/g)) {
      let target = match[1].trim();
      if (
        !target ||
        target.startsWith("#") ||
        target.startsWith("http:") ||
        target.startsWith("https:") ||
        target.startsWith("mailto:") ||
        target.includes("://")
      ) {
        continue;
      }
      target = target.replace(/^<|>$/g, "");
      const [targetPath] = target.split("#");
      if (!targetPath) {
        continue;
      }
      const resolved = resolve(dirname(file), targetPath);
      if (!existsSync(resolved)) {
        failures.push(`${relativeFile}: ${target}`);
      }
    }
  }
  assert(failures.length === 0, "Markdown local links must resolve", failures);
}

function assertMarkdownScriptReferencesResolve() {
  const failures = [];
  for (const file of listMarkdownFiles()) {
    const source = readFileSync(file, "utf8");
    const relativeFile = file.slice(repoRootPath.length);
    for (const match of source.matchAll(/(?:^|[^A-Za-z0-9_./-])(scripts\/[A-Za-z0-9_./-]+(?:\.sh|\.mjs))/g)) {
      const script = match[1];
      const resolved = resolve(repoRootPath, script);
      if (!existsSync(resolved) || !statSync(resolved).isFile()) {
        failures.push(`${relativeFile}: ${script}`);
      }
    }
  }
  assert(failures.length === 0, "Markdown script references must point to existing scripts", failures);
}

function assertToken(source, file, token, value) {
  const pattern = new RegExp(`${escapeRegExp(token)}\\s*:\\s*${escapeRegExp(value)}\\s*;`);
  assert(pattern.test(source), `${file} is missing canonical token ${token}: ${value}`);
}

function stripTags(value) {
  return value.replace(/<[^>]+>/g, "").replace(/\s+/g, " ").trim();
}

function attributeValue(markup, name) {
  const match = new RegExp(`${name}\\s*=\\s*["']([^"']*)["']`, "i").exec(markup);
  return match ? match[1].trim() : "";
}

function hasAttribute(markup, name) {
  return new RegExp(`\\s${name}(\\s|=|>)`, "i").test(markup);
}

function classList(markup) {
  return attributeValue(markup, "class").split(/\s+/).filter(Boolean);
}

function hasAccessibleName(markup) {
  return (
    attributeValue(markup, "aria-label").length > 0 ||
    attributeValue(markup, "title").length > 0 ||
    attributeValue(markup, "placeholder").length > 0 ||
    stripTags(markup).length > 0
  );
}

function isDynamicTemplateButton(markup) {
  const classes = new Set(classList(markup));
  return (
    classes.has("launcher-card") ||
    classes.has("taskbar-item") ||
    classes.has("taskbar-window-count")
  );
}

function assertStaticControlsAreNamed(file) {
  const source = read(file);
  for (const match of source.matchAll(/<button\b[\s\S]*?<\/button>/gi)) {
    const markup = match[0];
    const isHidden = hasAttribute(markup, "hidden") || attributeValue(markup, "aria-hidden") === "true";
    assert(
      isHidden || isDynamicTemplateButton(markup) || hasAccessibleName(markup),
      `${file} has a static button without a human-readable label`,
      { button: markup.replace(/\s+/g, " ").slice(0, 220) },
    );
  }
  for (const match of source.matchAll(/<input\b[^>]*>/gi)) {
    const markup = match[0];
    const type = attributeValue(markup, "type").toLowerCase();
    const isHidden = hasAttribute(markup, "hidden") || type === "hidden";
    assert(
      isHidden || hasAccessibleName(markup),
      `${file} has a static input without a human-readable label`,
      { input: markup.replace(/\s+/g, " ").slice(0, 220) },
    );
  }
  for (const match of source.matchAll(/<(textarea|select)\b[\s\S]*?<\/\1>/gi)) {
    const markup = match[0];
    const isHidden = hasAttribute(markup, "hidden");
    assert(
      isHidden || hasAccessibleName(markup),
      `${file} has a static ${match[1]} without a human-readable label`,
      { control: markup.replace(/\s+/g, " ").slice(0, 220) },
    );
  }
}

const activeUiFiles = [
  "capsules/home/browser/index.html",
  "capsules/home/browser/style.css",
  "capsules/home/browser/shell.js",
  "capsules/home/browser/shell-surface.js",
  "capsules/home/browser/shell-windows.js",
  "capsules/home/browser/service-worker.js",
  "capsules/system/browser/index.html",
  "capsules/system/browser/system.js",
  "capsules/system/browser/style.css",
  "capsules/documents/index.html",
  "capsules/inbox/index.html",
  "capsules/library/index.html",
  "capsules/chat-room/browser/index.html",
  "capsules/chat-room/browser/style.css",
  "capsules/gba-emulator/index.html",
  "capsules/gba-emulator/style.css",
  "capsules/gba-emulator/emulator.js",
];

const activeHtmlFiles = [
  "capsules/home/browser/index.html",
  "capsules/system/browser/index.html",
  "capsules/documents/index.html",
  "capsules/inbox/index.html",
  "capsules/library/index.html",
  "capsules/chat-room/browser/index.html",
  "capsules/gba-emulator/index.html",
];

const staleCopy = [
  "Quiet right now",
  "Nothing needs review",
  "Decide requests.",
  "Document objects with local working copies",
  "Game Boy Advance",
  "uCity Advance",
  "Room Browser",
  "md-viewer",
  "Choose an installed ROM or drop a .gba file to play",
  "Drop a .gba ROM here or click to browse",
];

for (const file of activeUiFiles) {
  const source = read(file);
  for (const phrase of staleCopy) {
    assert(!source.includes(phrase), `${file} still contains stale UI copy: ${phrase}`);
  }
}

for (const file of activeHtmlFiles) {
  assertStaticControlsAreNamed(file);
}

const lightTokenFiles = [
  "capsules/chat-room/browser/style.css",
  "capsules/gba-emulator/style.css",
  "capsules/system/browser/style.css",
  "capsules/documents/index.html",
  "capsules/inbox/index.html",
  "capsules/library/index.html",
];

const lightTokens = new Map([
  ["--bg", "#edf1fb"],
  ["--bg-strong", "#e3e9fb"],
  ["--panel", "rgba(255, 255, 255, 0.9)"],
  ["--panel-strong", "#ffffff"],
  ["--panel-soft", "#eef2ff"],
  ["--line", "rgba(83, 103, 164, 0.14)"],
  ["--line-strong", "rgba(83, 103, 164, 0.22)"],
  ["--ink", "#1d2438"],
  ["--muted", "#66708a"],
  ["--brand", "#f6921a"],
  ["--brand-soft", "#fff1dc"],
  ["--accent", "#5f76d8"],
  ["--accent-soft", "#e8edff"],
  ["--accent-deep", "#3c53a7"],
  ["--danger", "#b14c5a"],
]);

for (const file of lightTokenFiles) {
  const source = read(file);
  for (const [token, value] of lightTokens) {
    assertToken(source, file, token, value);
  }
}

const shellStyle = read("capsules/home/browser/style.css");
assertToken(shellStyle, "capsules/home/browser/style.css", "--brand", "#f6921a");
assertToken(shellStyle, "capsules/home/browser/style.css", "--brand-strong", "#ffb457");
assert(
  (shellStyle.match(/#f6921a/g) || []).length === 1,
  "Home brand orange should be defined once as --brand",
);
assert(
  (shellStyle.match(/#ffb457/g) || []).length === 1,
  "Home hover brand orange should be defined once as --brand-strong",
);
assert(shellStyle.includes("min-height: 100dvh;"), "Home must use dynamic viewport height for mobile browsers");
assert(shellStyle.includes("env(safe-area-inset-top"), "Home chrome/window layout must respect mobile safe-area top inset");
assert(shellStyle.includes("env(safe-area-inset-bottom"), "Home chrome/window layout must respect mobile safe-area bottom inset");
assert(shellStyle.includes("max-height: calc(100dvh - 54px);"), "Home context menu must stay inside short viewports");
assert(shellStyle.includes(".taskbar-sortable::-webkit-scrollbar"), "Home taskbar must remain scroll-safe on narrow screens");
assert(shellStyle.includes(".window[data-maximized=\"true\"]") && shellStyle.includes("inset: 0 !important;"), "Home maximized windows must own the full viewport");
assert(shellStyle.includes(".window[data-maximized=\"true\"].window-active"), "Home active maximized windows must stack above Home chrome");

const shellIndex = read("capsules/home/browser/index.html");
const shellManifest = JSON.parse(read("capsules/home/browser/manifest.webmanifest"));
const shellServiceWorker = read("capsules/home/browser/service-worker.js");
const shellSurface = read("capsules/home/browser/shell-surface.js");
const shellJs = read("capsules/home/browser/shell.js");
const shellCore = read("capsules/home/browser/shell-core.js");
const shellCmd = read("elastos/crates/elastos-server/src/shell_cmd.rs");
const operatorControl = read("elastos/crates/elastos-server/src/operator_control.rs");
const chatRoomUi = read("capsules/chat-room-ui/src/lib.rs");
const roomService = read("elastos/crates/elastos-server/src/room_service.rs");
const gatewayApi = read("elastos/crates/elastos-server/src/api/gateway.rs");
const debugPolicy = read("DEBUG.md");
assert(shellIndex.includes('role="listbox"'), "Home items must expose keyboard-selectable structure");
assert(shellIndex.includes('aria-label="Home items"'), "Home items list must be labeled");
assert(shellIndex.includes('data-home-status="booting"'), "Home readiness state must use Home naming");
assert(!shellIndex.includes("data-shell-status"), "Home readiness state must not preserve shell naming");
assert(shellIndex.includes('data-action="minimize"'), "Window minimize action must remain explicit");
assert(shellIndex.includes('data-action="maximize"'), "Window maximize action must remain explicit");
assert(shellIndex.includes('data-action="close"'), "Window close action must remain explicit");
assert(shellIndex.includes('rel="manifest"'), "Home must expose a web app manifest for mobile install");
assert(shellIndex.includes('manifest.webmanifest?v=home-20260427b'), "Home manifest URL must be cache-busted after PWA icon changes");
assert(shellIndex.includes('id="toolbar-fullscreen"'), "Home must expose a fullscreen control in the top toolbar");
assert(shellManifest.name === "ElastOS Home", "Home PWA manifest must install as ElastOS Home");
assert(shellManifest.display === "standalone", "Home PWA manifest must use standalone display mode");
assert(Array.isArray(shellManifest.icons) && shellManifest.icons.some((icon) => icon.src === "./elastos-home-icon.svg"), "Home PWA manifest must use the Elastos app icon");
assert(shellManifest.icons.some((icon) => icon.src === "./elastos-home-icon-192.png" && icon.sizes === "192x192"), "Home PWA manifest must include a 192px install icon");
assert(shellManifest.icons.some((icon) => icon.src === "./elastos-home-icon-512.png" && icon.sizes === "512x512"), "Home PWA manifest must include a 512px install icon");
assert(shellServiceWorker.includes("elastos-home-20260427b"), "Home service worker cache key must match the browser asset version");
assert(!shellServiceWorker.includes("elastos-home-shell"), "Home service worker must not preserve the old shell cache namespace");
const homePwaIcon = read("capsules/home/browser/elastos-home-icon.svg");
assert(homePwaIcon.includes("M669.61 232.852"), "Home PWA icon must use the same Elastos mark geometry as the top navbar logo");
assert(homePwaIcon.includes("paint0_linear_47_14"), "Home PWA icon must preserve the official Elastos mark gradient identity");
assert(!homePwaIcon.includes("M256 78 420 172"), "Home PWA icon must not use the old approximate mark");
assert(!homePwaIcon.includes("M256 206 420 300"), "Home PWA icon must not use the old approximate mark");
assertPngDimensions("capsules/home/browser/elastos-home-icon-192.png", 192, 192);
assertPngDimensions("capsules/home/browser/elastos-home-icon-512.png", 512, 512);
assert(shellJs.includes("registerHomeServiceWorker"), "Home must register its service worker from the shell entrypoint");
assert(shellJs.includes("dataset.homeStatus"), "Home runtime status must be exposed under data-home-status");
assert(!shellJs.includes("dataset.shellStatus"), "Home runtime status must not preserve data-shell-status");
assert(shellJs.includes("toggleShellFullscreen"), "Home fullscreen control must be wired to shell behavior");
assert(shellCore.includes("toolbarFullscreenButton"), "Home fullscreen control must be exported by shell-core");
assert(shellSurface.includes('card.setAttribute("aria-label", `Open ${app.title}`);'), "Launcher cards must expose human-readable action labels");
assert(shellSurface.includes('button.setAttribute("aria-label", desktopShortcutAriaLabel(label));'), "Desktop shortcuts must expose human-readable action labels");
assert(shellSurface.includes("shouldFocusLauncherSearch"), "Home launcher search focus must be gated for touch devices");
assert(!shellSurface.includes("ensureLauncherSelection(activeBrowserTargetId());\n  launcherSearch.focus();"), "Home launcher must not focus search unconditionally on mobile");
assert(shellJs.includes("SHELL_MESSAGE_OPEN_TARGET_SOURCES"), "Home open-target messages must stay source-gated");
assert(shellIndex.includes('shell.js?v=home-20260427b'), "Home entry module must cache-bust after shell browser changes");
assert(shellIndex.includes('style.css?v=home-20260427b'), "Home stylesheet must cache-bust after shell browser changes");
assert(shellJs.includes('shell-core.js?v=home-20260427b'), "Home shell.js must import the current shell-core module instance");
assert(shellJs.includes('shell-surface.js?v=home-20260427b'), "Home must not mix old shell-surface module instances with current shell-windows");
assert(shellJs.includes('shell-windows.js?v=home-20260427b'), "Home shell.js must import the current shell-windows module instance");
assert(shellSurface.includes('shell-core.js?v=home-20260427b'), "Home shell-surface must import the current shell-core module instance");
assert(shellSurface.includes('shell-windows.js?v=home-20260427b'), "Home shell-surface must import the same shell-windows module instance as shell.js");
assert(shellSurface.includes("shouldOpenDesktopShortcutFromClick"), "Home desktop icons must use touch-specific tap-open behavior");
assert(shellSurface.includes("longPressReady"), "Home desktop icons must require long-press before touch dragging");
assert(shellSurface.includes("clearDragSelection"), "Home desktop drag must actively clear browser text selection");
assert(shellSurface.includes('document.body.classList.add("dragging-target")'), "Home desktop drag must mark the selection-suppression lifetime");
assert(shellStyle.includes("body.dragging-target"), "Home desktop drag must suppress text selection while moving icons");
assert(shellCore.includes("desktopHidden: []"), "Home layout state must track per-target desktop icon removal");
assert(shellCore.includes("addTargetToDesktop") && shellCore.includes("removeTargetFromDesktop"), "Home must support reversible desktop icon presence");
assert(shellSurface.includes('action: "remove-desktop-icon"') && shellSurface.includes('action: "add-desktop-icon"'), "Home menus must expose remove/add desktop icon actions");
assert(shellJs.includes('type: "home:open-target"') || shellJs.includes('"home:open-target"'), "Home must keep the open-target message contract");
assert(shellJs.includes('type: "home:open-uri"') || shellJs.includes('"home:open-uri"'), "Home must keep the open-uri message contract");
assert(!shellJs.includes("pc2-shell:"), "Home postMessage contract must not preserve old pc2-shell message types");
assert(!shellJs.includes("shell_token"), "Home browser route tokens must use home_token");
assert(!shellJs.includes("x-elastos-shell-token"), "Home browser API tokens must use x-elastos-home-token");
assert(shellJs.includes("SHELL_MESSAGE_DELIVER_TARGET_SOURCES"), "Home must source-gate capsule-to-capsule picker returns");
assert(shellJs.includes('library: new Set(["chat-room"])'), "Home must allow Library picker results to return to Chat Room only");
assert(shellJs.includes('"home:close-self"'), "Home must allow token-bound picker windows to close themselves after selection");
assert(shellJs.includes('"chat-room": new Set(["library"])'), "Chat Room Attach must be allowed to open Library through Home message policy");
assert(shellJs.includes('new Set(["documents", "chat-room"])'), "Home must allow Chat Room to open elastos:// links through the same URI contract as Documents");
assert(roomService.includes('parsed.scheme() == "elastos"'), "Chat Room service must classify elastos:// document links as first-class room links");
assert(chatRoomUi.includes('data-open-uri'), "Chat Room must render elastos:// room links as shell-openable actions");
assert(chatRoomUi.includes("home:open-uri"), "Chat Room must open elastos:// room links through Home URI orchestration");
assert(!chatRoomUi.includes("/api/provider/documents/"), "Chat Room must not call the Documents provider directly");
assert(!chatRoomUi.includes("/ipfs/"), "Chat Room must not call IPFS routes directly");
assert(chatRoomUi.includes("data-guest-action"), "Chat Room must expose guest kick actions in shell mode");
assert(chatRoomUi.includes("data-node-action"), "Chat Room must expose runtime node block/cancel actions in shell mode");
assert(chatRoomUi.includes("data-room-policy"), "Chat Room must expose room policy controls in shell mode");
assert(chatRoomUi.includes("show_access_controls"), "Chat Room access controls must stay behind an explicit settings toggle");
assert(chatRoomUi.includes("home:open-target"), "Chat Room Attach must ask Home to open Library instead of directly opening host files in shell mode");
assert(chatRoomUi.includes("Open Home to attach from Library."), "Chat Room Attach must fail visibly when Home is unavailable");
assert(!chatRoomUi.includes("attachment_input.click()"), "Chat Room Attach must not open the host browser file picker");
assert(chatRoomUi.includes('"returnTarget"') && chatRoomUi.includes('"attach"'), "Chat Room Attach must launch Library in explicit attach mode");
assert(chatRoomUi.includes("chat-room:attach-library-item"), "Chat Room must accept Library picker results through Home delivery");
assert(chatRoomUi.includes("summary.local_runtime_role.is_none() && !summary.browser_access_allowed"), "Chat Room shell sessions must remain openable when guest requests are disabled for active members");
assert(gatewayApi.includes('"/api/apps/chat-room/guests/:session_id/kick"'), "Chat Room guest kicking must go through the gateway capacity-token API");
assert(gatewayApi.includes('"/api/apps/chat-room/members/invite"'), "Chat Room runtime node invites must go through the gateway capacity-token API");
assert(gatewayApi.includes('"/api/apps/chat-room/members/remove"'), "Chat Room runtime node blocking must go through the gateway capacity-token API");
assert(!gatewayApi.includes("ProviderBridge::spawn"), "Gateway must not spawn provider bridges directly");
assert(!gatewayApi.includes("ipfs_provider_binary") && !gatewayApi.includes("ipfs_bridge"), "Gateway must not keep a direct IPFS bridge fallback");
const gatewayActiveSession = gatewayApi.match(/struct GatewayActiveSessionSummary \{([\s\S]*?)\n\}/)?.[1] || "";
assert(gatewayActiveSession.includes("session_id: String"), "Gateway summaries must expose only a public active-session id");
assert(!gatewayActiveSession.includes("token:"), "Gateway summaries must not expose room session tokens");
assert(shellCore.includes("desktopIconsVisible: true"), "Home layout state must track global desktop icon visibility");
assert(shellSurface.includes('action: "toggle-desktop-icons"'), "Home desktop menu must expose desktop icon visibility");
assert(!shellSurface.includes('label: "Go Home"'), "Home desktop menu must not expose redundant Go Home");
assert(!shellSurface.includes('label: "Open Launcher"'), "Home desktop menu must not expose redundant Open Launcher");
assert(!shellSurface.includes('label: "Open System"'), "Home desktop menu must not expose redundant Open System");
assert(!shellCmd.includes("Legacy fields"), "Runtime coords must not preserve legacy token fields");
const runtimeCoordsBlock = shellCmd.match(/pub struct RuntimeCoords \{([\s\S]*?)\n\}/)?.[1] || "";
assert(!runtimeCoordsBlock.includes("shell_token") && !runtimeCoordsBlock.includes("client_token"), "Runtime coords must only persist attach_secret, not bearer tokens");
assert(!operatorControl.includes("struct RuntimeCoords"), "Operator control must use canonical shell_cmd runtime coords");
assert(operatorControl.includes("crate::runtime_control::read_operator_runtime_coords"), "Operator control must use canonical runtime coord validation");
assert(!operatorControl.includes("async fn attach_secret_matches"), "Operator control must not duplicate attach-secret validation");
assert(debugPolicy.includes("# Debugging Policy"), "Root DEBUG.md must stay a stable developer policy, not a work log");
assert(!debugPolicy.includes("pc2-shell") && !debugPolicy.includes("md-viewer"), "Root DEBUG.md must not preserve stale route history");

const components = JSON.parse(read("components.json"));
const homeProfile = new Set(components.profiles.home.components);
for (const component of ["home", "system", "documents", "library", "inbox"]) {
  assert(homeProfile.has(component), `Home profile must install first-party ${component} assets`);
  assert(components.external[component], `${component} must be a first-party external setup asset`);
  for (const platform of ["linux-amd64", "linux-arm64"]) {
    const metadata = components.external[component].platforms[platform] || components.external[component].platforms["*"];
    assert(metadata?.release_path && metadata?.extract_path, `${component} must publish archive metadata for ${platform}`);
  }
}

const documents = read("capsules/documents/index.html");
const inbox = read("capsules/inbox/index.html");
const library = read("capsules/library/index.html");
const chatStyle = read("capsules/chat-room/browser/style.css");
const gba = read("capsules/gba-emulator/index.html");
const gbaStyle = read("capsules/gba-emulator/style.css");
const gbaJs = read("capsules/gba-emulator/emulator.js");
const gbaScript = read("scripts/gba.sh");
const system = read("capsules/system/browser/index.html");
const systemStyle = read("capsules/system/browser/style.css");
assert(!documents.includes("home:open-uri"), "Documents published URI sharing must copy the elastos:// link, not reopen itself");
assert(documents.includes("/api/provider/documents/"), "Documents must use the runtime documents provider API");
assert(documents.includes('"x-elastos-home-token"'), "Documents provider API calls must carry the Home capacity token");
assert(!documents.includes("/ipfs/"), "Documents capsule must not call direct IPFS HTTP routes");
assert(!documents.includes("IpfsBridge"), "Documents capsule must not instantiate IPFS directly");
assert(!documents.includes("ipfs-provider"), "Documents capsule must not know provider-specific IPFS errors");
assert(documents.includes("item.latest_published_cid === requestedCid"), "Documents shell CID launches must resolve local published documents before public CID loading");
assert(documents.includes('id="copy-published-link"'), "Documents published URI must be copied from the toolbar action row");
assert(documents.includes("navigator.clipboard.writeText"), "Documents Copy link must write the elastos:// URI to the clipboard");
assert(!documents.includes("Open elastos://"), "Documents must not label published URIs as an Open action");
assert(documents.includes("confirmInCapsule"), "Documents destructive actions must use in-surface confirmation");
assert(!documents.includes("window.confirm"), "Documents must not use browser confirm for destructive actions");
assert(!documents.includes("object-uri-pill"), "Documents shell must not render a duplicate document URI pill");
assert(!documents.includes("published-pill"), "Documents shell must not render a duplicate published CID pill");
assert(!documents.includes("document-list-badge"), "Documents list must not use a text Published badge");
assert(documents.includes("document-list-published-icon"), "Documents list must show published state with an icon");
assert(documents.includes("document-list-item.published"), "Documents published rows must be visually distinct");
assert(documents.includes('aria-label="New document"'), "Documents create control must keep an accessible label");
assert(documents.includes("sidebar-controls"), "Documents create/search controls must share one compact row");
assert(!documents.includes('id="documents-count"'), "Documents sidebar must not render a duplicate document count");
assert(!documents.includes("sidebar-meta-label"), "Documents sidebar must not render duplicate section labels");
assert(!documents.includes("Start writing, then save."), "Documents must not show redundant draft instruction copy");
assert(!documents.includes("meta-pill"), "Documents must not use generic pill chrome for document state");
assert(!documents.includes("local-state-chip"), "Documents shell must not render duplicate draft state under the title");
assert(!documents.includes("updated-text"), "Documents shell must not render duplicate last-saved text under the title");
assert(!documents.includes("Delete document?"), "Documents delete confirmation must not repeat a modal title");
assert(documents.includes('class="action-primary action-icon-button"'), "Documents primary action must use compact icon buttons");
assert(documents.includes('aria-label="Save"'), "Documents Save icon button must keep an accessible label");
assert(documents.includes('aria-label="Hide list"'), "Documents Hide list icon button must keep an accessible label");
assert(documents.includes(".page-shell {\n    padding: 0;"), "Documents mobile shell must not add an extra outer gutter");
assert(documents.includes(".documents-main,\n  .share-main {\n    padding: 0.38rem;"), "Documents mobile panels must use compact padding");
assert(inbox.includes("button.dataset.actionId = actionId;"), "Inbox actions must expose stable action ids");
assert(inbox.includes("home:open-target"), "Inbox source-app opens must use Home orchestration");
assert(inbox.includes("min-height: 100dvh;"), "Inbox must use dynamic viewport height");
assert(inbox.includes("padding: 4px;") && inbox.includes("border-radius: 14px;"), "Inbox mobile panels must use compact Home-aligned spacing");
assert(library.includes("home:open-target"), "Library opens must use Home orchestration");
assert(library.includes("home:deliver-to-target"), "Library picker returns must use Home orchestration");
assert(library.includes("home:close-self"), "Library picker must close itself through Home after a successful attach");
assert(library.includes("chat-room:attach-library-item"), "Library must return selected documents using the Chat Room attach contract");
assert(library.includes("Publish the document before attaching it."), "Library must fail clearly when a draft is selected for Chat Room attachment");
assert(library.includes("data-attach-uri"), "Library attach mode must expose published URI selection state");
assert(!library.includes("entry-details"), "Library cards must not expose raw technical detail drawers");
assert(!library.includes("Working copy") && !library.includes("Published revision"), "Library cards must not show raw storage addresses by default");
assert(library.includes("min-height: 100dvh;"), "Library must use dynamic viewport height");
assert(library.includes(".toolbar {\n        display: grid;"), "Library toolbar must stack on narrow screens");
assert(library.includes("padding: 4px;") && library.includes("border-radius: 14px;"), "Library mobile panels must use compact Home-aligned spacing");
assert(chatStyle.includes("width: 100%;") && chatStyle.includes("padding-top: 0.35rem;"), "Chat Room mobile shell must avoid nested browser gutters");
assert(chatStyle.includes("border-radius: 0.82rem;") && chatStyle.includes("padding: 0.48rem;"), "Chat Room mobile cards must use compact Home-aligned spacing");
assert(gba.includes('aria-label="D-pad up, keyboard Arrow Up"'), "GBA directional controls must expose keyboard mapping labels");
assert(gba.includes('aria-label="Save state slot 1"'), "GBA save slots must expose slot-specific labels");
assert(gba.includes('aria-label="Load state slot 1"'), "GBA load slots must expose slot-specific labels");
assert(gba.includes("Insert Game"), "GBA empty state must use the concise Insert Game copy");
assert(gbaStyle.includes("--control-size: clamp(2.75rem, 13vw, 3.25rem);"), "GBA mobile d-pad buttons must stay touch-sized");
assert(gbaStyle.includes("grid-template-areas:") && gbaStyle.includes('"left select start right"') && gbaStyle.includes('"dpad dpad actions actions"'), "GBA mobile controls must place Select/Start in the L/R row");
assert(gbaStyle.includes(".shoulder-buttons,\n  .controls-row {\n    display: contents;"), "GBA mobile controls must let the full controller share one grid");
assert(gbaStyle.includes("#btn-select {\n    grid-area: select;") && gbaStyle.includes("#btn-start {\n    grid-area: start;"), "GBA mobile Select/Start must be direct grid items in the shoulder row");
assert(gbaStyle.includes("grid-area: left;\n    width: 100%;") && gbaStyle.includes("grid-area: right;\n    width: 100%;"), "GBA mobile L/R controls must be full shoulder targets, not content-width dots");
assert(gbaStyle.includes("#screen-container:focus"), "GBA screen focus must not show a browser outline");
assert(gbaStyle.includes("grid-template-rows: auto auto;"), "GBA mobile screen must not be starved by a flexible row");
assert(gbaStyle.includes("touch-action: none;"), "GBA virtual controls must own touch gestures");
assert(gbaStyle.includes("width: max-content;"), "GBA mobile collapsed Options must be a small centered Show control");
assert(gbaStyle.includes("max-height: min(8.25rem, 22dvh);"), "GBA mobile expanded Options must stay compact");
assert(gbaStyle.includes("grid-template-columns: repeat(3, minmax(0, 1fr));"), "GBA mobile save slots must use one row");
assert(gbaStyle.includes(".shell {\n    width: 100%;\n    padding: 0.2rem;"), "GBA mobile shell must not waste viewport on outer gutters");
assert(gbaStyle.includes(".screen-card {\n    grid-template-rows: auto auto;\n    align-content: start;\n    padding: 0.38rem;"), "GBA mobile screen card must keep compact chrome");
assert(gbaJs.includes("activeInputPointers"), "GBA touch controls must track pointer-specific presses");
assert(gbaJs.includes("pointerdown") && gbaJs.includes("pointerup"), "GBA controls must use a single pointer-event input path");
assert(!gbaJs.includes("touchstart") && !gbaJs.includes("mousedown"), "GBA controls must not mix touch and mouse input handlers");
assert(gbaJs.includes("syncUtilityDefaultForViewport"), "GBA Options must collapse automatically on compact viewports");
assert(gbaJs.includes("assertEmulatorRuntimeSupported"), "GBA startup must preflight threaded WebAssembly support before mGBA init");
assert(gbaJs.includes("EMULATOR_INIT_TIMEOUT_MS"), "GBA startup must fail visibly instead of hanging during mGBA init");
assert(gbaJs.includes("SharedArrayBuffer"), "GBA startup must explicitly guard WebAssembly thread requirements");
assert(gbaJs.includes("This device cannot run the current GBA engine"), "GBA unsupported-runtime copy must explain WebAssembly thread requirements");
assert(gbaJs.includes("Insert Game") && !gbaJs.includes("Choose an installed ROM"), "GBA runtime copy must stay concise");
assert(gbaScript.includes('echo "  X              A button"'), "GBA launcher help must match the emulator X -> A mapping");
assert(gbaScript.includes('echo "  Z              B button"'), "GBA launcher help must match the emulator Z -> B mapping");
assert(!gbaScript.includes('echo "  Z              A button"') && !gbaScript.includes('echo "  X              B button"'), "GBA launcher help must not preserve the old inverted action mapping");
assert(!system.includes("<dt>Overlay</dt>"), "System overlay controls must live inside the Background box");
assert(system.includes('class="system-inline-row background-actions"'), "System background image actions must stay in one row");
assert(system.includes("background-overlay-panel"), "System overlay controls must be integrated into the Background control panel");
assert(system.indexOf('id="background-preview"') < system.indexOf('id="background-overlay"'), "System background preview and overlay controls must share one field flow");
assert(systemStyle.includes("body {\n    padding: 0.2rem;") && systemStyle.includes("padding: 0.48rem;"), "System mobile panel must use compact Home-aligned spacing");

const principles = read("PRINCIPLES.md");
const architecture = read("docs/ARCHITECTURE.md");
const designSystem = read("docs/DESIGN_SYSTEM.md");
const commandMatrix = read("docs/COMMAND_MATRIX.md");
const tasks = read("TASKS.md");
const shellSmoke = read("scripts/home-camofox-smoke.mjs");
const runtimeChecklist = read("docs/RUNTIME_REPO_USER_STORY_CHECKLIST.md");
assertMarkdownLocalLinksResolve();
assertMarkdownScriptReferencesResolve();
assert(!tasks.includes("- [x]"), "TASKS.md must contain open work only; completed work belongs in elastos/CHANGELOG.md");
assert(!runtimeChecklist.includes("Shared is useful"), "Runtime checklist must not point reviewers at the retired Shared Home app");
assert(!runtimeChecklist.includes("public-install-update-smoke.sh"), "Runtime checklist must not reference retired public install update proof script");
assert(!runtimeChecklist.includes("public-linux-runtime-portability-smoke.sh"), "Runtime checklist must not reference retired public Linux portability proof script");
assert(principles.includes("every visible user action should map to the same capability-scoped operation"), "Principles must define human/agent action equality");
assert(architecture.includes("Interaction equality is part of the same rule"), "Architecture must define interaction equality");
assert(designSystem.includes("Every visible action must have the same contract for humans and agents"), "Design system must define human/agent interaction contract");
assert(commandMatrix.includes("Host-side provider bridge commands are explicit operator tooling, not app-capsule authority."), "Command matrix must distinguish host provider tooling from app-capsule authority");
assert(!commandMatrix.includes("direct IPFS bridge"), "Command matrix must not normalize direct IPFS bridge language");
assert(!commandMatrix.includes("IPFS bridge"), "Command matrix must describe ipfs-provider explicitly, not an ambiguous IPFS bridge");
const commandRules = commandMatrix.match(/## Rules\n([\s\S]*?)\n## Future:/)?.[1] || "";
const commandRuleNumbers = [...commandRules.matchAll(/^(\d+)\./gm)].map((match) => Number(match[1]));
assert(commandRuleNumbers.length > 0, "Command matrix must keep a numbered Rules section");
for (const [index, number] of commandRuleNumbers.entries()) {
  assert(number === index + 1, "Command matrix ordered rules must stay sequential", commandRuleNumbers);
}
assert(
  !/!\s*doc\.querySelector\([^)]*\)\?\.classList\.contains/.test(shellSmoke),
  "Smoke checks must not treat missing DOM nodes as visible with !optional chaining",
);

console.log("PASS home entropy check");
