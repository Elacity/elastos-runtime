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
//! - the tx confirmed ⇒ `Ok(rail_ref)` — the money moved; the reference names the tx.
//! - anything the settler cannot prove either way (broadcast then lost, RPC timeout) ⇒
//!   [`PayError::Indeterminate`] — the reservation is KEPT (the charge may have posted), resolved
//!   out of band via the idempotency key, exactly like the HTTP rail's post-send ambiguity.

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

/// The confirmed settlement of a DRM buy — the on-chain truth the receipt will carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrmSettlement {
    /// The buy transaction hash (`0x…`), as the chain/adapter reported it.
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

/// Execute a buy for an already-resolved binding and classify the outcome two-generals-honestly.
/// Production wraps `buy_authority::buy_access`; CI injects a mock that returns each branch.
pub trait DrmSettler: Send + Sync {
    fn settle(
        &self,
        binding: &DrmBinding,
        amount: u64,
        idempotency_key: &str,
    ) -> Result<DrmSettlement, DrmSettleError>;
}

/// A [`PaymentProvider`] whose rail is the DRM marketplace. Resolve (fail-closed) → settle
/// (two-generals) → a `rail_ref` naming the tx + the bound `(operative, tokenId)`.
pub struct DrmMarketplaceProvider {
    resolver: Arc<dyn DrmResolver>,
    settler: Arc<dyn DrmSettler>,
}

impl DrmMarketplaceProvider {
    pub fn new(resolver: Arc<dyn DrmResolver>, settler: Arc<dyn DrmSettler>) -> Self {
        Self { resolver, settler }
    }

    /// The canonical `rail_ref` for a settled DRM buy: `drm:tx=<hash>;op=<operative>;tid=<tokenId>`.
    /// Compact, greppable, and (after the pay path's `sanitize_rail_note`) printable-bounded before
    /// it enters the signed receipt.
    fn rail_ref(binding: &DrmBinding, settlement: &DrmSettlement) -> String {
        format!(
            "drm:tx={};op={};tid={}",
            settlement.tx_hash, binding.operative, binding.token_id
        )
    }
}

impl PaymentProvider for DrmMarketplaceProvider {
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
        // Settle. NotBroadcast ⇒ refund; Indeterminate ⇒ keep the reservation (the tx may confirm).
        let settlement = self
            .settler
            .settle(&binding, amount, idempotency_key)
            .map_err(|e| match e {
                DrmSettleError::NotBroadcast(why) => {
                    PayError::NotCharged(format!("DRM buy not broadcast: {why}"))
                }
                DrmSettleError::Indeterminate(why) => {
                    PayError::Indeterminate(format!("DRM buy outcome indeterminate: {why}"))
                }
            })?;
        Ok(Self::rail_ref(&binding, &settlement))
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
}

impl DrmResolver for ChainDrmMarketplace {
    fn resolve(&self, asset_ref: &str) -> Result<DrmBinding, DrmResolveError> {
        // The MKT-1-hardened live resolver: it accumulates every distinct (operative, tokenId)
        // across the whole channel range and binds ONLY when exactly one exists, else returns an
        // `ambiguous_kid_binding` error string — which we classify as Ambiguous (fail closed).
        match crate::api::chain_tx::resolve_token_id_live(asset_ref, &self.ledger) {
            Ok((token_id, operative)) if !token_id.is_empty() => Ok(DrmBinding {
                content_id: asset_ref.to_string(),
                operative,
                token_id,
            }),
            Ok(_) => Err(DrmResolveError::Unresolvable(
                "chain-provider returned an empty tokenId".to_string(),
            )),
            Err(e) if e.contains("ambiguous") => Err(DrmResolveError::Ambiguous(e)),
            Err(e) => Err(DrmResolveError::Unresolvable(e)),
        }
    }
}

impl DrmSettler for ChainDrmMarketplace {
    fn settle(
        &self,
        binding: &DrmBinding,
        _amount: u64,
        _idempotency_key: &str,
    ) -> Result<DrmSettlement, DrmSettleError> {
        // Pass the already-resolved binding to the buy so it never re-resolves (and never re-opens
        // the ambiguity window): operative + tokenId are pinned. The on-chain price is sourced LIVE
        // from the listing (the meter already capped the spend-unit budget; the meter-unit ⇄
        // on-chain-price reconciliation is a stated residual — KNOWN_GAPS MKT-DRM).
        let now_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let target = crate::api::buy_authority::BuyTarget {
            operative: Some(binding.operative.clone()),
            token_id: Some(binding.token_id.clone()),
            ledger: Some(self.ledger.clone()),
            ..Default::default()
        };
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
            // buy_access returns a plain error string. We classify CONSERVATIVELY: only errors
            // that provably describe a PRE-BROADCAST refusal are NotBroadcast (refundable);
            // anything that might have reached the chain stays Indeterminate (keep the
            // reservation — the one unbreakable invariant). The recognized pre-broadcast phrases
            // are the buy path's own fail-closed guards.
            Err(e) if is_pre_broadcast_refusal(&e) => Err(DrmSettleError::NotBroadcast(e)),
            Err(e) => Err(DrmSettleError::Indeterminate(e)),
        }
    }
}

/// Whether a `buy_access` error string PROVABLY describes a refusal BEFORE any broadcast — the only
/// case safe to classify NotCharged (refund). Everything else is indeterminate: a broadcast may
/// have landed. These phrases are the buy path's own pre-send fail-closed guards (wallet linkage,
/// listing resolution, sold-out, price-drift abort).
fn is_pre_broadcast_refusal(err: &str) -> bool {
    const PRE_BROADCAST: &[&str] = &[
        "wallet not linked",
        "listing sold out",
        "buy aborted (fail closed)",
        "ambiguous",
        "unresolved",
        "no active listing",
        "missing channel",
    ];
    let lower = err.to_lowercase();
    PRE_BROADCAST.iter().any(|p| lower.contains(p))
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

    /// A scripted settler that records whether it was called and returns a fixed outcome.
    struct MockSettler {
        outcome: Mutex<Option<Result<DrmSettlement, &'static str>>>,
        called: Mutex<bool>,
    }
    impl MockSettler {
        fn new(outcome: Result<DrmSettlement, &'static str>) -> Self {
            Self {
                outcome: Mutex::new(Some(outcome)),
                called: Mutex::new(false),
            }
        }
    }
    impl DrmSettler for MockSettler {
        fn settle(
            &self,
            _b: &DrmBinding,
            _a: u64,
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
    fn a_confirmed_buy_returns_a_rail_ref_naming_the_tx_and_binding() {
        let provider = DrmMarketplaceProvider::new(
            Arc::new(MockResolver(Ok(binding()))),
            Arc::new(MockSettler::new(Ok(DrmSettlement {
                tx_hash: "0xdead".to_string(),
            }))),
        );
        let rail_ref = provider.pay("QmAsset", 500, "flint-sig").unwrap();
        assert_eq!(rail_ref, "drm:tx=0xdead;op=0xop;tid=42");
    }

    #[test]
    fn an_ambiguous_binding_is_not_charged_and_never_settles() {
        let settler = Arc::new(MockSettler::new(Ok(DrmSettlement {
            tx_hash: "0xshouldnothappen".to_string(),
        })));
        let provider =
            DrmMarketplaceProvider::new(Arc::new(MockResolver(Err("ambiguous"))), settler.clone());
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
        );
        assert!(matches!(
            not_broadcast.pay("QmAsset", 1, "k").unwrap_err(),
            PayError::NotCharged(_)
        ));

        let indeterminate = DrmMarketplaceProvider::new(
            Arc::new(MockResolver(Ok(binding()))),
            Arc::new(MockSettler::new(Err("indeterminate"))),
        );
        assert!(matches!(
            indeterminate.pay("QmAsset", 1, "k").unwrap_err(),
            PayError::Indeterminate(_)
        ));
    }

    #[test]
    fn pre_broadcast_refusal_classification_is_conservative() {
        // Recognized pre-send guards ⇒ refundable.
        assert!(is_pre_broadcast_refusal("wallet not linked: a buy needs the EVM address"));
        assert!(is_pre_broadcast_refusal(
            "listing sold out (on-chain supply 0) — buy aborted (fail closed)"
        ));
        // An unclassifiable error must NOT be treated as pre-broadcast (stays indeterminate).
        assert!(!is_pre_broadcast_refusal("rpc connection reset after send"));
        assert!(!is_pre_broadcast_refusal("nonce too low"));
    }
}
