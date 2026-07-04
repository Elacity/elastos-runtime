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
    pub fn production(audit_log: Arc<AuditLog>) -> Self {
        let mut registry = Self::new();
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
        let reg = MethodRegistryExecutor::production(log);
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
        let reg = MethodRegistryExecutor::production(Arc::new(AuditLog::new()));
        assert!(matches!(
            reg.execute(&intent("runtime.audit_verify")),
            IntentExecution::Declined { .. }
        ));
    }
}
