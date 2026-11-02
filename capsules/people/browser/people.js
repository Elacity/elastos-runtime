import { createHomeClipboardClient } from "/apps/home/home-clipboard-client.js?v=home-20260726a";

const lockedShell = document.getElementById("locked-shell");
const peopleShell = document.getElementById("people-shell");
const statusNode = document.getElementById("people-status");
const profileForm = document.getElementById("profile-form");
const profileInput = document.getElementById("profile-name");
const profileTitle = document.getElementById("profile-title");
const profileDescription = document.getElementById("profile-description");
const profileSubmit = document.getElementById("profile-submit");
const profileIdSection = document.getElementById("profile-id");
const profileIdValue = document.getElementById("profile-id-value");
const profileIdCopy = document.getElementById("profile-id-copy");
const addContactForm = document.getElementById("add-contact-form");
const addContactInput = document.getElementById("add-contact-id");
const addContactSubmit = document.getElementById("add-contact-submit");
const addContactResult = document.getElementById("add-contact-result");
const addContactInbox = document.getElementById("add-contact-inbox");
const peopleList = document.getElementById("people-list");
const peopleRequests = document.getElementById("people-requests");
const peopleRequestsCount = document.getElementById("people-requests-count");
const peopleUndelivered = document.getElementById("people-undelivered");
const discoveryPeople = document.getElementById("discovery-people");
const discoveryList = document.getElementById("discovery-list");
const discoveryStatusNode = document.getElementById("discovery-status");
const discoveryNoteNode = document.getElementById("discovery-note");
const discoveryToggleButton = document.getElementById("discovery-toggle");
const discoveryRefreshButton = document.getElementById("discovery-refresh");
const peopleCountNode = document.getElementById("people-count");
const discoveryVisibleCountNode = document.getElementById("discovery-visible-count");
const pageTitle = document.querySelector(".people-page-title");
const launchParams = new URLSearchParams(window.location.search);
const homeToken = new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token") || "";
const homeParentOrigin = launchParams.get("home_origin") || "";
const DISCOVERY_SCHEMA = "elastos.people.discovery/v1";
const SECTION_TITLES = {
  people: "People",
  profile: "Profile",
  discovery: "Public",
};

const homeClipboard = createHomeClipboardClient({
  targetId: "people",
  homeOrigin: homeParentOrigin,
  homeToken,
});
let localProfileId = "";
let sendingContact = false;
let refreshGeneration = 0;
let profileDraftDirty = false;
let profileDraftValue = "";
let currentContacts = [];
let pendingRemoveId = "";
let removingContactId = "";
let currentSection = "";
let sectionChosen = false;

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
  homeClipboard.start();
  await refreshPeople();
}

function prefillContactFromLaunch() {
  const did = launchParams.get("did");
  if (!localProfileId || sendingContact || !addContactInput || !did || did !== did.trim()
      || !/^did:key:z[1-9A-HJ-NP-Za-km-z]+$/.test(did)) return;
  sectionChosen = true;
  activateSection("people");
  addContactInput.value = did;
  addContactInput.scrollIntoView({ block: "center" });
  addContactInput.focus();
  launchParams.delete("did");
}

function receivePeopleNavigation(event) {
  const data = event.data;
  if (event.source !== window.parent || event.origin !== "null"
      || data?.type !== "elastos.people.navigate/v1" || data.homeToken !== homeToken
      || !data.query || Object.keys(data.query).join(",") !== "did"
      || typeof data.query.did !== "string") return;
  launchParams.set("did", data.query.did);
  prefillContactFromLaunch();
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
      sectionChosen = true;
      activateSection(readText(button.dataset.sectionTarget));
    });
  }
  activateSection("people");
}

// The sidebar reads as tabs, so it behaves as tabs: one section at a time,
// with the page title naming the one you are looking at. A status line
// belongs to the page it came from.
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
  if (currentSection && currentSection !== targetId) {
    showStatus("");
  }
  currentSection = targetId;
}

function bindActions() {
  window.addEventListener("message", receivePeopleNavigation);
  profileIdCopy?.addEventListener("click", async () => {
    if (!localProfileId || profileIdCopy.disabled) return;
    profileIdCopy.disabled = true;
    try {
      await homeClipboard.writeText(localProfileId, { purpose: "profile.id" });
      showStatus("DID copied.", "ok");
    } catch (_error) {
      showStatus("DID could not be copied. Try again.", "error");
    } finally {
      profileIdCopy.disabled = false;
    }
  });
  addContactForm?.addEventListener("submit", (event) => {
    event.preventDefault();
    sendContactRequest();
  });
  profileForm?.addEventListener("submit", (event) => {
    event.preventDefault();
    saveProfile().catch((error) => showStatus(profileSaveFailureMessage(error), "error"));
  });
  profileInput?.addEventListener("input", () => {
    profileDraftDirty = true;
    profileDraftValue = profileInput.value;
  });
  // Home pushes people.changed when contacts or Discovery state change, so
  // "Connecting…" and newly visible people update without a manual Refresh.
  window.addEventListener("message", (event) => {
    if (event.origin !== homeParentOrigin || event.source !== window.top) {
      return;
    }
    const message = event.data || {};
    if (message.type !== "elastos:runtime-events" || !Array.isArray(message.events)) {
      return;
    }
    if (message.events.some((item) => item?.scope === "people" || item?.kind === "people.changed")) {
      refreshPeople({ quiet: true }).catch(() => {});
    }
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
  if (action === "remove" || action === "cancel-remove" || action === "confirm-remove") {
    const contactId = readText(button.dataset.contactId);
    if (removingContactId || !contactId) {
      return;
    }
    if (action === "cancel-remove") {
      if (contactId !== pendingRemoveId) return;
      pendingRemoveId = "";
      renderPeople({ contacts: currentContacts });
      focusContactAction("remove", contactId);
      return;
    }
    const contact = currentContacts.find((item) => readText(item.contact_id) === contactId
      && readText(item.relationship) === "connected");
    if (!contact || (action === "confirm-remove" && contactId !== pendingRemoveId)) {
      return;
    }
    if (action === "remove") {
      pendingRemoveId = contactId;
      renderPeople({ contacts: currentContacts });
      focusContactAction("cancel-remove", contactId);
      return;
    }
    removingContactId = contactId;
    renderPeople({ contacts: currentContacts });
    try {
      await mutatePeople(
        "/api/apps/people/contacts/remove",
        { contact_id: contactId },
        "Removed from People.",
        button,
      );
    } finally {
      removingContactId = "";
      renderPeople({ contacts: currentContacts });
    }
    return;
  }
  if (action === "chat") {
    openChat(readText(button.dataset.conversationId));
    return;
  }
  if (action === "discovery-toggle") {
    const enabled = button.dataset.enabled === "true";
    await mutateDiscovery("/api/apps/people/discovery", { enabled: !enabled }, button);
    return;
  }
  if (action === "discovery-refresh") {
    await mutateDiscovery("/api/apps/people/discovery/refresh", {}, button);
    return;
  }
  if (action === "discovery-request") {
    const advertisementId = readText(button.dataset.advertisementId);
    if (!advertisementId) {
      throw new Error("This person is no longer public.");
    }
    await mutateDiscovery(
      "/api/apps/people/discovery/requests",
      { advertisement_id: advertisementId },
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
  renderProfileId(summary?.profile_id);
  renderPeople(people);
  renderDiscovery(summary?.discovery);
  // Without a Profile there is nothing to do on People yet, so the first
  // load opens Profile; after that the person picks the page.
  if (!sectionChosen) {
    sectionChosen = true;
    activateSection(profileForm?.dataset.profileState === "saved" ? "people" : "profile");
  }
}

function renderProfileId(profileId) {
  localProfileId = readText(profileId);
  if (profileIdValue) profileIdValue.textContent = localProfileId;
  if (profileIdSection) profileIdSection.hidden = !localProfileId;
  if (addContactForm) addContactForm.hidden = !localProfileId;
  prefillContactFromLaunch();
}

async function sendContactRequest() {
  const profileId = readText(addContactInput?.value);
  if (!localProfileId || sendingContact || !profileId) return;
  sendingContact = true;
  addContactSubmit.disabled = true;
  addContactInput.disabled = true;
  addContactInbox.hidden = true;
  addContactResult.textContent = "";
  try {
    const result = await fetchJson("/api/apps/people/contacts/request", {
      method: "POST", body: JSON.stringify({ profile_id: profileId }),
    });
    const message = readText(result?.message);
    if (!message) throw new Error("Could not confirm the request. Try again.");
    addContactResult.textContent = message;
    addContactInbox.hidden = result.status !== "pending_from_them";
    addContactInput.value = "";
    // A refresh failure does not change the confirmed request outcome.
    await refreshPeople({ quiet: true }).catch(() => {});
  } catch (error) {
    addContactResult.textContent = [400, 409].includes(error?.status)
      ? readText(error.message) || "Could not send the request. Try again."
      : publicError(error, "Could not send the request. Try again.");
  } finally {
    sendingContact = false;
    addContactSubmit.disabled = false;
    addContactInput.disabled = false;
    prefillContactFromLaunch();
  }
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
  const createState = !hasProfile && !unavailable;
  if (hasProfile) {
    profileDraftDirty = false;
    profileDraftValue = displayName;
  } else if (!profileDraftDirty) {
    profileDraftValue = createState ? setupSuggestion : "";
  }
  if (document.activeElement !== profileInput) {
    profileInput.value = hasProfile ? displayName : profileDraftValue;
    profileInput.placeholder = "Your name";
  }
  // A saved Profile needs no heading or explanation: the page title says
  // Profile and the field holds the name.
  if (profileTitle) {
    profileTitle.textContent = hasProfile
      ? ""
      : (unavailable ? "Profile unavailable" : "Create your Profile");
    profileTitle.hidden = hasProfile;
  }
  if (profileDescription) {
    profileDescription.textContent = hasProfile
      ? ""
      : (unavailable
        ? "Profile could not be verified. Use System Recovery before continuing."
        : "Choose the name people see. Public stays off until you turn it on.");
    profileDescription.hidden = hasProfile;
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
  currentContacts = contacts;
  if (!contacts.some((contact) => readText(contact.contact_id) === pendingRemoveId
    && readText(contact.relationship) === "connected")) {
    pendingRemoveId = "";
  }
  if (peopleCountNode) {
    peopleCountNode.textContent = String(contacts.length);
  }
  // One note covers every request that cannot be delivered; the rows only
  // say Not delivered.
  if (peopleUndelivered) {
    peopleUndelivered.hidden = !contacts.some((contact) => readText(contact.relationship) === "requested"
      && readText(contact.delivery_detail));
  }
  peopleList.innerHTML = contacts.length
    ? contacts.map(contactMarkup).join("")
    : emptyMarkup("No contacts yet.");
}

function renderDiscovery(discovery) {
  const safeDiscovery = normalizeDiscoverySummary(discovery);
  const configured = safeDiscovery.configured;
  const enabled = safeDiscovery.enabled;
  const peers = safeDiscovery.discoveredPeers;
  if (discoveryVisibleCountNode) {
    discoveryVisibleCountNode.textContent = String(peers.length);
  }
  if (discoveryStatusNode) {
    discoveryStatusNode.textContent = safeDiscovery.statusMessage;
  }
  // The listing renews itself while you are signed in, so on is plain on.
  if (discoveryNoteNode) {
    discoveryNoteNode.hidden = !(enabled && ["visible", "connecting"].includes(safeDiscovery.status));
  }
  if (discoveryToggleButton) {
    discoveryToggleButton.hidden = !configured;
    discoveryToggleButton.dataset.enabled = enabled ? "true" : "false";
    discoveryToggleButton.textContent = enabled ? "Turn Off" : "Turn On";
    discoveryToggleButton.disabled = !configured || safeDiscovery.status === "community_left";
  }
  if (discoveryRefreshButton) {
    discoveryRefreshButton.hidden = !configured;
    discoveryRefreshButton.disabled = !configured || safeDiscovery.status === "community_left";
  }
  // The list shows while you are public; otherwise the status line says why
  // there is no one to show.
  const showList = peers.length > 0 || safeDiscovery.status === "visible";
  if (discoveryPeople) {
    discoveryPeople.hidden = !showList;
  }
  discoveryList.innerHTML = peers.length
    ? peers.map(discoveryPeerMarkup).join("")
    : (showList ? emptyMarkup("No one else is public right now.") : "");
  // Requests to you are answered in Inbox; People only points there.
  const requests = safeDiscovery.pendingRequestCount;
  if (peopleRequests) {
    peopleRequests.hidden = requests === 0;
  }
  if (peopleRequestsCount) {
    peopleRequestsCount.textContent = requests
      ? `${requests} contact request${requests === 1 ? "" : "s"} waiting.`
      : "";
  }
}

function normalizeDiscoverySummary(discovery) {
  if (discovery?.schema !== DISCOVERY_SCHEMA || typeof discovery?.configured !== "boolean") {
    return {
      configured: false,
      enabled: false,
      status: "unavailable",
      statusMessage: "Public isn’t available here yet.",
      discoveredPeers: [],
      pendingRequestCount: 0,
    };
  }
  const status = readText(discovery.status);
  return {
    configured: discovery.configured,
    enabled: discovery.enabled === true,
    status,
    statusMessage: PUBLIC_STATUS_TEXT[status] || readText(discovery.status_message)
      || "Public isn’t available right now.",
    discoveredPeers: arrayValue(discovery.discovered_peers).filter((peer) => {
      return peer && typeof peer === "object" && readText(peer.display_name) && readText(peer.advertisement_id);
    }),
    pendingRequestCount: Math.max(0, Number(discovery.request_count || 0)),
  };
}

// No countdown: a listing renews while you are signed in, and the seconds
// left after turning off are nothing a person can act on.
const PUBLIC_STATUS_TEXT = {
  visible: "Public is on.",
  connecting: "Public is on.",
  off_pending_expiry: "Public is off. You may stay listed for a few minutes.",
  off: "Public is off.",
};

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
  const relationship = readText(contact?.relationship);
  // Each row is a name and one short status. A request the Community node
  // cannot deliver says Not delivered. Presence is shown only for a
  // connected contact.
  const status = relationship === "requested"
    ? (readText(contact.delivery_detail) ? "Not delivered" : "Request sent")
    : (relationship === "connected" ? reachabilityLabel(contact) : relationshipLabel(relationship));
  // A request sent by DID before this Home holds their signed Profile has no
  // name to show, so the status stands in its place.
  const unnamed = !name && relationship === "requested";
  const details = status && !unnamed ? `<span>${escapeHtml(status)}</span>` : "";
  const conversationId = readText(contact?.conversation_id);
  const chat = contact?.can_message === true && conversationId
    ? `<button type="button" data-action="chat" data-conversation-id="${escapeHtml(conversationId)}">Message</button>`
    : "";
  // Remove ends an accepted relationship; the other states have nothing to
  // end. Removed pairs stay visible read-only until a fresh request through
  // Inbox reopens them.
  const contactId = readText(contact.contact_id);
  const disabled = removingContactId ? " disabled" : "";
  const confirming = pendingRemoveId === contactId;
  const remove = readText(contact?.relationship) === "connected" && !confirming
    ? `<button class="danger" type="button" data-action="remove" data-contact-id="${escapeHtml(contactId)}"${disabled}>Remove</button>`
    : "";
  const confirmation = confirming ? `
    <div class="person-confirm" role="group" aria-label="Remove contact">
      <p>Remove ${escapeHtml(name)} from People?</p>
      <div class="person-confirm-actions">
        <button type="button" data-action="cancel-remove" data-contact-id="${escapeHtml(contactId)}"${disabled}>Cancel</button>
        <button class="danger" type="button" data-action="confirm-remove" data-contact-id="${escapeHtml(contactId)}"${disabled}>Remove</button>
      </div>
    </div>` : "";
  return personCard({
    name: unnamed ? status : name,
    initial: unnamed ? "" : name,
    details,
    actions: confirming ? "" : `${chat}${remove}`,
    confirmation,
  });
}

function isValidContact(contact) {
  return Boolean(readText(contact?.contact_id)
    && (readText(contact?.display_name) || readText(contact?.relationship) === "requested"));
}

// Runtime stamps each public person with their relationship; only someone
// with none yet (`visible`) can be added.
const DISCOVERY_RELATIONSHIP_TEXT = {
  contact: "Contact",
  requested: "Request sent",
  pending: "Wants to add you",
};

function discoveryPeerMarkup(person) {
  const name = readText(person.display_name);
  const status = readText(person.status);
  const addable = status === "visible";
  // Add contact says what can be done; others get one relationship word.
  const detail = !addable && Object.hasOwn(DISCOVERY_RELATIONSHIP_TEXT, status)
    ? DISCOVERY_RELATIONSHIP_TEXT[status]
    : "";
  return personCard({
    name,
    initial: name,
    details: detail ? `<span>${escapeHtml(detail)}</span>` : "",
    actions: addable
      ? `<button type="button" data-action="discovery-request" data-advertisement-id="${escapeHtml(readText(person.advertisement_id))}">Add contact</button>`
      : "",
  });
}

function focusContactAction(action, contactId) {
  const buttons = peopleList?.querySelectorAll("[data-action][data-contact-id]") || [];
  [...buttons].find((button) => button.dataset.action === action
    && button.dataset.contactId === contactId)?.focus();
}

function personCard({ name, initial, details, actions, confirmation = "" }) {
  const displayName = readText(name);
  if (!displayName) {
    return "";
  }
  return `
    <article class="person-card${confirmation ? " is-confirming" : ""}">
      <div class="person-avatar" aria-hidden="true">${escapeHtml(readText(initial).slice(0, 1).toUpperCase())}</div>
      <div class="person-copy">
        <h4>${escapeHtml(displayName)}</h4>
        ${details ? `<p class="person-details">${details}</p>` : ""}
      </div>
      <div class="person-actions">${actions}</div>
      ${confirmation}
    </article>
  `;
}

function emptyMarkup(text) {
  return `<p class="empty-state">${escapeHtml(text)}</p>`;
}

async function saveProfile() {
  const displayName = profileInput.value.trim();
  setBusy(true);
  try {
    await fetchJson("/api/apps/people/profile", {
      method: "POST",
      body: JSON.stringify({ display_name: displayName }),
    });
    showStatus(profileForm?.dataset.profileState === "saved" ? "Profile saved." : "Profile created.", "ok");
    await refreshPeople({ quiet: true });
    if (homeParentOrigin && window.top && window.top !== window) {
      window.top.postMessage({ type: "home:refresh-summary", homeToken }, homeParentOrigin);
    }
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

async function mutateDiscovery(path, body, button) {
  button.disabled = true;
  try {
    const discovery = await fetchJson(path, {
      method: "POST",
      body: JSON.stringify(body),
    });
    renderDiscovery(discovery);
    await refreshPeople({ quiet: true });
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
}

function setBusy(busy) {
  for (const control of [profileInput, profileSubmit]) {
    if (control) {
      control.disabled = busy
        || profileForm?.dataset.profileState === "unavailable";
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
  return ![400, 409].includes(error?.status) || !message || /\b(schema|projection|provider|capability|launch token|hostcall|unauthorized|forbidden|[45]\d\d)\b/i.test(message)
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
