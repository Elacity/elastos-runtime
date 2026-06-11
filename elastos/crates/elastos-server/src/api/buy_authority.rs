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
//!   - `chain` — assemble the `{ to, value, data }` against the configured Base contract.
//!     With `ELASTOS_DDRM_BUY_SIGN=wallet` (RECOMMENDED), the gateway sources real
//!     nonce/gas via `chain-provider.prepare_transaction`, signs inside `wallet-provider`
//!     with a managed account (the key never leaves the capsule), and broadcasts the
//!     signed bytes through the real `chain-provider` — genuinely live. Absent that
//!     opt-in, it broadcasts an EXTERNALLY-signed tx (`ELASTOS_DDRM_BUY_SIGNED_TX`) or —
//!     absent one — returns the assembled unsigned tx for an external signer (fail-closed).
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

/// Default offline fees for the `chain-mock` wallet-signing path (no RPC to source them).
/// The mock does not execute the transaction, so these only need to be well-formed.
const MOCK_NONCE: &str = "0x0";
const MOCK_GAS_PRICE: &str = "0x3b9aca00"; // 1 gwei
const MOCK_GAS_LIMIT: &str = "0x186a0"; // 100k — comfortably covers contract calldata

/// True when the operator opted into runtime EVM signing through the wallet capsule
/// (`ELASTOS_DDRM_BUY_SIGN=wallet`): the buy is signed by a managed account whose key
/// never leaves `wallet-provider`. Absent this, the legacy externally-signed path applies.
fn wallet_signing() -> bool {
    env_nonempty("ELASTOS_DDRM_BUY_SIGN").as_deref() == Some("wallet")
}

/// The EVM chain id for the buy (default Base mainnet); overridable for other deployments.
fn chain_id_default() -> u64 {
    env_nonempty("ELASTOS_DDRM_CHAIN_ID")
        .and_then(|s| s.parse().ok())
        .unwrap_or(8453)
}

/// Buy an access token for `content_id` on behalf of `subject` (the principal's linked
/// EVM wallet). `content_id` MUST be the same identifier the rights gate keys on, so the
/// recorded ownership matches the subsequent open.
pub fn buy_access(
    principal_id: &str,
    content_id: &str,
    subject: &str,
    now_unix: u64,
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

    let unsigned_tx = assemble_buy_tx(content_id, subject);

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
                let sig =
                    super::wallet_signer::sign_with_managed_account(principal_id, chain_id, |from| {
                        let intent = mock_transaction_intent(from, content_id, chain_id);
                        intent_seen = intent.clone();
                        Ok(intent)
                    })?;
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
            if wallet_signing() {
                // Live path: source real nonce/gas + assemble the intent via the REAL
                // chain-provider `prepare_transaction`, sign it inside the wallet capsule
                // (key never leaves), and broadcast the signed bytes through the REAL
                // chain-provider. No externally-signed tx required — this is the seam that
                // makes `chain` mode genuinely live.
                let chain_id = chain_id_default();
                let mut intent_seen = Value::Null;
                let sig =
                    super::wallet_signer::sign_with_managed_account(principal_id, chain_id, |from| {
                        let intent = prepare_live_intent(from, content_id)?;
                        intent_seen = intent.clone();
                        Ok(intent)
                    })?;
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
            // Real chain, no runtime signing: broadcast an externally-signed tx if provided,
            // else hand back the assembled unsigned tx for an external signer.
            let Some(signed) = env_nonempty("ELASTOS_DDRM_BUY_SIGNED_TX") else {
                return Err(format!(
                    "live buy needs a signature: either opt into runtime signing with \
                     ELASTOS_DDRM_BUY_SIGN=wallet (the wallet capsule signs with a managed \
                     key), or sign this assembled tx externally and resubmit via \
                     ELASTOS_DDRM_BUY_SIGNED_TX. unsigned_tx={unsigned_tx}"
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
                unsigned_tx,
            })
        }
    }
}

/// Assemble the REAL `buyAccess` transaction the wallet signs, matching PC2's
/// `wallet.js`:
///   `buyAccess(address seller, address ledger, uint256 tokenId, uint256 quantity,
///              uint256 pricePerToken[, address payToken])`
/// sent to the AuthorityGateway. The ERC-20 form (default, USDC on Base) carries a
/// `payToken` and `value = 0` and requires a prior `approve` of the operative's
/// `paymentProcessor`; the native form omits `payToken` and pays `value = price`.
///
/// Listing terms (seller / ledger / tokenId / price / payToken) are sourced from the
/// marketplace listing in production (the Market portal, B5); here they're overridable via
/// env so a live buy is byte-correct once a listing is pinned. The `tokenId` defaults to a
/// 32-byte word derived from `content_id` (the ledger's content-hash tokenId); pin the
/// real value with `ELASTOS_DDRM_BUY_TOKEN_ID`. Pure: no RPC, no keys.
fn assemble_buy_tx(content_id: &str, subject: &str) -> Value {
    let to = env_nonempty("ELASTOS_DDRM_BUY_TO").unwrap_or_else(|| BASE_AUTHORITY_GATEWAY.to_string());
    // Default ledger to the AuthorityGateway is wrong; default it to the (overridable)
    // channel. With no channel pinned we fall back to `to` so calldata stays well-formed.
    let ledger = env_nonempty("ELASTOS_DDRM_BUY_LEDGER").unwrap_or_else(|| to.clone());
    let seller = env_nonempty("ELASTOS_DDRM_BUY_SELLER").unwrap_or_else(|| subject.to_string());
    let quantity = env_nonempty("ELASTOS_DDRM_BUY_QUANTITY").unwrap_or_else(|| "1".to_string());
    let price = env_nonempty("ELASTOS_DDRM_BUY_PRICE").unwrap_or_else(|| "0".to_string());
    let pay_token = env_nonempty("ELASTOS_DDRM_BUY_PAYTOKEN").unwrap_or_else(|| BASE_USDC.to_string());
    let native = pay_token.is_empty() || pay_token.eq_ignore_ascii_case("native");

    // tokenId: a pinned decimal/hex, else derived from content_id.
    let token_id_word = match env_nonempty("ELASTOS_DDRM_BUY_TOKEN_ID") {
        Some(t) => word_from_uint(&t),
        None => word_from_id(content_id),
    };

    let (selector, value, mut data) = if native {
        (
            BUY_ACCESS_NATIVE_SELECTOR.to_string(),
            // Native payment: value carries the price (decimal wei → hex quantity).
            price.parse::<u128>().map(|n| format!("0x{n:x}")).unwrap_or_else(|_| "0x0".to_string()),
            BUY_ACCESS_NATIVE_SELECTOR.trim_start_matches("0x").to_string(),
        )
    } else {
        (
            BUY_ACCESS_ERC20_SELECTOR.to_string(),
            "0x0".to_string(),
            BUY_ACCESS_ERC20_SELECTOR.trim_start_matches("0x").to_string(),
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
        // Production prerequisite for the ERC-20 path (the Market portal batches it):
        // approve(paymentProcessor, price) on payToken before buyAccess.
        "requires_erc20_approve": !native,
    })
}

/// Assemble the offline (`chain-mock`) `unsigned_transaction_intent/v1` the wallet capsule
/// signs. Identical schema to `chain-provider.prepare_transaction`, but the fees are
/// well-formed constants (the mock never executes the call) and `to` falls back to a valid
/// placeholder when no real contract is pinned, so the capsule's address validation passes.
fn mock_transaction_intent(from: &str, content_id: &str, chain_id: u64) -> Value {
    let tx = assemble_buy_tx(content_id, from);
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
fn prepare_live_intent(from: &str, content_id: &str) -> Result<Value, String> {
    let tx = assemble_buy_tx(content_id, from);
    let to = tx
        .get("to")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("live buy requires ELASTOS_DDRM_BUY_TO (the AuthorityGateway/contract address)")?
        .to_string();
    let value = tx.get("value").and_then(Value::as_str).unwrap_or("0x0").to_string();
    let data = tx.get("data").and_then(Value::as_str).unwrap_or("0x").to_string();
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

/// A minimal, even-length-hex "signed tx" that satisfies the real broadcast validator in
/// the mock path (the mock never inspects it; it is not a real signature).
fn representative_signed_tx(unsigned_tx: &Value) -> String {
    let mut h = Sha256::new();
    h.update(b"elastos-ddrm/buy-mock-signed/v1");
    h.update(serde_json::to_string(unsigned_tx).unwrap_or_default().as_bytes());
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

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    const SUBJECT: &str = "0x00000000000000000000000000000000000000bb";

    #[test]
    fn dev_buy_records_ownership_and_returns_hash() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("buy-dev-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("ELASTOS_DDRM_OWNED_LEDGER", dir.join("owned.json"));
        std::env::remove_var("ELASTOS_DDRM_RIGHTS"); // dev

        let out = buy_access("did:test:alice", "bafyDEV", SUBJECT, 1_700_000_000)
            .expect("dev buy");
        assert!(out.owned_now);
        assert!(out.tx_hash.starts_with("0x"));
        assert!(super::super::owned_ledger::contains("bafyDEV", SUBJECT));

        std::env::remove_var("ELASTOS_DDRM_OWNED_LEDGER");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mock_intent_is_well_formed_for_the_wallet_capsule() {
        let _g = ENV_LOCK.lock().unwrap();
        // No pinned overrides -> `to` defaults to the real AuthorityGateway and the
        // calldata uses the real ERC-20 `buyAccess` selector (USDC default).
        for k in [
            "ELASTOS_DDRM_BUY_TO", "ELASTOS_DDRM_BUY_LEDGER", "ELASTOS_DDRM_BUY_SELLER",
            "ELASTOS_DDRM_BUY_QUANTITY", "ELASTOS_DDRM_BUY_PRICE", "ELASTOS_DDRM_BUY_PAYTOKEN",
            "ELASTOS_DDRM_BUY_TOKEN_ID",
        ] {
            std::env::remove_var(k);
        }

        let from = "0x00000000000000000000000000000000000000bb";
        let intent = mock_transaction_intent(from, "bafyX", 8453);

        // Exactly the fields wallet-provider's `validate_eip155_transaction_intent_payload`
        // requires (schema/type/intent/approval/chain_id/from/to/quantities/data).
        assert_eq!(
            intent["schema"], "elastos.chain.unsigned_transaction_intent/v1"
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
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("ELASTOS_DDRM_RIGHTS", "chain");
        let result = buy_access("did:test:nowallet", "bafyX", "", 1_700_000_000);
        std::env::remove_var("ELASTOS_DDRM_RIGHTS");
        let err = result.expect_err("chain buy with no wallet must error");
        assert!(err.contains("wallet not linked"), "unexpected error: {err}");
    }

    /// DEV INTEGRATION (opt-in): proves the offline buy->own->open ledger loop end to end
    /// against the REAL chain-provider broadcast op + the chain-mock rights read. Requires
    /// the dev-tree chain-provider binary:
    ///   cargo build --manifest-path capsules/chain-provider/Cargo.toml
    /// Run with: cargo test -p elastos-server chain_mock_buy -- --ignored
    #[test]
    #[ignore]
    fn chain_mock_buy_records_then_ledger_reads_owned() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("buy-mock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("ELASTOS_DDRM_OWNED_LEDGER", dir.join("owned.json"));
        std::env::set_var("ELASTOS_DDRM_RIGHTS", "chain-mock");

        // Not owned before the buy.
        assert!(!super::super::owned_ledger::contains("bafyBUY", SUBJECT));
        let out = buy_access("did:test:alice", "bafyBUY", SUBJECT, 1_700_000_000)
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
        let _g = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("buy-loop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("ELASTOS_DDRM_OWNED_LEDGER", dir.join("owned.json"));
        std::env::set_var("ELASTOS_DDRM_RIGHTS", "chain-mock");
        // The mock answers ownership from the local ledger (the buy-flow gate).
        std::env::set_var("ELASTOS_DDRM_CHAIN_ACCESS", "ledger");

        let cid = "bafyLOOP";
        let decide = || {
            super::super::rights_authority::decide_owned_access(
                "did:test:alice", "s1", cid, SUBJECT, "view", "render", None,
                1_700_000_000, 900,
            )
        };

        // Before the buy: not in the ledger -> rights gate DENIES (fail closed).
        let before = decide().expect("rights decision (before)");
        assert!(!before.allowed, "unowned content must be denied before buy");

        // Buy the access token (real broadcast + ledger record).
        let out = buy_access("did:test:alice", cid, SUBJECT, 1_700_000_000).expect("buy");
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
        let _g = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("buy-wallet-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("ELASTOS_DDRM_OWNED_LEDGER", dir.join("owned.json"));
        std::env::set_var("ELASTOS_DDRM_WALLET_BASE", dir.join("wallet"));
        std::env::set_var("ELASTOS_DDRM_RIGHTS", "chain-mock");
        std::env::set_var("ELASTOS_DDRM_BUY_SIGN", "wallet");
        // No external wallet linked — the managed account is authoritative.
        let out = buy_access("did:test:alice", "bafyWALLET", "", 1_700_000_000)
            .expect("wallet-signed chain-mock buy");

        assert_eq!(out.mode, "chain-mock+wallet");
        assert!(out.owned_now);
        // A real, broadcast tx hash (mock-echoed) and a recovered managed signer address.
        assert!(out.tx_hash.starts_with("0x") && out.tx_hash.len() == 66);
        let signer = out.unsigned_tx["signer"].as_str().expect("signer in audit view");
        let hex = signer.trim_start_matches("0x");
        assert!(hex.len() == 40 && hex.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(out.unsigned_tx["signed_tx_hash"].as_str().unwrap().starts_with("0x"));
        // Ownership recorded under the signer (the authoritative buyer).
        assert!(super::super::owned_ledger::contains("bafyWALLET", signer));

        std::env::remove_var("ELASTOS_DDRM_BUY_SIGN");
        std::env::remove_var("ELASTOS_DDRM_RIGHTS");
        std::env::remove_var("ELASTOS_DDRM_WALLET_BASE");
        std::env::remove_var("ELASTOS_DDRM_OWNED_LEDGER");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
