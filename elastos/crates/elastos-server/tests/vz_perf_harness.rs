//! Phase 5 Day 7 — Performance measurement substrate.
//!
//! Cross-platform synthetic perf harness for the Phase-4/5
//! Rust code paths the Vz (Mac) and crosvm (Linux) backends
//! both lean on. Each `#[test]` runs the same code under
//! `cargo test` AND emits a JSON line into the file at
//! `ELASTOS_VZ_PERF_REPORT` (if set) so
//! `scripts/measure-{vz,crosvm}-baseline.sh` can aggregate
//! across multiple runs.
//!
//! ### What we measure today (Day 7)
//!
//! Six synthetic metrics, each scoped to a Rust code path
//! that exists today on both substrates:
//!
//! 1. `supervisor_new_cold` — first `Supervisor::new` in a
//!    process (paid once per `elastos serve`). Includes the
//!    Day-4 Mac-only orphan-cleanup pass — that's NOT noise,
//!    it's exactly the startup latency operators see.
//! 2. `supervisor_new_warm` — subsequent constructions in
//!    the same process (the orphan-cleanup work scales by
//!    on-disk artifact count, not by call count, so warm
//!    runs are dominated by directory walks against an
//!    empty pruned tree — the Phase-5 steady state).
//! 3. `synthetic_capsule_launch` — `EnsureCapsule →
//!    handle_request` round-trip against a synthetic capsule
//!    seeded on disk (no real microVM boot — that's a
//!    Phase-6 unblock; see PERFORMANCE_BASELINE.md).
//! 4. `provider_registry_send_raw_single` — single-sender
//!    `ProviderRegistry::send_raw` round-trip (the dispatch
//!    graph the chat smokes exercise).
//! 5. `provider_registry_send_raw_concurrent` — 4 senders ×
//!    25 messages = 100 messages — exercises the read-lock
//!    fan-out under the Phase-4-Day-3 contention test.
//! 6. `capability_manager_validate` — `validate` against a
//!    pre-seeded token store (the bridge dispatch hot path
//!    from Phase 4 Day 3).
//!
//! ### What we cannot measure yet
//!
//! Real Vz boot latency, real cross-VM RPC over vsock, real
//! bridge teardown — all gated on Phase 6 darwin-arm64
//! release metadata in `components.json`. The harness emits
//! `notes.real_vz_boot_measured = false` and the
//! `scripts/measure-vz-baseline.sh` summary surfaces the
//! same fact prominently.
//!
//! ### JSON schema
//!
//! See `docs/vz-backend/PERFORMANCE_BASELINE.md` § JSON schema.
//! `schema_version: 1` is the wire-format contract that
//! `target/{vz,crosvm}-baseline.json` consumers (today: the
//! shell scripts; future: a CI regression-detector) lean on.
//!
//! Anchored in: `docs/vz-backend/PHASE_5_DAY_7_NOTES.md` and
//! the Day-7 block of `docs/vz-backend/PHASE_5_PLAN.md`.

#![allow(clippy::missing_docs_in_private_items)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use elastos_common::{CapsuleManifest, CapsuleRole, CapsuleType, ResourceLimits, SCHEMA_V1};
use elastos_runtime::capability::pending::{GrantDuration, PendingRequestStore};
use elastos_runtime::capability::{Action, CapabilityManager, ResourceId, TokenConstraints};
use elastos_runtime::primitives::audit::AuditLog;
use elastos_runtime::primitives::metrics::MetricsManager;
use elastos_runtime::provider::{
    Provider, ProviderError, ProviderRegistry, ResourceRequest, ResourceResponse,
};
use elastos_server::setup::{CapsuleEntry, ComponentsManifest};
use elastos_server::supervisor::{Supervisor, SupervisorRequest};

// ───────────────────────────────────────────────────────────
// Sample counts (kept low so the harness stays under a few
// seconds on the slowest dev host; statistical noise at these
// counts is well-documented in PERFORMANCE_BASELINE.md § 4).
// ───────────────────────────────────────────────────────────
const COLD_SAMPLES: usize = 5;
const WARM_SAMPLES: usize = 20;
const LAUNCH_SAMPLES: usize = 20;
const SEND_RAW_SAMPLES: usize = 100;
const CAPABILITY_VALIDATE_SAMPLES: usize = 100;
const CONCURRENT_SENDERS: usize = 4;
const CONCURRENT_MESSAGES_PER_SENDER: usize = 25;

const PERF_REPORT_ENV: &str = "ELASTOS_VZ_PERF_REPORT";
const SCHEMA_VERSION: u32 = 1;

// ───────────────────────────────────────────────────────────
// JSON-serialisable report shape.
// ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
struct MetricStats {
    samples_count: usize,
    min_us: u128,
    p50_us: u128,
    p95_us: u128,
    p99_us: u128,
    max_us: u128,
}

impl MetricStats {
    fn from_samples(samples: &[Duration]) -> Self {
        assert!(!samples.is_empty(), "metric requires ≥1 sample");
        let mut us: Vec<u128> = samples.iter().map(|d| d.as_micros()).collect();
        us.sort_unstable();
        Self {
            samples_count: us.len(),
            min_us: us[0],
            p50_us: percentile(&us, 50),
            p95_us: percentile(&us, 95),
            p99_us: percentile(&us, 99),
            max_us: us[us.len() - 1],
        }
    }
}

fn percentile(sorted: &[u128], pct: usize) -> u128 {
    // Nearest-rank percentile — well-suited to small N (we
    // explicitly want the actual sample, not an interpolated
    // value, so p99 of 5 samples returns the worst sample).
    let n = sorted.len();
    let rank = ((pct as f64 / 100.0) * (n as f64)).ceil() as usize;
    let idx = rank.saturating_sub(1).min(n - 1);
    sorted[idx]
}

#[derive(Debug, Serialize, Deserialize)]
struct HostInfo {
    os: String,
    arch: String,
    rust_version: String,
    cpu_count_logical: usize,
    phase: String,
}

impl HostInfo {
    fn capture() -> Self {
        Self {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            // The Rust toolchain version is set at build time
            // by the `RUSTC_VERSION_AT_BUILD` env var if
            // available — otherwise reports "unknown". Keeps
            // the harness free of `rustc_version`-style build
            // dependencies (per Day-7 "no new deps" budget).
            rust_version: option_env!("RUSTC_VERSION_AT_BUILD")
                .unwrap_or("unknown")
                .to_string(),
            cpu_count_logical: std::thread::available_parallelism()
                .map(std::num::NonZeroUsize::get)
                .unwrap_or(1),
            phase: "5-day-7".to_string(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ReportNotes {
    real_vz_boot_measured: bool,
    real_vz_boot_blocker: String,
}

impl ReportNotes {
    fn day_seven_default() -> Self {
        Self {
            real_vz_boot_measured: false,
            real_vz_boot_blocker: "Phase 6 — components.json missing darwin-arm64 release metadata"
                .to_string(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct PerfReport {
    schema_version: u32,
    captured_at_unix_ms: u128,
    host: HostInfo,
    backend: String,
    metric_name: String,
    stats: MetricStats,
    notes: ReportNotes,
}

fn current_backend() -> &'static str {
    if cfg!(target_os = "macos") {
        "vz"
    } else {
        "crosvm"
    }
}

fn captured_at_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Append a single-metric report as one JSON line to
/// `ELASTOS_VZ_PERF_REPORT` if it's set. JSONL keeps the
/// emitter trivially parallel-safe (multiple `#[test]`s can
/// run under `--test-threads=N` without interleaving JSON
/// objects — at worst they interleave whole lines which jq
/// `--slurp` handles cleanly).
fn maybe_emit(metric_name: &str, stats: &MetricStats) {
    let Ok(path) = std::env::var(PERF_REPORT_ENV) else {
        return;
    };
    let report = PerfReport {
        schema_version: SCHEMA_VERSION,
        captured_at_unix_ms: captured_at_unix_ms(),
        host: HostInfo::capture(),
        backend: current_backend().to_string(),
        metric_name: metric_name.into(),
        stats: stats.clone(),
        notes: ReportNotes::day_seven_default(),
    };
    let line =
        serde_json::to_string(&report).expect("perf report must serialise — schema is stable");

    use std::io::Write;
    // Best-effort append — if the file is missing or the env
    // points at an unwritable path, log and continue. The
    // harness still validates the contract via the in-process
    // assertions below.
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(mut f) => {
            if let Err(e) = writeln!(f, "{line}") {
                eprintln!("[perf-harness] failed to write report to {path}: {e}");
            }
        }
        Err(e) => {
            eprintln!("[perf-harness] cannot open report path {path}: {e}");
        }
    }
}

// ───────────────────────────────────────────────────────────
// Synthetic capsule + manifest helpers — mirror the shape
// `vz_supervisor_startup_orphan_cleanup.rs` uses so the
// perf harness measures the same code path the orphan-cleanup
// integration test pins.
// ───────────────────────────────────────────────────────────

const PERF_SYNTHETIC_CAPSULE_NAME: &str = "phase5-day7-perf-capsule";
const PERF_SYNTHETIC_CAPSULE_CID: &str = "bafy-phase5-day7-perf-test";

fn seed_cached_synthetic_capsule(data_dir: &std::path::Path, name: &str, cid: &str) {
    let capsule_dir = data_dir.join("capsules").join(name);
    std::fs::create_dir_all(&capsule_dir).expect("create synthetic capsule dir");

    let manifest = CapsuleManifest {
        schema: SCHEMA_V1.into(),
        version: "0.1.0".into(),
        name: name.into(),
        description: Some("Phase 5 Day 7 perf-harness capsule".into()),
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
    std::fs::write(capsule_dir.join(".elastos-cid"), format!("{cid}\n"))
        .expect("write .elastos-cid");
    std::fs::write(
        capsule_dir.join(".elastos-artifact-sha256"),
        "synthetic-perf-sha\n",
    )
    .expect("write .elastos-artifact-sha256");
}

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

// ───────────────────────────────────────────────────────────
// Tiny synthetic provider for `send_raw` measurements. Same
// shape as the Day-3 chat bus but minimal: echoes the request
// back. Keeps the measurement focused on the dispatch graph,
// not provider-side work.
// ───────────────────────────────────────────────────────────

#[derive(Default)]
struct EchoCounters {
    seen: usize,
}

struct EchoProvider {
    counters: Arc<Mutex<EchoCounters>>,
}

#[async_trait]
impl Provider for EchoProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "EchoProvider does not implement resource-path handle".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["perf-echo"]
    }

    fn name(&self) -> &'static str {
        "phase5-day7-perf-echo"
    }

    async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
        let mut c = self.counters.lock().await;
        c.seen += 1;
        Ok(json!({ "status": "ok", "seq": c.seen, "echo": request }))
    }
}

// ───────────────────────────────────────────────────────────
// Measurements.
//
// Each test is also a `#[test]` so `cargo test` + threads=1/4
// keep the perf-harness greenness in the CI substrate (the
// Phase-5-Day-5 `mac-rust-tests` job picks them up
// automatically).
//
// The harness deliberately scopes ONE metric per test
// function so jsonl emission stays metric-keyed. A future
// run-all harness can call the inner functions directly.
// ───────────────────────────────────────────────────────────

fn measure_supervisor_new_cold_once(data_dir: &std::path::Path) -> Duration {
    let registry =
        synthetic_components_manifest(PERF_SYNTHETIC_CAPSULE_NAME, PERF_SYNTHETIC_CAPSULE_CID);
    let started = Instant::now();
    let _supervisor = Supervisor::new(data_dir.to_path_buf(), registry);
    started.elapsed()
}

#[test]
fn perf_supervisor_new_cold() {
    let mut samples = Vec::with_capacity(COLD_SAMPLES);
    for _ in 0..COLD_SAMPLES {
        // Cold = fresh data dir per iteration. The orphan
        // cleanup walks an empty tree, so the time we capture
        // is the floor — the cost of constructing the
        // supervisor's internal state (registries, manifests,
        // pruner-mutex) on a clean install.
        let tmp = tempfile::tempdir().expect("tempdir");
        samples.push(measure_supervisor_new_cold_once(tmp.path()));
    }
    let stats = MetricStats::from_samples(&samples);
    eprintln!("perf_supervisor_new_cold: {stats:?}");
    maybe_emit("supervisor_new_cold", &stats);
    // Sanity guard: a regression that pushes cold-start past
    // 5s is a real bug. Threshold deliberately loose; the
    // measurement IS the signal, this assert is the tripwire.
    assert!(
        stats.max_us < 5_000_000,
        "supervisor_new_cold max {} µs exceeds 5s tripwire",
        stats.max_us
    );
}

#[test]
fn perf_supervisor_new_warm() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // Prime the data dir with a single `Supervisor::new` so
    // subsequent iterations measure the no-orphan-found path.
    let _ = measure_supervisor_new_cold_once(tmp.path());

    let mut samples = Vec::with_capacity(WARM_SAMPLES);
    for _ in 0..WARM_SAMPLES {
        samples.push(measure_supervisor_new_cold_once(tmp.path()));
    }
    let stats = MetricStats::from_samples(&samples);
    eprintln!("perf_supervisor_new_warm: {stats:?}");
    maybe_emit("supervisor_new_warm", &stats);
    assert!(
        stats.max_us < 5_000_000,
        "supervisor_new_warm max {} µs exceeds 5s tripwire",
        stats.max_us
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn perf_synthetic_capsule_launch() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let data_dir = tmp.path().to_path_buf();
    seed_cached_synthetic_capsule(
        &data_dir,
        PERF_SYNTHETIC_CAPSULE_NAME,
        PERF_SYNTHETIC_CAPSULE_CID,
    );
    let registry =
        synthetic_components_manifest(PERF_SYNTHETIC_CAPSULE_NAME, PERF_SYNTHETIC_CAPSULE_CID);
    let supervisor = Supervisor::new(data_dir, registry);

    let mut samples = Vec::with_capacity(LAUNCH_SAMPLES);
    for _ in 0..LAUNCH_SAMPLES {
        let started = Instant::now();
        let response = supervisor
            .handle_request(SupervisorRequest::EnsureCapsule {
                name: PERF_SYNTHETIC_CAPSULE_NAME.into(),
            })
            .await;
        samples.push(started.elapsed());
        assert_eq!(
            response.status, "ok",
            "ensure_capsule must succeed on the seeded synthetic capsule"
        );
    }
    let stats = MetricStats::from_samples(&samples);
    eprintln!("perf_synthetic_capsule_launch: {stats:?}");
    maybe_emit("synthetic_capsule_launch", &stats);
    assert!(
        stats.max_us < 5_000_000,
        "synthetic_capsule_launch max {} µs exceeds 5s tripwire",
        stats.max_us
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn perf_provider_registry_send_raw_single() {
    let registry = ProviderRegistry::new();
    let counters = Arc::new(Mutex::new(EchoCounters::default()));
    let provider = Arc::new(EchoProvider {
        counters: Arc::clone(&counters),
    });
    registry.register(provider).await;

    let mut samples = Vec::with_capacity(SEND_RAW_SAMPLES);
    for i in 0..SEND_RAW_SAMPLES {
        let request = json!({ "op": "ping", "seq": i });
        let started = Instant::now();
        let response = registry
            .send_raw("perf-echo", &request)
            .await
            .expect("send_raw must succeed against echo provider");
        samples.push(started.elapsed());
        assert_eq!(
            response.get("status").and_then(|v| v.as_str()),
            Some("ok"),
            "echo response missing status:ok at i={i}: {response}"
        );
    }
    let stats = MetricStats::from_samples(&samples);
    eprintln!("perf_provider_registry_send_raw_single: {stats:?}");
    maybe_emit("provider_registry_send_raw_single", &stats);
    assert!(
        stats.max_us < 1_000_000,
        "send_raw_single max {} µs exceeds 1s tripwire",
        stats.max_us
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn perf_provider_registry_send_raw_concurrent() {
    let registry = Arc::new(ProviderRegistry::new());
    let counters = Arc::new(Mutex::new(EchoCounters::default()));
    let provider = Arc::new(EchoProvider {
        counters: Arc::clone(&counters),
    });
    registry.register(provider).await;

    // Per-sender wall-clock for the full N messages — gives
    // us a fan-in latency curve. A future Day-8/Phase-6
    // expansion can dial these up; today's defaults aim for
    // <500 ms total wall-clock on a dev laptop.
    let mut set = tokio::task::JoinSet::new();
    let started = Instant::now();
    for sender_idx in 0..CONCURRENT_SENDERS {
        let registry = Arc::clone(&registry);
        set.spawn(async move {
            let mut per_call: Vec<Duration> = Vec::with_capacity(CONCURRENT_MESSAGES_PER_SENDER);
            for msg_idx in 0..CONCURRENT_MESSAGES_PER_SENDER {
                let request = json!({ "op": "ping", "sender": sender_idx, "seq": msg_idx });
                let t = Instant::now();
                registry
                    .send_raw("perf-echo", &request)
                    .await
                    .expect("send_raw must succeed under concurrency");
                per_call.push(t.elapsed());
            }
            per_call
        });
    }

    let mut all_samples = Vec::with_capacity(CONCURRENT_SENDERS * CONCURRENT_MESSAGES_PER_SENDER);
    while let Some(joined) = set.join_next().await {
        let per_call = joined.expect("sender task must not panic");
        all_samples.extend(per_call);
    }
    let total_wall_clock = started.elapsed();
    let stats = MetricStats::from_samples(&all_samples);
    eprintln!(
        "perf_provider_registry_send_raw_concurrent: total_wall_clock_us={} per_call_stats={stats:?}",
        total_wall_clock.as_micros()
    );
    maybe_emit("provider_registry_send_raw_concurrent", &stats);
    assert_eq!(
        all_samples.len(),
        CONCURRENT_SENDERS * CONCURRENT_MESSAGES_PER_SENDER,
        "every concurrent send must be sampled"
    );
    assert!(
        total_wall_clock < Duration::from_secs(5),
        "concurrent send_raw wall-clock {total_wall_clock:?} exceeds 5s tripwire"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn perf_capability_manager_validate() {
    let store = Arc::new(elastos_runtime::capability::CapabilityStore::new());
    let audit_log = Arc::new(AuditLog::new());
    let metrics = Arc::new(MetricsManager::new());
    let manager = Arc::new(CapabilityManager::new(
        Arc::clone(&store),
        Arc::clone(&audit_log),
        Arc::clone(&metrics),
    ));

    // Single token, single resource — measures the validate
    // hot path: lock acquisition + resource-pattern match +
    // audit-log emit. Concurrency stress is already covered
    // by `capability_validate_under_1000_parallel_calls_*`;
    // this is the per-call cost.
    let capsule = "phase5-day7-perf-capsule-cap";
    let resource = ResourceId::new("localhost://Users/phase5-day7-perf/*".to_string());
    let token = manager.grant(
        capsule,
        resource,
        Action::Read,
        TokenConstraints::default(),
        None,
    );

    let mut samples = Vec::with_capacity(CAPABILITY_VALIDATE_SAMPLES);
    for i in 0..CAPABILITY_VALIDATE_SAMPLES {
        let req_resource = ResourceId::new(format!("localhost://Users/phase5-day7-perf/file-{i}"));
        let started = Instant::now();
        manager
            .validate(&token, capsule, Action::Read, &req_resource, None)
            .await
            .expect("validate must succeed for in-pattern resource");
        samples.push(started.elapsed());
    }
    let stats = MetricStats::from_samples(&samples);
    eprintln!("perf_capability_manager_validate: {stats:?}");
    maybe_emit("capability_manager_validate", &stats);
    assert!(
        stats.max_us < 1_000_000,
        "capability_validate max {} µs exceeds 1s tripwire",
        stats.max_us
    );
}

// ───────────────────────────────────────────────────────────
// Self-tests for the percentile + report shape — guards the
// JSON wire format the Day-7+ shell scripts and the eventual
// Phase-6 regression-detector lean on.
// ───────────────────────────────────────────────────────────

#[test]
fn percentile_nearest_rank_returns_actual_sample() {
    let sorted: Vec<u128> = (1u128..=10).collect();
    assert_eq!(percentile(&sorted, 50), 5);
    assert_eq!(percentile(&sorted, 95), 10);
    assert_eq!(percentile(&sorted, 99), 10);
    assert_eq!(percentile(&sorted, 100), 10);
}

#[test]
fn metric_stats_from_single_sample_is_degenerate_but_stable() {
    let one = MetricStats::from_samples(&[Duration::from_micros(42)]);
    assert_eq!(one.samples_count, 1);
    assert_eq!(one.min_us, 42);
    assert_eq!(one.p50_us, 42);
    assert_eq!(one.p95_us, 42);
    assert_eq!(one.p99_us, 42);
    assert_eq!(one.max_us, 42);
}

#[test]
fn perf_report_json_schema_is_stable_for_consumers() {
    // Pin the on-disk JSON shape — schema_version, metric_name,
    // stats keys, notes keys — so the shell aggregator and
    // future Phase-6 regression detector can parse without
    // ambiguity. Day-7 freezes schema_version=1; future bumps
    // must update consumers.
    let stats = MetricStats {
        samples_count: 5,
        min_us: 1,
        p50_us: 2,
        p95_us: 3,
        p99_us: 4,
        max_us: 5,
    };
    let report = PerfReport {
        schema_version: SCHEMA_VERSION,
        captured_at_unix_ms: 1_000,
        host: HostInfo {
            os: "darwin".to_string(),
            arch: "arm64".to_string(),
            rust_version: "1.85.0".to_string(),
            cpu_count_logical: 10,
            phase: "5-day-7".to_string(),
        },
        backend: "vz".to_string(),
        metric_name: "supervisor_new_cold".into(),
        stats,
        notes: ReportNotes::day_seven_default(),
    };
    let json = serde_json::to_value(&report).expect("perf report must serialise");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["backend"], "vz");
    assert_eq!(json["metric_name"], "supervisor_new_cold");
    assert!(json["host"].is_object());
    assert!(json["stats"].is_object());
    assert_eq!(json["stats"]["samples_count"], 5);
    assert_eq!(json["stats"]["p50_us"], 2);
    assert_eq!(json["stats"]["p99_us"], 4);
    assert!(json["notes"].is_object());
    assert_eq!(json["notes"]["real_vz_boot_measured"], false);

    // Round-trip stability — every consumer of the on-disk
    // file MUST be able to deserialise what we emit.
    let round_trip: PerfReport = serde_json::from_value(json).expect("round-trip must succeed");
    assert_eq!(round_trip.schema_version, SCHEMA_VERSION);
    assert_eq!(round_trip.stats.p50_us, 2);
}

// Sanity guard for the `EchoProvider` shape (in case a
// future Provider-trait change regresses the harness).
#[tokio::test]
async fn echo_provider_round_trips_under_send_raw() {
    let registry = ProviderRegistry::new();
    let counters = Arc::new(Mutex::new(EchoCounters::default()));
    let provider = Arc::new(EchoProvider {
        counters: Arc::clone(&counters),
    });
    registry.register(provider).await;
    let response = registry
        .send_raw("perf-echo", &json!({ "op": "ping" }))
        .await
        .expect("send_raw must succeed");
    assert_eq!(response.get("status").and_then(|v| v.as_str()), Some("ok"));
    assert_eq!(response.get("seq").and_then(|v| v.as_u64()), Some(1));
}

// Reference an unused symbol from elastos-runtime's
// capability::pending module so the import line above
// stays tied to a real type — keeps the rustdoc-link
// intact and `cargo doc` happy. The `GrantDuration` enum
// is the unit the validate path consults under TTL paths;
// Day-7 doesn't exercise it but Phase-6 perf work will.
#[test]
fn pending_module_grant_duration_link_is_live() {
    // Touch a real `GrantDuration` variant so the import
    // line stays anchored to the type the future Phase-6
    // perf work will exercise (TTL paths).
    let _ = GrantDuration::Once;
    let _store = PendingRequestStore::new(Arc::new(AuditLog::new()));
}
