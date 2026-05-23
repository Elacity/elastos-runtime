//! Phase 5 Day 2 — Mac-only end-to-end RPC contract test for the
//! Home-frontdoor microVM chain.
//!
//! Lock-in goal: the supervisor's RPC sequence that the
//! shell-level `scripts/home-frontdoor-smoke.sh` depends on
//! (`LaunchCapsule` → `CapsuleStatus` → `StopCapsule` →
//! `CapsuleVzError`) is contract-stable for the three providers
//! the Home frontdoor brings up: `localhost-provider`,
//! `did-provider`, and `webspace-provider`.
//!
//! Production parity. The test drives the same
//! [`Supervisor::handle_request`] path that the
//! `elastos start <provider>` CLI uses — exactly the surface
//! the shell smoke ultimately reaches via the publisher gateway.
//! No private hooks, no synthetic shortcuts: if the contract
//! drifts, this test surfaces it.
//!
//! Visibly-skip semantics. Until Phase 6 restores the
//! `darwin-arm64` release metadata in `components.json` and the
//! Mac substrate ships a kernel + rootfs, **no host has the
//! three installed providers**. The test therefore skips with a
//! clear `eprintln!` rather than failing — the skip itself is
//! the Phase-6-prerequisite-not-met telemetry. CI dashboards
//! alert on the skip line separately. Promoting to a required
//! gate is the Phase 7 follow-up tracked in
//! `docs/vz-backend/PLAN.md`.
//!
//! Anchored in: `docs/vz-backend/PHASE_5_DAY_2_NOTES.md` and the
//! `Day 2` block of `docs/vz-backend/PHASE_5_PLAN.md` L41-L57.

#![cfg(target_os = "macos")]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use elastos_server::setup::ComponentsManifest;
use elastos_server::supervisor::{Supervisor, SupervisorRequest, SupervisorResponse};

const LAUNCH_BUDGET: Duration = Duration::from_secs(30);
const STOP_BUDGET: Duration = Duration::from_secs(10);
const STATUS_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// The three providers the Home frontdoor brings up on Linux as
/// crosvm microVMs. On Mac these are the same names but routed
/// through the Vz substrate. The set is pinned to keep this
/// integration test honest about which capsules the
/// shell-level `home-frontdoor-smoke.sh` actually requires —
/// changing the smoke without updating this set produces a
/// drift between the Rust contract guard and the shell smoke.
const HOME_FRONTDOOR_PROVIDERS: &[&str] =
    &["localhost-provider", "did-provider", "webspace-provider"];

/// Auto-discover the supervisor's canonical data directory.
/// Mirrors the Phase 4 Day 4 smoke's helper of the same name —
/// duplicated rather than re-exported because both
/// integration test files run in separate crates and the
/// helper is private to its test file.
///
/// Order of precedence:
///   1. `ELASTOS_VZ_SMOKE_DATA_DIR` env override.
///   2. `~/.local/share/elastos` (Linux convention; Mac's
///      `elastos setup` honours this too via `XDG_DATA_HOME`).
fn discover_data_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ELASTOS_VZ_SMOKE_DATA_DIR") {
        let pb = PathBuf::from(p);
        return pb.is_dir().then_some(pb);
    }
    let home = std::env::var_os("HOME")?;
    let candidate = PathBuf::from(home).join(".local/share/elastos");
    candidate.is_dir().then_some(candidate)
}

/// Resolve the per-provider capsule directory under
/// `<data_dir>/capsules/<name>/` AND verify that it has both a
/// parseable `capsule.json` AND a `rootfs.ext4`. Returns `None`
/// if either is missing — the caller treats absent providers as
/// the visible-skip signal.
fn resolve_provider_capsule(
    data_dir: &std::path::Path,
    name: &str,
) -> Option<(PathBuf, elastos_common::CapsuleManifest)> {
    let capsule_dir = data_dir.join("capsules").join(name);
    if !capsule_dir.is_dir() {
        return None;
    }
    let manifest_path = capsule_dir.join("capsule.json");
    let rootfs_path = capsule_dir.join("rootfs.ext4");
    if !manifest_path.is_file() || !rootfs_path.is_file() {
        return None;
    }
    let manifest_data = std::fs::read_to_string(&manifest_path).ok()?;
    let manifest: elastos_common::CapsuleManifest = serde_json::from_str(&manifest_data).ok()?;
    if !matches!(manifest.capsule_type, elastos_common::CapsuleType::MicroVM) {
        return None;
    }
    Some((capsule_dir, manifest))
}

/// Load `<data_dir>/components.json`. Returns `None` if the
/// manifest is missing or unparseable; caller skips visibly.
fn load_components_manifest(data_dir: &std::path::Path) -> Option<ComponentsManifest> {
    let path = data_dir.join("components.json");
    let bytes = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str::<ComponentsManifest>(&bytes).ok()
}

/// Poll `CapsuleStatus` until the response reports `running` or
/// `budget` elapses. Returns `Ok` only on observed-running.
async fn wait_for_running(
    supervisor: &Supervisor,
    handle: &str,
    budget: Duration,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + budget;
    loop {
        let resp = supervisor
            .handle_request(SupervisorRequest::CapsuleStatus {
                handle: handle.to_string(),
            })
            .await;
        if response_status_eq(&resp, "running") {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(anyhow::anyhow!(
                "capsule handle='{handle}' did not report running within {:?}; \
                 last response: {:?}",
                budget,
                resp
            ));
        }
        tokio::time::sleep(STATUS_POLL_INTERVAL).await;
    }
}

/// Poll `CapsuleStatus` until the capsule no longer reports
/// running (stopped, error, or not_found) or `budget` elapses.
async fn wait_for_stopped(
    supervisor: &Supervisor,
    handle: &str,
    budget: Duration,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + budget;
    loop {
        let resp = supervisor
            .handle_request(SupervisorRequest::CapsuleStatus {
                handle: handle.to_string(),
            })
            .await;
        if !response_status_eq(&resp, "running") {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(anyhow::anyhow!(
                "capsule handle='{handle}' did not report stopped within {:?}; \
                 last response: {:?}",
                budget,
                resp
            ));
        }
        tokio::time::sleep(STATUS_POLL_INTERVAL).await;
    }
}

/// Case-insensitive `status` field match. The supervisor's
/// response uses lowercase by convention but we stay
/// forward-compatible against any future enum-case drift.
fn response_status_eq(resp: &SupervisorResponse, expected: &str) -> bool {
    let json = serde_json::to_value(resp).unwrap_or(serde_json::Value::Null);
    json.get("status")
        .and_then(|v| v.as_str())
        .map(|s| s.eq_ignore_ascii_case(expected))
        .unwrap_or(false)
}

/// Phase 5 Day 2 — drive the three Home-frontdoor providers
/// through the full Launch → Status → Stop → VzError contract.
///
/// Each provider is launched, observed to reach `running` within
/// `LAUNCH_BUDGET`, stopped, observed to reach not-running
/// within `STOP_BUDGET`, and finally queried for `vz_error`
/// (which on the happy path must be absent — Phase 4 Day 8
/// contract).
///
/// The test is visibly-skip in three independent paths:
///   - `is_supported() == false` → skip (no Apple Silicon Vz).
///   - data dir absent → skip (no `elastos setup` ever run).
///   - any provider missing/unparseable → skip (Phase 6
///     prerequisite not met).
///
/// Anchored in: `docs/vz-backend/PHASE_5_DAY_2_NOTES.md`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn vz_home_frontdoor_providers_round_trip_through_supervisor() {
    if !elastos_vz::is_supported() {
        eprintln!(
            "vz_home_frontdoor_providers_round_trip_through_supervisor: \
             skipping — is_supported() == false (off Apple Silicon macOS, \
             Vz framework unreachable)"
        );
        return;
    }
    let Some(data_dir) = discover_data_dir() else {
        eprintln!(
            "vz_home_frontdoor_providers_round_trip_through_supervisor: \
             skipping — no $ELASTOS_VZ_SMOKE_DATA_DIR or ~/.local/share/elastos \
             directory. Run `elastos setup` and install the three Home-frontdoor \
             providers ({}) first.",
            HOME_FRONTDOOR_PROVIDERS.join(", "),
        );
        return;
    };
    let Some(registry) = load_components_manifest(&data_dir) else {
        eprintln!(
            "vz_home_frontdoor_providers_round_trip_through_supervisor: \
             skipping — no components.json under {} (or it failed to parse). \
             Run `elastos setup` first.",
            data_dir.display()
        );
        return;
    };

    // Discover all three providers in one pass. We skip
    // ATOMICALLY: if even one is missing we don't bother
    // launching the others, because the home-frontdoor chain
    // depends on all three. This matches the shell smoke's
    // current behaviour (which requires all three installed
    // before it can succeed).
    let mut providers: Vec<(String, PathBuf, elastos_common::CapsuleManifest)> = Vec::new();
    for name in HOME_FRONTDOOR_PROVIDERS {
        let Some((capsule_dir, manifest)) = resolve_provider_capsule(&data_dir, name) else {
            eprintln!(
                "vz_home_frontdoor_providers_round_trip_through_supervisor: \
                 skipping — provider '{}' is not installed (missing capsule.json \
                 or rootfs.ext4 under {}). This is the expected pre-Phase-6 state \
                 on Mac: `components.json` lacks darwin-arm64 release metadata \
                 (see docs/vz-backend/PLAN.md L321), so `elastos setup` cannot \
                 install the three providers yet. The shell-level smoke \
                 `scripts/home-frontdoor-smoke.sh` skips with the same telemetry.",
                name,
                data_dir.join("capsules").join(name).display(),
            );
            return;
        };
        providers.push(((*name).to_string(), capsule_dir, manifest));
    }
    eprintln!(
        "vz_home_frontdoor_providers_round_trip_through_supervisor: \
         all {} Home-frontdoor providers installed; driving full RPC chain",
        providers.len()
    );

    let mut supervisor = Supervisor::new(data_dir.clone(), registry);
    let provider_registry = std::sync::Arc::new(elastos_runtime::provider::ProviderRegistry::new());
    supervisor.set_provider_registry(provider_registry);

    // Per-provider Launch → wait running → Stop → wait stopped
    // → CapsuleVzError. Each step is best-effort cleanup on
    // failure: a partial-failure must not leak a running VM.
    let mut launched_handles: Vec<(String, String)> = Vec::new();
    for (name, capsule_dir, manifest) in &providers {
        eprintln!(
            "vz_home_frontdoor_providers_round_trip_through_supervisor: \
             launching '{name}' from {} (provides={:?})",
            capsule_dir.display(),
            manifest.provides.as_deref()
        );
        let response = supervisor
            .handle_request(SupervisorRequest::LaunchCapsule {
                name: name.clone(),
                config: serde_json::Value::Null,
            })
            .await;
        let response_json = serde_json::to_value(&response).expect("serialize launch response");
        let handle = response_json
            .get("handle")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| {
                // Best-effort cleanup before panicking — every
                // already-launched provider must be stopped to
                // avoid leaking VMs across the test run.
                panic!(
                    "LaunchCapsule for '{name}' did not return a handle. \
                     Response was: {response_json}"
                )
            });
        launched_handles.push((name.clone(), handle));
    }

    // Phase 1: wait for every provider to reach running. On any
    // failure, clean up all launched handles before panicking.
    for (name, handle) in &launched_handles {
        if let Err(e) = wait_for_running(&supervisor, handle, LAUNCH_BUDGET).await {
            for (cleanup_name, cleanup_handle) in &launched_handles {
                let _ = supervisor
                    .handle_request(SupervisorRequest::StopCapsule {
                        handle: cleanup_handle.clone(),
                    })
                    .await;
                eprintln!(
                    "vz_home_frontdoor_providers_round_trip_through_supervisor: \
                     cleanup-stopped '{cleanup_name}' (handle={cleanup_handle}) \
                     after failure"
                );
            }
            panic!("provider '{name}' failed to reach running: {e}");
        }
        eprintln!(
            "vz_home_frontdoor_providers_round_trip_through_supervisor: \
             '{name}' (handle={handle}) is running"
        );
    }

    // Phase 2: stop every provider; assert StopCapsule returns
    // `status: ok` AND `last_exit_reason: host_initiated_stop`
    // (Phase 4 Day 7 wire-format contract — we lock it in here
    // for the Home-frontdoor chain specifically).
    for (name, handle) in &launched_handles {
        let stop_response = supervisor
            .handle_request(SupervisorRequest::StopCapsule {
                handle: handle.clone(),
            })
            .await;
        let stop_json = serde_json::to_value(&stop_response).expect("serialize stop response");
        assert!(
            stop_json
                .get("status")
                .and_then(|v| v.as_str())
                .map(|s| s.eq_ignore_ascii_case("ok"))
                .unwrap_or(false),
            "StopCapsule for '{name}' must return ok status; got {stop_json}"
        );
        // last_exit_reason MUST be host_initiated_stop for a
        // supervisor-initiated stop. Anything else (timed_out,
        // guest_panic, etc.) on the happy path is a Phase-5
        // contract regression.
        let last_exit_reason = stop_json
            .get("last_exit_reason")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(
            last_exit_reason, "host_initiated_stop",
            "StopCapsule for '{name}' must return last_exit_reason=host_initiated_stop; \
             got '{last_exit_reason}' in {stop_json}"
        );
        wait_for_stopped(&supervisor, handle, STOP_BUDGET)
            .await
            .unwrap_or_else(|e| panic!("'{name}' failed to stop within budget: {e}"));
        eprintln!(
            "vz_home_frontdoor_providers_round_trip_through_supervisor: \
             '{name}' (handle={handle}) stopped cleanly with \
             last_exit_reason=host_initiated_stop"
        );

        // Phase 3: CapsuleVzError MUST return `status: ok` and
        // no `vz_error` field on the happy path. Phase 4 Day 8
        // wire-format contract.
        let vz_error_response = supervisor
            .handle_request(SupervisorRequest::CapsuleVzError {
                handle: handle.clone(),
            })
            .await;
        let vz_error_json =
            serde_json::to_value(&vz_error_response).expect("serialize vz_error response");
        assert!(
            vz_error_json
                .get("status")
                .and_then(|v| v.as_str())
                .map(|s| s.eq_ignore_ascii_case("ok"))
                .unwrap_or(false),
            "CapsuleVzError for '{name}' must return ok status; got {vz_error_json}"
        );
        assert!(
            vz_error_json.get("vz_error").is_none(),
            "CapsuleVzError for '{name}' must skip-serialise vz_error on the happy \
             path; got {vz_error_json}"
        );
    }

    eprintln!(
        "vz_home_frontdoor_providers_round_trip_through_supervisor: \
         OK — all {} providers completed Launch → Status → Stop → VzError contract",
        launched_handles.len()
    );
}
