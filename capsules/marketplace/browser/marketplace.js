(function () {
  const params = new URLSearchParams(window.location.search);
  const homeToken = new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token") || "";
  const homeParentOrigin = params.get("home_origin") || "";

  if (homeToken && homeParentOrigin && window.top !== window) {
    window.top.postMessage({ type: "home:app-ready", homeToken }, homeParentOrigin);
  }

  const state = {
    apps: [],
    currentCategory: "all",
    currentTab: "discover",
    search: "",
    loading: true,
  };

  const els = {
    categoryList: document.querySelector("#category-list"),
    loadingState: document.querySelector("#loading-state"),
    discoverContent: document.querySelector("#discover-content"),
    installedContent: document.querySelector("#installed-content"),
    appsGrid: document.querySelector("#apps-grid"),
    allAppsTitle: document.querySelector("#all-apps-title"),
    installedList: document.querySelector("#installed-list"),
    installedBadge: document.querySelector("#installed-badge"),
    detailModal: document.querySelector("#detail-modal"),
    detailContent: document.querySelector("#detail-content"),
    installModal: document.querySelector("#install-modal"),
    installContent: document.querySelector("#install-content"),
    searchInput: document.querySelector("#search-input"),
    themeToggle: document.querySelector("#theme-toggle"),
    toast: document.querySelector("#toast"),
  };

  const categories = [
    { id: "all", label: "All", icon: "grid" },
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

  initTheme();
  boot();

  async function boot() {
    renderCategories();
    bindEvents();
    await loadData();
    render();
  }

  function initTheme() {
    if (window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches) {
      document.documentElement.setAttribute("data-theme", "dark");
    }
  }

  function toggleTheme() {
    const isDark = document.documentElement.getAttribute("data-theme") === "dark";
    if (isDark) {
      document.documentElement.removeAttribute("data-theme");
      return;
    }
    document.documentElement.setAttribute("data-theme", "dark");
  }

  function bindEvents() {
    els.themeToggle.addEventListener("click", toggleTheme);
    els.searchInput.addEventListener("input", (event) => {
      state.search = event.target.value.trim().toLowerCase();
      renderAppsGrid();
    });
    document.querySelectorAll(".tab").forEach((tab) => {
      tab.addEventListener("click", () => selectTab(tab.dataset.tab));
    });
    [els.detailModal, els.installModal].forEach((modal) => {
      modal.addEventListener("click", (event) => {
        if (event.target === modal) modal.classList.remove("active");
      });
    });
    document.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        els.detailModal.classList.remove("active");
        els.installModal.classList.remove("active");
      }
    });
  }

  function renderCategories() {
    els.categoryList.replaceChildren();
    for (const category of categories) {
      const button = document.createElement("button");
      button.className = "category-item";
      button.type = "button";
      button.dataset.category = category.id;
      button.innerHTML = `${icons[category.icon] || icons.package}<span>${escapeHtml(category.label)}</span>`;
      button.addEventListener("click", () => selectCategory(category.id));
      els.categoryList.append(button);
    }
  }

  async function loadData() {
    state.loading = true;
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
      showToast(publicError(error.message, "Apps and services could not be loaded."), true);
    } finally {
      state.loading = false;
      setLoading(false);
    }
  }

  function setLoading(loading) {
    els.loadingState.classList.toggle("hidden", !loading);
    els.discoverContent.classList.toggle("hidden", loading || state.currentTab !== "discover");
    els.installedContent.classList.toggle("hidden", loading || state.currentTab !== "installed");
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
    return {
      id: name,
      name: publicTitle(capsule),
      developer: capsule.author || "Unknown publisher",
      category,
      description: publicDescription(capsule),
      size: capsule.cid ? "Verified app" : "Local app",
      version: capsule.version || "",
      icon: appIcon(role),
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
    const badges = [role === "provider" ? "service" : (role || "app")];
    if (installed) badges.push("installed");
    if (String(capsule.drm_state || "") === "provider") badges.push("ddrm");
    if (String(capsule.payment_state || "") === "provider") badges.push("wallet");
    if (!installed && !capsule.launchable) badges.push("pending");
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
    return `${String(categoryIndex).padStart(2, "0")}:${app.name.toLowerCase()}`;
  }

  function render() {
    selectCategory(state.currentCategory, { silent: true });
    selectTab(state.currentTab, { silent: true });
    renderAppsGrid();
    renderInstalledApps();
    updateInstalledBadge();
  }

  function selectTab(tabId, options = {}) {
    state.currentTab = tabId === "installed" ? "installed" : "discover";
    document.querySelectorAll(".tab").forEach((tab) => {
      tab.classList.toggle("active", tab.dataset.tab === state.currentTab);
    });
    if (!state.loading) {
      els.discoverContent.classList.toggle("hidden", state.currentTab !== "discover");
      els.installedContent.classList.toggle("hidden", state.currentTab !== "installed");
    }
    if (!options.silent) renderInstalledApps();
  }

  function selectCategory(categoryId, options = {}) {
    state.currentCategory = categories.some((category) => category.id === categoryId) ? categoryId : "all";
    document.querySelectorAll(".category-item").forEach((item) => {
      item.classList.toggle("active", item.dataset.category === state.currentCategory);
    });
    const label = categories.find((category) => category.id === state.currentCategory)?.label || "All";
    els.allAppsTitle.textContent = state.currentCategory === "all" ? "All" : label;
    if (!options.silent) renderAppsGrid();
  }

  function filteredApps() {
    return state.apps.filter((app) => {
      if (state.currentCategory !== "all" && app.category !== state.currentCategory) return false;
      if (!state.search) return true;
      const haystack = [app.name, app.description, app.developer, app.role, app.category, app.badges.join(" ")]
        .join(" ")
        .toLowerCase();
      return haystack.includes(state.search);
    });
  }

  function renderAppsGrid() {
    const apps = filteredApps();
    if (!apps.length) {
      els.appsGrid.innerHTML = emptyState("No apps found", "Try a different search or category", icons.search);
      return;
    }
    els.appsGrid.innerHTML = apps.map(renderAppCard).join("");
    bindAppActions(els.appsGrid);
  }

  function renderInstalledApps() {
    const installed = state.apps.filter((app) => app.installed);
    if (!installed.length) {
      els.installedList.innerHTML = emptyState("No apps installed", "Installed apps will appear here.", icons.package);
      return;
    }
    els.installedList.innerHTML = installed.map((app) => `
      <article class="installed-item">
        ${appIconHtml(app)}
        <div class="installed-info">
          <div class="installed-name">${escapeHtml(app.name)}</div>
          <div class="installed-meta">${escapeHtml(installedMeta(app))}</div>
        </div>
        <div class="installed-actions">
          ${app.launchable
            ? `<button class="action-btn open" type="button" data-action="open" data-app="${escapeAttr(app.id)}">Open</button>`
            : `<button class="action-btn secondary" type="button" data-action="detail" data-app="${escapeAttr(app.id)}">Details</button>`}
        </div>
      </article>
    `).join("");
    bindAppActions(els.installedList);
  }

  function updateInstalledBadge() {
    const count = state.apps.filter((app) => app.installed).length;
    els.installedBadge.textContent = String(count);
    els.installedBadge.classList.toggle("hidden", count === 0);
  }

  function renderAppCard(app) {
    return `
      <article class="app-card" data-action="detail" data-app="${escapeAttr(app.id)}" tabindex="0">
        <div class="app-card-header">
          ${appIconHtml(app)}
          <div class="app-card-info">
            <div class="app-name">${escapeHtml(app.name)}</div>
            <div class="app-developer">${escapeHtml(app.developer)}</div>
            <div class="app-version-row">
              ${app.version ? `<span class="app-version">v${escapeHtml(app.version)}</span>` : ""}
              <div class="app-badges">${badgesHtml(app)}</div>
            </div>
          </div>
        </div>
        <div class="app-description">${escapeHtml(app.description)}</div>
        <div class="app-card-footer">
          <div class="app-footer-left">
            <span class="app-size">${escapeHtml(app.size)}</span>
            <span class="app-price free">${packageLabel(app)}</span>
          </div>
          ${actionButton(app)}
        </div>
      </article>
    `;
  }

  function actionButton(app) {
    if (app.launchable) {
      return `<button class="install-btn installed" type="button" data-action="open" data-app="${escapeAttr(app.id)}">Open</button>`;
    }
    if (app.installed) {
      return `<button class="install-btn secondary" type="button" data-action="detail" data-app="${escapeAttr(app.id)}">Details</button>`;
    }
    return `<button class="install-btn pending" type="button" data-action="install" data-app="${escapeAttr(app.id)}">Install pending</button>`;
  }

  function appIconHtml(app, extraClass = "") {
    return `<span class="app-icon ${escapeAttr(app.gradient)} ${escapeAttr(extraClass)}">${icons[app.icon] || icons.package}</span>`;
  }

  function badgesHtml(app) {
    return app.badges.map((badge) => `<span class="badge ${escapeAttr(badge)}">${escapeHtml(badgeLabel(badge))}</span>`).join("");
  }

  function showAppDetail(appId) {
    const app = state.apps.find((candidate) => candidate.id === appId);
    if (!app) return;
    const openButton = app.launchable
      ? `<button class="modal-btn primary" type="button" data-action="open" data-app="${escapeAttr(app.id)}">Open</button>`
      : `<button class="modal-btn disabled" type="button">Details only</button>`;
    const installButton = !app.launchable && !app.installed
      ? `<button class="modal-btn secondary" type="button" data-action="install" data-app="${escapeAttr(app.id)}">Install pending</button>`
      : "";

    els.detailContent.innerHTML = `
      <header class="modal-header">
        ${appIconHtml(app, "modal-icon-size")}
        <div class="modal-title-section">
          <div class="modal-title">${escapeHtml(app.name)}</div>
          <div class="modal-developer">${escapeHtml(app.developer)}</div>
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
        <div class="modal-footer-price"><span class="price-tag">${app.cid ? "Verified" : "Local"}</span></div>
        <div class="modal-footer-actions">
          <button class="modal-btn secondary" type="button" data-action="close-detail">Close</button>
          ${installButton}
          ${openButton}
        </div>
      </footer>
    `;
    bindAppActions(els.detailContent);
    els.detailModal.classList.add("active");
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
    return "Local";
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

  function showInstallPending(appId) {
    const app = state.apps.find((candidate) => candidate.id === appId);
    if (!app) return;
    els.installContent.innerHTML = `
      ${appIconHtml(app, "install-modal-icon")}
      <div class="install-modal-title">Install unavailable</div>
      <div class="install-modal-status">Installing new apps is not available yet.</div>
      <button class="modal-btn secondary" type="button" data-action="close-install">Close</button>
    `;
    bindAppActions(els.installContent);
    els.installModal.classList.add("active");
  }

  function openApp(appId) {
    const app = state.apps.find((candidate) => candidate.id === appId);
    if (!app || !app.launchable || !app.launchTarget) {
      showToast("This app is not launchable from Home.", true);
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
          els.detailModal.classList.remove("active");
          openApp(appId);
        }
        if (action === "install") showInstallPending(appId);
        if (action === "close-detail") els.detailModal.classList.remove("active");
        if (action === "close-install") els.installModal.classList.remove("active");
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
      <div class="empty-state" style="grid-column:1/-1;">
        <div class="empty-icon">${icon}</div>
        <div class="empty-title">${escapeHtml(title)}</div>
        <div class="empty-description">${escapeHtml(description)}</div>
      </div>
    `;
  }

  function installedMeta(app) {
    return [
      app.version ? `v${app.version}` : "",
      "Installed",
      app.role,
      app.cid ? "Verified" : "",
    ].filter(Boolean).join(" / ");
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
      content: "Content",
      shell: "Shell",
      ddrm: "dDRM",
      wallet: "Wallet",
      installed: "Installed",
      pending: "Pending",
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

  function short(value) {
    const text = String(value || "");
    return text.length > 24 ? `${text.slice(0, 12)}...${text.slice(-8)}` : text;
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
