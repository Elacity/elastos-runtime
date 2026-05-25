# Vz vs crosvm — Performance Baseline

> **Phase 5 Day 7.** Honest baseline-or-skip document for the
> Phase-4/5 Rust code paths the Vz (Mac) and crosvm (Linux)
> backends share. Real microVM-boot timings are Phase-6-gated
> on darwin-arm64 release metadata; this document ships the
> measurement substrate, captures every cell we honestly can
> today, and pre-bakes a comparison table for Phase 6 to fill
> in real-boot numbers without restructuring.

---

## 1. TL;DR

| What | Status |
|---|---|
| **Synthetic Rust-path measurements (orphan-cleanup, RPC dispatch, capability validate)** | **Measured today**, see § 5. Mac/M-series numbers below. |
| **Real Vz microVM boot latency** | **NOT measured.** Blocker: `components.json` lacks darwin-arm64 release metadata (Phase 6). |
| **Real crosvm microVM boot latency** | **NOT aggregated** by this baseline doc — works locally today, but the harness exercises Rust-level paths only. Phase-6 follow-up. |
| **Apples-to-apples Mac vs Linux comparison** | **Partial today** (synthetic paths only). Full picture requires Phase 6. |

## 2. Methodology

- **Runs:** 5 per `scripts/measure-{vz,crosvm}-baseline.sh` invocation.
- **Sample counts per run:** 5 (cold) / 20 (warm + launch) / 100 (RPC + validate).
- **Aggregation:** median of per-run percentiles (NOT mean — protects against single-outlier runs without throwing away long-tail signal).
- **Latency unit:** microseconds throughout. All values are wall-clock from inside the test harness.
- **No warm-up suppression** for `supervisor_new_cold` — cold-start IS the metric.
- **Host capture:** OS + arch + logical CPU count + Rust toolchain version + Phase tag, embedded in every JSONL line and the aggregated baseline.
- **Schema versioning:** the on-disk JSON carries `schema_version: 1`. Future schema bumps must update every consumer (`scripts/measure-*-baseline.sh`, the future Phase-6 regression detector).

## 3. What we measure today

| Metric | What it times | Phase-4/5 code path |
|---|---|---|
| `supervisor_new_cold` | First `Supervisor::new` in a process (one per `elastos serve`). Includes the Phase-5-Day-4 Mac-only orphan-cleanup pass. | `elastos-server::supervisor::Supervisor::new` + (Mac) `prune_stale_mac_artifacts`. |
| `supervisor_new_warm` | Subsequent `Supervisor::new` calls in the same process — exercises the no-orphan-found path (steady state). | Same as cold; the directory walks find an empty tree. |
| `synthetic_capsule_launch` | `EnsureCapsule` RPC round-trip against an on-disk pre-seeded synthetic capsule. **Does NOT boot a microVM** (the on-disk cache-metadata bypasses the IPFS-download path; the resolver returns Ok). | `Supervisor::handle_request(EnsureCapsule)` → cache check → response builder. |
| `provider_registry_send_raw_single` | Single-sender, single-receiver `ProviderRegistry::send_raw` round-trip through an echo provider. | `elastos-runtime::provider::ProviderRegistry::send_raw`. The Phase-5-Day-3 chat-interop dispatch graph. |
| `provider_registry_send_raw_concurrent` | 4 senders × 25 messages = 100 messages through the registry under concurrent load. Reports the per-call latency distribution AND the total wall-clock. | Same path, exercises the read-lock fan-out from the Phase-4-Day-3 contention audit. |
| `capability_manager_validate` | `CapabilityManager::validate` against a pre-seeded token store. | `elastos-runtime::capability::CapabilityManager::validate`. The bridge dispatch hot path from Phase 4 Day 3. |

## 4. What we cannot measure yet

| Surface | Why deferred | Unblock condition |
|---|---|---|
| Real Vz microVM boot latency (handle-mint → `capsule_status: running`) | `components.json` has no darwin-arm64 release metadata; the smokes visibly-skip on Mac pre-flight today. | Phase 6 lands the darwin-arm64 release pipeline (signing, notarisation, release metadata). |
| Real cross-VM RPC over vsock | Requires a real microVM. | Same as above. |
| Real bridge teardown latency | Requires a real microVM. | Same as above. |
| Performance regression detection in CI | Needs a stable baseline-history store + alert thresholds. Day-7 ships the on-disk JSON; Phase 6 wires the detector. | Self-hosted runner online (Day 6) + Phase-6 CI hooks. |
| Multi-host fleet aggregation | One host, one number is enough for today's honesty bar. | Phase 6+. |
| Cold-start with NSFileHandle pressure (the "Apple's lazy framework load adds ~700 ms" suspicion called out in PLAN.md L308) | Requires booting an actual Vz machine. | Phase 6. |

## 5. Initial baseline (Mac, M-series)

> **Captured:** 2026-05-25. Host: macOS / aarch64 (M-series Mac, 18 logical CPUs).
> **Sample size:** 5 runs, sample counts per run as documented in § 3.
> **Source of truth:** `elastos/target/vz-baseline.json` (regenerate via `scripts/measure-vz-baseline.sh`).

| Metric | samples / run | median p50 (µs) | median p95 (µs) | median p99 (µs) | median max (µs) |
|---|---:|---:|---:|---:|---:|
| `capability_manager_validate` | 100 | 29 | 34 | 38 | 45 |
| `provider_registry_send_raw_concurrent` (per-call) | 100 | 5 | 11 | 13 | 14 |
| `provider_registry_send_raw_single` | 100 | 0 | 0 | 0 | 1 |
| `supervisor_new_cold` | 5 | 7 | 31 | 31 | 31 |
| `supervisor_new_warm` | 20 | 3 | 4 | 4 | 4 |
| `synthetic_capsule_launch` | 20 | 47 | 73 | 93 | 93 |

### Observations (Mac side, isolated)

- **All Phase-4/5 synthetic paths run sub-millisecond at p99.** No path crosses 100 µs. Confidence: high that the Mac substrate doesn't add hidden overhead to the Rust-level dispatch graph itself.
- **`supervisor_new_cold` ≈ 4× `supervisor_new_warm` at p50** (7 vs 3 µs). The delta is the Day-4 orphan-cleanup directory walk against an empty tree — i.e. the floor cost of opening and reading two empty dirs. Tracks.
- **`send_raw_single` is at the timing resolution floor** (median p50 = 0 µs). The provider mutex acquire + echo response is faster than what `Instant::now()` can resolve at microsecond granularity. **Not a sign of zero work** — sign that the work fits inside one tokio task switch. A future Phase-6 work item could move to nanosecond resolution if this metric becomes load-bearing.
- **`capability_manager_validate` p99 = 38 µs.** The Phase-4-Day-3 stress test budgets 5 s for 1000 calls; today's per-call cost suggests the substrate has ~20× headroom over the existing tripwire (38 µs × 1000 ≈ 38 ms vs the 5 s budget).
- **`synthetic_capsule_launch` p50 = 47 µs.** This is the supervisor's resolver path (cache-metadata read + ComponentsManifest lookup + response build). Real microVM launches will push this into milliseconds — the Day-7 number is the floor.

## 6. Comparison template for Phase 6

> Empty cells filled in when the self-hosted Mac runner (Day 6) is provisioned with real Vz boot capability AND `scripts/measure-crosvm-baseline.sh` is run on the comparison Linux host.

| Metric | Mac (M-series) p50 | Mac (M-series) p99 | Linux (target host) p50 | Linux (target host) p99 | Delta (Mac p99 / Linux p99) | Honest cause if delta > 2× |
|---|---:|---:|---:|---:|---:|---|
| `capability_manager_validate` | 29 µs | 38 µs | _TBD_ | _TBD_ | _TBD_ | — |
| `provider_registry_send_raw_concurrent` | 5 µs | 13 µs | _TBD_ | _TBD_ | _TBD_ | — |
| `provider_registry_send_raw_single` | 0 µs | 0 µs | _TBD_ | _TBD_ | _TBD_ | — |
| `supervisor_new_cold` | 7 µs | 31 µs | _TBD_ | _TBD_ | _TBD_ | — |
| `supervisor_new_warm` | 3 µs | 4 µs | _TBD_ | _TBD_ | _TBD_ | — |
| `synthetic_capsule_launch` | 47 µs | 93 µs | _TBD_ | _TBD_ | _TBD_ | — |
| **Real microVM boot (handle → running)** | _TBD (Phase 6)_ | _TBD_ | _TBD_ | _TBD_ | _TBD_ | — |
| **Real cross-VM RPC round-trip** | _TBD (Phase 6)_ | _TBD_ | _TBD_ | _TBD_ | _TBD_ | — |
| **Real bridge teardown** | _TBD (Phase 6)_ | _TBD_ | _TBD_ | _TBD_ | _TBD_ | — |

### Rules of engagement for Phase-6 measurements

1. **Same Rust commit.** The Mac + Linux numbers MUST come from the same `git rev-parse HEAD` to be apples-to-apples. The captured JSON has `host` info but not commit SHA today — Phase 6 should add it.
2. **Same toolchain.** Both hosts on stable Rust at the same minor version. Document any deviation.
3. **No `--release` for the Day-7 numbers.** The harness runs under `cargo test` (debug profile) so these numbers are NOT release-profile-optimised. A delta > 2× between the two hosts is meaningful; a Mac-vs-Linux comparison at debug profile is honest **for tracking the substrate cost**, not for "this is what production latency looks like." Phase 6 should add a `--release`-profile lane.
4. **No comparison without a `host_load = idle` confirmation.** Both hosts should be measured with no other CPU-heavy workloads running. The harness does not enforce this; the operator does.

## 7. JSON wire format

The on-disk `target/{vz,crosvm}-baseline.json` is the canonical artefact. Schema:

```json
{
  "schema_version": 1,
  "captured_at_unix_ms": 1779693369330,
  "host": {
    "os": "macos",
    "arch": "aarch64",
    "rust_version": "unknown",
    "cpu_count_logical": 18,
    "phase": "5-day-7"
  },
  "backend": "vz",
  "notes": {
    "real_vz_boot_measured": false,
    "real_vz_boot_blocker": "Phase 6 — components.json missing darwin-arm64 release metadata"
  },
  "metrics": {
    "<metric_name>": {
      "runs": 5,
      "samples_per_run": 100,
      "median_min_us": 3,
      "median_p50_us": 29,
      "median_p95_us": 34,
      "median_p99_us": 38,
      "median_max_us": 45
    },
    ...
  }
}
```

The intermediate JSONL stream (`target/{vz,crosvm}-baseline.jsonl`) contains one line per metric per run, with the per-run percentiles. The aggregated JSON's `median_p*_us` fields are the median of the per-run percentiles across all runs.

**Schema-version contract:** `schema_version: 1` is frozen at Phase 5 Day 7. Any future change MUST bump the version AND update every consumer (the shell scripts that aggregate, the Phase-6 regression detector when wired). Field additions inside `metrics`/`host`/`notes` that are skip-serialisable do NOT need a bump.

## 8. How to regenerate

### Mac side

```sh
# Default lane (5 runs, full measurement):
bash scripts/measure-vz-baseline.sh

# Quick smoke (3 runs):
ELASTOS_VZ_PERF_RUNS=3 bash scripts/measure-vz-baseline.sh

# CI dry-run lane (parses, sources helpers, exits 0; no measurement):
ELASTOS_VZ_SMOKE_DRY_RUN=1 bash scripts/measure-vz-baseline.sh

# CI-detected automatically by `cross_platform_in_ci`:
CI=true bash scripts/measure-vz-baseline.sh   # auto dry-run

# Force full measurement even in CI (Day-6 self-hosted lane):
ELASTOS_VZ_SMOKE_FORCE_FULL=1 bash scripts/measure-vz-baseline.sh
```

### Linux side

```sh
bash scripts/measure-crosvm-baseline.sh   # writes target/crosvm-baseline.json
```

The Linux script exits cleanly with a message on macOS — same source, same harness, distinct on-disk artefacts so the Mac + Linux baselines never overwrite each other.

## 9. Anchors

- [`PHASE_5_PLAN.md`](PHASE_5_PLAN.md) § Day 7 — the original prompt + scope-deviation note.
- [`PHASE_5_DAY_7_NOTES.md`](PHASE_5_DAY_7_NOTES.md) — Day-7 outcome log.
- [`SELF_HOSTED_RUNNER_SPEC.md`](SELF_HOSTED_RUNNER_SPEC.md) — Day-6 self-hosted runner provisioning (the substrate Phase 6 plugs real boot numbers into).
- `elastos/crates/elastos-server/tests/vz_perf_harness.rs` — the measurement source of truth.
- `scripts/measure-vz-baseline.sh` / `scripts/measure-crosvm-baseline.sh` — the aggregation scripts.
- `target/vz-baseline.json` / `target/crosvm-baseline.json` — the on-disk baselines.
