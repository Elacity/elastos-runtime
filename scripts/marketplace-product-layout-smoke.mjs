#!/usr/bin/env node

import { createServer } from "node:http";
import { createRequire } from "node:module";
import { readFile } from "node:fs/promises";
import { extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(fileURLToPath(new URL("../", import.meta.url)));
const browserRoot = join(repoRoot, "capsules/marketplace/browser");
const brave = process.env.BRAVE_BIN || "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser";
const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");

const normalToken = "marketplace-layout-token";
const errorToken = "marketplace-error-token";
const mediaErrorToken = "marketplace-media-error-token";
const mediaMalformedToken = "marketplace-media-malformed-token";
const mediaEmptyToken = "marketplace-media-empty-token";
const mediaPendingBuyToken = "marketplace-media-pending-buy-token";
// A fresh page whose first market read fails once, and one whose market
// cannot be reached at all. Both hold one listing of this Home's own.
const marketRetryToken = "marketplace-market-retry-token";
const marketDownToken = "marketplace-market-down-token";
const mediaCreatorMint = "a".repeat(64);
const mediaPurchasedMint = "b".repeat(64);
const mediaAvailableMint = "c".repeat(64);
const mediaImportedMint = "d".repeat(64);
const mediaSongMint = "e".repeat(64);
const mediaPaperMint = "f".repeat(64);
const mediaSoldOutMint = "1".repeat(64);
const mediaCoverCid = "QmYtisnG1wCaUCAGp3hi2xKKeaTzMFhvxyFf1crWaTyoRH";
const mediaMetadataCid = "QmX1KSGkt3fLX5GsuN7wVWrCj55PSnfbk4JJ2BTY2QRJ7F";
const mediaBareMetadataCid = "QmUwdnvSofKYGnBSWwqtgnEJLWMPD9fDv9ZdvSrbFSg7ov";
// What a creator hands out: `elastos://` and the listing package's CID.
const importedListingUri = "elastos://QmcHh9eQuLiisfxZo4m4TxaVTSYzs4AF4LaVcmkd2o2Y76";
const mediaPayToken = "0x1111111111111111111111111111111111111111";
const mediaSellerAddress = "0x2222222222222222222222222222222222222222";
const mediaTokenId = "0x7";

// Task 9 / Task 10: `elastos.marketplace.listing/v1`, exactly as
// `src/market-listing.js` and its test pin it against Runtime's producer.
// The asset CID is distinct from `importedListingUri`'s so the same page can
// tell a live `ListingObject` apart from a link Runtime still answers with
// `legacy_listing: true`.
const marketAssetLinkCid = "QmgaWiKs3sjiZNuzFXvZMkvqj5Ksa6vMZhfFgDv5yK7oP6";
const marketForeignAssetCid = "Qme9vpu7iwjJxBCG2sZM1xRjmmeoM3NAefahEqUFDDzwE5";
const marketAssetLink = `elastos://${marketAssetLinkCid}`;
const marketForeignAssetLink = `elastos://${marketForeignAssetCid}`;
const marketListingLedger = "0x0000000000000000000000000000000051515151";
const marketListingTokenId = "0x2a";
const marketListingOperative = "0x0000000000000000000000000000000062626262";
const marketListingKid = "0x00000000000000000000000091929394";
const marketForeignLedger = "0x0000000000000000000000000000000095959595";
const marketForeignTokenId = "0x2b";
const marketSellerA = "0x0000000000000000000000000000000073737373";
const marketSellerB = "0x0000000000000000000000000000000084848484";
// The Explore catalog item Task 10's Buy resolves through the market path --
// `catalogItems[0]` below ("Someone Else's Film"), named by its own ledger
// and token id.
const marketExploreLedger = "0x0ebac909d31ef0074495e752c0cf4ea49ba13c41";
const marketExploreTokenId = "0x1f";

function marketOfferFixture(overrides = {}) {
  return {
    seller: marketSellerA,
    quantity: "0x3",
    price: "0x2dc6c0", // 3,000,000 base units of `mediaPayToken` (USDC, 6dp): $3
    pay_token: mediaPayToken,
    payment_processor: marketListingOperative,
    ...overrides,
  };
}

function marketListingFixture(overrides = {}) {
  return {
    schema: "elastos.marketplace.listing/v1",
    item: {
      chain_namespace: "eip155:8453",
      network: "base",
      ledger: marketListingLedger,
      token_id: marketListingTokenId,
      operative: marketListingOperative,
      kid: marketListingKid,
    },
    asset: {
      uri: marketAssetLink,
      title: "Shared Documentary",
      description: "",
      cover_cid: null,
      category: "video",
      mime_type: "video/mp4",
      readability: "verified",
    },
    offers: [
      marketOfferFixture(),
      marketOfferFixture({ seller: marketSellerB, quantity: "0x1", price: "0x1e8480" }), // $2
    ],
    access_state: "available",
    source: { start: "token_uri", read_at_block: "0x2a" },
    ...overrides,
  };
}

function marketForeignListingFixture() {
  return marketListingFixture({
    item: {
      chain_namespace: "eip155:8453",
      network: "base",
      ledger: marketForeignLedger,
      token_id: marketForeignTokenId,
      operative: marketListingOperative,
    },
    asset: {
      uri: marketForeignAssetLink,
      title: "Foreign Mint",
      description: "",
      cover_cid: null,
      category: "video",
      mime_type: "video/mp4",
      readability: "foreign",
    },
    offers: [marketOfferFixture()],
    source: { start: "token_uri", read_at_block: "0x2b" },
  });
}

function marketExploreListingFixture() {
  return marketListingFixture({
    item: {
      chain_namespace: "eip155:8453",
      network: "base",
      ledger: marketExploreLedger,
      token_id: marketExploreTokenId,
      operative: marketListingOperative,
    },
    asset: {
      uri: `elastos://${mediaBareMetadataCid}`,
      title: "Someone Else's Film",
      description: "",
      cover_cid: null,
      category: "video",
      mime_type: "video/mp4",
      readability: "verified",
    },
    offers: [marketOfferFixture({ seller: "0x34daf3000000000000000000000000000000beef", quantity: "0xc" })],
    source: { start: "item", read_at_block: "0x2c" },
  });
}

// `catalogItems[1]` ("Someone Else's Paper"), once bought: foreign, so it
// opens on ela.city and never in this Home's Library.
const marketExplorePaperTokenId = "0x20";
const marketExplorePaperOperative = "0x1aa4dd2cb9c9b784eaccedf5c679fd267cab1033";

function marketExplorePaperListingFixture() {
  return marketListingFixture({
    item: {
      chain_namespace: "eip155:8453",
      network: "base",
      ledger: marketExploreLedger,
      token_id: marketExplorePaperTokenId,
      operative: marketExplorePaperOperative,
    },
    asset: {
      uri: `elastos://${mediaBareMetadataCid}`,
      title: "Someone Else's Paper",
      description: "A <b>paper</b> minted elsewhere.",
      cover_cid: null,
      category: "document",
      mime_type: "application/pdf",
      readability: "foreign",
    },
    offers: [marketOfferFixture({ seller: "0x34daf3000000000000000000000000000000beef", quantity: "0x2" })],
    access_state: "purchased",
    source: { start: "item", read_at_block: "0x2d" },
  });
}

// Links whose `/listing` read is refused, one per refusal the page words for
// a person. Any `Qm` + 44 base58 characters is a folder CID to the page.
const marketMismatchLink = `elastos://Qm${"M".repeat(44)}`;
const marketUnboundLink = `elastos://Qm${"U".repeat(44)}`;
const marketUnavailableLink = `elastos://Qm${"V".repeat(44)}`;
const marketInvalidStartLink = `elastos://Qm${"S".repeat(44)}`;
// Items whose sheets carry one seller per `buy_offer` answer the page must
// word: the scenario item, an item with a purchase in progress through the
// other path (`current: null`), and two items this Home already holds.
const marketScenarioLink = `elastos://Qm${"A".repeat(44)}`;
const marketNullRaceLink = `elastos://Qm${"B".repeat(44)}`;
const marketPurchasedLink = `elastos://Qm${"C".repeat(44)}`;
const marketCreatorLink = `elastos://Qm${"D".repeat(44)}`;
// An item whose recorded purchase (`marketRecordedOffer`) names a seller
// that no longer lists it: its sheet shows only `marketSellerGoneOther`.
const marketGoneRecordedLink = `elastos://Qm${"G".repeat(44)}`;
// An item Runtime could not learn the metadata or KID binding of (R46): its
// readability is `unknown` and its kid null, and it is still for sale. One
// seller's Buy is refused as unavailable (R48); the other's completes.
const marketUnknownLink = `elastos://Qm${"K".repeat(44)}`;
const marketSellerOwn = `0x${"a1".repeat(20)}`;
const marketSellerOwned = `0x${"a2".repeat(20)}`;
const marketSellerMismatch = `0x${"a3".repeat(20)}`;
const marketSellerDeclined = `0x${"a4".repeat(20)}`;
const marketSellerSlow = `0x${"a5".repeat(20)}`;
const marketSellerRace = `0x${"a6".repeat(20)}`;
const marketSellerRecorded = `0x${"a7".repeat(20)}`;
const marketSellerNullRace = `0x${"a8".repeat(20)}`;
const marketSellerNullOther = `0x${"a9".repeat(20)}`;
const marketSellerGoneOther = `0x${"b1".repeat(20)}`;
const marketSellerUnreachable = `0x${"b2".repeat(20)}`;
const marketSellerUnknownOk = `0x${"b3".repeat(20)}`;
// A pay token this Home's table does not list.
const marketUnknownPayToken = `0x${"ee".repeat(20)}`;
// What Runtime recorded for `marketSellerRecorded`'s purchase: $4, not the
// $3 the seller's live offer shows now.
const marketRecordedOffer = marketOfferFixture({ seller: marketSellerRecorded, price: "0x3d0900" });

function marketItemFixtureListing(link, ledgerDigit, tokenId, title, offers, overrides = {}) {
  return marketListingFixture({
    item: {
      chain_namespace: "eip155:8453",
      network: "base",
      ledger: `0x${ledgerDigit.repeat(40)}`,
      token_id: tokenId,
      operative: marketListingOperative,
      kid: marketListingKid,
    },
    asset: {
      uri: link,
      title,
      description: "",
      cover_cid: null,
      category: "video",
      mime_type: "video/mp4",
      readability: "verified",
    },
    offers,
    ...overrides,
  });
}

function marketScenarioListingFixture() {
  return marketItemFixtureListing(marketScenarioLink, "c", "0x3c", "Scenario Film", [
    marketOfferFixture({ seller: marketSellerOwn }),
    marketOfferFixture({ seller: marketSellerOwned }),
    marketOfferFixture({ seller: marketSellerMismatch }),
    marketOfferFixture({ seller: marketSellerDeclined, pay_token: marketUnknownPayToken }),
    marketOfferFixture({ seller: marketSellerSlow, price: "0x5b8d80" }), // $6
    marketOfferFixture({ seller: marketSellerRace }),
    marketOfferFixture({ seller: marketSellerRecorded }),
  ], { offers_truncated: true });
}

function marketNullRaceListingFixture() {
  const listing = marketItemFixtureListing(marketNullRaceLink, "d", "0x3d", "Held Film", [
    marketOfferFixture({ seller: marketSellerNullRace }),
    marketOfferFixture({ seller: marketSellerNullOther }),
  ]);
  listing.asset.readability = "unverified";
  return listing;
}

function marketGoneRecordedListingFixture() {
  return marketItemFixtureListing(marketGoneRecordedLink, "9", "0x3f", "Relisted Film", [
    marketOfferFixture({ seller: marketSellerGoneOther }),
  ]);
}

function marketUnknownListingFixture() {
  const listing = marketItemFixtureListing(marketUnknownLink, "8", "0x40", "Unread Film", [
    marketOfferFixture({ seller: marketSellerUnreachable }),
    marketOfferFixture({ seller: marketSellerUnknownOk }),
  ]);
  listing.item.kid = null;
  listing.asset.readability = "unknown";
  return listing;
}

function marketHeldListingFixture(link, ledgerDigit, accessState) {
  return marketItemFixtureListing(link, ledgerDigit, "0x3e", `Held ${accessState}`, [
    marketOfferFixture({ seller: marketSellerNullOther }),
  ], { access_state: accessState });
}

// `buy_offer`'s error envelope, exactly as the proxy hands it over.
function buyOfferRefusal(code, extra = {}) {
  return { status: "error", code, message: "refused", ...extra };
}

// Exactly the document `runtime_custody_listing_availability` publishes,
// receipt digest included. This fixture once matched the parser instead of the
// producer, so the surface it renders here stayed green while every real row
// was refused in front of a person.
const mediaAvailability = {
  schema: "elastos.library.runtime-custody-availability-summary/v1",
  status: "last_verified_receipt",
  checked_at: 1_756_295_696,
  required_replicas: 3,
  observed_replicas: 3,
  receipt_digest: "357bc17ce614dda83ef2542c01f574c4db5571fe648e2e60eb9977c8b5f0c8ef",
  recheck_before_buy: true,
  recheck_before_open: true,
};

function mediaListing({ mintId, displayName, accessState, quantity, price, codecs = "avc1.640028" }) {
  return {
    schema: "elastos.library.runtime-custody-listing/v1",
    mint_id: mintId,
    display_name: displayName,
    listing_uri: importedListingUri,
    // The published metadata directory, which the shelf reads a title and a
    // cover from. A real CID from this Home's own records.
    metadata_cid: mediaMetadataCid,
    // Runtime names the kind; the shelf and the viewer session read the same
    // two words rather than each deciding from the MIME.
    content_kind: "media",
    mime_type: "video/mp4",
    codecs,
    quantity,
    price,
    pay_token: mediaPayToken,
    seller_address: mediaSellerAddress,
    token_id: mediaTokenId,
    published_at: 1_756_293_600,
    purchase_in_flight: false,
    availability: mediaAvailability,
    access_state: accessState,
  };
}

// The one market item the first-load scenarios expect to see, and the
// listing this Home holds beside it.
const firstLoadMarketItem = {
  ledger: `0x${"c4".repeat(20)}`,
  tokenId: "0x4",
  sellerAddress: `0x${"c5".repeat(20)}`,
  displayName: "Market Only Film",
  contentCategory: "video",
  quantity: 2,
  price: "5",
  payToken: "0x1111111111111111111111111111111111111111",
  publishedAt: 1_760_000_000,
};
const firstLoadMarketItemId = `${firstLoadMarketItem.ledger}:${firstLoadMarketItem.tokenId}`;

function firstLoadScenario(marketFailuresRemaining) {
  return {
    catalogFailuresRemaining: 0,
    marketFailuresRemaining,
    firstMarketBarrier: createBarrier(),
    firstMarketHeld: false,
    mediaListings: [
      mediaListing({
        mintId: mediaCreatorMint,
        displayName: "Home Own Video",
        accessState: "creator",
        quantity: "0x1",
        price: "0x2",
      }),
    ],
    catalogItems: [firstLoadMarketItem],
  };
}

function mediaListResponse(listings, truncated = false) {
  return {
    schema: "elastos.library.runtime-custody-listings/v1",
    truncated,
    listings,
  };
}

const catalogCapsules = [
  {
    name: "people",
    title: "People",
    author: "Elastos",
    description: "Find people and manage contacts.",
    category: "apps",
    role: "app",
    installed: true,
    launchable: true,
    launch_target: "people",
    type: "wasm",
    trust_state: "local-manifest-signature",
    signature_state: "manifest-signature-declared",
    icon: [
      { size: 32, route: "/apps/people/icons/icon-32.png" },
      { size: 128, route: "/apps/people/icons/icon-128.png" },
      { size: 256, route: "/apps/people/icons/icon-256.png" },
    ],
  },
  {
    name: "documents",
    title: "Documents",
    author: "Elastos",
    description: "Write and share Markdown documents.",
    category: "apps",
    role: "app",
    installed: true,
    launchable: true,
    launch_target: "documents",
    type: "wasm",
    trust_state: "cid-with-manifest-signature",
    signature_state: "manifest-signature-declared",
    accepted_content: [{ name: "markdown", title: "Markdown" }],
    requires: [{ name: "people" }],
    viewer_title: "Documents",
  },
  {
    name: "creator",
    title: "Creator",
    author: "Elastos",
    description: "Protect a file and list it for sale.",
    category: "apps",
    role: "app",
    installed: true,
    launchable: true,
    launch_target: "creator",
    type: "wasm",
    trust_state: "local-manifest-signature",
    signature_state: "manifest-signature-declared",
  },
  {
    name: "object-provider",
    title: "Object Provider",
    author: "Elastos",
    description: "Storage service for this Home.",
    category: "providers",
    role: "provider",
    installed: true,
    launchable: false,
    type: "native-provider",
    trust_state: "local-manifest-signature",
    signature_state: "manifest-signature-declared",
  },
  {
    name: "bad-icons",
    title: "Bad Icons",
    author: "Elastos",
    description: "App with invalid icon metadata.",
    category: "apps",
    role: "app",
    installed: false,
    launchable: false,
    type: "wasm",
    icon: [
      { size: 128, route: "/apps/not-allowed/icons/icon-128.png" },
      { size: "oops", route: "/apps/bad-icons/icons/icon-32.png" },
    ],
  },
  {
    name: "zip-viewer",
    title: "ZIP Viewer",
    author: "Elastos",
    description: "Open compatible files and content.",
    category: "viewers",
    role: "viewer",
    installed: false,
    launchable: false,
    type: "wasm",
    accepted_content: [{ name: "zip", title: "ZIP archives" }],
  },
];

const interfaceEntries = [
  {
    capsule: "people",
    interface: {
      methods: [
        {
          input_schema: { accepts: [] },
        },
      ],
    },
    bindings: [
      { method: "capsule.open", executable: true },
      { method: "people.refresh", executable: true },
    ],
  },
  {
    capsule: "documents",
    interface: {
      methods: [
        {
          input_schema: {
            accepts: [
              {
                extensions: [".md"],
              },
            ],
          },
        },
      ],
    },
    bindings: [
      { method: "capsule.open", executable: true },
      { method: "documents.share", executable: true },
    ],
  },
  {
    capsule: "zip-viewer",
    interface: {
      methods: [
        {
          input_schema: {
            accepts: [
              {
                extensions: [".zip"],
              },
            ],
          },
        },
      ],
    },
    bindings: [],
  },
];

function assert(condition, message, details = undefined) {
  if (!condition) {
    throw new Error(`${message}${details ? `\n${JSON.stringify(details, null, 2)}` : ""}`);
  }
}

function createBarrier() {
  let release = () => {};
  const promise = new Promise((resolve) => {
    release = resolve;
  });
  return { promise, release };
}

async function readJsonBody(request) {
  const chunks = [];
  for await (const chunk of request) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
  }
  const body = Buffer.concat(chunks).toString("utf8");
  return body ? JSON.parse(body) : null;
}

function json(response, value, status = 200, headers = {}) {
  const body = Buffer.from(JSON.stringify(value));
  response.writeHead(status, {
    "access-control-allow-origin": "null",
    "cache-control": "no-store",
    "content-length": body.length,
    "content-type": "application/json",
    ...headers,
  });
  response.end(body);
}

async function serveFile(response, pathname) {
  const relative = pathname === "/apps/marketplace/" ? "index.html" : pathname.slice("/apps/marketplace/".length);
  const path = join(browserRoot, relative);
  assert(path.startsWith(`${browserRoot}/`) || path === join(browserRoot, "index.html"), "invalid Marketplace asset path");
  const body = await readFile(path);
  const contentType = {
    ".css": "text/css",
    ".html": "text/html; charset=utf-8",
    ".js": "text/javascript",
    ".png": "image/png",
    ".woff2": "font/woff2",
  }[extname(path)] || "application/octet-stream";
  response.writeHead(200, {
    "access-control-allow-origin": "null",
    "content-length": body.length,
    "content-type": contentType,
  });
  response.end(body);
}

async function serveCapsuleIcon(response, pathname) {
  const match = pathname.match(/^\/apps\/([A-Za-z0-9_.-]+)\/icons\/(icon-(?:32|64|128|256)\.png)$/);
  assert(match, "invalid capsule icon path", { pathname });
  const path = join(repoRoot, "capsules", match[1], "browser", "icons", match[2]);
  const body = await readFile(path);
  response.writeHead(200, {
    "access-control-allow-origin": "null",
    "content-length": body.length,
    "content-type": "image/png",
  });
  response.end(body);
}

function buildShellDocument(appSrc) {
  return `<!doctype html>
<html>
<body style="margin:0">
  <iframe id="app" title="Apps" sandbox="allow-forms allow-modals allow-pointer-lock allow-scripts" src="${appSrc}" style="border:0;width:100%;height:100vh"></iframe>
  <script>
    window.addEventListener("message", (event) => {
      if (event.data?.type !== "fixture:post-to-app") return;
      document.getElementById("app").contentWindow.postMessage(event.data.payload, "*");
    });
  <\/script>
</body>
</html>`;
}

function buildFixtureHtml(shellSrc) {
  return `<!doctype html>
<html>
<body style="margin:0">
  <iframe id="shell" title="Home shell" sandbox="allow-forms allow-modals allow-pointer-lock allow-scripts" src="${shellSrc}" style="border:0;width:100%;height:100vh"></iframe>
  <script>
    window.homeMessages = [];
    window.addEventListener("message", (event) => {
      window.homeMessages.push({ origin: event.origin, message: event.data });
    });
    window.postToMarketplace = (payload) => {
      document.getElementById("shell").contentWindow.postMessage({ type: "fixture:post-to-app", payload }, "*");
    };
    window.postToMarketplaceDirect = (payload) => {
      const shell = document.getElementById("shell");
      const child = shell.contentWindow?.frames?.[0];
      if (!child) {
        throw new Error("Marketplace child frame is not ready");
      }
      child.postMessage(payload, "*");
    };
  <\/script>
</body>
</html>`;
}

function startServer() {
  const requestLog = [];
  const notFoundPaths = [];
  const state = {
    [errorToken]: { catalogFailuresRemaining: 1 },
    [normalToken]: {
      catalogFailuresRemaining: 0,
      initialCatalogBarrier: createBarrier(),
      initialInterfacesBarrier: createBarrier(),
      initialMediaBarrier: createBarrier(),
      initialCatalogHeld: false,
      initialInterfacesHeld: false,
      initialMediaHeld: false,
      channelsNeedApproval: true,
      // Each seller's offer answers `terms_changed` exactly once (then
      // `complete`), so the sheet's own re-render -- never an automatic
      // retry -- is what the test can falsify. `marketSellerA` gets fresh,
      // still-live terms; `marketSellerB`'s offer is simply gone.
      buyOfferTermsChangedAnswers: new Map([
        [marketSellerA, marketOfferFixture({ seller: marketSellerA, price: "0x4c4b40", quantity: "0x2" })],
        [marketSellerB, null],
      ]),
      // The Explore-sourced listing's own seller never answers past `wait`,
      // so a buy started against it can be left running in the background
      // while the test proves another item's sheet is untouched by its polls.
      buyOfferWaitingSellers: new Set(["0x34daf3000000000000000000000000000000beef"]),
      slowBuyBarrier: createBarrier(),
      exploreListingBarrier: createBarrier(),
      // Two assets nobody on this Home holds a listing for: the market has
      // them, and this Home can buy them and cannot open them.
      catalogItems: [
        {
          ledger: "0x0ebac909d31ef0074495e752c0cf4ea49ba13c41",
          tokenId: "0x1f",
          operative: "0x1aa4dd2cb9c9b784eaccedf5c679fd267cab1033",
          sellerAddress: "0x34daf3000000000000000000000000000000beef",
          contentAccessId: "0x27df70c564295e2fbc1967719ffdb12e",
          contentCid: "Qma6zsq5rQXK1dwtF6xvFSGNvVY8q5yGGUugH3bVTst5m8",
          metadataCid: mediaBareMetadataCid,
          displayName: "Someone Else's Film",
          contentCategory: "video",
          price: "250000",
          payToken: mediaPayToken,
          quantity: 12,
          opType: 1,
          views: 41,
          publishedAt: 1_789_500_000,
          accessState: "available",
        },
        {
          ledger: "0x0ebac909d31ef0074495e752c0cf4ea49ba13c41",
          tokenId: "0x20",
          sellerAddress: "0x34daf3000000000000000000000000000000beef",
          contentAccessId: "0x27df70c564295e2fbc1967719ffdb12f",
          metadataCid: mediaBareMetadataCid,
          displayName: "Someone Else's Paper",
          contentCategory: "document",
          price: "500000",
          payToken: mediaPayToken,
          quantity: 3,
          opType: 1,
          views: 2,
          publishedAt: 1_789_400_000,
          accessState: "available",
        },
      ],
      channelAccess: {
        "0x0ebac909d31ef0074495e752c0cf4ea49ba13c41": "none",
        "0x56d2d76a8e3a1c9551efe1201e15b517f77cb9b0": "administrator",
        "0x1234567890abcdef1234567890abcdef12345678": "unknown",
      },
      channels: [
        {
          address: "0x0ebac909d31ef0074495e752c0cf4ea49ba13c41",
          name: "Test CH-3.0-rc7",
          description: "Protected items",
          categories: ["music"],
          itemsCount: 40,
          imageCid: "QmYtisnG1wCaUCAGp3hi2xKKeaTzMFhvxyFf1crWaTyoRH",
        },
        {
          address: "0x56d2d76a8e3a1c9551efe1201e15b517f77cb9b0",
          itemsCount: 0,
        },
        // A channel the chain could not be read for. It exists in this fixture
        // so that "unknown must not draw a Subscribe button" is a claim the
        // run can actually falsify.
        {
          address: "0x1234567890abcdef1234567890abcdef12345678",
          name: "Unreadable Channel",
          itemsCount: 3,
        },
      ],
      mediaListings: [
        mediaListing({
          mintId: mediaCreatorMint,
          displayName: "Creator Video",
          accessState: "creator",
          quantity: "0x2",
          price: "0x5",
        }),
        mediaListing({
          mintId: mediaPurchasedMint,
          displayName: "Owned Video",
          accessState: "purchased",
          quantity: "0x3",
          price: "0x6",
        }),
        mediaListing({
          mintId: mediaAvailableMint,
          displayName: "Store Video",
          accessState: "available",
          quantity: "0x4",
          // 123 base units at six decimals: a mantissa with more than one
          // digit, so that dropping digits to shorten it would be visible.
          price: "0x7b",
        }),
        // A shelf worth grouping. The song is the newest of all, so it also
        // proves the order inside a section is the item's own age and not the
        // order Runtime happened to list them in.
        {
          ...mediaListing({
            mintId: mediaSongMint,
            displayName: "A Song.mp3",
            accessState: "available",
            quantity: "0x1",
            price: "0x9",
            codecs: "mp4a.40.2",
          }),
          metadata_cid: mediaBareMetadataCid,
          // Priced in the chain's own coin, so the card wears its mark.
          pay_token: "0x0000000000000000000000000000000000000000",
          mime_type: "audio/mpeg",
          published_at: 1_789_000_000,
        },
        {
          ...mediaListing({
            mintId: mediaSoldOutMint,
            displayName: "Nothing Left.mp4",
            accessState: "available",
            quantity: "0x0",
            price: "0x186a0",
          }),
          // The oldest video, so the three a shelf shows are the ones the
          // purchase flows below drive. Its own state is checked with the
          // section opened.
          published_at: 1_600_000_000,
        },
        {
          ...mediaListing({
            mintId: mediaPaperMint,
            displayName: "A Paper.pdf",
            // Listed by this Home, so it carries the secondary actions a
            // document folds behind one control.
            accessState: "creator",
            quantity: "0x2",
            // 12345000. Shortening it would mean "12345K", which is longer
            // than it is short, so the card must show it in full.
            price: "0xbc5ea8",
            codecs: "",
          }),
          metadata_cid: mediaBareMetadataCid,
          content_kind: "object",
          mime_type: "application/pdf",
          published_at: 1_700_000_000,
        },
      ],
    },
    [mediaErrorToken]: { catalogFailuresRemaining: 0 },
    [mediaMalformedToken]: { catalogFailuresRemaining: 0 },
    [mediaEmptyToken]: { catalogFailuresRemaining: 0 },
    [marketRetryToken]: firstLoadScenario(1),
    [marketDownToken]: firstLoadScenario(Number.POSITIVE_INFINITY),
    [mediaPendingBuyToken]: {
      catalogFailuresRemaining: 0,
      mediaListings: [
        mediaListing({
          mintId: mediaAvailableMint,
          displayName: "Store Video",
          accessState: "available",
          quantity: "0x4",
          price: "0x8",
        }),
        {
          ...mediaListing({
            mintId: mediaImportedMint,
            displayName: "Half-bought Video",
            accessState: "available",
            quantity: "0x5",
            price: "0x9",
          }),
          purchase_in_flight: true,
        },
      ],
    },
  };
  const server = createServer(async (request, response) => {
    try {
      const url = new URL(request.url, "http://localhost");
      if (request.method === "OPTIONS") {
        response.writeHead(204, {
          "access-control-allow-headers": "content-type,x-elastos-home-token",
          "access-control-allow-methods": "GET,POST,OPTIONS",
          "access-control-allow-origin": "null",
        }).end();
        return;
      }
      if (url.pathname === "/favicon.ico") {
        response.writeHead(204).end();
        return;
      }
      if (url.pathname === "/fixture-normal") {
        const topOrigin = `http://${request.headers.host}`;
        const appSrc = `/apps/marketplace/?home_origin=${encodeURIComponent(topOrigin)}#home_token=${encodeURIComponent(normalToken)}`;
        const shellSrc = `/fixture-shell?app_src=${encodeURIComponent(appSrc)}`;
        const body = Buffer.from(buildFixtureHtml(shellSrc));
        response.writeHead(200, { "content-length": body.length, "content-type": "text/html; charset=utf-8" });
        response.end(body);
        return;
      }
      if (url.pathname === "/fixture-error") {
        const topOrigin = `http://${request.headers.host}`;
        const appSrc = `/apps/marketplace/?home_origin=${encodeURIComponent(topOrigin)}#home_token=${encodeURIComponent(errorToken)}`;
        const shellSrc = `/fixture-shell?app_src=${encodeURIComponent(appSrc)}`;
        const body = Buffer.from(buildFixtureHtml(shellSrc));
        response.writeHead(200, { "content-length": body.length, "content-type": "text/html; charset=utf-8" });
        response.end(body);
        return;
      }
      if (url.pathname === "/fixture-media-error") {
        const topOrigin = `http://${request.headers.host}`;
        const appSrc = `/apps/marketplace/?home_origin=${encodeURIComponent(topOrigin)}#home_token=${encodeURIComponent(mediaErrorToken)}`;
        const shellSrc = `/fixture-shell?app_src=${encodeURIComponent(appSrc)}`;
        const body = Buffer.from(buildFixtureHtml(shellSrc));
        response.writeHead(200, { "content-length": body.length, "content-type": "text/html; charset=utf-8" });
        response.end(body);
        return;
      }
      if (url.pathname === "/fixture-media-pending-buy") {
        const topOrigin = `http://${request.headers.host}`;
        const appSrc = `/apps/marketplace/?home_origin=${encodeURIComponent(topOrigin)}#home_token=${encodeURIComponent(mediaPendingBuyToken)}`;
        const shellSrc = `/fixture-shell?app_src=${encodeURIComponent(appSrc)}`;
        const body = Buffer.from(buildFixtureHtml(shellSrc));
        response.writeHead(200, { "content-length": body.length, "content-type": "text/html; charset=utf-8" });
        response.end(body);
        return;
      }
      if (url.pathname === "/fixture-media-malformed") {
        const topOrigin = `http://${request.headers.host}`;
        const appSrc = `/apps/marketplace/?home_origin=${encodeURIComponent(topOrigin)}#home_token=${encodeURIComponent(mediaMalformedToken)}`;
        const shellSrc = `/fixture-shell?app_src=${encodeURIComponent(appSrc)}`;
        const body = Buffer.from(buildFixtureHtml(shellSrc));
        response.writeHead(200, { "content-length": body.length, "content-type": "text/html; charset=utf-8" });
        response.end(body);
        return;
      }
      if (url.pathname === "/fixture-media-empty") {
        const topOrigin = `http://${request.headers.host}`;
        const appSrc = `/apps/marketplace/?home_origin=${encodeURIComponent(topOrigin)}#home_token=${encodeURIComponent(mediaEmptyToken)}`;
        const shellSrc = `/fixture-shell?app_src=${encodeURIComponent(appSrc)}`;
        const body = Buffer.from(buildFixtureHtml(shellSrc));
        response.writeHead(200, { "content-length": body.length, "content-type": "text/html; charset=utf-8" });
        response.end(body);
        return;
      }
      if (url.pathname.startsWith("/fixture-token/")) {
        const topOrigin = `http://${request.headers.host}`;
        const fixtureToken = decodeURIComponent(url.pathname.slice("/fixture-token/".length));
        const appSrc = `/apps/marketplace/?home_origin=${encodeURIComponent(topOrigin)}#home_token=${encodeURIComponent(fixtureToken)}`;
        const shellSrc = `/fixture-shell?app_src=${encodeURIComponent(appSrc)}`;
        const body = Buffer.from(buildFixtureHtml(shellSrc));
        response.writeHead(200, { "content-length": body.length, "content-type": "text/html; charset=utf-8" });
        response.end(body);
        return;
      }
      if (url.pathname === "/fixture-shell") {
        const appSrc = url.searchParams.get("app_src") || "";
        const body = Buffer.from(buildShellDocument(appSrc));
        response.writeHead(200, { "content-length": body.length, "content-type": "text/html; charset=utf-8" });
        response.end(body);
        return;
      }
      if (url.pathname.startsWith("/apps/marketplace/")) {
        await serveFile(response, url.pathname);
        return;
      }
      if (/^\/apps\/[A-Za-z0-9_.-]+\/icons\/icon-(?:32|64|128|256)\.png$/.test(url.pathname)) {
        await serveCapsuleIcon(response, url.pathname);
        return;
      }
      if (url.pathname === "/api/capsules/catalog") {
        const token = String(request.headers["x-elastos-home-token"] || "");
        requestLog.push({ path: url.pathname, token });
        if (token === normalToken && !state[normalToken].initialCatalogHeld) {
          state[normalToken].initialCatalogHeld = true;
          await state[normalToken].initialCatalogBarrier.promise;
        }
        if (state[token]?.catalogFailuresRemaining > 0) {
          state[token].catalogFailuresRemaining -= 1;
          json(response, { message: "provider launch failed" }, 500);
          return;
        }
        json(response, { capsules: catalogCapsules });
        return;
      }
      // The market, as Runtime hands it over: the index's dialect already
      // converted, and each item joined against what this Home holds.
      if (url.pathname === "/api/apps/marketplace/items") {
        const token = String(request.headers["x-elastos-home-token"] || "");
        requestLog.push({ path: url.pathname, token });
        const scenario = state[token] || {};
        // The first read of a fresh page is held so the loading state can be
        // seen, then answered the way an installed Runtime answers a market
        // it timed out on: 200, `unavailable`, and no items.
        if (scenario.firstMarketBarrier && !scenario.firstMarketHeld) {
          scenario.firstMarketHeld = true;
          await scenario.firstMarketBarrier.promise;
        }
        if (scenario.marketFailuresRemaining > 0) {
          scenario.marketFailuresRemaining -= 1;
          json(response, {
            asOf: 1_790_000_000,
            unavailable: true,
            needsApproval: false,
            items: [],
            note: "market directory request failed: error sending request",
          });
          return;
        }
        if (scenario.catalogUnavailable) {
          json(response, {
            asOf: 1_790_000_000,
            unavailable: true,
            needsApproval: false,
            items: [],
            note: "market directory returned 502: bad gateway",
          });
          return;
        }
        if (scenario.catalogNeedsApproval) {
          json(response, { asOf: 1_790_000_000, unavailable: true, needsApproval: true, items: [] });
          return;
        }
        json(response, {
          asOf: 1_790_000_000,
          unavailable: false,
          needsApproval: false,
          items: scenario.catalogItems || [],
        });
        return;
      }
      // The Runtime-built `ListingObject` (task-9-brief, task-10-brief): any
      // common identifier -- a `token_uri`, or a chain `item` -- converges on
      // one listing. `legacy_listing: true` is a link Runtime still answers
      // through the old `import_runtime_custody` path, unchanged.
      if (url.pathname === "/api/apps/marketplace/listing") {
        const token = String(request.headers["x-elastos-home-token"] || "");
        const body = await readJsonBody(request);
        requestLog.push({ path: url.pathname, token, method: request.method, body });
        const start = (body && body.start) || {};
        if (start.token_uri === importedListingUri) {
          json(response, { legacy_listing: true });
          return;
        }
        if (start.token_uri === marketAssetLink) {
          json(response, marketListingFixture());
          return;
        }
        if (start.token_uri === marketForeignAssetLink) {
          json(response, marketForeignListingFixture());
          return;
        }
        if (start.item && start.item.ledger === marketExploreLedger && start.item.token_id === marketExploreTokenId) {
          const scenario = state[token] || {};
          // Held once, so the test can see the card say it is working while
          // the listing is read.
          if (scenario.exploreListingBarrier && !scenario.exploreListingHeld) {
            scenario.exploreListingHeld = true;
            await scenario.exploreListingBarrier.promise;
          }
          // Runtime answers from the chain: once the market row says this
          // Home bought the film, so does its listing.
          json(response, {
            ...marketExploreListingFixture(),
            access_state: scenario.catalogItems?.[0]?.accessState === "purchased" ? "purchased" : "available",
          });
          return;
        }
        // Explore's second film, once bought: an item minted elsewhere, whose
        // key custody this Home does not share, so no copy of it can exist
        // here. Details on its card reads this listing.
        if (start.item && start.item.ledger === marketExploreLedger && start.item.token_id === marketExplorePaperTokenId) {
          json(response, marketExplorePaperListingFixture());
          return;
        }
        // The page cuts `ipfs://<cid>/<file>` back to its folder before it
        // asks, so only the bare folder form is answered here.
        if (start.token_uri === `ipfs://${marketAssetLinkCid}`) {
          json(response, marketListingFixture());
          return;
        }
        const refusals = new Map([
          [marketMismatchLink, [409, "asset_mismatch"]],
          [marketUnavailableLink, [503, "unavailable"]],
          [marketInvalidStartLink, [400, "invalid_start"]],
        ]);
        if (refusals.has(start.token_uri)) {
          const [status, code] = refusals.get(start.token_uri);
          json(response, { code }, status);
          return;
        }
        const listings = new Map([
          [marketScenarioLink, marketScenarioListingFixture],
          [marketNullRaceLink, marketNullRaceListingFixture],
          [marketGoneRecordedLink, marketGoneRecordedListingFixture],
          [marketUnknownLink, marketUnknownListingFixture],
          [marketPurchasedLink, () => marketHeldListingFixture(marketPurchasedLink, "e", "purchased")],
          [marketCreatorLink, () => marketHeldListingFixture(marketCreatorLink, "f", "creator")],
        ]);
        if (listings.has(start.token_uri)) {
          json(response, listings.get(start.token_uri)());
          return;
        }
        json(response, { code: "unbound" }, 404);
        return;
      }
      // `buy_offer`: the market-item op behind the offer sheet's own Buy.
      // A waiting seller never resolves. A seller in
      // `buyOfferTermsChangedAnswers` answers `terms_changed` exactly once
      // (with its own mapped `current`, possibly `null`), then `complete`,
      // mirroring how `/api/provider/object/buy` resolves on its second ask.
      if (url.pathname === "/api/provider/object/buy_offer") {
        const token = String(request.headers["x-elastos-home-token"] || "");
        const body = await readJsonBody(request);
        requestLog.push({ path: url.pathname, token, method: request.method, body });
        const scenario = state[token] || {};
        const seller = String(body?.seller || "");
        if (scenario.buyOfferWaitingSellers?.has(seller)) {
          json(response, {
            status: "error",
            code: "library_error",
            message: "Runtime custody purchase is pending exact Wallet or Chain settlement",
            buy_progress: {
              schema: "elastos.protected-content.buy-progress/v1",
              stage: "chain_settlement",
              resumable: true,
              awaits_person: false,
              connector_id: "",
            },
          });
          return;
        }
        const bare = new Map([
          [marketSellerOwn, "own_offer"],
          [marketSellerOwned, "already_owned"],
          [marketSellerMismatch, "asset_mismatch"],
        ]);
        if (bare.has(seller)) {
          json(response, buyOfferRefusal(bare.get(seller)));
          return;
        }
        // Runtime could not reach the market or the chain: the envelope
        // `provider_error_from` builds from `purchase_unavailable!` (R48).
        if (seller === marketSellerUnreachable) {
          json(response, buyOfferRefusal("library_error", {
            message: "Runtime custody purchase is unavailable",
            detail: "gateway_marketplace_buy.rs:488: HTTP 429 Too Many Requests",
          }));
          return;
        }
        if (seller === marketSellerDeclined) {
          json(response, buyOfferRefusal("library_error", {
            buy_progress: {
              schema: "elastos.protected-content.buy-progress/v1",
              stage: "declined",
              resumable: false,
              awaits_person: false,
              connector_id: "",
            },
          }));
          return;
        }
        // Held until the test releases it, then `terms_changed` to $8 once.
        if (seller === marketSellerSlow && !scenario.slowBuyAnswered) {
          scenario.slowBuyAnswered = true;
          await scenario.slowBuyBarrier.promise;
          json(response, buyOfferRefusal("terms_changed", {
            current: marketOfferFixture({ seller: marketSellerSlow, price: "0x7a1200" }),
          }));
          return;
        }
        // A purchase is already recorded for `marketSellerRecorded` at $4:
        // any other seller, or that seller at any other price, is answered
        // `attempt_in_progress` naming the recorded offer.
        if (seller === marketSellerRace
          || seller === marketSellerGoneOther
          || (seller === marketSellerRecorded && body?.agreed?.price !== marketRecordedOffer.price)) {
          json(response, buyOfferRefusal("attempt_in_progress", { current: marketRecordedOffer }));
          return;
        }
        if (seller === marketSellerNullRace || seller === marketSellerNullOther) {
          json(response, buyOfferRefusal("attempt_in_progress", { current: null }));
          return;
        }
        const already = (scenario.buyOfferAnsweredSellers ||= new Set());
        if (scenario.buyOfferTermsChangedAnswers?.has(seller) && !already.has(seller)) {
          already.add(seller);
          json(response, {
            status: "error",
            code: "terms_changed",
            current: scenario.buyOfferTermsChangedAnswers.get(seller),
          });
          return;
        }
        json(response, {
          status: "ok",
          data: {
            schema: "elastos.marketplace.buy-offer-complete/v1",
            item: {
              chain_namespace: body?.item?.chain_namespace,
              network: body?.item?.network,
              ledger: body?.item?.ledger,
              token_id: body?.item?.token_id,
              operative: marketListingOperative,
              // R46: a listing sent without a kid completes with a null one.
              kid: body?.asset_uri === marketUnknownLink ? null : body?.item?.kid || marketListingKid,
            },
            asset_uri: body?.asset_uri,
            // How far adoption got decides what the page may promise.
            adoption: body?.asset_uri === marketForeignAssetLink
              ? "foreign"
              : seller === marketSellerRecorded ? "pending" : "adopted",
          },
        });
        return;
      }
      if (url.pathname === "/api/apps/marketplace/pay-tokens") {
        requestLog.push({ path: url.pathname });
        json(response, {
          payTokens: [
            { symbol: "USDC", address: mediaPayToken, decimals: 6 },
            { symbol: "ETH", address: "0x0000000000000000000000000000000000000000", decimals: 18 },
          ],
        });
        return;
      }
      if (url.pathname === "/api/apps/marketplace/channels") {
        const token = String(request.headers["x-elastos-home-token"] || "");
        requestLog.push({ path: url.pathname, token });
        const scenario = state[token] || {};
        // The answer this surface must tell apart: an unapproved read, an
        // index that could not be reached, and a market with channels in it.
        if (scenario.channelsNeedApproval) {
          json(response, {
            asOf: 1_790_000_000,
            stale: false,
            unavailable: true,
            needsApproval: true,
            channels: [],
            note: "external market directory HTTP source is not approved",
          });
          return;
        }
        json(response, {
          asOf: 1_790_000_000,
          stale: false,
          unavailable: false,
          needsApproval: false,
          channels: scenario.channels || [],
        });
        return;
      }
      if (url.pathname === "/api/apps/marketplace/channel-access") {
        const token = String(request.headers["x-elastos-home-token"] || "");
        const body = await readJsonBody(request);
        requestLog.push({ path: url.pathname, token, method: request.method, body });
        const scenario = state[token] || {};
        json(response, {
          unavailable: false,
          channels: (body?.channels || []).map((channel) => ({
            channel,
            state: (scenario.channelAccess || {})[channel] || "unknown",
          })),
        });
        return;
      }
      // The published metadata document, which the shelf reads a title and a
      // cover from. Served from this Home's own CID route, exactly as the
      // gateway serves it.
      if (url.pathname.endsWith("/metadata.json")) {
        requestLog.push({ path: url.pathname });
        // Only one item has a published document here. The others fall back
        // to the file name Runtime already sent, which is what a shelf shows
        // for anything minted before covers existed.
        if (!url.pathname.startsWith(`/ipfs/${mediaMetadataCid}/`)) {
          response.writeHead(404, {
            "access-control-allow-origin": "null",
            "cache-control": "no-store",
            "content-length": 0,
          });
          response.end();
          return;
        }
        json(response, {
          schema: "elacity.asset/v1",
          name: "Big Buck Bunny",
          description: "A rabbit, at some length.",
          image: `ipfs://${mediaCoverCid}`,
        });
        return;
      }
      if (url.pathname.startsWith("/ipfs/")) {
        const token = String(request.headers["x-elastos-home-token"] || "");
        requestLog.push({ path: url.pathname, token });
        // A 1x1 PNG, served the way this Home serves any CID.
        const png = Buffer.from(
          "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
          "base64",
        );
        response.writeHead(200, {
          "access-control-allow-origin": "null",
          "cache-control": "private, max-age=300",
          "content-length": png.length,
          "content-type": "image/png",
          "x-content-type-options": "nosniff",
        });
        response.end(png);
        return;
      }
      if (url.pathname === "/api/capsules/interfaces") {
        const token = String(request.headers["x-elastos-home-token"] || "");
        requestLog.push({ path: url.pathname, token });
        if (token === normalToken && !state[normalToken].initialInterfacesHeld) {
          state[normalToken].initialInterfacesHeld = true;
          await state[normalToken].initialInterfacesBarrier.promise;
        }
        json(response, { interfaces: interfaceEntries });
        return;
      }
      if (url.pathname === "/api/provider/object/list_runtime_custody") {
        const token = String(request.headers["x-elastos-home-token"] || "");
        const body = await readJsonBody(request);
        requestLog.push({ path: url.pathname, token, method: request.method, body });
        if (token === normalToken && !state[normalToken].initialMediaHeld) {
          state[normalToken].initialMediaHeld = true;
          await state[normalToken].initialMediaBarrier.promise;
        }
        if (token === mediaErrorToken) {
          json(response, { message: "runtime service unavailable" }, 500);
          return;
        }
        if (token === mediaMalformedToken) {
          json(response, {
            status: "ok",
            data: mediaListResponse([
              {
                ...mediaListing({
                  mintId: mediaCreatorMint,
                  displayName: "Broken Video",
                  accessState: "creator",
                  quantity: "0x1",
                  price: "0x2",
                }),
                access_state: "unknown",
              },
            ]),
          });
          return;
        }
        if (token === mediaEmptyToken) {
          json(response, { status: "ok", data: mediaListResponse([]) });
          return;
        }
        json(response, { status: "ok", data: mediaListResponse(state[token]?.mediaListings || []) });
        return;
      }
      if (url.pathname === "/api/provider/object/download_owned_copy") {
        const token = String(request.headers["x-elastos-home-token"] || "");
        const body = await readJsonBody(request);
        requestLog.push({ path: url.pathname, token, method: request.method, body });
        json(response, {
          status: "ok",
          data: {
            schema: "elastos.library.runtime-custody-download/v1",
            // A market item's copy is named by its item; Runtime answers
            // with the mint adoption gave it.
            mint_id: body?.mint_id || "9".repeat(64),
            capsule_uri: "localhost://Users/test/Pictures/Owned Video.ddrm",
            acquisition: "bought",
          },
        });
        return;
      }
      if (url.pathname === "/api/provider/object/import_runtime_custody") {
        const token = String(request.headers["x-elastos-home-token"] || "");
        const body = await readJsonBody(request);
        requestLog.push({ path: url.pathname, token, method: request.method, body });
        if (token === normalToken && body?.listing_uri === importedListingUri) {
          state[normalToken].mediaListings = [
            ...state[normalToken].mediaListings,
            {
              ...mediaListing({
                mintId: mediaImportedMint,
                displayName: "Imported Video",
                accessState: "available",
                quantity: "0x9",
                price: "0xa",
              }),
              // An item that just arrived is the newest thing on the shelf,
              // which is also where a person looks for it.
              published_at: 1_800_000_000,
            },
          ];
          json(response, {
            status: "ok",
            data: {
              schema: "elastos.library.runtime-custody-import/v1",
              listing_uri: importedListingUri,
              mint_id: mediaImportedMint,
              status: "verified",
            },
          });
          return;
        }
        json(response, { message: "runtime import unavailable" }, 500);
        return;
      }
      if (url.pathname === "/api/provider/object/buy") {
        const token = String(request.headers["x-elastos-home-token"] || "");
        const body = await readJsonBody(request);
        requestLog.push({ path: url.pathname, token, method: request.method, body });
        if (token === mediaPendingBuyToken) {
          json(response, {
            status: "error",
            code: "library_error",
            message: "Runtime custody purchase is pending exact Wallet or Chain settlement",
            buy_progress: {
              schema: "elastos.protected-content.buy-progress/v1",
              stage: "purchase_approval",
              resumable: true,
              awaits_person: true,
              external_signer: true,
              connector_id: "wallet-metamask",
            },
          });
          return;
        }
        if (token === normalToken && body?.mint_id === mediaAvailableMint) {
          state[normalToken].mediaListings = state[normalToken].mediaListings.map((listing) =>
            listing.mint_id === mediaAvailableMint
              ? { ...listing, access_state: "purchased" }
              : listing,
          );
          json(response, { status: "ok", data: { status: "accepted" } });
          return;
        }
        json(response, { message: "runtime purchase unavailable" }, 500);
        return;
      }
      notFoundPaths.push(url.pathname);
      response.writeHead(404).end("not found");
    } catch (error) {
      response.writeHead(500, { "content-type": "text/plain; charset=utf-8" });
      response.end(error.stack || String(error));
    }
  });
  return { notFoundPaths, requestLog, server, state };
}

async function waitForMarketplaceFrame(page) {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    const frame = page.frames().find((entry) => entry.url().includes("/apps/marketplace/"));
    if (frame) {
      return frame;
    }
    await page.waitForTimeout(100);
  }
  throw new Error(`Marketplace iframe did not appear\n${JSON.stringify(page.frames().map((entry) => entry.url()), null, 2)}`);
}

async function readHomeMessages(page) {
  return page.evaluate(() =>
    window.homeMessages.map((entry) => ({
      origin: entry.origin,
      type: entry.message?.type || "",
      target: entry.message?.target || "",
      query: entry.message?.query || null,
      menus: entry.message?.menus || null,
      homeToken: entry.message?.homeToken || "",
      keys: Object.keys(entry.message || {}).sort(),
    })),
  );
}

async function waitForFrameWidth(frame, expectedWidth) {
  await frame.waitForFunction((width) => window.innerWidth === width, expectedWidth);
}

async function waitForToastText(frame, text) {
  await frame.waitForFunction(
    (expected) => document.querySelector(".toast")?.textContent === expected,
    text,
  );
}

async function waitForRequestCount(requestLog, token, path, count) {
  const deadline = Date.now() + 5_000;
  while (Date.now() < deadline) {
    const current = requestLog.filter((entry) => entry.token === token && entry.path === path).length;
    if (current >= count) {
      return;
    }
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 10));
  }
  const current = requestLog.filter((entry) => entry.token === token && entry.path === path).length;
  throw new Error(`Timed out waiting for ${path}`, { cause: { token, path, expected: count, current } });
}

async function assertNoHorizontalOverflow(frame, label) {
  const overflow = await frame.evaluate(() => ({
    doc: document.documentElement.scrollWidth - document.documentElement.clientWidth,
    main: (() => {
      const node = document.getElementById("store-main");
      return node ? node.scrollWidth - node.clientWidth : 0;
    })(),
    shell: (() => {
      const node = document.querySelector(".store-shell");
      return node ? node.scrollWidth - node.clientWidth : 0;
    })(),
  }));
  assert(overflow.doc <= 1, `${label}: document overflowed horizontally`, overflow);
  assert(overflow.main <= 1, `${label}: main overflowed horizontally`, overflow);
  assert(overflow.shell <= 1, `${label}: shell overflowed horizontally`, overflow);
}

async function run() {
  const { notFoundPaths, requestLog, server, state } = startServer();
  const browser = await chromium.launch({ executablePath: brave, headless: true });
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const pageErrors = [];
  const consoleErrors = [];
  const nonOkResponses = [];
  const requestFailures = [];
  try {
    await new Promise((resolveListen, rejectListen) => {
      server.once("error", rejectListen);
      server.listen(0, "127.0.0.1", () => resolveListen());
    });
    const address = server.address();
    assert(address && typeof address === "object", "fixture server did not bind");
    const port = address.port;

    page.on("pageerror", (error) => {
      pageErrors.push(error?.stack || String(error));
    });
    page.on("console", (message) => {
      if (message.type() === "error") {
        consoleErrors.push(message.text());
      }
    });
    page.on("response", async (response) => {
      if (response.status() < 400) {
        return;
      }
      const request = response.request();
      const headers = request.headers();
      nonOkResponses.push({
        url: response.url(),
        status: response.status(),
        token: headers["x-elastos-home-token"] || "",
        method: request.method(),
      });
    });
    page.on("requestfailed", (request) => {
      requestFailures.push({
        url: request.url(),
        method: request.method(),
        error: request.failure()?.errorText || "unknown",
      });
    });

    await page.goto(`http://127.0.0.1:${port}/fixture-normal`);
    const frame = await waitForMarketplaceFrame(page);
    await frame.locator(".store-row-skeleton").first().waitFor();
    await waitForRequestCount(requestLog, normalToken, "/api/capsules/catalog", 1);
    await waitForRequestCount(requestLog, normalToken, "/api/capsules/interfaces", 1);
    await waitForRequestCount(requestLog, normalToken, "/api/provider/object/list_runtime_custody", 1);
    state[normalToken].initialCatalogBarrier.release();
    state[normalToken].initialInterfacesBarrier.release();
    await frame.locator(".store-row").filter({ hasText: "People" }).first().waitFor();
    const discoverTextWhileMediaPending = await frame.locator("#store-main").textContent();
    assert(
      /People/.test(discoverTextWhileMediaPending || ""),
      "Marketplace must render Discover when catalog requests finish even while Media is still pending",
      { discoverTextWhileMediaPending },
    );
    await frame.locator('[data-destination="media"]').click();
    await frame.locator(".store-row-skeleton").first().waitFor();
    const mediaRowsWhilePending = await frame.locator(".store-row-media").count();
    assert(mediaRowsWhilePending === 0, "Marketplace Media must keep its own loading state while listings are pending", { mediaRowsWhilePending });
    state[normalToken].initialMediaBarrier.release();
    await frame.locator('.store-row-media').first().waitFor();
    await frame.locator('[data-destination="discover"]').click();
    await frame.locator('.store-row[data-app="people"]').first().waitFor();

    const initialMessages = await readHomeMessages(page);
    assert(initialMessages[0]?.type === "home:app-ready", "Marketplace must announce Home readiness first", initialMessages);
    assert(initialMessages[1]?.type === "home:menu-manifest", "Marketplace must publish its menu after readiness", initialMessages);
    assert(initialMessages[1]?.menus?.[0]?.title === "File", "Marketplace menu must use the canonical title/items shape", initialMessages[1]);
    assert(initialMessages[1]?.menus?.[1]?.title === "View", "Marketplace must expose the View menu", initialMessages[1]);

    const normalCatalogRequests = requestLog.filter((entry) => entry.token === normalToken && entry.path === "/api/capsules/catalog").length;
    const normalInterfaceRequests = requestLog.filter((entry) => entry.token === normalToken && entry.path === "/api/capsules/interfaces").length;
    const initialMediaRequest = requestLog.find((entry) => entry.token === normalToken && entry.path === "/api/provider/object/list_runtime_custody");
    assert(
      normalCatalogRequests === 1 && normalInterfaceRequests === 1,
      "Marketplace must read the canonical catalog routes once on first load",
      requestLog,
    );
    assert(
      initialMediaRequest?.method === "POST" && JSON.stringify(initialMediaRequest.body) === "{}",
      "Marketplace must load protected media through the typed list_runtime_custody request",
      initialMediaRequest,
    );

    const peopleIconVisible = await frame.locator('.store-row[data-app="people"] .app-icon-img').first().getAttribute("src");
    assert(peopleIconVisible === "/apps/people/icons/icon-128.png", "Marketplace must prefer the declared 128px icon route", { peopleIconVisible });

    const badIconHasImage = await frame.locator('.store-row[data-app="bad-icons"] .app-icon-img').first().count();
    assert(badIconHasImage === 0, "Marketplace must reject malformed or cross-capsule icon routes");
    assert(
      !requestLog.some((entry) => entry.path === "/apps/not-allowed/icons/icon-128.png"),
      "Marketplace must not request rejected icon routes",
      requestLog,
    );

    const installedBadge = await frame.locator("#installed-badge").textContent();
    assert(installedBadge?.trim() === "4", "Marketplace installed count must come from catalog installed fields", { installedBadge });

    const discoverTitles = await frame.locator(".store-section-title").evaluateAll((nodes) => nodes.map((node) => node.textContent?.trim() || ""));
    assert(discoverTitles.includes("Installed"), "Marketplace Discover must include the Installed section", discoverTitles);
    assert(discoverTitles.includes("Apps"), "Marketplace Discover must include app category sections", discoverTitles);
    assert(discoverTitles.includes("Services"), "Marketplace Discover must include service category sections", discoverTitles);

    await frame.locator('.store-see-all[data-destination="providers"]').click();
    await frame.locator("#store-title").waitFor({ state: "visible" });
    const providerTitle = await frame.locator("#store-title").textContent();
    assert(providerTitle?.trim() === "Services", "Marketplace See All must route through the category destination", { providerTitle });

    await frame.locator('[data-destination="discover"]').click();
    await frame.locator("#search-input").fill("documents");
    const visibleRows = await frame.locator(".store-row").evaluateAll((nodes) =>
      nodes
        .filter((node) => node.offsetParent !== null)
        .map((node) => node.textContent?.trim() || "")
        .filter(Boolean),
    );
    assert(
      visibleRows.length >= 1 && visibleRows.every((row) => /Documents/.test(row)),
      "Marketplace search must filter the current catalog rows",
      visibleRows,
    );
    await frame.locator("#search-input").fill("");

    await frame.locator('[data-destination="media"]').click();
    await frame.locator('.store-row-media').first().waitFor();
    const mediaTitle = await frame.locator("#store-title").textContent();
    assert(mediaTitle?.trim() === "Media", "Marketplace must expose the Media destination", { mediaTitle });
    const mediaRows = await frame.locator(".store-row-media").evaluateAll((nodes) =>
      nodes.map((node) => node.textContent?.trim() || ""),
    );
    assert(
      mediaRows.some((row) => /Creator Video/.test(row) && /2 available/.test(row) && /0\.0₅5/.test(row))
        // A uint256 is a hex string all the way to the moment it is shown.
        // Whatever the card's wording, none of it may be a raw one.
        && mediaRows.every((row) => !/0x[0-9a-f]+\s*(units|available)/.test(row)),
      "Marketplace media cards must present canonical uint256 listing values in decimal",
      mediaRows,
    );
    // A person searches for what the card shows them, so the price is matched
    // in the form they read as well as in full.
    await frame.locator("#search-input").fill("0.0₅5");
    const decimalSearchRows = await frame.locator(".store-row-media").evaluateAll((nodes) =>
      nodes.map((node) => node.textContent?.trim() || ""),
    );
    assert(
      decimalSearchRows.length === 1 && /Creator Video/.test(decimalSearchRows[0]),
      "Marketplace media search must match the price a person can see",
      decimalSearchRows,
    );
    await frame.locator("#search-input").fill("");

    // The shelf is grouped by what each item IS, newest first, and a section
    // exists only when it has something in it.
    const sectionTitles = await frame.locator("#store-sections .section-head h2").evaluateAll(
      (nodes) => nodes.map((node) => node.textContent?.trim() || ""),
    );
    assert(
      // Each section says how many it holds, which is the design's own way of
      // telling a shelf of three from a shelf of thirty.
      // Five videos and two documents: this Home's own, plus the market's.
      sectionTitles.join("|") === "Videos 5|Audio 1|Documents 2",
      "Explore must group items by kind, in order, with counts and no empty section",
      sectionTitles,
    );
    const videoTitles = await frame
      .locator("#store-sections .section:has(.section-head h2:has-text('Videos')) .card-title")
      .evaluateAll((nodes) => nodes.map((node) => node.textContent?.trim() || ""));
    // A section shows three and offers the rest, so a shelf fits on screen.
    assert(
      videoTitles.length === 3
        && (await frame.locator('.section:has(.section-head h2:has-text("Videos")) [data-action="explore-more"]').count()) === 1,
      "A section shows three items and offers the rest behind See all",
      videoTitles,
    );
    assert(
      (await frame.locator('.section:has(.section-head h2:has-text("Audio")) [data-action="explore-more"]').count()) === 0,
      "A section with nothing held back offers no See all",
    );
    const musicTitles = await frame
      .locator("#store-sections .section:has(.section-head h2:has-text('Audio')) .card-title")
      .evaluateAll((nodes) => nodes.map((node) => node.textContent?.trim() || ""));
    assert(
      musicTitles.length === 1 && musicTitles[0] === "A Song.mp3",
      "A song belongs to Audio and to nothing else",
      musicTitles,
    );
    // Explore is the market, not this Home's shelf: an item nobody here holds
    // a listing for still has a card, and it offers to buy.
    // This Home's own items join the market's rather than standing in for it.
    await frame.locator('.card-title:text-is("Someone Else\'s Film")').waitFor();
    const foreignTitles = await frame.locator(".card-title").evaluateAll(
      (nodes) => nodes.map((node) => node.textContent?.trim() || ""),
    );
    assert(
      foreignTitles.includes("Someone Else's Film"),
      "Explore must show what other people minted",
      foreignTitles,
    );
    // An item known only to the index has no file name, so the line under its
    // title says what the card is otherwise missing rather than repeating it.
    const foreignMeta = await frame
      .locator('.card:has(.card-title:text-is("Someone Else\'s Film")) .card-meta')
      .textContent();
    assert(
      !/Someone Else's Film/.test(foreignMeta || "")
        && /12 available/.test(foreignMeta || "")
        && /by 0x34daf3/.test(foreignMeta || ""),
      "A card must not repeat its own title in the line beneath it",
      { foreignMeta },
    );
    // ...and My listings is this Home's own, which never includes them.
    await frame.locator('[data-media-tab="mine"]').click();
    await frame.locator(".card").first().waitFor();
    const mineTitles = await frame.locator(".card-title").evaluateAll(
      (nodes) => nodes.map((node) => node.textContent?.trim() || ""),
    );
    assert(
      !mineTitles.includes("Someone Else's Film") && mineTitles.length > 0,
      "My listings must hold only what this Home listed",
      mineTitles,
    );
    await frame.locator('[data-media-tab="explore"]').click();
    await frame.locator(".card").first().waitFor();

    // A cover that has landed leaves no shimmer behind it, and one that never
    // arrives leaves the placeholder rather than a frame that shimmers for
    // ever.
    const covers = await frame.locator(".card .thumb").evaluateAll((nodes) => nodes.map((node) => ({
      pending: node.classList.contains("thumb-pending"),
      img: node.querySelector("img")?.getAttribute("data-cover") || "",
    })));
    assert(
      covers.length > 0 && covers.every((cover) => !cover.pending && cover.img !== "pending"),
      "A settled shelf must leave no cover still loading",
      covers,
    );

    // A market that cannot be read is not an empty market, and the shelf says
    // which it is rather than leaving a person to guess from what is missing.
    state[normalToken].catalogUnavailable = true;
    await frame.locator('[data-action="refresh-media"]').click();
    await frame.locator('#store-sections [data-action="retry-market"]').waitFor();
    const downNotice = await frame.locator('#store-sections [role="status"]').first().textContent();
    assert(
      /Couldn’t reach the market\./.test(downNotice || "")
        && (await frame.locator(".store-row-media").count()) === 0,
      "An unreachable market must say so on Explore and offer Try again, not pass this Home's own items off as the market",
      { downNotice },
    );
    state[normalToken].catalogUnavailable = false;

    // Asking again is a control, and it says what it found rather than just
    // spinning: a market that cannot be read must not look like an empty one.
    await frame.locator('[data-action="refresh-media"]').click();
    await frame.locator("#toast", { hasText: "yours." }).waitFor();
    const refreshToast = await frame.locator("#toast").textContent();
    assert(
      /\d+ items? · \d+ yours\./.test(refreshToast || ""),
      "Refresh must say what it found",
      { refreshToast },
    );

    // A published document gives an item the title its creator typed and the
    // cover they chose; an item without one keeps the file name Runtime sent.
    const titled = await frame.locator(`.card[data-mint="${mediaCreatorMint}"]`).evaluate((node) => ({
      title: node.querySelector(".card-title")?.textContent?.trim() || "",
      meta: node.querySelector(".card-meta")?.textContent?.trim() || "",
      cover: node.querySelector(".app-icon-img")?.getAttribute("src") || "",
    }));
    // The line under the title names the file, because that differs from the
    // title and says which copy this is.
    assert(
      titled.title === "Big Buck Bunny"
        && /Creator Video/.test(titled.meta)
        && titled.cover === `/ipfs/${mediaCoverCid}`,
      "A published document gives a card its title and cover, with the file still named beneath",
      titled,
    );
    const untitled = await frame.locator(`.card[data-mint="${mediaSongMint}"]`).evaluate((node) => ({
      title: node.querySelector(".card-title")?.textContent?.trim() || "",
      cover: node.querySelector(".app-icon-img")?.getAttribute("src") || "",
    }));
    assert(
      untitled.title === "A Song.mp3" && untitled.cover === "",
      "An item with no published document keeps its file name and its placeholder",
      untitled,
    );

    // Open Videos in full, so every state below is on screen: a shelf shows
    // three, and these assertions are about the cards rather than the shelf.
    await frame.locator('.section:has(.section-head h2:has-text("Videos")) [data-action="explore-more"]').click();
    await frame.locator(`.card[data-mint="${mediaAvailableMint}"]`).waitFor();

    // From here the checks are about cards rather than about the shelf, so
    // Videos is opened in full: a shelf shows three of a kind on purpose.
    const openVideosInFull = async () => {
      await frame.locator('[data-media-tab="explore"]').click();
      await frame.locator(".card").first().waitFor();
      const more = frame.locator('.section:has(.section-head h2:has-text("Videos")) [data-action="explore-more"]');
      if (await more.count()) {
        await more.click();
      }
      await frame.locator(`.card[data-mint="${mediaAvailableMint}"]`).waitFor();
    };
    await openVideosInFull();

    // Each state names itself on the card, rather than leaving a person to
    // work it out from what is missing.
    // Pressing the tab a person is already on takes them back to the whole
    // shelf, which is what the helper below relies on.
    //
    // A shelf shows three of a kind and holds the rest behind See all, so a
    // check about one card brings that card into view first. Which section it
    // lives in is the shelf's business, not the test's.
    const ensureCard = async (mint) => {
      const card = frame.locator(`.card[data-mint="${mint}"]`);
      if (await card.count()) {
        return;
      }
      await frame.locator('[data-media-tab="explore"]').click();
      await frame.locator(".card").first().waitFor();
      if (await card.count()) {
        return;
      }
      const sections = await frame.locator('[data-action="explore-more"]').count();
      for (let index = 0; index < sections; index += 1) {
        await frame.locator('[data-media-tab="explore"]').click();
        await frame.locator(".card").first().waitFor();
        await frame.locator('[data-action="explore-more"]').nth(index).click();
        if (await card.count()) {
          return;
        }
      }
    };
    const cardState = async (mint) => {
      await ensureCard(mint);
      return frame.locator(`.card[data-mint="${mint}"]`).evaluate((node) => ({
      format: node.querySelector(".tag.left")?.textContent?.trim() || "",
      badge: node.querySelector(".tag.right")?.textContent?.trim() || "",
      units: node.querySelector(".price:not(.owned)")?.textContent?.replace(/\s+/g, " ").trim() || "",
      unitsTitle: node.querySelector(".price")?.getAttribute("title") || "",
      play: node.querySelectorAll(".play").length,
      stock: node.querySelector(".stock")?.textContent?.trim() || "",
      owned: node.querySelector(".price.owned")?.textContent?.trim() || "",
      note: node.querySelector(".btn-ghost[disabled]")?.textContent?.trim() || "",
      actions: [...node.querySelectorAll(".card-actions [data-action]")].map((c) => c.dataset.action),
      text: node.textContent.replace(/\s+/g, " ").trim(),
      }));
    };

    // The same facts for a card named by its selector or locator, for items
    // this Home holds no mint for.
    const ensureCardBy = async (selector) => {
      const card = frame.locator(selector);
      if (await card.count()) {
        return;
      }
      await frame.locator('[data-media-tab="explore"]').click();
      await frame.locator(".card").first().waitFor();
      if (await card.count()) {
        return;
      }
      const sections = await frame.locator('[data-action="explore-more"]').count();
      for (let index = 0; index < sections; index += 1) {
        await frame.locator('[data-media-tab="explore"]').click();
        await frame.locator(".card").first().waitFor();
        await frame.locator('[data-action="explore-more"]').nth(index).click();
        if (await card.count()) {
          return;
        }
      }
    };
    const catalogCardState = (locator) => locator.evaluate((node) => ({
      badge: node.querySelector(".tag.right")?.textContent?.trim() || "",
      owned: node.querySelector(".price.owned")?.textContent?.trim() || "",
      stock: node.querySelector(".stock")?.textContent?.trim() || "",
      play: node.querySelectorAll(".play").length,
      actions: [...node.querySelectorAll(".card-actions [data-action]")].map((c) => c.dataset.action),
      text: node.textContent.replace(/\s+/g, " ").trim(),
    }));
    // The properties an item sheet shows, by label: what a person reads, the
    // full value in its title, and the markup, to prove text stays text.
    const itemProperties = () => frame.locator("#detail-modal .item-properties").evaluate((section) =>
      Object.fromEntries([...section.querySelectorAll(".item-property")].map((row) => {
        const value = row.querySelector("dd");
        return [row.querySelector("dt")?.textContent?.trim() || "", {
          text: value?.textContent?.trim() || "",
          title: value?.getAttribute("title") || "",
          html: value?.innerHTML || "",
        }];
      })));
    // What else the sheet shows. A full 40-hex address in visible text is an
    // account the page must never print; shortened values keep theirs in a
    // title attribute.
    const sheetFacts = () => frame.locator("#detail-modal").evaluate((modal) => ({
      focus: document.activeElement?.dataset?.action || "",
      offerLists: modal.querySelectorAll(".offer-list").length,
      buyControls: modal.querySelectorAll('[data-action="buy-offer"]').length,
      downloads: modal.querySelectorAll('[data-action="download-market-copy"]').length,
      status: modal.querySelector('[role="status"]')?.textContent || "",
      fullAddresses: modal.innerText.match(/0x[0-9a-fA-F]{40}/g) || [],
    }));

    const creatorState = await cardState(mediaCreatorMint);
    assert(
      creatorState.badge === "Listed by you"
        && creatorState.actions.join("|") === "share-listing|download-copy|open-media",
      "A listing this Home made says so, and offers its link, a rebuild and the item",
      creatorState,
    );
    const ownedState = await cardState(mediaPurchasedMint);
    assert(
      ownedState.badge === "In your library"
        && ownedState.owned === "✓ In your library"
        && ownedState.units === ""
        // An owned card has no stock line: how many are left is a seller's
        // fact, and "Purchased" beside "In your library" says one thing twice.
        && ownedState.stock === ""
        && !/available/.test(ownedState.text)
        && !/Purchased/.test(ownedState.text)
        && ownedState.actions.join("|") === "download-copy|open-media",
      "A copy this Home holds says it is in the Library instead of naming a price or a stock again",
      ownedState,
    );
    const soldOutState = await cardState(mediaSoldOutMint);
    assert(
      soldOutState.badge === "Sold out"
        && soldOutState.stock === "0 available"
        // No control at all, rather than one that cannot work: this Home
        // cannot tell anyone when a copy frees up.
        && soldOutState.actions.length === 0
        && soldOutState.note === "Sold out",
      "An item with nothing left to sell offers nothing, and says why",
      soldOutState,
    );
    // A play control is offered on what this person can open, and on nothing
    // else: on an item still for sale it would promise what the next click
    // cannot do.
    assert(
      creatorState.play === 1 && ownedState.play === 1,
      "An item this Home can open offers to play it from the card",
      { creator: creatorState.play, owned: ownedState.play },
    );
    assert(
      (await cardState(mediaAvailableMint)).play === 0 && soldOutState.play === 0,
      "An item this Home cannot open offers no play control",
    );
    await frame.locator('[data-action="explore-all"]').click();
    await frame.locator(".card").first().waitFor();

    // A document folds its two secondary actions behind one control, because
    // a page leaves less room beside it. The actions are the same either way.
    const paperCard = frame.locator(`.card[data-mint="${mediaPaperMint}"]`);
    assert(
      (await paperCard.locator('[data-action="card-menu"]').count()) === 1
        && (await paperCard.locator('.card-actions > [data-action="download-copy"]').count()) === 0,
      "A document card offers its secondary actions behind one control",
    );
    assert(
      !(await paperCard.locator('[data-action="download-copy"]').isVisible()),
      "A folded menu stays shut until it is asked for",
    );
    await paperCard.locator('[data-action="card-menu"]').click();
    assert(
      (await paperCard.locator('[data-action="download-copy"]').isVisible())
        && (await paperCard.locator('[data-action="card-menu"]').getAttribute("aria-expanded")) === "true",
      "Opening the menu reveals the same actions a video shows plainly",
    );
    await frame.locator("#store-title").click();
    assert(
      !(await paperCard.locator('[data-action="download-copy"]').isVisible())
        && (await paperCard.locator('[data-action="card-menu"]').getAttribute("aria-expanded")) === "false",
      "A click elsewhere closes the menu",
    );
    // A video shows them plainly, in the room it has.
    const videoCard = frame.locator(`.card[data-mint="${mediaCreatorMint}"]`);
    assert(
      (await videoCard.locator('.card-actions > [data-action="download-copy"]').count()) === 1
        && (await videoCard.locator('[data-action="card-menu"]').count()) === 0,
      "A video card shows its secondary actions without folding them",
    );

    const paperState = await cardState(mediaPaperMint);
    assert(
      paperState.format === "PDF" && (await cardState(mediaSongMint)).format === "MP3",
      "A card names what kind of file it is",
      paperState,
    );
    // A document stands a page at the foot of the card; a video fills the
    // frame. A document with no cover still shows a page, with its title on
    // it, rather than a glyph that says only "file".
    // A cover that fails to load leaves the same placeholder as a cover that
    // was never there. Proved by pointing a card at a CID this fake Home does
    // not serve and watching the image stand down.
    const brokenCover = await frame.locator(`.card[data-mint="${mediaCreatorMint}"]`).evaluate(async (node) => {
      const image = node.querySelector(".app-icon-img");
      if (!image) {
        return { had: false };
      }
      const glyph = node.querySelector(".app-icon-glyph");
      image.dispatchEvent(new Event("error"));
      // The computed style, not the attribute. An author `display: block`
      // beats the browser's own `[hidden]` rule, which is how a dead image
      // once stayed on screen beside the glyph that replaced it.
      return {
        had: true,
        imageShown: getComputedStyle(image).display !== "none",
        glyphShown: glyph ? getComputedStyle(glyph).display !== "none" : false,
      };
    });
    assert(
      brokenCover.had && !brokenCover.imageShown && brokenCover.glyphShown,
      "A cover that fails to load must stand down and leave the placeholder",
      brokenCover,
    );

    const paperShape = await frame.locator(`.card[data-mint="${mediaPaperMint}"]`).evaluate((node) => ({
      doc: node.classList.contains("doc"),
      page: node.querySelector(".page-fallback")?.textContent?.trim() || "",
    }));
    assert(
      paperShape.doc && /A Paper\.pdf/.test(paperShape.page),
      "A document card is page-shaped and carries its own title",
      paperShape,
    );
    assert(
      await frame.locator(`.card[data-mint="${mediaCreatorMint}"]`).evaluate(
        (node) => node.classList.contains("video"),
      ),
      "A video card fills its frame",
    );
    // A price is shortened only when the short form is exactly the price, and
    // the exact value travels with it either way.
    assert(
      soldOutState.units === "$0.1" && soldOutState.unitsTitle === "0.1 USDC",
      "A price is shown as money, with the token amount kept exactly",
      soldOutState,
    );
    // A price too small to read in full is written with its significant
    // digits and an exponent -- the same number, short enough for a card --
    // and the exact amount stays on the element.
    const tiny = await cardState(mediaCreatorMint);
    assert(
      tiny.units === "$0.0₅5" && tiny.unitsTitle === "0.000005 USDC",
      "A very small price counts its zeros instead of printing them",
      tiny,
    );

    // Every significant digit survives being shortened.
    const smallMulti = await cardState(mediaAvailableMint);
    assert(
      smallMulti.units === "$0.0₃123" && smallMulti.unitsTitle === "0.000123 USDC",
      "Counting a price's zeros must keep every digit that follows them",
      smallMulti,
    );

    // A currency with a mark wears it instead of its name.
    const ether = await cardState(mediaSongMint);
    assert(
      ether.units === "Ξ0.0₁₇9" && ether.unitsTitle === "0.000000000000000009 ETH",
      "A price in ether is marked, not named",
      ether,
    );

    // ...and a price no shorter for being shortened stays as it is.
    const longPrice = await cardState(mediaPaperMint);
    assert(
      longPrice.units === "$12.345" && longPrice.unitsTitle === "12.345 USDC",
      "A price keeps every digit it has: no rounding on the way to the card",
      longPrice,
    );

    // Every card offers the one control that fits whoever is looking.
    await ensureCard(mediaAvailableMint);
    const available = frame.locator(`.card[data-mint="${mediaAvailableMint}"]`);
    const owned = frame.locator(`.card[data-mint="${mediaPurchasedMint}"]`);
    assert(
      (await available.locator('[data-action="buy-media"]').count()) === 1
        && (await available.locator('[data-action="download-copy"]').count()) === 0,
      "An item this Home does not hold offers Buy, and nothing to download",
    );
    assert(
      (await ensureCard(mediaPurchasedMint), await owned.locator('[data-action="download-copy"]').count()) === 1
        && (await owned.locator('[data-action="buy-media"]').count()) === 0,
      "An item this Home holds offers Download, and nothing to buy",
    );

    // Media is two surfaces behind one title. Switching away from Explore
    // must take the shelf and its Add-a-listing control with it, and coming
    // back must restore both -- the tab is the only thing that changed.
    const tabsVisible = await frame.locator("#media-tabs").isVisible();
    const tabLabels = await frame.locator("#media-tabs [data-media-tab]").evaluateAll((nodes) =>
      nodes.map((node) => node.textContent?.trim() || ""),
    );
    assert(
      tabsVisible && tabLabels.join("|") === "Explore|Shops|My listings",
      "Media must offer Explore, Shops and My listings",
      { tabsVisible, tabLabels },
    );
    assert(
      await frame.locator("#import-listing").isVisible(),
      "Add a listing must be offered on Explore",
    );
    await frame.locator('[data-media-tab="shops"]').click();
    assert(
      !(await frame.locator("#import-listing").isVisible())
        && !(await frame.locator("#media-toolbar").isVisible())
        && (await frame.locator(".store-row-media").count()) === 0
        && (await frame.locator('[data-media-tab="shops"]').getAttribute("aria-selected")) === "true",
      "Shops must replace the shelf and withdraw the Add-a-listing control",
    );

    // An unapproved directory is not an empty market. The surface must say
    // which it is, and offer the only thing that changes it.
    await frame.locator('[data-action="approve-shops"]').waitFor();
    const unapprovedText = await frame.locator("#store-sections").textContent();
    assert(
      /need your approval/i.test(unapprovedText || "")
        && !/No channels listed/i.test(unapprovedText || ""),
      "An unapproved channel directory must not read as an empty market",
      { unapprovedText },
    );
    // Approving happens in the Inbox, so the control says where to go and
    // then becomes the way back.
    await frame.locator('[data-action="approve-shops"]').click();
    const awaitingText = await frame.locator("#store-sections").textContent();
    assert(
      /Inbox/.test(awaitingText || "")
        && (await frame.locator('[data-action="approve-shops"]').textContent())?.trim() === "Check again",
      "The approval control must name the Inbox and then offer a re-check",
      { awaitingText },
    );
    // Once approved, the same control fetches, and the channels render.
    state[normalToken].channelsNeedApproval = false;
    await frame.locator('[data-action="approve-shops"]').click();
    await frame.locator(".store-row-shop").first().waitFor();
    const shopRows = await frame.locator(".store-row-shop").evaluateAll((nodes) =>
      nodes.map((node) => node.textContent?.trim() || ""),
    );
    assert(
      shopRows.length === 3
        && /Test CH-3\.0-rc7/.test(shopRows[0])
        && /40 items/.test(shopRows[0])
        && /Music/.test(shopRows[0]),
      "Shops must name each channel and how much is in it",
      shopRows,
    );
    assert(
      /0x56d2…b9b0|0x56d2/.test(shopRows[1]) && /0 items/.test(shopRows[1]),
      "A channel with no name must still be recognisable by its address",
      shopRows,
    );
    // A channel with a picture shows it, named by the channel rather than by
    // a CID the page was handed; a channel without one keeps its glyph.
    const shopImages = await frame.locator(".store-row-shop .app-icon-img").evaluateAll(
      (nodes) => nodes.map((node) => node.getAttribute("src") || ""),
    );
    assert(
      shopImages.length === 1
        && shopImages[0] === "/ipfs/QmYtisnG1wCaUCAGp3hi2xKKeaTzMFhvxyFf1crWaTyoRH",
      "A channel picture must come from this Home's own CID route",
      shopImages,
    );
    // The CID is fine -- it is what the route takes. A directory URL is not.
    const shopMarkup = await frame.locator("#store-sections").innerHTML();
    assert(
      !/10\.132\.0\.5/.test(shopMarkup)
        && !/thumbnail:/.test(shopMarkup)
        && !/initials:/.test(shopMarkup),
      "No directory URL or sentinel may reach the page",
    );

    // A subscription is offered only to someone who could use one. A channel
    // this Home administers has nothing to sell it, and an unconfirmed state
    // must not draw the button either.
    await frame.locator('.store-row-shop [data-action="subscribe-channel"]').first().waitFor();
    const subscribeButtons = await frame.locator('[data-action="subscribe-channel"]').evaluateAll(
      (nodes) => nodes.map((node) => ({
        shop: node.dataset.shop,
        disabled: node.disabled,
        label: node.textContent?.trim() || "",
      })),
    );
    assert(
      subscribeButtons.length === 1
        && subscribeButtons[0].shop === "0x0ebac909d31ef0074495e752c0cf4ea49ba13c41"
        && subscribeButtons[0].label === "Subscribe",
      "Subscribe must be offered on a channel this Home neither runs nor subscribes to",
      subscribeButtons,
    );
    assert(
      subscribeButtons[0].disabled,
      "Subscribe must not look available while the payment path does not exist",
      subscribeButtons,
    );
    const unknownRowText = await frame
      .locator('.store-row-shop[data-shop="0x1234567890abcdef1234567890abcdef12345678"]')
      .textContent();
    assert(
      /Access unknown/.test(unknownRowText || ""),
      "A channel whose access could not be read must say so, not offer a subscription",
      { unknownRowText },
    );
    const adminRowText = await frame
      .locator('.store-row-shop[data-shop="0x56d2d76a8e3a1c9551efe1201e15b517f77cb9b0"]')
      .textContent();
    assert(
      /Yours/.test(adminRowText || ""),
      "A channel this Home administers must say so instead of offering a subscription",
      { adminRowText },
    );


    // Making a listing is Creator's job. The tile that launches it belongs to
    // My listings: Explore is other people's shelf, and adding to that means
    // buying rather than minting.
    await frame.locator('[data-media-tab="explore"]').click();
    await frame.locator(".card").first().waitFor();
    assert(
      (await frame.locator('[data-action="open-creator"]').count()) === 0,
      "Explore must not offer to mint",
    );
    await frame.locator('[data-media-tab="mine"]').click();
    await frame.locator(".card").first().waitFor();
    const launchesBefore = (await readHomeMessages(page))
      .filter((entry) => entry.type === "home:open-target").length;
    await frame.locator('[data-action="open-creator"]').first().click();
    const afterLaunch = (await readHomeMessages(page)).filter((entry) => entry.type === "home:open-target");
    assert(
      afterLaunch.length === launchesBefore + 1 && afterLaunch.at(-1)?.target === "creator",
      "Add a video must open Creator through Home, exactly once",
      afterLaunch.at(-1),
    );
    await frame.locator('[data-media-tab="explore"]').click();
    await frame.locator('[data-media-tab="shops"]').click();

    // Search reaches Shops too, and reaches it by name.
    await frame.locator("#search-input").fill("rc7");
    assert(
      (await frame.locator(".store-row-shop").count()) === 1,
      "Search must filter the channel list",
    );
    await frame.locator("#search-input").fill("");
    await frame.locator('[data-media-tab="explore"]').click();
    await frame.locator(".store-row-media").first().waitFor();
    assert(
      await frame.locator("#import-listing").isVisible(),
      "Returning to Explore must restore the shelf and its Add-a-listing control",
    );
    // A tab lives in Media. Leaving for Apps and coming back must not leave a
    // tab strip stranded over a catalog that has no tabs.
    await frame.locator('[data-destination="discover"]').click();
    assert(
      !(await frame.locator("#media-tabs").isVisible()),
      "The Media tabs must not follow the person to the catalog",
    );
    await frame.locator('[data-destination="media"]').click();
    await frame.locator(".store-row-media").first().waitFor();

    const mediaPageText = await frame.locator("#store-main").textContent();
    assert(
      !mediaPageText?.includes(mediaCreatorMint)
        && !mediaPageText?.includes(mediaPurchasedMint)
        && !mediaPageText?.includes(mediaAvailableMint),
      "Marketplace must keep mint IDs internal to media actions",
      { mediaPageText },
    );

    const mediaOpenCountBefore = (await readHomeMessages(page)).filter((entry) => entry.type === "home:open-target").length;
    await ensureCard(mediaCreatorMint);
    await frame.locator(`.store-row-media[data-mint="${mediaCreatorMint}"] .card-actions [data-action="open-media"]`).click();
    const mediaOpenMessagesAfterCreator = await readHomeMessages(page);
    const creatorOpen = mediaOpenMessagesAfterCreator.filter((entry) => entry.type === "home:open-target").at(-1);
    assert(
      mediaOpenMessagesAfterCreator.filter((entry) => entry.type === "home:open-target").length === mediaOpenCountBefore + 1,
      "Marketplace creator media rows must open through Home exactly once",
      mediaOpenMessagesAfterCreator,
    );
    assert(
      creatorOpen?.target === "elacity-player"
        && creatorOpen.homeToken === normalToken
        && JSON.stringify(creatorOpen.query) === JSON.stringify({ mint_id: mediaCreatorMint })
        && JSON.stringify(creatorOpen.keys) === JSON.stringify(["homeToken", "query", "target", "type"]),
      "Marketplace creator media rows must send the exact authorized elacity-player launch command",
      creatorOpen,
    );

    const listCountBeforeBuy = requestLog.filter((entry) => entry.token === normalToken && entry.path === "/api/provider/object/list_runtime_custody").length;
    // Buy asks first. Cancelling leaves nothing behind: no request, and the
    // row exactly as it was.
    const buyRequestsBeforeCancel = requestLog.filter((entry) => entry.path === "/api/provider/object/buy").length;
    await ensureCard(mediaAvailableMint);
    await frame.locator(`.store-row-media[data-mint="${mediaAvailableMint}"] [data-action="buy-media"]`).click();
    await frame.locator('[data-action="confirm-buy"]').waitFor();
    // The confirmation opens on Cancel, never on Buy: an Enter or a held
    // key's repeat from the card's Buy must not confirm a spend.
    const confirmFocus = await frame.evaluate(() => ({
      action: document.activeElement?.dataset?.action || "",
      text: document.activeElement?.textContent?.trim() || "",
    }));
    assert(
      confirmFocus.action === "close-detail" && confirmFocus.text === "Cancel",
      "The purchase confirmation must open with focus on Cancel, not on Buy",
      confirmFocus,
    );
    const buyTermsText = await frame.locator("#detail-content").textContent();
    assert(
      /base units/.test(buyTermsText || "")
        && /still listed/.test(buyTermsText || "")
        && /Sold by/.test(buyTermsText || "")
        && /Your wallet asks you to approve the payment/.test(buyTermsText || ""),
      "Marketplace must show what a purchase costs and what happens next before it spends anything",
      { buyTermsText },
    );
    await frame.locator('#detail-content .modal-btn[data-action="close-detail"]').click();
    assert(
      requestLog.filter((entry) => entry.path === "/api/provider/object/buy").length === buyRequestsBeforeCancel,
      "Cancelling the terms must leave the purchase unstarted",
    );

    await ensureCard(mediaAvailableMint);

    await frame.locator(`.store-row-media[data-mint="${mediaAvailableMint}"] [data-action="buy-media"]`).click();
    await frame.locator('[data-action="confirm-buy"]').waitFor();
    const pendingBuyState = await frame.locator('[data-action="confirm-buy"]').evaluate((button) => {
      button.click();
      button.click();
      const activeButton = document.querySelector(`.store-row-media[data-mint="${button.dataset.mint}"] [data-action="buy-media"]`);
      return {
        disabled: activeButton?.disabled === true,
        label: activeButton?.textContent?.trim() || "",
      };
    });
    assert(
      pendingBuyState.disabled && pendingBuyState.label === "Buying...",
      "Marketplace must mark an in-memory media buy as pending before the request settles",
      pendingBuyState,
    );
    await waitForRequestCount(requestLog, normalToken, "/api/provider/object/buy", 1);
    await waitForRequestCount(requestLog, normalToken, "/api/provider/object/list_runtime_custody", listCountBeforeBuy + 1);
    const buyRequests = requestLog.filter((entry) => entry.token === normalToken && entry.path === "/api/provider/object/buy");
    const buyRequest = buyRequests[0];
    assert(
      buyRequests.length === 1
        && buyRequest?.method === "POST"
        && JSON.stringify(buyRequest.body) === JSON.stringify({ mint_id: mediaAvailableMint }),
      "Marketplace rapid media Buy activation must submit one exact typed request",
      buyRequests,
    );
    await ensureCard(mediaAvailableMint);
    await frame.locator(`.store-row-media[data-mint="${mediaAvailableMint}"] .card-actions [data-action="open-media"]`).waitFor();
    const boughtRowText = await frame.locator(`.store-row-media[data-mint="${mediaAvailableMint}"]`).textContent();
    assert(
      /In your library/.test(boughtRowText || "")
        && !/Purchased/.test(boughtRowText || "")
        && !/available/.test(boughtRowText || ""),
      "Marketplace must reload the media list after buy, and the bought copy reads as in the Library with no stock line",
      { boughtRowText },
    );

    // A listing published on another Home arrives by its link. The control
    // belongs to this shelf and appears with it, the link's shape is checked
    // before Runtime is asked, and a verified import puts the item on the
    // shelf without the person reloading anything.
    assert(
      await frame.locator("#import-listing").isVisible(),
      "Marketplace must offer the add-a-listing control on the protected shelf",
    );
    // Both ways the page could ask Runtime about a link: the market listing
    // read it asks first, and the legacy import it falls back to.
    const linkRequestPaths = ["/api/provider/object/import_runtime_custody", "/api/apps/marketplace/listing"];
    const importCountBeforeInvalid = requestLog.filter((entry) => linkRequestPaths.includes(entry.path)).length;
    await frame.locator("#import-listing-uri").fill("https://example.com/not-a-listing");
    await frame.locator("#import-listing-submit").click();
    await frame.locator(".toast").waitFor();
    const invalidImportToast = await frame.locator(".toast").textContent();
    assert(
      /does not look like a listing link/.test(invalidImportToast || ""),
      "Marketplace must refuse a link that is not a listing link",
      { invalidImportToast },
    );
    // A beat for any request the page might still send after the toast.
    await page.waitForTimeout(200);
    assert(
      requestLog.filter((entry) => linkRequestPaths.includes(entry.path)).length === importCountBeforeInvalid,
      "Marketplace must not ask Runtime about a link it already knows is wrong -- neither the listing read nor the import",
    );

    await frame.locator("#import-listing-uri").fill(importedListingUri);
    await frame.locator("#import-listing-submit").click();
    await waitForRequestCount(requestLog, normalToken, "/api/provider/object/import_runtime_custody", 1);
    const importRequest = requestLog.find(
      (entry) => entry.token === normalToken && entry.path === "/api/provider/object/import_runtime_custody",
    );
    assert(
      importRequest?.method === "POST"
        && JSON.stringify(importRequest.body) === JSON.stringify({ listing_uri: importedListingUri }),
      "Marketplace must send the listing link as the one typed request Runtime verifies",
      importRequest,
    );
    await ensureCard(mediaImportedMint);
    await frame.locator(`.store-row-media[data-mint="${mediaImportedMint}"]`).waitFor();
    const importedRowText = await frame.locator(`.store-row-media[data-mint="${mediaImportedMint}"]`).textContent();
    assert(
      // The control says what it does: an item this Home does not hold is
      // bought, and one it holds is opened.
      /Imported Video/.test(importedRowText || "") && /Buy/.test(importedRowText || ""),
      "An imported listing must reach the shelf with a control that buys it",
      { importedRowText },
    );
    assert(
      (await frame.locator("#import-listing-uri").inputValue()) === "",
      "Marketplace must clear the link field once the listing is added",
    );

    // A shared asset link is not necessarily a listing Runtime already
    // recognises by the old path: it may be a live `ListingObject`, which
    // opens the offer sheet directly (task-9-brief) rather than importing
    // anything onto this Home's own shelf.
    await frame.locator("#import-listing-uri").fill(marketAssetLink);
    await frame.locator("#import-listing-submit").click();
    await frame.locator("#detail-modal.active .offer-list").waitFor();
    const offerRowCount = await frame.locator("#detail-modal .offer-row").count();
    assert(offerRowCount === 2, "Pasting an asset link must open the offer sheet with one row per live offer", { offerRowCount });
    // The sheet opens on its Close, never on a Buy (UI-M6).
    assert(
      await frame.evaluate(() => document.activeElement?.classList.contains("modal-close") === true),
      "The offer sheet must open with focus on its Close, not on a Buy",
    );
    // Each Buy names who it buys from and at what price, for a person who
    // cannot see the row around it (UI-M5).
    const buyALabel = await frame
      .locator(`#detail-modal .offer-row[data-seller="${marketSellerA}"] [data-action="buy-offer"]`)
      .getAttribute("aria-label");
    const sellerATitle = await frame.locator(`#detail-modal .offer-row[data-seller="${marketSellerA}"] .offer-row-seller`).getAttribute("title");
    assert(
      buyALabel === `Buy from ${marketSellerA} for 3 USDC` && sellerATitle === marketSellerA,
      "A row's Buy must be labelled with the full seller and the price, and the seller's full address must be its title",
      { buyALabel, sellerATitle },
    );
    const offerPrices = await frame.locator("#detail-modal .offer-row-price").evaluateAll(
      (nodes) => nodes.map((node) => node.textContent?.trim() || ""),
    );
    assert(
      offerPrices.some((text) => /\$3\b/.test(text)) && offerPrices.some((text) => /\$2\b/.test(text)),
      "The offer sheet must show each live offer's own price",
      offerPrices,
    );

    // A seller whose offer is simply gone (`current: null`) loses its Buy
    // control, without its row vanishing -- and, unlike the case below, there
    // is nothing left to press a second time.
    const buyOfferRequestsBefore = requestLog.filter((entry) => entry.path === "/api/provider/object/buy_offer").length;
    await frame.locator(`#detail-modal .offer-row[data-seller="${marketSellerB}"] [data-action="buy-offer"]`).click();
    await waitForRequestCount(requestLog, normalToken, "/api/provider/object/buy_offer", buyOfferRequestsBefore + 1);
    await frame.locator(`#detail-modal .offer-row[data-seller="${marketSellerB}"]:has-text("This offer is no longer available")`).waitFor();
    assert(
      (await frame.locator(`#detail-modal .offer-row[data-seller="${marketSellerB}"] [data-action="buy-offer"]`).count()) === 0,
      "A terms_changed answer of current: null must leave that row's Buy gone, not clickable",
    );
    // A gone offer says only that (UI-I3): no quantity it no longer has, no
    // "new terms" it never got, and no invitation to review and confirm.
    const goneRowText = await frame.locator(`#detail-modal .offer-row[data-seller="${marketSellerB}"]`).textContent();
    const goneSheetText = await frame.locator("#detail-modal").textContent();
    assert(
      !/\d+ available|new terms|\$2/.test(goneRowText || "")
        && !/Review and confirm/.test(goneSheetText || "")
        && (await frame.locator(`#detail-modal .offer-row[data-seller="${marketSellerB}"].offer-row-changed`).count()) === 0,
      "A gone offer's row must say it is no longer available and nothing that contradicts it",
      { goneRowText, goneSheetText },
    );
    const toastAfterGone = await frame.locator(".toast").textContent().catch(() => "");
    assert(!/Bought/.test(toastAfterGone || ""), "An offer going away must never be shown as a successful purchase", { toastAfterGone });

    // A seller who moved their terms between the sheet opening and Buy being
    // pressed is answered `terms_changed`: the sheet re-renders with the
    // fresh terms, and nothing here retries on its own or claims success.
    const buyOfferRequestsBeforeA = requestLog.filter((entry) => entry.path === "/api/provider/object/buy_offer").length;
    await frame.locator(`#detail-modal .offer-row[data-seller="${marketSellerA}"] [data-action="buy-offer"]`).click();
    await waitForRequestCount(requestLog, normalToken, "/api/provider/object/buy_offer", buyOfferRequestsBeforeA + 1);
    const buyOfferRequest = requestLog.filter((entry) => entry.path === "/api/provider/object/buy_offer").at(-1);
    assert(
      buyOfferRequest?.method === "POST"
        && buyOfferRequest.body?.seller === marketSellerA
        && buyOfferRequest.body?.asset_uri === marketAssetLink
        // `agreed.quantity` is always "0x1" in this slice (shared-context.md
        // §5.2) -- never the seller's own available quantity ("0x3").
        && JSON.stringify(buyOfferRequest.body?.agreed) === JSON.stringify({ price: "0x2dc6c0", pay_token: mediaPayToken, quantity: "0x1" }),
      "Buying an offer must send the item, the seller and exactly the agreed terms, with agreed.quantity fixed at 0x1",
      buyOfferRequest,
    );
    await frame.locator("#detail-modal .offer-row-changed").waitFor();
    const termsChangedText = await frame.locator("#detail-modal").textContent();
    assert(
      /The seller changed the terms\. Review and confirm\./.test(termsChangedText || "")
        && /\$5\b/.test(termsChangedText || ""),
      "A terms_changed answer must re-render the sheet with the seller's fresh terms",
      { termsChangedText },
    );
    // The change is announced (UI-I4): the note is a status region, and the
    // repaint hands focus back to the control the person just pressed
    // rather than dropping it to the page behind the sheet.
    const termsNoteInStatus = await frame
      .locator('#detail-modal [role="status"]', { hasText: "The seller changed the terms. Review and confirm." })
      .count();
    const focusAfterTermsChanged = await frame.evaluate(() => ({
      seller: document.activeElement?.closest(".offer-row")?.dataset.seller || "",
      tag: document.activeElement?.tagName || "",
    }));
    assert(
      termsNoteInStatus === 1 && focusAfterTermsChanged.seller === marketSellerA && focusAfterTermsChanged.tag === "BUTTON",
      "A terms_changed note must be a status region, and focus must return to the same seller's control after the repaint",
      { termsNoteInStatus, focusAfterTermsChanged },
    );
    // The in-sheet note is repainted; the persistent toast is what a screen
    // reader is told.
    await waitForToastText(frame, "The seller changed the terms. Review and confirm.");
    const toastAfterTermsChanged = await frame.locator(".toast").textContent().catch(() => "");
    assert(
      !/Bought/.test(toastAfterTermsChanged || ""),
      "A terms_changed answer must never be shown as a successful purchase",
      { toastAfterTermsChanged },
    );
    // The row still offers Buy, for a person to confirm the new terms by
    // hand -- what must never happen is this page pressing it for them.
    await page.waitForTimeout(300);
    assert(
      requestLog.filter((entry) => entry.path === "/api/provider/object/buy_offer").length === buyOfferRequestsBeforeA + 1,
      "A terms_changed answer must never be retried automatically",
    );

    // Pressing Buy again on the re-rendered row must send exactly the fresh
    // terms Runtime just re-read -- never the superseded ones the first
    // press was refused for.
    await frame.locator(`#detail-modal .offer-row[data-seller="${marketSellerA}"] [data-action="buy-offer"]`).click();
    await waitForRequestCount(requestLog, normalToken, "/api/provider/object/buy_offer", buyOfferRequestsBeforeA + 2);
    const secondBuyOfferRequest = requestLog.filter((entry) => entry.path === "/api/provider/object/buy_offer").at(-1);
    assert(
      JSON.stringify(secondBuyOfferRequest?.body?.agreed) === JSON.stringify({ price: "0x4c4b40", pay_token: mediaPayToken, quantity: "0x1" }),
      "A second Buy on the re-rendered row must send the new price, never the terms Runtime already refused",
      secondBuyOfferRequest,
    );
    await frame.waitForFunction(() => /Bought/.test(document.querySelector(".toast")?.textContent || ""));
    // Adopted: only now is it in this Home's Library (UI-I2).
    const adoptedToast = await frame.locator(".toast").textContent();
    assert(
      adoptedToast === "Bought. The item is in your Library.",
      "An adopted market purchase must say it is in the Library",
      { adoptedToast },
    );
    assert(
      !(await frame.locator("#detail-modal").evaluate((node) => node.classList.contains("active"))),
      "A completed purchase must close the sheet that was showing it",
    );

    // Readability is advisory (shared-context.md D8): a foreign asset still
    // opens the same sheet, and says only where it opens.
    await frame.locator("#import-listing-uri").fill(marketForeignAssetLink);
    await frame.locator("#import-listing-submit").click();
    await frame.locator("#detail-modal.active .offer-list").waitFor();
    const foreignSheetText = await frame.locator("#detail-modal").textContent();
    assert(
      /Opens on ela\.city/.test(foreignSheetText || ""),
      "A foreign listing's sheet must say it opens on ela.city",
      { foreignSheetText },
    );
    await frame.locator('#detail-modal [data-action="close-detail"]').first().click();

    // Task 10: Explore's own Buy reaches the same sheet through the chain's
    // own key -- never an account, never a mint this Home does not hold.
    const listingRequestsBeforeExplore = requestLog.filter((entry) => entry.path === "/api/apps/marketplace/listing").length;
    await frame.locator('[data-media-tab="explore"]').click();
    await frame.locator('.card-title:text-is("Someone Else\'s Film")').waitFor();
    const exploreFilmCard = frame.locator('.card:has(.card-title:text-is("Someone Else\'s Film"))');
    await exploreFilmCard.locator('[data-action="buy-listing"]').click();
    await waitForRequestCount(requestLog, normalToken, "/api/apps/marketplace/listing", listingRequestsBeforeExplore + 1);
    // While the listing is read the card says it is working, and a second
    // press asks nothing more (UI-M11).
    const exploreBuyBusy = await exploreFilmCard.locator('[data-action="buy-listing"]').getAttribute("aria-busy");
    // A press on a control marked busy still reaches the page's handler.
    await exploreFilmCard.locator('[data-action="buy-listing"]').dispatchEvent("click");
    await page.waitForTimeout(200);
    assert(
      exploreBuyBusy === "true"
        && requestLog.filter((entry) => entry.path === "/api/apps/marketplace/listing").length === listingRequestsBeforeExplore + 1,
      "Explore's Buy must show it is busy while the listing is read, and a second press must not read it again",
      { exploreBuyBusy },
    );
    state[normalToken].exploreListingBarrier.release();
    const exploreListingRequest = requestLog.filter((entry) => entry.path === "/api/apps/marketplace/listing").at(-1);
    assert(
      exploreListingRequest?.method === "POST"
        && JSON.stringify(exploreListingRequest.body) === JSON.stringify({
          start: { item: { ledger: marketExploreLedger, token_id: marketExploreTokenId } },
        }),
      "Explore's Buy must request the listing by the row's own ledger and token id, and nothing else",
      exploreListingRequest,
    );
    await frame.locator("#detail-modal.active .offer-list").waitFor();
    assert(
      (await frame.locator("#detail-modal .offer-row").count()) === 1,
      "Explore's Buy must open the same offer sheet a pasted link opens",
    );

    // Cross-item race: this seller's offer never answers past `wait`, so
    // starting a buy on it and closing the sheet leaves a poll loop running
    // in the background. What it must never do is repaint whatever sheet is
    // open by the time its next tick lands -- proven here by opening a
    // different item's sheet and letting a tick pass over it undisturbed.
    const exploreBuyOfferRequestsBefore = requestLog.filter((entry) => entry.path === "/api/provider/object/buy_offer").length;
    await frame.locator('#detail-modal .offer-row [data-action="buy-offer"]').click();
    await waitForRequestCount(requestLog, normalToken, "/api/provider/object/buy_offer", exploreBuyOfferRequestsBefore + 1);
    await frame.locator('#detail-modal [data-action="close-detail"]').first().click();

    await frame.locator("#import-listing-uri").fill(marketForeignAssetLink);
    await frame.locator("#import-listing-submit").click();
    await frame.locator("#detail-modal.active .offer-list").waitFor();
    const foreignSheetTextBeforeTick = await frame.locator("#detail-modal").textContent();
    // Longer than marketplace.js's own `BUY_POLL_MS` (4000ms), so at least
    // one background tick for the Explore item has landed by the time this
    // reads the foreign item's sheet again.
    await page.waitForTimeout(4500);
    const foreignSheetTextAfterTick = await frame.locator("#detail-modal").textContent();
    assert(
      foreignSheetTextAfterTick === foreignSheetTextBeforeTick
        && /Opens on ela\.city/.test(foreignSheetTextAfterTick || "")
        && (await frame.locator("#detail-modal .offer-row").count()) === 1,
      "A background buy still polling for one item must never repaint another item's open sheet",
      { foreignSheetTextBeforeTick, foreignSheetTextAfterTick },
    );
    await frame.locator('#detail-modal [data-action="close-detail"]').first().click();

    // --- The market sheet, answer by answer ------------------------------
    const pasteLink = async (link) => {
      await frame.locator("#import-listing-uri").fill(link);
      await frame.locator("#import-listing-submit").click();
    };
    const waitForToast = async (text) => {
      await frame.waitForFunction(
        (expected) => document.querySelector(".toast")?.textContent === expected,
        text,
      );
    };
    const pendingAdoptionToast = "Bought. Use Download copy to add it to your Library here.";
    const offerRow = (seller) => frame.locator(`#detail-modal .offer-row[data-seller="${seller}"]`);
    const buyOfferCount = () => requestLog.filter((entry) => entry.path === "/api/provider/object/buy_offer").length;
    const pressBuy = async (seller) => {
      const before = buyOfferCount();
      await offerRow(seller).locator('[data-action="buy-offer"]').click();
      await waitForRequestCount(requestLog, normalToken, "/api/provider/object/buy_offer", before + 1);
    };
    const closeSheet = async () => {
      await frame.locator('#detail-modal [data-action="close-detail"]').first().click();
    };

    // A foreign item bought is not in this Home's Library (UI-I2).
    await pasteLink(marketForeignAssetLink);
    await frame.locator("#detail-modal.active .offer-list").waitFor();
    await pressBuy(marketSellerA);
    await waitForToast("Bought. It opens on ela.city.");

    // `ipfs://<cid>/<file>` is the folder's link with a file named inside
    // it: the page asks for the folder (UI-M16).
    const listingCountBeforeIpfs = requestLog.filter((entry) => entry.path === "/api/apps/marketplace/listing").length;
    await pasteLink(`ipfs://${marketAssetLinkCid}/metadata.json`);
    await frame.locator("#detail-modal.active .offer-list").waitFor();
    const ipfsListingRequest = requestLog.filter((entry) => entry.path === "/api/apps/marketplace/listing").at(-1);
    assert(
      requestLog.filter((entry) => entry.path === "/api/apps/marketplace/listing").length === listingCountBeforeIpfs + 1
        && JSON.stringify(ipfsListingRequest?.body) === JSON.stringify({ start: { token_uri: `ipfs://${marketAssetLinkCid}` } }),
      "An ipfs:// link naming a file in the folder must be read as the folder",
      ipfsListingRequest,
    );
    await closeSheet();

    // Each `/listing` refusal is worded for a person (UI-M8, UI-M15).
    for (const [link, toast] of [
      [marketMismatchLink, "That link doesn’t match this item."],
      [marketUnboundLink, "This isn’t an item this market can sell."],
      [marketUnavailableLink, "The market couldn’t be reached. Try again."],
      [marketInvalidStartLink, "That listing could not be loaded."],
    ]) {
      await pasteLink(link);
      await waitForToast(toast);
    }
    assert(
      !(await frame.locator("#detail-modal").evaluate((node) => node.classList.contains("active"))),
      "A refused listing read must not open a sheet",
    );

    // Readability unknown and kid null (R46): the sheet says nothing about
    // where the item opens, every Buy stays enabled, and the request names
    // no kid. A Buy refused as unavailable says the market could not be
    // reached, not that the purchase failed (R48).
    await pasteLink(marketUnknownLink);
    await frame.locator("#detail-modal.active .offer-list").waitFor();
    const unknownSheet = await frame.locator("#detail-modal").evaluate((node) => ({
      text: node.textContent || "",
      buys: [...node.querySelectorAll('[data-action="buy-offer"]')]
        .map((button) => button.getAttribute("aria-disabled") || ""),
    }));
    assert(
      !/Opens on ela\.city|Not yet verified to open here/.test(unknownSheet.text)
        && unknownSheet.buys.length === 2
        && unknownSheet.buys.every((disabled) => disabled === ""),
      "An item of unknown readability must say nothing about where it opens and keep every Buy enabled",
      unknownSheet,
    );
    await pressBuy(marketSellerUnreachable);
    await waitForToast("The market couldn’t be reached. Try again.");
    const unreachableRequest = requestLog.filter((entry) => entry.path === "/api/provider/object/buy_offer").at(-1);
    assert(
      unreachableRequest?.body?.item && !("kid" in unreachableRequest.body.item)
        && (await offerRow(marketSellerUnreachable).locator('[data-action="buy-offer"]').count()) === 1,
      "A Buy of an item whose kid is null must name no kid, and a Buy refused as unavailable must stay pressable",
      unreachableRequest,
    );
    await pressBuy(marketSellerUnknownOk);
    await waitForToast("Bought. The item is in your Library.");

    // The scenario item: one seller per typed refusal.
    await pasteLink(marketScenarioLink);
    await frame.locator("#detail-modal.active .offer-list").waitFor();
    const scenarioText = await frame.locator("#detail-modal").textContent();
    const unknownTokenTitle = await offerRow(marketSellerDeclined).locator(".offer-row-price").getAttribute("title");
    assert(
      /Showing the first 32 sellers\./.test(scenarioText || "")
        && (unknownTokenTitle || "").includes(marketUnknownPayToken),
      "A truncated offer list must say so, and an unknown pay token must be named in the price's title",
      { scenarioText, unknownTokenTitle },
    );
    for (const [seller, toast] of [
      [marketSellerOwn, "You can’t buy your own offer."],
      [marketSellerOwned, "You already own this."],
      [marketSellerMismatch, "This listing no longer matches the item."],
      [marketSellerDeclined, "You declined this in your wallet."],
    ]) {
      await pressBuy(seller);
      await waitForToast(toast);
    }

    // A buy whose answer is still on its way holds every other seller's Buy
    // (UI-DEF1), and its answer lands on the sheet that is showing -- even
    // one closed and reopened onto a fresh listing meanwhile (UI-DEF2).
    await pressBuy(marketSellerSlow);
    await closeSheet();
    await pasteLink(marketScenarioLink);
    await frame.locator("#detail-modal.active .offer-list").waitFor();
    const lockedWhileSlow = await frame.locator("#detail-modal .offer-row").evaluateAll((rows) => rows.map((row) => ({
      seller: row.dataset.seller,
      buy: row.querySelectorAll('[data-action="buy-offer"]').length,
      text: row.textContent || "",
    })));
    assert(
      lockedWhileSlow.every((row) => row.buy === 0)
        && lockedWhileSlow.filter((row) => row.seller !== marketSellerSlow)
          .every((row) => /Another purchase of this item is in progress/.test(row.text)),
      "While one seller's purchase runs, every other seller's Buy must be held and say why",
      lockedWhileSlow,
    );
    await offerRow(marketSellerSlow).locator("button").focus();
    state[normalToken].slowBuyBarrier.release();
    await frame.locator('#detail-modal [role="status"]', { hasText: "The seller changed the terms. Review and confirm." }).waitFor();
    const slowRowText = await offerRow(marketSellerSlow).textContent();
    const focusAfterSlow = await frame.evaluate(() => document.activeElement?.closest(".offer-row")?.dataset.seller || "");
    assert(
      /\$8\b/.test(slowRowText || "") && !/\$6\b/.test(slowRowText || "") && /new terms/.test(slowRowText || ""),
      "New terms answered after the sheet was reopened must reach the reopened sheet, not only the closed one",
      { slowRowText },
    );
    assert(
      focusAfterSlow === marketSellerSlow,
      "The repaint must hand focus back to the same seller's control",
      { focusAfterSlow },
    );

    // Another purchase of this item is already recorded, for a different
    // seller at different terms (R28): the recorded seller's row is marked as
    // the one in progress, every other Buy is held and says why, and
    // Continue resumes it on exactly the recorded terms.
    await pressBuy(marketSellerRace);
    await frame.locator(`#detail-modal .offer-row-in-progress[data-seller="${marketSellerRecorded}"]`).waitFor();
    await waitForToast("Another purchase of this item is in progress.");
    const afterRace = await frame.locator("#detail-modal .offer-row").evaluateAll((rows) => rows.map((row) => ({
      seller: row.dataset.seller,
      inProgress: row.classList.contains("offer-row-in-progress"),
      buy: [...row.querySelectorAll('[data-action="buy-offer"]')].map((node) => node.textContent?.trim()),
      text: row.textContent || "",
    })));
    const recordedRow = afterRace.find((row) => row.seller === marketSellerRecorded);
    assert(
      recordedRow?.inProgress
        && /Purchase in progress/.test(recordedRow.text)
        && /\$4\b/.test(recordedRow.text)
        && JSON.stringify(recordedRow.buy) === JSON.stringify(["Continue"])
        && afterRace.filter((row) => row.seller !== marketSellerRecorded).every((row) =>
          row.buy.length === 0 && !row.inProgress && /Another purchase of this item is in progress/.test(row.text)),
      "attempt_in_progress must mark the recorded seller's row and hold every other row's Buy",
      afterRace,
    );
    await pressBuy(marketSellerRecorded);
    const continueRequest = requestLog.filter((entry) => entry.path === "/api/provider/object/buy_offer").at(-1);
    assert(
      continueRequest?.body?.seller === marketSellerRecorded
        && JSON.stringify(continueRequest.body?.agreed) === JSON.stringify({ price: marketRecordedOffer.price, pay_token: mediaPayToken, quantity: "0x1" }),
      "Continue must resume the recorded purchase on exactly the recorded terms",
      continueRequest,
    );
    // Bought, and adoption still pending (UI-I2, R45): the toast names the
    // control that finishes it rather than promising it arrives by itself.
    await waitForToast(pendingAdoptionToast);

    // R44: after a reload the page learns of a recorded purchase only from
    // `attempt_in_progress`, and that purchase's seller may no longer list
    // the item. Its row still appears, on the recorded terms, with Continue,
    // because only a press on exactly those terms moves it on.
    await pasteLink(marketGoneRecordedLink);
    await frame.locator("#detail-modal.active .offer-list").waitFor();
    assert(
      (await offerRow(marketSellerRecorded).count()) === 0,
      "The recorded seller must start with no row on this sheet",
    );
    await pressBuy(marketSellerGoneOther);
    await frame.locator(`#detail-modal .offer-row-in-progress[data-seller="${marketSellerRecorded}"]`).waitFor();
    await waitForToast("Another purchase of this item is in progress.");
    const goneRecordedRows = await frame.locator("#detail-modal .offer-row").evaluateAll((rows) => rows.map((row) => ({
      seller: row.dataset.seller,
      buy: [...row.querySelectorAll('[data-action="buy-offer"]')].map((node) => node.textContent?.trim()),
      text: row.textContent || "",
    })));
    const goneRecordedRow = goneRecordedRows.find((row) => row.seller === marketSellerRecorded);
    const goneOtherRow = goneRecordedRows.find((row) => row.seller === marketSellerGoneOther);
    assert(
      goneRecordedRows.length === 2
        && /Purchase in progress/.test(goneRecordedRow?.text || "")
        && /\$4\b/.test(goneRecordedRow?.text || "")
        && JSON.stringify(goneRecordedRow?.buy) === JSON.stringify(["Continue"])
        && goneOtherRow?.buy.length === 0
        && /Another purchase of this item is in progress/.test(goneOtherRow?.text || ""),
      "A recorded purchase whose seller no longer lists the item must still offer Continue on its recorded terms",
      goneRecordedRows,
    );
    assert(
      await frame.evaluate(() => document.activeElement?.dataset?.action !== "buy-offer"),
      "The repaint must not move focus onto a control that spends money",
    );
    await pressBuy(marketSellerRecorded);
    const goneContinueRequest = requestLog.filter((entry) => entry.path === "/api/provider/object/buy_offer").at(-1);
    assert(
      goneContinueRequest?.body?.seller === marketSellerRecorded
        && goneContinueRequest.body?.item?.ledger === `0x${"9".repeat(40)}`
        && JSON.stringify(goneContinueRequest.body?.agreed) === JSON.stringify({ price: marketRecordedOffer.price, pay_token: mediaPayToken, quantity: "0x1" }),
      "Continue on a recorded purchase with no live row must send exactly the recorded seller and terms",
      goneContinueRequest,
    );
    await waitForToast(pendingAdoptionToast);

    // A purchase in progress through the other path names no seller: the
    // sheet says so as a whole, and holds every Buy.
    await pasteLink(marketNullRaceLink);
    await frame.locator("#detail-modal.active .offer-list").waitFor();
    // Unverified is advisory: said, and still buyable.
    assert(
      /Not yet verified to open here/.test((await frame.locator("#detail-modal").textContent()) || "")
        && (await frame.locator('#detail-modal [data-action="buy-offer"]').count()) === 2,
      "An unverified listing's sheet must say so and still offer its Buys",
    );
    await pressBuy(marketSellerNullRace);
    await frame.locator('#detail-modal [role="status"]', { hasText: "Another purchase of this item is in progress." }).waitFor();
    assert(
      (await frame.locator('#detail-modal [data-action="buy-offer"]').count()) === 0,
      "attempt_in_progress naming no seller must hold every Buy on the sheet",
    );
    await closeSheet();
    // A fresh listing read re-asks Runtime instead of keeping the hold: the
    // sheet offers Buy again, and Runtime answers again if it is still live.
    await pasteLink(marketNullRaceLink);
    await frame.locator("#detail-modal.active .offer-list").waitFor();
    assert(
      (await frame.locator('#detail-modal [data-action="buy-offer"]').count()) === 2
        && !/Another purchase of this item is in progress/.test((await frame.locator("#detail-modal").textContent()) || ""),
      "A sheet reopened from a fresh listing read must offer Buy again rather than keep an earlier hold",
    );
    await closeSheet();

    // An item this Home already holds, or listed, is not offered for sale
    // to it again (UI-I1). A copy this Home bought can always be fetched
    // again with Download copy (R45); an item this Home listed cannot.
    // A bought item's sheet shows what the item is instead of who else
    // sells it: offers are for a person who does not own it yet.
    for (const [link, copy, downloads, offerLists] of [
      [marketPurchasedLink, "You own this.", 1, 0],
      [marketCreatorLink, "You listed this.", 0, 1],
    ]) {
      await pasteLink(link);
      await frame.locator("#detail-modal.active .modal-body").waitFor();
      await frame.locator(`#detail-modal.active ${offerLists ? ".offer-list" : ".item-properties"}`).waitFor();
      const heldStatus = await frame.locator('#detail-modal [role="status"]').textContent();
      const heldControls = await frame.locator("#detail-modal .offer-row button").count();
      const heldDownloads = await frame.locator('#detail-modal [data-action="download-market-copy"]').count();
      const heldOfferLists = await frame.locator("#detail-modal .offer-list").count();
      const heldFocus = await frame.evaluate(() => document.activeElement?.dataset?.action || "");
      assert(
        (heldStatus || "").includes(copy) && heldControls === 0 && heldDownloads === downloads
          && heldOfferLists === offerLists,
        "A sheet for an item this Home holds or listed must say so and offer no Buy; a held copy offers Download copy and no offers",
        { link, heldStatus, heldControls, heldDownloads, heldOfferLists },
      );
      assert(heldFocus === "close-detail", "A held item's sheet must open on Close", { link, heldFocus });
      if (!offerLists) {
        const heldProperties = await itemProperties();
        assert(
          heldProperties.Title?.text === "Held purchased"
            && heldProperties.KID?.title === marketListingKid
            && heldProperties.KID?.text !== marketListingKid
            && heldProperties["Token ID"]?.text === "0x3e"
            && heldProperties.Opens?.text === "Opens here",
          "A bought item's sheet names its chain item and KID, shortened with the full value in its title",
          heldProperties,
        );
      }
      await closeSheet();
    }

    // R45: a purchase whose adoption is still pending has a way forward
    // that spends nothing. Download copy names the item (never a mint this
    // Home may not hold), Runtime reruns adoption and fetches the copy, and
    // the page reloads what it shows.
    const marketDownloads = () => requestLog.filter((entry) => entry.path === "/api/provider/object/download_owned_copy");
    const mediaReads = () => requestLog.filter((entry) => entry.token === normalToken && entry.path === "/api/provider/object/list_runtime_custody").length;
    const catalogReads = () => requestLog.filter((entry) => entry.token === normalToken && entry.path === "/api/apps/marketplace/items").length;
    // The Explore film this page already read a listing for is now held,
    // and not adopted here: the market still lists it without a mint.
    state[normalToken].catalogItems[0].accessState = "purchased";
    await pasteLink(marketPurchasedLink);
    await frame.locator("#detail-modal.active .item-properties").waitFor();
    const sheetDownloadsBefore = marketDownloads().length;
    const sheetMediaBefore = mediaReads();
    const sheetCatalogBefore = catalogReads();
    const buyOffersBeforeDownload = buyOfferCount();
    await frame.locator('#detail-modal [data-action="download-market-copy"]').click();
    await waitForRequestCount(requestLog, normalToken, "/api/provider/object/download_owned_copy", sheetDownloadsBefore + 1);
    await waitForToast("Downloaded. The copy is in your Library.");
    await waitForRequestCount(requestLog, normalToken, "/api/apps/marketplace/items", sheetCatalogBefore + 1);
    await waitForRequestCount(requestLog, normalToken, "/api/provider/object/list_runtime_custody", sheetMediaBefore + 1);
    const sheetDownload = marketDownloads().at(-1);
    assert(
      sheetDownload?.method === "POST"
        && JSON.stringify(sheetDownload.body) === JSON.stringify({
          item: {
            chain_namespace: "eip155:8453",
            network: "base",
            ledger: `0x${"e".repeat(40)}`,
            token_id: "0x3e",
            kid: marketListingKid,
          },
        })
        && mediaReads() > sheetMediaBefore
        && buyOfferCount() === buyOffersBeforeDownload,
      "Download copy on a held sheet must name the item, pay nothing, and reload media and catalog",
      { sheetDownload, mediaReads: mediaReads(), sheetMediaBefore },
    );
    assert(
      !(await frame.locator("#detail-modal").evaluate((node) => node.classList.contains("active"))),
      "A finished Download copy must close the sheet it was pressed on",
    );

    const preparingCard = frame.locator('.card:has(.card-title:text-is("Someone Else\'s Film"))');
    await preparingCard.locator('[data-action="download-market-copy"]').waitFor();
    const preparingState = await catalogCardState(preparingCard);
    assert(
      preparingState.badge === "Purchased"
        && preparingState.owned === "✓ Purchased"
        && preparingState.stock === ""
        && !/In your library/.test(preparingState.text)
        && !/available/.test(preparingState.text)
        && /by 0x34daf3/.test(preparingState.text)
        && preparingState.play === 0
        && preparingState.actions.join("|") === "download-market-copy|item-details",
      "A bought market item with no copy here yet says Purchased, keeps its seller, and offers Download copy and Details -- never Buy",
      preparingState,
    );
    // Details: what the item is, from its own listing, with focus on Close.
    await preparingCard.locator('[data-action="item-details"]').click();
    await frame.locator("#detail-modal.active .item-properties").waitFor();
    const preparingProperties = await itemProperties();
    const preparingSheet = await sheetFacts();
    assert(
      preparingProperties.Title?.text === "Someone Else's Film"
        && preparingProperties["Listed by"]?.title === "0x34daf3000000000000000000000000000000beef"
        && preparingProperties["Listed by"]?.text !== "0x34daf3000000000000000000000000000000beef"
        && preparingProperties.Ledger?.title === marketExploreLedger
        && preparingProperties["Token ID"]?.text === marketExploreTokenId
        && preparingProperties.Operative?.title === marketListingOperative
        && preparingProperties["Media type"]?.text === "video/mp4"
        && preparingProperties.Opens?.text === "Opens here"
        && !preparingProperties.KID
        && !preparingProperties.Description
        && preparingSheet.focus === "close-detail"
        && preparingSheet.offerLists === 0
        && preparingSheet.buyControls === 0
        && preparingSheet.fullAddresses.length === 0
        && /You own this\./.test(preparingSheet.status),
      "Details on a bought item shows its properties, no offers and no full address, and opens on Close",
      { preparingProperties, preparingSheet },
    );
    await closeSheet();
    const rowDownloadsBefore = marketDownloads().length;
    // The second film is bought too, from elsewhere; the reload that follows
    // this Download copy brings it back as purchased.
    state[normalToken].catalogItems[1].accessState = "purchased";
    await preparingCard.locator('[data-action="download-market-copy"]').click();
    await waitForRequestCount(requestLog, normalToken, "/api/provider/object/download_owned_copy", rowDownloadsBefore + 1);
    await waitForToast("Downloaded. The copy is in your Library.");
    const rowDownload = marketDownloads().at(-1);
    assert(
      JSON.stringify(rowDownload?.body) === JSON.stringify({
        item: {
          chain_namespace: "eip155:8453",
          network: "base",
          ledger: marketExploreLedger,
          token_id: marketExploreTokenId,
        },
      }),
      "Download copy on an Explore row must name exactly the item this page read the listing for",
      rowDownload,
    );

    // A bought item this Home has not learned about yet: it says Purchased,
    // never In your library, and Details finds out -- the item is foreign,
    // so the card then says where it opens.
    const foreignCard = frame.locator(`.card[data-id="${marketExploreLedger}:${marketExplorePaperTokenId}"]`);
    await ensureCardBy(`.card[data-id="${marketExploreLedger}:${marketExplorePaperTokenId}"]`);
    await foreignCard.locator('[data-action="item-details"]').waitFor();
    const unlearnedState = await catalogCardState(foreignCard);
    assert(
      unlearnedState.badge === "Purchased"
        && unlearnedState.stock === ""
        && !/In your library/.test(unlearnedState.text)
        && !/available/.test(unlearnedState.text)
        && unlearnedState.actions.includes("item-details")
        && !unlearnedState.actions.includes("buy-listing"),
      "A bought item whose readability is not learned yet says Purchased and offers Details",
      unlearnedState,
    );
    await foreignCard.locator('[data-action="item-details"]').click();
    await frame.locator("#detail-modal.active .item-properties").waitFor();
    const foreignProperties = await itemProperties();
    const foreignSheet = await sheetFacts();
    assert(
      foreignProperties.Title?.text === "Someone Else's Paper"
        && foreignProperties.Description?.text === "A <b>paper</b> minted elsewhere."
        && foreignProperties.Description?.html.includes("&lt;b&gt;")
        && foreignProperties["Listed by"]?.title === "0x34daf3000000000000000000000000000000beef"
        && foreignProperties.Ledger?.title === marketExploreLedger
        && foreignProperties["Token ID"]?.text === marketExplorePaperTokenId
        && foreignProperties.Operative?.title === marketExplorePaperOperative
        && foreignProperties["Media type"]?.text === "application/pdf"
        && foreignProperties.Opens?.text === "Opens on ela.city"
        && !foreignProperties.KID
        && foreignSheet.focus === "close-detail"
        && foreignSheet.offerLists === 0
        && foreignSheet.buyControls === 0
        && foreignSheet.downloads === 0
        && foreignSheet.fullAddresses.length === 0
        && /You own this\./.test(foreignSheet.status),
      "Details on a foreign bought item shows its properties and where it opens, with no offers, no download and no full address",
      { foreignProperties, foreignSheet },
    );
    await closeSheet();
    await foreignCard.locator(".store-row-state").waitFor();
    const foreignState = await catalogCardState(foreignCard);
    assert(
      foreignState.badge === "Purchased"
        && foreignState.stock === ""
        && /Opens on ela\.city/.test(foreignState.text)
        && !/In your library/.test(foreignState.text)
        && !/available/.test(foreignState.text)
        && /by 0x34daf3/.test(foreignState.text)
        && foreignState.actions.join("|") === "item-details",
      "A foreign bought item says Purchased and Opens on ela.city, and offers only Details",
      foreignState,
    );

    // At phone width the row wraps rather than pushing its control out of
    // the sheet (UI-M10).
    await pasteLink(marketAssetLink);
    await frame.locator("#detail-modal.active .offer-list").waitFor();
    await page.setViewportSize({ width: 360, height: 800 });
    await waitForFrameWidth(frame, 360);
    const narrowOverflow = await frame.locator("#detail-modal .offer-row").evaluateAll((rows) =>
      rows.filter((row) => row.scrollWidth > row.clientWidth + 1).length);
    await page.setViewportSize({ width: 1280, height: 900 });
    await waitForFrameWidth(frame, 1280);
    assert(narrowOverflow === 0, "Offer rows must fit a narrow sheet", { narrowOverflow });
    await closeSheet();

    const purchasedOpenCountBefore = (await readHomeMessages(page)).filter((entry) => entry.type === "home:open-target").length;
    await ensureCard(mediaPurchasedMint);
    await frame.locator(`.store-row-media[data-mint="${mediaPurchasedMint}"] .card-actions [data-action="open-media"]`).click();
    const mediaOpenMessagesAfterPurchased = await readHomeMessages(page);
    const purchasedOpen = mediaOpenMessagesAfterPurchased.filter((entry) => entry.type === "home:open-target").at(-1);
    assert(
      mediaOpenMessagesAfterPurchased.filter((entry) => entry.type === "home:open-target").length === purchasedOpenCountBefore + 1,
      "Marketplace purchased media rows must open through Home exactly once",
      mediaOpenMessagesAfterPurchased,
    );
    assert(
      purchasedOpen?.target === "elacity-player"
        && purchasedOpen.homeToken === normalToken
        && JSON.stringify(purchasedOpen.query) === JSON.stringify({ mint_id: mediaPurchasedMint })
        && JSON.stringify(purchasedOpen.keys) === JSON.stringify(["homeToken", "query", "target", "type"]),
      "Marketplace purchased media rows must send the exact authorized elacity-player launch command",
      purchasedOpen,
    );

    await frame.locator('[data-destination="discover"]').click();
    await frame.locator('.store-row[data-app="documents"]').first().waitFor();

    await frame.locator('.store-row[data-app="documents"]').first().focus();
    await frame.locator('.store-row[data-app="documents"]').first().click();
    await frame.locator("#detail-modal.active").waitFor();
    assert(
      await frame.evaluate(() => document.activeElement?.dataset?.action === "close-detail"),
      "An app's detail must open with focus on Close, not on Open",
    );
    const modalText = await frame.locator("#detail-content").textContent();
    assert(/Status/.test(modalText || ""), "Marketplace detail modal must expose trust and status", { modalText });
    assert(/Works with/.test(modalText || ""), "Marketplace detail modal must expose accepted content and dependencies", { modalText });
    assert(/Available actions/.test(modalText || ""), "Marketplace detail modal must expose executable actions", { modalText });

    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab");
    const trappedInsideModal = await frame.evaluate(() => {
      const modal = document.getElementById("detail-content");
      return Boolean(modal && modal.contains(document.activeElement));
    });
    assert(trappedInsideModal, "Marketplace detail focus must stay trapped in the modal");
    await page.keyboard.press("Escape");
    const modalActiveAfterEscape = await frame.locator("#detail-modal").evaluate((node) => node.classList.contains("active"));
    assert(!modalActiveAfterEscape, "Marketplace detail modal must close on Escape");
    const restoredFocus = await frame.evaluate(() => document.activeElement?.getAttribute("data-app") || "");
    assert(restoredFocus === "documents", "Marketplace must restore focus to the invoking row after closing the detail modal", { restoredFocus });

    const openMessagesBefore = (await readHomeMessages(page)).filter((entry) => entry.type === "home:open-target").length;
    await frame.locator('.store-row[data-app="people"] .store-pill').first().click();
    const openMessagesAfter = await readHomeMessages(page);
    const latestOpen = openMessagesAfter.filter((entry) => entry.type === "home:open-target").at(-1);
    assert((openMessagesAfter.filter((entry) => entry.type === "home:open-target").length) === openMessagesBefore + 1, "Marketplace must launch only through Home open-target", openMessagesAfter);
    assert(latestOpen?.target === "people", "Marketplace must use the catalog launch_target", latestOpen);

    const openCountBeforeBad = openMessagesAfter.filter((entry) => entry.type === "home:open-target").length;
    await frame.locator('.store-row[data-app="bad-icons"]').first().click();
    await frame.locator("#detail-modal.active").waitFor();
    const badOpenButtonCount = await frame.locator('#detail-content [data-action="open"]').count();
    assert(badOpenButtonCount === 0, "Marketplace must not offer Open for non-launchable entries");
    await frame.locator('#detail-content [data-action="close-detail"]').last().click();
    const openCountAfterBad = (await readHomeMessages(page)).filter((entry) => entry.type === "home:open-target").length;
    assert(openCountAfterBad === openCountBeforeBad, "Marketplace non-launchable entries must not send launch messages");

    await frame.locator('[data-destination="discover"]').focus();
    await page.keyboard.press("ArrowDown");
    let currentDestination = await frame.locator('[aria-current="page"]').textContent();
    assert(/Installed/.test(currentDestination || ""), "Marketplace sidebar ArrowDown must move among destinations", { currentDestination });
    await frame.locator('[data-destination="discover"]').click();
    await frame.locator("#search-input").focus();
    await page.keyboard.press("ArrowDown");
    currentDestination = await frame.locator('[aria-current="page"]').textContent();
    assert(/Discover/.test(currentDestination || ""), "Marketplace sidebar keys must not steal ArrowDown from inputs", { currentDestination });

    const countsBeforeInvalidRefresh = {
      catalog: requestLog.filter((entry) => entry.token === normalToken && entry.path === "/api/capsules/catalog").length,
      interfaces: requestLog.filter((entry) => entry.token === normalToken && entry.path === "/api/capsules/interfaces").length,
    };
    const refreshCatalogResponse = page.waitForResponse((response) => {
      const headers = response.request().headers();
      return response.url() === `http://127.0.0.1:${port}/api/capsules/catalog`
        && headers["x-elastos-home-token"] === normalToken;
    });
    const refreshInterfacesResponse = page.waitForResponse((response) => {
      const headers = response.request().headers();
      return response.url() === `http://127.0.0.1:${port}/api/capsules/interfaces`
        && headers["x-elastos-home-token"] === normalToken;
    });
    await page.evaluate(() => {
      window.postToMarketplaceDirect({ type: "elastos:menu-command", cmd: "refresh" });
      window.postToMarketplace({ type: "elastos:menu-command", cmd: "refresh" });
    });
    await Promise.all([refreshCatalogResponse, refreshInterfacesResponse]);
    const countsAfterRefresh = {
      catalog: requestLog.filter((entry) => entry.token === normalToken && entry.path === "/api/capsules/catalog").length,
      interfaces: requestLog.filter((entry) => entry.token === normalToken && entry.path === "/api/capsules/interfaces").length,
    };
    assert(
      countsAfterRefresh.catalog === countsBeforeInvalidRefresh.catalog + 1
        && countsAfterRefresh.interfaces === countsBeforeInvalidRefresh.interfaces + 1,
      "Marketplace must reject non-parent menu commands and reuse the same refresh path for trusted parent commands",
      { countsBeforeInvalidRefresh, countsAfterRefresh },
    );

    await page.setViewportSize({ width: 1280, height: 900 });
    await waitForFrameWidth(frame, 1280);
    await frame.locator('[data-destination="media"]').click();
    await frame.locator('.store-row-media').first().waitFor();
    await assertNoHorizontalOverflow(frame, "wide Marketplace layout");

    await page.setViewportSize({ width: 640, height: 900 });
    await waitForFrameWidth(frame, 640);
    await assertNoHorizontalOverflow(frame, "narrow Marketplace layout");

    // A copy someone owns can be built again, whether it never arrived or was
    // deleted. The control belongs to copies they hold and to nothing else.
    const downloadsBefore = requestLog.filter(
      (entry) => entry.path === "/api/provider/object/download_owned_copy",
    ).length;
    await ensureCard(mediaPurchasedMint);
    await frame.locator(`.store-row-media[data-mint="${mediaPurchasedMint}"] [data-action="download-copy"]`).click();
    await waitForRequestCount(requestLog, normalToken, "/api/provider/object/download_owned_copy", downloadsBefore + 1);
    const downloadRequest = requestLog.filter(
      (entry) => entry.path === "/api/provider/object/download_owned_copy",
    ).at(-1);
    assert(
      downloadRequest?.method === "POST"
        && JSON.stringify(downloadRequest.body) === JSON.stringify({ mint_id: mediaPurchasedMint }),
      "Download must ask Runtime to rebuild exactly the copy the row names",
      downloadRequest,
    );

    // The person who listed an item has a link to pass on. It is the other
    // half of Add a listing: without it, reaching an item on another Home
    // meant already knowing its address.
    // Share is an icon now, so the control is asserted rather than its label
    // -- and the label it does carry is asserted too, because an icon with no
    // accessible name is a control only some people have.
    const creatorCard = frame.locator(`.card[data-mint="${mediaCreatorMint}"]`);
    const creatorRowText = await creatorCard.textContent();
    const shareLabel = await creatorCard
      .locator('[data-action="share-listing"]')
      .getAttribute("aria-label");
    assert(
      shareLabel === "Share listing" && /Listed by you/.test(creatorRowText || ""),
      "A creator's own card must offer the link to their listing, and name the control",
      { creatorRowText, shareLabel },
    );
    assert(
      (await frame.locator(`.store-row-media[data-mint="${mediaPurchasedMint}"] [data-action="share-listing"]`).count()) === 0,
      "A bought copy is not a listing to share",
    );
    await ensureCard(mediaCreatorMint);
    await frame.locator(`.store-row-media[data-mint="${mediaCreatorMint}"] [data-action="share-listing"]`).click();
    await frame.locator("#share-listing-uri").waitFor();
    assert(
      (await frame.locator("#share-listing-uri").inputValue()) === importedListingUri,
      "The share control must show the address Runtime published the listing at",
    );
    await frame.locator('#detail-content .modal-btn[data-action="close-detail"]').click();

    // A purchase waiting on the person is not a failed purchase. The row says
    // whose turn it is, the control goes where the approval is, and nothing
    // reports an error over a purchase that is proceeding normally.
    await page.goto(`http://127.0.0.1:${port}/fixture-media-pending-buy`);
    const pendingBuyFrame = await waitForMarketplaceFrame(page);
    await pendingBuyFrame.locator('[data-destination="media"]').click();
    await pendingBuyFrame.locator(`.store-row-media[data-mint="${mediaAvailableMint}"] [data-action="buy-media"]`).click();
    await pendingBuyFrame.locator('[data-action="confirm-buy"]').click();
    await pendingBuyFrame.locator(`.store-row-media[data-mint="${mediaAvailableMint}"] [data-action="open-wallet"]`).waitFor();
    const pendingRowText = await pendingBuyFrame.locator(`.store-row-media[data-mint="${mediaAvailableMint}"]`).textContent();
    assert(
      /Approve in wallet/.test(pendingRowText || "")
        && /Approve this purchase in wallet-metamask/.test(pendingRowText || ""),
      "A purchase waiting on the person must say whose turn it is and where",
      { pendingRowText },
    );
    assert(
      (await pendingBuyFrame.locator(".store-error-card").count()) === 0,
      "A purchase in progress must not be reported as a failed surface",
    );
    // A purchase Runtime is still holding, from a visit this page does not
    // remember. The row says so and offers to carry on rather than looking
    // untouched, which is what it did before.
    assert(
      (await pendingBuyFrame.locator(`.store-row-media[data-mint="${mediaAvailableMint}"] [data-action="download-copy"]`).count()) === 0,
      "An item on offer is not a copy to download",
    );
    const resumeRowText = await pendingBuyFrame.locator(`.store-row-media[data-mint="${mediaImportedMint}"]`).textContent();
    assert(
      /Continue/.test(resumeRowText || "")
        && /A purchase of this is already under way/.test(resumeRowText || ""),
      "A purchase already under way must survive the page that started it",
      { resumeRowText },
    );

    const walletOpensBefore = (await readHomeMessages(page)).filter((entry) => entry.type === "home:open-target").length;
    await pendingBuyFrame.locator(`.store-row-media[data-mint="${mediaAvailableMint}"] [data-action="open-wallet"]`).click();
    const walletMessages = await readHomeMessages(page);
    const walletOpen = walletMessages.filter((entry) => entry.type === "home:open-target").at(-1);
    assert(
      walletMessages.filter((entry) => entry.type === "home:open-target").length === walletOpensBefore + 1
        && walletOpen?.target === "wallet"
        && walletOpen.homeToken === mediaPendingBuyToken,
      "The approval control must take the person to the wallet holding the request",
      walletOpen,
    );

    await page.goto(`http://127.0.0.1:${port}/fixture-media-error`);
    const mediaErrorFrame = await waitForMarketplaceFrame(page);
    await mediaErrorFrame.locator('.store-row[data-app="people"]').first().waitFor();
    await mediaErrorFrame.locator('[data-destination="media"]').click();
    await mediaErrorFrame.locator(".store-error-card").waitFor();
    const mediaErrorText = await mediaErrorFrame.locator("#load-error").textContent();
    assert(/Couldn’t load your items/.test(mediaErrorText || ""), "Marketplace must show a bounded public error when the shelf itself cannot load", { mediaErrorText });
    assert(!/runtime service unavailable/.test(mediaErrorText || ""), "Marketplace must keep raw Runtime errors out of visible media text", { mediaErrorText });
    await mediaErrorFrame.locator('[data-destination="discover"]').click();
    await mediaErrorFrame.locator('.store-row[data-app="people"]').first().waitFor();

    await page.goto(`http://127.0.0.1:${port}/fixture-media-empty`);
    const mediaEmptyFrame = await waitForMarketplaceFrame(page);
    await mediaEmptyFrame.locator('[data-destination="media"]').click();
    await mediaEmptyFrame.locator(".empty-state").waitFor();
    const mediaEmptyText = await mediaEmptyFrame.locator("#store-main").textContent();
    assert(/No protected items yet/.test(mediaEmptyText || ""), "Marketplace must keep a clear empty state for the protected shelf", { mediaEmptyText });

    // A row this app cannot read is refused on its own. It never reaches the
    // shelf, and the person is told one item is hidden rather than losing the
    // whole surface to it -- which is what used to happen, for every row,
    // whenever the listing contract moved.
    await page.goto(`http://127.0.0.1:${port}/fixture-media-malformed`);
    const mediaMalformedFrame = await waitForMarketplaceFrame(page);
    await mediaMalformedFrame.locator('[data-destination="media"]').click();
    await mediaMalformedFrame.locator(".store-inline-note").waitFor();
    const mediaMalformedText = await mediaMalformedFrame.locator("#store-main").textContent();
    assert(
      /One item could not be read and is hidden/.test(mediaMalformedText || ""),
      "Marketplace must say when it refused a listing row",
      { mediaMalformedText },
    );
    assert(
      (await mediaMalformedFrame.locator(".store-row-media").count()) === 0,
      "Marketplace must keep a refused listing row off the shelf",
    );
    assert(
      (await mediaMalformedFrame.locator(".store-error-card").count()) === 0,
      "Marketplace must keep one refused row from reading as a failed surface",
    );

    // Explore is the market. A fresh page waits for it rather than passing
    // this Home's own items off as the market, and a first read that fails
    // is asked once more on its own, without the person pressing refresh.
    const marketReads = (token) => requestLog.filter((entry) =>
      entry.token === token && entry.path === "/api/apps/marketplace/items").length;
    await page.goto(`http://127.0.0.1:${port}/fixture-token/${encodeURIComponent(marketRetryToken)}`);
    const retryFrame = await waitForMarketplaceFrame(page);
    await retryFrame.locator('.store-row[data-app="people"]').first().waitFor();
    await retryFrame.locator('[data-destination="media"]').click();
    await waitForRequestCount(requestLog, marketRetryToken, "/api/apps/marketplace/items", 1);
    await retryFrame.locator('#store-sections [role="status"]', { hasText: "Loading the market" }).waitFor();
    assert(
      (await retryFrame.locator(".store-row-media").count()) === 0
        && (await retryFrame.locator("#store-sections .card.skeleton").count()) > 0,
      "Explore must show a loading state, not this Home's own items, before the market answers",
    );
    state[marketRetryToken].firstMarketBarrier.release();
    await waitForRequestCount(requestLog, marketRetryToken, "/api/apps/marketplace/items", 2);
    await retryFrame.locator(`.store-row-media[data-id="${firstLoadMarketItemId}"]`).waitFor();
    const retryText = await retryFrame.locator("#store-sections").textContent();
    assert(
      marketReads(marketRetryToken) === 2
        && (await retryFrame.locator(`.store-row-media[data-mint="${mediaCreatorMint}"]`).count()) === 1
        && (await retryFrame.locator('[data-action="retry-market"]').count()) === 0
        && !/Couldn’t reach the market/.test(retryText || ""),
      "A first market read that fails must be retried once on its own, and the market shown with this Home's items joined to it",
      { reads: marketReads(marketRetryToken), retryText },
    );

    // A market that stays unreachable says so on Explore, with a way to ask
    // again, and the page stops asking by itself after the second read.
    await page.goto(`http://127.0.0.1:${port}/fixture-token/${encodeURIComponent(marketDownToken)}`);
    const downFrame = await waitForMarketplaceFrame(page);
    await downFrame.locator('.store-row[data-app="people"]').first().waitFor();
    await downFrame.locator('[data-destination="media"]').click();
    await downFrame.locator('#store-sections [role="status"]', { hasText: "Loading the market" }).waitFor();
    state[marketDownToken].firstMarketBarrier.release();
    await downFrame.locator('#store-sections [data-action="retry-market"]').waitFor();
    const downText = await downFrame.locator('#store-sections [role="status"]').first().textContent();
    assert(
      /Couldn’t reach the market\./.test(downText || "")
        && (await downFrame.locator(".store-row-media").count()) === 0
        && marketReads(marketDownToken) === 2,
      "An unreachable market must be read twice, then named on Explore with Try again, and never replaced by this Home's own items",
      { downText, reads: marketReads(marketDownToken) },
    );
    await page.waitForTimeout(2_500);
    assert(
      marketReads(marketDownToken) === 2,
      "A failed market must not be asked again without the person",
      { reads: marketReads(marketDownToken) },
    );
    state[marketDownToken].marketFailuresRemaining = 0;
    await downFrame.locator('[data-action="retry-market"]').click();
    await downFrame.locator(`.store-row-media[data-id="${firstLoadMarketItemId}"]`).waitFor();
    assert(
      marketReads(marketDownToken) === 3
        && (await downFrame.evaluate(() => document.activeElement !== document.body)),
      "Try again must read the market once and keep keyboard focus on the surface",
      { reads: marketReads(marketDownToken) },
    );

    await page.goto(`http://127.0.0.1:${port}/fixture-error`);
    const errorFrame = await waitForMarketplaceFrame(page);
    await errorFrame.locator(".store-error-card").waitFor();
    const errorText = await errorFrame.locator("#load-error").textContent();
    assert(/Couldn’t load apps/.test(errorText || ""), "Marketplace must show a bounded public error state", { errorText });
    assert(!/provider launch failed/.test(errorText || ""), "Marketplace must not expose raw internal load errors", { errorText });
    const errorMessagesBeforeRetry = (await readHomeMessages(page)).filter((entry) => entry.type === "home:menu-manifest").length;
    await errorFrame.locator('[data-action="retry"]').click();
    await errorFrame.locator('.store-row[data-app="people"]').first().waitFor();
    const errorMessagesAfterRetry = (await readHomeMessages(page)).filter((entry) => entry.type === "home:menu-manifest").length;
    assert(errorMessagesAfterRetry === errorMessagesBeforeRetry, "Marketplace must deduplicate unchanged menu manifests across retry");

    assert(notFoundPaths.length === 0, "Marketplace layout smoke hit unexpected fixture paths", notFoundPaths);
    // An item that published no metadata document answers 404, and the shelf
    // renders its file name instead. That is an expected answer rather than a
    // failure, so it is named here rather than counted.
    const metadataMisses = nonOkResponses.filter((entry) =>
      entry.url.endsWith(`/ipfs/${mediaBareMetadataCid}/metadata.json`) && entry.status === 404);
    // The deliberate `/listing` refusals whose wording the sheet test reads.
    const listingRefusals = nonOkResponses.filter((entry) =>
      entry.url.endsWith("/api/apps/marketplace/listing") && [400, 404, 409, 503].includes(entry.status));
    const unexpectedNonOk = nonOkResponses.filter((entry) => !metadataMisses.includes(entry) && !listingRefusals.includes(entry));
    assert(
      metadataMisses.length > 0,
      "An item with no published document must be answered, not left hanging",
    );
    assert(
      unexpectedNonOk.length === 2
        && unexpectedNonOk.some((entry) =>
          entry.url === `http://127.0.0.1:${port}/api/capsules/catalog`
            && entry.status === 500
            && entry.token === errorToken
            && entry.method === "GET",
        )
        && unexpectedNonOk.some((entry) =>
          entry.url === `http://127.0.0.1:${port}/api/provider/object/list_runtime_custody`
            && entry.status === 500
            && entry.token === mediaErrorToken
            && entry.method === "POST",
        ),
      "Marketplace layout smoke must see only the deliberate catalog and media fixture failures",
      unexpectedNonOk,
    );
    assert(pageErrors.length === 0, "Marketplace layout smoke saw page errors", pageErrors);
    const expectedConsoleError = "Failed to load resource: the server responded with a status of 500 (Internal Server Error)";
    const expectedMetadataMiss = "Failed to load resource: the server responded with a status of 404 (Not Found)";
    const expectedListingRefusals = [
      "Failed to load resource: the server responded with a status of 400 (Bad Request)",
      "Failed to load resource: the server responded with a status of 409 (Conflict)",
      "Failed to load resource: the server responded with a status of 503 (Service Unavailable)",
    ];
    assert(listingRefusals.length === 4, "Each deliberate listing refusal must be answered once", listingRefusals);
    const unexpectedConsoleErrors = consoleErrors.filter(
      (entry) => entry !== expectedConsoleError && entry !== expectedMetadataMiss && !expectedListingRefusals.includes(entry),
    );
    assert(
      consoleErrors.filter((entry) => entry === expectedConsoleError).length === 2,
      "Marketplace layout smoke must see only the deliberate catalog and media fixture console errors",
      consoleErrors,
    );
    assert(unexpectedConsoleErrors.length === 0, "Marketplace layout smoke saw console errors", unexpectedConsoleErrors);
    // A person can leave the shelf while its documents are still arriving, and
    // the browser abandons those reads. The shelf keeps the file names Runtime
    // sent and says nothing, which is the whole point of asking for titles
    // after the items are already on screen.
    const abandonedReads = requestFailures.filter((entry) =>
      entry.url.endsWith("/metadata.json") && entry.error === "net::ERR_ABORTED");
    assert(
      requestFailures.length === abandonedReads.length,
      "Marketplace layout smoke saw failed browser requests",
      requestFailures.filter((entry) => !abandonedReads.includes(entry)),
    );
  } finally {
    server.closeAllConnections?.();
    await new Promise((resolveClose) => server.close(() => resolveClose()));
    await browser.close();
  }
}

run()
  .then(() => {
    console.log("marketplace-product-layout-smoke: OK");
  })
  .catch((error) => {
    console.error(error.stack || String(error));
    process.exitCode = 1;
  });
