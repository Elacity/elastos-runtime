//! ddrm-plan-runner — the runtime-core executor for the dDRM `drm/open` plan.
//!
//! drm-provider (Day 67) emits a typed, executable `DrmOpenPlanV1` (`status:
//! "planned"`) that *declares* the canonical open sequence and the binding edges
//! between steps — but holds no authority and invokes nothing. Until now the only
//! thing that actually *followed* that plan was the hand-written consumer smoke
//! orchestrator: it read the order + edges off the plan, but the walk itself was
//! inline literal code.
//!
//! This crate is that walk, extracted into the runtime core. Given a plan, it:
//!
//!   1. validates it (schema, `planned` status, the canonical
//!      `rights_check < key_release < decrypt_session` order, every binding names
//!      real steps and identities);
//!   2. seeds the plan-level identities the virtual `drm_open` step produces
//!      (`content_id`, `object_cid`, `viewer_interface`);
//!   3. walks the steps IN ORDER, and for each step gathers the binding edges that
//!      feed it — pulling each declared artifact from the prior steps' outputs and
//!      threading it into the field name the PLAN declares — then asks the injected
//!      [`StepRunner`] to run the step with exactly those threaded inputs;
//!   4. fails closed if a step needs an artifact that has not been produced yet
//!      (out-of-order / a prior step silently failed) or if a step runs but does
//!      not produce the artifact the plan says it must emit.
//!
//! It mirrors PC2's open sequencer, where each stage is gated on the prior one's
//! output: `authenticate -> requireSecureViewSession` (resurrect the session view —
//! `src/api/middleware/secureViewSession.ts:61`), then `recoverCEKEnvelope`, whose
//! access gate is `hasAccessByContentId(ownerAddress, kid)` and which only then
//! recovers + unwraps the CEK in-boundary (`src/api/media.ts:1163`, `:1196`). A
//! missing/failed prior stage short-circuits the whole open. Here that gating is
//! data-driven off the plan instead of hard-coded middleware order.
//!
//! ## No authority
//!
//! The executor performs no I/O and holds no capability. The ONLY thing that can
//! reach a provider is the [`StepRunner`] the runtime injects. The CEK, key
//! material, wallet/chain RPC, etc. never appear in this crate — exactly the
//! `blocked_authority` set the plan advertises.

use serde_json::Value;
use std::collections::BTreeMap;

/// The schema the drm-provider stamps on every open plan (matches drm-provider's
/// `DRM_OPEN_PLAN_SCHEMA`).
pub const DRM_OPEN_PLAN_SCHEMA: &str = "elastos.drm.open.plan/v1";

/// The virtual source step for plan-level identities (`content_id` etc.). It is not
/// a provider call — drm-provider produces these when it emits the plan — so the
/// executor seeds them before the walk rather than running them.
pub const DRM_OPEN_SOURCE: &str = "drm_open";

/// The canonical steps whose relative order the runtime must never reorder: the
/// rights check gates the key release, which gates the decrypt session.
const RIGHTS_STEP: &str = "rights_check";
const KEY_STEP: &str = "key_release";
const DECRYPT_STEP: &str = "decrypt_session";

/// One step of the plan as the runtime sees it. Provider/operation are `None` for
/// runtime-owned events (e.g. `release_receipt`, `audit`) the executor never invokes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanStep {
    pub name: String,
    pub provider: Option<String>,
    pub operation: Option<String>,
}

impl PlanStep {
    /// A step the executor drives through the runner (has both a provider and an
    /// operation). Runtime-event steps are walked for ordering but never invoked.
    pub fn is_provider_call(&self) -> bool {
        self.provider.is_some() && self.operation.is_some()
    }
}

/// A binding edge: the artifact named `produces`, emitted by `from_step`, is threaded
/// into `into_step`'s request under `into_field`. Field names match the shared
/// contracts (pinned by `chain_seam_tests`), so a rename fails loudly downstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanBinding {
    pub from_step: String,
    pub produces: String,
    pub into_step: String,
    pub into_field: String,
}

/// The inputs the executor hands a step: the binding-threaded fields (already pulled
/// from prior steps' outputs, keyed by the plan's `into_field`) plus the full
/// artifact context (everything produced so far, keyed by `produces`) for
/// non-binding state a handler legitimately needs (e.g. sealed key material that
/// rides alongside the release receipt). A handler should read its rights/release
/// receipts from [`threaded`](Self::threaded) so it binds where the PLAN says.
pub struct StepInputs<'a> {
    pub step: &'a PlanStep,
    threaded: &'a BTreeMap<String, Value>,
    context: &'a BTreeMap<String, Value>,
}

impl StepInputs<'_> {
    /// The artifact the plan threaded into `into_field` for this step, if any.
    pub fn threaded(&self, into_field: &str) -> Option<&Value> {
        self.threaded.get(into_field)
    }

    /// Require a plan-threaded input — fail closed (with the step name) when the
    /// edge that should have delivered it is missing or mis-named.
    pub fn require_threaded(&self, into_field: &str) -> Result<&Value, String> {
        self.threaded(into_field).ok_or_else(|| {
            format!(
                "step `{}` is missing required input `{into_field}` — the plan declared no edge delivering it",
                self.step.name
            )
        })
    }

    /// A previously produced artifact, keyed by its `produces` name. For state the
    /// plan does not model as a binding edge (e.g. sealed material).
    pub fn artifact(&self, produces: &str) -> Option<&Value> {
        self.context.get(produces)
    }

    /// All binding-threaded inputs for this step.
    pub fn threaded_fields(&self) -> &BTreeMap<String, Value> {
        self.threaded
    }
}

/// The capability-injected step executor. This is the ONLY thing that can touch a
/// provider; the plan executor itself holds no authority. A handler builds the
/// step's request from its threaded inputs, performs the (capability-scoped)
/// provider call, and returns the artifacts the step produced, keyed by the
/// `produces` names the plan's outgoing bindings declare for the step. A step with
/// no outgoing binding may return an empty map (e.g. a runtime no-op for content
/// fetch / render).
pub trait StepRunner {
    fn run_step(&mut self, inputs: &StepInputs) -> Result<BTreeMap<String, Value>, String>;
}

/// What an execution produced: the steps that ran (in order) and the full artifact
/// context at the end (keyed by `produces`).
#[derive(Debug, Clone)]
pub struct ExecutionReport {
    pub steps_run: Vec<String>,
    pub artifacts: BTreeMap<String, Value>,
}

impl ExecutionReport {
    pub fn artifact(&self, produces: &str) -> Option<&Value> {
        self.artifacts.get(produces)
    }
}

/// A parsed + validated `DrmOpenPlanV1`, ready to execute.
#[derive(Debug, Clone)]
pub struct DrmOpenPlan {
    pub content_id: String,
    pub object_cid: String,
    pub viewer_interface: String,
    pub steps: Vec<PlanStep>,
    pub bindings: Vec<PlanBinding>,
}

impl DrmOpenPlan {
    /// Parse + validate the plan JSON the drm-provider emits. Fails closed on a wrong
    /// schema, a non-`planned` status, empty steps, a binding that names a step or
    /// identity not in the plan, or a canonical-order violation.
    pub fn parse(plan: &Value) -> Result<Self, String> {
        let schema = plan["schema"].as_str().unwrap_or_default();
        if schema != DRM_OPEN_PLAN_SCHEMA {
            return Err(format!(
                "not a dDRM open plan: schema `{schema}` != `{DRM_OPEN_PLAN_SCHEMA}`"
            ));
        }
        let status = plan["status"].as_str().unwrap_or_default();
        if status != "planned" {
            return Err(format!(
                "refusing to execute a non-planned plan (status `{status}`)"
            ));
        }

        let content_id = required_str(plan, "content_id")?;
        let object_cid = required_str(plan, "object_cid")?;
        // The shared contract requires the rights receipt's content_id to equal the
        // decrypt object_cid — one identity, two field names. Enforce it here so a
        // plan that splits them never executes.
        if content_id != object_cid {
            return Err(format!(
                "plan identity split: content_id `{content_id}` != object_cid `{object_cid}`"
            ));
        }
        let viewer_interface = required_str(plan, "viewer_interface")?;

        let steps = parse_steps(plan)?;
        let bindings = parse_bindings(plan)?;

        // Every binding must name steps that exist (or the virtual drm_open source),
        // and drm_open may only "produce" the plan-level identities we can seed.
        let step_names: Vec<&str> = steps.iter().map(|s| s.name.as_str()).collect();
        for b in &bindings {
            if b.from_step != DRM_OPEN_SOURCE && !step_names.contains(&b.from_step.as_str()) {
                return Err(format!("binding from unknown step `{}`", b.from_step));
            }
            if !step_names.contains(&b.into_step.as_str()) {
                return Err(format!("binding into unknown step `{}`", b.into_step));
            }
            if b.from_step == DRM_OPEN_SOURCE && seed_identity(&content_id, &object_cid, &viewer_interface, &b.produces).is_none() {
                return Err(format!(
                    "plan says `{DRM_OPEN_SOURCE}` produces unknown identity `{}`",
                    b.produces
                ));
            }
        }

        Self::assert_canonical_order(&steps)?;

        Ok(Self {
            content_id,
            object_cid,
            viewer_interface,
            steps,
            bindings,
        })
    }

    /// The rights check must precede the key release, which must precede the decrypt
    /// session. A plan that reorders these never executes.
    fn assert_canonical_order(steps: &[PlanStep]) -> Result<(), String> {
        let pos = |name: &str| steps.iter().position(|s| s.name == name);
        let rights = pos(RIGHTS_STEP).ok_or("plan missing rights_check step")?;
        let key = pos(KEY_STEP).ok_or("plan missing key_release step")?;
        let decrypt = pos(DECRYPT_STEP).ok_or("plan missing decrypt_session step")?;
        if rights < key && key < decrypt {
            Ok(())
        } else {
            Err(format!(
                "plan steps out of canonical order: rights_check={rights} key_release={key} decrypt_session={decrypt}"
            ))
        }
    }

    /// Walk the steps in order, threading each binding edge and gating each step on
    /// its declared inputs. The runtime injects `runner` — the only thing that can
    /// reach a provider. Returns the produced artifacts, or the first fail-closed
    /// error.
    pub fn execute(&self, runner: &mut dyn StepRunner) -> Result<ExecutionReport, String> {
        let mut artifacts: BTreeMap<String, Value> = BTreeMap::new();
        let mut steps_run: Vec<String> = Vec::new();

        for step in &self.steps {
            // 1. Gather the edges feeding THIS step and thread each declared artifact
            //    into the field name the plan names. A required artifact that has not
            //    been produced yet means an out-of-order step or a silently-failed
            //    prior step — fail closed rather than calling the provider blind.
            let mut threaded: BTreeMap<String, Value> = BTreeMap::new();
            for b in self.bindings.iter().filter(|b| b.into_step == step.name) {
                let artifact = self.resolve_artifact(&artifacts, &b.from_step, &b.produces)?;
                threaded.insert(b.into_field.clone(), artifact);
            }

            // 2. Provider-call steps go through the injected runner; runtime-event
            //    steps (no provider/operation) are walked for ordering only.
            if step.is_provider_call() {
                let inputs = StepInputs {
                    step,
                    threaded: &threaded,
                    context: &artifacts,
                };
                let produced = runner.run_step(&inputs)?;
                for (k, v) in produced {
                    artifacts.insert(k, v);
                }
            }

            // 3. Enforce the step's outgoing bindings: whatever the plan says this
            //    step PRODUCES must now exist, or the step silently dropped it.
            for b in self.bindings.iter().filter(|b| b.from_step == step.name) {
                if !artifacts.contains_key(&b.produces) {
                    return Err(format!(
                        "step `{}` did not produce its declared artifact `{}` (the `{}` edge is broken)",
                        step.name, b.produces, b.into_step
                    ));
                }
            }

            steps_run.push(step.name.clone());
        }

        Ok(ExecutionReport {
            steps_run,
            artifacts,
        })
    }

    /// Resolve a binding's source artifact: a plan-level identity (seeded from
    /// `drm_open`) or a prior provider step's output. Fails closed when a step needs
    /// an artifact that has not been produced yet.
    fn resolve_artifact(
        &self,
        artifacts: &BTreeMap<String, Value>,
        from_step: &str,
        produces: &str,
    ) -> Result<Value, String> {
        if from_step == DRM_OPEN_SOURCE {
            seed_identity(
                &self.content_id,
                &self.object_cid,
                &self.viewer_interface,
                produces,
            )
            .ok_or_else(|| format!("plan declares `{DRM_OPEN_SOURCE}` produces unknown identity `{produces}`"))
        } else {
            artifacts.get(produces).cloned().ok_or_else(|| {
                format!(
                    "broken plan edge: `{produces}` (from `{from_step}`) is not available yet — out-of-order or a prior step failed to produce it"
                )
            })
        }
    }
}

/// Map a `drm_open`-produced identity name to its plan-level value.
fn seed_identity(
    content_id: &str,
    object_cid: &str,
    viewer_interface: &str,
    produces: &str,
) -> Option<Value> {
    match produces {
        "content_id" => Some(Value::String(content_id.to_string())),
        "object_cid" => Some(Value::String(object_cid.to_string())),
        "viewer_interface" => Some(Value::String(viewer_interface.to_string())),
        _ => None,
    }
}

fn required_str(plan: &Value, field: &str) -> Result<String, String> {
    plan[field]
        .as_str()
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("plan is missing required field `{field}`"))
}

fn parse_steps(plan: &Value) -> Result<Vec<PlanStep>, String> {
    let arr = plan["steps"].as_array().ok_or("plan has no steps array")?;
    if arr.is_empty() {
        return Err("plan has an empty steps array".to_string());
    }
    let mut steps = Vec::with_capacity(arr.len());
    for s in arr {
        let name = s["step"]
            .as_str()
            .filter(|n| !n.is_empty())
            .ok_or("plan step is missing its `step` name")?
            .to_string();
        steps.push(PlanStep {
            name,
            provider: s["provider"].as_str().map(str::to_string),
            operation: s["operation"].as_str().map(str::to_string),
        });
    }
    Ok(steps)
}

fn parse_bindings(plan: &Value) -> Result<Vec<PlanBinding>, String> {
    let arr = plan["bindings"].as_array().ok_or("plan has no bindings array")?;
    let mut bindings = Vec::with_capacity(arr.len());
    for b in arr {
        let field = |k: &str| {
            b[k].as_str()
                .filter(|v| !v.is_empty())
                .map(str::to_string)
                .ok_or_else(|| format!("binding is missing `{k}`"))
        };
        bindings.push(PlanBinding {
            from_step: field("from_step")?,
            produces: field("produces")?,
            into_step: field("into_step")?,
            into_field: field("into_field")?,
        });
    }
    Ok(bindings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The canonical plan the drm-provider emits (mirrors `drm_open_sequence` +
    /// `drm_open_plan_bindings`). Tests mutate clones of this to model tampering.
    fn canonical_plan() -> Value {
        json!({
            "schema": DRM_OPEN_PLAN_SCHEMA,
            "status": "planned",
            "provider": "drm",
            "content_id": "bafycontent",
            "object_cid": "bafycontent",
            "viewer_interface": "elastos.viewer/document@1",
            "action": "view",
            "steps": [
                { "step": "content_status", "provider": "content", "operation": "status" },
                { "step": "content_fetch", "provider": "content", "operation": "fetch" },
                { "step": "rights_check", "provider": "rights", "operation": "has_access_by_content_id" },
                { "step": "key_release", "provider": "key", "operation": "release" },
                { "step": "decrypt_session", "provider": "decrypt", "operation": "open_session" },
                { "step": "render", "provider": "decrypt", "operation": "render" },
                { "step": "release_receipt", "owner": "runtime", "event": "release_receipt" },
                { "step": "audit", "owner": "runtime", "event": "protected_content.open.audit" }
            ],
            "bindings": [
                { "from_step": "drm_open", "produces": "content_id", "into_step": "rights_check", "into_field": "content_id" },
                { "from_step": "rights_check", "produces": "RightsDecisionReceiptV1", "into_step": "key_release", "into_field": "rights_receipt" },
                { "from_step": "key_release", "produces": "ReleaseReceiptV1", "into_step": "decrypt_session", "into_field": "release_receipt" },
                { "from_step": "drm_open", "produces": "object_cid", "into_step": "decrypt_session", "into_field": "object_cid" },
                { "from_step": "drm_open", "produces": "viewer_interface", "into_step": "decrypt_session", "into_field": "viewer_interface" }
            ]
        })
    }

    /// A scripted runner: records the threaded inputs each step received, emits the
    /// artifact the plan says the step produces, and (optionally) asserts the inputs
    /// it expected — so a mis-threaded / missing edge fails closed in-test too.
    #[derive(Default)]
    struct ScriptedRunner {
        calls: Vec<String>,
        seen_rights_receipt: Option<Value>,
        seen_release_receipt: Option<Value>,
        seen_object_cid: Option<Value>,
        seen_viewer_interface: Option<Value>,
        seen_content_id: Option<Value>,
        /// Steps for which the runner deliberately produces nothing (to model a
        /// silently-dropped artifact).
        skip_output_for: Vec<String>,
    }

    impl StepRunner for ScriptedRunner {
        fn run_step(&mut self, inputs: &StepInputs) -> Result<BTreeMap<String, Value>, String> {
            self.calls.push(inputs.step.name.clone());
            let mut out = BTreeMap::new();
            match inputs.step.name.as_str() {
                "rights_check" => {
                    self.seen_content_id = inputs.threaded("content_id").cloned();
                    if !self.skip_output_for.iter().any(|s| s == "rights_check") {
                        out.insert("RightsDecisionReceiptV1".to_string(), json!({ "allowed": true }));
                    }
                }
                "key_release" => {
                    // The handler MUST receive the rights receipt the plan threaded.
                    self.seen_rights_receipt = Some(inputs.require_threaded("rights_receipt")?.clone());
                    if !self.skip_output_for.iter().any(|s| s == "key_release") {
                        out.insert("ReleaseReceiptV1".to_string(), json!({ "status": "released" }));
                        out.insert("material".to_string(), json!({ "sealed_cek_b64": "..." }));
                    }
                }
                "decrypt_session" => {
                    self.seen_release_receipt = Some(inputs.require_threaded("release_receipt")?.clone());
                    self.seen_object_cid = Some(inputs.require_threaded("object_cid")?.clone());
                    self.seen_viewer_interface = Some(inputs.require_threaded("viewer_interface")?.clone());
                    // Non-binding state still reachable from the context.
                    inputs
                        .artifact("material")
                        .ok_or("decrypt_session lost the sealed material from key_release")?;
                    out.insert("decrypt_session".to_string(), json!({ "decision": "opened" }));
                }
                // content_status / content_fetch / render — runtime no-ops here.
                _ => {}
            }
            Ok(out)
        }
    }

    #[test]
    fn parses_the_canonical_plan() {
        let plan = DrmOpenPlan::parse(&canonical_plan()).expect("canonical plan parses");
        assert_eq!(plan.content_id, "bafycontent");
        assert_eq!(plan.steps.len(), 8);
        assert_eq!(plan.bindings.len(), 5);
    }

    #[test]
    fn valid_plan_drives_the_canonical_sequence_in_order() {
        let plan = DrmOpenPlan::parse(&canonical_plan()).unwrap();
        let mut runner = ScriptedRunner::default();
        let report = plan.execute(&mut runner).expect("canonical plan executes");
        // Provider-call steps ran in canonical order (runtime-event steps too, walked
        // for ordering); rights precedes key precedes decrypt.
        let r = runner.calls.iter().position(|s| s == "rights_check").unwrap();
        let k = runner.calls.iter().position(|s| s == "key_release").unwrap();
        let d = runner.calls.iter().position(|s| s == "decrypt_session").unwrap();
        assert!(r < k && k < d, "calls out of order: {:?}", runner.calls);
        assert!(report.artifact("decrypt_session").is_some());
    }

    #[test]
    fn threads_the_declared_binding_edges() {
        let plan = DrmOpenPlan::parse(&canonical_plan()).unwrap();
        let mut runner = ScriptedRunner::default();
        plan.execute(&mut runner).unwrap();
        // The rights receipt the rights step produced was threaded into key_release.
        assert_eq!(runner.seen_rights_receipt, Some(json!({ "allowed": true })));
        // The release receipt key_release produced was threaded into decrypt_session.
        assert_eq!(runner.seen_release_receipt, Some(json!({ "status": "released" })));
    }

    #[test]
    fn seeds_drm_open_identities_into_their_steps() {
        let plan = DrmOpenPlan::parse(&canonical_plan()).unwrap();
        let mut runner = ScriptedRunner::default();
        plan.execute(&mut runner).unwrap();
        assert_eq!(runner.seen_content_id, Some(json!("bafycontent")));
        assert_eq!(runner.seen_object_cid, Some(json!("bafycontent")));
        assert_eq!(
            runner.seen_viewer_interface,
            Some(json!("elastos.viewer/document@1"))
        );
    }

    #[test]
    fn rejects_a_non_planned_plan() {
        let mut p = canonical_plan();
        p["status"] = json!("blocked");
        let err = DrmOpenPlan::parse(&p).unwrap_err();
        assert!(err.contains("non-planned"), "{err}");
    }

    #[test]
    fn rejects_a_foreign_schema() {
        let mut p = canonical_plan();
        p["schema"] = json!("elastos.something.else/v1");
        assert!(DrmOpenPlan::parse(&p).is_err());
    }

    #[test]
    fn rejects_an_identity_split() {
        let mut p = canonical_plan();
        p["object_cid"] = json!("bafyOTHER");
        let err = DrmOpenPlan::parse(&p).unwrap_err();
        assert!(err.contains("identity split"), "{err}");
    }

    #[test]
    fn rejects_out_of_canonical_order_at_parse() {
        let mut p = canonical_plan();
        // Swap rights_check and key_release positions.
        let steps = p["steps"].as_array_mut().unwrap();
        steps.swap(2, 3);
        let err = DrmOpenPlan::parse(&p).unwrap_err();
        assert!(err.contains("out of canonical order"), "{err}");
    }

    #[test]
    fn rejects_a_binding_to_an_unknown_step() {
        let mut p = canonical_plan();
        p["bindings"].as_array_mut().unwrap().push(json!({
            "from_step": "rights_check", "produces": "X", "into_step": "ghost_step", "into_field": "x"
        }));
        assert!(DrmOpenPlan::parse(&p).is_err());
    }

    #[test]
    fn rejects_drm_open_producing_an_unknown_identity() {
        let mut p = canonical_plan();
        p["bindings"].as_array_mut().unwrap().push(json!({
            "from_step": "drm_open", "produces": "wallet_key", "into_step": "rights_check", "into_field": "wallet_key"
        }));
        let err = DrmOpenPlan::parse(&p).unwrap_err();
        assert!(err.contains("unknown identity"), "{err}");
    }

    #[test]
    fn a_renamed_edge_field_fails_closed() {
        // Rename the rights->key edge's target field: the executor threads the receipt
        // into `bogus`, so the key handler's `require_threaded("rights_receipt")` fails.
        let mut p = canonical_plan();
        for b in p["bindings"].as_array_mut().unwrap() {
            if b["into_step"] == json!("key_release") {
                b["into_field"] = json!("bogus");
            }
        }
        let plan = DrmOpenPlan::parse(&p).unwrap();
        let mut runner = ScriptedRunner::default();
        let err = plan.execute(&mut runner).unwrap_err();
        assert!(err.contains("rights_receipt"), "{err}");
    }

    #[test]
    fn a_step_that_drops_its_declared_artifact_fails_closed() {
        // rights_check runs but produces nothing -> the rights->key edge is broken and
        // the executor refuses to proceed (before key_release ever runs).
        let plan = DrmOpenPlan::parse(&canonical_plan()).unwrap();
        let mut runner = ScriptedRunner {
            skip_output_for: vec!["rights_check".to_string()],
            ..Default::default()
        };
        let err = plan.execute(&mut runner).unwrap_err();
        assert!(err.contains("did not produce"), "{err}");
        assert!(
            !runner.calls.iter().any(|s| s == "key_release"),
            "key_release must not run after the rights edge broke"
        );
    }

    #[test]
    fn a_backward_binding_fails_closed_at_execution() {
        // A plan whose order passes the rights<key<decrypt check but whose edge feeds an
        // EARLIER step from a LATER one: rights_check is told to consume key_release's
        // receipt. At rights_check time that artifact does not exist yet -> fail closed.
        let mut p = canonical_plan();
        p["bindings"].as_array_mut().unwrap().push(json!({
            "from_step": "key_release", "produces": "ReleaseReceiptV1", "into_step": "rights_check", "into_field": "premature"
        }));
        let plan = DrmOpenPlan::parse(&p).unwrap();
        let mut runner = ScriptedRunner::default();
        let err = plan.execute(&mut runner).unwrap_err();
        assert!(err.contains("out-of-order") || err.contains("not available yet"), "{err}");
    }

    /// A runner that refuses every call — models the executor holding no authority of
    /// its own: without an injected capability it cannot drive a single provider step.
    struct DenyAllRunner;
    impl StepRunner for DenyAllRunner {
        fn run_step(&mut self, inputs: &StepInputs) -> Result<BTreeMap<String, Value>, String> {
            Err(format!("no capability injected for `{}`", inputs.step.name))
        }
    }

    #[test]
    fn executor_holds_no_authority_without_an_injected_runner() {
        let plan = DrmOpenPlan::parse(&canonical_plan()).unwrap();
        let mut runner = DenyAllRunner;
        // The very first provider-call step (content_status) cannot proceed.
        let err = plan.execute(&mut runner).unwrap_err();
        assert!(err.contains("no capability injected"), "{err}");
    }
}
