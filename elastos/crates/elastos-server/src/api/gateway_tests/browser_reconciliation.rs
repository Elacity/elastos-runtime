use super::*;
use crate::api::browser_engine_protocol::{
    BROWSER_ENGINE_CLEANUP_BINDING_SCHEMA, BROWSER_ENGINE_PROTOCOL_VERSION,
    BROWSER_ENGINE_PROVIDER_ID,
};
use crate::api::gateway::gateway_browser::*;
use std::sync::atomic::Ordering;

fn browser_lifecycle(owner_launch_id: &str) -> BrowserLaunchLifecycle {
    BrowserLaunchLifecycle {
        owner_launch_id: owner_launch_id.to_string(),
        browser_instance: None,
        url: "https://example.com/".to_string(),
        exit_id: "mock-exit".to_string(),
        engine_route_provider: "mock-browser-engine".to_string(),
        selected_engine_adapter: Some("mock-browser-engine".to_string()),
        profile_key_hash: None,
        vm_key_hash: None,
    }
}

async fn record_pending_launch(
    state: &GatewayState,
    principal_id: &str,
    owner_launch_id: &str,
    stream_id: &str,
) -> BrowserLaunchReservation {
    let reservation = reserve_browser_launch(
        &state.data_dir,
        principal_id,
        browser_lifecycle(owner_launch_id),
    )
    .await
    .expect("test Browser launch reservation");
    record_browser_launch_reconciliation_obligation(&state.data_dir, &reservation, stream_id, None)
        .await
        .expect("durable test launch reconciliation");
    reservation
}

fn retrying_browser_lifecycle(owner_launch_id: &str) -> BrowserLaunchLifecycle {
    let mut lifecycle = browser_lifecycle(owner_launch_id);
    lifecycle.engine_route_provider = "mock-retrying-browser-engine".to_string();
    lifecycle
}

async fn record_active_page(
    state: &GatewayState,
    principal_id: &str,
    owner_launch_id: &str,
    page_id: &str,
    stream_cleanup: Option<BrowserStreamCleanup>,
) -> BrowserLaunchReservation {
    let reservation = reserve_browser_launch(
        &state.data_dir,
        principal_id,
        retrying_browser_lifecycle(owner_launch_id),
    )
    .await
    .expect("test active Browser launch reservation");
    complete_browser_launch(
        &state.data_dir,
        &reservation,
        BrowserLaunchEffect {
            page_id: page_id.to_string(),
            engine_provider: BROWSER_ENGINE_PROVIDER_ID.to_string(),
            engine_protocol_version: BROWSER_ENGINE_PROTOCOL_VERSION.to_string(),
            engine_adapter: "mock-browser-engine".to_string(),
            engine: "selkies_gstreamer".to_string(),
            provider_cleanup: serde_json::json!({
                "schema": BROWSER_ENGINE_CLEANUP_BINDING_SCHEMA,
                "page_id": page_id,
                "generation": reservation.generation(),
                "stream_id": format!("stream:{owner_launch_id}"),
                "adapter": "mock-browser-engine",
                "engine": "selkies_gstreamer",
            }),
            browser_page: serde_json::json!({
                "schema": "elastos.browser.engine.page/v1",
                "page_id": page_id,
            }),
            viewer_turn_capability: None,
            stream_cleanup,
        },
    )
    .await
    .expect("test active Browser page ownership");
    reservation
}

async fn yield_until_atomic(counter: &std::sync::atomic::AtomicUsize, expected: usize) {
    for _ in 0..100_000 {
        if counter.load(Ordering::SeqCst) >= expected {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!(
        "counter did not reach {expected}; observed {}",
        counter.load(Ordering::SeqCst)
    );
}

async fn yield_until_close_count(
    close_calls: &Arc<TokioMutex<Vec<serde_json::Value>>>,
    expected: usize,
) {
    for _ in 0..10_000 {
        if close_calls.lock().await.len() >= expected {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!(
        "close calls did not reach {expected}; observed {}",
        close_calls.lock().await.len()
    );
}

async fn yield_until_cleanup_obligation_count(state: &GatewayState, expected: usize) {
    for _ in 0..10_000 {
        if browser_engine_cleanup_obligation_count(&state.data_dir).await == expected {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!(
        "cleanup obligations did not reach {expected}; observed {}",
        browser_engine_cleanup_obligation_count(&state.data_dir).await
    );
}

async fn finish_current_reconciliation_sweep() {
    for _ in 0..1_000 {
        tokio::task::yield_now().await;
    }
}

async fn advance_until_atomic(
    counter: &std::sync::atomic::AtomicUsize,
    expected: usize,
    step: Duration,
) {
    for _ in 0..10 {
        if counter.load(Ordering::SeqCst) >= expected {
            return;
        }
        tokio::time::advance(step).await;
        finish_current_reconciliation_sweep().await;
    }
    yield_until_atomic(counter, expected).await;
}

#[tokio::test(start_paused = true)]
async fn runtime_restart_scans_pending_launch_and_closes_late_success_without_another_open() {
    let dir = tempfile::tempdir().unwrap();
    let (state, close_calls, reconciliation_calls) = browser_engine_reconciliation_test_state(
        dir.path(),
        MockDispatchedBrowserLaunchFailure::PendingThenLateSuccess,
    )
    .await;
    let principal_id = "person:local:restart-reconciliation";
    let reservation = record_pending_launch(
        &state,
        principal_id,
        "launch:restart-reconciliation",
        "stream:restart-reconciliation",
    )
    .await;
    assert_eq!(
        browser_launch_reconciliation_obligation_count(&state.data_dir).await,
        1
    );

    let first_runtime =
        start_browser_lifecycle_reconciler(state.clone()).expect("first Runtime reconciler");
    yield_until_atomic(&reconciliation_calls, 1).await;
    assert_eq!(
        browser_launch_reconciliation_obligation_count(&state.data_dir).await,
        1
    );
    assert!(close_calls.lock().await.is_empty());
    first_runtime.cancel();
    first_runtime.join().await.expect("first Runtime shutdown");

    let restarted_runtime =
        start_browser_lifecycle_reconciler(state.clone()).expect("restarted Runtime reconciler");
    yield_until_atomic(&reconciliation_calls, 2).await;
    yield_until_close_count(&close_calls, 1).await;
    assert_eq!(
        browser_launch_reconciliation_obligation_count(&state.data_dir).await,
        0
    );
    assert_eq!(
        browser_engine_cleanup_obligation_count(&state.data_dir).await,
        0
    );
    let calls = close_calls.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0]["runtime_cleanup"]["generation"],
        reservation.generation()
    );
    assert_eq!(
        calls[0]["runtime_cleanup"]["stream_id"],
        "stream:restart-reconciliation"
    );
    drop(calls);

    let replacement = reserve_browser_launch(
        &state.data_dir,
        principal_id,
        browser_lifecycle("launch:replacement-after-terminal-cleanup"),
    )
    .await
    .expect("replacement only after terminal cleanup");
    release_browser_launch(&replacement).await;
    restarted_runtime.cancel();
    restarted_runtime
        .join()
        .await
        .expect("restarted Runtime shutdown");
}

#[tokio::test(start_paused = true)]
async fn newly_committed_launch_obligation_wakes_reconciler_without_time_advance() {
    let dir = tempfile::tempdir().unwrap();
    let (state, close_calls, reconciliation_calls) = browser_engine_reconciliation_test_state(
        dir.path(),
        MockDispatchedBrowserLaunchFailure::ImmediateLateSuccess,
    )
    .await;
    let reconciler = start_browser_lifecycle_reconciler(state.clone()).expect("Runtime reconciler");
    tokio::task::yield_now().await;

    record_pending_launch(
        &state,
        "person:local:wake-reconciliation",
        "launch:wake-reconciliation",
        "stream:wake-reconciliation",
    )
    .await;

    yield_until_atomic(&reconciliation_calls, 1).await;
    yield_until_close_count(&close_calls, 1).await;
    assert_eq!(
        browser_launch_reconciliation_obligation_count(&state.data_dir).await,
        0
    );
    assert_eq!(close_calls.lock().await.len(), 1);
    reconciler.cancel();
    reconciler.join().await.expect("Runtime shutdown");
}

#[tokio::test(start_paused = true)]
async fn periodic_reconciler_reaps_stale_active_page_without_another_browser_open() {
    let dir = tempfile::tempdir().unwrap();
    let close_calls = Arc::new(TokioMutex::new(Vec::new()));
    let state = browser_engine_retrying_close_test_state(
        dir.path(),
        close_calls.clone(),
        MockBrowserEngineCloseFailure::Adapter,
        0,
        None,
        None,
    )
    .await;
    let reservation = record_active_page(
        &state,
        "person:local:stale-periodic",
        "launch:stale-periodic",
        "page:stale-periodic",
        None,
    )
    .await;
    let reconciler = start_controlled_browser_lifecycle_reconciler(state.clone())
        .expect("controlled Runtime reconciler");

    reconciler.wait_for_completed_sweeps(1).await;
    assert_eq!(browser_page_session_count(&state.data_dir).await, 1);
    assert!(close_calls.lock().await.is_empty());

    tokio::time::advance(ACTIVE_HEARTBEAT_STALE_TTL + Duration::from_millis(1)).await;
    reconciler.resume_sweeps();
    reconciler.wait_for_completed_sweeps(2).await;
    yield_until_cleanup_obligation_count(&state, 0).await;

    assert_eq!(browser_page_session_count(&state.data_dir).await, 0);
    let calls = close_calls.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["page_id"], "page:stale-periodic");
    assert_eq!(
        calls[0]["runtime_cleanup"]["generation"],
        reservation.generation()
    );
    drop(calls);
    reconciler.cancel();
    reconciler.join().await.expect("Runtime shutdown");
}

#[tokio::test(start_paused = true)]
async fn periodic_stale_cleanup_retries_exact_binding_and_blocks_replacement_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let close_calls = Arc::new(TokioMutex::new(Vec::new()));
    let state = browser_engine_retrying_close_test_state(
        dir.path(),
        close_calls.clone(),
        MockBrowserEngineCloseFailure::Adapter,
        2,
        None,
        None,
    )
    .await;
    let principal_id = "person:local:stale-retry";
    let reservation = record_active_page(
        &state,
        principal_id,
        "launch:stale-retry",
        "page:stale-retry",
        None,
    )
    .await;
    let reconciler = start_controlled_browser_lifecycle_reconciler(state.clone())
        .expect("controlled Runtime reconciler");

    reconciler.wait_for_completed_sweeps(1).await;
    tokio::time::advance(ACTIVE_HEARTBEAT_STALE_TTL + Duration::from_millis(1)).await;
    notify_browser_lifecycle_reconciler(&state.data_dir);
    reconciler.resume_sweeps();
    reconciler.wait_for_completed_sweeps(2).await;
    reconciler.resume_sweeps();
    reconciler.wait_for_completed_sweeps(3).await;

    assert_eq!(browser_page_session_count(&state.data_dir).await, 0);
    assert_eq!(
        browser_engine_cleanup_obligation_count(&state.data_dir).await,
        1
    );
    assert_eq!(close_calls.lock().await.len(), 2);
    let blocked = reserve_browser_launch(
        &state.data_dir,
        principal_id,
        retrying_browser_lifecycle("launch:blocked-while-stale-cleanup-pending"),
    )
    .await
    .expect_err("replacement must remain blocked while exact cleanup is pending");
    assert_eq!(blocked.0, StatusCode::SERVICE_UNAVAILABLE);
    assert!(blocked.1.contains("cleanup is pending"));

    notify_browser_lifecycle_reconciler(&state.data_dir);
    reconciler.resume_sweeps();
    reconciler.wait_for_completed_sweeps(4).await;
    yield_until_cleanup_obligation_count(&state, 0).await;

    let calls = close_calls.lock().await;
    assert_eq!(calls.len(), 3);
    assert!(calls
        .windows(2)
        .all(|pair| pair[0]["runtime_cleanup"] == pair[1]["runtime_cleanup"]));
    assert!(calls
        .iter()
        .all(|call| call["page_id"] == "page:stale-retry"));
    drop(calls);
    assert!(browser_reaped_page_tombstone_matches(
        &state.data_dir,
        "page:stale-retry",
        principal_id,
        "launch:stale-retry",
        reservation.cleanup_id(),
        None,
    )
    .await
    .expect("durable terminal retry receipt")
    .is_some());
    let replacement = reserve_browser_launch(
        &state.data_dir,
        principal_id,
        retrying_browser_lifecycle("launch:replacement-after-stale-cleanup"),
    )
    .await
    .expect("replacement only after typed terminal cleanup");
    release_browser_launch(&replacement).await;
    reconciler.cancel();
    reconciler.join().await.expect("Runtime shutdown");
}

#[tokio::test(start_paused = true)]
async fn stale_stream_timeout_releases_exact_claim_for_retry_without_restart() {
    let dir = tempfile::tempdir().unwrap();
    let engine_close_calls = Arc::new(TokioMutex::new(Vec::new()));
    let exit_close_calls = Arc::new(TokioMutex::new(Vec::new()));
    let exit_close_started = Arc::new(tokio::sync::Notify::new());
    let state = browser_engine_retrying_close_test_state(
        dir.path(),
        engine_close_calls,
        MockBrowserEngineCloseFailure::Adapter,
        0,
        Some(MockExitClosePlan {
            close_calls: exit_close_calls.clone(),
            close_failures: 0,
            close_hangs: 1,
            close_started: Some(exit_close_started.clone()),
        }),
        None,
    )
    .await;
    let principal_id = "person:local:stale-stream-timeout";
    record_active_page(
        &state,
        principal_id,
        "launch:stale-stream-timeout",
        "page:stale-stream-timeout",
        Some(BrowserStreamCleanup {
            stream_id: "stream:launch:stale-stream-timeout".to_string(),
            principal_id: principal_id.to_string(),
        }),
    )
    .await;
    let reconciler = start_controlled_browser_lifecycle_reconciler(state.clone())
        .expect("controlled Runtime reconciler");

    reconciler.wait_for_completed_sweeps(1).await;
    tokio::time::advance(ACTIVE_HEARTBEAT_STALE_TTL + Duration::from_millis(1)).await;
    notify_browser_lifecycle_reconciler(&state.data_dir);
    reconciler.resume_sweeps();
    exit_close_started.notified().await;
    let blocked = reserve_browser_launch(
        &state.data_dir,
        principal_id,
        retrying_browser_lifecycle("launch:blocked-during-stale-stream-timeout"),
    )
    .await
    .expect_err("stale cleanup must retain capacity while its stream call is hanging");
    assert_eq!(blocked.0, StatusCode::SERVICE_UNAVAILABLE);

    tokio::time::advance(BROWSER_LAUNCH_RECONCILIATION_CALL_TIMEOUT + Duration::from_millis(1))
        .await;
    reconciler.wait_for_completed_sweeps(2).await;
    assert_eq!(
        browser_engine_cleanup_obligation_count(&state.data_dir).await,
        1
    );
    assert_eq!(
        browser_stream_cleanup_obligation_count(&state.data_dir).await,
        1
    );

    let mut completed_sweeps = 2;
    for _ in 0..4 {
        if browser_engine_cleanup_obligation_count(&state.data_dir).await == 0
            && browser_stream_cleanup_obligation_count(&state.data_dir).await == 0
        {
            break;
        }
        notify_browser_lifecycle_reconciler(&state.data_dir);
        reconciler.resume_sweeps();
        completed_sweeps += 1;
        reconciler.wait_for_completed_sweeps(completed_sweeps).await;
    }
    let calls = exit_close_calls.lock().await;
    assert!(calls.len() >= 2);
    assert!(calls.iter().all(|call| {
        call["stream_id"] == "stream:launch:stale-stream-timeout"
            && call["principal_id"] == principal_id
    }));
    drop(calls);
    assert_eq!(
        browser_engine_cleanup_obligation_count(&state.data_dir).await,
        0
    );
    assert_eq!(
        browser_stream_cleanup_obligation_count(&state.data_dir).await,
        0
    );
    let replacement = reserve_browser_launch(
        &state.data_dir,
        principal_id,
        retrying_browser_lifecycle("launch:replacement-after-stale-stream-terminal"),
    )
    .await
    .expect("replacement only after stale engine and stream cleanup are terminal");
    release_browser_launch(&replacement).await;
    reconciler.cancel();
    reconciler.join().await.expect("Runtime shutdown");
}

#[tokio::test(start_paused = true)]
async fn hanging_stream_timeout_releases_exact_claim_for_retry_without_restart() {
    let dir = tempfile::tempdir().unwrap();
    let engine_close_calls = Arc::new(TokioMutex::new(Vec::new()));
    let exit_close_calls = Arc::new(TokioMutex::new(Vec::new()));
    let exit_close_started = Arc::new(tokio::sync::Notify::new());
    let state = browser_engine_retrying_close_test_state(
        dir.path(),
        engine_close_calls,
        MockBrowserEngineCloseFailure::Adapter,
        0,
        Some(MockExitClosePlan {
            close_calls: exit_close_calls.clone(),
            close_failures: 0,
            close_hangs: 1,
            close_started: Some(exit_close_started.clone()),
        }),
        None,
    )
    .await;
    let principal_id = "person:local:hanging-stream-timeout";
    let stream_cleanup = BrowserStreamCleanup {
        stream_id: "stream:hanging-stream-timeout".to_string(),
        principal_id: principal_id.to_string(),
    };
    record_browser_stream_cleanup_failure(&state.data_dir, stream_cleanup.clone())
        .await
        .expect("durable stream cleanup obligation");
    let reconciler = start_controlled_browser_lifecycle_reconciler(state.clone())
        .expect("controlled Runtime reconciler");

    exit_close_started.notified().await;
    let blocked = reserve_browser_launch(
        &state.data_dir,
        principal_id,
        retrying_browser_lifecycle("launch:blocked-during-stream-timeout"),
    )
    .await
    .expect_err("stream cleanup must retain capacity while its call is hanging");
    assert_eq!(blocked.0, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        browser_stream_cleanup_obligation_count(&state.data_dir).await,
        1
    );

    tokio::time::advance(BROWSER_LAUNCH_RECONCILIATION_CALL_TIMEOUT + Duration::from_millis(1))
        .await;
    reconciler.wait_for_completed_sweeps(1).await;
    assert_eq!(
        browser_stream_cleanup_obligation_count(&state.data_dir).await,
        1
    );

    let mut completed_sweeps = 1;
    for _ in 0..4 {
        if browser_stream_cleanup_obligation_count(&state.data_dir).await == 0 {
            break;
        }
        notify_browser_lifecycle_reconciler(&state.data_dir);
        reconciler.resume_sweeps();
        completed_sweeps += 1;
        reconciler.wait_for_completed_sweeps(completed_sweeps).await;
    }
    let calls = exit_close_calls.lock().await;
    assert!(calls.len() >= 2);
    assert!(calls.iter().all(|call| {
        call["stream_id"].as_str() == Some(stream_cleanup.stream_id.as_str())
            && call["principal_id"].as_str() == Some(stream_cleanup.principal_id.as_str())
    }));
    drop(calls);
    assert_eq!(
        browser_stream_cleanup_obligation_count(&state.data_dir).await,
        0
    );
    let replacement = reserve_browser_launch(
        &state.data_dir,
        principal_id,
        retrying_browser_lifecycle("launch:replacement-after-stream-terminal"),
    )
    .await
    .expect("replacement only after exact stream cleanup is terminal");
    release_browser_launch(&replacement).await;
    reconciler.cancel();
    reconciler.join().await.expect("Runtime shutdown");
}

#[tokio::test(start_paused = true)]
async fn transient_reconciliation_failure_retries_with_backoff_and_retains_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let (state, close_calls, reconciliation_calls) = browser_engine_reconciliation_test_state(
        dir.path(),
        MockDispatchedBrowserLaunchFailure::TransientThenLateSuccess,
    )
    .await;
    record_pending_launch(
        &state,
        "person:local:transient-reconciliation",
        "launch:transient-reconciliation",
        "stream:transient-reconciliation",
    )
    .await;
    let reconciler = start_browser_lifecycle_reconciler(state.clone()).expect("Runtime reconciler");

    yield_until_atomic(&reconciliation_calls, 1).await;
    assert_eq!(
        browser_launch_reconciliation_obligation_count(&state.data_dir).await,
        1
    );
    assert!(close_calls.lock().await.is_empty());
    finish_current_reconciliation_sweep().await;
    advance_until_atomic(&reconciliation_calls, 2, Duration::from_millis(100)).await;
    yield_until_close_count(&close_calls, 1).await;
    assert_eq!(
        browser_launch_reconciliation_obligation_count(&state.data_dir).await,
        0
    );
    reconciler.cancel();
    reconciler.join().await.expect("Runtime shutdown");
}

#[tokio::test(start_paused = true)]
async fn reconciliation_timeout_retains_exact_ownership_until_late_terminal_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let (state, close_calls, reconciliation_calls) = browser_engine_reconciliation_test_state(
        dir.path(),
        MockDispatchedBrowserLaunchFailure::TimeoutThenLateSuccess,
    )
    .await;
    let principal_id = "person:local:timeout-reconciliation";
    let reservation = record_pending_launch(
        &state,
        principal_id,
        "launch:timeout-reconciliation",
        "stream:timeout-reconciliation",
    )
    .await;
    let reconciler = start_browser_lifecycle_reconciler(state.clone()).expect("Runtime reconciler");

    yield_until_atomic(&reconciliation_calls, 1).await;
    assert_eq!(
        browser_launch_reconciliation_obligation_count(&state.data_dir).await,
        1
    );
    assert!(close_calls.lock().await.is_empty());

    tokio::time::advance(Duration::from_secs(31)).await;
    finish_current_reconciliation_sweep().await;
    assert_eq!(
        browser_launch_reconciliation_obligation_count(&state.data_dir).await,
        1
    );
    assert!(close_calls.lock().await.is_empty());

    advance_until_atomic(&reconciliation_calls, 2, Duration::from_millis(100)).await;
    yield_until_close_count(&close_calls, 1).await;
    assert_eq!(
        browser_launch_reconciliation_obligation_count(&state.data_dir).await,
        0
    );
    assert_eq!(
        browser_engine_cleanup_obligation_count(&state.data_dir).await,
        0
    );
    let calls = close_calls.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["page_id"], calls[0]["runtime_cleanup"]["page_id"]);
    assert_eq!(
        calls[0]["runtime_cleanup"]["generation"],
        reservation.generation()
    );
    assert_eq!(
        calls[0]["runtime_cleanup"]["stream_id"],
        "stream:timeout-reconciliation"
    );
    drop(calls);

    let replacement = reserve_browser_launch(
        &state.data_dir,
        principal_id,
        browser_lifecycle("launch:replacement-after-timeout-terminal-cleanup"),
    )
    .await
    .expect("replacement only after timeout path terminal cleanup");
    release_browser_launch(&replacement).await;
    reconciler.cancel();
    reconciler.join().await.expect("Runtime shutdown");
}

#[tokio::test(start_paused = true)]
async fn unavailable_reconciliation_is_capped_and_blocks_replacement_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _close_calls, reconciliation_calls) = browser_engine_reconciliation_test_state(
        dir.path(),
        MockDispatchedBrowserLaunchFailure::AlwaysUnavailable,
    )
    .await;
    let principal_id = "person:local:unavailable-reconciliation";
    record_pending_launch(
        &state,
        principal_id,
        "launch:unavailable-reconciliation",
        "stream:unavailable-reconciliation",
    )
    .await;
    let reconciler = start_browser_lifecycle_reconciler(state.clone()).expect("Runtime reconciler");

    yield_until_atomic(&reconciliation_calls, 1).await;
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    assert_eq!(reconciliation_calls.load(Ordering::SeqCst), 1);
    for delay in [
        Duration::from_millis(100),
        Duration::from_millis(200),
        Duration::from_millis(400),
        Duration::from_millis(800),
    ] {
        tokio::time::advance(delay).await;
        tokio::task::yield_now().await;
    }
    assert!(reconciliation_calls.load(Ordering::SeqCst) <= 5);
    assert_eq!(
        browser_launch_reconciliation_obligation_count(&state.data_dir).await,
        1
    );
    let blocked = reserve_browser_launch(
        &state.data_dir,
        principal_id,
        browser_lifecycle("launch:blocked-replacement"),
    )
    .await
    .expect_err("replacement must remain blocked");
    assert_eq!(blocked.0, StatusCode::SERVICE_UNAVAILABLE);
    assert!(blocked.1.contains("cleanup is pending"));
    reconciler.cancel();
    reconciler.join().await.expect("Runtime shutdown");
}

#[tokio::test(start_paused = true)]
async fn shutdown_cancels_active_reconciliation_and_releases_its_durable_claim() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _close_calls, reconciliation_calls) = browser_engine_reconciliation_test_state(
        dir.path(),
        MockDispatchedBrowserLaunchFailure::HangingReconciliation,
    )
    .await;
    record_pending_launch(
        &state,
        "person:local:shutdown-reconciliation",
        "launch:shutdown-reconciliation",
        "stream:shutdown-reconciliation",
    )
    .await;
    let reconciler = start_browser_lifecycle_reconciler(state.clone()).expect("Runtime reconciler");

    yield_until_atomic(&reconciliation_calls, 1).await;
    reconciler.cancel();
    reconciler.join().await.expect("prompt Runtime shutdown");
    assert_eq!(
        browser_launch_reconciliation_obligation_count(&state.data_dir).await,
        1
    );
    let claims = claim_pending_browser_launch_reconciliations(&state.data_dir, 1).await;
    assert_eq!(claims.len(), 1);
    release_browser_launch_reconciliation_claim(&state.data_dir, &claims[0]).await;
}

#[tokio::test(start_paused = true)]
async fn exact_cleanup_ownership_remains_until_typed_terminal_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let (state, close_calls, _reconciliation_calls) = browser_engine_reconciliation_test_state(
        dir.path(),
        MockDispatchedBrowserLaunchFailure::LateSuccessCleanupRetry,
    )
    .await;
    let principal_id = "person:local:terminal-receipt";
    record_pending_launch(
        &state,
        principal_id,
        "launch:terminal-receipt",
        "stream:terminal-receipt",
    )
    .await;
    let reconciler = start_controlled_browser_lifecycle_reconciler(state.clone())
        .expect("controlled Runtime reconciler");

    reconciler.wait_for_completed_sweeps(1).await;
    let close_calls_guard = close_calls.lock().await;
    assert_eq!(
        browser_launch_reconciliation_obligation_count(&state.data_dir).await,
        0
    );
    assert_eq!(
        browser_engine_cleanup_obligation_count(&state.data_dir).await,
        1
    );
    assert_eq!(close_calls_guard.len(), 2);
    assert_eq!(
        close_calls_guard[0]["runtime_cleanup"],
        close_calls_guard[1]["runtime_cleanup"]
    );
    assert!(reserve_browser_launch(
        &state.data_dir,
        principal_id,
        browser_lifecycle("launch:blocked-before-terminal-receipt"),
    )
    .await
    .is_err());
    notify_browser_lifecycle_reconciler(&state.data_dir);
    drop(close_calls_guard);

    reconciler.resume_sweeps();
    reconciler.wait_for_completed_sweeps(2).await;
    yield_until_cleanup_obligation_count(&state, 0).await;
    reconciler.cancel();
    reconciler.join().await.expect("Runtime shutdown");
    assert_eq!(
        browser_engine_cleanup_obligation_count(&state.data_dir).await,
        0
    );
    let calls = close_calls.lock().await;
    assert_eq!(calls.len(), 3);
    assert!(calls
        .windows(2)
        .all(|pair| pair[0]["runtime_cleanup"] == pair[1]["runtime_cleanup"]));
    drop(calls);
    let replacement = reserve_browser_launch(
        &state.data_dir,
        principal_id,
        browser_lifecycle("launch:replacement-after-terminal-receipt"),
    )
    .await
    .expect("replacement only after terminal receipt");
    release_browser_launch(&replacement).await;
}
