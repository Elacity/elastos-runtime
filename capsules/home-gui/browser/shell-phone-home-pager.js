/* Phone Home pages (shell-phone-home.js has the model). The pages scroll
   sideways under the thumb, one page per swipe (native scroll snap), and the
   dots under them are the button twin: tap a dot, or use the arrow keys on
   the focused dots, to turn the page. Left of page 1 sits the Assistant, the
   Agent Space that is far left of the Space ring on every size: settling on
   it opens the Assistant, and page 1 is waiting when the Assistant closes.
   docs/HOME_MOBILE.md. */

// A scroll that has stopped moving this long has settled on a page (the
// fallback where `scrollend` is missing).
const PAGE_SETTLE_MS = 140;
// The Assistant room takes the floor well inside this; if it never came, the
// grid slides back to page 1 rather than leave a dead page up.
const AGENT_OPEN_GRACE_MS = 1600;

let pagesEl = null;
let dotsEl = null;
let hooks = null;
let pageCount = 1;
let hasAgentPage = false;
let settleTimer = 0;
let agentTimer = 0;
let agentOpening = false;

function reducedMotion() {
  return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches === true;
}

function gridPages() {
  return pagesEl ? [...pagesEl.querySelectorAll(":scope > .phone-home-page[data-page]")] : [];
}

// The page geometry lives in the phone stylesheet (--phone-home-*), read
// here so the page size follows the screen: 4 or 6 columns, as many rows as
// fit. Off the phone the grid is not shown and everything is one page.
export function phoneHomePerPage() {
  if (!pagesEl) {
    return Number.POSITIVE_INFINITY;
  }
  const style = getComputedStyle(pagesEl);
  const read = (name) => parseFloat(style.getPropertyValue(name)) || 0;
  const columns = Math.round(read("--phone-home-cols"));
  const tile = read("--phone-home-tile-h");
  const gap = read("--phone-home-row-gap");
  const height = pagesEl.clientHeight - read("--phone-home-pad-top") - read("--phone-home-pad-bottom");
  if (columns < 1 || tile <= 0 || height <= 0) {
    return Number.POSITIVE_INFINITY;
  }
  return columns * Math.max(1, Math.floor((height + gap) / (tile + gap)));
}

// The grid page in view: 0 is page 1, -1 is the Assistant page.
export function currentPhoneHomePage() {
  if (!pagesEl || pagesEl.clientWidth === 0) {
    return 0;
  }
  const index = Math.round(pagesEl.scrollLeft / pagesEl.clientWidth) - (hasAgentPage ? 1 : 0);
  return Math.max(hasAgentPage ? -1 : 0, Math.min(index, pageCount - 1));
}

export function phoneHomePageCount() {
  return pageCount;
}

export function phoneHomePageNode(index) {
  return gridPages()[index] || null;
}

// An empty page past the last, for a drag heading there; the next render
// keeps it only if something landed on it.
export function appendPhoneHomePage() {
  const node = document.createElement("div");
  node.className = "phone-home-page";
  node.dataset.page = String(pageCount);
  node.setAttribute("role", "group");
  node.setAttribute("aria-label", `Page ${pageCount + 1} of ${pageCount + 1}`);
  pagesEl.appendChild(node);
  pageCount += 1;
  renderDots();
}

export function showPhoneHomePage(index, { smooth = true } = {}) {
  const pages = gridPages();
  const page = pages[Math.max(0, Math.min(index, pages.length - 1))];
  if (!page) {
    return;
  }
  pagesEl.scrollTo({ left: page.offsetLeft, behavior: smooth && !reducedMotion() ? "smooth" : "auto" });
  syncDots(Math.max(0, Math.min(index, pages.length - 1)));
}

// `buildAgentPage` returns the Assistant page, or null when this Home has no
// Assistant. The page in view survives a re-render.
export function renderPhoneHomePages(pages, { buildTile, buildAgentPage }) {
  if (!pagesEl) {
    return;
  }
  const keep = pagesEl.childElementCount > 0 ? Math.max(0, currentPhoneHomePage()) : 0;
  const nodes = [];
  const agentPage = buildAgentPage();
  if (agentPage) {
    nodes.push(agentPage);
  }
  for (const [index, page] of pages.entries()) {
    const node = document.createElement("div");
    node.className = "phone-home-page";
    node.dataset.page = String(index);
    node.setAttribute("role", "group");
    node.setAttribute("aria-label", `Page ${index + 1} of ${pages.length}`);
    for (const app of page) {
      node.appendChild(buildTile(app));
    }
    nodes.push(node);
  }
  pagesEl.replaceChildren(...nodes);
  hasAgentPage = Boolean(agentPage);
  pageCount = Math.max(1, pages.length);
  renderDots();
  showPhoneHomePage(Math.min(keep, pageCount - 1), { smooth: false });
}

function renderDots() {
  if (!dotsEl) {
    return;
  }
  const dots = [];
  for (let index = 0; index < pageCount; index += 1) {
    const dot = document.createElement("span");
    dot.className = "phone-home-dot";
    dots.push(dot);
  }
  dotsEl.replaceChildren(...dots);
  const single = pageCount < 2;
  dotsEl.dataset.single = single ? "true" : "false";
  dotsEl.setAttribute("aria-hidden", single ? "true" : "false");
  dotsEl.tabIndex = single ? -1 : 0;
  dotsEl.setAttribute("aria-valuemax", String(pageCount));
  syncDots(Math.max(0, currentPhoneHomePage()));
}

function syncDots(current) {
  if (!dotsEl) {
    return;
  }
  for (const [index, dot] of [...dotsEl.children].entries()) {
    dot.classList.toggle("is-current", index === current);
  }
  dotsEl.setAttribute("aria-valuenow", String(current + 1));
  dotsEl.setAttribute("aria-valuetext", `Page ${current + 1} of ${pageCount}`);
}

function onDotsPointer(event) {
  const dots = [...dotsEl.children];
  if (dots.length < 2) {
    return;
  }
  let nearest = 0;
  let nearestDistance = Number.POSITIVE_INFINITY;
  for (const [index, dot] of dots.entries()) {
    const box = dot.getBoundingClientRect();
    const distance = Math.abs(event.clientX - (box.left + box.width / 2));
    if (distance < nearestDistance) {
      nearest = index;
      nearestDistance = distance;
    }
  }
  showPhoneHomePage(nearest);
}

function onDotsKey(event) {
  const current = Math.max(0, currentPhoneHomePage());
  const next = {
    ArrowLeft: current - 1,
    ArrowDown: current - 1,
    ArrowRight: current + 1,
    ArrowUp: current + 1,
    Home: 0,
    End: pageCount - 1,
  }[event.key];
  if (next === undefined) {
    return;
  }
  event.preventDefault();
  showPhoneHomePage(Math.max(0, Math.min(next, pageCount - 1)));
}

function settled() {
  window.clearTimeout(settleTimer);
  settleTimer = 0;
  if (!hasAgentPage || currentPhoneHomePage() >= 0 || agentOpening) {
    return;
  }
  if (hooks.editing()) {
    showPhoneHomePage(0);
    return;
  }
  agentOpening = true;
  hooks.openAgent();
  window.clearTimeout(agentTimer);
  agentTimer = window.setTimeout(() => {
    agentOpening = false;
    if (currentPhoneHomePage() < 0) {
      showPhoneHomePage(0);
    }
  }, AGENT_OPEN_GRACE_MS);
}

// Once the Assistant room covers the floor the grid turns back to page 1
// unseen, so closing the Assistant lands on the Home, not on its doorway.
function watchAgentRoom() {
  new MutationObserver(() => {
    if (!agentOpening || !document.body.classList.contains("assistant-space-active")) {
      return;
    }
    agentOpening = false;
    window.clearTimeout(agentTimer);
    showPhoneHomePage(0, { smooth: false });
  }).observe(document.body, { attributes: true, attributeFilter: ["class"] });
}

/**
 * @param {{
 *   openAgent: () => void,
 *   editing: () => boolean,
 *   resized: () => void,
 * }} dependencies
 */
export function bindPhoneHomePager(dependencies) {
  hooks = dependencies;
  pagesEl = document.querySelector("#phone-home-pages");
  dotsEl = document.querySelector("#phone-home-dots");
  if (!pagesEl || !dotsEl) {
    return;
  }
  pagesEl.addEventListener("scroll", () => {
    syncDots(Math.max(0, currentPhoneHomePage()));
    window.clearTimeout(settleTimer);
    settleTimer = window.setTimeout(settled, PAGE_SETTLE_MS);
  }, { passive: true });
  pagesEl.addEventListener("scrollend", settled);
  dotsEl.addEventListener("click", onDotsPointer);
  dotsEl.addEventListener("keydown", onDotsKey);
  watchAgentRoom();
  let lastSize = "";
  new ResizeObserver(() => {
    const size = `${pagesEl.clientWidth}x${pagesEl.clientHeight}`;
    if (size !== lastSize) {
      lastSize = size;
      hooks.resized();
    }
  }).observe(pagesEl);
}
