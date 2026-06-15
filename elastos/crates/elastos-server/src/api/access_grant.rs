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

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
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
            .write_all(serde_json::to_vec(input).map_err(|e| e.to_string())?.as_slice())
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
            Pending { session_seed_b64, delegation_json, created_at: now },
        );
    }

    Ok(PreparedGrant { handle, delegation_canonical, owner_address, kid_hex, chain_id })
}

/// PHASE 2 — consume the handle, hand the sidecar the stashed material + the wallet signature, and
/// return the assembled grant JSON. The gateway does NOT verify the wallet signature here — that is
/// the node's job; a bad signature simply fails closed downstream.
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
    let out = run_sidecar(
        "--grant-assemble",
        &json!({
            "session_seed_b64": pending.session_seed_b64,
            "delegation_json": pending.delegation_json,
            "delegation_sig_hex": sig,
        }),
    )?;
    out.get("grant_json")
        .cloned()
        .ok_or_else(|| "sidecar returned no grant_json".to_string())
}

/// Base64 of the grant JSON — the CLI-safe form threaded to the `--quorum` helper.
pub fn grant_to_b64(grant: &Value) -> Result<String, String> {
    let bytes = serde_json::to_vec(grant).map_err(|e| format!("serialize grant: {e}"))?;
    Ok(B64.encode(bytes))
}
