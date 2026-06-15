//! `--quorum` open mode — the dKMS consumer-open path for a minted `.ddrm` asset.
//!
//! This is the PRODUCTION analogue of the local-test-KMS `--object` mode, but instead of
//! re-sealing plaintext under a fresh local CEK it recovers the asset's REAL CEK from the
//! 2-of-3 dKMS quorum the asset was minted to. The exact orchestration is the one proven by
//! `ddrm-producer-smoke --asset` (seal -> persist -> reload -> 2-of-3 recover -> decrypt ->
//! render byte-identical); here it runs at OPEN time from the persisted `.ddrm` capsule:
//!
//!   1. read the `.ddrm` capsule: `protections[0]` escrow (scheme, kid, node_set_id,
//!      producer_verifying_key, 3 sealed shares) + the persisted `ciphertext_b64`.
//!   2. spawn a SEPARATE rail `decrypt-provider` boundary; pin the 3 node identities from the
//!      quorum descriptor; it mints + publishes an in-VM session key.
//!   3. spawn `key-provider` (dkms backend); recover the CEK from ANY TWO live nodes over the
//!      authenticated channel and re-seal it to the decrypt session, bound to a STREAM
//!      transcript AAD (computed ONCE with the shared `ddrm-envelope` encoder).
//!   4. `decrypt-provider stream_segment` reconstructs the CEK in-VM and returns the decrypted
//!      single-sample fMP4 segment; we strip the wrapper (extract the `mdat` payload) to get
//!      the byte-identical original asset.
//!
//! The CEK never leaves the decrypt VM unsealed; no share blob and no plaintext crosses the
//! gateway. The recovered object bytes are then served over the SAME stdio protocol the
//! `--object` mode uses (`{"op":"object"}` -> `{"status":"ok","object_b64":...}`), so the
//! gateway's object viewer is unchanged.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use ddrm_envelope::transcript::{release_receipt_hash, DecryptTranscriptV1};
use ddrm_envelope::SUITE_PQ_HYBRID;
use serde_json::{json, Value};

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

/// Inputs the gateway passes to a quorum open (all PUBLIC — no key material).
pub struct QuorumArgs {
    pub principal: String,
    pub decrypt_bin: String,
    pub key_bin: String,
    /// The `.ddrm` capsule file (carries `protections[0]` escrow + `ciphertext_b64`).
    pub capsule_path: String,
    /// The dKMS quorum descriptor (threshold.nodes[] + per-node authority_endpoint).
    pub descriptor_path: String,
    /// The allow-listed caller seed (b64) the dkms backend authenticates to the nodes with.
    pub caller_seed_b64: String,
    pub object_cid: String,
    pub mime: String,
    pub ttl_secs: u64,
    /// OPTIONAL wallet-signed access grant (base64 of the AccessGrantV1 JSON). When present the
    /// dkms nodes authorize TRUSTLESSLY (verify the wallet/session signatures + read
    /// `hasAccessByContentId` themselves) instead of trusting the unsigned rights receipt. The
    /// gateway builds it from the user's `personal_sign`; this helper only forwards it.
    pub access_grant_b64: Option<String>,
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
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

/// Run the dKMS `release` (2-of-3 recover + re-seal to the per-open decrypt session) against the WARM
/// key-provider daemon over its Unix socket when the gateway wired one up (Phase A — node handshake
/// sessions reused across opens, no per-open process spawn or init+hello), falling back to spawning a
/// fresh key-provider for this open when no socket is set OR the daemon is unreachable. The daemon is
/// pre-init'd, so the socket path sends ONLY the release; the request/response and the fail-closed
/// 2-of-3 recover are byte-identical on both transports. Returns the release `data` (`{ material, .. }`).
fn key_release(
    key_bin: &str,
    descriptor_path: &str,
    caller_seed_b64: &str,
    release_request: &Value,
    session_ctx: &Value,
) -> Result<Value, String> {
    let release_msg = json!({ "op": "release", "request": release_request, "session": session_ctx });

    if let Ok(sock) = std::env::var("ELASTOS_DDRM_KEY_PROVIDER_SOCKET") {
        if !sock.trim().is_empty() {
            match key_release_via_daemon(&sock, &release_msg) {
                Ok(resp) => return ok_data(&resp, "key release (dkms 2-of-3, warm daemon)"),
                Err(err) => eprintln!(
                    "ddrm-media-authority: warm key-provider daemon ({sock}) unavailable ({err}); \
                     falling back to per-open key-provider spawn"
                ),
            }
        }
    }

    // Fallback / default: a fresh key-provider for this single open (the original cold path).
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
    let release = ok_data(&key.call(&release_msg)?, "key release (dkms 2-of-3)")?;
    key.shutdown();
    Ok(release)
}

/// One framed request/response against the warm key-provider daemon's Unix socket (same newline-JSON
/// protocol as the stdio capsule). Any connect/transport error returns `Err` so `key_release` can fall
/// back to a per-open spawn — the daemon being down degrades latency, never access.
fn key_release_via_daemon(sock_path: &str, release_msg: &Value) -> Result<Value, String> {
    use std::os::unix::net::UnixStream;
    let stream =
        UnixStream::connect(sock_path).map_err(|e| format!("connect {sock_path}: {e}"))?;
    let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);
    let line = serde_json::to_string(release_msg).map_err(|e| e.to_string())?;
    writeln!(writer, "{line}").map_err(|e| format!("write to key-provider daemon: {e}"))?;
    writer.flush().map_err(|e| e.to_string())?;
    let mut resp = String::new();
    let n = reader
        .read_line(&mut resp)
        .map_err(|e| format!("read from key-provider daemon: {e}"))?;
    if n == 0 {
        return Err("key-provider daemon closed its output unexpectedly".to_string());
    }
    serde_json::from_str(resp.trim())
        .map_err(|e| format!("key-provider daemon sent non-JSON: {e}: {resp}"))
}

fn ok_data(resp: &Value, ctx: &str) -> Result<Value, String> {
    if resp.get("status").and_then(Value::as_str) == Some("ok") {
        Ok(resp.get("data").cloned().unwrap_or(Value::Null))
    } else {
        Err(format!("{ctx}: expected ok, got {resp}"))
    }
}

/// Extract the `mdat` box payload from a single-sample fMP4 segment. The producer's
/// `mux_single_sample_segment` lays out `moof{…} + mdat{ciphertext}`; with full-sample
/// AES-CTR the DECRYPTED mdat payload is byte-identical to the original asset bytes. Walks
/// top-level boxes ([u32 size][4 type][payload]); handles 64-bit largesize (1) / to-EOF (0).
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

/// A short random hex id (process + nanos) — distinct per open, no extra dep.
fn random_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}-{:x}", std::process::id(), nanos)
}

/// Recover + decrypt + render the asset to its byte-identical cleartext via the 2-of-3 quorum.
fn open_quorum_object(args: &QuorumArgs) -> Result<Vec<u8>, String> {
    // --- read the `.ddrm` capsule: escrow + persisted ciphertext. ---
    let capsule: Value = serde_json::from_slice(
        &std::fs::read(&args.capsule_path).map_err(|e| format!("read capsule {}: {e}", args.capsule_path))?,
    )
    .map_err(|e| format!("parse capsule: {e}"))?;

    let protections = capsule
        .get("protections")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .ok_or("capsule has no protections[0] dKMS escrow")?;
    let scheme = protections
        .get("scheme")
        .and_then(Value::as_str)
        .ok_or("escrow missing scheme")?
        .to_string();
    let node_set_id_b64 = protections
        .get("node_set_id_b64")
        .and_then(Value::as_str)
        .ok_or("escrow missing node_set_id_b64")?
        .to_string();
    let producer_vk_b64 = protections
        .get("producer_verifying_key_b64")
        .and_then(Value::as_str)
        .ok_or("escrow missing producer_verifying_key_b64 — asset is UNRECOVERABLE (pre-keystone mint)")?
        .to_string();
    let shares = protections
        .get("shares")
        .and_then(Value::as_array)
        .ok_or("escrow missing shares")?;
    if shares.len() != 3 {
        return Err(format!("expected 3 escrow shares, got {}", shares.len()));
    }
    let share = |i: usize| -> Result<String, String> {
        shares[i]["wrapped_share_b64"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("escrow share {i} missing wrapped_share_b64"))
    };
    let (share1, share2, share3) = (share(0)?, share(1)?, share(2)?);
    // The on-chain KID the shares were escrowed under (the dkms recover keys on it).
    let kid_hex = capsule
        .get("kid")
        .and_then(Value::as_str)
        .ok_or("capsule missing kid")?
        .to_string();
    let segment_b64 = capsule
        .get("ciphertext_b64")
        .and_then(Value::as_str)
        .ok_or("capsule missing ciphertext_b64 — re-mint to persist the sealed segment")?
        .to_string();
    let node_set_id = B64.decode(&node_set_id_b64).map_err(|e| format!("node_set_id_b64: {e}"))?;

    // --- read the PUBLIC-ONLY quorum descriptor for the 3 node verifying keys. ---
    let desc: Value = serde_json::from_slice(
        &std::fs::read(&args.descriptor_path)
            .map_err(|e| format!("read descriptor {}: {e}", args.descriptor_path))?,
    )
    .map_err(|e| format!("parse descriptor: {e}"))?;
    let nodes_v = desc
        .get("threshold")
        .and_then(|t| t.get("nodes"))
        .and_then(Value::as_array)
        .ok_or("descriptor has no threshold.nodes array")?;
    if nodes_v.len() != 3 {
        return Err(format!("open requires a 3-node 2-of-3 descriptor; got {}", nodes_v.len()));
    }
    let node_vk = |i: usize| -> Result<String, String> {
        nodes_v[i]["verifying_key_b64"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("descriptor node {i} missing verifying_key_b64"))
    };
    let (vk1, vk2, vk3) = (node_vk(0)?, node_vk(1)?, node_vk(2)?);

    // --- per-open transcript identity (action/output = stream; the render read). ---
    let now = now_unix();
    let expires_at = now + args.ttl_secs.max(60);
    let session_id = format!("session:open:{}", random_id());
    let rr_request_id = format!("key-release:{session_id}");
    let principal = args.principal.as_str();
    let object_cid = args.object_cid.as_str();
    const ACTION: &str = "stream";
    const VIEWER: &str = "media";
    const OUTPUT: &str = "stream";
    const RR_SCHEMA: &str = "elastos.release.receipt/v1";
    const RR_PROVIDER: &str = "key-provider";
    const RR_STATUS: &str = "released";
    let rr_issued_at = now;
    // A fresh per-open content_hash + nonce (consistency between the release AAD and the decrypt
    // unwrap is automatic — the same values feed both; the decrypt material echoes them back).
    let content_hash = {
        let mut h = [0u8; 32];
        let id = random_id();
        let b = id.as_bytes();
        for (i, slot) in h.iter_mut().enumerate() {
            *slot = b[i % b.len()] ^ (i as u8);
        }
        h.to_vec()
    };
    let nonce = format!("open-nonce-{}", random_id()).into_bytes();

    let release_receipt = json!({
        "schema": RR_SCHEMA,
        "request_id": rr_request_id,
        "object_cid": object_cid,
        "principal_id": principal,
        "session_id": session_id,
        "action": ACTION,
        "provider": RR_PROVIDER,
        "status": RR_STATUS,
        "issued_at": rr_issued_at,
        "expires_at": expires_at,
    });
    let decrypt_request = json!({
        "schema": "elastos.decrypt.session.request/v1",
        "request_id": format!("decrypt:{session_id}"),
        "principal_id": principal,
        "session_id": session_id,
        "object_cid": object_cid,
        "action": ACTION,
        "viewer_interface": VIEWER,
        "release_receipt": release_receipt,
        "output_kind": OUTPUT,
        "reason": "owned dKMS asset render",
        "expires_at": expires_at,
    });
    let rights_receipt = json!({
        "schema": "elastos.rights.decision.receipt/v1",
        "request_id": format!("rights:{session_id}"),
        "content_id": object_cid,
        "principal_id": principal,
        "session_id": session_id,
        "right": ACTION,
        "provider": "rights-provider",
        "allowed": true,
        "issued_at": rr_issued_at,
        "expires_at": expires_at,
    });
    let key_release_request = json!({
        "schema": "elastos.key_release.request/v1",
        "request_id": rr_request_id,
        "principal_id": principal,
        "session_id": session_id,
        "object_cid": object_cid,
        "action": ACTION,
        "rights_receipt": rights_receipt,
        "key_envelope": {
            "scheme": scheme,
            "kid": kid_hex,
            "wrapped_cek": share1,
            "policy_hash": "sha256:owned-open",
            "algorithms": {
                "cipher": "aes-256-gcm",
                "signature": ["ed25519", "ml-dsa-65"],
                "kem": ["x25519", "ml-kem-768"],
                "share_scheme": "shamir-t-of-n",
            },
        },
        "reason": "owned dKMS asset render",
        "expires_at": expires_at,
    });
    // Thread the wallet-signed grant (if the gateway collected one) into the release request, so
    // key-provider forwards it to each node and the nodes authorize trustlessly. The grant arrives
    // base64-encoded (CLI-safe); decode it to the AccessGrantV1 JSON the node deserializes.
    let mut key_release_request = key_release_request;
    if let Some(grant_b64) = args.access_grant_b64.as_deref().filter(|s| !s.trim().is_empty()) {
        let grant_bytes = B64
            .decode(grant_b64.trim())
            .map_err(|e| format!("--access-grant is not valid base64: {e}"))?;
        let grant: Value = serde_json::from_slice(&grant_bytes)
            .map_err(|e| format!("--access-grant is not valid AccessGrantV1 JSON: {e}"))?;
        key_release_request["access_grant"] = grant;
    }

    // --- decrypt boundary: pin the 3 node identities + mint a session key. ---
    let mut decrypt = Capsule::spawn("decrypt-provider", &args.decrypt_bin)?;
    let dec_init = ok_data(
        &decrypt.call(&json!({
            "op": "init",
            "config": { "authority_vk_b64": vk1, "authority_vk2_b64": vk2, "authority_vk3_b64": vk3 }
        }))?,
        "decrypt init",
    )?;
    let session_pub_b64 = dec_init["decrypt_session_public_key_b64"]
        .as_str()
        .ok_or("decrypt-provider published no session key (build --features rail-stream,rail-mint)")?
        .to_string();
    let session_pub = B64.decode(&session_pub_b64).map_err(|e| e.to_string())?;

    // The canonical decrypt-transcript AAD (node-set bound), computed ONCE with the shared
    // encoder — exactly what the decrypt boundary recomputes from the request + material.
    let aad = DecryptTranscriptV1 {
        suite_id: SUITE_PQ_HYBRID,
        provider_id: "decrypt-provider",
        principal_id: principal,
        session_id: &session_id,
        object_cid,
        content_hash: &content_hash,
        action: ACTION,
        viewer_interface: VIEWER,
        output_kind: OUTPUT,
        expires_at,
        release_receipt_hash: release_receipt_hash(
            RR_SCHEMA,
            &rr_request_id,
            object_cid,
            principal,
            &session_id,
            ACTION,
            RR_PROVIDER,
            RR_STATUS,
            rr_issued_at,
            expires_at,
        ),
        decrypt_session_pub: &session_pub,
        nonce: &nonce,
        node_set_id: Some(&node_set_id),
    }
    .to_aad();

    // --- key-provider (dkms): recover 2-of-3 from the live nodes + re-seal to the session. ---
    let session_ctx = json!({
        "decrypt_session_pub_b64": session_pub_b64,
        // All three shares were signed by the SAME in-boundary producer identity at mint.
        "producer_vk_b64": producer_vk_b64,
        "producer_vk2_b64": producer_vk_b64,
        "producer_vk3_b64": producer_vk_b64,
        "aad_b64": B64.encode(&aad),
        "ciphertext_b64": segment_b64,
        "content_hash_b64": B64.encode(&content_hash),
        "nonce_b64": B64.encode(&nonce),
        "wrapped_cek_share2_b64": share2,
        "wrapped_cek_share3_b64": share3,
        "now_unix": now,
    });
    // Phase A: drive the recover against the WARM key-provider daemon (node sessions reused across
    // opens) when the gateway wired one up; otherwise spawn a fresh key-provider for this open.
    let release = key_release(
        &args.key_bin,
        &args.descriptor_path,
        &args.caller_seed_b64,
        &key_release_request,
        &session_ctx,
    )?;
    let material = release["material"].clone();
    if material["sealed_cek_b64"].as_str().unwrap_or_default().is_empty() {
        return Err(format!("key-provider returned no sealed material: {release}"));
    }

    // --- decrypt: reconstruct the CEK in-VM from the 2-of-3 shares + stream the segment. ---
    let rendered = decrypt.call(&json!({
        "op": "stream_segment",
        "request": decrypt_request,
        "material": material,
        "index": 0,
        "now_unix": now,
    }))?;
    let rendered_data = ok_data(&rendered, "decrypt stream_segment (quorum render)")?;
    let segment_out_b64 = rendered_data["segment_b64"]
        .as_str()
        .ok_or_else(|| format!("stream_segment returned no segment_b64: {rendered}"))?;
    decrypt.shutdown();
    let segment_out = B64.decode(segment_out_b64).map_err(|e| e.to_string())?;
    extract_mdat(&segment_out)
}

/// `--quorum` entrypoint: recover the asset once, print the object descriptor, then serve the
/// decrypted bytes over the same stdio protocol the gateway's object viewer already drives.
pub fn run_quorum(args: QuorumArgs) -> Result<(), String> {
    let object = open_quorum_object(&args)?;

    let descriptor = json!({
        "schema": "elastos.media-authority.session/v1",
        "kind": "object",
        "mime": args.mime,
        "byte_length": object.len(),
        "expires_at": now_unix() + args.ttl_secs.max(60),
    });
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(out, "{descriptor}").map_err(|e| format!("write descriptor: {e}"))?;
    out.flush().map_err(|e| format!("flush descriptor: {e}"))?;

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| format!("read stdin: {e}"))?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                reply(&mut out, &json!({"status": "error", "message": format!("bad request json: {e}")}))?;
                continue;
            }
        };
        match req.get("op").and_then(Value::as_str) {
            Some("shutdown") => return Ok(()),
            Some("object") => reply(
                &mut out,
                &json!({ "status": "ok", "object_b64": B64.encode(&object) }),
            )?,
            other => reply(
                &mut out,
                &json!({"status": "error", "message": format!("unknown op: {other:?}")}),
            )?,
        }
    }
    Ok(())
}

fn reply(out: &mut impl Write, value: &Value) -> Result<(), String> {
    writeln!(out, "{value}").map_err(|e| format!("write reply: {e}"))?;
    out.flush().map_err(|e| format!("flush reply: {e}"))
}
