//! Gateway-local Browser session accounting.
//!
//! This is not the final distributed Browser Session Manager, but it closes the
//! product hole where a launch-in-progress or dead page can make the Browser
//! look permanently busy. The Runtime gateway now accounts for launching and
//! active Browser pages before invoking the heavy engine supervisor.

use super::*;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_MAX_BROWSER_SESSIONS: usize = 4;
const DEFAULT_MAX_BROWSER_SESSIONS_PER_PRINCIPAL: usize = 4;
const MAX_BROWSER_SESSIONS_LIMIT: usize = 32;
const LAUNCH_RESERVATION_TTL: Duration = Duration::from_secs(90);
const ACTIVE_HEARTBEAT_STALE_TTL: Duration = Duration::from_secs(5 * 60);
const ACTIVE_SESSION_TTL: Duration = Duration::from_secs(4 * 60 * 60);
const OPEN_JOB_TTL: Duration = Duration::from_secs(15 * 60);
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

#[derive(Debug, Clone)]
pub(in crate::api::gateway) struct BrowserLaunchReservation {
    id: String,
}

#[derive(Debug, Clone)]
pub(in crate::api::gateway) struct BrowserLaunchLifecycle {
    pub(in crate::api::gateway) url: String,
    pub(in crate::api::gateway) exit_id: String,
    pub(in crate::api::gateway) profile_key_hash: Option<String>,
    pub(in crate::api::gateway) vm_key_hash: Option<String>,
}

#[derive(Debug, Clone)]
pub(in crate::api::gateway) struct BrowserOpenJobHandle {
    pub(in crate::api::gateway) id: String,
    scope: String,
    principal_id: String,
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
    page_id: Option<String>,
    stream_cleanup: Option<BrowserStreamCleanup>,
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
    pending_stream_cleanups: BTreeMap<String, BrowserStreamCleanup>,
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
    pub(in crate::api::gateway) page_id: String,
    pub(in crate::api::gateway) principal_id: String,
    pub(in crate::api::gateway) stream_cleanup: Option<BrowserStreamCleanup>,
}

#[derive(Debug, Clone)]
pub(in crate::api::gateway) struct BrowserStreamCleanup {
    pub(in crate::api::gateway) stream_id: String,
    pub(in crate::api::gateway) principal_id: String,
}

pub(in crate::api::gateway) async fn reserve_browser_launch(
    data_dir: &Path,
    principal_id: &str,
    lifecycle: BrowserLaunchLifecycle,
) -> Result<BrowserLaunchReservation, (StatusCode, String)> {
    if principal_id.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Browser launch requires a principal".to_string(),
        ));
    }
    let limits = browser_session_limits();
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    let now = Instant::now();
    registry.purge_expired(now);
    let active_total = registry
        .sessions
        .values()
        .filter(|session| session.scope == scope)
        .count();
    let active_for_principal = registry
        .sessions
        .values()
        .filter(|session| session.scope == scope && session.principal_id == principal_id)
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
    let now = Instant::now();
    let started_at = SystemTime::now();
    registry.sessions.insert(
        id.clone(),
        BrowserSessionRecord {
            scope,
            principal_id: principal_id.to_string(),
            page_id: None,
            stream_cleanup: None,
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
    Ok(BrowserLaunchReservation { id })
}

pub(in crate::api::gateway) async fn create_browser_open_job(
    data_dir: &Path,
    principal_id: &str,
) -> BrowserOpenJobHandle {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_OPEN_JOB_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    registry.purge_expired(Instant::now());
    let id = registry.next_job_id();
    let now = Instant::now();
    registry.jobs.insert(
        id.clone(),
        BrowserOpenJobRecord {
            scope: scope.clone(),
            principal_id: principal_id.to_string(),
            state: BrowserOpenJobState::Pending,
            updated_at: now,
        },
    );
    BrowserOpenJobHandle {
        id,
        scope,
        principal_id: principal_id.to_string(),
    }
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

pub(in crate::api::gateway) async fn browser_open_job_for_principal(
    data_dir: &Path,
    open_id: &str,
    principal_id: &str,
) -> Option<BrowserOpenJobSnapshot> {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_OPEN_JOB_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    registry.purge_expired(Instant::now());
    registry.jobs.get(open_id).and_then(|job| {
        (job.scope == scope && job.principal_id == principal_id).then(|| match &job.state {
            BrowserOpenJobState::Pending => BrowserOpenJobSnapshot::Pending,
            BrowserOpenJobState::Completed(result) => {
                BrowserOpenJobSnapshot::Completed(result.clone())
            }
            BrowserOpenJobState::Failed(error) => BrowserOpenJobSnapshot::Failed(error.clone()),
        })
    })
}

async fn update_browser_open_job(handle: &BrowserOpenJobHandle, state: BrowserOpenJobState) {
    let registry = BROWSER_OPEN_JOB_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    registry.purge_expired(Instant::now());
    if let Some(job) = registry.jobs.get_mut(&handle.id) {
        if job.scope == handle.scope && job.principal_id == handle.principal_id {
            job.state = state;
            job.updated_at = Instant::now();
        }
    }
}

pub(in crate::api::gateway) async fn complete_browser_launch(
    reservation: &BrowserLaunchReservation,
    page_id: &str,
    stream_cleanup: Option<BrowserStreamCleanup>,
) {
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    if let Some(record) = registry.sessions.get_mut(&reservation.id) {
        record.page_id = Some(page_id.to_string());
        record.stream_cleanup = stream_cleanup;
        record.state = BrowserSessionState::Active;
        record.phase = BrowserLifecyclePhase::ActiveSession;
        record.last_seen_at = Instant::now();
        record.last_navigation_at = Some(SystemTime::now());
        record.failure_reason = None;
    }
}

pub(in crate::api::gateway) async fn release_browser_launch(
    reservation: &BrowserLaunchReservation,
) {
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    registry.lock().await.sessions.remove(&reservation.id);
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
) -> Option<BrowserStreamCleanup> {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    let key = registry.sessions.iter().find_map(|(key, session)| {
        (session.scope == scope
            && session.principal_id == principal_id
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
    url: Option<&str>,
) -> bool {
    mark_browser_page_lifecycle(
        data_dir,
        page_id,
        principal_id,
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
) -> bool {
    mark_browser_page_lifecycle(
        data_dir,
        page_id,
        principal_id,
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
) -> bool {
    mark_browser_page_lifecycle(
        data_dir,
        page_id,
        principal_id,
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
    failure_reason: impl Into<String>,
) -> bool {
    mark_browser_page_lifecycle(
        data_dir,
        page_id,
        principal_id,
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
) -> Option<BrowserStreamCleanup> {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let registry = registry.lock().await;
    registry.sessions.values().find_map(|session| {
        (session.scope == scope
            && session.principal_id == principal_id
            && session.page_id.as_deref() == Some(page_id))
        .then(|| session.stream_cleanup.clone())
        .flatten()
    })
}

pub(in crate::api::gateway) async fn record_browser_stream_cleanup_failure(
    data_dir: &Path,
    cleanup: BrowserStreamCleanup,
) {
    let scope = browser_session_scope(data_dir);
    let key = browser_stream_cleanup_key(&scope, &cleanup.stream_id);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    registry
        .lock()
        .await
        .pending_stream_cleanups
        .insert(key, cleanup);
}

pub(in crate::api::gateway) async fn forget_browser_stream_cleanup_failure(
    data_dir: &Path,
    stream_id: &str,
) {
    let scope = browser_session_scope(data_dir);
    let key = browser_stream_cleanup_key(&scope, stream_id);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    registry.lock().await.pending_stream_cleanups.remove(&key);
}

pub(in crate::api::gateway) async fn touch_browser_page(
    data_dir: &Path,
    page_id: &str,
    principal_id: &str,
) -> bool {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    for session in registry.sessions.values_mut() {
        if session.scope == scope
            && session.principal_id == principal_id
            && session.page_id.as_deref() == Some(page_id)
        {
            session.last_seen_at = Instant::now();
            return true;
        }
    }
    false
}

pub(in crate::api::gateway) async fn browser_gateway_session_status(
    data_dir: &Path,
    principal_id: &str,
) -> serde_json::Value {
    let limits = browser_session_limits();
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    let now = Instant::now();
    registry.purge_expired(now);
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
    let capacity_available =
        total_sessions < limits.total && principal_sessions < limits.per_principal;
    let sessions = registry
        .sessions
        .iter()
        .filter(|(_, session)| session.scope == scope)
        .map(|(session_id, session)| {
            browser_lifecycle_session_value(session_id, session, now, capacity_available)
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "schema": "elastos.browser.session-capacity/v1",
        "status": "configured",
        "active_sessions": active_sessions,
        "launching_sessions": launching_sessions,
        "total_sessions": total_sessions,
        "principal_sessions": principal_sessions,
        "max_active_sessions": limits.total,
        "max_sessions_per_principal": limits.per_principal,
        "capacity_available": capacity_available,
        "lifecycle": {
            "schema": "elastos.browser.lifecycle-status/v1",
            "owner": "runtime_gateway",
            "phases": BROWSER_LIFECYCLE_PHASES,
            "capacity_available": capacity_available,
            "sessions": sessions,
            "redaction": {
                "principal_id": "sha256-16",
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
            "ttl_seconds": ACTIVE_SESSION_TTL.as_secs(),
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
    registry.purge_expired(Instant::now());
    registry
        .sessions
        .values()
        .any(|session| session.scope == scope && session.principal_id == principal_id)
}

pub(in crate::api::gateway) async fn take_stale_browser_pages(
    data_dir: &Path,
) -> Vec<BrowserPageCleanup> {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    registry.take_stale_active_pages(&scope, Instant::now())
}

pub(in crate::api::gateway) async fn take_pending_browser_stream_cleanups(
    data_dir: &Path,
) -> Vec<BrowserStreamCleanup> {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    let prefix = format!("{scope}\n");
    let keys = registry
        .pending_stream_cleanups
        .keys()
        .filter(|key| key.starts_with(&prefix))
        .cloned()
        .collect::<Vec<_>>();
    keys.into_iter()
        .filter_map(|key| registry.pending_stream_cleanups.remove(&key))
        .collect()
}

fn browser_session_scope(data_dir: &Path) -> String {
    data_dir.to_string_lossy().into_owned()
}

fn browser_stream_cleanup_key(scope: &str, stream_id: &str) -> String {
    format!("{scope}\n{stream_id}")
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
    fn next_reservation_id(&mut self) -> String {
        self.serial = self.serial.saturating_add(1);
        format!("browser-launch:{:016x}", self.serial)
    }

    fn purge_expired(&mut self, now: Instant) {
        self.sessions.retain(|_, session| match session.state {
            BrowserSessionState::Launching => {
                now.duration_since(session.created_at) <= LAUNCH_RESERVATION_TTL
            }
            BrowserSessionState::Active => {
                now.duration_since(session.last_seen_at) <= ACTIVE_SESSION_TTL
            }
        });
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
                    session.page_id.map(|page_id| BrowserPageCleanup {
                        page_id,
                        principal_id: session.principal_id,
                        stream_cleanup: session.stream_cleanup,
                    })
                })
            })
            .collect()
    }
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
            page_id: page_id.map(str::to_string),
            stream_cleanup: None,
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

    #[test]
    fn browser_registry_purges_expired_launches_but_keeps_recent_active_sessions() {
        let mut registry = BrowserSessionRegistry::default();
        let now = Instant::now();
        registry.sessions.insert(
            "old-launch".to_string(),
            test_session_record(
                "/tmp/elastos-test-a",
                None,
                BrowserSessionState::Launching,
                now - LAUNCH_RESERVATION_TTL - Duration::from_secs(1),
                now - LAUNCH_RESERVATION_TTL - Duration::from_secs(1),
            ),
        );
        registry.sessions.insert(
            "active".to_string(),
            test_session_record(
                "/tmp/elastos-test-a",
                Some("page:test"),
                BrowserSessionState::Active,
                now,
                now,
            ),
        );

        registry.purge_expired(now);

        assert!(!registry.sessions.contains_key("old-launch"));
        assert!(registry.sessions.contains_key("active"));
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
        assert_eq!(stale[0].page_id, "page:stale");
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
}
