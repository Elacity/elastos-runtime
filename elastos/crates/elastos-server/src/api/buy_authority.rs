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
//!   - `chain` — assemble the unsigned `{ to, value, data }` against the configured Base
//!     contract. EVM transaction signing is the same seam the live mint broadcast is
//!     waiting on, so this path broadcasts an EXTERNALLY-signed tx
//!     (`ELASTOS_DDRM_BUY_SIGNED_TX`) through the real chain-provider, or — absent one —
//!     returns the assembled unsigned tx for an external signer (fail-closed, honest).

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::rights_authority::{
    env_nonempty, resolve_chain_bin, run_chain_capsule, ChainRpcMock, RightsMode,
};

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

/// The default demo `buyAccess` selector when none is pinned. NOT a real signature —
/// only a placeholder so the assembled calldata is well-formed in the offline loop.
const DEMO_BUY_SELECTOR: &str = "0xb0a00000";

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
    // rights gate enforces, so a buy can never disagree with the ownership read).
    if matches!(mode, RightsMode::Chain | RightsMode::ChainMock) && subject.trim().is_empty() {
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
            // Real chain: broadcast an externally-signed tx if provided, else hand back
            // the assembled unsigned tx for an external signer (EVM tx signing is the
            // same pending seam as the live mint broadcast — never guessed here).
            let Some(signed) = env_nonempty("ELASTOS_DDRM_BUY_SIGNED_TX") else {
                return Err(format!(
                    "live buy needs a signed transaction: EVM tx signing is the same seam as \
                     the live mint broadcast. Sign this assembled tx externally and resubmit \
                     via ELASTOS_DDRM_BUY_SIGNED_TX. unsigned_tx={unsigned_tx}"
                ));
            };
            let tx_hash = broadcast_live(&signed)?;
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

/// Assemble the `buyAccess` transaction the wallet signs. `to`/`value` come from config
/// (the channel/AuthorityGateway address + the price the listing carries); `data` is the
/// pinned selector followed by the documented demo arg layout. Pure: no RPC, no keys.
fn assemble_buy_tx(content_id: &str, subject: &str) -> Value {
    let selector = env_nonempty("ELASTOS_DDRM_BUY_SELECTOR").unwrap_or_else(|| DEMO_BUY_SELECTOR.to_string());
    let to = env_nonempty("ELASTOS_DDRM_BUY_TO").unwrap_or_default();
    let value = env_nonempty("ELASTOS_DDRM_BUY_VALUE").unwrap_or_else(|| "0x0".to_string());

    // Demo arg layout (documented + overridable; NOT the real ABI): the 4-byte selector,
    // then three 32-byte words — contentId, subject, amount.
    let amount = env_nonempty("ELASTOS_DDRM_BUY_AMOUNT").unwrap_or_else(|| "1".to_string());
    let data = format!(
        "{}{}{}{}",
        selector.trim_start_matches("0x"),
        word_from_id(content_id),
        word_from_address(subject),
        word_from_uint(&amount),
    );
    json!({
        "to": to,
        "value": value,
        "data": format!("0x{data}"),
        "selector": selector,
        "content_id": content_id,
        "subject": subject,
    })
}

/// Broadcast through the REAL chain-provider against an in-process JSON-RPC mock that
/// answers `eth_sendRawTransaction` with a canned 32-byte tx hash. Exercises the real
/// `broadcast_transaction` op (validation + RPC plumbing) with no external network.
fn broadcast_mock(unsigned_tx: &Value) -> Result<String, String> {
    let chain_bin = resolve_chain_bin();
    if !std::path::Path::new(&chain_bin).is_file() {
        return Err(format!(
            "chain-provider not found at {chain_bin}; build it with \
             `cargo build --manifest-path capsules/chain-provider/Cargo.toml` \
             or set ELASTOS_CHAIN_PROVIDER_BIN"
        ));
    }
    // A deterministic, well-formed canned tx hash for the mock to return.
    let tx_word = mock_tx_hash(unsigned_tx);
    let guard = ChainRpcMock::start_with_word(tx_word.clone())?;
    let init = mock_init(&guard.url);
    // The mock ignores calldata; a minimal even-length-hex signed tx satisfies the real
    // `validate_signed_transaction` so the broadcast op actually runs.
    let broadcast = json!({
        "op": "broadcast_transaction",
        "network": "base-local-mock",
        "signed_transaction": representative_signed_tx(unsigned_tx),
    });
    let resp = run_chain_capsule(&chain_bin, &init, &broadcast);
    drop(guard);
    let data = resp?;
    data.get("transaction_hash")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "chain-provider broadcast missing transaction_hash".to_string())
}

/// Broadcast an externally-signed tx through the REAL chain-provider against the
/// configured Base RPC (production path).
fn broadcast_live(signed_tx: &str) -> Result<String, String> {
    let chain_bin = resolve_chain_bin();
    if !std::path::Path::new(&chain_bin).is_file() {
        return Err(format!("chain-provider not found at {chain_bin}"));
    }
    let network = env_nonempty("ELASTOS_DDRM_RIGHTS_NETWORK").unwrap_or_else(|| "base".to_string());
    let rpc_url = env_nonempty("ELASTOS_CHAIN_BASE_RPC")
        .ok_or("ELASTOS_CHAIN_BASE_RPC (Base RPC URL) is required for live buy")?;
    let chain_id: i64 = env_nonempty("ELASTOS_DDRM_CHAIN_ID")
        .and_then(|s| s.parse().ok())
        .unwrap_or(8453);
    let init = json!({
        "op": "init",
        "config": { "networks": [{
            "id": network,
            "display_name": network,
            "kind": "evm_json_rpc",
            "chain_id": chain_id,
            "native_symbol": "ETH",
            "provider": "elastos-gateway",
            "mainnet": true,
            "explorer_url": null,
            "rpc_url": rpc_url,
        }]}
    });
    let broadcast = json!({
        "op": "broadcast_transaction",
        "network": network,
        "signed_transaction": signed_tx,
    });
    let data = run_chain_capsule(&chain_bin, &init, &broadcast)?;
    data.get("transaction_hash")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "chain-provider broadcast missing transaction_hash".to_string())
}

fn mock_init(rpc_url: &str) -> Value {
    json!({
        "op": "init",
        "config": { "networks": [{
            "id": "base-local-mock",
            "display_name": "base-local-mock",
            "kind": "evm_json_rpc",
            "chain_id": 8453,
            "native_symbol": "ETH",
            "provider": "elastos-gateway",
            "mainnet": true,
            "explorer_url": null,
            "rpc_url": rpc_url,
        }]}
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

/// A deterministic, well-formed 32-byte hash for the mock to echo as the tx hash.
fn mock_tx_hash(unsigned_tx: &Value) -> String {
    let mut h = Sha256::new();
    h.update(b"elastos-ddrm/buy-mock-tx/v1");
    h.update(serde_json::to_string(unsigned_tx).unwrap_or_default().as_bytes());
    format!("0x{}", hex::encode(h.finalize()))
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
}
