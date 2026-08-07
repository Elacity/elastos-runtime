use super::*;
use crate::api::{buy_authority, chain_tx, content_index, market_reads, trade_authority};
use axum::extract::Query;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Defaults for the resale-order assembly (overridable per request). The AuthorityGateway is the
/// access-token commerce contract; USDC is the default Base pay-token.
const DEFAULT_GATEWAY: &str = "0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D";
const DEFAULT_PAY_TOKEN: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
/// CoreStorage (the protocol fee/registry config) on Base — the source of the protocol (Elacity) cut
/// read via `protocolShares()` (CONTRACTS.md, confirmed 3/3 sources).
const CORE_STORAGE: &str = "0x0C1EeA2A3361B80AC0e42179335dB536A951760b";
/// Upper bound on how many discovery-index assets the Vault's on-chain access sweep enriches + checks in
/// one request. The index is already bounded by the discovery window; this caps the cold-cache IPFS fetch
/// fan-out and the single Multicall3 batch to a sane size (the access read itself is one round-trip).
const VAULT_SCAN_MAX: usize = 96;

/// First-party capsules permitted to call the marketplace data/order endpoints. These routes are invoked
/// by the `marketplace-content` browser capsule (launched by the `marketplace` shell, itself launched by
/// `home`), so each request presents that capsule's own launch token — NOT a Home token. The token is still
/// required, gateway-signed, non-delegatable, and session-scoped; the gate just accepts the correct app.
/// Without `marketplace-content` here, every detail/vault/acquire/order call fails closed with 403.
const MARKET_CALLER_CAPSULE_IDS: &[&str] = &[
    HOME_CAPSULE_ID,
    MARKETPLACE_CAPSULE_ID,
    "marketplace-content",
];

/// Gate the marketplace endpoints to any of the first-party marketplace capsules (see
/// `MARKET_CALLER_CAPSULE_IDS`). Returns the authenticated launch-token context on success.
fn require_market_token_context(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
) -> anyhow::Result<HomeLaunchTokenContext> {
    require_home_launch_token_for_any_context(data_dir, headers, MARKET_CALLER_CAPSULE_IDS)
}

pub(super) async fn marketplace_catalog(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    match require_capsule_catalog_token(&state.data_dir, &headers) {
        Ok(_) => Json(capsule_catalog_summary(&state.data_dir)).into_response(),
        Err(err) => (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

/// `GET /api/market/search?op=&q=` — content discovery over the persistent `AssetCreated` index
/// (cursor + backfill, PHASE2_INDEX_AND_API.md chunk 2; all reads via chain-provider). READ-ONLY;
/// holds no keys/RPC of its own (P3). `coverage` reports how much history the index has reached
/// (`recent-window` → `indexing` → `indexed`); the money path NEVER trusts this — buy re-verifies
/// terms live (Phase-1).
pub(super) async fn market_search(
    State(state): State<GatewayState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    match recent_index_cached(&state.data_dir) {
        Ok(idx) => {
            let op = params
                .get("op")
                .map(String::as_str)
                .filter(|s| !s.is_empty());
            let q = params
                .get("q")
                .map(String::as_str)
                .filter(|s| !s.is_empty());
            // `channel` scopes results to one creator's channel — the "More from this creator" rail.
            let channel = params
                .get("channel")
                .map(String::as_str)
                .filter(|s| !s.is_empty());
            // Newest-first cap: with the persistent index the searchable set grows toward FULL chain
            // history, so an uncapped response (and its enrichment fan-out) would scale with the chain,
            // not the request. `indexed` still reports the whole set.
            const SEARCH_RESULTS_MAX: usize = 200;
            let listings: Vec<_> = idx
                .search(op, q, channel)
                .into_iter()
                .take(SEARCH_RESULTS_MAX)
                .map(|l| l.to_json())
                .collect();
            let listings = enrich_listings(&state, listings, is_lean(&params)).await;
            Json(serde_json::json!({
                "listings": listings,
                "indexed": idx.len(),
                "coverage": idx.coverage(),
            }))
            .into_response()
        }
        Err(err) => market_error(&err),
    }
}

/// `GET /api/market/sections` — discovery shelves (new / free / resellable) over the recent window.
/// `?lean=1` returns the lean-first paint (cached descriptive only, no on-chain terms) — see `enrich_listings`.
pub(super) async fn market_sections(
    State(state): State<GatewayState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    match recent_index_cached(&state.data_dir) {
        Ok(idx) => {
            let lean = is_lean(&params);
            let mut sections = idx.sections();
            if let Some(arr) = sections
                .get_mut("sections")
                .and_then(serde_json::Value::as_array_mut)
            {
                for sec in arr.iter_mut() {
                    if let Some(slot) = sec.get_mut("listings") {
                        if let serde_json::Value::Array(items) = std::mem::take(slot) {
                            *slot = serde_json::Value::Array(
                                enrich_listings(&state, items, lean).await,
                            );
                        }
                    }
                }
            }
            Json(sections).into_response()
        }
        Err(err) => market_error(&err),
    }
}

/// `GET /api/market/indexer-status` — the index's scan progress (PC2's `indexer-status` parity):
/// coverage label, covered block range, backfill % toward the deploy block, row count, and the poll
/// cadence. READS ONLY the in-memory cache cell / disk snapshot — never triggers a chain sweep, so
/// the public route cannot be used as an RPC-amplification sink.
pub(super) async fn market_indexer_status(State(state): State<GatewayState>) -> Response {
    ensure_index_snapshot_path(&state.data_dir);
    // Arm the poll loop here too: after a restart the first traffic may be a status probe, not a
    // discovery request, and the backfill must not stall waiting for someone to browse. Arming is
    // idempotent + spawn-only (the advance itself stays single-flight in the background); this
    // route still never performs a synchronous sweep.
    ensure_index_poll_loop();
    let idx = recent_index_cell()
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|(_, idx)| Arc::clone(idx)))
        .or_else(|| load_index_snapshot_if_recent().map(Arc::new));
    let (coverage, scanned_to, backfill_low, listings) = match idx.as_deref() {
        Some(i) => (i.coverage(), i.scanned_to(), i.backfill_low(), i.len()),
        None => ("recent-window", 0, 0, 0),
    };
    let deploy = content_index::EVENT_HUB_DEPLOY_BLOCK;
    // % of history covered below the cursor: 100 once the backfill lane reaches the deploy block.
    let backfill_pct = if backfill_low == 0 || scanned_to <= deploy {
        0.0
    } else if backfill_low <= deploy {
        100.0
    } else {
        let total = (scanned_to - deploy) as f64;
        ((scanned_to - backfill_low) as f64 / total * 100.0).min(100.0)
    };
    Json(serde_json::json!({
        "coverage": coverage,
        "scanned_to": scanned_to,
        "backfill_low": backfill_low,
        "deploy_block": deploy,
        "backfill_pct": (backfill_pct * 10.0).round() / 10.0,
        "listings": listings,
        "poll_secs": market_poll_secs(),
    }))
    .into_response()
}

/// Truthy `lean` query flag (`1`/`true`) — selects the lean-first first-paint response.
fn is_lean(params: &HashMap<String, String>) -> bool {
    matches!(
        params.get("lean").map(String::as_str),
        Some("1") | Some("true")
    )
}

/// Stale-while-revalidate cache around `advance_index` so discovery requests almost never block on a
/// chain sweep. `market_search`/`market_sections` are unauthenticated and each cycle spawns a
/// chain-provider subprocess per `getLogs` window, so without caching an unauthenticated flood amplifies
/// into per-request RPC cost and subprocess churn (deep-audit MED). Tiers by age: under `FRESH`, serve
/// as-is; past `FRESH`, serve the cached index IMMEDIATELY and advance in a single-flight background
/// cycle. Only a legacy CURSORLESS window cache past `MAX_STALE` (or a cold start) builds synchronously —
/// a cursor-bearing index is confirmed history and never blocks a request. The money path NEVER trusts
/// this cache (buy re-verifies terms live, Phase-1).
type RecentIndexCacheCell = Mutex<Option<(Instant, Arc<content_index::ContentIndex>)>>;

fn recent_index_cell() -> &'static RecentIndexCacheCell {
    static CACHE: OnceLock<RecentIndexCacheCell> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// On-disk snapshot of the discovery index, under the gateway data dir. Set on first discovery request so
/// a RESTART starts warm (serve the snapshot instantly + revalidate) instead of paying the full cold chain
/// sweep (the 30-window `getLogs` build — the dominant cold-start cost). Discovery is advisory; the money
/// path re-verifies live, so serving a briefly-stale snapshot is safe.
static INDEX_SNAPSHOT_PATH: OnceLock<PathBuf> = OnceLock::new();

fn ensure_index_snapshot_path(data_dir: &std::path::Path) {
    let _ = INDEX_SNAPSHOT_PATH.set(data_dir.join("market").join("index-snapshot.json"));
}

/// Load the disk snapshot IF present, parseable, and worth serving before a fresh sweep completes.
/// A CURSOR-BEARING (v2) snapshot is served at ANY age — its listings are confirmed chain history
/// (never invalidated by time; the delta lane catches the range up and `coverage` labels the gap
/// honestly). A legacy cursorless (v1) snapshot is only a bounded recent-window cache, so it keeps
/// the freshness gate: past `MAX_DISK_AGE` it is dropped rather than served as if recent.
/// `None` => fall through to a synchronous build.
fn load_index_snapshot_if_recent() -> Option<content_index::ContentIndex> {
    const MAX_DISK_AGE: Duration = Duration::from_secs(3600);
    let path = INDEX_SNAPSHOT_PATH.get()?;
    let idx =
        serde_json::from_slice::<content_index::ContentIndex>(&std::fs::read(path).ok()?).ok()?;
    if idx.len() == 0 {
        return None;
    }
    if idx.cursor_set() {
        return Some(idx);
    }
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    if modified.elapsed().map(|e| e > MAX_DISK_AGE).unwrap_or(true) {
        return None; // stale v1 window cache — rebuild fresh rather than show an ancient list
    }
    Some(idx)
}

/// Best-effort atomic write (tmp + rename) of the index snapshot. A failed write just leaves the prior
/// snapshot and retries next sweep.
fn persist_index_snapshot(idx: &content_index::ContentIndex) {
    let Some(path) = INDEX_SNAPSHOT_PATH.get() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec(idx) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, &bytes).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

/// Single-flight background revalidation: at most one advance cycle in flight; success swaps the cache
/// AND refreshes the disk snapshot, failure keeps the (stale) entry so the next request still serves and
/// retries. The cycle ADVANCES the cursor-bearing index (delta + backfill lanes) rather than rebuilding
/// from scratch, so already-covered history is never re-scanned. No-op outside a tokio runtime.
fn spawn_index_revalidate() {
    static REFRESHING: AtomicBool = AtomicBool::new(false);
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    if REFRESHING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    tokio::task::spawn_blocking(|| {
        let prev = recent_index_cell()
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|(_, idx)| Arc::clone(idx)));
        if let Ok(idx) = advance_index(prev.as_deref()) {
            persist_index_snapshot(&idx);
            let idx = Arc::new(idx);
            if let Ok(mut guard) = recent_index_cell().lock() {
                *guard = Some((Instant::now(), Arc::clone(&idx)));
            }
            // Swap first (new listings visible immediately), THEN warm prices for the next paint.
            warm_listing_terms(&idx);
        }
        REFRESHING.store(false, Ordering::Release);
    });
}

/// Start the periodic polling loop (once per process, on the first discovery request): every
/// `ELASTOS_MARKET_POLL_SECS` (default 300s, clamped 30..3600) trigger a single-flight advance cycle so
/// the index keeps backfilling + tracking head even while nobody browses. Request-driven SWR remains the
/// freshness floor; this loop is what turns the bounded window cache into the Phase-2 persistent index
/// (PHASE2_INDEX_AND_API.md chunk 2 — polling, not subscription).
fn ensure_index_poll_loop() {
    static STARTED: OnceLock<()> = OnceLock::new();
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    STARTED.get_or_init(|| {
        let secs = market_poll_secs();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(secs));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ticker.tick().await; // consume the immediate first tick — the SWR path covers "now"
            loop {
                ticker.tick().await;
                spawn_index_revalidate();
            }
        });
    });
}

fn recent_index_cached(
    data_dir: &std::path::Path,
) -> Result<Arc<content_index::ContentIndex>, String> {
    const FRESH: Duration = Duration::from_secs(10);
    const MAX_STALE: Duration = Duration::from_secs(300);
    ensure_index_snapshot_path(data_dir);
    ensure_index_poll_loop();
    let cell = recent_index_cell();
    let snapshot = cell
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|(at, idx)| (*at, Arc::clone(idx))));
    if let Some((at, idx)) = snapshot {
        let age = at.elapsed();
        if age < FRESH {
            return Ok(idx);
        }
        if age < MAX_STALE || idx.cursor_set() {
            // Serve now, refresh in background. A cursor-bearing index NEVER blocks the request on a
            // sync rebuild past MAX_STALE: its rows are confirmed history (only recency is missing, and
            // `coverage` says so) — fail-closed-to-freshness only applies to the legacy window cache.
            spawn_index_revalidate();
            return Ok(idx);
        }
    } else if let Some(disk) = load_index_snapshot_if_recent() {
        // Cold process, but a recent disk snapshot exists: serve it INSTANTLY (seeded as stale) and
        // revalidate in the background — turns the ~18s cold sweep into a near-instant first paint.
        let idx = Arc::new(disk);
        if let Ok(mut guard) = cell.lock() {
            let stale_at = Instant::now()
                .checked_sub(FRESH)
                .unwrap_or_else(Instant::now);
            *guard = Some((stale_at, Arc::clone(&idx)));
        }
        spawn_index_revalidate();
        return Ok(idx);
    }
    // Cold start with no usable snapshot, or a too-stale legacy window cache: build synchronously
    // (fail-closed to freshness) and persist the snapshot for the next cold start. This first build
    // seeds the cursor, so it happens at most once per data dir.
    let fresh = Arc::new(advance_index(None)?);
    persist_index_snapshot(&fresh);
    if let Ok(mut guard) = cell.lock() {
        *guard = Some((Instant::now(), Arc::clone(&fresh)));
    }
    Ok(fresh)
}

fn market_error(err: &str) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        Json(serde_json::json!({ "error": err, "coverage": "recent-window" })),
    )
        .into_response()
}

/// One `EVENT_HUB` content-mint `getLogs` window ingested into the index — `AssetCreated` OR the
/// legacy `DigitalAssetRegistered` (one call; topic0 alternatives, the same pair PC2 scans).
/// `to_block` is either an explicit pinned `0x…` bound or `"latest"` (newest window only — head may
/// advance mid-scan).
fn scan_asset_created_window(
    idx: &mut content_index::ContentIndex,
    from: u64,
    to_block: &str,
) -> Result<(), String> {
    let filter = serde_json::json!({
        "address": content_index::EVENT_HUB,
        "fromBlock": format!("0x{from:x}"),
        "toBlock": to_block,
        "topics": [[
            content_index::ASSET_CREATED_TOPIC0,
            content_index::DIGITAL_ASSET_REGISTERED_TOPIC0,
        ]],
    });
    idx.ingest_logs(&chain_tx::get_logs_live(filter)?);
    Ok(())
}

/// One advance cycle of the persistent discovery index (PHASE2_INDEX_AND_API.md chunk 2). Every read
/// goes through `chain-provider` (the sole RPC declarant, P10) in ≤10k-block `getLogs` windows.
///
/// - **Cold (no cursor):** the pre-cursor bounded sweep — `ELASTOS_MARKET_DISCOVERY_WINDOWS` (default 1,
///   clamp 1..64) windows newest-first — then the cursor is SEEDED over the swept range, so every later
///   cycle is incremental.
/// - **Delta lane:** scan `[scanned_to − reorg overlap, head]` upward, bounded windows per cycle; a
///   longer outage just takes a few cycles to catch up. The overlap re-derives rows a shallow reorg
///   may have moved (upsert is idempotent).
/// - **Backfill lane:** scan bounded windows DOWNWARD from `backfill_low` toward the EventHub deploy
///   block (`ELASTOS_MARKET_BACKFILL_WINDOWS` per cycle, default 8, clamp 0..64; 0 disables). Once the
///   deploy block is reached the index covers full history and `coverage` reports `indexed`.
///
/// Lane errors after the cursor exists are FAIL-SOFT: the cycle returns the progress it made (the cursor
/// only advanced over ranges actually ingested) and the next cycle retries — discovery is advisory, the
/// money path never trusts it.
fn advance_index(
    prev: Option<&content_index::ContentIndex>,
) -> Result<content_index::ContentIndex, String> {
    use content_index::{ContentIndex, EVENT_HUB_DEPLOY_BLOCK, REORG_OVERLAP_BLOCKS};
    const WINDOW: u64 = 10_000;
    /// Delta windows per cycle: 16 × 10k ≈ 3.7 days of Base blocks — one cycle absorbs a long outage.
    const DELTA_WINDOWS_MAX: u64 = 16;
    let head = chain_tx::block_number_live()?;
    let mut idx = prev.cloned().unwrap_or_else(ContentIndex::new);

    if !idx.cursor_set() {
        let windows: u64 = std::env::var("ELASTOS_MARKET_DISCOVERY_WINDOWS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1)
            .clamp(1, 64);
        let mut to = head;
        let mut low = head;
        for i in 0..windows {
            let from = to.saturating_sub(WINDOW - 1);
            let to_block = if i == 0 {
                "latest".to_string()
            } else {
                format!("0x{to:x}")
            };
            scan_asset_created_window(&mut idx, from, &to_block)?;
            low = from;
            if from == 0 {
                break;
            }
            to = from.saturating_sub(1);
        }
        idx.seed_cursor(low, head);
        return Ok(idx);
    }

    // Delta lane: contiguous upward from just below the cursor (reorg overlap) to head.
    let mut from = idx
        .scanned_to()
        .saturating_sub(REORG_OVERLAP_BLOCKS)
        .saturating_add(1);
    for _ in 0..DELTA_WINDOWS_MAX {
        if from > head {
            break;
        }
        let to = from.saturating_add(WINDOW - 1).min(head);
        let to_block = if to == head {
            "latest".to_string()
        } else {
            format!("0x{to:x}")
        };
        if scan_asset_created_window(&mut idx, from, &to_block).is_err() {
            return Ok(idx); // fail-soft: keep this cycle's progress, retry next cycle
        }
        idx.note_delta_scanned(to);
        from = to.saturating_add(1);
    }

    // Backfill lane: bounded windows downward toward the deploy block.
    let backfill_windows: u64 = std::env::var("ELASTOS_MARKET_BACKFILL_WINDOWS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8)
        .clamp(0, 64);
    for _ in 0..backfill_windows {
        if idx.backfill_complete() {
            break;
        }
        let hi = idx.backfill_low().saturating_sub(1);
        if hi <= EVENT_HUB_DEPLOY_BLOCK {
            idx.note_backfilled(EVENT_HUB_DEPLOY_BLOCK);
            break;
        }
        let lo = hi.saturating_sub(WINDOW - 1).max(EVENT_HUB_DEPLOY_BLOCK);
        if scan_asset_created_window(&mut idx, lo, &format!("0x{hi:x}")).is_err() {
            return Ok(idx); // fail-soft: resume from the same backfill_low next cycle
        }
        idx.note_backfilled(lo);
    }
    Ok(idx)
}

/// `GET /api/market/get?operative=&token_id=` — the LIVE re-verified detail terms for one asset: the
/// lowest-priced active seller's price / payToken / supply (CONTRACTS.md: take the lowest pricePerToken),
/// read fresh from `sellersOf` + `listings` (chain-provider eth_call). READ-ONLY; NEVER trusted from the
/// discovery cache — this is the Phase-1 truth the buy binds + re-checks. `has_access` (own-this) is the
/// enrichment follow-on (needs the bytes16 KID + wallet). The shell gets `operative`/`token_id` from the
/// index listing it is displaying.
pub(super) async fn market_get(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Query(p): Query<HashMap<String, String>>,
) -> Response {
    // Gate the detail view to the authenticated shell (deep-audit MED): unauthenticated, each call fans out
    // into up to ~9 chain-provider subprocess spawns + live RPC, so leaving it open is an amplification sink.
    // Parity with market_vault/market_acquire.
    let context = match require_market_token_context(&state.data_dir, &headers) {
        Ok(c) => c,
        Err(e) => return order_forbidden(&e.to_string()),
    };
    let operative = match p
        .get("operative")
        .map(String::as_str)
        .filter(|s| !s.is_empty())
    {
        Some(o) if is_evm_address(o) => o,
        Some(_) => return order_error("operative is not a 20-byte EVM address"),
        None => return order_error("operative is required"),
    };
    let token_id = match p
        .get("token_id")
        .map(String::as_str)
        .filter(|s| !s.is_empty())
    {
        Some(t) => t,
        None => return order_error("token_id is required"),
    };
    let gateway = DEFAULT_GATEWAY;
    let token_id_word = buy_authority::token_id_to_word(token_id);
    // Short-TTL cache keyed by (operative, tokenId) so a burst of detail views collapses to one sweep.
    let cache_key = format!("{}:{}", operative.to_lowercase(), token_id_word);
    // The terms + metadata are WALLET-INDEPENDENT, so cache them. (has_access below is wallet-specific and is
    // computed per-request, never cached — otherwise one buyer's ownership would leak to another.)
    let mut body = if let Some(cached) = get_terms_cache_lookup(&cache_key) {
        cached
    } else {
        let sellers = match market_reads::sellers_of_live(gateway, operative, &token_id_word) {
            Ok(s) => s,
            Err(e) => return market_error(&e),
        };
        // Across active (supply > 0) sellers, take the lowest pricePerToken (bounded scan — caps RPC).
        let mut best: Option<(buy_authority::BoundTerms, u128, u128)> = None; // (terms, supply, price)
        for seller in sellers.iter().take(8) {
            if let Ok((terms, supply)) =
                buy_authority::read_listing_terms(gateway, operative, &token_id_word, seller)
            {
                if supply == 0 {
                    continue;
                }
                let price: u128 = terms.price.parse().unwrap_or(u128::MAX);
                if best.as_ref().is_none_or(|(_, _, bp)| price < *bp) {
                    best = Some((terms, supply, price));
                }
            }
        }
        let mut b = match best {
            Some((terms, supply, _)) => {
                // Format the raw minor-unit price with the pay-token's real decimals + symbol so the shell
                // never renders raw integer wei as a price or hardcodes "USDC" (the raw `price` is retained
                // for the buy's abort-on-drift, which compares against the live listing in minor units).
                let (symbol, decimals) = pay_token_display(&terms.pay_token);
                serde_json::json!({
                    "on_chain": {
                        "token_id": terms.token_id,
                        "seller": terms.seller,
                        "price": terms.price,
                        "price_formatted": format_minor_units(&terms.price, decimals),
                        "pay_token": terms.pay_token,
                        "pay_token_symbol": symbol,
                        "pay_token_decimals": decimals,
                        "supply_left": supply.to_string(),
                        "has_access": serde_json::Value::Null,
                    },
                    "sellers": sellers.len(),
                    "coverage": "live",
                })
            }
            None => serde_json::json!({
                "on_chain": serde_json::Value::Null,
                "sellers": sellers.len(),
                "note": "no active listing (no seller with supply > 0)",
            }),
        };
        // The REAL per-asset royalty economics (operative royaltyInfo + resellerCut, CoreStorage
        // protocolShares), read live off the async pool. Wallet/listing-independent (so computed even when
        // there is no active listing) and cached with the terms below. Fail-closed inside compute_royalty_block:
        // any read error -> available:false, and the shell hides the splits panel (P11) — never a fabricated split.
        let op_for_royalty = operative.to_string();
        b["royalty"] = tokio::task::spawn_blocking(move || compute_royalty_block(&op_for_royalty))
            .await
            .unwrap_or(serde_json::Value::Null);
        // Enrichment: if the shell passes the asset's token_uri, fetch + parse its metadata.json
        // (name / cover / content_cid / mime / kid) and merge. PURE parse; the fetch is live.
        if let Some(token_uri) = p
            .get("token_uri")
            .map(String::as_str)
            .filter(|s| !s.is_empty())
        {
            if let Some(meta) = enrich_from_token_uri(&state, token_uri).await {
                b["metadata"] = meta;
            }
        }
        get_terms_cache_store(&cache_key, &b);
        b
    };
    // has_access + owned_balance (per-request, wallet-specific — NOT cached):
    //   has_access    = hasAccessByContentId(wallet, KID) — KID-GLOBAL "can I open this content".
    //   owned_balance = operative.balanceOf(wallet, ACCESS_TOKEN=1) — copies of THIS listing held
    //                   (empirically keyed from real buy receipts: TransferSingle emits from the
    //                   operative with id=1 to the buyer).
    let wallet =
        crate::api::viewer_open::resolve_subject_address(&state, &context.principal_id).await;
    if !wallet.trim().is_empty() {
        let kid = body
            .get("metadata")
            .and_then(|m| m.get("kid"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let gw = gateway.to_string();
        let op = operative.to_string();
        let w = wallet.clone();
        let reads = tokio::task::spawn_blocking(move || {
            let owned = kid
                .as_deref()
                .map(|kid| market_reads::has_access_live(&gw, &w, kid));
            let balance = market_reads::access_token_balance_live(&op, &w);
            (owned, balance)
        })
        .await;
        if let Ok((owned, balance)) = reads {
            if let Some(obj) = body.get_mut("on_chain").and_then(|v| v.as_object_mut()) {
                if let Some(Ok(owned)) = owned {
                    obj.insert("has_access".to_string(), serde_json::json!(owned));
                }
                if let Ok(balance) = balance {
                    obj.insert(
                        "owned_balance".to_string(),
                        serde_json::json!(balance.to_string()),
                    );
                }
            }
        }
    }
    Json(body).into_response()
}

/// `GET /api/market/history?operative=&token_id=` — the asset's on-chain trade history (ItemListed /
/// ItemSold / ItemUnlisted) read DIRECTLY from AuthorityGateway logs (no subgraph; P5/P13). Omit
/// `operative` for the marketplace-wide recent activity feed. Gated like `market_get`. READ-ONLY; a bounded
/// newest-first window scan, short-TTL cached so the Activity/History views don't re-scan on every open.
pub(super) async fn market_history(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Query(p): Query<HashMap<String, String>>,
) -> Response {
    if let Err(e) = require_market_token_context(&state.data_dir, &headers) {
        return order_forbidden(&e.to_string());
    }
    let op_topic = match p
        .get("operative")
        .map(String::as_str)
        .filter(|s| !s.is_empty())
    {
        Some(o) if is_evm_address(o) => match market_reads::address_topic(o) {
            Some(t) => Some(t),
            None => return order_error("operative is not an address"),
        },
        Some(_) => return order_error("operative is not a 20-byte EVM address"),
        None => None,
    };
    let cache_key = op_topic.clone().unwrap_or_else(|| "*".to_string());
    if let Some(rows) = history_cache_lookup(&cache_key) {
        return Json(serde_json::json!({ "history": rows, "coverage": "recent-window" }))
            .into_response();
    }
    let gateway = DEFAULT_GATEWAY.to_string();
    let rows = match tokio::task::spawn_blocking(move || {
        read_trade_history(&gateway, op_topic.as_deref())
    })
    .await
    {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => return market_error(&e),
        Err(_) => return market_error("history task panicked"),
    };
    let rows = serde_json::Value::Array(rows);
    history_cache_store(&cache_key, &rows);
    Json(serde_json::json!({ "history": rows, "coverage": "recent-window" })).into_response()
}

/// Short-TTL cache of decoded history rows (keyed by operative-topic, or `*` for the marketplace-wide feed)
/// so the Activity/History views don't re-run an 8-window getLogs scan on every open.
fn history_cache() -> &'static Mutex<HashMap<String, (Instant, serde_json::Value)>> {
    static C: OnceLock<Mutex<HashMap<String, (Instant, serde_json::Value)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}
fn history_cache_lookup(key: &str) -> Option<serde_json::Value> {
    const TTL: Duration = Duration::from_secs(30);
    let guard = history_cache().lock().ok()?;
    guard
        .get(key)
        .filter(|(at, _)| at.elapsed() < TTL)
        .map(|(_, v)| v.clone())
}
fn history_cache_store(key: &str, value: &serde_json::Value) {
    const MAX: usize = 256;
    if let Ok(mut guard) = history_cache().lock() {
        if guard.len() >= MAX {
            guard.clear();
        }
        guard.insert(key.to_string(), (Instant::now(), value.clone()));
    }
}

// ---------------------------------------------------------------------------
// Preview player: serve + prefetch a clear DASH preview through the gateway's own content route, with a
// short-TTL byte cache so the standalone MSE player in the shell plays smoothly instead of waiting on cold
// IPFS per segment. The preview carries NO key material (it's the public `previewURL`), so this stays below
// the owned-content viewer path entirely (P10: a distinct, public operation, never the decrypt path).
// ---------------------------------------------------------------------------

/// Per-file cap (preview segments are small); a hostile manifest can't make us buffer a large object.
const MAX_PREVIEW_FILE: usize = 8 * 1024 * 1024;
/// Cap segments per track surfaced/prefetched, so a crafted manifest can't fan out unbounded fetches.
const MAX_PREVIEW_SEGS: usize = 400;

type PreviewByteCache = Mutex<HashMap<String, (Instant, Arc<Vec<u8>>)>>;
fn preview_cache() -> &'static PreviewByteCache {
    static C: OnceLock<PreviewByteCache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Fetch one preview file (`cid` + in-dir `path`) through the canonical content provider, memoized in a
/// short-TTL byte cache. `None` on any provider error / oversize (caller degrades to no preview).
async fn preview_fetch_cached(state: &GatewayState, cid: &str, path: &str) -> Option<Arc<Vec<u8>>> {
    const TTL: Duration = Duration::from_secs(600);
    const MAX_ENTRIES: usize = 512;
    let key = format!("{cid}/{path}");
    if let Ok(guard) = preview_cache().lock() {
        if let Some((at, bytes)) = guard.get(&key) {
            if at.elapsed() < TTL {
                return Some(bytes.clone());
            }
        }
    }
    let registry = state.provider_registry.as_ref()?;
    // Interactive preview fetch: bounded so an unresolvable CID fails fast instead of holding
    // the SERIAL ipfs backend (and every other marketplace fetch behind it) for minutes.
    let bytes = crate::content::fetch_bytes_via_provider_bounded(
        registry,
        cid,
        (!path.is_empty()).then_some(path),
        Some(20_000),
    )
    .await
    .ok()?;
    if bytes.is_empty() || bytes.len() > MAX_PREVIEW_FILE {
        return None;
    }
    let arc = Arc::new(bytes);
    if let Ok(mut guard) = preview_cache().lock() {
        if guard.len() >= MAX_ENTRIES {
            guard.clear();
        }
        guard.insert(key, (Instant::now(), arc.clone()));
    }
    Some(arc)
}

/// `GET /api/market/preview/plan?token_uri=` — resolve the asset's clear DASH preview, return a per-track
/// play plan (MSE mime + ordered segment URLs on the cached `/file` route), and kick off a background
/// prefetch of those segments so the shell's player starts fast. Gated like the other market endpoints.
pub(super) async fn market_preview_plan(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Query(p): Query<HashMap<String, String>>,
) -> Response {
    if let Err(e) = require_market_token_context(&state.data_dir, &headers) {
        return order_forbidden(&e.to_string());
    }
    let token_uri = match p
        .get("token_uri")
        .map(String::as_str)
        .filter(|s| !s.is_empty())
    {
        Some(t) => t,
        None => return order_error("token_uri is required"),
    };
    let Some(meta) = enrich_from_token_uri(&state, token_uri).await else {
        return market_error("could not resolve asset metadata");
    };
    let Some(preview_url) = meta
        .get("preview_url")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
    else {
        return order_error("asset has no preview");
    };
    let Some(cid) = market_reads::extract_cid(preview_url) else {
        return order_error("preview URL has no CID");
    };
    let manifest_path = market_reads::extract_cid_subpath(preview_url).unwrap_or_default();
    let Some(manifest) = preview_fetch_cached(&state, &cid, &manifest_path).await else {
        return market_error("preview manifest unavailable");
    };
    let manifest_str = String::from_utf8_lossy(&manifest);
    let mut tracks = market_reads::parse_dash_preview(&manifest_str);
    if tracks.is_empty() {
        return order_error("preview is not a playable clear DASH manifest");
    }
    // The manifest's segment paths are relative to its directory; resolve against the manifest's parent.
    let base = manifest_path
        .rsplit_once('/')
        .map(|(dir, _)| format!("{dir}/"))
        .unwrap_or_default();
    let mut prefetch: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for t in tracks.iter_mut() {
        t.seg_paths.truncate(MAX_PREVIEW_SEGS);
        let init_rel = format!("{base}{}", t.init_path);
        let seg_rels: Vec<String> = t.seg_paths.iter().map(|s| format!("{base}{s}")).collect();
        prefetch.push(init_rel.clone());
        prefetch.extend(seg_rels.iter().cloned());
        let file_url = |rel: &str| format!("/api/market/preview/file/{cid}/{rel}");
        out.push(serde_json::json!({
            "kind": t.kind,
            "mime": t.mime,
            "init_url": file_url(&init_rel),
            "seg_urls": seg_rels.iter().map(|r| file_url(r)).collect::<Vec<_>>(),
        }));
    }
    // Warm the cache in the background so the player's first appends hit memory, not cold IPFS.
    let warm_state = state.clone();
    let warm_cid = cid.clone();
    tokio::spawn(async move {
        for rel in prefetch {
            let _ = preview_fetch_cached(&warm_state, &warm_cid, &rel).await;
        }
    });
    Json(serde_json::json!({ "cid": cid, "tracks": out })).into_response()
}

/// `GET /api/market/preview/file/:cid/*path` — serve one cached preview byte-range (init/segment) for the
/// MSE player. Public like `/s/` (preview content carries no keys); bounded by the cache + size cap.
pub(super) async fn market_preview_file(
    State(state): State<GatewayState>,
    axum::extract::Path((cid, path)): axum::extract::Path<(String, String)>,
) -> Response {
    // Minimal hardening: a plausible CID and a non-traversing in-dir path.
    if !(cid.starts_with("Qm") || cid.starts_with("bafy"))
        || !cid.chars().all(|c| c.is_ascii_alphanumeric())
    {
        return (StatusCode::BAD_REQUEST, "invalid CID").into_response();
    }
    if path.contains("..") || path.starts_with('/') || path.contains('\\') {
        return (StatusCode::BAD_REQUEST, "invalid path").into_response();
    }
    match preview_fetch_cached(&state, &cid, &path).await {
        Some(bytes) => {
            let ct = if path.ends_with(".mpd") {
                "application/dash+xml"
            } else if path.ends_with(".m4s") || path.ends_with(".mp4") {
                "video/mp4"
            } else {
                "application/octet-stream"
            };
            (
                [
                    (axum::http::header::CONTENT_TYPE, ct),
                    (axum::http::header::CACHE_CONTROL, "public, max-age=600"),
                ],
                (*bytes).clone(),
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "preview file unavailable").into_response(),
    }
}

/// Blocking: scan recent AuthorityGateway windows for trade events (optionally filtered to one operative via
/// indexed topic2), decode + format them into newest-first history rows (capped). One getLogs per window.
fn read_trade_history(
    gateway: &str,
    op_topic: Option<&str>,
) -> Result<Vec<serde_json::Value>, String> {
    const WINDOW: u64 = 10_000;
    const MAX_ROWS: usize = 60;
    // History reaches further back than discovery (an asset may have been listed long ago) but each window is
    // a cheap indexed-topic getLogs. Clamped so a hostile value can't fan out unbounded RPC.
    let windows: u64 = std::env::var("ELASTOS_MARKET_HISTORY_WINDOWS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8)
        .clamp(1, 64);
    let head = chain_tx::block_number_live()?;
    let mut events: Vec<market_reads::TradeEvent> = Vec::new();
    let mut to = head;
    for i in 0..windows {
        let from = to.saturating_sub(WINDOW - 1);
        let to_block = if i == 0 {
            "latest".to_string()
        } else {
            format!("0x{to:x}")
        };
        let event_topics = serde_json::json!([
            market_reads::ITEM_LISTED_TOPIC0,
            market_reads::ITEM_SOLD_TOPIC0,
            market_reads::ITEM_UNLISTED_TOPIC0
        ]);
        let topics = match op_topic {
            Some(op) => serde_json::json!([event_topics, serde_json::Value::Null, op]),
            None => serde_json::json!([event_topics]),
        };
        let filter = serde_json::json!({
            "address": gateway,
            "fromBlock": format!("0x{from:x}"),
            "toBlock": to_block,
            "topics": topics,
        });
        for log in chain_tx::get_logs_live(filter)? {
            if let Some(ev) = market_reads::decode_trade_log(&log) {
                events.push(ev);
            }
        }
        if from == 0 {
            break;
        }
        to = from.saturating_sub(1);
    }
    events.sort_by(|a, b| b.block.unwrap_or(0).cmp(&a.block.unwrap_or(0)));
    events.truncate(MAX_ROWS);
    Ok(events.into_iter().map(trade_event_to_json).collect())
}

/// Format a decoded trade event into the shell's history-row shape (human price + pay-token symbol).
fn trade_event_to_json(ev: market_reads::TradeEvent) -> serde_json::Value {
    let (symbol, decimals) = ev
        .pay_token
        .as_deref()
        .map(pay_token_display)
        .unwrap_or_else(|| (String::new(), 18));
    let price_formatted = ev.price.as_deref().map(|p| format_minor_units(p, decimals));
    serde_json::json!({
        "type": ev.kind,
        "seller": ev.seller,
        "buyer": ev.buyer,
        "quantity": ev.quantity,
        "price": ev.price,
        "price_formatted": price_formatted,
        "pay_token": ev.pay_token,
        "pay_token_symbol": if symbol.is_empty() { serde_json::Value::Null } else { serde_json::json!(symbol) },
        "block": ev.block,
        "tx": ev.tx,
    })
}

/// Fetch + parse an asset's `metadata.json` (descriptive fields) via the content/* plane (P4 — never raw
/// ipfs). Best-effort: returns None on any fetch/parse failure (the live `on_chain` terms are the
/// load-bearing part; the metadata only describes). The descriptive paths mirror content-market; the full
/// `kid == contentId` validation is the hardening follow-on (PHASE2_ENRICHMENT.md).
async fn enrich_from_token_uri(state: &GatewayState, token_uri: &str) -> Option<serde_json::Value> {
    let cid = market_reads::extract_cid(token_uri)?;
    // Mints publish `metadata.json` as a UnixFS directory, so the tokenURI is `ipfs://<dirCid>/metadata.json`;
    // the bare CID resolves to the directory, not the JSON — fetch the in-dir path when present.
    let subpath = market_reads::extract_cid_subpath(token_uri);
    let registry = state.provider_registry.as_ref()?;
    // metadata.json is small and this fetch sits on every discovery/detail paint: bound it TIGHT.
    // The backend is SERIAL, so a page of unresolvable CIDs costs (bound × count) wall-clock for
    // everyone queued behind it — and a metadata.json either resolves in well under a second
    // (local/cluster) or not at all (DHT-dead mint). Failures are negative-cached by
    // enrich_fields, so each dead CID pays this bound once per process, not per request.
    let bytes = crate::content::fetch_bytes_via_provider_bounded(
        registry,
        &cid,
        subpath.as_deref(),
        Some(2_500),
    )
    .await
    .ok()?;
    let meta: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    serde_json::to_value(market_reads::parse_asset_metadata(&meta)).ok()
}

/// Read the asset's REAL on-chain royalty economics for the detail view: the per-receiver distribution
/// (operative `royaltyInfo`), the secondary `resellerCut`, and the protocol cut
/// (`CoreStorage.protocolShares`). Best-effort + FAIL-CLOSED (P11): `available=false` whenever the
/// distribution can't be read, so the shell hides the splits panel rather than show a fabricated split.
/// Blocking (up to three eth_calls) — invoke off the async pool.
fn compute_royalty_block(operative: &str) -> serde_json::Value {
    let distributions = market_reads::royalty_info_live(operative).unwrap_or_default();
    let reseller = market_reads::reseller_cut_live(operative).ok();
    let protocol = market_reads::protocol_shares_live(CORE_STORAGE).ok();
    let dist_json: Vec<serde_json::Value> = distributions
        .iter()
        .map(|(addr, pct)| serde_json::json!({ "address": addr, "pct": pct }))
        .collect();
    // Available if ANY real economic field was read — so a buy_and_resell asset shows its "90% resale
    // royalty" even when royaltyInfo returns no per-party distribution map. Fail-closed only when nothing
    // could be read.
    serde_json::json!({
        "available": !dist_json.is_empty() || reseller.is_some() || protocol.is_some(),
        "source": "chain",
        "distributions": dist_json,
        "reseller_cut_pct": reseller,
        "protocol_pct": protocol,
    })
}

/// Map a Base pay-token address to `(symbol, decimals)` for honest price formatting. Native (zero
/// address) = ETH/18; the known Base pay-tokens (CONTRACTS.md) are mapped exactly; an unknown ERC-20
/// defaults to 18 dp with a truncated-address symbol — never silently claiming "USDC".
fn pay_token_display(pay_token: &str) -> (String, u32) {
    match pay_token.trim().to_ascii_lowercase().as_str() {
        "" | "0x0000000000000000000000000000000000000000" => ("ETH".to_string(), 18),
        "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913" => ("USDC".to_string(), 6),
        "0xfde4c96c8593536e31f229ea8f37b2ada2699bb2" => ("USDT".to_string(), 6),
        "0x50c5725949a6f0c72e6c4a641f24049a917db0cb" => ("DAI".to_string(), 18),
        "0x4200000000000000000000000000000000000006" => ("WETH".to_string(), 18),
        other => {
            let sym = if other.len() >= 10 {
                format!("{}…{}", &other[..6], &other[other.len() - 4..])
            } else {
                other.to_string()
            };
            (sym, 18)
        }
    }
}

/// Format a raw minor-unit amount (decimal string) into a human decimal with `decimals` places and
/// trailing zeros trimmed (e.g. `"4000000", 6 -> "4"`; `"1500000", 6 -> "1.5"`; `"10000", 6 -> "0.01"`).
/// Falls back to the raw string if it does not parse as a `u128` (so we never silently misreport).
fn format_minor_units(raw: &str, decimals: u32) -> String {
    let n: u128 = match raw.trim().parse() {
        Ok(n) => n,
        Err(_) => return raw.to_string(),
    };
    if decimals == 0 {
        return n.to_string();
    }
    let scale = 10u128.checked_pow(decimals).unwrap_or(1);
    let int = n / scale;
    let frac = n % scale;
    if frac == 0 {
        return int.to_string();
    }
    let frac_str = format!("{frac:0width$}", width = decimals as usize);
    format!("{int}.{}", frac_str.trim_end_matches('0'))
}

/// Process-lifetime cache of per-asset descriptive enrichment, keyed by `token_uri`. An asset's
/// `metadata.json` is content-addressed (immutable per tokenId), so a hit never needs re-fetching even when
/// the recent-window index is rebuilt (10s TTL) — only the FIRST sighting of each asset pays the fetch.
/// Disk-backed (see `ensure_enrich_cache_loaded` / `persist_enrich_cache`) so a gateway restart starts WARM
/// instead of re-fetching every `metadata.json` — the dominant cold-load cost.
fn enrich_cache() -> &'static Mutex<HashMap<String, serde_json::Value>> {
    static C: OnceLock<Mutex<HashMap<String, serde_json::Value>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// On-disk file backing `enrich_cache`, under the gateway's data dir. Set once on first enrichment.
static ENRICH_CACHE_PATH: OnceLock<PathBuf> = OnceLock::new();
/// Set when `enrich_cache` gains a NEW successful resolution since the last persist; gates redundant writes.
static ENRICH_CACHE_DIRTY: AtomicBool = AtomicBool::new(false);

/// Idempotently hydrate `enrich_cache` from disk (once per process) and remember the write path. Because the
/// map is content-addressed and immutable per `token_uri`, a persisted entry is always valid; we only load
/// successful (object) resolutions — negatives were intentionally NOT persisted, so a transient IPFS failure
/// is re-attempted after a restart rather than cached as permanently lean.
fn ensure_enrich_cache_loaded(data_dir: &std::path::Path) {
    static LOADED: OnceLock<()> = OnceLock::new();
    LOADED.get_or_init(|| {
        let path = data_dir.join("market").join("enrich-cache.json");
        if let Ok(bytes) = std::fs::read(&path) {
            if let Ok(map) = serde_json::from_slice::<HashMap<String, serde_json::Value>>(&bytes) {
                if let Ok(mut g) = enrich_cache().lock() {
                    for (k, v) in map {
                        // Adopt only entries from the CURRENT enrichment schema — an object carrying
                        // `category` (the finer Type taxonomy). Pre-schema entries are dropped so the asset
                        // is re-enriched on its next sighting and gains the new field, rather than being
                        // served permanently stale. This self-heals the cache across field additions.
                        if v.get("category").is_some() {
                            g.entry(k).or_insert(v);
                        }
                    }
                }
            }
        }
        let _ = ENRICH_CACHE_PATH.set(path);
    });
}

/// Write `enrich_cache` to disk if it gained a new resolution since the last write (debounced via
/// `ENRICH_CACHE_DIRTY`). Persists ONLY successful (object) entries — negatives stay process-lifetime so a
/// temporary fetch failure can recover on the next run. Best-effort + atomic (tmp file + rename); a failed
/// write just leaves the prior snapshot and retries next time.
fn persist_enrich_cache() {
    if !ENRICH_CACHE_DIRTY.swap(false, Ordering::AcqRel) {
        return;
    }
    let Some(path) = ENRICH_CACHE_PATH.get() else {
        return;
    };
    let snapshot: HashMap<String, serde_json::Value> = match enrich_cache().lock() {
        Ok(g) => g
            .iter()
            .filter(|(_, v)| v.is_object())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        Err(_) => return,
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec(&snapshot) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, &bytes).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

/// Map a metadata MIME to the shell's medium bucket so cards get the right glyph and the medium filter works.
/// Mirrors content-market's `contentType` classification (video→watch, audio→listen, image→view, doc→read).
fn medium_from_mime(mime: &str) -> &'static str {
    let m = mime.to_ascii_lowercase();
    if m.starts_with("video/") {
        "watch"
    } else if m.starts_with("audio/") {
        "listen"
    } else if m.starts_with("image/") {
        "view"
    } else if m == "application/pdf" || m == "application/epub+zip" || m.starts_with("text/") {
        "read"
    } else {
        "explore"
    }
}

/// Refine the coarse MIME `medium` into elacity's finer content category, the single normalised "Type" axis
/// the shell facets on (video/audio/image/document plus the kinds MIME alone can't express: 3d/comic/ebook/
/// article). TRUTH-ONLY: the metadata-derived kinds are assigned only when MIME OR a real author-declared
/// category/tag implies them (checked BEFORE the MIME bases, since a comic is image/* and an article is
/// text/*); everything else falls back to the MIME base, and an unknown MIME to `other`. Never guesses a
/// category to populate a facet — an asset with no resolvable signal lands in its honest base bucket.
fn content_category(mime: &str, categories: &[String], tags: &[String]) -> &'static str {
    let m = mime.to_ascii_lowercase();
    let hay: Vec<String> = categories
        .iter()
        .chain(tags.iter())
        .map(|s| s.to_ascii_lowercase())
        .collect();
    let tagged = |needles: &[&str]| hay.iter().any(|h| needles.iter().any(|n| h.contains(n)));
    if m.starts_with("model/")
        || m.contains("gltf")
        || m.contains("glb")
        || tagged(&["3d", "model"])
    {
        "3d"
    } else if tagged(&["comic", "manga"]) {
        "comic"
    } else if m == "application/epub+zip" || tagged(&["ebook", "e-book", "book", "novel"]) {
        "ebook"
    } else if tagged(&["article", "blog", "essay"]) {
        "article"
    } else if m.starts_with("video/") {
        "video"
    } else if m.starts_with("audio/") {
        "audio"
    } else if m.starts_with("image/") {
        "image"
    } else if m == "application/pdf" || m.starts_with("text/") {
        "document"
    } else {
        "other"
    }
}

/// Resolve one asset's descriptive enrichment (name/cover/medium/content_cid/kid), cached by `token_uri`.
/// Best-effort: `None` when the `metadata.json` can't be fetched/parsed (the card stays lean). The cover
/// (`image_url`) is returned RAW (`ipfs://…`); the shell resolves it to the `/s/<cid>` content route (P4).
async fn enrich_fields(state: &GatewayState, token_uri: &str) -> Option<serde_json::Value> {
    if token_uri.is_empty() {
        return None;
    }
    ensure_enrich_cache_loaded(&state.data_dir);
    if let Some(hit) = enrich_cache()
        .lock()
        .ok()
        .and_then(|g| g.get(token_uri).cloned())
    {
        // A `Null` sentinel is a cached NEGATIVE (metadata couldn't be resolved) — treat as None, don't refetch.
        return hit.is_object().then_some(hit);
    }
    let resolved = compute_enrich_fields(state, token_uri).await;
    // Cache the OUTCOME either way (Object on success, Null negative on failure) so a bad/unreachable CID can
    // never be re-fetched on every public discovery request (negative caching). Only a successful resolution
    // marks the cache dirty for disk persistence (negatives stay process-lifetime).
    if let Ok(mut g) = enrich_cache().lock() {
        g.insert(
            token_uri.to_string(),
            resolved.clone().unwrap_or(serde_json::Value::Null),
        );
    }
    if resolved.is_some() {
        ENRICH_CACHE_DIRTY.store(true, Ordering::Release);
    }
    resolved
}

/// The fetch+parse+build of one asset's descriptive fields. No caching — `enrich_fields` caches the outcome.
async fn compute_enrich_fields(state: &GatewayState, token_uri: &str) -> Option<serde_json::Value> {
    let meta = enrich_from_token_uri(state, token_uri).await?;
    let get = |k: &str| {
        meta.get(k)
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
    };
    let mut fields = serde_json::Map::new();
    if let Some(name) = get("name") {
        fields.insert("name".into(), serde_json::json!(name));
    }
    if let Some(desc) = get("description") {
        fields.insert("description".into(), serde_json::json!(desc));
    }
    if let Some(cid) = get("content_cid") {
        fields.insert("content_cid".into(), serde_json::json!(cid));
    }
    if let Some(url) = get("image_url") {
        fields.insert("image_url".into(), serde_json::json!(url));
    }
    if let Some(mime) = get("mime_type") {
        fields.insert("mime_type".into(), serde_json::json!(mime));
        fields.insert("medium".into(), serde_json::json!(medium_from_mime(mime)));
    }
    // Duration (ms) for the card's bottom-right chip — from the metadata `attributes[]` we already parsed
    // (AssetMeta surfaces them as {label,value}); mirrors elacity reading the `duration` trait. Truthful:
    // omitted entirely when the asset declares no duration trait.
    if let Some(ms) = meta
        .get("attributes")
        .and_then(serde_json::Value::as_array)
        .and_then(|arr| {
            arr.iter().find_map(|t| {
                let label = t.get("label").and_then(serde_json::Value::as_str)?;
                if !label.eq_ignore_ascii_case("duration") {
                    return None;
                }
                let v = t.get("value")?;
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.trim().parse::<u64>().ok()))
            })
        })
        .filter(|ms| *ms > 0)
    {
        fields.insert("duration".into(), serde_json::json!(ms));
    }
    // Upload time for the card's "owner • N ago" line — straight from the metadata (no chain read).
    if let Some(created) = get("created_at") {
        fields.insert("created_at".into(), serde_json::json!(created));
    }
    // Categories/tags for Explore faceting + card context (real metadata.properties arrays; omitted if empty).
    let str_array = |key: &str| -> Vec<String> {
        meta.get(key)
            .and_then(serde_json::Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let categories = str_array("categories");
    let tags = str_array("tags");
    for (key, arr) in [("categories", &categories), ("tags", &tags)] {
        if !arr.is_empty() {
            fields.insert(key.into(), serde_json::json!(arr));
        }
    }
    // Finer normalised content category (the shell's "Type" axis), derived from MIME + the author's declared
    // categories/tags. Always present — at minimum the MIME base — so every enriched row is faceted truthfully.
    let mime = fields
        .get("mime_type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    fields.insert(
        "category".into(),
        serde_json::json!(content_category(mime, &categories, &tags)),
    );
    if let Some(kid) = get("kid") {
        fields.insert("kid".into(), serde_json::json!(kid));
        // content_id == the bytes16 KID; AssetCreated carries none, so the metadata is its first source.
        fields.insert("content_id".into(), serde_json::json!(kid));
    }
    if fields.is_empty() {
        return None;
    }
    fields.insert("metadata_status".into(), serde_json::json!("resolved"));
    Some(serde_json::Value::Object(fields))
}

/// Merge per-asset descriptive enrichment onto a batch of lean listing JSONs (in place, by `token_uri`).
/// The routes are PUBLIC, so a cold/large window must NOT fan out into unbounded metadata.json fetches: cache
/// hits are always merged (free), but only `ENRICH_FETCH_MAX` NEW fetches happen per request — the rest stay
/// lean and are picked up on a later load as the process-lifetime cache warms (the shell tolerates lean rows).
/// `lean=true` is the lean-first first paint: merge only ALREADY-CACHED descriptive fields and skip the
/// on-chain term reads entirely, so the response returns with no new IPFS fetch and no `eth_call` — the
/// shell paints cards instantly, then issues the full (non-lean) call to fill price/cover/duration. Same
/// data, same shapes (P10) — lean is a strict subset that never fabricates, only omits.
/// Warm one asset's descriptive metadata in the BACKGROUND (single-flight per token_uri): the
/// request path merges only cache hits, so this is the only place an uncached metadata.json fetch
/// happens for discovery. `enrich_fields` negative-caches failures, so a dead CID is attempted
/// once per process, and `persist_enrich_cache` write-throughs successes for the next cold start.
fn spawn_enrich_warm(state: &GatewayState, uri: String) {
    static IN_FLIGHT: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    let in_flight = IN_FLIGHT.get_or_init(|| Mutex::new(std::collections::HashSet::new()));
    if let Ok(mut g) = in_flight.lock() {
        if !g.insert(uri.clone()) {
            return; // already being warmed
        }
    }
    let s = state.clone();
    tokio::spawn(async move {
        let _ = enrich_fields(&s, &uri).await;
        persist_enrich_cache();
        if let Ok(mut g) = in_flight.lock() {
            g.remove(&uri);
        }
    });
}

async fn enrich_listings(
    state: &GatewayState,
    mut listings: Vec<serde_json::Value>,
    lean: bool,
) -> Vec<serde_json::Value> {
    // Warm budget is deliberately SMALL: each dead-CID warm holds the serial ipfs backend for the
    // full fetch bound, and an interactive fetch (detail view metadata) queues behind them — 6 ×
    // ~2.5s caps that queue at ~15s worst, and repeat visits/polls warm the rest progressively.
    const ENRICH_FETCH_MAX: usize = 6;
    const ENRICH_CONCURRENCY: usize = 8;
    // Plan which rows to enrich: ONLY cache hits are merged into THIS response. Uncached rows are
    // handed to the background warmer (bounded fetch + negative cache) and stay lean now — the ipfs
    // backend is SERIAL, so awaiting even a few unresolvable metadata CIDs in the request path
    // queues MINUTES of wall-clock in front of every other caller (the storefront detail view
    // included). Discovery never blocks on a fetch; rows fill in as the warmer lands them.
    let mut plan: Vec<(usize, String)> = Vec::new();
    let mut warm_budget = if lean { 0 } else { ENRICH_FETCH_MAX };
    for (i, l) in listings.iter().enumerate() {
        let uri = l
            .get("token_uri")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        if uri.is_empty() {
            continue;
        }
        let cached = enrich_cache()
            .lock()
            .ok()
            .is_some_and(|g| g.contains_key(&uri));
        if cached {
            plan.push((i, uri));
        } else if warm_budget > 0 {
            warm_budget -= 1;
            spawn_enrich_warm(state, uri);
        }
        // else: over the per-request warm budget — a later load picks the row up
    }
    // Merge the planned (all-cached) rows with bounded concurrency. `enrich_fields` is internally
    // cached + fail-closed, so concurrent calls for distinct URIs are safe and never fabricate.
    // Same merged shape as before (P10).
    let mut set = tokio::task::JoinSet::new();
    let mut pending = plan.into_iter();
    for _ in 0..ENRICH_CONCURRENCY {
        if let Some((idx, uri)) = pending.next() {
            let s = state.clone();
            set.spawn(async move { (idx, enrich_fields(&s, &uri).await) });
        }
    }
    while let Some(joined) = set.join_next().await {
        if let Some((idx, uri)) = pending.next() {
            let s = state.clone();
            set.spawn(async move { (idx, enrich_fields(&s, &uri).await) });
        }
        if let Ok((idx, Some(serde_json::Value::Object(fields)))) = joined {
            if let Some(obj) = listings
                .get_mut(idx)
                .and_then(serde_json::Value::as_object_mut)
            {
                for (k, v) in fields {
                    obj.insert(k, v);
                }
            }
        }
    }
    persist_enrich_cache(); // write-through any new metadata resolutions (debounced, best-effort)
    if lean {
        return listings; // first paint: no on-chain term reads
    }
    attach_listing_terms(listings).await
}

/// The poll-loop interval (`ELASTOS_MARKET_POLL_SECS`, default 300s, clamped 30..3600) — shared by
/// the loop itself and the listing-terms TTL so warm prices always outlive the gap between cycles.
fn market_poll_secs() -> u64 {
    std::env::var("ELASTOS_MARKET_POLL_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300)
        .clamp(30, 3600)
}

/// Per-(operative,tokenId) cache of the compact discovery listing brief (cheapest active listing price +
/// summed available supply + resale %). Separate from `get_terms_cache` (which holds the FULL detail body)
/// so an enrichment pass never poisons the detail cache with a partial shape. TTL tracks the poll
/// interval (+ slack): `warm_listing_terms` re-reads the newest priced cards every cycle, so a card's
/// price is at most ~one cycle old — and the money path re-verifies live regardless (Phase-1).
fn listing_terms_cache() -> &'static Mutex<HashMap<String, (Instant, serde_json::Value)>> {
    static C: OnceLock<Mutex<HashMap<String, (Instant, serde_json::Value)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}
fn listing_terms_lookup(key: &str) -> Option<serde_json::Value> {
    let ttl = Duration::from_secs(market_poll_secs() + 120);
    let guard = listing_terms_cache().lock().ok()?;
    guard
        .get(key)
        .filter(|(at, _)| at.elapsed() < ttl)
        .map(|(_, v)| v.clone())
}
fn listing_terms_store(key: &str, value: &serde_json::Value) {
    const MAX: usize = 1024;
    if let Ok(mut guard) = listing_terms_cache().lock() {
        if guard.len() >= MAX {
            guard.clear();
        }
        guard.insert(key.to_string(), (Instant::now(), value.clone()));
    }
}

/// Read the cheapest active listing (price + pay-token), the total available supply across active sellers,
/// and the resale % for ONE asset — the same `sellersOf`+`listings` reads `market_get` does, in the compact
/// shape a Discover/Explore CARD needs. Blocking (eth_calls) — call off the async pool. Fail-closed: any
/// read error yields `{for_sale:false}` (the card shows a neutral state, never a fabricated price).
fn compute_listing_brief(operative: &str, token_id_word: &str) -> serde_json::Value {
    let gateway = DEFAULT_GATEWAY;
    let sellers = match market_reads::sellers_of_live(gateway, operative, token_id_word) {
        Ok(s) => s,
        Err(_) => return serde_json::json!({ "for_sale": false, "terms_read": false }),
    };
    let mut best: Option<(buy_authority::BoundTerms, u128)> = None; // (terms, price)
    let mut available: u128 = 0;
    for seller in sellers.iter().take(8) {
        if let Ok((terms, supply)) =
            buy_authority::read_listing_terms(gateway, operative, token_id_word, seller)
        {
            if supply == 0 {
                continue;
            }
            available = available.saturating_add(supply);
            let price: u128 = terms.price.parse().unwrap_or(u128::MAX);
            if best.as_ref().is_none_or(|(_, bp)| price < *bp) {
                best = Some((terms, price));
            }
        }
    }
    match best {
        Some((terms, _)) => {
            let (symbol, decimals) = pay_token_display(&terms.pay_token);
            serde_json::json!({
                "for_sale": true,
                "terms_read": true,
                "price": terms.price,
                "price_formatted": format_minor_units(&terms.price, decimals),
                "pay_token": terms.pay_token,
                "pay_token_symbol": symbol,
                "supply_available": available.to_string(),
                "resale_pct": market_reads::reseller_cut_live(operative).ok(),
            })
        }
        None => serde_json::json!({ "for_sale": false, "terms_read": true }),
    }
}

/// Shape a batched `CardBrief` into the same card JSON `compute_listing_brief` produces (one canonical
/// shape, P10): cheapest active listing + total available supply. `sellers_ok=false` => couldn't read
/// (`terms_read:false`); no active listing => `for_sale:false` with `terms_read:true`. Never fabricates.
fn brief_to_json(brief: &market_reads::CardBrief) -> serde_json::Value {
    if !brief.sellers_ok {
        return serde_json::json!({ "for_sale": false, "terms_read": false });
    }
    let mut best: Option<&buy_authority::BoundTerms> = None;
    let mut best_price = u128::MAX;
    let mut available: u128 = 0;
    for (terms, supply) in &brief.listings {
        available = available.saturating_add(*supply);
        let price = terms.price.parse::<u128>().unwrap_or(u128::MAX);
        if best.is_none() || price < best_price {
            best = Some(terms);
            best_price = price;
        }
    }
    match best {
        Some(terms) => {
            let (symbol, decimals) = pay_token_display(&terms.pay_token);
            serde_json::json!({
                "for_sale": true,
                "terms_read": true,
                "price": terms.price,
                "price_formatted": format_minor_units(&terms.price, decimals),
                "pay_token": terms.pay_token,
                "pay_token_symbol": symbol,
                "supply_available": available.to_string(),
                "resale_pct": brief.resale_pct,
            })
        }
        None => serde_json::json!({ "for_sale": false, "terms_read": true }),
    }
}

/// Attach the on-chain listing brief (price / available supply / resale %) to each discovery row so cards
/// show real money — the discovery index itself carries none. Bounded per request (`TERMS_ENRICH_MAX`) and
/// cached; rows beyond the budget that aren't cached stay lean and the card shows "price on open" rather
/// than a fabricated number. Free assets are skipped (no listing).
async fn attach_listing_terms(mut listings: Vec<serde_json::Value>) -> Vec<serde_json::Value> {
    const TERMS_ENRICH_MAX: usize = 12;
    let mut budget = TERMS_ENRICH_MAX;
    let mut jobs: Vec<(usize, String, String, String)> = Vec::new(); // (idx, operative, word, key)
    let mut cached_merges: Vec<(usize, serde_json::Value)> = Vec::new();
    for (i, l) in listings.iter().enumerate() {
        let operative = l
            .get("operative_address")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        let token_id = l
            .get("token_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        let is_free = l.get("op_type").and_then(serde_json::Value::as_str) == Some("free");
        if is_free || operative.is_empty() || token_id.is_empty() {
            continue;
        }
        let word = buy_authority::token_id_to_word(&token_id);
        let key = format!("{}:{}", operative.to_lowercase(), word);
        if let Some(brief) = listing_terms_lookup(&key) {
            cached_merges.push((i, brief));
        } else if budget > 0 {
            budget -= 1;
            jobs.push((i, operative, word, key));
        }
    }
    for (i, brief) in cached_merges {
        merge_into(&mut listings[i], &brief);
    }
    if jobs.is_empty() {
        return listings;
    }
    let computed = tokio::task::spawn_blocking(move || {
        // One batched read (2 `eth_call`s) for the whole page; fall back to the per-card path
        // (same canonical reads, P10) only if the batch eth_call itself fails.
        let items: Vec<(String, String)> = jobs
            .iter()
            .map(|(_, op, w, _)| (op.clone(), w.clone()))
            .collect();
        match market_reads::listing_briefs_batched(DEFAULT_GATEWAY, &items) {
            Ok(briefs) if briefs.len() == jobs.len() => jobs
                .into_iter()
                .zip(briefs.iter())
                .map(|((i, _op, _w, key), brief)| (i, key, brief_to_json(brief)))
                .collect::<Vec<_>>(),
            _ => jobs
                .into_iter()
                .map(|(i, operative, word, key)| (i, key, compute_listing_brief(&operative, &word)))
                .collect::<Vec<_>>(),
        }
    })
    .await
    .unwrap_or_default();
    for (i, key, brief) in computed {
        listing_terms_store(&key, &brief);
        if let Some(row) = listings.get_mut(i) {
            merge_into(row, &brief);
        }
    }
    listings
}

/// Refresh the listing-terms cache for the NEWEST priced listings after each poll cycle (PC2 parity:
/// prices ride the scan cadence, so browsing hits a warm cache instead of paying on-chain reads on the
/// request path). Same batched read + canonical brief shape as `attach_listing_terms` (P10), same
/// per-card fallback. Bounded to one Discover page; free assets carry no listing. Metadata warming is
/// NOT done here — `enrich_cache` is content-addressed and disk-backed, so each asset pays that fetch
/// once ever, on first sighting. Blocking (eth_calls) — poll-cycle context only.
fn warm_listing_terms(idx: &content_index::ContentIndex) {
    const WARM_MAX: usize = 24;
    let items: Vec<(String, String, String)> = idx
        .search(None, None, None)
        .into_iter()
        .filter(|l| l.op_type != "free" && !l.operative_address.is_empty())
        .take(WARM_MAX)
        .map(|l| {
            let word = buy_authority::token_id_to_word(&l.token_id);
            let key = format!("{}:{}", l.operative_address.to_lowercase(), word);
            (l.operative_address.clone(), word, key)
        })
        .collect();
    if items.is_empty() {
        return;
    }
    let batch: Vec<(String, String)> = items
        .iter()
        .map(|(op, w, _)| (op.clone(), w.clone()))
        .collect();
    match market_reads::listing_briefs_batched(DEFAULT_GATEWAY, &batch) {
        Ok(briefs) if briefs.len() == items.len() => {
            for ((_, _, key), brief) in items.iter().zip(briefs.iter()) {
                listing_terms_store(key, &brief_to_json(brief));
            }
        }
        _ => {
            for (operative, word, key) in &items {
                listing_terms_store(key, &compute_listing_brief(operative, word));
            }
        }
    }
}

/// Shallow-merge a JSON object's fields onto a row (both must be objects; no-op otherwise).
fn merge_into(row: &mut serde_json::Value, fields: &serde_json::Value) {
    if let (Some(obj), Some(src)) = (row.as_object_mut(), fields.as_object()) {
        for (k, v) in src {
            obj.insert(k.clone(), v.clone());
        }
    }
}

/// Per-(operative,tokenId) short-TTL cache for `market_get` so repeated/burst detail views collapse to one
/// `sellersOf`+`listings` sweep. The route is gated, but a hammering authed shell still shouldn't re-sweep.
fn get_terms_cache() -> &'static Mutex<HashMap<String, (Instant, serde_json::Value)>> {
    static C: OnceLock<Mutex<HashMap<String, (Instant, serde_json::Value)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}
fn get_terms_cache_lookup(key: &str) -> Option<serde_json::Value> {
    const TTL: Duration = Duration::from_secs(10);
    let guard = get_terms_cache().lock().ok()?;
    guard
        .get(key)
        .filter(|(at, _)| at.elapsed() < TTL)
        .map(|(_, v)| v.clone())
}
fn get_terms_cache_store(key: &str, value: &serde_json::Value) {
    const MAX: usize = 1024; // bound the map; if it grows past MAX, drop it wholesale (no LRU needed)
    if let Ok(mut guard) = get_terms_cache().lock() {
        if guard.len() >= MAX {
            guard.clear();
        }
        guard.insert(key.to_string(), (Instant::now(), value.clone()));
    }
}

/// `GET /api/market/vault` — the buyer's OWNED view: every asset their wallet HOLDS AN ACCESS TOKEN FOR
/// (minted OR bought), unioned with the assets already pinned into their Library (`<root>/Acquired/`).
/// elacity sources this from its centralized subgraph (`fetchAccessibleTokens`); the runtime reads it
/// DIRECTLY from the AuthorityGateway via `hasAccessByContentId` over the discovery index — no third-party
/// indexer in the trust path (P5/P13). Home-token-gated (per-user). Fail-CLOSED: a wallet that isn't linked
/// yet, or any failed read, degrades to the pinned set — never a fabricated holding. Two truth sources,
/// one canonical access read (the same `hasAccessByContentId` the detail view uses, P10).
async fn compute_owned(
    state: GatewayState,
    context: HomeLaunchTokenContext,
) -> Result<serde_json::Value, String> {
    // (A) The locally-pinned Library across the type-correct folders (P10) — assets already downloaded +
    // openable in the player. A bought `.ddrm` capsule reports its ASSET content_cid (not the on-disk hash),
    // so the map keys match the listing's content_cid and a chain-held asset that is ALSO pinned carries its
    // uri (the shell's "Open in your library" handoff needs it).
    let pinned_objects = list_acquired_objects(&state, &context).await;
    let mut pinned_uri_by_cid: HashMap<String, serde_json::Value> = HashMap::new();
    for o in &pinned_objects {
        if let Some(cid) = o.get("content_cid").and_then(serde_json::Value::as_str) {
            pinned_uri_by_cid.insert(
                cid.to_string(),
                o.get("uri").cloned().unwrap_or(serde_json::Value::Null),
            );
        }
    }

    // (B) On-chain ACCESS HOLDINGS (minted OR bought) read live over the discovery index. The KID for each
    // asset comes from its metadata (enrich, cached), then ONE Multicall3 `hasAccessByContentId` sweep.
    let wallet =
        crate::api::viewer_open::resolve_subject_address(&state, &context.principal_id).await;
    let mut owned: Vec<serde_json::Value> = Vec::new();
    let mut seen_cids: std::collections::HashSet<String> = std::collections::HashSet::new();
    if !wallet.trim().is_empty() {
        if let Ok(idx) = recent_index_cached(&state.data_dir) {
            // Snapshot the bounded candidate set (token_uri + display row) so no index borrow is held across
            // the enrichment awaits below.
            let candidates: Vec<(String, serde_json::Value)> = idx
                .search(None, None, None)
                .into_iter()
                .take(VAULT_SCAN_MAX)
                .map(|l| (l.token_uri.clone(), l.to_json()))
                .collect();
            let mut rows: Vec<serde_json::Value> = Vec::new();
            let mut kids: Vec<String> = Vec::new();
            for (token_uri, mut row) in candidates {
                let Some(meta) = enrich_fields(&state, &token_uri).await else {
                    continue;
                };
                let Some(kid) = meta
                    .get("kid")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                else {
                    continue; // no resolvable KID -> can't check access; skip (never assume ownership)
                };
                if let (Some(obj), Some(m)) = (row.as_object_mut(), meta.as_object()) {
                    for (k, v) in m {
                        obj.insert(k.clone(), v.clone());
                    }
                }
                rows.push(row);
                kids.push(kid);
            }
            // ONE batched access read; fail-closed (empty -> no holdings surfaced from chain this request).
            let gw = DEFAULT_GATEWAY.to_string();
            let w = wallet.clone();
            let kids_for_call = kids.clone();
            let flags = tokio::task::spawn_blocking(move || {
                market_reads::has_access_batched(&gw, &w, &kids_for_call)
            })
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
            if flags.len() == rows.len() {
                for (mut row, held) in rows.into_iter().zip(flags) {
                    if !held {
                        continue;
                    }
                    let cid = row
                        .get("content_cid")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string);
                    let acquired = cid
                        .as_ref()
                        .map(|c| pinned_uri_by_cid.contains_key(c))
                        .unwrap_or(false);
                    if let Some(obj) = row.as_object_mut() {
                        obj.insert("held".into(), serde_json::json!(true));
                        obj.insert("acquired".into(), serde_json::json!(acquired));
                        if let Some(uri) = cid.as_ref().and_then(|c| pinned_uri_by_cid.get(c)) {
                            obj.insert("uri".into(), uri.clone());
                        }
                    }
                    if let Some(c) = cid {
                        seen_cids.insert(c);
                    }
                    owned.push(row);
                }
            }
        }
    }

    // (C) Pinned assets the chain sweep didn't cover (older than the discovery window, or wallet unlinked)
    // so nothing already in the Library disappears. Minimal shape; flagged acquired + held.
    for o in &pinned_objects {
        let cid = o
            .get("content_cid")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        if let Some(c) = &cid {
            if seen_cids.contains(c) {
                continue;
            }
        }
        owned.push(serde_json::json!({
            "uri": o.get("uri"),
            "name": o.get("name"),
            "content_cid": o.get("content_cid"),
            "mime": o.get("mime"),
            "acquired": true,
            "held": true,
        }));
    }

    Ok(serde_json::json!({
        "owned": owned,                 // the array the shell reads (api.js vault() -> real.owned)
        "count": owned.len(),
        "wallet_linked": !wallet.trim().is_empty(),
        "source": "chain-access+library",
    }))
}

/// The per-user Vault surfaces the SWR layer caches (their compute is a bounded Multicall3 sweep over the
/// discovery index + IPFS enrichment — the part worth not re-running on every tab toggle).
#[derive(Clone, Copy)]
enum VaultSurface {
    Owned,
    Listed,
}

impl VaultSurface {
    fn as_str(self) -> &'static str {
        match self {
            VaultSurface::Owned => "owned",
            VaultSurface::Listed => "listed",
        }
    }
}

/// In-memory, per-PRINCIPAL serve-stale-while-revalidate cache for the Vault's per-user surfaces — the
/// runtime's "subgraph feel": a repeat load (tab toggle, re-open) serves the last computed result INSTANTLY
/// and refreshes in a single-flight BACKGROUND sweep, instead of re-running the Multicall3 + enrichment
/// every request. Keyed by `(surface, principal_id)` — NOT wallet — because Owned folds in the principal's
/// local Library, which must never leak across principals. IN-MEMORY ONLY: a principal's holdings are
/// sensitive and are never written to a shared on-disk snapshot (unlike the wallet-independent discovery
/// index). The money path NEVER trusts this (buy re-verifies live, Phase-1); holdings change slowly, so
/// brief staleness is safe and self-heals on the next request.
type VaultSurfaceCache = Mutex<HashMap<String, (Instant, Arc<serde_json::Value>)>>;

fn vault_surface_cache() -> &'static VaultSurfaceCache {
    static C: OnceLock<VaultSurfaceCache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn vault_surface_refreshing() -> &'static Mutex<std::collections::HashSet<String>> {
    static R: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

async fn vault_surface_compute(
    kind: VaultSurface,
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
) -> Result<serde_json::Value, String> {
    match kind {
        VaultSurface::Owned => compute_owned(state.clone(), context.clone()).await,
        VaultSurface::Listed => compute_listed(state.clone(), context.clone()).await,
    }
}

/// Serve-stale-while-revalidate around a per-principal Vault surface. Under `FRESH` serve as-is; under
/// `MAX_STALE` serve the cached value and kick a single-flight background refresh; past `MAX_STALE` (or
/// cold) compute synchronously and store (fail-closed to freshness on a cold miss).
async fn vault_surface_swr(
    kind: VaultSurface,
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
) -> Result<Arc<serde_json::Value>, String> {
    const FRESH: Duration = Duration::from_secs(15);
    const MAX_STALE: Duration = Duration::from_secs(120);
    let key = format!("{}:{}", kind.as_str(), context.principal_id);
    let cached = vault_surface_cache()
        .lock()
        .ok()
        .and_then(|g| g.get(&key).map(|(at, v)| (*at, Arc::clone(v))));
    if let Some((at, v)) = cached {
        let age = at.elapsed();
        if age < FRESH {
            return Ok(v);
        }
        if age < MAX_STALE {
            // single-flight background revalidation (clone state+context into the task; both are Clone)
            let claimed = vault_surface_refreshing()
                .lock()
                .map(|mut g| g.insert(key.clone()))
                .unwrap_or(false);
            if claimed {
                let s = state.clone();
                let c = context.clone();
                let k = key.clone();
                tokio::spawn(async move {
                    if let Ok(val) = vault_surface_compute(kind, &s, &c).await {
                        if let Ok(mut g) = vault_surface_cache().lock() {
                            g.insert(k.clone(), (Instant::now(), Arc::new(val)));
                        }
                    }
                    if let Ok(mut g) = vault_surface_refreshing().lock() {
                        g.remove(&k);
                    }
                });
            }
            return Ok(v);
        }
    }
    let val = Arc::new(vault_surface_compute(kind, state, context).await?);
    if let Ok(mut g) = vault_surface_cache().lock() {
        g.insert(key, (Instant::now(), Arc::clone(&val)));
    }
    Ok(val)
}

/// `GET /api/market/vault` — the buyer's OWNED view (on-chain access holdings ∪ local Library), served
/// through the per-principal SWR cache so repeat loads don't re-sweep. Home-token-gated.
pub(super) async fn market_vault(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let context = match require_market_token_context(&state.data_dir, &headers) {
        Ok(c) => c,
        Err(e) => return order_forbidden(&e.to_string()),
    };
    match vault_surface_swr(VaultSurface::Owned, &state, &context).await {
        Ok(v) => Json((*v).clone()).into_response(),
        Err(e) => market_error(&e),
    }
}

/// `GET /api/market/listed` — the seller's ACTIVE resale listings (the Vault "Listed" tab). elacity reads
/// this from its subgraph; the runtime reads each asset's CURRENT `listings(operative, wallet)` live over
/// the discovery index in ONE Multicall3 sweep (the same decode the buy path binds to, P10) — no indexer
/// (P5/P13). Home-token-gated (per-user). Fail-CLOSED: no linked wallet, or any failed read, yields an
/// empty list — never a fabricated listing. Each row carries the operative + remaining quantity so the
/// withdraw flow can route a correct unsigned `withdrawListing`.
async fn compute_listed(
    state: GatewayState,
    context: HomeLaunchTokenContext,
) -> Result<serde_json::Value, String> {
    let wallet =
        crate::api::viewer_open::resolve_subject_address(&state, &context.principal_id).await;
    if wallet.trim().is_empty() {
        return Ok(serde_json::json!({ "listed": [], "count": 0, "wallet_linked": false }));
    }
    let idx = match recent_index_cached(&state.data_dir) {
        Ok(i) => i,
        Err(e) => return Err(e),
    };
    // Snapshot the bounded candidate set (no index borrow held across the enrichment awaits below).
    let candidates: Vec<(String, String, serde_json::Value)> = idx
        .search(None, None, None)
        .into_iter()
        .take(VAULT_SCAN_MAX)
        .map(|l| {
            (
                l.operative_address.clone(),
                l.token_uri.clone(),
                l.to_json(),
            )
        })
        .collect();
    let operatives: Vec<String> = candidates.iter().map(|(op, _, _)| op.clone()).collect();
    // ONE batched read of the wallet's current listing on each asset; fail-closed (empty on read failure).
    let w = wallet.clone();
    let listings =
        tokio::task::spawn_blocking(move || market_reads::my_listings_batched(&w, &operatives))
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
    let mut listed: Vec<serde_json::Value> = Vec::new();
    if listings.len() == candidates.len() {
        for ((operative, token_uri, mut row), active) in candidates.into_iter().zip(listings) {
            let Some((supply, price_minor, pay_token)) = active else {
                continue;
            };
            // Enrich for the row's display (name / cover / medium); skip silently if metadata won't resolve.
            if let Some(meta) = enrich_fields(&state, &token_uri).await {
                if let (Some(obj), Some(m)) = (row.as_object_mut(), meta.as_object()) {
                    for (k, v) in m {
                        obj.insert(k.clone(), v.clone());
                    }
                }
            }
            let (symbol, decimals) = pay_token_display(&pay_token);
            if let Some(obj) = row.as_object_mut() {
                obj.insert("my_qty".into(), serde_json::json!(supply.to_string()));
                obj.insert("my_price".into(), serde_json::json!(price_minor));
                obj.insert(
                    "my_price_formatted".into(),
                    serde_json::json!(format_minor_units(&price_minor, decimals)),
                );
                obj.insert("pay_token".into(), serde_json::json!(pay_token));
                obj.insert("pay_token_symbol".into(), serde_json::json!(symbol));
                // listing_id: this contract keys listings by (operative, ACCESS_TOKEN, seller) — no numeric
                // id — so the operative is the stable per-row key the shell uses for withdraw.
                obj.insert("listing_id".into(), serde_json::json!(operative));
            }
            listed.push(row);
        }
    }
    Ok(serde_json::json!({
        "listed": listed,
        "count": listed.len(),
        "wallet_linked": true,
    }))
}

/// `GET /api/market/listed` — the seller's ACTIVE resale listings, served through the per-principal SWR
/// cache so repeat loads don't re-sweep. Home-token-gated.
pub(super) async fn market_listed(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let context = match require_market_token_context(&state.data_dir, &headers) {
        Ok(c) => c,
        Err(e) => return order_forbidden(&e.to_string()),
    };
    match vault_surface_swr(VaultSurface::Listed, &state, &context).await {
        Ok(v) => Json((*v).clone()).into_response(),
        Err(e) => market_error(&e),
    }
}

/// `GET /api/market/me` — the signed-in principal's OWN market identity: their linked wallet address + the
/// self-asserted display name they set in Home (the same per-principal handle, read from the one canonical
/// source via `load_gateway_identity_summary_for_context`). Home-token-gated, per-user. The shell uses this
/// ONLY to label the user's OWN cards (creator_address == this wallet) with their handle instead of a bare
/// address — a truthful SELF short-circuit (Phase 0 of creator profiles), never a claim about anyone else.
/// Fail-CLOSED: an empty handle (or no linked wallet) leaves the shell showing the address + identicon.
pub(super) async fn market_me(State(state): State<GatewayState>, headers: HeaderMap) -> Response {
    let context = match require_market_token_context(&state.data_dir, &headers) {
        Ok(c) => c,
        Err(e) => return order_forbidden(&e.to_string()),
    };
    let wallet =
        crate::api::viewer_open::resolve_subject_address(&state, &context.principal_id).await;
    let display_name = super::gateway_home_runtime::load_gateway_identity_summary_for_context(
        &state.data_dir,
        &context,
    )
    .handle
    .unwrap_or_default();
    Json(serde_json::json!({
        "wallet": wallet,
        "display_name": display_name,
    }))
    .into_response()
}

#[derive(serde::Deserialize)]
pub(super) struct AcquireReq {
    content_id: String, // the bytes16 KID — entitlement (hasAccessByContentId) is checked on THIS
    #[serde(default)]
    content_cid: String, // fallback encrypted CID (used only if no token_uri to resolve the canonical one)
    #[serde(default)]
    token_uri: Option<String>, // preferred: resolve the CANONICAL content_cid from metadata + verify the KID
    #[serde(default)]
    uri: Option<String>,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
    /// When true, return immediately after the (synchronous) entitlement gate + CID↔KID binding and run the
    /// pin/materialize dispatch in the BACKGROUND, so a large download never holds the HTTP request open. The
    /// shell then polls `GET /api/market/acquire-status`. Absent/false keeps the legacy synchronous behavior
    /// (the buy path and standalone open fallback rely on it). No trusted-core change — orchestration only.
    #[serde(default)]
    background: bool,
}

/// Ephemeral, in-memory tracking of a BACKGROUND acquire's lifecycle, keyed `"{principal_id}:{pin_cid}"`.
/// Gateway-layer state only (like the SWR/enrich caches) — nothing persisted, no trusted-core growth. The
/// durable source of truth is still the materialized file in `…/Acquired`; this map only lets the status
/// endpoint report an in-progress run and, crucially, a TRUTHFUL failure (which file-presence alone can't).
#[derive(Clone)]
enum AcquireState {
    Running,
    Done,
    Failed(String),
}
fn acquire_inflight() -> &'static Mutex<HashMap<String, (Instant, AcquireState)>> {
    static C: OnceLock<Mutex<HashMap<String, (Instant, AcquireState)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}
fn set_acquire_state(key: &str, st: AcquireState) {
    if let Ok(mut g) = acquire_inflight().lock() {
        // Opportunistic TTL sweep so the map can't grow unbounded across many downloads (10 min horizon).
        let cutoff = Duration::from_secs(600);
        g.retain(|_, (at, _)| at.elapsed() < cutoff);
        g.insert(key.to_string(), (Instant::now(), st));
    }
}
fn get_acquire_state(key: &str) -> Option<AcquireState> {
    acquire_inflight()
        .lock()
        .ok()
        .and_then(|g| g.get(key).map(|(_, st)| st.clone()))
}

/// Fetch the asset's RAW `metadata.json` (the full published descriptor, including `asset.protections` and
/// the top-level `media` DASH layout) — distinct from `enrich_from_token_uri`, which returns the normalized
/// card subset. Needed to reconstruct the openable dKMS capsule on acquire.
async fn fetch_raw_asset_metadata(
    state: &GatewayState,
    token_uri: &str,
) -> Option<serde_json::Value> {
    let cid = market_reads::extract_cid(token_uri)?;
    let subpath = market_reads::extract_cid_subpath(token_uri);
    let registry = state.provider_registry.as_ref()?;
    // Acquire-path metadata read: bounded like the other interactive marketplace fetches (a
    // purchasable asset's metadata.json resolved moments ago on the detail view, so 30s is
    // generous); on timeout the acquire fails closed with an explicit error, not a hang.
    let bytes = crate::content::fetch_bytes_via_provider_bounded(
        registry,
        &cid,
        subpath.as_deref(),
        Some(30_000),
    )
    .await
    .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Reconstruct the OPENABLE dKMS capsule from the asset's published `metadata.json`. The capsule is what the
/// open path (`viewer_open::dkms_capsule`) parses to find the on-chain identity (the bytes16 KID) and the
/// quorum escrow (`protections`) — a minted asset stores it locally; a bought asset reconstructs it here so
/// it opens the SAME way (P10). A media asset carries the DASH `media` layout + `asset_cid` (the open fetches
/// the directory at open time); a single-file asset has neither here — `library_acquire` inlines its
/// `ciphertext_b64` from the fetched bytes. Returns `None` when the metadata lacks the quorum protections (a
/// non-dDRM asset), so acquire falls back to the legacy raw materialize rather than writing a broken capsule.
fn build_acquire_capsule(raw: &serde_json::Value, content_cid: &str) -> Option<serde_json::Value> {
    let asset = raw.get("asset")?;
    let kid = asset
        .get("kid")
        .and_then(serde_json::Value::as_str)
        .or_else(|| raw.get("kid").and_then(serde_json::Value::as_str))
        .and_then(market_reads::normalize_kid)?;
    let protections = asset
        .get("protections")
        .filter(|p| p.as_array().map(|a| !a.is_empty()).unwrap_or(false))?;
    let mime = asset
        .get("mimeType")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            raw.get("media")
                .and_then(|m| m.get("mimeType"))
                .and_then(serde_json::Value::as_str)
        })
        .unwrap_or("application/octet-stream");
    let mut capsule = serde_json::json!({
        "schema": "elastos.ddrm.capsule/v1",
        "content_id": kid,
        "kid": kid,
        "mime": mime,
        "asset_cid": content_cid,
        "protections": protections.clone(),
    });
    if let Some(media) = raw.get("media").filter(|m| !m.is_null()) {
        capsule["media"] = media.clone();
    }
    if let Some(title) = raw.get("name").and_then(serde_json::Value::as_str) {
        capsule["title"] = serde_json::json!(title);
    }
    // `content_size` + `thumbnail` mirror the field names the Library reads from a minted capsule
    // (`DdrmCapsuleHints`) so a bought asset shows its real size + cover art, not the on-disk capsule size.
    if let Some(size) = asset.get("size") {
        capsule["content_size"] = size.clone();
    }
    if let Some(thumb) = raw
        .get("image")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.trim().is_empty())
    {
        capsule["thumbnail"] = serde_json::json!(thumb);
    }
    Some(capsule)
}

/// Resolve the acquire PLACEMENT — the destination metadata the object-provider materializes. When the asset
/// has a `token_uri`, reconstruct its openable capsule and route it to the type-correct folder with its real
/// mime; otherwise fall back to the client-supplied `uri`/`metadata` (legacy raw materialize). Returns the
/// gateway-derived `(uri, metadata)` so neither the destination nor the capsule is client-controlled.
async fn resolve_acquire_placement(
    state: &GatewayState,
    token_uri: Option<&str>,
    content_cid: &str,
    client_meta: Option<&serde_json::Value>,
    client_uri: Option<&str>,
) -> (Option<String>, Option<serde_json::Value>) {
    let fallback = || (client_uri.map(str::to_string), client_meta.cloned());
    let Some(tu) = token_uri.filter(|s| !s.trim().is_empty()) else {
        return fallback();
    };
    let Some(raw) = fetch_raw_asset_metadata(state, tu).await else {
        return fallback();
    };
    let Some(capsule) = build_acquire_capsule(&raw, content_cid) else {
        return fallback();
    };
    let mime = raw
        .get("asset")
        .and_then(|a| a.get("mimeType"))
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            raw.get("media")
                .and_then(|m| m.get("mimeType"))
                .and_then(serde_json::Value::as_str)
        })
        .or_else(|| {
            client_meta
                .and_then(|m| m.get("mime"))
                .and_then(serde_json::Value::as_str)
        })
        .unwrap_or("application/octet-stream")
        .to_string();
    let name = client_meta
        .and_then(|m| m.get("name"))
        .and_then(serde_json::Value::as_str)
        .or_else(|| raw.get("name").and_then(serde_json::Value::as_str))
        .unwrap_or("asset")
        .to_string();
    // No `uri`/`folder` here: `library_acquire` places the `.ddrm` capsule via the SAME authority the mint
    // uses (`library_folder_for_mime` keyed on `mime`), so a bought asset files exactly like a minted one.
    let meta = serde_json::json!({
        "name": name,
        "mime": mime,
        "capsule": capsule,
    });
    (None, Some(meta))
}

/// The Library folders a bought asset can land in: the type-correct user folders (where a `.ddrm` capsule is
/// filed, mirroring the mint via `library_folder_for_mime`) plus the legacy `Acquired/`. Scanned together so
/// the vault + acquire-status find a downloaded asset wherever it was placed.
const ACQUIRE_SCAN_FOLDERS: [&str; 5] = ["Videos", "Music", "Pictures", "Documents", "Acquired"];

/// List the buyer's acquired objects across the type-correct folders. In the typed user folders only `.ddrm`
/// capsules are taken (a bought asset), so the buyer's OWN files in Documents/Pictures/… are never swept into
/// the vault; the legacy `Acquired/` bucket is taken whole (older raw materializations). Best-effort: a
/// missing folder just yields nothing.
async fn list_acquired_objects(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
) -> Vec<serde_json::Value> {
    let root = crate::auth::principal_localhost_root(&context.principal_id);
    let is_ddrm = |o: &serde_json::Value| -> bool {
        o.get("name")
            .and_then(serde_json::Value::as_str)
            .map(|n| n.to_ascii_lowercase().ends_with(".ddrm"))
            .unwrap_or(false)
            || o.get("mime").and_then(serde_json::Value::as_str) == Some("application/x-ddrm")
    };
    let mut out = Vec::new();
    for folder in ACQUIRE_SCAN_FOLDERS {
        let acquired_bucket = folder == "Acquired";
        let uri = format!("{root}/{folder}");
        if let Ok(r) = crate::api::viewer_gateway::viewer_object_provider_request(
            state,
            context,
            MARKETPLACE_CAPSULE_ID,
            "list",
            serde_json::json!({ "uri": uri }),
        )
        .await
        {
            if r.get("status").and_then(serde_json::Value::as_str) == Some("ok") {
                if let Some(objs) = r
                    .get("data")
                    .and_then(|d| d.get("objects"))
                    .and_then(serde_json::Value::as_array)
                {
                    out.extend(
                        objs.iter()
                            .filter(|o| acquired_bucket || is_ddrm(o))
                            .cloned(),
                    );
                }
            }
        }
    }
    out
}

/// Dispatch the object-provider `acquire` op (pin + materialize into the buyer Library) and normalize the
/// reply to `Ok(data)` / `Err(message)`. Shared by the synchronous and background acquire paths so both bind
/// to the exact same canonical decode (P10).
async fn dispatch_acquire_op(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    pin_cid: &str,
    uri: Option<&str>,
    metadata: Option<&serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let mut req = serde_json::json!({ "content_cid": pin_cid });
    if let Some(u) = uri.filter(|s| !s.trim().is_empty()) {
        req["uri"] = serde_json::Value::String(u.to_string());
    }
    if let Some(md) = metadata {
        req["metadata"] = md.clone();
    }
    match crate::api::viewer_gateway::viewer_object_provider_request(
        state,
        context,
        MARKETPLACE_CAPSULE_ID,
        "acquire",
        req,
    )
    .await
    {
        Ok(resp) if resp.get("status").and_then(serde_json::Value::as_str) == Some("ok") => {
            Ok(resp.get("data").cloned().unwrap_or(resp))
        }
        Ok(resp) => Err(resp
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("acquire failed")
            .to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// `POST /api/market/acquire {content_id, content_cid, uri?, metadata?}` — the entitlement-gated TRIGGER
/// for buy→pin. Verifies the buyer actually holds on-chain access to `content_id` (the bytes16 KID) via
/// the canonical rights gate (`decide_owned_access` → `hasAccessByContentId`), then dispatches the
/// object-provider `Acquire` op to pin `content_cid` into the buyer's Library. This is the UPSTREAM gate
/// the Acquire op is designed around (the object-provider itself pins only what it is told). Home-token-
/// gated; holds no keys. Fails closed: no entitlement → 403, no pin.
pub(super) async fn market_acquire(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(b): Json<AcquireReq>,
) -> Response {
    let context = match require_market_token_context(&state.data_dir, &headers) {
        Ok(c) => c,
        Err(e) => return order_forbidden(&e.to_string()),
    };
    if b.content_id.trim().is_empty() {
        return order_error("content_id (the bytes16 KID) is required");
    }
    if b.content_cid.trim().is_empty() && b.token_uri.as_deref().unwrap_or("").trim().is_empty() {
        return order_error("acquire needs token_uri (preferred) or content_cid");
    }
    // Entitlement gate (the audit's requirement): the buyer must hold on-chain access to the KID. Mirrors
    // the open-time gate (viewer_open) exactly — same rights path, run off the async pool.
    let subject =
        crate::api::viewer_open::resolve_subject_address(&state, &context.principal_id).await;
    let now = crate::auth::now_ts();
    let principal_id = context.principal_id.clone();
    let session = context.session_id.clone();
    let content_id = b.content_id.clone();
    let rights = tokio::task::spawn_blocking(move || {
        crate::api::rights_authority::decide_owned_access(
            &principal_id,
            &session,
            &content_id,
            &subject,
            "view",
            "marketplace acquire (buy->pin)",
            None,
            now,
            3600,
        )
    })
    .await;
    let rights = match rights {
        Ok(Ok(decision)) => decision,
        Ok(Err(err)) => {
            if err.contains("wallet not linked") {
                return (StatusCode::FORBIDDEN, "link an EVM wallet to acquire").into_response();
            }
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("rights gate unavailable: {err}"),
            )
                .into_response();
        }
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "rights gate task panicked",
            )
                .into_response()
        }
    };
    if !rights.allowed {
        return (
            StatusCode::FORBIDDEN,
            "not entitled: buy this asset before acquiring it",
        )
            .into_response();
    }
    // CID<->KID binding (closes the deep-audit LOW finding): if the asset's token_uri is given, resolve the
    // CANONICAL content_cid from its metadata and verify `metadata.kid == the gated KID` — then pin THAT,
    // ignoring the client-supplied content_cid (which the entitlement gate never bound). Fail closed on a KID
    // mismatch. Absent a token_uri, fall back to the client CID (bounded: opaque ciphertext + open re-gates
    // on the embedded KID).
    let pin_cid = match b.token_uri.as_ref().filter(|s| !s.trim().is_empty()) {
        Some(token_uri) => {
            let Some(meta) = enrich_from_token_uri(&state, token_uri).await else {
                return market_error(
                    "acquire: could not resolve asset metadata to bind the CID to the KID",
                );
            };
            let meta_kid = meta
                .get("kid")
                .and_then(serde_json::Value::as_str)
                .and_then(market_reads::normalize_kid);
            let want_kid = market_reads::normalize_kid(&b.content_id);
            if want_kid.is_none() || meta_kid != want_kid {
                return (
                    StatusCode::FORBIDDEN,
                    "acquire: asset metadata KID does not match the entitled content_id (fail closed)",
                )
                    .into_response();
            }
            match meta.get("content_cid").and_then(serde_json::Value::as_str) {
                Some(c) if !c.is_empty() => c.to_string(),
                _ => return market_error("acquire: asset metadata has no content CID (media.uri)"),
            }
        }
        None => b.content_cid.clone(),
    };
    // Placement (P10 — one canonical, OPENABLE artifact): reconstruct the dKMS capsule from the asset's
    // published metadata and route it to the type-correct Library folder with its real mime, so the bought
    // asset opens exactly like a minted one and lands where the File Explorer expects it. Gateway-derived
    // (never client-controlled); falls back to the client uri/metadata for a non-dDRM / token_uri-less asset.
    let (acq_uri, acq_meta) = resolve_acquire_placement(
        &state,
        b.token_uri.as_deref(),
        &pin_cid,
        b.metadata.as_ref(),
        b.uri.as_deref(),
    )
    .await;
    // Entitled. BACKGROUND mode: register Running, spawn the pin/materialize dispatch, and return at once so
    // a multi-GB download never holds this request open — the shell polls `/acquire-status`. The entitlement
    // gate + CID↔KID binding above already ran synchronously, so we never spawn work for an unentitled caller.
    if b.background {
        let key = format!("{}:{}", context.principal_id, pin_cid);
        set_acquire_state(&key, AcquireState::Running);
        let s = state.clone();
        let c = context.clone();
        let cid = pin_cid.clone();
        let uri = acq_uri.clone();
        let md = acq_meta.clone();
        tokio::spawn(async move {
            let st = match dispatch_acquire_op(&s, &c, &cid, uri.as_deref(), md.as_ref()).await {
                Ok(_) => AcquireState::Done,
                Err(e) => AcquireState::Failed(e),
            };
            set_acquire_state(&key, st);
        });
        return Json(serde_json::json!({ "status": "started", "content_cid": pin_cid }))
            .into_response();
    }
    // SYNCHRONOUS (default): dispatch + return the materialized object (the buy path + standalone open rely
    // on this shape). viewer_object_provider_request injects op + principal_id from the Home token.
    match dispatch_acquire_op(
        &state,
        &context,
        &pin_cid,
        acq_uri.as_deref(),
        acq_meta.as_ref(),
    )
    .await
    {
        Ok(data) => Json(data).into_response(),
        Err(e) => market_error(&e),
    }
}

#[derive(serde::Deserialize)]
pub(super) struct AcquireStatusQuery {
    /// The asset's content CID (canonical, from the listing's enriched `content_cid`). When `token_uri` is
    /// also given we re-resolve the canonical CID server-side so status matches exactly what acquire pins.
    #[serde(default)]
    cid: Option<String>,
    #[serde(default)]
    token_uri: Option<String>,
}

/// `GET /api/market/acquire-status?cid=&token_uri=` — TRUTHFUL download state for a background acquire,
/// derived from primitives that already exist (no new persisted state, trusted core untouched). It checks
/// two sources in order: first the DURABLE truth — is the asset materialized in the buyer's Library
/// (canonical object-provider `list` across the type-correct folders, the same read the Vault uses)? →
/// `downloaded` + its Library `uri`.
/// Otherwise the EPHEMERAL in-flight map — `downloading` while the spawned task runs, or `failed` with the
/// real error (which file-presence alone can never report).
///
/// Home-token-gated (per-principal). Fail-CLOSED: any read failure degrades to `idle`/`downloading`, never a
/// fabricated "downloaded". No progress percentage is invented — the pin is opaque, so state is honest+binary.
pub(super) async fn market_acquire_status(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Query(q): Query<AcquireStatusQuery>,
) -> Response {
    let context = match require_market_token_context(&state.data_dir, &headers) {
        Ok(c) => c,
        Err(e) => return order_forbidden(&e.to_string()),
    };
    // Resolve the CANONICAL content CID exactly as acquire does (token_uri metadata wins), so the file we
    // look for is the one acquire actually materializes. Falls back to the client-supplied cid.
    let cid = match q.token_uri.as_ref().filter(|s| !s.trim().is_empty()) {
        Some(tu) => enrich_from_token_uri(&state, tu)
            .await
            .and_then(|m| {
                m.get("content_cid")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
            .or_else(|| q.cid.clone()),
        None => q.cid.clone(),
    };
    let cid = match cid.map(|c| c.trim().to_string()).filter(|c| !c.is_empty()) {
        Some(c) => c,
        None => return order_error("acquire-status needs cid or token_uri"),
    };
    // (1) Durable truth: is it materialized in the buyer's Library? A bought `.ddrm` capsule reports its
    // ASSET content_cid, so it matches the canonical `cid` regardless of which type-correct folder it landed
    // in. (Lost in-flight state after a restart still resolves here.)
    let downloaded_uri = list_acquired_objects(&state, &context)
        .await
        .into_iter()
        .find(|o| o.get("content_cid").and_then(serde_json::Value::as_str) == Some(cid.as_str()))
        .and_then(|o| {
            o.get("uri")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        });
    if let Some(uri) = downloaded_uri {
        return Json(serde_json::json!({ "state": "downloaded", "downloaded": true, "uri": uri }))
            .into_response();
    }
    // (2) No file yet → consult the ephemeral in-flight map for a running/failed background run.
    let key = format!("{}:{}", context.principal_id, cid);
    let (st, message) = match get_acquire_state(&key) {
        Some(AcquireState::Running) => ("downloading", None),
        Some(AcquireState::Failed(m)) => ("failed", Some(m)),
        // Done but the file isn't listed yet (object-provider write/list race) — finalizing, not idle.
        Some(AcquireState::Done) => ("downloading", None),
        None => ("idle", None),
    };
    Json(serde_json::json!({ "state": st, "downloaded": false, "message": message }))
        .into_response()
}

// ---- secondary-market resale order assembly (UNSIGNED; routed to wallet) ----

#[derive(serde::Deserialize)]
pub(super) struct SellReq {
    #[serde(default)]
    gateway: Option<String>,
    ledger: String,
    token_id: String,
    #[serde(default)]
    quantity: String,
    #[serde(default)]
    price: String,
    #[serde(default)]
    pay_token: Option<String>,
}

#[derive(serde::Deserialize)]
pub(super) struct WithdrawReq {
    #[serde(default)]
    gateway: Option<String>,
    operative: String,
    token_id: String,
    #[serde(default)]
    quantity: String,
}

#[derive(serde::Deserialize)]
pub(super) struct ApproveReq {
    operative: String,
    #[serde(default)]
    gateway: Option<String>,
}

/// Numeric body fields arrive as strings (tokenIds + 18-dp prices exceed JSON-safe integers).
fn u128_or(s: &str, default: u128) -> u128 {
    if s.trim().is_empty() {
        default
    } else {
        s.trim().parse().unwrap_or(default)
    }
}

fn unsigned_tx_json(tx: &trade_authority::UnsignedTx, selector: &str, note: &str) -> Response {
    Json(serde_json::json!({
        "unsigned_tx": { "to": tx.to, "data": tx.data, "value": tx.value, "selector": selector, "note": note }
    }))
    .into_response()
}

fn order_error(err: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": err })),
    )
        .into_response()
}

/// 403 for a missing/invalid Home launch token on the resale-order surface — parity with the sibling
/// `/api/market/buy` (`buy_owned_access`) and the spec's "gate behind the same auth as other viewer
/// routes" (PHASE2 Chunk 3). Keeps the authority model uniform (P7/P16) so this assembler surface can
/// never silently become a money path.
fn order_forbidden(err: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({ "error": err })),
    )
        .into_response()
}

/// A 20-byte EVM address (`0x` + 40 hex). Reject anything else so a caller-supplied destination (`to`)
/// fails early and explicitly instead of assembling an unsigned tx with a garbage `to` (P11).
fn is_evm_address(s: &str) -> bool {
    let h = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    h.len() == 40 && h.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `POST /api/market/order/sell` — assemble an UNSIGNED `sellAccess` (list owned access for resale).
/// Routed to wallet; the shell holds no keys (P16). PRE: `setApprovalForAll` on the operative (/approve).
pub(super) async fn market_order_sell(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(b): Json<SellReq>,
) -> Response {
    if let Err(e) = require_market_token_context(&state.data_dir, &headers) {
        return order_forbidden(&e.to_string());
    }
    let gateway = b
        .gateway
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_GATEWAY);
    if !is_evm_address(gateway) {
        return order_error("gateway is not a 20-byte EVM address");
    }
    let pay_token = b
        .pay_token
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_PAY_TOKEN);
    match trade_authority::build_sell_access(
        gateway,
        &b.ledger,
        &b.token_id,
        u128_or(&b.quantity, 1),
        u128_or(&b.price, 0),
        pay_token,
    ) {
        Ok(tx) => unsigned_tx_json(
            &tx,
            "sellAccess(ledger,tokenId,quantity,pricePerToken,payToken)",
            "UNSIGNED — routed to wallet; PRE: setApprovalForAll(gateway) on the operative",
        ),
        Err(e) => order_error(&e),
    }
}

/// `POST /api/market/order/withdraw` — assemble an UNSIGNED `withdrawListing` (cancel a resale listing,
/// keyed by OPERATIVE). The access right is unaffected.
pub(super) async fn market_order_withdraw(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(b): Json<WithdrawReq>,
) -> Response {
    if let Err(e) = require_market_token_context(&state.data_dir, &headers) {
        return order_forbidden(&e.to_string());
    }
    let gateway = b
        .gateway
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_GATEWAY);
    if !is_evm_address(gateway) {
        return order_error("gateway is not a 20-byte EVM address");
    }
    match trade_authority::build_withdraw_listing(
        gateway,
        &b.operative,
        &b.token_id,
        u128_or(&b.quantity, 1),
    ) {
        Ok(tx) => unsigned_tx_json(
            &tx,
            "withdrawListing(operative,tokenId,quantity)",
            "UNSIGNED — routed to wallet; only the resale listing is withdrawn",
        ),
        Err(e) => order_error(&e),
    }
}

/// `POST /api/market/order/approve` — assemble the UNSIGNED ERC-1155 `setApprovalForAll` on the Operative
/// (operator = AuthorityGateway), the prerequisite for listing access for resale.
pub(super) async fn market_order_approve(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(b): Json<ApproveReq>,
) -> Response {
    if let Err(e) = require_market_token_context(&state.data_dir, &headers) {
        return order_forbidden(&e.to_string());
    }
    let gateway = b
        .gateway
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_GATEWAY);
    if !is_evm_address(&b.operative) {
        return order_error("operative is not a 20-byte EVM address");
    }
    match trade_authority::build_set_approval_for_all(&b.operative, gateway, true) {
        Ok(tx) => unsigned_tx_json(
            &tx,
            "setApprovalForAll(operator,true)",
            "UNSIGNED — sent to the Operative ERC-1155; operator = gateway",
        ),
        Err(e) => order_error(&e),
    }
}

#[cfg(test)]
mod tests {
    use super::{content_category, format_minor_units, is_evm_address, pay_token_display};

    /// Lane-orchestration tests for `advance_index` against a SCRIPTED chain-provider stub (the
    /// same subprocess seam the buy tests use): each `chain_tx` call is one fresh conversation
    /// (init + one op), so the stub routes on the op — `block_number` answers from `head.txt`,
    /// `logs` answers from a per-`fromBlock` canned file (`logs_<0xfrom>.json`), a `fail_<0xfrom>`
    /// marker forces that window to error. Proves the cursor semantics end-to-end: cold seed,
    /// contiguous delta, bounded backfill, fail-soft partial progress, and deploy-block completion.
    #[cfg(all(unix, feature = "dev-modes"))]
    mod advance_index_lanes {
        use crate::api::content_index::{
            ASSET_CREATED_TOPIC0, EVENT_HUB_DEPLOY_BLOCK, REORG_OVERLAP_BLOCKS,
        };
        use serde_json::{json, Value};
        use std::os::unix::fs::PermissionsExt as _;
        use std::path::Path;

        const WINDOW: u64 = 10_000;

        /// A minimal well-formed `AssetCreated` log at `block` for a distinct `(channel, tokenId)`.
        fn ac_log(token_id: u64, block: u64) -> Value {
            let pad_addr = |a: &str| format!("{:0>64}", a.trim_start_matches("0x").to_lowercase());
            let uri = format!("ipfs://bafy-{token_id}");
            let uri_hex: String = uri.bytes().map(|b| format!("{b:02x}")).collect();
            let data = format!(
                "0x{:064x}{:064x}{:064x}{:064x}{}",
                token_id,
                96, // uri byte-offset
                1,  // opType buy_once
                uri.len(),
                format!("{uri_hex:0<width$}", width = uri.len().div_ceil(32) * 64),
            );
            json!({
                "topics": [
                    ASSET_CREATED_TOPIC0,
                    pad_addr("0x1111111111111111111111111111111111111111"),
                    pad_addr(&format!("0x{:040x}", 0xaaaa_0000_u64 + token_id)),
                    pad_addr(&format!("0x{:040x}", 0xb0b0_0000_u64 + token_id)),
                ],
                "data": data,
                "blockNumber": format!("0x{block:x}"),
            })
        }

        /// Write the scripted stub + `head.txt` into `dir`; returns the stub path.
        fn write_stub(dir: &Path, head: u64) -> std::path::PathBuf {
            std::fs::write(dir.join("head.txt"), format!("0x{head:x}")).unwrap();
            let stub = dir.join("scripted-chain.sh");
            std::fs::write(
                &stub,
                format!(
                    "#!/bin/sh\nread _init\nprintf '{{\"status\":\"ok\",\"data\":{{}}}}\\n'\n\
                     read op\ncase \"$op\" in\n\
                     *block_number*) printf '{{\"status\":\"ok\",\"data\":{{\"block_number\":\"%s\"}}}}\\n' \"$(cat {dir}/head.txt)\";;\n\
                     *)\n  from=$(printf '%s' \"$op\" | sed -n 's/.*\"fromBlock\":\"\\(0x[0-9a-f]*\\)\".*/\\1/p')\n\
                     if [ -f \"{dir}/fail_$from\" ]; then printf '{{\"status\":\"err\",\"message\":\"stub rpc failure\"}}\\n'\n\
                     elif [ -f \"{dir}/logs_$from.json\" ]; then cat \"{dir}/logs_$from.json\"\n\
                     else printf '{{\"status\":\"ok\",\"data\":{{\"logs\":[]}}}}\\n'\nfi;;\nesac\n",
                    dir = dir.display(),
                ),
            )
            .unwrap();
            std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
            stub
        }

        fn write_logs(dir: &Path, from: u64, logs: &[Value]) {
            std::fs::write(
                dir.join(format!("logs_0x{from:x}.json")),
                json!({ "status": "ok", "data": { "logs": logs } }).to_string(),
            )
            .unwrap();
        }

        fn set_env(stub: &Path) {
            std::env::set_var("ELASTOS_CHAIN_PROVIDER_BIN", stub);
            std::env::set_var("ELASTOS_CHAIN_BASE_RPC", "http://127.0.0.1:9");
            std::env::set_var("ELASTOS_MARKET_DISCOVERY_WINDOWS", "1");
            std::env::set_var("ELASTOS_MARKET_BACKFILL_WINDOWS", "1");
        }

        fn clear_env() {
            for k in [
                "ELASTOS_CHAIN_PROVIDER_BIN",
                "ELASTOS_CHAIN_BASE_RPC",
                "ELASTOS_MARKET_DISCOVERY_WINDOWS",
                "ELASTOS_MARKET_BACKFILL_WINDOWS",
            ] {
                std::env::remove_var(k);
            }
        }

        #[test]
        fn seeds_then_advances_delta_and_backfill_and_keeps_progress_on_rpc_failure() {
            let _g = crate::api::ddrm_env_lock();
            let dir = tempfile::tempdir().unwrap();
            let head = EVENT_HUB_DEPLOY_BLOCK + 1_000_000;
            let stub = write_stub(dir.path(), head);
            set_env(&stub);

            // Cycle 1 (cold): one discovery window [head-9999, head] carrying listing #1 seeds the cursor.
            write_logs(dir.path(), head - (WINDOW - 1), &[ac_log(1, head - 5)]);
            let idx = super::super::advance_index(None).expect("cold seed cycle");
            assert!(idx.cursor_set());
            assert_eq!(idx.scanned_to(), head);
            assert_eq!(idx.backfill_low(), head - (WINDOW - 1));
            assert_eq!(idx.len(), 1);
            assert_eq!(idx.coverage(), "indexing");

            // Cycle 2: the delta lane re-scans only the reorg overlap (head unchanged, one window);
            // the backfill lane takes exactly ONE window below the cursor and finds listing #2.
            let bf1_from = head - (2 * WINDOW - 1);
            write_logs(dir.path(), bf1_from, &[ac_log(2, head - 15_000)]);
            // The delta overlap window replaces listing #1's row at a new block (reorg re-derive).
            write_logs(
                dir.path(),
                idx.scanned_to() - REORG_OVERLAP_BLOCKS + 1,
                &[ac_log(1, head - 3)],
            );
            let idx = super::super::advance_index(Some(&idx)).expect("delta+backfill cycle");
            assert_eq!(idx.scanned_to(), head, "delta lane stays at head");
            assert_eq!(
                idx.backfill_low(),
                bf1_from,
                "one backfill window per cycle"
            );
            assert_eq!(idx.len(), 2, "backfill found the older listing");
            let newest = idx.search(None, None, None);
            assert_eq!(
                newest[0].first_seen_block,
                head - 3,
                "the overlap re-scan re-derived the reorged row (idempotent upsert, higher block wins)"
            );

            // Cycle 3: the next backfill window RPC-fails — the cycle is fail-soft: it returns the
            // progress it made and the cursor does NOT move past the failed window.
            std::fs::write(
                dir.path()
                    .join(format!("fail_0x{:x}", head - (3 * WINDOW - 1))),
                b"",
            )
            .unwrap();
            let idx = super::super::advance_index(Some(&idx)).expect("fail-soft cycle still Ok");
            assert_eq!(
                idx.backfill_low(),
                bf1_from,
                "a failed window leaves backfill_low unchanged — next cycle retries it"
            );
            assert_eq!(idx.len(), 2);

            clear_env();
        }

        #[test]
        fn backfill_reaching_the_deploy_block_flips_coverage_to_indexed() {
            let _g = crate::api::ddrm_env_lock();
            let dir = tempfile::tempdir().unwrap();
            // Head close enough that one seed window + one backfill window reach the deploy block.
            let head = EVENT_HUB_DEPLOY_BLOCK + 15_000;
            let stub = write_stub(dir.path(), head);
            set_env(&stub);

            let idx = super::super::advance_index(None).expect("cold seed");
            assert_eq!(idx.backfill_low(), head - (WINDOW - 1)); // deploy+5001
            assert!(!idx.backfill_complete());

            let idx = super::super::advance_index(Some(&idx)).expect("final backfill window");
            assert!(idx.backfill_complete(), "backfill reached the deploy block");
            assert_eq!(idx.coverage(), "indexed");
            assert_eq!(idx.backfill_low(), EVENT_HUB_DEPLOY_BLOCK);

            clear_env();
        }
    }

    #[test]
    fn content_category_refines_mime_with_metadata_truthfully() {
        let none: &[String] = &[];
        // MIME bases when no finer signal is declared.
        assert_eq!(content_category("video/mp4", none, none), "video");
        assert_eq!(content_category("audio/mpeg", none, none), "audio");
        assert_eq!(content_category("image/png", none, none), "image");
        assert_eq!(content_category("application/pdf", none, none), "document");
        // MIME-implied 3D / e-book regardless of tags.
        assert_eq!(content_category("model/gltf-binary", none, none), "3d");
        assert_eq!(
            content_category("application/epub+zip", none, none),
            "ebook"
        );
        // Metadata overrides the MIME base for kinds MIME can't express (comic is image/*, article is text/*).
        assert_eq!(
            content_category("image/jpeg", &["Comic".to_string()], none),
            "comic"
        );
        assert_eq!(
            content_category("text/plain", none, &["article".to_string()]),
            "article"
        );
        // No resolvable signal -> honest fallback, never a guessed facet.
        assert_eq!(content_category("application/zip", none, none), "other");
        assert_eq!(content_category("", none, none), "other");
    }

    #[test]
    fn format_minor_units_renders_human_amounts() {
        // USDC (6 dp): whole, fractional, sub-unit, and trailing-zero trimming.
        assert_eq!(format_minor_units("4000000", 6), "4");
        assert_eq!(format_minor_units("1500000", 6), "1.5");
        assert_eq!(format_minor_units("10000", 6), "0.01");
        assert_eq!(format_minor_units("0", 6), "0");
        // ETH/18 dp.
        assert_eq!(format_minor_units("1000000000000000000", 18), "1");
        // 0 decimals = identity; non-numeric falls back to the raw string (never misreports).
        assert_eq!(format_minor_units("123", 0), "123");
        assert_eq!(format_minor_units("not-a-number", 6), "not-a-number");
    }

    #[test]
    fn pay_token_display_maps_known_tokens_and_defaults_unknown() {
        assert_eq!(
            pay_token_display("0x0000000000000000000000000000000000000000"),
            ("ETH".to_string(), 18)
        );
        assert_eq!(pay_token_display(""), ("ETH".to_string(), 18));
        // USDC mapped case-insensitively to 6 dp.
        assert_eq!(
            pay_token_display("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
            ("USDC".to_string(), 6)
        );
        // Unknown ERC-20: defaults to 18 dp with a truncated-address symbol (never claims USDC).
        let (sym, dec) = pay_token_display("0x1234567890abcdef1234567890abcdef12345678");
        assert_eq!(dec, 18);
        assert!(sym.starts_with("0x1234") && sym.ends_with("5678") && sym.contains('…'));
    }

    #[test]
    fn is_evm_address_accepts_20_byte_hex_and_rejects_others() {
        assert!(is_evm_address("0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D"));
        assert!(is_evm_address("09dbe796f40eceffeaccf243c3d758c4c1d8d87d")); // no 0x prefix
        assert!(!is_evm_address("0x09dBe7")); // too short
        assert!(!is_evm_address(
            "0xZZdBe796f40ECEffEAccf243c3d758C4c1d8D87D"
        )); // non-hex
        assert!(!is_evm_address("")); // empty
        assert!(!is_evm_address(
            "0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87Dff"
        )); // too long
    }
}
