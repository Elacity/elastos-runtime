//! Audit logging for ElastOS
//!
//! Runtime generates audit events at every security-relevant operation.
//! These events CANNOT be bypassed by any capsule, including the shell.
//!
//! # Tamper-evidence (GAP-8)
//!
//! Each record is written as a [`ChainedRecord`]: a monotonically-increasing `seq`, the
//! `prev_hash` of the previous record, the event, a `record_hash` over
//! `domain ‖ seq ‖ prev_hash ‖ event_json`, and an ed25519 `sig` over that hash. Any edit,
//! reorder, drop, or truncation of the on-disk log breaks the [`AuditLog::verify_chain`] walk.
//! The `alg` field is a crypto-agility tag (`"ed25519"`) so a post-quantum scheme (ML-DSA) can be
//! swapped in later with **zero record-format change** — verifiers dispatch on `alg`.
//!
//! THREAT MODEL — be honest about what the signature buys: it gives tamper-evidence against
//! *offline/post-hoc* editing of the log and non-repudiation of records, because an attacker who
//! rewrites the file cannot re-sign the chain without the signing key. It does **not** defend
//! against a *live-compromised runtime*, which holds the key and could re-sign a rewritten chain.
//! Defending that requires EXTERNAL ANCHORING — periodically checkpointing the chain head to an
//! external witness (e.g. the Base chain). That anchoring is deliberate roadmap, out of scope here;
//! until it lands, do not claim more than "tamper-evident against external editing + non-repudiable".
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, RwLock};

use super::time::SecureTimestamp;
use crate::capability::token::{Action, ResourceId, TokenId};

/// Crypto-agility tag for an ed25519-signed audit record. Recorded in [`ChainedRecord::alg`] so a
/// post-quantum signature (ML-DSA) can replace it later WITHOUT changing the record format — a
/// verifier simply dispatches on this string. This is the deliberate forward-compat hook.
pub const AUDIT_SIG_ALG_ED25519: &str = "ed25519";

/// Tag for an UNSIGNED record (memory-only logs, or a file log with no signer provisioned). The
/// hash-chain still provides tamper-evidence; only non-repudiation is absent.
pub const AUDIT_SIG_ALG_NONE: &str = "none";

/// Domain separator folded into every `record_hash` so an audit-chain digest can never collide
/// with a hash computed for any other ElastOS protocol (cross-protocol binding).
const AUDIT_RECORD_DOMAIN: &[u8] = b"elastos.runtime/audit-chain/v1";

/// Genesis `prev_hash` (hex of 32 zero bytes): the link the very first record points back to.
fn genesis_prev_hash() -> [u8; 32] {
    [0u8; 32]
}

/// Compute the canonical record hash: `SHA-256(domain ‖ seq_be ‖ prev_hash ‖ event_json)`.
/// `seq` is fixed 8 bytes and `prev_hash` fixed 32, so the concatenation is unambiguous.
fn compute_record_hash(seq: u64, prev_hash: &[u8; 32], event_json: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(AUDIT_RECORD_DOMAIN);
    h.update(seq.to_be_bytes());
    h.update(prev_hash);
    h.update(event_json);
    h.finalize().into()
}

/// THE one canonicalization recipe every verifier uses: re-serialize the deserialized event with
/// this crate's serde and recompute the record hash over it. Shared by [`AuditLog::verify_chain`]
/// and [`verify_mandate_receipt`] so the meaning of "verified" can never quietly fork between the
/// on-disk walker and the portable-receipt walker (their LINKAGE policies differ by design — a
/// chain walk is contiguous, a Capability-scoped receipt is a bound selection — but the bytes
/// each record is hashed over must be identical). `Err` carries the raw serde error.
fn recompute_record_hash(rec: &ChainedRecord, prev_hash: &[u8; 32]) -> Result<[u8; 32], String> {
    let event_json = serde_json::to_string(&rec.event).map_err(|e| e.to_string())?;
    Ok(compute_record_hash(
        rec.seq,
        prev_hash,
        event_json.as_bytes(),
    ))
}

/// Domain separator for the mandate-receipt SET binding — distinct from [`AUDIT_RECORD_DOMAIN`] so a
/// set-binding signature can never be confused for (or replayed as) a per-record signature.
const MANDATE_RECEIPT_BINDING_DOMAIN: &[u8] = b"elastos.runtime/mandate-receipt-set/v1";

/// The canonical bytes an issuing runtime signs to BIND the exact ordered record set of a receipt:
/// the scope, the record count, and every `record_hash` in order. Recomputable by any verifier from
/// the receipt alone, so a signature over it makes the SET tamper-evident — adding, dropping, or
/// reordering any record changes these bytes. This closes the keyless "a holder trims a use in
/// transit" gap that a per-record filter cannot: each `record_hash` already commits to its record's
/// `(seq, prev_hash, event)`, so binding the ordered hash list fixes the whole membership. It does
/// NOT bind against the key-holding issuer itself (which can sign any set) — the same tamper-evident,
/// not tamper-proof, bound the chain's signing key carries everywhere.
fn mandate_receipt_binding_message(
    scope: &MandateReceiptScope,
    records: &[ChainedRecord],
) -> Vec<u8> {
    let mut msg =
        Vec::with_capacity(MANDATE_RECEIPT_BINDING_DOMAIN.len() + 32 + records.len() * 64);
    msg.extend_from_slice(MANDATE_RECEIPT_BINDING_DOMAIN);
    match scope {
        MandateReceiptScope::Contiguous => msg.push(0u8),
        MandateReceiptScope::Capability { token_id } => {
            msg.push(1u8);
            msg.extend_from_slice(&(token_id.len() as u64).to_be_bytes());
            msg.extend_from_slice(token_id.as_bytes());
        }
    }
    msg.extend_from_slice(&(records.len() as u64).to_be_bytes());
    for record in records {
        // `record_hash` is a fixed-width hex string (64 chars); position + count fix the order.
        msg.extend_from_slice(record.record_hash.as_bytes());
    }
    msg
}

/// One tamper-evident, hash-chained, signed audit record as persisted to disk (one JSON object per
/// line). The on-disk format; the in-memory ring buffer keeps bare [`AuditEvent`]s for fast reads.
///
/// FROZEN serialized shape — same rule as [`AuditEvent`]: verification re-serializes and
/// re-hashes these records, so field order/names must never change for existing chains to verify.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainedRecord {
    /// Monotonic sequence number (first record = 1). A gap or repeat is tamper-evidence.
    pub seq: u64,
    /// Hex of the previous record's `record_hash` (genesis = 64 zeros). Breaks on reorder/drop.
    pub prev_hash: String,
    /// The audited event.
    pub event: AuditEvent,
    /// Hex of `SHA-256(domain ‖ seq ‖ prev_hash ‖ event_json)`.
    pub record_hash: String,
    /// Crypto-agility tag: which signature scheme `sig` uses (`"ed25519"` or `"none"`).
    pub alg: String,
    /// Base64 signature over the raw `record_hash` bytes (`""` when `alg == "none"`).
    pub sig: String,
}

/// Schema tag for [`MandateReceipt`]; the verifier fail-closes on any other value.
pub const MANDATE_RECEIPT_SCHEMA: &str = "elastos.mandate_receipt/v1";

/// What a [`MandateReceipt`] covers — which determines how it is verified.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MandateReceiptScope {
    /// A CONTIGUOUS run of the chain (whole chain or a `seq` range). Verified with the contiguity
    /// check: each record links the prior and `seq` increments by 1, so nothing INTERIOR was dropped.
    Contiguous,
    /// EVERY record this runtime holds for ONE capability `token_id` — a FILTERED, non-contiguous
    /// view (a delegation's records are interleaved with others in the chain, so contiguity does NOT
    /// apply). Verified instead by requiring that every record carries this `token_id`, exactly one
    /// is the `CapabilityGrant` (the mandate itself), and the records are in strictly ascending
    /// `seq` (no duplicate or reorder). This is the per-delegation receipt shape.
    ///
    /// COMPLETENESS is bounded, and the bound differs by WHO the adversary is:
    /// - Against any HOLDER in transit (relay, custodian, the audited party): the issuer's
    ///   [`MandateReceipt::set_binding`] signature fixes the exact record set, so dropping,
    ///   adding, or reordering a use/revoke is DETECTED (`set_binding_ok` fails).
    /// - Against the key-holding ISSUER itself: NOT provable — a compromised runtime can sign a
    ///   selective set. This is the same tamper-evident-not-tamper-proof bound the chain's signing
    ///   key carries everywhere; unlike a `Contiguous` receipt (whose linkage would break on an
    ///   interior drop), a `Capability` receipt does not prove the issuer omitted nothing at export.
    Capability { token_id: String },
}

/// A PORTABLE, self-contained proof that a set of audit records were authorized and recorded under
/// one signer — verifiable by a THIRD PARTY (an auditor, insurer, counterparty) with NO runtime, NO
/// `AuditLog`, and NO disk access: just this JSON document and [`verify_mandate_receipt`]. This is
/// the "admissible receipt" product primitive for Flint — it turns the tamper-evident chain from an
/// internal integrity feature into an artifact you can hand someone off-box. By convention
/// `records[0]` is the authorization (the mandate) and the rest are the actions taken under it, in
/// ascending `seq`. The verifier proves each record is individually signed + untampered, that the
/// set satisfies its [`scope`](MandateReceipt::scope) rule (contiguity for `Contiguous`; token
/// binding + single grant + strict order for `Capability`), and — via
/// [`set_binding`](MandateReceipt::set_binding) — that no HOLDER altered the record set in transit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MandateReceipt {
    /// Schema tag; always `elastos.mandate_receipt/v1`.
    pub schema: String,
    /// Hex of the ed25519 verifying key the records are signed under (the issuing runtime's DID key).
    ///
    /// SELF-ASSERTED — READ THIS: a `verified == true` verdict authenticates the records against
    /// THIS key only. It does NOT prove the key is who you think it is: anyone can mint a keypair,
    /// fabricate a "mandate + actions" chain, sign it, and produce a receipt that verifies against
    /// its own key. A consumer MUST pin the expected signer out-of-band (pass `expected_signer_hex`
    /// to [`verify_mandate_receipt`], or compare this field to a DID key it trusts). Two further
    /// caveats travel with this artifact (same as the chain it comes from): (a) records dropped off
    /// the END of the exported range are undetectable without an external anchor — the receipt
    /// proves what it contains is a contiguous run, not that it is COMPLETE; (b) a live-compromised
    /// runtime holds the signing key and can sign a fabricated receipt. Tamper-EVIDENT, not
    /// tamper-proof.
    pub signer_public_key_hex: String,
    /// What this receipt covers — selects the verification model (contiguity vs per-capability).
    #[serde(default = "default_receipt_scope")]
    pub scope: MandateReceiptScope,
    /// The signed, hash-chained records, ascending `seq`. `records[0]` = mandate, rest = actions.
    pub records: Vec<ChainedRecord>,
    /// Ed25519 signature (base64) by the issuing runtime over
    /// [`mandate_receipt_binding_message`] — it BINDS the exact ordered record set (scope + count +
    /// each `record_hash`), so no HOLDER in transit can add, drop, or reorder a record undetectably.
    /// REQUIRED for `Capability` scope (whose membership is otherwise a keyless filter a holder could
    /// trim); optional for legacy `Contiguous` receipts, where linkage already fixes the interior.
    /// `None` only for a memory-only/unsigned log. This stops HOLDERS, not the key-holding issuer:
    /// a compromised runtime can still sign a selective set (tamper-evident, not tamper-proof).
    #[serde(default)]
    pub set_binding: Option<String>,
}

fn default_receipt_scope() -> MandateReceiptScope {
    MandateReceiptScope::Contiguous
}

/// The result of independently verifying a [`MandateReceipt`]. Every boolean is a distinct failure
/// mode so a consumer can see WHY: a tampered event breaks `hashes_ok`, a forged/wrong-key signature
/// breaks `signatures_ok`, a dropped/reordered record breaks `chain_linkage_ok`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MandateReceiptVerdict {
    /// AUTHENTIC — the single bit an auditor acts on. True ONLY when the receipt is
    /// `structurally_valid` AND its embedded signer matches the caller-PINNED expected signer.
    /// `false` whenever no expected signer was pinned: you CANNOT authenticate a self-asserted
    /// receipt without an out-of-band trust anchor (an attacker's self-signed receipt is
    /// structurally valid too — see [`MandateReceipt`]).
    pub authenticated: bool,
    /// Structurally valid under the receipt's OWN embedded key: every hash recomputes, every
    /// signature verifies, records are contiguous. This is NOT authenticity — use it only for
    /// display ("signed by <signer>"), never as the trust decision. Prefer [`authenticated`].
    pub structurally_valid: bool,
    /// Whether the receipt's embedded signer equals the caller-pinned expected signer. `None` when
    /// the caller pinned no expected signer (so authenticity could not be decided).
    pub signer_matches_expected: Option<bool>,
    /// True iff `records[0]` is the genesis-anchored start of the chain (`seq == 1`, `prev_hash` all
    /// zero) — so the receipt proves the run FROM THE BEGINNING. (Front-truncation is otherwise
    /// undetectable; END-truncation still needs an external head anchor — see [`MandateReceipt`].)
    /// N/A for `Capability` scope: a mandate's grant sits mid-chain after unrelated events, so this
    /// is expected to be `false` for a legitimate per-capability receipt — do not treat it as a
    /// completeness or suspicion signal there (use `scope_ok` + `set_binding_ok` instead).
    pub starts_at_genesis: bool,
    /// How many records were checked.
    pub records: usize,
    /// The signer the receipt claims to be signed under (echoed so a consumer can pin it).
    pub signer_public_key_hex: String,
    /// Every record's `record_hash` recomputes from its own contents (no event was edited).
    pub hashes_ok: bool,
    /// Every record is ed25519-signed and verifies against the signer key over its `record_hash`.
    pub signatures_ok: bool,
    /// The records form a contiguous run: `seq` increments by 1 and each `prev_hash` links the prior.
    /// (Reported for every receipt; only REQUIRED for a `Contiguous`-scope receipt.)
    pub chain_linkage_ok: bool,
    /// The scope's structural rule holds. For `Contiguous` this is `chain_linkage_ok` (a completeness
    /// statement: nothing interior was dropped). For `Capability` it is BINDING + shape, NOT
    /// completeness: every record carries the target `token_id`, exactly one is the grant, and the
    /// records are in strictly ascending `seq`. A `Capability` `scope_ok == true` does NOT assert no
    /// use was omitted at export by the issuer — see [`MandateReceiptScope::Capability`]; holder-side
    /// omission is caught separately by `set_binding_ok`.
    pub scope_ok: bool,
    /// The issuer's [`MandateReceipt::set_binding`] signature verifies over the exact ordered record
    /// set — so no HOLDER added, dropped, or reordered a record in transit. REQUIRED (must be `true`)
    /// for `Capability` scope; for `Contiguous` it is `true` when absent (linkage already binds the
    /// interior) and verified when present. Does not bind against the key-holding issuer itself.
    pub set_binding_ok: bool,
    /// The first structural failure (bad schema/hex, wrong key length, empty receipt), if any.
    pub error: Option<String>,
}

impl MandateReceiptVerdict {
    fn failed(signer_public_key_hex: String, error: impl Into<String>) -> Self {
        MandateReceiptVerdict {
            authenticated: false,
            structurally_valid: false,
            signer_matches_expected: None,
            starts_at_genesis: false,
            records: 0,
            signer_public_key_hex,
            hashes_ok: false,
            signatures_ok: false,
            chain_linkage_ok: false,
            scope_ok: false,
            set_binding_ok: false,
            error: Some(error.into()),
        }
    }
}

/// STANDALONE verification of a [`MandateReceipt`] — pure, no `self`, no I/O, no runtime. Re-derives
/// each `record_hash` exactly as the chain did (`compute_record_hash`), checks the ed25519 signature
/// over it against the receipt's own signer key, and checks contiguity. Mirrors
/// [`AuditLog::verify_chain`]'s recipe byte-for-byte, minus the disk.
///
/// AUTHENTICITY REQUIRES PINNING. `expected_signer_hex` is the caller's out-of-band trust anchor —
/// the DID key it already trusts for the issuing runtime. The result's `authenticated` bit is true
/// ONLY when the receipt is structurally sound AND its embedded signer equals that pinned key. Pass
/// `None` only when you deliberately want a STRUCTURAL check for display (never a trust decision):
/// an attacker can self-sign a fabricated receipt that is `structurally_valid` under its own key, so
/// `authenticated` is `false` without a pin. The record SET is also bound by the issuer's
/// `set_binding` signature (`set_binding_ok`), so a HOLDER cannot add/drop/reorder a record in
/// transit — REQUIRED for `Capability` scope. Residual caveats (carried from the chain): a
/// `Contiguous` receipt proves its records are a contiguous run but end-truncation still needs an
/// external head anchor (`starts_at_genesis` covers only the front); a `Capability` receipt's
/// completeness holds only against holders, not the key-holding issuer; and verification re-serializes
/// events with this crate's serde, so a third party must use a byte-compatible encoder.
pub fn verify_mandate_receipt(
    receipt: &MandateReceipt,
    expected_signer_hex: Option<&str>,
) -> MandateReceiptVerdict {
    let signer = receipt.signer_public_key_hex.clone();
    if receipt.schema != MANDATE_RECEIPT_SCHEMA {
        return MandateReceiptVerdict::failed(
            signer,
            format!(
                "unexpected schema {:?} (want {MANDATE_RECEIPT_SCHEMA})",
                receipt.schema
            ),
        );
    }
    if receipt.records.is_empty() {
        return MandateReceiptVerdict::failed(signer, "receipt has no records");
    }
    // Decode the self-contained signer key — the only key material the verifier needs.
    let vk_bytes = match hex::decode(receipt.signer_public_key_hex.trim()) {
        Ok(bytes) => bytes,
        Err(e) => return MandateReceiptVerdict::failed(signer, format!("bad signer key hex: {e}")),
    };
    let vk_array: [u8; 32] = match vk_bytes.as_slice().try_into() {
        Ok(array) => array,
        Err(_) => return MandateReceiptVerdict::failed(signer, "signer key is not 32 bytes"),
    };
    let vk = match VerifyingKey::from_bytes(&vk_array) {
        Ok(key) => key,
        Err(e) => return MandateReceiptVerdict::failed(signer, format!("invalid signer key: {e}")),
    };

    let mut hashes_ok = true;
    let mut signatures_ok = true;
    let mut chain_linkage_ok = true;

    for (index, record) in receipt.records.iter().enumerate() {
        // Contiguity: each record after the first must link the prior by hash AND increment seq by 1.
        if index > 0 {
            let prior = &receipt.records[index - 1];
            if record.prev_hash != prior.record_hash || record.seq != prior.seq.saturating_add(1) {
                chain_linkage_ok = false;
            }
        }
        // Internal integrity: recompute the record hash from the record's OWN contents.
        let prev_hash: [u8; 32] = match hex::decode(&record.prev_hash)
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
        {
            Some(array) => array,
            None => {
                hashes_ok = false;
                signatures_ok = false;
                continue;
            }
        };
        let computed = match recompute_record_hash(record, &prev_hash) {
            Ok(hash) => hash,
            Err(e) => {
                return MandateReceiptVerdict::failed(
                    signer,
                    format!("seq {}: re-serialize event: {e}", record.seq),
                )
            }
        };
        match hex::decode(&record.record_hash) {
            Ok(claimed) if claimed.as_slice() == computed.as_slice() => {}
            _ => hashes_ok = false,
        }
        // Signature: a mandate receipt MUST be ed25519-signed (never `alg="none"`), verifying over
        // the recomputed hash against the receipt's own signer key.
        if record.alg != AUDIT_SIG_ALG_ED25519 {
            signatures_ok = false;
            continue;
        }
        let sig_ok = BASE64
            .decode(record.sig.trim())
            .ok()
            .and_then(|bytes| Signature::from_slice(&bytes).ok())
            // STRICT verification (council S49 red-team F3): reject malleated-S and
            // small-order keys, matching the intent path's own posture — two conforming
            // verifiers must never disagree on a malleated signature.
            .map(|signature| vk.verify_strict(&computed, &signature).is_ok())
            .unwrap_or(false);
        if !sig_ok {
            signatures_ok = false;
        }
    }

    // The scope's completeness/binding rule. Contiguous ⇒ nothing interior dropped (linkage);
    // Capability ⇒ every record is bound to the target token_id and exactly one is the grant, so a
    // record from a DIFFERENT delegation can't be smuggled in and the mandate itself is present.
    let scope_ok = match &receipt.scope {
        MandateReceiptScope::Contiguous => chain_linkage_ok,
        MandateReceiptScope::Capability { token_id } => {
            let all_bound = receipt
                .records
                .iter()
                .all(|record| record.event.capability_token_id() == Some(token_id.as_str()));
            let grant_count = receipt
                .records
                .iter()
                .filter(|record| record.event.is_capability_grant())
                .count();
            // Strictly ascending, unique `seq`: a bundle-only guard against a duplicated or
            // reordered record inflating/misrepresenting the action set.
            let strictly_ordered = receipt
                .records
                .windows(2)
                .all(|pair| pair[1].seq > pair[0].seq);
            all_bound && grant_count == 1 && strictly_ordered
        }
    };
    // SET BINDING: the issuer's signature over the ordered record set (scope + count + record
    // hashes). It makes membership tamper-EVIDENT against any HOLDER — a dropped/added/reordered
    // record changes the signed message. REQUIRED for `Capability`; for `Contiguous` the linkage
    // already fixes the interior, so a binding is optional but MUST verify when present.
    let set_binding_ok = match &receipt.set_binding {
        Some(sig_b64) => BASE64
            .decode(sig_b64.trim())
            .ok()
            .and_then(|bytes| Signature::from_slice(&bytes).ok())
            .map(|signature| {
                // Strict for the same reason as the per-record sigs (S49 red-team F3).
                vk.verify_strict(
                    &mandate_receipt_binding_message(&receipt.scope, &receipt.records),
                    &signature,
                )
                .is_ok()
            })
            .unwrap_or(false),
        None => matches!(receipt.scope, MandateReceiptScope::Contiguous),
    };
    let structurally_valid = hashes_ok && signatures_ok && scope_ok && set_binding_ok;
    // Authenticity requires the caller's out-of-band pin: the embedded signer must equal a key the
    // consumer already trusts. Without a pin we CANNOT authenticate — an attacker self-signs too.
    let signer_matches_expected = expected_signer_hex.map(|expected| {
        expected
            .trim()
            .eq_ignore_ascii_case(receipt.signer_public_key_hex.trim())
    });
    let authenticated = structurally_valid && signer_matches_expected == Some(true);
    // Genesis anchor: records[0] is the true start of the chain (front-truncation guard).
    let first = &receipt.records[0];
    let starts_at_genesis = first.seq == 1
        && hex::decode(&first.prev_hash)
            .map(|bytes| bytes.len() == 32 && bytes.iter().all(|byte| *byte == 0))
            .unwrap_or(false);

    MandateReceiptVerdict {
        authenticated,
        structurally_valid,
        signer_matches_expected,
        starts_at_genesis,
        records: receipt.records.len(),
        signer_public_key_hex: signer,
        hashes_ok,
        signatures_ok,
        chain_linkage_ok,
        scope_ok,
        set_binding_ok,
        error: None,
    }
}

/// Errors from [`AuditLog::emit`]. A custody-relevant caller MUST fail its operation closed on any
/// of these (the record could not be durably committed to the tamper-evident log).
#[derive(Debug)]
pub enum AuditError {
    /// The event could not be serialized.
    Serialize(String),
    /// The record could not be written/flushed/synced to durable storage.
    Io(String),
    /// The chain-state or writer lock was poisoned.
    Lock,
}

impl std::fmt::Display for AuditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditError::Serialize(e) => write!(f, "audit event serialization failed: {e}"),
            AuditError::Io(e) => write!(f, "audit record durable-write failed: {e}"),
            AuditError::Lock => write!(f, "audit log lock poisoned"),
        }
    }
}

impl std::error::Error for AuditError {}

/// Mutable hash-chain head, guarded by a `Mutex` so `emit(&self, ..)` stays `&self`.
struct ChainState {
    /// `seq` of the last APPENDED record (0 = none yet; next record is `last_seq + 1`). On a
    /// file-backed log this advances once the record's bytes are flushed to the OS (append order
    /// is the chain order); DURABILITY is tracked separately by [`FlushState::durable_seq`] — an
    /// `emit` does not return `Ok` until its seq is durable (group commit, S51).
    last_seq: u64,
    /// `record_hash` of the last appended record (genesis = zeros). The next record's `prev_hash`.
    prev_hash: [u8; 32],
}

/// Group-commit durability state for a file-backed log (Sprint 51 — Track C2). MEASURED motivation
/// (`benches/audit_emit.rs`, the S51 decision run): one fsync per record costs ~950 µs — a ~1.1k
/// emits/s global ceiling, ~430× the CPU cost of the emit itself — and every concurrent emitter
/// serialized behind it (magnitudes are box-dependent; re-run the bench on the target). The
/// group commit lets N concurrent emits share ONE fsync: each emitter appends its record (ordered,
/// under the chain lock), then waits until a flusher's fsync covers its seq. THE CONTRACT IS
/// UNCHANGED: `emit` returns `Ok` only after ITS record is durable on disk — batching moves the
/// fsync, never the promise. Single-threaded emits still pay one fsync each (nothing to coalesce);
/// the ceiling lifts with CONCURRENCY, which is exactly the regime the server's per-request
/// custody emits are in.
struct FlushState {
    /// Highest seq durably on disk (covered by a completed fsync).
    durable_seq: u64,
    /// A flusher's fsync is currently in flight (exactly one at a time).
    flushing: bool,
    /// Set on the FIRST durable-write or fsync failure and never cleared: the log refuses every
    /// subsequent emit (fail-closed). After a failed write/fsync the on-disk suffix is UNKNOWN
    /// (the bytes may or may not land), so "retry the same seq" — the pre-S51 behavior — could
    /// append a DUPLICATE seq after a half-landed one and corrupt the chain for verifiers. The
    /// write-failure arm poisons while STILL HOLDING the chain lock and `emit` re-checks under it
    /// (council S51 guardian F1 / red-team F2), so an emit queued on the chain lock during the
    /// failure can never re-derive the failed seq and append behind the fragment. A poisoned log
    /// is an operator incident: restart re-opens from the durable prefix (a torn
    /// never-acknowledged tail is quarantined at open — `quarantine_torn_tail`; the verified open
    /// then re-verifies the whole remaining chain).
    poisoned: Option<String>,
}

/// Audit event types.
///
/// # The serialized shape of EVERY variant is FROZEN
///
/// Chain verification ([`AuditLog::verify_chain`], [`verify_mandate_receipt`]) RE-SERIALIZES each
/// deserialized event and recomputes its hash, so the serialized bytes of every variant — field
/// ORDER, field NAMES, omission rules — are load-bearing for every signed chain and every portable
/// receipt already in the world. Reordering, renaming, or inserting a field in ANY variant silently
/// breaks verification of all pre-existing records.
///
/// To extend a variant: append the new field LAST with
/// `#[serde(default, skip_serializing_if = "Option::is_none")]` so a pre-existing record (no field
/// ⇒ `None`) re-serializes byte-identically, and add a byte-identity ratchet test beside
/// `rail_ref_is_present_when_set_and_omitted_for_back_compat`. The `responsible_entity`,
/// `rail_ref`, and `grant_digest` fields below are the worked examples.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuditEvent {
    /// Runtime started
    RuntimeStart {
        timestamp: SecureTimestamp,
        version: String,
    },

    /// Runtime stopped
    RuntimeStop { timestamp: SecureTimestamp },

    /// Capsule launched
    CapsuleLaunch {
        timestamp: SecureTimestamp,
        capsule_id: String,
        capsule_name: String,
        cid: Option<String>,
        trust_level: TrustLevel,
    },

    /// Capsule stopped
    CapsuleStop {
        timestamp: SecureTimestamp,
        capsule_id: String,
        reason: StopReason,
    },

    /// Capability granted
    CapabilityGrant {
        timestamp: SecureTimestamp,
        token_id: String,
        capsule_id: String,
        resource: String,
        action: String,
        expiry: Option<SecureTimestamp>,
        /// The RESPONSIBLE ENTITY (Sprint 32): the operator/legal entity DID accountable for every
        /// act under this grant — the EU-AI-Act liability binding, hash-linked and signed into the
        /// chain (and therefore into the portable MandateReceipt). `skip_serializing_if` is
        /// LOAD-BEARING: chain verification RE-SERIALIZES deserialized events, so a pre-S32 record
        /// (no field ⇒ None) must re-serialize byte-identically or every old chain breaks.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        responsible_entity: Option<String>,
    },

    /// Capability revoked
    CapabilityRevoke {
        timestamp: SecureTimestamp,
        token_id: String,
        reason: String,
    },

    /// Capability used
    CapabilityUse {
        timestamp: SecureTimestamp,
        token_id: String,
        capsule_id: String,
        resource: String,
        action: String,
        success: bool,
        /// The rail's reference for an act that settled on an external rail (Sprint 34): for a
        /// `runtime.pay` act it is the payment/on-chain reference (e.g. a DRM buy's tx hash +
        /// `operative:tokenId`), sanitized and bounded. `None` for every non-rail act. This is
        /// the on-chain truth the portable receipt carries — WHICH tx settled the mandate's
        /// payment — beyond the amount already in `input_hash`.
        ///
        /// BACK-COMPAT IS LOAD-BEARING (mirrors S32 `responsible_entity`): appended LAST with
        /// `#[serde(default, skip_serializing_if = "Option::is_none")]`, so a pre-S34
        /// `CapabilityUse` (no field ⇒ None) re-serializes byte-identically and every pre-S34
        /// signed chain still verifies.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rail_ref: Option<String>,
    },

    /// Content fetched via elastos://
    ContentFetch {
        timestamp: SecureTimestamp,
        cid: String,
        source: FetchSource,
        success: bool,
    },

    /// Protected content OPENED (or refused) at the dDRM boundary (GAP-8). This is the
    /// who-opened-what-when custody record — it belongs in the tamper-evident log, not only in
    /// tracing. The audit log is the DELIBERATE place this identifier triple lives (cf. the metadata
    /// minimization elsewhere that stops logging it to operator-visible `info!`): a custody trail
    /// must name the subject, the content, and the decision to be worth anything to an auditor.
    ContentOpen {
        timestamp: SecureTimestamp,
        session_id: String,
        principal_id: String,
        content_id: String,
        action: String,
        /// `"opened"` or `"denied"`.
        decision: String,
        /// The rights-decision provenance (e.g. `chain-provider (live RPC: …)`), for the trail.
        source: String,
        /// The forensic-watermark grant anchor for this open (`grant_watermark_digest16` hex): a
        /// NON-reversible commitment to the buyer's signed delegation that the invisible pixel-lock
        /// mark also carries, so a leaked frame is verifiable against this custody row (see
        /// `docs/THREAT_MODEL.md` §4). `None` for opens with no wallet-signed grant (e.g. media /
        /// legacy enrolled-caller). Skipped when absent so prior records hash-verify unchanged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        grant_digest: Option<String>,
    },

    /// Authentication attempt
    AuthAttempt {
        timestamp: SecureTimestamp,
        identity: String,
        success: bool,
        method: String,
    },

    /// Epoch advanced (mass revocation)
    EpochAdvance {
        timestamp: SecureTimestamp,
        old_epoch: u64,
        new_epoch: u64,
        reason: String,
    },

    /// Configuration changed
    ConfigChange {
        timestamp: SecureTimestamp,
        setting: String,
        old_value: String,
        new_value: String,
    },

    /// Security warning
    SecurityWarning {
        timestamp: SecureTimestamp,
        warning_type: String,
        details: String,
    },

    /// Session created
    SessionCreated {
        timestamp: SecureTimestamp,
        session_id: String,
        session_type: String,
        vm_id: Option<String>,
    },

    /// Session destroyed
    SessionDestroyed {
        timestamp: SecureTimestamp,
        session_id: String,
        reason: String,
    },

    /// Capability requested (pending approval)
    CapabilityRequested {
        timestamp: SecureTimestamp,
        request_id: String,
        session_id: String,
        resource: String,
        action: String,
    },

    /// Capability request denied
    CapabilityDenied {
        timestamp: SecureTimestamp,
        request_id: String,
        session_id: String,
        reason: String,
    },

    /// Capability request approved — the human CONSENT decision (G4b). Distinct
    /// from `CapabilityGrant` (token issuance): keyed on request_id/session_id and
    /// records WHO approved, the exact mirror of `CapabilityDenied`.
    CapabilityApproved {
        timestamp: SecureTimestamp,
        request_id: String,
        session_id: String,
        resource: String,
        action: String,
        approver: String,
    },

    /// A capsule's spend budget was debited for an act (the meter charged the act-over-MCP path).
    SpendDebit {
        timestamp: SecureTimestamp,
        capsule_id: String,
        operation: String,
        cost: u64,
        remaining: u64,
    },

    /// A capsule's act was REFUSED because its spend budget was exhausted (fail-closed before
    /// dispatch; the single-use token was refunded since nothing acted).
    BudgetExhausted {
        timestamp: SecureTimestamp,
        capsule_id: String,
        operation: String,
        requested: u64,
    },

    /// A guest's network egress was DROPPED by the per-TAP kernel firewall (W1b).
    ///
    /// The drop itself happens in-kernel and NEVER depends on this record being
    /// written — this is the best-effort-durable custody of a contained attempt
    /// ("guest X tried to reach <dest> and was stopped"). It is emitted from a
    /// userspace NFLOG reader fed by the chain's rate-limited `log` rule, so a
    /// down or flooded reader loses audit records, never containment.
    EgressDenied {
        timestamp: SecureTimestamp,
        /// Canonical capsule identity (`vm-{name}`) for custody correlation with
        /// the spend/grant chain — NOT the TAP device name.
        capsule_id: String,
        /// The TAP device the drop was observed on (e.g. `cv1a2b3c4d`).
        tap: String,
        /// The blocked destination (`IP` or `IP:port`).
        dest: String,
        /// The transport of the blocked packet (e.g. `tcp`, `udp`, `icmp`).
        proto: String,
        /// Further drops the kernel rate-limit suppressed and folded into this
        /// record (0 = a lone drop; >0 = "and N more were suppressed").
        suppressed: u64,
    },

    /// An agent DECLARED an intent before acting (intent-proof loop). The agent's signed
    /// proof obligation, recorded on-chain so "what it said it would do" is tamper-evident
    /// custody independent of the outcome. See `capability::intent::IntentDeclarationV1`.
    IntentDeclared {
        timestamp: SecureTimestamp,
        /// Canonical capsule identity (`vm-{name}`) for custody correlation.
        capsule_id: String,
        /// The declaration id (`IntentDeclarationV1::intent_id`).
        intent_id: String,
        method_id: String,
        resource: String,
        action: String,
        /// The standing-grant envelope the intent claimed to fall within.
        standing_grant_id: String,
    },

    /// An intent was DENIED because it fell outside its standing-grant envelope
    /// (`intent ⊄ envelope`), fail-closed. A refused act leaves a signed trace WITH its
    /// reason — absence is never a pass.
    IntentDenied {
        timestamp: SecureTimestamp,
        capsule_id: String,
        intent_id: String,
        method_id: String,
        resource: String,
        action: String,
        standing_grant_id: String,
        /// The fail-closed denial reason (`EnvelopeDenial`, e.g. `method_not_in_envelope`).
        reason: String,
    },

    /// The declared-vs-done verdict for an intent (intent-proof loop): `matched`,
    /// `diverged` (a redeemed field differs from the declaration), or `undelivered`
    /// (declared, no receipt). The gap between said and done, made tamper-evident.
    IntentReconciled {
        timestamp: SecureTimestamp,
        capsule_id: String,
        intent_id: String,
        /// The reconciled receipt's token id (empty when `undelivered`).
        receipt_id: String,
        /// `matched` | `diverged` | `undelivered`.
        status: String,
        /// Diverged-field list / undelivered reason; empty when `matched`.
        divergence_detail: String,
    },

    /// Identity registered (passkey)
    IdentityRegistered {
        timestamp: SecureTimestamp,
        user_id: String,
        method: String,
    },

    /// Storage access via provider
    StorageAccess {
        timestamp: SecureTimestamp,
        session_id: String,
        user_id: String,
        uri: String,
        action: String,
        success: bool,
    },

    /// Inter-capsule message sent
    MessageSent {
        timestamp: SecureTimestamp,
        from: String,
        to: String,
        size_bytes: usize,
    },

    /// Policy proposal (advisory recommendation from proposer)
    PolicyProposal {
        timestamp: SecureTimestamp,
        request_id: String,
        recommended_outcome: String,
        confidence: f32,
        rationale: String,
    },

    /// Policy decision made (authoritative verifier decision)
    PolicyDecisionMade {
        timestamp: SecureTimestamp,
        decision_id: String,
        request_id: String,
        outcome: String,
        checks_passed: usize,
        checks_failed: usize,
        shadow: bool,
        rationale: String,
    },

    /// Policy divergence (real and shadow verifiers disagree)
    PolicyDivergence {
        timestamp: SecureTimestamp,
        request_id: String,
        real_decision_id: String,
        shadow_decision_id: String,
        real_outcome: String,
        shadow_outcome: String,
        real_rationale: String,
        shadow_rationale: String,
    },

    /// Custom event for extensibility
    Custom {
        event_type: String,
        details: serde_json::Value,
    },
}

/// Trust level for capsules
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustLevel {
    /// Signed by root-trusted key
    Trusted,
    /// Signed by known community key
    Community,
    /// Unsigned or unknown signer
    Untrusted,
}

/// Reason for capsule stop
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Normal stop requested
    Requested,
    /// Capsule exited normally
    Completed,
    /// Capsule crashed/errored
    Error(String),
    /// Resource limit exceeded
    ResourceLimit(String),
    /// Security violation
    SecurityViolation(String),
}

/// Source of fetched content
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FetchSource {
    LocalCache,
    IpfsGateway(String),
    Peer(String),
}

/// Maximum events to keep in memory buffer
const MAX_MEMORY_EVENTS: usize = 1000;

/// A LIVE full-chain integrity attestation of a file-backed audit log — the read-surface projection
/// of [`AuditLog::chain_attestation`]. Distinct from per-event signature checks (AUD-4): this is the
/// whole-chain walk (`verify_chain`), so it also catches reorder / drop / truncation, mid-session,
/// not just at startup.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChainAttestation {
    /// The full hash + signature chain walked clean end to end.
    pub verified: bool,
    /// Number of records verified (the chain length) on a clean walk; 0 on failure.
    pub records: u64,
    /// The signing key (hex) the chain verifies under, when signed.
    pub signer: Option<String>,
    /// The first break naming why verification failed (`verified == false`); `None` when clean.
    pub error: Option<String>,
}

/// Audit log manager
pub struct AuditLog {
    writer: Option<Mutex<BufWriter<File>>>,
    log_path: Option<PathBuf>,
    /// Also write to stdout (for development)
    echo_stdout: bool,
    /// In-memory buffer of recent events (ring buffer)
    memory_buffer: RwLock<VecDeque<AuditEvent>>,
    /// Hash-chain head (seq + prev_hash) — see [`ChainState`] for the append-vs-durable split.
    chain: Mutex<ChainState>,
    /// ed25519 signer for the chain. `None` ⇒ records carry `alg = "none"` (chain only, no
    /// non-repudiation). A persisted, dedicated key (NOT a fresh in-memory one) when file-backed.
    signer: Option<SigningKey>,
    /// Group-commit durability tracker (S51), file-backed logs only. LOCK ORDER: `chain` → `writer`
    /// during append; `flush.0` alone, then `writer` alone, during a flush — the flusher NEVER
    /// holds `chain`, so appends and fsyncs overlap only at the writer mutex.
    flush: Option<(Mutex<FlushState>, Condvar)>,
    /// Highest seq whose bytes are flushed to the OS (BufWriter flush complete). Written under the
    /// chain lock; read by the flusher (without `chain`) to know what seq its fsync will cover.
    written_seq: AtomicU64,
    /// Highest seq the tail-truncation head anchor has recorded. Its own mutex (never nested with
    /// the others) so the anchor write runs OFF the flush critical path and can never hold up the
    /// waiters a flush just released. Guarded-monotonic: a flusher anchors only a cover above the
    /// recorded high, so late/overlapping flushers can never regress the anchor.
    anchored_seq: Mutex<u64>,
    /// A CLONED fd of the log file, used ONLY for `sync_all` (never written). MEASURED necessity
    /// (the second S51 cut): fsyncing through the writer mutex convoyed every concurrent appender
    /// behind the ~1 ms fsync (an appender holds `chain` while waiting on `writer`), collapsing
    /// group-commit batches to ~1 record — SLOWER than the serial baseline. `fsync` commits the
    /// INODE, not the fd, so syncing this clone durably commits every byte the appenders already
    /// flushed to the OS, while they keep appending through the writer. `None` only if the clone
    /// failed at open (then the flusher falls back to fsync-under-the-writer-mutex: correct,
    /// convoy-slow).
    sync_handle: Option<File>,
}

impl AuditLog {
    /// Create a new audit log without file output (memory only, for testing). No signer: the
    /// hash-chain still binds in-memory records, but there is nothing durable to non-repudiate.
    pub fn new() -> Self {
        Self {
            writer: None,
            log_path: None,
            echo_stdout: false,
            memory_buffer: RwLock::new(VecDeque::with_capacity(MAX_MEMORY_EVENTS)),
            chain: Mutex::new(ChainState {
                last_seq: 0,
                prev_hash: genesis_prev_hash(),
            }),
            signer: None,
            flush: None,
            written_seq: AtomicU64::new(0),
            anchored_seq: Mutex::new(0),
            sync_handle: None,
        }
    }

    /// Fresh group-commit state for a file-backed log resuming at `resumed_seq` (everything already
    /// on disk is durable by definition — it was read back).
    fn fresh_flush_state(resumed_seq: u64) -> Option<(Mutex<FlushState>, Condvar)> {
        Some((
            Mutex::new(FlushState {
                durable_seq: resumed_seq,
                flushing: false,
                poisoned: None,
            }),
            Condvar::new(),
        ))
    }

    /// Create an audit log that writes to the given path, hash-chained and ed25519-signed.
    ///
    /// The signing key is loaded from (or, first time, generated into) a sibling `<log>.signing-key`
    /// file with `0600` permissions, and the verifying key is published to `<log>.pubkey`. This is a
    /// DEDICATED, PERSISTED key — not a fresh in-memory one — so the chain stays verifiable across
    /// restarts and a verifier has a stable key to check. (Host-file protection, not an HSM: see the
    /// live-compromise caveat in the module docs.) If the log already has records, the chain RESUMES
    /// from the last one (append-only continuity).
    pub fn with_file(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref().to_path_buf();

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Quarantine a torn (provably never-acknowledged) trailing fragment BEFORE resuming, so a
        // crash mid-writeback re-opens from the intact durable prefix instead of refusing (verified
        // mode) or appending onto the fragment (plain mode) — see `quarantine_torn_tail`.
        quarantine_torn_tail(&path)?;

        // Resume the chain head from any existing records (append-only continuity across restarts).
        let chain = resume_chain_state(&path);

        // Open APPEND-ONLY: never truncate or seek; the chain is the integrity, the OS append is the
        // ordering. (`append(true)` forces every write to EOF even under concurrent writers.)
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        // The fsync-only clone (see `sync_handle`) — taken before the fd moves into the BufWriter.
        let sync_handle = match file.try_clone() {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::warn!(
                    "audit log fd clone failed ({e}) — group-commit falls back to fsync under \
                     the writer lock (correct, slower under concurrency)"
                );
                None
            }
        };
        let writer = BufWriter::new(file);

        // Load-or-create the dedicated, persisted signing key alongside the log.
        let signer = load_or_create_signer(&path)?;

        let resumed_seq = chain.last_seq;
        // Seed the anchor high-water from the ON-DISK anchor too (council S51 guardian F5): a log
        // reopened (unverified) after truncation resumes at a LOWER seq, and seeding from
        // resumed_seq alone would let the first flush overwrite the anchor DOWNWARD — destroying
        // the very truncation evidence a later verified open would have caught. Best-effort read
        // (an unreadable anchor seeds from the resumed head; the verified open still fail-closes).
        let disk_anchor = read_head_anchor(&path).ok().flatten().unwrap_or(0);
        Ok(Self {
            writer: Some(Mutex::new(writer)),
            log_path: Some(path),
            echo_stdout: false,
            memory_buffer: RwLock::new(VecDeque::with_capacity(MAX_MEMORY_EVENTS)),
            chain: Mutex::new(chain),
            signer: Some(signer),
            flush: Self::fresh_flush_state(resumed_seq),
            written_seq: AtomicU64::new(resumed_seq),
            anchored_seq: Mutex::new(resumed_seq.max(disk_anchor)),
            sync_handle,
        })
    }

    /// Open a file-backed log AND verify the existing on-disk chain before returning, refusing to
    /// resume on a broken/tampered log. This is the fail-closed **verify-on-read** entry point: an
    /// operator who enables the durable audit trail (e.g. the EU AI Act custody record) must never
    /// append new records onto a chain whose existing records fail the hash + signature walk — a
    /// silent append would launder a tampered history under a fresh, valid-looking tail. A clean or
    /// empty log resumes normally (`Ok`).
    ///
    /// Verifies under the log's OWN persisted signing key (the same key future appends use), so a
    /// missing/swapped key file is caught as a signature failure.
    ///
    /// NOTE (truncation): `verify_chain` detects edit / reorder / mid-drop of records that ARE on
    /// disk, but a *tail* truncation removes records the walk never sees — detecting that needs an
    /// externally persisted head-anchor (still open; tracked in KNOWN_GAPS G8 verify-on-read).
    pub fn with_file_verified(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let log = Self::with_file(path)?;
        let vk = log.signer.as_ref().map(|s| s.verifying_key());
        let verified = log.verify_chain(vk.as_ref()).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("audit log failed integrity verification on open: {e}"),
            )
        })?;
        // Tail-truncation guard: the committed head-anchor is a LOWER bound on how many records must
        // be on disk. Fewer verified than the anchor promised ⇒ records were sliced off the END (a
        // cut the chain walk alone can't see — it would just observe a shorter, validly-linked log).
        if let Some(p) = &log.log_path {
            if let Some(anchor_seq) = read_head_anchor(p)? {
                if verified < anchor_seq {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "audit log tail-truncated: head-anchor committed seq {anchor_seq} but \
                             only {verified} records verify on disk"
                        ),
                    ));
                }
            }
        }
        Ok(log)
    }

    /// Enable/disable echoing to stdout
    pub fn set_echo_stdout(&mut self, echo: bool) {
        self.echo_stdout = echo;
    }

    /// TEST SEAM (never a production constructor): wrap an already-open file
    /// handle as a file-backed log, so a test can force a durable-write failure
    /// (e.g. a read-only fd) and prove a fail-closed caller (`emit` + `?`)
    /// aborts instead of silently completing (G8a). Chain starts at genesis and
    /// is unsigned (`signer: None`) — this seam exercises the write/IO failure
    /// path, not signature semantics. `pub` so downstream crates' ratchets
    /// (e.g. the spend-budget attest-failure rollback, council S28 F2) can
    /// inject the same failure; production wiring uses `with_file*` only.
    pub fn with_file_handle(file: File) -> Self {
        let sync_handle = file.try_clone().ok();
        Self {
            writer: Some(Mutex::new(BufWriter::new(file))),
            log_path: None,
            echo_stdout: false,
            memory_buffer: RwLock::new(VecDeque::with_capacity(MAX_MEMORY_EVENTS)),
            chain: Mutex::new(ChainState {
                last_seq: 0,
                prev_hash: genesis_prev_hash(),
            }),
            signer: None,
            flush: Self::fresh_flush_state(0),
            written_seq: AtomicU64::new(0),
            anchored_seq: Mutex::new(0),
            sync_handle,
        }
    }

    /// The audit log's verifying key (hex), when file-backed + signed. A verifier checks the
    /// chain's signatures against this. `None` for a memory-only (unsigned) log.
    pub fn verifying_key_hex(&self) -> Option<String> {
        self.signer
            .as_ref()
            .map(|s| hex::encode(s.verifying_key().to_bytes()))
    }

    /// Emit an audit event into the tamper-evident chain.
    ///
    /// This is the ONLY way to create audit records. Capsules cannot call this directly.
    ///
    /// FAIL-LOUD + FAIL-CLOSED: on a serialization or durable-write failure this returns `Err` AND
    /// logs at `error!`. A custody-relevant caller MUST propagate the `Err` and fail its operation
    /// closed (the open/grant did not make it into the custody trail). Best-effort callers may
    /// ignore the result; the loud log still fires.
    ///
    /// DURABILITY CONTRACT (unchanged by the S51 group commit): `Ok(())` means THIS record is
    /// durably on disk (written + `fsync`ed). What changed is HOW: concurrent emits append in
    /// chain order, then share fsyncs — one flusher's `sync_all` covers every record appended
    /// before it, so N concurrent emitters pay ~1 fsync instead of N (measured ~430× per-record
    /// fsync cost, S51 decision run; `benches/audit_emit.rs`). A failed append or fsync POISONS
    /// the log: the failing emit and every later one return `Err` (after a failed write/fsync the
    /// on-disk suffix is unknown, so retrying a seq could append a duplicate after a half-landed
    /// record and corrupt the chain — refusing is the only honest posture; a restart re-verifies
    /// the durable prefix, quarantining a torn never-acknowledged tail — see
    /// `quarantine_torn_tail`). The inverse direction is inherent to fsync semantics
    /// and unchanged: an emit that returned `Err` MAY still have landed on disk — callers already
    /// treat `Err` as "act did not happen", and an extra durable record is an over-record, never a
    /// lost one.
    pub fn emit(&self, event: AuditEvent) -> Result<(), AuditError> {
        let event_json =
            serde_json::to_string(&event).map_err(|e| AuditError::Serialize(e.to_string()))?;

        // Fast-path refusal on a poisoned log (an authoritative re-check runs UNDER the chain
        // lock below — this one just refuses cheap, before serializing the event).
        self.check_not_poisoned()?;

        // APPEND PHASE — the chain lock serializes seq assignment, signing, and the ORDERED append
        // (chain order IS append order), but no longer spans the fsync.
        let mut chain = self.chain.lock().map_err(|_| AuditError::Lock)?;
        // AUTHORITATIVE poison check, under the chain lock (council S51 guardian F1): a write
        // failure poisons BEFORE releasing the chain lock (below), so an emit that was queued on
        // the chain lock while another emit's append failed re-checks HERE and refuses — it can
        // never re-derive the failed seq and append after half-landed bytes (the duplicate-seq
        // corruption this closes). Lock order chain→flush is safe: nothing acquires `chain` while
        // holding the flush lock.
        self.check_not_poisoned()?;
        let seq = chain.last_seq + 1;
        let prev_hash = chain.prev_hash;
        let record_hash = compute_record_hash(seq, &prev_hash, event_json.as_bytes());

        let (alg, sig) = match &self.signer {
            Some(key) => {
                let signature: Signature = key.sign(&record_hash);
                (
                    AUDIT_SIG_ALG_ED25519.to_string(),
                    BASE64.encode(signature.to_bytes()),
                )
            }
            None => (AUDIT_SIG_ALG_NONE.to_string(), String::new()),
        };

        let record = ChainedRecord {
            seq,
            prev_hash: hex::encode(prev_hash),
            event: event.clone(),
            record_hash: hex::encode(record_hash),
            alg,
            sig,
        };
        let line =
            serde_json::to_string(&record).map_err(|e| AuditError::Serialize(e.to_string()))?;

        if self.echo_stdout {
            println!("[AUDIT] {}", line);
        }

        if let Some(writer) = &self.writer {
            // Append + flush THIS record's bytes to the OS, still under the chain lock (order).
            let write_res = {
                let mut w = writer.lock().map_err(|_| AuditError::Lock)?;
                writeln!(w, "{}", line).and_then(|_| w.flush())
            };
            if let Err(e) = write_res {
                tracing::error!("AUDIT durable-write failed (seq {seq}): {e}");
                // Poison while STILL HOLDING the chain lock (guardian F1): the next emit queued on
                // the chain lock must see the poison before it can compute this same seq and
                // append after our half-landed bytes. chain→flush nesting is acyclic (no path
                // takes `chain` while holding the flush lock).
                self.poison(format!("durable write failed at seq {seq}: {e}"));
                drop(chain);
                return Err(AuditError::Io(e.to_string()));
            }
            // The bytes are in the OS: advance the APPEND head and publish the flusher's cover
            // point. Durability is NOT yet promised — that is wait_durable's job.
            chain.last_seq = seq;
            chain.prev_hash = record_hash;
            self.written_seq.store(seq, Ordering::Release);
            drop(chain);

            // GROUP-COMMIT PHASE — block until an fsync (ours or a concurrent emitter's) covers
            // this seq. Only after this is the record a custody record.
            self.wait_durable(seq)?;
        } else {
            // Memory-only: nothing durable to wait for.
            chain.last_seq = seq;
            chain.prev_hash = record_hash;
            drop(chain);
        }

        // Store in the in-memory ring buffer (best-effort; the durable record is the source of
        // truth). Under concurrency the ring's arrival order may differ slightly from seq order —
        // it is an observability projection, never the chain.
        if let Ok(mut buffer) = self.memory_buffer.write() {
            if buffer.len() >= MAX_MEMORY_EVENTS {
                buffer.pop_front();
            }
            buffer.push_back(event);
        }
        Ok(())
    }

    /// Lock the flush state, RECOVERING from a panic-poisoned mutex (council S51 guardian F3):
    /// `FlushState` is three plain fields, each written in a single statement, so a panicked
    /// holder cannot leave it structurally invalid — refusing to recover would instead turn one
    /// panic into a permanent `AuditError::Lock` for every future custody emit (an unbounded
    /// outage, worse than fail-closed).
    fn lock_flush<'a>(flush: &'a Mutex<FlushState>) -> std::sync::MutexGuard<'a, FlushState> {
        flush
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Refuse if the durable log is poisoned (see [`FlushState::poisoned`]). `Ok` on memory-only
    /// logs (no durability to fail).
    fn check_not_poisoned(&self) -> Result<(), AuditError> {
        if let Some((flush, _)) = &self.flush {
            let fs = Self::lock_flush(flush);
            if let Some(why) = &fs.poisoned {
                return Err(AuditError::Io(format!(
                    "audit log poisoned by an earlier durability failure (fail-closed; a restart \
                     re-verifies the durable prefix — a torn tail needs operator attention): {why}"
                )));
            }
        }
        Ok(())
    }

    /// Mark the durable log permanently failed (until restart): every waiter and every future
    /// emit gets `Err`. Never called on a memory-only log. Callable while holding the CHAIN lock
    /// (guardian F1 — the write-failure arm must poison before releasing it): chain→flush nesting
    /// is acyclic because no path acquires `chain` while holding the flush lock.
    fn poison(&self, why: String) {
        if let Some((flush, wakeup)) = &self.flush {
            let mut fs = Self::lock_flush(flush);
            if fs.poisoned.is_none() {
                fs.poisoned = Some(why);
            }
            wakeup.notify_all();
        }
    }

    /// Block until `seq` is durably on disk (group commit, S51): serve from a completed fsync,
    /// wait out one in flight, or become the flusher. The flusher reads the cover point
    /// (`written_seq` — every record whose bytes reached the OS), fsyncs WITHOUT holding the chain
    /// lock (appends continue meanwhile), then publishes `durable_seq = cover` and wakes everyone;
    /// the head anchor advances to `cover` (durable seqs only — the anchor must never over-claim).
    /// An fsync failure poisons the log (see [`FlushState::poisoned`]).
    fn wait_durable(&self, seq: u64) -> Result<(), AuditError> {
        let Some((flush, wakeup)) = &self.flush else {
            // Unreachable for file-backed logs (constructors pair writer+flush); nothing to wait
            // for otherwise.
            return Ok(());
        };
        let mut fs = Self::lock_flush(flush);
        loop {
            if let Some(why) = &fs.poisoned {
                return Err(AuditError::Io(format!(
                    "audit record durability unknown — log poisoned (fail-closed): {why}"
                )));
            }
            if fs.durable_seq >= seq {
                return Ok(());
            }
            if fs.flushing {
                // An fsync is in flight; it may not cover us (we may have appended after its
                // cover point was read) — wait for its completion and re-check. Recover a
                // panic-poisoned wait the same way lock_flush does (see its doc).
                fs = wakeup
                    .wait(fs)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                continue;
            }
            // Become the flusher for everything appended so far. The guard clears `flushing` and
            // poisons on UNWIND (guardian F3): if anything in the flusher section panicked with
            // `flushing` stuck true, every later waiter would block forever with nothing left to
            // notify them — the guard converts that into the ordinary poisoned-log refusal.
            fs.flushing = true;
            drop(fs);
            struct FlusherGuard<'a> {
                log: &'a AuditLog,
                armed: bool,
            }
            impl Drop for FlusherGuard<'_> {
                fn drop(&mut self) {
                    if self.armed {
                        if let Some((flush, wakeup)) = &self.log.flush {
                            let mut fs = AuditLog::lock_flush(flush);
                            fs.flushing = false;
                            if fs.poisoned.is_none() {
                                fs.poisoned =
                                    Some("flusher panicked mid-flush (fail-closed)".to_string());
                            }
                            wakeup.notify_all();
                        }
                    }
                }
            }
            let mut guard = FlusherGuard {
                log: self,
                armed: true,
            };
            // Cover point BEFORE the fsync: all seqs ≤ written_seq have their bytes in the OS
            // (Release-stored under the chain lock after the BufWriter flush), so sync_all
            // durably commits every one of them. Our own seq is ≤ cover by construction.
            let cover = self.written_seq.load(Ordering::Acquire);
            // fsync the CLONED fd so appenders keep the writer mutex (see `sync_handle` — the
            // measured convoy fix); fall back to fsync-under-the-writer-lock if the clone failed.
            let sync_res = match &self.sync_handle {
                Some(h) => h.sync_all().map_err(|e| e.to_string()),
                None => match &self.writer {
                    Some(writer) => match writer.lock() {
                        Ok(w) => w.get_ref().sync_all().map_err(|e| e.to_string()),
                        Err(_) => Err("writer lock poisoned".to_string()),
                    },
                    None => Err("no writer to fsync (invariant violation)".to_string()),
                },
            };
            // Loud logging OUTSIDE the flush lock (a panicking tracing subscriber must not poison
            // it — guardian F3).
            if let Err(e) = &sync_res {
                tracing::error!("AUDIT sync_all failed (covering seq {cover}): {e}");
            }
            {
                let mut fs = Self::lock_flush(flush);
                fs.flushing = false;
                guard.armed = false; // completed normally — the guard must not poison
                match &sync_res {
                    Ok(()) => {
                        fs.durable_seq = fs.durable_seq.max(cover);
                        wakeup.notify_all();
                    }
                    Err(e) => {
                        fs.poisoned = Some(format!("fsync failed covering seq {cover}: {e}"));
                        wakeup.notify_all();
                        return Err(AuditError::Io(format!(
                            "audit record durability unknown — fsync failed (log poisoned, \
                             fail-closed): {e}"
                        )));
                    }
                }
            }
            // Tail-truncation anchor for the DURABLE cover — OFF the flush lock (it must never
            // hold up the waiters just woken above; the first S51 cut anchored under the flush
            // lock and serialized every group-commit cycle behind it). Guarded-monotone via its
            // own mutex (an overlapping later flusher cannot regress it); best-effort — a lagging
            // anchor under-claims, never lies.
            if let Some(path) = &self.log_path {
                if let Ok(mut anchored) = self.anchored_seq.lock() {
                    if cover > *anchored {
                        match write_head_anchor(path, cover) {
                            Ok(()) => *anchored = cover,
                            Err(e) => {
                                tracing::error!("AUDIT head-anchor write failed (seq {cover}): {e}")
                            }
                        }
                    }
                }
            }
            // The flusher's own record is covered by construction (seq ≤ written_seq ≤ cover).
            return Ok(());
        }
    }

    /// Walk the on-disk log and VERIFY the hash-chain + signatures end to end. Returns the number of
    /// records verified, or an error naming the first break (bad seq, broken link, wrong hash, or a
    /// signature that does not verify under `verifying_key`). This is the tamper-evidence check.
    pub fn verify_chain(&self, verifying_key: Option<&VerifyingKey>) -> Result<u64, String> {
        let path = match &self.log_path {
            Some(p) => p,
            None => return Err("no file-backed log to verify".to_string()),
        };
        let file = File::open(path).map_err(|e| format!("open audit log: {e}"))?;
        let reader = BufReader::new(file);

        let mut expected_seq: u64 = 1;
        let mut expected_prev = genesis_prev_hash();
        for line in reader.lines() {
            let line = line.map_err(|e| format!("read line: {e}"))?;
            if line.trim().is_empty() {
                continue;
            }
            let rec: ChainedRecord = serde_json::from_str(&line)
                .map_err(|e| format!("record {expected_seq} does not parse: {e}"))?;
            if rec.seq != expected_seq {
                return Err(format!(
                    "audit chain break: expected seq {expected_seq}, found {}",
                    rec.seq
                ));
            }
            let prev = hex::decode(&rec.prev_hash)
                .map_err(|e| format!("seq {}: bad prev_hash hex: {e}", rec.seq))?;
            if prev.as_slice() != expected_prev.as_slice() {
                return Err(format!(
                    "audit chain break at seq {}: prev_hash does not link",
                    rec.seq
                ));
            }
            let computed = recompute_record_hash(&rec, &expected_prev)
                .map_err(|e| format!("seq {}: re-serialize event: {e}", rec.seq))?;
            let claimed = hex::decode(&rec.record_hash)
                .map_err(|e| format!("seq {}: bad record_hash hex: {e}", rec.seq))?;
            if claimed.as_slice() != computed.as_slice() {
                return Err(format!(
                    "audit tamper at seq {}: record_hash mismatch (content edited)",
                    rec.seq
                ));
            }
            // Signature check. SECURITY (audit T4 — signature-downgrade): the
            // `alg` field is NOT part of the hashed preimage (see
            // `compute_record_hash`), so an offline editor with NO signing key
            // could otherwise rewrite events, recompute the (public) record_hash,
            // relink the chain, set `alg="none"`, drop `sig`, and pass — forging
            // a "verified" chain still advertising the real signer. Defeat that
            // by making the decision to check the signature independent of the
            // forgeable `alg`: whenever a verifying key is supplied (custody /
            // tamper-evidence mode), EVERY record MUST be ed25519-signed and
            // verify. A non-`ed25519` alg in a signed chain is a downgrade, not
            // an "unsigned record to skip".
            match verifying_key {
                Some(vk) => {
                    if rec.alg != AUDIT_SIG_ALG_ED25519 {
                        return Err(format!(
                            "audit tamper at seq {}: record is not ed25519-signed (alg={:?}) \
                             in a signed chain — signature downgrade refused",
                            rec.seq, rec.alg
                        ));
                    }
                    let sig_bytes = BASE64
                        .decode(&rec.sig)
                        .map_err(|e| format!("seq {}: bad signature base64: {e}", rec.seq))?;
                    let signature = Signature::from_slice(&sig_bytes)
                        .map_err(|e| format!("seq {}: malformed signature: {e}", rec.seq))?;
                    vk.verify(&computed, &signature).map_err(|_| {
                        format!("audit tamper at seq {}: signature does not verify", rec.seq)
                    })?;
                }
                None => {
                    // No key to verify against (unsigned / memory-only mode). We
                    // cannot validate a claimed signature, so refuse to report a
                    // signed chain as "verified" without its key rather than
                    // silently skipping — preserves the prior fail-closed stance.
                    if rec.alg == AUDIT_SIG_ALG_ED25519 {
                        return Err(format!(
                            "seq {}: record is ed25519-signed but no verifying key was supplied",
                            rec.seq
                        ));
                    }
                }
            }
            expected_prev = computed;
            expected_seq += 1;
        }
        Ok(expected_seq - 1)
    }

    /// A LIVE, full-chain integrity attestation for a READ surface (inspector / audit-artifact
    /// export) to project — closing the "verify-on-read is startup-only" gap. Walks the on-disk
    /// hash + signature chain under the log's OWN key and reports the result.
    ///
    /// `None` for a memory-only log: there is no durable chain to walk, and verify-on-read is
    /// meaningful only in durable mode. Honest under tamper: a broken chain reports
    /// `verified == false` with the first break, NEVER a fabricated ok.
    pub fn chain_attestation(&self) -> Option<ChainAttestation> {
        self.log_path.as_ref()?; // memory-only ⇒ nothing durable to attest
        let vk = self.signer.as_ref().map(|s| s.verifying_key());
        let signer = self.verifying_key_hex();
        Some(match self.verify_chain(vk.as_ref()) {
            Ok(records) => ChainAttestation {
                verified: true,
                records,
                signer,
                error: None,
            },
            Err(e) => ChainAttestation {
                verified: false,
                records: 0,
                signer,
                error: Some(e),
            },
        })
    }

    /// Sign the exact ordered record SET of a receipt with this log's key (base64 ed25519 over
    /// [`mandate_receipt_binding_message`]). The self-contained binding a verifier re-checks to
    /// detect any holder-side add/drop/reorder. `None` for an unsigned log.
    fn sign_receipt_set(
        &self,
        scope: &MandateReceiptScope,
        records: &[ChainedRecord],
    ) -> Option<String> {
        let signer = self.signer.as_ref()?;
        Some(
            BASE64.encode(
                signer
                    .sign(&mandate_receipt_binding_message(scope, records))
                    .to_bytes(),
            ),
        )
    }

    /// Export the durable chain as a PORTABLE [`MandateReceipt`] a third party can verify off-box
    /// with [`verify_mandate_receipt`] and NO runtime. `None` for a memory-only or unsigned log
    /// (nothing durable + signed to hand out).
    pub fn export_mandate_receipt(&self) -> Option<MandateReceipt> {
        self.export_mandate_receipt_range(1, u64::MAX)
    }

    /// As [`export_mandate_receipt`](Self::export_mandate_receipt), scoped to a `seq` range so a
    /// receipt can carry just ONE mandate + the actions taken under it (`[from_seq, to_seq]`,
    /// inclusive) rather than the whole history — the shape an auditor is handed per delegation.
    pub fn export_mandate_receipt_range(
        &self,
        from_seq: u64,
        to_seq: u64,
    ) -> Option<MandateReceipt> {
        let path = self.log_path.as_ref()?;
        let signer_public_key_hex = self.verifying_key_hex()?; // unsigned ⇒ no verifiable receipt
        let file = File::open(path).ok()?;
        let reader = BufReader::new(file);
        let mut records = Vec::new();
        for line in reader.lines() {
            let line = line.ok()?;
            if line.trim().is_empty() {
                continue;
            }
            let record: ChainedRecord = serde_json::from_str(&line).ok()?;
            if record.seq >= from_seq && record.seq <= to_seq {
                records.push(record);
            }
        }
        if records.is_empty() {
            return None;
        }
        let scope = MandateReceiptScope::Contiguous;
        let set_binding = self.sign_receipt_set(&scope, &records);
        Some(MandateReceipt {
            schema: MANDATE_RECEIPT_SCHEMA.to_string(),
            scope,
            signer_public_key_hex,
            records,
            set_binding,
        })
    }

    /// Export a PER-MANDATE receipt: EVERY durable record bound to one capability `token_id` — the
    /// grant (the mandate) plus every use / revoke under it — regardless of where they sit in the
    /// interleaved chain. This is the delegation-shaped artifact you hand an auditor: "here is the
    /// authorization, and here are the actions taken under it." `None` for a memory-only/unsigned
    /// log or a `token_id` with no records. Verified with [`MandateReceiptScope::Capability`].
    ///
    /// COMPLETENESS is bounded (see that scope's docs): the `set_binding` signature makes the record
    /// set tamper-evident against any HOLDER in transit, but a compromised key-holding issuer could
    /// still sign a selective set. Unlike a `Contiguous` receipt, this does NOT prove the issuer
    /// omitted no action at export — the bundle carries no such attestation, and none is implied.
    pub fn export_mandate_receipt_for_capability(&self, token_id: &str) -> Option<MandateReceipt> {
        let path = self.log_path.as_ref()?;
        let signer_public_key_hex = self.verifying_key_hex()?;
        let file = File::open(path).ok()?;
        let reader = BufReader::new(file);
        let mut records = Vec::new();
        for line in reader.lines() {
            let line = line.ok()?;
            if line.trim().is_empty() {
                continue;
            }
            let record: ChainedRecord = serde_json::from_str(&line).ok()?;
            if record.event.capability_token_id() == Some(token_id) {
                records.push(record);
            }
        }
        if records.is_empty() {
            return None;
        }
        let scope = MandateReceiptScope::Capability {
            token_id: token_id.to_string(),
        };
        let set_binding = self.sign_receipt_set(&scope, &records);
        Some(MandateReceipt {
            schema: MANDATE_RECEIPT_SCHEMA.to_string(),
            scope,
            signer_public_key_hex,
            records,
            set_binding,
        })
    }

    /// Best-effort emit for NON-custody events (capsule lifecycle, capability use, etc.): logs
    /// loudly at `error!` on failure but never propagates. This is the "fail-loud but don't block"
    /// half of the split — custody callers ([`content_open`](Self::content_open) and direct
    /// [`emit`](Self::emit)) instead propagate the `Err` and fail their operation closed.
    pub fn emit_best_effort(&self, event: AuditEvent) {
        if let Err(e) = self.emit(event) {
            tracing::error!("AUDIT append failed (best-effort event dropped): {e}");
        }
    }

    /// Tally the intent-proof issues (denied / diverged / undelivered) recorded for
    /// `capsule_id` over the IN-MEMORY audit buffer — the recent window the log retains.
    /// PRESENCE-aware: `None` when the capsule has no buffered intent activity (ABSENT —
    /// it never went through the gate, so it is not "clean"); `Some(counts)` when it has
    /// any intent event. This is the cheap live signal the inspector projects as the
    /// intent-proof custody channel; full-history counts would need a durable walk. A
    /// poisoned lock degrades to `None` (absent), never a fabricated clean tally.
    pub fn intent_proof_summary(
        &self,
        capsule_id: &str,
    ) -> Option<crate::capability::intent::IntentProofSummary> {
        match self.memory_buffer.read() {
            Ok(buf) => crate::capability::intent::count_intent_proof(buf.iter(), capsule_id),
            Err(_) => None,
        }
    }

    /// Emit the GAP-8 CONTENT-OPEN custody record and REQUIRE it to be durably committed.
    ///
    /// Returns `Err` if the record could not be hash-chained + signed + `fsync`ed. The dDRM open
    /// path MUST treat that as fail-closed: if the who-opened-what-when record cannot be written to
    /// the tamper-evident log, the open does not proceed (custody integrity over availability).
    #[allow(clippy::too_many_arguments)]
    pub fn content_open(
        &self,
        session_id: &str,
        principal_id: &str,
        content_id: &str,
        action: &str,
        decision: &str,
        source: &str,
        grant_digest: Option<&str>,
    ) -> Result<(), AuditError> {
        self.emit(AuditEvent::ContentOpen {
            timestamp: SecureTimestamp::now(),
            session_id: session_id.to_string(),
            principal_id: principal_id.to_string(),
            content_id: content_id.to_string(),
            action: action.to_string(),
            decision: decision.to_string(),
            source: source.to_string(),
            grant_digest: grant_digest.map(str::to_string),
        })
    }

    /// Emit a runtime start event
    pub fn runtime_start(&self, version: &str) {
        self.emit_best_effort(AuditEvent::RuntimeStart {
            timestamp: SecureTimestamp::now(),
            version: version.to_string(),
        });
    }

    /// Emit a runtime stop event
    pub fn runtime_stop(&self) {
        self.emit_best_effort(AuditEvent::RuntimeStop {
            timestamp: SecureTimestamp::now(),
        });
    }

    /// Emit a capsule launch event
    pub fn capsule_launch(
        &self,
        capsule_id: &str,
        capsule_name: &str,
        cid: Option<&str>,
        trust_level: TrustLevel,
    ) {
        self.emit_best_effort(AuditEvent::CapsuleLaunch {
            timestamp: SecureTimestamp::now(),
            capsule_id: capsule_id.to_string(),
            capsule_name: capsule_name.to_string(),
            cid: cid.map(String::from),
            trust_level,
        });
    }

    /// Emit a capsule stop event
    pub fn capsule_stop(&self, capsule_id: &str, reason: StopReason) {
        self.emit_best_effort(AuditEvent::CapsuleStop {
            timestamp: SecureTimestamp::now(),
            capsule_id: capsule_id.to_string(),
            reason,
        });
    }

    /// Emit a capability grant event
    pub fn capability_grant(
        &self,
        token_id: &TokenId,
        capsule_id: &str,
        resource: &ResourceId,
        action: Action,
        expiry: Option<SecureTimestamp>,
    ) {
        self.emit_best_effort(AuditEvent::CapabilityGrant {
            timestamp: SecureTimestamp::now(),
            token_id: token_id.to_string(),
            capsule_id: capsule_id.to_string(),
            resource: resource.to_string(),
            action: action.to_string(),
            expiry,
            // The legacy best-effort helper predates the liability binding; the durable mandate
            // mint (`grant_durable`) is the path that records the responsible entity.
            responsible_entity: None,
        });
    }

    /// Emit a capability revoke event
    pub fn capability_revoke(&self, token_id: &TokenId, reason: &str) {
        self.emit_best_effort(AuditEvent::CapabilityRevoke {
            timestamp: SecureTimestamp::now(),
            token_id: token_id.to_string(),
            reason: reason.to_string(),
        });
    }

    /// Emit a capability use event
    pub fn capability_use(
        &self,
        token_id: &TokenId,
        capsule_id: &str,
        resource: &ResourceId,
        action: Action,
        success: bool,
    ) {
        self.capability_use_with_rail_ref(token_id, capsule_id, resource, action, success, None);
    }

    /// Emit a `CapabilityUse` carrying the rail reference for an act that settled on an external
    /// rail (Sprint 34 — a `runtime.pay` DRM buy's tx hash + `operative:tokenId`). Passing `None`
    /// is exactly [`capability_use`](Self::capability_use); the field is omitted from the signed
    /// preimage, so a rail-less use stays byte-identical to a pre-S34 record.
    pub fn capability_use_with_rail_ref(
        &self,
        token_id: &TokenId,
        capsule_id: &str,
        resource: &ResourceId,
        action: Action,
        success: bool,
        rail_ref: Option<String>,
    ) {
        self.emit_best_effort(AuditEvent::CapabilityUse {
            timestamp: SecureTimestamp::now(),
            token_id: token_id.to_string(),
            capsule_id: capsule_id.to_string(),
            resource: resource.to_string(),
            action: action.to_string(),
            success,
            rail_ref,
        });
    }

    /// Emit a content fetch event
    pub fn content_fetch(&self, cid: &str, source: FetchSource, success: bool) {
        self.emit_best_effort(AuditEvent::ContentFetch {
            timestamp: SecureTimestamp::now(),
            cid: cid.to_string(),
            source,
            success,
        });
    }

    /// Emit an epoch advance event
    pub fn epoch_advance(&self, old_epoch: u64, new_epoch: u64, reason: &str) {
        self.emit_best_effort(AuditEvent::EpochAdvance {
            timestamp: SecureTimestamp::now(),
            old_epoch,
            new_epoch,
            reason: reason.to_string(),
        });
    }

    /// Emit a storage access event
    pub fn storage_access(
        &self,
        session_id: &str,
        user_id: &str,
        uri: &str,
        action: &str,
        success: bool,
    ) {
        self.emit_best_effort(AuditEvent::StorageAccess {
            timestamp: SecureTimestamp::now(),
            session_id: session_id.to_string(),
            user_id: user_id.to_string(),
            uri: uri.to_string(),
            action: action.to_string(),
            success,
        });
    }

    /// Emit a security warning
    pub fn security_warning(&self, warning_type: &str, details: &str) {
        self.emit_best_effort(AuditEvent::SecurityWarning {
            timestamp: SecureTimestamp::now(),
            warning_type: warning_type.to_string(),
            details: details.to_string(),
        });
    }

    /// Record an inter-capsule message
    pub fn message_sent(&self, from: &str, to: &str, size_bytes: usize) {
        self.emit_best_effort(AuditEvent::MessageSent {
            timestamp: SecureTimestamp::now(),
            from: from.to_string(),
            to: to.to_string(),
            size_bytes,
        });
    }

    /// Emit a policy proposal event
    pub fn policy_proposal(
        &self,
        request_id: &str,
        recommended_outcome: &str,
        confidence: f32,
        rationale: &str,
    ) {
        self.emit_best_effort(AuditEvent::PolicyProposal {
            timestamp: SecureTimestamp::now(),
            request_id: request_id.to_string(),
            recommended_outcome: recommended_outcome.to_string(),
            confidence,
            rationale: rationale.to_string(),
        });
    }

    /// Emit a policy divergence event (real and shadow verifiers disagree)
    #[allow(clippy::too_many_arguments)]
    pub fn policy_divergence(
        &self,
        request_id: &str,
        real_decision_id: &str,
        shadow_decision_id: &str,
        real_outcome: &str,
        shadow_outcome: &str,
        real_rationale: &str,
        shadow_rationale: &str,
    ) {
        self.emit_best_effort(AuditEvent::PolicyDivergence {
            timestamp: SecureTimestamp::now(),
            request_id: request_id.to_string(),
            real_decision_id: real_decision_id.to_string(),
            shadow_decision_id: shadow_decision_id.to_string(),
            real_outcome: real_outcome.to_string(),
            shadow_outcome: shadow_outcome.to_string(),
            real_rationale: real_rationale.to_string(),
            shadow_rationale: shadow_rationale.to_string(),
        });
    }

    /// Emit a policy decision made event
    #[allow(clippy::too_many_arguments)]
    pub fn policy_decision_made(
        &self,
        decision_id: &str,
        request_id: &str,
        outcome: &str,
        checks_passed: usize,
        checks_failed: usize,
        shadow: bool,
        rationale: &str,
    ) {
        self.emit_best_effort(AuditEvent::PolicyDecisionMade {
            timestamp: SecureTimestamp::now(),
            decision_id: decision_id.to_string(),
            request_id: request_id.to_string(),
            outcome: outcome.to_string(),
            checks_passed,
            checks_failed,
            shadow,
            rationale: rationale.to_string(),
        });
    }

    /// Get the log file path (if configured)
    pub fn log_path(&self) -> Option<&Path> {
        self.log_path.as_deref()
    }

    /// Get recent events from memory buffer
    ///
    /// Returns the most recent `limit` events, newest first.
    pub fn recent_events(&self, limit: usize) -> Vec<AuditEvent> {
        if let Ok(buffer) = self.memory_buffer.read() {
            buffer.iter().rev().take(limit).cloned().collect()
        } else {
            Vec::new()
        }
    }

    /// Get recent events filtered by type
    ///
    /// Returns the most recent events matching the filter, newest first.
    pub fn recent_events_filtered(
        &self,
        limit: usize,
        event_type: Option<&str>,
    ) -> Vec<AuditEvent> {
        if let Ok(buffer) = self.memory_buffer.read() {
            buffer
                .iter()
                .rev()
                .filter(|e| {
                    if let Some(filter) = event_type {
                        e.event_type_name() == filter
                    } else {
                        true
                    }
                })
                .take(limit)
                .cloned()
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Get events from file (reads entire log file)
    ///
    /// Returns all events from file, or events from memory if no file configured.
    /// For large logs, prefer recent_events() which uses the memory buffer.
    pub fn read_from_file(&self, limit: usize) -> Vec<AuditEvent> {
        if let Some(path) = &self.log_path {
            if let Ok(file) = File::open(path) {
                let reader = BufReader::new(file);
                // Lines are `ChainedRecord`s; extract the inner event. Lines that don't parse as a
                // chained record (e.g. a legacy pre-chain log) are skipped rather than aborting.
                let events: Vec<AuditEvent> = reader
                    .lines()
                    .map_while(Result::ok)
                    .filter_map(|line| serde_json::from_str::<ChainedRecord>(&line).ok())
                    .map(|rec| rec.event)
                    .collect();

                // Return last `limit` events
                let start = events.len().saturating_sub(limit);
                return events[start..].to_vec();
            }
        }

        // Fall back to memory buffer
        self.recent_events(limit)
    }

    /// Whether the audit history records that `principal_id` SUCCESSFULLY OPENED `content_id` — a
    /// `ContentOpen` with matching `principal_id` and `decision == "opened"`. PRINCIPAL-SCOPED: it
    /// answers "did THIS principal access X", never "did anyone" — so it cannot be a cross-principal
    /// existence oracle. (`ContentFetch` is deliberately NOT counted: it carries no principal, so it
    /// cannot be attributed.) A state-dependent, side-effect-free read whose answer varies with what
    /// the principal actually did. Streams the durable log and EARLY-RETURNS on the first match (O(1)
    /// memory); falls back to the in-memory buffer for an unsigned/memory-only log. A `false` over a
    /// file-backed log is authoritative for the DURABLE history.
    pub fn principal_opened_content(&self, principal_id: &str, content_id: &str) -> bool {
        fn matches(event: &AuditEvent, principal: &str, id: &str) -> bool {
            matches!(
                event,
                AuditEvent::ContentOpen { content_id, principal_id, decision, .. }
                    if content_id == id && principal_id == principal && decision == "opened"
            )
        }
        if let Some(path) = &self.log_path {
            if let Ok(file) = File::open(path) {
                for line in BufReader::new(file).lines().map_while(Result::ok) {
                    if line.trim().is_empty() {
                        continue;
                    }
                    if let Ok(rec) = serde_json::from_str::<ChainedRecord>(&line) {
                        if matches(&rec.event, principal_id, content_id) {
                            return true;
                        }
                    }
                }
                return false;
            }
        }
        self.memory_buffer
            .read()
            .map(|buf| buf.iter().any(|e| matches(e, principal_id, content_id)))
            .unwrap_or(false)
    }

    /// Get total event count in memory buffer
    pub fn event_count(&self) -> usize {
        if let Ok(buffer) = self.memory_buffer.read() {
            buffer.len()
        } else {
            0
        }
    }
}

impl AuditEvent {
    /// Get the event type name as a string
    pub fn event_type_name(&self) -> &'static str {
        match self {
            AuditEvent::RuntimeStart { .. } => "runtime_start",
            AuditEvent::RuntimeStop { .. } => "runtime_stop",
            AuditEvent::CapsuleLaunch { .. } => "capsule_launch",
            AuditEvent::CapsuleStop { .. } => "capsule_stop",
            AuditEvent::CapabilityGrant { .. } => "capability_grant",
            AuditEvent::CapabilityRevoke { .. } => "capability_revoke",
            AuditEvent::CapabilityUse { .. } => "capability_use",
            AuditEvent::ContentFetch { .. } => "content_fetch",
            AuditEvent::ContentOpen { .. } => "content_open",
            AuditEvent::AuthAttempt { .. } => "auth_attempt",
            AuditEvent::EpochAdvance { .. } => "epoch_advance",
            AuditEvent::ConfigChange { .. } => "config_change",
            AuditEvent::SecurityWarning { .. } => "security_warning",
            AuditEvent::SessionCreated { .. } => "session_created",
            AuditEvent::SessionDestroyed { .. } => "session_destroyed",
            AuditEvent::CapabilityRequested { .. } => "capability_requested",
            AuditEvent::CapabilityDenied { .. } => "capability_denied",
            AuditEvent::CapabilityApproved { .. } => "capability_approved",
            AuditEvent::SpendDebit { .. } => "spend_debit",
            AuditEvent::BudgetExhausted { .. } => "budget_exhausted",
            AuditEvent::EgressDenied { .. } => "egress_denied",
            AuditEvent::IntentDeclared { .. } => "intent_declared",
            AuditEvent::IntentDenied { .. } => "intent_denied",
            AuditEvent::IntentReconciled { .. } => "intent_reconciled",
            AuditEvent::IdentityRegistered { .. } => "identity_registered",
            AuditEvent::StorageAccess { .. } => "storage_access",
            AuditEvent::MessageSent { .. } => "message_sent",
            AuditEvent::PolicyProposal { .. } => "policy_proposal",
            AuditEvent::PolicyDecisionMade { .. } => "policy_decision_made",
            AuditEvent::PolicyDivergence { .. } => "policy_divergence",
            AuditEvent::Custom { .. } => "custom",
        }
    }

    /// The capability `token_id` this event pertains to — the identifier that binds a mandate
    /// (`CapabilityGrant`) to the actions taken under it (`CapabilityUse`) and its revocation
    /// (`CapabilityRevoke`). `None` for events that are not scoped to a single capability token.
    /// Used to filter the chain into a per-mandate receipt.
    pub fn capability_token_id(&self) -> Option<&str> {
        match self {
            AuditEvent::CapabilityGrant { token_id, .. }
            | AuditEvent::CapabilityRevoke { token_id, .. }
            | AuditEvent::CapabilityUse { token_id, .. } => Some(token_id.as_str()),
            _ => None,
        }
    }

    /// True iff this event is the authorization (the mandate itself) — a `CapabilityGrant`.
    pub fn is_capability_grant(&self) -> bool {
        matches!(self, AuditEvent::CapabilityGrant { .. })
    }
}

impl Default for AuditLog {
    fn default() -> Self {
        Self::new()
    }
}

// Bridge: implement namespace crate's audit traits for the runtime's AuditLog

impl elastos_namespace::AuditSink for AuditLog {
    fn content_fetch(
        &self,
        identifier: &str,
        source: elastos_namespace::FetchSource,
        verified: bool,
    ) {
        let runtime_source = match source {
            elastos_namespace::FetchSource::LocalCache => FetchSource::LocalCache,
            elastos_namespace::FetchSource::IpfsGateway(gw) => FetchSource::IpfsGateway(gw),
        };
        self.content_fetch(identifier, runtime_source, verified);
    }
}

impl elastos_namespace::NamespaceAuditSink for AuditLog {
    fn namespace_loaded(&self, owner: &str) {
        self.emit_best_effort(AuditEvent::Custom {
            event_type: "namespace_loaded".to_string(),
            details: serde_json::json!({ "owner": owner }),
        });
    }

    fn namespace_created(&self, owner: &str) {
        self.emit_best_effort(AuditEvent::Custom {
            event_type: "namespace_created".to_string(),
            details: serde_json::json!({ "owner": owner }),
        });
    }

    fn namespace_saved(&self, owner: &str, cid: &str) {
        self.emit_best_effort(AuditEvent::Custom {
            event_type: "namespace_saved".to_string(),
            details: serde_json::json!({ "owner": owner, "cid": cid }),
        });
    }
}

/// Resume the hash-chain head from an existing log so appends stay continuous across restarts.
/// Reads the LAST parseable [`ChainedRecord`] and returns its `seq` + `record_hash` as the next
/// link. A missing/empty/legacy-format log starts at genesis (seq 0). Tolerant by design: a
/// best-effort resume must not crash startup — `verify_chain` is the authoritative integrity gate.
fn resume_chain_state(path: &Path) -> ChainState {
    let genesis = ChainState {
        last_seq: 0,
        prev_hash: genesis_prev_hash(),
    };
    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return genesis,
    };
    let mut last: Option<ChainedRecord> = None;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if let Ok(rec) = serde_json::from_str::<ChainedRecord>(&line) {
            last = Some(rec);
        }
    }
    match last {
        Some(rec) => match hex::decode(&rec.record_hash) {
            Ok(h) if h.len() == 32 => {
                let mut prev = [0u8; 32];
                prev.copy_from_slice(&h);
                ChainState {
                    last_seq: rec.seq,
                    prev_hash: prev,
                }
            }
            _ => genesis,
        },
        None => genesis,
    }
}

/// Load (or first-time create) the DEDICATED, PERSISTED ed25519 signing key that signs the audit
/// chain. Stored at `<log>.signing-key` (32-byte seed) with `0600` perms; the verifying key is
/// published to `<log>.pubkey` (hex) for verifiers. Persisting it keeps the chain verifiable across
/// restarts and avoids a throwaway in-memory key. NOTE: this is host-file protection, NOT an HSM —
/// see the module-level live-compromise caveat; hardware/keystore custody is roadmap.
fn load_or_create_signer(log_path: &Path) -> std::io::Result<SigningKey> {
    let key_path = sibling(log_path, "signing-key");
    if let Ok(seed) = std::fs::read(&key_path) {
        if seed.len() == 32 {
            let mut s = [0u8; 32];
            s.copy_from_slice(&seed);
            return Ok(SigningKey::from_bytes(&s));
        }
        tracing::error!(
            "audit signing key at {:?} is malformed ({} bytes) — refusing to overwrite; \
             remove it to regenerate",
            key_path,
            seed.len()
        );
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "malformed audit signing key",
        ));
    }
    // First run: generate, persist with restrictive perms, publish the verifying key.
    let signing_key = SigningKey::generate(&mut rand::thread_rng());
    write_private(&key_path, &signing_key.to_bytes())?;
    let pub_path = sibling(log_path, "pubkey");
    // Publishing the verifying key is best-effort (the signing key is already durably held and
    // the chain stays verifiable via an out-of-band key copy) — but a silent failure would leave
    // verifiers with no key file and no clue, so say it loudly.
    if let Err(e) = std::fs::write(
        &pub_path,
        hex::encode(signing_key.verifying_key().to_bytes()),
    ) {
        tracing::error!(
            "audit verifying key could not be published to {}: {e} — verifiers have no key file \
             until it is written out of band",
            pub_path.display()
        );
    }
    Ok(signing_key)
}

/// Persist the committed chain-head sequence to `<log>.head-anchor` (atomic temp+rename) so a later
/// [`AuditLog::with_file_verified`] can detect TAIL TRUNCATION — records sliced off the END that a
/// plain chain walk would never notice (it would just observe a shorter, validly-linked chain).
///
/// Best-effort by design: a failed anchor write is logged but never fails the emit, because the
/// audit RECORD is already durably committed and the log is written BEFORE the anchor, so
/// `anchor_seq <= on-disk head` always holds — a stale/lagging anchor can only ever under-count
/// (never a false truncation alarm, never a false loss).
///
/// THREAT MODEL (honest): the anchor is an unsigned host file on the same disk. It defends against
/// truncation by something that does NOT also rewrite the anchor (log-rotation bugs, partial tamper,
/// naive `tail`/`truncate` deletion). A full-disk attacker who rewrites BOTH is not stopped here —
/// that needs an off-box / co-signed anchor (roadmap, same custody caveat as the signing key).
fn write_head_anchor(log_path: &Path, committed_seq: u64) -> std::io::Result<()> {
    let anchor_path = sibling(log_path, "head-anchor");
    let tmp_path = sibling(log_path, "head-anchor.tmp");
    // DELIBERATELY NOT fsynced (measured, S51 council fold): a single-threaded emitter is its own
    // flusher, so an anchor fsync would DOUBLE its per-record durable cost (~950 µs → ~1.7 ms
    // measured on the S51 box — the first fold cut did exactly that and the bench rejected it).
    // Crash safety without it (guardian F4's brick scenario): the payload is a ≤20-byte decimal in
    // ONE sector, so a crash leaves the renamed anchor either COMPLETE or (on filesystems with no
    // rename-data-ordering heuristic) EMPTY — never partial garbage — and `read_head_anchor`
    // treats EMPTY as absent: the floor is skipped for that one open, loudly, and the next flush
    // rewrites it. A stale-but-complete older anchor is the ordinary best-effort lag (never lies
    // high — see the caller's ordering).
    std::fs::write(&tmp_path, committed_seq.to_string())?;
    std::fs::rename(&tmp_path, &anchor_path)
}

/// Read the committed chain-head sequence from `<log>.head-anchor`.
///
/// - `Ok(None)` — no anchor (a pre-anchor log or a brand-new file), OR an EMPTY anchor file (the
///   pre-S51 non-fsynced writer could land one across a crash — defense in depth, warned loudly;
///   treating it as absent only SKIPS the truncation check, never invents a floor). The check is
///   skipped.
/// - `Ok(Some(seq))` — a well-formed anchor (the LOWER bound on how many records must be present).
/// - `Err` — the anchor has non-empty garbage; fail-CLOSED: the single-sector atomic write means a
///   crash yields a complete or EMPTY anchor (handled above), never partial garbage — so non-empty
///   garbage in durable mode is genuinely suspicious.
fn read_head_anchor(log_path: &Path) -> std::io::Result<Option<u64>> {
    let anchor_path = sibling(log_path, "head-anchor");
    match std::fs::read_to_string(&anchor_path) {
        Ok(s) if s.trim().is_empty() => {
            tracing::warn!(
                "audit head-anchor at {anchor_path:?} is EMPTY (a pre-S51 crash artifact) — \
                 treating as absent; the tail-truncation floor is unavailable for this open"
            );
            Ok(None)
        }
        Ok(s) => s.trim().parse::<u64>().map(Some).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("audit head-anchor at {anchor_path:?} is unparseable: {e}"),
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Recover a TORN TAIL at open (council S51 guardian F2 / red-team F1): if the log does not end
/// with `\n`, its trailing partial line is quarantined to `<log>.torn-tail` and the log truncated
/// back to the last newline, so both open modes resume from an intact prefix instead of (verified)
/// refusing a healthy log or (plain) appending onto the fragment and merging two records into one
/// garbage line.
///
/// WHY THIS IS SAFE, precisely:
/// - A record is acknowledged (`emit` → `Ok`) only after a successful fsync of its FULL
///   `line\n` write — so a file whose last bytes lack the terminating `\n` PROVES the fragment
///   was never acknowledged (a crash tore it mid-writeback, or its write failed and poisoned the
///   log). Removing it can never remove an acknowledged custody record.
/// - It grants a tamperer nothing: beyond the head-anchor floor, a clean cut at a line boundary
///   is already undetectable (the anchor is the only lower bound), so laundering a cut as a
///   "torn tail" adds no power; at or below the floor, the anchor check still refuses.
/// - The fragment is PRESERVED (appended to the sidecar with a marker line), not destroyed —
///   append-only in spirit: nothing acknowledged is ever dropped, and even the torn bytes remain
///   inspectable.
///
/// A torn/corrupt line in the MIDDLE of the file (terminated by `\n`) is NOT recovered — that is
/// indistinguishable from tamper and stays fail-closed at the verified open.
fn quarantine_torn_tail(path: &Path) -> std::io::Result<()> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if bytes.is_empty() || bytes.ends_with(b"\n") {
        return Ok(());
    }
    let keep = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    let fragment = &bytes[keep..];
    let sidecar = sibling(path, "torn-tail");
    {
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&sidecar)?;
        writeln!(
            f,
            "--- torn tail quarantined ({} bytes) ---",
            fragment.len()
        )?;
        f.write_all(fragment)?;
        writeln!(f)?;
        f.sync_all()?;
    }
    // Truncate AFTER the fragment is durably in the sidecar — a crash between the two leaves the
    // fragment in both places (harmless duplicate), never in neither.
    let f = OpenOptions::new().write(true).open(path)?;
    f.set_len(keep as u64)?;
    f.sync_all()?;
    tracing::warn!(
        "audit log at {path:?} had a torn (never-acknowledged) trailing fragment of {} bytes — \
         quarantined to {sidecar:?} and resumed from the intact prefix",
        fragment.len()
    );
    Ok(())
}

/// Build a sibling path `<log>.<suffix>` next to the audit log.
fn sibling(log_path: &Path, suffix: &str) -> PathBuf {
    let mut name = log_path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".");
    name.push(suffix);
    log_path.with_file_name(name)
}

/// Write a secret file, creating it `0600` on Unix (owner read/write only).
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(bytes)?;
        f.flush()?;
        f.sync_all()?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_event_serialization() {
        let event = AuditEvent::CapsuleLaunch {
            timestamp: SecureTimestamp::now(),
            capsule_id: "cap-123".to_string(),
            capsule_name: "test-capsule".to_string(),
            cid: Some("Qm123".to_string()),
            trust_level: TrustLevel::Trusted,
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("capsule_launch"));
        assert!(json.contains("cap-123"));
    }

    #[test]
    fn test_audit_log_memory() {
        let log = AuditLog::new();
        log.runtime_start("0.1.0");
        log.capsule_launch("cap-1", "test", None, TrustLevel::Untrusted);
        log.capsule_stop("cap-1", StopReason::Completed);
        log.runtime_stop();
        // No panic = success (memory-only log doesn't persist)
    }

    #[test]
    fn test_audit_log_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let log_path = temp_dir.path().join("audit.log");

        let log = AuditLog::with_file(&log_path).unwrap();
        log.runtime_start("0.1.0");
        log.capsule_launch("cap-1", "test", None, TrustLevel::Trusted);

        // Read back the log
        let content = std::fs::read_to_string(&log_path).unwrap();
        assert!(content.contains("runtime_start"));
        assert!(content.contains("capsule_launch"));
    }

    // --- GAP-8: tamper-evident, hash-chained, signed audit ----------------------------------

    fn read_verifying_key(log: &AuditLog) -> VerifyingKey {
        let hex = log
            .verifying_key_hex()
            .expect("a file-backed log must be signed");
        let bytes: [u8; 32] = hex::decode(&hex).unwrap().try_into().unwrap();
        VerifyingKey::from_bytes(&bytes).unwrap()
    }

    // ---- Portable mandate receipt (Sprint 1, Item 2) ----------------------------------------
    // Emit a small chain to a signed, file-backed log and return an exported receipt.
    fn emit_and_export_receipt() -> (tempfile::TempDir, MandateReceipt) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");
        let log = AuditLog::with_file(&path).unwrap();
        log.emit(AuditEvent::RuntimeStart {
            timestamp: SecureTimestamp::now(),
            version: "mandate".to_string(),
        })
        .expect("emit mandate");
        log.content_open(
            "sess-1",
            "person:local:alice",
            "elastos://content/abc",
            "view",
            "opened",
            "chain-provider",
            None,
        )
        .expect("emit action");
        let receipt = log
            .export_mandate_receipt()
            .expect("a signed file-backed log exports a receipt");
        (dir, receipt)
    }

    #[test]
    fn mandate_receipt_verifies_standalone_over_the_wire() {
        let (_dir, receipt) = emit_and_export_receipt();
        let pin = receipt.signer_public_key_hex.clone();
        // Round-trip through JSON to prove it is a PORTABLE document, then verify with ONLY the
        // receipt — no AuditLog, no disk, no runtime. This is the "an auditor can check it" primitive.
        let wire = serde_json::to_string(&receipt).unwrap();
        let received: MandateReceipt = serde_json::from_str(&wire).unwrap();

        // Structural check (no pin): sound, but NOT authenticity.
        let structural = verify_mandate_receipt(&received, None);
        assert!(
            structural.structurally_valid,
            "clean receipt is structurally valid: {structural:?}"
        );
        assert!(!structural.authenticated, "no pin ⇒ not authenticated");
        assert_eq!(structural.signer_matches_expected, None);
        assert!(structural.hashes_ok && structural.signatures_ok && structural.chain_linkage_ok);
        assert_eq!(structural.records, 2);
        assert!(
            structural.starts_at_genesis,
            "the export begins at seq 1 / genesis"
        );

        // Pinned to the real signer: AUTHENTIC (the bit an auditor acts on).
        let authentic = verify_mandate_receipt(&received, Some(&pin));
        assert!(
            authentic.authenticated,
            "pinned to the true signer ⇒ authenticated: {authentic:?}"
        );
        assert_eq!(authentic.signer_matches_expected, Some(true));
    }

    /// The security property both reviewers demanded: a receipt an ATTACKER self-signed is
    /// structurally valid under its own key, but is NEVER `authenticated` without an out-of-band pin,
    /// and fails authentication when pinned to the real (different) signer.
    #[test]
    fn mandate_receipt_requires_signer_pinning_for_authenticity() {
        let (_dir, real) = emit_and_export_receipt();
        let real_signer = real.signer_public_key_hex.clone();

        // Attacker fabricates their OWN chain, signs it with their OWN key, and ships a receipt.
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::with_file(dir.path().join("evil.log")).unwrap();
        log.emit(AuditEvent::RuntimeStart {
            timestamp: SecureTimestamp::now(),
            version: "fabricated".to_string(),
        })
        .unwrap();
        let forged = log.export_mandate_receipt().unwrap();

        // It is structurally valid under its OWN key...
        let structural = verify_mandate_receipt(&forged, None);
        assert!(structural.structurally_valid);
        assert!(
            !structural.authenticated,
            "self-signed is not authentic without a pin"
        );

        // ...but pinning to the REAL runtime's signer exposes it: not the expected signer.
        let against_real = verify_mandate_receipt(&forged, Some(&real_signer));
        assert!(
            !against_real.authenticated,
            "forged receipt must fail against the pinned real signer"
        );
        assert_eq!(against_real.signer_matches_expected, Some(false));
    }

    #[test]
    fn mandate_receipt_detects_event_tamper() {
        let (_dir, mut receipt) = emit_and_export_receipt();
        // Edit a recorded event AFTER export: the record_hash no longer recomputes.
        if let AuditEvent::RuntimeStart { version, .. } = &mut receipt.records[0].event {
            *version = "tampered".to_string();
        } else {
            panic!("expected RuntimeStart at records[0]");
        }
        let verdict = verify_mandate_receipt(&receipt, None);
        assert!(!verdict.structurally_valid, "an edited event must fail");
        assert!(
            !verdict.hashes_ok,
            "record_hash must not recompute after an edit"
        );
    }

    #[test]
    fn mandate_receipt_detects_signature_forgery() {
        let (_dir, mut receipt) = emit_and_export_receipt();
        // Corrupt a signature: a valid base64 blob that is not the real signature.
        receipt.records[1].sig = BASE64.encode([0u8; 64]);
        let verdict = verify_mandate_receipt(&receipt, None);
        assert!(!verdict.structurally_valid);
        assert!(!verdict.signatures_ok, "a forged signature must not verify");
    }

    #[test]
    fn mandate_receipt_detects_dropped_record() {
        // Emit three records so we can drop the MIDDLE one and break contiguity.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");
        let log = AuditLog::with_file(&path).unwrap();
        for v in ["a", "b", "c"] {
            log.emit(AuditEvent::RuntimeStart {
                timestamp: SecureTimestamp::now(),
                version: v.to_string(),
            })
            .unwrap();
        }
        let mut three = log.export_mandate_receipt().unwrap();
        assert_eq!(three.records.len(), 3);
        three.records.remove(1); // drop the middle record
        let verdict = verify_mandate_receipt(&three, None);
        assert!(!verdict.structurally_valid);
        assert!(
            !verdict.chain_linkage_ok,
            "a dropped record must break linkage"
        );
    }

    #[test]
    fn mandate_receipt_rejects_a_wrong_signer_key() {
        let (_dir, mut receipt) = emit_and_export_receipt();
        // A different (valid) ed25519 key — the records were not signed under it.
        let other: [u8; 32] = [
            0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64,
            0x07, 0x3a, 0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68,
            0xf7, 0x07, 0x51, 0x1a,
        ];
        receipt.signer_public_key_hex = hex::encode(other);
        let verdict = verify_mandate_receipt(&receipt, None);
        assert!(!verdict.structurally_valid);
        assert!(
            !verdict.signatures_ok,
            "records must not verify under a foreign key"
        );
    }

    #[test]
    fn mandate_receipt_rejects_a_wrong_schema() {
        let (_dir, mut receipt) = emit_and_export_receipt();
        receipt.schema = "elastos.evil/v9".to_string();
        let verdict = verify_mandate_receipt(&receipt, None);
        assert!(!verdict.structurally_valid && !verdict.authenticated);
        assert!(
            verdict.error.is_some(),
            "wrong schema must fail closed with a reason"
        );
    }

    #[test]
    fn mandate_receipt_is_none_for_a_memory_only_log() {
        // A memory-only log has no durable, signed chain to hand out — no receipt.
        assert!(AuditLog::new().export_mandate_receipt().is_none());
        // And an empty receipt fails closed.
        let empty = MandateReceipt {
            schema: MANDATE_RECEIPT_SCHEMA.to_string(),
            scope: MandateReceiptScope::Contiguous,
            signer_public_key_hex: String::new(),
            records: vec![],
            set_binding: None,
        };
        let verdict = verify_mandate_receipt(&empty, None);
        assert!(!verdict.structurally_valid && !verdict.authenticated);
    }

    // Emit a mandate (grant) + two uses under it, INTERLEAVED with unrelated events, then export the
    // per-capability receipt. Returns (dir, receipt, signer_hex, token_id).
    fn emit_and_export_capability_receipt() -> (tempfile::TempDir, MandateReceipt, String, String) {
        use crate::capability::token::{Action, ResourceId, TokenId};
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::with_file(dir.path().join("audit.log")).unwrap();
        let token = TokenId::new();
        let token_id = token.to_string();
        let vendor = ResourceId::new("elastos://pay/vendor");
        // Unrelated noise before + between the mandate's records (interleaving is the whole point).
        log.emit(AuditEvent::RuntimeStart {
            timestamp: SecureTimestamp::now(),
            version: "noise".to_string(),
        })
        .unwrap();
        log.capability_grant(&token, "vm-agent", &vendor, Action::Write, None);
        log.emit(AuditEvent::RuntimeStart {
            timestamp: SecureTimestamp::now(),
            version: "noise-2".to_string(),
        })
        .unwrap();
        log.capability_use(&token, "vm-agent", &vendor, Action::Write, true);
        log.capability_use(&token, "vm-agent", &vendor, Action::Write, true);
        let signer = log.verifying_key_hex().unwrap();
        let receipt = log
            .export_mandate_receipt_for_capability(&token_id)
            .expect("per-capability receipt");
        (dir, receipt, signer, token_id)
    }

    #[test]
    fn per_capability_receipt_binds_the_mandate_and_its_actions_and_authenticates() {
        let (_dir, receipt, signer, _token) = emit_and_export_capability_receipt();
        // Exactly the grant + its two uses — the interleaved noise is excluded.
        assert_eq!(receipt.records.len(), 3, "grant + 2 uses, noise excluded");
        assert!(matches!(
            receipt.scope,
            MandateReceiptScope::Capability { .. }
        ));
        // Round-trips as a portable document and authenticates when pinned to the real signer.
        let wire = serde_json::to_string(&receipt).unwrap();
        let received: MandateReceipt = serde_json::from_str(&wire).unwrap();
        let verdict = verify_mandate_receipt(&received, Some(&signer));
        assert!(
            verdict.structurally_valid,
            "scoped receipt is sound: {verdict:?}"
        );
        assert!(
            verdict.scope_ok,
            "all records bound to the token + exactly one grant"
        );
        assert!(
            verdict.set_binding_ok,
            "issuer's set-binding signature verifies"
        );
        assert!(
            verdict.authenticated,
            "pinned to the true signer ⇒ authenticated"
        );
        // `starts_at_genesis` is N/A here: the grant sits mid-chain after noise, so it is false and
        // MUST NOT be read as a completeness/suspicion signal for a Capability receipt.
        assert!(
            !verdict.starts_at_genesis,
            "grant is mid-chain ⇒ genesis anchor N/A"
        );
    }

    #[test]
    fn per_capability_set_binding_detects_a_keyless_dropped_use() {
        // The completeness attack both reviewers flagged: a HOLDER (no signing key) trims an
        // inconvenient use from the JSON. Every remaining record still hashes, signs, and is bound to
        // the token (scope_ok stays true), so ONLY the issuer's set-binding signature can catch it.
        let (_dir, mut receipt, signer, _token) = emit_and_export_capability_receipt();
        // Drop one USE (keep the grant): the filtered scope rule is still satisfied by what remains.
        let victim = receipt
            .records
            .iter()
            .position(|r| !r.event.is_capability_grant())
            .expect("a use to drop");
        receipt.records.remove(victim);
        let verdict = verify_mandate_receipt(&receipt, Some(&signer));
        assert!(
            verdict.scope_ok,
            "the trimmed set still passes the filter rule — that is the trap"
        );
        assert!(
            !verdict.set_binding_ok,
            "the issuer's set binding no longer matches the trimmed set"
        );
        assert!(
            !verdict.structurally_valid,
            "a holder-trimmed set must not verify"
        );
        assert!(!verdict.authenticated, "and it must not authenticate");
    }

    #[test]
    fn per_capability_receipt_rejects_a_duplicated_record() {
        // Duplicate a signed use to inflate/misrepresent activity. Strict-seq breaks scope_ok and the
        // changed set breaks the binding — belt and suspenders.
        let (_dir, mut receipt, signer, _token) = emit_and_export_capability_receipt();
        let clone = receipt.records.last().unwrap().clone();
        receipt.records.push(clone);
        let verdict = verify_mandate_receipt(&receipt, Some(&signer));
        assert!(!verdict.scope_ok, "duplicate seq breaks strict ordering");
        assert!(
            !verdict.set_binding_ok,
            "a duplicated record changes the bound set"
        );
        assert!(!verdict.structurally_valid);
    }

    #[test]
    fn per_capability_receipt_rejects_a_smuggled_foreign_record() {
        let (_dir, mut receipt, signer, _token) = emit_and_export_capability_receipt();
        // Splice in a record from a DIFFERENT delegation. It is individually AUTHENTIC (really
        // signed), but it is NOT bound to this mandate's token_id → scope_ok must fail.
        let dir2 = tempfile::tempdir().unwrap();
        let log2 = AuditLog::with_file(dir2.path().join("other.log")).unwrap();
        use crate::capability::token::{Action, ResourceId, TokenId};
        log2.capability_use(
            &TokenId::new(),
            "vm-agent",
            &ResourceId::new("elastos://pay/vendor"),
            Action::Write,
            true,
        );
        // Re-sign it under THE SAME signer so signatures still verify (only the token differs).
        // (Simplest: pull a real foreign record from log2's file.)
        let other_line = std::fs::read_to_string(dir2.path().join("other.log")).unwrap();
        let foreign: ChainedRecord =
            serde_json::from_str(other_line.lines().next().unwrap()).unwrap();
        receipt.records.push(foreign);
        // Verify against the receipt's OWN signer (structural), not the cross-log signer — the point
        // is the SCOPE check, which must reject the foreign token regardless of signature origin.
        let verdict = verify_mandate_receipt(&receipt, Some(&signer));
        assert!(
            !verdict.scope_ok,
            "a foreign-token record must break scope_ok"
        );
        assert!(
            !verdict.structurally_valid,
            "scope failure ⇒ not structurally valid"
        );
        let _ = signer;
    }

    #[test]
    fn per_capability_receipt_requires_the_grant_to_be_present() {
        let (_dir, mut receipt, signer, _token) = emit_and_export_capability_receipt();
        // Drop the mandate itself (the grant), leaving only uses: a receipt of actions with no
        // authorization must not pass its scope rule (exactly one grant required).
        receipt.records.retain(|r| !r.event.is_capability_grant());
        assert!(!receipt.records.is_empty());
        let verdict = verify_mandate_receipt(&receipt, Some(&signer));
        assert!(
            !verdict.scope_ok,
            "no grant ⇒ scope_ok false (actions without a mandate)"
        );
    }

    #[test]
    fn emit_chains_seq_and_a_clean_log_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");
        let log = AuditLog::with_file(&path).unwrap();

        // emit returns Ok and the custody helper records who-opened-what-when.
        log.emit(AuditEvent::RuntimeStart {
            timestamp: SecureTimestamp::now(),
            version: "1.2.3".to_string(),
        })
        .expect("emit must succeed");
        log.content_open(
            "sess-1",
            "person:local:alice",
            "elastos://content/abc",
            "view",
            "opened",
            "chain-provider",
            None,
        )
        .expect("content_open must commit");
        log.runtime_stop();

        let vk = read_verifying_key(&log);
        assert_eq!(
            log.verify_chain(Some(&vk)).unwrap(),
            3,
            "three records chain + verify"
        );

        // Sequence numbers are contiguous from 1 and each prev_hash links to the prior record_hash.
        let content = std::fs::read_to_string(&path).unwrap();
        let recs: Vec<ChainedRecord> = content
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(recs[0].seq, 1);
        assert_eq!(recs[1].seq, 2);
        assert_eq!(recs[2].seq, 3);
        assert_eq!(
            recs[0].prev_hash,
            hex::encode([0u8; 32]),
            "genesis links to zeros"
        );
        assert_eq!(
            recs[1].prev_hash, recs[0].record_hash,
            "chain links seq2 → seq1"
        );
        assert_eq!(
            recs[2].prev_hash, recs[1].record_hash,
            "chain links seq3 → seq2"
        );
        assert!(
            recs.iter().all(|r| r.alg == AUDIT_SIG_ALG_ED25519),
            "records carry the agility tag"
        );
    }

    #[test]
    fn egress_denied_tags_serializes_and_chains_signed() {
        // W1b C0: the contained-egress custody event carries the canonical
        // capsule id (not the TAP), tags as "egress_denied", round-trips its
        // snake_case JSON, and chains+verifies on the same signed plane as the
        // spend/grant events it correlates with.
        let event = AuditEvent::EgressDenied {
            timestamp: SecureTimestamp::now(),
            capsule_id: "vm-act-emitter".to_string(),
            tap: "cv1a2b3c4d".to_string(),
            dest: "1.2.3.4:443".to_string(),
            proto: "tcp".to_string(),
            suppressed: 0,
        };
        assert_eq!(event.event_type_name(), "egress_denied");

        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "egress_denied", "serde tag is snake_case");
        assert_eq!(json["capsule_id"], "vm-act-emitter");
        assert_eq!(json["tap"], "cv1a2b3c4d");
        assert_eq!(json["dest"], "1.2.3.4:443");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");
        let log = AuditLog::with_file(&path).unwrap();
        log.emit(event).expect("egress_denied must emit durably");
        let vk = read_verifying_key(&log);
        assert_eq!(
            log.verify_chain(Some(&vk)).unwrap(),
            1,
            "the egress_denied record chains and verifies"
        );
    }

    #[test]
    fn content_open_grant_digest_is_optional_and_chain_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");
        let log = AuditLog::with_file(&path).unwrap();
        // One open WITHOUT a grant (media/legacy) and one WITH the forensic anchor.
        log.content_open("s1", "p:alice", "c/1", "view", "opened", "src", None)
            .unwrap();
        log.content_open(
            "s2",
            "p:bob",
            "c/2",
            "view",
            "opened",
            "src",
            Some("00112233445566778899aabbccddeeff"),
        )
        .unwrap();
        let vk = read_verifying_key(&log);
        assert_eq!(
            log.verify_chain(Some(&vk)).unwrap(),
            2,
            "both records verify"
        );

        let content = std::fs::read_to_string(&path).unwrap();
        let recs: Vec<&str> = content.lines().collect();
        // Backward-compat: a no-grant open omits the field entirely (prior records hash unchanged).
        assert!(
            !recs[0].contains("grant_digest"),
            "absent digest must be skipped: {}",
            recs[0]
        );
        // A grant-bearing open carries the non-reversible commitment.
        assert!(
            recs[1].contains("\"grant_digest\":\"00112233445566778899aabbccddeeff\""),
            "present digest must be recorded: {}",
            recs[1]
        );
    }

    #[test]
    fn flipping_a_byte_in_a_record_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");
        let log = AuditLog::with_file(&path).unwrap();
        log.content_open(
            "s",
            "person:local:alice",
            "elastos://c/1",
            "view",
            "opened",
            "src",
            None,
        )
        .unwrap();
        log.runtime_stop();
        let vk = read_verifying_key(&log);
        assert!(log.verify_chain(Some(&vk)).is_ok(), "baseline verifies");

        // Edit the event content in place (still valid JSON). The record_hash no longer matches.
        let content = std::fs::read_to_string(&path).unwrap();
        let tampered = content.replacen("person:local:alice", "person:local:mallory", 1);
        assert_ne!(content, tampered, "the edit must have applied");
        std::fs::write(&path, tampered).unwrap();

        let err = log.verify_chain(Some(&vk)).unwrap_err();
        assert!(
            err.contains("tamper"),
            "a content edit must be detected: {err}"
        );
    }

    /// Audit T4: an offline attacker with NO signing key rewrites the event
    /// history, recomputes every (public) record_hash so the chain still links,
    /// then STRIPS the signatures (alg="none", sig=""). Because alg/sig are not
    /// in the hash preimage, pre-T4 this forged chain verified clean under the
    /// real signer. After T4, a verifying key means every record MUST be
    /// ed25519-signed, so the downgrade is refused fail-closed.
    #[test]
    fn signature_downgrade_forgery_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");
        let log = AuditLog::with_file(&path).unwrap();
        log.content_open(
            "s",
            "person:local:alice",
            "elastos://c/1",
            "view",
            "opened",
            "src",
            None,
        )
        .unwrap();
        log.runtime_stop();
        let vk = read_verifying_key(&log);
        assert!(
            log.verify_chain(Some(&vk)).is_ok(),
            "baseline signed chain verifies"
        );

        // Forge: edit the custody record, recompute record_hash + relink, strip sig.
        let content = std::fs::read_to_string(&path).unwrap();
        let mut prev = genesis_prev_hash();
        let mut forged = Vec::new();
        for line in content.lines().filter(|l| !l.trim().is_empty()) {
            let mut rec: ChainedRecord = serde_json::from_str(line).unwrap();
            let event_json = serde_json::to_string(&rec.event).unwrap().replacen(
                "person:local:alice",
                "person:local:mallory",
                1,
            );
            rec.event = serde_json::from_str(&event_json).unwrap();
            let re_event = serde_json::to_string(&rec.event).unwrap();
            let h = compute_record_hash(rec.seq, &prev, re_event.as_bytes());
            rec.prev_hash = hex::encode(prev);
            rec.record_hash = hex::encode(h);
            rec.alg = "none".to_string();
            rec.sig = String::new();
            prev = h;
            forged.push(serde_json::to_string(&rec).unwrap());
        }
        std::fs::write(&path, format!("{}\n", forged.join("\n"))).unwrap();

        // The hash-chain itself is internally consistent (attacker recomputed it),
        // so ONLY the mandatory-signature rule catches the forgery.
        let err = log.verify_chain(Some(&vk)).unwrap_err();
        assert!(
            err.contains("downgrade") || err.contains("not ed25519-signed"),
            "an alg-downgrade forgery must be refused: {err}"
        );
    }

    #[test]
    fn dropping_a_record_breaks_the_chain() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");
        let log = AuditLog::with_file(&path).unwrap();
        log.runtime_start("1.0.0");
        log.content_open(
            "s",
            "person:local:alice",
            "elastos://c/1",
            "view",
            "opened",
            "src",
            None,
        )
        .unwrap();
        log.runtime_stop();
        let vk = read_verifying_key(&log);
        assert_eq!(log.verify_chain(Some(&vk)).unwrap(), 3);

        // Drop the middle record: the next record's seq + prev_hash no longer line up.
        let content = std::fs::read_to_string(&path).unwrap();
        let mut lines: Vec<&str> = content.lines().collect();
        lines.remove(1);
        std::fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();

        let err = log.verify_chain(Some(&vk)).unwrap_err();
        assert!(
            err.contains("chain break"),
            "a dropped record must be detected: {err}"
        );
    }

    #[test]
    fn a_record_resigned_with_the_wrong_key_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");
        let log = AuditLog::with_file(&path).unwrap();
        log.runtime_start("1.0.0");
        let vk = read_verifying_key(&log);

        // Attacker keeps the event + record_hash intact (so the hash check passes) but re-signs with
        // a key they control. Only the genuine verifying key can validate the signature → detected.
        let content = std::fs::read_to_string(&path).unwrap();
        let mut rec: ChainedRecord = serde_json::from_str(content.lines().next().unwrap()).unwrap();
        let attacker = SigningKey::generate(&mut rand::thread_rng());
        let hash_bytes = hex::decode(&rec.record_hash).unwrap();
        rec.sig = BASE64.encode(attacker.sign(&hash_bytes).to_bytes());
        std::fs::write(&path, format!("{}\n", serde_json::to_string(&rec).unwrap())).unwrap();

        let err = log.verify_chain(Some(&vk)).unwrap_err();
        assert!(
            err.contains("signature does not verify"),
            "a forged signature must be detected: {err}"
        );
    }

    #[test]
    fn the_chain_resumes_across_reopen_and_stays_append_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");
        {
            let log = AuditLog::with_file(&path).unwrap();
            log.runtime_start("1.0.0");
            log.runtime_stop();
        }
        // Reopen: the chain head must resume at seq 2, so the next record is seq 3 (no reset to 1).
        let log = AuditLog::with_file(&path).unwrap();
        log.runtime_start("1.0.1");
        let vk = read_verifying_key(&log);
        assert_eq!(
            log.verify_chain(Some(&vk)).unwrap(),
            3,
            "appends continue the prior chain"
        );
        let content = std::fs::read_to_string(&path).unwrap();
        let last: ChainedRecord = serde_json::from_str(content.lines().last().unwrap()).unwrap();
        assert_eq!(
            last.seq, 3,
            "reopened log appended at seq 3, never truncated"
        );
    }

    #[test]
    fn with_file_verified_resumes_clean_log_and_rejects_tamper() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");

        // Write a few signed records and fully flush by dropping the writer.
        {
            let log = AuditLog::with_file(&path).unwrap();
            log.runtime_start("1.0.0");
            log.content_open(
                "s",
                "person:local:alice",
                "elastos://c/1",
                "view",
                "opened",
                "src",
                None,
            )
            .unwrap();
            log.runtime_stop();
        }

        // Clean log: verify-on-open resumes without error and continues the chain (seq 4).
        let reopened = AuditLog::with_file_verified(&path).unwrap();
        reopened.runtime_start("1.0.1");
        let vk = read_verifying_key(&reopened);
        assert_eq!(
            reopened.verify_chain(Some(&vk)).unwrap(),
            4,
            "a clean log resumes and appends"
        );
        drop(reopened);

        // Tamper a record on disk, then re-open: verify-on-open must FAIL CLOSED rather than append
        // a fresh tail onto a laundered history.
        let content = std::fs::read_to_string(&path).unwrap();
        let tampered = content.replacen("person:local:alice", "person:local:mallory", 1);
        assert_ne!(content, tampered, "the edit must have applied");
        std::fs::write(&path, tampered).unwrap();

        let err = match AuditLog::with_file_verified(&path) {
            Ok(_) => panic!("a tampered log must refuse to open"),
            Err(e) => e,
        };
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("integrity verification"),
            "a tampered log must refuse to open: {err}"
        );
    }

    #[test]
    fn chain_attestation_reports_live_integrity_and_catches_tamper() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");
        let log = AuditLog::with_file(&path).unwrap();
        log.runtime_start("1.0.0");
        log.runtime_stop();

        // Memory-only logs have no durable chain to attest.
        assert!(
            AuditLog::new().chain_attestation().is_none(),
            "memory-only ⇒ no attestation, never a fabricated ok"
        );

        // A clean file-backed chain attests verified with its record count + signer.
        let att = log.chain_attestation().expect("file-backed ⇒ Some");
        assert!(att.verified, "clean chain verifies: {att:?}");
        assert_eq!(att.records, 2);
        assert!(att.signer.is_some(), "a signed chain names its key");
        assert!(att.error.is_none());

        // Tamper a record on disk; a LIVE attestation (not just startup) catches it.
        let content = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, content.replacen("1.0.0", "9.9.9", 1)).unwrap();
        let tampered = log.chain_attestation().expect("Some");
        assert!(!tampered.verified, "a content edit must fail the live walk");
        assert!(
            tampered.error.unwrap_or_default().contains("tamper"),
            "the attestation names the break"
        );
    }

    #[test]
    fn with_file_verified_detects_tail_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");

        // Three durable records → head-anchor commits seq 3.
        {
            let log = AuditLog::with_file(&path).unwrap();
            log.runtime_start("1.0.0");
            log.runtime_start("1.0.1");
            log.runtime_start("1.0.2");
        }
        assert_eq!(
            super::read_head_anchor(&path).unwrap(),
            Some(3),
            "anchor records the committed head"
        );

        // A clean log opens fine.
        drop(AuditLog::with_file_verified(&path).unwrap());

        // Slice the LAST record off the end. The remaining 2-record chain is itself valid — only the
        // head-anchor (still 3) reveals that a committed record was removed.
        let content = std::fs::read_to_string(&path).unwrap();
        let mut lines: Vec<&str> = content.lines().collect();
        lines.pop();
        std::fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();

        let err = match AuditLog::with_file_verified(&path) {
            Ok(_) => panic!("tail truncation must be detected"),
            Err(e) => e,
        };
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("tail-truncated"),
            "a sliced-off tail must be detected: {err}"
        );
    }

    /// GROUP-COMMIT RATCHET (S51): concurrent emitters on one file-backed log all succeed, and the
    /// resulting chain is PERFECT — contiguous seqs, every hash link and signature verifying end to
    /// end, and the head anchor at the full count. This is the whole safety claim of the group
    /// commit in one test: coalescing fsyncs must not reorder, drop, interleave, or half-commit a
    /// single record. (The THROUGHPUT claim is measured, not asserted: `benches/audit_emit.rs`.)
    #[test]
    fn concurrent_emits_group_commit_and_the_chain_stays_perfect() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");
        let log = std::sync::Arc::new(AuditLog::with_file(&path).unwrap());

        const THREADS: usize = 8;
        const PER_THREAD: usize = 25;
        let handles: Vec<_> = (0..THREADS)
            .map(|t| {
                let log = log.clone();
                std::thread::spawn(move || {
                    for i in 0..PER_THREAD {
                        log.emit(AuditEvent::RuntimeStart {
                            timestamp: SecureTimestamp::now(),
                            version: format!("t{t}-{i}"),
                        })
                        .expect("a concurrent emit must durably commit");
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        let total = (THREADS * PER_THREAD) as u64;
        let vk = log.signer.as_ref().map(|s| s.verifying_key());
        assert_eq!(
            log.verify_chain(vk.as_ref()).expect("chain verifies clean"),
            total,
            "every concurrent emit is on the chain exactly once, in order, signed"
        );
        assert_eq!(
            super::read_head_anchor(&path).unwrap(),
            Some(total),
            "the head anchor reached the full durable count"
        );
        // And the log re-opens fail-closed-verified — the on-disk artifact is coherent.
        drop(log);
        drop(AuditLog::with_file_verified(&path).unwrap());
    }

    /// TORN-TAIL RECOVERY RATCHET (S51 council fold — guardian F2 / red-team F1): a crash can tear
    /// the final (never-acknowledged — its fsync never completed, so its emit never returned Ok)
    /// line mid-writeback. The open must QUARANTINE that fragment and resume from the intact
    /// durable prefix — never refuse the healthy log (verified mode) and never append onto the
    /// fragment merging two records into garbage (plain mode). A corrupt line in the MIDDLE stays
    /// fail-closed (tamper — covered by `with_file_verified_resumes_clean_log_and_rejects_tamper`).
    #[test]
    fn a_torn_tail_is_quarantined_and_the_log_resumes_from_the_durable_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");
        {
            let log = AuditLog::with_file(&path).unwrap();
            log.emit(AuditEvent::RuntimeStart {
                timestamp: SecureTimestamp::now(),
                version: "a".to_string(),
            })
            .unwrap();
            log.emit(AuditEvent::RuntimeStart {
                timestamp: SecureTimestamp::now(),
                version: "b".to_string(),
            })
            .unwrap();
        }
        // Simulate the crash-torn tail: half a record, NO terminating newline.
        {
            use std::io::Write as _;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            f.write_all(br#"{"seq":3,"prev_hash":"dead"#).unwrap();
        }
        // The VERIFIED open (the production custody boot path) recovers instead of bricking...
        let log = AuditLog::with_file_verified(&path).expect(
            "a torn never-acknowledged tail must be quarantined, not refuse the healthy log",
        );
        // ...the fragment is preserved in the sidecar...
        let sidecar = std::fs::read_to_string(super::sibling(&path, "torn-tail")).unwrap();
        assert!(
            sidecar.contains(r#"{"seq":3,"prev_hash":"dead"#),
            "the torn bytes are quarantined, not destroyed: {sidecar}"
        );
        // ...and the log RESUMES: the next emit is seq 3 and the whole chain verifies.
        log.emit(AuditEvent::RuntimeStart {
            timestamp: SecureTimestamp::now(),
            version: "c".to_string(),
        })
        .unwrap();
        let vk = log.signer.as_ref().map(|s| s.verifying_key());
        assert_eq!(
            log.verify_chain(vk.as_ref()).expect("chain verifies clean"),
            3,
            "resumed exactly at the durable prefix; no gap, no duplicate"
        );
    }

    /// POISON-BEFORE-APPEND RATCHET (S51 council fold — guardian F1 / red-team F2): the
    /// authoritative poison check runs UNDER the chain lock, so a poisoned log refuses an emit
    /// BEFORE assigning a seq or writing a byte — an emit racing a failing one can never re-derive
    /// the failed seq and append behind its half-landed bytes.
    #[test]
    fn a_poisoned_log_refuses_before_assigning_a_seq_or_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");
        let log = AuditLog::with_file(&path).unwrap();
        log.emit(AuditEvent::RuntimeStart {
            timestamp: SecureTimestamp::now(),
            version: "a".to_string(),
        })
        .unwrap();
        let bytes_before = std::fs::metadata(&path).unwrap().len();

        log.poison("injected durability failure (test)".to_string());
        let res = log.emit(AuditEvent::RuntimeStart {
            timestamp: SecureTimestamp::now(),
            version: "b".to_string(),
        });
        assert!(
            matches!(&res, Err(e) if e.to_string().contains("poisoned")),
            "a poisoned log refuses: {res:?}"
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            bytes_before,
            "the refused emit wrote NOTHING — no seq derived, no bytes appended"
        );
        assert_eq!(
            log.chain.lock().unwrap().last_seq,
            1,
            "the chain head is untouched by the refused emit"
        );
    }

    /// POISON RATCHET (S51): after a failed durable write the log refuses EVERY subsequent emit
    /// (fail-closed) instead of retrying the seq — after a failure the on-disk suffix is unknown,
    /// so a retry could append a duplicate seq behind a half-landed record and corrupt the chain
    /// for verifiers. The first failure reports the IO error; later emits fail FAST naming the
    /// poison.
    #[test]
    fn a_failed_durable_write_poisons_the_log_fail_closed() {
        let path = std::env::temp_dir().join(format!("audit-poison-{}.log", std::process::id()));
        std::fs::File::create(&path).unwrap();
        let ro = std::fs::OpenOptions::new().read(true).open(&path).unwrap();
        let log = AuditLog::with_file_handle(ro);

        let first = log.emit(AuditEvent::RuntimeStart {
            timestamp: SecureTimestamp::now(),
            version: "a".to_string(),
        });
        assert!(first.is_err(), "read-only fd: the durable write must fail");

        let second = log.emit(AuditEvent::RuntimeStart {
            timestamp: SecureTimestamp::now(),
            version: "b".to_string(),
        });
        match second {
            Err(e) => assert!(
                e.to_string().contains("poisoned"),
                "the second emit refuses fast, naming the poison: {e}"
            ),
            Ok(()) => panic!("a poisoned log must never accept another record"),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_policy_proposal_event_serialization() {
        let event = AuditEvent::PolicyProposal {
            timestamp: SecureTimestamp::now(),
            request_id: "req-001".to_string(),
            recommended_outcome: "grant".to_string(),
            confidence: 0.9,
            rationale: "User granted before".to_string(),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"policy_proposal\""));
        assert!(json.contains("req-001"));
        assert!(json.contains("0.9"));

        let restored: AuditEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.event_type_name(), "policy_proposal");
    }

    #[test]
    fn test_policy_decision_made_event_serialization() {
        let event = AuditEvent::PolicyDecisionMade {
            timestamp: SecureTimestamp::now(),
            decision_id: "dec-001".to_string(),
            request_id: "req-001".to_string(),
            outcome: "grant".to_string(),
            checks_passed: 3,
            checks_failed: 0,
            shadow: false,
            rationale: "All checks passed".to_string(),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"policy_decision_made\""));
        assert!(json.contains("dec-001"));
        assert!(json.contains("\"shadow\":false"));

        let restored: AuditEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.event_type_name(), "policy_decision_made");
    }

    #[test]
    fn test_policy_event_type_names() {
        let proposal = AuditEvent::PolicyProposal {
            timestamp: SecureTimestamp::now(),
            request_id: "r".to_string(),
            recommended_outcome: "grant".to_string(),
            confidence: 0.5,
            rationale: "test".to_string(),
        };
        assert_eq!(proposal.event_type_name(), "policy_proposal");

        let decision = AuditEvent::PolicyDecisionMade {
            timestamp: SecureTimestamp::now(),
            decision_id: "d".to_string(),
            request_id: "r".to_string(),
            outcome: "deny".to_string(),
            checks_passed: 1,
            checks_failed: 2,
            shadow: true,
            rationale: "test".to_string(),
        };
        assert_eq!(decision.event_type_name(), "policy_decision_made");
    }

    #[test]
    fn test_policy_divergence_event_serialization() {
        let event = AuditEvent::PolicyDivergence {
            timestamp: SecureTimestamp::now(),
            request_id: "req-001".to_string(),
            real_decision_id: "dec-real".to_string(),
            shadow_decision_id: "dec-shadow".to_string(),
            real_outcome: "deny".to_string(),
            shadow_outcome: "grant".to_string(),
            real_rationale: "Denied by user".to_string(),
            shadow_rationale: "Auto-grant: all requests approved".to_string(),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"policy_divergence\""));
        assert!(json.contains("req-001"));
        assert!(json.contains("dec-real"));
        assert!(json.contains("dec-shadow"));

        let restored: AuditEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.event_type_name(), "policy_divergence");
    }

    #[test]
    fn test_policy_divergence_event_type_name() {
        let event = AuditEvent::PolicyDivergence {
            timestamp: SecureTimestamp::now(),
            request_id: "r".to_string(),
            real_decision_id: "d1".to_string(),
            shadow_decision_id: "d2".to_string(),
            real_outcome: "deny".to_string(),
            shadow_outcome: "grant".to_string(),
            real_rationale: "test".to_string(),
            shadow_rationale: "test".to_string(),
        };
        assert_eq!(event.event_type_name(), "policy_divergence");
    }

    /// Sprint 32: the responsible-entity binding rides the SIGNED CapabilityGrant record, so it is
    /// in the receipt cryptographically. The load-bearing property is BACK-COMPAT: a pre-S32 grant
    /// (no field) must re-serialize BYTE-IDENTICALLY — chain verification re-serializes deserialized
    /// events to recompute each hash, so any drift silently breaks every old chain.
    #[test]
    fn responsible_entity_is_present_when_set_and_omitted_for_back_compat() {
        // A pre-S32 record on disk: no `responsible_entity` key at all.
        let legacy = r#"{"type":"capability_grant","timestamp":{"unix_secs":100,"monotonic_seq":0},"token_id":"abcd","capsule_id":"vm-a","resource":"elastos://x/y","action":"execute","expiry":null}"#;
        let ev: AuditEvent = serde_json::from_str(legacy).unwrap();
        match &ev {
            AuditEvent::CapabilityGrant {
                responsible_entity, ..
            } => {
                assert!(responsible_entity.is_none(), "absent field ⇒ None");
            }
            _ => panic!("wrong variant"),
        }
        // Re-serialization MUST be byte-identical to the signed original (skip_serializing_if).
        assert_eq!(
            serde_json::to_string(&ev).unwrap(),
            legacy,
            "a pre-S32 grant must re-serialize with NO responsible_entity key — else its chain hash \
             changes and every old receipt fails to verify"
        );

        // An S32 grant carries the DID verbatim, and it round-trips.
        let bound = AuditEvent::CapabilityGrant {
            timestamp: SecureTimestamp {
                unix_secs: 100,
                monotonic_seq: 0,
            },
            token_id: "abcd".to_string(),
            capsule_id: "vm-a".to_string(),
            resource: "elastos://x/y".to_string(),
            action: "execute".to_string(),
            expiry: None,
            responsible_entity: Some("did:web:acme.example".to_string()),
        };
        let json = serde_json::to_string(&bound).unwrap();
        assert!(
            json.contains("\"responsible_entity\":\"did:web:acme.example\""),
            "a bound grant carries the liability DID in its signed record: {json}"
        );
        // Round-trip (AuditEvent has no PartialEq — compare the canonical re-serialization).
        let reparsed = serde_json::from_str::<AuditEvent>(&json).unwrap();
        assert_eq!(serde_json::to_string(&reparsed).unwrap(), json);
    }

    /// Council S32 F1 belt: a REAL signed chain that MIXES a grant without the field (the pre-S32
    /// byte shape) and a grant with it (the S32 shape) verifies end-to-end against the signer — and
    /// the field-less record's on-disk line carries NO responsible_entity key. This is the
    /// whole-chain regression the pinned-literal event test complements: a field REORDER or an
    /// omission-rule change breaks the recomputed hash and this fails.
    #[test]
    fn a_signed_chain_mixing_pre_and_post_s32_grants_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");
        let log = AuditLog::with_file(&path).unwrap();
        // A pre-S32-shaped grant: responsible_entity None ⇒ omitted from the signed bytes.
        log.emit(AuditEvent::CapabilityGrant {
            timestamp: SecureTimestamp::now(),
            token_id: "aaaa".to_string(),
            capsule_id: "vm-a".to_string(),
            resource: "elastos://x/y".to_string(),
            action: "execute".to_string(),
            expiry: None,
            responsible_entity: None,
        })
        .unwrap();
        // An S32 grant: the DID is in the signed bytes.
        log.emit(AuditEvent::CapabilityGrant {
            timestamp: SecureTimestamp::now(),
            token_id: "bbbb".to_string(),
            capsule_id: "vm-b".to_string(),
            resource: "elastos://x/z".to_string(),
            action: "execute".to_string(),
            expiry: None,
            responsible_entity: Some("did:web:acme.example".to_string()),
        })
        .unwrap();
        let vk = read_verifying_key(&log);
        assert!(
            log.verify_chain(Some(&vk)).is_ok(),
            "a chain mixing pre- and post-S32 grant shapes verifies end-to-end"
        );
        let content = std::fs::read_to_string(&path).unwrap();
        let aaaa = content.lines().find(|l| l.contains("\"aaaa\"")).unwrap();
        assert!(
            !aaaa.contains("responsible_entity"),
            "a None grant's SIGNED line omits the field (byte-shape identical to pre-S32): {aaaa}"
        );
        assert!(content
            .lines()
            .find(|l| l.contains("\"bbbb\""))
            .unwrap()
            .contains("\"responsible_entity\":\"did:web:acme.example\""));
    }

    #[test]
    fn rail_ref_is_present_when_set_and_omitted_for_back_compat() {
        // A pre-S34 CapabilityUse on disk: no `rail_ref` key at all.
        let legacy = r#"{"type":"capability_use","timestamp":{"unix_secs":100,"monotonic_seq":0},"token_id":"abcd","capsule_id":"vm-a","resource":"elastos://runtime/pay/acme","action":"execute","success":true}"#;
        let ev: AuditEvent = serde_json::from_str(legacy).unwrap();
        match &ev {
            AuditEvent::CapabilityUse { rail_ref, .. } => {
                assert!(rail_ref.is_none(), "absent field ⇒ None");
            }
            _ => panic!("wrong variant"),
        }
        // Re-serialization MUST be byte-identical (skip_serializing_if) — else every pre-S34 use
        // record's chain hash changes and old receipts fail to verify.
        assert_eq!(
            serde_json::to_string(&ev).unwrap(),
            legacy,
            "a pre-S34 use must re-serialize with NO rail_ref key"
        );

        // An S34 use carries the reference verbatim, and it round-trips.
        let settled = AuditEvent::CapabilityUse {
            timestamp: SecureTimestamp {
                unix_secs: 100,
                monotonic_seq: 0,
            },
            token_id: "abcd".to_string(),
            capsule_id: "vm-a".to_string(),
            resource: "elastos://runtime/pay/acme".to_string(),
            action: "execute".to_string(),
            success: true,
            rail_ref: Some("drm:tx=0xdead;op=0xop;tid=42".to_string()),
        };
        let json = serde_json::to_string(&settled).unwrap();
        assert!(
            json.contains("\"rail_ref\":\"drm:tx=0xdead;op=0xop;tid=42\""),
            "a settled use carries the rail reference in its signed record: {json}"
        );
        let reparsed = serde_json::from_str::<AuditEvent>(&json).unwrap();
        assert_eq!(serde_json::to_string(&reparsed).unwrap(), json);
    }

    /// Sprint 34 belt (mirrors the S32 mixed-chain fixture): a REAL signed chain mixing a
    /// pre-S34-shaped `CapabilityUse` (no rail_ref) and an S34 one (with it) verifies end-to-end,
    /// and the field-less record's on-disk line carries NO rail_ref key.
    #[test]
    fn a_signed_chain_mixing_pre_and_post_s34_uses_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");
        let log = AuditLog::with_file(&path).unwrap();
        // A pre-S34-shaped use: rail_ref None ⇒ omitted from the signed bytes.
        log.emit(AuditEvent::CapabilityUse {
            timestamp: SecureTimestamp::now(),
            token_id: "aaaa".to_string(),
            capsule_id: "vm-a".to_string(),
            resource: "elastos://runtime/audit-chain".to_string(),
            action: "read".to_string(),
            success: true,
            rail_ref: None,
        })
        .unwrap();
        // An S34 settled pay use: the tx reference is in the signed bytes.
        log.emit(AuditEvent::CapabilityUse {
            timestamp: SecureTimestamp::now(),
            token_id: "bbbb".to_string(),
            capsule_id: "vm-b".to_string(),
            resource: "elastos://runtime/pay/acme".to_string(),
            action: "execute".to_string(),
            success: true,
            rail_ref: Some("drm:tx=0xbeef;op=0xop;tid=7".to_string()),
        })
        .unwrap();
        let vk = read_verifying_key(&log);
        assert!(
            log.verify_chain(Some(&vk)).is_ok(),
            "a chain mixing pre- and post-S34 use shapes verifies end-to-end"
        );
        let content = std::fs::read_to_string(&path).unwrap();
        let aaaa = content.lines().find(|l| l.contains("\"aaaa\"")).unwrap();
        assert!(
            !aaaa.contains("rail_ref"),
            "a None use's SIGNED line omits the field (byte-shape identical to pre-S34): {aaaa}"
        );
        assert!(content
            .lines()
            .find(|l| l.contains("\"bbbb\""))
            .unwrap()
            .contains("\"rail_ref\":\"drm:tx=0xbeef;op=0xop;tid=7\""));
    }

    /// Sprint 49 (the SPEC-mandate-v1 conformance ratchet): the WIRE FORMAT the spec documents is
    /// pinned here — the schema tag, the three domain strings, every serialized key of the receipt
    /// document / chained record / scope tags, and the mandate-relevant event shapes (including
    /// the absent-when-unset rules old chains depend on). A change that breaks this test breaks
    /// the published spec: mint a v2, never edit v1.
    #[test]
    fn the_wire_format_matches_spec_mandate_v1() {
        // §2/§4 domain strings + the schema tag are part of the format.
        assert_eq!(AUDIT_RECORD_DOMAIN, b"elastos.runtime/audit-chain/v1");
        assert_eq!(
            MANDATE_RECEIPT_BINDING_DOMAIN,
            b"elastos.runtime/mandate-receipt-set/v1"
        );
        assert_eq!(MANDATE_RECEIPT_SCHEMA, "elastos.mandate_receipt/v1");

        // A real capability receipt, serialized: the §3 document keys, exactly.
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::with_file(dir.path().join("audit.log")).unwrap();
        let token = crate::capability::token::TokenId::new();
        let vendor = crate::capability::ResourceId::new("elastos://pay/vendor");
        log.capability_grant(
            &token,
            "vm-agent",
            &vendor,
            crate::capability::token::Action::Write,
            None,
        );
        log.capability_use_with_rail_ref(
            &token,
            "vm-agent",
            &vendor,
            crate::capability::token::Action::Write,
            true,
            Some("erc20:tx=0xA;to=0xb;amount=1;tok=t".to_string()),
        );
        let receipt = log
            .export_mandate_receipt_for_capability(&token.to_string())
            .expect("receipt");
        let doc = serde_json::to_value(&receipt).unwrap();
        let mut doc_keys: Vec<_> = doc.as_object().unwrap().keys().cloned().collect();
        doc_keys.sort();
        assert_eq!(
            doc_keys,
            [
                "records",
                "schema",
                "scope",
                "set_binding",
                "signer_public_key_hex"
            ],
            "the §3 receipt document keys are frozen"
        );
        // §3 scope tags.
        assert_eq!(
            serde_json::to_value(MandateReceiptScope::Contiguous).unwrap(),
            serde_json::json!({"kind": "contiguous"})
        );
        assert_eq!(
            doc["scope"]["kind"], "capability",
            "capability scope tag: {:?}",
            doc["scope"]
        );
        assert!(doc["scope"]["token_id"].is_string());

        // §2 chained-record keys, exactly.
        let rec = &doc["records"][0];
        let mut rec_keys: Vec<_> = rec.as_object().unwrap().keys().cloned().collect();
        rec_keys.sort();
        assert_eq!(
            rec_keys,
            ["alg", "event", "prev_hash", "record_hash", "seq", "sig"],
            "the §2 record keys are frozen"
        );
        assert_eq!(rec["alg"], "ed25519");
        assert_eq!(
            rec["prev_hash"].as_str().unwrap().len(),
            64,
            "prev_hash is 64 hex chars (genesis = all zeros)"
        );

        // §5 event shapes: INTERNALLY tagged (`"type"`), snake_case; absent-when-unset rules.
        let grant = &rec["event"];
        assert_eq!(grant["type"], "capability_grant");
        let mut grant_keys: Vec<_> = grant.as_object().unwrap().keys().cloned().collect();
        grant_keys.sort();
        assert_eq!(
            grant_keys,
            [
                "action",
                "capsule_id",
                "expiry",
                "resource",
                "timestamp",
                "token_id",
                "type"
            ],
            "capability_grant fields (responsible_entity ABSENT — not null — when unset)"
        );
        let use_ev = &doc["records"][1]["event"];
        assert_eq!(use_ev["type"], "capability_use");
        let mut use_keys: Vec<_> = use_ev.as_object().unwrap().keys().cloned().collect();
        use_keys.sort();
        assert_eq!(
            use_keys,
            [
                "action",
                "capsule_id",
                "rail_ref",
                "resource",
                "success",
                "timestamp",
                "token_id",
                "type"
            ],
            "capability_use fields (rail_ref present here because it was set)"
        );
        // Timestamp shape (the §7 preimage depends on it too).
        let ts = &grant["timestamp"];
        let mut ts_keys: Vec<_> = ts.as_object().unwrap().keys().cloned().collect();
        ts_keys.sort();
        assert_eq!(
            ts_keys,
            ["monotonic_seq", "unix_secs"],
            "SecureTimestamp JSON shape is frozen"
        );

        // §5 revoke shape (S49 guardian F1 — the field list the first draft got wrong: there is
        // NO capsule_id on a revoke) + BYTE-IDENTITY fixtures pinning field ORDER + compactness
        // (S49 guardian F3): the hash preimage is these exact bytes, so order is normative.
        let fixed_ts = elastos_common::SecureTimestamp {
            unix_secs: 1_700_000_000,
            monotonic_seq: 7,
        };
        let grant_ev = AuditEvent::CapabilityGrant {
            timestamp: fixed_ts,
            token_id: "tok1".into(),
            capsule_id: "vm-a".into(),
            resource: "elastos://pay/v".into(),
            action: "write".into(),
            expiry: None,
            responsible_entity: None,
        };
        assert_eq!(
            serde_json::to_string(&grant_ev).unwrap(),
            "{\"type\":\"capability_grant\",\"timestamp\":{\"unix_secs\":1700000000,\
             \"monotonic_seq\":7},\"token_id\":\"tok1\",\"capsule_id\":\"vm-a\",\
             \"resource\":\"elastos://pay/v\",\"action\":\"write\",\"expiry\":null}",
            "capability_grant byte template (§5) — type first, declared order, compact, \
             expiry:null when unset, responsible_entity ABSENT when unset"
        );
        let revoke_ev = AuditEvent::CapabilityRevoke {
            timestamp: fixed_ts,
            token_id: "tok1".into(),
            reason: "kill switch".into(),
        };
        assert_eq!(
            serde_json::to_string(&revoke_ev).unwrap(),
            "{\"type\":\"capability_revoke\",\"timestamp\":{\"unix_secs\":1700000000,\
             \"monotonic_seq\":7},\"token_id\":\"tok1\",\"reason\":\"kill switch\"}",
            "capability_revoke byte template (§5) — exactly type/timestamp/token_id/reason"
        );

        // And the exported receipt verifies — the spec's §6 algorithm against its own §2-§4 shapes.
        let signer = log.verifying_key_hex().unwrap();
        assert!(verify_mandate_receipt(&receipt, Some(&signer)).authenticated);
    }
}
