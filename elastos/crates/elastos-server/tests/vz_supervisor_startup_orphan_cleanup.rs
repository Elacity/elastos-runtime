//! Phase 5 Day 4 — Mac-only integration test for the
//! supervisor's automatic startup orphan cleanup.
//!
//! Lock-in goal: validate the full Phase-5-Day-4 contract end
//! to end against the public [`Supervisor::handle_request`]
//! surface that production callers use — no in-crate
//! shortcuts, no private hooks.
//!
//! The supervisor unit tests in
//! `crates/elastos-server/src/supervisor.rs` exercise the
//! `Supervisor::new` / `take_pending_orphan_report` seam
//! directly; THIS test pins the production RPC contract that
//! `elastos serve`'s shell stub and the publisher gateway
//! actually call:
//!
//! 1. Seed a `data_dir` with stale overlay + control-socket +
//!    carrier-bridge-socket orphans.
//! 2. Construct a [`Supervisor`] with the default Vz config
//!    (which opts INTO startup pruning).
//! 3. Drive a [`SupervisorRequest::EnsureCapsule`] for a
//!    synthetic capsule registered in the `ComponentsManifest`.
//! 4. Assert the response surfaces the
//!    [`SupervisorResponse::orphans_pruned`] field with the
//!    expected category split (1/1/1).
//! 5. Drive a SECOND `EnsureCapsule` and assert the field is
//!    absent — one-shot delivery is the dashboard signal for
//!    "supervisor just started + cleaned" vs. "steady state".
//! 6. Assert the on-disk artifacts are gone (the prune ran
//!    for real, not just the field).
//!
//! The test deliberately avoids booting a real Vz microVM —
//! the orphan-prune surface is filesystem-level and runs
//! BEFORE any VM launch. `ensure_capsule` itself does not
//! launch a VM; we feed it a capsule pre-populated on disk
//! with the cache-metadata files that match the registry
//! entry, so the resolver returns Ok without going through
//! the IPFS download path.
//!
//! Anchored in: `docs/vz-backend/PHASE_5_DAY_4_NOTES.md` and
//! the `Day 4` block of `docs/vz-backend/PHASE_5_PLAN.md`.

#![cfg(target_os = "macos")]

use std::collections::HashMap;

use elastos_common::{CapsuleManifest, CapsuleRole, CapsuleType, ResourceLimits, SCHEMA_V1};
use elastos_server::setup::{CapsuleEntry, ComponentsManifest};
use elastos_server::supervisor::{Supervisor, SupervisorRequest, SupervisorResponse};

const SYNTHETIC_CAPSULE_NAME: &str = "phase5-day4-orphan-cleanup-capsule";
const SYNTHETIC_CAPSULE_CID: &str = "bafy-phase5-day4-orphan-test";

/// Pre-populate a capsule on disk so `ensure_capsule` short-circuits
/// the IPFS download path. The contract `ensure_capsule` actually
/// checks is:
///   1. `<capsules_dir>/<name>/capsule.json` exists,
///   2. `.elastos-cid` matches `entry.cid`,
///   3. `entry.sha256` is empty OR `.elastos-artifact-sha256`
///      matches it.
///
/// Mirrors the cached layout produced by a previous successful
/// `ensure_capsule` run — exactly the steady-state on-disk
/// shape Phase 5 Day 4 needs to drive the response builder.
fn seed_cached_synthetic_capsule(data_dir: &std::path::Path, name: &str, cid: &str) {
    let capsule_dir = data_dir.join("capsules").join(name);
    std::fs::create_dir_all(&capsule_dir).expect("create synthetic capsule dir");

    let manifest = CapsuleManifest {
        schema: SCHEMA_V1.into(),
        version: "0.1.0".into(),
        name: name.into(),
        description: Some("Phase 5 Day 4 orphan-cleanup integration test capsule".into()),
        author: None,
        role: CapsuleRole::App,
        capsule_type: CapsuleType::Wasm,
        entrypoint: "noop".into(),
        requires: Vec::new(),
        provides: None,
        capabilities: Vec::new(),
        resources: ResourceLimits {
            memory_mb: 64,
            cpu_shares: 100,
            gpu: false,
        },
        permissions: Default::default(),
        microvm: None,
        providers: None,
        viewer: None,
        signature: None,
    };
    let manifest_json = serde_json::to_string_pretty(&manifest).expect("serialise manifest");
    std::fs::write(capsule_dir.join("capsule.json"), manifest_json).expect("write capsule.json");
    std::fs::write(capsule_dir.join("noop"), b"").expect("write noop entrypoint");
    // Cache-metadata files that ensure_capsule consults to short-circuit
    // the IPFS download path. `.elastos-cid` must equal entry.cid;
    // `.elastos-artifact-sha256` can be anything since our entry leaves
    // sha256 empty.
    std::fs::write(capsule_dir.join(".elastos-cid"), format!("{}\n", cid))
        .expect("write .elastos-cid");
    std::fs::write(
        capsule_dir.join(".elastos-artifact-sha256"),
        "synthetic-test-sha\n",
    )
    .expect("write .elastos-artifact-sha256");
}

/// Build a `ComponentsManifest` whose `capsules` map contains
/// the synthetic entry. Empty `sha256` so the cache-metadata
/// check bypasses the on-disk SHA comparison.
fn synthetic_components_manifest(name: &str, cid: &str) -> ComponentsManifest {
    let mut capsules: HashMap<String, CapsuleEntry> = HashMap::new();
    capsules.insert(
        name.into(),
        CapsuleEntry {
            cid: cid.into(),
            sha256: String::new(),
            size: 0,
            platforms: Vec::new(),
        },
    );
    ComponentsManifest {
        external: HashMap::new(),
        capsules,
        profiles: HashMap::new(),
    }
}

/// Phase 5 Day 4 — the production RPC contract for the
/// supervisor's startup orphan cleanup.
///
/// This is the test the publisher-gateway / shell-stub
/// integration ultimately leans on: if the
/// `orphans_pruned` field stops appearing on the first
/// `EnsureCapsule` response, dashboards lose their
/// "supervisor just started" pivot.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supervisor_ensure_capsule_response_surfaces_one_shot_orphan_cleanup_report() {
    let temp = tempfile::tempdir().expect("tempdir");
    let data_dir = temp.path().to_path_buf();

    // Seed orphan artifacts: one of each category so the
    // response wire format can be asserted exactly.
    let socket_dir = data_dir.join("crosvm");
    let overlays_dir = data_dir.join("rootfs-cache").join("overlays");
    std::fs::create_dir_all(&socket_dir).expect("create socket_dir");
    std::fs::create_dir_all(&overlays_dir).expect("create overlays_dir");
    let orphan_overlay = overlays_dir.join("phase5day4-orphan.ext4");
    let orphan_control = socket_dir.join("phase5day4-orphan.sock");
    let orphan_bridge = socket_dir.join("phase5day4-orphan-carrier.sock");
    std::fs::write(&orphan_overlay, b"orphan overlay").expect("write orphan overlay");
    std::fs::write(&orphan_control, b"orphan control").expect("write orphan control");
    std::fs::write(&orphan_bridge, b"orphan bridge").expect("write orphan bridge");

    // Seed the synthetic capsule so `ensure_capsule` resolves Ok.
    seed_cached_synthetic_capsule(&data_dir, SYNTHETIC_CAPSULE_NAME, SYNTHETIC_CAPSULE_CID);
    let registry = synthetic_components_manifest(SYNTHETIC_CAPSULE_NAME, SYNTHETIC_CAPSULE_CID);

    // Construct the supervisor. The default `VzConfig` opts INTO
    // startup pruning, so the orphans must be gone by the time
    // `Supervisor::new` returns and the cached report must be
    // ready for the first `EnsureCapsule` response.
    let supervisor = Supervisor::new(data_dir.clone(), registry);

    // On-disk side-effect contract: orphans actually removed.
    assert!(
        !orphan_overlay.exists(),
        "Phase 5 Day 4: startup prune must remove the orphan overlay file"
    );
    assert!(
        !orphan_control.exists(),
        "Phase 5 Day 4: startup prune must remove the orphan control socket"
    );
    assert!(
        !orphan_bridge.exists(),
        "Phase 5 Day 4: startup prune must remove the orphan carrier-bridge socket"
    );

    // First `EnsureCapsule` → response carries `orphans_pruned`.
    let first = supervisor
        .handle_request(SupervisorRequest::EnsureCapsule {
            name: SYNTHETIC_CAPSULE_NAME.into(),
        })
        .await;
    assert_eq!(
        first.status, "ok",
        "first ensure_capsule must succeed against the seeded capsule; got {first:?}"
    );
    let first_orphans = first
        .orphans_pruned
        .expect("first ensure_capsule response MUST carry the one-shot orphans_pruned report");
    assert_eq!(
        first_orphans.overlays_removed, 1,
        "report must reflect the 1 overlay we seeded"
    );
    assert_eq!(
        first_orphans.sockets_removed, 1,
        "report must reflect the 1 generic control socket we seeded"
    );
    assert_eq!(
        first_orphans.bridge_sockets_removed, 1,
        "report must reflect the 1 carrier-bridge socket we seeded — split out from sockets_removed per Day-4 contract"
    );

    // First-response JSON wire format: `orphans_pruned` is
    // present as a nested object with the expected keys.
    // Dashboards alerting on field presence + exact category
    // hinge on this wire shape.
    let first_json = serde_json::to_value(&first).expect("first response → serde_json::Value");
    let orphans = first_json
        .get("orphans_pruned")
        .expect("wire format: orphans_pruned key must be present on first ensure_capsule response");
    assert_eq!(orphans["overlays_removed"], 1);
    assert_eq!(orphans["sockets_removed"], 1);
    assert_eq!(orphans["bridge_sockets_removed"], 1);

    // Second `EnsureCapsule` → response elides `orphans_pruned`.
    let second = supervisor
        .handle_request(SupervisorRequest::EnsureCapsule {
            name: SYNTHETIC_CAPSULE_NAME.into(),
        })
        .await;
    assert_eq!(
        second.status, "ok",
        "second ensure_capsule must also succeed; got {second:?}"
    );
    assert!(
        second.orphans_pruned.is_none(),
        "Phase 5 Day 4 ONE-SHOT contract: subsequent ensure_capsule responses MUST NOT carry orphans_pruned — field absence is the steady-state signal"
    );
    let second_json = serde_json::to_value(&second).expect("second response → serde_json::Value");
    assert!(
        second_json.get("orphans_pruned").is_none(),
        "wire format: second response must SKIP-SERIALISE orphans_pruned (legacy-dashboard compatibility) — got: {second_json}"
    );
}

/// Phase 5 Day 4 — operator opt-out path through the public
/// `handle_request` surface. When constructed with
/// `VzConfig::prune_orphans_on_startup = false`, the
/// supervisor must (a) leave orphan files in place and
/// (b) elide the `orphans_pruned` field on EVERY response,
/// including the first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supervisor_ensure_capsule_response_elides_orphan_report_when_opted_out() {
    let temp = tempfile::tempdir().expect("tempdir");
    let data_dir = temp.path().to_path_buf();

    let socket_dir = data_dir.join("crosvm");
    let overlays_dir = data_dir.join("rootfs-cache").join("overlays");
    std::fs::create_dir_all(&socket_dir).expect("create socket_dir");
    std::fs::create_dir_all(&overlays_dir).expect("create overlays_dir");
    let orphan_overlay = overlays_dir.join("opt-out.ext4");
    let orphan_bridge = socket_dir.join("opt-out-carrier.sock");
    std::fs::write(&orphan_overlay, b"keep").expect("write keep overlay");
    std::fs::write(&orphan_bridge, b"keep").expect("write keep bridge");

    seed_cached_synthetic_capsule(&data_dir, SYNTHETIC_CAPSULE_NAME, SYNTHETIC_CAPSULE_CID);
    let registry = synthetic_components_manifest(SYNTHETIC_CAPSULE_NAME, SYNTHETIC_CAPSULE_CID);

    let supervisor = Supervisor::new_with_vz_config(
        data_dir.clone(),
        registry,
        elastos_vz::VzConfig::new().with_prune_orphans_on_startup(false),
    );

    // Orphans must remain on disk.
    assert!(
        orphan_overlay.exists(),
        "opt-out: stale overlay must NOT be pruned when prune_orphans_on_startup is false"
    );
    assert!(
        orphan_bridge.exists(),
        "opt-out: stale carrier-bridge socket must NOT be pruned when prune_orphans_on_startup is false"
    );

    let response = supervisor
        .handle_request(SupervisorRequest::EnsureCapsule {
            name: SYNTHETIC_CAPSULE_NAME.into(),
        })
        .await;
    assert_eq!(
        response.status, "ok",
        "opt-out path: ensure_capsule must still succeed; got {response:?}"
    );
    assert!(
        response.orphans_pruned.is_none(),
        "opt-out: response MUST NOT carry orphans_pruned"
    );
    let response_json = serde_json::to_value(&response).expect("response → serde_json::Value");
    assert!(
        response_json.get("orphans_pruned").is_none(),
        "opt-out wire format: orphans_pruned key must be absent — got: {response_json}"
    );

    // Compile-time anti-regression: the response type still
    // carries the field even when None (silently coerce to
    // assert the struct shape is intact across the cfg gate).
    let _ack: SupervisorResponse = response;
}
