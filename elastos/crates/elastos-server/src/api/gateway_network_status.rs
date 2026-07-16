//! `GET /api/apps/home/network-status` — the shell top-bar's glanceable health view.
//!
//! One cheap, cached aggregation of signals the gateway ALREADY has (P10: one canonical
//! path per signal, no new probe planes):
//!
//! - Carrier: a bounded `peer.list_peers` provider op (registry, in-process).
//! - Chain RPC: the passive health cell in `chain_tx` — recorded by live reads that
//!   already run (indexer sweeps, enrichment); this route never spawns a chain call.
//! - Market index: the same cache/snapshot `market_indexer_status` serves.
//! - Availability: a bounded `availability.status` provider op (config echo, no upstream).
//!
//! Stale-while-revalidate: the handler serves the last snapshot IMMEDIATELY and, past
//! freshness, refreshes in a single-flight background task. It never blocks a shell poll
//! on a provider op, and a probe failure degrades that one row — never a 500.

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex as StdMutex;
use std::time::{Duration as StdDuration, Instant};

/// How long a snapshot stays fresh before a poll triggers a background refresh. The shell
/// polls every 30s; refreshing at 20s keeps at most one probe cycle per shell interval.
const NETWORK_STATUS_FRESH: StdDuration = StdDuration::from_secs(20);
/// Upper bound on each provider op during a refresh cycle (in-process, normally instant).
const NETWORK_PROBE_TIMEOUT: StdDuration = StdDuration::from_secs(4);

type SnapshotCell = StdMutex<Option<(Instant, serde_json::Value)>>;

fn network_status_cell() -> &'static SnapshotCell {
    static CELL: OnceLock<SnapshotCell> = OnceLock::new();
    CELL.get_or_init(|| StdMutex::new(None))
}

pub(super) async fn home_network_status(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = require_home_token(&state.data_dir, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": err.to_string() })),
        )
            .into_response();
    }

    let cached = network_status_cell()
        .lock()
        .ok()
        .and_then(|cell| cell.clone());
    match cached {
        Some((at, snapshot)) => {
            if at.elapsed() > NETWORK_STATUS_FRESH {
                spawn_network_status_refresh(state);
            }
            let mut body = snapshot;
            body["age_secs"] = serde_json::json!(at.elapsed().as_secs());
            Json(body).into_response()
        }
        None => {
            // Cold start: report every row unknown and kick the first probe cycle; the
            // shell's next poll (or the popover-open refetch) picks up the real snapshot.
            spawn_network_status_refresh(state);
            Json(serde_json::json!({
                "schema": "elastos.home.network-status/v1",
                "age_secs": null,
                "warming": true,
                "carrier": { "state": "unknown" },
                "chain": { "state": "unknown" },
                "index": { "state": "unknown" },
                "availability": { "state": "unknown" },
            }))
            .into_response()
        }
    }
}

/// Single-flight background refresh: at most one probe cycle in flight; failure keeps the
/// prior snapshot (stale beats absent — the response carries `age_secs` so staleness is honest).
fn spawn_network_status_refresh(state: GatewayState) {
    static REFRESHING: AtomicBool = AtomicBool::new(false);
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    if REFRESHING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    tokio::spawn(async move {
        let snapshot = build_network_status_snapshot(&state).await;
        if let Ok(mut cell) = network_status_cell().lock() {
            *cell = Some((Instant::now(), snapshot));
        }
        REFRESHING.store(false, Ordering::Release);
    });
}

async fn build_network_status_snapshot(state: &GatewayState) -> serde_json::Value {
    let registry = state.provider_registry.as_ref();

    let carrier = match registry {
        Some(registry) => probe_carrier(registry).await,
        None => serde_json::json!({ "state": "off", "detail": "no provider registry" }),
    };
    let availability = match registry {
        Some(registry) => probe_availability(registry).await,
        None => serde_json::json!({ "state": "off", "detail": "no provider registry" }),
    };

    // Passive cell only — reporting chain health must never CAUSE chain traffic.
    let (ok_secs, err_secs, err_msg) = crate::api::chain_tx::chain_health_snapshot();
    let chain_state = match (ok_secs, err_secs) {
        (Some(ok), Some(err)) if err < ok => "degraded",
        (Some(_), _) => "ok",
        (None, Some(_)) => "degraded",
        (None, None) => "unknown",
    };
    let chain = serde_json::json!({
        "state": chain_state,
        "last_ok_secs": ok_secs,
        "last_error_secs": err_secs,
        "last_error": err_msg,
    });

    let index = indexer_status_json(&state.data_dir);

    serde_json::json!({
        "schema": "elastos.home.network-status/v1",
        "carrier": carrier,
        "chain": chain,
        "index": index,
        "availability": availability,
    })
}

/// Carrier row: peer count from the in-process Carrier provider. `NoProvider` reads as
/// "off" (Carrier not running in this process), an op error as "degraded".
async fn probe_carrier(registry: &Arc<ProviderRegistry>) -> serde_json::Value {
    let request = serde_json::json!({ "op": "list_peers" });
    let op = registry.send_raw("peer", &request);
    match tokio::time::timeout(NETWORK_PROBE_TIMEOUT, op).await {
        Ok(Ok(body)) => {
            if body.get("status").and_then(|s| s.as_str()) == Some("error") {
                let detail = body
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("carrier error");
                return serde_json::json!({ "state": "degraded", "detail": detail });
            }
            let peers = body
                .get("data")
                .and_then(|d| d.get("peers"))
                .and_then(|p| p.as_array())
                .map(Vec::len)
                .unwrap_or(0);
            serde_json::json!({ "state": "ok", "peers": peers })
        }
        Ok(Err(elastos_runtime::provider::ProviderError::NoProvider(_))) => {
            serde_json::json!({ "state": "off", "detail": "carrier not running" })
        }
        Ok(Err(err)) => serde_json::json!({ "state": "degraded", "detail": err.to_string() }),
        Err(_) => serde_json::json!({ "state": "degraded", "detail": "carrier probe timed out" }),
    }
}

/// Availability row: the provider's `status` op is a config echo (targets it would pin to),
/// not an upstream call. Absent provider reads as "off" — pin-forward simply isn't wired.
async fn probe_availability(registry: &Arc<ProviderRegistry>) -> serde_json::Value {
    let request = serde_json::json!({ "op": "status" });
    let op = registry.send_raw("availability", &request);
    match tokio::time::timeout(NETWORK_PROBE_TIMEOUT, op).await {
        Ok(Ok(body)) => {
            if body.get("status").and_then(|s| s.as_str()) == Some("error") {
                let detail = body
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("availability error");
                return serde_json::json!({ "state": "degraded", "detail": detail });
            }
            let targets = body
                .get("data")
                .and_then(|d| d.get("targets"))
                .and_then(|t| t.as_array())
                .map(Vec::len)
                .unwrap_or(0);
            if targets == 0 {
                serde_json::json!({ "state": "unconfigured", "targets": 0 })
            } else {
                serde_json::json!({ "state": "ok", "targets": targets })
            }
        }
        Ok(Err(elastos_runtime::provider::ProviderError::NoProvider(_))) => {
            serde_json::json!({ "state": "off", "detail": "availability provider not registered" })
        }
        Ok(Err(err)) => serde_json::json!({ "state": "degraded", "detail": err.to_string() }),
        Err(_) => {
            serde_json::json!({ "state": "degraded", "detail": "availability probe timed out" })
        }
    }
}
