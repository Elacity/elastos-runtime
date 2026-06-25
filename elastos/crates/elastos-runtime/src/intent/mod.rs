//! Intent compilation: resolve a structured capability goal to a previewed plan.
//!
//! The inspection -> invocation bridge ([`crate::inspect`], [`crate::invoke`])
//! answers "given THIS affordance, what is its gate?". The intent compiler answers
//! the question a shell or agent actually has: "given a GOAL (an operation, maybe
//! narrowed to a resource), WHICH declared affordance or provider operation
//! satisfies it, and what is its gate?". It RESOLVES the goal to a real declared
//! capability across the capsule's manifest, then delegates gate derivation
//! VERBATIM to [`crate::invoke`] (one canonical gate path), and emits a
//! shell-agnostic [`CompiledPlan`] as data. Any shell renders the same bytes.
//!
//! Scope (honest): STRUCTURED intents only -- natural-language parsing is the
//! separate inference layer. SINGLE-CAPSULE, SINGLE-STEP resolution -- cross-capsule
//! discovery and multi-step composition are deferred (the [`CompiledPlan`] marks
//! its `composition` so a shell never mistakes scope). FAIL-CLOSED: an unresolvable
//! (0-match) or ambiguous (>1-match) goal yields a typed error, never a fabricated
//! or guessed plan; a gate failure propagates the underlying [`InvokeError`] whole.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use elastos_common::CapsuleManifest;

use crate::invoke::{self, InvocationPlan, InvokeError, ProviderOperationPlan};

/// A structured capability goal. NOT natural language: it names an `operation`
/// (optionally narrowed to a `resource`) that the compiler resolves to whatever
/// declared affordance or provider operation provides it -- so the goal names the
/// capability, never the affordance's own id. `args` carry through to the
/// affordance's `input_schema` validation when the goal resolves to an affordance
/// (ignored for provider operations, which take none).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StructuredIntent {
    /// The capability the goal wants to perform (e.g. "view", "release").
    pub operation: String,
    /// Optional resource to disambiguate (e.g. "elastos://key/*").
    #[serde(default)]
    pub resource: Option<String>,
    /// Arguments for the affordance's input_schema (defaults to null); unused for
    /// provider operations.
    #[serde(default)]
    pub args: Value,
}

/// Why a goal could not be compiled to a plan. Resolution errors are the new
/// vocabulary the compiler adds; gate errors are propagated VERBATIM from
/// [`crate::invoke`] (never re-invented, so a gate is never under-stated).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntentError {
    /// No declared affordance or provider operation satisfies the goal.
    Unresolvable { operation: String },
    /// More than one declared capability satisfies the goal; the compiler refuses
    /// to guess which (fail-closed) rather than pick one.
    Ambiguous { operation: String, matches: usize },
    /// The goal resolved, but deriving its gate failed (bad args, an unknown
    /// declared action, ...). Propagated whole from the canonical planner.
    Gate(InvokeError),
}

/// How a [`CompiledPlan`] was composed. Single-step today; the type leaves room
/// for the deferred multi-step composition without lying about the current scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompositionKind {
    SingleStep,
}

/// One resolved step of a compiled plan. A SUM type, because the two reflective
/// modes have field-disjoint gates that must not be lossily flattened: the
/// affordance gate is a single action + approval + audit-level; the provider gate
/// is a union of resources + actions + named audit events. Each carries its
/// existing gate struct VERBATIM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CompiledStep {
    /// The goal resolved to an interface affordance (`interface` + `method`).
    Affordance {
        interface: String,
        method: String,
        operation: String,
        gate: InvocationPlan,
    },
    /// The goal resolved to a provider operation.
    Operation {
        operation: String,
        gate: ProviderOperationPlan,
    },
}

/// A shell-agnostic compiled plan: which capsule, how it was composed, and the
/// ordered resolved steps with their previewed gates. Pure data -- no rendering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompiledPlan {
    pub capsule: String,
    pub composition: CompositionKind,
    pub steps: Vec<CompiledStep>,
}

/// Compile a structured goal against ONE capsule manifest into a previewed plan.
///
/// Resolves `intent.operation` (optionally narrowed by `intent.resource`) to the
/// declared affordance or provider operation that satisfies it, then delegates gate
/// derivation to [`crate::invoke`]. FAIL-CLOSED: 0 matches -> `Unresolvable`, >1 ->
/// `Ambiguous` (never guesses), a gate failure -> `Gate(InvokeError)`.
///
/// Precondition: `manifest` is assumed VALIDATED ([`CapsuleManifest::validate`]) --
/// interface and method ids are unique only after validation, the same implicit
/// contract `invoke::plan` and the inspector already rely on. A shell must not feed
/// raw unvalidated JSON expecting uniqueness-based behaviour.
pub fn compile(
    manifest: &CapsuleManifest,
    intent: &StructuredIntent,
) -> Result<CompiledPlan, IntentError> {
    let op = intent.operation.as_str();

    // Resolve against declared interface affordances by their `operation` field
    // (NOT their id -- the goal names the capability, not the affordance, so this
    // is real resolution: "view" finds the method whose operation is "view").
    let mut affordance_matches = Vec::new();
    for iface in &manifest.interfaces {
        for method in &iface.methods {
            let op_match = method.operation.as_deref() == Some(op);
            let resource_match = match &intent.resource {
                Some(r) => method.resource.as_deref() == Some(r.as_str()),
                None => true,
            };
            if op_match && resource_match {
                affordance_matches.push((iface.id.clone(), method));
            }
        }
    }

    // Resolve against provider operations (the authority's capability blocks). An
    // operation can span several blocks; `plan_provider_operation` unions them, so
    // it counts as exactly ONE provider match here.
    let provider_match = manifest.authority.as_ref().is_some_and(|authority| {
        authority.capabilities.iter().any(|cap| {
            let op_match = cap.operations.iter().any(|o| o == op);
            let resource_match = match &intent.resource {
                Some(r) => &cap.resource == r,
                None => true,
            };
            op_match && resource_match
        })
    });

    let total = affordance_matches.len() + usize::from(provider_match);
    if total == 0 {
        return Err(IntentError::Unresolvable {
            operation: intent.operation.clone(),
        });
    }
    if total > 1 {
        return Err(IntentError::Ambiguous {
            operation: intent.operation.clone(),
            matches: total,
        });
    }

    // Exactly one match. Delegate gate derivation to the canonical planner and
    // store its output verbatim; any InvokeError propagates whole (fail-closed).
    let step = if let Some((interface, method)) = affordance_matches.into_iter().next() {
        let gate = invoke::plan(method, &intent.args).map_err(IntentError::Gate)?;
        CompiledStep::Affordance {
            interface,
            method: method.id.clone(),
            operation: intent.operation.clone(),
            gate,
        }
    } else {
        let authority = manifest
            .authority
            .as_ref()
            .expect("a provider match implies the authority is present");
        let gate = invoke::plan_provider_operation(authority, op).map_err(IntentError::Gate)?;
        CompiledStep::Operation {
            operation: intent.operation.clone(),
            gate,
        }
    };

    Ok(CompiledPlan {
        capsule: manifest.name.clone(),
        composition: CompositionKind::SingleStep,
        steps: vec![step],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::token::Action;
    use elastos_common::{AffordanceApprovalMode, AffordanceAuditMode};

    fn shipped(name: &str) -> CapsuleManifest {
        // The intent compiler resolves over the REAL shipped manifests, so the test
        // does too (same CARGO_MANIFEST_DIR-relative path as the elastos-common
        // manifest tests). Validate, per compile()'s precondition.
        let path = format!(
            "{}/../../../capsules/{}/capsule.json",
            env!("CARGO_MANIFEST_DIR"),
            name
        );
        let json = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        let manifest: CapsuleManifest = serde_json::from_str(&json).unwrap();
        manifest.validate().expect("shipped manifest validates");
        manifest
    }

    fn intent(operation: &str, args: Value) -> StructuredIntent {
        StructuredIntent {
            operation: operation.to_string(),
            resource: None,
            args,
        }
    }

    #[test]
    fn compile_resolves_operation_to_affordance() {
        // GENUINE resolution: the goal names the operation "view", NOT the method id
        // "capsule.view" -- the compiler discovers which affordance provides it, then
        // plans it through the canonical gate path.
        let manifest = shipped("capsule-inspector");
        let plan = compile(
            &manifest,
            &intent("view", serde_json::json!({ "target": "capsule:probe" })),
        )
        .expect("'view' resolves to the capsule.view affordance");
        assert_eq!(plan.capsule, "capsule-inspector");
        assert_eq!(plan.composition, CompositionKind::SingleStep);
        assert_eq!(plan.steps.len(), 1);
        match &plan.steps[0] {
            CompiledStep::Affordance {
                method,
                operation,
                gate,
                ..
            } => {
                assert_eq!(
                    method, "capsule.view",
                    "resolved the goal to the real method id"
                );
                assert_eq!(operation, "view");
                // The gate is invoke::plan's output verbatim (Read / RuntimePolicy / Event).
                assert_eq!(gate.capability_action, Action::Read);
                assert_eq!(gate.approval, AffordanceApprovalMode::RuntimePolicy);
                assert_eq!(gate.audit, AffordanceAuditMode::Event);
            }
            other => panic!("expected an affordance step, got {other:?}"),
        }
    }

    #[test]
    fn compile_resolves_operation_to_provider_operation() {
        // GENUINE resolution against a provider authority: the goal names a provider
        // operation and the compiler resolves + plans it (the union of its gate).
        let manifest = shipped("rights-provider");
        let plan = compile(
            &manifest,
            &intent("has_access_by_content_id", serde_json::json!({})),
        )
        .expect("the provider operation resolves");
        match &plan.steps[0] {
            CompiledStep::Operation { operation, gate } => {
                assert_eq!(operation, "has_access_by_content_id");
                assert!(gate.actions.contains(&Action::Read));
                assert!(gate.resources.iter().any(|r| r == "elastos://rights/*"));
            }
            other => panic!("expected a provider-operation step, got {other:?}"),
        }
    }

    #[test]
    fn compile_fails_closed_on_unresolvable_goal() {
        // 0 matches: no affordance or provider op declares "obliterate" -> honest
        // error, never a fabricated plan.
        let manifest = shipped("capsule-inspector");
        let err = compile(&manifest, &intent("obliterate", serde_json::json!({})))
            .expect_err("an undeclared operation must not resolve");
        assert_eq!(
            err,
            IntentError::Unresolvable {
                operation: "obliterate".to_string()
            }
        );
    }

    #[test]
    fn compile_fails_closed_on_ambiguous_goal() {
        // >1 matches: even a VALID manifest can declare two affordances with the same
        // operation -- the compiler refuses to guess which, fail-closed.
        let manifest: CapsuleManifest = serde_json::from_value(serde_json::json!({
            "schema": "elastos.capsule/v1", "version": "0.1.0", "name": "amb",
            "role": "app", "type": "wasm", "entrypoint": "x.wasm",
            "interfaces": [{
                "id": "i", "version": "0.1.0",
                "methods": [
                    { "id": "a", "operation": "dup", "risk": "read", "approval": "none", "audit": "none" },
                    { "id": "b", "operation": "dup", "risk": "read", "approval": "none", "audit": "none" }
                ]
            }]
        }))
        .unwrap();
        manifest
            .validate()
            .expect("the manifest itself is valid (ids unique)");
        let err = compile(&manifest, &intent("dup", serde_json::json!({})))
            .expect_err("two affordances with one operation is ambiguous");
        assert_eq!(
            err,
            IntentError::Ambiguous {
                operation: "dup".to_string(),
                matches: 2
            }
        );
    }

    #[test]
    fn compile_propagates_gate_error_unchanged() {
        // The goal resolves, but its args fail the affordance's input_schema -- the
        // underlying InvokeError is propagated WHOLE, not re-invented or swallowed.
        let manifest = shipped("capsule-inspector");
        let err = compile(&manifest, &intent("view", serde_json::json!({})))
            .expect_err("capsule.view requires a 'target' arg");
        assert_eq!(
            err,
            IntentError::Gate(InvokeError::MissingRequiredField("target".to_string()))
        );
    }
}
