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
fn transcript_aad(
    session_pub: &[u8],
    content_hash: &[u8],
    nonce: &[u8],
    node_set_id: Option<&[u8]>,
) -> Vec<u8> {
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
        // On the LIVE quorum rail this binds the 2-of-3 node-set identity into the transcript
        // (the decrypt boundary recomputes the identical AAD from the material); `None` keeps
        // the single-node rail byte-identical to the pre-threshold transcript.
        node_set_id,
    }
    .to_aad()
}

fn step(n: u32, msg: &str) {
    println!("  [{n}] {msg}");
}

// --- RENDER proof helpers (Day 139): the consumer-open path that returns CLEARTEXT ----
// The decrypt boundary's `stream_segment` op returns the decrypted segment for the scoped
// `stream` output kind. The seal CEK is bound to the decrypt transcript, so the RENDER open
// must rebuild the SAME AAD the counts open used — but with action/output_kind = "stream"
// (a viewer read), and the key authority re-seals to THAT transcript. These mirror the
// `view`/`rendered` builders above with the stream-flavoured action/viewer/output kind.
const ACTION_STREAM: &str = "stream";
const VIEWER_STREAM: &str = "media";
const OUTPUT_STREAM: &str = "stream";

fn decrypt_request_stream() -> Value {
    json!({
        "schema": "elastos.decrypt.session.request/v1",
        "request_id": "decrypt:producer-smoke",
        "principal_id": PRINCIPAL,
        "session_id": SESSION,
        "object_cid": OBJECT_CID,
        "action": ACTION_STREAM,
        "viewer_interface": VIEWER_STREAM,
        "release_receipt": {
            "schema": RR_SCHEMA,
            "request_id": RR_REQUEST_ID,
            "object_cid": OBJECT_CID,
            "principal_id": PRINCIPAL,
            "session_id": SESSION,
            "action": ACTION_STREAM,
            "provider": RR_PROVIDER,
            "status": RR_STATUS,
            "issued_at": RR_ISSUED_AT,
            "expires_at": EXPIRES_AT,
        },
        "output_kind": OUTPUT_STREAM,
        "reason": "render content sealed in this run",
        "expires_at": EXPIRES_AT,
    })
}

fn fallback_rights_receipt_stream() -> Value {
    json!({
        "schema": "elastos.rights.decision.receipt/v1",
        "request_id": "rights:producer-smoke",
        "content_id": OBJECT_CID,
        "principal_id": PRINCIPAL,
        "session_id": SESSION,
        "right": ACTION_STREAM,
        "provider": "rights-provider",
        "allowed": true,
        "issued_at": RR_ISSUED_AT,
        "expires_at": EXPIRES_AT,
    })
}

fn key_release_request_live_stream(kid_hex: &str, wrapped_cek_b64: &str, scheme: &str) -> Value {
    json!({
        "schema": "elastos.key_release.request/v1",
        "request_id": RR_REQUEST_ID,
        "principal_id": PRINCIPAL,
        "session_id": SESSION,
        "object_cid": OBJECT_CID,
        "action": ACTION_STREAM,
        "rights_receipt": fallback_rights_receipt_stream(),
        "key_envelope": {
            "scheme": scheme,
            "kid": kid_hex,
            "wrapped_cek": wrapped_cek_b64,
            "policy_hash": "sha256:producer-live",
            "algorithms": {
                "cipher": "aes-256-gcm",
                "signature": ["ed25519", "ml-dsa-65"],
                "kem": ["x25519", "ml-kem-768"],
                "share_scheme": "shamir-t-of-n",
            },
        },
        "reason": "render content sealed in this run",
        "expires_at": EXPIRES_AT,
    })
}

/// Rebuild the decrypt-transcript AAD for the STREAM (render) open — identical to
/// [`transcript_aad`] but bound to the stream action/viewer/output kind.
fn transcript_aad_stream(
    session_pub: &[u8],
    content_hash: &[u8],
    nonce: &[u8],
    node_set_id: Option<&[u8]>,
) -> Vec<u8> {
    let receipt_hash = release_receipt_hash(
        RR_SCHEMA,
        RR_REQUEST_ID,
        OBJECT_CID,
        PRINCIPAL,
        SESSION,
        ACTION_STREAM,
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
        action: ACTION_STREAM,
        viewer_interface: VIEWER_STREAM,
        output_kind: OUTPUT_STREAM,
        expires_at: EXPIRES_AT,
        release_receipt_hash: receipt_hash,
        decrypt_session_pub: session_pub,
        nonce,
        node_set_id,
    }
    .to_aad()
}

/// Extract the `mdat` box payload from a single-sample fMP4 segment. The producer's
/// `mux_single_sample_segment` lays out `moof{…} + mdat{ciphertext}`; with full-sample
/// AES-CTR (clear_leader == 0) the DECRYPTED mdat payload is byte-identical to the
/// original asset bytes. Walks top-level boxes ([u32 size][4 type][payload]); handles
/// 64-bit largesize (1) and to-EOF (0) defensively.
fn extract_mdat(seg: &[u8]) -> Result<Vec<u8>, String> {
    let mut off = 0usize;
    while off + 8 <= seg.len() {
        let size = u32::from_be_bytes([seg[off], seg[off + 1], seg[off + 2], seg[off + 3]]) as usize;
        let typ = &seg[off + 4..off + 8];
        let (payload_start, box_end) = if size == 1 {
            if off + 16 > seg.len() {
                return Err("truncated 64-bit box header".into());
            }
            let large = u64::from_be_bytes(seg[off + 8..off + 16].try_into().unwrap()) as usize;
            (off + 16, off.checked_add(large).ok_or("box size overflow")?)
        } else if size == 0 {
            (off + 8, seg.len())
        } else {
            (off + 8, off.checked_add(size).ok_or("box size overflow")?)
        };
        if box_end > seg.len() || box_end < payload_start {
            return Err("box exceeds segment bounds".into());
        }
        if typ == b"mdat" {
            return Ok(seg[payload_start..box_end].to_vec());
        }
        off = box_end;
    }
    Err("no mdat box found in the decrypted segment".into())
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
    let aad = transcript_aad(&session_pub, &content_hash, &nonce, None);

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

/// A complete `KeyReleaseRequestV1` carrying the REAL minted KID + the producer's escrowed
/// share-1 (the live quorum recover keys on these). Mirrors [`key_release_request`] but with
/// the values seal_inline_threshold actually produced.
fn key_release_request_live(kid_hex: &str, wrapped_cek_b64: &str, scheme: &str) -> Value {
    json!({
        "schema": "elastos.key_release.request/v1",
        "request_id": RR_REQUEST_ID,
        "principal_id": PRINCIPAL,
        "session_id": SESSION,
        "object_cid": OBJECT_CID,
        "action": ACTION,
        "rights_receipt": fallback_rights_receipt(),
        "key_envelope": {
            "scheme": scheme,
            "kid": kid_hex,
            "wrapped_cek": wrapped_cek_b64,
            "policy_hash": "sha256:producer-live",
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

/// LIVE producer vertical: mint a CEK NOW, escrow it to the REAL 2-of-3 dKMS quorum (the
/// shares sealed to the nodes' published recipients, the CEK never assembled in the producer
/// boundary), then drive the REAL `key-provider` (dkms backend) to recover 2-of-3 from the
/// LIVE nodes over the authenticated PQ channel and re-seal to the decrypt boundary, which
/// decrypts the segment sealed in THIS run. The runtime spawns NO daemons and performs NO
/// destructive op — it only READS the public descriptor and uses the allow-listed caller seed.
fn run_live(
    encrypt_bin: &str,
    key_bin: &str,
    decrypt_bin: &str,
    descriptor_path: &str,
    caller_seed_b64: &str,
) -> Result<(), String> {
    println!("== dDRM producer-half LIVE smoke (encrypt[threshold-escrow] -> key[dkms 2-of-3 recover] -> decrypt) ==");

    // --- read the PUBLIC-ONLY dkms descriptor: the 3 quorum nodes, in node-set order. ---
    let desc: Value = serde_json::from_slice(
        &std::fs::read(descriptor_path).map_err(|e| format!("read descriptor {descriptor_path}: {e}"))?,
    )
    .map_err(|e| format!("parse descriptor: {e}"))?;
    if desc.get("authority_master_seed_b64").is_some() {
        return Err("descriptor carries a master seed — it must be PUBLIC-ONLY (the secret stays in the node)".to_string());
    }
    let nodes_v = desc
        .get("threshold")
        .and_then(|t| t.get("nodes"))
        .and_then(Value::as_array)
        .ok_or("descriptor has no threshold.nodes array")?;
    if nodes_v.len() != 3 {
        return Err(format!(
            "the live producer vertical requires a 3-node 2-of-3 descriptor; got {}",
            nodes_v.len()
        ));
    }
    let mut node_vks: Vec<String> = Vec::with_capacity(3);
    let mut node_json: Vec<Value> = Vec::with_capacity(3);
    for (i, n) in nodes_v.iter().enumerate() {
        let vk = n
            .get("verifying_key_b64")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("node {i} missing verifying_key_b64"))?
            .to_string();
        let recipient = n
            .get("recipient_pub_b64")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("node {i} missing recipient_pub_b64"))?
            .to_string();
        node_json.push(json!({ "verifying_key_b64": vk, "recipient_pub_b64": recipient }));
        node_vks.push(vk);
    }
    step(1, "descriptor: PUBLIC-ONLY 2-of-3 dkms quorum read (3 node identities + recipients, in node-set order)");

    // --- producer: mint a CEK NOW, CENC-encrypt, SPLIT + escrow each share to its LIVE node. ---
    let mut encrypt = Capsule::spawn("encrypt-provider", encrypt_bin)?;
    let enc_init = ok_data(&encrypt.call(&json!({ "op": "init" }))?, "encrypt init")?;
    let producer_vk_b64 = enc_init["producer_verifying_key_b64"]
        .as_str()
        .ok_or("encrypt-provider published no producer vk (build --features escrow)")?
        .to_string();
    let sealed = ok_data(
        &encrypt.call(&json!({
            "op": "seal_inline_threshold",
            "plaintext_b64": B64.encode(PLAINTEXT),
            "nodes": node_json,
        }))?,
        "encrypt seal_inline_threshold",
    )?;
    encrypt.shutdown();

    let kid_hex = sealed["kid_hex"].as_str().ok_or("no kid_hex")?.to_string();
    let scheme = sealed["scheme"].as_str().ok_or("no scheme")?.to_string();
    let segment_b64 = sealed["segment_b64"].as_str().ok_or("no segment")?.to_string();
    let node_set_id_b64 = sealed["node_set_id_b64"].as_str().ok_or("no node_set_id")?.to_string();
    let node_set_id = B64.decode(&node_set_id_b64).map_err(|e| e.to_string())?;
    // PRE-AUDIT #1: the producer now PUBLISHES a CEK commitment in the seal — forward it so the
    // decrypt boundary verifies the reconstructed CEK against it (alongside 3-share cheater detection).
    let cek_commitment_b64 = sealed["cek_commitment_b64"]
        .as_str()
        .ok_or("seal did not publish a cek_commitment_b64 (pre-audit #1)")?
        .to_string();
    let shares = sealed["shares"].as_array().ok_or("no shares")?;
    if shares.len() != 3 {
        return Err("expected 3 sealed shares".to_string());
    }
    // shares are ordered x=1,2,3 == node1,node2,node3 (the descriptor's node-set order).
    let share_wrapped = |i: usize| -> Result<String, String> {
        shares[i]["wrapped_share_b64"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("share {i} missing wrapped_share_b64"))
    };
    let share1 = share_wrapped(0)?;
    let share2 = share_wrapped(1)?;
    let share3 = share_wrapped(2)?;
    if serde_json::to_string(&sealed).unwrap_or_default().contains(&B64.encode(PLAINTEXT)) {
        return Err("plaintext leaked in the seal_inline_threshold response".to_string());
    }
    step(2, &format!(
        "encrypt-provider: minted CEK in-boundary, CENC-encrypted, SHAMIR-split + sealed 3 shares to the live nodes; kid={kid_hex} (CEK never whole, no plaintext on the wire)"
    ));

    // --- decrypt boundary: pin ALL THREE node identities, mint + publish a session key. ---
    let mut decrypt = Capsule::spawn("decrypt-provider", decrypt_bin)?;
    let dec_init = ok_data(
        &decrypt.call(&json!({
            "op": "init",
            "config": {
                "authority_vk_b64": node_vks[0],
                "authority_vk2_b64": node_vks[1],
                "authority_vk3_b64": node_vks[2],
            }
        }))?,
        "decrypt init",
    )?;
    let session_pub_b64 = dec_init["decrypt_session_public_key_b64"]
        .as_str()
        .ok_or("decrypt-provider published no session key (build --features rail-material)")?
        .to_string();
    let session_pub = B64.decode(&session_pub_b64).map_err(|e| e.to_string())?;
    step(3, "decrypt-provider: pinned all 3 node identities; minted + published an in-sandbox session key");

    // --- key-provider (dkms backend): recover 2-of-3 from the LIVE nodes + re-seal. ---
    let content_hash = b"producer-live-content-hash-0001".to_vec(); // 32 bytes
    let nonce = b"producer-live-nonce-1".to_vec();
    let aad = transcript_aad(&session_pub, &content_hash, &nonce, Some(&node_set_id));

    let mut key = Capsule::spawn("key-provider", key_bin)?;
    ok_data(
        &key.call(&json!({
            "op": "init",
            "config": {
                "backend": "dkms",
                "dkms_authority_descriptor": descriptor_path,
                "dkms_caller_seed_b64": caller_seed_b64,
            }
        }))?,
        "key init (dkms)",
    )?;
    let request = key_release_request_live(&kid_hex, &share1, &scheme);
    let session_ctx = json!({
        "decrypt_session_pub_b64": session_pub_b64,
        // All three shares were signed by the SAME in-boundary producer identity.
        "producer_vk_b64": producer_vk_b64,
        "producer_vk2_b64": producer_vk_b64,
        "producer_vk3_b64": producer_vk_b64,
        "aad_b64": B64.encode(&aad),
        "ciphertext_b64": segment_b64,
        "content_hash_b64": B64.encode(&content_hash),
        "nonce_b64": B64.encode(&nonce),
        "wrapped_cek_share2_b64": share2,
        "wrapped_cek_share3_b64": share3,
        "cek_commitment_b64": cek_commitment_b64,
        "now_unix": NOW_UNIX,
    });
    let release = ok_data(
        &key.call(&json!({ "op": "release", "request": request, "session": session_ctx }))?,
        "key release (dkms 2-of-3)",
    )?;
    key.shutdown();
    let material = release["material"].clone();
    if material["sealed_cek_b64"].as_str().unwrap_or_default().is_empty() {
        return Err(format!("key-provider returned no sealed material: {release}"));
    }
    let release_str = serde_json::to_string(&release).map_err(|e| e.to_string())?;
    if release_str.contains(&share1) || release_str.contains(&share2) || release_str.contains(&share3) {
        return Err("an escrowed share blob was echoed by the key authority".to_string());
    }
    step(4, "key-provider(dkms): recovered the CEK from ANY TWO live nodes over the authenticated PQ channel + re-sealed it to the session (no raw CEK, no escrow echo; no single node ever saw the whole key)");

    // --- decrypt: reconstruct the CEK in-VM from the 2-of-3 re-sealed shares + decrypt. ---
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
    if open_data["session"]["sample_count"].as_u64() != Some(EXPECTED_SAMPLE_COUNT) {
        return Err(format!("decrypt sample_count mismatch: {open}"));
    }
    if serde_json::to_string(&open).unwrap_or_default().contains(&B64.encode(PLAINTEXT)) {
        return Err("plaintext leaked in the decrypt-provider response".to_string());
    }
    decrypt.shutdown();
    step(5, "decrypt-provider: reconstructed the CEK in-VM from the 2-of-3 re-sealed shares and DECRYPTED the freshly-sealed segment");

    println!();
    println!("RESULT: a CEK minted NOW, escrowed to the LIVE 2-of-3 dKMS quorum, recovered 2-of-3, re-sealed and used to decrypt the segment sealed in THIS run.");
    println!("        No raw CEK, no share blob, and no plaintext crossed a process boundary. No golden, no Lit.");
    Ok(())
}

/// ASSET open vertical: prove that a REAL asset file, sealed to the 2-of-3 quorum and whose
/// escrow descriptor is PERSISTED TO DISK (the mint-time `protections[0]` envelope), can later
/// be reloaded FROM DISK ALONE and opened — the CEK recovered 2-of-3 from the live nodes and
/// the segment decrypted. This is the runtime's Library-open path distilled to its crypto core:
///
///   phase A (mint): seal_inline_threshold(real bytes) -> write { escrow.json, ciphertext.bin }
///   phase B (open): reload escrow.json + ciphertext.bin FROM DISK -> key[dkms 2-of-3 recover]
///                   -> decrypt[open_session_v1] (CEK reconstructed in-VM, segment decrypted)
///
/// Between the phases EVERY in-memory copy of the seal output is dropped: phase B reads only the
/// persisted sidecar, exactly as `/api/viewers/open` will read a stored owned object. This is the
/// proof the keystone fix made assets RECOVERABLE — `producer_verifying_key_b64` now lives in the
/// persisted escrow, so the quorum can validate the share signatures with nothing held in process.
fn run_asset(
    encrypt_bin: &str,
    key_bin: &str,
    decrypt_bin: &str,
    input_path: &str,
    descriptor_path: &str,
    caller_seed_b64: &str,
) -> Result<(), String> {
    println!("== dDRM ASSET open vertical (seal real file -> PERSIST escrow -> reload from disk -> 2-of-3 recover -> decrypt) ==");

    let plaintext = std::fs::read(input_path)
        .map_err(|e| format!("read input asset {input_path}: {e}"))?;
    if plaintext.is_empty() {
        return Err(format!("input asset {input_path} is empty"));
    }
    let plaintext_b64 = B64.encode(&plaintext);

    // --- read the PUBLIC-ONLY dkms descriptor (the 3 quorum nodes, in node-set order). ---
    let desc: Value = serde_json::from_slice(
        &std::fs::read(descriptor_path)
            .map_err(|e| format!("read descriptor {descriptor_path}: {e}"))?,
    )
    .map_err(|e| format!("parse descriptor: {e}"))?;
    if desc.get("authority_master_seed_b64").is_some() {
        return Err("descriptor carries a master seed — it must be PUBLIC-ONLY".to_string());
    }
    let nodes_v = desc
        .get("threshold")
        .and_then(|t| t.get("nodes"))
        .and_then(Value::as_array)
        .ok_or("descriptor has no threshold.nodes array")?;
    if nodes_v.len() != 3 {
        return Err(format!("asset open requires a 3-node 2-of-3 descriptor; got {}", nodes_v.len()));
    }
    let mut node_vks: Vec<String> = Vec::with_capacity(3);
    let mut node_json: Vec<Value> = Vec::with_capacity(3);
    for (i, n) in nodes_v.iter().enumerate() {
        let vk = n.get("verifying_key_b64").and_then(Value::as_str)
            .ok_or_else(|| format!("node {i} missing verifying_key_b64"))?.to_string();
        let recipient = n.get("recipient_pub_b64").and_then(Value::as_str)
            .ok_or_else(|| format!("node {i} missing recipient_pub_b64"))?.to_string();
        node_json.push(json!({ "verifying_key_b64": vk, "recipient_pub_b64": recipient }));
        node_vks.push(vk);
    }
    step(1, &format!("read {}-byte asset + PUBLIC-ONLY 2-of-3 descriptor", plaintext.len()));

    // === PHASE A — MINT: seal the real bytes to the quorum + PERSIST the escrow sidecar. ===
    let uniq = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let escrow_dir = std::env::temp_dir().join(format!(
        "elastos-asset-escrow-{}-{}",
        std::process::id(),
        uniq
    ));
    std::fs::create_dir_all(&escrow_dir).map_err(|e| format!("mk escrow dir: {e}"))?;
    let escrow_json_path = escrow_dir.join("escrow.json");
    let ciphertext_path = escrow_dir.join("ciphertext.bin");
    {
        let mut encrypt = Capsule::spawn("encrypt-provider", encrypt_bin)?;
        let enc_init = ok_data(&encrypt.call(&json!({ "op": "init" }))?, "encrypt init")?;
        let producer_vk_b64 = enc_init["producer_verifying_key_b64"].as_str()
            .ok_or("encrypt-provider published no producer vk (build --features escrow)")?.to_string();
        let sealed = ok_data(
            &encrypt.call(&json!({
                "op": "seal_inline_threshold",
                "plaintext_b64": plaintext_b64,
                "nodes": node_json,
            }))?,
            "encrypt seal_inline_threshold",
        )?;
        encrypt.shutdown();

        let kid_hex = sealed["kid_hex"].as_str().ok_or("no kid_hex")?.to_string();
        let scheme = sealed["scheme"].as_str().ok_or("no scheme")?.to_string();
        let segment_b64 = sealed["segment_b64"].as_str().ok_or("no segment")?.to_string();
        let node_set_id_b64 = sealed["node_set_id_b64"].as_str().ok_or("no node_set_id")?.to_string();
        let shares = sealed["shares"].as_array().ok_or("no shares")?;
        if shares.len() != 3 {
            return Err("expected 3 sealed shares".to_string());
        }
        if serde_json::to_string(&sealed).unwrap_or_default().contains(&plaintext_b64) {
            return Err("plaintext leaked in the seal_inline_threshold response".to_string());
        }

        // The PERSISTED escrow sidecar — the exact `protections[0]` shape the mint now writes.
        let escrow = json!({
            "schema": "elastos.ddrm.escrow/v1",
            "scheme": scheme,
            "kid_hex": kid_hex,
            "node_set_id_b64": node_set_id_b64,
            "producer_verifying_key_b64": producer_vk_b64,
            "shares": shares,
        });
        std::fs::write(&escrow_json_path, serde_json::to_vec_pretty(&escrow).unwrap())
            .map_err(|e| format!("persist escrow.json: {e}"))?;
        let segment_bytes = B64.decode(&segment_b64).map_err(|e| e.to_string())?;
        std::fs::write(&ciphertext_path, &segment_bytes)
            .map_err(|e| format!("persist ciphertext.bin: {e}"))?;

        step(2, &format!(
            "MINT: sealed {}B asset to the quorum; PERSISTED escrow.json ({}B) + ciphertext.bin ({}B); kid={kid_hex}",
            plaintext.len(),
            std::fs::metadata(&escrow_json_path).map(|m| m.len()).unwrap_or(0),
            segment_bytes.len(),
        ));
        // All seal outputs (kid_hex, shares, producer_vk, segment) go out of scope HERE.
    }

    // === PHASE B — OPEN: reload FROM DISK ONLY, recover 2-of-3, decrypt. ===
    let escrow: Value = serde_json::from_slice(
        &std::fs::read(&escrow_json_path).map_err(|e| format!("reload escrow.json: {e}"))?,
    )
    .map_err(|e| format!("parse persisted escrow: {e}"))?;
    let segment_bytes = std::fs::read(&ciphertext_path).map_err(|e| format!("reload ciphertext.bin: {e}"))?;
    let segment_b64 = B64.encode(&segment_bytes);

    let kid_hex = escrow["kid_hex"].as_str().ok_or("persisted escrow missing kid_hex")?.to_string();
    let scheme = escrow["scheme"].as_str().ok_or("persisted escrow missing scheme")?.to_string();
    let node_set_id_b64 = escrow["node_set_id_b64"].as_str().ok_or("persisted escrow missing node_set_id_b64")?.to_string();
    let node_set_id = B64.decode(&node_set_id_b64).map_err(|e| e.to_string())?;
    // PRE-AUDIT #1: the persisted escrow carries the producer's published CEK commitment; forward it.
    let cek_commitment_b64 = escrow["cek_commitment_b64"].as_str()
        .ok_or("persisted escrow missing cek_commitment_b64 (pre-audit #1)")?
        .to_string();
    let producer_vk_b64 = escrow["producer_verifying_key_b64"].as_str()
        .ok_or("persisted escrow missing producer_verifying_key_b64 — asset is UNRECOVERABLE (pre-keystone-fix mint)")?
        .to_string();
    let shares = escrow["shares"].as_array().ok_or("persisted escrow missing shares")?;
    let share_wrapped = |i: usize| -> Result<String, String> {
        shares[i]["wrapped_share_b64"].as_str().map(str::to_string)
            .ok_or_else(|| format!("persisted share {i} missing wrapped_share_b64"))
    };
    let share1 = share_wrapped(0)?;
    let share2 = share_wrapped(1)?;
    let share3 = share_wrapped(2)?;
    step(3, "OPEN: reloaded escrow.json + ciphertext.bin FROM DISK (nothing held in process from the mint)");

    // decrypt boundary: pin all 3 node identities + mint a session key.
    let mut decrypt = Capsule::spawn("decrypt-provider", decrypt_bin)?;
    let dec_init = ok_data(
        &decrypt.call(&json!({
            "op": "init",
            "config": {
                "authority_vk_b64": node_vks[0],
                "authority_vk2_b64": node_vks[1],
                "authority_vk3_b64": node_vks[2],
            }
        }))?,
        "decrypt init",
    )?;
    let session_pub_b64 = dec_init["decrypt_session_public_key_b64"].as_str()
        .ok_or("decrypt-provider published no session key (build --features rail-material)")?.to_string();
    let session_pub = B64.decode(&session_pub_b64).map_err(|e| e.to_string())?;

    // Fresh per-open session binding (content_hash + nonce bind the re-seal transcript; both the
    // key authority's re-seal AAD and the decrypt boundary's unwrap AAD recompute the same value).
    let content_hash = b"elastos-asset-open-content-hash0".to_vec(); // 32 bytes
    let nonce = b"elastos-asset-open-nonce".to_vec();
    let aad = transcript_aad(&session_pub, &content_hash, &nonce, Some(&node_set_id));

    let mut key = Capsule::spawn("key-provider", key_bin)?;
    ok_data(
        &key.call(&json!({
            "op": "init",
            "config": {
                "backend": "dkms",
                "dkms_authority_descriptor": descriptor_path,
                "dkms_caller_seed_b64": caller_seed_b64,
            }
        }))?,
        "key init (dkms)",
    )?;
    let request = key_release_request_live(&kid_hex, &share1, &scheme);
    let session_ctx = json!({
        "decrypt_session_pub_b64": session_pub_b64,
        "producer_vk_b64": producer_vk_b64,
        "producer_vk2_b64": producer_vk_b64,
        "producer_vk3_b64": producer_vk_b64,
        "aad_b64": B64.encode(&aad),
        "ciphertext_b64": segment_b64,
        "content_hash_b64": B64.encode(&content_hash),
        "nonce_b64": B64.encode(&nonce),
        "wrapped_cek_share2_b64": share2,
        "wrapped_cek_share3_b64": share3,
        "cek_commitment_b64": cek_commitment_b64,
        "now_unix": NOW_UNIX,
    });
    let release = ok_data(
        &key.call(&json!({ "op": "release", "request": request, "session": session_ctx }))?,
        "key release (dkms 2-of-3)",
    )?;
    let material = release["material"].clone();
    if material["sealed_cek_b64"].as_str().unwrap_or_default().is_empty() {
        return Err(format!("key-provider returned no sealed material: {release}"));
    }
    let release_str = serde_json::to_string(&release).map_err(|e| e.to_string())?;
    if release_str.contains(&share1) || release_str.contains(&share2) || release_str.contains(&share3) {
        return Err("an escrowed share blob was echoed by the key authority".to_string());
    }
    step(4, "key-provider(dkms): recovered the CEK from ANY TWO live nodes using ONLY the persisted escrow + re-sealed to the session");

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
    if serde_json::to_string(&open).unwrap_or_default().contains(&plaintext_b64) {
        return Err("plaintext leaked in the decrypt-provider response".to_string());
    }
    step(5, "decrypt-provider: reconstructed the CEK in-VM from the persisted 2-of-3 escrow and DECRYPTED the asset segment (no plaintext on the wire)");

    // === RENDER — the consumer-open path that hands a viewer the CLEARTEXT bytes. ===
    // A SECOND release bound to the STREAM transcript (action/output_kind = "stream"), so the
    // decrypt boundary's `stream_segment` op — which recomputes the AAD from the stream request
    // — opens the SAME re-sealed CEK and returns the decrypted segment. We then strip the fMP4
    // wrapper (extract the mdat payload) and prove it is BYTE-IDENTICAL to the original asset.
    let nonce_stream = b"elastos-asset-render-nonce".to_vec();
    let aad_stream = transcript_aad_stream(&session_pub, &content_hash, &nonce_stream, Some(&node_set_id));
    let request_stream = key_release_request_live_stream(&kid_hex, &share1, &scheme);
    let session_ctx_stream = json!({
        "decrypt_session_pub_b64": session_pub_b64,
        "producer_vk_b64": producer_vk_b64,
        "producer_vk2_b64": producer_vk_b64,
        "producer_vk3_b64": producer_vk_b64,
        "aad_b64": B64.encode(&aad_stream),
        "ciphertext_b64": segment_b64,
        "content_hash_b64": B64.encode(&content_hash),
        "nonce_b64": B64.encode(&nonce_stream),
        "wrapped_cek_share2_b64": share2,
        "wrapped_cek_share3_b64": share3,
        "cek_commitment_b64": cek_commitment_b64,
        "now_unix": NOW_UNIX,
    });
    let release_stream = ok_data(
        &key.call(&json!({ "op": "release", "request": request_stream, "session": session_ctx_stream }))?,
        "key release (dkms 2-of-3, stream)",
    )?;
    key.shutdown();
    let material_stream = release_stream["material"].clone();

    let rendered = decrypt.call(&json!({
        "op": "stream_segment",
        "request": decrypt_request_stream(),
        "material": material_stream,
        "index": 0,
        "now_unix": NOW_UNIX,
    }))?;
    let rendered_data = ok_data(&rendered, "decrypt stream_segment (quorum render)")?;
    let segment_out_b64 = rendered_data["segment_b64"]
        .as_str()
        .ok_or_else(|| format!("stream_segment returned no segment_b64: {rendered}"))?;
    let segment_out = B64.decode(segment_out_b64).map_err(|e| e.to_string())?;
    let recovered = extract_mdat(&segment_out)?;
    if recovered != plaintext {
        return Err(format!(
            "RENDER MISMATCH: recovered {} bytes != original {} bytes — the quorum-decrypted asset is not byte-identical",
            recovered.len(),
            plaintext.len()
        ));
    }
    decrypt.shutdown();
    let _ = std::fs::remove_dir_all(&escrow_dir);
    step(6, &format!(
        "decrypt-provider(stream): rendered the asset via the 2-of-3 quorum and recovered {} bytes BYTE-IDENTICAL to the original (CEK never left the VM)",
        recovered.len()
    ));

    println!();
    println!("RESULT: a REAL {}-byte asset was sealed to the live 2-of-3 quorum, its escrow PERSISTED to disk,", plaintext.len());
    println!("        reloaded FROM DISK ALONE, recovered 2-of-3, decrypted, and RENDERED byte-identical — the full");
    println!("        consumer-open path (no Lit, no plaintext/CEK/share on any wire). Minted assets are RECOVERABLE + RENDERABLE.");
    Ok(())
}

/// MINT-CAPSULE: seal a real asset to the quorum and write the `.ddrm` capsule EXACTLY as the
/// gateway's `persist_minted_asset_to_library` does — `protections[0]` escrow (scheme,
/// node_set_id_b64, producer_verifying_key_b64, shares[3]) + `kid` + the persisted
/// `ciphertext_b64`. This is the artifact `ddrm-media-authority --quorum` opens, so the helper
/// can be verified against a capsule with the production shape (not a bespoke fixture).
fn run_mint_capsule(
    encrypt_bin: &str,
    input_path: &str,
    descriptor_path: &str,
    out_capsule_path: &str,
) -> Result<(), String> {
    let plaintext =
        std::fs::read(input_path).map_err(|e| format!("read input asset {input_path}: {e}"))?;
    if plaintext.is_empty() {
        return Err(format!("input asset {input_path} is empty"));
    }
    let desc: Value = serde_json::from_slice(
        &std::fs::read(descriptor_path).map_err(|e| format!("read descriptor: {e}"))?,
    )
    .map_err(|e| format!("parse descriptor: {e}"))?;
    let nodes_v = desc
        .get("threshold")
        .and_then(|t| t.get("nodes"))
        .and_then(Value::as_array)
        .ok_or("descriptor has no threshold.nodes array")?;
    let node_json: Vec<Value> = nodes_v
        .iter()
        .map(|n| {
            json!({
                "verifying_key_b64": n.get("verifying_key_b64"),
                "recipient_pub_b64": n.get("recipient_pub_b64"),
            })
        })
        .collect();

    let mut encrypt = Capsule::spawn("encrypt-provider", encrypt_bin)?;
    let enc_init = ok_data(&encrypt.call(&json!({ "op": "init" }))?, "encrypt init")?;
    let producer_vk_b64 = enc_init["producer_verifying_key_b64"]
        .as_str()
        .ok_or("encrypt-provider published no producer vk")?
        .to_string();
    let sealed = ok_data(
        &encrypt.call(&json!({
            "op": "seal_inline_threshold",
            "plaintext_b64": B64.encode(&plaintext),
            "nodes": node_json,
        }))?,
        "encrypt seal_inline_threshold",
    )?;
    encrypt.shutdown();

    let kid_hex = sealed["kid_hex"].as_str().ok_or("no kid_hex")?;
    let scheme = sealed["scheme"].as_str().ok_or("no scheme")?;
    let segment_b64 = sealed["segment_b64"].as_str().ok_or("no segment")?;
    let node_set_id_b64 = sealed["node_set_id_b64"].as_str().ok_or("no node_set_id")?;
    let shares = sealed["shares"].as_array().ok_or("no shares")?;

    // The exact `.ddrm` capsule shape `persist_minted_asset_to_library` writes (+ ciphertext_b64).
    let capsule = json!({
        "schema": "elastos.ddrm.capsule/v1",
        "title": "verify-asset",
        "mime": "application/octet-stream",
        "is_media": false,
        "kid": kid_hex,
        "content_id": kid_hex,
        "ciphertext_b64": segment_b64,
        "protections": [{
            "algorithm": "aes-128",
            "scheme": scheme,
            "node_set_id_b64": node_set_id_b64,
            "producer_verifying_key_b64": producer_vk_b64,
            "shares": shares,
        }],
    });
    std::fs::write(out_capsule_path, serde_json::to_vec_pretty(&capsule).unwrap())
        .map_err(|e| format!("write capsule: {e}"))?;
    println!(
        "mint-capsule: sealed {}B asset -> {out_capsule_path} (kid={kid_hex})",
        plaintext.len()
    );
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = if let Some(pos) = args.iter().position(|a| a == "--mint-capsule") {
        let pre = &args[..pos];
        let post = &args[pos + 1..];
        if pre.is_empty() || post.len() < 3 {
            Err("usage: ddrm-producer-smoke <encrypt-bin> [..] --mint-capsule <input-file> <descriptor.json> <out.ddrm>".to_string())
        } else {
            run_mint_capsule(&pre[0], &post[0], &post[1], &post[2])
        }
    } else if let Some(pos) = args.iter().position(|a| a == "--asset") {
        let pre = &args[..pos];
        let post = &args[pos + 1..];
        if pre.len() < 3 || post.len() < 3 {
            Err("usage: ddrm-producer-smoke <encrypt-bin> <key-bin> <decrypt-bin> --asset <input-file> <descriptor.json> <caller_seed_b64>".to_string())
        } else {
            run_asset(&pre[0], &pre[1], &pre[2], &post[0], &post[1], &post[2])
        }
    } else if let Some(pos) = args.iter().position(|a| a == "--live") {
        let pre = &args[..pos];
        let post = &args[pos + 1..];
        if pre.len() < 3 || post.len() < 2 {
            Err("usage: ddrm-producer-smoke <encrypt-bin> <key-bin> <decrypt-bin> --live <descriptor.json> <caller_seed_b64>".to_string())
        } else {
            run_live(&pre[0], &pre[1], &pre[2], &post[0], &post[1])
        }
    } else {
        run(&args)
    };
    match result {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("ddrm-producer-smoke: {e}");
            std::process::exit(1);
        }
    }
}
