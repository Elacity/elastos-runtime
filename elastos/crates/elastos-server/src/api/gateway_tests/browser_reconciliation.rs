use super::*;
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

async fn finish_current_reconciliation_sweep() {
    for _ in 0..1_000 {
        tokio::task::yield_now().await;
    }
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
    tokio::time::advance(Duration::from_millis(100)).await;
    yield_until_atomic(&reconciliation_calls, 2).await;
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

    tokio::time::advance(Duration::from_millis(100)).await;
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
    let mut close_calls_guard = close_calls.lock().await;
    let reconciler = start_browser_lifecycle_reconciler(state.clone()).expect("Runtime reconciler");

    for _ in 0..10_000 {
        if close_calls_guard.len() == 2 {
            break;
        }
        tokio::task::yield_now().await;
        drop(close_calls_guard);
        close_calls_guard = close_calls.lock().await;
    }
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

    yield_until_close_count(&close_calls, 3).await;
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
