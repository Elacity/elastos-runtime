// Selection is user intent. Current Runtime catalog/offer reads supply readiness.
export const validModelCid = value => typeof value === "string" && /^bafybei[a-z2-7]{51}[aeimquy4]$/.test(value);
const text = (value, max = 256) => typeof value === "string" && value.length > 0 && value.length <= max && value.trim() === value;

export function catalogModels(catalog) {
  if (catalog?.schema !== "elastos.capsules.catalog/v1" || !Array.isArray(catalog.capsules) || catalog.capsules.length > 1024 ||
      !["verified", "unavailable", "unconfigured"].includes(catalog.model_catalog_state)) throw new Error("Model catalog unavailable");
  const rows = catalog.capsules.filter(row => row?.source === "signed-model-catalog");
  if (rows.length > 1) throw new Error("Model catalog unavailable");
  if (catalog.model_catalog_state !== "verified") return [];
  return rows.map(row => {
    const facts = row.model_runtime;
    if (row.role !== "content" || row.installed !== false || row.launchable !== false || !validModelCid(row.cid) ||
        !text(row.title) || row.signature_state !== "catalog-signature-verified" || !facts ||
        typeof facts.admitted !== "boolean" || typeof facts.dispatch_ready !== "boolean" ||
        (facts.dispatch_ready ? !facts.admitted || !text(facts.offer_id) : facts.offer_id !== null)) throw new Error("Model catalog unavailable");
    return { cid: row.cid, title: row.title, offerId: facts.offer_id, ready: facts.dispatch_ready };
  });
}

export function selectedModelOffer(offers, offerId, selectedCid, models) {
  const matches = offers.filter(offer => offer.id === offerId);
  if (!offerId || matches.length !== 1) return null;
  if (selectedCid != null) {
    if (!validModelCid(selectedCid)) return null;
    const mapped = models.filter(row => row.cid === selectedCid && row.ready && row.offerId === offerId);
    if (mapped.length !== 1) return null;
  }
  return matches[0];
}

export function readyModelChoices(models, offers) {
  return models.filter(row => row.ready && selectedModelOffer(offers, row.offerId, row.cid, models));
}
