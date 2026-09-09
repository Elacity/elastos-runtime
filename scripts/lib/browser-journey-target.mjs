import assert from "node:assert/strict";
import { createHash } from "node:crypto";

// Product navigation and operator reads can reach the same fixture through
// different origins. Neither input grants Home or Runtime authority.
export function browserJourneyTargetConfig(env = {}) {
  const allowRemote = env.HOME_VIRTUAL_AUTH_BROWSER_ALLOW_REMOTE_FIXTURE === "1";
  const parseOrigin = (raw, name) => {
    const url = new URL(raw);
    assert(["http:", "https:"].includes(url.protocol) && !url.username && !url.password &&
      url.pathname === "/" && !url.search && !url.hash &&
      (raw === url.origin || raw === `${url.origin}/`), `${name} must be an exact HTTP(S) origin`);
    const loopback = ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname);
    assert(loopback || allowRemote,
      `${name} requires HOME_VIRTUAL_AUTH_BROWSER_ALLOW_REMOTE_FIXTURE=1 for a non-loopback origin`);
    return url.origin;
  };
  const origin = parseOrigin(env.HOME_VIRTUAL_AUTH_BROWSER_FIXTURE_ORIGIN || "http://localhost:61511",
    "HOME_VIRTUAL_AUTH_BROWSER_FIXTURE_ORIGIN");
  const adminOrigin = parseOrigin(env.HOME_VIRTUAL_AUTH_BROWSER_FIXTURE_ADMIN_ORIGIN || origin,
    "HOME_VIRTUAL_AUTH_BROWSER_FIXTURE_ADMIN_ORIGIN");
  const engineId = env.HOME_VIRTUAL_AUTH_BROWSER_ENGINE_ID || "";
  assert(engineId === "" || /^[A-Za-z0-9:_-]{1,128}$/.test(engineId), "Browser Engine requires an exact Runtime identifier");
  const mode = env.HOME_VIRTUAL_AUTH_BROWSER_PROFILE_MODE || "";
  const marker = env.HOME_VIRTUAL_AUTH_BROWSER_PROFILE_MARKER || "";
  const profile = mode || marker ? { mode, marker } : null;
  if (profile) {
    assert(["write", "read"].includes(mode) && /^[A-Za-z0-9_-]{8,64}$/.test(marker),
      "Browser profile probe requires write/read mode and an 8..64 character safe marker");
    assert(env.HOME_VIRTUAL_AUTH_BROWSER_CONTROLLED_JOURNEY === "1" &&
      env.HOME_VIRTUAL_AUTH_BROWSER_REQUIRE_VZ_TRANSPORT === "1" &&
      env.HOME_VIRTUAL_AUTH_BROWSER_PROFILE_RESET !== "1",
    "Browser profile probe requires the controlled VZ journey with profile reset disabled");
  }
  return { origin, adminOrigin, engineId, allowRemote, ...(profile ? { profile: Object.freeze(profile) } : {}) };
}

export function browserJourneyFixtureUrl(config, run, page, { media = false, qualification = false } = {}) {
  assert(/^[A-Za-z0-9_-]{8,64}$/.test(run) && ["main", "nav"].includes(page), "Invalid controlled fixture URL");
  return `${config.origin}/${page}?run=${run}${media ? "&media=1" : ""}${qualification ? "&qualification=1" : ""}` +
    (config.profile ? `&profile=${config.profile.mode}&marker=${config.profile.marker}` : "");
}

// Return null only while the expected document's storage event is still pending.
export function browserJourneyProfileStorage(config, run, receipt, page, afterSequence = 0) {
  try {
    assert(config.profile && receipt?.schema === "elastos.browser.journey-receipt/v1" && receipt.run === run &&
      Array.isArray(receipt.events), "Browser profile receipt/run mismatch");
    assert.deepEqual(receipt.profile_probe, config.profile, "Browser profile phase/marker mismatch");
    const events = receipt.events.filter(event => event.type === "profile_storage");
    for (const event of events) {
      const p = event.profile_storage, idb = p?.indexed_db;
      assert(["main", "nav"].includes(event.page) && Number.isSafeInteger(event.sequence) && event.sequence > 0 &&
        p?.schema === "elastos.browser.profile-storage/v1" && p.mode === config.profile.mode && p.marker === config.profile.marker &&
        p.measurement_ok === true && p.ok === true && p.error === null &&
        [p.cookie, p.local_storage, idb].every(value => value?.present === true && value.matches === true) &&
        idb.read_request_succeeded === true && idb.read_completed === true &&
        idb.write_request_succeeded === (p.mode === "write") && idb.write_committed === (p.mode === "write"),
      "Browser profile storage measurement/commit/match failed");
    }
    return events.find(event => event.page === page && event.sequence > afterSequence) || null;
  } catch (error) {
    error.details = { profile_receipt: receipt };
    throw error;
  }
}

const sha256 = value => createHash("sha256").update(value).digest("hex");
const lifecycleHash = value => `sha256:${sha256(value).slice(0, 16)}`;

// The API has already accepted this launch token. Record identity fingerprints,
// not the token, and bind them to Runtime's current page/profile projection.
export function browserJourneyProfileBinding(config, summary, pageId, runtimeOrigin, token) {
  const launch = JSON.parse(Buffer.from(token, "base64url").toString("utf8"));
  const principal = summary?.principal_id, owner = summary?.sessions?.recoverable_page;
  assert(summary?.schema === "elastos.browser.runtime/v1" && typeof principal === "string" && principal.length > 0 &&
    launch.payload?.schema === "elastos.home.launch-token/v4" && launch.payload.principal_id === principal &&
    launch.payload.launch_context?.executable_actor === "browser" &&
    typeof launch.signer_did === "string" && launch.signer_did.startsWith("did:"),
  "Browser profile Runtime/principal authority mismatch");
  const rows = summary.sessions?.lifecycle?.sessions?.filter(row => row.page_id === lifecycleHash(pageId)) || [];
  assert(summary.sessions?.lifecycle?.schema === "elastos.browser.lifecycle-status/v1" && rows.length === 1 &&
    rows[0].phase === "ACTIVE_SESSION" && rows[0].principal_id === lifecycleHash(principal) &&
    rows[0].profile_key_hash === lifecycleHash(`profile-${sha256(principal.trim())}`),
  "Browser profile lifecycle identity mismatch");
  const engineId = owner?.service_selection?.engine_id, adapterId = owner?.engine_page?.adapter;
  // Runtime preserves an empty requested Engine ID for Automatic selection.
  assert(typeof engineId === "string" && /^[A-Za-z0-9:_-]{0,128}$/.test(engineId) && typeof adapterId === "string" && adapterId &&
    (!config.engineId || config.engineId === engineId), "Browser profile Engine selection mismatch");
  const route = browserJourneyEngineRoute(summary, { engineId, adapterId }, pageId);
  assert(typeof route.provider === "string" && route.provider, "Browser profile Engine provider missing");
  return { runtime_origin: new URL(runtimeOrigin).origin, runtime_signer_sha256: sha256(launch.signer_did),
    principal_sha256: sha256(principal), profile_key_hash: rows[0].profile_key_hash,
    engine_id: route.engine_id, adapter_id: route.adapter_id, provider: route.provider, fixture_origin: config.origin };
}

// Compares phase evidence; the ordinary journey's exact close remains mandatory.
export function browserJourneyProfilePair(write, read) {
  assert(write?.schema === "elastos.browser.profile-journey/v1" && read?.schema === write.schema &&
    write.mode === "write" && read.mode === "read" && write.marker === read.marker &&
    typeof write.marker === "string" && /^[A-Za-z0-9_-]{8,64}$/.test(write.marker) &&
    write.run !== read.run && write.complete === true && read.complete === true,
  "Browser profile pair requires complete write/read phases with distinct runs and the same marker");
  for (const phase of [write, read]) {
    assert(/^[A-Za-z0-9_-]{8,64}$/.test(phase.run) && Number.isSafeInteger(phase.started_at) &&
      Number.isSafeInteger(phase.completed_at) && phase.started_at > 0 && phase.completed_at >= phase.started_at,
    "Browser profile pair is missing phase clocks");
    assert(phase.binding && ["runtime_signer_sha256", "principal_sha256"].every(key => /^[a-f0-9]{64}$/.test(phase.binding[key])) &&
      /^sha256:[a-f0-9]{16}$/.test(phase.binding.profile_key_hash) &&
      typeof phase.binding.engine_id === "string" && /^[A-Za-z0-9:_-]{0,128}$/.test(phase.binding.engine_id) &&
      ["runtime_origin", "fixture_origin", "adapter_id", "provider"].every(key =>
        typeof phase.binding[key] === "string" && phase.binding[key]), "Browser profile pair is missing identity");
  }
  assert(read.started_at >= write.completed_at, "Browser profile read began before write close completed");
  assert.deepEqual(read.binding, write.binding, "Browser profile pair Runtime/principal/profile/Engine/origin mismatch");
  return { marker: write.marker, write_run: write.run, read_run: read.run, binding: write.binding };
}

export async function readBrowserJourneyHealth(config, fetchImpl = fetch) {
  const response = await fetchImpl(`${config.adminOrigin}/health`, {
    signal: AbortSignal.timeout(5_000), redirect: "error", credentials: "omit",
  });
  const health = await response.json();
  assert(response.ok && health.schema === "elastos.browser.journey-fixture/v1" && health.ok === true,
    "Controlled Browser fixture is unavailable");
  return health;
}

export async function readBrowserJourneyReceipt(config, run, { signal, fetchImpl = fetch } = {}) {
  const response = await fetchImpl(`${config.adminOrigin}/receipt?run=${encodeURIComponent(run)}`, {
    signal: signal || AbortSignal.timeout(5_000), redirect: "error", credentials: "omit",
  });
  const body = await response.json();
  if (response.status === 404 && body.error === "unknown run") {
    return { schema: "elastos.browser.journey-receipt/v1", run, events: [] };
  }
  assert(response.ok && body.schema === "elastos.browser.journey-receipt/v1" &&
    body.run === run && Array.isArray(body.events), "Controlled fixture receipt failed");
  return body;
}

export function browserJourneyEngineChoice(summary, engineId) {
  const adapters = summary?.engine_adapter?.adapters || [];
  const choices = adapters.filter(adapter => adapter.id === engineId);
  assert(choices.length > 0, "Requested Browser Engine is absent from the Runtime inventory");
  assert(choices.length === 1, "Requested Browser Engine is ambiguous in the Runtime inventory");
  assert(choices[0].direct_network === false && choices[0].wallet_injection === false,
    "Requested Browser Engine violates the Runtime network or Wallet boundary");
  if (!engineId.startsWith("remote-engine-")) return { engineId, adapterId: engineId };
  const offers = (summary.engine_adapter.remote_services?.offers || []).filter(offer =>
    offer.state === "approved" && offer.launch_available === true &&
    offer.selectable_adapters?.some(adapter => adapter.id === engineId));
  assert(offers.length === 1, "Requested remote Browser Engine requires its approved service offer");
  // Runtime preserves inventory order when it projects grant-bound selection IDs.
  const index = offers[0].selectable_adapters.findIndex(adapter => adapter.id === engineId);
  const adapterId = offers[0].adapters?.[index]?.id;
  assert(typeof adapterId === "string" && /^[A-Za-z0-9:_-]{1,128}$/.test(adapterId),
    "Approved Browser Engine offer lacks its concrete adapter");
  return { engineId, adapterId };
}

export function browserJourneyEngineRoute(summary, choice, pageId) {
  const owner = summary?.sessions?.recoverable_page;
  assert(owner?.schema === "elastos.browser.recoverable-page/v1" && owner.state === "active" &&
    owner.page_id === pageId && owner.engine_page?.page_id === pageId &&
    owner.service_selection?.schema === "elastos.browser.service-selection/v1" &&
    owner.service_selection.engine_id === choice.engineId && owner.engine_page.adapter === choice.adapterId,
  "Controlled Browser Runtime ownership used a different Engine or page");
  return { page_id: pageId, engine_id: choice.engineId, adapter_id: owner.engine_page.adapter,
    provider: owner.engine_page.provider };
}
