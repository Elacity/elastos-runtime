//! Gateway-side buy-flow orchestration: put a real access token in the wallet.
//!
//! This is PC2's stage 5 (`AuthorityGateway.buyAccess(...)` → operative Access Token,
//! role 1) brought onto the runtime as an ORCHESTRATION over the existing real seams —
//! the gateway invents NO contract semantics:
//!
//!   resolve listing/price  ->  assemble buyAccess tx { to, value, data }
//!     ->  sign (wallet)  ->  broadcast (chain-provider `eth_sendRawTransaction`)
//!     ->  await receipt  ->  the rights gate's `hasAccessByContentId` now reads true
//!
//! The `buyAccess` CALLDATA is operator-pinned config, exactly like the `has_access` and
//! `mint` selectors are pinned from real PC2 source — never a guessed signature. The
//! arg layout the gateway assembles (`contentId` ‖ `subject` ‖ `amount`) is the demo's
//! documented default and is fully overridable; nothing about the contract is hardcoded
//! as product truth.
//!
//! Three modes, selected by `ELASTOS_DDRM_RIGHTS` (shared with the rights gate):
//!   - `dev` — record the purchase in the local owned-token ledger and return a
//!     deterministic synthetic tx hash. Offline; no chain, no signing.
//!   - `chain-mock` — assemble the calldata and broadcast a representative signed tx
//!     through the REAL `chain-provider.broadcast_transaction` op against an in-process
//!     JSON-RPC mock (the real broadcast code path runs), then record the purchase in
//!     the ledger so the subsequent open's `chain-mock` rights read (`…=ledger`) returns
//!     owned. Proves not-owned → buy → own → open end to end on a Mac, no network.
//!   - `chain` — assemble the `{ to, value, data }` against the configured Base contract and
//!     return it UNSIGNED for the user's external wallet. SCOPE hard-gate (SCOPE.md:32/53): the
//!     production buy path is unsigned -> external-wallet ONLY. A `dev-modes` build may instead
//!     (with `ELASTOS_DDRM_BUY_SIGN=wallet`) source nonce/gas via `chain-provider.prepare_transaction`,
//!     sign inside `wallet-provider` with a managed account (key never leaves the capsule), and
//!     broadcast — for offline/live TESTING only. A release build ignores that opt-in and either
//!     broadcasts an EXTERNALLY-signed tx (`ELASTOS_DDRM_BUY_SIGNED_TX`) or hands back the unsigned tx.
//!
//! Runtime signing (`ELASTOS_DDRM_BUY_SIGN=wallet`) also applies to `chain-mock`: the
//! wallet capsule signs a well-formed buyAccess tx and the genuine signed bytes are
//! broadcast through the in-process RPC mock, proving the full sign→broadcast rail offline.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::rights_authority::{env_nonempty, RightsMode};

/// The outcome of a buy-access orchestration.
#[derive(Debug)]
pub struct BuyOutcome {
    /// The broadcast (or synthetic) transaction hash.
    pub tx_hash: String,
    /// True once the purchase is reflected so the rights gate will now allow the open.
    pub owned_now: bool,
    /// The mode that produced this outcome (audit/debug).
    pub mode: String,
    /// The assembled `buyAccess` call (`to` / `value` / `data`), for audit + external
    /// signing. Carries no secret.
    pub unsigned_tx: Value,
}

/// Real Base AuthorityGateway — `buyAccess` is sent here (from `~/.pc2` `wallet.js` /
/// `abis.ts`), NOT to the operative directly. Default `to` for the buy; overridable.
const BASE_AUTHORITY_GATEWAY: &str = "0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D";

/// Real `buyAccess` selectors (`keccak256(sig)[..4]`, confirmed via `~/.pc2` ethers):
/// the 5-arg form is paid in the chain's native token (`value = price`); the 6-arg form
/// adds an ERC-20 `payToken` (USDC on Base) and requires a prior `approve` of the
/// operative's `paymentProcessor`.
const BUY_ACCESS_NATIVE_SELECTOR: &str = "0xf7580ad9"; // buyAccess(address,address,uint256,uint256,uint256)
const BUY_ACCESS_ERC20_SELECTOR: &str = "0x0ede2294"; // + address payToken

/// USDC on Base (6 decimals) — the default Elacity payment token.
const BASE_USDC: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";

/// `approve(address spender, uint256 amount)` selector (`keccak256(sig)[..4]`). The ERC-20 buy's
/// prerequisite approve, where `spender` is the operative's `paymentProcessor()` (NOT the gateway).
const ERC20_APPROVE_SELECTOR: &str = "0x095ea7b3";

/// Default offline fees for the `chain-mock` wallet-signing path (no RPC to source them).
/// The mock does not execute the transaction, so these only need to be well-formed.
const MOCK_NONCE: &str = "0x0";
const MOCK_GAS_PRICE: &str = "0x3b9aca00"; // 1 gwei
const MOCK_GAS_LIMIT: &str = "0x186a0"; // 100k — comfortably covers contract calldata

/// True when managed-account EVM signing through the wallet capsule is active
/// (`ELASTOS_DDRM_BUY_SIGN=wallet`). SCOPE.md:32/53 HARD-GATE: the production buy path is UNSIGNED ->
/// external wallet, never the managed-account autosign mode (it self-approves the signature
/// server-side, P16). Managed signing is therefore reachable ONLY in a `dev-modes` build (the offline
/// chain-mock sign->broadcast proof + live-chain dev testing); a release build always returns the
/// assembled unsigned tx for the user's external wallet, so this is unconditionally `false` there.
fn wallet_signing() -> bool {
    cfg!(feature = "dev-modes")
        && env_nonempty("ELASTOS_DDRM_BUY_SIGN").as_deref() == Some("wallet")
}

/// The EVM chain id for the buy (default Base mainnet); overridable for other deployments.
fn chain_id_default() -> u64 {
    env_nonempty("ELASTOS_DDRM_CHAIN_ID")
        .and_then(|s| s.parse().ok())
        .unwrap_or(8453)
}

/// The asset identity + buyer-agreed terms the storefront supplies for a live buy (it has these from the
/// discovery index / `/api/market/get`). On the `chain` path the seller/price/payToken are sourced LIVE
/// from `sellersOf`/`listings` (keyed at ACCESS_TOKEN id=1) using this identity — NO `ELASTOS_DDRM_BUY_*`
/// pins required. `expected_price`/`expected_pay_token` are what the buyer saw in the UI: when present
/// they arm abort-on-drift (P11) against the live re-read. All fields optional; env values still override
/// (dev/fixtures) and an empty target falls back to the env/resolve path.
#[derive(Debug, Clone, Default)]
pub struct BuyTarget {
    pub operative: Option<String>,
    pub token_id: Option<String>,
    pub ledger: Option<String>,
    pub quantity: Option<String>,
    pub seller: Option<String>,
    pub expected_price: Option<String>,
    pub expected_pay_token: Option<String>,
}

/// Buy an access token for `content_id` on behalf of `subject` (the principal's linked
/// EVM wallet). `content_id` MUST be the same identifier the rights gate keys on, so the
/// recorded ownership matches the subsequent open.
pub fn buy_access(
    principal_id: &str,
    content_id: &str,
    subject: &str,
    now_unix: u64,
    target: &BuyTarget,
) -> Result<BuyOutcome, String> {
    let mode = super::rights_authority::rights_mode();

    // Chain modes are keyed on a real wallet — fail closed without one (same rule the
    // rights gate enforces, so a buy can never disagree with the ownership read). When the
    // operator opted into runtime signing, the managed account IS the wallet, so an
    // unlinked external `subject` is fine — the signer's address becomes authoritative.
    if matches!(mode, RightsMode::Chain | RightsMode::ChainMock)
        && subject.trim().is_empty()
        && !wallet_signing()
    {
        return Err("wallet not linked: a buy needs the principal's EVM address".to_string());
    }

    let unsigned_tx = assemble_buy_tx(content_id, subject, None);

    match mode {
        RightsMode::Dev => {
            super::owned_ledger::record(content_id, &dev_subject(principal_id, subject))?;
            Ok(BuyOutcome {
                tx_hash: synthetic_hash(content_id, subject, now_unix),
                owned_now: true,
                mode: "dev".to_string(),
                unsigned_tx,
            })
        }
        RightsMode::ChainMock => {
            if wallet_signing() {
                // REAL signing, offline: the wallet capsule signs a well-formed buyAccess
                // tx with a managed key, and the REAL chain-provider broadcast op sends the
                // genuine signed bytes through the in-process RPC mock. Proves the full
                // sign→broadcast rail on a Mac with no network. The managed account is the
                // authoritative buyer, so ownership is recorded under its address.
                let chain_id = chain_id_default();
                let mut intent_seen = Value::Null;
                let sig = super::wallet_signer::sign_with_managed_account(
                    principal_id,
                    chain_id,
                    |from| {
                        let intent = mock_transaction_intent(from, content_id, chain_id);
                        intent_seen = intent.clone();
                        Ok(intent)
                    },
                )?;
                let tx_hash =
                    super::chain_tx::broadcast_signed_mock(&intent_seen, &sig.signed_transaction)?;
                super::owned_ledger::record(content_id, &sig.signer)?;
                return Ok(BuyOutcome {
                    tx_hash,
                    owned_now: true,
                    mode: "chain-mock+wallet".to_string(),
                    unsigned_tx: buy_audit_view(&intent_seen, &sig),
                });
            }
            // Run the REAL chain-provider broadcast op against an in-process RPC mock that
            // returns a canned tx hash, so the production broadcast path is exercised.
            let tx_hash = broadcast_mock(&unsigned_tx)?;
            // The mock chain has no token state, so record the purchase in the ledger the
            // chain-mock rights read (`ELASTOS_DDRM_CHAIN_ACCESS=ledger`) consults.
            super::owned_ledger::record(content_id, subject)?;
            Ok(BuyOutcome {
                tx_hash,
                owned_now: true,
                mode: "chain-mock".to_string(),
                unsigned_tx,
            })
        }
        RightsMode::Chain => {
            // Phase-1 (P11 fail closed): a LIVE buy binds the REAL ledger tokenId (resolved from the KID
            // or supplied by the storefront — NEVER word_from_id, a content hash) and sources the listing
            // seller/price/payToken LIVE from sellersOf/listings (id=1). No ELASTOS_DDRM_BUY_* pins
            // required; env still overrides for dev. Fails closed on a missing channel, an unresolved
            // tokenId, or no active listing. The CEK path is untouched (P15).
            let sourced = source_buy_terms(content_id, target)?;
            if sourced.supply == 0 {
                return Err(
                    "listing sold out (on-chain supply 0) — buy aborted (fail closed)".to_string(),
                );
            }
            // Abort-on-drift (P11): if the buyer agreed to a price/pay-token in the UI, the live re-read
            // MUST match — else fail closed (the listing changed under them).
            if let Some(expected) = sourced.expected.as_ref() {
                ensure_no_drift(expected, &sourced.live)?;
            }
            let unsigned = assemble_buy_tx_core(content_id, subject, &sourced.terms);
            if wallet_signing() {
                // Live path: source real nonce/gas + assemble the intent via the REAL chain-provider
                // `prepare_transaction`, sign inside the wallet capsule (key never leaves), and broadcast
                // the signed bytes through the REAL chain-provider — the seam that makes `chain` live.
                let chain_id = chain_id_default();
                let mut intent_seen = Value::Null;
                let sig = super::wallet_signer::sign_with_managed_account(
                    principal_id,
                    chain_id,
                    |from| {
                        let intent = prepare_live_intent(from, content_id, &sourced.terms)?;
                        intent_seen = intent.clone();
                        Ok(intent)
                    },
                )?;
                let tx_hash = super::chain_tx::broadcast_signed_live(&sig.signed_transaction)?;
                // Ownership is read back from `hasAccessByContentId` once the tx confirms,
                // not from the local ledger; owned_now reflects "broadcast accepted".
                return Ok(BuyOutcome {
                    tx_hash,
                    owned_now: false,
                    mode: "chain+wallet".to_string(),
                    unsigned_tx: buy_audit_view(&intent_seen, &sig),
                });
            }
            // Real chain, no runtime signing: broadcast an externally-signed tx if provided, else hand
            // back the live-assembled unsigned tx for the user's external wallet (the release path).
            let Some(signed) = env_nonempty("ELASTOS_DDRM_BUY_SIGNED_TX") else {
                return Err(format!(
                    "live buy needs a signature: either opt into runtime signing with \
                     ELASTOS_DDRM_BUY_SIGN=wallet (the wallet capsule signs with a managed \
                     key), or sign this assembled tx externally and resubmit via \
                     ELASTOS_DDRM_BUY_SIGNED_TX. unsigned_tx={unsigned}"
                ));
            };
            let tx_hash = super::chain_tx::broadcast_signed_live(&signed)?;
            // On real chain, ownership is read back from `hasAccessByContentId` once the
            // tx confirms — NOT from the local ledger. owned_now reflects "broadcast
            // accepted", not "confirmed"; the open re-reads the chain.
            Ok(BuyOutcome {
                tx_hash,
                owned_now: false,
                mode: "chain".to_string(),
                unsigned_tx: unsigned,
            })
        }
    }
}

/// The on-chain price + pay-token of a DRM listing, READ-ONLY (Sprint 36 — the price gate). The
/// price is the pay-token's smallest-unit amount as a decimal string (e.g. USDC 6-decimals);
/// `pay_token` is the ERC-20 address, or `"native"` for a zero-address (ETH) listing. Sourced
/// WITHOUT broadcasting so the pay gate can compare the mandate's cap against the real cost before
/// any money moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuyQuote {
    pub price: String,
    pub pay_token: String,
    pub supply: u128,
}

/// Read-only quote of what `buy_access` WOULD charge for `content_id`, without broadcasting
/// (Sprint 36). On the `chain` path it live-sources the lowest active listing's price + pay-token
/// via [`source_buy_terms`] (fail-closed on no listing / sold out); on dev/chain-mock there is no
/// real listing, so it returns a FREE quote (`price = "0"`, native) — the price gate is a no-op in
/// those insecure modes, which already require the explicit mock opt-in to wire the DRM rail.
pub fn quote_buy(content_id: &str, target: &BuyTarget) -> Result<BuyQuote, String> {
    match super::rights_authority::rights_mode() {
        RightsMode::Chain => {
            let sourced = source_buy_terms(content_id, target)?;
            if sourced.supply == 0 {
                return Err(
                    "listing sold out (on-chain supply 0) — buy aborted (fail closed)".to_string(),
                );
            }
            Ok(BuyQuote {
                price: sourced.live.price,
                pay_token: if sourced.live.pay_token.eq_ignore_ascii_case(ZERO_ADDR) {
                    "native".to_string()
                } else {
                    sourced.live.pay_token
                },
                supply: sourced.supply,
            })
        }
        // Dev / ChainMock have no real listing to price — a FREE quote (the gate always passes).
        _ => Ok(BuyQuote {
            price: "0".to_string(),
            pay_token: "native".to_string(),
            supply: 1,
        }),
    }
}

/// The zero EVM address (a native/ETH listing's `payToken`).
const ZERO_ADDR: &str = "0x0000000000000000000000000000000000000000";
/// Cap the live seller scan when sourcing the lowest active listing (bounded RPC fan-out).
const MAX_SELLERS_SCAN: usize = 8;

/// The live-sourced buy: the assembled `terms`, the asset's `operative`, the live-read listing `live`
/// terms + `supply`, and the buyer-agreed `expected` terms (for abort-on-drift, when the UI supplied a
/// price).
#[derive(Debug)]
struct SourcedBuy {
    terms: BuyTerms,
    live: BoundTerms,
    supply: u128,
    expected: Option<BoundTerms>,
}

/// Pick the lowest-priced ACTIVE seller for `(operative, ACCESS_TOKEN=1)` from `sellersOf` + per-seller
/// `listings` (both keyed at id=1; READ-ONLY). Mirrors `/api/market/get`'s selection so the buy binds the
/// same listing the storefront showed. Fails closed when no active listing exists.
fn pick_lowest_active_seller(
    gateway: &str,
    operative: &str,
    token_id_word: &str,
) -> Result<String, String> {
    let sellers = super::market_reads::sellers_of_live(gateway, operative, token_id_word)?;
    let mut best: Option<(String, u128)> = None;
    for s in sellers.iter().take(MAX_SELLERS_SCAN) {
        if let Ok((terms, supply)) = read_listing_terms(gateway, operative, token_id_word, s) {
            if supply == 0 {
                continue;
            }
            let price: u128 = terms.price.parse().unwrap_or(u128::MAX);
            if best.as_ref().is_none_or(|(_, bp)| price < *bp) {
                best = Some((s.clone(), price));
            }
        }
    }
    best.map(|(s, _)| s).ok_or_else(|| {
        "no active listing for this asset (sellersOf/listings empty at ACCESS_TOKEN id=1) — buy \
         aborted (fail closed)"
            .to_string()
    })
}

/// Live-source the buyAccess terms for `content_id` from the asset identity the storefront supplies
/// (`target`) + on-chain `sellersOf`/`listings` (keyed at ACCESS_TOKEN id=1). No `ELASTOS_DDRM_BUY_*`
/// pins required on the live path; env values still override (dev/fixtures). Resolves the real ledger
/// tokenId/operative (target → pinned env → KID→AssetCreated via the channel), picks the lowest active
/// seller, and binds price/payToken from that seller's LIVE listing. Fails closed on a missing channel,
/// an unresolved tokenId, or no active listing (P11).
fn source_buy_terms(content_id: &str, target: &BuyTarget) -> Result<SourcedBuy, String> {
    let gateway =
        env_nonempty("ELASTOS_DDRM_BUY_TO").unwrap_or_else(|| BASE_AUTHORITY_GATEWAY.to_string());
    // The asset's channel ledger — required to resolve KID→tokenId AND as the buyAccess `ledger` arg.
    let ledger = target
        .ledger
        .clone()
        .or_else(|| env_nonempty("ELASTOS_DDRM_BUY_LEDGER"))
        .ok_or(
            "live buy requires the asset's channel/ledger (the storefront supplies it from the \
             listing) — fail closed",
        )?;

    // (tokenId, operative): supplied by the storefront > pinned env > resolved from the KID via
    // chain-provider (scan AssetCreated + bind the mint calldata). NEVER word_from_id (a content hash,
    // not the ledger tokenId): a fabricated tokenId debits the wallet for access never granted.
    let (token_id, operative) = match (target.token_id.clone(), target.operative.clone()) {
        (Some(t), Some(o)) => (t, o),
        _ => match env_nonempty("ELASTOS_DDRM_BUY_TOKEN_ID") {
            Some(pinned) => {
                let op = target
                    .operative
                    .clone()
                    .or_else(|| env_nonempty("ELASTOS_DDRM_BUY_OPERATIVE"))
                    .ok_or(
                        "pinned ELASTOS_DDRM_BUY_TOKEN_ID also needs the operative (per-asset \
                         ERC-1155) to re-read listing terms",
                    )?;
                (pinned, op)
            }
            None => super::chain_tx::resolve_token_id_live(content_id, &ledger).map_err(|e| {
                format!(
                    "live buy requires the resolved on-chain tokenId (KID->AssetCreated mint \
                     calldata) — word_from_id(content_id) is NOT the real ledger tokenId: {e}"
                )
            })?,
        },
    };
    if operative.trim().is_empty() {
        return Err(
            "abort-on-drift requires the asset's operative; none resolved — fail closed"
                .to_string(),
        );
    }
    let token_id_word = token_id_to_word(&token_id);

    // The on-chain listing seller keys the `listings` re-read. Supplied by the storefront > pinned env >
    // the lowest active seller read live from `sellersOf` (id=1). NEVER defaults to the buyer (a
    // self-query returns nothing and the drift echo can't detect a forged seller).
    let seller = match target
        .seller
        .clone()
        .or_else(|| env_nonempty("ELASTOS_DDRM_BUY_SELLER"))
    {
        Some(s) => s,
        None => pick_lowest_active_seller(&gateway, &operative, &token_id_word)?,
    };

    // Bind price/payToken from the chosen seller's LIVE listing (read at id=1).
    let (live, supply) = read_listing_terms(&gateway, &operative, &token_id_word, &seller)?;

    let quantity = target
        .quantity
        .clone()
        .or_else(|| env_nonempty("ELASTOS_DDRM_BUY_QUANTITY"))
        .unwrap_or_else(|| "1".to_string());

    // A zero-address payToken is a native listing; map it to the native form for assembly.
    let pay_token = if live.pay_token.eq_ignore_ascii_case(ZERO_ADDR) {
        String::new()
    } else {
        live.pay_token.clone()
    };
    let terms = BuyTerms {
        gateway,
        ledger,
        seller: seller.clone(),
        token_id_word: token_id_word.clone(),
        quantity,
        price: live.price.clone(),
        pay_token,
    };

    // Abort-on-drift arms only when the buyer agreed to a price in the UI; compared against `live` below.
    let expected = target.expected_price.clone().map(|price| BoundTerms {
        seller,
        token_id: format!("0x{token_id_word}"),
        price,
        pay_token: target
            .expected_pay_token
            .clone()
            .unwrap_or_else(|| live.pay_token.clone()),
    });

    Ok(SourcedBuy {
        terms,
        live,
        supply,
        expected,
    })
}

/// The concrete buyAccess listing terms a tx is assembled from. On the `chain` path these are sourced
/// LIVE (`source_buy_terms`); on dev/mock they come from env (`buy_terms_from_env`). `pay_token` empty or
/// `"native"` selects the native form; any other value is the ERC-20 token address.
#[derive(Debug, Clone)]
struct BuyTerms {
    gateway: String,
    ledger: String,
    seller: String,
    token_id_word: String,
    quantity: String,
    price: String,
    pay_token: String,
}

/// Build `BuyTerms` from `ELASTOS_DDRM_BUY_*` env (dev/mock/fixtures). The `tokenId` defaults to a 32-byte
/// word derived from `content_id` (a content hash, NOT the ledger tokenId) — dev/mock ONLY; the live
/// `chain` path sources a resolved tokenId via `source_buy_terms`, so the fallback never binds a real buy.
fn buy_terms_from_env(
    content_id: &str,
    subject: &str,
    token_id_override: Option<&str>,
) -> BuyTerms {
    let gateway =
        env_nonempty("ELASTOS_DDRM_BUY_TO").unwrap_or_else(|| BASE_AUTHORITY_GATEWAY.to_string());
    // Default ledger to the AuthorityGateway is wrong; default it to the (overridable) channel. With no
    // channel pinned we fall back to `gateway` so calldata stays well-formed.
    let ledger = env_nonempty("ELASTOS_DDRM_BUY_LEDGER").unwrap_or_else(|| gateway.clone());
    let seller = env_nonempty("ELASTOS_DDRM_BUY_SELLER").unwrap_or_else(|| subject.to_string());
    let quantity = env_nonempty("ELASTOS_DDRM_BUY_QUANTITY").unwrap_or_else(|| "1".to_string());
    let price = env_nonempty("ELASTOS_DDRM_BUY_PRICE").unwrap_or_else(|| "0".to_string());
    let pay_token =
        env_nonempty("ELASTOS_DDRM_BUY_PAYTOKEN").unwrap_or_else(|| BASE_USDC.to_string());
    let token_id_word = match token_id_override
        .map(str::to_string)
        .or_else(|| env_nonempty("ELASTOS_DDRM_BUY_TOKEN_ID"))
    {
        Some(t) => token_id_to_word(&t),
        None => word_from_id(content_id),
    };
    BuyTerms {
        gateway,
        ledger,
        seller,
        token_id_word,
        quantity,
        price,
        pay_token,
    }
}

/// Assemble the REAL `buyAccess` transaction the wallet signs, matching PC2's
/// `wallet.js`:
///   `buyAccess(address seller, address ledger, uint256 tokenId, uint256 quantity,
///              uint256 pricePerToken[, address payToken])`
/// sent to the AuthorityGateway. The ERC-20 form (default, USDC on Base) carries a
/// `payToken` and `value = 0` and requires a prior `approve` of the operative's
/// `paymentProcessor`; the native form omits `payToken` and pays `value = price`. Pure: no RPC, no keys.
fn assemble_buy_tx(content_id: &str, subject: &str, token_id_override: Option<&str>) -> Value {
    assemble_buy_tx_core(
        content_id,
        subject,
        &buy_terms_from_env(content_id, subject, token_id_override),
    )
}

/// Pure buyAccess assembly from explicit `BuyTerms` (the single calldata path for env + live-sourced
/// terms). Pure: no RPC, no keys.
fn assemble_buy_tx_core(content_id: &str, subject: &str, terms: &BuyTerms) -> Value {
    let to = terms.gateway.clone();
    let ledger = terms.ledger.clone();
    let seller = terms.seller.clone();
    let quantity = terms.quantity.clone();
    let price = terms.price.clone();
    let pay_token = terms.pay_token.clone();
    let native = pay_token.is_empty() || pay_token.eq_ignore_ascii_case("native");
    let token_id_word = terms.token_id_word.clone();

    // Phase-1: the order TOTAL = pricePerToken * quantity (the *quantity multiplier was missing — a
    // single-unit price underpaid a multi-unit buy). Used as the native `value` AND the ERC-20 approve
    // amount. Fail-closed to 0 on parse/overflow (a 0 value reverts at the contract, never overpays).
    let total_hex = match (price.parse::<u128>(), quantity.parse::<u128>()) {
        (Ok(p), Ok(q)) => p
            .checked_mul(q)
            .map(|n| format!("0x{n:x}"))
            .unwrap_or_else(|| "0x0".to_string()),
        _ => "0x0".to_string(),
    };
    let (selector, value, mut data) = if native {
        (
            BUY_ACCESS_NATIVE_SELECTOR.to_string(),
            total_hex.clone(),
            BUY_ACCESS_NATIVE_SELECTOR
                .trim_start_matches("0x")
                .to_string(),
        )
    } else {
        (
            BUY_ACCESS_ERC20_SELECTOR.to_string(),
            "0x0".to_string(),
            BUY_ACCESS_ERC20_SELECTOR
                .trim_start_matches("0x")
                .to_string(),
        )
    };

    // Args: seller, ledger, tokenId, quantity, pricePerToken[, payToken].
    data.push_str(&word_from_address(&seller));
    data.push_str(&word_from_address(&ledger));
    data.push_str(&token_id_word);
    data.push_str(&word_from_uint(&quantity));
    data.push_str(&word_from_uint(&price));
    if !native {
        data.push_str(&word_from_address(&pay_token));
    }

    // Phase-1: surface the ERC-20 approve leg as a concrete tx. spender = the asset's Operative
    // `paymentProcessor()` (read live; NOT the gateway). Pinned via env until the gateway resolves it
    // from the operative; amount = the order total (price * quantity). Null on the native path.
    let approve = if native {
        Value::Null
    } else {
        let processor = env_nonempty("ELASTOS_DDRM_BUY_PAYMENT_PROCESSOR").unwrap_or_default();
        if processor.is_empty() {
            json!({
                "required": true,
                "spender": "Operative.paymentProcessor() — resolve live (pin ELASTOS_DDRM_BUY_PAYMENT_PROCESSOR)",
                "pay_token": pay_token,
                "amount": total_hex,
            })
        } else {
            erc20_approve_call(&pay_token, &processor, &total_hex)
        }
    };

    json!({
        "to": to,
        "value": value,
        "data": format!("0x{data}"),
        "selector": selector,
        "content_id": content_id,
        "subject": subject,
        "seller": seller,
        "ledger": ledger,
        "pay_token": if native { Value::Null } else { json!(pay_token) },
        // The ERC-20 approve leg (spender = Operative.paymentProcessor, NOT the gateway), batched
        // before buyAccess; null on the native path.
        "approve": approve,
    })
}

/// Assemble the offline (`chain-mock`) `unsigned_transaction_intent/v1` the wallet capsule
/// signs. Identical schema to `chain-provider.prepare_transaction`, but the fees are
/// well-formed constants (the mock never executes the call) and `to` falls back to a valid
/// placeholder when no real contract is pinned, so the capsule's address validation passes.
fn mock_transaction_intent(from: &str, content_id: &str, chain_id: u64) -> Value {
    let tx = assemble_buy_tx(content_id, from, None);
    let to = tx
        .get("to")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(BASE_AUTHORITY_GATEWAY);
    let value = tx.get("value").and_then(Value::as_str).unwrap_or("0x0");
    let data = tx.get("data").and_then(Value::as_str).unwrap_or("0x");
    json!({
        "schema": "elastos.chain.unsigned_transaction_intent/v1",
        "transaction_type": "eip155_legacy",
        "from": from,
        "to": to,
        "value": value,
        "data": data,
        "chain_id": chain_id,
        "nonce": MOCK_NONCE,
        "gas_price": MOCK_GAS_PRICE,
        "gas_limit": MOCK_GAS_LIMIT,
        "requires_wallet_approval": true,
        "wallet_intent": "transaction_intent",
    })
}

/// Source real nonce/gas and assemble the live `unsigned_transaction_intent/v1` for the
/// buy via the shared chain plumbing. The returned intent is exactly what the wallet
/// capsule's `transaction_intent` consumes.
fn prepare_live_intent(from: &str, content_id: &str, terms: &BuyTerms) -> Result<Value, String> {
    let tx = assemble_buy_tx_core(content_id, from, terms);
    let to = tx
        .get("to")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("live buy requires ELASTOS_DDRM_BUY_TO (the AuthorityGateway/contract address)")?
        .to_string();
    let value = tx
        .get("value")
        .and_then(Value::as_str)
        .unwrap_or("0x0")
        .to_string();
    let data = tx
        .get("data")
        .and_then(Value::as_str)
        .unwrap_or("0x")
        .to_string();
    super::chain_tx::prepare_intent_live(from, &to, &value, &data)
}

/// Broadcast through the REAL chain-provider against the in-process RPC mock. The mock
/// ignores calldata; a minimal even-length-hex signed tx satisfies the real
/// `validate_signed_transaction` so the broadcast op actually runs.
fn broadcast_mock(unsigned_tx: &Value) -> Result<String, String> {
    super::chain_tx::broadcast_signed_mock(unsigned_tx, &representative_signed_tx(unsigned_tx))
}

/// A non-secret audit view of a wallet-signed buy: the assembled call plus the recovered
/// signer and the signed-tx hash. Carries no key material.
fn buy_audit_view(intent: &Value, sig: &super::wallet_signer::ManagedSignature) -> Value {
    json!({
        "to": intent.get("to").cloned().unwrap_or(Value::Null),
        "value": intent.get("value").cloned().unwrap_or(Value::Null),
        "data": intent.get("data").cloned().unwrap_or(Value::Null),
        "from": sig.signer,
        "signer": sig.signer,
        "signed_tx_hash": sig.transaction_hash,
        "account_id": sig.account_id,
    })
}

/// A 32-byte word from a content identifier (its SHA-256), so the assembled calldata is
/// well-formed. The real contract would take a `bytes16 contentId`; this is the demo's
/// representative encoding only.
fn word_from_id(id: &str) -> String {
    hex::encode(Sha256::digest(id.as_bytes()))
}

/// Left-pad a 20-byte EVM address to a 32-byte word. Tolerates a missing `0x` / short
/// input by hashing (demo calldata is never sent to a real contract in mock/dev).
fn word_from_address(addr: &str) -> String {
    let clean = addr.trim().trim_start_matches("0x");
    if clean.len() == 40 && clean.bytes().all(|b| b.is_ascii_hexdigit()) {
        format!("{:0>64}", clean.to_ascii_lowercase())
    } else {
        hex::encode(Sha256::digest(addr.as_bytes()))
    }
}

/// A decimal `uint` as a 32-byte word (saturates absurd inputs; demo encoding only).
fn word_from_uint(dec: &str) -> String {
    let n: u128 = dec.trim().parse().unwrap_or(1);
    format!("{n:064x}")
}

/// A tokenId string -> 32-byte ABI word. Accepts a resolved `0x`+64-hex word (chain-provider
/// `resolve_token_id` output, used verbatim — tokenIds exceed u128) or a decimal uint (a pinned
/// override), falling back to `word_from_uint` for decimals.
pub(crate) fn token_id_to_word(t: &str) -> String {
    let clean = t.trim().trim_start_matches("0x");
    if clean.len() == 64 && clean.bytes().all(|b| b.is_ascii_hexdigit()) {
        clean.to_ascii_lowercase()
    } else {
        word_from_uint(t)
    }
}

/// `approve(spender, amount)` calldata on the pay-token ERC-20. `spender` = the asset's Operative
/// `paymentProcessor()` (read live; NOT the gateway). `amount_hex` is the order total (`0x…`). PURE.
fn erc20_approve_call(pay_token: &str, spender: &str, amount_hex: &str) -> Value {
    let amount = u128::from_str_radix(amount_hex.trim().trim_start_matches("0x"), 16).unwrap_or(0);
    let data = format!(
        "{}{}{:064x}",
        ERC20_APPROVE_SELECTOR.trim_start_matches("0x"),
        word_from_address(spender),
        amount,
    );
    json!({
        "to": pay_token,
        "value": "0x0",
        "data": format!("0x{data}"),
        "selector": ERC20_APPROVE_SELECTOR,
        "spender": spender,
        "note": "spender = Operative.paymentProcessor() (read live), NOT the gateway",
    })
}

/// `listings(address operative, uint256 tokenId, address seller) -> (uint256 qty, uint256
/// pricePerToken, address payToken)` selector (verified live on the AuthorityGateway bytecode; the
/// return word order confirmed against real Base listings + the `ItemListed` event — see
/// `decode_listing_return`). The pre-broadcast re-read for abort-on-drift.
const LISTINGS_SELECTOR: &str = "0x6bd3a64b";

/// The ERC-1155 ACCESS_TOKEN sub-token id (== 1) inside every per-asset Operative. `listings`/`sellersOf`
/// on the AuthorityGateway are ALWAYS keyed at THIS id — never the asset's content/ledger tokenId (which
/// `buyAccess` uses). Confirmed live on Base: `listings(op, 1, seller)` is populated while
/// `listings(op, contentTokenId, seller)` is empty (CONTRACTS.md §1.3; PC2 wallet.js TOKEN_ID_ACCESS=1).
/// Keying these reads at the content tokenId reads an empty slot → abort-on-drift / sold-out aborts
/// EVERY live buy and shows no `/get` price.
pub(crate) const ACCESS_TOKEN_ID_WORD: &str =
    "0000000000000000000000000000000000000000000000000000000000000001";

/// The listing terms a buy is bound to at assembly. Phase-1 abort-on-drift: immediately before
/// broadcast the gateway re-reads these live (`listings`) and MUST find them identical — else fail
/// closed (P11). Terms that silently changed = wrong price / pay-token (or sold out).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoundTerms {
    pub seller: String,
    pub token_id: String,
    pub price: String,
    pub pay_token: String,
}

/// Fail-closed term comparison: `Ok(())` iff every field matches (addresses/hex case-insensitive).
/// Any drift between assembly and the pre-broadcast re-read aborts the buy, naming the drifted field.
pub(crate) fn ensure_no_drift(bound: &BoundTerms, reread: &BoundTerms) -> Result<(), String> {
    let checks = [
        ("seller", &bound.seller, &reread.seller),
        ("tokenId", &bound.token_id, &reread.token_id),
        ("price", &bound.price, &reread.price),
        ("payToken", &bound.pay_token, &reread.pay_token),
    ];
    for (field, a, b) in checks {
        if !a.trim().eq_ignore_ascii_case(b.trim()) {
            return Err(format!(
                "listing drift on {field}: bound {a:?} != re-read {b:?} — buy aborted (fail closed)"
            ));
        }
    }
    Ok(())
}

/// Parse a 32-byte ABI word (64 hex, no `0x`) as a `u128`. Fail-closed if the high 128 bits are
/// non-zero (a price/quantity beyond u128 is absurd for a real listing — treat as malformed).
fn word_to_u128(word_hex: &str) -> Result<u128, String> {
    let w = word_hex.trim();
    if w.len() != 64 || !w.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("not a 32-byte hex word: {word_hex}"));
    }
    if w[..32].bytes().any(|b| b != b'0') {
        return Err("uint exceeds u128 (malformed listing word)".to_string());
    }
    u128::from_str_radix(&w[32..], 16).map_err(|e| e.to_string())
}

/// Decode a `listings(op, tokenId, seller)` return — SSOT word layout `(uint256 qty, uint256
/// pricePerToken, address payToken)` (CONTRACTS.md:106/230) — into the re-read `BoundTerms` plus the
/// remaining `quantity`. PURE (unit-tested); the live `eth_call` is `read_listing_terms`. Fails closed
/// (never panics) on a short or non-hex return so a hostile/compromised RPC cannot straddle a `&str`
/// byte-slice on a non-char boundary. Live-confirm the word order against a real listings() return.
pub(crate) fn decode_listing_return(
    result: &str,
    seller: &str,
    token_id_word: &str,
) -> Result<(BoundTerms, u128), String> {
    let clean = result.trim().trim_start_matches("0x");
    if clean.len() < 192 || !clean.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("listings() return is not 3+ hex words: {result}"));
    }
    let quantity = word_to_u128(&clean[0..64])?; // word 0 = qty (SSOT)
    let price = word_to_u128(&clean[64..128])?; // word 1 = pricePerToken (SSOT)
    let pay_token = format!("0x{}", &clean[128 + 24..192]); // word 2 = payToken (last 20 bytes)
    Ok((
        BoundTerms {
            seller: seller.to_string(),
            token_id: format!("0x{token_id_word}"),
            price: price.to_string(),
            pay_token,
        },
        quantity,
    ))
}

/// Encode `listings(operative, ACCESS_TOKEN=1, seller)` calldata. The AuthorityGateway keys listings at
/// the ERC-1155 ACCESS_TOKEN sub-id (== 1), NOT the asset content tokenId (`ACCESS_TOKEN_ID_WORD`). PURE
/// (unit-tested) so the id-keying invariant can't silently regress.
pub(crate) fn encode_listings(operative: &str, seller: &str) -> String {
    format!(
        "0x{}{}{}{}",
        LISTINGS_SELECTOR.trim_start_matches("0x"),
        word_from_address(operative),
        ACCESS_TOKEN_ID_WORD,
        word_from_address(seller),
    )
}

/// Re-read the on-chain listing terms for `(operative, seller)` via the gateway's `listings` view
/// (chain-provider `eth_call`), keyed at the ACCESS_TOKEN id (== 1) like the live chain. Live; the
/// decode is `decode_listing_return` (unit-tested). `token_id_word` is the asset's content tokenId —
/// echoed into the returned `BoundTerms.token_id` for display/drift (`buyAccess` binds it), NOT used to
/// key the read.
pub(crate) fn read_listing_terms(
    gateway: &str,
    operative: &str,
    token_id_word: &str,
    seller: &str,
) -> Result<(BoundTerms, u128), String> {
    let data = encode_listings(operative, seller);
    let result = super::chain_tx::contract_call_live(gateway, &data)?;
    decode_listing_return(&result, seller, token_id_word)
}

/// A minimal, even-length-hex "signed tx" that satisfies the real broadcast validator in
/// the mock path (the mock never inspects it; it is not a real signature).
fn representative_signed_tx(unsigned_tx: &Value) -> String {
    let mut h = Sha256::new();
    h.update(b"elastos-ddrm/buy-mock-signed/v1");
    h.update(
        serde_json::to_string(unsigned_tx)
            .unwrap_or_default()
            .as_bytes(),
    );
    format!("0x02{}", hex::encode(h.finalize()))
}

/// A deterministic synthetic tx hash for dev mode (no chain).
fn synthetic_hash(content_id: &str, subject: &str, now_unix: u64) -> String {
    let mut h = Sha256::new();
    h.update(b"elastos-ddrm/buy-dev/v1");
    h.update(content_id.as_bytes());
    h.update(subject.as_bytes());
    h.update(now_unix.to_le_bytes());
    format!("0x{}", hex::encode(h.finalize()))
}

/// In dev mode the subject may be empty (no linked wallet). Use the same stable
/// placeholder the dev rights attestation derives, so the dev ledger entry would match
/// were dev mode ever to consult it.
fn dev_subject(principal_id: &str, subject: &str) -> String {
    if subject.trim().is_empty() {
        let digest = Sha256::digest(format!("elastos-dev-subject:{principal_id}").as_bytes());
        format!("0x{}", hex::encode(&digest[..20]))
    } else {
        subject.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUBJECT: &str = "0x00000000000000000000000000000000000000bb";

    // The dev buy loop (free ownership ledger, no on-chain payment) is a `dev-modes`-only path
    // now that rights_mode() defaults to Chain (DEV_MODE_GUARD_SPEC): without `dev-modes`, an
    // unset ELASTOS_DDRM_RIGHTS resolves to Chain, so this dev-ledger flow is unreachable.
    #[test]
    #[cfg(feature = "dev-modes")]
    fn dev_buy_records_ownership_and_returns_hash() {
        let _g = crate::api::ddrm_env_lock();
        let dir = std::env::temp_dir().join(format!("buy-dev-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("ELASTOS_DDRM_OWNED_LEDGER", dir.join("owned.json"));
        std::env::remove_var("ELASTOS_DDRM_RIGHTS"); // dev

        let out = buy_access(
            "did:test:alice",
            "bafyDEV",
            SUBJECT,
            1_700_000_000,
            &BuyTarget::default(),
        )
        .expect("dev buy");
        assert!(out.owned_now);
        assert!(out.tx_hash.starts_with("0x"));
        assert!(super::super::owned_ledger::contains("bafyDEV", SUBJECT));

        std::env::remove_var("ELASTOS_DDRM_OWNED_LEDGER");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mock_intent_is_well_formed_for_the_wallet_capsule() {
        let _g = crate::api::ddrm_env_lock();
        // No pinned overrides -> `to` defaults to the real AuthorityGateway and the
        // calldata uses the real ERC-20 `buyAccess` selector (USDC default).
        for k in [
            "ELASTOS_DDRM_BUY_TO",
            "ELASTOS_DDRM_BUY_LEDGER",
            "ELASTOS_DDRM_BUY_SELLER",
            "ELASTOS_DDRM_BUY_QUANTITY",
            "ELASTOS_DDRM_BUY_PRICE",
            "ELASTOS_DDRM_BUY_PAYTOKEN",
            "ELASTOS_DDRM_BUY_TOKEN_ID",
        ] {
            std::env::remove_var(k);
        }

        let from = "0x00000000000000000000000000000000000000bb";
        let intent = mock_transaction_intent(from, "bafyX", 8453);

        // Exactly the fields wallet-provider's `validate_eip155_transaction_intent_payload`
        // requires (schema/type/intent/approval/chain_id/from/to/quantities/data).
        assert_eq!(
            intent["schema"],
            "elastos.chain.unsigned_transaction_intent/v1"
        );
        assert_eq!(intent["transaction_type"], "eip155_legacy");
        assert_eq!(intent["wallet_intent"], "transaction_intent");
        assert_eq!(intent["requires_wallet_approval"], true);
        assert_eq!(intent["from"], from);
        assert_eq!(intent["to"], BASE_AUTHORITY_GATEWAY);
        assert_eq!(intent["chain_id"], 8453);
        let to = intent["to"].as_str().unwrap().trim_start_matches("0x");
        assert!(to.len() == 40 && to.chars().all(|c| c.is_ascii_hexdigit()));
        // Real ERC-20 buyAccess selector (default USDC payment).
        assert!(intent["data"].as_str().unwrap().starts_with("0x0ede2294"));
        assert!(intent["gas_price"].as_str().unwrap().starts_with("0x"));
    }

    #[test]
    fn chain_buy_without_wallet_fails_closed() {
        let _g = crate::api::ddrm_env_lock();
        std::env::set_var("ELASTOS_DDRM_RIGHTS", "chain");
        let result = buy_access(
            "did:test:nowallet",
            "bafyX",
            "",
            1_700_000_000,
            &BuyTarget::default(),
        );
        std::env::remove_var("ELASTOS_DDRM_RIGHTS");
        let err = result.expect_err("chain buy with no wallet must error");
        assert!(err.contains("wallet not linked"), "unexpected error: {err}");
    }

    fn clear_buy_env() {
        for k in [
            "ELASTOS_DDRM_BUY_TO",
            "ELASTOS_DDRM_BUY_LEDGER",
            "ELASTOS_DDRM_BUY_SELLER",
            "ELASTOS_DDRM_BUY_QUANTITY",
            "ELASTOS_DDRM_BUY_PRICE",
            "ELASTOS_DDRM_BUY_PAYTOKEN",
            "ELASTOS_DDRM_BUY_TOKEN_ID",
            "ELASTOS_DDRM_BUY_PAYMENT_PROCESSOR",
        ] {
            std::env::remove_var(k);
        }
    }

    #[test]
    fn native_value_is_price_times_quantity() {
        let _g = crate::api::ddrm_env_lock();
        clear_buy_env();
        std::env::set_var("ELASTOS_DDRM_BUY_PRICE", "100");
        std::env::set_var("ELASTOS_DDRM_BUY_QUANTITY", "3");
        std::env::set_var("ELASTOS_DDRM_BUY_PAYTOKEN", "native");
        let tx = assemble_buy_tx("bafyX", SUBJECT, None);
        // value = 100 * 3 = 300 = 0x12c (the *quantity multiplier the Phase-1 fix adds).
        assert_eq!(tx["value"], "0x12c");
        assert!(tx["data"].as_str().unwrap().starts_with("0xf7580ad9")); // native selector
        clear_buy_env();
    }

    #[test]
    fn erc20_path_emits_approve_to_payment_processor() {
        let _g = crate::api::ddrm_env_lock();
        clear_buy_env();
        let processor = "0x1111111111111111111111111111111111111111";
        std::env::set_var("ELASTOS_DDRM_BUY_PRICE", "1000000"); // 1 USDC (6 dp)
        std::env::set_var("ELASTOS_DDRM_BUY_QUANTITY", "2");
        std::env::set_var("ELASTOS_DDRM_BUY_PAYMENT_PROCESSOR", processor);
        let tx = assemble_buy_tx("bafyX", SUBJECT, None);
        assert_eq!(tx["value"], "0x0"); // ERC-20 path pays via approve, not value
        let approve = &tx["approve"];
        assert_eq!(approve["to"], BASE_USDC); // approve is on the pay-token
        assert_eq!(approve["selector"], "0x095ea7b3");
        assert_eq!(approve["spender"], processor); // spender = paymentProcessor, NOT the gateway
        assert_ne!(approve["spender"], BASE_AUTHORITY_GATEWAY);
        clear_buy_env();
    }

    #[test]
    fn chain_buy_without_resolved_tokenid_fails_closed() {
        let _g = crate::api::ddrm_env_lock();
        clear_buy_env();
        std::env::set_var("ELASTOS_DDRM_RIGHTS", "chain");
        std::env::set_var("ELASTOS_DDRM_BUY_SIGN", "wallet"); // pass the top wallet-linked check
                                                              // A channel/ledger but NO pinned tokenId -> the Chain arm must resolve KID->tokenId and, with no
                                                              // live chain in the unit test, fail closed (never falling back to word_from_id).
        std::env::set_var(
            "ELASTOS_DDRM_BUY_LEDGER",
            "0x807f9eb55a165c2daa74a5baefc6f47324a2825d",
        );
        let err = buy_access(
            "did:test:alice",
            "bafyX",
            SUBJECT,
            1_700_000_000,
            &BuyTarget::default(),
        )
        .expect_err("live buy without a resolved tokenId must fail closed");
        assert!(
            err.contains("resolved on-chain tokenId"),
            "unexpected error: {err}"
        );
        std::env::remove_var("ELASTOS_DDRM_RIGHTS");
        std::env::remove_var("ELASTOS_DDRM_BUY_SIGN");
        clear_buy_env();
    }

    #[test]
    fn ensure_no_drift_passes_on_match_and_fails_on_change() {
        let bound = BoundTerms {
            seller: "0xAbC".to_string(),
            token_id: "0x07".to_string(),
            price: "1000000".to_string(),
            pay_token: BASE_USDC.to_string(),
        };
        // Identical (address/hex case-insensitive) -> Ok.
        let mut same = bound.clone();
        same.seller = "0xabc".to_string();
        assert!(ensure_no_drift(&bound, &same).is_ok());
        // Price drift -> fail closed, naming the field.
        let mut drift = bound.clone();
        drift.price = "2000000".to_string();
        let err = ensure_no_drift(&bound, &drift).expect_err("price drift must abort");
        assert!(
            err.contains("price"),
            "error should name the drifted field: {err}"
        );
        // Pay-token drift -> fail closed.
        let mut pt = bound.clone();
        pt.pay_token = "0x0000000000000000000000000000000000000000".to_string();
        assert!(ensure_no_drift(&bound, &pt).is_err());
    }

    #[test]
    fn decode_listing_return_reads_qty_price_paytoken_and_fails_closed() {
        // listings() SSOT layout -> (qty = 5, pricePerToken = 1_000_000, payToken = USDC). Three ABI words.
        let price = format!("{:064x}", 1_000_000u128);
        let qty = format!("{:064x}", 5u128);
        let pay = format!("{:0>64}", &BASE_USDC[2..].to_lowercase());
        let result = format!("0x{qty}{price}{pay}");
        let token_id_word = format!("{:064x}", 7u128);
        let (terms, supply) = decode_listing_return(&result, "0xseller", &token_id_word)
            .expect("valid listings decode");
        assert_eq!(terms.price, "1000000");
        assert_eq!(supply, 5);
        assert_eq!(terms.pay_token.to_lowercase(), BASE_USDC.to_lowercase());
        assert_eq!(terms.token_id, format!("0x{token_id_word}"));
        // Truncated return -> fail closed.
        assert!(decode_listing_return("0x1234", "0xseller", &token_id_word).is_err());
        // A uint beyond u128 (high word set) -> fail closed.
        let huge = format!("{}{price}{pay}", "f".repeat(64));
        assert!(decode_listing_return(&format!("0x{huge}"), "0xseller", &token_id_word).is_err());
        // A non-hex multibyte body that straddles the word-0 slice boundary (byte 64 mid-`é`) must
        // fail closed, NOT panic on a `&str` non-char-boundary slice (compromised-RPC hardening).
        let straddle = format!("0x{}\u{e9}{}", "a".repeat(63), "a".repeat(127));
        assert!(decode_listing_return(&straddle, "0xseller", &token_id_word).is_err());
    }

    #[test]
    fn listings_read_is_keyed_at_access_token_id_one_not_content_tokenid() {
        // The AuthorityGateway keys listings/sellersOf at the ACCESS_TOKEN sub-id (==1), NOT the asset's
        // content tokenId (confirmed live on Base: a content-tokenId read returns an empty slot). A
        // regression here makes every live buy abort (empty re-read => drift/sold-out) and /get show no
        // price. Words: selector | operative | tokenId | seller.
        let op = "0x8b0ae79abf9b41dfe8aabf3c791dd52fe7713530";
        let seller = "0x34daf31b99b5a59ceb18e424dbc112fa6e5f3dc3";
        let body = encode_listings(op, seller);
        let body = body.trim_start_matches("0x");
        assert_eq!(&body[..8], "6bd3a64b", "listings selector");
        let token_word = &body[8 + 64..8 + 128];
        assert_eq!(
            token_word, ACCESS_TOKEN_ID_WORD,
            "listings must be keyed at ACCESS_TOKEN id=1, not the content tokenId"
        );
        assert!(body.contains(&op[2..].to_lowercase()), "operative present");
        assert!(body.contains(&seller[2..].to_lowercase()), "seller present");
    }

    #[test]
    fn assemble_from_live_terms_needs_no_env_and_binds_terms() {
        let _g = crate::api::ddrm_env_lock();
        clear_buy_env(); // the live path assembles from explicit terms — NO ELASTOS_DDRM_BUY_* required.
        let terms = BuyTerms {
            gateway: BASE_AUTHORITY_GATEWAY.to_string(),
            ledger: "0x807f9eb55a165c2daa74a5baefc6f47324a2825d".to_string(),
            seller: "0x34daf31b99b5a59ceb18e424dbc112fa6e5f3dc3".to_string(),
            token_id_word: format!("{:064x}", 7u128),
            quantity: "2".to_string(),
            price: "1000000".to_string(),
            pay_token: BASE_USDC.to_string(),
        };
        let tx = assemble_buy_tx_core("bafyLIVE", SUBJECT, &terms);
        assert!(tx["data"].as_str().unwrap().starts_with("0x0ede2294")); // ERC-20 buyAccess
        assert_eq!(tx["value"], "0x0"); // ERC-20 pays via approve, not value
        assert_eq!(tx["ledger"], terms.ledger);
        assert_eq!(tx["seller"], terms.seller);
        let data = tx["data"].as_str().unwrap().to_lowercase();
        assert!(
            data.contains(&terms.seller[2..].to_lowercase()),
            "calldata binds seller"
        );
        assert!(
            data.contains(&terms.ledger[2..].to_lowercase()),
            "calldata binds ledger"
        );
        assert!(
            data.contains(&terms.token_id_word),
            "calldata binds content tokenId"
        );
        clear_buy_env();
    }

    #[test]
    fn source_buy_terms_fails_closed_without_channel_ledger() {
        let _g = crate::api::ddrm_env_lock();
        clear_buy_env(); // no target, no env -> the very first gate (channel/ledger) fails closed.
        let err = source_buy_terms("bafyX", &BuyTarget::default())
            .expect_err("a live buy with no channel/ledger must fail closed");
        assert!(err.contains("channel/ledger"), "unexpected error: {err}");
        clear_buy_env();
    }

    /// DEV INTEGRATION (opt-in): proves the offline buy->own->open ledger loop end to end
    /// against the REAL chain-provider broadcast op + the chain-mock rights read. Requires
    /// the dev-tree chain-provider binary:
    ///   cargo build --manifest-path capsules/chain-provider/Cargo.toml
    /// Run with: cargo test -p elastos-server chain_mock_buy -- --ignored
    #[test]
    #[ignore]
    fn chain_mock_buy_records_then_ledger_reads_owned() {
        let _g = crate::api::ddrm_env_lock();
        let dir = std::env::temp_dir().join(format!("buy-mock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("ELASTOS_DDRM_OWNED_LEDGER", dir.join("owned.json"));
        std::env::set_var("ELASTOS_DDRM_RIGHTS", "chain-mock");

        // Not owned before the buy.
        assert!(!super::super::owned_ledger::contains("bafyBUY", SUBJECT));
        let out = buy_access(
            "did:test:alice",
            "bafyBUY",
            SUBJECT,
            1_700_000_000,
            &BuyTarget::default(),
        )
        .expect("chain-mock buy");
        assert!(out.tx_hash.starts_with("0x") && out.tx_hash.len() == 66);
        // Owned after the buy — the ledger the chain-mock rights read consults.
        assert!(super::super::owned_ledger::contains("bafyBUY", SUBJECT));

        std::env::remove_var("ELASTOS_DDRM_RIGHTS");
        std::env::remove_var("ELASTOS_DDRM_OWNED_LEDGER");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// DEV INTEGRATION (opt-in): THE headline loop — in `chain-mock` + ledger-gated rights,
    /// the rights gate DENIES an unowned object, the buy records ownership, and the gate
    /// then ALLOWS it. Drives the REAL chain-provider + rights-provider. Requires:
    ///   cargo build --manifest-path capsules/chain-provider/Cargo.toml
    ///   cargo build --manifest-path capsules/rights-provider/Cargo.toml --features chain-rights
    /// Run with: cargo test -p elastos-server buy_then_open_loop -- --ignored
    #[test]
    #[ignore]
    fn buy_then_open_loop_flips_rights_from_denied_to_allowed() {
        let _g = crate::api::ddrm_env_lock();
        let dir = std::env::temp_dir().join(format!("buy-loop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("ELASTOS_DDRM_OWNED_LEDGER", dir.join("owned.json"));
        std::env::set_var("ELASTOS_DDRM_RIGHTS", "chain-mock");
        // The mock answers ownership from the local ledger (the buy-flow gate).
        std::env::set_var("ELASTOS_DDRM_CHAIN_ACCESS", "ledger");

        let cid = "bafyLOOP";
        let decide = || {
            super::super::rights_authority::decide_owned_access(
                "did:test:alice",
                "s1",
                cid,
                SUBJECT,
                "view",
                "render",
                None,
                1_700_000_000,
                900,
            )
        };

        // Before the buy: not in the ledger -> rights gate DENIES (fail closed).
        let before = decide().expect("rights decision (before)");
        assert!(!before.allowed, "unowned content must be denied before buy");

        // Buy the access token (real broadcast + ledger record).
        let out = buy_access(
            "did:test:alice",
            cid,
            SUBJECT,
            1_700_000_000,
            &BuyTarget::default(),
        )
        .expect("buy");
        assert!(out.owned_now);

        // After the buy: ledger has it -> rights gate ALLOWS.
        let after = decide().expect("rights decision (after)");
        assert!(after.allowed, "content must be allowed after buy");

        std::env::remove_var("ELASTOS_DDRM_CHAIN_ACCESS");
        std::env::remove_var("ELASTOS_DDRM_RIGHTS");
        std::env::remove_var("ELASTOS_DDRM_OWNED_LEDGER");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// DEV INTEGRATION (opt-in): proves the REAL signing rail offline — the wallet capsule
    /// signs a buyAccess tx with a managed secp256k1 key (key never leaves the capsule) and
    /// the genuine signed bytes are broadcast through the REAL chain-provider against the
    /// in-process RPC mock. Ownership is recorded under the recovered signer. Requires the
    /// dev-tree wallet-provider + chain-provider binaries:
    ///   cargo build --manifest-path capsules/wallet-provider/Cargo.toml
    ///   cargo build --manifest-path capsules/chain-provider/Cargo.toml
    /// Run with: cargo test -p elastos-server chain_mock_wallet_signs -- --ignored
    #[test]
    #[ignore]
    fn chain_mock_wallet_signs_and_broadcasts_real_tx() {
        let _g = crate::api::ddrm_env_lock();
        let dir = std::env::temp_dir().join(format!("buy-wallet-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("ELASTOS_DDRM_OWNED_LEDGER", dir.join("owned.json"));
        std::env::set_var("ELASTOS_DDRM_WALLET_BASE", dir.join("wallet"));
        std::env::set_var("ELASTOS_DDRM_RIGHTS", "chain-mock");
        std::env::set_var("ELASTOS_DDRM_BUY_SIGN", "wallet");
        // No external wallet linked — the managed account is authoritative.
        let out = buy_access(
            "did:test:alice",
            "bafyWALLET",
            "",
            1_700_000_000,
            &BuyTarget::default(),
        )
        .expect("wallet-signed chain-mock buy");

        assert_eq!(out.mode, "chain-mock+wallet");
        assert!(out.owned_now);
        // A real, broadcast tx hash (mock-echoed) and a recovered managed signer address.
        assert!(out.tx_hash.starts_with("0x") && out.tx_hash.len() == 66);
        let signer = out.unsigned_tx["signer"]
            .as_str()
            .expect("signer in audit view");
        let hex = signer.trim_start_matches("0x");
        assert!(hex.len() == 40 && hex.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(out.unsigned_tx["signed_tx_hash"]
            .as_str()
            .unwrap()
            .starts_with("0x"));
        // Ownership recorded under the signer (the authoritative buyer).
        assert!(super::super::owned_ledger::contains("bafyWALLET", signer));

        std::env::remove_var("ELASTOS_DDRM_BUY_SIGN");
        std::env::remove_var("ELASTOS_DDRM_RIGHTS");
        std::env::remove_var("ELASTOS_DDRM_WALLET_BASE");
        std::env::remove_var("ELASTOS_DDRM_OWNED_LEDGER");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
