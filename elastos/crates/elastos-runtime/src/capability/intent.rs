//! Intent-proof loop — chunk 1: the two signed records + the pure verifier.
//!
//! The prover/verifier loop for agent *actions* (design: `docs/INTENT_PROOF_LOOP.md`).
//! An agent DECLARES what it intends to do ([`IntentDeclarationV1`]) before acting; the
//! runtime PROVES that intent is within a standing authorization
//! ([`check_intent_within_envelope`], fail-closed) before the act fires; and after the
//! act the declared-vs-done delta is recorded as a signed custody fact
//! ([`IntentReconciliationV1`] built from [`reconcile`]).
//!
//! This chunk is the crypto + pure logic only — the records sign/verify like
//! [`crate::capability::receipt::AffordanceGrantReceiptV1`], and the verifier is a pure
//! function with the full fail-closed branch matrix under test. EMISSION onto the audit
//! chain (new `AuditEvent` variants) and WIRING into the live dispatch path are the next
//! chunks; nothing here touches the act path yet.
//!
//! Scope boundary (carried from the design): this verifies CONTAINMENT + CUSTODY — that an
//! intent is within the authorized envelope, and a tamper-evident record of declared vs
//! done — NOT the correctness/wisdom of the act.

use std::collections::BTreeSet;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::capability::receipt::AffordanceGrantReceiptV1;
use crate::capability::token::CapabilityToken;
use crate::primitives::audit::AuditEvent;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> SigningKey {
        SigningKey::generate(&mut rand::thread_rng())
    }

    fn an_intent(sk: &SigningKey, method: &str, args_hash: &str) -> IntentDeclarationV1 {
        IntentDeclarationV1::issue(
            sk,
            sk.verifying_key().to_bytes(),
            "intent-1",
            "vm-agent",
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
}
