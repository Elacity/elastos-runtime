// Static content stays useful when live release checks fail.
// Metadata consistency and a computed checksum are not signature verification.
export const HOME_PATH = "/home/";

const hex = (value, length) => typeof value === "string" && new RegExp(`^[a-f0-9]{${length}}$`, "i").test(value);
const isCid = (value) => typeof value === "string" && /^(Qm[1-9A-HJ-NP-Za-km-z]{44}|b[a-z2-7]{10,})$/.test(value);
const isArtifact = (value) => value && isCid(value.cid) && hex(value.sha256, 64) && Number.isSafeInteger(value.size) && value.size > 0;
const isDate = (value) => Number.isSafeInteger(value) && value > 0 && value < 100000000000;

export function assessRelease(head, release) {
  if (!head || !release) return { state: "unavailable" };
  const h = head.payload;
  const r = release.payload;
  if (!h || !r || h.schema !== "elastos.release.head/v1" || r.schema !== "elastos.release/v1"
    || typeof h.version !== "string" || !/^\d+\.\d+\.\d+(?:-[a-z0-9.-]+)?$/i.test(h.version)
    || h.version !== r.version || typeof h.channel !== "string" || !h.channel || h.channel !== r.channel
    || typeof head.signer_did !== "string" || !head.signer_did.startsWith("did:key:")
    || head.signer_did !== h.signer_did || head.signer_did !== release.signer_did
    || !hex(head.signature, 128) || !hex(release.signature, 128) || !isCid(h.latest_release_cid)
    || !isDate(h.updated_at) || !isDate(r.released_at) || h.updated_at < r.released_at
    || !r.platforms || Array.isArray(r.platforms) || typeof r.platforms !== "object"
    || !Object.keys(r.platforms).length
    || Object.values(r.platforms).some((entry) => !isArtifact(entry?.binary) || !isArtifact(entry?.components))) {
    return { state: "inconsistent" };
  }
  return {
    state: "matched",
    version: h.version,
    channel: h.channel,
    released: new Date(r.released_at * 1000).toISOString().slice(0, 10),
    signer: head.signer_did,
    platforms: Object.keys(r.platforms),
  };
}

export async function loadRelease(fetcher = globalThis.fetch) {
  try {
    const documents = await Promise.all(["/release-head.json", "/release.json"].map(async (path) => {
      const response = await fetcher(path, { cache: "no-store", credentials: "omit", signal: AbortSignal.timeout(8000) });
      if (!response.ok) throw new Error("Release document unavailable");
      return response.json();
    }));
    return assessRelease(...documents);
  } catch {
    return { state: "unavailable" };
  }
}

export function assessInstall(release, platform, facts) {
  if (release.state === "inconsistent") return { enabled: false, message: "The release documents disagree or are incomplete. Installation stays unavailable until the publisher fixes them." };
  if (release.state !== "matched") return { enabled: false, message: "The publisher could not be checked. Use the source guide, or return after the release service is available." };
  if (!release.platforms.includes(platform)) return { enabled: false, message: `The publisher has no ${platform === "aarch64-darwin" ? "Apple silicon" : "matching platform"} installer in ${release.version}. Use the source guide for current Home testing.` };
  if (!facts?.candidate || release.version !== facts.candidate.version) return { enabled: false, message: `The publisher offers ${release.version}. A published installer for the Home candidate is still pending.` };
  // This pilot displays release facts. Enabling install needs separate proof
  // bound to the exact release, installer and selected platform artifacts.
  return { enabled: false, message: `${release.version} lists this platform. This preview keeps installation unavailable until the exact published files pass the Home install checks.` };
}

function isObservation(value, now) {
  if (typeof value !== "string" || !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{3})?Z$/.test(value)) return false;
  const time = Date.parse(value);
  const canonical = value.length === 20 ? `${value.slice(0, -1)}.000Z` : value;
  return Number.isFinite(time) && time <= now && new Date(time).toISOString() === canonical;
}

export function hostedStatus(hosted, origin = globalThis.location?.origin, now = Date.now()) {
  // This is a dated operator record, not a signature or an ongoing parity check.
  if (hosted?.status === "verified" && typeof hosted.version === "string" && /^\d+\.\d+\.\d+(?:-[a-z0-9.-]+)?$/i.test(hosted.version)
    && hex(hosted.source_commit, 40) && hex(hosted.source_tree, 40)
    && ["binary_sha256", "components_sha256", "home_index_sha256", "site_index_sha256"].every((key) => hex(hosted[key], 64))
    && hosted.evidence === "/claims.json" && typeof origin === "string"
    && hosted.target === `${origin}/` && /^https?:\/\//.test(origin)
    && isObservation(hosted.observed_at, now)) {
    return {
      text: `${hosted.version} · ${hosted.source_commit.slice(0, 8)}`,
      evidence: hosted.evidence,
      checked: `Last checked ${hosted.observed_at.slice(0, 10)}`,
    };
  }
  return { text: "Version awaiting verification" };
}

async function setupPage() {
  const $ = (selector) => document.querySelector(selector);
  let facts = null;
  let release = { state: "unavailable" };
  const platform = $("#platform");

  function renderInstall() {
    const result = assessInstall(release, platform.value, facts);
    $("#install-state").textContent = result.message;
  }

  platform.addEventListener("change", renderInstall);
  // Platform detection changes a selection only; it never probes the computer.
  if (/Linux/.test(navigator.userAgent) && !/Android/.test(navigator.userAgent)) platform.value = "x86_64-linux";
  const factsRequest = fetch(new URL("./claims.json", import.meta.url), { cache: "no-store", credentials: "omit", signal: AbortSignal.timeout(8000) })
    .then((response) => response.ok ? response.json() : null)
    .catch(() => null);
  const releaseRequest = loadRelease();
  const hashRequest = fetch("/install.sh", { cache: "no-store", credentials: "omit", signal: AbortSignal.timeout(8000) })
    .then(async (response) => {
      if (!response.ok) throw new Error("Installer unavailable");
      const bytes = await response.arrayBuffer();
      const digest = await crypto.subtle.digest("SHA-256", bytes);
      $("#installer-sha").textContent = Array.from(new Uint8Array(digest), (value) => value.toString(16).padStart(2, "0")).join("");
    })
    .catch(() => { $("#installer-sha").textContent = "Checksum unavailable"; });

  [facts, release] = await Promise.all([factsRequest, releaseRequest]);
  if (facts?.schema !== "elastos.website.claims/v1") facts = null;
  const hosted = hostedStatus(facts?.hosted);
  $("#hosted-version").textContent = hosted.text;
  $("#hosted-checked").textContent = hosted.checked || "";
  if (hosted.evidence) {
    const link = document.createElement("a");
    link.href = hosted.evidence;
    link.textContent = "Last verified deployment · operator record";
    $("#hosted-evidence").append(link);
    $("#hosted-evidence").hidden = false;
  }
  if (facts?.candidate?.version) $("#candidate-version").textContent = `${facts.candidate.version} candidate`;

  if (release.state === "matched") {
    const linuxOnly = release.platforms.every((value) => value.endsWith("-linux"));
    $("#public-version").textContent = `${release.version} · ${linuxOnly ? "Linux only" : release.platforms.join(", ")}`;
    $("#public-status").textContent = `${release.channel} · Released ${release.released}`;
    $("#installer-intro").textContent = release.version === facts?.candidate?.version
      ? `The publisher lists ${release.version}. This preview reports the release facts; installation needs a separate check of the exact published files.`
      : `The publisher serves ${release.version}. This is a separate release from the Home candidate.`;
    $("#metadata-state").textContent = "Versions, channels and publishers match. Signature verification belongs to the installer.";
    $("#publisher-did").textContent = release.signer;
  } else {
    $("#public-version").textContent = release.state === "inconsistent" ? "Release documents disagree" : "Live release check unavailable";
    $("#public-status").textContent = "Installer action paused. The dated observation is in the evidence record.";
    $("#metadata-state").textContent = release.state === "inconsistent" ? "Documents disagree or required fields are missing." : "Publisher unavailable or response unreadable.";
  }
  renderInstall();
  await hashRequest;
}

if (typeof document !== "undefined") setupPage();
