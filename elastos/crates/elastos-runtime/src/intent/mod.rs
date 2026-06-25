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

use elastos_common::{CapsuleAffordanceDescriptor, CapsuleManifest, ProviderAuthority};

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

/// A located capability within ONE manifest: enough to plan it, borrowing into the
/// manifest so the gate path re-derives nothing. The provider arm carries the matched
/// authority, so presence is type-level (no re-lookup).
enum Located<'m> {
    Affordance {
        capsule: &'m str,
        interface: String,
        method: &'m CapsuleAffordanceDescriptor,
    },
    Provider {
        capsule: &'m str,
        authority: &'m ProviderAuthority,
    },
}

/// Every declared affordance or provider operation in ONE manifest that satisfies the
/// goal (matched by `operation`, narrowed by `resource`). A provider operation counts
/// as EXACTLY ONE match per manifest even when split across capability blocks
/// (`plan_provider_operation` re-unions them downstream), so a split-privilege
/// provider is never falsely reported ambiguous.
fn locate<'m>(manifest: &'m CapsuleManifest, intent: &StructuredIntent) -> Vec<Located<'m>> {
    let op = intent.operation.as_str();
    let mut hits = Vec::new();

    // Interface affordances, matched by their `operation` field (NOT their id -- the
    // goal names the capability: "view" finds the method whose operation is "view").
    for iface in &manifest.interfaces {
        for method in &iface.methods {
            let op_match = method.operation.as_deref() == Some(op);
            let resource_match = match &intent.resource {
                Some(r) => method.resource.as_deref() == Some(r.as_str()),
                None => true,
            };
            if op_match && resource_match {
                hits.push(Located::Affordance {
                    capsule: &manifest.name,
                    interface: iface.id.clone(),
                    method,
                });
            }
        }
    }

    // Provider operations: ONE match per manifest (the union across blocks), never one
    // per block -- else a split-privilege provider would self-collide to ambiguous.
    if let Some(authority) = manifest.authority.as_ref() {
        let provider_match = authority.capabilities.iter().any(|cap| {
            let op_match = cap.operations.iter().any(|o| o == op);
            let resource_match = match &intent.resource {
                Some(r) => &cap.resource == r,
                None => true,
            };
            op_match && resource_match
        });
        if provider_match {
            hits.push(Located::Provider {
                capsule: &manifest.name,
                authority,
            });
        }
    }

    hits
}

/// Derive the gate for a located capability, delegating VERBATIM to the canonical
/// invoke planner. The one gate-derivation path shared by every entry point.
fn plan_located(located: Located, intent: &StructuredIntent) -> Result<CompiledStep, IntentError> {
    match located {
        Located::Affordance {
            capsule,
            interface,
            method,
        } => {
            let gate = invoke::plan(method, &intent.args).map_err(IntentError::Gate)?;
            Ok(CompiledStep::Affordance {
                capsule: capsule.to_string(),
                interface,
                method: method.id.clone(),
                operation: intent.operation.clone(),
                gate,
            })
        }
        Located::Provider { capsule, authority } => {
            let gate = invoke::plan_provider_operation(authority, &intent.operation)
                .map_err(IntentError::Gate)?;
            Ok(CompiledStep::Operation {
                capsule: capsule.to_string(),
                operation: intent.operation.clone(),
                gate,
            })
        }
    }
}

/// Resolve ONE goal against ONE manifest to a single [`CompiledStep`]. This is the one
/// canonical resolution + gate-derivation path; [`compile`], [`compile_sequence`], and
/// [`discover`] all reuse it (via [`locate`] / [`plan_located`]) verbatim.
///
/// Precondition: `manifest` is assumed VALIDATED ([`CapsuleManifest::validate`]) --
/// interface and method ids are unique only after validation, the same implicit
/// contract `invoke::plan` and the inspector already rely on.
fn compile_step(
    manifest: &CapsuleManifest,
    intent: &StructuredIntent,
) -> Result<CompiledStep, IntentError> {
    let mut hits = locate(manifest, intent);
    match hits.len() {
        0 => Err(IntentError::Unresolvable {
            operation: intent.operation.clone(),
        }),
        1 => plan_located(hits.remove(0), intent),
        n => Err(IntentError::Ambiguous {
            operation: intent.operation.clone(),
            matches: n,
        }),
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

/// Resolve ONE goal across a SET of manifests to a single [`CompiledStep`]: the
/// cross-capsule core shared by [`discover`] and [`compile_sequence_discovered`]. 0
/// across the set -> `Unresolvable`; exactly 1 -> plan it (the canonical gate path); >1
/// -> `Ambiguous`, never guessing which capsule. A provider op is ONE match per
/// manifest (see [`locate`]), so a split-privilege provider is never falsely ambiguous.
fn discover_step(
    manifests: &[&CapsuleManifest],
    intent: &StructuredIntent,
) -> Result<CompiledStep, IntentError> {
    let mut all: Vec<Located> = manifests
        .iter()
        .copied()
        .flat_map(|manifest| locate(manifest, intent))
        .collect();
    match all.len() {
        0 => Err(IntentError::Unresolvable {
            operation: intent.operation.clone(),
        }),
        1 => plan_located(all.remove(0), intent),
        n => Err(IntentError::Ambiguous {
            operation: intent.operation.clone(),
            matches: n,
        }),
    }
}

/// Discover which capsule in a SET offers the goal, then plan it. Unlike [`compile`]
/// (which the caller pins to one manifest), `discover` searches the set and makes the
/// CROSS-CAPSULE decision the single-manifest path cannot: 0 capabilities across the
/// set -> `Unresolvable`; EXACTLY 1 -> resolve + plan it (the same canonical gate
/// path); >1 (two capsules each offering the op, OR one capsule offering it
/// ambiguously) -> `Ambiguous`, NEVER guessing which capsule. Pure: the caller passes
/// the loaded set (enumerating installed capsules is the server/shell's job). The
/// `Ambiguous` error reports the operation and aggregate match count, never which
/// capsules -- the runtime never names a candidate it refuses to pick.
pub fn discover(
    manifests: &[&CapsuleManifest],
    intent: &StructuredIntent,
) -> Result<CompiledPlan, IntentError> {
    Ok(CompiledPlan {
        composition: CompositionKind::SingleStep,
        steps: vec![discover_step(manifests, intent)?],
    })
}

/// Compile an ORDERED sequence of goals where each step is DISCOVERED across the SET
/// (the caller does not pin a manifest per step, unlike [`compile_sequence`]). Each
/// step is discovered + planned in order; FAIL-CLOSED and atomic: the first step that
/// is `Unresolvable`/`Ambiguous`/`Gate(..)` aborts the WHOLE compile with
/// `IntentError::Step { index, source }` carrying the failing index -- no partial plan.
/// Empty -> `EmptySequence`. The steps are independently discovered + gated (cross-step
/// dataflow binding is deferred, as in [`compile_sequence`]).
pub fn compile_sequence_discovered(
    manifests: &[&CapsuleManifest],
    intents: &[StructuredIntent],
) -> Result<CompiledPlan, IntentError> {
    if intents.is_empty() {
        return Err(IntentError::EmptySequence);
    }
    let mut compiled = Vec::with_capacity(intents.len());
    for (index, intent) in intents.iter().enumerate() {
        let step = discover_step(manifests, intent).map_err(|source| IntentError::Step {
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

    #[test]
    fn discover_resolves_unique_provider_op_across_the_set() {
        // The caller does NOT name the capsule: discover finds that "release" is
        // offered only by key-provider out of the whole set, and gates it.
        let rights = shipped("rights-provider");
        let key = shipped("key-provider");
        let inspector = shipped("capsule-inspector");
        let plan = discover(
            &[&rights, &key, &inspector],
            &intent("release", serde_json::json!({})),
        )
        .expect("'release' is offered only by key-provider");
        assert_eq!(plan.composition, CompositionKind::SingleStep);
        match &plan.steps[0] {
            CompiledStep::Operation {
                capsule, operation, ..
            } => {
                assert_eq!(capsule, "key-provider", "discovered the right capsule");
                assert_eq!(operation, "release");
            }
            other => panic!("expected the key provider op, got {other:?}"),
        }
    }

    #[test]
    fn discover_resolves_unique_affordance_across_the_set() {
        // Discovery also resolves an interface affordance: "view" is offered only by
        // capsule-inspector (an affordance, not a provider op).
        let rights = shipped("rights-provider");
        let key = shipped("key-provider");
        let inspector = shipped("capsule-inspector");
        let plan = discover(
            &[&rights, &key, &inspector],
            &intent("view", serde_json::json!({ "target": "capsule:probe" })),
        )
        .expect("'view' is offered only by capsule-inspector");
        match &plan.steps[0] {
            CompiledStep::Affordance {
                capsule, method, ..
            } => {
                assert_eq!(capsule, "capsule-inspector");
                assert_eq!(method, "capsule.view");
            }
            other => panic!("expected the inspector affordance, got {other:?}"),
        }
    }

    #[test]
    fn discover_fails_closed_on_cross_capsule_ambiguity() {
        // The load-bearing cross-capsule decision: "status" is declared by BOTH
        // rights-provider AND key-provider. N independent compile() calls each succeed
        // and never see the collision; only discover, summing across the set, fails
        // closed -- and never guesses which capsule.
        let rights = shipped("rights-provider");
        let key = shipped("key-provider");
        let inspector = shipped("capsule-inspector");
        // Each capsule resolves "status" on its own ...
        assert!(compile(&rights, &intent("status", serde_json::json!({}))).is_ok());
        assert!(compile(&key, &intent("status", serde_json::json!({}))).is_ok());
        // ... but discovering across the set is ambiguous.
        let err = discover(
            &[&rights, &key, &inspector],
            &intent("status", serde_json::json!({})),
        )
        .expect_err("two capsules offer 'status' -> ambiguous, never guessed");
        assert_eq!(
            err,
            IntentError::Ambiguous {
                operation: "status".to_string(),
                matches: 2
            }
        );
    }

    #[test]
    fn discover_fails_closed_on_operation_no_capsule_offers() {
        let rights = shipped("rights-provider");
        let key = shipped("key-provider");
        let inspector = shipped("capsule-inspector");
        let err = discover(
            &[&rights, &key, &inspector],
            &intent("obliterate", serde_json::json!({})),
        )
        .expect_err("no capsule in the set offers it");
        assert_eq!(
            err,
            IntentError::Unresolvable {
                operation: "obliterate".to_string()
            }
        );
    }

    #[test]
    fn compile_sequence_discovered_resolves_each_step_across_the_set() {
        // Each step is DISCOVERED across the set (the caller pins no manifest): a key
        // release THEN an inspector view, in order, as one MultiStep plan.
        let rights = shipped("rights-provider");
        let key = shipped("key-provider");
        let inspector = shipped("capsule-inspector");
        let set = [&rights, &key, &inspector];
        let plan = compile_sequence_discovered(
            &set,
            &[
                intent("release", serde_json::json!({})),
                intent("view", serde_json::json!({ "target": "capsule:probe" })),
            ],
        )
        .expect("both goals discover uniquely across the set");
        assert_eq!(plan.composition, CompositionKind::MultiStep);
        assert_eq!(plan.steps.len(), 2);
        match &plan.steps[0] {
            CompiledStep::Operation {
                capsule, operation, ..
            } => {
                assert_eq!(capsule, "key-provider");
                assert_eq!(operation, "release");
            }
            other => panic!("step 0 should be the discovered key op, got {other:?}"),
        }
        match &plan.steps[1] {
            CompiledStep::Affordance {
                capsule, method, ..
            } => {
                assert_eq!(capsule, "capsule-inspector");
                assert_eq!(method, "capsule.view");
            }
            other => panic!("step 1 should be the discovered inspector affordance, got {other:?}"),
        }
    }

    #[test]
    fn compile_sequence_discovered_fails_closed_on_ambiguous_step() {
        // A cross-capsule-ambiguous step ("status", offered by both providers) fails
        // the WHOLE sequence with its index -- no partial plan, never a guess.
        let rights = shipped("rights-provider");
        let key = shipped("key-provider");
        let inspector = shipped("capsule-inspector");
        let set = [&rights, &key, &inspector];
        let err = compile_sequence_discovered(
            &set,
            &[
                intent("release", serde_json::json!({})),
                intent("status", serde_json::json!({})),
            ],
        )
        .expect_err("the ambiguous 'status' step fails the whole sequence");
        assert_eq!(
            err,
            IntentError::Step {
                index: 1,
                source: Box::new(IntentError::Ambiguous {
                    operation: "status".to_string(),
                    matches: 2
                }),
            }
        );
    }

    #[test]
    fn compile_sequence_discovered_rejects_empty() {
        let rights = shipped("rights-provider");
        let set = [&rights];
        assert_eq!(
            compile_sequence_discovered(&set, &[]).unwrap_err(),
            IntentError::EmptySequence
        );
    }
}
