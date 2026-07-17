(function () {
  const params = new URLSearchParams(window.location.search);
  const homeToken = params.get("home_token") || "";

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
    staffPicksSection: document.querySelector("#staff-picks-section"),
    staffPicks: document.querySelector("#staff-picks"),
    popularSection: document.querySelector("#popular-section"),
    popularApps: document.querySelector("#popular-apps"),
    appsGrid: document.querySelector("#apps-grid"),
    allAppsTitle: document.querySelector("#all-apps-title"),
    installedList: document.querySelector("#installed-list"),
    installedBadge: document.querySelector("#installed-badge"),
    detailModal: document.querySelector("#detail-modal"),
    detailContent: document.querySelector("#detail-content"),
    installModal: document.querySelector("#install-modal"),
    installContent: document.querySelector("#install-content"),
    searchInput: document.querySelector("#search-input"),
    viewAll: document.querySelector("#view-all"),
    toast: document.querySelector("#toast"),
  };

  const categories = [
    { id: "all", label: "All Apps", icon: "grid" },
    { id: "marketplace", label: "Marketplace", icon: "marketplace" },
    { id: "media", label: "Media", icon: "play" },
    { id: "defi", label: "DeFi", icon: "wallet" },
    { id: "blockchain", label: "Blockchain", icon: "blocks" },
    { id: "tools", label: "Tools", icon: "tools" },
    { id: "system", label: "System", icon: "system" },
    { id: "providers", label: "Providers", icon: "server" },
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

  const staffGradients = {
    "gradient-purple": "linear-gradient(135deg, #667eea 0%, #764ba2 100%)",
    "gradient-blue": "linear-gradient(135deg, #3b82f6 0%, #1d4ed8 100%)",
    "gradient-orange": "linear-gradient(135deg, #f97316 0%, #ea580c 100%)",
    "gradient-green": "linear-gradient(135deg, #22c55e 0%, #16a34a 100%)",
    "gradient-teal": "linear-gradient(135deg, #14b8a6 0%, #0d9488 100%)",
    "gradient-pink": "linear-gradient(135deg, #ec4899 0%, #db2777 100%)",
    "gradient-yellow": "linear-gradient(135deg, #eab308 0%, #ca8a04 100%)",
    "gradient-indigo": "linear-gradient(135deg, #6366f1 0%, #4f46e5 100%)",
    "gradient-cyan": "linear-gradient(135deg, #06b6d4 0%, #0891b2 100%)",
    "gradient-slate": "linear-gradient(135deg, #64748b 0%, #475569 100%)",
  };

  boot();

  async function boot() {
    renderCategories();
    bindEvents();
    await loadData();
    render();
  }

  function bindEvents() {
    els.viewAll.addEventListener("click", () => selectCategory("all"));
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
      const response = await fetch("/api/capsules/catalog", {
        headers: { "x-elastos-home-token": homeToken },
      });
      const payload = await response.json().catch(() => ({}));
      if (!response.ok) {
        throw new Error(catalogErrorMessage(response, payload));
      }
      state.apps = (Array.isArray(payload.capsules) ? payload.capsules : [])
        .map(capsuleToApp)
        .sort((left, right) => appSortKey(left).localeCompare(appSortKey(right)));
    } catch (error) {
      state.apps = [];
      showToast(`Marketplace catalog unavailable: ${error.message || "unknown error"}`, true);
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

  function capsuleToApp(capsule) {
    const name = String(capsule.name || "");
    const role = String(capsule.role || "").toLowerCase();
    const category = appCategory(name, role, capsule);
    const installed = capsule.installed || capsule.source === "runtime-bundle";
    const badges = appBadges(name, role, capsule, installed);
    const launchable = Boolean(capsule.launchable && capsule.launch_target);
    return {
      id: name,
      name: capsule.title || titleCase(name),
      developer: capsule.author || "Unknown publisher",
      category,
      description: capsule.description || "App metadata available through Runtime.",
      shortDesc: shortDescription(capsule.description),
      size: capsule.cid ? "Verified app" : "Local app",
      version: capsule.version || "",
      icon: appIcon(name, role),
      gradient: appGradient(name, role, category),
      badges,
      permissions: permissionsFor(capsule),
      requirements: {
        storage: capsule.cid ? "SmartWeb" : "Local",
        memory: role === "provider" ? "Provider" : "App",
        compatibility: "ElastOS Runtime",
      },
      installed,
      isBuiltIn: capsule.source === "runtime-bundle",
      isPendingInstall: !installed && !launchable,
      isStaffPick: isStaffPick(name, role, launchable),
      isPopular: isPopular(name, role, launchable),
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
    };
  }

  function appCategory(name, role, capsule) {
    if (name === "marketplace") return "marketplace";
    if (role === "provider") return "providers";
    if (name.includes("wallet") || name.includes("chain") || name.includes("block") || name.includes("nft")) {
      return "blockchain";
    }
    if (name.includes("market")) return "marketplace";
    if (name.includes("defi") || name.includes("swap")) {
      return "defi";
    }
    if (name.includes("player") || name.includes("viewer") || name.includes("media") || name.includes("gba") || name.includes("archive")) {
      return "media";
    }
    if (role === "shell" || name === "system" || name.includes("provider")) return "system";
    if (role === "content") return "media";
    if (Array.isArray(capsule.capabilities) && capsule.capabilities.some((cap) => String(cap).includes("wallet"))) {
      return "blockchain";
    }
    return "tools";
  }

  function catalogErrorMessage(response, payload) {
    if (response.status === 404) {
      return "app catalog unavailable; update the Runtime gateway and try again";
    }
    return payload.error || payload.message || `request failed: ${response.status}`;
  }

  function appIcon(name, role) {
    if (name === "marketplace") return "marketplace";
    if (name.includes("wallet")) return "wallet";
    if (name.includes("doc") || name.includes("library")) return "document";
    if (name.includes("player") || name.includes("viewer") || name.includes("media") || name.includes("gba")) return "play";
    if (name.includes("chain") || name.includes("block")) return "blocks";
    if (role === "provider") return "server";
    if (role === "shell" || name === "system") return "system";
    return "package";
  }

  function appGradient(name, role, category) {
    if (name === "marketplace") return "gradient-purple";
    if (name.includes("wallet")) return "gradient-orange";
    if (category === "defi") return "gradient-yellow";
    if (category === "blockchain") return "gradient-indigo";
    if (category === "media") return "gradient-cyan";
    if (role === "provider") return "gradient-slate";
    if (role === "viewer") return "gradient-blue";
    if (role === "content") return "gradient-green";
    return "gradient-teal";
  }

  function appBadges(name, role, capsule, installed) {
    const badges = [role || "app"];
    if (installed) badges.push("installed");
    if (String(capsule.drm_state || "").includes("provider") || name.includes("drm")) badges.push("ddrm");
    if (String(capsule.payment_state || "").includes("provider") || name.includes("wallet")) badges.push("wallet");
    if (!installed && !capsule.launchable) badges.push("pending");
    return [...new Set(badges)];
  }

  function permissionsFor(capsule) {
    const permissions = [];
    if (capsule.provides) permissions.push(`Provides ${capsule.provides}`);
    if (Array.isArray(capsule.capabilities)) {
      permissions.push(...capsule.capabilities.map((cap) => `Requests ${cap}`));
    }
    if (Array.isArray(capsule.requires)) {
      permissions.push(...capsule.requires.map((req) => `Requires ${req.name || "app"} (${req.kind || "app"})`));
    }
    if (capsule.cid) permissions.push("Verified app identity");
    if (capsule.trust_state) permissions.push(`Trust ${trustLabel(capsule.trust_state)}`);
    return permissions;
  }

  function isStaffPick(name, role, launchable) {
    return (launchable && ["marketplace", "library", "documents", "wallet", "browser"].includes(name))
      || role === "shell";
  }

  function isPopular(name, role, launchable) {
    return launchable || ["provider", "viewer"].includes(role);
  }

  function appSortKey(app) {
    const categoryIndex = categories.findIndex((category) => category.id === app.category);
    return `${String(categoryIndex).padStart(2, "0")}:${app.name.toLowerCase()}`;
  }

  function render() {
    selectCategory(state.currentCategory, { silent: true });
    selectTab(state.currentTab, { silent: true });
    renderStaffPicks();
    renderPopularApps();
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
    const label = categories.find((category) => category.id === state.currentCategory)?.label || "All Apps";
    els.allAppsTitle.textContent = state.currentCategory === "all" ? "All Apps" : label;
    els.staffPicksSection.classList.toggle("hidden", state.currentCategory !== "all");
    els.popularSection.classList.toggle("hidden", state.currentCategory !== "all");
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

  function renderStaffPicks() {
    const picks = state.apps.filter((app) => app.isStaffPick).slice(0, 5);
    els.staffPicksSection.classList.toggle("hidden", state.currentCategory !== "all" || picks.length === 0);
    els.staffPicks.innerHTML = picks.map(renderStaffPick).join("");
    bindAppActions(els.staffPicks);
  }

  function renderStaffPick(app) {
    const bg = staffGradients[app.gradient] || staffGradients["gradient-purple"];
    return `
      <button class="staff-pick-card" type="button" style="background:${bg};" data-action="detail" data-app="${escapeAttr(app.id)}">
        <span class="staff-pick-icon">${icons[app.icon] || icons.package}</span>
        <span class="staff-pick-tag">${escapeHtml(categoryLabel(app.category))}</span>
        <span class="staff-pick-content">
          <span class="staff-pick-name">${escapeHtml(app.name)}</span>
          <span class="staff-pick-desc">${escapeHtml(app.shortDesc || app.description)}</span>
        </span>
      </button>
    `;
  }

  function renderPopularApps() {
    const apps = state.apps.filter((app) => app.isPopular).slice(0, 8);
    els.popularSection.classList.toggle("hidden", state.currentCategory !== "all" || apps.length === 0);
    els.popularApps.innerHTML = apps.map(renderAppCard).join("");
    bindAppActions(els.popularApps);
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
    const installed = state.apps.filter((app) => app.installed || app.isBuiltIn);
    if (!installed.length) {
      els.installedList.innerHTML = emptyState("No apps installed", "Installed Runtime apps will appear here.", icons.package);
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
    const count = state.apps.filter((app) => app.installed || app.isBuiltIn).length;
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
    if (app.installed || app.isBuiltIn) {
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
    const installButton = !app.launchable && !app.installed && !app.isBuiltIn
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
          <div class="modal-section-title">System</div>
          <div class="requirements-grid">
            <div class="requirement-item">
              <div class="requirement-value">${escapeHtml(app.requirements.storage)}</div>
              <div class="requirement-label">Source</div>
            </div>
            <div class="requirement-item">
              <div class="requirement-value">${escapeHtml(app.role || "-")}</div>
              <div class="requirement-label">Role</div>
            </div>
            <div class="requirement-item">
              <div class="requirement-value">${escapeHtml(app.capsuleType || "-")}</div>
              <div class="requirement-label">Type</div>
            </div>
          </div>
        </section>
        <section class="modal-section">
          <div class="modal-section-title">Authority</div>
          <ul class="permissions-list">
            ${authorityItems(app).map((item) => `<li><span class="permission-icon">${icons.check}</span>${escapeHtml(item)}</li>`).join("")}
          </ul>
        </section>
        <section class="modal-section">
          <div class="modal-section-title">Distribution</div>
          <ul class="permissions-list">
            ${distributionItems(app).map((item) => `<li><span class="permission-icon">${icons.check}</span>${escapeHtml(item)}</li>`).join("")}
          </ul>
        </section>
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

  function authorityItems(app) {
    const items = app.permissions.length ? app.permissions : ["No provider authority declared"];
    items.push(`dDRM: ${app.drmState || "not-declared"}`);
    items.push(`Payment: ${app.paymentState || "not-declared"}`);
    return items;
  }

  function distributionItems(app) {
    return [
      `Source: ${app.source || "runtime"}`,
      `Trust: ${trustLabel(app.trustState)}`,
      `Signature: ${signatureLabel(app.signatureState)}`,
      `App identity: ${app.cid ? "Verified SmartWeb app" : "Local app"}`,
      app.repository ? `Repository: ${app.repository}` : "",
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
      <div class="install-modal-title">Signed install pending</div>
      <div class="install-modal-status">Marketplace can browse and open installed apps. Installing new apps will be enabled after app signatures, receipts, and provider policy are verified.</div>
      <button class="modal-btn secondary" type="button" data-action="close-install">Close</button>
    `;
    bindAppActions(els.installContent);
    els.installModal.classList.add("active");
  }

  async function openApp(appId) {
    const app = state.apps.find((candidate) => candidate.id === appId);
    if (!app || !app.launchable || !app.launchTarget) {
      showToast("This app is not launchable from Home.", true);
      return;
    }
    try {
      if (window.parent && window.parent !== window) {
        window.parent.postMessage({
          type: "home:open-target",
          target: app.launchTarget,
          homeToken,
        }, window.location.origin);
        return;
      }
      const response = await fetch("/api/apps/home/launch", {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "x-elastos-home-token": homeToken,
        },
        body: JSON.stringify({ target: app.launchTarget }),
      });
      const payload = await response.json().catch(() => ({}));
      if (!response.ok) {
        throw new Error(payload.error || payload.message || `open failed: ${response.status}`);
      }
      if (payload.route) window.location.href = payload.route;
    } catch (error) {
      showToast(error.message || "Open failed", true);
    }
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
      app.isBuiltIn ? "Runtime bundle" : "Installed",
      app.role,
      app.cid ? "Verified" : "",
    ].filter(Boolean).join(" / ");
  }

  function categoryLabel(categoryId) {
    return categories.find((category) => category.id === categoryId)?.label.replace(" Apps", "") || titleCase(categoryId);
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
      provider: "Provider",
      content: "Content",
      shell: "Shell",
      ddrm: "dDRM",
      wallet: "Wallet",
      installed: "Installed",
      pending: "Pending",
    };
    return labels[badge] || titleCase(badge);
  }

  function shortDescription(description) {
    const text = String(description || "");
    return text.length > 110 ? `${text.slice(0, 107)}...` : text;
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
