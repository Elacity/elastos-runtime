import assert from "node:assert/strict";

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
  return { origin, adminOrigin, engineId, allowRemote };
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
