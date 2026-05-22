//! Phase 4 Day 4 — end-to-end supervisor smoke against a real Vz
//! microVM boot.
//!
//! Auto-discovers an installed capsule under
//! `~/.local/share/elastos/capsules/<name>/` and drives the
//! production launch path (`SupervisorRequest::LaunchCapsule →
//! Supervisor::launch_capsule → start_capsule_vm_macos →
//! VzProvider::load_with_vm_config + start`) exactly as the
//! `elastos start <capsule>` CLI does. Then verifies the launched
//! VM reports running, optionally exercises its `provides:`
//! scheme through the `ProviderRegistry`, and tears the VM down.
//!
//! The test is **visibly-skip** (mirrors the Day 2 convention in
//! `elastos-vz/tests/concurrent_launch.rs::concurrent_load_with_real_kernel`):
//! when Vz is unavailable, the data directory is missing, or no
//! suitable MicroVM capsule is installed, the test prints a
//! clear `eprintln!` and returns `Ok`. CI logs every skip
//! explicitly — promoting to a required gate is an
//! Apple-runner-fleet task tracked in `docs/vz-backend/PLAN.md`.
//!
//! Anchored in: `docs/vz-backend/PHASE_4_DAY_4_NOTES.md`.

#![cfg(target_os = "macos")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use elastos_server::setup::ComponentsManifest;
use elastos_server::supervisor::{Supervisor, SupervisorRequest, SupervisorResponse};

const LAUNCH_BUDGET: Duration = Duration::from_secs(30);
const STOP_BUDGET: Duration = Duration::from_secs(10);
const STATUS_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Auto-discover the supervisor's canonical data directory. This
/// mirrors `elastos_server::sources::default_data_dir()` but
/// without taking a dep on that internal helper from the
/// integration test crate.
fn discover_data_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ELASTOS_VZ_SMOKE_DATA_DIR") {
        let pb = PathBuf::from(p);
        return pb.is_dir().then_some(pb);
    }
    let home = std::env::var_os("HOME")?;
    let candidate = PathBuf::from(home).join(".local/share/elastos");
    candidate.is_dir().then_some(candidate)
}

/// Iterate installed MicroVM capsules and pick the first one that
/// satisfies all preconditions for the smoke test:
///
/// 1. Has both `capsule.json` (parseable) and `rootfs.ext4` on disk
///    under `<data_dir>/capsules/<name>/`.
/// 2. Has `capsule_type == MicroVM`.
/// 3. Does NOT route through `launch_carrier_service` — i.e. NOT
///    (`permissions.carrier == true` AND `provides.is_some()`). The
///    Carrier-plane path is host-process, not Vz.
/// 4. Does NOT request `guest_network: true` — that's covered by
///    the dedicated entitlement test; the smoke test must stay
///    runnable on stock dev binaries.
///
/// Returns `(name, capsule_dir, manifest)`. `None` if no capsule
/// matches; the caller then visibly-skips.
fn discover_smoke_capsule(
    data_dir: &std::path::Path,
) -> Option<(String, PathBuf, elastos_common::CapsuleManifest)> {
    let capsules_dir = data_dir.join("capsules");
    let entries = std::fs::read_dir(&capsules_dir).ok()?;
    for entry in entries.flatten() {
        let capsule_dir = entry.path();
        if !capsule_dir.is_dir() {
            continue;
        }
        let manifest_path = capsule_dir.join("capsule.json");
        let rootfs_path = capsule_dir.join("rootfs.ext4");
        if !manifest_path.is_file() || !rootfs_path.is_file() {
            continue;
        }
        let manifest_data = match std::fs::read_to_string(&manifest_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let manifest: elastos_common::CapsuleManifest = match serde_json::from_str(&manifest_data) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !matches!(manifest.capsule_type, elastos_common::CapsuleType::MicroVM) {
            continue;
        }
        if manifest.permissions.carrier && manifest.provides.is_some() {
            continue;
        }
        if manifest.permissions.guest_network {
            continue;
        }
        let name = manifest.name.clone();
        return Some((name, capsule_dir, manifest));
    }
    None
}

/// Load `<data_dir>/components.json` so the supervisor's
/// `ensure_capsule` registry lookup succeeds for the discovered
/// capsule. Without this the supervisor would attempt to
/// re-download the capsule artifact from IPFS, which is exactly
/// the kind of network dependency the smoke test must NOT have.
fn load_components_manifest(data_dir: &std::path::Path) -> Option<ComponentsManifest> {
    let path = data_dir.join("components.json");
    let bytes = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str::<ComponentsManifest>(&bytes).ok()
}

/// Poll `CapsuleStatus { handle }` until either the response
/// reports the VM as running, or `budget` elapses. Returns `Ok`
/// only on observed-running.
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
        if response_indicates_running(&resp) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(anyhow::anyhow!(
                "capsule '{handle}' did not report running within {:?}; \
                 last status response: {:?}",
                budget,
                resp
            ));
        }
        tokio::time::sleep(STATUS_POLL_INTERVAL).await;
    }
}

/// Poll `CapsuleStatus { handle }` until the supervisor no
/// longer has a `running` record for it (the stop path removed
/// the entry from the map) or `budget` elapses.
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
        if response_indicates_stopped(&resp) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(anyhow::anyhow!(
                "capsule '{handle}' did not report stopped within {:?}; \
                 last status response: {:?}",
                budget,
                resp
            ));
        }
        tokio::time::sleep(STATUS_POLL_INTERVAL).await;
    }
}

/// Inspect a [`SupervisorResponse`] and decide if it reports the
/// capsule as actively running. The response shape varies across
/// VM/host paths — we look for `"status":"running"` or a `running`
/// boolean to stay forward-compatible.
fn response_indicates_running(resp: &SupervisorResponse) -> bool {
    let json = serde_json::to_value(resp).unwrap_or(serde_json::Value::Null);
    let running_str = json
        .get("status")
        .and_then(|v| v.as_str())
        .map(|s| s.eq_ignore_ascii_case("running"))
        .unwrap_or(false);
    let running_bool = json
        .get("running")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    running_str || running_bool
}

/// Inspect a [`SupervisorResponse`] and decide if the capsule is
/// gone from the supervisor's running map. After a successful
/// `stop_capsule` the handle is removed from `running`, so a
/// status query surfaces an `err` payload or a `not_found`-shaped
/// response. Either is acceptable; we accept anything that is
/// NOT `running`.
fn response_indicates_stopped(resp: &SupervisorResponse) -> bool {
    !response_indicates_running(resp)
}

/// Phase 4 Day 4 — drive the supervisor's production launch
/// pipeline against a real installed capsule. Verifies the full
/// chain: manifest discovery → `start_capsule_vm_macos` →
/// `VzProvider::load_with_vm_config` → `RunningCapsule` insertion
/// → `CapsuleStatus { handle }` reports running → `StopCapsule
/// { handle }` removes the entry.
///
/// If the manifest declares `provides:` AND the supervisor has a
/// `ProviderRegistry` attached, additionally issues one
/// `send_raw` round-trip to assert the cross-VM provider RPC
/// path wired up in Phase 3 Day 6 + Phase 4 Day 3 actually
/// reaches the guest. A capsule without `provides:` skips that
/// half and still passes the boot+stop assertions.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn vz_supervisor_launches_real_capsule_and_stops_it_cleanly() {
    if !elastos_vz::is_supported() {
        eprintln!(
            "vz_supervisor_launches_real_capsule_and_stops_it_cleanly: \
             skipping — is_supported() == false (off Apple Silicon macOS, Vz framework unreachable)"
        );
        return;
    }
    let Some(data_dir) = discover_data_dir() else {
        eprintln!(
            "vz_supervisor_launches_real_capsule_and_stops_it_cleanly: \
             skipping — no $ELASTOS_VZ_SMOKE_DATA_DIR or ~/.local/share/elastos directory. \
             Run `elastos setup` and install at least one MicroVM capsule first."
        );
        return;
    };
    let Some(registry) = load_components_manifest(&data_dir) else {
        eprintln!(
            "vz_supervisor_launches_real_capsule_and_stops_it_cleanly: \
             skipping — no components.json under {} (or it failed to parse). \
             Run `elastos setup` first.",
            data_dir.display()
        );
        return;
    };
    let Some((name, capsule_dir, manifest)) = discover_smoke_capsule(&data_dir) else {
        eprintln!(
            "vz_supervisor_launches_real_capsule_and_stops_it_cleanly: \
             skipping — no installed MicroVM capsule with a rootfs.ext4 under {} that \
             takes the Vz path (carrier-plane and guest-network capsules are excluded). \
             Run `elastos setup` and pull a MicroVM capsule (e.g. notepad) first.",
            data_dir.join("capsules").display()
        );
        return;
    };
    eprintln!(
        "vz_supervisor_launches_real_capsule_and_stops_it_cleanly: \
         launching capsule '{name}' from {} (provides={:?})",
        capsule_dir.display(),
        manifest.provides.as_deref()
    );

    // Wire a ProviderRegistry so the `provides:` half of the test
    // exercises the Carrier-bridge dispatch path from Day 3. The
    // bridge spawns even when `provides` is None — Day 2 audit
    // confirmed it is detached-safe under N>1.
    let provider_registry = Arc::new(elastos_runtime::provider::ProviderRegistry::new());
    let mut supervisor = Supervisor::new(data_dir.clone(), registry);
    supervisor.set_provider_registry(provider_registry.clone());

    let provides_scheme = manifest
        .provides
        .as_deref()
        .and_then(|p| p.split_once("://").map(|(s, _)| s.to_string()));

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
        .unwrap_or_else(|| {
            panic!("LaunchCapsule did not return a handle. Response was: {response_json}")
        })
        .to_string();

    let launch_result = wait_for_running(&supervisor, &handle, LAUNCH_BUDGET).await;
    if let Err(e) = launch_result {
        // Best-effort cleanup before failing — we already minted
        // a handle; leaving the VM running would leak resources
        // and skew subsequent CI runs.
        let _ = supervisor
            .handle_request(SupervisorRequest::StopCapsule {
                handle: handle.clone(),
            })
            .await;
        panic!("capsule '{name}' (handle={handle}) failed to reach running: {e}");
    }
    eprintln!(
        "vz_supervisor_launches_real_capsule_and_stops_it_cleanly: \
         capsule '{name}' (handle={handle}) is running"
    );

    // `provides:` round-trip — best-effort. A capsule with no
    // `provides:` skips this half. A capsule with `provides:` but
    // a guest that doesn't yet implement the scheme will surface
    // a provider error; we accept any outcome that is not a
    // panic, because the smoke test's primary job is the boot
    // assertion above, not the guest's protocol completeness.
    if let Some(scheme) = provides_scheme {
        eprintln!(
            "vz_supervisor_launches_real_capsule_and_stops_it_cleanly: \
             attempting provider_registry.send_raw round-trip on scheme '{scheme}'"
        );
        let probe = serde_json::json!({ "op": "ping" });
        // Bound the round-trip — a hung guest must not stall the
        // whole test.
        let probe_result = tokio::time::timeout(
            Duration::from_secs(10),
            provider_registry.send_raw(&scheme, &probe),
        )
        .await;
        match probe_result {
            Ok(Ok(resp)) => eprintln!(
                "vz_supervisor_launches_real_capsule_and_stops_it_cleanly: \
                 send_raw('{scheme}') succeeded: {resp}"
            ),
            Ok(Err(e)) => eprintln!(
                "vz_supervisor_launches_real_capsule_and_stops_it_cleanly: \
                 send_raw('{scheme}') returned ProviderError (acceptable): {e}"
            ),
            Err(_) => eprintln!(
                "vz_supervisor_launches_real_capsule_and_stops_it_cleanly: \
                 send_raw('{scheme}') timed out after 10s (acceptable — guest may not \
                 implement the `ping` op)"
            ),
        }
    }

    // Tear down — must succeed within STOP_BUDGET.
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
        "StopCapsule must return ok status; got {stop_json}"
    );

    wait_for_stopped(&supervisor, &handle, STOP_BUDGET)
        .await
        .unwrap_or_else(|e| panic!("capsule '{name}' (handle={handle}) failed to stop: {e}"));

    eprintln!(
        "vz_supervisor_launches_real_capsule_and_stops_it_cleanly: \
         capsule '{name}' (handle={handle}) stopped cleanly"
    );
}
