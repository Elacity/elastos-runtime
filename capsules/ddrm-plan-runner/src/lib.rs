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
/// runtime-owned events (e.g. `release_receipt`, `audit`) the executor never invokes —
/// those carry an `event` name the runtime HOST emits after the provider steps run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanStep {
    pub name: String,
    pub provider: Option<String>,
    pub operation: Option<String>,
    /// The runtime-event this step emits (e.g. `release_receipt`,
    /// `protected_content.open.audit`), for `owner: runtime` steps with no provider.
    pub event: Option<String>,
}

impl PlanStep {
    /// A step the executor drives through the runner (has both a provider and an
    /// operation). Runtime-event steps are walked for ordering but never invoked.
    pub fn is_provider_call(&self) -> bool {
        self.provider.is_some() && self.operation.is_some()
    }

    /// A runtime-owned event step (no provider, carries an `event`) — the host emits it
    /// after the provider chain runs; the executor only walks it for ordering.
    pub fn is_runtime_event(&self) -> bool {
        self.provider.is_none() && self.event.is_some()
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

/// A single injected provider capability. The runtime hands the core one of these per
/// provider it is authorized to drive — the runtime-core analogue of PC2's
/// per-request `BackendSessionView` (`secureViewSession.ts:124` resurrects it, then
/// `media.ts:1207` threads it into `recoverCEKEnvelope`): a stage never opens its own
/// connection, it uses the handle it was given. The handle builds its step's request
/// from the executor-threaded inputs and invokes the provider; it produces the
/// artifacts the plan's outgoing bindings name. The handle is the boundary at which
/// authority enters — the [`RuntimeStepRunner`] over it holds none.
pub trait ProviderHandle {
    /// The provider role this handle services — matches a plan step's `provider`
    /// (`rights`/`key`/`decrypt`/…), the normalized form of `next_required_providers`.
    fn provider(&self) -> &str;

    /// Drive ONE plan step for this provider, given the executor's threaded inputs +
    /// context. Returns the artifacts the step produced (keyed by `produces`).
    fn run(&mut self, inputs: &StepInputs) -> Result<BTreeMap<String, Value>, String>;
}

/// The runtime-core `StepRunner`: it holds NO authority of its own — it walks each
/// plan step to the injected [`ProviderHandle`] registered for that step's provider.
/// This is what the trusted core wires to the real provider rail; the consumer smoke
/// injects capsule-backed handles into the SAME type (no second code path).
///
/// Fail-closed construction ([`RuntimeStepRunner::new`]):
///   * every provider the plan's `next_required_providers` names MUST have an injected
///     handle — no ambient default, the core cannot fabricate a missing capability;
///   * no STRAY handle may be injected for a provider the plan does not require — a
///     capability the plan never authorized can never be invoked.
///
/// At execution a provider-call step whose provider has no handle and is not required
/// (e.g. the `content` status/fetch steps this chain does not drive) is a no-op; a
/// REQUIRED provider can never be missing because construction guaranteed it.
pub struct RuntimeStepRunner {
    handles: BTreeMap<String, Box<dyn ProviderHandle>>,
}

impl std::fmt::Debug for RuntimeStepRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Only the provider KEYS — never the handles' contents (which may hold
        // capability material). The runner itself carries no other authority.
        f.debug_struct("RuntimeStepRunner")
            .field("providers", &self.provider_keys())
            .finish()
    }
}

impl RuntimeStepRunner {
    /// Inject the provider handles for a plan, fail-closed. See the type docs.
    pub fn new(
        plan: &DrmOpenPlan,
        handles: Vec<Box<dyn ProviderHandle>>,
    ) -> Result<Self, String> {
        let required = plan.required_provider_keys();
        let mut map: BTreeMap<String, Box<dyn ProviderHandle>> = BTreeMap::new();
        for h in handles {
            let key = h.provider().to_string();
            if !required.contains(&key) {
                return Err(format!(
                    "refusing a stray `{key}` handle: the plan does not name it in next_required_providers ({required:?})"
                ));
            }
            if map.insert(key.clone(), h).is_some() {
                return Err(format!("two handles injected for provider `{key}`"));
            }
        }
        for req in &required {
            if !map.contains_key(req) {
                return Err(format!(
                    "no capability handle injected for required provider `{req}` — the core cannot drive the plan"
                ));
            }
        }
        Ok(Self { handles: map })
    }

    /// Build the runner by RESOLVING each provider the plan requires from the runtime
    /// capability table — the composition-root constructor. Calls `table.resolve` once
    /// per required provider (handlers never re-resolve, mirroring PC2's "handlers must
    /// NOT re-load by token", `secureViewSession.ts:13`) and fails closed if the table
    /// holds no capability for a required provider, or hands back a handle for the wrong
    /// provider. The final fail-closed checks (required/stray/duplicate) run in [`new`].
    pub fn resolve_from(
        plan: &DrmOpenPlan,
        table: &mut dyn CapabilityTable,
    ) -> Result<Self, String> {
        let mut handles: Vec<Box<dyn ProviderHandle>> = Vec::new();
        for provider in plan.required_provider_keys() {
            let handle = table.resolve(&provider).ok_or_else(|| {
                format!(
                    "runtime capability table holds no handle for required provider `{provider}` — the core cannot drive the plan"
                )
            })?;
            if handle.provider() != provider {
                return Err(format!(
                    "capability table returned a `{}` handle when asked for `{provider}`",
                    handle.provider()
                ));
            }
            handles.push(handle);
        }
        Self::new(plan, handles)
    }

    /// The provider keys this runner can drive (one per injected handle).
    pub fn provider_keys(&self) -> Vec<&str> {
        self.handles.keys().map(String::as_str).collect()
    }
}

/// A runtime-supplied capability table: resolves the injected [`ProviderHandle`] for a
/// provider role, or `None` if the runtime holds no capability for it. This is the
/// runtime-core analogue of PC2's backend-keyed session factory
/// (`BackendSessionService.getSessionView(token)` dispatching on `stored.backend`,
/// `src/services/session/BackendSessionService.ts:368`): the ONE place a capability is
/// resolved from runtime-held state. The core entrypoint [`open_drm_plan`] calls it once
/// per required provider; nothing downstream re-resolves.
pub trait CapabilityTable {
    fn resolve(&mut self, provider: &str) -> Option<Box<dyn ProviderHandle>>;
}

/// The runtime-core composition root for a dDRM open: parse the plan, resolve each
/// provider the plan requires from the runtime capability `table` (fail closed if the
/// table holds no capability for a required provider), build the [`RuntimeStepRunner`],
/// and execute — returning the [`ExecutionReport`]. This is the SINGLE entrypoint the
/// trusted runtime calls; the consumer smoke calls the SAME function with a table backed
/// by spawned capsule binaries (no second code path). Mirrors PC2's
/// `requireSecureViewSession` composition root, which resolves the session view once
/// (`src/api/middleware/secureViewSession.ts:124`) and hands it to the handler (`:129`),
/// the handler invoking it from request state rather than re-resolving (`media.ts:481`).
pub fn open_drm_plan(
    plan: &Value,
    table: &mut dyn CapabilityTable,
) -> Result<ExecutionReport, String> {
    let plan = DrmOpenPlan::parse(plan)?;
    let mut runner = RuntimeStepRunner::resolve_from(&plan, table)?;
    plan.execute(&mut runner)
}

/// A runtime-OWNED provider transport: the long-lived capability to drive one
/// provider's steps. The runtime registers one transport per provider into a
/// [`RuntimeCapabilityTable`] at startup — the analogue of PC2's `sessionService`
/// singleton owning the per-backend view constructors
/// (`export const sessionService = new BackendSessionService(...)`,
/// `src/services/session/BackendSessionService.ts:495`). On each open the table asks the
/// transport to OPEN a fresh [`ProviderHandle`], mirroring `getSessionView` constructing
/// a fresh `BackendSessionView`/`WasmSessionView` per request from the runtime-owned
/// record (`:368`). The runtime owns the transports; the open supplies only the plan.
pub trait ProviderTransport {
    /// The provider role this transport drives — matches a plan step's `provider`.
    fn provider(&self) -> &str;
    /// Open a fresh handle over this transport for ONE plan execution.
    fn open(&self) -> Box<dyn ProviderHandle>;
    /// Tear down the connection this transport OWNS (the analogue of PC2's
    /// `ISessionView.dispose()` releasing the per-view WASM L2 handle via `requestDrop`,
    /// `src/api/chipotle-client.ts:231`/`:694`). Called by [`RuntimeCapabilityTable::shutdown`]
    /// when the host shuts down, so the runtime that OWNS the transport also owns its
    /// teardown. Default no-op for transports that own no releasable resource; fail-closed
    /// (an error here surfaces to the host's shutdown).
    fn shutdown(&mut self) -> Result<(), String> {
        Ok(())
    }
}

/// How the runtime BRINGS UP a provider transport: the host LAUNCHES each launcher, which
/// connects to / spawns its provider, drives its init (the provider publishes the material
/// the rail needs — a key authority's verifying/recipient keys, a decrypt boundary's
/// session key), and hands back a [`ProviderTransport`] that OWNS that connection's
/// lifecycle (open handles → `shutdown`). The runtime-core analogue of PC2's
/// `BackendSessionService.createSession` launching a backend view —
/// `WasmSessionView.createNew()` mints + PUBLISHES the session key inside the runtime
/// (`src/api/chipotle-client.ts:603`–`:613`, `BackendSessionService.ts:307`) — so the
/// HOST, not a dev harness, brings the rail up. Fail-closed: a launch that cannot bring its
/// provider up surfaces, and the host tears down whatever already launched.
pub trait ProviderLauncher {
    /// The provider role this launcher brings up — matches a plan step's `provider`.
    fn provider(&self) -> &str;
    /// Launch the provider and hand back the transport that owns its connection. Consumes
    /// the launcher (a launcher brings up its provider exactly once).
    fn launch(self: Box<Self>) -> Result<Box<dyn ProviderTransport>, String>;
}

/// The runtime-core capability table: a registry of runtime-OWNED provider transports.
/// The runtime `register`s one transport per provider it can drive (at startup); on a
/// dDRM open, [`open_drm_plan`] → `resolve(provider)` opens a fresh handle over the
/// registered transport, or returns `None` for a provider the runtime never registered
/// (→ the open fails closed). This is the concrete [`CapabilityTable`] the trusted core
/// owns; the consumer smoke registers capsule-backed transports into the SAME type — no
/// second code path. Mirrors the PC2 factory that dispatches on `stored.backend` to a
/// runtime-owned view constructor, `null` for an unknown backend
/// (`BackendSessionService.ts:368`–`:377`).
#[derive(Default)]
pub struct RuntimeCapabilityTable {
    transports: BTreeMap<String, Box<dyn ProviderTransport>>,
}

impl RuntimeCapabilityTable {
    pub fn new() -> Self {
        Self {
            transports: BTreeMap::new(),
        }
    }

    /// Register a runtime-owned transport for its provider. Fails closed if that
    /// provider already has a transport — a provider has exactly ONE owner, so a
    /// second registration is a wiring bug, never a silent override.
    pub fn register(&mut self, transport: Box<dyn ProviderTransport>) -> Result<(), String> {
        let key = transport.provider().to_string();
        if self.transports.contains_key(&key) {
            return Err(format!(
                "provider `{key}` already has a registered transport — a provider has one owner"
            ));
        }
        self.transports.insert(key, transport);
        Ok(())
    }

    /// Bring up the rail: LAUNCH each launcher in the given order (the caller supplies the
    /// dependency order — e.g. the key authority before the decrypt boundary that trusts
    /// it) and register the resulting transport. Fail-closed: if any launch fails, the
    /// transports already brought up are TORN DOWN before the error surfaces — a partially
    /// launched rail never lingers. This is the runtime-core analogue of the host launching
    /// each backend view via `createSession`/`createNew` (`BackendSessionService.ts:307`).
    pub fn from_launchers(launchers: Vec<Box<dyn ProviderLauncher>>) -> Result<Self, String> {
        let mut table = Self::new();
        for launcher in launchers {
            let provider = launcher.provider().to_string();
            match launcher.launch() {
                Ok(transport) => {
                    if let Err(e) = table.register(transport) {
                        let _ = table.shutdown();
                        return Err(e);
                    }
                }
                Err(e) => {
                    // Tear down whatever already came up, then surface the launch failure.
                    let _ = table.shutdown();
                    return Err(format!("launching provider `{provider}` failed: {e}"));
                }
            }
        }
        Ok(table)
    }

    /// The providers the runtime has registered a transport for.
    pub fn registered_providers(&self) -> Vec<&str> {
        self.transports.keys().map(String::as_str).collect()
    }

    /// Tear down every registered transport (the runtime owns the transports, so it owns
    /// their teardown). Attempts ALL transports even if one fails, then returns the first
    /// error — fail-closed: a transport that cannot release its connection surfaces to the
    /// caller rather than being silently dropped.
    pub fn shutdown(&mut self) -> Result<(), String> {
        let mut first_err: Option<String> = None;
        for (provider, transport) in self.transports.iter_mut() {
            if let Err(e) = transport.shutdown() {
                first_err.get_or_insert_with(|| format!("transport `{provider}` shutdown failed: {e}"));
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

impl CapabilityTable for RuntimeCapabilityTable {
    fn resolve(&mut self, provider: &str) -> Option<Box<dyn ProviderHandle>> {
        self.transports.get(provider).map(|t| t.open())
    }
}

/// The runtime's capability to obtain the open PLAN for a piece of content — the
/// runtime-owned source the host asks "what is the canonical sequence to open this?".
/// The plan itself is emitted by `drm-provider` (which holds no authority); this trait
/// is the host's seam to that provider. The runtime-core analogue of PC2's `/init`
/// route fetching + parsing the MPD before driving recovery (`src/api/media.ts:162`–`:208`):
/// the host first resolves WHAT to open, then drives the open.
pub trait PlanSource {
    /// Fetch the `DrmOpenPlanV1` JSON for `content_id` under `viewer_interface`.
    fn fetch(&mut self, content_id: &str, viewer_interface: &str) -> Result<Value, String>;
}

/// The identity of the open being recorded — handed to the [`RuntimeEventSink`] so the
/// persisted receipt / audit record names WHAT was opened (the runtime-core analogue of
/// the content identity PC2's `/init` logs + stores on the session, `media.ts:489`).
#[derive(Debug, Clone, Copy)]
pub struct OpenContext<'a> {
    pub content_id: &'a str,
    pub viewer_interface: &'a str,
}

/// A runtime-owned sink for the plan's runtime-EVENT steps — the steps the plan declares
/// with `owner: runtime` (e.g. `release_receipt`, `protected_content.open.audit`) that no
/// provider performs. The host emits each, in plan order, after the provider chain runs,
/// handing the sink the open identity ([`OpenContext`]) + the finished [`ExecutionReport`]
/// so it can persist the receipt / write the audit record. The runtime-core analogue of
/// PC2's `/init` creating the playback session + logging the open (`media.ts:489`
/// `mediaSessionManager.create`, `:483`/`:518`). Fail-closed: if a declared runtime event
/// cannot be emitted (e.g. persistence fails), the open fails.
pub trait RuntimeEventSink {
    fn emit(
        &mut self,
        event: &str,
        ctx: &OpenContext,
        report: &ExecutionReport,
    ) -> Result<(), String>;
}

/// A durable store the runtime persists open records into — the I/O-free seam the
/// [`PersistingEventSink`] writes through, so the lib owns the (CEK-free) record SHAPE +
/// fail-closed logic while the concrete durability (filesystem, DB, …) is injected. The
/// runtime-core analogue of `mediaSessionManager`'s in-process session store
/// (`src/services/media/sessionManager.ts:78`); a real runtime injects a durable impl.
pub trait EventStore {
    /// Persist `record` under `key`. Fail-closed: an error fails the emit (and the open).
    fn persist(&mut self, key: &str, record: &Value) -> Result<(), String>;
}

/// Build the CEK-free record the runtime persists for a runtime event. It carries ONLY
/// open METADATA — the event, the open identity, the steps that ran, whether a decrypt
/// session was opened, and the NAMES of the artifacts produced — and NEVER the artifact
/// VALUES (which can carry sealed CEK material). This is the audit/receipt invariant: the
/// durable record describes the open without ever holding key material, mirroring PC2
/// keeping the CEK server-side and out of the session record it returns
/// (`sessionManager.ts:5`–`:6`, `:18`).
pub fn open_event_record(event: &str, ctx: &OpenContext, report: &ExecutionReport) -> Value {
    let artifact_names: Vec<&str> = report.artifacts.keys().map(String::as_str).collect();
    serde_json::json!({
        "schema": "elastos.drm.open_event_record/v1",
        "event": event,
        "content_id": ctx.content_id,
        "viewer_interface": ctx.viewer_interface,
        "steps_run": report.steps_run,
        "decrypt_session_opened": report.artifact("decrypt_session").is_some(),
        "artifact_names": artifact_names,
    })
}

/// A [`RuntimeEventSink`] that PERSISTS each runtime event as a durable, CEK-free record
/// into an injected [`EventStore`]. The runtime-core analogue of PC2's `/init` persisting
/// the open (creating the lifetime session via `mediaSessionManager.create`) + writing the
/// audit log — except the persisted record holds NO key material (see [`open_event_record`]).
/// Fail-closed: a store that cannot persist a declared runtime event fails the open.
pub struct PersistingEventSink<S: EventStore> {
    store: S,
    persisted: Vec<String>,
}

impl<S: EventStore> PersistingEventSink<S> {
    pub fn new(store: S) -> Self {
        Self {
            store,
            persisted: Vec::new(),
        }
    }

    /// The events persisted so far, in order (for assertions / introspection).
    pub fn persisted(&self) -> &[String] {
        &self.persisted
    }

    /// Borrow the underlying store (e.g. to inspect what was written).
    pub fn store(&self) -> &S {
        &self.store
    }
}

impl<S: EventStore> RuntimeEventSink for PersistingEventSink<S> {
    fn emit(
        &mut self,
        event: &str,
        ctx: &OpenContext,
        report: &ExecutionReport,
    ) -> Result<(), String> {
        let record = open_event_record(event, ctx, report);
        let key = format!("{}/{}", ctx.content_id, event);
        self.store.persist(&key, &record)?;
        self.persisted.push(event.to_string());
        Ok(())
    }
}

/// Turn an open-record key (`content_id/event`) into a single stable, collision-resistant
/// filename — the on-disk layout the [`DurableEventStore`] writes. Mirrors `FileSessionStore`
/// keying a file by the session id (`BackendSessionService.ts:140`–`:143`).
fn durable_record_filename(key: &str) -> String {
    let safe: String = key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .collect();
    format!("{safe}.json")
}

/// A production-shaped DURABLE [`EventStore`]: writes each open record as a JSON file under
/// a directory, keyed by `content_id/event`, and reads them back on a fresh instance/process.
/// Mirrors PC2's `FileSessionStore` — one file per record id, restored by `loadAll` across a
/// process restart, corrupt files skipped (`src/services/session/BackendSessionService.ts:107`,
/// `:140`–`:196`). Durability + integrity properties:
///   * **atomic**: writes to a `*.tmp` sibling then `rename`s into place, so a reader never
///     sees a half-written record (a crash mid-write leaves the old record or none, never a
///     torn one); the temp file is cleaned up on a failed write.
///   * **idempotent**: re-persisting the same key atomically replaces the record.
///   * **fail-closed**: any I/O error (create dir, write, rename) surfaces to the caller, so
///     a runtime event that cannot be durably recorded fails the open.
///   * **read-back**: [`DurableEventStore::load`] returns every persisted record from a
///     directory (skipping non-record / corrupt files), proving durability across a fresh
///     reader — the analogue of `FileSessionStore::loadAll`.
pub struct DurableEventStore {
    dir: std::path::PathBuf,
}

impl DurableEventStore {
    /// Open (creating if absent) a durable store rooted at `dir`. Fail-closed if the
    /// directory cannot be created.
    pub fn open(dir: impl Into<std::path::PathBuf>) -> Result<Self, String> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir).map_err(|e| format!("durable store dir {}: {e}", dir.display()))?;
        Ok(Self { dir })
    }

    /// The directory this store persists into.
    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    /// Read every persisted open record back from `dir`, as `(filename, record)` — skipping
    /// non-`.json` and corrupt files (a corrupt record is never served as if intact). The
    /// analogue of `FileSessionStore::loadAll` restoring sessions across a fresh process.
    pub fn load(dir: impl AsRef<std::path::Path>) -> Result<Vec<(String, Value)>, String> {
        let dir = dir.as_ref();
        let mut out = Vec::new();
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            // A never-written store reads back as empty, not an error.
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(format!("read durable store {}: {e}", dir.display())),
        };
        for entry in entries {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            // Skip a corrupt record rather than serving it as intact (fail-closed read).
            if let Ok(record) = serde_json::from_slice::<Value>(&bytes) {
                out.push((name, record));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }
}

impl EventStore for DurableEventStore {
    fn persist(&mut self, key: &str, record: &Value) -> Result<(), String> {
        let fname = durable_record_filename(key);
        let final_path = self.dir.join(&fname);
        let tmp_path = self.dir.join(format!("{fname}.tmp"));
        let bytes = serde_json::to_vec_pretty(record).map_err(|e| e.to_string())?;
        // Atomic publish: write the temp file fully, then rename over the final path. On any
        // failure, best-effort remove the temp file so no torn `*.tmp` lingers.
        if let Err(e) = std::fs::write(&tmp_path, &bytes) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(format!("write durable record {}: {e}", tmp_path.display()));
        }
        if let Err(e) = std::fs::rename(&tmp_path, &final_path) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(format!("publish durable record {}: {e}", final_path.display()));
        }
        Ok(())
    }
}

/// What a host open produced: the steps that ran, the artifacts, and the runtime events
/// the host emitted (in order).
#[derive(Debug, Clone)]
pub struct HostOpenReport {
    pub execution: ExecutionReport,
    pub events_emitted: Vec<String>,
}

impl HostOpenReport {
    pub fn artifact(&self, produces: &str) -> Option<&Value> {
        self.execution.artifact(produces)
    }
}

/// The trusted runtime-core HOST for a dDRM open: the single owned entrypoint that
/// composes the WHOLE open. It owns (1) a [`PlanSource`] to obtain the plan, (2) the
/// runtime [`RuntimeCapabilityTable`] of provider transports, and (3) a
/// [`RuntimeEventSink`] for the plan's runtime-owned post-steps. This is the runtime-core
/// analogue of PC2's `/init` route, which — once the middleware has resolved the
/// capability — owns fetching the plan-equivalent (MPD), driving recovery over that
/// capability, creating the session, and logging, all in one place, fail-closed
/// (`src/api/media.ts:133` route → `:481`/`:482` recover → `:489` session → `:528` catch).
/// The consumer smoke calls THIS entrypoint (its capsule binaries become the host's
/// registered transports + plan source) — no second code path.
pub struct DrmHost {
    plan_source: Box<dyn PlanSource>,
    table: RuntimeCapabilityTable,
    events: Box<dyn RuntimeEventSink>,
}

impl DrmHost {
    pub fn new(
        plan_source: Box<dyn PlanSource>,
        table: RuntimeCapabilityTable,
        events: Box<dyn RuntimeEventSink>,
    ) -> Self {
        Self {
            plan_source,
            table,
            events,
        }
    }

    /// Build a host that BRINGS UP ITS OWN RAIL: LAUNCH each provider via its
    /// [`ProviderLauncher`] (in caller-supplied dependency order) into the capability table,
    /// then wire the plan source + event sink. The composition lives HERE in the trusted
    /// core — a caller hands the host launchers + a sink and gets back a host that owns the
    /// rail, rather than assembling the table itself. Fail-closed: a launch failure tears
    /// down the partially-launched rail (via [`RuntimeCapabilityTable::from_launchers`]) and
    /// surfaces before any plan is fetched.
    pub fn launch(
        plan_source: Box<dyn PlanSource>,
        launchers: Vec<Box<dyn ProviderLauncher>>,
        events: Box<dyn RuntimeEventSink>,
    ) -> Result<Self, String> {
        let table = RuntimeCapabilityTable::from_launchers(launchers)?;
        Ok(Self::new(plan_source, table, events))
    }

    /// Open `content_id` under `viewer_interface`: fetch the plan, drive it through the
    /// runtime capability registry (parse → resolve each required transport → execute —
    /// exactly [`open_drm_plan`]'s core), then emit the plan's runtime-event steps in
    /// order through the host's sink. Fail-closed at every seam: a bad plan never resolves
    /// a capability, a missing transport fails closed, and a runtime event that cannot be
    /// emitted fails the open.
    pub fn open(
        &mut self,
        content_id: &str,
        viewer_interface: &str,
    ) -> Result<HostOpenReport, String> {
        let plan_json = self.plan_source.fetch(content_id, viewer_interface)?;
        let plan = DrmOpenPlan::parse(&plan_json)?;
        let mut runner = RuntimeStepRunner::resolve_from(&plan, &mut self.table)?;
        let execution = plan.execute(&mut runner)?;

        // Runtime-owned post-steps: the plan declares them (owner: runtime), no provider
        // performs them — the HOST emits each, in plan order, after the provider chain.
        let ctx = OpenContext {
            content_id,
            viewer_interface,
        };
        let mut events_emitted = Vec::new();
        for step in plan.steps.iter().filter(|s| s.is_runtime_event()) {
            let event = step
                .event
                .as_deref()
                .expect("is_runtime_event guarantees an event name");
            self.events.emit(event, &ctx, &execution)?;
            events_emitted.push(event.to_string());
        }

        Ok(HostOpenReport {
            execution,
            events_emitted,
        })
    }

    /// The providers this host can drive (one per registered transport).
    pub fn registered_providers(&self) -> Vec<&str> {
        self.table.registered_providers()
    }

    /// Shut the host down: tear down every transport the runtime owns (each releases its
    /// own connection — the analogue of disposing every per-view WASM handle). Consumes
    /// the host so its capabilities cannot be used after teardown. Fail-closed: a transport
    /// that cannot release its connection surfaces here.
    pub fn shutdown(mut self) -> Result<(), String> {
        self.table.shutdown()
    }
}

impl StepRunner for RuntimeStepRunner {
    fn run_step(&mut self, inputs: &StepInputs) -> Result<BTreeMap<String, Value>, String> {
        let provider = match &inputs.step.provider {
            Some(p) => p.as_str(),
            // Runtime-event steps (no provider) are walked for ordering only.
            None => return Ok(BTreeMap::new()),
        };
        match self.handles.get_mut(provider) {
            Some(handle) => handle.run(inputs),
            // A provider-call step with no injected handle is a runtime no-op (e.g. the
            // `content` status/fetch steps); construction proved every REQUIRED provider
            // has a handle, so this can only be an un-required step.
            None => Ok(BTreeMap::new()),
        }
    }
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
    /// The providers the runtime MUST be able to drive to run this plan (the plan's
    /// `next_required_providers`, e.g. `["rights-provider", "key-provider", ...]`). The
    /// runtime injects exactly one capability handle per entry; see [`RuntimeStepRunner`].
    pub next_required_providers: Vec<String>,
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

        let next_required_providers = parse_next_required_providers(plan)?;

        Ok(Self {
            content_id,
            object_cid,
            viewer_interface,
            steps,
            bindings,
            next_required_providers,
        })
    }

    /// The providers this plan requires, normalized to their step-`provider` names
    /// (`rights-provider` → `rights`) — the keys the runtime injects capability
    /// handles under. A `RuntimeStepRunner` must hold one handle per entry.
    pub fn required_provider_keys(&self) -> Vec<String> {
        self.next_required_providers
            .iter()
            .map(|p| normalize_provider(p))
            .collect()
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
            event: s["event"].as_str().map(str::to_string),
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

fn parse_next_required_providers(plan: &Value) -> Result<Vec<String>, String> {
    let arr = plan["next_required_providers"]
        .as_array()
        .ok_or("plan has no next_required_providers array")?;
    if arr.is_empty() {
        return Err("plan declares no required providers".to_string());
    }
    arr.iter()
        .map(|p| {
            p.as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .ok_or_else(|| "next_required_providers entry is not a non-empty string".to_string())
        })
        .collect()
}

/// Normalize a provider name to its step-`provider` key: the plan's
/// `next_required_providers` carry the `-provider` suffix (`key-provider`) while a
/// step's `provider` is the bare role (`key`). One identity, two spellings.
fn normalize_provider(name: &str) -> String {
    name.strip_suffix("-provider").unwrap_or(name).to_string()
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
            ],
            "next_required_providers": ["rights-provider", "key-provider", "decrypt-provider"]
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

    #[test]
    fn parses_next_required_providers() {
        let plan = DrmOpenPlan::parse(&canonical_plan()).unwrap();
        assert_eq!(
            plan.next_required_providers,
            vec!["rights-provider", "key-provider", "decrypt-provider"]
        );
        // Normalized to the bare step-provider keys the runtime injects handles under.
        assert_eq!(plan.required_provider_keys(), vec!["rights", "key", "decrypt"]);
    }

    #[test]
    fn rejects_a_plan_with_no_required_providers() {
        let mut p = canonical_plan();
        p["next_required_providers"] = json!([]);
        assert!(DrmOpenPlan::parse(&p).is_err());
    }

    // ── RuntimeStepRunner: the runtime-core StepRunner over injected provider handles ──

    /// A capability handle for one provider, backed by a recorder rather than a real
    /// capsule. Emits whatever artifacts the plan says its step produces.
    /// A recognizable "sealed material" value the `decrypt_session` artifact carries, so a
    /// persisted open record can be proven to record the artifact NAME but never this VALUE.
    const SENTINEL_SEALED_VALUE: &str = "SEALED-CEK-DO-NOT-LEAK";

    struct FakeHandle {
        provider: String,
        invoked: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    }

    impl FakeHandle {
        fn boxed(provider: &str, log: &std::rc::Rc<std::cell::RefCell<Vec<String>>>) -> Box<dyn ProviderHandle> {
            Box::new(FakeHandle {
                provider: provider.to_string(),
                invoked: log.clone(),
            })
        }
    }

    impl ProviderHandle for FakeHandle {
        fn provider(&self) -> &str {
            &self.provider
        }
        fn run(&mut self, inputs: &StepInputs) -> Result<BTreeMap<String, Value>, String> {
            self.invoked.borrow_mut().push(self.provider.clone());
            let mut out = BTreeMap::new();
            match inputs.step.name.as_str() {
                "rights_check" => {
                    out.insert("RightsDecisionReceiptV1".to_string(), json!({ "allowed": true }));
                }
                "key_release" => {
                    inputs.require_threaded("rights_receipt")?;
                    out.insert("ReleaseReceiptV1".to_string(), json!({ "status": "released" }));
                }
                "decrypt_session" => {
                    inputs.require_threaded("release_receipt")?;
                    // The artifact VALUE carries a sentinel "sealed" blob — a persisted
                    // open record must record the artifact NAME but never this VALUE.
                    out.insert(
                        "decrypt_session".to_string(),
                        json!({ "decision": "opened", "sealed_blob": SENTINEL_SEALED_VALUE }),
                    );
                }
                _ => {}
            }
            Ok(out)
        }
    }

    fn handle_set(log: &std::rc::Rc<std::cell::RefCell<Vec<String>>>) -> Vec<Box<dyn ProviderHandle>> {
        vec![
            FakeHandle::boxed("rights", log),
            FakeHandle::boxed("key", log),
            FakeHandle::boxed("decrypt", log),
        ]
    }

    #[test]
    fn runtime_runner_drives_the_plan_through_injected_handles() {
        let plan = DrmOpenPlan::parse(&canonical_plan()).unwrap();
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut runner = RuntimeStepRunner::new(&plan, handle_set(&log)).expect("handles cover the plan");
        let report = plan.execute(&mut runner).expect("runtime runner drives the plan");
        assert!(report.artifact("decrypt_session").is_some());
        // Each required provider's handle was invoked, in canonical order; the unhandled
        // `content` steps were no-ops (never routed to a handle). The decrypt handle is
        // invoked twice because BOTH `decrypt_session` and `render` are decrypt-provider
        // steps — render is a no-op here but still routes to the same injected handle.
        assert_eq!(*log.borrow(), vec!["rights", "key", "decrypt", "decrypt"]);
    }

    #[test]
    fn runtime_runner_refuses_to_build_without_a_required_handle() {
        let plan = DrmOpenPlan::parse(&canonical_plan()).unwrap();
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        // Drop the key handle — the plan requires key-provider, so construction fails closed.
        let handles = vec![FakeHandle::boxed("rights", &log), FakeHandle::boxed("decrypt", &log)];
        let err = RuntimeStepRunner::new(&plan, handles).unwrap_err();
        assert!(err.contains("no capability handle injected for required provider `key`"), "{err}");
    }

    #[test]
    fn runtime_runner_refuses_a_stray_unnamed_handle() {
        let plan = DrmOpenPlan::parse(&canonical_plan()).unwrap();
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        // A `wallet` handle the plan never names must be rejected — a capability the plan
        // did not authorize can never enter the runner.
        let mut handles = handle_set(&log);
        handles.push(FakeHandle::boxed("wallet", &log));
        let err = RuntimeStepRunner::new(&plan, handles).unwrap_err();
        assert!(err.contains("stray `wallet` handle"), "{err}");
    }

    #[test]
    fn runtime_runner_rejects_duplicate_handles() {
        let plan = DrmOpenPlan::parse(&canonical_plan()).unwrap();
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut handles = handle_set(&log);
        handles.push(FakeHandle::boxed("key", &log));
        let err = RuntimeStepRunner::new(&plan, handles).unwrap_err();
        assert!(err.contains("two handles injected for provider `key`"), "{err}");
    }

    #[test]
    fn runtime_runner_never_invokes_a_handle_for_an_unnamed_provider() {
        // The runner only ever routes by the step's provider; a `content`-step has no
        // handle, so it is a no-op and no handle is invoked for it. Combined with the
        // stray-handle rejection above, a handle is only ever invoked for a plan-named
        // provider. Assert the content steps produced nothing and were not logged.
        let plan = DrmOpenPlan::parse(&canonical_plan()).unwrap();
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut runner = RuntimeStepRunner::new(&plan, handle_set(&log)).unwrap();
        plan.execute(&mut runner).unwrap();
        assert!(!log.borrow().iter().any(|p| p == "content"));
        assert_eq!(runner.provider_keys(), vec!["decrypt", "key", "rights"]);
    }

    // ── open_drm_plan: the runtime-core composition root over a capability table ──

    /// A runtime capability table backed by fake handles — the test analogue of PC2's
    /// backend-keyed session factory. Records which providers it was asked to resolve;
    /// `withhold`/`misroute` model a runtime that cannot (or wrongly) supplies a handle.
    struct FakeTable {
        invoked: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
        resolved: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
        withhold: Option<String>,
        misroute: bool,
    }

    impl FakeTable {
        fn new(
            invoked: &std::rc::Rc<std::cell::RefCell<Vec<String>>>,
            resolved: &std::rc::Rc<std::cell::RefCell<Vec<String>>>,
        ) -> Self {
            FakeTable {
                invoked: invoked.clone(),
                resolved: resolved.clone(),
                withhold: None,
                misroute: false,
            }
        }
    }

    impl CapabilityTable for FakeTable {
        fn resolve(&mut self, provider: &str) -> Option<Box<dyn ProviderHandle>> {
            self.resolved.borrow_mut().push(provider.to_string());
            if self.withhold.as_deref() == Some(provider) {
                return None;
            }
            // A misrouting table hands back a `decrypt` handle when asked for `key`.
            let served = if self.misroute && provider == "key" {
                "decrypt"
            } else {
                provider
            };
            Some(FakeHandle::boxed(served, &self.invoked))
        }
    }

    #[test]
    fn open_drm_plan_drives_the_plan_through_a_capability_table() {
        let invoked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let resolved = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut table = FakeTable::new(&invoked, &resolved);
        let report = open_drm_plan(&canonical_plan(), &mut table).expect("the core entrypoint drives the plan");
        assert!(report.artifact("decrypt_session").is_some());
        // The composition root resolved each required provider exactly once, in order,
        // from the table — and only those (no `content` resolve).
        assert_eq!(*resolved.borrow(), vec!["rights", "key", "decrypt"]);
        assert_eq!(*invoked.borrow(), vec!["rights", "key", "decrypt", "decrypt"]);
    }

    #[test]
    fn open_drm_plan_fails_closed_when_the_table_lacks_a_required_provider() {
        let invoked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let resolved = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut table = FakeTable::new(&invoked, &resolved);
        table.withhold = Some("key".to_string());
        let err = open_drm_plan(&canonical_plan(), &mut table).unwrap_err();
        assert!(
            err.contains("holds no handle for required provider `key`"),
            "{err}"
        );
        // It never reached the decrypt resolve (fail-closed at the missing key), and the
        // plan never executed a single provider call.
        assert!(invoked.borrow().is_empty());
    }

    #[test]
    fn open_drm_plan_rejects_a_table_that_misroutes_a_provider() {
        let invoked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let resolved = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut table = FakeTable::new(&invoked, &resolved);
        table.misroute = true;
        let err = open_drm_plan(&canonical_plan(), &mut table).unwrap_err();
        assert!(
            err.contains("returned a `decrypt` handle when asked for `key`"),
            "{err}"
        );
    }

    #[test]
    fn open_drm_plan_refuses_a_non_planned_plan_before_touching_the_table() {
        let invoked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let resolved = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut table = FakeTable::new(&invoked, &resolved);
        let mut p = canonical_plan();
        p["status"] = json!("opened");
        let err = open_drm_plan(&p, &mut table).unwrap_err();
        assert!(err.contains("non-planned"), "{err}");
        // The composition root parses BEFORE resolving — a bad plan never reaches the table.
        assert!(resolved.borrow().is_empty());
    }

    // ── RuntimeCapabilityTable: the runtime-OWNED registry of provider transports ──

    /// A runtime-owned transport backed by a fake handle — the test analogue of PC2's
    /// per-backend view constructor the `sessionService` singleton owns.
    struct FakeTransport {
        provider: String,
        invoked: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    }

    impl FakeTransport {
        fn boxed(provider: &str, invoked: &std::rc::Rc<std::cell::RefCell<Vec<String>>>) -> Box<dyn ProviderTransport> {
            Box::new(FakeTransport {
                provider: provider.to_string(),
                invoked: invoked.clone(),
            })
        }
    }

    impl ProviderTransport for FakeTransport {
        fn provider(&self) -> &str {
            &self.provider
        }
        fn open(&self) -> Box<dyn ProviderHandle> {
            FakeHandle::boxed(&self.provider, &self.invoked)
        }
    }

    #[test]
    fn runtime_table_drives_the_plan_from_registered_transports() {
        let invoked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut table = RuntimeCapabilityTable::new();
        table.register(FakeTransport::boxed("rights", &invoked)).unwrap();
        table.register(FakeTransport::boxed("key", &invoked)).unwrap();
        table.register(FakeTransport::boxed("decrypt", &invoked)).unwrap();
        assert_eq!(table.registered_providers(), vec!["decrypt", "key", "rights"]);
        let report = open_drm_plan(&canonical_plan(), &mut table).expect("registered transports drive the plan");
        assert!(report.artifact("decrypt_session").is_some());
        assert_eq!(*invoked.borrow(), vec!["rights", "key", "decrypt", "decrypt"]);
    }

    #[test]
    fn runtime_table_fails_closed_for_an_unregistered_required_provider() {
        let invoked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut table = RuntimeCapabilityTable::new();
        // The runtime registers rights + decrypt but NOT key — an open requiring key
        // resolves to None and fails closed; no provider step ever runs.
        table.register(FakeTransport::boxed("rights", &invoked)).unwrap();
        table.register(FakeTransport::boxed("decrypt", &invoked)).unwrap();
        let err = open_drm_plan(&canonical_plan(), &mut table).unwrap_err();
        assert!(err.contains("holds no handle for required provider `key`"), "{err}");
        assert!(invoked.borrow().is_empty());
    }

    #[test]
    fn runtime_table_rejects_a_duplicate_transport_registration() {
        let invoked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut table = RuntimeCapabilityTable::new();
        table.register(FakeTransport::boxed("key", &invoked)).unwrap();
        let err = table.register(FakeTransport::boxed("key", &invoked)).unwrap_err();
        assert!(err.contains("provider `key` already has a registered transport"), "{err}");
    }

    #[test]
    fn runtime_table_opens_a_fresh_handle_per_open() {
        // The runtime owns ONE transport per provider but resolves a FRESH handle on each
        // open (PC2 reuses the singleton across requests, minting a view per request). Two
        // opens over the same registered transports both succeed.
        let invoked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut table = RuntimeCapabilityTable::new();
        table.register(FakeTransport::boxed("rights", &invoked)).unwrap();
        table.register(FakeTransport::boxed("key", &invoked)).unwrap();
        table.register(FakeTransport::boxed("decrypt", &invoked)).unwrap();
        open_drm_plan(&canonical_plan(), &mut table).expect("first open");
        open_drm_plan(&canonical_plan(), &mut table).expect("second open over the same transports");
        // Both opens drove the full chain (rights,key,decrypt,decrypt) ×2.
        assert_eq!(invoked.borrow().len(), 8);
    }

    // ── DrmHost: the trusted runtime-core host that owns plan source + registry + sink ──

    #[test]
    fn plan_steps_parse_runtime_events() {
        let plan = DrmOpenPlan::parse(&canonical_plan()).unwrap();
        let events: Vec<&str> = plan
            .steps
            .iter()
            .filter(|s| s.is_runtime_event())
            .map(|s| s.event.as_deref().unwrap())
            .collect();
        assert_eq!(events, vec!["release_receipt", "protected_content.open.audit"]);
        // The provider-call steps are NOT runtime events.
        assert!(!plan.steps.iter().find(|s| s.name == "key_release").unwrap().is_runtime_event());
    }

    /// A plan source backed by the canonical plan; `tamper` models a source that yields a
    /// corrupted plan (renamed key edge) so the host must fail closed.
    struct FakePlanSource {
        tamper: bool,
        fetched: std::rc::Rc<std::cell::RefCell<u32>>,
    }

    impl PlanSource for FakePlanSource {
        fn fetch(&mut self, _content_id: &str, _viewer: &str) -> Result<Value, String> {
            *self.fetched.borrow_mut() += 1;
            let mut p = canonical_plan();
            if self.tamper {
                for b in p["bindings"].as_array_mut().unwrap() {
                    if b["into_step"] == json!("key_release") {
                        b["into_field"] = json!("bogus_edge");
                    }
                }
            }
            Ok(p)
        }
    }

    /// A sink that records emitted events; `refuse` models a sink that cannot persist the
    /// named event (e.g. the audit write fails) so the host fails closed.
    struct RecordingSink {
        emitted: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
        refuse: Option<String>,
    }

    impl RuntimeEventSink for RecordingSink {
        fn emit(
            &mut self,
            event: &str,
            _ctx: &OpenContext,
            _report: &ExecutionReport,
        ) -> Result<(), String> {
            if self.refuse.as_deref() == Some(event) {
                return Err(format!("sink refused to emit `{event}`"));
            }
            self.emitted.borrow_mut().push(event.to_string());
            Ok(())
        }
    }

    fn full_table(invoked: &std::rc::Rc<std::cell::RefCell<Vec<String>>>) -> RuntimeCapabilityTable {
        let mut table = RuntimeCapabilityTable::new();
        table.register(FakeTransport::boxed("rights", invoked)).unwrap();
        table.register(FakeTransport::boxed("key", invoked)).unwrap();
        table.register(FakeTransport::boxed("decrypt", invoked)).unwrap();
        table
    }

    #[test]
    fn drm_host_opens_via_plan_source_registry_and_emits_runtime_events() {
        let invoked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let fetched = std::rc::Rc::new(std::cell::RefCell::new(0u32));
        let emitted = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut host = DrmHost::new(
            Box::new(FakePlanSource { tamper: false, fetched: fetched.clone() }),
            full_table(&invoked),
            Box::new(RecordingSink { emitted: emitted.clone(), refuse: None }),
        );
        let report = host.open("bafycontent", "elastos.viewer/document@1").expect("host drives the open");
        assert!(report.artifact("decrypt_session").is_some());
        assert_eq!(*fetched.borrow(), 1, "the host fetched the plan from its source");
        // The host emitted BOTH runtime-event steps the plan declares, in order, after the chain.
        assert_eq!(report.events_emitted, vec!["release_receipt", "protected_content.open.audit"]);
        assert_eq!(*emitted.borrow(), vec!["release_receipt", "protected_content.open.audit"]);
    }

    #[test]
    fn drm_host_fails_closed_on_a_tampered_plan_from_the_source() {
        let invoked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let fetched = std::rc::Rc::new(std::cell::RefCell::new(0u32));
        let emitted = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut host = DrmHost::new(
            Box::new(FakePlanSource { tamper: true, fetched }),
            full_table(&invoked),
            Box::new(RecordingSink { emitted: emitted.clone(), refuse: None }),
        );
        // The tampered edge mislabels the key_release input; the fake key handle requires
        // the rights_receipt threaded edge, so execution fails closed — BEFORE any event.
        assert!(host.open("bafycontent", "elastos.viewer/document@1").is_err());
        assert!(emitted.borrow().is_empty(), "no runtime event is emitted when the open fails");
    }

    #[test]
    fn drm_host_fails_closed_when_the_event_sink_refuses() {
        let invoked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let fetched = std::rc::Rc::new(std::cell::RefCell::new(0u32));
        let emitted = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut host = DrmHost::new(
            Box::new(FakePlanSource { tamper: false, fetched }),
            full_table(&invoked),
            // The audit write fails — a declared runtime event that cannot be emitted
            // must fail the open (the receipt was emitted first, then audit refuses).
            Box::new(RecordingSink { emitted: emitted.clone(), refuse: Some("protected_content.open.audit".to_string()) }),
        );
        let err = host.open("bafycontent", "elastos.viewer/document@1").unwrap_err();
        assert!(err.contains("sink refused to emit `protected_content.open.audit`"), "{err}");
        assert_eq!(*emitted.borrow(), vec!["release_receipt"], "the receipt emitted before audit refused");
    }

    #[test]
    fn drm_host_fails_closed_when_a_required_transport_is_unregistered() {
        let invoked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let fetched = std::rc::Rc::new(std::cell::RefCell::new(0u32));
        let emitted = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        // The host's table registers rights + decrypt but NOT key.
        let mut table = RuntimeCapabilityTable::new();
        table.register(FakeTransport::boxed("rights", &invoked)).unwrap();
        table.register(FakeTransport::boxed("decrypt", &invoked)).unwrap();
        let mut host = DrmHost::new(
            Box::new(FakePlanSource { tamper: false, fetched }),
            table,
            Box::new(RecordingSink { emitted: emitted.clone(), refuse: None }),
        );
        let err = host.open("bafycontent", "elastos.viewer/document@1").unwrap_err();
        assert!(err.contains("holds no handle for required provider `key`"), "{err}");
        assert!(invoked.borrow().is_empty());
        assert!(emitted.borrow().is_empty());
    }

    // ── Day 77–78: the host OWNS transport teardown + a persisting (CEK-free) event sink ──

    /// A transport that records when it is opened and torn down; `fail_shutdown` models a
    /// transport whose connection cannot be released (teardown must surface fail-closed).
    struct OwningTransport {
        provider: String,
        log: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
        invoked: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
        fail_shutdown: bool,
    }

    impl ProviderTransport for OwningTransport {
        fn provider(&self) -> &str {
            &self.provider
        }
        fn open(&self) -> Box<dyn ProviderHandle> {
            self.log.borrow_mut().push(format!("open:{}", self.provider));
            FakeHandle::boxed(&self.provider, &self.invoked)
        }
        fn shutdown(&mut self) -> Result<(), String> {
            self.log.borrow_mut().push(format!("shutdown:{}", self.provider));
            if self.fail_shutdown {
                return Err("connection still in use".to_string());
            }
            Ok(())
        }
    }

    fn owning(
        provider: &str,
        log: &std::rc::Rc<std::cell::RefCell<Vec<String>>>,
        invoked: &std::rc::Rc<std::cell::RefCell<Vec<String>>>,
        fail_shutdown: bool,
    ) -> Box<dyn ProviderTransport> {
        Box::new(OwningTransport {
            provider: provider.to_string(),
            log: log.clone(),
            invoked: invoked.clone(),
            fail_shutdown,
        })
    }

    #[test]
    fn host_shutdown_tears_down_every_owned_transport() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let invoked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let fetched = std::rc::Rc::new(std::cell::RefCell::new(0u32));
        let emitted = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut table = RuntimeCapabilityTable::new();
        table.register(owning("rights", &log, &invoked, false)).unwrap();
        table.register(owning("key", &log, &invoked, false)).unwrap();
        table.register(owning("decrypt", &log, &invoked, false)).unwrap();
        let mut host = DrmHost::new(
            Box::new(FakePlanSource { tamper: false, fetched }),
            table,
            Box::new(RecordingSink { emitted, refuse: None }),
        );
        host.open("bafycontent", "elastos.viewer/document@1").expect("open");
        host.shutdown().expect("host tears down its owned transports");
        // Every registered transport was opened AND torn down (the runtime owns teardown).
        let log = log.borrow();
        for p in ["rights", "key", "decrypt"] {
            assert!(log.contains(&format!("open:{p}")), "{p} opened");
            assert!(log.contains(&format!("shutdown:{p}")), "{p} torn down");
        }
    }

    #[test]
    fn host_shutdown_fails_closed_when_a_transport_cannot_release() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let invoked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let fetched = std::rc::Rc::new(std::cell::RefCell::new(0u32));
        let emitted = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut table = RuntimeCapabilityTable::new();
        table.register(owning("rights", &log, &invoked, false)).unwrap();
        table.register(owning("key", &log, &invoked, true)).unwrap(); // key cannot release
        table.register(owning("decrypt", &log, &invoked, false)).unwrap();
        let mut host = DrmHost::new(
            Box::new(FakePlanSource { tamper: false, fetched }),
            table,
            Box::new(RecordingSink { emitted, refuse: None }),
        );
        host.open("bafycontent", "elastos.viewer/document@1").expect("open");
        let err = host.shutdown().unwrap_err();
        assert!(err.contains("transport `key` shutdown failed"), "{err}");
        // It still attempted to tear down the OTHER transports (best-effort, then surfaces).
        let log = log.borrow();
        assert!(log.contains(&"shutdown:rights".to_string()));
        assert!(log.contains(&"shutdown:decrypt".to_string()));
    }

    /// A store that records persisted records; `fail_on` models a durable store that
    /// cannot write a given event (the open must fail closed).
    struct FakeStore {
        records: std::rc::Rc<std::cell::RefCell<Vec<(String, Value)>>>,
        fail_on: Option<String>,
    }

    impl EventStore for FakeStore {
        fn persist(&mut self, key: &str, record: &Value) -> Result<(), String> {
            if self.fail_on.as_deref() == Some(record["event"].as_str().unwrap_or("")) {
                return Err(format!("store could not persist `{key}`"));
            }
            self.records.borrow_mut().push((key.to_string(), record.clone()));
            Ok(())
        }
    }

    #[test]
    fn persisting_sink_writes_cek_free_records_for_every_runtime_event() {
        let invoked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let fetched = std::rc::Rc::new(std::cell::RefCell::new(0u32));
        let records = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut host = DrmHost::new(
            Box::new(FakePlanSource { tamper: false, fetched }),
            full_table(&invoked),
            Box::new(PersistingEventSink::new(FakeStore {
                records: records.clone(),
                fail_on: None,
            })),
        );
        host.open("bafycontent", "elastos.viewer/document@1").expect("open + persist");
        let records = records.borrow();
        // Both runtime events were persisted, keyed by content + event, in order.
        let keys: Vec<&str> = records.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            vec!["bafycontent/release_receipt", "bafycontent/protected_content.open.audit"]
        );
        // Every persisted record is CEK-free: it carries open METADATA (identity, decision,
        // artifact NAMES) but never an artifact VALUE / key material.
        // The record carries artifact NAMES (safe) but never an artifact VALUE / key
        // material. Assert structurally (no `artifacts` map) below; here, a sealed-blob
        // value placed in an artifact must NOT have leaked into the persisted record.
        let blob = serde_json::to_string(&*records).unwrap();
        assert!(!blob.contains(SENTINEL_SEALED_VALUE), "persisted audit leaked an artifact value: {blob}");
        for (_, rec) in records.iter() {
            assert_eq!(rec["content_id"], serde_json::json!("bafycontent"));
            assert_eq!(rec["decrypt_session_opened"], serde_json::json!(true));
            assert!(rec["artifact_names"].is_array(), "names only, not values");
            // The artifact NAMES are recorded, but never the VALUES under those names.
            assert!(rec.get("artifacts").is_none(), "must persist names, not the artifact map");
        }
    }

    #[test]
    fn host_fails_closed_when_the_store_cannot_persist() {
        let invoked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let fetched = std::rc::Rc::new(std::cell::RefCell::new(0u32));
        let records = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut host = DrmHost::new(
            Box::new(FakePlanSource { tamper: false, fetched }),
            full_table(&invoked),
            // The durable store cannot write the audit record — the open must fail closed.
            Box::new(PersistingEventSink::new(FakeStore {
                records: records.clone(),
                fail_on: Some("protected_content.open.audit".to_string()),
            })),
        );
        let err = host.open("bafycontent", "elastos.viewer/document@1").unwrap_err();
        assert!(err.contains("store could not persist"), "{err}");
        // The receipt persisted first; the audit failed — the open did not silently succeed.
        assert_eq!(records.borrow().len(), 1);
    }

    // ── Day 79–80: the host LAUNCHES the rail + a production-shaped DURABLE store ──

    /// A launcher that brings up a `FakeTransport`; `fail` models a provider that cannot be
    /// brought up (the host must tear down whatever already launched and fail closed). The
    /// shared `log` records launch + (via the transport) teardown order.
    struct FakeLauncher {
        provider: String,
        log: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
        invoked: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
        fail: bool,
    }

    impl ProviderLauncher for FakeLauncher {
        fn provider(&self) -> &str {
            &self.provider
        }
        fn launch(self: Box<Self>) -> Result<Box<dyn ProviderTransport>, String> {
            self.log.borrow_mut().push(format!("launch:{}", self.provider));
            if self.fail {
                return Err("provider would not come up".to_string());
            }
            Ok(owning(&self.provider, &self.log, &self.invoked, false))
        }
    }

    fn launcher(
        provider: &str,
        log: &std::rc::Rc<std::cell::RefCell<Vec<String>>>,
        invoked: &std::rc::Rc<std::cell::RefCell<Vec<String>>>,
        fail: bool,
    ) -> Box<dyn ProviderLauncher> {
        Box::new(FakeLauncher {
            provider: provider.to_string(),
            log: log.clone(),
            invoked: invoked.clone(),
            fail,
        })
    }

    #[test]
    fn host_launches_the_rail_then_drives_and_tears_it_down() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let invoked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        // The host brings up the rail by LAUNCHING each provider (caller-supplied order).
        let table = RuntimeCapabilityTable::from_launchers(vec![
            launcher("rights", &log, &invoked, false),
            launcher("key", &log, &invoked, false),
            launcher("decrypt", &log, &invoked, false),
        ])
        .expect("the host launches the whole rail");
        let fetched = std::rc::Rc::new(std::cell::RefCell::new(0u32));
        let emitted = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut host = DrmHost::new(
            Box::new(FakePlanSource { tamper: false, fetched }),
            table,
            Box::new(RecordingSink { emitted, refuse: None }),
        );
        host.open("bafycontent", "elastos.viewer/document@1").expect("open over the launched rail");
        host.shutdown().expect("teardown");
        let log = log.borrow();
        // Launched in the given order, then every transport torn down.
        assert_eq!(log[0], "launch:rights");
        assert_eq!(log[1], "launch:key");
        assert_eq!(log[2], "launch:decrypt");
        for p in ["rights", "key", "decrypt"] {
            assert!(log.contains(&format!("shutdown:{p}")), "{p} torn down");
        }
    }

    #[test]
    fn host_launch_composes_the_rail_in_the_core() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let invoked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let fetched = std::rc::Rc::new(std::cell::RefCell::new(0u32));
        let emitted = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        // The host composes its OWN rail from launchers — the caller hands launchers + a sink,
        // not a pre-built table.
        let mut host = DrmHost::launch(
            Box::new(FakePlanSource { tamper: false, fetched }),
            vec![
                launcher("rights", &log, &invoked, false),
                launcher("key", &log, &invoked, false),
                launcher("decrypt", &log, &invoked, false),
            ],
            Box::new(RecordingSink { emitted, refuse: None }),
        )
        .expect("the host launches its own rail");
        host.open("bafycontent", "elastos.viewer/document@1").expect("open over the launched rail");
        host.shutdown().expect("teardown");
        let log = log.borrow();
        assert_eq!(log[0], "launch:rights");
        for p in ["rights", "key", "decrypt"] {
            assert!(log.contains(&format!("shutdown:{p}")), "{p} torn down");
        }
    }

    #[test]
    fn host_launch_fails_closed_when_the_rail_cannot_come_up() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let invoked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let fetched = std::rc::Rc::new(std::cell::RefCell::new(0u32));
        let emitted = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let err = match DrmHost::launch(
            Box::new(FakePlanSource { tamper: false, fetched }),
            vec![
                launcher("key", &log, &invoked, false),
                launcher("decrypt", &log, &invoked, true),
            ],
            Box::new(RecordingSink { emitted, refuse: None }),
        ) {
            Ok(_) => panic!("a rail that cannot come up must fail the host build"),
            Err(e) => e,
        };
        assert!(err.contains("launching provider `decrypt` failed"), "{err}");
    }

    #[test]
    fn from_launchers_fails_closed_and_tears_down_the_partial_rail() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let invoked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        // `key` comes up, then `decrypt` refuses — the partial rail must be torn down.
        let err = match RuntimeCapabilityTable::from_launchers(vec![
            launcher("key", &log, &invoked, false),
            launcher("decrypt", &log, &invoked, true),
        ]) {
            Ok(_) => panic!("the rail should have failed to come up"),
            Err(e) => e,
        };
        assert!(err.contains("launching provider `decrypt` failed"), "{err}");
        let log = log.borrow();
        // `key` launched, then was torn down when the rail failed to come up fully.
        assert!(log.contains(&"launch:key".to_string()));
        assert!(log.contains(&"shutdown:key".to_string()), "the partial rail was torn down");
    }

    fn unique_tmp_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("ddrm-durable-{tag}-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn durable_store_persists_and_reads_back_across_a_fresh_instance() {
        let dir = unique_tmp_dir("rt");
        {
            let mut store = DurableEventStore::open(&dir).unwrap();
            store.persist("bafy/release_receipt", &json!({ "event": "release_receipt", "n": 1 })).unwrap();
            store.persist("bafy/audit", &json!({ "event": "audit", "n": 2 })).unwrap();
        } // the writer goes away — durability must survive it.
        let loaded = DurableEventStore::load(&dir).unwrap();
        assert_eq!(loaded.len(), 2, "both records survived a fresh reader");
        // No torn `*.tmp` lingers — the atomic publish cleaned up after itself.
        let any_tmp = std::fs::read_dir(&dir)
            .unwrap()
            .any(|e| e.unwrap().path().extension().and_then(|x| x.to_str()) == Some("tmp"));
        assert!(!any_tmp, "no temp file left behind");
        // Idempotent: re-persisting the same key atomically replaces the record.
        {
            let mut store = DurableEventStore::open(&dir).unwrap();
            store.persist("bafy/audit", &json!({ "event": "audit", "n": 99 })).unwrap();
        }
        let reloaded = DurableEventStore::load(&dir).unwrap();
        assert_eq!(reloaded.len(), 2, "idempotent overwrite, not a duplicate");
        let audit = reloaded.iter().find(|(_, r)| r["event"] == json!("audit")).unwrap();
        assert_eq!(audit.1["n"], json!(99), "the record was replaced");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn durable_store_load_skips_a_corrupt_record() {
        let dir = unique_tmp_dir("corrupt");
        let mut store = DurableEventStore::open(&dir).unwrap();
        store.persist("bafy/release_receipt", &json!({ "event": "release_receipt" })).unwrap();
        // A corrupt file must be skipped on read-back, never served as if intact.
        std::fs::write(dir.join("bafy_audit.json"), b"{not json").unwrap();
        let loaded = DurableEventStore::load(&dir).unwrap();
        assert_eq!(loaded.len(), 1, "the corrupt record is skipped, the intact one survives");
        assert_eq!(loaded[0].1["event"], json!("release_receipt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn host_persists_durably_through_the_real_store() {
        let dir = unique_tmp_dir("host");
        let invoked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let fetched = std::rc::Rc::new(std::cell::RefCell::new(0u32));
        let mut host = DrmHost::new(
            Box::new(FakePlanSource { tamper: false, fetched }),
            full_table(&invoked),
            Box::new(PersistingEventSink::new(DurableEventStore::open(&dir).unwrap())),
        );
        host.open("bafycontent", "elastos.viewer/document@1").expect("open + durable persist");
        // A FRESH reader sees both durable records — the host persisted across the store.
        let loaded = DurableEventStore::load(&dir).unwrap();
        let events: Vec<&str> = loaded.iter().map(|(_, r)| r["event"].as_str().unwrap()).collect();
        assert!(events.contains(&"release_receipt"));
        assert!(events.contains(&"protected_content.open.audit"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
