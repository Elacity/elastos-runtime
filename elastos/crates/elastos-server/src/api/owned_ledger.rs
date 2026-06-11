//! A local owned-access-token ledger for the offline buy→own→open demo loop.
//!
//! In production, ownership lives on-chain: `buyAccess(...)` mints an Access Token and
//! `hasAccessByContentId(...)` reads it back. There is no separate ledger — the chain IS
//! the ledger. But to prove the WHOLE loop on a Mac with no funded wallet and no Base
//! RPC, the `chain-mock` rights mode needs SOMETHING that remembers a purchase across the
//! two separate HTTP requests (the buy, then the open). This file is that memory: a
//! tiny, append-only record of `(content_id, subject)` pairs the local buy step "minted".
//!
//! It carries NO authority on its own — it is only consulted by the `chain-mock` rights
//! attestation (`ELASTOS_DDRM_CHAIN_ACCESS=ledger`), never by `chain` (real RPC) or by
//! the local-sovereign gate. It is the mock's stand-in for on-chain token state, nothing
//! more. Real `chain` mode ignores it entirely.
//!
//! Path: `ELASTOS_DDRM_OWNED_LEDGER` if set, else `<temp>/elastos-ddrm-owned-tokens.json`.

use std::io::Write as _;
use std::path::PathBuf;

use serde_json::{json, Value};

const LEDGER_SCHEMA: &str = "elastos.ddrm.owned_tokens/v1";

/// The ledger file path: an operator override, else a stable temp-dir default.
pub fn ledger_path() -> PathBuf {
    if let Ok(p) = std::env::var("ELASTOS_DDRM_OWNED_LEDGER") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    std::env::temp_dir().join("elastos-ddrm-owned-tokens.json")
}

/// EVM addresses are case-insensitive; compare/store them lowercased so a buy and a
/// later read agree regardless of checksum casing.
fn norm_subject(subject: &str) -> String {
    subject.trim().to_ascii_lowercase()
}

fn load_entries(path: &PathBuf) -> Vec<(String, String)> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        // A corrupt cosmetic ledger must never wedge the loop — start fresh.
        return Vec::new();
    };
    value
        .get("entries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let cid = e.get("content_id").and_then(Value::as_str)?;
            let subj = e.get("subject").and_then(Value::as_str)?;
            Some((cid.to_string(), subj.to_string()))
        })
        .collect()
}

/// Is this `(content_id, subject)` recorded as owned in the local ledger?
pub fn contains(content_id: &str, subject: &str) -> bool {
    let want_subj = norm_subject(subject);
    load_entries(&ledger_path())
        .into_iter()
        .any(|(cid, subj)| cid == content_id && norm_subject(&subj) == want_subj)
}

/// Record `(content_id, subject)` as owned (idempotent). Atomic `*.tmp`→rename so a
/// concurrent reader never sees a half-written file.
pub fn record(content_id: &str, subject: &str) -> Result<(), String> {
    let path = ledger_path();
    let mut entries = load_entries(&path);
    let subject = norm_subject(subject);
    let exists = entries
        .iter()
        .any(|(cid, subj)| cid == content_id && norm_subject(subj) == subject);
    if !exists {
        entries.push((content_id.to_string(), subject.clone()));
    }
    let doc = json!({
        "schema": LEDGER_SCHEMA,
        "entries": entries
            .iter()
            .map(|(cid, subj)| json!({ "content_id": cid, "subject": subj }))
            .collect::<Vec<_>>(),
    });
    let body = serde_json::to_vec_pretty(&doc).map_err(|e| format!("serialize ledger: {e}"))?;

    let tmp = path.with_extension("json.tmp");
    {
        let mut f = std::fs::File::create(&tmp).map_err(|e| format!("ledger tmp create: {e}"))?;
        f.write_all(&body).map_err(|e| format!("ledger write: {e}"))?;
        f.flush().map_err(|e| format!("ledger flush: {e}"))?;
    }
    std::fs::rename(&tmp, &path).map_err(|e| format!("ledger rename: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn record_then_contains_roundtrips_case_insensitively() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("ledger-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("owned.json");
        std::env::set_var("ELASTOS_DDRM_OWNED_LEDGER", &path);

        assert!(!contains("bafyA", "0xABC"));
        record("bafyA", "0xABC").unwrap();
        // Stored lowercased; a checksum-cased read still matches.
        assert!(contains("bafyA", "0xabc"));
        assert!(contains("bafyA", "0xABC"));
        // A different subject or content is still not owned.
        assert!(!contains("bafyA", "0xdef"));
        assert!(!contains("bafyB", "0xABC"));

        // Idempotent: recording again does not duplicate.
        record("bafyA", "0xabc").unwrap();
        let entries = load_entries(&path);
        assert_eq!(entries.len(), 1);

        std::env::remove_var("ELASTOS_DDRM_OWNED_LEDGER");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
