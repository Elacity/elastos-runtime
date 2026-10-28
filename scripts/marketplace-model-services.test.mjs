import assert from "node:assert/strict";
import { test } from "node:test";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import { sharedModelOffers, modelAccessOpportunities } from "../capsules/marketplace/browser/model-services.js";

const offer = () => ({
  id: "remote:grant-one:model:hosted-one", title: "Shared writing model", operation: "text.generate",
  input_modalities: ["text/plain"], output_modalities: ["text/plain"],
  remote_service: { grant_id: "grant-one", display_name: "Owner Home", expires_at: 200 },
  hosted: { backend_provider_label: "Venice", requested_selector: "writing-model", payer: "this Home" },
  policy: { concurrency_limit: 1, input_bytes_limit: 4096, runtime_ms_limit: 30000 },
});

test("shared cards retain exact grant identity and consumer-facing execution facts", () => {
  const input = offer();
  const [card] = sharedModelOffers({ status: "ok", data: { offers: [input] } }, 100);
  assert.equal(card.id, input.id);
  assert.equal(card.owner, "Owner Home");
  const rows = Object.fromEntries(card.facts.detailRows.map(row => [row.term, row.value]));
  assert.equal(rows.Processor, "Venice");
  assert.equal(rows.Payer, "Provider Home (Owner Home)");
  assert.equal(rows["Prompt destination"], "Owner Home, then Venice");
  assert.match(rows.Limits, /1 run at a time.*4096 bytes.*30 seconds/);
});

test("only current text offers with matching grant identity become shared cards", () => {
  const local = { ...offer(), id: "model:local", remote_service: undefined };
  const decision = { ...offer(), operation: "decision.evaluate" };
  assert.deepEqual(sharedModelOffers({ offers: [local, decision] }, 100), []);
  for (const remote_service of [
    { ...offer().remote_service, grant_id: "other" },
    { ...offer().remote_service, expires_at: 100 },
    { ...offer().remote_service, expires_at: "200" },
  ]) assert.throws(() => sharedModelOffers({ offers: [{ ...offer(), remote_service }] }, 100));
  assert.throws(() => sharedModelOffers({ offers: [offer(), offer()] }, 100));
  assert.throws(() => sharedModelOffers({ offers: {} }, 100));
});

test("contact opportunities remain requests, separate from exact usable model offers", () => {
  const access = modelAccessOpportunities({ remote_offers: [{ service_kind: "remote_model", offer_id: "selected", status: "approved" }],
    available_remote_offers: [{ service_kind: "remote_model", offer_id: "contact-model", display_name: "Contact", status: "requestable" },
      { service_kind: "remote_browser", offer_id: "browser" }] });
  assert.deepEqual(access.map(item => [item.id, item.status, item.requestable]),
    [["selected", "Approved", false], ["contact-model", "Not shared yet", true]]);
  assert.equal(access[1].facts, undefined);
  assert.throws(() => modelAccessOpportunities({ remote_offers: [], available_remote_offers: [{ service_kind: "remote_model", offer_id: "bad/id" }] }));
});

test("Home accepts an exact remote offer handoff and rejects extra authority or malformed IDs", () => {
  const source = readFileSync(new URL("../capsules/home/browser/home-shell-host.js", import.meta.url), "utf8");
  const fn = source.slice(source.indexOf("function marketplaceAssistantHandoffQuery("), source.indexOf("function hasExactMessageKeys("));
  const context = vm.createContext({ MODEL_CONTENT_CID: /^bafy[a-z2-7]{20,}$/, MODEL_OFFER_ID: /^model:/ });
  vm.runInContext(fn, context);
  const check = query => context.marketplaceAssistantHandoffQuery(query);
  assert.equal(check({ offer_id: offer().id }), true);
  for (const query of [{ offer_id: "model:hosted-private" }, { offer_id: offer().id, grant: "new" },
    { offer_id: "remote:grant-one:" }, { offer_id: "remote:grant-one:model/bad" }, null, []]) assert.equal(check(query), false);
});

test("one refresh discovers a grant installed by Services summary, including summary failure recovery", async () => {
  const source = readFileSync(new URL("../capsules/marketplace/browser/marketplace.js", import.meta.url), "utf8");
  const fn = source.slice(source.indexOf("  async function loadSharedModels()"), source.indexOf("  async function loadCatalogData()"));
  for (const summaryFails of [false, true]) {
    let reconciled = false;
    const toasts = [];
    const state = { destination: "models", remoteAvailability: [{ status: "stale" }] };
    const context = vm.createContext({ state, homeToken: "fixture", render() {}, AbortSignal,
      showToast: (message, isError) => toasts.push([message, isError]),
      modelAccessOpportunities, sharedModelOffers: payload => sharedModelOffers(payload, 100),
      fetch: async url => {
        if (url.endsWith("summary")) {
          await new Promise(setImmediate);
          reconciled = true;
          if (summaryFails) throw new Error("summary unavailable");
          return { ok: true, json: async () => ({ remote_offers: [], available_remote_offers: [] }) };
        }
        assert.equal(reconciled, true, "discovery waits for the grant reconciliation attempt");
        return { ok: true, json: async () => ({ offers: [offer()] }) };
      },
    });
    vm.runInContext(fn, context);
    await context.loadSharedModels();
    assert.equal(state.sharedModels[0].id, offer().id);
    assert.equal(state.remoteAvailability.length, 0);
    assert.deepEqual(toasts, summaryFails ? [["Couldn’t load shared models.", true]] : []);
    assert.equal(state.sharedLoading, false);
  }
});

test("Models show catalog, shared and contact rows in one list, and a plain line when the catalog is unverified", () => {
  const source = readFileSync(new URL("../capsules/marketplace/browser/marketplace.js", import.meta.url), "utf8");
  const els = { storeSections: { innerHTML: "" } };
  const state = { search: "", sharedLoading: false, modelCatalogState: "verified", remoteAvailability: [],
    sharedModels: [], modelAccess: [] };
  let local = [];
  const context = vm.createContext({ state, els, MODEL_ICON: {}, icons: { search: "" },
    filteredByDestination: () => local, renderAppRow: app => `<article class="store-row">${app.name}</article>`,
    skeletonRows: () => "skeleton", emptyState: (title, description) => `empty:${title}|${description}`,
    escapeHtml: String, escapeAttr: String, appIconHtml: () => "", bindAppActions() {} });
  vm.runInContext(source.slice(source.indexOf("  function renderModels()"), source.indexOf("  function renderCategorySections()")), context);
  const html = () => els.storeSections.innerHTML;

  context.renderModels();
  assert.equal(html(), "empty:No models to show|Choose another category from the sidebar.");
  state.modelCatalogState = "unavailable";
  context.renderModels();
  assert.equal(html(), "empty:Models are not available right now.|");
  state.sharedLoading = true;
  context.renderModels();
  assert.equal(html(), "skeleton");

  state.sharedLoading = false;
  local = [{ name: "Catalog model" }];
  state.sharedModels = [{ id: "remote:one", title: "Shared writing model", owner: "Owner Home" }];
  state.modelAccess = [{ id: "offer:one", title: "Contact AI", status: "Not shared yet", requestable: true }];
  context.renderModels();
  assert.equal((html().match(/<article class="store-row/g) || []).length, 3);
  assert(html().indexOf("Catalog model") < html().indexOf("Shared writing model"), "catalog models come first");
  assert.match(html(), /<div class="store-row-sub">Shared by Owner Home<\/div>/);
  assert.match(html(), /<div class="store-row-sub">From a contact · Not shared yet<\/div>/);
  assert.doesNotMatch(html(), /class="badge/, "model rows carry no badge, like app rows");
  assert.doesNotMatch(html(), /store-section-head|Run on this Home|Shared with you|Nothing shared|Refresh|internet/);
});

function publisherFixture() {
  const source = readFileSync(new URL("../capsules/marketplace/browser/marketplace.js", import.meta.url), "utf8");
  const context = vm.createContext({
    state: {}, homeToken: "fixture", publicError: value => value,
    publicTitle: capsule => capsule.name, publicDescription: () => "", appCategory: () => "apps",
    catalogIconRoute: () => "", appIcon: () => "", appGradient: () => "", appBadges: () => [],
    acceptedContentLabels: () => [], executableActions: () => [], appSortKey: app => app.id,
  });
  for (const [start, end] of [
    ["  async function loadCatalogData()", "  async function loadMediaData()"],
    ["  function listedInMarketplace(", "  function appCategory("],
    ["  function modelSizeLabel(", "  function renderAppRow("],
    ["  function packageLabel(", "  function openApp("],
  ]) vm.runInContext(source.slice(source.indexOf(start), source.indexOf(end)), context);
  return context;
}

test("only a Runtime-verified signer gives a publisher line; authors and signature claims give none", () => {
  const context = publisherFixture();
  for (const author of [undefined, "", "Unknown publisher", "ElastOS", "elastos"]) {
    for (const signature_state of ["invalid", { malformed: true }, "catalog-signature-verified"]) {
      for (const publisher_did of [undefined, "", "ElastOS", { did: "did:key:z" }]) {
        const app = context.capsuleToApp({ name: "fixture", role: "app", source: "installed", author, signature_state,
          publisher_did, upstream_origin: "github.com/ipfs/kubo" }, new Map(), []);
        assert.equal(context.detailPublisher(app), "");
        assert.equal(context.rowSubtitle(app), "");
        assert.equal(context.packageLabel(app), "Publisher verification unavailable");
        assert.doesNotMatch(context.signatureLabel(app), /Runtime verified|Verified publisher/);
      }
    }
  }
  const signed = context.capsuleToApp({ name: "library", role: "app", source: "installed", author: "elastos",
    publisher_did: "did:key:zRelease" }, new Map(), []);
  assert.equal(context.detailPublisher(signed), "Verified publisher");
  assert.equal(context.rowSubtitle(signed), "Verified publisher");
  assert.equal(signed.developer, "did:key:zRelease");
  assert.equal(context.packageLabel(signed), "Release signature verified");
});

test("Marketplace lists only what a person can open or get", async () => {
  const context = publisherFixture();
  const capsules = [
    { name: "library", role: "app", launchable: true, launch_target: "library" },
    { name: "wallet-metamask", role: "app", launchable: true, launch_target: "wallet-metamask" },
    { name: "documents", role: "viewer", launchable: true, launch_target: "documents" },
    { name: "gba-ucity", role: "content", launchable: true, launch_target: "gba-ucity" },
    { name: "archive-manager", role: "viewer", launchable: false, launch_target: "archive-manager" },
    { name: "home", role: "app", launchable: false },
    { name: "home-gui", role: "shell", launchable: true, launch_target: "home-gui" },
    { name: "object-provider", role: "provider", launchable: false },
    { name: "kubo", role: "content", launchable: false },
    { name: "no-target", role: "app", launchable: true },
    { name: "model", source: "signed-model-catalog", role: "content", installed: false, launchable: false,
      cid: `bafybei${"a".repeat(52)}`, publisher_did: "did:key:zFixture", content_size_bytes: 1024,
      signature_state: "catalog-signature-verified" },
  ];
  context.fetch = async url => ({ ok: true, json: async () => url.endsWith("catalog")
    ? { model_catalog_state: "verified", capsules } : { interfaces: [] } });
  await context.loadCatalogData();
  assert.deepEqual([...context.state.apps.map(app => app.id)].sort(),
    ["documents", "gba-ucity", "library", `model:bafybei${"a".repeat(52)}`, "wallet-metamask"]);
});

test("invalid or unavailable model catalog verification clears publisher cards", async () => {
  const context = publisherFixture();
  const model = { name: "fixture", source: "signed-model-catalog", role: "content", installed: false, launchable: false,
    cid: `bafybei${"a".repeat(52)}`, publisher_did: "did:key:zFixture", content_size_bytes: 1024,
    signature_state: "catalog-signature-verified" };
  for (const [status, changes, accepted] of [
    ["verified", {}, true], ["verified", { signature_state: "invalid" }, false],
    ["verified", { signature_state: { malformed: true } }, false], ["unavailable", {}, false],
  ]) {
    context.fetch = async url => ({ ok: true, json: async () => url.endsWith("catalog")
      ? { model_catalog_state: status, capsules: [{ ...model, ...changes }] } : { interfaces: [] } });
    await context.loadCatalogData();
    assert.equal(context.state.apps.length, accepted ? 1 : 0);
    if (accepted) assert.equal(context.detailPublisher(context.state.apps[0]), "Verified publisher");
    else assert.match(context.state.appLoadError, /verification is unavailable/);
  }
});

test("a signed model shows its own description and never Runtime's generic capsule text", () => {
  const context = publisherFixture();
  const model = { name: "fixture", source: "signed-model-catalog", role: "content", cid: `bafybei${"a".repeat(52)}`, content_size_bytes: 1024 };
  for (const [description, expected] of [
    ["Larger model for better answers.", "Larger model for better answers."],
    ["Capsule metadata available through Runtime.", "Language model for Assistant."],
    [undefined, "Language model for Assistant."],
  ]) assert.equal(context.capsuleToApp({ ...model, description }, new Map(), []).description, expected);
});

test("the catalog's protected-content policy decides whether Marketplace shows Media", async () => {
  const source = readFileSync(new URL("../capsules/marketplace/browser/marketplace.js", import.meta.url), "utf8");
  for (const enabled of [false, true]) {
    const mediaNav = { hidden: false };
    const context = publisherFixture();
    context.state.destination = "media";
    context.document = { querySelectorAll: selector => selector === '[data-destination="media"]' ? [mediaNav] : [] };
    context.categories = [];
    context.selectDestination = id => { context.state.destination = context.normalizeDestination(id); };
    vm.runInContext(source.slice(source.indexOf("  function syncProtectedContent()"), source.indexOf("  function destinationTitle(")), context);
    context.fetch = async url => ({ ok: true, json: async () => url.endsWith("catalog")
      ? { model_catalog_state: "unconfigured", capsules: [], policy: { protected_content: enabled } } : { interfaces: [] } });
    await context.loadCatalogData();
    context.syncProtectedContent();
    assert.equal(mediaNav.hidden, !enabled);
    assert.equal(context.state.destination, enabled ? "media" : "discover");
  }
});
