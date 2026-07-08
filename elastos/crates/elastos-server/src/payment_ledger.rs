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
//! HONEST BOUNDS (council S30):
//! - CUSTODY IS "EVERY RAIL ATTEMPT THE PROCESS LIVED TO RECORD", not literally every attempt: a
//!   crash between the persisted reservation and the rail verdict leaves a durable reservation
//!   with NO ledger entry (the S29 orphaned-reservation window) — recovery there is still
//!   deriving `flint-<signature>` from the on-chain declaration. PENDING custody is
//!   guaranteed-or-stated (an unrecordable pending is named in the decline reason); TERMINAL
//!   records (performed / not-charged) are best-effort — a failed write drops the rail reference
//!   to the runtime log only.
//! - THE LEDGER GATES MONEY IN THE RELEASE DIRECTION (council S30 G-F1): it never gates the
//!   CHARGE path (a payment completes with its fail-closed semantics whether or not its record
//!   lands), but a resolved record's (capsule, amount) is the INPUT to a reconciliation refund —
//!   so this file is money-trusted core on release and carries the meter's protections: same
//!   snapshot discipline, same single-opener flock, same data_dir trust class (the snapshot is
//!   not self-authenticating; a data_dir writer already owns the stronger attack on the meter
//!   file itself).
//! - This ledger is OPERATIONAL custody, not the signed chain: entries are not signed; the signed
//!   record of the act remains the intent declaration + reconciliation on the audit chain, and a
//!   RESOLUTION is attested there (`Custom` event, emitted by the handler).
//! - BOUNDED, per capsule and globally: terminal entries are evicted oldest-first past the global
//!   cap; PENDING entries are never evicted (they are the reconciliation obligations); a capsule
//!   at its own pending cap — or a global map full of pendings — refuses NEW pending entries
//!   (recorded=false ⇒ stated in the reason). The per-capsule bound (council S30 RT-F3) confines
//!   the blinding to the misbehaving capsule: one agent flooding indeterminates cannot push a
//!   VICTIM capsule's obligations out of the work list. A full pending set also stops best-effort
//!   terminal custody — the stated consequence of the bound.

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

    /// Whether an entry may move money (or already has) — the states the idempotency guard relies
    /// on to refuse a re-charge (council S35 guardian F3 / red-team F1). These MUST NOT be evicted:
    /// evicting a `Performed`/`ResolvedCharged` key would let a cross-window re-dispatch of the
    /// same signed intent find no entry and buy again. Only the provably-nothing-moved terminals
    /// (`NotCharged`/`ResolvedNotCharged`) are safe to evict.
    fn is_money_bearing(self) -> bool {
        matches!(
            self,
            PaymentStatus::Pending | PaymentStatus::Performed | PaymentStatus::ResolvedCharged
        )
    }
}

/// The result of [`PaymentLedger::begin_attempt`] — the durable custody handle the pay path takes
/// BEFORE it broadcasts (council S35 red-team F1), so a re-dispatch can never find "no entry" for a
/// buy whose money moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeginAttempt {
    /// A `Pending` placeholder is durably recorded; the caller may now move money and then
    /// [`finalize`](PaymentLedger::finalize) the outcome.
    Started,
    /// The key already carries a money-bearing entry (a concurrent dispatch beat this one). The
    /// caller must NOT move money again — decline idempotently and refund THIS attempt's reservation.
    AlreadyActive(PaymentStatus),
    /// The ledger could not durably custody the attempt (per-capsule pending cap, ledger full of
    /// money-bearing entries, or a persist failure). The caller MUST NOT broadcast — refund and
    /// decline fail-closed, so money never moves into an unrecordable state.
    CapacityRefused,
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
    /// For `ResolvedNotCharged`: whether the refund was durably applied to the meter (council S30
    /// G-F3/RT-F2). `false` on a resolved-not-charged entry is the FORENSIC marker for a crash (or
    /// persist failure) between resolution and refund: the refund was never applied, the cap
    /// remains debited, and the recovery lever is raising the limit. Absent/false on all other
    /// statuses.
    #[serde(default)]
    pub refund_applied: bool,
    /// The mandate token (`standing_grant_id`) this payment was made under (Sprint 35). Lets a
    /// reconciliation bind the confirmed settlement back onto the mandate's receipt (a token-keyed
    /// `CapabilityUse`). `None` for a payment recorded without a bound mandate, and for every
    /// pre-S35 record on disk. BACK-COMPAT: appended last with `#[serde(default,
    /// skip_serializing_if = "Option::is_none")]`, so a pre-S35 ledger snapshot round-trips
    /// unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_id: Option<String>,
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
/// Per-capsule bound on PENDING entries (council S30 RT-F3): one capsule flooding indeterminates
/// cannot exhaust the global map and blind a VICTIM capsule's reconciliation obligations.
const PENDING_PER_CAPSULE_CAP: usize = 256;

/// Hold rail-controlled bytes to a printable discipline BEFORE they enter any reason, record, or
/// surface (council S29 RT-F7): printable ASCII only (control chars — incl. CR/LF/ESC, killing
/// log/ANSI injection — and non-ASCII dropped), ≤256. NOT HTML-escaped: `<>"'&` survive as inert
/// ASCII — any HTML renderer of a `rail_note` MUST escape at render (council S30 F8).
pub fn sanitize_rail_note(raw: &str) -> String {
    raw.chars()
        .filter(|c| (' '..='~').contains(c))
        .take(256)
        .collect()
}

/// Why a resolution was refused — structured so AUTOMATION can distinguish retry from terminal
/// (council S30 G-F2: one 409 string made a transient persist failure read as "already resolved",
/// abandoning a live refund obligation).
#[derive(Debug, PartialEq, Eq)]
pub enum ResolveError {
    /// No entry for this key (404): never recorded, or an unrecorded pending — reconcile via the
    /// operator's out-of-band levers, not this endpoint.
    NotFound,
    /// The entry is terminal (409): the payment already resolved (or never needed to) — a payment
    /// resolves exactly once, retrying is wrong.
    NotPending(PaymentStatus),
    /// The resolution could not be durably recorded and was ROLLED BACK — the entry is STILL
    /// pending and the obligation still live (503): RETRY.
    Persist,
    /// Lock poisoned (503): retry.
    Lock,
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::NotFound => write!(f, "no ledger entry for this idempotency key"),
            ResolveError::NotPending(st) => write!(
                f,
                "entry is not pending (status {st:?}) — a payment resolves exactly once"
            ),
            ResolveError::Persist => write!(
                f,
                "resolution could not be durably recorded; the entry stays pending (rolled back) — retry"
            ),
            ResolveError::Lock => write!(f, "ledger lock poisoned — retry"),
        }
    }
}

/// Admission control for a NEW ledger key — the single home of the bounding invariant, shared by
/// [`PaymentLedger::record_with_token`] and [`PaymentLedger::begin_attempt`]. Returns `false` when
/// the entry cannot be admitted.
///
/// Two rules, in order:
/// 1. Per-capsule pending bound (council S30 RT-F3): a capsule at its pending cap refuses NEW
///    pending entries — the blinding is confined to the misbehaving capsule, never a victim's
///    work list.
/// 2. Global bound: evict the oldest EVICTABLE entry — a provably-nothing-moved terminal
///    (`NotCharged`/`ResolvedNotCharged`) ONLY. Money-bearing keys (`Pending`/`Performed`/
///    `ResolvedCharged`) are NEVER evicted (council S35 guardian F3): evicting a charged key
///    would let a cross-window re-dispatch find no entry and re-buy. A cap full of money-bearing
///    entries REFUSES the new insert fail-closed (the pay path then refuses to broadcast) rather
///    than forgetting a key idempotency depends on.
fn admit_new_key(records: &mut HashMap<String, PaymentRecord>, capsule: &str, pending: bool) -> bool {
    if pending
        && records
            .values()
            .filter(|r| r.status == PaymentStatus::Pending && r.capsule == capsule)
            .count()
            >= PENDING_PER_CAPSULE_CAP
    {
        return false;
    }
    while records.len() >= LEDGER_CAP {
        let oldest_evictable = records
            .values()
            .filter(|r| r.status.is_terminal() && !r.status.is_money_bearing())
            .min_by_key(|r| r.seq)
            .map(|r| r.idempotency_key.clone());
        match oldest_evictable {
            Some(k) => {
                records.remove(&k);
            }
            None => return false,
        }
    }
    true
}

/// A durable, bounded ledger of rail attempts. All mutations persist snapshot-atomically before
/// returning success; a persist failure rolls the mutation back and reports it (the caller decides
/// what that means — the pay path proceeds and says "unrecorded", the reconcile path REFUSES).
pub struct PaymentLedger {
    records: RwLock<HashMap<String, PaymentRecord>>,
    next_seq: std::sync::atomic::AtomicU64,
    storage_path: Option<PathBuf>,
    /// Exclusive advisory flock on `<path>.lock`, held for the ledger's lifetime (council S30
    /// RT-F8/G-F1): the ledger gates money on release, so it carries the METER's single-opener
    /// discipline — a second live instance could last-writer-wins RESURRECT a resolved entry to
    /// pending and reopen refund-exactly-once across processes.
    _lock_file: Option<std::fs::File>,
}

impl PaymentLedger {
    /// In-memory ledger (tests/embedded).
    pub fn new() -> Self {
        Self {
            records: RwLock::new(HashMap::new()),
            next_seq: std::sync::atomic::AtomicU64::new(0),
            storage_path: None,
            _lock_file: None,
        }
    }

    /// Open the durable ledger. Missing file ⇒ fresh; corrupt/oversized/dup-key snapshot ⇒ REFUSE
    /// (fail-closed — booting over a lost pending set would orphan reconciliation obligations).
    pub fn open_durable(path: PathBuf) -> std::io::Result<Self> {
        #[cfg(unix)]
        let lock_file = {
            use std::os::unix::io::AsRawFd as _;
            let lock_path = path.with_extension("lock");
            let f = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&lock_path)?;
            let rc = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if rc != 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "payment ledger is already open elsewhere (single-opener, fail-closed)",
                ));
            }
            Some(f)
        };
        #[cfg(not(unix))]
        let lock_file = None;
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
            _lock_file: lock_file,
        })
    }

    /// Record one rail attempt. Returns `true` iff the entry is durably recorded — `false` means
    /// the caller's user-facing reason must say "unrecorded; reconcile from the error log"
    /// (a full pending set or a failed persist NEVER blocks the payment's own money semantics).
    /// Insert an entry at a chosen status, without the begin/finalize custody protocol.
    ///
    /// TEST/SEEDING ONLY: the production pay path records attempts via
    /// [`begin_attempt`](Self::begin_attempt) + [`finalize`](Self::finalize) (record-before-
    /// broadcast), and reconciliation resolves via [`resolve`](Self::resolve). This direct insert
    /// survives as the way tests seed a ledger into a known state. A key already present is left
    /// untouched (`false`): the first record wins — a replayed key cannot rewrite history.
    pub fn record(
        &self,
        idempotency_key: &str,
        capsule: &str,
        payee: &str,
        amount: u64,
        status: PaymentStatus,
        rail_note: &str,
    ) -> bool {
        self.record_with_token(idempotency_key, capsule, payee, amount, status, rail_note, None)
    }

    /// Like [`record`](Self::record) — TEST/SEEDING ONLY — but binds the mandate token
    /// (`standing_grant_id`) onto the entry so a later reconciliation can key the confirmed
    /// settlement back onto the mandate's receipt. `record` is exactly `record_with_token(.., None)`.
    #[allow(clippy::too_many_arguments)]
    pub fn record_with_token(
        &self,
        idempotency_key: &str,
        capsule: &str,
        payee: &str,
        amount: u64,
        status: PaymentStatus,
        rail_note: &str,
        token_id: Option<&str>,
    ) -> bool {
        let mut records = match self.records.write() {
            Ok(r) => r,
            Err(_) => return false,
        };
        if records.contains_key(idempotency_key) {
            return false;
        }
        if !admit_new_key(&mut records, capsule, status == PaymentStatus::Pending) {
            return false;
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
                refund_applied: false,
                token_id: token_id.map(str::to_string),
            },
        );
        if self.persist_locked(&records).is_err() {
            records.remove(idempotency_key);
            return false;
        }
        true
    }

    /// Durably custody a payment attempt as `Pending` BEFORE any money moves (council S35 red-team
    /// F1). The pay path calls this AFTER reserving the cap and BEFORE broadcasting, so a
    /// re-dispatch can never find "no entry" for a buy whose money moved:
    /// - key absent ⇒ insert a `Pending` placeholder (subject to the per-capsule pending cap +
    ///   the money-bearing-aware ledger bound) ⇒ `Started`, or `CapacityRefused` if it cannot be
    ///   custodied;
    /// - key present + money-bearing (`Pending`/`Performed`/`ResolvedCharged`) ⇒ `AlreadyActive`
    ///   (a concurrent dispatch beat this one; the caller must not move money again);
    /// - key present + provably-nothing-moved (`NotCharged`/`ResolvedNotCharged`) ⇒ REOPEN to
    ///   `Pending` (a legitimate retry the guard allows) ⇒ `Started`.
    ///
    /// The caller then moves money and calls [`finalize`](Self::finalize) with the outcome.
    pub fn begin_attempt(
        &self,
        idempotency_key: &str,
        capsule: &str,
        payee: &str,
        amount: u64,
        token_id: Option<&str>,
    ) -> BeginAttempt {
        let mut records = match self.records.write() {
            Ok(r) => r,
            Err(_) => return BeginAttempt::CapacityRefused,
        };
        if let Some(existing) = records.get(idempotency_key) {
            if existing.status.is_money_bearing() {
                return BeginAttempt::AlreadyActive(existing.status);
            }
            // A provably-nothing-moved terminal ⇒ reopen to Pending for the retry. Snapshot the
            // WHOLE prior record so a failed persist restores exact memory⇄disk agreement —
            // including `rail_note` and the `refund_applied` forensics bit, not just the status.
            let prev = existing.clone();
            if let Some(r) = records.get_mut(idempotency_key) {
                r.status = PaymentStatus::Pending;
                r.rail_note = sanitize_rail_note("reserving");
                r.refund_applied = false;
            }
            if self.persist_locked(&records).is_err() {
                records.insert(idempotency_key.to_string(), prev); // roll back the reopen
                return BeginAttempt::CapacityRefused;
            }
            return BeginAttempt::Started;
        }
        // New key: the same per-capsule pending cap + money-bearing-aware eviction as `record`.
        if !admit_new_key(&mut records, capsule, true) {
            return BeginAttempt::CapacityRefused;
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
                status: PaymentStatus::Pending,
                rail_note: sanitize_rail_note("reserving"),
                seq,
                refund_applied: false,
                token_id: token_id.map(str::to_string),
            },
        );
        if self.persist_locked(&records).is_err() {
            records.remove(idempotency_key);
            return BeginAttempt::CapacityRefused;
        }
        BeginAttempt::Started
    }

    /// Record the OUTCOME of a `begin_attempt` custody, transitioning the `Pending` placeholder to
    /// its final status + rail note (council S35 red-team F1). Only a `Pending` entry is updated
    /// (never clobbers a resolved one); a `Pending` target status keeps it Pending with the tx in
    /// the note (the DRM broadcast-accepted case). Best-effort persist — the in-memory state is
    /// authoritative for the guard; a persist failure is logged by the caller.
    pub fn finalize(&self, idempotency_key: &str, status: PaymentStatus, rail_note: &str) -> bool {
        let mut records = match self.records.write() {
            Ok(r) => r,
            Err(_) => return false,
        };
        match records.get_mut(idempotency_key) {
            Some(r) if r.status == PaymentStatus::Pending => {
                r.status = status;
                r.rail_note = sanitize_rail_note(rail_note);
            }
            _ => return false,
        }
        let _ = self.persist_locked(&records);
        true
    }

    /// Resolve a PENDING entry exactly once. Durable-before-visible with rollback: on success the
    /// entry is terminally `ResolvedCharged`/`ResolvedNotCharged` and can never resolve again
    /// (the double-refund guard the reconcile handler builds on). Errors are honest strings.
    pub fn resolve(
        &self,
        idempotency_key: &str,
        charged: bool,
    ) -> Result<PaymentRecord, ResolveError> {
        let mut records = self.records.write().map_err(|_| ResolveError::Lock)?;
        let rec = records
            .get_mut(idempotency_key)
            .ok_or(ResolveError::NotFound)?;
        if rec.status != PaymentStatus::Pending {
            return Err(ResolveError::NotPending(rec.status));
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
            return Err(ResolveError::Persist);
        }
        Ok(resolved)
    }

    /// Mark a `ResolvedNotCharged` entry's refund as durably applied (called by the reconcile
    /// handler AFTER `try_refund` returns Ok). Best-effort durable: a persist failure keeps the
    /// in-memory mark (the refund DID apply; disk catches up at the next mutation) — the forensic
    /// meaning of `refund_applied=false` on disk is "refund may be lost, check the chain event".
    pub fn mark_refund_applied(&self, idempotency_key: &str) -> bool {
        let mut records = match self.records.write() {
            Ok(r) => r,
            Err(_) => return false,
        };
        match records.get_mut(idempotency_key) {
            Some(r) if r.status == PaymentStatus::ResolvedNotCharged => {
                r.refund_applied = true;
            }
            _ => return false,
        }
        let _ = self.persist_locked(&records);
        true
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

    /// How many entries the ledger holds (read-only projection; 0 on a poisoned lock, matching
    /// `pending()`'s convention).
    pub fn len(&self) -> usize {
        self.records.read().map(|r| r.len()).unwrap_or(0)
    }

    /// True when the ledger holds no entries.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The most recent `limit` entries across ALL statuses, newest first (read-only projection —
    /// the Marketplace panel's buys table, which pairs it with `pending()` so a live obligation
    /// is never truncated away). Bounded by the caller; the ledger itself is bounded by
    /// `LEDGER_CAP`.
    pub fn recent(&self, limit: usize) -> Vec<PaymentRecord> {
        let records = match self.records.read() {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        let mut out: Vec<PaymentRecord> = records.values().cloned().collect();
        out.sort_by_key(|r| std::cmp::Reverse(r.seq));
        out.truncate(limit);
        out
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
        // Fill to the cap with EVICTABLE terminals (NotCharged = provably nothing moved) plus one
        // early pending. Council S35 guardian F3: money-bearing terminals (Performed/
        // ResolvedCharged) are NOT evictable, so only NotCharged/ResolvedNotCharged make room.
        assert!(ledger.record("pend-1", "vm-ap", "acme", 5, PaymentStatus::Pending, ""));
        for i in 0..(LEDGER_CAP - 1) {
            assert!(ledger.record(
                &format!("t-{i}"),
                "vm-ap",
                "acme",
                1,
                PaymentStatus::NotCharged,
                ""
            ));
        }
        // At cap: a new record evicts the OLDEST EVICTABLE terminal (t-0), never pend-1.
        assert!(ledger.record("t-new", "vm-ap", "acme", 1, PaymentStatus::NotCharged, ""));
        assert!(ledger.get("pend-1").is_some(), "pending is never evicted");
        assert!(ledger.get("t-0").is_none(), "oldest evictable terminal was evicted");
        // A cap full of ONLY pending refuses new inserts (bounded obligations). Spread across
        // capsules so the GLOBAL bound is what trips, not the per-capsule one.
        let small = PaymentLedger::new();
        let capsules = LEDGER_CAP / PENDING_PER_CAPSULE_CAP;
        for c in 0..capsules {
            for i in 0..PENDING_PER_CAPSULE_CAP {
                assert!(small.record(
                    &format!("p-{c}-{i}"),
                    &format!("vm-{c}"),
                    "a",
                    1,
                    PaymentStatus::Pending,
                    ""
                ));
            }
        }
        assert!(
            !small.record("one-more", "vm-new", "a", 1, PaymentStatus::Pending, ""),
            "a pending set at the cap refuses new entries instead of growing unbounded"
        );
    }

    /// Council S35 guardian F3 / red-team F1: a money-bearing terminal (`Performed`/
    /// `ResolvedCharged`) is NEVER evicted — evicting a charged key would let a cross-window
    /// re-dispatch find no entry and re-buy. A cap FULL of money-bearing entries refuses a new
    /// insert fail-closed (the pay path then refuses to broadcast), rather than forgetting a key
    /// idempotency depends on.
    #[test]
    fn money_bearing_keys_are_never_evicted() {
        let ledger = PaymentLedger::new();
        for i in 0..LEDGER_CAP {
            assert!(ledger.record(
                &format!("charged-{i}"),
                "vm-ap",
                "acme",
                1,
                PaymentStatus::Performed,
                "drm:tx=0x1;op=o;tid=1",
            ));
        }
        // Cap full of Performed (money-bearing): a NEW insert is REFUSED, none evicted.
        assert!(
            !ledger.record("new-charged", "vm-ap", "acme", 1, PaymentStatus::Performed, ""),
            "a cap full of money-bearing keys refuses new inserts (fail-closed)"
        );
        assert!(ledger.get("charged-0").is_some(), "the oldest charged key survives");
        // begin_attempt refuses too — so the pay path declines WITHOUT broadcasting.
        assert_eq!(
            ledger.begin_attempt("brand-new", "vm-ap", "acme", 1, None),
            BeginAttempt::CapacityRefused
        );
    }

    /// Council S35 red-team F1: begin_attempt custodies a Pending placeholder BEFORE money moves,
    /// reopens a provably-nothing-moved terminal for a retry, and refuses a money-bearing key.
    #[test]
    fn begin_attempt_custodies_reopens_and_refuses() {
        let ledger = PaymentLedger::new();
        // Fresh key ⇒ Started + a Pending placeholder is durably present.
        assert_eq!(
            ledger.begin_attempt("k", "vm-a", "acme", 100, Some("tok")),
            BeginAttempt::Started
        );
        let rec = ledger.get("k").unwrap();
        assert_eq!(rec.status, PaymentStatus::Pending);
        assert_eq!(rec.token_id.as_deref(), Some("tok"));
        // A money-bearing key ⇒ AlreadyActive (a concurrent dispatch must not move money again).
        assert_eq!(
            ledger.begin_attempt("k", "vm-a", "acme", 100, Some("tok")),
            BeginAttempt::AlreadyActive(PaymentStatus::Pending)
        );
        // Finalize to a provably-nothing-moved terminal, then begin_attempt REOPENS for a retry.
        assert!(ledger.finalize("k", PaymentStatus::NotCharged, "rail refused"));
        assert_eq!(
            ledger.begin_attempt("k", "vm-a", "acme", 100, Some("tok")),
            BeginAttempt::Started
        );
        assert_eq!(ledger.get("k").unwrap().status, PaymentStatus::Pending);
        // Finalize charged ⇒ money-bearing ⇒ a retry is refused (idempotent).
        assert!(ledger.finalize("k", PaymentStatus::Performed, "drm:tx=0xabc;op=o;tid=1"));
        assert_eq!(
            ledger.begin_attempt("k", "vm-a", "acme", 100, Some("tok")),
            BeginAttempt::AlreadyActive(PaymentStatus::Performed)
        );
    }

    /// Council S35 guardian F4: a pre-S35 on-disk ledger snapshot (records with NO `token_id` key)
    /// loads, and a legacy record re-persists with no `token_id` key — the field's back-compat is
    /// byte-shape safe (skip_serializing_if + appended last), like the S32 audit pattern.
    #[test]
    fn pre_s35_ledger_snapshot_without_token_id_round_trips() {
        // A record deserialized from a pre-S35 line: no token_id key at all.
        let legacy = r#"{"idempotency_key":"flint-x","capsule":"vm-a","payee":"acme","amount":200,"status":"pending","rail_note":"drm:tx=0xC0;op=o;tid=7","seq":0}"#;
        let rec: PaymentRecord = serde_json::from_str(legacy).unwrap();
        assert!(rec.token_id.is_none(), "absent token_id ⇒ None");
        assert!(!rec.refund_applied, "absent refund_applied ⇒ false");
        // Re-serialization omits both defaulted-absent fields' keys where skip applies: token_id
        // (skip_serializing_if) is omitted; the record is byte-shape compatible.
        let round = serde_json::to_string(&rec).unwrap();
        assert!(
            !round.contains("token_id"),
            "a legacy record re-serializes with NO token_id key: {round}"
        );
    }

    #[test]
    fn one_capsule_cannot_blind_a_victims_obligations() {
        // Council S30 RT-F3: the pending bound is PER CAPSULE — an attacker flooding
        // indeterminates fills only its own quota; a victim's obligation still records.
        let ledger = PaymentLedger::new();
        for i in 0..PENDING_PER_CAPSULE_CAP {
            assert!(ledger.record(
                &format!("atk-{i}"),
                "vm-attacker",
                "a",
                1,
                PaymentStatus::Pending,
                ""
            ));
        }
        assert!(
            !ledger.record("atk-more", "vm-attacker", "a", 1, PaymentStatus::Pending, ""),
            "the attacker capsule is at its own pending cap"
        );
        assert!(
            ledger.record("victim-1", "vm-victim", "b", 7, PaymentStatus::Pending, ""),
            "a victim capsule's obligation still records — the blind is confined"
        );
        assert_eq!(ledger.pending_for("vm-victim"), (7, 1));
    }

    #[test]
    fn second_ledger_opener_is_refused() {
        // Council S30 RT-F8: the ledger gates money on release, so it carries the meter's
        // single-opener flock — a second live instance could resurrect a resolved entry to
        // pending and reopen refund-exactly-once across processes.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("payments.json");
        let first = PaymentLedger::open_durable(path.clone()).unwrap();
        assert!(PaymentLedger::open_durable(path.clone()).is_err());
        drop(first);
        assert!(PaymentLedger::open_durable(path).is_ok());
    }

    #[test]
    fn resolve_errors_are_structured_for_automation() {
        // Council S30 G-F2: 404 (absent) vs 409 (terminal) vs 503 (retry) must be
        // machine-distinguishable — a retryable failure must never read as "already resolved".
        let ledger = PaymentLedger::new();
        assert_eq!(ledger.resolve("ghost", false).unwrap_err(), ResolveError::NotFound);
        assert!(ledger.record("k", "vm", "a", 5, PaymentStatus::NotCharged, ""));
        assert_eq!(
            ledger.resolve("k", false).unwrap_err(),
            ResolveError::NotPending(PaymentStatus::NotCharged),
            "a rail-refunded (terminal) payment can never ALSO be reconcile-refunded"
        );
        // refund_applied marks only a resolved-not-charged entry.
        assert!(ledger.record("p", "vm", "a", 5, PaymentStatus::Pending, ""));
        assert!(!ledger.mark_refund_applied("p"), "pending: not markable");
        ledger.resolve("p", false).unwrap();
        assert!(ledger.mark_refund_applied("p"));
        assert!(ledger.get("p").unwrap().refund_applied);
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
        assert_eq!(
            ledger.resolve("k1", false).unwrap_err(),
            ResolveError::Persist,
            "a transient persist failure is DISTINGUISHABLE from terminal — automation retries"
        );
        assert_eq!(
            ledger.get("k1").unwrap().status,
            PaymentStatus::Pending,
            "an unpersistable resolution rolls back — the refund handle is not lost"
        );
    }

    #[test]
    fn a_failed_reopen_restores_the_whole_prior_record() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("ledger");
        std::fs::create_dir(&sub).unwrap();
        let ledger = PaymentLedger::open_durable(sub.join("payments.json")).unwrap();
        assert!(ledger.record("k", "vm-ap", "acme", 10, PaymentStatus::Pending, "rail said no"));
        ledger.resolve("k", false).unwrap();
        assert!(ledger.mark_refund_applied("k"));
        let before = ledger.get("k").unwrap();
        std::fs::remove_dir_all(&sub).unwrap();
        assert_eq!(
            ledger.begin_attempt("k", "vm-ap", "acme", 10, None),
            BeginAttempt::CapacityRefused,
            "an unpersistable reopen is refused"
        );
        let after = ledger.get("k").unwrap();
        assert_eq!(
            after, before,
            "a failed reopen restores the WHOLE prior record — status, rail_note, and the \
             refund-applied forensics bit, not just the status"
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
