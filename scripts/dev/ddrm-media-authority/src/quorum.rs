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
    /// MEDIA only: the local path of the DASH directory the gateway pre-fetched from `asset_cid`
    /// (contains the clear per-track inits + the ENCRYPTED segments + `stream.mpd`). When set and
    /// the capsule carries a `media` layout, the open recovers the CEK 2-of-3 and serves the
    /// CENC-decrypted fMP4 segments to the MSE player (instead of the single-blob object path).
    pub dash_dir: Option<String>,
    /// OPTIONAL wallet-signed access grant (base64 of the AccessGrantV1 JSON). When present the
    /// dkms nodes authorize TRUSTLESSLY (verify the wallet/session signatures + read
    /// `hasAccessByContentId` themselves) instead of trusting the unsigned rights receipt. The
    /// gateway builds it from the user's `personal_sign`; this helper only forwards it.
    pub access_grant_b64: Option<String>,
    /// AV forensic variants (chunk 3): the serving node's per-node bias MASTER secret (base64).
    /// Combined with the stable `object_cid` it derives the per-asset bias vector
    /// ([`ddrm_envelope::av::asset_secret_from_master`]) the selector keys on. `None` (node not
    /// provisioned for AV) ⇒ the single encode is served, `fingerprinted:false` (honest).
    pub av_master_b64: Option<String>,
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
    // The ciphertext the boundary will decrypt: a single embedded sample (object) OR the
    // flattened, CENC-ordered DASH segment set fetched from the published directory (media).
    let payload = compute_open_payload(&capsule, args)?;
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
    // MEDIA: the WHOLE ordered, content-addressed segment set is welded into the transcript AAD
    // (the decrypt boundary recomputes the identical digest from the segments in the material, so
    // a substituted/reordered fragment fails the CEK unwrap closed before any byte is decrypted).
    // OBJECT (no digests): byte-identical to the single-segment `to_aad()`.
    .to_aad_with_segments(payload.segment_digests.as_deref());

    // --- key-provider (dkms): recover 2-of-3 from the live nodes + re-seal to the session. ---
    let mut session_ctx = json!({
        "decrypt_session_pub_b64": session_pub_b64,
        // All three shares were signed by the SAME in-boundary producer identity at mint.
        "producer_vk_b64": producer_vk_b64,
        "producer_vk2_b64": producer_vk_b64,
        "producer_vk3_b64": producer_vk_b64,
        "aad_b64": B64.encode(&aad),
        "ciphertext_b64": payload.ciphertext_b64,
        "content_hash_b64": B64.encode(&content_hash),
        "nonce_b64": B64.encode(&nonce),
        "wrapped_cek_share2_b64": share2,
        "wrapped_cek_share3_b64": share3,
        "now_unix": now,
    });
    // MEDIA multi-segment: segments 1..N ride as `extra_segments_b64` and the clear init/moov as
    // `init_segment_b64`. They never reach the dkms nodes (the key-provider strips content to the
    // nodes); they are welded back into the material AFTER recovery and feed the in-VM decrypt.
    if !payload.extra_segments_b64.is_empty() {
        session_ctx["extra_segments_b64"] = json!(payload.extra_segments_b64);
    }
    if let Some(init_b64) = &payload.init_b64 {
        session_ctx["init_segment_b64"] = json!(init_b64);
    }
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
        media: payload.media,
        fingerprinted: payload.fingerprinted,
    })
}

/// The decryptable payload for an open: a single embedded sample (object) or the flattened,
/// CENC-ordered DASH segment set fetched from the published directory (media).
struct OpenPayload {
    /// Segment 0 (object: the only sample; media: the first fragment in seal order).
    ciphertext_b64: String,
    /// Segments 1..N in seal order (media only; empty for an object).
    extra_segments_b64: Vec<String>,
    /// The clear init/moov fragment (media only) — CENC encrypts media fragments, not the init.
    init_b64: Option<String>,
    /// The whole-set segment-digest binding for the transcript AAD (media only; `None` ⇒ object).
    segment_digests: Option<Vec<u8>>,
    /// Media presentation info for the streaming descriptor (`None` for objects).
    media: Option<MediaInfo>,
    /// AV forensic state (chunk 3): `true` iff a valid per-buyer variant selection was served from a
    /// published manifest. The HONEST default — single encode, no manifest, or a node not provisioned
    /// for AV — is `false` (never claim a mark that isn't there).
    fingerprinted: bool,
}

/// One playable DASH track (AdaptationSet) for the MSE player: its clear init, the MSE
/// `addSourceBuffer` mime/codecs string derived from that init, and the GLOBAL (flat,
/// CENC-counter) indices of its segments in playback order. The player attaches one
/// `SourceBuffer` per track (PC2's two-viewer model) and asks for `(track, local_index)`;
/// the serve loop maps that to `global_indices[local_index]` to decrypt the right fragment.
struct MediaTrack {
    kind: String,
    mime: String,
    init: Vec<u8>,
    global_indices: Vec<usize>,
}

/// What the media descriptor + segment-serve loop need: one or more tracks (a single track
/// for audio / muxed video; two — video + audio — for split DASH). Per-track segment ranges are
/// the source of truth; the flat sealed set is addressed via each track's `global_indices`.
struct MediaInfo {
    tracks: Vec<MediaTrack>,
}

/// Derive the MSE `addSourceBuffer` mime/codecs string from the clear DASH init segment.
///
/// The capsule records the ORIGINAL upload type (e.g. `audio/mpeg` for an MP3), but the streamed
/// asset is transcoded fragmented MP4 (`audio/mp4` / `video/mp4` carrying AAC / AVC). Handing the
/// original type to `MediaSource.addSourceBuffer` makes the browser create a buffer it then refuses
/// to decode the fMP4 fragments into — the player "plays" silence at 0:00. Parse the init's `moov`
/// for its tracks (PC2's `extractTrackInfo`) and emit the same container + codecs string the MPD
/// advertises. Falls back to `fallback` only if the init has no parseable track, in which case the
/// player surfaces an explicit unsupported-type error rather than silent playback.
fn mse_mime_from_init(init: &[u8], fallback: &str) -> String {
    use ddrm_media::mpd::TrackKind;
    let Ok(meta) = ddrm_media::mp4::parse_fragment_metadata(init) else {
        return fallback.to_string();
    };
    let codecs: Vec<String> = meta
        .tracks
        .iter()
        .map(|t| t.codec.clone())
        .filter(|c| c != "unknown")
        .collect();
    if codecs.is_empty() {
        return fallback.to_string();
    }
    let container = if meta.tracks.iter().any(|t| t.kind == TrackKind::Video) {
        "video/mp4"
    } else {
        "audio/mp4"
    };
    format!("{container}; codecs=\"{}\"", codecs.join(","))
}

/// Resolve the open payload from the `.ddrm` capsule (+ the pre-fetched DASH dir for media).
fn compute_open_payload(capsule: &Value, args: &QuorumArgs) -> Result<OpenPayload, String> {
    // MEDIA: the capsule carries a `media` layout and the gateway pre-fetched the DASH directory.
    if let (Some(media), Some(dir)) = (
        capsule.get("media").filter(|m| !m.is_null()),
        args.dash_dir.as_deref().filter(|s| !s.is_empty()),
    ) {
        let dir = std::path::Path::new(dir);
        let init_path = media
            .get("init_path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let seg_paths = media
            .get("segment_paths")
            .and_then(Value::as_array)
            .ok_or("capsule media layout missing segment_paths")?;
        if seg_paths.is_empty() {
            return Err("capsule media layout has no segments".to_string());
        }
        // Read each ENCRYPTED segment from the fetched directory in the sealed (flattened
        // CENC-counter) order. The decrypt boundary recomputes the segment-digest AAD over
        // exactly this ordered set, so the order MUST match the seal — hence the persisted list.
        let mut segments: Vec<Vec<u8>> = Vec::with_capacity(seg_paths.len());
        for p in seg_paths {
            let rel = p.as_str().ok_or("segment_paths entry is not a string")?;
            segments.push(read_dash_file(dir, rel)?);
        }
        // AV forensic variants (chunk 3): if the asset published a variant manifest and this node is
        // provisioned for AV with a wallet-signed grant, replace each marked segment's ciphertext
        // with the per-buyer SELECTED variant. The served bytes are then welded into the transcript
        // AAD via `segment_digests` below (the decrypt boundary recomputes the same digest), so a
        // buyer's served copy is cryptographically bound to their selection. Absent any of those
        // (no manifest / no master / no grant) the single encode is served, `fingerprinted:false`.
        let fingerprinted = apply_variant_selection(dir, &mut segments, args)?;
        let init_bytes = if init_path.is_empty() {
            Vec::new()
        } else {
            read_dash_init(dir, init_path)?
        };
        // Weld the ordered segment set into the transcript AAD ONLY when there is more than one
        // segment — this MUST match the decrypt boundary, which computes `segment_digests` iff
        // `extra_segments` is non-empty (`decrypt-provider/src/main.rs`). For a single-segment
        // asset both sides use `None` (the lone ciphertext is already bound); a mismatch here
        // makes the CEK unwrap fail closed (`decrypt_failed`) before any byte is decrypted.
        let segment_digests = if segments.len() > 1 {
            let ordered: Vec<&[u8]> = segments.iter().map(|s| s.as_slice()).collect();
            Some(ddrm_envelope::segment_digests(&ordered))
        } else {
            None
        };
        let ciphertext_b64 = B64.encode(&segments[0]);
        let extra_segments_b64 = segments[1..].iter().map(|s| B64.encode(s)).collect();
        // The seal binds the FIRST track's init (`creator.rs` `first_init_b64`); keep it for the
        // decrypt material/AAD. Per-track inits for MSE are resolved separately in the track model.
        let init_b64 = if init_bytes.is_empty() {
            None
        } else {
            Some(B64.encode(&init_bytes))
        };
        let tracks = build_media_tracks(media, dir, seg_paths, &init_bytes, args)?;
        return Ok(OpenPayload {
            ciphertext_b64,
            extra_segments_b64,
            init_b64,
            segment_digests,
            media: Some(MediaInfo { tracks }),
            fingerprinted,
        });
    }

    // OBJECT: the single sealed sample is embedded in the capsule (no IPFS fetch at open).
    let segment_b64 = capsule
        .get("ciphertext_b64")
        .and_then(Value::as_str)
        .ok_or("capsule missing ciphertext_b64 — re-mint to persist the sealed segment")?
        .to_string();
    Ok(OpenPayload {
        ciphertext_b64: segment_b64,
        extra_segments_b64: Vec::new(),
        init_b64: None,
        segment_digests: None,
        media: None,
        fingerprinted: false,
    })
}

/// Build the per-track MSE grouping from the capsule's `media.tracks[]` (the per-AdaptationSet
/// layout minted alongside the flat seal order). Each track's segment paths are mapped to their
/// GLOBAL (flat) CENC-counter index via `seg_paths` so the serve loop can decrypt `(track, i)` by
/// the right global index. Falls back to a SINGLE track over the whole flat set (init = the first
/// init) for older capsules minted before per-track layout existed, or for muxed single-track
/// assets — so existing `.ddrm` keep opening unchanged.
fn build_media_tracks(
    media: &Value,
    dir: &std::path::Path,
    seg_paths: &[Value],
    first_init: &[u8],
    args: &QuorumArgs,
) -> Result<Vec<MediaTrack>, String> {
    // path -> global flat index (position in the sealed segment order).
    let mut global_of: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for (i, p) in seg_paths.iter().enumerate() {
        if let Some(s) = p.as_str() {
            global_of.entry(s).or_insert(i);
        }
    }
    if let Some(track_arr) = media
        .get("tracks")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
    {
        let mut tracks = Vec::with_capacity(track_arr.len());
        for t in track_arr {
            let kind = t
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let init_path = t
                .get("init_path")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let init = if init_path.is_empty() {
                Vec::new()
            } else {
                read_dash_init(dir, init_path)?
            };
            let seg_list = t
                .get("segment_paths")
                .and_then(Value::as_array)
                .ok_or("capsule track missing segment_paths")?;
            let mut global_indices = Vec::with_capacity(seg_list.len());
            for p in seg_list {
                let rel = p
                    .as_str()
                    .ok_or("track segment_paths entry is not a string")?;
                let idx = *global_of
                    .get(rel)
                    .ok_or_else(|| format!("track segment {rel:?} not in sealed segment set"))?;
                global_indices.push(idx);
            }
            // Per-track container fallback (used only if the init has no parseable codec):
            // a video AdaptationSet is `video/mp4`, audio is `audio/mp4`.
            let fallback = match kind.as_str() {
                "video" => "video/mp4",
                "audio" => "audio/mp4",
                _ => args.mime.as_str(),
            };
            tracks.push(MediaTrack {
                mime: mse_mime_from_init(&init, fallback),
                kind,
                init,
                global_indices,
            });
        }
        return Ok(tracks);
    }
    // Back-compat: a single track spanning the whole flat set, with the first (only) init.
    let kind = if args.mime.starts_with("video") {
        "video"
    } else {
        "audio"
    };
    Ok(vec![MediaTrack {
        kind: kind.to_string(),
        mime: mse_mime_from_init(first_init, &args.mime),
        init: first_init.to_vec(),
        global_indices: (0..seg_paths.len()).collect(),
    }])
}

/// Read a DASH directory file at relative `rel` under the pre-fetched `dir`, rejecting any
/// path-escape. `rel` comes from the (owner-minted) capsule layout; the guard is defence-in-depth.
fn read_dash_file(dir: &std::path::Path, rel: &str) -> Result<Vec<u8>, String> {
    if rel.is_empty() || rel.split(['/', '\\']).any(|c| c == ".." || c.is_empty()) {
        return Err(format!("rejecting unsafe DASH path: {rel:?}"));
    }
    let path = dir.join(rel);
    std::fs::read(&path).map_err(|e| format!("read DASH file {}: {e}", path.display()))
}

/// Read a DASH **init** segment from the dir and down-convert it for the server-decrypt rail:
/// strip any MPEG-DASH/CENC signaling (`encv`/`enca` -> original, drop `sinf`/`tenc`/`pssh`) so it
/// matches the PLAINTEXT init the mint sealed (the seal binds `first_init` BEFORE PSSH injection)
/// AND the clear, `senc`-stripped segments this rail serves. No-op on unsignaled inits (demo /
/// pre-compliance assets); best-effort — a malformed init falls back to the raw bytes.
fn read_dash_init(dir: &std::path::Path, rel: &str) -> Result<Vec<u8>, String> {
    let raw = read_dash_file(dir, rel)?;
    Ok(ddrm_media::mp4::strip_cenc_signal(&raw).unwrap_or(raw))
}

/// A warm quorum open: the CEK is recovered + sealed into the live decrypt boundary. Drives
/// the boundary for either a one-shot raw object read (media/other) or repeated in-boundary
/// page renders (pixel-lock), reusing the same sealed material — no extra quorum round-trips.
struct QuorumOpen {
    decrypt: Capsule,
    decrypt_request: Value,
    material: Value,
    now: u64,
    /// MEDIA: the clear init + streamable segment count (set when the open recovered a DASH
    /// segment set). `None` for the object / pixel-lock paths.
    media: Option<MediaInfo>,
    /// AV forensic state (chunk 3): whether this open served a per-buyer variant selection (an
    /// honest `false` for single-encode assets). Surfaced on the media descriptor so the gateway /
    /// player / audit can record that the served stream is fingerprinted — never claimed otherwise.
    fingerprinted: bool,
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

    /// Decrypt DASH media segment `index` from the recovered set. Reconstructs the CEK in-VM and
    /// returns the CENC-decrypted (senc-stripped) fMP4 fragment — the byte unit the MSE player
    /// appends after the init. The whole ordered set is bound into the transcript AAD, so a
    /// substituted/reordered fragment fails the unwrap closed before any byte is returned. Unlike
    /// `decrypt_object` this does NOT extract `mdat`: the fragment IS the playable presentation unit.
    fn decrypt_media_segment(&mut self, index: usize) -> Result<Vec<u8>, String> {
        let (request, material, now) = (
            self.decrypt_request.clone(),
            self.material.clone(),
            self.now,
        );
        let resp = self.decrypt.call(&json!({
            "op": "stream_segment",
            "request": request,
            "material": material,
            "index": index,
            "now_unix": now,
        }))?;
        let data = ok_data(&resp, "decrypt stream_segment (quorum media)")?;
        let segment_b64 = data["segment_b64"]
            .as_str()
            .ok_or_else(|| format!("stream_segment returned no segment_b64: {resp}"))?;
        B64.decode(segment_b64).map_err(|e| e.to_string())
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

/// Whether a mime is served as a rendered representation ("pixel-lock"/"html-lock") rather than
/// as its raw bytes. MUST mirror the decrypt boundary's `render::is_pixel_lock` (Principle 12):
/// PDF (multi-page), CBZ comics (multi-page), text/code (multi-page), SVG (rasterised), raster
/// images (single page), and EPUB (sanitised chapter HTML — "html-lock"). SVG and EPUB are
/// rendered rather than shipped raw because both are scriptable.
fn is_pixel_lock(mime: &str) -> bool {
    let m = mime.trim().to_ascii_lowercase();
    m == "application/pdf"
        || m == "application/vnd.comicbook+zip"
        || m == "application/x-cbz"
        || m == "application/epub+zip"
        || m == "application/epub"
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

/// Build the forensic stamp for THIS open. The mark is traceable to (1) the WALLET that authorized
/// the view — recovered from the wallet-signed access grant when present, so it is the real on-chain
/// owner identity rather than the local principal — (2) the short content id of the asset, and (3)
/// the UTC minute the view was opened. Tiled across the page by the renderer so it cannot be cropped
/// off, this answers "who opened WHAT and WHEN" from any leaked frame.
///
/// Falls back to the local principal + object cid when no wallet grant is attached (e.g. the local
/// dev rights mode), so the stamp is never empty (the renderer fails closed on an empty stamp).
fn watermark_for(args: &QuorumArgs, opened_at: u64) -> String {
    let (identity, content) = forensic_identity(args);
    let human = format!(
        "{identity} \u{00b7} {content} \u{00b7} {}",
        format_utc_minute(opened_at)
    );
    // Append the AUTHENTICATED grant-digest token for the invisible layer only (the decrypt boundary
    // splits it back off before the visible compositor). When no wallet-signed grant is attached
    // (local-dev rights mode), the stamp stays the plain human string and the invisible layer falls
    // back to the unauthenticated compact wallet.
    match grant_digest_hex(args) {
        Some(hex) => format!("{human}\u{1F}gd:{hex}"),
        None => human,
    }
}

/// The 32-hex authenticated forensic anchor for this open: SHA-256 of the wallet-signed grant's
/// EIP-191 delegation signature, truncated to 16 bytes. Computed via the SHARED
/// [`ddrm_envelope::grant_watermark_digest16`] so the verifier (`decrypt-provider
/// --extract-watermark --verify-grant`) derives the identical value (no drift, Principle 12).
/// `None` when no parseable grant is attached.
fn grant_digest_hex(args: &QuorumArgs) -> Option<String> {
    let digest = grant_digest_raw(args)?;
    let mut hex = String::with_capacity(32);
    for b in digest {
        hex.push_str(&format!("{b:02x}"));
    }
    Some(hex)
}

/// The raw 16-byte forensic grant digest (the codeword input the serve selector keys on), derived
/// via the SHARED [`ddrm_envelope::grant_watermark_digest16`] from the wallet-signed grant's EIP-191
/// delegation signature. `None` when no parseable grant is attached.
fn grant_digest_raw(args: &QuorumArgs) -> Option<[u8; 16]> {
    let grant_b64 = args
        .access_grant_b64
        .as_deref()
        .filter(|s| !s.trim().is_empty())?;
    let bytes = B64.decode(grant_b64.trim()).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    let sig = v.get("delegation_sig_hex")?.as_str()?;
    Some(ddrm_envelope::grant_watermark_digest16(sig))
}

/// The published variant manifest filename in the DASH directory (chunk 3).
const AV_VARIANTS_FILE: &str = "av-variants.json";

/// AV forensic variant selection (chunk 3). If `dir` carries a valid, fingerprinted variant manifest
/// AND this node is provisioned with a bias master AND a wallet-signed grant is present, replace each
/// marked segment's ciphertext in `segments` with the per-buyer SELECTED variant (read from the DASH
/// dir) and return `true`. Otherwise leave `segments` untouched and return `false` — the honest
/// single-encode path (no manifest / not provisioned / no grant). It FAILS CLOSED only when the
/// manifest claims a mark and the node is AV-provisioned with a grant but the selection cannot be
/// honored (malformed manifest, wrong master ⇒ bias-commitment mismatch, out-of-range index, or a
/// missing variant file) — never silently serving a half-applied mark.
fn apply_variant_selection(
    dir: &std::path::Path,
    segments: &mut [Vec<u8>],
    args: &QuorumArgs,
) -> Result<bool, String> {
    // Manifest present? Absent ⇒ single encode (the common, honest case).
    let Ok(bytes) = std::fs::read(dir.join(AV_VARIANTS_FILE)) else {
        return Ok(false);
    };
    let manifest: ddrm_envelope::av::VariantManifestV1 = serde_json::from_slice(&bytes)
        .map_err(|e| format!("{AV_VARIANTS_FILE} is not a valid variant manifest: {e}"))?;
    manifest
        .validate()
        .map_err(|e| format!("{AV_VARIANTS_FILE} failed validation: {e}"))?;
    if !manifest.fingerprinted {
        return Ok(false);
    }
    // Node provisioned for AV? (a bias master) — absent ⇒ serve the single encode, honestly.
    let Some(master_b64) = args
        .av_master_b64
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    else {
        return Ok(false);
    };
    let master = B64
        .decode(master_b64.trim())
        .map_err(|e| format!("--av-master is not valid base64: {e}"))?;
    // Per-buyer grant present? Without it there is no codeword input ⇒ serve the single encode.
    let Some(grant_digest) = grant_digest_raw(args) else {
        return Ok(false);
    };
    // From here the manifest claims a mark, the node is provisioned, and a buyer is identified —
    // any failure is a real integrity/config error and MUST fail closed.
    let asset_secret =
        ddrm_envelope::av::asset_secret_from_master(&master, args.object_cid.as_bytes());
    let bias = ddrm_envelope::av::asset_bias_vector(&asset_secret, manifest.codeword.length);
    let symbols = ddrm_envelope::av::select_symbols(&manifest, &bias, &grant_digest)
        .map_err(|e| format!("variant selection failed closed: {e}"))?;
    for (seg, &sym) in manifest.marked_segments.iter().zip(&symbols) {
        let idx = seg.index as usize;
        let variant = seg
            .variants
            .get(sym as usize)
            .ok_or("selected symbol out of range for the marked segment")?;
        let selected = read_dash_file(dir, &variant.uri)?;
        let slot = segments.get_mut(idx).ok_or_else(|| {
            format!("marked segment index {idx} is out of range of the sealed segment set")
        })?;
        *slot = selected;
    }
    Ok(true)
}

/// `(display identity, short content id)` for the stamp. Prefers the wallet address + on-chain
/// `contentId` carried by the wallet-signed [`AccessGrantV1`]; falls back to the principal + cid.
fn forensic_identity(args: &QuorumArgs) -> (String, String) {
    if let Some(grant_b64) = args
        .access_grant_b64
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        if let Some((wallet, kid)) = grant_owner_and_content(grant_b64) {
            let content = if kid.trim().is_empty() {
                short_id(&args.object_cid)
            } else {
                short_id(&kid)
            };
            // FULL wallet address — a half-elided address is not directly traceable on-chain; the
            // complete `0x…` can be looked up + matched against the access grant. (The invisible
            // forensic layer carries the identical string, so a leaked frame is attributable.)
            return (normalize_addr(&wallet), content);
        }
    }
    (elide_principal(&args.principal), short_id(&args.object_cid))
}

/// Decode the base64 [`AccessGrantV1`] JSON and read the wallet `owner_address` + `kid_hex`
/// (on-chain content id) from its delegation. Returns `None` if the grant is not parseable (the
/// caller then falls back to the principal-derived stamp).
fn grant_owner_and_content(grant_b64: &str) -> Option<(String, String)> {
    let bytes = B64.decode(grant_b64.trim()).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    let delegation = v.get("delegation")?;
    let owner = delegation.get("owner_address")?.as_str()?.to_string();
    let kid = delegation
        .get("kid_hex")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Some((owner, kid))
}

/// Normalise an EVM wallet address for the stamp: trim and lower-case the `0x…` hex so the FULL,
/// directly-searchable address is shown (no elision) and renders consistently. Non-`0x` inputs
/// pass through trimmed.
fn normalize_addr(addr: &str) -> String {
    let a = addr.trim();
    if let Some(h) = a.strip_prefix("0x").or_else(|| a.strip_prefix("0X")) {
        format!("0x{}", h.to_ascii_lowercase())
    } else {
        a.to_string()
    }
}

/// Short content id: strip any `0x` and take the leading 10 hex chars (enough to disambiguate an
/// asset in a marketplace of this size while keeping the stamp compact).
fn short_id(id: &str) -> String {
    let s = id.trim();
    let body = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    let take = body.chars().take(10).collect::<String>();
    if take.is_empty() {
        s.to_string()
    } else {
        take
    }
}

/// Elide a long principal id to `first..last` (the no-wallet fallback identity).
fn elide_principal(principal: &str) -> String {
    let p = principal.trim();
    if p.len() <= 22 {
        p.to_string()
    } else {
        format!("{}..{}", &p[..10], &p[p.len() - 8..])
    }
}

/// Format a unix timestamp as a compact UTC `YYYY-MM-DD HH:MMZ` (minute precision is enough for a
/// per-open forensic mark, and avoids implying false precision). Dependency-free civil-date
/// conversion (Howard Hinnant's algorithm) so the helper pulls no date crate.
fn format_utc_minute(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (hour, minute) = (rem / 3600, (rem % 3600) / 60);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02} {hour:02}:{minute:02}Z")
}

/// Convert days-since-Unix-epoch to `(year, month, day)` in the proleptic Gregorian calendar.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `--quorum` entrypoint: recover the asset once, print the object descriptor, then serve the
/// decrypted bytes over the same stdio protocol the gateway's object viewer already drives.
pub fn run_quorum(args: QuorumArgs) -> Result<(), String> {
    let mut open = recover_quorum(&args)?;

    // MEDIA (audio/video): the CEK was recovered over the DASH segment set. Serve the
    // CENC-decrypted fMP4 fragments to the MSE player (init + per-index segments) over the
    // SAME descriptor/segment protocol the local media path uses, so the gateway relay is shared.
    if open.media.is_some() {
        return serve_media(&mut open, &args);
    }

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

/// Serve a quorum-recovered DASH asset to the MSE player: publish a multi-track media descriptor
/// (`tracks[]`, each with its own mime + clear init + per-track segment count) then answer
/// `{"op":"segment","track":T,"index":I}` by CENC-decrypting that track's fragment in the warm
/// decrypt boundary under the recovered CEK. A `(track, local_index)` is mapped to its GLOBAL
/// (flat) CENC-counter position before decrypt, so the continuous-counter seal still verifies.
/// Audio is the single-track degenerate case (`tracks.len() == 1`). The recovered CEK never
/// leaves the decrypt VM.
fn serve_media(open: &mut QuorumOpen, args: &QuorumArgs) -> Result<(), String> {
    // Snapshot the track model up front: the descriptor tracks (key-free) and each track's global
    // flat indices. Snapshotting lets the segment loop take `&mut open` without holding a borrow of
    // `open.media`.
    let (descriptor_tracks, track_global): (Vec<Value>, Vec<Vec<usize>>) = {
        let m = open
            .media
            .as_ref()
            .ok_or("serve_media called without recovered media info")?;
        let mut desc = Vec::with_capacity(m.tracks.len());
        let mut globals = Vec::with_capacity(m.tracks.len());
        for t in &m.tracks {
            desc.push(json!({
                "kind": t.kind,
                "mime": t.mime,
                "init_b64": B64.encode(&t.init),
                "segment_count": t.global_indices.len(),
            }));
            globals.push(t.global_indices.clone());
        }
        (desc, globals)
    };

    // Emit the LEGACY single-track fields (from track 0) alongside `tracks[]`. A gateway that
    // predates multi-track reads the top-level `mime`/`segment_count`/`init_b64` and still plays
    // (track 0 = the whole asset for audio); a multi-track-aware gateway reads `tracks[]`. This
    // keeps the descriptor robust to gateway/helper version skew (the helper is respawned per
    // open, the gateway is long-lived) instead of failing closed on a missing field.
    let mut descriptor = json!({
        "schema": "elastos.media-authority.session/v1",
        "kind": "media",
        "tracks": descriptor_tracks,
        "expires_at": now_unix() + args.ttl_secs.max(60),
        // AV forensic state (chunk 3): honest signal of whether the served stream carries a per-buyer
        // variant selection. `false` for single-encode assets — never claim a mark that isn't there.
        "fingerprinted": open.fingerprinted,
    });
    if let Some(first) = descriptor
        .get("tracks")
        .and_then(|t| t.as_array())
        .and_then(|a| a.first())
        .cloned()
    {
        descriptor["mime"] = first.get("mime").cloned().unwrap_or(Value::Null);
        descriptor["segment_count"] = first.get("segment_count").cloned().unwrap_or(json!(0));
        descriptor["init_b64"] = first.get("init_b64").cloned().unwrap_or(Value::Null);
    }
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
            Some("segment") => {
                // `track` defaults to 0 (single-track / legacy callers). `index` is the track-local
                // segment number; we map it to the global flat CENC-counter position to decrypt.
                let track = req.get("track").and_then(Value::as_u64).unwrap_or(0) as usize;
                let index = req.get("index").and_then(Value::as_u64).unwrap_or(u64::MAX) as usize;
                let Some(globals) = track_global.get(track) else {
                    reply(
                        &mut out,
                        &json!({"status": "error", "message": "track index out of range"}),
                    )?;
                    continue;
                };
                let Some(&global) = globals.get(index) else {
                    reply(
                        &mut out,
                        &json!({"status": "error", "message": "segment index past end of track"}),
                    )?;
                    continue;
                };
                match open.decrypt_media_segment(global) {
                    Ok(bytes) => reply(
                        &mut out,
                        &json!({ "status": "ok", "segment_b64": B64.encode(&bytes) }),
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

/// Serve a pixel-lock asset as on-demand, watermarked page images. The descriptor advertises
/// `pixel_locked: true` + `total_pages`; the gateway's page route drives `{"op":"page","n":I}`.
/// The decrypt boundary stays warm so each page renders in-VM without another quorum recovery.
fn serve_pixel_lock(open: &mut QuorumOpen, args: &QuorumArgs) -> Result<(), String> {
    // Stamp = wallet (from the signed grant) · short content id · UTC open minute. Built ONCE so
    // every page of this open carries the same per-open mark (constant open time across pages).
    let watermark = watermark_for(args, now_unix());

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

#[cfg(test)]
mod av_selection_tests {
    use super::*;
    use ddrm_envelope::av;

    const OBJECT_CID: &str = "owned:av-selection-test";
    const MASTER: &[u8] = b"node-bias-master-secret-0001";

    /// `(variant-A bytes, variant-B bytes)` for one marked segment.
    type VariantPair = (Vec<u8>, Vec<u8>);

    fn unique_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "ddrm-av-sel-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn grant(sig: &str) -> String {
        B64.encode(serde_json::to_vec(&json!({ "delegation_sig_hex": sig })).unwrap())
    }

    fn av_args(
        dir: &std::path::Path,
        master_b64: Option<String>,
        grant_b64: Option<String>,
    ) -> QuorumArgs {
        QuorumArgs {
            principal: "person:test".into(),
            decrypt_bin: String::new(),
            key_bin: String::new(),
            capsule_path: String::new(),
            descriptor_path: String::new(),
            caller_seed_b64: String::new(),
            object_cid: OBJECT_CID.into(),
            mime: "video/mp4".into(),
            ttl_secs: 60,
            dash_dir: Some(dir.to_string_lossy().into_owned()),
            access_grant_b64: grant_b64,
            av_master_b64: master_b64,
        }
    }

    /// Mint a fingerprinted DASH dir (manifest + per-segment A/B variant files) keyed by the SAME
    /// secret the serve selector derives from `(MASTER, OBJECT_CID)`. Returns the default (all-A)
    /// served segments + the (A,B) byte pairs so a test can check which variant was selected.
    fn mint_fixture(dir: &std::path::Path, nseg: usize) -> (Vec<Vec<u8>>, Vec<VariantPair>) {
        let asset_secret = av::asset_secret_from_master(MASTER, OBJECT_CID.as_bytes());
        let mut marked = Vec::new();
        let mut default_segments = Vec::new();
        let mut pairs = Vec::new();
        for i in 0..nseg {
            let a = format!("seg-{i}-A-ciphertext-bytes").into_bytes();
            let b = format!("seg-{i}-B-ciphertext-bytes").into_bytes();
            let (uri_a, uri_b) = (format!("seg-{i}.A.m4s"), format!("seg-{i}.B.m4s"));
            std::fs::write(dir.join(&uri_a), &a).unwrap();
            std::fs::write(dir.join(&uri_b), &b).unwrap();
            marked.push((
                i as u32,
                vec![
                    av::VariantRef {
                        symbol: 0,
                        uri: uri_a,
                        digest_hex: format!("da{i}"),
                    },
                    av::VariantRef {
                        symbol: 1,
                        uri: uri_b,
                        digest_hex: format!("db{i}"),
                    },
                ],
            ));
            default_segments.push(a.clone());
            pairs.push((a, b));
        }
        let manifest = av::build_manifest(2, 2.0, &asset_secret, &marked).unwrap();
        std::fs::write(
            dir.join(AV_VARIANTS_FILE),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        (default_segments, pairs)
    }

    fn symbols_for(grant_sig: &str, nseg: usize) -> Vec<u8> {
        let asset_secret = av::asset_secret_from_master(MASTER, OBJECT_CID.as_bytes());
        let bias = av::asset_bias_vector(&asset_secret, nseg as u32);
        let gd = ddrm_envelope::grant_watermark_digest16(grant_sig);
        // build a throwaway manifest only to satisfy select_symbols' shape (commitment matches).
        let marked: Vec<(u32, Vec<av::VariantRef>)> = (0..nseg as u32)
            .map(|i| {
                (
                    i,
                    vec![
                        av::VariantRef {
                            symbol: 0,
                            uri: String::new(),
                            digest_hex: "x".into(),
                        },
                        av::VariantRef {
                            symbol: 1,
                            uri: String::new(),
                            digest_hex: "y".into(),
                        },
                    ],
                )
            })
            .collect();
        let m = av::build_manifest(2, 2.0, &asset_secret, &marked).unwrap();
        av::select_symbols(&m, &bias, &gd).unwrap()
    }

    #[test]
    fn serve_selection_diverges_per_buyer_and_matches_the_codeword() {
        let dir = unique_dir("diverge");
        let nseg = 16usize;
        let (default_segments, pairs) = mint_fixture(&dir, nseg);
        let master_b64 = B64.encode(MASTER);

        let args_a = av_args(&dir, Some(master_b64.clone()), Some(grant("0xalice")));
        let args_b = av_args(&dir, Some(master_b64), Some(grant("0xbob")));

        let mut seg_a = default_segments.clone();
        let mut seg_b = default_segments.clone();
        assert!(apply_variant_selection(&dir, &mut seg_a, &args_a).unwrap());
        assert!(apply_variant_selection(&dir, &mut seg_b, &args_b).unwrap());

        let sym_a = symbols_for("0xalice", nseg);
        let sym_b = symbols_for("0xbob", nseg);
        assert_ne!(sym_a, sym_b, "distinct buyers select distinct variant sets");
        assert_ne!(seg_a, seg_b, "served bytes diverge between buyers");

        // Each served segment is exactly the variant the buyer's codeword selects.
        for i in 0..nseg {
            let (a, b) = &pairs[i];
            let want_a = if sym_a[i] == 0 { a } else { b };
            let want_b = if sym_b[i] == 0 { a } else { b };
            assert_eq!(&seg_a[i], want_a, "buyer A segment {i}");
            assert_eq!(&seg_b[i], want_b, "buyer B segment {i}");
            if sym_a[i] == sym_b[i] {
                assert_eq!(
                    seg_a[i], seg_b[i],
                    "same symbol ⇒ identical served bytes at {i}"
                );
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn serve_falls_back_to_single_encode_when_not_provisioned() {
        let dir = unique_dir("fallback");
        let (default_segments, _) = mint_fixture(&dir, 8);

        // No master ⇒ honest single encode (segments untouched, fingerprinted=false).
        let mut segs = default_segments.clone();
        let args = av_args(&dir, None, Some(grant("0xalice")));
        assert!(!apply_variant_selection(&dir, &mut segs, &args).unwrap());
        assert_eq!(segs, default_segments, "no master ⇒ served bytes unchanged");

        // Master but no grant ⇒ can't select per-buyer ⇒ single encode.
        let mut segs2 = default_segments.clone();
        let args2 = av_args(&dir, Some(B64.encode(MASTER)), None);
        assert!(!apply_variant_selection(&dir, &mut segs2, &args2).unwrap());
        assert_eq!(segs2, default_segments);

        // No manifest at all ⇒ single encode.
        let empty = unique_dir("empty");
        let mut segs3 = default_segments.clone();
        let args3 = av_args(&empty, Some(B64.encode(MASTER)), Some(grant("0xalice")));
        assert!(!apply_variant_selection(&empty, &mut segs3, &args3).unwrap());
        assert_eq!(segs3, default_segments);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn serve_fails_closed_on_wrong_master() {
        // Provisioned + grant + fingerprinted manifest, but the WRONG master ⇒ bias-commitment
        // mismatch ⇒ refuse to serve (never a half-applied or mis-keyed mark).
        let dir = unique_dir("wrongmaster");
        let (default_segments, _) = mint_fixture(&dir, 8);
        let mut segs = default_segments.clone();
        let wrong = B64.encode(b"a-different-node-master-secret");
        let args = av_args(&dir, Some(wrong), Some(grant("0xalice")));
        assert!(apply_variant_selection(&dir, &mut segs, &args).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod watermark_tests {
    use super::*;

    fn args_with_grant(grant_b64: Option<String>) -> QuorumArgs {
        QuorumArgs {
            principal: "person:local:0123456789abcdef0123456789abcdef".to_string(),
            decrypt_bin: String::new(),
            key_bin: String::new(),
            capsule_path: String::new(),
            descriptor_path: String::new(),
            caller_seed_b64: String::new(),
            object_cid: "0xCAFEBABEdeadbeef00112233".to_string(),
            mime: "application/pdf".to_string(),
            ttl_secs: 60,
            dash_dir: None,
            access_grant_b64: grant_b64,
            av_master_b64: None,
        }
    }

    fn grant_b64(owner: &str, kid: &str) -> String {
        let v = json!({ "delegation": { "owner_address": owner, "kid_hex": kid } });
        B64.encode(serde_json::to_vec(&v).unwrap())
    }

    #[test]
    fn normalize_addr_keeps_full_lowercased_address() {
        assert_eq!(
            normalize_addr("0x1234567890ABCDEF1234567890abcdef12345678"),
            "0x1234567890abcdef1234567890abcdef12345678"
        );
        // Non-0x inputs pass through trimmed.
        assert_eq!(normalize_addr("  anon  "), "anon");
    }

    #[test]
    fn short_id_strips_0x_and_truncates() {
        assert_eq!(short_id("0xCAFEBABEdeadbeef"), "CAFEBABEde");
        assert_eq!(short_id("short"), "short");
    }

    #[test]
    fn utc_minute_formats_known_epoch() {
        // 1700000000 == 2023-11-14 22:13:20 UTC (well-known epoch).
        assert_eq!(format_utc_minute(1_700_000_000), "2023-11-14 22:13Z");
        // Unix epoch.
        assert_eq!(format_utc_minute(0), "1970-01-01 00:00Z");
    }

    #[test]
    fn full_stamp_uses_the_open_time() {
        let stamp = watermark_for(&args_with_grant(None), 1_700_000_000);
        assert!(stamp.ends_with("2023-11-14 22:13Z"), "stamp: {stamp}");
    }

    #[test]
    fn stamp_prefers_wallet_and_content_from_grant() {
        let args = args_with_grant(Some(grant_b64(
            "0xAbC1230000000000000000000000000000009f0e",
            "0xdeadbeefcafef00d",
        )));
        let (identity, content) = forensic_identity(&args);
        assert_eq!(identity, "0xabc1230000000000000000000000000000009f0e");
        assert_eq!(content, "deadbeefca");
    }

    #[test]
    fn stamp_falls_back_to_principal_without_a_grant() {
        let args = args_with_grant(None);
        let (identity, content) = forensic_identity(&args);
        // Principal is elided (first10..last8); content from the object cid.
        assert!(identity.contains(".."));
        assert_eq!(content, "CAFEBABEde");
    }

    #[test]
    fn full_stamp_is_never_empty() {
        let stamp = watermark_for(&args_with_grant(None), 1_781_516_433);
        assert!(stamp.contains('\u{00b7}'));
        assert!(!stamp.trim().is_empty());
    }

    fn signed_grant_b64(owner: &str, kid: &str, sig_hex: &str) -> String {
        let v = json!({
            "delegation": { "owner_address": owner, "kid_hex": kid },
            "delegation_sig_hex": sig_hex,
        });
        B64.encode(serde_json::to_vec(&v).unwrap())
    }

    fn signed_args(sig_hex: &str) -> QuorumArgs {
        args_with_grant(Some(signed_grant_b64(
            "0xAbC1230000000000000000000000000000009f0e",
            "0xdeadbeefcafef00d",
            sig_hex,
        )))
    }

    #[test]
    fn no_grant_digest_token_without_a_signature() {
        // A grant without `delegation_sig_hex` (or no grant) yields the plain human stamp.
        assert!(grant_digest_hex(&args_with_grant(None)).is_none());
        let unsigned = args_with_grant(Some(grant_b64("0xabc", "0xdead")));
        assert!(grant_digest_hex(&unsigned).is_none());
        assert!(!watermark_for(&unsigned, 1_700_000_000).contains('\u{1F}'));
    }

    #[test]
    fn signed_grant_appends_authenticated_digest_token() {
        let sig = "0x1234567890abcdef";
        let args = signed_args(sig);
        // The token's hex matches the SHARED digest fn (no drift with the decrypt-provider verifier).
        let expect: String = ddrm_envelope::grant_watermark_digest16(sig)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(grant_digest_hex(&args).as_deref(), Some(expect.as_str()));

        let stamp = watermark_for(&args, 1_700_000_000);
        let (human, token) = stamp.split_once('\u{1F}').expect("grant token present");
        // Human half stays a clean `wallet · content · time` stamp.
        assert!(human.ends_with("2023-11-14 22:13Z"));
        assert!(!human.contains('\u{1F}'));
        assert_eq!(token, format!("gd:{expect}"));
    }
}
