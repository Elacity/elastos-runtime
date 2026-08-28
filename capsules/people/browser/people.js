const lockedShell = document.getElementById("locked-shell");
const peopleShell = document.getElementById("people-shell");
const statusNode = document.getElementById("people-status");
const profileForm = document.getElementById("profile-form");
const profileInput = document.getElementById("profile-name");
const profileTitle = document.getElementById("profile-title");
const profileDescription = document.getElementById("profile-description");
const profileSubmit = document.getElementById("profile-submit");
const peopleList = document.getElementById("people-list");
const discoveryList = document.getElementById("discovery-list");
const discoveryRequestsList = document.getElementById("discovery-requests-list");
const discoveryStatusNode = document.getElementById("discovery-status");
const discoveryToggleButton = document.getElementById("discovery-toggle");
const discoveryRefreshButton = document.getElementById("discovery-refresh");
const peopleCountNode = document.getElementById("people-count");
const discoveryVisibleCountNode = document.getElementById("discovery-visible-count");
const discoveryRequestsCountNode = document.getElementById("discovery-requests-count");
const pageTitle = document.querySelector(".people-page-title");
const launchParams = new URLSearchParams(window.location.search);
const homeToken = new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token") || "";
const homeParentOrigin = launchParams.get("home_origin") || "";
const DISCOVERY_SCHEMA = "elastos.people.discovery/v1";
const SECTION_TITLES = {
  people: "People",
  discovery: "Discovery",
};

let refreshGeneration = 0;

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

function bindNavigation() {
  for (const button of document.querySelectorAll("[data-section-target]")) {
    button.addEventListener("click", () => {
      activateSection(readText(button.dataset.sectionTarget));
    });
  }
  activateSection("people");
}

// The sidebar reads as tabs, so it behaves as tabs: one section at a time,
// with the page title naming the one you are looking at.
function activateSection(targetId) {
  if (!SECTION_TITLES[targetId]) {
    return;
  }
  for (const item of document.querySelectorAll("[data-section-target]")) {
    const selected = readText(item.dataset.sectionTarget) === targetId;
    item.classList.toggle("active", selected);
    if (selected) {
      item.setAttribute("aria-current", "page");
    } else {
      item.removeAttribute("aria-current");
    }
  }
  for (const section of document.querySelectorAll(".people-section[id]")) {
    section.hidden = section.id !== targetId;
  }
  if (pageTitle) {
    pageTitle.textContent = SECTION_TITLES[targetId];
  }
}

function bindActions() {
  profileForm?.addEventListener("submit", (event) => {
    event.preventDefault();
    saveProfile().catch((error) => showStatus(profileSaveFailureMessage(error), "error"));
  });
  window.addEventListener("message", (event) => {
    if (event.origin !== "null" || event.source !== window.parent) {
      return;
    }
    const message = event.data;
    if (message?.type !== "elastos:menu-command" || readText(message.cmd) !== "refresh") {
      return;
    }
    refreshPeople().catch((error) => {
      showStatus(publicError(error, "People could not load."), "error");
    });
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
}

async function handleAction(button) {
  const action = readText(button.dataset.action);
  if (action === "remove") {
    const contactId = readText(button.dataset.contactId);
    const label = readText(button.dataset.contactName) || "this person";
    if (!contactId || !window.confirm(`Remove ${label} from People?`)) {
      return;
    }
    await mutatePeople(
      "/api/apps/people/contacts/remove",
      { contact_id: contactId },
      "Removed from People.",
      button,
    );
    return;
  }
  if (action === "chat") {
    openChat(readText(button.dataset.conversationId));
    return;
  }
  if (action === "discovery-toggle") {
    const enabled = button.dataset.enabled === "true";
    await mutateDiscovery(
      "/api/apps/people/discovery",
      { enabled: !enabled },
      !enabled ? "Discovery is on." : "Discovery is off.",
      button,
    );
    return;
  }
  if (action === "discovery-refresh") {
    await mutateDiscovery(
      "/api/apps/people/discovery/refresh",
      {},
      "Refresh requested.",
      button,
    );
    return;
  }
  if (action === "discovery-request") {
    const advertisementId = readText(button.dataset.advertisementId);
    if (!advertisementId) {
      throw new Error("Discovery person is unavailable.");
    }
    await mutateDiscovery(
      "/api/apps/people/discovery/requests",
      { advertisement_id: advertisementId },
      "Contact request queued.",
      button,
    );
    return;
  }
  if (action === "open-inbox") {
    window.top.postMessage({
      type: "home:open-target",
      target: "inbox",
      query: {},
      homeToken,
    }, homeParentOrigin);
  }
}

async function refreshPeople({ quiet = false } = {}) {
  const generation = ++refreshGeneration;
  if (!quiet) {
    setBusy(true);
  }
  try {
    const summary = await fetchJson("/api/apps/people/summary");
    if (generation === refreshGeneration) {
      renderSummary(summary);
    }
    return summary;
  } finally {
    if (!quiet) {
      setBusy(false);
    }
  }
}

function renderSummary(summary) {
  const identity = objectValue(summary?.identity);
  const people = objectValue(summary?.people);
  renderProfile(identity);
  renderPeople(people);
  renderDiscovery(summary?.discovery);
}

function renderProfile(identity) {
  const profile = objectValue(identity?.profile);
  const readiness = objectValue(identity?.profile_readiness);
  const displayName = readText(profile.display_name);
  const setupSuggestion = readText(identity?.profile_setup_display_name);
  const readinessStatus = readiness.schema === "elastos.profile.readiness/v1"
    ? readText(readiness.status)
    : "unavailable";
  const hasProfile = readinessStatus === "ready" && Boolean(displayName);
  const unavailable = readinessStatus === "unavailable"
    || (readinessStatus === "ready" && !displayName);
  if (document.activeElement !== profileInput) {
    profileInput.value = hasProfile ? displayName : "";
    profileInput.placeholder = readinessStatus === "setup_required" && setupSuggestion
      ? setupSuggestion
      : "Your name";
  }
  if (profileTitle) {
    profileTitle.textContent = hasProfile
      ? "My Profile"
      : (unavailable ? "Profile unavailable" : "Create your Profile");
  }
  if (profileDescription) {
    profileDescription.textContent = hasProfile
      ? "Shown to people you connect with."
      : (unavailable
        ? "Profile could not be verified. Use System Recovery before continuing."
        : "Your Profile is your signed identity for People and Chat.");
  }
  if (profileSubmit) {
    profileSubmit.textContent = hasProfile
      ? "Save"
      : (unavailable ? "Recovery required" : "Create Profile");
  }
  if (profileForm) {
    profileForm.dataset.profileState = hasProfile ? "saved" : (unavailable ? "unavailable" : "create");
  }
}

function renderPeople(people) {
  const contacts = arrayValue(people?.contacts).filter(isValidContact);
  if (peopleCountNode) {
    peopleCountNode.textContent = `${contacts.length} contact${contacts.length === 1 ? "" : "s"}`;
  }
  peopleList.innerHTML = contacts.length
    ? contacts.map(contactMarkup).join("")
    : emptyMarkup("No contacts yet", "Accepted contacts appear here.");
}

function renderDiscovery(discovery) {
  const safeDiscovery = normalizeDiscoverySummary(discovery);
  const configured = safeDiscovery.configured;
  const enabled = safeDiscovery.enabled;
  if (discoveryVisibleCountNode) {
    discoveryVisibleCountNode.textContent =
      `${safeDiscovery.discoveredPeers.length} visible`;
  }
  if (discoveryRequestsCountNode) {
    discoveryRequestsCountNode.textContent =
      `${safeDiscovery.pendingRequestCount} request${safeDiscovery.pendingRequestCount === 1 ? "" : "s"}`;
  }
  if (discoveryStatusNode) {
    discoveryStatusNode.textContent = safeDiscovery.statusMessage;
  }
  if (discoveryToggleButton) {
    discoveryToggleButton.hidden = !configured;
    discoveryToggleButton.dataset.enabled = enabled ? "true" : "false";
    discoveryToggleButton.textContent = enabled ? "Turn Off" : "Turn On";
  }
  if (discoveryRefreshButton) {
    discoveryRefreshButton.hidden = !configured;
    discoveryRefreshButton.disabled = !configured;
  }

  discoveryList.innerHTML = safeDiscovery.discoveredPeers.length
    ? safeDiscovery.discoveredPeers.map(discoveryPeerMarkup).join("")
    : emptyMarkup(
        safeDiscovery.emptyTitle,
        safeDiscovery.emptyCopy,
      );
  discoveryRequestsList.innerHTML = safeDiscovery.pendingRequestCount > 0
    ? personCard({
      name: `${safeDiscovery.pendingRequestCount} request${safeDiscovery.pendingRequestCount === 1 ? "" : "s"} waiting`,
      details: "<span>Contact requests for your Profile are decided in Inbox.</span>",
      actions: '<button type="button" data-action="open-inbox">Open Inbox</button>',
    })
    : emptyMarkup(
        "No requests",
        configured
          ? "Contact requests for your Profile are decided in Inbox."
          : "Discovery requests are unavailable until collaboration is configured.",
      );
}

function normalizeDiscoverySummary(discovery) {
  if (discovery?.schema !== DISCOVERY_SCHEMA || typeof discovery?.configured !== "boolean") {
    return {
      configured: false,
      enabled: false,
      status: "unavailable",
      statusMessage: "Discovery is unavailable.",
      discoveredPeers: [],
      pendingRequestCount: 0,
      emptyTitle: "Discovery unavailable",
      emptyCopy: "Discovery isn’t available here yet.",
    };
  }
  const configured = discovery.configured;
  const enabled = discovery.enabled === true;
  const status = readText(discovery.status);
  const remainingSeconds = readPositiveInteger(discovery.remaining_seconds);
  const remoteVisibilityRemainingSeconds =
    readPositiveInteger(discovery.remote_visibility_remaining_seconds);
  return {
    configured,
    enabled,
    status,
    statusMessage: normalizeDiscoveryStatusMessage(
      status,
      readText(discovery.status_message),
      remainingSeconds,
      remoteVisibilityRemainingSeconds,
    ),
    discoveredPeers: arrayValue(discovery.discovered_peers).filter((peer) => {
      return peer && typeof peer === "object" && readText(peer.display_name) && readText(peer.advertisement_id);
    }),
    pendingRequestCount: Math.max(0, Number(discovery.request_count || 0)),
    ...discoveryEmptyState(status, configured, enabled),
  };
}

function normalizeDiscoveryStatusMessage(status, runtimeMessage, remainingSeconds, remoteVisibilityRemainingSeconds) {
  if (status === "visible") {
    if (remainingSeconds === null) {
      return "Discovery is on. Visibility lasts up to ten minutes, and both people must be visible at the same time.";
    }
    return `Discovery is on. Visibility lasts up to ten minutes. This window has ${remainingSeconds} seconds left, and both people must be visible at the same time.`;
  }
  if (status === "off_pending_expiry") {
    if (runtimeMessage) {
      return runtimeMessage;
    }
    if (remoteVisibilityRemainingSeconds !== null) {
      return `Discovery is off on this Home. It stopped advertising locally, but may remain visible for another ${remoteVisibilityRemainingSeconds} seconds.`;
    }
    return "Discovery is off on this Home. It stopped advertising locally, but may remain visible for a short time.";
  }
  if (status === "off") {
    return "Discovery is off.";
  }
  return runtimeMessage || "Discovery is unavailable.";
}

function discoveryEmptyState(status, configured, enabled) {
  if (!configured) {
    return {
      emptyTitle: "Discovery unavailable",
      emptyCopy: "Discovery is not configured on this Home.",
    };
  }
  if (status === "visible") {
    return {
      emptyTitle: "No one visible right now",
      emptyCopy: "People appear here only while your visibility windows overlap.",
    };
  }
  if (status === "off_pending_expiry") {
    return {
      emptyTitle: "Visibility is expiring",
      emptyCopy: "Your last visibility window is closing.",
    };
  }
  if (status === "off") {
    return {
      emptyTitle: "Discovery is off",
      emptyCopy: "Turn Discovery on when you want a ten-minute visibility window.",
    };
  }
  if (enabled) {
    return {
      emptyTitle: "Discovery unavailable",
      emptyCopy: "Runtime will retry when Discovery is unavailable.",
    };
  }
  return {
    emptyTitle: "Discovery unavailable",
    emptyCopy: "Discovery isn’t available here yet.",
  };
}

/* The Runtime emits exactly these today. A state this build does not know is
   a state it cannot describe, so it says so rather than picking the friendliest
   reading — presenting an unrecognised relationship as "connected" would tell
   someone they are still connected to a person they may not be. */
const KNOWN_RELATIONSHIPS = new Map([
  ["connected", "Connected"],
  ["conversation", "In conversation"],
  ["requested", "Request sent"],
  ["declined", "Declined"],
  // Removal is symmetric and visible: both sides keep the relationship
  // rather than letting it vanish, and each side sees who ended it.
  ["removed", "Removed"],
  ["removed_you", "No longer connected"],
]);

function relationshipLabel(value) {
  const relationship = readText(value);
  if (!relationship) {
    return "Unknown state";
  }
  return KNOWN_RELATIONSHIPS.get(relationship) || `Unknown state (${relationship})`;
}

/* Presence-derived, tri-state on purpose: true and false are answers from an
   unexpired heartbeat window; absence of the field means the Runtime has no
   presence basis, and no basis is not the same fact as offline. */
function reachabilityLabel(contact) {
  if (contact?.reachable === true) {
    return "Online now";
  }
  if (contact?.reachable === false) {
    return "Offline";
  }
  return "";
}

function contactMarkup(contact) {
  const name = readText(contact?.display_name);
  const relationship = relationshipLabel(contact?.relationship);
  const handle = readText(contact?.handle);
  const details = [relationship, reachabilityLabel(contact), handle && handle !== name ? handle : ""]
    .filter(Boolean)
    .map((value) => `<span>${escapeHtml(value)}</span>`)
    .join("");
  const conversationId = readText(contact?.conversation_id);
  const chat = contact?.can_message === true && conversationId
    ? `<button type="button" data-action="chat" data-conversation-id="${escapeHtml(conversationId)}">Message</button>`
    : "";
  // Remove ends an accepted relationship; the other states have nothing to
  // end. Removed pairs stay visible read-only until a fresh request through
  // Inbox reopens them.
  const remove = readText(contact?.relationship) === "connected"
    ? `<button class="danger" type="button" data-action="remove" data-contact-id="${escapeHtml(readText(contact?.contact_id))}" data-contact-name="${escapeHtml(name)}">Remove</button>`
    : "";
  return personCard({
    name,
    details,
    actions: `${chat}${remove}`,
  });
}

function isValidContact(contact) {
  return Boolean(readText(contact?.contact_id) && readText(contact?.display_name));
}

function discoveryPeerMarkup(person) {
  const name = readText(person.display_name);
  const handle = readText(person.handle);
  return personCard({
    name,
    details: `<span>${escapeHtml(handle && handle !== name ? handle : "Visible now")}</span>`,
    actions: `<button type="button" data-action="discovery-request" data-advertisement-id="${escapeHtml(readText(person.advertisement_id))}">Add contact</button>`,
  });
}

function personCard({ name, details, actions }) {
  const displayName = readText(name);
  if (!displayName) {
    return "";
  }
  return `
    <article class="person-card">
      <div class="person-avatar" aria-hidden="true">${escapeHtml(displayName.slice(0, 1).toUpperCase())}</div>
      <div class="person-copy">
        <h4>${escapeHtml(displayName)}</h4>
        <p class="person-details">${details}</p>
      </div>
      <div class="person-actions">${actions}</div>
    </article>
  `;
}

function emptyMarkup(title, copy) {
  return `<div class="empty-state"><div><h4>${escapeHtml(title)}</h4><p>${escapeHtml(copy)}</p></div></div>`;
}

async function saveProfile() {
  if (profileForm?.dataset.profileState === "recovery_required") {
    window.top.postMessage({
      type: "home:open-target",
      target: "system",
      query: {},
      homeToken,
    }, homeParentOrigin);
    profileForm.dataset.profileState = "retry";
    profileSubmit.textContent = "Retry Create Profile";
    setBusy(false);
    showStatus("Complete Recovery in System, then retry creating your Profile.", "error");
    return;
  }
  const displayName = profileInput.value.trim();
  setBusy(true);
  try {
    await fetchJson("/api/apps/people/profile", {
      method: "POST",
      body: JSON.stringify({ display_name: displayName }),
    });
    showStatus(profileForm?.dataset.profileState === "saved" ? "Profile saved." : "Profile created.", "ok");
    await refreshPeople({ quiet: true });
  } catch (error) {
    if (error.status === 409
      && error.schema === "elastos.people.profile-protection-required/v1"
      && error.resultStatus === "recovery_required"
      && error.actionTarget === "system") {
      profileForm.dataset.profileState = "recovery_required";
      profileTitle.textContent = "Recovery required";
      profileDescription.textContent = readText(error.message)
        || "Open System, choose Security, and download Recovery. Then retry creating your Profile.";
      profileSubmit.textContent = "Open System";
      showStatus(profileDescription.textContent, "error");
      return;
    }
    throw error;
  } finally {
    setBusy(false);
  }
}

function profileSaveFailureMessage(error) {
  const fallback = profileForm?.dataset.profileState === "saved"
    ? "Could not save your Profile. Try again."
    : "Could not create your Profile. Try again.";
  const message = readText(error?.message);
  if (!message || /\b(did:|device|endpoint|route|provider|carrier|schema|projection|launch token|unauthorized|forbidden|[45]\d\d)\b/i.test(message)) {
    return fallback;
  }
  return message;
}

async function mutatePeople(path, body, message, button) {
  button.disabled = true;
  try {
    await fetchJson(path, {
      method: "POST",
      body: JSON.stringify(body),
    });
    showStatus(message, "ok");
    await refreshPeople({ quiet: true });
  } finally {
    button.disabled = false;
  }
}

async function mutateDiscovery(path, body, message, button) {
  button.disabled = true;
  try {
    const discovery = await fetchJson(path, {
      method: "POST",
      body: JSON.stringify(body),
    });
    renderDiscovery(discovery);
    await refreshPeople({ quiet: true });
    showStatus(message, "ok");
  } finally {
    button.disabled = false;
  }
}

function openChat(conversationId) {
  const selector = readText(conversationId);
  if (!selector) {
    throw new Error("Chat is not available for this person yet.");
  }
  window.top.postMessage({
    type: "home:open-target",
    target: "chat-room",
    query: { conversation_id: selector },
    homeToken,
  }, homeParentOrigin);
  showStatus("Opening Chat.", "ok");
}

function setBusy(busy) {
  for (const control of [profileInput, profileSubmit]) {
    if (control) {
      control.disabled = busy
        || profileForm?.dataset.profileState === "unavailable"
        || (control === profileInput && profileForm?.dataset.profileState === "recovery_required");
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
    const error = new Error(readText(payload?.error) || readText(payload?.message) || text || "Request failed.");
    error.status = response.status;
    error.schema = readText(payload?.schema);
    error.resultStatus = readText(payload?.status);
    error.actionTarget = readText(payload?.action_target);
    throw error;
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

function readPositiveInteger(value) {
  const number = Number(value);
  return Number.isInteger(number) && number >= 0 ? number : null;
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
