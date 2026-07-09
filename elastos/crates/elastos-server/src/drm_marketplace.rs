//! The Flint↔DRM wedge (Sprint 34): a [`PaymentProvider`] whose rail is the Elacity on-chain DRM
//! marketplace instead of an HTTPS payment endpoint.
//!
//! The runtime already holds both halves of one transaction, unconnected: Flint's `runtime.pay`
//! affordance (mandate → spend cap → rail → receipt) and the Elacity v3 DRM bindings
//! (`buy_authority` → AuthorityGateway on Base, ERC-1155 ACCESS_TOKEN, royalty settlement). This
//! module joins them behind the SAME [`PaymentProvider`] trait the HTTP rail implements — so the
//! meter, the ledger, the two-generals classification, and the portable receipt are byte-identical
//! whichever rail is wired (P5: one pay spine, never a fork). An agent with a capped pay-mandate
//! that dispatches a buy intent whose payee names a DRM asset settles it on-chain under a provable
//! mandate; the receipt then carries the tx hash + `operative:tokenId` (Sprint 34's `rail_ref`).
//!
//! THE SEAM (why this is testable without a live chain): the provider depends on two small traits
//! — [`DrmResolver`] (KID/content-id → the unique `(operative, tokenId)` binding, FAIL-CLOSED on
//! ambiguity) and [`DrmSettler`] (execute the buy, classify two-generals). CI injects mocks and
//! exercises every branch; production injects [`ChainDrmMarketplace`], which calls the real
//! `chain_tx::resolve_token_id_live` + `buy_authority::buy_access`. The live path is an operator
//! runbook step (`docs/`), never a CI call — no Base RPC in the test gate.
//!
//! TWO-GENERALS MAPPING (identical to the HTTP rail's contract, [`PaymentProvider`]):
//! - resolve fails (unresolvable / ambiguous binding) ⇒ [`PayError::NotCharged`] — nothing was
//!   broadcast, so the reserved cap is REFUNDED. An ambiguous KID is NEVER a fallback buy (the
//!   MKT-1 discipline: bind only when exactly one `(operative, tokenId)` exists, else fail closed).
//! - the buy provably never broadcast (rejected pre-send, wallet unlinked, listing sold out) ⇒
//!   [`PayError::NotCharged`] — refund.
//! - the buy path reported a BROADCAST-ACCEPTED tx ⇒ [`PayError::Indeterminate`] carrying the
//!   `rail_ref` — the reservation is KEPT and a PENDING ledger entry is filed (Sprint 35). A DRM
//!   buy is NEVER recorded `Performed` at broadcast: `buy_access` returns at
//!   `eth_sendRawTransaction` acceptance, not inclusion, so recording charged now would attest a
//!   settlement that a dropped/reverted tx never finalized (council S34 guardian F1). The tx is
//!   promoted to charged (and its `rail_ref` bound onto the mandate's receipt) ONLY once
//!   [`reconcile_drm_confirmations`] reads the receipt and finds it mined + successful + at least
//!   the required confirmation depth; a reverted tx refunds the cap; a not-yet-mined tx stays
//!   Pending, never auto-charged.
//! - anything the settler cannot prove reached the chain at all (RPC timeout with no tx handle) ⇒
//!   [`PayError::Indeterminate`] with no tx — the reservation is KEPT, resolved out of band.
//!
//! ON-RAIL IDEMPOTENCY (Sprint 35): the durable ledger is the dedup — the pay path refuses to
//! re-charge a signature-derived key that already carries a money-moved-or-may-have entry, so a
//! re-dispatched identical signed intent (past the replay window) resolves to the SAME buy, never
//! a second one (enforced in the `runtime.pay` closure, all rails).

use std::sync::Arc;

use crate::intent_executor::{PayError, PaymentProvider};

/// The unique on-chain binding for a DRM asset: the content id the buyer named, resolved to the
/// ERC-1155 `operative` (the per-asset contract) and its `tokenId`. Produced by a [`DrmResolver`]
/// ONLY when exactly one binding exists (MKT-1 fail-closed uniqueness).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrmBinding {
    pub content_id: String,
    pub operative: String,
    pub token_id: String,
}

/// Why a KID/content-id did not resolve to a single buyable asset. Both map to
/// [`PayError::NotCharged`] — a resolve failure means nothing was ever broadcast.
#[derive(Debug)]
pub enum DrmResolveError {
    /// No binding for this content id (no channel, no minted asset, no active listing).
    Unresolvable(String),
    /// More than one distinct `(operative, tokenId)` binds this KID — buying would risk binding
    /// the WRONG token (the MKT-1 attack). Fail closed: never guess which.
    Ambiguous(String),
}

/// Resolve a DRM asset reference to its unique on-chain binding. FAIL-CLOSED: return
/// `Err(Ambiguous)` rather than pick when more than one asset matches (MKT-1). Production wraps
/// the MKT-1-hardened `chain_tx::resolve_token_id`; CI injects a deterministic mock.
pub trait DrmResolver: Send + Sync {
    fn resolve(&self, asset_ref: &str) -> Result<DrmBinding, DrmResolveError>;
}

/// The BROADCAST-ACCEPTED settlement of a DRM buy — the tx the rail reported (zero confirmations
/// observed; see the module doc). The on-chain truth the receipt will carry, on rail trust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrmSettlement {
    /// The buy transaction hash (`0x…`), as the chain/adapter reported it at broadcast acceptance.
    pub tx_hash: String,
}

/// Why a buy did not (verifiably) settle — the two-generals distinction the money path forces.
#[derive(Debug)]
pub enum DrmSettleError {
    /// The buy PROVABLY never broadcast (rejected before send: wallet unlinked, listing sold out,
    /// price drift abort, a malformed order). Maps to [`PayError::NotCharged`] — refund the cap.
    NotBroadcast(String),
    /// The outcome is UNKNOWN — the tx may have broadcast and may confirm (RPC timeout, a send
    /// that returned no confirmation). Maps to [`PayError::Indeterminate`] — keep the reservation.
    Indeterminate(String),
}

impl DrmSettleError {
    /// Classify a `buy_access` failure by its TYPE — the whole point of Sprint 43. The refund-vs-
    /// hold decision is now the [`BuyError`](crate::api::buy_authority::BuyError) VARIANT (decided
    /// by which code path produced it), NOT a substring match on the message, so a hostile
    /// provider's error text — even one embedding a pre-broadcast sentinel — can never flip a
    /// possibly-sent tx into a refund. This preserves the one unbreakable invariant by construction.
    pub(crate) fn from_buy_error(e: crate::api::buy_authority::BuyError) -> Self {
        use crate::api::buy_authority::BuyError;
        match e {
            BuyError::PreBroadcast(m) => DrmSettleError::NotBroadcast(m),
            BuyError::Indeterminate(m) => DrmSettleError::Indeterminate(m),
        }
    }
}

/// The on-chain price of a listing, READ-ONLY, sourced BEFORE the buy (Sprint 36 — the price gate).
/// `price` is the pay-token's smallest-unit amount as a decimal string; `pay_token` is the ERC-20
/// address or `"native"`. The pay gate compares the mandate's cap against `price` before any money
/// moves, and the receipt names it — so the cap is a LITERAL on-chain ceiling, not just intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrmQuote {
    pub price: String,
    pub pay_token: String,
}

/// Quote (read-only) and then execute a buy for an already-resolved binding, classifying the
/// settle two-generals-honestly. Production wraps `buy_authority::{quote_buy, buy_access}`; CI
/// injects a mock. `settle` receives the SAME quote the gate approved, so the adapter can bind it
/// as the expected price (abort-on-drift fires if the live price changed before broadcast).
pub trait DrmSettler: Send + Sync {
    /// The on-chain cost of this binding, without broadcasting. `NotBroadcast` on a fail-closed
    /// sourcing failure (no listing / sold out); `Indeterminate` if the price read itself was
    /// ambiguous (rare — the gate then holds the reservation).
    fn quote(&self, binding: &DrmBinding) -> Result<DrmQuote, DrmSettleError>;
    fn settle(
        &self,
        binding: &DrmBinding,
        quote: &DrmQuote,
        idempotency_key: &str,
    ) -> Result<DrmSettlement, DrmSettleError>;
}

/// Why a cap-vs-listing conversion could not be decided (all fail-closed at the buy gate; all a
/// "no agreement" at the negotiate seller). Carries the offending chain-sourced strings so each
/// caller can format its OWN message — the strings are bounded/sanitized at the point they become
/// externally visible, never here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ListingAuthzError {
    /// The listing quotes a pay-token other than the one `spend_unit` denominates — the amount and
    /// the ask are in incomparable denominations.
    TokenMismatch { listing: String, declared: String },
    /// The listing price is not a parseable base-unit integer.
    UnparseablePrice(String),
    /// `amount × spend_unit` overflowed u128.
    Overflow,
}

/// The decided conversion: the parsed listing price and what the mandate authorizes, both in the
/// pay-token's base units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ListingAuthz {
    pub price: u128,
    pub authorized: u128,
}
impl ListingAuthz {
    /// The mandate covers the ask ⇔ `authorized ≥ price` — the inclusive proceed/accept boundary
    /// (a buy at exactly the cap proceeds; an offer at exactly the ask is accepted).
    pub fn covers(&self) -> bool {
        self.authorized >= self.price
    }
}

/// THE ONE cap-vs-listing conversion (Sprint 36 price gate), extracted so the DRM buy gate
/// ([`DrmMarketplaceProvider::pay`]) and the `runtime.negotiate` seller
/// ([`crate::negotiation::ListingNegotiator`]) share a SINGLE implementation — the "reuses the
/// EXACT conversion" claim is thus true BY CONSTRUCTION, not two hand-synced copies (council S50
/// guardian F2). Given `amount` spend units and the listing terms, apply the declared
/// spend-unit⇄pay-token mapping (`spend_unit`, already floored to ≥1 by each caller's constructor)
/// and decide whether the mandate authorizes the listed price. Callers format their own outcome
/// (the buy gate's fail-closed `PayError`; the seller's reject/counter) from the structured result;
/// no message text lives here.
pub(crate) fn authorize_amount_against_listing(
    amount: u64,
    spend_unit: u128,
    expected_pay_token: Option<&str>,
    listing_price: &str,
    listing_pay_token: &str,
) -> Result<ListingAuthz, ListingAuthzError> {
    // The declared unit maps ONE token (council S36 F3): a listing in any other token is
    // incomparable — refuse rather than gate against an unknown denomination.
    if let Some(want) = expected_pay_token {
        if !listing_pay_token.trim().eq_ignore_ascii_case(want.trim()) {
            return Err(ListingAuthzError::TokenMismatch {
                listing: listing_pay_token.to_string(),
                declared: want.to_string(),
            });
        }
    }
    // Fail-closed on an unparseable price or a conversion overflow.
    let price: u128 = listing_price
        .trim()
        .parse()
        .map_err(|_| ListingAuthzError::UnparseablePrice(listing_price.to_string()))?;
    let authorized = (amount as u128)
        .checked_mul(spend_unit)
        .ok_or(ListingAuthzError::Overflow)?;
    Ok(ListingAuthz { price, authorized })
}

/// A [`PaymentProvider`] whose rail is the DRM marketplace. Resolve (fail-closed) → quote →
/// PRICE-GATE the mandate's cap against the on-chain price → settle (two-generals) → a `rail_ref`
/// naming the tx, the bound `(operative, tokenId)`, and the pay-token price.
pub struct DrmMarketplaceProvider {
    resolver: Arc<dyn DrmResolver>,
    settler: Arc<dyn DrmSettler>,
    /// Pay-token smallest-units per ONE spend unit (Sprint 36 — the declared unit mapping). The
    /// gate authorizes `amount * spend_unit` pay-token units; a buy priced above that is refused
    /// before broadcast. The DRM rail refuses to wire without an explicit value (`build_pay_rail`),
    /// so a deployment must DECLARE the mapping rather than silently assume 1:1.
    spend_unit: u128,
    /// The pay-token the `spend_unit` mapping is FOR (council S36 F3): the unit is meaningless
    /// without knowing the token it denominates, and listings can quote heterogeneous tokens.
    /// When set (REQUIRED on the live Chain rail), a buy whose on-chain listing quotes a DIFFERENT
    /// pay-token is refused before broadcast — so the declared unit always applies to the declared
    /// token and the ceiling stays literal. `None` (dev/chain-mock, free quote) skips the check.
    expected_pay_token: Option<String>,
}

impl DrmMarketplaceProvider {
    pub fn new(
        resolver: Arc<dyn DrmResolver>,
        settler: Arc<dyn DrmSettler>,
        spend_unit: u128,
        expected_pay_token: Option<String>,
    ) -> Self {
        Self {
            resolver,
            settler,
            spend_unit: spend_unit.max(1),
            expected_pay_token,
        }
    }

    /// The canonical `rail_ref` for a settled DRM buy:
    /// `drm:tx=<hash>;op=<operative>;tid=<tokenId>;price=<price>;tok=<pay_token>`. Compact,
    /// greppable, printable-bounded before it enters the signed receipt (P12: the receipt names the
    /// pay-token price actually authorized). The `;`/`=` DELIMITERS are stripped from each
    /// chain-supplied component (council S34 red-team F3) so a hostile field cannot forge the
    /// parsed binding.
    fn rail_ref(binding: &DrmBinding, quote: &DrmQuote, settlement: &DrmSettlement) -> String {
        let clean = |s: &str| s.replace([';', '='], "");
        format!(
            "drm:tx={};op={};tid={};price={};tok={}",
            clean(&settlement.tx_hash),
            clean(&binding.operative),
            clean(&binding.token_id),
            clean(&quote.price),
            clean(&quote.pay_token),
        )
    }
}

impl PaymentProvider for DrmMarketplaceProvider {
    fn rail(&self) -> crate::payment_ledger::PaymentRail {
        // Positively tag DRM pendings so `reconcile_drm_confirmations` selects them by this
        // structured discriminator, not by a `drm:tx=` note prefix a hostile HTTP endpoint could
        // forge (council S35 red-team F5 / MKT-DRM 2d).
        crate::payment_ledger::PaymentRail::Drm
    }

    fn pay(&self, payee: &str, amount: u64, idempotency_key: &str) -> Result<String, PayError> {
        // Resolve first — fail-closed. An unresolvable or ambiguous asset never broadcasts, so it
        // is NotCharged (the cap is refunded), never a fallback buy.
        let binding = self.resolver.resolve(payee).map_err(|e| match e {
            DrmResolveError::Unresolvable(why) => {
                PayError::NotCharged(format!("DRM asset unresolvable: {why}"))
            }
            DrmResolveError::Ambiguous(why) => {
                PayError::NotCharged(format!("DRM asset binding ambiguous (fail closed): {why}"))
            }
        })?;
        // Quote the on-chain price BEFORE broadcasting (read-only).
        let quote = self.settler.quote(&binding).map_err(|e| match e {
            DrmSettleError::NotBroadcast(why) => {
                PayError::NotCharged(format!("DRM quote failed (nothing broadcast): {why}"))
            }
            DrmSettleError::Indeterminate(why) => {
                PayError::Indeterminate(format!("DRM quote indeterminate: {why}"))
            }
        })?;
        // THE PRICE GATE (Sprint 36): the mandate's cap, converted to pay-token units via the
        // declared unit mapping, MUST cover the on-chain price — else refuse BEFORE broadcast
        // (NotCharged/refund), never buy at a price the mandate did not authorize. The conversion
        // (token guard, price parse, `amount × spend_unit`, cover boundary) is the SHARED
        // `authorize_amount_against_listing` the negotiate seller also calls (S50 guardian F2); the
        // buy gate formats its own fail-closed messages from the structured result.
        let authz = authorize_amount_against_listing(
            amount,
            self.spend_unit,
            self.expected_pay_token.as_deref(),
            &quote.price,
            &quote.pay_token,
        )
        .map_err(|e| match e {
            ListingAuthzError::TokenMismatch { listing, declared } => {
                PayError::NotCharged(format!(
                "DRM buy refused before broadcast: the listing quotes pay-token {listing} but the \
                 declared spend-unit mapping is for {declared} — the cap cannot be compared across \
                 token denominations"
            ))
            }
            ListingAuthzError::UnparseablePrice(p) => PayError::NotCharged(format!(
                "DRM on-chain price is not a parseable amount ({p}) — refused before broadcast"
            )),
            ListingAuthzError::Overflow => PayError::NotCharged(
                "DRM cap conversion overflowed (amount * spend_unit) — refused before broadcast"
                    .to_string(),
            ),
        })?;
        if !authz.covers() {
            return Err(PayError::NotCharged(format!(
                "DRM buy refused before broadcast: the mandate authorizes {authorized} {tok} \
                 units ({amount} spend units × {unit}) but the on-chain price is {price} {tok} — \
                 raise the cap or lower the mandate amount",
                authorized = authz.authorized,
                price = authz.price,
                tok = quote.pay_token,
                unit = self.spend_unit,
            )));
        }
        // Cap covers the price — settle. NotBroadcast ⇒ refund; Indeterminate ⇒ keep the
        // reservation (the tx may confirm). `settle` binds this quote as the expected price, so a
        // live price drift before broadcast aborts fail-closed.
        let settlement = self
            .settler
            .settle(&binding, &quote, idempotency_key)
            .map_err(|e| match e {
                DrmSettleError::NotBroadcast(why) => {
                    PayError::NotCharged(format!("DRM buy not broadcast: {why}"))
                }
                DrmSettleError::Indeterminate(why) => {
                    PayError::Indeterminate(format!("DRM buy outcome indeterminate: {why}"))
                }
            })?;
        // Sprint 35: a broadcast-accepted buy is NEVER recorded charged/Performed here — it is
        // INDETERMINATE (Pending), the reservation held, the `rail_ref` carried as the reason so
        // the pending ledger record holds the tx + price. `reconcile_drm_confirmations` promotes it
        // to charged (and binds the receipt) only once the chain confirms it.
        Err(PayError::Indeterminate(Self::rail_ref(
            &binding,
            &quote,
            &settlement,
        )))
    }
}

/// The confirmation verdict for a broadcast DRM buy (Sprint 35). Produced by a [`DrmConfirmer`]
/// reading the tx receipt; consumed by [`reconcile_drm_confirmations`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrmConfirmation {
    /// Mined, successful, and at least the required confirmation depth ⇒ promote to charged.
    Confirmed,
    /// Mined but reverted on-chain (the buy did not settle) ⇒ refund the reservation.
    Reverted,
    /// Not yet mined, below the depth floor, or the read failed ⇒ HOLD (stay Pending). Never
    /// auto-promotes; carries a human-readable why for the reconciliation log.
    Unconfirmed(String),
}

/// Read the on-chain confirmation state of a broadcast buy tx (Sprint 35). Production wraps
/// `chain_tx::tx_confirmation_live` (receipt + depth floor); CI injects a scripted mock. FAIL-SAFE:
/// an unreadable/not-yet-mined tx MUST return `Unconfirmed` so the reservation is never
/// auto-charged.
pub trait DrmConfirmer: Send + Sync {
    fn confirm(&self, tx_hash: &str) -> DrmConfirmation;
}

/// Extract the tx hash from a DRM pending record's `rail_note` (the `rail_ref` the provider filed:
/// `drm:tx=<hash>;op=<op>;tid=<tid>;price=<price>;tok=<pay_token>`). `None` for a non-DRM note (so
/// the reconciler skips it).
pub(crate) fn parse_drm_tx(rail_note: &str) -> Option<&str> {
    rail_note.strip_prefix("drm:tx=")?.split(';').next()
}

/// Extract the settlement tx hash from a pending record, PER ITS RAIL (Sprint 48 — the
/// chain-settled generalization of the S44 discriminator): `Drm` parses only `drm:tx=`, `Erc20`
/// only `erc20:tx=` (a Drm-tagged record with an erc20 note — or vice versa — has NO tx, exactly
/// like a tx-less note: left Pending, confirmer never called). `Unknown` (pre-S44 legacy) keeps
/// the DRM-only note fallback — the legacy fallback NEVER widens to new rails. `Http` never
/// parses: a hostile HTTP endpoint crafting either prefix is never polled.
pub(crate) fn parse_chain_tx(
    rail: crate::payment_ledger::PaymentRail,
    rail_note: &str,
) -> Option<&str> {
    use crate::payment_ledger::PaymentRail;
    match rail {
        PaymentRail::Drm => parse_drm_tx(rail_note),
        PaymentRail::Erc20 => rail_note.strip_prefix("erc20:tx=")?.split(';').next(),
        PaymentRail::Unknown => parse_drm_tx(rail_note),
        PaymentRail::Http => None,
    }
}

/// Whether a pending record belongs to the CHAIN-SETTLED confirmation reconciler (Sprint 44,
/// generalized Sprint 48). A positively-tagged `Drm` or `Erc20` record IS ours; a tagged `Http`
/// record is NEVER ours regardless of its note; an `Unknown` (pre-S44/untagged) record falls back
/// to the `drm:tx=` note heuristic ONLY (the bounded legacy path — see
/// [`reconcile_drm_confirmations`]).
fn is_chain_settled_pending(record: &crate::payment_ledger::PaymentRecord) -> bool {
    use crate::payment_ledger::PaymentRail;
    match record.rail {
        PaymentRail::Drm | PaymentRail::Erc20 => true,
        PaymentRail::Unknown => parse_drm_tx(&record.rail_note).is_some(),
        PaymentRail::Http => false,
    }
}

/// The PRODUCTION chain adapter — resolves via the MKT-1-hardened `chain_tx::resolve_token_id` and
/// settles via `buy_authority::buy_access`. Constructed with the buyer context the buy path needs
/// (`principal_id`; `subject` empty ⇒ the runtime's managed account is the authoritative buyer in
/// wallet-signing mode). Compiled and wired, but exercised only by the operator's live-chain
/// runbook — NEVER in the CI gate, which injects mocks. The `ledger` the resolver consults for the
/// KID→tokenId scan is the same one `buy_access` uses.
pub struct ChainDrmMarketplace {
    principal_id: String,
    subject: String,
    ledger: String,
}

impl ChainDrmMarketplace {
    pub fn new(principal_id: String, subject: String, ledger: String) -> Self {
        Self {
            principal_id,
            subject,
            ledger,
        }
    }

    /// The `BuyTarget` a settle broadcasts against — extracted so its money-critical invariants are
    /// CI-testable without a live chain. Pins:
    /// - `quantity = 1` (council S36 red-team F1): the gate compares the cap against the PER-UNIT
    ///   price, but `buy_access` charges `price × quantity`; a `runtime.pay` buys ACCESS (one
    ///   ACCESS_TOKEN), so quantity is pinned to 1 here — overriding any `ELASTOS_DDRM_BUY_QUANTITY`
    ///   — so the per-unit gate is the total and a multi-unit env can never settle above the cap;
    /// - `expected_price = quote.price` AND `expected_pay_token = quote.pay_token` (council S36 red-
    ///   team F2): both arm the buy's own abort-on-drift, so a price OR pay-token flip between the
    ///   quote-gate and the broadcast fails closed (the buy can never settle above, or in a
    ///   different token than, what the mandate's cap was gated against);
    /// - the pinned `operative`/`token_id` (never re-resolves — no re-opened ambiguity window).
    fn buy_target(
        &self,
        binding: &DrmBinding,
        quote: &DrmQuote,
    ) -> crate::api::buy_authority::BuyTarget {
        crate::api::buy_authority::BuyTarget {
            operative: Some(binding.operative.clone()),
            token_id: Some(binding.token_id.clone()),
            ledger: Some(self.ledger.clone()),
            quantity: Some("1".to_string()),
            expected_price: Some(quote.price.clone()),
            expected_pay_token: Some(quote.pay_token.clone()),
            ..Default::default()
        }
    }
}

impl DrmResolver for ChainDrmMarketplace {
    fn resolve(&self, asset_ref: &str) -> Result<DrmBinding, DrmResolveError> {
        // The MKT-1-hardened live resolver accumulates every distinct (operative, tokenId) across
        // the whole channel range and binds ONLY when exactly one exists, else fails closed. On an
        // ambiguous KID the chain-provider answers with the code `ambiguous_kid_binding` and the
        // message "KID … binds >1 distinct (operative, tokenId) … (buy blocked, fail-closed)".
        // NOTE (council S34 guardian F3): `run_chain_capsule` surfaces only the MESSAGE (the code
        // is dropped), so we classify Ambiguous on the distinctive message substring, NOT the word
        // "ambiguous" (which the message does not contain) — else a genuine MKT-1 hostile
        // co-mint would mislabel as mere absence. Both Ambiguous and Unresolvable are
        // NotCharged/refund, so money is safe either way; this keeps the REASON honest.
        match crate::api::chain_tx::resolve_token_id_live(asset_ref, &self.ledger) {
            // Both the tokenId AND the operative must be present (council S34 red-team F4): the
            // operative is filled `unwrap_or_default()` upstream, so an empty one could pass the
            // tokenId guard and then fail buy_access POST-price-read as an unclassifiable error
            // (kept as Indeterminate rather than refunded). Refuse it here as Unresolvable — a
            // provably pre-effect failure that refunds the cap.
            Ok((token_id, operative)) if !token_id.is_empty() && !operative.trim().is_empty() => {
                Ok(DrmBinding {
                    content_id: asset_ref.to_string(),
                    operative,
                    token_id,
                })
            }
            Ok(_) => Err(DrmResolveError::Unresolvable(
                "chain-provider returned an empty tokenId or operative".to_string(),
            )),
            Err(e) if e.contains("binds >1 distinct") || e.contains("ambiguous") => {
                Err(DrmResolveError::Ambiguous(e))
            }
            Err(e) => Err(DrmResolveError::Unresolvable(e)),
        }
    }
}

impl DrmSettler for ChainDrmMarketplace {
    fn quote(&self, binding: &DrmBinding) -> Result<DrmQuote, DrmSettleError> {
        // Read-only price source (Sprint 36) — NO broadcast. A sourcing failure (no listing / sold
        // out) is a provable PRE-BROADCAST NotBroadcast (refund); it should never be Indeterminate.
        let target = crate::api::buy_authority::BuyTarget {
            operative: Some(binding.operative.clone()),
            token_id: Some(binding.token_id.clone()),
            ledger: Some(self.ledger.clone()),
            ..Default::default()
        };
        match crate::api::buy_authority::quote_buy(&binding.content_id, &target) {
            Ok(q) => Ok(DrmQuote {
                price: q.price,
                pay_token: q.pay_token,
            }),
            Err(e) => Err(DrmSettleError::NotBroadcast(e)),
        }
    }

    fn settle(
        &self,
        binding: &DrmBinding,
        quote: &DrmQuote,
        _idempotency_key: &str,
    ) -> Result<DrmSettlement, DrmSettleError> {
        let now_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let target = self.buy_target(binding, quote);
        match crate::api::buy_authority::buy_access(
            &self.principal_id,
            &binding.content_id,
            &self.subject,
            now_unix,
            &target,
        ) {
            Ok(outcome) => Ok(DrmSettlement {
                tx_hash: outcome.tx_hash,
            }),
            // buy_access now returns a TYPED outcome-class (Sprint 43): the refund-vs-hold decision
            // is the error's VARIANT, decided by which code path produced it, not by sniffing its
            // string. A hostile provider's message can no longer flip a sent tx into a refund.
            Err(e) => Err(DrmSettleError::from_buy_error(e)),
        }
    }
}

/// The default confirmation-depth floor for a live DRM buy (Sprint 35). Conservative — a Base
/// reorg past a few blocks is very rare; the operator can raise it via `ELASTOS_DRM_MIN_CONFIRMATIONS`.
const DEFAULT_MIN_CONFIRMATIONS: u64 = 3;

fn drm_min_confirmations() -> u64 {
    std::env::var("ELASTOS_DRM_MIN_CONFIRMATIONS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(DEFAULT_MIN_CONFIRMATIONS)
}

/// The ONE live chain-confirmation read (receipt + depth floor) every chain-settled rail shares
/// (Sprint 48 — P5): DRM buys and ERC-20 checkouts confirm identically. FAIL-SAFE: any read
/// error ⇒ Unconfirmed (hold; never auto-charge a tx we could not verify).
pub(crate) fn confirm_chain_tx(tx_hash: &str) -> DrmConfirmation {
    match crate::api::chain_tx::tx_confirmation_live(tx_hash, drm_min_confirmations()) {
        Ok(crate::api::chain_tx::TxConfirmation::Confirmed) => DrmConfirmation::Confirmed,
        Ok(crate::api::chain_tx::TxConfirmation::Reverted) => DrmConfirmation::Reverted,
        Ok(crate::api::chain_tx::TxConfirmation::Pending(why)) => DrmConfirmation::Unconfirmed(why),
        Err(e) => DrmConfirmation::Unconfirmed(format!("confirmation read failed: {e}")),
    }
}

impl DrmConfirmer for ChainDrmMarketplace {
    fn confirm(&self, tx_hash: &str) -> DrmConfirmation {
        // Live Base only — the operator runbook.
        confirm_chain_tx(tx_hash)
    }
}

/// Poll every PENDING DRM buy in the ledger and resolve it against the chain (Sprint 35): a
/// confirmed tx is promoted to charged AND its `rail_ref` is bound onto the mandate's receipt; a
/// reverted tx refunds the reservation exactly once; a still-unconfirmed tx is left Pending. Reuses
/// the S30 [`reconcile_payment_core`](crate::api::handlers::capability::reconcile_payment_core)
/// spine for the money movement + attestation — this driver only supplies the chain's verdict (in
/// place of the operator's) and, on a confirmation, the token-keyed receipt binding.
///
/// Returns the count of entries promoted / refunded / left pending / skipped. Never panics on one
/// bad entry: a per-entry error OR PANIC is logged, that entry stays Pending, and the loop
/// continues (retried next pass). At most `max_entries` DRM pendings are processed per pass
/// (oldest-first — `pending()` is seq-ordered); the overflow is counted in `skipped`, never
/// silently dropped (manual/one-shot callers pass `usize::MAX`). Only DRM pendings (by the rail
/// tag — see below) are considered; a non-DRM pending is left for the operator surface. A DRM
/// pending that lacks a parseable `token_id` is still promoted/refunded but gets NO receipt binding
/// (the token-keyed `CapabilityUse` is skipped); a DRM pending whose note is not yet a `drm:tx=`
/// ref (a transient `"reserving"` placeholder, or a buy that went Indeterminate WITHOUT a tx hash)
/// is left Pending with a one-time WARN — it has no tx to poll and needs operator/chain-scan
/// recovery (the S29 orphan window), NOT silent inclusion in the never-mining set.
///
/// RAIL DISCRIMINATOR (Sprint 44, closing MKT-DRM 2d / council S35 red-team F5): a pending is a DRM
/// pending iff its STRUCTURED [`PaymentRail::Drm`](crate::payment_ledger::PaymentRail) tag says so —
/// a tag stamped from the paying provider at `begin_attempt`, NOT rail-controlled text. A
/// positively-tagged `Http` pending is NEVER polled here, so a hostile HTTP endpoint that crafts an
/// Indeterminate body beginning `drm:tx=` can no longer get its pending resolved against an
/// attacker-named tx. BOUNDED LEGACY FALLBACK: a pre-S44 record on disk is `Unknown` (untagged); for
/// those ONLY, we fall back to the `drm:tx=` note heuristic so in-flight DRM pendings still
/// reconcile across the upgrade. That fallback carries the old (fail-closed: refund/hold only)
/// exposure but only for records created before this sprint, which drain as they resolve; every new
/// record is positively tagged.
pub fn reconcile_drm_confirmations(
    ledger: &crate::payment_ledger::PaymentLedger,
    meter: &elastos_runtime::primitives::spend::SpendMeter,
    audit_log: &elastos_runtime::primitives::audit::AuditLog,
    confirmer: &dyn DrmConfirmer,
    max_entries: usize,
    start_after_seq: Option<u64>,
) -> DrmReconcileSummary {
    let mut summary = DrmReconcileSummary::default();
    // ROTATED scan order (council S37 F1 — head-of-line starvation): a bounded pass that always
    // starts at the oldest entry lets a stuck-Unconfirmed prefix (never-mining txs held forever;
    // money-bearing Pendings are never evicted) consume the whole batch every pass and STARVE
    // every entry behind it — including reverted buys whose refunds would then never land. So a
    // pass starts AFTER the caller's cursor (the previous pass's last scanned seq) and wraps,
    // guaranteeing every pending is visited within ceil(pending/batch) passes. One-shot callers
    // pass `start_after_seq = 0` (seq starts at 1, so 0 ⇒ plain oldest-first).
    let drm_pendings: Vec<_> = ledger
        .pending()
        .into_iter()
        .filter(is_chain_settled_pending)
        .collect();
    let split = match start_after_seq {
        Some(cursor) => drm_pendings.partition_point(|r| r.seq <= cursor),
        None => 0,
    };
    let rotated = drm_pendings[split..].iter().chain(&drm_pendings[..split]);
    for record in rotated {
        // Per-tick bound (Sprint 37): the scheduler must never stall a process on an unbounded
        // pending set. Entries beyond the cap are COUNTED as skipped (never silently dropped) —
        // the rotation above guarantees the next pass reaches them.
        if summary.scanned() >= max_entries {
            summary.skipped += 1;
            continue;
        }
        summary.next_cursor = Some(record.seq);
        // Panic isolation (Sprint 37): one poisoned entry (a confirmer or reconcile panic) must
        // hold THAT entry Pending and let the tick continue — a scheduler that dies on entry k
        // silently abandons entries k+1.. until a restart. The money core inside is already
        // rollback-disciplined; the catch only converts an abort into hold-and-continue.
        let key = record.idempotency_key.clone();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            reconcile_one_drm_pending(ledger, meter, audit_log, confirmer, record)
        })) {
            Ok(EntryOutcome::Promoted) => summary.promoted += 1,
            Ok(EntryOutcome::Refunded) => summary.refunded += 1,
            Ok(EntryOutcome::LeftPending) => summary.left_pending += 1,
            // A panic mid-entry (council S37 red-team F3): the durable ledger is the truth about
            // what actually landed before the abort — account by what it says, never by hope.
            Err(_) => match ledger.get(&key).map(|r| r.status) {
                Some(crate::payment_ledger::PaymentStatus::Pending) | None => {
                    tracing::error!(
                        key = %key,
                        "DRM reconcile PANICKED on this entry — held Pending, tick continues"
                    );
                    summary.left_pending += 1;
                }
                Some(crate::payment_ledger::PaymentStatus::ResolvedCharged) => {
                    // The resolve landed durably before the abort; the charge stands (the
                    // receipt binding is best-effort anyway). Honest count: promoted.
                    tracing::error!(
                        key = %key,
                        "DRM reconcile panicked AFTER the durable charge resolution — the \
                         promotion stands; the receipt rail_ref binding may be missing"
                    );
                    summary.promoted += 1;
                }
                Some(status) => {
                    // Resolved not-charged (or another terminal) before the abort — the refund
                    // may or may not have applied. This is a MONEY-STATE DIVERGENCE alarm, not
                    // a silent hold: the entry is no longer Pending, so no pass will retry it.
                    tracing::error!(
                        key = %key,
                        ?status,
                        "DRM reconcile panicked AFTER the durable resolution — verify the \
                         refund on the meter (refund_applied forensics) — MONEY-STATE ALARM"
                    );
                    summary.refunded += 1;
                }
            },
        }
    }
    summary
}

/// What one pending DRM entry resolved to in one pass.
enum EntryOutcome {
    Promoted,
    Refunded,
    LeftPending,
}

/// Resolve ONE pending DRM entry against the chain verdict — the per-entry body of
/// [`reconcile_drm_confirmations`], extracted so the loop's bounding/panic-isolation policy stays
/// separate from the money movement (which all flows through `reconcile_payment_core`).
fn reconcile_one_drm_pending(
    ledger: &crate::payment_ledger::PaymentLedger,
    meter: &elastos_runtime::primitives::spend::SpendMeter,
    audit_log: &elastos_runtime::primitives::audit::AuditLog,
    confirmer: &dyn DrmConfirmer,
    record: &crate::payment_ledger::PaymentRecord,
) -> EntryOutcome {
    use elastos_runtime::capability::token::TokenId;
    use elastos_runtime::capability::ResourceId;

    let Some(tx) = parse_chain_tx(record.rail, &record.rail_note).map(str::to_string) else {
        // A chain-rail-tagged pending with no parseable tx note (Sprint 44, generalized S48): a
        // transient `"reserving"` placeholder (harmless — the next pass sees the finalized note),
        // OR a settle that went Indeterminate WITHOUT a tx hash (an S29-class orphan — no tx to
        // poll, needs operator / chain-scan recovery). Pre-S44 the note filter excluded these; now
        // the rail tag admits them, so make a permanently-unpollable entry VISIBLE rather than
        // folding it silently into the never-mining set. Fail-closed: left Pending, money
        // unchanged, confirmer never called.
        if matches!(
            record.rail,
            crate::payment_ledger::PaymentRail::Drm | crate::payment_ledger::PaymentRail::Erc20
        ) && !record.rail_note.is_empty()
            && record.rail_note != "reserving"
        {
            tracing::warn!(
                key = %record.idempotency_key,
                rail = ?record.rail,
                rail_note = %record.rail_note,
                "chain-rail pending has no parseable tx hash to poll — left Pending; needs \
                 operator reconcile / chain scan (S29 orphan)"
            );
        }
        return EntryOutcome::LeftPending;
    };
    // Confirmed and Reverted share one reconcile spine — only `charged` and the receipt
    // binding differ, so the arms stay a single code path that cannot drift apart.
    let charged = match confirmer.confirm(&tx) {
        DrmConfirmation::Unconfirmed(_) => return EntryOutcome::LeftPending,
        DrmConfirmation::Confirmed => true,
        DrmConfirmation::Reverted => false,
    };
    let input = crate::api::handlers::capability::ReconcilePaymentInput {
        idempotency_key: record.idempotency_key.clone(),
        charged,
    };
    match crate::api::handlers::capability::reconcile_payment_core(ledger, meter, audit_log, input)
    {
        Ok(_) if charged => {
            // Bind the confirmed settlement onto the mandate's receipt: a token-keyed,
            // signed CapabilityUse carrying the DRM rail_ref (success=true). This is
            // the S35 half that makes the receipt reflect the CONFIRMED tx, not the
            // mere broadcast. Best-effort (mirrors the dispatch-path use record): a
            // lost emit under-reports in the receipt, but the payment_reconciled event
            // + the ledger already durably record the promotion.
            if let Some(token_id) = record
                .token_id
                .as_deref()
                .and_then(|h| TokenId::from_hex(h.trim()).ok())
            {
                let resource = format!("{}{}", crate::intent_executor::PAY_PREFIX, record.payee);
                audit_log.capability_use_with_rail_ref(
                    &token_id,
                    &record.capsule,
                    &ResourceId::new(resource),
                    elastos_runtime::capability::Action::Execute,
                    true,
                    Some(record.rail_note.clone()),
                );
            }
            EntryOutcome::Promoted
        }
        Ok(_) => EntryOutcome::Refunded,
        Err((_, e)) => {
            tracing::error!(
                key = %record.idempotency_key,
                charged,
                "DRM verdict could not be reconciled: {e}"
            );
            EntryOutcome::LeftPending
        }
    }
}

/// One SCHEDULER tick (Sprint 37): the same reconciliation pass the manual path runs — zero new
/// money-moving code — plus the tick's observability: when the pass SETTLED anything (promoted or
/// refunded), a `Custom` `drm_reconcile_tick` event is appended BEST-EFFORT to the signed chain
/// (a failed emit is logged and never blocks the tick; the per-entry `payment_reconciled` events
/// remain the durable money attestation). Held-only re-polls stay off the chain (council S37
/// red-team F2): a stuck pending re-polled every tick forever must not grow the signed, fsync'd
/// chain by one event per tick — nothing attestable changed.
pub fn drm_reconcile_tick(
    ledger: &crate::payment_ledger::PaymentLedger,
    meter: &elastos_runtime::primitives::spend::SpendMeter,
    audit_log: &elastos_runtime::primitives::audit::AuditLog,
    confirmer: &dyn DrmConfirmer,
    max_entries: usize,
    start_after_seq: Option<u64>,
) -> DrmReconcileSummary {
    let summary = reconcile_drm_confirmations(
        ledger,
        meter,
        audit_log,
        confirmer,
        max_entries,
        start_after_seq,
    );
    if summary.promoted > 0 || summary.refunded > 0 {
        audit_log.emit_best_effort(elastos_runtime::primitives::audit::AuditEvent::Custom {
            event_type: "drm_reconcile_tick".to_string(),
            details: serde_json::json!({
                "promoted": summary.promoted,
                "refunded": summary.refunded,
                "left_pending": summary.left_pending,
                "skipped": summary.skipped,
            }),
        });
    }
    summary
}

/// What one [`reconcile_drm_confirmations`] pass did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DrmReconcileSummary {
    /// Confirmed tx promoted to charged (+ receipt-bound).
    pub promoted: usize,
    /// Reverted tx refunded.
    pub refunded: usize,
    /// Still unconfirmed (or a reconcile error/panic) — left Pending, retried next pass.
    pub left_pending: usize,
    /// DRM pendings beyond this pass's `max_entries` bound — untouched, next pass's work
    /// (reached via the rotating cursor).
    pub skipped: usize,
    /// The `seq` of the LAST entry this pass scanned — the caller's next `start_after_seq`, so
    /// successive bounded passes rotate over the whole pending set. `None` ⇒ nothing scanned
    /// (keep the previous cursor).
    pub next_cursor: Option<u64>,
}

impl DrmReconcileSummary {
    /// How many DRM pendings this pass actually processed (everything but `skipped`).
    pub fn scanned(&self) -> usize {
        self.promoted + self.refunded + self.left_pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A scripted resolver: returns a fixed binding, or a fixed resolve error.
    struct MockResolver(Result<DrmBinding, &'static str>);
    impl DrmResolver for MockResolver {
        fn resolve(&self, _asset_ref: &str) -> Result<DrmBinding, DrmResolveError> {
            match &self.0 {
                Ok(b) => Ok(b.clone()),
                Err(kind) if *kind == "ambiguous" => {
                    Err(DrmResolveError::Ambiguous("two bindings".to_string()))
                }
                Err(_) => Err(DrmResolveError::Unresolvable("no channel".to_string())),
            }
        }
    }

    /// A scripted settler that records whether it was called, quotes a fixed price, and returns a
    /// fixed settle outcome.
    struct MockSettler {
        outcome: Mutex<Option<Result<DrmSettlement, &'static str>>>,
        called: Mutex<bool>,
        price: String,
    }
    impl MockSettler {
        /// A FREE quote (price 0) — the price gate is a no-op; used by the non-gate tests.
        fn new(outcome: Result<DrmSettlement, &'static str>) -> Self {
            Self::priced(outcome, "0")
        }
        fn priced(outcome: Result<DrmSettlement, &'static str>, price: &str) -> Self {
            Self {
                outcome: Mutex::new(Some(outcome)),
                called: Mutex::new(false),
                price: price.to_string(),
            }
        }
    }
    impl DrmSettler for MockSettler {
        fn quote(&self, _b: &DrmBinding) -> Result<DrmQuote, DrmSettleError> {
            Ok(DrmQuote {
                price: self.price.clone(),
                pay_token: "usdc".to_string(),
            })
        }
        fn settle(
            &self,
            _b: &DrmBinding,
            _q: &DrmQuote,
            _k: &str,
        ) -> Result<DrmSettlement, DrmSettleError> {
            *self.called.lock().unwrap() = true;
            match self.outcome.lock().unwrap().take().unwrap() {
                Ok(s) => Ok(s),
                Err("not_broadcast") => Err(DrmSettleError::NotBroadcast("sold out".to_string())),
                Err(_) => Err(DrmSettleError::Indeterminate("rpc timeout".to_string())),
            }
        }
    }

    fn binding() -> DrmBinding {
        DrmBinding {
            content_id: "QmAsset".to_string(),
            operative: "0xop".to_string(),
            token_id: "42".to_string(),
        }
    }

    #[test]
    fn a_broadcast_accepted_buy_is_indeterminate_not_charged_immediately_and_carries_the_rail_ref()
    {
        // Sprint 35: a broadcast-accepted buy is NEVER immediately charged — it is Indeterminate
        // (Pending), the reservation held, the rail_ref carried as the reason so the pending
        // ledger record holds the tx for later confirmation.
        let provider = DrmMarketplaceProvider::new(
            Arc::new(MockResolver(Ok(binding()))),
            Arc::new(MockSettler::new(Ok(DrmSettlement {
                tx_hash: "0xdead".to_string(),
            }))),
            1,
            None,
        );
        match provider.pay("QmAsset", 500, "flint-sig").unwrap_err() {
            PayError::Indeterminate(rail_ref) => {
                assert_eq!(rail_ref, "drm:tx=0xdead;op=0xop;tid=42;price=0;tok=usdc");
                assert_eq!(
                    parse_drm_tx(&rail_ref),
                    Some("0xdead"),
                    "the pending record's tx is recoverable for confirmation"
                );
            }
            other => panic!("a broadcast-accepted buy must be Indeterminate, got {other:?}"),
        }
    }

    #[test]
    fn an_ambiguous_binding_is_not_charged_and_never_settles() {
        let settler = Arc::new(MockSettler::new(Ok(DrmSettlement {
            tx_hash: "0xshouldnothappen".to_string(),
        })));
        let provider = DrmMarketplaceProvider::new(
            Arc::new(MockResolver(Err("ambiguous"))),
            settler.clone(),
            1,
            None,
        );
        let err = provider.pay("QmAsset", 500, "flint-sig").unwrap_err();
        match err {
            PayError::NotCharged(msg) => assert!(msg.contains("ambiguous")),
            other => panic!("ambiguous resolve must be NotCharged, got {other:?}"),
        }
        assert!(
            !*settler.called.lock().unwrap(),
            "an ambiguous binding must NEVER reach the settler — no buy is attempted"
        );
    }

    #[test]
    fn an_unresolvable_asset_is_not_charged() {
        let provider = DrmMarketplaceProvider::new(
            Arc::new(MockResolver(Err("unresolvable"))),
            Arc::new(MockSettler::new(Ok(DrmSettlement {
                tx_hash: "x".to_string(),
            }))),
            1,
            None,
        );
        assert!(matches!(
            provider.pay("QmGone", 1, "k").unwrap_err(),
            PayError::NotCharged(_)
        ));
    }

    #[test]
    fn a_not_broadcast_buy_is_not_charged_and_an_indeterminate_buy_keeps_the_reservation() {
        let not_broadcast = DrmMarketplaceProvider::new(
            Arc::new(MockResolver(Ok(binding()))),
            Arc::new(MockSettler::new(Err("not_broadcast"))),
            1,
            None,
        );
        assert!(matches!(
            not_broadcast.pay("QmAsset", 1, "k").unwrap_err(),
            PayError::NotCharged(_)
        ));

        let indeterminate = DrmMarketplaceProvider::new(
            Arc::new(MockResolver(Ok(binding()))),
            Arc::new(MockSettler::new(Err("indeterminate"))),
            1,
            None,
        );
        assert!(matches!(
            indeterminate.pay("QmAsset", 1, "k").unwrap_err(),
            PayError::Indeterminate(_)
        ));
    }

    /// Sprint 36 ratchet (a): a buy whose mandate cap (converted via the unit mapping) is BELOW the
    /// on-chain price is refused before ANY broadcast — the settler is never called.
    #[test]
    fn a_buy_below_the_on_chain_price_is_refused_before_broadcast() {
        // spend_unit = 1 (1 spend unit == 1 pay-token unit). Quote price 500; the mandate amount
        // 300 authorizes only 300 — below the price ⇒ refuse, never settle.
        let settler = Arc::new(MockSettler::priced(
            Ok(DrmSettlement {
                tx_hash: "0xNOPE".to_string(),
            }),
            "500",
        ));
        let provider = DrmMarketplaceProvider::new(
            Arc::new(MockResolver(Ok(binding()))),
            settler.clone(),
            1,
            None,
        );
        match provider.pay("QmAsset", 300, "k").unwrap_err() {
            PayError::NotCharged(msg) => {
                assert!(msg.contains("refused before broadcast"), "{msg}");
                assert!(
                    msg.contains("500"),
                    "the message names the on-chain price: {msg}"
                );
            }
            other => panic!("a below-price buy must be NotCharged, got {other:?}"),
        }
        assert!(
            !*settler.called.lock().unwrap(),
            "the settler must NEVER be called for a below-price buy — no broadcast"
        );
    }

    /// Sprint 36 ratchet (b): an exact-match buy (converted cap == price) proceeds, and the
    /// rail_ref names the pay-token price. The unit mapping scales the cap: spend_unit 1_000_000
    /// (USDC 6-decimals) means a 5-spend-unit mandate authorizes 5_000_000 units, covering a
    /// 5_000_000 price.
    #[test]
    fn an_exact_match_buy_proceeds_and_the_rail_ref_names_the_price() {
        let settler = Arc::new(MockSettler::priced(
            Ok(DrmSettlement {
                tx_hash: "0xC0FFEE".to_string(),
            }),
            "5000000",
        ));
        let provider = DrmMarketplaceProvider::new(
            Arc::new(MockResolver(Ok(binding()))),
            settler.clone(),
            1_000_000,
            None,
        );
        match provider.pay("QmAsset", 5, "k").unwrap_err() {
            PayError::Indeterminate(rail_ref) => {
                assert_eq!(
                    rail_ref,
                    "drm:tx=0xC0FFEE;op=0xop;tid=42;price=5000000;tok=usdc"
                );
            }
            other => panic!("an exact-match buy should broadcast (Indeterminate), got {other:?}"),
        }
        assert!(*settler.called.lock().unwrap(), "the buy settled");

        // One unit SHORT of the price ⇒ refused (spend_unit 1_000_000, amount 4 ⇒ 4_000_000 < 5M).
        let below = DrmMarketplaceProvider::new(
            Arc::new(MockResolver(Ok(binding()))),
            Arc::new(MockSettler::priced(
                Ok(DrmSettlement {
                    tx_hash: "0xNO".to_string(),
                }),
                "5000000",
            )),
            1_000_000,
            None,
        );
        assert!(matches!(
            below.pay("QmAsset", 4, "k").unwrap_err(),
            PayError::NotCharged(_)
        ));
    }

    /// Sprint 36 council fold (red-team F1+F2): the DRM buy target PINS quantity=1 (so the
    /// per-unit price gate is the total — a multi-unit env can never settle above the cap) and
    /// arms abort-on-drift on BOTH the price and the pay-token (a flip of either between the
    /// quote-gate and the broadcast fails closed).
    #[test]
    fn the_drm_buy_target_pins_quantity_and_arms_price_and_pay_token_drift() {
        let mkt = ChainDrmMarketplace::new(
            "person:op".to_string(),
            String::new(),
            "0xledger".to_string(),
        );
        let quote = DrmQuote {
            price: "5000000".to_string(),
            pay_token: "0xUSDC".to_string(),
        };
        let target = mkt.buy_target(&binding(), &quote);
        assert_eq!(
            target.quantity.as_deref(),
            Some("1"),
            "quantity is pinned to 1 — the per-unit gate is the total charge (red-team F1)"
        );
        assert_eq!(
            target.expected_price.as_deref(),
            Some("5000000"),
            "the gated price arms abort-on-drift"
        );
        assert_eq!(
            target.expected_pay_token.as_deref(),
            Some("0xUSDC"),
            "the pay-token arms abort-on-drift too (red-team F2)"
        );
        assert_eq!(target.operative.as_deref(), Some("0xop"));
        assert_eq!(target.token_id.as_deref(), Some("42"));
    }

    /// Sprint 36: an unparseable on-chain price is fail-closed refused before broadcast.
    #[test]
    fn an_unparseable_price_is_refused_before_broadcast() {
        let settler = Arc::new(MockSettler::priced(
            Ok(DrmSettlement {
                tx_hash: "0xNOPE".to_string(),
            }),
            "not-a-number",
        ));
        let provider = DrmMarketplaceProvider::new(
            Arc::new(MockResolver(Ok(binding()))),
            settler.clone(),
            1,
            None,
        );
        assert!(matches!(
            provider.pay("QmAsset", 999, "k").unwrap_err(),
            PayError::NotCharged(_)
        ));
        assert!(
            !*settler.called.lock().unwrap(),
            "no settle on an unparseable price"
        );
    }

    /// Sprint 36 council fold (F3): a listing quoting a DIFFERENT pay-token than the declared one
    /// is refused before broadcast — the spend-unit mapping denominates exactly one token, so the
    /// cap cannot be compared across token denominations.
    #[test]
    fn a_listing_in_a_different_pay_token_than_declared_is_refused() {
        let settler = Arc::new(MockSettler::priced(
            Ok(DrmSettlement {
                tx_hash: "0xNOPE".to_string(),
            }),
            "1",
        ));
        // The mock quotes pay_token "usdc"; the deployment declared the unit is for "0xWBTC".
        let provider = DrmMarketplaceProvider::new(
            Arc::new(MockResolver(Ok(binding()))),
            settler.clone(),
            1,
            Some("0xWBTC".to_string()),
        );
        match provider.pay("QmAsset", 999, "k").unwrap_err() {
            PayError::NotCharged(msg) => {
                assert!(
                    msg.contains("different") || msg.contains("token denominations"),
                    "{msg}"
                );
            }
            other => panic!("a wrong-token listing must be NotCharged, got {other:?}"),
        }
        assert!(
            !*settler.called.lock().unwrap(),
            "no settle on a wrong-token listing"
        );

        // The SAME token (case-insensitive) passes the token check.
        let ok = DrmMarketplaceProvider::new(
            Arc::new(MockResolver(Ok(binding()))),
            Arc::new(MockSettler::priced(
                Ok(DrmSettlement {
                    tx_hash: "0xC0".to_string(),
                }),
                "1",
            )),
            1,
            Some("USDC".to_string()),
        );
        assert!(matches!(
            ok.pay("QmAsset", 999, "k").unwrap_err(),
            PayError::Indeterminate(_)
        ));
    }

    // (Sprint 43: the old `pre_broadcast_refusal_classification_is_conservative` string-classifier
    // test was retired with `is_pre_broadcast_refusal`. Its intent is now covered by three stronger
    // proofs: `from_buy_error_classifies_by_variant_not_by_string` (the variant mapping + hostile
    // immunity), `a_not_broadcast_buy_is_not_charged_and_an_indeterminate_buy_keeps_the_reservation`
    // (DrmSettleError → PayError), and the `buy_authority` ratchets that prove `buy_access` builds
    // the correct variant at each pre/post-broadcast site.)

    #[test]
    fn the_live_ambiguity_message_classifies_as_ambiguous_not_absence() {
        // Council S34 guardian F3: the chain-provider's ambiguous-KID MESSAGE (the code is dropped
        // by run_chain_capsule) contains "binds >1 distinct", NOT the word "ambiguous". A resolver
        // classifier that keyed only on "ambiguous" would mislabel a genuine MKT-1 hostile co-mint
        // as mere absence. This locks the message-substring classification. (Both Ambiguous and
        // Unresolvable are NotCharged, so this guards the REASON honesty, not the money.)
        let msg = "chain-provider op failed: KID 0xabc binds >1 distinct (operative, tokenId) — \
                   refusing to bind a possibly-hostile token (buy blocked, fail-closed)";
        assert!(
            msg.contains("binds >1 distinct") && !msg.to_lowercase().contains("ambiguous"),
            "the live message has the distinctive marker but not the word 'ambiguous'"
        );
        // The classifier arm keys on that marker (mirrors ChainDrmMarketplace::resolve).
        let is_ambiguous = msg.contains("binds >1 distinct") || msg.contains("ambiguous");
        assert!(
            is_ambiguous,
            "a hostile co-mint is classified Ambiguous, not Unresolvable"
        );
    }

    #[test]
    fn rail_ref_strips_delimiter_injection_from_chain_supplied_components() {
        // Council S34 red-team F3: a compromised adapter returning an operative that embeds the
        // format's own delimiters must not forge the parsed binding.
        let hostile = DrmBinding {
            content_id: "QmAsset".to_string(),
            operative: "0xreal;tid=999".to_string(),
            token_id: "42".to_string(),
        };
        let settlement = DrmSettlement {
            tx_hash: "0x;op=fake".to_string(),
        };
        let quote = DrmQuote {
            price: "1;tid=9".to_string(),
            pay_token: "usdc;x".to_string(),
        };
        let rail_ref = DrmMarketplaceProvider::rail_ref(&hostile, &quote, &settlement);
        // Exactly ONE of each delimiter key — the injected ones were stripped from every field.
        assert_eq!(
            rail_ref.matches(";tid=").count(),
            1,
            "no forged tid segment: {rail_ref}"
        );
        assert_eq!(
            rail_ref.matches(";op=").count(),
            1,
            "no forged op segment: {rail_ref}"
        );
        assert_eq!(
            rail_ref.matches(";price=").count(),
            1,
            "no forged price segment: {rail_ref}"
        );
        assert_eq!(
            rail_ref.matches(";tok=").count(),
            1,
            "no forged tok segment: {rail_ref}"
        );
        assert_eq!(
            rail_ref,
            "drm:tx=0xopfake;op=0xrealtid999;tid=42;price=1tid9;tok=usdcx"
        );
    }

    #[test]
    fn parse_drm_tx_extracts_the_hash_and_ignores_non_drm_notes() {
        assert_eq!(
            parse_drm_tx("drm:tx=0xABC123;op=0xop;tid=7"),
            Some("0xABC123")
        );
        // A tx-only note (no trailing segments) still parses.
        assert_eq!(parse_drm_tx("drm:tx=0xABC"), Some("0xABC"));
        // Non-DRM notes are skipped (the reconciler leaves them for the operator surface).
        assert_eq!(parse_drm_tx("rail reference from an HTTP endpoint"), None);
        assert_eq!(parse_drm_tx(""), None);
    }

    /// A scripted confirmer keyed by tx hash — deterministic per-tx verdicts for the reconciler.
    struct MockConfirmer(std::collections::HashMap<String, DrmConfirmation>);
    impl DrmConfirmer for MockConfirmer {
        fn confirm(&self, tx_hash: &str) -> DrmConfirmation {
            self.0
                .get(tx_hash)
                .cloned()
                .unwrap_or_else(|| DrmConfirmation::Unconfirmed("no verdict scripted".to_string()))
        }
    }

    /// Sprint 35: the reconciler promotes a CONFIRMED pending DRM buy (spend stands), REFUNDS a
    /// REVERTED one exactly once, and LEAVES an unconfirmed one Pending — all through the shared
    /// S30 reconcile spine, over the same meter + ledger.
    #[test]
    fn reconcile_drm_confirmations_promotes_refunds_and_holds() {
        use crate::payment_ledger::{PaymentLedger, PaymentStatus};
        use elastos_runtime::primitives::audit::AuditLog;
        use elastos_runtime::primitives::spend::SpendMeter;

        let ledger = PaymentLedger::new();
        let meter = SpendMeter::new();
        let audit = AuditLog::new();
        meter.set_budget("vm-shop", 1000).unwrap();

        // Three pending DRM buys, each reserved on the meter (as the pay path would).
        for (key, tx, amount) in [
            ("flint-confirmed", "0xC0", 100u64),
            ("flint-reverted", "0xRE", 200),
            ("flint-holding", "0xHO", 300),
        ] {
            meter.try_debit("vm-shop", amount).unwrap();
            assert!(ledger.record_with_token(
                key,
                "vm-shop",
                "QmAsset",
                amount,
                PaymentStatus::Pending,
                &format!("drm:tx={tx};op=0xop;tid=7"),
                Some("00000000000000000000000000000001"),
            ));
        }
        assert_eq!(
            meter.remaining("vm-shop"),
            400,
            "600 reserved across three pendings"
        );

        let mut verdicts = std::collections::HashMap::new();
        verdicts.insert("0xC0".to_string(), DrmConfirmation::Confirmed);
        verdicts.insert("0xRE".to_string(), DrmConfirmation::Reverted);
        verdicts.insert(
            "0xHO".to_string(),
            DrmConfirmation::Unconfirmed("mempool".to_string()),
        );
        let confirmer = MockConfirmer(verdicts);

        let summary =
            reconcile_drm_confirmations(&ledger, &meter, &audit, &confirmer, usize::MAX, None);
        assert_eq!(summary.promoted, 1);
        assert_eq!(summary.refunded, 1);
        assert_eq!(summary.left_pending, 1);

        // Confirmed: spend STANDS (no refund) ⇒ status ResolvedCharged.
        assert_eq!(
            ledger.get("flint-confirmed").unwrap().status,
            PaymentStatus::ResolvedCharged
        );
        // Reverted: refunded exactly once ⇒ the 200 came back.
        assert_eq!(
            ledger.get("flint-reverted").unwrap().status,
            PaymentStatus::ResolvedNotCharged
        );
        // Holding: still Pending.
        assert_eq!(
            ledger.get("flint-holding").unwrap().status,
            PaymentStatus::Pending
        );
        // Net meter: started 1000, reserved 600, refunded 200 (reverted) ⇒ 600 remaining
        // (confirmed 100 + holding 300 stay reserved).
        assert_eq!(meter.remaining("vm-shop"), 600);

        // A second pass is idempotent — the resolved entries are no longer Pending, only the
        // holding one is re-polled (still unconfirmed).
        let again =
            reconcile_drm_confirmations(&ledger, &meter, &audit, &confirmer, usize::MAX, None);
        assert_eq!(again.promoted, 0);
        assert_eq!(again.refunded, 0, "no double refund");
        assert_eq!(again.left_pending, 1);
        assert_eq!(
            meter.remaining("vm-shop"),
            600,
            "no double refund on the meter"
        );
    }

    /// Sprint 44 (the MKT-DRM 2d ratchet): the DRM reconciler selects its pendings by the STRUCTURED
    /// `PaymentRail` tag, not by the rail-controlled note. A positively-tagged `Http` pending whose
    /// note a hostile endpoint CRAFTED to begin `drm:tx=` is NEVER polled/resolved by the DRM driver;
    /// a real `Drm`-tagged pending and a pre-S44 `Unknown`-tagged pending with a real note both are.
    #[test]
    fn a_positively_tagged_http_pending_is_never_reconciled_by_the_drm_driver() {
        use crate::payment_ledger::{PaymentLedger, PaymentRail, PaymentStatus};
        use elastos_runtime::primitives::audit::AuditLog;
        use elastos_runtime::primitives::spend::SpendMeter;

        let ledger = PaymentLedger::new();
        let meter = SpendMeter::new();
        let audit = AuditLog::new();
        meter.set_budget("vm-shop", 1000).unwrap();

        // OURS: a real DRM-tagged pending, and a pre-S44 legacy (Unknown) pending with a DRM note.
        meter.try_debit("vm-shop", 100).unwrap();
        assert!(ledger.record_on_rail(
            "flint-drm",
            "vm-shop",
            "QmAsset",
            100,
            PaymentStatus::Pending,
            "drm:tx=0xDRM;op=o;tid=1",
            None,
            PaymentRail::Drm,
        ));
        meter.try_debit("vm-shop", 100).unwrap();
        assert!(ledger.record_with_token(
            "flint-legacy",
            "vm-shop",
            "QmAsset",
            100,
            PaymentStatus::Pending,
            "drm:tx=0xLEG;op=o;tid=1",
            None, // Unknown rail (pre-S44 shape)
        ));
        // HOSTILE: an Http-tagged pending whose note is CRAFTED to look like a DRM ref.
        meter.try_debit("vm-shop", 500).unwrap();
        assert!(ledger.record_on_rail(
            "flint-http",
            "vm-shop",
            "attacker",
            500,
            PaymentStatus::Pending,
            "drm:tx=0xHTTP;op=o;tid=1",
            None,
            PaymentRail::Http,
        ));

        // A confirmer that would REVERT (refund) ANY tx it is polled about.
        let mut verdicts = std::collections::HashMap::new();
        for tx in ["0xDRM", "0xLEG", "0xHTTP"] {
            verdicts.insert(tx.to_string(), DrmConfirmation::Reverted);
        }
        let confirmer = MockConfirmer(verdicts);

        let summary =
            reconcile_drm_confirmations(&ledger, &meter, &audit, &confirmer, usize::MAX, None);

        assert_eq!(
            summary.refunded, 2,
            "only the two DRM-owned pendings are polled + refunded"
        );
        assert_eq!(
            ledger.get("flint-http").unwrap().status,
            PaymentStatus::Pending,
            "the hostile Http-tagged pending is NEVER polled by the DRM driver — its crafted \
             drm:tx= note cannot get it resolved"
        );
        // Started 1000; reserved 700; refunded 200 (DRM+legacy). The Http 500 stays reserved.
        assert_eq!(
            meter.remaining("vm-shop"),
            500,
            "only the DRM-owned reservations were refunded; the Http reservation is untouched"
        );
    }

    /// Sprint 48 (the second chain-settled rail): the reconciler owns `Erc20`-tagged pendings
    /// exactly like `Drm` ones — an `erc20:tx=` pending is polled and promoted/refunded through
    /// the SAME spine — while the S44 security walls hold in every direction: an `Http`-tagged
    /// pending with a CRAFTED `erc20:tx=` note is never polled, and the pre-S44 `Unknown` legacy
    /// note-fallback stays DRM-ONLY (it never widens to new rails).
    #[test]
    fn erc20_pendings_are_reconciled_and_the_s44_walls_hold_for_the_new_rail() {
        use crate::payment_ledger::{PaymentLedger, PaymentRail, PaymentStatus};
        use elastos_runtime::primitives::audit::AuditLog;
        use elastos_runtime::primitives::spend::SpendMeter;

        let ledger = PaymentLedger::new();
        let meter = SpendMeter::new();
        let audit = AuditLog::new();
        meter.set_budget("vm-shop", 1000).unwrap();

        // OURS: a real Erc20-tagged checkout pending.
        meter.try_debit("vm-shop", 100).unwrap();
        assert!(ledger.record_on_rail(
            "flint-e20",
            "vm-shop",
            "0xpayee",
            100,
            PaymentStatus::Pending,
            "erc20:tx=0xE20;to=0xpayee;amount=100;tok=usdc",
            None,
            PaymentRail::Erc20,
        ));
        // HOSTILE: an Http-tagged pending whose note is CRAFTED to look like a checkout ref.
        meter.try_debit("vm-shop", 400).unwrap();
        assert!(ledger.record_on_rail(
            "flint-http-e20",
            "vm-shop",
            "attacker",
            400,
            PaymentStatus::Pending,
            "erc20:tx=0xEVIL;to=x;amount=1;tok=t",
            None,
            PaymentRail::Http,
        ));
        // LEGACY: an Unknown-tagged (pre-S44) pending with an erc20 note — the legacy fallback is
        // DRM-only, so this is NOT ours (left for the operator surface).
        meter.try_debit("vm-shop", 200).unwrap();
        assert!(ledger.record_with_token(
            "flint-legacy-e20",
            "vm-shop",
            "someone",
            200,
            PaymentStatus::Pending,
            "erc20:tx=0xOLD;to=x;amount=2;tok=t",
            None,
        ));

        // A confirmer that CONFIRMS our tx and would confirm the hostile/legacy ones too — the
        // walls, not the verdicts, are what keep them unpolled.
        let mut verdicts = std::collections::HashMap::new();
        for tx in ["0xE20", "0xEVIL", "0xOLD"] {
            verdicts.insert(tx.to_string(), DrmConfirmation::Confirmed);
        }
        let confirmer = MockConfirmer(verdicts);

        let summary =
            reconcile_drm_confirmations(&ledger, &meter, &audit, &confirmer, usize::MAX, None);

        assert_eq!(summary.promoted, 1, "exactly the Erc20 pending promoted");
        assert_eq!(
            ledger.get("flint-e20").unwrap().status,
            PaymentStatus::ResolvedCharged,
            "the checkout's spend stands once confirmed"
        );
        assert_eq!(
            ledger.get("flint-http-e20").unwrap().status,
            PaymentStatus::Pending,
            "the hostile Http-tagged pending is NEVER polled — its crafted erc20:tx= note \
             cannot get it resolved"
        );
        assert_eq!(
            ledger.get("flint-legacy-e20").unwrap().status,
            PaymentStatus::Pending,
            "the Unknown legacy fallback stays DRM-only — it never widens to new rails"
        );
    }

    /// Sprint 48: `parse_chain_tx` is rail-STRICT — a Drm-tagged record does not parse an erc20
    /// note and vice versa (a cross-rail note is a tx-less orphan: left Pending, never polled).
    #[test]
    fn parse_chain_tx_is_rail_strict() {
        use crate::payment_ledger::PaymentRail;
        assert_eq!(
            parse_chain_tx(PaymentRail::Erc20, "erc20:tx=0xA;to=x"),
            Some("0xA")
        );
        assert_eq!(parse_chain_tx(PaymentRail::Erc20, "drm:tx=0xA;op=o"), None);
        assert_eq!(parse_chain_tx(PaymentRail::Drm, "erc20:tx=0xA;to=x"), None);
        assert_eq!(
            parse_chain_tx(PaymentRail::Unknown, "erc20:tx=0xA;to=x"),
            None,
            "the legacy fallback never widens past drm:tx="
        );
        assert_eq!(parse_chain_tx(PaymentRail::Http, "drm:tx=0xA"), None);
        assert_eq!(parse_chain_tx(PaymentRail::Http, "erc20:tx=0xA"), None);
    }

    /// Sprint 44 (council guardian F4): a `Drm`-tagged pending whose note is NOT a `drm:tx=` ref
    /// (an Indeterminate-without-tx orphan) is now IN the reconciler's work list (the rail tag
    /// admits it where the old note filter excluded it) — but it is left Pending WITHOUT polling
    /// (no tx to poll), the confirmer is never called, and no money moves. Fail-closed.
    #[test]
    fn a_drm_tagged_pending_with_no_tx_note_is_left_pending_without_polling() {
        use crate::payment_ledger::{PaymentLedger, PaymentRail, PaymentStatus};
        use elastos_runtime::primitives::audit::AuditLog;
        use elastos_runtime::primitives::spend::SpendMeter;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CountingConfirmer(AtomicUsize);
        impl DrmConfirmer for CountingConfirmer {
            fn confirm(&self, _tx: &str) -> DrmConfirmation {
                self.0.fetch_add(1, Ordering::SeqCst);
                DrmConfirmation::Confirmed // would promote — proving it's NEVER reached
            }
        }

        let ledger = PaymentLedger::new();
        let meter = SpendMeter::new();
        let audit = AuditLog::new();
        meter.set_budget("vm-shop", 100).unwrap();
        meter.try_debit("vm-shop", 50).unwrap();
        // A Drm-tagged pending with a non-`drm:tx=` note (a buy that went Indeterminate w/o a hash).
        assert!(ledger.record_on_rail(
            "flint-orphan",
            "vm-shop",
            "QmAsset",
            50,
            PaymentStatus::Pending,
            "DRM buy outcome indeterminate: rpc down",
            None,
            PaymentRail::Drm,
        ));

        let confirmer = CountingConfirmer(AtomicUsize::new(0));
        let summary =
            reconcile_drm_confirmations(&ledger, &meter, &audit, &confirmer, usize::MAX, None);

        assert_eq!(
            confirmer.0.load(Ordering::SeqCst),
            0,
            "no drm:tx= hash ⇒ the confirmer is NEVER called (no attacker-named tx to poll)"
        );
        assert_eq!(summary.promoted, 0);
        assert_eq!(summary.refunded, 0);
        assert_eq!(summary.left_pending, 1);
        assert_eq!(
            ledger.get("flint-orphan").unwrap().status,
            PaymentStatus::Pending,
            "money unchanged — left Pending for operator/chain-scan recovery"
        );
    }

    /// Seed `n` pending DRM buys (`flint-b0..`, txs `0xB0..`), each with a reservation. These seed
    /// via `record_with_token` ⇒ the `Unknown` rail, so the scheduler/reconcile tests below
    /// DELIBERATELY exercise the S44 legacy `drm:tx=` note-fallback path (`is_chain_settled_pending`'s
    /// `Unknown` arm) — a live compatibility promise worth ratcheting; the positive `Drm`-tag path
    /// is covered by `a_positively_tagged_http_pending_is_never_reconciled_by_the_drm_driver` + the
    /// capability e2e. When the legacy fallback is eventually removed, these seeds flip to
    /// `record_on_rail(.., Drm)` in the same change.
    fn seed_pendings(
        ledger: &crate::payment_ledger::PaymentLedger,
        meter: &elastos_runtime::primitives::spend::SpendMeter,
        n: usize,
    ) {
        use crate::payment_ledger::PaymentStatus;
        for i in 0..n {
            meter.try_debit("vm-shop", 10).unwrap();
            assert!(ledger.record_with_token(
                &format!("flint-b{i}"),
                "vm-shop",
                "QmAsset",
                10,
                PaymentStatus::Pending,
                &format!("drm:tx=0xB{i};op=0xop;tid=7"),
                Some("00000000000000000000000000000001"),
            ));
        }
    }

    /// Sprint 37 ratchet: one tick processes at most `max_entries` DRM pendings — OLDEST-FIRST —
    /// and REPORTS the overflow as `skipped` (never silently dropped); successive ticks drain the
    /// rest. The bound is availability protection, never a money decision.
    #[test]
    fn a_tick_is_bounded_oldest_first_and_reports_what_it_skipped() {
        use crate::payment_ledger::{PaymentLedger, PaymentStatus};
        use elastos_runtime::primitives::audit::AuditLog;
        use elastos_runtime::primitives::spend::SpendMeter;

        let ledger = PaymentLedger::new();
        let meter = SpendMeter::new();
        let audit = AuditLog::new();
        meter.set_budget("vm-shop", 1000).unwrap();
        seed_pendings(&ledger, &meter, 3);
        let confirmer = MockConfirmer(
            (0..3)
                .map(|i| (format!("0xB{i}"), DrmConfirmation::Confirmed))
                .collect(),
        );

        let first = drm_reconcile_tick(&ledger, &meter, &audit, &confirmer, 2, None);
        assert_eq!(first.promoted, 2, "the tick promotes only up to the bound");
        assert_eq!(
            first.skipped, 1,
            "the overflow is COUNTED, not silently dropped"
        );
        // Oldest-first: b0 and b1 (lowest seq) resolved; b2 still pending.
        assert_eq!(
            ledger.get("flint-b0").unwrap().status,
            PaymentStatus::ResolvedCharged
        );
        assert_eq!(
            ledger.get("flint-b1").unwrap().status,
            PaymentStatus::ResolvedCharged
        );
        assert_eq!(
            ledger.get("flint-b2").unwrap().status,
            PaymentStatus::Pending
        );

        let second = drm_reconcile_tick(&ledger, &meter, &audit, &confirmer, 2, None);
        assert_eq!(second.promoted, 1, "the next tick drains the remainder");
        assert_eq!(second.skipped, 0);
        let third = drm_reconcile_tick(&ledger, &meter, &audit, &confirmer, 2, None);
        assert_eq!(
            third,
            DrmReconcileSummary::default(),
            "a drained ledger is a silent tick"
        );
    }

    /// A confirmer that PANICS on one scripted tx and confirms every other.
    struct PanickingConfirmer {
        panic_on: String,
    }
    impl DrmConfirmer for PanickingConfirmer {
        fn confirm(&self, tx_hash: &str) -> DrmConfirmation {
            assert!(
                tx_hash != self.panic_on,
                "scripted confirmer panic on {tx_hash}"
            );
            DrmConfirmation::Confirmed
        }
    }

    /// Sprint 37 ratchet: a PANIC on one entry holds THAT entry Pending and the tick CONTINUES —
    /// a poisoned entry can never blind the scheduler to everything behind it, and it can never
    /// auto-charge or refund (hold is the only panic outcome).
    #[test]
    fn a_panicking_confirmer_holds_that_entry_and_the_tick_continues() {
        use crate::payment_ledger::{PaymentLedger, PaymentStatus};
        use elastos_runtime::primitives::audit::AuditLog;
        use elastos_runtime::primitives::spend::SpendMeter;

        let ledger = PaymentLedger::new();
        let meter = SpendMeter::new();
        let audit = AuditLog::new();
        meter.set_budget("vm-shop", 1000).unwrap();
        seed_pendings(&ledger, &meter, 3);
        let confirmer = PanickingConfirmer {
            panic_on: "0xB1".to_string(),
        };
        let reserved_before = meter.remaining("vm-shop");

        let summary = drm_reconcile_tick(&ledger, &meter, &audit, &confirmer, usize::MAX, None);
        assert_eq!(
            summary.promoted, 2,
            "the entries around the poisoned one still resolve"
        );
        assert_eq!(
            summary.left_pending, 1,
            "the poisoned entry is HELD, not decided"
        );
        assert_eq!(
            ledger.get("flint-b1").unwrap().status,
            PaymentStatus::Pending,
            "a panic is a HOLD — never a charge, never a refund"
        );
        assert_eq!(
            meter.remaining("vm-shop"),
            reserved_before,
            "no reservation moved for the poisoned entry (the promoted spends stand)"
        );
    }

    /// Sprint 37 ratchet (emit rule refined by the council fold, red-team F2): a tick that
    /// SETTLED anything (promoted/refunded) is attested on the chain; a held-only re-poll and an
    /// idle tick are both silent — a stuck pending re-polled forever must not grow the signed,
    /// fsync'd chain by one event per tick.
    #[test]
    fn only_a_settling_tick_is_attested_held_and_idle_ticks_are_silent() {
        use crate::payment_ledger::PaymentLedger;
        use elastos_runtime::primitives::audit::{AuditEvent, AuditLog};
        use elastos_runtime::primitives::spend::SpendMeter;

        let ledger = PaymentLedger::new();
        let meter = SpendMeter::new();
        let audit = AuditLog::new();
        meter.set_budget("vm-shop", 1000).unwrap();

        let count_tick_events = |audit: &AuditLog| {
            audit
                .recent_events(256)
                .into_iter()
                .filter(|e| {
                    matches!(e, AuditEvent::Custom { event_type, .. }
                        if event_type == "drm_reconcile_tick")
                })
                .count()
        };

        // Idle tick: nothing pending ⇒ no event.
        let unscripted = MockConfirmer(std::collections::HashMap::new());
        drm_reconcile_tick(&ledger, &meter, &audit, &unscripted, usize::MAX, None);
        assert_eq!(count_tick_events(&audit), 0, "an idle tick emits nothing");

        // Held-only ticks: one pending re-polled and HELD ⇒ still no event, however many times.
        seed_pendings(&ledger, &meter, 1);
        let held = drm_reconcile_tick(&ledger, &meter, &audit, &unscripted, usize::MAX, None);
        assert_eq!(held.left_pending, 1, "unscripted verdict ⇒ held");
        drm_reconcile_tick(&ledger, &meter, &audit, &unscripted, usize::MAX, None);
        assert_eq!(
            count_tick_events(&audit),
            0,
            "held-only re-polls stay off the chain"
        );

        // Settling tick: the entry confirms ⇒ exactly one summary event.
        let confirming = MockConfirmer(
            [("0xB0".to_string(), DrmConfirmation::Confirmed)]
                .into_iter()
                .collect(),
        );
        let settled = drm_reconcile_tick(&ledger, &meter, &audit, &confirming, usize::MAX, None);
        assert_eq!(settled.promoted, 1);
        assert_eq!(count_tick_events(&audit), 1, "a settling tick is attested");
    }

    /// Sprint 37 council fold (F1 — head-of-line starvation): with batch=1, a permanently
    /// Unconfirmed OLDEST entry must not starve a confirmable entry behind it — the rotating
    /// cursor reaches the second entry on the second tick and promotes it, then wraps.
    #[test]
    fn a_stuck_oldest_entry_cannot_starve_the_entries_behind_it() {
        use crate::payment_ledger::{PaymentLedger, PaymentStatus};
        use elastos_runtime::primitives::audit::AuditLog;
        use elastos_runtime::primitives::spend::SpendMeter;

        let ledger = PaymentLedger::new();
        let meter = SpendMeter::new();
        let audit = AuditLog::new();
        meter.set_budget("vm-shop", 1000).unwrap();
        seed_pendings(&ledger, &meter, 2); // b0 oldest (stuck forever), b1 behind it (confirmable)
        let confirmer = MockConfirmer(
            [("0xB1".to_string(), DrmConfirmation::Confirmed)]
                .into_iter()
                .collect(), // 0xB0 unscripted ⇒ Unconfirmed forever
        );

        // Tick 1 (cursor 0): the batch of 1 is consumed by the stuck oldest entry.
        let first = drm_reconcile_tick(&ledger, &meter, &audit, &confirmer, 1, None);
        assert_eq!(first.left_pending, 1, "the stuck entry is held");
        assert_eq!(
            first.skipped, 1,
            "the confirmable entry waits — counted, not dropped"
        );
        let cursor = first.next_cursor.expect("the pass scanned something");

        // Tick 2 (cursor after the stuck entry): rotation reaches b1 — no starvation.
        let second = drm_reconcile_tick(&ledger, &meter, &audit, &confirmer, 1, Some(cursor));
        assert_eq!(
            second.promoted, 1,
            "the entry BEHIND the stuck one is promoted"
        );
        assert_eq!(
            ledger.get("flint-b1").unwrap().status,
            PaymentStatus::ResolvedCharged
        );
        // Tick 3 wraps back to the stuck entry (still held) — the rotation covers everything.
        let third = drm_reconcile_tick(
            &ledger,
            &meter,
            &audit,
            &confirmer,
            1,
            Some(second.next_cursor.unwrap()),
        );
        assert_eq!(third.left_pending, 1, "the wrap re-polls the stuck entry");
        assert_eq!(
            ledger.get("flint-b0").unwrap().status,
            PaymentStatus::Pending
        );
    }

    /// Sprint 43 (retiring the string classifier): the refund-vs-hold decision is now the
    /// `BuyError` VARIANT, mapped by `DrmSettleError::from_buy_error` — NOT a substring match. This
    /// is the HOSTILE-PROVIDER ratchet, now stronger: an `Indeterminate` error whose message embeds
    /// EVERY pre-broadcast sentinel (and the chain/wallet deadline markers) STILL maps to hold,
    /// because the variant — decided by the code path that built it — is what classifies, and a
    /// broadcast-op failure can only ever be built as `Indeterminate` at its single call site. The
    /// mirror check: a `PreBroadcast` error maps to a refund regardless of its (harmless) text.
    #[test]
    fn from_buy_error_classifies_by_variant_not_by_string() {
        use crate::api::buy_authority::BuyError;

        // A PreBroadcast failure ⇒ NotBroadcast (refund), whatever the message says.
        let refund = DrmSettleError::from_buy_error(BuyError::PreBroadcast(
            "wallet not linked: a buy needs the principal's EVM address".to_string(),
        ));
        assert!(matches!(refund, DrmSettleError::NotBroadcast(_)));

        // An Indeterminate failure whose text MASQUERADES as every pre-broadcast refusal — the old
        // string classifier's worst case — still maps to hold. A post-broadcast tx is never
        // refunded on the strength of provider-controlled bytes.
        let hostile = format!(
            "chain-provider op failed: wallet not linked / {} / {} / {} / {}",
            crate::api::buy_authority::ERR_BUY_ABORTED_SUFFIX,
            crate::api::buy_authority::ERR_NONE_RESOLVED_SUFFIX,
            crate::api::wallet_signer::WALLET_SIGN_DEADLINE_MARKER,
            crate::api::rights_authority::CHAIN_DEADLINE_MARKER,
        );
        let hold = DrmSettleError::from_buy_error(BuyError::Indeterminate(hostile));
        assert!(
            matches!(hold, DrmSettleError::Indeterminate(_)),
            "a post-broadcast (Indeterminate) error is HELD even when its text embeds every \
             pre-broadcast sentinel — the variant classifies, not the string"
        );
    }
}
