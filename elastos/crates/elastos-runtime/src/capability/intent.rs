//! Intent-proof loop — chunk 1: the two signed records + the pure verifier.
//!
//! The prover/verifier loop for agent *actions* (design: `docs/INTENT_PROOF_LOOP.md`).
//! An agent DECLARES what it intends to do ([`IntentDeclarationV1`]) before acting; the
//! runtime PROVES that intent is within a standing authorization
//! ([`check_intent_within_envelope`], fail-closed) before the act fires; and after the
//! act the declared-vs-done delta is recorded as a signed custody fact
//! ([`IntentReconciliationV1`] built from [`reconcile`]).
//!
//! The records sign/verify like [`crate::capability::receipt::AffordanceGrantReceiptV1`], and
//! the verifier is a pure function with the full fail-closed branch matrix under test. On top of
//! that this module also carries the STANDING-GRANT machinery: [`StandingGrantStore`] (the
//! fail-closed issue/revoke registry) and [`dispatch_standing_act`] (the net-new dispatch path
//! that runs a self-declared agent act through [`run_intent_gate`] against a standing grant, so an
//! agent can run unsupervised under the loop). This standing-grant dispatch is deliberately SEPARATE
//! from the live per-act carrier path, which already enforces via single-use consent.
//!
//! Scope boundary (carried from the design): this verifies CONTAINMENT + CUSTODY — that an
//! intent is within the authorized envelope, and a tamper-evident record of declared vs
//! done — NOT the correctness/wisdom of the act.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, RwLock};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::capability::receipt::AffordanceGrantReceiptV1;
use crate::capability::token::CapabilityToken;
use crate::primitives::audit::{AuditError, AuditEvent, AuditLog};
use crate::primitives::time::SecureTimestamp;

/// Schema tag for the v1 intent declaration.
pub const INTENT_DECLARATION_SCHEMA_V1: &str = "elastos.intent.declaration.v1";
/// Schema tag for the v1 intent reconciliation.
pub const INTENT_RECONCILIATION_SCHEMA_V1: &str = "elastos.intent.reconciliation.v1";

/// Domain-separation tags so an intent/reconciliation signature can never be confused
/// with any other ed25519 signature this key produces (mirrors `receipt.rs`).
const INTENT_SIG_DOMAIN: &[u8] = b"elastos.intent.declaration.v1\0";
const RECONCILE_SIG_DOMAIN: &[u8] = b"elastos.intent.reconciliation.v1\0";

// ─────────────────────────── The agent's pre-act proof obligation ────────────

/// A signed declaration of what an agent INTENDS to do, recorded before the act fires.
/// The agent's proof obligation: `input_hash` is the `canonical_input_hash` of the
/// declared arguments (same hashing path as the W2 binding), so the later receipt can be
/// compared field-for-field. Signed exactly like [`AffordanceGrantReceiptV1`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntentDeclarationV1 {
    pub schema: String,
    /// Stable id for this declaration (the reconciliation references it).
    pub intent_id: String,
    /// The acting capsule identity (`vm-{name}`).
    pub capsule: String,
    /// The affordance method the agent intends to invoke.
    pub method_id: String,
    /// Canonical hash of the declared invocation arguments.
    pub input_hash: String,
    /// The resource the act targets.
    pub resource: String,
    /// The action to be performed.
    pub action: String,
    /// The standing-grant envelope this intent is claimed to fall within.
    pub standing_grant_id: String,
    /// When the intent was declared.
    pub declared_at: SecureTimestamp,
    /// Issuer ed25519 public key (hex) that signed this declaration.
    pub signer: String,
    /// Ed25519 signature (base64) over the canonical declaration bytes.
    pub signature: String,
}

impl IntentDeclarationV1 {
    fn signable_digest(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(INTENT_SIG_DOMAIN);
        for field in [
            self.schema.as_str(),
            self.intent_id.as_str(),
            self.capsule.as_str(),
            self.method_id.as_str(),
            self.input_hash.as_str(),
            self.resource.as_str(),
            self.action.as_str(),
            self.standing_grant_id.as_str(),
            self.signer.as_str(),
        ] {
            hasher.update((field.len() as u64).to_le_bytes());
            hasher.update(field.as_bytes());
        }
        let ts = serde_json::to_vec(&self.declared_at).unwrap_or_default();
        hasher.update((ts.len() as u64).to_le_bytes());
        hasher.update(&ts);
        hasher.finalize().into()
    }

    /// Build and sign an intent declaration with the issuer signing key.
    #[allow(clippy::too_many_arguments)]
    pub fn issue(
        signing_key: &SigningKey,
        signer_pubkey: [u8; 32],
        intent_id: &str,
        capsule: &str,
        method_id: &str,
        input_hash: &str,
        resource: &str,
        action: &str,
        standing_grant_id: &str,
    ) -> Self {
        Self::issue_at(
            signing_key,
            signer_pubkey,
            intent_id,
            capsule,
            method_id,
            input_hash,
            resource,
            action,
            standing_grant_id,
            SecureTimestamp::now(),
        )
    }

    /// As [`issue`](Self::issue) but with an explicit `declared_at` — the signature covers it, so a
    /// stale/future-dated declaration is authentic AND caught by [`check_intent_freshness`] (rather
    /// than looking like a forgery). The freshness-window paths need this to be exercised.
    #[allow(clippy::too_many_arguments)]
    pub fn issue_at(
        signing_key: &SigningKey,
        signer_pubkey: [u8; 32],
        intent_id: &str,
        capsule: &str,
        method_id: &str,
        input_hash: &str,
        resource: &str,
        action: &str,
        standing_grant_id: &str,
        declared_at: SecureTimestamp,
    ) -> Self {
        let mut intent = Self {
            schema: INTENT_DECLARATION_SCHEMA_V1.to_string(),
            intent_id: intent_id.to_string(),
            capsule: capsule.to_string(),
            method_id: method_id.to_string(),
            input_hash: input_hash.to_string(),
            resource: resource.to_string(),
            action: action.to_string(),
            standing_grant_id: standing_grant_id.to_string(),
            declared_at,
            signer: hex::encode(signer_pubkey),
            signature: String::new(),
        };
        let signature: Signature = signing_key.sign(&intent.signable_digest());
        intent.signature = BASE64.encode(signature.to_bytes());
        intent
    }

    /// Verify the signature against a verifying key. Fails closed on any malformed signature.
    pub fn verify(&self, verifying_key: &VerifyingKey) -> bool {
        verify_b64_sig(&self.signature, &self.signable_digest(), verifying_key)
    }

    /// Verify the declaration against the signer key it NAMES (`self.signer`, hex-encoded). True iff
    /// the signature is valid for that key. This proves internal authenticity — the declaration was
    /// signed by the key it claims — which is what an untrusted transport (e.g. the HTTP boundary)
    /// must check before trusting an intent it did not construct. Fails closed on any malformed
    /// signer/signature. (Binding that key to a capsule's DID identity is a separate, later check.)
    pub fn verify_self(&self) -> bool {
        let Ok(bytes) = hex::decode(&self.signer) else {
            return false;
        };
        let Ok(arr): Result<[u8; 32], _> = bytes.try_into() else {
            return false;
        };
        let Ok(vk) = VerifyingKey::from_bytes(&arr) else {
            return false;
        };
        self.verify(&vk)
    }
}

// ─────────────────────────── The standing authorization envelope ─────────────

/// The fields of a standing grant the containment check needs (chunk-1 representation).
///
/// A standing grant authorizes a METHOD on a RESOURCE with an ACTION for a capsule — it
/// deliberately does NOT freeze an `input_hash`, so the agent may declare-and-act
/// repeatedly with different arguments within the envelope (the whole point of
/// unsupervised autonomy). Wiring this from a real issued `CapabilityToken` / grant is a
/// later chunk; here it is the value the pure check consumes.
/// Presence-required `Option` deserializer: the KEY must exist (serde's implicit
/// missing-`Option`-means-`None` is disabled by using `deserialize_with` without a default), and
/// an explicit `null` is an honest `None`. Guards the snapshot's two narrowing fields: a
/// hand-repaired file that DROPS `agent_pubkey` must not silently UNBIND an agent-bound mandate,
/// and one that drops `expires_at` must not immortalize it — the boot error invites the operator
/// to repair the file, so the repair path must be widen-proof, not just the happy path.
fn de_present_option<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d)
}

// `deny_unknown_fields` so a snapshot carrying fields this binary does not understand refuses to
// load (loud, fail-closed) instead of silently dropping semantics on a version rollback.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StandingGrantEnvelope {
    pub grant_id: String,
    pub capsule: String,
    /// The method ids the envelope authorizes (a sorted set for stable, fast containment).
    pub allowed_methods: BTreeSet<String>,
    pub resource: String,
    pub action: String,
    /// Expiry; `None` = never expires (until revoked), mirroring `CapabilityToken::expiry`.
    /// Presence-required on load: a snapshot missing the KEY is corrupt, never "never expires".
    #[serde(deserialize_with = "de_present_option")]
    pub expires_at: Option<SecureTimestamp>,
    pub revoked: bool,
    /// The AUTHORIZED AGENT's ed25519 verifying key (hex), if the grant bound one. When set, the
    /// gate requires the intent to be signed by THIS key — so the mandate authorizes a specific
    /// agent, and the audit attribution is the real actor, not "some self-signed key". `None` =
    /// capsule-string-only authorization (weaker attribution; see KNOWN_GAPS G-M4).
    /// Presence-required on load: a snapshot missing the KEY is corrupt, never an unbound mandate.
    #[serde(deserialize_with = "de_present_option")]
    pub agent_pubkey: Option<String>,
    /// The backing token's revocation epoch, captured at issue. A grant is dead once the runtime's
    /// current epoch passes it (key rotation / revoke-all advance the epoch), so the dispatcher can
    /// deny epoch-dead mandates without re-deriving the token.
    pub token_epoch: u64,
}

impl StandingGrantEnvelope {
    /// Active = not revoked AND not past expiry. Fail-closed: a revoked or expired
    /// envelope authorizes nothing. A `None` expiry never expires (until revoked).
    pub fn is_active(&self) -> bool {
        if self.revoked {
            return false;
        }
        match self.expires_at {
            Some(exp) => exp.is_future(),
            None => true,
        }
    }

    /// Derive a standing envelope from a real issued [`CapabilityToken`] (chunk 3): the
    /// token supplies the authority that is actually signed into it — `capsule`,
    /// `resource`, `action`, and `expiry`. The token does NOT enumerate affordance
    /// methods, nor does it carry revocation status (revocation is external state held by
    /// the `CapabilityManager`), so `allowed_methods` and `revoked` are supplied by the
    /// caller. This is the honest seam: the cryptographic grant gives the resource/action
    /// envelope; the method mapping + revocation check are layered on by the dispatcher.
    pub fn from_token(
        token: &CapabilityToken,
        allowed_methods: BTreeSet<String>,
        revoked: bool,
        agent_pubkey: Option<String>,
    ) -> Self {
        StandingGrantEnvelope {
            grant_id: token.id().to_string(),
            capsule: token.capsule().to_string(),
            allowed_methods,
            resource: token.resource().to_string(),
            action: token.action().to_string(),
            expires_at: token.expiry().copied(),
            revoked,
            agent_pubkey: agent_pubkey.map(|k| k.trim().to_lowercase()),
            token_epoch: token.constraints().epoch(),
        }
    }
}

/// Why an intent was denied against an envelope — one variant per fail-closed branch, so
/// the denial reason is explicit and testable (and can be recorded honestly).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeDenial {
    Revoked,
    Expired,
    WrongCapsule,
    /// The intent was signed by a key other than the mandate's bound agent key.
    WrongAgent,
    MethodNotInEnvelope,
    WrongResource,
    WrongAction,
    /// No standing grant was ever issued for the intent's `standing_grant_id`. Produced by the
    /// standing-grant dispatcher (never by `check_intent_within_envelope`, which needs an
    /// envelope to check): a fail-closed refusal when there is no authority to check against.
    NoGrant,
}

impl EnvelopeDenial {
    /// Stable snake_case reason string for the on-chain `IntentDenied` record.
    pub fn as_str(self) -> &'static str {
        match self {
            EnvelopeDenial::Revoked => "revoked",
            EnvelopeDenial::Expired => "expired",
            EnvelopeDenial::WrongCapsule => "wrong_capsule",
            EnvelopeDenial::WrongAgent => "wrong_agent",
            EnvelopeDenial::MethodNotInEnvelope => "method_not_in_envelope",
            EnvelopeDenial::WrongResource => "wrong_resource",
            EnvelopeDenial::WrongAction => "wrong_action",
            EnvelopeDenial::NoGrant => "no_standing_grant",
        }
    }
}

/// The verifier's verdict: the intent is within the envelope, or denied with a reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeCheck {
    Allowed,
    Denied(EnvelopeDenial),
}

/// Prove `intent ⊆ standing grant`, fail-closed. Envelope validity (revoked / expired) is
/// checked BEFORE field containment, so an inactive envelope authorizes nothing regardless
/// of the intent. `input_hash` is intentionally NOT checked — the envelope authorizes the
/// method/resource/action, not a frozen argument hash.
pub fn check_intent_within_envelope(
    intent: &IntentDeclarationV1,
    envelope: &StandingGrantEnvelope,
) -> EnvelopeCheck {
    if envelope.revoked {
        return EnvelopeCheck::Denied(EnvelopeDenial::Revoked);
    }
    if let Some(exp) = envelope.expires_at {
        if !exp.is_future() {
            return EnvelopeCheck::Denied(EnvelopeDenial::Expired);
        }
    }
    if intent.capsule != envelope.capsule {
        return EnvelopeCheck::Denied(EnvelopeDenial::WrongCapsule);
    }
    // Agent-key binding: if the mandate bound a specific agent key, ONLY that key may act under it.
    // `verify_self` already proved the declaration was signed by the key it names (`intent.signer`);
    // this proves that key is the AUTHORIZED agent, so the audit attribution is the real actor and
    // a different self-signed key cannot borrow the mandate. A mandate with no bound key (`None`)
    // stays capsule-string-only — weaker attribution, tracked in KNOWN_GAPS G-M4.
    if let Some(agent) = &envelope.agent_pubkey {
        if !intent.signer.trim().eq_ignore_ascii_case(agent) {
            return EnvelopeCheck::Denied(EnvelopeDenial::WrongAgent);
        }
    }
    if !envelope.allowed_methods.contains(&intent.method_id) {
        return EnvelopeCheck::Denied(EnvelopeDenial::MethodNotInEnvelope);
    }
    if intent.resource != envelope.resource {
        return EnvelopeCheck::Denied(EnvelopeDenial::WrongResource);
    }
    if intent.action != envelope.action {
        return EnvelopeCheck::Denied(EnvelopeDenial::WrongAction);
    }
    EnvelopeCheck::Allowed
}

// ─────────────────────────── Declared-vs-done reconciliation ──────────────────

/// The declared-vs-done verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationStatus {
    /// The receipt's bound fields equal the declared intent.
    Matched,
    /// A receipt exists but a bound field differs from the declared intent (the act fired
    /// within the envelope, but not as declared — flagged, never masked).
    Diverged,
    /// The intent was declared but no receipt was produced (the act never completed).
    /// Absence is recorded, never a silent pass.
    Undelivered,
}

impl ReconciliationStatus {
    /// Stable string form for the signature digest and the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            ReconciliationStatus::Matched => "matched",
            ReconciliationStatus::Diverged => "diverged",
            ReconciliationStatus::Undelivered => "undelivered",
        }
    }
}

/// Compare a declared intent against the receipt of what was actually redeemed. Returns the
/// status and, when not matched, a human-readable detail (the diverged field list, or the
/// undelivered reason). Pure — the signed [`IntentReconciliationV1`] is built from this.
pub fn reconcile(
    intent: &IntentDeclarationV1,
    receipt: Option<&AffordanceGrantReceiptV1>,
) -> (ReconciliationStatus, String) {
    let Some(r) = receipt else {
        return (
            ReconciliationStatus::Undelivered,
            "no receipt produced for the declared intent".to_string(),
        );
    };
    let mut diffs: Vec<&str> = Vec::new();
    if r.capsule != intent.capsule {
        diffs.push("capsule");
    }
    if r.method_id != intent.method_id {
        diffs.push("method_id");
    }
    if r.input_hash != intent.input_hash {
        diffs.push("input_hash");
    }
    if r.resource != intent.resource {
        diffs.push("resource");
    }
    if r.action != intent.action {
        diffs.push("action");
    }
    if diffs.is_empty() {
        (ReconciliationStatus::Matched, String::new())
    } else {
        (
            ReconciliationStatus::Diverged,
            format!("diverged fields: {}", diffs.join(",")),
        )
    }
}

/// A signed record of the declared-vs-done verdict. `receipt_id` is empty for an
/// `Undelivered` status; `divergence_detail` is empty for a `Matched` status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentReconciliationV1 {
    pub schema: String,
    /// The [`IntentDeclarationV1::intent_id`] this reconciles.
    pub intent_id: String,
    /// The reconciled receipt's `token_id` (empty when `Undelivered`).
    pub receipt_id: String,
    pub status: ReconciliationStatus,
    /// Human-readable detail (diverged fields / undelivered reason); empty when `Matched`.
    pub divergence_detail: String,
    pub reconciled_at: SecureTimestamp,
    pub signer: String,
    pub signature: String,
}

impl IntentReconciliationV1 {
    fn signable_digest(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(RECONCILE_SIG_DOMAIN);
        for field in [
            self.schema.as_str(),
            self.intent_id.as_str(),
            self.receipt_id.as_str(),
            self.status.as_str(),
            self.divergence_detail.as_str(),
            self.signer.as_str(),
        ] {
            hasher.update((field.len() as u64).to_le_bytes());
            hasher.update(field.as_bytes());
        }
        let ts = serde_json::to_vec(&self.reconciled_at).unwrap_or_default();
        hasher.update((ts.len() as u64).to_le_bytes());
        hasher.update(&ts);
        hasher.finalize().into()
    }

    /// Build and sign a reconciliation record from a [`reconcile`] result.
    pub fn issue(
        signing_key: &SigningKey,
        signer_pubkey: [u8; 32],
        intent_id: &str,
        receipt_id: &str,
        status: ReconciliationStatus,
        divergence_detail: &str,
    ) -> Self {
        let mut record = Self {
            schema: INTENT_RECONCILIATION_SCHEMA_V1.to_string(),
            intent_id: intent_id.to_string(),
            receipt_id: receipt_id.to_string(),
            status,
            divergence_detail: divergence_detail.to_string(),
            reconciled_at: SecureTimestamp::now(),
            signer: hex::encode(signer_pubkey),
            signature: String::new(),
        };
        let signature: Signature = signing_key.sign(&record.signable_digest());
        record.signature = BASE64.encode(signature.to_bytes());
        record
    }

    /// Verify the signature against a verifying key. Fails closed on any malformed signature.
    pub fn verify(&self, verifying_key: &VerifyingKey) -> bool {
        verify_b64_sig(&self.signature, &self.signable_digest(), verifying_key)
    }
}

/// Shared base64-signature verification (fail-closed on any malformed input).
fn verify_b64_sig(signature_b64: &str, digest: &[u8; 32], verifying_key: &VerifyingKey) -> bool {
    let Ok(sig_bytes) = BASE64.decode(signature_b64) else {
        return false;
    };
    let sig_arr: [u8; 64] = match sig_bytes.try_into() {
        Ok(arr) => arr,
        Err(_) => return false,
    };
    let signature = Signature::from_bytes(&sig_arr);
    // STRICT verification (council, Sprint 20 red-team F1): `verify` (non-strict) accepts low-order
    // / identity verifying keys, for which a forged signature validates for ANY message. A mandate
    // "bound" to such a key would be satisfiable by anyone — an effectively-UNBOUND mandate wearing
    // a "bound" badge. `verify_strict` rejects small-order keys + non-canonical signatures, closing
    // it at the gate everywhere (not just at mint), the security-correct default for authenticity.
    verifying_key.verify_strict(digest, &signature).is_ok()
}

// ─────────────────────────── On-chain custody events (chunk 2) ────────────────
// Turn the intent records + verdict into `AuditEvent`s for the signed hash chain, so the
// declaration, a denial, and the declared-vs-done verdict are tamper-evident custody —
// keyed on the canonical `vm-{name}` like the spend/grant/egress records. Emitting these
// onto a live `AuditLog` (and the wiring into dispatch) is the caller's job / a later
// chunk; these are the pure event builders.

/// Custody event for a declared intent (the agent's recorded proof obligation).
pub fn intent_declared_event(intent: &IntentDeclarationV1) -> AuditEvent {
    AuditEvent::IntentDeclared {
        timestamp: SecureTimestamp::now(),
        capsule_id: intent.capsule.clone(),
        intent_id: intent.intent_id.clone(),
        method_id: intent.method_id.clone(),
        resource: intent.resource.clone(),
        action: intent.action.clone(),
        standing_grant_id: intent.standing_grant_id.clone(),
    }
}

/// Custody event for an intent DENIED outside its envelope (`intent ⊄ envelope`),
/// carrying the fail-closed reason — a refused act leaves a signed trace.
pub fn intent_denied_event(intent: &IntentDeclarationV1, denial: EnvelopeDenial) -> AuditEvent {
    AuditEvent::IntentDenied {
        timestamp: SecureTimestamp::now(),
        capsule_id: intent.capsule.clone(),
        intent_id: intent.intent_id.clone(),
        method_id: intent.method_id.clone(),
        resource: intent.resource.clone(),
        action: intent.action.clone(),
        standing_grant_id: intent.standing_grant_id.clone(),
        reason: denial.as_str().to_string(),
    }
}

/// Custody event for the declared-vs-done verdict. `capsule_id` (the acting `vm-{name}`)
/// is supplied by the caller, since the reconciliation record correlates by `intent_id`.
pub fn intent_reconciled_event(
    reconciliation: &IntentReconciliationV1,
    capsule_id: &str,
) -> AuditEvent {
    AuditEvent::IntentReconciled {
        timestamp: SecureTimestamp::now(),
        capsule_id: capsule_id.to_string(),
        intent_id: reconciliation.intent_id.clone(),
        receipt_id: reconciliation.receipt_id.clone(),
        status: reconciliation.status.as_str().to_string(),
        divergence_detail: reconciliation.divergence_detail.clone(),
    }
}

// ─────────────────────────── Per-capsule intent-proof tally (chunk 5b) ────────

/// Per-capsule tally of intent-proof issues, for the custody projection. Mirrors the ESP
/// `IntentProofSummaryV1` shape. All-zero ⇒ clean; any non-zero ⇒ a flagged custody alarm.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IntentProofSummary {
    /// Intents denied because they fell outside their envelope (`intent ⊄ envelope`).
    pub denied: u64,
    /// Reconciliations whose act diverged from the declared intent.
    pub diverged: u64,
    /// Intents declared but never completed (no receipt).
    pub undelivered: u64,
}

impl IntentProofSummary {
    /// No issues recorded.
    pub fn is_clean(&self) -> bool {
        self.denied == 0 && self.diverged == 0 && self.undelivered == 0
    }

    /// Total flagged = denied + diverged + undelivered.
    pub fn flagged(&self) -> u64 {
        self.denied + self.diverged + self.undelivered
    }
}

/// Count intent-proof issues for one capsule from a stream of audit events, PRESENCE-aware.
///
/// Returns `None` when the capsule has NO intent activity at all in the stream — i.e. it
/// never went through the intent gate, so it is ABSENT (not "clean"): projecting it as
/// clean would be false reassurance. Returns `Some(summary)` when the capsule has any
/// intent event (`IntentDeclared` / `IntentDenied` / `IntentReconciled`); the counts are
/// the ISSUES — an `IntentDenied` ⇒ `denied`; an `IntentReconciled` whose status is
/// `diverged` / `undelivered` ⇒ the matching counter (a `matched` verdict / a bare
/// declaration is activity but NOT an issue). Events for other capsules are ignored. Pure.
pub fn count_intent_proof<'a>(
    events: impl IntoIterator<Item = &'a AuditEvent>,
    capsule_id: &str,
) -> Option<IntentProofSummary> {
    let mut s = IntentProofSummary::default();
    let mut seen = false;
    for ev in events {
        match ev {
            AuditEvent::IntentDeclared { capsule_id: c, .. } if c == capsule_id => seen = true,
            AuditEvent::IntentDenied { capsule_id: c, .. } if c == capsule_id => {
                seen = true;
                s.denied += 1;
            }
            AuditEvent::IntentReconciled {
                capsule_id: c,
                status,
                ..
            } if c == capsule_id => {
                seen = true;
                match status.as_str() {
                    "diverged" => s.diverged += 1,
                    "undelivered" => s.undelivered += 1,
                    _ => {}
                }
            }
            _ => {}
        }
    }
    seen.then_some(s)
}

// ─────────────────────────── The enforcement gate (chunk 4) ───────────────────
// The orchestrator that RUNS the loop: declare → verify → [abort | act → reconcile],
// fail-closed. This is the reusable enforcement unit a STANDING-GRANT dispatch mode calls
// to run an agent act unsupervised. It is deliberately NOT wired into the live per-act
// carrier path: that path already enforces via validate-and-consume (single-use consent),
// so re-checking an envelope derived from the very token that authorized the act would be
// redundant. The gate belongs to the standing-grant mode (net-new dispatch); see
// docs/INTENT_PROOF_LOOP.md.

/// The result of running the intent gate over one act.
#[derive(Debug)]
pub enum IntentGateOutcome {
    /// The declaration could not be recorded — the act was NOT run (custody is mandatory).
    BlockedNoCustody(AuditError),
    /// The intent fell outside its envelope — the act was NOT run (fail-closed).
    Denied(EnvelopeDenial),
    /// The act ran; here is the signed declared-vs-done reconciliation.
    Acted(IntentReconciliationV1),
}

/// Run the intent-proof loop over a single agent act, fail-closed. The ORDER is the
/// enforcement:
///   1. record the declaration FIRST — if custody fails, the act never runs
///      (`BlockedNoCustody`), because a custody chain is mandatory;
///   2. verify `intent ⊆ envelope` — a denial aborts BEFORE the act (`Denied`, with the
///      denial recorded on-chain);
///   3. run `act` ONLY past the fail-closed gate;
///   4. reconcile declared-vs-done and record the verdict (best-effort: the act already
///      happened, so this emit cannot un-run it, but it is loud on failure).
///
/// `act` returns the act's signed receipt, or `None` if it did not act (⇒ an `Undelivered`
/// reconciliation). The closure runs at most once and only in step 3.
pub fn run_intent_gate<F>(
    audit: &AuditLog,
    signing_key: &SigningKey,
    signer_pubkey: [u8; 32],
    intent: &IntentDeclarationV1,
    envelope: &StandingGrantEnvelope,
    act: F,
) -> IntentGateOutcome
where
    F: FnOnce() -> Option<AffordanceGrantReceiptV1>,
{
    // 1. Custody FIRST, fail-closed: no recorded declaration ⇒ no act.
    if let Err(e) = audit.emit(intent_declared_event(intent)) {
        return IntentGateOutcome::BlockedNoCustody(e);
    }
    // 2. Verify intent ⊆ envelope (fail-closed): a denial aborts before the act, recorded.
    if let EnvelopeCheck::Denied(reason) = check_intent_within_envelope(intent, envelope) {
        audit.emit_best_effort(intent_denied_event(intent, reason));
        return IntentGateOutcome::Denied(reason);
    }
    // 3. The act runs ONLY past the fail-closed gate.
    let receipt = act();
    // 4. Reconcile declared-vs-done and record the verdict.
    let (status, detail) = reconcile(intent, receipt.as_ref());
    let receipt_id = receipt
        .as_ref()
        .map(|r| r.token_id.clone())
        .unwrap_or_default();
    let rec = IntentReconciliationV1::issue(
        signing_key,
        signer_pubkey,
        &intent.intent_id,
        &receipt_id,
        status,
        &detail,
    );
    audit.emit_best_effort(intent_reconciled_event(&rec, &intent.capsule));
    IntentGateOutcome::Acted(rec)
}

// ─────────────────────────── The standing-grant registry (chunk 2c-1) ─────────
// The stateful home for the envelopes a standing-grant dispatcher runs against. The
// envelope's own doc names this: the cryptographic token gives resource/action, but
// REVOCATION and the method mapping are EXTERNAL state — held here, the way the
// CapabilityManager owns token revocation. Kept deliberately small and fail-closed so an
// agent can only ever run under a grant that was issued and is still live.

/// The oldest a dispatched intent's `declared_at` may be and still act — a captured signed
/// declaration EXPIRES after this, so it cannot be replayed indefinitely, and the replay guard
/// only has to remember intents this recent (bounding its size; G-M7). NOTE (clock trust): the
/// freshness window + compaction trust the host `SystemTime::now()` — the same custody class as the
/// same-disk snapshot caveat. A bad clock fails CLOSED (rejects, never over-admits): a backward
/// jump can spuriously reject valid intents, a forward jump compacts+rejects more, and neither
/// enables a double-act (the first act's id sits in the seen-set, which a rewind does not compact).
pub const MAX_INTENT_AGE_SECS: u64 = 3600; // 1 hour
/// How far in the FUTURE an intent's `declared_at` may be (clock skew) before it is refused — a
/// far-future date is either a badly-skewed clock or a forgery reaching for a longer replay life.
pub const MAX_CLOCK_SKEW_SECS: u64 = 300; // 5 minutes
/// The replay guard keeps a seen id until its `declared_at` ages past the whole acceptance window
/// (age + skew): while an intent could still pass the freshness check it MUST stay remembered, and
/// once it can no longer pass, forgetting it opens no replay (freshness rejects any re-POST). This
/// margin — RETENTION strictly greater than the max accepted age by exactly the skew term — is
/// load-bearing: it is why a compacted id is ALWAYS freshness-rejected on replay.
pub const SEEN_INTENT_RETENTION_SECS: u64 = MAX_INTENT_AGE_SECS + MAX_CLOCK_SKEW_SECS;

/// Why a dispatched intent failed the freshness window — the fail-closed reasons the dispatcher
/// records/returns before consulting the replay guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshnessError {
    /// `declared_at` is older than [`MAX_INTENT_AGE_SECS`] — the declaration has expired.
    Stale,
    /// `declared_at` is more than [`MAX_CLOCK_SKEW_SECS`] in the future.
    FutureDated,
}

impl FreshnessError {
    pub fn as_str(self) -> &'static str {
        match self {
            FreshnessError::Stale => "intent_declaration_expired",
            FreshnessError::FutureDated => "intent_declaration_future_dated",
        }
    }
}

/// Fail-closed freshness gate for a dispatched intent: `declared_at` must sit within
/// `[now - MAX_INTENT_AGE_SECS, now + MAX_CLOCK_SKEW_SECS]`. This bounds how long a captured
/// declaration can be replayed AND lets the replay guard forget anything older than the window
/// (G-M7). Pure over its two `u64` unix-second inputs, so it is trivially testable.
pub fn check_intent_freshness(declared_at_secs: u64, now_secs: u64) -> Result<(), FreshnessError> {
    if declared_at_secs > now_secs.saturating_add(MAX_CLOCK_SKEW_SECS) {
        return Err(FreshnessError::FutureDated);
    }
    if declared_at_secs < now_secs.saturating_sub(MAX_INTENT_AGE_SECS) {
        return Err(FreshnessError::Stale);
    }
    Ok(())
}

/// A fail-closed registry of [`StandingGrantEnvelope`]s for unsupervised agent dispatch,
/// keyed by `grant_id`. Every mutation is a single locked statement, so the map is never
/// observed half-updated. Fail-closed by construction:
///   - a grant that was never issued is ABSENT (`get` → `None`) — the dispatcher denies;
///   - [`revoke`](Self::revoke) flips the stored envelope's `revoked` flag and KEEPS the
///     record, so a revoked grant stays queryable as revoked (honest, recorded denial +
///     audit) rather than vanishing into an ambiguous "no such grant";
///   - a revoked or expired envelope authorizes NOTHING — enforced by the gate's own
///     [`check_intent_within_envelope`], not re-implemented here.
#[derive(Default)]
pub struct StandingGrantStore {
    grants: RwLock<HashMap<String, StandingGrantEnvelope>>,
    /// Intent ids already dispatched → their `declared_at` (unix secs) — the replay guard. A
    /// standing mandate is deliberately multi-use (the agent may act repeatedly with DIFFERENT
    /// intents), but the SAME signed declaration must act at most once, or a captured/retried blob
    /// is a double-act. With a persistent store the map survives restart (G-M5); it is COMPACTED
    /// against the freshness window on every write, so it is bounded, not monotonic (G-M7). The
    /// stored `declared_at` is what compaction ages against.
    seen_intents: RwLock<HashMap<String, u64>>,
    /// The highest `declared_at` ever COMPACTED out of `seen_intents` — a persisted anti-readmit
    /// watermark. Compaction forgets aged ids to stay bounded, which (red-team, Sprint 19) would
    /// otherwise let a captured intent be REPLAYED after a BACKWARD wall-clock step (evicted while
    /// the clock was high, then re-POSTed after the clock rewinds into its freshness window). An
    /// intent whose `declared_at <= max_evicted` was, or is shadowed by, an already-forgotten
    /// dispatch, so it is refused as a replay — clock-direction-independent, unlike the freshness
    /// window alone. Monotonic non-decreasing; guarded by the `seen_intents` write lock.
    max_evicted_declared_at: std::sync::atomic::AtomicU64,
    /// Per-mandate dispatch RATE budget (G-M7): `grant_id → (window_start_secs, count_in_window)`.
    /// Each mandate may perform at most [`MANDATE_DISPATCH_LIMIT`] acts per
    /// [`MANDATE_DISPATCH_WINDOW_SECS`], so a mandate-holding agent cannot flood dispatch (each act
    /// otherwise costs a durable snapshot write + fsync). In-memory: a restart refills the budget,
    /// which is the safe/generous direction (never a spurious denial), and the agent cannot restart
    /// the runtime. Bounded two ways: elapsed windows are pruned when the map grows, AND a HARD CAP
    /// (`DISPATCH_RATE_SOFT_CAP`) refuses new keys once reached — so even a same-window flood of
    /// distinct grant_ids cannot grow it without bound. In the dispatch path the handler also refuses
    /// unknown grant_ids before this is ever reached, so only real, operator-issued grant_ids land
    /// here at all.
    dispatch_rate: RwLock<HashMap<String, (u64, u32)>>,
    /// Snapshot file for a PERSISTENT registry (`None` = memory-only). Every mutation writes the
    /// full snapshot atomically (temp + fsync + rename, mirroring `CapabilityStore`) BEFORE the
    /// change becomes visible — on a write failure the mutation is rolled back and the error
    /// surfaces, so disk and memory can never diverge (no crash-revived mandate, no crash-forgotten
    /// revoke, no crash-forgotten replay guard).
    storage_path: Option<std::path::PathBuf>,
}

/// The most dispatch acts a single mandate may perform per [`MANDATE_DISPATCH_WINDOW_SECS`] — the
/// per-mandate rate budget (G-M7). Generous for a real agent (1/sec average, burstable), but bounds
/// a flood: an agent cannot make its mandate perform unbounded acts (each act is a durable write).
pub const MANDATE_DISPATCH_LIMIT: u32 = 60;
/// The rolling window (seconds) the per-mandate dispatch budget is measured over.
pub const MANDATE_DISPATCH_WINDOW_SECS: u64 = 60;
/// The dispatch-rate map's bound. When it exceeds this, elapsed windows are pruned; if it is still
/// at/over the cap a NEW key is then refused (hard cap) — so an attacker spamming DISTINCT (even
/// non-existent) grant_ids cannot grow it without bound, even within a single window.
const DISPATCH_RATE_SOFT_CAP: usize = 4096;

/// The on-disk snapshot of the standing-grant registry, version-pinned. Same-disk custody caveat
/// as the audit log's head-anchor: this defends against loss/corruption (strict parse, fail-closed
/// boot), not against a root attacker rewriting the file — that adversary already owns the runtime
/// key material on the same disk. Honest bound on "strict": the parse catches STRUCTURAL damage
/// (truncation, wrong version, unknown fields via `deny_unknown_fields`); a semantically-valid
/// same-disk edit that DROPS an `Option` field (e.g. deleting `agent_pubkey` to unbind an
/// agent-bound mandate — serde defaults a missing `Option` to `None`) is inside the same-disk
/// caveat, not caught here. Making well-formed edits detectable needs a keyed MAC (roadmap, same
/// custody class as the head-anchor co-signing).
/// One remembered intent id + the `declared_at` (unix secs) compaction ages it against.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SeenIntentRecord {
    id: String,
    declared_at: u64,
}

/// The current (v2) snapshot: the replay guard stores `declared_at` per id so the seen-set can be
/// compacted against the freshness window (bounded, not monotonic — G-M7).
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StandingGrantSnapshotV2 {
    version: u32,
    grants: Vec<StandingGrantEnvelope>,
    seen_intents: Vec<SeenIntentRecord>,
    /// The anti-readmit watermark (see [`StandingGrantStore::max_evicted_declared_at`]). `default`
    /// so a v2 file written before this field existed loads as 0 (no watermark yet — safe, the
    /// remembered ids still guard replay until they age, at which point the watermark takes over).
    #[serde(default)]
    max_evicted_declared_at: u64,
}

/// The prior (v1) snapshot: seen intents were bare ids with no timestamp. Read for a one-time
/// MIGRATION (never written): its ids are re-stamped `declared_at = load time` so they age out one
/// full window after upgrade — conservative (remembered longer, never a replay window).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StandingGrantSnapshotV1 {
    #[allow(dead_code)]
    version: u32,
    grants: Vec<StandingGrantEnvelope>,
    seen_intents: Vec<String>,
}

const STANDING_GRANT_SNAPSHOT_VERSION: u32 = 2;

impl StandingGrantStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open (or create) a PERSISTENT registry backed by a snapshot file — mandates and the replay
    /// guard survive restart. STRICT load, fail-closed at boot: a present-but-unparseable (or
    /// wrong-version) file is an error, never silently skipped — a skipped record could resurrect
    /// a revoked mandate or forget a dispatched intent (a replay window). A missing file is a
    /// clean first boot.
    pub fn with_persistence(path: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let store = Self {
            storage_path: Some(path.clone()),
            ..Self::default()
        };
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            let invalid = |e: String| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "standing-grant registry at {} is unreadable ({e}); refusing to boot over \
                         corrupt mandate state — repair or remove the file explicitly",
                        path.display()
                    ),
                )
            };
            // Probe the version first so a v1 file can be MIGRATED rather than fail-closed-refused
            // (refusing would drop every mandate + the replay guard on upgrade). Any other version
            // is still refused, and structural corruption in EITHER shape is refused.
            let version = serde_json::from_str::<serde_json::Value>(&content)
                .ok()
                .and_then(|v| v.get("version").and_then(|n| n.as_u64()))
                .ok_or_else(|| invalid("missing/invalid version".to_string()))?;
            #[allow(clippy::type_complexity)]
            let (grants_in, seen_in, max_evicted): (
                Vec<StandingGrantEnvelope>,
                Vec<(String, u64)>,
                u64,
            ) = match version {
                2 => {
                    let s: StandingGrantSnapshotV2 =
                        serde_json::from_str(&content).map_err(|e| invalid(e.to_string()))?;
                    let seen = s
                        .seen_intents
                        .into_iter()
                        .map(|r| (r.id, r.declared_at))
                        .collect();
                    (s.grants, seen, s.max_evicted_declared_at)
                }
                1 => {
                    // MIGRATION: re-stamp legacy bare ids with the load time so they age out one
                    // full window from now — never a replay window (a re-POST of an old intent is
                    // caught while remembered, and rejected by freshness/watermark once forgotten).
                    // No v1 watermark existed; 0 is safe (remembered ids guard until they age, then
                    // the watermark starts tracking their eviction).
                    let s: StandingGrantSnapshotV1 =
                        serde_json::from_str(&content).map_err(|e| invalid(e.to_string()))?;
                    let now = SecureTimestamp::now().unix_secs;
                    let seen = s.seen_intents.into_iter().map(|id| (id, now)).collect();
                    (s.grants, seen, 0)
                }
                other => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "standing-grant registry at {} has unsupported version {} \
                             (expected {})",
                            path.display(),
                            other,
                            STANDING_GRANT_SNAPSHOT_VERSION
                        ),
                    ));
                }
            };
            let mut grants = match store.grants.write() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            for env in grants_in {
                grants.insert(env.grant_id.clone(), env);
            }
            drop(grants);
            let mut seen = match store.seen_intents.write() {
                Ok(s) => s,
                Err(poisoned) => poisoned.into_inner(),
            };
            seen.extend(seen_in);
            store
                .max_evicted_declared_at
                .store(max_evicted, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(store)
    }

    /// Write the full snapshot atomically (temp + fsync + rename). Called with BOTH write guards
    /// held so the serialized state is exactly the state that becomes visible. Memory-only ⇒ no-op.
    fn persist_locked(
        &self,
        grants: &HashMap<String, StandingGrantEnvelope>,
        seen: &HashMap<String, u64>,
    ) -> std::io::Result<()> {
        let Some(path) = &self.storage_path else {
            return Ok(());
        };
        let mut grant_list: Vec<StandingGrantEnvelope> = grants.values().cloned().collect();
        grant_list.sort_by(|a, b| a.grant_id.cmp(&b.grant_id));
        let mut seen_list: Vec<SeenIntentRecord> = seen
            .iter()
            .map(|(id, declared_at)| SeenIntentRecord {
                id: id.clone(),
                declared_at: *declared_at,
            })
            .collect();
        seen_list.sort_by(|a, b| a.id.cmp(&b.id));
        let snapshot = StandingGrantSnapshotV2 {
            version: STANDING_GRANT_SNAPSHOT_VERSION,
            grants: grant_list,
            seen_intents: seen_list,
            max_evicted_declared_at: self
                .max_evicted_declared_at
                .load(std::sync::atomic::Ordering::SeqCst),
        };
        let content = serde_json::to_vec(&snapshot)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp_path = path.with_extension("tmp");
        {
            use std::io::Write as _;
            let mut tmp = std::fs::File::create(&tmp_path)?;
            tmp.write_all(&content)?;
            // Durable BEFORE visible: the rename must never publish bytes still in the page cache.
            tmp.sync_all()?;
        }
        std::fs::rename(&tmp_path, path)?;
        // DURABLE rename (red-team F1): without fsyncing the parent directory, a power cut after
        // the rename can revert the directory entry to the OLD snapshot — atomic but not yet
        // durable. For the replay guard that revert IS a replay window (a captured signed intent
        // acts twice), so the fsync is part of the write, not a nicety. If THIS fsync fails the
        // rename has already landed: the caller still rolls back memory and surfaces the error —
        // disk then holds the newer snapshot, which reconciles at the next successful mutation or
        // restart, and the disk-ahead direction never loses a revoke or a seen intent.
        #[cfg(unix)]
        if let Some(parent) = path.parent() {
            std::fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    }

    /// Issue (or replace) a standing grant, keyed by its `grant_id`, durable-before-visible.
    /// Issuing an envelope whose `revoked` flag is already set stores it as revoked (authorizes
    /// nothing) — issuing never silently un-revokes a grant. On a persistence failure the issuance
    /// is rolled back and the error surfaces: a mandate that cannot survive a restart is not
    /// issued at all (fail-closed, mirroring the manager's emit-before-mutate revoke).
    pub fn issue(&self, envelope: StandingGrantEnvelope) -> std::io::Result<()> {
        let mut grants = match self.grants.write() {
            Ok(g) => g,
            // A poisoned lock can only mean a prior panic; every write is one statement, so the
            // map is structurally intact — recover the guard rather than drop the issuance.
            Err(poisoned) => poisoned.into_inner(),
        };
        let seen = match self.seen_intents.write() {
            Ok(s) => s,
            Err(poisoned) => poisoned.into_inner(),
        };
        let grant_id = envelope.grant_id.clone();
        let previous = grants.insert(grant_id.clone(), envelope);
        if let Err(e) = self.persist_locked(&grants, &seen) {
            // Roll back: disk is the durable truth; memory must not run ahead of it.
            match previous {
                Some(prev) => grants.insert(grant_id, prev),
                None => grants.remove(&grant_id),
            };
            return Err(e);
        }
        Ok(())
    }

    /// Revoke a standing grant by id, fail-closed AND durable-before-visible. Returns `true` iff a
    /// live (not-already-revoked) grant was revoked by THIS call — a double-revoke or an unknown id
    /// returns `false`. The record is retained with `revoked = true` so the grant stays queryable
    /// as revoked. On a persistence failure the flag is rolled back and the error surfaces — a
    /// revoke that would crash-revive on restart does not report success.
    pub fn revoke(&self, grant_id: &str) -> std::io::Result<bool> {
        let mut grants = match self.grants.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let seen = match self.seen_intents.write() {
            Ok(s) => s,
            Err(poisoned) => poisoned.into_inner(),
        };
        match grants.get_mut(grant_id) {
            Some(env) if !env.revoked => {
                env.revoked = true;
            }
            _ => return Ok(false),
        }
        if let Err(e) = self.persist_locked(&grants, &seen) {
            if let Some(env) = grants.get_mut(grant_id) {
                env.revoked = false;
            }
            return Err(e);
        }
        Ok(true)
    }

    /// The standing grant for `grant_id`, if one was ever issued (revoked or not). The dispatcher
    /// runs it through the fail-closed gate, which denies a revoked/expired envelope — so returning
    /// a revoked envelope yields an honest, recorded `Revoked`/`Expired` denial, never a silent pass.
    /// A poisoned lock degrades to `None` (absent), never a fabricated grant.
    pub fn get(&self, grant_id: &str) -> Option<StandingGrantEnvelope> {
        let grants = self.grants.read().ok()?;
        grants.get(grant_id).cloned()
    }

    /// Every standing grant ever issued this runtime lifetime (revoked ones INCLUDED, flagged —
    /// an operator surface must show what was killed, not erase it), sorted by `grant_id` for a
    /// stable listing. Read-only. A poisoned lock recovers via `into_inner()` exactly like the
    /// writes do (every write is one statement, so the map is structurally intact): the operator
    /// surface must NEVER render a fabricated-clean "no mandates" over a map that has entries.
    pub fn list(&self) -> Vec<StandingGrantEnvelope> {
        let grants = match self.grants.read() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut all: Vec<StandingGrantEnvelope> = grants.values().cloned().collect();
        all.sort_by(|a, b| a.grant_id.cmp(&b.grant_id));
        all
    }

    /// Record a dispatch against the per-mandate RATE budget (G-M7): returns `true` iff this
    /// mandate is WITHIN [`MANDATE_DISPATCH_LIMIT`] acts for the current
    /// [`MANDATE_DISPATCH_WINDOW_SECS`] window (and counts this attempt), `false` if it is over
    /// budget. The dispatcher calls this AFTER authenticity + freshness (so only well-formed fresh
    /// intents count) and BEFORE the replay guard's durable write, so a flood is rejected before it
    /// costs an fsync. In-memory + bounded: a stale window resets on access, the map is pruned of
    /// elapsed windows when it grows past the soft cap, and a HARD CAP then refuses NEW keys while at
    /// capacity — so even a same-window flood of distinct (even fake) grant_ids cannot grow it
    /// without bound (an existing key is always still counted, so a real mandate is never denied by
    /// the cap). A poisoned lock recovers via `into_inner()` (fail-safe: a recovered map is
    /// structurally intact).
    pub fn record_dispatch_within_budget(&self, grant_id: &str, now_secs: u64) -> bool {
        let mut rate = match self.dispatch_rate.write() {
            Ok(r) => r,
            Err(poisoned) => poisoned.into_inner(),
        };
        // Bound memory in two steps. First drop windows that have FULLY ELAPSED (they would reset
        // on next access anyway) — this reclaims across-window entries.
        if rate.len() > DISPATCH_RATE_SOFT_CAP {
            rate.retain(|_, (window_start, _)| {
                now_secs < window_start.saturating_add(MANDATE_DISPATCH_WINDOW_SECS)
            });
            // Then a HARD CAP for the within-a-single-window case the prune above cannot help:
            // inside one window every entry is non-stale, so `retain` reclaims nothing and a
            // distinct-grant_id flood would otherwise grow the map without bound. Once we are STILL
            // at/over the cap after pruning, refuse a NEW (unseen) grant_id — return false, i.e.
            // over-budget → 429. An EXISTING grant_id keeps its entry and is counted normally, so a
            // real mandate is never denied by the cap; only never-before-seen keys are shed. The
            // map is thus hard-bounded at ~DISPATCH_RATE_SOFT_CAP. Fail-closed. (In the dispatch
            // path the handler's grant-existence check already ensures only real grant_ids arrive,
            // so this is belt-and-suspenders that also makes this method bounded when called alone.)
            if rate.len() >= DISPATCH_RATE_SOFT_CAP && !rate.contains_key(grant_id) {
                return false;
            }
        }
        let entry = rate.entry(grant_id.to_string()).or_insert((now_secs, 0));
        if now_secs >= entry.0.saturating_add(MANDATE_DISPATCH_WINDOW_SECS) {
            // A new window — reset and count this act.
            *entry = (now_secs, 1);
            return true;
        }
        if entry.1 < MANDATE_DISPATCH_LIMIT {
            entry.1 += 1;
            return true;
        }
        false
    }

    /// Whether ANY dispatch-rate entry exists. Test observability: proves a distinct-fake-grant_id
    /// flood created no rate entries (the handler's existence check ran before the budget).
    pub fn any_dispatch_rate_entries(&self) -> bool {
        match self.dispatch_rate.read() {
            Ok(r) => !r.is_empty(),
            Err(poisoned) => !poisoned.into_inner().is_empty(),
        }
    }

    /// Register an intent id as dispatched, returning `true` iff it was FRESH. The replay guard:
    /// the caller acts only on `Ok(true)`, so a re-POSTed signed declaration is refused. The caller
    /// MUST have passed [`check_intent_freshness`] first (this stores `declared_at` and COMPACTS the
    /// map against the freshness window on every call, so it stays bounded — G-M7 — but it does NOT
    /// itself reject a stale intent; that is the dispatcher's fail-closed gate). Durable-before-
    /// visible: on a persistence failure the id is rolled back and the error surfaces, and the
    /// caller must REFUSE the act (G-M5). A poisoned lock recovers via `into_inner()`.
    pub fn record_fresh_intent(
        &self,
        intent_id: &str,
        declared_at_secs: u64,
        now_secs: u64,
    ) -> std::io::Result<bool> {
        let grants = match self.grants.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut seen = match self.seen_intents.write() {
            Ok(s) => s,
            Err(poisoned) => poisoned.into_inner(),
        };
        use std::sync::atomic::Ordering::SeqCst;
        // Already seen ⇒ replay refused, BEFORE any compaction (never forget an id we are about to
        // reject on).
        if seen.contains_key(intent_id) {
            return Ok(false);
        }
        // Anti-readmit watermark (red-team, Sprint 19): an id whose declared_at is at/below the
        // highest already-EVICTED declared_at was, or is shadowed by, a forgotten dispatch — refuse
        // it as a replay. This holds regardless of clock direction, closing the backward-clock-step
        // readmission the freshness window alone cannot (freshness ages against a movable clock;
        // this watermark never regresses).
        let prev_watermark = self.max_evicted_declared_at.load(SeqCst);
        if declared_at_secs <= prev_watermark {
            return Ok(false);
        }
        // Compact: drop ids whose declared_at has aged past the whole acceptance window — a re-POST
        // of one would be rejected by check_intent_freshness (monotonic clock) AND by the watermark
        // (any clock), so forgetting opens no replay. This bounds the map to ~one window of intents
        // (no longer monotonic). Compaction runs only here (the write path), so an idle store is not
        // pruned — strictly SAFER (idle ⇒ remembered longer); boundedness holds under any traffic
        // that reaches the cap. Every eviction raises the watermark to the evicted declared_at.
        let cutoff = now_secs.saturating_sub(SEEN_INTENT_RETENTION_SECS);
        let mut new_watermark = prev_watermark;
        seen.retain(|_, declared_at| {
            if *declared_at < cutoff {
                new_watermark = new_watermark.max(*declared_at);
                false
            } else {
                true
            }
        });
        if new_watermark > prev_watermark {
            self.max_evicted_declared_at.store(new_watermark, SeqCst);
        }
        seen.insert(intent_id.to_string(), declared_at_secs);
        if let Err(e) = self.persist_locked(&grants, &seen) {
            // Roll back the INSERT (the just-added id must not survive a failed persist), but KEEP
            // the bumped watermark: compaction already forgot the evicted ids from memory, and an
            // evicted id can have declared_at ABOVE prev_watermark — restoring the LOWER prev would
            // leave it caught by neither the set (forgotten) nor the watermark (too low), reopening
            // the backward-clock replay under a persist failure. The watermark is monotonic and
            // fail-closed (higher ⇒ rejects MORE); disk still holds the old (lower) watermark + the
            // un-evicted entries, so a restart is also safe. (Re-verification of the F1 fix.)
            seen.remove(intent_id);
            return Err(e);
        }
        Ok(true)
    }

    /// Revoke EVERY standing grant — the envelope side of the mass kill switch, durable-before-
    /// visible. Called alongside an epoch advance (`revoke_all`), which kills every backing token
    /// but knows nothing of the envelope registry; without this, an epoch-dead mandate would keep
    /// rendering (and, once dispatch is wired, dispatching) as LIVE. Returns how many live
    /// envelopes this call killed; on a persistence failure every flag is rolled back and the
    /// error surfaces (all-or-nothing — a partially-persisted mass kill is worse than a loud
    /// failure, because it looks complete).
    pub fn revoke_all(&self) -> std::io::Result<usize> {
        let mut grants = match self.grants.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let seen = match self.seen_intents.write() {
            Ok(s) => s,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut killed_ids = Vec::new();
        for env in grants.values_mut() {
            if !env.revoked {
                env.revoked = true;
                killed_ids.push(env.grant_id.clone());
            }
        }
        if killed_ids.is_empty() {
            return Ok(0);
        }
        if let Err(e) = self.persist_locked(&grants, &seen) {
            for id in &killed_ids {
                if let Some(env) = grants.get_mut(id) {
                    env.revoked = false;
                }
            }
            return Err(e);
        }
        Ok(killed_ids.len())
    }

    /// True iff an ACTIVE (issued, not revoked, not expired) grant exists for `grant_id`. A read-only
    /// probe for surfaces that want the live count; the dispatch decision itself always goes through
    /// the gate, never this shortcut.
    pub fn is_active(&self, grant_id: &str) -> bool {
        match self.grants.read() {
            Ok(g) => g
                .get(grant_id)
                .map(StandingGrantEnvelope::is_active)
                .unwrap_or(false),
            Err(_) => false,
        }
    }
}

// ─────────────────────────── The standing-grant dispatcher (chunk 2c-2) ───────
// The entry point that RUNS an agent unsupervised under the loop: given a self-declared
// intent, resolve the standing grant it claims, and drive it through the fail-closed
// `run_intent_gate`. This is the net-new dispatch path the design reserved for standing
// grants — it is NOT the live per-act carrier path (that already enforces via single-use
// consent), so nothing here re-checks a token that already authorized an act.

/// Dispatch a self-declared agent act against the standing-grant registry, fail-closed.
///
/// Resolves the intent's `standing_grant_id` in `store` and drives it through
/// [`run_intent_gate`] (declare → verify `intent ⊆ envelope` → act → reconcile). A grant that
/// was never issued is a fail-closed refusal: the declaration is still recorded (custody is
/// mandatory), a `NoGrant` denial is recorded on-chain, and the `act` closure is NEVER invoked —
/// so a missing authority looks identical, on the intent channel, to any other denial. The return
/// is a uniform [`IntentGateOutcome`]: `Denied(NoGrant)` when unauthorized, otherwise exactly what
/// the gate decided (`BlockedNoCustody` / `Denied` / `Acted`).
pub fn dispatch_standing_act<F>(
    store: &StandingGrantStore,
    audit: &AuditLog,
    signing_key: &SigningKey,
    signer_pubkey: [u8; 32],
    intent: &IntentDeclarationV1,
    act: F,
) -> IntentGateOutcome
where
    F: FnOnce() -> Option<AffordanceGrantReceiptV1>,
{
    match store.get(&intent.standing_grant_id) {
        // A grant exists (active or not) — the gate makes the fail-closed decision, denying a
        // revoked/expired/out-of-envelope intent with its true reason and recording it.
        Some(envelope) => {
            run_intent_gate(audit, signing_key, signer_pubkey, intent, &envelope, act)
        }
        // No standing authority to check against. Record custody FIRST (mandatory — no recorded
        // declaration ⇒ no act), then the NoGrant denial, in the same order `run_intent_gate`
        // uses, so the refusal is on-chain and shows on the intent channel. `act` never runs.
        None => {
            if let Err(e) = audit.emit(intent_declared_event(intent)) {
                return IntentGateOutcome::BlockedNoCustody(e);
            }
            audit.emit_best_effort(intent_denied_event(intent, EnvelopeDenial::NoGrant));
            IntentGateOutcome::Denied(EnvelopeDenial::NoGrant)
        }
    }
}

// ─────────────────────────── The standing-grant service (chunk 2c-gw-A) ───────
// One object that bundles the fail-closed registry with the audit log and issuer key the
// dispatcher needs — the seam a caller (e.g. a shell-only gateway handler) holds in state to
// issue/revoke standing grants and run agent acts under them. It only WIRES the pieces
// together; it adds no new authority and no bypass — every decision still runs through the
// fail-closed gate.

/// A ready-to-use standing-grant service over a shared [`AuditLog`] and an issuer signing key.
/// The natural unit of state for a standing-grant API: `issue_from_token` to grant, `revoke` to
/// kill, `dispatch` to run one agent act under the loop. Cloneable-friendly (wrap in `Arc` to
/// share); the inner [`StandingGrantStore`] is itself interior-mutable and thread-safe.
pub struct StandingGrantService {
    store: StandingGrantStore,
    audit: Arc<AuditLog>,
    signing_key: SigningKey,
    signer_pubkey: [u8; 32],
}

impl StandingGrantService {
    /// Build a service over a shared audit log and the issuer signing key (its public half is
    /// derived once, for the reconciliation signatures the dispatcher writes).
    pub fn new(audit: Arc<AuditLog>, signing_key: SigningKey) -> Self {
        let signer_pubkey = signing_key.verifying_key().to_bytes();
        Self {
            store: StandingGrantStore::new(),
            audit,
            signing_key,
            signer_pubkey,
        }
    }

    /// Like [`new`](Self::new) but over a PERSISTENT registry (see
    /// [`StandingGrantStore::with_persistence`]): mandates and the replay guard survive restart.
    /// Fail-closed at boot — corrupt on-disk state is an error, never silently skipped.
    pub fn with_persistence(
        audit: Arc<AuditLog>,
        signing_key: SigningKey,
        path: impl AsRef<std::path::Path>,
    ) -> std::io::Result<Self> {
        let signer_pubkey = signing_key.verifying_key().to_bytes();
        Ok(Self {
            store: StandingGrantStore::with_persistence(path)?,
            audit,
            signing_key,
            signer_pubkey,
        })
    }

    /// Issue a standing grant derived from a REAL issued [`CapabilityToken`] (the cryptographic
    /// root): the token supplies capsule/resource/action/expiry, the caller supplies the authorized
    /// method set. Returns the `grant_id` (the token's id) to revoke or dispatch against later.
    /// With a persistent registry, a mandate that cannot be durably recorded is NOT issued
    /// (fail-closed) — the error surfaces instead.
    pub fn issue_from_token(
        &self,
        token: &CapabilityToken,
        allowed_methods: BTreeSet<String>,
        agent_pubkey: Option<String>,
    ) -> std::io::Result<String> {
        let envelope =
            StandingGrantEnvelope::from_token(token, allowed_methods, false, agent_pubkey);
        let grant_id = envelope.grant_id.clone();
        self.store.issue(envelope)?;
        Ok(grant_id)
    }

    /// The standing grant envelope for `grant_id` (revoked or not), if ever issued. Read-only;
    /// an operator/dispatch surface uses it to consult liveness the pure gate cannot re-derive.
    pub fn get(&self, grant_id: &str) -> Option<StandingGrantEnvelope> {
        self.store.get(grant_id)
    }

    /// Record a dispatch against the mandate's per-mandate RATE budget; `true` iff within budget.
    /// See [`StandingGrantStore::record_dispatch_within_budget`]. `false` ⇒ the caller must REFUSE
    /// the act (429). Call AFTER freshness, BEFORE the replay guard's durable write.
    pub fn record_dispatch_within_budget(&self, grant_id: &str, now_secs: u64) -> bool {
        self.store.record_dispatch_within_budget(grant_id, now_secs)
    }

    /// Whether any dispatch-rate entry exists (test observability). See
    /// [`StandingGrantStore::any_dispatch_rate_entries`].
    pub fn any_dispatch_rate_entries(&self) -> bool {
        self.store.any_dispatch_rate_entries()
    }

    /// Register an intent id as dispatched; `Ok(true)` iff FRESH. The replay guard — see
    /// [`StandingGrantStore::record_fresh_intent`]. The caller must have passed
    /// [`check_intent_freshness`] first. On `Err` the caller must REFUSE the act.
    pub fn record_fresh_intent(
        &self,
        intent_id: &str,
        declared_at_secs: u64,
        now_secs: u64,
    ) -> std::io::Result<bool> {
        self.store
            .record_fresh_intent(intent_id, declared_at_secs, now_secs)
    }

    /// Revoke a standing grant by id, fail-closed and durable-before-visible. `Ok(true)` iff a
    /// live grant was revoked by this call (double-revoke / unknown id → `Ok(false)`); a
    /// persistence failure rolls back and surfaces.
    pub fn revoke(&self, grant_id: &str) -> std::io::Result<bool> {
        self.store.revoke(grant_id)
    }

    /// True iff an ACTIVE (issued, not revoked, not expired) grant exists for `grant_id`.
    pub fn is_active(&self, grant_id: &str) -> bool {
        self.store.is_active(grant_id)
    }

    /// Every standing grant issued this runtime lifetime (revoked included, flagged), sorted by
    /// id — the operator's mandate list. Read-only; see [`StandingGrantStore::list`].
    pub fn list(&self) -> Vec<StandingGrantEnvelope> {
        self.store.list()
    }

    /// Revoke EVERY standing grant (the envelope side of the mass kill switch — pair with the
    /// manager's epoch advance). Returns how many live envelopes were killed by this call;
    /// all-or-nothing under a persistent registry.
    pub fn revoke_all(&self) -> std::io::Result<usize> {
        self.store.revoke_all()
    }

    /// Dispatch a self-declared agent act under its standing grant, fail-closed — the full loop
    /// (declare → verify `intent ⊆ envelope` → act → reconcile). Thin wrapper over
    /// [`dispatch_standing_act`] with the service's own store/audit/key.
    pub fn dispatch<F>(&self, intent: &IntentDeclarationV1, act: F) -> IntentGateOutcome
    where
        F: FnOnce() -> Option<AffordanceGrantReceiptV1>,
    {
        dispatch_standing_act(
            &self.store,
            self.audit.as_ref(),
            &self.signing_key,
            self.signer_pubkey,
            intent,
            act,
        )
    }

    /// Preview whether an intent WOULD be allowed under its standing grant — pure containment, the
    /// READ-ONLY half of [`dispatch`](Self::dispatch): it records NOTHING and runs no act, so it is
    /// side-effect-free (safe for dashboards / dry-runs). A grant that was never issued is a
    /// fail-closed `Denied(NoGrant)`. Does NOT authenticate the intent — see
    /// [`authenticated_preview`](Self::authenticated_preview) for the transport-facing entry.
    pub fn preview(&self, intent: &IntentDeclarationV1) -> EnvelopeCheck {
        match self.store.get(&intent.standing_grant_id) {
            Some(envelope) => check_intent_within_envelope(intent, &envelope),
            None => EnvelopeCheck::Denied(EnvelopeDenial::NoGrant),
        }
    }

    /// Authenticate an intent, then preview its verdict — the entry an untrusted transport (the HTTP
    /// boundary) uses. Returns `None` when the declaration is not authentic (its signature does not
    /// verify against the key it names), so a forged or malformed intent is rejected fail-closed
    /// BEFORE any grant lookup; otherwise the containment verdict. Still side-effect-free.
    pub fn authenticated_preview(&self, intent: &IntentDeclarationV1) -> Option<EnvelopeCheck> {
        if !intent.verify_self() {
            return None;
        }
        Some(self.preview(intent))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> SigningKey {
        SigningKey::generate(&mut rand::thread_rng())
    }

    fn an_intent(sk: &SigningKey, method: &str, args_hash: &str) -> IntentDeclarationV1 {
        an_intent_for(sk, "vm-agent", method, args_hash)
    }

    fn an_intent_for(
        sk: &SigningKey,
        capsule: &str,
        method: &str,
        args_hash: &str,
    ) -> IntentDeclarationV1 {
        IntentDeclarationV1::issue(
            sk,
            sk.verifying_key().to_bytes(),
            "intent-1",
            capsule,
            method,
            args_hash,
            "elastos://mail/send",
            "execute",
            "grant-1",
        )
    }

    fn an_envelope(methods: &[&str]) -> StandingGrantEnvelope {
        StandingGrantEnvelope {
            grant_id: "grant-1".to_string(),
            capsule: "vm-agent".to_string(),
            allowed_methods: methods.iter().map(|m| m.to_string()).collect(),
            resource: "elastos://mail/send".to_string(),
            action: "execute".to_string(),
            expires_at: Some(SecureTimestamp::after_secs(3600)),
            revoked: false,
            agent_pubkey: None,
            token_epoch: 0,
        }
    }

    fn a_receipt(
        sk: &SigningKey,
        capsule: &str,
        method: &str,
        args_hash: &str,
        resource: &str,
        action: &str,
    ) -> AffordanceGrantReceiptV1 {
        AffordanceGrantReceiptV1::issue(
            sk,
            sk.verifying_key().to_bytes(),
            "tok-1",
            capsule,
            method,
            args_hash,
            resource,
            action,
        )
    }

    // ── Records sign + verify, and tampering breaks the signature ──────────────

    #[test]
    fn intent_declaration_signs_and_verifies_and_tamper_breaks_it() {
        let sk = key();
        let vk = sk.verifying_key();
        let intent = an_intent(&sk, "send", "args-abc");
        assert_eq!(intent.schema, INTENT_DECLARATION_SCHEMA_V1);
        assert!(intent.verify(&vk), "a freshly issued intent verifies");
        // A different key never verifies.
        assert!(!intent.verify(&key().verifying_key()));
        // Every bound field is covered.
        for mutate in [
            |i: &mut IntentDeclarationV1| i.method_id = "delete".to_string(),
            |i: &mut IntentDeclarationV1| i.input_hash = "deadbeef".to_string(),
            |i: &mut IntentDeclarationV1| i.capsule = "vm-evil".to_string(),
            |i: &mut IntentDeclarationV1| i.resource = "elastos://mail/all".to_string(),
            |i: &mut IntentDeclarationV1| i.action = "admin".to_string(),
            |i: &mut IntentDeclarationV1| i.standing_grant_id = "grant-evil".to_string(),
        ] {
            let mut t = intent.clone();
            mutate(&mut t);
            assert!(!t.verify(&vk), "a tampered intent must not verify");
        }
    }

    #[test]
    fn reconciliation_record_signs_and_verifies() {
        let sk = key();
        let vk = sk.verifying_key();
        let rec = IntentReconciliationV1::issue(
            &sk,
            vk.to_bytes(),
            "intent-1",
            "tok-1",
            ReconciliationStatus::Matched,
            "",
        );
        assert!(rec.verify(&vk));
        // Tampering the status (the load-bearing field) breaks it.
        let mut t = rec.clone();
        t.status = ReconciliationStatus::Diverged;
        assert!(
            !t.verify(&vk),
            "flipping the verdict must break the signature"
        );
    }

    // ── The fail-closed envelope-containment branch matrix ─────────────────────

    #[test]
    fn intent_within_envelope_is_allowed() {
        let sk = key();
        let intent = an_intent(&sk, "send", "args-abc");
        assert_eq!(
            check_intent_within_envelope(&intent, &an_envelope(&["send", "draft"])),
            EnvelopeCheck::Allowed
        );
    }

    #[test]
    fn method_outside_envelope_is_denied() {
        let sk = key();
        let intent = an_intent(&sk, "delete", "args-abc"); // not in the envelope
        assert_eq!(
            check_intent_within_envelope(&intent, &an_envelope(&["send", "draft"])),
            EnvelopeCheck::Denied(EnvelopeDenial::MethodNotInEnvelope)
        );
    }

    #[test]
    fn wrong_capsule_resource_or_action_is_denied() {
        let sk = key();
        let base = an_envelope(&["send"]);

        let mut wrong_cap = base.clone();
        wrong_cap.capsule = "vm-other".to_string();
        assert_eq!(
            check_intent_within_envelope(&an_intent(&sk, "send", "h"), &wrong_cap),
            EnvelopeCheck::Denied(EnvelopeDenial::WrongCapsule)
        );

        let mut wrong_res = base.clone();
        wrong_res.resource = "elastos://mail/all".to_string();
        assert_eq!(
            check_intent_within_envelope(&an_intent(&sk, "send", "h"), &wrong_res),
            EnvelopeCheck::Denied(EnvelopeDenial::WrongResource)
        );

        let mut wrong_act = base.clone();
        wrong_act.action = "admin".to_string();
        assert_eq!(
            check_intent_within_envelope(&an_intent(&sk, "send", "h"), &wrong_act),
            EnvelopeCheck::Denied(EnvelopeDenial::WrongAction)
        );
    }

    #[test]
    fn revoked_or_expired_envelope_is_denied_before_any_field_match() {
        let sk = key();
        let intent = an_intent(&sk, "send", "h");

        let mut revoked = an_envelope(&["send"]);
        revoked.revoked = true;
        assert_eq!(
            check_intent_within_envelope(&intent, &revoked),
            EnvelopeCheck::Denied(EnvelopeDenial::Revoked),
            "a revoked envelope authorizes nothing, even a perfectly-matching intent"
        );

        let mut expired = an_envelope(&["send"]);
        expired.expires_at = Some(SecureTimestamp::at(1)); // far in the past
        assert_eq!(
            check_intent_within_envelope(&intent, &expired),
            EnvelopeCheck::Denied(EnvelopeDenial::Expired)
        );
        assert!(!expired.is_active());
    }

    #[test]
    fn input_hash_is_not_constrained_by_the_envelope() {
        // The envelope authorizes the method/resource/action, NOT a frozen arg hash — two
        // intents with different args both pass (repeated acts within one standing grant).
        let sk = key();
        let env = an_envelope(&["send"]);
        assert_eq!(
            check_intent_within_envelope(&an_intent(&sk, "send", "args-one"), &env),
            EnvelopeCheck::Allowed
        );
        assert_eq!(
            check_intent_within_envelope(&an_intent(&sk, "send", "args-two"), &env),
            EnvelopeCheck::Allowed
        );
    }

    // ── Chunk 3: derive the envelope from a real CapabilityToken ───────────────

    #[test]
    fn from_token_derives_the_envelope_and_drives_the_same_check() {
        use crate::capability::token::{Action, ResourceId, TokenConstraints};

        let token = CapabilityToken::new(
            "vm-agent".to_string(),
            [0u8; 32],
            ResourceId::new("elastos://mail/send"),
            Action::Execute,
            TokenConstraints::default(),
            SecureTimestamp::now(),
            Some(SecureTimestamp::after_secs(3600)),
        );
        let methods: BTreeSet<String> = ["send", "draft"].iter().map(|m| m.to_string()).collect();
        let env = StandingGrantEnvelope::from_token(&token, methods, false, None);

        // The token supplies capsule/resource/action/expiry that were actually signed in.
        assert_eq!(env.capsule, "vm-agent");
        assert_eq!(env.resource, "elastos://mail/send");
        assert_eq!(env.action, "execute", "Action::Execute → \"execute\"");
        assert_eq!(env.grant_id, token.id().to_string());
        assert!(
            env.is_active(),
            "a fresh, unrevoked, future-expiry token is active"
        );

        // The token-derived envelope drives the SAME containment check as a hand-built one.
        let sk = key();
        assert_eq!(
            check_intent_within_envelope(&an_intent(&sk, "send", "h"), &env),
            EnvelopeCheck::Allowed
        );
        assert_eq!(
            check_intent_within_envelope(&an_intent(&sk, "delete", "h"), &env),
            EnvelopeCheck::Denied(EnvelopeDenial::MethodNotInEnvelope)
        );
    }

    #[test]
    fn from_token_none_expiry_never_expires_and_revoked_is_honored() {
        use crate::capability::token::{Action, ResourceId, TokenConstraints};

        // None expiry = "until revoked": the envelope never expires on time alone.
        let token = CapabilityToken::new(
            "vm-agent".to_string(),
            [0u8; 32],
            ResourceId::new("elastos://mail/send"),
            Action::Execute,
            TokenConstraints::default(),
            SecureTimestamp::now(),
            None,
        );
        let methods: BTreeSet<String> = ["send"].iter().map(|m| m.to_string()).collect();
        let active = StandingGrantEnvelope::from_token(&token, methods.clone(), false, None);
        assert_eq!(active.expires_at, None);
        assert!(active.is_active(), "None expiry never expires");

        // The caller's external revocation check is honored, fail-closed.
        let revoked = StandingGrantEnvelope::from_token(&token, methods, true, None);
        assert!(!revoked.is_active());
        let sk = key();
        assert_eq!(
            check_intent_within_envelope(&an_intent(&sk, "send", "h"), &revoked),
            EnvelopeCheck::Denied(EnvelopeDenial::Revoked)
        );
    }

    // ── The reconciliation branch matrix ───────────────────────────────────────

    #[test]
    fn reconcile_matched_when_receipt_equals_intent() {
        let sk = key();
        let intent = an_intent(&sk, "send", "args-abc");
        let receipt = a_receipt(
            &sk,
            "vm-agent",
            "send",
            "args-abc",
            "elastos://mail/send",
            "execute",
        );
        let (status, detail) = reconcile(&intent, Some(&receipt));
        assert_eq!(status, ReconciliationStatus::Matched);
        assert_eq!(detail, "");
    }

    #[test]
    fn reconcile_diverged_when_a_bound_field_differs() {
        let sk = key();
        let intent = an_intent(&sk, "send", "args-abc");
        // Same method/resource/action, but the redeemed ARGS differ from the declared ones.
        let receipt = a_receipt(
            &sk,
            "vm-agent",
            "send",
            "args-XXX",
            "elastos://mail/send",
            "execute",
        );
        let (status, detail) = reconcile(&intent, Some(&receipt));
        assert_eq!(status, ReconciliationStatus::Diverged);
        assert!(
            detail.contains("input_hash"),
            "the diverged field is named: {detail}"
        );
    }

    #[test]
    fn reconcile_undelivered_when_no_receipt() {
        let sk = key();
        let intent = an_intent(&sk, "send", "args-abc");
        let (status, detail) = reconcile(&intent, None);
        assert_eq!(status, ReconciliationStatus::Undelivered);
        assert!(
            !detail.is_empty(),
            "undelivered is recorded with a reason, never silent"
        );
    }

    // ── Chunk 4: the enforcement gate (the act runs ONLY past a passing verify) ─

    fn gate_log() -> (tempfile::TempDir, crate::primitives::audit::AuditLog) {
        let dir = tempfile::tempdir().unwrap();
        let log = crate::primitives::audit::AuditLog::with_file(dir.path().join("a.log")).unwrap();
        (dir, log)
    }

    #[test]
    fn the_gate_denies_outside_the_envelope_and_never_runs_the_act() {
        let (_dir, log) = gate_log();
        let sk = key();
        let intent = an_intent(&sk, "delete", "h"); // method not in the envelope
        let env = an_envelope(&["send"]);
        let ran = std::cell::Cell::new(false);
        let outcome = run_intent_gate(
            &log,
            &sk,
            sk.verifying_key().to_bytes(),
            &intent,
            &env,
            || {
                ran.set(true);
                Some(a_receipt(
                    &sk,
                    "vm-agent",
                    "delete",
                    "h",
                    "elastos://mail/send",
                    "execute",
                ))
            },
        );
        assert!(matches!(
            outcome,
            IntentGateOutcome::Denied(EnvelopeDenial::MethodNotInEnvelope)
        ));
        assert!(
            !ran.get(),
            "a denied intent must NEVER run the act (fail-closed enforcement)"
        );
        // declared + denied are both on-chain, and the chain verifies.
        let att = log.chain_attestation().unwrap();
        assert!(att.verified);
        assert_eq!(att.records, 2, "declared + denied, no act");
    }

    #[test]
    fn the_gate_runs_the_act_past_the_envelope_and_reconciles_matched() {
        let (_dir, log) = gate_log();
        let sk = key();
        let intent = an_intent(&sk, "send", "args-abc");
        let env = an_envelope(&["send"]);
        let ran = std::cell::Cell::new(false);
        let outcome = run_intent_gate(
            &log,
            &sk,
            sk.verifying_key().to_bytes(),
            &intent,
            &env,
            || {
                ran.set(true);
                Some(a_receipt(
                    &sk,
                    "vm-agent",
                    "send",
                    "args-abc",
                    "elastos://mail/send",
                    "execute",
                ))
            },
        );
        assert!(ran.get(), "an allowed intent runs the act");
        match outcome {
            IntentGateOutcome::Acted(rec) => assert_eq!(rec.status, ReconciliationStatus::Matched),
            other => panic!("expected Acted(matched), got {other:?}"),
        }
        let att = log.chain_attestation().unwrap();
        assert!(att.verified);
        assert_eq!(att.records, 2, "declared + reconciled, no denial");
    }

    #[test]
    fn the_gate_records_undelivered_when_the_act_does_not_act() {
        let (_dir, log) = gate_log();
        let sk = key();
        let intent = an_intent(&sk, "send", "args-abc");
        let env = an_envelope(&["send"]);
        let outcome = run_intent_gate(
            &log,
            &sk,
            sk.verifying_key().to_bytes(),
            &intent,
            &env,
            || None,
        );
        match outcome {
            IntentGateOutcome::Acted(rec) => {
                assert_eq!(rec.status, ReconciliationStatus::Undelivered);
                assert!(rec.receipt_id.is_empty(), "no receipt id when undelivered");
            }
            other => panic!("expected Acted(undelivered), got {other:?}"),
        }
    }

    // ── Chunk 2: on-chain custody events ───────────────────────────────────────

    #[test]
    fn intent_events_carry_their_type_names_and_denial_reason() {
        let sk = key();
        let intent = an_intent(&sk, "send", "h");
        assert_eq!(
            intent_declared_event(&intent).event_type_name(),
            "intent_declared"
        );
        let denied = intent_denied_event(&intent, EnvelopeDenial::MethodNotInEnvelope);
        assert_eq!(denied.event_type_name(), "intent_denied");
        match denied {
            AuditEvent::IntentDenied { reason, .. } => {
                assert_eq!(reason, "method_not_in_envelope")
            }
            other => panic!("expected IntentDenied, got {other:?}"),
        }
        let rec = IntentReconciliationV1::issue(
            &sk,
            sk.verifying_key().to_bytes(),
            "intent-1",
            "",
            ReconciliationStatus::Matched,
            "",
        );
        assert_eq!(
            intent_reconciled_event(&rec, "vm-agent").event_type_name(),
            "intent_reconciled"
        );
    }

    #[test]
    fn every_denial_reason_has_a_stable_string() {
        for (denial, expected) in [
            (EnvelopeDenial::Revoked, "revoked"),
            (EnvelopeDenial::Expired, "expired"),
            (EnvelopeDenial::WrongCapsule, "wrong_capsule"),
            (
                EnvelopeDenial::MethodNotInEnvelope,
                "method_not_in_envelope",
            ),
            (EnvelopeDenial::WrongResource, "wrong_resource"),
            (EnvelopeDenial::WrongAction, "wrong_action"),
        ] {
            assert_eq!(denial.as_str(), expected);
        }
    }

    #[test]
    fn intent_custody_events_emit_onto_the_durable_chain_and_verify() {
        // The Kent Beck bar: the declaration, the denial (with reason), and the verdict ride
        // the SAME ed25519 signed hash chain as every other custody record — and the whole
        // chain self-verifies via chain_attestation, exactly as in production.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");
        let log = crate::primitives::audit::AuditLog::with_file(&path).unwrap();

        let sk = key();
        let intent = an_intent(&sk, "send", "args-abc");

        // 1. declared
        log.emit(intent_declared_event(&intent))
            .expect("emit declared");

        // 2. denied — method not in this envelope ⇒ a real EnvelopeDenial reason on-chain
        let EnvelopeCheck::Denied(reason) =
            check_intent_within_envelope(&intent, &an_envelope(&["draft"]))
        else {
            panic!("expected a denial");
        };
        log.emit(intent_denied_event(&intent, reason))
            .expect("emit denied");

        // 3. reconciled — undelivered (declared, no receipt)
        let (status, detail) = reconcile(&intent, None);
        let rec = IntentReconciliationV1::issue(
            &sk,
            sk.verifying_key().to_bytes(),
            &intent.intent_id,
            "",
            status,
            &detail,
        );
        log.emit(intent_reconciled_event(&rec, &intent.capsule))
            .expect("emit reconciled");

        // The whole chain (declared → denied → reconciled) verifies under the log's key.
        let att = log.chain_attestation().expect("file-backed ⇒ attestable");
        assert!(
            att.verified,
            "intent custody records chain + verify: {att:?}"
        );
        assert_eq!(
            att.records, 3,
            "all three intent custody records are on-chain"
        );
    }

    // ── Chunk 5b: per-capsule intent-proof tally ───────────────────────────────

    #[test]
    fn count_intent_proof_tallies_issues_and_ignores_matched_and_other_capsules() {
        let sk = key();
        let mine = an_intent(&sk, "send", "h"); // capsule = vm-agent
        let denied = intent_denied_event(&mine, EnvelopeDenial::MethodNotInEnvelope);
        let diverged = intent_reconciled_event(
            &IntentReconciliationV1::issue(
                &sk,
                sk.verifying_key().to_bytes(),
                "i1",
                "t1",
                ReconciliationStatus::Diverged,
                "diverged fields: input_hash",
            ),
            "vm-agent",
        );
        let undelivered = intent_reconciled_event(
            &IntentReconciliationV1::issue(
                &sk,
                sk.verifying_key().to_bytes(),
                "i2",
                "",
                ReconciliationStatus::Undelivered,
                "no receipt",
            ),
            "vm-agent",
        );
        // Not issues: a matched verdict, a declaration, and an issue for ANOTHER capsule.
        let matched = intent_reconciled_event(
            &IntentReconciliationV1::issue(
                &sk,
                sk.verifying_key().to_bytes(),
                "i3",
                "t3",
                ReconciliationStatus::Matched,
                "",
            ),
            "vm-agent",
        );
        let declared = intent_declared_event(&mine);
        let other = intent_denied_event(
            &an_intent_for(&sk, "vm-other", "send", "h"),
            EnvelopeDenial::Revoked,
        );

        // A capsule with intent activity but NO issues (only a declaration) is clean.
        let clean_decl = intent_declared_event(&an_intent_for(&sk, "vm-clean", "send", "h"));
        let events = [
            denied,
            diverged,
            undelivered,
            matched,
            declared,
            other,
            clean_decl,
        ];

        let s =
            count_intent_proof(events.iter(), "vm-agent").expect("vm-agent has intent activity");
        assert_eq!(s.denied, 1);
        assert_eq!(s.diverged, 1);
        assert_eq!(s.undelivered, 1);
        assert_eq!(s.flagged(), 3);
        assert!(!s.is_clean());

        // Present + no issues ⇒ CLEAN (all-zero), distinct from absent.
        let clean = count_intent_proof(events.iter(), "vm-clean").expect("a declaration ⇒ present");
        assert!(clean.is_clean());
        assert_eq!(clean.flagged(), 0);

        // NO intent activity ⇒ ABSENT (None), never falsely clean.
        assert!(
            count_intent_proof(events.iter(), "vm-nobody").is_none(),
            "no intent activity ⇒ absent, not clean"
        );
    }

    #[test]
    fn audit_log_intent_proof_summary_counts_over_the_buffer() {
        let (_dir, log) = gate_log();
        let sk = key();
        let intent = an_intent(&sk, "send", "h");
        // Record a denial + a diverged reconciliation + a matched one (not an issue).
        log.emit(intent_denied_event(&intent, EnvelopeDenial::WrongResource))
            .unwrap();
        log.emit(intent_reconciled_event(
            &IntentReconciliationV1::issue(
                &sk,
                sk.verifying_key().to_bytes(),
                "i1",
                "t1",
                ReconciliationStatus::Diverged,
                "diverged fields: action",
            ),
            "vm-agent",
        ))
        .unwrap();
        log.emit(intent_reconciled_event(
            &IntentReconciliationV1::issue(
                &sk,
                sk.verifying_key().to_bytes(),
                "i2",
                "t2",
                ReconciliationStatus::Matched,
                "",
            ),
            "vm-agent",
        ))
        .unwrap();

        let s = log
            .intent_proof_summary("vm-agent")
            .expect("vm-agent has intent activity");
        assert_eq!(s.denied, 1);
        assert_eq!(s.diverged, 1);
        assert_eq!(s.undelivered, 0);
        // An unrelated capsule with no intent activity is ABSENT (None), never fabricated.
        assert!(log.intent_proof_summary("vm-elsewhere").is_none());
    }

    // ── StandingGrantStore (chunk 2c-1): fail-closed issue/revoke registry ──

    /// An envelope with a specific grant_id + expiry, for store tests.
    fn envelope_with(grant_id: &str, expires_at: Option<SecureTimestamp>) -> StandingGrantEnvelope {
        StandingGrantEnvelope {
            grant_id: grant_id.to_string(),
            capsule: "vm-agent".to_string(),
            allowed_methods: ["send"].iter().map(|m| m.to_string()).collect(),
            resource: "elastos://mail/send".to_string(),
            action: "execute".to_string(),
            expires_at,
            revoked: false,
            agent_pubkey: None,
            token_epoch: 0,
        }
    }

    #[test]
    fn store_issue_then_get_returns_the_envelope() {
        let store = StandingGrantStore::new();
        assert!(store.get("g1").is_none(), "an unissued grant is absent");
        assert!(!store.is_active("g1"), "an unissued grant is not active");

        store.issue(envelope_with("g1", Some(SecureTimestamp::after_secs(3600)))).unwrap();
        let got = store.get("g1").expect("issued grant is retrievable");
        assert_eq!(got.grant_id, "g1");
        assert!(!got.revoked);
        assert!(store.is_active("g1"), "a fresh, unexpired grant is active");
    }

    #[test]
    fn store_revoke_flips_the_flag_keeps_the_record_and_is_idempotent() {
        let store = StandingGrantStore::new();
        store.issue(envelope_with("g1", None)).unwrap(); // None expiry ⇒ never expires until revoked.
        assert!(store.is_active("g1"));

        assert!(store.revoke("g1").unwrap(), "revoking a live grant returns true");
        // The record is KEPT, now marked revoked — queryable as revoked for honest denial.
        let got = store.get("g1").expect("a revoked grant is still queryable");
        assert!(got.revoked, "the stored envelope is marked revoked");
        assert!(!store.is_active("g1"), "a revoked grant is not active");

        // Idempotent: a second revoke (already revoked) returns false — no live grant was revoked.
        assert!(!store.revoke("g1").unwrap(), "double-revoke returns false");
        // Revoking an unknown id is a fail-closed no-op, never a panic.
        assert!(!store.revoke("does-not-exist").unwrap());
    }

    #[test]
    fn store_never_un_revokes_and_expiry_deactivates_without_revocation() {
        let store = StandingGrantStore::new();

        // Issuing an already-revoked envelope stores it as revoked — issue never un-revokes.
        let mut revoked_env = envelope_with("g1", None);
        revoked_env.revoked = true;
        store.issue(revoked_env).unwrap();
        assert!(
            !store.is_active("g1"),
            "an issued-revoked grant is not active"
        );
        assert!(store.get("g1").unwrap().revoked);

        // A past expiry deactivates a grant even though it was never revoked (fail-closed on time).
        store.issue(envelope_with("g2", Some(SecureTimestamp::after_secs(0)))).unwrap();
        assert!(
            !store.is_active("g2"),
            "an expired grant is inactive without any revocation"
        );
        // ...but it is still retrievable, so the gate can deny it with an honest `Expired` reason.
        assert!(store.get("g2").is_some());
    }

    // ── dispatch_standing_act (chunk 2c-2): unsupervised dispatch through the gate ──

    /// Issue `an_envelope(methods)` (grant_id "grant-1") into a fresh store.
    fn store_with(methods: &[&str]) -> StandingGrantStore {
        let store = StandingGrantStore::new();
        store.issue(an_envelope(methods)).unwrap();
        store
    }

    #[test]
    fn dispatch_runs_the_act_under_an_active_grant_and_reconciles_matched() {
        let (_dir, log) = gate_log();
        let sk = key();
        let store = store_with(&["send"]);
        let intent = an_intent(&sk, "send", "args-abc"); // standing_grant_id == "grant-1"
        let ran = std::cell::Cell::new(false);
        let outcome = dispatch_standing_act(
            &store,
            &log,
            &sk,
            sk.verifying_key().to_bytes(),
            &intent,
            || {
                ran.set(true);
                Some(a_receipt(
                    &sk,
                    "vm-agent",
                    "send",
                    "args-abc",
                    "elastos://mail/send",
                    "execute",
                ))
            },
        );
        assert!(ran.get(), "an intent within an active grant runs the act");
        match outcome {
            IntentGateOutcome::Acted(rec) => assert_eq!(rec.status, ReconciliationStatus::Matched),
            other => panic!("expected Acted(matched), got {other:?}"),
        }
    }

    #[test]
    fn dispatch_catches_divergence_after_the_act() {
        let (_dir, log) = gate_log();
        let sk = key();
        let store = store_with(&["send"]);
        let intent = an_intent(&sk, "send", "args-declared");
        // The act runs within the envelope but delivers a DIFFERENT input_hash than declared.
        let outcome = dispatch_standing_act(
            &store,
            &log,
            &sk,
            sk.verifying_key().to_bytes(),
            &intent,
            || {
                Some(a_receipt(
                    &sk,
                    "vm-agent",
                    "send",
                    "args-DIFFERENT",
                    "elastos://mail/send",
                    "execute",
                ))
            },
        );
        match outcome {
            IntentGateOutcome::Acted(rec) => {
                assert_eq!(rec.status, ReconciliationStatus::Diverged);
                assert!(rec.divergence_detail.contains("input_hash"));
            }
            other => panic!("expected Acted(diverged), got {other:?}"),
        }
    }

    #[test]
    fn dispatch_denies_out_of_envelope_before_the_act() {
        let (_dir, log) = gate_log();
        let sk = key();
        let store = store_with(&["send"]); // "delete" is NOT in the envelope
        let intent = an_intent(&sk, "delete", "h");
        let ran = std::cell::Cell::new(false);
        let outcome = dispatch_standing_act(
            &store,
            &log,
            &sk,
            sk.verifying_key().to_bytes(),
            &intent,
            || {
                ran.set(true);
                None
            },
        );
        assert!(matches!(
            outcome,
            IntentGateOutcome::Denied(EnvelopeDenial::MethodNotInEnvelope)
        ));
        assert!(
            !ran.get(),
            "a denied intent never runs the act (fail-closed)"
        );
    }

    #[test]
    fn dispatch_denies_a_revoked_grant() {
        let (_dir, log) = gate_log();
        let sk = key();
        let store = store_with(&["send"]);
        assert!(store.revoke("grant-1").unwrap(), "revoke the standing grant");
        let intent = an_intent(&sk, "send", "args-abc");
        let ran = std::cell::Cell::new(false);
        let outcome = dispatch_standing_act(
            &store,
            &log,
            &sk,
            sk.verifying_key().to_bytes(),
            &intent,
            || {
                ran.set(true);
                None
            },
        );
        assert!(matches!(
            outcome,
            IntentGateOutcome::Denied(EnvelopeDenial::Revoked)
        ));
        assert!(!ran.get(), "a revoked grant authorizes nothing");
    }

    #[test]
    fn dispatch_denies_and_records_when_no_grant_was_issued() {
        let (_dir, log) = gate_log();
        let sk = key();
        let store = StandingGrantStore::new(); // empty — no grant for "grant-1"
        let intent = an_intent(&sk, "send", "args-abc");
        let ran = std::cell::Cell::new(false);
        let outcome = dispatch_standing_act(
            &store,
            &log,
            &sk,
            sk.verifying_key().to_bytes(),
            &intent,
            || {
                ran.set(true);
                None
            },
        );
        assert!(matches!(
            outcome,
            IntentGateOutcome::Denied(EnvelopeDenial::NoGrant)
        ));
        assert!(
            !ran.get(),
            "a missing grant authorizes nothing (fail-closed)"
        );
        // The refusal is on-chain: declaration + denial recorded, chain verifies, and the
        // intent channel counts it as a denial (never a silent drop).
        let att = log.chain_attestation().unwrap();
        assert!(att.verified);
        assert_eq!(att.records, 2, "declared + denied, no act");
        let summary = log
            .intent_proof_summary("vm-agent")
            .expect("the declared intent makes vm-agent PRESENT on the intent channel");
        assert_eq!(summary.denied, 1, "a missing-grant refusal is a denial");
    }

    // ── Chunk 2c-3: end-to-end from a REAL capability token ────────────────────

    #[test]
    fn end_to_end_a_real_token_runs_the_agent_then_revocation_shuts_it_down() {
        use crate::capability::token::{Action, CapabilityToken, ResourceId};

        let (_dir, log) = gate_log();
        let sk = key();
        let store = StandingGrantStore::new();

        // 1. A REAL issued capability token is the root of authority for the standing grant.
        let token = CapabilityToken::new(
            "vm-agent".to_string(),
            [0u8; 32],
            ResourceId::new("elastos://mail/send"),
            Action::Execute,
            Default::default(),
            SecureTimestamp::now(),
            Some(SecureTimestamp::after_secs(3600)),
        );
        let grant_id = token.id().to_string();

        // 2. Derive the standing envelope FROM the token (the honest seam: the token supplies
        //    capsule/resource/action/expiry; the method set + revocation are layered on) and issue it.
        let envelope = StandingGrantEnvelope::from_token(
            &token,
            ["send"].iter().map(|m| m.to_string()).collect(),
            false,
            None,
        );
        store.issue(envelope).unwrap();

        // A fresh, correctly-signed intent for THIS token's grant, invoking an in-envelope method.
        let declare = |args: &str| {
            IntentDeclarationV1::issue(
                &sk,
                sk.verifying_key().to_bytes(),
                "intent-e2e",
                "vm-agent",
                "send",
                args,
                "elastos://mail/send",
                "execute",
                &grant_id,
            )
        };

        // 3. Unsupervised dispatch under the LIVE grant: the act runs past the gate and reconciles matched.
        let outcome = dispatch_standing_act(
            &store,
            &log,
            &sk,
            sk.verifying_key().to_bytes(),
            &declare("args-1"),
            || {
                Some(a_receipt(
                    &sk,
                    "vm-agent",
                    "send",
                    "args-1",
                    "elastos://mail/send",
                    "execute",
                ))
            },
        );
        assert!(
            matches!(outcome, IntentGateOutcome::Acted(rec) if rec.status == ReconciliationStatus::Matched),
            "an intent within a live token-derived grant runs and reconciles matched"
        );

        // 4. Revoke the standing grant by the TOKEN's id — the SAME dispatch is now denied,
        //    fail-closed, and the act never runs. This is the kill switch on an autonomous agent.
        assert!(store.revoke(&grant_id).unwrap(), "revoke the token's standing grant");
        let ran = std::cell::Cell::new(false);
        let after = dispatch_standing_act(
            &store,
            &log,
            &sk,
            sk.verifying_key().to_bytes(),
            &declare("args-2"),
            || {
                ran.set(true);
                Some(a_receipt(
                    &sk,
                    "vm-agent",
                    "send",
                    "args-2",
                    "elastos://mail/send",
                    "execute",
                ))
            },
        );
        assert!(matches!(
            after,
            IntentGateOutcome::Denied(EnvelopeDenial::Revoked)
        ));
        assert!(
            !ran.get(),
            "a revoked standing grant shuts the agent down — the act never runs"
        );

        // 5. The whole story is on the signed chain and shows on the intent channel: the
        //    reconciled act + the post-revocation denial, and the chain verifies end to end.
        let att = log.chain_attestation().unwrap();
        assert!(att.verified, "the intent-proof chain self-verifies");
        let summary = log
            .intent_proof_summary("vm-agent")
            .expect("the agent is PRESENT on the intent channel");
        assert_eq!(
            summary.denied, 1,
            "the post-revocation attempt is one denial"
        );
        assert_eq!(summary.diverged, 0);
        assert_eq!(summary.undelivered, 0);
    }

    // ── Chunk 2c-gw-A: StandingGrantService (the API-facing seam) ──────────────

    #[test]
    fn service_issues_from_a_token_dispatches_then_revocation_denies() {
        use crate::capability::token::{Action, CapabilityToken, ResourceId};

        let dir = tempfile::tempdir().unwrap();
        let audit = std::sync::Arc::new(
            crate::primitives::audit::AuditLog::with_file(dir.path().join("a.log")).unwrap(),
        );
        let sk = key();
        let svc = StandingGrantService::new(audit.clone(), sk.clone());

        // Issue a standing grant from a real token; the returned grant_id is the token's id.
        let token = CapabilityToken::new(
            "vm-agent".to_string(),
            [0u8; 32],
            ResourceId::new("elastos://mail/send"),
            Action::Execute,
            Default::default(),
            SecureTimestamp::now(),
            Some(SecureTimestamp::after_secs(3600)),
        );
        let grant_id =
            svc.issue_from_token(&token, ["send"].iter().map(|m| m.to_string()).collect(), None).unwrap();
        assert_eq!(grant_id, token.id().to_string());
        assert!(svc.is_active(&grant_id), "a freshly issued grant is active");

        let declare = |args: &str| {
            IntentDeclarationV1::issue(
                &sk,
                sk.verifying_key().to_bytes(),
                "svc-intent",
                "vm-agent",
                "send",
                args,
                "elastos://mail/send",
                "execute",
                &grant_id,
            )
        };

        // Dispatch through the service: the act runs and reconciles matched.
        let outcome = svc.dispatch(&declare("a1"), || {
            Some(a_receipt(
                &sk,
                "vm-agent",
                "send",
                "a1",
                "elastos://mail/send",
                "execute",
            ))
        });
        assert!(
            matches!(outcome, IntentGateOutcome::Acted(rec) if rec.status == ReconciliationStatus::Matched)
        );

        // Revoke through the service: the grant goes inactive and the next dispatch is denied.
        assert!(svc.revoke(&grant_id).unwrap());
        assert!(!svc.is_active(&grant_id));
        let ran = std::cell::Cell::new(false);
        let after = svc.dispatch(&declare("a2"), || {
            ran.set(true);
            None
        });
        assert!(matches!(
            after,
            IntentGateOutcome::Denied(EnvelopeDenial::Revoked)
        ));
        assert!(
            !ran.get(),
            "a revoked grant denies the act through the service too"
        );

        // The service wrote to the SHARED audit log: the story is queryable on the intent channel.
        let summary = audit.intent_proof_summary("vm-agent").expect("present");
        assert_eq!(summary.denied, 1);
    }

    #[test]
    fn service_dispatch_with_no_issued_grant_is_denied_no_grant() {
        let dir = tempfile::tempdir().unwrap();
        let audit = std::sync::Arc::new(
            crate::primitives::audit::AuditLog::with_file(dir.path().join("a.log")).unwrap(),
        );
        let sk = key();
        let svc = StandingGrantService::new(audit, sk.clone());
        // Never issued anything ⇒ any dispatch is a fail-closed NoGrant denial.
        let intent = an_intent(&sk, "send", "a1");
        let ran = std::cell::Cell::new(false);
        let outcome = svc.dispatch(&intent, || {
            ran.set(true);
            None
        });
        assert!(matches!(
            outcome,
            IntentGateOutcome::Denied(EnvelopeDenial::NoGrant)
        ));
        assert!(!ran.get());
    }

    // ── Chunk 2c-gw-C: signed-intent authenticity + read-only preview ──────────

    #[test]
    fn verify_self_accepts_authentic_and_rejects_tampered() {
        let sk = key();
        let intent = an_intent(&sk, "send", "h1");
        assert!(
            intent.verify_self(),
            "a freshly issued intent verifies against its own signer"
        );

        // Tamper with a signed field: the signature no longer matches the digest.
        let mut edited = intent.clone();
        edited.method_id = "delete".to_string();
        assert!(
            !edited.verify_self(),
            "a mutated field breaks self-verification"
        );

        // Swap in a DIFFERENT signer key (that did not sign this): rejected.
        let mut wrong_signer = intent.clone();
        wrong_signer.signer = hex::encode(key().verifying_key().to_bytes());
        assert!(
            !wrong_signer.verify_self(),
            "a mismatched signer key fails closed"
        );

        // A structurally malformed signer is rejected, not panicked on.
        let mut junk = intent.clone();
        junk.signer = "not-hex".to_string();
        assert!(!junk.verify_self());
    }

    #[test]
    fn service_preview_is_side_effect_free_and_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let audit = std::sync::Arc::new(
            crate::primitives::audit::AuditLog::with_file(dir.path().join("a.log")).unwrap(),
        );
        let sk = key();
        let svc = StandingGrantService::new(audit.clone(), sk.clone());

        // No grant issued yet ⇒ fail-closed NoGrant, and NOTHING is recorded (preview never writes).
        let intent = an_intent(&sk, "send", "h1");
        assert!(matches!(
            svc.preview(&intent),
            EnvelopeCheck::Denied(EnvelopeDenial::NoGrant)
        ));
        assert!(
            audit.intent_proof_summary("vm-agent").is_none(),
            "preview must record nothing — the capsule stays ABSENT on the intent channel"
        );

        // Issue a grant, then preview both an in-envelope and an out-of-envelope intent.
        svc.store.issue(an_envelope(&["send"])).unwrap();
        assert_eq!(svc.preview(&intent), EnvelopeCheck::Allowed);
        let out = an_intent(&sk, "delete", "h1"); // method not in the envelope
        assert!(matches!(
            svc.preview(&out),
            EnvelopeCheck::Denied(EnvelopeDenial::MethodNotInEnvelope)
        ));
        // Still nothing recorded after several previews.
        assert!(audit.intent_proof_summary("vm-agent").is_none());
    }

    #[test]
    fn authenticated_preview_rejects_a_forged_intent_before_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let audit = std::sync::Arc::new(
            crate::primitives::audit::AuditLog::with_file(dir.path().join("a.log")).unwrap(),
        );
        let sk = key();
        let svc = StandingGrantService::new(audit, sk.clone());
        svc.store.issue(an_envelope(&["send"])).unwrap();

        // Authentic intent ⇒ Some(verdict).
        let intent = an_intent(&sk, "send", "h1");
        assert_eq!(
            svc.authenticated_preview(&intent),
            Some(EnvelopeCheck::Allowed)
        );

        // Forge it (mutate a signed field so the signature no longer verifies) ⇒ None, fail-closed,
        // rejected on authenticity BEFORE any containment answer is given.
        let mut forged = intent.clone();
        forged.action = "admin".to_string();
        assert_eq!(svc.authenticated_preview(&forged), None);
    }

    // ── Durable mandates (G-M5): the registry + replay guard survive restart ──

    /// The core reboot invariant, all four legs on ONE store file: after a reopen, a LIVE mandate
    /// stays live, a REVOKED mandate stays dead (never crash-revived), an EXPIRED mandate reads
    /// inactive, and a dispatched intent id is STILL refused (the replay guard survives — G-M5).
    #[test]
    fn persistent_store_survives_reopen_live_dead_and_replay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("standing_grants.json");
        {
            let store = StandingGrantStore::with_persistence(&path).unwrap();
            store
                .issue(envelope_with("live-1", Some(SecureTimestamp::after_secs(3600))))
                .unwrap();
            store.issue(envelope_with("dead-1", None)).unwrap();
            assert!(store.revoke("dead-1").unwrap());
            store
                .issue(envelope_with("exp-1", Some(SecureTimestamp::after_secs(0))))
                .unwrap();
            let now = SecureTimestamp::now().unix_secs;
            assert!(store.record_fresh_intent("intent-once", now, now).unwrap());
        } // drop = the "restart"
        let store = StandingGrantStore::with_persistence(&path).unwrap();
        assert!(store.is_active("live-1"), "a live mandate survives reboot LIVE");
        assert!(
            !store.is_active("dead-1"),
            "a revoked mandate stays DEAD after reboot — never crash-revived"
        );
        assert!(
            store.get("dead-1").expect("still queryable").revoked,
            "the revoked record is retained as revoked, not vanished"
        );
        assert!(!store.is_active("exp-1"), "an expired mandate reloads inactive");
        let now = SecureTimestamp::now().unix_secs;
        assert!(
            !store.record_fresh_intent("intent-once", now, now).unwrap(),
            "the replay guard survives reboot: the same intent id is refused (G-M5)"
        );
        assert!(
            store.record_fresh_intent("intent-new", now, now).unwrap(),
            "fresh intents still register after reload"
        );
        assert_eq!(store.list().len(), 3, "every issued mandate is still listed");
    }

    /// The per-mandate RATE budget (G-M7): a mandate may perform up to the limit per window, then
    /// is refused; a NEW window resets it; and the budget is PER grant_id (one mandate's flood does
    /// not spend another's).
    #[test]
    fn dispatch_rate_budget_bounds_a_mandate_then_resets_next_window() {
        let store = StandingGrantStore::new();
        let t0 = 1_000_000u64;
        // Up to the limit is allowed within the window...
        for i in 0..MANDATE_DISPATCH_LIMIT {
            assert!(
                store.record_dispatch_within_budget("g1", t0),
                "act {i} within budget"
            );
        }
        // ...the next act in the same window is refused.
        assert!(
            !store.record_dispatch_within_budget("g1", t0),
            "over budget in the same window"
        );
        // A DIFFERENT mandate has its own budget — not spent by g1's flood.
        assert!(store.record_dispatch_within_budget("g2", t0), "g2 has its own budget");
        // A new window (past the window end) resets g1.
        let t1 = t0 + MANDATE_DISPATCH_WINDOW_SECS;
        assert!(
            store.record_dispatch_within_budget("g1", t1),
            "the budget resets in the next window"
        );
    }

    /// The rate map is BOUNDED even against a SAME-WINDOW flood of distinct grant_ids (the attack
    /// the council flagged: the across-window prune reclaims nothing inside one window, so a hard
    /// cap must refuse new keys at capacity). This is the ratchet reproducing that exact failure —
    /// every id shares one window `t0`, so nothing is ever stale, yet the map must stay bounded.
    #[test]
    fn dispatch_rate_map_is_bounded_against_distinct_grant_spam() {
        let store = StandingGrantStore::new();
        let t0 = 1_000u64;
        // Flood distinct grant_ids ALL WITHIN ONE WINDOW — none ever goes stale during the flood,
        // so the across-window prune cannot help; only the hard cap can bound this.
        for i in 0..(DISPATCH_RATE_SOFT_CAP * 4) {
            store.record_dispatch_within_budget(&format!("flood-{i}"), t0);
        }
        let n = store.dispatch_rate.read().map(|m| m.len()).unwrap_or(usize::MAX);
        assert!(
            n <= DISPATCH_RATE_SOFT_CAP + 1,
            "same-window distinct-grant_id flood is hard-capped at ~{DISPATCH_RATE_SOFT_CAP}, got {n}"
        );
        // The hard cap must NOT deny an ALREADY-SEEN grant_id (a real mandate keeps acting): a key
        // that already has an entry is still counted even while the map is at capacity.
        assert!(
            store.record_dispatch_within_budget("flood-0", t0),
            "an existing grant_id is still counted at capacity — a real mandate is never shed"
        );
        // A brand-new key at capacity IS refused (fail-closed memory bound).
        assert!(
            !store.record_dispatch_within_budget("brand-new-at-capacity", t0),
            "a new grant_id is refused while the map is at its hard cap"
        );
    }

    /// The freshness window (G-M7): a declaration expires (too old) and a future-dated one is
    /// refused; a just-declared one passes. Pure over unix seconds.
    #[test]
    fn freshness_window_rejects_stale_and_future_dated() {
        let now = 1_000_000u64;
        assert!(check_intent_freshness(now, now).is_ok(), "declared now → fresh");
        assert!(check_intent_freshness(now - MAX_INTENT_AGE_SECS, now).is_ok(), "at the age edge → fresh");
        assert_eq!(
            check_intent_freshness(now - MAX_INTENT_AGE_SECS - 1, now),
            Err(FreshnessError::Stale),
            "one second past the age → stale"
        );
        assert!(check_intent_freshness(now + MAX_CLOCK_SKEW_SECS, now).is_ok(), "at the skew edge → fresh");
        assert_eq!(
            check_intent_freshness(now + MAX_CLOCK_SKEW_SECS + 1, now),
            Err(FreshnessError::FutureDated),
            "one second past the skew → future-dated"
        );
    }

    /// The replay guard is BOUNDED, not monotonic (G-M7): recording a fresh intent COMPACTS ids
    /// whose declared_at has aged past the window — but a replay of a STILL-remembered id is caught
    /// BEFORE compaction, so bounding never opens a replay.
    #[test]
    fn seen_set_compacts_aged_ids_but_still_catches_in_window_replay() {
        let store = StandingGrantStore::new(); // memory-only is enough for the guard logic
        let base = 10_000_000u64;
        // An OLD intent recorded when "now" was `base`.
        assert!(store.record_fresh_intent("old", base, base).unwrap());
        // Much later, a fresh intent — its recording compacts "old" (aged past retention).
        let later = base + SEEN_INTENT_RETENTION_SECS + 10;
        assert!(store.record_fresh_intent("recent", later, later).unwrap());
        // "old" was forgotten (bounded), so re-recording it as if fresh-at-`later` succeeds — but in
        // production `check_intent_freshness` would reject a re-POST carrying old's real declared_at.
        assert!(
            store.record_fresh_intent("old", later, later).unwrap(),
            "an aged id is compacted out — the freshness gate is what stops its replay"
        );
        // A replay of a STILL-in-window id is refused (caught before compaction).
        assert!(
            !store.record_fresh_intent("recent", later, later).unwrap(),
            "an in-window id is still remembered → replay refused"
        );
    }

    /// The BACKWARD-CLOCK replay hole (red-team, Sprint 19) is CLOSED by the anti-readmit
    /// watermark: an intent evicted while the clock was high cannot be replayed after the clock
    /// rewinds into its freshness window. Reproduces the exact attack sequence.
    #[test]
    fn backward_clock_step_cannot_readmit_an_evicted_intent() {
        let store = StandingGrantStore::new();
        let d = 10_000_000u64; // captured intent X's declared_at
        // 1. X dispatched legitimately at wall-time ≈ d — remembered.
        assert!(store.record_fresh_intent("X", d, d).unwrap());
        // 2. Clock advances past d + retention; another dispatch compacts X out (and raises the
        //    watermark to ≥ d).
        let high = d + SEEN_INTENT_RETENTION_SECS + 100;
        assert!(store.record_fresh_intent("Y", high, high).unwrap());
        // 3. Clock steps BACKWARD to ≈ d (t2 within X's freshness window). Freshness would ACCEPT
        //    X here (that is the regression) — but the watermark refuses the readmit.
        let t2 = d; // check_intent_freshness(d, t2) == Ok — the dangerous case
        assert_eq!(check_intent_freshness(d, t2), Ok(()), "freshness alone would accept the replay");
        assert!(
            !store.record_fresh_intent("X", d, t2).unwrap(),
            "the watermark refuses X's replay regardless of the clock rewind — no double-act"
        );
        // A genuinely NEW, newer intent still acts (the watermark only blocks at/below evicted).
        assert!(store.record_fresh_intent("Z", high + 1, high + 1).unwrap());
    }

    /// The watermark survives restart (it is persisted), so the backward-clock hole stays closed
    /// across a reboot too.
    #[test]
    fn evicted_watermark_persists_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("standing_grants.json");
        let d = 20_000_000u64;
        {
            let store = StandingGrantStore::with_persistence(&path).unwrap();
            store.record_fresh_intent("X", d, d).unwrap();
            let high = d + SEEN_INTENT_RETENTION_SECS + 100;
            store.record_fresh_intent("Y", high, high).unwrap(); // evicts X, persists watermark
        } // restart
        let store = StandingGrantStore::with_persistence(&path).unwrap();
        assert!(
            !store.record_fresh_intent("X", d, d).unwrap(),
            "the persisted watermark still refuses X after a reboot"
        );
    }

    /// Even when the compacting write FAILS to persist, the bumped watermark is KEPT (not rolled
    /// back to the lower prev) — so an id evicted from memory during that failed call still cannot
    /// be replayed (re-verification of the F1 fix: restoring the low watermark would have reopened
    /// the hole under a persist failure). The evicted id is caught by the retained watermark before
    /// any persist is attempted.
    #[test]
    fn persist_failure_keeps_the_watermark_so_an_evicted_id_cannot_replay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("standing_grants.json");
        let d = 30_000_000u64;
        let store = StandingGrantStore::with_persistence(&path).unwrap();
        store.record_fresh_intent("X", d, d).unwrap();
        // Squat the temp path so the NEXT (compacting) persist fails.
        std::fs::create_dir(path.with_extension("tmp")).unwrap();
        let high = d + SEEN_INTENT_RETENTION_SECS + 100;
        assert!(
            store.record_fresh_intent("Y", high, high).is_err(),
            "the compacting write fails to persist"
        );
        // X was evicted in that failed call; a replay of X is refused by the RETAINED watermark
        // (the check runs before any persist, so it succeeds even while the dir is squatted).
        assert!(
            !store.record_fresh_intent("X", d, d).unwrap(),
            "the retained watermark still refuses the evicted id after a persist failure"
        );
    }

    /// A v1 snapshot (bare-string seen ids, no timestamps) MIGRATES on load — mandates AND the
    /// replay guard are preserved (never a fail-closed refusal that would drop them on upgrade).
    #[test]
    fn v1_snapshot_migrates_preserving_mandates_and_replay_guard() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("standing_grants.json");
        // A hand-written v1 file: one mandate + one seen intent, old bare-string format.
        std::fs::write(
            &path,
            r#"{"version":1,"grants":[{"grant_id":"g1","capsule":"vm-agent",
                "allowed_methods":["send"],"resource":"elastos://mail/send","action":"execute",
                "expires_at":null,"agent_pubkey":null,"revoked":false,"token_epoch":0}],
                "seen_intents":["legacy-intent"]}"#,
        )
        .unwrap();
        let store = StandingGrantStore::with_persistence(&path).unwrap();
        assert!(store.is_active("g1"), "the v1 mandate survives migration");
        let now = SecureTimestamp::now().unix_secs;
        assert!(
            !store.record_fresh_intent("legacy-intent", now, now).unwrap(),
            "the migrated replay guard still refuses the legacy intent id"
        );
        // And it is now written back as v2 (a fresh intent persists in the new format).
        assert!(store.record_fresh_intent("new-intent", now, now).unwrap());
        let reopened = StandingGrantStore::with_persistence(&path).unwrap();
        assert!(
            !reopened.record_fresh_intent("new-intent", now, now).unwrap(),
            "the v2 rewrite round-trips the guard"
        );
    }

    /// Fail-closed boot: a present-but-corrupt registry file is a loud error, never silently
    /// skipped (a skipped record could resurrect a revoked mandate or reopen a replay window).
    #[test]
    fn persistent_store_refuses_corrupt_state_at_boot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("standing_grants.json");
        std::fs::write(&path, "{not json").unwrap();
        let err = StandingGrantStore::with_persistence(&path)
            .err()
            .expect("corrupt mandate state must refuse to boot");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

        // Same for a future/unknown snapshot version — never guess at semantics.
        std::fs::write(&path, r#"{"version":99,"grants":[],"seen_intents":[]}"#).unwrap();
        let err = StandingGrantStore::with_persistence(&path)
            .err()
            .expect("unknown version must refuse to boot");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    /// Widen-proof reload (guardian F3): a snapshot edit that DROPS a narrowing `Option` key must
    /// refuse to load, never silently widen — a missing `agent_pubkey` would UNBIND an agent-bound
    /// mandate, a missing `expires_at` would immortalize it. An explicit `null` (what the runtime
    /// itself writes for None) still loads. Unknown fields also refuse (version-rollback safety).
    #[test]
    fn reload_refuses_dropped_option_keys_and_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("standing_grants.json");
        let base = |fields: &str| {
            format!(
                r#"{{"version":1,"grants":[{{"grant_id":"g1","capsule":"vm-agent",
                     "allowed_methods":["send"],"resource":"elastos://mail/send",
                     "action":"execute",{fields}"revoked":false,"token_epoch":0}}],
                     "seen_intents":[]}}"#
            )
        };
        // Both keys present (explicit null) — loads, honestly None.
        std::fs::write(&path, base(r#""expires_at":null,"agent_pubkey":null,"#)).unwrap();
        let store = StandingGrantStore::with_persistence(&path).unwrap();
        assert!(store.is_active("g1"));

        // agent_pubkey KEY dropped — refuses (would unbind an agent-bound mandate).
        std::fs::write(&path, base(r#""expires_at":null,"#)).unwrap();
        assert!(StandingGrantStore::with_persistence(&path).is_err());

        // expires_at KEY dropped — refuses (would immortalize the mandate).
        std::fs::write(&path, base(r#""agent_pubkey":null,"#)).unwrap();
        assert!(StandingGrantStore::with_persistence(&path).is_err());

        // An unknown field — refuses (a binary rollback must not silently drop semantics).
        std::fs::write(
            &path,
            base(r#""expires_at":null,"agent_pubkey":null,"future_narrowing_field":true,"#),
        )
        .unwrap();
        assert!(StandingGrantStore::with_persistence(&path).is_err());
    }

    /// Durable-before-visible: when the snapshot write FAILS, the mutation rolls back and the
    /// error surfaces — memory never runs ahead of disk. (Seam: a DIRECTORY squatting on the
    /// snapshot's temp path makes `File::create` fail — works even when the test runs as root,
    /// which ignores permission bits.)
    #[test]
    fn persist_failure_rolls_back_and_surfaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("standing_grants.json");
        let store = StandingGrantStore::with_persistence(&path).unwrap();
        store.issue(envelope_with("g1", None)).unwrap();

        // Squat a directory on the temp path so the NEXT snapshot write fails.
        let tmp_path = path.with_extension("tmp");
        std::fs::create_dir(&tmp_path).unwrap();

        assert!(store.issue(envelope_with("g2", None)).is_err(), "issue surfaces the failure");
        assert!(store.get("g2").is_none(), "the unpersistable mandate was NOT issued");
        assert!(store.revoke("g1").is_err(), "revoke surfaces the failure");
        assert!(store.is_active("g1"), "the unpersistable revoke did not half-apply");
        let now = SecureTimestamp::now().unix_secs;
        assert!(store.record_fresh_intent("i1", now, now).is_err(), "replay-guard write surfaces");
        // Clear the failure and verify the store still works (loud failure, re-runnable).
        std::fs::remove_dir(&tmp_path).unwrap();
        assert!(store.revoke("g1").unwrap(), "after the failure clears, the revoke lands");
        assert!(
            store.record_fresh_intent("i1", SecureTimestamp::now().unix_secs, SecureTimestamp::now().unix_secs).unwrap(),
            "the rolled-back intent id was not half-registered"
        );
    }
}
