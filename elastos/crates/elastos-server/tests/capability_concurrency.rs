//! Phase 4 Day 3 — capability-bridge dispatch race audit.
//!
//! The carrier bridge's `handle_request` path (in
//! `elastos-server::carrier_bridge`) dispatches into THREE concurrency
//! surfaces of the runtime crate when a guest issues a capability or
//! provider call:
//!
//! 1. `PendingRequestStore`            — holds in-flight grant
//!    requests, written by `request_capability` handlers and read /
//!    mutated by `approve` / `deny` paths.
//! 2. `CapabilityManager::validate`    — verifies a `CapabilityToken`,
//!    consults the `CapabilityStore` (epoch state, revocation set,
//!    use-count counters), and emits to the `AuditLog`.
//! 3. `ProviderRegistry::send_raw`     — exercised by Phase 4 Day 3's
//!    cross-VM RPC stress test in `vm_provider.rs`.
//!
//! These integration tests cover (1) and (2). Their purpose is NOT
//! to add new behaviour to the runtime — it is to *prove*, from the
//! server's vantage point, that the existing locking primitives
//! compose safely when N microVMs simultaneously punch capability
//! traffic through the bridge. If any of these tests flakes or
//! deadlocks the conclusion is a real bug worth a separate ticket
//! (per the Day 3 ground rules — no silent runtime crate edits).
//!
//! Anchored in: `docs/vz-backend/PHASE_4_DAY_3_NOTES.md`.

use std::sync::Arc;
use std::time::Instant;

use elastos_runtime::capability::pending::{GrantDuration, PendingRequestStore, RequestStatus};
use elastos_runtime::capability::{
    Action, CapabilityManager, CapabilityToken, ResourceId, TokenConstraints,
};
use elastos_runtime::primitives::audit::AuditLog;
use elastos_runtime::primitives::metrics::MetricsManager;
use elastos_runtime::session::SessionId;

/// Drive 100 concurrent `create_request` calls — each in its own
/// "session" (capsule) to stay below the per-session quota — then
/// resolve them: the first half via `grant_request`, the second
/// half via `deny_request`. Every request must end in EXACTLY one
/// outcome (Granted | Denied), no request may be lost, no request
/// may resolve to two outcomes.
///
/// Audit target: `PendingRequestStore::{requests, session_requests}`
/// — two `RwLock<HashMap<…>>`. The bridge holds neither across an
/// await, but multiple bridges can race the same store.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pending_store_resolves_100_concurrent_requests_without_loss() {
    const REQUEST_COUNT: usize = 100;

    let store = Arc::new(PendingRequestStore::new(Arc::new(AuditLog::new())));

    // 100 distinct sessions side-step the per-session pending
    // quota (`MAX_PENDING_PER_SESSION = 32`) and model the
    // realistic N-microVMs-each-asking-once scenario.
    let mut create_set = tokio::task::JoinSet::new();
    for idx in 0..REQUEST_COUNT {
        let store = Arc::clone(&store);
        create_set.spawn(async move {
            let session = SessionId::from_string(format!("session-{idx:03}"));
            let resource =
                ResourceId::new(format!("localhost://Users/capsule-{idx:03}/scratchpad"));
            let action = if idx % 2 == 0 {
                Action::Read
            } else {
                Action::Write
            };
            let req = store.create_request(session, resource, action).await;
            (idx, req)
        });
    }

    let mut requests = Vec::with_capacity(REQUEST_COUNT);
    while let Some(joined) = create_set.join_next().await {
        requests.push(joined.expect("create task must not panic"));
    }
    assert_eq!(requests.len(), REQUEST_COUNT);

    // Every newly created request must be Pending — if `create_request`
    // had silently dropped one we'd see a `Denied{ reason: "Too many …" }`
    // here (the in-store capacity cap is well above 100).
    for (idx, req) in &requests {
        assert!(
            matches!(req.status, RequestStatus::Pending),
            "request {idx} unexpectedly entered non-Pending state at create: {:?}",
            req.status
        );
    }

    // Resolve them concurrently — half grant, half deny, mixed
    // ordering so the lock contention is genuinely interleaved.
    let mut resolve_set = tokio::task::JoinSet::new();
    for (idx, req) in requests.iter().cloned() {
        let store = Arc::clone(&store);
        if idx % 2 == 0 {
            resolve_set.spawn(async move {
                let token = CapabilityToken::new(
                    format!("capsule-{idx:03}"),
                    [0u8; 32],
                    req.resource.clone(),
                    req.action,
                    TokenConstraints::default(),
                    req.requested_at,
                    None,
                );
                store
                    .grant_request(req.id.as_str(), token, GrantDuration::Session)
                    .await
                    .map(|()| (idx, "granted"))
                    .map_err(|e| format!("grant {idx}: {e}"))
            });
        } else {
            resolve_set.spawn(async move {
                store
                    .deny_request(req.id.as_str(), "test denial")
                    .await
                    .map(|()| (idx, "denied"))
                    .map_err(|e| format!("deny {idx}: {e}"))
            });
        }
    }

    let mut resolved = Vec::with_capacity(REQUEST_COUNT);
    while let Some(joined) = resolve_set.join_next().await {
        let outcome = joined
            .expect("resolve task must not panic")
            .expect("every resolve call must succeed");
        resolved.push(outcome);
    }
    assert_eq!(resolved.len(), REQUEST_COUNT);

    // Final read-back: every request must be in its expected
    // terminal state, no losses, no spurious extras.
    let mut granted_seen = 0usize;
    let mut denied_seen = 0usize;
    for (idx, req) in &requests {
        let fetched = store
            .get_request(req.id.as_str())
            .await
            .unwrap_or_else(|| panic!("request {idx} disappeared from store"));
        match fetched.status {
            RequestStatus::Granted { .. } => granted_seen += 1,
            RequestStatus::Denied { .. } => denied_seen += 1,
            other => panic!(
                "request {idx} ended in unexpected state {:?} (expected Granted|Denied)",
                other
            ),
        }
    }
    assert_eq!(
        granted_seen,
        REQUEST_COUNT / 2,
        "exactly half the requests must end Granted"
    );
    assert_eq!(
        denied_seen,
        REQUEST_COUNT / 2,
        "exactly half the requests must end Denied"
    );
}

/// Mint 10 distinct capability tokens (resources overlap to
/// stress the resource-matcher) and fire 1000 parallel `validate`
/// calls. With `max_uses = None` the validator only takes read
/// locks on `CapabilityStore`; the entire batch should clear well
/// inside a 5s wall-clock budget. The acceptance threshold below
/// is intentionally loose (5s) so the test stays green even on
/// the slowest CI worker, while still catching a regression that
/// would degrade the path from "lock-free read" to "global
/// serialization".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn capability_validate_under_1000_parallel_calls_does_not_serialize() {
    const TOKEN_COUNT: usize = 10;
    const PARALLEL_CALLS: usize = 1000;
    const WALL_CLOCK_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);

    let store = Arc::new(elastos_runtime::capability::CapabilityStore::new());
    let audit_log = Arc::new(AuditLog::new());
    let metrics = Arc::new(MetricsManager::new());
    let manager = Arc::new(CapabilityManager::new(
        Arc::clone(&store),
        Arc::clone(&audit_log),
        Arc::clone(&metrics),
    ));

    // Resources overlap deliberately — every token grants
    // access to `localhost://Users/capsule-N/*`, and validate
    // calls request `localhost://Users/capsule-N/file-K` so the
    // resource matcher walks the pattern on every call.
    let tokens: Vec<(String, CapabilityToken)> = (0..TOKEN_COUNT)
        .map(|n| {
            let capsule = format!("capsule-{n}");
            let resource = ResourceId::new(format!("localhost://Users/capsule-{n}/*"));
            let token = manager.grant(
                &capsule,
                resource,
                Action::Read,
                TokenConstraints::default(),
                None,
            );
            (capsule, token)
        })
        .collect();
    let tokens = Arc::new(tokens);

    let started = Instant::now();
    let mut set = tokio::task::JoinSet::new();
    for call_idx in 0..PARALLEL_CALLS {
        let manager = Arc::clone(&manager);
        let tokens = Arc::clone(&tokens);
        set.spawn(async move {
            let (capsule, token) = &tokens[call_idx % TOKEN_COUNT];
            let resource = ResourceId::new(format!("localhost://Users/{capsule}/file-{call_idx}"));
            manager
                .validate(token, capsule, Action::Read, &resource, None)
                .await
        });
    }

    let mut ok_count = 0usize;
    while let Some(joined) = set.join_next().await {
        joined
            .expect("validate task must not panic")
            .unwrap_or_else(|e| panic!("validate must succeed for in-pattern resource: {e:?}"));
        ok_count += 1;
    }
    let elapsed = started.elapsed();

    assert_eq!(
        ok_count, PARALLEL_CALLS,
        "all 1000 validate calls must terminate Ok"
    );
    assert!(
        elapsed < WALL_CLOCK_BUDGET,
        "1000 parallel validates completed in {:?}, expected < {:?} — lock-contention regression?",
        elapsed,
        WALL_CLOCK_BUDGET
    );
}
