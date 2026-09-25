/* Agent Settings page (Models · Prompt).
   Bound from agent-harness.js (ctx + host).
   UI ≠ authority (Principle 16): pages never mint grants. */

import {
  DEFAULT_LIVE_SYSTEM_PROMPT,
  MAX_LIVE_SYSTEM_PROMPT_CHARS,
  clampLiveMaxTokens,
  normalizeLiveSystemPrompt,
  normalizeAgentNotes,
  probeLiveInference,
  selectedLiveOffer,
} from "./agent-live.js";

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
};
export const CONFIGURE_SECTIONS = new Set(["models", "prompt"]);
let harnessPage = null;
let configureSection = "models";

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
    if (target?.matches?.("[data-agent-notes]")) {
      ctx.agentNotes = normalizeAgentNotes(target.value);
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
  });
}

function renderPromptPanel() {
  bindPromptPanelOnce();
  if (!ctx) {
    return;
  }
  const prompt =
    normalizeLiveSystemPrompt(ctx.systemPrompt || DEFAULT_LIVE_SYSTEM_PROMPT);
  const maxTokens = clampLiveMaxTokens(ctx.maxTokens);
  ctx.systemPrompt = prompt;
  ctx.maxTokens = maxTokens;
  const ta = document.querySelector("[data-system-prompt]");
  if (ta && ta.value !== prompt) {
    ta.value = prompt;
  }
  const count = document.querySelector("[data-system-prompt-count]");
  if (count) {
    count.textContent = `${prompt.length}/${MAX_LIVE_SYSTEM_PROMPT_CHARS}`;
  }
  const notes = normalizeAgentNotes(ctx.agentNotes || "");
  ctx.agentNotes = notes;
  const notesEl = document.querySelector("[data-agent-notes]");
  if (notesEl && notesEl.value !== notes) {
    notesEl.value = notes;
  }
  const tokens = document.querySelector("[data-agent-max-tokens]");
  if (tokens) {
    tokens.value = String(maxTokens);
  }
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

function renderModelSelectionFacts() {
  const factsHost = document.querySelector("[data-model-selection-facts]");
  if (!factsHost) {
    return;
  }
  const facts = selectedLiveOffer()?.selectionFacts;
  factsHost.replaceChildren();
  const rows = Array.isArray(facts?.detailRows) ? facts.detailRows : [];
  if (!facts || !rows.length) {
    factsHost.hidden = true;
    return;
  }
  for (const row of rows) {
    const dt = document.createElement("dt");
    dt.textContent = row.term;
    const dd = document.createElement("dd");
    dd.textContent = row.value;
    factsHost.append(dt, dd);
  }
  factsHost.hidden = false;
}

/* Models = the offers this Home advertises, nothing else. Re-probe on open so
   a service installed a moment ago shows up without a reload. */
function renderConfigureModels() {
  const page = harnessPageEl();
  if (!page) {
    return;
  }
  const label = page.querySelector("[data-current-model-label]");
  if (label) label.textContent = selectedLiveOffer()?.label || "No model selected";
  renderModelSelectionFacts();
  void probeLiveInference({ force: true }).then(() => {
    if (harnessPage === "configure" && configureSection === "models") {
      if (label) label.textContent = selectedLiveOffer()?.label || "No model selected";
      host.syncModelTrigger?.();
      renderModelSelectionFacts();
    }
  });
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
    if (configureSection === "models") {
      renderConfigureModels();
    } else if (configureSection === "prompt") {
      renderPromptPanel();
    }
  }
}

export function openHarnessPage(dest, { section } = {}) {
  /* Back-compat deep links from composer / older tips. */
  if (dest === "models") {
    section = section || dest;
    dest = "configure";
  }
  if (!HARNESS_PAGES[dest]) {
    return;
  }
  harnessPage = dest;
  if (dest === "configure") {
    configureSection = CONFIGURE_SECTIONS.has(section) ? section : configureSection || "models";
    if (!CONFIGURE_SECTIONS.has(configureSection)) {
      configureSection = "models";
    }
  }
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
