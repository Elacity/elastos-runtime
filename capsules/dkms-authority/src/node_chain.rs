//! The dKMS node's OWN pinned, read-only Base capability + the trustless
//! `authorize_access` gate.
//!
//! To be a trustless evaluator (not an enrollment-trusting receipt checker), each
//! node must read `hasAccessByContentId` itself and verify the user's wallet
//! signature itself. This module gives `dkms-authority` a minimal, operator-pinned,
//! read-only multi-RPC Base client (No Ambient Authority: explicit, narrow,
//! config-only, fail-closed) and the `authorize_access` function that combines the
//! W1 grant verifier (`ddrm-envelope::access`) with a node-side on-chain check.
//!
//! Multi-RPC discipline mirrors `chain-provider`: query the pool, treat a contract
//! revert as `false` (PC2 `.catch(() => false)`), and FAIL CLOSED on disagreement or
//! insufficient reachability — so a single lying endpoint can never fabricate a `true`.

use std::time::Duration;

use ddrm_envelope::access::{self, AccessGrantV1, AccessVerifyContext, Eip1271Caller, ReplayGuard};
use serde_json::{json, Value};

/// The on-chain access oracle the node depends on. `dkms-authority` ships
/// [`NodeChain`]; tests inject a mock. Supertrait of [`Eip1271Caller`] so the same
/// object can answer smart-account delegation checks.
pub trait AccessOracle: Eip1271Caller {
    /// `true` iff `holder` currently holds access to `kid16` on-chain. Fail-closed:
    /// `Err` on unavailability/disagreement, never a fabricated `true`.
    fn has_access_by_content_id(&self, holder: &str, kid16: &[u8; 16]) -> Result<bool, String>;
}

/// Outcome of one RPC's `hasAccessByContentId` read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RpcOutcome {
    /// The contract answered (a revert is normalized to `Allowed(false)`).
    Allowed(bool),
    /// Transport/parse failure — this endpoint did not contribute a vote.
    Unavailable,
}

/// Operator-pinned, read-only Base RPC client. Config comes ONLY from the node's
/// environment (`/etc/elastos/dkms-authority.env`), never from the caller.
pub struct NodeChain {
    rpc_pool: Vec<String>,
    contract: String,
    selector: String,
    chain_id: u64,
}

impl NodeChain {
    /// Build from operator env. `None` when no RPC pool is configured (the node then
    /// cannot do trustless grant authorization and fails closed on grant requests).
    ///   DKMS_CHAIN_RPC_POOL=https://mainnet.base.org,https://base.gateway.tenderly.co
    ///   DKMS_RIGHTS_CONTRACT=0x09dBe796...   DKMS_RIGHTS_SELECTOR=0x54d42821   DKMS_CHAIN_ID=8453
    pub fn from_env() -> Option<NodeChain> {
        let pool_raw = std::env::var("DKMS_CHAIN_RPC_POOL").ok()?;
        let rpc_pool: Vec<String> = pool_raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if rpc_pool.is_empty() {
            return None;
        }
        let contract = std::env::var("DKMS_RIGHTS_CONTRACT")
            .unwrap_or_else(|_| "0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D".to_string());
        let selector = std::env::var("DKMS_RIGHTS_SELECTOR").unwrap_or_else(|_| "0x54d42821".to_string());
        let chain_id = std::env::var("DKMS_CHAIN_ID")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8453);
        Some(NodeChain { rpc_pool, contract, selector, chain_id })
    }

    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    fn eth_call(&self, rpc: &str, to: &str, data_hex: &str) -> Result<Value, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|e| e.to_string())?;
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_call",
            "params": [{ "to": to, "data": data_hex }, "latest"],
        });
        let resp = client.post(rpc).json(&body).send().map_err(|e| e.to_string())?;
        resp.json::<Value>().map_err(|e| e.to_string())
    }

    fn read_one(&self, rpc: &str, holder: &str, kid16: &[u8; 16]) -> RpcOutcome {
        let data = match access::has_access_calldata(&self.selector, holder, kid16) {
            Ok(d) => d,
            Err(_) => return RpcOutcome::Unavailable,
        };
        let data_hex = format!("0x{}", hex::encode(&data));
        let v = match self.eth_call(rpc, &self.contract, &data_hex) {
            Ok(v) => v,
            Err(_) => return RpcOutcome::Unavailable,
        };
        if let Some(err) = v.get("error") {
            // A contract revert means "no access" (PC2 `.catch(() => false)`); any other
            // error is a transport-class failure that does not get to vote.
            return if is_revert(err) { RpcOutcome::Allowed(false) } else { RpcOutcome::Unavailable };
        }
        match v.get("result").and_then(Value::as_str) {
            Some(hexs) => match hex::decode(strip_0x(hexs)).ok().and_then(|b| access::decode_evm_bool(&b)) {
                Some(b) => RpcOutcome::Allowed(b),
                None => RpcOutcome::Unavailable,
            },
            None => RpcOutcome::Unavailable,
        }
    }
}

impl AccessOracle for NodeChain {
    fn has_access_by_content_id(&self, holder: &str, kid16: &[u8; 16]) -> Result<bool, String> {
        let outcomes: Vec<RpcOutcome> =
            self.rpc_pool.iter().map(|rpc| self.read_one(rpc, holder, kid16)).collect();
        combine_quorum(&outcomes, self.rpc_pool.len())
    }
}

impl Eip1271Caller for NodeChain {
    fn is_valid_signature(&self, owner: &str, message_hash: &[u8; 32], sig_hex: &str) -> Option<Vec<u8>> {
        let data = access::eip1271_calldata(message_hash, sig_hex).ok()?;
        let data_hex = format!("0x{}", hex::encode(&data));
        for rpc in &self.rpc_pool {
            match self.eth_call(rpc, owner, &data_hex) {
                Ok(v) => {
                    if let Some(err) = v.get("error") {
                        // revert => not a valid signature (empty, non-magic); transport error => try next
                        if is_revert(err) {
                            return Some(Vec::new());
                        }
                        continue;
                    }
                    if let Some(hexs) = v.get("result").and_then(Value::as_str) {
                        return hex::decode(strip_0x(hexs)).ok();
                    }
                }
                Err(_) => continue,
            }
        }
        None
    }
}

/// Combine per-RPC outcomes into one fail-closed decision. Require at least
/// `min(2, pool_len)` reachable endpoints, and that all reachable endpoints AGREE.
/// Disagreement or insufficient reachability ⇒ `Err` (fail closed).
fn combine_quorum(outcomes: &[RpcOutcome], pool_len: usize) -> Result<bool, String> {
    let mut trues = 0usize;
    let mut falses = 0usize;
    for o in outcomes {
        match o {
            RpcOutcome::Allowed(true) => trues += 1,
            RpcOutcome::Allowed(false) => falses += 1,
            RpcOutcome::Unavailable => {}
        }
    }
    let reachable = trues + falses;
    let need = pool_len.min(2).max(1);
    if reachable < need {
        return Err(format!(
            "on-chain read unavailable: {reachable}/{pool_len} RPC endpoints answered (need >= {need})"
        ));
    }
    if trues > 0 && falses > 0 {
        return Err("on-chain RPC disagreement — failing closed".to_string());
    }
    Ok(trues > 0)
}

fn is_revert(err: &Value) -> bool {
    if err.get("code").and_then(Value::as_i64) == Some(3) {
        return true;
    }
    let msg = err
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    msg.contains("execution reverted") || msg.contains("revert")
}

fn strip_0x(s: &str) -> &str {
    s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s)
}

/// THE TRUSTLESS GATE. Verify the wallet-signed grant in the node's own boundary
/// (EIP-191/1271 + session-key request sig + window/nonce/node-set/kid binding), then
/// evaluate `hasAccessByContentId` ITSELF per covered address, requiring at least one
/// `true`. Every failure path FAILS CLOSED. Replaces trust-the-receipt entirely.
pub fn authorize_access<O: AccessOracle>(
    grant: &AccessGrantV1,
    expected_node_set_id_b64: &str,
    chain_id: u64,
    kid_hex: &str,
    now: u64,
    replay: Option<&mut ReplayGuard>,
    chain: &O,
) -> Result<(), String> {
    let ctx = AccessVerifyContext {
        expected_node_set_id_b64: expected_node_set_id_b64.to_string(),
        expected_chain_id: chain_id,
        expected_kid_hex: kid_hex.to_string(),
        now,
    };
    let verified = access::verify_access_grant(grant, &ctx, replay, Some(chain as &dyn Eip1271Caller))
        .map_err(|e| format!("access grant rejected: {e}"))?;
    let kid16 = access::kid_to_bytes16(&verified.kid_hex).ok_or("kid malformed")?;

    let mut last_err: Option<String> = None;
    for addr in &verified.covered_addresses {
        match chain.has_access_by_content_id(addr, &kid16) {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(e) => last_err = Some(e),
        }
    }
    match last_err {
        Some(e) => Err(format!("on-chain access check failed closed: {e}")),
        None => Err("no on-chain access for any covered address".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ddrm_envelope::access::testkit::signed_grant;

    const NS: &str = "dGVzdC1ub2RlLXNldA=="; // base64("test-node-set")
    const KID: &str = "0xabababababababababababababababab";
    const NOW: u64 = 1_000_000;

    // ── multi-RPC quorum read ──
    #[test]
    fn quorum_all_true_allows() {
        assert_eq!(combine_quorum(&[RpcOutcome::Allowed(true), RpcOutcome::Allowed(true)], 2), Ok(true));
    }

    #[test]
    fn quorum_all_false_denies() {
        assert_eq!(combine_quorum(&[RpcOutcome::Allowed(false), RpcOutcome::Allowed(false)], 2), Ok(false));
    }

    #[test]
    fn quorum_disagreement_fails_closed() {
        assert!(combine_quorum(&[RpcOutcome::Allowed(true), RpcOutcome::Allowed(false)], 2).is_err());
    }

    #[test]
    fn quorum_insufficient_reachability_fails_closed() {
        // pool of 2, only 1 answered → below need(2) → fail closed
        assert!(combine_quorum(&[RpcOutcome::Allowed(true), RpcOutcome::Unavailable], 2).is_err());
    }

    #[test]
    fn quorum_single_endpoint_pool_ok() {
        assert_eq!(combine_quorum(&[RpcOutcome::Allowed(true)], 1), Ok(true));
    }

    #[test]
    fn revert_is_a_false_vote() {
        let err = json!({ "code": 3, "message": "execution reverted" });
        assert!(is_revert(&err));
        let transport = json!({ "code": -32000, "message": "header not found" });
        assert!(!is_revert(&transport));
    }

    // ── authorize_access with an injected oracle ──
    struct MockChain {
        access: Result<bool, String>,
    }
    impl Eip1271Caller for MockChain {
        fn is_valid_signature(&self, _o: &str, _h: &[u8; 32], _s: &str) -> Option<Vec<u8>> {
            None
        }
    }
    impl AccessOracle for MockChain {
        fn has_access_by_content_id(&self, _holder: &str, _kid16: &[u8; 16]) -> Result<bool, String> {
            self.access.clone()
        }
    }

    #[test]
    fn authorize_allows_when_grant_valid_and_chain_true() {
        let (grant, _owner) = signed_grant(NS, KID, 8453, NOW, 3600, &[], 11);
        let chain = MockChain { access: Ok(true) };
        assert!(authorize_access(&grant, NS, 8453, KID, NOW, None, &chain).is_ok());
    }

    #[test]
    fn authorize_denies_when_chain_false() {
        let (grant, _owner) = signed_grant(NS, KID, 8453, NOW, 3600, &[], 12);
        let chain = MockChain { access: Ok(false) };
        let err = authorize_access(&grant, NS, 8453, KID, NOW, None, &chain).unwrap_err();
        assert!(err.contains("no on-chain access"), "got: {err}");
    }

    #[test]
    fn authorize_fails_closed_when_chain_unavailable() {
        let (grant, _owner) = signed_grant(NS, KID, 8453, NOW, 3600, &[], 13);
        let chain = MockChain { access: Err("rpc disagreement".to_string()) };
        let err = authorize_access(&grant, NS, 8453, KID, NOW, None, &chain).unwrap_err();
        assert!(err.contains("failed closed"), "got: {err}");
    }

    #[test]
    fn authorize_rejects_forged_grant_before_touching_chain() {
        let (mut grant, _owner) = signed_grant(NS, KID, 8453, NOW, 3600, &[], 14);
        // tamper the covered set AFTER signing → wallet sig no longer recovers the owner
        grant.delegation.covered_addresses.push("0x000000000000000000000000000000000000dead".into());
        // even with a chain that would say true, the grant must be rejected first
        let chain = MockChain { access: Ok(true) };
        let err = authorize_access(&grant, NS, 8453, KID, NOW, None, &chain).unwrap_err();
        assert!(err.contains("access grant rejected"), "got: {err}");
    }

    #[test]
    fn authorize_rejects_foreign_quorum() {
        let (grant, _owner) = signed_grant(NS, KID, 8453, NOW, 3600, &[], 15);
        let chain = MockChain { access: Ok(true) };
        // node expects a DIFFERENT node-set than the grant was bound to
        let err = authorize_access(&grant, "b3RoZXItc2V0", 8453, KID, NOW, None, &chain).unwrap_err();
        assert!(err.contains("access grant rejected"), "got: {err}");
    }
}
