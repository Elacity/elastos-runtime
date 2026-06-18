//! Capsule Inspector provider (`elastos://inspect/*`).
//!
//! This is the product-side home of the Capsule Inspector. Both product
//! transports — the browser gateway and the capsule `carrier_invoke` bridge —
//! dispatch resource calls through `ProviderRegistry::send_raw(scheme, …)`, so
//! a single provider registered for the `inspect` scheme serves both (the
//! "one canonical path", Principle #10).
//!
//! The authority *decision* is the shared, transport-agnostic core in
//! `elastos_runtime::inspect` (`InspectScope`). Access is gated upstream — the
//! gateway app-allow-list for the browser, the capability-resource contract for
//! capsules — exactly as every other provider scheme is gated; this provider is
//! a read-only projection over runtime-owned state.
//!
//! Security (Principle #16): the projection allow-lists safe fields and never
//! echoes a bearer token, a raw signature, or any mutation handle. The raw
//! manifest `signature` is reduced to `signature_present`.
//!
//! Data is read through the [`InspectSource`] trait. The provider holds a
//! strong `Arc<dyn InspectSource>`; each concrete source holds only a `Weak`
//! reference to the heavy runtime object it reads from, so registering the
//! provider on the registry never creates a reference cycle.

use std::path::PathBuf;
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use elastos_common::{CapsuleAffordanceDescriptor, CapsuleManifest};
use elastos_runtime::inspect::InspectScope;
use elastos_runtime::invoke::{self, InvokeError};
use elastos_runtime::provider::{
    Provider, ProviderError, ProviderRegistry, ResourceRequest, ResourceResponse,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::runtime::Runtime;

/// A short, non-secret provenance anchor for a manifest signature: the first 16
/// hex chars of SHA-256 over the decoded signature bytes (raw base64 if decode
/// fails). This identifies *which* signature signed the capsule for audit
/// correlation without ever echoing the signature material itself (#16).
fn signature_fingerprint(sig_b64: &str) -> Option<String> {
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    if sig_b64.is_empty() {
        return None;
    }
    let bytes = B64
        .decode(sig_b64)
        .unwrap_or_else(|_| sig_b64.as_bytes().to_vec());
    // `.take(16)` rather than a `[..16]` slice: can never panic regardless of the
    // digest's hex length (hex chars are single-byte ASCII, so no boundary risk).
    Some(
        hex::encode(Sha256::digest(&bytes))
            .chars()
            .take(16)
            .collect(),
    )
}

/// Derive a fail-closed trust classification from what is actually verifiable
/// about a capsule. We never claim more trust than the evidence supports: a
/// signature yields `signed`; absent that, a content address yields
/// `content-addressed`; with neither it is `unsigned`. Signer *verification* is
/// not yet wired (the manifest schema carries no signer DID/pubkey), so this is
/// presence-based, not a verified-identity claim.
fn trust_level(signature_present: bool, has_cid: bool) -> &'static str {
    if signature_present {
        "signed"
    } else if has_cid {
        "content-addressed"
    } else {
        "unsigned"
    }
}

/// A DID for the capsule only when one genuinely exists — the capsule id or the
/// declared author when it is a `did:` string. Never fabricated; `Null`
/// otherwise. A declared author DID is self-asserted, not verified.
fn capsule_did(entry_id: &str, author: &Value) -> Value {
    if entry_id.starts_with("did:") {
        return json!(entry_id);
    }
    match author.as_str() {
        Some(a) if a.starts_with("did:") => json!(a),
        _ => Value::Null,
    }
}

/// One inspectable capsule, as seen by the provider.
#[derive(Debug, Clone)]
pub struct InspectEntry {
    pub id: String,
    pub name: String,
    pub status: String,
    pub capsule_type: String,
    /// Manifest the capsule was launched with, when retained.
    pub manifest: Option<CapsuleManifest>,
    /// Content identity (IPFS CID) from the installed-capsule catalog, when
    /// known — the provenance anchor (Principle #15).
    pub cid: Option<String>,
}

/// Read-only source of inspectable capsules. Decouples the provider from the
/// server's (fragmented) capsule tracking, and lets sources be aggregated.
#[async_trait]
pub trait InspectSource: Send + Sync {
    async fn inspect_list(&self) -> Vec<InspectEntry>;
    async fn inspect_get(&self, id: &str) -> Option<InspectEntry>;
}

/// A single audited event, projected for display (no signatures/handles).
#[derive(Debug, Clone)]
pub struct AuditRecord {
    pub ts: u64,
    pub event: String,
    pub detail: String,
    pub success: bool,
    /// Whether this event is cryptographically attested (a signature is present).
    /// We surface the *fact* of attestation, never the signature itself (#16).
    pub signed: bool,
    /// The DID that attested the event, when present. A DID is public identity,
    /// safe to surface, and is the verified-signer evidence the audit plane
    /// actually carries (#15).
    pub signer: Option<String>,
}

/// A capsule's recent audit activity.
#[derive(Debug, Clone, Default)]
pub struct CapsuleAudit {
    pub total: u64,
    pub denied: u64,
    /// How many of the recent events are cryptographically attested.
    pub attested: u64,
    pub recent: Vec<AuditRecord>,
}

/// Read-only source of per-capsule audit activity. Optional on the provider;
/// when absent, the audit section is empty.
#[async_trait]
pub trait AuditSource: Send + Sync {
    /// Audit for `capsule_key`, newest-first, capped at `recent_limit`.
    async fn for_capsule(&self, capsule_key: &str, recent_limit: usize) -> CapsuleAudit;
}

/// Audit source backed by the signed runtime audit log in the auth state
/// (`RuntimeAuditEventV1`). Correlates by `capsule_id`.
pub struct AuthAuditSource {
    data_dir: PathBuf,
}

impl AuthAuditSource {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }
}

#[async_trait]
impl AuditSource for AuthAuditSource {
    async fn for_capsule(&self, capsule_key: &str, recent_limit: usize) -> CapsuleAudit {
        let data_dir = self.data_dir.clone();
        let key = capsule_key.to_string();
        // Auth state load is blocking std::fs; keep it off the async worker.
        tokio::task::spawn_blocking(move || {
            let state = match crate::auth::load_auth_state(&data_dir) {
                Ok(state) => state,
                Err(_) => return CapsuleAudit::default(),
            };
            let mut total = 0u64;
            let mut denied = 0u64;
            let mut attested = 0u64;
            let mut recent = Vec::new();
            // Newest-first.
            for event in state.audit.iter().rev() {
                if event.capsule_id.as_deref() != Some(key.as_str()) {
                    continue;
                }
                total += 1;
                let success = matches!(event.result.as_str(), "ok" | "success" | "allowed");
                if !success {
                    denied += 1;
                }
                let signed = event.signature.is_some();
                if signed {
                    attested += 1;
                }
                if recent.len() < recent_limit {
                    recent.push(AuditRecord {
                        ts: event.occurred_at,
                        event: event.event_type.clone(),
                        detail: event.reason.clone(),
                        success,
                        signed,
                        signer: event.signer_did.clone(),
                    });
                }
            }
            CapsuleAudit {
                total,
                denied,
                attested,
                recent,
            }
        })
        .await
        .unwrap_or_default()
    }
}

// ── Sources ─────────────────────────────────────────────────────────

/// Source backed by the server `Runtime`'s running-capsule registry (the
/// capsules launched with a retained manifest — e.g. the single-VM serve path).
pub struct RuntimeInspectSource {
    runtime: Weak<Runtime>,
}

impl RuntimeInspectSource {
    pub fn new(runtime: Weak<Runtime>) -> Self {
        Self { runtime }
    }
}

fn running_to_entry(info: crate::runtime::RunningCapsuleInfo) -> InspectEntry {
    InspectEntry {
        id: info.id,
        name: info.name,
        status: info.status,
        capsule_type: format!("{:?}", info.capsule_type).to_lowercase(),
        manifest: Some(*info.manifest),
        cid: None,
    }
}

#[async_trait]
impl InspectSource for RuntimeInspectSource {
    async fn inspect_list(&self) -> Vec<InspectEntry> {
        match self.runtime.upgrade() {
            Some(rt) => rt
                .list_capsules()
                .await
                .into_iter()
                .map(running_to_entry)
                .collect(),
            None => Vec::new(),
        }
    }

    async fn inspect_get(&self, id: &str) -> Option<InspectEntry> {
        self.runtime
            .upgrade()?
            .get_capsule(id)
            .await
            .map(running_to_entry)
    }
}

/// Source backed by the `ProviderRegistry`: the registered provider schemes
/// (the running provider capsules/services). Always populated on the main
/// product path. Thin — the registry does not carry per-provider manifests, so
/// these entries have no manifest (affordances/capabilities are empty until a
/// catalog/manifest source enriches them).
pub struct RegistryInspectSource {
    registry: Weak<ProviderRegistry>,
}

impl RegistryInspectSource {
    pub fn new(registry: Weak<ProviderRegistry>) -> Self {
        Self { registry }
    }

    fn scheme_entry(scheme: String) -> InspectEntry {
        InspectEntry {
            id: format!("provider:{scheme}"),
            name: scheme,
            status: "running".to_string(),
            capsule_type: "provider".to_string(),
            manifest: None,
            cid: None,
        }
    }
}

#[async_trait]
impl InspectSource for RegistryInspectSource {
    async fn inspect_list(&self) -> Vec<InspectEntry> {
        match self.registry.upgrade() {
            Some(reg) => {
                let mut schemes = reg.schemes().await;
                schemes.extend(reg.sub_provider_schemes().await);
                schemes.sort();
                schemes.dedup();
                schemes.into_iter().map(Self::scheme_entry).collect()
            }
            None => Vec::new(),
        }
    }

    async fn inspect_get(&self, id: &str) -> Option<InspectEntry> {
        let scheme = id.strip_prefix("provider:")?;
        let reg = self.registry.upgrade()?;
        let known = reg.has_provider(scheme).await
            || reg.sub_provider_schemes().await.iter().any(|s| s == scheme);
        known.then(|| Self::scheme_entry(scheme.to_string()))
    }
}

/// Source backed by the installed-capsule catalog on disk:
/// `<data_dir>/capsules/<name>/capsule.json`. Reads each capsule's full
/// manifest (rich detail: capabilities, affordances, provenance) and marks it
/// `running` when the scheme it `provides` is registered in the live registry,
/// else `installed`. This is the rich, manifest-backed source for the product.
pub struct CatalogInspectSource {
    capsules_dir: PathBuf,
    registry: Weak<ProviderRegistry>,
}

impl CatalogInspectSource {
    pub fn new(capsules_dir: PathBuf, registry: Weak<ProviderRegistry>) -> Self {
        Self {
            capsules_dir,
            registry,
        }
    }

    /// The scheme a provider capsule serves, parsed from `provides`
    /// (e.g. `elastos://wallet/*` → `wallet`).
    fn provided_scheme(manifest: &CapsuleManifest) -> Option<String> {
        manifest
            .provides
            .as_ref()?
            .strip_prefix("elastos://")
            .and_then(|rest| rest.split('/').next())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }

    async fn running_schemes(&self) -> std::collections::HashSet<String> {
        match self.registry.upgrade() {
            Some(reg) => {
                let mut set: std::collections::HashSet<String> =
                    reg.schemes().await.into_iter().collect();
                set.extend(reg.sub_provider_schemes().await);
                set
            }
            None => std::collections::HashSet::new(),
        }
    }

    /// Content identities (capsule name → CID) from the installed-capsule
    /// catalog (`<data_dir>/components.json`). Best-effort: empty if absent.
    async fn catalog_cids(&self) -> std::collections::HashMap<String, String> {
        let Some(path) = self
            .capsules_dir
            .parent()
            .map(|p| p.join("components.json"))
        else {
            return std::collections::HashMap::new();
        };
        let Ok(data) = tokio::fs::read_to_string(&path).await else {
            return std::collections::HashMap::new();
        };
        match serde_json::from_str::<crate::setup::ComponentsManifest>(&data) {
            Ok(manifest) => manifest
                .capsules
                .into_iter()
                .filter(|(_, entry)| !entry.cid.is_empty())
                .map(|(name, entry)| (name, entry.cid))
                .collect(),
            Err(_) => std::collections::HashMap::new(),
        }
    }

    async fn read_entry(
        &self,
        name: &str,
        running: &std::collections::HashSet<String>,
        cid: Option<String>,
    ) -> Option<InspectEntry> {
        let path = self.capsules_dir.join(name).join("capsule.json");
        let data = tokio::fs::read_to_string(&path).await.ok()?;
        let manifest: CapsuleManifest = serde_json::from_str(&data).ok()?;
        let is_running = Self::provided_scheme(&manifest)
            .map(|s| running.contains(&s))
            .unwrap_or(false);
        Some(InspectEntry {
            id: format!("capsule:{name}"),
            name: manifest.name.clone(),
            status: if is_running { "running" } else { "installed" }.to_string(),
            capsule_type: format!("{:?}", manifest.capsule_type).to_lowercase(),
            manifest: Some(manifest),
            cid,
        })
    }
}

#[async_trait]
impl InspectSource for CatalogInspectSource {
    async fn inspect_list(&self) -> Vec<InspectEntry> {
        let running = self.running_schemes().await;
        let cids = self.catalog_cids().await;
        let mut out = Vec::new();
        if let Ok(mut rd) = tokio::fs::read_dir(&self.capsules_dir).await {
            while let Ok(Some(dir_entry)) = rd.next_entry().await {
                let is_dir = dir_entry
                    .file_type()
                    .await
                    .map(|t| t.is_dir())
                    .unwrap_or(false);
                if !is_dir {
                    continue;
                }
                if let Some(name) = dir_entry.file_name().to_str() {
                    let cid = cids.get(name).cloned();
                    if let Some(entry) = self.read_entry(name, &running, cid).await {
                        out.push(entry);
                    }
                }
            }
        }
        out
    }

    async fn inspect_get(&self, id: &str) -> Option<InspectEntry> {
        let name = id.strip_prefix("capsule:")?;
        // Reject path traversal in the id.
        if name.contains('/') || name.contains("..") {
            return None;
        }
        let running = self.running_schemes().await;
        let cid = self.catalog_cids().await.get(name).cloned();
        self.read_entry(name, &running, cid).await
    }
}

/// Aggregates several sources into one. This is the unification point: the main
/// product path composes the runtime, catalog, and/or registry sources so the
/// browser Inspector shows every capsule any source knows about. De-duplicates
/// by id (first source wins).
pub struct AggregateInspectSource {
    sources: Vec<Arc<dyn InspectSource>>,
}

impl AggregateInspectSource {
    pub fn new(sources: Vec<Arc<dyn InspectSource>>) -> Self {
        Self { sources }
    }
}

#[async_trait]
impl InspectSource for AggregateInspectSource {
    async fn inspect_list(&self) -> Vec<InspectEntry> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for source in &self.sources {
            for entry in source.inspect_list().await {
                if seen.insert(entry.id.clone()) {
                    out.push(entry);
                }
            }
        }
        out
    }

    async fn inspect_get(&self, id: &str) -> Option<InspectEntry> {
        for source in &self.sources {
            if let Some(entry) = source.inspect_get(id).await {
                return Some(entry);
            }
        }
        None
    }
}

// ── Provider ────────────────────────────────────────────────────────

pub struct InspectProvider {
    source: Arc<dyn InspectSource>,
    audit: Option<Arc<dyn AuditSource>>,
}

impl InspectProvider {
    pub fn new(source: Arc<dyn InspectSource>) -> Self {
        Self {
            source,
            audit: None,
        }
    }

    /// Attach a per-capsule audit source so detail views show live activity.
    pub fn with_audit(mut self, audit: Arc<dyn AuditSource>) -> Self {
        self.audit = Some(audit);
        self
    }

    fn scope_label(scope: InspectScope) -> &'static str {
        match scope {
            InspectScope::System => "system",
            InspectScope::SelfOnly => "self",
        }
    }

    /// Project a capsule into the inspector wire contract (see
    /// docs/CAPSULE_INSPECTOR.md). Read-only; unknown fields are null rather
    /// than fabricated, and no bearer token / raw signature is ever included.
    fn project(entry: &InspectEntry, audit: Value) -> Value {
        fn field(v: &Value, key: &str) -> Value {
            v.get(key).cloned().unwrap_or(Value::Null)
        }

        // Serialize the manifest once and pick allow-listed fields. The raw
        // `signature` is deliberately *not* surfaced — only its presence.
        let manifest = entry
            .manifest
            .as_ref()
            .and_then(|m| serde_json::to_value(m).ok())
            .unwrap_or_else(|| json!({}));

        let signature_present = manifest
            .get("signature")
            .map(|s| s.is_string())
            .unwrap_or(false);
        let cid = entry.cid.clone();

        // Provenance, derived honestly from what is actually present — never
        // fabricated (#15: trust travels with DID/CID/hash/sig).
        let author = field(&manifest, "author");
        let did = capsule_did(&entry.id, &author);
        let trust = trust_level(signature_present, cid.is_some());
        let sig_fingerprint = manifest
            .get("signature")
            .and_then(Value::as_str)
            .and_then(signature_fingerprint)
            .map(Value::from)
            .unwrap_or(Value::Null);

        // Provider authority — the declarative powers a provider capsule is
        // authorized for (resource/actions/operations + audit events). DDRM and
        // other provider capsules express their real powers here, not via
        // interface methods, so surfacing it is what makes the Inspector show
        // what a provider can actually do (e.g. key release, decrypt render,
        // rights decisions). Declarative metadata only — no secrets/handles.
        let authority = manifest
            .get("authority")
            .map(|a| {
                json!({
                    "reason": a.get("reason").cloned().unwrap_or(Value::Null),
                    "capabilities": a.get("capabilities").cloned().unwrap_or(Value::Null),
                    "audit_events": a.get("audit_events").cloned().unwrap_or(Value::Null),
                })
            })
            .unwrap_or(Value::Null);

        // Affordances: flatten declared interface methods.
        let mut affordances = Vec::new();
        if let Some(interfaces) = manifest.get("interfaces").and_then(|v| v.as_array()) {
            for iface in interfaces {
                let iface_id = field(iface, "id");
                if let Some(methods) = iface.get("methods").and_then(|v| v.as_array()) {
                    for m in methods {
                        // The typed interface contract (metadata-driven
                        // reflection): risk/approval/audit class plus the
                        // input/output schemas that describe how to invoke the
                        // affordance — the basis for typed, location-agnostic,
                        // capability-gated calls, not just display.
                        affordances.push(json!({
                            "interface": iface_id,
                            "id": field(m, "id"),
                            "risk": field(m, "risk"),
                            "approval": field(m, "approval"),
                            "audit": field(m, "audit"),
                            "description": field(m, "description"),
                            "input_schema": field(m, "input_schema"),
                            "output_schema": field(m, "output_schema"),
                        }));
                    }
                }
            }
        }

        json!({
            "id": entry.id,
            "name": entry.name,
            "version": field(&manifest, "version"),
            "role": field(&manifest, "role"),
            "type": field(&manifest, "type"),
            "description": field(&manifest, "description"),
            "author": field(&manifest, "author"),
            "identity": {
                "did": did.clone(),
                "cid": cid.clone(),
                "trust_level": trust,
                "signature_present": signature_present,
                // No verified signer is available from the manifest schema; we do
                // not invent one. `signed_by` stays null until verification is wired.
                "signed_by": Value::Null,
            },
            "manifest": {
                "schema": field(&manifest, "schema"),
                "entrypoint": field(&manifest, "entrypoint"),
            },
            "affordances": affordances,
            // Provider powers (declarative authority), for provider capsules.
            "authority": authority,
            "required_capabilities": field(&manifest, "capabilities"),
            // Bearer-token object-capabilities have no central per-capsule grant
            // registry; observed grants come from the audit plane (not yet wired
            // into this provider). Empty rather than fabricated.
            "granted_capabilities": Value::Array(vec![]),
            "storage_namespaces": manifest
                .pointer("/permissions/storage")
                .cloned()
                .unwrap_or(Value::Null),
            "carrier": {
                "enabled": manifest.pointer("/permissions/carrier").cloned().unwrap_or(Value::Null),
                "endpoints": [],
                "peers": 0,
            },
            "provenance": {
                // Self-declared author (unverified) and the trust evidence we can
                // actually derive. `signed_by` (a verified signer) is not yet
                // available, so it stays null rather than echoing the author as if
                // verified.
                "author": author,
                "signed_by": Value::Null,
                "trust_level": trust,
                "version": field(&manifest, "version"),
                "installed_at": Value::Null,
                "cid": cid.clone(),
                "signature_present": signature_present,
                "signature_fingerprint": sig_fingerprint,
            },
            "audit": audit,
            "processes": [{ "kind": entry.capsule_type, "status": entry.status }],
        })
    }

    /// Build the audit section for a capsule from the audit source (keyed by
    /// the capsule name, which is how runtime audit events record `capsule_id`).
    /// Empty when no audit source is attached.
    async fn audit_value(&self, entry: &InspectEntry) -> Value {
        let Some(audit) = &self.audit else {
            return json!({ "counts": { "total": 0, "denied": 0, "attested": 0 }, "recent": [] });
        };
        let a = audit.for_capsule(&entry.name, 20).await;
        let recent: Vec<Value> = a
            .recent
            .iter()
            .map(|r| {
                json!({
                    "ts": r.ts, "event": r.event, "detail": r.detail, "success": r.success,
                    "signed": r.signed, "signer": r.signer,
                })
            })
            .collect();
        json!({
            "counts": { "total": a.total, "denied": a.denied, "attested": a.attested },
            "recent": recent,
        })
    }

    async fn handle_op(&self, request: &Value) -> Value {
        match request.get("op").and_then(Value::as_str).unwrap_or("") {
            // System-scope list. Upstream (gateway allow-list / capability
            // contract) gates who may reach this op.
            "capsules" => {
                let capsules: Vec<Value> = self
                    .source
                    .inspect_list()
                    .await
                    .iter()
                    .map(|e| {
                        json!({
                            "id": e.id,
                            "name": e.name,
                            "role": e.manifest.as_ref().and_then(|m| serde_json::to_value(&m.role).ok()),
                            "type": e.capsule_type,
                            "state": e.status,
                        })
                    })
                    .collect();
                json!({
                    "status": "ok",
                    "data": { "scope": Self::scope_label(InspectScope::System), "capsules": capsules },
                })
            }
            // System-scope detail.
            "capsule" => match request.get("id").and_then(Value::as_str) {
                Some(id) => match self.source.inspect_get(id).await {
                    Some(entry) => {
                        let audit = self.audit_value(&entry).await;
                        json!({ "status": "ok", "data": Self::project(&entry, audit) })
                    }
                    None => provider_error("not_found", "no such capsule"),
                },
                None => provider_error("invalid_request", "inspect/capsule requires an \"id\""),
            },
            // Metadata-driven invocation *preview* (read-only, dry-run): given a
            // capsule + interface + method + args, validate the args against the
            // affordance's input_schema and derive the capability/approval/audit
            // gate the call would require. Dispatches NO effect — this is the
            // reflective half of the CAR invoke kernel.
            "plan" => self.handle_plan(request).await,
            other => provider_error("unknown_op", &format!("unknown inspect op: {other}")),
        }
    }

    async fn handle_plan(&self, request: &Value) -> Value {
        let id = match request.get("id").and_then(Value::as_str) {
            Some(id) => id,
            None => return provider_error("invalid_request", "inspect/plan requires an \"id\""),
        };
        let entry = match self.source.inspect_get(id).await {
            Some(entry) => entry,
            None => return provider_error("not_found", "no such capsule"),
        };
        let manifest = match entry.manifest.as_ref() {
            Some(m) => m,
            None => return provider_error("not_found", "capsule has no manifest to plan against"),
        };

        // Two reflective modes, never mixed:
        //  - interface/method → preview an affordance call (interfaces[].methods);
        //  - operation        → preview a provider power (authority.capabilities[]).
        match (
            request.get("interface").and_then(Value::as_str),
            request.get("method").and_then(Value::as_str),
            request.get("operation").and_then(Value::as_str),
        ) {
            (Some(interface), Some(method), None) => {
                Self::plan_affordance(manifest, interface, method, request)
            }
            (None, None, Some(operation)) => Self::plan_operation(manifest, operation),
            _ => provider_error(
                "invalid_request",
                "inspect/plan requires either \"interface\"+\"method\" or \"operation\"",
            ),
        }
    }

    /// Affordance preview: validate args against the input schema and derive the
    /// capability/approval/audit gate.
    fn plan_affordance(
        manifest: &CapsuleManifest,
        interface: &str,
        method: &str,
        request: &Value,
    ) -> Value {
        let affordance = match find_affordance(manifest, interface, method) {
            Some(a) => a,
            None => return provider_error("not_found", "no such affordance"),
        };
        let args = request.get("args").cloned().unwrap_or(json!({}));

        match invoke::plan(&affordance, &args) {
            Ok(plan) => json!({
                "status": "ok",
                "data": {
                    "valid": true,
                    "kind": "affordance",
                    // The gate the runtime would enforce for this call.
                    "capability_action": plan.capability_action.to_string(),
                    "approval": serde_json::to_value(&plan.approval).ok(),
                    "audit": serde_json::to_value(&plan.audit).ok(),
                }
            }),
            // The query succeeded; the proposed args do not satisfy the contract.
            Err(InvokeError::MissingRequiredField(field)) => json!({
                "status": "ok",
                "data": { "valid": false, "error": "missing_required_field", "field": field }
            }),
            Err(InvokeError::InputTypeMismatch { expected }) => json!({
                "status": "ok",
                "data": { "valid": false, "error": "input_type_mismatch", "expected": expected }
            }),
            // Affordance planning never raises the provider-authority variants.
            Err(other) => provider_error("invalid_request", &format!("{other:?}")),
        }
    }

    /// Provider-power preview: reflect the `authority` metadata to show the exact
    /// capability tuple (resource + actions) an operation demands. Read-only.
    fn plan_operation(manifest: &CapsuleManifest, operation: &str) -> Value {
        let authority = match manifest.authority.as_ref() {
            Some(a) => a,
            None => {
                return provider_error(
                    "invalid_request",
                    "capsule declares no provider authority to plan against",
                )
            }
        };
        match invoke::plan_provider_operation(authority, operation) {
            Ok(plan) => json!({
                "status": "ok",
                "data": {
                    "valid": true,
                    "kind": "operation",
                    // Union of every resource the op touches across all matching
                    // authority blocks — never just the first (fail-closed).
                    "resources": plan.resources,
                    // Every action a caller's capability must cover (full set).
                    "capability_actions": plan
                        .actions
                        .iter()
                        .map(|a| a.to_string())
                        .collect::<Vec<_>>(),
                    "audit_events": plan.audit_events,
                }
            }),
            Err(InvokeError::UnknownOperation(op)) => json!({
                "status": "ok",
                "data": { "valid": false, "error": "unknown_operation", "operation": op }
            }),
            Err(InvokeError::UnknownDeclaredAction(action)) => provider_error(
                "manifest_error",
                &format!("authority declares an unknown action \"{action}\""),
            ),
            Err(other) => provider_error("invalid_request", &format!("{other:?}")),
        }
    }
}

/// Look up an affordance descriptor in a manifest by interface id + method id.
fn find_affordance(
    manifest: &CapsuleManifest,
    interface: &str,
    method: &str,
) -> Option<CapsuleAffordanceDescriptor> {
    manifest
        .interfaces
        .iter()
        .find(|iface| iface.id == interface)?
        .methods
        .iter()
        .find(|m| m.id == method)
        .cloned()
}

#[async_trait]
impl Provider for InspectProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "inspect provider uses raw operations; route via send_raw".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["inspect"]
    }

    fn name(&self) -> &'static str {
        "inspect-provider"
    }

    async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
        Ok(self.handle_op(request).await)
    }
}

fn provider_error(code: &str, message: &str) -> Value {
    json!({ "status": "error", "code": code, "message": message })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe_manifest() -> CapsuleManifest {
        serde_json::from_value(json!({
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "probe",
            "role": "app",
            "type": "wasm",
            "entrypoint": "probe.wasm",
            "capabilities": ["elastos://storage/probe"],
            "interfaces": [{
                "id": "elastos.probe/v1",
                "version": "1",
                "methods": [{
                    "id": "ping", "risk": "read", "approval": "none", "audit": "summary",
                    "input_schema": { "type": "object" },
                    "output_schema": { "type": "string" }
                }]
            }],
            "permissions": { "storage": ["localhost://WebSpaces/probe/"] },
            "signature": "SECRET_SIGNATURE_MUST_NOT_LEAK"
        }))
        .expect("probe manifest deserializes")
    }

    struct MockSource {
        entries: Vec<InspectEntry>,
    }

    #[async_trait]
    impl InspectSource for MockSource {
        async fn inspect_list(&self) -> Vec<InspectEntry> {
            self.entries.clone()
        }
        async fn inspect_get(&self, id: &str) -> Option<InspectEntry> {
            self.entries.iter().find(|e| e.id == id).cloned()
        }
    }

    fn probe_entry() -> InspectEntry {
        InspectEntry {
            id: "cap_probe_1".to_string(),
            name: "probe".to_string(),
            status: "running".to_string(),
            capsule_type: "wasm".to_string(),
            manifest: Some(probe_manifest()),
            cid: None,
        }
    }

    fn provider_with_probe() -> InspectProvider {
        InspectProvider::new(Arc::new(MockSource {
            entries: vec![probe_entry()],
        }))
    }

    #[tokio::test]
    async fn capsules_lists_with_system_scope() {
        let resp = provider_with_probe()
            .send_raw(&json!({ "op": "capsules" }))
            .await
            .unwrap();
        assert_eq!(resp["status"], "ok");
        assert_eq!(resp["data"]["scope"], "system");
        assert_eq!(resp["data"]["capsules"][0]["id"], "cap_probe_1");
        assert_eq!(resp["data"]["capsules"][0]["name"], "probe");
    }

    #[tokio::test]
    async fn capsule_detail_renders_contract_without_leaking_authority() {
        let resp = provider_with_probe()
            .send_raw(&json!({ "op": "capsule", "id": "cap_probe_1" }))
            .await
            .unwrap();
        assert_eq!(resp["status"], "ok");
        let data = &resp["data"];
        assert_eq!(data["affordances"][0]["id"], "ping");
        assert_eq!(data["affordances"][0]["risk"], "read");
        // Typed interface contract is surfaced (metadata-driven reflection).
        assert_eq!(data["affordances"][0]["input_schema"]["type"], "object");
        assert_eq!(data["affordances"][0]["output_schema"]["type"], "string");
        assert_eq!(data["required_capabilities"][0], "elastos://storage/probe");
        assert_eq!(
            data["storage_namespaces"][0],
            "localhost://WebSpaces/probe/"
        );
        assert_eq!(data["identity"]["signature_present"], true);

        // Principle #16: never echo the raw signature or any bearer token.
        let serialized = serde_json::to_string(data).unwrap();
        assert!(
            !serialized.contains("SECRET_SIGNATURE_MUST_NOT_LEAK"),
            "raw signature leaked into inspect output"
        );
        assert!(
            !serialized.contains("\"token\""),
            "bearer token field leaked into inspect output"
        );
    }

    #[tokio::test]
    async fn provenance_is_derived_honestly_not_fabricated() {
        // The signed probe (id is not a DID, no cid): trust is "signed", a
        // signature fingerprint is present, and we never invent a signer DID.
        let resp = provider_with_probe()
            .send_raw(&json!({ "op": "capsule", "id": "cap_probe_1" }))
            .await
            .unwrap();
        let data = &resp["data"];
        assert_eq!(data["identity"]["trust_level"], "signed");
        assert_eq!(data["identity"]["did"], Value::Null);
        assert_eq!(data["provenance"]["signed_by"], Value::Null);
        // A real, non-secret 16-hex fingerprint that is NOT the raw signature.
        let fp = data["provenance"]["signature_fingerprint"]
            .as_str()
            .unwrap();
        assert_eq!(fp.len(), 16);
        assert!(fp.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(fp, "SECRET_SIGNATURE_MUST_NOT_LEAK");
    }

    // ── KNOWN_GAPS ratchet tests ────────────────────────────────────────
    // These encode the desired end-state of an OPEN gap (see docs/KNOWN_GAPS.md)
    // and fail today, so they are #[ignore]d (non-blocking in a shared tree).
    // Closing a gap = wire the feature, delete the #[ignore], the test goes
    // green. Run `cargo test -- --ignored` to see them fail (proving they are
    // real ratchets, not vacuous passes).

    #[tokio::test]
    #[ignore = "KNOWN_GAPS G1: granted_capabilities not yet wired (no resource/action in audit events); see docs/KNOWN_GAPS.md"]
    async fn ratchet_granted_capabilities_populated() {
        // Desired: a capsule with grants reports them. Today the projection
        // always returns []. When an observed-grant source is wired, delete the
        // #[ignore] and this goes green.
        let resp = provider_with_probe()
            .send_raw(&json!({ "op": "capsule", "id": "cap_probe_1" }))
            .await
            .unwrap();
        let granted = resp["data"]["granted_capabilities"].as_array().unwrap();
        assert!(
            !granted.is_empty(),
            "granted_capabilities should be populated once an observed-grant source exists"
        );
    }

    #[tokio::test]
    #[ignore = "KNOWN_GAPS G2: signer verification not yet wired (manifest has no signer DID); see docs/KNOWN_GAPS.md"]
    async fn ratchet_provenance_verified_signer_present() {
        // Desired: a signed capsule reports a *verified* signer. Today signed_by
        // is null (we refuse to present the declared author as verified). When a
        // signature-verification source is wired, delete the #[ignore].
        let resp = provider_with_probe()
            .send_raw(&json!({ "op": "capsule", "id": "cap_probe_1" }))
            .await
            .unwrap();
        assert!(
            !resp["data"]["provenance"]["signed_by"].is_null(),
            "provenance.signed_by should carry a verified signer once verification is wired"
        );
    }

    #[tokio::test]
    async fn provenance_surfaces_did_when_genuinely_present() {
        // A capsule whose id IS a DID: surface it (not fabricated — it exists).
        let mut entry = probe_entry();
        entry.id = "did:elastos:abc123".to_string();
        let provider = InspectProvider::new(Arc::new(MockSource {
            entries: vec![entry],
        }));
        let resp = provider
            .send_raw(&json!({ "op": "capsule", "id": "did:elastos:abc123" }))
            .await
            .unwrap();
        assert_eq!(resp["data"]["identity"]["did"], "did:elastos:abc123");
    }

    #[test]
    fn trust_level_fails_closed() {
        // Never claims more trust than the evidence supports.
        assert_eq!(trust_level(true, true), "signed");
        assert_eq!(trust_level(true, false), "signed");
        assert_eq!(trust_level(false, true), "content-addressed");
        assert_eq!(trust_level(false, false), "unsigned");
    }

    #[test]
    fn signature_fingerprint_is_stable_and_non_empty() {
        let a = signature_fingerprint("c2lnbmF0dXJl").unwrap();
        let b = signature_fingerprint("c2lnbmF0dXJl").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
        assert_eq!(signature_fingerprint(""), None);
    }

    #[tokio::test]
    async fn capsule_detail_unknown_id_is_not_found() {
        let resp = provider_with_probe()
            .send_raw(&json!({ "op": "capsule", "id": "nope" }))
            .await
            .unwrap();
        assert_eq!(resp["status"], "error");
        assert_eq!(resp["code"], "not_found");
    }

    #[tokio::test]
    async fn unknown_op_is_rejected() {
        let resp = provider_with_probe()
            .send_raw(&json!({ "op": "revoke" }))
            .await
            .unwrap();
        assert_eq!(resp["status"], "error");
        assert_eq!(resp["code"], "unknown_op");
    }

    #[tokio::test]
    async fn registry_source_lists_registered_schemes() {
        // A real registry with a registered provider scheme appears in inspect.
        let registry = Arc::new(ProviderRegistry::new());
        registry.register(Arc::new(MockSchemeProvider)).await;
        let source = RegistryInspectSource::new(Arc::downgrade(&registry));

        let entries = source.inspect_list().await;
        assert!(entries
            .iter()
            .any(|e| e.name == "wallet" && e.id == "provider:wallet"));
        assert!(source.inspect_get("provider:wallet").await.is_some());
        assert!(source.inspect_get("provider:nope").await.is_none());
    }

    #[tokio::test]
    async fn aggregate_source_unions_and_dedups() {
        let a: Arc<dyn InspectSource> = Arc::new(MockSource {
            entries: vec![probe_entry()],
        });
        let b: Arc<dyn InspectSource> = Arc::new(MockSource {
            entries: vec![
                probe_entry(), // duplicate id — should be deduped
                InspectEntry {
                    id: "provider:wallet".to_string(),
                    name: "wallet".to_string(),
                    status: "running".to_string(),
                    capsule_type: "provider".to_string(),
                    manifest: None,
                    cid: None,
                },
            ],
        });
        let agg = AggregateInspectSource::new(vec![a, b]);
        let entries = agg.inspect_list().await;
        assert_eq!(entries.len(), 2, "duplicate ids must be deduped");
        assert!(agg.inspect_get("provider:wallet").await.is_some());
    }

    fn write_capsule(capsules_dir: &std::path::Path, dir_name: &str, manifest: &Value) {
        let dir = capsules_dir.join(dir_name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("capsule.json"),
            serde_json::to_vec_pretty(manifest).unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn catalog_reads_installed_manifest_richly_without_leaking() {
        let tmp = tempfile::tempdir().unwrap();
        let capsules_dir = tmp.path().join("capsules");
        write_capsule(
            &capsules_dir,
            "probe",
            &serde_json::to_value(probe_manifest()).unwrap(),
        );

        let registry = Arc::new(ProviderRegistry::new());
        let source = CatalogInspectSource::new(capsules_dir, Arc::downgrade(&registry));

        let entries = source.inspect_list().await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "capsule:probe");
        assert_eq!(entries[0].status, "installed"); // nothing registered

        // Detail is rich and never leaks the raw signature.
        let provider = InspectProvider::new(Arc::new(source));
        let resp = provider
            .send_raw(&json!({ "op": "capsule", "id": "capsule:probe" }))
            .await
            .unwrap();
        assert_eq!(resp["data"]["affordances"][0]["id"], "ping");
        assert_eq!(
            resp["data"]["required_capabilities"][0],
            "elastos://storage/probe"
        );
        let serialized = serde_json::to_string(&resp).unwrap();
        assert!(!serialized.contains("SECRET_SIGNATURE_MUST_NOT_LEAK"));
    }

    #[tokio::test]
    async fn catalog_marks_running_when_provided_scheme_registered() {
        let tmp = tempfile::tempdir().unwrap();
        let capsules_dir = tmp.path().join("capsules");
        write_capsule(
            &capsules_dir,
            "wallet-provider",
            &json!({
                "schema": "elastos.capsule/v1",
                "version": "0.1.0",
                "name": "wallet-provider",
                "role": "app",
                "type": "wasm",
                "entrypoint": "wallet.wasm",
                "provides": "elastos://wallet/*"
            }),
        );

        let registry = Arc::new(ProviderRegistry::new());
        registry.register(Arc::new(MockSchemeProvider)).await; // registers scheme "wallet"
        let source = CatalogInspectSource::new(capsules_dir, Arc::downgrade(&registry));

        let entry = source.inspect_get("capsule:wallet-provider").await.unwrap();
        assert_eq!(entry.status, "running");

        // Path-traversal ids are rejected.
        assert!(source.inspect_get("capsule:../etc").await.is_none());
    }

    #[tokio::test]
    async fn detail_includes_live_audit_when_source_attached() {
        struct MockAudit;
        #[async_trait]
        impl AuditSource for MockAudit {
            async fn for_capsule(&self, key: &str, _limit: usize) -> CapsuleAudit {
                if key == "probe" {
                    CapsuleAudit {
                        total: 3,
                        denied: 1,
                        attested: 1,
                        recent: vec![AuditRecord {
                            ts: 100,
                            event: "capability.use".to_string(),
                            detail: "did read".to_string(),
                            success: true,
                            signed: true,
                            signer: Some("did:elastos:gateway".to_string()),
                        }],
                    }
                } else {
                    CapsuleAudit::default()
                }
            }
        }

        let provider = InspectProvider::new(Arc::new(MockSource {
            entries: vec![probe_entry()],
        }))
        .with_audit(Arc::new(MockAudit));
        let resp = provider
            .send_raw(&json!({ "op": "capsule", "id": "cap_probe_1" }))
            .await
            .unwrap();
        assert_eq!(resp["data"]["audit"]["counts"]["total"], 3);
        assert_eq!(resp["data"]["audit"]["counts"]["denied"], 1);
        // Attestation fidelity: who cryptographically signed each event (#15),
        // surfaced as presence + DID, never the signature itself (#16).
        assert_eq!(resp["data"]["audit"]["counts"]["attested"], 1);
        assert_eq!(
            resp["data"]["audit"]["recent"][0]["event"],
            "capability.use"
        );
        assert_eq!(resp["data"]["audit"]["recent"][0]["success"], true);
        assert_eq!(resp["data"]["audit"]["recent"][0]["signed"], true);
        assert_eq!(
            resp["data"]["audit"]["recent"][0]["signer"],
            "did:elastos:gateway"
        );
    }

    #[tokio::test]
    async fn plan_previews_gate_for_valid_call() {
        let resp = provider_with_probe()
            .send_raw(&json!({
                "op": "plan", "id": "cap_probe_1",
                "interface": "elastos.probe/v1", "method": "ping", "args": {}
            }))
            .await
            .unwrap();
        assert_eq!(resp["status"], "ok");
        assert_eq!(resp["data"]["valid"], true);
        assert_eq!(resp["data"]["capability_action"], "read");
        assert_eq!(resp["data"]["approval"], "none");
    }

    #[tokio::test]
    async fn plan_reports_input_type_mismatch() {
        // ping declares input_schema {type:object}; a scalar must fail validation.
        let resp = provider_with_probe()
            .send_raw(&json!({
                "op": "plan", "id": "cap_probe_1",
                "interface": "elastos.probe/v1", "method": "ping", "args": "scalar"
            }))
            .await
            .unwrap();
        assert_eq!(resp["status"], "ok");
        assert_eq!(resp["data"]["valid"], false);
        assert_eq!(resp["data"]["error"], "input_type_mismatch");
    }

    #[tokio::test]
    async fn plan_unknown_affordance_is_not_found() {
        let resp = provider_with_probe()
            .send_raw(&json!({
                "op": "plan", "id": "cap_probe_1",
                "interface": "elastos.probe/v1", "method": "nope", "args": {}
            }))
            .await
            .unwrap();
        assert_eq!(resp["status"], "error");
        assert_eq!(resp["code"], "not_found");
    }

    #[tokio::test]
    async fn registry_dispatch_reaches_inspect_provider() {
        // The leg both product transports converge on: ProviderRegistry::send_raw
        // by scheme must resolve to the registered inspect provider.
        let registry = Arc::new(ProviderRegistry::new());
        registry
            .register(Arc::new(InspectProvider::new(Arc::new(MockSource {
                entries: vec![probe_entry()],
            }))))
            .await;
        let resp = registry
            .send_raw("inspect", &json!({ "op": "capsules" }))
            .await
            .expect("registry dispatch");
        assert_eq!(resp["status"], "ok");
        assert_eq!(resp["data"]["capsules"][0]["id"], "cap_probe_1");
    }

    #[tokio::test]
    async fn registry_source_includes_sub_providers() {
        let registry = Arc::new(ProviderRegistry::new());
        // "did" is a reserved sub-provider scheme.
        registry
            .register_sub_provider("did", Arc::new(MockSchemeProvider))
            .await
            .unwrap();
        let source = RegistryInspectSource::new(Arc::downgrade(&registry));

        let entries = source.inspect_list().await;
        assert!(
            entries
                .iter()
                .any(|e| e.name == "did" && e.id == "provider:did"),
            "sub-provider scheme must be listed"
        );
        assert!(source.inspect_get("provider:did").await.is_some());
    }

    #[tokio::test]
    async fn catalog_surfaces_content_cid_from_components_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let capsules_dir = tmp.path().join("capsules");
        write_capsule(
            &capsules_dir,
            "probe",
            &serde_json::to_value(probe_manifest()).unwrap(),
        );
        // Seed the installed-capsule catalog with a content CID (provenance).
        std::fs::write(
            tmp.path().join("components.json"),
            serde_json::to_vec(&json!({
                "external": {},
                "profiles": {},
                "capsules": { "probe": { "cid": "bafyprobecid", "sha256": "deadbeef", "size": 0 } }
            }))
            .unwrap(),
        )
        .unwrap();

        let registry = Arc::new(ProviderRegistry::new());
        let provider = InspectProvider::new(Arc::new(CatalogInspectSource::new(
            capsules_dir,
            Arc::downgrade(&registry),
        )));
        let resp = provider
            .send_raw(&json!({ "op": "capsule", "id": "capsule:probe" }))
            .await
            .unwrap();
        assert_eq!(resp["data"]["provenance"]["cid"], "bafyprobecid");
        assert_eq!(resp["data"]["identity"]["cid"], "bafyprobecid");
    }

    #[tokio::test]
    async fn detail_surfaces_provider_authority() {
        // A provider capsule (DDRM-style) expresses its powers via `authority`,
        // not interface methods — the Inspector must surface them.
        let manifest = serde_json::from_value::<CapsuleManifest>(json!({
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "key-provider",
            "role": "provider",
            "type": "microvm",
            "entrypoint": "rootfs.ext4",
            "provides": "elastos://key/*",
            "authority": {
                "reason": "Runtime key-release boundary for protected content",
                "capabilities": [
                    { "resource": "elastos://key/*", "actions": ["read"],
                      "operations": ["status", "release"] }
                ],
                "audit_events": ["key.status", "key.release.denied"]
            },
            "signature": "SECRET_SIGNATURE_MUST_NOT_LEAK"
        }))
        .expect("provider manifest deserializes");

        let entry = InspectEntry {
            id: "capsule:key-provider".to_string(),
            name: "key-provider".to_string(),
            status: "running".to_string(),
            capsule_type: "microvm".to_string(),
            manifest: Some(manifest),
            cid: None,
        };
        let provider = InspectProvider::new(Arc::new(MockSource {
            entries: vec![entry],
        }));
        let resp = provider
            .send_raw(&json!({ "op": "capsule", "id": "capsule:key-provider" }))
            .await
            .unwrap();
        let data = &resp["data"];
        assert_eq!(
            data["authority"]["capabilities"][0]["resource"],
            "elastos://key/*"
        );
        assert_eq!(
            data["authority"]["capabilities"][0]["operations"][1],
            "release"
        );
        assert_eq!(data["authority"]["audit_events"][1], "key.release.denied");
        // #16: the raw signature is still never echoed.
        assert!(!serde_json::to_string(data)
            .unwrap()
            .contains("SECRET_SIGNATURE_MUST_NOT_LEAK"));
    }

    // A DDRM-style provider capsule whose powers live in `authority`, used to
    // exercise the operation-preview leg of inspect/plan.
    fn key_provider_with_release() -> InspectProvider {
        let manifest = serde_json::from_value::<CapsuleManifest>(json!({
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "key-provider",
            "role": "provider",
            "type": "microvm",
            "entrypoint": "rootfs.ext4",
            "provides": "elastos://key/*",
            "authority": {
                "reason": "Runtime key-release boundary for protected content",
                "capabilities": [
                    { "resource": "elastos://key/*", "actions": ["read"],
                      "operations": ["status"] },
                    { "resource": "elastos://key/*", "actions": ["execute"],
                      "operations": ["release"] }
                ],
                "audit_events": ["key.release.denied", "key.release.granted"]
            }
        }))
        .expect("provider manifest deserializes");
        let entry = InspectEntry {
            id: "capsule:key-provider".to_string(),
            name: "key-provider".to_string(),
            status: "running".to_string(),
            capsule_type: "microvm".to_string(),
            manifest: Some(manifest),
            cid: None,
        };
        InspectProvider::new(Arc::new(MockSource {
            entries: vec![entry],
        }))
    }

    #[tokio::test]
    async fn plan_previews_provider_operation_gate() {
        // The agent-safe wedge: preview the exact capability tuple key.release
        // demands, straight from the manifest authority — dispatching nothing.
        let resp = key_provider_with_release()
            .send_raw(&json!({
                "op": "plan", "id": "capsule:key-provider", "operation": "release"
            }))
            .await
            .unwrap();
        assert_eq!(resp["status"], "ok");
        assert_eq!(resp["data"]["valid"], true);
        assert_eq!(resp["data"]["kind"], "operation");
        assert_eq!(resp["data"]["resources"][0], "elastos://key/*");
        assert_eq!(resp["data"]["capability_actions"][0], "execute");
        assert_eq!(resp["data"]["audit_events"][0], "key.release.denied");
    }

    #[tokio::test]
    async fn plan_unknown_operation_reports_invalid() {
        let resp = key_provider_with_release()
            .send_raw(&json!({
                "op": "plan", "id": "capsule:key-provider", "operation": "self_destruct"
            }))
            .await
            .unwrap();
        assert_eq!(resp["status"], "ok");
        assert_eq!(resp["data"]["valid"], false);
        assert_eq!(resp["data"]["error"], "unknown_operation");
    }

    #[tokio::test]
    async fn plan_rejects_mixed_or_empty_selector() {
        // Neither interface/method nor operation → invalid_request (fail-closed).
        let resp = key_provider_with_release()
            .send_raw(&json!({ "op": "plan", "id": "capsule:key-provider" }))
            .await
            .unwrap();
        assert_eq!(resp["status"], "error");
        assert_eq!(resp["code"], "invalid_request");
    }

    // Minimal provider used only to register a scheme in the registry.
    struct MockSchemeProvider;

    #[async_trait]
    impl Provider for MockSchemeProvider {
        async fn handle(&self, _r: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
            Err(ProviderError::Provider("unused".into()))
        }
        fn schemes(&self) -> Vec<&'static str> {
            vec!["wallet"]
        }
        fn name(&self) -> &'static str {
            "mock-wallet"
        }
    }
}
