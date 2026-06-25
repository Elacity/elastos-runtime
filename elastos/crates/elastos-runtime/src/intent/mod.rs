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
//! [`compile_sequence`] extends this to a MULTI-STEP plan: an ORDERED, CALLER-
//! supplied sequence of `(manifest, sub-goal)` steps (which may cross capsules)
//! compiles to one [`CompiledPlan`] with each step's gate, fail-closed if ANY step
//! is unresolvable (the whole compile fails, carrying the failing step index; no
//! partial plan is ever built). The sequence and its ORDER come from the caller --
//! the compiler never reorders, dedups, or derives a dependency.
//!
//! Scope (honest, stated in the types): STRUCTURED intents only -- natural-language
//! parsing is the separate inference layer. The compiler is PURE: every manifest is
//! an input, never discovered (which capsule offers `key.release` is the caller's
//! job). DEFERRED, because nothing in the manifests declares it and inventing it
//! would fabricate: deriving the step sequence from a high-level goal (the recipe
//! layer), cross-step DATAFLOW binding (step A's output into step B's input --
//! `invoke::plan` never reads `output_schema`, and shipped output schemas are
//! opaque), a combined-authority AGGREGATE gate (the two gate shapes are
//! field-disjoint; a union would under-state -- the per-step gates are all present
//! for any shell to summarise itself), and full cross-capsule DISCOVERY.
//! FAIL-CLOSED throughout: 0-match -> Unresolvable, >1 -> Ambiguous (never guesses),
//! a gate failure propagates the underlying [`InvokeError`] whole.

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

/// One step of a caller-supplied sequence: a goal against a specific manifest. The
/// caller supplies the ordered list (and so the order); the compiler never derives
/// or reorders it.
pub struct SubGoal<'a> {
    pub manifest: &'a CapsuleManifest,
    pub intent: StructuredIntent,
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
    /// A step of a multi-step sequence failed; carries its 0-based `index` and the
    /// underlying error WHOLE. The whole sequence fails closed -- no partial plan.
    Step {
        index: usize,
        source: Box<IntentError>,
    },
    /// A sequence with no steps is not a plan.
    EmptySequence,
}

/// How a [`CompiledPlan`] was composed, so a shell never mistakes a chained plan
/// for a single-step one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompositionKind {
    SingleStep,
    MultiStep,
}

/// One resolved step of a compiled plan, naming its own `capsule` (so a
/// cross-capsule multi-step plan never lies about a single capsule). A SUM type,
/// because the two reflective modes have field-disjoint gates that must not be
/// lossily flattened: the affordance gate is a single action + approval +
/// audit-level; the provider gate is a union of resources + actions + named audit
/// events. Each carries its existing gate struct VERBATIM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CompiledStep {
    /// The goal resolved to an interface affordance (`interface` + `method`).
    Affordance {
        capsule: String,
        interface: String,
        method: String,
        operation: String,
        gate: InvocationPlan,
    },
    /// The goal resolved to a provider operation.
    Operation {
        capsule: String,
        operation: String,
        gate: ProviderOperationPlan,
    },
}

/// A shell-agnostic compiled plan: how it was composed and the ordered resolved
/// steps with their previewed gates (each naming its capsule). Pure data -- no
/// rendering, no aggregate gate (a shell computes its own from the per-step gates).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompiledPlan {
    pub composition: CompositionKind,
    pub steps: Vec<CompiledStep>,
}

/// Resolve ONE goal against ONE manifest to a single [`CompiledStep`]. This is the
/// one canonical resolution + gate-derivation path; both [`compile`] and
/// [`compile_sequence`] reuse it verbatim.
///
/// Precondition: `manifest` is assumed VALIDATED ([`CapsuleManifest::validate`]) --
/// interface and method ids are unique only after validation, the same implicit
/// contract `invoke::plan` and the inspector already rely on.
fn compile_step(
    manifest: &CapsuleManifest,
    intent: &StructuredIntent,
) -> Result<CompiledStep, IntentError> {
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
    if let Some((interface, method)) = affordance_matches.into_iter().next() {
        let gate = invoke::plan(method, &intent.args).map_err(IntentError::Gate)?;
        Ok(CompiledStep::Affordance {
            capsule: manifest.name.clone(),
            interface,
            method: method.id.clone(),
            operation: intent.operation.clone(),
            gate,
        })
    } else {
        let authority = manifest
            .authority
            .as_ref()
            .expect("a provider match implies the authority is present");
        let gate = invoke::plan_provider_operation(authority, op).map_err(IntentError::Gate)?;
        Ok(CompiledStep::Operation {
            capsule: manifest.name.clone(),
            operation: intent.operation.clone(),
            gate,
        })
    }
}

/// Compile a single structured goal against ONE capsule manifest into a previewed
/// single-step plan. See [`compile_step`] for resolution + fail-closed semantics.
pub fn compile(
    manifest: &CapsuleManifest,
    intent: &StructuredIntent,
) -> Result<CompiledPlan, IntentError> {
    let step = compile_step(manifest, intent)?;
    Ok(CompiledPlan {
        composition: CompositionKind::SingleStep,
        steps: vec![step],
    })
}

/// Compile an ORDERED, caller-supplied sequence of goals into one multi-step plan.
///
/// Each step is resolved + planned independently via [`compile_step`], in the exact
/// order given (never reordered, deduped, or dependency-inferred). FAIL-CLOSED and
/// atomic: the first step that is `Unresolvable`/`Ambiguous`/`Gate(..)` aborts the
/// WHOLE compile with `IntentError::Step { index, source }` carrying the failing
/// index -- no partial plan is ever constructed. An empty sequence is
/// `EmptySequence` (a goal with no steps is not a plan). The steps are independently
/// gated; the compiler does NOT promise step A's output is bound into step B's input
/// (cross-step dataflow is deferred).
pub fn compile_sequence(steps: &[SubGoal]) -> Result<CompiledPlan, IntentError> {
    if steps.is_empty() {
        return Err(IntentError::EmptySequence);
    }
    let mut compiled = Vec::with_capacity(steps.len());
    for (index, sub) in steps.iter().enumerate() {
        let step = compile_step(sub.manifest, &sub.intent).map_err(|source| IntentError::Step {
            index,
            source: Box::new(source),
        })?;
        compiled.push(step);
    }
    Ok(CompiledPlan {
        composition: CompositionKind::MultiStep,
        steps: compiled,
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
        // manifest tests). Validate, per compile_step()'s precondition.
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
        // "capsule.view" -- the compiler discovers which affordance provides it.
        let manifest = shipped("capsule-inspector");
        let plan = compile(
            &manifest,
            &intent("view", serde_json::json!({ "target": "capsule:probe" })),
        )
        .expect("'view' resolves to the capsule.view affordance");
        assert_eq!(plan.composition, CompositionKind::SingleStep);
        assert_eq!(plan.steps.len(), 1);
        match &plan.steps[0] {
            CompiledStep::Affordance {
                capsule,
                method,
                operation,
                gate,
                ..
            } => {
                assert_eq!(capsule, "capsule-inspector");
                assert_eq!(
                    method, "capsule.view",
                    "resolved the goal to the real method id"
                );
                assert_eq!(operation, "view");
                assert_eq!(gate.capability_action, Action::Read);
                assert_eq!(gate.approval, AffordanceApprovalMode::RuntimePolicy);
                assert_eq!(gate.audit, AffordanceAuditMode::Event);
            }
            other => panic!("expected an affordance step, got {other:?}"),
        }
    }

    #[test]
    fn compile_resolves_operation_to_provider_operation() {
        let manifest = shipped("rights-provider");
        let plan = compile(
            &manifest,
            &intent("has_access_by_content_id", serde_json::json!({})),
        )
        .expect("the provider operation resolves");
        match &plan.steps[0] {
            CompiledStep::Operation {
                capsule,
                operation,
                gate,
            } => {
                assert_eq!(capsule, "rights-provider");
                assert_eq!(operation, "has_access_by_content_id");
                assert!(gate.actions.contains(&Action::Read));
                assert!(gate.resources.iter().any(|r| r == "elastos://rights/*"));
            }
            other => panic!("expected a provider-operation step, got {other:?}"),
        }
    }

    #[test]
    fn compile_fails_closed_on_unresolvable_goal() {
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
        // Even a VALID manifest can declare two affordances with the same operation;
        // the compiler refuses to guess which, fail-closed.
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
        let manifest = shipped("capsule-inspector");
        let err = compile(&manifest, &intent("view", serde_json::json!({})))
            .expect_err("capsule.view requires a 'target' arg");
        assert_eq!(
            err,
            IntentError::Gate(InvokeError::MissingRequiredField("target".to_string()))
        );
    }

    #[test]
    fn compile_sequence_intra_capsule_preserves_order() {
        // A two-step sequence within one capsule compiles to a MultiStep plan whose
        // steps are in the caller's order (list THEN view), each with its own gate.
        let inspector = shipped("capsule-inspector");
        let plan = compile_sequence(&[
            SubGoal {
                manifest: &inspector,
                intent: intent("list", serde_json::json!({})),
            },
            SubGoal {
                manifest: &inspector,
                intent: intent("view", serde_json::json!({ "target": "capsule:probe" })),
            },
        ])
        .expect("a two-step intra-capsule sequence compiles");
        assert_eq!(plan.composition, CompositionKind::MultiStep);
        assert_eq!(plan.steps.len(), 2);
        let ops: Vec<&str> = plan
            .steps
            .iter()
            .map(|s| match s {
                CompiledStep::Affordance { operation, .. } => operation.as_str(),
                CompiledStep::Operation { operation, .. } => operation.as_str(),
            })
            .collect();
        assert_eq!(ops, vec!["list", "view"], "order in == order out");
    }

    #[test]
    fn compile_sequence_cross_provider_chain_compiles_both_gates() {
        // The honest cross-capsule chain spans TWO separate provider manifests: a
        // rights check THEN a key release. Each step names its own capsule and
        // carries its own provider gate (the authority's declared audit events).
        let rights = shipped("rights-provider");
        let key = shipped("key-provider");
        let plan = compile_sequence(&[
            SubGoal {
                manifest: &rights,
                intent: intent("has_access_by_content_id", serde_json::json!({})),
            },
            SubGoal {
                manifest: &key,
                intent: intent("release", serde_json::json!({})),
            },
        ])
        .expect("a real cross-provider chain compiles");
        assert_eq!(plan.composition, CompositionKind::MultiStep);
        assert_eq!(plan.steps.len(), 2);
        match &plan.steps[0] {
            CompiledStep::Operation {
                capsule,
                operation,
                gate,
            } => {
                assert_eq!(capsule, "rights-provider");
                assert_eq!(operation, "has_access_by_content_id");
                let events: Vec<&str> = gate.audit_events.iter().map(|s| s.as_str()).collect();
                assert_eq!(events, vec!["rights.status", "rights.check.denied"]);
            }
            other => panic!("step 0 should be the rights provider op, got {other:?}"),
        }
        match &plan.steps[1] {
            CompiledStep::Operation {
                capsule,
                operation,
                gate,
            } => {
                assert_eq!(capsule, "key-provider");
                assert_eq!(operation, "release");
                let events: Vec<&str> = gate.audit_events.iter().map(|s| s.as_str()).collect();
                assert_eq!(
                    events,
                    vec!["key.status", "key.release.denied", "key.rewrap.denied"]
                );
            }
            other => panic!("step 1 should be the key provider op, got {other:?}"),
        }
    }

    #[test]
    fn compile_sequence_fails_closed_on_intermediate_unresolvable_no_partial_plan() {
        // An unresolvable MIDDLE step fails the WHOLE compile with the failing index
        // and the underlying error whole -- no partial 2-of-3 plan escapes.
        let inspector = shipped("capsule-inspector");
        let err = compile_sequence(&[
            SubGoal {
                manifest: &inspector,
                intent: intent("list", serde_json::json!({})),
            },
            SubGoal {
                manifest: &inspector,
                intent: intent("obliterate", serde_json::json!({})),
            },
            SubGoal {
                manifest: &inspector,
                intent: intent("view", serde_json::json!({ "target": "x" })),
            },
        ])
        .expect_err("an unresolvable middle step fails the whole plan");
        assert_eq!(
            err,
            IntentError::Step {
                index: 1,
                source: Box::new(IntentError::Unresolvable {
                    operation: "obliterate".to_string()
                }),
            }
        );
    }

    #[test]
    fn compile_sequence_propagates_gate_error_with_index() {
        // A resolvable step whose ARGS fail its input_schema fails the whole compile,
        // the InvokeError carried whole inside Step at the right index.
        let inspector = shipped("capsule-inspector");
        let err = compile_sequence(&[
            SubGoal {
                manifest: &inspector,
                intent: intent("list", serde_json::json!({})),
            },
            SubGoal {
                manifest: &inspector,
                intent: intent("view", serde_json::json!({})), // missing required "target"
            },
        ])
        .expect_err("the second step's args are invalid");
        assert_eq!(
            err,
            IntentError::Step {
                index: 1,
                source: Box::new(IntentError::Gate(InvokeError::MissingRequiredField(
                    "target".to_string()
                ))),
            }
        );
    }

    #[test]
    fn compile_sequence_rejects_empty_sequence() {
        let empty: &[SubGoal] = &[];
        assert_eq!(
            compile_sequence(empty).unwrap_err(),
            IntentError::EmptySequence
        );
    }
}
