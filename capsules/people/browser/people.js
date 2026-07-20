const lockedShell = document.getElementById("locked-shell");
const peopleShell = document.getElementById("people-shell");
const statusNode = document.getElementById("people-status");
const profileForm = document.getElementById("profile-form");
const profileInput = document.getElementById("profile-name");
const peopleList = document.getElementById("people-list");
const discoveredList = document.getElementById("discovered-list");
const requestList = document.getElementById("request-list");
const toggleDiscoveryButton = document.getElementById("toggle-discovery");
const refreshDiscoveryButton = document.getElementById("refresh-discovery");
const discoveryCountdown = document.getElementById("discovery-countdown");
const launchParams = new URLSearchParams(window.location.search);
const homeToken = new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token") || "";
const homeParentOrigin = launchParams.get("home_origin") || "";
const AUTO_REFRESH_MS = 15_000;

let currentSummary = null;
let refreshTimer = 0;
let pendingRemoveContactId = null;

announceReady();

boot().catch((error) => {
  showStatus(publicError(error, "People could not load."), "error");
});

async function boot() {
  bindNavigation();
  bindActions();
  if (!homeToken) {
    lockedShell?.classList.remove("hidden");
    peopleShell?.classList.add("hidden");
    return;
  }
  lockedShell?.classList.add("hidden");
  peopleShell?.classList.remove("hidden");
  await refreshPeople();
}

function announceReady() {
  if (homeToken && homeParentOrigin && window.top !== window) {
    window.top.postMessage({ type: "home:app-ready", homeToken }, homeParentOrigin);
    window.top.postMessage({
      type: "home:menu-manifest",
      homeToken,
      menus: [
        {
          title: "File",
          items: [{ label: "Close Window", cmd: "__close-window" }],
        },
        {
          title: "View",
          items: [{ label: "Refresh", cmd: "refresh" }],
        },
      ],
    }, homeParentOrigin);
  }
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
    refreshPeople().catch((error) => showStatus(publicError(error, "People could not load."), "error"));
  }
});

function bindNavigation() {
  for (const button of document.querySelectorAll("[data-section-target]")) {
    button.addEventListener("click", () => {
      const target = readText(button.dataset.sectionTarget);
      document.getElementById(target)?.scrollIntoView({ block: "start", behavior: "smooth" });
      for (const item of document.querySelectorAll("[data-section-target]")) {
        item.classList.toggle("active", item === button);
      }
    });
  }
}

function bindActions() {
  profileForm?.addEventListener("submit", (event) => {
    event.preventDefault();
    saveProfile().catch((error) => showStatus(publicError(error, "Could not save profile."), "error"));
  });
  toggleDiscoveryButton?.addEventListener("click", () => {
    updateDiscovery(currentSummary?.people?.discovery?.enabled !== true)
      .catch((error) => showStatus(publicError(error, "Could not update discovery."), "error"));
  });
  refreshDiscoveryButton?.addEventListener("click", () => {
    refreshDiscovery().catch((error) => showStatus(publicError(error, "Could not refresh discovery."), "error"));
  });
  document.addEventListener("click", (event) => {
    const target = event.target instanceof Element ? event.target.closest("[data-action]") : null;
    if (!(target instanceof HTMLButtonElement)) {
      return;
    }
    handleAction(target).catch((error) => {
      showStatus(publicError(error, "Could not complete that action."), "error");
    });
  });
  window.addEventListener("beforeunload", stopAutoRefresh);
}

async function handleAction(button) {
  const action = readText(button.dataset.action);
  if (action === "request") {
    await mutatePeople("/api/apps/people/discovery/requests", {
      peer_id: readText(button.dataset.peerId),
    }, "Request sent.", button);
    return;
  }
  if (action === "accept") {
    const requestId = readText(button.dataset.requestId);
    if (!requestId) {
      throw new Error("This request is no longer available.");
    }
    await mutatePeople(
      `/api/apps/people/discovery/requests/${encodeURIComponent(requestId)}/accept`,
      null,
      "Request accepted.",
      button,
    );
    return;
  }
  if (action === "remove") {
    const contactId = readText(button.dataset.contactId);
    if (!contactId || !currentSummary) {
      return;
    }
    pendingRemoveContactId = contactId;
    renderPeople(currentSummary);
    return;
  }
  if (action === "remove-cancel") {
    pendingRemoveContactId = null;
    if (currentSummary) {
      renderPeople(currentSummary);
    }
    return;
  }
  if (action === "remove-confirm") {
    const contactId = readText(button.dataset.contactId);
    if (!contactId) {
      return;
    }
    pendingRemoveContactId = null;
    await mutatePeople(
      "/api/apps/people/contacts/remove",
      { contact_id: contactId },
      "Removed from People.",
      button,
    );
    return;
  }
  if (action === "chat") {
    openChat(readText(button.dataset.contactRoute));
  }
}

async function refreshPeople({ quiet = false } = {}) {
  if (!quiet) {
    setBusy(true);
  }
  try {
    const summary = await fetchJson("/api/apps/people/summary");
    renderPeople(summary);
  } finally {
    if (!quiet) {
      setBusy(false);
    }
  }
}

function renderPeople(summary) {
  currentSummary = summary;
  const identity = objectValue(summary?.identity);
  const people = objectValue(summary?.people);
  const contacts = arrayValue(people.contacts);
  if (
    pendingRemoveContactId
    && !contacts.some((contact) => readText(contact?.contact_id) === pendingRemoveContactId)
  ) {
    pendingRemoveContactId = null;
  }
  const discovery = objectValue(people.discovery);
  const peers = filterDiscoveredPeople(arrayValue(discovery.discovered_peers), contacts);
  const requests = arrayValue(discovery.requests).filter(requestIsVisible);

  if (document.activeElement !== profileInput) {
    profileInput.value = profileDisplayName(identity);
  }
  peopleList.innerHTML = contacts.length
    ? contacts.map(contactMarkup).join("")
    : emptyMarkup("No people yet", "Turn on Discovery to find another ElastOS home and send a request.");
  discoveredList.innerHTML = peers.length
    ? peers.map(discoveredPeerMarkup).join("")
    : emptyMarkup("Nobody nearby", "Turn on Discovery to find people on other homes nearby.");
  requestList.innerHTML = requests.length
    ? requests.map(requestMarkup).join("")
    : emptyMarkup("No requests", "Requests to add people will appear here.");

  const remaining = remainingSeconds(discovery);
  const enabled = discovery.enabled === true && remaining > 0;
  toggleDiscoveryButton.textContent = enabled ? "Stop" : "Turn On";
  discoveryCountdown.hidden = !enabled;
  discoveryCountdown.textContent = enabled ? `Discoverable for ${remainingText(remaining)}` : "";
  scheduleAutoRefresh(enabled);
}

function contactMarkup(contact) {
  const name = displayName(contact, "Person");
  const contactId = readText(contact?.contact_id);
  const relationship = readText(contact?.relationship) || "connected";
  const handle = readText(contact?.handle);
  const device = readText(contact?.device_label);
  const details = [relationship, handle !== name ? handle : "", device !== name ? device : ""]
    .filter(Boolean)
    .map((value) => `<span>${escapeHtml(value)}</span>`)
    .join("");
  const route = readText(contact?.route);
  const confirming = pendingRemoveContactId === contactId && Boolean(contactId);
  const chat = !confirming && contact?.can_message === true && route
    ? `<button type="button" data-action="chat" data-contact-route="${escapeHtml(route)}">Chat</button>`
    : "";
  const actions = confirming
    ? ""
    : `${chat}<button class="danger" type="button" data-action="remove" data-contact-id="${escapeHtml(contactId)}">Remove</button>`;
  const confirm = confirming
    ? {
      message: `Remove ${name} from People?`,
      contactId,
    }
    : null;
  return personCard({
    name,
    details,
    actions,
    confirm,
  });
}

function discoveredPeerMarkup(peer) {
  const name = displayName(peer, "Visible person");
  const handle = readText(peer?.handle);
  const status = readText(peer?.status) || "visible";
  const peerId = readText(peer?.peer_id);
  return personCard({
    name,
    details: `<span>${escapeHtml(handle && handle !== name ? handle : "Discoverable")}</span><span>${escapeHtml(status)}</span>`,
    actions: `<button type="button" data-action="request" data-peer-id="${escapeHtml(peerId)}" ${peerId ? "" : "disabled"}>Request</button>`,
  });
}

function requestMarkup(request) {
  const name = displayName(request, "Person");
  const status = readText(request?.status) || "requested";
  const requestId = readText(request?.request_id);
  const action = status === "incoming"
    ? `<button type="button" data-action="accept" data-request-id="${escapeHtml(requestId)}" ${requestId ? "" : "disabled"}>Accept</button>`
    : '<span class="requested">Requested</span>';
  return personCard({
    name,
    details: `<span>${escapeHtml(status)}</span>`,
    actions: action,
  });
}

function personCard({ name, details, actions, confirm = null }) {
  const confirmMarkup = confirm
    ? `<div class="person-confirm" role="alert">
        <p>${escapeHtml(confirm.message)}</p>
        <div class="person-confirm-actions">
          <button type="button" data-action="remove-cancel">Cancel</button>
          <button class="danger" type="button" data-action="remove-confirm" data-contact-id="${escapeHtml(confirm.contactId)}">Remove</button>
        </div>
      </div>`
    : "";
  return `
    <article class="person-card${confirm ? " is-confirming" : ""}">
      <div class="person-avatar" aria-hidden="true">${escapeHtml(name.slice(0, 1).toUpperCase() || "E")}</div>
      <div class="person-copy">
        <h4>${escapeHtml(name)}</h4>
        <p>${details}</p>
      </div>
      <div class="person-actions">${actions}</div>
      ${confirmMarkup}
    </article>
  `;
}

function emptyMarkup(title, copy) {
  return `<div class="empty-state"><div><h4>${escapeHtml(title)}</h4><p>${escapeHtml(copy)}</p></div></div>`;
}

async function saveProfile() {
  const handle = profileInput.value.trim();
  setBusy(true);
  try {
    await fetchJson("/api/apps/people/profile-card", {
      method: "POST",
      body: JSON.stringify({ handle }),
    });
    showStatus("Profile saved.", "ok");
    await refreshPeople({ quiet: true });
  } finally {
    setBusy(false);
  }
}

async function updateDiscovery(enabled) {
  setBusy(true);
  try {
    await fetchJson("/api/apps/people/discovery", {
      method: "POST",
      body: JSON.stringify({ enabled }),
    });
    showStatus(enabled ? "Discovery is on." : "Discovery is off.", "ok");
    await refreshPeople({ quiet: true });
  } finally {
    setBusy(false);
  }
}

async function refreshDiscovery({ quiet = false } = {}) {
  if (!quiet) {
    setBusy(true);
  }
  try {
    await fetchJson("/api/apps/people/discovery/refresh", { method: "POST" });
    if (!quiet) {
      showStatus("Discovery refreshed.", "ok");
    }
    await refreshPeople({ quiet: true });
  } finally {
    if (!quiet) {
      setBusy(false);
    }
  }
}

async function mutatePeople(path, body, message, button) {
  button.disabled = true;
  try {
    await fetchJson(path, {
      method: "POST",
      body: body === null ? undefined : JSON.stringify(body),
    });
    showStatus(message, "ok");
    await refreshPeople({ quiet: true });
  } finally {
    button.disabled = false;
  }
}

function openChat(route) {
  let target = "";
  try {
    const url = new URL(route, window.location.origin);
    const match = url.pathname.match(/^\/apps\/([^/]+)\/?$/);
    target = match ? decodeURIComponent(match[1]) : "";
  } catch (_error) {
    target = "";
  }
  if (target !== "chat-room") {
    throw new Error("Chat is not available for this person yet.");
  }
  window.top.postMessage({
    type: "home:open-target",
    target,
    query: {},
    homeToken,
  }, homeParentOrigin);
  showStatus("Opening Chat.", "ok");
}

function scheduleAutoRefresh(enabled) {
  stopAutoRefresh();
  if (!enabled) {
    return;
  }
  refreshTimer = window.setTimeout(() => {
    refreshDiscovery({ quiet: true }).catch(() => {
      scheduleAutoRefresh(true);
    });
  }, AUTO_REFRESH_MS);
}

function stopAutoRefresh() {
  window.clearTimeout(refreshTimer);
  refreshTimer = 0;
}

function filterDiscoveredPeople(peers, contacts) {
  const connected = new Set();
  for (const contact of contacts) {
    const device = readText(contact?.device_label);
    if (device) {
      connected.add(device);
    }
    const route = readText(contact?.route);
    if (route.startsWith("elastos://peer/")) {
      connected.add(route.slice("elastos://peer/".length));
    }
  }
  return peers.filter((peer) => !connected.has(readText(peer?.peer_id)));
}

function requestIsVisible(request) {
  const status = readText(request?.status) || "requested";
  return status === "incoming" || status === "requested";
}

function profileDisplayName(identity) {
  const profile = objectValue(identity?.profile_card);
  return readText(profile.display_name) || readText(identity?.handle);
}

function displayName(person, fallback) {
  const profile = objectValue(person?.profile_card);
  const name = readText(profile.display_name) || readText(person?.display_name);
  const handle = readText(profile.handle) || readText(person?.handle);
  const peer = readText(person?.device_label) || readText(person?.peer_id);
  return name && name !== "ElastOS user" ? name : handle || peer || fallback;
}

function remainingSeconds(discovery) {
  const value = Number(discovery?.remaining_seconds || 0);
  return Number.isFinite(value) && value > 0 ? Math.ceil(value) : 0;
}

function remainingText(seconds) {
  if (seconds >= 60) {
    return `${Math.ceil(seconds / 60)} min`;
  }
  return `${Math.max(0, seconds)} sec`;
}

function setBusy(busy) {
  for (const control of [profileInput, profileForm?.querySelector("button"), toggleDiscoveryButton, refreshDiscoveryButton]) {
    if (control) {
      control.disabled = busy;
    }
  }
}

function showStatus(message, tone = "muted") {
  statusNode.textContent = message;
  statusNode.dataset.tone = tone;
  statusNode.hidden = !message;
}

async function fetchJson(path, options = {}) {
  const headers = new Headers(options.headers || {});
  headers.set("x-elastos-home-token", homeToken);
  if (typeof options.body === "string") {
    headers.set("content-type", "application/json");
  }
  const response = await fetch(path, { ...options, headers, credentials: "same-origin" });
  const text = await response.text();
  let payload = null;
  try {
    payload = text ? JSON.parse(text) : null;
  } catch (_error) {
    payload = null;
  }
  if (!response.ok) {
    throw new Error(readText(payload?.error) || readText(payload?.message) || text || "Request failed.");
  }
  return payload;
}

function publicError(error, fallback) {
  const message = readText(error?.message);
  return !message || /\b(schema|projection|provider|capability|launch token|hostcall|unauthorized|forbidden|[45]\d\d)\b/i.test(message)
    ? fallback
    : message;
}

function objectValue(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function arrayValue(value) {
  return Array.isArray(value) ? value : [];
}

function readText(value) {
  return typeof value === "string" ? value.trim() : "";
}

function escapeHtml(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;");
}
