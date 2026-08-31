#!/usr/bin/env node

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

function assert(condition, message, details = undefined) {
  if (!condition) {
    throw new Error(`${message}${details ? `\n${JSON.stringify(details, null, 2)}` : ""}`);
  }
}

const html = readFileSync(resolve("capsules/marketplace/browser/index.html"), "utf8");
const js = readFileSync(resolve("capsules/marketplace/browser/marketplace.js"), "utf8");
const css = readFileSync(resolve("capsules/marketplace/browser/marketplace.css"), "utf8");
const vendorScript = readFileSync(resolve("scripts/vendor-ui-tokens.sh"), "utf8");

const letterSpacingValues = [...css.matchAll(/letter-spacing:\s*([^;]+);/g)].map((match) => match[1].trim());

assert(
  html.includes("<title>Apps · ElastOS</title>")
    && html.includes('<script src="./elastos-theme.js"></script>')
    && html.includes('<link rel="stylesheet" href="./elastos-ui.css">')
    && html.includes('<div class="store-product-name">Apps</div>')
    && html.includes('data-destination="media"'),
  "Marketplace must expose the Apps and Media surfaces and load the canonical shared theme assets.",
);
assert(
  vendorScript.includes("marketplace/browser"),
  "Marketplace must participate in the canonical shared token vendoring list.",
);
assert(
  letterSpacingValues.length > 0 && letterSpacingValues.every((value) => value === "0"),
  "Marketplace UIUX must keep every letter-spacing declaration at 0.",
  { letterSpacingValues },
);
assert(
  js.includes('const homeToken = new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token") || "";')
    && !js.includes('params.get("home_token")'),
  "Marketplace must read the Home token from the hash once, not from search params.",
);
assert(
  js.includes("function announceHomeChrome()")
    && js.includes('window.top.postMessage({ type: "home:app-ready", homeToken }, homeParentOrigin);')
    && js.includes("homeChromeReady = true;")
    && js.includes("syncHomeMenuManifest();"),
  "Marketplace must announce Home readiness before syncing the menu manifest.",
);
assert(
  js.includes('type: "home:menu-manifest"')
    && js.includes('title: "File"')
    && js.includes('title: "View"')
    && js.includes('{ label: "New Window", cmd: "__new-window" }')
    && js.includes('{ label: "Close Window", cmd: "__close-window" }')
    && js.includes('{ label: "Refresh", cmd: "refresh" }')
    && js.includes("lastHomeMenuManifestSignature"),
  "Marketplace must publish the accepted Home menu and deduplicate unchanged manifests.",
);
assert(
  js.includes('if (event.origin !== "null" || event.source !== window.parent) {')
    && js.includes('if (data?.type !== "elastos:menu-command" || typeof data.cmd !== "string") {')
    && !js.includes("window.location.origin")
    && !js.includes('event.source !== window.top'),
  "Marketplace must accept inbound Home commands only from the opaque parent boundary.",
);
assert(
  js.includes('fetch("/api/capsules/catalog", { headers: { "x-elastos-home-token": homeToken } })')
    && js.includes('fetch("/api/capsules/interfaces", { headers: { "x-elastos-home-token": homeToken } })')
    && js.includes('postObjectProvider("list_runtime_custody", {})')
    && js.includes('`/api/provider/object/${operation}`')
    && js.includes('postObjectProvider("buy", { mint_id: mintId })')
    && !js.includes("/api/apps/marketplace/catalog")
    && js.includes('type: "home:open-target"')
    && js.includes('target: "elacity-player"'),
  "Marketplace must keep the canonical catalog/interface reads, typed media routes, and Home launch target path.",
);
assert(
  js.includes('const RUNTIME_CUSTODY_LISTINGS_RESPONSE_SCHEMA_V1 = "elastos.library.runtime-custody-listings/v1";')
    && js.includes('const RUNTIME_CUSTODY_LISTING_SCHEMA_V1 = "elastos.library.runtime-custody-listing/v1";')
    && js.includes('const RUNTIME_CUSTODY_AVAILABILITY_SCHEMA_V1 = "elastos.library.runtime-custody-availability-summary/v1";')
    && js.includes("const MAX_RUNTIME_CUSTODY_LISTINGS = 128;")
    && js.includes("const MAX_RUNTIME_CUSTODY_PUBLIC_TEXT_BYTES = 256;")
    && js.includes("const MAX_U32 = 0xffffffff;")
    && js.includes("const UINT256_HEX = /^0x(?:0|[1-9a-f][0-9a-f]{0,63})$/;")
    && js.includes("const ADDRESS_HEX = /^0x[0-9a-f]{40}$/;")
    && js.includes('const pendingMediaBuys = new Set();')
    && js.includes('if (pendingMediaBuys.has(mintId)) {')
    && js.includes('pendingMediaBuys.add(mintId);')
    && js.includes('pendingMediaBuys.delete(mintId);')
    && js.includes('disabled aria-busy="true">Buying...</button>')
    && js.includes("function boundedTimestamp(value) {")
    && js.includes("Number.isSafeInteger(value)")
    && js.includes('typeof value !== "string"')
    && js.includes("new TextEncoder().encode(value).length > maxBytes")
    && js.includes("observedReplicas < requiredReplicas")
    && js.includes("function uint256Decimal(value) {")
    && js.includes("if (!UINT256_HEX.test(value)) {")
    && js.includes("return BigInt(value).toString(10);")
    && js.includes("`quantity ${uint256Decimal(listing.quantity)}`")
    && js.includes("`price ${uint256Decimal(listing.price)} base units`")
    && js.includes('postObjectProvider("buy", { mint_id: mintId })')
    && !js.includes("MAX_PUBLISHED_AT_LENGTH"),
  "Marketplace must retain canonical media values while presenting validated uint256 values in decimal.",
);
assert(
  js.includes("loadCatalogData().then(render)")
    && js.includes("loadMediaData().then(render)")
    && !js.includes("await Promise.all([loadCatalogData(), loadMediaData()]);"),
  "Marketplace must let catalog and media surfaces finish independently.",
);
assert(
  !js.includes("/api/viewers/")
    && !js.includes("publisher_principal_id")
    && !js.includes("window.open")
    && !js.includes("target=_blank"),
  "Marketplace must keep protected media actions on the typed Runtime path only.",
);
assert(
  js.includes("function isValidCapsuleIconVariant(capsuleName, entry)")
    && js.includes("CAPSULE_ICON_ROUTE")
    && js.includes('route.startsWith(`/apps/${capsuleName}/`)')
    && !js.includes("FIRST_PARTY_ICON_IDS")
    && !js.includes("OWN_ICON_CAPSULES")
    && !js.includes("resolveFirstPartyIconId")
    && !js.includes('id.includes("wallet")')
    && !js.includes('`/apps/${encodeURIComponent(name)}/icons/icon-128.png`'),
  "Marketplace must use only strict declared capsule icon routes and one generic fallback.",
);
assert(
  !html.includes("onerror=")
    && !js.includes("onerror=")
    && js.includes("function bindRasterIconFallbacks(root) {")
    && js.includes('image.addEventListener("error", () => {'),
  "Marketplace must bind raster icon fallback in JavaScript, not with inline event attributes.",
);
assert(
  js.includes('if (!app.iconRoute) {')
    && js.includes("app-icon-glyph")
    && js.includes("No executable actions declared")
    && !js.includes("Install pending")
    && !html.includes("install-modal"),
  "Marketplace must show a generic glyph fallback and must not offer fake install actions.",
);
assert(
  !js.includes("localStorage")
    && !js.includes("sessionStorage")
    && !js.includes("navigator.clipboard")
    && !js.includes("indexedDB")
    && !js.includes("carrier")
    && js.includes("\\b(schema|projection|provider|adapter|capability|affordance|runtime|runtime-owned|launch token|hostcall|request failed|failed to fetch|unauthorized|forbidden|[45]\\d\\d)\\b"),
  "Marketplace must keep browser authority local and redact internal Runtime errors.",
);

console.log("marketplace-product-behavior-smoke: OK");
