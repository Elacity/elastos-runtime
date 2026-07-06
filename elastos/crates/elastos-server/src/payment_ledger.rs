//! The payment ledger — a durable record of every rail attempt, and the operator's reconciliation
//! surface for INDETERMINATE outcomes (Sprint 30, council S29 RT-F4/G-F7).
//!
//! WHY THIS EXISTS: the two-generals handling in the pay affordance keeps the cap reservation when
//! a rail outcome is indeterminate (timeout/5xx/panic — the charge may have posted). That is the
//! fail-closed direction, but it strands headroom the operator can only safely restore by asking
//! the RAIL what actually happened (a blind cap raise can authorize real spend beyond the original
//! intent). This ledger makes that reconciliation possible without reading logs or deriving keys
//! from the chain: every rail attempt lands here with its idempotency key, and a PENDING entry can
//! be RESOLVED exactly once — "not charged" refunds the reservation, "charged" confirms the spend.
//!
//! It also durably custodies the RAIL REFERENCE for every performed payment (council S29 G-F7) —
//! the audit bridge from a Performed receipt to the rail's transaction — until a receipt field
//! carries it (tracked follow-on).
//!
//! HONEST BOUNDS:
//! - This ledger is OPERATIONAL custody, not the signed chain: entries are not signed, and the
//!   signed record of the act remains the intent declaration + reconciliation on the audit chain.
//!   A reconciliation RESOLUTION is attested on the signed chain (`Custom` event, emitted by the
//!   handler) — the resolution's authority trail is the chain, this ledger is the working state.
//! - Durability mirrors the spend meter (versioned snapshot, temp + fsync + rename + parent-dir
//!   fsync, size-capped fail-closed open) but the ledger deliberately does NOT block money on its
//!   own failures: a payment whose ledger write fails still completes with its fail-closed money
//!   semantics, and the decline/performed reason says the entry is unrecorded (reconcile from the
//!   error log). Money invariants live in the meter + replay guard, never here.
//! - BOUNDED: terminal entries (performed / not-charged / resolved) are evicted oldest-first past
//!   the cap; PENDING entries are never evicted (they are the reconciliation obligations), and a
//!   pending set at the cap refuses NEW pending entries (recorded=false ⇒ the reason tells the
//!   operator to reconcile from the log) rather than growing without bound.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

/// Where one rail attempt stands. `Pending` is the only non-terminal state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentStatus {
    /// The rail confirmed the charge (2xx) — `rail_ref` carries its reference.
    Performed,
    /// The rail provably did not charge — the reservation was refunded by the pay path.
    NotCharged,
    /// INDETERMINATE — the charge may have posted; the reservation is held. Awaiting resolution.
    Pending,
    /// Operator resolved against the rail: the charge DID post. Spend stands.
    ResolvedCharged,
    /// Operator resolved against the rail: the charge did NOT post. The reservation was refunded
    /// (or the refund's failure surfaced loudly — see the reconcile handler).
    ResolvedNotCharged,
}

impl PaymentStatus {
    fn is_terminal(self) -> bool {
        !matches!(self, PaymentStatus::Pending)
    }
}

/// One rail attempt. `rail_note` is the sanitized (printable, bounded) rail body/reference.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PaymentRecord {
    /// The signature-derived idempotency key — the rail-side lookup handle. Ledger key.
    pub idempotency_key: String,
    pub capsule: String,
    pub payee: String,
    pub amount: u64,
    pub status: PaymentStatus,
    /// Sanitized rail reference/body head (printable ASCII, ≤256) — empty when none.
    pub rail_note: String,
    /// Monotonic insertion sequence (this ledger's own counter) — the eviction order.
    pub seq: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct LedgerSnapshotV1 {
    version: u32,
    next_seq: u64,
    records: Vec<PaymentRecord>,
}

const LEDGER_SNAPSHOT_VERSION: u32 = 1;
/// Total record cap; terminal entries beyond it are evicted oldest-first.
const LEDGER_CAP: usize = 4096;

/// Hold rail-controlled bytes to a renderable discipline BEFORE they enter any reason, record, or
/// surface (council S29 RT-F7): printable ASCII only (control chars and non-ASCII dropped), ≤256.
pub fn sanitize_rail_note(raw: &str) -> String {
    raw.chars()
        .filter(|c| (' '..='~').contains(c))
        .take(256)
        .collect()
}

/// A durable, bounded ledger of rail attempts. All mutations persist snapshot-atomically before
/// returning success; a persist failure rolls the mutation back and reports it (the caller decides
/// what that means — the pay path proceeds and says "unrecorded", the reconcile path REFUSES).
pub struct PaymentLedger {
    records: RwLock<HashMap<String, PaymentRecord>>,
    next_seq: std::sync::atomic::AtomicU64,
    storage_path: Option<PathBuf>,
}

impl PaymentLedger {
    /// In-memory ledger (tests/embedded).
    pub fn new() -> Self {
        Self {
            records: RwLock::new(HashMap::new()),
            next_seq: std::sync::atomic::AtomicU64::new(0),
            storage_path: None,
        }
    }

    /// Open the durable ledger. Missing file ⇒ fresh; corrupt/oversized/dup-key snapshot ⇒ REFUSE
    /// (fail-closed — booting over a lost pending set would orphan reconciliation obligations).
    pub fn open_durable(path: PathBuf) -> std::io::Result<Self> {
        let mut records = HashMap::new();
        let mut next_seq = 0u64;
        match std::fs::read(&path) {
            Ok(bytes) => {
                if bytes.len() > 8 * 1024 * 1024 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "payment ledger snapshot exceeds the 8 MiB bound",
                    ));
                }
                let snapshot: LedgerSnapshotV1 = serde_json::from_slice(&bytes)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                if snapshot.version != LEDGER_SNAPSHOT_VERSION {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("unsupported ledger snapshot version {}", snapshot.version),
                    ));
                }
                next_seq = snapshot.next_seq;
                for rec in snapshot.records {
                    if records.insert(rec.idempotency_key.clone(), rec).is_some() {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "duplicate idempotency key in ledger snapshot",
                        ));
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        Ok(Self {
            records: RwLock::new(records),
            next_seq: std::sync::atomic::AtomicU64::new(next_seq),
            storage_path: Some(path),
        })
    }

    /// Record one rail attempt. Returns `true` iff the entry is durably recorded — `false` means
    /// the caller's user-facing reason must say "unrecorded; reconcile from the error log"
    /// (a full pending set or a failed persist NEVER blocks the payment's own money semantics).
    /// A key already present is left untouched (`false`): the first record wins — a replayed key
    /// cannot rewrite history.
    pub fn record(
        &self,
        idempotency_key: &str,
        capsule: &str,
        payee: &str,
        amount: u64,
        status: PaymentStatus,
        rail_note: &str,
    ) -> bool {
        let mut records = match self.records.write() {
            Ok(r) => r,
            Err(_) => return false,
        };
        if records.contains_key(idempotency_key) {
            return false;
        }
        // Bound the map: evict oldest TERMINAL entries; never a pending one. If everything at the
        // cap is pending, refuse a NEW pending (bounded obligations) but allow terminal records to
        // temporarily exceed by eviction failure being impossible for them... keep it simple:
        // refuse any insert that cannot make room.
        while records.len() >= LEDGER_CAP {
            let oldest_terminal = records
                .values()
                .filter(|r| r.status.is_terminal())
                .min_by_key(|r| r.seq)
                .map(|r| r.idempotency_key.clone());
            match oldest_terminal {
                Some(k) => {
                    records.remove(&k);
                }
                None => return false, // cap full of pending obligations — refuse, stay bounded
            }
        }
        let seq = self
            .next_seq
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        records.insert(
            idempotency_key.to_string(),
            PaymentRecord {
                idempotency_key: idempotency_key.to_string(),
                capsule: capsule.to_string(),
                payee: payee.to_string(),
                amount,
                status,
                rail_note: sanitize_rail_note(rail_note),
                seq,
            },
        );
        if self.persist_locked(&records).is_err() {
            records.remove(idempotency_key);
            return false;
        }
        true
    }

    /// Resolve a PENDING entry exactly once. Durable-before-visible with rollback: on success the
    /// entry is terminally `ResolvedCharged`/`ResolvedNotCharged` and can never resolve again
    /// (the double-refund guard the reconcile handler builds on). Errors are honest strings.
    pub fn resolve(&self, idempotency_key: &str, charged: bool) -> Result<PaymentRecord, String> {
        let mut records = match self.records.write() {
            Ok(r) => r,
            Err(_) => return Err("ledger lock poisoned".to_string()),
        };
        let rec = records
            .get_mut(idempotency_key)
            .ok_or_else(|| "no ledger entry for this idempotency key".to_string())?;
        if rec.status != PaymentStatus::Pending {
            return Err(format!(
                "entry is not pending (status {:?}) — a payment resolves exactly once",
                rec.status
            ));
        }
        rec.status = if charged {
            PaymentStatus::ResolvedCharged
        } else {
            PaymentStatus::ResolvedNotCharged
        };
        let resolved = rec.clone();
        if self.persist_locked(&records).is_err() {
            if let Some(r) = records.get_mut(idempotency_key) {
                r.status = PaymentStatus::Pending;
            }
            return Err(
                "resolution could not be durably recorded; the entry stays pending (rolled back)"
                    .to_string(),
            );
        }
        Ok(resolved)
    }

    /// All PENDING entries (the reconciliation work list), oldest first.
    pub fn pending(&self) -> Vec<PaymentRecord> {
        let records = match self.records.read() {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        let mut out: Vec<PaymentRecord> = records
            .values()
            .filter(|r| r.status == PaymentStatus::Pending)
            .cloned()
            .collect();
        out.sort_by_key(|r| r.seq);
        out
    }

    /// Sum + count of one capsule's PENDING (indeterminate) units — the "held, unconfirmed" figure
    /// the budget surface shows next to confirmed spend (council S29 RT-F4).
    pub fn pending_for(&self, capsule: &str) -> (u64, usize) {
        let records = match self.records.read() {
            Ok(r) => r,
            Err(_) => return (0, 0),
        };
        records
            .values()
            .filter(|r| r.status == PaymentStatus::Pending && r.capsule == capsule)
            .fold((0u64, 0usize), |(units, n), r| {
                (units.saturating_add(r.amount), n + 1)
            })
    }

    /// One entry by key (read-only projection).
    pub fn get(&self, idempotency_key: &str) -> Option<PaymentRecord> {
        self.records.read().ok()?.get(idempotency_key).cloned()
    }

    /// Snapshot write, mirroring the spend meter's discipline (temp + fsync + rename + parent-dir
    /// fsync). Memory-only ⇒ no-op. The ledger does not poison: it is operational custody, and its
    /// callers already treat a failed write as "unrecorded, say so" / "refuse the resolution".
    fn persist_locked(&self, records: &HashMap<String, PaymentRecord>) -> std::io::Result<()> {
        let Some(path) = &self.storage_path else {
            return Ok(());
        };
        let mut list: Vec<PaymentRecord> = records.values().cloned().collect();
        list.sort_by_key(|r| r.seq);
        let content = serde_json::to_vec(&LedgerSnapshotV1 {
            version: LEDGER_SNAPSHOT_VERSION,
            next_seq: self.next_seq.load(std::sync::atomic::Ordering::SeqCst),
            records: list,
        })
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp_path = path.with_extension("tmp");
        {
            use std::io::Write as _;
            let mut tmp = std::fs::File::create(&tmp_path)?;
            tmp.write_all(&content)?;
            tmp.sync_all()?;
        }
        std::fs::rename(&tmp_path, path)?;
        #[cfg(unix)]
        if let Some(parent) = path.parent() {
            std::fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    }
}

impl Default for PaymentLedger {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_entries_survive_restart_and_resolve_exactly_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("payments.json");
        {
            let ledger = PaymentLedger::open_durable(path.clone()).unwrap();
            assert!(ledger.record("flint-k1", "vm-ap", "acme", 200, PaymentStatus::Pending, ""));
            assert!(ledger.record(
                "flint-k2",
                "vm-ap",
                "acme",
                50,
                PaymentStatus::Performed,
                "rail-ref-9"
            ));
            assert_eq!(ledger.pending_for("vm-ap"), (200, 1));
        } // restart
        let ledger = PaymentLedger::open_durable(path.clone()).unwrap();
        assert_eq!(
            ledger.pending_for("vm-ap"),
            (200, 1),
            "the reconciliation obligation survives restart"
        );
        assert_eq!(
            ledger.get("flint-k2").unwrap().rail_note,
            "rail-ref-9",
            "the rail reference is durably custodied"
        );
        let resolved = ledger.resolve("flint-k1", false).unwrap();
        assert_eq!(resolved.status, PaymentStatus::ResolvedNotCharged);
        assert!(
            ledger.resolve("flint-k1", false).is_err(),
            "a payment resolves EXACTLY once — no double-refund handle"
        );
        assert_eq!(ledger.pending_for("vm-ap"), (0, 0));
        // The resolution also survives restart.
        drop(ledger);
        let reopened = PaymentLedger::open_durable(path).unwrap();
        assert!(reopened.resolve("flint-k1", true).is_err());
    }

    #[test]
    fn record_is_first_write_wins_and_notes_are_sanitized() {
        let ledger = PaymentLedger::new();
        assert!(ledger.record(
            "k",
            "vm-ap",
            "acme",
            10,
            PaymentStatus::Pending,
            "ok\x1b[31m<script>é" // control chars + non-ASCII dropped, markup kept-but-inert ASCII
        ));
        assert!(
            !ledger.record("k", "vm-x", "other", 999, PaymentStatus::Performed, ""),
            "a replayed key cannot rewrite history"
        );
        let rec = ledger.get("k").unwrap();
        assert_eq!(rec.capsule, "vm-ap");
        assert_eq!(rec.rail_note, "ok[31m<script>");
        assert_eq!(sanitize_rail_note(&"x".repeat(999)).len(), 256);
    }

    #[test]
    fn eviction_never_sheds_a_pending_obligation() {
        let ledger = PaymentLedger::new();
        // Fill to the cap with terminal entries plus one early pending.
        assert!(ledger.record("pend-1", "vm-ap", "acme", 5, PaymentStatus::Pending, ""));
        for i in 0..(LEDGER_CAP - 1) {
            assert!(ledger.record(
                &format!("t-{i}"),
                "vm-ap",
                "acme",
                1,
                PaymentStatus::Performed,
                ""
            ));
        }
        // At cap: a new record evicts the OLDEST TERMINAL (t-0), never pend-1.
        assert!(ledger.record("t-new", "vm-ap", "acme", 1, PaymentStatus::Performed, ""));
        assert!(ledger.get("pend-1").is_some(), "pending is never evicted");
        assert!(ledger.get("t-0").is_none(), "oldest terminal was evicted");
        // A cap full of ONLY pending refuses new inserts (bounded obligations).
        let small = PaymentLedger::new();
        for i in 0..LEDGER_CAP {
            assert!(small.record(&format!("p-{i}"), "vm", "a", 1, PaymentStatus::Pending, ""));
        }
        assert!(
            !small.record("one-more", "vm", "a", 1, PaymentStatus::Pending, ""),
            "a pending set at the cap refuses new entries instead of growing unbounded"
        );
    }

    #[test]
    fn unpersistable_mutations_roll_back() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("ledger");
        std::fs::create_dir(&sub).unwrap();
        let ledger = PaymentLedger::open_durable(sub.join("payments.json")).unwrap();
        assert!(ledger.record("k1", "vm-ap", "acme", 10, PaymentStatus::Pending, ""));
        std::fs::remove_dir_all(&sub).unwrap();
        assert!(
            !ledger.record("k2", "vm-ap", "acme", 10, PaymentStatus::Pending, ""),
            "an unpersistable record reports unrecorded"
        );
        assert!(ledger.get("k2").is_none(), "rolled back — no phantom entry");
        let err = ledger.resolve("k1", false).unwrap_err();
        assert!(err.contains("stays pending"), "{err}");
        assert_eq!(
            ledger.get("k1").unwrap().status,
            PaymentStatus::Pending,
            "an unpersistable resolution rolls back — the refund handle is not lost"
        );
    }

    #[test]
    fn corrupt_or_oversized_snapshot_refuses_to_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("payments.json");
        std::fs::write(&path, b"{ nope").unwrap();
        assert!(PaymentLedger::open_durable(path.clone()).is_err());
        let mut huge = Vec::from(&b"{\"version\":1,"[..]);
        huge.resize(8 * 1024 * 1024 + 1, b' ');
        std::fs::write(&path, huge).unwrap();
        assert!(PaymentLedger::open_durable(path).is_err());
    }
}
