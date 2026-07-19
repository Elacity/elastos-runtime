import {
  shellState,
  fetchJson,
  mountGlyph,
  allVisibleTargets,
  desktopObjects,
} from "./shell-core.js?v=home-20260719e";
import { openFileObject } from "./shell-surface.js?v=home-20260719e";
import {
  openTarget,
  focusWindow,
  browserWindowEntries,
  sortWindowEntriesByZOrder,
  browserWindowDisplayTitle,
} from "./shell-windows.js?v=home-20260719e";

/* Spotlight: shell-wide search (macOS anatomy — dimmed backdrop, centered
 * floating bar, grouped results that grow beneath it). Searches everything
 * the shell already knows without new server sweeps:
 *
 *   windows      shellState.windows (switch to)         in memory
 *   apps         summary.targets (launch/focus)         in memory
 *   documents    documents summary (titles + uris)      one cheap POST, cached
 *   files        summary.desktop_objects                in memory
 *   library      summary.targets (target_kind=object)   in memory
 *   people       summary.people.contacts                in memory
 *
 * Focus stays in the field the whole time (real Spotlight behavior): arrows
 * move the aria-activedescendant selection, Enter activates, Esc closes.
 */

/* Bound by bindSpotlight() once the lazy GUI template is in the DOM. */
let spotlight = null;
let spotlightInput = null;
let spotlightResults = null;

const DOCUMENTS_CACHE_MS = 60_000;

const spotlightState = {
  invoker: null,
  query: "",
  results: [],
  index: -1,
  documents: [],
  documentsFetchedAt: 0,
};

/* ---- Sources ---- */

function windowResults() {
  return sortWindowEntriesByZOrder(browserWindowEntries()).map((entry) => ({
    kind: "window",
    group: "Windows",
    title: browserWindowDisplayTitle(entry) || entry.title || entry.id,
    detail: "Switch to window",
    glyphTarget: entry.targetId || entry.id,
    activate: () => focusWindow(entry.id, { moveFocus: true }),
  }));
}

function targetResults() {
  const recents = shellState.recentTargetIds || [];
  return allVisibleTargets(shellState.currentSummary).map((target) => ({
    kind: target.target_kind === "object" ? "library" : "app",
    group: target.target_kind === "object" ? "Library" : "Applications",
    title: target.title || target.target,
    detail: target.target_kind === "object" ? "Library item" : "Application",
    glyphTarget: target.target,
    extraText: target.description || "",
    recentRank: recents.indexOf(target.target),
    activate: () => openTarget(target.target),
  }));
}

function documentResults() {
  return spotlightState.documents.map((doc) => {
    const title = doc.title || doc.file_name || "Untitled";
    const uri = doc.working_copy_uri || doc.document_uri || "";
    return {
      kind: "document",
      group: "Documents",
      title,
      detail: "Document",
      glyphTarget: "documents",
      activate: () =>
        openTarget("documents", {
          query: uri ? { objectUri: uri, uri, name: title } : {},
        }),
    };
  });
}

function fileResults() {
  return desktopObjects(shellState.currentSummary).map((object) => ({
    kind: "file",
    group: "Files",
    title: object.name,
    detail: object.kind === "directory" ? "Folder" : "Desktop file",
    glyphTarget: object.kind === "directory" ? "library" : "documents",
    extraText: object.mime || "",
    activate: () => openFileObject(object),
  }));
}

function peopleResults() {
  const contacts = shellState.currentSummary?.people?.contacts;
  if (!Array.isArray(contacts)) {
    return [];
  }
  return contacts
    .filter((contact) => contact && typeof contact.display_name === "string")
    .map((contact) => ({
      kind: "person",
      group: "People",
      title: contact.display_name,
      detail: "Contact",
      glyphTarget: "people",
      extraText: typeof contact.handle === "string" ? contact.handle : "",
      activate: () => openTarget("people"),
    }));
}

/* Documents titles come from ONE cheap summary op (no bodies), cached for a
   minute; a fetch failure just means no Documents group this open. */
async function refreshDocuments() {
  if (Date.now() - spotlightState.documentsFetchedAt < DOCUMENTS_CACHE_MS) {
    return;
  }
  spotlightState.documentsFetchedAt = Date.now();
  try {
    const response = await fetchJson("/api/provider/documents/summary", {
      method: "POST",
      body: JSON.stringify({}),
    });
    const docs = response?.data?.documents || response?.documents;
    spotlightState.documents = Array.isArray(docs) ? docs : [];
  } catch (_error) {
    spotlightState.documents = [];
  }
  if (spotlightOpen() && spotlightInput.value.trim() !== "") {
    runSearch(spotlightInput.value);
  }
}

/* ---- Matching + ranking ---- */

const GROUP_ORDER = ["Windows", "Applications", "Documents", "Files", "Library", "People"];

function matchScore(item, query) {
  const title = item.title.toLowerCase();
  if (title.startsWith(query)) {
    return 3;
  }
  if (title.includes(query)) {
    return 2;
  }
  if ((item.extraText || "").toLowerCase().includes(query)) {
    return 1;
  }
  return 0;
}

function collectResults(rawQuery) {
  const query = rawQuery.trim().toLowerCase();
  if (query === "") {
    return [];
  }
  const all = [
    ...windowResults(),
    ...targetResults(),
    ...documentResults(),
    ...fileResults(),
    ...peopleResults(),
  ];
  const matched = [];
  for (const item of all) {
    const score = matchScore(item, query);
    if (score > 0) {
      matched.push({ ...item, score });
    }
  }
  matched.sort((a, b) => {
    const groupDelta = GROUP_ORDER.indexOf(a.group) - GROUP_ORDER.indexOf(b.group);
    if (groupDelta !== 0) {
      return groupDelta;
    }
    if (a.score !== b.score) {
      return b.score - a.score;
    }
    // Recently used apps float within their group.
    const aRecent = a.recentRank ?? -1;
    const bRecent = b.recentRank ?? -1;
    if (aRecent !== bRecent) {
      if (aRecent === -1) return 1;
      if (bRecent === -1) return -1;
      return aRecent - bRecent;
    }
    return a.title.localeCompare(b.title);
  });
  return matched.slice(0, 40);
}

/* ---- Rendering ---- */

function renderResults() {
  spotlightResults.replaceChildren();
  let lastGroup = null;
  spotlightState.results.forEach((item, index) => {
    if (item.group !== lastGroup) {
      lastGroup = item.group;
      const header = document.createElement("div");
      header.className = "spotlight-section";
      header.setAttribute("role", "presentation");
      header.textContent = item.group;
      spotlightResults.appendChild(header);
    }
    const row = document.createElement("div");
    row.className = "spotlight-item";
    row.id = `spotlight-item-${index}`;
    row.setAttribute("role", "option");
    row.setAttribute("aria-selected", index === spotlightState.index ? "true" : "false");
    const glyph = document.createElement("span");
    glyph.className = "spotlight-glyph app-glyph";
    glyph.setAttribute("aria-hidden", "true");
    mountGlyph(glyph, item.glyphTarget);
    const title = document.createElement("span");
    title.className = "spotlight-title";
    title.textContent = item.title;
    const detail = document.createElement("span");
    detail.className = "spotlight-detail";
    detail.textContent = item.detail;
    row.append(glyph, title, detail);
    row.addEventListener("pointerdown", (event) => {
      event.preventDefault();
      activateResult(index);
    });
    row.addEventListener("pointerenter", () => {
      setSelection(index);
    });
    spotlightResults.appendChild(row);
  });
  // A searched-for miss deserves an answer, not a silent panel (the empty
  // prompt state, before typing, stays quiet).
  if (spotlightState.results.length === 0 && spotlightState.query) {
    const empty = document.createElement("div");
    empty.className = "spotlight-empty";
    empty.setAttribute("role", "presentation");
    empty.textContent = `No results for \u201C${spotlightState.query}\u201D`;
    spotlightResults.appendChild(empty);
  }
  spotlightResults.hidden =
    spotlightState.results.length === 0 && !spotlightState.query;
  spotlightInput.setAttribute(
    "aria-expanded",
    spotlightState.results.length > 0 ? "true" : "false",
  );
  syncActiveDescendant();
}

function setSelection(index) {
  if (index === spotlightState.index) {
    return;
  }
  spotlightState.index = index;
  for (const row of spotlightResults.querySelectorAll(".spotlight-item")) {
    const rowIndex = Number(row.id.slice("spotlight-item-".length));
    row.setAttribute("aria-selected", rowIndex === index ? "true" : "false");
  }
  syncActiveDescendant();
}

function syncActiveDescendant() {
  const active =
    spotlightState.index >= 0 ? `spotlight-item-${spotlightState.index}` : "";
  spotlightInput.setAttribute("aria-activedescendant", active);
  if (active) {
    document.getElementById(active)?.scrollIntoView({ block: "nearest" });
  }
}

function runSearch(rawQuery) {
  spotlightState.query = String(rawQuery || "").trim();
  spotlightState.results = collectResults(rawQuery);
  spotlightState.index = spotlightState.results.length > 0 ? 0 : -1;
  renderResults();
}

function activateResult(index) {
  const item = spotlightState.results[index];
  if (!item) {
    return;
  }
  hideSpotlight({ restoreFocus: false });
  item.activate();
}

/* ---- Open / close ---- */

export function spotlightOpen() {
  return spotlight ? !spotlight.hidden : false;
}

export function showSpotlight() {
  if (!spotlight || spotlightOpen()) {
    spotlightInput?.focus();
    return;
  }
  spotlightState.invoker =
    document.activeElement && document.activeElement !== document.body
      ? document.activeElement
      : null;
  spotlight.hidden = false;
  spotlight.setAttribute("aria-hidden", "false");
  spotlightInput.value = "";
  runSearch("");
  spotlightInput.focus();
  refreshDocuments();
}

export function hideSpotlight({ restoreFocus = true } = {}) {
  if (!spotlight || spotlight.hidden) {
    return;
  }
  spotlight.hidden = true;
  spotlight.setAttribute("aria-hidden", "true");
  spotlightState.query = "";
  spotlightState.results = [];
  spotlightState.index = -1;
  spotlightResults.replaceChildren();
  spotlightInput.value = "";
  if (restoreFocus) {
    spotlightState.invoker?.focus?.();
  }
  spotlightState.invoker = null;
}

export function toggleSpotlight() {
  if (spotlightOpen()) {
    hideSpotlight();
  } else {
    showSpotlight();
  }
}

/* ---- Events ---- */

/* Called by the home-gui facade once ensureHomeGuiDom() has instantiated the
   lazy GUI template — these nodes do not exist at module-evaluation time. */
export function bindSpotlight() {
  if (spotlight) {
    return;
  }
  spotlight = document.querySelector("#spotlight");
  spotlightInput = document.querySelector("#spotlight-input");
  spotlightResults = document.querySelector("#spotlight-results");
  if (!spotlight || !spotlightInput) {
    return;
  }
  spotlightInput.addEventListener("input", () => {
    runSearch(spotlightInput.value);
  });
  spotlight.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      hideSpotlight();
      return;
    }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const count = spotlightState.results.length;
      if (count === 0) {
        return;
      }
      const delta = event.key === "ArrowDown" ? 1 : -1;
      setSelection((spotlightState.index + delta + count) % count);
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      if (spotlightState.index >= 0) {
        activateResult(spotlightState.index);
      }
      return;
    }
    // Focus lives in the field for the whole session (real Spotlight): Tab
    // neither leaves the dialog nor moves focus.
    if (event.key === "Tab") {
      event.preventDefault();
    }
  });
  spotlight.addEventListener("pointerdown", (event) => {
    if (!event.target.closest(".spotlight-panel")) {
      hideSpotlight({ restoreFocus: false });
    }
  });
}
