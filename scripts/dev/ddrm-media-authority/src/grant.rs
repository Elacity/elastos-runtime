//! One-shot grant prepare/assemble — the gateway's trustless-open sidecar.
//!
//! The gateway must NOT link the PQ crypto (ml-dsa/ml-kem) — see this crate's Cargo.toml. So the
//! two-phase wallet-signed [`AccessGrantV1`] assembly happens here, invoked by the gateway as a
//! one-shot subprocess, and the gateway holds only OPAQUE strings (the session seed + the
//! delegation JSON) between the browser's two HTTP calls:
//!
//!   `--grant-prepare`  stdin  {chain_id, kid_hex, node_set_id_b64, owner_address}
//!                      stdout {session_seed_b64, delegation_json, delegation_canonical,
//!                              owner_address, kid}
//!   `--grant-assemble` stdin  {session_seed_b64, delegation_json, delegation_sig_hex}
//!                      stdout {grant_json}
//!
//! The session PRIVATE key (ML-DSA) is minted here and re-derived from the opaque seed on assemble;
//! it only AUTHORIZES (signs the per-request payload) and never guards CEK material. The wallet
//! signature is produced in the browser and verified at the NODE — never here.

use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use ddrm_envelope::access::{assemble_grant, build_delegation, build_request, AccessDelegationV1};
use ddrm_envelope::seal::mldsa_seal_keypair;
use serde_json::{json, Value};

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

/// The delegation validity window. With the gateway's session cache (PC2 parity), the wallet signs
/// ONCE and re-opens within this window are popup-free, so the window doubles as the "signed-in to
/// the dDRM system" period. Kept well under the node's `MAX_DELEGATION_WINDOW_SECONDS` (24h) so a
/// leaked delegation is bounded; the live on-chain check still gates every recover regardless.
const DELEGATION_WINDOW_SECS: u64 = 3600;

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn read_stdin_json() -> Result<Value, String> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| format!("read stdin: {e}"))?;
    serde_json::from_str(buf.trim()).map_err(|e| format!("parse stdin json: {e}"))
}

fn require_str<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("missing or empty `{key}`"))
}

/// PHASE 1 (pure): mint a session keypair + build the delegation the wallet will sign. Returns the
/// opaque session seed (base64) the gateway stashes + the delegation.
pub fn prepare_grant(
    chain_id: u64,
    kid_hex: &str,
    node_set_id_b64: &str,
    owner_address: &str,
) -> Result<(String, AccessDelegationV1), String> {
    let owner = owner_address.trim();
    if owner.is_empty() {
        return Err("owner_address is required (a chain check needs a real wallet)".to_string());
    }
    let session_seed = ddrm_envelope::random_seed();
    let (_signer, session_vk) = mldsa_seal_keypair(session_seed);
    let session_pub_b64 = B64.encode(&session_vk);
    let rnd = ddrm_envelope::random_seed();
    let nonce_b64 = B64.encode(&rnd[..16]);
    let delegation = build_delegation(
        chain_id,
        kid_hex,
        node_set_id_b64,
        owner,
        vec![owner.to_string()],
        &session_pub_b64,
        now_unix(),
        DELEGATION_WINDOW_SECS,
        &nonce_b64,
    );
    Ok((B64.encode(session_seed), delegation))
}

/// PHASE 2 (pure): re-derive the session signer from the opaque seed, build a fresh request, sign
/// it, and assemble the grant the node will verify.
pub fn assemble_from(
    session_seed_b64: &str,
    delegation: AccessDelegationV1,
    delegation_sig_hex: &str,
) -> Result<Value, String> {
    let sig = delegation_sig_hex.trim();
    if sig.is_empty() {
        return Err("delegation_sig_hex is required".to_string());
    }
    let seed_bytes = B64
        .decode(session_seed_b64.trim())
        .map_err(|e| format!("session_seed_b64 is not base64: {e}"))?;
    let seed: [u8; 32] = seed_bytes
        .as_slice()
        .try_into()
        .map_err(|_| "session seed must be 32 bytes".to_string())?;
    let (signer, _vk) = mldsa_seal_keypair(seed);
    let rnd = ddrm_envelope::random_seed();
    let request = build_request(
        &delegation.kid_hex,
        &delegation.node_set_id_b64,
        now_unix(),
        &B64.encode(&rnd[..12]),
    );
    let grant = assemble_grant(delegation, sig.to_string(), request, &signer);
    serde_json::to_value(&grant).map_err(|e| format!("serialize grant: {e}"))
}

/// `--grant-prepare` entrypoint.
pub fn run_grant_prepare() -> Result<(), String> {
    let inp = read_stdin_json()?;
    let chain_id = inp
        .get("chain_id")
        .and_then(Value::as_u64)
        .ok_or("missing or non-integer `chain_id`")?;
    let kid_hex = require_str(&inp, "kid_hex")?;
    let node_set_id_b64 = require_str(&inp, "node_set_id_b64")?;
    let owner_address = require_str(&inp, "owner_address")?;

    let (session_seed_b64, delegation) =
        prepare_grant(chain_id, kid_hex, node_set_id_b64, owner_address)?;
    let canonical = delegation.canonical();
    let out = json!({
        "session_seed_b64": session_seed_b64,
        "delegation_json": serde_json::to_value(&delegation).map_err(|e| e.to_string())?,
        "delegation_canonical": canonical,
        "owner_address": delegation.owner_address,
        "kid": delegation.kid_hex,
    });
    println!("{out}");
    Ok(())
}

/// `--grant-assemble` entrypoint.
pub fn run_grant_assemble() -> Result<(), String> {
    let inp = read_stdin_json()?;
    let session_seed_b64 = require_str(&inp, "session_seed_b64")?;
    let delegation_sig_hex = require_str(&inp, "delegation_sig_hex")?;
    let delegation: AccessDelegationV1 = serde_json::from_value(
        inp.get("delegation_json").cloned().ok_or("missing `delegation_json`")?,
    )
    .map_err(|e| format!("delegation_json is not an AccessDelegationV1: {e}"))?;

    let grant = assemble_from(session_seed_b64, delegation, delegation_sig_hex)?;
    println!("{}", json!({ "grant_json": grant }));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ddrm_envelope::access::testkit::{personal_sign_canonical, test_wallet_address};
    use ddrm_envelope::access::{
        verify_access_grant, AccessGrantV1, AccessVerifyContext, ReplayGuard,
    };

    const CHAIN_ID: u64 = 8453;
    const WALLET_SEED: u8 = 0x21;

    fn kid() -> String {
        format!("0x{}", "cd".repeat(16))
    }
    fn node_set() -> String {
        B64.encode(b"elastos-sidecar-test-node-set")
    }

    /// The sidecar's prepare→sign→assemble produces a grant the NODE verifies — the real
    /// browser→gateway→node path, with `personal_sign_canonical` standing in for MetaMask.
    #[test]
    fn sidecar_roundtrip_produces_node_verifiable_grant() {
        let owner = test_wallet_address(WALLET_SEED);
        let (seed_b64, delegation) =
            prepare_grant(CHAIN_ID, &kid(), &node_set(), &owner).expect("prepare");
        assert_eq!(delegation.owner_address, owner);

        let sig = personal_sign_canonical(WALLET_SEED, &delegation.canonical());
        let grant_json = assemble_from(&seed_b64, delegation, &sig).expect("assemble");
        let grant: AccessGrantV1 = serde_json::from_value(grant_json).expect("grant type");

        let ctx = AccessVerifyContext {
            expected_node_set_id_b64: node_set(),
            expected_chain_id: CHAIN_ID,
            expected_kid_hex: kid(),
            now: now_unix(),
        };
        let mut guard = ReplayGuard::new();
        verify_access_grant(&grant, &ctx, Some(&mut guard), None)
            .expect("sidecar-built grant must verify at the node");
    }

    /// A wrong wallet signature (different seed) fails the node's recovery check — fail closed.
    #[test]
    fn wrong_wallet_signature_fails_node_verification() {
        let owner = test_wallet_address(WALLET_SEED);
        let (seed_b64, delegation) =
            prepare_grant(CHAIN_ID, &kid(), &node_set(), &owner).expect("prepare");
        // Sign with a DIFFERENT wallet — the recovered address won't match owner_address.
        let bad_sig = personal_sign_canonical(WALLET_SEED ^ 0xFF, &delegation.canonical());
        let grant_json = assemble_from(&seed_b64, delegation, &bad_sig).expect("assemble");
        let grant: AccessGrantV1 = serde_json::from_value(grant_json).expect("grant type");
        let ctx = AccessVerifyContext {
            expected_node_set_id_b64: node_set(),
            expected_chain_id: CHAIN_ID,
            expected_kid_hex: kid(),
            now: now_unix(),
        };
        assert!(verify_access_grant(&grant, &ctx, None, None).is_err());
    }

    #[test]
    fn prepare_grant_requires_wallet() {
        assert!(prepare_grant(CHAIN_ID, &kid(), &node_set(), "  ").is_err());
    }
}
