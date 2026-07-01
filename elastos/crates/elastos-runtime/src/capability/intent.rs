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
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
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
        let mut intent = Self {
            schema: INTENT_DECLARATION_SCHEMA_V1.to_string(),
            intent_id: intent_id.to_string(),
            capsule: capsule.to_string(),
            method_id: method_id.to_string(),
            input_hash: input_hash.to_string(),
            resource: resource.to_string(),
            action: action.to_string(),
            standing_grant_id: standing_grant_id.to_string(),
            declared_at: SecureTimestamp::now(),
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
#[derive(Debug, Clone)]
pub struct StandingGrantEnvelope {
    pub grant_id: String,
    pub capsule: String,
    /// The method ids the envelope authorizes (a sorted set for stable, fast containment).
    pub allowed_methods: BTreeSet<String>,
    pub resource: String,
    pub action: String,
    /// Expiry; `None` = never expires (until revoked), mirroring `CapabilityToken::expiry`.
    pub expires_at: Option<SecureTimestamp>,
    pub revoked: bool,
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
    ) -> Self {
        StandingGrantEnvelope {
            grant_id: token.id().to_string(),
            capsule: token.capsule().to_string(),
            allowed_methods,
            resource: token.resource().to_string(),
            action: token.action().to_string(),
            expires_at: token.expiry().copied(),
            revoked,
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
    verifying_key.verify(digest, &signature).is_ok()
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
}

impl StandingGrantStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Issue (or replace) a standing grant, keyed by its `grant_id`. Issuing an envelope whose
    /// `revoked` flag is already set stores it as revoked (authorizes nothing) — issuing never
    /// silently un-revokes a grant.
    pub fn issue(&self, envelope: StandingGrantEnvelope) {
        let mut grants = match self.grants.write() {
            Ok(g) => g,
            // A poisoned lock can only mean a prior panic; every write is one statement, so the
            // map is structurally intact — recover the guard rather than drop the issuance.
            Err(poisoned) => poisoned.into_inner(),
        };
        grants.insert(envelope.grant_id.clone(), envelope);
    }

    /// Revoke a standing grant by id, fail-closed. Returns `true` iff a live (not-already-revoked)
    /// grant was revoked by THIS call — so a double-revoke or an unknown id returns `false`. The
    /// record is retained with `revoked = true` so the grant stays queryable as revoked.
    pub fn revoke(&self, grant_id: &str) -> bool {
        let mut grants = match self.grants.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        match grants.get_mut(grant_id) {
            Some(env) if !env.revoked => {
                env.revoked = true;
                true
            }
            _ => false,
        }
    }

    /// The standing grant for `grant_id`, if one was ever issued (revoked or not). The dispatcher
    /// runs it through the fail-closed gate, which denies a revoked/expired envelope — so returning
    /// a revoked envelope yields an honest, recorded `Revoked`/`Expired` denial, never a silent pass.
    /// A poisoned lock degrades to `None` (absent), never a fabricated grant.
    pub fn get(&self, grant_id: &str) -> Option<StandingGrantEnvelope> {
        let grants = self.grants.read().ok()?;
        grants.get(grant_id).cloned()
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

    /// Issue a standing grant derived from a REAL issued [`CapabilityToken`] (the cryptographic
    /// root): the token supplies capsule/resource/action/expiry, the caller supplies the authorized
    /// method set. Returns the `grant_id` (the token's id) to revoke or dispatch against later.
    pub fn issue_from_token(
        &self,
        token: &CapabilityToken,
        allowed_methods: BTreeSet<String>,
    ) -> String {
        let envelope = StandingGrantEnvelope::from_token(token, allowed_methods, false);
        let grant_id = envelope.grant_id.clone();
        self.store.issue(envelope);
        grant_id
    }

    /// Revoke a standing grant by id, fail-closed. Returns `true` iff a live grant was revoked by
    /// this call (double-revoke / unknown id → `false`).
    pub fn revoke(&self, grant_id: &str) -> bool {
        self.store.revoke(grant_id)
    }

    /// True iff an ACTIVE (issued, not revoked, not expired) grant exists for `grant_id`.
    pub fn is_active(&self, grant_id: &str) -> bool {
        self.store.is_active(grant_id)
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
        let env = StandingGrantEnvelope::from_token(&token, methods, false);

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
        let active = StandingGrantEnvelope::from_token(&token, methods.clone(), false);
        assert_eq!(active.expires_at, None);
        assert!(active.is_active(), "None expiry never expires");

        // The caller's external revocation check is honored, fail-closed.
        let revoked = StandingGrantEnvelope::from_token(&token, methods, true);
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
        }
    }

    #[test]
    fn store_issue_then_get_returns_the_envelope() {
        let store = StandingGrantStore::new();
        assert!(store.get("g1").is_none(), "an unissued grant is absent");
        assert!(!store.is_active("g1"), "an unissued grant is not active");

        store.issue(envelope_with("g1", Some(SecureTimestamp::after_secs(3600))));
        let got = store.get("g1").expect("issued grant is retrievable");
        assert_eq!(got.grant_id, "g1");
        assert!(!got.revoked);
        assert!(store.is_active("g1"), "a fresh, unexpired grant is active");
    }

    #[test]
    fn store_revoke_flips_the_flag_keeps_the_record_and_is_idempotent() {
        let store = StandingGrantStore::new();
        store.issue(envelope_with("g1", None)); // None expiry ⇒ never expires until revoked.
        assert!(store.is_active("g1"));

        assert!(store.revoke("g1"), "revoking a live grant returns true");
        // The record is KEPT, now marked revoked — queryable as revoked for honest denial.
        let got = store.get("g1").expect("a revoked grant is still queryable");
        assert!(got.revoked, "the stored envelope is marked revoked");
        assert!(!store.is_active("g1"), "a revoked grant is not active");

        // Idempotent: a second revoke (already revoked) returns false — no live grant was revoked.
        assert!(!store.revoke("g1"), "double-revoke returns false");
        // Revoking an unknown id is a fail-closed no-op, never a panic.
        assert!(!store.revoke("does-not-exist"));
    }

    #[test]
    fn store_never_un_revokes_and_expiry_deactivates_without_revocation() {
        let store = StandingGrantStore::new();

        // Issuing an already-revoked envelope stores it as revoked — issue never un-revokes.
        let mut revoked_env = envelope_with("g1", None);
        revoked_env.revoked = true;
        store.issue(revoked_env);
        assert!(
            !store.is_active("g1"),
            "an issued-revoked grant is not active"
        );
        assert!(store.get("g1").unwrap().revoked);

        // A past expiry deactivates a grant even though it was never revoked (fail-closed on time).
        store.issue(envelope_with("g2", Some(SecureTimestamp::after_secs(0))));
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
        store.issue(an_envelope(methods));
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
        assert!(store.revoke("grant-1"), "revoke the standing grant");
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
        );
        store.issue(envelope);

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
        assert!(store.revoke(&grant_id), "revoke the token's standing grant");
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
            svc.issue_from_token(&token, ["send"].iter().map(|m| m.to_string()).collect());
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
        assert!(svc.revoke(&grant_id));
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
        svc.store.issue(an_envelope(&["send"]));
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
        svc.store.issue(an_envelope(&["send"]));

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
}
