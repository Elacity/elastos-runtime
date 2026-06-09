//! ElastOS Decrypt Provider Capsule
//!
//! Fail-closed protected-content decrypt/render boundary. App capsules never
//! receive raw CEKs, broad plaintext authority, filesystem authority,
//! key-backend SDK objects, KMS credentials, chain RPC, wallet RPC, or provider credentials
//! through this provider.

use elastos_common::protected_content::{
    DecryptSessionRequestV1, ReleaseReceiptV1, DECRYPT_SESSION_REQUEST_SCHEMA,
    DECRYPT_SESSION_SCHEMA, PROTECTED_CONTENT_ACTIONS, PROTECTED_CONTENT_OUTPUTS,
    RELEASE_RECEIPT_SCHEMA,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

// CENC/AES-128-CTR decrypt engine vendored from PC2 `cenc-decrypt`. Held here as a
// provider-internal backend; wired into open_session/render behind the fail-closed
// contract in a later step (see docs/convergence/CONVERGENCE_PLAYBOOK.md §6).
#[allow(dead_code)]
mod cenc;
mod envelope;
// PQ-hybrid CEK-seal de-risking island (feature `pq-envelope`): the post-quantum
// analogue of `envelope.rs`, proving x25519+ml-kem-768 -> AEAD unwrap recovers a
// CEK in `Zeroizing`. Not wired into dispatch; see DDRM_DECRYPT_RAIL.md §PQ.
#[cfg(feature = "pq-envelope")]
mod pq_envelope;
// Portable golden-vector schema (features `vectors` / `rail-shim`):
// substrate-independent fixtures the engines and the rail shim are replayed
// against. See src/vector_format.rs.
#[cfg(any(feature = "vectors", feature = "rail-shim", feature = "pq-mldsa"))]
mod vector_format;
// Rail transport shim (feature `rail-shim`): adapter from a sealed-CEK carrier to
// the proven unwrap->cenc engines. Tested island; wired into dispatch only under
// `rail-live` (via OpenSessionLive below).
#[cfg(feature = "rail-shim")]
mod rail_shim;

const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Init {
        #[serde(default)]
        config: Value,
    },
    Status,
    OpenSession {
        request: Box<DecryptSessionRequestV1>,
    },
    Render {
        request: Box<DecryptSessionRequestV1>,
    },
    // Live decrypt rail (feature `rail-live`, DDRM_DECRYPT_RAIL.md Option A): the
    // VM-sealed material rides a capsule-LOCAL variant so the shared
    // `DecryptSessionRequestV1` contract stays byte-identical. When Anders blesses a
    // `material`/`sealed_cek` field on the public contract, this folds into the
    // normal `OpenSession` and this variant is removed.
    #[cfg(feature = "rail-live")]
    OpenSessionLive {
        request: Box<DecryptSessionRequestV1>,
        material: RailMaterial,
    },
    // Transcript-bound live rail (feature `rail-bind`, Anders Day-45 decision): the
    // sealed material is cryptographically welded to the full request transcript.
    #[cfg(feature = "rail-bind")]
    OpenSessionBound {
        request: Box<DecryptSessionRequestV1>,
        material: BoundRailMaterial,
    },
    Shutdown,
}

/// The VM-sealed decrypt material delivered on the live rail (Option A). Carries
/// only sealed/public bytes — never a raw CEK. The VM's session secret is held
/// in-VM (provisioned, never on the wire), mirroring PC2's `unwrap_envelope`.
#[cfg(feature = "rail-live")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RailMaterial {
    /// "pq_hybrid" (shipped target) or "classical_p256" (PC2-migration parity).
    profile: String,
    /// CEK sealed to the VM session key (PqSealedEnvelope wire form / classical blob), base64.
    sealed_cek_b64: String,
    /// The ciphertext fMP4 segment to decrypt, base64.
    ciphertext_b64: String,
    /// Optional init segment (e.g. `tenc` IV defaults), base64.
    #[serde(default)]
    init_segment_b64: Option<String>,
}

/// Transcript-bound decrypt material (feature `rail-bind`). Extends `RailMaterial`
/// with the replay nonce and the object content hash that the sealed CEK is bound
/// to (the remaining transcript fields are taken from the authenticated request +
/// the boundary's own provisioned state, never trusted from the carrier).
#[cfg(feature = "rail-bind")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundRailMaterial {
    sealed_cek_b64: String,
    ciphertext_b64: String,
    #[serde(default)]
    init_segment_b64: Option<String>,
    /// Per-release replay nonce (key-authority chosen); bound into the transcript.
    nonce_b64: String,
    /// Object content hash (binds the CEK to THIS content; Anders' "object CID/
    /// content hash"), base64.
    content_hash_b64: String,
}

/// Canonical decrypt transcript (feature `rail-bind`) — the exact field set Anders
/// requires the sealed material to bind (Day-45 decision): principal, session,
/// object CID + content hash, action, viewer interface, output kind, expiry,
/// release-receipt hash, decrypt-session public key, algorithm suite, provider
/// identity, and a replay nonce. `to_aad()` is a domain-separated, length-prefixed
/// encoding used as the AES-256-GCM AAD and signed alongside the envelope, so any
/// mismatch fails closed before a CEK exists.
#[cfg(feature = "rail-bind")]
struct DecryptTranscriptV1<'a> {
    suite_id: &'a str,
    provider_id: &'a str,
    principal_id: &'a str,
    session_id: &'a str,
    object_cid: &'a str,
    content_hash: &'a [u8],
    action: &'a str,
    viewer_interface: &'a str,
    output_kind: &'a str,
    expires_at: u64,
    release_receipt_hash: [u8; 32],
    decrypt_session_pub: &'a [u8],
    nonce: &'a [u8],
}

#[cfg(feature = "rail-bind")]
const DECRYPT_TRANSCRIPT_LABEL: &[u8] = b"elastos-ddrm/decrypt-transcript/v1";
#[cfg(feature = "rail-bind")]
const DECRYPT_SUITE_ID: &str = "elastos-pq-hybrid-threshold-v0";
#[cfg(feature = "rail-bind")]
const DECRYPT_PROVIDER_ID: &str = "decrypt-provider";

#[cfg(feature = "rail-bind")]
impl DecryptTranscriptV1<'_> {
    /// Deterministic, unambiguous AAD: a domain label then every field
    /// length-prefixed (be32 len ‖ bytes) / fixed-width, so no two distinct
    /// transcripts can collide and no field can be slid into another.
    fn to_aad(&self) -> Vec<u8> {
        let mut v = Vec::new();
        let mut put = |bytes: &[u8]| {
            v.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            v.extend_from_slice(bytes);
        };
        put(DECRYPT_TRANSCRIPT_LABEL);
        put(self.suite_id.as_bytes());
        put(self.provider_id.as_bytes());
        put(self.principal_id.as_bytes());
        put(self.session_id.as_bytes());
        put(self.object_cid.as_bytes());
        put(self.content_hash);
        put(self.action.as_bytes());
        put(self.viewer_interface.as_bytes());
        put(self.output_kind.as_bytes());
        put(&self.expires_at.to_be_bytes());
        put(&self.release_receipt_hash);
        put(self.decrypt_session_pub);
        put(self.nonce);
        v
    }
}

/// Bind the release receipt into the transcript by hashing its identifying fields
/// (Anders: "release receipt hash"). Deterministic + domain-separated.
#[cfg(feature = "rail-bind")]
fn release_receipt_hash(receipt: &ReleaseReceiptV1) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"elastos-ddrm/release-receipt/v1");
    for field in [
        receipt.schema.as_str(),
        receipt.request_id.as_str(),
        receipt.object_cid.as_str(),
        receipt.principal_id.as_str(),
        receipt.session_id.as_str(),
        receipt.action.as_str(),
        receipt.provider.as_str(),
        receipt.status.as_str(),
    ] {
        h.update((field.len() as u32).to_be_bytes());
        h.update(field.as_bytes());
    }
    h.update(receipt.issued_at.to_be_bytes());
    h.update(receipt.expires_at.to_be_bytes());
    h.finalize().into()
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Response {
    Ok {
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<Value>,
    },
    Error {
        code: String,
        message: String,
    },
}

impl Response {
    fn ok(data: Value) -> Self {
        Response::Ok { data: Some(data) }
    }

    fn empty_ok() -> Self {
        Response::Ok { data: None }
    }

    fn error(code: &str, message: impl Into<String>) -> Self {
        Response::Error {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

#[derive(Default)]
struct DecryptProvider {
    // Live-rail state (feature `rail-live`). The session secret is VM-minted/
    // provisioned and never leaves the boundary; `authority_vk` is the trusted
    // key-authority ML-DSA-65 verifying key the seal signature is checked against.
    #[cfg(feature = "rail-live")]
    session: Option<rail_shim::SessionSecret>,
    #[cfg(feature = "rail-live")]
    authority_vk: Option<Vec<u8>>,
    // The published decrypt-session public key (transcript binding, `rail-bind`).
    // It is minted in-sandbox and is what the key authority seals the CEK to; it
    // is bound into the transcript so a carrier cannot be replayed against a
    // different session key.
    #[cfg(feature = "rail-bind")]
    session_pub: Option<Vec<u8>>,
}

impl DecryptProvider {
    fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::Init { config } => self.init(config),
            Request::Status => self.status(),
            Request::OpenSession { request } => self.open_session(*request),
            Request::Render { request } => self.render(*request),
            #[cfg(feature = "rail-live")]
            Request::OpenSessionLive { request, material } => {
                self.open_session_live(*request, &material)
            }
            #[cfg(feature = "rail-bind")]
            Request::OpenSessionBound { request, material } => {
                self.open_session_bound(*request, &material)
            }
            Request::Shutdown => Response::empty_ok(),
        }
    }

    #[cfg(not(feature = "rail-mint"))]
    fn init(&mut self, _config: Value) -> Response {
        Response::ok(json!({
            "provider": "decrypt",
            "protocol_version": "1.0",
            "configured": false,
            "supported_operations": ["status", "open_session", "render"],
        }))
    }

    /// `init` under the in-sandbox mint (feature `rail-mint`, Anders' Day-45 ask):
    /// the boundary MINTS its own per-session hybrid KEM keypair, keeps the secret
    /// in-VM, and PUBLISHES the public key (+ suite) so the key authority can seal
    /// the CEK to it. The optional `authority_vk_b64` config pins the trusted
    /// key-authority verifying key. The secret never appears in the response.
    #[cfg(feature = "rail-mint")]
    fn init(&mut self, config: Value) -> Response {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;

        let (secret, public) = crate::pq_envelope::mint_session();
        let pub_bytes = crate::pq_envelope::session_public_bytes(&public);
        self.session = Some(rail_shim::SessionSecret::PqHybrid(secret));
        self.session_pub = Some(pub_bytes.clone());

        if let Some(vk_b64) = config.get("authority_vk_b64").and_then(Value::as_str) {
            match b64.decode(vk_b64) {
                Ok(vk) => self.authority_vk = Some(vk),
                Err(_) => return Response::error("invalid_request", "authority_vk_b64 is not valid base64"),
            }
        }

        Response::ok(json!({
            "provider": "decrypt",
            "protocol_version": "1.0",
            "configured": self.authority_vk.is_some(),
            "supported_operations": ["status", "open_session", "render", "open_session_bound"],
            "suite": "elastos-pq-hybrid-threshold-v0",
            // The freshly-minted, in-sandbox session public key the key authority
            // seals the CEK to. The matching secret never leaves this boundary.
            "decrypt_session_public_key_b64": b64.encode(&pub_bytes),
        }))
    }

    fn status(&self) -> Response {
        Response::ok(json!({
            "provider": "decrypt",
            "version": PROVIDER_VERSION,
            "configured": false,
            "supported_operations": ["status", "open_session", "render"],
            "supported_outputs": PROTECTED_CONTENT_OUTPUTS,
            "blocked_authority": [
                "raw_cek",
                "raw_plaintext",
                "filesystem",
                "key_backend_sdk",
                "kms_node_credentials",
                "chain_rpc",
                "wallet_rpc",
                "provider_credentials"
            ],
            "next_required_providers": [
                "key-provider"
            ],
        }))
    }

    fn open_session(&self, request: DecryptSessionRequestV1) -> Response {
        if let Err(err) = validate_decrypt_session_request(&request) {
            return Response::error("invalid_request", err);
        }
        Response::error(
            "not_configured",
            "decrypt sessions require a configured key release and decrypt/render backend",
        )
    }

    fn render(&self, request: DecryptSessionRequestV1) -> Response {
        if let Err(err) = validate_decrypt_session_request(&request) {
            return Response::error("invalid_request", err);
        }
        Response::error(
            "not_configured",
            "rendering requires a configured key release and decrypt/render backend",
        )
    }

    /// Live decrypt rail (feature `rail-live`, DDRM_DECRYPT_RAIL.md Option A): the
    /// single in-boundary operation the recommended rail performs. Recovers the CEK
    /// from the VM-sealed carrier via the proven `rail_shim::decrypt_from_carrier`,
    /// decrypts the segment, and returns the SCOPED response — which carries session/
    /// output metadata only. The recovered CEK lives in `Zeroizing` inside the engine
    /// and the plaintext is dropped here; neither ever reaches the caller-facing
    /// `Response`. Fails closed (coarse `decrypt_failed`) on any unprovisioned state,
    /// profile/secret mismatch, malformed carrier, wrong session, or bad signature.
    #[cfg(feature = "rail-live")]
    fn open_session_live(&self, request: DecryptSessionRequestV1, material: &RailMaterial) -> Response {
        use base64::Engine as _;

        if let Err(err) = validate_decrypt_session_request(&request) {
            return Response::error("invalid_request", err);
        }

        // Trust + key state must be provisioned (the VM minted its session; the
        // operator configured the trusted key-authority verifying key).
        let session = match self.session.as_ref() {
            Some(s) => s,
            None => {
                return Response::error(
                    "not_configured",
                    "decrypt session key is not provisioned in this boundary",
                )
            }
        };
        let authority_vk = match self.authority_vk.as_ref() {
            Some(vk) => vk,
            None => {
                return Response::error(
                    "not_configured",
                    "trusted key-authority verifying key is not configured",
                )
            }
        };

        let b64 = base64::engine::general_purpose::STANDARD;
        let profile = match material.profile.as_str() {
            "pq_hybrid" => rail_shim::SealProfile::PqHybrid,
            "classical_p256" => rail_shim::SealProfile::ClassicalP256,
            other => return Response::error("invalid_request", format!("unsupported seal profile: {other}")),
        };
        let sealed_cek = match b64.decode(&material.sealed_cek_b64) {
            Ok(bytes) => bytes,
            Err(_) => return Response::error("invalid_request", "sealed_cek_b64 is not valid base64"),
        };
        let ciphertext_segment = match b64.decode(&material.ciphertext_b64) {
            Ok(bytes) => bytes,
            Err(_) => return Response::error("invalid_request", "ciphertext_b64 is not valid base64"),
        };
        let init_segment = match material.init_segment_b64.as_deref().map(|s| b64.decode(s)) {
            None => None,
            Some(Ok(bytes)) => Some(bytes),
            Some(Err(_)) => return Response::error("invalid_request", "init_segment_b64 is not valid base64"),
        };

        let verifier = match crate::pq_envelope::mldsa::MlDsa65Verifier::from_encoded(authority_vk) {
            Some(v) => v,
            None => return Response::error("not_configured", "configured key-authority verifying key is malformed"),
        };

        let carrier = rail_shim::SealedDecryptCarrier {
            profile,
            sealed_cek,
            ciphertext_segment,
            init_segment,
        };

        // The CEK materializes only inside this call (Zeroizing) and is zeroized by
        // the cenc engine; `_plaintext` is dropped here and never surfaced.
        match rail_shim::decrypt_from_carrier(session, &carrier, &verifier) {
            Ok((_plaintext, meta)) => scoped_session_response(&request, &meta),
            // Coarse, uniform failure — never reveal which step failed (no oracle).
            Err(_) => Response::error("decrypt_failed", "decrypt session could not be opened"),
        }
    }

    /// Transcript-bound live rail (feature `rail-bind`, Anders Day-45 decision). Same
    /// in-boundary unwrap→decrypt→scoped-output as `open_session_live`, but the CEK
    /// is cryptographically WELDED to the full request transcript (AES-256-GCM AAD +
    /// ML-DSA-65 signature over `payload ‖ transcript`). The transcript is rebuilt
    /// here from the AUTHENTICATED request + the boundary's own provisioned session
    /// public key (never trusted from the carrier), so a CEK sealed for one
    /// (principal, session, object, receipt, session key, suite, provider, nonce)
    /// cannot be replayed against any other — it fails closed at the GCM tag /
    /// signature before any plaintext exists.
    #[cfg(feature = "rail-bind")]
    fn open_session_bound(&self, request: DecryptSessionRequestV1, material: &BoundRailMaterial) -> Response {
        use base64::Engine as _;

        if let Err(err) = validate_decrypt_session_request(&request) {
            return Response::error("invalid_request", err);
        }

        let session = match self.session.as_ref() {
            Some(s) => s,
            None => return Response::error("not_configured", "decrypt session key is not provisioned in this boundary"),
        };
        let session_pub = match self.session_pub.as_ref() {
            Some(p) => p,
            None => return Response::error("not_configured", "decrypt session public key is not published"),
        };
        let authority_vk = match self.authority_vk.as_ref() {
            Some(vk) => vk,
            None => return Response::error("not_configured", "trusted key-authority verifying key is not configured"),
        };

        let b64 = base64::engine::general_purpose::STANDARD;
        let sealed_cek = match b64.decode(&material.sealed_cek_b64) {
            Ok(b) => b,
            Err(_) => return Response::error("invalid_request", "sealed_cek_b64 is not valid base64"),
        };
        let ciphertext_segment = match b64.decode(&material.ciphertext_b64) {
            Ok(b) => b,
            Err(_) => return Response::error("invalid_request", "ciphertext_b64 is not valid base64"),
        };
        let init_segment = match material.init_segment_b64.as_deref().map(|s| b64.decode(s)) {
            None => None,
            Some(Ok(b)) => Some(b),
            Some(Err(_)) => return Response::error("invalid_request", "init_segment_b64 is not valid base64"),
        };
        let nonce = match b64.decode(&material.nonce_b64) {
            Ok(b) => b,
            Err(_) => return Response::error("invalid_request", "nonce_b64 is not valid base64"),
        };
        let content_hash = match b64.decode(&material.content_hash_b64) {
            Ok(b) => b,
            Err(_) => return Response::error("invalid_request", "content_hash_b64 is not valid base64"),
        };

        let verifier = match crate::pq_envelope::mldsa::MlDsa65Verifier::from_encoded(authority_vk) {
            Some(v) => v,
            None => return Response::error("not_configured", "configured key-authority verifying key is malformed"),
        };

        // Rebuild the transcript from the AUTHENTICATED request + provisioned state.
        let aad = DecryptTranscriptV1 {
            suite_id: DECRYPT_SUITE_ID,
            provider_id: DECRYPT_PROVIDER_ID,
            principal_id: &request.principal_id,
            session_id: &request.session_id,
            object_cid: &request.object_cid,
            content_hash: &content_hash,
            action: &request.action,
            viewer_interface: &request.viewer_interface,
            output_kind: &request.output_kind,
            expires_at: request.expires_at,
            release_receipt_hash: release_receipt_hash(&request.release_receipt),
            decrypt_session_pub: session_pub,
            nonce: &nonce,
        }
        .to_aad();

        let carrier = rail_shim::SealedDecryptCarrier {
            profile: rail_shim::SealProfile::PqHybrid,
            sealed_cek,
            ciphertext_segment,
            init_segment,
        };

        match rail_shim::decrypt_from_carrier_bound(session, &carrier, &aad, &verifier) {
            Ok((_plaintext, meta)) => scoped_session_response(&request, &meta),
            Err(_) => Response::error("decrypt_failed", "decrypt session could not be opened"),
        }
    }
}

/// Decrypt a protected-content segment using session material (the decrypt-step core).
///
/// Branch-by-Abstraction seam (see `docs/convergence/DDRM_DECRYPT_RAIL.md`): this is the
/// decrypt-step backend for the Hybrid rail, where the decrypt boundary *receives* its
/// material rather than reaching out for it. It is intentionally not yet reachable from
/// `open_session`/`render` — the CEK + ciphertext transport rail is an open architecture
/// decision. It is exercised directly by tests to prove the engine is correct at the
/// provider boundary and that the CEK never escapes this function.
///
/// The vendored cenc engine owns the CEK lifetime: it decodes `cek_b64`, uses it, and
/// zeroizes it on every return path. The returned plaintext is consumed only by the
/// scoped output sink inside the isolation boundary; it is never placed in a
/// caller-visible `Response`.
#[allow(dead_code)]
fn decrypt_session_segment(
    cek_b64: &str,
    ciphertext_segment: &[u8],
    init_segment: Option<&[u8]>,
) -> Result<(Vec<u8>, Value), String> {
    let command = json!({ "cek_b64": cek_b64, "iv_size": 8 }).to_string();
    let (result_json, output) = cenc::process(&command, ciphertext_segment, init_segment);
    let meta: Value = serde_json::from_str(&result_json).map_err(|err| err.to_string())?;
    if meta.get("success").and_then(Value::as_bool) != Some(true) {
        let message = meta
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("decrypt failed");
        return Err(message.to_string());
    }
    let plaintext = output.ok_or_else(|| "decrypt produced no output".to_string())?;
    Ok((plaintext, meta))
}

/// Rail-landing composition (PREP — gated behind the `rail-prep` feature; not yet
/// wired into `open_session`/`render`).
///
/// This joins the chain's two tested islands into the single in-boundary operation
/// the Hybrid decrypt rail will invoke once Anders confirms the CEK-transport rail
/// (`docs/convergence/DDRM_DECRYPT_RAIL.md`): the upstream CEK-sealing envelope
/// unwrap (`envelope::{parse, ecdh_unwrap, extract_cek}`) immediately followed by
/// the decrypt-step core (`decrypt_session_segment`). It mirrors PC2
/// `ddrm-decrypt::session::unwrap_envelope` (recover CEK) → cenc segment decrypt,
/// so the CEK:
///   - materializes only after a correct ECDH unwrap against the session secret key;
///   - is held in `Zeroizing` storage for its whole (short) lifetime;
///   - is consumed by the cenc engine inside this boundary and zeroized there;
///   - never appears in the scoped, caller-facing response (see `scoped_session_response`).
///
/// Keeping it behind a feature flag means the default build and the 25-test default
/// suite are unchanged (Parallel Change): the live wiring becomes a one-step swap
/// into dispatch once the rail and session-key provisioning land.
#[cfg(feature = "rail-prep")]
#[allow(dead_code)]
fn decrypt_sealed_segment(
    session_secret_key: &p256::SecretKey,
    sealed_envelope: &[u8],
    ciphertext_segment: &[u8],
    init_segment: Option<&[u8]>,
) -> Result<(Vec<u8>, Value), String> {
    use base64::Engine as _;
    use zeroize::Zeroizing;

    let parsed = envelope::parse(sealed_envelope).map_err(|err| format!("{err:?}"))?;
    let plaintext =
        envelope::ecdh_unwrap(session_secret_key, &parsed).map_err(|err| format!("{err:?}"))?;
    let cek = envelope::extract_cek(&plaintext).map_err(|err| format!("{err:?}"))?;

    // Bridge the recovered CEK into the cenc engine's command surface. The base64
    // form is held in `Zeroizing` so it is scrubbed from linear memory on drop,
    // keeping the CEK contained across this internal hand-off.
    let cek_b64 = Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(cek.as_slice()));
    decrypt_session_segment(&cek_b64, ciphertext_segment, init_segment)
}

/// Build the scoped, containment-safe decrypt-session response for the caller.
///
/// Carries session and output metadata only. The raw CEK and the decrypted plaintext
/// never cross this boundary to the caller (app/viewer capsule).
#[allow(dead_code)]
fn scoped_session_response(request: &DecryptSessionRequestV1, decrypt_meta: &Value) -> Response {
    Response::ok(json!({
        "schema": DECRYPT_SESSION_SCHEMA,
        "session_id": request.session_id,
        "object_cid": request.object_cid,
        "viewer_interface": request.viewer_interface,
        "output_kind": request.output_kind,
        "is_protected": decrypt_meta.get("is_protected"),
        "sample_count": decrypt_meta.get("sample_count"),
        "expires_at": request.expires_at,
    }))
}

fn validate_decrypt_session_request(request: &DecryptSessionRequestV1) -> Result<(), String> {
    if request.schema != DECRYPT_SESSION_REQUEST_SCHEMA {
        return Err("decrypt session request schema is unsupported".to_string());
    }
    require_non_empty(&request.request_id, "request_id")?;
    require_non_empty(&request.principal_id, "principal_id")?;
    require_non_empty(&request.session_id, "session_id")?;
    require_identifier(&request.object_cid, "object_cid")?;
    validate_action(&request.action)?;
    require_non_empty(&request.viewer_interface, "viewer_interface")?;
    validate_release_receipt(&request.release_receipt)?;
    validate_output_kind(&request.output_kind)?;
    require_non_empty(&request.reason, "reason")?;
    if request.expires_at == 0 {
        return Err("expires_at is required".to_string());
    }
    Ok(())
}

fn validate_release_receipt(receipt: &ReleaseReceiptV1) -> Result<(), String> {
    if receipt.schema != RELEASE_RECEIPT_SCHEMA {
        return Err("release receipt schema is unsupported".to_string());
    }
    require_non_empty(&receipt.request_id, "release_receipt.request_id")?;
    if receipt.status != "released" {
        return Err("release receipt status must be released".to_string());
    }
    Ok(())
}

fn validate_action(action: &str) -> Result<(), String> {
    if PROTECTED_CONTENT_ACTIONS.contains(&action) {
        Ok(())
    } else {
        Err(format!("unsupported protected-content action: {action}"))
    }
}

fn validate_output_kind(output_kind: &str) -> Result<(), String> {
    if PROTECTED_CONTENT_OUTPUTS.contains(&output_kind) {
        Ok(())
    } else {
        Err(format!(
            "unsupported protected-content output: {output_kind}"
        ))
    }
}

fn require_non_empty(value: &str, field: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("{field} is required"))
    } else {
        Ok(())
    }
}

fn require_identifier(value: &str, field: &str) -> Result<(), String> {
    require_non_empty(value, field)?;
    if value.len() > 256
        || value == "."
        || value == ".."
        || value
            .chars()
            .any(|ch| ch.is_ascii_whitespace() || ch.is_ascii_control() || ch == '/' || ch == '\\')
    {
        Err(format!("{field} must be an opaque identifier"))
    } else {
        Ok(())
    }
}

fn main() {
    eprintln!(
        "decrypt-provider: starting v{} (protected content decrypt/render)",
        PROVIDER_VERSION
    );

    let mut provider = DecryptProvider::default();
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                eprintln!("decrypt-provider read error: {}", err);
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }

        let request = match serde_json::from_str::<Request>(&line) {
            Ok(request) => request,
            Err(err) => {
                let response = Response::error("invalid_request", err.to_string());
                writeln!(stdout, "{}", serde_json::to_string(&response).unwrap()).unwrap();
                stdout.flush().unwrap();
                continue;
            }
        };
        let is_shutdown = matches!(request, Request::Shutdown);
        let response = provider.handle(request);
        writeln!(stdout, "{}", serde_json::to_string(&response).unwrap()).unwrap();
        stdout.flush().unwrap();
        if is_shutdown {
            break;
        }
    }

    eprintln!("decrypt-provider exiting");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decrypt_request() -> DecryptSessionRequestV1 {
        DecryptSessionRequestV1 {
            schema: DECRYPT_SESSION_REQUEST_SCHEMA.to_string(),
            request_id: "decrypt:test".to_string(),
            principal_id: "person:local:test".to_string(),
            session_id: "session:test".to_string(),
            object_cid: "bafybeigprotectedcontent".to_string(),
            action: "view".to_string(),
            viewer_interface: "elastos.viewer/document@1".to_string(),
            release_receipt: ReleaseReceiptV1 {
                schema: RELEASE_RECEIPT_SCHEMA.to_string(),
                request_id: "key-release:test".to_string(),
                object_cid: "bafybeigprotectedcontent".to_string(),
                principal_id: "person:local:test".to_string(),
                session_id: "session:test".to_string(),
                action: "view".to_string(),
                provider: "key-provider".to_string(),
                status: "released".to_string(),
                issued_at: 1_800_000_000,
                expires_at: 1_900_000_000,
            },
            output_kind: "rendered".to_string(),
            reason: "open protected document".to_string(),
            expires_at: 1_900_000_000,
        }
    }

    fn error_code(response: Response) -> String {
        match response {
            Response::Error { code, .. } => code,
            other => panic!("expected error, got {other:?}"),
        }
    }

    fn ok_data(response: Response) -> Value {
        match response {
            Response::Ok { data: Some(data) } => data,
            other => panic!("expected ok data, got {other:?}"),
        }
    }

    #[test]
    fn status_advertises_blocked_raw_authority() {
        let provider = DecryptProvider::default();
        let data = ok_data(provider.status());

        assert_eq!(data["provider"], "decrypt");
        assert_eq!(data["configured"], false);
        assert!(data["blocked_authority"]
            .as_array()
            .unwrap()
            .contains(&json!("raw_cek")));
        assert!(data["blocked_authority"]
            .as_array()
            .unwrap()
            .contains(&json!("raw_plaintext")));
    }

    #[test]
    fn open_session_fails_closed_until_backend_exists() {
        let provider = DecryptProvider::default();
        assert_eq!(
            error_code(provider.open_session(decrypt_request())),
            "not_configured"
        );
    }

    #[test]
    fn render_fails_closed_until_backend_exists() {
        let provider = DecryptProvider::default();
        assert_eq!(
            error_code(provider.render(decrypt_request())),
            "not_configured"
        );
    }

    #[test]
    fn open_session_rejects_unsupported_output_kind() {
        let provider = DecryptProvider::default();
        let mut request = decrypt_request();
        request.output_kind = "raw_plaintext".to_string();

        assert_eq!(
            error_code(provider.open_session(request)),
            "invalid_request"
        );
    }

    #[test]
    fn open_session_rejects_path_like_object_ids() {
        let provider = DecryptProvider::default();
        let mut request = decrypt_request();
        request.object_cid = "../secret".to_string();

        assert_eq!(
            error_code(provider.open_session(request)),
            "invalid_request"
        );
    }

    #[test]
    fn open_session_rejects_dot_segment_object_ids() {
        let provider = DecryptProvider::default();
        let mut request = decrypt_request();
        request.object_cid = "..".to_string();

        assert_eq!(
            error_code(provider.open_session(request)),
            "invalid_request"
        );
    }

    // --- decrypt-step core seam (Branch-by-Abstraction; see DDRM_DECRYPT_RAIL.md) ---

    use aes::cipher::{KeyIvInit, StreamCipher};
    use base64::Engine;

    type Aes128Ctr = ctr::Ctr128BE<aes::Aes128>;

    fn make_box(box_type: &[u8; 4], content: &[u8]) -> Vec<u8> {
        let size = (8 + content.len()) as u32;
        let mut b = size.to_be_bytes().to_vec();
        b.extend_from_slice(box_type);
        b.extend_from_slice(content);
        b
    }

    /// Minimal single-sample encrypted fMP4 segment: moof{traf{trun,senc}} + mdat{ciphertext}.
    fn build_encrypted_segment(plaintext: &[u8], cek: &[u8; 16], iv8: &[u8; 8]) -> Vec<u8> {
        let mut iv16 = [0u8; 16];
        iv16[..8].copy_from_slice(iv8);
        let mut ciphertext = plaintext.to_vec();
        let mut cipher = Aes128Ctr::new(cek.into(), (&iv16).into());
        cipher.apply_keystream(&mut ciphertext);

        let mut trun_content = vec![0u8, 0x00, 0x02, 0x00, 0, 0, 0, 1];
        trun_content.extend_from_slice(&(plaintext.len() as u32).to_be_bytes());
        let trun = make_box(b"trun", &trun_content);

        let mut senc_content = vec![0u8, 0, 0, 0, 0, 0, 0, 1];
        senc_content.extend_from_slice(iv8);
        let senc = make_box(b"senc", &senc_content);

        let mut traf_content = trun;
        traf_content.extend_from_slice(&senc);
        let traf = make_box(b"traf", &traf_content);
        let moof = make_box(b"moof", &traf);
        let mdat = make_box(b"mdat", &ciphertext);

        let mut segment = moof;
        segment.extend_from_slice(&mdat);
        segment
    }

    #[test]
    fn decrypt_session_segment_recovers_plaintext() {
        let plaintext = b"the quick brown fox jumps over!!"; // 32 bytes
        let cek = [0x11u8; 16];
        let iv8 = [0x22u8; 8];
        let segment = build_encrypted_segment(plaintext, &cek, &iv8);
        let cek_b64 = base64::engine::general_purpose::STANDARD.encode(cek);

        let (output, meta) = decrypt_session_segment(&cek_b64, &segment, None).unwrap();

        let moof_len = segment.len() - (8 + plaintext.len());
        let mdat_off = moof_len + 8;
        assert_eq!(&output[mdat_off..mdat_off + plaintext.len()], plaintext);
        assert_eq!(meta["is_protected"], json!(true));
        assert_eq!(meta["sample_count"], json!(1));
    }

    /// Cross-invariant round-trip: replay the golden PRODUCED BY encrypt-provider's
    /// real in-boundary engine (mint CEK+KID -> CENC encrypt -> mux) and prove THIS
    /// provider decrypts it back to the producer's original bytes, with the CEK
    /// staying off the scoped boundary. Pins #1 (produce) ↔ #2 (consume) on one
    /// artifact. Regenerate the fixture with:
    ///   (cd ../encrypt-provider && cargo test --features gen-vectors emit_roundtrip_vector)
    #[cfg(all(feature = "vectors", not(feature = "gen-vectors")))]
    #[test]
    fn encrypt_to_decrypt_round_trip_golden() {
        let b64 = base64::engine::general_purpose::STANDARD;
        let v: crate::vector_format::RoundTripVector = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/vectors/roundtrip_encrypt_to_decrypt.json"
        )))
        .unwrap();

        // The producer surfaced a 16-byte KID; the CEK never appears in the KID.
        assert_eq!(v.kid_hex.len(), 32, "producer KID is 16 bytes (32 hex)");

        let cek_b64 = v.cek_b64.clone();
        let segment = b64.decode(&v.encrypted_segment_b64).unwrap();
        let expected = b64.decode(&v.expected_plaintext_b64).unwrap();

        let (output, meta) = decrypt_session_segment(&cek_b64, &segment, None).unwrap();
        let mdat_off = segment.len() - expected.len();
        assert_eq!(
            &output[mdat_off..],
            expected.as_slice(),
            "decrypt-provider must recover the exact bytes encrypt-provider sealed"
        );
        assert_eq!(meta["is_protected"], json!(true));
        assert_eq!(meta["sample_count"], json!(1));

        // Containment at the consumer edge: the scoped response leaks neither the
        // (rail stand-in) CEK nor the recovered plaintext.
        let response = scoped_session_response(&decrypt_request(), &meta);
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(!serialized.contains(&cek_b64), "CEK must not cross the boundary");
        assert!(
            !serialized.contains(std::str::from_utf8(&expected).unwrap()),
            "plaintext must not cross the boundary"
        );
    }

    /// Replay the producer's MULTI-SAMPLE round-trip golden (real playback shape):
    /// encrypt-provider's real engine sealed 4 samples with per-sample IVs; this
    /// provider must recover the exact concatenated plaintext, report N samples,
    /// and leak neither CEK nor plaintext across the scoped boundary.
    #[cfg(all(feature = "vectors", not(feature = "gen-vectors")))]
    #[test]
    fn encrypt_to_decrypt_multisample_round_trip_golden() {
        let b64 = base64::engine::general_purpose::STANDARD;
        let v: crate::vector_format::RoundTripVector = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/vectors/roundtrip_multisample_encrypt_to_decrypt.json"
        )))
        .unwrap();

        let cek_b64 = v.cek_b64.clone();
        let segment = b64.decode(&v.encrypted_segment_b64).unwrap();
        let expected = b64.decode(&v.expected_plaintext_b64).unwrap();

        let (output, meta) = decrypt_session_segment(&cek_b64, &segment, None).unwrap();
        let mdat_off = segment.len() - expected.len();
        assert_eq!(
            &output[mdat_off..],
            expected.as_slice(),
            "decrypt-provider must recover every sample encrypt-provider sealed"
        );
        assert_eq!(meta["is_protected"], json!(true));
        assert!(
            meta["sample_count"].as_u64().unwrap() >= 2,
            "the golden is a multi-sample segment"
        );

        let response = scoped_session_response(&decrypt_request(), &meta);
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(!serialized.contains(&cek_b64), "CEK must not cross the boundary");
        assert!(
            !serialized.contains(std::str::from_utf8(&expected).unwrap()),
            "plaintext must not cross the boundary"
        );
    }

    /// Replay the producer's SUBSAMPLE round-trip golden (clear leader + encrypted
    /// body): the real engine left a 16-byte codec header in the clear and
    /// encrypted the remainder; this provider must reconstruct the full sample
    /// (clear bytes untouched, body decrypted) back to the producer's plaintext.
    #[cfg(all(feature = "vectors", not(feature = "gen-vectors")))]
    #[test]
    fn encrypt_to_decrypt_subsample_round_trip_golden() {
        let b64 = base64::engine::general_purpose::STANDARD;
        let v: crate::vector_format::RoundTripVector = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/vectors/roundtrip_subsample_encrypt_to_decrypt.json"
        )))
        .unwrap();

        let cek_b64 = v.cek_b64.clone();
        let segment = b64.decode(&v.encrypted_segment_b64).unwrap();
        let expected = b64.decode(&v.expected_plaintext_b64).unwrap();

        let (output, meta) = decrypt_session_segment(&cek_b64, &segment, None).unwrap();
        let mdat_off = segment.len() - expected.len();
        // The clear leader survives untouched and the body decrypts: the whole
        // sample equals the producer's original plaintext.
        assert_eq!(
            &output[mdat_off..],
            expected.as_slice(),
            "subsample reconstruction must equal the producer's plaintext"
        );
        assert_eq!(meta["is_protected"], json!(true));
        assert_eq!(meta["sample_count"], json!(1));

        let response = scoped_session_response(&decrypt_request(), &meta);
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(!serialized.contains(&cek_b64), "CEK must not cross the boundary");
        assert!(
            !serialized.contains(std::str::from_utf8(&expected).unwrap()),
            "plaintext must not cross the boundary"
        );
    }

    #[test]
    fn decrypt_session_segment_fails_closed_on_bad_cek() {
        let short_cek = base64::engine::general_purpose::STANDARD.encode([0u8; 8]);
        assert!(decrypt_session_segment(&short_cek, &[], None).is_err());
    }

    #[test]
    fn scoped_session_response_leaks_neither_cek_nor_plaintext() {
        let plaintext = b"the quick brown fox jumps over!!";
        let cek = [0x11u8; 16];
        let iv8 = [0x22u8; 8];
        let segment = build_encrypted_segment(plaintext, &cek, &iv8);
        let cek_b64 = base64::engine::general_purpose::STANDARD.encode(cek);

        let (_plaintext_bytes, meta) = decrypt_session_segment(&cek_b64, &segment, None).unwrap();
        let response = scoped_session_response(&decrypt_request(), &meta);
        let serialized = serde_json::to_string(&response).unwrap();

        assert!(
            !serialized.contains(&cek_b64),
            "CEK must never cross the provider boundary to the caller"
        );
        let plaintext_str = std::str::from_utf8(plaintext).unwrap();
        assert!(
            !serialized.contains(plaintext_str),
            "decrypted plaintext must never cross the provider boundary to the caller"
        );
    }

    // --- decrypt -> player consumer contract (Day 13) -----------------------
    //
    // The chain's downstream boundary. Both viewer capsules consume scoped
    // output ONLY; neither ever receives the CEK. Pins PC2's contract where the
    // media player gets decrypted fMP4 segments and the non-media player gets
    // render_only plaintext — in both cases addressed by an opaque session, with
    // key material confined to this provider (Irzhy invariant #2 at the edge).

    /// A media-player session (video/audio): streamed segments.
    fn media_decrypt_request() -> DecryptSessionRequestV1 {
        let mut request = decrypt_request();
        request.action = "stream".to_string();
        request.viewer_interface = "elastos.viewer/media@1".to_string();
        request.output_kind = "stream".to_string();
        request.release_receipt.action = "stream".to_string();
        request.reason = "open protected media stream".to_string();
        request
    }

    /// Field names that, if they ever appeared in a scoped response, would mean
    /// key material or raw content escaped the provider boundary.
    const FORBIDDEN_SCOPED_KEYS: &[&str] = &[
        "cek",
        "cek_b64",
        "iv",
        "iv_b64",
        "key",
        "keys",
        "plaintext",
        "decrypted",
        "secret",
        "private_key",
        "rendered_bytes",
        "output",
    ];

    /// Keys the scoped response is allowed to carry — metadata only.
    const ALLOWED_SCOPED_KEYS: &[&str] = &[
        "schema",
        "session_id",
        "object_cid",
        "viewer_interface",
        "output_kind",
        "is_protected",
        "sample_count",
        "expires_at",
    ];

    fn assert_scoped_response_is_metadata_only(request: &DecryptSessionRequestV1) {
        // A representative decrypt meta as produced by the cenc engine.
        let meta = json!({ "is_protected": true, "sample_count": 1 });
        let data = ok_data(scoped_session_response(request, &meta));
        let obj = data.as_object().expect("scoped response must be an object");

        for key in obj.keys() {
            assert!(
                ALLOWED_SCOPED_KEYS.contains(&key.as_str()),
                "scoped response carried an unexpected key `{key}` for {}",
                request.viewer_interface
            );
            assert!(
                !FORBIDDEN_SCOPED_KEYS.contains(&key.as_str()),
                "scoped response leaked forbidden key `{key}` for {}",
                request.viewer_interface
            );
        }

        // The player references the session by opaque id, never by key material.
        assert_eq!(data["session_id"], json!(request.session_id));
    }

    #[test]
    fn media_player_scoped_response_is_metadata_only() {
        assert_scoped_response_is_metadata_only(&media_decrypt_request());
    }

    #[test]
    fn non_media_player_scoped_response_is_metadata_only() {
        assert_scoped_response_is_metadata_only(&decrypt_request());
    }

    /// Media-player variant of the containment check: a real decrypted segment
    /// must not let the CEK or plaintext reach the scoped (player-facing) output.
    #[test]
    fn media_segment_decrypt_keeps_cek_and_plaintext_off_the_player_boundary() {
        let plaintext = b"the quick brown fox jumps over!!";
        let cek = [0x11u8; 16];
        let iv8 = [0x22u8; 8];
        let segment = build_encrypted_segment(plaintext, &cek, &iv8);
        let cek_b64 = base64::engine::general_purpose::STANDARD.encode(cek);

        let (_segment_bytes, meta) = decrypt_session_segment(&cek_b64, &segment, None).unwrap();
        let serialized =
            serde_json::to_string(&scoped_session_response(&media_decrypt_request(), &meta)).unwrap();

        assert!(!serialized.contains(&cek_b64), "CEK must not reach the media player");
        assert!(
            !serialized.contains(std::str::from_utf8(plaintext).unwrap()),
            "decrypted media must not reach the player as plaintext in the scoped response"
        );
    }

    // --- rail-landing composition (PREP, feature = "rail-prep") ----------------
    //
    // Proves the end-to-end in-boundary flow the Hybrid decrypt rail will invoke
    // once Anders confirms the CEK-transport rail: a session-sealed CEK envelope +
    // an encrypted media segment go in; scoped metadata comes out; the CEK and the
    // decrypted bytes never cross the provider boundary. Gated behind the feature so
    // the default suite stays at 25; run with:  cargo test --features rail-prep

    /// Seal a CEK to `session_pk` exactly as the upstream sealer (Lit/key-provider)
    /// would — independently constructing the wire format so the round-trip pins the
    /// rail contract end to end. Mirrors `envelope.rs`'s sealer.
    #[cfg(feature = "rail-prep")]
    fn seal_cek_envelope(session_pk: &p256::PublicKey, cek: &[u8], version: u8) -> Vec<u8> {
        use aes::Aes256;
        use cbc::Encryptor as CbcEncryptor;
        use cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
        use elliptic_curve::sec1::ToEncodedPoint;
        use p256::ecdh::EphemeralSecret;
        use rand_core::OsRng;
        type Aes256CbcEnc = CbcEncryptor<Aes256>;

        let eph = EphemeralSecret::random(&mut OsRng);
        let eph_point = eph.public_key().to_encoded_point(true);
        let eph_bytes = eph_point.as_bytes();
        let shared = eph.diffie_hellman(session_pk);
        let key_bytes = shared.raw_secret_bytes();

        let mut inner = Vec::new();
        inner.extend_from_slice(&0u32.to_be_bytes()); // metaSize
        inner.extend_from_slice(&1u32.to_be_bytes()); // keyCount
        inner.extend_from_slice(cek);

        let iv: [u8; 16] = if version == 0x03 {
            let mut iv = [0u8; 16];
            getrandom::getrandom(&mut iv).unwrap();
            iv
        } else {
            let mut iv = [0u8; 16];
            iv.copy_from_slice(&eph_bytes[..16]);
            iv
        };

        let cipher = Aes256CbcEnc::new(key_bytes.as_slice().into(), (&iv).into());
        let mut buf = vec![0u8; inner.len() + 16];
        buf[..inner.len()].copy_from_slice(&inner);
        let ct_len = cipher
            .encrypt_padded_mut::<Pkcs7>(&mut buf, inner.len())
            .unwrap()
            .len();
        let ciphertext = &buf[..ct_len];

        let mut env = Vec::new();
        env.extend_from_slice(&[0, 0, 0, version]);
        env.extend_from_slice(&(eph_bytes.len() as u16).to_be_bytes());
        env.extend_from_slice(eph_bytes);
        if version == 0x03 {
            env.extend_from_slice(&iv);
        }
        env.extend_from_slice(&0u16.to_be_bytes()); // empty signature
        env.extend_from_slice(&[0u8; 33]); // signer pubkey (skipped)
        env.extend_from_slice(&(ciphertext.len() as u32).to_be_bytes());
        env.extend_from_slice(ciphertext);
        env
    }

    #[cfg(feature = "rail-prep")]
    #[test]
    fn sealed_segment_decrypts_end_to_end_and_keeps_cek_off_the_boundary() {
        use p256::SecretKey;
        use rand_core::OsRng;

        let plaintext = b"the quick brown fox jumps over!!"; // 32 bytes
        let cek = [0x11u8; 16];
        let iv8 = [0x22u8; 8];

        let session_sk = SecretKey::random(&mut OsRng);
        let sealed = seal_cek_envelope(&session_sk.public_key(), &cek, 0x03);
        let segment = build_encrypted_segment(plaintext, &cek, &iv8);

        // The whole rail step: sealed CEK envelope + encrypted segment -> plaintext
        // recovered inside the boundary, CEK recovered only via ECDH unwrap.
        let (output, meta) = decrypt_sealed_segment(&session_sk, &sealed, &segment, None).unwrap();

        let moof_len = segment.len() - (8 + plaintext.len());
        let mdat_off = moof_len + 8;
        assert_eq!(&output[mdat_off..mdat_off + plaintext.len()], plaintext);
        assert_eq!(meta["is_protected"], json!(true));
        assert_eq!(meta["sample_count"], json!(1));

        // Containment: neither the CEK nor the plaintext reaches the scoped response,
        // and the sealed envelope never carried the raw CEK in cleartext.
        let cek_b64 = base64::engine::general_purpose::STANDARD.encode(cek);
        let serialized =
            serde_json::to_string(&scoped_session_response(&decrypt_request(), &meta)).unwrap();
        assert!(
            !serialized.contains(&cek_b64),
            "CEK must never cross the provider boundary to the caller"
        );
        assert!(
            !serialized.contains(std::str::from_utf8(plaintext).unwrap()),
            "decrypted plaintext must never cross the provider boundary to the caller"
        );
        assert!(
            !sealed.windows(cek.len()).any(|w| w == cek),
            "sealed envelope must not contain the raw CEK"
        );
    }

    #[cfg(feature = "rail-prep")]
    #[test]
    fn sealed_segment_fails_closed_on_wrong_session_key() {
        use p256::SecretKey;
        use rand_core::OsRng;

        let cek = [0x11u8; 16];
        let session_sk = SecretKey::random(&mut OsRng);
        let wrong_sk = SecretKey::random(&mut OsRng);
        let sealed = seal_cek_envelope(&session_sk.public_key(), &cek, 0x03);
        let segment = build_encrypted_segment(b"the quick brown fox jumps over!!", &cek, &[0x22u8; 8]);

        // A wrong session key cannot unwrap the envelope -> the whole step fails
        // closed before any segment decryption is attempted.
        assert!(decrypt_sealed_segment(&wrong_sk, &sealed, &segment, None).is_err());
    }

    // --- live decrypt rail through the provider dispatch (feature `rail-live`) ----
    //
    // The recommended rail (Option A) wired into OpenSessionLive: a real ML-DSA-65-
    // signed PQ-hybrid carrier is opened through the ACTUAL provider entrypoint, the
    // CEK is recovered + the segment decrypted in-boundary, and the scoped response
    // proves containment (neither CEK nor plaintext crosses to the caller).

    #[cfg(feature = "rail-live")]
    fn pq_rail_material(seed: [u8; 32], cek: &[u8; 16], plaintext: &[u8]) -> (RailMaterial, Vec<u8>, crate::rail_shim::SessionSecret, Vec<u8>) {
        use crate::pq_envelope::seal_support::{gen_session, mldsa_seal_keypair, seal};
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let (secret, public) = gen_session();
        let (signer, authority_vk) = mldsa_seal_keypair(seed);
        let segment = build_encrypted_segment(plaintext, cek, &[0x77u8; 8]);
        let sealed = seal(&public, cek, &signer).to_bytes();
        let material = RailMaterial {
            profile: "pq_hybrid".to_string(),
            sealed_cek_b64: b64.encode(&sealed),
            ciphertext_b64: b64.encode(&segment),
            init_segment_b64: None,
        };
        (material, sealed, crate::rail_shim::SessionSecret::PqHybrid(secret), authority_vk)
    }

    #[cfg(feature = "rail-live")]
    #[test]
    fn open_session_live_decrypts_pq_carrier_through_dispatch_without_leaking() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let cek = [0x5Au8; 16];
        let plaintext = b"rail-live: protected payload through the provider";
        let (material, _sealed, session, authority_vk) = pq_rail_material([0x33u8; 32], &cek, plaintext);

        // Provision the boundary (VM-minted session + trusted authority vk), then
        // drive the REAL dispatch through `handle`.
        let mut provider = DecryptProvider {
            session: Some(session),
            authority_vk: Some(authority_vk),
            ..Default::default()
        };
        let resp = provider.handle(Request::OpenSessionLive {
            request: Box::new(decrypt_request()),
            material,
        });

        let serialized = serde_json::to_string(&resp).unwrap();
        assert!(serialized.contains("\"status\":\"ok\""), "live rail must open the session: {serialized}");
        assert!(serialized.contains("session:test"), "scoped response carries the session id");
        // Containment through the full provider path: neither plaintext nor CEK leak.
        assert!(
            !serialized.contains(std::str::from_utf8(plaintext).unwrap()),
            "decrypted plaintext must never cross the provider boundary to the caller"
        );
        assert!(!serialized.contains(&b64.encode(cek)), "raw CEK must never cross the boundary");
    }

    #[cfg(feature = "rail-live")]
    #[test]
    fn open_session_live_fails_closed_on_tampered_carrier() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let cek = [0x5Au8; 16];
        let (mut material, mut sealed, session, authority_vk) =
            pq_rail_material([0x44u8; 32], &cek, b"payload");
        sealed[0] ^= 0xFF; // tamper the sealed carrier
        material.sealed_cek_b64 = b64.encode(&sealed);

        let provider = DecryptProvider {
            session: Some(session),
            authority_vk: Some(authority_vk),
            ..Default::default()
        };
        let resp = provider.open_session_live(decrypt_request(), &material);
        assert_eq!(error_code(resp), "decrypt_failed", "a tampered carrier must fail closed");
    }

    #[cfg(feature = "rail-live")]
    #[test]
    fn open_session_live_fails_closed_when_unprovisioned() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let material = RailMaterial {
            profile: "pq_hybrid".to_string(),
            sealed_cek_b64: b64.encode([0u8; 64]),
            ciphertext_b64: b64.encode([0u8; 32]),
            init_segment_b64: None,
        };
        // No session / vk provisioned -> fail closed (default boundary state).
        let provider = DecryptProvider::default();
        let resp = provider.open_session_live(decrypt_request(), &material);
        assert_eq!(error_code(resp), "not_configured");
    }

    // --- transcript-bound rail (feature `rail-bind`, Anders Day-45 decision) ------
    //
    // The headline security property: a CEK sealed for one decrypt transcript
    // cannot be opened under any other. We seal a real ML-DSA-65 PQ-hybrid carrier
    // bound to a transcript, drive it through `OpenSessionBound`, and prove (a) the
    // matching transcript decrypts with no CEK/plaintext leak, and (b) a replay
    // against a DIFFERENT session — and a tampered carrier — both fail closed.

    #[cfg(feature = "rail-bind")]
    fn bound_setup(
        seed: [u8; 32],
        seal_req: &DecryptSessionRequestV1,
        cek: &[u8; 16],
        plaintext: &[u8],
        nonce: &[u8],
        content_hash: &[u8],
    ) -> (BoundRailMaterial, crate::rail_shim::SessionSecret, Vec<u8>, Vec<u8>) {
        use crate::pq_envelope::seal_support::{gen_session, mldsa_seal_keypair, seal_bound};
        use crate::pq_envelope::session_public_bytes;
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;

        let (secret, public) = gen_session();
        let pub_bytes = session_public_bytes(&public);
        let (signer, authority_vk) = mldsa_seal_keypair(seed);

        // The key authority seals BOUND to the transcript of `seal_req`.
        let aad = DecryptTranscriptV1 {
            suite_id: DECRYPT_SUITE_ID,
            provider_id: DECRYPT_PROVIDER_ID,
            principal_id: &seal_req.principal_id,
            session_id: &seal_req.session_id,
            object_cid: &seal_req.object_cid,
            content_hash,
            action: &seal_req.action,
            viewer_interface: &seal_req.viewer_interface,
            output_kind: &seal_req.output_kind,
            expires_at: seal_req.expires_at,
            release_receipt_hash: release_receipt_hash(&seal_req.release_receipt),
            decrypt_session_pub: &pub_bytes,
            nonce,
        }
        .to_aad();

        let segment = build_encrypted_segment(plaintext, cek, &[0x77u8; 8]);
        let sealed = seal_bound(&public, cek, &aad, &signer).to_bytes();
        let material = BoundRailMaterial {
            sealed_cek_b64: b64.encode(&sealed),
            ciphertext_b64: b64.encode(&segment),
            init_segment_b64: None,
            nonce_b64: b64.encode(nonce),
            content_hash_b64: b64.encode(content_hash),
        };
        (material, crate::rail_shim::SessionSecret::PqHybrid(secret), authority_vk, pub_bytes)
    }

    #[cfg(feature = "rail-bind")]
    #[test]
    fn open_session_bound_decrypts_matching_transcript_without_leaking() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let req = decrypt_request();
        let cek = [0x5Au8; 16];
        let plaintext = b"transcript-bound payload through the provider";
        let (material, session, authority_vk, pub_bytes) =
            bound_setup([0x55u8; 32], &req, &cek, plaintext, b"nonce-0001", &[0xABu8; 32]);

        let mut provider = DecryptProvider {
            session: Some(session),
            authority_vk: Some(authority_vk),
            session_pub: Some(pub_bytes),
        };
        let resp = provider.handle(Request::OpenSessionBound {
            request: Box::new(req),
            material,
        });

        let serialized = serde_json::to_string(&resp).unwrap();
        assert!(serialized.contains("\"status\":\"ok\""), "matching transcript must open: {serialized}");
        assert!(!serialized.contains(std::str::from_utf8(plaintext).unwrap()), "plaintext must not leak");
        assert!(!serialized.contains(&b64.encode(cek)), "CEK must not leak");
    }

    #[cfg(feature = "rail-bind")]
    #[test]
    fn open_session_bound_fails_closed_on_replay_against_different_session() {
        let seal_req = decrypt_request();
        let cek = [0x5Au8; 16];
        let (material, session, authority_vk, pub_bytes) =
            bound_setup([0x66u8; 32], &seal_req, &cek, b"payload", b"nonce-0002", &[0xABu8; 32]);

        // Submit the SAME sealed material under a different session id — exactly the
        // replay the transcript binding must defeat. The boundary rebuilds the AAD
        // from the (different) request, so the GCM tag / signature reject it.
        let mut replay_req = decrypt_request();
        replay_req.session_id = "session:attacker".to_string();

        let mut provider = DecryptProvider {
            session: Some(session),
            authority_vk: Some(authority_vk),
            session_pub: Some(pub_bytes),
        };
        let resp = provider.handle(Request::OpenSessionBound {
            request: Box::new(replay_req),
            material,
        });
        assert_eq!(error_code(resp), "decrypt_failed", "a CEK bound to one session must not open another");
    }

    #[cfg(feature = "rail-bind")]
    #[test]
    fn open_session_bound_fails_closed_on_nonce_mismatch_and_tamper() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let req = decrypt_request();
        let cek = [0x5Au8; 16];

        // (a) replayed/altered nonce -> rebuilt transcript differs -> fail closed.
        let (mut material, session, authority_vk, pub_bytes) =
            bound_setup([0x77u8; 32], &req, &cek, b"payload", b"nonce-0003", &[0xABu8; 32]);
        material.nonce_b64 = b64.encode(b"nonce-XXXX");
        let mut provider = DecryptProvider {
            session: Some(session),
            authority_vk: Some(authority_vk),
            session_pub: Some(pub_bytes),
        };
        let resp = provider.handle(Request::OpenSessionBound {
            request: Box::new(decrypt_request()),
            material,
        });
        assert_eq!(error_code(resp), "decrypt_failed", "a swapped replay nonce must fail closed");

        // (b) tampered sealed carrier -> fail closed.
        let (mut material2, session2, authority_vk2, pub_bytes2) =
            bound_setup([0x88u8; 32], &req, &cek, b"payload", b"nonce-0004", &[0xABu8; 32]);
        let mut sealed = b64.decode(&material2.sealed_cek_b64).unwrap();
        sealed[0] ^= 0xFF;
        material2.sealed_cek_b64 = b64.encode(&sealed);
        let mut provider2 = DecryptProvider {
            session: Some(session2),
            authority_vk: Some(authority_vk2),
            session_pub: Some(pub_bytes2),
        };
        let resp2 = provider2.handle(Request::OpenSessionBound {
            request: Box::new(decrypt_request()),
            material: material2,
        });
        assert_eq!(error_code(resp2), "decrypt_failed", "a tampered carrier must fail closed");
    }

    // --- in-sandbox session-key mint + publish (feature `rail-mint`) -------------
    //
    // The faithful end-to-end flow Anders specified: the sandbox MINTS its own
    // per-session key at init and PUBLISHES the public key; the key authority seals
    // the CEK to that published key (transcript-bound); the boundary opens it using
    // the MINTED secret it holds — no secret is ever injected by the test.

    #[cfg(feature = "rail-mint")]
    #[test]
    fn minted_session_publishes_key_and_opens_bound_carrier() {
        use crate::pq_envelope::seal_support::{mldsa_seal_keypair, seal_bound};
        use crate::pq_envelope::session_public_from_bytes;
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;

        let (signer, authority_vk) = mldsa_seal_keypair([0x99u8; 32]);

        // Sandbox mints + publishes its session key at init.
        let mut provider = DecryptProvider::default();
        let init = provider.handle(Request::Init {
            config: json!({ "authority_vk_b64": b64.encode(&authority_vk) }),
        });
        let init_json = serde_json::to_value(&init).unwrap();
        assert_eq!(init_json["data"]["configured"], json!(true), "trusted vk pins configured");
        let pub_bytes = b64
            .decode(init_json["data"]["decrypt_session_public_key_b64"].as_str().unwrap())
            .unwrap();
        // The minted secret must never appear in the published init response.
        assert!(!serde_json::to_string(&init).unwrap().contains("secret"));

        // Key authority seals the CEK to the PUBLISHED key, bound to the transcript.
        let public = session_public_from_bytes(&pub_bytes).expect("published key parses");
        let req = decrypt_request();
        let cek = [0x5Au8; 16];
        let plaintext = b"minted-session faithful flow payload";
        let nonce = b"nonce-mint-01";
        let content_hash = [0xCDu8; 32];
        let aad = DecryptTranscriptV1 {
            suite_id: DECRYPT_SUITE_ID,
            provider_id: DECRYPT_PROVIDER_ID,
            principal_id: &req.principal_id,
            session_id: &req.session_id,
            object_cid: &req.object_cid,
            content_hash: &content_hash,
            action: &req.action,
            viewer_interface: &req.viewer_interface,
            output_kind: &req.output_kind,
            expires_at: req.expires_at,
            release_receipt_hash: release_receipt_hash(&req.release_receipt),
            decrypt_session_pub: &pub_bytes,
            nonce,
        }
        .to_aad();
        let segment = build_encrypted_segment(plaintext, &cek, &[0x77u8; 8]);
        let sealed = seal_bound(&public, &cek, &aad, &signer).to_bytes();
        let material = BoundRailMaterial {
            sealed_cek_b64: b64.encode(&sealed),
            ciphertext_b64: b64.encode(&segment),
            init_segment_b64: None,
            nonce_b64: b64.encode(nonce),
            content_hash_b64: b64.encode(content_hash),
        };

        // Open using the MINTED secret the boundary holds (never injected).
        let resp = provider.handle(Request::OpenSessionBound {
            request: Box::new(req),
            material,
        });
        let serialized = serde_json::to_string(&resp).unwrap();
        assert!(serialized.contains("\"status\":\"ok\""), "minted+published flow must open: {serialized}");
        assert!(!serialized.contains(std::str::from_utf8(plaintext).unwrap()), "plaintext must not leak");
        assert!(!serialized.contains(&b64.encode(cek)), "CEK must not leak");
    }

    #[cfg(feature = "rail-mint")]
    #[test]
    fn minted_session_is_fresh_each_init() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let key_of = |mut p: DecryptProvider| {
            let r = p.handle(Request::Init { config: json!({}) });
            serde_json::to_value(&r).unwrap()["data"]["decrypt_session_public_key_b64"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let k1 = key_of(DecryptProvider::default());
        let k2 = key_of(DecryptProvider::default());
        assert_ne!(k1, k2, "each sandbox mints a fresh per-session key");
        // x25519(32) ‖ ML-KEM-768 ek(1184) = 1216 published bytes.
        assert_eq!(b64.decode(&k1).unwrap().len(), 32 + 1184);
    }
}
