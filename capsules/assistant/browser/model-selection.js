// Selection is user intent. Current Runtime catalog/offer reads supply readiness.
export const validModelCid = value => typeof value === "string" && /^bafybei[a-z2-7]{51}[aeimquy4]$/.test(value);
const text = (value, max = 256) => typeof value === "string" && value.length > 0 && value.length <= max && value.trim() === value;

export function catalogModels(catalog) {
  if (catalog?.schema !== "elastos.capsules.catalog/v1" || !Array.isArray(catalog.capsules) || catalog.capsules.length > 1024 ||
      !["verified", "unavailable", "unconfigured"].includes(catalog.model_catalog_state)) throw new Error("Model catalog unavailable");
  const rows = catalog.capsules.filter(row => row?.source === "signed-model-catalog");
  if (rows.length > 8) throw new Error("Model catalog unavailable");
  const seen = new Set();
  for (const row of rows) {
    if (typeof row.cid !== "string" || seen.has(row.cid)) throw new Error("Model catalog unavailable");
    seen.add(row.cid);
  }
  if (catalog.model_catalog_state !== "verified") return [];
  return rows.map(row => {
    const facts = row.model_runtime;
    if (row.role !== "content" || row.installed !== false || row.launchable !== false || !validModelCid(row.cid) ||
        !text(row.title) || row.signature_state !== "catalog-signature-verified" || !facts ||
        typeof facts.admitted !== "boolean" || typeof facts.dispatch_ready !== "boolean" ||
        (facts.dispatch_ready ? !facts.admitted || !text(facts.offer_id) : facts.offer_id !== null)) throw new Error("Model catalog unavailable");
    return { cid: row.cid, title: row.title, offerId: facts.offer_id, ready: facts.dispatch_ready,
      downloadPercent: downloadPercent(facts.preparation) };
  });
}

// Runtime preparation states that are still fetching or checking model files.
const DOWNLOADING_STATES = new Set(["capacity_pending", "reserved", "preparing", "verifying", "admission_pending"]);

// Whole-percent download progress for a running preparation, otherwise null.
function downloadPercent(preparation) {
  if (!preparation || !DOWNLOADING_STATES.has(preparation.state) || preparation.cancel_requested) return null;
  const { completed_bytes: done, total_bytes: total } = preparation;
  if (!Number.isSafeInteger(done) || !Number.isSafeInteger(total) || total <= 0) return 0;
  return Math.min(100, Math.floor((Math.max(0, done) * 100) / total));
}

// The model a running preparation is downloading, for when no model is ready yet.
export function downloadingModel(models) {
  return models.find(row => !row.ready && row.downloadPercent !== null) || null;
}

export function selectedModelOffer(offers, offerId, selectedCid, models) {
  if (selectedCid != null) {
    if (!validModelCid(selectedCid)) return null;
    const mapped = models.filter(row => row.cid === selectedCid);
    if (mapped.length !== 1 || !mapped[0].ready) return null;
    // CID is saved user intent. The current verified row supplies its offer hint.
    offerId = mapped[0].offerId;
  }
  const matches = offers.filter(offer => offer.id === offerId);
  if (!offerId || matches.length !== 1) return null;
  return matches[0];
}

export function readyModelChoices(models, offers) {
  return models.filter(row => row.ready && selectedModelOffer(offers, row.offerId, row.cid, models));
}
