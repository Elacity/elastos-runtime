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
use std::time::{Duration, Instant};

const DEFAULT_MAX_BROWSER_SESSIONS: usize = 8;
const DEFAULT_MAX_BROWSER_SESSIONS_PER_PRINCIPAL: usize = 4;
const MAX_BROWSER_SESSIONS_LIMIT: usize = 32;
const LAUNCH_RESERVATION_TTL: Duration = Duration::from_secs(90);
const ACTIVE_HEARTBEAT_STALE_TTL: Duration = Duration::from_secs(5 * 60);
const ACTIVE_SESSION_TTL: Duration = Duration::from_secs(4 * 60 * 60);

static BROWSER_SESSION_REGISTRY: OnceLock<tokio::sync::Mutex<BrowserSessionRegistry>> =
    OnceLock::new();

#[derive(Debug, Clone)]
pub(in crate::api::gateway) struct BrowserLaunchReservation {
    id: String,
}

#[derive(Debug, Clone)]
struct BrowserSessionRecord {
    scope: String,
    principal_id: String,
    page_id: Option<String>,
    state: BrowserSessionState,
    created_at: Instant,
    last_seen_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserSessionState {
    Launching,
    Active,
}

#[derive(Debug, Default)]
struct BrowserSessionRegistry {
    sessions: BTreeMap<String, BrowserSessionRecord>,
    serial: u64,
}

#[derive(Debug, Clone, Copy)]
struct BrowserSessionLimits {
    total: usize,
    per_principal: usize,
}

#[derive(Debug, Clone)]
pub(in crate::api::gateway) struct BrowserStalePage {
    pub(in crate::api::gateway) page_id: String,
    pub(in crate::api::gateway) principal_id: String,
}

pub(in crate::api::gateway) async fn reserve_browser_launch(
    data_dir: &Path,
    principal_id: &str,
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
    registry.purge_expired(Instant::now());
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
    registry.sessions.insert(
        id.clone(),
        BrowserSessionRecord {
            scope,
            principal_id: principal_id.to_string(),
            page_id: None,
            state: BrowserSessionState::Launching,
            created_at: now,
            last_seen_at: now,
        },
    );
    Ok(BrowserLaunchReservation { id })
}

pub(in crate::api::gateway) async fn complete_browser_launch(
    reservation: &BrowserLaunchReservation,
    page_id: &str,
) {
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    if let Some(record) = registry.sessions.get_mut(&reservation.id) {
        record.page_id = Some(page_id.to_string());
        record.state = BrowserSessionState::Active;
        record.last_seen_at = Instant::now();
    }
}

pub(in crate::api::gateway) async fn release_browser_launch(
    reservation: &BrowserLaunchReservation,
) {
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    registry.lock().await.sessions.remove(&reservation.id);
}

pub(in crate::api::gateway) async fn release_browser_page(data_dir: &Path, page_id: &str) {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    let key = registry.sessions.iter().find_map(|(key, session)| {
        (session.scope == scope && session.page_id.as_deref() == Some(page_id)).then(|| key.clone())
    });
    if let Some(key) = key {
        registry.sessions.remove(&key);
    }
}

pub(in crate::api::gateway) async fn touch_browser_page(data_dir: &Path, page_id: &str) -> bool {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    for session in registry.sessions.values_mut() {
        if session.scope == scope && session.page_id.as_deref() == Some(page_id) {
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
    registry.purge_expired(Instant::now());
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
    serde_json::json!({
        "schema": "elastos.browser.session-capacity/v1",
        "status": "configured",
        "active_sessions": active_sessions,
        "launching_sessions": launching_sessions,
        "total_sessions": total_sessions,
        "principal_sessions": principal_sessions,
        "max_active_sessions": limits.total,
        "max_sessions_per_principal": limits.per_principal,
        "capacity_available": total_sessions < limits.total && principal_sessions < limits.per_principal,
        "heartbeat": {
            "route": "/api/apps/browser/pages/:page_id/heartbeat",
            "stale_after_seconds": ACTIVE_HEARTBEAT_STALE_TTL.as_secs(),
            "ttl_seconds": ACTIVE_SESSION_TTL.as_secs(),
        }
    })
}

pub(in crate::api::gateway) async fn take_stale_browser_pages(
    data_dir: &Path,
) -> Vec<BrowserStalePage> {
    let scope = browser_session_scope(data_dir);
    let registry = BROWSER_SESSION_REGISTRY.get_or_init(Default::default);
    let mut registry = registry.lock().await;
    registry.take_stale_active_pages(&scope, Instant::now())
}

fn browser_session_scope(data_dir: &Path) -> String {
    data_dir.to_string_lossy().into_owned()
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

    fn take_stale_active_pages(&mut self, scope: &str, now: Instant) -> Vec<BrowserStalePage> {
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
                    session.page_id.map(|page_id| BrowserStalePage {
                        page_id,
                        principal_id: session.principal_id,
                    })
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_registry_purges_expired_launches_but_keeps_recent_active_sessions() {
        let mut registry = BrowserSessionRegistry::default();
        let now = Instant::now();
        registry.sessions.insert(
            "old-launch".to_string(),
            BrowserSessionRecord {
                scope: "/tmp/elastos-test-a".to_string(),
                principal_id: "person:local:test".to_string(),
                page_id: None,
                state: BrowserSessionState::Launching,
                created_at: now - LAUNCH_RESERVATION_TTL - Duration::from_secs(1),
                last_seen_at: now - LAUNCH_RESERVATION_TTL - Duration::from_secs(1),
            },
        );
        registry.sessions.insert(
            "active".to_string(),
            BrowserSessionRecord {
                scope: "/tmp/elastos-test-a".to_string(),
                principal_id: "person:local:test".to_string(),
                page_id: Some("page:test".to_string()),
                state: BrowserSessionState::Active,
                created_at: now,
                last_seen_at: now,
            },
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
            BrowserSessionRecord {
                scope: "/tmp/elastos-a".to_string(),
                principal_id: "person:local:test".to_string(),
                page_id: Some("page:a".to_string()),
                state: BrowserSessionState::Active,
                created_at: now,
                last_seen_at: now,
            },
        );
        registry.sessions.insert(
            "scope-b".to_string(),
            BrowserSessionRecord {
                scope: "/tmp/elastos-b".to_string(),
                principal_id: "person:local:test".to_string(),
                page_id: Some("page:b".to_string()),
                state: BrowserSessionState::Active,
                created_at: now,
                last_seen_at: now,
            },
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
        registry.sessions.insert(
            "stale-active".to_string(),
            BrowserSessionRecord {
                scope: "/tmp/elastos-test-a".to_string(),
                principal_id: "person:local:test".to_string(),
                page_id: Some("page:stale".to_string()),
                state: BrowserSessionState::Active,
                created_at: now - ACTIVE_HEARTBEAT_STALE_TTL - Duration::from_secs(5),
                last_seen_at: now - ACTIVE_HEARTBEAT_STALE_TTL - Duration::from_secs(5),
            },
        );
        registry.sessions.insert(
            "fresh-active".to_string(),
            BrowserSessionRecord {
                scope: "/tmp/elastos-test-a".to_string(),
                principal_id: "person:local:test".to_string(),
                page_id: Some("page:fresh".to_string()),
                state: BrowserSessionState::Active,
                created_at: now,
                last_seen_at: now,
            },
        );
        registry.sessions.insert(
            "other-scope".to_string(),
            BrowserSessionRecord {
                scope: "/tmp/elastos-test-b".to_string(),
                principal_id: "person:local:test".to_string(),
                page_id: Some("page:other".to_string()),
                state: BrowserSessionState::Active,
                created_at: now - ACTIVE_HEARTBEAT_STALE_TTL - Duration::from_secs(5),
                last_seen_at: now - ACTIVE_HEARTBEAT_STALE_TTL - Duration::from_secs(5),
            },
        );

        let stale = registry.take_stale_active_pages("/tmp/elastos-test-a", now);

        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].page_id, "page:stale");
        assert!(!registry.sessions.contains_key("stale-active"));
        assert!(registry.sessions.contains_key("fresh-active"));
        assert!(registry.sessions.contains_key("other-scope"));
    }
}
