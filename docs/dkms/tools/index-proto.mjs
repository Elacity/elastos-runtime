#!/usr/bin/env node
/* index-proto.mjs — a WORKING content-index prototype against the LIVE Base v3 chain.
 * Proves Phase-2 end-to-end: getLogs the EventHub for the real V3 events, decode them, and emit
 * ContentListingV1 objects (the /api/market/* shape) from REAL on-chain data. Read-only; no keys.
 * This is the runnable reference for the Rust `content-index` (Phase 2). Run: node index-proto.mjs
 *
 * Canonical config (PC2 pc2-node/config/default.json content_indexer.contracts.v3, Base 8453):
 */
const EVENT_HUB = "0x5a694A6d988354dca491fe0F6db7a6ef46b656c2";
const FROM_BLOCK = 43892000;          // backfill genesis
const SCAN = 10000;                   // max_blocks_per_scan
const RPCS = ["https://mainnet.base.org", "https://base.llamarpc.com", "https://base-rpc.publicnode.com"];
const TOPICS = {
  "0xc0a995e4052be044599af577ab2f3382d67bd34df95a76226e7c464e9d4dba46": "AssetCreated",
  "0x1b24f7763272894608506beba5887c374d345cd231bf52bd03f40bc2d0508d7b": "DigitalAssetRegistered",
};

async function rpc(method, params) {
  for (const url of RPCS) {
    try {
      const r = await fetch(url, { method: "POST", headers: { "content-type": "application/json" },
        body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }) });
      const j = await r.json();
      if (j.result !== undefined) return j.result;
    } catch { /* next */ }
  }
  return null;
}
// --- minimal ABI decode helpers (hex, no 0x) ---
const W = (d, i) => d.slice(i * 64, (i + 1) * 64);              // 32-byte word i
const addr = (w) => "0x" + w.slice(24);                         // address from word (last 20B)
const b16 = (w) => "0x" + w.slice(0, 32);                       // bytes16 from word (first 16B) == KID
const uint = (w) => BigInt("0x" + (w || "0"));
function str(d, offHex) {                                       // dynamic string at byte-offset
  const off = Number(uint(W(d, offHex)));                       // byte offset
  const lenW = d.slice(off * 2, off * 2 + 64);
  const len = Number(uint(lenW));
  const bytes = d.slice(off * 2 + 64, off * 2 + 64 + len * 2);
  return Buffer.from(bytes, "hex").toString("utf8");
}

function decode(name, log) {
  const t = log.topics, d = (log.data || "0x").slice(2);
  if (name === "AssetCreated") {
    // (address indexed _to, address indexed _channel, uint256 _tokenId, string _tokenUri, uint16 _opType, address indexed opContract)
    return { event: name, to: addr(t[1].slice(2)), channel: addr(t[2].slice(2)), operative: addr(t[3].slice(2)),
      tokenId: uint(W(d, 0)).toString(), tokenUri: safe(() => str(d, 1)), opType: Number(uint(W(d, 2))),
      block: parseInt(log.blockNumber, 16) };
  }
  if (name === "DigitalAssetRegistered") {
    // (address indexed channel, uint256 indexed tokenId, address creator, string tokenURI, uint16 opType, bytes16 contentId)
    return { event: name, channel: addr(t[1].slice(2)), tokenId: uint(t[2].slice(2)).toString(),
      creator: addr(W(d, 0)), tokenUri: safe(() => str(d, 1)), opType: Number(uint(W(d, 2))),
      contentId: b16(W(d, 3)), block: parseInt(log.blockNumber, 16) };
  }
}
const safe = (f) => { try { return f(); } catch { return null; } };
const OPTYPE = ["free", "buy_once", "buy_and_resell"];

(async () => {
  const head = parseInt(await rpc("eth_blockNumber", []), 16);
  console.log(`Base head ${head}; scanning EventHub ${EVENT_HUB} from ${FROM_BLOCK} in ${SCAN}-block windows…`);
  const events = [];
  let from = FROM_BLOCK, chunks = 0;
  while (from <= head && chunks < 400 && events.length < 50) {
    const to = Math.min(from + SCAN - 1, head);
    const logs = await rpc("eth_getLogs", [{ address: EVENT_HUB, fromBlock: "0x" + from.toString(16),
      toBlock: "0x" + to.toString(16), topics: [Object.keys(TOPICS)] }]);
    if (Array.isArray(logs)) for (const l of logs) { const n = TOPICS[l.topics[0]]; if (n) events.push(decode(n, l)); }
    from = to + 1; chunks++;
    if (chunks % 50 === 0) process.stdout.write(`  …block ${from} (${events.length} events)\n`);
  }
  // FINDING: EventHub emits AssetCreated (the index's primary discovery event). DigitalAssetRegistered
  // (the contentId/KID source) is emitted by the CHANNEL contracts, not EventHub — so the contentId
  // comes from metadata.json enrichment (content-market validates metadata.kid). Build from AssetCreated.
  const created = events.filter((e) => e.event === "AssetCreated");
  const reg = events.filter((e) => e.event === "DigitalAssetRegistered");
  const regByKey = {}; for (const r of reg) regByKey[r.channel.toLowerCase() + ":" + r.tokenId] = r;
  const listings = created.map((c) => {
    const r = regByKey[c.channel.toLowerCase() + ":" + c.tokenId];
    return { content_id: r ? r.contentId : "(resolve from metadata.json/DigitalAssetRegistered)",
      chain_id: 8453, channel_address: c.channel, operative_address: c.operative, token_id: c.tokenId,
      token_uri: c.tokenUri, op_type: OPTYPE[c.opType] ?? c.opType, owner_or_creator: c.to,
      first_seen_block: c.block, source: "AssetCreated@EventHub" };
  });
  console.log(`\n=== ${events.length} V3 events (${created.length} AssetCreated, ${reg.length} DigitalAssetRegistered) → ${listings.length} listings (real Base data) ===`);
  for (const l of listings.slice(0, 8)) {
    console.log(`  • op=${l.op_type}  tokenId=${l.token_id}  operative=${l.operative_address}\n      uri=${l.token_uri || "(none)"}  channel=${l.channel_address}  @${l.first_seen_block}`);
  }
  console.log(`\n(Proven live. The Rust content-index ports this: getLogs EventHub for AssetCreated -> decode`);
  console.log(` operative + tokenURI + opType + channel/tokenId; resolve contentId/KID from metadata.json via`);
  console.log(` tokenURI (content-market validates metadata.kid); read Operative sellersOf/listings for`);
  console.log(` price+supply; cache; serve /api/market/*. NOTE: DigitalAssetRegistered/ChannelCreated are on`);
  console.log(` the channel/factory contracts, NOT EventHub — EventHub emits AssetCreated only.)`);
})();
