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
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
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
    let release_msg =
        json!({ "op": "release", "request": release_request, "session": session_ctx });

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
    let stream = UnixStream::connect(sock_path).map_err(|e| format!("connect {sock_path}: {e}"))?;
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
        let size =
            u32::from_be_bytes([seg[off], seg[off + 1], seg[off + 2], seg[off + 3]]) as usize;
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
fn recover_quorum(args: &QuorumArgs) -> Result<QuorumOpen, String> {
    // --- read the `.ddrm` capsule: escrow + persisted ciphertext. ---
    let capsule: Value = serde_json::from_slice(
        &std::fs::read(&args.capsule_path)
            .map_err(|e| format!("read capsule {}: {e}", args.capsule_path))?,
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
    // PRE-AUDIT #1: the producer's published CEK commitment, when the asset was minted with one.
    // Forwarded verbatim into the session so the decrypt boundary verifies the reconstructed CEK
    // against it (integrity backstop). Absent ⇒ legacy asset; integrity rests on the boundary's
    // 3-share cheater detection (the rail serves all three).
    let cek_commitment_b64 = protections
        .get("cek_commitment_b64")
        .and_then(Value::as_str)
        .map(str::to_string);
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
    let node_set_id = B64
        .decode(&node_set_id_b64)
        .map_err(|e| format!("node_set_id_b64: {e}"))?;

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
        return Err(format!(
            "open requires a 3-node 2-of-3 descriptor; got {}",
            nodes_v.len()
        ));
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
    if let Some(grant_b64) = args
        .access_grant_b64
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
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
        .ok_or(
            "decrypt-provider published no session key (build --features rail-stream,rail-mint)",
        )?
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
    let mut session_ctx = json!({
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
    // PRE-AUDIT #1: forward the published commitment so the key-provider welds it into the merged
    // material and the decrypt boundary fails closed if the reconstructed CEK does not match.
    if let Some(commit) = &cek_commitment_b64 {
        session_ctx["cek_commitment_b64"] = json!(commit);
    }
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
    if material["sealed_cek_b64"]
        .as_str()
        .unwrap_or_default()
        .is_empty()
    {
        return Err(format!(
            "key-provider returned no sealed material: {release}"
        ));
    }

    // The CEK is recovered + re-sealed; the live decrypt boundary holds it in-VM. We DO NOT
    // decrypt here — the boundary is kept warm so the object can be decrypted once (raw mimes)
    // or rendered page-by-page (pixel-lock mimes) WITHOUT re-hitting the quorum. The recovered
    // CEK never leaves the VM; for pixel-lock content not even the plaintext leaves the VM.
    Ok(QuorumOpen {
        decrypt,
        decrypt_request,
        material,
        now,
    })
}

/// A warm quorum open: the CEK is recovered + sealed into the live decrypt boundary. Drives
/// the boundary for either a one-shot raw object read (media/other) or repeated in-boundary
/// page renders (pixel-lock), reusing the same sealed material — no extra quorum round-trips.
struct QuorumOpen {
    decrypt: Capsule,
    decrypt_request: Value,
    material: Value,
    now: u64,
}

/// A rendered page returned from the decrypt boundary: the watermarked image plus the
/// document's page count (so the viewer can page through without the source file).
struct RenderedPage {
    image: Vec<u8>,
    content_type: String,
    total_pages: u32,
}

impl QuorumOpen {
    /// Decrypt the whole object once (the legacy raw path: media + non-pixel-lock objects).
    /// Reconstructs the CEK in-VM, returns the byte-identical cleartext (mdat payload).
    fn decrypt_object(&mut self) -> Result<Vec<u8>, String> {
        let (request, material, now) = (
            self.decrypt_request.clone(),
            self.material.clone(),
            self.now,
        );
        let rendered = self.decrypt.call(&json!({
            "op": "stream_segment",
            "request": request,
            "material": material,
            "index": 0,
            "now_unix": now,
        }))?;
        let data = ok_data(&rendered, "decrypt stream_segment (quorum render)")?;
        let segment_b64 = data["segment_b64"]
            .as_str()
            .ok_or_else(|| format!("stream_segment returned no segment_b64: {rendered}"))?;
        let segment = B64.decode(segment_b64).map_err(|e| e.to_string())?;
        extract_mdat(&segment)
    }

    /// FIRST page render: ships the sealed material ONCE so the boundary reconstructs the CEK,
    /// decrypts + extracts the object, and PARSES it into a warm in-VM session keyed by this
    /// open's `session_id`. Only the watermarked page image comes back (the raw file never
    /// leaves the boundary). Every later page is served by `render_warm_page` with NO material.
    fn render_first_page(&mut self, mime: &str, watermark: &str) -> Result<RenderedPage, String> {
        let (request, material, now) = (
            self.decrypt_request.clone(),
            self.material.clone(),
            self.now,
        );
        let resp = self.decrypt.call(&json!({
            "op": "stream_segment",
            "request": request,
            "material": material,
            "index": 0,
            "now_unix": now,
            "render": { "mime": mime, "page": 0, "watermark": watermark },
        }))?;
        Self::parse_rendered_page(resp)
    }

    /// Render a FURTHER page from the warm in-VM session — NO sealed material, NO ciphertext,
    /// NO quorum round-trip. The boundary rasterises from the already-parsed document (or serves
    /// a cached page image). Fails closed in the boundary if the warm session is gone.
    fn render_warm_page(&mut self, page: u32, watermark: &str) -> Result<RenderedPage, String> {
        let session_id = self
            .decrypt_request
            .get("session_id")
            .and_then(Value::as_str)
            .ok_or("decrypt request missing session_id for a warm page render")?
            .to_string();
        let resp = self.decrypt.call(&json!({
            "op": "render_page",
            "session_id": session_id,
            "page": page,
            "watermark": watermark,
        }))?;
        Self::parse_rendered_page(resp)
    }

    /// Decode a `render-page/v1` response into a `RenderedPage`. Fails closed on a missing image.
    fn parse_rendered_page(resp: Value) -> Result<RenderedPage, String> {
        let data = ok_data(&resp, "decrypt render page")?;
        let image_b64 = data["rendered_b64"]
            .as_str()
            .ok_or_else(|| format!("render returned no rendered_b64: {resp}"))?;
        let image = B64.decode(image_b64).map_err(|e| e.to_string())?;
        let content_type = data["content_type"]
            .as_str()
            .unwrap_or("image/jpeg")
            .to_string();
        let total_pages = data["total_pages"].as_u64().unwrap_or(1) as u32;
        Ok(RenderedPage {
            image,
            content_type,
            total_pages,
        })
    }

    fn shutdown(self) {
        self.decrypt.shutdown();
    }
}

/// Whether a mime is served as flattened, watermarked page images ("pixel-lock") rather than
/// as its raw bytes. MUST mirror the decrypt boundary's `render::is_pixel_lock` (Principle 12):
/// PDF (multi-page), CBZ comics (multi-page), text/code (multi-page), SVG (rasterised), raster
/// images (single page). SVG is rasterised rather than shipped raw (it is scriptable XML).
fn is_pixel_lock(mime: &str) -> bool {
    let m = mime.trim().to_ascii_lowercase();
    m == "application/pdf"
        || m == "application/vnd.comicbook+zip"
        || m == "application/x-cbz"
        || m.starts_with("text/")
        || matches!(
            m.as_str(),
            "application/json"
                | "application/javascript"
                | "application/xml"
                | "application/x-yaml"
                | "application/yaml"
                | "application/toml"
                | "application/x-sh"
                | "application/x-shellscript"
        )
        || m.starts_with("image/")
}

/// A short, readable forensic stamp from a principal id (the full-ASCII watermark font renders
/// any principal; long DIDs/wallets are elided to first…last so a stamp always fits + shows).
fn watermark_for(principal: &str) -> String {
    let p = principal.trim();
    if p.len() <= 22 {
        p.to_string()
    } else {
        format!("{}..{}", &p[..10], &p[p.len() - 8..])
    }
}

/// `--quorum` entrypoint: recover the asset once, print the object descriptor, then serve the
/// decrypted bytes over the same stdio protocol the gateway's object viewer already drives.
pub fn run_quorum(args: QuorumArgs) -> Result<(), String> {
    let mut open = recover_quorum(&args)?;

    // PIXEL-LOCK (e.g. PDF): the raw file NEVER leaves the decrypt boundary. We render page 0
    // in-VM to learn the page count + serve the first image, then answer `{"op":"page","n":I}`
    // by rendering that page on demand (reusing the warm boundary). The gateway/browser only
    // ever receive watermarked JPEGs — closing the "raw plaintext reaches the client" gap and
    // sidestepping browser PDF-viewer quirks.
    if is_pixel_lock(&args.mime) {
        return serve_pixel_lock(&mut open, &args);
    }

    // RAW path (media + non-pixel-lock objects): unchanged — decrypt once, serve the bytes.
    let object = open.decrypt_object()?;
    open.shutdown();

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
                reply(
                    &mut out,
                    &json!({"status": "error", "message": format!("bad request json: {e}")}),
                )?;
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

/// Serve a pixel-lock asset as on-demand, watermarked page images. The descriptor advertises
/// `pixel_locked: true` + `total_pages`; the gateway's page route drives `{"op":"page","n":I}`.
/// The decrypt boundary stays warm so each page renders in-VM without another quorum recovery.
fn serve_pixel_lock(open: &mut QuorumOpen, args: &QuorumArgs) -> Result<(), String> {
    let watermark = watermark_for(&args.principal);

    // Render page 0 up front: ships material ONCE, warms the in-VM parsed-document session, and
    // yields the page count for the descriptor. Every later page reuses that warm session.
    let first = open.render_first_page(&args.mime, &watermark)?;

    let descriptor = json!({
        "schema": "elastos.media-authority.session/v1",
        "kind": "object",
        "mime": args.mime,
        "pixel_locked": true,
        "page_content_type": first.content_type,
        "total_pages": first.total_pages,
        // No single coherent byte_length for a paged render; report the first page's size.
        "byte_length": first.image.len(),
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
                reply(
                    &mut out,
                    &json!({"status": "error", "message": format!("bad request json: {e}")}),
                )?;
                continue;
            }
        };
        match req.get("op").and_then(Value::as_str) {
            Some("shutdown") => return Ok(()),
            Some("page") => {
                let n = req.get("n").and_then(Value::as_u64).unwrap_or(0) as u32;
                // Reuse the page-0 render rather than re-rendering it.
                let page = if n == 0 {
                    Ok(RenderedPage {
                        image: first.image.clone(),
                        content_type: first.content_type.clone(),
                        total_pages: first.total_pages,
                    })
                } else {
                    // Warm path: no material, no quorum — rasterise from the parsed doc in-VM.
                    open.render_warm_page(n, &watermark)
                };
                match page {
                    Ok(p) => reply(
                        &mut out,
                        &json!({
                            "status": "ok",
                            "page_index": n,
                            "total_pages": p.total_pages,
                            "content_type": p.content_type,
                            "page_b64": B64.encode(&p.image),
                        }),
                    )?,
                    Err(e) => reply(&mut out, &json!({"status": "error", "message": e}))?,
                }
            }
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
