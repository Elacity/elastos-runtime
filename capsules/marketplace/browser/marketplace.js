import { sharedModelOffers, modelAccessOpportunities, REMOTE_MODEL_ID, SERVICE_OFFER_ID } from "./model-services.js";

(function () {
  const params = new URLSearchParams(window.location.search);
  const homeToken = new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token") || "";
  const homeParentOrigin = params.get("home_origin") || "";
  const RUNTIME_CUSTODY_LISTINGS_RESPONSE_SCHEMA_V1 = "elastos.library.runtime-custody-listings/v1";
  const RUNTIME_CUSTODY_LISTING_SCHEMA_V1 = "elastos.library.runtime-custody-listing/v1";
  const RUNTIME_CUSTODY_AVAILABILITY_SCHEMA_V1 = "elastos.library.runtime-custody-availability-summary/v1";
  const MAX_RUNTIME_CUSTODY_LISTINGS = 128;
  const MAX_RUNTIME_CUSTODY_PUBLIC_TEXT_BYTES = 256;
  const MAX_U32 = 0xffffffff;
  const MAX_SAFE_TIMESTAMP = Number.MAX_SAFE_INTEGER;
  const RUNTIME_CUSTODY_MINT_ID = /^[0-9a-f]{64}$/;
  const UINT256_HEX = /^0x(?:0|[1-9a-f][0-9a-f]{0,63})$/;
  const ADDRESS_HEX = /^0x[0-9a-f]{40}$/;

  const state = {
    apps: [],
    mediaListings: [],
    destination: "discover",
    search: "",
    appLoading: true,
    mediaLoading: true,
    appLoadError: null,
    mediaLoadError: null,
    mediaTruncated: false,
    // Runtime policy: protected content (Media) ships after 0.8.0 (#91).
    protectedContent: false,
    modelCatalogState: "unconfigured",
    sharedModels: [],
    modelAccess: [],
    sharedLoading: false,
    sharedLoaded: false,
    remoteAvailability: [],
  };

  const pendingMediaBuys = new Set();
  let detailPreviousFocus = null;
  let detailModels = null;
  let pendingModelLaunch = null;
  const MODEL_CONTENT_CID = /^bafybei[a-z2-7]{51}[aeimquy4]$/;
  const MODEL_OFFER_ID = /^model:[0-9a-f]{64}$/;
  let homeChromeReady = false;
  let lastHomeMenuManifestSignature = "";

  const els = {
    categoryList: document.querySelector("#category-list"),
    loadingState: document.querySelector("#loading-state"),
    loadError: document.querySelector("#load-error"),
    storeSections: document.querySelector("#store-sections"),
    storeTitle: document.querySelector("#store-title"),
    installedBadge: document.querySelector("#installed-badge"),
    detailModal: document.querySelector("#detail-modal"),
    detailContent: document.querySelector("#detail-content"),
    searchInput: document.querySelector("#search-input"),
    toast: document.querySelector("#toast"),
  };

  const categories = [
    { id: "apps", label: "Apps", icon: "package" },
    { id: "viewers", label: "Viewers", icon: "play" },
    { id: "models", label: "Models", icon: "package" },
    { id: "content", label: "Content", icon: "document" },
  ];

  // Every model row and detail, local or shared, uses the content icon.
  const MODEL_ICON = { icon: "document", gradient: "gradient-green", iconRoute: "" };
  const MODEL_FIT_LABELS = { fits: "Fits this device", insufficient: "Needs more memory", unavailable: "Memory not checked" };

  const icons = {
    package: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M16.5 9.4l-9-5.19M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z"></path><polyline points="3.27 6.96 12 12.01 20.73 6.96"></polyline><line x1="12" y1="22.08" x2="12" y2="12"></line></svg>',
    play: '<svg viewBox="0 0 24 24" fill="currentColor"><polygon points="5 3 19 12 5 21 5 3"></polygon></svg>',
    document: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M7 3h7l4 4v14H7V3zm6 1.5V8h3.5L13 4.5zM9 12h6v1.5H9V12zm0 3h6v1.5H9V15z"/></svg>',
    search: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="8"></circle><path d="m21 21-4.35-4.35"></path></svg>',
    close: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M19 6.41 17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z"/></svg>',
    check: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M9 16.17 4.83 12l-1.42 1.41L9 19 21 7l-1.41-1.41z"/></svg>',
    media: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="3" y="5" width="18" height="14" rx="2"></rect><path d="m10 9 5 3-5 3V9Z" fill="currentColor" stroke="none"></path></svg>',
  };

  const CAPSULE_ICON_ROUTE = /^\/apps\/[A-Za-z0-9_.-]+\/[A-Za-z0-9_./-]+\.png$/;

  boot();

  async function boot() {
    renderCategories();
    bindEvents();
    announceHomeChrome();
    await loadData();
    render();
    applyModelsLaunch(modelsQueryFromLocation());
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
    els.detailModal.addEventListener("click", (event) => {
      if (event.target === els.detailModal) {
        closeDetail();
      }
    });
    document.addEventListener("keydown", (event) => {
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
    window.addEventListener("message", handleTrustedHomeMessage);
  }

  function handleTrustedHomeMessage(event) {
    if (event.origin !== "null" || event.source !== window.parent) {
      return;
    }
    const data = event.data;
    if (data?.type === "elastos.marketplace.navigate/v1") {
      const keys = data && typeof data === "object" ? Object.keys(data).sort() : [];
      if (data.homeToken !== homeToken || keys.length !== 3
          || keys[0] !== "homeToken" || keys[1] !== "query" || keys[2] !== "type") {
        return;
      }
      applyModelsLaunch(data.query);
      return;
    }
    if (data?.type !== "elastos:menu-command" || typeof data.cmd !== "string") {
      return;
    }
    handleHomeMenuCommand(data.cmd);
  }

  function modelsQueryFromLocation() {
    const query = {};
    const category = params.get("category");
    const modelCid = params.get("model_cid");
    if (category) query.category = category;
    if (modelCid) query.model_cid = modelCid;
    return query;
  }

  function modelsLaunchQuery(query) {
    if (!query || typeof query !== "object" || Array.isArray(query) || query.category !== "models") {
      return null;
    }
    const keys = Object.keys(query);
    if (keys.length === 1 && keys[0] === "category") {
      return { category: "models" };
    }
    if (keys.length === 2 && Object.prototype.hasOwnProperty.call(query, "model_cid")
        && MODEL_CONTENT_CID.test(query.model_cid)) {
      return { category: "models", model_cid: query.model_cid };
    }
    return null;
  }

  function applyModelsLaunch(query) {
    const launch = modelsLaunchQuery(query);
    if (!launch) {
      return;
    }
    pendingModelLaunch = launch.model_cid ? launch : null;
    selectDestination("models");
    openPendingModelDetail();
  }

  function openPendingModelDetail() {
    const launch = pendingModelLaunch;
    if (!launch?.model_cid) {
      return;
    }
    const app = state.apps.find((candidate) => candidate.modelCid === launch.model_cid);
    if (!app) {
      return;
    }
    pendingModelLaunch = null;
    showAppDetail(app.id);
  }

  function openReadyModelInAssistant({ cid, offer_id }) {
    if (!MODEL_CONTENT_CID.test(cid || "")) {
      return;
    }
    if (window.top === window || !homeParentOrigin) {
      showToast("Open Marketplace from Home to use this model.", true);
      return;
    }
    const query = { model_cid: cid };
    if (MODEL_OFFER_ID.test(offer_id || "")) {
      query.offer_id = offer_id;
    }
    window.top.postMessage({
      type: "home:open-target",
      target: "assistant",
      query,
      homeToken,
    }, homeParentOrigin);
  }

  function handleHomeMenuCommand(command) {
    if (command === "refresh") {
      // View > Refresh also reads an open model detail again, in place.
      void detailModels?.refresh();
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
      loadCatalogData().then(() => {
        render();
        if (!state.protectedContent) {
          state.mediaLoading = false;
          return undefined;
        }
        return loadMediaData().then(render);
      }),
      ...(state.sharedLoaded ? [loadSharedModels()] : []),
    ]);
  }

  async function loadSharedModels() {
    if (state.sharedLoading) return;
    state.sharedLoading = true;
    state.sharedLoaded = true;
    state.sharedModels = [];
    state.modelAccess = [];
    state.remoteAvailability = [];
    render();
    const read = async (url, post = false) => {
      const response = await fetch(url, { method: post ? "POST" : "GET", headers: {
        "x-elastos-home-token": homeToken, ...(post ? { "content-type": "application/json" } : {}),
      }, ...(post ? { body: "{}" } : {}), signal: AbortSignal.timeout(35000) });
      if (!response.ok) throw new Error("Model access unavailable");
      const reply = await response.json();
      if (reply?.status === "error") throw new Error("Model access unavailable");
      return reply;
    };
    let failed = false;
    // Summary reconciles new Inbox approvals into grants before discovery reads them.
    await read("/api/apps/services/summary")
      .then(reply => { state.modelAccess = modelAccessOpportunities(reply); })
      .catch(() => { failed = true; });
    await read("/api/provider/model/offers_list", true).then(reply => {
      state.sharedModels = sharedModelOffers(reply);
      state.remoteAvailability = (reply?.data ?? reply)?.remote_services || [];
    }).catch(() => { failed = true; });
    state.sharedLoading = false;
    render();
    if (failed && state.destination === "models") showToast("Couldn’t load shared models.", true);
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
      state.modelCatalogState = catalog.model_catalog_state || "unconfigured";
      state.protectedContent = catalog.policy?.protected_content === true;
      const modelCids = new Set();
      for (const capsule of capsules.filter(item => item.source === "signed-model-catalog")) {
        if (catalog.model_catalog_state !== "verified" || capsule.role !== "content" || capsule.installed !== false || capsule.launchable !== false ||
            capsule.signature_state !== "catalog-signature-verified" || !/^bafybei[a-z2-7]{52}$/.test(capsule.cid) ||
            typeof capsule.publisher_did !== "string" || !capsule.publisher_did.startsWith("did:") ||
            !Number.isSafeInteger(capsule.content_size_bytes) || capsule.content_size_bytes <= 0 || modelCids.has(capsule.cid)) {
          throw new Error("Model catalog verification is unavailable.");
        }
        modelCids.add(capsule.cid);
      }
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
        .filter(listedInMarketplace)
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
    } catch (error) {
      state.mediaListings = [];
      state.mediaTruncated = false;
      state.mediaLoadError = publicError(error.message, "Couldn’t load media.");
    } finally {
      state.mediaLoading = false;
    }
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

  // Marketplace lists only what a person can open or get: apps, viewers and
  // content Home can open, and signed models. Home views, services and helpers
  // Home cannot open are not listed.
  function listedInMarketplace(capsule) {
    if (capsule.source === "signed-model-catalog") return true;
    return capsule.launchable === true && Boolean(capsule.launch_target) && String(capsule.role || "").toLowerCase() !== "shell";
  }

  function capsuleToApp(capsule, capsulesByName, interfaceEntries) {
    const model = capsule.source === "signed-model-catalog";
    const role = String(capsule.role || "").toLowerCase();
    const installed = capsule.installed === true;
    const launchable = !model && Boolean(capsule.launchable && capsule.launch_target);
    const dependencies = (Array.isArray(capsule.requires) ? capsule.requires : [])
      .map((entry) => {
        const dependency = capsulesByName.get(String(entry && entry.name || ""));
        return dependency ? publicTitle(dependency) : "";
      })
      .filter(Boolean);
    return {
      id: model ? `model:${capsule.cid}` : String(capsule.name || ""),
      modelCid: model ? capsule.cid : null,
      contentBytes: model ? capsule.content_size_bytes : null,
      modelFit: model ? String(capsule.model_memory_fit || "") : "",
      name: publicTitle(capsule),
      // Runtime names a publisher only for a signed model or an installed item
      // whose package matches a record signed by a release key it trusts.
      developer: typeof capsule.publisher_did === "string" && capsule.publisher_did.startsWith("did:") ? capsule.publisher_did : "",
      category: appCategory(capsule, role),
      description: model ? modelDescription(capsule) : publicDescription(capsule),
      version: String(capsule.version || ""),
      installed,
      launchable,
      launchTarget: model ? "" : String(capsule.launch_target || ""),
      role,
      capsuleType: String(capsule.type || capsule.capsule_type || ""),
      trustState: String(capsule.trust_state || ""),
      signatureState: String(capsule.signature_state || ""),
      paymentState: String(capsule.payment_state || ""),
      drmState: String(capsule.drm_state || ""),
      iconRoute: catalogIconRoute(capsule),
      icon: appIcon(role),
      gradient: appGradient(role),
      badges: model ? ["model"] : appBadges(role, capsule, installed),
      acceptedContent: acceptedContentLabels(capsule, capsulesByName, interfaceEntries),
      dependencies,
      availableActions: model ? ["Get and keep on this device", "Open in Assistant", "Remove in System → Models"] : executableActions(interfaceEntries),
      viewerTitle: String(capsule.viewer_title || ""),
      sourceSummary: capsule.cid ? "SmartWeb" : "Local",
    };
  }

  // A signed catalogue entry may carry its own one-line description; without
  // one, Runtime's generic capsule text is replaced by the model's purpose.
  function modelDescription(capsule) {
    const description = String(capsule.description || "").trim();
    return description && !/\bruntime\b/i.test(description) ? description : "Language model for Assistant.";
  }

  function appCategory(capsule, role) {
    if (capsule.source === "signed-model-catalog") return "models";
    const category = String(capsule.category || "").toLowerCase();
    const canonical = {
      apps: "apps",
      viewers: "viewers",
      content: "content",
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
    if (role === "viewer") return "play";
    if (role === "content") return "document";
    return "package";
  }

  function appGradient(role) {
    if (role === "viewer") return "gradient-blue";
    if (role === "content") return "gradient-green";
    return "gradient-teal";
  }

  function appBadges(role, capsule, installed) {
    const badges = [role || "app"];
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
    syncProtectedContent();
    selectDestination(state.destination, { silent: true });
    updateInstalledBadge();
    const surfaceBlocked = renderSurfaceState();
    if (!surfaceBlocked) {
      renderSections();
    }
    syncHomeMenuManifest();
    openPendingModelDetail();
  }

  function syncProtectedContent() {
    document.querySelectorAll('[data-destination="media"]').forEach((node) => {
      node.hidden = !state.protectedContent;
    });
    if (!state.protectedContent && state.destination === "media") selectDestination("discover", { silent: true });
  }

  function normalizeDestination(id) {
    if (id === "media") return state.protectedContent ? "media" : "discover";
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
    if (state.destination === "models" && !state.sharedLoaded) void loadSharedModels();
    document.querySelectorAll("[data-destination]").forEach((node) => {
      const selected = node.dataset.destination === state.destination;
      node.classList.toggle("selected", selected);
      if (selected) node.setAttribute("aria-current", "page");
      else node.removeAttribute("aria-current");
    });
    els.storeTitle.textContent = destinationTitle(state.destination);
    if (!options.silent) {
      const surfaceBlocked = renderSurfaceState();
      if (!surfaceBlocked) {
        renderSections();
      }
    }
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
      return {
        destination: "media",
        loading: state.mediaLoading,
        error: state.mediaLoadError,
        retryAction: "retry-media",
        title: "Couldn’t load media",
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

  // Models use the same store rows as every category: models this Home can run,
  // then models shared with it and contacts' AI services.
  function renderModels() {
    const matches = item => !state.search || JSON.stringify(item).toLowerCase().includes(state.search);
    const rows = [
      ...filteredByDestination().map(renderAppRow),
      ...state.sharedModels.filter(matches).map(renderSharedModelRow),
      ...state.modelAccess.filter(matches).map(renderModelAccessRow),
    ];
    if (!rows.length) {
      els.storeSections.innerHTML = state.sharedLoading ? skeletonRows(3)
        : state.search ? emptyState("No results", "Try a different search.", icons.search)
        : state.modelCatalogState === "unavailable" ? emptyState("Models are not available right now.", "", icons.search)
        : emptyState("No models to show", "Choose another category from the sidebar.", icons.search);
      return;
    }
    const unreachable = state.sharedLoading ? 0 : state.remoteAvailability.filter(service => service.status !== "reachable").length;
    els.storeSections.innerHTML = `
      <section class="store-section">
        <div class="store-row-grid">${rows.join("")}</div>
        ${unreachable ? `<p class="store-inline-note" role="status">${unreachable} approved service${unreachable === 1 ? " is" : "s are"} not reachable right now.</p>` : ""}
      </section>
    `;
    bindAppActions(els.storeSections);
  }

  function renderSharedModelRow(model) {
    return `
      <article class="store-row" data-action="shared-detail" data-offer="${escapeAttr(model.id)}" tabindex="0">
        ${appIconHtml(MODEL_ICON, "store-row-icon")}
        <div class="store-row-text">
          <div class="store-row-title">${escapeHtml(model.title)}</div>
          <div class="store-row-sub">Shared by ${escapeHtml(model.owner)}</div>
        </div>
        <button class="store-pill" type="button" data-action="use-shared-model" data-offer="${escapeAttr(model.id)}">Open</button>
      </article>
    `;
  }

  function renderModelAccessRow(item) {
    return `
      <article class="store-row store-row-static">
        ${appIconHtml(MODEL_ICON, "store-row-icon")}
        <div class="store-row-text">
          <div class="store-row-title">${escapeHtml(item.title)}</div>
          <div class="store-row-sub">From a contact · ${escapeHtml(item.status)}</div>
        </div>
        <button class="store-pill" type="button" data-action="model-service-access" data-offer="${escapeAttr(item.id)}">${item.requestable ? "Ask" : "Review"}</button>
      </article>
    `;
  }

  function renderCategorySections() {
    if (state.destination === "models") { renderModels(); return; }
    const apps = filteredByDestination();
    if (!apps.length) {
      els.storeSections.innerHTML = emptyState(
        state.search ? "No results" : "No apps in this category",
        state.search ? "Try a different search." : "Choose another category from the sidebar.",
        icons.search,
      );
      return;
    }
    els.storeSections.innerHTML = renderSection(destinationTitle(state.destination), apps, { showHeading: false });
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
    els.storeSections.innerHTML = renderSection("Installed", installed, { showHeading: false });
    bindAppActions(els.storeSections);
  }

  function renderMediaSections() {
    const listings = filteredMediaListings();
    if (!listings.length) {
      els.storeSections.innerHTML = emptyState(
        state.search ? "No results" : "No protected media available",
        state.search ? "Try a different search." : "Protected listings on this Home will appear here.",
        icons.media,
      );
      return;
    }
    const notice = state.mediaTruncated
      ? `<p class="store-inline-note">Showing the first ${MAX_RUNTIME_CUSTODY_LISTINGS} protected items.</p>`
      : "";
    els.storeSections.innerHTML = `
      ${notice}
      <section class="store-section">
        <div class="store-section-head">
          <h2 class="store-section-title">Protected media</h2>
        </div>
        <div class="store-row-grid">
          ${listings.map(renderMediaRow).join("")}
        </div>
      </section>
    `;
    bindAppActions(els.storeSections);
  }

  function filteredMediaListings() {
    return state.mediaListings.filter((listing) => {
      if (!state.search) {
        return true;
      }
      const haystack = [
        listing.displayName,
        listing.mimeType,
        listing.codecs,
        `quantity ${uint256Decimal(listing.quantity)}`,
        `price ${uint256Decimal(listing.price)} base units`,
        listing.payToken,
        listing.accessState,
      ].join(" ").toLowerCase();
      return haystack.includes(state.search);
    });
  }

  function renderSection(title, apps, { seeAllDestination, showHeading = true } = {}) {
    const seeAll = seeAllDestination && apps.length
      ? `<button type="button" class="store-see-all" data-action="see-all" data-destination="${escapeAttr(seeAllDestination)}">See All</button>`
      : "";
    return `
      <section class="store-section">
        ${showHeading ? `<div class="store-section-head">
          <h2 class="store-section-title">${escapeHtml(title)}</h2>
          ${seeAll}
        </div>` : ""}
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

  function modelSizeLabel(bytes) {
    const units = ["B", "KB", "MB", "GB", "TB"];
    const rank = bytes ? Math.min(4, Math.floor(Math.log(bytes) / Math.log(1000))) : 0;
    return `${(bytes / 1000 ** rank).toLocaleString(undefined, { maximumFractionDigits: rank ? 2 : 0 })} ${units[rank]}`;
  }

  // Only a Runtime-verified signer gives a publisher line; there is no other.
  function detailPublisher(app) {
    return app.developer ? "Verified publisher" : "";
  }

  // A model row shows its download size and whether it fits this device.
  function rowSubtitle(app) {
    if (!app.modelCid) return detailPublisher(app);
    const fit = Object.hasOwn(MODEL_FIT_LABELS, app.modelFit) ? MODEL_FIT_LABELS[app.modelFit] : "";
    return [modelSizeLabel(app.contentBytes), fit].filter(Boolean).join(" · ");
  }

  function renderAppRow(app) {
    return `
      <article class="store-row" data-action="${app.modelCid ? "model-detail" : "detail"}" data-app="${escapeAttr(app.id)}" tabindex="0">
        ${appIconHtml(app, "store-row-icon")}
        <div class="store-row-text">
          <div class="store-row-title">${escapeHtml(app.name)}</div>
          ${rowSubtitle(app) ? `<div class="store-row-sub">${escapeHtml(rowSubtitle(app))}</div>` : ""}
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
        </div>
        ${mediaActionButton(listing)}
      </article>
    `;
  }

  function mediaSubtitle(listing) {
    return listing.codecs
      ? `${listing.mimeType} • ${listing.codecs}`
      : listing.mimeType;
  }

  function mediaActionButton(listing) {
    if (listing.accessState === "available" && pendingMediaBuys.has(listing.mintId)) {
      return `<button class="store-pill" type="button" data-action="buy-media" data-mint="${escapeAttr(listing.mintId)}" disabled aria-busy="true">Buying...</button>`;
    }
    const action = listing.accessState === "available" ? "buy-media" : "open-media";
    const label = listing.accessState === "available" ? "Buy" : "Open";
    return `<button class="store-pill" type="button" data-action="${escapeAttr(action)}" data-mint="${escapeAttr(listing.mintId)}">${label}</button>`;
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

  function uint256Decimal(value) {
    if (!UINT256_HEX.test(value)) {
      throw new Error("invalid protected media quantity");
    }
    return BigInt(value).toString(10);
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
      installed: "Installed on this Home",
    };
    return tips[badge] || "";
  }

  function renderDetail({ icon, title, developer, developerTitle = "", version = "", badges, body, footerNote = "", actions = "" }) {
    detailModels?.destroy();
    detailModels = null;
    detailPreviousFocus = document.activeElement;
    els.detailContent.innerHTML = `
      <header class="modal-header">
        ${icon}
        <div class="modal-title-section">
          <div class="modal-title">${escapeHtml(title)}</div>
          ${developer ? `<div class="modal-developer"${developerTitle ? ` title="${escapeAttr(developerTitle)}"` : ""}>${escapeHtml(developer)}</div>` : ""}
          ${version ? `<div class="modal-version">Version ${escapeHtml(version)}</div>` : ""}
          <div class="modal-badges">${badges}</div>
        </div>
        <button class="modal-close" type="button" data-action="close-detail" aria-label="Close">${icons.close}</button>
      </header>
      <div class="modal-body">${body}</div>
      <footer class="modal-footer">
        ${footerNote ? `<div class="modal-footer-price"><span class="trust-chip">${escapeHtml(footerNote)}</span></div>` : ""}
        <div class="modal-footer-actions">
          <button class="modal-btn secondary" type="button" data-action="close-detail">Close</button>
          ${actions}
        </div>
      </footer>
    `;
    bindAppActions(els.detailContent);
  }

  function activateDetail() {
    els.detailModal.classList.add("active");
    const focusTarget = els.detailContent.querySelector(".modal-btn.primary")
      || els.detailContent.querySelector("[data-action='close-detail']");
    focusTarget?.focus();
  }

  // Every item uses one detail layout: About, Status, Works with (when it has
  // relationships), Available actions and Technical details, with its one
  // forward action next to Close.
  function showAppDetail(appId) {
    const app = state.apps.find((candidate) => candidate.id === appId);
    if (!app) {
      return;
    }
    renderDetail({
      icon: appIconHtml(app, "modal-icon-size"), title: app.name, developer: detailPublisher(app),
      developerTitle: app.developer, version: app.version,
      badges: badgesHtml(app), footerNote: packageLabel(app),
      body: `${aboutSection(app.description)}
        ${app.modelCid
          ? listSection("Status", [], 'data-model-management role="group" aria-label="Model status"')
          : listSection("Status", statusItems(app))}
        ${relationshipSection(app)}
        ${listSection("Available actions", availableActionItems(app))}
        ${technicalDetails(app)}`,
      actions: app.modelCid ? '<span class="model-actions" data-model-actions></span>'
        : app.launchable
          ? `<button class="modal-btn primary" type="button" data-action="open" data-app="${escapeAttr(app.id)}">Open</button>`
          : "",
    });
    if (app.modelCid) {
      // Runtime owns the model's state: the shared controller fills Status and
      // puts Get, Retry, Stop or Open in Assistant next to Close.
      const itemMarker = document.createElement("span");
      itemMarker.className = "permission-icon";
      itemMarker.innerHTML = icons.check;
      detailModels = window.ElastosModelManagement.create({
        root: els.detailContent.querySelector("[data-model-management]"), capsule: "marketplace", token: homeToken,
        buttonClass: "modal-btn secondary", primaryButtonClass: "modal-btn primary", cid: app.modelCid, compact: true, itemMarker,
        onReadyOpen: openReadyModelInAssistant,
        actionsRoot: els.detailContent.querySelector("[data-model-actions]"),
      });
      detailModels.setVisible(true);
    }
    activateDetail();
  }

  // A shared model shows where prompts go before it opens in Assistant.
  function showSharedModelDetail(offerId) {
    const model = state.sharedModels.find((candidate) => candidate.id === offerId);
    if (!model) {
      return;
    }
    const facts = [{ term: "Model", value: model.facts.requestedModel }, ...model.facts.detailRows];
    renderDetail({
      icon: appIconHtml(MODEL_ICON, "modal-icon-size"), title: model.title, developer: `Shared by ${model.owner}`,
      badges: badgesHtml({ badges: ["model", "shared"] }),
      footerNote: `Access ends ${new Date(model.expiresAt * 1000).toLocaleString()}`,
      body: `${aboutSection(`Shared with this Home by ${model.owner}.`)}
        ${listSection("Status", facts.map((row) => `${row.term}: ${row.value}`))}
        ${listSection("Available actions", ["Open in Assistant"])}
        ${technicalSection(["Shared", "Model", "Remote"], [`Offer ID: ${model.id}`])}`,
      actions: `<button class="modal-btn primary" type="button" data-action="use-shared-model" data-offer="${escapeAttr(model.id)}">Open in Assistant</button>`,
    });
    activateDetail();
  }

  function aboutSection(description) {
    return `<section class="modal-section">
          <div class="modal-section-title">About</div>
          <div class="modal-description">${escapeHtml(description)}</div>
        </section>`;
  }

  function listSection(title, items, listAttributes = "") {
    return `
      <section class="modal-section">
        <div class="modal-section-title">${escapeHtml(title)}</div>
        <ul class="permissions-list"${listAttributes ? ` ${listAttributes}` : ""}>
          ${items.map((item) => `<li><span class="permission-icon">${icons.check}</span><span>${escapeHtml(item)}</span></li>`).join("")}
        </ul>
      </section>
    `;
  }

  function closeDetail() {
    detailModels?.destroy();
    detailModels = null;
    els.detailModal.classList.remove("active");
    const restore = detailPreviousFocus;
    detailPreviousFocus = null;
    if (restore && typeof restore.focus === "function" && document.contains(restore)) {
      restore.focus();
    }
  }

  function trapDetailFocus(event) {
    const focusables = [...els.detailContent.querySelectorAll(
      'button:not([disabled]), summary, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])',
    )].filter((node) => !node.closest("[hidden]") && node.offsetParent !== null && node.checkVisibility());
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
    if (app.paymentState === "provider") {
      items.push("Supports payments");
    }
    if (app.drmState === "provider") {
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
    return items.length ? listSection("Works with", items) : "";
  }

  function technicalDetails(app) {
    return app.modelCid
      ? technicalSection([app.sourceSummary, roleLabel(app.role), "Model"], [
        packageLabel(app), `Publisher: ${app.developer}`, `Content ID: ${app.modelCid}`, `${app.contentBytes.toLocaleString()} bytes`,
      ])
      : technicalSection([app.sourceSummary, roleLabel(app.role), app.capsuleType || "Unknown"], [
        signatureLabel(app), packageLabel(app), ...(app.developer ? [`Publisher: ${app.developer}`] : []),
      ]);
  }

  function technicalSection([source, role, type], rows) {
    return `
      <details class="modal-section technical-details">
        <summary class="modal-section-title">Technical details</summary>
        <div class="requirements-grid">
          <div class="requirement-item"><div class="requirement-value">${escapeHtml(source)}</div><div class="requirement-label">Source</div></div>
          <div class="requirement-item"><div class="requirement-value">${escapeHtml(role)}</div><div class="requirement-label">Role</div></div>
          <div class="requirement-item"><div class="requirement-value">${escapeHtml(type)}</div><div class="requirement-label">Type</div></div>
        </div>
        <ul class="permissions-list">
          ${rows.map((row) => `<li><span class="permission-icon">${icons.check}</span><span>${escapeHtml(row)}</span></li>`).join("")}
        </ul>
      </details>
    `;
  }

  function packageLabel(app) {
    if (app.modelCid) return "Catalog signature verified";
    return app.developer ? "Release signature verified" : "Publisher verification unavailable";
  }

  function trustLabel(stateValue) {
    const labels = {
      "cid-with-manifest-signature": "Published content and manifest signature declared",
      "local-manifest-signature": "Local manifest signature declared",
      "cid-without-manifest-signature": "Published content declared",
      "local-dev": "Local app",
    };
    return labels[stateValue] || "Not declared";
  }

  function signatureLabel(app) {
    const labels = {
      "manifest-signature-declared": "Manifest signature declared",
      "no-manifest-signature": "Manifest signature not declared",
    };
    return labels[app.signatureState] || "Manifest signature status unavailable";
  }

  function openApp(appId) {
    const app = state.apps.find((candidate) => candidate.id === appId);
    if (!app || !app.launchable || !app.launchTarget) {
      return;
    }
    if (window.top === window || !homeParentOrigin) {
      showToast("Open Marketplace from Home to launch apps.", true);
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
    if (window.top === window || !homeParentOrigin) {
      showToast("Open Marketplace from Home to launch apps.", true);
      return;
    }
    window.top.postMessage({
      type: "home:open-target",
      target: "elacity-player",
      query: { mint_id: mintId },
      homeToken,
    }, homeParentOrigin);
  }

  async function buyMedia(mintId) {
    if (!RUNTIME_CUSTODY_MINT_ID.test(mintId)) {
      showToast("Media action could not be completed.", true);
      return;
    }
    if (pendingMediaBuys.has(mintId)) {
      return;
    }
    pendingMediaBuys.add(mintId);
    render();
    try {
      await postObjectProvider("buy", { mint_id: mintId });
      await loadMediaData();
    } catch (error) {
      showToast(publicError(error.message, "Media action could not be completed."), true);
    } finally {
      pendingMediaBuys.delete(mintId);
      render();
    }
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
        if (action === "detail" || action === "model-detail") {
          if (target instanceof HTMLElement) {
            target.focus();
          }
          showAppDetail(appId);
        }
        if (action === "shared-detail") {
          if (target instanceof HTMLElement) {
            target.focus();
          }
          showSharedModelDetail(target.dataset.offer);
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
              showToast(publicError(error.message, "Couldn’t load media."), true);
            });
        }
        if (action === "see-all") {
          selectDestination(target.dataset.destination);
        }
        if (action === "buy-media") {
          buyMedia(target.dataset.mint);
        }
        if (action === "open-media") {
          openMedia(target.dataset.mint);
        }
        if (action === "close-detail") {
          closeDetail();
        }
        if (action === "use-shared-model" && REMOTE_MODEL_ID.test(target.dataset.offer || "")
            && state.sharedModels.some(model => model.id === target.dataset.offer)) {
          closeDetail();
          window.top.postMessage({ type: "home:open-target", target: "assistant", query: { offer_id: target.dataset.offer }, homeToken }, homeParentOrigin);
        }
        if (action === "model-service-access" && SERVICE_OFFER_ID.test(target.dataset.offer || "")
            && state.modelAccess.some(item => item.id === target.dataset.offer)) {
          window.top.postMessage({ type: "home:open-target", target: "services", query: { service_offer_id: target.dataset.offer }, homeToken }, homeParentOrigin);
        }
      });
      node.addEventListener("keydown", (event) => {
        if (event.target === node && (event.key === "Enter" || event.key === " ")
            && ["detail", "model-detail", "shared-detail"].includes(node.dataset.action)) {
          event.preventDefault();
          if (node.dataset.action === "shared-detail") showSharedModelDetail(node.dataset.offer);
          else showAppDetail(node.dataset.app);
        }
      });
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
      image.addEventListener("error", () => {
        image.hidden = true;
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
        ${description ? `<div class="empty-description">${escapeHtml(description)}</div>` : ""}
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
      throw new Error(providerErrorMessage(response, data));
    }
    if (data && typeof data === "object" && data.status === "error") {
      throw new Error(providerErrorMessage(response, data.error || data));
    }
    return data && typeof data === "object" && "data" in data ? data.data : data;
  }

  function providerErrorMessage(response, payload) {
    if (response.status === 404) {
      return "Protected media is unavailable. Update ElastOS and try again.";
    }
    return publicError(payload.error || payload.message, "Protected media could not be loaded.");
  }

  function publicDescription(capsule) {
    const description = String(capsule.description || "").trim();
    const role = String(capsule.role || "app").toLowerCase();
    const title = publicTitle(capsule);
    if (description && !/\b(runtime|capsules?|providers?|projection|schema|derived facts?|boundary|capability surface|affordances?|host-loaded|structured home intents?)\b/i.test(description)) {
      return description;
    }
    if (role === "viewer") return `${title} opens compatible files and content.`;
    if (role === "content") return `${title} content.`;
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

  function parseRuntimeCustodyListings(payload) {
    assertExactKeys(payload, ["schema", "listings", "truncated"]);
    if (payload.schema !== RUNTIME_CUSTODY_LISTINGS_RESPONSE_SCHEMA_V1) {
      throw new Error("invalid protected media schema");
    }
    if (!Array.isArray(payload.listings) || payload.listings.length > MAX_RUNTIME_CUSTODY_LISTINGS) {
      throw new Error("invalid protected media list");
    }
    if (typeof payload.truncated !== "boolean") {
      throw new Error("invalid protected media truncation state");
    }
    return {
      truncated: payload.truncated,
      listings: payload.listings.map(parseRuntimeCustodyListing),
    };
  }

  function parseRuntimeCustodyListing(value) {
    assertExactKeys(value, [
      "access_state",
      "availability",
      "codecs",
      "display_name",
      "mime_type",
      "mint_id",
      "pay_token",
      "price",
      "published_at",
      "quantity",
      "schema",
      "seller_address",
      "token_id",
    ]);
    if (value.schema !== RUNTIME_CUSTODY_LISTING_SCHEMA_V1) {
      throw new Error("invalid protected media entry");
    }
    const accessState = boundedString(value.access_state, 16);
    if (!["available", "creator", "purchased"].includes(accessState)) {
      throw new Error("invalid protected media access state");
    }
    const mintId = boundedString(value.mint_id, 64);
    const displayName = boundedString(value.display_name, MAX_RUNTIME_CUSTODY_PUBLIC_TEXT_BYTES);
    const mimeType = boundedString(value.mime_type, MAX_RUNTIME_CUSTODY_PUBLIC_TEXT_BYTES);
    const codecs = boundedString(value.codecs, MAX_RUNTIME_CUSTODY_PUBLIC_TEXT_BYTES);
    const quantity = boundedString(value.quantity, 66);
    const price = boundedString(value.price, 66);
    const payToken = boundedString(value.pay_token, 42);
    const publishedAt = boundedTimestamp(value.published_at);
    const sellerAddress = boundedString(value.seller_address, 42);
    const tokenId = boundedString(value.token_id, 66);
    if (!RUNTIME_CUSTODY_MINT_ID.test(mintId)) {
      throw new Error("invalid protected media mint");
    }
    if (!UINT256_HEX.test(quantity) || !UINT256_HEX.test(price)) {
      throw new Error("invalid protected media quantity");
    }
    if (!ADDRESS_HEX.test(payToken) || !ADDRESS_HEX.test(sellerAddress)) {
      throw new Error("invalid protected media address");
    }
    if (!UINT256_HEX.test(tokenId)) {
      throw new Error("invalid protected media token");
    }
    return {
      accessState,
      availability: parseAvailabilitySummary(value.availability),
      codecs,
      displayName,
      mimeType,
      mintId,
      payToken,
      price,
      publishedAt,
      quantity,
    };
  }

  function parseAvailabilitySummary(value) {
    assertExactKeys(value, [
      "checked_at",
      "observed_replicas",
      "recheck_before_buy",
      "recheck_before_open",
      "required_replicas",
      "schema",
      "status",
    ]);
    if (value.schema !== RUNTIME_CUSTODY_AVAILABILITY_SCHEMA_V1 || value.status !== "last_verified_receipt") {
      throw new Error("invalid protected media availability");
    }
    const checkedAt = boundedTimestamp(value.checked_at);
    const requiredReplicas = boundedCount(value.required_replicas);
    const observedReplicas = boundedCount(value.observed_replicas);
    if (
      requiredReplicas === 0
      || observedReplicas < requiredReplicas
      || value.recheck_before_buy !== true
      || value.recheck_before_open !== true
    ) {
      throw new Error("invalid protected media availability state");
    }
    return {
      checkedAt,
      observedReplicas,
      requiredReplicas,
    };
  }

  function boundedString(value, maxBytes) {
    if (
      typeof value !== "string"
      || !value
      || new TextEncoder().encode(value).length > maxBytes
      || /[\u0000-\u001f\u007f]/.test(value)
    ) {
      throw new Error("invalid protected media field");
    }
    return value;
  }

  function boundedCount(value) {
    if (!Number.isInteger(value) || value < 0 || value > MAX_U32) {
      throw new Error("invalid protected media count");
    }
    return value;
  }

  function boundedTimestamp(value) {
    if (!Number.isSafeInteger(value) || value <= 0 || value > MAX_SAFE_TIMESTAMP) {
      throw new Error("invalid protected media timestamp");
    }
    return value;
  }

  function assertExactKeys(value, keys) {
    if (!value || typeof value !== "object" || Array.isArray(value)) {
      throw new Error("invalid protected media object");
    }
    const actual = Object.keys(value).sort().join("\n");
    const expected = [...keys].sort().join("\n");
    if (actual !== expected) {
      throw new Error("invalid protected media shape");
    }
  }

  function badgeLabel(badge) {
    const labels = {
      app: "App",
      viewer: "Viewer",
      content: "Content",
      ddrm: "dDRM",
      wallet: "Wallet",
      installed: "Installed",
    };
    return labels[badge] || titleCase(badge);
  }

  function roleLabel(role) {
    const labels = { app: "App", viewer: "Viewer", content: "Content" };
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
