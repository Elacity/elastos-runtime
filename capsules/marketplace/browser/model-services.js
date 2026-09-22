import { eligibleTextOffers, offerSelectionFacts } from "./model-contract.js";

export const REMOTE_MODEL_ID = /^remote:[A-Za-z0-9_-]{1,128}:[A-Za-z0-9_.:-]{1,160}$/;
export const SERVICE_OFFER_ID = /^[A-Za-z0-9_.:-]{1,256}$/;

// These are projections of current Runtime replies, never a saved catalogue.
export function sharedModelOffers(payload, now = Date.now() / 1000) {
  const data = payload?.data ?? payload;
  if (!Array.isArray(data?.offers) || data.offers.length > 1024) throw new Error("Model offers unavailable");
  const seen = new Set();
  return eligibleTextOffers(data).filter(offer => {
    if (!offer.remote_service || offer.operation !== "text.generate") return false;
    if (!REMOTE_MODEL_ID.test(offer.id) || seen.has(offer.id)
        || offer.id.split(":")[1] !== offer.remote_service.grant_id
        || !Number.isSafeInteger(offer.remote_service.expires_at)
        || offer.remote_service.expires_at <= now) throw new Error("Invalid shared model offer");
    seen.add(offer.id);
    return true;
  }).map(offer => ({
    id: offer.id, title: offer.title, owner: offer.remote_service.display_name || "Provider Home",
    expiresAt: offer.remote_service.expires_at, facts: offerSelectionFacts(offer),
  }));
}

export function modelAccessOpportunities(summary) {
  const selected = summary?.remote_offers;
  const available = summary?.available_remote_offers;
  if (![selected, available].every(items => Array.isArray(items) && items.length <= 1024)) throw new Error("Service access unavailable");
  const seen = new Set();
  return [...selected, ...available].filter(offer => {
    if (offer.service_kind !== "remote_model") return false;
    if (!SERVICE_OFFER_ID.test(offer.offer_id) || seen.has(offer.offer_id)) throw new Error("Invalid service access");
    seen.add(offer.offer_id);
    return true;
  }).map(offer => ({
    id: offer.offer_id, title: offer.display_name || "A contact's AI service",
    status: ({ requestable: "Ask this person", requested: "Waiting for approval", pending: "Waiting for approval",
      approved: "Approved", active: "Approved", denied: "Request denied", expired: "Access expired" })[offer.status] || "Check access in Services",
    requestable: offer.status === "requestable" || offer.status === "expired",
  }));
}
