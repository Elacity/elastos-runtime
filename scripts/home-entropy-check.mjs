#!/usr/bin/env node

import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = new URL("../", import.meta.url);
const repoRootPath = fileURLToPath(repoRoot);

function read(path) {
  return readFileSync(new URL(path, repoRoot), "utf8");
}

function readAll(paths) {
  return paths.map((path) => read(path)).join("\n");
}

function readBytes(path) {
  return readFileSync(new URL(path, repoRoot));
}

function pngDimensions(path) {
  const bytes = readBytes(path);
  const pngSignature = "89504e470d0a1a0a";
  assert(
    bytes.subarray(0, 8).toString("hex") === pngSignature,
    `${path} must be a PNG image`,
  );
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

function includesNormalized(source, needle) {
  return source.replace(/\s+/g, " ").includes(needle.replace(/\s+/g, " "));
}

function assert(condition, message, details = undefined) {
  if (!condition) {
    const suffix = details ? `\n${JSON.stringify(details, null, 2)}` : "";
    throw new Error(`${message}${suffix}`);
  }
}

function sourceBlock(source, needle, label) {
  const start = source.indexOf(needle);
  assert(start >= 0, `${label} must exist`);
  const open = source.indexOf("{", start);
  assert(open >= 0, `${label} must have a body`);
  let depth = 0;
  for (let index = open; index < source.length; index += 1) {
    const char = source[index];
    if (char === "{") {
      depth += 1;
    } else if (char === "}") {
      depth -= 1;
      if (depth === 0) {
        return source.slice(start, index + 1);
      }
    }
  }
  throw new Error(`${label} body is not balanced`);
}

function assertProtectedPrincipalRootAccessor(source, needle, helper, label) {
  const block = sourceBlock(source, needle, label);
  assert(block.includes(helper), `${label} must use ${helper}`);
  const forbidden = [
    "std::fs::read(",
    "std::fs::read_to_string(",
    "tokio::fs::read(",
    "std::fs::write(",
    "tokio::fs::write(",
    "atomic_write(",
  ].filter((pattern) => pattern !== helper);
  const hits = forbidden.filter((pattern) => block.includes(pattern));
  assert(
    hits.length === 0,
    `${label} must not bypass protected principal-root object helpers`,
    hits,
  );
}

function listMarkdownFiles(dir = repoRootPath) {
  const entries = readdirSync(dir, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    if (
      entry.name === ".git" ||
      entry.name === "target" ||
      entry.name === "node_modules"
    ) {
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

function listTextFiles(dir) {
  const entries = readdirSync(dir, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    if (
      entry.name === ".git" ||
      entry.name === "target" ||
      entry.name === "node_modules"
    ) {
      continue;
    }
    const full = resolve(dir, entry.name);
    if (entry.isDirectory()) {
      files.push(...listTextFiles(full));
    } else if (
      entry.isFile() &&
      /\.(rs|json|md|html|js|css|toml)$/.test(entry.name)
    ) {
      files.push(full);
    }
  }
  return files;
}

function assertProviderOperationEnumsRejectUnknownFields() {
  const unguarded = listTextFiles(repoRootPath)
    .filter((file) => file.endsWith(".rs"))
    .filter((file) =>
      readFileSync(file, "utf8").includes(
        '#[serde(tag = "op", rename_all = "snake_case")]',
      ),
    )
    .map((file) => file.slice(repoRootPath.length + 1));
  assert(
    unguarded.length === 0,
    "Provider operation enums must use deny_unknown_fields so hidden authority fields fail closed",
    unguarded,
  );
}

function assertGatewayRequestStructsRejectUnknownFields() {
  const names = [
    "HomeBrowserStateUpdate",
    "SystemHandleUpdateRequest",
    "SystemBackgroundOverlayRequest",
    "SystemGuestRegistrationRequest",
    "WalletApprovalRejectRequest",
    "WalletApprovalApproveRequest",
    "WalletApprovalCompleteRequest",
    "SystemWalletManagedCreateRequest",
    "SystemWalletDefaultRequest",
    "HomeLaunchRequest",
    "InboxActionRequest",
    "RoomPollBody",
    "RoomSendBody",
    "ChatRoomAccessPolicyBody",
    "ChatRoomMemberInviteBody",
    "ChatRoomMemberRemoveBody",
    "ChatRoomInviteRevokeBody",
    "RoomUploadStartBody",
    "CapsuleInterfaceInvokeRequest",
  ];
  const missing = names.filter((name) => {
    const pattern = new RegExp(
      `#\\[serde\\(deny_unknown_fields\\)\\]\\s*(?:pub\\([^)]*\\)\\s+)?struct ${name}\\b`,
    );
    return !pattern.test(gatewayApi);
  });
  assert(
    missing.length === 0,
    "Browser-facing gateway request bodies must reject hidden authority fields at decode time",
    missing,
  );
  assert(
    gatewayTests.includes(
      "test_wallet_request_bodies_reject_hidden_authority_fields",
    ) &&
      gatewayTests.includes(
        "test_chat_request_bodies_reject_hidden_identity_fields",
      ) &&
      gatewayApi.includes(
        "capsule_interface_invoke_request_rejects_hidden_authority_fields",
      ),
    "Gateway request-body strictness must keep regression tests for wallet, chat, and capsule interface authority fields",
  );
}

function assertCapabilityRequestStructsRejectUnknownFields() {
  const names = [
    "RequestCapabilityInput",
    "GrantRequestInput",
    "DenyRequestInput",
    "RevokeAllInput",
    "AuditLogQuery",
  ];
  const missing = names.filter((name) => {
    const pattern = new RegExp(
      `#\\[serde\\(deny_unknown_fields\\)\\]\\s*pub struct ${name}\\b`,
    );
    return !pattern.test(capabilityHandler);
  });
  assert(
    missing.length === 0,
    "Capability API request bodies must reject hidden authority fields at decode time",
    missing,
  );
  assert(
    capabilityHandler.includes(
      "test_capability_inputs_reject_hidden_authority_fields",
    ),
    "Capability API request-body strictness must keep regression tests",
  );
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

function assertUsersSelfReferencesAreApproved() {
  const allowed = new Set([
    "capsules/chat/capsule.json",
    "capsules/chat/src/carrier.rs",
    "capsules/chat/src/session.rs",
    "capsules/browser-engine-adapter/src/main.rs",
    "capsules/browser-engine-adapter/src/tests.rs",
    "capsules/browser-engine-adapter/src/validation.rs",
    "capsules/gba-emulator/capsule.json",
    "capsules/gba-ucity/capsule.json",
    "elastos/crates/elastos-server/src/api/browser_capsules.rs",
    "elastos/crates/elastos-server/src/api/gateway_browser.rs",
    "elastos/crates/elastos-server/src/api/gateway_browser_tests.rs",
    "elastos/crates/elastos-server/src/api/gateway_tests/documents.rs",
    "elastos/crates/elastos-server/src/api/gateway_tests/browser_profile.rs",
    "elastos/crates/elastos-server/src/api/gateway_tests/home_system.rs",
    "elastos/crates/elastos-server/src/api/gateway_tests/mod.rs",
    "elastos/crates/elastos-server/src/api/gateway_tests/support_providers.rs",
    "elastos/crates/elastos-server/src/api/gateway_tests/support_runtime.rs",
    "elastos/crates/elastos-server/src/api/handlers/storage.rs",
    "elastos/crates/elastos-server/src/api/viewer_gateway.rs",
    "elastos/crates/elastos-server/src/carrier_bridge.rs",
    "elastos/crates/elastos-server/src/notifications.rs",
    "elastos/crates/elastos-server/src/runtime_control.rs",
  ]);
  const files = [
    ...listTextFiles(resolve(repoRootPath, "capsules")),
    ...listTextFiles(
      resolve(repoRootPath, "elastos/crates/elastos-server/src"),
    ),
  ];
  const unexpected = files
    .map((file) => file.slice(repoRootPath.length).replaceAll("\\", "/"))
    .filter((file) => read(file).includes("Users/self") && !allowed.has(file));
  assert(
    unexpected.length === 0,
    "`Users/self` may only appear in approved scoped-alias code/tests",
    unexpected,
  );
}

function assertMarkdownScriptReferencesResolve() {
  const failures = [];
  for (const file of listMarkdownFiles()) {
    const source = readFileSync(file, "utf8");
    const relativeFile = file.slice(repoRootPath.length);
    for (const match of source.matchAll(
      /(?:^|[^A-Za-z0-9_./-])(scripts\/[A-Za-z0-9_./-]+(?:\.sh|\.mjs))/g,
    )) {
      const script = match[1];
      const resolved = resolve(repoRootPath, script);
      if (!existsSync(resolved) || !statSync(resolved).isFile()) {
        failures.push(`${relativeFile}: ${script}`);
      }
    }
  }
  assert(
    failures.length === 0,
    "Markdown script references must point to existing scripts",
    failures,
  );
}

function listFilesRecursive(dir) {
  if (!existsSync(dir)) {
    return [];
  }
  const entries = readdirSync(dir, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    if (
      entry.name === ".git" ||
      entry.name === "target" ||
      entry.name === "node_modules"
    ) {
      continue;
    }
    const full = resolve(dir, entry.name);
    if (entry.isDirectory()) {
      files.push(...listFilesRecursive(full));
    } else if (entry.isFile()) {
      files.push(full);
    }
  }
  return files;
}

function isTestOrGeneratedPath(path) {
  const parts = path.slice(repoRootPath.length).split(/[\\/]/);
  const name = parts.at(-1)?.toLowerCase() || "";
  return (
    parts.includes("target") ||
    parts.includes("tests") ||
    parts.includes("test") ||
    parts.includes("__tests__") ||
    name.includes("test") ||
    name.includes("spec")
  );
}

function relativeToRepo(path) {
  return path.slice(repoRootPath.length);
}

function assertOrdinaryCapsulesDoNotReferenceRawBlockchainAuthority() {
  const allowedRoles = new Set([
    "shell",
    "app",
    "viewer",
    "provider",
    "content",
  ]);
  const ordinaryRoles = new Set(["app", "viewer", "content"]);
  // System is the runtime-owned approval/diagnostic surface. Dedicated wallet
  // connector capsules and the Browser shell are privileged adapter UIs, not
  // general app authority.
  const privilegedOrdinaryAuthorityUi = new Set([
    "system",
    "wallet-metamask",
    "wallet-unisat",
    "wallet",
    "wallet-walletconnect",
    "browser",
  ]);
  const forbiddenAuthorityPatterns = new Map([
    ["elastos://chain", "raw chain provider namespace"],
    ["elastos://net", "raw Browser/Net provider namespace"],
    ["elastos://exit", "raw Browser Exit provider namespace"],
    ["elastos://wallet", "raw wallet provider namespace"],
    ["/api/provider/chain", "direct chain provider route"],
    ["/api/provider/net", "direct Browser/Net provider route"],
    ["/api/provider/exit", "direct Browser Exit provider route"],
    ["/api/provider/wallet", "direct wallet provider route"],
    ["chain-provider", "raw chain backend provider"],
    ["net-provider", "raw Browser/Net backend provider"],
    ["exit-provider", "raw Browser Exit backend provider"],
    ["wallet-provider", "raw wallet backend provider"],
    ["WalletConnect", "direct browser wallet adapter authority"],
    ["walletconnect", "direct browser wallet adapter authority"],
    ["MetaMask", "direct browser wallet adapter authority"],
    ["metamask", "direct browser wallet adapter authority"],
    ["UniSat", "direct browser wallet adapter authority"],
    ["unisat", "direct browser wallet adapter authority"],
    ["window.ethereum", "direct injected wallet authority"],
    ["window.unisat", "direct injected wallet authority"],
    ["ethereum.request", "direct injected wallet authority"],
    ["signMessage", "direct wallet signing authority"],
    ["personal_sign", "direct wallet signing authority"],
    ["eth_requestAccounts", "direct wallet account authority"],
    ["eth_sendTransaction", "direct wallet transaction authority"],
    ["wallet_switchEthereumChain", "direct wallet chain-switch authority"],
    ["rpc_url", "raw RPC endpoint authority"],
    ["RPC_URL", "raw RPC endpoint authority"],
    ["JSON-RPC", "raw RPC protocol authority"],
    ["jsonrpc", "raw RPC protocol authority"],
    ["eth_call", "raw EVM RPC authority"],
    ["eth_chainId", "raw EVM RPC authority"],
    ["bitcoin-cli", "raw node CLI authority"],
    ["bitcoind", "raw node daemon authority"],
    ["Bitcoin Core RPC", "raw node RPC authority"],
    ["blockchain provider", "raw blockchain provider authority"],
  ]);
  const sourceExtensions = new Set([
    ".html",
    ".rs",
    ".ts",
    ".tsx",
    ".js",
    ".mjs",
  ]);
  const manifests = [
    ...listFilesRecursive(resolve(repoRootPath, "capsules")).filter((path) =>
      path.endsWith("/capsule.json"),
    ),
    ...listFilesRecursive(resolve(repoRootPath, "elastos/capsules")).filter(
      (path) => path.endsWith("/capsule.json"),
    ),
  ].sort();
  const failures = [];

  for (const manifestPath of manifests) {
    const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
    const role = manifest.role;
    const name = manifest.name || relativeToRepo(manifestPath);
    assert(
      allowedRoles.has(role),
      `${relativeToRepo(manifestPath)} has unknown capsule role: ${role}`,
    );
    if (!ordinaryRoles.has(role) || privilegedOrdinaryAuthorityUi.has(name)) {
      continue;
    }

    const manifestText = readFileSync(manifestPath, "utf8");
    for (const [pattern, reason] of forbiddenAuthorityPatterns) {
      if (name === "home" && reason === "direct browser wallet adapter authority") {
        continue;
      }
      if (manifestText.includes(pattern)) {
        failures.push(`${relativeToRepo(manifestPath)}: ${reason}`);
      }
    }

    const capsuleRoot = dirname(manifestPath);
    for (const source of listFilesRecursive(capsuleRoot)) {
      if (source === manifestPath || isTestOrGeneratedPath(source)) {
        continue;
      }
      if (
        !sourceExtensions.has(source.match(/\.[^.]+$/)?.[0] || "") ||
        source.endsWith("/mgba.js")
      ) {
        continue;
      }
      const sourceText = readFileSync(source, "utf8");
      for (const [pattern, reason] of forbiddenAuthorityPatterns) {
        if (name === "home" && reason === "direct browser wallet adapter authority") {
          continue;
        }
        if (sourceText.includes(pattern)) {
          failures.push(`${relativeToRepo(source)}: ${reason}`);
        }
      }
    }
  }

  assert(
    failures.length === 0,
    "Ordinary app/viewer/content capsules must not reference raw wallet, chain, node, RPC, WalletConnect, MetaMask, or blockchain provider authority",
    failures,
  );
}

function assertToken(source, file, token, value) {
  const pattern = new RegExp(
    `${escapeRegExp(token)}\\s*:\\s*${escapeRegExp(value)}\\s*;`,
  );
  assert(
    pattern.test(source),
    `${file} is missing canonical token ${token}: ${value}`,
  );
}

function stripTags(value) {
  return value
    .replace(/<[^>]+>/g, "")
    .replace(/\s+/g, " ")
    .trim();
}

function attributeValue(markup, name) {
  const match = new RegExp(`${name}\\s*=\\s*["']([^"']*)["']`, "i").exec(
    markup,
  );
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
    const isHidden =
      hasAttribute(markup, "hidden") ||
      attributeValue(markup, "aria-hidden") === "true";
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
  "capsules/home/browser/home-shell-host.js",
  "capsules/home-gui/browser/shell-surface.js",
  "capsules/home-gui/browser/shell-windows.js",
  "capsules/home/browser/service-worker.js",
  "capsules/system/browser/index.html",
  "capsules/system/browser/system.js",
  "capsules/system/browser/style.css",
  "capsules/wallet-metamask/browser/index.html",
  "capsules/wallet-metamask/browser/wallet-metamask.js",
  "capsules/wallet-metamask/browser/style.css",
  "capsules/wallet-unisat/browser/index.html",
  "capsules/wallet-unisat/browser/wallet-unisat.js",
  "capsules/wallet-unisat/browser/style.css",
  "capsules/wallet/browser/index.html",
  "capsules/wallet/browser/wallet.js",
  "capsules/wallet/browser/wallet-account-actions.js",
  "capsules/wallet/browser/wallet-activity.js",
  "capsules/wallet/browser/wallet-api.js",
  "capsules/wallet/browser/wallet-create-account-flow.js",
  "capsules/wallet/browser/wallet-flows.js",
  "capsules/wallet/browser/wallet-format.js",
  "capsules/wallet/browser/wallet-preferences.js",
  "capsules/wallet/browser/wallet-receive-flow.js",
  "capsules/wallet/browser/wallet-requests.js",
  "capsules/wallet/browser/wallet-render.js",
  "capsules/wallet/browser/wallet-state.js",
  "capsules/wallet/browser/style.css",
  "capsules/browser/browser/index.html",
  "capsules/browser/browser/browser.js",
  "capsules/browser/browser/browser-clipboard.js",
  "capsules/browser/browser/browser-history.js",
  "capsules/browser/browser/browser-input.js",
  "capsules/browser/browser/browser-input-surface.js",
  "capsules/browser/browser/browser-location.js",
  "capsules/browser/browser/browser-remote-display.js",
  "capsules/browser/browser/browser-runtime-api.js",
  "capsules/browser/browser/browser-status.js",
  "capsules/browser/browser/browser-webrtc.js",
  "capsules/browser/browser/style.css",
  "capsules/documents/browser/index.html",
  "capsules/inbox/browser/index.html",
  "capsules/library/browser/index.html",
  "capsules/library/browser/library.css",
  "capsules/library/browser/src/actions.js",
  "capsules/library/browser/src/api.js",
  "capsules/chat-room/browser/index.html",
  "capsules/chat-room/browser/style.css",
  "capsules/gba-emulator/browser/index.html",
  "capsules/gba-emulator/browser/style.css",
  "capsules/gba-emulator/browser/emulator.js",
];

const activeHtmlFiles = [
  "capsules/home/browser/index.html",
  "capsules/system/browser/index.html",
  "capsules/wallet-metamask/browser/index.html",
  "capsules/wallet-unisat/browser/index.html",
  "capsules/wallet/browser/index.html",
  "capsules/browser/browser/index.html",
  "capsules/documents/browser/index.html",
  "capsules/inbox/browser/index.html",
  "capsules/library/browser/index.html",
  "capsules/chat-room/browser/index.html",
  "capsules/gba-emulator/browser/index.html",
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
    assert(
      !source.includes(phrase),
      `${file} still contains stale UI copy: ${phrase}`,
    );
  }
}

for (const file of activeHtmlFiles) {
  assertStaticControlsAreNamed(file);
}

const lightTokenFiles = [
  "capsules/chat-room/browser/style.css",
  "capsules/gba-emulator/browser/style.css",
  "capsules/documents/browser/index.html",
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

const inboxStyle = read("capsules/inbox/browser/index.html");
for (const [token, value] of new Map([
  ["--bg", "#ffffff"],
  ["--sidebar-bg", "#f9f9f9"],
  ["--toolbar-bg", "#fafafa"],
  ["--panel", "#ffffff"],
  ["--panel-soft", "#f9f9f9"],
  ["--line", "#e5e7eb"],
  ["--line-strong", "#d1d5db"],
  ["--ink", "#1f2937"],
  ["--muted", "#6b7280"],
  ["--brand", "#f6921a"],
  ["--accent", "#007aff"],
  ["--accent-soft", "#e5f0ff"],
])) {
  assertToken(inboxStyle, "capsules/inbox/browser/index.html", token, value);
}

const systemSettingsStyle = read("capsules/system/browser/style.css");
for (const [token, value] of new Map([
  ["--color-settings-bg", "#ffffff"],
  ["--color-settings-sidebar", "#f9f9f9"],
  ["--color-settings-card", "#ffffff"],
  ["--color-bg-tertiary", "#f3f4f6"],
  ["--color-text-primary", "#1f2937"],
  ["--color-text-secondary", "#4b5563"],
  ["--color-text-muted", "#6b7280"],
  ["--color-border", "#e5e7eb"],
  ["--color-border-light", "#d1d5db"],
  ["--color-input-bg", "#ffffff"],
  ["--color-input-border", "#d1d5db"],
  ["--color-input-text", "#1f2937"],
])) {
  assertToken(systemSettingsStyle, "capsules/system/browser/style.css", token, value);
}

const libraryStyle = read("capsules/library/browser/library.css");
for (const [token, value] of new Map([
  ["--bg", "#f6f7f9"],
  ["--sidebar-bg", "#f0f1f4"],
  ["--panel", "#ffffff"],
  ["--panel-soft", "#f3f4f6"],
  ["--line", "rgba(60, 60, 67, 0.14)"],
  ["--ink", "#1d1d1f"],
  ["--muted", "#6b6b6b"],
  ["--brand", "#f6921a"],
  ["--accent", "#007aff"],
])) {
  assertToken(libraryStyle, "capsules/library/browser/library.css", token, value);
}

const shellStyle = [
  read("capsules/home/browser/style.css"),
  read("capsules/home-gui/browser/style.css"),
].join("\n");
assertToken(
  shellStyle,
  "capsules/home/browser/style.css",
  "--brand",
  "#f6921a",
);
assertToken(
  shellStyle,
  "capsules/home/browser/style.css",
  "--brand-strong",
  "#ffb457",
);
assert(
  (shellStyle.match(/#f6921a/g) || []).length === 1,
  "Home brand orange should be defined once as --brand",
);
assert(
  (shellStyle.match(/#ffb457/g) || []).length === 1,
  "Home hover brand orange should be defined once as --brand-strong",
);
assert(
  shellStyle.includes("min-height: 100dvh;"),
  "Home must use dynamic viewport height for mobile browsers",
);
assert(
  shellStyle.includes("env(safe-area-inset-top"),
  "Home chrome/window layout must respect mobile safe-area top inset",
);
assert(
  shellStyle.includes("env(safe-area-inset-bottom"),
  "Home chrome/window layout must respect mobile safe-area bottom inset",
);
assert(
  shellStyle.includes("max-height: calc(100dvh - 54px);"),
  "Home context menu must stay inside short viewports",
);
assert(
  shellStyle.includes(".taskbar-sortable::-webkit-scrollbar"),
  "Home taskbar must remain scroll-safe on narrow screens",
);
assert(
  shellStyle.includes('.window[data-maximized="true"]') &&
    shellStyle.includes("inset: 0 !important;"),
  "Home maximized windows must own the full viewport",
);
assert(
  shellStyle.includes('.window[data-maximized="true"].window-active'),
  "Home active maximized windows must stack above Home chrome",
);

const shellIndex = readAll([
  "capsules/home/browser/index.html",
  "capsules/home-gui/browser/home-gui-template.html",
]);
const shellManifest = JSON.parse(
  read("capsules/home/browser/manifest.webmanifest"),
);
const shellServiceWorker = read("capsules/home/browser/service-worker.js");
const shellSurface = read("capsules/home-gui/browser/shell-surface.js");
const shellJs = readAll([
  "capsules/home/browser/home-shell-host.js",
  "capsules/home-gui/browser/home-gui.js",
  "capsules/home-gui/browser/shell-core.js",
  "capsules/home-gui/browser/shell-chrome.js",
]);
const shellCore = readAll([
  "capsules/home/browser/shell-core.js",
  "capsules/home-gui/browser/shell-core.js",
]);
const shellWindows = read("capsules/home-gui/browser/shell-windows.js");
const homeShellRegressionSmoke = read("scripts/home-shell-regression-smoke.mjs");
const servicesCapsule = read("capsules/services/capsule.json");
const peopleCapsule = read("capsules/people/capsule.json");
const peopleIndex = read("capsules/people/browser/index.html");
const peopleScript = read("capsules/people/browser/people.js");
const peopleStyle = read("capsules/people/browser/style.css");
const servicesIndex = read("capsules/services/browser/index.html");
const servicesScript = read("capsules/services/browser/services.js");
const servicesStyle = read("capsules/services/browser/style.css");
const shellWindowGeometry = read(
  "capsules/home-gui/browser/shell-window-geometry.js",
);
const shellCmd = read("elastos/crates/elastos-server/src/shell_cmd.rs");
const homeCmd = read("elastos/crates/elastos-server/src/home_cmd.rs");
const homeCli = read("capsules/home-cli/src/main.rs");
const chatCarrier = read("capsules/chat/src/carrier.rs");
const carrierService = read("elastos/crates/elastos-server/src/carrier_service.rs");
const localhostProvider = read("elastos/capsules/localhost-provider/src/main.rs");
const operatorControl = read(
  "elastos/crates/elastos-server/src/operator_control.rs",
);
const agentsContract = read("AGENTS.md");
const linuxSourceHomeRestart = read("scripts/linux-source-home-restart.sh");
const linuxSourceHomeRestartSmoke = read(
  "scripts/linux-source-home-restart-smoke.sh",
);
const chatRoomUi = read("capsules/chat-room-ui/src/lib.rs");
const roomService = read("elastos/crates/elastos-server/src/room_service.rs");
const gatewayApi = readAll([
  "elastos/crates/elastos-server/src/api/gateway.rs",
  "elastos/crates/elastos-server/src/api/gateway_home_runtime.rs",
  "elastos/crates/elastos-server/src/api/gateway_home_system.rs",
  "elastos/crates/elastos-server/src/api/gateway_home_token.rs",
  "elastos/crates/elastos-server/src/api/gateway_inbox.rs",
  "elastos/crates/elastos-server/src/api/gateway_inspect_actions.rs",
  "elastos/crates/elastos-server/src/api/gateway_models.rs",
  "elastos/crates/elastos-server/src/api/gateway_capsule_catalog.rs",
  "elastos/crates/elastos-server/src/api/gateway_provider_proxy.rs",
  "elastos/crates/elastos-server/src/api/gateway_room.rs",
  "elastos/crates/elastos-server/src/api/gateway_server.rs",
  "elastos/crates/elastos-server/src/api/gateway_site.rs",
  "elastos/crates/elastos-server/src/api/gateway_wallet.rs",
  "elastos/crates/elastos-server/src/api/gateway_wallet_accounts.rs",
  "elastos/crates/elastos-server/src/api/gateway_wallet_app.rs",
  "elastos/crates/elastos-server/src/api/gateway_wallet_approvals.rs",
  "elastos/crates/elastos-server/src/api/gateway_wallet_connectors.rs",
  "elastos/crates/elastos-server/src/api/gateway_wallet_prices.rs",
  "elastos/crates/elastos-server/src/api/gateway_wallet_send.rs",
]);
const gatewayHomeSystemTests = read(
  "elastos/crates/elastos-server/src/api/gateway_tests/home_system.rs",
);
const gatewayInboxApi = read("elastos/crates/elastos-server/src/api/gateway_inbox.rs");
const gatewayWalletAppApi = read("elastos/crates/elastos-server/src/api/gateway_wallet_app.rs");
const gatewayBrowserApi = readAll([
  "elastos/crates/elastos-server/src/api/gateway_browser.rs",
  "elastos/crates/elastos-server/src/api/gateway_browser_engine.rs",
  "elastos/crates/elastos-server/src/api/gateway_browser_response.rs",
  "elastos/crates/elastos-server/src/api/gateway_browser_stream.rs",
  "elastos/crates/elastos-server/src/api/gateway_browser_validation.rs",
  "elastos/crates/elastos-server/src/api/gateway_browser_wallet.rs",
  "elastos/crates/elastos-server/src/api/gateway_browser_wallet_bridge.rs",
  "elastos/crates/elastos-server/src/api/gateway_browser_wallet_reads.rs",
]);
const authGatewayApi = read(
  "elastos/crates/elastos-server/src/api/auth_gateway.rs",
);
const recoveryKitLiveSmoke = read("scripts/recovery-kit-live-smoke.sh");
const browserCapsulesApi = read(
  "elastos/crates/elastos-server/src/api/browser_capsules.rs",
);
const viewerGatewayApi = read(
  "elastos/crates/elastos-server/src/api/viewer_gateway.rs",
);
const archivePolicyDoc = read("docs/ARCHIVE_POLICY.md");
const browserVmTargetDoc = read("docs/BROWSER_VM_TARGET.md");
const documentsProvider = read(
  "elastos/crates/elastos-server/src/documents.rs",
);
const documentsReadme = read("capsules/documents/browser/README.md");
const authContract = read("elastos/crates/elastos-auth/src/lib.rs");
const webauthnIdentity = read(
  "elastos/crates/elastos-identity/src/webauthn.rs",
);
const didProvider = read("capsules/did-provider/src/main.rs");
const didProviderManifest = read("capsules/did-provider/capsule.json");
const runtimeAuth = read("elastos/crates/elastos-server/src/auth.rs");
const apiRoutes = read("elastos/crates/elastos-server/src/api/routes.rs");
const supervisorApi = read(
  "elastos/crates/elastos-server/src/api/handlers/supervisor_api.rs",
);
const supervisorCore = read("elastos/crates/elastos-server/src/supervisor.rs");
const serveCmd = read("elastos/crates/elastos-server/src/serve_cmd.rs");
const storageHandler = read(
  "elastos/crates/elastos-server/src/api/handlers/storage.rs",
);
const identityHandler = read(
  "elastos/crates/elastos-server/src/api/handlers/identity.rs",
);
const capabilityHandler = read(
  "elastos/crates/elastos-server/src/api/handlers/capability.rs",
);
const providerResource = read(
  "elastos/crates/elastos-server/src/provider_resource.rs",
);
const inspectorProvider = readAll([
  "elastos/crates/elastos-server/src/inspect_provider/mod.rs",
  "elastos/crates/elastos-server/src/inspect_provider/sources.rs",
  "elastos/crates/elastos-server/src/inspect_provider/projection.rs",
  "elastos/crates/elastos-server/src/inspect_provider/planning.rs",
  "elastos/crates/elastos-server/src/inspect_provider/dispatch.rs",
]);
const inspectorCore = read("elastos/crates/elastos-runtime/src/inspect/mod.rs");
const capsuleInspectorDocs = read("docs/CAPSULE_INSPECTOR.md");
const inspectorTestingDocs = read("docs/INSPECTOR_TESTING.md");
const invokeCore = read("elastos/crates/elastos-runtime/src/invoke/mod.rs");
const vmProvider = read("elastos/crates/elastos-server/src/vm_provider.rs");
const notifications = read(
  "elastos/crates/elastos-server/src/notifications.rs",
);
const carrierBridge = read(
  "elastos/crates/elastos-server/src/carrier_bridge.rs",
);
const carrierRuntime = read("elastos/crates/elastos-server/src/carrier.rs");
const runtimeCore = read("elastos/crates/elastos-server/src/runtime.rs");
const runtimeControl = read(
  "elastos/crates/elastos-server/src/runtime_control.rs",
);
const serverInfra = read("elastos/crates/elastos-server/src/server_infra.rs");
const wasmProvider = read(
  "elastos/crates/elastos-compute/src/providers/component.rs",
);
const protectedContent = read(
  "elastos/crates/elastos-common/src/protected_content.rs",
);
const chainProvider = readAll([
  "capsules/chain-provider/src/main.rs",
  "capsules/chain-provider/src/abi.rs",
  "capsules/chain-provider/src/backends.rs",
  "capsules/chain-provider/src/config.rs",
  "capsules/chain-provider/src/lifecycle.rs",
  "capsules/chain-provider/src/protocol.rs",
  "capsules/chain-provider/src/rpc.rs",
  "capsules/chain-provider/src/validation.rs",
  "capsules/chain-provider/src/tests.rs",
  "capsules/chain-provider/src/tests/support.rs",
]);
const netProvider = read("capsules/net-provider/src/main.rs");
const exitProvider = read("capsules/exit-provider/src/main.rs");
const browserEngineAdapter = readAll([
  "capsules/browser-engine-adapter/src/main.rs",
  "capsules/browser-engine-adapter/src/display.rs",
  "capsules/browser-engine-adapter/src/ids.rs",
  "capsules/browser-engine-adapter/src/supervisor.rs",
  "capsules/browser-engine-adapter/src/validation.rs",
  "capsules/browser-engine-adapter/src/tests.rs",
]);
const browserEngineSupervisor = read(
  "elastos/tools/browser-engine-supervisor/src/main.rs",
);
const browserStreamBridge = read(
  "elastos/tools/browser-stream-bridge/src/main.rs",
);
const browserLocalExit = read("elastos/tools/browser-local-exit/src/main.rs");
const browserNativeHostCapability = read(
  "scripts/browser-native-host-capability.mjs",
);
const browserNativeOperatorConfig = read(
  "scripts/browser-native-operator-config.mjs",
);
const remoteCarrierExitOperatorReport = read(
  "scripts/remote-carrier-exit-operator-report.mjs",
);
const remoteCarrierExitOperatorReportSmoke = read(
  "scripts/remote-carrier-exit-operator-report-smoke.sh",
);
const remoteCarrierExitArtifactReadiness = read(
  "scripts/remote-carrier-exit-artifact-readiness.mjs",
);
const remoteCarrierExitArtifactReadinessSmoke = read(
  "scripts/remote-carrier-exit-artifact-readiness-smoke.sh",
);
const remoteCarrierExitReadiness = read(
  "scripts/remote-carrier-exit-readiness.mjs",
);
const remoteCarrierExitReadinessSmoke = read(
  "scripts/remote-carrier-exit-readiness-smoke.sh",
);
const remoteCarrierExitSourceConfig = read(
  "scripts/remote-carrier-exit-source-config.mjs",
);
const remoteCarrierExitSourceConfigSmoke = read(
  "scripts/remote-carrier-exit-source-config-smoke.sh",
);
const remoteCarrierExitPublicLivePlan = read(
  "scripts/remote-carrier-exit-public-live-plan.mjs",
);
const remoteCarrierExitPublicLivePlanSmoke = read(
  "scripts/remote-carrier-exit-public-live-plan-smoke.sh",
);
const carrierOnlyAuthorityCheck = read("scripts/carrier-only-authority-check.sh");
const browserNativeTargetPreflight = read(
  "scripts/browser-native-target-preflight.sh",
);
const browserNativeSupervisorSmoke = read(
  "scripts/browser-native-supervisor-smoke.sh",
);
const browserNativeSupervisorProxySmoke = read(
  "scripts/browser-native-supervisor-proxy-smoke.sh",
);
const browserHostedProductOperatorConfig = read(
  "scripts/browser-hosted-product-operator-config.mjs",
);
const browserHostedProductSupervisor = read(
  "scripts/browser-hosted-product-supervisor.mjs",
);
const browserHostedProductWebrtcSmoke = read(
  "scripts/browser-hosted-product-webrtc-smoke.mjs",
);
const browserHostedProductWebrtcSmokeShell = read(
  "scripts/browser-hosted-product-webrtc-smoke.sh",
);
const browserHostedProductNavigationSmoke = read(
  "scripts/browser-hosted-product-navigation-smoke.mjs",
);
const browserHostedProductNavigationSmokeShell = read(
  "scripts/browser-hosted-product-navigation-smoke.sh",
);
const browserHostedProductWalletSmoke = read(
  "scripts/browser-hosted-product-wallet-smoke.sh",
);
const browserHostedProductGlideWalletSmoke = read(
  "scripts/browser-hosted-product-glide-wallet-smoke.sh",
);
const browserKasmControlService = read(
  "scripts/browser-kasm-control-service.mjs",
);
const browserKasmControlServiceSmoke = read(
  "scripts/browser-kasm-control-service-smoke.sh",
);
const browserDisplayModeSmoke = read("scripts/browser-display-mode-smoke.mjs");
const browserHostedProviderCandidateSmoke = read(
  "scripts/browser-hosted-provider-candidate-smoke.sh",
);
const browserHostedProviderBakeoff = read(
  "scripts/browser-hosted-provider-bakeoff.sh",
);
const browserObjectiveAudit = read("scripts/browser-objective-audit.mjs");
const browserObjectiveAuditSmoke = read(
  "scripts/browser-objective-audit-smoke.sh",
);
const browserProviderDecisionReport = read(
  "scripts/browser-provider-decision-report.mjs",
);
const browserProviderDecisionReportSmoke = read(
  "scripts/browser-provider-decision-report-smoke.sh",
);
const browserProviderRunbook = read("scripts/browser-provider-runbook.mjs");
const browserProviderRunbookSmoke = read(
  "scripts/browser-provider-runbook-smoke.sh",
);
const browserManualUxChecks = read("scripts/browser-manual-ux-checks.mjs");
const browserManualUxReport = read("scripts/browser-manual-ux-report.mjs");
const browserManualUxValidation = read(
  "scripts/browser-manual-ux-validation.mjs",
);
const browserMacVmManualUxSmoke = read(
  "scripts/browser-mac-vm-manual-ux-smoke.sh",
);
const browserMacVmManualReviewPacket = read(
  "scripts/browser-mac-vm-manual-review-packet.mjs",
);
const browserMacVmManualReviewPacketSmoke = read(
  "scripts/browser-mac-vm-manual-review-packet-smoke.sh",
);
const browserMacVmProof = read("scripts/browser-mac-vm-proof.sh");
const browserMacVmAcceptanceAudit = read(
  "scripts/browser-mac-vm-acceptance-audit.mjs",
);
const browserMacVmAcceptanceAuditSmoke = read(
  "scripts/browser-mac-vm-acceptance-audit-smoke.sh",
);
const browserMacVmAcceptanceHandoff = read(
  "scripts/browser-mac-vm-acceptance-handoff.sh",
);
const browserMacVmAcceptanceHandoffSmoke = read(
  "scripts/browser-mac-vm-acceptance-handoff-smoke.sh",
);
const browserMacVmAuthProfileSetup = read(
  "scripts/browser-mac-vm-auth-profile-setup.sh",
);
const browserMacVmAuthProfileSetupSmoke = read(
  "scripts/browser-mac-vm-auth-profile-setup-smoke.sh",
);
const macSourceHomeRestart = read("scripts/mac-source-home-restart.sh");
const macSourceHomeRestartSmoke = read("scripts/mac-source-home-restart-smoke.sh");
const browserExperimentCleanup = read("scripts/browser-experiment-cleanup.mjs");
const currentState = read("state.md");
const browserSelkiesControlService = read(
  "scripts/browser-selkies-control-service.mjs",
);
const browserSelkiesControlServiceSmoke = read(
  "scripts/browser-selkies-control-service-smoke.sh",
);
const browserSelkiesTargetPreflight = read(
  "scripts/browser-selkies-target-preflight.sh",
);
const browserSelkiesCurrentWheelSmoke = read(
  "scripts/browser-selkies-current-wheel-smoke.sh",
);
const browserSelkiesRealChromiumSmoke = read(
  "scripts/browser-selkies-real-chromium-smoke.sh",
);
const browserSelkiesRuntimeExitSmoke = read(
  "scripts/browser-selkies-runtime-exit-smoke.sh",
);
const browserSelkiesRuntimeExitTarget = read(
  "scripts/browser-selkies-runtime-exit-target.sh",
);
const browserPerLaunchSelkiesSupervisor = read(
  "scripts/browser-per-launch-selkies-supervisor.mjs",
);
const browserPerLaunchSelkiesSupervisorSmoke = read(
  "scripts/browser-per-launch-selkies-supervisor-smoke.sh",
);
const browserSelkiesOperatorImageBuild = read(
  "scripts/browser-selkies-operator-image-build.sh",
);
const browserSelkiesOperatorDockerfile = read(
  "deploy/browser-selkies-runtime-target/Dockerfile",
);
const browserSelkiesSystemService = read(
  "scripts/system/elastos-browser-selkies.service",
);
const browserSelkiesSystemScript = read(
  "scripts/system/elastos-browser-selkies.sh",
);
const browserSelkiesSystemEnv = read(
  "scripts/system/elastos-browser-selkies.env.example",
);
const setupSourceHome = read("scripts/setup-source-home.sh");
const publishReleaseScript = read("scripts/publish-release.sh");
const wciAlignmentScript = read("scripts/check-wci-alignment.sh");
const walletProvider = readAll([
  "capsules/wallet-provider/src/main.rs",
  "capsules/wallet-provider/src/approval.rs",
  "capsules/wallet-provider/src/crypto.rs",
  "capsules/wallet-provider/src/crypto/bitcoin.rs",
  "capsules/wallet-provider/src/crypto/evm.rs",
  "capsules/wallet-provider/src/models.rs",
  "capsules/wallet-provider/src/protocol.rs",
  "capsules/wallet-provider/src/storage.rs",
  "capsules/wallet-provider/src/validation.rs",
  "capsules/wallet-provider/src/tests/accounts.rs",
  "capsules/wallet-provider/src/tests/approvals.rs",
  "capsules/wallet-provider/src/tests/approvals/external.rs",
  "capsules/wallet-provider/src/tests/approvals/managed.rs",
  "capsules/wallet-provider/src/tests/approvals/transactions.rs",
  "capsules/wallet-provider/src/tests/approvals/validation.rs",
  "capsules/wallet-provider/src/tests/browser_signing.rs",
  "capsules/wallet-provider/src/tests/mod.rs",
  "capsules/wallet-provider/src/tests/proofs.rs",
  "capsules/wallet-provider/src/tests/support.rs",
]);
const walletProviderManifest = read("capsules/wallet-provider/capsule.json");
const gatewayTests = readAll([
  "elastos/crates/elastos-server/src/api/gateway_tests/mod.rs",
  "elastos/crates/elastos-server/src/api/gateway_tests/support_providers.rs",
  "elastos/crates/elastos-server/src/api/gateway_tests/support_runtime.rs",
  "elastos/crates/elastos-server/src/api/gateway_tests/documents.rs",
  "elastos/crates/elastos-server/src/api/gateway_tests/home_system.rs",
  "elastos/crates/elastos-server/src/api/gateway_tests/inspect.rs",
  "elastos/crates/elastos-server/src/api/gateway_tests/recovery.rs",
  "elastos/crates/elastos-server/src/api/gateway_tests/room.rs",
  "elastos/crates/elastos-server/src/api/gateway_tests/site_publication.rs",
  "elastos/crates/elastos-server/src/api/gateway_tests/wallet.rs",
  "elastos/crates/elastos-server/src/api/gateway_tests/wallet/accounts.rs",
  "elastos/crates/elastos-server/src/api/gateway_tests/wallet/auth.rs",
  "elastos/crates/elastos-server/src/api/gateway_tests/wallet/chain.rs",
  "elastos/crates/elastos-server/src/api/gateway_tests/wallet/connectors.rs",
  "elastos/crates/elastos-server/src/api/gateway_tests/wallet/inbox.rs",
  "elastos/crates/elastos-server/src/api/gateway_tests/wallet/managed_approvals.rs",
  "elastos/crates/elastos-server/src/api/gateway_tests/wallet/prices.rs",
  "elastos/crates/elastos-server/src/api/gateway_tests/wallet/send.rs",
  "elastos/crates/elastos-server/src/api/gateway_browser_route_tests.rs",
]);
const gatewayBrowserRouteTests = read(
  "elastos/crates/elastos-server/src/api/gateway_browser_route_tests.rs",
);

assert(
  agentsContract.includes("## Branch Roles") &&
    agentsContract.includes("## Branch Lifecycle") &&
    agentsContract.includes("## Publishing Terms And Gates") &&
    agentsContract.includes("## Review And Commit Discipline") &&
    agentsContract.includes("## Verification Gate") &&
    agentsContract.includes("## Public Live Deployment") &&
    agentsContract.includes("## Staging Machines") &&
    agentsContract.includes("## Browser Claim Discipline") &&
    agentsContract.includes("review/0.5.0") &&
    agentsContract.includes("reporting its exact branch, commit, tree id, dirty status") &&
    agentsContract.includes("Target proof must cite the exact source tree") &&
    agentsContract.includes("explicit user approval before the mutation") &&
    agentsContract.includes("WebRTC remote display"),
  "Root AGENTS.md must preserve the operator contract headings, review branch role, target proof discipline, public-live approval rule, and Browser claim discipline",
);
assert(
  linuxSourceHomeRestart.includes("process_matches_gateway_listener") &&
    linuxSourceHomeRestart.includes("refusing to kill unrelated listener") &&
    linuxSourceHomeRestart.includes("http_status") &&
    linuxSourceHomeRestart.includes("OK=0") &&
    linuxSourceHomeRestart.includes("Linux source-home restart verification failed") &&
    linuxSourceHomeRestartSmoke.includes("live_failure_receipt") &&
    linuxSourceHomeRestartSmoke.includes("hash_mismatch_receipt") &&
    linuxSourceHomeRestartSmoke.includes("unrelated_listener_protected"),
  "Linux source-home restart must preserve safe listener ownership checks and ok=false failure receipts",
);
const debugPolicy = read("DEBUG.md");
const homeAssetVersion = "home-20260713a";
assertUsersSelfReferencesAreApproved();
assert(
  shellIndex.includes('role="listbox"'),
  "Home items must expose keyboard-selectable structure",
);
assert(
  shellIndex.includes('aria-label="Home items"'),
  "Home items list must be labeled",
);
assert(
  shellIndex.includes('data-home-status="booting"'),
  "Home readiness state must use Home naming",
);
assert(
  !shellIndex.includes("data-shell-status"),
  "Home readiness state must not preserve shell naming",
);
assert(
  shellIndex.includes('data-action="minimize"'),
  "Window minimize action must remain explicit",
);
assert(
  shellIndex.includes('data-action="maximize"'),
  "Window maximize action must remain explicit",
);
assert(
  shellIndex.includes('data-action="close"'),
  "Window close action must remain explicit",
);
assert(
  shellIndex.includes('rel="manifest"'),
  "Home must expose a web app manifest for mobile install",
);
assert(
  shellIndex.includes(`manifest.webmanifest?v=${homeAssetVersion}`),
  "Home manifest URL must be cache-busted after PWA icon changes",
);
assert(
  shellIndex.includes('name="mobile-web-app-capable" content="yes"'),
  "Home PWA metadata must include mobile-web-app-capable",
);
assert(
  shellIndex.includes('id="toolbar-fullscreen"'),
  "Home must expose a fullscreen control in the top toolbar",
);
assert(
  shellManifest.name === "ElastOS Home",
  "Home PWA manifest must install as ElastOS Home",
);
assert(
  shellManifest.display === "standalone",
  "Home PWA manifest must use standalone display mode",
);
assert(
  Array.isArray(shellManifest.icons) &&
    shellManifest.icons.some((icon) => icon.src === "./elastos-home-icon.svg"),
  "Home PWA manifest must use the Elastos app icon",
);
assert(
  shellManifest.icons.some(
    (icon) =>
      icon.src === "./elastos-home-icon-192.png" && icon.sizes === "192x192",
  ),
  "Home PWA manifest must include a 192px install icon",
);
assert(
  shellManifest.icons.some(
    (icon) =>
      icon.src === "./elastos-home-icon-512.png" && icon.sizes === "512x512",
  ),
  "Home PWA manifest must include a 512px install icon",
);
assert(
  shellServiceWorker.includes('const CACHE_PREFIX = "elastos-home-";') &&
    shellServiceWorker.includes("caches.delete(key)") &&
    shellServiceWorker.includes("self.registration.unregister()") &&
    shellServiceWorker.includes('self.addEventListener("fetch", () => {});'),
  "Home service worker must actively remove old Home caches and unregister instead of preserving stale shell assets",
);
assert(
  !shellServiceWorker.includes("elastos-home-shell"),
  "Home service worker must not preserve the old shell cache namespace",
);
const homePwaIcon = read("capsules/home/browser/elastos-home-icon.svg");
assert(
  homePwaIcon.includes("M669.61 232.852"),
  "Home PWA icon must use the same Elastos mark geometry as the top navbar logo",
);
assert(
  homePwaIcon.includes("paint0_linear_47_14"),
  "Home PWA icon must preserve the official Elastos mark gradient identity",
);
assert(
  !homePwaIcon.includes("M256 78 420 172"),
  "Home PWA icon must not use the old approximate mark",
);
assert(
  !homePwaIcon.includes("M256 206 420 300"),
  "Home PWA icon must not use the old approximate mark",
);
assertPngDimensions(
  "capsules/home/browser/elastos-home-icon-192.png",
  192,
  192,
);
assertPngDimensions(
  "capsules/home/browser/elastos-home-icon-512.png",
  512,
  512,
);
assert(
  shellJs.includes("registerHomeServiceWorker"),
  "Home must register its service worker from the shell entrypoint",
);
assert(
  shellJs.includes("dataset.homeStatus"),
  "Home runtime status must be exposed under data-home-status",
);
assert(
  !shellJs.includes("dataset.shellStatus"),
  "Home runtime status must not preserve data-shell-status",
);
assert(
  shellJs.includes("toggleHomeGuiFullscreen"),
  "Home fullscreen control must be wired through Home GUI",
);
assert(
  shellCore.includes("toolbarFullscreenButton"),
  "Home fullscreen control must be exported by shell-core",
);
assert(
  shellSurface.includes(
    'card.setAttribute("aria-label", `Open ${app.title}`);',
  ),
  "Launcher cards must expose human-readable action labels",
);
assert(
  shellSurface.includes(
    'button.setAttribute("aria-label", desktopShortcutAriaLabel(label));',
  ),
  "Desktop shortcuts must expose human-readable action labels",
);
assert(
  shellSurface.includes("shouldFocusLauncherSearch"),
  "Home launcher search focus must be gated for touch devices",
);
assert(
  !shellSurface.includes(
    "ensureLauncherSelection(activeBrowserTargetId());\n  launcherSearch.focus();",
  ),
  "Home launcher must not focus search unconditionally on mobile",
);
assert(
  shellJs.includes("SHELL_MESSAGE_OPEN_TARGET_SOURCES"),
  "Home open-target messages must stay source-gated",
);
assert(
  shellJs.includes('"archive-manager": new Set(["library"])'),
  "Home must allow Archive to route users into Library for open/create archive journeys",
);
assert(
  shellJs.includes('browser: new Set(["library"])'),
  "Home must allow Browser to route file chooser requests into Library through an explicit source gate",
);
assert(
  shellJs.includes('library: new Set(["archive-manager", "documents", "gba-emulator", "library"])'),
  "Home must keep Library viewer routing source-gated",
);
assert(
  shellIndex.includes(`home-shell-host.js?v=${homeAssetVersion}`),
  "Home entry module must cache-bust after shell browser changes",
);
assert(
  shellIndex.includes(`style.css?v=${homeAssetVersion}`),
  "Home stylesheet must cache-bust after shell browser changes",
);
assert(
  shellJs.includes(`shell-core.js?v=${homeAssetVersion}`),
  "Home shell.js must import the current shell-core module instance",
);
assert(
  shellJs.includes(`shell-surface.js?v=${homeAssetVersion}`),
  "Home must not mix old shell-surface module instances with current shell-windows",
);
assert(
  shellJs.includes(`shell-windows.js?v=${homeAssetVersion}`),
  "Home shell.js must import the current shell-windows module instance",
);
assert(
  shellSurface.includes(`shell-core.js?v=${homeAssetVersion}`),
  "Home shell-surface must import the current shell-core module instance",
);
assert(
  shellSurface.includes(`shell-windows.js?v=${homeAssetVersion}`),
  "Home shell-surface must import the same shell-windows module instance as shell.js",
);
assert(
  !shellJs.includes("prebootBrowserTarget") &&
    !shellJs.includes("home session restore/preboot failed") &&
    !shellWindows.includes("export function prebootBrowserTarget") &&
    !shellWindows.includes("dataset.preboot") &&
    browserVmTargetDoc.includes("does not automatically start a hidden Browser VM") &&
    browserVmTargetDoc.includes("warm sessions must be Runtime/provider-owned"),
  "Home must not auto-start hidden Browser preboot VMs; Browser warm sessions need an explicit Runtime/provider-owned contract",
);
assert(
  !shellCore.includes("TARGET_TITLE_OVERRIDES") &&
    shellCore.includes("export function canonicalTargetTitle") &&
    shellCore.includes("title: normalizeText(target?.title) || target?.target") &&
    shellCore.includes("return normalizedTitle || targetId") &&
    !shellCore.includes("STALE_TARGET_TITLES") &&
    shellWindows.includes("canonicalTargetTitle(launched.target, launched.title)"),
  "Home must preserve Runtime catalog titles without local target-name overrides",
);
assert(
  shellCore.includes("desktopPositionOverlapsAny") &&
    shellCore.includes("nextAvailableDesktopPosition") &&
    shellCore.includes("occupiedDesktopPositionsExcept(targetId)") &&
    shellCore.includes("DESKTOP_ICON_WIDTH") &&
    shellCore.includes("DESKTOP_ICON_HEIGHT") &&
    homeShellRegressionSmoke.includes("People and Wallet desktop positions still overlap") &&
    homeShellRegressionSmoke.includes("de-collided desktop layout was not saved"),
  "Home desktop layout must de-collide persisted shortcut positions so People cannot reload on top of Wallet",
);
assert(
  shellWindows.includes("glyphTarget || id") &&
    shellWindows.includes("glyphTarget: launched.target") &&
    shellCore.includes("targetId === PEOPLE_TARGET_ID") &&
    shellCore.includes('<circle cx="9" cy="8" r="3" />'),
  "Home window titlebar glyphs must use app target identity, and People must not fall back to the generic app glyph",
);
assert(
  !shellWindows.includes("allowfullscreen") &&
    shellWindows.includes('const COMMON_IFRAME_ALLOW = ["autoplay", "fullscreen"]') &&
    shellWindows.includes('allow="${iframeAllowForLaunch(launched)}"'),
  "Home iframes must allow autoplay and fullscreen through the allow policy without deprecated allowfullscreen",
);
assert(
  shellWindows.includes('const BROWSER_IFRAME_ALLOW_EXTRAS = ["clipboard-read", "clipboard-write"]') &&
    shellWindows.includes('launched?.target === "browser"') &&
    shellWindows.includes("tokens.push(...BROWSER_IFRAME_ALLOW_EXTRAS)"),
  "Home must grant clipboard-read/write explicitly and only to the Browser iframe",
);
assert(
  shellWindows.includes('const WEBAUTHN_IFRAME_ALLOW_TARGETS = new Set(["inbox", "wallet"])') &&
    shellWindows.includes('tokens.push("publickey-credentials-get")'),
  "Home must grant WebAuthn only to Inbox and Wallet approval surfaces so passkey-gated signing works without broad capsule authority",
);
assert(
  shellJs.includes('scope === "wallet"') &&
    shellJs.includes('kind === "wallet.requests.changed"') &&
    shellJs.includes("hadCursor || broadcastInitial || events.length > 0"),
  "Home event handling must refresh the shell summary when Wallet request events change, including the first long-poll payload after SSE fallback",
);
assert(
  shellSurface.includes("notifications.attention_count") &&
    shellSurface.includes("notifications.unread_count") &&
    shellSurface.includes("Math.max(0, semanticCount || entries.length)"),
  "Home Inbox bell must prefer semantic notification counts over entries length so approval alerts survive payload-shape changes",
);
assert(
  shellIndex.includes('id="home-notification-toast"') &&
    shellJs.includes("maybeShowWalletApprovalToast(previous, summary)") &&
    shellSurface.includes("wallet_approval_request") &&
    shellSurface.includes('openTarget("inbox")'),
  "Home must surface new Wallet approval requests as a desktop toast that opens Inbox",
);
assert(
  shellWindows.includes("cross-origin or failed state") &&
    shellWindows.includes("frameWindow.removeEventListener"),
  "Home frame cleanup must tolerate cross-origin failed iframe states on close",
);
assert(
  !shellWindows.includes("releaseFrameRuntimePage") &&
    !shellWindows.includes("__elastosBrowserReleaseRuntimePage"),
  "Home must not call into Browser frames for Runtime page cleanup; Browser owns its unload lifecycle",
);
const persistSessionBlock = sourceBlock(
  shellWindows,
  "function persistBrowserSession()",
  "Home browser session persistence",
);
assert(
  persistSessionBlock.includes("saveShellSessionState({ root_shell: rootShell, windows: [] });"),
  "Home must persist an explicit empty session after the last window closes",
);
assert(
  !persistSessionBlock.includes("clearShellSessionState();"),
  "Home must not clear session state from the last-window-close path",
);
assert(
  shellSurface.includes("shouldOpenDesktopShortcutFromClick"),
  "Home desktop icons must use touch-specific tap-open behavior",
);
assert(
  shellSurface.includes("longPressReady"),
  "Home desktop icons must require long-press before touch dragging",
);
assert(
  shellSurface.includes("clearDragSelection"),
  "Home desktop drag must actively clear browser text selection",
);
assert(
  shellSurface.includes('document.body.classList.add("dragging-target")'),
  "Home desktop drag must mark the selection-suppression lifetime",
);
assert(
  shellStyle.includes("body.dragging-target"),
  "Home desktop drag must suppress text selection while moving icons",
);
assert(
  shellCore.includes("desktopHidden: []"),
  "Home layout state must track per-target desktop icon removal",
);
assert(
  shellCore.includes("addTargetToDesktop") &&
    shellCore.includes("removeTargetFromDesktop"),
  "Home must support reversible desktop icon presence",
);
assert(
  shellSurface.includes('action: "remove-desktop-icon"') &&
    shellSurface.includes('action: "add-desktop-icon"'),
  "Home menus must expose remove/add desktop icon actions",
);
assert(
  shellJs.includes('type: "home:open-target"') ||
    shellJs.includes('"home:open-target"'),
  "Home must keep the open-target message contract",
);
assert(
  shellJs.includes('type: "home:open-uri"') ||
    shellJs.includes('"home:open-uri"'),
  "Home must keep the open-uri message contract",
);
assert(
  !shellJs.includes("pc2-shell:"),
  "Home postMessage contract must not preserve old pc2-shell message types",
);
assert(
  !shellJs.includes("shell_token"),
  "Home browser route tokens must use home_token",
);
assert(
  !shellJs.includes("x-elastos-shell-token"),
  "Home browser API tokens must use x-elastos-home-token",
);
assert(
  shellJs.includes("SHELL_MESSAGE_DELIVER_TARGET_SOURCES"),
  "Home must source-gate capsule-to-capsule picker returns",
);
assert(
  shellJs.includes('library: new Set(["archive-manager", "browser", "chat-room"])'),
  "Home must allow Library picker results to return only to Archive, Browser and Chat Room",
);
assert(
  shellJs.includes('marketplace: "runtime-target"') &&
    shellJs.includes('if (policy === "runtime-target")') &&
    shellJs.includes("return normalizedActiveShellName(target) !== HOME_GUI_SHELL_ID;"),
  "Home must allow Marketplace to open installed Runtime targets while still blocking Home self-launch",
);
assert(
  shellJs.includes('"home:close-self"'),
  "Home must allow token-bound picker windows to close themselves after selection",
);
assert(
  shellJs.includes('"chat-room": new Set(["library"])'),
  "Chat Room Attach must be allowed to open Library through Home message policy",
);
assert(
  shellJs.includes('"chat-room": new Set(["documents"])') &&
    shellJs.includes('"home:open-target-with-payload"') &&
    shellJs.includes("openHomeGuiTargetWithPayload"),
  "Home must broker Chat Room attachment payloads into Documents without host-browser navigation",
);
const deliverMessageToTargetFrameBlock = sourceBlock(
  shellJs,
  "export function deliverMessageToHomeGuiTargetFrame",
  "Home target message delivery",
);
assert(
  deliverMessageToTargetFrameBlock.includes("options = null") &&
    deliverMessageToTargetFrameBlock.includes("if (options?.focus === true)") &&
    deliverMessageToTargetFrameBlock.includes("focusWindow(entry.id);"),
  "Home generic target delivery must only focus a recipient when explicitly requested",
);
const openTargetWithPayloadBlock = sourceBlock(
  shellJs,
  "export function openHomeGuiTargetWithPayload",
  "Home open-target-with-payload delivery",
);
assert(
  shellJs.includes("deliverMessageToHomeGuiTargetFrame(target, payload)") &&
    (
      openTargetWithPayloadBlock.match(
        /deliverMessageToHomeGuiTargetFrame\(target, payload, \{ focus: true \}\)/g,
      ) || []
    ).length >= 2,
  "Home must keep deliver-to-target background-only while open-target-with-payload focuses the opened app",
);
assert(
  shellJs.includes('new Set(["documents", "chat-room"])'),
  "Home must allow Chat Room to open elastos:// links through the same URI contract as Documents",
);
assert(
  shellCore.includes('export const PEOPLE_TARGET_ID = "people"') &&
    !shellCore.includes('target: PEOPLE_TARGET_ID') &&
    !shellCore.includes('route: "home://people"'),
  "Home must render the canonical People target without manufacturing a local launcher entry",
);
assert(
  peopleCapsule.includes('"name": "people"') &&
    peopleCapsule.includes('"role": "app"') &&
    peopleCapsule.includes('"runtime_abi": "elastos.runtime-projection/v1"') &&
    peopleIndex.includes("People · ElastOS") &&
    peopleIndex.includes("Open People from Home.") &&
    peopleIndex.includes('id="profile-form"') &&
    peopleIndex.includes('id="discovery"') &&
    peopleScript.includes("/api/apps/people/summary") &&
    peopleScript.includes("/api/apps/people/profile-card") &&
    peopleScript.includes("/api/apps/people/discovery") &&
    peopleScript.includes("/api/apps/people/discovery/refresh") &&
    peopleScript.includes("/api/apps/people/discovery/requests") &&
    peopleScript.includes("/api/apps/people/contacts/remove") &&
    peopleScript.includes('type: "home:open-target"') &&
    peopleScript.includes('target !== "chat-room"') &&
    peopleStyle.includes(".people-shell") &&
    peopleStyle.includes(".people-sidebar") &&
    peopleStyle.includes(".discovery-grid") &&
    !shellWindows.includes("renderPeopleWindowBody") &&
    !shellWindows.includes("/api/apps/people/") &&
    !shellStyle.includes(".home-people-") &&
    shellWindows.includes('"people",') &&
    shellJs.includes('people: new Set(["chat-room"])') &&
    homeCmd.includes("issue_capsule_launch_token(&data_dir, PEOPLE_CAPSULE_NAME)"),
  "People must be a standalone app capsule while Home remains only its launch and message host",
);
assert(
  servicesCapsule.includes('"name": "services"') &&
    servicesCapsule.includes('"role": "app"') &&
    servicesIndex.includes("Services · ElastOS") &&
    servicesIndex.includes("This device") &&
    servicesIndex.includes("From People") &&
    servicesIndex.includes("mine-services") &&
    servicesIndex.includes("other-services") &&
    servicesIndex.includes("services-20260711i") &&
    servicesScript.includes("/api/apps/services/summary") &&
    servicesScript.includes("/api/apps/services/offers") &&
    servicesScript.includes("Browser Engine") &&
    servicesScript.includes("Browser Exit service") &&
    !servicesScript.includes("Exit Node") &&
    !servicesCapsule.includes("Exit Node") &&
    servicesScript.includes("activateServicesSection") &&
    servicesScript.includes("EXIT_SERVICE_KIND") &&
    servicesScript.includes('const EXIT_SERVICE_KIND = "remote_exit"') &&
    servicesScript.includes('const BROWSER_ENGINE_SERVICE_KIND = "browser_engine"') &&
    servicesScript.includes('const CONFIGURED_REMOTE_EXIT_SOURCE = "configured_remote_exit"') &&
    servicesScript.includes("VISIBLE_SERVICE_KINDS") &&
    servicesScript.includes("visibleServiceOffers") &&
    servicesScript.includes("isReadOnlyServiceOffer") &&
    servicesScript.includes("Managed by config") &&
    servicesScript.includes("Approved") &&
    servicesScript.includes("Denied") &&
    servicesScript.includes("serviceRequestStatus") &&
    servicesScript.includes("Share with People") &&
    servicesScript.includes("Stop sharing") &&
    servicesScript.includes("Subscribe") &&
    servicesScript.includes("Remove") &&
    servicesScript.includes("data-confirm-service-action") &&
    servicesScript.includes("renderInlineConfirmation") &&
    servicesScript.includes("Approval needed") &&
    servicesScript.includes("Ask to use") &&
    servicesScript.includes("Service request sent.") &&
    servicesStyle.includes(".settings-sidebar") &&
    servicesStyle.includes(".services-toolbar") &&
    servicesStyle.includes(".service-confirm") &&
    servicesStyle.includes(".pc2-btn-danger") &&
    !servicesIndex.includes("settings-sidebar-title") &&
    !servicesStyle.includes(".settings-sidebar-title") &&
    !servicesStyle.includes(".service-filter-banner") &&
    servicesStyle.includes(".settings-sidebar-icon-mine") &&
    servicesStyle.includes(".settings-sidebar-icon-others") &&
    setupSourceHome.includes("services") &&
    read("components.json").includes('"services"') &&
    gatewayApi.includes("const SERVICES_CAPSULE_ID") &&
    gatewayApi.includes('"/api/apps/services/summary"') &&
    gatewayApi.includes('"/api/apps/services/offers"') &&
    gatewayApi.includes("pub(super) async fn services_summary") &&
    gatewayApi.includes("pub(super) async fn services_offer_update") &&
    gatewayApi.includes("HOME_SERVICES_STATE_SCHEMA") &&
    gatewayApi.includes(".AppData/ElastOS/Home/services-state.json") &&
    gatewayApi.includes("write_principal_root_object") &&
    !gatewayApi.includes('data_dir.join("config/services-state.json")') &&
    gatewayHomeSystemTests.includes(
      "test_services_selection_state_is_principal_scoped",
    ) &&
    gatewayHomeSystemTests.includes(
      "test_services_summary_projects_configured_remote_exit_without_ticket",
    ) &&
    gatewayHomeSystemTests.includes(
      "test_services_remote_exit_request_delivers_provider_inbox_notification",
    ) &&
    gatewayHomeSystemTests.includes(
      "test_services_remote_exit_request_local_only_does_not_save_requested_state",
    ) &&
    gatewayApi.includes("home_services_require_remote_request_delivery") &&
    gatewayApi.includes("elastos.service-access-decision/v1") &&
    gatewayApi.includes("home_services_sync_access_decisions") &&
    gatewayApi.includes("home_services_send_access_decision") &&
    gatewayApi.includes("home_services_install_remote_exit_grant") &&
    gatewayApi.includes("home_services_remove_remote_exit_grant") &&
    gatewayApi.includes("elastos.service.remote-exit-grant/v1") &&
    gatewayApi.includes("installed_remote_exit_id") &&
    gatewayApi.includes(
      "Carrier service access request was not delivered to the other person's device",
    ) &&
    gatewayHomeSystemTests.includes('approved_offer["status"], "active"') &&
    gatewayHomeSystemTests.includes("fake-ticket-services-right") &&
    gatewayHomeSystemTests.includes("offer[\"grant_required\"] == true") &&
    gatewayApi.includes("local:provider:browser-engine") &&
    gatewayApi.includes("local:provider:browser-exit") &&
    gatewayApi.includes("home_configured_remote_exit_offers") &&
    gatewayApi.includes("configured_remote_exit") &&
    gatewayApi.includes("managed by Exit Provider config") &&
    gatewayApi.includes("elastos://peer/browser-exit") &&
    gatewayApi.includes("HOME_SERVICES_REQUESTS_TOPIC") &&
    gatewayApi.includes("service-approve-request:") &&
    shellJs.includes('services: new Set(["browser", "chat-room"])'),
  "Services must be a first-party capsule with Mine/Others Browser Engine + Browser Exit service UI and a scoped summary API",
);
for (const staleServicesToken of [
  "Services needs a signed Home launch token",
  "Start with no services enabled",
  "Trusted Services",
  "Local Services",
  "No trusted services have been discovered through People yet.",
  "services-20260624a",
  "services-20260624b",
  "services-20260624c",
  "services-20260624d",
  "services-20260625a",
  "services-20260625b",
  "services-20260625c",
  "available-services",
  "browser-exit-card",
  "trusted-count",
  "local-count",
  "settings-sidebar-title",
  "serviceRequestFromLocation",
  "data-clear-services-filter",
  "filterServiceOffersByContact",
  "Copy URI",
  "provider URI",
  "Request access",
  "No grant prompt",
  "service-meta",
  "grant_scope",
  "No services enabled yet",
  "No services from other homes",
  "Approval required",
  "Open app",
  "data-open-target=",
  "openCapsuleTarget",
  "No enabled services from this person yet",
  "No available services from this person",
]) {
  assert(
    !servicesIndex.includes(staleServicesToken) &&
      !servicesScript.includes(staleServicesToken),
    `Services UI must not contain stale discovery token: ${staleServicesToken}`,
  );
}
assert(
  gatewayApi.includes("struct HomeServicesSummary") &&
    gatewayApi.includes('"elastos.runtime.services/v1"') &&
    gatewayApi.includes("local_offer_count") &&
    gatewayApi.includes("remote_offer_count") &&
    gatewayApi.includes("available_local_offer_count") &&
    gatewayApi.includes("available_remote_offer_count") &&
    gatewayApi.includes("local_offers") &&
    gatewayApi.includes("remote_offers") &&
    gatewayApi.includes("available_local_offers") &&
    gatewayApi.includes("available_remote_offers") &&
    gatewayApi.includes("principal_scoped_provider_grant") &&
    gatewayApi.includes("HomeServiceRuntimeContractSummary") &&
    gatewayApi.includes('"elastos.service.runtime-contract/v1"') &&
    gatewayApi.includes("home_browser_engine_runtime_contract") &&
    gatewayApi.includes("home_browser_engine_supported_display_mode") &&
    gatewayApi.includes("home_browser_engine_supported_display_mode(mode)") &&
    gatewayApi.includes('matches!(mode, "webrtc_remote_display" | "native_surface")') &&
    gatewayApi.includes("browser_engine_adapter_uses_remote_vm_launcher") &&
    gatewayApi.includes('"remote_operator_vm"') &&
    gatewayApi.includes('"local_microvm"') &&
    gatewayApi.includes('"mechanism_microvm"') &&
    gatewayApi.includes("supported_display_modes") &&
    gatewayApi.includes("home_services_summary") &&
    gatewayApi.includes("home_local_service_offers") &&
    gatewayApi.includes('"browser_engine"') &&
    gatewayApi.includes('"content_availability"') &&
    gatewayApi.includes('"elastos://browser-engine/*"') &&
    gatewayApi.includes('"elastos://content/*"') &&
    gatewayApi.includes("services: home_state.services") &&
    gatewayApi.includes("home_services_realtime_signature") &&
    gatewayApi.includes("services.changed"),
  "Home must expose local and remote service offers through a top-level runtime services summary",
);
assert(
  gatewayApi.includes('const PEOPLE_CAPSULE_ID: &str = "people"') &&
    gatewayApi.includes('"/api/apps/people/summary"') &&
    gatewayApi.includes('"/api/apps/people/discovery"') &&
    gatewayApi.includes('"/api/apps/people/profile-card"') &&
    gatewayApi.includes('"/api/apps/people/discovery/refresh"') &&
    gatewayApi.includes('"/api/apps/people/discovery/requests"') &&
    gatewayApi.includes('"/api/apps/people/discovery/requests/:request_id/accept"') &&
    gatewayApi.includes('"/api/apps/people/discovery/requests/:request_id/join"') &&
    gatewayApi.includes("HOME_PEOPLE_DISCOVERY_SCHEMA") &&
    gatewayApi.includes("HOME_PEOPLE_CONTACTS_SCHEMA") &&
    gatewayApi.includes("HOME_PEOPLE_DISCOVERY_TOPIC") &&
    gatewayApi.includes("people_discovery_update") &&
    gatewayApi.includes("people_profile_card_update") &&
    gatewayApi.includes("update_profile_card_for_context") &&
    gatewayApi.includes("people_discovery_refresh") &&
    gatewayApi.includes("home_people_discovery_sync") &&
    gatewayApi.includes("HOME_PEOPLE_DISCOVERY_PRESENCE_INTERVAL_SECS") &&
    gatewayApi.includes("HOME_PEOPLE_DISCOVERY_BOOTSTRAP_INTERVAL_SECS") &&
    gatewayApi.includes("home_people_discovery_annotate_refresh") &&
    gatewayApi.includes("home_people_discovery_state_signature") &&
    gatewayApi.includes("next_refresh_after_ms") &&
    gatewayApi.includes("refresh_fingerprint") &&
    gatewayApi.includes('"gossip_send"') &&
    gatewayApi.includes('"gossip_recv"') &&
    gatewayApi.includes("HOME_PEOPLE_DISCOVERY_ENABLED_SECS") &&
    gatewayApi.includes("enabled_until: Option<u64>") &&
    gatewayApi.includes("remaining_seconds: Option<u64>") &&
    gatewayApi.includes("home_people_discovery_active") &&
    gatewayApi.includes("home_people_discovery_apply_expiry") &&
    gatewayApi.includes("people_discovery_request_create") &&
    gatewayApi.includes("people_discovery_request_accept") &&
    gatewayApi.includes("people_discovery_request_join") &&
    gatewayApi.includes("home_people_discovery_send_acceptance") &&
    gatewayApi.includes("home_people_discovery_send_room_acceptance") &&
    gatewayApi.includes("people discovery delivery failed") &&
    gatewayApi.includes("home_people_upsert_contact") &&
    gatewayApi.includes("clean_people_person_display_name") &&
    gatewayApi.includes("home_people_discovery_request_visible") &&
    gatewayApi.includes('matches!(request.status.as_str(), "incoming" | "requested")') &&
    gatewayApi.includes('"connected".to_string()') &&
    !gatewayApi.includes('"Carrier contact".to_string()') &&
    !gatewayApi.includes("local Carrier runtime") &&
    gatewayApi.includes('"Conversation provider".to_string()') &&
    gatewayApi.includes('"Remote Exit".to_string()') &&
    !gatewayApi.includes('"Carrier conversation".to_string()') &&
    !gatewayApi.includes('"Carrier room service".to_string()') &&
    gatewayApi.includes("apply_home_people_contacts_state") &&
    gatewayApi.includes("home_people_discovery_sync_contacts") &&
    gatewayApi.includes("merge_people_discovery_acceptance") &&
    !sourceBlock(
      gatewayApi,
      "pub(super) async fn people_discovery_request_accept",
      "People discovery accept handler",
    ).includes("export_room_invite") &&
    sourceBlock(
      gatewayApi,
      "pub(super) async fn people_discovery_request_create",
      "People discovery request create handler",
    ).includes('.context("people discovery delivery failed")?') &&
    sourceBlock(
      gatewayApi,
      "pub(super) async fn people_discovery_request_accept",
      "People discovery request accept handler",
    ).includes('.context("people discovery delivery failed")?') &&
    sourceBlock(
      gatewayApi,
      "pub(super) async fn people_discovery_request_join",
      "People discovery request join handler",
    ).includes('.context("people discovery delivery failed")?') &&
    !sourceBlock(
      gatewayApi,
      "pub(super) async fn people_discovery_request_create",
      "People discovery request create handler",
    ).includes("let _ = home_people_discovery_send_request") &&
    !sourceBlock(
      gatewayApi,
      "pub(super) async fn people_discovery_request_accept",
      "People discovery request accept handler",
    ).includes("let _ = home_people_discovery_send_acceptance") &&
    !sourceBlock(
      gatewayApi,
      "pub(super) async fn people_discovery_request_join",
      "People discovery request join handler",
    ).includes("let _ = home_people_discovery_send_room_acceptance") &&
    !gatewayApi.includes("home_people_discovery_send_invite") &&
    gatewayApi.includes("struct HomePeopleDiscoverySummary") &&
    gatewayApi.includes("discovery: HomePeopleDiscoverySummary") &&
    gatewayHomeSystemTests.includes("test_people_discovery_toggle_persists_in_home_summary") &&
    gatewayHomeSystemTests.includes(
      "test_people_discovery_expired_visibility_reports_off_and_refresh_does_not_publish",
    ) &&
    gatewayHomeSystemTests.includes("test_people_discovery_refresh_finds_visible_peer") &&
    gatewayHomeSystemTests.includes("test_people_profile_card_update_uses_people_launch_token") &&
    gatewayHomeSystemTests.includes("test_people_summary_requires_people_launch_token") &&
    gatewayHomeSystemTests.includes("test_people_discovery_request_accept_contact_round_trip") &&
    gatewayHomeSystemTests.includes(
      "test_people_discovery_request_send_failure_does_not_save_requested_state",
    ) &&
    gatewayHomeSystemTests.includes(
      "test_people_discovery_accept_send_failure_does_not_save_joined_state",
    ) &&
    gatewayHomeSystemTests.includes(
      "test_people_discovery_join_send_failure_does_not_save_joined_state",
    ) &&
    gatewayApi.includes('"/api/apps/people/invites/create"') &&
    gatewayApi.includes("people_invite_create") &&
    gatewayApi.includes("ensure_local_principal_room_session") &&
    gatewayApi.includes("export_room_join_invite") &&
    roomService.includes('invite_url: format!("elastos://peer/invite?token={token}")') &&
    roomService.includes("RoomRole::Member => matches!(invited_role, RoomRole::Member)"),
  "People Discovery must create People entries while conversation invites remain a separate room policy route",
);
const finishAttachmentUploadBlock = sourceBlock(
  roomService,
  "pub fn finish_attachment_upload",
  "Room attachment upload finish",
);
assert(
  finishAttachmentUploadBlock.includes("let upload = state.uploads[upload_index].clone();") &&
    finishAttachmentUploadBlock.indexOf("append_attachment_record(") <
      finishAttachmentUploadBlock.indexOf("state.uploads.remove(upload_index)") &&
    roomService.includes(
      "attachment_upload_finish_preserves_retry_state_when_incomplete",
    ) &&
    roomService.includes(
      "attachment_upload_finish_preserves_retry_state_when_staged_file_is_missing",
    ),
  "Room attachment upload finish must keep retry state until the attachment commit is proven",
);
assert(
  gatewayApi.includes("CarrierClient::connect_endpoint_addr") &&
    gatewayApi.includes("room transport trusted-source pull returned messages") &&
    gatewayTests.includes("Chat Room summary must not expose raw trusted-source ticket authority") &&
    gatewayTests.includes("Chat Room summary must not expose trusted-source connect_ticket fields") &&
    read("docs/ARCHITECTURE.md").includes(
      "Runtime-owned trusted-source Room bootstrap exception",
    ) &&
    read("docs/CARRIER.md").includes("raw trusted-source") &&
    read("docs/CARRIER.md").includes("decoded endpoints") &&
    read("docs/CARRIER.md").includes("direct Carrier socket authority"),
  "Room trusted-source bootstrap must be classified as a Runtime-owned Carrier exception and must not expose raw ticket authority to capsules/UI",
);
assert(
  gatewayApi.includes('"/api/apps/people/contacts/remove"') &&
    gatewayApi.includes("people_contact_remove") &&
    gatewayApi.includes("HOME_PEOPLE_REMOVED_CONTACTS_SCHEMA") &&
    gatewayApi.includes("home_mark_people_contact_removed") &&
    gatewayApi.includes("filter_removed_people_contacts") &&
    !sourceBlock(
      gatewayApi,
      "pub(super) async fn people_contact_remove",
      "People contact remove handler",
    ).includes("remove_room_member"),
  "People Remove must be local People state, not conversation member ejection",
);
assert(
  shellJs.includes('scope === "people"') &&
    shellJs.includes('kind === "people.changed"'),
  "Home event handling must refresh the People surface on people.changed events",
);
assert(
  roomService.includes('parsed.scheme() == "elastos"'),
  "Chat Room service must classify elastos:// document links as first-class room links",
);
assert(
  chatRoomUi.includes("data-open-uri"),
  "Chat Room must render elastos:// room links as shell-openable actions",
);
assert(
  chatRoomUi.includes("home:open-uri"),
  "Chat Room must open elastos:// room links through Home URI orchestration",
);
assert(
  !chatRoomUi.includes("/api/provider/documents/"),
  "Chat Room must not call the Documents provider directly",
);
assert(
  !chatRoomUi.includes("/ipfs/"),
  "Chat Room must not call IPFS routes directly",
);
assert(
  chatRoomUi.includes("data-guest-action"),
  "Chat Room must expose guest kick actions in shell mode",
);
assert(
  chatRoomUi.includes("data-node-action"),
  "Chat Room must expose runtime node block/cancel actions in shell mode",
);
assert(
  chatRoomUi.includes("data-room-policy"),
  "Chat Room must expose room policy controls in shell mode",
);
assert(
  chatRoomUi.includes("show_access_controls"),
  "Chat Room access controls must stay behind an explicit settings toggle",
);
assert(
  chatRoomUi.includes("home:open-target"),
  "Chat Room Attach must ask Home to open Library instead of directly opening host files in shell mode",
);
assert(
  chatRoomUi.includes("Open Home to attach from Library."),
  "Chat Room Attach must fail visibly when Home is unavailable",
);
assert(
  !chatRoomUi.includes("attachment_input.click()"),
  "Chat Room Attach must not open the host browser file picker",
);
assert(
  chatRoomUi.includes('"returnTarget"') && chatRoomUi.includes('"attach"'),
  "Chat Room Attach must launch Library in explicit attach mode",
);
assert(
  chatRoomUi.includes("chat-room:attach-library-item"),
  "Chat Room must accept Library picker results through Home delivery",
);
assert(
  chatRoomUi.includes("documents:open-chat-attachment") &&
    chatRoomUi.includes("home:open-target-with-payload") &&
    chatRoomUi.includes("data-open-attachment") &&
    chatRoomUi.includes("Open in Documents") &&
    !chatRoomUi.includes('open.set_attribute("download", &attachment.file_name)?') &&
    !chatRoomUi.includes('open.set_attribute("href", url)?') &&
    !chatRoomUi.includes('"/api/apps/chat-room/attachments/{}"') &&
    !chatRoomUi.includes('open.set_attribute("target", "_blank")?'),
  "Chat Room attachment actions must open cached attachment bytes in Documents, not download or navigate browser routes",
);
assert(
  shellJs.includes("resolvePeerInviteUri(uri)") &&
    shellJs.includes('parsed.hostname === "peer"') &&
    !shellJs.includes('parsed.hostname === "chat"') &&
    !shellJs.includes("isLegacyChatJoin") &&
    shellJs.includes('target: "chat-room"') &&
    shellJs.includes("query: { invite: uri }") &&
    chatRoomUi.includes("initial_join_invite") &&
    chatRoomUi.includes("join_conversation_from_invite().await"),
  "Home must route only elastos://peer/invite links into Chat",
);
assert(
  chatRoomUi.includes('"/session/leave"') &&
    chatRoomUi.includes('"pagehide"') &&
    chatRoomUi.includes('"beforeunload"') &&
    chatRoomUi.includes('"keepalive"'),
  "Chat Room shell close must send a scoped leave request before Home removes the capsule frame",
);
assert(
  chatRoomUi.includes(
    "summary.local_runtime_role.is_none() && !summary.browser_access_allowed",
  ),
  "Chat Room shell sessions must remain openable when guest requests are disabled for active members",
);
assert(
  gatewayApi.includes('"/api/apps/chat-room/guests/:session_id/kick"'),
  "Chat Room guest kicking must go through the gateway capacity-token API",
);
assert(
  gatewayApi.includes('"/api/apps/chat-room/members/invite"'),
  "Chat Room runtime node invites must go through the gateway capacity-token API",
);
assert(
  gatewayApi.includes('"/api/apps/chat-room/members/remove"'),
  "Chat Room runtime node blocking must go through the gateway capacity-token API",
);
assert(
  !gatewayApi.includes("ProviderBridge::spawn"),
  "Gateway must not spawn provider bridges directly",
);
assert(
  !gatewayApi.includes("ipfs_provider_binary") &&
    !gatewayApi.includes("ipfs_bridge"),
  "Gateway must not keep a direct IPFS bridge fallback",
);
assert(
  authGatewayApi.includes("admin passkey required to remove another passkey"),
  "Passkey revocation must enforce admin authority in the runtime route",
);
assert(
  authGatewayApi.includes(
    "last admin passkey cannot be removed while guest passkeys remain",
  ),
  "Passkey revocation must not strand guest accounts without an admin",
);
assert(
  gatewayApi.includes('"/api/auth/recovery/status"'),
  "Principal-root recovery status must be a runtime auth route, not app-local state",
);
assert(
  gatewayApi.includes('"/api/auth/recovery/export"') &&
    gatewayApi.includes('"/api/auth/recovery/import"') &&
    gatewayApi.includes('"/api/auth/recovery/full-export"') &&
    gatewayApi.includes('"/api/auth/recovery/full-import"'),
  "Recovery Kit import/export handlers must be wired as runtime auth routes",
);
assert(
  recoveryKitLiveSmoke.includes("elastos.principal.root-recovery.status/v1"),
  "Recovery Kit live smoke must validate the current runtime recovery-status schema",
);
assert(
  gatewayTests.includes(
    "test_recovery_kit_routes_create_export_and_import_password_package",
  ),
  "Recovery Kit route journey must be covered at the public gateway route layer",
);
assert(
  gatewayTests.includes(
    "test_recovery_kit_routes_prevent_admin_exporting_guest_kit",
  ),
  "Recovery Kit route coverage must prove admins cannot export another principal's kit",
);
assert(
  authGatewayApi.includes("PrincipalRootRecoveryStatusV1::unprotected"),
  "Recovery status must fail honest until encrypted roots and recovery kits exist",
);
assert(
  authGatewayApi.includes("recovery_archive_from_kit") &&
    authGatewayApi.includes("recovery_kit_from_archive"),
  "Recovery Kit routes must store a principal-bound encrypted archive for later System downloads",
);
assert(
  authContract.includes("PRINCIPAL_ROOT_PROTECTION_SCHEMA") &&
    authContract.includes("RECOVERY_KIT_SCHEMA"),
  "Auth contract must declare principal-root protection and recovery kit schemas",
);
assert(
  authContract.includes("ml-kem-768") &&
    authContract.includes("ml-dsa-65") &&
    authContract.includes("slh-dsa"),
  "Principal-root recovery contract must keep PQ-ready algorithm metadata",
);
assert(
  authContract.includes(
    "principal_root_protection_rejects_unknown_contract_fields_at_decode",
  ) &&
    authContract.includes(
      "recovery_kit_import_request_rejects_unknown_nested_fields_at_decode",
    ),
  "Principal-root recovery contracts must reject unknown hidden fields at decode time",
);
assert(
  authContract.includes("validate_principal_root_protector_kind_envelope") &&
    authContract.includes(
      "principal_root_protection_accepts_webauthn_prf_protector",
    ) &&
    authContract.includes(
      "principal_root_protection_rejects_webauthn_prf_with_wrong_kdf",
    ) &&
    authContract.includes(
      "principal_root_protection_rejects_archive_on_non_recovery_kit_protector",
    ),
  "WebAuthn PRF root protectors must stay distinct from generic Recovery Kit protectors",
);
assert(
  authContract.includes("validate_principal_root_protector_subject") &&
    authContract.includes(
      "principal_root_protection_accepts_did_recovery_protector",
    ) &&
    authContract.includes(
      "principal_root_protection_rejects_did_recovery_without_did_subject",
    ) &&
    authContract.includes(
      "principal_root_protection_rejects_did_recovery_with_wrong_kdf",
    ),
  "DID recovery root protectors must carry DID-bound metadata and fail closed without a DID subject",
);
assert(
  didProvider.includes("VerifyDidRecovery") &&
    didProvider.includes("DID_RECOVERY_PROOF_SCHEMA") &&
    didProvider.includes(
      "test_verify_did_recovery_accepts_typed_did_key_proof",
    ) &&
    didProvider.includes(
      "test_verify_did_recovery_rejects_did_elastos_until_resolver_exists",
    ) &&
    didProvider.includes(
      "test_verify_did_recovery_rejects_noncanonical_root_binding",
    ) &&
    didProvider.includes("deny_unknown_fields") &&
    didProvider.includes(
      "test_did_provider_rejects_hidden_recovery_request_fields",
    ) &&
    didProvider.includes(
      "test_did_provider_rejects_hidden_chat_signing_fields",
    ) &&
    didProviderManifest.includes("verify_did_recovery"),
  "did-provider must keep typed did:key recovery-proof verification fail-closed and reject hidden authority fields until did:elastos resolver wiring exists",
);
assert(
  authGatewayApi.includes("verify_did_recovery_import_proof") &&
    authGatewayApi.includes("DID provider rejected the recovery proof") &&
    authGatewayApi.includes(
      "recovery_kit_import_consumes_matching_did_recovery_proof",
    ) &&
    authGatewayApi.includes(
      "recovery_kit_import_rejects_unverified_did_recovery_proof",
    ),
  "Recovery Kit import must verify DID recovery proofs through did-provider and fail closed on invalid proofs",
);
assert(
  runtimeAuth.includes(
    'PRINCIPAL_ROOT_OBJECT_SCHEMA: &str = "elastos.principal-root.object/v1"',
  ),
  "Runtime auth must declare a versioned protected principal-root object envelope",
);
assert(
  runtimeAuth.includes("struct PrincipalRootObjectEnvelopeV1") &&
    runtimeAuth.includes("principal_root_object_aad"),
  "Runtime auth must bind protected principal-root objects with envelope metadata and AAD",
);
assert(
  runtimeAuth.includes("validate_principal_root_object_binding") &&
    runtimeAuth.includes(
      "principal-root object URI is outside the principal root",
    ),
  "Runtime auth must reject protected-object URI/root binding mismatches",
);
assert(
  runtimeAuth.includes("protected principal-root object is not encrypted"),
  "Protected principal roots must reject plaintext object reads",
);
assert(
  storageHandler.includes("reject_principal_root_storage_path") &&
    storageHandler.includes(
      "principal-root storage requires a runtime principal-scoped provider route",
    ),
  "Generic localhost storage handlers must fail closed for Users roots without principal-scoped provider context",
);
assert(
  storageHandler.includes(
    "test_public_storage_rejects_users_root_without_principal_context",
  ),
  "Generic localhost storage handlers must have coverage proving Users roots are rejected without principal context",
);
assert(
  storageHandler.split("Users/self").length - 1 === 1,
  "Generic localhost storage tests must only mention Users/self in the explicit rejection case",
);
assert(
  !capabilityHandler.includes("Users/self") &&
    !providerResource.includes("Users/self"),
  "Generic capability/provider-resource examples must not preserve shared Users/self storage examples",
);
assert(
  !vmProvider.includes("Users/self"),
  "VM provider path-shaping tests must not preserve shared Users/self storage examples",
);
assertProtectedPrincipalRootAccessor(
  documentsProvider,
  "fn documents_load_body(",
  "read_principal_root_object(",
  "Documents body reads",
);
assertProtectedPrincipalRootAccessor(
  documentsProvider,
  "fn documents_write_body(",
  "write_principal_root_object(",
  "Documents body writes",
);
assertProtectedPrincipalRootAccessor(
  gatewayApi,
  "fn home_browser_state(\n",
  "read_principal_root_object(",
  "Home browser state reads",
);
assertProtectedPrincipalRootAccessor(
  gatewayApi,
  "fn home_save_browser_state(",
  "write_principal_root_object(",
  "Home browser state writes",
);
assert(
  gatewayApi.includes("is_unencrypted_principal_root_state") &&
    gatewayTests.includes(
      "test_home_browser_state_resets_plaintext_for_protected_principal_root",
    ),
  "Home must reset untrusted plaintext browser state for protected roots without accepting the plaintext",
);
assertProtectedPrincipalRootAccessor(
  viewerGatewayApi,
  "pub async fn viewer_storage_get(",
  "read_principal_root_object(",
  "Viewer/content storage reads",
);
assertProtectedPrincipalRootAccessor(
  viewerGatewayApi,
  "pub async fn viewer_storage_put(",
  "write_principal_root_object(",
  "Viewer/content storage writes",
);
assert(
  chainProvider.includes("NODE_LIFECYCLE_STATE_SCHEMA") &&
    chainProvider.includes("PersistedNodeLifecycleState"),
  "Chain provider node lifecycle status must persist typed state instead of remaining request-local",
);
assert(
  chainProvider.includes(
    "node_lifecycle_state_survives_provider_reload_without_raw_rpc",
  ),
  "Chain provider lifecycle persistence must have a reload regression that does not expose raw RPC",
);
assert(
  gatewayApi.includes('"sync_health"') &&
    gatewayTests.includes(
      "test_gateway_blocks_chain_proof_prepare_and_broadcast_routes",
    ),
  "Gateway must expose only read-only chain sync health to System and keep proof/prepare/broadcast blocked until a capability approval path exists",
);
assert(
  chainProvider.includes('id: "base-mainnet"') &&
    chainProvider.includes("chain_id: Some(8453)") &&
    chainProvider.includes('"https://mainnet.base.org"'),
  "Chain provider must include Base mainnet using the PC2 Base RPC default",
);
assert(
  viewerGatewayApi.includes("principal_scoped_storage_uri") &&
    viewerGatewayApi.includes("principal_localhost_root"),
  "Viewer/content storage must resolve Users/self through the launch-token principal before hitting disk",
);
assert(
  documentsProvider.includes("DocumentsClient::for_principal"),
  "Documents provider clients must be bound to an explicit runtime principal",
);
assert(
  documentsProvider.includes("documents_load_metadata_for_principal"),
  "Documents provider must verify document ownership before document operations",
);
assert(
  documentsProvider.includes("principal_localhost_root") &&
    !documentsProvider.includes("Users/self/Documents"),
  "Documents working copies must resolve through the runtime principal root, not shared Users/self storage",
);
assert(
  documentsReadme.includes("signed Home launch-token principal"),
  "Documents README must document provider requests as principal-scoped",
);
assert(
  documentsReadme.includes("localhost://Users/<principal-root>/Documents/..."),
  "Documents README must document the real working-copy root as the runtime principal root",
);
assert(
  !documentsReadme.includes("localhost://Users/self/Documents"),
  "Documents README must not present shared Users/self storage as the real working-copy path",
);
assert(
  !notifications.includes("NATIVE_CHAT_ROOT_URI") &&
    !notifications.includes("sync_native_chat_relay"),
  "Notifications must not relay into shared native Chat Users/self storage without a principal",
);
assert(
  carrierBridge.includes("scope_current_user_alias") &&
    carrierBridge.includes("principal_context_required"),
  "Capsule-kernel bridge must scope Users/self through a principal context or fail closed",
);
assert(
  carrierBridge.includes("protected_principal_root_carrier_response") &&
    carrierBridge.includes("write_principal_root_object("),
  "Capsule-kernel bridge must route protected Users/self object writes through runtime principal-root encryption",
);
assert(
  wasmProvider.includes("principal_id: Option<String>") &&
    wasmProvider.includes("bridge_principals"),
  "WASM bridge pipes must carry an explicit runtime principal context",
);
assert(
  runtimeCore.includes("run_local_with_principal") &&
    runtimeCore.includes("set_bridge_principal"),
  "Runtime WASM launches must bind the launch principal before bridge startup",
);
assert(
  runtimeCore.includes("self.run_local_with_principal(path, args, None).await"),
  "Default runtime run_local must stay principal-less and fail closed for Users/self until a principal context is explicit",
);
assert(
  apiRoutes.includes("reject_raw_launch_principal_id") &&
    apiRoutes.includes("launch_principal_id_rejects_raw_values") &&
    apiRoutes.includes(
      "principal_launch_rejects_raw_principal_id_even_with_grant",
    ),
  "/api/capsules must reject raw principal_id authority even when it looks opaque or arrives beside a grant",
);
assert(
  gatewayApi.includes(
    '"launch_grant": issue_home_launch_token_with_context(data_dir, capsule_name, context)?',
  ) &&
    !gatewayApi.includes('"principal_id": context.principal_id.as_str()') &&
    apiRoutes.includes("principal_launch_accepts_home_launch_grant") &&
    apiRoutes.includes("principal_launch_rejects_wrong_app_grant"),
  "Home runtime-backed launch must attach principals through a signed launch grant, not a raw principal_id",
);
assert(
  apiRoutes.includes(
    "principal_launch_rejects_grant_without_runtime_data_dir",
  ) && serveCmd.includes("data_dir: Some(data_dir.clone())"),
  "Managed runtime API must receive data_dir so /api/capsules can validate Home principal launch grants",
);
assert(
  supervisorApi.includes("supervisor_launch_principal_from_input") &&
    supervisorApi.includes("supervisor_launch_accepts_signed_launch_grant") &&
    supervisorApi.includes("supervisor_launch_rejects_wrong_app_grant") &&
    supervisorApi.includes(
      "supervisor_launch_rejects_top_level_principal_authority",
    ) &&
    supervisorApi.includes(
      "supervisor_launch_rejects_config_principal_authority",
    ) &&
    supervisorCore.includes(
      "principal launch grants are only valid for shell-launchable capsules",
    ) &&
    supervisorCore.includes(
      "test_launch_capsule_rejects_principal_for_provider_role",
    ),
  "Supervisor/microVM launch path must accept signed app-scoped launch grants, reject raw principal authority, and keep provider launches out of user scope",
);
assert(
  runtimeControl.includes('"localhost://Users/*"') &&
    !runtimeControl.includes(
      '"localhost://Users/self/.AppData/LocalHost/Chat/*"',
    ) &&
    !runtimeControl.includes(
      '"localhost://Users/self/.AppData/LocalHost/GBA/*"',
    ),
  "Managed runtime policy must allow principal roots, not shared Users/self paths",
);
assert(
  carrierBridge.includes(
    "localhost://Users roots must use Users/self or the active principal root",
  ),
  "Capsule-kernel bridge must reject explicit foreign principal roots",
);
assert(
  gatewayApi.includes("let principal_id = context.principal_id.clone()") &&
    gatewayApi.includes(
      'request["principal_id"] = serde_json::Value::String(principal_id.clone())',
    ),
  "Gateway must inject the Home launch-token principal into Documents provider calls",
);
assert(
  !homeCmd.includes("localhost://Users/self/.AppData/LocalHost/Chat") &&
    homeCmd.includes(
      "localhost://Users/<principal-root>/.AppData/LocalHost/Chat",
    ),
  "Home CLI root descriptors must describe principal-root Users storage, not shared Users/self examples",
);
assert(
  localhostProvider.includes("token: Option<String>") &&
    localhostProvider.includes("test_storage_request_body_token_is_optional") &&
    carrierBridge.includes('object.remove("token")') &&
    carrierBridge.includes(
      "carrier_invoke_localhost_uses_envelope_token_and_redacts_body_token",
    ) &&
    carrierBridge.includes(
      "carrier_invoke_localhost_rejects_missing_envelope_token_even_with_body_token",
    ) &&
    !carrierService.includes('"token": ""') &&
    !homeCmd.includes('"token": access.') &&
    !homeCli.includes('"token": token') &&
    !chatCarrier.includes('"token": storage_token'),
  "Localhost provider storage authority must use the carrier/provider envelope token, not duplicate body token fields",
);
assert(
  !gatewayApi.includes("save_nickname(&state.data_dir, &req.handle)"),
  "System handle updates must not use the device-global nickname path",
);
assert(
  !gatewayApi.includes("load_nickname(data_dir)"),
  "Home/System/Chat gateway identity must not use the device-global nickname path",
);
const gatewayActiveSession =
  gatewayApi.match(
    /struct GatewayActiveSessionSummary \{([\s\S]*?)\n\}/,
  )?.[1] || "";
assert(
  gatewayActiveSession.includes("session_id: String"),
  "Gateway summaries must expose only a public active-session id",
);
assert(
  !gatewayActiveSession.includes("token:"),
  "Gateway summaries must not expose room session tokens",
);
assert(
  shellCore.includes("desktopIconsVisible: true"),
  "Home layout state must track global desktop icon visibility",
);
assert(
  shellSurface.includes('action: "toggle-desktop-icons"'),
  "Home desktop menu must expose desktop icon visibility",
);
assert(
  !shellSurface.includes('label: "Go Home"'),
  "Home desktop menu must not expose redundant Go Home",
);
assert(
  !shellSurface.includes('label: "Open Launcher"'),
  "Home desktop menu must not expose redundant Open Launcher",
);
assert(
  !shellSurface.includes('label: "Open System"'),
  "Home desktop menu must not expose redundant Open System",
);
assert(
  !shellCmd.includes("Legacy fields"),
  "Runtime coords must not preserve legacy token fields",
);
const runtimeCoordsBlock =
  shellCmd.match(/pub struct RuntimeCoords \{([\s\S]*?)\n\}/)?.[1] || "";
assert(
  !runtimeCoordsBlock.includes("shell_token") &&
    !runtimeCoordsBlock.includes("client_token"),
  "Runtime coords must only persist attach_secret, not bearer tokens",
);
assert(
  !operatorControl.includes("struct RuntimeCoords"),
  "Operator control must use canonical shell_cmd runtime coords",
);
assert(
  operatorControl.includes(
    "crate::runtime_control::read_operator_runtime_coords",
  ),
  "Operator control must use canonical runtime coord validation",
);
assert(
  !operatorControl.includes("async fn attach_secret_matches"),
  "Operator control must not duplicate attach-secret validation",
);
assert(
  debugPolicy.includes("# Debugging Policy"),
  "Root DEBUG.md must stay a stable developer policy, not a work log",
);
assert(
  !debugPolicy.includes("pc2-shell") && !debugPolicy.includes("md-viewer"),
  "Root DEBUG.md must not preserve stale route history",
);
assert(
  !/\b(Investigation|Observations|Hypotheses|Experiments|Root Cause|Conclusion|Follow-up)\b|^### Fix$/m.test(
    debugPolicy,
  ),
  "Root DEBUG.md must not contain active debugging log headings",
);

const components = JSON.parse(read("components.json"));
const publishRust = read("elastos/crates/elastos-server/src/publish.rs");
function shellArrayItems(text, name) {
  const pattern = new RegExp(`^${name}=\\(([\\s\\S]*?)^\\)`, "m");
  const match = text.match(pattern);
  assert(Boolean(match), `publish-release must define ${name}`);
  return new Set(
    match[1]
      .split("\n")
      .map((line) => line.trim())
      .filter((line) => /^[A-Za-z0-9_-]+$/.test(line)),
  );
}
function rustConstItems(text, name) {
  const pattern = new RegExp(
    `const\\s+${name}:\\s*&\\[\\&str\\]\\s*=\\s*&\\[([\\s\\S]*?)\\];`,
  );
  const match = text.match(pattern);
  assert(Boolean(match), `publish.rs must define ${name}`);
  return new Set([...match[1].matchAll(/"([^"]+)"/g)].map((entry) => entry[1]));
}
const publishReleaseDefault = shellArrayItems(publishReleaseScript, "DEFAULT_CAPSULES");
const publishReleaseRequired = shellArrayItems(
  publishReleaseScript,
  "REQUIRED_SUPPORTED_CAPSULES",
);
const publishRustHome = rustConstItems(publishRust, "HOME_PUBLISH_CAPSULES");
const publishRustDemo = rustConstItems(publishRust, "DEMO_PUBLISH_CAPSULES");
const publishRustRequired = rustConstItems(
  publishRust,
  "REQUIRED_SUPPORTED_PUBLISH_CAPSULES",
);
const homeProfile = new Set(components.profiles.home.components);
for (const component of [
  "chain-provider",
  "net-provider",
  "exit-provider",
  "browser-engine-adapter",
  "browser-engine-supervisor",
  "browser-native-proxy-engine",
  "browser-stream-bridge",
  "browser-local-exit",
  "object-provider",
  "wallet-provider",
]) {
  assert(homeProfile.has(component), `Home profile must install ${component}`);
  assert(
    components.external[component],
    `${component} must be a first-party external setup asset`,
  );
  assert(
    publishReleaseScript.includes(`    ${component}\n`),
    `publish-release must include ${component} in support binary assets`,
  );
  for (const platform of ["linux-amd64", "linux-arm64"]) {
    const metadata =
      components.external[component].platforms[platform] ||
      components.external[component].platforms["*"];
    assert(
      metadata?.release_path,
      `${component} must publish binary metadata for ${platform}`,
    );
  }
}
const walletBrowserSurfaces = new Set([
  "wallet",
  "wallet-metamask",
  "wallet-unisat",
  "wallet-walletconnect",
  "browser",
  "inbox",
]);
for (const [profileName, profile] of Object.entries(components.profiles)) {
  const profileComponents = new Set(profile.components || []);
  const hasWalletBrowserSurface = [...walletBrowserSurfaces].some((component) =>
    profileComponents.has(component),
  );
  if (hasWalletBrowserSurface) {
    for (const provider of ["chain-provider", "wallet-provider"]) {
      assert(
        profileComponents.has(provider),
        `${profileName} profile must install ${provider} with Wallet/Browser surfaces`,
      );
    }
  }
}
for (const provider of ["chain-provider", "wallet-provider"]) {
  assert(
    publishReleaseDefault.has(provider),
    `publish-release default capsule set must include ${provider}`,
  );
  assert(
    publishReleaseRequired.has(provider),
    `publish-release required supported capsule set must include ${provider}`,
  );
  assert(
    publishRustHome.has(provider),
    `Rust home publish capsule set must include ${provider}`,
  );
  assert(
    publishRustRequired.has(provider),
    `Rust required supported publish capsule set must include ${provider}`,
  );
}
const setupHomeCapsules = new Set(
  [...homeProfile].filter((component) => {
    return (
      existsSync(new URL(`capsules/${component}/capsule.json`, repoRoot)) ||
      existsSync(new URL(`elastos/capsules/${component}/capsule.json`, repoRoot))
    );
  }),
);
for (const component of setupHomeCapsules) {
  assert(
    publishReleaseDefault.has(component),
    `publish-release default capsule set must include setup home capsule ${component}`,
  );
  assert(
    publishReleaseRequired.has(component),
    `publish-release required capsule set must include setup home capsule ${component}`,
  );
  assert(
    publishRustHome.has(component),
    `Rust home publish profile must include setup home capsule ${component}`,
  );
  assert(
    publishRustRequired.has(component),
    `Rust required publish set must include setup home capsule ${component}`,
  );
}
for (const component of [...publishReleaseDefault]) {
  assert(
    setupHomeCapsules.has(component),
    `publish-release default capsule set must not include demo-only capsule ${component}`,
  );
}
for (const component of ["chat", "gba-emulator", "gba-ucity", "chat-room", "ipfs-provider", "tunnel-provider"]) {
  assert(
    publishRustDemo.has(component),
    `Rust demo publish profile must include ${component}`,
  );
}
for (const component of [
  "home",
  "system",
  "services",
  "documents",
  "library",
  "marketplace",
  "archive-manager",
  "inbox",
]) {
  assert(
    homeProfile.has(component),
    `Home profile must install first-party ${component} assets`,
  );
  assert(
    components.external[component],
    `${component} must be a first-party external setup asset`,
  );
  assert(
    publishReleaseScript.includes(`    ${component}\n`) ||
      publishReleaseScript.includes(`        ${component} \\\n`),
    `publish-release must package first-party ${component} assets`,
  );
  for (const platform of ["linux-amd64", "linux-arm64"]) {
    const metadata =
      components.external[component].platforms[platform] ||
      components.external[component].platforms["*"];
    assert(
      metadata?.release_path && metadata?.extract_path,
      `${component} must publish archive metadata for ${platform}`,
    );
  }
}
assert(
  read("capsules/marketplace/browser/marketplace.js").includes(
    'fetch("/api/capsules/catalog"',
  ),
  "Marketplace must read the canonical capsule catalog, not an app-scoped marketplace catalog",
);
const marketplaceUi = read("capsules/marketplace/browser/marketplace.js");
assert(
  marketplaceUi.includes('size: capsule.cid ? "Verified app" : "Local app"') &&
    marketplaceUi.includes('storage: capsule.cid ? "SmartWeb" : "Local"') &&
    marketplaceUi.includes("App identity:") &&
    !marketplaceUi.includes("CID-backed") &&
    !marketplaceUi.includes("Content ID") &&
    !marketplaceUi.includes("Signed package") &&
    !marketplaceUi.includes("Package identity:") &&
    !marketplaceUi.includes('price-tag">${app.cid ? "CID"') &&
    !marketplaceUi.includes("signed CID manifests") &&
    !marketplaceUi.includes("capsule catalog route missing") &&
    !marketplaceUi.includes("`CID:"),
  "Marketplace must describe app trust in user-facing language instead of raw CID/package jargon",
);
assert(
  read("elastos/crates/elastos-server/src/api/gateway_marketplace.rs").includes(
    "capsule_catalog_summary",
  ),
  "Marketplace route must delegate to the canonical capsule catalog",
);
const marketplaceCatalogReadModel = read(
  "elastos/crates/elastos-server/src/api/gateway_capsule_catalog/read_model.rs",
);
assert(
  marketplaceCatalogReadModel.includes("payment_state") &&
    marketplaceCatalogReadModel.includes("drm_state") &&
    marketplaceUi.includes("Install pending") &&
    marketplaceUi.includes("Installing new apps is not available yet.") &&
    marketplaceUi.includes("Supports payments") &&
    marketplaceUi.includes("Uses protected content") &&
    !marketplaceUi.includes("signed CID manifests") &&
    !marketplaceUi.includes("Paid capsules") &&
    !marketplaceUi.includes("Protected capsules"),
  "Marketplace must translate neutral catalog payment and dDRM facts into app-facing language",
);

const documents = read("capsules/documents/browser/index.html");
const archiveManager = read("capsules/archive-manager/browser/index.html");
const archiveManagerManifest = read("capsules/archive-manager/capsule.json");
const inbox = read("capsules/inbox/browser/index.html");
const libraryIndex = read("capsules/library/browser/index.html");
const libraryCss = read("capsules/library/browser/library.css");
const libraryApp = read("capsules/library/browser/src/app.js");
const libraryActions = read("capsules/library/browser/src/actions.js");
const libraryApi = read("capsules/library/browser/src/api.js");
const libraryDialog = read("capsules/library/browser/src/dialog.js");
const libraryEditor = read("capsules/library/browser/src/editor.js");
const libraryEvents = read("capsules/library/browser/src/events.js");
const libraryMenu = read("capsules/library/browser/src/menu.js");
const libraryModel = read("capsules/library/browser/src/model.js");
const libraryNavigation = read("capsules/library/browser/src/navigation.js");
const libraryPreview = read("capsules/library/browser/src/preview.js");
const libraryRealtime = read("capsules/library/browser/src/realtime.js");
const libraryRender = read("capsules/library/browser/src/render.js");
const librarySelection = read("capsules/library/browser/src/selection.js");
const libraryState = read("capsules/library/browser/src/state.js");
const libraryUploads = read("capsules/library/browser/src/uploads.js");
const library = readAll([
  "capsules/library/browser/index.html",
  "capsules/library/browser/library.css",
  "capsules/library/browser/src/app.js",
  "capsules/library/browser/src/actions.js",
  "capsules/library/browser/src/api.js",
  "capsules/library/browser/src/dialog.js",
  "capsules/library/browser/src/editor.js",
  "capsules/library/browser/src/events.js",
  "capsules/library/browser/src/menu.js",
  "capsules/library/browser/src/model.js",
  "capsules/library/browser/src/navigation.js",
  "capsules/library/browser/src/preview.js",
  "capsules/library/browser/src/realtime.js",
  "capsules/library/browser/src/render.js",
  "capsules/library/browser/src/selection.js",
  "capsules/library/browser/src/state.js",
  "capsules/library/browser/src/uploads.js",
]);
const objectProviderManifest = read("capsules/object-provider/capsule.json");
const objectProviderImpl = read("elastos/crates/elastos-server/src/library.rs");
const gatewayProviderProxy = read("elastos/crates/elastos-server/src/api/gateway_provider_proxy.rs");
const gatewayInspectActions = readAll([
  "elastos/crates/elastos-server/src/api/gateway_inspect_actions.rs",
  "elastos/crates/elastos-server/src/api/gateway_inspect_actions/binding.rs",
  "elastos/crates/elastos-server/src/api/gateway_inspect_actions/store.rs",
]);
const libraryGatewayTests = read("elastos/crates/elastos-server/src/api/gateway_tests/library.rs");
const retiredObjectProviderMarkers = {
  oldBinary: ["library", "provider"].join("-"),
  oldNamespace: ["elastos://", "library"].join(""),
  oldSchemeRegistration: [
    'CapsuleProvider::with_scheme(bridge, "',
    "library",
    '")',
  ].join(""),
};
const contentProvider = read("elastos/crates/elastos-server/src/content.rs");
const contentCmd = read("elastos/crates/elastos-server/src/content_cmd.rs");
const availabilityProvider = read("capsules/availability-provider/src/main.rs");
const webspaceProvider = read("capsules/webspace-provider/src/main.rs");
const webspaceCmd = read("elastos/crates/elastos-server/src/webspace_cmd.rs");
const serverMain = read("elastos/crates/elastos-server/src/main.rs");
const providerRegistry = read("elastos/crates/elastos-runtime/src/provider/registry.rs");
const libraryDesktopIcon = read("capsules/library/browser/icons/sidebar-folder-desktop.svg");
const libraryMenuSmoke = read("scripts/library-menu-smoke.mjs");
const libraryPerformanceSmoke = read("scripts/library-performance-smoke.mjs");
const libraryLiveSmoke = read("scripts/library-live-smoke.sh");
const namespacesDoc = read("docs/NAMESPACES.md");
const chatStyle = read("capsules/chat-room/browser/style.css");
const gba = read("capsules/gba-emulator/browser/index.html");
const gbaStyle = read("capsules/gba-emulator/browser/style.css");
const gbaJs = read("capsules/gba-emulator/browser/emulator.js");
const system = read("capsules/system/browser/index.html");
const systemJs = read("capsules/system/browser/system.js");
const systemStyle = read("capsules/system/browser/style.css");
const walletMetamask = read("capsules/wallet-metamask/browser/index.html");
const walletMetamaskJs = read("capsules/wallet-metamask/browser/wallet-metamask.js");
const walletUnisat = read("capsules/wallet-unisat/browser/index.html");
const walletUnisatJs = read("capsules/wallet-unisat/browser/wallet-unisat.js");
const wallet = read("capsules/wallet/browser/index.html");
const walletJs = readAll([
  "capsules/wallet/browser/wallet.js",
  "capsules/wallet/browser/wallet-account-actions.js",
  "capsules/wallet/browser/wallet-activity.js",
  "capsules/wallet/browser/wallet-api.js",
  "capsules/wallet/browser/wallet-create-account-flow.js",
  "capsules/wallet/browser/wallet-flows.js",
  "capsules/wallet/browser/wallet-format.js",
  "capsules/wallet/browser/wallet-preferences.js",
  "capsules/wallet/browser/wallet-receive-flow.js",
  "capsules/wallet/browser/wallet-requests.js",
  "capsules/wallet/browser/wallet-render.js",
  "capsules/wallet/browser/wallet-send-flow.js",
  "capsules/wallet/browser/wallet-state.js",
]);
const walletStyle = read("capsules/wallet/browser/style.css");
const browserManifest = read("capsules/browser/capsule.json");
const browser = read("capsules/browser/browser/index.html");
const browserJs = readAll([
  "capsules/browser/browser/browser.js",
  "capsules/browser/browser/browser-clipboard.js",
  "capsules/browser/browser/browser-history.js",
  "capsules/browser/browser/browser-input.js",
  "capsules/browser/browser/browser-input-surface.js",
  "capsules/browser/browser/browser-location.js",
  "capsules/browser/browser/browser-remote-display.js",
  "capsules/browser/browser/browser-runtime-api.js",
  "capsules/browser/browser/browser-status.js",
  "capsules/browser/browser/browser-webrtc.js",
]);
const browserStyle = read("capsules/browser/browser/style.css");
assert(
  browserJs.includes('window.addEventListener("beforeunload"') &&
    browserJs.includes('window.addEventListener("pagehide", releaseRuntimePageForUnload)') &&
    browserJs.includes("window.__elastosBrowserCurrentPageId") &&
    !browserJs.includes("window.__elastosBrowserReleaseRuntimePage") &&
    gatewayApi.includes('"/api/apps/browser/pages/:page_id/close"') &&
    gatewayBrowserApi.includes("pub(super) async fn browser_app_page_close") &&
    gatewayBrowserRouteTests.includes(
      '.uri(format!("/api/apps/browser/pages/{page_id}/close"))',
    ),
  "Browser must own unload cleanup and the gateway must route close_page so iframe teardown releases singleton providers without a Home-callable frame hook",
);
const walletWalletconnect = read("capsules/wallet-walletconnect/browser/index.html");
const walletWalletconnectJs = read(
  "capsules/wallet-walletconnect/browser/wallet-walletconnect.js",
);
const homeSmoke = read("scripts/home-camofox-smoke.mjs");
const systemSmoke = read("scripts/system-camofox-smoke.mjs");
const homeVirtualAuthSmoke = read(
  "scripts/home-passkey-virtual-auth-smoke.mjs",
);
const authWalletSmoke = read("scripts/auth-wallet-focus-smoke.sh");
const walletconnectVendorScript = read(
  "scripts/vendor-walletconnect-adapter.sh",
);
const walletconnectConfigScript = read(
  "scripts/configure-walletconnect-connector.mjs",
);
const walletconnectConfigSmoke = read(
  "scripts/walletconnect-connector-config-smoke.sh",
);
const walletProviderDoc = read("docs/WALLET_PROVIDER.md");
const systemAssetVersion = "system-20260712a";
const shellAuth = read("capsules/home/browser/shell-auth.js");
const protectedHomeStateSmoke = read("scripts/protected-home-state-smoke.sh");
assert(
  !documents.includes("home:open-uri"),
  "Documents published URI sharing must copy the elastos:// link, not reopen itself",
);
assert(
  documents.includes("/api/provider/documents/"),
  "Documents must use the runtime documents provider API",
);
assert(
  documents.includes('"x-elastos-home-token"'),
  "Documents provider API calls must carry the Home capacity token",
);
assert(
  !documents.includes("/ipfs/"),
  "Documents capsule must not call direct IPFS HTTP routes",
);
assert(
  !documents.includes("IpfsBridge"),
  "Documents capsule must not instantiate IPFS directly",
);
assert(
  !documents.includes("ipfs-provider"),
  "Documents capsule must not know provider-specific IPFS errors",
);
assert(
  documents.includes("item.latest_published_cid === requestedCid"),
  "Documents shell CID launches must resolve local published documents before public CID loading",
);
assert(
  documents.includes("allowRawCidFallback") &&
    documents.includes("loadRawCidMode") &&
    documents.includes('"/content/" + encodeURIComponent(cleanCid)'),
  "Documents CID launches must fall back to raw content objects when the CID is not a Documents share bundle",
);
assert(
  documents.includes("documents:open-chat-attachment") &&
    documents.includes("openChatAttachment") &&
    documents.includes("dataUrlToUtf8") &&
    documents.includes('documentsProviderApi("import_chat_attachment"') &&
    documents.includes("Opened the Chat attachment.") &&
    !documents.includes("/api/apps/chat-room/attachments/"),
  "Documents must import Chat attachment payloads through its provider after Home delivery, not call Chat Room routes directly",
);
assert(
  documents.includes('id="copy-published-link"'),
  "Documents published URI must be copied from the toolbar action row",
);
assert(
  documents.includes("navigator.clipboard.writeText"),
  "Documents Copy link must write the elastos:// URI to the clipboard",
);
assert(
  !documents.includes("Open elastos://"),
  "Documents must not label published URIs as an Open action",
);
assert(
  documents.includes("confirmInCapsule"),
  "Documents destructive actions must use in-surface confirmation",
);
assert(
  !documents.includes("window.confirm"),
  "Documents must not use browser confirm for destructive actions",
);
assert(
  !documents.includes("object-uri-pill"),
  "Documents shell must not render a duplicate document URI pill",
);
assert(
  !documents.includes("published-pill"),
  "Documents shell must not render a duplicate published CID pill",
);
assert(
  !documents.includes("document-list-badge"),
  "Documents list must not use a text Published badge",
);
assert(
  documents.includes("document-list-published-icon"),
  "Documents list must show published state with an icon",
);
assert(
  documents.includes("document-list-item.published"),
  "Documents published rows must be visually distinct",
);
assert(
  documents.includes('aria-label="New document"'),
  "Documents create control must keep an accessible label",
);
assert(
  documents.includes("sidebar-controls"),
  "Documents create/search controls must share one compact row",
);
assert(
  !documents.includes('id="documents-count"'),
  "Documents sidebar must not render a duplicate document count",
);
assert(
  !documents.includes("sidebar-meta-label"),
  "Documents sidebar must not render duplicate section labels",
);
assert(
  !documents.includes("Start writing, then save."),
  "Documents must not show redundant draft instruction copy",
);
assert(
  !documents.includes("meta-pill"),
  "Documents must not use generic pill chrome for document state",
);
assert(
  !documents.includes("local-state-chip"),
  "Documents shell must not render duplicate draft state under the title",
);
assert(
  !documents.includes("updated-text"),
  "Documents shell must not render duplicate last-saved text under the title",
);
assert(
  !documents.includes("Delete document?"),
  "Documents delete confirmation must not repeat a modal title",
);
assert(
  documents.includes('class="action-primary action-icon-button"'),
  "Documents primary action must use compact icon buttons",
);
assert(
  documents.includes('aria-label="Save"'),
  "Documents Save icon button must keep an accessible label",
);
assert(
  documents.includes('aria-label="Hide list"'),
  "Documents Hide list icon button must keep an accessible label",
);
assert(
  documents.includes(".page-shell {\n    padding: 0;"),
  "Documents mobile shell must not add an extra outer gutter",
);
assert(
  documents.includes(
    ".documents-main,\n  .share-main {\n    padding: 0.38rem;",
  ),
  "Documents mobile panels must use compact padding",
);
assert(
  inbox.includes("button.dataset.actionId = actionId;"),
  "Inbox actions must expose stable action ids",
);
assert(
  inbox.includes("home:open-target"),
  "Inbox source-app opens must use Home orchestration",
);
const inboxWalletApprovalBoundary = {
  inboxCanApproveWithFreshPasskey: inbox.includes("requestFreshPasskeyHomeToken") &&
    inbox.includes("/api/auth/passkey/authenticate/begin") &&
    inbox.includes("navigator.credentials.get") &&
    inbox.includes("home_token: homeToken") &&
    inbox.includes('inboxAction("wallet-approve-request:" + requestId'),
  inboxKeepsWalletDeepLink: inbox.includes("Review in Wallet") &&
    inbox.includes('openSource("wallet", { wallet_request: requestId })'),
  gatewayRequiresFreshPasskeyForInboxSigning: gatewayInboxApi.includes(
    "fresh passkey verification is required to sign with a built-in wallet",
  ) &&
    gatewayInboxApi.includes(
      "require_fresh_passkey_home_token(data_dir, home_token, context, 180)?",
    ),
  gatewayApprovesManagedWalletFromInbox: gatewayInboxApi.includes(
    "approve_managed_wallet_request(",
  ) &&
    gatewayInboxApi.includes('"Approved in Inbox"') &&
    gatewayInboxApi.includes("INBOX_CAPSULE_ID"),
  gatewayReadsInboxHomeToken: gatewayInboxApi.includes(
    "action.home_token.as_deref()",
  ),
  gatewayModelAllowsInboxHomeToken: gatewayApi.includes(
    "home_token: Option<String>",
  ),
  gatewayTestCoversInboxSigning: gatewayTests.includes(
    "test_inbox_approves_wallet_requests_through_runtime_wallet_signing",
  ) &&
    gatewayTests.includes("fresh passkey verification is required") &&
    gatewayTests.includes("Approved and signed by built-in wallet."),
  staleWalletOnlyRejectionRemoved: !gatewayInboxApi.includes(
    "Open Wallet to approve wallet signing requests.",
  ),
  homeGrantsWebAuthnOnlyToApprovalSurfaces: shellWindows.includes(
    'const WEBAUTHN_IFRAME_ALLOW_TARGETS = new Set(["inbox", "wallet"])',
  ),
  wciAllowsInboxFreshAuthOnly: wciAlignmentScript.includes(
    "--glob '!capsules/inbox/browser/*'",
  ) &&
    wciAlignmentScript.includes(
      "Inbox may request fresh passkey authentication for wallet or Inspector approval, but must not register passkeys",
    ),
  walletOwnsFreshPasskeyToken: walletJs.includes("requestFreshPasskeyHomeToken"),
  walletReadsFocusedRequest: walletJs.includes('readQueryParam("wallet_request")'),
  walletMarksFocusedRequest: walletJs.includes("wallet-request-focused"),
};
assert(
  Object.values(inboxWalletApprovalBoundary).every(Boolean),
  "Inbox wallet approval must approve through fresh-passkey Runtime delegation and keep the Wallet review deep-link",
  inboxWalletApprovalBoundary,
);
const inboxInspectorApprovalBoundary = {
  inboxCanApproveWithFreshPasskey: inbox.includes("approveInspectRequest") &&
    inbox.includes('inboxAction("inspect-approve-request:" + requestId') &&
    inbox.includes("requestFreshPasskeyHomeToken") &&
    inbox.includes("home_token: homeToken") &&
    inbox.includes("Confirm with your passkey to approve this System action."),
  gatewayRequiresFreshPasskeyForInspectApproval: gatewayInboxApi.includes(
    "fresh passkey verification is required to approve an Inspector action",
  ) &&
    gatewayInboxApi.includes(
      "require_fresh_passkey_home_token(data_dir, home_token, context, 180)?",
    ) &&
    gatewayInboxApi.includes("approve_inspect_action_request(state, context, request_id)"),
  gatewayTestsCoverFreshInspectorProof: gatewayTests.includes(
    "inspect_action_requires_inbox_approval_before_dispatch",
  ) &&
    gatewayTests.includes("missing_fresh_proof") &&
    gatewayTests.includes("inspect_action_rejects_stale_fresh_passkey_before_dispatch") &&
    gatewayTests.includes('message.contains("auth session")') &&
    gatewayTests.includes("other.home_token.as_str()"),
  docsDeclareFreshInspectorProof: capsuleInspectorDocs.includes(
    "fresh same-principal passkey Home token",
  ) &&
    capsuleInspectorDocs.includes(
      "Verifies the fresh passkey Home token belongs to the same principal",
    ),
};
assert(
  Object.values(inboxInspectorApprovalBoundary).every(Boolean),
  "Inbox Inspector approval must require fresh same-principal passkey proof before approved provider dispatch",
  inboxInspectorApprovalBoundary,
);
assert(
  inbox.includes('entry.kind !== "wallet_approval_request"') &&
    inbox.includes("const unreadIdSet = new Set(unreadIds)") &&
    inbox.includes("unread_count: unreadCount") &&
    !inbox.includes("renderInbox(Object.assign({}, notifications, { unread_count: 0 }))"),
  "Inbox auto-read must not locally mark wallet approval requests as read when clearing ordinary visible notifications",
);
assert(
  inbox.includes("capability-approve-request:") &&
    inbox.includes("capability-deny-request:") &&
    inbox.includes("inspect-approve-request:") &&
    inbox.includes("inspect-deny-request:") &&
    inbox.includes('entry.kind !== "inspect_action_request"') &&
    inbox.includes("wallet-price-http-approve:") &&
    inbox.includes("wallet-price-http-deny:") &&
    gatewayApi.includes("append_runtime_capability_notifications") &&
    gatewayApi.includes("/api/capability/pending") &&
    gatewayTests.includes(
      "test_capsule_capability_requests_render_as_inbox_notifications",
    ),
  "Capsule capability, Inspector action, and approved external HTTP requests must surface in Inbox with approve/deny actions",
);
assert(
  inbox.includes("min-height: 100dvh;"),
  "Inbox must use dynamic viewport height",
);
assert(
  inbox.includes(".sidebar {\n        display: none;") &&
    inbox.includes("border-radius: 0;"),
  "Inbox mobile layout must collapse the sidebar and keep compact Home-aligned panels",
);
assert(
  libraryApi.includes("home:open-target"),
  "Library opens must use Home orchestration",
);
{
  const openObjectStart = libraryActions.indexOf("async function openObject(object)");
  const viewerIndex = libraryActions.indexOf("const viewer = viewerOptions(object)[0];", openObjectStart);
  const previewIndex = libraryActions.indexOf("if (canPreviewObject(object))", openObjectStart);
  assert(
    openObjectStart >= 0 && viewerIndex > openObjectStart && previewIndex > viewerIndex,
    "Library double-click must prefer the installed default viewer before falling back to internal preview",
  );
}
assert(
  libraryApi.includes("home:deliver-to-target"),
  "Library picker returns must use Home orchestration",
);
assert(
  libraryApi.includes("home:close-self"),
  "Library picker must close itself through Home after a successful attach",
);
assert(
  libraryActions.includes("chat-room:attach-library-item"),
  "Library must return selected documents using the Chat Room attach contract",
);
assert(
  libraryActions.includes("downloadObjectRaw({ uri: object.uri })"),
  "Library Chat Room attachments must read bytes through the object-provider download path",
);
assert(
  libraryActions.includes("blob: raw.blob") &&
    !libraryActions.includes('uri: "elastos://" + cid'),
  "Library attach mode must return a file Blob payload, not a raw published CID text URI",
);
assert(
  libraryApp.includes("Choose an item for Chat.") &&
    libraryApp.includes("Choose an item for Browser.") &&
    libraryApp.includes("Select for Browser") &&
    libraryApp.includes("Attach to Chat") &&
    !libraryApp.includes("Choose a published object for Chat Room."),
  "Library attach mode must not imply that Chat Room attachments require publishing, and must expose target-specific picker actions",
);
assert(
  libraryApp.includes('desktop: "icons/sidebar-folder-desktop.svg"') &&
    libraryApp.includes('"icons/trash.svg"') &&
    libraryApp.includes('"icons/trash-full.svg"'),
  "Library sidebar must expose Desktop and visible provider-backed Trash with empty/full icons",
);
assert(
  libraryDesktopIcon.includes('width="12px"') && libraryDesktopIcon.includes('height="12px"'),
  "Library Desktop sidebar icon must use compact sidebar sizing",
);
assert(
  gatewayApi.includes("HOME_SYSTEM_DESKTOP_OBJECT_SCHEMA") &&
    gatewayApi.includes("home_trash_desktop_object") &&
    gatewayApi.includes('"system_kind": "trash"') &&
    shellCore.includes('targetId === "trash-full"') &&
    shellSurface.includes("function isTrashDesktopObject") &&
    shellSurface.includes("Open Trash") &&
    shellSurface.includes('action: "empty-trash"') &&
    libraryApp.includes('state.initialAction === "empty-trash"'),
  "Home desktop must expose provider-backed Trash as a system desktop object",
);
assert(
    objectProviderManifest.includes('"role": "provider"') &&
    objectProviderManifest.includes('"name": "object-provider"') &&
    objectProviderManifest.includes('"provides": "elastos://object/*"') &&
    objectProviderManifest.includes('"resource": "elastos://object/*"') &&
    !objectProviderManifest.includes(retiredObjectProviderMarkers.oldNamespace) &&
    objectProviderManifest.includes('"audit_events"') &&
    ["publish", "unpublish", "repair", "share", "events"].every((op) =>
      objectProviderManifest.includes(`"${op}"`),
    ),
  "Object provider must be the only provider capsule authority metadata for every routed operation",
);
assert(
  serverInfra.includes('find_installed_provider_binary("object-provider")') &&
    !serverInfra.includes(
      `find_installed_provider_binary("${retiredObjectProviderMarkers.oldBinary}")`,
    ) &&
    serverInfra.includes('CapsuleProvider::with_scheme(bridge.clone(), "object")') &&
    !serverInfra.includes(retiredObjectProviderMarkers.oldSchemeRegistration) &&
    !serverInfra.includes("ObjectProvider::new("),
  "Runtime server must register object-provider only, without the retired object provider alias",
);
assert(
  objectProviderImpl.includes('("desktop", "Desktop", format!("{root}/Desktop"), "directory")') &&
    objectProviderImpl.includes('id: "trash"') &&
    objectProviderImpl.includes('label: "Trash"') &&
    objectProviderImpl.includes('format!("{root}/.Trash")') &&
    objectProviderImpl.includes('"elastos.library.trash-root/v1"'),
  "Object provider roots must expose Desktop and visible provider-backed Trash",
);
assert(
  gatewayApi.includes('"/api/provider/object/upload"') &&
    gatewayApi.includes('"/api/provider/object/upload/start"') &&
    gatewayApi.includes('"/api/provider/object/upload/:upload_id/chunk"') &&
    gatewayApi.includes('"/api/provider/object/upload/:upload_id/finish"') &&
    gatewayApi.includes("LIBRARY_UPLOAD_CHUNK_MAX_BYTES") &&
    gatewayApi.includes("pub(super) async fn gateway_library_upload") &&
    gatewayApi.includes("pub(super) async fn gateway_library_upload_start") &&
    gatewayApi.includes("pub(super) async fn gateway_library_upload_chunk") &&
    gatewayApi.includes("pub(super) async fn gateway_library_upload_finish") &&
    gatewayApi.includes("elastos.object.upload-session/v1") &&
    gatewayApi.includes("http-chunk-session") &&
    gatewayApi.includes("client_waits_for_chunk_ack") &&
    gatewayApi.includes("LIBRARY_TRANSFER_RECEIPT_SCHEMA") &&
    gatewayApi.includes("x-elastos-transfer-receipt") &&
    objectProviderImpl.includes("pub fn handle_library_upload_bytes(") &&
    objectProviderImpl.includes("fn write_library_file_bytes("),
  "Object-provider raw browser uploads must route through Runtime auth/audit, emit transfer receipts, and use the shared object-provider byte-write helper",
);
assert(
  gatewayApi.includes('"/api/provider/object/download/raw"') &&
    gatewayApi.includes("pub(super) async fn gateway_library_download") &&
    gatewayApi.includes("LIBRARY_TRANSFER_RECEIPT_SCHEMA") &&
    gatewayApi.includes("x-elastos-transfer-receipt") &&
    gatewayApi.includes("fn library_download_byte_range(") &&
    gatewayApi.includes("StatusCode::PARTIAL_CONTENT") &&
    gatewayApi.includes("StatusCode::RANGE_NOT_SATISFIABLE") &&
    gatewayApi.includes("LibraryArchiveFormat::parse") &&
    gatewayApi.includes('"archive" => archive_format_value = Some(value.into_owned())') &&
    objectProviderImpl.includes("pub(crate) enum LibraryArchiveFormat") &&
    objectProviderImpl.includes("pub(crate) fn handle_library_download_bytes_with_format(") &&
    objectProviderImpl.includes("pub(crate) async fn handle_library_download_bytes_runtime(") &&
    objectProviderImpl.includes("archive_format: LibraryArchiveFormat") &&
    objectProviderImpl.includes("async fn webspace_download_bytes(") &&
    objectProviderImpl.includes("fn library_download_object(") &&
    libraryActions.includes("downloadObjectRaw({ uri: object.uri })") &&
    libraryActions.includes('downloadObjectRaw({ uri: object.uri, archive: "zip" })') &&
    !libraryActions.includes('providerApi("download"') &&
    !libraryActions.includes("base64ToBlob"),
  "Library raw browser downloads must route through Runtime auth/audit and the shared Library/WebSpace byte-read helpers without JSON/base64 app fallbacks",
);
assert(
  gatewayApi.includes('"compress_archive"') &&
    objectProviderImpl.includes("CompressArchive {") &&
    objectProviderImpl.includes("fn compress_library_archive(") &&
    objectProviderImpl.includes("fn archive_library_single_zip(") &&
    objectProviderImpl.includes("archive_library_selection_zip(data_dir, principal_id, &targets)") &&
    objectProviderImpl.includes('capabilities.push("compress_archive")') &&
    libraryActions.includes('providerApi("compress_archive"') &&
    libraryActions.includes("async function compressObjectToZip(object)") &&
    libraryActions.includes("async function compressSelectedObjectsToZip()") &&
    libraryApp.includes('menuAction("Compress to ZIP"') &&
    libraryApp.includes('menuAction("Compress Selected to ZIP"') &&
    libraryState.includes('"compress_archive"'),
  "Library Compress to ZIP must be provider-owned, capability-gated, cache-invalidating, and available for single objects and same-folder selections",
);
assert(
    archiveManagerManifest.includes('"name": "archive-manager"') &&
    archiveManagerManifest.includes('"role": "viewer"') &&
    archiveManager.includes("<title>Archive - ElastOS</title>") &&
    archiveManager.includes("/api/viewers/archive-manager/library-object") &&
    archiveManager.includes('url.searchParams.set("stat_only", "true")') &&
    archiveManager.includes('url.searchParams.set("entries", "true")') &&
    archiveManager.includes("/api/viewers/archive-manager/library-roots") &&
    archiveManager.includes('url.searchParams.set("preview_entry", path)') &&
    archiveManager.includes('aria-label="Archive contents"') &&
    archiveManager.includes("<strong>Open archive</strong>") &&
    archiveManager.includes("<strong>New ZIP</strong>") &&
    archiveManager.includes('id="open-archive-button"') &&
    archiveManager.includes('id="new-archive-button"') &&
    !archiveManager.includes('class="archive-mark"') &&
    !archiveManager.includes("Work with archives from Library.") &&
    !archiveManager.includes("Archive never edits storage directly") &&
    !archiveManager.includes("Safety details") &&
    !archiveManager.includes("Technical details") &&
    !archiveManager.includes("Runtime and Library services") &&
    !archiveManager.includes("Entry listing unavailable") &&
    !archiveManager.includes("Choose a Library destination") &&
    !archiveManager.includes("viewers:") &&
    archiveManager.includes('mode: intent === "create" ? "archive-create" : "archive-open"') &&
    archiveManager.includes('returnTarget: "archive-manager"') &&
    archiveManager.includes('archive:open-library-object') &&
    archiveManager.includes("async function openLibraryObject(object)") &&
    archiveManager.includes('new URL("/apps/library/", window.location.origin)') &&
    archiveManager.includes("Extract selected") &&
    archiveManager.includes("Extract all") &&
    archiveManager.includes("Select visible") &&
    archiveManager.includes("Archive files load when this format is supported.") &&
    archiveManager.includes("Select files to extract.") &&
    !archiveManager.includes("Cancel pending") &&
    !archiveManager.includes("Runtime Boundary") &&
    archiveManager.includes("handleEntryKeyboard") &&
    archiveManager.includes("Preview loaded through Runtime.") &&
    archiveManager.includes("async function extractSelectedEntries()") &&
    archiveManager.includes("async function extractAllEntries()") &&
    archiveManager.includes("async function selectPreviewEntry(path)") &&
    archiveManager.includes("renderEntries()") &&
    archiveManager.includes("This archive format needs review before extraction.") &&
    objectProviderImpl.includes("LIBRARY_ARCHIVE_ENTRIES_SCHEMA") &&
    objectProviderImpl.includes("LIBRARY_ARCHIVE_EXTRACT_ENTRIES_SCHEMA") &&
    objectProviderImpl.includes("LIBRARY_ARCHIVE_PREVIEW_ENTRY_SCHEMA") &&
    objectProviderImpl.includes("MAX_ARCHIVE_LIST_ENTRIES") &&
    objectProviderImpl.includes("MAX_ARCHIVE_PREVIEW_BYTES") &&
    objectProviderImpl.includes("ArchiveEntries {") &&
    objectProviderImpl.includes("ArchivePreviewEntry {") &&
    objectProviderImpl.includes("ArchiveExtractEntries {") &&
    objectProviderImpl.includes("fn library_archive_entries(") &&
    objectProviderImpl.includes("fn archive_preview_entry(") &&
    objectProviderImpl.includes("fn archive_preview_entry_for_object(") &&
    objectProviderImpl.includes("fn preview_zip_archive_entry(") &&
    objectProviderImpl.includes("fn preview_tar_archive_entry(") &&
    objectProviderImpl.includes("fn extract_library_archive_entries(") &&
    objectProviderImpl.includes("fn extract_archive_entries_to_webspace_destination(") &&
    objectProviderImpl.includes("fn ensure_webspace_archive_write_allowed(") &&
    objectProviderImpl.includes("fn webspace_archive_object(") &&
    objectProviderImpl.includes("resolver_target_redacted") &&
    objectProviderImpl.includes("enum ArchiveConflictPolicy") &&
    objectProviderImpl.includes("fn normalized_archive_entry_path(") &&
    objectProviderImpl.includes('vec!["archive-manager"]') &&
    objectProviderImpl.includes('"archive-manager" => "Archive"') &&
    gatewayApi.includes('name == "archive-manager"') &&
    viewerGatewayApi.includes("stat_only") &&
    viewerGatewayApi.includes("entries: bool") &&
    viewerGatewayApi.includes("preview_entry: Option<String>") &&
    viewerGatewayApi.includes('"archive_entries"') &&
    viewerGatewayApi.includes('"archive_preview_entry"') &&
    viewerGatewayApi.includes('"archive_extract_entries"') &&
    viewerGatewayApi.includes("viewer_library_roots_get") &&
    viewerGatewayApi.includes("viewer_library_object_post") &&
    viewerGatewayApi.includes("ensure_viewer_can_view_library_object") &&
    viewerGatewayApi.includes("handle_object_provider_runtime_request") &&
    viewerGatewayApi.includes("viewer supports Library object metadata only") &&
    viewerGatewayApi.includes("viewer does not support Library object writes") &&
    libraryActions.includes("query.archiveSupport = JSON.stringify") &&
    libraryActions.includes("contentCid(object)") &&
    libraryActions.includes("function deliverArchiveObject(object)") &&
    libraryActions.includes('isArchiveObject(object) && openWithViewer(object, "archive-manager")') &&
    libraryActions.includes('type: "archive:open-library-object"') &&
    !libraryActions.includes("function archiveLibraryObjectPayload(") &&
    libraryModel.includes("export function isArchiveObject(object)") &&
    libraryModel.includes("export function archiveLibraryObjectPayload(object)") &&
    libraryModel.includes("metadata?.archive_support") &&
    libraryModel.includes("isArchiveName(name)") &&
    libraryState.includes('"archive-open"') &&
    libraryState.includes('"archive-create"') &&
    !libraryState.includes("archiveMode") &&
    libraryApp.includes("function completeArchivePicker()") &&
    !libraryApp.includes("function archiveLibraryObjectPayload(") &&
    !libraryApp.includes("function pickerInstruction()") &&
    !libraryApp.includes("Choose an archive, then double-click it or press Open in Archive.") &&
    !libraryApp.includes("Select one item, or several same-folder items, then press Create ZIP.") &&
    libraryApp.includes('type: "archive:open-library-object"') &&
    libraryMenuSmoke.includes("Legacy.7z") &&
    libraryMenuSmoke.includes("Loose.zip") &&
    libraryMenuSmoke.includes('message?.type === "home:deliver-to-target"') &&
    libraryMenuSmoke.includes("archive_entries") &&
    libraryMenuSmoke.includes("archive_preview_entry") &&
    libraryMenuSmoke.includes("archive_extract_entries") &&
    libraryMenuSmoke.includes("Nested/deep.txt") &&
    libraryMenuSmoke.includes("#destination-roots") &&
    archiveManager.includes("function isWritableDestinationRoot(root)") &&
    archiveManager.includes('root.kind === "webspace-root") return false') &&
    libraryMenuSmoke.includes("#entry-preview") &&
    libraryMenuSmoke.includes("#select-all-safe") &&
    libraryMenuSmoke.includes("#extract-all") &&
    libraryMenuSmoke.includes("#open-existing-archive") &&
    libraryMenuSmoke.includes("#make-new-archive") &&
    libraryMenuSmoke.includes('get("mode") === "archive-open"') &&
    libraryMenuSmoke.includes('get("mode") === "archive-create"') &&
    libraryMenuSmoke.includes('get("returnTarget") === "archive-manager"') &&
    !libraryMenuSmoke.includes("#cancel-extract") &&
    libraryMenuSmoke.includes("#extract-status") &&
    libraryMenuSmoke.includes("policy_gated_unsupported_archive_family") &&
    libraryMenuSmoke.includes('message?.target === "archive-manager"') &&
    shellJs.includes('library: new Set(["archive-manager", "browser", "chat-room"])') &&
    libraryGatewayTests.includes("/api/viewers/archive-manager/library-object?uri=") &&
    libraryGatewayTests.includes("test_library_provider_lists_supported_archive_entries_through_viewer_route") &&
    libraryGatewayTests.includes("test_library_provider_lists_unsafe_archive_entries_as_blocked") &&
    libraryGatewayTests.includes("test_library_provider_selectively_extracts_archive_entries_through_viewer_route") &&
    libraryGatewayTests.includes("test_library_provider_selective_extract_blocks_unsafe_entries") &&
    libraryGatewayTests.includes("test_library_gateway_lists_external_webspace_archive_entries_without_resolver_leak") &&
    libraryGatewayTests.includes("test_library_gateway_imports_external_webspace_archive_entries_to_local_library") &&
    libraryGatewayTests.includes("test_library_gateway_webspace_archive_writeback_requires_mutable_write_adapter") &&
    libraryGatewayTests.includes("elastos.library.archive-preview-entry/v1") &&
    libraryGatewayTests.includes("/api/viewers/archive-manager/library-roots") &&
    libraryGatewayTests.includes("/api/provider/object/archive_preview_entry") &&
    libraryGatewayTests.includes("resolver_target_redacted") &&
    libraryGatewayTests.includes("mutable destination Space") &&
    libraryGatewayTests.includes("elastos.library.archive-entries/v1") &&
    libraryGatewayTests.includes("elastos.library.archive-extract-entries/v1") &&
    libraryGatewayTests.includes("StatusCode::FORBIDDEN") &&
    archivePolicyDoc.includes("No generic non-tar/non-zip family is approved in this branch") &&
    archivePolicyDoc.includes(".7z") &&
    archivePolicyDoc.includes(".rar") &&
    archivePolicyDoc.includes("Unsupported families remain visible as policy-gated archives"),
  "Archive must provide an installed viewer shell, stat-only/archive-entry/preview/root viewer routes, supported-family browsing, preview, selected/all extraction, policy-gated unsupported archive UX, conflict receipts, policy documentation, and no direct viewer provider access or unsafe extraction",
);
assert(
  gatewayApi.includes("HOME_DESKTOP_OBJECTS_SCHEMA") &&
    gatewayApi.includes("home_desktop_objects_summary") &&
    gatewayApi.includes("home_desktop_events_signature") &&
    gatewayApi.includes("is_home_desktop_object_layout_entry") &&
    gatewayApi.includes('entry.strip_prefix("object:")') &&
    gatewayApi.includes('format!("{root}/.Trash")') &&
    gatewayApi.includes('registry.send_raw("object", &request).await') &&
    gatewayApi.includes('"op": "list"') &&
    gatewayApi.includes('"op": "events"') &&
    gatewayApi.includes('"uri": uri'),
  "Home desktop must project localhost://Users/self/Desktop through the registered object provider, not direct filesystem access or server-local helpers",
);
assert(
  shellCore.includes("function desktopObjects(summary)") &&
    shellCore.includes("function desktopObjectEntryId(object)") &&
    shellCore.includes("function desktopLayoutEntries(summary)"),
  "Home layout state must treat Library Desktop objects as first-class desktop entries",
);
assert(
  shellSurface.includes("attachDesktopObjectInteractions") &&
    shellSurface.includes("export function openSelectedDesktopEntry()") &&
    shellSurface.includes('openTarget("library", { query: { uri: object.uri } })') &&
    shellSurface.includes("desktopObjectViewer(object)") &&
    shellSurface.includes('openTarget(viewer,') &&
    shellSurface.includes('objectUri: object.uri') &&
    shellSurface.includes("function desktopObjectContextMenuItems(target)") &&
    shellSurface.includes('action: "open-desktop-object-new-window"') &&
    shellSurface.includes('action: "download-desktop-object"') &&
    shellSurface.includes('action: "properties-desktop-object"') &&
    shellSurface.includes("function libraryActionForObject(object, action)") &&
    shellSurface.includes('action === "download-desktop-object" ? "download" : "properties"') &&
    shellJs.includes("openSelectedDesktopEntry()") &&
    !shellJs.includes("openTarget(shellState.selectedDesktopTargetId)"),
  "Home desktop objects must open and expose object actions through Library/Documents orchestration",
);
assert(
  libraryState.includes("initialObjectUri: queryParams.get(\"objectUri\")") &&
    libraryState.includes("initialAction: queryParams.get(\"action\")") &&
    libraryState.includes("initialActionHandled: false") &&
    libraryApp.includes("async function runInitialObjectAction()") &&
    libraryApp.includes("const object = objectByUri(state.initialObjectUri)") &&
    libraryApp.includes("state.currentObject?.uri === state.initialObjectUri") &&
    libraryApp.includes('state.initialAction === "properties"') &&
    libraryApp.includes('state.initialAction === "download"') &&
    libraryApp.includes("await downloadObject(object)"),
  "Library must accept Home-delegated object actions by launch query without moving object-provider authority into Home",
);
assert(
  shellJs.includes("desktopObjectsChanged(previous, summary)") &&
    shellJs.includes('kind === "home.desktop.changed"'),
  "Home shell must refresh when Library Desktop objects change",
);
assert(
  libraryApp.includes('menuAction("Open With", null') &&
    libraryApp.includes("children: viewers.map") &&
    libraryApp.includes('menuAction("Sort By", null') &&
    libraryApp.includes('menuAction("Name"') &&
    libraryApp.includes('menuAction("Date Modified"') &&
    libraryApp.includes('menuAction("New", null') &&
    libraryApp.includes('menuAction("Folder"') &&
    libraryApp.includes('menuAction("Show Hidden"') &&
    libraryApp.includes("library.showHidden") &&
    libraryApp.includes('menuAction("Paste Into Folder"') &&
    libraryApp.includes('menuAction("Properties"') &&
    libraryApp.includes('menuAction("Delete"') &&
    libraryApp.includes('if (canPasteInto(state.currentUri)) actions.push(menuAction("Paste"') &&
    libraryMenu.includes('if (!menuActions.length)') &&
    !libraryApp.includes('menuAction("Undo"') &&
    !libraryApp.includes("Copy Runtime URI") &&
    !libraryApp.includes("Get Info") &&
    !/[⌘⌫⇧]/.test(library) &&
    !/(Apple|Finder|-apple|BlinkMac|SF Pro)/.test(library),
  "Library context menus must use clear file-manager labels without platform-branded shortcuts",
);
assert(
  libraryDialog.includes("properties-card") &&
    libraryDialog.includes("window-item-properties") &&
    libraryDialog.includes("item-props-tabview") &&
    libraryDialog.includes("item-props-tab-btn") &&
    libraryDialog.includes("item-props-tab-content") &&
    libraryDialog.includes("item-props-tbl") &&
    libraryDialog.includes('data-tab="general"') &&
    libraryDialog.includes('data-tab="technical"') &&
    libraryDialog.includes("propertiesPanel(\"general\"") &&
    libraryDialog.includes("propertiesPanel(\"technical\"") &&
    libraryDialog.includes("propertiesPanel(\"archive\"") &&
    libraryDialog.includes("smartWebIdentity") &&
    libraryDialog.includes("safeAvailabilitySummary") &&
    libraryDialog.includes("safePublishReceiptSummary") &&
    libraryDialog.includes("safeShareReceiptSummary") &&
    libraryDialog.includes("Published CID") &&
    libraryDialog.includes("Published Link") &&
    libraryDialog.includes("publishedCid(object)") &&
    libraryDialog.includes("propertiesVisibilitySummary") &&
    libraryDialog.includes("copyableValue(identity.contentId") &&
    libraryDialog.includes("data-prop-copy") &&
    libraryDialog.includes("props-copy-btn") &&
    libraryDialog.includes("item-prop-badge") &&
    libraryDialog.includes("Availability Summary") &&
    libraryDialog.includes("Publish Receipt Summary") &&
    libraryDialog.includes("Share Receipt Summary") &&
    !libraryDialog.includes("Availability Receipt") &&
    !libraryDialog.includes("Share Receipt</strong>") &&
    !libraryDialog.includes("item-props-tab-btn-versions") &&
    libraryDialog.includes("Resolver Target") &&
    libraryCss.includes(".window-item-properties") &&
    libraryCss.includes(".item-props-tab-content-selected") &&
    libraryCss.includes(".item-prop-label") &&
    libraryCss.includes(".item-prop-val") &&
    libraryCss.includes(".props-copy-btn") &&
    libraryCss.includes(".item-prop-badge") &&
    !libraryCss.includes(".properties-hero") &&
    !libraryCss.includes(".properties-icon"),
  "Library Properties must render a compact tabbed property table instead of a wide diagnostic metadata grid",
);
assert(
  objectProviderImpl.includes("published_cid: Option<String>") &&
    objectProviderImpl.includes("fn raw_sha256_cid(") &&
    objectProviderImpl.includes("\"current_cid\"") &&
    libraryApp.includes("publishedCid(object)") &&
    libraryApp.includes("Copy Published Link") &&
    libraryApp.includes("Copy Content CID") &&
    libraryActions.includes("publishedCid(object)") &&
    libraryMenuSmoke.includes("SMOKE_LOCAL_CONTENT_CID") &&
    libraryMenuSmoke.includes("SMOKE_PUBLISHED_CID"),
  "Library object model must separate current file-byte content_cid from published_cid/public elastos:// links",
);
assert(
  namespacesDoc.includes("Library's user-facing `Public` place") &&
    namespacesDoc.includes("placement is separate from published content identity") &&
    namespacesDoc.includes("`content_cid`") &&
    namespacesDoc.includes("`published_cid`") &&
    namespacesDoc.includes("`elastos://<cid>` receipt") &&
    namespacesDoc.includes("placing an object in `Public`") &&
    namespacesDoc.includes("silently publish it"),
  "Docs must explain that local files are provider-owned mutable storage with CIDs, while published_cid is the public SmartWeb availability identity",
);
assert(
  libraryApi.includes("/api/provider/object/upload") &&
    libraryApi.includes("/api/provider/object/upload/start") &&
    libraryApi.includes("/api/provider/object/upload/${encodeURIComponent(uploadId)}/chunk") &&
    libraryApi.includes("CHUNKED_UPLOAD_THRESHOLD_BYTES") &&
    libraryApi.includes("CHUNKED_UPLOAD_BYTES") &&
    libraryApi.includes("uploadFailureMessage") &&
    libraryApi.includes("This file is too large for the current upload service.") &&
    !libraryApi.includes("/api/provider/library/upload"),
  "Library upload must use object-provider upload only, use chunk sessions for large files, and explain edge-proxy 413 body-size failures",
);
assert(
  library.includes("elements.statusText.classList.toggle(\"hidden\", !message)") &&
    libraryMenuSmoke.includes("too large for the current upload service"),
  "Library status messages, including upload body-size failures, must be visible to users",
);
assert(
  libraryMenuSmoke.includes("/api/provider/object/") &&
    libraryMenuSmoke.includes("/api/provider/object/upload/start") &&
    libraryMenuSmoke.includes("LargeVideo.mp4") &&
    libraryMenuSmoke.includes("http-chunk-session") &&
    !libraryMenuSmoke.includes("/api/provider/library/"),
  "Library menu smoke must exercise canonical object-provider routes without retired library-provider fallback paths",
);
assert(
  !library.includes("moveObjectWithPrompt") &&
    !library.includes("copyObjectWithPrompt") &&
    !library.includes("window.prompt") &&
    !library.includes("window.confirm") &&
    !library.includes("Restore to URI") &&
    !library.includes("repairSelectedObjects") &&
    !library.includes("Repair availability"),
  "Library must not ship browser-native prompts/confirms, raw create/move/copy/restore prompts, or repair-only implementation leftovers",
);
assert(
  !libraryIndex.includes('id="selection-actions"') &&
    !library.includes("renderSelectionActions") &&
    !library.includes("data-selection-action"),
  "Library must keep actions in the file right-click menu, not a disruptive selection strip",
);
assert(
  libraryApp.includes("function activeRootForUri(uri)") &&
    libraryApp.includes(".sort((left, right) => right.uri.length - left.uri.length)") &&
    libraryApp.includes('sidebar: document.querySelector(".sidebar")') &&
    libraryApp.includes("function orderRoots(roots)") &&
    libraryApp.includes("function reorderPlace(sourceRootId, targetRootId") &&
    libraryApp.includes("localStorage.setItem(\"library.sidebarOrder\"") &&
    libraryState.includes("sidebarOrder: readStoredStringArray(storage.getItem(\"library.sidebarOrder\"))") &&
    libraryApp.includes("function showPlaceMenu(uri, x, y)") &&
    libraryApp.includes('menuAction("Open in New Window", () => openTarget("library", { uri: root.uri }))') &&
    libraryEvents.includes('elements.sidebar?.addEventListener("contextmenu"') &&
    libraryEvents.includes('elements.places.addEventListener("contextmenu"') &&
    libraryEvents.includes('application/x-elastos-library-root-id') &&
    libraryEvents.includes("markPlaceDropTarget(elements, button, event)") &&
    libraryEvents.includes("showPlaceMenu(button.dataset.uri, event.clientX, event.clientY)") &&
    libraryEvents.includes("event.stopPropagation();\n    hideMenu();") &&
    libraryCss.includes(".place.window-sidebar-item-dragging") &&
    libraryCss.includes(".place[data-drop-position=\"before\"]"),
  "Library sidebar must suppress browser right-click on chrome, expose Open/Open in New Window on place items, persist user root ordering, and mark only the most specific active place",
);
assert(
  libraryRender.includes('const badgesMarkup = badges ? `<span class="badges">${badges}</span>` : "";') &&
    libraryCss.includes("grid-template-rows: 45px auto 18px;") &&
    libraryCss.includes('.content[data-view="grid"] .badges') &&
    libraryCss.includes('.content[data-view="list"] .badges') &&
    !libraryCss.includes(".badges {\n      position: absolute") &&
    !libraryCss.includes(".content[data-view=\"list\"] .badges {\n      display: none;"),
  "Library Published/blocked/trash badges must be layout participants and remain visible in list view instead of overlaying icons or disappearing",
);
assert(
    libraryRender.includes('elements.content.dataset.empty = "true"') &&
    libraryRender.includes('class="empty-inner"') &&
    libraryCss.includes('.content[data-empty="true"]') &&
    libraryRender.includes("This space is empty") &&
    libraryRender.includes("Add files or folders to this space.") &&
    libraryRender.includes("This folder is empty") &&
    !libraryRender.includes("No connected spaces") &&
    libraryActions.includes("This Space is read-only.") &&
    !libraryActions.includes("Mounted WebSpaces are read-only resolver handles."),
  "Library empty Spaces states must be centered and explicit instead of rendering a cramped generic folder message",
);
assert(
  ['"refresh"', '"cache"', '"sync"'].every((op) => webspaceProvider.includes(op)) &&
    webspaceProvider.includes("Refresh {") &&
    webspaceProvider.includes("Cache {") &&
    webspaceProvider.includes("Sync {") &&
    webspaceProvider.includes("fn refresh_handle(") &&
    webspaceProvider.includes("fn cache_handle(") &&
    webspaceProvider.includes("fn sync_handle(") &&
    webspaceProvider.includes("elastos.webspace.refresh-receipt/v1") &&
    webspaceProvider.includes("elastos.webspace.cache-receipt/v1") &&
    webspaceProvider.includes("elastos.webspace.sync-receipt/v1") &&
    webspaceProvider.includes("refresh_replaces_index_and_persists_refreshed_head") &&
    webspaceProvider.includes("sync_clears_dirty_fork_head_without_claiming_byte_sync") &&
    webspaceCmd.includes("async fn refresh(") &&
    webspaceCmd.includes("async fn cache(") &&
    webspaceCmd.includes("async fn sync(") &&
    serverMain.includes("Refresh {") &&
    serverMain.includes("Cache {") &&
    serverMain.includes("Sync {"),
  "WebSpace provider must expose real provider-owned refresh/cache/sync lifecycle operations and CLI verbs instead of only static head/status metadata",
);
assert(
  webspaceProvider.includes('"health"') &&
    webspaceProvider.includes("Health {") &&
    webspaceProvider.includes("fn health_report(") &&
    webspaceProvider.includes("fn health_for_handle(") &&
    webspaceProvider.includes("elastos.webspace.health/v1") &&
    webspaceProvider.includes("elastos.webspace.resolver-health/v1") &&
    webspaceProvider.includes(
      "health_reports_external_resolver_attention_and_metadata_readiness",
    ) &&
    webspaceProvider.includes("dirty_head_count") &&
    webspaceCmd.includes("async fn health(") &&
    webspaceCmd.includes("WebSpace health:") &&
    serverMain.includes("Health {"),
  "WebSpace provider must expose resolver health as a provider/CLI contract with metadata-ready, mounted-no-index, and dirty-head coverage",
);
assert(
  ['"write"', '"mkdir"', '"delete"'].every((op) => webspaceProvider.includes(op)) &&
    webspaceProvider.includes("OBJECT_TABLE_SCHEMA") &&
    webspaceProvider.includes("struct WebSpaceObject") &&
    webspaceProvider.includes("fn write_handle(") &&
    webspaceProvider.includes("fn mkdir_handle(") &&
    webspaceProvider.includes("fn delete_handle(") &&
    webspaceProvider.includes("materialized_object_handle") &&
    webspaceProvider.includes("elastos.webspace.write-receipt/v1") &&
    webspaceProvider.includes("elastos.webspace.mkdir-receipt/v1") &&
    webspaceProvider.includes("elastos.webspace.delete-receipt/v1") &&
    webspaceProvider.includes("DEFAULT_MUTABLE_ACCESS_POLICY") &&
    webspaceProvider.includes("mutable_mount_materializes_objects_and_persists_them") &&
    objectProviderImpl.includes("async fn webspace_write_bytes(") &&
    objectProviderImpl.includes("async fn webspace_mkdir(") &&
    objectProviderImpl.includes("async fn webspace_delete_permanently(") &&
    objectProviderImpl.includes("handle_library_upload_bytes_runtime") &&
    gatewayProviderProxy.includes("handle_library_upload_bytes_runtime") &&
    libraryGatewayTests.includes(
      "test_library_gateway_mutates_writable_webspace_through_runtime_provider",
    ),
  "WebSpace provider and Library gateway must support local materialized mutable WebSpace objects without exposing raw resolver authority",
);
assert(
  providerRegistry.includes("fn apply_provider_byte_range(") &&
    providerRegistry.includes("fn apply_provider_stream_response(") &&
    providerRegistry.includes("fn decode_provider_stream_payload(") &&
    providerRegistry.includes('const PROVIDER_STREAM_SCHEMA: &str = "elastos.provider.stream/v1"') &&
    providerRegistry.includes('const PROVIDER_STREAM_ENCODING: &str = "base64-chunks"') &&
    providerRegistry.includes("fn attach_provider_invocation_envelope(") &&
    providerRegistry.includes("elastos.provider.invocation/v1") &&
    providerRegistry.includes("runtime-local-provider-plane") &&
    providerRegistry.includes("carrier-provider-plane") &&
    providerRegistry.includes("struct ProviderCarrierRoute") &&
    providerRegistry.includes("enum ProviderInvocationTransport") &&
    providerRegistry.includes("trait ProviderCarrierInvoker") &&
    providerRegistry.includes("set_carrier_invoker") &&
    providerRegistry.includes("provider_carrier_route_receipt") &&
    providerRegistry.includes("route.peer_did.as_deref()") &&
    providerRegistry.includes(
      "Carrier provider invocation requires registered Carrier invoker",
    ) &&
    providerRegistry.includes("fn provider_invocation_capability(") &&
    providerRegistry.includes("provider byte range requires response data.data base64 payload") &&
    providerRegistry.includes("provider stream expected {expected} bytes") &&
    providerRegistry.includes("provider stream chunk index mismatch") &&
    providerRegistry.includes("base64::engine::general_purpose::STANDARD") &&
    providerRegistry.includes("provider byte range expected {expected} bytes") &&
    providerRegistry.includes("provider invocation request must not predeclare runtime field") &&
    providerRegistry.includes('response["data"]["runtime_invocation"]["schema"]') &&
    providerRegistry.includes('response["_runtime_transfer"]["capability"]') &&
    providerRegistry.includes('response["_runtime_transfer"]["transport"]') &&
    providerRegistry.includes("test_provider_invocation_attaches_range_progress_transfer_receipt") &&
    providerRegistry.includes('assert_eq!(sliced, b"abcdefghij");') &&
    providerRegistry.includes(
      "test_provider_invocation_stream_normalizes_range_progress_transfer_receipt",
    ) &&
    providerRegistry.includes("test_provider_invocation_rejects_malformed_stream_payload") &&
    providerRegistry.includes("test_provider_invocation_rejects_predeclared_runtime_metadata") &&
    providerRegistry.includes("test_provider_invocation_carrier_routes_through_registered_invoker") &&
    providerRegistry.includes("test_provider_invocation_carrier_requires_registered_invoker") &&
    gatewayProviderProxy.includes("provider_proxy_runtime_metadata_field") &&
    gatewayProviderProxy.includes('key.starts_with("_runtime")') &&
    gatewayProviderProxy.includes("provider request must not predeclare Runtime metadata field") &&
    libraryGatewayTests.includes("test_gateway_provider_proxy_rejects_predeclared_runtime_metadata"),
  "Runtime provider invocation and browser-facing provider proxy must send typed source/target/capability envelopes to target providers, enforce byte-range and stream receipts, expose Carrier provider-plane transport, reject malformed provider stream payloads, and reject capsule-supplied Runtime metadata",
);
assert(
  inspectorCore.includes('pub const INSPECT_SYSTEM: &str = "elastos://inspect/*"') &&
    inspectorCore.includes('pub const INSPECT_SELF: &str = "elastos://inspect/self"') &&
    inspectorCore.includes("authorize_view") &&
    inspectorCore.includes("self_only_scope_cannot_cross_capsule_boundary") &&
    invokeCore.includes("plan_provider_operation") &&
    invokeCore.includes("reflect the capability/audit gate before any dispatch") &&
    invokeCore.includes("provider_operation_plan_reflects_authority_union") &&
    providerResource.includes('"inspect" => inspect_resource(op)') &&
    providerResource.includes("inspect_op_required_action") &&
    providerResource.includes("inspect_resource_and_actions_are_canonical") &&
    providerRegistry.includes('"inspect"') &&
    providerRegistry.includes("sub_provider_schemes") &&
    serverInfra.includes("InspectProvider::with_registry") &&
    serverInfra.includes('register_sub_provider("inspect"') &&
    gatewayProviderProxy.includes('"inspect" => match op.as_str()') &&
    gatewayProviderProxy.includes('"capsules" | "capsule" | "self" | "plan" | "request_act" => &[SYSTEM_CAPSULE_ID]') &&
    gatewayProviderProxy.includes('if scheme == "inspect" && op == "request_act"') &&
    !gatewayProviderProxy.includes('"dispatch_approved" => &[SYSTEM_CAPSULE_ID]') &&
    !gatewayProviderProxy.includes('"revoke" => &[SYSTEM_CAPSULE_ID]') &&
    inspectorProvider.includes("elastos.inspect.gate-preview/v1") &&
    inspectorProvider.includes("elastos.inspect.dispatch-result/v1") &&
    inspectorProvider.includes("elastos.inspect.execution-policy/v1") &&
    inspectorProvider.includes("preview_execution_policy") &&
    inspectorProvider.includes("approved_execution_policy") &&
    inspectorProvider.includes('"mode": "approved_dispatch"') &&
    inspectorProvider.includes("ProviderInvocationTransport::Local") &&
    inspectorProvider.includes('"can_dispatch": false') &&
    inspectorProvider.includes('"can_mutate": false') &&
    inspectorProvider.includes('"dispatch": false') &&
    gatewayInspectActions.includes("elastos.inspect.action-request/v1") &&
    gatewayInspectActions.includes("append_inspect_action_notifications") &&
    gatewayInspectActions.includes("inspect_action_gate_summary") &&
    gatewayInspectActions.includes("Gate preview:") &&
    gatewayInspectActions.includes("approve_inspect_action_request") &&
    gatewayInspectActions.includes("deny_inspect_action_request") &&
    gatewayInspectActions.includes('"op": "dispatch_approved"') &&
    gatewayInboxApi.includes("inspect-approve-request:") &&
    gatewayInboxApi.includes("inspect-deny-request:") &&
    gatewayTests.includes("inspect_action_requires_inbox_approval_before_dispatch") &&
    gatewayTests.includes("system_approval_attempt") &&
    gatewayTests.includes("StatusCode::FORBIDDEN") &&
    gatewayTests.includes("Gate preview: Capability elastos://exit/*: read") &&
    gatewayTests.includes("fresh passkey verification is required") &&
    gatewayTests.includes("inspect_action_can_be_denied_without_dispatch") &&
    capsuleInspectorDocs.includes("concise gate-preview summary") &&
    capsuleInspectorDocs.includes("System launch token can") &&
    capsuleInspectorDocs.includes("cannot call the Inbox action endpoint") &&
    capsuleInspectorDocs.includes("fresh same-principal passkey Home token") &&
    capsuleInspectorDocs.includes("Current product routing keeps `/api/provider/inspect/self` System-only") &&
    inspectorTestingDocs.includes("pure SelfOnly can view only self") &&
    inspectorTestingDocs.includes("Do not treat the pure SelfOnly test as proof") &&
    inspectorProvider.includes("signature_fingerprint") &&
    inspectorProvider.includes("signature_present") &&
    !inspectorProvider.includes('"signed_by": manifest') &&
    inspectorProvider.includes("inspect revoke is not implemented") &&
    inspectorProvider.includes("plan_reflects_provider_authority_without_dispatch") &&
    inspectorProvider.includes("projection_redacts_raw_signature_but_keeps_fingerprint"),
  "Capsule Inspector must remain a fail-closed Runtime mirror with System/Self inspect scopes, metadata-only gate preview, redacted provenance, no revoke dispatch, and ProviderRegistry registration",
);
assert(
  carrierRuntime.includes('"provider_invoke"') &&
    carrierRuntime.includes("MAX_CARRIER_REPLICATION_CANDIDATES") &&
    carrierRuntime.includes("MAX_CARRIER_AVAILABILITY_TICKET_LEN") &&
    carrierRuntime.includes("MAX_CARRIER_AVAILABILITY_ENDPOINT_ID_LEN") &&
    carrierRuntime.includes("struct CarrierAvailabilityRequirements") &&
    carrierRuntime.includes("struct CarrierReplicationProof") &&
    carrierRuntime.includes("remote_receipt: Option<serde_json::Value>") &&
    carrierRuntime.includes("carrier_provider_invoke_registry") &&
    carrierRuntime.includes("validate_carrier_provider_invocation") &&
    carrierRuntime.includes("decode_carrier_provider_stream_payload") &&
    carrierRuntime.includes("remote_content_provider_response_bytes") &&
    carrierRuntime.includes("remote_content_receipt_peer_selection_summary") &&
    carrierRuntime.includes("remote_content_receipt_peer_selection_replicas_summary") &&
    carrierRuntime.includes("remote_content_receipt_accounting_summary") &&
    carrierRuntime.includes("remote_content_receipt_abuse_controls_summary") &&
    carrierRuntime.includes("carrier_provider_target_allowed") &&
    carrierRuntime.includes("fetch_content_via_carrier_provider_invocation") &&
    carrierRuntime.includes("ensure_content_via_carrier_provider_invocation") &&
    carrierRuntime.includes("import_content_via_carrier_provider_invocation") &&
    carrierRuntime.includes("import_object_content_via_carrier_provider_invocation") &&
    carrierRuntime.includes("import_exact_content_via_carrier_provider_invocation") &&
    carrierRuntime.includes("MAX_CARRIER_OBJECT_IMPORT_FILES") &&
    carrierRuntime.includes("MAX_CARRIER_OBJECT_IMPORT_BYTES") &&
    carrierRuntime.includes("remote_content_receipt_summary") &&
    carrierRuntime.includes("content_availability_replicas") &&
    carrierRuntime.includes("carrier_replica_candidate_score") &&
    carrierRuntime.includes("CarrierPeerReputation") &&
    carrierRuntime.includes("CarrierPeerReputationStore") &&
    carrierRuntime.includes("carrier_reputation_score") &&
    carrierRuntime.includes("carrier-peer-reputation.json") &&
    carrierRuntime.includes("with_provider_registry_and_data_dir") &&
    carrierRuntime.includes("save_carrier_peer_reputation") &&
    carrierRuntime.includes("content_availability_replicas_with_reputation") &&
    carrierRuntime.includes('"selection_reason"') &&
    carrierRuntime.includes('"local_reputation"') &&
    carrierRuntime.includes('"replica_summary_limit"') &&
    carrierRuntime.includes('"replicas_truncated"') &&
    carrierRuntime.includes("carrier_peer_selection_json") &&
    carrierRuntime.includes("carrier_provider_quota") &&
    carrierRuntime.includes("carrier_abuse_controls_json") &&
    carrierRuntime.includes("carrier_remote_candidate_limit") &&
    carrierRuntime.includes('["remote_receipt"]["abuse_controls"]') &&
    carrierRuntime.includes('"requirements_exceed_quota"') &&
    carrierRuntime.includes('"effective_max_replicas"') &&
    carrierRuntime.includes('"carrier_provider_invocation_guardrail"') &&
    carrierRuntime.includes("test_carrier_quota_marks_impossible_replica_requirements") &&
    carrierRuntime.includes(
      "test_carrier_remote_candidate_limit_keeps_live_multi_peer_requirement",
    ) &&
    carrierRuntime.includes("carrier_provider_replication") &&
    carrierRuntime.includes('"remote_receipt"') &&
    carrierRuntime.includes('"storage_quota_status"') &&
    carrierRuntime.includes('"content_bytes"') &&
    carrierRuntime.includes('"local_only": true') &&
    carrierRuntime.includes('"transfer": "stream"') &&
    carrierRuntime.includes("ProviderTransfer::Stream") &&
    carrierRuntime.includes('"carrier_provider_invoke"') &&
    carrierRuntime.includes('"content" | "availability" | "rights" | "key" | "decrypt" | "drm"') &&
    carrierRuntime.includes("CarrierProviderInvoker") &&
    carrierRuntime.includes("ProviderCarrierInvoker for CarrierProviderInvoker") &&
    carrierRuntime.includes("invoke_provider(") &&
    carrierRuntime.includes("carrier_endpoint_matches_peer") &&
    carrierRuntime.includes("provider_invoke carrier metadata must not expose connect_ticket") &&
    carrierRuntime.includes("test_carrier_provider_invoke_dispatches_runtime_enveloped_request") &&
    carrierRuntime.includes("test_carrier_provider_invoke_accepts_stream_contract_metadata") &&
    carrierRuntime.includes("test_carrier_provider_invoke_rejects_stream_without_contract_metadata") &&
    carrierRuntime.includes("test_carrier_provider_invoke_rejects_raw_backend_target") &&
    carrierRuntime.includes("test_carrier_availability_fetch_uses_provider_invocation_transport") &&
    carrierRuntime.includes("test_carrier_replication_proof_uses_remote_content_provider_invocation") &&
    carrierRuntime.includes("test_carrier_replication_falls_back_to_exact_import_when_remote_pin_fails") &&
    carrierRuntime.includes("test_carrier_replication_prefers_object_import_when_manifest_exists") &&
    carrierRuntime.includes("test_carrier_availability_ensure_proves_remote_replica_via_provider_plane") &&
    carrierRuntime.includes(
      "test_carrier_availability_requires_remote_attempt_for_live_proof_when_min_met",
    ) &&
    carrierRuntime.includes("test_carrier_peer_selection_proof_redacts_connect_tickets") &&
    carrierRuntime.includes(
      "test_remote_content_receipt_peer_selection_summary_redacts_replica_rows",
    ) &&
    carrierRuntime.includes(
      "test_remote_content_receipt_peer_selection_summary_marks_truncated_rows",
    ) &&
    carrierRuntime.includes("test_content_availability_replicas_are_scored_and_sorted") &&
    carrierRuntime.includes("test_content_availability_replicas_apply_local_runtime_reputation") &&
    carrierRuntime.includes("test_carrier_peer_reputation_persists_local_history") &&
    carrierRuntime.includes("test_content_availability_replicas_ignore_signed_repair_only_announcements") &&
    carrierRuntime.includes("test_content_availability_replicas_ignore_oversized_candidate_metadata") &&
    carrierRuntime.includes('remote_transfer["transfer"], "stream"') &&
    serverInfra.includes("set_carrier_invoker") &&
    serverInfra.includes("CarrierAvailabilityProvider::with_provider_registry") &&
    serverInfra.includes("CarrierProviderInvoker::new()") &&
    serverInfra.includes("maybe_spawn_content_repair_scheduler") &&
    serverInfra.includes("ELASTOS_CONTENT_REPAIR_SCHEDULER") &&
    serverInfra.includes("invoke_content_repair_worker(") &&
    serverInfra.includes("content_repair_scheduler_config_clamps_operator_env") &&
    serverInfra.includes("content_repair_scheduler_is_opt_in"),
  "Carrier provider invocation must be Runtime-mediated over provider_invoke, service-provider-only, Stream-contract validated for Carrier availability fetch, registered only with the built-in Carrier node, prove remote replicas through remote content/ensure+status, enforce bounded peer-selection/quota metadata, ignore repair-only announcements as candidates, provide an opt-in bounded content repair scheduler, and must not leak raw connect tickets",
);
assert(
  contentProvider.includes("struct AvailabilityRequirements") &&
    contentProvider.includes("struct ContentRepairTask") &&
    contentProvider.includes("struct ContentFetchTransfer") &&
    contentProvider.includes("IMPORT_EXACT_MAX_BYTES") &&
    contentProvider.includes("IMPORT_OBJECT_MAX_FILES") &&
    contentProvider.includes("AVAILABILITY_DASHBOARD_SCHEMA") &&
    contentProvider.includes("CONTENT_ACCOUNTING_SCHEMA") &&
    contentProvider.includes("CONTENT_STORAGE_QUOTA_SCHEMA") &&
    contentProvider.includes("CONTENT_ABUSE_CONTROLS_SCHEMA") &&
    contentProvider.includes("REPAIR_TASK_SCHEMA") &&
    contentProvider.includes("REPAIR_WORKER_RUN_SCHEMA") &&
    contentProvider.includes("REPAIR_WORKER_ABUSE_CONTROLS_SCHEMA") &&
    contentProvider.includes("REPAIR_WORKER_DEFAULT_MAX_ATTEMPTS") &&
    contentProvider.includes('"repair_worker"') &&
    contentProvider.includes("fn record_repair_task(") &&
    contentProvider.includes("fn availability_dashboard(") &&
    contentProvider.includes("fn availability_quota_status(") &&
    contentProvider.includes("fn content_accounting_json(") &&
    contentProvider.includes("fn content_accounting_observation_from_publish_request(") &&
    contentProvider.includes("fn content_accounting_from_previous_or_unknown(") &&
    contentProvider.includes("fn local_abuse_controls_json(") &&
    contentProvider.includes("fn provider_abuse_controls_json(") &&
    contentProvider.includes('"verified_remote_receipts"') &&
    contentProvider.includes('"recent_remote_replicas"') &&
    contentProvider.includes('"recent_remote_replica_limit"') &&
    contentProvider.includes('"recent_remote_replicas_truncated"') &&
    contentProvider.includes('"local_reputation"') &&
    contentProvider.includes('"local_runtime"') &&
    contentProvider.includes('"replica_bytes_estimate"') &&
    contentProvider.includes('"storage_quota_policy"') &&
    contentProvider.includes('"abuse_controls"') &&
    contentProvider.includes('dashboard["data"]["proofs"]["live_multi_peer"]') &&
    contentProvider.includes('dashboard["data"]["quota"]["by_status"]["within_quota"]') &&
    contentProvider.includes("async fn run_repair_worker(") &&
    contentProvider.includes("fn validate_repair_worker_invocation(") &&
    contentProvider.includes("repair-tasks.jsonl") &&
    contentProvider.includes("fn provider_transfer_value(") &&
    contentProvider.includes("async fn import_exact(") &&
    contentProvider.includes("async fn import_object(") &&
    contentProvider.includes("fn validate_import_exact_invocation(") &&
    contentProvider.includes("fn validate_import_object_invocation(") &&
    contentProvider.includes("fn validate_import_object_payload_bounds(") &&
    contentProvider.includes("fn validate_runtime_invocation_fields(") &&
    contentProvider.includes("fn provider_stream_payload_bytes(") &&
    contentProvider.includes("fn validate_network_availability_claim(") &&
    contentProvider.includes("content_repair_worker_guardrail") &&
    contentProvider.includes("runtime_invocation_required") &&
    contentProvider.includes('"requirements": requirements.to_json()') &&
    contentProvider.includes('ProviderTransfer::Stream =>') &&
    contentProvider.includes('content fetch transfer must be bytes or stream') &&
    contentProvider.includes('fn provider_response_stream(') &&
    contentProvider.includes("content_fetch_propagates_range_progress_transfer_receipt") &&
    contentProvider.includes("content_fetch_stream_returns_provider_stream_payload") &&
    contentProvider.includes("content_fetch_ranges_availability_provider_when_local_backend_misses") &&
    contentProvider.includes(
      "content_fetch_stream_ranges_availability_provider_when_local_backend_misses",
    ) &&
    contentProvider.includes("content_fetch_local_only_skips_availability_provider") &&
    contentProvider.includes("multi-peer availability requires live_multi_peer_proof=true") &&
    contentProvider.includes("network availability peer_selection requires mode or strategy") &&
    contentProvider.includes("Carrier availability announcement requires a topic") &&
    contentProvider.includes("content_publish_rejects_unproven_multi_peer_availability_claim") &&
    contentProvider.includes("content_publish_requires_peer_selection_policy_metadata") &&
    contentProvider.includes("content_publish_records_local_only_repair_task") &&
    contentProvider.includes("content_import_exact_accepts_matching_cid_stream") &&
    contentProvider.includes("content_import_exact_rejects_cid_mismatch_and_unpins_import") &&
    contentProvider.includes("content_import_exact_requires_runtime_provider_invocation") &&
    contentProvider.includes("content_import_object_requires_runtime_provider_invocation") &&
    contentProvider.includes("content_import_object_reconstructs_manifest_directory") &&
    contentProvider.includes('"carrier_object_import"') &&
    contentProvider.includes('status["data"]["accounting"]["content_bytes"]') &&
    contentProvider.includes("content_repair_worker_requires_runtime_provider_invocation") &&
    contentProvider.includes("content_repair_worker_retries_queued_availability_task") &&
    contentProvider.includes("content_repair_worker_enforces_attempt_budget") &&
    contentProvider.includes("content_status_without_cid_returns_availability_dashboard") &&
    contentProvider.includes('dashboard["data"]["proofs"]["recent_remote_replicas"]') &&
    contentCmd.includes("#[command(name = \"repair-worker\")]") &&
    contentCmd.includes("fn repair_worker_request(") &&
    contentCmd.includes("fn content_command_builds_repair_worker_request(") &&
    contentCmd.includes("ProviderInvocationTransport::Local") &&
    contentCmd.includes("ProviderTransfer::Json") &&
    contentProvider.includes("content_publish_enforces_availability_requirements") &&
    availabilityProvider.includes("requirements: Value") &&
    availabilityProvider.includes("fn upstream_peer_selection(") &&
    availabilityProvider.includes("multi-replica availability target response requires peer_selection metadata") &&
    availabilityProvider.includes("upstream_multi_replica_network_available_requires_peer_selection"),
  "Content availability must enforce requested replica/quota/live-proof requirements before recording network/Carrier availability claims, persist repair task state, keep repair-worker passes Runtime-provider-internal with bounded guardrails, and propagate provider byte-range/progress receipts plus optional Stream payloads through fetch",
);
assert(
    objectProviderImpl.includes("fn shared_access_decision(") &&
    objectProviderImpl.includes("fn shared_access_open_contract(") &&
    objectProviderImpl.includes("fn validate_shared_access_recipient_proof(") &&
    objectProviderImpl.includes("fn shared_access_recipient_proof_state(") &&
    objectProviderImpl.includes("elastos.library.recipient-proof/v1") &&
    objectProviderImpl.includes("elastos.library.recipient-proof-state/v1") &&
    objectProviderImpl.includes("recipient_proof requires proof_binding_id") &&
    objectProviderImpl.includes("requires passkey proof binding") &&
    gatewayProviderProxy.includes('object.remove("recipient_proof")') &&
    gatewayProviderProxy.includes('"proof_binding_id": context.proof_binding_id.as_deref().unwrap_or_default()') &&
    gatewayProviderProxy.includes('"source": "runtime-launch-grant"') &&
    objectProviderImpl.includes("elastos.library.access-decision/v1") &&
    objectProviderImpl.includes("elastos.library.shared-open/v1") &&
    objectProviderImpl.includes("runtime-provider-fetch") &&
    objectProviderImpl.includes('"allowed": false') &&
    objectProviderImpl.includes('"reason": err.to_string()') &&
    libraryDialog.includes("Share Grants / Key Release") &&
    libraryDialog.includes("<strong>Recipients</strong>") &&
    libraryDialog.includes("<strong>Grants</strong>") &&
    libraryDialog.includes("contentSecurity?.published_payload") &&
    libraryDialog.includes('name="sharePolicy" value="encrypted_recipient" disabled') &&
    libraryDialog.includes("<strong>Key Release</strong>") &&
    libraryDialog.includes("<strong>Provider Chain</strong>") &&
    read("docs/PROTECTED_CONTENT.md").includes(
      "Visible protected-content UI may ship only as a disabled/read-only readiness",
    ) &&
    read("docs/RUNTIME_REPO_USER_STORY_CHECKLIST.md").includes(
      "Library protected-content rail is visible only as disabled/read-only readiness/status",
    ) &&
    libraryMenuSmoke.includes("Share Grants / Key Release") &&
    libraryMenuSmoke.includes("Recipients") &&
    libraryMenuSmoke.includes("not_required_for_plain_published_content") &&
    libraryGatewayTests.includes("ready_for_plain_content_fetch") &&
    libraryGatewayTests.includes("recipient_proof_verified") &&
    libraryGatewayTests.includes("authority.proof_binding_id") &&
    libraryGatewayTests.includes("requires Runtime recipient_proof") &&
    libraryGatewayTests.includes("not authorized"),
  "Library recipient sharing must expose explicit access decisions, Runtime recipient-proof state, open contracts, key-release state, and denied-access audit coverage",
);
assert(
  libraryApp.includes("function syncPlacesActive()") &&
    libraryRender.includes('loading="eager" decoding="async"') &&
    !library.includes("hydrateInlineIcons") &&
    !library.includes("loadIconMarkup") &&
    !/function renderAll\(\) \{\s*renderPlaces\(\);/.test(library),
  "Library must not rebuild the sidebar or run async icon hydration on every folder render",
);
assert(
  libraryNavigation.includes("LIBRARY_HISTORY_SCHEMA") &&
    libraryNavigation.includes("LIBRARY_HISTORY_GUARD_SCHEMA") &&
    libraryNavigation.includes("window.history.pushState") &&
    libraryNavigation.includes("window.history.replaceState") &&
    libraryEvents.includes('window.addEventListener("popstate"') &&
    libraryNavigation.includes("window.history.back()") &&
    libraryNavigation.includes("window.history.forward()"),
  "Library must take over browser Back/Forward for Library navigation instead of letting native history leave the capsule",
);
assert(
  libraryRender.includes("document.createDocumentFragment()") &&
    libraryRender.includes("elements.content.replaceChildren(fragment)"),
  "Library content pane must redraw with one DOM replacement to avoid sluggish view changes",
);
assert(
  libraryCss.includes("--explorer-list-columns: 30px minmax(260px, 1fr) 170px 86px 180px;") &&
    libraryCss.includes(".content[data-view=\"list\"] .item-date {\n      grid-column: 3;") &&
    libraryCss.includes(".explore-table-headers-th--size {\n      grid-column: 4;\n      padding: 0 10px;\n      text-align: right;") &&
    libraryCss.includes(".explore-table-headers-th--type {\n      grid-column: 5;"),
  "Library list rows and headers must share one file-list column grid",
);
assert(
  libraryCss.includes("max-height: calc(100vh - 16px);") &&
    libraryMenu.includes("event.stopPropagation();") &&
    libraryMenu.includes("contextMenu.style.visibility = \"hidden\"") &&
    libraryMenu.includes("function normalizedMenuActions(actions)") &&
    libraryMenu.includes("divider.setAttribute(\"role\", \"separator\")"),
  "Library context menu must use one viewport-aware renderer whose action buttons do not get swallowed by document click handling",
);
assert(
  libraryMenuSmoke.includes("PASS Library menu smoke") &&
    libraryMenuSmoke.includes("openBackgroundMenu") &&
    libraryMenuSmoke.includes("openItemMenu") &&
    libraryMenuSmoke.includes("Spaces background menu") &&
    libraryMenuSmoke.includes('label: "Spaces"') &&
    !libraryMenuSmoke.includes('label: "WebSpaces"') &&
    libraryMenuSmoke.includes("right-click must not open the Library context menu") &&
    libraryMenuSmoke.includes("right-click must cancel the browser context menu") &&
    libraryMenuSmoke.includes("sidebar place menu") &&
    libraryMenuSmoke.includes("Open in New Window") &&
    libraryMenuSmoke.includes("sidebar must mark only") &&
    libraryMenuSmoke.includes("empty state must be centered horizontally") &&
    libraryMenuSmoke.includes("Published badge must be visible in ${view} view") &&
    libraryMenuSmoke.includes("New Folder must use inline naming, not window.prompt") &&
    libraryMenuSmoke.includes("window.history.back()") &&
    libraryMenuSmoke.includes("window.history.forward()") &&
    libraryMenuSmoke.includes("Upload must use the raw Library upload transport") &&
    libraryMenuSmoke.includes("Drop upload must use the raw Library upload transport") &&
    libraryMenuSmoke.includes("F2 rename must call rename") &&
    libraryMenuSmoke.includes("Selected-name click must not start rename; rename is explicit through context menu or F2") &&
    libraryMenuSmoke.includes("Folder Download must use the raw Library download transport") &&
    libraryMenuSmoke.includes("Download must use the raw Library download transport") &&
    libraryMenuSmoke.includes("Compress to ZIP must call compress_archive") &&
    libraryMenuSmoke.includes("Compress Selected to ZIP must call compress_archive") &&
    libraryMenuSmoke.includes("mounted WebSpace menu") &&
    libraryMenuSmoke.includes("indexed WebSpace file menu") &&
    libraryMenuSmoke.includes("/WebSpaces/Cloud/Drive/Project X/file.pdf") &&
    libraryMenuSmoke.includes("Indexed WebSpace Download must use the raw Library download transport") &&
    libraryMenuSmoke.includes("Elastos content WebSpace file menu") &&
    libraryMenuSmoke.includes("Status must call status") &&
    libraryMenuSmoke.includes("Repair must call repair") &&
    libraryMenuSmoke.includes("Share must copy the published elastos:// link") &&
    libraryMenuSmoke.includes("Double-clicking a selected folder name must open it instead of starting rename") &&
    libraryMenuSmoke.includes("List-view double-clicking a selected folder name must open it instead of starting rename") &&
    libraryMenuSmoke.includes("List-view double-clicking a selected file name must open it without later starting rename") &&
    libraryMenuSmoke.includes("List-view double-clicking the active rename editor must not also open/read the file") &&
    libraryMenuSmoke.includes("Shift-click must select a visible range in list view") &&
    libraryMenuSmoke.includes("Enter on a selected file must open/read it") &&
    libraryMenuSmoke.includes("Shift-F10 selected file menu") &&
    libraryMenuSmoke.includes("Enter on multiple selected files must open every selected item") &&
    libraryMenuSmoke.includes("Double-clicking a selected file name must open it instead of starting rename") &&
    libraryMenuSmoke.includes("Double-click preview without an installed viewer must call read") &&
    libraryMenuSmoke.includes("Copy/Paste Into Folder must call copy") &&
    libraryMenuSmoke.includes("Drag/drop move must call move") &&
    libraryMenuSmoke.includes("Alt-drag/drop copy must call copy") &&
    libraryMenuSmoke.includes("Publish must call publish") &&
    libraryMenuSmoke.includes("Delete Permanently must use in-app confirmation, not window.confirm") &&
    libraryMenuSmoke.includes("Delete must move the object to Trash") &&
    libraryMenuSmoke.includes("Trash sidebar place menu"),
  "Library must have an operator smoke for context-menu reliability, WebSpaces read-only behavior, and core object journeys",
);
assert(
  !libraryEvents.includes("NAME_CLICK_RENAME_DELAY_MS") &&
    !libraryEvents.includes("cancelPendingNameClickRename") &&
    !libraryEvents.includes("clickedName") &&
    !sourceBlock(
      libraryEvents,
      'elements.content.addEventListener("click"',
      "Library click handler",
    ).includes("startRename(") &&
    libraryEvents.includes("function isNameEditorTarget(target)") &&
    sourceBlock(
      libraryEvents,
      'elements.content.addEventListener("dblclick"',
      "Library double-click handler",
    ).includes("isNameEditorTarget(event.target)") &&
    libraryEvents.includes("selectRangeTo(item.dataset.uri") &&
    libraryEvents.includes('event.key === "Enter"') &&
    libraryEvents.includes("openSelectedObjects(objects, openObject, showError)") &&
    libraryEvents.includes("async function openSelectedObjects(objects, openObject, showError)") &&
    libraryEvents.includes('event.key === "ContextMenu"') &&
    libraryEvents.includes('event.shiftKey && event.key === "F10"') &&
    libraryEvents.includes("isMenuOpen(elements)") &&
    libraryEvents.includes('if (event.key === "F2" && !editable)') &&
    libraryApp.includes('menuAction("Rename", () => startRename(object))'),
  "Library rename must be explicit through context menu/F2; content clicks must never start rename, and range selection/keyboard open/context menu must stay wired",
);
assert(
  libraryLiveSmoke.includes("verify_public_library_assets") &&
    libraryLiveSmoke.includes("/apps/library/src/uploads.js") &&
    libraryLiveSmoke.includes("/apps/library/src/api.js") &&
    libraryLiveSmoke.includes("/api/provider/object/upload") &&
    libraryLiveSmoke.includes("/api/provider/object/upload/start") &&
    libraryLiveSmoke.includes("CHUNKED_UPLOAD_THRESHOLD_BYTES") &&
    libraryLiveSmoke.includes("http-chunk-session") &&
    libraryLiveSmoke.includes("/api/provider/object/download/raw") &&
    libraryLiveSmoke.includes("raw download readback mismatch") &&
	    libraryLiveSmoke.includes("elastos.object.transfer.receipt/v1") &&
	    libraryLiveSmoke.includes("function isNameEditorTarget(target)") &&
	    libraryLiveSmoke.includes("NAME_CLICK_RENAME_DELAY_MS") &&
	    libraryLiveSmoke.includes("clickedName") &&
	    libraryLiveSmoke.includes("cancelPendingNameClickRename") &&
    libraryLiveSmoke.includes("selectRangeTo(item.dataset.uri") &&
    libraryLiveSmoke.includes('event.key === "Enter"') &&
    libraryLiveSmoke.includes("openSelectedObjects(objects, openObject, showError)") &&
    libraryLiveSmoke.includes('event.key === "ContextMenu"') &&
    libraryLiveSmoke.includes('event.shiftKey && event.key === "F10"') &&
	    libraryLiveSmoke.includes("signed provider path skipped") &&
    libraryLiveSmoke.includes("PASS Library live publish/share smoke"),
  "Library live smoke must verify public static assets without a signed session and provider publish/share with a signed Home session",
);
assert(
  libraryIndex.includes('rel="stylesheet" href="library.css"') &&
    libraryIndex.includes('type="module" src="src/app.js?v=') &&
    !libraryIndex.includes("<style>") &&
    !libraryIndex.includes("function renderContent"),
  "Library index must stay a static shell with CSS and app code split out",
);
assert(
  libraryApp.includes('from "./model.js"') &&
    libraryApp.includes('from "./actions.js') &&
    libraryApp.includes('from "./api.js') &&
    libraryApp.includes('from "./dialog.js') &&
    libraryApp.includes('from "./editor.js"') &&
    libraryApp.includes('from "./events.js"') &&
    libraryApp.includes('from "./menu.js"') &&
    libraryApp.includes('from "./navigation.js"') &&
    libraryApp.includes('from "./preview.js"') &&
    libraryApp.includes('from "./realtime.js"') &&
    libraryApp.includes('from "./render.js') &&
    libraryApp.includes('from "./selection.js"') &&
    libraryApp.includes('from "./state.js"') &&
    libraryApp.includes('from "./uploads.js"') &&
    !libraryApp.includes("function renderContent(") &&
    !libraryApp.includes("function uploadFiles(") &&
    !libraryApp.includes("function handleLibraryEventsPayload(") &&
    libraryActions.includes("createLibraryActions") &&
    libraryActions.includes("function openObject(") &&
    libraryActions.includes("function uploadFiles(") &&
    libraryActions.includes("function publishObject(") &&
    libraryActions.includes("uploadObject({") &&
    libraryActions.includes("downloadObjectRaw({") &&
    !libraryActions.includes("fileToBase64") &&
    libraryApi.includes("createLibraryRuntime") &&
    libraryApi.includes("/api/provider/object/upload") &&
    libraryApi.includes("/api/provider/object/download/raw") &&
    libraryApi.includes("x-elastos-transfer-receipt") &&
    libraryApi.includes("XMLHttpRequest") &&
    libraryDialog.includes("createLibraryDialog") &&
    libraryEditor.includes("createLibraryEditor") &&
    libraryEditor.includes("function startCreateObject(") &&
    libraryEvents.includes("bindLibraryEvents") &&
    libraryEvents.includes("elements.content.addEventListener(") &&
    libraryEvents.includes("elements.places.addEventListener(") &&
    libraryMenu.includes("createLibraryMenu") &&
    libraryNavigation.includes("createLibraryNavigation") &&
    libraryPreview.includes("createLibraryPreview") &&
    libraryRealtime.includes("createLibraryRealtime") &&
    libraryRealtime.includes("new EventSource(") &&
    libraryRealtime.includes("function handleLibraryEventsPayload(") &&
    libraryRender.includes("createLibraryRenderer") &&
    libraryRender.includes("function renderContent(") &&
    libraryModel.includes("export function iconFor") &&
    librarySelection.includes("createLibrarySelection") &&
    libraryState.includes("createLibraryState") &&
    libraryState.includes("cacheFolderListing") &&
    libraryState.includes("MUTATING_PROVIDER_OPS") &&
    libraryUploads.includes("createLibraryUploads") &&
    libraryUploads.includes("function scheduleUploadRender(") &&
    libraryUploads.includes("window.requestAnimationFrame("),
  "Library app must keep pure model helpers, Runtime provider/frame calls, action control, dialog control, inline editor control, UI event binding, menu rendering, navigation/history, preview control, realtime refresh, content rendering, selection state, state/cache ownership, and upload progress split into dedicated modules",
);
assert(
  libraryPerformanceSmoke.includes("PASS Library performance smoke") &&
    libraryPerformanceSmoke.includes("renderPlacesCount === 1") &&
    libraryPerformanceSmoke.includes("iconFetchCount === 0") &&
    libraryPerformanceSmoke.includes("lastContentRender?.objectCount === 1000") &&
    libraryPerformanceSmoke.includes("lastContentRender?.chunked === true") &&
    libraryPerformanceSmoke.includes("Upload progress must render through a scheduled frame") &&
    libraryPerformanceSmoke.includes("Upload progress rendered too often"),
  "Library must have a performance smoke for sidebar stability, icon hydration removal, bounded render cost, and upload-progress render coalescing",
);
assert(
  !library.includes("entry-details"),
  "Library cards must not expose raw technical detail drawers",
);
assert(
  !library.includes("Working copy") && !library.includes("Published revision"),
  "Library cards must not show raw storage addresses by default",
);
assert(
  libraryCss.includes("min-height: 100dvh;"),
  "Library must use dynamic viewport height",
);
assert(
  libraryCss.includes(".toolbar {\n        display: grid;"),
  "Library toolbar must stack on narrow screens",
);
assert(
  libraryCss.includes("padding: 4px;") && libraryCss.includes("border-radius: 14px;"),
  "Library mobile panels must use compact Home-aligned spacing",
);
assert(
  chatStyle.includes("width: 100%;") &&
    chatStyle.includes("padding-top: 0.35rem;"),
  "Chat Room mobile shell must avoid nested browser gutters",
);
assert(
  chatStyle.includes("border-radius: 0.82rem;") &&
    chatStyle.includes("padding: 0.48rem;"),
  "Chat Room mobile cards must use compact Home-aligned spacing",
);
assert(
  gba.includes('aria-label="D-pad up, keyboard Arrow Up"'),
  "GBA directional controls must expose keyboard mapping labels",
);
assert(
  gba.includes('aria-label="Save state slot 1"'),
  "GBA save slots must expose slot-specific labels",
);
assert(
  gba.includes('aria-label="Load state slot 1"'),
  "GBA load slots must expose slot-specific labels",
);
assert(
  gba.includes("Choose a game from Library"),
  "GBA empty state must direct game selection through Library",
);
assert(
  gbaStyle.includes("--control-size: clamp(2.75rem, 13vw, 3.25rem);"),
  "GBA mobile d-pad buttons must stay touch-sized",
);
assert(
  gbaStyle.includes("grid-template-areas:") &&
    gbaStyle.includes('"left select start right"') &&
    gbaStyle.includes('"dpad dpad actions actions"'),
  "GBA mobile controls must place Select/Start in the L/R row",
);
assert(
  gbaStyle.includes(
    ".shoulder-buttons,\n  .controls-row {\n    display: contents;",
  ),
  "GBA mobile controls must let the full controller share one grid",
);
assert(
  gbaStyle.includes("#btn-select {\n    grid-area: select;") &&
    gbaStyle.includes("#btn-start {\n    grid-area: start;"),
  "GBA mobile Select/Start must be direct grid items in the shoulder row",
);
assert(
  gbaStyle.includes("grid-area: left;\n    width: 100%;") &&
    gbaStyle.includes("grid-area: right;\n    width: 100%;"),
  "GBA mobile L/R controls must be full shoulder targets, not content-width dots",
);
assert(
  gbaStyle.includes("#screen-container:focus"),
  "GBA screen focus must not show a browser outline",
);
assert(
  gbaStyle.includes("grid-template-rows: auto auto;"),
  "GBA mobile screen must not be starved by a flexible row",
);
assert(
  gbaStyle.includes("touch-action: none;"),
  "GBA virtual controls must own touch gestures",
);
assert(
  gba.includes('<aside class="utility-card">') &&
    gba.includes('id="utility-panel"') &&
    !gba.includes('id="utility-toggle"'),
  "GBA Options must remain directly available without a separate collapsed toggle",
);
assert(
  gbaStyle.includes("max-height: min(8.25rem, 22dvh);"),
  "GBA mobile expanded Options must stay compact",
);
assert(
  gbaStyle.includes("grid-template-columns: repeat(3, minmax(0, 1fr));"),
  "GBA mobile save slots must use one row",
);
assert(
  gbaStyle.includes(".shell {\n    width: 100%;\n    padding: 0.2rem;"),
  "GBA mobile shell must not waste viewport on outer gutters",
);
assert(
  gbaStyle.includes(
    ".screen-card {\n    grid-template-rows: auto auto;\n    align-content: start;\n    padding: 0.38rem;",
  ),
  "GBA mobile screen card must keep compact chrome",
);
assert(
  gbaJs.includes("touchPointers.set(event.pointerId, button)") &&
    gbaJs.includes("touchPointers.delete(event.pointerId)"),
  "GBA touch controls must track pointer-specific presses",
);
assert(
  gbaJs.includes("pointerdown") && gbaJs.includes("pointerup"),
  "GBA controls must use a single pointer-event input path",
);
assert(
  !gbaJs.includes("touchstart") && !gbaJs.includes("mousedown"),
  "GBA controls must not mix touch and mouse input handlers",
);
assert(
  !gbaJs.includes("syncUtilityDefaultForViewport") &&
    gbaStyle.includes("max-height: min(8.25rem, 22dvh);"),
  "GBA compact Options layout must remain CSS-driven",
);
assert(
  gbaJs.includes("assertPortableEngineSupport"),
  "GBA startup must preflight threaded WebAssembly support before mGBA init",
);
assert(
  gbaJs.includes("withTimeout(") &&
    gbaJs.includes("The GBA engine did not start."),
  "GBA startup must fail visibly instead of hanging during mGBA init",
);
assert(
  gbaJs.includes("SharedArrayBuffer"),
  "GBA startup must explicitly guard WebAssembly thread requirements",
);
assert(
  gbaJs.includes("This browser cannot run the GBA engine.") &&
    gbaJs.includes("This browser does not provide isolated WebAssembly threads."),
  "GBA unsupported-runtime copy must explain WebAssembly thread requirements",
);
assert(
  gbaJs.includes("Choose a GBA game from Library.") &&
    !gbaJs.includes("Choose an installed ROM"),
  "GBA runtime copy must direct game selection through Library",
);
assert(
  !system.includes("<dt>Overlay</dt>"),
  "System overlay controls must live inside the Background box",
);
assert(
  system.includes('data-settings="account"') &&
    system.includes('class="settings-content active"') &&
    system.includes('<h1 id="accounts-title" class="pc2-section-title">Accounts</h1>') &&
    system.includes("Local browser credentials for this Home account.") &&
    !system.includes('id="handle-form"') &&
    !system.includes("<dt>Display name</dt>"),
  "System Account tab must focus on accounts; display-name Profile belongs in People",
);
assert(
  system.includes(`./style.css?v=${systemAssetVersion}`) &&
    system.includes(`./system.js?v=${systemAssetVersion}`),
  "System browser assets must be cache-busted after UI changes",
);
assert(
  system.includes('data-settings="security"') &&
    system.includes('id="technical-details"') &&
    system.includes('id="technical-inspect-list"') &&
    system.includes('id="technical-inspect-detail"') &&
    system.includes('id="technical-inspect-refresh"') &&
    system.includes("Review component identity, permissions, verification, and approval requirements.") &&
    systemJs.includes("configureTechnicalDetails") &&
    systemJs.includes("refreshTechnicalDetails") &&
    systemJs.includes('inspectProvider("capsules"') &&
    systemJs.includes('inspectProvider("capsule"') &&
    systemJs.includes('inspectProvider("plan"') &&
    systemJs.includes('inspectProvider("request_act"') &&
    systemJs.includes('operation === "request_act"') &&
    systemJs.includes('response.schema === "elastos.inspect.action-request/v1"') &&
    systemJs.includes('response.status === "pending"') &&
    systemJs.includes("Request approval") &&
    systemJs.includes("/api/provider/inspect/") &&
    !systemJs.includes('inspectProvider("dispatch_approved"') &&
    !systemJs.includes('inspectProvider("revoke"') &&
    !systemJs.includes("/api/provider/inspect/revoke") &&
    systemStyle.includes(".technical-inspect-grid") &&
    systemStyle.includes(".technical-inspect-detail"),
  "System Inspector must expose the Runtime inspect mirror as a System-only read/preview UI that can request Inbox approval without direct revoke or dispatch affordances",
);
assert(
  !system.includes("wallet-create") && !system.includes("Approval requests"),
  "System Advanced must not duplicate Wallet accounts or Wallet/Inbox approval review",
);
const systemBootBlock = sourceBlock(
  systemJs,
  "async function boot()",
  "System boot",
);
const systemPasskeyConfigBlock = sourceBlock(
  systemJs,
  "function configurePasskeyAccess()",
  "System passkey setup",
);
assert(
  systemBootBlock.indexOf("await refreshSystemSummary();") >= 0 &&
    systemBootBlock.indexOf("await refreshAccountList().catch(() => {});") >
      systemBootBlock.indexOf("await refreshSystemSummary();"),
  "System must load access role before rendering account admin actions",
);
assert(
  !systemPasskeyConfigBlock.includes("refreshAccountList()"),
  "System must not render accounts before access role is known",
);
assert(
  system.includes('<h1 id="appearance-title" class="pc2-section-title">Appearance</h1>') &&
    system.includes('data-settings="personalization"'),
  "System must keep appearance as a first-class settings area",
);
assert(
  system.includes('<h1 id="device-title" class="pc2-section-title">This Device</h1>') &&
    system.includes('data-settings="about"') &&
    system.includes("settings-sidebar-text\">About</span>") &&
    !system.includes("settings-sidebar-text\">System</span>") &&
    !system.includes("settings-sidebar-text\">Runtime</span>"),
  "System app must expose technical device details as About, not as duplicate Runtime sections",
);
assert(
  !system.includes('<h2 id="identity-title">Profile</h2>') &&
    !system.includes('<h2 id="account-title">Account</h2>') &&
    !system.includes('<h2 id="status-title">Local state</h2>') &&
    !system.includes('<h2 id="networks-title">Networks</h2>'),
  "System must not preserve the old Account/Profile/Local state/Networks dashboard structure",
);
assert(
  system.includes('<h2 id="access-title" class="pc2-section-title">Access</h2>') &&
    system.includes("<dt>Guest access</dt>"),
  "System must expose guest signup as the concise Access setting",
);
const staleSystemIdentityLabel = "<dt>Runtime " + "identity</dt>";
assert(
  !system.includes(staleSystemIdentityLabel),
  "System must not label the device DID as runtime identity",
);
assert(
  system.includes("<dt>Device identity</dt>"),
  "System must label the DID as device identity",
);
assert(
  system.indexOf('<h1 id="about-panel-title">About</h1>') <
    system.indexOf("<dt>Device identity</dt>"),
  "System device DID must live under About, not the primary account surface",
);
assert(
  system.includes("<dt>Accounts</dt>") &&
    !system.includes("<dt>Access keys</dt>") &&
    !system.includes("<dt>Passkeys</dt>"),
  "System must show accounts as the user-facing ontology",
);
assert(
  systemJs.includes("function accountRow(") &&
    systemStyle.includes(".account-table"),
  "System accounts must render as a responsive table instead of a long card list",
);
assert(
  !system.includes('id="handle-input"') &&
    !system.includes('data-field="handle-status"') &&
    !system.includes("<dt>Name</dt>") &&
    !system.includes("<dt>Handle</dt>") &&
    !systemJs.includes("configureHandleEditor") &&
    !systemJs.includes("onHandleSubmit"),
  "System must not retain the old display-name Profile editor after moving Profile to People",
);
assert(
  !system.includes('id="passkey-name"') &&
    !system.includes("Create guest") &&
    !system.includes('id="passkey-signin"'),
  "System must not create guest passkeys; guests self-register from Home when enrollment is open",
);
assert(
  system.includes("Lets new people create their own guest account from Home."),
  "System guest policy copy must say admins open enrollment, not create guest keys",
);
assert(
  !system.includes("Passkeys unlock Home and scope app access."),
  "System must not show stale internal passkey explainer copy",
);
assert(
  system.includes('id="account-list"'),
  "System must expose account management through the existing passkey provider routes",
);
assert(
  system.includes("<dt>Guest access</dt>"),
  "System must expose the admin-controlled guest enrollment gate",
);
assert(
  systemJs.includes("guest_registration_enabled"),
  "System must render guest enrollment from runtime auth state",
);
assert(
  !systemJs.includes("navigator.credentials.get") ||
    (systemJs.includes("requestFreshPasskeyHomeToken") &&
      systemJs.includes("/api/auth/recovery/full-export") &&
      systemJs.includes("elastos.full-recovery-bundle.export.request/v1")),
  "System must not duplicate general Home sign-in; fresh passkey verification is allowed only for Full Recovery Bundle export",
);
assert(
  !systemJs.includes("navigator.credentials.create") &&
    !systemJs.includes("serializeCreatedCredential"),
  "System must not create guest passkeys for other people",
);
assert(
  !systemJs.includes('showPasskeyStatus("Signed in"'),
  "System must not show redundant Signed in passkey status",
);
assert(
  systemJs.includes("protectsLastAdmin"),
  "System passkey UI must reflect the runtime last-admin protection rule",
);
assert(
  systemJs.includes("data-passkey-promote") &&
    systemJs.includes("promote-admin") &&
    gatewayApi.includes("/api/auth/passkeys/:proof_binding_id/promote-admin"),
  "System must expose admin-only guest passkey promotion through a runtime auth route",
);
assert(
  systemJs.includes("data-passkey-demote") &&
    systemJs.includes("demote-guest") &&
    systemJs.includes("Make guest") &&
    gatewayApi.includes("/api/auth/passkeys/:proof_binding_id/demote-guest"),
  "System must expose admin-only admin-to-guest demotion through a runtime auth route",
);
assert(
  systemJs.includes('passkeyRole === "admin" && !passkey.current'),
  "System must only show demotion for another admin passkey",
);
assert(
  !systemJs.includes('showRecoveryStatus("Ready"'),
  "System Account must not show redundant Ready recovery copy",
);
assert(
  systemJs.includes("recoveryStatusNode.hidden = text.length === 0"),
  "System Account must hide empty recovery status text instead of rendering a blank chip",
);
assert(
  shellAuth.includes("guest_registration_enabled"),
  "Home unlock must respect the guest enrollment gate before creating guests",
);
assert(
  shellAuth.includes("/api/auth/sessions/refresh"),
  "Home auth client must use the runtime session refresh route",
);
assert(
  shellJs.includes("refreshHomeSession"),
  "Home shell must refresh proof-bound sessions after sign-in",
);
assert(
  shellJs.includes("SESSION_REFRESH_MS"),
  "Home shell must keep signed sessions fresh on long-lived desktops",
);
assert(
  shellCore.includes("HOME_BROWSER_CONTEXT_KEY") &&
    shellCore.includes("browser_context_id"),
  "Home open-window restore must be bound to a browser-context id so clearing site data cannot replay stale windows",
);
assert(
  shellCore.includes("newBrowserContextId") &&
    shellCore.includes("getRandomValues") &&
    !shellCore.includes("Math.random()"),
  "Home browser context ids must use browser crypto instead of random fallback ids",
);
assert(
  shellWindows.includes("seenTargets") &&
    shellWindows.includes("seenTargets.has(targetId)"),
  "Home session restore must de-dupe targets so one stale session cannot spawn repeated System windows",
);
assert(
  protectedHomeStateSmoke.includes("home_browser_state"),
  "Protected Home state smoke must run the source HTTP regression",
);
assert(
  protectedHomeStateSmoke.includes("/api/apps/home/summary"),
  "Protected Home state smoke must prove the live Home summary path",
);
assert(
  protectedHomeStateSmoke.includes("ELASTOS_HOME_TOKEN"),
  "Protected Home state smoke must support an explicit signed Home state proof",
);
assert(
  !shellIndex.includes("home-unlock-kicker") &&
    shellAuth.includes(
      "Use your passkey to unlock your data, apps and desktop.",
    ),
  "Home passkey login must use concise data/apps/desktop copy without redundant kicker text",
);
assert(
  shellIndex.includes('id="home-unlock-name"') &&
    shellAuth.includes("display_name: displayName"),
  "Home passkey creation must collect and persist a passkey/user display name",
);
assert(
  shellAuth.includes("Enter a name for this passkey."),
  "Home must not create anonymous Passkey/guest principals",
);
assert(
  shellAuth.includes("Create guest account") &&
    shellAuth.includes("create your own guest account"),
  "Home must present guest enrollment as self-registration",
);
assert(
  shellAuth.includes("startAutomaticPasskeySignIn") &&
    shellAuth.includes("Choose your passkey."),
  "Home sign-in must automatically ask for a passkey instead of requiring a duplicate continue click",
);
assert(
  shellAuth.includes('unlockMode === "create_guest"') &&
    shellAuth.includes("Back to sign in"),
  "Home guest creation must be a distinct state, not blended into sign-in",
);
assert(
  shellAuth.includes("setUnlockNameVisible(canCreate)") &&
    !shellAuth.includes(
      "const canCreate = !registered || guestRegistrationEnabled",
    ),
  "Home sign-in must not show the passkey-name input unless a passkey is being created",
);
assert(
  shellAuth.includes("isPasskeyNotSelected") &&
    shellAuth.includes('setUnlockStatus("No passkey selected.", "muted")'),
  "Home sign-in must suppress raw WebAuthn cancellation errors and keep onboarding actionable",
);
assert(
  shellAuth.includes(
    "unlockSecondary.hidden = !registered || !guestRegistrationEnabled",
  ),
  "Home guest creation must stay available in both modal and prompt unlock presentations",
);
assert(
  !shellAuth.includes("getClientExtensionResults"),
  "Home must not capture or serialize raw WebAuthn extension output until client-side PRF wrapping exists",
);
assert(
  !authGatewayApi.includes("clientExtensionResults") &&
    !authGatewayApi.includes("prf_output"),
  "Runtime auth routes must not accept raw WebAuthn PRF output",
);
assert(
  webauthnIdentity.includes(
    'serde(rename_all = "camelCase", deny_unknown_fields)',
  ) &&
    webauthnIdentity.includes(
      "registration_response_rejects_extension_payloads",
    ) &&
    webauthnIdentity.includes(
      "authentication_response_rejects_extension_payloads",
    ),
  "Runtime WebAuthn response structs must reject hidden extension payloads until client-side PRF wrapping exists",
);
assert(
  protectedContent.includes("pub struct SealedObjectV1") &&
    protectedContent.includes("#[serde(deny_unknown_fields)]") &&
    protectedContent.includes(
      "sealed_object_rejects_unknown_contract_fields",
    ) &&
    protectedContent.includes(
      "sealed_object_rejects_unknown_nested_key_envelope_fields",
    ) &&
    protectedContent.includes(
      "key_release_request_rejects_unknown_contract_fields",
    ) &&
    protectedContent.includes(
      "decrypt_session_request_rejects_unknown_contract_fields",
    ),
  "Protected-content contracts must reject hidden object, key-release, and decrypt-session authority fields at decode time",
);
assert(
  shellStyle.includes(".visually-hidden") &&
    shellIndex.includes('class="visually-hidden"'),
  "Home unlock labels must use a real visually-hidden utility instead of leaking form labels into the UI",
);
assert(
  !shellStyle.includes("home-unlock-kicker"),
  "Home passkey login must not keep dead kicker CSS after removing the label",
);
assert(
  !shellAuth.includes("No password. No wallet required."),
  "Home passkey login must not show redundant no-password/no-wallet copy",
);
assert(
  shellAuth.includes("unlockStatus.hidden = !message;"),
  "Home passkey login must hide the status row when no status copy is shown",
);
assert(
  !shellIndex.includes("Checking passkey status.") &&
    !shellAuth.includes("Checking passkey status.") &&
    shellAuth.includes("Opening Home"),
  "Home passkey flow must not flicker from checking copy before the final unlock card",
);
assert(
  shellIndex.includes("toolbar-sign-out") &&
    shellAuth.includes("/api/auth/sessions/sign-out"),
  "Home must expose an explicit sign-out path that clears the browser session through Runtime",
);
assert(
  shellStyle.includes(".sign-out-btn") &&
    shellStyle.includes('background-image: url("data:image/svg+xml'),
  "Home sign-out toolbar icon must use a complete SVG glyph",
);
assert(
  !shellStyle.includes(".sign-out-btn::before") &&
    !shellStyle.includes(".sign-out-btn::after"),
  "Home sign-out icon must not use clipped pseudo-element borders",
);
assert(
  !identityHandler.includes("host_fallback") &&
    !identityHandler.includes("Fallback to Referer"),
  "WebAuthn RP handling must not describe host authority as a fallback path",
);
assert(
  !system.includes("<dt>Wallet</dt>") &&
    !system.includes("wallet-create") &&
    !systemJs.includes("/api/apps/system/wallet/managed"),
  "System must not duplicate Wallet account creation or approval controls",
);
assert(
  system.includes('id="recovery-password"') &&
    system.includes("Download Recovery Kit") &&
    system.includes("Downloads everything recoverable for this account") &&
    systemJs.includes("download_password") &&
    systemJs.includes("recoveryDownloadPassword") &&
    systemJs.includes("elastos.full-recovery-bundle.export.request/v1") &&
    systemJs.includes("/api/auth/recovery/full-export") &&
    authGatewayApi.includes("elastos.full-recovery-bundle/v1") &&
    authGatewayApi.includes("wallet_recovery_keys_for_principal"),
  "System Recovery Kit download must be the full recover-everything path: data root plus built-in Wallet keys with optional password wrapping",
);
assert(
    system.includes('id="recovery-import"') &&
    system.includes('id="recovery-pending"') &&
    system.includes("Recover account") &&
    systemJs.includes("pendingRecoveryImport") &&
    systemJs.includes("onRecoveryAttach"),
  "System Recovery Kit import must expose an explicit in-surface reassignment review before recovering an existing account",
);
assert(
  systemJs.includes(
    "reassign_to_current_principal: Boolean(reassign && allowReassign)",
  ) && !systemJs.includes("reassign_to_current_principal: reassign,"),
  "System Recovery Kit import must not silently infer root reassignment",
);
assert(
  !systemJs.includes("window.confirm") &&
    !systemJs.includes("confirm(") &&
    !systemJs.includes("alert("),
  "System Recovery Kit import must not use browser prompts for reassignment authority",
);
assert(
  !system.includes('data-field="wallet-status"') &&
    !system.includes(
      "Passkey-controlled built-in wallet. Apps never receive wallet authority.",
    ),
  "System wallet surface must stay removed after Wallet becomes the owner of accounts and approvals",
);
assert(
  !systemJs.includes("MANAGED_WALLET_SUPPORTED_CHAIN_NAMESPACES") &&
    gatewayApi.includes("MANAGED_WALLET_CHAIN_NAMESPACES") &&
    gatewayApi.includes('"bip122:000000000019d6689c085ae165831e93"'),
  "Built-in managed wallet creation belongs to Gateway/Wallet, not System browser code",
);
assert(
  !systemJs.includes("/api/apps/system/wallet/default") &&
    walletProvider.includes("set_default_account") &&
    walletProvider.includes("default_account"),
  "Default wallet selection must live in Wallet provider surfaces, not System Advanced",
);
assert(
  walletProvider.includes("chain_namespace is required") &&
    walletProvider.includes(
      "wallet account does not match requested chain_namespace",
    ) &&
    gatewayApi.includes('"op": "set_default_account"') &&
    gatewayTests.includes('"chain_namespace": "eip155:20"'),
  "Wallet signing must be chain-and-intent scoped before resolving a default or explicit account",
);
assert(
  walletProvider.includes("managed_key_aad") &&
    walletProvider.includes("Payload {") &&
    walletProvider.includes("tampered_principal") &&
    walletProvider.includes("tampered_chain"),
  "Managed wallet private-key envelopes must be principal/metadata-bound and tamper-tested",
);
assert(
  !walletProvider.includes(
    '"managed_wallet_storage": "localhost://ElastOS/SystemServices/Wallet/wallet-key.hex"',
  ) &&
    !walletProvider.includes(
      '"storage": "localhost://ElastOS/SystemServices/Wallet/wallet-state.json"',
    ),
  "Wallet-provider status/init responses must not expose internal wallet storage object paths",
);
assert(
  walletProvider.includes("deny_unknown_fields") &&
    walletProvider.includes(
      "wallet_provider_rejects_hidden_signature_request_fields",
    ) &&
    walletProvider.includes(
      "wallet_provider_rejects_hidden_connector_completion_fields",
    ),
  "Wallet-provider wire requests must reject hidden signing, connector, and wallet-object fields at decode time",
);
assert(
  gatewayApi.includes("WALLET_CAPSULE_ID") &&
    gatewayApi.includes("WALLET_WALLETCONNECT_CAPSULE_ID") &&
    gatewayApi.includes("WALLET_LINK_CAPSULE_IDS") &&
    gatewayApi.includes("WALLET_WALLETCONNECT_CAPSULE_ID") &&
    authGatewayApi.includes(
      '"connector_id": wallet_connector_id_for_wallet_link(&app)?',
    ) &&
    !authGatewayApi.includes("app == super::gateway::WALLET_CAPSULE_ID"),
  "External wallet linking must be owned by dedicated connector capsules instead of Home/System/Wallet manual proof",
);
assert(
  gatewayApi.includes("/api/apps/:wallet_connector/wallet/approvals") &&
    gatewayApi.includes("unknown wallet connector capsule") &&
    gatewayTests.includes(
      "test_wallet_connector_route_rejects_unknown_connector_capsule",
    ),
  "Wallet connector approval routes must be generic connector-capsule routes and reject unknown connector IDs",
);
assert(
  gatewayApi.includes("WALLET_CONNECTOR_CAPSULE_IDS") &&
    gatewayApi.includes("/api/apps/:wallet_connector/wallet/config") &&
    gatewayApi.includes("WALLETCONNECT_CONFIG_SCHEMA") &&
    gatewayApi.includes("WALLETCONNECT_SDK_PATH") &&
    gatewayTests.includes(
      "test_walletconnect_connector_requires_pinned_config",
    ) &&
    gatewayTests.includes(
      "test_walletconnect_connector_accepts_pinned_config",
    ) &&
    gatewayTests.includes(
      "test_walletconnect_connector_config_returns_pinned_sdk_contract",
    ) &&
    browserCapsulesApi.includes(
      "walletconnect_browser_capsule_requires_pinned_runtime_config",
    ) &&
    browserCapsulesApi.includes(
      "ensure_wallet_connector_configured(data_dir, app)",
    ) &&
    !shellJs.includes("wallet-walletconnect") &&
    !systemJs.includes("wallet-walletconnect") &&
    read("components.json").includes('"wallet-walletconnect"'),
  "WalletConnect must be installable as a capsule while its SDK/configuration stay fail-closed until pinned and tested",
);
assert(
  walletWalletconnect.includes('id="wallet-connect"') &&
    walletWalletconnect.includes('id="wallet-accounts"') &&
    walletWalletconnect.includes('id="wallet-requests"') &&
    walletWalletconnectJs.includes('CONNECTOR_ID = "wallet-walletconnect"') &&
    walletWalletconnectJs.includes(
      `/api/apps/\${CONNECTOR_ID}/wallet/config`,
    ) &&
    walletWalletconnectJs.includes("connectWalletConnectEvm") &&
    walletWalletconnectJs.includes("/api/auth/evm/challenge") &&
    walletWalletconnectJs.includes("/api/auth/evm/verify") &&
    walletWalletconnectJs.includes(
      `/api/apps/\${CONNECTOR_ID}/wallet/accounts`,
    ) &&
    walletWalletconnectJs.includes(
      `/api/apps/\${CONNECTOR_ID}/wallet/approvals`,
    ),
  "WalletConnect source capsule must use the pinned connector config and runtime wallet-link/approval routes only",
);
assert(
  !walletWalletconnectJs.includes("https://") &&
    !walletWalletconnectJs.includes("unpkg") &&
    !walletWalletconnectJs.includes("jsdelivr") &&
    walletWalletconnectJs.includes("sdk_asset_path"),
  "WalletConnect connector must import the pinned local SDK asset, not an unpinned CDN",
);
assert(
  walletconnectVendorScript.includes(
    'APPKIT_VERSION="${APPKIT_VERSION:-1.8.19}"',
  ) &&
    walletconnectVendorScript.includes("@reown/appkit-adapter-wagmi") &&
    walletconnectVendorScript.includes("@walletconnect/ethereum-provider") &&
    walletconnectVendorScript.includes("@metamask/connect-evm") &&
    walletconnectVendorScript.includes("connectWalletConnectEvm") &&
    walletconnectVendorScript.includes("defineChain") &&
    walletconnectVendorScript.includes("sha256sum") &&
    authWalletSmoke.includes("bash -n scripts/vendor-walletconnect-adapter.sh"),
  "WalletConnect adapter vendoring must use exact package pins and stay syntax-checked",
);
assert(
  walletconnectConfigScript.includes(
    'CONFIG_SCHEMA = "elastos.walletconnect.connector/v1"',
  ) &&
    walletconnectConfigScript.includes('SDK_PACKAGE = "@reown/appkit"') &&
    walletconnectConfigScript.includes("sdk_sha256") &&
    walletconnectConfigScript.includes("connectWalletConnectEvm(options)") &&
    walletconnectConfigSmoke.includes(
      "configure-walletconnect-connector.mjs",
    ) &&
    authWalletSmoke.includes("walletconnect-connector-config-smoke.sh"),
  "WalletConnect operator config must have a smoke-tested local SDK hash pinning path",
);
assert(
  walletconnectConfigScript.includes('requiredFlag(flags, "project-id")') &&
    !walletconnectConfigScript.includes("ELASTOS_WALLETCONNECT_PROJECT_ID"),
  "WalletConnect config must require an explicit operator Project ID instead of environment or repository defaults",
);
assert(
  walletProviderDoc.includes(
    "WalletConnect is a dedicated connector capsule",
  ) &&
    walletProviderDoc.includes("wallet-provider SDK backend") &&
    walletProviderDoc.includes(
      "Do not commit a bundled default Reown Project ID",
    ),
  "WalletConnect docs must keep the connector/authority split and no bundled Project ID rule",
);
assert(
  walletProviderDoc.includes("User-Facing Wallet Ontology") &&
    walletProviderDoc.includes("Approval method") &&
    walletMetamask.includes("Add approval method") &&
    walletUnisat.includes("Add approval method") &&
    wallet.includes("Approval methods") &&
    wallet.includes("Total balance") &&
    walletWalletconnect.includes("Add approval method") &&
    !walletMetamask.includes("Wallet Connector") &&
    !walletUnisat.includes("Wallet Connector") &&
    !walletWalletconnect.includes("Wallet Connector"),
  "Wallet UI and docs must present connector capsules as approval methods under one Wallet model",
);
assert(
  walletProvider.includes("external wallet links require a connector_id") &&
    walletProvider.includes(
      "wallet approval request belongs to a different connector",
    ) &&
    walletJs.includes("connector_id"),
  "External wallet approvals must carry connector_id and fail closed when a connector does not match",
);
assert(
  !walletProvider.includes("PrepareTransaction") &&
    !walletProvider.includes("BroadcastTransaction") &&
    !read("capsules/wallet-provider/capsule.json").includes(
      "broadcast_transaction",
    ),
  "Wallet-provider must not duplicate chain-provider transaction prepare/broadcast authority",
);
assert(
  walletProvider.includes("sign_eip155_legacy_transaction") &&
    walletProvider.includes("external_transaction_result") &&
    walletProvider.includes("awaiting_wallet_transaction") &&
    walletProvider.includes("elastos.wallet.signed_transaction/v1") &&
    walletMetamaskJs.includes("eth_sendTransaction") &&
    walletWalletconnectJs.includes("eth_sendTransaction"),
  "Built-in EVM transaction signing must be typed and external transaction completion must stay connector-bound",
);
assert(
  chainProvider.includes("deny_unknown_fields") &&
    chainProvider.includes(
      "chain_provider_rejects_hidden_prepare_transaction_fields",
    ) &&
    chainProvider.includes(
      "chain_provider_rejects_hidden_node_lifecycle_fields",
    ),
  "Chain-provider wire requests must reject hidden raw transaction and node RPC authority fields at decode time",
);
assert(
  walletProvider.includes("verify_contract_proof") &&
    walletProvider.includes("siwe_erc1271") &&
    chainProvider.includes("erc1271_is_valid_signature") &&
    authGatewayApi.includes("verify_contract_proof"),
  "ERC-1271 wallet proofs must be chain-provider verified before wallet-provider consumes the Runtime challenge",
);
assert(
  walletProvider.includes("verify_bip322_simple") &&
    walletProvider.includes("verify_bitcoin_signed_message") &&
    walletProvider.includes("bip322_simple_p2tr_verifies") &&
    walletProvider.includes("bitcoin_signed_message_p2pkh_verifies") &&
    walletProvider.includes("bitcoin_signed_message_p2shwpkh_verifies") &&
    walletProvider.includes("challenge_and_verify_bitcoin_taproot_bip322_proof") &&
    walletProvider.includes(
      "challenge_and_verify_bitcoin_legacy_signed_message_proof",
    ) &&
    walletProvider.includes(
      "managed_btc_account_signs_bip322_after_runtime_approval",
    ) &&
    walletProvider.includes(
      "managed_btc_account_rejects_unbound_bip322_messages",
    ) &&
    walletProvider.includes(
      "bitcoin_bip322_challenge_rejects_unsupported_p2wsh_script",
    ) &&
    read("elastos/crates/elastos-server/src/provider_resource.rs").includes(
      "elastos://wallet/proof/bip322/verify",
    ),
  "Bitcoin wallet proof must use typed BIP-322/signed-message capability resources with fail-closed verification coverage",
);
assert(
  gatewayApi.includes("/api/auth/btc/challenge") &&
    gatewayApi.includes("/api/auth/btc/verify") &&
    authGatewayApi.includes('"op": "bitcoin_challenge"') &&
    authGatewayApi.includes('"op": "verify_bip322_proof"') &&
    gatewayTests.includes(
      "test_btc_wallet_link_rejects_system_token_without_connector",
    ) &&
    gatewayTests.includes("test_wallet_token_cannot_link_bip322_account"),
  "Bitcoin wallet proof linking must require a connector token and stay covered at the browser auth boundary",
);
assert(
  walletProvider.includes(
    "approval_external_bitcoin_request_completes_with_bip322_connector_signature",
  ) &&
    walletProvider.includes(
      "Bitcoin proof signing requires a supported Bitcoin account",
    ) &&
    walletJs.includes("/api/apps/wallet/wallet/approvals") &&
    walletJs.includes('actionButton("Open UniSat"') &&
    !walletJs.includes("Paste Bitcoin wallet signature") &&
    !wallet.includes('id="bitcoin-address"') &&
    !wallet.includes("Manual proof"),
  "Wallet must use built-in managed Bitcoin signing or connector handoff, not manual BIP-322 forms",
);
assert(
  read("elastos/crates/elastos-server/src/provider_resource.rs").includes(
    "wallet/{chain_namespace}/sign/{intent}",
  ) && carrierBridge.includes("wallet_signature_parts_from_uri"),
  "Wallet capability resources must bind chain namespace and intent through the Carrier/provider path",
);
assert(
  systemJs.includes('["eip155:1", "Ethereum"]') &&
    systemJs.includes("CHAIN_NAMESPACE_LABELS") &&
    systemJs.includes("`EVM ${chainId}`"),
  "System external wallet labels must use human network names instead of raw eip155 namespaces",
);
assert(
  gatewayApi.includes("MANAGED_WALLET_CHAIN_NAMESPACES") &&
    gatewayApi.includes("managed_wallet_label"),
  "System gateway must own the wallet-supported chain list and labels",
);
assert(
  !system.includes('id="wallet-metamask"') &&
    !system.includes('id="wallet-connect"'),
  "System must not host optional browser wallet connectors",
);
assert(
  !systemJs.includes("selectedMetaMaskProvider") &&
    !systemJs.includes("personal_sign") &&
    !systemJs.includes("eth_requestAccounts") &&
    !systemJs.includes("window.ethereum"),
  "System must not hold browser wallet adapter authority",
);
const tasks = read("TASKS.md");
const browserPlanningSurface = [
  tasks,
  read("docs/BROWSER_CAPSULE.md"),
  read("docs/BROWSER_PROVIDER_BAKEOFF.md"),
].join("\n");
assert(
  browserManifest.includes('"name": "browser"') &&
    browserManifest.includes('"elastos://browser/page"') &&
    browserManifest.includes('"elastos://browser/display"') &&
    browserManifest.includes('"elastos://browser/exit"') &&
    browserManifest.includes('"elastos://browser/profile"') &&
    browserManifest.includes('"elastos://browser/wallet-bridge"') &&
    browserManifest.includes('"name": "net-provider"') &&
    browserManifest.includes('"name": "wallet-provider"') &&
    !browserManifest.includes('"elastos://wallet/*"') &&
    !browserManifest.includes('"elastos://net/stream"') &&
    !browserManifest.includes("guest_network") &&
    !browserManifest.includes('"provides"'),
  "Browser capsule manifest must declare Browser-specific intents and provider dependencies without direct wallet, network, provider, or guest-network authority",
);
assert(
  wciAlignmentScript.includes(
    "app capsules must not open absolute external network URLs directly",
  ) &&
    wciAlignmentScript.includes("direct_external_network_patterns") &&
    wciAlignmentScript.includes("ordinary capsule {manifest.get('name', path)} opens direct external network") &&
    wciAlignmentScript.includes("rg_search $'fetch[[:space:]]*") &&
    wciAlignmentScript.includes("fetch[[:space:]]*") &&
    wciAlignmentScript.includes("\\.open[[:space:]]*") &&
    wciAlignmentScript.includes("absolute external XMLHttpRequest") &&
    wciAlignmentScript.includes("new[[:space:]]+WebSocket") &&
    wciAlignmentScript.includes("new[[:space:]]+EventSource") &&
    wciAlignmentScript.includes("sendBeacon"),
  "WCI alignment must forbid ordinary app capsule code from opening absolute external network URLs directly",
);
assert(
  netProvider.includes("exit_unavailable") &&
    netProvider.includes("private_network_blocked") &&
    netProvider.includes("direct host networking") &&
    netProvider.includes("deny_unknown_fields") &&
    read("capsules/net-provider/capsule.json").includes(
      '"provides": "elastos://net/*"',
    ),
  "Net provider must be a fail-closed Browser/Net boundary, not raw host networking",
);
assert(
  exitProvider.includes("exit_policy_blocked") &&
    exitProvider.includes("private_network_blocked") &&
    exitProvider.includes("direct host networking") &&
    exitProvider.includes('allowed == "*"') &&
    exitProvider.includes("deny_unknown_fields") &&
    exitProvider.includes("allowed_hosts") &&
    exitProvider.includes("allowed_private_targets") &&
    exitProvider.includes("allows_private_target") &&
    exitProvider.includes("stream_backend_can_allow_exact_runtime_gateway_private_target_only") &&
    exitProvider.includes("remote_carrier_exits") &&
    exitProvider.includes("elastos.exit.remote-carrier.discovery/v1") &&
    exitProvider.includes("elastos.exit.remote-carrier.quote/v1") &&
    exitProvider.includes("elastos.exit.remote-carrier-session/v1") &&
    exitProvider.includes("grant_id") &&
    exitProvider.includes("expires_at") &&
    exitProvider.includes('"state": exit.state(now)') &&
    exitProvider.includes("remote Carrier Exit grant_id must be a safe identifier") &&
    exitProvider.includes("Remote Carrier Exit grant is expired") &&
    exitProvider.includes("exit_permission_denied") &&
    exitProvider.includes("exit_quota_exceeded") &&
    exitProvider.includes("remote_carrier_exit_discovery_is_principal_scoped_and_policy_filtered") &&
    exitProvider.includes("remote_carrier_exit_expired_grant_is_diagnosable_but_not_usable") &&
    exitProvider.includes("remote_carrier_exit_enforces_active_stream_quota") &&
    exitProvider.includes("max_active_streams_per_principal") &&
    exitProvider.includes("remote_carrier_exit_enforces_principal_stream_quota_on_shared_grant") &&
    exitProvider.includes('"byte_transport": "carrier_stream"') &&
    gatewayBrowserApi.includes("browser_visible_remote_carrier_exits") &&
    gatewayBrowserApi.includes("scrub_exit_authority_fields") &&
    gatewayBrowserApi.includes('"remote_carrier_exit_count"') &&
    gatewayBrowserApi.includes('"remote_carrier_exits"') &&
    gatewayBrowserApi.includes('"allowed_principals"') &&
    gatewayBrowserApi.includes('"connect_ticket"') &&
    gatewayBrowserRouteTests.includes(
      "test_browser_app_summary_reports_remote_carrier_exit_policy_without_authority_leaks",
    ) &&
    gatewayBrowserRouteTests.includes(
      "test_raw_browser_engine_and_exit_provider_proxy_routes_are_unavailable",
    ) &&
    gatewayBrowserRouteTests.includes("/api/provider/browser-engine/launch") &&
    gatewayBrowserRouteTests.includes("/api/provider/browser-engine/page_status") &&
    gatewayBrowserRouteTests.includes("/api/provider/exit/open_stream") &&
    exitProvider.includes("max_body_bytes") &&
    exitProvider.includes("elastos.exit.http-fetch.result/v1") &&
    exitProvider.includes("elastos.exit.stream-session/v1") &&
    exitProvider.includes("elastos.adapter-ipc/v1") &&
    exitProvider.includes("elastos.exit.relay-ipc/v1") &&
    exitProvider.includes("AdapterIpcConfig") &&
    exitProvider.includes("RelayIpcConfig") &&
    exitProvider.includes("StreamRelay") &&
    !exitProvider.includes("runtime_stream_path") &&
    serverInfra.includes("ELASTOS_EXIT_PROVIDER_CONFIG") &&
    gatewayBrowserApi.includes("gateway_browser_net_http") &&
    gatewayBrowserApi.includes("browser_reserve_stream_session") &&
    gatewayBrowserApi.includes('"stream_nonce"') &&
    gatewayBrowserApi.includes('"remote_exit_id"') &&
    gatewayBrowserApi.includes('"op": "http_fetch"') &&
    gatewayBrowserApi.includes('"open_stream"') &&
    !gatewayApi.includes("fn gateway_browser_net_http(") &&
    read("capsules/exit-provider/capsule.json").includes(
      '"provides": "elastos://exit/*"',
    ) &&
    read("capsules/exit-provider/capsule.json").includes(
      "discover_remote_carrier_exits",
    ),
  "Exit provider must be an internal fail-closed egress contract with operator-configured http_fetch/stream_relay/remote-Carrier exits, private adapter_ipc/relay_ipc descriptors, principal-scoped discovery, permission/accounting, public-web wildcard support, and no raw host networking or Runtime stream-path authority",
);
assert(
  remoteCarrierExitOperatorReport.includes(
    "elastos.remote-carrier-exit.operator-evidence/v1",
  ) &&
    remoteCarrierExitOperatorReport.includes("sha256File") &&
    remoteCarrierExitOperatorReport.includes("localArtifactTextIsRedacted") &&
    remoteCarrierExitOperatorReport.includes("fs.existsSync(value.path)") &&
    remoteCarrierExitOperatorReport.includes(
      "artifacts.${artifact}.sha256 must match the local redacted artifact file",
    ) &&
    remoteCarrierExitOperatorReport.includes(
      "artifacts.${artifact}.path must point to a redacted artifact without private route material",
    ) &&
    remoteCarrierExitOperatorReport.includes(
      'for (const field of ["principal", "grant_id", "target"])',
    ) &&
    remoteCarrierExitOperatorReport.includes(
      "evidence.remote_exit_discovery_observed must cite route.principal and route.grant_id",
    ) &&
    remoteCarrierExitOperatorReport.includes(
      "installed_artifact_readiness_observed",
    ) &&
    remoteCarrierExitOperatorReport.includes(
      "installed_artifact_readiness",
    ) &&
    remoteCarrierExitOperatorReport.includes("route_readiness_observed") &&
    remoteCarrierExitOperatorReport.includes("route_readiness") &&
    remoteCarrierExitOperatorReport.includes(
      "artifacts.route_readiness.path must report ok=true",
    ) &&
    remoteCarrierExitOperatorReport.includes(
      "artifacts.route_readiness.path route.${field} must match report.route.${field}",
    ) &&
    remoteCarrierExitOperatorReport.includes(
      "artifacts.route_readiness.path must include source.config_sha256",
    ) &&
    remoteCarrierExitOperatorReport.includes(
      "artifacts.installed_artifact_readiness.path must report ok=true",
    ) &&
    remoteCarrierExitOperatorReport.includes(
      "artifacts.installed_artifact_readiness.path must prove gateway Browser Carrier stream strings",
    ) &&
    remoteCarrierExitOperatorReport.includes(
      "evidence.two_runtimes_distinct must cite source_runtime.did and exit_runtime.did",
    ) &&
    remoteCarrierExitOperatorReport.includes(
      "evidence.two_runtimes_distinct must cite source_runtime.endpoint and exit_runtime.endpoint",
    ) &&
    remoteCarrierExitOperatorReport.includes(
      "evidence.policy_target_allowlist_enforced must cite route.target",
    ) &&
    remoteCarrierExitOperatorReport.includes(
      "evidence.principal_accounting_observed must cite route.principal",
    ) &&
    remoteCarrierExitOperatorReport.includes(
      "evidence.cleanup_observed must cite route.principal or route.grant_id",
    ) &&
    remoteCarrierExitOperatorReport.includes("routeTargetReferences") &&
    remoteCarrierExitOperatorReport.includes(
      "artifacts.browser_machine_proof.path must cite route.target or its host",
    ) &&
    remoteCarrierExitOperatorReportSmoke.includes(
      "local_artifact_hash_mismatch_rejected",
    ) &&
    remoteCarrierExitOperatorReportSmoke.includes(
      "local_artifact_secret_leak_rejected",
    ) &&
    remoteCarrierExitOperatorReportSmoke.includes(
      "local_browser_proof_target_mismatch_rejected",
    ) &&
    remoteCarrierExitOperatorReportSmoke.includes(
      "stale_artifact_readiness_rejected",
    ) &&
    remoteCarrierExitOperatorReportSmoke.includes(
      "stale_route_readiness_rejected",
    ) &&
    remoteCarrierExitOperatorReportSmoke.includes(
      "hash-bound route-readiness evidence",
    ) &&
    remoteCarrierExitOperatorReportSmoke.includes("route-bound cleanup evidence") &&
    remoteCarrierExitOperatorReportSmoke.includes("DID-bound runtime evidence") &&
    remoteCarrierExitOperatorReportSmoke.includes("endpoint-bound runtime evidence") &&
    remoteCarrierExitOperatorReportSmoke.includes("missing_principal_rejected") &&
    remoteCarrierExitOperatorReportSmoke.includes("weak_evidence_rejected") &&
    read("docs/CARRIER.md").includes("scans the reviewed artifact") &&
    read("docs/CARRIER.md").includes("installed artifact readiness report") &&
    read("docs/CARRIER.md").includes("route-readiness report") &&
    read("docs/CARRIER.md").includes("local route-readiness artifacts must be") &&
    read("docs/CARRIER.md").includes("local installed artifact readiness artifacts must be") &&
    read("docs/CARRIER.md").includes("the reviewed principal, grant id, and target") &&
    read("docs/CARRIER.md").includes("reviewed route target or target host") &&
    read("docs/CARRIER.md").includes("cleanup evidence must cite") &&
    read("docs/CARRIER.md").includes("private route material") &&
    currentState.includes("local redacted artifact hash mismatches") &&
    currentState.includes("stale or route-mismatched hash-bound route-readiness reports") &&
    currentState.includes("stale local installed artifact readiness reports") &&
    currentState.includes("missing route principals") &&
    currentState.includes("local Browser machine-proof artifacts that do not cite the reviewed route target or target host") &&
    currentState.includes("weak evidence that does not cite the reviewed source/exit runtime DIDs and endpoints") &&
    currentState.includes("weak evidence that does not cite the reviewed principal/grant/target/Carrier stream/cleanup route nouns") &&
    read("TASKS.md").includes("two-runtime evidence must cite the exact source/exit runtime DIDs and endpoint evidence") &&
    read("TASKS.md").includes("installed artifact readiness report") &&
    read("TASKS.md").includes("evidence for route readiness, installed artifact readiness") &&
    read("TASKS.md").includes("local Browser machine-proof artifact must cite the reviewed route target or target host") &&
    currentState.includes(
      "local redacted artifacts that still contain private route material",
    ),
  "Remote Carrier Exit operator evidence must hash-bind and redaction-check local artifacts while preserving remote-path evidence",
);
assert(
  remoteCarrierExitArtifactReadiness.includes(
    "elastos.remote-carrier-exit.artifact-readiness/v1",
  ) &&
    remoteCarrierExitArtifactReadiness.includes("browser_exit_stream") &&
    remoteCarrierExitArtifactReadiness.includes(
      "elastos.browser.carrier-stream/v1",
    ) &&
    remoteCarrierExitArtifactReadiness.includes(
      "elastos.exit.remote-carrier-session/v1",
    ) &&
    remoteCarrierExitArtifactReadiness.includes(
      "max_active_streams_per_principal",
    ) &&
    remoteCarrierExitArtifactReadinessSmoke.includes(
      "stale_gateway_rejected",
    ) &&
    remoteCarrierExitArtifactReadinessSmoke.includes(
      "stale_exit_provider_rejected",
    ) &&
    carrierOnlyAuthorityCheck.includes(
      "remote Carrier installed-artifact readiness contract",
    ) &&
    carrierOnlyAuthorityCheck.includes(
      "remote_carrier_installed_artifact_readiness",
    ) &&
    remoteCarrierExitPublicLivePlan.includes(
      "elastos.remote-carrier-exit.public-live-update-plan/v1",
    ) &&
    remoteCarrierExitPublicLivePlan.includes("mutation_allowed") &&
    remoteCarrierExitPublicLivePlan.includes("public_live_backup") &&
    remoteCarrierExitPublicLivePlan.includes("operator_workstation_stage_candidates") &&
    remoteCarrierExitPublicLivePlan.includes("candidate_staging_dir") &&
    remoteCarrierExitPublicLivePlan.includes("candidate_public_live_executables") &&
    remoteCarrierExitPublicLivePlan.includes("gateway_not_linux_x86_64_elf") &&
    remoteCarrierExitPublicLivePlan.includes("command_contexts") &&
    remoteCarrierExitPublicLivePlan.includes("public_server_after_explicit_approval") &&
    remoteCarrierExitPublicLivePlan.includes("rollback") &&
    remoteCarrierExitPublicLivePlanSmoke.includes("stale_candidate_rejected") &&
    remoteCarrierExitPublicLivePlanSmoke.includes("mach_o_candidate_rejected") &&
    remoteCarrierExitPublicLivePlanSmoke.includes("linux_x86_64_elf_required") &&
    remoteCarrierExitPublicLivePlanSmoke.includes("server_candidate_staging_required") &&
    remoteCarrierExitPublicLivePlanSmoke.includes("command_contexts_required") &&
    remoteCarrierExitPublicLivePlanSmoke.includes("dry_run_only") &&
    carrierOnlyAuthorityCheck.includes(
      "remote Carrier public-live update plan contract",
    ) &&
    carrierOnlyAuthorityCheck.includes("remote_carrier_public_live_update_plan") &&
    read("docs/CARRIER.md").includes(
      "readiness is not enough if the running binaries are stale",
    ) &&
    read("docs/CARRIER.md").includes("dry-run update plan") &&
    read("docs/CARRIER.md").includes("server-side staging directory") &&
    read("docs/CARRIER.md").includes("Linux x86_64 ELF") &&
    currentState.includes("server-side candidate directory"),
  "Remote Carrier Exit readiness must fail closed on stale installed gateway/provider artifacts before operator acceptance",
);
assert(
  browserEngineAdapter.includes("elastos.browser.engine.page/v1") &&
    browserEngineAdapter.includes("elastos.adapter-ipc/v1") &&
    browserEngineAdapter.includes("runtime_stream_path") &&
    browserEngineAdapter.includes("elastos.browser.engine.launch-request/v1") &&
    browserEngineAdapter.includes(
      "elastos.browser.engine.supervisor-result/v1",
    ) &&
    browserEngineAdapter.includes("ELASTOS_BROWSER_ENGINE_REQUEST") &&
    browserEngineAdapter.includes("byte_transport_unavailable") &&
    browserEngineAdapter.includes("engine_process_unavailable") &&
    browserEngineAdapter.includes("validate_supervisor_result") &&
    browserEngineAdapter.includes("adapter_ipc") &&
    browserEngineAdapter.includes("display_modes") &&
    browserEngineAdapter.includes("webrtc_signal") &&
    browserEngineAdapter.includes("diagnostics") &&
    browserEngineAdapter.includes("/pages/{page_id}/diagnostics") &&
    browserEngineAdapter.includes("direct_network") &&
    browserEngineAdapter.includes("wallet_injection") &&
    serverInfra.includes("ELASTOS_BROWSER_ENGINE_ADAPTER_CONFIG") &&
    gatewayBrowserApi.includes("browser_engine_summary") &&
    gatewayBrowserRouteTests.includes(
      "test_browser_app_summary_reports_registered_engine_adapter_status",
    ) &&
    read("capsules/browser-engine-adapter/capsule.json").includes(
      '"provides": "elastos://browser-engine/*"',
    ) &&
    read("capsules/browser-engine-adapter/capsule.json").includes("diagnostics"),
  "Browser Engine Adapter must be an internal fail-closed contract with explicit adapter_ipc transport, explicit display modes, WebRTC signaling, page diagnostics, and supervisor launch proof, not host browser authority or fake native page launches",
);
assert(
  browserEngineSupervisor.includes(
    "ELASTOS_BROWSER_ENGINE_SUPERVISOR_CONFIG",
  ) &&
    browserEngineSupervisor.includes("ELASTOS_BROWSER_ENGINE_REQUEST") &&
    browserEngineSupervisor.includes("ELASTOS_BROWSER_STREAM_BRIDGE_CONFIG") &&
    browserEngineSupervisor.includes("BrowserDisplayMode") &&
    browserEngineSupervisor.includes("NativeSurface") &&
    browserEngineSupervisor.includes('"mode": "native_surface"') &&
    browserEngineSupervisor.includes('"input": "native_ipc"') &&
    browserEngineSupervisor.includes("CLONE_NEWNET") &&
    browserEngineSupervisor.includes("bring_loopback_up") &&
    browserEngineSupervisor.includes("SIOCSIFFLAGS") &&
    browserEngineSupervisor.includes(
      "elastos.browser.engine.supervisor-config/v1",
    ) &&
    browserEngineSupervisor.includes(
      "elastos.browser.engine.supervisor-result/v1",
    ) &&
    browserEngineSupervisor.includes(
      "elastos.browser.stream-bridge.config/v1",
    ) &&
    browserEngineSupervisor.includes("stream_bridge_pid") &&
    browserEngineSupervisor.includes("ELASTOS_BROWSER_ENGINE_IPC") &&
    browserEngineSupervisor.includes("ELASTOS_BROWSER_ENGINE_RELAY_IPC") &&
    browserEngineSupervisor.includes("ELASTOS_BROWSER_ENGINE_STREAM_ID") &&
    browserEngineSupervisor.includes("ELASTOS_BROWSER_ENGINE_TARGET") &&
    browserEngineSupervisor.includes("ELASTOS_BROWSER_ENGINE_URL") &&
    browserEngineSupervisor.includes(
      "display_capabilities: DisplayCapabilities",
    ) &&
    browserEngineSupervisor.includes("config.display_capabilities.audio") &&
    browserEngineSupervisor.includes(
      "supervisor_result_does_not_claim_media_without_operator_capability",
    ),
  "Browser Engine Supervisor must enforce the typed Linux host-helper contract, return native_surface display sessions, optionally launch the stream bridge, bring loopback up for the local browser proxy, pass only explicit stream/IPC/relay/target/URL/operator environment to the native engine, and never claim native audio/video without explicit operator display capabilities",
);
assert(
  browserStreamBridge.includes("ELASTOS_BROWSER_STREAM_BRIDGE_CONFIG") &&
    browserStreamBridge.includes("elastos.browser.stream-bridge.config/v1") &&
    browserStreamBridge.includes("elastos.browser.stream-bridge.ready/v1") &&
    browserStreamBridge.includes("UnixListener") &&
    browserStreamBridge.includes("UnixStream") &&
    browserStreamBridge.includes("runtime_net_only") &&
    browserStreamBridge.includes("direct_network") &&
    browserStreamBridge.includes("adapter_ipc_path") &&
    browserStreamBridge.includes("runtime_stream_path") &&
    !browserStreamBridge.includes("TcpStream") &&
    !browserStreamBridge.includes("ToSocketAddrs"),
  "Browser Stream Bridge must be a typed Unix-socket byte transport only, with no TCP/DNS host-network path",
);
assert(
  browserLocalExit.includes("ELASTOS_BROWSER_LOCAL_EXIT_CONFIG") &&
    browserLocalExit.includes("elastos.browser.local-exit.config/v1") &&
    browserLocalExit.includes("elastos.exit.relay-open/v1") &&
    browserLocalExit.includes("allowed_hosts") &&
    browserLocalExit.includes("allowed_private_targets") &&
    browserLocalExit.includes("private_target_allowed") &&
    browserLocalExit.includes("wildcard_can_allow_exact_runtime_gateway_private_target_only") &&
    browserLocalExit.includes('allowed == "*"') &&
    browserLocalExit.includes("address_family") &&
    browserLocalExit.includes("PreferIpv4") &&
    browserLocalExit.includes("TcpStream") &&
    browserLocalExit.includes("ToSocketAddrs") &&
    browserLocalExit.includes("private resolved IP blocked"),
  "Browser Local Exit must be the only explicit server-side TCP/DNS relay and must require typed handshakes, public-web wildcard support, address-family policy, private-IP blocking, and allowlists",
);
assert(
  browserNativeOperatorConfig.includes("browser-engine-adapter.json") &&
    browserNativeOperatorConfig.includes("exit-provider.json") &&
    browserNativeOperatorConfig.includes("browser-local-exit.json") &&
    browserNativeOperatorConfig.includes(
      "ELASTOS_BROWSER_ENGINE_SUPERVISOR_CONFIG",
    ) &&
    browserNativeOperatorConfig.includes(
      "ELASTOS_BROWSER_NATIVE_PROXY_ENGINE_CONFIG",
    ) &&
    browserNativeOperatorConfig.includes("--proxy-server={proxy_url}") &&
    browserNativeOperatorConfig.includes("--address-family") &&
    browserNativeOperatorConfig.includes("runtime_net_only") &&
    browserNativeOperatorConfig.includes("native_surface") &&
    browserNativeOperatorConfig.includes("nativeAudio: false") &&
    browserNativeOperatorConfig.includes("nativeVideo: false") &&
    browserNativeOperatorConfig.includes("--native-audio") &&
    browserNativeOperatorConfig.includes("--native-video") &&
    browserNativeOperatorConfig.includes("display_capabilities") &&
    browserPlanningSurface.includes("browser-native-operator-config.mjs") &&
    read("docs/BROWSER_CAPSULE.md").includes(
      "browser-native-operator-config.mjs",
    ),
  "Native Browser operator config generator must keep Browser Engine Adapter, Exit provider, local Exit relay, address-family policy, supervisor, and proxy wrapper on one explicit runtime-net-only config path while defaulting native audio/video false unless explicitly declared by the operator",
);
assert(
  browserNativeSupervisorSmoke.includes(
    'result["display_session"]["audio"] is False',
  ) &&
    browserNativeSupervisorSmoke.includes(
      'result["display_session"]["video"] is False',
    ) &&
    browserNativeSupervisorSmoke.includes("native_audio_proven") &&
    browserNativeSupervisorProxySmoke.includes(
      'result["display_session"]["audio"] is False',
    ) &&
    browserNativeSupervisorProxySmoke.includes(
      'result["display_session"]["video"] is False',
    ) &&
    browserNativeSupervisorProxySmoke.includes("native_audio_proven"),
  "Native Browser namespace/proxy smokes must not pretend fake browser processes prove native audio or video",
);
assert(
  browserLocalExit.includes("upstream_http_proxy") &&
    browserLocalExit.includes("Proxy-Authorization") &&
    browserNativeOperatorConfig.includes("--upstream-http-proxy") &&
    browserNativeOperatorConfig.includes("upstream_http_proxy"),
  "Browser Local Exit must support operator-approved upstream HTTP CONNECT exits without exposing raw networking to capsules",
);
assert(
  read("docs/BROWSER_CAPSULE.md").includes("--upstream-http-proxy") &&
    read("docs/BROWSER_CAPSULE.md").includes(
      "scripts/browser-youtube-acceptance-smoke.sh",
    ) &&
    read("docs/BROWSER_CAPSULE.md").includes("decoded video and audio bytes"),
  "Browser docs must explain how an operator-approved upstream Exit is configured and gated by the YouTube media smoke",
);
assert(
  read("scripts/browser-native-target-preflight.sh").includes("--version") &&
    read("scripts/browser-native-target-preflight.sh").includes(
      "does not look like a Chromium/CEF-compatible browser",
    ),
  "Native Browser target preflight must reject non-browser executables before treating a host as proven",
);
assert(
  browserNativeHostCapability.includes(
    "elastos.browser.native-host-capability/v1",
  ) &&
    browserNativeHostCapability.includes("--require-product-native") &&
    browserNativeHostCapability.includes("host_compositor_display") &&
    browserNativeHostCapability.includes("host_audio_service") &&
    browserNativeHostCapability.includes("linux_network_namespace") &&
    browserNativeHostCapability.includes(
      "It does not install anything, launch a",
    ) &&
    browserNativeHostCapability.includes("or use Docker"),
  "Native Browser host capability probe must check browser/display/audio/network prerequisites without installing software or using Docker",
);
assert(
  browserNativeTargetPreflight.includes("browser-native-host-capability.mjs") &&
    browserNativeTargetPreflight.includes("--require-network-isolation") &&
    browserNativeTargetPreflight.includes(
      "native host capability probe failed",
    ) &&
    browserNativeTargetPreflight.includes('cat "$host_capability_report"') &&
    browserNativeTargetPreflight.includes(
      "browser-native-operator-config.mjs",
    ) &&
    browserNativeTargetPreflight.includes(
      "browser-native-supervisor-proxy-smoke.sh",
    ) &&
    browserNativeTargetPreflight.includes("target host is not proven") &&
    browserNativeTargetPreflight.includes(
      "capsules/exit-provider/Cargo.toml",
    ) &&
    browserNativeTargetPreflight.includes(
      "capsules/browser-engine-adapter/Cargo.toml",
    ) &&
    browserNativeTargetPreflight.includes(
      "--require-native-media requires both --native-audio and --native-video",
    ) &&
    browserNativeTargetPreflight.includes(
      "native media readiness requires display_capabilities audio=true and video=true",
    ) &&
    browserNativeTargetPreflight.includes(
      "native media readiness requires the target proof to report native_audio_proven=true and native_video_proven=true",
    ) &&
    browserNativeTargetPreflight.includes("native_audio_proven") &&
    browserNativeTargetPreflight.includes("native_video_proven") &&
    browserNativeTargetPreflight.includes(
      "elastos.browser.native-target-preflight/v1",
    ) &&
    browserNativeTargetPreflight.includes("native_media_required") &&
    browserPlanningSurface.includes("browser-native-host-capability.mjs") &&
    browserPlanningSurface.includes("browser-native-target-preflight.sh") &&
    read("docs/BROWSER_CAPSULE.md").includes(
      "browser-native-target-preflight.sh",
    ),
  "Native Browser target preflight must run the host capability probe, print fail-closed probe details, generate configs, validate provider capsule init, fail closed when host-gated namespace proof skips, and require explicit typed native media proof before claiming product audio/video readiness",
);
assert(
  browserManualUxReport.includes("--machine-artifact") &&
    browserManualUxReport.includes("validateManualUxReport") &&
    browserManualUxReport.includes("artifact.candidate") &&
    browserManualUxReport.includes("artifact.browser_program") &&
    browserManualUxValidation.includes('crypto.createHash("sha256")') &&
    browserManualUxValidation.includes(
      "machine_artifact.sha256 must be a 64-character hex SHA-256 digest",
    ) &&
    browserManualUxValidation.includes(
      "machine_artifact.schema must identify the accepted hosted bake-off, native preflight, or Mac VM proof schema",
    ) &&
    browserManualUxValidation.includes(
      "machine_artifact.path must point to the reviewed machine artifact JSON",
    ) &&
    browserManualUxValidation.includes(
      "machine_artifact.sha256 must match machine_artifact.path",
    ) &&
    browserManualUxValidation.includes(
      "machine_artifact.schema must match machine_artifact.path",
    ) &&
    browserManualUxValidation.includes(
      "machine_artifact.path must point to a successful machine artifact",
    ) &&
    browserManualUxReport.includes(
      "evidence.display_session_audio_advertised",
    ) &&
    browserManualUxReport.includes("evidence.received_audio_evidence") &&
    browserManualUxValidation.includes(
      "must describe the observed hosted WebRTC audio proof",
    ) &&
    browserManualUxReport.includes("Mac VM proof artifact") &&
    browserManualUxReport.includes("mac-vm") &&
    browserManualUxValidation.includes("successful Mac VM proof") &&
    browserManualUxValidation.includes("fresh restart evidence") &&
    browserManualUxValidation.includes("safe profile reset proof") &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "--machine-artifact /path/to/accepted-hosted-or-native-proof.json",
    ) &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "pre-fills provider and target from the machine artifact",
    ) &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes("checkmarks alone") &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "not sufficient audio evidence",
    ) &&
    browserProviderRunbook.includes(
      "--machine-artifact <accepted-hosted-or-native-proof.json>",
    ),
  "Browser manual UX report must generate and validate a hash-bound machine artifact reference, pre-fill provider/target from the artifact, and require text evidence for hosted WebRTC audio proof through the shared validator",
);
assert(
  browserObjectiveAudit.includes("elastos.browser.objective-audit/v1") &&
    browserObjectiveAudit.includes("Browser objective is not complete") &&
    browserObjectiveAudit.includes("hosted_provider_product_accepted") &&
    browserObjectiveAudit.includes("native_product_media_accepted") &&
    browserObjectiveAudit.includes("manual_ux_accepted") &&
    browserObjectiveAudit.includes("TASKS.md") &&
    browserObjectiveAudit.includes("ROADMAP.md") &&
    browserObjectiveAudit.includes("docs/BROWSER_PROVIDER_BAKEOFF.md") &&
    browserObjectiveAudit.includes("provider_decision_next_action_defined") &&
    browserObjectiveAudit.includes("consult_provider_decision_report") &&
    browserObjectiveAudit.indexOf("consult_provider_decision_report") <
      browserObjectiveAudit.indexOf("run_hosted_provider_bakeoff") &&
    browserObjectiveAudit.includes("function nextAction(") &&
    browserObjectiveAudit.includes("next_action: nextAction") &&
    browserObjectiveAudit.includes(
      "structured provider decision next_action",
    ) &&
    browserObjectiveAudit.includes("next_actions") &&
    browserObjectiveAudit.includes("--candidate kasm-workspaces") &&
    browserObjectiveAudit.includes("--candidate browserbox") &&
    browserObjectiveAudit.includes(
      "--native-audio --native-video --require-native-media",
    ) &&
    browserObjectiveAudit.includes("native_audio_proven === true") &&
    browserObjectiveAudit.includes("native_video_proven === true") &&
    browserObjectiveAudit.includes("qualityGateAccepted") &&
    browserObjectiveAudit.includes("Number(candidate.held_ms || 0) >= 5000") &&
    browserObjectiveAudit.includes("Number(youtube.held_ms || 0) >= 5000") &&
    browserObjectiveAudit.includes(
      'candidate.backend_class === "product_compositor"',
    ) &&
    browserObjectiveAudit.includes(
      "bakeoff.youtube_stress?.skipped !== true",
    ) &&
    browserObjectiveAudit.includes(
      "Number(youtube.media?.audio_decoded_delta || 0) > 0",
    ) &&
    browserObjectiveAudit.includes("acceptedMachineArtifacts") &&
    browserObjectiveAudit.includes("sha256File") &&
    browserObjectiveAudit.includes("validateManualUxReport") &&
    browserManualUxValidation.includes("realpath") &&
    browserManualUxValidation.includes("artifact.path") &&
    browserManualUxValidation.includes("machine_artifact.sha256") &&
    browserObjectiveAudit.includes("elastos.browser.manual-ux/v1") &&
    browserObjectiveAudit.includes(
      "elastos.browser.native-target-preflight/v1",
    ) &&
    browserPlanningSurface.includes("browser-objective-audit.mjs") &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "browser-objective-audit.mjs",
    ) &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "path, schema, or hash does not",
    ) &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "match the accepted machine proof",
    ),
  "Browser objective audit must remain the fail-closed completion gate for hosted/native media proof plus path/hash-bound manual UX evidence and must require the structured provider next-action path before encouraging bake-offs or native preflights",
);
assert(
  browserObjectiveAudit.includes("current_host_stop_condition_defined") &&
    browserObjectiveAudit.includes(
      "Current-host blockers stop local Browser provider tuning",
    ) &&
    browserObjectiveAudit.includes("busy_selkies_next_action_exercised") &&
    browserObjectiveAudit.includes(
      "do not spend more branch time tuning Selkies as the product path",
    ) &&
    browserObjectiveAuditSmoke.includes(
      "current_host_stop_condition_defined criterion must pass",
    ) &&
    browserObjectiveAuditSmoke.includes("provider decision report smoke") &&
    browserProviderDecisionReportSmoke.includes(
      "busy_selkies_next_action_exercised",
    ) &&
    browserPlanningSurface.includes(
      "Freeze new Browser provider implementation",
    ) &&
    read("ROADMAP.md").includes("Browser work should stop") &&
    read("ROADMAP.md").includes("contract/gate layer"),
  "Browser objective audit must expose the current-host stop condition so local work does not keep tuning the running Selkies baseline as product architecture",
);
assert(
  browserManualUxChecks.includes("COMMON_MANUAL_CHECKS") &&
    browserManualUxChecks.includes("HOSTED_WEBRTC_MANUAL_CHECKS") &&
    browserManualUxChecks.includes("display_session_audio_advertised") &&
    browserManualUxChecks.includes("received_audio_evidence") &&
    browserManualUxReport.includes("browser-manual-ux-validation.mjs") &&
    browserObjectiveAudit.includes("browser-manual-ux-validation.mjs") &&
    browserManualUxValidation.includes("browser-manual-ux-checks.mjs") &&
    browserManualUxValidation.includes("HOSTED_WEBRTC_MANUAL_CHECKS") &&
    browserManualUxValidation.includes("report.evidence[name]") &&
    !browserManualUxReport.includes("const COMMON_REQUIRED_CHECKS") &&
    !browserObjectiveAudit.includes("const COMMON_MANUAL_CHECKS"),
  "Browser manual UX report and objective audit must share one validation module so hosted WebRTC audio evidence cannot drift between scripts",
);
assert(
  browserObjectiveAuditSmoke.includes("native-declared-only.json") &&
    browserObjectiveAuditSmoke.includes('native_audio_proven": false') &&
    browserObjectiveAuditSmoke.includes(
      "declared_only_native_media_rejected",
    ) &&
    browserObjectiveAuditSmoke.includes(
      "native_product_media_accepted must fail",
    ) &&
    browserObjectiveAuditSmoke.includes("planned_evidence_is_durable") &&
    browserObjectiveAuditSmoke.includes(
      "planned_and_iterated evidence must use durable docs/scripts",
    ) &&
    browserObjectiveAuditSmoke.includes("hosted-shallow-ok.json") &&
    browserObjectiveAuditSmoke.includes("shallow_hosted_ok_rejected") &&
    browserObjectiveAuditSmoke.includes("hosted-skipped-youtube.json") &&
    browserObjectiveAuditSmoke.includes("skipped_youtube_rejected") &&
    browserObjectiveAuditSmoke.includes("manual-template-hosted.json") &&
    browserObjectiveAuditSmoke.includes("manual_template_prefilled") &&
    browserObjectiveAuditSmoke.includes(
      "manual UX template must prefill hosted provider",
    ) &&
    browserObjectiveAuditSmoke.includes(
      "manual UX template must prefill hosted target",
    ) &&
    browserObjectiveAuditSmoke.includes(
      "manual UX template must include empty hosted WebRTC audio evidence field",
    ) &&
    browserObjectiveAuditSmoke.includes("manual-hosted-detached.json") &&
    browserObjectiveAuditSmoke.includes("manual_hash_mismatch_rejected") &&
    browserObjectiveAuditSmoke.includes("machine artifact hash mismatch") &&
    browserObjectiveAuditSmoke.includes("manual-hosted-schema-mismatch.json") &&
    browserObjectiveAuditSmoke.includes("manual_schema_mismatch_rejected") &&
    browserObjectiveAuditSmoke.includes("machine artifact schema mismatch") &&
    browserObjectiveAuditSmoke.includes("manual-hosted-copy-path.json") &&
    browserObjectiveAuditSmoke.includes(
      "manual_artifact_path_mismatch_rejected",
    ) &&
    browserObjectiveAuditSmoke.includes("copied machine artifact") &&
    browserObjectiveAuditSmoke.includes("detached_manual_ux_rejected") &&
    browserObjectiveAuditSmoke.includes(
      "manual-hosted-missing-audio-evidence.json",
    ) &&
    browserObjectiveAuditSmoke.includes(
      "checkmarks without text-backed audio evidence",
    ) &&
    browserObjectiveAuditSmoke.includes(
      "hosted_manual_audio_evidence_required",
    ) &&
    browserObjectiveAuditSmoke.includes("manual-hosted-stale-check.json") &&
    browserObjectiveAuditSmoke.includes("legacy_frame_preview_audio") &&
    browserObjectiveAuditSmoke.includes("stale_manual_fields_rejected") &&
    browserObjectiveAuditSmoke.includes("manual-hosted-matched.json") &&
    browserObjectiveAuditSmoke.includes(
      "display session reported audio=true",
    ) &&
    browserObjectiveAuditSmoke.includes("matched_manual_ux_accepted"),
  "Browser objective audit smoke must prove declaration-only native audio/video, durable planned evidence, shallow hosted ok=true artifacts, skipped YouTube hosted artifacts, detached/manual hash-schema-or-path-mismatched UX, text-backed hosted audio evidence, stale manual fields, and ambiguous manual templates cannot satisfy the completion gate while a strict artifact with matching manual hash can pass",
);
assert(
  browserHostedProviderBakeoff.includes(
    "!skipYoutube && youtubeStatus === 0",
  ) &&
    browserHostedProviderBakeoff.includes(
      "rejected because product-compositor YouTube stress was skipped",
    ) &&
    browserHostedProviderBakeoff.includes("partial_candidate_ok") &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "Do not use `--skip-youtube` for acceptance",
    ),
  "Hosted provider bake-off must not produce accepted artifacts when YouTube/audio stress is skipped",
);
assert(
  browserHostedProviderBakeoff.includes("browser-manual-ux-checks.mjs") &&
    browserHostedProviderBakeoff.includes("manual_ux_schema") &&
    browserHostedProviderBakeoff.includes(
      'requiredManualChecksForSchema("elastos.browser.hosted-provider-bakeoff/v1")',
    ) &&
    read("scripts/browser-objective-audit-smoke.sh").includes(
      "display_session_audio_advertised",
    ) &&
    read("scripts/browser-objective-audit-smoke.sh").includes(
      "checks.display_session_audio_advertised must be true",
    ),
  "Hosted provider bake-off artifacts must emit the shared hosted WebRTC manual UX checklist instead of a stale prose checklist",
);
assert(
  read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes("`manual_ux_schema`") &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes("`manual_ux_checks`") &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "review guidance, not as a",
    ) &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "substitute for a signed-off manual UX report",
    ),
  "Browser provider bake-off docs must explain that artifact manual_ux_checks are guidance, not manual UX acceptance",
);
assert(
  browserProviderDecisionReport.includes("native_host_capability") &&
    browserProviderDecisionReport.includes(
      "browser-native-host-capability.mjs",
    ) &&
    browserProviderDecisionReport.includes("--require-product-native") &&
    browserProviderDecisionReport.indexOf(
      "This host appears ready for the native product path",
    ) < browserProviderDecisionReport.indexOf("single-session and has an active page") &&
    browserProviderDecisionReport.includes(
      "This host is not ready for native product media",
    ) &&
    browserProviderRunbook.includes("Current native host blockers") &&
    browserProviderRunbook.includes("Native host product-ready") &&
    browserProviderRunbook.includes("## Current Host Stop Condition") &&
    browserProviderRunbook.includes(
      "Do not keep tuning the running Selkies baseline as product architecture.",
    ) &&
    browserProviderRunbookSmoke.includes(
      "not accepted as product Browser proof",
    ),
  "Browser provider decision report/runbook must include native host readiness, an explicit current-host stop condition, and prioritize a native-ready host over more Docker/Selkies tuning",
);
assert(
  browserProviderDecisionReport.includes(
    "elastos.browser.provider-decision-report/v1",
  ) &&
    browserProviderDecisionReport.includes("candidate_readiness") &&
    browserProviderDecisionReport.includes("control_status") &&
    browserProviderDecisionReport.includes("active_pages") &&
    browserProviderDecisionReport.includes("single_session") &&
    browserProviderDecisionReport.includes("single-session and has an active page") &&
    browserProviderDecisionReport.includes("generateCandidateConfig") &&
    browserProviderDecisionReport.includes("generated_config_removed") &&
    browserProviderDecisionReport.includes(
      "operator_control_socket not provisioned",
    ) &&
    browserProviderDecisionReport.includes("fs.rmSync(prepared.cleanupDir") &&
    browserProviderDecisionReport.includes('hostedPreflight("browserbox"') &&
    browserProviderDecisionReport.includes(
      'hostedPreflight("kasm-workspaces"',
    ) &&
    browserProviderDecisionReport.includes('hostedPreflight("kasmvnc"') &&
    browserProviderDecisionReport.includes("goalStatus") &&
    browserProviderDecisionReport.includes('status: "blocked"') &&
    browserProviderDecisionReport.includes(
      "external provider/native evidence",
    ) &&
    browserProviderDecisionReport.includes("blockedBy") &&
    browserProviderDecisionReport.includes("return []") &&
    browserProviderDecisionReport.includes("nextAction") &&
    browserProviderDecisionReport.includes("next_action: nextAction") &&
    browserProviderDecisionReport.includes(
      "free_or_isolate_selkies_before_bakeoff",
    ) &&
    browserProviderDecisionReport.includes("keep_accepted_browser_artifacts") &&
    browserProviderDecisionReport.includes("provision_kasm_workspaces_first") &&
    browserProviderDecisionReport.includes("selkies_single_session_busy") &&
    browserProviderDecisionReport.includes("native_host_not_product_ready") &&
    browserProviderDecisionReport.includes("hostedBakeoffSummary") &&
    browserProviderDecisionReport.includes("hosted_bakeoff_rejected") &&
    browserProviderDecisionReport.includes("nativePreflightSummary") &&
    browserProviderDecisionReport.includes("native_preflight_rejected") &&
    browserProviderDecisionReport.includes(
      "native preflight did not prove required native audio/video media readiness",
    ) &&
    browserProviderDecisionReport.includes(
      "docker_is_product_architecture: false",
    ) &&
    browserProviderDecisionReport.includes(
      "managed_baseline_not_final_product",
    ) &&
    browserProviderDecisionReport.includes("Kasm Workspaces first") &&
    browserProviderDecisionReport.includes("BrowserBox if licensed") &&
    browserProviderDecisionReport.includes("objectiveAudit") &&
    browserProviderDecisionReportSmoke.includes(
      "elastos.browser.provider-decision-report-smoke/v1",
    ) &&
    browserProviderDecisionReportSmoke.includes("structured_next_action") &&
    browserProviderDecisionReportSmoke.includes("blocked_by_visible") &&
    browserProviderDecisionReportSmoke.includes(
      "candidate_readiness_visible",
    ) &&
    browserProviderDecisionReportSmoke.includes(
      "native_preflight_rejection_visible",
    ) &&
    browserProviderDecisionReportSmoke.includes(
      "native_preflight_acceptance_visible",
    ) &&
    browserProviderDecisionReportSmoke.includes(
      "accepted decision report must not keep unrelated live-host/provider blockers",
    ) &&
    browserProviderDecisionReportSmoke.includes(
      "temporary placeholder socket paths",
    ) &&
    browserProviderDecisionReportSmoke.includes("audio_product_proven") &&
    browserProviderDecisionReportSmoke.includes("manual_user_acceptance") &&
    browserPlanningSurface.includes("browser-provider-decision-report.mjs") &&
    browserPlanningSurface.includes("structured `next_action`") &&
    browserPlanningSurface.includes("hosted-candidate readiness matrix") &&
    browserPlanningSurface.includes("serialization blocker") &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "browser-provider-decision-report.mjs",
    ) &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "hosted-candidate readiness matrix",
    ) &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "top-level `goal_status`",
    ) &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "structured `next_action`",
    ) &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "top-level `native_preflight` summary",
    ) &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "native_preflight_rejected",
    ) &&
    read("state.md").includes(
      "summarizes supplied `hosted_bakeoff` and `native_preflight` artifacts",
    ) &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "removes it and reports the real blockers",
    ) &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "Generated placeholder socket paths must not be shown as operator instructions",
    ) &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "active singleton target is a serialization limit",
    ),
  "Browser provider decision reporting must inspect live adapter/service state, report BrowserBox/Kasm/Selkies readiness, clean up generated temporary configs, expose blocked goal status, structured next action, rejected hosted/native artifact summaries, accepted artifact preservation with no stale blockers, distinguish singleton busy state from single-VM multipage state, and point toward provider/native gates instead of treating the running Selkies Docker service as product completion",
);
assert(
  browserProviderDecisionReportSmoke.includes(
    "free_or_isolate_selkies_before_bakeoff",
  ) &&
    browserProviderDecisionReportSmoke.includes('owner !== "operator"') &&
    browserProviderDecisionReportSmoke.includes("separate provider instance") &&
    browserProviderDecisionReportSmoke.includes(
      "must not recommend more Selkies tuning",
    ) &&
    browserProviderDecisionReportSmoke.includes(
      "busy_selkies_next_action_exercised",
    ) &&
    browserPlanningSurface.includes("separate provider instance") &&
    browserPlanningSurface.includes("Selkies tuning") &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "using a separate provider instance",
    ) &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes("instead of more") &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes("Selkies tuning"),
  "Browser provider decision-report smoke must keep busy single-session Selkies as an operator-owned serialization blocker, not a local tuning instruction",
);
assert(
  browserProviderRunbook.includes("## Objective Checklist") &&
    browserProviderRunbook.includes("objectiveChecklistBlock") &&
    browserProviderRunbook.includes("prompt_to_artifact_checklist") &&
    browserProviderRunbook.includes("This runbook is read-only guidance") &&
    browserProviderRunbook.includes("It does not install vendors, launch") &&
    browserProviderRunbook.includes(
      "preserve it and use a separate provider instance",
    ) &&
    browserProviderRunbook.includes("--hosted-bakeoff") &&
    browserProviderRunbook.includes("--native-preflight") &&
    browserProviderRunbook.includes("--manual-ux") &&
    browserProviderRunbook.includes("regenerates the decision report") &&
    browserProviderRunbook.includes(
      "cannot be combined with proof artifacts",
    ) &&
    browserProviderRunbook.includes("Goal status:") &&
    browserProviderRunbook.includes("Selkies session: single-session=") &&
    browserProviderRunbook.includes("active-pages=") &&
    browserProviderRunbook.includes("## Current Host Stop Condition") &&
    browserProviderRunbook.includes("currentHostStopConditionBlock") &&
    browserProviderRunbook.includes("## Blocking Summary") &&
    browserProviderRunbook.includes("blockingSummaryBlock") &&
    browserProviderRunbook.includes("## Next Action") &&
    browserProviderRunbook.includes("nextActionBlock") &&
    browserProviderRunbook.includes("## Local Pass Checks") &&
    browserProviderRunbook.includes(
      "scripts/browser-provider-decision-report-smoke.sh",
    ) &&
    browserProviderRunbook.includes("## Expected-Failing Completion Audit") &&
    browserProviderRunbook.includes(
      "It should exit non-zero until product audio evidence",
    ) &&
    browserProviderRunbookSmoke.includes(
      "elastos.browser.provider-runbook-smoke/v1",
    ) &&
    browserProviderRunbookSmoke.includes("audio_product_proven") &&
    browserProviderRunbookSmoke.includes("manual_user_acceptance") &&
    browserProviderRunbookSmoke.includes(
      "This runbook is read-only guidance",
    ) &&
    browserProviderRunbookSmoke.includes("Goal status: `blocked`") &&
    browserProviderRunbookSmoke.includes(
      "Selkies session: single-session=`true`, active-pages=`1`, page-ids=`page:selkies-test`",
    ) &&
    browserProviderRunbookSmoke.includes("Goal status: `accepted`") &&
    browserProviderRunbookSmoke.includes(
      "Browser/audio objective has accepted product proof and manual UX evidence.",
    ) &&
    browserProviderRunbookSmoke.includes(
      "native_manual_artifact_forwarding_checked",
    ) &&
    browserProviderRunbookSmoke.includes(
      "hosted_artifact_forwarding_checked",
    ) &&
    browserProviderRunbookSmoke.includes("## Current Host Stop Condition") &&
    browserProviderRunbookSmoke.includes(
      "Do not keep tuning the running Selkies baseline as product architecture.",
    ) &&
    browserProviderRunbookSmoke.includes("## Blocking Summary") &&
    browserProviderRunbookSmoke.includes("## Next Action") &&
    browserProviderRunbookSmoke.includes(
      "free_or_isolate_selkies_before_bakeoff",
    ) &&
    browserProviderRunbookSmoke.includes("objective_checklist_rendered") &&
    browserProviderRunbookSmoke.includes("missing_audio_visible") &&
    browserProviderRunbookSmoke.includes("missing_manual_ux_visible") &&
    browserProviderRunbookSmoke.includes("local_pass_checks_rendered") &&
    browserProviderRunbookSmoke.includes(
      "expected_failing_completion_audit_rendered",
    ) &&
    browserProviderRunbookSmoke.includes(
      "--hosted-bakeoff /path/to/bakeoff.json",
    ) &&
    browserProviderRunbookSmoke.includes(
      "cannot be combined with proof artifacts",
    ) &&
    browserProviderRunbookSmoke.includes("hosted_bakeoff_rejected") &&
    browserProviderRunbookSmoke.includes("## Local Pass Checks") &&
    browserProviderRunbookSmoke.includes(
      "scripts/browser-provider-decision-report-smoke.sh",
    ) &&
    browserProviderRunbookSmoke.includes(
      "## Expected-Failing Completion Audit",
    ) &&
    browserPlanningSurface.includes("Current Host Stop Condition") &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "Current Host Stop Condition",
    ) &&
    browserPlanningSurface.includes(
      "scripts/browser-provider-runbook-smoke.sh",
    ) &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "browser-provider-runbook.mjs",
    ) &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "When a hosted/native proof or manual UX report already exists",
    ) &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "Do not combine these proof flags with `--decision-report`",
    ) &&
    read("TASKS.md").includes(
      "artifact-aware `scripts/browser-provider-runbook.mjs --hosted-bakeoff/--native-preflight --manual-ux`",
    ) &&
    read("state.md").includes(
      "operator guidance is generated from the actual evidence",
    ) &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "structured next action",
    ) &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes("`blocked_by` summary"),
  "Browser provider runbook must render, document, and smoke-test the objective checklist plus blocked goal, Selkies session occupancy, artifact-bound decision reports, current-host stop condition, structured next action, read-only safety boundary, and blocking summary so missing product audio, manual UX proof, and provider blockers stay visible before operator commands",
);
assert(
  currentState.includes("Last updated: 2026-06-29 UTC") &&
    currentState.includes(
      "Browser architecture is coherent enough to preserve",
    ) &&
    currentState.includes(
      "fails product audio proof and hash-bound manual UX evidence",
    ) &&
    currentState.includes(
      "Docker/Selkies is only `managed_baseline_not_final_product`",
    ) &&
    currentState.includes(
      "single-session; active pages are a serialization blocker",
    ) &&
    currentState.includes("not a product native-browser proof target") &&
    currentState.includes(
      "lacks a real host compositor/display, host audio service, and working network namespace support",
    ) &&
    currentState.includes(
      "Kasm Workspaces, BrowserBox, or KasmVNC cannot replace Selkies",
    ) &&
    currentState.includes("operator_control_socket not provisioned") &&
    currentState.includes(
      "hosted Selkies/GStreamer service is a managed baseline",
    ) &&
    currentState.includes("not accepted as the final Browser") &&
    currentState.includes("protected/recoverable Browser profile storage"),
  "state.md must preserve the current Browser truth: architecture valid, Selkies/Docker baseline-only, native proof blocked on this server, hosted candidates unprovisioned, and product audio/manual UX still incomplete",
);
assert(
  read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes("bbx install") &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "BROWSERBOX_LICENSE_CONFIRMED=1",
    ) &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "display_backend=browserbox_webrtc",
    ) &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes("KASM_BASE_URL") &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes("request_kasm") &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes("get_kasm_status") &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes("allow_kasm_audio") &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "must not leak Kasm API",
    ) &&
    browserPlanningSurface.includes("Kasm Workspaces") &&
    browserPlanningSurface.includes("BrowserBox") &&
    browserPlanningSurface.includes("Selkies tuning"),
  "BrowserBox/Kasm operator prerequisites must be explicit, provider-owned, and capability-aligned before more Selkies tuning",
);
assert(
  browserKasmControlService.includes("ELASTOS_BROWSER_KASM_CONTROL_CONFIG") &&
    browserKasmControlService.includes("request_kasm") &&
    browserKasmControlService.includes("get_kasm_status") &&
    browserKasmControlService.includes("delete_kasm") &&
    browserKasmControlService.includes(
      "kasm_product_display_bridge_required",
    ) &&
    browserKasmControlService.includes("product_display_bridge_socket") &&
    browserKasmControlService.includes(
      "Kasm display bridge must not leak raw Kasm session URLs",
    ) &&
    browserKasmControlService.includes("allow_kasm_audio") &&
    browserKasmControlServiceSmoke.includes("url_only_rejected_before_api") &&
    browserKasmControlServiceSmoke.includes(
      "product_bridge_preflight_passed",
    ) &&
    browserKasmControlServiceSmoke.includes("delete_called_on_close") &&
    browserPlanningSurface.includes(
      "scripts/browser-kasm-control-service.mjs",
    ) &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "scripts/browser-kasm-control-service.mjs",
    ),
  "Kasm hosted provider path must have a fail-closed lifecycle control service that rejects URL-only sessions before Kasm API calls, delegates display to a product bridge, and deletes Kasm sessions on Browser close",
);
assert(
  browserManualUxReport.includes("elastos.browser.manual-ux/v1") &&
    browserManualUxReport.includes("--template") &&
    browserManualUxReport.includes("--input") &&
    browserManualUxReport.includes("--machine-artifact") &&
    browserManualUxReport.includes("browser-manual-ux-checks.mjs") &&
    browserManualUxReport.includes("review_artifacts") &&
    browserManualUxChecks.includes("youtube_audible_audio") &&
    browserManualUxChecks.includes("glide_wallet_connect") &&
    browserManualUxChecks.includes("display_session_audio_advertised") &&
    browserManualUxChecks.includes("received_audio_evidence") &&
    browserManualUxChecks.includes("MAC_VM_MANUAL_CHECKS") &&
    browserManualUxChecks.includes("ela_city_edit_profile_modal") &&
    browserManualUxValidation.includes("elastos.browser.mac-vm-proof/v1") &&
    browserManualUxValidation.includes("macVmArtifactAccepted") &&
    browserManualUxValidation.includes("clickExpectedUrlMatches") &&
    browserManualUxValidation.includes("clickChangedFromStartingUrl") &&
    browserManualUxValidation.includes("changed click URL sync") &&
    browserManualUxValidation.includes("Runtime media relay proof") &&
    browserManualUxValidation.includes("hasEditProfileDiagnosticClick") &&
    browserManualUxValidation.includes("edit-profile diagnostic click proof") &&
    homeVirtualAuthSmoke.includes(
      "HOME_VIRTUAL_AUTH_BROWSER_UI_CLICK_TARGET_TIMEOUT_MS",
    ) &&
    homeVirtualAuthSmoke.includes("waitForBrowserHrefClickTarget") &&
    homeVirtualAuthSmoke.includes("display_session: summarizeDisplaySession(displaySession)") &&
    homeVirtualAuthSmoke.includes("engine_identity") &&
    homeVirtualAuthSmoke.includes("username_present") &&
    browserMacVmProof.includes("runtimeMediaRelayOk") &&
    browserMacVmProof.includes("vmIsolationOk") &&
    browserMacVmProof.includes("credentialed_turn_ice_server_count") &&
    browserManualUxValidation.includes("review_artifacts must include at least one hash-bound redacted Mac VM screen recording artifact") &&
    browserManualUxValidation.includes("review_artifacts[${index}].redacted must be true") &&
    browserManualUxValidation.includes("review_artifacts[${index}].sha256 must match") &&
    browserManualUxValidation.includes("without raw authority text") &&
    browserManualUxValidation.includes("must cite Edit Profile or Account Settings") &&
    browserManualUxValidation.includes("elastos.browser.mac-vm-control-restart/v1") &&
    browserManualUxValidation.includes("elastos.browser.profile-reset/v1") &&
    browserManualUxValidation.includes("principal_owned_reset_scoped_unprotected") &&
    browserManualUxValidation.includes("protected_storage === false") &&
    browserManualUxValidation.includes("removed_profile_disk === true") &&
    browserManualUxValidation.includes("expectedViewportWidth") &&
    browserManualUxValidation.includes("vmIsolation.adapter") &&
    browserManualUxReport.includes("Mac VM proof artifact") &&
    browserManualUxReport.includes("redacted=true") &&
    browserMacVmProof.includes("ELASTOS_BROWSER_MAC_VM_MAX_CONTROL_UPTIME_MS") &&
    browserMacVmProof.includes("expected_viewport_width") &&
    browserMacVmProof.includes("HOME_VIRTUAL_AUTH_BROWSER_OPEN_VIEWPORT_WIDTH") &&
    browserMacVmProof.includes("HOME_VIRTUAL_AUTH_BROWSER_OPEN_VIEWPORT_HEIGHT") &&
    browserMacVmProof.includes("elastos.browser.mac-vm-control-restart/v1") &&
    browserMacVmProof.includes("profileReset.profile?.storage_posture") &&
    browserMacVmProof.includes("profileReset.removed_profile_disk === true") &&
    gatewayBrowserRouteTests.includes(
      "test_browser_profile_reset_refuses_route_open_live_page",
    ) &&
    gatewayBrowserRouteTests.includes(
      "/api/apps/browser/profile/reset",
    ) &&
    gatewayBrowserRouteTests.includes(
      "requires all Browser pages",
    ) &&
    browserMacVmManualUxSmoke.includes("shallow_mac_artifact_rejected") &&
    browserMacVmManualUxSmoke.includes("stale_restart_rejected") &&
    browserMacVmManualUxSmoke.includes("url_unchanged_rejected") &&
    browserMacVmManualUxSmoke.includes("missing_runtime_media_relay_rejected") &&
    browserMacVmManualUxSmoke.includes("missing_edit_profile_diagnostic_rejected") &&
    browserMacVmManualUxSmoke.includes("missing_profile_reset_rejected") &&
    browserMacVmManualUxSmoke.includes("reset_without_removal_rejected") &&
    browserMacVmManualUxSmoke.includes("leaky_profile_reset_rejected") &&
    browserMacVmManualUxSmoke.includes("generic_edit_profile_evidence_rejected") &&
    browserMacVmManualUxSmoke.includes("review_artifact_required") &&
    browserMacVmManualUxSmoke.includes("review_artifact_hash_mismatch_rejected") &&
    browserMacVmManualUxSmoke.includes("review_artifact_redaction_required") &&
    browserMacVmManualUxSmoke.includes("review_artifact_secret_leak_rejected") &&
    browserMacVmManualUxSmoke.includes(
      "mac_manual_does_not_satisfy_product_audio_audit",
    ) &&
    browserMacVmManualUxSmoke.includes(
      "resized_mac_artifact_accepted",
    ) &&
    browserMacVmManualReviewPacket.includes("elastos.browser.mac-vm-manual-review-packet/v1") &&
    browserMacVmManualReviewPacket.includes("--handoff-summary") &&
    browserMacVmManualReviewPacket.includes("ok: false") &&
    browserMacVmManualReviewPacket.includes("Add at least one separate redacted screen recording") &&
    browserMacVmManualReviewPacketSmoke.includes("draft_fails_closed") &&
    browserMacVmManualReviewPacketSmoke.includes("mismatched_handoff_rejected") &&
    browserMacVmAcceptanceAudit.includes(
      "elastos.browser.mac-vm-acceptance-audit/v1",
    ) &&
    browserMacVmAcceptanceAudit.includes("--handoff-summary") &&
    browserMacVmAcceptanceAudit.includes("auth_setup_receipt_chain") &&
    browserMacVmAcceptanceAudit.includes(
      "handoff summary auth setup receipt sha256 must match its path",
    ) &&
    browserMacVmAcceptanceAudit.includes(
      "handoff summary auth setup receipt generated_at must be at or before machine proof generated_at",
    ) &&
    browserMacVmAcceptanceAudit.includes(
      "handoff summary generated_at must be at or after machine proof generated_at",
    ) &&
    browserMacVmAcceptanceAudit.includes("ela_city_authenticated_surface") &&
    browserMacVmAcceptanceAudit.includes("clickExpectedUrlMatches") &&
    browserMacVmAcceptanceAudit.includes("click_changed_from_starting_url") &&
    browserMacVmAcceptanceAudit.includes("browser_vm_isolation") &&
    browserMacVmAcceptanceAudit.includes("runtime_media_relay") &&
    browserMacVmAcceptanceAudit.includes("media_transport=runtime_relay") &&
    browserMacVmAcceptanceAudit.includes("source_home_restart_freshness") &&
    browserMacVmAcceptanceAudit.includes("browser_helper_rootfs_sha256") &&
    browserMacVmAcceptanceAudit.includes(
      "changed URL matching the recorded expected_url_re",
    ) &&
    browserMacVmAcceptanceAudit.includes("looks_unauthenticated") &&
    browserMacVmAcceptanceAudit.includes("ela_city_edit_profile_modal") &&
    browserMacVmAcceptanceAudit.includes("editProfileActionPattern") &&
    browserMacVmAcceptanceAudit.includes("authenticatedProfilePattern") &&
    browserMacVmAcceptanceAudit.includes("visible_text_samples") &&
    browserMacVmAcceptanceAudit.includes("dialog_elements") &&
    browserMacVmAcceptanceAudit.includes("has_edit_profile_dialog_signal") &&
    browserMacVmAcceptanceAudit.includes("elastos.browser.mac-vm-control-restart/v1") &&
    browserMacVmAcceptanceAudit.includes("profile_reset_safety") &&
    browserMacVmAcceptanceAudit.includes("elastos.browser.profile-reset/v1") &&
    browserMacVmAcceptanceAudit.includes("storage_posture=principal_owned_reset_scoped_unprotected") &&
    browserMacVmAcceptanceAudit.includes("protected_storage=false") &&
    browserMacVmAcceptanceAudit.includes("removed_profile_disk=true") &&
    browserMacVmAcceptanceAudit.includes("expectedViewportWidth") &&
    browserMacVmAcceptanceAuditSmoke.includes("missing_manual_rejected") &&
    browserMacVmAcceptanceAuditSmoke.includes(
      "unauthenticated_ela_city_rejected",
    ) &&
    browserMacVmAcceptanceAuditSmoke.includes(
      "generic_profile_text_rejected",
    ) &&
    browserMacVmAcceptanceAuditSmoke.includes("url_unchanged_rejected") &&
    browserMacVmAcceptanceAuditSmoke.includes("missing_runtime_media_relay_rejected") &&
    browserMacVmAcceptanceAuditSmoke.includes("missing_vm_isolation_rejected") &&
    browserMacVmAcceptanceAuditSmoke.includes("missing_source_home_restart_rejected") &&
    browserMacVmAcceptanceAuditSmoke.includes(
      "missing_profile_reset_rejected",
    ) &&
    browserMacVmAcceptanceAuditSmoke.includes(
      "reset_without_removal_rejected",
    ) &&
    browserMacVmAcceptanceAuditSmoke.includes(
      "leaky_profile_reset_rejected",
    ) &&
    browserMacVmAcceptanceAuditSmoke.includes(
      "stale_restart_proof_rejected",
    ) &&
    browserMacVmAcceptanceAuditSmoke.includes(
      "missing_handoff_summary_rejected",
    ) &&
    browserMacVmAcceptanceAuditSmoke.includes(
      "mismatched_handoff_summary_rejected",
    ) &&
    browserMacVmAcceptanceAuditSmoke.includes(
      "mismatched_auth_receipt_rejected",
    ) &&
    browserMacVmAcceptanceAuditSmoke.includes(
      "receipt_after_proof_rejected",
    ) &&
    browserMacVmAcceptanceAuditSmoke.includes(
      "summary_before_proof_rejected",
    ) &&
    browserMacVmAcceptanceAuditSmoke.includes(
      "matched_authenticated_manual_accepted",
    ) &&
    browserMacVmAcceptanceAuditSmoke.includes(
      "resized_authenticated_manual_accepted",
    ) &&
    read("docs/MAC.md").includes(
      "recomputes the receipt SHA-256 from that receipt path",
    ) &&
    currentState.includes(
      "recomputes the receipt SHA-256 from the receipt path",
    ) &&
    currentState.includes("rejects auth setup receipts generated after") &&
    read("docs/MAC.md").includes("requires the setup receipt timestamp") &&
    read("docs/MAC.md").includes(
      "no later than the machine proof timestamp",
    ) &&
    read("docs/MAC.md").includes("ELASTOS_BROWSER_MAC_VM_EXPECT_VIEWPORT_WIDTH=1000") &&
    read("docs/MAC.md").includes("virtual-auth Browser open request") &&
    currentState.includes("drive the virtual-auth Browser open viewport by default") &&
    read("docs/MAC.md").includes("removed_profile_disk=true") &&
    currentState.includes("removed_profile_disk=true") &&
    browserMacVmAcceptanceHandoff.includes("vm_control_restart") &&
    browserMacVmAcceptanceHandoff.includes("acceptance_ready") &&
    browserMacVmAcceptanceHandoff.includes("remaining_acceptance_gaps") &&
    browserMacVmAcceptanceHandoffSmoke.includes(
      "acceptance_ready_false_visible",
    ) &&
    browserMacVmAcceptanceHandoff.includes("--restart-source-home") &&
    browserMacVmAcceptanceHandoff.includes("--auth-setup-receipt") &&
    browserMacVmAcceptanceHandoff.includes("--source-home-restart-receipt") &&
    browserMacVmAcceptanceHandoff.includes("--handoff-summary") &&
    browserMacVmAcceptanceHandoff.includes('--handoff-summary "$summary_out"') &&
    browserMacVmAcceptanceHandoff.includes("auth_setup_receipt") &&
    browserMacVmAcceptanceHandoff.includes("Boolean(authSetupReceiptPath)") &&
    browserMacVmAcceptanceHandoff.includes("const authSetupReady = authSetupReceiptOk && persistentProfile") &&
    browserMacVmAcceptanceHandoff.includes("source_home_restart") &&
    browserMacVmAcceptanceHandoff.includes("browser-mac-vm-manual-review-packet.mjs") &&
    browserMacVmAcceptanceHandoff.includes(
      "ELASTOS_BROWSER_MAC_VM_PROFILE_RESET_PROOF",
    ) &&
    browserMacVmAuthProfileSetup.includes(
      "elastos.browser.mac-vm-auth-profile-setup/v1",
    ) &&
    homeVirtualAuthSmoke.includes(
      "elastos.home.virtual-authenticator-credentials/v1",
    ) &&
    homeVirtualAuthSmoke.includes("WebAuthn.getCredentials") &&
    homeVirtualAuthSmoke.includes("WebAuthn.addCredential") &&
    homeVirtualAuthSmoke.includes("chmodSync(VIRTUAL_AUTH_CREDENTIAL_STORE, 0o600)") &&
    read("docs/MAC.md").includes(
      "owner-only virtual authenticator credential store",
    ) &&
    currentState.includes("owner-only local file") &&
    homeVirtualAuthSmoke.includes("HOME_VIRTUAL_AUTH_BROWSER_UI_SETUP") &&
    homeVirtualAuthSmoke.includes("holdBrowserUiForSetup") &&
    browserMacVmAuthProfileSetup.includes("HOME_VIRTUAL_AUTH_BROWSER_UI_SETUP=1") &&
    !browserMacVmAuthProfileSetup.includes("HOME_VIRTUAL_AUTH_BROWSER_OPEN=1") &&
    !browserMacVmAuthProfileSetup.includes("HOME_VIRTUAL_AUTH_BROWSER_DIAGNOSTICS=1") &&
    browserMacVmAuthProfileSetup.includes("--receipt-out") &&
    browserMacVmAuthProfileSetup.includes("--auth-setup-receipt") &&
    browserMacVmAuthProfileSetup.includes("setup_only_not_authentication_proof") &&
    browserMacVmAcceptanceAudit.includes("must not claim ela.city authentication by itself") &&
    browserMacVmAuthProfileSetupSmoke.includes("receipt_checked") &&
    browserMacVmAuthProfileSetupSmoke.includes("visible Browser UI setup path") &&
    browserMacVmAuthProfileSetupSmoke.includes("invalid_hold_rejected") &&
    browserMacVmAuthProfileSetupSmoke.includes(
      "elastos.browser.mac-vm-auth-profile-setup.dry-run/v1",
    ) &&
    browserMacVmAcceptanceHandoffSmoke.includes(
      "auth_setup_receipt_mismatch_rejected",
    ) &&
    browserMacVmAcceptanceAuditSmoke.includes("ambiguous_auth_receipt_rejected") &&
    browserMacVmAcceptanceHandoffSmoke.includes(
      "matched auth setup receipt must satisfy the final audit receipt chain",
    ) &&
    browserMacVmAcceptanceHandoffSmoke.includes(
      "handoff final audit must expose the verified auth setup receipt",
    ) &&
    browserMacVmAcceptanceHandoffSmoke.includes("unauth_handoff_status") &&
    browserMacVmAcceptanceHandoffSmoke.includes("handoff-failed until auth setup is bound") &&
    browserMacVmAcceptanceHandoffSmoke.includes("source_home_restart_receipt_checked") &&
    read("docs/MAC.md").includes("exits non-zero until a headed auth setup receipt") &&
    currentState.includes("exits non-zero until the headed auth setup receipt") &&
    macSourceHomeRestart.includes("elastos.mac-source-home-restart/v1") &&
    macSourceHomeRestart.includes("served_index_sha256") &&
    macSourceHomeRestart.includes("installed_index_sha256") &&
    macSourceHomeRestart.includes("source_index_sha256") &&
    macSourceHomeRestart.includes("verify_browser_helper_freshness") &&
    macSourceHomeRestart.includes("browser_helper_rootfs_sha256") &&
    macSourceHomeRestart.includes("Mac source-home Browser helper verification failed") &&
    macSourceHomeRestartSmoke.includes("invalid_addr_rejected") &&
    macSourceHomeRestartSmoke.includes("browser_helper_freshness_gate_present") &&
    read("docs/MAC.md").includes("scripts/mac-source-home-restart.sh") &&
    currentState.includes("scripts/mac-source-home-restart.sh") &&
    browserPlanningSurface.includes("browser-manual-ux-report.mjs") &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "browser-manual-ux-report.mjs",
    ) &&
    read("docs/MAC.md").includes(
      "scripts/browser-mac-vm-manual-ux-smoke.sh",
    ) &&
    read("docs/MAC.md").includes(
      "scripts/browser-mac-vm-acceptance-audit-smoke.sh",
    ),
  "Browser manual UX evidence must have a template/validator with machine artifact hashing, shared hosted WebRTC audio checks, Mac VM evidence checks, a fail-closed Mac VM acceptance audit, and stay wired into the completion gate docs",
);
assert(
  remoteCarrierExitReadiness.includes("config_sha256") &&
    remoteCarrierExitReadiness.includes("sha256File(args.sourceConfig)") &&
    remoteCarrierExitReadinessSmoke.includes("hash-bound to source and exit configs") &&
    remoteCarrierExitSourceConfig.includes("source_config_sha256") &&
    remoteCarrierExitSourceConfig.includes("exit_config_sha256") &&
    remoteCarrierExitSourceConfigSmoke.includes("readiness hashes must match") &&
    currentState.includes("hash-bound remote route readiness") &&
    read("TASKS.md").includes("Compose Inspector, Carrier-only authority") &&
    read("TASKS.md").includes("route-readiness, operator evidence, Browser handoff"),
  "Remote Carrier Exit readiness must remain hash-bound without requiring private goal-completion meta tooling",
);
assert(
  browserExperimentCleanup.includes("elastos.browser.experiment-cleanup/v1") &&
    browserExperimentCleanup.includes("dry_run: !args.apply") &&
    browserExperimentCleanup.includes("1x1x24") &&
    browserExperimentCleanup.includes("elastos-selkies-runtime-exit-target-") &&
    browserExperimentCleanup.includes("running_containers_preserved") &&
    !browserExperimentCleanup.includes("docker rm -f") &&
    browserPlanningSurface.includes("browser-experiment-cleanup.mjs") &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "browser-experiment-cleanup.mjs",
    ),
  "Browser experiment cleanup must stay dry-run by default, preserve running Selkies targets, and avoid force-removing active containers",
);
assert(
  read("elastos/tools/browser-playwright-engine/src/supervisor.mjs").includes(
    "Page.startScreencast",
  ) &&
    read("elastos/tools/browser-playwright-engine/src/supervisor.mjs").includes(
      'DISPLAY_BACKEND = "cdp_screencast_i420"',
    ) &&
    read("elastos/tools/browser-playwright-engine/src/supervisor.mjs").includes(
      'DISPLAY_BACKEND_CLASS = "proof_surface"',
    ) &&
    read("elastos/tools/browser-playwright-engine/src/supervisor.mjs").includes(
      "startRuntimeProxy",
    ) &&
    !read(
      "elastos/tools/browser-playwright-engine/src/supervisor.mjs",
    ).includes("page.route(") &&
    read("elastos/tools/browser-playwright-engine/src/supervisor.mjs").includes(
      "webrtc_remote_display",
    ) &&
    read("elastos/tools/browser-playwright-engine/src/supervisor.mjs").includes(
      "config_fingerprint",
    ) &&
    read("elastos/tools/browser-playwright-engine/src/supervisor.mjs").includes(
      "supported_display_modes",
    ) &&
    read("elastos/tools/browser-playwright-engine/package.json").includes(
      "@roamhq/wrtc",
    ) &&
    read("scripts/browser-runtime-proxy-smoke.sh").includes(
      'display_backend === "cdp_screencast_i420"',
    ) &&
    read("scripts/browser-runtime-proxy-smoke.sh").includes("audio === false"),
  "Hosted Browser proof must expose an explicit WebRTC remote-display sender, mark CDP screencast as a proof backend, advertise audio=false until real capture exists, use the Runtime proxy path instead of Playwright request interception, and reject stale diagnostic-only daemons instead of using HTTP frames as the product display",
);
assert(
  browserJs.includes('const PRODUCT_DISPLAY_MODE = "webrtc_remote_display"') &&
    browserJs.includes(
      '["webrtc_remote_display", "native_surface"].includes(value)',
    ) &&
    !browserJs.includes(["diagnostic", "frame"].join("_")) &&
    !browserJs.includes(["runtime", "frame"].join("_")) &&
    browserDisplayModeSmoke.includes("elastos.browser.display-mode-smoke/v1") &&
    browserDisplayModeSmoke.includes("display_mode=frame") &&
    !browserDisplayModeSmoke.includes(["diagnostic", "frame"].join("_")) &&
    !browserDisplayModeSmoke.includes(["runtime", "frame"].join("_")) &&
    !read("docs/BROWSER_CAPSULE.md").includes(["diagnostic", "frame"].join("_")) &&
    !read("docs/BROWSER_CAPSULE.md").includes("/api/apps/browser/pages/:page_id/frame") &&
    !read("docs/BROWSER_CAPSULE.md").includes("Playwright Chromium frame/input"),
  "Browser UI must expose only WebRTC/native display modes instead of accepting frame/image rendering as a product display",
);
assert(
  browserEngineAdapter.includes("SelkiesGstreamer") &&
    browserEngineAdapter.includes(
      "engine-offer WebRTC display sessions require initial_offer",
    ) &&
    browserEngineAdapter.includes("elastos.browser.webrtc-answer/v1") &&
    browserJs.includes('displaySession.offerer === "engine"') &&
    browserJs.includes("initial_offer") &&
    browserJs.includes('type: "answer"') &&
    gatewayBrowserApi.includes(
      '"schema": "elastos.browser.webrtc-answer/v1"',
    ) &&
    browserHostedProductOperatorConfig.includes("selkies_gstreamer") &&
    browserHostedProductOperatorConfig.includes("selkies_gstreamer_webrtc") &&
    browserHostedProductOperatorConfig.includes("audio_required") &&
    browserHostedProductOperatorConfig.includes("control_socket_path") &&
    browserHostedProductSupervisor.includes(
      "elastos.browser.hosted-product.open/v1",
    ) &&
    browserHostedProductSupervisor.includes("product_compositor") &&
    browserHostedProductSupervisor.includes(
      "hosted product display session must advertise video=true",
    ) &&
    browserHostedProductSupervisor.includes(
      "hosted product display session must report audio availability",
    ) &&
    browserHostedProductSupervisor.includes(
      "hosted product audio sessions must include an audio media section",
    ) &&
    browserHostedProductSupervisor.includes("offerer=engine") &&
    browserHostedProductSupervisor.includes("cdp_screencast_i420") &&
    browserSelkiesControlService.includes(
      "ELASTOS_BROWSER_SELKIES_CONTROL_CONFIG",
    ) &&
    browserSelkiesControlService.includes(
      "browser_control.kind=cdp_http is required",
    ) &&
    browserSelkiesControlService.includes(
      "browser_control.endpoint must be loopback/private",
    ) &&
    browserSelkiesControlService.includes(
      "basic_auth.user must be a non-empty string",
    ) &&
    browserSelkiesControlService.includes("Authorization: Basic") &&
    browserSelkiesControlService.includes(
      "browser CDP navigation did not return a page debugger URL",
    ) &&
    browserSelkiesControlService.includes("HELLO client") &&
    browserSelkiesControlService.includes("SESSION server") &&
    browserSelkiesControlServiceSmoke.includes("HELLO 1") &&
    browserSelkiesControlServiceSmoke.includes("base64") &&
    browserSelkiesControlService.includes('offerer: "engine"') &&
    browserSelkiesControlService.includes("elastos.browser.webrtc-answer/v1") &&
    browserSelkiesControlServiceSmoke.includes("fake-selkies") &&
    browserSelkiesControlServiceSmoke.includes("fake-cdp") &&
    browserSelkiesControlServiceSmoke.includes(
      "browser-selkies-target-preflight.sh",
    ) &&
    browserSelkiesControlServiceSmoke.includes(
      "scripts/browser-selkies-control-service.mjs",
    ) &&
    browserSelkiesControlServiceSmoke.includes("m=audio") &&
    browserSelkiesControlServiceSmoke.includes(
      "elastos.browser.webrtc-answer/v1",
    ) &&
    browserSelkiesTargetPreflight.includes(
      "browser-selkies-control-service.mjs",
    ) &&
    browserSelkiesTargetPreflight.includes(
      "browser-hosted-product-target-preflight.sh",
    ) &&
    browserSelkiesTargetPreflight.includes(
      "--browser-cdp-endpoint must be loopback/private",
    ) &&
    browserSelkiesTargetPreflight.includes("--selkies-basic-auth-user") &&
    browserSelkiesTargetPreflight.includes("--selkies-basic-auth-password") &&
    browserSelkiesCurrentWheelSmoke.includes(
      "ghcr.io/selkies-project/selkies/py-build:main",
    ) &&
    browserSelkiesCurrentWheelSmoke.includes(
      "browser-selkies-target-preflight.sh",
    ) &&
    browserSelkiesCurrentWheelSmoke.includes("audio-enabled=true") &&
    browserSelkiesRealChromiumSmoke.includes(
      "BROWSER_SELKIES_CHROMIUM_PROGRAM",
    ) &&
    browserSelkiesRealChromiumSmoke.includes(
      "--host-resolver-rules='MAP * ~NOTFOUND, EXCLUDE 127.0.0.1'",
    ) &&
    browserSelkiesRealChromiumSmoke.includes(
      "browser-selkies-target-preflight.sh",
    ) &&
    browserSelkiesRealChromiumSmoke.includes("real_chromium_cdp") &&
    browserSelkiesRuntimeExitTarget.includes("browser-local-exit") &&
    browserSelkiesRuntimeExitTarget.includes("browser-native-proxy-engine") &&
    browserSelkiesRuntimeExitTarget.includes(
      '$repo_root/bin/browser-local-exit',
    ) &&
    browserSelkiesRuntimeExitTarget.includes(
      '$repo_root/bin/browser-native-proxy-engine',
    ) &&
    browserSelkiesRuntimeExitTarget.includes(
      "browser-hosted-product-operator-config.mjs",
    ) &&
    browserSelkiesRuntimeExitTarget.includes(
      "elastos.browser.selkies-runtime-exit-target/v1",
    ) &&
    browserSelkiesRuntimeExitTarget.includes("--profile-dir") &&
    browserSelkiesRuntimeExitTarget.includes("/var/lib/elastos-browser-profile") &&
    browserSelkiesRuntimeExitTarget.includes(".elastos-profile.lock") &&
    browserSelkiesRuntimeExitTarget.includes("profile_persistent: true") &&
    !browserSelkiesRuntimeExitTarget.includes("/tmp/chromium-profile") &&
    browserPerLaunchSelkiesSupervisor.includes("PROFILE_ROOT_ENV") &&
    browserPerLaunchSelkiesSupervisor.includes("DEFAULT_STARTUP_TIMEOUT_MS = 90000") &&
    browserPerLaunchSelkiesSupervisor.includes("readinessDiagnostics(outDir)") &&
    browserPerLaunchSelkiesSupervisor.includes("--profile-dir") &&
    browserPerLaunchSelkiesSupervisorSmoke.includes("ELASTOS_BROWSER_PROFILE_ROOT") &&
    browserPerLaunchSelkiesSupervisorSmoke.includes("profile_persistent === true") &&
    browserSelkiesRuntimeExitSmoke.includes(
      "browser-selkies-runtime-exit-target.sh",
    ) &&
    browserSelkiesRuntimeExitSmoke.includes("--cleanup-after-verify") &&
    setupSourceHome.includes("install_browser_runtime_helpers") &&
    !setupSourceHome.includes("browser-per-launch-selkies-supervisor.mjs") &&
    !setupSourceHome.includes("browser-selkies-runtime-exit-target.sh") &&
    !setupSourceHome.includes("browser-hosted-product-operator-config.mjs") &&
    !setupSourceHome.includes("browser-hosted-product-supervisor.mjs") &&
    setupSourceHome.includes("browser-selkies-control-service.mjs") &&
    setupSourceHome.includes("browser-vm-selkies-start") &&
    setupSourceHome.includes("build Browser VZ engine supervisor") &&
    setupSourceHome.includes("-p elastos-vz --bin browser-vz-engine-supervisor") &&
    setupSourceHome.includes("extract_browser_vm_selkies_start") &&
    setupSourceHome.includes("resolve_browser_vm_native_proxy_source") &&
    setupSourceHome.includes("validate_linux_guest_binary") &&
    setupSourceHome.includes("/opt/elastos/bin/browser-native-proxy-engine") &&
    setupSourceHome.includes("refresh_browser_vm_initrd_control_service") &&
    setupSourceHome.includes("refresh_browser_vm_rootfs_files") &&
    setupSourceHome.includes("ELASTOS_DEBUGFS_BIN") &&
    setupSourceHome.includes("debugfs") &&
    read("scripts/browser-hosted-product-target-preflight.sh").includes(
      "browser-hosted-product-display-smoke.sh",
    ) &&
    read("scripts/browser-hosted-product-target-preflight.sh").includes(
      "hosted product control socket is not available",
    ) &&
    read("scripts/browser-hosted-product-display-smoke.sh").includes(
      "Selkies/GStreamer hosted display must use engine-offer WebRTC negotiation",
    ) &&
    read("scripts/browser-hosted-product-display-smoke.sh").includes(
      "backend_class = product_compositor",
    ) &&
    read("scripts/browser-hosted-product-display-smoke.sh").includes(
      "audio = true",
    ) &&
    read("scripts/browser-hosted-product-config-smoke.sh").includes(
      "browser-hosted-product-target-preflight.sh",
    ) &&
    read("scripts/browser-hosted-product-config-smoke.sh").includes(
      "elastos.browser.hosted-product.open/v1",
    ) &&
    browserPlanningSurface.includes("browser-selkies-control-service.mjs") &&
    browserPlanningSurface.includes(
      "browser-selkies-control-service-smoke.sh",
    ) &&
    browserPlanningSurface.includes("browser-selkies-target-preflight.sh") &&
    browserPlanningSurface.includes("browser-selkies-current-wheel-smoke.sh") &&
    browserPlanningSurface.includes("browser-selkies-real-chromium-smoke.sh") &&
    browserPlanningSurface.includes("browser-selkies-runtime-exit-target.sh") &&
    browserPlanningSurface.includes("browser-selkies-runtime-exit-smoke.sh") &&
    browserPlanningSurface.includes("--selkies-basic-auth-user") &&
    browserPlanningSurface.includes("gst-py-example") &&
    browserPlanningSurface.includes("legacy/numeric signaling flow") &&
    read("docs/BROWSER_CAPSULE.md").includes(
      "Hosted Product Control Service",
    ) &&
    read("docs/BROWSER_CAPSULE.md").includes("browser_control.kind=cdp_http") &&
    read("docs/BROWSER_CAPSULE.md").includes(
      "browser-selkies-target-preflight.sh",
    ) &&
    read("docs/BROWSER_CAPSULE.md").includes(
      "browser-selkies-current-wheel-smoke.sh",
    ) &&
    read("docs/BROWSER_CAPSULE.md").includes(
      "browser-selkies-real-chromium-smoke.sh",
    ) &&
    read("docs/BROWSER_CAPSULE.md").includes(
      "browser-selkies-runtime-exit-target.sh",
    ) &&
    read("docs/BROWSER_CAPSULE.md").includes(
      "browser-selkies-runtime-exit-smoke.sh",
    ) &&
    read("docs/BROWSER_CAPSULE.md").includes("--selkies-basic-auth-password") &&
    read("docs/BROWSER_CAPSULE.md").includes("gst-py-example") &&
    read("docs/BROWSER_CAPSULE.md").includes("legacy/numeric signaling flow") &&
    read("docs/BROWSER_CAPSULE.md").includes(
      "browser-selkies-control-service.mjs",
    ) &&
    read("docs/BROWSER_CAPSULE.md").includes("offerer=engine") &&
    read("docs/BROWSER_CAPSULE.md").includes("POST /pages/{page_id}/webrtc"),
  "Standalone hosted/Selkies Browser protocol tests must stay explicit, while source-home setup refreshes only VM guest rootfs files and not hosted Browser proof runtimes",
);
assert(
  browserEngineAdapter.includes("HostedRemoteBrowser") &&
    browserHostedProductOperatorConfig.includes("--candidate") &&
    browserHostedProductOperatorConfig.includes("candidatePreset") &&
    browserHostedProductOperatorConfig.includes("browserbox_webrtc") &&
    browserHostedProductOperatorConfig.includes("kasm_workspaces_webrtc") &&
    browserHostedProductOperatorConfig.includes("hosted_remote_browser") &&
    browserHostedProductOperatorConfig.includes("--display-backend") &&
    read("scripts/browser-hosted-product-target-preflight.sh").includes(
      "--candidate",
    ) &&
    read("scripts/browser-hosted-product-config-smoke.sh").includes(
      "--candidate browserbox",
    ) &&
    read("scripts/browser-hosted-product-config-smoke.sh").includes(
      "--candidate kasm-workspaces",
    ) &&
    read("scripts/browser-hosted-product-config-smoke.sh").includes(
      "--candidate kasmvnc",
    ) &&
    read("scripts/browser-hosted-product-config-smoke.sh").includes(
      "kasm_url",
    ) &&
    read("scripts/browser-hosted-product-config-smoke.sh").includes(
      "Kasm URL-only rejection",
    ) &&
    browserPlanningSurface.includes("kind=hosted_remote_browser") &&
    browserPlanningSurface.includes(
      "browser-hosted-product-operator-config.mjs",
    ) &&
    browserPlanningSurface.includes("Kasm's `kasm_url`") &&
    read("docs/BROWSER_CAPSULE.md").includes("hosted_remote_browser") &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "--candidate browserbox",
    ) &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "Returning only Kasm's `kasm_url` is not an ElastOS Browser proof",
    ),
  "Hosted Browser product path must allow KasmVNC/BrowserBox-style provider spikes through named candidate presets behind the same product-compositor contract, reject Kasm URL-only adapters, and avoid hardcoding Selkies or relying on hand-matched engine/backend strings",
);
assert(
  read("scripts/browser-hosted-provider-preflight.mjs").includes(
    "elastos.browser.hosted-provider-preflight/v1",
  ) &&
    read("scripts/browser-hosted-provider-preflight.mjs").includes(
      "BROWSERBOX_LICENSE_CONFIRMED",
    ) &&
    read("scripts/browser-hosted-provider-preflight.mjs").includes(
      "KASM_BASE_URL",
    ) &&
    read("scripts/browser-hosted-provider-preflight.mjs").includes(
      "operator_control_socket",
    ) &&
    read("scripts/browser-hosted-provider-preflight.mjs").includes(
      "--artifact-out <hosted-bakeoff.json>",
    ) &&
    read("docs/BROWSER_PROVIDER_BAKEOFF.md").includes(
      "browser-hosted-provider-preflight.mjs",
    ) &&
    browserPlanningSurface.includes(
      "scripts/browser-hosted-provider-preflight.mjs",
    ),
  "Hosted Browser provider bake-off must have a fail-closed preflight for BrowserBox/Kasm prerequisites and return an artifact-producing bake-off next_command before running candidate gates or vendor installers",
);
assert(
  read("scripts/browser-hosted-provider-candidate-smoke.sh").includes(
    "browser-hosted-product-display-smoke.sh",
  ) &&
    read("scripts/browser-hosted-provider-candidate-smoke.sh").includes(
      "browser-hosted-product-webrtc-smoke.sh",
    ) &&
    read("scripts/browser-hosted-provider-candidate-smoke.sh").includes(
      "browser-hosted-product-navigation-smoke.sh",
    ) &&
    read("scripts/browser-hosted-provider-candidate-smoke.sh").includes(
      "browser-hosted-product-wallet-smoke.sh",
    ) &&
    read("scripts/browser-hosted-provider-candidate-smoke.sh").includes(
      "browser-hosted-product-glide-wallet-smoke.sh",
    ) &&
    browserHostedProductNavigationSmoke.includes('command: "navigate"') &&
    browserHostedProductNavigationSmoke.includes('command: "back"') &&
    browserHostedProductNavigationSmoke.includes('command: "forward"') &&
    browserHostedProductNavigationSmoke.includes('command: "reload"') &&
    browserHostedProductNavigationSmokeShell.includes(
      "browser-hosted-product-navigation-smoke.mjs",
    ) &&
    browserPlanningSurface.includes(
      "browser-hosted-provider-candidate-smoke.sh",
    ) &&
    browserPlanningSurface.includes("Runtime/provider navigation") &&
    read("docs/BROWSER_CAPSULE.md").includes(
      "browser-hosted-provider-candidate-smoke.sh",
    ) &&
    read("docs/BROWSER_CAPSULE.md").includes(
      "browser-hosted-product-navigation-smoke.sh",
    ),
  "Hosted Browser provider replacement decisions must have one candidate gate covering product display, Runtime/provider navigation, media/audio quality, wallet bridge, and Glide instead of subjective provider preference",
);
assert(
  browserSelkiesControlService.includes("sendFrame(0xa, frame.payload)") &&
    browserSelkiesControlServiceSmoke.includes("keepalive") &&
    browserSelkiesControlServiceSmoke.includes("masked pong") &&
    read("docs/BROWSER_CAPSULE.md").includes(
      "client-to-server pong frames are masked",
    ),
  "Hosted Selkies control bridge must preserve WebSocket ping/pong keepalive semantics so long media sessions are not torn down by signaling disconnects",
);
assert(
  browserHostedProductWebrtcSmoke.includes("playwright") &&
    browserHostedProductWebrtcSmoke.includes(
      'state.tracks.includes("audio")',
    ) &&
    browserHostedProductWebrtcSmoke.includes(
      'state.tracks.includes("video")',
    ) &&
    browserHostedProductWebrtcSmoke.includes("dataChannelOpen") &&
    browserHostedProductWebrtcSmoke.includes("iceConnectionState") &&
    browserHostedProductWebrtcSmoke.includes("holdMs") &&
    browserHostedProductWebrtcSmokeShell.includes("--hold-ms") &&
    browserHostedProductWebrtcSmokeShell.includes(
      "browser-hosted-product-webrtc-smoke.mjs",
    ) &&
    browserSelkiesRuntimeExitTarget.includes(
      "browser-hosted-product-webrtc-smoke.sh",
    ) &&
    browserSelkiesRuntimeExitTarget.includes("--target-image") &&
    browserSelkiesRuntimeExitTarget.includes("prebuilt_target_image") &&
    browserSelkiesOperatorImageBuild.includes(
      "deploy/browser-selkies-runtime-target/Dockerfile",
    ) &&
    browserSelkiesOperatorDockerfile.includes(
      "selkies-0.0.0.dev0-py3-none-any.whl",
    ) &&
    browserSelkiesOperatorDockerfile.includes("libnss3") &&
    browserSelkiesOperatorDockerfile.includes("xclip") &&
    browserSelkiesOperatorDockerfile.includes("PIXELFLUX_VERSION=1.4.7") &&
    browserSelkiesOperatorDockerfile.includes("ctypes.CDLL") &&
    browserSelkiesOperatorDockerfile.includes("screen_capture_module.so") &&
    browserPlanningSurface.includes(
      "browser-selkies-operator-image-build.sh",
    ) &&
    read("docs/BROWSER_CAPSULE.md").includes(
      "browser-selkies-operator-image-build.sh",
    ),
  "Hosted Browser product path must verify real WebRTC audio/video/input tracks, support a long-session hold gate, and have a controlled prebuilt Selkies target image path with required runtime dependencies and screen-capture import proof",
);
assert(
  homeVirtualAuthSmoke.includes("Promise.allSettled") &&
    homeVirtualAuthSmoke.includes("failedOpen.reason") &&
    homeVirtualAuthSmoke.includes("Browser open smoke could not close Runtime Browser page"),
  "Home virtual-auth Browser open smoke must wait for all concurrent open attempts before cleanup so a failed capacity probe cannot orphan an already-opened page",
);
assert(
  browserSelkiesControlService.includes('input_protocol: "selkies_v1"') &&
    browserSelkiesControlServiceSmoke.includes(
      "Selkies display must declare datachannel selkies_v1 input",
    ) &&
    browserHostedProductWebrtcSmoke.includes(
      'session.input_protocol !== "selkies_v1"',
    ) &&
    browserHostedProductWebrtcSmoke.includes("selkiesMessagesForInput") &&
    browserJs.includes("currentDisplayInputProtocol") &&
    browserJs.includes("selkiesMessagesForInput") &&
    browserJs.includes('displaySession.input_protocol === "selkies_v1"') &&
    browserPlanningSurface.includes("input_protocol=selkies_v1") &&
    read("docs/BROWSER_CAPSULE.md").includes("input_protocol=selkies_v1"),
  "Hosted Selkies product sessions must declare selkies_v1 input and Browser UI must translate input events instead of sending ElastOS JSON to the Selkies-native datachannel",
);
assert(
  browserJs.includes("const requiresRuntimeRoute =") &&
    browserJs.includes('event?.type === "browser_command"') &&
    browserJs.includes('event?.type === "resize"') &&
    browserJs.includes("!requiresRuntimeRoute") &&
    browserJs.includes("navigateAddress") &&
    browserJs.includes('command: "navigate"') &&
    browserJs.includes("function isBrowserErrorUrl") &&
    browserJs.includes("if (isBrowserErrorUrl(currentUrl))") &&
    browserJs.includes("return requestRuntimeOpen(nextUrl);") &&
    browserSelkiesControlService.includes("validateBrowserNavigationUrl") &&
    browserSelkiesControlService.includes("Page.navigate") &&
    browserSelkiesControlService.includes("Page.navigateToHistoryEntry") &&
    browserSelkiesControlService.includes("Page.reload") &&
    browserSelkiesControlService.includes("assertBrowserNavigationSucceeded") &&
    browserSelkiesControlService.includes("assertBrowserStateDidNotLandOnErrorPage") &&
    browserSelkiesControlService.includes("chrome-error://chromewebdata/") &&
    browserSelkiesControlService.includes(
      "Emulation.setDeviceMetricsOverride",
    ) &&
    browserSelkiesControlService.includes('body?.event?.type === "resize"') &&
    browserSelkiesControlServiceSmoke.includes('"type": "browser_command"') &&
    browserSelkiesControlServiceSmoke.includes('"command": "navigate"') &&
    browserSelkiesControlServiceSmoke.includes("ERR_CONNECTION_CLOSED") &&
    browserSelkiesControlServiceSmoke.includes("late Chrome error navigation must retry on a fresh target") &&
    browserSelkiesControlServiceSmoke.includes('"command": "reload"') &&
    browserPlanningSurface.includes("Runtime/provider navigation") &&
    read("docs/BROWSER_CAPSULE.md").includes(
      "commands such as address navigation, back, forward, reload, and",
    ) &&
    read("docs/BROWSER_CAPSULE.md").includes(
      "viewport resize remain Runtime/provider input calls",
    ) &&
    read("docs/BROWSER_CAPSULE.md").includes("private CDP"),
  "Hosted Selkies product navigation and viewport resize commands must stay on the Runtime/provider route and be applied by private CDP instead of disappearing into the Selkies pointer/key datachannel or reopening compositor sessions",
);
assert(
    browserJs.includes("lastPageStatus = status") &&
    browserJs.includes("syncViewFromResponse(status)") &&
    browserJs.includes("PAGE_STATUS_AFTER_INPUT_DELAY_MS") &&
    browserJs.includes("PAGE_STATUS_AFTER_INPUT_FOLLOWUP_DELAYS_MS") &&
    browserJs.includes("pageStatusRefreshTimers = delays.map") &&
    browserJs.includes("schedulePageStatusRefresh") &&
    browserJs.includes("forceAddress: true") &&
    browserJs.includes("(!forceAddress && isAddressEditing())") &&
    browserJs.includes("fast = false") &&
    browserJs.includes("?fast=1") &&
    browserJs.includes("fetchPageStatus({ fast: true })") &&
    browserJs.includes("fetchPageStatus({ history, forceAddress })") &&
    gatewayBrowserApi.includes("browser_app_page_diagnostics") &&
    gatewayBrowserApi.includes("elastos://browser-engine/page/diagnostics") &&
    gatewayApi.includes("/api/apps/browser/pages/:page_id/diagnostics") &&
    gatewayBrowserRouteTests.includes("elastos.browser.page-diagnostics/v1") &&
    gatewayBrowserRouteTests.includes('"broken_image_count"') &&
    browserSelkiesControlService.includes("function cachedBrowserPageState(browserPage)") &&
    browserSelkiesControlService.includes('state_source: fastStatus ? "cache" : "cdp"') &&
	    browserSelkiesControlService.includes("refreshBrowserPageState(") &&
    browserSelkiesControlService.includes("broken_image_count") &&
    browserSelkiesControlService.includes("clickable_elements") &&
    browserSelkiesControlService.includes("top_element") &&
    browserSelkiesControlService.includes("visible_text_samples") &&
    browserSelkiesControlService.includes("dialog_elements") &&
    browserSelkiesControlService.includes("summarizeElement") &&
    homeVirtualAuthSmoke.includes("diagnostics.body.visible_text_samples") &&
    homeVirtualAuthSmoke.includes("diagnostics.body.dialog_elements") &&
    browserSelkiesControlService.includes("viewport_width") &&
    browserSelkiesControlService.includes("direct_network: false") &&
	    browserSelkiesControlServiceSmoke.includes(
	      "status did not refresh CDP URL after datachannel navigation",
	    ) &&
    browserSelkiesControlServiceSmoke.includes("fast page status must be cache-backed") &&
    browserJs.includes("collectWebrtcStats") &&
    browserJs.includes("item.kind || item.mediaType") &&
    browserJs.includes('"framesDecoded" in item') &&
    browserJs.includes("webkitDecodedFrameCount") &&
    browserJs.includes("isAddressEditing()") &&
    browserJs.includes("ADDRESS_EDIT_STALE_MS") &&
    browserHostedProductWebrtcSmoke.includes("webrtc_stats") &&
    browserHostedProductWebrtcSmoke.includes("video_frames_decoded") &&
    browserHostedProductWebrtcSmoke.includes("item.kind || item.mediaType") &&
    browserHostedProductWebrtcSmoke.includes("video_element_decoded_frames") &&
    browserHostedProductWebrtcSmoke.includes("assertQualityGate") &&
    browserHostedProductWebrtcSmoke.includes("assertRemoteViewportResize") &&
    browserHostedProductWebrtcSmoke.includes("resize_gate") &&
    browserHostedProductWebrtcSmokeShell.includes("--resize-width") &&
    browserHostedProductWebrtcSmokeShell.includes("--resize-height") &&
    browserHostedProviderCandidateSmoke.includes(
      "resize_gate: media.resize_gate",
    ) &&
    browserObjectiveAudit.includes(
      "resizeGateAccepted(candidate.resize_gate)",
    ) &&
    browserObjectiveAudit.includes("resizeGateAccepted(youtube.resize_gate)") &&
    browserHostedProductWebrtcSmoke.includes("minVideoFps: 24") &&
    browserHostedProductWebrtcSmoke.includes("minVideoWidth: 1280") &&
    browserHostedProductWebrtcSmoke.includes("quality_gate") &&
    browserPlanningSurface.includes("quality floor") &&
    read("docs/BROWSER_CAPSULE.md").includes("quality floor") &&
    read("docs/BROWSER_CAPSULE.md").includes(
      "rendered video-element counters",
    ) &&
    read("docs/BROWSER_CAPSULE.md").includes(
      "Browser UI pauses page-status polling while the user is actively editing",
    ),
  "Hosted Browser product quality must expose engine history state, refresh address state after datachannel navigation, protect address-bar editing from status polling, and provide measurable WebRTC/video-element stats with enforced media and remote-viewport resize gates instead of stale navigation state, fixed or unproven compositor scale, or subjective quality reports",
);
assert(
  browserSelkiesControlService.includes("readIceServersConfig") &&
    browserSelkiesControlService.includes(
      "ice_servers may contain at most 8 entries",
    ) &&
    browserSelkiesControlService.includes("display_session") &&
    browserSelkiesControlService.includes("publicDisplaySession(page.displaySession)") &&
    browserSelkiesControlService.includes("credential_present") &&
    browserSelkiesControlService.includes(
      "ice_servers: this.config.iceServers",
    ) &&
    browserSelkiesControlServiceSmoke.includes("status lost Runtime media relay proof") &&
    browserSelkiesControlService.includes("function mediaKindsForSdp") &&
    !browserSelkiesControlService.includes("isSelkiesAudioUnavailable") &&
    !browserSelkiesControlService.includes("audio_offer_unavailable") &&
    browserSelkiesControlService.includes("const audioMedia = mediaKindsForSdp(audioSdp)") &&
    browserSelkiesControlService.includes("audio_offer: {") &&
    browserSelkiesControlService.includes("audio: audioMedia.audio") &&
    browserSelkiesControlService.includes("video: media.video") &&
    read("scripts/browser-vm-target-preflight.sh").includes("audio_default_ready") &&
    read("scripts/browser-vm-target-preflight.sh").includes("target and missing audio support fails this preflight") &&
    read("scripts/browser-vm-artifact-preflight.sh").includes("audio_default_ready") &&
    read("scripts/browser-vm-artifact-preflight.sh").includes("rootfs manifest target preflight reports audio_default_ready=false") &&
    read("docs/BROWSER_VM_TARGET.md").includes("Audio is part of the default product VM target") &&
    read("docs/BROWSER_VM_TARGET.md").includes("audio_default_ready=true") &&
    read("docs/BROWSER_VM_TARGET.md").includes("failed product VM artifact") &&
    read("docs/BROWSER_CAPSULE.md").includes("Fresh VM artifacts install") &&
    browserSelkiesControlServiceSmoke.includes("audio-unavailable product display launch unexpectedly succeeded") &&
    browserSelkiesControlServiceSmoke.includes("audio-unavailable launch did not fail with a Selkies audio error") &&
    !browserEngineAdapter.includes("supervisor_accepts_video_only_vm_product_display") &&
    browserEngineAdapter.includes("Browser VM product display sessions must advertise audio=true and video=true") &&
    browserHostedProductWebrtcSmoke.includes(
      "sessionHasAudioOffer",
    ) &&
    browserHostedProductWebrtcSmoke.includes(
      "initialOfferHasAudio",
    ) &&
    browserHostedProductWebrtcSmoke.includes(
      "audio_session: session.audio",
    ) &&
    browserSelkiesControlServiceSmoke.includes(
      "stun:stun.example.invalid:3478",
    ) &&
    browserSelkiesControlServiceSmoke.includes(
      "ICE servers were not propagated",
    ) &&
    browserSelkiesTargetPreflight.includes("--ice-server") &&
    browserSelkiesTargetPreflight.includes("--ice-username") &&
    browserSelkiesTargetPreflight.includes("--ice-credential") &&
    read("scripts/browser-vm-target-preflight.sh").includes(
      "ELASTOS_BROWSER_VM_ICE_SERVERS_JSON",
    ) &&
    read("scripts/browser-vm-target-preflight.sh").includes(
      "webrtc_remote_display requires at least one turn:/turns:",
    ) &&
    read("scripts/browser-vm-target-preflight.sh").includes(
      "/run/elastos/browser-ice-servers.json",
    ) &&
    browserSelkiesRuntimeExitTarget.includes("--ice-server") &&
    browserSelkiesRuntimeExitTarget.includes("ice_servers_configured") &&
    browserPlanningSurface.includes(
      "Operators that need traversal beyond direct host UDP must pass `--ice-server`",
    ) &&
    read("docs/BROWSER_CAPSULE.md").includes("display_session.ice_servers"),
  "Hosted Browser product WebRTC path must propagate explicit operator STUN/TURN/TURNS configuration through the typed display session instead of relying on hidden fallback ICE behavior",
);
assert(
  browserSelkiesRuntimeExitTarget.includes('selkies_encoder="x264enc"') &&
    browserSelkiesRuntimeExitTarget.includes('selkies_framerate="30"') &&
    browserSelkiesRuntimeExitTarget.includes('selkies_video_bitrate="16"') &&
    browserSelkiesRuntimeExitTarget.includes('selkies_width="1920"') &&
    browserSelkiesRuntimeExitTarget.includes('selkies_height="1080"') &&
    browserSelkiesRuntimeExitTarget.includes(
      '\\"--force-device-scale-factor=1.5\\"',
    ) &&
    browserSelkiesRuntimeExitTarget.includes(
      "ELASTOS_SELKIES_INITIAL_RESOLUTION",
    ) &&
    browserSelkiesRuntimeExitTarget.includes(
      "needle = 'resize_display(' + quote + '1920x1080' + quote + ')'",
    ) &&
    browserSelkiesRuntimeExitTarget.includes(
      'selkies_resolution_mode="dynamic"',
    ) &&
    browserSelkiesRuntimeExitTarget.includes("--selkies-resolution-mode") &&
    browserSelkiesRuntimeExitTarget.includes(
      "--is-manual-resolution-mode=false",
    ) &&
    browserSelkiesRuntimeExitTarget.includes("--enable-resize=true") &&
    browserSelkiesRuntimeExitTarget.includes("--selkies-encoder") &&
    browserSelkiesRuntimeExitTarget.includes("selkies_resolution") &&
    read("docs/BROWSER_CAPSULE.md").includes(
      "1920x1080 stream with a stable 1280x720 CSS viewport",
    ),
  "Canonical hosted Browser launcher must default to a tunable H.264 profile with explicit normal-browser viewport scale and remote-resize gating instead of the old JPEG proof profile or unproven zoomed-out CSS surface",
);
assert(
  browserEngineAdapter.includes("MAX_SUPERVISOR_TIMEOUT_MS: u64 = 300_000") &&
    browserEngineAdapter.includes("timeout_ms must be 100-300000") &&
    browserHostedProductOperatorConfig.includes("timeoutMs > 300000") &&
    read("docs/BROWSER_CAPSULE.md").includes(
      "supervisor timeouts up to 300 seconds",
    ),
  "Hosted Browser adapter and operator config must agree on the 300-second supervisor timeout needed for heavier per-launch product sessions",
);
assert(
  browserSelkiesSystemService.includes("elastos-browser-selkies.sh") &&
    browserSelkiesSystemScript.includes(
      "browser-selkies-runtime-exit-target.sh",
    ) &&
    browserSelkiesSystemScript.includes(
      "ELASTOS_BROWSER_SELKIES_ICE_SERVERS",
    ) &&
    browserSelkiesSystemScript.includes(
      "ELASTOS_BROWSER_SELKIES_TARGET_IMAGE",
    ) &&
    browserSelkiesSystemEnv.includes(
      "ELASTOS_BROWSER_SELKIES_TARGET_IMAGE=elastos/browser-selkies-runtime-target:dev",
    ) &&
    read("docs/BROWSER_CAPSULE.md").includes(
      "scripts/system/elastos-browser-selkies.service",
    ) &&
    browserPlanningSurface.includes("elastos-browser-selkies.service") &&
    read("ROADMAP.md").includes(
      "operator-image and durable service wrapper path now exists",
    ),
  "Hosted Browser product target must have a durable systemd operator wrapper around the canonical Selkies launcher, not only one-off shell deployment commands",
);
assert(
  !browserPlanningSurface.includes("transient systemd unit") &&
    !browserPlanningSurface.includes("promoting the transient"),
  "Browser plans must not keep stale transient-service TODOs after the durable Selkies service promotion",
);
assert(
  browserSelkiesControlService.includes(
    "Page.addScriptToEvaluateOnNewDocument",
  ) &&
    browserSelkiesControlService.includes("Runtime.addBinding") &&
    browserSelkiesControlService.includes("__elastosBrowserWalletRuntime") &&
    browserSelkiesControlService.includes("runtime_mediated_eip1193") &&
    browserSelkiesControlService.includes("wallet_switchEthereumChain") &&
    browserSelkiesControlService.includes("walletApprovalPending") &&
    browserSelkiesControlService.includes("waitForCachedWalletApproval") &&
    browserSelkiesControlService.includes("approval_reuse") &&
    browserSelkiesControlService.includes("request_suffix") &&
    browserSelkiesControlService.includes("Runtime wallet bridge proxy is required") &&
    !browserSelkiesControlService.includes("walletRuntimeFetchDirect") &&
    browserSelkiesControlService.includes('signing: "approval_required"') &&
    browserSelkiesControlServiceSmoke.includes(
      "wallet bridge init script was not installed before navigation",
    ) &&
    browserSelkiesControlServiceSmoke.includes("Browser wallet init script is missing approval coalescing marker") &&
    browserHostedProductWalletSmoke.includes("eth_requestAccounts") &&
    browserHostedProductWalletSmoke.includes("wallet_switchEthereumChain") &&
    browserHostedProductWalletSmoke.includes("approval_required") &&
    read("docs/BROWSER_CAPSULE.md").includes(
      "scripts/browser-hosted-product-wallet-smoke.sh",
    ) &&
    browserPlanningSurface.includes("browser-hosted-product-wallet-smoke.sh"),
  "Hosted Browser product path must prove the remote Chromium page receives the constrained Runtime-mediated EIP-1193 bridge while duplicate signature prompts coalesce and signing routes through Wallet/Inbox approval",
);
assert(
  browserHostedProductGlideWalletSmoke.includes("https://glidefinance.io/") &&
    browserHostedProductGlideWalletSmoke.includes("Connect Wallet") &&
    browserHostedProductGlideWalletSmoke.includes(
      "metamask|browser wallet|injected",
    ) &&
    browserHostedProductGlideWalletSmoke.includes("direct_network") &&
    read("docs/BROWSER_CAPSULE.md").includes(
      "scripts/browser-hosted-product-glide-wallet-smoke.sh",
    ) &&
    browserPlanningSurface.includes(
      "browser-hosted-product-glide-wallet-smoke.sh",
    ),
  "Hosted Browser product path must prove the real Glide connect-wallet dapp flow, not only a fixture page with window.ethereum",
);
assert(
  browserJs.includes("closeRuntimePage") &&
    browserJs.includes("/api/apps/browser/pages/") &&
    browserSelkiesControlService.includes("closeActivePages()") &&
    browserSelkiesControlService.includes("single_vm_session: true") &&
    browserSelkiesControlService.includes("pages.size > 0 || lastSessionClosedAt > 0") &&
    browserSelkiesControlService.includes(
      "elastos.browser.selkies-control.status/v1",
    ) &&
    browserSelkiesControlServiceSmoke.includes(
      "post-close Selkies recovery reused a stale CDP target",
    ) &&
    browserSelkiesControlServiceSmoke.includes("replacement-response.json") &&
    browserSelkiesControlServiceSmoke.includes("preserved-page-status.json") &&
    browserSelkiesControlServiceSmoke.includes("multi-status.json") &&
    browserSelkiesControlServiceSmoke.includes("initial-status.json") &&
    browserPlanningSurface.includes("single_vm_session=true") &&
    browserPlanningSurface.includes(
      "elastos.browser.selkies-control.status/v1",
    ) &&
    read("docs/BROWSER_CAPSULE.md").includes("one VM session") &&
    read("docs/BROWSER_CAPSULE.md").includes("multiple Browser page") &&
    read("docs/BROWSER_CAPSULE.md").includes("GET /status") &&
    read("docs/BROWSER_CAPSULE.md").includes(
      "single_vm_session=true",
    ),
  "Browser docs and code must keep one profile-owned VM while allowing multiple page sessions and exposing operator status for that shape",
);
assert(
  browserSelkiesControlService.includes("sessionCooldownMs") &&
    browserSelkiesControlService.includes(
      "page.close();\n          markSessionClosed();",
    ) &&
    browserSelkiesControlServiceSmoke.includes("failed-open-response.json") &&
    browserSelkiesControlServiceSmoke.includes(
      "failed open leaked an active Selkies page",
    ) &&
    browserSelkiesControlServiceSmoke.includes(
      "Selkies control bridge did not close the WebSocket after failed open",
    ),
  "Hosted Selkies control service must clean up failed opens and apply a bounded session cooldown instead of leaving orphaned controllers that poison the next launch",
);
assert(
  read("scripts/browser-runtime-proxy-smoke.sh").includes(
    "BROWSER_SMOKE_URL",
  ) &&
    read("scripts/browser-runtime-proxy-smoke.sh").includes(
      "BROWSER_SMOKE_ALLOWED_HOSTS",
    ),
  "Browser runtime proxy smoke must support real target URL proofs such as Glide without duplicating the smoke script",
);
assert(
  read("scripts/browser-runtime-proxy-smoke.sh").includes(
    "BROWSER_SMOKE_ADDRESS_FAMILY",
  ) &&
    read("scripts/browser-youtube-acceptance-smoke.sh").includes(
      "prefer_ipv4",
    ) &&
    read("docs/BROWSER_CAPSULE.md").includes("address_family"),
  "Browser media smokes and docs must expose explicit Exit address-family policy instead of relying on host resolver order",
);
assert(
  read("elastos/tools/browser-playwright-engine/src/supervisor.mjs").includes(
    "request.referer",
  ) &&
    read("scripts/browser-runtime-proxy-smoke.sh").includes(
      "BROWSER_SMOKE_REFERER",
    ),
  "Browser media smoke must be able to model embedded YouTube referrer identity without adding fake fallback rendering",
);
assert(
  read("elastos/tools/browser-playwright-engine/src/supervisor.mjs").includes(
    "headless: input.headless !== false",
  ) &&
    read("scripts/browser-runtime-proxy-smoke.sh").includes(
      "BROWSER_SMOKE_HEADLESS",
    ),
  "Browser media smoke must support headful/Xvfb verification so YouTube failures can be separated from headless proof limitations",
);
assert(
  read("scripts/browser-youtube-acceptance-smoke.sh").includes(
    "BROWSER_SMOKE_REQUIRE_MEDIA=1",
  ) && read("scripts/browser-youtube-acceptance-smoke.sh").includes("xvfb-run"),
  "Browser YouTube acceptance must remain a real media playback gate, not a URL-load proxy signal",
);
assert(
  read("scripts/browser-native-youtube-smoke.sh").includes(
    "browser-native-proxy-engine",
  ) &&
    read("scripts/browser-native-youtube-smoke.sh").includes(
      "connectOverCDP",
    ) &&
    read("scripts/browser-native-youtube-smoke.sh").includes(
      "audio_decoded_delta",
    ) &&
    read("scripts/browser-native-youtube-smoke.sh").includes(
      "YouTube upstream bot challenge",
    ),
  "Native Browser YouTube smoke must launch Chromium through browser-native-proxy-engine/Runtime Exit and require decoded audio/video, not Playwright launch or URL-only success",
);
assert(
  browserSelkiesRuntimeExitTarget.includes('\\"--kiosk\\"') &&
    browserSelkiesRuntimeExitTarget.includes('\\"--app=about:blank\\"') &&
    browserSelkiesRuntimeExitTarget.includes('\\"--disable-infobars\\"') &&
    browserSelkiesRuntimeExitTarget.includes('\\"--window-position=0,0\\"') &&
    browserSelkiesRuntimeExitTarget.includes(
      '\\"--window-size=$selkies_width,$selkies_height\\"',
    ) &&
    browserSelkiesControlService.includes("/json/list") &&
    browserSelkiesControlService.includes(
      'Page.navigate", { url: "about:blank"',
    ) &&
    browserSelkiesControlService.includes("function closeBrowserTarget") &&
    browserSelkiesControlService.includes("/json/close/") &&
    browser.includes('data-shell-window-fit="fixed"') &&
    browserJs.includes("target.videoWidth || view.width || rect.width") &&
    browserJs.includes("target.videoHeight || view.height || rect.height") &&
    browserJs.includes("browserMediaContentRect(target, width, height)") &&
    browserJs.includes('renderPanel.addEventListener("click"') &&
    browserJs.includes("isMediaClickTarget(event.target)") &&
    browserJs.includes("queueWheelInput") &&
    browserJs.includes("touchPanState") &&
    browserJs.includes("suppressSyntheticClickUntil") &&
    browserJs.includes("bindInputChannel") &&
    browserJs.includes("scheduleViewportResize({ force: true })") &&
    browserJs.includes("function scheduleViewportResize()") &&
    browserJs.includes("lastViewport = viewport;") &&
    !browserJs.includes('type: "resize",') &&
    browserStyle.includes("object-fit: contain;") &&
    browserStyle.includes("object-position: center center;") &&
    browserStyle.includes("touch-action: none;"),
  "Hosted Selkies Browser must stream a content-only app-mode Chromium surface, suppress Chrome-for-Testing infobars, reuse/reset or replace failed kiosk targets, disable Home iframe auto-fit for the dynamic Browser viewport, map input against the actual remote video coordinate space, support touch/pan input, preserve remote display aspect ratio without visual zoom/stretch, and keep WebRTC resize authority in the launch/session contract so users do not see a nested browser or misaligned input",
);
assert(
  shellWindows.includes("function fitLaunchedWindow") &&
    shellWindows.includes("fitWindowToBrowserAspect") &&
    shellWindows.includes("fitWindowToLargestBrowserAspect") &&
    shellWindows.includes("dataset.browserMaximized") &&
    shellWindows.includes(`syncBrowserWindow(entry, launched);
  if (entry.targetId === "browser") {
    fitLaunchedWindow(entry);
  }`) &&
    !shellWindows.includes("prebootBrowserTarget") &&
    !shellWindows.includes("dataset.preboot") &&
    shellWindows.includes(`if (entry.targetId === "browser") {
    fitWindowToBrowserAspect(entry.node);
    rememberWindowRestoreBounds(entry.node);
    return;
  }`) &&
    shellWindows.includes('SINGLE_SESSION_TARGETS = new Set(["people", "inbox", "wallet"])') &&
    !shellWindows.includes('SINGLE_SESSION_TARGETS = new Set(["browser"])') &&
    shellWindows.includes("export function normalizeRestorableSession") &&
    shellWindows.includes("withBrowserInstanceQuery(options)") &&
    shellWindows.includes("activateTargetGroup(targetId)") &&
    shellWindows.includes("restoredSingleSessionTargets") &&
    homeShellRegressionSmoke.includes("People restored more than once") &&
    homeShellRegressionSmoke.includes("Browser should allow multiple restored windows") &&
    shellWindows.includes("height: 804") &&
    shellWindows.includes(
      'if (entry.targetId !== "browser") {\n      installFrameAutoFit',
    ) &&
    shellWindowGeometry.includes("BROWSER_REMOTE_ASPECT_RATIO = 16 / 9") &&
    shellWindowGeometry.includes("browserAspectBoundsForState") &&
    shellWindowGeometry.includes(
      "export function fitWindowToLargestBrowserAspect",
    ) &&
    shellWindowGeometry.includes("node.dataset.browserMaximized") &&
    shellWindowGeometry.includes("browserAspectResizeBounds"),
  "Home Browser windows must lock to the current 16:9 remote compositor aspect, allow multiple Browser instances, keep true singleton handling scoped to People, and must not install generic iframe auto-fit observers that fight the remote display during resize",
);
assert(
  shellWindows.includes("query: normalizedLaunchQuery(entry.launchQuery)") &&
    shellWindows.includes("query: restorableLaunchQuery(targetId, item)") &&
    shellWindows.includes('if (targetId === "browser" && !query.browser_instance)') &&
    shellWindows.includes('const launchQuery = targetId === "browser"') &&
    shellWindows.includes("withBrowserInstanceQuery({ query: options.query }).query") &&
    shellWindows.includes("query: restoredWindow.query") &&
    browserJs.includes("const stalePage = previousPage ? null : rememberedRuntimePage();") &&
    browserJs.includes("await closeRuntimePage(stalePage);") &&
    !browserJs.includes(
      "window.__elastosBrowserReleaseRuntimePage = releaseRuntimePageForUnload;\npublishRuntimePageForHost(null);",
    ),
  "Home must persist Browser window launch query/browser_instance across restore and Browser must not clear remembered runtime page ids before stale-page cleanup runs",
);
assert(
  read("scripts/browser-youtube-acceptance-smoke.sh").includes("dQw4w9WgXcQ") &&
    read("scripts/browser-native-youtube-smoke.sh").includes("dQw4w9WgXcQ") &&
    browserPlanningSurface.includes("not product audio acceptance") &&
    read("docs/BROWSER_CAPSULE.md").includes(
      "not product audio acceptance by themselves",
    ),
  "Browser YouTube media gates must use a fixed fixture while documenting that fixture evidence is not product audio acceptance and arbitrary YouTube URLs may still require an approved Exit/provider route",
);
assert(
  read("scripts/browser-runtime-proxy-smoke.sh").includes(
    "BROWSER_SMOKE_UPSTREAM_HTTP_PROXY",
  ) &&
    read("scripts/browser-youtube-acceptance-smoke.sh").includes(
      "BROWSER_SMOKE_UPSTREAM_HTTP_PROXY",
    ) &&
    read("scripts/browser-native-youtube-smoke.sh").includes(
      "BROWSER_SMOKE_UPSTREAM_HTTP_PROXY",
    ),
  "Browser media smokes must be able to exercise an operator-approved upstream HTTP CONNECT Exit without hand-editing config",
);
assert(
  read("scripts/browser-wallet-bridge-smoke.sh").includes(
    "eth_requestAccounts",
  ) &&
    read("scripts/browser-wallet-bridge-smoke.sh").includes(
      "wallet_switchEthereumChain",
    ) &&
    read("scripts/browser-wallet-bridge-smoke.sh").includes("0x2105") &&
    read("docs/BROWSER_CAPSULE.md").includes(
      "scripts/browser-wallet-bridge-smoke.sh",
    ),
  "Browser wallet bridge must have an actual Browser-page smoke for account discovery and chain switching",
);
assert(
  !read("elastos/tools/browser-playwright-engine/src/supervisor.mjs").includes(
    "ElastOS Browser/0.1",
  ),
  "Browser engine must not force a non-browser user agent that breaks modern sites such as YouTube or dapps",
);
assert(
  browserJs.includes("const expectsAudio = displaySession.audio === true") &&
    browserJs.includes("prepareAudio(expectsAudio)") &&
    browserJs.includes("unlockRemoteAudioFromGesture") &&
    browserJs.includes('nextPeerConnection.addTransceiver("audio"') &&
    browserJs.includes("Remote audio enabled.") &&
    browserJs.includes("arx ${audioBytes}") &&
    browserJs.includes("hasRenderableFrame") &&
    browserDisplayModeSmoke.includes("audio_invariants_checked") &&
    browserDisplayModeSmoke.includes("prepareAudio(expectsAudio)") &&
    browserDisplayModeSmoke.includes(
      "Remote display ready. Click the page to enable audio.",
    ),
  "Browser UI must require explicit display-session audio=true, keep initial remote display playback muted for autoplay policy, wait for a renderable video frame before claiming readiness, then unlock product WebRTC audio on a user gesture, expose debug audio receive bytes, still receive advertised audio tracks, and keep those invariants in the dedicated Browser display smoke",
);
assert(
  browserEngineAdapter.includes(
    "webrtc_remote_display audio requires a product compositor backend",
  ) &&
    browserEngineAdapter.includes(
      "webrtc_proof_surface_cannot_advertise_audio",
    ) &&
    browserEngineAdapter.includes(
      "webrtc_product_compositor_can_advertise_audio",
    ),
  "Browser Engine Adapter must reject proof-surface audio claims while allowing real product compositor WebRTC sessions to advertise audio",
);
assert(
  gatewayBrowserApi.includes("authority_false_proof_missing") &&
    gatewayBrowserApi.includes("invalid_provider_summary") &&
    gatewayBrowserApi.includes("invalid_provider_status") &&
    gatewayBrowserApi.includes(
      "Browser Engine Adapter status omitted direct_network=false proof",
    ) &&
    gatewayBrowserApi.includes(
      "Runtime Net provider status omitted direct_network=false proof",
    ) &&
    gatewayBrowserApi.includes(
      "Browser Exit provider status omitted direct_network=false proof",
    ) &&
    gatewayBrowserRouteTests.includes(
      "test_browser_app_summary_rejects_missing_authority_status_proofs",
    ),
  "Browser summaries must not default missing authority proofs to safe-looking status; net, exit, and browser-engine status must surface invalid_provider_status unless direct_network=false and wallet_injection=false are explicitly proven where applicable",
);
assert(
  gatewayBrowserApi.includes("BrowserProviderResourceCall") &&
    gatewayBrowserApi.includes("provider_response_data") &&
    gatewayBrowserApi.includes("provider_response_error_message") &&
    gatewayBrowserApi.includes("browser_provider_resource_call") &&
    gatewayBrowserApi.includes("BrowserOpenRequest") &&
    gatewayBrowserApi.includes("browser_app_open") &&
    gatewayBrowserApi.includes("create_browser_wallet_transaction_request") &&
    gatewayBrowserApi.includes("browser_engine_summary") &&
    gatewayBrowserApi.includes("browser_net_summary") &&
    gatewayBrowserApi.includes("is_browser_wallet_intent") &&
    gatewayBrowserApi.includes("browser_chain_namespace_network") &&
    gatewayBrowserApi.includes("browser_wallet_bridge_payload") &&
    gatewayBrowserApi.includes("browser_request_origin") &&
    !gatewayApi.includes("struct BrowserProviderResourceCall") &&
    !gatewayApi.includes("fn provider_response_data(") &&
    !gatewayApi.includes("fn browser_app_open(") &&
    !gatewayApi.includes("fn create_browser_wallet_transaction_request(") &&
    !gatewayApi.includes("fn browser_engine_summary(") &&
    !gatewayApi.includes("fn browser_net_summary(") &&
    !gatewayApi.includes("fn is_browser_wallet_intent(") &&
    !gatewayApi.includes("fn browser_chain_namespace_network(") &&
    !gatewayApi.includes("fn browser_wallet_bridge_payload(") &&
    !gatewayApi.includes("fn browser_request_origin("),
  "Browser provider-envelope helpers, route handlers, Browser wallet approval flows, summary helpers, wallet bridge helpers, and DTOs must stay in gateway_browser.rs instead of expanding the public gateway module again",
);
assert(
  gatewayBrowserApi.includes("browser_attach_runtime_stream_path") &&
    gatewayBrowserApi.includes("browser_stream_relay") &&
    gatewayBrowserApi.includes("read_browser_relay_open_line") &&
    gatewayBrowserApi.includes("BROWSER_RUNTIME_RELAY_OPEN_MAX_BYTES") &&
    gatewayBrowserApi.includes("write_all(&open_line)") &&
    gatewayBrowserApi.includes("copy_bidirectional") &&
    gatewayBrowserApi.includes("spawn_browser_runtime_stream_listener") &&
    gatewayBrowserApi.includes("UnixListener") &&
    gatewayBrowserApi.includes("BROWSER_RUNTIME_STREAM_TMP_DIR") &&
    gatewayBrowserApi.includes("validate_browser_stream_receipt") &&
    gatewayBrowserApi.includes("browser_engine_stream_session") &&
    gatewayBrowserApi.includes("browser_visible_stream_session") &&
    gatewayBrowserApi.includes('object.remove("adapter_ipc")') &&
    gatewayBrowserApi.includes('object.remove("relay_ipc")') &&
    gatewayBrowserApi.includes('"adapter_ipc": receipt.get("adapter_ipc")') &&
    gatewayBrowserApi.includes('receipt.get("relay_ipc").filter(|value| !value.is_null())') &&
    gatewayBrowserApi.includes('"relay_ipc".to_string()') &&
    !gatewayApi.includes("fn browser_attach_runtime_stream_path(") &&
    !gatewayApi.includes("fn browser_stream_relay(") &&
    !gatewayApi.includes("fn validate_browser_stream_receipt(") &&
    !gatewayApi.includes("fn browser_engine_stream_session(") &&
    !gatewayApi.includes("fn browser_visible_stream_session(") &&
    gatewayBrowserApi.includes("engine_stream_session_keeps_relay_ipc_for_vm_launch") &&
    gatewayBrowserRouteTests.includes(
      "test_browser_open_runtime_stream_socket_accepts_and_closes_fail_closed",
    ) &&
    gatewayBrowserRouteTests.includes(
      "test_browser_open_runtime_stream_relays_to_exit_ipc_without_host_network",
    ) &&
    gatewayBrowserRouteTests.includes("runtime_stream_path") &&
    gatewayBrowserRouteTests.includes(
      'payload["stream_session"].get("adapter_ipc").is_none()',
    ) &&
    gatewayBrowserRouteTests.includes(
      'payload["stream_session"].get("relay_ipc").is_none()',
    ),
  "Browser open route must allocate private Runtime stream socket paths, relay only to private Exit IPC with typed open handshakes, bind fail-closed without relay, keep relay_ipc for engine launches, and strip adapter_ipc/relay_ipc descriptors from Browser UI responses while keeping stream relay/shaping helpers in gateway_browser.rs",
);
assert(
  browser.includes("https://ela.city/") &&
    browser.includes('id="browser-back"') &&
    browser.includes('id="browser-forward"') &&
    browser.includes('id="browser-refresh"') &&
    browser.includes('id="browser-url"') &&
    !browser.includes("Runtime boundary") &&
    !browser.includes("Last request") &&
    !browser.includes('id="browser-frame"') &&
    !browser.includes("Open outside ElastOS"),
  "Browser capsule must present compact browser chrome without debug/proof panels or host-browser escape hatches",
);
assert(
  browserJs.includes("normalizeUrl") &&
    browserJs.includes("streamTargetForUrl") &&
    browserJs.includes("Only http and https addresses") &&
    browserJs.includes("/api/apps/browser/open") &&
    browserJs.includes("elastos.browser.open-result/v1") &&
    browserJs.includes("Browser could not complete the request.") &&
    browserJs.includes("This page was blocked by your Exit Node settings.") &&
    browserJs.includes("historyEntries") &&
    !browserJs.includes("/api/provider/net/stream") &&
    !browserJs.includes("/api/provider/net/http") &&
    !browserJs.includes("frame.src") &&
    !browserJs.includes("window.open") &&
    !browserJs.includes("window.ethereum") &&
    !browserJs.includes("eth_requestAccounts") &&
    !browserJs.includes("personal_sign") &&
    !browserJs.includes("Remote video path is unavailable") &&
    !browserJs.includes("Runtime frame preview") &&
    !browserJs.includes("showing Runtime frame"),
  "Browser capsule must use the high-level Browser open route without faking EIP-1193 wallet injection, external host browsing, iframe host browsing, or diagnostic frame fallbacks into arbitrary web pages",
);
assert(
  browserStyle.includes(".browser-stage") &&
    browserStyle.includes("@media (max-width: 640px)") &&
    browserStyle.includes("--accent: #d46f24") &&
    !browserStyle.includes(".browser-hero") &&
    !browserStyle.includes(".browser-card"),
  "Browser capsule must have a compact responsive ElastOS-aligned host-adapter UI without proof/debug cards",
);
assert(
  read("components.json").includes('"browser"') &&
    read("components.json").includes('"release_path": "browser.tar.gz"'),
  "Browser capsule must be registered as a platform-independent capsule artifact",
);
assert(
  browserCapsulesApi.includes(
    "general_browser_capsule_assets_remain_cross_origin_isolated_for_home_embedding",
  ) &&
    !browserCapsulesApi.includes(
      'headers_mut().remove("cross-origin-embedder-policy")',
    ),
  "Browser app serving must keep COEP/CORP headers so Home can embed it inside the cross-origin-isolated shell",
);
assert(
  gatewayApi.includes('const BROWSER_CAPSULE_ID: &str = "browser"') &&
    gatewayApi.includes("fn is_home_visible_target(name: &str)") &&
    !sourceBlock(
      gatewayApi,
      "fn is_home_visible_target(name: &str)",
      "Home visible target filter",
    ).includes("BROWSER_CAPSULE_ID") &&
    gatewayTests.includes('browser["attach_kind"], "iframe"') &&
    shellWindows.includes('launched.attach_kind !== "iframe"') &&
    shellWindows.includes("function iframeSandboxForLaunch(launched)") &&
    shellWindows.includes('launched?.target === "browser"') &&
    shellWindows.includes("BROWSER_IFRAME_SANDBOX_EXTRAS") &&
    shellWindows.includes("launched?.target === SYSTEM_APP_ID") &&
    shellWindows.includes("SYSTEM_IFRAME_SANDBOX_EXTRAS") &&
    sourceBlock(
      shellWindows,
      "function iframeSandboxForLaunch(launched)",
      "Home iframe sandbox policy",
    ).includes("COMMON_IFRAME_SANDBOX") &&
    !shellWindows.includes(
      'sandbox="allow-downloads allow-forms allow-modals allow-pointer-lock allow-popups',
    ) &&
    !shellWindows.includes(
      'sandbox="allow-downloads allow-forms allow-modals allow-pointer-lock allow-same-origin allow-scripts allow-top-navigation',
    ) &&
    !shellWindows.includes("reservedTab") &&
    !shellWindows.includes('window.open("about:blank"'),
  "Home must open the Browser capsule as an ElastOS window while Browser networking remains Runtime/Exit mediated, and popup/top-navigation iframe privileges must be scoped to Browser/System instead of granted globally",
);
assert(
  walletMetamask.includes('id="wallet-connect"') &&
    walletMetamask.includes('id="wallet-state"') &&
    walletMetamask.includes('id="wallet-accounts"') &&
    walletMetamask.includes('id="wallet-requests"'),
  "MetaMask must live in a dedicated connector capsule with connected-account visibility",
);
assert(
  !walletMetamask.includes("<h1>MetaMask</h1>") &&
    !walletMetamask.includes("Link MetaMask") &&
    !walletMetamask.includes("Refresh</button>") &&
    !walletMetamask.includes("Accounts linked through MetaMask") &&
    !walletMetamask.includes(
      "Review and sign requests for accounts linked through MetaMask",
    ),
  "MetaMask connector UI must avoid redundant wallet copy and manual refresh chrome",
);
assert(
  walletMetamaskJs.includes("selectedMetaMaskProvider") &&
    walletMetamaskJs.includes("eth_requestAccounts") &&
    walletMetamaskJs.includes("personal_sign") &&
    !walletMetamaskJs.includes("wallet-refresh"),
  "MetaMask connector must own the injected-wallet interaction without stale refresh controls",
);
assert(
  walletMetamaskJs.includes("/api/auth/evm/challenge") &&
    walletMetamaskJs.includes("/api/auth/evm/verify") &&
    walletMetamaskJs.includes("/api/apps/wallet-metamask/wallet/accounts") &&
    walletMetamaskJs.includes("/api/apps/wallet-metamask/wallet/approvals"),
  "MetaMask connector must use runtime wallet-link, account, and approval routes",
);
assert(
  walletMetamaskJs.includes("navigator.clipboard.writeText") &&
    walletMetamask.includes("Connected accounts"),
  "MetaMask connector must show copyable full linked wallet accounts",
);
assert(
  walletMetamaskJs.includes("isManagedWalletRequest") &&
    walletMetamaskJs.includes("managed_btc_p2wpkh") &&
    walletMetamaskJs.includes("isMetaMaskSignableRequest") &&
    walletMetamaskJs.includes('intent !== "bitcoin_bip322_proof"'),
  "MetaMask connector must not show built-in or Bitcoin BIP-322 requests as MetaMask-signable requests",
);
assert(
  walletUnisat.includes('id="wallet-connect"') &&
    walletUnisat.includes('id="wallet-state"') &&
    walletUnisat.includes('id="wallet-accounts"') &&
    walletUnisat.includes('id="wallet-requests"'),
  "UniSat must live in a dedicated connector capsule with connected-account visibility",
);
assert(
  walletUnisatJs.includes('CONNECTOR_ID = "wallet-unisat"') &&
    walletUnisatJs.includes("candidateWindow.unisat") &&
    walletUnisatJs.includes("openTopLevelConnector") &&
    walletUnisat.includes('id="wallet-open-popup"') &&
    walletUnisatJs.includes('signMessage(message, "bip322-simple")') &&
    walletUnisatJs.includes('signMessage(message, "ecdsa")') &&
    walletUnisatJs.includes("bitcoinAddressType") &&
    walletUnisatJs.includes("bitcoin_signed_message") &&
    walletUnisatJs.includes("/api/auth/btc/challenge") &&
    walletUnisatJs.includes("/api/auth/btc/verify") &&
    walletUnisatJs.includes(`/api/apps/\${CONNECTOR_ID}/wallet/accounts`) &&
    walletUnisatJs.includes(`/api/apps/\${CONNECTOR_ID}/wallet/approvals`),
  "UniSat connector must own BIP-322 browser wallet signing while using runtime wallet-link and approval routes only",
);
assert(
  !walletJs.includes("/api/auth/btc/challenge") &&
    !walletJs.includes("/api/auth/btc/verify") &&
    walletJs.includes("/api/apps/wallet/wallet/summary") &&
    walletJs.includes("/api/provider/chain/balance") &&
    walletJs.includes('"bip122:000000000019d6689c085ae165831e93"'),
  "Wallet must provide balances and built-in Bitcoin accounts without manual Bitcoin proof linking",
);
assert(
  wallet.includes("wallet-20260523a") &&
    wallet.includes('id="wallet-send"') &&
    wallet.includes('id="wallet-receive"') &&
    wallet.includes("data-wallet-create-account") &&
    wallet.includes("data-wallet-import-recovery-key") &&
    (wallet.match(/data-wallet-create-account/g) || []).length === 2 &&
    (wallet.match(/data-wallet-import-recovery-key/g) || []).length === 2 &&
    !walletJs.includes("wallet-empty-stack") &&
    !walletJs.includes("Create your first account") &&
    walletJs.includes('"EVM"') &&
    walletJs.includes('"Bitcoin"') &&
    walletJs.includes("Create an EVM account for supported networks.") &&
    !wallet.includes('id="wallet-create-method"') &&
    walletJs.includes("openReceiveFlow") &&
    walletJs.includes("openSendFlow") &&
    walletJs.includes("balance_key") &&
    walletJs.includes("wallet-detail-summary") &&
    walletJs.includes("fundedSendableAccounts"),
  "Wallet must expose Send/Receive plus canonical Accounts/Settings create-import surfaces with cache-busted assets",
);
assert(
  wallet.includes('id="wallet-currency-settings"') &&
    wallet.includes('data-wallet-currency="btc"') &&
    wallet.includes('data-wallet-currency="usd"') &&
    wallet.includes('data-wallet-currency="ela"') &&
    !wallet.includes('data-wallet-currency="ela" hidden') &&
    !wallet.includes('id="wallet-enable-ela-currency"') &&
    !wallet.includes('id="wallet-currency"') &&
    walletJs.includes('readStoredValue(DISPLAY_CURRENCY_STORAGE_KEY, "btc"') &&
    !walletJs.includes("ELA_DISPLAY_ENABLED_STORAGE_KEY") &&
    !walletJs.includes("enableElaDisplayCurrency") &&
    !walletJs.includes("elaCurrencyEnabled") &&
    walletJs.includes("/api/wallet/prices") &&
    gatewayApi.includes("/api/wallet/prices") &&
    gatewayApi.includes("ELASTOS_WALLET_PRICE_SOURCE") &&
    gatewayApi.includes("ELASTOS_WALLET_PRICE_HTTP_APPROVED") &&
    gatewayApi.includes("WALLET_PRICE_POLICY_SCHEMA") &&
    gatewayApi.includes("upsert_wallet_price_http_request") &&
    gatewayTests.includes(
      "test_wallet_price_http_source_requires_explicit_approval",
    ) &&
    gatewayTests.includes("test_wallet_price_source_policy_round_trips"),
  "Wallet pricing currency must be selected in Settings, default to BTC, expose USD and ELA without an enablement gate, and require explicit provider approval for HTTP prices",
);
assert(
  wallet.includes('id="wallet-activity-open"') &&
    wallet.includes('id="wallet-settings-open"') &&
    wallet.includes("wallet-hero-actions") &&
    !wallet.includes("wallet-topbar") &&
    !wallet.includes("wallet-sidebar") &&
    !wallet.includes('id="wallet-pending-ribbon"') &&
    walletJs.includes("renderRequests(reviewRequests, reviewWalletRequestId)") &&
    !walletJs.includes("renderPendingRibbon"),
  "Wallet must keep Activity/Privacy/Settings inside the hero card, route approval requests only through the Requests panel, and avoid separate header/sidebar chrome",
);
assert(
  wallet.includes("wallet-settings-drawer") &&
    wallet.includes("wallet-settings-main") &&
    wallet.includes("wallet-settings-side") &&
    wallet.indexOf('class="wallet-settings-side"') <
      wallet.indexOf('id="wallet-methods"') &&
    !wallet.includes("<h2>Wallet settings</h2>") &&
    !wallet.includes("<h3>Identity</h3>") &&
    !wallet.includes('id="wallet-theme"') &&
    !wallet.includes("data-wallet-theme") &&
    !walletJs.includes("applyStoredTheme") &&
    !walletJs.includes("setTheme") &&
    !walletJs.includes("wallet.theme"),
  "Wallet Settings must only contain wallet-local controls, keep approval methods on the side, and leave global appearance to System",
);
assert(
  !walletJs.includes("native balances ready") &&
    walletJs.includes("No approved price source configured."),
  "Wallet must not expose implementation-count balance copy when price providers are not approved",
);
assert(
  !wallet.includes("wallet-brand") &&
    walletJs.includes("selectedAccountId") &&
    walletJs.includes("wallet-detail-address") &&
    walletJs.includes("wallet-detail-qr") &&
    walletJs.includes('account.proof_type === "siwe"') &&
    walletJs.includes("unavailable: Boolean(payload.unavailable)") &&
    !walletJs.includes("Balances update through approved Runtime providers.") &&
    !walletJs.includes('textNode("code", account.address, "wallet-address")'),
  "Wallet must keep top chrome minimal, map SIWE accounts, preserve price-unavailable state, and reveal one copyable account address without provider jargon or duplicate address text",
);
assert(
  wallet.indexOf('aria-label="Accounts"') <
    wallet.indexOf("data-wallet-create-account") &&
    wallet.indexOf('id="wallet-settings-drawer"') <
    wallet.indexOf('id="wallet-create"') &&
    walletJs.includes("Choose the account type you want to create.") &&
    walletJs.includes("One passkey-controlled account for ESC, Base, and supported EVM networks.") &&
    walletJs.includes("evmChainNamespaces.slice(0, 1)") &&
    walletJs.includes("accountGroupKey") &&
    walletJs.includes("accountNetworkLabel") &&
    walletJs.includes("balanceTargetsForAccounts") &&
    walletJs.includes("accountForAsset") &&
    walletJs.includes("account_ids") &&
    walletJs.includes("accountActionsNode.hidden = accounts.length > 0") &&
    walletJs.includes("onWalletActionClick") &&
    !walletJs.includes("createAccountTile") &&
    !walletStyle.includes(".wallet-create-card") &&
    walletJs.includes("create_new: index === 0") &&
    walletJs.includes("Create an EVM account for supported networks.") &&
    walletJs.includes("dataset.walletAccountMenu") &&
    walletJs.includes("/api/apps/wallet/wallet/default") &&
    walletJs.includes("openRenameAccount") &&
    walletJs.includes('method: "PUT"') &&
    gatewayWalletAppApi.includes('"op": "rename_account"') &&
    gatewayTests.includes("test_wallet_app_can_rename_account") &&
    walletProvider.includes("RenameAccount") &&
    walletProvider.includes("rename_account"),
  "Wallet account creation must be available from the main Wallet surface while rename and card action menus stay provider-backed",
);
assert(
    walletJs.includes("requestFreshPasskeyHomeToken") &&
    walletJs.includes("/api/auth/passkey/authenticate/begin") &&
    walletJs.includes("/api/auth/passkey/authenticate/complete") &&
    walletJs.includes("/recovery-key") &&
    walletJs.includes('schema: "elastos.wallet.recovery-key/v1"') &&
    walletJs.includes("JSON.stringify(recoveryKey, null, 2)") &&
    walletJs.includes("Use the full Wallet recovery key JSON below") &&
    walletJs.includes("Delete account") &&
    walletJs.includes('method: "DELETE"'),
  "Wallet account recovery and deletion must be explicit runtime-backed actions, with passkey-protected importable Wallet recovery key export instead of fake seed phrase copy",
);
assert(
  walletProvider.includes("ExportManagedSecret") &&
    walletProvider.includes("elastos.wallet.recovery-key/v1") &&
    gatewayApi.includes(
      "/api/apps/wallet/wallet/accounts/:account_id/recovery-key",
    ) &&
    gatewayApi.includes("require_fresh_passkey_home_token") &&
    gatewayTests.includes(
      "test_wallet_recovery_key_requires_fresh_passkey_home_token",
    ) &&
    gatewayTests.includes("test_wallet_app_can_delete_managed_account"),
  "Wallet recovery/delete routes must be provider-backed, passkey-gated, and covered by gateway tests",
);
assert(
  walletProviderManifest.includes("rename_account") &&
    walletProviderManifest.includes("export_managed_secret") &&
    walletProviderManifest.includes("wallet.account.renamed") &&
    walletProviderManifest.includes("wallet.recovery_key.viewed") &&
    walletProviderManifest.includes("passkey-gated managed recovery export") &&
    !walletProviderManifest.includes("private keys to app capsules"),
  "wallet-provider manifest must document rename and managed recovery export without claiming impossible zero key display for the Wallet recovery surface",
);
assert(
  wallet.includes(
    '<section id="wallet-account-detail" class="wallet-detail" aria-label="Default account"></section>',
  ) &&
    wallet.indexOf('class="wallet-hero-balance"') <
      wallet.indexOf('id="wallet-delta"') &&
    wallet.indexOf('id="wallet-delta"') <
      wallet.indexOf('id="wallet-account-detail"') &&
    wallet.indexOf('id="wallet-total-balance"') <
      wallet.indexOf('class="wallet-action-row"') &&
    walletJs.includes("renderHeroAccount(allAccounts)") &&
    walletJs.includes("selectedOrDefaultAccount") &&
    walletJs.includes("defaultWalletAccount") &&
    walletJs.includes("latestDefault") &&
    walletJs.includes("wallet-detail-inline") &&
    walletJs.includes(
      'selectedAccountId = selectedAccountId === accountId ? "" : accountId',
    ) &&
    walletJs.includes(
      'getSelectedAccountId() === account.account_id ? "Show default wallet" : "Show in hero"',
    ) &&
    walletJs.includes("clearAccountSelection") &&
    walletJs.includes("is-selected") &&
    walletJs.includes("wallet-detail-qr") &&
    walletJs.includes("/api/wallet/qr") &&
    !walletJs.includes("visibleAccounts") &&
    !walletJs.includes("Selected account ·") &&
    !walletJs.includes("Hide details") &&
    !walletJs.includes("clearAccountFilter") &&
    !walletJs.includes("accountDetailNode.scrollIntoView") &&
    !walletJs.includes("wallet-detail-balance") &&
    !walletJs.includes("wallet-detail-section") &&
    !walletJs.includes("No transactions yet") &&
    !walletJs.includes("closeOverlaySurfaces") &&
    !walletJs.includes("walletPageNode") &&
    sourceBlock(
      walletStyle,
      ".wallet-hero-row {",
      "Wallet hero row style",
    ).includes(
      "grid-template-columns: minmax(240px, 1fr) minmax(160px, 0.6fr) minmax(190px, 220px)",
    ) &&
    sourceBlock(walletStyle, ".wallet-delta {", "Wallet graph style").includes(
      "justify-self: center",
    ) &&
    sourceBlock(
      walletStyle,
      ".wallet-detail {",
      "Wallet detail style",
    ).includes("justify-self: end") &&
    sourceBlock(
      walletStyle,
      "@media (max-width: 780px)",
      "Wallet mobile media",
    ).includes(".wallet-detail {\n    justify-self: center;") &&
    !sourceBlock(
      walletStyle,
      ".wallet-detail {",
      "Wallet detail style",
    ).includes("min-height:") &&
    !sourceBlock(
      walletStyle,
      ".wallet-detail {",
      "Wallet detail style",
    ).includes("border:") &&
    !sourceBlock(
      walletStyle,
      ".wallet-detail {",
      "Wallet detail style",
    ).includes("background:") &&
    !sourceBlock(
      walletStyle,
      ".wallet-detail {",
      "Wallet detail style",
    ).includes("position: fixed") &&
    walletStyle.includes(".wallet-detail-inline") &&
    walletStyle.includes("width: 156px;") &&
    walletStyle.includes("width: 132px;") &&
    walletStyle.includes(
      "grid-template-columns: repeat(auto-fill, minmax(200px, 1fr))",
    ) &&
    walletStyle.includes("min-height: 118px") &&
    !walletStyle.includes(".wallet-address") &&
    !walletStyle.includes(".wallet-detail-close") &&
    !walletStyle.includes(".wallet-detail-balance") &&
    !walletStyle.includes(".wallet-detail-section") &&
    walletStyle.includes(".wallet-account.is-selected"),
  "Wallet hero must keep balance/actions on the left, graph centered, QR/address on the right, center QR on mobile, and denser account cards without separate containers, side sheets, scroll jumps, transactions placeholders, or duplicate balances",
);
assert(
  gatewayApi.includes('WALLET_CAPSULE_ID => "Wallet"') &&
    gatewayApi.includes("fn home_launch_target") &&
    gatewayApi.includes("fn is_home_visible_target") &&
    gatewayApi.includes(
      "WALLET_UNISAT_CAPSULE_ID | WALLET_WALLETCONNECT_CAPSULE_ID",
    ) &&
    gatewayTests.includes(
      "test_wallet_app_can_create_and_summarize_accounts",
    ) &&
    gatewayTests.includes("test_wallet_token_can_read_chain_provider_balance"),
  "Wallet must be the visible product surface while connector capsules remain launchable as approval methods",
);
assert(
  !systemJs.includes("walletConnectorForRequest") &&
    walletUnisatJs.includes('CONNECTOR_ID = "wallet-unisat"') &&
    walletJs.includes('actionButton("Open UniSat"') &&
    !walletJs.includes("Paste Bitcoin wallet signature"),
  "Wallet must route Bitcoin proof requests to the Bitcoin signer connector, not System manual proof or MetaMask",
);
const walletMethodForAccountBlock = sourceBlock(
  walletJs,
  "function methodForAccount",
  "Wallet method mapper",
);
assert(
  walletMethodForAccountBlock.indexOf(
    'connectorId === "wallet-walletconnect"',
  ) > 0 &&
    walletMethodForAccountBlock.indexOf(
      'connectorId === "wallet-walletconnect"',
    ) < walletMethodForAccountBlock.indexOf('account.proof_type === "siwe"'),
  "Wallet method mapper must classify connector id before SIWE proof type so WalletConnect SIWE accounts do not render as MetaMask",
);
assert(
  gatewayTests.includes(
    "test_wallet_connector_approvals_are_scoped_to_connector",
  ) &&
    gatewayApi.includes(
      "request.connector_id.as_deref() == Some(wallet_connector.as_str())",
    ),
  "Connector account/request lists must be scoped to the active connector capsule",
);
assert(
  authWalletSmoke.includes("metamask_connector") &&
    authWalletSmoke.includes("unisat") &&
    authWalletSmoke.includes("wallet_token_cannot_link_bip322_account"),
  "Auth/wallet focus smoke must include MetaMask, UniSat, and the Wallet manual-proof rejection filter",
);
assert(
  !walletMetamask.includes("WalletConnect") &&
    !walletMetamaskJs.includes("walletconnect"),
  "WalletConnect must not be visible until a pinned connector exists",
);
assert(
  !system.includes("<dt>Wallet requests</dt>") &&
    !system.includes("wallet-requests-block") &&
    !system.includes('id="wallet-accounts"') &&
    !system.includes('id="wallet-approvals"'),
  "System wallet approvals must be removed from Advanced after Wallet/Inbox becomes the owner",
);
assert(
  !homeSmoke.includes("Wallet proof") &&
    !homeSmoke.includes("Wallet requests") &&
    homeSmoke.includes("walletControlsRemoved"),
  "Home smoke must track that System no longer owns Wallet/Requests layout",
);
assert(
  !systemSmoke.includes("Wallet proof") &&
    !systemSmoke.includes("Wallet requests") &&
    systemSmoke.includes("walletControlsRemoved"),
  "System smoke must track that Wallet/Requests controls are removed from System",
);
assert(
  homeSmoke.includes("recoveryPasswordPresent") &&
    homeSmoke.includes("Recovery Kit download") &&
    homeSmoke.includes("Recovery Kit import"),
  "Home smoke must cover Recovery Kit controls in the Home-launched System frame",
);
assert(
  systemSmoke.includes("recoveryPasswordPresent") &&
    systemSmoke.includes("Recovery Kit download") &&
    systemSmoke.includes("Recovery Kit import"),
  "System smoke must cover Recovery Kit download/import controls",
);
assert(
  systemSmoke.includes("technicalDetailsPresent") &&
    systemSmoke.includes("System Security must expose Technical Details") &&
    systemSmoke.includes(".technical-inspect-grid") &&
    systemSmoke.includes("legacyInspectorPresent === false") &&
    !systemSmoke.includes("System window is missing Advanced"),
  "System smoke must cover privileged Technical Details and avoid stale Inspector or Advanced-panel expectations",
);
assert(
  homeSmoke.includes("unsigned-launch-prompts-passkey") &&
    homeSmoke.includes("HOME_SMOKE_PRESERVE_SESSION"),
  "Home smoke must treat unsigned app launch as a passkey gate and keep signed journeys explicit",
);
assert(
  homeSmoke.includes("archive-launch-routes-to-library") &&
    homeSmoke.includes("#open-existing-archive") &&
    homeSmoke.includes('archiveWindow.title !== "Archive"') &&
    homeSmoke.includes('archive.title === "Archive - ElastOS"') &&
    homeSmoke.includes('window.target === "library" && window.title === "Library"'),
  "Home smoke must cover Archive's authorized open-target handoff to Library and visible Archive labeling",
);
assert(
  homeVirtualAuthSmoke.includes("WebAuthn.addVirtualAuthenticator") &&
    homeVirtualAuthSmoke.includes("HOME_VIRTUAL_AUTH_ALLOW_REMOTE") &&
    homeVirtualAuthSmoke.includes("http://localhost:8090") &&
    homeVirtualAuthSmoke.includes("/api/auth/passkeys") &&
    homeVirtualAuthSmoke.includes("/api/auth/sessions/refresh") &&
    homeVirtualAuthSmoke.includes("/api/auth/sessions/sign-out") &&
    homeVirtualAuthSmoke.includes("refreshCurrentHomeToken") &&
    homeVirtualAuthSmoke.includes("Home sign-out request failed") &&
    homeVirtualAuthSmoke.includes('"x-elastos-home-token"') &&
    homeVirtualAuthSmoke.includes("/api/apps/home/launch") &&
    homeVirtualAuthSmoke.includes(
      "System should not duplicate Wallet controls",
    ),
  "Home signed-session smoke must use a real CDP WebAuthn virtual authenticator on localhost, refuse remote mutation by default, exercise sign-out/sign-in, launch app-scoped System without human cookies, and catch System/Wallet layout drift",
);
assert(
  gatewayApi.includes("pub(crate) fn home_launch_auth_data_dir") &&
    authGatewayApi.includes("home_launch_auth_data_dir(&state.data_dir)") &&
    authGatewayApi.includes("crate::auth::store_session_grant(&auth_data_dir") &&
    authGatewayApi.includes("crate::auth::revoke_session_grant(&auth_data_dir") &&
    authGatewayApi.includes(
      "refresh_session_uses_trusted_auth_data_dir_for_refreshed_tokens",
    ),
  "Home auth session refresh/sign-out must use the trusted Home-launch auth data root so refreshed tokens validate and revoke against the same authority state",
);
assert(
  homeVirtualAuthSmoke.includes("/revoke") &&
    homeVirtualAuthSmoke.includes("CLEANUP_PASSKEY"),
  "Home virtual passkey smoke must clean up its disposable test credential by default",
);
assert(
  !system.includes('id="wallet-approval-check"') &&
    !systemJs.includes("/api/apps/system/wallet/approvals/check"),
  "System must not expose synthetic wallet approval test affordances",
);
assert(
  system.includes("<dt>Network status</dt>"),
  "System chain reads must be labeled as advanced network status",
);
assert(
  system.includes('id="chain-table"') &&
    systemJs.includes("renderChainTable") &&
    systemStyle.includes(".network-row"),
  "System network status must render a compact provider-backed chain table",
);
assert(
  systemJs.includes('action: "status"') &&
    systemJs.includes("onChainLifecycleAction") &&
    systemJs.includes("lifecycle.control_available === true"),
  "System node lifecycle controls must render only from provider-reported control_available state",
);
assert(
  systemStyle.includes(".network-actions") &&
    systemStyle.includes(".network-action"),
  "System node lifecycle controls must use compact network-row actions instead of separate noisy panels",
);
assert(
  chainProvider.includes('"transaction_type": "eip155_legacy"') &&
    chainProvider.includes("eth_getTransactionCount") &&
    chainProvider.includes("eth_estimateGas"),
  "Chain-provider transaction prepare must produce a signable typed EVM intent without raw RPC exposure",
);
assert(
  read("elastos/crates/elastos-server/src/provider_resource.rs").includes(
    "elastos://chain/{network}/proof/erc1271",
  ),
  "ERC-1271 chain proof must have a narrow capability resource",
);
assert(
  !system.includes('data-field="chain-status"') &&
    !system.includes('data-field="chain-note"') &&
    !systemJs.includes("Chain reads stay inside chain-provider"),
  "System network status must not show redundant provider-policy copy",
);
assert(
  !system.includes('id="chain-refresh"') &&
    systemJs.includes("onChainRowClick") &&
    !systemJs.includes("row.href = `elastos://chain/"),
  "System chain rows must not use sandbox-blocked external protocol navigation",
);
const systemSettingsTabStart = system.indexOf(
  '<section class="settings-content" data-settings="about"',
);
const systemSettingsTab = system.slice(systemSettingsTabStart);
assert(
  systemSettingsTabStart >= 0 &&
    systemSettingsTab.indexOf("<dt>Device identity</dt>") <
      systemSettingsTab.indexOf("<dt>Version</dt>") &&
    systemSettingsTab.indexOf("<dt>Version</dt>") <
      systemSettingsTab.indexOf("<dt>Network status</dt>") &&
    !systemSettingsTab.includes("<dt>Documents</dt>"),
  "System About must order identity and version before Network without a Documents row",
);
assert(
  system.includes('class="settings-container"') &&
    system.includes('class="settings-sidebar disable-user-select disable-context-menu"') &&
    system.includes('class="settings-content-container"') &&
    systemStyle.includes(".settings-sidebar") &&
    !system.includes("settings-sidebar-title") &&
    !systemStyle.includes(".settings-sidebar-title") &&
    systemStyle.includes(".settings-content-container") &&
    systemStyle.includes(".pc2-group-row") &&
    systemJs.includes("activateSettingsTab") &&
    !system.includes("system-hero") &&
    !systemStyle.includes(".system-hero"),
  "System must copy the PC2 Settings shell instead of keeping a bespoke dashboard",
);
assert(
  !systemStyle.includes(".wallet-subsection-title"),
  "System Wallet Requests label styles must be removed with the Wallet/Requests surface",
);
assert(
  system.includes('class="system-inline-row background-actions"'),
  "System background image actions must stay in one row",
);
assert(
  system.includes("background-overlay-panel"),
  "System overlay controls must be integrated into the Background control panel",
);
assert(
  system.indexOf('id="background-preview"') <
    system.indexOf('id="background-overlay"'),
  "System background preview and overlay controls must share one field flow",
);
assert(
  systemStyle.includes(".settings-sidebar.active") &&
    systemStyle.includes(".sidebar-toggle") &&
    systemStyle.includes("@media (max-width: 767.98px)"),
  "System mobile layout must keep the PC2 Settings sidebar toggle",
);
assert(
  systemStyle.includes("aspect-ratio: 16 / 9;"),
  "System background preview must preserve wallpaper proportions",
);
assert(
  systemStyle.includes("min-height: 9rem;") &&
    !systemStyle.includes("max-height: min(13rem, 18dvh);"),
  "System background preview must not collapse into a low strip on desktop",
);
assert(
  !systemStyle.includes(".system-advanced summary") &&
    !system.includes("Runtime, storage, wallets, and networks") &&
    !systemStyle.includes('background-preview[data-empty="true"]::after'),
  "System must reduce Advanced clutter without stale disclosure copy or default wallpaper badge",
);
assert(
  systemStyle.includes(".pc2-section-title") &&
    systemStyle.includes("font-size: 11px;") &&
    systemStyle.includes("text-transform: uppercase;") &&
    systemStyle.includes("background: #f9f9f9;") &&
    systemStyle.includes("border: 1px solid #d0d0d0;"),
  "System Settings must keep PC2 compact section/card styling",
);
assert(
  !system.includes('data-settings="storage"') &&
    !systemStyle.includes(".webspace-list") &&
    system.includes('id="capsule-catalog"') &&
    systemStyle.includes(".capsule-catalog"),
  "System must keep removed Storage/WebSpace UI out and render Apps & Services through the catalog",
);
assert(
  systemStyle.includes(".technical-inspect-list") &&
    systemStyle.includes(".technical-inspect-detail") &&
    systemStyle.includes("max-height: min(30rem, 55vh);") &&
    systemStyle.includes("grid-template-columns: minmax(12rem, 0.36fr) minmax(0, 1fr);"),
  "System Technical Details must bound object lists and keep previews inside the Settings layout",
);

const principles = read("PRINCIPLES.md");
const architecture = read("docs/ARCHITECTURE.md");
const contentAvailabilityDoc = read("docs/CONTENT_AVAILABILITY.md");
const roadmap = read("ROADMAP.md");
const overviewDoc = read("docs/OVERVIEW.md");
const designSystem = read("docs/DESIGN_SYSTEM.md");
const commandMatrix = read("docs/COMMAND_MATRIX.md");
const shellSmoke = homeSmoke;
const runtimeChecklist = read("docs/RUNTIME_REPO_USER_STORY_CHECKLIST.md");
const installDoc = read("docs/INSTALL.md");
const scriptsReadme = read("scripts/README.md");
const publicInstallHomeFrontdoorSmoke = read("scripts/public-install-home-frontdoor-smoke.sh");
const publicInstallIdentitySmoke = read("scripts/public-install-identity-smoke.sh");
assertMarkdownLocalLinksResolve();
assertMarkdownScriptReferencesResolve();
assertOrdinaryCapsulesDoNotReferenceRawBlockchainAuthority();
assert(
    installDoc.includes("## Handoff Verification") &&
    installDoc.includes("just candidate-command-audit") &&
    installDoc.includes("ELASTOS_PUBLISHER_GATEWAY=<candidate-url>") &&
    installDoc.includes("ELASTOS_BIN_OVERRIDE=\"$PWD/elastos/target/release/elastos\"") &&
    installDoc.includes("0.5.0-compatible manifest") &&
    installDoc.includes("home profile and checksummed artifacts") &&
    installDoc.includes("scripts/local-carrier-setup-smoke.sh") &&
    installDoc.includes("scripts/public-install-identity-smoke.sh") &&
    installDoc.includes("scripts/public-install-home-frontdoor-smoke.sh") &&
    installDoc.includes("Final public install path after publishing 0.5.0") &&
    installDoc.includes("ELASTOS_PUBLIC_INSTALL_FORCE_RELAY_ONLY=1") &&
    installDoc.includes("--min-active-crosvm-seconds 3600") &&
    installDoc.includes("manual installed-device check is still separate") &&
    installDoc.includes("Source-home and seed-node proofs do not replace this") &&
    installDoc.includes("installed-path check") &&
    runtimeChecklist.includes("## 0.5.0 Handoff Order") &&
    runtimeChecklist.includes("just candidate-command-audit") &&
    runtimeChecklist.includes("ELASTOS_PUBLISHER_GATEWAY=<candidate-url>") &&
    runtimeChecklist.includes("ELASTOS_BIN_OVERRIDE=\"$PWD/elastos/target/release/elastos\"") &&
    runtimeChecklist.includes("0.5.0-compatible manifest") &&
    runtimeChecklist.includes("home profile and checksummed artifacts") &&
    runtimeChecklist.includes("scripts/local-carrier-setup-smoke.sh") &&
    runtimeChecklist.includes("scripts/public-install-identity-smoke.sh") &&
    runtimeChecklist.includes("scripts/public-install-home-frontdoor-smoke.sh") &&
    runtimeChecklist.includes("Final public install path after publishing 0.5.0") &&
    runtimeChecklist.includes("ELASTOS_PUBLIC_INSTALL_FORCE_RELAY_ONLY=1") &&
    runtimeChecklist.includes("--min-active-crosvm-seconds 3600") &&
    runtimeChecklist.includes("Do not") &&
    runtimeChecklist.includes("count source-home or seed-node proof as installed-host acceptance") &&
    scriptsReadme.includes("ELASTOS_BIN_OVERRIDE=<path-to-branch-elastos>") &&
    scriptsReadme.includes("0.5.0-compatible manifest") &&
    scriptsReadme.includes("current `home` setup profile") &&
    scriptsReadme.includes("checksummed artifacts") &&
    includesNormalized(scriptsReadme, "pin the installer-selected components manifest") &&
    includesNormalized(scriptsReadme, "source checkout metadata cannot leak") &&
    scriptsReadme.includes("scripts/local-carrier-setup-smoke.sh") &&
    scriptsReadme.includes("ELASTOS_PUBLIC_INSTALL_FORCE_RELAY_ONLY=1") &&
    scriptsReadme.includes("stricter publisher relay-health check") &&
    publicInstallHomeFrontdoorSmoke.includes("guard_branch_binary_requires_checksummed_public_manifest") &&
    publicInstallHomeFrontdoorSmoke.includes("ELASTOS_COMPONENTS_MANIFEST") &&
    read("scripts/lib/public-install-guards.sh").includes("current 'home' setup profile") &&
    publicInstallHomeFrontdoorSmoke.includes('FORCE_RELAY_ONLY="${ELASTOS_PUBLIC_INSTALL_FORCE_RELAY_ONLY:-0}"') &&
    publicInstallHomeFrontdoorSmoke.includes('"$RUN_BIN" setup >/tmp/elastos-public-home-setup.log') &&
    !publicInstallHomeFrontdoorSmoke.includes("setup --profile home") &&
    publicInstallIdentitySmoke.includes("guard_branch_binary_requires_checksummed_public_manifest") &&
    publicInstallIdentitySmoke.includes("ELASTOS_COMPONENTS_MANIFEST") &&
    publicInstallIdentitySmoke.includes('FORCE_RELAY_ONLY="${ELASTOS_PUBLIC_INSTALL_FORCE_RELAY_ONLY:-0}"') &&
    publicInstallIdentitySmoke.includes('"${RUN_BIN}" setup >/tmp/elastos-public-identity-setup.log') &&
    !publicInstallIdentitySmoke.includes("setup --profile home") &&
    currentState.includes("pin the installer-selected components manifest") &&
    currentState.includes("source checkout `components.json` from leaking") &&
    currentState.includes("lacks the current `home` setup profile") &&
    currentState.includes("Branch-override public smokes require a staged or published 0.5.0-compatible") &&
    includesNormalized(currentState, "source/local Carrier setup proof stays in") &&
    includesNormalized(
      currentState,
      "require a staged or published 0.5.0-compatible manifest with the current `home` profile",
    ) &&
    currentState.includes("ELASTOS_PUBLIC_INSTALL_FORCE_RELAY_ONLY=1") &&
    currentState.includes("Final public installed-path proof waits for publishing the 0.5.0") &&
    tasks.includes("Keep source/local Carrier setup proof green") &&
    tasks.includes("Candidate public install proof with the branch binary needs a staged or") &&
    tasks.includes("0.5.0-compatible manifest") &&
    tasks.includes("current `home` profile") &&
    tasks.includes("scripts/local-carrier-setup-smoke.sh") &&
    tasks.includes("publish the 0.5.0 binary/artifact set so no-override public installed-path smokes use current code"),
  "Install and runtime checklist docs must preserve the branch, public install, candidate gateway, target closeout, and manual installed-device handoff boundaries",
);
assert(
  !tasks.includes("- [x]"),
  "TASKS.md must contain open work only; completed work belongs in elastos/CHANGELOG.md",
);
const productionStorageTaskLines =
  tasks.match(/^- \[ \] BLOCKER - production multi-peer availability\/storage markets .+$/gm) ??
  [];
assert(
  productionStorageTaskLines.length === 1 &&
    productionStorageTaskLines[0].includes(
      "BLOCKER - production multi-peer availability/storage markets require real external infrastructure before this can close",
    ) &&
    productionStorageTaskLines[0].includes(
      "production independent provider-network quota-ledger federation beyond the configured bounded endpoint quorum",
    ) &&
    productionStorageTaskLines[0].includes(
      "production network-wide abuse throttles/banlists/abuse ledgers beyond the configured bounded abuse-control endpoint quorum",
    ) &&
    productionStorageTaskLines[0].includes(
      "production cross-runtime peer reputation trust policy, third-party attestations, revocation, and fleet-wide reputation exchange beyond the configured Carrier peer-attestation endpoint quorum",
    ) &&
    productionStorageTaskLines[0].includes(
      "production storage-market offer/pricing/SLA execution beyond the configured storage-market endpoint-quorum admission gate",
    ) &&
    productionStorageTaskLines[0].includes(
      "repair-fleet worker attestation/SLA/settlement beyond configured dispatch quorum",
    ),
  "TASKS.md must keep exactly one open production multi-peer availability/storage infrastructure blocker",
  productionStorageTaskLines,
);
assert(
  !tasks.includes("branch-local availability/storage foundations") &&
    !tasks.includes("signed bounded remote admission preflight") &&
    !tasks.includes("configured federated quota-ledger exchange") &&
    !tasks.includes("configured federated abuse-control exchange") &&
    !tasks.includes("configured Carrier peer-attestation exchange") &&
    !tasks.includes("configured external storage-market admission") &&
    !tasks.includes("configured external repair-fleet dispatch"),
  "TASKS.md must not re-list completed branch-local availability/storage proof slices as open work",
);
assert(
  contentAvailabilityDoc.includes("This is") &&
    contentAvailabilityDoc.includes(
      "provider-mediated autonomous cross-peer repair for announced Carrier peers",
    ) &&
    contentAvailabilityDoc.includes(
      "not yet a complete global storage market",
    ) &&
    contentAvailabilityDoc.includes(
      "External availability providers still own production peer admission across",
    ) &&
    contentAvailabilityDoc.includes(
      "independent provider networks",
    ) &&
    productionStorageTaskLines[0].includes(
      "production multi-peer availability/storage markets require real external infrastructure before this can close",
    ),
  "Durable docs must distinguish completed branch-local proof work from the open production infrastructure blocker",
);
assert(
  contentAvailabilityDoc.includes("production independent provider-network") &&
    contentAvailabilityDoc.includes(
      "quota-ledger federation beyond the configured bounded endpoint quorum",
    ) &&
    contentAvailabilityDoc.includes(
      "federated network abuse throttles/banlists/abuse ledgers beyond the configured",
    ) &&
    contentAvailabilityDoc.includes("bounded abuse-control endpoint quorum") &&
    contentAvailabilityDoc.includes(
      "configured Carrier peer-attestation endpoint quorum",
    ) &&
    contentAvailabilityDoc.includes("production storage-market admission/execution") &&
    contentAvailabilityDoc.includes("current signed") &&
    contentAvailabilityDoc.includes("admission proof path") &&
    contentAvailabilityDoc.includes("live") &&
    contentAvailabilityDoc.includes("settlement/escrow execution"),
  "Content availability docs must keep a hard production-infrastructure gate so local proof/status shims cannot be mistaken for production completion",
);
assert(
  contentAvailabilityDoc.includes(
    "Optional federated quota-ledger exchange",
  ) &&
    contentAvailabilityDoc.includes(
      "Optional federated abuse-control exchange",
    ) &&
    contentAvailabilityDoc.includes(
      "Optional Carrier peer-attestation exchange",
    ) &&
    contentAvailabilityDoc.includes(
      "Optional storage-market endpoint-quorum admission gate",
    ) &&
    contentAvailabilityDoc.includes("Optional external repair-fleet dispatch") &&
    tasks.includes(
      "repair-fleet worker attestation/SLA/settlement beyond configured dispatch quorum",
    ) &&
    namespacesDoc.includes("explicit capability keys"),
  "Durable docs must record quota-ledger, abuse-control, storage-market, Carrier peer-attestation, repair-fleet, and capability-key gates",
);
assert(
  !runtimeChecklist.includes("Shared is useful"),
  "Runtime checklist must not point reviewers at the retired Shared Home app",
);
assert(
  !runtimeChecklist.includes("public-install-update-smoke.sh"),
  "Runtime checklist must not reference retired public install update proof script",
);
assert(
  !runtimeChecklist.includes("public-linux-runtime-portability-smoke.sh"),
  "Runtime checklist must not reference retired public Linux portability proof script",
);
assert(
  principles.includes(
    "every visible user action should map to the same capability-scoped operation",
  ),
  "Principles must define human/agent action equality",
);
assert(
  architecture.includes("Interaction equality is part of the same rule"),
  "Architecture must define interaction equality",
);
assert(
  ![architecture, roadmap, namespacesDoc, overviewDoc].some((doc) =>
    doc.includes("localhost://Users/self"),
  ),
  "Canonical namespace docs must present principal-root storage, not shared Users/self examples",
);
assert(
  designSystem.includes(
    "Every visible action must have the same contract for humans and agents",
  ),
  "Design system must define human/agent interaction contract",
);
assert(
  commandMatrix.includes(
    "Host-side provider bridge commands are explicit operator tooling, not app-capsule authority.",
  ),
  "Command matrix must distinguish host provider tooling from app-capsule authority",
);
assert(
  !commandMatrix.includes("direct IPFS bridge"),
  "Command matrix must not normalize direct IPFS bridge language",
);
assert(
  !commandMatrix.includes("IPFS bridge"),
  "Command matrix must describe ipfs-provider explicitly, not an ambiguous IPFS bridge",
);
const commandRules =
  commandMatrix.match(/## Rules\n([\s\S]*?)\n## Future:/)?.[1] || "";
const commandRuleNumbers = [...commandRules.matchAll(/^(\d+)\./gm)].map(
  (match) => Number(match[1]),
);
assert(
  commandRuleNumbers.length > 0,
  "Command matrix must keep a numbered Rules section",
);
for (const [index, number] of commandRuleNumbers.entries()) {
  assert(
    number === index + 1,
    "Command matrix ordered rules must stay sequential",
    commandRuleNumbers,
  );
}
assert(
  !/!\s*doc\.querySelector\([^)]*\)\?\.classList\.contains/.test(shellSmoke),
  "Smoke checks must not treat missing DOM nodes as visible with !optional chaining",
);
assertProviderOperationEnumsRejectUnknownFields();
assertGatewayRequestStructsRejectUnknownFields();
assertCapabilityRequestStructsRejectUnknownFields();

console.log("PASS home entropy check");
