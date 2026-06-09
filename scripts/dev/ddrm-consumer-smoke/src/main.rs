//! dDRM consumer-half orchestration smoke (Phase A.4).
//!
//! Drives the REAL capsule binaries over their newline-delimited JSON stdin/stdout
//! protocol to prove the consumer half of the Elacity dDRM chain runs end to end:
//!
//!   drm/open  ->  rights  ->  key (reference authority)  ->  decrypt (OpenSessionV1)
//!
//! with NO Lit, NO dKMS and NO chain. The novel, previously-unproven step is the
//! cross-process `key -> decrypt` handoff:
//!
//!   1. the key authority publishes its ML-DSA-65 verifying key (`key init`);
//!   2. the decrypt boundary is configured to trust it, then MINTS an in-sandbox
//!      session keypair and PUBLISHES the public key (`decrypt init`, never the secret);
//!   3. the content CEK is ESCROWED to the authority's published recipient key, and the
//!      authority's CANONICAL `release` op RECOVERS it in-boundary (from the rights-bound
//!      key_envelope) and re-seals it to the published session key, bound to the canonical
//!      decrypt transcript — the SAME `to_aad` both sides share. No raw CEK is handed in;
//!   4. the decrypt boundary unwraps with its in-VM secret and decrypts a real CENC
//!      segment (`decrypt open_session_v1`), returning ONLY a scoped session — no CEK,
//!      no plaintext crosses the process boundary.
//!
//! The content `(CEK, ciphertext, plaintext)` is a committed golden
//! (`capsules/decrypt-provider/tests/vectors/classical_cenc.json`); the CEK reaches the
//! authority SEALED (escrowed, recovered in-boundary) via the canonical `release` op — no
//! dev raw-CEK shim — and the boundary recovers it and decrypts, proving the rail end to
//! end through the op drm-provider's plan actually names.
//!
//! The chain is no longer hand-walked here, and the smoke no longer drives the open
//! itself: it builds the trusted runtime-core HOST `ddrm_plan_runner::DrmHost` and calls
//! `host.open(content_id, viewer)`. The HOST owns the whole open — it (1) asks its
//! `PlanSource` (a `SmokePlanSource` wrapping the REAL `drm-provider`) for the canonical
//! plan, (2) drives it through the runtime `RuntimeCapabilityTable` of registered
//! transports (`open_drm_plan`'s parse → resolve each required provider → execute), and
//! (3) emits the plan's runtime-OWNED post-steps (`release_receipt` + the open audit)
//! through a `RuntimeEventSink` (a `SmokeEventSink`). This is the runtime-core analogue of
//! PC2's server-owned `/init` route, which — once the capability is resolved — owns
//! fetching the MPD, driving recovery, creating the session, and logging, all in one
//! place (`pc2-node/src/api/media.ts:133` route → `:481`/`:482` recover → `:489` session).
//! The smoke REGISTERS three runtime-owned transports (`RightsTransport`/`KeyTransport`/
//! `DecryptTransport`, each OWNING one real capsule binary) into the host's table — the
//! same registry type the trusted core uses; each transport opens a fresh per-provider
//! handle on demand and TEARS DOWN its capsule on `shutdown`. The HOST owns the transports,
//! so `host.shutdown()` tears down the whole rail (the analogue of disposing every per-view
//! handle) — the smoke performs no manual per-capsule shutdown. The event sink is the lib's
//! `PersistingEventSink` over a `FileEventStore`: each runtime-event step is written as a
//! durable, CEK-FREE record (open identity + decision + artifact NAMES, never key material)
//! to a temp dir, which the smoke reads back to prove the receipt + audit persisted without
//! leaking the CEK. No second code path. The host fails closed unless every provider the
//! plan's `next_required_providers` names has a registered transport, and unless every
//! runtime event it declares can be PERSISTED. Two more fail-closed gates ride along: a
//! transcript-mismatched seal must not open, and a TAMPERED plan FROM THE SOURCE (driven
//! back through the SAME host) must be rejected by the real key-provider.
//!
//! Usage: ddrm-consumer-smoke <key-provider-bin> <decrypt-provider-bin> <drm-bin> [rights-bin]

use base64::Engine as _;
use ddrm_envelope::transcript::{escrow_aad, release_receipt_hash, DecryptTranscriptV1};
use ddrm_envelope::SUITE_PQ_HYBRID;
use ddrm_plan_runner::{
    DrmHost, EventStore, PersistingEventSink, PlanSource, ProviderHandle, ProviderTransport,
    RuntimeCapabilityTable, StepInputs,
};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::rc::Rc;

const B64: base64::engine::general_purpose::GeneralPurpose = base64::engine::general_purpose::STANDARD;

// --- the shared identity used by BOTH the decrypt request and the sealed transcript -
const PRINCIPAL: &str = "person:local:smoke";
const SESSION: &str = "session:smoke";
const OBJECT_CID: &str = "bafybeigprotectedcontent";
const ACTION: &str = "view";
const VIEWER: &str = "elastos.viewer/document@1";
const OUTPUT_KIND: &str = "rendered";
const EXPIRES_AT: u64 = 1_900_000_000;
const NOW_UNIX: u64 = 1_850_000_000;

// Release receipt identity (hashed into the transcript by both sides).
const RR_SCHEMA: &str = "elastos.release.receipt/v1";
const RR_REQUEST_ID: &str = "key-release:smoke";
const RR_PROVIDER: &str = "key-provider";
const RR_STATUS: &str = "released";
const RR_ISSUED_AT: u64 = 1_800_000_000;

// Committed golden (classical_cenc.json): a 16-byte CEK + an AES-128-CTR CENC segment
// whose plaintext is "the quick brown fox jumps over!!" — a single protected sample.
const GOLDEN_CEK_B64: &str = "EREREREREREREREREREREQ==";
const GOLDEN_CIPHERTEXT_B64: &str = "AAAAPG1vb2YAAAA0dHJhZgAAABR0cnVuAAACAAAAAAEAAAAgAAAAGHNlbmMAAAAAAAAAASIiIiIiIiIiAAAAKG1kYXScNDPiT64BF0MfL13dprDn+6eX7LyGcmlu1lMPPiQQpA==";
const EXPECTED_SAMPLE_COUNT: u64 = 1;

/// The content identity carried across the WHOLE chain (the chain ownership query,
/// the rights binding, and the decrypt transcript). Defaults to the golden's CID for
/// the offline smoke; a live run overrides it with the on-chain contentId/KID the
/// AuthorityGateway actually answers for (`DDRM_SMOKE_CONTENT_ID`).
fn cid() -> String {
    std::env::var("DDRM_SMOKE_CONTENT_ID")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| OBJECT_CID.to_string())
}

/// A live capsule process driven over its stdin/stdout JSON line protocol.
struct Capsule {
    name: String,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Capsule {
    fn spawn(name: &str, bin: &str) -> Result<Self, String> {
        let mut child = Command::new(bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("spawn {name} ({bin}): {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("no stdout")?);
        Ok(Self { name: name.to_string(), child, stdin, stdout })
    }

    /// Send one request line, read one response line, parse it. The capsule emits
    /// exactly one JSON object per input line.
    fn call(&mut self, req: &Value) -> Result<Value, String> {
        let line = serde_json::to_string(req).map_err(|e| e.to_string())?;
        writeln!(self.stdin, "{line}").map_err(|e| format!("write to {}: {e}", self.name))?;
        self.stdin.flush().map_err(|e| e.to_string())?;
        let mut resp = String::new();
        let n = self
            .stdout
            .read_line(&mut resp)
            .map_err(|e| format!("read from {}: {e}", self.name))?;
        if n == 0 {
            return Err(format!("{} closed its output unexpectedly", self.name));
        }
        serde_json::from_str(resp.trim()).map_err(|e| format!("{} sent non-JSON: {e}: {resp}", self.name))
    }

    fn shutdown(&mut self) {
        let _ = self.call(&json!({ "op": "shutdown" }));
        let _ = self.child.wait();
    }
}

fn ok_data(resp: &Value, ctx: &str) -> Result<Value, String> {
    if resp.get("status").and_then(Value::as_str) == Some("ok") {
        Ok(resp.get("data").cloned().unwrap_or(Value::Null))
    } else {
        Err(format!("{ctx}: expected ok, got {resp}"))
    }
}

/// The key step's release receipt, in the shape the decrypt boundary consumes.
fn release_receipt_json() -> Value {
    json!({
        "schema": RR_SCHEMA,
        "request_id": RR_REQUEST_ID,
        "object_cid": cid(),
        "principal_id": PRINCIPAL,
        "session_id": SESSION,
        "action": ACTION,
        "provider": RR_PROVIDER,
        "status": RR_STATUS,
        "issued_at": RR_ISSUED_AT,
        "expires_at": EXPIRES_AT,
    })
}

/// Build the decrypt-session request WITHOUT the plan-threaded fields. The runtime
/// executor threads `release_receipt`, `object_cid` and `viewer_interface` in from the
/// plan's binding edges (see [`SmokeRunner::run_step`]) — so a wrong edge places an
/// artifact under an unknown field (or omits it) and the real decrypt-provider fails
/// closed, rather than this literal hard-coding where each value lands.
fn decrypt_request_base() -> Value {
    json!({
        "schema": "elastos.decrypt.session.request/v1",
        "request_id": "decrypt:consumer-smoke",
        "principal_id": PRINCIPAL,
        "session_id": SESSION,
        "action": ACTION,
        "output_kind": OUTPUT_KIND,
        "reason": "open protected document",
        "expires_at": EXPIRES_AT,
    })
}

fn rights_access_request() -> Value {
    json!({
        "principal_id": PRINCIPAL,
        "session_id": SESSION,
        "content_id": cid(),
        "right": ACTION,
        "reason": "open protected document",
    })
}

/// A mocked on-chain ownership answer (stands in for `chain-provider::has_access_by_
/// content_id`; used on the offline path). `has_access: true` => owned.
fn owned_chain_attestation() -> Value {
    json!({
        "network": "base",
        "contract": "0x00000000000000000000000000000000000000aa",
        "content_id": cid(),
        "subject": "0x00000000000000000000000000000000000000bb",
        "right": ACTION,
        "has_access": true,
    })
}

/// A hardcoded fallback receipt (used only when no rights binary is supplied).
fn fallback_rights_receipt() -> Value {
    json!({
        "schema": "elastos.rights.decision.receipt/v1",
        "request_id": "rights:smoke",
        "content_id": cid(),
        "principal_id": PRINCIPAL,
        "session_id": SESSION,
        "right": ACTION,
        "provider": "rights-provider",
        "allowed": true,
        "issued_at": RR_ISSUED_AT,
        "expires_at": EXPIRES_AT,
    })
}

/// Build the key-release request WITHOUT the plan-threaded rights receipt. The
/// runtime executor threads the `RightsDecisionReceiptV1` in under the field the
/// plan's `rights_check -> key_release` edge declares (default `rights_receipt`); a
/// wrong edge omits it and the real key-provider fails closed.
fn key_release_request_base(kid_hex: &str, wrapped_cek_b64: &str) -> Value {
    json!({
        "schema": "elastos.key_release.request/v1",
        "request_id": RR_REQUEST_ID,
        "principal_id": PRINCIPAL,
        "session_id": SESSION,
        "object_cid": cid(),
        "action": ACTION,
        "key_envelope": {
            "scheme": "elastos-pq-hybrid-threshold-v0",
            // The bytes16 KID the escrow is bound to, and the producer's escrow blob —
            // the wrapped CEK rides INSIDE the rights-bound request (canonical `release`).
            "kid": kid_hex,
            "wrapped_cek": wrapped_cek_b64,
            "policy_hash": "sha256:smoke",
            "algorithms": {
                "cipher": "aes-256-gcm",
                "signature": ["ed25519", "ml-dsa-65"],
                "kem": ["x25519", "ml-kem-768"],
                "share_scheme": "shamir-t-of-n",
            },
        },
        "reason": "open protected document",
        "expires_at": EXPIRES_AT,
    })
}

/// The `drm/open` request: a sealed object whose KID is the content identity the
/// chain keys on. The drm-provider validates it and returns the canonical open plan.
fn drm_open_request() -> Value {
    json!({
        "op": "open",
        "request": {
            "object": {
                "schema": "elastos.sealed.object/v1",
                "payload_cid": "bafybeigpayload",
                "rights_policy_cid": "bafybeigpolicy",
                "availability_receipt_cid": "bafybeigreceipt",
                "key_envelope": {
                    "scheme": "elastos-pq-hybrid-threshold-v0",
                    "kid": cid(),
                    "wrapped_cek": "wrapped",
                    "policy_hash": "sha256:smoke",
                    "algorithms": {
                        "cipher": "aes-256-gcm",
                        "signature": ["ed25519", "ml-dsa-65"],
                        "kem": ["x25519", "ml-kem-768"],
                        "share_scheme": "shamir-t-of-n",
                    },
                },
                "viewer": { "required_interface": VIEWER },
            },
            "principal_id": PRINCIPAL,
            "session_id": SESSION,
            "action": ACTION,
            "reason": "open protected document",
        }
    })
}

/// Rebuild the canonical decrypt-transcript AAD exactly as the decrypt boundary will,
/// using the SHARED `ddrm-envelope` encoder (no parallel definition).
fn transcript_aad(session_pub: &[u8], content_hash: &[u8], nonce: &[u8]) -> Vec<u8> {
    let c = cid();
    let receipt_hash = release_receipt_hash(
        RR_SCHEMA,
        RR_REQUEST_ID,
        &c,
        PRINCIPAL,
        SESSION,
        ACTION,
        RR_PROVIDER,
        RR_STATUS,
        RR_ISSUED_AT,
        EXPIRES_AT,
    );
    DecryptTranscriptV1 {
        suite_id: SUITE_PQ_HYBRID,
        provider_id: "decrypt-provider",
        principal_id: PRINCIPAL,
        session_id: SESSION,
        object_cid: &c,
        content_hash,
        action: ACTION,
        viewer_interface: VIEWER,
        output_kind: OUTPUT_KIND,
        expires_at: EXPIRES_AT,
        release_receipt_hash: receipt_hash,
        decrypt_session_pub: session_pub,
        nonce,
    }
    .to_aad()
}

fn step(n: u32, msg: &str) {
    println!("  [{n}] {msg}");
}

/// Resolve the on-chain ownership answer. Live (`DDRM_SMOKE_CHAIN_RPC` set + a
/// chain-provider binary supplied) drives the REAL `chain-provider` against Base;
/// otherwise a mocked-owned attestation keeps the offline smoke deterministic.
/// Returns `(attestation, mode_label)`.
fn chain_attestation(chain_bin: Option<&String>) -> Result<(Value, String), String> {
    let rpc = std::env::var("DDRM_SMOKE_CHAIN_RPC").ok().filter(|s| !s.is_empty());
    match (rpc, chain_bin) {
        (Some(rpc_url), Some(bin)) => Ok((live_chain_attestation(bin, &rpc_url)?, "live chain".to_string())),
        (Some(_), None) => Err("DDRM_SMOKE_CHAIN_RPC is set but no chain-provider binary was supplied".to_string()),
        _ => Ok((owned_chain_attestation(), "mocked owned".to_string())),
    }
}

/// Drive the real `chain-provider` for a live `has_access_by_content_id` query, and
/// return its response shaped as the rights attestation (1:1 by field name). All the
/// network/contract/subject inputs come from the environment so nothing is hard-coded.
fn live_chain_attestation(bin: &str, rpc_url: &str) -> Result<Value, String> {
    let env = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
    let network = env("DDRM_SMOKE_CHAIN_NETWORK").unwrap_or_else(|| "base".to_string());
    let contract = env("DDRM_SMOKE_CHAIN_CONTRACT")
        .ok_or("DDRM_SMOKE_CHAIN_CONTRACT (AuthorityGateway address) is required for a live chain check")?;
    let selector = env("DDRM_SMOKE_CHAIN_SELECTOR")
        .ok_or("DDRM_SMOKE_CHAIN_SELECTOR (has_access selector, e.g. 0x........) is required for a live chain check")?;
    let subject = env("DDRM_SMOKE_CHAIN_SUBJECT")
        .ok_or("DDRM_SMOKE_CHAIN_SUBJECT (your wallet address) is required for a live chain check")?;
    let chain_id: i64 = env("DDRM_SMOKE_CHAIN_ID").and_then(|s| s.parse().ok()).unwrap_or(8453);

    let mut chain = Capsule::spawn("chain-provider", bin)?;
    ok_data(
        &chain.call(&json!({
            "op": "init",
            "config": { "networks": [{
                "id": network,
                "display_name": network,
                "kind": "evm_json_rpc",
                "chain_id": chain_id,
                "native_symbol": "ETH",
                "provider": "ddrm-consumer-smoke",
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
        }))?,
        "chain init",
    )?;
    let resp = ok_data(
        &chain.call(&json!({
            "op": "has_access_by_content_id",
            "network": network,
            "contract": contract,
            "content_id": cid(),
            "subject": subject,
            "right": ACTION,
        }))?,
        "chain has_access_by_content_id",
    )?;
    chain.shutdown();

    // chain-provider's response IS the attestation shape rights-provider consumes.
    Ok(json!({
        "network": resp["network"],
        "contract": resp["contract"],
        "content_id": resp["content_id"],
        "subject": resp["subject"],
        "right": resp["right"],
        "has_access": resp["has_access"],
    }))
}

/// Thread the plan's binding inputs into a base request: each artifact lands under the
/// field name the PLAN's edge declared. A wrong edge therefore places it under an
/// unknown field (or omits it) and the real provider — `deny_unknown_fields` over a
/// required field — fails closed.
fn thread_into(base: &mut Value, inputs: &StepInputs) {
    if let Some(obj) = base.as_object_mut() {
        for (field, artifact) in inputs.threaded_fields() {
            obj.insert(field.clone(), artifact.clone());
        }
    }
}

// The consumer half is now driven by the runtime-core `RuntimeStepRunner` over three
// INJECTED per-provider capability handles — the runtime-core analogue of PC2's
// per-request `BackendSessionView` (resurrected in middleware, threaded into the
// downstream stage). Each handle wraps one real capsule binary (the runtime would
// inject real provider handles instead); the core routes each plan step to the handle
// for that step's provider, and fails closed if a required handle is missing. The
// capsules are shared (`Rc<RefCell<_>>`) so the post-walk fail-closed checks + shutdown
// can still reach them after the runner has borrowed the handles.

/// Injected `rights` capability: the REAL rights-provider rendering the (live or
/// mocked) on-chain ownership answer into a typed `RightsDecisionReceiptV1` (falls back
/// to a hardcoded receipt only when no rights binary was supplied).
struct RightsHandle {
    rights: Rc<RefCell<Option<Capsule>>>,
    chain_attestation: Value,
    chain_mode: String,
}

impl ProviderHandle for RightsHandle {
    fn provider(&self) -> &str {
        "rights"
    }
    fn run(&mut self, _inputs: &StepInputs) -> Result<BTreeMap<String, Value>, String> {
        let mut guard = self.rights.borrow_mut();
        let receipt = if let Some(rights) = guard.as_mut() {
            ok_data(&rights.call(&json!({ "op": "status" }))?, "rights status")?;
            let decision = ok_data(
                &rights.call(&json!({
                    "op": "decide_access_from_chain",
                    "request_id": RR_REQUEST_ID,
                    "request": rights_access_request(),
                    "chain_access": self.chain_attestation.clone(),
                    "now_unix": RR_ISSUED_AT,
                    "ttl_secs": EXPIRES_AT - RR_ISSUED_AT,
                }))?,
                "rights decide_access_from_chain",
            )?;
            if decision["decision"].as_str() != Some("allowed") {
                return Err(format!(
                    "rights did not allow this content ({}); the chain says you do not own it: {decision}",
                    self.chain_mode
                ));
            }
            decision["receipt"].clone()
        } else {
            fallback_rights_receipt()
        };
        Ok(BTreeMap::from([(
            "RightsDecisionReceiptV1".to_string(),
            receipt,
        )]))
    }
}

/// Injected `key` capability: the canonical `release` op (the one drm-provider's plan
/// names). The rights receipt is threaded in by the executor; the authority RECOVERS
/// the escrowed CEK from the rights-bound `key_envelope` and re-seals it to the
/// published session key — no raw CEK ever handed in. Produces the release receipt
/// (threaded onward into decrypt) and the sealed material (carried in the context).
struct KeyHandle {
    key: Rc<RefCell<Capsule>>,
    kid_hex: String,
    wrapped_cek_b64: String,
    producer_vk_b64: String,
    session_pub_b64: String,
    aad_b64: String,
    content_hash_b64: String,
    nonce_b64: String,
}

impl ProviderHandle for KeyHandle {
    fn provider(&self) -> &str {
        "key"
    }
    fn run(&mut self, inputs: &StepInputs) -> Result<BTreeMap<String, Value>, String> {
        let mut request = key_release_request_base(&self.kid_hex, &self.wrapped_cek_b64);
        thread_into(&mut request, inputs);
        let release = ok_data(
            &self.key.borrow_mut().call(&json!({
                "op": "release",
                "request": request,
                "session": {
                    "decrypt_session_pub_b64": self.session_pub_b64,
                    "producer_vk_b64": self.producer_vk_b64,
                    "aad_b64": self.aad_b64,
                    "ciphertext_b64": GOLDEN_CIPHERTEXT_B64,
                    "content_hash_b64": self.content_hash_b64,
                    "nonce_b64": self.nonce_b64,
                    "now_unix": NOW_UNIX,
                }
            }))?,
            "key release",
        )?;
        let material = release["material"].clone();
        if material["sealed_cek_b64"].as_str().unwrap_or_default().is_empty() {
            return Err(format!("key-provider returned no sealed material: {release}"));
        }
        // Containment on the key->decrypt wire: neither the raw CEK nor the escrow blob is echoed.
        let release_str = serde_json::to_string(&release).map_err(|e| e.to_string())?;
        if release_str.contains(GOLDEN_CEK_B64) {
            return Err("raw CEK leaked in the key-provider response".to_string());
        }
        if release_str.contains(&self.wrapped_cek_b64) {
            return Err("the producer escrow blob was echoed by the key authority".to_string());
        }
        Ok(BTreeMap::from([
            ("ReleaseReceiptV1".to_string(), release_receipt_json()),
            ("material".to_string(), material),
        ]))
    }
}

/// Injected `decrypt` capability: `open_session_v1` pushes the executor-threaded
/// release receipt + sealed material into the boundary, which unwraps in-VM and
/// decrypts a real CENC segment, returning ONLY a scoped session — no CEK, no plaintext
/// crosses the boundary. The `render` step is also a decrypt-provider step but is a
/// no-op here (the smoke drives the open, not playback).
struct DecryptHandle {
    decrypt: Rc<RefCell<Capsule>>,
}

impl ProviderHandle for DecryptHandle {
    fn provider(&self) -> &str {
        "decrypt"
    }
    fn run(&mut self, inputs: &StepInputs) -> Result<BTreeMap<String, Value>, String> {
        if inputs.step.name != "decrypt_session" {
            return Ok(BTreeMap::new());
        }
        // The sealed material rides the context alongside the release receipt (it is
        // not a plan binding edge — only the receipt is — so read it from the context).
        let material = inputs
            .artifact("material")
            .ok_or("decrypt_session lost the sealed material produced by key_release")?
            .clone();
        let mut request = decrypt_request_base();
        thread_into(&mut request, inputs);
        let open = self.decrypt.borrow_mut().call(&json!({
            "op": "open_session_v1",
            "request": request,
            "material": material,
            "now_unix": NOW_UNIX,
        }))?;
        let open_data = ok_data(&open, "decrypt open_session_v1")?;
        if open_data["decision"].as_str() != Some("opened") {
            return Err(format!("decrypt did not open the session: {open}"));
        }
        let session = &open_data["session"];
        if session["is_protected"].as_bool() != Some(true) {
            return Err(format!("decrypt did not report a protected segment: {open}"));
        }
        if session["sample_count"].as_u64() != Some(EXPECTED_SAMPLE_COUNT) {
            return Err(format!(
                "decrypt sample_count mismatch: expected {EXPECTED_SAMPLE_COUNT}, got {open}"
            ));
        }
        // Containment at the consumer edge: neither CEK nor plaintext crosses the boundary.
        let open_str = serde_json::to_string(&open).map_err(|e| e.to_string())?;
        if open_str.contains(GOLDEN_CEK_B64) || open_str.contains("the quick brown fox") {
            return Err("CEK or plaintext leaked from the decrypt boundary".to_string());
        }
        Ok(BTreeMap::from([(
            "decrypt_session".to_string(),
            session.clone(),
        )]))
    }
}

// The smoke registers three runtime-OWNED transports into the lib's
// `RuntimeCapabilityTable` — the same registry type the trusted core will use. Each
// transport holds a shared capsule cell (so the post-walk fail-closed checks + shutdown
// can still reach the binaries) plus the per-session material the runtime provisioned up
// front, and OPENS a fresh per-provider `ProviderHandle` (bound to the same capsule) on
// each open — mirroring PC2's `sessionService` singleton owning the per-backend view
// constructors and minting a fresh view per request.

/// Runtime-owned `rights` transport: opens a `RightsHandle` over the rights capsule.
struct RightsTransport {
    rights: Rc<RefCell<Option<Capsule>>>,
    chain_attestation: Value,
    chain_mode: String,
}

impl ProviderTransport for RightsTransport {
    fn provider(&self) -> &str {
        "rights"
    }
    fn open(&self) -> Box<dyn ProviderHandle> {
        Box::new(RightsHandle {
            rights: self.rights.clone(),
            chain_attestation: self.chain_attestation.clone(),
            chain_mode: self.chain_mode.clone(),
        })
    }
    fn shutdown(&mut self) -> Result<(), String> {
        // The host owns this transport, so it owns the capsule's teardown.
        if let Some(rights) = self.rights.borrow_mut().as_mut() {
            rights.shutdown();
        }
        Ok(())
    }
}

/// Runtime-owned `key` transport: opens a `KeyHandle` over the key-provider capsule,
/// carrying the provisioned escrow + session material.
struct KeyTransport {
    key: Rc<RefCell<Capsule>>,
    kid_hex: String,
    wrapped_cek_b64: String,
    producer_vk_b64: String,
    session_pub_b64: String,
    aad_b64: String,
    content_hash_b64: String,
    nonce_b64: String,
}

impl ProviderTransport for KeyTransport {
    fn provider(&self) -> &str {
        "key"
    }
    fn open(&self) -> Box<dyn ProviderHandle> {
        Box::new(KeyHandle {
            key: self.key.clone(),
            kid_hex: self.kid_hex.clone(),
            wrapped_cek_b64: self.wrapped_cek_b64.clone(),
            producer_vk_b64: self.producer_vk_b64.clone(),
            session_pub_b64: self.session_pub_b64.clone(),
            aad_b64: self.aad_b64.clone(),
            content_hash_b64: self.content_hash_b64.clone(),
            nonce_b64: self.nonce_b64.clone(),
        })
    }
    fn shutdown(&mut self) -> Result<(), String> {
        self.key.borrow_mut().shutdown();
        Ok(())
    }
}

/// Runtime-owned `decrypt` transport: opens a `DecryptHandle` over the decrypt boundary.
struct DecryptTransport {
    decrypt: Rc<RefCell<Capsule>>,
}

impl ProviderTransport for DecryptTransport {
    fn provider(&self) -> &str {
        "decrypt"
    }
    fn open(&self) -> Box<dyn ProviderHandle> {
        Box::new(DecryptHandle {
            decrypt: self.decrypt.clone(),
        })
    }
    fn shutdown(&mut self) -> Result<(), String> {
        self.decrypt.borrow_mut().shutdown();
        Ok(())
    }
}

/// The host's runtime-owned PLAN SOURCE: asks the REAL drm-provider for the canonical
/// open plan (the drm-provider holds no authority and decrypts nothing — it only emits
/// the `planned` plan). Spawns + shuts down the drm capsule per fetch and validates the
/// plan carries the content identity flowing through the chain. `tamper` (shared with
/// `run`) models a source that yields a corrupted plan so the host must fail closed.
struct SmokePlanSource {
    drm_bin: String,
    tamper: Rc<std::cell::Cell<bool>>,
}

impl PlanSource for SmokePlanSource {
    fn fetch(&mut self, _content_id: &str, _viewer_interface: &str) -> Result<Value, String> {
        let mut drm = Capsule::spawn("drm-provider", &self.drm_bin)?;
        ok_data(&drm.call(&json!({ "op": "status" }))?, "drm status")?;
        let mut plan = ok_data(&drm.call(&drm_open_request())?, "drm open")?;
        drm.shutdown();
        if plan["content_id"].as_str() != Some(cid().as_str()) {
            return Err(format!(
                "plan content_id {} != the content identity flowing through the chain {}",
                plan["content_id"],
                cid()
            ));
        }
        if plan["action"].as_str() != Some(ACTION) {
            return Err(format!("plan action {} != {ACTION}", plan["action"]));
        }
        if self.tamper.get() {
            // Relabel the key_release input edge — the real key-provider
            // (deny_unknown_fields over a required `rights_receipt`) must reject it.
            for b in plan["bindings"].as_array_mut().ok_or("plan has no bindings")? {
                if b["into_step"] == json!("key_release") {
                    b["into_field"] = json!("bogus_edge");
                }
            }
        }
        Ok(plan)
    }
}

/// The host's runtime-owned, PERSISTING event store: writes each runtime-event record the
/// host emits (`release_receipt`, `protected_content.open.audit`) as a durable JSON file in
/// `dir`. The `ddrm_plan_runner::PersistingEventSink` builds the CEK-free record (open
/// identity + decision + artifact NAMES, never key material) and hands it here. The runtime
/// analogue of PC2's `/init` persisting the open (the lifetime session + audit log,
/// `sessionManager.ts:78`) — except the durable record holds no CEK. Fail-closed: an I/O
/// error fails the emit, and so the open.
struct FileEventStore {
    dir: PathBuf,
}

impl EventStore for FileEventStore {
    fn persist(&mut self, key: &str, record: &Value) -> Result<(), String> {
        let fname = format!("{}.json", key.replace(['/', ':'], "_"));
        let path = self.dir.join(fname);
        let bytes = serde_json::to_vec_pretty(record).map_err(|e| e.to_string())?;
        std::fs::write(&path, bytes).map_err(|e| format!("persist {}: {e}", path.display()))?;
        Ok(())
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let key_bin = args.first().ok_or("missing <key-provider-bin>")?;
    let decrypt_bin = args.get(1).ok_or("missing <decrypt-provider-bin>")?;
    let drm_bin = args.get(2).ok_or("missing <drm-provider-bin>")?;
    let rights_bin = args.get(3);
    let chain_bin = args.get(4);

    println!("== dDRM consumer-half smoke (drm -> rights -> key -> decrypt, via the runtime-core host DrmHost::open) ==");

    // --- runtime capability provisioning (BEFORE the open): the authority + decrypt
    // boundary come up, the content CEK is ESCROWED to the authority's recipient, and
    // the canonical transcript AAD is computed. These are the capabilities/material the
    // runtime registers as transports — none of it is authority the host holds.
    let (attestation, chain_mode) = chain_attestation(chain_bin)?;
    let rights = match rights_bin {
        Some(bin) => Some(Capsule::spawn("rights-provider", bin)?),
        None => None,
    };

    let mut key = Capsule::spawn("key-provider", key_bin)?;
    let key_init = ok_data(&key.call(&json!({ "op": "init", "config": { "backend": "reference" } }))?, "key init")?;
    let vk_b64 = key_init["seal_verifying_key_b64"]
        .as_str()
        .ok_or("key-provider did not publish a seal verifying key (build with --features key-authority-ref)")?
        .to_string();
    let recipient_pub_b64 = key_init["seal_recipient_pub_b64"]
        .as_str()
        .ok_or("key-provider did not publish an escrow recipient key (build with --features key-authority-ref)")?
        .to_string();
    step(1, "key-provider: reference authority up; verifying + escrow-recipient keys published");

    // ESCROW the content CEK to the authority's recipient — so the CEK reaches the
    // authority SEALED, recovered in-boundary, never handed in raw (no dev shim).
    let recipient_pub_bytes = B64.decode(&recipient_pub_b64).map_err(|e| e.to_string())?;
    let recipient_public = ddrm_envelope::session_public_from_bytes(&recipient_pub_bytes)
        .ok_or("key-provider published a malformed escrow recipient key")?;
    let (producer_signer, producer_vk) = ddrm_envelope::seal::mldsa_seal_keypair([0x70u8; 32]);
    let producer_vk_b64 = B64.encode(&producer_vk);
    let cek_bytes = B64.decode(GOLDEN_CEK_B64).map_err(|e| e.to_string())?;
    let kid16 = [0xC5u8; 16];
    let kid_hex: String = kid16.iter().map(|b| format!("{b:02x}")).collect();
    let escrow = escrow_aad(SUITE_PQ_HYBRID, &kid16, &recipient_pub_bytes);
    let wrapped_cek_b64 = B64.encode(
        ddrm_envelope::seal::seal_bound(&recipient_public, &cek_bytes, &escrow, &producer_signer)
            .to_bytes(),
    );

    let mut decrypt = Capsule::spawn("decrypt-provider", decrypt_bin)?;
    let dec_init = ok_data(
        &decrypt.call(&json!({ "op": "init", "config": { "authority_vk_b64": vk_b64 } }))?,
        "decrypt init",
    )?;
    let session_pub_b64 = dec_init["decrypt_session_public_key_b64"]
        .as_str()
        .ok_or("decrypt-provider did not publish a session key (build with --features rail-material)")?
        .to_string();
    let session_pub = B64.decode(&session_pub_b64).map_err(|e| e.to_string())?;
    step(2, "decrypt-provider: trusts authority; minted + published an in-sandbox session key");

    let content_hash = b"consumer-smoke-content-hash-0001".to_vec(); // 32 bytes
    let nonce = b"consumer-smoke-nonce-1".to_vec();
    let aad = transcript_aad(&session_pub, &content_hash, &nonce);

    // --- build the trusted runtime-core HOST and let it OWN the WHOLE open + the rail.
    // The runtime hands its three capsules to runtime-OWNED transports (the transports own
    // the capsules; the HOST owns the transports, so `host.shutdown()` tears down the rail
    // — the analogue of disposing every per-view handle), registers them into the lib's
    // `RuntimeCapabilityTable`, wires a `SmokePlanSource` (asks the REAL drm-provider for
    // the plan) and a PERSISTING event sink (`ddrm_plan_runner::PersistingEventSink` over a
    // `FileEventStore` writing durable, CEK-free open records to a temp dir), and hands all
    // three to `DrmHost`. `host.open(content_id, viewer)` then fetches the plan, drives it
    // through the registry (`open_drm_plan`'s parse→resolve→execute), and PERSISTS the
    // runtime-event steps — the SAME host entrypoint the trusted core will call, no second
    // code path. The cells are kept ONLY for the raw transcript-mismatch gate below; the
    // host owns teardown (no manual capsule shutdown in `run`).
    let rights_cell = Rc::new(RefCell::new(rights));
    let key_cell = Rc::new(RefCell::new(key));
    let decrypt_cell = Rc::new(RefCell::new(decrypt));
    let mut table = RuntimeCapabilityTable::new();
    table.register(Box::new(RightsTransport {
        rights: rights_cell.clone(),
        chain_attestation: attestation,
        chain_mode: chain_mode.clone(),
    }))?;
    table.register(Box::new(KeyTransport {
        key: key_cell.clone(),
        kid_hex: kid_hex.clone(),
        wrapped_cek_b64: wrapped_cek_b64.clone(),
        producer_vk_b64: producer_vk_b64.clone(),
        session_pub_b64: session_pub_b64.clone(),
        aad_b64: B64.encode(&aad),
        content_hash_b64: B64.encode(&content_hash),
        nonce_b64: B64.encode(&nonce),
    }))?;
    table.register(Box::new(DecryptTransport {
        decrypt: decrypt_cell.clone(),
    }))?;

    // The host's durable event store: a fresh temp dir the runtime persists open records
    // into. The `PersistingEventSink` builds the CEK-free record and writes it here.
    let receipts_dir = std::env::temp_dir().join(format!("ddrm-open-{}", std::process::id()));
    std::fs::create_dir_all(&receipts_dir).map_err(|e| format!("create receipts dir: {e}"))?;

    let tamper = Rc::new(std::cell::Cell::new(false));
    let mut host = DrmHost::new(
        Box::new(SmokePlanSource {
            drm_bin: drm_bin.clone(),
            tamper: tamper.clone(),
        }),
        table,
        Box::new(PersistingEventSink::new(FileEventStore {
            dir: receipts_dir.clone(),
        })),
    );

    // --- the trusted host owns the open end to end ---
    let report = host.open(&cid(), VIEWER)?;
    if report.artifact("decrypt_session").is_none() {
        return Err("the host finished without opening a decrypt session".to_string());
    }
    step(3, "drm-provider (host plan source): emitted the canonical plan (planned); host parsed + validated its order + edges");
    step(4, &format!("rights-provider: on-chain ownership ({chain_mode}) -> allowed; typed receipt issued"));
    step(5, "key-provider: canonical `release` recovered the escrowed CEK + re-sealed it to the session (no raw CEK, no shim)");
    step(6, "decrypt-provider: unwrapped in-VM + decrypted the segment; only a scoped session returned");
    // The host emitted the plan's runtime-OWNED post-steps (no provider performs these).
    if report.events_emitted != ["release_receipt", "protected_content.open.audit"] {
        return Err(format!(
            "host did not emit the plan's runtime events in order: {:?}",
            report.events_emitted
        ));
    }
    // The host PERSISTED a durable record per runtime event — and every record is CEK-free.
    let receipt_files = read_persisted_records(&receipts_dir)?;
    if receipt_files.len() != 2 {
        return Err(format!("expected 2 persisted open records, found {}", receipt_files.len()));
    }
    // The record may carry artifact NAMES (safe — PC2 logs step names) but NEVER any secret
    // VALUE. Forbid the concrete secret bytes that flowed through this open: the CEK, the
    // sealed/escrowed material, the ciphertext, and the session/producer keys.
    for (path, record) in &receipt_files {
        let blob = serde_json::to_string(record).map_err(|e| e.to_string())?;
        let secrets = [
            ("CEK", GOLDEN_CEK_B64),
            ("ciphertext", GOLDEN_CIPHERTEXT_B64),
            ("wrapped/escrowed CEK", wrapped_cek_b64.as_str()),
            ("producer vk", producer_vk_b64.as_str()),
            ("decrypt session key", session_pub_b64.as_str()),
        ];
        for (what, secret) in secrets {
            if blob.contains(secret) {
                return Err(format!("persisted open record {} leaks the {what}", path.display()));
            }
        }
        // It records artifact NAMES, never the artifact map (values).
        if record.get("artifacts").is_some() {
            return Err(format!("persisted record {} embeds artifact values", path.display()));
        }
        if record["content_id"].as_str() != Some(cid().as_str()) {
            return Err(format!("persisted record {} has wrong content_id", path.display()));
        }
    }
    step(7, "runtime-core host: PERSISTED the runtime-owned post-steps (release_receipt + audit) as durable CEK-free records");

    // --- fail-closed #1 (crypto binding): a replayed/altered transcript must not open.
    // Re-seal to a DIFFERENT nonce while the material still names the original — the
    // boundary rebuilds the original transcript and the seal cannot open.
    let bad_nonce = b"consumer-smoke-nonce-9".to_vec();
    let bad_aad = transcript_aad(&session_pub, &content_hash, &bad_nonce);
    let mut bad_req = key_release_request_base(&kid_hex, &wrapped_cek_b64);
    bad_req
        .as_object_mut()
        .expect("key release request is an object")
        .insert("rights_receipt".to_string(), fallback_rights_receipt());
    let bad_release = ok_data(
        &key_cell.borrow_mut().call(&json!({
            "op": "release",
            "request": bad_req,
            "session": {
                "decrypt_session_pub_b64": session_pub_b64,
                "producer_vk_b64": producer_vk_b64,
                "aad_b64": B64.encode(&bad_aad),
                "ciphertext_b64": GOLDEN_CIPHERTEXT_B64,
                "content_hash_b64": B64.encode(&content_hash),
                "nonce_b64": B64.encode(&nonce),
                "now_unix": NOW_UNIX,
            }
        }))?,
        "key release (mismatch)",
    )?;
    let mut bad_open_req = decrypt_request_base();
    bad_open_req
        .as_object_mut()
        .expect("decrypt request is an object")
        .insert("release_receipt".to_string(), release_receipt_json());
    bad_open_req["object_cid"] = json!(cid());
    bad_open_req["viewer_interface"] = json!(VIEWER);
    let bad_open = decrypt_cell.borrow_mut().call(&json!({
        "op": "open_session_v1",
        "request": bad_open_req,
        "material": bad_release["material"].clone(),
        "now_unix": NOW_UNIX,
    }))?;
    if bad_open.get("data").and_then(|d| d.get("decision")).and_then(Value::as_str) == Some("opened") {
        return Err(format!("a transcript-mismatched seal must NOT open: {bad_open}"));
    }
    step(8, "decrypt-provider: a transcript-mismatched seal failed closed");

    // --- fail-closed #2 (plan integrity): flip the host's plan source into TAMPER mode
    // (it relabels the key_release input edge) and re-open through the SAME host. The
    // host fetches the corrupted plan, threads the rights receipt into the wrong field,
    // and the real key-provider (deny_unknown_fields over a required `rights_receipt`)
    // rejects it — proving the host only proceeds when the plan's edges are intact, and
    // that a bad plan FROM THE SOURCE fails closed, cross-binary, with no event emitted.
    let persisted_before = read_persisted_records(&receipts_dir)?.len();
    tamper.set(true);
    match host.open(&cid(), VIEWER) {
        Ok(_) => return Err("a tampered plan edge must NOT drive a successful open".to_string()),
        Err(_) => step(9, "runtime-core host: a tampered binding edge from the plan source failed closed at the real key-provider"),
    }
    if read_persisted_records(&receipts_dir)?.len() != persisted_before {
        return Err("a failed open must persist no runtime-event record".to_string());
    }

    // The HOST owns the rail it was handed: shutting it down tears down every runtime-owned
    // transport (each shuts down the capsule it owns) — no manual per-capsule shutdown here.
    host.shutdown()?;
    step(10, "runtime-core host: shut down — every runtime-owned transport tore down its capsule");
    // The cells kept for the raw gate are now stale (the host tore the processes down); drop
    // them and clean up the durable records the smoke wrote.
    drop(rights_cell);
    drop(key_cell);
    drop(decrypt_cell);
    let _ = std::fs::remove_dir_all(&receipts_dir);
    Ok(())
}

/// Read every persisted open record the host wrote into `dir` (as `(path, json)`), so the
/// smoke can assert the durable receipt/audit were written and are CEK-free.
fn read_persisted_records(dir: &std::path::Path) -> Result<Vec<(PathBuf, Value)>, String> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| format!("read receipts dir: {e}"))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        let record: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        out.push((path, record));
    }
    Ok(out)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => {
            println!("\nconsumer-smoke: PASS — the consumer half runs end to end (key -> decrypt), fail-closed, no key/plaintext leak.");
        }
        Err(e) => {
            eprintln!("\nconsumer-smoke: FAIL — {e}");
            std::process::exit(1);
        }
    }
}
