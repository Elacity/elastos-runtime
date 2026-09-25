// Marketplace reads the capsule catalog and the protected-item shelf, and asks
// Home to launch what a person chooses. The listing contract it reads lives in
// `src/listing.js`, beside the tests that pin it against Runtime's producer.
import {
  ACCESS_STATES,
  MAX_RUNTIME_CUSTODY_LISTINGS,
  RUNTIME_CUSTODY_MINT_ID,
  buyOutcomeFromAnswer,
  parseRuntimeCustodyListings,
  uint256Decimal,
  viewerForListing,
} from "./src/listing.js";
// The market listing contract (Runtime's `elastos.marketplace.listing/v1`)
// and the `buy_offer` answer contract, read exactly as `src/market-listing.js`
// and its test pin them against Runtime's producer. Buy is a separate
// workflow from open (shared-context.md D1): these never touch a mint.
import {
  MAX_MARKET_OFFERS,
  marketItemClaim,
  marketItemKey,
  offersWithRecordedAttempt,
  parseBuyOfferAnswer,
  parseMarketListing,
} from "./src/market-listing.js";

(function () {
  const params = new URLSearchParams(window.location.search);
  const homeToken = new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token") || "";
  const homeParentOrigin = params.get("home_origin") || "";

  const state = {
    apps: [],
    mediaListings: [],
    destination: "discover",
    // Which of Media's three surfaces is showing. Held here and nowhere else:
    // this capsule keeps no browser storage, so a reload returns to Explore,
    // which is the surface that works without anything external.
    mediaTab: "explore",
    search: "",
    appLoading: true,
    mediaLoading: true,
    appLoadError: null,
    mediaLoadError: null,
    mediaTruncated: false,
    mediaRejected: 0,
    // Shops reads an outside index, so it carries the three answers that are
    // not "here are the channels": nobody has approved the read, the index
    // could not be reached, or it answered with nothing.
    shops: [],
    shopsLoading: false,
    shopsLoaded: false,
    shopsLoadError: null,
    shopsNeedsApproval: false,
    shopsUnavailable: false,
    shopsAwaitingApproval: false,
    // What this Home already holds on each channel, keyed by address:
    // "administrator", "subscribed", "none", or "unknown" when the chain
    // could not be read. Absent means not asked yet.
    shopAccess: new Map(),
    // The group a person opened from a "See all" link, or "" for the shelf.
    exploreGroup: "",
    // What anyone has minted, as the index reports it, merged by Runtime with
    // what this Home holds. Explore shows these; My listings never does.
    catalog: [],
    catalogLoading: false,
    catalogLoaded: false,
    // Explore shows the market only once the market has answered: with its
    // items, or with a request for approval. Until then it is loading, and a
    // market that could not be reached is said to be one.
    catalogReady: false,
    catalogFailed: false,
    catalogNeedsApproval: false,
    catalogAwaitingApproval: false,
    mediaRefreshing: false,
    // Which kind the chips are filtering to, and the order items arrive in.
    exploreFilter: "all",
    exploreSort: "newest",
  };

  // What each item's published metadata says about itself: the title its
  // creator typed and the cover they chose. Read once per item, from this
  // Home's own `/ipfs/` route, and kept only for as long as this page lives.
  //
  // Presentation only. Price, quantity, availability and who may open it are
  // answered by Runtime in the listing itself and are never taken from here.
  const listingMetadata = new Map();
  // What identifies one item on this shelf.
  //
  // A listing this Home holds answers to its mint. An item known only to the
  // index has no mint here at all, and is named by the asset instead --
  // channel and token, which is how the chain names it. Keying both on the
  // mint alone gave every catalogue item the same empty key, so one item's
  // title and cover appeared on all of them.
  const itemKey = (item) => item.mintId || item.catalogId || "";
  // What a price is denominated in, keyed by the token's address: its decimals
  // and the name a person knows it by. Read once from this Home, which holds
  // the same allow-list a mint is priced against.
  const payTokens = new Map();
  const listingMetadataAsked = new Set();

  // What each purchase in flight is waiting for, keyed by mint. A purchase
  // lives in Runtime, not here: this only remembers what to show while this
  // page stays open, and pressing Buy again after a reload resumes the same
  // attempt rather than starting a second one.
  const pendingMediaBuys = new Map();
  const pendingDownloads = new Set();
  let importInFlight = false;
  const BUY_POLL_MS = 4000;
  // A first market read that fails is asked once more, after this long, and
  // then the page waits for the person. One retry per page open.
  const CATALOG_RETRY_MS = 2000;
  let catalogRetryUsed = false;
  // Which market read is the latest. An older read that lands after a newer
  // one started (a refresh during the first load) changes nothing.
  let catalogReadSeq = 0;
  const BUY_POLL_BUDGET_MS = 15 * 60 * 1000;
  let detailPreviousFocus = null;

  // A market purchase's poll state, keyed by `marketItemKey(listing.item)` --
  // the same discipline `pendingMediaBuys` keeps for a mint, applied to the
  // chain's own key because a market item has no mint until it is adopted.
  // One buy runs per item at a time, so each entry also names which seller
  // it is for -- `{seller, kind, stage, awaitsPerson, connectorId}` -- read
  // fresh at render time exactly as `mediaActionButton` reads
  // `pendingMediaBuys`, rather than threaded through as a render option.
  // This is also what lets the sheet redraw correctly when it is reopened
  // for an item whose buy is still running in the background.
  const pendingOfferBuys = new Map();
  // A purchase of an item Runtime answered is already in progress on terms
  // recorded earlier (`attempt_in_progress`), keyed like `pendingOfferBuys`:
  // `{offer}`, the recorded offer, or `{offer: null}` when that purchase
  // runs through another path and names no seller here. It lasts for this
  // page's life, and clears on any other answer for the item.
  const recordedOfferAttempts = new Map();
  // Which ledger:tokenId requests are already in flight, so a second Buy
  // press on the same Explore card cannot start a second `/listing` read.
  const catalogListingLoading = new Set();
  // What the last `ListingObject` this page actually read said about an
  // item's readability, keyed by `<ledger>|<token_id>` (lowercase) since a
  // catalog row has no chain namespace to complete `marketItemKey` with.
  // Never a guess from the index: only ever what Runtime answered.
  const marketAssetReadability = new Map();
  // The item that same `ListingObject` named, under the same key, so a
  // catalog row (which knows only ledger and token id) can name the whole
  // item when it asks Runtime for its copy.
  const marketAssetItems = new Map();
  // Which market items' copies are being fetched now, keyed like
  // `marketAssetReadability`, so a second press asks nothing more.
  const pendingMarketDownloads = new Set();
  // The listing the offer sheet is currently showing, so a click on one of
  // its rows knows which offer it is buying.
  let currentOfferListing = null;
  // True while the sheet shows an item's properties (Details) rather than
  // its offers. Kept for the life of one opening, so a repaint keeps it.
  let currentOfferSheetProperties = false;
  let homeChromeReady = false;
  let lastHomeMenuManifestSignature = "";

  const els = {
    categoryList: document.querySelector("#category-list"),
    loadingState: document.querySelector("#loading-state"),
    loadError: document.querySelector("#load-error"),
    storeMain: document.querySelector("#store-main"),
    storeSections: document.querySelector("#store-sections"),
    storeTitle: document.querySelector("#store-title"),
    installedBadge: document.querySelector("#installed-badge"),
    detailModal: document.querySelector("#detail-modal"),
    detailContent: document.querySelector("#detail-content"),
    searchInput: document.querySelector("#search-input"),
    mediaHead: document.querySelector("#media-head"),
    mediaTabs: document.querySelector("#media-tabs"),
    mediaToolbar: document.querySelector("#media-toolbar"),
    mediaChips: document.querySelector("#media-chips"),
    mediaSort: document.querySelector("#media-sort"),
    importForm: document.querySelector("#import-listing"),
    importInput: document.querySelector("#import-listing-uri"),
    importSubmit: document.querySelector("#import-listing-submit"),
    toast: document.querySelector("#toast"),
  };

  const categories = [
    { id: "apps", label: "Apps", icon: "package" },
    { id: "viewers", label: "Viewers", icon: "play" },
    { id: "content", label: "Content", icon: "document" },
    { id: "providers", label: "Services", icon: "server" },
    { id: "shells", label: "Home views", icon: "system" },
  ];

  const icons = {
    package: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M16.5 9.4l-9-5.19M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z"></path><polyline points="3.27 6.96 12 12.01 20.73 6.96"></polyline><line x1="12" y1="22.08" x2="12" y2="12"></line></svg>',
    play: '<svg viewBox="0 0 24 24" fill="currentColor"><polygon points="5 3 19 12 5 21 5 3"></polygon></svg>',
    document: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M7 3h7l4 4v14H7V3zm6 1.5V8h3.5L13 4.5zM9 12h6v1.5H9V12zm0 3h6v1.5H9V15z"/></svg>',
    server: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M4 1h16c1.1 0 2 .9 2 2v4c0 1.1-.9 2-2 2H4c-1.1 0-2-.9-2-2V3c0-1.1.9-2 2-2zm0 8h16c1.1 0 2 .9 2 2v4c0 1.1-.9 2-2 2H4c-1.1 0-2-.9-2-2v-4c0-1.1.9-2 2-2zm0 8h16c1.1 0 2 .9 2 2v4c0 1.1-.9 2-2 2H4c-1.1 0-2-.9-2-2v-4c0-1.1.9-2 2-2zm2-12v2h2V5H6zm0 8v2h2v-2H6zm0 8v2h2v-2H6z"/></svg>',
    system: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="3"></circle><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path></svg>',
    search: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="8"></circle><path d="m21 21-4.35-4.35"></path></svg>',
    close: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M19 6.41 17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z"/></svg>',
    check: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M9 16.17 4.83 12l-1.42 1.41L9 19 21 7l-1.41-1.41z"/></svg>',
    share: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 3v12"></path><path d="m8 7 4-4 4 4"></path><path d="M5 13v6a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2v-6"></path></svg>',
    download: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 3v12"></path><path d="m8 11 4 4 4-4"></path><path d="M5 19h14"></path></svg>',
    mediaOutline: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6"><rect x="2.5" y="5.5" width="19" height="13" rx="3"></rect><path d="m10.5 9.8 4.6 2.7-4.6 2.7V9.8Z"></path></svg>',
    playFill: '<svg viewBox="0 0 24 24" fill="currentColor"><polygon points="7 4 20 12 7 20 7 4"></polygon></svg>',
    media: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="3" y="5" width="18" height="14" rx="2"></rect><path d="m10 9 5 3-5 3V9Z" fill="currentColor" stroke="none"></path></svg>',
  };

  const CAPSULE_ICON_ROUTE = /^\/apps\/[A-Za-z0-9_.-]+\/[A-Za-z0-9_./-]+\.png$/;
  const MEDIA_TABS = ["explore", "shops", "mine"];
  const SHOP_ACCESS_STATES = ["administrator", "subscribed", "none", "unknown"];
  // The shelf's sections, in the order a person sees them, each matched by the
  // MIME Runtime published. The kind comes from Runtime rather than from the
  // creator's chosen category: a category is a label on a document this app
  // fetches separately and may not have yet, and a shelf that regroups itself
  // once a fetch lands is a shelf that moved under someone's hand.
  // `shape` is how a card holds its cover, and it follows what the thing is
  // rather than which kind Runtime calls it: an image and a video both fill a
  // frame, and only a document stands a page at the foot of the card.
  const EXPLORE_GROUPS = [
    { id: "videos", title: "Videos", shape: "wide", limit: 3, matches: (mime) => mime.startsWith("video/") },
    { id: "musics", title: "Audio", shape: "wide", limit: 3, matches: (mime) => mime.startsWith("audio/") },
    {
      id: "documents",
      title: "Documents",
      shape: "page",
      // A page is narrower than a frame, so one more fits the same row.
      limit: 5,
      matches: (mime) => mime === "application/pdf"
        || mime.startsWith("text/")
        || mime === "application/epub+zip"
        || mime === "application/msword"
        || mime.startsWith("application/vnd.openxmlformats-officedocument")
        || mime.startsWith("application/vnd.oasis.opendocument"),
    },
    { id: "images", title: "Images", shape: "wide", limit: 3, matches: (mime) => mime.startsWith("image/") },
    // Everything Runtime published that none of the above claims. It is named
    // rather than dropped: an item on this Home that no section wanted would
    // otherwise be invisible to the person who owns it.
    { id: "other", title: "Other", shape: "wide", limit: 3, matches: () => true },
  ];
  // How many items a section shows before it offers the rest, when its own
  // kind does not say. A shelf is meant to fit on screen.
  const EXPLORE_ROW_LIMIT = 3;
  // The app that makes a listing. Named once: this app launches it, and never
  // pretends to do its work.
  const CREATOR_CAPSULE_ID = "creator";
  // Currencies written as a mark rather than a name, which is both shorter on
  // a card and what a person already reads as money. Only the marks in common
  // use: a token nobody writes as a symbol keeps its name.
  const CURRENCY_MARKS = {
    USDC: "$",
    USDT: "$",
    USD: "$",
    ETH: "Ξ",
    WETH: "Ξ",
  };
  // A title longer than this is not a title. Bounded because it comes from a
  // document published by whoever minted the item.
  const MAX_LISTING_TITLE_CHARS = 120;
  // A shared link names the asset only (shared-context.md D3): the folder
  // CID, under either scheme a person might paste it with. No path, no
  // query, no seller -- the same shape `market-listing.js`'s own `ASSET_URI`
  // accepts, checked here before Runtime is asked anything.
  const NATIVE_PAY_TOKEN = "0x0000000000000000000000000000000000000000";
  const MARKET_LINK_URI = /^(?:elastos|ipfs):\/\/(?:Qm[1-9A-HJ-NP-Za-km-z]{44}|b[a-z2-7]{45,95})$/;

  boot();

  async function boot() {
    renderCategories();
    bindEvents();
    announceHomeChrome();
    await loadData();
    render();
  }

  function announceHomeChrome() {
    if (homeChromeReady || !homeToken || !homeParentOrigin || window.top === window) {
      return;
    }
    window.top.postMessage({ type: "home:app-ready", homeToken }, homeParentOrigin);
    homeChromeReady = true;
    syncHomeMenuManifest();
  }

  function syncHomeMenuManifest() {
    if (!homeChromeReady) {
      return;
    }
    const manifest = {
      type: "home:menu-manifest",
      homeToken,
      menus: [
        {
          title: "File",
          items: [
            { label: "New Window", cmd: "__new-window" },
            { label: "Close Window", cmd: "__close-window" },
          ],
        },
        {
          title: "View",
          items: [{ label: "Refresh", cmd: "refresh" }],
        },
      ],
    };
    const signature = JSON.stringify(manifest.menus);
    if (signature === lastHomeMenuManifestSignature) {
      return;
    }
    lastHomeMenuManifestSignature = signature;
    window.top.postMessage(manifest, homeParentOrigin);
  }

  function bindEvents() {
    els.importForm.addEventListener("submit", (event) => {
    event.preventDefault();
    importListing(els.importInput.value);
  });

  els.searchInput.addEventListener("input", (event) => {
      state.search = event.target.value.trim().toLowerCase();
      renderSections();
    });
    document.querySelectorAll("[data-destination]").forEach((node) => {
      if (node.closest("#category-list")) {
        return;
      }
      node.addEventListener("click", () => selectDestination(node.dataset.destination));
    });
    els.mediaTabs.querySelectorAll("[data-media-tab]").forEach((node) => {
      node.addEventListener("click", () => selectMediaTab(node.dataset.mediaTab));
    });
    // The toolbar is in the page rather than rendered, so its controls are
    // bound here. `bindAppActions` only reaches markup this app drew.
    bindAppActions(els.mediaToolbar);
    els.mediaSort.addEventListener("change", (event) => {
      state.exploreSort = String(event.target.value || "newest");
      renderSections();
    });
    els.detailModal.addEventListener("click", (event) => {
      if (event.target === els.detailModal) {
        closeDetail();
      }
    });
    document.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && document.querySelector(".menu-list:not([hidden])")) {
        event.preventDefault();
        closeCardMenus();
        return;
      }
      if (event.key === "Escape" && els.detailModal.classList.contains("active")) {
        event.preventDefault();
        closeDetail();
        return;
      }
      if (event.key === "Tab" && els.detailModal.classList.contains("active")) {
        trapDetailFocus(event);
      }
    });
    document.querySelector(".store-sidebar")?.addEventListener("keydown", (event) => {
      if (event.key !== "ArrowDown" && event.key !== "ArrowUp") {
        return;
      }
      const field = event.target;
      if (
        field instanceof HTMLElement &&
        (field.matches("input, textarea, select") || field.isContentEditable)
      ) {
        return;
      }
      const items = [...document.querySelectorAll(".store-nav-item")];
      if (!items.length) {
        return;
      }
      const current = event.target.closest?.(".store-nav-item");
      let index = current
        ? items.indexOf(current)
        : items.findIndex((node) => node.classList.contains("selected"));
      if (index < 0) {
        index = 0;
      }
      event.preventDefault();
      const next = event.key === "ArrowDown"
        ? Math.min(items.length - 1, index + 1)
        : Math.max(0, index - 1);
      const item = items[next];
      item.focus();
      selectDestination(item.dataset.destination);
    });
    document.addEventListener("click", (event) => {
      if (!event.target.closest(".menu")) {
        closeCardMenus();
      }
    }, true);
    window.addEventListener("message", handleTrustedHomeMessage);
  }

  function handleTrustedHomeMessage(event) {
    if (event.origin !== "null" || event.source !== window.parent) {
      return;
    }
    const data = event.data;
    if (data?.type !== "elastos:menu-command" || typeof data.cmd !== "string") {
      return;
    }
    handleHomeMenuCommand(data.cmd);
  }

  function handleHomeMenuCommand(command) {
    if (command === "refresh") {
      loadData()
        .then(render)
        .catch((error) => {
          showToast(publicError(error.message, "Couldn’t load apps."), true);
        });
    }
  }

  function renderCategories() {
    els.categoryList.replaceChildren();
    for (const category of categories) {
      const button = document.createElement("button");
      button.className = "store-nav-item";
      button.type = "button";
      button.dataset.destination = category.id;
      button.innerHTML = `
        <span class="store-nav-icon" aria-hidden="true">${icons[category.icon] || icons.package}</span>
        <span class="store-nav-label-text">${escapeHtml(category.label)}</span>
      `;
      button.addEventListener("click", () => selectDestination(category.id));
      els.categoryList.append(button);
    }
  }

  async function loadData() {
    state.appLoading = true;
    state.mediaLoading = true;
    state.appLoadError = null;
    state.mediaLoadError = null;
    renderSurfaceState();
    await Promise.all([
      loadCatalogData().then(render),
      loadMediaData().then(render),
      loadPayTokens().then(render),
    ]);
  }

  async function loadCatalogData() {
    state.appLoadError = null;
    try {
      const [catalogResponse, interfacesResponse] = await Promise.all([
        fetch("/api/capsules/catalog", { headers: { "x-elastos-home-token": homeToken } }),
        fetch("/api/capsules/interfaces", { headers: { "x-elastos-home-token": homeToken } }),
      ]);
      const catalog = await catalogResponse.json().catch(() => ({}));
      const interfaces = await interfacesResponse.json().catch(() => ({}));
      if (!catalogResponse.ok) {
        throw new Error(catalogErrorMessage(catalogResponse, catalog));
      }
      if (!interfacesResponse.ok) {
        throw new Error(catalogErrorMessage(interfacesResponse, interfaces));
      }
      const capsules = Array.isArray(catalog.capsules) ? catalog.capsules : [];
      const capsulesByName = new Map(capsules.map((capsule) => [String(capsule.name || ""), capsule]));
      const interfacesByCapsule = new Map();
      for (const entry of Array.isArray(interfaces.interfaces) ? interfaces.interfaces : []) {
        const capsule = String(entry && entry.capsule || "");
        if (!capsule) {
          continue;
        }
        const records = interfacesByCapsule.get(capsule) || [];
        records.push(entry);
        interfacesByCapsule.set(capsule, records);
      }
      state.apps = capsules
        .map((capsule) => capsuleToApp(
          capsule,
          capsulesByName,
          interfacesByCapsule.get(String(capsule.name || "")) || [],
        ))
        .sort((left, right) => appSortKey(left).localeCompare(appSortKey(right)));
    } catch (error) {
      state.apps = [];
      state.appLoadError = publicError(error.message, "Couldn’t load apps.");
    } finally {
      state.appLoading = false;
    }
  }

  async function loadMediaData() {
    state.mediaLoadError = null;
    try {
      const payload = await postObjectProvider("list_runtime_custody", {});
      const parsed = parseRuntimeCustodyListings(payload);
      state.mediaListings = parsed.listings;
      state.mediaTruncated = parsed.truncated;
      state.mediaRejected = parsed.rejected;
    } catch (error) {
      state.mediaListings = [];
      state.mediaTruncated = false;
      state.mediaRejected = 0;
      state.mediaLoadError = publicError(error.message, "Couldn’t load your items.");
    } finally {
      state.mediaLoading = false;
    }
  }

  // The channels publishing protected items. Discovery, never authority: what
  // it lists is what exists, and what a Home may do with any of it is decided
  // on chain when it tries.
  // The market: everything anyone has minted, not only what this Home holds.
  //
  // Runtime has already converted the index's dialect into the values this
  // app speaks, and joined each item against this Home's own listings. What
  // arrives is therefore the same kind of thing the shelf already renders,
  // with one difference that matters: an item with no `mintId` is one this
  // Home holds nothing for, so it can be bought and cannot be opened.
  //
  // `retryOnce` is for the read a page makes by itself when Explore opens: a
  // market that fails to answer is asked once more after CATALOG_RETRY_MS,
  // and Explore keeps its loading state through the wait. A read a person
  // asked for (refresh, Try again) is made once.
  async function loadCatalogItems({ retryOnce = false } = {}) {
    const read = ++catalogReadSeq;
    const current = () => read === catalogReadSeq;
    state.catalogLoading = true;
    state.catalogFailed = false;
    renderSurfaceState();
    try {
      let answered = await readCatalogOnce(current);
      if (!answered && retryOnce && !catalogRetryUsed && current()) {
        catalogRetryUsed = true;
        await new Promise((resolveWait) => setTimeout(resolveWait, CATALOG_RETRY_MS));
        if (current()) {
          answered = await readCatalogOnce(current);
        }
      }
      if (current()) {
        state.catalogFailed = !answered;
      }
    } finally {
      // Attempted either way. Without this a failure is retried on every
      // paint, and a paint follows every retry -- which is a loop that asks
      // an outside service as fast as the page can draw.
      if (current()) {
        state.catalogLoaded = true;
        state.catalogLoading = false;
      }
    }
  }

  // One read of the market. True when the market answered -- with items, or
  // with a request for approval -- and false when it could not be reached.
  // A read that is no longer the latest leaves the state alone.
  async function readCatalogOnce(current) {
    let answer = null;
    try {
      const response = await fetch("/api/apps/marketplace/items", {
        headers: { accept: "application/json", "x-elastos-home-token": homeToken },
      });
      const payload = await response.json().catch(() => ({}));
      if (response.ok && !(payload.unavailable === true && payload.needsApproval !== true)) {
        answer = payload;
      }
    } catch {
      answer = null;
    }
    if (!current()) {
      return answer !== null;
    }
    if (!answer) {
      // An unreachable market is not an empty one, and this Home's own items
      // are not the market: Explore says the market could not be read.
      state.catalog = [];
      state.catalogReady = false;
      state.catalogNeedsApproval = false;
      return false;
    }
    state.catalog = (Array.isArray(answer.items) ? answer.items : [])
      .map(catalogItemFromAnswer)
      .filter(Boolean);
    state.catalogNeedsApproval = answer.needsApproval === true;
    state.catalogReady = true;
    return true;
  }

  // One catalog item, in the shape the card already reads.
  //
  // Runtime screened and converted these; this keeps the page's own shape so
  // that a field it has never seen cannot reach a card, and so a catalog item
  // and a listing render through one function rather than two.
  function catalogItemFromAnswer(entry) {
    const ledger = String(entry && entry.ledger || "");
    const tokenId = String(entry && entry.tokenId || "");
    if (!/^0x[0-9a-f]{40}$/.test(ledger) || !/^0x[0-9a-f]{1,64}$/.test(tokenId)) {
      return null;
    }
    const accessState = String(entry && entry.accessState || "available");
    return {
      // A catalog item is identified by its asset until this Home holds a
      // listing for it; then it answers to that listing's mint like any other.
      mintId: String(entry && entry.mintId || ""),
      catalogId: `${ledger}:${tokenId}`,
      // The chain's own key (shared-context.md D2), kept apart from
      // `catalogId` so a Buy on this card can send exactly this and nothing
      // else -- never an account, never a mint this Home may not hold.
      ledger,
      tokenId,
      displayName: String(entry && entry.displayName || "").slice(0, 256),
      mimeType: catalogMimeType(String(entry && entry.contentCategory || "")),
      codecs: "",
      contentKind: "",
      quantity: `0x${Math.max(0, Number(entry && entry.quantity) || 0).toString(16)}`,
      price: `0x${(BigInt(String(entry && entry.price || "0"))).toString(16)}`,
      payToken: String(entry && entry.payToken || ""),
      sellerAddress: String(entry && entry.sellerAddress || ""),
      publishedAt: Number(entry && entry.publishedAt) || 0,
      views: Number(entry && entry.views) || 0,
      metadataCid: cidOrEmpty(entry && entry.metadataCid),
      accessState: ACCESS_STATES.includes(accessState) ? accessState : "available",
      purchaseInFlight: false,
      availability: null,
      // What this Home can do about it. An item it holds no listing for can be
      // bought; opening one needs the listing, which a purchase creates.
      catalogOnly: !entry || !entry.mintId,
    };
  }

  // The index reports a category where a listing reports a MIME. The shelf
  // groups on MIME, so the category is spelled as one -- `video` becomes
  // `video/*` -- rather than teaching the grouping a second vocabulary.
  function catalogMimeType(category) {
    const known = {
      video: "video/*",
      audio: "audio/*",
      image: "image/*",
      document: "application/pdf",
      ebook: "application/epub+zip",
      comic: "application/pdf",
      article: "text/plain",
    }[category];
    return known || "application/octet-stream";
  }

  // Asked once, early: a price is unreadable without it, and it is local
  // configuration rather than anything this Home has to reach out for.
  async function loadPayTokens() {
    try {
      const response = await fetch("/api/apps/marketplace/pay-tokens", {
        headers: { accept: "application/json", "x-elastos-home-token": homeToken },
      });
      if (!response.ok) {
        return;
      }
      const answer = await response.json().catch(() => ({}));
      for (const token of Array.isArray(answer.payTokens) ? answer.payTokens : []) {
        const address = String(token && token.address || "").toLowerCase();
        const decimals = Number(token && token.decimals);
        const symbol = String(token && token.symbol || "").slice(0, 12);
        if (/^0x[0-9a-f]{40}$/.test(address) && Number.isInteger(decimals) && decimals >= 0 && decimals <= 36) {
          payTokens.set(address, { decimals, symbol });
        }
      }
    } catch {
      // A shelf without this shows base units.
    }
  }

  async function loadShopsData() {
    state.shopsLoading = true;
    state.shopsLoadError = null;
    renderSurfaceState();
    try {
      const response = await fetch("/api/apps/marketplace/channels", {
        headers: { accept: "application/json", "x-elastos-home-token": homeToken },
      });
      const answer = await response.json().catch(() => ({}));
      if (!response.ok) {
        throw new Error(catalogErrorMessage(response, answer));
      }
      state.shops = Array.isArray(answer.channels) ? answer.channels.map(shopFromAnswer) : [];
      state.shopsNeedsApproval = answer.needsApproval === true;
      state.shopsUnavailable = answer.unavailable === true;
      state.shopsLoaded = true;
      // Which cards to offer a subscription for is a separate question with a
      // separate answer, asked once the channels are known and never allowed
      // to hold the list up.
      loadShopAccess(state.shops.map((shop) => shop.address)).then(render);
    } catch (error) {
      state.shops = [];
      state.shopsNeedsApproval = false;
      state.shopsUnavailable = false;
      state.shopsLoadError = publicError(error.message, "Couldn’t load channels.");
    } finally {
      state.shopsLoading = false;
    }
  }

  // What this Home already holds on each of these channels. Its failure is not
  // the surface's failure: every card simply stays at "checking", which is a
  // state it draws, and no card claims access it could not confirm.
  async function loadShopAccess(addresses) {
    if (!addresses.length) {
      return;
    }
    try {
      const response = await fetch("/api/apps/marketplace/channel-access", {
        method: "POST",
        headers: {
          "content-type": "application/json",
          accept: "application/json",
          "x-elastos-home-token": homeToken,
        },
        body: JSON.stringify({ channels: addresses }),
      });
      if (!response.ok) {
        return;
      }
      const answer = await response.json().catch(() => ({}));
      for (const entry of Array.isArray(answer.channels) ? answer.channels : []) {
        const channel = String(entry && entry.channel || "");
        const access = String(entry && entry.state || "");
        if (channel && SHOP_ACCESS_STATES.includes(access)) {
          state.shopAccess.set(channel, access);
        }
      }
    } catch {
      // Nothing to say and nothing to stop: the cards keep checking.
    }
  }

  // Runtime has already bounded and screened these; this keeps the page's own
  // shape rather than rendering whatever arrived.
  function shopFromAnswer(entry) {
    return {
      address: String(entry && entry.address || ""),
      name: String(entry && entry.name || ""),
      description: String(entry && entry.description || ""),
      categories: (Array.isArray(entry && entry.categories) ? entry.categories : [])
        .map((value) => String(value || ""))
        .filter(Boolean),
      // A CID and nothing else. The page never takes a URL from the directory:
      // this Home serves the picture from its own node.
      imageCid: cidOrEmpty(entry && entry.imageCid),
      itemsCount: Number.isSafeInteger(entry && entry.itemsCount) && entry.itemsCount >= 0
        ? entry.itemsCount
        : 0,
    };
  }

  function skeletonRows(count) {
    return `<div class="store-row-grid store-skeleton-grid">${Array.from({ length: count }, () => `
      <div class="store-row store-row-skeleton" aria-hidden="true">
        <span class="store-skel-icon"></span>
        <span class="store-skel-text"><span></span><span></span></span>
        <span class="store-skel-pill"></span>
      </div>
    `).join("")}</div>`;
  }

  function capsuleToApp(capsule, capsulesByName, interfaceEntries) {
    const role = String(capsule.role || "").toLowerCase();
    const installed = capsule.installed === true;
    const launchable = Boolean(capsule.launchable && capsule.launch_target);
    const dependencies = (Array.isArray(capsule.requires) ? capsule.requires : [])
      .map((entry) => {
        const dependency = capsulesByName.get(String(entry && entry.name || ""));
        return dependency ? publicTitle(dependency) : "";
      })
      .filter(Boolean);
    return {
      id: String(capsule.name || ""),
      name: publicTitle(capsule),
      developer: String(capsule.author || "Unknown publisher"),
      category: appCategory(capsule, role),
      description: publicDescription(capsule),
      version: String(capsule.version || ""),
      installed,
      launchable,
      launchTarget: String(capsule.launch_target || ""),
      role,
      capsuleType: String(capsule.type || capsule.capsule_type || ""),
      trustState: String(capsule.trust_state || ""),
      signatureState: String(capsule.signature_state || ""),
      paymentState: String(capsule.payment_state || ""),
      drmState: String(capsule.drm_state || ""),
      iconRoute: catalogIconRoute(capsule),
      icon: appIcon(role),
      gradient: appGradient(role),
      badges: appBadges(role, capsule, installed),
      acceptedContent: acceptedContentLabels(capsule, capsulesByName, interfaceEntries),
      dependencies,
      availableActions: executableActions(interfaceEntries),
      viewerTitle: String(capsule.viewer_title || ""),
      size: capsule.cid ? "Verified app" : "Local app",
      sourceSummary: capsule.cid ? "SmartWeb" : "Local",
    };
  }

  function appCategory(capsule, role) {
    const category = String(capsule.category || "").toLowerCase();
    const canonical = {
      apps: "apps",
      viewers: "viewers",
      content: "content",
      providers: "providers",
      shells: "shells",
    };
    return canonical[category] || canonical[`${role}s`] || "apps";
  }

  function catalogIconRoute(capsule) {
    const capsuleName = String(capsule?.name || "").trim();
    const variants = (Array.isArray(capsule?.icon) ? capsule.icon : [])
      .filter((entry) => isValidCapsuleIconVariant(capsuleName, entry))
      .sort((left, right) => Number(left.size) - Number(right.size));
    if (!variants.length) {
      return "";
    }
    const preferred = variants.find((entry) => Number(entry.size) === 128) || variants[variants.length - 1];
    return preferred.route;
  }

  function isValidCapsuleIconVariant(capsuleName, entry) {
    if (!capsuleName) {
      return false;
    }
    const size = Number(entry?.size);
    const route = String(entry?.route || "");
    return Number.isFinite(size)
      && size > 0
      && CAPSULE_ICON_ROUTE.test(route)
      && !route.includes("..")
      && route.startsWith(`/apps/${capsuleName}/`);
  }

  function appIcon(role) {
    if (role === "provider") return "server";
    if (role === "shell") return "system";
    if (role === "viewer") return "play";
    if (role === "content") return "document";
    return "package";
  }

  function appGradient(role) {
    if (role === "provider") return "gradient-slate";
    if (role === "viewer") return "gradient-blue";
    if (role === "content") return "gradient-green";
    if (role === "shell") return "gradient-indigo";
    return "gradient-teal";
  }

  function appBadges(role, capsule, installed) {
    const badges = [role === "provider" ? "service" : (role || "app")];
    if (installed) badges.push("installed");
    if (String(capsule.drm_state || "") === "provider") badges.push("ddrm");
    if (String(capsule.payment_state || "") === "provider") badges.push("wallet");
    return [...new Set(badges)];
  }

  function executableActions(interfaceEntries) {
    const actions = [];
    for (const entry of interfaceEntries) {
      for (const binding of Array.isArray(entry?.bindings) ? entry.bindings : []) {
        if (binding?.executable !== true) {
          continue;
        }
        const methodId = String(binding.method || "");
        if (methodId === "capsule.open") {
          actions.push("Open");
          continue;
        }
        const operation = methodId.split(".").filter(Boolean).at(-1);
        if (operation) {
          actions.push(titleCase(operation));
        }
      }
    }
    return [...new Set(actions.filter(Boolean))];
  }

  function acceptedContentLabels(capsule, capsulesByName, interfaceEntries) {
    const labels = (Array.isArray(capsule.accepted_content) ? capsule.accepted_content : [])
      .map((entry) => entry.title || capsulesByName.get(String(entry.name || ""))?.title || entry.name)
      .filter(Boolean);
    const extensions = new Set();
    for (const entry of interfaceEntries) {
      for (const method of Array.isArray(entry?.interface?.methods) ? entry.interface.methods : []) {
        for (const accepted of Array.isArray(method?.input_schema?.accepts) ? method.input_schema.accepts : []) {
          if (accepted?.mode === "unsupported_family_diagnostic") {
            continue;
          }
          for (const extension of Array.isArray(accepted?.extensions) ? accepted.extensions : []) {
            if (extension) {
              extensions.add(String(extension));
            }
          }
        }
      }
    }
    if (extensions.size) {
      labels.push(`${[...extensions].join(", ")} files`);
    }
    return [...new Set(labels)];
  }

  function appSortKey(app) {
    const categoryIndex = categories.findIndex((category) => category.id === app.category);
    return `${String(categoryIndex < 0 ? 99 : categoryIndex).padStart(2, "0")}:${app.name.toLowerCase()}`;
  }

  function render() {
    selectDestination(state.destination, { silent: true });
    updateInstalledBadge();
    const surfaceBlocked = renderSurfaceState();
    if (!surfaceBlocked) {
      renderSections();
    }
    syncHomeMenuManifest();
  }

  function normalizeDestination(id) {
    if (id === "media") return "media";
    if (id === "installed") return "installed";
    if (categories.some((category) => category.id === id)) return id;
    return "discover";
  }

  function destinationTitle(id) {
    if (id === "media") return "Media";
    if (id === "installed") return "Installed";
    const category = categories.find((entry) => entry.id === id);
    if (category) return category.label;
    return "Discover";
  }

  function selectDestination(id, options = {}) {
    state.destination = normalizeDestination(id);
    document.querySelectorAll("[data-destination]").forEach((node) => {
      const selected = node.dataset.destination === state.destination;
      node.classList.toggle("selected", selected);
      if (selected) node.setAttribute("aria-current", "page");
      else node.removeAttribute("aria-current");
    });
    els.storeTitle.textContent = destinationTitle(state.destination);
    const onMedia = state.destination === "media";
    // `media-studio.css` is the design's own stylesheet, scoped to this class
    // so it governs the Media surface and leaves the catalog alone. The scope
    // is switched on with the surface rather than wrapped around part of the
    // page, because both surfaces render into the same sections element.
    els.storeMain.classList.toggle("media-studio", onMedia);
    els.mediaHead.classList.toggle("hidden", !onMedia);
    syncMediaTabSelection();
    syncMediaChrome(onMedia);
    if (!options.silent) {
      const surfaceBlocked = renderSurfaceState();
      if (!surfaceBlocked) {
        renderSections();
      }
    }
  }

  function normalizeMediaTab(id) {
    return MEDIA_TABS.includes(id) ? id : "explore";
  }

  function selectMediaTab(id) {
    const next = normalizeMediaTab(id);
    if (next === state.mediaTab) {
      // The tab a person is already on still takes them back to the whole of
      // it. Pressing Explore while inside one of its sections is how anyone
      // would expect to leave that section.
      if (state.exploreGroup) {
        state.exploreGroup = "";
        renderSections();
      }
      return;
    }
    state.mediaTab = next;
    state.exploreGroup = "";
    state.exploreFilter = "all";
    syncMediaTabSelection();
    syncMediaChrome(true);
    // The surface a person left must not still be in the page while the one
    // they chose is loading. It is hidden rather than gone otherwise, which
    // leaves a shelf of items sitting underneath the word "Shops".
    els.storeSections.innerHTML = "";
    // Asked for the first time the person looks, and not before: asking is
    // what raises the approval request, and a Home whose owner never opened
    // Shops should never be asked about it.
    ensureMediaTabData();
    const surfaceBlocked = renderSurfaceState();
    if (!surfaceBlocked) {
      renderSections();
    }
  }

  // The chrome each surface needs. Adding a listing belongs to the shelves it
  // adds to, and filtering by kind belongs where there are kinds to filter.
  function syncMediaChrome(onMedia) {
    const shelf = onMedia && state.mediaTab !== "shops";
    els.importForm.classList.toggle("hidden", !shelf);
    els.mediaToolbar.classList.toggle("hidden", !shelf);
    ensureMediaTabData();
  }

  // Read this surface again, because a person asked.
  //
  // The market is an outside service and this Home's own records are on disk;
  // either can have moved since the page was drawn, and until now the only
  // way to find out was to reopen the app. A failure to reach the market used
  // to leave Explore quietly showing this Home's own items -- so this also
  // says what it found, which is how anyone would notice it found nothing.
  async function refreshMediaSurface(control) {
    if (state.mediaRefreshing) {
      return;
    }
    state.mediaRefreshing = true;
    if (control) {
      control.disabled = true;
      control.setAttribute("aria-busy", "true");
    }
    try {
      state.catalogLoaded = false;
      state.shopsLoaded = false;
      const reads = state.mediaTab === "shops"
        ? [loadShopsData()]
        : [loadMediaData(), loadCatalogItems(), loadPayTokens()];
      await Promise.all(reads);
      render();
      showToast(refreshSummary());
    } finally {
      state.mediaRefreshing = false;
      if (control) {
        control.disabled = false;
        control.removeAttribute("aria-busy");
      }
    }
  }

  // What the refresh found, in the terms a person would check it against.
  function refreshSummary() {
    if (state.mediaTab === "shops") {
      if (state.shopsNeedsApproval) {
        return "Channels need your approval.";
      }
      return state.shopsUnavailable
        ? "Channel list unavailable."
        : `${state.shops.length} ${state.shops.length === 1 ? "channel" : "channels"}.`;
    }
    const mine = state.mediaListings.filter((listing) => listing.accessState === "creator").length;
    const total = exploreSourceItems().length;
    if (state.catalogFailed && state.mediaTab === "explore") {
      // The important half: the market was not read, which is not the same
      // as a market with nothing in it.
      return "Couldn’t reach the market.";
    }
    return `${total} ${total === 1 ? "item" : "items"} · ${mine} yours.`;
  }

  // What the surface a person is looking at needs, asked for once.
  //
  // Called when Media is opened as well as when a tab is chosen, because
  // Explore is where a person lands: a load that only happened on a switch
  // would never happen for the tab they never switch to.
  //
  // Asking is what raises an approval, so a surface nobody opens asks for
  // nothing.
  function ensureMediaTabData() {
    if (state.destination !== "media") {
      return;
    }
    if (state.mediaTab === "shops" && !state.shopsLoaded && !state.shopsLoading) {
      loadShopsData().then(render);
    }
    if (state.mediaTab === "explore" && !state.catalogLoaded && !state.catalogLoading) {
      loadCatalogItems({ retryOnce: true }).then(render);
    }
  }

  function syncMediaTabSelection() {
    els.mediaTabs.querySelectorAll("[data-media-tab]").forEach((node) => {
      const selected = node.dataset.mediaTab === state.mediaTab;
      node.classList.toggle("selected", selected);
      node.setAttribute("aria-selected", selected ? "true" : "false");
    });
  }

  function matchesSearch(app) {
    if (!state.search) return true;
    const haystack = [app.name, app.description, app.developer, app.role, app.category, app.badges.join(" ")]
      .join(" ")
      .toLowerCase();
    return haystack.includes(state.search);
  }

  function filteredByDestination() {
    return state.apps.filter((app) => {
      if (state.destination === "installed") return app.installed && matchesSearch(app);
      if (state.destination !== "discover" && app.category !== state.destination) return false;
      return matchesSearch(app);
    });
  }

  function renderSurfaceState() {
    const surface = surfaceState();
    els.loadingState.classList.toggle("hidden", !surface.loading);
    if (surface.loading) {
      els.loadingState.innerHTML = skeletonRows(surface.destination === "media" ? 3 : 6);
      els.storeSections.classList.add("hidden");
      els.loadError.classList.add("hidden");
      els.loadError.innerHTML = "";
      return true;
    }
    els.loadingState.innerHTML = "";
    els.loadingState.classList.add("hidden");
    if (!surface.error) {
      els.loadError.classList.add("hidden");
      els.loadError.innerHTML = "";
      return false;
    }
    els.storeSections.classList.add("hidden");
    els.loadError.classList.remove("hidden");
    els.loadError.innerHTML = `
      <div class="store-error-card">
        <div class="store-error-title">${escapeHtml(surface.title)}</div>
        <div class="store-error-body">${escapeHtml(surface.error)}</div>
        <button type="button" class="store-pill" data-action="${surface.retryAction}">Retry</button>
      </div>
    `;
    bindAppActions(els.loadError);
    return true;
  }

  function surfaceState() {
    if (state.destination === "media") {
      // Each Media tab reports its own loading and failure, so a channel
      // directory that cannot be reached leaves the shelf exactly as it was.
      if (state.mediaTab === "shops") {
        return {
          destination: "shops",
          loading: state.shopsLoading,
          error: state.shopsLoadError,
          retryAction: "retry-shops",
          title: "Couldn’t load channels",
        };
      }
      return {
        destination: "media",
        loading: state.mediaLoading,
        error: state.mediaLoadError,
        retryAction: "retry-media",
        title: "Couldn’t load your items",
      };
    }
    return {
      destination: "apps",
      loading: state.appLoading,
      error: state.appLoadError,
      retryAction: "retry",
      title: "Couldn’t load apps",
    };
  }

  function renderSections() {
    if (surfaceState().loading || surfaceState().error) {
      return;
    }
    els.storeSections.classList.remove("hidden");
    if (state.destination === "media") {
      renderMediaSections();
      return;
    }
    if (state.destination === "installed") {
      renderInstalledSections();
      return;
    }
    if (state.destination !== "discover") {
      renderCategorySections();
      return;
    }
    renderDiscoverSections();
  }

  function renderDiscoverSections() {
    const parts = [];
    const installed = state.apps.filter((app) => app.installed && matchesSearch(app));
    if (installed.length) {
      parts.push(renderSection("Installed", installed.slice(0, 9), {
        seeAllDestination: "installed",
      }));
    }
    for (const category of categories) {
      const apps = state.apps.filter((app) => app.category === category.id && matchesSearch(app));
      if (!apps.length) {
        continue;
      }
      parts.push(renderSection(category.label, apps, {
        seeAllDestination: category.id,
      }));
    }
    if (!parts.length) {
      els.storeSections.innerHTML = emptyState(
        state.search ? "No results" : "No apps to show",
        state.search ? "Try a different search." : "Apps on this Home will appear here.",
        icons.search,
      );
      return;
    }
    els.storeSections.innerHTML = parts.join("");
    bindAppActions(els.storeSections);
  }

  function renderCategorySections() {
    const apps = filteredByDestination();
    if (!apps.length) {
      els.storeSections.innerHTML = emptyState(
        state.search ? "No results" : "No apps in this category",
        state.search ? "Try a different search." : "Choose another category from the sidebar.",
        icons.search,
      );
      return;
    }
    els.storeSections.innerHTML = renderSection(destinationTitle(state.destination), apps);
    bindAppActions(els.storeSections);
  }

  function renderInstalledSections() {
    const installed = filteredByDestination();
    if (!installed.length) {
      els.storeSections.innerHTML = emptyState(
        state.search ? "No results" : "No apps installed",
        state.search ? "Try a different search." : "Installed apps appear here.",
        icons.package,
      );
      return;
    }
    els.storeSections.innerHTML = renderSection("Installed", installed);
    bindAppActions(els.storeSections);
  }

  function renderMediaSections() {
    if (state.mediaTab === "shops") {
      renderShopsSections();
      return;
    }
    // "My listings" is the same shelf asked a narrower question, so it is the
    // same rendering rather than a second one that could disagree with it.
    renderExploreSections();
  }

  // Four answers, and each one says which it is. An unapproved read, an index
  // that could not be reached and a market with nothing in it look identical
  // on screen unless the page insists on telling them apart.
  function renderShopsSections() {
    if (state.shopsNeedsApproval) {
      els.storeSections.innerHTML = `
        ${emptyState(
          "Channels need your approval",
          state.shopsAwaitingApproval
            ? "Approve “Marketplace requests the channel and sales index” in your Inbox, then choose Check again."
            : "Marketplace can list the channels publishing protected items by reading an on-chain index. Buying and opening work without it.",
          icons.package,
        )}
        <div class="store-empty-actions">
          <button type="button" class="store-pill" data-action="approve-shops">${
            state.shopsAwaitingApproval ? "Check again" : "Allow channel list"
          }</button>
        </div>
      `;
      bindAppActions(els.storeSections);
      return;
    }
    if (state.shopsUnavailable) {
      els.storeSections.innerHTML = `
        ${emptyState(
          "Channel list unavailable",
          "The on-chain index could not be reached. Nothing else is affected.",
          icons.package,
        )}
        <div class="store-empty-actions">
          <button type="button" class="store-pill" data-action="retry-shops">Try again</button>
        </div>
      `;
      bindAppActions(els.storeSections);
      return;
    }
    const shops = filteredShops();
    if (!shops.length) {
      els.storeSections.innerHTML = emptyState(
        state.search ? "No results" : "No channels listed",
        state.search ? "Try a different search." : "The index answered with no channels.",
        icons.package,
      );
      return;
    }
    els.storeSections.innerHTML = `
      <section class="store-section">
        <div class="store-section-head">
          <h2 class="store-section-title">Channels</h2>
        </div>
        <div class="store-row-grid">
          ${shops.map(renderShopRow).join("")}
        </div>
      </section>
    `;
    bindAppActions(els.storeSections);
  }

  function filteredShops() {
    return state.shops.filter((shop) => {
      if (!state.search) {
        return true;
      }
      return [shop.name, shop.description, shop.categories.join(" "), shop.address]
        .join(" ")
        .toLowerCase()
        .includes(state.search);
    });
  }

  // A shop card names the channel and says how much is in it. The address is
  // shown abbreviated because a channel with no name is still a channel a
  // person may recognise, and it is the only durable thing on the card.
  function renderShopRow(shop) {
    const itemLabel = shop.itemsCount === 1 ? "1 item" : `${shop.itemsCount} items`;
    const categories = shop.categories
      .map((category) => `<span class="store-row-fact">${escapeHtml(titleCase(category))}</span>`)
      .join("");
    return `
      <article class="store-row store-row-static store-row-shop" data-shop="${escapeAttr(shop.address)}">
        ${shopIconHtml(shop)}
        <div class="store-row-text">
          <div class="store-row-title">${escapeHtml(shop.name || abbreviateAddress(shop.address))}</div>
          <div class="store-row-sub">${escapeHtml(shop.description || abbreviateAddress(shop.address))}</div>
          <div class="store-row-facts">
            <span class="store-row-fact">${escapeHtml(itemLabel)}</span>
            ${categories}
          </div>
        </div>
        ${shopActionButton(shop)}
      </article>
    `;
  }

  // CIDv0 or CIDv1, and nothing else. Runtime has already screened this; the
  // page screens it again because it is about to put it in a URL, and the one
  // thing that must never end up there is something the directory chose.
  function cidOrEmpty(value) {
    const cid = String(value || "").trim();
    return /^(Qm[1-9A-HJ-NP-Za-km-z]{44}|b[a-z2-7]{45,95})$/.test(cid) ? cid : "";
  }

  // A channel's own picture, served by this Home from its own node through
  // `/ipfs/:cid` -- the route that already fetches and caches CID content for
  // every other surface, rather than a second content path built for this one.
  //
  // The glyph stays underneath for a picture that does not arrive: a CID no
  // node has yet is the ordinary case, not an error.
  function shopIconHtml(shop) {
    if (!shop.imageCid) {
      return `<span class="app-icon gradient-blue store-row-icon" aria-hidden="true">${icons.package}</span>`;
    }
    const route = `/ipfs/${encodeURIComponent(shop.imageCid)}`;
    // The gradient rides along so that a picture which never arrives falls
    // back to the same tile a channel without one gets, rather than to a bare
    // glyph on nothing.
    return `<span class="app-icon app-icon-raster gradient-blue store-row-icon"><img class="app-icon-img" src="${escapeAttr(route)}" alt="" draggable="false"><span class="app-icon-glyph" hidden>${icons.package}</span></span>`;
  }

  // A subscription is offered to whoever could use one, and to nobody else: a
  // channel this Home administers and one it is already subscribed to have
  // nothing to sell this person.
  //
  // An unconfirmed state offers nothing either. "Not subscribed" is the answer
  // that draws the button, so a chain that could not be read must not be
  // reported as one.
  function shopActionButton(shop) {
    const access = state.shopAccess.get(shop.address);
    if (access === "administrator") {
      return `<span class="store-row-state">Yours</span>`;
    }
    if (access === "subscribed") {
      return `<span class="store-row-state">Subscribed</span>`;
    }
    if (access === "none") {
      // Subscribing is a payment, and the path that makes one is not built
      // yet. The control says so rather than doing nothing when pressed.
      return `<button class="store-pill" type="button" data-action="subscribe-channel" data-shop="${escapeAttr(shop.address)}" disabled title="Subscribing isn't available on this Home yet.">Subscribe</button>`;
    }
    return `<span class="store-row-state">${access === "unknown" ? "Access unknown" : "Checking…"}</span>`;
  }


  // The chips: every kind, with how many of it this shelf holds. A kind with
  // nothing in it is shown and disabled rather than hidden, so the set a
  // person can filter by does not change shape under them.
  function renderExploreChips(counts, total) {
    const chips = [{ id: "all", title: "All", count: total }]
      .concat(EXPLORE_GROUPS.map((group) => ({
        id: group.id,
        title: group.title,
        count: counts.get(group.id) || 0,
      })))
      .filter((chip) => chip.id !== "other" || chip.count > 0);
    els.mediaChips.innerHTML = chips.map((chip) => `
      <button class="chip" type="button" data-action="explore-filter" data-filter="${escapeAttr(chip.id)}"
              aria-pressed="${chip.id === state.exploreFilter ? "true" : "false"}"
              ${chip.count === 0 && chip.id !== "all" ? "disabled" : ""}>
        ${escapeHtml(chip.title)}${chip.count > 0 ? `<span class="n">${chip.count}</span>` : ""}
      </button>
    `).join("");
    bindAppActions(els.mediaChips);
  }

  // Newest first by default, and the other orders a shelf is usually asked
  // for. A price is a uint256, so it is compared as one -- never as a number
  // this app rounded on the way.
  function sortExploreListings(listings) {
    const sorted = [...listings];
    if (state.exploreSort === "title") {
      return sorted.sort((left, right) => cardTitle(left).localeCompare(cardTitle(right)));
    }
    if (state.exploreSort === "price-asc" || state.exploreSort === "price-desc") {
      const direction = state.exploreSort === "price-asc" ? 1n : -1n;
      return sorted.sort((left, right) => {
        const difference = (BigInt(left.price) - BigInt(right.price)) * direction;
        return difference === 0n ? 0 : difference < 0n ? -1 : 1;
      });
    }
    return sorted.sort((left, right) => right.publishedAt - left.publishedAt);
  }

  function cardTitle(listing) {
    const meta = listingMetadata.get(itemKey(listing));
    return (meta && meta.title) || listing.displayName;
  }

  function renderExploreSections() {
    if (state.mediaTab === "explore" && !state.catalogReady) {
      renderMarketPending();
      return;
    }
    const listings = filteredMediaListings();
    const notice = shelfNotice();
    if (!listings.length) {
      // The note rides with the empty shelf too. A person whose only item was
      // refused is looking at an empty surface that needs an explanation more
      // than a full one does.
      els.storeSections.innerHTML = notice + emptyState(
        state.search ? "No results" : "No protected items yet",
        state.search ? "Try a different search." : "Items listed on this Home appear here.",
        icons.media,
      );
      return;
    }
    const sorted = sortExploreListings(listings);
    const counts = new Map();
    for (const listing of sorted) {
      const id = exploreGroupId(listing);
      counts.set(id, (counts.get(id) || 0) + 1);
    }
    renderExploreChips(counts, sorted.length);
    const groups = groupExploreListings(sorted)
      .filter((group) => state.exploreFilter === "all" || group.id === state.exploreFilter);
    if (!groups.length) {
      els.storeSections.innerHTML = notice + emptyState(
        "Nothing of that kind",
        "This shelf holds nothing in the kind you picked.",
        icons.media,
      );
      return;
    }
    // A group opened from its own "More" link shows everything it has.
    const opened = groups.find((group) => group.id === state.exploreGroup);
    if (opened) {
      els.storeSections.innerHTML = `
        ${notice}
        <section class="section">
          <div class="section-head">
            <h2>${escapeHtml(opened.title)} <span class="n">${opened.listings.length}</span></h2>
            <button type="button" class="store-see-all" data-action="explore-all">&larr; All items</button>
          </div>
          <div class="grid ${escapeAttr(opened.shape === "page" ? "docs" : "videos")}">
            ${opened.listings.map(renderMediaCard).join("")}
            ${state.mediaTab === "mine" ? addCardTile(opened.id) : ""}
          </div>
        </section>
      `;
      bindAppActions(els.storeSections);
      requestListingMetadata(opened.listings);
      return;
    }
    const arriving = state.mediaTab === "explore" && state.catalogLoading;
    els.storeSections.innerHTML = notice + marketNotice() + groups.map((group) => `
      <section class="section">
        <div class="section-head">
          <h2>${escapeHtml(group.title)} <span class="n">${group.listings.length}</span></h2>
          ${group.listings.length > group.limit
            ? `<button type="button" class="store-see-all" data-action="explore-more" data-group="${escapeAttr(group.id)}">See all</button>`
            : ""}
        </div>
        <div class="grid ${escapeAttr(group.shape === "page" ? "docs" : "videos")}">
          ${group.listings.slice(0, group.limit).map(renderMediaCard).join("")}
          ${arriving && group.listings.length < group.limit
            ? skeletonCards(group.shape, group.limit - group.listings.length)
            : ""}
          ${state.mediaTab === "mine" && group.listings.length <= group.limit ? addCardTile(group.id) : ""}
        </div>
      </section>
    `).join("");
    bindAppActions(els.storeSections);
    requestListingMetadata(sorted.slice(0, 4 * EXPLORE_GROUPS.length));
  }

  // The end of a section in My listings: where a new item of that kind comes
  // from. Explore is other people's shelf and offers no such thing -- adding
  // to it means buying, not minting.
  //
  // Making one is Creator's job, not this app's, so the tile launches Creator
  // through Home exactly as any other launch does. It is offered only when
  // Creator is actually installed and launchable -- a tile that opens nothing
  // is worse than a tile that is not there, so when it cannot launch it says
  // what is missing instead.
  function addCardTile(groupId) {
    const noun = {
      videos: "a video",
      musics: "audio",
      documents: "a document",
      images: "an image",
      other: "an item",
    }[groupId] || "an item";
    const creator = state.apps.find((app) => app.id === CREATOR_CAPSULE_ID);
    if (!creator || !creator.launchable || !creator.launchTarget) {
      return `
        <div class="add-tile">
          <span class="plus">${spriteIcon("i-plus", "", "width:20px;height:20px;stroke-width:2")}</span>
          <strong>Add ${escapeHtml(noun)}</strong><span>Creator isn’t installed on this Home.</span>
        </div>
      `;
    }
    return `
      <button class="add-tile" type="button" data-action="open-creator">
        <span class="plus">${spriteIcon("i-plus", "", "width:20px;height:20px;stroke-width:2")}</span>
        <strong>Add ${escapeHtml(noun)}</strong><span>Protect and list a new one in Creator</span>
      </button>
    `;
  }

  // Explore before the market has answered: loading while a read is in
  // flight (the automatic retry included), and a plain statement with a way
  // to ask again once it could not be reached. This Home's own items stay in
  // My listings meanwhile; standing alone here they would read as the market.
  function renderMarketPending() {
    renderExploreChips(new Map(), 0);
    if (state.catalogFailed && !state.catalogLoading) {
      els.storeSections.innerHTML = `
        <div class="market-unreachable" role="status">
          ${emptyState(
            "Couldn’t reach the market.",
            "Other people’s items could not be read just now. Your own items are in My listings.",
            icons.media,
          )}
        </div>
        <div class="store-empty-actions">
          <button type="button" class="store-pill" data-action="retry-market">Try again</button>
        </div>
      `;
      bindAppActions(els.storeSections);
      return;
    }
    els.storeSections.innerHTML = `
      <p class="sr-only" role="status">Loading the market…</p>
      <section class="section" aria-hidden="true">
        <div class="grid videos">${skeletonCards("video", EXPLORE_ROW_LIMIT)}</div>
      </section>
    `;
  }

  // A market that needs approval answers with nothing, and Explore keeps
  // this Home's own items -- which look exactly like a market with nothing
  // else in it unless it says so.
  function marketNotice() {
    if (state.mediaTab !== "explore" || !state.catalogLoaded) {
      return "";
    }
    if (state.catalogNeedsApproval) {
      return `
        <p class="store-inline-note">
          Other people’s items need your approval. Approve “Marketplace requests the channel list” in your Inbox, then refresh.
        </p>
      `;
    }
    return "";
  }

  // A card that is on its way, in the shape the real one will take.
  //
  // The design's own skeleton: the shelf keeps its layout while it fills, so
  // nothing jumps once the items arrive.
  function skeletonCards(shape, count) {
    const frame = shape === "page" ? "height:236px" : "aspect-ratio:16/9";
    return Array.from({ length: count }, () => `
      <div class="card skeleton ${shape === "page" ? "doc" : "video"}" aria-hidden="true">
        <div class="thumb" style="${frame}"></div>
        <div class="card-body" style="gap:8px">
          <div class="bar" style="width:80%"></div>
          <div class="bar" style="width:55%"></div>
        </div>
        <div style="padding:8px 16px 16px"><div class="bar" style="width:40%"></div></div>
      </div>
    `).join("");
  }

  // One section per kind, and a section only when it has something in it. An
  // empty "Musics" heading tells a person nothing except that this app knows
  // the word.
  function groupExploreListings(listings) {
    return EXPLORE_GROUPS
      .map((group) => ({
        id: group.id,
        title: group.title,
        shape: group.shape,
        limit: group.limit || EXPLORE_ROW_LIMIT,
        listings: listings.filter((listing) => exploreGroupId(listing) === group.id),
      }))
      .filter((group) => group.listings.length > 0);
  }

  // The first section that claims this item's MIME. Order decides: "Other"
  // matches everything and is last, so it only ever gets what nothing else
  // wanted.
  function exploreGroupFor(listing) {
    const mime = String(listing.mimeType || "").toLowerCase();
    return EXPLORE_GROUPS.find((group) => group.matches(mime)) || EXPLORE_GROUPS[EXPLORE_GROUPS.length - 1];
  }

  function exploreGroupId(listing) {
    return exploreGroupFor(listing).id;
  }

  // The format badge: what this item IS, in the shortest true word.
  //
  // Taken from the MIME's subtype rather than from a table of names, so an
  // item of a kind nobody anticipated still gets a badge instead of nothing.
  // Deliberately no resolution beside it: the design shows "MP4 · 720p", and
  // this Home is told the codec but never the frame size, so it says the part
  // it knows rather than a number it guessed.
  function formatBadge(listing) {
    const mime = String(listing.mimeType || "").toLowerCase();
    const subtype = mime.slice(mime.indexOf("/") + 1);
    const named = {
      "quicktime": "MOV",
      "x-matroska": "MKV",
      "mpeg": mime.startsWith("audio/") ? "MP3" : "MPEG",
      "svg+xml": "SVG",
      "epub+zip": "EPUB",
      "vnd.openxmlformats-officedocument.wordprocessingml.document": "DOCX",
      "plain": "TXT",
      "jpeg": "JPG",
    }[subtype];
    const badge = named || subtype.replace(/^x-/, "").toUpperCase();
    return badge.length > 0 && badge.length <= 6 ? badge : "FILE";
  }

  // The state badge, top right, and only when there is something to say. An
  // item someone else listed and can still sell wears nothing.
  function stateBadge(listing) {
    if (listing.accessState === "creator") {
      return { label: "Listed by you", tone: "own" };
    }
    if (listing.accessState === "purchased") {
      return { label: ownedLabel(listing), tone: "owned" };
    }
    if (isSoldOut(listing)) {
      return { label: "Sold out", tone: "gone" };
    }
    return null;
  }

  // What this Home actually holds of an item it bought. Only a row with a
  // mint of this Home's own -- its own listing, or an adopted purchase -- has
  // a copy in the Library. A bought market item without one is `pending`
  // when it can open here (Download copy adopts it), `foreign` when it opens
  // only on ela.city, and `unlearned` until this page reads its listing.
  function ownedHolding(listing) {
    if (listing.accessState !== "purchased") {
      return "";
    }
    if (!listing.catalogOnly) {
      return "library";
    }
    const readability = marketAssetReadability.get(catalogLedgerKey(listing));
    if (readability === "verified" || readability === "unverified") {
      return "pending";
    }
    return readability === "foreign" ? "foreign" : "unlearned";
  }

  // "In your library" is a promise that the Library shows the item, so only
  // a copy this Home holds wears it. Every other bought item is "Purchased".
  function ownedLabel(listing) {
    return ownedHolding(listing) === "library" ? "In your library" : "Purchased";
  }

  // Who listed the item, shortened. A seller is a public chain fact; the
  // buyer's own account never reaches this page.
  function sellerByline(listing) {
    if (listing.accessState === "creator") {
      return "by you";
    }
    return listing.sellerAddress ? `by ${abbreviateAddress(listing.sellerAddress)}` : "";
  }

  // Nothing left to sell. Read from the quantity Runtime published, as a
  // string, because a uint256 is not a number this app may hold.
  function isSoldOut(listing) {
    return uint256Decimal(listing.quantity) === "0";
  }

  // A price, short enough for a card and still exactly itself.
  //
  // The design shows "100K units" and "10¹⁷ units", so long values are
  // shortened -- but only when the short form is EXACT. 100000 is exactly
  // 100K; 123456 is not 123.5K, so it stays 123456. A price a person reads is
  // either the price or it is a lie, and the full value is on the element for
  // anything that needs it.
  function compactUnits(decimal) {
    const digits = String(decimal || "0");
    // A clean power of ten says itself best as one, once it is long enough
    // that a suffix would still leave a wall of zeros.
    if (digits.length > 9 && /^10*$/.test(digits)) {
      return `10${superscript(digits.length - 1)}`;
    }
    for (const [suffix, zeros] of [["T", 12], ["B", 9], ["M", 6], ["K", 3]]) {
      if (digits.length > zeros && /^0+$/.test(digits.slice(-zeros))) {
        const head = digits.slice(0, -zeros);
        // A head longer than this is not shorter than the number it replaced.
        if (head.length <= 4) {
          return `${head}${suffix}`;
        }
      }
    }
    return digits;
  }

  function superscript(value) {
    const glyphs = "⁰¹²³⁴⁵⁶⁷⁸⁹";
    return String(value)
      .replace("-", "⁻")
      .replace(/[0-9]/g, (digit) => glyphs[Number(digit)]);
  }

  // One item, as a card.
  //
  // Six states, and each one says which it is rather than leaving a person to
  // work it out from what is missing: someone else's listing to buy, one this
  // Home listed, one it already holds, one with nothing left to sell, a
  // purchase still running, and a card whose metadata has not arrived yet.
  // One item, as a card.
  //
  // This is `media-studio.html`'s `videoCard` and `docCard`, kept in its own
  // class names and structure so the next change can be diffed against the
  // design instead of translated out of it. What differs is data and actions:
  // the values come from Runtime's listing, and every control carries the
  // `data-action` this app already answers.
  function renderMediaCard(listing) {
    const kind = exploreGroupFor(listing).shape === "page" ? "doc" : "video";
    const mint = escapeAttr(listing.mintId);
    const state = cardStateName(listing);
    const meta = listingMetadata.get(itemKey(listing));
    const title = (meta && meta.title) || listing.displayName;
    const cover = meta && meta.coverCid ? `/ipfs/${escapeAttr(meta.coverCid)}` : "";
    const stock = `${escapeHtml(uint256Decimal(listing.quantity))} available`;
    const seller = listing.accessState === "creator" ? " · by you" : "";
    if (kind === "doc") {
      const thumb = cover
        ? `<img class="page app-icon-img" src="${cover}" alt="" loading="lazy" data-cover="pending">`
        : "";
      return `
      <article class="card doc store-row-media" data-mint="${mint}" data-id="${escapeAttr(itemKey(listing))}" data-state="${escapeAttr(state)}">
        <div class="thumb app-icon-raster${cover ? " thumb-pending" : ""}">
          ${thumb}
          <div class="page-fallback app-icon-glyph"${cover ? " hidden" : ""}><b>${escapeHtml(title)}</b><i></i><i></i><i></i><i></i></div>
          <span class="tag left">${escapeHtml(formatBadge(listing))}</span>
          ${statusTag(state, listing)}
        </div>
        <div class="card-body">
          <h3 class="card-title" title="${escapeAttr(title)}">${escapeHtml(title)}</h3>
          <div class="card-meta">${state === "owned" ? escapeHtml(sellerByline(listing) || ownedLabel(listing)) : `${stock}${seller}`}</div>
          <div class="card-progress">${buyStateNoteMarkup(listing)}</div>
        </div>
        <div class="card-foot">
          ${priceBlock(listing, state)}
          <div class="card-actions">${cardActions(listing, state, "btn-sm", true)}</div>
        </div>
      </article>`;
    }
    const thumb = cover
      ? `<img class="app-icon-img" src="${cover}" alt="" loading="lazy" data-cover="pending">`
      : "";
    // An image is looked at, not played, so the control over it says so.
    const previewIcon = exploreGroupId(listing) === "images" ? "i-eye" : "i-play";
    // Only a mint this Home holds opens here; a bought market item without
    // one has nothing for the play control to open.
    const play = state !== "soldout" && listing.accessState !== "available" && !listing.catalogOnly
      ? `<button class="play" type="button" data-action="open-media" data-mint="${mint}" aria-label="Open ${escapeAttr(title)}">${spriteIcon(previewIcon, "fill", "width:22px;height:22px")}</button>`
      : "";
    return `
    <article class="card video store-row-media" data-mint="${mint}" data-id="${escapeAttr(itemKey(listing))}" data-state="${escapeAttr(state)}">
      <div class="thumb app-icon-raster${cover ? " thumb-pending" : ""}">
        ${thumb}
        <div class="video-fallback app-icon-glyph"${cover ? " hidden" : ""}>${spriteIcon("i-media")}</div>
        <span class="tag left">${escapeHtml(formatBadge(listing))}</span>
        ${statusTag(state, listing)}
        ${play}
      </div>
      <div class="card-body">
        <h3 class="card-title" title="${escapeAttr(title)}">${escapeHtml(title)}</h3>
        <div class="card-meta mono" title="${escapeAttr(listing.displayName)}">${escapeHtml(mediaCardSubtitle(listing, title))}</div>
        <div class="card-progress">${buyStateNoteMarkup(listing)}</div>
      </div>
      <div class="card-foot">
        <div>${priceBlock(listing, state)}${state === "owned" ? "" : `<div class="stock">${stock}</div>`}</div>
        <div class="card-actions">${cardActions(listing, state)}</div>
      </div>
    </article>`;
  }

  // The line under the title, which must never repeat it.
  //
  // For an item this Home holds, that line is the file name in mono: it says
  // WHICH copy you are looking at, since two mints of one title are one title
  // and two files, and it differs from the title because the title comes from
  // the published document.
  //
  // An item known only to the index has no file name -- its `name` IS the
  // title -- so the line would echo the title and say nothing. It says who is
  // selling it and how many are left instead, which is what a card about
  // someone else's item is otherwise missing. A bought item keeps who listed
  // it and drops the stock: how many are left is a buyer's question, and
  // this person already bought.
  function mediaCardSubtitle(listing, title) {
    const fileName = listing.displayName;
    if (fileName && fileName !== title) {
      return listing.codecs ? `${fileName} · ${listing.codecs}` : fileName;
    }
    const seller = sellerByline(listing);
    if (listing.accessState === "purchased") {
      return seller || ownedLabel(listing);
    }
    const stock = `${uint256Decimal(listing.quantity)} available`;
    return seller ? `${stock} · ${seller}` : stock;
  }

  // The design's four card states, named from what Runtime says.
  function cardStateName(listing) {
    if (listing.accessState === "purchased") {
      return "owned";
    }
    if (listing.accessState === "creator") {
      return "mine";
    }
    return isSoldOut(listing) ? "soldout" : "default";
  }

  function statusTag(state, listing) {
    if (state === "mine") {
      return `<span class="tag right mine">Listed by you</span>`;
    }
    if (state === "owned") {
      return `<span class="tag right owned">${escapeHtml(ownedLabel(listing))}</span>`;
    }
    if (state === "soldout") {
      return `<span class="tag right sold">Sold out</span>`;
    }
    return "";
  }

  function priceBlock(listing, state) {
    if (state === "owned") {
      return `<div class="price owned">✓ ${escapeHtml(ownedLabel(listing))}</div>`;
    }
    const money = formatMoney(listing);
    return `<div class="price${state === "soldout" ? " sold" : ""}" title="${escapeAttr(money.title)}">${escapeHtml(money.value)}${money.unit ? ` <small>${escapeHtml(money.unit)}</small>` : ""}</div>`;
  }

  // A price a person can read.
  //
  // The listing carries a uint256 and the address it is priced in; this Home
  // says how many decimals that address has and what it is called. The
  // scaling is integer arithmetic on the digits -- a price is never put
  // through a float, which is why this shifts a decimal point by hand rather
  // than dividing.
  //
  // A token this Home does not know is shown in base units, and its address
  // is named in the title. That is useless to read and honest, which is the
  // right way round: inventing decimals would turn an unknown price into a
  // confident wrong one. The native token (the zero address) is the one
  // exception, because its decimals are the chain's, not a contract's.
  function formatMoney(listing) {
    const full = uint256Decimal(listing.price);
    const payToken = String(listing.payToken || "").toLowerCase();
    // The zero address is the chain's native token: 18 decimals whether or
    // not this Home's table lists it, named by the table when it does.
    const token = payTokens.get(payToken)
      || (payToken === NATIVE_PAY_TOKEN ? { symbol: "native", decimals: 18 } : null);
    if (!token) {
      return { value: compactUnits(full), unit: "units", title: `${full} base units of ${payToken}` };
    }
    const exact = trimZeros(shiftDecimal(full, token.decimals));
    const shown = readableAmount(exact);
    const mark = CURRENCY_MARKS[token.symbol.toUpperCase()];
    // A token with a mark of its own wears it, which is both shorter and what
    // a person recognises. One without keeps its name beside the number.
    return mark
      ? { value: `${mark}${shown}`, unit: "", title: `${exact} ${token.symbol}` }
      : { value: shown, unit: token.symbol, title: `${exact} ${token.symbol}` };
  }

  // `digits` divided by 10^decimals, exactly, as a decimal string. No
  // division and no Number: the point is moved through the digit string.
  function shiftDecimal(digits, decimals) {
    const value = String(digits || "0");
    if (decimals <= 0) {
      return value;
    }
    if (value.length <= decimals) {
      return `0.${value.padStart(decimals, "0")}`;
    }
    return `${value.slice(0, value.length - decimals)}.${value.slice(value.length - decimals)}`;
  }

  // An amount a card can hold, written the way token prices are written.
  //
  // A token with eighteen decimals makes very small prices very long:
  // 0.0000000000001 ETH is thirteen leading zeros, unreadable and wider than
  // the card. The run of zeros is counted instead of printed -- 0.0₁₃1 --
  // which is what Uniswap, DexScreener and CoinGecko all do, so a person who
  // has seen a token price before already reads it.
  //
  // Nothing is rounded and nothing is dropped: the subscript is a count, not
  // an approximation, and the exact amount stays on the element as well.
  function readableAmount(exact) {
    const value = String(exact || "0");
    if (!value.startsWith("0.")) {
      return value;
    }
    const fraction = value.slice(2);
    const leadingZeros = fraction.length - fraction.replace(/^0+/, "").length;
    // Two zeros are shorter written out than counted.
    if (leadingZeros < 3) {
      return value;
    }
    return `0.0${subscript(leadingZeros)}${fraction.replace(/^0+/, "")}`;
  }

  function subscript(value) {
    const glyphs = "₀₁₂₃₄₅₆₇₈₉";
    return String(value).replace(/[0-9]/g, (digit) => glyphs[Number(digit)]);
  }

  // Trailing zeros after a decimal point say nothing. A whole number keeps no
  // point at all.
  function trimZeros(value) {
    if (!value.includes(".")) {
      return value;
    }
    return value.replace(/0+$/, "").replace(/\.$/, "");
  }

  // The design's secondary icons and primary button, carrying this app's
  // actions. A sold-out item keeps the design's disabled control rather than
  // its "Notify me": this Home cannot tell anyone when a copy frees up.
  function cardActions(listing, state, size = "", overflow = false) {
    const mint = escapeAttr(listing.mintId);
    if (state === "soldout") {
      return `<button class="btn btn-ghost ${size}" type="button" disabled>Sold out</button>`;
    }
    // Share and Download both answer to a mint this Home holds. A catalog
    // item known only to the index has none, so it offers neither -- only
    // the Buy that reaches it through the market path below.
    const secondary = !listing.catalogOnly && (state === "mine" || state === "owned")
      ? cardSecondaryActions(listing, state, size, overflow)
      : "";
    return `${secondary}${mediaActionButton(listing, size)}`;
  }

  // The same two things either way: the link a creator passes on, and the
  // rebuild anyone holding a copy may ask for.
  //
  // A document card has less room beside a taller page, so the design folds
  // them behind one control there and shows them plainly on a video. The
  // actions are identical; only how many buttons they occupy differs.
  function cardSecondaryActions(listing, state, size, overflow) {
    const mint = escapeAttr(listing.mintId);
    const downloading = pendingDownloads.has(listing.mintId);
    const share = state === "mine";
    if (!overflow) {
      return `${share
        ? `<button class="btn btn-icon ${size}" type="button" data-action="share-listing" data-mint="${mint}" aria-label="Share listing">${spriteIcon("i-share", "", "width:17px;height:17px")}</button>`
        : ""}
        <button class="btn btn-icon ${size}" type="button" data-action="download-copy" data-mint="${mint}" aria-label="Download"${downloading ? " disabled aria-busy=\"true\"" : ""}>${spriteIcon("i-download", "", "width:17px;height:17px")}</button>`;
    }
    return `
      <div class="menu">
        <button class="btn btn-icon ${size}" type="button" data-action="card-menu" data-mint="${mint}"
                aria-label="More actions" aria-haspopup="menu" aria-expanded="false">${spriteIcon("i-more", "fill", "width:16px;height:16px")}</button>
        <ul class="menu-list" role="menu" hidden>
          ${share
            ? `<li><button role="menuitem" type="button" data-action="share-listing" data-mint="${mint}">${spriteIcon("i-share", "", "width:15px;height:15px")}Share</button></li>`
            : ""}
          <li><button role="menuitem" type="button" data-action="download-copy" data-mint="${mint}"${downloading ? " disabled aria-busy=\"true\"" : ""}>${spriteIcon("i-download", "", "width:15px;height:15px")}Download</button></li>
        </ul>
      </div>`;
  }

  // An icon from the design's own sprite, drawn the way the design draws it.
  function spriteIcon(id, extraClass = "", style = "") {
    return `<svg class="icon ${extraClass}"${style ? ` style="${escapeAttr(style)}"` : ""} aria-hidden="true"><use href="#${escapeAttr(id)}"/></svg>`;
  }

  // Reads each item's published metadata document from this Home's own node,
  // once, and redraws when they arrive.
  //
  // Asked for after the shelf has rendered rather than before: a person should
  // see their items immediately with the file names Runtime already sent, and
  // gain the titles and covers a moment later. A document that never arrives
  // costs a cover, never a row.
  async function requestListingMetadata(listings) {
    const pending = listings.filter((listing) =>
      listing.metadataCid && itemKey(listing) && !listingMetadataAsked.has(itemKey(listing)));
    if (!pending.length) {
      return;
    }
    for (const listing of pending) {
      listingMetadataAsked.add(itemKey(listing));
    }
    const found = await Promise.all(pending.map(async (listing) => {
      try {
        const response = await fetch(`/ipfs/${encodeURIComponent(listing.metadataCid)}/metadata.json`, {
          headers: { accept: "application/json" },
        });
        if (!response.ok) {
          return null;
        }
        const document = await response.json();
        return { key: itemKey(listing), ...metadataFacts(document) };
      } catch {
        // A cover this Home cannot fetch is a cover it does not show.
        return null;
      }
    }));
    let learned = false;
    for (const entry of found) {
      if (entry && entry.key && (entry.title || entry.coverCid)) {
        listingMetadata.set(entry.key, entry);
        learned = true;
      }
    }
    if (learned) {
      render();
    }
  }

  // The two things this shelf takes from a published metadata document, each
  // bounded and screened. Everything else in that document is ignored here:
  // what an item costs and who may open it are Runtime's answers.
  function metadataFacts(document) {
    const source = document && typeof document === "object" ? document : {};
    const name = typeof source.name === "string" ? source.name.trim() : "";
    // `ipfs://<cid>` is what this Home writes for a cover it published. A
    // document naming anything else -- an http URL, a path, a data URI -- gets
    // no cover rather than a fetch this app was told to make.
    const image = typeof source.image === "string" ? source.image.trim() : "";
    const coverCid = image.startsWith("ipfs://") ? cidOrEmpty(image.slice("ipfs://".length)) : "";
    return {
      title: name.slice(0, MAX_LISTING_TITLE_CHARS),
      coverCid,
    };
  }

  // Two facts belong above the shelf, and only when they are true: Runtime
  // sent more items than it will list, and this app refused a row it could not
  // read. A refused row is the app's own limitation, so it says so plainly
  // rather than describing the item.
  function shelfNotice() {
    const notes = [];
    if (state.mediaTruncated) {
      notes.push(`Showing the first ${MAX_RUNTIME_CUSTODY_LISTINGS} items.`);
    }
    if (state.mediaRejected > 0) {
      notes.push(state.mediaRejected === 1
        ? "One item could not be read and is hidden."
        : `${state.mediaRejected} items could not be read and are hidden.`);
    }
    return notes.length ? `<p class="store-inline-note">${escapeHtml(notes.join(" "))}</p>` : "";
  }

  // Explore is the market: everything the index knows, with this Home's own
  // items among them rather than instead of them. My listings is this Home's
  // own, from its own records, and never asks the index anything.
  function exploreSourceItems() {
    if (state.mediaTab === "mine") {
      return state.mediaListings.filter((listing) => listing.accessState === "creator");
    }
    if (!state.catalog.length) {
      return state.mediaListings;
    }
    // An asset this Home holds a listing for is shown from that listing: it
    // knows the availability, the purchase in flight and the viewer its kind
    // names, none of which an index can answer.
    const held = new Set(state.mediaListings.map((listing) => listing.mintId));
    return state.mediaListings.concat(
      state.catalog.filter((item) => !item.mintId || !held.has(item.mintId)),
    );
  }

  function filteredMediaListings() {
    return exploreSourceItems().filter((listing) => {
      if (!state.search) {
        return true;
      }
      // What a person can see is what they can search for. The price is
      // matched as it is shown as well as in full, so typing what is on the
      // card finds the card.
      const money = formatMoney(listing);
      const haystack = [
        listing.displayName,
        listing.mimeType,
        listing.codecs || "",
        `quantity ${uint256Decimal(listing.quantity)}`,
        `price ${money.value} ${money.title}`,
        listing.payToken,
        listing.accessState,
      ].join(" ").toLowerCase();
      return haystack.includes(state.search);
    });
  }

  function renderSection(title, apps, { seeAllDestination } = {}) {
    const seeAll = seeAllDestination && apps.length
      ? `<button type="button" class="store-see-all" data-action="see-all" data-destination="${escapeAttr(seeAllDestination)}">See All</button>`
      : "";
    return `
      <section class="store-section">
        <div class="store-section-head">
          <h2 class="store-section-title">${escapeHtml(title)}</h2>
          ${seeAll}
        </div>
        <div class="store-row-grid">
          ${apps.map(renderAppRow).join("")}
        </div>
      </section>
    `;
  }

  function updateInstalledBadge() {
    const count = state.apps.filter((app) => app.installed).length;
    els.installedBadge.textContent = String(count);
    els.installedBadge.classList.toggle("hidden", count === 0);
  }

  function isFirstPartyPublisher(author) {
    const value = String(author || "").trim();
    return !value || /^elastos$/i.test(value) || value === "Unknown publisher";
  }

  function rowSubtitle(app) {
    if (!isFirstPartyPublisher(app.developer)) {
      return app.developer;
    }
    const description = String(app.description || "").trim();
    if (description) {
      const line = description.split(/[.!?]/)[0].trim();
      if (line) {
        return line;
      }
    }
    return roleLabel(app.role);
  }

  function detailPublisher(app) {
    if (isFirstPartyPublisher(app.developer)) {
      return "ElastOS";
    }
    return app.developer;
  }

  function renderAppRow(app) {
    return `
      <article class="store-row" data-action="detail" data-app="${escapeAttr(app.id)}" tabindex="0">
        ${appIconHtml(app, "store-row-icon")}
        <div class="store-row-text">
          <div class="store-row-title">${escapeHtml(app.name)}</div>
          <div class="store-row-sub">${escapeHtml(rowSubtitle(app))}</div>
        </div>
        ${actionButton(app)}
      </article>
    `;
  }

  function renderMediaRow(listing) {
    return `
      <article class="store-row store-row-static store-row-media" data-mint="${escapeAttr(listing.mintId)}">
        <span class="app-icon gradient-blue store-row-icon" aria-hidden="true">${icons.media}</span>
        <div class="store-row-text">
          <div class="store-row-title">${escapeHtml(listing.displayName)}</div>
          <div class="store-row-sub">${escapeHtml(mediaSubtitle(listing))}</div>
          <div class="store-row-facts">
            <span class="store-row-fact">Quantity ${escapeHtml(uint256Decimal(listing.quantity))}</span>
            <span class="store-row-fact">Price ${escapeHtml(uint256Decimal(listing.price))} base units</span>
            <span class="store-row-fact">Token ${escapeHtml(abbreviateAddress(listing.payToken))}</span>
            <span class="store-row-fact">${escapeHtml(availabilitySummary(listing.availability))}</span>
            <span class="store-row-fact">${escapeHtml(accessStateLabel(listing.accessState))}</span>
          </div>
          <div class="store-row-facts">
            ${buyStateNoteMarkup(listing)}
          </div>
        </div>
        ${mediaActionButton(listing)}
      </article>
    `;
  }

  function buyStateNoteMarkup(listing) {
    const buyState = pendingMediaBuys.get(listing.mintId);
    const note = buyState
      ? buyStateNote(buyState)
      : (listing.accessState === "available" && listing.purchaseInFlight
        ? "A purchase of this is already under way."
        : "");
    return note ? `<span class="store-row-fact store-row-buy-note">${escapeHtml(note)}</span>` : "";
  }

  function mediaSubtitle(listing) {
    return listing.codecs
      ? `${listing.mimeType} • ${listing.codecs}`
      : listing.mimeType;
  }

  function mediaActionButton(listing, size = "") {
    // A catalog item has no mint to buy or resume against: it reaches a
    // purchase through the market path, one offer sheet at a time.
    if (listing.catalogOnly) {
      return catalogActionButton(listing, size);
    }
    const mint = escapeAttr(listing.mintId);
    const buyState = listing.accessState === "available" ? pendingMediaBuys.get(listing.mintId) : null;
    // A purchase Runtime is still holding, from a visit this page does not
    // remember. Pressing Continue resumes that attempt rather than starting a
    // second one, and the terms it was started on are already fixed by the
    // record, so it does not ask for them again.
    if (!buyState && listing.accessState === "available" && listing.purchaseInFlight) {
      return `<button class="btn btn-primary ${size}" type="button" data-action="resume-buy" data-mint="${mint}">Continue</button>`;
    }
    if (buyState) {
      // Whose turn it is decides what the control does. When it is the
      // person's, the control takes them to the wallet holding the request;
      // when it is the network's, there is nothing to press.
      if (buyState.kind === "declined") {
        return `<button class="btn btn-primary ${size}" type="button" data-action="buy-media" data-mint="${mint}">Buy again</button>`;
      }
      if (buyState.awaitsPerson) {
        return `<button class="btn btn-primary ${size}" type="button" data-action="open-wallet" data-mint="${mint}">Approve in wallet</button>`;
      }
      return `<button class="btn btn-ghost ${size}" type="button" data-action="buy-media" data-mint="${mint}" disabled aria-busy="true">${escapeHtml(buyStateLabel(buyState))}</button>`;
    }
    // The one control that acts on the item itself. What surrounds it -- the
    // creator's link, the rebuild of a copy already owned -- is added by
    // `cardActions`, so this stays the answer to one question: what does
    // pressing the main button do for whoever is looking?
    // What the button does, said plainly. The design calls it "Get"; this
    // says "Buy", because it spends money and the word should say so.
    const action = listing.accessState === "available" ? "buy-media" : "open-media";
    const label = listing.accessState === "available" ? "Buy" : "Open";
    return `<button class="btn btn-primary ${size}" type="button" data-action="${escapeAttr(action)}" data-mint="${mint}">${label}</button>`;
  }

  // A catalog item's control: the chain's own key, never a mint, and never an
  // account. `purchased`/`creator` reuse the state a market purchase now
  // reports (Task 8); a foreign asset never gets a local mint to open through,
  // so its card says where it does open instead of offering a dead control.
  // A row whose readability this page has not learned yet neither guesses
  // "Open" nor stays silent -- it offers to find out first. A bought row also
  // offers Details: what the item is, read from its own listing, which is
  // all a person can see of an item that has no copy on this Home.
  function catalogActionButton(listing, size) {
    if (listing.accessState === "purchased" || listing.accessState === "creator") {
      const readability = marketAssetReadability.get(catalogLedgerKey(listing));
      const details = listing.accessState === "purchased" ? itemDetailsButton(listing, size) : "";
      if (readability === "foreign") {
        return `<span class="store-row-state">Opens on ela.city</span>${details}`;
      }
      if (readability === "verified" || readability === "unverified") {
        // Bought on the market, and not yet adopted onto this Home's shelf:
        // there is nothing here to open yet. Adoption runs again only when
        // something asks for the copy, so a bought row offers Download copy,
        // which reruns adoption and never pays (R45).
        if (listing.accessState === "creator") {
          return `<span class="store-row-state">You listed this</span>`;
        }
        return `${marketDownloadButton(listing, size)}${details}`;
      }
      return `<button class="btn btn-ghost ${size}" type="button" data-action="open-listing" data-ledger="${escapeAttr(listing.ledger)}" data-token="${escapeAttr(listing.tokenId)}">Open</button>${details}`;
    }
    const loading = catalogListingLoading.has(`${listing.ledger}|${listing.tokenId}`)
      ? ' aria-busy="true" aria-disabled="true"'
      : "";
    return `<button class="btn btn-primary ${size}" type="button" data-action="buy-listing" data-ledger="${escapeAttr(listing.ledger)}" data-token="${escapeAttr(listing.tokenId)}"${loading}>Buy</button>`;
  }

  // Details for a bought market item, named by the chain's own key. It reads
  // the item's listing and shows its properties in the item sheet.
  function itemDetailsButton(listing, size) {
    const busy = catalogListingLoading.has(`${listing.ledger}|${listing.tokenId}`)
      ? ' aria-busy="true" aria-disabled="true"'
      : "";
    return `<button class="btn btn-ghost ${size}" type="button" data-action="item-details" data-ledger="${escapeAttr(listing.ledger)}" data-token="${escapeAttr(listing.tokenId)}"${busy}>Details</button>`;
  }

  // Download copy for a bought market item on a catalog row, named by the
  // chain's own key. Only an item this page read a listing for can be named
  // whole, so a row without one shows its state alone.
  function marketDownloadButton(listing, size) {
    const key = catalogLedgerKey(listing);
    if (!marketAssetItems.has(key)) {
      return "";
    }
    const busy = pendingMarketDownloads.has(key) ? ' aria-busy="true" aria-disabled="true"' : "";
    return `<button class="btn btn-ghost ${size}" type="button" data-action="download-market-copy" data-ledger="${escapeAttr(listing.ledger)}" data-token="${escapeAttr(listing.tokenId)}"${busy}>Download copy</button>`;
  }

  // `<ledger>|<token_id>`, lowercase, the same identity for a catalog row and
  // for the `item` of a `ListingObject` this page fetched for it -- the only
  // two shapes this key is ever built from.
  function catalogLedgerKey(item) {
    const ledger = String((item && (item.ledger ?? "")) || "").toLowerCase();
    const tokenId = String((item && (item.tokenId ?? item.token_id ?? "")) || "").toLowerCase();
    return `${ledger}|${tokenId}`;
  }

  // What the row says while a purchase settles. Each stage is a different fact
  // about who is waiting for what, and none of them is a failure.
  function buyStateLabel(buyState) {
    const labels = {
      allowance_approval: "Allowing payment...",
      purchase_approval: "Approving...",
      chain_settlement: "Confirming...",
      access_evidence: "Almost yours...",
      declined: "Declined",
    };
    return labels[buyState.stage] || "Buying...";
  }

  function buyStateNote(buyState) {
    if (buyState.kind === "declined") {
      return "You declined this in your wallet.";
    }
    if (buyState.awaitsPerson) {
      return buyState.connectorId
        ? `Approve this purchase in ${buyState.connectorId}.`
        : "Approve this purchase in your wallet.";
    }
    if (buyState.stage === "chain_settlement") {
      return "The network is confirming your purchase.";
    }
    if (buyState.stage === "access_evidence") {
      return "Confirming the copy is yours.";
    }
    return "";
  }

  function availabilitySummary(availability) {
    return `${availability.observedReplicas}/${availability.requiredReplicas} replicas checked`;
  }

  function accessStateLabel(accessState) {
    if (accessState === "creator") {
      return "You listed this";
    }
    if (accessState === "purchased") {
      return "Owned";
    }
    return "Available";
  }

  function abbreviateAddress(value) {
    if (value.length <= 14) {
      return value;
    }
    return `${value.slice(0, 8)}...${value.slice(-6)}`;
  }

  function actionButton(app) {
    if (!app.launchable) {
      return "";
    }
    return `<button class="store-pill" type="button" data-action="open" data-app="${escapeAttr(app.id)}">Open</button>`;
  }

  function appIconHtml(app, extraClass = "") {
    const glyph = icons[app.icon] || icons.package;
    if (!app.iconRoute) {
      return `<span class="app-icon ${escapeAttr(app.gradient)} ${escapeAttr(extraClass)}">${glyph}</span>`;
    }
    return `<span class="app-icon app-icon-raster ${escapeAttr(extraClass)}"><img class="app-icon-img" src="${escapeAttr(app.iconRoute)}" alt="" draggable="false"><span class="app-icon-glyph" hidden>${glyph}</span></span>`;
  }

  function badgesHtml(app) {
    return app.badges.map((badge) => {
      const tip = badgeTooltip(badge);
      return `<span class="badge ${escapeAttr(badge)}"${tip ? ` title="${escapeAttr(tip)}"` : ""}>${escapeHtml(badgeLabel(badge))}</span>`;
    }).join("");
  }

  function badgeTooltip(badge) {
    const tips = {
      wallet: "Supports payments",
      ddrm: "Uses protected content",
      provider: "System service",
      installed: "Installed on this Home",
    };
    return tips[badge] || "";
  }

  function showAppDetail(appId) {
    const app = state.apps.find((candidate) => candidate.id === appId);
    if (!app) {
      return;
    }
    detailPreviousFocus = document.activeElement;
    const openButton = app.launchable
      ? `<button class="modal-btn primary" type="button" data-action="open" data-app="${escapeAttr(app.id)}">Open</button>`
      : "";
    els.detailContent.innerHTML = `
      <header class="modal-header">
        ${appIconHtml(app, "modal-icon-size")}
        <div class="modal-title-section">
          <div class="modal-title">${escapeHtml(app.name)}</div>
          <div class="modal-developer">${escapeHtml(detailPublisher(app))}</div>
          ${app.version ? `<div class="modal-version">Version ${escapeHtml(app.version)}</div>` : ""}
          <div class="modal-badges">${badgesHtml(app)}</div>
        </div>
        <button class="modal-close" type="button" data-action="close-detail" aria-label="Close">${icons.close}</button>
      </header>
      <div class="modal-body">
        <section class="modal-section">
          <div class="modal-section-title">About</div>
          <div class="modal-description">${escapeHtml(app.description)}</div>
        </section>
        <section class="modal-section">
          <div class="modal-section-title">Status</div>
          <ul class="permissions-list">
            ${statusItems(app).map((item) => `<li><span class="permission-icon">${icons.check}</span>${escapeHtml(item)}</li>`).join("")}
          </ul>
        </section>
        ${relationshipSection(app)}
        <section class="modal-section">
          <div class="modal-section-title">Available actions</div>
          <ul class="permissions-list">
            ${availableActionItems(app).map((item) => `<li><span class="permission-icon">${icons.check}</span>${escapeHtml(item)}</li>`).join("")}
          </ul>
        </section>
        ${technicalDetails(app)}
      </div>
      <footer class="modal-footer">
        <div class="modal-footer-price"><span class="trust-chip">${escapeHtml(packageLabel(app))}</span></div>
        <div class="modal-footer-actions">
          <button class="modal-btn secondary" type="button" data-action="close-detail">Close</button>
          ${openButton}
        </div>
      </footer>
    `;
    bindAppActions(els.detailContent);
    els.detailModal.classList.add("active");
    // Opens on Close, never on Open: a key still held from the row must not
    // launch the app.
    els.detailContent.querySelector("[data-action='close-detail']")?.focus();
  }

  // What a purchase costs, who is selling it, and what happens next. The
  // wallet asks for a signature, not for a decision, and it arrives after the
  // availability re-check -- so a person who has not seen the terms by then
  // has already waited minutes to see them.
  function showBuyConfirmation(mintId) {
    const listing = state.mediaListings.find((entry) => entry.mintId === mintId);
    if (!listing || listing.accessState !== "available" || pendingMediaBuys.has(mintId)) {
      return;
    }
    detailPreviousFocus = document.activeElement;
    els.detailContent.innerHTML = `
      <header class="modal-header">
        <div class="modal-title-section">
          <div class="modal-title">${escapeHtml(listing.displayName)}</div>
          <div class="modal-developer">${escapeHtml(mediaSubtitle(listing))}</div>
        </div>
        <button class="modal-close" type="button" data-action="close-detail" aria-label="Close">${icons.close}</button>
      </header>
      <div class="modal-body">
        <section class="modal-section">
          <div class="modal-section-title">What you pay</div>
          <ul class="permissions-list">
            <li><span class="permission-icon">${icons.check}</span>${escapeHtml(uint256Decimal(listing.price))} base units of ${escapeHtml(abbreviateAddress(listing.payToken))}</li>
            <li><span class="permission-icon">${icons.check}</span>One copy, of ${escapeHtml(uint256Decimal(listing.quantity))} still listed</li>
            <li><span class="permission-icon">${icons.check}</span>Sold by ${escapeHtml(abbreviateAddress(listing.sellerAddress))}</li>
            <li><span class="permission-icon">${icons.check}</span>${escapeHtml(availabilitySummary(listing.availability))}</li>
          </ul>
        </section>
        <section class="modal-section">
          <div class="modal-section-title">What happens next</div>
          <p class="store-inline-note">Your wallet asks you to approve the payment. The item arrives in your Library once the network confirms it.</p>
        </section>
      </div>
      <footer class="modal-footer">
        <div class="modal-footer-price"></div>
        <div class="modal-footer-actions">
          <button class="modal-btn secondary" type="button" data-action="close-detail">Cancel</button>
          <button class="modal-btn primary" type="button" data-action="confirm-buy" data-mint="${escapeAttr(listing.mintId)}">Buy</button>
        </div>
      </footer>
    `;
    bindAppActions(els.detailContent);
    els.detailModal.classList.add("active");
    // Opens on Cancel, never on Buy: an Enter (or a held key's repeat) from
    // the card's own Buy must not confirm a spend.
    els.detailContent.querySelector(".modal-footer [data-action='close-detail']")?.focus();
  }

  // The link a listing lives at, for the person who listed it to pass on.
  // Runtime has answered with it since listings existed and no surface has ever
  // shown it, so the only way anyone could reach an item on another Home was to
  // already know its address.
  //
  // The link is handed over selected rather than copied for them. Reading a
  // capsule's clipboard authority is a boundary this app does not cross, and
  // its own gate holds that line, so the honest control is the text itself,
  // ready for the copy key.
  function showShareListing(mintId) {
    const listing = state.mediaListings.find((entry) => entry.mintId === mintId);
    if (!listing || listing.accessState !== "creator") {
      return;
    }
    detailPreviousFocus = document.activeElement;
    els.detailContent.innerHTML = `
      <header class="modal-header">
        <div class="modal-title-section">
          <div class="modal-title">Share ${escapeHtml(listing.displayName)}</div>
          <div class="modal-developer">Anyone with this link can add your listing and buy a copy.</div>
        </div>
        <button class="modal-close" type="button" data-action="close-detail" aria-label="Close">${icons.close}</button>
      </header>
      <div class="modal-body">
        <section class="modal-section">
          <div class="modal-section-title">Listing link</div>
          <input id="share-listing-uri" class="store-import-input" type="text" readonly value="${escapeAttr(listing.listingUri)}">
          <p class="store-inline-note">The link is selected: press the copy key, then send it. They paste it into Add a listing on their own Home.</p>
        </section>
      </div>
      <footer class="modal-footer">
        <div class="modal-footer-price"></div>
        <div class="modal-footer-actions">
          <button class="modal-btn primary" type="button" data-action="close-detail">Close</button>
        </div>
      </footer>
    `;
    bindAppActions(els.detailContent);
    els.detailModal.classList.add("active");
    const field = els.detailContent.querySelector("#share-listing-uri");
    field?.focus();
    field?.select();
  }

  // --- Offer sheet: buy a market listing, from a link or from Explore ------
  //
  // One sheet, reusing `#detail-modal` / `.modal-overlay` exactly as every
  // other modal here does, for a `ListingObject` reached from either
  // starting point (task-9-brief, task-10-brief): a pasted link, or an
  // Explore card's own `(ledger, token_id)`. Readability is advisory
  // (shared-context.md D8) -- shown, never a gate.

  // `verified` needs no note, and `unknown` (R46) has nothing true to say
  // about where the item opens, so both say nothing; Buy stays enabled.
  function offerSheetReadabilityCopy(readability) {
    if (readability === "unverified") {
      return "Not yet verified to open here";
    }
    if (readability === "foreign") {
      return "Opens on ela.city";
    }
    return "";
  }

  function offerSheetIcon(listing) {
    if (listing.asset.coverCid) {
      const route = `/ipfs/${encodeURIComponent(listing.asset.coverCid)}`;
      return `<span class="app-icon app-icon-raster modal-icon-size"><img class="app-icon-img" src="${escapeAttr(route)}" alt="" draggable="false"><span class="app-icon-glyph" hidden>${icons.media}</span></span>`;
    }
    return `<span class="app-icon gradient-blue modal-icon-size" aria-hidden="true">${icons.media}</span>`;
  }

  // What the pasted link or the Explore item's `/listing` read answered.
  // `legacy: true` tells the caller to fall back to today's
  // `import_runtime_custody` path unchanged; otherwise a parsed listing, or
  // this throws.
  async function readMarketListing(start) {
    const answer = await requestMarketListing(start);
    if (answer && typeof answer === "object" && answer.legacy_listing === true) {
      return { legacy: true };
    }
    const parsed = parseMarketListing(answer);
    if (!parsed.ok) {
      throw new Error("That listing could not be read.");
    }
    return { legacy: false, listing: parsed.listing };
  }

  async function requestMarketListing(start) {
    const response = await fetch("/api/apps/marketplace/listing", {
      method: "POST",
      headers: {
        "content-type": "application/json",
        accept: "application/json",
        "x-elastos-home-token": homeToken,
      },
      body: JSON.stringify({ start }),
    });
    const data = await response.json().catch(() => ({}));
    if (!response.ok) {
      const code = data && typeof data === "object" ? String(data.code || "") : "";
      throw new Error(marketListingErrorMessage(code));
    }
    return data;
  }

  function marketListingErrorMessage(code) {
    if (code === "asset_mismatch") {
      return "That link doesn’t match this item.";
    }
    if (code === "unbound") {
      return "This isn’t an item this market can sell.";
    }
    if (code === "unavailable") {
      return MARKET_UNREACHABLE_COPY;
    }
    return "That listing could not be loaded.";
  }

  // What this page actually learned the last time it read a `ListingObject`
  // for this item -- never a guess from the catalog index, and never a
  // purchase gate (D8). Feeds the Explore card's Open label once a market
  // purchase completes. `unknown` (R46) means Runtime could not learn it this
  // time, so the row goes back to "not learned yet" rather than keeping an
  // older answer or reading `unknown` as one.
  function noteMarketListingReadability(listing) {
    const key = catalogLedgerKey(listing.item);
    if (listing.asset.readability === "unknown") {
      marketAssetReadability.delete(key);
    } else {
      marketAssetReadability.set(key, listing.asset.readability);
    }
    marketAssetItems.set(key, listing.item);
  }

  function openOfferSheet(listing, { properties = false, returnFocus = null } = {}) {
    // A fresh listing read re-asks Runtime rather than trusting an earlier
    // `attempt_in_progress`: if that purchase is still live, the next Buy is
    // answered the same way again and raises nothing.
    recordedOfferAttempts.delete(marketItemKey(listing.item));
    detailPreviousFocus = returnFocus || document.activeElement;
    currentOfferListing = listing;
    currentOfferSheetProperties = properties;
    renderOfferSheetContent(listing, {});
    els.detailModal.classList.add("active");
    // The sheet opens on its own Close, never on a Buy: the first key a
    // person presses must not be the one that spends money.
    els.detailContent.querySelector("[data-action='close-detail']")?.focus();
  }

  // The poll state for this item's offer, if a buy is running -- read fresh
  // at render time (like `mediaActionButton` reads `pendingMediaBuys`)
  // rather than threaded in as a render option. This is what lets the sheet
  // show the right row as busy whether this call is a poll tick or a fresh
  // reopen of a sheet whose buy kept running in the background.
  function offerBuyStateFor(item, seller) {
    const pending = pendingOfferBuys.get(marketItemKey(item));
    return pending && pending.seller === seller ? pending : null;
  }

  // Which purchase of this item is already under way, if any: one this page
  // is driving, or one Runtime answered `attempt_in_progress` for. Only one
  // purchase of an item runs at a time, so every other seller's Buy waits.
  // `{seller, recorded}`: `seller` is `null` when Runtime named none, and
  // `recorded` is the offer Runtime recorded (what Continue sends).
  function offerItemPurchaseInProgress(item) {
    const key = marketItemKey(item);
    const pending = pendingOfferBuys.get(key);
    if (pending) {
      return { seller: pending.seller, recorded: null };
    }
    const recorded = recordedOfferAttempts.get(key);
    if (recorded) {
      return { seller: recorded.offer ? recorded.offer.seller : null, recorded: recorded.offer };
    }
    return null;
  }

  // True only while the sheet on screen is showing this exact item. A
  // background buy's poll tick must never repaint the modal once the person
  // has closed it or moved on to another item's sheet -- gated here instead
  // of trusting whichever `listing` object a loop closed over.
  function offerSheetShowingItem(item) {
    return Boolean(currentOfferListing)
      && els.detailModal.classList.contains("active")
      && marketItemKey(currentOfferListing.item) === marketItemKey(item);
  }

  // Repaints the sheet only if it is still open on this item, and always
  // from `currentOfferListing` -- the listing actually on screen -- rather
  // than a `listing` reference a background loop may be holding onto from
  // before the sheet was closed and reopened.
  function renderOfferSheetIfShowing(item, options = {}) {
    if (offerSheetShowingItem(item)) {
      renderOfferSheetContent(currentOfferListing, options);
    }
  }

  const OFFER_IN_PROGRESS_COPY = "Another purchase of this item is in progress";
  // Runtime could not reach the market or the chain: nothing was bought and
  // pressing again is the whole remedy (R48). Said the same way for a
  // listing read and for a Buy.
  const MARKET_UNREACHABLE_COPY = "The market couldn’t be reached. Try again.";

  // What the sheet says instead of offering to buy something this Home
  // already holds or listed: `access_state` is Runtime's answer, and a Buy
  // on either would only be refused.
  function offerSheetOwnershipCopy(accessState) {
    if (accessState === "purchased") {
      return "You own this";
    }
    if (accessState === "creator") {
      return "You listed this";
    }
    return "";
  }

  function renderOfferSheetContent(listing, options = {}) {
    const focusMark = offerSheetFocusMark();
    currentOfferListing = listing;
    const changedFor = options.termsChangedFor || "";
    const changedOffer = changedFor ? listing.offers.find((entry) => entry.seller === changedFor) : null;
    // An item this Home bought shows what it is, not who else sells it:
    // offers are for a person who does not own it yet. Details asks for the
    // same view on any card.
    const properties = currentOfferSheetProperties || listing.accessState === "purchased";
    // The properties say where the item opens, so the header does not.
    const readabilityCopy = properties ? "" : offerSheetReadabilityCopy(listing.asset.readability);
    const ownership = offerSheetOwnershipCopy(listing.accessState);
    const inProgress = ownership ? null : offerItemPurchaseInProgress(listing.item);
    // A purchase Runtime recorded keeps its row even when its seller no
    // longer lists the item (R44): Continue on its recorded terms is the
    // only press that moves it on.
    const offers = offersWithRecordedAttempt(listing.offers, inProgress?.recorded);
    const download = listing.accessState === "purchased" && listing.asset.readability !== "foreign";
    const downloading = download && pendingMarketDownloads.has(catalogLedgerKey(listing.item));
    const notes = [];
    if (ownership) {
      notes.push(`${ownership}.`);
    }
    if (inProgress && !offers.some((entry) => entry.seller === inProgress.seller)) {
      notes.push(`${OFFER_IN_PROGRESS_COPY}.`);
    }
    if (changedOffer) {
      notes.push(changedOffer._gone
        ? "This offer is no longer available."
        : "The seller changed the terms. Review and confirm.");
    }
    els.detailContent.innerHTML = `
      <header class="modal-header">
        ${offerSheetIcon(listing)}
        <div class="modal-title-section">
          <div class="modal-title">${escapeHtml(listing.asset.title || "Protected item")}</div>
          ${readabilityCopy ? `<div class="modal-developer">${escapeHtml(readabilityCopy)}</div>` : ""}
        </div>
        <button class="modal-close" type="button" data-action="close-detail" aria-label="Close">${icons.close}</button>
      </header>
      <div class="modal-body">
        <div class="offer-sheet-status" role="status">${notes
          .map((note) => `<p class="store-inline-note offer-terms-note">${escapeHtml(note)}</p>`)
          .join("")}</div>
        ${properties ? itemPropertiesSection(listing) : `<section class="modal-section">
          <div class="modal-section-title">Offers</div>
          <ul class="offer-list">
            ${offers.length
              ? offers.map((offer) => renderOfferRow(offer, {
                busy: offerBuyStateFor(listing.item, offer.seller),
                highlighted: offer.seller === changedFor && !offer._gone,
                owned: Boolean(ownership),
                inProgress,
              })).join("")
              : `<li class="offer-row offer-row-empty">No live offers.</li>`}
          </ul>
          ${listing.offersTruncated
            ? `<p class="store-inline-note offer-truncated-note">Showing the first ${MAX_MARKET_OFFERS} sellers.</p>`
            : ""}
        </section>`}
      </div>
      <footer class="modal-footer">
        <div class="modal-footer-price"></div>
        <div class="modal-footer-actions">
          ${download
            ? `<button class="modal-btn primary" type="button" data-action="download-market-copy"${downloading ? ' aria-busy="true" aria-disabled="true"' : ""}>Download copy</button>`
            : ""}
          <button class="modal-btn secondary" type="button" data-action="close-detail">Close</button>
        </div>
      </footer>
    `;
    bindAppActions(els.detailContent);
    restoreOfferSheetFocus(focusMark);
  }

  // Where a bought item opens, in plain words. `unknown` has nothing true to
  // say, so it says nothing.
  function itemPropertiesOpensCopy(readability) {
    if (readability === "foreign") {
      return "Opens on ela.city";
    }
    if (readability === "verified" || readability === "unverified") {
      return "Opens here";
    }
    return "";
  }

  // What an item is, from its own `ListingObject`, plus who listed it when
  // this page's market row names them. Long values are shortened and keep
  // their full form in the title. Every value is escaped here, once.
  function itemPropertiesSection(listing) {
    const row = state.catalog.find((entry) => catalogLedgerKey(entry) === catalogLedgerKey(listing.item));
    const rows = [
      ["Title", listing.asset.title || (row && row.displayName) || "Protected item"],
      ["Description", listing.asset.description],
      ["Listed by", row && row.sellerAddress, true],
      ["Network", listing.item.network],
      ["Ledger", listing.item.ledger, true],
      ["Token ID", listing.item.tokenId, true],
      ["Operative", listing.item.operative, true],
      ["KID", listing.item.kid, true],
      ["Media type", listing.asset.mimeType || listing.asset.category],
      ["Opens", itemPropertiesOpensCopy(listing.asset.readability)],
    ].filter(([, value]) => typeof value === "string" && value.length > 0);
    return `
        <section class="modal-section item-properties">
          <div class="modal-section-title">Details</div>
          <dl class="item-properties-list">
            ${rows.map(([label, value, shorten]) => `
            <div class="item-property">
              <dt>${escapeHtml(label)}</dt>
              <dd${shorten ? ` class="mono" title="${escapeAttr(value)}"` : ""}>${escapeHtml(shorten ? abbreviateAddress(value) : value)}</dd>
            </div>`).join("")}
          </dl>
        </section>`;
  }

  // A repaint replaces every control in the sheet, and a poll repaints it
  // every few seconds. Which control had focus is remembered by what it is
  // -- the seller's row, or the action it takes -- so focus comes back to the
  // same control rather than falling to the page behind the sheet.
  function offerSheetFocusMark() {
    const active = document.activeElement;
    if (!active || !els.detailModal.classList.contains("active") || !els.detailContent.contains(active)) {
      return null;
    }
    return {
      seller: active.closest(".offer-row")?.dataset.seller || "",
      action: active.dataset.action || "",
    };
  }

  function restoreOfferSheetFocus(mark) {
    if (!mark) {
      return;
    }
    const byData = (selector, name, value) => [...els.detailContent.querySelectorAll(selector)]
      .find((node) => node.dataset[name] === value);
    const row = mark.seller ? byData(".offer-row", "seller", mark.seller) : null;
    const target = row?.querySelector("button:not([disabled])")
      || (mark.action ? byData("[data-action]", "action", mark.action) : null)
      || els.detailContent.querySelector("[data-action='close-detail']");
    target?.focus();
  }

  // One offer, as a row: who is selling, at what price and quantity, and
  // what pressing the row does next. `offer` is always the listing's own
  // live entry for this seller -- `applyTermsChanged` keeps it that way, so
  // this never renders (or lets Buy resubmit) terms Runtime already
  // rejected. An offer Runtime marked `_gone` (its `current` was `null`)
  // keeps its row, without a Buy control and without the terms it no longer
  // has, so a person can see what happened rather than watch it vanish.
  //
  // A control that cannot act right now is `aria-disabled`, not `disabled`:
  // it keeps focus through a repaint, and it carries no `data-action`, so
  // pressing it does nothing.
  function renderOfferRow(offer, { busy, highlighted, owned, inProgress } = {}) {
    const money = formatMoney(offer);
    const sellerLabel = abbreviateAddress(offer.seller);
    const seller = escapeAttr(offer.seller);
    let meta = `${escapeHtml(uint256Decimal(offer.quantity))} available${highlighted ? " · new terms" : ""}`;
    let action = "";
    let rowClass = highlighted ? " offer-row-changed" : "";
    if (offer._gone) {
      meta = "This offer is no longer available";
      rowClass = " offer-row-gone";
    } else if (owned) {
      action = "";
    } else if (busy) {
      action = busy.awaitsPerson
        ? `<button class="modal-btn primary" type="button" data-action="open-wallet" data-seller="${seller}">Approve in wallet</button>`
        : `<button class="modal-btn secondary" type="button" aria-disabled="true" aria-busy="true" data-seller="${seller}">${escapeHtml(buyStateLabel(busy))}</button>`;
    } else if (inProgress && inProgress.seller === offer.seller) {
      // The purchase Runtime recorded for this seller: Continue sends exactly
      // the recorded terms, which is what lets Runtime resume it.
      const recordedMoney = inProgress.recorded ? formatMoney(inProgress.recorded) : money;
      meta = `Purchase in progress · ${escapeHtml(recordedMoney.value)}${recordedMoney.unit ? ` ${escapeHtml(recordedMoney.unit)}` : ""}`;
      rowClass = " offer-row-in-progress";
      action = `<button class="modal-btn primary" type="button" data-action="buy-offer" data-seller="${seller}" aria-label="${escapeAttr(`Continue the purchase from ${offer.seller}`)}">Continue</button>`;
    } else if (inProgress) {
      meta = OFFER_IN_PROGRESS_COPY;
      action = `<button class="modal-btn secondary" type="button" aria-disabled="true" data-seller="${seller}" aria-label="${escapeAttr(`Buy from ${offer.seller}: ${OFFER_IN_PROGRESS_COPY}`)}">Buy</button>`;
    } else {
      action = `<button class="modal-btn primary" type="button" data-action="buy-offer" data-seller="${seller}" aria-label="${escapeAttr(`Buy from ${offer.seller} for ${money.title}`)}" title="${escapeAttr(`Buy from ${offer.seller}`)}">Buy</button>`;
    }
    const price = offer._gone
      ? ""
      : `${escapeHtml(money.value)}${money.unit ? ` <small>${escapeHtml(money.unit)}</small>` : ""}`;
    return `
      <li class="offer-row${rowClass}" data-seller="${seller}">
        <div class="offer-row-text">
          <div class="offer-row-seller" title="${seller}">${escapeHtml(sellerLabel)}</div>
          <div class="offer-row-meta">${meta}</div>
        </div>
        <div class="offer-row-price" title="${escapeAttr(offer._gone ? "" : money.title)}">${price}</div>
        ${action ? `<div class="offer-row-action">${action}</div>` : ""}
      </li>
    `;
  }

  // Replaces `seller`'s entry in `listing.offers` with the fresh terms
  // Runtime just re-read (or drops it, marked `_gone`, when `current` is
  // `null`), so the listing itself -- the one thing every render and every
  // future Buy reads from -- never again shows or resubmits terms Runtime
  // already refused.
  function applyTermsChanged(listing, seller, current) {
    const index = listing.offers.findIndex((entry) => entry.seller === seller);
    if (index === -1) {
      return;
    }
    listing.offers[index] = current === null ? { ...listing.offers[index], _gone: true } : current;
  }

  // `agreed.quantity` is always `"0x1"` in this slice (shared-context.md
  // §5.2) -- never the seller's own available quantity, which is what the
  // offer's `quantity` field actually names.
  function buyOfferRequestBody(listing, offer) {
    return {
      item: marketItemClaim(listing.item),
      asset_uri: listing.asset.uri,
      seller: offer.seller,
      agreed: { price: offer.price, pay_token: offer.payToken, quantity: "0x1" },
    };
  }

  // What a finished market purchase says depends on how far adoption got:
  // only an adopted item is in this Home's Library yet. Adoption runs again
  // only when the copy is asked for, so a pending one names Download copy,
  // the control that finishes it without paying again (R45).
  function offerCompletionMessage(adoption) {
    if (adoption === "foreign") {
      return "Bought. It opens on ela.city.";
    }
    if (adoption === "pending") {
      return "Bought. Use Download copy to add it to your Library here.";
    }
    return "Bought. The item is in your Library.";
  }

  function offerBareOutcomeMessage(kind) {
    if (kind === "own_offer") {
      return "You can’t buy your own offer.";
    }
    if (kind === "already_owned") {
      return "You already own this.";
    }
    return "This listing no longer matches the item.";
  }

  // Mirrors `buyMedia`: the same poll budget, the same stages, resumed by
  // pressing Buy again rather than by this loop retrying on its own. Unlike
  // `buyMedia`, progress is shown in the sheet itself (one row per offer,
  // Task 9), not on a card, and `terms_changed` re-renders the sheet with
  // the fresh terms rather than ever retrying with the old ones. Every
  // repaint goes through `renderOfferSheetIfShowing`, so a poll tick for one
  // item can never overwrite the sheet once it is showing something else.
  async function buyOffer(listing, offer) {
    const key = marketItemKey(listing.item);
    // One purchase of an item at a time: while one runs, or while Runtime
    // holds one recorded for another seller, only that seller's row acts.
    const inProgress = offerItemPurchaseInProgress(listing.item);
    if (inProgress && (inProgress.recorded === null || inProgress.seller !== offer.seller)) {
      return;
    }
    const body = buyOfferRequestBody(listing, offer);
    pendingOfferBuys.set(key, { seller: offer.seller, kind: "wait", stage: "", awaitsPerson: false, connectorId: "" });
    renderOfferSheetIfShowing(listing.item);
    const deadline = Date.now() + BUY_POLL_BUDGET_MS;
    for (;;) {
      let outcome;
      try {
        const answer = await postObjectProvider("buy_offer", body);
        outcome = parseBuyOfferAnswer(answer);
      } catch (error) {
        outcome = parseBuyOfferAnswer(error.answer);
      }
      // A running purchase keeps its recorded terms: a Continue row added
      // for a seller that no longer lists the item must stay on the sheet
      // while that purchase settles, or its progress would have no row.
      if (outcome.kind !== "attempt_in_progress" && outcome.kind !== "wait") {
        recordedOfferAttempts.delete(key);
      }
      if (outcome.kind === "attempt_in_progress") {
        // Another purchase of this item is already under way on terms
        // Runtime recorded earlier. Nothing was started for this press; the
        // sheet now shows which seller that purchase is with, and holds
        // every other seller's Buy until it is done.
        pendingOfferBuys.delete(key);
        recordedOfferAttempts.set(key, { offer: outcome.current });
        renderOfferSheetIfShowing(listing.item);
        // The in-sheet note is rebuilt on every paint; the toast is the
        // persistent status region, so this is what gets announced.
        showToast(`${OFFER_IN_PROGRESS_COPY}.`, false);
        return;
      }
      if (outcome.kind === "complete") {
        pendingOfferBuys.delete(key);
        // What this listing said is now what the item's catalog row reads,
        // so a pending adoption's row can offer Download copy at once.
        noteMarketListingReadability(listing);
        if (outcome.adoption === "foreign") {
          marketAssetReadability.set(catalogLedgerKey(listing.item), "foreign");
        }
        showToast(offerCompletionMessage(outcome.adoption), false);
        if (offerSheetShowingItem(listing.item)) {
          closeDetail();
        }
        await Promise.all([loadMediaData(), state.catalogLoaded ? loadCatalogItems() : Promise.resolve()]);
        render();
        return;
      }
      if (outcome.kind === "terms_changed") {
        pendingOfferBuys.delete(key);
        applyTermsChanged(listing, offer.seller, outcome.current);
        // The sheet may have been closed and reopened while this answer was
        // on its way, onto a fresh listing object: the terms go there too,
        // or the sheet would say "new terms" beside the old price.
        if (offerSheetShowingItem(listing.item) && currentOfferListing !== listing) {
          applyTermsChanged(currentOfferListing, offer.seller, outcome.current);
        }
        renderOfferSheetIfShowing(listing.item, { termsChangedFor: offer.seller });
        showToast(
          outcome.current === null
            ? "This offer is no longer available."
            : "The seller changed the terms. Review and confirm.",
          false,
        );
        return;
      }
      if (outcome.kind === "own_offer" || outcome.kind === "already_owned" || outcome.kind === "asset_mismatch") {
        pendingOfferBuys.delete(key);
        showToast(offerBareOutcomeMessage(outcome.kind), true);
        renderOfferSheetIfShowing(listing.item);
        return;
      }
      if (outcome.kind === "unavailable") {
        // Runtime could not reach the market or the chain (R48). Nothing
        // was bought; the same Buy tries again.
        pendingOfferBuys.delete(key);
        showToast(MARKET_UNREACHABLE_COPY, true);
        renderOfferSheetIfShowing(listing.item);
        return;
      }
      if (outcome.kind === "wait") {
        if (Date.now() > deadline) {
          pendingOfferBuys.delete(key);
          showToast("This purchase is still settling. Press Buy to pick it up again.", false);
          renderOfferSheetIfShowing(listing.item);
          return;
        }
        pendingOfferBuys.set(key, { seller: offer.seller, ...outcome });
        renderOfferSheetIfShowing(listing.item);
        await new Promise((resolve) => { window.setTimeout(resolve, BUY_POLL_MS); });
        continue;
      }
      // failed -- a real failed purchase, including a declined approval,
      // which is the person's own decision and nothing this loop retries on
      // their behalf.
      pendingOfferBuys.delete(key);
      showToast(
        outcome.stage === "declined" ? "You declined this in your wallet." : "The purchase did not finish.",
        true,
      );
      renderOfferSheetIfShowing(listing.item);
      return;
    }
  }

  // Explore's own way in: the chain's own key, never an account
  // (shared-context.md D14), reaching the same sheet a pasted link opens.
  async function buyCatalogItem(ledger, tokenId) {
    const key = `${ledger}|${tokenId}`;
    if (catalogListingLoading.has(key)) {
      return;
    }
    catalogListingLoading.add(key);
    markCatalogListingBusy(ledger, tokenId, true);
    try {
      const result = await readMarketListing({ item: { ledger, token_id: tokenId } });
      if (!result.legacy) {
        noteMarketListingReadability(result.listing);
      }
      // The person moved on while the listing was read -- another sheet or
      // detail is open now. This late answer must not replace it.
      if (els.detailModal.classList.contains("active")) {
        return;
      }
      if (result.legacy) {
        showToast("That item could not be loaded.", true);
        return;
      }
      openOfferSheet(result.listing);
    } catch (error) {
      showToast(publicError(error.message, "That item could not be loaded."), true);
    } finally {
      catalogListingLoading.delete(key);
      markCatalogListingBusy(ledger, tokenId, false);
    }
  }

  // An Explore Buy says it is working while its listing is read. The flag is
  // set on whatever card controls are on screen now; `catalogActionButton`
  // reads `catalogListingLoading` so a repaint in between keeps it.
  function markCatalogListingBusy(ledger, tokenId, busy) {
    for (const node of document.querySelectorAll('[data-action="buy-listing"]')) {
      if (node.dataset.ledger !== ledger || node.dataset.token !== tokenId) {
        continue;
      }
      if (busy) {
        node.setAttribute("aria-busy", "true");
        node.setAttribute("aria-disabled", "true");
      } else {
        node.removeAttribute("aria-busy");
        node.removeAttribute("aria-disabled");
      }
    }
  }

  // Details on a bought Explore card: the item's own `ListingObject`, read by
  // the chain's own key, shown as properties in the item sheet. What the read
  // learns about readability also updates the card behind the sheet.
  async function showCatalogDetails(ledger, tokenId) {
    const key = `${ledger}|${tokenId}`;
    if (catalogListingLoading.has(key)) {
      return;
    }
    catalogListingLoading.add(key);
    try {
      const result = await readMarketListing({ item: { ledger, token_id: tokenId } });
      if (result.legacy) {
        showToast("That item could not be loaded.", true);
        return;
      }
      noteMarketListingReadability(result.listing);
      // The card now shows what the read learned. The repaint replaces the
      // pressed control, so Close returns focus to its successor.
      renderSections();
      // The person moved on while the listing was read -- another sheet or
      // detail is open now. This late answer must not replace it.
      if (els.detailModal.classList.contains("active")) {
        return;
      }
      const successor = [...document.querySelectorAll('[data-action="item-details"]')]
        .find((node) => node.dataset.ledger === ledger && node.dataset.token === tokenId);
      openOfferSheet(result.listing, { properties: true, returnFocus: successor || null });
    } catch (error) {
      showToast(publicError(error.message, "That item could not be loaded."), true);
    } finally {
      catalogListingLoading.delete(key);
    }
  }

  // Task 10 fix round 1, minor: a `purchased`/`creator` catalog row whose
  // readability this page has not yet learned neither guesses "Open" nor
  // stays silent -- it reads the item's own `ListingObject` first, then
  // shows the label that answer actually earns.
  async function openCatalogPurchase(ledger, tokenId) {
    const key = `${ledger}|${tokenId}`;
    if (catalogListingLoading.has(key)) {
      return;
    }
    catalogListingLoading.add(key);
    try {
      const result = await readMarketListing({ item: { ledger, token_id: tokenId } });
      if (result.legacy) {
        return;
      }
      noteMarketListingReadability(result.listing);
      if (result.listing.asset.readability !== "foreign") {
        // Verified, unverified or unknown: this Home's own open path takes it from
        // here once adoption has written the mint (D10) -- reloading is
        // what lets that merge happen; nothing here invents a way to open it.
        await Promise.all([loadMediaData(), loadCatalogItems()]);
      }
      render();
    } catch (error) {
      showToast(publicError(error.message, "That item could not be loaded."), true);
    } finally {
      catalogListingLoading.delete(key);
    }
  }

  function closeDetail() {
    els.detailModal.classList.remove("active");
    currentOfferListing = null;
    currentOfferSheetProperties = false;
    const restore = detailPreviousFocus;
    detailPreviousFocus = null;
    if (restore && typeof restore.focus === "function" && document.contains(restore)) {
      restore.focus();
    }
  }

  function trapDetailFocus(event) {
    const focusables = [...els.detailContent.querySelectorAll(
      'button:not([disabled]), [href], input, select, textarea, [tabindex]:not([tabindex="-1"])',
    )].filter((node) => !node.closest("[hidden]") && node.offsetParent !== null);
    if (focusables.length < 2) {
      event.preventDefault();
      focusables[0]?.focus();
      return;
    }
    const index = focusables.indexOf(document.activeElement);
    event.preventDefault();
    if (event.shiftKey) {
      focusables[index <= 0 ? focusables.length - 1 : index - 1].focus();
      return;
    }
    focusables[index >= focusables.length - 1 ? 0 : index + 1].focus();
  }

  function statusItems(app) {
    const items = [
      `Trust: ${trustLabel(app.trustState)}`,
      `Status: ${app.installed ? "Installed on this Home" : "Not installed on this Home"}`,
      `Launch: ${app.launchable ? "Open from Home available" : "Open from Home unavailable"}`,
    ];
    if (app.paymentState && app.paymentState !== "none") {
      items.push("Supports payments");
    }
    if (app.drmState && app.drmState !== "none") {
      items.push("Uses protected content");
    }
    return items;
  }

  function availableActionItems(app) {
    if (app.availableActions.length) {
      return app.availableActions.map((action) => action);
    }
    return ["No executable actions declared"];
  }

  function relationshipSection(app) {
    const items = [];
    if (app.viewerTitle) items.push(`Opens with ${app.viewerTitle}`);
    if (app.acceptedContent.length) items.push(`Accepts ${app.acceptedContent.join(", ")}`);
    if (app.dependencies.length) items.push(`Needs ${app.dependencies.join(", ")}`);
    if (!items.length) {
      return "";
    }
    return `
      <section class="modal-section">
        <div class="modal-section-title">Works with</div>
        <ul class="permissions-list">
          ${items.map((item) => `<li><span class="permission-icon">${icons.check}</span>${escapeHtml(item)}</li>`).join("")}
        </ul>
      </section>
    `;
  }

  function technicalDetails(app) {
    return `
      <details class="modal-section technical-details">
        <summary class="modal-section-title">Technical details</summary>
        <div class="requirements-grid">
          <div class="requirement-item"><div class="requirement-value">${escapeHtml(app.sourceSummary)}</div><div class="requirement-label">Source</div></div>
          <div class="requirement-item"><div class="requirement-value">${escapeHtml(roleLabel(app.role))}</div><div class="requirement-label">Role</div></div>
          <div class="requirement-item"><div class="requirement-value">${escapeHtml(app.capsuleType || "Unknown")}</div><div class="requirement-label">Type</div></div>
        </div>
        <ul class="permissions-list">
          <li><span class="permission-icon">${icons.check}</span>${escapeHtml(signatureLabel(app.signatureState))}</li>
          <li><span class="permission-icon">${icons.check}</span>${escapeHtml(packageLabel(app))}</li>
        </ul>
      </details>
    `;
  }

  function packageLabel(app) {
    if (app.trustState === "cid-with-manifest-signature") return "Verified";
    if (app.trustState === "local-manifest-signature") return "Signed local";
    if (app.installed) return "On this device";
    return "Catalog entry";
  }

  function trustLabel(stateValue) {
    const labels = {
      "cid-with-manifest-signature": "Verified SmartWeb app",
      "local-manifest-signature": "Signed local app",
      "cid-without-manifest-signature": "Verification incomplete",
      "local-dev": "Local app",
    };
    return labels[stateValue] || "Not declared";
  }

  function signatureLabel(stateValue) {
    const labels = {
      "manifest-signature-declared": "Manifest signature declared",
      "no-manifest-signature": "Manifest signature not declared",
    };
    return labels[stateValue] || "Manifest signature status unavailable";
  }

  function openApp(appId) {
    const app = state.apps.find((candidate) => candidate.id === appId);
    if (!app || !app.launchable || !app.launchTarget) {
      return;
    }
    if (window.top === window || !homeParentOrigin) {
      showToast("Open Apps from Home to launch apps.", true);
      return;
    }
    window.top.postMessage({
      type: "home:open-target",
      target: app.launchTarget,
      homeToken,
    }, homeParentOrigin);
  }

  function openMedia(mintId) {
    if (!RUNTIME_CUSTODY_MINT_ID.test(mintId)) {
      return;
    }
    // The viewer follows the item, not the surface it was opened from. Every
    // item used to go to the player, so a picture arrived at a video frame.
    const listing = state.mediaListings.find((entry) => entry.mintId === mintId);
    if (!listing) {
      return;
    }
    if (window.top === window || !homeParentOrigin) {
      showToast("Open Apps from Home to open items.", true);
      return;
    }
    window.top.postMessage({
      type: "home:open-target",
      target: viewerForListing(listing),
      query: { mint_id: mintId },
      homeToken,
    }, homeParentOrigin);
  }

  // A shared asset link reaches Runtime as a `token_uri` start (task-9-brief).
  // `legacy_listing: true` is a listing link Runtime still recognises by the
  // old path, and that path is unchanged: it fetches the package and
  // verifies its metadata, its chain record and its content before the item
  // appears on this Home's own shelf. Anything else is a live
  // `ListingObject`, which opens the offer sheet instead -- nothing here
  // decides that an item is genuine either way.
  async function importListing(value) {
    const link = marketLinkFromInput(value);
    if (!link) {
      showToast("That does not look like a listing link.", true);
      return;
    }
    if (importInFlight) {
      return;
    }
    importInFlight = true;
    els.importSubmit.disabled = true;
    els.importSubmit.setAttribute("aria-busy", "true");
    try {
      const result = await readMarketListing({ token_uri: link });
      if (result.legacy) {
        await postObjectProvider("import_runtime_custody", { listing_uri: legacyListingUriFromLink(link) });
        els.importInput.value = "";
        showToast("Listing added.", false);
        await loadMediaData();
        return;
      }
      els.importInput.value = "";
      noteMarketListingReadability(result.listing);
      openOfferSheet(result.listing);
    } catch (error) {
      showToast(publicError(error.message, "That listing could not be added."), true);
    } finally {
      importInFlight = false;
      els.importSubmit.disabled = false;
      els.importSubmit.removeAttribute("aria-busy");
      render();
    }
  }

  // Only `elastos://` or `ipfs://` plus exactly a folder CID -- a shared link
  // names the asset alone (shared-context.md D3), never a seller. The one
  // path Runtime also accepts, `ipfs://<cid>/<file>` (a token URI names the
  // file inside the folder), is cut back to its folder first. Checked before
  // Runtime is asked anything.
  function marketLinkFromInput(value) {
    const text = String(value ?? "").trim().replace(/^(ipfs:\/\/[^/]+)\/[^/]+$/, "$1");
    return MARKET_LINK_URI.test(text) ? text : "";
  }

  // The old import path only ever recognised `elastos://`. Both schemes name
  // the same folder CID (D4), so this keeps sending that path exactly the
  // link shape it has always required, whichever scheme a person pasted.
  function legacyListingUriFromLink(link) {
    return `elastos://${link.replace(/^(?:elastos|ipfs):\/\//, "")}`;
  }

  // A purchase is a sequence of waits, and re-issuing the identical buy is how
  // each one is collected: Runtime resumes the attempt it already holds against
  // the same effect, so asking again continues the purchase rather than making
  // a second one. Asking stops when the answer stops being a wait, when the
  // budget runs out, or when the person leaves the page.
  async function buyMedia(mintId) {
    if (!RUNTIME_CUSTODY_MINT_ID.test(mintId)) {
      showToast("This item cannot be bought.", true);
      return;
    }
    if (pendingMediaBuys.has(mintId)) {
      return;
    }
    setBuyState(mintId, { kind: "waiting", stage: "", awaitsPerson: false, connectorId: "" });
    const deadline = Date.now() + BUY_POLL_BUDGET_MS;
    try {
      for (;;) {
        try {
          await postObjectProvider("buy", { mint_id: mintId });
          pendingMediaBuys.delete(mintId);
          showToast("Bought. The item is in your Library.", false);
          await loadMediaData();
          return;
        } catch (error) {
          const outcome = buyOutcomeFromAnswer(error.answer);
          if (outcome.kind === "failed") {
            pendingMediaBuys.delete(mintId);
            showToast(publicError(error.message, "The purchase did not finish."), true);
            return;
          }
          if (outcome.kind === "declined") {
            // The person answered. Nothing here asks again on their behalf.
            setBuyState(mintId, outcome);
            return;
          }
          if (Date.now() > deadline) {
            // Out of the time this page will spend watching. The purchase is
            // still Runtime's, so say what is true rather than calling it
            // failed: pressing Buy picks the same attempt back up.
            pendingMediaBuys.delete(mintId);
            showToast("This purchase is still settling. Press Buy to pick it up again.", false);
            return;
          }
          setBuyState(mintId, outcome);
          await new Promise((resolve) => { window.setTimeout(resolve, BUY_POLL_MS); });
        }
      }
    } finally {
      render();
    }
  }

  // Ask Runtime to build this copy again. It reads the chain to see the item is
  // theirs, fetches the metadata the token URI points at and the content the
  // listing names, and files the result where the person will find it. Nothing
  // here decides ownership, and nothing here is a second way to get a copy.
  async function downloadOwnedCopy(mintId) {
    if (!RUNTIME_CUSTODY_MINT_ID.test(mintId) || pendingDownloads.has(mintId)) {
      return;
    }
    pendingDownloads.add(mintId);
    render();
    try {
      await postObjectProvider("download_owned_copy", { mint_id: mintId });
      showToast("Downloaded. The copy is in your Library.", false);
    } catch (error) {
      showToast(publicError(error.message, "That copy could not be downloaded."), true);
    } finally {
      pendingDownloads.delete(mintId);
      render();
    }
  }

  // A bought market item's copy, asked for by its item. Runtime reruns
  // adoption for a completed purchase and then fetches the copy exactly as
  // `downloadOwnedCopy` does for a mint. It never pays, and it is refused
  // for an item this principal has not completed a purchase of.
  async function downloadMarketCopy(item) {
    const key = catalogLedgerKey(item);
    if (pendingMarketDownloads.has(key)) {
      return;
    }
    pendingMarketDownloads.add(key);
    render();
    renderOfferSheetIfShowing(item);
    try {
      await postObjectProvider("download_owned_copy", { item: marketItemClaim(item) });
      showToast("Downloaded. The copy is in your Library.", false);
      if (offerSheetShowingItem(item)) {
        closeDetail();
      }
      await Promise.all([loadMediaData(), state.catalogLoaded ? loadCatalogItems() : Promise.resolve()]);
    } catch (error) {
      showToast(publicError(error.message, "That copy could not be downloaded."), true);
    } finally {
      pendingMarketDownloads.delete(key);
      render();
      renderOfferSheetIfShowing(item);
    }
  }

  function setBuyState(mintId, outcome) {
    pendingMediaBuys.set(mintId, outcome);
    render();
  }

  function openWalletForApproval() {
    if (window.top === window || !homeParentOrigin) {
      showToast("Open Apps from Home to reach your wallet.", true);
      return;
    }
    window.top.postMessage({
      type: "home:open-target",
      target: "wallet",
      query: {},
      homeToken,
    }, homeParentOrigin);
  }

  function bindAppActions(root) {
    bindRasterIconFallbacks(root);
    root.querySelectorAll("[data-action]").forEach((node) => {
      if (node.dataset.bound === "true") {
        return;
      }
      node.dataset.bound = "true";
      node.addEventListener("click", (event) => {
        const target = event.currentTarget;
        const action = target.dataset.action;
        const appId = target.dataset.app;
        if (action !== "detail") {
          event.stopPropagation();
        }
        if (action === "detail") {
          if (target instanceof HTMLElement) {
            target.focus();
          }
          showAppDetail(appId);
        }
        if (action === "open") {
          closeDetail();
          openApp(appId);
        }
        if (action === "retry") {
          state.appLoading = true;
          state.appLoadError = null;
          renderSurfaceState();
          loadCatalogData()
            .then(render)
            .catch((error) => {
              showToast(publicError(error.message, "Couldn’t load apps."), true);
            });
        }
        if (action === "retry-media") {
          state.mediaLoading = true;
          state.mediaLoadError = null;
          renderSurfaceState();
          loadMediaData()
            .then(render)
            .catch((error) => {
              showToast(publicError(error.message, "Couldn’t load your items."), true);
            });
        }
        if (action === "refresh-media") {
          refreshMediaSurface(target);
        }
        if (action === "retry-market") {
          if (state.catalogLoading) {
            return;
          }
          const reading = loadCatalogItems();
          renderSections();
          reading.then(() => {
            render();
            // The button that was pressed is gone; focus goes to the one that
            // replaced it, or to the tab this surface belongs to.
            const next = els.storeSections.querySelector("[data-action='retry-market']")
              || els.mediaTabs.querySelector("[data-media-tab='explore']");
            next?.focus();
          });
        }
        if (action === "card-menu") {
          // One menu open at a time, and the control says which state it is
          // in so a screen reader is told the same thing the arrow shows.
          const menu = target.parentElement?.querySelector(".menu-list");
          const open = target.getAttribute("aria-expanded") === "true";
          closeCardMenus();
          if (menu && !open) {
            menu.hidden = false;
            target.setAttribute("aria-expanded", "true");
          }
        }
        if (action === "open-creator") {
          openApp(CREATOR_CAPSULE_ID);
        }
        if (action === "explore-filter") {
          state.exploreFilter = String(target.dataset.filter || "all");
          state.exploreGroup = "";
          renderSections();
        }
        if (action === "explore-more") {
          state.exploreGroup = String(target.dataset.group || "");
          renderSections();
        }
        if (action === "explore-all") {
          state.exploreGroup = "";
          renderSections();
        }
        if (action === "retry-shops") {
          loadShopsData().then(render).catch((error) => {
            showToast(publicError(error.message, "Couldn’t load channels."), true);
          });
        }
        if (action === "approve-shops") {
          // Approving is the Inbox's job, not this page's: it is an operator
          // decision about letting this Home talk to an outside service, and
          // it is recorded there with who allowed it. Asking for the list
          // already raised the request, so this says where it is waiting and
          // then becomes the way back.
          if (state.shopsAwaitingApproval) {
            loadShopsData().then(render).catch((error) => {
              showToast(publicError(error.message, "Couldn’t load channels."), true);
            });
            return;
          }
          state.shopsAwaitingApproval = true;
          renderSections();
        }
        if (action === "see-all") {
          selectDestination(target.dataset.destination);
        }
        if (action === "buy-media") {
          showBuyConfirmation(target.dataset.mint);
        }
        if (action === "confirm-buy") {
          const mintId = target.dataset.mint;
          closeDetail();
          buyMedia(mintId);
        }
        if (action === "open-wallet") {
          openWalletForApproval();
        }
        if (action === "share-listing") {
          showShareListing(target.dataset.mint);
        }
        if (action === "resume-buy") {
          buyMedia(target.dataset.mint);
        }
        if (action === "download-copy") {
          downloadOwnedCopy(target.dataset.mint);
        }
        if (action === "download-market-copy") {
          // The sheet names the item it is showing; a catalog row names the
          // item this page read a listing for under its ledger and token id.
          const item = els.detailContent.contains(target) && currentOfferListing
            ? currentOfferListing.item
            : marketAssetItems.get(catalogLedgerKey({ ledger: target.dataset.ledger, tokenId: target.dataset.token }));
          if (item) {
            downloadMarketCopy(item);
          }
        }
        if (action === "open-media") {
          openMedia(target.dataset.mint);
        }
        if (action === "buy-listing") {
          buyCatalogItem(target.dataset.ledger, target.dataset.token);
        }
        if (action === "open-listing") {
          openCatalogPurchase(target.dataset.ledger, target.dataset.token);
        }
        if (action === "item-details") {
          showCatalogDetails(target.dataset.ledger, target.dataset.token);
        }
        if (action === "buy-offer" && currentOfferListing) {
          const seller = target.dataset.seller;
          // A purchase Runtime recorded for this seller continues on the
          // recorded terms; anything else buys the row's live offer.
          const inProgress = offerItemPurchaseInProgress(currentOfferListing.item);
          const offer = inProgress?.recorded && inProgress.seller === seller
            ? inProgress.recorded
            : currentOfferListing.offers.find((entry) => entry.seller === seller && !entry._gone);
          if (offer) {
            buyOffer(currentOfferListing, offer);
          }
        }
        if (action === "close-detail") {
          closeDetail();
        }
      });
      node.addEventListener("keydown", (event) => {
        if ((event.key === "Enter" || event.key === " ") && node.dataset.action === "detail") {
          event.preventDefault();
          showAppDetail(node.dataset.app);
        }
      });
    });
  }

  // A menu stays open only while it is being used. Anywhere else, Escape and
  // the next click close it -- including a click on the control that opened
  // it, which is what makes it a toggle.
  function closeCardMenus() {
    document.querySelectorAll(".menu-list:not([hidden])").forEach((menu) => {
      menu.hidden = true;
    });
    document.querySelectorAll('[data-action="card-menu"][aria-expanded="true"]').forEach((control) => {
      control.setAttribute("aria-expanded", "false");
    });
  }

  function bindRasterIconFallbacks(root) {
    root.querySelectorAll(".app-icon-raster").forEach((container) => {
      if (container.dataset.iconFallbackBound === "true") {
        return;
      }
      container.dataset.iconFallbackBound = "true";
      const image = container.querySelector(".app-icon-img");
      const glyph = container.querySelector(".app-icon-glyph");
      if (!image || !glyph) {
        return;
      }
      // A cover in flight leaves the frame shimmering rather than blank: the
      // fetch goes to this Home's node and then, for anything it does not
      // already hold, out to the network, which is not instant.
      const settle = () => {
        image.removeAttribute("data-cover");
        container.classList.remove("thumb-pending");
      };
      if (image.complete) {
        settle();
      } else {
        image.addEventListener("load", settle, { once: true });
      }
      image.addEventListener("error", () => {
        settle();
        // An inline style rather than the `hidden` attribute alone. A
        // stylesheet that gives the image its own `display` outranks the
        // browser's `[hidden]` rule, which is how a dead image once stayed on
        // screen beside the glyph that replaced it -- and the design's own
        // stylesheet does exactly that, for both shapes.
        image.hidden = true;
        image.style.display = "none";
        glyph.hidden = false;
        container.classList.remove("app-icon-raster");
      });
    });
  }

  function emptyState(title, description, icon) {
    return `
      <div class="empty-state">
        <div class="empty-icon">${icon}</div>
        <div class="empty-title">${escapeHtml(title)}</div>
        <div class="empty-description">${escapeHtml(description)}</div>
      </div>
    `;
  }

  function catalogErrorMessage(response, payload) {
    if (response.status === 404) {
      return "Apps and services are unavailable. Update ElastOS and try again.";
    }
    return publicError(payload.error || payload.message, "Apps and services could not be loaded.");
  }

  async function postObjectProvider(operation, payload) {
    const response = await fetch(`/api/provider/object/${operation}`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-elastos-home-token": homeToken,
      },
      body: JSON.stringify(payload),
    });
    const data = await response.json().catch(() => ({}));
    if (!response.ok) {
      throw providerFailure(response, data);
    }
    if (data && typeof data === "object" && data.status === "error") {
      throw providerFailure(response, data.error || data);
    }
    return data && typeof data === "object" && "data" in data ? data.data : data;
  }

  // The sentence is for a person; the answer beside it is for this app. A
  // refusal that carries its state as data is how the app decides what to
  // offer, instead of matching words written to be read.
  function providerFailure(response, answer) {
    const failure = new Error(providerErrorMessage(response, answer));
    failure.answer = answer && typeof answer === "object" ? answer : {};
    return failure;
  }

  function providerErrorMessage(response, payload) {
    if (response.status === 404) {
      return "Protected items are unavailable. Update ElastOS and try again.";
    }
    return publicError(payload.error || payload.message, "Your items could not be loaded.");
  }

  function publicDescription(capsule) {
    const description = String(capsule.description || "").trim();
    const role = String(capsule.role || "app").toLowerCase();
    const title = publicTitle(capsule);
    if (role === "provider") return `${title} service for apps on this Home.`;
    if (description && !/\b(runtime|capsules?|providers?|projection|schema|derived facts?|boundary|capability surface|affordances?|host-loaded|structured home intents?)\b/i.test(description)) {
      return description;
    }
    if (role === "viewer") return `${title} opens compatible files and content.`;
    if (role === "content") return `${title} content.`;
    if (role === "shell") return `${title} Home view.`;
    return `${title} app.`;
  }

  function publicTitle(capsule) {
    const role = String(capsule?.role || "app").toLowerCase();
    let title = String(capsule?.title || titleCase(capsule?.name || "App")).trim();
    title = title
      .replace(/\bDid\b/g, "DID")
      .replace(/\bIpfs\b/g, "IPFS")
      .replace(/\bGba\b/g, "GBA");
    if (role === "provider") {
      title = title.replace(/\s+(Provider|Adapter)$/i, "").trim();
    }
    return title || (role === "provider" ? "Service" : "App");
  }

  function publicError(value, fallback) {
    const message = String(value || "").trim();
    if (!message || /\b(schema|projection|provider|adapter|capability|affordance|runtime|runtime-owned|launch token|hostcall|request failed|failed to fetch|unauthorized|forbidden|[45]\d\d)\b|engine_[a-z_]+/i.test(message)) {
      return fallback;
    }
    return message;
  }

  function badgeLabel(badge) {
    const labels = {
      app: "App",
      viewer: "Viewer",
      provider: "Service",
      service: "Service",
      content: "Content",
      shell: "Shell",
      ddrm: "dDRM",
      wallet: "Wallet",
      installed: "Installed",
    };
    return labels[badge] || titleCase(badge);
  }

  function roleLabel(role) {
    const labels = { app: "App", viewer: "Viewer", provider: "Service", content: "Content", shell: "Home view" };
    return labels[String(role || "").toLowerCase()] || "App";
  }

  function showToast(message, isError = false) {
    els.toast.textContent = message;
    els.toast.classList.toggle("error", isError);
    els.toast.classList.add("visible");
    window.clearTimeout(showToast.timer);
    showToast.timer = window.setTimeout(() => {
      els.toast.classList.remove("visible");
    }, 3200);
  }

  function titleCase(value) {
    return String(value || "")
      .split(/[-_\s]+/)
      .filter(Boolean)
      .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
      .join(" ");
  }

  function escapeHtml(value) {
    return String(value || "")
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;")
      .replace(/'/g, "&#039;");
  }

  function escapeAttr(value) {
    return escapeHtml(value);
  }
})();
