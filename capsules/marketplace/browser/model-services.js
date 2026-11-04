import { eligibleTextOffers, offerSelectionFacts } from "./model-contract.js";

export const REMOTE_MODEL_ID = /^remote:[A-Za-z0-9_-]{1,128}:[A-Za-z0-9_.:-]{1,160}$/;

// A projection of the current Runtime reply, never a saved catalogue.
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
