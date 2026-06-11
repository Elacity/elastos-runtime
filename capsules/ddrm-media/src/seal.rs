//! The local key-authority: turn a fragmented MP4 into a playable, transcript-bound
//! decrypt session against a freshly launched `decrypt-provider`.
//!
//! This is the exact `ddrm-envelope` handshake a dKMS / `key-provider` performs,
//! done locally so an owned video plays end-to-end without Lit or an external KMS:
//!   1. CENC-pack the asset under a fresh random CEK (unique per-sample IVs).
//!   2. Launch the provider; it mints + publishes an in-VM session key.
//!   3. Seal the CEK to that session key, bound to the full decrypt transcript.
//!   4. Zeroize the raw CEK; emit only CEK-free sealed material + the request.

use base64::Engine as _;
use rand::RngCore;
use serde_json::{json, Value};

use ddrm_envelope::seal::{mldsa_seal_keypair, seal_bound};
use ddrm_envelope::transcript::{release_receipt_hash, DecryptTranscriptV1};
use ddrm_envelope::{segment_digests, session_public_from_bytes, SUITE_PQ_HYBRID};

use crate::mp4;
use crate::rail::DecryptProviderProc;

/// The media viewer interface the player satisfies (matches `viewer_media`).
pub const VIEWER_INTERFACE: &str = "elastos.viewer/media@1";
const RELEASE_SCHEMA: &str = "elastos.release.receipt/v1";
const RELEASE_PROVIDER: &str = "key-provider";
const RELEASE_STATUS: &str = "released";

/// Caller-supplied transcript identity for a media decrypt session. Everything here
/// is bound into the sealed material's AAD, so a substituted field fails closed.
#[derive(Debug, Clone)]
pub struct SessionParams {
    /// The owning principal (bound into the transcript + release receipt).
    pub principal_id: String,
    /// The decrypt transcript session id.
    pub session_id: String,
    /// The content identifier of the owned object.
    pub object_cid: String,
    /// Opaque request id for the decrypt request.
    pub request_id: String,
    /// Opaque request id for the release receipt.
    pub release_request_id: String,
    /// Seconds from now until the session (and every segment read) fails closed.
    pub ttl_secs: u64,
    /// Human-readable reason recorded on the request.
    pub reason: String,
}

impl SessionParams {
    /// Sensible defaults for an owned-video play session.
    pub fn for_object(principal_id: impl Into<String>, object_cid: impl Into<String>) -> Self {
        Self {
            principal_id: principal_id.into(),
            session_id: format!("media-{}", random_token()),
            object_cid: object_cid.into(),
            request_id: format!("decrypt-{}", random_token()),
            release_request_id: format!("release-{}", random_token()),
            ttl_secs: 3600,
            reason: "owned media playback".to_string(),
        }
    }
}

/// A prepared, ready-to-serve media decrypt session. Holds ONLY the CEK-free sealed
/// material + the clear init segment + the live provider; never the CEK, never any
/// decrypted media (each segment is decrypted on demand via [`DecryptProviderProc`]).
pub struct PreparedSession {
    /// MSE `addSourceBuffer` mime/codecs string.
    pub mime: String,
    /// The clear init segment bytes (CENC init/`moov` is unencrypted).
    pub init: Vec<u8>,
    /// The CEK-free sealed material relayed to the provider per segment.
    pub material: Value,
    /// The authenticated decrypt request (no key material).
    pub request: Value,
    /// Number of addressable media segments.
    pub segment_count: usize,
    /// Unix expiry; reads after this fail closed.
    pub expires_at: u64,
    /// The live `decrypt-provider` that unwraps + decrypts each segment in-VM.
    pub provider: DecryptProviderProc,
}

impl PreparedSession {
    /// Decrypt one media segment in-VM and strip its `senc` so it is browser-ready.
    pub fn decrypt_segment_clean(&self, index: usize, now_unix: u64) -> Result<Vec<u8>, String> {
        let decrypted = self
            .provider
            .stream_segment(&self.request, &self.material, index, now_unix)?;
        mp4::strip_senc(&decrypted)
    }
}

/// Prepare a transcript-bound media session from a fragmented MP4.
///
/// `fragmented_mp4` must already be a fragmented MP4 (init `moov` with
/// `+empty_moov` + `moof`/`mdat` fragments). `decrypt_bin` is the path to a
/// `rail-stream` + `rail-mint` `decrypt-provider` binary. `now_unix` seeds the
/// expiry + receipt timestamps.
pub fn prepare(
    fragmented_mp4: &[u8],
    decrypt_bin: &str,
    params: &SessionParams,
    now_unix: u64,
) -> Result<PreparedSession, String> {
    let split = mp4::split_fragmented(fragmented_mp4)?;
    if split.fragments.is_empty() {
        return Err("asset has no media fragments".to_string());
    }
    let mime = format!("video/mp4; codecs=\"{}\"", mp4::avc_codec_string(&split.init));

    // Fresh CEK + a local key-authority seal identity (ML-DSA-65).
    let mut cek = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut cek);
    let mut seal_seed = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seal_seed);
    let (signer, authority_vk) = mldsa_seal_keypair(seal_seed);
    let b64 = base64::engine::general_purpose::STANDARD;
    let authority_vk_b64 = b64.encode(authority_vk);

    // CENC-encrypt every fragment under the CEK (globally-unique per-sample IVs).
    let mut iv_counter: u64 = 1;
    let mut encrypted: Vec<Vec<u8>> = Vec::with_capacity(split.fragments.len());
    for frag in &split.fragments {
        encrypted.push(mp4::encrypt_fragment(frag, &cek, &mut iv_counter)?);
    }

    // Launch the provider; it mints + publishes its in-VM session key.
    let provider = DecryptProviderProc::launch(decrypt_bin, &authority_vk_b64)?;
    let session_pub_bytes = b64
        .decode(&provider.session_pub_b64)
        .map_err(|e| format!("decode session pub: {e}"))?;
    let public = session_public_from_bytes(&session_pub_bytes)
        .ok_or("could not parse the provider's published session key")?;

    // Seal the CEK to the published session key, bound to the decrypt transcript.
    let mut nonce = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut nonce);
    let mut content_hash = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut content_hash);
    let expires_at = now_unix + params.ttl_secs;
    let issued_at = now_unix;
    let rr_hash = release_receipt_hash(
        RELEASE_SCHEMA,
        &params.release_request_id,
        &params.object_cid,
        &params.principal_id,
        &params.session_id,
        "stream",
        RELEASE_PROVIDER,
        RELEASE_STATUS,
        issued_at,
        expires_at,
    );

    let seg_refs: Vec<&[u8]> = encrypted.iter().map(|s| s.as_slice()).collect();
    let digests = segment_digests(&seg_refs);
    let aad = DecryptTranscriptV1 {
        suite_id: SUITE_PQ_HYBRID,
        provider_id: "decrypt-provider",
        principal_id: &params.principal_id,
        session_id: &params.session_id,
        object_cid: &params.object_cid,
        content_hash: &content_hash,
        action: "stream",
        viewer_interface: VIEWER_INTERFACE,
        output_kind: "stream",
        expires_at,
        release_receipt_hash: rr_hash,
        decrypt_session_pub: &session_pub_bytes,
        nonce: &nonce,
        node_set_id: None,
    }
    .to_aad_with_segments(Some(&digests));
    let sealed = seal_bound(&public, &cek, &aad, &signer).to_bytes();
    // Scrub the CEK now that it is sealed — it never leaves this process unsealed.
    cek.iter_mut().for_each(|byte| *byte = 0);

    let material = json!({
        "suite": SUITE_PQ_HYBRID,
        "sealed_cek_b64": b64.encode(&sealed),
        "ciphertext_b64": b64.encode(&encrypted[0]),
        "init_segment_b64": Value::Null,
        "nonce_b64": b64.encode(nonce),
        "content_hash_b64": b64.encode(content_hash),
        "extra_segments_b64": encrypted[1..].iter().map(|s| b64.encode(s)).collect::<Vec<_>>(),
    });
    let request = json!({
        "schema": "elastos.decrypt.session.request/v1",
        "request_id": params.request_id,
        "principal_id": params.principal_id,
        "session_id": params.session_id,
        "object_cid": params.object_cid,
        "action": "stream",
        "viewer_interface": VIEWER_INTERFACE,
        "release_receipt": {
            "schema": RELEASE_SCHEMA,
            "request_id": params.release_request_id,
            "object_cid": params.object_cid,
            "principal_id": params.principal_id,
            "session_id": params.session_id,
            "action": "stream",
            "provider": RELEASE_PROVIDER,
            "status": RELEASE_STATUS,
            "issued_at": issued_at,
            "expires_at": expires_at,
        },
        "output_kind": "stream",
        "reason": params.reason,
        "expires_at": expires_at,
    });

    Ok(PreparedSession {
        mime,
        init: split.init,
        material,
        request,
        segment_count: encrypted.len(),
        expires_at,
        provider,
    })
}

fn random_token() -> String {
    let mut bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}
