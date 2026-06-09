//! dDRM producer-half orchestration smoke (Phase C, Day 60).
//!
//! Drives the REAL capsule binaries over their newline-delimited JSON stdin/stdout
//! protocol to prove the PRODUCER half of the Elacity dDRM chain runs end to end:
//!
//!   encrypt (mint CEK + seal_inline)  ->  key (recover-from-escrow + re-seal)  ->
//!   decrypt (OpenSessionV1)
//!
//! Unlike the consumer-half smoke (which seals a committed GOLDEN CEK), here a CEK is
//! minted RIGHT NOW inside encrypt-provider, used to CENC-encrypt fresh plaintext, and
//! ESCROWED to the key authority's published recipient key. The authority recovers it
//! from the escrow blob (never a raw CEK on the wire), re-seals it to the decrypt
//! boundary's freshly-minted session key, and the boundary unwraps + decrypts the
//! segment that was sealed in THIS run — "a video sealed now decrypts now".
//!
//! Containment is asserted on every wire: no raw CEK and no plaintext ever leaves a
//! process. Fail-closed throughout; no golden.
//!
//! Usage: ddrm-producer-smoke <encrypt-bin> <key-bin> <decrypt-bin>

use base64::Engine as _;
use ddrm_envelope::transcript::{release_receipt_hash, DecryptTranscriptV1};
use ddrm_envelope::SUITE_PQ_HYBRID;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

// --- the shared identity used by BOTH the decrypt request and the sealed transcript -
const PRINCIPAL: &str = "person:local:producer-smoke";
const SESSION: &str = "session:producer-smoke";
const OBJECT_CID: &str = "bafybeiproducedrightnow";
const ACTION: &str = "view";
const VIEWER: &str = "elastos.viewer/document@1";
const OUTPUT_KIND: &str = "rendered";
const EXPIRES_AT: u64 = 1_900_000_000;
const NOW_UNIX: u64 = 1_850_000_000;

const RR_SCHEMA: &str = "elastos.release.receipt/v1";
const RR_REQUEST_ID: &str = "key-release:producer-smoke";
const RR_PROVIDER: &str = "key-provider";
const RR_STATUS: &str = "released";
const RR_ISSUED_AT: u64 = 1_800_000_000;

const EXPECTED_SAMPLE_COUNT: u64 = 1;

/// The fresh plaintext sealed in THIS run — its bytes must never appear on any wire,
/// and the only place it can be recovered is inside the decrypt sandbox.
const PLAINTEXT: &[u8] = b"elastos dDRM: this content was sealed *now* and decrypts *now*!!";

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
        Ok(Self {
            name: name.to_string(),
            child,
            stdin,
            stdout,
        })
    }

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
        serde_json::from_str(resp.trim())
            .map_err(|e| format!("{} sent non-JSON: {e}: {resp}", self.name))
    }

    fn shutdown(mut self) {
        let _ = self.call(&json!({ "op": "shutdown" }));
        let _ = self.child.wait();
    }
}

/// Independently recompute the IPFS CIDv1 (raw codec, sha2-256) of `bytes` using the
/// canonical IPLD `cid` crate — the ecosystem oracle. encrypt-provider derives the same
/// CID with a hand-rolled multibase/multihash assembly; if the two disagree, the
/// producer's `payload_cid` is not a real content address and the smoke fails closed.
fn recompute_payload_cid(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    const SHA2_256: u64 = 0x12;
    const RAW_CODEC: u64 = 0x55;
    let digest = Sha256::digest(bytes);
    let mh = cid::multihash::Multihash::<64>::wrap(SHA2_256, &digest)
        .expect("sha2-256 digest fits a 64-byte multihash");
    cid::Cid::new_v1(RAW_CODEC, mh).to_string()
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
        "request_id": "decrypt:producer-smoke",
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
        "reason": "open content sealed in this run",
        "expires_at": EXPIRES_AT,
    })
}

fn fallback_rights_receipt() -> Value {
    json!({
        "schema": "elastos.rights.decision.receipt/v1",
        "request_id": "rights:producer-smoke",
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

fn key_release_request() -> Value {
    json!({
        "schema": "elastos.key_release.request/v1",
        "request_id": RR_REQUEST_ID,
        "principal_id": PRINCIPAL,
        "session_id": SESSION,
        "object_cid": OBJECT_CID,
        "action": ACTION,
        "rights_receipt": fallback_rights_receipt(),
        "key_envelope": {
            "scheme": "elastos-pq-hybrid-threshold-v0",
            "kid": "kid:producer-smoke",
            "wrapped_cek": "wrapped",
            "policy_hash": "sha256:producer-smoke",
            "algorithms": {
                "cipher": "aes-256-gcm",
                "signature": ["ed25519", "ml-dsa-65"],
                "kem": ["x25519", "ml-kem-768"],
                "share_scheme": "shamir-t-of-n",
            },
        },
        "reason": "open content sealed in this run",
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
    let encrypt_bin = args.first().ok_or("missing <encrypt-provider-bin>")?;
    let key_bin = args.get(1).ok_or("missing <key-provider-bin>")?;
    let decrypt_bin = args.get(2).ok_or("missing <decrypt-provider-bin>")?;

    println!("== dDRM producer-half smoke (encrypt -> key[recover+re-seal] -> decrypt) ==");

    // --- key authority: publish the seal verifying key AND the escrow recipient key. -
    let mut key = Capsule::spawn("key-provider", key_bin)?;
    let key_init = ok_data(
        &key.call(&json!({ "op": "init", "config": { "backend": "reference" } }))?,
        "key init",
    )?;
    let vk_b64 = key_init["seal_verifying_key_b64"]
        .as_str()
        .ok_or("key-provider did not publish a seal verifying key (build --features key-authority-ref)")?
        .to_string();
    let recipient_pub_b64 = key_init["seal_recipient_pub_b64"]
        .as_str()
        .ok_or("key-provider did not publish an escrow recipient key (build --features key-authority-ref)")?
        .to_string();
    step(1, "key-provider: reference authority up; verifying + escrow-recipient keys published");

    // --- producer: mint a CEK NOW, encrypt fresh plaintext, escrow the CEK. ----------
    let mut encrypt = Capsule::spawn("encrypt-provider", encrypt_bin)?;
    let enc_init = ok_data(&encrypt.call(&json!({ "op": "init" }))?, "encrypt init")?;
    let producer_vk_b64 = enc_init["producer_verifying_key_b64"]
        .as_str()
        .ok_or("encrypt-provider did not publish a producer verifying key (build --features escrow)")?
        .to_string();

    let sealed = ok_data(
        &encrypt.call(&json!({
            "op": "seal_inline",
            "plaintext_b64": B64.encode(PLAINTEXT),
            "recipient_pub_b64": recipient_pub_b64,
        }))?,
        "encrypt seal_inline",
    )?;
    let kid_hex = sealed["kid_hex"].as_str().ok_or("no kid_hex from encrypt")?.to_string();
    let scheme = sealed["scheme"].as_str().ok_or("no scheme from encrypt")?.to_string();
    let segment_b64 = sealed["segment_b64"].as_str().ok_or("no segment from encrypt")?.to_string();
    let payload_cid = sealed["payload_cid"]
        .as_str()
        .ok_or("no payload_cid from encrypt (the producer must content-address the segment)")?
        .to_string();
    let wrapped_cek_b64 = sealed["wrapped_cek_b64"]
        .as_str()
        .ok_or("no wrapped_cek from encrypt")?
        .to_string();

    // Containment on the encrypt wire: the response carries ciphertext + SEALED CEK,
    // never the fresh plaintext.
    let sealed_str = serde_json::to_string(&sealed).map_err(|e| e.to_string())?;
    if sealed_str.contains(&B64.encode(PLAINTEXT)) {
        return Err("plaintext leaked in the encrypt-provider response".to_string());
    }

    // --- production `seal`: drive the FULL pipeline on HANDED-IN bytes -> SealedObjectV1.
    // Same boundary, same recipient — but now the production op assembles the complete,
    // chain-shaped sealed object (Day 69). The capsule fetches nothing; it seals the bytes
    // the orchestrator hands it, exactly as PC2's host hands segment bytes to the CENC WASM.
    let prod = ok_data(
        &encrypt.call(&json!({
            "op": "seal",
            "request": {
                "schema": "elastos.encrypt.seal.request/v1",
                "plaintext_ref": "producer-smoke-asset-handle",
                "content_b64": B64.encode(PLAINTEXT),
                "recipient_pub_b64": recipient_pub_b64,
                "availability_receipt_cid": "bafyproducersmokeavail",
                "rights_policy_cid": "bafyrightspolicy",
                "scheme": "elastos-pq-hybrid-threshold-v0",
                "viewer": { "required_interface": "media" }
            }
        }))?,
        "encrypt seal (production)",
    )?;
    encrypt.shutdown();

    // The response must deserialize into the SHARED `SealedObjectV1` contract (so its full
    // shape + `deny_unknown_fields` hold) and its algorithm suite must pass the SAME
    // validator the downstream key-provider runs — cross-binary proof the chain accepts it.
    let prod_str = serde_json::to_string(&prod).map_err(|e| e.to_string())?;
    if prod_str.contains(&B64.encode(PLAINTEXT)) {
        return Err("plaintext leaked in the production seal response".to_string());
    }
    let sealed_object: elastos_common::protected_content::SealedObjectV1 =
        serde_json::from_value(prod["sealed_object"].clone())
            .map_err(|e| format!("production seal did not emit a valid SealedObjectV1: {e}"))?;
    elastos_common::protected_content::validate_protected_content_key_envelope_algorithms(
        &sealed_object.key_envelope.algorithms,
    )
    .map_err(|e| format!("producer's SealedObjectV1 rejected by the shared chain validator: {e}"))?;
    if !sealed_object.payload_cid.starts_with("bafkrei") {
        return Err(format!(
            "production seal payload_cid {} is not a raw CIDv1/sha256",
            sealed_object.payload_cid
        ));
    }
    // The envelope KID must be the on-chain bytes16 contentId (32 lowercase hex chars),
    // a DIFFERENT identity from the payload CID — the two must never be conflated.
    if sealed_object.key_envelope.kid.len() != 32
        || !sealed_object.key_envelope.kid.chars().all(|c| c.is_ascii_hexdigit())
    {
        return Err(format!("envelope kid {} is not a bytes16 contentId", sealed_object.key_envelope.kid));
    }
    if sealed_object.payload_cid == sealed_object.key_envelope.kid {
        return Err("payload CID and contentId(KID) must be distinct identities".to_string());
    }
    if sealed_object.key_envelope.wrapped_cek.is_empty() {
        return Err("production SealedObjectV1 carries no SEALED CEK".to_string());
    }

    // payload_cid is REAL, not a placeholder: independently recompute the CIDv1 of the
    // exact ciphertext bytes the producer emitted (via the canonical `cid` crate) and
    // demand a byte-for-byte match. This is the point a human can verify the producer's
    // content address resolves to the bytes it sealed — no "trust me".
    let segment_bytes = B64.decode(&segment_b64).map_err(|e| e.to_string())?;
    let expected_cid = recompute_payload_cid(&segment_bytes);
    if payload_cid != expected_cid {
        return Err(format!(
            "producer payload_cid {payload_cid} != independently-recomputed CID {expected_cid} \
             of the {}-byte ciphertext segment",
            segment_bytes.len()
        ));
    }
    if !payload_cid.starts_with("bafkrei") {
        return Err(format!("payload_cid {payload_cid} is not a raw CIDv1/sha256 (bafkrei…)"));
    }
    step(2, &format!("encrypt-provider: minted CEK + escrowed it; kid={kid_hex} (no plaintext on the wire)"));
    step(3, &format!("encrypt-provider: payload_cid={payload_cid} — matches the independently-recomputed CID of the sealed segment"));
    step(4, &format!(
        "encrypt-provider: production `seal` emitted a complete SealedObjectV1 (payload_cid={}, kid={}) — accepted by the shared chain validator",
        sealed_object.payload_cid, sealed_object.key_envelope.kid
    ));

    // --- decrypt boundary: trust the authority, then MINT + PUBLISH a session key. ---
    let mut decrypt = Capsule::spawn("decrypt-provider", decrypt_bin)?;
    let dec_init = ok_data(
        &decrypt.call(&json!({ "op": "init", "config": { "authority_vk_b64": vk_b64 } }))?,
        "decrypt init",
    )?;
    let session_pub_b64 = dec_init["decrypt_session_public_key_b64"]
        .as_str()
        .ok_or("decrypt-provider did not publish a session key (build --features rail-material)")?
        .to_string();
    let session_pub = B64.decode(&session_pub_b64).map_err(|e| e.to_string())?;
    step(5, "decrypt-provider: trusts authority; minted + published an in-sandbox session key");

    // --- authority: recover the escrowed CEK + re-seal it to the published key. ------
    let content_hash = b"producer-smoke-content-hash-0001".to_vec(); // 32 bytes
    let nonce = b"producer-smoke-nonce-1".to_vec();
    let aad = transcript_aad(&session_pub, &content_hash, &nonce);

    let release = ok_data(
        &key.call(&json!({
            "op": "release_from_escrow_ref",
            "request": key_release_request(),
            "decrypt_session_pub_b64": session_pub_b64,
            "wrapped_cek_b64": wrapped_cek_b64,
            "producer_vk_b64": producer_vk_b64,
            "kid_hex": kid_hex,
            "scheme": scheme,
            "aad_b64": B64.encode(&aad),
            "ciphertext_b64": segment_b64,
            "content_hash_b64": B64.encode(&content_hash),
            "nonce_b64": B64.encode(&nonce),
        }))?,
        "key release_from_escrow_ref",
    )?;
    let material = release["material"].clone();
    if material["sealed_cek_b64"].as_str().unwrap_or_default().is_empty() {
        return Err(format!("key-provider returned no sealed material: {release}"));
    }
    // Containment on the key wire: neither the producer's escrow blob nor any raw CEK
    // is echoed back (the only CEK-bearing field is the freshly re-sealed envelope).
    let release_str = serde_json::to_string(&release).map_err(|e| e.to_string())?;
    if release_str.contains(&wrapped_cek_b64) {
        return Err("the producer escrow blob was echoed by the key authority".to_string());
    }
    key.shutdown();
    step(6, "key-provider: recovered the escrowed CEK + re-sealed it to the session (no raw CEK, no escrow echo)");

    // --- decrypt: push the sealed material in, unwrap in-VM, decrypt the fresh segment.
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
    // Containment on the decrypt wire: the fresh plaintext never leaves the sandbox.
    let open_str = serde_json::to_string(&open).map_err(|e| e.to_string())?;
    if open_str.contains(&B64.encode(PLAINTEXT)) {
        return Err("plaintext leaked in the decrypt-provider response".to_string());
    }
    decrypt.shutdown();
    step(7, "decrypt-provider: unwrapped the re-sealed CEK in-VM and decrypted the freshly-sealed segment");

    println!();
    println!("RESULT: a CEK minted, escrowed, recovered, re-sealed and used to decrypt — all in this run.");
    println!("        No raw CEK and no plaintext ever crossed a process boundary. No golden.");
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("ddrm-producer-smoke: {e}");
            std::process::exit(1);
        }
    }
}
