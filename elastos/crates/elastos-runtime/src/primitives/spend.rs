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

use std::collections::HashMap;
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

/// A per-key spend budget with atomic, fail-closed debit and a provably-no-op refund.
///
/// All mutations take a single write lock and complete in one statement, so the balance map is never
/// observed half-updated; a debit can never race another into an overspend (proven by
/// `tests::concurrent_debits_never_overspend`).
#[derive(Default)]
pub struct SpendMeter {
    balances: RwLock<HashMap<String, Balance>>,
}

impl SpendMeter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Provision (or re-set) a key's TOTAL budget. Raising the limit grants more headroom; lowering
    /// it below what is already spent simply clamps remaining to zero (never refunds silently).
    pub fn set_budget(&self, key: &str, limit: SpendUnits) {
        let mut balances = match self.balances.write() {
            Ok(b) => b,
            // A poisoned lock here can only mean a prior panic; the map is structurally intact
            // (every write is one statement), so recover the guard rather than drop provisioning.
            Err(poisoned) => poisoned.into_inner(),
        };
        balances
            .entry(key.to_string())
            .and_modify(|b| b.limit = limit)
            .or_insert(Balance { limit, spent: 0 });
    }

    /// Provision `key` with `limit` ONLY if it has no budget yet (idempotent first-touch). Unlike
    /// [`set_budget`](Self::set_budget) this NEVER disturbs an existing budget's limit or spent — so
    /// it is safe to call on every act to lazily provision a per-capsule default without ever
    /// resetting accumulated spend.
    pub fn ensure_budget(&self, key: &str, limit: SpendUnits) {
        let mut balances = match self.balances.write() {
            Ok(b) => b,
            Err(poisoned) => poisoned.into_inner(),
        };
        balances
            .entry(key.to_string())
            .or_insert(Balance { limit, spent: 0 });
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
    pub fn try_debit(&self, key: &str, cost: SpendUnits) -> Result<SpendUnits, SpendError> {
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
        Ok(bal.remaining())
    }

    /// Debit up to `amount`, draining no further than zero, and return the amount ACTUALLY debited
    /// (less than `amount` when the budget could not cover it). For a POST-HOC charge whose action
    /// has ALREADY happened (e.g. a provider reporting the units it actually consumed): the act can
    /// no longer be refused, so an over-budget cost drains the remainder and the next act is refused
    /// fail-closed by [`try_debit`]. Unprovisioned/poisoned ⇒ debits nothing.
    pub fn debit_saturating(&self, key: &str, amount: SpendUnits) -> SpendUnits {
        let mut balances = match self.balances.write() {
            Ok(b) => b,
            Err(_) => return 0,
        };
        match balances.get_mut(key) {
            Some(bal) => {
                let take = amount.min(bal.remaining());
                bal.spent += take;
                take
            }
            None => 0,
        }
    }

    /// Refund a prior debit (saturating). ONLY for a charge whose action provably did NOT occur —
    /// the same contract as `refund_token_use` + `ProviderError::DidNotAct`: refundable only when
    /// nothing acted AND a replay would be a guaranteed no-op. Returns remaining AFTER the refund.
    ///
    /// Conservative under failure: an unknown key or a poisoned lock credits nothing back (a meter
    /// erring toward *more* spent / *less* available is the fail-closed direction for a budget).
    pub fn refund(&self, key: &str, cost: SpendUnits) -> SpendUnits {
        let mut balances = match self.balances.write() {
            Ok(b) => b,
            Err(_) => return 0,
        };
        match balances.get_mut(key) {
            Some(bal) => {
                bal.spent = bal.spent.saturating_sub(cost);
                bal.remaining()
            }
            None => 0,
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
        meter.set_budget("vm-alice", 100);
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
        meter.set_budget("vm-alice", 50);
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
        meter.set_budget("vm-alice", 100);
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
        meter.set_budget("vm-alice", 100);
        meter.try_debit("vm-alice", 80).unwrap();
        meter.set_budget("vm-alice", 50); // below the 80 already spent
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
        meter.set_budget("vm-alice", 100);
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
        meter.set_budget("vm-alice", 10);
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
        meter.ensure_budget("vm-alice", 100);
        meter.try_debit("vm-alice", 40).unwrap();
        // A second ensure_budget (even with a different limit) must NOT reset the 40 already spent.
        meter.ensure_budget("vm-alice", 5);
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
        meter.set_budget("vm-alice", 40);
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
}
