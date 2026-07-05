//! Mandate-scoped durable agent state — the store behind the `runtime.state_put` affordance.
//!
//! The second SIDE-EFFECTING mandate affordance (Sprint 17). Where `runtime.notify` writes a
//! one-shot row into the operator's Inbox, this is durable, mutable, READABLE-BACK state an agent
//! maintains under a mandate: a key → commitment entry, last-write-wins, every write attributed to
//! the mandate + intent that authorized it. It is the honest generalization of the side-effecting
//! pattern to real data — and it stays honest by carrying only the SIGNED declaration's own fields
//! (there is no free-text payload channel; the value an agent commits to is its `input_hash`, the
//! same commitment the mandate receipt already binds), so nothing an agent writes here can smuggle
//! content the intent signature does not cover (the council-F1 lesson from `runtime.notify`).
//!
//! PRINCIPAL-SCOPED: an entry's identity is `(capsule, key)`, so one agent can never read or
//! overwrite another agent's key — the acting `capsule` is gate-bound to the mandate, so this is
//! the same principal-scoping `content_seen` uses, not a spoofable string.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use elastos_common::localhost::rooted_localhost_fs_path;
use serde::{Deserialize, Serialize};

const AGENT_STATE_SCHEMA: &str = "elastos.agent-state/v1";
const AGENT_STATE_ROOT_URI: &str = "localhost://Local/Shared/System/AgentState";
const AGENT_STATE_FILE: &str = "agent_state.json";

/// A cap on the number of distinct keys ONE agent (capsule) may hold, so an agent flooding
/// distinct keys under a mandate cannot grow the store without bound (the council-F1 flood lesson).
/// Newest-written keys are kept. Generous — real agent state is a handful of cursors/flags.
const MAX_KEYS_PER_CAPSULE: usize = 256;

/// One durable agent-state entry: a key an agent wrote under a mandate, and the commitment it made.
/// Every field is either gate-bound (`capsule`, `grant_id`) or lifted verbatim from the signed
/// declaration (`key`, `input_hash`, `intent_id`) — never free-form operator/attacker text.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentStateEntry {
    /// The acting capsule (principal) that owns this key.
    pub capsule: String,
    /// The state key (a slug, ≤64 chars of [A-Za-z0-9._-]).
    pub key: String,
    /// The value the agent COMMITTED to — the declaration's `input_hash` (hex, or empty). This is a
    /// commitment, not a payload: the intent declaration carries no bytes, only their hash, so the
    /// store records exactly what the mandate receipt also binds.
    pub value_hash: String,
    /// The mandate (standing-grant / token id) that authorized the write.
    pub grant_id: String,
    /// The intent that performed this specific write.
    pub intent_id: String,
    /// Unix seconds when the write landed.
    pub written_at: u64,
    /// Monotonic per-key version, incremented on each overwrite (a durable, attributed history
    /// depth without keeping every revision).
    pub version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AgentStateStore {
    #[serde(default)]
    schema: String,
    #[serde(default)]
    entries: Vec<AgentStateEntry>,
}

fn agent_state_path(data_dir: &Path) -> anyhow::Result<PathBuf> {
    let root = rooted_localhost_fs_path(data_dir, AGENT_STATE_ROOT_URI)
        .context("failed to resolve agent-state root")?;
    Ok(root.join(AGENT_STATE_FILE))
}

fn read_store(path: &Path) -> anyhow::Result<AgentStateStore> {
    if !path.exists() {
        return Ok(AgentStateStore::default());
    }
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    if bytes.is_empty() {
        return Ok(AgentStateStore::default());
    }
    serde_json::from_slice(&bytes).with_context(|| format!("invalid {}", path.display()))
}

fn write_store_atomic(path: &Path, store: &AgentStateStore) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("agent-state path missing parent"))?;
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".{AGENT_STATE_FILE}.tmp"));
    let json = serde_json::to_vec_pretty(store)?;
    fs::write(&tmp, &json)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Write (or overwrite, last-write-wins) an agent-state key, PRINCIPAL-SCOPED to `capsule`. Returns
/// the landed entry's version. Fail-closed on I/O: a write that cannot be persisted returns Err (⇒
/// the `runtime.state_put` executor Declines, never a claimed write). Overwriting an existing
/// `(capsule, key)` increments its version; a new key past the per-capsule cap evicts the
/// oldest-written key of THAT capsule (never another capsule's).
pub fn put_agent_state(
    data_dir: &Path,
    capsule: &str,
    key: &str,
    value_hash: &str,
    grant_id: &str,
    intent_id: &str,
) -> anyhow::Result<u64> {
    let path = agent_state_path(data_dir)?;
    let mut store = read_store(&path)?;
    if store.schema.trim().is_empty() {
        store.schema = AGENT_STATE_SCHEMA.to_string();
    }
    let written_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let version = if let Some(existing) = store
        .entries
        .iter_mut()
        .find(|e| e.capsule == capsule && e.key == key)
    {
        // Overwrite in place — last-write-wins, version deepens the attributed history.
        existing.value_hash = value_hash.to_string();
        existing.grant_id = grant_id.to_string();
        existing.intent_id = intent_id.to_string();
        existing.written_at = written_at;
        existing.version = existing.version.saturating_add(1);
        existing.version
    } else {
        store.entries.push(AgentStateEntry {
            capsule: capsule.to_string(),
            key: key.to_string(),
            value_hash: value_hash.to_string(),
            grant_id: grant_id.to_string(),
            intent_id: intent_id.to_string(),
            written_at,
            version: 1,
        });
        // Enforce the per-capsule key cap — evict the oldest-written keys of THIS capsule only, and
        // NEVER the key we just wrote (else `put` would return Ok for a key that is not in the
        // persisted store — a Performed-without-effect honesty break; `written_at` is coarse
        // seconds, so a burst could otherwise tie the new key with evictees). Oldest-first by
        // (written_at, then version) with the just-written (capsule,key) explicitly excluded.
        let mut candidates: Vec<(usize, u64, u64)> = store
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.capsule == capsule && e.key != key)
            .map(|(i, e)| (i, e.written_at, e.version))
            .collect();
        // +1 because the just-written key is excluded from candidates but counts toward the cap.
        if candidates.len() + 1 > MAX_KEYS_PER_CAPSULE {
            candidates.sort_by_key(|(_, ts, ver)| (*ts, *ver));
            let drop = candidates.len() + 1 - MAX_KEYS_PER_CAPSULE;
            let drop_idx: std::collections::HashSet<usize> =
                candidates.iter().take(drop).map(|(i, _, _)| *i).collect();
            let mut i = 0;
            store.entries.retain(|_| {
                let keep = !drop_idx.contains(&i);
                i += 1;
                keep
            });
        }
        1
    };

    write_store_atomic(&path, &store)?;
    Ok(version)
}

/// Read back an agent-state key, PRINCIPAL-SCOPED: only `capsule`'s own key is returned. A missing
/// key (or a different capsule's key) is `None` — never another principal's state. This is the
/// AGENT-facing read (one principal, its own key); an agent must never reach across principals.
pub fn get_agent_state(
    data_dir: &Path,
    capsule: &str,
    key: &str,
) -> anyhow::Result<Option<AgentStateEntry>> {
    let path = agent_state_path(data_dir)?;
    let store = read_store(&path)?;
    Ok(store
        .entries
        .into_iter()
        .find(|e| e.capsule == capsule && e.key == key))
}

/// Every agent-state entry, sorted by `(capsule, key)` — the OPERATOR-facing read. This deliberately
/// spans ALL principals because the caller is the operator/shell (the runtime's grant root, gated by
/// the home-launch token), the same trust level that already sees every mandate. It is NOT an agent
/// path: no agent reaches this — the per-principal isolation (`get_agent_state`) is what protects
/// agents from each other; the operator, who owns the runtime, sees the whole picture.
pub fn list_agent_state(data_dir: &Path) -> anyhow::Result<Vec<AgentStateEntry>> {
    let path = agent_state_path(data_dir)?;
    let mut entries = read_store(&path)?.entries;
    entries.sort_by(|a, b| a.capsule.cmp(&b.capsule).then(a.key.cmp(&b.key)));
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_then_get_is_principal_scoped_and_versions_on_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        // Agent A writes a key.
        let v1 = put_agent_state(d, "vm-a", "cursor", "cafe01", "grant-a", "i1").unwrap();
        assert_eq!(v1, 1);
        let got = get_agent_state(d, "vm-a", "cursor").unwrap().unwrap();
        assert_eq!(got.value_hash, "cafe01");
        assert_eq!(got.version, 1);
        assert_eq!(got.grant_id, "grant-a");

        // Overwrite deepens the version, last-write-wins.
        let v2 = put_agent_state(d, "vm-a", "cursor", "beef02", "grant-a", "i2").unwrap();
        assert_eq!(v2, 2);
        let got = get_agent_state(d, "vm-a", "cursor").unwrap().unwrap();
        assert_eq!(got.value_hash, "beef02");
        assert_eq!(got.version, 2);
        assert_eq!(got.intent_id, "i2");

        // PRINCIPAL-SCOPED: agent B cannot see A's key, and its own same-named key is independent.
        assert!(get_agent_state(d, "vm-b", "cursor").unwrap().is_none());
        put_agent_state(d, "vm-b", "cursor", "d00d03", "grant-b", "i3").unwrap();
        assert_eq!(
            get_agent_state(d, "vm-b", "cursor").unwrap().unwrap().value_hash,
            "d00d03"
        );
        // A's key is untouched by B's write.
        assert_eq!(
            get_agent_state(d, "vm-a", "cursor").unwrap().unwrap().value_hash,
            "beef02"
        );
    }

    #[test]
    fn per_capsule_key_cap_evicts_only_that_capsule() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        // One durable key for capsule B that must SURVIVE A's flood.
        put_agent_state(d, "vm-b", "keep", "b0", "g", "ib").unwrap();
        for i in 0..(MAX_KEYS_PER_CAPSULE + 50) {
            put_agent_state(d, "vm-a", &format!("k{i}"), "aa", "g", &format!("i{i}")).unwrap();
        }
        let store = read_store(&agent_state_path(d).unwrap()).unwrap();
        let a_keys = store.entries.iter().filter(|e| e.capsule == "vm-a").count();
        assert!(a_keys <= MAX_KEYS_PER_CAPSULE, "A capped at {MAX_KEYS_PER_CAPSULE}, got {a_keys}");
        // The operator-facing list spans all principals (sorted), unlike the per-agent read.
        let all = list_agent_state(d).unwrap();
        assert!(all.iter().any(|e| e.capsule == "vm-b" && e.key == "keep"));
        assert!(all.iter().any(|e| e.capsule == "vm-a"));
        assert!(all.windows(2).all(|w| (w[0].capsule.as_str(), w[0].key.as_str())
            <= (w[1].capsule.as_str(), w[1].key.as_str())), "sorted by (capsule, key)");
        // B's key is never evicted by A's flood.
        assert!(get_agent_state(d, "vm-b", "keep").unwrap().is_some());
        // The LAST key written is always readable back — the cap never evicts the just-written key
        // (would be a Performed-without-effect break). Coarse-second timestamps mean the whole
        // flood shares written_at, so this is the real same-second-tie guard.
        let last_key = format!("k{}", MAX_KEYS_PER_CAPSULE + 50 - 1);
        assert!(
            get_agent_state(d, "vm-a", &last_key).unwrap().is_some(),
            "the most recently written key must survive its own cap eviction"
        );
    }
}
