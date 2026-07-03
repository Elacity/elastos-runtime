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

    println!("\n== audit emit throughput ==");
    report("memory-only (new)", mem_iters, mem_secs);
    report("file-backed (with_file/fsync)", file_iters, file_secs);

    let mem_us = (mem_secs * 1e6 / mem_iters as f64).max(f64::MIN_POSITIVE);
    let file_us = file_secs * 1e6 / file_iters as f64;
    println!(
        "\ndurable-custody cost ~{:.1}x per record ({:.2} us -> {:.2} us); \
         single-writer durable ceiling ~{:.0} emits/s",
        file_us / mem_us,
        mem_us,
        file_us,
        1e6 / file_us,
    );
    println!(
        "note: hardware- and filesystem-dependent (SSD vs HDD vs networked). Re-run on the target \
         box before deciding the group-commit rewrite (KNOWN_GAPS Wave-2)."
    );
}
