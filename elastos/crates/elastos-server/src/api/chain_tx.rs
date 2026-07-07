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
    // Public Base endpoints the chain-provider ROTATES to when the primary rate-limits (HTTP 429 /
    // JSON-RPC -32016). Without these the single-endpoint config has nothing to fail over to, so a burst
    // (discovery getLogs sweep + a detail view's sellersOf/listings/royalty reads) drains one bucket and
    // every read fails — surfacing as a false "not listed". Mirrors the chain-provider's own PC2 pool.
    // `eth_call` fallbacks are broad (any keyless endpoint serves a call); the log pool is RANGE-CAPABLE
    // ONLY (publicnode silently truncates getLogs, so it must never serve a discovery scan). Override via
    // ELASTOS_CHAIN_BASE_RPC_FALLBACKS / _LOG_RPCS (comma-separated).
    let split = |s: String| -> Vec<String> {
        s.split(',')
            .map(|u| u.trim().to_string())
            .filter(|u| !u.is_empty() && *u != rpc_url)
            .collect()
    };
    let call_fallbacks = env_nonempty("ELASTOS_CHAIN_BASE_RPC_FALLBACKS")
        .map(split)
        .unwrap_or_else(|| {
            [
                "https://base-rpc.publicnode.com",
                "https://base.drpc.org",
                "https://base.meowrpc.com",
                "https://1rpc.io/base",
            ]
            .iter()
            .map(|s| s.to_string())
            .filter(|u| *u != rpc_url)
            .collect()
        });
    let log_rpcs = env_nonempty("ELASTOS_CHAIN_BASE_LOG_RPCS")
        .map(split)
        .unwrap_or_else(|| {
            std::iter::once(rpc_url.clone())
                .chain(["https://base.gateway.tenderly.co".to_string()])
                .collect()
        });
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
            "rpc_fallback_urls": call_fallbacks,
            "log_query_rpc_urls": log_rpcs,
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

/// Resolve the REAL on-chain ledger `tokenId` for a `bytes16` KID via the REAL chain-provider
/// `resolve_token_id` op (scans `AssetCreated` on the channel/`ledger` + binds the KID through the
/// mint calldata). READ-ONLY (`eth_getLogs` + `eth_getTransactionByHash`); no keys. Returns the
/// `0x…` tokenId, or an error if unresolved — the live buy then fails closed (P11). This is the
/// Phase-1 fix: the buy binds THIS, never `word_from_id(content_id)`.
pub(crate) fn resolve_token_id_live(
    content_id: &str,
    ledger: &str,
) -> Result<(String, String), String> {
    let (network, init) = live_chain_init()?;
    let mut request = json!({
        "op": "resolve_token_id",
        "network": network,
        "ledger": ledger,
        "content_id": content_id,
    });
    if let Some(from_block) = env_nonempty("ELASTOS_DDRM_BUY_FROM_BLOCK") {
        request["from_block"] = json!(from_block);
    }
    let chain_bin = resolve_chain_bin();
    let resp = run_chain_capsule(&chain_bin, &init, &request)?;
    let token_id = resp
        .get("token_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("chain-provider resolve_token_id returned no token_id: {resp}"))?;
    // The operative (the per-asset ERC-1155) is needed to re-read the listing terms before broadcast.
    let operative = resp
        .get("operative")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Ok((token_id, operative))
}

/// A read-only `eth_call` through the REAL chain-provider `contract_call` op (live Base RPC); returns
/// the raw `0x…` return data. No keys (P3). Used by the buy's pre-broadcast listing re-read (abort-on-drift).
pub(crate) fn contract_call_live(to: &str, data: &str) -> Result<String, String> {
    let (network, init) = live_chain_init()?;
    let request = json!({ "op": "contract_call", "network": network, "to": to, "data": data });
    let chain_bin = resolve_chain_bin();
    let resp = run_chain_capsule(&chain_bin, &init, &request)?;
    resp.get("result")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("chain-provider contract_call returned no result: {resp}"))
}

/// Read the latest block number via the REAL chain-provider `block_number` op (live Base RPC).
pub(crate) fn block_number_live() -> Result<u64, String> {
    let (network, init) = live_chain_init()?;
    let request = json!({ "op": "block_number", "network": network });
    let chain_bin = resolve_chain_bin();
    let resp = run_chain_capsule(&chain_bin, &init, &request)?;
    let bn = resp.get("block_number");
    if let Some(s) = bn.and_then(Value::as_str) {
        return u64::from_str_radix(s.trim_start_matches("0x"), 16).map_err(|e| e.to_string());
    }
    bn.and_then(Value::as_u64)
        .ok_or_else(|| format!("chain-provider block_number returned no usable value: {resp}"))
}

/// The confirmation state of a broadcast tx, read from its receipt + the chain tip (Sprint 35).
/// The money-critical distinction: only `Confirmed` promotes a pending DRM buy to charged, only
/// `Reverted` refunds it, and `Pending` (not yet mined, below the depth floor, or the RPC was
/// unreachable) leaves the reservation held — NEVER auto-promoted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TxConfirmation {
    /// The receipt is mined, `status == 0x1` (success), AND at least `min_confirmations` deep.
    Confirmed,
    /// The receipt is mined and `status == 0x0` (the tx reverted on-chain).
    Reverted,
    /// Not yet mined, below the depth floor, or the confirmation read failed — hold, do not
    /// promote. Carries a human-readable why for the reconciliation log.
    Pending(String),
}

/// Read a broadcast tx's confirmation state via the REAL chain-provider `receipt` op
/// (`eth_getTransactionReceipt`) + the chain tip (`block_number`), applying a depth floor
/// (Sprint 35). READ-ONLY, no keys (P3). Fail-SAFE: any read failure returns `Pending` (the
/// reservation stays held; a not-yet-mined or unreachable tx is NEVER auto-charged). Live Base
/// only — the operator runbook, never CI.
pub(crate) fn tx_confirmation_live(
    tx_hash: &str,
    min_confirmations: u64,
) -> Result<TxConfirmation, String> {
    let (network, init) = live_chain_init()?;
    let chain_bin = resolve_chain_bin();
    let request = json!({ "op": "receipt", "network": network, "hash": tx_hash });
    let resp = run_chain_capsule(&chain_bin, &init, &request)?;
    let receipt = resp.get("receipt");
    // A null/absent receipt = the tx is not yet mined ⇒ Pending, hold (fail-safe).
    if receipt.map(Value::is_null).unwrap_or(true) {
        return Ok(TxConfirmation::Pending(
            "tx not yet mined (no receipt)".to_string(),
        ));
    }
    let tip = block_number_live()?;
    Ok(classify_receipt(receipt.unwrap(), tip, min_confirmations))
}

/// The PURE, money-critical classification of a mined receipt against the chain tip + depth floor
/// (Sprint 35). Extracted so the fail-closed rules (council S35 guardian F1/F2, red-team F3) are
/// CI-testable without a live chain. Rules, in order:
/// 1. no usable `blockNumber` ⇒ `Pending` (cannot compute depth);
/// 2. depth (`tip - mined + 1`) below the floor ⇒ `Pending` — gates BOTH verdicts, so a shallow
///    revert (which a reorg can re-include successfully) never refunds prematurely;
/// 3. at/above the floor: `status` explicit `0` ⇒ `Reverted`; explicit non-zero ⇒ `Confirmed`;
///    absent/empty/unparseable ⇒ `Pending` (a money gate defaults to HOLD, never "success").
///    Accepts a hex STRING (`0x1`) or a JSON NUMBER.
fn classify_receipt(receipt: &Value, tip: u64, min_confirmations: u64) -> TxConfirmation {
    let mined_block = receipt
        .get("blockNumber")
        .and_then(Value::as_str)
        .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok());
    let Some(mined_block) = mined_block else {
        return TxConfirmation::Pending("receipt has no usable blockNumber".to_string());
    };
    let depth = tip.saturating_sub(mined_block).saturating_add(1);
    if depth < min_confirmations {
        return TxConfirmation::Pending(format!(
            "at depth {depth} of {min_confirmations} required"
        ));
    }
    let status_val = receipt.get("status").and_then(|s| {
        s.as_str()
            .map(|h| h.trim_start_matches("0x").trim())
            .and_then(|h| u64::from_str_radix(h, 16).ok())
            .or_else(|| s.as_u64())
    });
    match status_val {
        Some(0) => TxConfirmation::Reverted,
        Some(_) => TxConfirmation::Confirmed,
        None => TxConfirmation::Pending("receipt has no parseable success status".to_string()),
    }
}

#[cfg(test)]
mod confirmation_tests {
    use super::*;

    #[test]
    fn classify_receipt_is_fail_closed_on_status_and_depth() {
        let mined = json!({"status": "0x1", "blockNumber": "0x64"}); // block 100
        // Deep enough + success ⇒ Confirmed.
        assert_eq!(classify_receipt(&mined, 102, 3), TxConfirmation::Confirmed);
        // Below the depth floor ⇒ Pending (tip 100 == mined ⇒ depth 1 < 3).
        assert!(matches!(
            classify_receipt(&mined, 100, 3),
            TxConfirmation::Pending(_)
        ));
        // Reverted, deep enough ⇒ Reverted (council S35 guardian F2: depth gates the revert too).
        let reverted = json!({"status": "0x0", "blockNumber": "0x64"});
        assert_eq!(classify_receipt(&reverted, 110, 3), TxConfirmation::Reverted);
        // Reverted but SHALLOW ⇒ Pending, NOT an immediate refund.
        assert!(matches!(
            classify_receipt(&reverted, 100, 3),
            TxConfirmation::Pending(_)
        ));
        // council S35 red-team F3 / guardian F1: a receipt with NO status but a deep block must
        // NOT read as success — hold.
        let no_status = json!({"blockNumber": "0x64"});
        assert!(matches!(
            classify_receipt(&no_status, 999, 3),
            TxConfirmation::Pending(_)
        ));
        // A numeric status is accepted (0 ⇒ revert, 1 ⇒ success).
        assert_eq!(
            classify_receipt(&json!({"status": 1, "blockNumber": "0x64"}), 999, 3),
            TxConfirmation::Confirmed
        );
        assert_eq!(
            classify_receipt(&json!({"status": 0, "blockNumber": "0x64"}), 999, 3),
            TxConfirmation::Reverted
        );
        // No blockNumber ⇒ cannot compute depth ⇒ Pending.
        assert!(matches!(
            classify_receipt(&json!({"status": "0x1"}), 999, 3),
            TxConfirmation::Pending(_)
        ));
    }
}

/// Fetch `eth_getLogs` entries via the REAL chain-provider `logs` op (one window; the caller bounds
/// the range). READ-ONLY; no keys (P3). Returns the raw log array (empty on no matches).
pub(crate) fn get_logs_live(filter: Value) -> Result<Vec<Value>, String> {
    let (network, init) = live_chain_init()?;
    let request = json!({ "op": "logs", "network": network, "filter": filter });
    let chain_bin = resolve_chain_bin();
    let resp = run_chain_capsule(&chain_bin, &init, &request)?;
    Ok(resp
        .get("logs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
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
