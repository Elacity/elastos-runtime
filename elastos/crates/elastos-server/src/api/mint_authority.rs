//! Gateway-side content MINT orchestration (PC2 "Create" stage 2 — put the asset on chain).
//!
//! Mirrors `buy_authority`, on the SAME signing rail: it (1) assembles the real
//! `mint(string,uint16,bytes,bytes)` calldata via the REAL `chain-provider.assemble_mint`
//! (pure — no RPC, no keys; byte-for-byte the PC2 `opRawData`/`sellRawData` layout), (2)
//! signs it with a managed account whose key NEVER leaves `wallet-provider`, and (3)
//! broadcasts the genuine signed bytes through the REAL `chain-provider`.
//!
//! Modes follow `ELASTOS_DDRM_RIGHTS` (same switch the rights gate + buy use):
//! - `dev` — assemble calldata only, synthetic tx hash (no chain, no signing).
//! - `chain-mock` — REAL wallet signature + REAL broadcast op against an in-process RPC
//!   mock. Proves the full sign→broadcast mint rail on a Mac, offline.
//! - `chain` — source real nonce/gas via `prepare_transaction`, sign, broadcast to the
//!   configured Base RPC (production).
//!
//! The minter (`from`) is a managed account; mint records `msg.sender` as the creator, so
//! the managed account's address IS the on-chain creator. No external `subject` to
//! reconcile (unlike buy).

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::rights_authority::{env_nonempty, resolve_chain_bin, run_chain_capsule, RightsMode};

/// Real Base selector for `mint(string,uint16,bytes,bytes)`
/// (`keccak256("mint(string,uint16,bytes,bytes)")[..4]`); overridable for other deployments.
/// Supplied (not computed) to the pure assembler, exactly like the `has_access` selector.
const MINT_SELECTOR: &str = "0x47cbeeb4";

/// Default offline fees for the `chain-mock` wallet-signing path (no RPC to source them).
/// The mock does not execute the transaction, so these only need to be well-formed; the
/// gas limit is generous because a content mint is heavier than a token transfer/buy.
const MOCK_NONCE: &str = "0x0";
const MOCK_GAS_PRICE: &str = "0x3b9aca00"; // 1 gwei
const MOCK_GAS_LIMIT: &str = "0x7a120"; // 500k

/// The outcome of a content-mint orchestration. Carries no key material.
#[derive(Debug)]
pub struct MintOutcome {
    /// The broadcast (or synthetic) transaction hash.
    pub tx_hash: String,
    /// Which path produced it (`dev` / `chain-mock+wallet` / `chain+wallet`).
    pub mode: String,
    /// The Channel/DigitalAsset contract the mint was sent to.
    pub to: String,
    /// The on-chain `bytes16 contentId` minted (the KID the rights gate later keys on).
    pub content_id: String,
    /// The recovered managed-account creator address (None in `dev`).
    pub signer: Option<String>,
    /// Non-secret audit view of the assembled call + signer (no key material).
    pub assembled: Value,
}

/// Mint a dDRM content asset. `mint` is a publish-provider `UnsignedMintV1` shaped as the
/// chain-provider `MintAssembly` (`to`, `token_uri`, `op_type_code`, `content_id`,
/// optional `value_wei` / `op_raw` / `sell`); the mint `selector` is pinned here when the
/// caller didn't supply one. `principal_id` owns the managed minting account.
pub fn mint_asset(principal_id: &str, mint: Value, now_unix: u64) -> Result<MintOutcome, String> {
    let mode = super::rights_authority::rights_mode();

    // Assemble the real mint calldata first (pure step, all modes). This also validates the
    // mint shape (selector, `to`, content_id, op/sell terms) and fails closed if malformed.
    let assembled = assemble_calldata(mint)?;
    let to = assembled
        .get("to")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let value = assembled
        .get("value")
        .and_then(Value::as_str)
        .unwrap_or("0x0")
        .to_string();
    let data = assembled
        .get("data")
        .and_then(Value::as_str)
        .unwrap_or("0x")
        .to_string();
    let content_id = assembled
        .get("content_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    match mode {
        RightsMode::Dev => Ok(MintOutcome {
            tx_hash: synthetic_hash(&to, &content_id, now_unix),
            mode: "dev".to_string(),
            to,
            content_id,
            signer: None,
            assembled,
        }),
        RightsMode::ChainMock => {
            // REAL signing, offline: the wallet capsule signs the well-formed mint tx with a
            // managed key, and the REAL chain-provider broadcast op sends the genuine signed
            // bytes through the in-process RPC mock. Proves the full mint sign→broadcast rail
            // on a Mac with no network.
            let chain_id = chain_id_default();
            let mut intent_seen = Value::Null;
            let sig =
                super::wallet_signer::sign_with_managed_account(principal_id, chain_id, |from| {
                    let intent = mock_transaction_intent(from, &to, &value, &data, chain_id);
                    intent_seen = intent.clone();
                    Ok(intent)
                })?;
            let tx_hash =
                super::chain_tx::broadcast_signed_mock(&intent_seen, &sig.signed_transaction)?;
            Ok(MintOutcome {
                tx_hash,
                mode: "chain-mock+wallet".to_string(),
                to,
                content_id,
                signer: Some(sig.signer.clone()),
                assembled: mint_audit_view(&intent_seen, &sig),
            })
        }
        RightsMode::Chain => {
            // Live: source real nonce/gas + assemble the intent via the REAL chain-provider
            // `prepare_transaction`, sign inside the wallet capsule (key never leaves), and
            // broadcast the signed bytes through the REAL chain-provider.
            let chain_id = chain_id_default();
            let mut intent_seen = Value::Null;
            let sig =
                super::wallet_signer::sign_with_managed_account(principal_id, chain_id, |from| {
                    let intent = super::chain_tx::prepare_intent_live(from, &to, &value, &data)?;
                    intent_seen = intent.clone();
                    Ok(intent)
                })?;
            let tx_hash = super::chain_tx::broadcast_signed_live(&sig.signed_transaction)?;
            Ok(MintOutcome {
                tx_hash,
                mode: "chain+wallet".to_string(),
                to,
                content_id,
                signer: Some(sig.signer.clone()),
                assembled: mint_audit_view(&intent_seen, &sig),
            })
        }
    }
}

/// Assemble the real mint calldata via the REAL `chain-provider.assemble_mint` (pure: no
/// RPC, no keys). Pins the configured mint selector when the caller didn't supply one.
/// Returns the `{ to, data, value, content_id, … }` the signer/broadcaster consume.
fn assemble_calldata(mut mint: Value) -> Result<Value, String> {
    if !mint.is_object() {
        return Err("mint must be a JSON object (chain-provider MintAssembly shape)".to_string());
    }
    let has_selector = mint
        .get("selector")
        .and_then(Value::as_str)
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    if !has_selector {
        let selector =
            env_nonempty("ELASTOS_DDRM_MINT_SELECTOR").unwrap_or_else(|| MINT_SELECTOR.to_string());
        mint["selector"] = json!(selector);
    }

    let chain_bin = resolve_chain_bin();
    if !std::path::Path::new(&chain_bin).is_file() {
        return Err(format!(
            "chain-provider not found at {chain_bin}; build it with \
             `cargo build --manifest-path capsules/chain-provider/Cargo.toml` \
             or set ELASTOS_CHAIN_PROVIDER_BIN"
        ));
    }
    // `assemble_mint` is pure and touches no network, so an empty init keeps the defaults.
    let init = json!({ "op": "init", "config": {} });
    let op = json!({ "op": "assemble_mint", "mint": mint });
    run_chain_capsule(&chain_bin, &init, &op)
}

/// The EVM chain id for the mint (default Base mainnet); overridable for other deployments.
fn chain_id_default() -> u64 {
    env_nonempty("ELASTOS_DDRM_CHAIN_ID")
        .and_then(|s| s.parse().ok())
        .unwrap_or(8453)
}

/// Assemble the offline (`chain-mock`) `unsigned_transaction_intent/v1` the wallet capsule
/// signs. Identical schema to `chain-provider.prepare_transaction`, but the fees are
/// well-formed constants (the mock never executes the call). `to` is the assembled,
/// already-validated Channel address.
fn mock_transaction_intent(from: &str, to: &str, value: &str, data: &str, chain_id: u64) -> Value {
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

/// A non-secret audit view of a wallet-signed mint: the assembled call plus the recovered
/// creator (signer) and the signed-tx hash. Carries no key material.
fn mint_audit_view(intent: &Value, sig: &super::wallet_signer::ManagedSignature) -> Value {
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

/// A deterministic synthetic tx hash for dev mode (no chain).
fn synthetic_hash(to: &str, content_id: &str, now_unix: u64) -> String {
    let mut h = Sha256::new();
    h.update(b"elastos-ddrm/mint-dev-tx/v1");
    h.update(to.as_bytes());
    h.update(content_id.as_bytes());
    h.update(now_unix.to_le_bytes());
    format!("0x{}", hex::encode(h.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// A free-mint MintAssembly (the simplest real mint: opRawData = abi.encode(bytes16),
    /// no sale terms). content_id is a 16-byte KID, hex-prefixed.
    fn free_mint() -> Value {
        json!({
            "to": "0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D",
            "token_uri": "QmMetaFolderCidV0/metadata.json",
            "op_type_code": 0,
            "content_id": "0x38691296765e76a331f5d5630bddf9f5",
        })
    }

    #[test]
    fn mint_intent_is_well_formed_for_the_wallet_capsule() {
        let from = "0x00000000000000000000000000000000000000bb";
        let to = "0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D";
        let intent = mock_transaction_intent(from, to, "0x0", "0x47cbeeb4", 8453);

        // Exactly the fields wallet-provider's eip155 intent validator requires.
        assert_eq!(
            intent["schema"],
            "elastos.chain.unsigned_transaction_intent/v1"
        );
        assert_eq!(intent["transaction_type"], "eip155_legacy");
        assert_eq!(intent["wallet_intent"], "transaction_intent");
        assert_eq!(intent["requires_wallet_approval"], true);
        assert_eq!(intent["from"], from);
        assert_eq!(intent["to"], to);
        assert_eq!(intent["chain_id"], 8453);
        assert!(intent["data"].as_str().unwrap().starts_with("0x"));
        assert!(intent["gas_price"].as_str().unwrap().starts_with("0x"));
        assert!(intent["gas_limit"].as_str().unwrap().starts_with("0x"));
    }

    #[test]
    fn mint_rejects_a_non_object_assembly() {
        let err = assemble_calldata(json!("not-an-object")).expect_err("must reject");
        assert!(err.contains("JSON object"), "unexpected error: {err}");
    }

    /// DEV INTEGRATION (opt-in): proves the REAL mint signing rail offline — the wallet
    /// capsule signs the real `mint(...)` calldata with a managed secp256k1 key (key never
    /// leaves the capsule) and the genuine signed bytes are broadcast through the REAL
    /// chain-provider against the in-process RPC mock. Requires the dev-tree binaries:
    ///   cargo build --manifest-path capsules/wallet-provider/Cargo.toml
    ///   cargo build --manifest-path capsules/chain-provider/Cargo.toml
    /// Run with: cargo test -p elastos-server chain_mock_mint -- --ignored
    #[test]
    #[ignore]
    fn chain_mock_mint_signs_and_broadcasts_real_tx() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("mint-wallet-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("ELASTOS_DDRM_WALLET_BASE", dir.join("wallet"));
        std::env::set_var("ELASTOS_DDRM_RIGHTS", "chain-mock");

        let out = mint_asset("did:test:creator", free_mint(), 1_700_000_000)
            .expect("wallet-signed chain-mock mint");

        assert_eq!(out.mode, "chain-mock+wallet");
        // The assembled calldata leads with the real mint selector.
        assert!(out.assembled["data"]
            .as_str()
            .unwrap()
            .starts_with("0x47cbeeb4"));
        // A real, broadcast tx hash (mock-echoed) and a recovered managed creator address.
        assert!(out.tx_hash.starts_with("0x") && out.tx_hash.len() == 66);
        let signer = out.signer.expect("managed signer");
        let hex = signer.trim_start_matches("0x");
        assert!(hex.len() == 40 && hex.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(out.content_id, "0x38691296765e76a331f5d5630bddf9f5");

        std::env::remove_var("ELASTOS_DDRM_RIGHTS");
        std::env::remove_var("ELASTOS_DDRM_WALLET_BASE");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
