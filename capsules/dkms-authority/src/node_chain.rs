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

use ddrm_envelope::access::{self, AccessGrantV1, AccessVerifyContext, Eip1271Caller};
use ddrm_envelope::replay::{ReplayStore, IN_FLIGHT_TTL_SECONDS};
use serde_json::{json, Value};

/// Per-endpoint `eth_call` timeout, in seconds. Also the per-call multiplier in the replay-TTL
/// safety bound ([`check_replay_ttl_bound`]): a covered subject's sequential poll can spend up to
/// this long on each pool endpoint before the on-chain read returns.
const ETH_CALL_TIMEOUT_SECONDS: u64 = 8;

/// Fail-closed startup bound tying the replay `InFlight` TTL to the worst-case `begin`→`commit`
/// latency. `authorize_access` reserves an `InFlight` slot (`begin`), then `evaluate_onchain` polls
/// the pool SEQUENTIALLY at up to [`ETH_CALL_TIMEOUT_SECONDS`] per endpoint, for up to
/// [`access::MAX_COVERED_ADDRESSES`] subjects, before it can `commit`. If that worst case can reach
/// or exceed [`IN_FLIGHT_TTL_SECONDS`], the reservation could be pruned mid-authorization — then a
/// concurrent duplicate `begin` would see neither `in_flight` nor `seen` and reserve the same nonce
/// again (two live authorizations), and the eventual `commit`, now recording an already-expired
/// window, provides no protection. Refuse such a pool at construction so the node fails CLOSED at
/// startup rather than opening a replay window at runtime. Pure and total for unit testing.
fn check_replay_ttl_bound(pool_len: usize) -> Result<(), String> {
    let worst_case_seconds = (access::MAX_COVERED_ADDRESSES as u64)
        .saturating_mul(pool_len as u64)
        .saturating_mul(ETH_CALL_TIMEOUT_SECONDS);
    if IN_FLIGHT_TTL_SECONDS > worst_case_seconds {
        return Ok(());
    }
    Err(format!(
        "replay InFlight TTL {IN_FLIGHT_TTL_SECONDS}s must exceed the worst-case begin→commit \
         latency {worst_case_seconds}s (MAX_COVERED_ADDRESSES {} × pool_len {pool_len} × \
         {ETH_CALL_TIMEOUT_SECONDS}s/call); reduce the RPC pool size or per-call timeout, or raise \
         IN_FLIGHT_TTL_SECONDS. Refusing to construct — failing closed at startup.",
        access::MAX_COVERED_ADDRESSES,
    ))
}

/// The on-chain access oracle the node depends on. `dkms-authority` ships
/// [`NodeChain`]; tests inject a mock. Supertrait of [`Eip1271Caller`] so the same
/// object can answer smart-account delegation checks.
pub trait AccessOracle: Eip1271Caller {
    /// `true` iff `holder` currently holds access to `kid16` on-chain. Fail-closed:
    /// `Err` on unavailability/disagreement, never a fabricated `true`.
    fn has_access_by_content_id(&self, holder: &str, kid16: &[u8; 16]) -> Result<bool, String>;
}

/// One endpoint's contribution to a pooled decision. `Answered(T)` is a well-formed,
/// on-topic answer from that endpoint (it gets a vote); `Unavailable` is a transport,
/// parse, or RPC-protocol failure (it does NOT get a vote). This is the single typed
/// outcome model shared by BOTH `hasAccessByContentId` and EIP-1271 reads (DKMS-2), so
/// the two paths cannot drift into different agreement semantics.
///
/// For access reads `T = bool` is the `hasAccessByContentId` answer (a revert is
/// normalized to `Answered(false)`). For EIP-1271 `T = bool` is signature validity
/// (`Answered(true)` = magic value returned, `Answered(false)` = well-formed non-magic
/// or contract revert).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RpcVote<T> {
    Answered(T),
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
        // Fail CLOSED at startup: refuse a pool large/slow enough that a reservation could be pruned
        // between `begin` and `commit`, which would open a replay window (DKMS-3 review fix).
        if let Err(e) = check_replay_ttl_bound(rpc_pool.len()) {
            eprintln!("dkms-authority: refusing NodeChain configuration: {e}");
            return None;
        }
        let contract = std::env::var("DKMS_RIGHTS_CONTRACT")
            .unwrap_or_else(|_| "0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D".to_string());
        let selector =
            std::env::var("DKMS_RIGHTS_SELECTOR").unwrap_or_else(|_| "0x54d42821".to_string());
        let chain_id = std::env::var("DKMS_CHAIN_ID")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8453);
        Some(NodeChain {
            rpc_pool,
            contract,
            selector,
            chain_id,
        })
    }

    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    fn eth_call(&self, rpc: &str, to: &str, data_hex: &str) -> Result<Value, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(ETH_CALL_TIMEOUT_SECONDS))
            .build()
            .map_err(|e| e.to_string())?;
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_call",
            "params": [{ "to": to, "data": data_hex }, "latest"],
        });
        let resp = client
            .post(rpc)
            .json(&body)
            .send()
            .map_err(|e| e.to_string())?;
        resp.json::<Value>().map_err(|e| e.to_string())
    }

    fn read_one(&self, rpc: &str, holder: &str, kid16: &[u8; 16]) -> RpcVote<bool> {
        let data = match access::has_access_calldata(&self.selector, holder, kid16) {
            Ok(d) => d,
            Err(_) => return RpcVote::Unavailable,
        };
        let data_hex = format!("0x{}", hex::encode(&data));
        let v = match self.eth_call(rpc, &self.contract, &data_hex) {
            Ok(v) => v,
            Err(_) => return RpcVote::Unavailable,
        };
        if let Some(err) = v.get("error") {
            // A contract revert means "no access" (PC2 `.catch(() => false)`); any other
            // error is a transport-class failure that does not get to vote.
            return if is_revert(err) {
                RpcVote::Answered(false)
            } else {
                RpcVote::Unavailable
            };
        }
        match v.get("result").and_then(Value::as_str) {
            Some(hexs) => match hex::decode(strip_0x(hexs))
                .ok()
                .and_then(|b| access::decode_evm_bool(&b))
            {
                Some(b) => RpcVote::Answered(b),
                None => RpcVote::Unavailable,
            },
            None => RpcVote::Unavailable,
        }
    }

    /// One endpoint's EIP-1271 `isValidSignature` read, classified into the SAME typed
    /// [`RpcVote`] the access read uses. Magic value ⇒ `Answered(true)` (valid); a
    /// well-formed non-magic word or a contract revert ⇒ `Answered(false)` (an explicit
    /// "not a valid signature"); a malformed body, transport failure, or non-revert RPC
    /// error ⇒ `Unavailable` (no vote). No endpoint URL is ever surfaced.
    fn read_one_eip1271(
        &self,
        rpc: &str,
        owner: &str,
        message_hash: &[u8; 32],
        sig_hex: &str,
    ) -> RpcVote<bool> {
        let data = match access::eip1271_calldata(message_hash, sig_hex) {
            Ok(d) => d,
            Err(_) => return RpcVote::Unavailable,
        };
        let data_hex = format!("0x{}", hex::encode(&data));
        let v = match self.eth_call(rpc, owner, &data_hex) {
            Ok(v) => v,
            Err(_) => return RpcVote::Unavailable,
        };
        if let Some(err) = v.get("error") {
            // A revert is a legitimate "signature not valid" answer; any other error is a
            // transport/protocol failure that does not get to vote.
            return if is_revert(err) {
                RpcVote::Answered(false)
            } else {
                RpcVote::Unavailable
            };
        }
        match v.get("result").and_then(Value::as_str) {
            Some(hexs) => match hex::decode(strip_0x(hexs)) {
                Ok(bytes) => RpcVote::Answered(access::eip1271_is_magic(&bytes)),
                Err(_) => RpcVote::Unavailable,
            },
            None => RpcVote::Unavailable,
        }
    }
}

impl AccessOracle for NodeChain {
    fn has_access_by_content_id(&self, holder: &str, kid16: &[u8; 16]) -> Result<bool, String> {
        let votes: Vec<RpcVote<bool>> = self
            .rpc_pool
            .iter()
            .map(|rpc| self.read_one(rpc, holder, kid16))
            .collect();
        combine_votes(&votes, self.rpc_pool.len())
    }
}

impl Eip1271Caller for NodeChain {
    /// DKMS-2: a smart-account signature is only `valid` when the WHOLE configured pool
    /// agrees under the same reachability/agreement policy as `hasAccessByContentId` — it
    /// never stops at the first usable reply, so endpoint order cannot decide the verdict
    /// and a single endpoint cannot fabricate a `valid`.
    ///
    /// The `Option<Vec<u8>>` return is preserved (the verifier checks `eip1271_is_magic`):
    /// `Some(magic)` = the pool agreed the signature is valid; `Some([])` = the pool agreed
    /// it is NOT valid (well-formed non-magic / revert); `None` = fail closed (insufficient
    /// reachability or valid/invalid disagreement). Only `Some(magic)` is treated as valid.
    fn is_valid_signature(
        &self,
        owner: &str,
        message_hash: &[u8; 32],
        sig_hex: &str,
    ) -> Option<Vec<u8>> {
        let votes: Vec<RpcVote<bool>> = self
            .rpc_pool
            .iter()
            .map(|rpc| self.read_one_eip1271(rpc, owner, message_hash, sig_hex))
            .collect();
        match combine_votes(&votes, self.rpc_pool.len()) {
            Ok(true) => Some(access::EIP1271_MAGIC_VALUE.to_vec()),
            Ok(false) => Some(Vec::new()),
            Err(_) => None,
        }
    }
}

/// THE shared fail-closed agreement combiner used by BOTH `hasAccessByContentId` and
/// EIP-1271 (DKMS-2). Require at least `min(2, pool_len)` reachable endpoints, and that
/// every reachable endpoint AGREE on the answer. Disagreement or insufficient
/// reachability ⇒ `Err` (fail closed). Order-independent: the verdict is a function of
/// the multiset of votes, not their sequence. Error text carries only counts, never an
/// endpoint URL.
fn combine_votes<T: Copy + PartialEq>(votes: &[RpcVote<T>], pool_len: usize) -> Result<T, String> {
    let answered: Vec<T> = votes
        .iter()
        .filter_map(|v| match v {
            RpcVote::Answered(t) => Some(*t),
            RpcVote::Unavailable => None,
        })
        .collect();
    let reachable = answered.len();
    let need = pool_len.min(2).max(1);
    if reachable < need {
        crate::counters::incr(&crate::counters::QUORUM_UNAVAILABLE);
        return Err(format!(
            "on-chain read unavailable: {reachable}/{pool_len} RPC endpoints answered (need >= {need})"
        ));
    }
    let first = answered[0];
    if answered.iter().any(|t| *t != first) {
        crate::counters::incr(&crate::counters::QUORUM_DISAGREEMENT);
        return Err("on-chain RPC disagreement — failing closed".to_string());
    }
    Ok(first)
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
    s.strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s)
}

/// THE TRUSTLESS GATE (DKMS-3 two-phase pipeline). Verify the wallet-signed grant in the node's own
/// boundary (EIP-191/1271 + session-key request sig + window/nonce/node-set/kid binding), reserve a
/// bounded, expiring InFlight replay slot, evaluate `hasAccessByContentId` ITSELF per covered
/// address WITH NO REPLAY LOCK HELD, and only then commit the reserved nonce. Every failure path
/// FAILS CLOSED, and a denied/unavailable on-chain read burns NOTHING (the reservation aborts on
/// drop). Replaces trust-the-receipt entirely.
///
/// `replay` is the injected node/server replay store (retiring the old process-global singleton).
/// When `None` (fixtures with no replay concern) the verify + on-chain check still run; only the
/// nonce single-use bookkeeping is skipped.
pub fn authorize_access<O: AccessOracle>(
    grant: &AccessGrantV1,
    expected_node_set_id_b64: &str,
    chain_id: u64,
    kid_hex: &str,
    now: u64,
    replay: Option<&ReplayStore>,
    chain: &O,
) -> Result<(), String> {
    let ctx = AccessVerifyContext {
        expected_node_set_id_b64: expected_node_set_id_b64.to_string(),
        expected_chain_id: chain_id,
        expected_kid_hex: kid_hex.to_string(),
        now,
    };
    match replay {
        Some(store) => {
            // Phases 1–4: pure verification (no replay lock held across the EIP-1271 RPC) then a
            // short locked reservation → InFlight. An invalid grant returns here, before `begin`.
            let (verified, reservation) =
                access::verify_and_reserve(grant, &ctx, store, Some(chain as &dyn Eip1271Caller))
                    .map_err(|e| format!("access grant rejected: {e}"))?;
            // Phase 5: the on-chain read runs with NO lock held (the reservation is a plain RAII
            // token, not a lock). Phase 6: commit on success; on ANY error the reservation is
            // dropped → abort → the nonce is not burned and remains retryable.
            evaluate_onchain(&verified, chain)?;
            reservation.commit();
            Ok(())
        }
        None => {
            let verified =
                access::verify_access_grant(grant, &ctx, Some(chain as &dyn Eip1271Caller))
                    .map_err(|e| format!("access grant rejected: {e}"))?;
            evaluate_onchain(&verified, chain)
        }
    }
}

/// Phase 5: the node-side on-chain fan-out. Bound the outbound work BEFORE dispatching any read
/// (DKMS-4), then require at least one covered subject to hold access. `Ok(())` = authorized; every
/// other outcome (no access / unavailable / over-fan-out) is a fail-closed `Err`.
fn evaluate_onchain<O: AccessOracle>(
    verified: &access::VerifiedAccess,
    chain: &O,
) -> Result<(), String> {
    // DKMS-4: the verifier already caps the validated subject set at `MAX_COVERED_ADDRESSES`; this
    // is the explicit, defense-in-depth gate that makes the worst-case outbound work per recover
    // calculable from a named constant — at most `MAX_COVERED_ADDRESSES * rpc_pool_len` `eth_call`s.
    if verified.covered_addresses.len() > access::MAX_COVERED_ADDRESSES {
        crate::counters::incr(&crate::counters::GRANT_LIST_BOUND_REJECTED);
        return Err(format!(
            "covered subject fan-out {} exceeds MAX_COVERED_ADDRESSES {}",
            verified.covered_addresses.len(),
            access::MAX_COVERED_ADDRESSES,
        ));
    }
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

    // ── multi-RPC quorum read (shared combiner, T = bool = hasAccessByContentId answer) ──
    // ── DKMS-3 review fix: startup replay-TTL safety bound ──
    #[test]
    fn startup_ttl_bound_accepts_a_sane_pool_and_rejects_an_oversized_one() {
        // A normal small pool keeps the worst-case begin→commit latency well under the TTL.
        assert!(
            check_replay_ttl_bound(1).is_ok(),
            "a single-endpoint pool is accepted"
        );
        assert!(
            check_replay_ttl_bound(2).is_ok(),
            "a sane two-endpoint pool is accepted"
        );
        // The smallest pool whose worst-case latency reaches/exceeds IN_FLIGHT_TTL_SECONDS must be
        // refused (fail closed), derived from the actual named constants so it tracks their values.
        let per_subject = access::MAX_COVERED_ADDRESSES * ETH_CALL_TIMEOUT_SECONDS as usize;
        let first_bad = IN_FLIGHT_TTL_SECONDS as usize / per_subject + 1;
        assert!(
            check_replay_ttl_bound(first_bad).is_err(),
            "a pool at/over the TTL bound must be refused at startup",
        );
        assert!(
            check_replay_ttl_bound(first_bad * 8).is_err(),
            "an even larger pool stays refused",
        );
    }

    #[test]
    fn quorum_all_true_allows() {
        assert_eq!(
            combine_votes(&[RpcVote::Answered(true), RpcVote::Answered(true)], 2),
            Ok(true)
        );
    }

    #[test]
    fn quorum_all_false_denies() {
        assert_eq!(
            combine_votes(&[RpcVote::Answered(false), RpcVote::Answered(false)], 2),
            Ok(false)
        );
    }

    #[test]
    fn quorum_disagreement_fails_closed() {
        assert!(combine_votes(&[RpcVote::Answered(true), RpcVote::Answered(false)], 2).is_err());
    }

    #[test]
    fn quorum_insufficient_reachability_fails_closed() {
        // pool of 2, only 1 answered → below need(2) → fail closed
        assert!(
            combine_votes(&[RpcVote::Answered(true), RpcVote::<bool>::Unavailable], 2).is_err()
        );
    }

    #[test]
    fn quorum_single_endpoint_pool_ok() {
        assert_eq!(combine_votes(&[RpcVote::Answered(true)], 1), Ok(true));
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
        fn has_access_by_content_id(
            &self,
            _holder: &str,
            _kid16: &[u8; 16],
        ) -> Result<bool, String> {
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
        let chain = MockChain {
            access: Err("rpc disagreement".to_string()),
        };
        let err = authorize_access(&grant, NS, 8453, KID, NOW, None, &chain).unwrap_err();
        assert!(err.contains("failed closed"), "got: {err}");
    }

    #[test]
    fn authorize_rejects_forged_grant_before_touching_chain() {
        let (mut grant, _owner) = signed_grant(NS, KID, 8453, NOW, 3600, &[], 14);
        // tamper the covered set AFTER signing → wallet sig no longer recovers the owner
        grant
            .delegation
            .covered_addresses
            .push("0x000000000000000000000000000000000000dead".into());
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
        let err =
            authorize_access(&grant, "b3RoZXItc2V0", 8453, KID, NOW, None, &chain).unwrap_err();
        assert!(err.contains("access grant rejected"), "got: {err}");
    }

    // ─────────────────────────────────────────────────────────────────────────────────────────
    // Stage 0 regression tests — each one asserts the SECURE behavior and is `#[ignore]`d until
    // the stage that implements it. They are the contract the fixes must turn green; do NOT
    // weaken an assertion to make a stage pass.
    // ─────────────────────────────────────────────────────────────────────────────────────────

    use crate::test_rpc::{CountingOracle, RpcReply, ScriptedRpc};
    use ddrm_envelope::access::testkit::test_wallet_address;

    /// A `NodeChain` over an explicit, test-owned RPC pool (the operator-pinned config the daemon
    /// would read from its env), so an adversarial pool can be assembled without touching env.
    fn chain_over(pool: &[String]) -> NodeChain {
        NodeChain {
            rpc_pool: pool.to_vec(),
            contract: "0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D".to_string(),
            selector: "0x54d42821".to_string(),
            chain_id: 8453,
        }
    }

    /// A syntactically valid 65-byte `0x` signature (its content is irrelevant: the endpoints are
    /// scripted, and the node must not let one of them decide).
    fn any_sig_hex() -> String {
        format!("0x{}", "ab".repeat(65))
    }

    /// DKMS-1 (P0): a wallet signature over a LIST of addresses is not proof that the signer
    /// controls every listed address. Here the owner legitimately signs a delegation that also
    /// lists an unrelated wallet, and only that unrelated wallet holds on-chain access. The
    /// delegation must not be able to borrow a stranger's entitlement: only the signing owner (or
    /// an address whose relation to it was independently authenticated) may satisfy the check.
    #[test]
    fn a_delegation_cannot_borrow_an_unrelated_wallets_entitlement() {
        let owner = test_wallet_address(21);
        let stranger = "0x00000000000000000000000000000000000000ff".to_string();
        assert_ne!(
            owner.to_ascii_lowercase(),
            stranger,
            "the two wallets must differ"
        );
        // The owner really did sign this delegation — the signature is valid, the CONTENT is not
        // trustworthy: it names an address the owner does not control.
        let (grant, signer) = signed_grant(
            NS,
            KID,
            8453,
            NOW,
            3600,
            &[owner.clone(), stranger.clone()],
            21,
        );
        assert_eq!(signer.to_ascii_lowercase(), owner.to_ascii_lowercase());

        // The chain entitles ONLY the stranger; the signing owner holds nothing for this content.
        let chain = CountingOracle::entitled(&[&stranger]);
        let decision = authorize_access(&grant, NS, 8453, KID, NOW, None, &chain);
        assert!(
            decision.is_err(),
            "a delegation signed by an owner with no entitlement must NOT be authorized by an \
             unrelated address the signer merely listed in covered_addresses",
        );
        assert_eq!(
            chain.reads(),
            0,
            "the unrelated address must NEVER be queried when relation validation fails",
        );
        assert_eq!(
            chain.eip1271_calls(),
            0,
            "no smart-account read for the stranger either"
        );
    }

    /// DKMS-4 (P1): `covered_addresses` is unbounded and each entry costs one multi-RPC access
    /// read, so a single grant can amplify into arbitrarily much outbound work. Cardinality (and
    /// encoded size) must be bounded BEFORE any RPC is issued.
    #[test]
    fn an_oversized_covered_set_is_refused_before_any_outbound_rpc() {
        let owner = test_wallet_address(22);
        let mut covered = vec![owner];
        covered.extend((1..=1024u32).map(|i| format!("0x{i:040x}")));
        let (grant, _owner) = signed_grant(NS, KID, 8453, NOW, 3600, &covered, 22);

        let chain = CountingOracle::entitled(&[]);
        let decision = authorize_access(&grant, NS, 8453, KID, NOW, None, &chain);
        assert_eq!(
            chain.reads(),
            0,
            "a grant covering {} addresses must be refused on cardinality BEFORE any on-chain read",
            covered.len(),
        );
        assert_eq!(
            chain.eip1271_calls(),
            0,
            "no smart-account read either, for the same reason"
        );
        assert!(
            decision.is_err(),
            "an unbounded covered set must fail closed"
        );
    }

    /// DKMS-4, made falsifiable: an oversized covered set on a SMART-ACCOUNT grant (owner is a
    /// contract, so EIP-191 recovery yields a DIFFERENT address and the node would otherwise fall
    /// back to an EIP-1271 dial) must be refused on cardinality BEFORE that dial. Absent the
    /// pre-dispatch bound this asserts `eip1271_calls() == 1`; with it, `0` — so the assertion can
    /// actually fail if the bound regresses.
    #[test]
    fn a_smart_account_grant_bounds_fanout_before_any_eip1271_dial() {
        use ddrm_envelope::access::testkit::signed_grant_smart_account;
        let smart_account = "0x00000000000000000000000000000000c0ffee01".to_string();
        let mut covered = vec![smart_account.clone()];
        covered.extend((1..=1024u32).map(|i| format!("0x{i:040x}")));
        let grant =
            signed_grant_smart_account(NS, KID, 8453, NOW, 3600, &smart_account, &covered, 30);

        let chain = CountingOracle::entitled(&[]);
        let decision = authorize_access(&grant, NS, 8453, KID, NOW, None, &chain);
        assert_eq!(
            chain.eip1271_calls(),
            0,
            "an oversized covered set must be refused on cardinality BEFORE the EIP-1271 dial the \
             smart-account owner would otherwise trigger",
        );
        assert_eq!(chain.reads(), 0, "and before any hasAccessByContentId read");
        assert!(
            decision.is_err(),
            "an unbounded covered set must fail closed"
        );
    }

    /// DKMS-4: the maximum on-chain fan-out per recover is calculable from a named constant. An
    /// ACCEPTED owner-only grant queries at most `MAX_COVERED_ADDRESSES` subjects — proven here by
    /// a counting oracle that entitles the owner, so the grant is authorized and the read count is
    /// observable (and bounded), not merely zero-on-rejection.
    #[test]
    fn an_accepted_owner_only_grant_reads_at_most_max_covered_addresses() {
        let owner = test_wallet_address(31);
        let (grant, signer) = signed_grant(NS, KID, 8453, NOW, 3600, &[owner.clone()], 31);
        assert_eq!(signer.to_ascii_lowercase(), owner.to_ascii_lowercase());

        let chain = CountingOracle::entitled(&[&owner]);
        authorize_access(&grant, NS, 8453, KID, NOW, None, &chain)
            .expect("owner-only grant authorized");
        assert!(
            chain.reads() <= access::MAX_COVERED_ADDRESSES,
            "an accepted grant reads at most MAX_COVERED_ADDRESSES ({}) subjects, got {}",
            access::MAX_COVERED_ADDRESSES,
            chain.reads(),
        );
        assert_eq!(
            chain.reads(),
            1,
            "owner-only ⇒ exactly the owner is queried"
        );
    }

    /// DKMS-2 (P0): EIP-1271 takes the FIRST usable reply, so endpoint ORDER decides smart-account
    /// signature validity and a single endpoint can fabricate a `valid` verdict. A disagreeing pool
    /// must fail closed, and the decision must not depend on the order the pool is listed in.
    #[test]
    fn eip1271_disagreement_fails_closed_and_ignores_endpoint_order() {
        let owner = "0x00000000000000000000000000000000c0ffee01";
        let hash = [0x5Au8; 32];
        let sig = any_sig_hex();

        let yes = ScriptedRpc::start(&[RpcReply::Eip1271Magic]);
        let no = ScriptedRpc::start(&[RpcReply::Eip1271NonMagic]);
        let forward = chain_over(&[yes.url(), no.url()]).is_valid_signature(owner, &hash, &sig);
        let yes2 = ScriptedRpc::start(&[RpcReply::Eip1271Magic]);
        let no2 = ScriptedRpc::start(&[RpcReply::Eip1271NonMagic]);
        let reverse = chain_over(&[no2.url(), yes2.url()]).is_valid_signature(owner, &hash, &sig);

        let magic =
            |r: &Option<Vec<u8>>| r.as_deref().map(access::eip1271_is_magic).unwrap_or(false);
        assert!(
            yes.bodies().iter().any(|b| b.contains("eth_call")),
            "sanity: the node really dialed the scripted endpoint",
        );
        assert_eq!(
            magic(&forward),
            magic(&reverse),
            "endpoint ORDER must never decide smart-account signature validity",
        );
        assert!(
            !magic(&forward),
            "a pool that DISAGREES about an EIP-1271 signature must fail closed, never report valid",
        );
        assert!(
            no.calls() >= 1,
            "the configured agreement policy must poll the whole pool, not stop at the first reply",
        );
    }

    /// DKMS-2 (P0), policy parity: the SAME pool, on the SAME chain read semantics, must reach the
    /// same reachability verdict. `hasAccessByContentId` already refuses to decide when only one of
    /// two configured endpoints answers; EIP-1271 happily lets that one endpoint decide.
    #[test]
    fn one_reachable_endpoint_cannot_decide_an_eip1271_signature() {
        let owner = "0x00000000000000000000000000000000c0ffee02";
        let holder = "0x000000000000000000000000000000000000beef";
        let kid16 = access::kid_to_bytes16(KID).unwrap();
        let hash = [0x5Bu8; 32];
        let sig = any_sig_hex();

        // One endpoint answers both reads; its peer is dead. Ordered script: the access read first,
        // then the EIP-1271 read.
        let up = ScriptedRpc::start(&[RpcReply::Bool(true), RpcReply::Eip1271Magic]);
        let down = ScriptedRpc::start(&[RpcReply::TransportError]);
        let chain = chain_over(&[up.url(), down.url()]);

        assert!(
            chain.has_access_by_content_id(holder, &kid16).is_err(),
            "baseline: the access read already refuses to decide on 1 of 2 reachable endpoints",
        );
        let verdict = chain.is_valid_signature(owner, &hash, &sig);
        assert!(
            !verdict
                .as_deref()
                .map(access::eip1271_is_magic)
                .unwrap_or(false),
            "EIP-1271 must obey the SAME reachability/agreement policy as the access read: one \
             endpoint out of a configured two can never decide a smart-account signature is valid",
        );
    }

    /// DKMS-2 (P0), adversarial shapes: a malformed reply must not END the poll (letting one lying
    /// endpoint force a verdict), and an endpoint that never answers must not hand the decision to
    /// the single endpoint that did. SLOW: the timeout leg waits out the node's 8s `eth_call`
    /// timeout on purpose — that is the behavior under test.
    #[test]
    fn a_malformed_or_silent_endpoint_cannot_decide_an_eip1271_signature() {
        let owner = "0x00000000000000000000000000000000c0ffee03";
        let hash = [0x5Cu8; 32];
        let sig = any_sig_hex();
        let magic =
            |r: &Option<Vec<u8>>| r.as_deref().map(access::eip1271_is_magic).unwrap_or(false);

        // (a) a malformed reply from the first endpoint currently ends the poll outright.
        let junk = ScriptedRpc::start(&[RpcReply::Malformed]);
        let peer = ScriptedRpc::start(&[RpcReply::Eip1271Magic]);
        let _ = chain_over(&[junk.url(), peer.url()]).is_valid_signature(owner, &hash, &sig);
        assert!(
            peer.calls() >= 1,
            "a malformed reply from one endpoint must not end the poll before its peers answer",
        );

        // (b) a revert is a legitimate 'not valid' answer, but it is still ONE endpoint's answer.
        let reverts = ScriptedRpc::start(&[RpcReply::Revert]);
        let agrees = ScriptedRpc::start(&[RpcReply::Eip1271NonMagic]);
        let both_say_no =
            chain_over(&[reverts.url(), agrees.url()]).is_valid_signature(owner, &hash, &sig);
        assert!(
            !magic(&both_say_no),
            "an agreeing 'not valid' pool stays not valid"
        );

        // (c) an endpoint that never answers leaves the pool under-reachable; the one endpoint that
        //     did answer must not be allowed to decide.
        let silent = ScriptedRpc::start(&[RpcReply::Timeout]);
        let lone = ScriptedRpc::start(&[RpcReply::Eip1271Magic]);
        let verdict =
            chain_over(&[silent.url(), lone.url()]).is_valid_signature(owner, &hash, &sig);
        assert!(
            !magic(&verdict),
            "an unreachable endpoint must make the pool under-reachable and FAIL CLOSED, not \
             promote the single reachable endpoint's `magic` to a verdict",
        );
    }

    /// DKMS-2 (P0), the remaining agreement-matrix cells the plan names, on the live
    /// `is_valid_signature` path over real scripted endpoints: a UNANIMOUS pool decides
    /// (both ways), any valid/invalid split fails closed regardless of which side comes
    /// first, and a single-endpoint pool is decided by that one endpoint (the explicit
    /// `need = min(2, pool_len).max(1)` policy the access read already uses).
    #[test]
    fn eip1271_agreement_matrix_matches_the_shared_policy() {
        let owner = "0x00000000000000000000000000000000c0ffee04";
        let hash = [0x5Du8; 32];
        let sig = any_sig_hex();
        let magic =
            |r: &Option<Vec<u8>>| r.as_deref().map(access::eip1271_is_magic).unwrap_or(false);

        // [magic, magic] ⇒ the pool agrees the signature is valid.
        let m1 = ScriptedRpc::start(&[RpcReply::Eip1271Magic]);
        let m2 = ScriptedRpc::start(&[RpcReply::Eip1271Magic]);
        let allow = chain_over(&[m1.url(), m2.url()]).is_valid_signature(owner, &hash, &sig);
        assert!(magic(&allow), "a unanimous magic pool must report valid");
        assert!(
            m1.calls() >= 1 && m2.calls() >= 1,
            "both endpoints must be polled, not just the first"
        );

        // [nonmagic, nonmagic] ⇒ the pool agrees the signature is NOT valid.
        let n1 = ScriptedRpc::start(&[RpcReply::Eip1271NonMagic]);
        let n2 = ScriptedRpc::start(&[RpcReply::Eip1271NonMagic]);
        let deny = chain_over(&[n1.url(), n2.url()]).is_valid_signature(owner, &hash, &sig);
        assert!(
            !magic(&deny),
            "a unanimous non-magic pool must report NOT valid"
        );

        // first magic / second revert ⇒ valid-vs-invalid split ⇒ fail closed.
        let fm = ScriptedRpc::start(&[RpcReply::Eip1271Magic]);
        let sr = ScriptedRpc::start(&[RpcReply::Revert]);
        let split_a = chain_over(&[fm.url(), sr.url()]).is_valid_signature(owner, &hash, &sig);
        assert!(
            !magic(&split_a),
            "a first-magic then-revert split must fail closed"
        );

        // first revert / second magic ⇒ same split, other order ⇒ still fail closed.
        let fr = ScriptedRpc::start(&[RpcReply::Revert]);
        let sm = ScriptedRpc::start(&[RpcReply::Eip1271Magic]);
        let split_b = chain_over(&[fr.url(), sm.url()]).is_valid_signature(owner, &hash, &sig);
        assert!(
            !magic(&split_b),
            "a first-revert then-magic split must fail closed too"
        );

        // one-endpoint pool: the configured policy needs exactly one reachable endpoint,
        // so that single endpoint legitimately decides (and its peer count is 1, not 2).
        let solo_yes = ScriptedRpc::start(&[RpcReply::Eip1271Magic]);
        let one_valid = chain_over(&[solo_yes.url()]).is_valid_signature(owner, &hash, &sig);
        assert!(
            magic(&one_valid),
            "a single-endpoint pool is decided by its one endpoint (valid)"
        );
        let solo_no = ScriptedRpc::start(&[RpcReply::Eip1271NonMagic]);
        let one_invalid = chain_over(&[solo_no.url()]).is_valid_signature(owner, &hash, &sig);
        assert!(
            !magic(&one_invalid),
            "a single non-magic endpoint decides NOT valid"
        );
    }
}
