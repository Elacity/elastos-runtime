//! The ERC-20 checkout rail (Sprint 48 — Track D2): the SECOND market vertical behind the
//! `PaymentProvider` seam, proving the contract (docs/SPEC-market-provider-v1.md) is not
//! DRM-shaped. An agent pays an arbitrary EVM address in ONE operator-declared ERC-20 under its
//! mandate cap: `runtime.pay { payee: "0x…", amount }` becomes `transfer(payee, amount ×
//! ELASTOS_ERC20_SPEND_UNIT)` on the declared token — held `Pending` at broadcast, promoted to
//! charged only after on-chain confirmation (the same chain-settled reconciler spine as the DRM
//! rail), refunded exactly once on revert.
//!
//! MONEY INVARIANTS (the S43 discipline, by construction — the call site decides, not the bytes):
//! - Every failure STRICTLY BEFORE the broadcast op (bad payee address, unit overflow, calldata
//!   assembly, the wallet SIGN leg incl. the chain PREPARE read) is `PayError::NotCharged` —
//!   provably nothing moved, the cap reservation is refunded.
//! - The broadcast op and everything after is `PayError::Indeterminate` carrying the
//!   `erc20:tx=<hash>;to=…;amount=…;tok=…` rail_ref — the tx may be out; the reservation is HELD
//!   and the reconciler resolves it from the chain receipt. A success return does not exist:
//!   like the DRM rail, a broadcast-accepted checkout is NEVER "charged" until confirmed.
//! - `rail()` is `PaymentRail::Erc20` (compiler-forced), so the reconciler selects these pendings
//!   by STRUCTURED tag; a hostile HTTP endpoint crafting an `erc20:tx=` note is never polled.
//!
//! SCOPE HONESTY: the live leg signs inside the wallet capsule (managed account), which is a
//! `dev-modes` build capability — a RELEASE build refuses with `NotCharged` (nothing moves).
//! The external-signature checkout flow (release-grade) is a tracked follow-up; the DRM rail has
//! the same posture. Mock settlement (gate tests) requires BOTH `dev-modes` AND the operator's
//! explicit mock-money opt-in at wiring time.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::intent_executor::{PayError, PaymentProvider};

/// `transfer(address,uint256)` — the canonical ERC-20 transfer selector.
pub(crate) const ERC20_TRANSFER_SELECTOR: &str = "a9059cbb";

/// Normalize an EVM address: `0x` + 40 hex chars, lowercased. Anything else is refused — a payee
/// that is not a real address must fail BEFORE any money moves (P11).
pub(crate) fn normalize_evm_address(raw: &str) -> Result<String, String> {
    let t = raw.trim();
    let hex_part = t
        .strip_prefix("0x")
        .or_else(|| t.strip_prefix("0X"))
        .ok_or_else(|| format!("payee {t:?} is not a 0x-prefixed EVM address"))?;
    if hex_part.len() != 40 || !hex_part.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "payee {t:?} is not a 40-hex-char EVM address — refusing before any money moves"
        ));
    }
    Ok(format!("0x{}", hex_part.to_ascii_lowercase()))
}

/// Encode `transfer(to, amount)` calldata. PURE (unit-tested): selector + address word + amount
/// word — the exact bytes the token contract executes, so the encoding invariant can never
/// silently regress.
pub(crate) fn encode_erc20_transfer(to: &str, amount_base: u128) -> Result<String, String> {
    let addr = normalize_evm_address(to)?;
    Ok(format!(
        "0x{ERC20_TRANSFER_SELECTOR}{:0>64}{:064x}",
        addr.trim_start_matches("0x"),
        amount_base
    ))
}

/// A deterministic, even-length-hex "signed tx" for the MOCK settlement path (the mock chain
/// never inspects it; it is not a real signature) — mirrors the DRM rail's mock discipline.
fn representative_signed_tx(unsigned: &Value) -> String {
    let mut h = Sha256::new();
    h.update(b"elastos-erc20/checkout-mock-signed/v1");
    h.update(
        serde_json::to_string(unsigned)
            .unwrap_or_default()
            .as_bytes(),
    );
    format!("0x02{}", hex::encode(h.finalize()))
}

/// How this provider settles. Selected at WIRING time (not per-pay), like the DRM rail's mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Erc20Mode {
    /// Mock settlement through the chain-provider's mock broadcast (gate tests / demos). Wired
    /// only under `dev-modes` AND the explicit mock-money opt-in — see `build_pay_rail`.
    Mock,
    /// Live settlement: wallet-capsule managed signing + real broadcast (dev-modes builds; a
    /// release build refuses fail-closed — external signing is the tracked follow-up).
    Live,
}

/// The ERC-20 checkout provider — see the module docs for the contract it implements.
pub struct Erc20CheckoutProvider {
    /// The ONE ERC-20 token this rail pays in (operator-declared; the unit mapping denominates it).
    token: String,
    /// Token base-units per meter unit (like the DRM `ELASTOS_DRM_SPEND_UNIT`) — REQUIRED ≥ 1 at
    /// wiring, so the mandate cap is a literal on-chain ceiling in the declared token.
    spend_unit: u128,
    /// The managed-account owner for the live sign leg.
    principal_id: String,
    mode: Erc20Mode,
}

impl Erc20CheckoutProvider {
    pub fn new(token: String, spend_unit: u128, principal_id: String, mode: Erc20Mode) -> Self {
        Self {
            token,
            spend_unit: spend_unit.max(1),
            principal_id,
            mode,
        }
    }

    /// The canonical `rail_ref` for a broadcast checkout:
    /// `erc20:tx=<hash>;to=<payee>;amount=<base units>;tok=<token>` — compact, greppable, and
    /// delimiter-stripped per component (a hostile field cannot forge the parsed binding), the
    /// same discipline as the DRM `rail_ref` (council S34 red-team F3).
    fn rail_ref(&self, tx_hash: &str, to: &str, amount_base: u128) -> String {
        let clean = |s: &str| s.replace([';', '='], "");
        format!(
            "erc20:tx={};to={};amount={amount_base};tok={}",
            clean(tx_hash),
            clean(to),
            clean(&self.token)
        )
    }
}

impl PaymentProvider for Erc20CheckoutProvider {
    fn rail(&self) -> crate::payment_ledger::PaymentRail {
        // Positively tag checkout pendings so the chain-settled reconciler selects them by this
        // structured discriminator (Sprint 44's contract) — never by rail-controlled text.
        crate::payment_ledger::PaymentRail::Erc20
    }

    fn pay(&self, payee: &str, amount: u64, _idempotency_key: &str) -> Result<String, PayError> {
        // ── Every leg here is STRICTLY PRE-BROADCAST ⇒ NotCharged (refund) by construction. ──
        let to = normalize_evm_address(payee).map_err(PayError::NotCharged)?;
        let amount_base = (amount as u128)
            .checked_mul(self.spend_unit)
            .ok_or_else(|| {
                PayError::NotCharged(format!(
                    "amount {amount} × unit {} overflows the token amount word — refusing \
                     before any money moves",
                    self.spend_unit
                ))
            })?;
        let data = encode_erc20_transfer(&to, amount_base).map_err(PayError::NotCharged)?;

        match self.mode {
            Erc20Mode::Mock => {
                #[cfg(feature = "dev-modes")]
                {
                    let unsigned = json!({ "to": self.token, "value": "0x0", "data": data });
                    let signed = representative_signed_tx(&unsigned);
                    // ── The broadcast op and after ⇒ Indeterminate (HELD) by construction. ──
                    let tx_hash = super::chain_tx::broadcast_signed_mock(&unsigned, &signed)
                        .map_err(PayError::Indeterminate)?;
                    Err(PayError::Indeterminate(self.rail_ref(
                        &tx_hash,
                        &to,
                        amount_base,
                    )))
                }
                #[cfg(not(feature = "dev-modes"))]
                {
                    Err(PayError::NotCharged(
                        "mock ERC-20 settlement is a dev-modes build capability — nothing moved"
                            .to_string(),
                    ))
                }
            }
            Erc20Mode::Live => {
                #[cfg(feature = "dev-modes")]
                {
                    // SIGN leg (incl. the chain PREPARE read inside the closure) runs strictly
                    // BEFORE broadcast ⇒ a failure here is provably pre-broadcast (the S43/S46
                    // discipline; the prepare-deadline refund ratchet covers this call shape).
                    let chain_id = crate::api::buy_authority::chain_id_default();
                    let token = self.token.clone();
                    let sig = super::wallet_signer::sign_with_managed_account(
                        &self.principal_id,
                        chain_id,
                        |from| super::chain_tx::prepare_intent_live(from, &token, "0x0", &data),
                    )
                    .map_err(PayError::NotCharged)?;
                    // ── The broadcast op and after ⇒ Indeterminate (HELD) by construction. ──
                    let tx_hash = super::chain_tx::broadcast_signed_live(&sig.signed_transaction)
                        .map_err(PayError::Indeterminate)?;
                    Err(PayError::Indeterminate(self.rail_ref(
                        &tx_hash,
                        &to,
                        amount_base,
                    )))
                }
                #[cfg(not(feature = "dev-modes"))]
                {
                    Err(PayError::NotCharged(
                        "live ERC-20 checkout signs in the wallet capsule (managed account), a \
                         dev-modes build capability; the external-signature checkout flow is a \
                         tracked follow-up — nothing moved (fail-closed)"
                            .to_string(),
                    ))
                }
            }
        }
    }
}

/// The chain-confirmation reader for checkout pendings — the same receipt + depth-floor logic as
/// the DRM rail (one confirmation discipline, P5), so the in-runtime scheduler polls
/// `erc20:tx=` pendings exactly like `drm:tx=` ones.
impl crate::drm_marketplace::DrmConfirmer for Erc20CheckoutProvider {
    fn confirm(&self, tx_hash: &str) -> crate::drm_marketplace::DrmConfirmation {
        crate::drm_marketplace::confirm_chain_tx(tx_hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAYEE: &str = "0x34DAf31b99b5A59CEB18e424dbC112FA6E5F3dc3";

    #[test]
    fn transfer_calldata_encodes_selector_address_and_amount_words() {
        let data = encode_erc20_transfer(PAYEE, 1_000_000).expect("valid encode");
        let body = data.strip_prefix("0x").unwrap();
        assert_eq!(&body[..8], ERC20_TRANSFER_SELECTOR);
        assert_eq!(body.len(), 8 + 64 + 64, "selector + two ABI words");
        assert_eq!(
            &body[8..72],
            format!("{:0>64}", PAYEE[2..].to_ascii_lowercase()),
            "address word, left-padded, lowercased"
        );
        assert_eq!(
            &body[72..],
            format!("{:064x}", 1_000_000u128),
            "amount word"
        );
    }

    #[test]
    fn junk_payees_are_refused_before_any_money_moves() {
        for junk in [
            "QmNotAnAddress",
            "0x1234",                                       // too short
            "0x34daf31b99b5a59ceb18e424dbc112fa6e5f3dc3ff", // too long
            "0xZZZZf31b99b5a59ceb18e424dbc112fa6e5f3dc3",   // non-hex
            "",
        ] {
            assert!(normalize_evm_address(junk).is_err(), "must refuse {junk:?}");
        }
        // The provider maps that refusal to NotCharged (refund) — pre-broadcast by construction.
        let p = Erc20CheckoutProvider::new(
            "0x1111111111111111111111111111111111111111".into(),
            1_000_000,
            "did:test:payer".into(),
            Erc20Mode::Mock,
        );
        match p.pay("not-an-address", 5, "flint-k") {
            Err(PayError::NotCharged(_)) => {}
            other => panic!("junk payee must be NotCharged, got {other:?}"),
        }
    }

    #[test]
    fn a_unit_overflow_is_refused_before_any_money_moves() {
        let p = Erc20CheckoutProvider::new(
            "0x1111111111111111111111111111111111111111".into(),
            u128::MAX,
            "did:test:payer".into(),
            Erc20Mode::Mock,
        );
        match p.pay(PAYEE, 2, "flint-k") {
            Err(PayError::NotCharged(why)) => {
                assert!(why.contains("overflow"), "names the refusal: {why}")
            }
            other => panic!("overflow must be NotCharged, got {other:?}"),
        }
    }

    /// The vertical's own money ratchet (mirrors the DRM rail's): a broadcast-ACCEPTED checkout
    /// is NEVER charged at broadcast — it is `Indeterminate`, HELD, carrying a parseable
    /// `erc20:tx=` rail_ref the chain-settled reconciler can poll.
    #[test]
    #[cfg(all(unix, feature = "dev-modes"))]
    fn a_broadcast_checkout_is_held_indeterminate_with_a_parseable_rail_ref() {
        let _g = crate::api::ddrm_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let stub = dir.path().join("ok-broadcast.sh");
        std::fs::write(
            &stub,
            "#!/bin/sh\nread _i\nprintf '{\"status\":\"ok\",\"data\":{}}\\n'\nread _o\nprintf \
             '{\"status\":\"ok\",\"data\":{\"transaction_hash\":\"0xfeedbeef\"}}\\n'\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("ELASTOS_CHAIN_PROVIDER_BIN", &stub);

        let p = Erc20CheckoutProvider::new(
            "0x1111111111111111111111111111111111111111".into(),
            1_000_000,
            "did:test:payer".into(),
            Erc20Mode::Mock,
        );
        let out = p.pay(PAYEE, 3, "flint-k");
        std::env::remove_var("ELASTOS_CHAIN_PROVIDER_BIN");

        match out {
            Err(PayError::Indeterminate(rail_ref)) => {
                assert!(
                    rail_ref.starts_with("erc20:tx=0xfeedbeef;"),
                    "parseable chain-settled rail_ref: {rail_ref}"
                );
                assert!(
                    rail_ref.contains(";amount=3000000;"),
                    "amount in token base units (3 × 1_000_000): {rail_ref}"
                );
            }
            other => panic!("a broadcast checkout must be HELD Indeterminate, got {other:?}"),
        }
    }
}
