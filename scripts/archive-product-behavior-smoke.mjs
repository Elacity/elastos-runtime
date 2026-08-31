#!/usr/bin/env node

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

const source = readFileSync(resolve("capsules/archive-manager/browser/index.html"), "utf8");
const letterSpacingValues = [...source.matchAll(/letter-spacing:\s*([^;]+);/g)].map((match) => match[1].trim());

assert(
  source.includes('<script src="./elastos-theme.js"></script>') &&
    source.includes('<link rel="stylesheet" href="./elastos-ui.css">'),
  "Archive must load the canonical shared theme and token sheet.",
);
assert(
  letterSpacingValues.length === 5 && letterSpacingValues.every((value) => value === "0"),
  "Archive UIUX must keep every letter-spacing declaration at 0.",
);
assert(
  source.includes('const homeToken = new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token") || "";') &&
    !source.includes('params.get("home_token")'),
  "Archive must read the Home token from the hash once, not from search params.",
);
assert(
  source.includes("function announceHomeChrome()") &&
    source.includes('window.top.postMessage({ type: "home:app-ready", homeToken }, homeParentOrigin);') &&
    source.includes("homeChromeReady = true;") &&
    source.includes("syncHomeMenuManifest();"),
  "Archive must announce Home readiness once before syncing its menu manifest.",
);
assert(
  source.includes('type: "home:menu-manifest"') &&
    source.includes('title: "File"') &&
    source.includes('title: "Edit"') &&
    source.includes('{ label: "Open Archive...", cmd: "open-archive" }') &&
    source.includes('{ label: "New Archive...", cmd: "new-archive" }') &&
    source.includes('{ label: "Select All Safe Files", cmd: "select-all-safe" }') &&
    source.includes('{ label: "Clear Selection", cmd: "clear-selection" }') &&
    source.includes('{ label: "Close Window", cmd: "__close-window" }'),
  "Archive must expose the accepted File and Edit Home menus.",
);
assert(
  source.includes('function isTrustedHomeMessage(event) {') &&
    source.includes('return event.origin === "null" && event.source === window.parent;') &&
    source.includes("window.addEventListener(\"message\", handleTrustedHomeMessage);") &&
    !source.includes("event.origin !== homeParentOrigin || event.source !== window.top"),
  "Archive must accept inbound Home messages only from the opaque parent frame boundary.",
);
assert(
  source.includes('if (data.type === "archive:open-library-object") {') &&
    source.includes('if (data.type !== "elastos:menu-command" || typeof data.cmd !== "string") return;') &&
    source.includes('handleHomeMenuCommand(data.cmd);'),
  "Archive must route trusted Home object handoff and menu commands through one listener.",
);
assert(
  source.includes('document.querySelector("#open-archive-button").click();') &&
    source.includes('document.querySelector("#new-archive-button").click();') &&
    source.includes('document.querySelector("#select-all-safe").click();') &&
    source.includes('document.querySelector("#clear-selection").click();'),
  "Archive Home menu commands must reuse the existing button paths.",
);
assert(
  source.includes('const url = new URL("/api/viewers/archive-manager/library-object", window.location.origin);') &&
    source.includes('await fetch("/api/viewers/archive-manager/library-roots", {') &&
    source.includes('destination_uri: destinationUri,') &&
    source.includes("entries,") &&
    source.includes('conflict_policy: document.querySelector("#conflict-policy").value,'),
  "Archive must keep the current viewer routes and extract payload keys unchanged.",
);
assert(
  source.includes("function safeEntries()") &&
    source.includes('entry.path && entry.kind !== "blocked" && entry?.safety?.status !== "blocked"') &&
    source.includes("function safeVisibleEntries()") &&
    source.includes('String(entry.path || entry.name || "").toLowerCase().includes(query)'),
  "Archive must keep search and safe-only selection derived from the current entry set.",
);
assert(
  !source.includes("localStorage") &&
    !source.includes("sessionStorage") &&
    !source.includes("navigator.clipboard") &&
    !source.includes("indexedDB") &&
    !source.includes("carrier"),
  "Archive must not add browser storage, clipboard fallback, or direct Carrier authority.",
);

console.log("archive-product-behavior-smoke: OK");
