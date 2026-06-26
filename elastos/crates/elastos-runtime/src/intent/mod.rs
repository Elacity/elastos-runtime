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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
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
    /// A caller-declared dataflow port binding could not be validated; carries its
    /// 0-based `binding` edge index and the reason. Fail-closed: the WHOLE pipeline
    /// fails, no partial plan. The compiler NEVER infers a binding (caller declares,
    /// compiler validates -- the multi-step honesty extended to dataflow).
    Unbindable {
        binding: usize,
        reason: UnbindableReason,
    },
}

/// Why a caller-declared [`PortBinding`] could not be validated (fail-closed). The
/// compiler never coerces a type or synthesizes a pointer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnbindableReason {
    /// `from_step` or `to_step` is out of range for the step list.
    StepIndexOutOfRange { index: usize, len: usize },
    /// `from_step >= to_step`: a backward or self edge would need the reorder the
    /// compiler refuses; only forward dataflow is honest.
    BackwardEdge { from_step: usize, to_step: usize },
    /// The step has no typed port surface: a provider operation (no per-op schema) or an
    /// affordance whose schema is the opaque `{type:object}` with no declared
    /// `properties` (the shipped state -- nothing binds until typed outputs are declared).
    UntypedPort { step: usize },
    /// `output_pointer` names a property absent from the source's `output_schema.properties`.
    MissingOutputPointer { pointer: String },
    /// `input_field` names a property absent from the target's `input_schema.properties`.
    MissingInputField { field: String },
    /// Both ports are typed leaves but their JSON-Schema `type` strings differ.
    TypeMismatch {
        output_type: String,
        input_type: String,
    },
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

/// A caller-DECLARED dataflow edge: wire step `from_step`'s named output to step
/// `to_step`'s named input. The typed-edge analogue of [`compile_sequence`]'s
/// caller-supplied ORDER -- the caller declares it, the compiler only VALIDATES it (never
/// infers/synthesizes an edge). v1 pointer grammar: a FLAT top-level property name in the
/// affordance's schema `properties` map.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PortBinding {
    pub from_step: usize,
    pub output_pointer: String,
    pub to_step: usize,
    pub input_field: String,
}

/// A [`PortBinding`] the compiler VALIDATED: forward-only, both ports typed, and the
/// JSON-Schema `type` strings agree (`port_type`). Shell-agnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ValidatedBinding {
    pub from_step: usize,
    pub output_pointer: String,
    pub to_step: usize,
    pub input_field: String,
    pub port_type: String,
}

/// A shell-agnostic compiled PIPELINE: the ordered, gated steps (identical to
/// [`compile_sequence`]) PLUS the caller-declared, compiler-VALIDATED dataflow edges.
///
/// NOTE: `bindings` is a validated edge LIST, not yet a conflict-free graph -- duplicate
/// edges into the same `(to_step, input_field)` are NOT yet rejected (a deferred unbindable
/// case), and required-input COMPLETENESS is not checked. Runtime value-passing/execution
/// of the pipeline is also deferred; this is PURE planning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompiledPipeline {
    pub composition: CompositionKind,
    pub steps: Vec<CompiledStep>,
    pub bindings: Vec<ValidatedBinding>,
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

/// Which side of an affordance schema a port resolves into.
enum PortSide<'a> {
    Output(&'a str),
    Input(&'a str),
}

/// Resolve the JSON-Schema `type` string at a named port of a step's affordance.
/// Fail-closed: a provider step or an opaque-schema affordance is `UntypedPort`; a name
/// absent from `properties` is Missing{Output,Input}; a property with no `type` keyword is
/// treated as missing (we never invent a type). The step resolved in [`compile_sequence`]
/// already, so [`locate`] returns exactly one match.
fn port_type_at(sub: &SubGoal, side: PortSide, step: usize) -> Result<String, UnbindableReason> {
    let located = locate(sub.manifest, &sub.intent);
    let descriptor = located.iter().find_map(|l| match l {
        Located::Affordance { method, .. } => Some(*method),
        Located::Provider { .. } => None,
    });
    // A provider operation (or no affordance) has no typed port surface.
    let Some(descriptor) = descriptor else {
        return Err(UnbindableReason::UntypedPort { step });
    };
    let (schema, name) = match side {
        PortSide::Output(p) => (&descriptor.output_schema, p),
        PortSide::Input(f) => (&descriptor.input_schema, f),
    };
    // The opaque shipped case: `{type:object}` with no `properties` -> no typed ports.
    let Some(properties) = schema
        .as_ref()
        .and_then(|s| s.get("properties"))
        .and_then(|p| p.as_object())
    else {
        return Err(UnbindableReason::UntypedPort { step });
    };
    let missing = || match side {
        PortSide::Output(p) => UnbindableReason::MissingOutputPointer {
            pointer: p.to_string(),
        },
        PortSide::Input(f) => UnbindableReason::MissingInputField {
            field: f.to_string(),
        },
    };
    let type_str = properties
        .get(name)
        .and_then(|field| field.get("type"))
        .and_then(|t| t.as_str())
        .ok_or_else(missing)?;
    Ok(type_str.to_string())
}

/// Validate ONE caller-declared [`PortBinding`] against the steps' typed schemas,
/// returning the agreed port `type`. Fail-closed, never inferring or coercing.
fn validate_binding(steps: &[SubGoal], binding: &PortBinding) -> Result<String, UnbindableReason> {
    let len = steps.len();
    if binding.from_step >= len {
        return Err(UnbindableReason::StepIndexOutOfRange {
            index: binding.from_step,
            len,
        });
    }
    if binding.to_step >= len {
        return Err(UnbindableReason::StepIndexOutOfRange {
            index: binding.to_step,
            len,
        });
    }
    // Forward-only: a later output cannot feed an earlier input without the reorder the
    // compiler refuses (mirrors the no-derived-order honesty).
    if binding.from_step >= binding.to_step {
        return Err(UnbindableReason::BackwardEdge {
            from_step: binding.from_step,
            to_step: binding.to_step,
        });
    }
    let output_type = port_type_at(
        &steps[binding.from_step],
        PortSide::Output(&binding.output_pointer),
        binding.from_step,
    )?;
    let input_type = port_type_at(
        &steps[binding.to_step],
        PortSide::Input(&binding.input_field),
        binding.to_step,
    )?;
    if output_type != input_type {
        return Err(UnbindableReason::TypeMismatch {
            output_type,
            input_type,
        });
    }
    Ok(output_type)
}

/// Compile a caller-ordered sequence of goals into a typed dataflow PIPELINE: the
/// ordered, gated steps (identical to [`compile_sequence`]) PLUS a set of caller-declared,
/// compiler-VALIDATED port bindings (step output -> step input).
///
/// A strict additive superset of [`compile_sequence`]: with no `bindings` it is exactly
/// that. The steps are resolved + gated first (fail-closed atomic via
/// [`IntentError::Step`]); only then are the bindings validated, fail-closed on the FIRST
/// unbindable edge via [`IntentError::Unbindable`] (out-of-range / backward / untyped port
/// / missing pointer or field / type mismatch) -- no partial pipeline. The compiler NEVER
/// infers a binding: the caller declares both the order AND the edges; the compiler only
/// validates + gates. Affordance-only (provider operations have no typed port surface).
pub fn compile_pipeline(
    steps: &[SubGoal],
    bindings: &[PortBinding],
) -> Result<CompiledPipeline, IntentError> {
    // Phase (a): the ordered, gated steps -- reuse compile_sequence verbatim.
    let plan = compile_sequence(steps)?;

    // Phase (b): validate each caller-declared binding, fail-closed on the first.
    let mut validated = Vec::with_capacity(bindings.len());
    for (i, binding) in bindings.iter().enumerate() {
        let port_type = validate_binding(steps, binding)
            .map_err(|reason| IntentError::Unbindable { binding: i, reason })?;
        validated.push(ValidatedBinding {
            from_step: binding.from_step,
            output_pointer: binding.output_pointer.clone(),
            to_step: binding.to_step,
            input_field: binding.input_field.clone(),
            port_type,
        });
    }

    Ok(CompiledPipeline {
        composition: plan.composition,
        steps: plan.steps,
        bindings: validated,
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

    #[test]
    fn plan_and_error_serialize_to_shell_agnostic_json() {
        // The full shell-agnostic data contract: BOTH a success plan AND a failure
        // serialize to JSON a shell renders without any runtime types in scope.
        let inspector = shipped("capsule-inspector");

        let plan = compile(&inspector, &intent("list", serde_json::json!({}))).unwrap();
        let plan_json = serde_json::to_value(&plan).unwrap();
        assert_eq!(plan_json["composition"], "single_step");
        assert_eq!(plan_json["steps"][0]["kind"], "affordance");
        assert_eq!(plan_json["steps"][0]["operation"], "list");

        // The failure path is data too: externally tagged, snake_case variant name.
        let err = compile(&inspector, &intent("nope", serde_json::json!({}))).unwrap_err();
        let err_json = serde_json::to_value(&err).unwrap();
        assert_eq!(err_json["unresolvable"]["operation"], "nope");
    }

    // ── Typed dataflow binding (compile_pipeline) ──────────────────

    /// A manifest with ONE affordance whose input/output schemas carry typed `properties`
    /// (the surface a port binding type-checks against).
    fn typed_manifest(
        name: &str,
        op: &str,
        input_props: Value,
        output_props: Value,
    ) -> CapsuleManifest {
        let manifest: CapsuleManifest = serde_json::from_value(serde_json::json!({
            "schema": "elastos.capsule/v1", "version": "0.1.0", "name": name,
            "role": "app", "type": "wasm", "entrypoint": "x.wasm",
            "interfaces": [{
                "id": "i", "version": "0.1.0",
                "methods": [{
                    "id": "m", "operation": op, "risk": "read", "approval": "none", "audit": "none",
                    "input_schema": {"type": "object", "properties": input_props},
                    "output_schema": {"type": "object", "properties": output_props},
                }]
            }]
        }))
        .unwrap();
        manifest.validate().expect("typed manifest is valid");
        manifest
    }

    #[test]
    fn pipeline_empty_bindings_equals_compile_sequence() {
        // The strict additive superset: no edges => exactly compile_sequence's steps.
        let src = typed_manifest(
            "src",
            "fetch",
            serde_json::json!({}),
            serde_json::json!({"cid": {"type": "string"}}),
        );
        let steps = [SubGoal {
            manifest: &src,
            intent: intent("fetch", serde_json::json!({})),
        }];
        let pipeline = compile_pipeline(&steps, &[]).unwrap();
        let plan = compile_sequence(&steps).unwrap();
        assert_eq!(pipeline.steps, plan.steps);
        assert!(pipeline.bindings.is_empty());
    }

    #[test]
    fn pipeline_binds_string_to_string_over_typed_manifests() {
        // step0 outputs {cid: string}; step1 inputs {cid: string}; the edge type-checks.
        let src = typed_manifest(
            "src",
            "fetch",
            serde_json::json!({}),
            serde_json::json!({"cid": {"type": "string"}}),
        );
        let dst = typed_manifest(
            "dst",
            "open",
            serde_json::json!({"cid": {"type": "string"}}),
            serde_json::json!({}),
        );
        let steps = [
            SubGoal {
                manifest: &src,
                intent: intent("fetch", serde_json::json!({})),
            },
            SubGoal {
                manifest: &dst,
                intent: intent("open", serde_json::json!({})),
            },
        ];
        let binding = PortBinding {
            from_step: 0,
            output_pointer: "cid".to_string(),
            to_step: 1,
            input_field: "cid".to_string(),
        };
        let pipeline = compile_pipeline(&steps, std::slice::from_ref(&binding)).unwrap();
        assert_eq!(pipeline.bindings.len(), 1);
        assert_eq!(pipeline.bindings[0].port_type, "string");
        assert_eq!(pipeline.bindings[0].from_step, 0);
    }

    #[test]
    fn pipeline_type_mismatch_fails_closed_with_index_and_reason() {
        let src = typed_manifest(
            "src",
            "fetch",
            serde_json::json!({}),
            serde_json::json!({"cid": {"type": "string"}}),
        );
        let dst = typed_manifest(
            "dst",
            "open",
            serde_json::json!({"n": {"type": "number"}}),
            serde_json::json!({}),
        );
        let steps = [
            SubGoal {
                manifest: &src,
                intent: intent("fetch", serde_json::json!({})),
            },
            SubGoal {
                manifest: &dst,
                intent: intent("open", serde_json::json!({})),
            },
        ];
        let binding = PortBinding {
            from_step: 0,
            output_pointer: "cid".to_string(),
            to_step: 1,
            input_field: "n".to_string(),
        };
        let err = compile_pipeline(&steps, std::slice::from_ref(&binding)).unwrap_err();
        // mustFix: assert BOTH the 0-based edge index AND the reason payload.
        assert_eq!(
            err,
            IntentError::Unbindable {
                binding: 0,
                reason: UnbindableReason::TypeMismatch {
                    output_type: "string".to_string(),
                    input_type: "number".to_string(),
                },
            }
        );
    }

    #[test]
    fn pipeline_missing_pointer_and_field_fail_closed() {
        let src = typed_manifest(
            "src",
            "fetch",
            serde_json::json!({}),
            serde_json::json!({"cid": {"type": "string"}}),
        );
        let dst = typed_manifest(
            "dst",
            "open",
            serde_json::json!({"cid": {"type": "string"}}),
            serde_json::json!({}),
        );
        let steps = [
            SubGoal {
                manifest: &src,
                intent: intent("fetch", serde_json::json!({})),
            },
            SubGoal {
                manifest: &dst,
                intent: intent("open", serde_json::json!({})),
            },
        ];
        let bad_out = PortBinding {
            from_step: 0,
            output_pointer: "nope".to_string(),
            to_step: 1,
            input_field: "cid".to_string(),
        };
        assert!(matches!(
            compile_pipeline(&steps, std::slice::from_ref(&bad_out)).unwrap_err(),
            IntentError::Unbindable {
                binding: 0,
                reason: UnbindableReason::MissingOutputPointer { .. }
            }
        ));
        let bad_in = PortBinding {
            from_step: 0,
            output_pointer: "cid".to_string(),
            to_step: 1,
            input_field: "nope".to_string(),
        };
        assert!(matches!(
            compile_pipeline(&steps, std::slice::from_ref(&bad_in)).unwrap_err(),
            IntentError::Unbindable {
                binding: 0,
                reason: UnbindableReason::MissingInputField { .. }
            }
        ));
    }

    #[test]
    fn pipeline_backward_and_out_of_range_fail_closed() {
        let m = typed_manifest(
            "m",
            "fetch",
            serde_json::json!({"cid": {"type": "string"}}),
            serde_json::json!({"cid": {"type": "string"}}),
        );
        let steps = [
            SubGoal {
                manifest: &m,
                intent: intent("fetch", serde_json::json!({})),
            },
            SubGoal {
                manifest: &m,
                intent: intent("fetch", serde_json::json!({})),
            },
        ];
        let backward = PortBinding {
            from_step: 1,
            output_pointer: "cid".to_string(),
            to_step: 0,
            input_field: "cid".to_string(),
        };
        assert!(matches!(
            compile_pipeline(&steps, std::slice::from_ref(&backward)).unwrap_err(),
            IntentError::Unbindable {
                binding: 0,
                reason: UnbindableReason::BackwardEdge { .. }
            }
        ));
        let oor = PortBinding {
            from_step: 0,
            output_pointer: "cid".to_string(),
            to_step: 9,
            input_field: "cid".to_string(),
        };
        assert!(matches!(
            compile_pipeline(&steps, std::slice::from_ref(&oor)).unwrap_err(),
            IntentError::Unbindable {
                binding: 0,
                reason: UnbindableReason::StepIndexOutOfRange { index: 9, .. }
            }
        ));
    }

    #[test]
    fn pipeline_opaque_shipped_output_is_untyped_port() {
        // Over the REAL shipped inspector (output_schema {type:object}, no properties):
        // there is no typed output to bind -> UntypedPort (honest, not a wrong-pointer lie).
        let inspector = shipped("capsule-inspector");
        let steps = [
            SubGoal {
                manifest: &inspector,
                intent: intent("list", serde_json::json!({})),
            },
            SubGoal {
                manifest: &inspector,
                intent: intent("view", serde_json::json!({ "target": "x" })),
            },
        ];
        let binding = PortBinding {
            from_step: 0,
            output_pointer: "anything".to_string(),
            to_step: 1,
            input_field: "target".to_string(),
        };
        assert!(matches!(
            compile_pipeline(&steps, std::slice::from_ref(&binding)).unwrap_err(),
            IntentError::Unbindable {
                binding: 0,
                reason: UnbindableReason::UntypedPort { step: 0 }
            }
        ));
    }

    #[test]
    fn pipeline_provider_endpoint_is_untyped_port_not_a_panic() {
        // A provider operation has no per-op schema: a binding touching it is UntypedPort,
        // never a panic / None-unwrap (mustFix).
        let src = typed_manifest(
            "src",
            "fetch",
            serde_json::json!({}),
            serde_json::json!({"cid": {"type": "string"}}),
        );
        let key = shipped("key-provider");
        let steps = [
            SubGoal {
                manifest: &src,
                intent: intent("fetch", serde_json::json!({})),
            },
            SubGoal {
                manifest: &key,
                intent: intent("release", serde_json::json!({})),
            },
        ];
        let binding = PortBinding {
            from_step: 0,
            output_pointer: "cid".to_string(),
            to_step: 1,
            input_field: "cid".to_string(),
        };
        assert!(matches!(
            compile_pipeline(&steps, std::slice::from_ref(&binding)).unwrap_err(),
            IntentError::Unbindable {
                binding: 0,
                reason: UnbindableReason::UntypedPort { step: 1 }
            }
        ));
    }

    #[test]
    fn pipeline_step_error_aborts_before_binding_pass() {
        // An unresolvable step fails as Step{index,source} BEFORE the binding pass runs.
        let m = typed_manifest(
            "m",
            "fetch",
            serde_json::json!({}),
            serde_json::json!({"cid": {"type": "string"}}),
        );
        let steps = [
            SubGoal {
                manifest: &m,
                intent: intent("fetch", serde_json::json!({})),
            },
            SubGoal {
                manifest: &m,
                intent: intent("nope", serde_json::json!({})),
            },
        ];
        let binding = PortBinding {
            from_step: 0,
            output_pointer: "cid".to_string(),
            to_step: 1,
            input_field: "cid".to_string(),
        };
        let err = compile_pipeline(&steps, std::slice::from_ref(&binding)).unwrap_err();
        assert!(matches!(err, IntentError::Step { index: 1, .. }));
    }

    #[test]
    fn pipeline_and_unbindable_serialize_to_shell_agnostic_json() {
        let src = typed_manifest(
            "src",
            "fetch",
            serde_json::json!({}),
            serde_json::json!({"cid": {"type": "string"}}),
        );
        let dst = typed_manifest(
            "dst",
            "open",
            serde_json::json!({"cid": {"type": "string"}}),
            serde_json::json!({}),
        );
        let steps = [
            SubGoal {
                manifest: &src,
                intent: intent("fetch", serde_json::json!({})),
            },
            SubGoal {
                manifest: &dst,
                intent: intent("open", serde_json::json!({})),
            },
        ];
        let binding = PortBinding {
            from_step: 0,
            output_pointer: "cid".to_string(),
            to_step: 1,
            input_field: "cid".to_string(),
        };
        let pipeline = compile_pipeline(&steps, std::slice::from_ref(&binding)).unwrap();
        let pj = serde_json::to_value(&pipeline).unwrap();
        assert_eq!(pj["composition"], "multi_step");
        assert_eq!(pj["bindings"][0]["port_type"], "string");
        assert_eq!(pj["bindings"][0]["input_field"], "cid");

        let err = IntentError::Unbindable {
            binding: 2,
            reason: UnbindableReason::TypeMismatch {
                output_type: "string".into(),
                input_type: "number".into(),
            },
        };
        let ej = serde_json::to_value(&err).unwrap();
        assert_eq!(ej["unbindable"]["binding"], 2);
        assert_eq!(
            ej["unbindable"]["reason"]["type_mismatch"]["output_type"],
            "string"
        );
    }
}
