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
//!   3. the authority seals a CEK to that published key, bound to the canonical
//!      decrypt transcript (`key release_ref`) — the SAME `to_aad` both sides share;
//!   4. the decrypt boundary unwraps with its in-VM secret and decrypts a real CENC
//!      segment (`decrypt open_session_v1`), returning ONLY a scoped session — no CEK,
//!      no plaintext crosses the process boundary.
//!
//! The content `(CEK, ciphertext, plaintext)` is a committed golden
//! (`capsules/decrypt-provider/tests/vectors/classical_cenc.json`); the authority is
//! handed that CEK directly (the dev reference backend), seals it, and the boundary
//! recovers it and decrypts — proving the rail, not the cenc engine (already pinned).
//!
//! Usage: ddrm-consumer-smoke <key-provider-bin> <decrypt-provider-bin> [drm-bin] [rights-bin]

use base64::Engine as _;
use ddrm_envelope::transcript::{release_receipt_hash, DecryptTranscriptV1};
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

fn decrypt_request() -> Value {
    json!({
        "schema": "elastos.decrypt.session.request/v1",
        "request_id": "decrypt:consumer-smoke",
        "principal_id": PRINCIPAL,
        "session_id": SESSION,
        "object_cid": OBJECT_CID,
        "action": ACTION,
        "viewer_interface": VIEWER,
        "release_receipt": {
            "schema": RR_SCHEMA,
            "request_id": RR_REQUEST_ID,
            "object_cid": OBJECT_CID,
            "principal_id": PRINCIPAL,
            "session_id": SESSION,
            "action": ACTION,
            "provider": RR_PROVIDER,
            "status": RR_STATUS,
            "issued_at": RR_ISSUED_AT,
            "expires_at": EXPIRES_AT,
        },
        "output_kind": OUTPUT_KIND,
        "reason": "open protected document",
        "expires_at": EXPIRES_AT,
    })
}

fn rights_access_request() -> Value {
    json!({
        "principal_id": PRINCIPAL,
        "session_id": SESSION,
        "content_id": OBJECT_CID,
        "right": ACTION,
        "reason": "open protected document",
    })
}

/// A mocked on-chain ownership answer (stands in for `chain-provider::has_access_by_
/// content_id`; no live RPC in the smoke). `has_access: true` => owned.
fn owned_chain_attestation() -> Value {
    json!({
        "network": "base",
        "contract": "0x00000000000000000000000000000000000000aa",
        "content_id": OBJECT_CID,
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
        "content_id": OBJECT_CID,
        "principal_id": PRINCIPAL,
        "session_id": SESSION,
        "right": ACTION,
        "provider": "rights-provider",
        "allowed": true,
        "issued_at": RR_ISSUED_AT,
        "expires_at": EXPIRES_AT,
    })
}

fn key_release_request(rights_receipt: Value) -> Value {
    json!({
        "schema": "elastos.key_release.request/v1",
        "request_id": RR_REQUEST_ID,
        "principal_id": PRINCIPAL,
        "session_id": SESSION,
        "object_cid": OBJECT_CID,
        "action": ACTION,
        "rights_receipt": rights_receipt,
        "key_envelope": {
            "scheme": "elastos-pq-hybrid-threshold-v0",
            "kid": "kid:smoke",
            "wrapped_cek": "wrapped",
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

/// Rebuild the canonical decrypt-transcript AAD exactly as the decrypt boundary will,
/// using the SHARED `ddrm-envelope` encoder (no parallel definition).
fn transcript_aad(session_pub: &[u8], content_hash: &[u8], nonce: &[u8]) -> Vec<u8> {
    let receipt_hash = release_receipt_hash(
        RR_SCHEMA,
        RR_REQUEST_ID,
        OBJECT_CID,
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
        object_cid: OBJECT_CID,
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

fn run(args: &[String]) -> Result<(), String> {
    let key_bin = args.first().ok_or("missing <key-provider-bin>")?;
    let decrypt_bin = args.get(1).ok_or("missing <decrypt-provider-bin>")?;
    let drm_bin = args.get(2);
    let rights_bin = args.get(3);

    println!("== dDRM consumer-half smoke (drm -> rights -> key -> decrypt) ==");

    // --- front of chain: prove the providers are sequenced + alive (real ops). ------
    if let Some(bin) = drm_bin {
        let mut drm = Capsule::spawn("drm-provider", bin)?;
        let resp = drm.call(&json!({ "op": "status" }))?;
        ok_data(&resp, "drm status")?;
        step(1, "drm-provider: status ok (front door reachable)");
        drm.shutdown();
    }
    // rights step: render the (mocked) on-chain ownership answer into a typed
    // RightsDecisionReceiptV1 via the REAL rights-provider (feature `chain-rights`).
    // The receipt then gates the key release. Falls back to a hardcoded receipt only
    // when no rights binary is supplied.
    let rights_receipt = if let Some(bin) = rights_bin {
        let mut rights = Capsule::spawn("rights-provider", bin)?;
        ok_data(&rights.call(&json!({ "op": "status" }))?, "rights status")?;
        let decision = ok_data(
            &rights.call(&json!({
                "op": "decide_access_from_chain",
                "request_id": RR_REQUEST_ID,
                "request": rights_access_request(),
                "chain_access": owned_chain_attestation(),
                "now_unix": RR_ISSUED_AT,
                "ttl_secs": EXPIRES_AT - RR_ISSUED_AT,
            }))?,
            "rights decide_access_from_chain",
        )?;
        if decision["decision"].as_str() != Some("allowed") {
            return Err(format!("rights did not allow owned content: {decision}"));
        }
        rights.shutdown();
        step(2, "rights-provider: on-chain ownership -> allowed; typed receipt issued");
        decision["receipt"].clone()
    } else {
        fallback_rights_receipt()
    };

    // --- key authority: publish the seal verifying key. -----------------------------
    let mut key = Capsule::spawn("key-provider", key_bin)?;
    let key_init = ok_data(&key.call(&json!({ "op": "init", "config": { "backend": "reference" } }))?, "key init")?;
    let vk_b64 = key_init["seal_verifying_key_b64"]
        .as_str()
        .ok_or("key-provider did not publish a seal verifying key (build with --features key-authority-ref)")?
        .to_string();
    step(3, "key-provider: reference authority up; verifying key published");

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

    let release = ok_data(
        &key.call(&json!({
            "op": "release_ref",
            "request": key_release_request(rights_receipt.clone()),
            "decrypt_session_pub_b64": session_pub_b64,
            "cek_b64": GOLDEN_CEK_B64,
            "aad_b64": B64.encode(&aad),
            "ciphertext_b64": GOLDEN_CIPHERTEXT_B64,
            "content_hash_b64": B64.encode(&content_hash),
            "nonce_b64": B64.encode(&nonce),
        }))?,
        "key release_ref",
    )?;
    let material = release["material"].clone();
    if material["sealed_cek_b64"].as_str().unwrap_or_default().is_empty() {
        return Err(format!("key-provider returned no sealed material: {release}"));
    }
    // Containment on the key->decrypt wire: the raw CEK is never echoed.
    let release_str = serde_json::to_string(&release).map_err(|e| e.to_string())?;
    if release_str.contains(GOLDEN_CEK_B64) {
        return Err("raw CEK leaked in the key-provider response".to_string());
    }
    step(5, "key-provider: sealed the CEK to the published key, transcript-bound (no raw CEK on the wire)");

    // --- decrypt: push the sealed material in, unwrap in-VM, decrypt the segment. ----
    let open = decrypt.call(&json!({
        "op": "open_session_v1",
        "request": decrypt_request(),
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
            "op": "release_ref",
            "request": key_release_request(rights_receipt.clone()),
            "decrypt_session_pub_b64": session_pub_b64,
            "cek_b64": GOLDEN_CEK_B64,
            "aad_b64": B64.encode(&bad_aad),
            "ciphertext_b64": GOLDEN_CIPHERTEXT_B64,
            "content_hash_b64": B64.encode(&content_hash),
            // The material's nonce still says nonce-1, so the boundary rebuilds the
            // ORIGINAL transcript and the seal (bound to nonce-9) cannot open.
            "nonce_b64": B64.encode(&nonce),
        }))?,
        "key release_ref (mismatch)",
    )?;
    let bad_open = decrypt.call(&json!({
        "op": "open_session_v1",
        "request": decrypt_request(),
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
