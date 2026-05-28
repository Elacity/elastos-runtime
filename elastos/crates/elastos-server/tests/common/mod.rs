//! Shared test fixtures for the `elastos-server` integration
//! test suite.
//!
//! **Phase 5 Day 8** — extracted from
//! `vz_supervisor_startup_orphan_cleanup.rs` (Day 4) and
//! `vz_perf_harness.rs` (Day 7) which previously carried
//! near-identical copies of these two helpers. Centralising
//! them follows the Phase-5 DRY discipline that Days 1–3
//! established for the shell smokes (`scripts/lib/`) and
//! Day 6 + Day 7 carry-forward called out as the analogous
//! cleanup for the Rust side.
//!
//! The contract these helpers implement is exact — they
//! produce the same cache-metadata-file shape that a real
//! `ensure_capsule` run would produce, so the integration
//! tests' `EnsureCapsule` calls short-circuit the IPFS
//! download path without paying the cost of a fake fetcher.
//!
//! ## Why these are not under `tests/common/<crate>.rs`
//!
//! Each `tests/*.rs` file is its own crate, so a plain
//! `mod common;` at the head of each test file pulls in
//! this `tests/common/mod.rs` separately. That's the
//! idiomatic Rust pattern for shared test fixtures —
//! avoids the `dead_code` warnings that a single workspace
//! library member would incur for helpers not used by every
//! test file.
//!
//! ## When to add a helper here
//!
//! Only when **two or more `tests/*.rs` files would
//! otherwise duplicate it**. Single-use helpers stay in
//! their test file (Day-1 + Day-2 + Day-3 integration tests
//! have plenty that aren't shared and shouldn't be hoisted).

#![allow(dead_code)]

use std::collections::HashMap;

use elastos_common::{CapsuleManifest, CapsuleRole, CapsuleType, ResourceLimits, SCHEMA_V1};
use elastos_server::setup::{CapsuleEntry, ComponentsManifest};

/// Pre-populate a capsule on disk so `ensure_capsule`
/// short-circuits the IPFS download path. The contract
/// `ensure_capsule` actually checks is:
///   1. `<capsules_dir>/<name>/capsule.json` exists,
///   2. `.elastos-cid` matches `entry.cid`,
///   3. `entry.sha256` is empty OR `.elastos-artifact-sha256`
///      matches it.
///
/// Mirrors the cached layout produced by a previous
/// successful `ensure_capsule` run — exactly the
/// steady-state on-disk shape the Phase-5 integration tests
/// need to drive the response builder without paying the
/// download cost.
pub fn seed_cached_synthetic_capsule(
    data_dir: &std::path::Path,
    name: &str,
    cid: &str,
    description: &str,
) {
    let capsule_dir = data_dir.join("capsules").join(name);
    std::fs::create_dir_all(&capsule_dir).expect("create synthetic capsule dir");

    let manifest = CapsuleManifest {
        schema: SCHEMA_V1.into(),
        version: "0.1.0".into(),
        name: name.into(),
        description: Some(description.into()),
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
        authority: None,
    };
    let manifest_json = serde_json::to_string_pretty(&manifest).expect("serialise manifest");
    std::fs::write(capsule_dir.join("capsule.json"), manifest_json).expect("write capsule.json");
    std::fs::write(capsule_dir.join("noop"), b"").expect("write noop entrypoint");
    std::fs::write(capsule_dir.join(".elastos-cid"), format!("{cid}\n"))
        .expect("write .elastos-cid");
    std::fs::write(
        capsule_dir.join(".elastos-artifact-sha256"),
        "synthetic-test-sha\n",
    )
    .expect("write .elastos-artifact-sha256");
}

/// Build a `ComponentsManifest` whose `capsules` map
/// contains one synthetic entry. Empty `sha256` so the
/// cache-metadata check bypasses the on-disk SHA
/// comparison (matching what
/// [`seed_cached_synthetic_capsule`] writes for the
/// `.elastos-artifact-sha256` file).
pub fn synthetic_components_manifest(name: &str, cid: &str) -> ComponentsManifest {
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
