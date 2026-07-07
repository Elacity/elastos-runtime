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
use std::sync::{Mutex, RwLock};

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
            format!("unexpected schema {:?} (want {MANDATE_RECEIPT_SCHEMA})", receipt.schema),
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
        let event_json = match serde_json::to_string(&record.event) {
            Ok(json) => json,
            Err(e) => {
                return MandateReceiptVerdict::failed(
                    signer,
                    format!("seq {}: re-serialize event: {e}", record.seq),
                )
            }
        };
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
        let computed = compute_record_hash(record.seq, &prev_hash, event_json.as_bytes());
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
            .map(|signature| vk.verify(&computed, &signature).is_ok())
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
            let strictly_ordered =
                receipt.records.windows(2).all(|pair| pair[1].seq > pair[0].seq);
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
                vk.verify(
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
    let signer_matches_expected = expected_signer_hex
        .map(|expected| expected.trim().eq_ignore_ascii_case(receipt.signer_public_key_hex.trim()));
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
    /// `seq` of the last DURABLY-committed record (0 = none yet; next record is `last_seq + 1`).
    last_seq: u64,
    /// `record_hash` of the last committed record (genesis = zeros). The next record's `prev_hash`.
    prev_hash: [u8; 32],
}

/// Audit event types
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
    /// Hash-chain head (seq + prev_hash), advanced only on a DURABLY-committed record.
    chain: Mutex<ChainState>,
    /// ed25519 signer for the chain. `None` ⇒ records carry `alg = "none"` (chain only, no
    /// non-repudiation). A persisted, dedicated key (NOT a fresh in-memory one) when file-backed.
    signer: Option<SigningKey>,
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
        }
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

        // Resume the chain head from any existing records (append-only continuity across restarts).
        let chain = resume_chain_state(&path);

        // Open APPEND-ONLY: never truncate or seek; the chain is the integrity, the OS append is the
        // ordering. (`append(true)` forces every write to EOF even under concurrent writers.)
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let writer = BufWriter::new(file);

        // Load-or-create the dedicated, persisted signing key alongside the log.
        let signer = load_or_create_signer(&path)?;

        Ok(Self {
            writer: Some(Mutex::new(writer)),
            log_path: Some(path),
            echo_stdout: false,
            memory_buffer: RwLock::new(VecDeque::with_capacity(MAX_MEMORY_EVENTS)),
            chain: Mutex::new(chain),
            signer: Some(signer),
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
    /// ignore the result; the loud log still fires. The hash-chain head only advances on a record
    /// that was durably written + `fsync`ed, so a failed write does not corrupt the chain.
    pub fn emit(&self, event: AuditEvent) -> Result<(), AuditError> {
        let event_json =
            serde_json::to_string(&event).map_err(|e| AuditError::Serialize(e.to_string()))?;

        // Take the chain lock for the whole compute→write→advance critical section so `seq`/
        // `prev_hash` cannot interleave across threads.
        let mut chain = self.chain.lock().map_err(|_| AuditError::Lock)?;
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

        // Durably commit BEFORE advancing the chain head. A failed write leaves `seq`/`prev_hash`
        // untouched, so the next emit retries the same seq — no gap, no silent loss.
        if let Some(writer) = &self.writer {
            let mut w = writer.lock().map_err(|_| AuditError::Lock)?;
            writeln!(w, "{}", line).map_err(|e| {
                tracing::error!("AUDIT durable-write failed (seq {seq}): {e}");
                AuditError::Io(e.to_string())
            })?;
            w.flush().map_err(|e| {
                tracing::error!("AUDIT flush failed (seq {seq}): {e}");
                AuditError::Io(e.to_string())
            })?;
            // fsync: the record must survive a crash/power loss to be a custody record at all.
            w.get_ref().sync_all().map_err(|e| {
                tracing::error!("AUDIT sync_all failed (seq {seq}): {e}");
                AuditError::Io(e.to_string())
            })?;
        }

        // Committed: advance the chain head.
        chain.last_seq = seq;
        chain.prev_hash = record_hash;

        // Persist the committed head seq for tail-truncation detection (file-backed only). Done under
        // the chain lock so the anchor advances monotonically with the head; best-effort — the record
        // is already durable and the log was written first, so a failed/lagging anchor never lies.
        if let Some(path) = &self.log_path {
            if let Err(e) = write_head_anchor(path, seq) {
                tracing::error!("AUDIT head-anchor write failed (seq {seq}): {e}");
            }
        }
        drop(chain);

        // Store in the in-memory ring buffer (best-effort; the durable record is the source of truth).
        if let Ok(mut buffer) = self.memory_buffer.write() {
            if buffer.len() >= MAX_MEMORY_EVENTS {
                buffer.pop_front();
            }
            buffer.push_back(event);
        }
        Ok(())
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
            let event_json = serde_json::to_string(&rec.event)
                .map_err(|e| format!("seq {}: re-serialize event: {e}", rec.seq))?;
            let computed = compute_record_hash(rec.seq, &expected_prev, event_json.as_bytes());
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
        Some(BASE64.encode(signer.sign(&mandate_receipt_binding_message(scope, records)).to_bytes()))
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
    let _ = std::fs::write(
        &pub_path,
        hex::encode(signing_key.verifying_key().to_bytes()),
    );
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
    std::fs::write(&tmp_path, committed_seq.to_string())?;
    std::fs::rename(&tmp_path, &anchor_path)
}

/// Read the committed chain-head sequence from `<log>.head-anchor`.
///
/// - `Ok(None)` — no anchor (a pre-anchor log or a brand-new file); the truncation check is skipped.
/// - `Ok(Some(seq))` — a well-formed anchor (the LOWER bound on how many records must be present).
/// - `Err` — the anchor exists but is unparseable; fail-CLOSED, since the atomic rename rules out a
///   torn write, so a corrupt anchor in durable mode is genuinely suspicious.
fn read_head_anchor(log_path: &Path) -> std::io::Result<Option<u64>> {
    let anchor_path = sibling(log_path, "head-anchor");
    match std::fs::read_to_string(&anchor_path) {
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
        assert!(structural.structurally_valid, "clean receipt is structurally valid: {structural:?}");
        assert!(!structural.authenticated, "no pin ⇒ not authenticated");
        assert_eq!(structural.signer_matches_expected, None);
        assert!(structural.hashes_ok && structural.signatures_ok && structural.chain_linkage_ok);
        assert_eq!(structural.records, 2);
        assert!(structural.starts_at_genesis, "the export begins at seq 1 / genesis");

        // Pinned to the real signer: AUTHENTIC (the bit an auditor acts on).
        let authentic = verify_mandate_receipt(&received, Some(&pin));
        assert!(authentic.authenticated, "pinned to the true signer ⇒ authenticated: {authentic:?}");
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
        assert!(!structural.authenticated, "self-signed is not authentic without a pin");

        // ...but pinning to the REAL runtime's signer exposes it: not the expected signer.
        let against_real = verify_mandate_receipt(&forged, Some(&real_signer));
        assert!(!against_real.authenticated, "forged receipt must fail against the pinned real signer");
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
        assert!(!verdict.hashes_ok, "record_hash must not recompute after an edit");
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
        assert!(!verdict.chain_linkage_ok, "a dropped record must break linkage");
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
        assert!(!verdict.signatures_ok, "records must not verify under a foreign key");
    }

    #[test]
    fn mandate_receipt_rejects_a_wrong_schema() {
        let (_dir, mut receipt) = emit_and_export_receipt();
        receipt.schema = "elastos.evil/v9".to_string();
        let verdict = verify_mandate_receipt(&receipt, None);
        assert!(!verdict.structurally_valid && !verdict.authenticated);
        assert!(verdict.error.is_some(), "wrong schema must fail closed with a reason");
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
        assert!(matches!(receipt.scope, MandateReceiptScope::Capability { .. }));
        // Round-trips as a portable document and authenticates when pinned to the real signer.
        let wire = serde_json::to_string(&receipt).unwrap();
        let received: MandateReceipt = serde_json::from_str(&wire).unwrap();
        let verdict = verify_mandate_receipt(&received, Some(&signer));
        assert!(verdict.structurally_valid, "scoped receipt is sound: {verdict:?}");
        assert!(verdict.scope_ok, "all records bound to the token + exactly one grant");
        assert!(verdict.set_binding_ok, "issuer's set-binding signature verifies");
        assert!(verdict.authenticated, "pinned to the true signer ⇒ authenticated");
        // `starts_at_genesis` is N/A here: the grant sits mid-chain after noise, so it is false and
        // MUST NOT be read as a completeness/suspicion signal for a Capability receipt.
        assert!(!verdict.starts_at_genesis, "grant is mid-chain ⇒ genesis anchor N/A");
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
        assert!(verdict.scope_ok, "the trimmed set still passes the filter rule — that is the trap");
        assert!(!verdict.set_binding_ok, "the issuer's set binding no longer matches the trimmed set");
        assert!(!verdict.structurally_valid, "a holder-trimmed set must not verify");
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
        assert!(!verdict.set_binding_ok, "a duplicated record changes the bound set");
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
        let foreign: ChainedRecord = serde_json::from_str(other_line.lines().next().unwrap()).unwrap();
        receipt.records.push(foreign);
        // Verify against the receipt's OWN signer (structural), not the cross-log signer — the point
        // is the SCOPE check, which must reject the foreign token regardless of signature origin.
        let verdict = verify_mandate_receipt(&receipt, Some(&signer));
        assert!(!verdict.scope_ok, "a foreign-token record must break scope_ok");
        assert!(!verdict.structurally_valid, "scope failure ⇒ not structurally valid");
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
        assert!(!verdict.scope_ok, "no grant ⇒ scope_ok false (actions without a mandate)");
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
            AuditEvent::CapabilityGrant { responsible_entity, .. } => {
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
            timestamp: SecureTimestamp { unix_secs: 100, monotonic_seq: 0 },
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
            timestamp: SecureTimestamp { unix_secs: 100, monotonic_seq: 0 },
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
}
