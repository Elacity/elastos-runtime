//! Content-index: the marketplace discovery cache (Phase 2).
//!
//! A re-derivable view over the chain's `AssetCreated` events (the only mint event that emits on
//! Base — EventHub, `0xc0a995e4…`). PURE + read-only: it decodes logs into browsable `Listing` rows
//! and answers search/sections/get. It holds NO keys, NO RPC of its own, NO write authority (P3/P16) —
//! the gateway feeds it logs fetched through `chain-provider` (the sole RPC declarant). The money path
//! never trusts this cache: buy re-verifies terms live (Phase-1 abort-on-drift).
//!
//! `AssetCreated(address indexed _to, address indexed _channel, uint256 _tokenId, string _tokenUri,
//! uint16 _opType, address indexed opContract)` carries no `contentId` — so a row's `content_id` (KID)
//! is DEFERRED (`metadata_status: "needs_kid"`) until enrichment (the mint calldata / `metadata.json`).
//! Discovery keys on `(channel, tokenId)`; the buy resolves the KID via `resolve_token_id`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// `AssetCreated` topic0 (keccak of the signature) — must match `chain-provider`.
pub(crate) const ASSET_CREATED_TOPIC0: &str =
    "0xc0a995e4052be044599af577ab2f3382d67bd34df95a76226e7c464e9d4dba46";

/// EventHub (Base) — the contract that emits `AssetCreated`; the index's getLogs source
/// (`pc2-node config/default.json content_indexer.contracts.v3.event_hub`).
pub(crate) const EVENT_HUB: &str = "0x5a694A6d988354dca491fe0F6db7a6ef46b656c2";

/// The EventHub deployment block on Base — the backfill floor. No `AssetCreated` exists below it,
/// so the backfill lane stops here (`pc2-node config/default.json content_indexer.contracts.v3.from_block`).
pub(crate) const EVENT_HUB_DEPLOY_BLOCK: u64 = 43_892_000;

/// Blocks re-scanned behind the cursor each delta cycle so a shallow head reorg re-derives its rows
/// (`upsert` is idempotent, so the overlap is pure insurance, PHASE2_INDEX_AND_API.md chunk 2).
pub(crate) const REORG_OVERLAP_BLOCKS: u64 = 120;

/// op-type codes carried by the mint / `AssetCreated._opType`.
fn op_type_name(code: u64) -> &'static str {
    match code {
        0 => "free",
        1 => "buy_once",
        2 => "buy_and_resell",
        _ => "unknown",
    }
}

/// A browsable marketplace listing row, re-derived from one `AssetCreated` log.
/// `Serialize`/`Deserialize` back the on-disk index snapshot (cold-start warm cache); the fields ARE the
/// snapshot schema, so renaming one is a snapshot-format change (old snapshots just fail to parse → rebuild).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Listing {
    pub channel_address: String,
    pub operative_address: String,
    pub token_id: String,
    pub token_uri: String,
    pub op_type: String,
    pub creator_address: String,
    pub first_seen_block: u64,
    /// `"needs_kid"` until the KID/contentId is enriched (AssetCreated carries none).
    pub metadata_status: String,
}

impl Listing {
    fn key(&self) -> String {
        format!("{}:{}", self.channel_address.to_lowercase(), self.token_id)
    }

    pub(crate) fn to_json(&self) -> Value {
        json!({
            "schema": "elastos.market.listing/v1",
            "chain_id": 8453,
            "channel_address": self.channel_address,
            "operative_address": self.operative_address,
            "token_id": self.token_id,
            "token_uri": self.token_uri,
            "op_type": self.op_type,
            "creator_address": self.creator_address,
            "first_seen_block": self.first_seen_block,
            "metadata_status": self.metadata_status,
            // content_id (== bytes16 KID) is resolved at enrich/buy time, not from AssetCreated.
            "content_id": Value::Null,
        })
    }
}

/// The in-memory discovery cache: newest-first listings, deduped by `(channel, tokenId)`.
///
/// Snapshot v2 adds the two persistent cursors (PHASE2_INDEX_AND_API.md chunk 2): the covered
/// contiguous block range is `[backfill_low, scanned_to]`. Both default to 0 so a v1 snapshot
/// (listings only) still parses — 0 means "cursor unset", and the next advance cycle seeds it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ContentIndex {
    listings: Vec<Listing>,
    /// Highest block the delta lane has scanned (top of the covered range). 0 = never scanned.
    #[serde(default)]
    scanned_to: u64,
    /// Lowest block the backfill lane has reached (bottom of the covered range). 0 = never scanned.
    /// Backfill is COMPLETE once this is at/below `EVENT_HUB_DEPLOY_BLOCK`.
    #[serde(default)]
    backfill_low: u64,
}

impl ContentIndex {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn scanned_to(&self) -> u64 {
        self.scanned_to
    }

    pub(crate) fn backfill_low(&self) -> u64 {
        self.backfill_low
    }

    /// True once a cursor exists (some contiguous range is covered).
    pub(crate) fn cursor_set(&self) -> bool {
        self.scanned_to > 0 && self.backfill_low > 0
    }

    /// True once the backfill lane has reached the EventHub deployment block — full history covered.
    pub(crate) fn backfill_complete(&self) -> bool {
        self.cursor_set() && self.backfill_low <= EVENT_HUB_DEPLOY_BLOCK
    }

    /// The honest coverage label served with every discovery response: full history (`indexed`),
    /// cursor advancing but history incomplete (`indexing`), or the legacy bounded window.
    pub(crate) fn coverage(&self) -> &'static str {
        if self.backfill_complete() {
            "indexed"
        } else if self.cursor_set() {
            "indexing"
        } else {
            "recent-window"
        }
    }

    /// Seed the cursor after the FIRST full sweep covered `[low, high]`.
    pub(crate) fn seed_cursor(&mut self, low: u64, high: u64) {
        self.backfill_low = low.max(1);
        self.scanned_to = high.max(self.backfill_low);
    }

    /// Advance the delta lane's top-of-range after a `[.., to]` head scan.
    pub(crate) fn note_delta_scanned(&mut self, to: u64) {
        self.scanned_to = self.scanned_to.max(to);
    }

    /// Lower the backfill lane's bottom-of-range after a `[low, ..]` history scan.
    pub(crate) fn note_backfilled(&mut self, low: u64) {
        let low = low.max(1);
        if self.backfill_low == 0 || low < self.backfill_low {
            self.backfill_low = low;
        }
    }

    /// Insert or replace a listing (keyed by `(channel, tokenId)`); a re-mint of the same id wins by
    /// the higher `first_seen_block`. Returns true if the row was added or updated.
    pub(crate) fn upsert(&mut self, listing: Listing) -> bool {
        let key = listing.key();
        if let Some(existing) = self.listings.iter_mut().find(|l| l.key() == key) {
            if listing.first_seen_block >= existing.first_seen_block {
                *existing = listing;
                return true;
            }
            return false;
        }
        self.listings.push(listing);
        true
    }

    /// Ingest a batch of `eth_getLogs` entries, decoding the `AssetCreated` ones (others ignored,
    /// fail-soft per-entry). Returns the count upserted.
    pub(crate) fn ingest_logs(&mut self, logs: &[Value]) -> usize {
        let mut n = 0;
        for log in logs {
            if let Some(listing) = decode_asset_created(log) {
                if self.upsert(listing) {
                    n += 1;
                }
            }
        }
        self.sort_newest_first();
        n
    }

    fn sort_newest_first(&mut self) {
        self.listings
            .sort_by(|a, b| b.first_seen_block.cmp(&a.first_seen_block));
    }

    /// Filtered search. `op` filters by op-type; `channel` filters to one creator's channel (for the
    /// "More from this creator" rail); `q` is a case-insensitive substring over creator/token_uri/channel.
    /// Newest-first.
    pub(crate) fn search(
        &self,
        op: Option<&str>,
        q: Option<&str>,
        channel: Option<&str>,
    ) -> Vec<&Listing> {
        let ql = q.map(|s| s.to_lowercase());
        self.listings
            .iter()
            .filter(|l| op.map(|o| l.op_type == o).unwrap_or(true))
            .filter(|l| {
                channel
                    .map(|c| l.channel_address.eq_ignore_ascii_case(c))
                    .unwrap_or(true)
            })
            .filter(|l| {
                ql.as_ref()
                    .map(|q| {
                        l.creator_address.to_lowercase().contains(q)
                            || l.token_uri.to_lowercase().contains(q)
                            || l.channel_address.to_lowercase().contains(q)
                    })
                    .unwrap_or(true)
            })
            .collect()
    }

    /// Discovery shelves (the `/api/market/sections` shape): newest, free, resellable.
    pub(crate) fn sections(&self) -> Value {
        let pick = |f: &dyn Fn(&Listing) -> bool| -> Vec<Value> {
            self.listings
                .iter()
                .filter(|l| f(l))
                .take(24)
                .map(Listing::to_json)
                .collect()
        };
        json!({
            "sections": [
                { "id": "new", "title": "New mints", "listings": pick(&|_| true) },
                { "id": "free", "title": "Free to open", "listings": pick(&|l| l.op_type == "free") },
                { "id": "resell", "title": "Resellable rights", "listings": pick(&|l| l.op_type == "buy_and_resell") },
            ]
        })
    }

    pub(crate) fn len(&self) -> usize {
        self.listings.len()
    }
}

// --- pure ABI decode helpers (no external crate) ---

fn topic_str(log: &Value, i: usize) -> Option<&str> {
    log.get("topics")?.as_array()?.get(i)?.as_str()
}

/// 32-byte indexed topic -> `0x` + last 20 bytes (an address), lowercased.
fn addr_from_topic(topic: &str) -> Option<String> {
    let clean = topic.strip_prefix("0x").unwrap_or(topic);
    if clean.len() != 64 || !clean.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("0x{}", &clean[24..].to_lowercase()))
}

/// data word `i` (32 bytes -> 64 hex), from the `0x`-prefixed data blob.
fn data_word(data_clean: &str, i: usize) -> Option<&str> {
    data_clean.get(i * 64..(i + 1) * 64)
}

fn word_to_u64(word: &str) -> Option<u64> {
    // low 16 hex are plenty for opType/small ints; high bits must be zero for a clean parse.
    let (hi, lo) = word.split_at(word.len().saturating_sub(16));
    if hi.bytes().any(|b| b != b'0') {
        return None;
    }
    u64::from_str_radix(lo, 16).ok()
}

/// Decode the dynamic `string` at byte-offset `off` within the data blob. Fail-soft (`None`) on any
/// malformed/hostile offset or length: a hostile log word can be up to `u64::MAX`, which would overflow
/// `off*2` / `len*2` and PANIC in a debug build (unwinding the whole ingest batch), so all index
/// arithmetic is checked; the length is also capped at `MAX_ABI_STRING_BYTES` so a forged length cannot
/// drive a huge `Vec` allocation.
fn abi_string(data_clean: &str, off: usize) -> Option<String> {
    const MAX_ABI_STRING_BYTES: usize = 64 * 1024; // a tokenURI/string field is small; cap defensively
    let len_at = off.checked_mul(2)?;
    let len_end = len_at.checked_add(64)?;
    let len = u64::from_str_radix(data_clean.get(len_at..len_end)?, 16).ok()? as usize;
    if len > MAX_ABI_STRING_BYTES {
        return None;
    }
    let bytes_at = len_end;
    let hex = data_clean.get(bytes_at..bytes_at.checked_add(len.checked_mul(2)?)?)?;
    let mut bytes = Vec::with_capacity(len);
    let raw = hex.as_bytes();
    let mut i = 0;
    while i + 1 < raw.len() {
        let h = u8::from_str_radix(std::str::from_utf8(&raw[i..i + 2]).ok()?, 16).ok()?;
        bytes.push(h);
        i += 2;
    }
    String::from_utf8(bytes).ok()
}

/// Decode one `AssetCreated` log into a `Listing`. `None` on a foreign/malformed entry (fail-soft).
pub(crate) fn decode_asset_created(log: &Value) -> Option<Listing> {
    let t0 = topic_str(log, 0)?;
    if !t0.eq_ignore_ascii_case(ASSET_CREATED_TOPIC0) {
        return None;
    }
    let creator = addr_from_topic(topic_str(log, 1)?)?; // _to (the recipient/creator)
    let channel = addr_from_topic(topic_str(log, 2)?)?;
    let operative = addr_from_topic(topic_str(log, 3)?)?; // opContract (4th indexed)
    let data = log.get("data").and_then(Value::as_str)?;
    let data_clean = data.strip_prefix("0x").unwrap_or(data);
    let token_id = format!("0x{}", data_word(data_clean, 0)?);
    let op_type = op_type_name(word_to_u64(data_word(data_clean, 2)?)?);
    // _tokenUri is the 2nd non-indexed param: word 1 is its byte-offset into data.
    let uri_off = word_to_u64(data_word(data_clean, 1)?)? as usize;
    let token_uri = abi_string(data_clean, uri_off).unwrap_or_default();
    let first_seen_block = log
        .get("blockNumber")
        .and_then(Value::as_str)
        .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0);
    Some(Listing {
        channel_address: channel,
        operative_address: operative,
        token_id,
        token_uri,
        op_type: op_type.to_string(),
        creator_address: creator,
        first_seen_block,
        metadata_status: "needs_kid".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // A well-formed AssetCreated log (channel, operative, tokenId, tokenURI, opType).
    fn ac_log(
        channel: &str,
        operative: &str,
        token_id: u64,
        uri: &str,
        op: u64,
        block: u64,
    ) -> Value {
        let pad_addr = |a: &str| format!("{:0>64}", a.trim_start_matches("0x").to_lowercase());
        // data: [0]=tokenId, [1]=uri offset(0x60=96), [2]=opType, then at 96: len + bytes.
        let uri_hex: String = uri.bytes().map(|b| format!("{b:02x}")).collect();
        let uri_len = format!("{:064x}", uri.len());
        let uri_padded = format!("{uri_hex:0<width$}", width = uri.len().div_ceil(32) * 64);
        let tid = format!("{token_id:064x}");
        let off = format!("{:064x}", 96);
        let opt = format!("{op:064x}");
        let data = format!("0x{tid}{off}{opt}{uri_len}{uri_padded}");
        json!({
            "topics": [
                ASSET_CREATED_TOPIC0,
                pad_addr("0x1111111111111111111111111111111111111111"), // _to / creator
                pad_addr(channel),
                pad_addr(operative),
            ],
            "data": data,
            "blockNumber": format!("0x{block:x}"),
        })
    }

    #[test]
    fn decodes_asset_created_into_a_listing() {
        let log = ac_log(
            "0x6756e1407164ae34f8df5334d48d0e45c094b8b9",
            "0x483adcf310d9344cc017536810d65a87ebcc1760",
            7,
            "ipfs://bafy/metadata.json",
            2,
            0x100,
        );
        let l = decode_asset_created(&log).expect("valid AssetCreated decodes");
        assert_eq!(
            l.channel_address,
            "0x6756e1407164ae34f8df5334d48d0e45c094b8b9"
        );
        assert_eq!(
            l.operative_address,
            "0x483adcf310d9344cc017536810d65a87ebcc1760"
        );
        assert_eq!(l.token_id, format!("0x{:064x}", 7));
        assert_eq!(l.token_uri, "ipfs://bafy/metadata.json");
        assert_eq!(l.op_type, "buy_and_resell");
        assert_eq!(l.first_seen_block, 0x100);
        assert_eq!(l.metadata_status, "needs_kid");
        // foreign topic -> None
        assert!(decode_asset_created(&json!({ "topics": ["0xdeadbeef"], "data": "0x" })).is_none());
    }

    #[test]
    fn ingest_dedup_search_and_sections() {
        let mut idx = ContentIndex::new();
        let chan_a = "0xaaaa000000000000000000000000000000000001";
        let chan_b = "0xbbbb000000000000000000000000000000000002";
        let op_a = "0xa0a0000000000000000000000000000000000001";
        let op_b = "0xb0b0000000000000000000000000000000000002";
        let logs = vec![
            ac_log(chan_a, op_a, 1, "ipfs://a", 0, 10),
            ac_log(chan_b, op_b, 2, "ipfs://b", 2, 20),
            // re-mint of (chanA, token 1) at a higher block -> replaces, not duplicates.
            ac_log(chan_a, op_a, 1, "ipfs://a2", 0, 30),
        ];
        idx.ingest_logs(&logs);
        assert_eq!(idx.len(), 2, "dedup by (channel, tokenId)");
        // newest-first
        let all = idx.search(None, None, None);
        assert_eq!(all[0].first_seen_block, 30);
        // op filter
        assert_eq!(idx.search(Some("free"), None, None).len(), 1);
        assert_eq!(idx.search(Some("buy_and_resell"), None, None).len(), 1);
        // q filter (token_uri substring) — the re-mint updated chanA's uri to a2
        assert_eq!(idx.search(None, Some("ipfs://a2"), None).len(), 1);
        assert!(idx.search(None, Some("nomatch"), None).is_empty());
        // channel filter (More-from-creator rail) — only chanA's listing
        assert_eq!(idx.search(None, None, Some(chan_a)).len(), 1);
        assert_eq!(idx.search(None, None, Some(chan_b)).len(), 1);
        assert!(idx
            .search(
                None,
                None,
                Some("0xc0c0000000000000000000000000000000000003")
            )
            .is_empty());
        // sections shape
        let s = idx.sections();
        let secs = s["sections"].as_array().unwrap();
        assert_eq!(secs.len(), 3);
        assert_eq!(secs[1]["id"], "free");
    }

    #[test]
    fn index_snapshot_round_trips_through_serde() {
        // The on-disk cold-start snapshot must serialize and parse back identically (same listings, order).
        let mut idx = ContentIndex::new();
        idx.ingest_logs(&[
            ac_log(
                "0xaaaa000000000000000000000000000000000001",
                "0xa0a0000000000000000000000000000000000001",
                1,
                "ipfs://a",
                0,
                10,
            ),
            ac_log(
                "0xbbbb000000000000000000000000000000000002",
                "0xb0b0000000000000000000000000000000000002",
                2,
                "ipfs://b",
                2,
                20,
            ),
        ]);
        let bytes = serde_json::to_vec(&idx).expect("serialize snapshot");
        let restored: ContentIndex = serde_json::from_slice(&bytes).expect("parse snapshot");
        assert_eq!(restored.len(), idx.len());
        // newest-first order + fields preserved
        let a = idx.search(None, None, None);
        let b = restored.search(None, None, None);
        assert_eq!(a, b);
        assert_eq!(b[0].first_seen_block, 20);
    }

    #[test]
    fn cursor_seed_advance_and_coverage_labels() {
        let mut idx = ContentIndex::new();
        // No cursor: legacy label.
        assert!(!idx.cursor_set());
        assert_eq!(idx.coverage(), "recent-window");
        // First sweep covered [head-10k, head] well above the deploy block: indexing.
        let head = EVENT_HUB_DEPLOY_BLOCK + 1_000_000;
        idx.seed_cursor(head - 9_999, head);
        assert!(idx.cursor_set());
        assert!(!idx.backfill_complete());
        assert_eq!(idx.coverage(), "indexing");
        // Delta lane only moves the top upward (a lower value is a no-op).
        idx.note_delta_scanned(head + 5_000);
        idx.note_delta_scanned(head);
        assert_eq!(idx.scanned_to(), head + 5_000);
        // Backfill lane only moves the bottom downward (a higher value is a no-op).
        idx.note_backfilled(head - 50_000);
        idx.note_backfilled(head);
        assert_eq!(idx.backfill_low(), head - 50_000);
        // Reaching the deploy block flips coverage to the full-history label.
        idx.note_backfilled(EVENT_HUB_DEPLOY_BLOCK);
        assert!(idx.backfill_complete());
        assert_eq!(idx.coverage(), "indexed");
    }

    #[test]
    fn v1_snapshot_without_cursors_still_parses() {
        // A pre-cursor (v1) on-disk snapshot carries only `listings`; serde defaults must accept it
        // and report an unset cursor so the next advance cycle seeds it instead of trusting garbage.
        let v1 = json!({ "listings": [] }).to_string();
        let idx: ContentIndex = serde_json::from_str(&v1).expect("v1 snapshot parses");
        assert!(!idx.cursor_set());
        assert_eq!(idx.coverage(), "recent-window");
    }

    #[test]
    fn abi_string_fails_soft_on_hostile_offset_and_length() {
        // A hostile offset whose *2 would overflow usize must return None, never panic (debug builds
        // would otherwise panic on arithmetic overflow and unwind the whole ingest batch).
        assert!(abi_string("0000", usize::MAX).is_none());
        // A forged huge length word at offset 0 must be capped/None — no giant Vec allocation.
        let huge_len = format!("{:064x}", u64::MAX);
        assert!(abi_string(&huge_len, 0).is_none());
    }
}
