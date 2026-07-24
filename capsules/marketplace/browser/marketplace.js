(function () {
  const params = new URLSearchParams(window.location.search);
  const homeToken = new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token") || "";
  const homeParentOrigin = params.get("home_origin") || "";

  if (homeToken && homeParentOrigin && window.top !== window) {
    window.top.postMessage({ type: "home:app-ready", homeToken }, homeParentOrigin);
    window.top.postMessage({
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
    }, homeParentOrigin);
  }

  window.addEventListener("message", (event) => {
    if (event.origin !== "null" || event.source !== window.parent) {
      return;
    }
    const message = event.data;
    if (message?.type !== "elastos:menu-command" || typeof message.cmd !== "string") {
      return;
    }
    if (message.cmd === "refresh") {
      loadData().then(render).catch((error) => showToast(String(error.message || error)));
    }
  });

  const state = {
    apps: [],
    destination: "discover",
    search: "",
    loading: true,
    loadError: null,
  };

  let detailPreviousFocus = null;

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
    { id: "content", label: "Content", icon: "document" },
    { id: "providers", label: "Services", icon: "server" },
    { id: "shells", label: "Home views", icon: "system" },
  ];

  const icons = {
    grid: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="3" y="3" width="7" height="7"></rect><rect x="14" y="3" width="7" height="7"></rect><rect x="14" y="14" width="7" height="7"></rect><rect x="3" y="14" width="7" height="7"></rect></svg>',
    marketplace: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M18.36 9l.6 3H5.04l.6-3h12.72M20 4H4v2h16V4zm0 3H4l-1 5v2h1v6h10v-6h4v6h2v-6h1v-2l-1-5zM6 18v-4h6v4H6z"/></svg>',
    play: '<svg viewBox="0 0 24 24" fill="currentColor"><polygon points="5 3 19 12 5 21 5 3"></polygon></svg>',
    tools: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"></path></svg>',
    system: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="3"></circle><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path></svg>',
    server: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M4 1h16c1.1 0 2 .9 2 2v4c0 1.1-.9 2-2 2H4c-1.1 0-2-.9-2-2V3c0-1.1.9-2 2-2zm0 8h16c1.1 0 2 .9 2 2v4c0 1.1-.9 2-2 2H4c-1.1 0-2-.9-2-2v-4c0-1.1.9-2 2-2zm0 8h16c1.1 0 2 .9 2 2v4c0 1.1-.9 2-2 2H4c-1.1 0-2-.9-2-2v-4c0-1.1.9-2 2-2zm2-12v2h2V5H6zm0 8v2h2v-2H6zm0 8v2h2v-2H6z"/></svg>',
    blocks: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="2" y="7" width="6" height="6" rx="1"></rect><rect x="16" y="7" width="6" height="6" rx="1"></rect><rect x="9" y="2" width="6" height="6" rx="1"></rect><rect x="9" y="16" width="6" height="6" rx="1"></rect><path d="M8 10h2"></path><path d="M14 10h2"></path><path d="M12 8v2"></path><path d="M12 14v2"></path></svg>',
    wallet: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M21 18v1c0 1.1-.9 2-2 2H5c-1.11 0-2-.9-2-2V5c0-1.1.89-2 2-2h14c1.1 0 2 .9 2 2v1h-9c-1.11 0-2 .9-2 2v8c0 1.1.89 2 2 2h9zm-9-2h10V8H12v8zm4-2.5c-.83 0-1.5-.67-1.5-1.5s.67-1.5 1.5-1.5 1.5.67 1.5 1.5-.67 1.5-1.5 1.5z"/></svg>',
    document: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M7 3h7l4 4v14H7V3zm6 1.5V8h3.5L13 4.5zM9 12h6v1.5H9V12zm0 3h6v1.5H9V15z"/></svg>',
    package: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M16.5 9.4l-9-5.19M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z"></path><polyline points="3.27 6.96 12 12.01 20.73 6.96"></polyline><line x1="12" y1="22.08" x2="12" y2="12"></line></svg>',
    check: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M9 16.17L4.83 12l-1.42 1.41L9 19 21 7l-1.41-1.41z"/></svg>',
    close: '<svg viewBox="0 0 24 24"><path d="M19 6.41 17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z"/></svg>',
    search: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="8"></circle><path d="m21 21-4.35-4.35"></path></svg>',
  };

  boot();

  async function boot() {
    renderCategories();
    bindEvents();
    await loadData();
    render();
  }

  function bindEvents() {
    els.searchInput.addEventListener("input", (event) => {
      state.search = event.target.value.trim().toLowerCase();
      renderSections();
    });
    document.querySelectorAll("[data-destination]").forEach((node) => {
      if (node.closest("#category-list")) return;
      node.addEventListener("click", () => selectDestination(node.dataset.destination));
    });
    els.detailModal.addEventListener("click", (event) => {
      if (event.target === els.detailModal) closeDetail();
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
    /* Sidebar roving: ArrowUp/Down among Discover / Installed / categories.
       Never steal keys from Search (or other fields inside the sidebar). */
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
      if (item.dataset.destination) {
        selectDestination(item.dataset.destination);
      }
    });
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
    state.loading = true;
    state.loadError = null;
    setLoading(true);
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
        if (!capsule) continue;
        const entries = interfacesByCapsule.get(capsule) || [];
        entries.push(entry);
        interfacesByCapsule.set(capsule, entries);
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
      state.loadError = publicError(error.message, "Couldn’t load apps.");
    } finally {
      state.loading = false;
      setLoading(false);
    }
  }

  function setLoading(loading) {
    els.loadingState.classList.toggle("hidden", !loading);
    if (loading) {
      els.loadingState.innerHTML = skeletonRows(6);
      els.storeSections.classList.add("hidden");
      els.loadError.classList.add("hidden");
    } else {
      els.storeSections.classList.toggle("hidden", Boolean(state.loadError));
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

  /* First-party liquid masters live in home-gui; App Store loads them via the
     shared /apps/home-gui/icons/ surface so Shelf and Apps stay one family. */
  const HOME_ICON_ASSET_VERSION = "home-20260724ai";
  const FIRST_PARTY_ICON_IDS = new Set([
    "browser",
    "library",
    "documents",
    "inbox",
    "wallet",
    "system",
    "controls",
    "chat-room",
    "people",
    "services",
    "marketplace",
    "archive-manager",
    "gba-emulator",
    "gba-ucity",
    "bin",
    "home",
    "home-cli",
    "home-gui",
    "apps-launcher",
    "capsules",
    "security",
    "elastos",
    "intelligence",
    "browser-engine",
    "browser-exit",
    "chains",
    "content-index",
    "content-storage",
    "identity",
    "network",
    "storage",
    "wallet-security",
    "webspaces",
    "metamask",
    "unisat",
    "walletconnect",
  ]);
  const FIRST_PARTY_ICON_ALIASES = {
    trash: "bin",
    "trash-full": "bin",
    chat: "chat-room",
    launcher: "apps-launcher",
    "browser-engine-adapter": "browser-engine",
    "exit-provider": "browser-exit",
    "chain-provider": "chains",
    "content-block-graph-provider": "content-index",
    "object-provider": "content-storage",
    "did-provider": "identity",
    "net-provider": "network",
    "ipfs-provider": "storage",
    "wallet-provider": "wallet-security",
    "key-provider": "wallet-security",
    "webspace-provider": "webspaces",
    "site-provider": "webspaces",
    "wallet-metamask": "metamask",
    "wallet-unisat": "unisat",
    "wallet-walletconnect": "walletconnect",
    wc: "walletconnect",
  };

  function resolveFirstPartyIconId(capsule) {
    const name = String(capsule?.name || "").trim();
    const launch = String(capsule?.launch_target || "").trim();
    const candidates = [name, launch].filter(Boolean);
    for (const raw of candidates) {
      const id = raw;
      if (FIRST_PARTY_ICON_ALIASES[id]) return FIRST_PARTY_ICON_ALIASES[id];
      if (FIRST_PARTY_ICON_IDS.has(id)) return id;
      if (id.includes("wallet")) return "wallet";
      if (id.includes("ucity")) return "gba-ucity";
      if (id.includes("gba") || id.includes("emu") || id.includes("game")) return "gba-emulator";
      if (id.includes("doc") || id.includes("md") || id.includes("viewer")) return "documents";
      if (id.includes("file")) return "library";
      if (id.includes("room")) return "chat-room";
    }
    return null;
  }

  function capsuleToApp(capsule, capsulesByName, interfaceEntries) {
    const name = String(capsule.name || "");
    const role = String(capsule.role || "").toLowerCase();
    const category = appCategory(capsule, role);
    const installed = capsule.installed === true;
    const badges = appBadges(role, capsule, installed);
    const launchable = Boolean(capsule.launchable && capsule.launch_target);
    const acceptedContent = acceptedContentLabels(capsule, capsulesByName, interfaceEntries);
    const requirements = Array.isArray(capsule.requires) ? capsule.requires : [];
    const dependencies = requirements
      .map((entry) => {
        const dependency = capsulesByName.get(String(entry && entry.name || ""));
        return dependency ? publicTitle(dependency) : "";
      })
      .filter(Boolean);
    const technicalDependencies = requirements
      .map((entry) => String(entry && entry.name || "").trim())
      .filter(Boolean);
    const iconId = resolveFirstPartyIconId(capsule);
    return {
      id: name,
      name: publicTitle(capsule),
      developer: capsule.author || "Unknown publisher",
      category,
      description: publicDescription(capsule),
      size: capsule.cid ? "Verified app" : "Local app",
      version: capsule.version || "",
      icon: appIcon(role),
      iconId,
      gradient: appGradient(role),
      badges,
      requirements: {
        storage: capsule.cid ? "SmartWeb" : "Local",
      },
      installed,
      launchable,
      launchTarget: capsule.launch_target,
      route: capsule.route,
      role,
      capsuleType: capsule.type || capsule.capsule_type || "",
      cid: capsule.cid || "",
      cidState: capsule.cid_state || "",
      trustState: capsule.trust_state || "",
      signatureState: capsule.signature_state || "",
      paymentState: capsule.payment_state || "",
      drmState: capsule.drm_state || "",
      source: capsule.source || "",
      installPath: capsule.install_path || "",
      releasePath: capsule.release_path || "",
      repository: capsule.repository || "",
      viewerTitle: capsule.viewer_title || "",
      acceptedContent,
      dependencies,
      technicalDependencies,
      availableActions: executableActions(interfaceEntries),
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

  function catalogErrorMessage(response, payload) {
    if (response.status === 404) {
      return "Apps and services are unavailable. Update ElastOS and try again.";
    }
    return publicError(payload.error || payload.message, "Apps and services could not be loaded.");
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
    if (!message || /\b(schema|projection|provider|adapter|capability|affordance|runtime-owned|launch token|hostcall|request failed|failed to fetch|unauthorized|forbidden|[45]\d\d)\b|engine_[a-z_]+/i.test(message)) {
      return fallback;
    }
    return message;
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
    const badges = [role === "provider" ? "provider" : (role || "app")];
    if (installed) badges.push("installed");
    if (String(capsule.drm_state || "") === "provider") badges.push("ddrm");
    if (String(capsule.payment_state || "") === "provider") badges.push("wallet");
    return [...new Set(badges)];
  }

  function executableActions(interfaceEntries) {
    const actions = [];
    for (const entry of interfaceEntries) {
      for (const binding of Array.isArray(entry?.bindings) ? entry.bindings : []) {
        if (binding?.executable !== true) continue;
        const methodId = String(binding.method || "");
        if (methodId === "capsule.open") {
          actions.push("Open");
          continue;
        }
        const operation = methodId.split(".").filter(Boolean).at(-1);
        if (operation) actions.push(titleCase(operation));
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
          if (accepted?.mode === "unsupported_family_diagnostic") continue;
          for (const extension of Array.isArray(accepted?.extensions) ? accepted.extensions : []) {
            if (extension) extensions.add(String(extension));
          }
        }
      }
    }
    if (extensions.size) labels.push(`${[...extensions].join(", ")} files`);
    return [...new Set(labels)];
  }

  function appSortKey(app) {
    const categoryIndex = categories.findIndex((category) => category.id === app.category);
    return `${String(categoryIndex < 0 ? 99 : categoryIndex).padStart(2, "0")}:${app.name.toLowerCase()}`;
  }

  function render() {
    selectDestination(state.destination, { silent: true });
    updateInstalledBadge();
    renderLoadError();
    if (!state.loadError) renderSections();
  }

  function normalizeDestination(id) {
    if (id === "installed") return "installed";
    if (categories.some((category) => category.id === id)) return id;
    return "discover";
  }

  function destinationTitle(id) {
    if (id === "installed") return "Installed";
    const category = categories.find((entry) => entry.id === id);
    if (category) return category.label;
    return "Discover";
  }

  function selectDestination(id, options = {}) {
    state.destination = normalizeDestination(id);
    document.querySelectorAll("[data-destination]").forEach((node) => {
      const on = node.dataset.destination === state.destination;
      node.classList.toggle("selected", on);
      if (on) node.setAttribute("aria-current", "page");
      else node.removeAttribute("aria-current");
    });
    els.storeTitle.textContent = destinationTitle(state.destination);
    if (!options.silent) {
      renderLoadError();
      if (!state.loadError) renderSections();
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

  function renderLoadError() {
    if (!state.loadError) {
      els.loadError.classList.add("hidden");
      els.loadError.innerHTML = "";
      return;
    }
    els.storeSections.classList.add("hidden");
    els.loadError.classList.remove("hidden");
    els.loadError.innerHTML = `
      <div class="store-error-card">
        <div class="store-error-title">Couldn’t load apps</div>
        <div class="store-error-body">${escapeHtml(state.loadError)}</div>
        <button type="button" class="store-pill" data-action="retry">Retry</button>
      </div>
    `;
    bindAppActions(els.loadError);
  }

  function renderSections() {
    if (state.loading || state.loadError) return;
    els.storeSections.classList.remove("hidden");
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
    const launchable = state.apps.filter((app) => app.launchable && matchesSearch(app));
    if (launchable.length) {
      parts.push(renderSection("On This Device", launchable.slice(0, 9), {
        seeAllDestination: "installed",
      }));
    }
    for (const category of categories) {
      const apps = state.apps.filter((app) => app.category === category.id && matchesSearch(app));
      if (!apps.length) continue;
      parts.push(renderSection(category.label, apps, {
        seeAllDestination: category.id,
      }));
    }
    if (!parts.length) {
      els.storeSections.innerHTML = emptyState(
        state.search ? "No results" : "No apps to show",
        state.search
          ? "Try a different search."
          : "Apps on this Home will appear here.",
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
    els.storeSections.innerHTML = renderSection(destinationTitle(state.destination), apps, {});
    bindAppActions(els.storeSections);
  }

  function renderInstalledSections() {
    const installed = filteredByDestination();
    if (!installed.length) {
      els.storeSections.innerHTML = emptyState(
        state.search ? "No results" : "No apps installed",
        state.search
          ? "Try a different search."
          : "Launchable apps on this Home appear here.",
        icons.package,
      );
      return;
    }
    els.storeSections.innerHTML = renderSection("On This Device", installed, {});
    bindAppActions(els.storeSections);
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
      if (line) return line;
    }
    return roleLabel(app.role);
  }

  function detailPublisher(app) {
    if (isFirstPartyPublisher(app.developer)) return "ElastOS";
    return app.developer;
  }

  function renderAppRow(app) {
    const sub = rowSubtitle(app);
    return `
      <article class="store-row" data-action="detail" data-app="${escapeAttr(app.id)}" tabindex="0">
        ${appIconHtml(app, "store-row-icon")}
        <div class="store-row-text">
          <div class="store-row-title">${escapeHtml(app.name)}</div>
          <div class="store-row-sub">${escapeHtml(sub)}</div>
        </div>
        ${actionButton(app)}
      </article>
    `;
  }

  function actionButton(app) {
    if (!app.launchable) return "";
    return `<button class="store-pill" type="button" data-action="open" data-app="${escapeAttr(app.id)}">Open</button>`;
  }

  function appIconHtml(app, extraClass = "") {
    if (app.iconId) {
      const src = `/apps/home-gui/icons/${encodeURIComponent(app.iconId)}/icon-128.png?v=${HOME_ICON_ASSET_VERSION}`;
      return `<span class="app-icon app-icon-raster ${escapeAttr(extraClass)}"><img class="app-icon-img" src="${escapeAttr(src)}" alt="" draggable="false" /></span>`;
    }
    return `<span class="app-icon ${escapeAttr(app.gradient)} ${escapeAttr(extraClass)}">${icons[app.icon] || icons.package}</span>`;
  }

  function badgesHtml(app) {
    return app.badges
      .filter((badge) => badge !== "pending")
      .map((badge) => {
        const tip = badgeTooltip(badge);
        return `<span class="badge ${escapeAttr(badge)}"${tip ? ` title="${escapeAttr(tip)}"` : ""}>${escapeHtml(badgeLabel(badge))}</span>`;
      })
      .join("");
  }

  function badgeTooltip(badge) {
    const tips = {
      wallet: "Supports payments (capability flag — not a purchase)",
      ddrm: "Uses protected content (capability flag)",
      provider: "System service",
      installed: "Installed on this Home",
    };
    return tips[badge] || "";
  }

  function showAppDetail(appId) {
    const app = state.apps.find((candidate) => candidate.id === appId);
    if (!app) return;
    const openButton = app.launchable
      ? `<button class="modal-btn primary" type="button" data-action="open" data-app="${escapeAttr(app.id)}">Open</button>`
      : "";

    detailPreviousFocus = document.activeElement;
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
          <div class="modal-section-title">Access</div>
          <ul class="permissions-list">
            ${accessItems(app).map((item) => `<li><span class="permission-icon">${icons.check}</span>${escapeHtml(item)}</li>`).join("")}
          </ul>
        </section>
        ${relationshipSection(app)}
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
    } else {
      focusables[index >= focusables.length - 1 ? 0 : index + 1].focus();
    }
  }

  function accessItems(app) {
    const items = [];
    if (app.availableActions.length) items.push(`Available: ${app.availableActions.join(", ")}`);
    if (app.paymentState && app.paymentState !== "none") items.push("Supports payments");
    if (app.drmState && app.drmState !== "none") items.push("Uses protected content");
    return items.length ? items : ["No extra access requested"];
  }

  function relationshipSection(app) {
    const items = [];
    if (app.viewerTitle) items.push(`Opens with ${app.viewerTitle}`);
    if (app.acceptedContent.length) items.push(`Accepts ${app.acceptedContent.join(", ")}`);
    if (app.dependencies.length) items.push(`Needs ${app.dependencies.join(", ")}`);
    if (!items.length) return "";
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
          <div class="requirement-item"><div class="requirement-value">${escapeHtml(app.requirements.storage)}</div><div class="requirement-label">Source</div></div>
          <div class="requirement-item"><div class="requirement-value">${escapeHtml(roleLabel(app.role))}</div><div class="requirement-label">Role</div></div>
          <div class="requirement-item"><div class="requirement-value">${escapeHtml(app.capsuleType || "Unknown")}</div><div class="requirement-label">Type</div></div>
        </div>
        <ul class="permissions-list">
          ${distributionItems(app).map((item) => `<li>${escapeHtml(item)}</li>`).join("")}
        </ul>
      </details>
    `;
  }

  function distributionItems(app) {
    return [
      `Source: ${app.source || "Local"}`,
      `Trust: ${trustLabel(app.trustState)}`,
      `Signature: ${signatureLabel(app.signatureState)}`,
      `App identity: ${app.cid ? "Verified SmartWeb app" : "Local app"}`,
      app.repository ? `Repository: ${app.repository}` : "",
      app.technicalDependencies.length ? `Requires: ${app.technicalDependencies.join(", ")}` : "",
    ].filter(Boolean);
  }

  function packageLabel(app) {
    if (app.trustState === "cid-with-manifest-signature") return "Verified";
    if (app.trustState === "local-manifest-signature") return "Signed local";
    if (app.cid) return "Verified";
    return "On this device";
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
      "manifest-signature-declared": "Declared in manifest",
      "no-manifest-signature": "Not declared",
    };
    return labels[stateValue] || "Unknown";
  }

  function openApp(appId) {
    const app = state.apps.find((candidate) => candidate.id === appId);
    if (!app || !app.launchable || !app.launchTarget) {
      showToast("This app is not launchable from Home.", true);
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

  function bindAppActions(root) {
    root.querySelectorAll("[data-action]").forEach((node) => {
      if (node.dataset.bound === "true") return;
      node.dataset.bound = "true";
      node.addEventListener("click", (event) => {
        const target = event.currentTarget;
        const action = target.dataset.action;
        const appId = target.dataset.app;
        if (action !== "detail") event.stopPropagation();
        if (action === "detail") showAppDetail(appId);
        if (action === "open") {
          closeDetail();
          openApp(appId);
        }
        if (action === "close-detail") closeDetail();
        if (action === "see-all") selectDestination(target.dataset.destination);
        if (action === "retry") {
          loadData().then(render).catch((error) => {
            state.loadError = publicError(error.message, "Couldn’t load apps.");
            renderLoadError();
          });
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

  function emptyState(title, description, icon) {
    return `
      <div class="empty-state">
        <div class="empty-icon">${icon}</div>
        <div class="empty-title">${escapeHtml(title)}</div>
        <div class="empty-description">${escapeHtml(description)}</div>
      </div>
    `;
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
