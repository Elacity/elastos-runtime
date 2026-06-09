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
//! Usage: ddrm-consumer-smoke <key-provider-bin> <decrypt-provider-bin> [drm-bin] [rights-bin]

use base64::Engine as _;
use ddrm_envelope::transcript::{escrow_aad, release_receipt_hash, DecryptTranscriptV1};
use ddrm_envelope::SUITE_PQ_HYBRID;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

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

    fn shutdown(mut self) {
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

/// Build the decrypt-session request, binding the release receipt under the field
/// name the drm-provider's plan declares for the `key_release -> decrypt_session`
/// edge (default `release_receipt`). Driving the field off the plan means a wrong
/// edge would place the receipt under an unknown field and fail closed.
fn decrypt_request(release_field: &str) -> Value {
    let mut req = json!({
        "schema": "elastos.decrypt.session.request/v1",
        "request_id": "decrypt:consumer-smoke",
        "principal_id": PRINCIPAL,
        "session_id": SESSION,
        "object_cid": cid(),
        "action": ACTION,
        "viewer_interface": VIEWER,
        "output_kind": OUTPUT_KIND,
        "reason": "open protected document",
        "expires_at": EXPIRES_AT,
    });
    req.as_object_mut()
        .expect("decrypt request is an object")
        .insert(release_field.to_string(), release_receipt_json());
    req
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

/// Build the key-release request, binding the rights decision receipt under the
/// field name the drm-provider's plan declares for the `rights_check -> key_release`
/// edge (default `rights_receipt`).
fn key_release_request(
    rights_receipt: Value,
    rights_field: &str,
    kid_hex: &str,
    wrapped_cek_b64: &str,
) -> Value {
    let mut req = json!({
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
    });
    req.as_object_mut()
        .expect("key release request is an object")
        .insert(rights_field.to_string(), rights_receipt);
    req
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

/// The request field a plan binding edge declares for an artifact flowing into a
/// step — so the orchestrator threads receipts where the PLAN says, not where a
/// hardcoded literal says.
fn plan_binding_field(plan: &Value, produces: &str, into_step: &str) -> Result<String, String> {
    plan["bindings"]
        .as_array()
        .and_then(|edges| {
            edges.iter().find(|b| {
                b["produces"].as_str() == Some(produces)
                    && b["into_step"].as_str() == Some(into_step)
            })
        })
        .and_then(|b| b["into_field"].as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("plan declares no binding {produces} -> {into_step}"))
}

/// Assert the canonical order the runtime must follow: the rights check precedes the
/// key release, which precedes the decrypt session.
fn assert_plan_order(plan: &Value) -> Result<(), String> {
    let steps = plan["steps"].as_array().ok_or("plan has no steps")?;
    let pos = |provider: &str, op: &str| {
        steps.iter().position(|s| {
            s["provider"].as_str() == Some(provider) && s["operation"].as_str() == Some(op)
        })
    };
    let rights = pos("rights", "has_access_by_content_id").ok_or("plan missing rights step")?;
    let key = pos("key", "release").ok_or("plan missing key step")?;
    let decrypt = pos("decrypt", "open_session").ok_or("plan missing decrypt step")?;
    if rights < key && key < decrypt {
        Ok(())
    } else {
        Err(format!(
            "plan steps out of canonical order: rights={rights} key={key} decrypt={decrypt}"
        ))
    }
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

fn run(args: &[String]) -> Result<(), String> {
    let key_bin = args.first().ok_or("missing <key-provider-bin>")?;
    let decrypt_bin = args.get(1).ok_or("missing <decrypt-provider-bin>")?;
    let drm_bin = args.get(2);
    let rights_bin = args.get(3);
    let chain_bin = args.get(4);

    println!("== dDRM consumer-half smoke (drm -> rights -> key -> decrypt) ==");

    // --- front of chain: ask the REAL drm-provider for the canonical open PLAN. -----
    // The orchestrator no longer hardcodes the rights -> key -> decrypt order; it
    // drives `drm open`, then FOLLOWS the plan: its declared step order and its
    // binding edges (which receipt feeds which next-step field). The drm-provider
    // holds no authority and decrypts nothing — it only emits the plan (`planned`).
    let plan = if let Some(bin) = drm_bin {
        let mut drm = Capsule::spawn("drm-provider", bin)?;
        ok_data(&drm.call(&json!({ "op": "status" }))?, "drm status")?;
        let plan = ok_data(&drm.call(&drm_open_request())?, "drm open")?;
        drm.shutdown();
        if plan["status"].as_str() != Some("planned") {
            return Err(format!("drm open must return a `planned` plan, got {plan}"));
        }
        if plan["content_id"].as_str() != Some(cid().as_str()) {
            return Err(format!(
                "plan content_id {} != the content identity flowing through the chain {}",
                plan["content_id"], cid()
            ));
        }
        if plan["action"].as_str() != Some(ACTION) {
            return Err(format!("plan action {} != {ACTION}", plan["action"]));
        }
        assert_plan_order(&plan)?;
        step(1, "drm-provider: emitted the canonical open plan (planned); following its order + bindings");
        Some(plan)
    } else {
        None
    };

    // Thread receipts where the PLAN declares, not where a hardcoded literal does.
    // Without a drm binary, fall back to the canonical field names directly.
    let (rights_field, release_field) = match &plan {
        Some(p) => (
            plan_binding_field(p, "RightsDecisionReceiptV1", "key_release")?,
            plan_binding_field(p, "ReleaseReceiptV1", "decrypt_session")?,
        ),
        None => ("rights_receipt".to_string(), "release_receipt".to_string()),
    };
    // rights step: render the (mocked) on-chain ownership answer into a typed
    // RightsDecisionReceiptV1 via the REAL rights-provider (feature `chain-rights`).
    // The receipt then gates the key release. Falls back to a hardcoded receipt only
    // when no rights binary is supplied.
    // Resolve the on-chain ownership answer: a live `chain-provider` call when the
    // DDRM_SMOKE_CHAIN_* env is set (your wallet vs the AuthorityGateway on Base),
    // otherwise a mocked-owned attestation so the offline smoke is deterministic.
    let (attestation, chain_mode) = chain_attestation(chain_bin)?;

    let rights_receipt = if let Some(bin) = rights_bin {
        let mut rights = Capsule::spawn("rights-provider", bin)?;
        ok_data(&rights.call(&json!({ "op": "status" }))?, "rights status")?;
        let decision = ok_data(
            &rights.call(&json!({
                "op": "decide_access_from_chain",
                "request_id": RR_REQUEST_ID,
                "request": rights_access_request(),
                "chain_access": attestation,
                "now_unix": RR_ISSUED_AT,
                "ttl_secs": EXPIRES_AT - RR_ISSUED_AT,
            }))?,
            "rights decide_access_from_chain",
        )?;
        if decision["decision"].as_str() != Some("allowed") {
            return Err(format!(
                "rights did not allow this content ({chain_mode}); the chain says you do not own it: {decision}"
            ));
        }
        rights.shutdown();
        step(2, &format!("rights-provider: on-chain ownership ({chain_mode}) -> allowed; typed receipt issued"));
        decision["receipt"].clone()
    } else {
        fallback_rights_receipt()
    };

    // --- key authority: publish the seal verifying key AND the escrow recipient key. -
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
    step(3, "key-provider: reference authority up; verifying + escrow-recipient keys published");

    // ESCROW the content CEK to the authority's recipient — so the CEK reaches the
    // authority SEALED, recovered in-boundary, never handed in raw (no dev shim). The
    // golden CEK is the content key; a producer (here, the smoke) seals it under the
    // SHARED escrow AAD bound to a bytes16 KID, signed by its ML-DSA key. This is the
    // exact producer→authority escrow the canonical `release` op recovers from.
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

    // --- decrypt boundary: trust the authority, then MINT + PUBLISH a session key. --
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
    step(4, "decrypt-provider: trusts authority; minted + published an in-sandbox session key");

    // --- compute the shared transcript AAD + seal the CEK to the published key. -----
    let content_hash = b"consumer-smoke-content-hash-0001".to_vec(); // 32 bytes
    let nonce = b"consumer-smoke-nonce-1".to_vec();
    let aad = transcript_aad(&session_pub, &content_hash, &nonce);

    // The CANONICAL `release` op (the one drm-provider's plan names): the authority
    // RECOVERS the escrowed CEK from the rights-bound key_envelope and re-seals it to the
    // published session key — no raw CEK is ever handed in.
    let release = ok_data(
        &key.call(&json!({
            "op": "release",
            "request": key_release_request(rights_receipt.clone(), &rights_field, &kid_hex, &wrapped_cek_b64),
            "session": {
                "decrypt_session_pub_b64": session_pub_b64,
                "producer_vk_b64": producer_vk_b64,
                "aad_b64": B64.encode(&aad),
                "ciphertext_b64": GOLDEN_CIPHERTEXT_B64,
                "content_hash_b64": B64.encode(&content_hash),
                "nonce_b64": B64.encode(&nonce),
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
    if release_str.contains(&wrapped_cek_b64) {
        return Err("the producer escrow blob was echoed by the key authority".to_string());
    }
    step(5, "key-provider: canonical `release` recovered the escrowed CEK + re-sealed it to the session (no raw CEK, no shim)");

    // --- decrypt: push the sealed material in, unwrap in-VM, decrypt the segment. ----
    let open = decrypt.call(&json!({
        "op": "open_session_v1",
        "request": decrypt_request(&release_field),
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
    step(6, "decrypt-provider: unwrapped in-VM + decrypted the segment; only a scoped session returned");

    // A replayed/altered transcript must fail closed: flip the nonce the authority
    // sealed to and prove the boundary refuses to open.
    let bad_nonce = b"consumer-smoke-nonce-9".to_vec();
    let bad_aad = transcript_aad(&session_pub, &content_hash, &bad_nonce);
    let bad_release = ok_data(
        &key.call(&json!({
            "op": "release",
            "request": key_release_request(rights_receipt.clone(), &rights_field, &kid_hex, &wrapped_cek_b64),
            "session": {
                "decrypt_session_pub_b64": session_pub_b64,
                "producer_vk_b64": producer_vk_b64,
                "aad_b64": B64.encode(&bad_aad),
                "ciphertext_b64": GOLDEN_CIPHERTEXT_B64,
                "content_hash_b64": B64.encode(&content_hash),
                // The material's nonce still says nonce-1, so the boundary rebuilds the
                // ORIGINAL transcript and the seal (bound to nonce-9) cannot open.
                "nonce_b64": B64.encode(&nonce),
                "now_unix": NOW_UNIX,
            }
        }))?,
        "key release (mismatch)",
    )?;
    let bad_open = decrypt.call(&json!({
        "op": "open_session_v1",
        "request": decrypt_request(&release_field),
        "material": bad_release["material"].clone(),
        "now_unix": NOW_UNIX,
    }))?;
    let opened = bad_open
        .get("data")
        .and_then(|d| d.get("decision"))
        .and_then(Value::as_str)
        == Some("opened");
    if opened {
        return Err(format!("a transcript-mismatched seal must NOT open: {bad_open}"));
    }
    step(7, "decrypt-provider: a transcript-mismatched seal failed closed");

    key.shutdown();
    decrypt.shutdown();
    Ok(())
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
