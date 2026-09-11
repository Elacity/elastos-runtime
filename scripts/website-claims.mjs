// Validate dated deployment receipts used by the website truth check.
const hex = (value, length) => typeof value === "string" && new RegExp(`^[a-f0-9]{${length}}$`, "i").test(value);

function isObservation(value, now) {
  if (typeof value !== "string" || !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{3})?Z$/.test(value)) return false;
  const time = Date.parse(value);
  const canonical = value.length === 20 ? `${value.slice(0, -1)}.000Z` : value;
  return Number.isFinite(time) && time <= now && new Date(time).toISOString() === canonical;
}

export function validHostedReceipt(hosted, origin, now = Date.now()) {
  // This is a dated operator record, not a signature or an ongoing parity check.
  if (hosted?.status === "verified" && typeof hosted.version === "string" && /^\d+\.\d+\.\d+(?:-[a-z0-9.-]+)?$/i.test(hosted.version)
    && hex(hosted.source_commit, 40) && hex(hosted.source_tree, 40)
    && ["binary_sha256", "components_sha256", "home_index_sha256", "site_index_sha256"].every((key) => hex(hosted[key], 64))
    && hosted.evidence === "/claims.json" && typeof origin === "string"
    && hosted.target === `${origin}/` && /^https?:\/\//.test(origin)
    && isObservation(hosted.observed_at, now)) {
    return true;
  }
  return false;
}
