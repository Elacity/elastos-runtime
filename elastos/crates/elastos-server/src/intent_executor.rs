//! The "done" half of the mandate loop: performing a dispatched intent and REPORTING what was
//! actually done, so the reconciliation is an independent observation — not the declaration copied
//! back to itself.
//!
//! Before this, the dispatch act closure minted the affordance receipt from the intent's OWN
//! declared fields, so the reconciliation was structurally always `Matched` and an authorized-but-
//! unperformed intent still produced a "matched" receipt (KNOWN_GAPS G-M6). An [`IntentExecutor`]
//! fixes that: the runtime invokes a REAL executor for the declared method, and the receipt is
//! minted from the executor's [`IntentExecution::Performed`] report. A method with no registered
//! executor — or one that declines — yields [`IntentExecution::Declined`], which the gate reconciles
//! as `Undelivered`. So only an intent a real executor performed AS DECLARED reconciles as
//! `Matched`; a drifting executor `Diverges`; an unperformed one is `Undelivered`.

use std::collections::HashMap;
use std::sync::Arc;

use elastos_runtime::capability::IntentDeclarationV1;
use elastos_runtime::primitives::audit::AuditLog;

/// The canonical resource `runtime.audit_verify` actually reads: the runtime's whole audit chain.
/// The executor reports THIS (not the declared resource), so a mandate mis-scoped to some unrelated
/// resource reconciles `Diverged` (the runtime read the chain, not what was declared) instead of a
/// misleading `Matched` — the receipt names what was truly read.
pub const AUDIT_CHAIN_RESOURCE: &str = "elastos://runtime/audit-chain";

/// The resource namespace `runtime.content_seen` operates on: a content-ACCESS-CHECK reference of
/// the form `elastos://runtime/content-access/<content-id>`. A `content_seen` mandate is scoped to
/// THIS (not the bare content id), so the receipt's `CapabilityUse` — which carries the resource but
/// not the method — honestly reads as "a read of the access-CHECK for <id>", never as "a read of the
/// content itself". The executor answers a RUNTIME-level question (does the audit history record ANY
/// successful access to <id>), which the operator authorizes by granting the check mandate.
pub const CONTENT_ACCESS_CHECK_PREFIX: &str = "elastos://runtime/content-access/";

/// The resource namespace `runtime.notify` delivers into: an operator-Inbox TOPIC of the form
/// `elastos://runtime/inbox/<topic>`. A notify mandate is scoped to ONE topic (AUD-5-safe: a real
/// path segment, never a bare wildcard) with `action = message` — the receipt therefore reads as
/// "a message delivered to inbox topic <topic>", exactly what happened.
pub const INBOX_NOTIFY_PREFIX: &str = "elastos://runtime/inbox/";

/// The resource namespace `runtime.state_put` writes into: a durable agent-state KEY of the form
/// `elastos://runtime/store/<key>`. A state mandate is scoped to ONE key (AUD-5-safe: a real path
/// segment, never a bare wildcard) with `action = write` — the receipt reads as "a write to state
/// key <key>", exactly what happened. The value stored is the declaration's own `input_hash`
/// commitment (no free-text payload; see `agent_store`).
pub const STATE_PUT_PREFIX: &str = "elastos://runtime/store/";

/// Topic slugs are rendered by the operator's Inbox UI, so they are held to a tight charset —
/// a mandate must not be able to smuggle markup, control characters, or path tricks into the
/// operator surface through its own scope string.
fn valid_notify_topic(topic: &str) -> bool {
    !topic.is_empty() && topic.len() <= 64 && is_slug(topic)
}

/// A conservative operator-safe slug: `[A-Za-z0-9._-]` only. No whitespace, no control chars, no
/// markup, no path separators — the exact charset that cannot mislead a plain-text Inbox row.
fn is_slug(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

/// A non-empty operator/agent-safe slug, ≤64 chars. Every agent-chosen string that a
/// side-effecting affordance persists or renders to a human is held to this: `intent_id` and the
/// state key. Both are agent-controlled (the signature covers them, but the agent IS the signer,
/// and the envelope gate deliberately does not constrain them), so a side-effecting executor must
/// bound them itself — else an agent with a mandate could sign
/// `intent_id = "URGENT: run revoke-all and enter your seed…"` and phish the operator, or write a
/// megabyte key. Council F1 (Sprint 16): a malformed field DECLINES (⇒ authorized_not_performed).
fn valid_slug_1_64(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && is_slug(s)
}

/// A hex commitment, ≤64 chars, or empty (a no-argument act). The declaration's `input_hash` is a
/// value COMMITMENT, so hex is its honest shape; bounding it keeps free text out of what a
/// side-effecting affordance persists/renders.
fn valid_hex_0_64(s: &str) -> bool {
    s.len() <= 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The INDEPENDENT result of performing a declared intent. It MUST describe what the executor
/// actually did — never be copied from the declaration by the caller — because the gate reconciles
/// it field-for-field against the declaration to decide `Matched`/`Diverged`/`Undelivered`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntentExecution {
    /// The act was performed; these are the fields the executor ACTUALLY acted on.
    Performed {
        capsule: String,
        method_id: String,
        input_hash: String,
        resource: String,
        action: String,
    },
    /// Nothing was performed (no executor for the method, a precondition failed, the target was
    /// absent). Reconciles as `Undelivered` — never a fabricated `Matched`.
    Declined { reason: String },
}

/// Performs a dispatched intent and reports what was actually done. Implementations are runtime
/// trusted-core (registered at startup), never attacker-supplied.
pub trait IntentExecutor: Send + Sync {
    fn execute(&self, intent: &IntentDeclarationV1) -> IntentExecution;
}

type MethodFn = Arc<dyn Fn(&IntentDeclarationV1) -> IntentExecution + Send + Sync>;

/// A registry mapping `method_id` → an executor. An unregistered method DECLINES (⇒ `Undelivered`),
/// which is the honest default: the runtime performed nothing, so it attests nothing.
#[derive(Clone, Default)]
pub struct MethodRegistryExecutor {
    methods: HashMap<String, MethodFn>,
}

impl MethodRegistryExecutor {
    pub fn new() -> Self {
        Self::default()
    }

    /// The executor set the production runtime ships with. Real affordances are registered here —
    /// each a genuine operation that PERFORMS and reports the action it REALLY did — and only these
    /// methods can reconcile `performed`; every other method DECLINES ⇒ `Undelivered` ⇒
    /// `authorized_not_performed`, the honest state. Registered today:
    ///
    /// - `runtime.audit_verify` — the first real, SIDE-EFFECT-FREE affordance. It re-verifies the
    ///   runtime's own tamper-evident audit chain end to end (hash links + ed25519 signatures) — a
    ///   pure read — and `Performed`s iff the chain actually verifies, `Declined`s otherwise. So the
    ///   outcome tracks REAL chain state, not the declaration: a corrupt or memory-only log is
    ///   honestly `Undelivered`. It reports `action = "read"` (what it truly did), so it is usable
    ///   only under a `read` mandate.
    /// - `runtime.content_seen` — a state-DEPENDENT read: does the audit history record a successful
    ///   access (ContentFetch/ContentOpen) to the mandate's `resource` (a content id)? `Performed`s
    ///   iff yes, `Declined`s iff not — so the SAME intent reconciles `performed` or
    ///   `authorized_not_performed` depending on real runtime state, not the declaration. Unlike
    ///   audit_verify the operation is PARAMETERIZED by the declared resource (it searches for that
    ///   id), so echoing it is honest. Reports `action = "read"`.
    /// - `runtime.notify` — the first SIDE-EFFECTING affordance: deliver a message about the act
    ///   into the operator's Inbox (the shell's Inbox app renders it). Registered ONLY when the
    ///   runtime has a `notify_data_dir` (the Inbox store lives there) — without one the method is
    ///   unwired ⇒ `Undelivered`, never a fabricated delivery. `Performed` iff the notification
    ///   write actually LANDED (atomic store write returned Ok); a failed write `Declined`s with
    ///   the true reason. The message content is a FIXED shape built from the signed declaration's
    ///   own fields (no free-text channel — nothing reaches the operator surface that the intent
    ///   signature does not cover), and the topic slug is charset-checked so a mandate's scope
    ///   string cannot smuggle markup into the Inbox. Reports `action = "message"` — usable only
    ///   under a `message` mandate.
    /// - `runtime.state_put` — the SECOND side-effecting affordance: write a durable, readable-back
    ///   agent-state key (`elastos://runtime/store/<key>`), principal-scoped to the acting capsule.
    ///   The stored value is the declaration's `input_hash` COMMITMENT (no free-text payload), so
    ///   nothing is persisted that the intent signature + mandate receipt do not already bind. Same
    ///   discipline as notify: registered only with a `data_dir`; key + input_hash bounded to safe
    ///   shapes before the write; `Performed` iff the atomic write LANDED, else `Declined`. Reports
    ///   `action = "write"` — usable only under a `write` mandate.
    /// - `runtime.state_get` — the READ side of that KV (the pair of state_put): a PRINCIPAL-SCOPED
    ///   ATTESTED read of `elastos://runtime/store/<key>`. The declared `input_hash` is the value
    ///   the agent EXPECTS; `Performed` echoes the ACTUAL stored value-hash, so the read reconciles
    ///   `Matched` iff the key holds that value (a provable "K = V"), `Diverged` (with the real
    ///   value in the receipt) if it holds a different one, and `Declined` (⇒ authorized_not_performed)
    ///   if the key is absent. Keyed on the acting capsule ⇒ an agent reads only its OWN state.
    ///   Reports `action = "read"` — usable only under a `read` mandate. Registered with a `data_dir`.
    ///
    /// The side-effecting affordances + state_get need the runtime data dir (their stores live under
    /// it); a `None` data dir leaves them honestly unwired ⇒ `Undelivered`.
    pub fn production(audit_log: Arc<AuditLog>, data_dir: Option<std::path::PathBuf>) -> Self {
        let mut registry = Self::new();
        if let Some(data_dir) = data_dir {
            let state_dir = data_dir.clone();
            let state_get_dir = data_dir.clone();
            registry.register(
                "runtime.state_put",
                Arc::new(move |intent: &IntentDeclarationV1| {
                    // The mandate is scoped to a state KEY; the key is the suffix. Outside the
                    // namespace ⇒ Decline.
                    let Some(key) = intent.resource.strip_prefix(STATE_PUT_PREFIX) else {
                        return IntentExecution::Declined {
                            reason: format!("state_put resource must be {STATE_PUT_PREFIX}<key>"),
                        };
                    };
                    // The key is a durable identifier and appears in operator/agent read-backs, and
                    // the value is a COMMITMENT (hex hash) — bound both before the write, so nothing
                    // free-form is persisted under the mandate.
                    if !valid_slug_1_64(key) {
                        return IntentExecution::Declined {
                            reason: "state_put key must be 1-64 chars of [A-Za-z0-9._-]".to_string(),
                        };
                    }
                    if !valid_hex_0_64(&intent.input_hash) {
                        return IntentExecution::Declined {
                            reason: "state_put value (input_hash) must be <=64 hex chars (or empty)"
                                .to_string(),
                        };
                    }
                    // The intent_id is PERSISTED in the entry (attribution), so it is bounded to a
                    // slug like every other agent-chosen string a side-effecting affordance stores —
                    // no unbounded per-entry payload smuggled through the id.
                    if !valid_slug_1_64(&intent.intent_id) {
                        return IntentExecution::Declined {
                            reason: "state_put intent_id must be 1-64 chars of [A-Za-z0-9._-]"
                                .to_string(),
                        };
                    }
                    // The REAL side effect: persist the key. Performed only after the write lands.
                    match crate::agent_store::put_agent_state(
                        &state_dir,
                        &intent.capsule,
                        key,
                        &intent.input_hash,
                        &intent.standing_grant_id,
                        &intent.intent_id,
                    ) {
                        Ok(_version) => IntentExecution::Performed {
                            capsule: intent.capsule.clone(),
                            method_id: intent.method_id.clone(),
                            // The declared value-hash is genuinely CONSUMED — it is what was written
                            // as the key's value — so echoing it is honest.
                            input_hash: intent.input_hash.clone(),
                            // The key actually written, and the action actually performed: a write.
                            resource: intent.resource.clone(),
                            action: "write".to_string(),
                        },
                        Err(e) => IntentExecution::Declined {
                            reason: format!("state_put could not be persisted: {e}"),
                        },
                    }
                }),
            );
            registry.register(
                "runtime.state_get",
                Arc::new(move |intent: &IntentDeclarationV1| {
                    // The READ side of the agent-state KV (Sprint 25) — same store namespace as
                    // state_put; the key is the suffix. Outside the namespace ⇒ Decline.
                    let Some(key) = intent.resource.strip_prefix(STATE_PUT_PREFIX) else {
                        return IntentExecution::Declined {
                            reason: format!("state_get resource must be {STATE_PUT_PREFIX}<key>"),
                        };
                    };
                    if !valid_slug_1_64(key) {
                        return IntentExecution::Declined {
                            reason: "state_get key must be 1-64 chars of [A-Za-z0-9._-]".to_string(),
                        };
                    }
                    // An ATTESTED read (like content_seen's boolean check): the declared input_hash
                    // is the value the agent EXPECTS the key to hold, bounded to the same commitment
                    // shape state_put wrote. A read that reconciles Matched PROVES "key K holds V".
                    if !valid_hex_0_64(&intent.input_hash) {
                        return IntentExecution::Declined {
                            reason: "state_get expected-value (input_hash) must be <=64 hex chars (or empty)"
                                .to_string(),
                        };
                    }
                    // PRINCIPAL-SCOPED: get_agent_state keys on the acting capsule, so an agent can
                    // only ever read its OWN state — never another principal's (the per-capsule
                    // isolation the operator-facing list deliberately does NOT have).
                    match crate::agent_store::get_agent_state(&state_get_dir, &intent.capsule, key) {
                        Ok(Some(entry)) => IntentExecution::Performed {
                            capsule: intent.capsule.clone(),
                            method_id: intent.method_id.clone(),
                            // Echo the ACTUAL stored value-hash. Reconcile Matches iff the agent
                            // declared it correctly (an attested "K = V"); otherwise Diverges AND
                            // the receipt carries the real value — so the agent still LEARNS the
                            // true value on a mismatch, honestly, never a misleading Matched.
                            input_hash: entry.value_hash,
                            resource: intent.resource.clone(),
                            action: "read".to_string(),
                        },
                        // No such key for this principal ⇒ authorized-but-not-performed (honest:
                        // there is nothing to read), never a fabricated empty value.
                        Ok(None) => IntentExecution::Declined {
                            reason: format!("no state for {}/{key}", intent.capsule),
                        },
                        Err(e) => IntentExecution::Declined {
                            reason: format!("state_get could not read the store: {e}"),
                        },
                    }
                }),
            );
            registry.register(
                "runtime.notify",
                Arc::new(move |intent: &IntentDeclarationV1| {
                    // The mandate is scoped to an inbox TOPIC; the topic is the suffix. A resource
                    // outside this namespace is not a notify target ⇒ Decline.
                    let Some(topic) = intent.resource.strip_prefix(INBOX_NOTIFY_PREFIX) else {
                        return IntentExecution::Declined {
                            reason: format!(
                                "notify resource must be {INBOX_NOTIFY_PREFIX}<topic>"
                            ),
                        };
                    };
                    if !valid_notify_topic(topic) {
                        return IntentExecution::Declined {
                            reason: "notify topic must be 1-64 chars of [A-Za-z0-9._-]".to_string(),
                        };
                    }
                    // Council F1: the intent_id + input_hash reach the OPERATOR's Inbox body, so
                    // they are bounded to operator-safe shapes BEFORE delivery — a malformed field
                    // declines rather than smuggling free text into the operator's trust surface.
                    if !valid_slug_1_64(&intent.intent_id) {
                        return IntentExecution::Declined {
                            reason: "notify intent_id must be 1-64 chars of [A-Za-z0-9._-]"
                                .to_string(),
                        };
                    }
                    if !valid_hex_0_64(&intent.input_hash) {
                        return IntentExecution::Declined {
                            reason: "notify input_hash must be <=64 hex chars (or empty)".to_string(),
                        };
                    }
                    // The REAL side effect: land the row in the operator's Inbox store. Performed
                    // is reported ONLY after the atomic write returns Ok — a failed delivery is a
                    // Declined (⇒ authorized_not_performed), never a claimed message.
                    match crate::notifications::post_agent_act_notification(
                        &data_dir,
                        &intent.intent_id,
                        &intent.capsule,
                        topic,
                        &intent.input_hash,
                        &intent.standing_grant_id,
                    ) {
                        Ok(()) => IntentExecution::Performed {
                            capsule: intent.capsule.clone(),
                            method_id: intent.method_id.clone(),
                            // The declared input hash is genuinely CONSUMED — it is written into
                            // the delivered notification body — so echoing it is honest (the same
                            // way content_seen echoes the resource it actually searched).
                            input_hash: intent.input_hash.clone(),
                            // The topic actually delivered to, and the action actually performed:
                            // a message. A mandate scoped elsewhere, or a non-message action,
                            // reconciles Diverged, never a misleading Matched.
                            resource: intent.resource.clone(),
                            action: "message".to_string(),
                        },
                        Err(e) => IntentExecution::Declined {
                            reason: format!("notification could not be delivered: {e}"),
                        },
                    }
                }),
            );
        }
        let content_log = audit_log.clone();
        registry.register(
            "runtime.content_seen",
            Arc::new(move |intent: &IntentDeclarationV1| {
                // The mandate is scoped to a content-ACCESS-CHECK resource; the content id is the
                // suffix. A resource outside this namespace is not a content_seen target ⇒ Decline.
                let Some(content_id) = intent.resource.strip_prefix(CONTENT_ACCESS_CHECK_PREFIX)
                else {
                    return IntentExecution::Declined {
                        reason: format!(
                            "content_seen resource must be {CONTENT_ACCESS_CHECK_PREFIX}<content-id>"
                        ),
                    };
                };
                // Same evidentiary bar as audit_verify: the log must be SIGNED and the chain must
                // VERIFY, so a matched ContentOpen is a signature-attested record an offline editor
                // could not have forged. Then answer PRINCIPAL-SCOPED: did THIS capsule open it?
                let verifying_key = content_log
                    .verifying_key_hex()
                    .and_then(|hex_key| hex::decode(hex_key).ok())
                    .and_then(|bytes| <[u8; 32]>::try_from(bytes.as_slice()).ok())
                    .and_then(|arr| ed25519_dalek::VerifyingKey::from_bytes(&arr).ok());
                let Some(verifying_key) = verifying_key else {
                    return IntentExecution::Declined {
                        reason: "audit chain is unsigned; cannot attest a verified access".to_string(),
                    };
                };
                if content_log.verify_chain(Some(&verifying_key)).is_err() {
                    return IntentExecution::Declined {
                        reason: "audit chain did not verify".to_string(),
                    };
                }
                if content_log.principal_opened_content(&intent.capsule, content_id) {
                    IntentExecution::Performed {
                        capsule: intent.capsule.clone(),
                        method_id: intent.method_id.clone(),
                        input_hash: String::new(), // the search key IS the resource; no other args
                        // The access-CHECK resource actually searched (== declared: parameterized by
                        // it), and the action performed (a read of the audit history). The receipt
                        // therefore names a read of the CHECK, never of the content bytes.
                        resource: intent.resource.clone(),
                        action: "read".to_string(),
                    }
                } else {
                    IntentExecution::Declined {
                        reason: format!("{} did not open {content_id}", intent.capsule),
                    }
                }
            }),
        );
        registry.register(
            "runtime.audit_verify",
            Arc::new(move |intent: &IntentDeclarationV1| {
                // Require a SIGNING KEY: `verify_chain(None)` is a hash-links-only walk, and
                // `record_hash` is a public algorithm — an offline editor could rewrite an unsigned
                // chain and pass. So a `performed` audit_verify must mean SIGNATURE-verified: with no
                // key (memory-only/unsigned log) we Decline rather than over-claim tamper-evidence.
                let verifying_key = audit_log
                    .verifying_key_hex()
                    .and_then(|hex_key| hex::decode(hex_key).ok())
                    .and_then(|bytes| <[u8; 32]>::try_from(bytes.as_slice()).ok())
                    .and_then(|arr| ed25519_dalek::VerifyingKey::from_bytes(&arr).ok());
                let Some(verifying_key) = verifying_key else {
                    return IntentExecution::Declined {
                        reason: "audit chain is unsigned; cannot attest a signature-verified read"
                            .to_string(),
                    };
                };
                match audit_log.verify_chain(Some(&verifying_key)) {
                    Ok(_verified_count) => IntentExecution::Performed {
                        // capsule + method_id are the bound identity the executor acted as/under
                        // (the gate already tied them to the mandate); the fields below are what the
                        // runtime REALLY did, reported independently of the declaration:
                        capsule: intent.capsule.clone(),
                        method_id: intent.method_id.clone(),
                        // audit_verify consumes NO arguments, so the honest args-hash is empty. An
                        // intent that declared some other input_hash reconciles `Diverged`.
                        input_hash: String::new(),
                        // The resource actually read (the whole chain) and the action actually
                        // performed (read) — a mandate scoped elsewhere, or a non-read action,
                        // therefore reconciles `Diverged`, never a misleading `Matched`.
                        resource: AUDIT_CHAIN_RESOURCE.to_string(),
                        action: "read".to_string(),
                    },
                    Err(reason) => IntentExecution::Declined {
                        reason: format!("audit chain did not verify: {reason}"),
                    },
                }
            }),
        );
        registry
    }

    pub fn register(&mut self, method_id: &str, executor: MethodFn) {
        self.methods.insert(method_id.to_string(), executor);
    }
}

impl IntentExecutor for MethodRegistryExecutor {
    fn execute(&self, intent: &IntentDeclarationV1) -> IntentExecution {
        match self.methods.get(&intent.method_id) {
            Some(executor) => executor(intent),
            None => IntentExecution::Declined {
                reason: format!("no executor registered for method {}", intent.method_id),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent(method: &str) -> IntentDeclarationV1 {
        let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        IntentDeclarationV1::issue(
            &sk,
            sk.verifying_key().to_bytes(),
            "i1",
            "vm-agent",
            method,
            "cafe",
            "elastos://pay/vendor",
            "write",
            "grant-1",
        )
    }

    #[test]
    fn registered_method_performs_and_reports_its_fields() {
        let mut reg = MethodRegistryExecutor::new();
        reg.register(
            "demo.read",
            Arc::new(|i: &IntentDeclarationV1| IntentExecution::Performed {
                capsule: i.capsule.clone(),
                method_id: i.method_id.clone(),
                input_hash: i.input_hash.clone(),
                resource: i.resource.clone(),
                action: i.action.clone(),
            }),
        );
        match reg.execute(&intent("demo.read")) {
            IntentExecution::Performed { method_id, resource, .. } => {
                assert_eq!(method_id, "demo.read");
                assert_eq!(resource, "elastos://pay/vendor");
            }
            other => panic!("expected Performed, got {other:?}"),
        }
    }

    #[test]
    fn production_declines_unwired_methods_and_performs_the_real_audit_read() {
        let dir = tempfile::tempdir().unwrap();
        let log = Arc::new(AuditLog::with_file(dir.path().join("audit.log")).unwrap());
        log.emit(elastos_runtime::primitives::audit::AuditEvent::RuntimeStart {
            timestamp: elastos_common::SecureTimestamp::now(),
            version: "t".to_string(),
        })
        .unwrap();
        let reg = MethodRegistryExecutor::production(log, None);
        // Unwired methods decline (⇒ Undelivered), never a fabricated match.
        assert!(matches!(
            reg.execute(&intent("pay.invoke")),
            IntentExecution::Declined { .. }
        ));
        // The real affordance PERFORMS against a signed, verifiable chain and reports action=read.
        match reg.execute(&intent("runtime.audit_verify")) {
            IntentExecution::Performed { action, .. } => assert_eq!(action, "read"),
            other => panic!("expected Performed, got {other:?}"),
        }
    }

    #[test]
    fn audit_verify_declines_on_a_memory_only_chain() {
        // Real state drives the outcome: a memory-only log has nothing durable to verify ⇒ Declined,
        // never a fabricated "performed".
        let reg = MethodRegistryExecutor::production(Arc::new(AuditLog::new()), None);
        assert!(matches!(
            reg.execute(&intent("runtime.audit_verify")),
            IntentExecution::Declined { .. }
        ));
    }

    fn notify_intent(resource: &str, capsule: &str, args: &str) -> IntentDeclarationV1 {
        let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        IntentDeclarationV1::issue(
            &sk,
            sk.verifying_key().to_bytes(),
            "notify-1",
            capsule,
            "runtime.notify",
            args,
            resource,
            "message",
            "grant-1",
        )
    }

    /// The first side-effecting affordance: `runtime.notify` PERFORMS iff the notification
    /// actually LANDS in the operator's Inbox store — and the delivered row is real, readable
    /// state (visible to the Inbox app via `load_summary`), not a claim.
    #[test]
    fn notify_delivers_a_real_inbox_notification_and_reports_message() {
        let dir = tempfile::tempdir().unwrap();
        let reg = MethodRegistryExecutor::production(
            Arc::new(AuditLog::new()),
            Some(dir.path().to_path_buf()),
        );
        let resource = format!("{INBOX_NOTIFY_PREFIX}agent-status");
        match reg.execute(&notify_intent(&resource, "vm-agent", "cafe")) {
            IntentExecution::Performed { action, resource: r, input_hash, .. } => {
                assert_eq!(action, "message", "the act performed IS a message");
                assert_eq!(r, resource, "delivered to the declared topic");
                assert_eq!(input_hash, "cafe", "the consumed input hash is reported");
            }
            other => panic!("expected Performed, got {other:?}"),
        }
        // The side effect is REAL: the Inbox summary shows the delivered row.
        let summary = crate::notifications::load_summary(dir.path()).unwrap();
        assert_eq!(summary.unread_count, 1, "one unread notification landed");
        let entry = &summary.entries[0];
        assert_eq!(entry.kind, crate::notifications::AGENT_ACT_KIND);
        assert!(entry.title.contains("agent-status"), "title names the topic");
        assert!(entry.body.contains("vm-agent"), "body names the acting capsule");
        assert!(entry.body.contains("grant-1"), "body names the mandate");
        assert!(entry.body.contains("cafe"), "body carries the input-hash commitment");
    }

    /// Fail-closed scoping: outside the inbox namespace, or with a topic that could smuggle
    /// content into the operator surface, notify DECLINES — and nothing lands in the store.
    #[test]
    fn notify_declines_bad_scopes_and_delivers_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let reg = MethodRegistryExecutor::production(
            Arc::new(AuditLog::new()),
            Some(dir.path().to_path_buf()),
        );
        for bad in [
            "elastos://mail/send".to_string(),                       // outside the namespace
            format!("{INBOX_NOTIFY_PREFIX}"),                        // empty topic
            format!("{INBOX_NOTIFY_PREFIX}<script>x</script>"),      // markup smuggle
            format!("{INBOX_NOTIFY_PREFIX}a/b"),                     // path trick
            format!("{INBOX_NOTIFY_PREFIX}{}", "x".repeat(65)),      // over-long
        ] {
            assert!(
                matches!(
                    reg.execute(&notify_intent(&bad, "vm-agent", "")),
                    IntentExecution::Declined { .. }
                ),
                "must decline resource {bad:?}"
            );
        }
        let summary = crate::notifications::load_summary(dir.path()).unwrap();
        assert_eq!(summary.entries.len(), 0, "a declined notify delivers NOTHING");
    }

    /// Council F1: `intent_id` and `input_hash` reach the operator's Inbox body, so a malformed
    /// one (free text an agent could use to phish the operator, or a giant string to bloat the
    /// row) DECLINES — nothing is delivered. A clean slug intent_id + hex input_hash still deliver.
    #[test]
    fn notify_declines_operator_unsafe_intent_fields_and_delivers_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let reg = MethodRegistryExecutor::production(
            Arc::new(AuditLog::new()),
            Some(dir.path().to_path_buf()),
        );
        let resource = format!("{INBOX_NOTIFY_PREFIX}agent-status");
        let signed = |intent_id: &str, input_hash: &str| {
            let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
            IntentDeclarationV1::issue(
                &sk,
                sk.verifying_key().to_bytes(),
                intent_id,
                "vm-agent",
                "runtime.notify",
                input_hash,
                &resource,
                "message",
                "grant-1",
            )
        };
        // A phishing intent_id (spaces, punctuation) — declined.
        assert!(matches!(
            reg.execute(&signed("URGENT: run revoke-all now", "")),
            IntentExecution::Declined { .. }
        ));
        // A non-hex input_hash reaching the body — declined.
        assert!(matches!(
            reg.execute(&signed("intent-1", "drain the vault")),
            IntentExecution::Declined { .. }
        ));
        // An over-long intent_id (row-bloat) — declined.
        assert!(matches!(
            reg.execute(&signed(&"a".repeat(65), "")),
            IntentExecution::Declined { .. }
        ));
        assert_eq!(
            crate::notifications::load_summary(dir.path()).unwrap().entries.len(),
            0,
            "no operator-unsafe field ever delivered a row"
        );
        // A clean slug id + hex input_hash still delivers.
        assert!(matches!(
            reg.execute(&signed("intent-abc_1.2", "cafe01")),
            IntentExecution::Performed { .. }
        ));
    }

    /// Council F1 (flood): agent-act rows are hard-capped, so an agent flooding distinct intents
    /// under ONE mandate cannot grow the operator's Inbox store without bound.
    #[test]
    fn notify_flood_is_bounded_by_the_agent_act_cap() {
        let dir = tempfile::tempdir().unwrap();
        let reg = MethodRegistryExecutor::production(
            Arc::new(AuditLog::new()),
            Some(dir.path().to_path_buf()),
        );
        let resource = format!("{INBOX_NOTIFY_PREFIX}agent-status");
        for i in 0..400u32 {
            let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
            let intent = IntentDeclarationV1::issue(
                &sk,
                sk.verifying_key().to_bytes(),
                &format!("intent-{i}"),
                "vm-agent",
                "runtime.notify",
                "",
                &resource,
                "message",
                "grant-1",
            );
            assert!(matches!(reg.execute(&intent), IntentExecution::Performed { .. }));
        }
        let summary = crate::notifications::load_summary(dir.path()).unwrap();
        assert!(
            summary.entries.len() <= 256,
            "agent-act rows are capped at 256, got {}",
            summary.entries.len()
        );
    }

    fn state_put_intent(resource: &str, capsule: &str, value_hash: &str) -> IntentDeclarationV1 {
        let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        IntentDeclarationV1::issue(
            &sk,
            sk.verifying_key().to_bytes(),
            "state-intent-1",
            capsule,
            "runtime.state_put",
            value_hash,
            resource,
            "write",
            "grant-1",
        )
    }

    /// The SECOND side-effecting affordance: `runtime.state_put` PERFORMS iff the durable write
    /// LANDS, and the written value is readable back — a real, observable mutation, principal-scoped.
    #[test]
    fn state_put_writes_durable_readable_state_and_reports_write() {
        let dir = tempfile::tempdir().unwrap();
        let reg = MethodRegistryExecutor::production(
            Arc::new(AuditLog::new()),
            Some(dir.path().to_path_buf()),
        );
        let resource = format!("{STATE_PUT_PREFIX}cursor");
        match reg.execute(&state_put_intent(&resource, "vm-agent", "cafe01")) {
            IntentExecution::Performed { action, resource: r, input_hash, .. } => {
                assert_eq!(action, "write", "the act performed IS a write");
                assert_eq!(r, resource);
                assert_eq!(input_hash, "cafe01");
            }
            other => panic!("expected Performed, got {other:?}"),
        }
        // The side effect is REAL and readable back — principal-scoped to the acting capsule.
        let got = crate::agent_store::get_agent_state(dir.path(), "vm-agent", "cursor")
            .unwrap()
            .expect("the written key is readable back");
        assert_eq!(got.value_hash, "cafe01");
        assert_eq!(got.grant_id, "grant-1");
        // A DIFFERENT capsule cannot read it — no cross-principal state leak.
        assert!(crate::agent_store::get_agent_state(dir.path(), "vm-other", "cursor")
            .unwrap()
            .is_none());
    }

    /// Fail-closed scoping: outside the store namespace, or with a key/value that could smuggle
    /// free text into durable state, state_put DECLINES — and nothing is persisted.
    #[test]
    fn state_put_declines_bad_scopes_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let reg = MethodRegistryExecutor::production(
            Arc::new(AuditLog::new()),
            Some(dir.path().to_path_buf()),
        );
        for (resource, value) in [
            ("elastos://mail/send".to_string(), "aa".to_string()), // outside namespace
            (format!("{STATE_PUT_PREFIX}"), "aa".to_string()),     // empty key
            (format!("{STATE_PUT_PREFIX}a/b"), "aa".to_string()),  // path trick
            (format!("{STATE_PUT_PREFIX}k"), "not hex".to_string()), // free-text value
            (format!("{STATE_PUT_PREFIX}{}", "x".repeat(65)), "aa".to_string()), // over-long key
        ] {
            assert!(
                matches!(
                    reg.execute(&state_put_intent(&resource, "vm-agent", &value)),
                    IntentExecution::Declined { .. }
                ),
                "must decline resource {resource:?} value {value:?}"
            );
        }
        assert!(
            crate::agent_store::get_agent_state(dir.path(), "vm-agent", "k").unwrap().is_none(),
            "a declined state_put persists NOTHING"
        );
    }

    /// The PERSISTED intent_id is bounded like every other agent-chosen stored string (council
    /// carry-over): a giant/free-form intent_id declines rather than bloating durable state.
    #[test]
    fn state_put_declines_an_unbounded_intent_id() {
        let dir = tempfile::tempdir().unwrap();
        let reg = MethodRegistryExecutor::production(
            Arc::new(AuditLog::new()),
            Some(dir.path().to_path_buf()),
        );
        let resource = format!("{STATE_PUT_PREFIX}cursor");
        let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        let intent = IntentDeclarationV1::issue(
            &sk,
            sk.verifying_key().to_bytes(),
            &"z".repeat(65), // over-long intent_id
            "vm-agent",
            "runtime.state_put",
            "cafe01",
            &resource,
            "write",
            "grant-1",
        );
        assert!(matches!(reg.execute(&intent), IntentExecution::Declined { .. }));
        assert!(
            crate::agent_store::get_agent_state(dir.path(), "vm-agent", "cursor").unwrap().is_none(),
            "a declined write persists nothing"
        );
    }

    /// Without a data dir there is no store to write into — state_put is honestly UNWIRED.
    #[test]
    fn state_put_is_unwired_without_a_data_dir() {
        let reg = MethodRegistryExecutor::production(Arc::new(AuditLog::new()), None);
        let resource = format!("{STATE_PUT_PREFIX}cursor");
        assert!(matches!(
            reg.execute(&state_put_intent(&resource, "vm-agent", "cafe01")),
            IntentExecution::Declined { .. }
        ));
    }

    fn state_get_intent(resource: &str, capsule: &str, expected: &str) -> IntentDeclarationV1 {
        let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        IntentDeclarationV1::issue(
            &sk,
            sk.verifying_key().to_bytes(),
            "state-get-1",
            capsule,
            "runtime.state_get",
            expected, // the value the agent EXPECTS (input_hash)
            resource,
            "read",
            "grant-1",
        )
    }

    /// Sprint 25: `runtime.state_get` is the READ side of the KV. It echoes the ACTUAL stored
    /// value-hash (so the read reconciles Matched only when the agent declared the right value — an
    /// attested "K = V" — proven end-to-end in the handler tests), Declines an absent key, and is
    /// PRINCIPAL-SCOPED (an agent reads only its own state).
    #[test]
    fn state_get_reads_back_own_state_attested_and_principal_scoped() {
        let dir = tempfile::tempdir().unwrap();
        let reg = MethodRegistryExecutor::production(
            Arc::new(AuditLog::new()),
            Some(dir.path().to_path_buf()),
        );
        let resource = format!("{STATE_PUT_PREFIX}cursor");
        // Seed the store via the real state_put affordance.
        assert!(matches!(
            reg.execute(&state_put_intent(&resource, "vm-agent", "cafe01")),
            IntentExecution::Performed { .. }
        ));

        // A read → Performed echoing the ACTUAL stored value, action "read" — regardless of what
        // the agent declared, so reconcile can Match (declared==actual) or Diverge (declared!=actual).
        for declared in ["cafe01", "beef99", ""] {
            match reg.execute(&state_get_intent(&resource, "vm-agent", declared)) {
                IntentExecution::Performed { action, resource: r, input_hash, .. } => {
                    assert_eq!(action, "read", "the act performed IS a read");
                    assert_eq!(r, resource);
                    assert_eq!(
                        input_hash, "cafe01",
                        "echoes the REAL stored value-hash, not the agent's claim ({declared:?})"
                    );
                }
                other => panic!("expected Performed for declared {declared:?}, got {other:?}"),
            }
        }

        // A DIFFERENT principal reading the same key → Declined (no cross-principal state read).
        assert!(
            matches!(
                reg.execute(&state_get_intent(&resource, "vm-other", "cafe01")),
                IntentExecution::Declined { .. }
            ),
            "an agent can only read its OWN state — never another principal's"
        );

        // An ABSENT key for the acting principal → Declined (authorized_not_performed).
        let absent = format!("{STATE_PUT_PREFIX}never-written");
        assert!(matches!(
            reg.execute(&state_get_intent(&absent, "vm-agent", "cafe01")),
            IntentExecution::Declined { .. }
        ));
    }

    /// Fail-closed scoping for the read: outside the store namespace, a bad key, or an unbounded
    /// expected-value DECLINES — the read affordance is as strict about its inputs as the write.
    #[test]
    fn state_get_declines_bad_scopes() {
        let dir = tempfile::tempdir().unwrap();
        let reg = MethodRegistryExecutor::production(
            Arc::new(AuditLog::new()),
            Some(dir.path().to_path_buf()),
        );
        for (resource, expected) in [
            ("elastos://mail/send".to_string(), "aa".to_string()), // outside namespace
            (STATE_PUT_PREFIX.to_string(), "aa".to_string()),      // empty key
            (format!("{STATE_PUT_PREFIX}a/b"), "aa".to_string()),  // path trick
            (format!("{STATE_PUT_PREFIX}k"), "not hex".to_string()), // free-text expected value
        ] {
            assert!(
                matches!(
                    reg.execute(&state_get_intent(&resource, "vm-agent", &expected)),
                    IntentExecution::Declined { .. }
                ),
                "must decline resource {resource:?} expected {expected:?}"
            );
        }
    }

    /// Without a data dir there is no store to read — state_get is honestly UNWIRED.
    #[test]
    fn state_get_is_unwired_without_a_data_dir() {
        let reg = MethodRegistryExecutor::production(Arc::new(AuditLog::new()), None);
        let resource = format!("{STATE_PUT_PREFIX}cursor");
        assert!(matches!(
            reg.execute(&state_get_intent(&resource, "vm-agent", "cafe01")),
            IntentExecution::Declined { .. }
        ));
    }

    /// A write the store cannot persist DECLINES with the true reason — Performed only for a write
    /// that landed. (Seam: a FILE squatting where the store's directory tree must be created.)
    #[test]
    fn state_put_declines_when_the_store_write_fails() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Local"), b"squat").unwrap();
        let reg = MethodRegistryExecutor::production(
            Arc::new(AuditLog::new()),
            Some(dir.path().to_path_buf()),
        );
        let resource = format!("{STATE_PUT_PREFIX}cursor");
        match reg.execute(&state_put_intent(&resource, "vm-agent", "cafe01")) {
            IntentExecution::Declined { reason } => {
                assert!(reason.contains("could not be persisted"), "true reason: {reason}");
            }
            other => panic!("an unlanded write must Decline, got {other:?}"),
        }
    }

    /// Without a data dir there is no Inbox store to deliver into — the method is honestly
    /// UNWIRED (⇒ Undelivered), never a fabricated delivery.
    #[test]
    fn notify_is_unwired_without_a_data_dir() {
        let reg = MethodRegistryExecutor::production(Arc::new(AuditLog::new()), None);
        let resource = format!("{INBOX_NOTIFY_PREFIX}agent-status");
        assert!(matches!(
            reg.execute(&notify_intent(&resource, "vm-agent", "")),
            IntentExecution::Declined { .. }
        ));
    }

    /// A delivery the store cannot persist is DECLINED with the true reason — Performed is only
    /// ever reported for a write that landed. (Seam: a FILE squatting where the notifications
    /// directory tree must be created makes the store write fail, root or not.)
    #[test]
    fn notify_declines_when_the_store_write_fails() {
        let dir = tempfile::tempdir().unwrap();
        // The notifications store lives under <data_dir>/Local/... — squat a FILE at Local.
        std::fs::write(dir.path().join("Local"), b"squat").unwrap();
        let reg = MethodRegistryExecutor::production(
            Arc::new(AuditLog::new()),
            Some(dir.path().to_path_buf()),
        );
        let resource = format!("{INBOX_NOTIFY_PREFIX}agent-status");
        match reg.execute(&notify_intent(&resource, "vm-agent", "")) {
            IntentExecution::Declined { reason } => {
                assert!(
                    reason.contains("could not be delivered"),
                    "the true failure is named: {reason}"
                );
            }
            other => panic!("an unlanded delivery must Decline, got {other:?}"),
        }
    }

    #[test]
    fn content_seen_tracks_real_state_not_the_declaration() {
        use elastos_runtime::capability::IntentDeclarationV1;
        let dir = tempfile::tempdir().unwrap();
        let log = Arc::new(AuditLog::with_file(dir.path().join("audit.log")).unwrap());
        // Record that principal "vm-agent" SUCCESSFULLY OPENED one content id.
        log.content_open("sess", "vm-agent", "QmSEEN", "view", "opened", "prov", None)
            .unwrap();
        let reg = MethodRegistryExecutor::production(log, None);

        // Intent resource is a content-access-CHECK ref: prefix + content id.
        let check = |content_id: &str| format!("{CONTENT_ACCESS_CHECK_PREFIX}{content_id}");
        let intent_for = |resource: String, capsule: &str| {
            let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
            IntentDeclarationV1::issue(
                &sk,
                sk.verifying_key().to_bytes(),
                "i",
                capsule,
                "runtime.content_seen",
                "",
                &resource,
                "read",
                "grant-1",
            )
        };
        // The SAME method + declaration shape reconciles differently based on REAL state:
        match reg.execute(&intent_for(check("QmSEEN"), "vm-agent")) {
            IntentExecution::Performed { resource, action, .. } => {
                assert_eq!(resource, check("QmSEEN")); // the CHECK resource, honestly echoed
                assert_eq!(action, "read");
            }
            other => panic!("expected Performed for a seen content id, got {other:?}"),
        }
        // Never-opened id ⇒ Declined.
        assert!(matches!(
            reg.execute(&intent_for(check("QmNEVER"), "vm-agent")),
            IntentExecution::Declined { .. }
        ));
        // PRINCIPAL-SCOPED: a DIFFERENT capsule asking about the same id gets Declined — no
        // cross-principal existence oracle.
        assert!(
            matches!(
                reg.execute(&intent_for(check("QmSEEN"), "vm-other")),
                IntentExecution::Declined { .. }
            ),
            "content_seen must not reveal another principal's access"
        );
    }
}
