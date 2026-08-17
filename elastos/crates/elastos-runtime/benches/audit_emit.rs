//! First benchmark of the audit `emit` hot path — the throughput ceiling the whole runtime's
//! SPEED grade rests on. `KNOWN_GAPS` flags the audit fsync-per-record (under a global mutex) as
//! the real scalability wall AND explicitly notes it was never measured ("no flamegraph run yet;
//! magnitudes are I/O-class estimates"). This converts that estimate into a number, and — per the
//! measure-first discipline — tells us whether the group-commit rewrite is even worth its risk.
//!
//! Hermetic: std timing only, no `criterion`, no network. Run:
//!   cargo bench -p elastos-runtime --bench audit_emit
//!
//! Two regimes, ops/sec + µs/op each:
//!   1. memory-only  (`AuditLog::new`)       — no writer, no fsync: the CPU + ed25519-sign +
//!      hash-chain cost of an emit, nothing else.
//!   2. file-backed  (`AuditLog::with_file`) — sign + append + `fsync` per record: THE ceiling
//!      (this is what a durable-custody deployment pays on the capability-use path).
//!
//! (2)/(1) is the price of durable custody per record; 1/µs_file is the single-writer durable
//! throughput ceiling the group-commit rewrite would lift.

use std::time::Instant;

use elastos_runtime::primitives::{AuditEvent, AuditLog, SecureTimestamp};

/// A minimal, allocation-light event so the measurement reflects the emit machinery
/// (sign + hash-chain + write), not event construction.
fn cheap_event() -> AuditEvent {
    AuditEvent::RuntimeStart {
        timestamp: SecureTimestamp::now(),
        version: "bench".to_string(),
    }
}

fn bench_memory(iters: u64) -> f64 {
    let log = AuditLog::new();
    let start = Instant::now();
    for _ in 0..iters {
        log.emit(cheap_event()).expect("memory emit must succeed");
    }
    start.elapsed().as_secs_f64()
}

fn bench_file(iters: u64) -> f64 {
    let dir = tempfile::tempdir().expect("tempdir");
    let log = AuditLog::with_file(dir.path().join("bench.log")).expect("file-backed log opens");
    let start = Instant::now();
    for _ in 0..iters {
        log.emit(cheap_event()).expect("file emit must succeed");
    }
    start.elapsed().as_secs_f64()
}

/// The group-commit regime (S51): N threads hammer ONE file-backed log. Pre-S51 every emitter
/// serialized behind one fsync per record (total throughput pinned at the single-writer ceiling
/// regardless of thread count); with group commit, concurrent emits coalesce into shared fsyncs,
/// so total ops/s should scale well past that ceiling. Verifies the chain afterwards so the
/// number is for CORRECT commits only.
fn bench_file_concurrent(threads: u64, per_thread: u64) -> f64 {
    let dir = tempfile::tempdir().expect("tempdir");
    let log =
        std::sync::Arc::new(AuditLog::with_file(dir.path().join("bench.log")).expect("opens"));
    let start = Instant::now();
    let handles: Vec<_> = (0..threads)
        .map(|_| {
            let log = log.clone();
            std::thread::spawn(move || {
                for _ in 0..per_thread {
                    log.emit(cheap_event()).expect("concurrent emit succeeds");
                }
            })
        })
        .collect();
    for h in handles {
        h.join().expect("emitter thread");
    }
    let secs = start.elapsed().as_secs_f64();
    // Verify under the log's REAL key (a signed chain refuses a keyless walk, fail-closed).
    let vk_bytes: [u8; 32] = hex::decode(log.verifying_key_hex().expect("signed log"))
        .expect("hex")
        .try_into()
        .expect("32 bytes");
    let vk = ed25519_dalek::VerifyingKey::from_bytes(&vk_bytes).expect("valid key");
    let verified = log
        .verify_chain(Some(&vk))
        .expect("the concurrent chain must verify");
    assert_eq!(
        verified,
        threads * per_thread,
        "no record lost or duplicated"
    );
    secs
}

fn report(label: &str, iters: u64, secs: f64) {
    let ops = iters as f64 / secs;
    let us = secs * 1_000_000.0 / iters as f64;
    println!("{label:<30} {iters:>8} emits  {secs:>7.3}s  {ops:>12.0} ops/s  {us:>9.2} us/op");
}

fn main() {
    // Warm up (allocator, first-fsync, page cache) so the first timed regime isn't penalized.
    let _ = bench_memory(1_000);
    let _ = bench_file(200);

    let mem_iters: u64 = 200_000;
    let file_iters: u64 = 20_000; // fewer: each performs a real fsync

    let mem_secs = bench_memory(mem_iters);
    let file_secs = bench_file(file_iters);

    // Group-commit regime (S51): 8 concurrent emitters, one log. Pre-S51 this was pinned at the
    // single-writer ceiling (every emitter serialized behind fsync-per-record); with group commit
    // the coalesced fsyncs should push total ops/s well past it.
    let conc_threads: u64 = 8;
    let conc_per_thread: u64 = 2_500;
    let conc_secs = bench_file_concurrent(conc_threads, conc_per_thread);

    println!("\n== audit emit throughput ==");
    report("memory-only (new)", mem_iters, mem_secs);
    report("file-backed (with_file/fsync)", file_iters, file_secs);
    report(
        "file-backed 8-thread (group)",
        conc_threads * conc_per_thread,
        conc_secs,
    );

    let mem_us = (mem_secs * 1e6 / mem_iters as f64).max(f64::MIN_POSITIVE);
    let file_us = file_secs * 1e6 / file_iters as f64;
    let single_ops = 1e6 / file_us;
    let conc_ops = (conc_threads * conc_per_thread) as f64 / conc_secs;
    println!(
        "\ndurable-custody cost ~{:.1}x per record ({:.2} us -> {:.2} us); \
         single-writer durable ceiling ~{single_ops:.0} emits/s",
        file_us / mem_us,
        mem_us,
        file_us,
    );
    println!(
        "group commit (S51): {conc_threads} concurrent emitters reach ~{conc_ops:.0} emits/s \
         ({:.1}x the single-writer ceiling; pre-S51 they were PINNED AT it — every emitter \
         serialized behind fsync-per-record)",
        conc_ops / single_ops,
    );
    println!(
        "note: hardware- and filesystem-dependent (SSD vs HDD vs networked). Re-run on the target \
         box before relying on these magnitudes."
    );
}
