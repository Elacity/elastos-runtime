/* Agent Settings / Usage pages + workbench panels.
   Bound from agent-harness.js (ctx + host). Tip: home-20260804as
   UI ≠ authority (Principle 16): pages never mint grants. */

import {
  getConfigureOverviewSnapshot,
  getMachineProfile,
  getRuntimeSnapshot,
  listPicksByTier,
  getUsageSnapshot,
  listCapabilities,
  getHardwareEstimate,
  getPlanMarkdown,
  getTruthSnapshot,
} from "./mock-agent-provider.js?v=home-20260804as";
import {
  DEFAULT_LIVE_SYSTEM_PROMPT,
  MAX_LIVE_SYSTEM_PROMPT_CHARS,
  clampLiveMaxTokens,
  clampLiveTemperature,
  normalizeLiveSystemPrompt,
  fetchAgentBackends,
  saveAgentBackends,
  getAgentBackendsCache,
} from "./agent-live.js?v=home-20260804as";

/** @type {null | object} */
let ctx = null;
/** @type {null | Record<string, Function>} */
let host = null;
let promptPanelBound = false;

export function bindAgentConfigure(nextCtx, nextHost = {}) {
  ctx = nextCtx;
  host = nextHost;
  bindPromptPanelOnce();
}

export const HARNESS_PAGES = {
  configure: {
    title: "Settings",
    sub: "What this AI needs on this machine.",
  },
  usage: {
    title: "Usage",
    sub: "On this device.",
  },
};
export const CONFIGURE_SECTIONS = new Set([
  "overview",
  "machine",
  "models",
  "prompt",
  "tools",
  "runtime",
]);
let harnessPage = null;
let configureSection = "overview";

function persistPromptPrefsSoon() {
  try {
    host?.persistAgentWorkspaceSoon?.();
  } catch {
    /* optional */
  }
}

function bindPromptPanelOnce() {
  if (promptPanelBound) {
    return;
  }
  const panel = document.querySelector("[data-prompt-panel]");
  if (!panel) {
    return;
  }
  promptPanelBound = true;
  panel.addEventListener("input", (event) => {
    if (!ctx) {
      return;
    }
    const target = event.target;
    if (target?.matches?.("[data-system-prompt]")) {
      ctx.systemPrompt = normalizeLiveSystemPrompt(target.value);
      const count = panel.querySelector("[data-system-prompt-count]");
      if (count) {
        count.textContent = `${String(ctx.systemPrompt).length}/${MAX_LIVE_SYSTEM_PROMPT_CHARS}`;
      }
      persistPromptPrefsSoon();
      return;
    }
    if (target?.matches?.("[data-agent-temperature]")) {
      ctx.temperature = clampLiveTemperature(target.value);
      persistPromptPrefsSoon();
      return;
    }
    if (target?.matches?.("[data-agent-max-tokens]")) {
      ctx.maxTokens = clampLiveMaxTokens(target.value);
      persistPromptPrefsSoon();
    }
  });
  panel.addEventListener("change", (event) => {
    if (!ctx) {
      return;
    }
    const target = event.target;
    if (target?.matches?.("[data-agent-temperature]")) {
      target.value = String(clampLiveTemperature(target.value));
      ctx.temperature = clampLiveTemperature(target.value);
      persistPromptPrefsSoon();
    }
    if (target?.matches?.("[data-agent-max-tokens]")) {
      target.value = String(clampLiveMaxTokens(target.value));
      ctx.maxTokens = clampLiveMaxTokens(target.value);
      persistPromptPrefsSoon();
    }
  });
  panel.addEventListener("click", (event) => {
    if (!ctx) {
      return;
    }
    if (event.target?.closest?.("[data-system-prompt-reset]")) {
      event.preventDefault();
      ctx.systemPrompt = DEFAULT_LIVE_SYSTEM_PROMPT;
      renderPromptPanel();
      persistPromptPrefsSoon();
      return;
    }
    if (event.target?.closest?.("[data-backends-save]")) {
      event.preventDefault();
      void saveBackendsFromPanel();
    }
  });
}

function fillBackendsPanel(backends) {
  if (!backends) {
    return;
  }
  const model = document.querySelector("[data-backend-model]");
  if (model) {
    model.value = String(backends.model || "");
  }
  const pairs = Array.isArray(backends.pairs) ? backends.pairs : [];
  const a = pairs.find((p) => p.id === "a") || pairs[0] || {};
  const b = pairs.find((p) => p.id === "b") || pairs[1] || {};
  const set = (sel, value) => {
    const el = document.querySelector(sel);
    if (el) {
      el.value = String(value || "");
    }
  };
  set("[data-backend-pair-a-label]", a.label);
  set("[data-backend-pair-a-url]", a.url);
  set("[data-backend-pair-b-label]", b.label);
  set("[data-backend-pair-b-url]", b.url);
  const status = document.querySelector("[data-backends-status]");
  if (status) {
    status.textContent =
      backends.source === "file" ? "Saved on this Home" : "Env defaults";
  }
}

async function saveBackendsFromPanel() {
  const status = document.querySelector("[data-backends-status]");
  const btn = document.querySelector("[data-backends-save]");
  if (btn) {
    btn.disabled = true;
  }
  if (status) {
    status.textContent = "Saving…";
  }
  try {
    const backends = await saveAgentBackends({
      model: document.querySelector("[data-backend-model]")?.value || "",
      pair_a_label: document.querySelector("[data-backend-pair-a-label]")?.value || "",
      pair_a_url: document.querySelector("[data-backend-pair-a-url]")?.value || "",
      pair_b_label: document.querySelector("[data-backend-pair-b-label]")?.value || "",
      pair_b_url: document.querySelector("[data-backend-pair-b-url]")?.value || "",
    });
    fillBackendsPanel(backends);
    if (status) {
      status.textContent = "Saved · gateway validated";
    }
    host?.syncModelMenu?.();
  } catch (error) {
    if (status) {
      status.textContent = String(error?.message || error);
    }
  } finally {
    if (btn) {
      btn.disabled = false;
    }
  }
}

function renderPromptPanel() {
  bindPromptPanelOnce();
  if (!ctx) {
    return;
  }
  const prompt =
    normalizeLiveSystemPrompt(ctx.systemPrompt || DEFAULT_LIVE_SYSTEM_PROMPT);
  const temperature = clampLiveTemperature(ctx.temperature);
  const maxTokens = clampLiveMaxTokens(ctx.maxTokens);
  ctx.systemPrompt = prompt;
  ctx.temperature = temperature;
  ctx.maxTokens = maxTokens;
  const ta = document.querySelector("[data-system-prompt]");
  if (ta && ta.value !== prompt) {
    ta.value = prompt;
  }
  const count = document.querySelector("[data-system-prompt-count]");
  if (count) {
    count.textContent = `${prompt.length}/${MAX_LIVE_SYSTEM_PROMPT_CHARS}`;
  }
  const temp = document.querySelector("[data-agent-temperature]");
  if (temp) {
    temp.value = String(temperature);
  }
  const tokens = document.querySelector("[data-agent-max-tokens]");
  if (tokens) {
    tokens.value = String(maxTokens);
  }
  fillBackendsPanel(getAgentBackendsCache());
  void fetchAgentBackends({ force: true }).then((backends) => {
    if (configureSection === "prompt") {
      fillBackendsPanel(backends);
    }
  });
}

function harnessPageEl() {
  return document.querySelector("[data-harness-page]");
}

export function harnessPageOpen() {
  return harnessPage !== null;
}

function syncSidebarNavActive() {
  for (const row of document.querySelectorAll("[data-sidebar-nav]")) {
    row.classList.toggle("is-active", row.dataset.sidebarNav === harnessPage);
  }
}

function syncConfigureSectionChips() {
  for (const chip of document.querySelectorAll("[data-configure-section]")) {
    const on = chip.dataset.configureSection === configureSection;
    chip.classList.toggle("is-active", on);
    chip.setAttribute("aria-current", on ? "true" : "false");
  }
  for (const panel of document.querySelectorAll("[data-configure-panel]")) {
    panel.hidden = panel.dataset.configurePanel !== configureSection;
  }
}

function renderConfigureOverview() {
  const host = document.querySelector("[data-configure-overview]");
  if (!host) {
    return;
  }
  host.replaceChildren();
  for (const row of getConfigureOverviewSnapshot()) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "agent-configure-manage-row";
    btn.dataset.openConfigureSection = row.section;
    btn.innerHTML =
      `<span class="agent-configure-manage-copy">` +
      `<span class="agent-configure-manage-title"></span>` +
      `<span class="agent-configure-manage-detail"></span>` +
      `</span>` +
      `<span class="agent-configure-manage-status"></span>` +
      `<span class="agent-configure-manage-chevron" aria-hidden="true"></span>`;
    btn.querySelector(".agent-configure-manage-title").textContent = row.title;
    btn.querySelector(".agent-configure-manage-detail").textContent = row.detail;
    const status = btn.querySelector(".agent-configure-manage-status");
    status.textContent = row.status;
    status.title = row.statusTitle || "";
    host.append(btn);
  }
}

function renderMachinePanel() {
  const host = document.querySelector("[data-machine-card]");
  if (!host) {
    return;
  }
  const m = getMachineProfile();
  host.replaceChildren();
  const name = document.createElement("p");
  name.className = "agent-machine-name";
  name.textContent = m.label || "This device";
  const meta = document.createElement("p");
  meta.className = "agent-machine-meta";
  meta.textContent = `${m.platform} · this machine`;
  const specs = document.createElement("p");
  specs.className = "agent-machine-specs";
  specs.textContent = [
    m.gpuLine !== "GPU unknown" ? m.gpuLine : null,
    m.cores ? `${m.cores} cores` : null,
    `${m.memLabel} est`,
  ]
    .filter(Boolean)
    .join(" · ");
  const source = document.createElement("p");
  source.className = "agent-machine-source";
  source.textContent =
    m.source === "runtime-probe"
      ? "Source: runtime probe"
      : "Source: browser estimate — runtime probe later";
  const pill = document.createElement("span");
  pill.className = "agent-machine-pill";
  pill.textContent = "this machine";
  pill.title = "Profile available — not an agent grant";
  host.append(name, meta, specs, source, pill);
}

function renderRuntimePanel() {
  const snap = getRuntimeSnapshot();
  const badges = document.querySelector("[data-runtime-badges]");
  if (badges) {
    badges.replaceChildren();
    for (const label of [
      snap.preview ? "Preview" : "Live",
      snap.locality,
      snap.backend ? `Backend: ${snap.backend}` : "Backend: —",
      `Process: ${snap.process}`,
    ]) {
      const pill = document.createElement("span");
      pill.className = "agent-runtime-badge";
      pill.textContent = label;
      badges.append(pill);
    }
  }
  const dl = document.querySelector("[data-configure-panel='runtime'] [data-page-status]");
  if (dl) {
    renderStatusInto(dl);
  }
  const note = document.querySelector("[data-runtime-note]");
  if (note) {
    note.textContent = snap.note || "";
  }
}

function renderModelsPicks() {
  const host = document.querySelector("[data-models-picks]");
  if (!host) {
    return;
  }
  host.replaceChildren();
  const title = document.createElement("p");
  title.className = "agent-models-section-title";
  title.textContent = "Picks";
  host.append(title);
  for (const group of listPicksByTier()) {
    if (!group.models.length) {
      continue;
    }
    const blurb = document.createElement("p");
    blurb.className = "agent-model-tier-blurb";
    blurb.textContent = group.blurb;
    host.append(blurb);
    for (const model of group.models) {
      const row = document.createElement("div");
      const blocked = model.fit === "blocked";
      row.className = `agent-model-download-row${blocked ? " is-unfit" : ""}`;
      row.dataset.modelId = model.id;
      row.innerHTML =
        `<span class="agent-model-option-copy">` +
        `<span class="agent-model-option-title"></span>` +
        `<span class="agent-model-option-desc"></span>` +
        `</span>` +
        `<button type="button" class="agent-model-download-btn"></button>`;
      row.querySelector(".agent-model-option-title").textContent = model.label;
      row.querySelector(".agent-model-option-desc").textContent = blocked
        ? "Needs more memory"
        : [model.sizeLabel, model.detail].filter(Boolean).join(" · ");
      const action = row.querySelector(".agent-model-download-btn");
      if (model.status === "installed") {
        action.textContent = "Use";
        action.dataset.modelUse = model.id;
        action.title = "Use this model in the composer";
      } else if (blocked) {
        action.textContent = "Blocked";
        action.disabled = true;
        action.title = "Too large for this device estimate";
      } else {
        action.textContent = "Get";
        action.dataset.modelDownload = model.id;
        action.title = "Preview install — asks first; not a Carrier grant";
      }
      host.append(row);
    }
  }
}

function renderConfigureModels() {
  const page = harnessPageEl();
  if (!page) {
    return;
  }
  const device = page.querySelector("[data-models-device]");
  if (device) {
    const hw = getHardwareEstimate();
    device.textContent = hw ? `${hw.deviceLabel} · estimate` : "";
    device.hidden = !hw;
    device.title = "Browser estimate — runtime probes real hardware at Spark/W2";
  }
  const heroHost = page.querySelector("[data-models-hero]");
  if (heroHost) {
    renderModelHero(heroHost);
  }
  const installed = page.querySelector("[data-models-installed]");
  if (installed) {
    buildInstalledModelRows(installed, "No preview models in Mine yet.");
  }
  renderModelsPicks();
  /* Picks carry Get/Use — keep Get list as available-only backup. */
  const discover = page.querySelector("[data-models-discover]");
  if (discover) {
    buildDiscoverModelRows(discover, "All catalog picks are already in the preview list.");
  }
}

function renderUsageHeatmap(daily = [], { live = false } = {}) {
  const wrap = document.createElement("div");
  wrap.className = "agent-usage-heatmap";
  const head = document.createElement("div");
  head.className = "agent-usage-heatmap-head";
  head.innerHTML =
    `<span class="agent-models-section-title">Token activity</span>` +
    `<span class="agent-usage-heatmap-hint">${
      live ? "Daily · on this Home" : "Daily · empty until Live"
    }</span>`;
  const grid = document.createElement("div");
  grid.className = "agent-usage-heatmap-grid";
  grid.setAttribute("role", "img");
  grid.setAttribute(
    "aria-label",
    live
      ? "Token activity heatmap from Live turns on this Home"
      : "Empty heatmap — no Live turns recorded yet",
  );
  const byDate = new Map((daily || []).map((d) => [d.date, d]));
  const end = new Date();
  end.setUTCHours(0, 0, 0, 0);
  /* Align to Sunday start of the first week (Studio-style year grid). */
  const start = new Date(end);
  start.setUTCDate(start.getUTCDate() - 53 * 7 + 1);
  while (start.getUTCDay() !== 0) {
    start.setUTCDate(start.getUTCDate() - 1);
  }
  for (let i = 0; i < 53 * 7; i += 1) {
    const day = new Date(start.getTime() + i * 86400000);
    const key = day.toISOString().slice(0, 10);
    const row = byDate.get(key);
    const tokens = row?.total_tokens || 0;
    const cell = document.createElement("span");
    cell.className = "agent-usage-heatmap-cell";
    cell.dataset.level = tokens <= 0 ? "0" : tokens < 1e3 ? "1" : tokens < 1e4 ? "2" : tokens < 5e4 ? "3" : "4";
    cell.title = `${key} · ${tokens} tokens · ${row?.requests || 0} requests`;
    grid.append(cell);
  }
  const foot = document.createElement("div");
  foot.className = "agent-usage-heatmap-foot";
  foot.innerHTML =
    `<span>Less</span><span class="agent-usage-heatmap-legend" aria-hidden="true"></span><span>More</span>`;
  wrap.append(head, grid, foot);
  return wrap;
}

function renderUsagePage() {
  const host = document.querySelector("[data-usage-page]");
  if (!host) {
    return;
  }
  const u = getUsageSnapshot();
  host.replaceChildren();
  const heroLabel = document.createElement("p");
  heroLabel.className = "agent-usage-kicker";
  heroLabel.textContent = u.preview ? "Tokens used (waiting for Live)" : "Tokens used (this Home)";
  const hero = document.createElement("p");
  hero.className = "agent-usage-hero";
  hero.textContent = String(u.tokens);
  const note = document.createElement("p");
  note.className = "agent-usage-note";
  note.textContent = u.note || "";
  const strip = document.createElement("div");
  strip.className = "agent-usage-strip";
  for (const [label, value] of [
    ["Requests", u.requests],
    ["Active days", u.activeDays],
    [
      "Last latency",
      u.lastLatencyMs
        ? u.lastLatencyMs >= 1000
          ? `${(u.lastLatencyMs / 1000).toFixed(1)}s`
          : `${u.lastLatencyMs}ms`
        : "—",
    ],
  ]) {
    const cell = document.createElement("div");
    cell.innerHTML = `<strong></strong><span></span>`;
    cell.querySelector("strong").textContent = String(value);
    cell.querySelector("span").textContent = label;
    strip.append(cell);
  }
  const heat = renderUsageHeatmap(u.daily, { live: !u.preview });
  const modelsTitle = document.createElement("p");
  modelsTitle.className = "agent-models-section-title";
  modelsTitle.textContent = u.preview ? "Models" : "Most used models";
  const models = document.createElement("p");
  models.className = "agent-usage-models";
  models.textContent = u.byModel?.length ? u.byModel.join(" · ") : "—";
  const locality = document.createElement("p");
  locality.className = "agent-harness-page-foot";
  locality.textContent = `${u.locality} · ${u.note}`;
  if (u.preview) {
    const banner = document.createElement("p");
    banner.className = "agent-preview-banner";
    banner.setAttribute("role", "status");
    banner.textContent =
      "No Live turns yet — Usage stays empty rather than inventing heatmap activity.";
    host.append(banner, heroLabel, hero, note, strip, heat, modelsTitle, models, locality);
    return;
  }
  host.append(heroLabel, hero, note, strip, heat, modelsTitle, models, locality);
}

export function renderHarnessPage() {
  const page = harnessPageEl();
  if (!page || !harnessPage) {
    return;
  }
  const spec = HARNESS_PAGES[harnessPage];
  const title = page.querySelector("[data-page-title]");
  if (title) {
    title.textContent = spec?.title || "";
  }
  const sub = page.querySelector("[data-page-sub]");
  if (sub) {
    sub.textContent = spec?.sub || "";
    sub.hidden = !spec?.sub;
  }
  for (const section of page.querySelectorAll("[data-page-section]")) {
    section.hidden = section.dataset.pageSection !== harnessPage;
  }
  if (harnessPage === "configure") {
    syncConfigureSectionChips();
    if (configureSection === "overview") {
      renderConfigureOverview();
    } else if (configureSection === "machine") {
      renderMachinePanel();
    } else if (configureSection === "models") {
      renderConfigureModels();
    } else if (configureSection === "prompt") {
      renderPromptPanel();
    } else if (configureSection === "tools") {
      const list = page.querySelector("[data-page-tools]");
      if (list) {
        renderToolsInto(list);
      }
    } else if (configureSection === "runtime") {
      renderRuntimePanel();
    }
  } else if (harnessPage === "usage") {
    renderUsagePage();
  }
}

export function openHarnessPage(dest, { section } = {}) {
  /* Back-compat deep links from composer / older tips. */
  if (dest === "models" || dest === "status" || dest === "tools") {
    if (dest === "status") {
      section = section || "runtime";
    } else {
      section = section || dest;
    }
    dest = "configure";
  }
  if (!HARNESS_PAGES[dest]) {
    return;
  }
  harnessPage = dest;
  if (dest === "configure") {
    configureSection = CONFIGURE_SECTIONS.has(section) ? section : configureSection || "overview";
    if (!CONFIGURE_SECTIONS.has(configureSection)) {
      configureSection = "overview";
    }
  }
  host.closeApproveMenu();
  host.closeModelMenu();
  renderHarnessPage();
  const page = harnessPageEl();
  if (page) {
    page.hidden = false;
    page.scrollTop = 0;
  }
  document.querySelector(".agent-harness")?.setAttribute("data-page", dest);
  syncSidebarNavActive();
  if (host.isNarrowHarness()) {
    host.closeHarnessDrawer();
  }
  page?.focus?.({ preventScroll: true });
}

export function openConfigureSection(section) {
  openHarnessPage("configure", { section });
}

export function closeHarnessPage() {
  if (!harnessPage) {
    return;
  }
  harnessPage = null;
  const page = harnessPageEl();
  if (page) {
    page.hidden = true;
  }
  document.querySelector(".agent-harness")?.removeAttribute("data-page");
  syncSidebarNavActive();
}


function workbenchRailEl() {
  return document.querySelector("[data-workbench-rail]");
}

export function syncWorkbenchOpenUi() {
  const rail = workbenchRailEl();
  const open = ctx.workbenchOpen;
  if (rail) {
    rail.hidden = !open;
  }
  document.querySelector(".agent-harness")?.classList.toggle("workbench-open", open);
  for (const btn of document.querySelectorAll("[data-workbench-open]")) {
    btn.hidden = open;
    btn.setAttribute("aria-expanded", open ? "true" : "false");
  }
}

export function openWorkbench({ tab = null, force = false } = {}) {
  if (tab) {
    setWorkbenchTab(tab);
  }
  if (!force && ctx.workbenchUserClosed && ctx.sessionMode !== "build") {
    /* Soft nudge only — badge via title; user chose closed. */
    for (const btn of document.querySelectorAll("[data-workbench-open]")) {
      btn.classList.add("has-nudge");
      btn.title = "Workbench updated — open to view";
    }
    return;
  }
  ctx.workbenchOpen = true;
  ctx.workbenchUserClosed = false;
  for (const btn of document.querySelectorAll("[data-workbench-open]")) {
    btn.classList.remove("has-nudge");
    btn.title = "Workbench — outputs, plan, library";
  }
  syncWorkbenchOpenUi();
  syncWorkbenchPanels();
}

export function closeWorkbench() {
  ctx.workbenchOpen = false;
  ctx.workbenchUserClosed = true;
  syncWorkbenchOpenUi();
}

export function setWorkbenchTab(tabId) {
  const allowed = new Set([
    "outputs",
    "plan",
    "library",
    "diff",
    "browser",
    "terminal",
  ]);
  ctx.workbenchTab = allowed.has(tabId) ? tabId : "outputs";
  if (ctx.workbenchTab === "diff" && ctx.sessionMode !== "build") {
    ctx.workbenchTab = "outputs";
  }
  for (const tab of document.querySelectorAll("[data-workbench-tab]")) {
    const on = tab.dataset.workbenchTab === ctx.workbenchTab;
    tab.classList.toggle("is-active", on);
    tab.setAttribute("aria-selected", on ? "true" : "false");
  }
  for (const panel of document.querySelectorAll("[data-workbench-panel]")) {
    const on = panel.dataset.workbenchPanel === ctx.workbenchTab;
    panel.classList.toggle("is-active", on);
    panel.hidden = !on;
  }
}

function renderStatusInto(el) {
  const snap = getTruthSnapshot();
  el.innerHTML =
    `<div><dt>Locality</dt><dd></dd></div>` +
    `<div><dt>Model</dt><dd></dd></div>` +
    `<div><dt>Context</dt><dd></dd></div>` +
    `<div><dt>Tools</dt><dd></dd></div>` +
    `<div><dt>Hardware</dt><dd></dd></div>` +
    `<div><dt>Mode</dt><dd></dd></div>`;
  const dds = host.querySelectorAll("dd");
  dds[0].textContent = snap.locality;
  dds[1].textContent = snap.modelLabel;
  dds[2].textContent = snap.contextLabel;
  dds[3].textContent = String(snap.toolsLabel || "").replace(/^Tools:\s*/i, "");
  dds[4].textContent = snap.hwLabel;
  dds[5].textContent = `${ctx.sessionMode} · ${ctx.toolMode} intent`;
}

function renderToolsInto(el) {
  el.replaceChildren();
  const manage = el.classList.contains("agent-tools-list-manage");
  for (const cap of listCapabilities()) {
    const li = document.createElement("li");
    li.className = manage ? "agent-tools-manage-row" : "agent-tools-item";
    li.dataset.state = cap.state;
    if (!manage) {
      li.textContent = `${cap.label} · ${cap.state}`;
      el.append(li);
      continue;
    }
    li.innerHTML =
      `<span class="agent-tools-manage-copy">` +
      `<span class="agent-tools-manage-title"></span>` +
      `<span class="agent-tools-manage-desc"></span>` +
      `</span>` +
      `<span class="agent-tools-manage-state"></span>` +
      `<button type="button" class="agent-tools-manage-btn" data-tools-demo-grant>Manage</button>`;
    li.querySelector(".agent-tools-manage-title").textContent = cap.label;
    li.querySelector(".agent-tools-manage-desc").textContent =
      cap.id === "wallet.sign"
        ? "Explicit approval per signature"
        : "Documents the agent may open";
    li.querySelector(".agent-tools-manage-state").textContent = cap.state;
    el.append(li);
  }
}

export function syncWorkbenchPanels() {
  const plan = document.querySelector("[data-plan-markdown]");
  if (plan && document.activeElement !== plan) {
    plan.value = getPlanMarkdown();
  }
  /* Status / Tools left the rail — Settings is the one ops path. */
  renderHarnessPage();
  try {
    host?.renderLibraryWorkbench?.();
  } catch {
    /* harness may bind later */
  }
}

