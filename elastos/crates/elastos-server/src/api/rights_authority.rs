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
//!   - `chain` — the REAL `chain-provider` capsule does an `eth_call` of the real Base
//!     `hasAccessByContentId(address holder, bytes16 contentId)` (selector `0x54d42821`)
//!     against the AuthorityGateway. Network/contract/selector default to the real Base
//!     values (`~/.pc2` `contracts/abis.ts`) and are overridable via
//!     `ELASTOS_DDRM_RIGHTS_NETWORK` / `_CONTRACT` / `_SELECTOR`; only
//!     `ELASTOS_CHAIN_BASE_RPC` is required. This is the production path.
//!   - `chain-mock` — the REAL `chain-provider` path, but pointed at an in-process
//!     JSON-RPC mock (no external network) so owned→opens / not-owned→fail-closed can
//!     be proven locally on a Mac. `ELASTOS_DDRM_CHAIN_ACCESS=denied` flips it to
//!     not-owned. The calldata is still really encoded, sent, and decoded.
//!
//! Mirrors the proven reference in `scripts/dev/ddrm-runtime-open` (the canonical CLI
//! vertical) — same chain-provider contract, same attestation shape, same mock.

use std::io::{BufReader, Read, Write};
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

/// Real Base mainnet defaults (from `~/.pc2` `packages/access/src/contracts/abis.ts`),
/// used in `chain` mode when not overridden. The AuthorityGateway is the rights contract;
/// the selector is `keccak256("hasAccessByContentId(address,bytes16)")[..4]`.
const BASE_AUTHORITY_GATEWAY: &str = "0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D";
const BASE_HAS_ACCESS_SELECTOR: &str = "0x54d42821";

/// The real Base ABI shape for the rights read (`hasAccessByContentId(address,bytes16)`);
/// `chain-mock` keeps the tolerant string shape because local CIDs aren't `bytes16` KIDs.
const LIVE_RIGHTS_ABI: &str = "has_access_by_content_id_address_bytes16";
const MOCK_RIGHTS_ABI: &str = "has_access_by_content_id_string_address_string";

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RightsMode {
    // Dev/ChainMock are constructed by rights_mode() ONLY in a `dev-modes` build
    // (DEV_MODE_GUARD_SPEC); a release build never produces them, but they remain valid
    // match targets, so suppress the "never constructed" lint for the non-dev build.
    #[cfg_attr(not(feature = "dev-modes"), allow(dead_code))]
    Dev,
    Chain,
    #[cfg_attr(not(feature = "dev-modes"), allow(dead_code))]
    ChainMock,
}

pub(crate) fn rights_mode() -> RightsMode {
    match std::env::var("ELASTOS_DDRM_RIGHTS").ok().as_deref() {
        Some("chain") => RightsMode::Chain,
        // `dev` (free unlocks, no chain) and `chain-mock` (in-process RPC) are INSECURE
        // conveniences. They are reachable ONLY in a `dev-modes` build (DEV_MODE_GUARD_SPEC):
        // a plain release build defaults to — and cannot leave — the secure `Chain` path, so a
        // production deploy can never silently hand out content for free. (The startup guard
        // additionally refuses to boot when a release build is *handed* a dev mode, so the
        // misconfiguration is loud rather than silently upgraded.)
        #[cfg(feature = "dev-modes")]
        Some("chain-mock") => RightsMode::ChainMock,
        #[cfg(feature = "dev-modes")]
        _ => RightsMode::Dev,
        #[cfg(not(feature = "dev-modes"))]
        _ => RightsMode::Chain,
    }
}

/// Fail-closed build-configuration guard (DEV_MODE_GUARD_SPEC; PRINCIPLE #11 — "fail closed,
/// then explain" — extended to *build config*). A RELEASE build (compiled WITHOUT `dev-modes`)
/// must REFUSE TO START if it was handed an insecure dev rights mode, rather than silently
/// overriding it to `Chain` — so an accidental production misconfiguration is loud, not silent.
/// In a `dev-modes` build this is a no-op (the dev modes are intentionally available).
pub(crate) fn enforce_release_build_rights_safety() -> Result<(), String> {
    #[cfg(not(feature = "dev-modes"))]
    {
        if let Ok(v) = std::env::var("ELASTOS_DDRM_RIGHTS") {
            let v = v.trim();
            if v == "dev" || v == "chain-mock" {
                return Err(format!(
                    "ELASTOS_DDRM_RIGHTS=\"{v}\" selects an INSECURE dev rights mode (free unlocks / \
                     no live on-chain check), but this binary is a release build compiled WITHOUT the \
                     `dev-modes` feature. Refusing to start (fail closed). Set ELASTOS_DDRM_RIGHTS=chain \
                     for production, or rebuild with `--features dev-modes` for local/CI."
                ));
            }
        }
    }
    Ok(())
}

fn resolve_rights_bin() -> String {
    std::env::var("ELASTOS_RIGHTS_PROVIDER_BIN")
        .unwrap_or_else(|_| DEV_RIGHTS_PROVIDER_BIN.to_string())
}

pub(crate) fn resolve_chain_bin() -> String {
    std::env::var("ELASTOS_CHAIN_PROVIDER_BIN")
        .unwrap_or_else(|_| DEV_CHAIN_PROVIDER_BIN.to_string())
}

pub(crate) fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.trim().is_empty())
}

/// Deterministic placeholder EVM address for `dev` mode when no real wallet subject is
/// supplied. NEVER used in chain mode (which fails closed without a real wallet).
fn dev_subject_address(principal_id: &str) -> String {
    let digest = Sha256::digest(format!("elastos-dev-subject:{principal_id}").as_bytes());
    format!("0x{}", hex::encode(&digest[..20]))
}

/// PC2's `normalizedKid` rule (`storage.ts`: `kid.startsWith('0x') ? kid : '0x'+kid`):
/// the on-chain `bytes16 contentId` is the KID, and PC2 always passes it `0x`-prefixed.
/// Our capsule stores the KID as a bare 32-hex string, so prefix it for the strict
/// chain-provider encoder. Anything that is NOT a bare 32-hex KID (already-prefixed, or a
/// non-hex CID) passes through unchanged — so a misconfigured id still reaches the encoder
/// and fails loudly there rather than being silently mangled.
fn normalize_kid_0x(content_id: &str) -> String {
    let trimmed = content_id.trim();
    if trimmed.len() == 32 && trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        format!("0x{trimmed}")
    } else {
        trimmed.to_string()
    }
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
    let mode = rights_mode();
    // The subject the on-chain check is keyed on. Chain modes REQUIRE a real wallet;
    // dev mode derives a stable placeholder when none is linked. Validate this FIRST — a
    // request with no linked wallet in a chain mode is invalid on its face and must fail
    // closed BEFORE we resolve or spawn any capsule binary (and so the check is hermetic:
    // it does not depend on the rights-provider binary being present).
    let subject = if subject.trim().is_empty() {
        match mode {
            RightsMode::Dev => dev_subject_address(principal_id),
            RightsMode::Chain | RightsMode::ChainMock => {
                return Err(
                    "wallet not linked: a chain rights check needs the principal's EVM address"
                        .to_string(),
                );
            }
        }
    } else {
        subject.to_string()
    };

    let bin = resolve_rights_bin();
    if !std::path::Path::new(&bin).is_file() {
        return Err(format!(
            "rights-provider (chain-rights) not found at {bin}; build it with \
             `cargo build --manifest-path capsules/rights-provider/Cargo.toml \
             --features chain-rights` or set ELASTOS_RIGHTS_PROVIDER_BIN"
        ));
    }

    // PC2 parity: the live `hasAccessByContentId(address,bytes16)` read keys on the
    // 0x-prefixed KID (PC2's `normalizedKid = kid.startsWith('0x') ? kid : '0x'+kid`). Our
    // `.ddrm` capsule stores the KID as a BARE 32-hex string, but the chain-provider's strict
    // bytes16 encoder requires the `0x` prefix — so normalize it for the live chain path.
    // The SAME normalized value is bound into the rights-provider request below so the
    // capsule's `attestation.content_id == request.content_id` check still matches. Only the
    // real `chain` mode is normalized: `dev`/`chain-mock` use the tolerant string shape and
    // may carry non-hex CIDs that must NOT be `0x`-prefixed.
    let chain_content_id = match mode {
        RightsMode::Chain => normalize_kid_0x(content_id),
        RightsMode::Dev | RightsMode::ChainMock => content_id.to_string(),
    };

    let (attestation, source) = match mode {
        RightsMode::Dev => (
            json!({
                "network": "base-mainnet",
                "contract": "0x0000000000000000000000000000000000000001",
                "content_id": chain_content_id,
                "subject": subject,
                "right": right,
                "has_access": !dev_denies(content_id),
            }),
            "dev-local-attestation".to_string(),
        ),
        RightsMode::Chain => chain_attestation(&chain_content_id, &subject, right, false)?,
        RightsMode::ChainMock => chain_attestation(&chain_content_id, &subject, right, true)?,
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
            "content_id": chain_content_id,
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
    let (network, contract, selector, abi, rpc_url, mock_guard) = if mock {
        // The mock's ownership answer, by precedence:
        //   - `ELASTOS_DDRM_CHAIN_ACCESS=denied` -> not owned (force the fail-closed path)
        //   - `ELASTOS_DDRM_CHAIN_ACCESS=owned`  -> owned
        //   - `ELASTOS_DDRM_CHAIN_ACCESS=ledger` -> owned IFF the local owned-token ledger
        //       has this `(content_id, subject)` (the offline buy->own->open loop)
        //   - unset / other -> owned (back-compat: the "everything owned" local demo)
        let owned = match env_nonempty("ELASTOS_DDRM_CHAIN_ACCESS").as_deref() {
            Some("denied") => false,
            Some("owned") => true,
            Some("ledger") => super::owned_ledger::contains(content_id, subject),
            _ => true,
        };
        let guard = ChainRpcMock::start(owned)?;
        (
            MOCK_NETWORK.to_string(),
            MOCK_CONTRACT.to_string(),
            MOCK_SELECTOR.to_string(),
            MOCK_RIGHTS_ABI.to_string(),
            guard.url.clone(),
            Some(guard),
        )
    } else {
        // Real Base ABI by default; all three pinnable. Only the RPC URL has no sane
        // default (it's deployment-specific), so it stays required.
        let network =
            env_nonempty("ELASTOS_DDRM_RIGHTS_NETWORK").unwrap_or_else(|| "base".to_string());
        let contract = env_nonempty("ELASTOS_DDRM_RIGHTS_CONTRACT")
            .unwrap_or_else(|| BASE_AUTHORITY_GATEWAY.to_string());
        let selector = env_nonempty("ELASTOS_DDRM_RIGHTS_SELECTOR")
            .unwrap_or_else(|| BASE_HAS_ACCESS_SELECTOR.to_string());
        let rpc_url = env_nonempty("ELASTOS_CHAIN_BASE_RPC")
            .ok_or("ELASTOS_CHAIN_BASE_RPC (Base RPC URL) is required for chain mode")?;
        (
            network,
            contract,
            selector,
            LIVE_RIGHTS_ABI.to_string(),
            rpc_url,
            None,
        )
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
                "abi": abi,
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

/// The marker every chain-conversation deadline error carries (Sprint 40). MONEY-CLASSIFICATION
/// NOTE: this string must NEVER contain a pre-broadcast refusal sentinel
/// (`buy_authority::ERR_*`) — the DRM classifier refunds on those, and a deadline on the SEND leg
/// means the tx MAY have broadcast: it must classify INDETERMINATE (hold), never refund. The
/// conservative default in `ChainDrmMarketplace::settle` (non-sentinel ⇒ Indeterminate) gives
/// exactly that; a ratchet pins the non-collision.
pub(crate) const CHAIN_DEADLINE_MARKER: &str = "chain-provider read deadline exceeded";

/// Spawn chain-provider, send `init` then one op (e.g. `has_access_by_content_id` or
/// `broadcast_transaction`) + `shutdown`, returning the op's `data`.
///
/// DEADLINE (Sprint 40/41): the WHOLE conversation is bounded by the shared
/// [`capsule_watchdog`](super::capsule_watchdog) — a
/// watchdog kills the child at expiry, so a hung RPC/subprocess can never park a blocking
/// thread forever (the pay pipeline's, the quote spine's, or the confirmation scheduler's).
/// The child is ALWAYS reaped (no zombies) and the reap itself is BOUNDED (council S41 guardian
/// F1/F2): on the normal path it exits on `shutdown`/EOF; on the deadline path the read-watchdog
/// kill makes the read return; and a child that answered but then refuses to exit is group-killed
/// by [`reap_grouped`](super::capsule_watchdog::reap_grouped) after a short grace — no reap ever
/// parks the thread. A deadline error carries
/// [`CHAIN_DEADLINE_MARKER`] so consumers classify it correctly (see the marker's money note).
/// Unix-only kill (like the flock protections); elsewhere the watchdog is a no-op and the old
/// unbounded behavior remains, stated here.
pub(crate) fn run_chain_capsule(bin: &str, init: &Value, query: &Value) -> Result<Value, String> {
    let deadline = super::capsule_watchdog::capsule_read_deadline();
    let mut cmd = Command::new(bin);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut child = super::capsule_watchdog::spawn_grouped(&mut cmd)
        .map_err(|e| format!("spawn chain-provider ({bin}): {e}"))?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let mut reader = BufReader::new(child.stdout.take().ok_or("no stdout")?);
    let watchdog = super::capsule_watchdog::DeadlineWatchdog::arm(child.id(), deadline);

    let result = (|| -> Result<Value, String> {
        writeln!(stdin, "{init}").map_err(|e| format!("write chain init: {e}"))?;
        stdin.flush().map_err(|e| format!("flush: {e}"))?;
        let init_resp = read_capsule_line(&mut reader)?;
        if init_resp.get("status").and_then(Value::as_str) != Some("ok") {
            let _ = writeln!(stdin, "{}", json!({ "op": "shutdown" }));
            return Err(format!(
                "chain-provider init failed: {}",
                init_resp
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            ));
        }

        writeln!(stdin, "{query}").map_err(|e| format!("write chain query: {e}"))?;
        stdin.flush().map_err(|e| format!("flush: {e}"))?;
        let query_resp = read_capsule_line(&mut reader)?;

        let _ = writeln!(stdin, "{}", json!({ "op": "shutdown" }));
        let _ = stdin.flush();

        if query_resp.get("status").and_then(Value::as_str) != Some("ok") {
            return Err(format!(
                "chain-provider op failed: {}",
                query_resp
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            ));
        }
        query_resp
            .get("data")
            .cloned()
            .ok_or_else(|| "chain-provider ok response missing data".to_string())
    })();
    // Drop stdin (EOF) so a well-behaved child exits even on the error paths. Then DISARM and
    // JOIN the watchdog BEFORE reaping (council S40 red-team F1 / guardian F3): the watchdog can
    // only ever SIGKILL a child we have NOT yet reaped, so a reaped-then-recycled pid can never
    // receive a stray group kill. A hung conversation is already resolved by the time we reach
    // here — the watchdog killed the (still-live, un-reaped) child, EOF ended the read, the
    // closure returned Err; a fast/normal conversation disarms the watchdog before it fires.
    // Drop stdin (EOF), disarm+join the watchdog, THEN reap — the disarm-before-reap ordering
    // (S40 red-team F1) lives in `DeadlineWatchdog::disarm`.
    drop(stdin);
    let fired = watchdog.disarm();
    // Reap is BOUNDED (S41): an answered-then-lingering child is group-killed after a short grace
    // rather than parking the thread on an unbounded `wait()`.
    super::capsule_watchdog::reap_grouped(&mut child);

    if result.is_err() && fired {
        let underlying = result.err().unwrap_or_default();
        return Err(format!(
            "{CHAIN_DEADLINE_MARKER}: no response within {}s — chain-provider killed; the op's \
             outcome is UNRESOLVED (a send may have gone out); underlying: {underlying}",
            deadline.as_secs()
        ));
    }
    result
}

/// The marker a rights-provider DECIDE-leg deadline carries (Sprint 41). MONEY/ACCESS NOTE: a
/// rights-decide timeout is an Err, and every access consumer DENIES on Err (a 503 "rights gate
/// unavailable", never an open) — so, unlike the chain send leg, this needs no special
/// classification: deny is the fail-closed direction. The marker is for diagnostics.
pub(crate) const RIGHTS_DEADLINE_MARKER: &str = "rights-provider read deadline exceeded";

/// Spawn the rights-provider capsule, send one request + `shutdown`, return its `data`.
///
/// DEADLINE (Sprint 41): the conversation is bounded by the shared
/// [`capsule_watchdog`](super::capsule_watchdog) — a hung rights provider is killed (process
/// group and all) so the DECIDE leg can never park a request thread forever. A deadline Errs,
/// which every access consumer treats as DENY (fail-closed).
fn run_rights_capsule(bin: &str, request: &Value) -> Result<Value, String> {
    let deadline = super::capsule_watchdog::capsule_read_deadline();
    let mut cmd = Command::new(bin);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut child = super::capsule_watchdog::spawn_grouped(&mut cmd)
        .map_err(|e| format!("spawn rights-provider ({bin}): {e}"))?;
    let watchdog = super::capsule_watchdog::DeadlineWatchdog::arm(child.id(), deadline);

    let result = (|| -> Result<Value, String> {
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        writeln!(stdin, "{request}").map_err(|e| format!("write rights request: {e}"))?;
        writeln!(stdin, "{}", json!({ "op": "shutdown" }))
            .map_err(|e| format!("write shutdown: {e}"))?;
        stdin.flush().map_err(|e| format!("flush: {e}"))?;
        drop(stdin); // EOF so a well-behaved child exits
        let mut reader = BufReader::new(child.stdout.take().ok_or("no stdout")?);
        let resp = read_capsule_line(&mut reader)?;
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
    })();
    // Disarm+join BEFORE reap (S40 ordering — lives in DeadlineWatchdog::disarm), then a BOUNDED
    // reap (S41): an answered-then-lingering rights child is group-killed after a short grace, not
    // parked on an unbounded `wait()`.
    let fired = watchdog.disarm();
    super::capsule_watchdog::reap_grouped(&mut child);

    if result.is_err() && fired {
        let underlying = result.err().unwrap_or_default();
        return Err(format!(
            "{RIGHTS_DEADLINE_MARKER}: no decision within {}s — rights-provider killed; access \
             is DENIED (fail-closed); underlying: {underlying}",
            deadline.as_secs()
        ));
    }
    result
}

use super::capsule_watchdog::read_capsule_line;

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
pub(crate) struct ChainRpcMock {
    pub(crate) url: String,
    shutdown: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl ChainRpcMock {
    fn start(owned: bool) -> Result<Self, String> {
        Self::start_with_word(format!("0x{:064x}", u8::from(owned)))
    }

    /// Stand up a loopback JSON-RPC endpoint that answers EVERY request with the same
    /// 32-byte hex `result` word. Used by the rights gate (a `0…01`/`0…00` ABI bool) and
    /// by the buy broadcast path (a canned `eth_sendRawTransaction` tx hash) — both are
    /// 32-byte hex, exactly what the REAL chain-provider decoders expect.
    pub(crate) fn start_with_word(result_word: String) -> Result<Self, String> {
        let listener =
            TcpListener::bind("127.0.0.1:0").map_err(|e| format!("bind chain RPC mock: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let stop = shutdown.clone();
        let bool_word = result_word;
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
        let _g = crate::api::ddrm_env_lock();
        std::env::set_var("ELASTOS_DDRM_RIGHTS", "chain-mock");

        std::env::remove_var("ELASTOS_DDRM_CHAIN_ACCESS");
        let owned = decide_owned_access(
            "did:test:alice",
            "s1",
            "bafyowned",
            SUBJECT,
            "view",
            "render",
            None,
            1_700_000_000,
            900,
        )
        .expect("owned decision");
        assert!(owned.allowed, "owned content must be allowed");
        assert!(
            owned.source.contains("mock"),
            "source should be the in-process mock"
        );

        std::env::set_var("ELASTOS_DDRM_CHAIN_ACCESS", "denied");
        let denied = decide_owned_access(
            "did:test:alice",
            "s1",
            "bafynotowned",
            SUBJECT,
            "view",
            "render",
            None,
            1_700_000_000,
            900,
        )
        .expect("denied decision");
        assert!(
            !denied.allowed,
            "not-owned content must be denied (fail closed)"
        );

        std::env::remove_var("ELASTOS_DDRM_RIGHTS");
        std::env::remove_var("ELASTOS_DDRM_CHAIN_ACCESS");
    }

    /// PC2's `normalizedKid` rule: a bare 32-hex KID gets a `0x` prefix (so the strict
    /// chain-provider bytes16 encoder accepts it); already-prefixed or non-hex ids pass
    /// through unchanged.
    #[test]
    fn normalize_kid_matches_pc2_normalized_kid() {
        // Bare 32-hex KID -> 0x-prefixed (the capsule stores it bare).
        assert_eq!(
            normalize_kid_0x("38691296765e76a331f5d5630bddf9f5"),
            "0x38691296765e76a331f5d5630bddf9f5"
        );
        // Already prefixed -> unchanged.
        assert_eq!(
            normalize_kid_0x("0x38691296765e76a331f5d5630bddf9f5"),
            "0x38691296765e76a331f5d5630bddf9f5"
        );
        // A non-hex CID (chain-mock string shape) must NOT be 0x-prefixed.
        assert_eq!(
            normalize_kid_0x("bafybeigprotectedcontent"),
            "bafybeigprotectedcontent"
        );
        // Wrong length hex is left alone (so it fails loudly at the encoder, not silently).
        assert_eq!(normalize_kid_0x("deadbeef"), "deadbeef");
    }

    /// Chain mode with no wallet subject and no override must fail closed (not open).
    #[test]
    fn chain_mode_without_wallet_fails_closed() {
        let _g = crate::api::ddrm_env_lock();
        std::env::set_var("ELASTOS_DDRM_RIGHTS", "chain");
        let result = decide_owned_access(
            "did:test:nowallet",
            "s1",
            "bafyx",
            "",
            "view",
            "render",
            None,
            1_700_000_000,
            900,
        );
        std::env::remove_var("ELASTOS_DDRM_RIGHTS");
        let err = result.expect_err("chain mode with no wallet must error");
        assert!(err.contains("wallet not linked"), "unexpected error: {err}");
    }

    // ── DEV_MODE_GUARD_SPEC: secure-by-construction build posture ──────────────────────

    /// RELEASE posture (no `dev-modes`): `rights_mode()` is `Chain` even when handed a dev
    /// value, and the startup guard REFUSES to boot in a dev rights mode (fail closed) while
    /// accepting `chain`. This is the audit's HIGH→closed assertion for the gateway.
    #[test]
    #[cfg(not(feature = "dev-modes"))]
    fn release_build_defaults_to_chain_and_refuses_dev_rights_modes() {
        let _g = crate::api::ddrm_env_lock();

        std::env::remove_var("ELASTOS_DDRM_RIGHTS");
        assert_eq!(
            rights_mode(),
            RightsMode::Chain,
            "unset must default to Chain"
        );

        for dev in ["dev", "chain-mock"] {
            std::env::set_var("ELASTOS_DDRM_RIGHTS", dev);
            // A release build cannot leave the secure path...
            assert_eq!(
                rights_mode(),
                RightsMode::Chain,
                "{dev} must NOT downgrade rights_mode"
            );
            // ...and refuses to start rather than silently upgrading the misconfig.
            let err = enforce_release_build_rights_safety()
                .expect_err("release build must refuse to start in a dev rights mode");
            assert!(
                err.contains("Refusing to start"),
                "unexpected guard error: {err}"
            );
        }

        std::env::set_var("ELASTOS_DDRM_RIGHTS", "chain");
        assert!(
            enforce_release_build_rights_safety().is_ok(),
            "chain mode must boot in a release build"
        );
        std::env::remove_var("ELASTOS_DDRM_RIGHTS");
    }

    /// DEV posture (`--features dev-modes`): the dev modes are intentionally selectable and the
    /// guard is a no-op, so local/CI keeps working.
    #[test]
    #[cfg(feature = "dev-modes")]
    fn dev_build_allows_dev_rights_modes() {
        let _g = crate::api::ddrm_env_lock();
        std::env::set_var("ELASTOS_DDRM_RIGHTS", "dev");
        assert_eq!(rights_mode(), RightsMode::Dev);
        assert!(
            enforce_release_build_rights_safety().is_ok(),
            "dev-modes build never fails the guard"
        );
        std::env::set_var("ELASTOS_DDRM_RIGHTS", "chain-mock");
        assert_eq!(rights_mode(), RightsMode::ChainMock);
        std::env::remove_var("ELASTOS_DDRM_RIGHTS");
    }

    /// Sprint 40 ratchet: a HUNG chain-provider is killed at the deadline — the call returns
    /// bounded (never parks a thread for the child's lifetime), the child is reaped, and the
    /// error carries the classification marker + says the outcome is UNRESOLVED (a send may
    /// have gone out — the money classifiers treat it as indeterminate, never a refund).
    #[test]
    #[cfg(unix)]
    fn a_hung_chain_provider_is_killed_at_the_deadline() {
        let _g = crate::api::ddrm_env_lock();
        let prior = std::env::var("ELASTOS_CHAIN_READ_DEADLINE_SECS").ok();
        std::env::set_var("ELASTOS_CHAIN_READ_DEADLINE_SECS", "1");

        let dir = tempfile::tempdir().unwrap();
        let stub = dir.path().join("hung-chain-provider.sh");
        std::fs::write(&stub, "#!/bin/sh\nsleep 300\n").unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

        let started = std::time::Instant::now();
        let err = run_chain_capsule(
            stub.to_str().unwrap(),
            &serde_json::json!({ "op": "init" }),
            &serde_json::json!({ "op": "receipt" }),
        )
        .unwrap_err();
        match prior {
            Some(v) => std::env::set_var("ELASTOS_CHAIN_READ_DEADLINE_SECS", v),
            None => std::env::remove_var("ELASTOS_CHAIN_READ_DEADLINE_SECS"),
        }

        assert!(
            started.elapsed() < std::time::Duration::from_secs(15),
            "the call is BOUNDED by the deadline, not by the child's 300s sleep"
        );
        assert!(
            err.contains(CHAIN_DEADLINE_MARKER),
            "the error carries the classification marker: {err}"
        );
        assert!(
            err.contains("UNRESOLVED"),
            "the error says the outcome is unresolved (indeterminate direction): {err}"
        );
    }

    /// Sprint 41 ratchet: a HUNG rights-provider (the DECIDE leg) is killed at the deadline too —
    /// the call returns bounded, the child is reaped, and the error carries the rights marker +
    /// says access is DENIED (the fail-closed direction; every access consumer denies on Err).
    #[test]
    #[cfg(unix)]
    fn a_hung_rights_provider_is_killed_and_access_is_denied() {
        let _g = crate::api::ddrm_env_lock();
        let prior = std::env::var("ELASTOS_CHAIN_READ_DEADLINE_SECS").ok();
        std::env::set_var("ELASTOS_CHAIN_READ_DEADLINE_SECS", "1");

        let dir = tempfile::tempdir().unwrap();
        let stub = dir.path().join("hung-rights-provider.sh");
        std::fs::write(&stub, "#!/bin/sh\nsleep 300\n").unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

        let started = std::time::Instant::now();
        let err = run_rights_capsule(
            stub.to_str().unwrap(),
            &serde_json::json!({ "op": "decide" }),
        )
        .unwrap_err();
        match prior {
            Some(v) => std::env::set_var("ELASTOS_CHAIN_READ_DEADLINE_SECS", v),
            None => std::env::remove_var("ELASTOS_CHAIN_READ_DEADLINE_SECS"),
        }

        assert!(
            started.elapsed() < std::time::Duration::from_secs(15),
            "the DECIDE leg is BOUNDED by the deadline, not the child's 300s sleep"
        );
        assert!(
            err.contains(RIGHTS_DEADLINE_MARKER),
            "the error carries the rights marker: {err}"
        );
        assert!(
            err.contains("DENIED"),
            "a rights-decide timeout DENIES access (fail-closed): {err}"
        );
    }

    // The malformed-env fallback test moved to `capsule_watchdog` (the shared home of the
    // deadline parser); this module keeps the live hung-provider kill ratchets above.
}
