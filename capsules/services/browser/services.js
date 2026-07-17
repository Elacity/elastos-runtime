const lockedShell = document.getElementById("locked-shell");
const servicesShell = document.getElementById("services-shell");
const refreshButton = document.getElementById("refresh-services");
const statusNode = document.getElementById("services-status");
const mineCountNode = document.getElementById("mine-count");
const othersCountNode = document.getElementById("others-count");
const mineServicesList = document.getElementById("mine-services-list");
const otherServicesList = document.getElementById("other-services-list");
const launchParams = new URLSearchParams(window.location.search);
const homeToken = new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token") || "";
const homeOrigin = launchParams.get("home_origin") || "";
const EXIT_SERVICE_KIND = "remote_exit";
const BROWSER_ENGINE_SERVICE_KIND = "browser_engine";
const CONFIGURED_REMOTE_EXIT_SOURCE = "configured_remote_exit";
const VISIBLE_SERVICE_KINDS = new Set([BROWSER_ENGINE_SERVICE_KIND, EXIT_SERVICE_KIND]);

let currentServices = null;
let pendingServiceAction = null;

announceReady();

boot().catch((error) => {
  showStatus(error.message || "Services failed to load.", "error");
  lockedShell?.classList.remove("hidden");
  servicesShell?.classList.add("hidden");
});

async function boot() {
  bindNavigation();
  bindActions();
  if (!homeToken) {
    lockedShell?.classList.remove("hidden");
    servicesShell?.classList.add("hidden");
    return;
  }
  lockedShell?.classList.add("hidden");
  servicesShell?.classList.remove("hidden");
  await refreshServices();
}

function announceReady() {
  if (homeToken && homeOrigin && window.top !== window) {
    window.top.postMessage({ type: "home:app-ready", homeToken }, homeOrigin);
  }
}

function bindNavigation() {
  for (const button of document.querySelectorAll("[data-section-target]")) {
    button.addEventListener("click", () => {
      const target = button.getAttribute("data-section-target") || "";
      activateServicesSection(target);
    });
  }
}

function activateServicesSection(target, options = {}) {
  const targetId = readText(target);
  if (!targetId) {
    return;
  }
  document.getElementById(targetId)?.scrollIntoView({
    block: "start",
    behavior: options.behavior || "smooth",
  });
  for (const item of document.querySelectorAll("[data-section-target]")) {
    item.classList.toggle("active", item.getAttribute("data-section-target") === targetId);
  }
}

function bindActions() {
  refreshButton?.addEventListener("click", () => {
    refreshServices().catch((error) => showStatus(error.message || "Could not refresh Services.", "error"));
  });
  document.addEventListener("click", (event) => {
    const target = event.target instanceof Element ? event.target : event.target?.parentElement;
    if (!target) {
      return;
    }
    const confirmButton = target.closest("[data-confirm-service-action]");
    if (confirmButton) {
      handlePendingServiceAction(confirmButton)
        .catch((error) => showStatus(error.message || "Could not update Service.", "error"));
      return;
    }
    const serviceToggle = target.closest("[data-service-offer-id]");
    if (serviceToggle) {
      handleServiceOfferAction(serviceToggle)
        .catch((error) => showStatus(error.message || "Could not update Service.", "error"));
    }
  });
}

async function refreshServices() {
  setBusy(true);
  showStatus("Refreshing Services...", "muted");
  try {
    const services = await fetchJson("/api/apps/services/summary", {
      headers: shellHeaders(),
    });
    renderServices(services);
    showStatus("Services updated.", "ok");
  } finally {
    setBusy(false);
  }
}

function renderServices(services) {
  currentServices = services;
  const localOffers = visibleServiceOffers(services?.local_offers);
  const remoteOffers = visibleServiceOffers(services?.remote_offers);
  const availableLocalOffers = visibleServiceOffers(services?.available_local_offers);
  const availableRemoteOffers = visibleServiceOffers(services?.available_remote_offers);
  mineCountNode.textContent = String(localOffers.length);
  othersCountNode.textContent = String(remoteOffers.length);
  mineServicesList.innerHTML = renderServiceSection({
    selected: localOffers,
    available: availableLocalOffers,
    source: "mine",
    selectedTitle: "Shared",
    availableTitle: "Available on this device",
    emptySelected: "No Services are shared.",
    emptyAvailable: "No Browser Engine or Browser Exit service is installed on this device.",
  });
  otherServicesList.innerHTML = renderServiceSection({
    selected: remoteOffers,
    available: availableRemoteOffers,
    source: "others",
    selectedTitle: "Subscribed",
    availableTitle: "Available from People",
    emptySelected: "No Services from others are subscribed.",
    emptyAvailable: "No Browser Engine or Browser Exit services are available from People you are connected with.",
  });
}

function renderServiceSection({ selected, available, source, selectedTitle, availableTitle, emptySelected, emptyAvailable }) {
  return `
    <div class="service-subsection">
      <div class="service-subsection-title">${escapeHtml(selectedTitle)}</div>
      ${selected.length
        ? orderedServiceOffers(selected).map((offer) => renderServiceCard(offer, source, true)).join("")
        : renderEmptyCard(emptySelected)}
    </div>
    <div class="service-subsection">
      <div class="service-subsection-title">${escapeHtml(availableTitle)}</div>
      ${available.length
        ? orderedServiceOffers(available).map((offer) => renderServiceCard(offer, source, false)).join("")
        : renderEmptyCard(emptyAvailable)}
    </div>
  `;
}

function renderServiceCard(offer, source, selected) {
  const title = serviceTitle(offer, source, selected);
  const copy = serviceCopy(offer, source, selected);
  const status = serviceStatus(offer, source, selected);
  const grantRequired = offer?.grant_required === true;
  const offerId = readText(offer?.offer_id);
  const readOnly = isReadOnlyServiceOffer(offer);
  const statusTone = serviceStatusTone(offer, source);
  const primaryAction = serviceActionLabel(source, selected, offer);
  const pending = pendingServiceAction?.offerId === offerId && pendingServiceAction?.section === source;
  return `
    <article class="service-card">
      <div class="service-card-main">
        <div>
          <div class="service-title-row">
            <h3 class="service-title">${escapeHtml(title)}</h3>
            <span class="status-badge" data-tone="${statusTone}">${escapeHtml(status)}</span>
            ${grantRequired && !selected ? '<span class="status-badge" data-tone="warn">Approval needed</span>' : ""}
          </div>
          <p class="service-copy">${escapeHtml(copy)}</p>
        </div>
        <div class="service-actions">
          ${offerId && !readOnly ? `<button class="pc2-btn" type="button" data-service-offer-id="${escapeHtml(offerId)}" data-service-section="${source}" data-service-selected="${selected ? "false" : "true"}">${primaryAction}</button>` : ""}
          ${readOnly ? '<span class="status-badge" data-tone="ok">Managed by config</span>' : ""}
        </div>
      </div>
      ${pending ? renderInlineConfirmation(primaryAction) : ""}
    </article>
  `;
}

function renderInlineConfirmation(actionLabel) {
  return `
    <div class="service-confirm" role="alert">
      <p>${escapeHtml(confirmMessage(actionLabel))}</p>
      <div class="service-confirm-actions">
        <button class="pc2-btn" type="button" data-confirm-service-action="cancel">Cancel</button>
        <button class="pc2-btn pc2-btn-danger" type="button" data-confirm-service-action="apply">${escapeHtml(actionLabel)}</button>
      </div>
    </div>
  `;
}

function confirmMessage(actionLabel) {
  return actionLabel === "Stop sharing"
    ? "Stop sharing this Service?"
    : "Remove this Service from your subscriptions?";
}

async function handleServiceOfferAction(button) {
  if (!(button instanceof HTMLButtonElement)) {
    return;
  }
  const selected = button.dataset.serviceSelected === "true";
  if (!selected) {
    requestServiceActionConfirmation(button);
    return;
  }
  await setServiceOfferSelection(button);
}

function requestServiceActionConfirmation(button) {
  const offerId = readText(button.dataset.serviceOfferId);
  const section = readText(button.dataset.serviceSection);
  if (!offerId || !section) {
    showStatus("This service could not be selected. Refresh and try again.", "error");
    return;
  }
  pendingServiceAction = {
    offerId,
    section,
    selected: false,
  };
  renderServices(currentServices);
  showStatus("Confirm the change in the Service card.", "muted");
}

async function handlePendingServiceAction(button) {
  const action = readText(button.getAttribute("data-confirm-service-action"));
  if (action === "cancel") {
    pendingServiceAction = null;
    renderServices(currentServices);
    showStatus("No changes made.", "muted");
    return;
  }
  if (action !== "apply" || !pendingServiceAction) {
    return;
  }
  const pending = pendingServiceAction;
  pendingServiceAction = null;
  await setServiceOfferSelection(pending);
}

async function setServiceOfferSelection(button) {
  const offerId = readText(button?.dataset?.serviceOfferId || button?.offerId);
  const section = readText(button?.dataset?.serviceSection || button?.section);
  const selected = button?.dataset?.serviceSelected === "true" || button?.selected === true;
  if (!offerId || !section) {
    throw new Error("This service could not be selected. Refresh and try again.");
  }
  setBusy(true);
  if (button instanceof HTMLButtonElement) {
    button.disabled = true;
  }
  showStatus(selectionProgressMessage(section, selected), "muted");
  try {
    const services = await fetchJson("/api/apps/services/offers", {
      method: "POST",
      headers: shellHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ offer_id: offerId, section, selected }),
    });
    renderServices(services);
    showStatus(selectionDoneMessage(section, selected), "ok");
  } finally {
    setBusy(false);
    if (button instanceof HTMLButtonElement) {
      button.disabled = false;
    }
  }
}

function selectionProgressMessage(section, selected) {
  if (section === "mine") {
    return selected ? "Sharing service..." : "Stopping service sharing...";
  }
  return selected ? "Sending service request..." : "Removing service...";
}

function selectionDoneMessage(section, selected) {
  if (section === "mine") {
    return selected ? "Service shared with People." : "Service is private.";
  }
  return selected ? "Service request sent." : "Service removed.";
}

function visibleServiceOffers(offers) {
  return Array.isArray(offers)
    ? offers.filter((offer) => VISIBLE_SERVICE_KINDS.has(readText(offer?.service_kind)))
    : [];
}

function serviceTitle(offer, source, selected) {
  const kind = readText(offer?.service_kind);
  if (kind === BROWSER_ENGINE_SERVICE_KIND) {
    if (source === "mine") {
      return selected ? "My Browser Engine is shared" : "Share my Browser Engine";
    }
    return readText(offer?.display_name) || "Browser Engine";
  }
  if (source === "mine") {
    return selected ? "My Browser Exit service is shared" : "Share my Browser Exit service";
  }
  return readText(offer?.display_name) || "External Browser Exit service";
}

function serviceCopy(offer, source, selected) {
  const kind = readText(offer?.service_kind);
  if (kind === BROWSER_ENGINE_SERVICE_KIND) {
    if (source === "mine") {
      return selected
        ? "People you trust can use this Browser Engine after you approve access. Low-level connection details stay hidden."
        : "Make this Browser Engine available to People you trust. Access stays under your approval.";
    }
    const name = readText(offer?.display_name) || "this person's Browser Engine";
    if (offer?.grant_required === true) {
      const requestStatus = serviceRequestStatus(offer);
      if (selected && requestStatus === "approved") {
        return `${name} was approved. Browser can use it when access becomes active.`;
      }
      if (selected && requestStatus === "denied") {
        return `${name} denied the request. Remove it and ask again if needed.`;
      }
      return selected
        ? `${name} is waiting for approval.`
        : `Ask to use ${name}. You need to be connected in People first.`;
    }
    return selected
      ? `${name} is saved as a Browser Engine option. Browser can use it when the service connection is active.`
      : `Subscribe to ${name}. You need to be connected in People first.`;
  }
  const name = readText(offer?.display_name) || "this person's Browser Exit service";
  if (source === "mine") {
    return selected
      ? "People you trust can use this device's Browser Exit service after approval. Low-level network details stay hidden."
      : "Let People you trust use this device's Browser Exit service. Access stays under your approval.";
  }
  if (readText(offer?.source) === CONFIGURED_REMOTE_EXIT_SOURCE) {
    return `${name} is available as a Browser Exit option on this device.`;
  }
  if (readText(offer?.status) === "active" && offer?.enabled === true) {
    return `${name} is active and ready for Browser.`;
  }
  if (offer?.grant_required === true) {
    const requestStatus = serviceRequestStatus(offer);
    if (selected && requestStatus === "approved") {
      return `${name} was approved. Browser can use it when access becomes active.`;
    }
    if (selected && requestStatus === "denied") {
      return `${name} denied the request. Remove it and ask again if needed.`;
    }
    return selected
      ? `${name} is waiting for approval.`
      : `Ask to use ${name}. You need to be connected in People first.`;
  }
  return selected
    ? `${name} is saved as a Browser Exit option. Browser can use it when the service connection is active.`
    : `Subscribe to ${name}. You need to be connected in People first.`;
}

function serviceStatus(offer, source, selected) {
  if (source === "mine") {
    return selected ? "Shared" : "Private";
  }
  if (readText(offer?.source) === CONFIGURED_REMOTE_EXIT_SOURCE) {
    return "Active";
  }
  if (readText(offer?.status) === "active" && offer?.enabled === true) {
    return "Active";
  }
  if (selected) {
    if (offer?.grant_required === true) {
      const requestStatus = serviceRequestStatus(offer);
      if (requestStatus === "approved") {
        return "Approved";
      }
      if (requestStatus === "denied") {
        return "Denied";
      }
      return "Requested";
    }
    return "Subscribed";
  }
  const status = readText(offer?.status);
  return status === "requestable" ? "Available" : status || "Available";
}

function serviceActionLabel(source, selected, offer = null) {
  if (selected) {
    return source === "others" ? "Remove" : "Stop sharing";
  }
  if (source === "others") {
    return offer?.grant_required === true ? "Ask to use" : "Subscribe";
  }
  return "Share with People";
}

function serviceRequestStatus(offer) {
  const status = readText(offer?.status);
  return status === "approved" || status === "denied" ? status : "requested";
}

function orderedServiceOffers(offers) {
  return [...offers].sort((left, right) => (
    serviceKindRank(readText(left?.service_kind)) - serviceKindRank(readText(right?.service_kind))
    || readText(left?.display_name).localeCompare(readText(right?.display_name))
    || readText(left?.offer_id).localeCompare(readText(right?.offer_id))
  ));
}

function serviceKindRank(kind) {
  switch (kind) {
    case BROWSER_ENGINE_SERVICE_KIND:
      return 0;
    case EXIT_SERVICE_KIND:
      return 1;
    default:
      return 10;
  }
}

function serviceStatusTone(offer, source) {
  if (readText(offer?.source) === CONFIGURED_REMOTE_EXIT_SOURCE) {
    return "ok";
  }
  if (offer?.enabled === true) {
    return "ok";
  }
  if (source === "others" && offer?.grant_required === true) {
    return serviceRequestStatus(offer) === "approved" ? "ok" : "warn";
  }
  return "muted";
}

function isReadOnlyServiceOffer(offer) {
  return readText(offer?.source) === CONFIGURED_REMOTE_EXIT_SOURCE;
}

function renderEmptyCard(text) {
  return `<div class="empty-card">${escapeHtml(text)}</div>`;
}

async function fetchJson(url, init) {
  const response = await fetch(url, init);
  if (!response.ok) {
    const detail = await response.text().catch(() => "");
    const suffix = detail.trim() ? ` ${detail.trim()}` : ` ${response.statusText}`;
    throw new Error(`request failed: ${response.status}${suffix}`);
  }
  return response.json();
}

function shellHeaders(extra) {
  return Object.assign(
    homeToken.length > 0 ? { "x-elastos-home-token": homeToken } : {},
    extra || {},
  );
}

function setBusy(busy) {
  if (refreshButton instanceof HTMLButtonElement) {
    refreshButton.disabled = busy;
  }
}

function showStatus(text, tone = "muted") {
  if (!statusNode) {
    return;
  }
  statusNode.textContent = tone === "error"
    ? publicServicesError(text, "Services could not be updated.")
    : text;
  statusNode.dataset.tone = tone;
  statusNode.hidden = !text;
}

function publicServicesError(value, fallback) {
  const message = String(value || "").trim();
  if (!message || /\b(schema|projection|provider|adapter|capability|affordance|runtime-owned|launch token|hostcall|request failed|failed to fetch|unauthorized|forbidden|[45]\d\d)\b|engine_[a-z_]+/i.test(message)) {
    return fallback;
  }
  return message;
}

function readText(value) {
  return typeof value === "string" ? value.trim() : "";
}

function escapeHtml(value) {
  return String(value).replace(/[&<>"']/g, (char) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[char]);
}
