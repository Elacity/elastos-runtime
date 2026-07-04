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

    /// The executor set the production runtime ships with. It is DELIBERATELY EMPTY: no real
    /// side-effecting affordance (mail, payment, storage) is wired yet, and shipping a no-op that
    /// echoed the declaration back would re-introduce the exact over-claim this seam exists to
    /// retire (a durable `success=true` "write" for a method that performed no write). So every
    /// dispatched method currently DECLINES ⇒ `Undelivered` ⇒ outcome `authorized_not_performed` —
    /// the honest state of the system. Real affordances are registered here (each behind its own
    /// capability gate) as they are wired, at which point their methods become genuinely `performed`.
    pub fn production() -> Self {
        Self::new()
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
    fn production_registry_is_empty_so_every_method_declines() {
        // The honest default: no real executor is wired, so nothing is "performed" — every method
        // Declines (⇒ Undelivered), never a fabricated match.
        let reg = MethodRegistryExecutor::production();
        assert!(matches!(
            reg.execute(&intent("pay.invoke")),
            IntentExecution::Declined { .. }
        ));
        assert!(matches!(
            reg.execute(&intent("runtime.echo")),
            IntentExecution::Declined { .. }
        ));
    }
}
