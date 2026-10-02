// Validate the current isolation scope and bounded hosted deployment receipts.
const hex = (value, length) => typeof value === "string" && new RegExp(`^[a-f0-9]{${length}}$`, "i").test(value);
export const HOSTED_RECEIPT_MAX_AGE_MS = 24 * 60 * 60 * 1000;
const receiptKeys = ["status", "version", "observed_at", "target", "evidence", "source_commit", "source_tree", "binary_sha256", "components_sha256", "home_index_sha256", "site_index_sha256"];
const artifactKeys = ["source_commit", "source_tree", "binary_sha256", "components_sha256", "home_index_sha256", "site_index_sha256"];

function observationTime(value) {
  if (typeof value !== "string" || !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{3})?Z$/.test(value)) return NaN;
  const time = Date.parse(value);
  const canonical = value.length === 20 ? `${value.slice(0, -1)}.000Z` : value;
  return Number.isFinite(time) && new Date(time).toISOString() === canonical ? time : NaN;
}

export function validIsolationClaims(isolation) {
  const scope = {
    apps: "opaque-browser-frames-runtime-checks",
    home: "cross-app-capabilities",
    providers: "native-processes-model-partly-confined",
    stolen_device_key: "new-identity",
    stored_keys: "beside-data-backups-include-all-keys",
    seed_operator: "can-read-data-wallet-keys-recovery-phrases",
    proof: "requires-integrated-s0-and-installed-acceptance",
    evidence: "https://github.com/Elacity/elastos-runtime/issues/173",
  };
  return isolation != null && Object.keys(isolation).length === Object.keys(scope).length
    && Object.entries(scope).every(([key, value]) => isolation[key] === value);
}

export function validHostedReceipt(hosted, origin, now = Date.now(), expected) {
  // Operator evidence needs exact independently accepted identities and a fresh
  // observation. This validates a record; served-byte verification is separate.
  if (!hosted || Object.keys(hosted).length !== receiptKeys.length || !receiptKeys.every((key) => Object.hasOwn(hosted, key))) return false;
  const time = observationTime(hosted.observed_at);
  if (!Number.isFinite(now) || !Number.isFinite(time) || time > now || now - time > HOSTED_RECEIPT_MAX_AGE_MS) return false;
  if (hosted?.status !== "verified" || typeof hosted.version !== "string"
    || !/^\d+\.\d+\.\d+(?:-[a-z0-9.-]+)?$/i.test(hosted.version)
    || expected?.version !== hosted.version || hosted.evidence !== "/claims.json") return false;
  if (typeof origin !== "string") return false;
  try {
    const url = new URL(origin);
    if (!["http:", "https:"].includes(url.protocol) || url.origin !== origin || hosted.target !== `${origin}/`) return false;
  } catch { return false; }
  return artifactKeys.every((key) => hex(hosted[key], key.startsWith("source_") ? 40 : 64)
    && hex(expected?.[key], key.startsWith("source_") ? 40 : 64)
    && hosted[key] === expected[key]);
}
