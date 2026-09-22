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
// The protected-item listing contract lives in its own module so that the
// tests beside it can import it. This file pins the contract in that module
// and pins marketplace.js to importing it rather than keeping a second copy.
const listingJs = readFileSync(resolve("capsules/marketplace/browser/src/listing.js"), "utf8");
const listingTests = readFileSync(resolve("capsules/marketplace/browser/src/listing.test.mjs"), "utf8");
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
    // The viewer follows the item. Marketplace used to name the player for
    // everything, which sent pictures and documents to a video surface.
    && js.includes("target: viewerForListing(listing),")
    && !js.includes('target: "elacity-player"'),
  "Marketplace must keep the canonical catalog/interface reads, typed media routes, and Home launch target path.",
);
assert(
  // The kind is Runtime's answer, carried in the row. Deciding it from the
  // MIME instead would be a second rule for a question the open path already
  // enforces, and a second rule can disagree.
  listingJs.includes("export function viewerForListing(listing) {")
    && listingJs.includes("listing?.contentKind === CONTENT_KIND_MEDIA")
    && listingJs.includes('? "elacity-player"')
    && listingJs.includes(': "elacity-reader"')
    && listingJs.includes('export const CONTENT_KIND_MEDIA = "media";')
    && listingJs.includes('export const CONTENT_KIND_OBJECT = "object";')
    && listingJs.includes('"content_kind",')
    && !/viewerForListing[\s\S]{0,200}startsWith\("video\//.test(listingJs)
    && listingTests.includes("an item opens in the viewer its own kind names")
    && listingTests.includes("the kind decides the viewer, and the MIME never overrides it")
    && listingTests.includes("an item whose kind cannot be read opens in the viewer that can say so"),
  "The viewer for a listed item must be chosen by the kind Runtime names, with tests covering both viewers, a disagreeing MIME, and the unreadable case.",
);
assert(
  listingJs.includes('export const RUNTIME_CUSTODY_LISTINGS_RESPONSE_SCHEMA_V1 = "elastos.library.runtime-custody-listings/v1";')
    && listingJs.includes('export const RUNTIME_CUSTODY_LISTING_SCHEMA_V1 = "elastos.library.runtime-custody-listing/v1";')
    && listingJs.includes('export const RUNTIME_CUSTODY_AVAILABILITY_SCHEMA_V1 = "elastos.library.runtime-custody-availability-summary/v1";')
    && listingJs.includes("export const MAX_RUNTIME_CUSTODY_LISTINGS = 128;")
    && listingJs.includes("export const MAX_RUNTIME_CUSTODY_PUBLIC_TEXT_BYTES = 256;")
    && listingJs.includes("const MAX_U32 = 0xffffffff;")
    && listingJs.includes("const UINT256_HEX = /^0x(?:0|[1-9a-f][0-9a-f]{0,63})$/;")
    && listingJs.includes("const ADDRESS_HEX = /^0x[0-9a-f]{40}$/;")
    && listingJs.includes("const SHA256_HEX = /^[0-9a-f]{64}$/;")
    && listingJs.includes("function boundedTimestamp(value) {")
    && listingJs.includes("Number.isSafeInteger(value)")
    && listingJs.includes('typeof value !== "string"')
    && listingJs.includes("new TextEncoder().encode(value).length > maxBytes")
    && listingJs.includes("observedReplicas < requiredReplicas")
    && listingJs.includes("export function uint256Decimal(value) {")
    && listingJs.includes("if (!UINT256_HEX.test(value)) {")
    && listingJs.includes("return BigInt(value).toString(10);")
    && !listingJs.includes("MAX_PUBLISHED_AT_LENGTH"),
  "The protected-item listing contract must keep its canonical Runtime values and validators.",
);
assert(
  // Every field Runtime's own summary emits is named here, including the
  // availability receipt digest whose absence once emptied the whole shelf.
  listingJs.includes('"receipt_digest",')
    && listingJs.includes("const AVAILABILITY_KEYS = [")
    && listingJs.includes("const LISTING_KEYS = [")
    && listingJs.includes("boundedText(value.codecs, MAX_RUNTIME_CUSTODY_PUBLIC_TEXT_BYTES)")
    && listingTests.includes("an object row Runtime publishes today is accepted, empty codecs and all")
    && listingTests.includes("the availability receipt digest is accepted and stays out of the rendered row"),
  "The listing parser must accept exactly what Runtime publishes for both media and object rows, with tests that say so.",
);
assert(
  // An owned copy can always be built again: the chain says whose it is, and
  // the file is made of material anyone can fetch. So a person who holds one
  // is offered that, whether the copy never arrived or they deleted it.
  js.includes('postObjectProvider("download_owned_copy", { mint_id: mintId })')
    && js.includes("async function downloadOwnedCopy(mintId) {")
    && js.includes('data-action="download-copy"')
    && js.includes("const pendingDownloads = new Set();")
    // The gate is that an item still on sale to this person offers no
    // rebuild. The sentence carrying it has changed twice -- an early return,
    // then a branch, now the card's own state -- so the rule is asserted
    // rather than the spelling.
    && /const secondary = state === "mine" \|\| state === "owned"/.test(js)
    && /data-action="download-copy"/.test(js),
  "Marketplace must let a person rebuild a copy they own, and offer it only on copies they own.",
);
assert(
  // The other half of adding a listing: the person who listed it has the link
  // to pass on. Runtime has answered with it since listings existed and no
  // surface showed it, so reaching an item on another Home meant already
  // knowing its address.
  js.includes("function showShareListing(mintId) {")
    && js.includes('data-action="share-listing"')
    && js.includes('id="share-listing-uri"')
    // The link is selected for the person rather than written to their
    // clipboard: reading that authority from a capsule is a line this app's
    // own gate holds, a few assertions below.
    && js.includes("field?.select();")
    && js.includes("press the copy key")
    && js.includes('listing.accessState !== "creator"')
    && listingJs.includes('"listing_uri",')
    && listingJs.includes("listingUriFromInput(value.listing_uri)")
    && listingTests.includes("a link that is not a listing link"),
  "Marketplace must give the person who listed an item the link others add it by.",
);
assert(
  // A listing published on another Home reaches this one by its link. The
  // app checks the link's shape and Runtime verifies everything that matters.
  js.includes('postObjectProvider("import_runtime_custody", { listing_uri: listingUri })')
    && js.includes("listingUriFromInput(value)")
    && html.includes('id="import-listing-uri"')
    && html.includes('id="import-listing"')
    && listingJs.includes("export function listingUriFromInput(value) {")
    && listingJs.includes("const LISTING_URI = /^elastos:\\/\\/[A-Za-z0-9]{16,128}$/;")
    && listingTests.includes("a listing link is accepted in the shapes Runtime publishes")
    && listingTests.includes("a listing link that is not one is refused before Runtime is asked"),
  "Marketplace must let a person add a listing another Home published, and check the link's shape before asking Runtime.",
);
assert(
  // The wallet asks for a signature, not for a decision, and it arrives after
  // the availability re-check. A person who has not seen the terms by then has
  // waited minutes to find out what they cost.
  js.includes("function showBuyConfirmation(mintId) {")
    && js.includes('if (action === "confirm-buy") {')
    && js.includes("Your wallet asks you to approve the payment.")
    && js.includes("uint256Decimal(listing.price)")
    && js.includes("uint256Decimal(listing.quantity)")
    && js.includes("abbreviateAddress(listing.sellerAddress)")
    && js.includes("showBuyConfirmation(target.dataset.mint)"),
  "Marketplace must show a purchase's terms, and what follows them, before it spends anything.",
);
assert(
  // A purchase lives in Runtime, not in the page that started it. Saying so is
  // its own field rather than another access_state value, because a new value
  // falls silently into a consumer's default while an unknown field is refused
  // loudly by the parser on the other side of this contract.
  listingJs.includes('"purchase_in_flight",')
    && listingJs.includes("purchaseInFlight: value.purchase_in_flight,")
    && js.includes('data-action="resume-buy"')
    && js.includes("listing.purchaseInFlight")
    && js.includes("A purchase of this is already under way.")
    && listingTests.includes("a purchase already under way survives the page that started it"),
  "Marketplace must show a purchase that is still under way after the page that started it has gone.",
);
assert(
  // A purchase is a sequence of waits. The app reads which one it is in from
  // typed data and keeps asking, instead of posting once and reporting a
  // sentence it filtered away.
  js.includes("buyOutcomeFromAnswer(error.answer)")
    && js.includes("const BUY_POLL_MS = 4000;")
    && js.includes('outcome.kind === "declined"')
    && js.includes('target: "wallet"')
    && js.includes("function buyStateLabel(buyState) {")
    && js.includes("function buyStateNote(buyState) {")
    && js.includes("failure.answer = answer && typeof answer === \"object\" ? answer : {};")
    && !js.includes("const pendingMediaBuys = new Set();")
    && listingJs.includes("export function buyOutcomeFromAnswer(answer) {")
    && listingJs.includes('export const BUY_PROGRESS_SCHEMA_V1 = "elastos.protected-content.buy-progress/v1";')
    && listingTests.includes("a purchase waiting on the person is told apart from one waiting on the network")
    && listingTests.includes("a declined purchase stops, and asking again is the person's choice")
    && listingTests.includes("an answer that carries no progress is a failure, not a wait"),
  "Marketplace must drive a purchase from the typed stage Runtime reports, and stop when the person declined.",
);
assert(
  js.includes('} from "./src/listing.js";')
    && js.includes("uint256Decimal(listing.quantity)")
    && js.includes("uint256Decimal(listing.price)")
    && js.includes("`quantity ${uint256Decimal(listing.quantity)}`")
    // A price is searched as it is shown as well as in full. Both come from
    // `formatMoney`, which reads the uint256 through `uint256Decimal` and
    // scales it by moving digits -- no step of a price is a JavaScript
    // number, which is the rule this line exists to hold.
    && js.includes("const money = formatMoney(listing);")
    && js.includes("`price ${money.value} ${money.title}`")
    && js.includes("const full = uint256Decimal(listing.price);")
    && js.includes("function shiftDecimal(digits, decimals) {")
    && !js.includes("Number(listing.price)")
    && !js.includes("parseFloat")
    && js.includes('postObjectProvider("buy", { mint_id: mintId })')
    && js.includes("state.mediaRejected")
    && !js.includes("function parseRuntimeCustodyListing(")
    && !js.includes("function assertExactKeys("),
  "Marketplace must read the listing contract from its module, keep one copy of it, and say when a row was refused.",
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
