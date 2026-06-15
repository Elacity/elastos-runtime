//! Wallet-signed access-delegation primitives — the trustless authorization
//! object for the PQ dKMS quorum (PC2 `SecureViewDelegation` parity).
//!
//! Mirrors `~/.pc2/pc2-node/src/utils/secureViewSession.ts`: a user's EVM wallet
//! `personal_sign`s (EIP-191) a canonical, sorted-key JSON delegation of an
//! ephemeral session key over a bounded window and a set of covered addresses;
//! every recover carries a per-request signature by that session key. Each dKMS
//! node verifies BOTH signatures itself and then evaluates the on-chain access
//! condition (`hasAccessByContentId`) — so authorization needs no enrollment and
//! no trusted gateway.
//!
//! This module is the shared, single-source-of-truth verifier (client + node +
//! tests use the exact same canonical encoder, so they cannot drift). It does NOT
//! perform the on-chain check — that is the node's job (W2), deliberately, exactly
//! as PC2 leaves `hasAccessByContentId` to the Lit Action inside the TEE.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha3::{Digest, Keccak256};

/// Domain tag the wallet signs over (delegation). Domain-separated from the request.
pub const DELEGATION_DOMAIN: &str = "elastos.ddrm.access.v1";
/// Domain tag the session key signs over (per recover request).
pub const REQUEST_DOMAIN: &str = "elastos.ddrm.access.request.v1";
/// Max delegation window (PC2 parity: 24h).
pub const MAX_DELEGATION_WINDOW_SECONDS: u64 = 24 * 3600;
/// Max +/- clock skew on per-request freshness (PC2 parity: 60s).
pub const REQUEST_FRESHNESS_WINDOW_SECONDS: u64 = 60;
/// Leeway for `issued_at > now` to absorb minor skew (PC2 parity: 5s).
pub const DELEGATION_CLOCK_SKEW_SECONDS: u64 = 5;
/// EIP-1271 magic value (bytes4 selector of `isValidSignature`) — success marker.
pub const EIP1271_MAGIC_VALUE: [u8; 4] = [0x16, 0x26, 0xba, 0x7e];

/// Why a verify failed — every variant FAILS CLOSED. Mirrors PC2's `VerifyErrorCode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessVerifyError {
    BadDomain,
    BadChain,
    BadNodeSet,
    BadKid,
    DelNotYetValid,
    DelExpired,
    DelWindowTooWide,
    ReqStaleOrFuture,
    DelSigInvalid,
    ReqSigInvalid,
    DelMalformed,
    ReqMalformed,
    PubMalformed,
    Replayed,
    Revoked,
}

impl core::fmt::Display for AccessVerifyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        use AccessVerifyError::*;
        let s = match self {
            BadDomain => "bad_domain",
            BadChain => "bad_chain",
            BadNodeSet => "bad_node_set",
            BadKid => "bad_kid",
            DelNotYetValid => "del_not_yet_valid",
            DelExpired => "del_expired",
            DelWindowTooWide => "del_window_too_wide",
            ReqStaleOrFuture => "req_stale_or_future",
            DelSigInvalid => "del_sig_invalid",
            ReqSigInvalid => "req_sig_invalid",
            DelMalformed => "del_malformed",
            ReqMalformed => "req_malformed",
            PubMalformed => "pub_malformed",
            Replayed => "replayed",
            Revoked => "revoked",
        };
        f.write_str(s)
    }
}

impl std::error::Error for AccessVerifyError {}

/// The wallet-signed delegation (PC2 `SecureViewDelegation`). The owner's EVM
/// wallet signs `canonical()` of this struct via EIP-191 `personal_sign`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessDelegationV1 {
    /// Must equal [`DELEGATION_DOMAIN`].
    pub domain: String,
    /// EVM chain id the on-chain access lives on (8453 = Base).
    pub chain_id: u64,
    /// `0x` + 32 hex — the `bytes16` contentId this delegation authorizes.
    pub kid_hex: String,
    /// Binds the grant to THIS quorum node-set (anti cross-quorum replay).
    pub node_set_id_b64: String,
    /// The wallet that signs this delegation (checksummed/0x-lowercased EVM address).
    pub owner_address: String,
    /// Every address whose on-chain access counts (EOA + smart account(s)).
    pub covered_addresses: Vec<String>,
    /// base64 of the ephemeral session verifying key (ML-DSA-65) the runtime holds.
    pub session_pub_b64: String,
    pub issued_at: u64,
    pub expires_at: u64,
    /// base64 random nonce — the revocation/replay handle for the whole delegation.
    pub nonce_b64: String,
}

/// The per-recover request (PC2 `SecureViewRequest`). Signed by the session key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessRequestV1 {
    /// Must equal [`REQUEST_DOMAIN`].
    pub domain: String,
    /// `0x` + 32 hex — must match the delegation's `kid_hex`.
    pub kid_hex: String,
    /// Must match the delegation's `node_set_id_b64`.
    pub node_set_id_b64: String,
    pub requested_at: u64,
    /// base64 random per-request nonce (single-use within the delegation window).
    pub request_nonce_b64: String,
}

/// The full bundle a runtime hands a node on `recover` (PC2 `VerifyBundle`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessGrantV1 {
    pub delegation: AccessDelegationV1,
    /// `0x` + 130 hex — the wallet's EIP-191 signature over `delegation.canonical()`.
    pub delegation_sig_hex: String,
    pub request: AccessRequestV1,
    /// base64 ML-DSA-65 signature (by the session key) over `request.canonical()`.
    pub request_sig_b64: String,
}

/// On success, what the node needs to run the on-chain check itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedAccess {
    /// The address recovered from the delegation signature (0x-lowercased).
    pub recovered_owner: String,
    /// Every address the node must `hasAccessByContentId`-check (>= 1 true ⇒ allow).
    pub covered_addresses: Vec<String>,
    /// `0x` + 32 hex content id (kid), normalized.
    pub kid_hex: String,
}

/// Canonical JSON — sorted keys, no whitespace, recursive. Byte-identical to PC2's
/// `canonicalize()` (`secureViewSession.ts`), so a delegation built here verifies
/// against a signature produced anywhere that uses the same algorithm.
fn canonical_json(v: &Value) -> String {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let inner: Vec<String> = keys
                .into_iter()
                .map(|k| {
                    let key = serde_json::to_string(k).expect("string key is serializable");
                    format!("{key}:{}", canonical_json(&map[k]))
                })
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        Value::Array(arr) => {
            let inner: Vec<String> = arr.iter().map(canonical_json).collect();
            format!("[{}]", inner.join(","))
        }
        other => serde_json::to_string(other).expect("scalar is serializable"),
    }
}

impl AccessDelegationV1 {
    /// The exact bytes the wallet `personal_sign`s.
    pub fn canonical(&self) -> String {
        canonical_json(&serde_json::to_value(self).expect("delegation serializes"))
    }
}

impl AccessRequestV1 {
    /// The exact bytes the session key signs.
    pub fn canonical(&self) -> String {
        canonical_json(&serde_json::to_value(self).expect("request serializes"))
    }
}

// ── Client-side assembly (the runtime builds grants; the wallet signs once) ───

/// CLIENT: construct the delegation the WALLET will sign. The runtime fills the binding
/// (its freshly-minted session verifying key + the covered addresses + kid + this quorum's
/// node-set + a bounded window + a fresh nonce); `.canonical()` is then handed to
/// `wallet-provider` for an EIP-191 `personal_sign`. PURE — no signing here.
#[allow(clippy::too_many_arguments)]
pub fn build_delegation(
    chain_id: u64,
    kid_hex: &str,
    node_set_id_b64: &str,
    owner_address: &str,
    covered_addresses: Vec<String>,
    session_pub_b64: &str,
    issued_at: u64,
    window_secs: u64,
    nonce_b64: &str,
) -> AccessDelegationV1 {
    AccessDelegationV1 {
        domain: DELEGATION_DOMAIN.to_string(),
        chain_id,
        kid_hex: kid_hex.to_string(),
        node_set_id_b64: node_set_id_b64.to_string(),
        owner_address: owner_address.to_string(),
        covered_addresses,
        session_pub_b64: session_pub_b64.to_string(),
        issued_at,
        expires_at: issued_at.saturating_add(window_secs),
        nonce_b64: nonce_b64.to_string(),
    }
}

/// CLIENT: a fresh per-recover request, bound to the same kid + node-set as the delegation.
pub fn build_request(kid_hex: &str, node_set_id_b64: &str, requested_at: u64, request_nonce_b64: &str) -> AccessRequestV1 {
    AccessRequestV1 {
        domain: REQUEST_DOMAIN.to_string(),
        kid_hex: kid_hex.to_string(),
        node_set_id_b64: node_set_id_b64.to_string(),
        requested_at,
        request_nonce_b64: request_nonce_b64.to_string(),
    }
}

/// CLIENT: sign a request's canonical bytes with the ML-DSA session signer (the half the
/// runtime holds for this open). Returns base64.
pub fn sign_request<S: crate::seal::CekSealSigner>(session_signer: &S, request: &AccessRequestV1) -> String {
    b64_encode(&session_signer.sign(request.canonical().as_bytes()))
}

/// CLIENT: assemble the wire grant from the delegation + the wallet's EIP-191 signature over
/// `delegation.canonical()` + the per-request object signed by the session key.
pub fn assemble_grant<S: crate::seal::CekSealSigner>(
    delegation: AccessDelegationV1,
    delegation_sig_hex: String,
    request: AccessRequestV1,
    session_signer: &S,
) -> AccessGrantV1 {
    let request_sig_b64 = sign_request(session_signer, &request);
    AccessGrantV1 { delegation, delegation_sig_hex, request, request_sig_b64 }
}

// ── EIP-191 (secp256k1 personal_sign) ────────────────────────────────────────

fn keccak256(bytes: &[u8]) -> [u8; 32] {
    let mut h = Keccak256::new();
    h.update(bytes);
    h.finalize().into()
}

/// The Ethereum signed-message hash of `msg`: `keccak256("\x19Ethereum Signed
/// Message:\n" + len(msg) + msg)`. This is what both EIP-191 recovery and the
/// EIP-1271 `isValidSignature` hash argument use.
pub fn eip191_message_hash(msg: &[u8]) -> [u8; 32] {
    let mut buf = Vec::with_capacity(64 + msg.len());
    buf.extend_from_slice(b"\x19Ethereum Signed Message:\n");
    buf.extend_from_slice(msg.len().to_string().as_bytes());
    buf.extend_from_slice(msg);
    keccak256(&buf)
}

fn strip_0x(s: &str) -> &str {
    s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s)
}

/// Lowercased `0x` address from an secp256k1 SEC1-uncompressed public key
/// (`0x04 || X || Y`, 65 bytes): `0x` + last 20 bytes of `keccak256(X || Y)`.
fn address_from_uncompressed_pubkey(pub65: &[u8]) -> Option<String> {
    if pub65.len() != 65 || pub65[0] != 0x04 {
        return None;
    }
    let h = keccak256(&pub65[1..]);
    Some(format!("0x{}", hex::encode(&h[12..])))
}

/// Recover the signing EVM address from an EIP-191 `personal_sign` over
/// `canonical` text. `sig_hex` is `0x` + 130 hex (r ‖ s ‖ v). Returns the
/// lowercased `0x` address, or an error if the signature is malformed/unrecoverable.
pub fn recover_eip191(canonical: &str, sig_hex: &str) -> Result<String, AccessVerifyError> {
    use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};

    let sig_bytes = hex::decode(strip_0x(sig_hex)).map_err(|_| AccessVerifyError::DelSigInvalid)?;
    if sig_bytes.len() != 65 {
        return Err(AccessVerifyError::DelSigInvalid);
    }
    // v is 27/28 (or 0/1) — normalize to a 0/1 recovery id.
    let v = sig_bytes[64];
    let rec = match v {
        0 | 1 => v,
        27 | 28 => v - 27,
        _ => return Err(AccessVerifyError::DelSigInvalid),
    };
    let recovery_id = RecoveryId::from_byte(rec).ok_or(AccessVerifyError::DelSigInvalid)?;
    let signature = Signature::from_slice(&sig_bytes[..64]).map_err(|_| AccessVerifyError::DelSigInvalid)?;
    let prehash = eip191_message_hash(canonical.as_bytes());
    let vk = VerifyingKey::recover_from_prehash(&prehash, &signature, recovery_id)
        .map_err(|_| AccessVerifyError::DelSigInvalid)?;
    let point = vk.to_encoded_point(false);
    address_from_uncompressed_pubkey(point.as_bytes()).ok_or(AccessVerifyError::DelSigInvalid)
}

fn eq_addr(a: &str, b: &str) -> bool {
    strip_0x(a).eq_ignore_ascii_case(strip_0x(b))
}

// ── EIP-1271 (smart-account owners) — interface + calldata, node wires eth_call ──

/// ABI calldata for `isValidSignature(bytes32 hash, bytes signature)` — the node's
/// chain client sends this to the owner contract and checks the return is the magic
/// value. Built here so the node never re-implements the encoding (anti-drift).
pub fn eip1271_calldata(message_hash: &[u8; 32], sig_hex: &str) -> Result<Vec<u8>, AccessVerifyError> {
    let sig = hex::decode(strip_0x(sig_hex)).map_err(|_| AccessVerifyError::DelSigInvalid)?;
    let mut data = Vec::with_capacity(4 + 32 + 32 + 32 + sig.len() + 32);
    data.extend_from_slice(&[0x16, 0x26, 0xba, 0x7e]); // selector isValidSignature(bytes32,bytes)
    data.extend_from_slice(message_hash); // arg0: bytes32 hash
    let mut offset = [0u8; 32]; // arg1: offset to the dynamic bytes (0x40)
    offset[31] = 0x40;
    data.extend_from_slice(&offset);
    let mut len = [0u8; 32]; // bytes length
    len[24..].copy_from_slice(&(sig.len() as u64).to_be_bytes());
    data.extend_from_slice(&len);
    data.extend_from_slice(&sig);
    let pad = (32 - (sig.len() % 32)) % 32; // right-pad to a 32-byte boundary
    data.extend(std::iter::repeat(0u8).take(pad));
    Ok(data)
}

/// `true` iff an `isValidSignature` return begins with the EIP-1271 magic bytes4.
pub fn eip1271_is_magic(ret: &[u8]) -> bool {
    ret.len() >= 4 && ret[..4] == EIP1271_MAGIC_VALUE
}

/// The node's chain client implements this to support smart-account (EIP-1271)
/// owners. `is_valid_signature` does an `eth_call` of [`eip1271_calldata`] against
/// `owner` and returns the raw return bytes, or `None` on revert/transport error.
/// For W1 this is a pure interface; the live `eth_call` is wired in W2/Day 4-5.
pub trait Eip1271Caller {
    fn is_valid_signature(&self, owner: &str, message_hash: &[u8; 32], sig_hex: &str) -> Option<Vec<u8>>;
}

// ── Per-request signature (ML-DSA-65 session key) ────────────────────────────

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(s).ok()
}

/// base64-encode bytes with the STANDARD alphabet (the session_pub / request_sig form).
pub fn b64_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Verify the per-request signature: the session key (`session_pub_b64`, a base64
/// ML-DSA-65 verifying key) signed `canonical_request`. Fails closed on any
/// malformed input. ML-DSA-65 (FIPS 204) is the same PQ primitive the dKMS channel
/// already uses; the session key only AUTHORIZES, it never guards CEK material.
pub fn verify_request_sig(session_pub_b64: &str, canonical_request: &str, request_sig_b64: &str) -> bool {
    use crate::CekSealVerifier;
    let vk = match b64_decode(session_pub_b64) {
        Some(b) => b,
        None => return false,
    };
    let sig = match b64_decode(request_sig_b64) {
        Some(b) => b,
        None => return false,
    };
    match crate::MlDsa65Verifier::from_encoded(&vk) {
        Some(v) => v.verify(canonical_request.as_bytes(), &sig),
        None => false,
    }
}

// ── Anti-replay + revocation (node-held state; PC2's two maps) ────────────────

/// Per-process revocation + per-request single-use nonce tracking, mirroring PC2's
/// `revokedDelegations` + `seenRequestNonces`. The node holds one of these; the
/// quorum's PQ channel + the delegation window bound the rest.
#[derive(Default)]
pub struct ReplayGuard {
    revoked: std::collections::HashMap<String, u64>,
    seen: std::collections::HashMap<String, u64>,
}

impl ReplayGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Revoke a delegation (by its nonce) until its natural expiry.
    pub fn revoke(&mut self, delegation_nonce_b64: &str, expires_at: u64) {
        self.revoked.insert(delegation_nonce_b64.to_string(), expires_at);
    }

    fn is_revoked(&mut self, delegation_nonce_b64: &str, now: u64) -> bool {
        match self.revoked.get(delegation_nonce_b64).copied() {
            None => false,
            Some(exp) if exp < now => {
                self.revoked.remove(delegation_nonce_b64);
                false
            }
            Some(_) => true,
        }
    }

    /// Record `(delegation_nonce, request_nonce)`; `Err(Replayed)` if already seen,
    /// `Err(Revoked)` if the delegation was revoked. Cheap — call before sig math.
    pub fn check_and_record(
        &mut self,
        delegation_nonce_b64: &str,
        request_nonce_b64: &str,
        expires_at: u64,
        now: u64,
    ) -> Result<(), AccessVerifyError> {
        if self.is_revoked(delegation_nonce_b64, now) {
            return Err(AccessVerifyError::Revoked);
        }
        let key = format!("{delegation_nonce_b64}:{request_nonce_b64}");
        if self.seen.contains_key(&key) {
            return Err(AccessVerifyError::Replayed);
        }
        self.seen.insert(key, expires_at);
        Ok(())
    }
}

// ── Orchestration ────────────────────────────────────────────────────────────

/// What the node expects this grant to be bound to (its own quorum + the requested
/// content + the wall clock). Mirrors PC2's `VerifyContext`.
pub struct AccessVerifyContext {
    pub expected_node_set_id_b64: String,
    pub expected_chain_id: u64,
    /// `0x` + 32 hex (or bare 32 hex) — the kid the recover is for.
    pub expected_kid_hex: String,
    pub now: u64,
}

fn is_evm_address(s: &str) -> bool {
    let h = strip_0x(s);
    h.len() == 40 && h.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Normalize a kid to lowercased `0x` + 32 hex; `None` if not a 16-byte hex id.
fn normalize_kid(s: &str) -> Option<String> {
    let h = strip_0x(s);
    if h.len() == 32 && h.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(format!("0x{}", h.to_ascii_lowercase()))
    } else {
        None
    }
}

/// Structural + cryptographic verification of an access grant — every check fails
/// closed. Mirrors PC2's `verifySecureViewBundle`. Deliberately does NOT run the
/// on-chain `hasAccessByContentId`: that is the node's job (W2), per covered address
/// in the returned [`VerifiedAccess`]. `replay` is optional node-held state; `eip1271`
/// is optional smart-account support.
pub fn verify_access_grant(
    grant: &AccessGrantV1,
    ctx: &AccessVerifyContext,
    replay: Option<&mut ReplayGuard>,
    eip1271: Option<&dyn Eip1271Caller>,
) -> Result<VerifiedAccess, AccessVerifyError> {
    use AccessVerifyError::*;
    let del = &grant.delegation;
    let req = &grant.request;

    // ── structural: delegation ──
    if del.domain != DELEGATION_DOMAIN {
        return Err(BadDomain);
    }
    if del.chain_id != ctx.expected_chain_id {
        return Err(BadChain);
    }
    if del.node_set_id_b64 != ctx.expected_node_set_id_b64 {
        return Err(BadNodeSet);
    }
    let kid = normalize_kid(&del.kid_hex).ok_or(BadKid)?;
    let expected_kid = normalize_kid(&ctx.expected_kid_hex).ok_or(BadKid)?;
    if kid != expected_kid {
        return Err(BadKid);
    }
    if !is_evm_address(&del.owner_address) {
        return Err(DelMalformed);
    }
    if del.covered_addresses.is_empty() || !del.covered_addresses.iter().all(|a| is_evm_address(a)) {
        return Err(DelMalformed);
    }
    if del.session_pub_b64.is_empty() || b64_decode(&del.session_pub_b64).is_none() {
        return Err(PubMalformed);
    }
    if del.nonce_b64.is_empty() || b64_decode(&del.nonce_b64).is_none() {
        return Err(DelMalformed);
    }
    if del.expires_at < del.issued_at {
        return Err(DelMalformed);
    }
    if ctx.now + DELEGATION_CLOCK_SKEW_SECONDS < del.issued_at {
        return Err(DelNotYetValid);
    }
    if ctx.now > del.expires_at {
        return Err(DelExpired);
    }
    if del.expires_at - del.issued_at > MAX_DELEGATION_WINDOW_SECONDS {
        return Err(DelWindowTooWide);
    }

    // ── structural: request, bound to the delegation ──
    if req.domain != REQUEST_DOMAIN {
        return Err(BadDomain);
    }
    let req_kid = normalize_kid(&req.kid_hex).ok_or(BadKid)?;
    if req_kid != kid {
        return Err(BadKid);
    }
    if req.node_set_id_b64 != del.node_set_id_b64 {
        return Err(BadNodeSet);
    }
    if req.request_nonce_b64.is_empty() || b64_decode(&req.request_nonce_b64).is_none() {
        return Err(ReqMalformed);
    }
    let skew = ctx.now.abs_diff(req.requested_at);
    if skew > REQUEST_FRESHNESS_WINDOW_SECONDS {
        return Err(ReqStaleOrFuture);
    }

    // ── revocation + replay (cheap, before sig math) ──
    if let Some(guard) = replay {
        guard.check_and_record(&del.nonce_b64, &req.request_nonce_b64, del.expires_at, ctx.now)?;
    }

    // ── delegation signature: EIP-191 first, EIP-1271 fallback (smart accounts) ──
    let canonical_del = del.canonical();
    let recovered = recover_eip191(&canonical_del, &grant.delegation_sig_hex).ok();
    let mut owner_ok = recovered.as_deref().map(|r| eq_addr(r, &del.owner_address)).unwrap_or(false);
    let mut recovered_owner = recovered.unwrap_or_default();
    if !owner_ok {
        if let Some(caller) = eip1271 {
            let hash = eip191_message_hash(canonical_del.as_bytes());
            if let Some(ret) = caller.is_valid_signature(&del.owner_address, &hash, &grant.delegation_sig_hex) {
                if eip1271_is_magic(&ret) {
                    owner_ok = true;
                    recovered_owner = strip_0x(&del.owner_address).to_ascii_lowercase();
                    recovered_owner.insert_str(0, "0x");
                }
            }
        }
    }
    if !owner_ok {
        return Err(DelSigInvalid);
    }

    // ── per-request signature (session key authorized by the delegation) ──
    if !verify_request_sig(&del.session_pub_b64, &req.canonical(), &grant.request_sig_b64) {
        return Err(ReqSigInvalid);
    }

    Ok(VerifiedAccess {
        recovered_owner,
        covered_addresses: del.covered_addresses.clone(),
        kid_hex: kid,
    })
}

// ── On-chain access ABI (shared so the node never re-implements the encoding) ──

/// ABI calldata for `hasAccessByContentId(address holder, bytes16 contentId) -> bool`
/// — the real Base AuthorityGateway method (selector `0x54d42821`, confirmed against
/// PC2 `contracts/abis.ts`). `selector_hex` is `0x` + 8 hex; `holder` is an EVM
/// address; `kid16` is the 16-byte contentId. Two static words: holder (right-aligned)
/// then contentId (left-aligned). PURE — the node's chain client sends this via eth_call.
pub fn has_access_calldata(selector_hex: &str, holder: &str, kid16: &[u8; 16]) -> Result<Vec<u8>, AccessVerifyError> {
    let sel = hex::decode(strip_0x(selector_hex)).map_err(|_| AccessVerifyError::BadChain)?;
    if sel.len() != 4 {
        return Err(AccessVerifyError::BadChain);
    }
    let addr = hex::decode(strip_0x(holder)).map_err(|_| AccessVerifyError::DelMalformed)?;
    if addr.len() != 20 {
        return Err(AccessVerifyError::DelMalformed);
    }
    let mut data = Vec::with_capacity(4 + 32 + 32);
    data.extend_from_slice(&sel);
    let mut holder_word = [0u8; 32];
    holder_word[12..].copy_from_slice(&addr);
    data.extend_from_slice(&holder_word);
    let mut kid_word = [0u8; 32];
    kid_word[..16].copy_from_slice(kid16);
    data.extend_from_slice(&kid_word);
    Ok(data)
}

/// Decode an EVM `bool` return (32-byte word, low byte 0/1). `None` if malformed —
/// the node treats that as fail-closed (no `true` ever fabricated from junk).
pub fn decode_evm_bool(ret: &[u8]) -> Option<bool> {
    if ret.len() != 32 || ret[..31].iter().any(|b| *b != 0) {
        return None;
    }
    match ret[31] {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

/// Parse a `0x`+32-hex kid into the 16-byte contentId, or `None`.
pub fn kid_to_bytes16(kid_hex: &str) -> Option<[u8; 16]> {
    let h = strip_0x(kid_hex);
    if h.len() != 32 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let raw = hex::decode(h).ok()?;
    let mut out = [0u8; 16];
    out.copy_from_slice(&raw);
    Some(out)
}

/// Dev/test helpers to build fully-signed grants. NOT used on the production path
/// (the runtime builds real grants from the user's wallet + a minted session key);
/// available to downstream crates so they can exercise the node's authorize path.
pub mod testkit {
    use super::*;
    use crate::seal::{mldsa_seal_keypair, CekSealSigner};
    use k256::ecdsa::SigningKey;

    /// The EVM address of the deterministic test wallet derived from `seed` — so a caller can
    /// learn the owner address BEFORE building a delegation it will later sign. Pairs with
    /// [`personal_sign_canonical`].
    pub fn test_wallet_address(seed: u8) -> String {
        let sk = SigningKey::from_slice(&[seed.max(1); 32]).expect("valid scalar");
        let point = sk.verifying_key().to_encoded_point(false);
        address_from_uncompressed_pubkey(point.as_bytes()).expect("addr")
    }

    /// EIP-191 `personal_sign` an ARBITRARY canonical string with the test wallet derived from
    /// `seed`, exactly as MetaMask would. Lets a downstream crate (e.g. the gateway's grant
    /// builder) sign a delegation IT constructed — exercising the real browser→gateway→node path
    /// without a browser or a `k256` dependency. Returns the `0x`-hex signature.
    pub fn personal_sign_canonical(seed: u8, canonical: &str) -> String {
        let sk = SigningKey::from_slice(&[seed.max(1); 32]).expect("valid scalar");
        let prehash = eip191_message_hash(canonical.as_bytes());
        let (sig, recid) = sk.sign_prehash_recoverable(&prehash).expect("sign");
        let mut sb = sig.to_bytes().to_vec();
        sb.push(recid.to_byte() + 27);
        format!("0x{}", hex::encode(sb))
    }

    /// Build a valid `AccessGrantV1` for a freshly-minted wallet + ML-DSA session key.
    /// Returns the grant and the owner address it recovers to.
    #[allow(clippy::too_many_arguments)]
    pub fn signed_grant(
        node_set_id_b64: &str,
        kid_hex: &str,
        chain_id: u64,
        now: u64,
        window_secs: u64,
        covered: &[String],
        seed: u8,
    ) -> (AccessGrantV1, String) {
        let sk = SigningKey::from_slice(&[seed.max(1); 32]).expect("valid scalar");
        let point = sk.verifying_key().to_encoded_point(false);
        let owner = address_from_uncompressed_pubkey(point.as_bytes()).expect("addr");
        let (signer, session_vk) = mldsa_seal_keypair([seed.max(2); 32]);
        let covered = if covered.is_empty() { vec![owner.clone()] } else { covered.to_vec() };
        let del = AccessDelegationV1 {
            domain: DELEGATION_DOMAIN.to_string(),
            chain_id,
            kid_hex: kid_hex.to_string(),
            node_set_id_b64: node_set_id_b64.to_string(),
            owner_address: owner.clone(),
            covered_addresses: covered,
            session_pub_b64: b64_encode(&session_vk),
            issued_at: now,
            expires_at: now + window_secs,
            nonce_b64: b64_encode(&[seed; 16]),
        };
        let prehash = eip191_message_hash(del.canonical().as_bytes());
        let (sig, recid) = sk.sign_prehash_recoverable(&prehash).expect("sign");
        let mut sb = sig.to_bytes().to_vec();
        sb.push(recid.to_byte() + 27);
        let delegation_sig_hex = format!("0x{}", hex::encode(sb));
        let req = AccessRequestV1 {
            domain: REQUEST_DOMAIN.to_string(),
            kid_hex: kid_hex.to_string(),
            node_set_id_b64: node_set_id_b64.to_string(),
            requested_at: now,
            request_nonce_b64: b64_encode(&[seed.wrapping_add(1); 8]),
        };
        let request_sig_b64 = b64_encode(&signer.sign(req.canonical().as_bytes()));
        (
            AccessGrantV1 { delegation: del, delegation_sig_hex, request: req, request_sig_b64 },
            owner,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seal::{mldsa_seal_keypair, CekSealSigner};
    use k256::ecdsa::SigningKey;

    const NOW: u64 = 1_000_000;

    fn wallet(seed: u8) -> (SigningKey, String) {
        let sk = SigningKey::from_slice(&[seed.max(1); 32]).expect("valid scalar");
        let point = sk.verifying_key().to_encoded_point(false);
        let addr = address_from_uncompressed_pubkey(point.as_bytes()).expect("addr");
        (sk, addr)
    }

    fn sign_eip191(sk: &SigningKey, canonical: &str) -> String {
        let prehash = eip191_message_hash(canonical.as_bytes());
        let (sig, recid) = sk.sign_prehash_recoverable(&prehash).expect("sign");
        let mut bytes = sig.to_bytes().to_vec();
        bytes.push(recid.to_byte() + 27);
        format!("0x{}", hex::encode(bytes))
    }

    fn node_set() -> String {
        b64_encode(b"test-node-set-A")
    }

    fn kid() -> String {
        format!("0x{}", "ab".repeat(16))
    }

    /// Build a fully valid, signed grant + a matching verify context.
    fn mk_grant() -> (AccessGrantV1, AccessVerifyContext) {
        let (sk, owner) = wallet(11);
        let (signer, session_vk) = mldsa_seal_keypair([5u8; 32]);
        let del = AccessDelegationV1 {
            domain: DELEGATION_DOMAIN.to_string(),
            chain_id: 8453,
            kid_hex: kid(),
            node_set_id_b64: node_set(),
            owner_address: owner.clone(),
            covered_addresses: vec![owner.clone()],
            session_pub_b64: b64_encode(&session_vk),
            issued_at: NOW,
            expires_at: NOW + 3600,
            nonce_b64: b64_encode(&[1u8; 16]),
        };
        let delegation_sig_hex = sign_eip191(&sk, &del.canonical());
        let req = AccessRequestV1 {
            domain: REQUEST_DOMAIN.to_string(),
            kid_hex: kid(),
            node_set_id_b64: node_set(),
            requested_at: NOW,
            request_nonce_b64: b64_encode(&[2u8; 8]),
        };
        let request_sig_b64 = b64_encode(&signer.sign(req.canonical().as_bytes()));
        let grant = AccessGrantV1 { delegation: del, delegation_sig_hex, request: req, request_sig_b64 };
        let ctx = AccessVerifyContext {
            expected_node_set_id_b64: node_set(),
            expected_chain_id: 8453,
            expected_kid_hex: kid(),
            now: NOW,
        };
        (grant, ctx)
    }

    #[test]
    fn happy_path_verifies_and_recovers_owner() {
        let (grant, ctx) = mk_grant();
        let mut guard = ReplayGuard::new();
        let v = verify_access_grant(&grant, &ctx, Some(&mut guard), None).expect("ok");
        assert!(eq_addr(&v.recovered_owner, &grant.delegation.owner_address));
        assert_eq!(v.covered_addresses, grant.delegation.covered_addresses);
        assert_eq!(v.kid_hex, kid());
    }

    #[test]
    fn canonical_is_byte_identical_round_trip() {
        let (grant, _) = mk_grant();
        let c1 = grant.delegation.canonical();
        let parsed: AccessDelegationV1 = serde_json::from_str(&serde_json::to_string(&grant.delegation).unwrap()).unwrap();
        assert_eq!(c1, parsed.canonical(), "canonical must be stable across (de)serialize");
        // keys are emitted in sorted order, no whitespace
        assert!(c1.starts_with("{\"chain_id\":"));
        assert!(!c1.contains(": "));
    }

    #[test]
    fn tampered_covered_addresses_fails_closed() {
        let (mut grant, ctx) = mk_grant();
        // mutate AFTER signing → recovered owner no longer matches → del_sig_invalid
        grant.delegation.covered_addresses.push("0x000000000000000000000000000000000000dead".to_string());
        let err = verify_access_grant(&grant, &ctx, None, None).unwrap_err();
        assert_eq!(err, AccessVerifyError::DelSigInvalid);
    }

    #[test]
    fn stripped_request_sig_fails_closed() {
        let (mut grant, ctx) = mk_grant();
        grant.request_sig_b64 = String::new();
        let err = verify_access_grant(&grant, &ctx, None, None).unwrap_err();
        assert_eq!(err, AccessVerifyError::ReqSigInvalid);
    }

    #[test]
    fn request_replay_is_rejected() {
        let (grant, ctx) = mk_grant();
        let mut guard = ReplayGuard::new();
        verify_access_grant(&grant, &ctx, Some(&mut guard), None).expect("first ok");
        let err = verify_access_grant(&grant, &ctx, Some(&mut guard), None).unwrap_err();
        assert_eq!(err, AccessVerifyError::Replayed);
    }

    #[test]
    fn revoked_delegation_is_rejected() {
        let (grant, ctx) = mk_grant();
        let mut guard = ReplayGuard::new();
        guard.revoke(&grant.delegation.nonce_b64, grant.delegation.expires_at);
        let err = verify_access_grant(&grant, &ctx, Some(&mut guard), None).unwrap_err();
        assert_eq!(err, AccessVerifyError::Revoked);
    }

    #[test]
    fn expired_window_fails_closed() {
        let (grant, mut ctx) = mk_grant();
        ctx.now = grant.delegation.expires_at + 1;
        let err = verify_access_grant(&grant, &ctx, None, None).unwrap_err();
        assert_eq!(err, AccessVerifyError::DelExpired);
    }

    #[test]
    fn not_yet_valid_fails_closed() {
        let (grant, mut ctx) = mk_grant();
        ctx.now = grant.delegation.issued_at - (DELEGATION_CLOCK_SKEW_SECONDS + 1);
        let err = verify_access_grant(&grant, &ctx, None, None).unwrap_err();
        assert_eq!(err, AccessVerifyError::DelNotYetValid);
    }

    #[test]
    fn over_wide_window_fails_closed() {
        // Re-sign a delegation whose window exceeds the 24h cap.
        let (sk, owner) = wallet(11);
        let (signer, session_vk) = mldsa_seal_keypair([5u8; 32]);
        let del = AccessDelegationV1 {
            domain: DELEGATION_DOMAIN.to_string(),
            chain_id: 8453,
            kid_hex: kid(),
            node_set_id_b64: node_set(),
            owner_address: owner.clone(),
            covered_addresses: vec![owner],
            session_pub_b64: b64_encode(&session_vk),
            issued_at: NOW,
            expires_at: NOW + MAX_DELEGATION_WINDOW_SECONDS + 10,
            nonce_b64: b64_encode(&[1u8; 16]),
        };
        let delegation_sig_hex = sign_eip191(&sk, &del.canonical());
        let req = AccessRequestV1 {
            domain: REQUEST_DOMAIN.to_string(),
            kid_hex: kid(),
            node_set_id_b64: node_set(),
            requested_at: NOW,
            request_nonce_b64: b64_encode(&[2u8; 8]),
        };
        let request_sig_b64 = b64_encode(&signer.sign(req.canonical().as_bytes()));
        let grant = AccessGrantV1 { delegation: del, delegation_sig_hex, request: req, request_sig_b64 };
        let ctx = AccessVerifyContext {
            expected_node_set_id_b64: node_set(),
            expected_chain_id: 8453,
            expected_kid_hex: kid(),
            now: NOW,
        };
        let err = verify_access_grant(&grant, &ctx, None, None).unwrap_err();
        assert_eq!(err, AccessVerifyError::DelWindowTooWide);
    }

    #[test]
    fn wrong_kid_fails_closed() {
        let (grant, mut ctx) = mk_grant();
        ctx.expected_kid_hex = format!("0x{}", "cd".repeat(16));
        let err = verify_access_grant(&grant, &ctx, None, None).unwrap_err();
        assert_eq!(err, AccessVerifyError::BadKid);
    }

    #[test]
    fn wrong_session_key_request_sig_fails_closed() {
        let (mut grant, ctx) = mk_grant();
        // sign the request with a DIFFERENT session key than session_pub_b64 advertises
        let (other_signer, _other_vk) = mldsa_seal_keypair([9u8; 32]);
        grant.request_sig_b64 = b64_encode(&other_signer.sign(grant.request.canonical().as_bytes()));
        let err = verify_access_grant(&grant, &ctx, None, None).unwrap_err();
        assert_eq!(err, AccessVerifyError::ReqSigInvalid);
    }

    #[test]
    fn foreign_quorum_fails_closed() {
        let (grant, mut ctx) = mk_grant();
        ctx.expected_node_set_id_b64 = b64_encode(b"some-other-quorum");
        let err = verify_access_grant(&grant, &ctx, None, None).unwrap_err();
        assert_eq!(err, AccessVerifyError::BadNodeSet);
    }

    struct MockEip1271 {
        owner: String,
        ret: Vec<u8>,
    }
    impl Eip1271Caller for MockEip1271 {
        fn is_valid_signature(&self, owner: &str, _hash: &[u8; 32], _sig_hex: &str) -> Option<Vec<u8>> {
            if eq_addr(owner, &self.owner) {
                Some(self.ret.clone())
            } else {
                None
            }
        }
    }

    /// Smart-account owner: EIP-191 recovery yields the SIGNER, not the contract
    /// owner; the EIP-1271 hook returning the magic value authorizes it.
    #[test]
    fn eip1271_smart_account_path() {
        let (sk, _signer_addr) = wallet(11);
        let (signer, session_vk) = mldsa_seal_keypair([5u8; 32]);
        let contract_owner = "0x00000000000000000000000000000000c0ffee01".to_string();
        let del = AccessDelegationV1 {
            domain: DELEGATION_DOMAIN.to_string(),
            chain_id: 8453,
            kid_hex: kid(),
            node_set_id_b64: node_set(),
            owner_address: contract_owner.clone(),
            covered_addresses: vec![contract_owner.clone()],
            session_pub_b64: b64_encode(&session_vk),
            issued_at: NOW,
            expires_at: NOW + 3600,
            nonce_b64: b64_encode(&[1u8; 16]),
        };
        let delegation_sig_hex = sign_eip191(&sk, &del.canonical()); // signed by EOA, not the contract
        let req = AccessRequestV1 {
            domain: REQUEST_DOMAIN.to_string(),
            kid_hex: kid(),
            node_set_id_b64: node_set(),
            requested_at: NOW,
            request_nonce_b64: b64_encode(&[2u8; 8]),
        };
        let request_sig_b64 = b64_encode(&signer.sign(req.canonical().as_bytes()));
        let grant = AccessGrantV1 { delegation: del, delegation_sig_hex, request: req, request_sig_b64 };
        let ctx = AccessVerifyContext {
            expected_node_set_id_b64: node_set(),
            expected_chain_id: 8453,
            expected_kid_hex: kid(),
            now: NOW,
        };

        let mut magic = EIP1271_MAGIC_VALUE.to_vec();
        magic.extend_from_slice(&[0u8; 28]);
        let good = MockEip1271 { owner: contract_owner.clone(), ret: magic };
        let v = verify_access_grant(&grant, &ctx, None, Some(&good)).expect("eip1271 ok");
        assert!(eq_addr(&v.recovered_owner, &contract_owner));

        // non-magic return → fail closed
        let bad = MockEip1271 { owner: contract_owner, ret: vec![0u8; 32] };
        let err = verify_access_grant(&grant, &ctx, None, Some(&bad)).unwrap_err();
        assert_eq!(err, AccessVerifyError::DelSigInvalid);
    }

    #[test]
    fn eip191_recover_round_trips_to_signer_address() {
        let (sk, addr) = wallet(11);
        let msg = "hello-elastos";
        let sig = sign_eip191(&sk, msg);
        let recovered = recover_eip191(msg, &sig).expect("recover");
        assert!(eq_addr(&recovered, &addr));
        // a 1-byte tamper of the message must NOT recover the same address
        let other = recover_eip191("hello-elastoX", &sig).expect("recover");
        assert!(!eq_addr(&other, &addr));
    }

    #[test]
    fn malformed_delegation_sig_fails_closed() {
        let (mut grant, ctx) = mk_grant();
        grant.delegation_sig_hex = "0xdeadbeef".to_string();
        let err = verify_access_grant(&grant, &ctx, None, None).unwrap_err();
        assert_eq!(err, AccessVerifyError::DelSigInvalid);
    }

    #[test]
    fn has_access_calldata_matches_real_base_abi() {
        let kid16 = kid_to_bytes16(&kid()).unwrap();
        let holder = "0x000000000000000000000000000000000000beef";
        let data = has_access_calldata("0x54d42821", holder, &kid16).unwrap();
        assert_eq!(data.len(), 4 + 32 + 32);
        assert_eq!(&data[..4], &[0x54, 0xd4, 0x28, 0x21]);
        // holder right-aligned in word 0
        assert_eq!(&data[4 + 12..4 + 32], &hex::decode("000000000000000000000000000000000000beef").unwrap()[..]);
        // bytes16 left-aligned in word 1
        assert_eq!(&data[4 + 32..4 + 32 + 16], &kid16[..]);
        assert!(data[4 + 32 + 16..].iter().all(|b| *b == 0));
    }

    #[test]
    fn client_builder_round_trips_to_a_verifiable_grant() {
        // The runtime builds the delegation; the wallet (EOA) personal_signs its canonical bytes;
        // the runtime assembles + the session key signs the request. The result must verify.
        let (sk, owner) = wallet(11);
        let (session_signer, session_vk) = mldsa_seal_keypair([7u8; 32]);
        let del = build_delegation(
            8453,
            &kid(),
            &node_set(),
            &owner,
            vec![owner.clone()],
            &b64_encode(&session_vk),
            NOW,
            3600,
            &b64_encode(&[9u8; 16]),
        );
        let delegation_sig_hex = sign_eip191(&sk, &del.canonical());
        let req = build_request(&kid(), &node_set(), NOW, &b64_encode(&[3u8; 8]));
        let grant = assemble_grant(del, delegation_sig_hex, req, &session_signer);
        let ctx = AccessVerifyContext {
            expected_node_set_id_b64: node_set(),
            expected_chain_id: 8453,
            expected_kid_hex: kid(),
            now: NOW,
        };
        let v = verify_access_grant(&grant, &ctx, None, None).expect("client-built grant verifies");
        assert!(eq_addr(&v.recovered_owner, &owner));
    }

    #[test]
    fn decode_evm_bool_is_fail_closed() {
        let mut t = [0u8; 32];
        t[31] = 1;
        assert_eq!(decode_evm_bool(&t), Some(true));
        assert_eq!(decode_evm_bool(&[0u8; 32]), Some(false));
        let mut dirty = [0u8; 32];
        dirty[0] = 1; // non-zero high bytes → reject
        assert_eq!(decode_evm_bool(&dirty), None);
        assert_eq!(decode_evm_bool(&[0u8; 4]), None); // wrong length → reject
    }
}
