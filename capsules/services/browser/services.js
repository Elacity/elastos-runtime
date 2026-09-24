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
const MODEL_SERVICE_KIND = "remote_model";
const CONFIGURED_REMOTE_EXIT_SOURCE = "configured_remote_exit";
const HOSTED_SHARE_SOURCE = "hosted_connection";
const HOSTED_SHARE_TERMS_BY_PROCESSOR = {
  OpenRouter: {
    ack: "openrouter-5.1-5.2+model",
    label: "I accept OpenRouter Terms 5.1-5.2 and the selected model terms. This Home pays. OpenRouter receives prompts.",
  },
  Venice: {
    ack: "venice-7.3+model",
    label: "I accept Venice TOS 7.3 End User API terms for this model. Section 7.1 is personal use and is not a Share grant. This Home pays. Venice receives prompts.",
  },
};
const HOSTED_SHARE_TERMS = {
  "model:openrouter": HOSTED_SHARE_TERMS_BY_PROCESSOR.OpenRouter,
  "model:venice": HOSTED_SHARE_TERMS_BY_PROCESSOR.Venice,
};
const VISIBLE_SERVICE_KINDS = new Set([BROWSER_ENGINE_SERVICE_KIND, MODEL_SERVICE_KIND, EXIT_SERVICE_KIND]);
const SERVICE_KIND_COPY = {
  [BROWSER_ENGINE_SERVICE_KIND]: {
    rank: 0,
    noun: "Browser Engine",
    mineTitle: { shared: "My Browser Engine is shared", private: "Share my Browser Engine" },
    mineCopy: {
      shared: "People you trust can use this Browser Engine after you approve access. Low-level connection details stay hidden.",
      private: "Make this Browser Engine available to People you trust. Access stays under your approval.",
    },
    consumer: "Browser",
    otherFallback: "this person's Browser Engine",
  },
  [MODEL_SERVICE_KIND]: {
    rank: 1,
    noun: "AI model",
    mineTitle: { shared: "My AI model is shared", private: "Share my AI model" },
    mineCopy: {
      shared: "People you trust can run your local AI model after you approve each request. The model and every run stay on this device.",
      private: "Let People you trust run your local AI model. Access stays under your approval and you can revoke it. Hosted connections stay private until you enable Share for that provider and model after the terms check.",
    },
    consumer: "Assistant",
    otherFallback: "this person's AI model",
  },
  [EXIT_SERVICE_KIND]: {
    rank: 2,
    noun: "Browser Exit service",
    mineTitle: { shared: "My Browser Exit service is shared", private: "Share my Browser Exit service" },
    mineCopy: {
      shared: "People you trust can use this device's Browser Exit service after approval. Low-level network details stay hidden.",
      private: "Let People you trust use this device's Browser Exit service. Access stays under your approval.",
    },
    consumer: "Browser",
    otherFallback: "this person's Browser Exit service",
  },
};
const SERVICE_KIND_LIST_COPY = "Browser Engine, AI model or Browser Exit";

let currentServices = null;
let pendingServiceAction = null;
let requestedServiceOffer = launchParams.get("service_offer_id") || "";

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
  window.addEventListener("message", (event) => {
    const data = event.data;
    if (event.source !== window.parent || event.origin !== "null"
        || data?.type !== "elastos.services.navigate/v1" || data.homeToken !== homeToken
        || Object.keys(data).sort().join(",") !== "homeToken,query,type"
        || !data.query || Object.keys(data.query).join(",") !== "service_offer_id"
        || typeof data.query.service_offer_id !== "string"
        || !/^[A-Za-z0-9_.:-]{1,256}$/.test(data.query.service_offer_id)) return;
    requestedServiceOffer = data.query.service_offer_id;
    void refreshServices().catch(() => showStatus("Could not check the requested service.", "error"));
  });
  refreshButton?.addEventListener("click", () => {
    refreshServices().catch((error) => showStatus(error.message || "Could not refresh Services.", "error"));
  });
  document.addEventListener("click", (event) => {
    const target = event.target instanceof Element ? event.target : event.target?.parentElement;
    if (!target) {
      return;
    }
    const catalogButton = target.closest("[data-model-catalog]");
    if (catalogButton) {
      const offerId = readText(catalogButton.getAttribute("data-model-catalog"));
      setServiceOfferSelection({ offerId, section: "catalog", selected: true })
        .then(() => {
          showStatus("Checking named models shared by this person. Refresh if they do not appear yet.", "muted");
          window.setTimeout(() => { void refreshServices().catch(() => {}); }, 2000);
        })
        .catch((error) => showStatus(error.message || "Could not check models.", "error"));
      return;
    }
    const modelButton = target.closest("[data-model-request]");
    if (modelButton) {
      const offerId = readText(modelButton.getAttribute("data-model-request"));
      const choice = [...otherServicesList.querySelectorAll("[data-model-choice]")]
        .find(node => node.getAttribute("data-model-choice") === offerId);
      const option = choice instanceof HTMLSelectElement ? choice.selectedOptions[0] : null;
      setServiceOfferSelection({
        offerId, section: "others", selected: true,
        modelOfferId: readText(option?.value),
        modelOfferRevision: readText(option?.getAttribute("data-model-revision")),
      }).catch((error) => showStatus(error.message || "Could not request this model.", "error"));
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
    if (requestedServiceOffer) {
      const offerId = requestedServiceOffer;
      requestedServiceOffer = "";
      const button = [...otherServicesList.querySelectorAll("[data-service-offer-id], [data-model-catalog], [data-model-request]")]
        .find(node => node.dataset.serviceOfferId === offerId
          || node.dataset.modelCatalog === offerId || node.dataset.modelRequest === offerId);
      if (button) {
        activateServicesSection("other-services", { behavior: "instant" });
        button.scrollIntoView({ block: "center" });
        button.focus();
        showStatus("Review this service, then choose its action. Access changes only after your choice.", "muted");
      } else {
        showStatus("That service is unavailable. Choose from the current services below.", "muted");
      }
    }
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
    emptyAvailable: `No ${SERVICE_KIND_LIST_COPY} service is installed on this device.`,
  });
  otherServicesList.innerHTML = renderServiceSection({
    selected: remoteOffers,
    available: availableRemoteOffers,
    source: "others",
    selectedTitle: "Subscribed",
    availableTitle: "Available from People",
    emptySelected: "No Services from others are subscribed.",
    emptyAvailable: `No ${SERVICE_KIND_LIST_COPY} services are available from People you are connected with.`,
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
  const expired = grantRequired && serviceRequestStatus(offer) === "expired";
  // An expired grant stays in the subscribed list. Asking again posts
  // selected=true so the Runtime sends a fresh request.
  const nextSelected = expired ? "true" : selected ? "false" : "true";
  const modelRequest = source === "others" && readText(offer?.service_kind) === MODEL_SERVICE_KIND;
  const modelEntries = currentServices?.model_catalogs?.[offerId] || [];
  const modelActions = modelRequest
    ? modelEntries.length
      ? `<label>Named model <select data-model-choice="${escapeHtml(offerId)}">${modelEntries.map(entry =>
          `<option value="${escapeHtml(entry.id)}" data-model-revision="${escapeHtml(entry.revision)}">${escapeHtml(entry.title)}</option>`
        ).join("")}</select></label><button class="pc2-btn" type="button" data-model-request="${escapeHtml(offerId)}">Request this model</button>`
      : `<button class="pc2-btn" type="button" data-model-catalog="${escapeHtml(offerId)}">Check named models</button>`
    : "";
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
          ${modelActions}
          ${offerId && !readOnly && (!modelRequest || selected) ? `<button class="pc2-btn" type="button" data-service-offer-id="${escapeHtml(offerId)}" data-service-section="${source}" data-service-selected="${nextSelected}">${primaryAction}</button>` : ""}
          ${readOnly ? '<span class="status-badge" data-tone="ok">Managed by config</span>' : ""}
        </div>
      </div>
      ${pending ? renderInlineConfirmation(primaryAction, offer, nextSelected === "true") : ""}
    </article>
  `;
}

function renderInlineConfirmation(actionLabel, offer, enabling) {
  const terms = hostedShareTerms(offer);
  const termsBlock = enabling && terms
    ? `<label class="service-terms"><input type="checkbox" data-service-terms-ack="${escapeHtml(terms.ack)}"> ${escapeHtml(terms.label)}</label>`
    : "";
  return `
    <div class="service-confirm" role="alert">
      <p>${escapeHtml(confirmMessage(actionLabel, enabling && !!terms))}</p>
      ${termsBlock}
      <div class="service-confirm-actions">
        <button class="pc2-btn" type="button" data-confirm-service-action="cancel">Cancel</button>
        <button class="pc2-btn pc2-btn-danger" type="button" data-confirm-service-action="apply">${escapeHtml(actionLabel)}</button>
      </div>
    </div>
  `;
}

function confirmMessage(actionLabel, hostedTerms) {
  if (hostedTerms) {
    return "Share this hosted model after you accept the named terms for this provider and model?";
  }
  return actionLabel === "Stop sharing"
    ? "Stop sharing this Service?"
    : "Remove this Service from your subscriptions?";
}

async function handleServiceOfferAction(button) {
  if (!(button instanceof HTMLButtonElement)) {
    return;
  }
  const selected = button.dataset.serviceSelected === "true";
  const offerId = readText(button.dataset.serviceOfferId);
  if (!selected || hostedShareTerms(offerId) || hostedShareTerms(findServiceOffer(offerId))) {
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
    selected: button.dataset.serviceSelected === "true",
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
  const terms = hostedShareTerms(pending.offerId) || hostedShareTerms(findServiceOffer(pending.offerId));
  if (pending.selected && terms) {
    const checked = button.closest(".service-confirm")?.querySelector("[data-service-terms-ack]");
    if (!(checked instanceof HTMLInputElement) || !checked.checked) {
      showStatus("Accept the named terms for this provider and model before Share.", "error");
      return;
    }
    pending.termsAck = readText(checked.getAttribute("data-service-terms-ack")) || terms.ack;
  }
  pendingServiceAction = null;
  await setServiceOfferSelection(pending);
}

async function setServiceOfferSelection(button) {
  const offerId = readText(button?.dataset?.serviceOfferId || button?.offerId);
  const section = readText(button?.dataset?.serviceSection || button?.section);
  const selected = button?.dataset?.serviceSelected === "true" || button?.selected === true;
  const termsAck = readText(button?.termsAck);
  const modelOfferId = readText(button?.modelOfferId);
  const modelOfferRevision = readText(button?.modelOfferRevision);
  if (!offerId || !section) {
    throw new Error("This service could not be selected. Refresh and try again.");
  }
  setBusy(true);
  if (button instanceof HTMLButtonElement) {
    button.disabled = true;
  }
  showStatus(selectionProgressMessage(section, selected), "muted");
  try {
    const payload = { offer_id: offerId, section, selected };
    if (selected && termsAck) {
      payload.terms_ack = termsAck;
    }
    if (modelOfferId && modelOfferRevision) {
      payload.model_offer_id = modelOfferId;
      payload.model_offer_revision = modelOfferRevision;
    }
    const services = await fetchJson("/api/apps/services/offers", {
      method: "POST",
      headers: shellHeaders({ "content-type": "application/json" }),
      body: JSON.stringify(payload),
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

function serviceKindCopy(offer) {
  return SERVICE_KIND_COPY[readText(offer?.service_kind)] || SERVICE_KIND_COPY[EXIT_SERVICE_KIND];
}

function serviceTitle(offer, source, selected) {
  const copy = serviceKindCopy(offer);
  if (source === "mine" && isHostedShareOffer(offer)) {
    const name = readText(offer?.display_name) || copy.noun;
    return selected ? `${name} is shared` : `Share ${name}`;
  }
  if (source === "mine") {
    return selected ? copy.mineTitle.shared : copy.mineTitle.private;
  }
  return readText(offer?.display_name) || (copy === SERVICE_KIND_COPY[EXIT_SERVICE_KIND] ? "External Browser Exit service" : copy.noun);
}

function serviceCopy(offer, source, selected) {
  const copy = serviceKindCopy(offer);
  if (source === "mine" && isHostedShareOffer(offer)) {
    const policy = readText(offer?.policy_summary);
    if (policy) {
      return policy;
    }
    return selected
      ? "People you trust can run this hosted model after you approve each request. This Home pays. The external processor receives prompts."
      : "Hosted connections stay private until you enable Share for this provider and model after the terms check. This Home pays. The external processor receives prompts.";
  }
  if (source === "mine") {
    return selected ? copy.mineCopy.shared : copy.mineCopy.private;
  }
  const name = readText(offer?.display_name) || copy.otherFallback;
  if (readText(offer?.source) === CONFIGURED_REMOTE_EXIT_SOURCE) {
    return `${name} is available as a Browser Exit option on this device.`;
  }
  if (readText(offer?.status) === "active" && offer?.enabled === true) {
    return `${name} is active and ready for ${copy.consumer}.`;
  }
  if (offer?.grant_required === true) {
    const requestStatus = serviceRequestStatus(offer);
    if (selected && requestStatus === "approved") {
      return `${name} was approved. ${copy.consumer} can use it when access becomes active.`;
    }
    if (selected && requestStatus === "expired") {
      return `${name} approval expired. Ask again to renew access.`;
    }
    if (selected && requestStatus === "denied") {
      return `${name} denied the request. Remove it and ask again if needed.`;
    }
    return selected
      ? `${name} is waiting for approval.`
      : `Ask to use ${name}. You need to be connected in People first.`;
  }
  return selected
    ? `${name} is saved as a ${copy.noun} option. ${copy.consumer} can use it when the service connection is active.`
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
      if (requestStatus === "expired") {
        return "Expired";
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
  if (selected && serviceRequestStatus(offer) === "expired") {
    return "Ask to use";
  }
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
  return status === "approved" || status === "denied" || status === "expired"
    ? status
    : "requested";
}

function orderedServiceOffers(offers) {
  return [...offers].sort((left, right) => (
    serviceKindRank(readText(left?.service_kind)) - serviceKindRank(readText(right?.service_kind))
    || readText(left?.display_name).localeCompare(readText(right?.display_name))
    || readText(left?.offer_id).localeCompare(readText(right?.offer_id))
  ));
}

function serviceKindRank(kind) {
  return SERVICE_KIND_COPY[kind]?.rank ?? 10;
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

function isHostedShareOffer(offer) {
  return readText(offer?.source) === HOSTED_SHARE_SOURCE || Boolean(hostedShareTerms(offer));
}

function hostedShareTerms(offerOrId) {
  if (typeof offerOrId === "string") {
    if (HOSTED_SHARE_TERMS[offerOrId]) {
      return HOSTED_SHARE_TERMS[offerOrId];
    }
    offerOrId = findServiceOffer(offerOrId);
    if (!offerOrId) {
      return null;
    }
  }
  const id = readText(offerOrId?.offer_id);
  const processor = readText(offerOrId?.provider_label);
  return HOSTED_SHARE_TERMS[id] || HOSTED_SHARE_TERMS_BY_PROCESSOR[processor] || null;
}

function findServiceOffer(offerId) {
  const id = readText(offerId);
  if (!id || !currentServices) {
    return null;
  }
  const lists = [
    currentServices.local_offers,
    currentServices.available_local_offers,
    currentServices.remote_offers,
    currentServices.available_remote_offers,
  ];
  for (const list of lists) {
    if (!Array.isArray(list)) continue;
    const found = list.find((offer) => readText(offer?.offer_id) === id);
    if (found) return found;
  }
  return null;
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
