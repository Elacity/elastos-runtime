// Marketplace reads the capsule catalog and the protected-item shelf, and asks
// Home to launch what a person chooses. The listing contract it reads lives in
// `src/listing.js`, beside the tests that pin it against Runtime's producer.
import {
  ACCESS_STATES,
  MAX_RUNTIME_CUSTODY_LISTINGS,
  RUNTIME_CUSTODY_MINT_ID,
  buyOutcomeFromAnswer,
  listingUriFromInput,
  parseRuntimeCustodyListings,
  uint256Decimal,
  viewerForListing,
} from "./src/listing.js";

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
    catalogUnavailable: false,
    catalogNeedsApproval: false,
    catalogAwaitingApproval: false,
    catalogNote: "",
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
  const BUY_POLL_BUDGET_MS = 15 * 60 * 1000;
  let detailPreviousFocus = null;
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
  async function loadCatalogItems() {
    state.catalogLoading = true;
    renderSurfaceState();
    try {
      const response = await fetch("/api/apps/marketplace/items", {
        headers: { accept: "application/json", "x-elastos-home-token": homeToken },
      });
      const answer = await response.json().catch(() => ({}));
      if (!response.ok) {
        throw new Error(catalogErrorMessage(response, answer));
      }
      state.catalog = (Array.isArray(answer.items) ? answer.items : [])
        .map(catalogItemFromAnswer)
        .filter(Boolean);
      state.catalogUnavailable = answer.unavailable === true;
      state.catalogNeedsApproval = answer.needsApproval === true;
      state.catalogLoaded = true;
    } catch (error) {
      // An index that cannot be read leaves Explore with this Home's own
      // items: a thinner shelf, not a broken one.
      state.catalog = [];
      state.catalogUnavailable = true;
      state.catalogNote = publicError(error.message, "Couldn’t reach the market.");
    } finally {
      // Attempted either way. Without this a failure is retried on every
      // paint, and a paint follows every retry -- which is a loop that asks
      // an outside service as fast as the page can draw.
      state.catalogLoaded = true;
      state.catalogLoading = false;
    }
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
    if (state.catalogUnavailable && state.mediaTab === "explore") {
      // The important half: the market was not read, and what is on screen is
      // this Home's own shelf rather than the market being empty.
      return `Market unavailable — showing ${total} of your own.`;
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
      loadCatalogItems().then(render);
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

  // When the market could not be read, Explore is this Home's own shelf --
  // which looks exactly like a market with nothing in it unless it says so.
  // It is a note above the items rather than an error in place of them,
  // because the items are real and this is about what is missing beside them.
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
    if (state.catalogUnavailable) {
      return `<p class="store-inline-note">Couldn’t reach the market. Showing your own items.</p>`;
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
      return { label: "In your library", tone: "owned" };
    }
    if (isSoldOut(listing)) {
      return { label: "Sold out", tone: "gone" };
    }
    return null;
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
          ${statusTag(state)}
        </div>
        <div class="card-body">
          <h3 class="card-title" title="${escapeAttr(title)}">${escapeHtml(title)}</h3>
          <div class="card-meta">${state === "owned" ? "In your library" : `${stock}${seller}`}</div>
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
    const play = state !== "soldout" && listing.accessState !== "available"
      ? `<button class="play" type="button" data-action="open-media" data-mint="${mint}" aria-label="Open ${escapeAttr(title)}">${spriteIcon(previewIcon, "fill", "width:22px;height:22px")}</button>`
      : "";
    return `
    <article class="card video store-row-media" data-mint="${mint}" data-id="${escapeAttr(itemKey(listing))}" data-state="${escapeAttr(state)}">
      <div class="thumb app-icon-raster${cover ? " thumb-pending" : ""}">
        ${thumb}
        <div class="video-fallback app-icon-glyph"${cover ? " hidden" : ""}>${spriteIcon("i-media")}</div>
        <span class="tag left">${escapeHtml(formatBadge(listing))}</span>
        ${statusTag(state)}
        ${play}
      </div>
      <div class="card-body">
        <h3 class="card-title" title="${escapeAttr(title)}">${escapeHtml(title)}</h3>
        <div class="card-meta mono" title="${escapeAttr(listing.displayName)}">${escapeHtml(mediaCardSubtitle(listing, title))}</div>
        <div class="card-progress">${buyStateNoteMarkup(listing)}</div>
      </div>
      <div class="card-foot">
        <div>${priceBlock(listing, state)}<div class="stock">${state === "owned" ? "Purchased" : stock}</div></div>
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
  // someone else's item is otherwise missing.
  function mediaCardSubtitle(listing, title) {
    const fileName = listing.displayName;
    if (fileName && fileName !== title) {
      return listing.codecs ? `${fileName} · ${listing.codecs}` : fileName;
    }
    const seller = listing.accessState === "creator"
      ? "by you"
      : listing.sellerAddress
        ? `by ${abbreviateAddress(listing.sellerAddress)}`
        : "";
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

  function statusTag(state) {
    if (state === "mine") {
      return `<span class="tag right mine">Listed by you</span>`;
    }
    if (state === "owned") {
      return `<span class="tag right owned">In your library</span>`;
    }
    if (state === "soldout") {
      return `<span class="tag right sold">Sold out</span>`;
    }
    return "";
  }

  function priceBlock(listing, state) {
    if (state === "owned") {
      return `<div class="price owned">✓ In your library</div>`;
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
  // A token this Home does not know is shown in base units. That is useless
  // to read and honest, which is the right way round: inventing decimals
  // would turn an unknown price into a confident wrong one.
  function formatMoney(listing) {
    const full = uint256Decimal(listing.price);
    const token = payTokens.get(String(listing.payToken || "").toLowerCase());
    if (!token) {
      return { value: compactUnits(full), unit: "units", title: `${full} base units` };
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
    const secondary = state === "mine" || state === "owned"
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
    const focusTarget = els.detailContent.querySelector(".modal-btn.primary")
      || els.detailContent.querySelector("[data-action='close-detail']");
    focusTarget?.focus();
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
    els.detailContent.querySelector(".modal-btn.primary")?.focus();
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

  function closeDetail() {
    els.detailModal.classList.remove("active");
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

  // A listing published on another Home reaches this one by its link. This
  // checks the link's shape and hands it to Runtime, which fetches the package
  // and verifies its metadata, its chain record and its content before the
  // item appears on the shelf. Nothing here decides that an item is genuine.
  async function importListing(value) {
    const listingUri = listingUriFromInput(value);
    if (!listingUri) {
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
      await postObjectProvider("import_runtime_custody", { listing_uri: listingUri });
      els.importInput.value = "";
      showToast("Listing added.", false);
      await loadMediaData();
    } catch (error) {
      showToast(publicError(error.message, "That listing could not be added."), true);
    } finally {
      importInFlight = false;
      els.importSubmit.disabled = false;
      els.importSubmit.removeAttribute("aria-busy");
      render();
    }
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
        if (action === "open-media") {
          openMedia(target.dataset.mint);
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
