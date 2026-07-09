//! LIVE testnet DRM buy — the operator/CI-driven last mile (Sprint 45).
//!
//! This drives the REAL [`ChainDrmMarketplace`] (resolve → on-chain price gate → sign → broadcast →
//! confirm) against a live EVM testnet, exactly as `docs/LIVE_BUY_RUNBOOK.md` describes. It is
//! compiled ONLY under `--features live-chain` and is additionally `#[ignore]`d, so it NEVER runs in
//! the default CI gate (a sandbox has no funded wallet, no live RPC, and no deployed listing). Run
//! it deliberately, with the runbook's environment set:
//!
//! ```sh
//! # …export the LIVE_BUY_RUNBOOK env (ELASTOS_DDRM_RIGHTS=chain, ELASTOS_DDRM_BUY_SIGN=wallet,
//! #   ELASTOS_CHAIN_PROVIDER_BIN, ELASTOS_WALLET_PROVIDER_BIN, ELASTOS_DDRM_CHAIN_ID,
//! #   ELASTOS_DDRM_BUY_LEDGER, ELASTOS_DRM_SPEND_UNIT, ELASTOS_DRM_PAY_TOKEN, …) plus the three
//! #   test-only vars below, then:
//! cargo test -p elastos-server --features live-chain --test live_drm_buy -- --ignored --nocapture
//! ```
//!
//! Test-only environment (in addition to the runbook's):
//! * `ELASTOS_LIVE_BUY_ASSET`   — the asset/content ref to buy (the `payee` of the pay intent).
//! * `ELASTOS_LIVE_BUY_CAP`     — the mandate cap in meter units (must cover `price ÷ SPEND_UNIT`).
//! * `ELASTOS_LIVE_BUY_PRINCIPAL` — the buyer principal DID (the managed account's owner).
//!
//! SCOPE — this test drives the ADAPTER→CHAIN leg only: `DrmMarketplaceProvider::pay` (resolve →
//! on-chain price gate → sign → broadcast) and `ChainDrmMarketplace::confirm`. It asserts the buy
//! is HELD at broadcast — an `Indeterminate` reservation carrying a real `drm:tx=<hash>`, NEVER
//! `Ok`/charged (the S35 invariant) — or PRE-broadcast-refused fail-closed (cap too low / sold out /
//! drift — a legitimate NotCharged the runbook expects you to fix); then that the tx reaches the
//! confirmation-depth floor on the real chain. The rest of the money spine — ledger custody
//! (`begin_attempt`/record-before-broadcast), the spend meter, `reconcile_drm_confirmations`
//! promotion, and receipt binding+export — is NOT exercised here; it is gate-proven against mocks
//! (the S35 e2e + the S45 `verify_receipt_cmd` CLI ratchets) and driven live end to end by the
//! gateway per LIVE_BUY_RUNBOOK.md §2 (steps 3–5, Grant→Act→Prove). NOTE: this has not yet been run
//! against a live testnet in this repo — the first operator to run it should record the tx hash.
#![cfg(feature = "live-chain")]

use std::sync::Arc;
use std::time::Duration;

use elastos_server::drm_marketplace::{
    ChainDrmMarketplace, DrmConfirmation, DrmConfirmer, DrmMarketplaceProvider,
};
use elastos_server::intent_executor::{PayError, PaymentProvider};

/// Read a required env var or panic with a runbook-pointing message (this test is opt-in; a missing
/// var is operator error, not a silent skip that could mask a broken wiring).
fn require(key: &str) -> String {
    match std::env::var(key) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => panic!(
            "{key} is required for the live DRM buy — see docs/LIVE_BUY_RUNBOOK.md (§1 environment)"
        ),
    }
}

/// Extract the tx hash from a settled DRM `rail_ref` (`drm:tx=<hash>;op=…`). Inlined (the crate's
/// `parse_drm_tx` is `pub(crate)`); the format is the stable receipt contract.
fn tx_from_rail_ref(rail_ref: &str) -> Option<&str> {
    rail_ref.strip_prefix("drm:tx=")?.split(';').next()
}

#[test]
#[ignore = "live testnet buy — needs a funded wallet + live RPC; run explicitly, never in the gate"]
fn a_live_testnet_drm_buy_holds_then_confirms() {
    // The runbook environment selects the LIVE chain rights + managed signing; assert the operator
    // actually armed it (a plain build would silently mock).
    assert_eq!(
        std::env::var("ELASTOS_DDRM_RIGHTS").as_deref(),
        Ok("chain"),
        "set ELASTOS_DDRM_RIGHTS=chain for the LIVE path (runbook §1)"
    );
    // Equally mandatory: the subject is empty (the managed account is the buyer), so the settle can
    // only sign with managed wallet signing armed. Without it the buy fails ERR_WALLET_NOT_LINKED
    // (a NotCharged whose fixed advice below would misdirect) — assert it up front (council G-F1).
    assert_eq!(
        std::env::var("ELASTOS_DDRM_BUY_SIGN").as_deref(),
        Ok("wallet"),
        "set ELASTOS_DDRM_BUY_SIGN=wallet — the managed account signs the buy (runbook §1)"
    );

    let asset = require("ELASTOS_LIVE_BUY_ASSET");
    let principal = require("ELASTOS_LIVE_BUY_PRINCIPAL");
    let ledger = require("ELASTOS_DDRM_BUY_LEDGER");
    let cap: u64 = require("ELASTOS_LIVE_BUY_CAP")
        .parse()
        .expect("ELASTOS_LIVE_BUY_CAP must be a u64 (meter units)");
    let spend_unit: u128 = require("ELASTOS_DRM_SPEND_UNIT")
        .parse()
        .expect("ELASTOS_DRM_SPEND_UNIT must be a u128 (pay-token base units per meter unit)");
    let pay_token = require("ELASTOS_DRM_PAY_TOKEN");

    // The managed account is the authoritative buyer in wallet-signing mode ⇒ empty subject.
    let chain = Arc::new(ChainDrmMarketplace::new(principal, String::new(), ledger));
    let provider = DrmMarketplaceProvider::new(
        chain.clone(), // DrmResolver
        chain.clone(), // DrmSettler
        spend_unit,
        Some(pay_token),
    );

    // ACT — one real buy under the cap. The idempotency key stands in for a signed intent's key.
    let idempotency_key = format!("flint-live-{asset}");
    let outcome = provider.pay(&asset, cap, &idempotency_key);

    let rail_ref = match outcome {
        // Broadcast-accepted: HELD, never charged here — carries the real tx (runbook §2 step 3).
        Err(PayError::Indeterminate(rail_ref)) => {
            eprintln!("buy broadcast (held): {rail_ref}");
            rail_ref
        }
        // A provable pre-broadcast refusal is a legitimate outcome (cap too low, sold out, drift) —
        // the runbook expects the operator to fix the listing/cap and re-run. Surface and stop.
        Err(PayError::NotCharged(why)) => {
            panic!(
                "buy refused PRE-broadcast (fail-closed, nothing charged): {why}\n\
                 fix the listing/cap/pay-token per LIVE_BUY_RUNBOOK.md §1 and re-run"
            );
        }
        Ok(reference) => panic!(
            "a DRM buy must NEVER report charged at broadcast (S35) — got Ok({reference}); the \
             reservation must be HELD until the chain confirms"
        ),
    };

    let tx = tx_from_rail_ref(&rail_ref)
        .expect("a settled DRM rail_ref carries drm:tx=<hash>")
        .to_string();
    // The rail_ref must carry a real EVM tx hash (0x + 64 lowercase hex), not a stub string — so a
    // misconfigured provider binary answering `drm:tx=abc` cannot make the confirm loop pass on
    // garbage (council red-team F4).
    assert!(
        tx.strip_prefix("0x")
            .is_some_and(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit())),
        "expected a 32-byte EVM tx hash (0x + 64 hex), got {tx:?} — check the chain-provider binary"
    );
    eprintln!("confirming tx {tx} …");

    // CONFIRM — poll the real chain until the depth floor is met (bounded; this is operator-run, so
    // a real sleep is fine — it is never in the gate). ~5 min budget at 10s cadence.
    let mut confirmed = false;
    for attempt in 0..30 {
        match chain.confirm(&tx) {
            DrmConfirmation::Confirmed => {
                confirmed = true;
                break;
            }
            DrmConfirmation::Reverted => {
                panic!(
                    "tx {tx} REVERTED on-chain — the buy did not settle (reservation refundable)"
                )
            }
            DrmConfirmation::Unconfirmed(why) => {
                eprintln!("attempt {attempt}: not yet confirmed ({why}); waiting…");
                std::thread::sleep(Duration::from_secs(10));
            }
        }
    }
    assert!(
        confirmed,
        "tx {tx} did not reach the confirmation-depth floor within the poll budget. Do NOT blindly \
         re-run: this standalone test bypasses the runtime's ledger dedup and `settle` ignores the \
         idempotency key, so a re-run BROADCASTS A SECOND REAL BUY. First resolve tx {tx} on-chain \
         (it may still confirm); only then re-run, or raise the budget / ELASTOS_DRM_MIN_CONFIRMATIONS"
    );
    eprintln!("LIVE DRM buy confirmed on-chain: tx={tx}");
}
