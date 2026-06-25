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

    /// Enable/disable echoing to stdout
    pub fn set_echo_stdout(&mut self, echo: bool) {
        self.echo_stdout = echo;
    }

    /// Test-only: wrap an already-open file handle as a file-backed log, so a
    /// test can force a durable-write failure (e.g. a read-only fd) and prove a
    /// fail-closed caller (`emit` + `?`) aborts instead of silently completing
    /// (G8a). Chain starts at genesis and is unsigned (`signer: None`) — this
    /// seam exercises the write/IO failure path, not signature semantics.
    #[cfg(test)]
    pub(crate) fn with_file_handle(file: File) -> Self {
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
            // Signature (when the record claims one and a verifying key was supplied).
            if rec.alg == AUDIT_SIG_ALG_ED25519 {
                let vk = verifying_key.ok_or_else(|| {
                    format!(
                        "seq {}: record is ed25519-signed but no verifying key was supplied",
                        rec.seq
                    )
                })?;
                let sig_bytes = BASE64
                    .decode(&rec.sig)
                    .map_err(|e| format!("seq {}: bad signature base64: {e}", rec.seq))?;
                let signature = Signature::from_slice(&sig_bytes)
                    .map_err(|e| format!("seq {}: malformed signature: {e}", rec.seq))?;
                vk.verify(&computed, &signature).map_err(|_| {
                    format!("audit tamper at seq {}: signature does not verify", rec.seq)
                })?;
            }
            expected_prev = computed;
            expected_seq += 1;
        }
        Ok(expected_seq - 1)
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
        self.emit_best_effort(AuditEvent::CapabilityUse {
            timestamp: SecureTimestamp::now(),
            token_id: token_id.to_string(),
            capsule_id: capsule_id.to_string(),
            resource: resource.to_string(),
            action: action.to_string(),
            success,
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
            AuditEvent::IdentityRegistered { .. } => "identity_registered",
            AuditEvent::StorageAccess { .. } => "storage_access",
            AuditEvent::MessageSent { .. } => "message_sent",
            AuditEvent::PolicyProposal { .. } => "policy_proposal",
            AuditEvent::PolicyDecisionMade { .. } => "policy_decision_made",
            AuditEvent::PolicyDivergence { .. } => "policy_divergence",
            AuditEvent::Custom { .. } => "custom",
        }
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
}
