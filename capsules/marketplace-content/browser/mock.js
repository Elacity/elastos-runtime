/* mock.js — embedded sample data so the shell runs STANDALONE (open index.html).
 * It also DOCUMENTS the elastos://market/* listing shape the index/gateway must serve.
 * Every field except {tier,medium,listings,resale_floor,holders} comes from content-market's
 * decode; those few are index-derived. content_id == bytes16 KID (the trust anchor).
 * Pay token on Base = USDC (6 decimals) 0x833589fC… — confirmed against PC2 v3 + the gateway. */
window.MOCK = (function () {
  const USDC = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"; // Base USDC — the confirmed pay token (gas = ETH)
  function L(o) {
    const base = {
      schema: "elastos.market.listing/v1",
      channel_address: "0x6756…b8b9", chain_id: 8453, token_uri: "ipfs://bafy…/metadata.json",
      metadata_cid: "bafy…", pay_token: USDC, mime_type: "", asset_type: "", kind: o.kind || "content", // kind: content|app|game (apps/games later)
      creator_address: o.creator || "0x4f…a2", metadata_status: "resolved",
      op_type_code: { free: 0, buy_once: 1, buy_and_resell: 2 }[o.op_type],
      listings: [], resale_floor: null, holders: o.holders || 0, sold: o.sold || 0,
    };
    const merged = Object.assign(base, o);
    // primary listing + optional resale listings (for cheapest-default + vendor selector)
    merged.listings = [{ seller: "primary", price: o.price ?? 0, copies_left: (o.copies||0)-(o.sold||0) }];
    if (o.op_type === "buy_and_resell" && o.price) {
      merged.listings.push({ seller: "0x91…7c", price: +(o.price * 0.92).toFixed(2), copies_left: 3 });
      merged.resale_floor = +(o.price * 0.92).toFixed(2);
    }
    return merged;
  }
  const listings = [
    L({ content_id: "0x9c2a000000000000000000000000e1a1", name: "Aerials — Episode 1", medium: "watch", tier: 1, op_type: "buy_and_resell", price: 4.0, copies: 500, sold: 388, holders: 388, description: "A short aerial film. ISCC-fingerprinted. Pins to your library on purchase; opens in your player." }),
    L({ content_id: "0x7b11000000000000000000000000c0fe", name: "Nocturne (LP)", medium: "listen", tier: 1, op_type: "buy_once", price: 2.5, copies: 1000, sold: 210, creator: "lumen" }),
    L({ content_id: "0x33aa00000000000000000000000010ff", name: "The Long Field", medium: "read", tier: 2, op_type: "free", price: 0, copies: 0, creator: "hatch", description: "A novella (PDF), pixel-locked reader. Free to open." }),
    L({ content_id: "0xd1ce000000000000000000000000beef", name: "Relic — scan #12", medium: "explore", tier: 5, op_type: "buy_and_resell", price: 9.0, copies: 50, sold: 12, creator: "atlas", description: "A 3D scan (glTF). Orbit-only secure preview." }),
    L({ content_id: "0x5501000000000000000000000000a17e", name: "Solar / 03", medium: "view", tier: 2, op_type: "buy_once", price: 1.2, copies: 200, sold: 41, creator: "mira" }),
    L({ content_id: "0x6f20000000000000000000000000c01d", name: "Field Notes", medium: "read", tier: 2, op_type: "buy_once", price: 3.3, copies: 300, sold: 96, creator: "koto", description: "Comic (CBZ), pixel-locked pager." }),
    L({ content_id: "0x8e44000000000000000000000000dr1f", name: "Drift — short", medium: "watch", tier: 1, op_type: "free", price: 0, copies: 0, creator: "0x91…7c" }),
    L({ content_id: "0x2c77000000000000000000000000e3ad", name: "Loops vol.2", medium: "listen", tier: 1, op_type: "buy_and_resell", price: 0.8, copies: 800, sold: 305, creator: "ember" }),
  ];
  const owned = [listings[0], listings[2], listings[4]]; // mock "My Vault"
  const listed = [{ ...listings[0], listing_id: "0xL1", my_price: 3.7, my_qty: 2 }]; // you listed 2 copies of Aerials for resale
  const history = [
    { type: "buy", name: "Aerials — Episode 1", value: "−4.0 USDC", when: "2d ago" },
    { type: "buy", name: "The Long Field", value: "Free", when: "5d ago" },
    { type: "resold", name: "Loops vol.2", value: "+0.74 USDC net", when: "1w ago" },
    { type: "royalty", name: "Solar / 03", value: "+0.18 USDC", when: "1w ago" },
  ];
  const sections = [
    { id: "trending", title: "Trending", ids: listings.slice(0, 4).map(x => x.content_id) },
    { id: "new", title: "New mints", ids: listings.slice(4).map(x => x.content_id) },
    { id: "free", title: "Free to open", filter: x => x.op_type === "free" },
    { id: "resell", title: "Resellable rights", filter: x => x.op_type === "buy_and_resell" },
    { id: "watch", title: "Watch" }, { id: "read", title: "Read" },
  ];
  return { listings, owned, listed, history, sections };
})();
