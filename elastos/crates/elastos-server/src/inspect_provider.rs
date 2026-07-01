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
use elastos_runtime::approval;
use elastos_runtime::inspect::InspectScope;
use elastos_runtime::intent;
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
    /// Verified signer identity (a trusted-key fingerprint) resolved by a real
    /// signature check at launch, when available. `None` until launch-time
    /// verification is wired (loop 3c); the projection reports "verified" trust
    /// and a non-null `signed_by` only when this is `Some`, never from mere
    /// signature presence.
    pub verified_signer: Option<String>,
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
    /// Whether this event is cryptographically attested: its signature VERIFIES (AUD-4)
    /// against the runtime's own DID, not merely that a signature is present. We surface
    /// the *fact* of attestation, never the signature itself (#16).
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

/// A single OBSERVED granted capability (resource + action), derived from
/// recorded audit events — never from the manifest's *requested* capabilities.
/// Safe to display: no bearer token, no signature (#16). Observed / best-effort /
/// unsigned per G8; `granted=false` means an observed grant whose later use was
/// denied, not absence of authority.
#[derive(Debug, Clone)]
pub struct GrantRecord {
    pub resource: String,
    pub action: String,
    pub granted: bool,
}

/// Read-only source of per-capsule audit activity. Optional on the provider;
/// when absent, the audit section is empty.
#[async_trait]
pub trait AuditSource: Send + Sync {
    /// Audit for `capsule_key`, newest-first, capped at `recent_limit`.
    async fn for_capsule(&self, capsule_key: &str, recent_limit: usize) -> CapsuleAudit;

    /// Observed granted capabilities for `capsule_key` (resource + action), derived
    /// from recorded grant/use events. Default empty: a source whose plane lacks
    /// resource/action honestly reports no observed grants rather than fabricating.
    async fn granted_for_capsule(
        &self,
        _capsule_key: &str,
        _recent_limit: usize,
    ) -> Vec<GrantRecord> {
        Vec::new()
    }

    /// A LIVE full-chain integrity attestation of the audit plane backing this source, when it is
    /// file-backed (durable mode). Default `None`: a source without a verifiable durable chain
    /// honestly reports nothing rather than fabricating an "ok".
    async fn chain_attestation(
        &self,
    ) -> Option<elastos_runtime::primitives::audit::ChainAttestation> {
        None
    }

    /// The per-capsule intent-proof tally (denied / diverged / undelivered) for `capsule_key`, when
    /// this source's plane records intent activity. PRESENCE-aware: `None` is ABSENT — the capsule
    /// never went through the intent gate, which is NOT the same as "clean" (all-zero); the panel
    /// renders absence as absence. Default `None`: a source without intent-proof custody honestly
    /// reports nothing rather than fabricating a zeroed pass.
    async fn intent_proof_summary(
        &self,
        _capsule_key: &str,
    ) -> Option<elastos_runtime::capability::intent::IntentProofSummary> {
        None
    }
}

/// Composes signed activity (one source) with observed grants (another) so the
/// inspector shows BOTH a capsule's attested auth/session activity AND its observed
/// granted capabilities (G1b-LIVE). `for_capsule` delegates to the activity source;
/// `granted_for_capsule` delegates to the grants source, both keyed by the same
/// capsule id. Neither plane's data is fabricated or merged -- each answer comes
/// from the source that owns it.
pub struct CompositeAuditSource {
    activity: Arc<dyn AuditSource>,
    grants: Arc<dyn AuditSource>,
}

impl CompositeAuditSource {
    pub fn new(activity: Arc<dyn AuditSource>, grants: Arc<dyn AuditSource>) -> Self {
        Self { activity, grants }
    }
}

#[async_trait]
impl AuditSource for CompositeAuditSource {
    async fn for_capsule(&self, capsule_key: &str, recent_limit: usize) -> CapsuleAudit {
        self.activity.for_capsule(capsule_key, recent_limit).await
    }

    async fn granted_for_capsule(
        &self,
        capsule_key: &str,
        recent_limit: usize,
    ) -> Vec<GrantRecord> {
        self.grants
            .granted_for_capsule(capsule_key, recent_limit)
            .await
    }

    // The grants source owns the runtime AuditLog (the plane with the hash chain), so the
    // chain attestation comes from there — not the auth-state activity source.
    async fn chain_attestation(
        &self,
    ) -> Option<elastos_runtime::primitives::audit::ChainAttestation> {
        self.grants.chain_attestation().await
    }

    // Intent-proof events land on the runtime AuditLog too, so the tally comes from the
    // same grants source that owns it — not the auth-state activity source.
    async fn intent_proof_summary(
        &self,
        capsule_key: &str,
    ) -> Option<elastos_runtime::capability::intent::IntentProofSummary> {
        self.grants.intent_proof_summary(capsule_key).await
    }
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

/// AUD-4 (verify-on-read): is this audit event cryptographically ATTESTED? True ONLY
/// when it carries a signature, its `signer_did` is the runtime's OWN DID (the
/// load-bearing anti-spoof pin: a file-rewriting attacker who re-signs with their own
/// key and sets a matching `signer_did` is rejected here), AND the ed25519 signature
/// verifies over the canonical signed bytes — the exact contract of `sign_audit_event`
/// (`signer_did` set, `signature` omitted via `skip_serializing_if`). A present-but-
/// unverifiable, wrong-signer, or absent signature returns false (not attested), so the
/// inspector never claims attestation it cannot prove.
fn audit_event_is_attested(
    event: &elastos_runtime::auth::RuntimeAuditEventV1,
    expected_did: &str,
    vk: &ed25519_dalek::VerifyingKey,
) -> bool {
    let Some(sig_hex) = event.signature.as_deref() else {
        return false;
    };
    if event.signer_did.as_deref() != Some(expected_did) {
        return false;
    }
    let mut probe = event.clone();
    probe.signature = None;
    let Ok(bytes) = serde_json::to_vec(&probe) else {
        return false;
    };
    crate::crypto::domain_separated_verify(vk, crate::auth::AUDIT_EVENT_DOMAIN, &bytes, sig_hex)
        .is_ok()
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
            // AUD-4: load the runtime's OWN DID + verifying key ONCE so each event's
            // signature can be VERIFIED on read (not merely presence-checked). If the DID
            // cannot be loaded, degrade CLOSED: claim no attestation rather than fake it.
            let expected = elastos_identity::load_or_create_did(&data_dir)
                .ok()
                .and_then(|(_sk, did)| {
                    crate::crypto::decode_did_key(&did).ok().map(|vk| (did, vk))
                });
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
                // AUD-4: attested == cryptographically VERIFIED (+ anti-spoof DID pin),
                // never signature presence. A forged / wrong-signer / unverifiable
                // signature is NOT attested.
                let signed = match &expected {
                    Some((did, vk)) => audit_event_is_attested(event, did, vk),
                    None => false,
                };
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

/// Audit source backed by the in-memory runtime [`AuditLog`] — the only plane that
/// records resource + action with each capability grant/use. Surfaces OBSERVED
/// granted capabilities by folding recorded `capability_grant` / `capability_use`
/// events: the exact fold the runtime-side inspector uses, so the two agree.
/// Observed / best-effort / unsigned per G8; never the manifest's requested caps.
pub struct RuntimeAuditLogGrantSource {
    audit_log: std::sync::Arc<elastos_runtime::primitives::audit::AuditLog>,
}

impl RuntimeAuditLogGrantSource {
    pub fn new(audit_log: std::sync::Arc<elastos_runtime::primitives::audit::AuditLog>) -> Self {
        Self { audit_log }
    }
}

#[async_trait]
impl AuditSource for RuntimeAuditLogGrantSource {
    // Grant-focused: the signed activity list comes from the auth plane; this
    // source reports no activity (honest empty) and only observed grants.
    async fn for_capsule(&self, _capsule_key: &str, _recent_limit: usize) -> CapsuleAudit {
        CapsuleAudit::default()
    }

    // This source owns the runtime AuditLog, so it can run the full-chain walk live.
    async fn chain_attestation(
        &self,
    ) -> Option<elastos_runtime::primitives::audit::ChainAttestation> {
        self.audit_log.chain_attestation()
    }

    // The AuditLog owns the intent-proof events, so the presence-aware per-capsule tally
    // (keyed on the acting vm-{name}) comes straight from it — None ⇒ absent, never faked.
    async fn intent_proof_summary(
        &self,
        capsule_key: &str,
    ) -> Option<elastos_runtime::capability::intent::IntentProofSummary> {
        self.audit_log.intent_proof_summary(capsule_key)
    }

    async fn granted_for_capsule(
        &self,
        capsule_key: &str,
        recent_limit: usize,
    ) -> Vec<GrantRecord> {
        // Mirror of the runtime-side fold (handler/request_handler.rs): fold the
        // recorded capability_grant/use events into one entry per (resource,
        // action), flipping granted=false when an observed use was denied.
        let events = self.audit_log.recent_events(recent_limit);
        let mut grants: std::collections::BTreeMap<String, bool> =
            std::collections::BTreeMap::new();
        for ev in &events {
            let Ok(v) = serde_json::to_value(ev) else {
                continue;
            };
            if v.get("capsule_id").and_then(|c| c.as_str()) != Some(capsule_key) {
                continue;
            }
            let etype = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
            let Some(resource) = v.get("resource").and_then(|r| r.as_str()) else {
                continue;
            };
            let action = v.get("action").and_then(|a| a.as_str()).unwrap_or("");
            let success = v.get("success").and_then(|s| s.as_bool()).unwrap_or(true);
            let entry = grants.entry(format!("{resource} {action}")).or_insert(true);
            if etype == "capability_use" && !success {
                *entry = false;
            }
        }
        grants
            .into_iter()
            .map(|(key, granted)| {
                let mut parts = key.splitn(2, ' ');
                GrantRecord {
                    resource: parts.next().unwrap_or("").to_string(),
                    action: parts.next().unwrap_or("").to_string(),
                    granted,
                }
            })
            .collect()
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
        // G2b: the launch-time verified-signer is threaded straight through — `Some` only
        // when a real trusted-key ed25519 check matched at launch, so a running capsule
        // reads "verified" trust strictly behind a genuine signature check.
        verified_signer: info.verified_signer,
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
            verified_signer: None,
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
            verified_signer: None,
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
    spend_meter: Option<Arc<elastos_runtime::primitives::spend::SpendMeter>>,
}

impl InspectProvider {
    pub fn new(source: Arc<dyn InspectSource>) -> Self {
        Self {
            source,
            audit: None,
            spend_meter: None,
        }
    }

    /// Attach a per-capsule audit source so detail views show live activity.
    pub fn with_audit(mut self, audit: Arc<dyn AuditSource>) -> Self {
        self.audit = Some(audit);
        self
    }

    /// Attach the shared spend meter so a capsule's detail view projects its live budget
    /// (read-only — the inspector reflects the meter, never edits it). `None` ⇒ no `spend_budget`.
    pub fn with_spend_meter(
        mut self,
        spend_meter: Option<Arc<elastos_runtime::primitives::spend::SpendMeter>>,
    ) -> Self {
        self.spend_meter = spend_meter;
        self
    }

    /// Read-only projection of a capsule's budget (keyed on the canonical `vm-{name}`, the same key
    /// the meter is debited under). `Null` when no meter is attached or the capsule is unprovisioned.
    fn spend_budget_value(&self, entry: &InspectEntry) -> Value {
        self.spend_meter
            .as_ref()
            .and_then(|m| m.snapshot(&format!("vm-{}", entry.name)))
            .and_then(|snap| serde_json::to_value(snap).ok())
            .unwrap_or(Value::Null)
    }

    /// Read-only projection of a capsule's intent-proof custody tally (keyed on the acting
    /// `vm-{name}`, the identity intent events record under). PRESENCE-aware: `Null` when there is
    /// no audit source OR the capsule has no buffered intent activity (ABSENT — it never went
    /// through the gate, which is NOT "clean"). A present tally projects `{denied, diverged,
    /// undelivered}` — the exact `IntentProofSummaryV1` shape the ESP `homeCustodyView` consumes.
    async fn intent_proof_value(&self, entry: &InspectEntry) -> Value {
        let Some(audit) = &self.audit else {
            return Value::Null;
        };
        match audit
            .intent_proof_summary(&format!("vm-{}", entry.name))
            .await
        {
            Some(s) => json!({
                "denied": s.denied,
                "diverged": s.diverged,
                "undelivered": s.undelivered,
            }),
            None => Value::Null,
        }
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
    fn project(
        entry: &InspectEntry,
        audit: Value,
        granted: Value,
        spend_budget: Value,
        intent_proof: Value,
    ) -> Value {
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
        // Verified signer (G2): present ONLY when a real signature check resolved
        // one (threaded onto the entry at launch). Never fabricated from the
        // self-asserted author or from mere signature presence.
        let verified_signer = entry.verified_signer.clone();
        let signed_by = verified_signer
            .clone()
            .map(Value::from)
            .unwrap_or(Value::Null);
        // Trust reaches the "verified" tier only when a signer was genuinely
        // verified; otherwise the presence-based classifier applies.
        let trust = if verified_signer.is_some() {
            "verified".to_string()
        } else {
            trust_level(signature_present, cid.is_some()).to_string()
        };
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
                "trust_level": trust.clone(),
                "signature_present": signature_present,
                // A verified signer appears only when a real ed25519 check resolved
                // one (G2); honest null otherwise, never the self-asserted author.
                "signed_by": signed_by.clone(),
            },
            "manifest": {
                "schema": field(&manifest, "schema"),
                "entrypoint": field(&manifest, "entrypoint"),
            },
            "affordances": affordances,
            // Provider powers (declarative authority), for provider capsules.
            "authority": authority,
            "required_capabilities": field(&manifest, "capabilities"),
            // OBSERVED granted capabilities from the runtime audit plane
            // (capability_grant folded with capability_use failures); best-effort
            // + unsigned per G8, and NEVER the manifest's requested capabilities
            // (those stay in required_capabilities above). Empty when unobserved.
            "granted_capabilities": granted,
            // Live per-capsule spend budget {limit, spent, remaining}, keyed on vm-{name} — a
            // read-only projection of the meter the act paths debit. Null when unmetered/unprovisioned.
            "spend_budget": spend_budget,
            // Live per-capsule intent-proof tally {denied, diverged, undelivered}, keyed on vm-{name} —
            // the prover/verifier custody channel. Null when ABSENT (no intent activity through the
            // gate); present-and-all-zero is CLEAN, any non-zero is a flagged custody alarm.
            "intent_proof": intent_proof,
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
                "signed_by": signed_by,
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
        // LIVE full-chain integrity (not just per-event signatures, not just at startup): the
        // whole hash+signature walk, projected when the audit plane is file-backed. Null otherwise
        // (memory-only ⇒ no durable chain to attest); never a fabricated ok.
        let chain = audit
            .chain_attestation()
            .await
            .and_then(|att| serde_json::to_value(att).ok())
            .unwrap_or(Value::Null);
        json!({
            "counts": { "total": a.total, "denied": a.denied, "attested": a.attested },
            "chain": chain,
            "recent": recent,
        })
    }

    /// Observed granted capabilities for display: empty when no source is attached
    /// or none were observed. Never the manifest's requested capabilities.
    async fn granted_value(&self, entry: &InspectEntry) -> Value {
        let Some(audit) = &self.audit else {
            return Value::Array(vec![]);
        };
        // G-ID flip: grants are recorded under the canonical "vm-{name}" (the mint
        // keys the audit event on the token's capsule id), so look them up there.
        let grants = audit
            .granted_for_capsule(&format!("vm-{}", entry.name), 500)
            .await;
        Value::Array(
            grants
                .into_iter()
                .map(
                    |g| json!({ "resource": g.resource, "action": g.action, "granted": g.granted }),
                )
                .collect(),
        )
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
            // System-scope GLOBAL custody-chain attestation: a LIVE full-chain `verify_chain` walk
            // of the audit plane, for an exporter to embed in the W7 EU-AI-Act artifact so the
            // exported evidence is self-verifying (the chain is global, not per-capsule, so this is
            // its own op rather than riding an arbitrary capsule's detail). `null` when the plane is
            // memory-only (nothing durable to attest) or no audit source is attached — never faked.
            "audit_attestation" => {
                let chain = match &self.audit {
                    Some(a) => a
                        .chain_attestation()
                        .await
                        .and_then(|att| serde_json::to_value(att).ok())
                        .unwrap_or(Value::Null),
                    None => Value::Null,
                };
                json!({
                    "status": "ok",
                    "data": { "scope": Self::scope_label(InspectScope::System), "chain": chain },
                })
            }
            // System-scope detail.
            "capsule" => match request.get("id").and_then(Value::as_str) {
                Some(id) => match self.source.inspect_get(id).await {
                    Some(entry) => {
                        let audit = self.audit_value(&entry).await;
                        let granted = self.granted_value(&entry).await;
                        let spend_budget = self.spend_budget_value(&entry);
                        let intent_proof = self.intent_proof_value(&entry).await;
                        json!({ "status": "ok", "data": Self::project(&entry, audit, granted, spend_budget, intent_proof) })
                    }
                    None => provider_error("not_found", "no such capsule"),
                },
                None => provider_error("invalid_request", "inspect/capsule requires an \"id\""),
            },
            // Self-scope detail: a SelfOnly caller inspects ONLY its own record.
            // The target is ALWAYS the authenticated principal injected by the
            // gateway (principal_id), NEVER a client-supplied `id` (Principle 16).
            // Routed through the canonical runtime gate (inspect::authorize_view /
            // InspectScope::SelfOnly: caller == target), fail-closed otherwise.
            "self" => {
                let Some(caller) = request
                    .get("principal_id")
                    .and_then(Value::as_str)
                    .filter(|p| !p.is_empty())
                else {
                    return provider_error("out_of_scope", "missing authenticated principal");
                };
                // SelfOnly: the target is the caller's own id; request["id"] is ignored.
                let granted = [elastos_runtime::inspect::INSPECT_SELF.to_string()];
                if !elastos_runtime::inspect::authorize_view(false, caller, caller, &granted) {
                    return provider_error("out_of_scope", "caller may not inspect this capsule");
                }
                match self.source.inspect_get(caller).await {
                    Some(entry) => {
                        let audit = self.audit_value(&entry).await;
                        let granted_v = self.granted_value(&entry).await;
                        let spend_budget = self.spend_budget_value(&entry);
                        let intent_proof = self.intent_proof_value(&entry).await;
                        json!({ "status": "ok", "data": Self::project(&entry, audit, granted_v, spend_budget, intent_proof) })
                    }
                    None => provider_error("not_found", "no such capsule"),
                }
            }
            // Metadata-driven invocation *preview* (read-only, dry-run): given a
            // capsule + interface + method + args, validate the args against the
            // affordance's input_schema and derive the capability/approval/audit
            // gate the call would require. Dispatches NO effect — this is the
            // reflective half of the CAR invoke kernel.
            "plan" => self.handle_plan(request).await,
            // Approval-intent preview (read-only): given a provider operation,
            // derive the gate (via plan) and the approval it would require, and
            // show the fail-closed default decision. Records nothing, dispatches
            // nothing — the "approve" step of the loop, in preview form.
            "intent" => self.handle_intent(request).await,
            // Cross-capsule discovery (read-only, System scope): resolve WHICH capsule
            // in the whole installed set offers a goal. Gated to SYSTEM_CAPSULE_ID at
            // the gateway allow-list; carrier-locked to Admin (deliberately absent from
            // inspect_op_required_action), so a routine inspect/* Read token cannot
            // reach the cross-capsule capability map. Dispatches nothing.
            "discover" => self.handle_discover(request).await,
            other => provider_error("unknown_op", &format!("unknown inspect op: {other}")),
        }
    }

    async fn handle_intent(&self, request: &Value) -> Value {
        let (id, operation) = match (
            request.get("id").and_then(Value::as_str),
            request.get("operation").and_then(Value::as_str),
        ) {
            (Some(id), Some(op)) => (id, op),
            _ => {
                return provider_error(
                    "invalid_request",
                    "inspect/intent requires \"id\" and \"operation\"",
                )
            }
        };
        let entry = match self.source.inspect_get(id).await {
            Some(entry) => entry,
            None => return provider_error("not_found", "no such capsule"),
        };
        let authority = match entry.manifest.as_ref().and_then(|m| m.authority.as_ref()) {
            Some(a) => a,
            None => {
                return provider_error(
                    "invalid_request",
                    "capsule declares no provider authority to plan against",
                )
            }
        };
        match invoke::plan_provider_operation(authority, operation) {
            Ok(plan) => {
                // Derive the approval this gate requires, then the fail-closed
                // default decision (no approver yet → never auto-approve a
                // write/execute/admin op).
                let mode = approval::required_approval(&plan.actions);
                let default = approval::decide(&mode, None);
                json!({
                    "status": "ok",
                    "data": {
                        "valid": true,
                        "kind": "approval_intent",
                        "capsule": id,
                        "operation": operation,
                        "resources": plan.resources,
                        "capability_actions": plan
                            .actions
                            .iter()
                            .map(|a| a.to_string())
                            .collect::<Vec<_>>(),
                        "requires_approval": serde_json::to_value(&mode).ok(),
                        "default_decision": serde_json::to_value(&default).ok(),
                        "audit_events": plan.audit_events,
                    }
                })
            }
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

    /// Cross-capsule discovery (read-only, System scope): given a StructuredIntent,
    /// resolve WHICH installed capsule offers that goal across the whole manifest set,
    /// returning the runtime's CompiledPlan (exactly one match) or IntentError (0 =>
    /// Unresolvable, >1 => Ambiguous) verbatim as JSON. Unlike `intent`/`plan`, the
    /// caller supplies NO capsule id -- the runtime makes the cross-capsule resolution
    /// and Ambiguous decision the single-id ops structurally cannot. Dispatches
    /// nothing, mutates nothing, writes no audit (a pure preview, like `plan`).
    async fn handle_discover(&self, request: &Value) -> Value {
        let intent: intent::StructuredIntent = match request.get("intent").cloned() {
            Some(value) => match serde_json::from_value(value) {
                Ok(parsed) => parsed,
                Err(_) => return provider_error(
                    "invalid_request",
                    "inspect/discover requires an \"intent\" object {operation, resource?, args?}",
                ),
            },
            None => {
                return provider_error(
                    "invalid_request",
                    "inspect/discover requires an \"intent\" object",
                )
            }
        };
        // The manifest SET comes from the source; capsules that did not retain a
        // manifest are simply not discoverable (filtered out, never erroring the op).
        // `manifests` owns the values; `refs` borrows it -- both must live to the end
        // of the handler, so do NOT inline the two steps into a borrow error.
        let manifests: Vec<CapsuleManifest> = self
            .source
            .inspect_list()
            .await
            .into_iter()
            .filter_map(|entry| entry.manifest)
            .collect();
        let refs: Vec<&CapsuleManifest> = manifests.iter().collect();
        match intent::discover(&refs, &intent) {
            Ok(plan) => json!({ "status": "ok", "data": serde_json::to_value(&plan).ok() }),
            // Unresolvable/Ambiguous are NORMAL fail-closed ANSWERS (the query
            // succeeded; the goal does not resolve across the set), NOT transport
            // errors -- mirroring handle_intent's valid:false precedent. The
            // IntentError is serialized whole; the handler re-derives no resolution.
            Err(err) => json!({
                "status": "ok",
                "data": { "valid": false, "error": serde_json::to_value(&err).ok() }
            }),
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
            verified_signer: None,
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
    async fn capsule_detail_projects_live_spend_budget() {
        use elastos_runtime::primitives::spend::SpendMeter;

        // A capsule's budget is keyed on the canonical vm-{name}; the detail view reflects it.
        let meter = Arc::new(SpendMeter::new());
        meter.set_budget("vm-probe", 100);
        meter.try_debit("vm-probe", 30).unwrap();

        let provider = InspectProvider::new(Arc::new(MockSource {
            entries: vec![probe_entry()],
        }))
        .with_spend_meter(Some(meter.clone()));
        let resp = provider
            .send_raw(&json!({ "op": "capsule", "id": "cap_probe_1" }))
            .await
            .unwrap();
        assert_eq!(resp["status"], "ok");
        assert_eq!(resp["data"]["spend_budget"]["limit"], 100);
        assert_eq!(resp["data"]["spend_budget"]["spent"], 30);
        assert_eq!(
            resp["data"]["spend_budget"]["remaining"], 70,
            "the inspector projects the live remaining budget: {resp}"
        );

        // No meter attached ⇒ null, never fabricated (honest projection).
        let bare = provider_with_probe()
            .send_raw(&json!({ "op": "capsule", "id": "cap_probe_1" }))
            .await
            .unwrap();
        assert!(
            bare["data"]["spend_budget"].is_null(),
            "an unmetered inspector must project null, not a fabricated budget"
        );
    }

    #[tokio::test]
    async fn audit_attestation_op_returns_live_global_chain_for_export() {
        use elastos_runtime::primitives::audit::AuditLog;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");
        let log = Arc::new(AuditLog::with_file(&path).unwrap());
        log.runtime_start("1.0.0");
        log.runtime_stop();

        let grants: Arc<dyn AuditSource> = Arc::new(RuntimeAuditLogGrantSource::new(log.clone()));
        let provider = InspectProvider::new(Arc::new(MockSource {
            entries: vec![probe_entry()],
        }))
        .with_audit(Arc::new(CompositeAuditSource::new(grants.clone(), grants)));

        // The dedicated export read path: a clean file-backed plane attests verified.
        let resp = provider
            .send_raw(&json!({ "op": "audit_attestation" }))
            .await
            .unwrap();
        assert_eq!(resp["status"], "ok");
        assert_eq!(resp["data"]["chain"]["verified"], true);
        assert_eq!(resp["data"]["chain"]["records"], 2);
        assert!(resp["data"]["chain"]["signer"].is_string());

        // After an on-disk tamper the SAME live read path reports verified=false.
        let content = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, content.replacen("1.0.0", "9.9.9", 1)).unwrap();
        let after = provider
            .send_raw(&json!({ "op": "audit_attestation" }))
            .await
            .unwrap();
        assert_eq!(after["data"]["chain"]["verified"], false, "{after}");

        // No audit source / memory-only ⇒ null (never a fabricated ok).
        let bare = provider_with_probe()
            .send_raw(&json!({ "op": "audit_attestation" }))
            .await
            .unwrap();
        assert!(bare["data"]["chain"].is_null());
    }

    #[tokio::test]
    async fn capsule_detail_projects_live_chain_attestation() {
        use elastos_runtime::primitives::audit::AuditLog;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");
        let log = Arc::new(AuditLog::with_file(&path).unwrap());
        log.runtime_start("1.0.0");
        log.runtime_stop();

        // The grants source owns the file-backed AuditLog; the composite delegates the chain walk.
        let grants: Arc<dyn AuditSource> = Arc::new(RuntimeAuditLogGrantSource::new(log.clone()));
        let composite = Arc::new(CompositeAuditSource::new(grants.clone(), grants));
        let provider = InspectProvider::new(Arc::new(MockSource {
            entries: vec![probe_entry()],
        }))
        .with_audit(composite);

        // Clean chain: a LIVE inspect projects verified=true (verify-on-read beyond startup).
        let resp = provider
            .send_raw(&json!({ "op": "capsule", "id": "cap_probe_1" }))
            .await
            .unwrap();
        assert_eq!(resp["data"]["audit"]["chain"]["verified"], true);
        assert_eq!(resp["data"]["audit"]["chain"]["records"], 2);

        // Tamper the on-disk chain; the NEXT live inspect catches it (not just startup).
        let content = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, content.replacen("1.0.0", "9.9.9", 1)).unwrap();
        let after = provider
            .send_raw(&json!({ "op": "capsule", "id": "cap_probe_1" }))
            .await
            .unwrap();
        assert_eq!(
            after["data"]["audit"]["chain"]["verified"], false,
            "a tampered chain must project verified=false live: {after}"
        );
        assert!(after["data"]["audit"]["chain"]["error"]
            .as_str()
            .unwrap_or_default()
            .contains("tamper"));

        // A memory-only inspector projects null chain (no durable chain to attest, never faked ok).
        let mem = provider_with_probe()
            .send_raw(&json!({ "op": "capsule", "id": "cap_probe_1" }))
            .await
            .unwrap();
        assert!(mem["data"]["audit"]["chain"].is_null());
    }

    fn provider_entry(id: &str, name: &str, resource: &str, ops: &[&str]) -> InspectEntry {
        let manifest: CapsuleManifest = serde_json::from_value(json!({
            "schema": "elastos.capsule/v1", "version": "0.1.0", "name": name,
            "role": "app", "type": "wasm", "entrypoint": "x.wasm",
            "authority": {
                "reason": "test provider authority",
                "capabilities": [
                    { "resource": resource, "operations": ops, "actions": ["read"] }
                ],
                "audit_events": []
            }
        }))
        .expect("provider manifest deserializes");
        InspectEntry {
            id: id.to_string(),
            name: name.to_string(),
            status: "running".to_string(),
            capsule_type: "wasm".to_string(),
            manifest: Some(manifest),
            cid: None,
            verified_signer: None,
        }
    }

    // A set mirroring the shipped providers: key-provider offers [status, release],
    // rights-provider offers [status, ...] -- so "status" collides across capsules.
    fn discover_provider() -> InspectProvider {
        InspectProvider::new(Arc::new(MockSource {
            entries: vec![
                provider_entry(
                    "cap_key_1",
                    "key-provider",
                    "elastos://key/*",
                    &["status", "release"],
                ),
                provider_entry(
                    "cap_rights_1",
                    "rights-provider",
                    "elastos://rights/*",
                    &["status", "has_access_by_content_id"],
                ),
            ],
        }))
    }

    #[tokio::test]
    async fn discover_resolves_a_unique_op_to_its_capsule() {
        // "release" is offered only by key-provider across the set: discover finds it
        // (the caller named no capsule) and returns the planned step.
        let resp = discover_provider()
            .send_raw(&json!({ "op": "discover", "intent": { "operation": "release" } }))
            .await
            .unwrap();
        assert_eq!(resp["status"], "ok");
        assert_eq!(resp["data"]["composition"], "single_step");
        assert_eq!(resp["data"]["steps"][0]["kind"], "operation");
        assert_eq!(resp["data"]["steps"][0]["capsule"], "key-provider");
        assert_eq!(resp["data"]["steps"][0]["operation"], "release");
    }

    #[tokio::test]
    async fn discover_reports_cross_capsule_ambiguity_fail_closed() {
        // "status" is offered by BOTH providers: discover never guesses -- it returns a
        // fail-closed valid:false Ambiguous answer, the decision a single-id op cannot
        // make.
        let resp = discover_provider()
            .send_raw(&json!({ "op": "discover", "intent": { "operation": "status" } }))
            .await
            .unwrap();
        assert_eq!(resp["status"], "ok");
        assert_eq!(resp["data"]["valid"], false);
        assert_eq!(resp["data"]["error"]["ambiguous"]["operation"], "status");
        assert_eq!(resp["data"]["error"]["ambiguous"]["matches"], 2);
    }

    #[tokio::test]
    async fn discover_reports_unresolvable_for_absent_op() {
        let resp = discover_provider()
            .send_raw(&json!({ "op": "discover", "intent": { "operation": "obliterate" } }))
            .await
            .unwrap();
        assert_eq!(resp["status"], "ok");
        assert_eq!(resp["data"]["valid"], false);
        assert_eq!(
            resp["data"]["error"]["unresolvable"]["operation"],
            "obliterate"
        );
    }

    #[tokio::test]
    async fn discover_rejects_a_malformed_intent() {
        // A missing "intent" object is a transport error (invalid_request), distinct
        // from a well-formed query whose goal merely does not resolve.
        let resp = discover_provider()
            .send_raw(&json!({ "op": "discover" }))
            .await
            .unwrap();
        assert_eq!(resp["status"], "error");
        assert_eq!(resp["code"], "invalid_request");
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

    // ── Flint foundation: the SHIPPED inspector manifest is a typed tool ──
    //
    // The wedge: 0 of the shipped capsules used to declare `interfaces[]`, so an
    // agent inspecting any real capsule saw `affordances: []`. These tests drive
    // the public op path (reflect -> plan) over the on-disk capsule-inspector
    // manifest, proving the perceive->plan machinery fires on a real capsule.

    fn shipped_inspector_manifest() -> CapsuleManifest {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../capsules/capsule-inspector/capsule.json"
        );
        let data = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read shipped capsule-inspector manifest at {path}: {e}"));
        let manifest: CapsuleManifest =
            serde_json::from_str(&data).expect("shipped capsule-inspector manifest parses");
        manifest
            .validate()
            .expect("shipped capsule-inspector manifest validates");
        manifest
    }

    fn provider_over(manifest: CapsuleManifest, id: &str) -> InspectProvider {
        InspectProvider::new(Arc::new(MockSource {
            entries: vec![InspectEntry {
                id: id.to_string(),
                name: manifest.name.clone(),
                status: "running".to_string(),
                capsule_type: "wasm".to_string(),
                manifest: Some(manifest),
                cid: None,
                verified_signer: None,
            }],
        }))
    }

    #[tokio::test]
    async fn shipped_inspector_surfaces_discoverable_affordances() {
        let manifest = shipped_inspector_manifest();
        assert!(
            !manifest.interfaces.is_empty(),
            "capsule-inspector must declare interfaces[]"
        );

        // EYES: the projection surfaces the declared methods as affordances, so
        // an agent sees a non-empty tool surface instead of affordances:[].
        let resp = provider_over(manifest, "cap_inspector_1")
            .send_raw(&json!({ "op": "capsule", "id": "cap_inspector_1" }))
            .await
            .unwrap();
        assert_eq!(resp["status"], "ok");
        let affordances = resp["data"]["affordances"]
            .as_array()
            .expect("affordances array");
        assert!(
            !affordances.is_empty(),
            "agent must see a non-empty affordance surface, not affordances:[]"
        );
        assert!(
            affordances.iter().any(|a| a["id"] == "capsule.view"),
            "the inspector must expose a capsule.view affordance"
        );
    }

    #[tokio::test]
    async fn shipped_inspector_affordance_derives_gate_fail_closed() {
        let provider = provider_over(shipped_inspector_manifest(), "cap_inspector_1");

        // HANDS: a declared read affordance derives the Read capability gate purely
        // from metadata, before any dispatch.
        let ok = provider
            .send_raw(&json!({
                "op": "plan",
                "id": "cap_inspector_1",
                "interface": "elastos.inspect",
                "method": "capsule.view",
                "args": { "target": "elastos://inspect/capsule-inspector" }
            }))
            .await
            .unwrap();
        assert_eq!(ok["status"], "ok");
        assert_eq!(ok["data"]["valid"], true);
        assert_eq!(ok["data"]["capability_action"], "read");

        // Fail-closed: the same call with the required `target` missing is rejected
        // by the declared input_schema, never planned through.
        let bad = provider
            .send_raw(&json!({
                "op": "plan",
                "id": "cap_inspector_1",
                "interface": "elastos.inspect",
                "method": "capsule.view",
                "args": {}
            }))
            .await
            .unwrap();
        assert_eq!(bad["status"], "ok");
        assert_eq!(bad["data"]["valid"], false);
        assert_eq!(bad["data"]["error"], "missing_required_field");
        assert_eq!(bad["data"]["field"], "target");
    }

    #[tokio::test]
    async fn registered_capsules_surface_their_affordances_via_op_path() {
        // EYES at scale: every capsule in the Flint gap registry must, through the
        // real inspect op path, surface exactly its declared affordances to an
        // agent (count matches, each with a known risk class). Keep this list in
        // step with the elastos-common gap registry.
        const REGISTRY: &[&str] = &[
            "capsule-inspector",
            "documents",
            "library",
            "inbox",
            "chat",
            "archive-manager",
            "gba-emulator",
            "system",
            "chat-room",
            "chat-wasm",
        ];
        for capsule in REGISTRY {
            let path = format!(
                "{}/../../../capsules/{}/capsule.json",
                env!("CARGO_MANIFEST_DIR"),
                capsule
            );
            let data = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {capsule} manifest at {path}: {e}"));
            let manifest: CapsuleManifest =
                serde_json::from_str(&data).unwrap_or_else(|e| panic!("{capsule} parses: {e}"));
            manifest
                .validate()
                .unwrap_or_else(|e| panic!("{capsule} validates: {e}"));
            let declared: usize = manifest.interfaces.iter().map(|i| i.methods.len()).sum();

            let id = format!("cap_{capsule}");
            let resp = provider_over(manifest, &id)
                .send_raw(&json!({ "op": "capsule", "id": id }))
                .await
                .unwrap();
            assert_eq!(resp["status"], "ok", "{capsule}: inspect op failed");
            let affordances = resp["data"]["affordances"]
                .as_array()
                .unwrap_or_else(|| panic!("{capsule}: affordances must be an array"));
            assert_eq!(
                affordances.len(),
                declared,
                "{capsule}: projected affordances must match declared methods"
            );
            for a in affordances {
                assert!(
                    a["id"].is_string(),
                    "{capsule}: affordance id must be a string"
                );
                let risk = a["risk"].as_str().unwrap_or("");
                assert!(
                    matches!(
                        risk,
                        "read"
                            | "write"
                            | "launch"
                            | "payment"
                            | "rights"
                            | "actuator"
                            | "privileged"
                    ),
                    "{capsule}: affordance must carry a known risk class, got {risk:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn registered_providers_reflect_their_authority_via_op_path() {
        // EYES for providers: a provider exposes its tool surface through
        // authority.capabilities[] (the twin of an app's interfaces[]). Each
        // dDRM-critical provider must, through the real inspect plan op, reflect
        // the resource an operation touches and the full capability-action set the
        // manifest declares (fail-closed union, never under-stated).
        struct Expect {
            capsule: &'static str,
            operation: &'static str,
            resource: &'static str,
            actions: &'static [&'static str],
        }
        const PROVIDERS: &[Expect] = &[
            Expect {
                capsule: "key-provider",
                operation: "release",
                resource: "elastos://key/*",
                actions: &["read"],
            },
            Expect {
                capsule: "rights-provider",
                operation: "has_access_by_content_id",
                resource: "elastos://rights/*",
                actions: &["read"],
            },
            Expect {
                capsule: "chain-provider",
                operation: "broadcast_transaction",
                resource: "elastos://chain/*",
                actions: &["read", "write", "admin"],
            },
        ];
        for p in PROVIDERS {
            let path = format!(
                "{}/../../../capsules/{}/capsule.json",
                env!("CARGO_MANIFEST_DIR"),
                p.capsule
            );
            let data = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {} manifest at {path}: {e}", p.capsule));
            let manifest: CapsuleManifest =
                serde_json::from_str(&data).unwrap_or_else(|e| panic!("{} parses: {e}", p.capsule));
            manifest
                .validate()
                .unwrap_or_else(|e| panic!("{} validates: {e}", p.capsule));

            let id = format!("cap_{}", p.capsule);
            let resp = provider_over(manifest, &id)
                .send_raw(&json!({ "op": "plan", "id": id, "operation": p.operation }))
                .await
                .unwrap();
            assert_eq!(resp["status"], "ok", "{}: plan op failed", p.capsule);
            assert_eq!(
                resp["data"]["valid"], true,
                "{}: plan should be valid",
                p.capsule
            );
            assert_eq!(
                resp["data"]["kind"], "operation",
                "{}: should reflect a provider operation",
                p.capsule
            );
            let resources = resp["data"]["resources"]
                .as_array()
                .expect("resources array");
            assert!(
                resources.iter().any(|r| r == p.resource),
                "{}: must reflect resource {} (got {:?})",
                p.capsule,
                p.resource,
                resources
            );
            let got: Vec<&str> = resp["data"]["capability_actions"]
                .as_array()
                .expect("capability_actions array")
                .iter()
                .filter_map(|a| a.as_str())
                .collect();
            for a in p.actions {
                assert!(
                    got.contains(a),
                    "{}: must reflect capability action {} (got {:?})",
                    p.capsule,
                    a,
                    got
                );
            }
        }
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
    async fn ratchet_granted_capabilities_populated() {
        // G1 (loop 4): granted_capabilities lists OBSERVED grants from the audit
        // plane. The grant below is a REAL recorded capability_grant event (the
        // same call the canonical mint path makes), folded by the real source — so
        // this cannot pass against the old hardcoded [] nor a hand-set fixture.
        use elastos_runtime::capability::token::{Action, ResourceId, TokenId};
        use elastos_runtime::primitives::audit::AuditLog;

        let audit_log = Arc::new(AuditLog::new());
        audit_log.capability_grant(
            &TokenId::new(),
            "vm-cap_granted_1",
            &ResourceId::new("elastos://inspect/*"),
            Action::Read,
            None,
        );

        let provider = InspectProvider::new(Arc::new(MockSource {
            entries: vec![InspectEntry {
                id: "cap_granted_1".to_string(),
                // The grant source keys by the normalized "vm-{name}"; the fixture
                // records under "vm-cap_granted_1", which format!("vm-{}", name) matches.
                name: "cap_granted_1".to_string(),
                status: "running".to_string(),
                capsule_type: "wasm".to_string(),
                manifest: Some(probe_manifest()),
                cid: None,
                verified_signer: None,
            }],
        }))
        .with_audit(Arc::new(RuntimeAuditLogGrantSource::new(audit_log)));

        let resp = provider
            .send_raw(&json!({ "op": "capsule", "id": "cap_granted_1" }))
            .await
            .unwrap();
        let granted = resp["data"]["granted_capabilities"]
            .as_array()
            .expect("granted_capabilities array");
        assert!(
            granted
                .iter()
                .any(|g| g["resource"] == "elastos://inspect/*"
                    && g["action"] == "read"
                    && g["granted"] == true),
            "granted_capabilities must list the observed grant, got {granted:?}"
        );

        // Honest empty: a capsule with NO recorded grants (no source) reports [].
        let empty = provider_with_probe()
            .send_raw(&json!({ "op": "capsule", "id": "cap_probe_1" }))
            .await
            .unwrap();
        assert_eq!(
            empty["data"]["granted_capabilities"],
            json!([]),
            "no observed grants => honest empty"
        );
    }

    #[tokio::test]
    async fn composite_audit_source_delegates_activity_and_grants_separately() {
        // G1b-LIVE: the composite serves observed grants from the GRANTS source (a
        // real grant recorded on the capability manager's log) AND signed activity
        // from the ACTIVITY source -- each answer comes from the source that owns it,
        // neither fabricated nor merged.
        use elastos_runtime::capability::token::{Action, ResourceId, TokenId};
        use elastos_runtime::primitives::audit::AuditLog;

        // grants source: a real recorded grant under the canonical "vm-probe".
        let audit_log = Arc::new(AuditLog::new());
        audit_log.capability_grant(
            &TokenId::new(),
            "vm-probe",
            &ResourceId::new("elastos://rights/*"),
            Action::Read,
            None,
        );
        let grants: Arc<dyn AuditSource> = Arc::new(RuntimeAuditLogGrantSource::new(audit_log));

        // activity source: a mock returning a sentinel CapsuleAudit.
        struct ActivityMock;
        #[async_trait]
        impl AuditSource for ActivityMock {
            async fn for_capsule(&self, _key: &str, _limit: usize) -> CapsuleAudit {
                CapsuleAudit {
                    total: 42,
                    denied: 7,
                    attested: 3,
                    recent: vec![],
                }
            }
        }
        let activity: Arc<dyn AuditSource> = Arc::new(ActivityMock);

        let composite = CompositeAuditSource::new(activity, grants);

        // granted_for_capsule delegates to the grants source (the observed grant).
        let granted = composite.granted_for_capsule("vm-probe", 100).await;
        assert!(
            granted
                .iter()
                .any(|g| g.resource == "elastos://rights/*" && g.action == "read" && g.granted),
            "composite serves observed grants from the grants source: {granted:?}"
        );

        // for_capsule delegates to the activity source (its sentinel), not the
        // grants source's empty activity.
        let audit = composite.for_capsule("vm-probe", 100).await;
        assert_eq!(
            audit.total, 42,
            "composite serves signed activity from the activity source"
        );
        assert_eq!(audit.denied, 7);
    }

    #[tokio::test]
    async fn ratchet_provenance_verified_signer_present() {
        // G2 (loop 3b): a capsule whose signature was GENUINELY verified reports a
        // verified signer + "verified" trust. The Some(fingerprint) below is earned
        // by a real ed25519 check (generate -> sign -> verify_capsule_signer), not a
        // hand-set field, so this ratchet cannot pass against a stub.
        use elastos_runtime::signature::{
            generate_keypair, hash_content, key_fingerprint, sign_capsule, SignatureVerifier,
        };
        let (signing_key, verifying_key) = generate_keypair();
        let mut manifest = probe_manifest();
        let content_hash = hash_content(b"verified-capsule-bytes");
        sign_capsule(&signing_key, &mut manifest, &content_hash).unwrap();

        // A real verification resolves the signer; only then do we name it.
        let mut verifier = SignatureVerifier::new();
        verifier.add_trusted_key(verifying_key);
        let matched = verifier
            .verify_capsule_signer(&manifest, &content_hash)
            .unwrap()
            .expect("the trusted key verifies the freshly signed manifest");
        let fingerprint = key_fingerprint(&matched);

        // Build an entry as the launch path will once 3c wires it: verified_signer
        // set from the real check.
        let provider = InspectProvider::new(Arc::new(MockSource {
            entries: vec![InspectEntry {
                id: "cap_verified_1".to_string(),
                name: "verified".to_string(),
                status: "running".to_string(),
                capsule_type: "wasm".to_string(),
                manifest: Some(manifest),
                cid: None,
                verified_signer: Some(fingerprint.clone()),
            }],
        }));
        let resp = provider
            .send_raw(&json!({ "op": "capsule", "id": "cap_verified_1" }))
            .await
            .unwrap();
        assert_eq!(
            resp["data"]["provenance"]["signed_by"], fingerprint,
            "signed_by must carry the verified signer fingerprint"
        );
        assert_eq!(
            resp["data"]["identity"]["trust_level"], "verified",
            "trust reaches 'verified' only when a real check resolved a signer"
        );

        // Negative: the same projection with NO verified signer stays honest-null
        // and presence-based — proving the green is earned by the check, not the field.
        let unverified = provider_with_probe()
            .send_raw(&json!({ "op": "capsule", "id": "cap_probe_1" }))
            .await
            .unwrap();
        assert!(unverified["data"]["provenance"]["signed_by"].is_null());
        assert_eq!(unverified["data"]["identity"]["trust_level"], "signed");
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
                    verified_signer: None,
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
    async fn capsule_detail_projects_intent_proof_present_absent_and_flagged() {
        use elastos_runtime::capability::intent::IntentProofSummary;

        // A source that returns a FLAGGED tally for the acting vm-{name}, and absent otherwise.
        struct IntentMock;
        #[async_trait]
        impl AuditSource for IntentMock {
            async fn for_capsule(&self, _key: &str, _limit: usize) -> CapsuleAudit {
                CapsuleAudit::default()
            }
            async fn intent_proof_summary(&self, key: &str) -> Option<IntentProofSummary> {
                // Intent events are keyed on the acting vm-{name}, never the bare capsule name.
                if key == "vm-probe" {
                    Some(IntentProofSummary {
                        denied: 2,
                        diverged: 1,
                        undelivered: 0,
                    })
                } else {
                    None
                }
            }
        }

        let provider = InspectProvider::new(Arc::new(MockSource {
            entries: vec![probe_entry()],
        }))
        .with_audit(Arc::new(IntentMock));
        let resp = provider
            .send_raw(&json!({ "op": "capsule", "id": "cap_probe_1" }))
            .await
            .unwrap();
        assert_eq!(resp["status"], "ok");
        // Present + non-zero ⇒ the exact IntentProofSummaryV1 shape, projected honestly.
        assert_eq!(resp["data"]["intent_proof"]["denied"], 2, "{resp}");
        assert_eq!(resp["data"]["intent_proof"]["diverged"], 1);
        assert_eq!(resp["data"]["intent_proof"]["undelivered"], 0);

        // No audit source attached ⇒ ABSENT (null), never a fabricated zeroed/clean tally.
        let bare = provider_with_probe()
            .send_raw(&json!({ "op": "capsule", "id": "cap_probe_1" }))
            .await
            .unwrap();
        assert!(
            bare["data"]["intent_proof"].is_null(),
            "a capsule with no intent-proof custody must project null (absent), not a clean 0/0/0: {bare}"
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
            verified_signer: None,
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
            verified_signer: None,
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
    async fn intent_requires_approval_and_defaults_fail_closed() {
        // key.release is an Execute op → it requires User approval, and with no
        // approver recorded the default decision is fail-closed (pending), never
        // auto-approved. Read-only: records and dispatches nothing.
        let resp = key_provider_with_release()
            .send_raw(&json!({
                "op": "intent", "id": "capsule:key-provider", "operation": "release"
            }))
            .await
            .unwrap();
        assert_eq!(resp["status"], "ok");
        assert_eq!(resp["data"]["valid"], true);
        assert_eq!(resp["data"]["kind"], "approval_intent");
        assert_eq!(resp["data"]["requires_approval"], "user");
        assert_eq!(resp["data"]["default_decision"], "pending_approval");
        assert_eq!(resp["data"]["resources"][0], "elastos://key/*");
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

    fn aud4_test_event() -> elastos_runtime::auth::RuntimeAuditEventV1 {
        elastos_runtime::auth::RuntimeAuditEventV1 {
            schema: "elastos.runtime.audit-event/v1".to_string(),
            event_id: "evt-1".to_string(),
            event_type: "capability.use".to_string(),
            principal_id: None,
            proof_binding_id: None,
            session_id: None,
            challenge_id: None,
            capsule_id: Some("vm-probe".to_string()),
            result: "ok".to_string(),
            reason: "test".to_string(),
            occurred_at: 1,
            signer_did: None,
            signature: None,
        }
    }

    #[test]
    fn aud4_attested_only_when_signature_verifies_against_runtime_did() {
        let dir = tempfile::tempdir().unwrap();
        let (_sk, did) = elastos_identity::load_or_create_did(dir.path()).unwrap();
        let vk = crate::crypto::decode_did_key(&did).unwrap();

        // A genuinely signed event verifies (the canonical-bytes round-trip: a real
        // event is NEVER false-denied).
        let signed = crate::auth::sign_audit_event(dir.path(), aud4_test_event()).unwrap();
        assert!(
            audit_event_is_attested(&signed, &did, &vk),
            "a genuine runtime signature must attest"
        );

        // No signature -> not attested, no error (legacy / seed events).
        assert!(!audit_event_is_attested(&aud4_test_event(), &did, &vk));

        // Present-but-forged signature -> not attested.
        let mut forged = signed.clone();
        forged.signature = Some("00".repeat(64));
        assert!(
            !audit_event_is_attested(&forged, &did, &vk),
            "a present-but-invalid signature must NOT count as attested"
        );

        // A signature by a DIFFERENT (attacker) DID -> not attested. The verifier pins
        // the runtime's OWN DID + key, never the embedded signer_did (anti-spoof).
        let attacker_dir = tempfile::tempdir().unwrap();
        let attacker_signed =
            crate::auth::sign_audit_event(attacker_dir.path(), aud4_test_event()).unwrap();
        assert!(
            !audit_event_is_attested(&attacker_signed, &did, &vk),
            "a signature by a non-runtime DID must be rejected"
        );
    }
}
