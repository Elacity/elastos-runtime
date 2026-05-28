# Day 3 — Mac VZ rebase onto v0.3.0 main

> Branch: `sash/local-test-v030` (PR #3, draft)
> Date: 2026-05-28
> Phase: **The big one** — `supervisor.rs` + `vm_provider.rs` + `carrier_bridge.rs`
> 3-way reconciliation, plus the new Mac-only command surfaces
> (`doctor_cmd`, `vm_debug_cmd`) and the supervisor-dependent test suite.

## Goal

Land every `elastos-server` source change Mac VZ originally carried on top of
the v0.3.0 main baseline, layering v0.3.0's surgical changes onto Mac VZ's
larger Mac-substrate additions where both sides edited the same file.

This is the largest single rebase day of the four — the supervisor,
vm-provider, and carrier-bridge files all have non-trivial overlap on both
sides, and the Mac VZ test suite couldn't compile until those files agreed
on a single set of types and signatures (`BridgeContext.principal_id` /
`data_dir` / `on_terminate`, `LaunchCapsule { principal_id }`,
`launch_capsule(name, config, principal_id)`,
`start_capsule_vm_macos(..., principal_id)`).

End-of-day acceptance: workspace clean under `cargo check` /
`cargo clippy --workspace --tests -- -D warnings` / `cargo fmt --all -- --check`,
all 60 supervisor unit tests pass on Linux **and** Mac, branch pushed
to origin to refresh CI on the draft PR.

## What landed

### `elastos-server` source — 12 files

| File | Strategy | Notes |
|------|----------|-------|
| `setup.rs` | Mac VZ base + v0.3.0 surgical | Mac VZ adds `darwin` token in `detect_platform`, Phase 8 D5/D6 (`ensure_standalone_capsule_metadata`, `ensure_overlay_initrd`), `pub(crate)` on `load_manifest` so `doctor_cmd` can call it. v0.3.0 contributes the blockchain profile messages, comment text on `capsules`, `load_manifest` source-checkout-first ordering, `test_load_manifest_finds_current_manifest` rename, blockchain assertion. |
| `supervisor.rs` | Mac VZ base + v0.3.0 surgical | Mac VZ adds the +3,800 lines of Mac-substrate work (Mac path detection, `prune_stale_mac_artifacts`, `start_capsule_vm_macos`, `build_vm_config_for_mac`, `OrphanCounts`/`StaleArtifactCounts`, `register_provider_route_with_vsock_dialer`, the Vz error/exit-reason surfacing, the `vz_stubs` cross-platform shim). v0.3.0 contributes the `principal_id: Option<String>` field on `SupervisorRequest::LaunchCapsule`, the matching parameter on `launch_capsule`, the `manifest.role.is_shell_launchable()` guard, the `try_download_capsule_via_carrier` → `try_download_capsule_via_content_provider` rename, `ipfs_cat_via_provider` → `content_fetch_via_provider` (now using `crate::content::fetch_bytes_via_provider`), `peer-provider` → `carrier-service` test fixture rename, `TEST_SUPERVISOR_CID`, `registry_with_content_provider` helper, and the new `test_launch_capsule_rejects_principal_for_provider_role` test. The two BridgeContext init sites (Linux launch path + `start_capsule_vm_macos`) now thread `principal_id` and `data_dir: Some(self.data_dir.clone())` into the bridge. `start_capsule_vm_macos` itself takes the `principal_id` parameter so the Mac path matches the Linux path. |
| `vm_provider.rs` | Mac VZ base + v0.3.0 surgical | Mac VZ adds `MacVsockDial`, `new_with_vsock_dialer`, the +544 lines of Phase 3 Day 6 vsock integration tests. v0.3.0 contributes the split of `VM_PROVIDER_READ_TIMEOUT` into `VM_PROVIDER_DEFAULT_READ_TIMEOUT` (15s) + `VM_PROVIDER_LAUNCH_READ_TIMEOUT` (300s), the `read_timeout` parameter on `send_line_and_read_json`, `read_timeout_for_request` (per-op routing), and the `Users/self` → `MyWebSite` test rename. |
| `carrier_bridge.rs` | Mac VZ base + v0.3.0 fields only | Mac VZ adds the +539 lines of Phase 4 Day 6 `on_terminate: Option<Arc<Notify>>` lifecycle observability, the dedicated socketpair `spawn_carrier_bridge_on_stream` entry point, the WASM-stdio `spawn_wasm_carrier_bridge`. v0.3.0 contributes the `principal_id: Option<String>` and `data_dir: Option<PathBuf>` fields on `BridgeContext`. **Deferred to Day 4**: v0.3.0's full principal-aware logic (`scope_current_user_alias`, `protected_principal_root_carrier_response`, `principal_root_read_write_uri`, the principal-rooted localhost-fs path resolution). The fields are wired through, but the read/write paths still operate in legacy non-principal-aware mode until Day 4 finishes the merge. |
| `runtime_control.rs` | v0.3.0 base + Mac VZ surgical | v0.3.0 base (we branched off it) already has the +231 lines of managed-runtime startup locking (`acquire_managed_runtime_start_lock`, `wait_for_managed_runtime_ready`, `terminate_sibling_managed_runtime_children`). Mac VZ contributes only the portable `pid_is_alive(pid)` helper using `kill(pid, 0)` instead of `/proc/<pid>/exists`, replacing four call sites in `read_runtime_coords` and `terminate_managed_chat_runtime`. |
| `vm_provider.rs` | (above) | |
| `runtime.rs` | v0.3.0 base + Mac VZ surgical | One-field tweak: `BridgeContext.on_terminate: None` on the WASM-stdio bridge spawner (legacy callers — Day 4 may revisit if the WASM bridge gains lifecycle observability). |
| `home_cmd.rs` | Mac VZ base + v0.3.0 surgical | Mac VZ adds the +85 lines of HomeApp UI + dynamic-action policy. v0.3.0 contributes the chain/wallet/drm/rights/key/decrypt provider names in `PROVIDER_CAPSULE_NAMES`, the `PC2Host` removal, and the `Users/self/.AppData/...` → `Users/<principal-root>/.AppData/...` documentation update. |
| `run_cmd.rs` | Mac VZ base + v0.3.0 surgical | Mac VZ adds the +372 lines of Mac-aware `elastos run` flow. v0.3.0 contributes the `get_ipfs_bridge` → `get_content_registry` + `prepare_capsule_from_cid` → `prepare_capsule_from_content_provider` migration. |
| `main.rs` | Mac VZ base + v0.3.0 surgical | Mac VZ adds Mac-specific module wiring and platform detection. v0.3.0 contributes `mod content_cmd`, the `Commands::Content(content_cmd::ContentCommand)` variant + match arm, the explicit split of `get_ipfs_bridge` into `get_operator_ipfs_bridge` (low-level operator) + `get_content_registry`/`get_content_registry_with_local_ipfs_backend` (the canonical capsule path), `start_public_share_tunnel` → `start_operator_public_share_tunnel`, `print_share_open_warnings(content_registry)` (now uses `content::fetch_bytes_via_provider`), `host_helpers: infra.host_helpers` on `serve_web_capsule`, the blockchain mention in `--profile` help, and the publish/share/open/shares command description text updates ("Publish a capsule through the content availability provider", etc.). |
| `lib.rs` | overlay | Add `pub mod doctor_cmd;` so `elastos doctor` finds the new entry point. |
| `doctor_cmd.rs` | NEW (Mac VZ) | Phase 5 `elastos doctor` — runtime install/health diagnostics, depends on `Supervisor` + `setup::{detect_platform, load_manifest, PlatformInfo, ComponentsManifest}`. |
| `vm_debug_cmd.rs` | NEW (Mac VZ) | Phase 2 Day 4 `elastos vm-debug boot` — direct `VzProvider` boot bypass for ephemeral kernel/rootfs validation. Manifest now sets `authority: None` to match v0.3.0's `CapsuleManifest` schema. |

### Tests — 8 new files

All under `elastos/crates/elastos-server/tests/` and copied verbatim from
the archive, with the v0.3.0 schema/struct deltas applied at the call sites:

- `common/mod.rs`: shared synthetic-capsule seeder. `CapsuleManifest`
  initializer now includes `authority: None`.
- `capability_concurrency.rs`: capability-concurrency stress test
  (re-staged unchanged — already only references public APIs that survived
  the rebase).
- `vz_supervisor_smoke.rs` / `vz_home_frontdoor_smoke.rs`:
  `SupervisorRequest::LaunchCapsule` initializer now includes
  `principal_id: None`.
- `vz_shutdown_semantics.rs`: two `BridgeContext` initializers now include
  `principal_id: None` and `data_dir: None`.
- `vz_chat_interop_smoke.rs` / `vz_perf_harness.rs` /
  `vz_supervisor_startup_orphan_cleanup.rs`: re-staged unchanged — they
  only reference public APIs that survived the rebase.

### Docs — none new

The Phase 0–10 doc tree is already in place (Day 2). `docs/PC2_CONVERGENCE.md`
deliberately stays on v0.3.0's "Convergence Plan" version (the archive
copy was the older v0.2.0 vision draft and is intentionally superseded).

## What did **not** land today (deferred to Day 4)

1. **`carrier_bridge.rs` principal-aware logic.** The `principal_id` /
   `data_dir` fields on `BridgeContext` are now wired in, but the actual
   v0.3.0 logic that consumes them — `scope_current_user_alias`,
   `principal_root_read_write_uri`, `protected_principal_root_carrier_response`,
   `rooted_localhost_fs_path` — is not yet ported. Today the supervisor
   sets the fields and `carrier_bridge::handle_request` ignores them.
   Net effect: v0.3.0's principal-rooted localhost-fs scoping behaves like
   v0.2.0 (flat-rooted) on this branch until Day 4. No regression vs the
   archive (which never had the v0.3.0 logic to begin with), and no
   regression vs v0.3.0's behavior when no principal is set.
2. **Final sign-off pass:** workspace-wide manual review for "Mac-only
   surface accidentally cfg-gated to Linux" or "Linux-only surface
   accidentally bled into Mac". Walking the diff start-to-end is a Day 4
   activity once everything compiles.
3. **CI re-baseline of `linux-untouched.yml`:** the gate's
   `VZ_BACKEND_BASELINE` was set to Day-2's HEAD (`ded1333`). After Day 3,
   only `elastos-server` changes, so the gate still holds — but Day 4's
   final commit will probably want to re-baseline once more so future
   commits are evaluated against the rebased branch's actual final state,
   not Day-2-of-the-rebase.
4. **The 6 Mac CI test failures we saw locally** in
   `gateway::gateway_tests::gateway_browser_route_tests::*`. These are
   pre-existing v0.3.0 cross-OS issues — same root cause as the 14
   failures we documented at the end of Day 2. Linux CI passes them; Mac
   CI does not. Out of scope for the rebase. To be raised with Anders
   alongside the Day 2 finding for a project-level decision (cfg-gate to
   Linux / fix Mac compatibility / accept Mac CI as informational).

## Local validation

```text
$ cargo check --workspace --tests
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.75s

$ cargo clippy --workspace --tests -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 11.98s

$ cargo fmt --all -- --check
(silent — clean)

$ cargo test -p elastos-server --lib supervisor::
test result: ok. 60 passed; 0 failed; 0 ignored; 0 measured; 593 filtered out.
```

The 60 passing supervisor tests include:

- v0.3.0's new `test_launch_capsule_rejects_principal_for_provider_role`
- v0.3.0's renamed `test_content_fetch_via_provider_uses_content_contract`
  / `test_content_fetch_via_provider_surfaces_provider_error`
- Mac VZ's
  `start_capsule_vm_macos_seam_surfaces_vz_validation_error_after_phase3_day3`
- Mac VZ's
  `start_capsule_vm_macos_fails_closed_when_guest_network_lacks_entitlement`
- Mac VZ's
  `prune_stale_mac_artifacts_removes_overlays_and_sockets_but_preserves_unrelated_files`
- Mac VZ's `stop_capsule_proceeds_immediately_when_bridge_termination_notify_fires`
- Mac VZ's `mac_supervisor_picks_up_installed_initrd_as_default`
- Mac VZ's `fresh_supervisor_auto_prunes_orphans_on_startup_by_default`

## CI signal

Push to origin will trigger:

- `Linux Untouched Guarantee` — should pass (Day 3 only modifies
  `elastos-server`, which is not protected; protected crates
  `elastos-crosvm`/`elastos-runtime`/`elastos-common`/`elastos-compute`
  remain at the Day 2 baseline `ded1333`).
- `CI / cargo build + cargo test` (Linux) — should pass; the supervisor
  surface change is byte-compatible on Linux.
- `Mac VZ` (self-hosted Mac) — should pass clippy/fmt; the
  `elastos-server` lib + bin compiles, the new tests link, and supervisor
  unit tests run green. The 6 pre-existing v0.3.0 Mac cross-OS failures
  will surface — same group of `gateway_browser_route_tests` we already
  flagged on Day 2 (under different test names since v0.3.0's gateway
  refactor renamed many of them, but same underlying class of issue:
  v0.3.0's gateway/browser path leaks Linux-isms into platform-agnostic
  tests).

## Next (Day 4)

- Reconcile `carrier_bridge.rs` v0.3.0 principal-aware logic into the Mac
  VZ base.
- Final workspace walk-through: diff `sash/local-test-v030` against
  `archive/local-test-pre-v030-rebase` for every Mac VZ source surface and
  confirm no Mac-only edit was lost or accidentally Linux-gated.
- Re-baseline `linux-untouched.yml` to the Day-3 HEAD so the gate compares
  future commits against "Mac VZ rebase complete" rather than "Mac VZ
  rebase mid-flight".
- Run the cross-platform smoke scripts under `scripts/lib/` against both
  the Linux crosvm path and the Mac Vz path on the dev box, document
  results, and present the rebase to Anders as ready for v0.3.1 review.
- Compose the Anders message for the Mac VZ branch (mirroring the CVE
  rebase message) — same "branch is now against v0.3.0 main" framing.
