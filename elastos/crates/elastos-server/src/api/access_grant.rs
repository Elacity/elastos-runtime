//! Gateway-side wallet-signed access-grant assembly for the TRUSTLESS dKMS open.
//!
//! The dKMS nodes authorize a recover themselves from a wallet-signed `AccessGrantV1` (verify the
//! wallet/session signatures + read `hasAccessByContentId` on Base, all in-boundary). The gateway's
//! only job is to help the user PRODUCE that grant — it gains no authority by doing so (a forged or
//! stale grant fails closed at the node).
//!
//! The PQ session signer (ML-DSA) is NOT linked here — the gateway shells out to the
//! `ddrm-media-authority` sidecar's `--grant-prepare` / `--grant-assemble` one-shot modes (see this
//! workspace's note: the PQ crypto stack must not feature-unify into elastos-server). The gateway
//! holds only OPAQUE strings (the session seed + the delegation JSON) between the browser's two
//! HTTP calls:
//!
//!   phase 1 (`prepare`): sidecar mints a session key + builds the delegation; we stash the opaque
//!     `(session_seed_b64, delegation_json)` under a random handle and return `delegation_canonical`
//!     — the exact UTF-8 string the browser hands MetaMask for an EIP-191 `personal_sign`.
//!   phase 2 (`assemble`): given the handle + the wallet signature, the sidecar builds + signs the
//!     request and assembles the grant. The pending entry is single-use and expires quickly.

use std::collections::HashMap;
use std::io::Write as _;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use rand::RngCore;
use serde_json::{json, Value};

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

/// A pending grant must be completed (signed + assembled) within this window, or it ages out.
const PENDING_TTL_SECS: u64 = 300;

/// Compile-time dev-tree default for the sidecar (the media-authority helper also serves the
/// grant modes); override with `ELASTOS_DDRM_MEDIA_AUTHORITY_BIN`.
const DEV_MEDIA_AUTHORITY_BIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../scripts/dev/ddrm-media-authority/target/debug/ddrm-media-authority"
);

fn sidecar_bin() -> String {
    std::env::var("ELASTOS_DDRM_MEDIA_AUTHORITY_BIN")
        .unwrap_or_else(|_| DEV_MEDIA_AUTHORITY_BIN.to_string())
}

/// Server-held state for a prepared-but-unsigned grant — OPAQUE to the gateway (no crypto types):
/// the session seed the sidecar re-derives the signer from + the delegation JSON the wallet signs.
struct Pending {
    session_seed_b64: String,
    delegation_json: Value,
    created_at: u64,
}

type PendingStore = Mutex<HashMap<String, Pending>>;

fn store() -> &'static PendingStore {
    static STORE: OnceLock<PendingStore> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A LIVE wallet-signed delegation cached for reuse across opens (PC2 "secure-view session"
/// parity, node-compatible variant). After the wallet signs ONCE, we keep the opaque material
/// (session seed + delegation + the wallet's signature) until the delegation's own `expires_at`,
/// so re-opening the SAME asset within the window assembles a FRESH grant (new per-request nonce +
/// timestamp, signed by the cached session key) WITHOUT another MetaMask popup. The live on-chain
/// `hasAccessByContentId` gate still runs on every open AND every node recover — this only removes
/// the redundant re-signature, never the revocation check. Keyed by (owner, kid): our delegation
/// binds a single kid (the node verifies it), so a session is per-asset; "one signature for all
/// assets" would require dropping kid from the wire schema (a coordinated geo-node redeploy).
struct GrantSession {
    session_seed_b64: String,
    delegation_json: Value,
    delegation_sig_hex: String,
    expires_at: u64,
}

type GrantSessionStore = Mutex<HashMap<String, GrantSession>>;

fn sessions() -> &'static GrantSessionStore {
    static SESSIONS: OnceLock<GrantSessionStore> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Cache key for a live delegation: the on-chain owner + the asset's kid, both normalized so the
/// browser's open path and the prepare path agree on the slot.
fn session_key(owner_address: &str, kid_hex: &str) -> String {
    format!(
        "{}|{}",
        owner_address.trim().to_ascii_lowercase(),
        kid_hex.trim().to_ascii_lowercase()
    )
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn random_b64(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut bytes);
    B64.encode(&bytes)
}

fn sweep_expired(map: &mut HashMap<String, Pending>, now: u64) {
    map.retain(|_, p| now.saturating_sub(p.created_at) <= PENDING_TTL_SECS);
}

/// Run a sidecar one-shot mode: write `input` JSON to stdin, read one JSON object from stdout.
fn run_sidecar(mode: &str, input: &Value) -> Result<Value, String> {
    let bin = sidecar_bin();
    if !std::path::Path::new(&bin).is_file() {
        return Err(format!(
            "grant sidecar not found at {bin}; build scripts/dev/ddrm-media-authority or set ELASTOS_DDRM_MEDIA_AUTHORITY_BIN"
        ));
    }
    let mut child = Command::new(&bin)
        .arg(mode)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("spawn grant sidecar ({bin} {mode}): {e}"))?;
    {
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        stdin
            .write_all(
                serde_json::to_vec(input)
                    .map_err(|e| e.to_string())?
                    .as_slice(),
            )
            .map_err(|e| format!("write sidecar stdin: {e}"))?;
        // stdin dropped here -> EOF, so the sidecar's read_to_string returns.
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("wait sidecar: {e}"))?;
    if !out.status.success() {
        return Err(format!("grant sidecar {mode} exited with {}", out.status));
    }
    let line = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(line.trim()).map_err(|e| format!("parse sidecar output: {e}: {line}"))
}

/// What `prepare` hands back to the browser.
pub struct PreparedGrant {
    pub handle: String,
    pub delegation_canonical: String,
    pub owner_address: String,
    pub kid_hex: String,
    pub chain_id: u64,
}

/// PHASE 1 — sidecar mints a session key + builds the delegation; stash the opaque material and
/// return the canonical text the browser must `personal_sign`. Fails closed without a wallet.
pub fn prepare(
    chain_id: u64,
    kid_hex: &str,
    node_set_id_b64: &str,
    owner_address: &str,
) -> Result<PreparedGrant, String> {
    let owner = owner_address.trim();
    if owner.is_empty() {
        return Err("link an EVM wallet to authorize protected content".to_string());
    }
    let out = run_sidecar(
        "--grant-prepare",
        &json!({
            "chain_id": chain_id,
            "kid_hex": kid_hex,
            "node_set_id_b64": node_set_id_b64,
            "owner_address": owner,
        }),
    )?;
    let session_seed_b64 = out
        .get("session_seed_b64")
        .and_then(Value::as_str)
        .ok_or("sidecar returned no session_seed_b64")?
        .to_string();
    let delegation_json = out
        .get("delegation_json")
        .cloned()
        .ok_or("sidecar returned no delegation_json")?;
    let delegation_canonical = out
        .get("delegation_canonical")
        .and_then(Value::as_str)
        .ok_or("sidecar returned no delegation_canonical")?
        .to_string();
    let owner_address = out
        .get("owner_address")
        .and_then(Value::as_str)
        .unwrap_or(owner)
        .to_string();
    let kid_hex = out
        .get("kid")
        .and_then(Value::as_str)
        .unwrap_or(kid_hex)
        .to_string();

    let handle = random_b64(18);
    {
        let mut map = store().lock().map_err(|_| "grant store poisoned")?;
        let now = now_unix();
        sweep_expired(&mut map, now);
        map.insert(
            handle.clone(),
            Pending {
                session_seed_b64,
                delegation_json,
                created_at: now,
            },
        );
    }

    Ok(PreparedGrant {
        handle,
        delegation_canonical,
        owner_address,
        kid_hex,
        chain_id,
    })
}

/// PHASE 2 — consume the handle, hand the sidecar the stashed material + the wallet signature, and
/// return the assembled grant JSON. The gateway does NOT verify the wallet signature here — that is
/// the node's job; a bad signature simply fails closed downstream.
///
/// On success we ALSO promote the (seed + delegation + signature) into the live session cache keyed
/// by (owner, kid), so the NEXT open of this asset within the delegation window reuses it (no popup).
pub fn assemble(handle: &str, delegation_sig_hex: &str) -> Result<Value, String> {
    let sig = delegation_sig_hex.trim();
    if sig.is_empty() {
        return Err("missing wallet signature".to_string());
    }
    let pending = {
        let mut map = store().lock().map_err(|_| "grant store poisoned")?;
        let now = now_unix();
        sweep_expired(&mut map, now);
        map.remove(handle)
            .ok_or("unknown or expired grant handle (re-prepare the open)")?
    };
    let grant = assemble_with(&pending.session_seed_b64, &pending.delegation_json, sig)?;
    // Cache the live delegation for popup-free re-opens until its own expiry.
    cache_session(&pending.session_seed_b64, &pending.delegation_json, sig);
    Ok(grant)
}

/// Run the sidecar `--grant-assemble` over the given material and return the grant JSON. Each call
/// builds a FRESH per-request object (new nonce + timestamp), so reusing the same delegation +
/// signature across opens yields a new, non-replayable grant every time.
fn assemble_with(
    session_seed_b64: &str,
    delegation_json: &Value,
    delegation_sig_hex: &str,
) -> Result<Value, String> {
    let out = run_sidecar(
        "--grant-assemble",
        &json!({
            "session_seed_b64": session_seed_b64,
            "delegation_json": delegation_json,
            "delegation_sig_hex": delegation_sig_hex,
        }),
    )?;
    out.get("grant_json")
        .cloned()
        .ok_or_else(|| "sidecar returned no grant_json".to_string())
}

/// Promote a just-signed delegation into the live session cache keyed by (owner, kid), with the
/// delegation's own `expires_at` as the TTL. Best-effort: a malformed delegation simply isn't cached.
fn cache_session(session_seed_b64: &str, delegation_json: &Value, delegation_sig_hex: &str) {
    let owner = delegation_json
        .get("owner_address")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let kid = delegation_json
        .get("kid_hex")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let expires_at = delegation_json
        .get("expires_at")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if owner.is_empty() || kid.is_empty() || expires_at == 0 {
        return;
    }
    if let Ok(mut map) = sessions().lock() {
        let now = now_unix();
        map.retain(|_, s| s.expires_at > now);
        map.insert(
            session_key(owner, kid),
            GrantSession {
                session_seed_b64: session_seed_b64.to_string(),
                delegation_json: delegation_json.clone(),
                delegation_sig_hex: delegation_sig_hex.to_string(),
                expires_at,
            },
        );
    }
}

/// True iff a non-expired wallet-signed delegation is cached for (owner, kid) — i.e. the browser can
/// skip the MetaMask popup and let the gateway assemble a fresh grant from the cache.
pub fn has_live_session(owner_address: &str, kid_hex: &str) -> bool {
    let key = session_key(owner_address, kid_hex);
    if let Ok(mut map) = sessions().lock() {
        let now = now_unix();
        map.retain(|_, s| s.expires_at > now);
        return map.contains_key(&key);
    }
    false
}

/// Assemble a fresh grant from a cached live delegation for (owner, kid), WITHOUT a new wallet
/// signature. Returns `Ok(None)` when no live session is cached (caller falls back to prepare+sign).
pub fn assemble_cached(owner_address: &str, kid_hex: &str) -> Result<Option<Value>, String> {
    let key = session_key(owner_address, kid_hex);
    let (seed, delegation, sig) = {
        let mut map = sessions()
            .lock()
            .map_err(|_| "grant session store poisoned")?;
        let now = now_unix();
        map.retain(|_, s| s.expires_at > now);
        match map.get(&key) {
            Some(s) => (
                s.session_seed_b64.clone(),
                s.delegation_json.clone(),
                s.delegation_sig_hex.clone(),
            ),
            None => return Ok(None),
        }
    };
    let grant = assemble_with(&seed, &delegation, &sig)?;
    Ok(Some(grant))
}

/// Base64 of the grant JSON — the CLI-safe form threaded to the `--quorum` helper.
pub fn grant_to_b64(grant: &Value) -> Result<String, String> {
    let bytes = serde_json::to_vec(grant).map_err(|e| format!("serialize grant: {e}"))?;
    Ok(B64.encode(bytes))
}

/// Choose the grant base64 to send on quorum-open `attempt` (1-based).
///
/// The dKMS node's replay guard makes each grant's per-request nonce SINGLE-USE, so re-sending the
/// same grant on a retry is (correctly) rejected as a replay — which would turn transient Carrier
/// transport flakiness on attempt 1 into a hard "access grant rejected: replayed" open failure
/// (DKMS_OVER_CARRIER known-gap). On attempts after the first we therefore assemble a FRESH grant
/// from the cached wallet-signed delegation: a new per-request nonce + timestamp, signed by the
/// cached session key — no MetaMask popup, and the SAME delegation signature, so the forensic
/// watermark anchor is unchanged. Fail-soft: if no live session is cached or regeneration fails,
/// fall back to the original grant (never worse than the pre-fix behavior). Returns `None` only
/// when there was no grant to begin with (the legacy enrolled-caller path, which carries no
/// replay-protected nonce). Shells the grant sidecar via `assemble_cached`, so call it off the
/// async executor (e.g. inside `spawn_blocking`).
pub fn grant_for_attempt(
    original: Option<&str>,
    owner_address: &str,
    kid_hex: &str,
    attempt: usize,
) -> Option<String> {
    pick_grant_for_attempt(original, attempt, || {
        match assemble_cached(owner_address, kid_hex) {
            Ok(Some(grant)) => grant_to_b64(&grant).ok(),
            _ => None,
        }
    })
}

/// I/O-free core of [`grant_for_attempt`] so the per-attempt decision is unit-testable without the
/// grant sidecar. `regenerate` is only invoked on retries (attempt > 1).
fn pick_grant_for_attempt(
    original: Option<&str>,
    attempt: usize,
    regenerate: impl FnOnce() -> Option<String>,
) -> Option<String> {
    match original {
        // Legacy enrolled-caller path: no wallet grant on the wire, so no single-use nonce to refresh.
        None => None,
        // First attempt uses the grant assembled up front for this open.
        Some(orig) if attempt <= 1 => Some(orig.to_string()),
        // Retry: send a fresh per-request grant; fall back to the original if we cannot regenerate.
        Some(orig) => Some(regenerate().unwrap_or_else(|| orig.to_string())),
    }
}

#[cfg(test)]
mod attempt_tests {
    use super::*;

    #[test]
    fn attempt_one_uses_original_without_regenerating() {
        let mut regenerated = false;
        let g = pick_grant_for_attempt(Some("ORIG"), 1, || {
            regenerated = true;
            Some("FRESH".to_string())
        });
        assert_eq!(g.as_deref(), Some("ORIG"));
        assert!(
            !regenerated,
            "attempt 1 must not waste a sidecar call regenerating"
        );
    }

    #[test]
    fn retry_uses_freshly_regenerated_grant() {
        // The load-bearing A7 fix: a retry must carry a DIFFERENT (fresh-nonce) grant, not the
        // original, or the node's single-use replay guard rejects the legitimate retry.
        let g = pick_grant_for_attempt(Some("ORIG"), 2, || Some("FRESH".to_string()));
        assert_eq!(g.as_deref(), Some("FRESH"));
    }

    #[test]
    fn retry_falls_back_to_original_when_regeneration_unavailable() {
        let g = pick_grant_for_attempt(Some("ORIG"), 3, || None);
        assert_eq!(
            g.as_deref(),
            Some("ORIG"),
            "no cached session -> fail soft to original"
        );
    }

    #[test]
    fn no_original_grant_stays_none() {
        let g = pick_grant_for_attempt(None, 2, || Some("FRESH".to_string()));
        assert_eq!(g, None);
    }
}
