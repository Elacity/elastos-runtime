//! Gateway-side rights gate for the live-chain open path.
//!
//! Anders' rule: the rights DECISION lives in the `rights-provider` capsule, not in
//! the gateway, and the gateway never holds chain RPC. So this module does NOT decide
//! access itself — it obtains a typed on-chain ownership attestation
//! (`ChainAccessAttestationV1`) and hands it to the real `rights-provider` capsule
//! (built with `chain-rights`), which binds it to the request and mints the signed
//! `RightsDecisionReceiptV1`. The gateway reads only the `allowed` bit (to gate) and a
//! stable hash of the receipt (to weld into the decrypt transcript).
//!
//! The attestation has three sources, selected by `ELASTOS_DDRM_RIGHTS`:
//!   - `dev` (default) — a local attestation: owned (the caller already proved local
//!     ownership) unless the CID is in `ELASTOS_DDRM_DENY_CIDS`. Offline, no chain.
//!   - `chain` — the REAL `chain-provider` capsule does an `eth_call` of
//!     `hasAccessByContentId(string,address,string)` against the configured Base
//!     contract (`ELASTOS_CHAIN_BASE_RPC` + `ELASTOS_DDRM_RIGHTS_CONTRACT` +
//!     `ELASTOS_DDRM_RIGHTS_SELECTOR`). This is the production path.
//!   - `chain-mock` — the REAL `chain-provider` path, but pointed at an in-process
//!     JSON-RPC mock (no external network) so owned→opens / not-owned→fail-closed can
//!     be proven locally on a Mac. `ELASTOS_DDRM_CHAIN_ACCESS=denied` flips it to
//!     not-owned. The calldata is still really encoded, sent, and decoded.
//!
//! Mirrors the proven reference in `scripts/dev/ddrm-runtime-open` (the canonical CLI
//! vertical) — same chain-provider contract, same attestation shape, same mock.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Compile-time dev-tree default for the rights-provider capsule built with
/// `--features chain-rights`; override with `ELASTOS_RIGHTS_PROVIDER_BIN`.
const DEV_RIGHTS_PROVIDER_BIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../capsules/rights-provider/target/debug/rights-provider"
);

/// Compile-time dev-tree default for the chain-provider capsule; override with
/// `ELASTOS_CHAIN_PROVIDER_BIN`.
const DEV_CHAIN_PROVIDER_BIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../capsules/chain-provider/target/debug/chain-provider"
);

/// Local-mock canned inputs (only used in `chain-mock`): the mock ignores calldata and
/// answers the configured bool, but the call is still really encoded/sent/decoded.
const MOCK_CONTRACT: &str = "0x00000000000000000000000000000000000000aa";
const MOCK_SELECTOR: &str = "0x12345678";
const MOCK_NETWORK: &str = "base-local-mock";

/// The outcome of a rights decision for an owned-object open.
#[derive(Debug)]
pub struct RightsDecision {
    /// The capsule's verdict — the gate proceeds only when this is true.
    pub allowed: bool,
    /// A stable hash (hex) of the minted `RightsDecisionReceiptV1`, for transcript
    /// binding: the authority welds it into the decrypt AAD so the seal is bound to
    /// THIS rights decision.
    pub receipt_hash_hex: String,
    /// The rights source that produced the attestation (audit/debug only).
    pub source: String,
    /// The full receipt the capsule minted (audit only; carries no authority).
    pub receipt: Value,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RightsMode {
    Dev,
    Chain,
    ChainMock,
}

fn rights_mode() -> RightsMode {
    match std::env::var("ELASTOS_DDRM_RIGHTS").ok().as_deref() {
        Some("chain") => RightsMode::Chain,
        Some("chain-mock") => RightsMode::ChainMock,
        _ => RightsMode::Dev,
    }
}

fn resolve_rights_bin() -> String {
    std::env::var("ELASTOS_RIGHTS_PROVIDER_BIN")
        .unwrap_or_else(|_| DEV_RIGHTS_PROVIDER_BIN.to_string())
}

fn resolve_chain_bin() -> String {
    std::env::var("ELASTOS_CHAIN_PROVIDER_BIN")
        .unwrap_or_else(|_| DEV_CHAIN_PROVIDER_BIN.to_string())
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.trim().is_empty())
}

/// Deterministic placeholder EVM address for `dev` mode when no real wallet subject is
/// supplied. NEVER used in chain mode (which fails closed without a real wallet).
fn dev_subject_address(principal_id: &str) -> String {
    let digest = Sha256::digest(format!("elastos-dev-subject:{principal_id}").as_bytes());
    format!("0x{}", hex::encode(&digest[..20]))
}

/// Is this content explicitly denied for the dev attestation? `ELASTOS_DDRM_DENY_CIDS`
/// is a comma-separated list used to exercise the fail-closed path locally.
fn dev_denies(content_id: &str) -> bool {
    std::env::var("ELASTOS_DDRM_DENY_CIDS")
        .ok()
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .any(|denied| denied == content_id)
        })
        .unwrap_or(false)
}

/// Gate an owned-object open through the rights-provider capsule.
///
/// `content_id` is the object's on-chain content identifier (the asset's CID / KID);
/// `subject` is the principal's linked EVM wallet address (required in chain modes);
/// `right` is the action (`view`). Returns the capsule's decision.
#[allow(clippy::too_many_arguments)]
pub fn decide_owned_access(
    principal_id: &str,
    session_id: &str,
    content_id: &str,
    subject: &str,
    right: &str,
    reason: &str,
    policy_ref: Option<&str>,
    now_unix: u64,
    ttl_secs: u64,
) -> Result<RightsDecision, String> {
    let bin = resolve_rights_bin();
    if !std::path::Path::new(&bin).is_file() {
        return Err(format!(
            "rights-provider (chain-rights) not found at {bin}; build it with \
             `cargo build --manifest-path capsules/rights-provider/Cargo.toml \
             --features chain-rights` or set ELASTOS_RIGHTS_PROVIDER_BIN"
        ));
    }

    let mode = rights_mode();
    // The subject the on-chain check is keyed on. Chain modes REQUIRE a real wallet;
    // dev mode derives a stable placeholder when none is linked.
    let subject = if subject.trim().is_empty() {
        match mode {
            RightsMode::Dev => dev_subject_address(principal_id),
            RightsMode::Chain | RightsMode::ChainMock => {
                return Err("wallet not linked: a chain rights check needs the principal's EVM address".to_string());
            }
        }
    } else {
        subject.to_string()
    };

    let (attestation, source) = match mode {
        RightsMode::Dev => (
            json!({
                "network": "base-mainnet",
                "contract": "0x0000000000000000000000000000000000000001",
                "content_id": content_id,
                "subject": subject,
                "right": right,
                "has_access": !dev_denies(content_id),
            }),
            "dev-local-attestation".to_string(),
        ),
        RightsMode::Chain => chain_attestation(content_id, &subject, right, false)?,
        RightsMode::ChainMock => chain_attestation(content_id, &subject, right, true)?,
    };

    // The rights DECISION is minted by the rights-provider capsule, bound to the request.
    let request_id = format!(
        "rights-{}",
        hex::encode(&Sha256::digest(format!("{content_id}:{principal_id}:{now_unix}"))[..12])
    );
    let decide = json!({
        "op": "decide_access_from_chain",
        "request_id": request_id,
        "request": {
            "principal_id": principal_id,
            "session_id": session_id,
            "content_id": content_id,
            "right": right,
            "reason": reason,
            "policy_ref": policy_ref,
        },
        "chain_access": attestation,
        "now_unix": now_unix,
        "ttl_secs": ttl_secs,
    });

    let data = run_rights_capsule(&bin, &decide)?;
    let receipt = data
        .get("receipt")
        .cloned()
        .ok_or("rights-provider decision missing receipt")?;
    let allowed = receipt
        .get("allowed")
        .and_then(Value::as_bool)
        .ok_or("rights receipt missing allowed")?;
    let receipt_hash_hex = canonical_receipt_hash_hex(&receipt);

    Ok(RightsDecision {
        allowed,
        receipt_hash_hex,
        source,
        receipt,
    })
}

/// Drive the REAL `chain-provider` capsule for an `hasAccessByContentId` ownership read.
/// `mock = true` points it at an in-process JSON-RPC mock (local proof, no network);
/// `mock = false` points it at the configured Base RPC (production). Returns the typed
/// attestation (the exact shape rights-provider's `decide_access_from_chain` consumes)
/// plus a human-readable source label.
fn chain_attestation(
    content_id: &str,
    subject: &str,
    right: &str,
    mock: bool,
) -> Result<(Value, String), String> {
    let chain_bin = resolve_chain_bin();
    if !std::path::Path::new(&chain_bin).is_file() {
        return Err(format!(
            "chain-provider not found at {chain_bin}; build it with \
             `cargo build --manifest-path capsules/chain-provider/Cargo.toml` \
             or set ELASTOS_CHAIN_PROVIDER_BIN"
        ));
    }

    // Resolve network/contract/selector/rpc. Real chain REQUIRES all of them; the mock
    // supplies canned contract/selector and stands up its own loopback RPC.
    let (network, contract, selector, rpc_url, mock_guard) = if mock {
        let owned = std::env::var("ELASTOS_DDRM_CHAIN_ACCESS")
            .ok()
            .map(|s| s != "denied")
            .unwrap_or(true);
        let guard = ChainRpcMock::start(owned)?;
        (
            MOCK_NETWORK.to_string(),
            MOCK_CONTRACT.to_string(),
            MOCK_SELECTOR.to_string(),
            guard.url.clone(),
            Some(guard),
        )
    } else {
        let network = env_nonempty("ELASTOS_DDRM_RIGHTS_NETWORK").unwrap_or_else(|| "base".to_string());
        let contract = env_nonempty("ELASTOS_DDRM_RIGHTS_CONTRACT")
            .ok_or("ELASTOS_DDRM_RIGHTS_CONTRACT (rights contract address) is required for chain mode")?;
        let selector = env_nonempty("ELASTOS_DDRM_RIGHTS_SELECTOR")
            .ok_or("ELASTOS_DDRM_RIGHTS_SELECTOR (has_access selector, e.g. 0x........) is required for chain mode")?;
        let rpc_url = env_nonempty("ELASTOS_CHAIN_BASE_RPC")
            .ok_or("ELASTOS_CHAIN_BASE_RPC (Base RPC URL) is required for chain mode")?;
        (network, contract, selector, rpc_url, None)
    };
    let chain_id: i64 = env_nonempty("ELASTOS_DDRM_CHAIN_ID")
        .and_then(|s| s.parse().ok())
        .unwrap_or(8453);

    let init = json!({
        "op": "init",
        "config": { "networks": [{
            "id": network,
            "display_name": network,
            "kind": "evm_json_rpc",
            "chain_id": chain_id,
            "native_symbol": "ETH",
            "provider": "elastos-gateway",
            "mainnet": true,
            "explorer_url": null,
            "rpc_url": rpc_url,
            "rights_methods": [{
                "id": "has_access_by_content_id",
                "contract": contract,
                "abi": "has_access_by_content_id_string_address_string",
                "selector": selector,
            }]
        }]}
    });
    let query = json!({
        "op": "has_access_by_content_id",
        "network": network,
        "contract": contract,
        "content_id": content_id,
        "subject": subject,
        "right": right,
    });

    let resp = run_chain_capsule(&chain_bin, &init, &query);
    // Keep the mock alive until the query has fully returned, then drop it.
    drop(mock_guard);
    let resp = resp?;

    let attestation = json!({
        "network": resp.get("network").cloned().unwrap_or(json!(network)),
        "contract": resp.get("contract").cloned().unwrap_or(json!(contract)),
        "content_id": resp.get("content_id").cloned().unwrap_or(json!(content_id)),
        "subject": resp.get("subject").cloned().unwrap_or(json!(subject)),
        "right": resp.get("right").cloned().unwrap_or(json!(right)),
        "has_access": resp.get("has_access").and_then(Value::as_bool).unwrap_or(false),
    });
    let source = if mock {
        "chain-provider (in-process mock)".to_string()
    } else {
        format!("chain-provider (live RPC: {network})")
    };
    Ok((attestation, source))
}

/// Spawn chain-provider, send `init` then `has_access_by_content_id` + `shutdown`,
/// returning the query's `data`.
fn run_chain_capsule(bin: &str, init: &Value, query: &Value) -> Result<Value, String> {
    let mut child = Command::new(bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("spawn chain-provider ({bin}): {e}"))?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let mut reader = BufReader::new(child.stdout.take().ok_or("no stdout")?);

    writeln!(stdin, "{init}").map_err(|e| format!("write chain init: {e}"))?;
    stdin.flush().map_err(|e| format!("flush: {e}"))?;
    let init_resp = read_capsule_line(&mut reader)?;
    if init_resp.get("status").and_then(Value::as_str) != Some("ok") {
        let _ = writeln!(stdin, "{}", json!({ "op": "shutdown" }));
        let _ = child.wait();
        return Err(format!(
            "chain-provider init failed: {}",
            init_resp.get("message").and_then(Value::as_str).unwrap_or("unknown")
        ));
    }

    writeln!(stdin, "{query}").map_err(|e| format!("write chain query: {e}"))?;
    stdin.flush().map_err(|e| format!("flush: {e}"))?;
    let query_resp = read_capsule_line(&mut reader)?;

    let _ = writeln!(stdin, "{}", json!({ "op": "shutdown" }));
    let _ = stdin.flush();
    let _ = child.wait();

    if query_resp.get("status").and_then(Value::as_str) != Some("ok") {
        return Err(format!(
            "chain has_access_by_content_id failed: {}",
            query_resp.get("message").and_then(Value::as_str).unwrap_or("unknown")
        ));
    }
    query_resp
        .get("data")
        .cloned()
        .ok_or_else(|| "chain-provider ok response missing data".to_string())
}

/// Spawn the rights-provider capsule, send one request + `shutdown`, return its `data`.
fn run_rights_capsule(bin: &str, request: &Value) -> Result<Value, String> {
    let mut child = Command::new(bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("spawn rights-provider ({bin}): {e}"))?;
    {
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        writeln!(stdin, "{request}").map_err(|e| format!("write rights request: {e}"))?;
        writeln!(stdin, "{}", json!({ "op": "shutdown" }))
            .map_err(|e| format!("write shutdown: {e}"))?;
        stdin.flush().map_err(|e| format!("flush: {e}"))?;
    }
    let mut reader = BufReader::new(child.stdout.take().ok_or("no stdout")?);
    let resp = read_capsule_line(&mut reader);
    let _ = child.wait();
    let resp = resp?;
    if resp.get("status").and_then(Value::as_str) != Some("ok") {
        let message = resp
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("rights-provider error");
        return Err(message.to_string());
    }
    resp.get("data")
        .cloned()
        .ok_or_else(|| "rights-provider ok response missing data".to_string())
}

/// Read one newline-delimited JSON response line from a capsule's stdout.
fn read_capsule_line(reader: &mut impl BufRead) -> Result<Value, String> {
    let mut line = String::new();
    let n = reader
        .read_line(&mut line)
        .map_err(|e| format!("read capsule stdout: {e}"))?;
    if n == 0 {
        return Err("capsule exited before answering".to_string());
    }
    serde_json::from_str(line.trim()).map_err(|e| format!("parse capsule response: {e}"))
}

/// Stable hash of the minted receipt for transcript binding: a domain-separated
/// SHA-256 over the receipt re-serialized with sorted keys, so the gateway, the
/// key-authority, and the decrypt boundary all derive the SAME 32 bytes.
fn canonical_receipt_hash_hex(receipt: &Value) -> String {
    let mut h = Sha256::new();
    h.update(b"elastos-ddrm/rights-binding/v1");
    h.update(canonical_json(receipt).as_bytes());
    hex::encode(h.finalize())
}

/// Re-serialize a JSON value with object keys sorted, for a deterministic digest.
fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            let inner: Vec<String> = entries
                .into_iter()
                .map(|(k, v)| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap_or_default(),
                        canonical_json(v)
                    )
                })
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(canonical_json).collect();
            format!("[{}]", inner.join(","))
        }
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// A minimal in-process JSON-RPC endpoint standing in for a Base RPC node, so the REAL
/// `chain-provider` `eth_call` drives the ownership check with NO external network. It
/// answers every request with a canned 32-byte ABI bool word — `…01` (owned) or `…00`
/// (not owned) — exactly what `has_access_by_content_id` decodes. Mirrors the proven
/// `ChainRpcMock` in `scripts/dev/ddrm-runtime-open`.
struct ChainRpcMock {
    url: String,
    shutdown: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl ChainRpcMock {
    fn start(owned: bool) -> Result<Self, String> {
        let listener =
            TcpListener::bind("127.0.0.1:0").map_err(|e| format!("bind chain RPC mock: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let stop = shutdown.clone();
        let bool_word = format!("0x{:064x}", u8::from(owned));
        let handle = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                match stream {
                    Ok(mut s) => {
                        let _ = serve_one_rpc(&mut s, &bool_word);
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            url: format!("http://127.0.0.1:{port}"),
            shutdown,
            handle: Some(handle),
        })
    }
}

impl Drop for ChainRpcMock {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(self.url.trim_start_matches("http://"));
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Read one HTTP request and write a single JSON-RPC `{ "result": <bool_word> }` 200.
fn serve_one_rpc(stream: &mut TcpStream, bool_word: &str) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    let header_end = loop {
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        if buf.len() > 64 * 1024 {
            break buf.len();
        }
    };
    // Drain any declared Content-Length body so the client's write completes cleanly.
    let headers = String::from_utf8_lossy(&buf[..header_end]).to_lowercase();
    if let Some(idx) = headers.find("content-length:") {
        let len: usize = headers[idx + "content-length:".len()..]
            .lines()
            .next()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        let have = buf.len() - header_end;
        if have < len {
            let mut remaining = len - have;
            while remaining > 0 {
                let n = stream.read(&mut tmp)?;
                if n == 0 {
                    break;
                }
                remaining = remaining.saturating_sub(n);
            }
        }
    }
    let body = format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"{bool_word}\"}}");
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Serialize the process-global env mutation across tests in this module.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    const SUBJECT: &str = "0x00000000000000000000000000000000000000bb";

    /// DEV INTEGRATION (opt-in): drives the REAL chain-provider against the in-process
    /// JSON-RPC mock through the REAL rights-provider, proving owned -> allowed and
    /// not-owned -> denied end to end. Requires the dev-tree binaries:
    ///   cargo build --manifest-path capsules/chain-provider/Cargo.toml
    ///   cargo build --manifest-path capsules/rights-provider/Cargo.toml --features chain-rights
    /// Run with: cargo test -p elastos-server chain_mock_gate -- --ignored
    #[test]
    #[ignore]
    fn chain_mock_gate_allows_owned_and_denies_not_owned() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("ELASTOS_DDRM_RIGHTS", "chain-mock");

        std::env::remove_var("ELASTOS_DDRM_CHAIN_ACCESS");
        let owned = decide_owned_access(
            "did:test:alice", "s1", "bafyowned", SUBJECT, "view", "render", None,
            1_700_000_000, 900,
        )
        .expect("owned decision");
        assert!(owned.allowed, "owned content must be allowed");
        assert!(owned.source.contains("mock"), "source should be the in-process mock");

        std::env::set_var("ELASTOS_DDRM_CHAIN_ACCESS", "denied");
        let denied = decide_owned_access(
            "did:test:alice", "s1", "bafynotowned", SUBJECT, "view", "render", None,
            1_700_000_000, 900,
        )
        .expect("denied decision");
        assert!(!denied.allowed, "not-owned content must be denied (fail closed)");

        std::env::remove_var("ELASTOS_DDRM_RIGHTS");
        std::env::remove_var("ELASTOS_DDRM_CHAIN_ACCESS");
    }

    /// Chain mode with no wallet subject and no override must fail closed (not open).
    #[test]
    fn chain_mode_without_wallet_fails_closed() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("ELASTOS_DDRM_RIGHTS", "chain");
        let result = decide_owned_access(
            "did:test:nowallet", "s1", "bafyx", "", "view", "render", None,
            1_700_000_000, 900,
        );
        std::env::remove_var("ELASTOS_DDRM_RIGHTS");
        let err = result.expect_err("chain mode with no wallet must error");
        assert!(err.contains("wallet not linked"), "unexpected error: {err}");
    }
}
