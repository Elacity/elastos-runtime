//! The ONE market-quote spine (Sprint 39): a single-flight, TTL-cached, read-only view of a DRM
//! listing's live terms, shared by BOTH quote consumers — the Mandates app's Marketplace panel
//! (Sprint 38) and the agent-facing `runtime.market_quote` affordance (Sprint 39). One cache, one
//! claim discipline, one fan-out bound: however many surfaces ask, an asset costs at most one
//! live chain read per TTL window.
//!
//! READ-ONLY BY CONSTRUCTION: the only chain call behind this module is
//! [`buy_authority::quote_buy`](crate::api::buy_authority::quote_buy) — no keys, no broadcast
//! (P3). CI injects a [`MarketQuoter`] mock; production uses [`LiveMarketQuoter`].

use std::sync::Arc;

/// How long one asset's quote is served from cache before a re-read. Together with the in-flight
/// sentinel this makes the fan-out bound literal: at most one LIVE chain read per asset per
/// window, however many concurrent consumers race (a miss claims the slot under the lock before
/// any read starts; later misses see the claim and wait for the cache).
pub const MARKET_QUOTE_TTL_SECS: u64 = 30;

/// One asset's quote outcome: either the live terms or a length-bounded error string (rendered
/// via textContent only on the panel; surfaced as a decline reason to an agent).
#[derive(Clone, serde::Serialize)]
pub struct MarketQuote {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pay_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supply: Option<u128>,
    /// Length-bounded read failure — the asset stays listed/answerable, honestly unquoted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl MarketQuote {
    /// The canonical one-line terms encoding for a SUCCESSFUL quote —
    /// `price=<p>;tok=<t>;supply=<s>` — the string an attested quote intent declares and the
    /// executor echoes (declared-vs-done). `None` when this quote is an error outcome.
    pub fn canonical_terms(&self) -> Option<String> {
        match (&self.price, &self.pay_token, self.supply) {
            (Some(price), Some(tok), Some(supply)) if self.error.is_none() => {
                // The same `;`/`=` stripping the DRM rail_ref uses, so segments can't be forged.
                let clean = |s: &str| s.replace([';', '='], "");
                Some(format!(
                    "price={};tok={};supply={supply}",
                    clean(price),
                    clean(tok)
                ))
            }
            _ => None,
        }
    }
}

/// One cache slot: `quote: None` is the IN-FLIGHT sentinel — a consumer has claimed this asset's
/// chain read and not yet finished. The sentinel expires by the same TTL (a crashed fetch can
/// never wedge an asset), and concurrent consumers that see a fresh sentinel get "in progress"
/// instead of spawning a duplicate read (single-flight).
#[derive(Clone)]
pub struct MarketQuoteSlot {
    pub quoted_at: u64,
    pub quote: Option<MarketQuote>,
}

/// The per-process quote cache: asset → slot. Pruned by TTL on every claim pass, so it stays
/// bounded by the recently-asked asset set. Lives on [`PayRail`](crate::api::server::PayRail) —
/// the same one-per-process home as the meter and ledger — so the panel and the affordance share
/// it same-Arc by construction.
pub type MarketQuoteCache =
    Arc<std::sync::Mutex<std::collections::HashMap<String, MarketQuoteSlot>>>;

/// What one single-flight claim pass says about one asset.
pub enum CachedQuote {
    /// A fresh quote is cached — served free, no chain read.
    Fresh(MarketQuote),
    /// Another consumer's read is in flight — do not duplicate it; retry shortly.
    InFlight,
    /// THIS consumer claimed the read (the sentinel is now in place): perform it, then [`fill`].
    Claimed,
    /// No cached quote and the caller declined to claim (its fresh-read budget is spent).
    NotClaimed,
}

/// The single-flight claim pass for ONE asset, under the cache lock: prune expired slots, serve a
/// fresh quote, respect an in-flight claim, or (when `may_claim`) claim the read by inserting the
/// sentinel BEFORE any chain call starts — the step that makes "one live read per asset per
/// window" literal under concurrency.
pub fn claim_or_serve(
    cache: &MarketQuoteCache,
    asset: &str,
    now: u64,
    may_claim: bool,
) -> CachedQuote {
    let mut cache = match cache.lock() {
        Ok(c) => c,
        Err(poisoned) => poisoned.into_inner(),
    };
    let fresh = |slot: &MarketQuoteSlot| now.saturating_sub(slot.quoted_at) < MARKET_QUOTE_TTL_SECS;
    match cache.get(asset) {
        Some(slot) if fresh(slot) => match &slot.quote {
            Some(quote) => CachedQuote::Fresh(quote.clone()),
            None => CachedQuote::InFlight,
        },
        // Absent OR stale (an expired quote/sentinel reads as absent — freshness is checked
        // inline so callers prune ONCE per pass, not per asset; council S39 fold).
        _ if may_claim => {
            cache.insert(
                asset.to_string(),
                MarketQuoteSlot {
                    quoted_at: now,
                    quote: None,
                },
            );
            CachedQuote::Claimed
        }
        _ => CachedQuote::NotClaimed,
    }
}

/// Drop every slot past the TTL — the size bound. Call ONCE per batch pass (the panel view) or
/// per single-asset ask; `claim_or_serve` itself never scans the whole map.
pub fn prune(cache: &MarketQuoteCache, now: u64) {
    if let Ok(mut cache) = cache.lock() {
        cache.retain(|_, slot| now.saturating_sub(slot.quoted_at) < MARKET_QUOTE_TTL_SECS);
    }
}

/// Publish a completed read into the cache, stamped at COMPLETION time (a slow fetch must not be
/// served as fresh for a full TTL past its actual read time).
pub fn fill(cache: &MarketQuoteCache, asset: &str, quote: MarketQuote, completed_at: u64) {
    if let Ok(mut cache) = cache.lock() {
        cache.insert(
            asset.to_string(),
            MarketQuoteSlot {
                quoted_at: completed_at,
                quote: Some(quote),
            },
        );
    }
}

/// The read seam: CI injects a mock; production injects [`LiveMarketQuoter`], whose only call is
/// the existing read-only `quote_buy` path (no new chain code).
pub trait MarketQuoter: Send + Sync {
    fn quote(&self, asset: &str) -> Result<crate::api::buy_authority::BuyQuote, String>;
}

/// Production quoter — `buy_authority::quote_buy` with an empty target (the same call the panel's
/// batch pass makes).
pub struct LiveMarketQuoter;
impl MarketQuoter for LiveMarketQuoter {
    fn quote(&self, asset: &str) -> Result<crate::api::buy_authority::BuyQuote, String> {
        crate::api::buy_authority::quote_buy(asset, &crate::api::buy_authority::BuyTarget::default())
    }
}

/// Convert one quoter result into the cacheable outcome, bounding the error string (a chain error
/// must not balloon a panel payload or a decline reason).
pub fn quote_outcome(result: Result<crate::api::buy_authority::BuyQuote, String>) -> MarketQuote {
    match result {
        Ok(q) => MarketQuote {
            price: Some(q.price),
            pay_token: Some(q.pay_token),
            supply: Some(q.supply),
            error: None,
        },
        Err(e) => MarketQuote {
            price: None,
            pay_token: None,
            supply: None,
            error: Some(e.chars().take(200).collect()),
        },
    }
}

/// One asset's quote through the shared spine, blocking (the caller runs on the blocking pool):
/// serve fresh, refuse to duplicate an in-flight read, or claim-read-fill. `Err(())` ⇒ another
/// consumer's read is in flight — retry shortly (bounded wait is the CALLER's policy; this module
/// never sleeps).
pub fn quote_single_flight(
    cache: &MarketQuoteCache,
    quoter: &dyn MarketQuoter,
    asset: &str,
    now: u64,
) -> Result<MarketQuote, ()> {
    prune(cache, now); // single-asset ask: one bounded prune keeps the cache size-bounded
    match claim_or_serve(cache, asset, now, true) {
        CachedQuote::Fresh(quote) => Ok(quote),
        CachedQuote::InFlight => Err(()),
        CachedQuote::Claimed => {
            let quote = quote_outcome(quoter.quote(asset));
            fill(cache, asset, quote.clone(), now_unix());
            Ok(quote)
        }
        // Unreachable with may_claim=true, but fail SAFE (treat as in-flight) if it ever is.
        CachedQuote::NotClaimed => Err(()),
    }
}

/// Seconds since the unix epoch (the cache's clock).
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_terms_strip_separator_bytes_and_require_success() {
        let ok = MarketQuote {
            price: Some("50;00=00".to_string()),
            pay_token: Some("0xUSDC".to_string()),
            supply: Some(3),
            error: None,
        };
        assert_eq!(
            ok.canonical_terms().unwrap(),
            "price=500000;tok=0xUSDC;supply=3",
            "separator bytes in chain-sourced fields cannot forge extra segments"
        );
        let err = MarketQuote {
            price: None,
            pay_token: None,
            supply: None,
            error: Some("nope".to_string()),
        };
        assert!(err.canonical_terms().is_none(), "an error outcome has no terms");
    }

    #[test]
    fn single_flight_claims_once_and_serves_the_cache() {
        struct CountingQuoter(std::sync::atomic::AtomicUsize);
        impl MarketQuoter for CountingQuoter {
            fn quote(&self, _: &str) -> Result<crate::api::buy_authority::BuyQuote, String> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(crate::api::buy_authority::BuyQuote {
                    price: "5".to_string(),
                    pay_token: "native".to_string(),
                    supply: 1,
                })
            }
        }
        let cache: MarketQuoteCache = Arc::default();
        let quoter = CountingQuoter(std::sync::atomic::AtomicUsize::new(0));
        let now = now_unix();
        let first = quote_single_flight(&cache, &quoter, "QmA", now).expect("claim + read");
        assert_eq!(first.price.as_deref(), Some("5"));
        let second = quote_single_flight(&cache, &quoter, "QmA", now).expect("cache hit");
        assert_eq!(second.price.as_deref(), Some("5"));
        assert_eq!(
            quoter.0.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "one live read per asset per window — the second ask is a cache hit"
        );
        // A fresh in-flight sentinel refuses a duplicate read rather than racing it.
        claim_or_serve(&cache, "QmB", now, true); // claims QmB, never filled
        assert!(matches!(
            claim_or_serve(&cache, "QmB", now, true),
            CachedQuote::InFlight
        ));
        assert!(
            quote_single_flight(&cache, &quoter, "QmB", now).is_err(),
            "a concurrent claim is respected — no duplicate chain read"
        );
    }
}
