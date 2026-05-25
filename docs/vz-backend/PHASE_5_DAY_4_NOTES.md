# Phase 5 — Day 4 — Wire `prune_stale_mac_artifacts` into supervisor startup + one-shot orphan telemetry

> **Status:** Complete. One commit, push deferred.
>
> **Plan reference:** [`PHASE_5_PLAN.md` § Day 4](PHASE_5_PLAN.md#day-4--wire-prune_stale_mac_artifacts-into-supervisor-startup-46-h).
>
> **Anchors:** [`PHASE_4_DAY_5_NOTES.md`](PHASE_4_DAY_5_NOTES.md) (the original opt-in helper), [`PHASE_4_DAY_7_NOTES.md`](PHASE_4_DAY_7_NOTES.md) (the `last_exit_reason` skip-serialise wire pattern this day reuses), [`PHASE_4_DAY_8_NOTES.md`](PHASE_4_DAY_8_NOTES.md) (the `vz_error` field this day's `orphans_pruned` sits next to on the wire).

---

## 1. What shipped

### 1.1 New public types

| Type | Crate | Purpose |
|---|---|---|
| `OrphanCounts` | `elastos-server::supervisor` | Operator-facing JSON projection of `StaleArtifactCounts`. `serde::Serialize` + `Deserialize`. Three integer fields: `overlays_removed`, `sockets_removed`, `bridge_sockets_removed`. `From<StaleArtifactCounts>` for ergonomic conversion. `is_zero()` convenience. |
| `SupervisorResponse::orphans_pruned: Option<OrphanCounts>` | `elastos-server::supervisor` | One-shot field surfaced on the FIRST `EnsureCapsule` response after `Supervisor::new`. `#[serde(skip_serializing_if = "Option::is_none")]` so legacy dashboards keep working unchanged. |
| `VzConfig::prune_orphans_on_startup: bool` (default `true`) | `elastos-vz::config` | Operator opt-out switch for the Mac-only startup orphan prune. Companion builder: `with_prune_orphans_on_startup(enabled)`. |
| `Supervisor::new_with_vz_config(data_dir, registry, vz_config)` | `elastos-server::supervisor` | Non-default supervisor constructor used by tests + future operator harnesses that need to override the Vz config (currently: only the `prune_orphans_on_startup` flag). `Supervisor::new` delegates to this with `VzConfig::new()`. |
| `Supervisor::vz_config()` | `elastos-server::supervisor` | Read-only accessor used by tests to assert the round-trip. |

### 1.2 `StaleArtifactCounts` extended: socket category split

The original Phase-4-Day-5 struct counted both `*.sock` (crosvm-style control) and `*-carrier.sock` (carrier-bridge IPC) under a single `sockets_removed` field. Day 4 splits these into:

- `sockets_removed` — generic `*.sock` files (control-socket orphans).
- `bridge_sockets_removed` — `*-carrier.sock` files specifically.

**Why:** operator alerting on a sustained non-zero carrier-bridge orphan rate has a different root cause (bridge teardown bug, Phase 4 Day 2 surface) than a sustained non-zero control-socket rate (supervisor SIGKILL during launch). One number can't carry both signals.

The unit test `prune_stale_mac_artifacts_removes_overlays_and_sockets_but_preserves_unrelated_files` was updated to assert the 1+1 split it produces from the (1 carrier, 1 control) input it already had.

### 1.3 `Supervisor::new` wires the prune

```text
Supervisor::new(data_dir, registry)
  └─→ new_with_vz_config(data_dir, registry, VzConfig::new())
        └─→ #[cfg(target_os = "macos")]
              if vz_config.prune_orphans_on_startup {
                  let counts = prune_stale_mac_artifacts(
                      &crosvm_config.socket_dir,
                      &crosvm_config.rootfs_cache_dir,
                  );
                  tracing::info!(
                      overlays_removed = counts.overlays_removed,
                      sockets_removed = counts.sockets_removed,
                      bridge_sockets_removed = counts.bridge_sockets_removed,
                      "vz: startup orphan-prune complete"
                  );
                  Some(OrphanCounts::from(counts))   // cached for one-shot
              } else {
                  None
              }
            #[cfg(not(target_os = "macos"))]
              None                                    // Linux: byte-identical no-op
```

The cached `Option<OrphanCounts>` lives in `Supervisor::pending_orphan_report: std::sync::Mutex<Option<OrphanCounts>>` and is consumed by the `EnsureCapsule` handler via `Supervisor::take_pending_orphan_report()`. **One-shot:** the first take returns `Some(report)`; every subsequent take returns `None`. Dashboards alert on field presence as the "supervisor just restarted + cleaned" signal.

`std::sync::Mutex` (not `tokio::sync::RwLock`) because the slot is single-writer (filled in `new`) and single-reader-on-take. Holding it across an `await` is impossible — we never read it while waiting for I/O.

### 1.4 `SupervisorResponse::orphans_pruned` plumbed into `EnsureCapsule`

```rust
SupervisorRequest::EnsureCapsule { name } => match self.ensure_capsule(&name).await {
    Ok(path) => SupervisorResponse::ok_with_path(path.display().to_string())
        .with_orphans_pruned(self.take_pending_orphan_report()),
    Err(e) => SupervisorResponse::err(format!("ensure_capsule failed: {e}")),
},
```

`with_orphans_pruned(Option<OrphanCounts>)` is the new chainable setter on `SupervisorResponse`. Other response builders (`ok_with_handle`, `ok_with_exit_reason`, `ok_with_vz_error`, `not_found`, `err`) all default the field to `None` via `..Self::ok()` or explicit struct-literal initialisers.

### 1.5 Tests landed

| Test | Location | What it pins |
|---|---|---|
| `vz_config_default_prune_orphans_on_startup_is_true` | `elastos-vz/src/config.rs` | The default flag value — guards against accidental `false` regressions. |
| `vz_config_with_prune_orphans_on_startup_round_trip` | `elastos-vz/src/config.rs` | Builder method round-trip in both directions. |
| `orphan_counts_projection_round_trip_from_stale_artifact_counts` | `elastos-server/src/supervisor.rs` | `From<StaleArtifactCounts>` projects field-by-field; `is_zero()` true only when all three are zero. |
| `prune_stale_mac_artifacts_removes_overlays_and_sockets_but_preserves_unrelated_files` (updated) | `elastos-server/src/supervisor.rs` | The new socket category split: 1 bridge + 1 control from the existing (1 carrier, 1 control) input. |
| `fresh_supervisor_auto_prunes_orphans_on_startup_by_default` | `elastos-server/src/supervisor.rs` (Mac-only) | Default `Supervisor::new` cleans seeded orphans, cached `OrphanCounts` matches the 1/1/1 split, second take returns `None` (one-shot), idempotent re-prune returns zeros. |
| `supervisor_new_with_prune_orphans_on_startup_false_preserves_artifacts` | `elastos-server/src/supervisor.rs` (Mac-only) | Opt-out path: orphans stay on disk, cached report is `None`. |
| `supervisor_new_is_noop_on_linux_even_with_prune_flag_set` | `elastos-server/src/supervisor.rs` (Linux-only via `#[cfg(not(target_os = "macos"))]`) | Linux byte-identical contract: even with `prune_orphans_on_startup = true`, on-disk artifacts are untouched and cached report is `None`. |
| `supervisor_ensure_capsule_response_surfaces_one_shot_orphan_cleanup_report` | `elastos-server/tests/vz_supervisor_startup_orphan_cleanup.rs` (Mac-only) | Full RPC contract: seeded orphans (1/1/1) → first `EnsureCapsule` response carries the populated `orphans_pruned`; second response elides the field; JSON wire format keys present + absent respectively. |
| `supervisor_ensure_capsule_response_elides_orphan_report_when_opted_out` | `elastos-server/tests/vz_supervisor_startup_orphan_cleanup.rs` (Mac-only) | Opt-out via `handle_request`: response field absent on the wire even on the first call. |
| `supervisor_response_json_wire_format_for_orphans_pruned` | `elastos-server/tests/vz_shutdown_semantics.rs` | Wire-format triad: populated → 3 integer keys; zero-counts → 3 zero-integer keys (presence is the restart signal); `None` → field skip-serialises (legacy-dashboard compatibility). |

**Test count delta:** +9 new tests (1 updated). All green on this Mac under both `--test-threads=1` and `--test-threads=4`.

### 1.6 Wire-format anti-regression: existing `vz_shutdown_semantics.rs` constructors

Every `SupervisorResponse { ... }` struct-literal constructor in `vz_shutdown_semantics.rs` was extended with `orphans_pruned: None` to keep the file compiling against the new field. The wire-format tests for `last_exit_reason` (Phase 4 Day 7) and `vz_error` (Phase 4 Day 8) still pass unchanged — `orphans_pruned: None` skip-serialises so the previous wire-shape assertions remain true.

---

## 2. What the change buys operators

### 2.1 Crash-recovery story on Mac

Before Day 4: a `SIGKILL`'d supervisor leaves orphan `<rootfs_cache>/overlays/*.ext4` + `<socket_dir>/*.sock` + `<socket_dir>/*-carrier.sock` files behind. The next `elastos serve` boots cleanly because the supervisor's `running` map starts empty, but the on-disk junk accumulates until an operator runs `supervisor.prune_stale_mac_artifacts()` manually (which nothing does in production).

After Day 4: every `elastos serve` invocation on Mac auto-cleans its data dir's overlay + socket orphans, logs the per-category counts (`tracing::info!`), and surfaces them on the first `EnsureCapsule` response so dashboards can alert on:

- **Field presence** — "supervisor just restarted within the last N seconds."
- **`overlays_removed > 0`** — supervisor died mid-launch; investigate launch path.
- **`bridge_sockets_removed > 0`** — carrier-bridge teardown bug; the Phase-4-Day-2 surface.
- **Sustained `> 0`** rates across multiple restarts — pathological loop.

### 2.2 Operator opt-out

The dual-supervisor edge case Phase 4 Day 5 called out (two `elastos serve` instances against the same `data_dir`) is preserved via `VzConfig::with_prune_orphans_on_startup(false)`. Tests verify both that the opt-out preserves on-disk artifacts and that the response field stays absent.

Use case: CI harnesses that need to reuse a `data_dir` across multiple supervisor lifetimes for fixture sharing.

### 2.3 Linux byte-identical guarantee

The `Supervisor::new` change is gated behind `#[cfg(target_os = "macos")]`. Linux: zero code reachable, zero filesystem operations, zero new RPC field on the wire (`orphans_pruned: None` always, skip-serialises). The Linux-only test `supervisor_new_is_noop_on_linux_even_with_prune_flag_set` is the contract guard.

---

## 3. Carry-forward findings (no scope expansion)

### 3.1 Persistent orphan-history accounting (deferred to Phase 6)

Day 4 surfaces the orphan-prune outcome of the **current** supervisor startup. It does not record a history across restarts. An operator chasing an intermittent orphan-leak bug has to scrape `tracing::info!` logs and correlate by timestamp.

The Phase-4 Day-8 deferral ("persistent state for orphan history across supervisor restarts") still stands. Phase 6 would add a `<data_dir>/orphan-history.jsonl` append-only log + a CLI surface (`elastos vz-orphan-history`) reading from it. **Not in Phase 5 scope.**

### 3.2 `tracing::info!` is the only structured log emission

The Day 4 prune writes `tracing::info!(overlays_removed = …, sockets_removed = …, bridge_sockets_removed = …, "vz: startup orphan-prune complete")`. CI dashboards picking this up will already get a structured event with the three integer fields.

The `cross_platform_alert_on_vz_error_in_logs` helper from Day 3 alerts on `VzError::Display` kind-label tokens (`vz_*:`); it does NOT alert on `vz:` info messages by design — startup-prune events are informational, not alerts. Operators wanting "supervisor restart" alerts pivot on the new `orphans_pruned` JSON field instead (Datadog/Grafana JSON ingest, not log grepping).

### 3.3 No CI runner work yet

Day 4 ships the substrate. Day 5 (per the Phase 5 plan) lays down the Mac GitHub Actions runner that will exercise this auto-prune on a real macOS host. Until then the validation is local-Mac + Linux-CI for the byte-identical guarantee.

---

## 4. Operator runbook addendum

### 4.1 New JSON field on `elastos ensure <capsule>` responses

```jsonc
{
  "status": "ok",
  "path": "/Users/me/.local/share/elastos/capsules/chat",
  "orphans_pruned": {                  // ← Phase 5 Day 4 — present ONLY on first response after supervisor restart
    "overlays_removed": 2,
    "sockets_removed": 0,
    "bridge_sockets_removed": 1
  }
}
```

**Alert recipe (Datadog):** `@orphans_pruned.bridge_sockets_removed:[1 TO *]` over a 1h window → page on-call. Bridge-orphan accumulation across restarts indicates a carrier-bridge teardown regression and is the Phase-4-Day-2 surface re-emerging.

### 4.2 New `VzConfig` flag

```rust
let vz_config = VzConfig::new().with_prune_orphans_on_startup(false);
let supervisor = Supervisor::new_with_vz_config(data_dir, registry, vz_config);
```

Default: `true`. Set to `false` only when running multiple supervisors against the same data dir. Operators NEVER need to set this in `elastos serve` — the binary uses `Supervisor::new` which always defaults to `true`.

### 4.3 Manual cleanup remains available

`supervisor.prune_stale_mac_artifacts()` (the Phase 4 Day 5 surface) still works as an explicit, idempotent operator-driven sweep. After the Day 4 auto-prune, this surface returns zero counts on a freshly started supervisor; it remains useful for cleaning up artifacts that accumulated WHILE the supervisor was running (the Day 4 change only sweeps at startup).

---

## 5. Quality gates

| Gate | Status |
|---|---|
| `cargo fmt --check` (workspace) | ✓ |
| `cargo clippy --workspace --all-targets -- -D warnings` | ✓ |
| `cargo test -p elastos-server` under `--test-threads=1` | ✓ (55 lib + tests passed) |
| `cargo test -p elastos-server` under `--test-threads=4` | ✓ |
| `cargo test -p elastos-vz` | ✓ (14 config tests passed including the 2 new ones) |
| `scripts/check-linux-untouched.sh bcf5a0a` | ✓ (`elastos-common`, `elastos-compute`, `elastos-crosvm`, `elastos-runtime/src/{capability,carrier,primitives,trust}` all untouched) |
| `scripts/lib/cross-platform-test.sh` | ✓ (37 assertions, unchanged this day) |
| `ELASTOS_VZ_SMOKE_DRY_RUN=1 scripts/local-carrier-setup-smoke.sh` | ✓ |
| `ELASTOS_VZ_SMOKE_DRY_RUN=1 scripts/home-frontdoor-smoke.sh` | ✓ |
| `ELASTOS_VZ_SMOKE_DRY_RUN=1 scripts/chat-wasm-native-interop-smoke.sh` | ✓ |
| Single commit (push deferred) | ✓ |

---

## 6. Next: Day 5

Phase 5 Day 5 sets up the macOS GitHub Actions runner that will exercise the Day 1–4 deliverables against a real macOS host on every push. The 10/10 prompt for Day 5 is the next deliverable; details live in [`PHASE_5_PLAN.md`](PHASE_5_PLAN.md) § Day 5.
