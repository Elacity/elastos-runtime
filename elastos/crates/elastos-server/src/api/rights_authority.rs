//! Gateway-side rights gate for the live-chain open path.
//!
//! Anders' rule: the rights DECISION lives in the `rights-provider` capsule, not in
//! the gateway, and the gateway never holds chain RPC. So this module does NOT decide
//! access itself — it:
//!   1. obtains a typed on-chain ownership attestation (`ChainAccessAttestationV1`)
//!      from a rights source (dev local-attestation today; `chain-provider` when it is
//!      registered on this host), and
//!   2. spawns the real `rights-provider` capsule (built with `chain-rights`) and asks
//!      it to `decide_access_from_chain` — the capsule binds the attestation to the
//!      request and mints the signed `RightsDecisionReceiptV1`.
//!
//! The gateway only reads the receipt's `allowed` bit to gate, and a stable hash of the
//! receipt to weld into the decrypt transcript (so a seal made under one decision cannot
//! be replayed under another). Swapping the dev attestation for a live
//! `chain-provider.has_access_by_content_id` read is a source change only — the capsule
//! contract and the receipt shape are unchanged.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Compile-time dev-tree default for the rights-provider capsule built with
/// `--features chain-rights`; override with `ELASTOS_RIGHTS_PROVIDER_BIN`.
const DEV_RIGHTS_PROVIDER_BIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../capsules/rights-provider/target/debug/rights-provider"
);

/// The outcome of a rights decision for an owned-object open.
pub struct RightsDecision {
    /// The capsule's verdict — the gate proceeds only when this is true.
    pub allowed: bool,
    /// A stable hash (hex) of the minted `RightsDecisionReceiptV1`, for transcript
    /// binding: the authority welds it into the decrypt AAD so the seal is bound to
    /// THIS decision.
    pub receipt_hash_hex: String,
    /// The rights source that produced the attestation (audit/debug only).
    pub source: &'static str,
    /// The full receipt the capsule minted (audit only; carries no authority).
    pub receipt: Value,
}

fn resolve_rights_bin() -> String {
    std::env::var("ELASTOS_RIGHTS_PROVIDER_BIN")
        .unwrap_or_else(|_| DEV_RIGHTS_PROVIDER_BIN.to_string())
}

/// Map a Home principal DID to the EVM `address subject` the on-chain access-token
/// check is keyed on. The production path reads the principal's LINKED wallet address
/// (wallet-provider); until that binding is surfaced on this host, dev derives a
/// deterministic placeholder address from the principal so the attestation + receipt
/// are well-formed and stable. Never app-visible.
fn dev_subject_address(principal_id: &str) -> String {
    let digest = Sha256::digest(format!("elastos-dev-subject:{principal_id}").as_bytes());
    format!("0x{}", hex::encode(&digest[..20]))
}

/// Is this content explicitly denied for the dev attestation? `ELASTOS_DDRM_DENY_CIDS`
/// is a comma-separated allowlist-inverse used to exercise the fail-closed path (a
/// not-owned / no-access-token object) locally without a real chain.
fn dev_denies(content_id: &str) -> bool {
    std::env::var("ELASTOS_DDRM_DENY_CIDS")
        .ok()
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .any(|denied| denied == content_id)
        })
        .unwrap_or(false)
}

/// Gate an owned-object open through the rights-provider capsule.
///
/// `content_id` is the object's content CID; `right` is the action (`view`). Returns the
/// capsule's decision. The DEV attestation reports `has_access = true` for an owned
/// object (the caller has already proven local ownership by resolving + reading it under
/// the principal's own root) UNLESS it is listed in `ELASTOS_DDRM_DENY_CIDS`. A future
/// `ELASTOS_DDRM_RIGHTS=chain` mode will instead read `chain-provider`.
pub fn decide_owned_access(
    principal_id: &str,
    session_id: &str,
    content_id: &str,
    right: &str,
    reason: &str,
    policy_ref: Option<&str>,
    now_unix: u64,
    ttl_secs: u64,
) -> Result<RightsDecision, String> {
    let bin = resolve_rights_bin();
    if !std::path::Path::new(&bin).is_file() {
        return Err(format!(
            "rights-provider (chain-rights) not found at {bin}; build it with \
             `cargo build --manifest-path capsules/rights-provider/Cargo.toml \
             --features chain-rights` or set ELASTOS_RIGHTS_PROVIDER_BIN"
        ));
    }

    // DEV attestation: the on-chain ownership answer chain-provider WOULD return.
    // Local ownership was already proven by the caller; deny-list simulates "no token".
    let has_access = !dev_denies(content_id);
    let subject = dev_subject_address(principal_id);
    let chain_access = json!({
        "network": "base-mainnet",
        "contract": "0x0000000000000000000000000000000000000001",
        "content_id": content_id,
        "subject": subject,
        "right": right,
        "has_access": has_access,
    });

    let request_id = format!(
        "rights-{}",
        hex::encode(&Sha256::digest(format!("{content_id}:{principal_id}:{now_unix}"))[..12])
    );
    let decide = json!({
        "op": "decide_access_from_chain",
        "request_id": request_id,
        "request": {
            "principal_id": principal_id,
            "session_id": session_id,
            "content_id": content_id,
            "right": right,
            "reason": reason,
            "policy_ref": policy_ref,
        },
        "chain_access": chain_access,
        "now_unix": now_unix,
        "ttl_secs": ttl_secs,
    });

    let data = run_rights_capsule(&bin, &decide)?;
    let receipt = data
        .get("receipt")
        .cloned()
        .ok_or("rights-provider decision missing receipt")?;
    let allowed = receipt
        .get("allowed")
        .and_then(Value::as_bool)
        .ok_or("rights receipt missing allowed")?;
    let receipt_hash_hex = canonical_receipt_hash_hex(&receipt);

    Ok(RightsDecision {
        allowed,
        receipt_hash_hex,
        source: "dev-local-attestation",
        receipt,
    })
}

/// Spawn the rights-provider capsule, send one request + `shutdown`, return its `data`.
fn run_rights_capsule(bin: &str, request: &Value) -> Result<Value, String> {
    let mut child = Command::new(bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("spawn rights-provider ({bin}): {e}"))?;
    {
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        writeln!(stdin, "{request}").map_err(|e| format!("write rights request: {e}"))?;
        writeln!(stdin, "{}", json!({ "op": "shutdown" }))
            .map_err(|e| format!("write shutdown: {e}"))?;
        stdin.flush().map_err(|e| format!("flush: {e}"))?;
    }
    let mut reader = BufReader::new(child.stdout.take().ok_or("no stdout")?);
    let mut line = String::new();
    let n = reader
        .read_line(&mut line)
        .map_err(|e| format!("read rights response: {e}"))?;
    let _ = child.wait();
    if n == 0 {
        return Err("rights-provider exited before answering".to_string());
    }
    let resp: Value =
        serde_json::from_str(line.trim()).map_err(|e| format!("parse rights response: {e}"))?;
    if resp.get("status").and_then(Value::as_str) != Some("ok") {
        let message = resp
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("rights-provider error");
        return Err(message.to_string());
    }
    resp.get("data")
        .cloned()
        .ok_or_else(|| "rights-provider ok response missing data".to_string())
}

/// Stable hash of the minted receipt for transcript binding: a domain-separated
/// SHA-256 over the receipt re-serialized with sorted keys, so the gateway, the
/// key-authority, and (later) the decrypt boundary all derive the SAME 32 bytes.
fn canonical_receipt_hash_hex(receipt: &Value) -> String {
    let mut h = Sha256::new();
    h.update(b"elastos-ddrm/rights-binding/v1");
    let canonical = canonical_json(receipt);
    h.update(canonical.as_bytes());
    hex::encode(h.finalize())
}

/// Re-serialize a JSON value with object keys sorted, for a deterministic digest.
fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            let inner: Vec<String> = entries
                .into_iter()
                .map(|(k, v)| format!("{}:{}", serde_json::to_string(k).unwrap_or_default(), canonical_json(v)))
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(canonical_json).collect();
            format!("[{}]", inner.join(","))
        }
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}
