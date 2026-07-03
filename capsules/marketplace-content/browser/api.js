/* api.js — the /api/market/* client. Canonical contract: docs/marketplace/API_CONTRACT.md (the SSOT both
 * this shell and the gateway converge on). Falls back to MOCK so the shell runs standalone for review.
 *
 * BUILT (served by gateway.rs today; a call may set live=true):
 *   GET  /api/market/search?op&q                       -> { listings:[Listing], indexed, coverage }
 *   GET  /api/market/sections                          -> { sections:[{id,title,...}] }
 *   POST /api/market/order/sell {ledger,token_id,quantity,price,pay_token?}  -> { unsigned_tx }  (Home-gated)
 *   POST /api/market/order/withdraw {operative,token_id,quantity}            -> { unsigned_tx }  (Home-gated)
 *   POST /api/market/order/approve {operative}                              -> { unsigned_tx }  (Home-gated)
 *   POST /api/market/buy {uri,…}                                            -> buy outcome      (Home-gated)
 *   GET  /api/market/get?operative&token_id  -> { on_chain{token_id,seller,price,pay_token,supply_left,has_access} }  (live re-verify)
 *   GET  /api/market/vault                   -> { owned:[{uri,name,content_cid,mime}] }  (Home-gated; the Library Acquired assets)
 *   POST /api/market/acquire {content_id,content_cid,metadata?,background?} -> sync: { object,uri,… }; background:true -> { status:"started",content_cid }  (Home-gated; gates hasAccessByContentId then pins)
 *   GET  /api/market/acquire-status?cid&token_uri -> { state:"downloaded"|"downloading"|"failed"|"idle", downloaded, uri?, message? }  (Home-gated; truth from Acquired file-presence + in-flight run)
 *
 * PENDING (this shell SPECs them; gateway does NOT serve them yet -> MOCK ONLY, never set live=true):
 *   GET  /api/market/get?content_id   (by-CID variant; needs KID/metadata enrichment — use ?operative&token_id today)
 *   GET  /api/market/listed|history                              (Phase 2)
 *
 * The marketplace MINTS nothing (creator app) and PLAYS nothing (runtime players). After buy it TRIGGERS a
 * pin of the encrypted asset into the local Library (acquire), then hands off opening to the runtime's
 * `POST /api/viewers/open { uri }` — the marketplace never builds a viewer session or touches a CEK (P15/P16).
 *
 * OnChain (the Phase-1 re-verified terms read live at detail/buy time, never trusted from cache):
 *   { token_id, price, pay_token, supply_left, seller, has_access }   (price in USDC minor units on chain)
 */
window.API = (function () {
  const base = "/api/market";
  // The runtime launches this capsule with ?home_token=… in the URL; gated routes need it as a header.
  const homeToken = () => { try { return new URL(location.href).searchParams.get("home_token") || ""; } catch { return ""; } };
  const H = () => { const t = homeToken(); const h = { accept: "application/json" }; if (t) h["x-elastos-home-token"] = t; return h; };
  // Live = embedded in the runtime (the launcher passes ?home_token). In live mode we NEVER fall back to the
  // demo fixtures (mock.js) — a missing/failed endpoint fails CLOSED to empty, so the UI shows truthful state
  // (e.g. "no listings yet") instead of fabricated assets. Mock fixtures are only for standalone review.
  const liveMode = () => !!homeToken();
  async function tryGet(path) {
    try {
      const r = await fetch(`${base}${path}`, { headers: H() });
      if (!r.ok) throw new Error(String(r.status));
      return await r.json();
    } catch { return null; } // standalone / gateway absent -> mock fallback below
  }
  // The LIVE index Listing is lean (operative_address, token_id, op_type, token_uri; content_id null until
  // KID/metadata enrichment). Normalize it into the shell render shape with graceful fallbacks, and cache by
  // id so the detail view can resolve operative/token_id from the route. A mock/enriched listing passes through.
  const _cache = {};
  function normalize(raw) {
    if (!raw || (raw.name && Array.isArray(raw.listings))) return raw;
    const id = raw.content_id || `${raw.operative_address || ""}:${raw.token_id || ""}`;
    const n = {
      content_id: id, operative_address: raw.operative_address, token_id: raw.token_id,
      channel_address: raw.channel_address, token_uri: raw.token_uri, content_cid: raw.content_cid || null,
      op_type: raw.op_type || "buy_once", name: raw.name || `Asset ${String(raw.token_id || id).slice(0, 12)}`,
      medium: raw.medium || "view", creator_address: raw.creator_address || raw.channel_address || "—",
      category: raw.category || "", image_url: raw.image_url || null,
      // No fabricated tier/holders/copies: only pass through values a real source actually provided
      // (mock listings carry their own; live listings leave these undefined and the UI omits them).
      tier: raw.tier, copies: raw.copies, holders: raw.holders,
      description: raw.description || "", pay_token: raw.pay_token || "",
      // Phase-A card economics: the cheapest active listing's price/supply/resale, attached by the gateway's
      // discovery enrichment (sellersOf+listings). Undefined on rows beyond the per-sweep read budget — the
      // card then shows a neutral placeholder, never a fabricated number.
      price: raw.price, price_formatted: raw.price_formatted, pay_token_symbol: raw.pay_token_symbol,
      for_sale: raw.for_sale, terms_read: raw.terms_read,
      supply_available: raw.supply_available, resale_pct: raw.resale_pct, duration: raw.duration,
      created_at: raw.created_at, preview_url: raw.preview_url, content_type: raw.content_type,
      categories: Array.isArray(raw.categories) ? raw.categories : [], tags: Array.isArray(raw.tags) ? raw.tags : [],
      listings: Array.isArray(raw.listings) ? raw.listings : [], _lean: !raw.name,
    };
    _cache[id] = n; return n;
  }
  const mimeToMedium = (mime) => {
    const t = String(mime || "").split("/")[0];
    if (t === "video") return "watch";
    if (t === "audio") return "listen";
    if (t === "image") return "view";
    if (mime === "application/pdf" || t === "text") return "read";
    if (t === "model") return "explore";
    return null;
  };
  const byId = (id) => _cache[id] || (window.MOCK && window.MOCK.listings.find((x) => x.content_id === id));
  // Ask the Home shell to open another runtime window on our behalf (the runtime's only cross-app launch
  // seam — there is no server-side "pop a window"). Home authorizes this per-source in shell.js
  // (SHELL_MESSAGE_OPEN_TARGET_SOURCES["marketplace-content"]) and re-checks our home_token against our
  // frame route, so this carries no ambient authority (P7/P16): it can only open the targets Home allows.
  // Returns false when we're standalone (no parent / no token) so the caller can fall back truthfully.
  function launch(target, query) {
    try {
      const t = homeToken();
      if (!t || window.parent === window) return false;
      window.parent.postMessage({ type: "home:open-target", target, query: query || {}, homeToken: t }, location.origin);
      return true;
    } catch { return false; }
  }

  return {
    live: false, // flips true if a real gateway answers
    // Your OWN market identity (linked wallet + the handle you set in Home), home-token-gated. Used only to
    // label your own cards with your name instead of a bare address (creator profiles, Phase 0). The address
    // is normalized lower-case by the caller; null in demo/standalone -> the address is shown (fail-closed).
    async me() { const r = await tryGet("/me"); if (r) { this.live = true; } return r || null; },
    async sections({ lean } = {}) {
      const real = await tryGet(`/sections${lean ? "?lean=1" : ""}`);
      if (real) { this.live = true; return (real.sections || []).map((s) => ({ ...s, listings: (s.listings || []).map(normalize) })); }
      if (liveMode()) return [];
      return window.MOCK.sections.map((s) => ({
        ...s,
        listings: s.ids ? s.ids.map(byId) : window.MOCK.listings.filter(s.filter || ((x)=>x.medium===s.id)),
      }));
    },
    async search({ medium, q, op, channel, lean } = {}) {
      const real = await tryGet(`/search?medium=${medium||""}&q=${encodeURIComponent(q||"")}&op=${op||""}&channel=${encodeURIComponent(channel||"")}${lean ? "&lean=1" : ""}`);
      if (real) { this.live = true; let r = (real.listings || []).map(normalize); if (medium) r = r.filter((x) => x.medium === medium); return r; }
      if (liveMode()) return [];
      let r = window.MOCK.listings.slice();
      if (medium) r = r.filter((x) => x.medium === medium);
      if (op) r = r.filter((x) => x.op_type === op);
      if (channel) r = r.filter((x) => (x.channel_address || "").toLowerCase() === channel.toLowerCase());
      if (q) { const s = q.toLowerCase(); r = r.filter((x) => (x.name + x.creator_address).toLowerCase().includes(s)); }
      return r;
    },
    async get(content_id) {
      // LIVE: resolve operative/token_id from the cached listing, then read the on-chain terms via /get.
      const cached = _cache[content_id];
      if (cached && cached.operative_address && cached.token_id) {
        const tu = cached.token_uri ? `&token_uri=${encodeURIComponent(cached.token_uri)}` : "";
        const oc = await tryGet(`/get?operative=${encodeURIComponent(cached.operative_address)}&token_id=${encodeURIComponent(cached.token_id)}${tu}`);
        // Use the LIVE answer whenever the gateway responded with real data — even if there is no active
        // listing (on_chain null). We must NOT discard the real metadata/royalty/supply and fall back to
        // mock zeros just because nobody currently has it listed for sale.
        if (oc && (oc.on_chain || oc.metadata || oc.royalty)) {
          this.live = true;
          const on = oc.on_chain, m = oc.metadata || {};
          // Merge the enriched metadata.json fields (name/cover/content_cid/mime) onto the cached listing.
          const listing = { ...cached,
            name: m.name || cached.name, description: m.description || cached.description,
            image_url: m.image_url || cached.image_url, content_cid: m.content_cid || cached.content_cid,
            medium: mimeToMedium(m.mime_type) || cached.medium,
            // Phase-B Properties panel: the real metadata.json attributes/properties + file size + mime.
            mime_type: m.mime_type || cached.mime_type,
            attributes: Array.isArray(m.attributes) ? m.attributes : cached.attributes,
            properties: Array.isArray(m.properties) ? m.properties : cached.properties,
            media_size: m.media_size != null ? m.media_size : cached.media_size,
            content_type: m.content_type || cached.content_type, preview_url: m.preview_url || cached.preview_url,
            created_at: m.created_at || cached.created_at,
            categories: Array.isArray(m.categories) ? m.categories : cached.categories,
            tags: Array.isArray(m.tags) ? m.tags : cached.tags,
            listings: cached.listings.length ? cached.listings : (on && on.price != null ? [{ price: on.price, seller: on.seller }] : []) };
          _cache[content_id] = listing; // remember the enriched shape (so acquire() has content_cid)
          // on_chain is null when there is no active listing: present a null-safe block + listed flag so
          // the UI shows "not listed for sale" rather than a fabricated 0 USDC / 0 copies.
          const on_chain = on || { token_id: cached.token_id || "—", price: null, pay_token: "", supply_left: null, seller: null, has_access: null };
          // royalty = the REAL per-asset splits read on-chain by the gateway (operative royaltyInfo +
          // resellerCut, CoreStorage protocolShares); null/unavailable -> the UI hides the splits panel.
          return { listing, on_chain, royalty: oc.royalty || null, listed: !!on };
        }
      }
      const listing = byId(content_id);
      if (!listing) return { listing: { content_id, name: "Unknown asset", medium: "view", op_type: "buy_once", listings: [] }, on_chain: { token_id: "—", price: null, pay_token: "", supply_left: null, seller: null, has_access: false }, royalty: null, listed: false };
      // MOCK of the LIVE re-verified on-chain block (Phase-1: never trusted from the cache)
      const cheapest = listing.listings.reduce((a, b) => (b.price < a.price ? b : a), listing.listings[0] || { price: 0 });
      return {
        listing,
        on_chain: {
          token_id: "0x" + String(content_id).slice(2, 10) + "…(real tokenId resolved from chain)",
          price: cheapest.price, pay_token: listing.pay_token,
          supply_left: (listing.copies || 0) - (listing.sold || 0), seller: cheapest.seller,
          has_access: (window.MOCK && window.MOCK.owned.some((o) => o.content_id === content_id)) || false,
        },
        royalty: null, // standalone/demo has no chain read -> the splits panel is hidden (no fabricated splits)
        listed: (listing.listings && listing.listings.length > 0) || false,
      };
    },
    // Resolve the asset's CLEAR DASH preview into an MSE play plan (per-track mime + cached segment URLs).
    // The gateway parses the manifest + warms its byte cache; the shell's standalone MSE player plays it.
    async previewPlan(token_uri) {
      if (!token_uri) return null;
      const r = await tryGet(`/preview/plan?token_uri=${encodeURIComponent(token_uri)}`);
      if (r && Array.isArray(r.tracks) && r.tracks.length) { this.live = true; return r; }
      return null;
    },
    async assembleOrder({ content_id, quantity, seller, price, pay_token }) {
      // BUILT route: POST /api/market/buy (buy_owned_access, Home-gated). Send the asset's on-chain
      // identity from the cached listing so the gateway sources live terms (sellersOf/listings at id=1)
      // with no env pins; price/pay_token arm abort-on-drift (the live re-read must match what was shown).
      const c = _cache[content_id] || {};
      const real = await fetch(`${base}/buy`, {
        method: "POST", headers: { ...H(), "content-type": "application/json" },
        body: JSON.stringify({
          content_id, quantity, seller,
          operative: c.operative_address, token_id: c.token_id, ledger: c.channel_address,
          expected_price: price != null ? String(price) : undefined,
          expected_pay_token: pay_token || undefined,
        }),
      }).then((r) => (r.ok ? r.json() : null)).catch(() => null);
      if (real) { this.live = true; return real; }
      // MOCK unsigned tx — the shell NEVER signs; this is handed to your wallet (human-in-loop).
      return {
        unsigned_tx: {
          to: "AuthorityGateway 0xf758…0ad9", selector: "buyAccess(...)",
          content_id, quantity, seller, value: "(price re-read from chain at assembly)",
          note: "UNSIGNED. Signed only by your wallet after human approve. Phase-1 invariant: "
              + "terms re-verified from chain + aborts on drift before broadcast.",
        },
      };
    },
    async vault() {
      const real = await tryGet("/vault");
      if (real) {
        this.live = true;
        return (real.owned || []).map((o) => {
          // Chain-access-held rows arrive listing-shaped (operative/token_id/token_uri/op_type/medium +
          // enriched name/cover/kid), so they normalize into real cards that open the detail view. Library-
          // only rows (pinned but outside the discovery window) carry just uri/name/content_cid/mime — fall
          // back to a minimal medium from the mime. Either way, preserve the held/acquired flags.
          const n = normalize(o.content_id || o.operative_address ? o : {
            content_id: o.content_cid || o.uri, content_cid: o.content_cid, name: o.name,
            op_type: "free", medium: mimeToMedium(o.mime) || "view",
          });
          n.acquired = o.acquired !== false;
          n.held = o.held !== false;
          if (!n.medium || n.medium === "view") { const m = mimeToMedium(o.mime || o.mime_type); if (m) n.medium = m; }
          n.uri = o.uri; // the Library URI — the open handoff opens this
          return n;
        });
      }
      return liveMode() ? [] : window.MOCK.owned;
    },
    // Hand off to the EXISTING runtime open path (POST /api/viewers/open). The marketplace renders nothing;
    // the runtime gates rights, recovers the CEK in decrypt-provider, and opens the player.
    async open(uri) {
      if (!uri) return null;
      const r = await fetch("/api/viewers/open", {
        method: "POST", headers: { ...H(), "content-type": "application/json" }, body: JSON.stringify({ uri }),
      }).then((r) => (r.ok ? r.json() : null)).catch(() => null);
      if (r) this.live = true;
      return r;
    },
    // Open the downloaded asset in its runtime player via Home's launch seam — the SAME path the Library app
    // uses (openTarget(viewer, {objectUri,…})). The viewer capsule itself runs the rights/decrypt open; the
    // marketplace renders nothing and holds no CEK (P15/P16). `medium` chooses elacity-player (av) vs
    // ddrm-viewer (everything else), mirroring the Library's viewer routing. Returns false if standalone.
    openInPlayer({ uri, name, mime, medium, content_cid }) {
      if (!uri) return false;
      const viewer = medium === "watch" || medium === "listen" ? "elacity-player" : "ddrm-viewer";
      const query = { objectUri: uri, uri, name: name || "", mime: mime || "application/octet-stream" };
      if (content_cid) query.contentCid = content_cid;
      return launch(viewer, query);
    },
    // Reveal the downloaded file in the File Explorer (the Library app), opening its containing folder —
    // the runtime equivalent of PC2's openFolder. We open the PARENT directory so the file shows in context
    // (the buyer's `…/Acquired` space). Returns false if standalone (no Home parent).
    reveal(uri) {
      if (!uri) return false;
      const cut = uri.lastIndexOf("/");
      const folder = cut > "localhost://".length ? uri.slice(0, cut) : uri;
      return launch("library", { uri: folder });
    },
    async listed() {
      const r = await tryGet("/listed");
      if (r) {
        this.live = true;
        // Live rows carry my_price in minor units + my_price_formatted (human) + the operative for withdraw.
        return (r.listed || []).map((it) => ({
          ...it,
          medium: it.medium || mimeToMedium(it.mime_type || it.mime) || "view",
          my_price: it.my_price_formatted != null ? it.my_price_formatted : it.my_price,
          content_id: it.content_id || `${it.operative_address || ""}:${it.token_id || ""}`,
        }));
      }
      return liveMode() ? [] : window.MOCK.listed;
    },
    // Marketplace-wide recent on-chain activity (ItemListed/ItemSold/ItemUnlisted), read live from the
    // AuthorityGateway logs by the gateway. Falls back to the demo feed only when no gateway answers.
    async history() { const r = await tryGet("/history"); if (r) { this.live = true; return r.history || []; } return liveMode() ? [] : window.MOCK.history; },
    // One asset's on-chain trade history (same events, filtered to its operative). [] when none/standalone.
    async assetHistory(operative, token_id) {
      if (!operative) return [];
      const tid = token_id ? `&token_id=${encodeURIComponent(token_id)}` : "";
      const r = await tryGet(`/history?operative=${encodeURIComponent(operative)}${tid}`);
      if (r) { this.live = true; return r.history || []; }
      return [];
    },
    async assembleCancel({ operative, quantity }) {
      // BUILT route: POST /api/market/order/withdraw {operative, token_id, quantity}. The listing is keyed
      // at the ERC-1155 ACCESS_TOKEN id (== 1), so withdraw passes token_id "1" + the listed quantity.
      const real = await fetch(`${base}/order/withdraw`, {
        method: "POST", headers: { ...H(), "content-type": "application/json" },
        body: JSON.stringify({ operative, token_id: "1", quantity: String(quantity || 1) }),
      }).then((r) => (r.ok ? r.json() : null)).catch(() => null);
      if (real) { this.live = true; return real; }
      return { unsigned_tx: { to: "AuthorityGateway", selector: "withdrawListing(operative,tokenId,quantity) 0x3e65bbba", operative, token_id: "1", quantity: String(quantity || 1),
        note: "UNSIGNED — routed to wallet; the access right is unaffected, only the resale listing is cancelled." } };
    },
    // Buy -> pin: after the access right is granted, TRIGGER (never perform) the encrypted asset's pin into
    // the local Library via content/*, then it's openable from the runtime player. BUILT route (Home-gated):
    // it gates hasAccessByContentId(content_id) then dispatches the Acquire op. content_id = the bytes16
    // KID (entitlement); content_cid = the encrypted IPFS CID (what is pinned).
    async acquire({ content_id, content_cid, token_uri, metadata, background }) {
      const real = await fetch(`${base}/acquire`, {
        method: "POST", headers: { ...H(), "content-type": "application/json" },
        body: JSON.stringify({ content_id, content_cid, token_uri, metadata, background: !!background }),
      }).then((r) => (r.ok ? r.json() : null)).catch(() => null);
      if (real) { this.live = true; return real; }
      // Fail CLOSED in live mode: a failed gateway acquire must NOT fabricate a Library URI. A fake
      // `localhost://Users/<placeholder>/…` path would send Reveal/Open to a phantom folder OUTSIDE your
      // real principal root (the trusted core then rejects it as "outside the active principal root").
      if (liveMode()) return null;
      // Standalone demo only (no gateway): acknowledge the trigger WITHOUT a navigable Library URI — nothing
      // is actually materialized, so Reveal/Open correctly report "download it to your node first".
      return { content_id, pin_status: "complete",
        note: "Standalone demo — no gateway. In the runtime this pins the encrypted CID (content/ensure) and registers a Library object; the marketplace holds no keys (P15)." };
    },
    // Truthful download state for a BACKGROUND acquire — derived server-side from the materialized file in
    // your Acquired space (durable truth) and the in-flight run (running/failed). No fabricated %. Returns
    // { state: "downloaded"|"downloading"|"failed"|"idle", downloaded, uri?, message? } or null standalone.
    async acquireStatus({ cid, token_uri }) {
      const tu = token_uri ? `&token_uri=${encodeURIComponent(token_uri)}` : "";
      const r = await tryGet(`/acquire-status?cid=${encodeURIComponent(cid || "")}${tu}`);
      if (r) this.live = true;
      return r || null;
    },
  };
})();
