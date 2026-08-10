#!/usr/bin/env node

import {
  readdirSync,
  readFileSync,
  statSync,
} from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = fileURLToPath(new URL("../", import.meta.url));
const capsulesRoot = join(repoRoot, "capsules");
const trustedHost = "capsules/home/browser/home-clipboard-host.js";
const canonicalProtocol =
  "capsules/home/browser/home-clipboard-protocol.js";
const canonicalClient = "capsules/home/browser/home-clipboard-client.js";
const directClipboardPattern =
  /\bnavigator\s*(?:\.\s*|\?\.\s*)clipboard\b|\bnavigator\s*\[\s*["']clipboard["']\s*\]/u;
const productionExtensions = new Set([".html", ".js", ".mjs"]);

function assert(condition, message, details = undefined) {
  if (condition) {
    return;
  }
  const suffix = details === undefined ? "" : `\n${JSON.stringify(details, null, 2)}`;
  throw new Error(`${message}${suffix}`);
}

function extension(path) {
  const index = path.lastIndexOf(".");
  return index < 0 ? "" : path.slice(index);
}

function productionBrowserFiles(root) {
  const files = [];
  function visit(path) {
    for (const name of readdirSync(path)) {
      if (name === "node_modules" || name === "vendor") {
        continue;
      }
      const child = join(path, name);
      if (statSync(child).isDirectory()) {
        visit(child);
        continue;
      }
      if (
        productionExtensions.has(extension(name)) &&
        !name.includes(".test.")
      ) {
        files.push(child);
      }
    }
  }
  visit(root);
  return files;
}

const directAccess = [];
const iframePermissions = [];
for (const capsuleName of readdirSync(capsulesRoot)) {
  const browserRoot = join(capsulesRoot, capsuleName, "browser");
  try {
    if (!statSync(browserRoot).isDirectory()) {
      continue;
    }
  } catch (_error) {
    continue;
  }
  for (const file of productionBrowserFiles(browserRoot)) {
    const source = readFileSync(file, "utf8");
    const path = relative(repoRoot, file);
    if (directClipboardPattern.test(source)) {
      directAccess.push(path);
    }
    if (
      path !== trustedHost &&
      /clipboard-(?:read|write)\b/u.test(source)
    ) {
      iframePermissions.push(path);
    }
  }
}

assert(
  directAccess.length === 1 && directAccess[0] === trustedHost,
  "Only the trusted top-level Home Clipboard host may access navigator.clipboard",
  { directAccess },
);
assert(
  iframePermissions.length === 0,
  "Opaque first-party capsule frames must receive no Clipboard permission",
  { iframePermissions },
);

const canonicalClientImports = Object.freeze({
  "capsules/browser/browser/browser.js": 'targetId: "browser"',
  "capsules/wallet/browser/wallet.js": 'targetId: "wallet"',
  "capsules/wallet-metamask/browser/wallet-metamask.js": "targetId: CONNECTOR_ID",
  "capsules/wallet-unisat/browser/wallet-unisat.js": "targetId: CONNECTOR_ID",
  "capsules/wallet-walletconnect/browser/wallet-walletconnect.js":
    "targetId: CONNECTOR_ID",
  "capsules/library/browser/src/app.js": 'targetId: "library"',
  "capsules/documents/browser/index.html": 'targetId: "documents"',
  "capsules/chat-room/browser/index.html": 'targetId: "chat-room"',
});
for (const [path, targetBinding] of Object.entries(canonicalClientImports)) {
  const source = readFileSync(join(repoRoot, path), "utf8");
  assert(
    source.includes(
      'from "/apps/home/home-clipboard-client.js?v=home-20260726a"',
    ) &&
      source.includes("createHomeClipboardClient({") &&
      source.includes(targetBinding) &&
      source.includes("homeClipboard.start()"),
    `${path} must use the canonical Home Clipboard client and its fixed target binding`,
  );
}

const hostSource = readFileSync(join(repoRoot, trustedHost), "utf8");
const clientSource = readFileSync(join(repoRoot, canonicalClient), "utf8");
const protocolSource = readFileSync(join(repoRoot, canonicalProtocol), "utf8");
const libraryAppSource = readFileSync(
  join(repoRoot, "capsules/library/browser/src/app.js"),
  "utf8",
);
const libraryActionsSource = readFileSync(
  join(repoRoot, "capsules/library/browser/src/actions.js"),
  "utf8",
);
const libraryDialogSource = readFileSync(
  join(repoRoot, "capsules/library/browser/src/dialog.js"),
  "utf8",
);
const chatRoomUiSource = readFileSync(
  join(repoRoot, "capsules/chat-room-ui/src/lib.rs"),
  "utf8",
);
assert(
  protocolSource.includes("HOME_CLIPBOARD_TARGET_PURPOSE_POLICY") &&
    protocolSource.includes('"browser.text"') &&
    protocolSource.includes('"wallet.address"') &&
    protocolSource.includes('"wallet.recovery-key"') &&
    protocolSource.includes('"resource.identifier"') &&
    protocolSource.includes('"resource.uri"') &&
    hostSource.includes(
      'from "./home-clipboard-protocol.js?v=home-20260726a"',
    ) &&
    clientSource.includes(
      'from "./home-clipboard-protocol.js?v=home-20260726a"',
    ) &&
    hostSource.includes("context.targetId") &&
    !hostSource.includes("data.targetId"),
  "Home Clipboard must share one closed protocol policy and derive target identity from verified frame context",
);
assert(
  protocolSource.includes('"conversation.invite"') &&
    hostSource.includes('"chat-room:conversation.invite:write"') &&
    chatRoomUiSource.includes('JsValue::from_str("elastosChatCopyInvite")') &&
    !chatRoomUiSource.includes('JsValue::from_str("clipboard")'),
  "Chat invite copies must use the canonical trusted Home Clipboard path",
);
assert(
  protocolSource.includes(
    "Object.hasOwn(HOME_CLIPBOARD_TARGET_PURPOSE_POLICY, targetId)",
  ) &&
    protocolSource.includes("Object.hasOwn(targetPolicy, purpose)") &&
    protocolSource.includes("text.length > policy.maxUtf8Bytes") &&
    hostSource.includes(
      "data.purpose.length <= MAX_HOME_CLIPBOARD_PURPOSE_CODE_UNITS",
    ) &&
    hostSource.includes(': "invalid";'),
  "Clipboard lookup must be own-property-only, cheaply bounded, and return bounded malformed results",
);
assert(
  !hostSource.includes("elastos.home.clipboard.request/v1") &&
    !clientSource.includes("elastos.home.clipboard.request/v1") &&
    !hostSource.includes("const TARGET_PURPOSE_POLICY") &&
    !clientSource.includes("const TARGET_PURPOSE_POLICY"),
  "Clipboard wire schemas and target policy must have only one source of truth",
);
assert(
  libraryAppSource.includes("Copy Content CID") &&
    libraryAppSource.includes('purpose: "resource.identifier"') &&
    libraryAppSource.includes('{ purpose: "resource.uri" }') &&
    libraryActionsSource.includes('purpose === "resource.identifier"') &&
    libraryActionsSource.includes('purpose === "resource.uri"') &&
    libraryDialogSource.includes('data-copy-purpose=') &&
    libraryDialogSource.includes(
      '"content ID", "resource.identifier"',
    ) &&
    libraryDialogSource.includes(
      '"published CID", "resource.identifier"',
    ) &&
    libraryDialogSource.includes(
      '"object head", "resource.identifier"',
    ) &&
    libraryDialogSource.includes(
      '"resolver target", "resource.identifier"',
    ),
  "Library identifier and resource URI actions must remain distinct canonical Home Clipboard callers",
);

process.stdout.write(
  `${JSON.stringify({
    schema: "elastos.home.clipboard-source-gate/v1",
    ok: true,
    trusted_clipboard_authority: trustedHost,
    canonical_protocol: canonicalProtocol,
    library_identifier_purpose: "resource.identifier",
    library_uri_purpose: "resource.uri",
    migrated_capsules: Object.keys(canonicalClientImports),
    direct_capsule_clipboard_access: 0,
    iframe_clipboard_permissions: 0,
  })}\n`,
);
