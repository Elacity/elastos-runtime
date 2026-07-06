//! Spend meter — a per-capsule resource budget the consent layer debits, fail-closed at zero.
//!
//! This is the ENFORCEMENT layer that the advisory metrics ([`super::metrics`]) deliberately stop
//! short of: `MetricsManager` *tracks* consumption; `SpendMeter` *bounds* it. A debit that would
//! overrun the remaining budget is refused — fail-closed, atomically, BEFORE the spending action
//! runs — exactly mirroring the single-use capability token's atomic check-and-consume
//! (`capability::store::CapabilityStore::try_use_token`), including the provably-no-op REFUND for an
//! action that was charged but then did not act (the BUG-4 / `DidNotAct` discipline).
//!
//! WIRED: the carrier `carrier_invoke` act path charges this meter — it reserves 1 unit before
//! dispatch (fail-closed if the budget is gone) and reconciles the provider-reported `cost_units`
//! (variable cost) post-success, refunding on the provably-no-op branches. Enabled per deployment
//! via `ELASTOS_DEFAULT_SPEND_BUDGET` on the serve act path; the microVM/WASM carrier sites are
//! follow-ups (see `docs/READY_FOR_CURSOR.md`).
//!
//! KEYING: a budget is keyed by an opaque string the caller picks — in production the canonical
//! `vm-{name}` capsule id (the same identity the carrier gate validates), so a budget bounds one
//! capsule. The unit is the caller's choice (AI tokens, request credits, byte quotas) as long as it
//! is used consistently.
//!
//! FAIL-CLOSED DEFAULT: an unprovisioned key has ZERO budget, not unlimited — an unknown capsule
//! cannot spend. Provisioning ([`SpendMeter::set_budget`]) is an explicit act.
//!
//! DURABILITY (council Sprint 27 F1, closed Sprint 28): the meter has TWO modes, mirroring
//! `StandingGrantStore`:
//!
//! - **In-memory** ([`SpendMeter::new`]) — for rate/credit limiting (the carrier act path), where a
//!   restart refilling the budget is the safe/generous direction and a per-act fsync would be an
//!   unacceptable flood. Mutations never fail.
//! - **Durable** ([`SpendMeter::open_durable`]) — for a MONEY cap, where a restart refilling the cap
//!   would let the intended cumulative limit be exceeded. Every balance mutation is snapshot+fsync'd
//!   (temp + fsync + rename + parent-dir fsync, durable-before-visible) BEFORE it takes effect; a
//!   debit whose reservation cannot be recorded is REFUSED and debits nothing
//!   ([`SpendError::Persist`]), so money never moves against a balance that would not survive a
//!   restart. A corrupt snapshot refuses to open (fail-closed) rather than booting with a silently
//!   refilled cap.
//!
//! The pay affordance remains dev/demo-gated (`ELASTOS_ALLOW_MOCK_PAYMENTS`) until a REAL
//! `PaymentProvider` rail connector replaces the mock (Sprint 29) — durability alone does not make
//! the mock rail honest.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

/// A whole-number quantity of spend (AI tokens, request credits, …). `u64` so a balance can never
/// go negative; the caller owns the unit and stays consistent.
pub type SpendUnits = u64;

/// A read-only view of one key's budget — the projection a UI / inspector / API renders. Per the
/// moat ("every pixel is a read-only projection of real crypto"), a surface shows THIS; it never
/// holds an editable budget field. Serializable so a projection layer can ship it as-is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct BudgetSnapshot {
    pub limit: SpendUnits,
    pub spent: SpendUnits,
    pub remaining: SpendUnits,
}

/// Why a debit was refused. Every variant means **nothing was debited** (fail-closed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpendError {
    /// The requested cost exceeds what remains. The action MUST NOT proceed.
    Exhausted {
        requested: SpendUnits,
        remaining: SpendUnits,
    },
    /// No budget has been provisioned for this key (unknown ⇒ zero, fail-closed).
    NoBudget,
    /// The balance lock was poisoned by a panicking thread — refuse to debit rather than risk an
    /// unbounded spend against a possibly-torn balance.
    Lock,
    /// A durable meter could not persist the mutation. The mutation was ROLLED BACK — a reservation
    /// that would not survive a restart is never granted (in-memory mode never returns this).
    Persist,
    /// The durable meter is POISONED: a prior persist failed AFTER the rename published the new
    /// snapshot (council S28 F1), so a power cut could still revert what memory holds. All further
    /// mutations refuse until the meter is reopened from disk (fail-closed; reads still project).
    Poisoned,
}

impl std::fmt::Display for SpendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpendError::Exhausted {
                requested,
                remaining,
            } => write!(
                f,
                "spend budget exhausted: requested {requested} but only {remaining} remain"
            ),
            SpendError::NoBudget => write!(f, "no spend budget provisioned for this key"),
            SpendError::Lock => write!(f, "spend meter lock poisoned"),
            SpendError::Persist => write!(
                f,
                "spend meter could not durably record the mutation; it was rolled back"
            ),
            SpendError::Poisoned => write!(
                f,
                "spend meter is poisoned (a prior persist failed after publish); mutations \
                 refuse until it is reopened from disk"
            ),
        }
    }
}

impl std::error::Error for SpendError {}

#[derive(Debug, Clone, Copy)]
struct Balance {
    limit: SpendUnits,
    spent: SpendUnits,
}

impl Balance {
    fn remaining(&self) -> SpendUnits {
        self.limit.saturating_sub(self.spent)
    }
}

/// One key's balance as it appears in the durable snapshot (sorted by key for determinism).
#[derive(serde::Serialize, serde::Deserialize)]
struct BalanceRecord {
    key: String,
    limit: SpendUnits,
    spent: SpendUnits,
}

/// The versioned on-disk snapshot of a durable meter.
#[derive(serde::Serialize, serde::Deserialize)]
struct SpendSnapshotV1 {
    version: u32,
    balances: Vec<BalanceRecord>,
}

const SPEND_SNAPSHOT_VERSION: u32 = 1;

/// How a durable persist failed — the distinction council S28 F1 demanded. Before the rename, the
/// OLD snapshot is still the visible file, so rolling back memory restores exact agreement. After
/// the rename the NEW snapshot is published; only the parent-dir fsync (power-cut protection) is
/// missing, so memory must KEEP the mutation (it matches the visible disk) and the meter must
/// POISON (a power cut could still revert the publish — no further mutation may stack on that).
enum PersistFailure {
    PrePublish,
    PostPublish,
}

/// A per-key spend budget with atomic, fail-closed debit and a provably-no-op refund.
///
/// All mutations take a single write lock and complete in one statement, so the balance map is never
/// observed half-updated; a debit can never race another into an overspend (proven by
/// `tests::concurrent_debits_never_overspend`).
///
/// In DURABLE mode ([`open_durable`](Self::open_durable)) every mutation is persisted under that
/// same write lock, durable-before-visible: on a persist failure the mutation is rolled back in
/// memory and surfaced ([`SpendError::Persist`]) — money never moves against a reservation only
/// memory holds.
///
/// POST-PUBLISH failures (council S28 F1, closed S29): when a persist fails AFTER the rename has
/// published the new snapshot (only the parent-dir power-cut fsync missing), memory KEEPS the
/// mutation — it matches the visible disk, so no divergence — and the meter POISONS: every further
/// mutation refuses ([`SpendError::Poisoned`]) until reopened from disk, because stacking more
/// mutations on a publish a power cut could revert would compound the revert window. `try_debit`
/// still refuses the payment in that case (the reservation stays on disk — an orphaned-reservation
/// shape the operator repairs by raising the limit); `try_refund` reports the refund in force
/// (memory and visible disk agree; the only residual, a power-cut revert to the MORE-spent
/// snapshot, is the fail-closed direction).
#[derive(Default)]
pub struct SpendMeter {
    balances: RwLock<HashMap<String, Balance>>,
    /// `Some` ⇒ durable mode: every mutation snapshots to this path before taking effect.
    storage_path: Option<PathBuf>,
    /// Set when a persist failed AFTER publish (council S28 F1): memory matches the visible disk,
    /// but a power cut could revert the publish — every further mutation refuses ([`SpendError::
    /// Poisoned`]) until the meter is reopened from disk. Reads keep projecting.
    poisoned: std::sync::atomic::AtomicBool,
    /// Held for the meter's lifetime: an exclusive advisory flock on `<path>.lock` (council S28
    /// F4), so single-opener no longer depends on the caller's host-lock discipline — a second
    /// opener of the same snapshot fails at `open_durable`, never last-writer-wins clobbering.
    _lock_file: Option<std::fs::File>,
    /// TEST SEAM: force the next persists to fail post-publish (the parent-dir fsync erroring
    /// after a successful rename — unreachable from outside without root-only permission games).
    #[cfg(test)]
    fail_parent_fsync: std::sync::atomic::AtomicBool,
}

impl SpendMeter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a DURABLE meter backed by `path` (the money mode). A missing file is a fresh, empty
    /// meter (every key fail-closed at zero until provisioned); an existing snapshot restores every
    /// balance INCLUDING `spent` — a restart never refills a cap. A corrupt, oversized, duplicated,
    /// or unreadable snapshot REFUSES to open (fail-closed): booting a money meter with silently
    /// zeroed spend would let the cumulative cap be exceeded, exactly what durability exists to
    /// prevent.
    ///
    /// STATED BOUND (council S28): the snapshot is NOT self-authenticating — unlike the signed
    /// audit chain, anyone who can write `data_dir` can forge it (the same trust boundary as the
    /// runtime's key material; a hostile disk already owns the box). SINGLE-OPENER is enforced here
    /// (S29, council F4): an exclusive advisory flock on `<path>.lock` is held for the meter's
    /// lifetime, so a second opener fails fail-closed instead of last-writer-wins clobbering the
    /// other's `spent` — independent of the serve/gateway host lock.
    pub fn open_durable(path: PathBuf) -> std::io::Result<Self> {
        #[cfg(unix)]
        let lock_file = {
            use std::os::unix::io::AsRawFd as _;
            let lock_path = path.with_extension("lock");
            let f = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false) // the lock file carries no content; never disturb it
                .write(true)
                .open(&lock_path)?;
            let rc = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if rc != 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "spend meter snapshot is already open elsewhere (single-opener, fail-closed)",
                ));
            }
            Some(f)
        };
        #[cfg(not(unix))]
        let lock_file = None;
        let mut balances = HashMap::new();
        match std::fs::read(&path) {
            Ok(bytes) => {
                // Size bound before parse: a money meter has at most a few thousand keys; a huge
                // file is a forgery/corruption, not a balance set — refuse rather than OOM at boot.
                if bytes.len() > 4 * 1024 * 1024 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "spend snapshot exceeds the 4 MiB bound",
                    ));
                }
                let snapshot: SpendSnapshotV1 = serde_json::from_slice(&bytes)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                if snapshot.version != SPEND_SNAPSHOT_VERSION {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("unsupported spend snapshot version {}", snapshot.version),
                    ));
                }
                for rec in snapshot.balances {
                    // A duplicate key would silently last-write-win a balance away — the writer
                    // never produces one (it serializes a map), so a dup is tampering: refuse.
                    if balances
                        .insert(
                            rec.key.clone(),
                            Balance {
                                limit: rec.limit,
                                spent: rec.spent,
                            },
                        )
                        .is_some()
                    {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("duplicate key {:?} in spend snapshot", rec.key),
                        ));
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        Ok(Self {
            balances: RwLock::new(balances),
            storage_path: Some(path),
            poisoned: std::sync::atomic::AtomicBool::new(false),
            _lock_file: lock_file,
            #[cfg(test)]
            fail_parent_fsync: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// True once a persist has failed post-publish ([`SpendError::Poisoned`]) — mutations refuse;
    /// reopen from disk to recover. Surfaces so a wiring/ops layer can alarm on it.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn refuse_if_poisoned(&self) -> Result<(), SpendError> {
        if self.is_poisoned() {
            return Err(SpendError::Poisoned);
        }
        Ok(())
    }

    /// True when this meter persists every mutation ([`open_durable`](Self::open_durable)) — the
    /// property a MONEY cap requires. Lets a wiring layer refuse to put real spend on a meter that
    /// would refill across restart.
    pub fn is_durable(&self) -> bool {
        self.storage_path.is_some()
    }

    /// Write the full snapshot atomically (temp + fsync + rename + parent-dir fsync), mirroring
    /// `StandingGrantStore::persist_locked`. Called with the write guard held so the serialized
    /// state is exactly the state that becomes visible. Memory-only ⇒ no-op.
    ///
    /// The failure REPORTS which side of the rename it happened on (council S28 F1): `PrePublish`
    /// means the old snapshot is still the visible file (caller rolls memory back — exact agreement
    /// restored); `PostPublish` means the NEW snapshot is published and only the power-cut
    /// protection (parent-dir fsync) is missing (caller keeps memory and POISONS the meter).
    fn persist_locked(&self, balances: &HashMap<String, Balance>) -> Result<(), PersistFailure> {
        let Some(path) = &self.storage_path else {
            return Ok(());
        };
        let mut records: Vec<BalanceRecord> = balances
            .iter()
            .map(|(key, b)| BalanceRecord {
                key: key.clone(),
                limit: b.limit,
                spent: b.spent,
            })
            .collect();
        records.sort_by(|a, b| a.key.cmp(&b.key));
        let Ok(content) = serde_json::to_vec(&SpendSnapshotV1 {
            version: SPEND_SNAPSHOT_VERSION,
            balances: records,
        }) else {
            return Err(PersistFailure::PrePublish);
        };
        let tmp_path = path.with_extension("tmp");
        let write_and_sync = || -> std::io::Result<()> {
            use std::io::Write as _;
            let mut tmp = std::fs::File::create(&tmp_path)?;
            tmp.write_all(&content)?;
            // Durable BEFORE visible: the rename must never publish bytes still in the page cache.
            tmp.sync_all()
        };
        if write_and_sync().is_err() {
            return Err(PersistFailure::PrePublish);
        }
        if std::fs::rename(&tmp_path, path).is_err() {
            return Err(PersistFailure::PrePublish);
        }
        // PUBLISHED from here on. Without fsyncing the parent directory, a power cut can revert the
        // entry to the OLD snapshot — for a money meter that revert is a refilled cap (an
        // already-reserved spend disappears), so the fsync is part of the write, not a nicety.
        #[cfg(test)]
        if self
            .fail_parent_fsync
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(PersistFailure::PostPublish);
        }
        #[cfg(unix)]
        if let Some(parent) = path.parent() {
            let synced = std::fs::File::open(parent).and_then(|d| d.sync_all());
            if synced.is_err() {
                return Err(PersistFailure::PostPublish);
            }
        }
        Ok(())
    }

    /// Shared post-publish handling: memory KEEPS the mutation (it matches the visible disk) and
    /// the meter poisons — no further mutation may stack on a publish a power cut could revert.
    fn poison(&self) {
        self.poisoned
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Provision (or re-set) a key's TOTAL budget. Raising the limit grants more headroom; lowering
    /// it below what is already spent simply clamps remaining to zero (never refunds silently).
    /// Durable mode: persisted before returning `Ok`; a persist failure rolls the limit back and
    /// returns [`SpendError::Persist`] (in-memory mode never fails).
    ///
    /// Returns the PRIOR limit (`None` = the key was unprovisioned), read under the same write lock
    /// as the mutation — the one linearizable old-value an attestation and a rollback can both trust
    /// (council S28 F6: two lock-free `snapshot()` reads let concurrent provisions attest the same
    /// stale "old").
    pub fn set_budget(
        &self,
        key: &str,
        limit: SpendUnits,
    ) -> Result<Option<SpendUnits>, SpendError> {
        self.refuse_if_poisoned()?;
        let mut balances = match self.balances.write() {
            Ok(b) => b,
            // A poisoned lock here can only mean a prior panic; the map is structurally intact
            // (every write is one statement), so recover the guard rather than drop provisioning.
            Err(poisoned) => poisoned.into_inner(),
        };
        let prior = balances.get(key).copied();
        balances
            .entry(key.to_string())
            .and_modify(|b| b.limit = limit)
            .or_insert(Balance { limit, spent: 0 });
        match self.persist_locked(&balances) {
            Ok(()) => Ok(prior.map(|b| b.limit)),
            Err(PersistFailure::PrePublish) => {
                match prior {
                    Some(bal) => {
                        balances.insert(key.to_string(), bal);
                    }
                    None => {
                        balances.remove(key);
                    }
                }
                Err(SpendError::Persist)
            }
            Err(PersistFailure::PostPublish) => {
                // The new limit IS the visible disk state — keep memory in agreement, poison, and
                // refuse: the caller's loud double-failure path is the right surface for this.
                self.poison();
                Err(SpendError::Persist)
            }
        }
    }

    /// Remove a key's budget entirely (durable, same rollback discipline): afterwards the key is
    /// UNPROVISIONED — fail-closed `NoBudget`, `snapshot() == None` — exactly as if it had never
    /// been set. Returns whether the key existed. Exists so a provisioning surface can truly undo a
    /// failed first-time provision (council S28 F7: rolling an unprovisioned key "back" to limit 0
    /// leaves an enumerable provisioned-at-zero artifact the chain never granted).
    pub fn remove_budget(&self, key: &str) -> Result<bool, SpendError> {
        self.refuse_if_poisoned()?;
        let mut balances = match self.balances.write() {
            Ok(b) => b,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(prior) = balances.remove(key) else {
            return Ok(false);
        };
        match self.persist_locked(&balances) {
            Ok(()) => Ok(true),
            Err(PersistFailure::PrePublish) => {
                balances.insert(key.to_string(), prior);
                Err(SpendError::Persist)
            }
            Err(PersistFailure::PostPublish) => {
                self.poison();
                Err(SpendError::Persist)
            }
        }
    }

    /// Provision `key` with `limit` ONLY if it has no budget yet (idempotent first-touch). Unlike
    /// [`set_budget`](Self::set_budget) this NEVER disturbs an existing budget's limit or spent — so
    /// it is safe to call on every act to lazily provision a per-capsule default without ever
    /// resetting accumulated spend. Durable mode: persists only when it actually inserted; a persist
    /// failure rolls the insert back (the key stays unprovisioned ⇒ fail-closed `NoBudget` on debit).
    pub fn ensure_budget(&self, key: &str, limit: SpendUnits) -> Result<(), SpendError> {
        self.refuse_if_poisoned()?;
        let mut balances = match self.balances.write() {
            Ok(b) => b,
            Err(poisoned) => poisoned.into_inner(),
        };
        if balances.contains_key(key) {
            return Ok(());
        }
        balances.insert(key.to_string(), Balance { limit, spent: 0 });
        match self.persist_locked(&balances) {
            Ok(()) => Ok(()),
            Err(PersistFailure::PrePublish) => {
                balances.remove(key);
                Err(SpendError::Persist)
            }
            Err(PersistFailure::PostPublish) => {
                self.poison();
                Err(SpendError::Persist)
            }
        }
    }

    /// Remaining budget for `key` (0 if unprovisioned).
    pub fn remaining(&self, key: &str) -> SpendUnits {
        match self.balances.read() {
            Ok(balances) => balances.get(key).map(Balance::remaining).unwrap_or(0),
            Err(_) => 0,
        }
    }

    /// A read-only [`BudgetSnapshot`] of `key` for projection (UI / inspector / API). `None` if the
    /// key has no provisioned budget. OBSERVE-only — it never mutates; the meter stays the single
    /// source of truth the projection reflects.
    pub fn snapshot(&self, key: &str) -> Option<BudgetSnapshot> {
        let balances = self.balances.read().ok()?;
        balances.get(key).map(|b| BudgetSnapshot {
            limit: b.limit,
            spent: b.spent,
            remaining: b.remaining(),
        })
    }

    /// Atomically debit `cost` from `key` if it fits, else refuse and debit NOTHING. On success
    /// returns the remaining budget AFTER the debit. `cost == 0` is always allowed (a no-op debit);
    /// `cost == remaining` is allowed (spends to exactly zero).
    ///
    /// Durable mode: the debit is PERSISTED before `Ok` returns — the reservation survives a crash
    /// between this call and the spending action, so the action can never replay against a refilled
    /// cap. A persist failure rolls the debit back and refuses ([`SpendError::Persist`]): money must
    /// not move on a reservation the disk does not hold.
    pub fn try_debit(&self, key: &str, cost: SpendUnits) -> Result<SpendUnits, SpendError> {
        self.refuse_if_poisoned()?;
        let mut balances = self.balances.write().map_err(|_| SpendError::Lock)?;
        let bal = balances.get_mut(key).ok_or(SpendError::NoBudget)?;
        let remaining = bal.remaining();
        if cost > remaining {
            return Err(SpendError::Exhausted {
                requested: cost,
                remaining,
            });
        }
        // cost <= remaining = limit - spent, so spent + cost <= limit: no overflow.
        bal.spent += cost;
        let after = bal.remaining();
        match self.persist_locked(&balances) {
            Ok(()) => Ok(after),
            Err(PersistFailure::PrePublish) => {
                if let Some(bal) = balances.get_mut(key) {
                    bal.spent = bal.spent.saturating_sub(cost);
                }
                Err(SpendError::Persist)
            }
            Err(PersistFailure::PostPublish) => {
                // The debit IS the visible disk state, but its power-cut protection is missing —
                // refuse the payment (fail-closed; the reservation stays, an orphaned-reservation
                // shape the operator can repair) and poison against further churn.
                self.poison();
                Err(SpendError::Persist)
            }
        }
    }

    /// Debit up to `amount`, draining no further than zero, and return the amount ACTUALLY debited
    /// (less than `amount` when the budget could not cover it). For a POST-HOC charge whose action
    /// has ALREADY happened (e.g. a provider reporting the units it actually consumed): the act can
    /// no longer be refused, so an over-budget cost drains the remainder and the next act is refused
    /// fail-closed by [`try_debit`]. Unprovisioned/poisoned ⇒ debits nothing.
    ///
    /// Durable mode: the debit stays in force even if the persist fails (the action ALREADY
    /// happened — rolling back would grant headroom the world has already consumed); the snapshot
    /// catches up at the next successful mutation. The reservation path a MONEY act depends on is
    /// [`try_debit`], which is strictly durable-before-visible.
    pub fn debit_saturating(&self, key: &str, amount: SpendUnits) -> SpendUnits {
        if self.is_poisoned() {
            return 0;
        }
        let mut balances = match self.balances.write() {
            Ok(b) => b,
            Err(_) => return 0,
        };
        let take = match balances.get_mut(key) {
            Some(bal) => {
                let take = amount.min(bal.remaining());
                bal.spent += take;
                take
            }
            None => return 0,
        };
        if let Err(PersistFailure::PostPublish) = self.persist_locked(&balances) {
            self.poison();
        }
        take
    }

    /// Refund a prior debit (saturating). ONLY for a charge whose action provably did NOT occur —
    /// the same contract as `refund_token_use` + `ProviderError::DidNotAct`: refundable only when
    /// nothing acted AND a replay would be a guaranteed no-op. Returns remaining AFTER the refund.
    ///
    /// Conservative under failure: an unknown key or a poisoned lock credits nothing back (a meter
    /// erring toward *more* spent / *less* available is the fail-closed direction for a budget).
    /// Durable mode: a refund whose persist fails is ROLLED BACK in memory (not granted) — the
    /// fail-closed direction. Callers that must RECORD whether the refund actually stuck (a money
    /// path minting a signed reason) use [`try_refund`](Self::try_refund), which distinguishes the
    /// rollback; this convenience face only reports the resulting remaining.
    pub fn refund(&self, key: &str, cost: SpendUnits) -> SpendUnits {
        match self.try_refund(key, cost) {
            Ok(remaining) => remaining,
            Err(_) => self.remaining(key),
        }
    }

    /// [`refund`](Self::refund) with an honest failure channel: `Ok(remaining)` iff the refund is
    /// IN FORCE (and, on a durable meter, persisted); `Err(Persist)` when it was rolled back
    /// (the cap REMAINS DEBITED), `Err(NoBudget)`/`Err(Lock)` when nothing could be credited. A
    /// signed record derived from this call must only claim "refunded" on `Ok` (council S28 F3: the
    /// pay path's Declined reason said "spend refunded" even when the durable refund rolled back).
    pub fn try_refund(&self, key: &str, cost: SpendUnits) -> Result<SpendUnits, SpendError> {
        self.refuse_if_poisoned()?;
        let mut balances = self.balances.write().map_err(|_| SpendError::Lock)?;
        let (refunded, remaining) = match balances.get_mut(key) {
            Some(bal) => {
                let before = bal.spent;
                bal.spent = bal.spent.saturating_sub(cost);
                (before - bal.spent, bal.remaining())
            }
            None => return Err(SpendError::NoBudget),
        };
        match self.persist_locked(&balances) {
            Ok(()) => Ok(remaining),
            Err(PersistFailure::PrePublish) => {
                if let Some(bal) = balances.get_mut(key) {
                    bal.spent += refunded;
                }
                Err(SpendError::Persist)
            }
            Err(PersistFailure::PostPublish) => {
                // The refund IS in force (memory and the visible disk agree) — claiming otherwise
                // would be false — but poison against stacking further mutations on an unfsynced
                // publish. The only residual is a power-cut revert to the MORE-spent snapshot,
                // which is the fail-closed direction.
                self.poison();
                Ok(remaining)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn debit_within_budget_decrements_remaining() {
        let meter = SpendMeter::new();
        meter.set_budget("vm-alice", 100).unwrap();
        assert_eq!(meter.remaining("vm-alice"), 100);
        assert_eq!(meter.try_debit("vm-alice", 30).unwrap(), 70);
        assert_eq!(
            meter.try_debit("vm-alice", 70).unwrap(),
            0,
            "spends to zero"
        );
        assert_eq!(meter.remaining("vm-alice"), 0);
    }

    #[test]
    fn debit_over_budget_is_refused_and_charges_nothing() {
        let meter = SpendMeter::new();
        meter.set_budget("vm-alice", 50).unwrap();
        meter.try_debit("vm-alice", 40).unwrap();
        let err = meter.try_debit("vm-alice", 20).unwrap_err();
        assert_eq!(
            err,
            SpendError::Exhausted {
                requested: 20,
                remaining: 10
            }
        );
        assert_eq!(
            meter.remaining("vm-alice"),
            10,
            "a refused debit must not move the balance"
        );
        // The remaining 10 is still spendable exactly.
        assert_eq!(meter.try_debit("vm-alice", 10).unwrap(), 0);
        assert_eq!(
            meter.try_debit("vm-alice", 1).unwrap_err(),
            SpendError::Exhausted {
                requested: 1,
                remaining: 0
            }
        );
    }

    #[test]
    fn unprovisioned_key_is_fail_closed_zero() {
        let meter = SpendMeter::new();
        assert_eq!(meter.remaining("vm-ghost"), 0);
        assert_eq!(
            meter.try_debit("vm-ghost", 1).unwrap_err(),
            SpendError::NoBudget
        );
    }

    #[test]
    fn refund_restores_only_what_was_spent() {
        let meter = SpendMeter::new();
        meter.set_budget("vm-alice", 100).unwrap();
        meter.try_debit("vm-alice", 60).unwrap();
        assert_eq!(
            meter.refund("vm-alice", 60),
            100,
            "full refund restores headroom"
        );
        // Over-refund cannot exceed the limit (saturating at spent=0 ⇒ remaining=limit).
        assert_eq!(meter.refund("vm-alice", 999), 100);
        assert_eq!(meter.remaining("vm-alice"), 100);
    }

    #[test]
    fn lowering_budget_below_spent_clamps_remaining_to_zero() {
        let meter = SpendMeter::new();
        meter.set_budget("vm-alice", 100).unwrap();
        meter.try_debit("vm-alice", 80).unwrap();
        meter.set_budget("vm-alice", 50).unwrap(); // below the 80 already spent
        assert_eq!(meter.remaining("vm-alice"), 0, "clamped, never negative");
        assert_eq!(
            meter.try_debit("vm-alice", 1).unwrap_err(),
            SpendError::Exhausted {
                requested: 1,
                remaining: 0
            }
        );
    }

    #[test]
    fn snapshot_projects_limit_spent_remaining() {
        let meter = SpendMeter::new();
        assert_eq!(
            meter.snapshot("vm-ghost"),
            None,
            "an unprovisioned key has no budget to project"
        );
        meter.set_budget("vm-alice", 100).unwrap();
        meter.try_debit("vm-alice", 30).unwrap();
        assert_eq!(
            meter.snapshot("vm-alice"),
            Some(BudgetSnapshot {
                limit: 100,
                spent: 30,
                remaining: 70,
            })
        );
    }

    #[test]
    fn debit_saturating_drains_to_zero_and_reports_actual() {
        let meter = SpendMeter::new();
        meter.set_budget("vm-alice", 10).unwrap();
        assert_eq!(
            meter.debit_saturating("vm-alice", 3),
            3,
            "fits: debits all 3"
        );
        // Only 7 remain; an 11-unit post-hoc charge drains the remainder and reports 7.
        assert_eq!(
            meter.debit_saturating("vm-alice", 11),
            7,
            "drains to zero, reports actual"
        );
        assert_eq!(meter.remaining("vm-alice"), 0);
        assert_eq!(
            meter.debit_saturating("vm-ghost", 5),
            0,
            "unprovisioned debits nothing"
        );
    }

    #[test]
    fn ensure_budget_provisions_once_and_never_resets_spend() {
        let meter = SpendMeter::new();
        meter.ensure_budget("vm-alice", 100).unwrap();
        meter.try_debit("vm-alice", 40).unwrap();
        // A second ensure_budget (even with a different limit) must NOT reset the 40 already spent.
        meter.ensure_budget("vm-alice", 5).unwrap();
        assert_eq!(
            meter.remaining("vm-alice"),
            60,
            "ensure_budget is first-touch only; spend is preserved and the limit is untouched"
        );
    }

    #[test]
    fn concurrent_debits_never_overspend() {
        // 64 threads each try to debit 1 from a budget of 40. EXACTLY 40 may succeed — the atomic
        // check-and-debit must never let the 41st through (the property the meter exists to hold).
        let meter = Arc::new(SpendMeter::new());
        meter.set_budget("vm-alice", 40).unwrap();
        let mut handles = Vec::new();
        for _ in 0..64 {
            let m = Arc::clone(&meter);
            handles.push(std::thread::spawn(move || {
                m.try_debit("vm-alice", 1).is_ok()
            }));
        }
        let granted = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(|&ok| ok)
            .count();
        assert_eq!(granted, 40, "exactly the budget was granted, never more");
        assert_eq!(meter.remaining("vm-alice"), 0);
    }

    // === Durable mode (Sprint 28 — the money-cap prerequisites) ===

    #[test]
    fn durable_meter_restart_never_refills_the_cap() {
        // THE money property (council Sprint 27 F1): spend survives a restart. A cap of 500 with
        // 200 already spent must come back as 300 remaining — never a fresh 500.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spend_meter.json");
        {
            let meter = SpendMeter::open_durable(path.clone()).unwrap();
            assert!(meter.is_durable());
            meter.set_budget("vm-ap-agent", 500).unwrap();
            assert_eq!(meter.try_debit("vm-ap-agent", 200).unwrap(), 300);
        } // restart
        let reopened = SpendMeter::open_durable(path).unwrap();
        assert_eq!(
            reopened.remaining("vm-ap-agent"),
            300,
            "the 200 spent before the restart is still spent after it"
        );
        assert!(
            matches!(
                reopened.try_debit("vm-ap-agent", 400),
                Err(SpendError::Exhausted { remaining: 300, .. })
            ),
            "an over-remaining debit is still refused across the restart boundary"
        );
        // An unprovisioned key stays fail-closed after reopen, exactly like a fresh meter.
        assert_eq!(
            reopened.try_debit("vm-ghost", 1).unwrap_err(),
            SpendError::NoBudget
        );
    }

    #[test]
    fn durable_meter_refuses_a_corrupt_snapshot() {
        // Fail-closed boot: a money meter must never open over a snapshot it cannot parse — that
        // would silently zero `spent` and refill every cap. Missing file (fresh install) is fine.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spend_meter.json");
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(
            SpendMeter::open_durable(path.clone()).is_err(),
            "a corrupt snapshot refuses to open"
        );
        std::fs::write(
            &path,
            serde_json::to_vec(&SpendSnapshotV1 {
                version: 999,
                balances: vec![],
            })
            .unwrap(),
        )
        .unwrap();
        assert!(
            SpendMeter::open_durable(path).is_err(),
            "an unknown snapshot version refuses to open"
        );
        assert!(
            SpendMeter::open_durable(dir.path().join("absent.json")).is_ok(),
            "a missing file is a fresh empty meter, not an error"
        );
    }

    #[test]
    fn durable_debit_that_cannot_persist_is_refused_and_rolled_back() {
        // Durable-before-visible: if the reservation cannot land on disk, the debit must be REFUSED
        // and the in-memory balance unchanged — money must never move on a reservation only memory
        // holds. Forced by destroying the snapshot's directory between provisioning and the debit.
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("meter");
        std::fs::create_dir(&sub).unwrap();
        let meter = SpendMeter::open_durable(sub.join("spend_meter.json")).unwrap();
        meter.set_budget("vm-ap-agent", 500).unwrap();
        std::fs::remove_dir_all(&sub).unwrap(); // the persist target is now unwritable
        assert_eq!(
            meter.try_debit("vm-ap-agent", 200).unwrap_err(),
            SpendError::Persist,
            "a debit whose reservation cannot persist is refused"
        );
        assert_eq!(
            meter.remaining("vm-ap-agent"),
            500,
            "the refused debit was rolled back — nothing reserved"
        );
        // Provisioning is equally fail-closed: a set_budget that cannot persist rolls back.
        assert_eq!(
            meter.set_budget("vm-new", 100).unwrap_err(),
            SpendError::Persist
        );
        assert_eq!(
            meter.try_debit("vm-new", 1).unwrap_err(),
            SpendError::NoBudget,
            "the failed provisioning left no budget behind"
        );
    }

    #[test]
    fn durable_refund_that_cannot_persist_is_rolled_back_in_memory() {
        // The refund mirror: headroom memory shows but disk would revoke on restart is a phantom
        // refund — on a persist failure the refund is rolled back IN MEMORY (fail-closed = LESS
        // available), and try_refund SURFACES it so a signed record never claims "refunded" for a
        // refund that is not in force (council S28 F3). Honest bound: this asserts the in-memory
        // outcome; a post-PUBLISH persist failure can leave the refund on disk (module doc, F1).
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("meter");
        std::fs::create_dir(&sub).unwrap();
        let meter = SpendMeter::open_durable(sub.join("spend_meter.json")).unwrap();
        meter.set_budget("vm-ap-agent", 500).unwrap();
        meter.try_debit("vm-ap-agent", 200).unwrap();
        std::fs::remove_dir_all(&sub).unwrap();
        assert_eq!(
            meter.try_refund("vm-ap-agent", 200).unwrap_err(),
            SpendError::Persist,
            "the money path is TOLD the refund did not stick — the cap remains debited"
        );
        assert_eq!(
            meter.refund("vm-ap-agent", 200),
            300,
            "the convenience face reports the unchanged remaining after the rollback"
        );
        assert_eq!(meter.remaining("vm-ap-agent"), 300);
    }

    #[test]
    fn set_budget_returns_the_prior_limit_read_under_the_lock() {
        // Council S28 F6: the attestation's old→new must be linearizable against the mutation, so
        // the prior comes back from set_budget itself, not a separate racy read.
        let meter = SpendMeter::new();
        assert_eq!(
            meter.set_budget("vm-alice", 100).unwrap(),
            None,
            "first provision: no prior"
        );
        assert_eq!(
            meter.set_budget("vm-alice", 250).unwrap(),
            Some(100),
            "re-provision reports the limit it replaced"
        );
    }

    #[test]
    fn remove_budget_truly_unprovisions_durably() {
        // Council S28 F7: the provisioning rollback must be able to UNDO a first-time provision
        // completely — afterwards the key is indistinguishable from never-provisioned (NoBudget,
        // no snapshot), including across a restart.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spend_meter.json");
        {
            let meter = SpendMeter::open_durable(path.clone()).unwrap();
            meter.set_budget("vm-ap-agent", 500).unwrap();
            assert!(meter.remove_budget("vm-ap-agent").unwrap(), "existed");
            assert!(!meter.remove_budget("vm-ap-agent").unwrap(), "idempotent");
            assert_eq!(meter.snapshot("vm-ap-agent"), None);
            assert_eq!(
                meter.try_debit("vm-ap-agent", 1).unwrap_err(),
                SpendError::NoBudget
            );
        }
        let reopened = SpendMeter::open_durable(path).unwrap();
        assert_eq!(
            reopened.snapshot("vm-ap-agent"),
            None,
            "the removal survives restart — no provisioned-at-zero artifact"
        );
        // And a removal that cannot persist is ROLLED BACK — the budget (and its spend) stay.
        let dir2 = tempfile::tempdir().unwrap();
        let sub = dir2.path().join("meter");
        std::fs::create_dir(&sub).unwrap();
        let meter = SpendMeter::open_durable(sub.join("spend_meter.json")).unwrap();
        meter.set_budget("vm-ap-agent", 500).unwrap();
        meter.try_debit("vm-ap-agent", 200).unwrap();
        std::fs::remove_dir_all(&sub).unwrap();
        assert_eq!(
            meter.remove_budget("vm-ap-agent").unwrap_err(),
            SpendError::Persist
        );
        assert_eq!(
            meter.remaining("vm-ap-agent"),
            300,
            "the failed removal left the balance (limit AND spent) intact"
        );
    }

    #[test]
    fn post_publish_persist_failure_poisons_the_meter() {
        // Council S28 F1 (closed S29): a persist that fails AFTER the rename published the new
        // snapshot keeps memory in agreement with the visible disk and POISONS the meter — the
        // payment is still refused, and every further mutation refuses until reopen. Reads project.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spend_meter.json");
        let meter = SpendMeter::open_durable(path.clone()).unwrap();
        meter.set_budget("vm-ap-agent", 500).unwrap();
        meter
            .fail_parent_fsync
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            meter.try_debit("vm-ap-agent", 200).unwrap_err(),
            SpendError::Persist,
            "the payment is refused — its power-cut protection is missing"
        );
        assert!(meter.is_poisoned());
        assert_eq!(
            meter.remaining("vm-ap-agent"),
            300,
            "memory keeps the published debit (it matches the visible disk) — no divergence"
        );
        assert_eq!(
            meter.try_debit("vm-ap-agent", 1).unwrap_err(),
            SpendError::Poisoned,
            "further mutations refuse fail-closed"
        );
        assert_eq!(
            meter.set_budget("vm-ap-agent", 900).unwrap_err(),
            SpendError::Poisoned
        );
        assert_eq!(meter.debit_saturating("vm-ap-agent", 5), 0);
        assert!(
            meter.snapshot("vm-ap-agent").is_some(),
            "reads still project while poisoned"
        );
        // Reopen from disk = the recovery path; the published debit is exactly what disk holds.
        drop(meter);
        let reopened = SpendMeter::open_durable(path).unwrap();
        assert!(!reopened.is_poisoned());
        assert_eq!(reopened.remaining("vm-ap-agent"), 300);
    }

    #[test]
    fn second_opener_of_one_snapshot_is_refused() {
        // Council S28 F4: single-opener is enforced by the meter itself (exclusive flock held for
        // its lifetime) — two live meters on one snapshot would last-writer-wins clobber `spent`.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spend_meter.json");
        let first = SpendMeter::open_durable(path.clone()).unwrap();
        assert!(
            SpendMeter::open_durable(path.clone()).is_err(),
            "a second opener fails fail-closed while the first is alive"
        );
        drop(first);
        assert!(
            SpendMeter::open_durable(path).is_ok(),
            "the lock releases with the meter"
        );
    }

    #[test]
    fn durable_meter_refuses_a_tampered_snapshot_shape() {
        // Council S28 hardening: the writer never produces duplicate keys (it serializes a map),
        // so a duplicate is tampering — last-write-wins would silently swap a balance. And a huge
        // file is a forgery/corruption, not a balance set — bound it before parsing.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spend_meter.json");
        std::fs::write(
            &path,
            br#"{"version":1,"balances":[
                {"key":"vm-a","limit":10,"spent":10},
                {"key":"vm-a","limit":1000,"spent":0}
            ]}"#,
        )
        .unwrap();
        assert!(
            SpendMeter::open_durable(path.clone()).is_err(),
            "a duplicate key refuses to open"
        );
        let mut huge = Vec::from(&b"{\"version\":1,\"balances\":["[..]);
        huge.resize(4 * 1024 * 1024 + 1, b' ');
        std::fs::write(&path, huge).unwrap();
        assert!(
            SpendMeter::open_durable(path).is_err(),
            "an oversized snapshot refuses to open"
        );
    }
}
