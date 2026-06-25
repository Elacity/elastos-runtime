//! Shared chain-provider transaction plumbing for the buy + mint orchestrations.
//!
//! One canonical path for "source fees → assemble the unsigned intent" and "broadcast a
//! signed tx", so the buy flow and the mint flow can never drift in how they talk to the
//! chain. macOS drives `chain-provider` as a host SUBPROCESS here (the same isolation tier
//! the rights gate uses), reusing `rights_authority`'s spawn helper.
//!
//! Nothing in this module holds keys or invents transaction fields: fees come from the
//! REAL `chain-provider.prepare_transaction`, and the signed bytes are produced elsewhere
//! (the wallet capsule) before being handed to `broadcast_*`.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::rights_authority::{env_nonempty, resolve_chain_bin, run_chain_capsule, ChainRpcMock};

/// Init for the in-process JSON-RPC mock network (offline broadcast proof).
pub(crate) fn mock_init(rpc_url: &str) -> Value {
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

/// The shared `init` for the live Base RPC (used by both prepare and broadcast), so the
/// network definition can never drift between the two ops. Returns `(network_id, init)`.
pub(crate) fn live_chain_init() -> Result<(String, Value), String> {
    let chain_bin = resolve_chain_bin();
    if !std::path::Path::new(&chain_bin).is_file() {
        return Err(format!("chain-provider not found at {chain_bin}"));
    }
    let network = env_nonempty("ELASTOS_DDRM_RIGHTS_NETWORK").unwrap_or_else(|| "base".to_string());
    let rpc_url = env_nonempty("ELASTOS_CHAIN_BASE_RPC")
        .ok_or("ELASTOS_CHAIN_BASE_RPC (Base RPC URL) is required for a live transaction")?;
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
    Ok((network, init))
}

/// Source real nonce/gas and assemble the live `unsigned_transaction_intent/v1` via the
/// REAL `chain-provider.prepare_transaction`. The returned intent is exactly what the
/// wallet capsule's `transaction_intent` consumes.
pub(crate) fn prepare_intent_live(
    from: &str,
    to: &str,
    value: &str,
    data: &str,
) -> Result<Value, String> {
    let (network, init) = live_chain_init()?;
    let prepare = json!({
        "op": "prepare_transaction",
        "network": network,
        "from": from,
        "to": to,
        "value": value,
        "data": data,
    });
    let chain_bin = resolve_chain_bin();
    run_chain_capsule(&chain_bin, &init, &prepare)
}

/// Broadcast a signed tx through the REAL chain-provider against an in-process JSON-RPC
/// mock that answers `eth_sendRawTransaction` with a deterministic canned tx hash. The
/// signed bytes are really validated and sent; `seed` only derives the canned hash.
pub(crate) fn broadcast_signed_mock(seed: &Value, signed_tx: &str) -> Result<String, String> {
    let chain_bin = resolve_chain_bin();
    if !std::path::Path::new(&chain_bin).is_file() {
        return Err(format!(
            "chain-provider not found at {chain_bin}; build it with \
             `cargo build --manifest-path capsules/chain-provider/Cargo.toml` \
             or set ELASTOS_CHAIN_PROVIDER_BIN"
        ));
    }
    let guard = ChainRpcMock::start_with_word(mock_tx_word(seed))?;
    let init = mock_init(&guard.url);
    let broadcast = json!({
        "op": "broadcast_transaction",
        "network": "base-local-mock",
        "signed_transaction": signed_tx,
    });
    let resp = run_chain_capsule(&chain_bin, &init, &broadcast);
    drop(guard);
    let data = resp?;
    data.get("transaction_hash")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "chain-provider broadcast missing transaction_hash".to_string())
}

/// Broadcast an already-signed tx through the REAL chain-provider against the configured
/// Base RPC (production path).
pub(crate) fn broadcast_signed_live(signed_tx: &str) -> Result<String, String> {
    let (network, init) = live_chain_init()?;
    let broadcast = json!({
        "op": "broadcast_transaction",
        "network": network,
        "signed_transaction": signed_tx,
    });
    let chain_bin = resolve_chain_bin();
    let data = run_chain_capsule(&chain_bin, &init, &broadcast)?;
    data.get("transaction_hash")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "chain-provider broadcast missing transaction_hash".to_string())
}

/// A deterministic, well-formed 32-byte hash for the mock to echo as the tx hash.
fn mock_tx_word(seed: &Value) -> String {
    let mut h = Sha256::new();
    h.update(b"elastos-ddrm/mock-tx/v1");
    h.update(serde_json::to_string(seed).unwrap_or_default().as_bytes());
    format!("0x{}", hex::encode(h.finalize()))
}
