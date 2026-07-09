//! The negotiate seam (Sprint 50 — Track D3): the middle leg of the shop loop
//! (quote → **negotiate** → pay). An agent under a pay-mandate makes a BOUNDED OFFER for a
//! mandate-scoped asset; the SELLER decides. The runtime supplies the AGENT side of the protocol —
//! a mandate-capped, receipted way to make an offer — and injects a [`Negotiator`] for the SELLER
//! side, exactly as the quote spine injects a [`MarketQuoter`](crate::market_quote::MarketQuoter).
//!
//! WHAT THE RUNTIME OWNS (in the `runtime.negotiate` executor, not here): the offer is a canonical
//! positive integer of SPEND UNITS, and it is refused BEFORE reaching the seller if it exceeds the
//! mandate's UN-SPENT cap (`SpendMeter::remaining`). That is the one provable property this leg
//! adds: **an agent can never propose to commit its operator beyond the granted, un-spent
//! authority** — the same ceiling the buy enforces, applied to the OFFER.
//!
//! WHAT THIS SEAM OWNS: the seller's accept/counter/reject decision, and any unit math the
//! seller's own price domain needs. The runtime passes the offer as an opaque spend-unit integer
//! and RELAYS the seller's answer; it does not interpret the seller's numbers or attest them (a
//! counterparty's word is not something the runtime can verify — see the receipt note below).
//!
//! NOT VALUE-MOVING BY CONSTRUCTION: negotiate produces AGREED/COUNTER TERMS, never a charge. It
//! touches neither the ledger nor the meter's balance (it only READS `remaining`). Settlement stays
//! `runtime.pay`, which re-checks the cap and runs the whole two-generals custody path. So there is
//! no two-generals problem here: nothing is broadcast, nothing is reserved, no money can move.
//!
//! THE RECEIPT records the ACT — "under this mandate, the agent offered N spend units for asset X"
//! (N is signed, and N ≤ cap is proven) — NOT the seller's disposition. The accept-vs-counter
//! outcome and the seller's price ride the response's agent-visible channel, exactly as a quote's
//! terms do (ephemeral market data, not something the runtime signs on a counterparty's behalf).

use std::sync::Arc;

/// The seller's answer to one bounded offer. `price`/`pay_token` are the seller's own price domain
/// (rail units for the listing seller) — DISPLAY data the agent reads to decide its next move
/// (pay the counter, re-offer, or walk); the runtime never interprets them numerically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NegotiationOutcome {
    /// The seller ACCEPTS at `price` in `pay_token`. A fair seller accepts AT its ask, never above
    /// the offer — the agent pays `price`, not whatever it over-offered.
    Accepted { price: String, pay_token: String },
    /// The seller COUNTERS with `price` in `pay_token` (its firm terms). The agent may pay it,
    /// re-negotiate, or walk — each a fresh receipted dispatch.
    Countered { price: String, pay_token: String },
    /// No terms — a length-bounded reason. The negotiation act performed no agreement (⇒ the
    /// executor DECLINES, `authorized_not_performed`), exactly as an unreadable quote does.
    Rejected(String),
}

impl NegotiationOutcome {
    /// The canonical one-line report the agent receives for a SUCCESSFUL negotiation (accept or
    /// counter): `outcome=<accept|counter>;price=<p>;tok=<t>`. `None` for a rejection (no terms).
    /// Separator bytes in seller-sourced fields are stripped, the same defense the DRM `rail_ref`
    /// and [`MarketQuote::canonical_terms`](crate::market_quote::MarketQuote::canonical_terms) use,
    /// so a hostile seller cannot forge extra segments into the signed-adjacent report.
    pub fn agent_report(&self) -> Option<String> {
        let clean = |s: &str| s.replace([';', '='], "");
        match self {
            NegotiationOutcome::Accepted { price, pay_token } => Some(format!(
                "outcome=accept;price={};tok={}",
                clean(price),
                clean(pay_token)
            )),
            NegotiationOutcome::Countered { price, pay_token } => Some(format!(
                "outcome=counter;price={};tok={}",
                clean(price),
                clean(pay_token)
            )),
            NegotiationOutcome::Rejected(_) => None,
        }
    }
}

/// The seller seam. CI injects a scripted seller; the DRM deployment injects [`ListingNegotiator`].
/// `offer` is in SPEND UNITS (the mandate's cap domain); the implementor maps it into its own price
/// domain if it needs to. Blocking is fine (the caller runs on the blocking pool) but the
/// implementor MUST bound any I/O it does — the listing seller inherits the quote spine's chain-read
/// deadline (S40) for exactly this reason.
pub trait Negotiator: Send + Sync {
    fn negotiate(&self, asset: &str, offer: u64) -> NegotiationOutcome;
}

/// The production seller for the DRM marketplace: a FIXED-PRICE ("take it or leave it") seller
/// backed by the SAME read-only quote spine the Marketplace panel and `runtime.market_quote` read
/// (one live chain read per asset per TTL window, no keys, no broadcast — P3). It does NOT haggle
/// below its ask, because on-chain DRM listings are fixed-price: a deployment with a discountable
/// seller wires a different [`Negotiator`]. The VALUE this leg delivers is the AGENT-side
/// mandate-bound offer primitive and the loop closure (quote → negotiate → pay), which holds
/// whatever the seller's sophistication.
///
/// THE DECISION, reusing the buy gate's EXACT conversion (never a second, divergent one):
/// 1. Read the live listing terms. Unreadable / sold out / no listing ⇒ [`Rejected`] (no agreement
///    on an unreadable listing — the honest counterpart of the buy quote failing NotCharged).
/// 2. PAY-TOKEN GUARD (council S36 F3, mirrored): if the listing quotes a different pay-token than
///    the deployment declared `spend_unit` FOR, the offer and the ask are in incomparable
///    denominations ⇒ [`Rejected`]. Never accept across token denominations.
/// 3. `authorized = offer × spend_unit` (checked; overflow ⇒ [`Rejected`]) — the offer in the
///    listing's own base-units, the identical `amount × spend_unit` the buy price gate computes.
/// 4. `authorized ≥ price` ⇒ [`Accepted`] at the LISTING price (never above the offer). Else
///    ⇒ [`Countered`] with the listing price (the seller's firm ask).
///
/// HONEST BOUND: the terms are as of the spine's LAST read (≤ `MARKET_QUOTE_TTL_SECS` old, shared
/// with the panel/quote fan-out bound); a listing that changes inside the cache window is caught on
/// the next re-read. A settlement STILL re-quotes live and aborts on drift (the buy gate binds the
/// quote), so a stale accept here can never make the agent overpay at pay time.
pub struct ListingNegotiator {
    cache: crate::market_quote::MarketQuoteCache,
    quoter: Arc<dyn crate::market_quote::MarketQuoter>,
    /// Pay-token smallest-units per spend unit — the SAME mapping the DRM buy gate is wired with.
    spend_unit: u128,
    /// The pay-token that `spend_unit` denominates, if the deployment declared one (live Chain
    /// mode always does; dev/chain-mock may omit it, matching the buy gate).
    expected_pay_token: Option<String>,
}

impl ListingNegotiator {
    pub fn new(
        cache: crate::market_quote::MarketQuoteCache,
        quoter: Arc<dyn crate::market_quote::MarketQuoter>,
        spend_unit: u128,
        expected_pay_token: Option<String>,
    ) -> Self {
        Self {
            cache,
            quoter,
            // Mirror the buy gate's `spend_unit.max(1)` floor: a zero mapping would make every
            // offer authorize nothing (0 base-units) and counter forever — the buy provider clamps
            // to 1 for the same reason, so the two conversions stay identical.
            spend_unit: spend_unit.max(1),
            expected_pay_token,
        }
    }
}

impl Negotiator for ListingNegotiator {
    fn negotiate(&self, asset: &str, offer: u64) -> NegotiationOutcome {
        // Read the live listing through the shared spine (bounded, single-flight, TTL-cached).
        let quote = match crate::market_quote::quote_single_flight(
            &self.cache,
            self.quoter.as_ref(),
            asset,
            crate::market_quote::now_unix(),
        ) {
            Ok(q) => q,
            Err(crate::market_quote::ReadInFlight) => {
                return NegotiationOutcome::Rejected(
                    "a listing read for this asset is already in flight — retry shortly"
                        .to_string(),
                );
            }
        };
        let (Some(price_str), Some(pay_token)) = (quote.price.clone(), quote.pay_token.clone())
        else {
            return NegotiationOutcome::Rejected(format!(
                "the listing could not be read: {}",
                quote.error.as_deref().unwrap_or("no terms returned")
            ));
        };
        // PAY-TOKEN GUARD: the offer is denominated (via `spend_unit`) in exactly one token; a
        // listing in any other token is incomparable ⇒ refuse, never accept across denominations.
        if let Some(want) = &self.expected_pay_token {
            if !pay_token.trim().eq_ignore_ascii_case(want.trim()) {
                return NegotiationOutcome::Rejected(format!(
                    "the listing quotes pay-token {pay_token} but the declared spend-unit mapping \
                     is for {want} — the offer cannot be compared across token denominations"
                ));
            }
        }
        // Fail-closed on an unparseable listing price (the same posture as the buy gate).
        let price: u128 = match price_str.trim().parse() {
            Ok(p) => p,
            Err(_) => {
                return NegotiationOutcome::Rejected(format!(
                    "the listing price is not a parseable amount ({price_str})"
                ));
            }
        };
        // `authorized = offer × spend_unit`, the IDENTICAL conversion the buy price gate computes.
        let Some(authorized) = (offer as u128).checked_mul(self.spend_unit) else {
            return NegotiationOutcome::Rejected(
                "offer × spend_unit overflowed — refusing to negotiate an unrepresentable amount"
                    .to_string(),
            );
        };
        if authorized >= price {
            // The offer covers the ask — accept AT the ask (never take more than listed).
            NegotiationOutcome::Accepted {
                price: price.to_string(),
                pay_token,
            }
        } else {
            // Below ask — the fixed-price seller counters with its firm listing price.
            NegotiationOutcome::Countered {
                price: price.to_string(),
                pay_token,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::buy_authority::BuyQuote;

    /// A quoter that returns a fixed listing (or an error) — the seam the ListingNegotiator reads.
    struct FixedQuoter(Result<BuyQuote, String>);
    impl crate::market_quote::MarketQuoter for FixedQuoter {
        fn quote(&self, _: &str) -> Result<BuyQuote, String> {
            self.0.clone()
        }
    }

    fn negotiator_over(
        listing: Result<BuyQuote, String>,
        spend_unit: u128,
        pay_token: Option<&str>,
    ) -> ListingNegotiator {
        ListingNegotiator::new(
            Arc::default(),
            Arc::new(FixedQuoter(listing)),
            spend_unit,
            pay_token.map(|s| s.to_string()),
        )
    }

    fn listing(price: &str, tok: &str) -> Result<BuyQuote, String> {
        Ok(BuyQuote {
            price: price.to_string(),
            pay_token: tok.to_string(),
            supply: 1,
        })
    }

    #[test]
    fn an_offer_that_covers_the_ask_is_accepted_at_the_ask_not_above() {
        // spend_unit 1_000_000 (USDC 6dp): offer 5 ⇒ authorized 5_000_000 ≥ price 3_000_000.
        let neg = negotiator_over(listing("3000000", "0xUSDC"), 1_000_000, Some("0xUSDC"));
        let out = neg.negotiate("QmA", 5);
        assert_eq!(
            out,
            NegotiationOutcome::Accepted {
                price: "3000000".to_string(), // AT the ask, not the over-offer
                pay_token: "0xUSDC".to_string(),
            }
        );
        assert_eq!(
            out.agent_report().unwrap(),
            "outcome=accept;price=3000000;tok=0xUSDC"
        );
    }

    #[test]
    fn an_offer_below_the_ask_is_countered_with_the_firm_listing_price() {
        // offer 2 ⇒ authorized 2_000_000 < price 3_000_000 ⇒ counter at 3_000_000.
        let neg = negotiator_over(listing("3000000", "0xUSDC"), 1_000_000, Some("0xUSDC"));
        let out = neg.negotiate("QmA", 2);
        assert_eq!(
            out,
            NegotiationOutcome::Countered {
                price: "3000000".to_string(),
                pay_token: "0xUSDC".to_string(),
            }
        );
        assert_eq!(
            out.agent_report().unwrap(),
            "outcome=counter;price=3000000;tok=0xUSDC"
        );
    }

    #[test]
    fn a_listing_in_a_different_pay_token_is_rejected_never_accepted_across_denominations() {
        let neg = negotiator_over(listing("1", "0xWETH"), 1_000_000, Some("0xUSDC"));
        assert!(matches!(
            neg.negotiate("QmA", 100),
            NegotiationOutcome::Rejected(why) if why.contains("across token denominations")
        ));
    }

    #[test]
    fn an_unreadable_listing_yields_no_agreement() {
        let neg = negotiator_over(Err("chain unreachable".to_string()), 1, None);
        assert!(matches!(
            neg.negotiate("QmA", 100),
            NegotiationOutcome::Rejected(why) if why.contains("could not be read")
        ));
    }

    #[test]
    fn an_unparseable_listing_price_is_rejected_fail_closed() {
        let neg = negotiator_over(listing("not-a-number", "native"), 1, None);
        assert!(matches!(
            neg.negotiate("QmA", 100),
            NegotiationOutcome::Rejected(why) if why.contains("not a parseable amount")
        ));
    }

    #[test]
    fn the_conversion_overflow_refuses_rather_than_wraps() {
        // offer × spend_unit overflows u128 ⇒ refuse (never wrap into a bogus "authorized").
        let neg = negotiator_over(listing("1", "native"), u128::MAX, Some("native"));
        assert!(matches!(
            neg.negotiate("QmA", u64::MAX),
            NegotiationOutcome::Rejected(why) if why.contains("overflow")
        ));
    }

    #[test]
    fn a_zero_spend_unit_is_floored_to_one_like_the_buy_gate() {
        // spend_unit 0 would authorize nothing; the floor to 1 keeps it identical to the buy gate.
        let neg = negotiator_over(listing("5", "native"), 0, Some("native"));
        // offer 5 × floored-unit 1 = 5 ≥ price 5 ⇒ accept.
        assert!(matches!(
            neg.negotiate("QmA", 5),
            NegotiationOutcome::Accepted { .. }
        ));
    }

    #[test]
    fn separator_bytes_in_seller_terms_cannot_forge_report_segments() {
        let out = NegotiationOutcome::Accepted {
            price: "3;tok=evil".to_string(),
            pay_token: "0x=;USDC".to_string(),
        };
        assert_eq!(
            out.agent_report().unwrap(),
            "outcome=accept;price=3tokevil;tok=0xUSDC",
            "a hostile seller cannot inject extra ;/= segments into the report"
        );
    }
}
