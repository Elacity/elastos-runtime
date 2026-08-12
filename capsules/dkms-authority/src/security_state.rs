//! Durable, versioned SECURITY STATE (DKMS-6): the daemon's authoritative record of standing caller
//! revocations and live delegation revocations.
//!
//! Before Stage 6 both revocation kinds lived ONLY in process memory: caller revocations in the
//! shared [`RevokedSet`], delegation revocations in the injected [`ReplayStore`] — and delegation
//! revocation was wired to no node operation at all. A restart therefore RESURRECTED every revoked
//! caller/delegation, and an operator had no way to reach the key-holding node with a delegation
//! revocation. This module closes both gaps.
//!
//! ## Model — durable truth, in-memory cache
//!
//! The durable file (a single versioned JSON document under the node's operator-owned state root,
//! written through the reviewed Stage 4 primitive [`secure_store::write_atomic_durable`]) is the
//! TRUTH. The in-memory [`RevokedSet`] (caller revocations) and [`ReplayStore`] revocation table
//! (delegation revocations) are CACHES the hot recover path reads; they are hydrated from durable
//! truth at startup and updated only AFTER a durable write succeeds.
//!
//! ## Authority (ADR)
//!
//! Both revocation kinds are OPERATOR-signed — the single administrative trust root the node already
//! pins (`DKMS_AUTHORITY_OPERATOR_VK`), the same one that authorizes caller revocation, rotation, and
//! every lifecycle op. The node pins NO wallet identity, so a wallet-owner cannot self-authorize a
//! delegation revocation at the node (it structurally cannot bind an unseen delegation nonce to a
//! claimed owner). Each persisted record carries the operator signature that authorized it; on load
//! every signature is RE-VERIFIED against the pinned operator vk, so a tampered or unknown-schema or
//! impossible-timestamp state file fails startup CLOSED rather than resetting to empty.
//!
//! ## Fail-closed / fail-safe ordering
//!
//! Persistence enforces bounded capacity (fail closed). A revocation is a DENY primitive: the caller
//! updates the in-memory cache first (so the running process denies immediately, fail-safe) and only
//! reports SUCCESS when the durable write also succeeds — a durable-write failure returns an error
//! and never claims the revocation is durable, so the operator retries until it is. Revocation is
//! PERMANENT for callers (there is no un-revoke op — absence of one means permanence); delegation
//! revocations lapse at their signed `expires_at` and are compacted (crash-safely, via the atomic
//! whole-file write) without ever removing a standing caller revocation.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::secure_store;
use crate::RevokedSet;
use ddrm_envelope::replay::ReplayStore;

/// The one supported durable schema. An unknown value on load fails startup CLOSED (never a silent
/// migration to empty).
const SECURITY_STATE_SCHEMA_V1: &str = "elastos.dkms.authority/security-state/v1";

/// Global cap on standing caller revocations — capacity exhaustion FAILS CLOSED (a revocation is
/// refused rather than admitted un-persisted). Matches the replay store's revocation bound.
pub const MAX_CALLER_REVOCATIONS: usize = 16_384;
/// Global cap on live delegation revocations. Kept at the replay store's own bound so a durable
/// accept implies the in-memory cache can accept too.
pub const MAX_DELEGATION_REVOCATIONS: usize = ddrm_envelope::replay::MAX_REVOCATIONS;

/// One standing, operator-signed caller revocation. `issued_at` is audit metadata (node clock at
/// persist); only `caller_pub_b64` is covered by the operator signature (caller revocation is
/// permanent, so no window is signed).
#[derive(Serialize, Deserialize, Clone)]
struct CallerRevocationRecord {
    caller_pub_b64: String,
    operator_sig_b64: String,
    issued_at: u64,
}

/// One live, operator-signed delegation revocation. The operator signature covers the exact
/// `(nonce, expires_at, issued_at)` triple, so a persisted record cannot be re-aimed at another
/// delegation nor have its window extended after the fact.
#[derive(Serialize, Deserialize, Clone)]
struct DelegationRevocationRecord {
    delegation_nonce_b64: String,
    expires_at: u64,
    issued_at: u64,
    operator_sig_b64: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct DurableState {
    schema: String,
    caller_revocations: Vec<CallerRevocationRecord>,
    delegation_revocations: Vec<DelegationRevocationRecord>,
}

impl DurableState {
    fn empty() -> Self {
        Self {
            schema: SECURITY_STATE_SCHEMA_V1.to_string(),
            caller_revocations: Vec::new(),
            delegation_revocations: Vec::new(),
        }
    }
}

/// The daemon-owned, cheaply-cloneable handle to the durable security state. `Clone` shares one
/// backing store (an `Arc`), like [`ReplayStore`] — one per node process, cloned into every
/// connection so a persisted revocation is durable node-wide. `Default` yields an in-memory-only
/// store (no durable path): persistence is a no-op success and nothing survives a restart — used by
/// fixtures/tests and the stdin dev adapter; the socket/TCP daemon always wires a real path.
#[derive(Clone, Default)]
pub struct SecurityStore(Arc<StoreInner>);

struct StoreInner {
    /// `None` = in-memory-only (no durable persistence).
    path: Option<PathBuf>,
    state: Mutex<DurableState>,
}

impl Default for StoreInner {
    fn default() -> Self {
        Self {
            path: None,
            state: Mutex::new(DurableState::empty()),
        }
    }
}

/// The durable file lives next to the key store, in the SAME operator-owned state root (systemd
/// `StateDirectory`, mode 0700), so it inherits the exact filesystem hardening the master seed
/// relies on.
fn derive_state_path(key_store_path: &str) -> PathBuf {
    PathBuf::from(format!("{key_store_path}.security-state.json"))
}

impl SecurityStore {
    /// Build the daemon's durable store: derive the state path from the key-store path, LOAD +
    /// VALIDATE the persisted state, and HYDRATE the shared caches (`revoked_callers` + `replay`)
    /// from durable truth. Called BEFORE the listener binds. Fail-closed: an unknown schema, an
    /// invalid/absent operator signature, an impossible timestamp, or corrupt bytes is a hard
    /// startup error (never a silent reset to empty). A NON-EXISTENT file is a clean empty state
    /// (first run). `operator_vk` is required to re-verify persisted records; if the state carries
    /// records but no operator identity is pinned, startup fails closed.
    pub fn load_and_hydrate(
        key_store_path: &str,
        operator_vk: Option<Vec<u8>>,
        revoked_callers: &RevokedSet,
        replay: &ReplayStore,
        now: u64,
    ) -> Result<Self, String> {
        let path = derive_state_path(key_store_path);
        secure_store::validate_parent_dir(&path)?;
        let state = read_and_validate(&path, operator_vk.as_deref(), now)?;
        hydrate(&state, revoked_callers, replay, now)?;
        Ok(Self(Arc::new(StoreInner {
            path: Some(path),
            state: Mutex::new(state),
        })))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, DurableState> {
        self.0
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Serialize the CURRENT in-memory state and write it atomically + durably. A no-op success on
    /// an in-memory-only store (no path). Any I/O failure is surfaced so the caller can report the
    /// revocation as NOT durable.
    fn commit_locked(&self, state: &DurableState) -> Result<(), String> {
        let path = match &self.0.path {
            Some(path) => path,
            None => return Ok(()), // in-memory-only: nothing to persist
        };
        let bytes = serde_json::to_vec(state)
            .map_err(|e| format!("could not serialize security state: {e}"))?;
        secure_store::write_atomic_durable(path, &bytes)
            .map_err(|e| format!("could not durably persist security state: {e}"))
    }

    /// Persist a standing caller revocation. Idempotent (a repeat is a no-op success, so the table
    /// cannot grow unboundedly under repeated revokes). Capacity exhaustion fails CLOSED. On a
    /// durable-write failure the in-memory state is rolled back and an error is returned (the caller
    /// must NOT claim success).
    pub fn persist_caller_revocation(
        &self,
        caller_pub_b64: &str,
        operator_sig_b64: &str,
        issued_at: u64,
    ) -> Result<(), String> {
        let mut g = self.lock();
        if g.caller_revocations
            .iter()
            .any(|r| r.caller_pub_b64 == caller_pub_b64)
        {
            return Ok(());
        }
        if g.caller_revocations.len() >= MAX_CALLER_REVOCATIONS {
            return Err(
                "caller revocation table is at capacity — refusing to revoke (fail closed)".into(),
            );
        }
        g.caller_revocations.push(CallerRevocationRecord {
            caller_pub_b64: caller_pub_b64.to_string(),
            operator_sig_b64: operator_sig_b64.to_string(),
            issued_at,
        });
        match self.commit_locked(&g) {
            Ok(()) => Ok(()),
            Err(e) => {
                g.caller_revocations.pop();
                Err(e)
            }
        }
    }

    /// Persist a live delegation revocation. Compacts EXPIRED delegation revocations first (crash-
    /// safely, via the atomic whole-file write) — standing caller revocations are never touched.
    /// Idempotent per nonce (a fresher window replaces an older one). Capacity exhaustion fails
    /// CLOSED. On a durable-write failure the in-memory state is rolled back to its last durable
    /// value and an error is returned.
    pub fn persist_delegation_revocation(
        &self,
        delegation_nonce_b64: &str,
        expires_at: u64,
        issued_at: u64,
        operator_sig_b64: &str,
        now: u64,
    ) -> Result<(), String> {
        let mut g = self.lock();
        let before = g.delegation_revocations.clone();
        // Compaction: drop lapsed revocations (bounded state), and any prior entry for THIS nonce so
        // a re-revoke refreshes rather than duplicates.
        g.delegation_revocations
            .retain(|r| r.expires_at >= now && r.delegation_nonce_b64 != delegation_nonce_b64);
        if g.delegation_revocations.len() >= MAX_DELEGATION_REVOCATIONS {
            g.delegation_revocations = before;
            return Err(
                "delegation revocation table is at capacity — refusing to revoke (fail closed)"
                    .into(),
            );
        }
        g.delegation_revocations.push(DelegationRevocationRecord {
            delegation_nonce_b64: delegation_nonce_b64.to_string(),
            expires_at,
            issued_at,
            operator_sig_b64: operator_sig_b64.to_string(),
        });
        match self.commit_locked(&g) {
            Ok(()) => Ok(()),
            Err(e) => {
                g.delegation_revocations = before;
                Err(e)
            }
        }
    }
}

/// Read + validate the durable state file. A missing file is a clean empty state; any other read
/// error, a symlink, corrupt bytes, an unknown schema, an over-capacity table, an invalid/absent
/// operator signature, or an impossible timestamp is a fail-closed error.
fn read_and_validate(
    path: &Path,
    operator_vk: Option<&[u8]>,
    _now: u64,
) -> Result<DurableState, String> {
    let bytes = match secure_store::read_no_follow(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(DurableState::empty()),
        Err(e) => {
            return Err(format!(
                "security state {} is unreadable — refusing to serve (fail closed): {e}",
                path.display()
            ))
        }
    };
    let state: DurableState = serde_json::from_slice(&bytes).map_err(|e| {
        format!(
            "security state {} is corrupt — refusing to serve (fail closed): {e}",
            path.display()
        )
    })?;
    if state.schema != SECURITY_STATE_SCHEMA_V1 {
        return Err(format!(
            "security state {} has an unknown schema — refusing to serve (fail closed)",
            path.display()
        ));
    }
    if state.caller_revocations.len() > MAX_CALLER_REVOCATIONS
        || state.delegation_revocations.len() > MAX_DELEGATION_REVOCATIONS
    {
        return Err(format!(
            "security state {} exceeds its capacity bound — refusing to serve (fail closed)",
            path.display()
        ));
    }
    // Records exist but no operator identity is pinned to verify them ⇒ fail closed (we can neither
    // trust nor safely discard authenticated deny state).
    let verifier = match operator_vk {
        Some(vk) => Some(
            ddrm_envelope::MlDsa65Verifier::from_encoded(vk).ok_or_else(|| {
                "pinned operator identity is malformed — cannot verify security state".to_string()
            })?,
        ),
        None => {
            if !state.caller_revocations.is_empty() || !state.delegation_revocations.is_empty() {
                return Err(
                    "security state carries revocations but no operator identity is pinned to \
                     verify them — refusing to serve (fail closed)"
                        .to_string(),
                );
            }
            None
        }
    };
    if let Some(verifier) = verifier.as_ref() {
        for rec in &state.caller_revocations {
            let caller_pub = decode_nonempty(&rec.caller_pub_b64)
                .ok_or("a persisted caller revocation has a malformed caller key")?;
            let sig = crate::b64()
                .decode(&rec.operator_sig_b64)
                .map_err(|_| "a persisted caller revocation has a malformed signature")?;
            if !ddrm_envelope::verify_revocation(verifier, &caller_pub, &sig) {
                return Err(
                    "a persisted caller revocation does not verify under the pinned operator \
                     identity — refusing to serve (fail closed)"
                        .to_string(),
                );
            }
        }
        for rec in &state.delegation_revocations {
            if rec.expires_at < rec.issued_at {
                return Err(
                    "a persisted delegation revocation has an impossible timestamp (expiry before \
                     issue) — refusing to serve (fail closed)"
                        .to_string(),
                );
            }
            let nonce = decode_nonempty(&rec.delegation_nonce_b64)
                .filter(|n| n.len() <= ddrm_envelope::replay::MAX_NONCE_BYTES)
                .ok_or("a persisted delegation revocation has a malformed nonce")?;
            let sig = crate::b64()
                .decode(&rec.operator_sig_b64)
                .map_err(|_| "a persisted delegation revocation has a malformed signature")?;
            if !ddrm_envelope::verify_delegation_revocation(
                verifier,
                &nonce,
                rec.expires_at,
                rec.issued_at,
                &sig,
            ) {
                return Err(
                    "a persisted delegation revocation does not verify under the pinned operator \
                     identity — refusing to serve (fail closed)"
                        .to_string(),
                );
            }
        }
    }
    Ok(state)
}

/// Hydrate the in-memory caches from validated durable truth: every caller revocation into the
/// revoked-caller set, every still-live delegation revocation into the replay store. Capacity
/// exhaustion here fails CLOSED (a validated durable record the cache cannot hold is a hard error,
/// not a silent drop).
fn hydrate(
    state: &DurableState,
    revoked_callers: &RevokedSet,
    replay: &ReplayStore,
    now: u64,
) -> Result<(), String> {
    {
        let mut set = revoked_callers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for rec in &state.caller_revocations {
            if let Some(caller_pub) = decode_nonempty(&rec.caller_pub_b64) {
                set.insert(caller_pub);
            }
        }
    }
    for rec in &state.delegation_revocations {
        if rec.expires_at < now {
            continue; // lapsed — will be compacted on the next delegation write
        }
        if let Some(nonce) = decode_nonempty(&rec.delegation_nonce_b64) {
            replay.revoke(&nonce, rec.expires_at, now).map_err(|e| {
                format!("could not hydrate a delegation revocation (fail closed): {e:?}")
            })?;
        }
    }
    Ok(())
}

fn decode_nonempty(b64: &str) -> Option<Vec<u8>> {
    match crate::b64().decode(b64) {
        Ok(bytes) if !bytes.is_empty() => Some(bytes),
        _ => None,
    }
}
