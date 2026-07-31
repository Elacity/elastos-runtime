//! Gateway-local Browser session accounting.
//!
//! This is not the final distributed Browser Session Manager, but it closes the
//! product hole where a launch-in-progress or dead page can make the Browser
//! look permanently busy. The Runtime gateway now accounts for launching and
//! active Browser pages before invoking the heavy engine supervisor.

use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_MAX_BROWSER_SESSIONS: usize = 4;
const DEFAULT_MAX_BROWSER_SESSIONS_PER_PRINCIPAL: usize = 4;
const MAX_BROWSER_SESSIONS_LIMIT: usize = 32;
const ACTIVE_HEARTBEAT_STALE_TTL: Duration = Duration::from_secs(5 * 60);
const OPEN_JOB_TTL: Duration = Duration::from_secs(15 * 60);
const MAX_BROWSER_DURABLE_OWNERSHIPS: usize = MAX_BROWSER_SESSIONS_LIMIT * 2;
const MAX_BROWSER_DURABLE_OWNERSHIP_BYTES: usize = 64 * 1024;
const BROWSER_DURABLE_OWNERSHIP_SCHEMA: &str = "elastos.browser.runtime-ownership/v1";
const BROWSER_LAUNCH_RECONCILIATION_SCHEMA: &str =
    "elastos.browser.runtime-launch-reconciliation/v1";
const BROWSER_LIFECYCLE_PHASES: &[&str] = &[
    "CONTROL_READY",
    "ACQUIRING_SLOT",
    "PREPARING_IMAGE",
    "STARTING_VM",
    "GUEST_READY",
    "ACTIVE_SESSION",
    "NAVIGATING",
    "QUIESCING_PAGE",
    "WARM_IDLE",
    "HIBERNATING",
    "HIBERNATED",
    "RETIRING",
    "FAILED",
];

static BROWSER_SESSION_REGISTRY: OnceLock<tokio::sync::Mutex<BrowserSessionRegistry>> =
    OnceLock::new();
static BROWSER_OPEN_JOB_REGISTRY: OnceLock<tokio::sync::Mutex<BrowserOpenJobRegistry>> =
    OnceLock::new();
static BROWSER_DURABLE_TEMP_SERIAL: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
static BROWSER_DURABLE_DELETE_FAILURES: OnceLock<std::sync::Mutex<BTreeSet<PathBuf>>> =
    OnceLock::new();

#[derive(Debug, Clone)]
pub(in crate::api::gateway) struct BrowserLaunchReservation {
    id: String,
    cleanup_id: String,
    generation: String,
    page_id: String,
    vm_id: String,
    engine_route_provider: String,
    selected_engine_adapter: Option<String>,
}

impl BrowserLaunchReservation {
    pub(in crate::api::gateway) fn generation(&self) -> &str {
        &self.generation
    }

    pub(in crate::api::gateway) fn cleanup_id(&self) -> &str {
        &self.cleanup_id
    }

    pub(in crate::api::gateway) fn page_id(&self) -> &str {
        &self.page_id
    }

    pub(in crate::api::gateway) fn vm_id(&self) -> &str {
        &self.vm_id
    }

    pub(in crate::api::gateway) fn engine_route_provider(&self) -> &str {
        &self.engine_route_provider
    }

    pub(in crate::api::gateway) fn selected_engine_adapter(&self) -> Option<&str> {
        self.selected_engine_adapter.as_deref()
    }
}

#[derive(Debug, Clone)]
pub(in crate::api::gateway) struct BrowserLaunchLifecycle {
    pub(in crate::api::gateway) owner_launch_id: String,
    pub(in crate::api::gateway) browser_instance: Option<String>,
    pub(in crate::api::gateway) url: String,
    pub(in crate::api::gateway) exit_id: String,
    pub(in crate::api::gateway) engine_route_provider: String,
    pub(in crate::api::gateway) selected_engine_adapter: Option<String>,
    pub(in crate::api::gateway) profile_key_hash: Option<String>,
    pub(in crate::api::gateway) vm_key_hash: Option<String>,
}

pub(in crate::api::gateway) struct BrowserLaunchEffect {
    pub(in crate::api::gateway) page_id: String,
    pub(in crate::api::gateway) engine_provider: String,
    pub(in crate::api::gateway) engine_protocol_version: String,
    pub(in crate::api::gateway) engine_adapter: String,
    pub(in crate::api::gateway) engine: String,
    pub(in crate::api::gateway) provider_cleanup: serde_json::Value,
    pub(in crate::api::gateway) browser_page: serde_json::Value,
    pub(in crate::api::gateway) stream_cleanup: Option<BrowserStreamCleanup>,
}

#[derive(Debug)]
pub(in crate::api::gateway) struct BrowserOpenJobReservation {
    pub(in crate::api::gateway) handle: BrowserOpenJobHandle,
    pub(in crate::api::gateway) should_spawn: bool,
}

#[derive(Debug, Clone)]
pub(in crate::api::gateway) struct BrowserOpenJobHandle {
    pub(in crate::api::gateway) id: String,
    scope: String,
    principal_id: String,
    owner_launch_id: String,
}

#[derive(Debug, Clone)]
pub(in crate::api::gateway) enum BrowserOpenJobSnapshot {
    Pending,
    Completed(serde_json::Value),
    Failed(serde_json::Value),
}

#[derive(Debug, Clone)]
struct BrowserSessionRecord {
    scope: String,
    principal_id: String,
    owner_launch_id: String,
    browser_instance: Option<String>,
    cleanup_id: String,
    generation: String,
    expected_page_id: String,
    vm_id: String,
    page_id: Option<String>,
    engine_route_provider: String,
    selected_engine_adapter: Option<String>,
    engine_provider: Option<String>,
    engine_protocol_version: Option<String>,
    engine_adapter: Option<String>,
    engine: Option<String>,
    provider_cleanup: Option<serde_json::Value>,
    browser_page: Option<serde_json::Value>,
    stream_cleanup: Option<BrowserStreamCleanup>,
    transport_authority: Option<serde_json::Value>,
    state: BrowserSessionState,
    phase: BrowserLifecyclePhase,
    url: String,
    exit_id: String,
    profile_key_hash: Option<String>,
    vm_key_hash: Option<String>,
    created_at: Instant,
    last_seen_at: Instant,
    started_at: SystemTime,
    last_navigation_at: Option<SystemTime>,
    last_frame_at: Option<SystemTime>,
    failure_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserSessionState {
    Launching,
    Active,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserLifecyclePhase {
    AcquiringSlot,
    PreparingImage,
    StartingVm,
    ActiveSession,
    Navigating,
    Retiring,
    Failed,
}

#[derive(Debug, Default)]
struct BrowserSessionRegistry {
    sessions: BTreeMap<String, BrowserSessionRecord>,
    pending_launch_reconciliations: BTreeMap<String, BrowserLaunchReconciliationObligation>,
    pending_engine_cleanups: BTreeMap<String, BrowserEngineCleanupObligation>,
    pending_stream_cleanups: BTreeMap<String, BrowserStreamCleanupObligation>,
    loaded_scopes: BTreeSet<String>,
    serial: u64,
}

#[derive(Debug, Default)]
struct BrowserOpenJobRegistry {
    jobs: BTreeMap<String, BrowserOpenJobRecord>,
    serial: u64,
}

#[derive(Debug, Clone)]
struct BrowserOpenJobRecord {
    scope: String,
    principal_id: String,
    owner_launch_id: String,
    intent_hash: String,
    state: BrowserOpenJobState,
    updated_at: Instant,
}

#[derive(Debug, Clone)]
enum BrowserOpenJobState {
    Pending,
    Completed(serde_json::Value),
    Failed(serde_json::Value),
}

#[derive(Debug, Clone, Copy)]
struct BrowserSessionLimits {
    total: usize,
    per_principal: usize,
}

#[derive(Debug, Clone)]
pub(in crate::api::gateway) struct BrowserPageCleanup {
    pub(in crate::api::gateway) engine_cleanup: BrowserEngineCleanup,
    pub(in crate::api::gateway) stream_cleanup: Option<BrowserStreamCleanup>,
    pub(in crate::api::gateway) active_session: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(in crate::api::gateway) struct BrowserCleanupHandle {
    pub(in crate::api::gateway) schema: String,
    pub(in crate::api::gateway) id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(in crate::api::gateway) struct BrowserEngineCleanup {
    pub(in crate::api::gateway) cleanup_id: String,
    pub(in crate::api::gateway) page_id: String,
    pub(in crate::api::gateway) principal_id: String,
    pub(in crate::api::gateway) owner_launch_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) browser_instance: Option<String>,
    pub(in crate::api::gateway) generation: String,
    pub(in crate::api::gateway) engine_route_provider: String,
    pub(in crate::api::gateway) engine_provider: String,
    pub(in crate::api::gateway) engine_protocol_version: String,
    pub(in crate::api::gateway) engine_adapter: String,
    pub(in crate::api::gateway) engine: String,
    pub(in crate::api::gateway) stream_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) transport_authority: Option<serde_json::Value>,
    pub(in crate::api::gateway) provider_cleanup: serde_json::Value,
}

#[derive(Debug, Clone)]
struct BrowserEngineCleanupObligation {
    scope: String,
    cleanup: BrowserEngineCleanup,
    browser_page: Option<serde_json::Value>,
    in_flight: bool,
    attempts: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(in crate::api::gateway) struct BrowserLaunchReconciliation {
    schema: String,
    pub(in crate::api::gateway) cleanup_id: String,
    pub(in crate::api::gateway) principal_id: String,
    pub(in crate::api::gateway) owner_launch_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) browser_instance: Option<String>,
    pub(in crate::api::gateway) generation: String,
    pub(in crate::api::gateway) engine_route_provider: String,
    #[serde(default)]
    pub(in crate::api::gateway) selected_engine_adapter: Option<String>,
    pub(in crate::api::gateway) stream_id: String,
    #[serde(default)]
    pub(in crate::api::gateway) stream_cleanup: Option<BrowserStreamCleanup>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) transport_authority: Option<serde_json::Value>,
    #[serde(
        default = "browser_launch_dispatch_state_dispatched",
        skip_serializing_if = "browser_launch_dispatch_state_is_dispatched"
    )]
    dispatch_state: BrowserLaunchDispatchState,
}

impl BrowserLaunchReconciliation {
    pub(in crate::api::gateway) fn was_dispatched(&self) -> bool {
        self.dispatch_state == BrowserLaunchDispatchState::Dispatched
    }

    pub(in crate::api::gateway) fn transport_authority(&self) -> Option<&serde_json::Value> {
        self.transport_authority.as_ref()
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum BrowserLaunchDispatchState {
    Prepared,
    Dispatched,
}

fn browser_launch_dispatch_state_dispatched() -> BrowserLaunchDispatchState {
    BrowserLaunchDispatchState::Dispatched
}

fn browser_launch_dispatch_state_is_dispatched(state: &BrowserLaunchDispatchState) -> bool {
    *state == BrowserLaunchDispatchState::Dispatched
}

#[derive(Debug, Clone)]
struct BrowserLaunchReconciliationObligation {
    scope: String,
    reconciliation: BrowserLaunchReconciliation,
    in_flight: bool,
}

#[derive(Debug, Clone)]
struct BrowserStreamCleanupObligation {
    cleanup: BrowserStreamCleanup,
    in_flight: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(in crate::api::gateway) struct BrowserStreamCleanup {
    pub(in crate::api::gateway) stream_id: String,
    pub(in crate::api::gateway) principal_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BrowserDurableOwnership {
    schema: String,
    engine_cleanup: BrowserEngineCleanup,
    #[serde(default)]
    stream_cleanup: Option<BrowserStreamCleanup>,
    browser_page: serde_json::Value,
}

pub(in crate::api::gateway) async fn reserve_browser_launch(
    data_dir: &Path,
    principal_id: &str,
    lifecycle: BrowserLaunchLifecycle,
) -> Result<BrowserLaunchReservation, (StatusCode, String)> {
    if principal_id.trim().is_empty()
        || lifecycle.owner_launch_id.trim().is_empty()
        || lifecycle.engine_route_provider.trim().is_empty()
        || lifecycle
            .selected_engine_adapter
            .as_deref()
            .is_some_and(|adapter| adapter.len() > 128 || !is_safe_runtime_id(adapter))
        || lifecycle
            .browser_instance
            .as_deref()
            .is_some_and(|instance| browser_instance_id(Some(instance.to_string())).is_err())
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "Browser launch requires verified lifecycle authority".to_string(),
        ));
    }
    let limits = browser_session_limits();
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    if let Err(message) = registry.load_durable_ownerships(data_dir) {
        return Err((StatusCode::SERVICE_UNAVAILABLE, message));
    }
    if registry.sessions.values().any(|session| {
        session.scope == scope
            && session.principal_id == principal_id
            && session.owner_launch_id == lifecycle.owner_launch_id
    }) {
        return Err((
            StatusCode::CONFLICT,
            "Browser lifecycle is already active or launching for this verified launch".to_string(),
        ));
    }
    if lifecycle.browser_instance.as_ref().is_some_and(|instance| {
        registry.sessions.values().any(|session| {
            session.scope == scope
                && session.principal_id == principal_id
                && session.browser_instance.as_ref() == Some(instance)
        })
    }) {
        return Err((
            StatusCode::CONFLICT,
            "Browser instance already owns an active or launching lifecycle".to_string(),
        ));
    }
    let stream_scope_prefix = format!("{scope}\n");
    if registry
        .pending_launch_reconciliations
        .values()
        .any(|obligation| {
            obligation.scope == scope && obligation.reconciliation.principal_id == principal_id
        })
        || registry.pending_engine_cleanups.values().any(|obligation| {
            obligation.scope == scope && obligation.cleanup.principal_id == principal_id
        })
        || registry
            .pending_stream_cleanups
            .iter()
            .any(|(key, obligation)| {
                key.starts_with(&stream_scope_prefix)
                    && obligation.cleanup.principal_id == principal_id
            })
    {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "Browser cleanup is pending for this account; no replacement page may open before terminal provider closure".to_string(),
        ));
    }
    let active_total = registry
        .sessions
        .values()
        .filter(|session| session.scope == scope)
        .count()
        + registry
            .pending_launch_reconciliations
            .values()
            .filter(|obligation| obligation.scope == scope)
            .count()
        + registry
            .pending_engine_cleanups
            .values()
            .filter(|obligation| obligation.scope == scope)
            .count();
    let active_for_principal = registry
        .sessions
        .values()
        .filter(|session| session.scope == scope && session.principal_id == principal_id)
        .count()
        + registry
            .pending_launch_reconciliations
            .values()
            .filter(|obligation| {
                obligation.scope == scope && obligation.reconciliation.principal_id == principal_id
            })
            .count()
        + registry
            .pending_engine_cleanups
            .values()
            .filter(|obligation| {
                obligation.scope == scope && obligation.cleanup.principal_id == principal_id
            })
            .count();
    if active_total >= limits.total {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            format!(
                "Browser capacity unavailable: {active_total}/{} Runtime Browser sessions are active or launching",
                limits.total
            ),
        ));
    }
    if active_for_principal >= limits.per_principal {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            format!(
                "Browser capacity unavailable: {active_for_principal}/{} Browser sessions are active or launching for this account",
                limits.per_principal
            ),
        ));
    }
    let id = registry.next_reservation_id();
    let generation = browser_launch_generation_hash_label(&format!(
        "{scope}\n{principal_id}\n{}\n{id}\n{}",
        lifecycle.owner_launch_id,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let cleanup_id = format!("browser-cleanup:{}", generation.replace(':', "-"));
    let page_id = format!(
        "page:vz-{}",
        hex::encode(Sha256::digest(format!("{generation}\npage").as_bytes()))
    );
    let vm_id = format!(
        "browser-vm-{}",
        hex::encode(Sha256::digest(format!("{generation}\nvm").as_bytes()))
    );
    let engine_route_provider = lifecycle.engine_route_provider.clone();
    let selected_engine_adapter = lifecycle.selected_engine_adapter.clone();
    let now = Instant::now();
    let started_at = SystemTime::now();
    registry.sessions.insert(
        id.clone(),
        BrowserSessionRecord {
            scope,
            principal_id: principal_id.to_string(),
            owner_launch_id: lifecycle.owner_launch_id,
            browser_instance: lifecycle.browser_instance,
            cleanup_id: cleanup_id.clone(),
            generation: generation.clone(),
            expected_page_id: page_id.clone(),
            vm_id: vm_id.clone(),
            page_id: None,
            engine_route_provider: lifecycle.engine_route_provider,
            selected_engine_adapter: lifecycle.selected_engine_adapter,
            engine_provider: None,
            engine_protocol_version: None,
            engine_adapter: None,
            engine: None,
            provider_cleanup: None,
            browser_page: None,
            stream_cleanup: None,
            transport_authority: None,
            state: BrowserSessionState::Launching,
            phase: BrowserLifecyclePhase::AcquiringSlot,
            url: sanitize_browser_lifecycle_url(&lifecycle.url),
            exit_id: lifecycle.exit_id,
            profile_key_hash: lifecycle.profile_key_hash,
            vm_key_hash: lifecycle.vm_key_hash,
            created_at: now,
            last_seen_at: now,
            started_at,
            last_navigation_at: None,
            last_frame_at: None,
            failure_reason: None,
        },
    );
    Ok(BrowserLaunchReservation {
        id,
        cleanup_id,
        generation,
        page_id,
        vm_id,
        engine_route_provider,
        selected_engine_adapter,
    })
}

pub(in crate::api::gateway) async fn create_browser_open_job(
    data_dir: &Path,
    principal_id: &str,
    owner_launch_id: &str,
    intent_hash: &str,
) -> Result<BrowserOpenJobReservation, (StatusCode, String)> {
    if principal_id.trim().is_empty()
        || owner_launch_id.trim().is_empty()
        || intent_hash.trim().is_empty()
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "Browser open job requires verified lifecycle authority and intent".to_string(),
        ));
    }
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_OPEN_JOB_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    registry.purge_expired(Instant::now());
    if let Some((id, job)) = registry.jobs.iter().find(|(_, job)| {
        job.scope == scope
            && job.principal_id == principal_id
            && job.owner_launch_id == owner_launch_id
            && matches!(
                &job.state,
                BrowserOpenJobState::Pending | BrowserOpenJobState::Completed(_)
            )
    }) {
        if job.intent_hash != intent_hash {
            return Err((
                StatusCode::CONFLICT,
                "Browser lifecycle already owns a different open intent".to_string(),
            ));
        }
        return Ok(BrowserOpenJobReservation {
            handle: BrowserOpenJobHandle {
                id: id.clone(),
                scope,
                principal_id: principal_id.to_string(),
                owner_launch_id: owner_launch_id.to_string(),
            },
            should_spawn: false,
        });
    }
    let id = registry.next_job_id();
    let now = Instant::now();
    registry.jobs.insert(
        id.clone(),
        BrowserOpenJobRecord {
            scope: scope.clone(),
            principal_id: principal_id.to_string(),
            owner_launch_id: owner_launch_id.to_string(),
            intent_hash: intent_hash.to_string(),
            state: BrowserOpenJobState::Pending,
            updated_at: now,
        },
    );
    Ok(BrowserOpenJobReservation {
        handle: BrowserOpenJobHandle {
            id,
            scope,
            principal_id: principal_id.to_string(),
            owner_launch_id: owner_launch_id.to_string(),
        },
        should_spawn: true,
    })
}

pub(in crate::api::gateway) async fn complete_browser_open_job(
    handle: &BrowserOpenJobHandle,
    result: serde_json::Value,
) {
    update_browser_open_job(handle, BrowserOpenJobState::Completed(result)).await;
}

pub(in crate::api::gateway) async fn fail_browser_open_job(
    handle: &BrowserOpenJobHandle,
    error: serde_json::Value,
) {
    update_browser_open_job(handle, BrowserOpenJobState::Failed(error)).await;
}

pub(in crate::api::gateway) async fn browser_open_job_for_owner(
    data_dir: &Path,
    open_id: &str,
    principal_id: &str,
    owner_launch_id: &str,
) -> Option<BrowserOpenJobSnapshot> {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_OPEN_JOB_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    registry.purge_expired(Instant::now());
    registry.jobs.get(open_id).and_then(|job| {
        (job.scope == scope
            && job.principal_id == principal_id
            && job.owner_launch_id == owner_launch_id)
            .then(|| match &job.state {
                BrowserOpenJobState::Pending => BrowserOpenJobSnapshot::Pending,
                BrowserOpenJobState::Completed(result) => {
                    BrowserOpenJobSnapshot::Completed(result.clone())
                }
                BrowserOpenJobState::Failed(error) => BrowserOpenJobSnapshot::Failed(error.clone()),
            })
    })
}

pub(in crate::api::gateway) async fn forget_browser_open_job_for_owner(
    data_dir: &Path,
    principal_id: &str,
    owner_launch_id: &str,
) {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_OPEN_JOB_REGISTRY.get_or_init(Default::default);
    registry.lock().await.jobs.retain(|_, job| {
        job.scope != scope
            || job.principal_id != principal_id
            || job.owner_launch_id != owner_launch_id
    });
}

async fn update_browser_open_job(handle: &BrowserOpenJobHandle, state: BrowserOpenJobState) {
    let registry = BROWSER_OPEN_JOB_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    registry.purge_expired(Instant::now());
    if let Some(job) = registry.jobs.get_mut(&handle.id) {
        if job.scope == handle.scope
            && job.principal_id == handle.principal_id
            && job.owner_launch_id == handle.owner_launch_id
        {
            job.state = state;
            job.updated_at = Instant::now();
        }
    }
}

pub(in crate::api::gateway) async fn complete_browser_launch(
    data_dir: &Path,
    reservation: &BrowserLaunchReservation,
    effect: BrowserLaunchEffect,
) -> Result<BrowserCleanupHandle, String> {
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    let ownership = {
        let record = registry.sessions.get_mut(&reservation.id).ok_or_else(|| {
            "Browser launch ownership disappeared before effect binding".to_string()
        })?;
        if record.cleanup_id != reservation.cleanup_id
            || record.generation != reservation.generation
            || record.scope != browser_session_scope(data_dir)
            || record.selected_engine_adapter != reservation.selected_engine_adapter
            || record
                .transport_authority
                .as_ref()
                .is_some_and(|authority| {
                    effect.page_id != record.expected_page_id
                        || effect.provider_cleanup.get("transport_authority") != Some(authority)
                })
            || record
                .selected_engine_adapter
                .as_deref()
                .is_some_and(|selected| selected != effect.engine_adapter)
        {
            return Err("Browser launch ownership changed before effect binding".to_string());
        }
        record.page_id = Some(effect.page_id);
        record.engine_provider = Some(effect.engine_provider);
        record.engine_protocol_version = Some(effect.engine_protocol_version);
        record.engine_adapter = Some(effect.engine_adapter);
        record.engine = Some(effect.engine);
        record.provider_cleanup = Some(effect.provider_cleanup);
        record.browser_page = Some(effect.browser_page);
        record.stream_cleanup = effect.stream_cleanup;
        record.state = BrowserSessionState::Active;
        record.phase = BrowserLifecyclePhase::ActiveSession;
        record.last_seen_at = Instant::now();
        record.last_navigation_at = Some(SystemTime::now());
        record.failure_reason = None;
        browser_durable_ownership(record)
            .ok_or_else(|| "Browser provider cleanup binding is incomplete".to_string())?
    };
    write_browser_durable_ownership(data_dir, &ownership)?;
    if reservation_transport_authority_is_bound(&registry, reservation) {
        remove_browser_durable_file(
            data_dir,
            browser_launch_reconciliation_path(data_dir, &reservation.cleanup_id),
        )?;
    }
    Ok(BrowserCleanupHandle {
        schema: "elastos.browser.cleanup-handle/v1".to_string(),
        id: reservation.cleanup_id.clone(),
    })
}

fn reservation_transport_authority_is_bound(
    registry: &BrowserSessionRegistry,
    reservation: &BrowserLaunchReservation,
) -> bool {
    registry
        .sessions
        .get(&reservation.id)
        .is_some_and(|record| record.transport_authority.is_some())
}

pub(in crate::api::gateway) async fn release_browser_launch(
    reservation: &BrowserLaunchReservation,
) {
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    registry.lock().await.sessions.remove(&reservation.id);
}

pub(in crate::api::gateway) async fn browser_launch_transport_authority(
    reservation: &BrowserLaunchReservation,
) -> Option<serde_json::Value> {
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    registry
        .lock()
        .await
        .sessions
        .get(&reservation.id)
        .and_then(|record| record.transport_authority.clone())
}

pub(in crate::api::gateway) async fn bind_browser_vz_transport_authority(
    data_dir: &Path,
    reservation: &BrowserLaunchReservation,
    stream_id: &str,
    stream_cleanup: Option<BrowserStreamCleanup>,
    authority: serde_json::Value,
) -> Result<(), String> {
    validate_live_browser_vz_transport_authority(&authority)?;
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    let record = registry.sessions.get_mut(&reservation.id).ok_or_else(|| {
        "Browser launch ownership disappeared before transport binding".to_string()
    })?;
    if record.scope != scope
        || record.cleanup_id != reservation.cleanup_id
        || record.generation != reservation.generation
        || record.expected_page_id != reservation.page_id
        || record.vm_id != reservation.vm_id
        || authority
            .get("generation")
            .and_then(serde_json::Value::as_str)
            != Some(record.generation.as_str())
        || authority.get("page_id").and_then(serde_json::Value::as_str)
            != Some(record.expected_page_id.as_str())
        || authority.get("vm_id").and_then(serde_json::Value::as_str) != Some(record.vm_id.as_str())
        || authority
            .pointer("/egress/stream_id")
            .and_then(serde_json::Value::as_str)
            != Some(stream_id)
        || stream_cleanup.as_ref().is_some_and(|cleanup| {
            cleanup.stream_id != stream_id || cleanup.principal_id != record.principal_id
        })
    {
        return Err("Browser VZ transport ownership changed before binding".to_string());
    }
    record.transport_authority = Some(authority.clone());
    record.stream_cleanup = stream_cleanup.clone();
    let reconciliation = browser_launch_reconciliation_for_record(
        record,
        stream_id,
        stream_cleanup,
        BrowserLaunchDispatchState::Prepared,
    );
    if !browser_launch_reconciliation_is_safe(&reconciliation) {
        record.transport_authority = None;
        return Err("Browser VZ transport reconciliation binding is invalid".to_string());
    }
    if let Err(err) = write_browser_launch_reconciliation(data_dir, &reconciliation) {
        record.transport_authority = None;
        return Err(err);
    }
    Ok(())
}

pub(in crate::api::gateway) async fn mark_browser_vz_transport_dispatched(
    data_dir: &Path,
    reservation: &BrowserLaunchReservation,
    stream_id: &str,
) -> Result<(), String> {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let registry = registry.lock().await;
    let record = registry
        .sessions
        .get(&reservation.id)
        .ok_or_else(|| "Browser launch ownership disappeared before dispatch".to_string())?;
    if record.scope != scope
        || record.cleanup_id != reservation.cleanup_id
        || record.generation != reservation.generation
        || record.transport_authority.is_none()
        || record
            .transport_authority
            .as_ref()
            .and_then(|authority| authority.pointer("/egress/stream_id"))
            .and_then(serde_json::Value::as_str)
            != Some(stream_id)
    {
        return Err("Browser VZ transport ownership changed before dispatch".to_string());
    }
    let reconciliation = browser_launch_reconciliation_for_record(
        record,
        stream_id,
        record.stream_cleanup.clone(),
        BrowserLaunchDispatchState::Dispatched,
    );
    write_browser_launch_reconciliation(data_dir, &reconciliation)
}

pub(in crate::api::gateway) async fn discard_browser_vz_transport_preparation(
    data_dir: &Path,
    reservation: &BrowserLaunchReservation,
) -> Result<(), String> {
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let registry = registry.lock().await;
    let Some(record) = registry.sessions.get(&reservation.id) else {
        return Ok(());
    };
    if record.transport_authority.is_none() {
        return Ok(());
    }
    remove_browser_durable_file(
        data_dir,
        browser_launch_reconciliation_path(data_dir, &reservation.cleanup_id),
    )
}

pub(in crate::api::gateway) async fn record_browser_launch_reconciliation_obligation(
    data_dir: &Path,
    reservation: &BrowserLaunchReservation,
    stream_id: &str,
    stream_cleanup: Option<BrowserStreamCleanup>,
) -> Result<(), String> {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    let record = registry
        .sessions
        .get(&reservation.id)
        .ok_or_else(|| "Browser launch ownership disappeared before reconciliation".to_string())?;
    if record.scope != scope
        || record.cleanup_id != reservation.cleanup_id
        || record.generation != reservation.generation
        || record.selected_engine_adapter != reservation.selected_engine_adapter
        || stream_cleanup.as_ref().is_some_and(|cleanup| {
            cleanup.stream_id != stream_id || cleanup.principal_id != record.principal_id
        })
    {
        return Err("Browser launch reconciliation ownership changed".to_string());
    }
    let reconciliation = browser_launch_reconciliation_for_record(
        record,
        stream_id,
        stream_cleanup,
        BrowserLaunchDispatchState::Dispatched,
    );
    if !browser_launch_reconciliation_is_safe(&reconciliation) {
        return Err("Browser launch reconciliation binding is invalid".to_string());
    }
    write_browser_launch_reconciliation(data_dir, &reconciliation)?;
    registry.pending_launch_reconciliations.insert(
        browser_launch_reconciliation_key(&scope, &reconciliation.generation),
        BrowserLaunchReconciliationObligation {
            scope,
            reconciliation,
            in_flight: false,
        },
    );
    registry.sessions.remove(&reservation.id);
    drop(registry);
    super::notify_browser_lifecycle_reconciler(data_dir);
    Ok(())
}

fn browser_launch_reconciliation_for_record(
    record: &BrowserSessionRecord,
    stream_id: &str,
    stream_cleanup: Option<BrowserStreamCleanup>,
    dispatch_state: BrowserLaunchDispatchState,
) -> BrowserLaunchReconciliation {
    BrowserLaunchReconciliation {
        schema: BROWSER_LAUNCH_RECONCILIATION_SCHEMA.to_string(),
        cleanup_id: record.cleanup_id.clone(),
        principal_id: record.principal_id.clone(),
        owner_launch_id: record.owner_launch_id.clone(),
        browser_instance: record.browser_instance.clone(),
        generation: record.generation.clone(),
        engine_route_provider: record.engine_route_provider.clone(),
        selected_engine_adapter: record.selected_engine_adapter.clone(),
        stream_id: stream_id.to_string(),
        stream_cleanup,
        transport_authority: record.transport_authority.clone(),
        dispatch_state,
    }
}

pub(in crate::api::gateway) async fn claim_pending_browser_launch_reconciliations(
    data_dir: &Path,
    limit: usize,
) -> Vec<BrowserLaunchReconciliation> {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    if registry.load_durable_ownerships(data_dir).is_err() {
        return Vec::new();
    }
    registry
        .pending_launch_reconciliations
        .values_mut()
        .filter(|obligation| obligation.scope == scope && !obligation.in_flight)
        .take(limit)
        .map(|obligation| {
            obligation.in_flight = true;
            obligation.reconciliation.clone()
        })
        .collect()
}

pub(in crate::api::gateway) async fn release_browser_launch_reconciliation_claim(
    data_dir: &Path,
    reconciliation: &BrowserLaunchReconciliation,
) {
    let scope = browser_session_scope(data_dir);
    let key = browser_launch_reconciliation_key(&scope, &reconciliation.generation);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    if let Some(obligation) = registry
        .lock()
        .await
        .pending_launch_reconciliations
        .get_mut(&key)
    {
        obligation.in_flight = false;
    }
}

pub(in crate::api::gateway) async fn release_browser_lifecycle_reconciliation_claims(
    data_dir: &Path,
) {
    let scope = browser_session_scope(data_dir);
    let stream_prefix = format!("{scope}\n");
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    for obligation in registry.pending_launch_reconciliations.values_mut() {
        if obligation.scope == scope {
            obligation.in_flight = false;
        }
    }
    for obligation in registry.pending_engine_cleanups.values_mut() {
        if obligation.scope == scope {
            obligation.in_flight = false;
        }
    }
    for (key, obligation) in registry.pending_stream_cleanups.iter_mut() {
        if key.starts_with(&stream_prefix) {
            obligation.in_flight = false;
        }
    }
}

pub(in crate::api::gateway) async fn promote_browser_launch_reconciliation_effect(
    data_dir: &Path,
    reconciliation: &BrowserLaunchReconciliation,
    cleanup: BrowserEngineCleanup,
) -> Result<(), String> {
    if cleanup.cleanup_id != reconciliation.cleanup_id
        || cleanup.principal_id != reconciliation.principal_id
        || cleanup.owner_launch_id != reconciliation.owner_launch_id
        || cleanup.generation != reconciliation.generation
        || cleanup.engine_route_provider != reconciliation.engine_route_provider
        || cleanup.stream_id != reconciliation.stream_id
        || cleanup.transport_authority != reconciliation.transport_authority
        || !browser_engine_cleanup_is_safe(&cleanup)
    {
        return Err("Browser reconciled effect ownership changed".to_string());
    }
    let ownership = BrowserDurableOwnership {
        schema: BROWSER_DURABLE_OWNERSHIP_SCHEMA.to_string(),
        engine_cleanup: cleanup.clone(),
        stream_cleanup: reconciliation.stream_cleanup.clone(),
        browser_page: serde_json::json!({
            "schema": "elastos.browser.engine.reconciled-effect/v1",
            "page_id": cleanup.page_id,
        }),
    };
    write_browser_durable_ownership(data_dir, &ownership)?;

    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    registry.load_durable_ownerships(data_dir)?;
    if let Some(stream) = reconciliation.stream_cleanup.clone() {
        registry
            .pending_stream_cleanups
            .entry(browser_stream_cleanup_key(&scope, &stream.stream_id))
            .or_insert(BrowserStreamCleanupObligation {
                cleanup: stream,
                in_flight: false,
            });
    }
    registry.pending_engine_cleanups.insert(
        browser_engine_cleanup_key(&scope, &cleanup),
        BrowserEngineCleanupObligation {
            scope: scope.clone(),
            cleanup,
            browser_page: Some(ownership.browser_page),
            in_flight: true,
            attempts: 1,
        },
    );
    remove_browser_durable_file(
        data_dir,
        browser_launch_reconciliation_path(data_dir, &reconciliation.cleanup_id),
    )?;
    registry
        .pending_launch_reconciliations
        .remove(&browser_launch_reconciliation_key(
            &scope,
            &reconciliation.generation,
        ));
    registry.sessions.retain(|_, session| {
        session.scope != scope || session.generation != reconciliation.generation
    });
    drop(registry);
    super::notify_browser_lifecycle_reconciler(data_dir);
    Ok(())
}

pub(in crate::api::gateway) async fn forget_browser_launch_reconciliation_obligation(
    data_dir: &Path,
    reconciliation: &BrowserLaunchReconciliation,
) -> Result<(), String> {
    remove_browser_durable_file(
        data_dir,
        browser_launch_reconciliation_path(data_dir, &reconciliation.cleanup_id),
    )?;
    let scope = browser_session_scope(data_dir);
    let key = browser_launch_reconciliation_key(&scope, &reconciliation.generation);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    registry.pending_launch_reconciliations.remove(&key);
    registry.sessions.retain(|_, session| {
        session.scope != scope || session.generation != reconciliation.generation
    });
    Ok(())
}

#[cfg(test)]
pub(in crate::api::gateway) async fn browser_launch_reconciliation_obligation_count(
    data_dir: &Path,
) -> usize {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    registry
        .lock()
        .await
        .pending_launch_reconciliations
        .values()
        .filter(|obligation| obligation.scope == scope)
        .count()
}

pub(in crate::api::gateway) async fn browser_page_cleanup_for_principal(
    data_dir: &Path,
    page_id: &str,
    principal_id: &str,
    owner_launch_id: &str,
    cleanup_id: &str,
) -> Result<Option<BrowserPageCleanup>, String> {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    registry.load_durable_ownerships(data_dir)?;
    let active = registry.sessions.values().find(|session| {
        session.scope == scope
            && session.principal_id == principal_id
            && (session.owner_launch_id == owner_launch_id || session.browser_instance.is_some())
            && session.cleanup_id == cleanup_id
            && session.page_id.as_deref() == Some(page_id)
    });
    if let Some(session) = active {
        if let Some(engine_cleanup) = browser_engine_cleanup(session) {
            return Ok(Some(BrowserPageCleanup {
                engine_cleanup,
                stream_cleanup: session.stream_cleanup.clone(),
                active_session: true,
            }));
        }
        return Ok(None);
    }

    Ok(registry
        .pending_engine_cleanups
        .values()
        .find(|obligation| {
            obligation.scope == scope
                && obligation.cleanup.page_id == page_id
                && obligation.cleanup.principal_id == principal_id
                && (obligation.cleanup.owner_launch_id == owner_launch_id
                    || obligation.cleanup.browser_instance.is_some())
                && obligation.cleanup.cleanup_id == cleanup_id
        })
        .map(|obligation| {
            let stream_cleanup = (!obligation.cleanup.stream_id.is_empty())
                .then(|| {
                    let key = browser_stream_cleanup_key(&scope, &obligation.cleanup.stream_id);
                    registry
                        .pending_stream_cleanups
                        .get(&key)
                        .map(|obligation| obligation.cleanup.clone())
                })
                .flatten()
                .filter(|cleanup| cleanup.principal_id == principal_id);
            BrowserPageCleanup {
                engine_cleanup: obligation.cleanup.clone(),
                stream_cleanup,
                active_session: false,
            }
        }))
}

pub(in crate::api::gateway) async fn mark_browser_launch_preparing_image(
    reservation: &BrowserLaunchReservation,
) {
    mark_browser_launch_phase(reservation, BrowserLifecyclePhase::PreparingImage, None).await;
}

pub(in crate::api::gateway) async fn mark_browser_launch_starting_vm(
    reservation: &BrowserLaunchReservation,
) {
    mark_browser_launch_phase(reservation, BrowserLifecyclePhase::StartingVm, None).await;
}

pub(in crate::api::gateway) async fn mark_browser_launch_failed(
    reservation: &BrowserLaunchReservation,
    failure_reason: impl Into<String>,
) {
    mark_browser_launch_phase(
        reservation,
        BrowserLifecyclePhase::Failed,
        Some(failure_reason.into()),
    )
    .await;
}

async fn mark_browser_launch_phase(
    reservation: &BrowserLaunchReservation,
    phase: BrowserLifecyclePhase,
    failure_reason: Option<String>,
) {
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    if let Some(record) = registry.sessions.get_mut(&reservation.id) {
        record.phase = phase;
        if phase == BrowserLifecyclePhase::Failed {
            record.failure_reason = failure_reason.map(sanitize_failure_reason);
        }
    }
}

pub(in crate::api::gateway) async fn release_browser_page_for_principal(
    data_dir: &Path,
    page_id: &str,
    principal_id: &str,
    owner_launch_id: &str,
) -> Option<BrowserStreamCleanup> {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    let key = registry.sessions.iter().find_map(|(key, session)| {
        (session.scope == scope
            && session.principal_id == principal_id
            && session.owner_launch_id == owner_launch_id
            && session.page_id.as_deref() == Some(page_id))
        .then(|| key.clone())
    });
    key.and_then(|key| registry.sessions.remove(&key))
        .and_then(|session| session.stream_cleanup)
}

pub(in crate::api::gateway) async fn mark_browser_page_navigating(
    data_dir: &Path,
    page_id: &str,
    principal_id: &str,
    owner_launch_id: &str,
    url: Option<&str>,
) -> bool {
    mark_browser_page_lifecycle(
        data_dir,
        page_id,
        principal_id,
        owner_launch_id,
        BrowserLifecyclePhase::Navigating,
        url,
        None,
    )
    .await
}

pub(in crate::api::gateway) async fn mark_browser_page_active(
    data_dir: &Path,
    page_id: &str,
    principal_id: &str,
    owner_launch_id: &str,
) -> bool {
    mark_browser_page_lifecycle(
        data_dir,
        page_id,
        principal_id,
        owner_launch_id,
        BrowserLifecyclePhase::ActiveSession,
        None,
        None,
    )
    .await
}

pub(in crate::api::gateway) async fn mark_browser_page_retiring(
    data_dir: &Path,
    page_id: &str,
    principal_id: &str,
    owner_launch_id: &str,
) -> bool {
    mark_browser_page_lifecycle(
        data_dir,
        page_id,
        principal_id,
        owner_launch_id,
        BrowserLifecyclePhase::Retiring,
        None,
        None,
    )
    .await
}

pub(in crate::api::gateway) async fn mark_browser_page_failed(
    data_dir: &Path,
    page_id: &str,
    principal_id: &str,
    owner_launch_id: &str,
    failure_reason: impl Into<String>,
) -> bool {
    mark_browser_page_lifecycle(
        data_dir,
        page_id,
        principal_id,
        owner_launch_id,
        BrowserLifecyclePhase::Failed,
        None,
        Some(failure_reason.into()),
    )
    .await
}

async fn mark_browser_page_lifecycle(
    data_dir: &Path,
    page_id: &str,
    principal_id: &str,
    owner_launch_id: &str,
    phase: BrowserLifecyclePhase,
    url: Option<&str>,
    failure_reason: Option<String>,
) -> bool {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    for session in registry.sessions.values_mut() {
        if session.scope == scope
            && session.principal_id == principal_id
            && session.owner_launch_id == owner_launch_id
            && session.page_id.as_deref() == Some(page_id)
        {
            session.phase = phase;
            session.last_seen_at = Instant::now();
            if phase == BrowserLifecyclePhase::Navigating {
                session.last_navigation_at = Some(SystemTime::now());
                if let Some(url) = url {
                    session.url = sanitize_browser_lifecycle_url(url);
                }
            }
            if phase == BrowserLifecyclePhase::ActiveSession {
                session.failure_reason = None;
            }
            if phase == BrowserLifecyclePhase::Failed {
                session.failure_reason = failure_reason.map(sanitize_failure_reason);
            }
            return true;
        }
    }
    false
}

pub(in crate::api::gateway) async fn browser_page_stream_cleanup_for_principal(
    data_dir: &Path,
    page_id: &str,
    principal_id: &str,
    owner_launch_id: &str,
) -> Option<BrowserStreamCleanup> {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let registry = registry.lock().await;
    registry.sessions.values().find_map(|session| {
        (session.scope == scope
            && session.principal_id == principal_id
            && session.owner_launch_id == owner_launch_id
            && session.page_id.as_deref() == Some(page_id))
        .then(|| session.stream_cleanup.clone())
        .flatten()
    })
}

pub(in crate::api::gateway) async fn record_browser_engine_cleanup_obligation(
    data_dir: &Path,
    cleanup: BrowserEngineCleanup,
) -> Result<(), String> {
    let scope = browser_session_scope(data_dir);
    let key = browser_engine_cleanup_key(&scope, &cleanup);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    registry.load_durable_ownerships(data_dir)?;
    let session = registry
        .sessions
        .values()
        .find(|session| session.scope == scope && session.cleanup_id == cleanup.cleanup_id);
    let browser_page = session
        .and_then(|session| session.browser_page.clone())
        .unwrap_or_else(|| serde_json::json!({"page_id": cleanup.page_id}));
    let persist_result = write_browser_durable_ownership(
        data_dir,
        &BrowserDurableOwnership {
            schema: BROWSER_DURABLE_OWNERSHIP_SCHEMA.to_string(),
            engine_cleanup: cleanup.clone(),
            stream_cleanup: session.and_then(|session| session.stream_cleanup.clone()),
            browser_page: browser_page.clone(),
        },
    );
    let is_new_obligation = !registry.pending_engine_cleanups.contains_key(&key);
    registry
        .pending_engine_cleanups
        .entry(key)
        .and_modify(|obligation| {
            obligation.in_flight = true;
            obligation.attempts = obligation.attempts.saturating_add(1);
        })
        .or_insert(BrowserEngineCleanupObligation {
            scope,
            cleanup,
            browser_page: Some(browser_page),
            in_flight: true,
            attempts: 1,
        });
    drop(registry);
    if persist_result.is_ok() && is_new_obligation {
        super::notify_browser_lifecycle_reconciler(data_dir);
    }
    persist_result
}

pub(in crate::api::gateway) async fn claim_pending_browser_engine_cleanups(
    data_dir: &Path,
    limit: usize,
) -> Vec<BrowserEngineCleanup> {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    if registry.load_durable_ownerships(data_dir).is_err() {
        return Vec::new();
    }
    registry
        .pending_engine_cleanups
        .values_mut()
        .filter(|obligation| obligation.scope == scope && !obligation.in_flight)
        .take(limit)
        .map(|obligation| {
            obligation.in_flight = true;
            obligation.attempts = obligation.attempts.saturating_add(1);
            obligation.cleanup.clone()
        })
        .collect()
}

pub(in crate::api::gateway) async fn release_browser_engine_cleanup_claim(
    data_dir: &Path,
    cleanup: &BrowserEngineCleanup,
) {
    let scope = browser_session_scope(data_dir);
    let key = browser_engine_cleanup_key(&scope, cleanup);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    if let Some(obligation) = registry.lock().await.pending_engine_cleanups.get_mut(&key) {
        obligation.in_flight = false;
    }
}

pub(in crate::api::gateway) async fn forget_browser_engine_cleanup_obligation(
    data_dir: &Path,
    cleanup: &BrowserEngineCleanup,
) -> Result<(), String> {
    let scope = browser_session_scope(data_dir);
    let key = browser_engine_cleanup_key(&scope, cleanup);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    if let Err(err) = remove_browser_durable_file(
        data_dir,
        browser_ownership_path(data_dir, &cleanup.cleanup_id),
    ) {
        if let Some(obligation) = registry.lock().await.pending_engine_cleanups.get_mut(&key) {
            obligation.in_flight = false;
        }
        return Err(err);
    }
    registry.lock().await.pending_engine_cleanups.remove(&key);
    Ok(())
}

#[cfg(test)]
pub(in crate::api::gateway) async fn browser_engine_cleanup_obligation_count(
    data_dir: &Path,
) -> usize {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    registry
        .lock()
        .await
        .pending_engine_cleanups
        .values()
        .filter(|obligation| obligation.scope == scope)
        .count()
}

#[cfg(test)]
pub(in crate::api::gateway) async fn browser_page_session_count(data_dir: &Path) -> usize {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    registry
        .lock()
        .await
        .sessions
        .values()
        .filter(|session| session.scope == scope)
        .count()
}

pub(in crate::api::gateway) async fn record_browser_stream_cleanup_failure(
    data_dir: &Path,
    cleanup: BrowserStreamCleanup,
) -> Result<(), String> {
    let persist_result = write_browser_durable_stream_cleanup(data_dir, &cleanup);
    let scope = browser_session_scope(data_dir);
    let key = browser_stream_cleanup_key(&scope, &cleanup.stream_id);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    let is_new_obligation = !registry.pending_stream_cleanups.contains_key(&key);
    registry
        .pending_stream_cleanups
        .entry(key)
        .and_modify(|obligation| {
            obligation.cleanup = cleanup.clone();
            obligation.in_flight = false;
        })
        .or_insert(BrowserStreamCleanupObligation {
            cleanup,
            in_flight: false,
        });
    drop(registry);
    if persist_result.is_ok() && is_new_obligation {
        super::notify_browser_lifecycle_reconciler(data_dir);
    }
    persist_result
}

pub(in crate::api::gateway) async fn forget_browser_stream_cleanup_failure(
    data_dir: &Path,
    cleanup: &BrowserStreamCleanup,
) -> Result<(), String> {
    let scope = browser_session_scope(data_dir);
    let key = browser_stream_cleanup_key(&scope, &cleanup.stream_id);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    if let Err(err) = remove_browser_durable_file(
        data_dir,
        browser_stream_cleanup_path(data_dir, &cleanup.stream_id),
    ) {
        registry
            .lock()
            .await
            .pending_stream_cleanups
            .entry(key)
            .and_modify(|obligation| {
                obligation.cleanup = cleanup.clone();
                obligation.in_flight = false;
            })
            .or_insert(BrowserStreamCleanupObligation {
                cleanup: cleanup.clone(),
                in_flight: false,
            });
        return Err(err);
    }
    registry.lock().await.pending_stream_cleanups.remove(&key);
    Ok(())
}

pub(in crate::api::gateway) async fn browser_pending_stream_cleanup_for_engine(
    data_dir: &Path,
    cleanup: &BrowserEngineCleanup,
) -> Option<BrowserStreamCleanup> {
    if cleanup.stream_id.is_empty() {
        return None;
    }
    let scope = browser_session_scope(data_dir);
    let key = browser_stream_cleanup_key(&scope, &cleanup.stream_id);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    registry
        .lock()
        .await
        .pending_stream_cleanups
        .get(&key)
        .filter(|obligation| obligation.cleanup.principal_id == cleanup.principal_id)
        .map(|obligation| obligation.cleanup.clone())
}

pub(in crate::api::gateway) async fn touch_browser_page(
    data_dir: &Path,
    page_id: &str,
    principal_id: &str,
    owner_launch_id: &str,
) -> bool {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    for session in registry.sessions.values_mut() {
        if session.scope == scope
            && session.principal_id == principal_id
            && session.owner_launch_id == owner_launch_id
            && session.page_id.as_deref() == Some(page_id)
        {
            session.last_seen_at = Instant::now();
            return true;
        }
    }
    false
}

pub(in crate::api::gateway) async fn touch_browser_page_transport_authority(
    data_dir: &Path,
    page_id: &str,
    principal_id: &str,
    owner_launch_id: &str,
) -> Option<Option<serde_json::Value>> {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    for session in registry.sessions.values_mut() {
        if session.scope == scope
            && session.principal_id == principal_id
            && session.owner_launch_id == owner_launch_id
            && session.page_id.as_deref() == Some(page_id)
        {
            session.last_seen_at = Instant::now();
            return Some(session.transport_authority.clone());
        }
    }
    None
}

pub(in crate::api::gateway) async fn browser_gateway_session_status(
    data_dir: &Path,
    principal_id: &str,
    owner_launch_id: Option<&str>,
    browser_instance: Option<&str>,
) -> serde_json::Value {
    let limits = browser_session_limits();
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    if let Err(message) = registry.load_durable_ownerships(data_dir) {
        return serde_json::json!({
            "schema": "elastos.browser.session-capacity/v1",
            "status": "unavailable",
            "active_sessions": 0,
            "launching_sessions": 0,
            "total_sessions": 0,
            "principal_sessions": 0,
            "engine_cleanup_obligations": 1,
            "capacity_available": false,
            "recoverable_page": serde_json::Value::Null,
            "reason": sanitize_failure_reason(message),
        });
    }
    let now = Instant::now();
    let mut active_sessions = 0_usize;
    let mut launching_sessions = 0_usize;
    let mut principal_sessions = 0_usize;
    for session in registry.sessions.values() {
        if session.scope != scope {
            continue;
        }
        if session.principal_id == principal_id {
            principal_sessions += 1;
        }
        match session.state {
            BrowserSessionState::Launching => launching_sessions += 1,
            BrowserSessionState::Active => active_sessions += 1,
        }
    }
    let total_sessions = active_sessions + launching_sessions;
    let launch_reconciliation_obligations = registry
        .pending_launch_reconciliations
        .values()
        .filter(|obligation| obligation.scope == scope)
        .count();
    let principal_launch_reconciliation_obligations = registry
        .pending_launch_reconciliations
        .values()
        .filter(|obligation| {
            obligation.scope == scope && obligation.reconciliation.principal_id == principal_id
        })
        .count();
    let engine_cleanup_obligations = registry
        .pending_engine_cleanups
        .values()
        .filter(|obligation| obligation.scope == scope)
        .count();
    let principal_engine_cleanup_obligations = registry
        .pending_engine_cleanups
        .values()
        .filter(|obligation| {
            obligation.scope == scope && obligation.cleanup.principal_id == principal_id
        })
        .count();
    let capacity_available =
        total_sessions + launch_reconciliation_obligations + engine_cleanup_obligations
            < limits.total
            && principal_sessions
                + principal_launch_reconciliation_obligations
                + principal_engine_cleanup_obligations
                < limits.per_principal;
    let sessions = registry
        .sessions
        .iter()
        .filter(|(_, session)| session.scope == scope)
        .map(|(session_id, session)| {
            browser_lifecycle_session_value(session_id, session, now, capacity_available)
        })
        .collect::<Vec<_>>();
    let recoverable_page = owner_launch_id
        .and_then(|owner_launch_id| {
            registry.sessions.values().find_map(|session| {
                (session.scope == scope
                    && session.principal_id == principal_id
                    && session.owner_launch_id == owner_launch_id
                    && session.state == BrowserSessionState::Active)
                    .then(|| browser_recoverable_active_page(session))
                    .flatten()
            })
        })
        .or_else(|| {
            browser_instance.and_then(|browser_instance| {
                let mut matches = registry.sessions.values().filter(|session| {
                    session.scope == scope
                        && session.principal_id == principal_id
                        && session.browser_instance.as_deref() == Some(browser_instance)
                        && session.state == BrowserSessionState::Active
                });
                let page = matches.next().and_then(browser_recoverable_active_page);
                (matches.next().is_none()).then_some(page).flatten()
            })
        })
        .or_else(|| {
            owner_launch_id.and_then(|owner_launch_id| {
                registry
                    .pending_engine_cleanups
                    .values()
                    .find(|obligation| {
                        obligation.scope == scope
                            && obligation.cleanup.principal_id == principal_id
                            && obligation.cleanup.owner_launch_id == owner_launch_id
                    })
                    .map(browser_recoverable_cleanup_page)
            })
        })
        .or_else(|| {
            browser_instance.and_then(|browser_instance| {
                let mut matches = registry
                    .pending_engine_cleanups
                    .values()
                    .filter(|obligation| {
                        obligation.scope == scope
                            && obligation.cleanup.principal_id == principal_id
                            && obligation.cleanup.browser_instance.as_deref()
                                == Some(browser_instance)
                    });
                let page = matches.next().map(browser_recoverable_cleanup_page);
                (matches.next().is_none()).then_some(page).flatten()
            })
        });
    serde_json::json!({
        "schema": "elastos.browser.session-capacity/v1",
        "status": "configured",
        "active_sessions": active_sessions,
        "launching_sessions": launching_sessions,
        "total_sessions": total_sessions,
        "principal_sessions": principal_sessions,
        "launch_reconciliation_obligations": launch_reconciliation_obligations,
        "engine_cleanup_obligations": engine_cleanup_obligations,
        "max_active_sessions": limits.total,
        "max_sessions_per_principal": limits.per_principal,
        "capacity_available": capacity_available,
        "recoverable_page": recoverable_page,
        "lifecycle": {
            "schema": "elastos.browser.lifecycle-status/v1",
            "owner": "runtime_gateway",
            "phases": BROWSER_LIFECYCLE_PHASES,
            "capacity_available": capacity_available,
            "sessions": sessions,
            "redaction": {
                "principal_id": "sha256-16",
                "owner_launch_id": "sha256-16",
                "session_id": "sha256-16",
                "page_id": "sha256-16",
                "profile_key": "sha256-16",
                "exit_id": "local-or-sha256-16",
                "vm_key": "sha256-16"
            }
        },
        "heartbeat": {
            "route": "/api/apps/browser/pages/:page_id/heartbeat",
            "stale_after_seconds": ACTIVE_HEARTBEAT_STALE_TTL.as_secs(),
            "ownership_ttl_seconds": serde_json::Value::Null,
        }
    })
}

pub(in crate::api::gateway) async fn browser_principal_has_live_sessions(
    data_dir: &Path,
    principal_id: &str,
) -> bool {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    if registry.load_durable_ownerships(data_dir).is_err() {
        return true;
    }
    registry
        .sessions
        .values()
        .any(|session| session.scope == scope && session.principal_id == principal_id)
        || registry
            .pending_launch_reconciliations
            .values()
            .any(|obligation| {
                obligation.scope == scope && obligation.reconciliation.principal_id == principal_id
            })
        || registry.pending_engine_cleanups.values().any(|obligation| {
            obligation.scope == scope && obligation.cleanup.principal_id == principal_id
        })
        || registry
            .pending_stream_cleanups
            .iter()
            .any(|(key, obligation)| {
                key.starts_with(&format!("{scope}\n"))
                    && obligation.cleanup.principal_id == principal_id
            })
}

pub(in crate::api::gateway) async fn take_stale_browser_pages(
    data_dir: &Path,
) -> Vec<BrowserPageCleanup> {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    if registry.load_durable_ownerships(data_dir).is_err() {
        return Vec::new();
    }
    registry.take_stale_active_pages(&scope, Instant::now())
}

pub(in crate::api::gateway) async fn claim_pending_browser_stream_cleanups(
    data_dir: &Path,
    limit: usize,
) -> Vec<BrowserStreamCleanup> {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    if registry.load_durable_ownerships(data_dir).is_err() {
        return Vec::new();
    }
    let prefix = format!("{scope}\n");
    registry
        .pending_stream_cleanups
        .iter_mut()
        .filter(|(key, obligation)| key.starts_with(&prefix) && !obligation.in_flight)
        .take(limit)
        .map(|(_, obligation)| {
            obligation.in_flight = true;
            obligation.cleanup.clone()
        })
        .collect()
}

#[cfg(test)]
pub(in crate::api::gateway) async fn browser_stream_cleanup_obligation_count(
    data_dir: &Path,
) -> usize {
    let scope = browser_session_scope(data_dir);
    let prefix = format!("{scope}\n");
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    registry
        .lock()
        .await
        .pending_stream_cleanups
        .keys()
        .filter(|key| key.starts_with(&prefix))
        .count()
}

fn browser_session_scope(data_dir: &Path) -> String {
    data_dir.to_string_lossy().into_owned()
}

fn browser_stream_cleanup_key(scope: &str, stream_id: &str) -> String {
    format!("{scope}\n{stream_id}")
}

fn browser_cleanup_handle(cleanup_id: &str) -> BrowserCleanupHandle {
    BrowserCleanupHandle {
        schema: "elastos.browser.cleanup-handle/v1".to_string(),
        id: cleanup_id.to_string(),
    }
}

fn browser_engine_cleanup(session: &BrowserSessionRecord) -> Option<BrowserEngineCleanup> {
    Some(BrowserEngineCleanup {
        cleanup_id: session.cleanup_id.clone(),
        page_id: session.page_id.clone()?,
        principal_id: session.principal_id.clone(),
        owner_launch_id: session.owner_launch_id.clone(),
        browser_instance: session.browser_instance.clone(),
        generation: session.generation.clone(),
        engine_route_provider: session.engine_route_provider.clone(),
        engine_provider: session.engine_provider.clone()?,
        engine_protocol_version: session.engine_protocol_version.clone()?,
        engine_adapter: session.engine_adapter.clone()?,
        engine: session.engine.clone()?,
        stream_id: session
            .provider_cleanup
            .as_ref()
            .and_then(|binding| binding.get("stream_id"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .unwrap_or_default(),
        transport_authority: session.transport_authority.clone(),
        provider_cleanup: session.provider_cleanup.clone()?,
    })
}

fn browser_engine_cleanup_key(scope: &str, cleanup: &BrowserEngineCleanup) -> String {
    format!("{scope}\n{}", cleanup.cleanup_id)
}

fn browser_launch_reconciliation_key(scope: &str, generation: &str) -> String {
    format!("{scope}\n{generation}")
}

fn browser_durable_ownership(session: &BrowserSessionRecord) -> Option<BrowserDurableOwnership> {
    Some(BrowserDurableOwnership {
        schema: BROWSER_DURABLE_OWNERSHIP_SCHEMA.to_string(),
        engine_cleanup: browser_engine_cleanup(session)?,
        stream_cleanup: session.stream_cleanup.clone(),
        browser_page: session.browser_page.clone()?,
    })
}

fn browser_lifecycle_root(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("Runtime").join("BrowserLifecycle")
}

fn browser_ownership_dir(data_dir: &Path) -> std::path::PathBuf {
    browser_lifecycle_root(data_dir).join("ownership")
}

fn browser_stream_cleanup_dir(data_dir: &Path) -> std::path::PathBuf {
    browser_lifecycle_root(data_dir).join("streams")
}

fn browser_launch_reconciliation_dir(data_dir: &Path) -> std::path::PathBuf {
    browser_lifecycle_root(data_dir).join("reconciliation")
}

fn browser_durable_file_name(value: &str) -> String {
    format!("{}.json", hex::encode(Sha256::digest(value.as_bytes())))
}

fn browser_ownership_path(data_dir: &Path, cleanup_id: &str) -> std::path::PathBuf {
    browser_ownership_dir(data_dir).join(browser_durable_file_name(cleanup_id))
}

fn browser_stream_cleanup_path(data_dir: &Path, stream_id: &str) -> std::path::PathBuf {
    browser_stream_cleanup_dir(data_dir).join(browser_durable_file_name(stream_id))
}

fn browser_launch_reconciliation_path(data_dir: &Path, cleanup_id: &str) -> std::path::PathBuf {
    browser_launch_reconciliation_dir(data_dir).join(browser_durable_file_name(cleanup_id))
}

#[cfg(unix)]
fn open_browser_dir_nofollow(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW);
    options.open(path)
}

#[cfg(not(unix))]
fn open_browser_dir_nofollow(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

#[cfg(unix)]
fn open_browser_file_nofollow(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    options.open(path)
}

#[cfg(not(unix))]
fn open_browser_file_nofollow(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

fn set_private_browser_dir(handle: &File) -> std::io::Result<()> {
    #[cfg(unix)]
    handle.set_permissions(std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn set_private_browser_file(handle: &File) -> std::io::Result<()> {
    #[cfg(unix)]
    handle.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn secure_browser_lifecycle_dir(
    data_dir: &Path,
    dir: &Path,
    create: bool,
) -> Result<Option<File>, String> {
    let relative = dir
        .strip_prefix(data_dir)
        .map_err(|_| "Browser lifecycle directory escaped the Runtime data root".to_string())?;
    let root_type = std::fs::symlink_metadata(data_dir)
        .map_err(|err| format!("Browser lifecycle data root is inaccessible: {err}"))?
        .file_type();
    if root_type.is_symlink() || !root_type.is_dir() {
        return Err("Browser lifecycle data root is not a regular directory".to_string());
    }
    let mut parent = open_browser_dir_nofollow(data_dir)
        .map_err(|err| format!("Browser lifecycle data root is inaccessible: {err}"))?;
    let mut current = data_dir.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            return Err("Browser lifecycle directory path is invalid".to_string());
        };
        current.push(component);
        let created = if create {
            match std::fs::create_dir(&current) {
                Ok(()) => true,
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => false,
                Err(err) => {
                    return Err(format!(
                        "Browser lifecycle directory could not be created: {err}"
                    ))
                }
            }
        } else {
            match std::fs::symlink_metadata(&current) {
                Ok(_) => false,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(err) => {
                    return Err(format!(
                        "Browser lifecycle directory is inaccessible: {err}"
                    ))
                }
            }
        };
        let child = open_browser_dir_nofollow(&current)
            .map_err(|err| format!("Browser lifecycle directory is unsafe: {err}"))?;
        if !child
            .metadata()
            .map_err(|err| format!("Browser lifecycle directory is inaccessible: {err}"))?
            .is_dir()
        {
            return Err("Browser lifecycle path contains a non-directory".to_string());
        }
        set_private_browser_dir(&child)
            .map_err(|err| format!("Browser lifecycle directory is not private: {err}"))?;
        child
            .sync_all()
            .map_err(|err| format!("Browser lifecycle directory could not be synced: {err}"))?;
        if created {
            parent.sync_all().map_err(|err| {
                format!("Browser lifecycle parent directory could not be synced: {err}")
            })?;
        }
        parent = child;
    }
    Ok(Some(parent))
}

fn create_browser_temp_file(parent: &Path, target: &Path) -> Result<(PathBuf, File), String> {
    let target_name = target
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "Browser lifecycle target name is invalid".to_string())?;
    for _ in 0..8 {
        let serial = BROWSER_DURABLE_TEMP_SERIAL.fetch_add(1, Ordering::Relaxed);
        let temp = parent.join(format!(
            ".{target_name}.{}.{}.tmp",
            std::process::id(),
            serial
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
        match options.open(&temp) {
            Ok(file) => return Ok((temp, file)),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(format!(
                    "Browser lifecycle temporary state could not be created: {err}"
                ))
            }
        }
    }
    Err("Browser lifecycle temporary state name could not be allocated".to_string())
}

fn write_browser_json_atomic<T: Serialize>(
    data_dir: &Path,
    path: &Path,
    value: &T,
    max_bytes: usize,
) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|err| format!("Browser lifecycle state could not be encoded: {err}"))?;
    if bytes.len() > max_bytes {
        return Err("Browser lifecycle state exceeded its bounded size".to_string());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "Browser lifecycle path has no parent".to_string())?;
    let parent_handle = secure_browser_lifecycle_dir(data_dir, parent, true)?
        .ok_or_else(|| "Browser lifecycle directory unavailable".to_string())?;
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_file() => {
            return Err("Browser lifecycle target is not a regular file".to_string())
        }
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(format!("Browser lifecycle target is inaccessible: {err}")),
    }
    let (temp, mut file) = create_browser_temp_file(parent, path)?;
    let write_result = (|| -> std::io::Result<()> {
        set_private_browser_file(&file)?;
        file.write_all(&bytes)?;
        file.sync_all()
    })();
    if let Err(err) = write_result {
        let _ = std::fs::remove_file(&temp);
        return Err(format!(
            "Browser lifecycle state could not be written: {err}"
        ));
    }
    drop(file);
    if let Err(err) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!(
            "Browser lifecycle state could not be committed: {err}"
        ));
    }
    parent_handle
        .sync_all()
        .map_err(|err| format!("Browser lifecycle state directory could not be synced: {err}"))?;
    Ok(())
}

fn write_browser_durable_ownership(
    data_dir: &Path,
    ownership: &BrowserDurableOwnership,
) -> Result<(), String> {
    if ownership.schema != BROWSER_DURABLE_OWNERSHIP_SCHEMA
        || !browser_engine_cleanup_is_safe(&ownership.engine_cleanup)
    {
        return Err("Browser durable ownership binding is invalid".to_string());
    }
    write_browser_json_atomic(
        data_dir,
        &browser_ownership_path(data_dir, &ownership.engine_cleanup.cleanup_id),
        ownership,
        MAX_BROWSER_DURABLE_OWNERSHIP_BYTES,
    )
}

fn write_browser_durable_stream_cleanup(
    data_dir: &Path,
    cleanup: &BrowserStreamCleanup,
) -> Result<(), String> {
    if cleanup.stream_id.is_empty()
        || cleanup.stream_id.len() > 256
        || !is_safe_runtime_id(&cleanup.stream_id)
        || cleanup.principal_id.is_empty()
        || cleanup.principal_id.len() > 512
    {
        return Err("Browser durable stream cleanup binding is invalid".to_string());
    }
    write_browser_json_atomic(
        data_dir,
        &browser_stream_cleanup_path(data_dir, &cleanup.stream_id),
        cleanup,
        4096,
    )
}

fn write_browser_launch_reconciliation(
    data_dir: &Path,
    reconciliation: &BrowserLaunchReconciliation,
) -> Result<(), String> {
    if !browser_launch_reconciliation_is_safe(reconciliation) {
        return Err("Browser launch reconciliation binding is invalid".to_string());
    }
    write_browser_json_atomic(
        data_dir,
        &browser_launch_reconciliation_path(data_dir, &reconciliation.cleanup_id),
        reconciliation,
        16 * 1024,
    )
}

fn remove_browser_durable_file(data_dir: &Path, path: PathBuf) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Browser lifecycle path has no parent".to_string())?;
    let Some(parent_handle) = secure_browser_lifecycle_dir(data_dir, parent, false)? else {
        return Ok(());
    };
    #[cfg(test)]
    if BROWSER_DURABLE_DELETE_FAILURES
        .get_or_init(Default::default)
        .lock()
        .expect("Browser durable delete test lock")
        .contains(&path)
    {
        return Err("Browser lifecycle terminal state deletion failed for test".to_string());
    }
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_file() => {
            return Err("Browser lifecycle terminal state is not a regular file".to_string())
        }
        Ok(_) => std::fs::remove_file(&path).map_err(|err| {
            format!("Browser lifecycle terminal state could not be committed: {err}")
        })?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(format!(
                "Browser lifecycle terminal state is inaccessible: {err}"
            ))
        }
    }
    parent_handle.sync_all().map_err(|err| {
        format!("Browser lifecycle terminal state directory could not be synced: {err}")
    })
}

#[cfg(test)]
fn set_browser_durable_delete_failure(path: &Path, fail: bool) {
    let failures = BROWSER_DURABLE_DELETE_FAILURES.get_or_init(Default::default);
    let mut failures = failures.lock().expect("Browser durable delete test lock");
    if fail {
        failures.insert(path.to_path_buf());
    } else {
        failures.remove(path);
    }
}

fn browser_engine_cleanup_is_safe(cleanup: &BrowserEngineCleanup) -> bool {
    let provider_cleanup = &cleanup.provider_cleanup;
    cleanup.cleanup_id.len() <= 128
        && cleanup.page_id.len() <= 256
        && cleanup.principal_id.len() <= 512
        && cleanup.owner_launch_id.len() <= 512
        && cleanup
            .browser_instance
            .as_deref()
            .is_none_or(|instance| browser_instance_id(Some(instance.to_string())).is_ok())
        && cleanup.generation.len() <= 256
        && cleanup.engine_route_provider.len() <= 256
        && cleanup.engine_provider.len() <= 256
        && cleanup.engine_protocol_version.len() <= 64
        && cleanup.engine_adapter.len() <= 256
        && cleanup.engine.len() <= 256
        && cleanup.stream_id.len() <= 256
        && cleanup
            .transport_authority
            .as_ref()
            .is_none_or(|authority| {
                validate_browser_vz_transport_authority(authority).is_ok()
                    && authority
                        .get("generation")
                        .and_then(serde_json::Value::as_str)
                        == Some(cleanup.generation.as_str())
                    && authority.get("page_id").and_then(serde_json::Value::as_str)
                        == Some(cleanup.page_id.as_str())
                    && authority
                        .pointer("/egress/stream_id")
                        .and_then(serde_json::Value::as_str)
                        == Some(cleanup.stream_id.as_str())
                    && provider_cleanup.get("transport_authority") == Some(authority)
            })
        && is_safe_runtime_id(&cleanup.cleanup_id)
        && is_safe_runtime_id(&cleanup.page_id)
        && is_safe_runtime_id(&cleanup.principal_id)
        && is_safe_runtime_id(&cleanup.owner_launch_id)
        && is_safe_runtime_id(&cleanup.generation)
        && is_safe_runtime_id(&cleanup.engine_route_provider)
        && is_safe_runtime_id(&cleanup.engine_provider)
        && cleanup.engine_provider == BROWSER_ENGINE_PROVIDER_ID
        && cleanup.engine_protocol_version == BROWSER_ENGINE_PROTOCOL_VERSION
        && is_safe_runtime_id(&cleanup.engine_adapter)
        && is_safe_runtime_id(&cleanup.engine)
        && (cleanup.stream_id.is_empty() || is_safe_runtime_id(&cleanup.stream_id))
        && provider_cleanup
            .get("schema")
            .and_then(|value| value.as_str())
            == Some(BROWSER_ENGINE_CLEANUP_BINDING_SCHEMA)
        && provider_cleanup
            .get("page_id")
            .and_then(|value| value.as_str())
            == Some(cleanup.page_id.as_str())
        && provider_cleanup
            .get("generation")
            .and_then(|value| value.as_str())
            == Some(cleanup.generation.as_str())
        && provider_cleanup
            .get("stream_id")
            .and_then(|value| value.as_str())
            == Some(cleanup.stream_id.as_str())
        && provider_cleanup
            .get("adapter")
            .and_then(|value| value.as_str())
            == Some(cleanup.engine_adapter.as_str())
        && provider_cleanup
            .get("engine")
            .and_then(|value| value.as_str())
            == Some(cleanup.engine.as_str())
        && browser_provider_cleanup_value_is_safe(provider_cleanup, 0)
        && serde_json::to_vec(provider_cleanup).is_ok_and(|bytes| bytes.len() <= 16 * 1024)
}

fn browser_launch_reconciliation_is_safe(reconciliation: &BrowserLaunchReconciliation) -> bool {
    reconciliation.schema == BROWSER_LAUNCH_RECONCILIATION_SCHEMA
        && reconciliation.cleanup_id.len() <= 128
        && reconciliation.principal_id.len() <= 512
        && reconciliation.owner_launch_id.len() <= 512
        && reconciliation
            .browser_instance
            .as_deref()
            .is_none_or(|instance| browser_instance_id(Some(instance.to_string())).is_ok())
        && reconciliation.generation.len() <= 256
        && reconciliation.engine_route_provider.len() <= 256
        && reconciliation
            .selected_engine_adapter
            .as_deref()
            .is_none_or(|adapter| adapter.len() <= 128 && is_safe_runtime_id(adapter))
        && reconciliation.stream_id.len() <= 256
        && is_safe_runtime_id(&reconciliation.cleanup_id)
        && is_safe_runtime_id(&reconciliation.principal_id)
        && is_safe_runtime_id(&reconciliation.owner_launch_id)
        && is_safe_runtime_id(&reconciliation.generation)
        && is_safe_runtime_id(&reconciliation.engine_route_provider)
        && is_safe_runtime_id(&reconciliation.stream_id)
        && reconciliation
            .transport_authority
            .as_ref()
            .is_none_or(|authority| {
                validate_browser_vz_transport_authority(authority).is_ok()
                    && authority
                        .get("generation")
                        .and_then(serde_json::Value::as_str)
                        == Some(reconciliation.generation.as_str())
                    && authority
                        .pointer("/egress/stream_id")
                        .and_then(serde_json::Value::as_str)
                        == Some(reconciliation.stream_id.as_str())
            })
        && reconciliation
            .stream_cleanup
            .as_ref()
            .is_none_or(|cleanup| {
                cleanup.stream_id == reconciliation.stream_id
                    && cleanup.principal_id == reconciliation.principal_id
            })
}

fn browser_session_limits() -> BrowserSessionLimits {
    let total = std::env::var("ELASTOS_BROWSER_MAX_ACTIVE_SESSIONS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_MAX_BROWSER_SESSIONS)
        .clamp(1, MAX_BROWSER_SESSIONS_LIMIT);
    let per_principal = std::env::var("ELASTOS_BROWSER_MAX_SESSIONS_PER_PRINCIPAL")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_MAX_BROWSER_SESSIONS_PER_PRINCIPAL)
        .clamp(1, total);
    BrowserSessionLimits {
        total,
        per_principal,
    }
}

impl BrowserSessionRegistry {
    fn load_durable_ownerships(&mut self, data_dir: &Path) -> Result<(), String> {
        let scope = browser_session_scope(data_dir);
        if self.loaded_scopes.contains(&scope) {
            return Ok(());
        }

        let ownerships = read_bounded_browser_json_dir::<BrowserDurableOwnership>(
            data_dir,
            &browser_ownership_dir(data_dir),
            MAX_BROWSER_DURABLE_OWNERSHIPS,
            MAX_BROWSER_DURABLE_OWNERSHIP_BYTES,
        )?;
        let stream_cleanups = read_bounded_browser_json_dir::<BrowserStreamCleanup>(
            data_dir,
            &browser_stream_cleanup_dir(data_dir),
            MAX_BROWSER_DURABLE_OWNERSHIPS,
            4096,
        )?;
        let launch_reconciliations = read_bounded_browser_json_dir::<BrowserLaunchReconciliation>(
            data_dir,
            &browser_launch_reconciliation_dir(data_dir),
            MAX_BROWSER_DURABLE_OWNERSHIPS,
            16 * 1024,
        )?;

        for ownership in ownerships {
            if ownership.schema != BROWSER_DURABLE_OWNERSHIP_SCHEMA
                || !browser_engine_cleanup_is_safe(&ownership.engine_cleanup)
                || ownership.stream_cleanup.as_ref().is_some_and(|stream| {
                    stream.stream_id != ownership.engine_cleanup.stream_id
                        || stream.principal_id != ownership.engine_cleanup.principal_id
                })
            {
                return Err("Browser durable ownership state is invalid".to_string());
            }
            let cleanup = ownership.engine_cleanup;
            if self
                .sessions
                .values()
                .any(|session| session.scope == scope && session.cleanup_id == cleanup.cleanup_id)
            {
                continue;
            }
            if let Some(stream) = ownership.stream_cleanup {
                let stream_key = browser_stream_cleanup_key(&scope, &stream.stream_id);
                self.pending_stream_cleanups.entry(stream_key).or_insert(
                    BrowserStreamCleanupObligation {
                        cleanup: stream,
                        in_flight: false,
                    },
                );
            }
            let key = browser_engine_cleanup_key(&scope, &cleanup);
            self.pending_engine_cleanups
                .entry(key)
                .or_insert(BrowserEngineCleanupObligation {
                    scope: scope.clone(),
                    cleanup,
                    browser_page: Some(ownership.browser_page),
                    in_flight: false,
                    attempts: 0,
                });
        }

        for stream in stream_cleanups {
            if stream.stream_id.is_empty()
                || stream.stream_id.len() > 256
                || !is_safe_runtime_id(&stream.stream_id)
                || stream.principal_id.is_empty()
                || stream.principal_id.len() > 512
            {
                return Err("Browser durable stream cleanup state is invalid".to_string());
            }
            let key = browser_stream_cleanup_key(&scope, &stream.stream_id);
            self.pending_stream_cleanups
                .entry(key)
                .or_insert(BrowserStreamCleanupObligation {
                    cleanup: stream,
                    in_flight: false,
                });
        }
        for reconciliation in launch_reconciliations {
            if !browser_launch_reconciliation_is_safe(&reconciliation) {
                return Err("Browser durable launch reconciliation state is invalid".to_string());
            }
            let key = browser_launch_reconciliation_key(&scope, &reconciliation.generation);
            self.pending_launch_reconciliations.entry(key).or_insert(
                BrowserLaunchReconciliationObligation {
                    scope: scope.clone(),
                    reconciliation,
                    in_flight: false,
                },
            );
        }
        self.loaded_scopes.insert(scope);
        Ok(())
    }

    fn next_reservation_id(&mut self) -> String {
        self.serial = self.serial.saturating_add(1);
        format!("browser-launch:{:016x}", self.serial)
    }

    fn take_stale_active_pages(&mut self, scope: &str, now: Instant) -> Vec<BrowserPageCleanup> {
        let stale_keys: Vec<_> = self
            .sessions
            .iter()
            .filter(|(_, session)| {
                session.scope == scope
                    && session.state == BrowserSessionState::Active
                    && now.duration_since(session.last_seen_at) > ACTIVE_HEARTBEAT_STALE_TTL
            })
            .map(|(key, _)| key.clone())
            .collect();
        stale_keys
            .into_iter()
            .filter_map(|key| {
                self.sessions.remove(&key).and_then(|session| {
                    let engine_cleanup = browser_engine_cleanup(&session)?;
                    Some(BrowserPageCleanup {
                        engine_cleanup,
                        stream_cleanup: session.stream_cleanup,
                        active_session: false,
                    })
                })
            })
            .collect()
    }
}

fn read_bounded_browser_json_dir<T: for<'de> Deserialize<'de>>(
    data_dir: &Path,
    dir: &Path,
    max_entries: usize,
    max_bytes: usize,
) -> Result<Vec<T>, String> {
    let Some(_dir_handle) = secure_browser_lifecycle_dir(data_dir, dir, false)? else {
        return Ok(Vec::new());
    };
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            return Err(format!(
                "Browser lifecycle state directory is inaccessible: {err}"
            ))
        }
    };
    let mut paths = Vec::new();
    let mut entry_count = 0_usize;
    for entry in entries {
        entry_count = entry_count.saturating_add(1);
        if entry_count > max_entries {
            return Err("Browser lifecycle state exceeded its bounded record count".to_string());
        }
        let entry =
            entry.map_err(|err| format!("Browser lifecycle state entry is inaccessible: {err}"))?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let file_type = entry
            .file_type()
            .map_err(|err| format!("Browser lifecycle state type is inaccessible: {err}"))?;
        if !file_type.is_file() || file_type.is_symlink() {
            return Err("Browser lifecycle state contains a non-regular record".to_string());
        }
        paths.push(path);
    }
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let file = open_browser_file_nofollow(&path)
                .map_err(|err| format!("Browser lifecycle state is inaccessible: {err}"))?;
            let metadata = file
                .metadata()
                .map_err(|err| format!("Browser lifecycle state is inaccessible: {err}"))?;
            if !metadata.is_file() {
                return Err("Browser lifecycle state contains a non-regular record".to_string());
            }
            if metadata.len() > max_bytes as u64 {
                return Err("Browser lifecycle state exceeded its bounded size".to_string());
            }
            set_private_browser_file(&file)
                .map_err(|err| format!("Browser lifecycle state is not private: {err}"))?;
            file.sync_all()
                .map_err(|err| format!("Browser lifecycle state could not be synced: {err}"))?;
            let mut bytes = Vec::with_capacity(max_bytes.min(8192));
            file.take(max_bytes as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|err| format!("Browser lifecycle state is inaccessible: {err}"))?;
            if bytes.len() > max_bytes {
                return Err("Browser lifecycle state exceeded its bounded size".to_string());
            }
            serde_json::from_slice(&bytes)
                .map_err(|err| format!("Browser lifecycle state is invalid: {err}"))
        })
        .collect()
}

fn browser_recoverable_active_page(session: &BrowserSessionRecord) -> Option<serde_json::Value> {
    Some(serde_json::json!({
        "schema": "elastos.browser.recoverable-page/v1",
        "state": "active",
        "page_id": session.page_id.as_deref()?,
        "cleanup": browser_cleanup_handle(&session.cleanup_id),
        "engine_page": session.browser_page.as_ref()?,
    }))
}

fn browser_recoverable_cleanup_page(
    obligation: &BrowserEngineCleanupObligation,
) -> serde_json::Value {
    serde_json::json!({
        "schema": "elastos.browser.recoverable-page/v1",
        "state": "cleanup_pending",
        "page_id": obligation.cleanup.page_id,
        "cleanup": browser_cleanup_handle(&obligation.cleanup.cleanup_id),
        "engine_page": obligation.browser_page,
    })
}

impl BrowserLifecyclePhase {
    fn as_str(self) -> &'static str {
        match self {
            Self::AcquiringSlot => "ACQUIRING_SLOT",
            Self::PreparingImage => "PREPARING_IMAGE",
            Self::StartingVm => "STARTING_VM",
            Self::ActiveSession => "ACTIVE_SESSION",
            Self::Navigating => "NAVIGATING",
            Self::Retiring => "RETIRING",
            Self::Failed => "FAILED",
        }
    }

    fn pending_launch_age_ms(self, started_at: Instant, now: Instant) -> Option<u128> {
        matches!(
            self,
            Self::AcquiringSlot | Self::PreparingImage | Self::StartingVm
        )
        .then(|| now.duration_since(started_at).as_millis())
    }

    fn is_warm_vm(self) -> bool {
        false
    }
}

fn browser_lifecycle_session_value(
    session_id: &str,
    session: &BrowserSessionRecord,
    now: Instant,
    capacity_available: bool,
) -> serde_json::Value {
    serde_json::json!({
        "session_id": hash_label(session_id),
        "owner_launch_id": hash_label(&session.owner_launch_id),
        "page_id": session.page_id.as_deref().map(hash_label),
        "principal_id": hash_label(&session.principal_id),
        "profile_key_hash": session.profile_key_hash,
        "exit_id": session.exit_id,
        "url": session.url,
        "phase": session.phase.as_str(),
        "started_at": system_time_ms(session.started_at),
        "age_ms": now.duration_since(session.created_at).as_millis(),
        "last_navigation_at": session.last_navigation_at.and_then(system_time_ms),
        "last_frame_at": session.last_frame_at.and_then(system_time_ms),
        "pending_launch_age_ms": session.phase.pending_launch_age_ms(session.created_at, now),
        "vm_key_hash": session.vm_key_hash,
        "warm_vm": session.phase.is_warm_vm(),
        "capacity_available": capacity_available,
        "failure_reason": session.failure_reason,
    })
}

fn system_time_ms(value: SystemTime) -> Option<u128> {
    value
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis())
}

pub(in crate::api::gateway) fn browser_lifecycle_hash(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    Some(hash_label(value))
}

pub(in crate::api::gateway) fn browser_lifecycle_exit_id(value: Option<&str>) -> String {
    value
        .and_then(browser_lifecycle_hash)
        .map(|hash| format!("remote-carrier:{hash}"))
        .unwrap_or_else(|| "local-runtime".to_string())
}

pub(in crate::api::gateway) fn browser_lifecycle_vm_key_hash(parts: &[&str]) -> Option<String> {
    let joined = parts
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    browser_lifecycle_hash(&joined)
}

fn browser_launch_generation_hash_label(value: &str) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(value.as_bytes())))
}

fn hash_label(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    format!("sha256:{}", hex::encode(&digest[..8]))
}

fn sanitize_browser_lifecycle_url(value: &str) -> String {
    match url::Url::parse(value) {
        Ok(mut parsed) => {
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            parsed.set_fragment(None);
            parsed.to_string()
        }
        Err(_) => String::new(),
    }
}

fn sanitize_failure_reason(value: String) -> String {
    let mut text = value.replace(['\r', '\n', '\0'], " ");
    if text.len() > 240 {
        text.truncate(240);
    }
    text
}

impl BrowserOpenJobRegistry {
    fn next_job_id(&mut self) -> String {
        self.serial = self.serial.saturating_add(1);
        format!("browser-open:{:016x}", self.serial)
    }

    fn purge_expired(&mut self, now: Instant) {
        self.jobs
            .retain(|_, job| now.duration_since(job.updated_at) <= OPEN_JOB_TTL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_session_record(
        scope: &str,
        page_id: Option<&str>,
        state: BrowserSessionState,
        created_at: Instant,
        last_seen_at: Instant,
    ) -> BrowserSessionRecord {
        BrowserSessionRecord {
            scope: scope.to_string(),
            principal_id: "person:local:test".to_string(),
            owner_launch_id: "launch:test".to_string(),
            browser_instance: None,
            cleanup_id: format!("browser-cleanup:{}", page_id.unwrap_or("launching")),
            generation: "sha256:generation".to_string(),
            expected_page_id: "page:vz-test".to_string(),
            vm_id: "browser-vm-test".to_string(),
            page_id: page_id.map(str::to_string),
            engine_route_provider: "mock-browser-route".to_string(),
            selected_engine_adapter: Some("mock-adapter".to_string()),
            engine_provider: page_id.map(|_| "mock-browser-engine".to_string()),
            engine_protocol_version: page_id.map(|_| BROWSER_ENGINE_PROTOCOL_VERSION.to_string()),
            engine_adapter: page_id.map(|_| "mock-adapter".to_string()),
            engine: page_id.map(|_| "mock-engine".to_string()),
            provider_cleanup: page_id.map(|page_id| {
                serde_json::json!({
                    "schema": BROWSER_ENGINE_CLEANUP_BINDING_SCHEMA,
                    "page_id": page_id,
                    "generation": "sha256:generation",
                    "stream_id": "stream:test",
                    "adapter": "mock-adapter",
                    "engine": "mock-engine",
                })
            }),
            browser_page: page_id.map(|page_id| serde_json::json!({"page_id": page_id})),
            stream_cleanup: None,
            transport_authority: None,
            state,
            phase: match state {
                BrowserSessionState::Launching => BrowserLifecyclePhase::AcquiringSlot,
                BrowserSessionState::Active => BrowserLifecyclePhase::ActiveSession,
            },
            url: "https://example.com/".to_string(),
            exit_id: "local-runtime".to_string(),
            profile_key_hash: Some("sha256:profilehash".to_string()),
            vm_key_hash: Some("sha256:vmhash".to_string()),
            created_at,
            last_seen_at,
            started_at: UNIX_EPOCH,
            last_navigation_at: None,
            last_frame_at: None,
            failure_reason: None,
        }
    }

    fn test_lifecycle(owner_launch_id: &str) -> BrowserLaunchLifecycle {
        BrowserLaunchLifecycle {
            owner_launch_id: owner_launch_id.to_string(),
            browser_instance: None,
            url: "https://example.com/".to_string(),
            exit_id: "local-runtime".to_string(),
            engine_route_provider: "mock-browser-route".to_string(),
            selected_engine_adapter: Some("mock-adapter".to_string()),
            profile_key_hash: Some("sha256:profilehash".to_string()),
            vm_key_hash: Some("sha256:vmhash".to_string()),
        }
    }

    fn test_durable_launch_effect(
        reservation: &BrowserLaunchReservation,
        page_id: &str,
        stream_id: &str,
        stream_cleanup: Option<BrowserStreamCleanup>,
    ) -> BrowserLaunchEffect {
        BrowserLaunchEffect {
            page_id: page_id.to_string(),
            engine_provider: BROWSER_ENGINE_PROVIDER_ID.to_string(),
            engine_protocol_version: BROWSER_ENGINE_PROTOCOL_VERSION.to_string(),
            engine_adapter: "mock-adapter".to_string(),
            engine: "mock-engine".to_string(),
            provider_cleanup: serde_json::json!({
                "schema": BROWSER_ENGINE_CLEANUP_BINDING_SCHEMA,
                "page_id": page_id,
                "generation": reservation.generation(),
                "stream_id": stream_id,
                "adapter": "mock-adapter",
                "engine": "mock-engine",
            }),
            browser_page: serde_json::json!({
                "schema": "elastos.browser.engine.page/v1",
                "page_id": page_id,
            }),
            stream_cleanup,
        }
    }

    #[tokio::test]
    async fn status_reads_preserve_90_second_launch_and_four_hour_active_owner() {
        let dir = tempfile::tempdir().unwrap();
        let scope = browser_session_scope(dir.path());
        let now = Instant::now();
        let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
        let mut registry = registry.lock().await;
        registry.loaded_scopes.insert(scope.clone());
        registry.sessions.insert(
            "slow-launch".to_string(),
            test_session_record(
                &scope,
                None,
                BrowserSessionState::Launching,
                now - Duration::from_secs(91),
                now - Duration::from_secs(91),
            ),
        );
        registry.sessions.insert(
            "sleeping-active".to_string(),
            test_session_record(
                &scope,
                Some("page:test"),
                BrowserSessionState::Active,
                now - Duration::from_secs(4 * 60 * 60),
                now - Duration::from_secs(4 * 60 * 60),
            ),
        );
        drop(registry);

        let status =
            browser_gateway_session_status(dir.path(), "person:local:test", None, None).await;

        assert_eq!(status["launching_sessions"], 1);
        assert_eq!(status["active_sessions"], 1);
        let mut registry = BROWSER_SESSION_REGISTRY
            .get()
            .expect("browser registry")
            .lock()
            .await;
        assert!(registry.sessions.contains_key("slow-launch"));
        assert!(registry.sessions.contains_key("sleeping-active"));
        registry.sessions.remove("slow-launch");
        registry.sessions.remove("sleeping-active");
        registry.loaded_scopes.remove(&scope);
    }

    #[test]
    fn browser_registry_counts_only_the_requested_scope() {
        let mut registry = BrowserSessionRegistry::default();
        let now = Instant::now();
        registry.sessions.insert(
            "scope-a".to_string(),
            test_session_record(
                "/tmp/elastos-a",
                Some("page:a"),
                BrowserSessionState::Active,
                now,
                now,
            ),
        );
        registry.sessions.insert(
            "scope-b".to_string(),
            test_session_record(
                "/tmp/elastos-b",
                Some("page:b"),
                BrowserSessionState::Active,
                now,
                now,
            ),
        );

        let scope_a_count = registry
            .sessions
            .values()
            .filter(|session| session.scope == "/tmp/elastos-a")
            .count();

        assert_eq!(scope_a_count, 1);
    }

    #[test]
    fn browser_registry_takes_stale_active_pages_for_cleanup() {
        let mut registry = BrowserSessionRegistry::default();
        let now = Instant::now();
        let mut stale = test_session_record(
            "/tmp/elastos-test-a",
            Some("page:stale"),
            BrowserSessionState::Active,
            now - ACTIVE_HEARTBEAT_STALE_TTL - Duration::from_secs(5),
            now - ACTIVE_HEARTBEAT_STALE_TTL - Duration::from_secs(5),
        );
        stale.stream_cleanup = Some(BrowserStreamCleanup {
            stream_id: "remote-carrier:stale".to_string(),
            principal_id: "person:local:test".to_string(),
        });
        registry.sessions.insert("stale-active".to_string(), stale);
        registry.sessions.insert(
            "fresh-active".to_string(),
            test_session_record(
                "/tmp/elastos-test-a",
                Some("page:fresh"),
                BrowserSessionState::Active,
                now,
                now,
            ),
        );
        registry.sessions.insert(
            "other-scope".to_string(),
            test_session_record(
                "/tmp/elastos-test-b",
                Some("page:other"),
                BrowserSessionState::Active,
                now - ACTIVE_HEARTBEAT_STALE_TTL - Duration::from_secs(5),
                now - ACTIVE_HEARTBEAT_STALE_TTL - Duration::from_secs(5),
            ),
        );

        let stale = registry.take_stale_active_pages("/tmp/elastos-test-a", now);

        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].engine_cleanup.page_id, "page:stale");
        assert_eq!(
            stale[0]
                .stream_cleanup
                .as_ref()
                .map(|cleanup| cleanup.stream_id.as_str()),
            Some("remote-carrier:stale")
        );
        assert!(!registry.sessions.contains_key("stale-active"));
        assert!(registry.sessions.contains_key("fresh-active"));
        assert!(registry.sessions.contains_key("other-scope"));
    }

    #[tokio::test]
    async fn browser_open_jobs_coalesce_matching_pending_and_completed_owner_intent() {
        let dir = tempfile::tempdir().unwrap();
        let first = create_browser_open_job(
            dir.path(),
            "person:local:test",
            "launch:owner-a",
            "intent:a",
        )
        .await
        .unwrap();
        assert!(first.should_spawn);

        let duplicate = create_browser_open_job(
            dir.path(),
            "person:local:test",
            "launch:owner-a",
            "intent:a",
        )
        .await
        .unwrap();
        assert!(!duplicate.should_spawn);
        assert_eq!(duplicate.handle.id, first.handle.id);

        let other_owner = create_browser_open_job(
            dir.path(),
            "person:local:test",
            "launch:owner-b",
            "intent:a",
        )
        .await
        .unwrap();
        assert!(other_owner.should_spawn);
        assert_ne!(other_owner.handle.id, first.handle.id);

        let conflict = create_browser_open_job(
            dir.path(),
            "person:local:test",
            "launch:owner-a",
            "intent:b",
        )
        .await
        .unwrap_err();
        assert_eq!(conflict.0, StatusCode::CONFLICT);

        complete_browser_open_job(&first.handle, serde_json::json!({"ok": true})).await;
        let replay = create_browser_open_job(
            dir.path(),
            "person:local:test",
            "launch:owner-a",
            "intent:a",
        )
        .await
        .unwrap();
        assert!(!replay.should_spawn);
        assert_eq!(replay.handle.id, first.handle.id);
        assert!(matches!(
            browser_open_job_for_owner(
                dir.path(),
                &first.handle.id,
                "person:local:test",
                "launch:owner-a",
            )
            .await,
            Some(BrowserOpenJobSnapshot::Completed(_))
        ));
        assert!(browser_open_job_for_owner(
            dir.path(),
            &first.handle.id,
            "person:local:test",
            "launch:owner-b",
        )
        .await
        .is_none());

        let completed_conflict = create_browser_open_job(
            dir.path(),
            "person:local:test",
            "launch:owner-a",
            "intent:b",
        )
        .await
        .unwrap_err();
        assert_eq!(completed_conflict.0, StatusCode::CONFLICT);

        let failed = create_browser_open_job(
            dir.path(),
            "person:local:test",
            "launch:owner-c",
            "intent:failed",
        )
        .await
        .unwrap();
        fail_browser_open_job(
            &failed.handle,
            serde_json::json!({"error": "simulated terminal failure"}),
        )
        .await;
        let retry = create_browser_open_job(
            dir.path(),
            "person:local:test",
            "launch:owner-c",
            "intent:retry",
        )
        .await
        .unwrap();
        assert!(retry.should_spawn);
        assert_ne!(retry.handle.id, failed.handle.id);
    }

    #[tokio::test]
    async fn browser_lifecycle_replay_and_capacity_conflict_preserve_healthy_owner() {
        let dir = tempfile::tempdir().unwrap();
        let principal_id = "person:local:test";
        let first =
            reserve_browser_launch(dir.path(), principal_id, test_lifecycle("launch:owner-a"))
                .await
                .unwrap();
        complete_browser_launch(
            dir.path(),
            &first,
            BrowserLaunchEffect {
                page_id: "page:healthy".to_string(),
                engine_provider: "browser-engine-adapter".to_string(),
                engine_protocol_version: BROWSER_ENGINE_PROTOCOL_VERSION.to_string(),
                engine_adapter: "mock-adapter".to_string(),
                engine: "mock-engine".to_string(),
                provider_cleanup: serde_json::json!({
                    "schema": BROWSER_ENGINE_CLEANUP_BINDING_SCHEMA,
                    "page_id": "page:healthy",
                    "generation": first.generation(),
                    "stream_id": "stream:healthy",
                    "adapter": "mock-adapter",
                    "engine": "mock-engine",
                }),
                browser_page: serde_json::json!({"page_id": "page:healthy"}),
                stream_cleanup: None,
            },
        )
        .await
        .unwrap();

        let replay =
            reserve_browser_launch(dir.path(), principal_id, test_lifecycle("launch:owner-a"))
                .await
                .unwrap_err();
        assert_eq!(replay.0, StatusCode::CONFLICT);

        let limits = browser_session_limits();
        for index in 1..limits.total {
            reserve_browser_launch(
                dir.path(),
                &format!("person:local:test-{index}"),
                test_lifecycle(&format!("launch:owner-{index}")),
            )
            .await
            .unwrap();
        }
        let capacity = reserve_browser_launch(
            dir.path(),
            "person:local:overflow",
            test_lifecycle("launch:overflow"),
        )
        .await
        .unwrap_err();
        assert_eq!(capacity.0, StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            touch_browser_page(dir.path(), "page:healthy", principal_id, "launch:owner-a",).await
        );

        assert!(release_browser_page_for_principal(
            dir.path(),
            "page:healthy",
            principal_id,
            "launch:owner-a",
        )
        .await
        .is_none());
        let next =
            reserve_browser_launch(dir.path(), principal_id, test_lifecycle("launch:owner-a"))
                .await
                .unwrap();
        release_browser_launch(&next).await;
    }

    #[cfg(unix)]
    #[test]
    fn lifecycle_journal_uses_private_directory_and_file_permissions() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let cleanup = BrowserStreamCleanup {
            stream_id: "stream:private-permissions".to_string(),
            principal_id: "person:local:test".to_string(),
        };

        write_browser_durable_stream_cleanup(dir.path(), &cleanup).unwrap();

        for path in [
            dir.path().join("Runtime"),
            browser_lifecycle_root(dir.path()),
            browser_stream_cleanup_dir(dir.path()),
        ] {
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        assert_eq!(
            std::fs::metadata(browser_stream_cleanup_path(dir.path(), &cleanup.stream_id,))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn lifecycle_journal_refuses_symlinked_write_directory() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(browser_lifecycle_root(dir.path())).unwrap();
        symlink(outside.path(), browser_stream_cleanup_dir(dir.path())).unwrap();
        let cleanup = BrowserStreamCleanup {
            stream_id: "stream:no-follow".to_string(),
            principal_id: "person:local:test".to_string(),
        };

        let error = write_browser_durable_stream_cleanup(dir.path(), &cleanup).unwrap_err();

        assert!(error.contains("unsafe"));
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn engine_delete_failure_retains_retryable_in_memory_obligation() {
        let dir = tempfile::tempdir().unwrap();
        let principal_id = "person:local:engine-delete";
        let owner_launch_id = "launch:engine-delete";
        let reservation =
            reserve_browser_launch(dir.path(), principal_id, test_lifecycle(owner_launch_id))
                .await
                .unwrap();
        let handle = complete_browser_launch(
            dir.path(),
            &reservation,
            test_durable_launch_effect(
                &reservation,
                "page:engine-delete",
                "stream:engine-delete",
                None,
            ),
        )
        .await
        .unwrap();
        let cleanup = browser_page_cleanup_for_principal(
            dir.path(),
            "page:engine-delete",
            principal_id,
            owner_launch_id,
            &handle.id,
        )
        .await
        .unwrap()
        .unwrap()
        .engine_cleanup;
        record_browser_engine_cleanup_obligation(dir.path(), cleanup.clone())
            .await
            .unwrap();
        let ownership_path = browser_ownership_path(dir.path(), &cleanup.cleanup_id);
        set_browser_durable_delete_failure(&ownership_path, true);

        assert!(
            forget_browser_engine_cleanup_obligation(dir.path(), &cleanup)
                .await
                .is_err()
        );
        assert_eq!(browser_engine_cleanup_obligation_count(dir.path()).await, 1);
        let retry = claim_pending_browser_engine_cleanups(dir.path(), usize::MAX).await;
        assert_eq!(retry.len(), 1);
        assert_eq!(retry[0], cleanup);

        release_browser_engine_cleanup_claim(dir.path(), &cleanup).await;
        set_browser_durable_delete_failure(&ownership_path, false);
        forget_browser_engine_cleanup_obligation(dir.path(), &cleanup)
            .await
            .unwrap();
        assert_eq!(browser_engine_cleanup_obligation_count(dir.path()).await, 0);
        assert!(!ownership_path.exists());
        let _ = release_browser_page_for_principal(
            dir.path(),
            "page:engine-delete",
            principal_id,
            owner_launch_id,
        )
        .await;
    }

    #[tokio::test]
    async fn stream_delete_failure_retains_retryable_in_memory_obligation() {
        let dir = tempfile::tempdir().unwrap();
        let cleanup = BrowserStreamCleanup {
            stream_id: "stream:delete-failure".to_string(),
            principal_id: "person:local:stream-delete".to_string(),
        };
        write_browser_durable_stream_cleanup(dir.path(), &cleanup).unwrap();
        let cleanup_path = browser_stream_cleanup_path(dir.path(), &cleanup.stream_id);
        set_browser_durable_delete_failure(&cleanup_path, true);

        assert!(forget_browser_stream_cleanup_failure(dir.path(), &cleanup)
            .await
            .is_err());
        assert_eq!(browser_stream_cleanup_obligation_count(dir.path()).await, 1);
        assert_eq!(
            claim_pending_browser_stream_cleanups(dir.path(), usize::MAX).await,
            vec![cleanup.clone()]
        );

        set_browser_durable_delete_failure(&cleanup_path, false);
        forget_browser_stream_cleanup_failure(dir.path(), &cleanup)
            .await
            .unwrap();
        assert_eq!(browser_stream_cleanup_obligation_count(dir.path()).await, 0);
        assert!(!cleanup_path.exists());
    }

    #[tokio::test]
    async fn runtime_restart_recovers_exact_durable_cleanup_ownership_by_browser_instance() {
        let dir = tempfile::tempdir().unwrap();
        let principal_id = "person:local:restart";
        let owner_launch_id = "launch:restart";
        let refreshed_owner_launch_id = "launch:restart-refreshed";
        let browser_instance = "browser:0123456789abcdef0123456789abcdef";
        let mut lifecycle = test_lifecycle(owner_launch_id);
        lifecycle.browser_instance = Some(browser_instance.to_string());
        let reservation = reserve_browser_launch(dir.path(), principal_id, lifecycle)
            .await
            .unwrap();
        let cleanup = complete_browser_launch(
            dir.path(),
            &reservation,
            BrowserLaunchEffect {
                page_id: "page:restart".to_string(),
                engine_provider: "browser-engine-adapter".to_string(),
                engine_protocol_version: BROWSER_ENGINE_PROTOCOL_VERSION.to_string(),
                engine_adapter: "mock-adapter".to_string(),
                engine: "mock-engine".to_string(),
                provider_cleanup: serde_json::json!({
                    "schema": BROWSER_ENGINE_CLEANUP_BINDING_SCHEMA,
                    "page_id": "page:restart",
                    "generation": reservation.generation(),
                    "stream_id": "stream:restart",
                    "adapter": "mock-adapter",
                    "engine": "mock-engine",
                }),
                browser_page: serde_json::json!({
                    "schema": "elastos.browser.engine.page/v1",
                    "page_id": "page:restart",
                }),
                stream_cleanup: None,
            },
        )
        .await
        .unwrap();

        let scope = browser_session_scope(dir.path());
        let registry = BROWSER_SESSION_REGISTRY.get().expect("browser registry");
        let mut registry = registry.lock().await;
        registry
            .sessions
            .retain(|_, session| session.scope != scope);
        registry
            .pending_engine_cleanups
            .retain(|_, obligation| obligation.scope != scope);
        registry
            .pending_stream_cleanups
            .retain(|key, _| !key.starts_with(&format!("{scope}\n")));
        registry.loaded_scopes.remove(&scope);
        drop(registry);

        let foreign = browser_gateway_session_status(
            dir.path(),
            principal_id,
            Some(refreshed_owner_launch_id),
            Some("browser:fedcba9876543210fedcba9876543210"),
        )
        .await;
        assert!(foreign["recoverable_page"].is_null());

        let status = browser_gateway_session_status(
            dir.path(),
            principal_id,
            Some(refreshed_owner_launch_id),
            Some(browser_instance),
        )
        .await;
        assert_eq!(status["active_sessions"], 0);
        assert_eq!(status["engine_cleanup_obligations"], 1);
        assert_eq!(status["recoverable_page"]["state"], "cleanup_pending");
        assert_eq!(status["recoverable_page"]["cleanup"]["id"], cleanup.id);
        assert_eq!(
            status["recoverable_page"]["engine_page"]["page_id"],
            "page:restart"
        );

        let recovered = browser_page_cleanup_for_principal(
            dir.path(),
            "page:restart",
            principal_id,
            refreshed_owner_launch_id,
            &cleanup.id,
        )
        .await
        .unwrap()
        .expect("durable cleanup obligation");
        assert!(!recovered.active_session);
        assert_eq!(
            recovered.engine_cleanup.generation,
            reservation.generation()
        );
        assert_eq!(recovered.engine_cleanup.owner_launch_id, owner_launch_id);
        assert_eq!(
            recovered.engine_cleanup.browser_instance.as_deref(),
            Some(browser_instance)
        );
        assert!(browser_page_cleanup_for_principal(
            dir.path(),
            "page:restart",
            principal_id,
            refreshed_owner_launch_id,
            "browser-cleanup:substituted",
        )
        .await
        .unwrap()
        .is_none());
    }

    #[tokio::test]
    async fn runtime_restart_recovers_indeterminate_launch_and_blocks_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let principal_id = "person:local:launch-reconciliation-restart";
        let browser_instance = "browser:00112233445566778899aabbccddeeff";
        let mut lifecycle = test_lifecycle("launch:reconciliation-restart");
        lifecycle.browser_instance = Some(browser_instance.to_string());
        let reservation = reserve_browser_launch(dir.path(), principal_id, lifecycle)
            .await
            .unwrap();
        let stream_cleanup = BrowserStreamCleanup {
            stream_id: "stream:reconciliation-restart".to_string(),
            principal_id: principal_id.to_string(),
        };
        record_browser_launch_reconciliation_obligation(
            dir.path(),
            &reservation,
            &stream_cleanup.stream_id,
            Some(stream_cleanup.clone()),
        )
        .await
        .unwrap();
        let reconciliation_path =
            browser_launch_reconciliation_path(dir.path(), reservation.cleanup_id());
        assert!(reconciliation_path.exists());
        let durable_reconciliation: BrowserLaunchReconciliation = serde_json::from_slice(
            &std::fs::read(&reconciliation_path).expect("durable launch reconciliation"),
        )
        .unwrap();
        assert_eq!(
            durable_reconciliation.selected_engine_adapter.as_deref(),
            Some("mock-adapter")
        );
        assert_eq!(
            durable_reconciliation.browser_instance.as_deref(),
            Some(browser_instance)
        );
        assert_eq!(browser_page_session_count(dir.path()).await, 0);

        let scope = browser_session_scope(dir.path());
        let registry = BROWSER_SESSION_REGISTRY.get().expect("browser registry");
        let mut registry = registry.lock().await;
        registry
            .pending_launch_reconciliations
            .retain(|_, obligation| obligation.scope != scope);
        registry.loaded_scopes.remove(&scope);
        drop(registry);

        let status = browser_gateway_session_status(
            dir.path(),
            principal_id,
            Some("launch:reconciliation-restart"),
            None,
        )
        .await;
        assert_eq!(status["launch_reconciliation_obligations"], 1);
        let blocked = reserve_browser_launch(
            dir.path(),
            principal_id,
            test_lifecycle("launch:replacement"),
        )
        .await
        .unwrap_err();
        assert_eq!(blocked.0, StatusCode::SERVICE_UNAVAILABLE);
        assert!(blocked.1.contains("cleanup is pending"));

        let recovered = claim_pending_browser_launch_reconciliations(dir.path(), usize::MAX).await;
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].generation, reservation.generation());
        assert_eq!(recovered[0].stream_id, stream_cleanup.stream_id);
        assert_eq!(
            recovered[0].selected_engine_adapter.as_deref(),
            Some("mock-adapter")
        );
        assert_eq!(
            recovered[0].browser_instance.as_deref(),
            Some(browser_instance)
        );
        forget_browser_launch_reconciliation_obligation(dir.path(), &recovered[0])
            .await
            .unwrap();
        assert!(!reconciliation_path.exists());
    }
}
