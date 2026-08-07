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
// Pixel-lock secure renderer (feature `pdf-render`): rasterises a decrypted PDF page to
// a watermarked JPEG inside this boundary so the plaintext never egresses. PC2
// ddrm-renderer parity. Wired into the `Render` op under the fail-closed contract.
#[cfg(feature = "pdf-render")]
mod render;

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
    // Audited, expiry-enforcing open (feature `rail-audit`): adds the injected
    // capability clock (`now_unix`) and returns a scoped audit envelope.
    #[cfg(feature = "rail-audit")]
    OpenSessionAudited {
        request: Box<DecryptSessionRequestV1>,
        material: BoundRailMaterial,
        now_unix: u64,
    },
    // Canonical open over the consolidated, suite-tagged `SealedDecryptMaterialV1`
    // (feature `rail-material`) — the exact shape that folds into the shared
    // contract. Routes through the audited/expiry-enforcing bound path.
    #[cfg(feature = "rail-material")]
    OpenSessionV1 {
        request: Box<DecryptSessionRequestV1>,
        material: SealedDecryptMaterialV1,
        now_unix: u64,
    },
    // Session-scoped media STREAM read (feature `rail-stream`, B2 of the viewer
    // seam). Given an opened media session's sealed material + a segment index, the
    // boundary unwraps the CEK in-VM, decrypts, and returns ONLY that segment's
    // already-decrypted bytes as the `stream` output kind — never the CEK/IV. The
    // runtime relays the (CEK-free) sealed material per request and never holds
    // plaintext; the elacity-player viewer feeds the returned segments into MSE.
    #[cfg(feature = "rail-stream")]
    StreamSegment {
        request: Box<DecryptSessionRequestV1>,
        material: SealedDecryptMaterialV1,
        index: usize,
        now_unix: u64,
        /// Pixel-lock render directive (feature `pdf-render`). When present, the decrypted
        /// object is NOT returned as raw bytes — it is rasterised to a single watermarked page
        /// image INSIDE this boundary and only that image egresses (the plaintext object never
        /// leaves the sandbox). Absent ⇒ the legacy raw-segment response, byte-identical.
        #[serde(default)]
        render: Option<RenderDirective>,
    },
    // Pixel-lock NEXT-PAGE read (feature `pdf-render`): render one more page of an asset
    // ALREADY decrypted + parsed in THIS session by a prior `StreamSegment { render }`. It
    // carries NO sealed material and NO ciphertext — only the warm session id + the page —
    // so paging never re-ships the object or re-contacts the quorum. Fails closed if the
    // warm session is absent or its id does not match (the helper must establish it first).
    #[cfg(feature = "pdf-render")]
    RenderPage {
        session_id: String,
        page: u32,
        #[serde(default)]
        max_width: Option<u32>,
        #[serde(default)]
        watermark: Option<String>,
    },
    Shutdown,
}

/// A pixel-lock render request rider on `StreamSegment` (capsule-local, so the shared
/// `DecryptSessionRequestV1` contract stays byte-identical). Carries the object's real
/// mime (to pick the renderer + confirm it is pixel-lock), the page to render, a width
/// cap, and the forensic watermark text (the buyer/owner identity stamped onto the page).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RenderDirective {
    mime: String,
    #[serde(default)]
    page: Option<u32>,
    #[serde(default)]
    max_width: Option<u32>,
    #[serde(default)]
    watermark: Option<String>,
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
    /// MULTI-SEGMENT (optional): segments after segment 0 (`ciphertext_b64`), in order. See
    /// `SealedDecryptMaterialV1::extra_segments_b64`. Absent ⇒ single-segment (default).
    #[serde(default)]
    extra_segments_b64: Option<Vec<String>>,
    /// Optional rights-decision binding (base64 of the rights receipt hash), welded into
    /// the transcript AAD when present. Absent ⇒ ungated seal (byte-identical AAD).
    #[serde(default)]
    rights_receipt_hash_b64: Option<String>,
}

/// Consolidated, backend-neutral sealed decrypt material (feature `rail-material`)
/// — the single envelope Anders named, and the exact drop-in shape to fold into the
/// shared `DecryptSessionRequestV1` when the contract opens. The `suite` tag makes
/// the backend a FIELD, not a fork: dKMS-native PQ-hybrid vs P-256/Lit compat are
/// the same wire type. Carries only sealed/public bytes — never a raw CEK; the VM
/// session secret stays in-VM. The remaining transcript fields come from the
/// authenticated request + the boundary's provisioned state.
#[cfg(feature = "rail-material")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SealedDecryptMaterialV1 {
    /// Algorithm-suite tag: `elastos-pq-hybrid-threshold-v0` (product target) or
    /// `p256-classical-compat` (PC2/Lit migration only).
    suite: String,
    sealed_cek_b64: String,
    /// 2-of-2 threshold (Day 97–98): the SECOND sealed CEK share, re-sealed by the
    /// second dKMS node to THIS session and bound to the SAME transcript. When
    /// present, `sealed_cek_b64` is share-1 (node A) and this is share-2 (node B);
    /// the boundary unwraps BOTH and reconstructs `cek = share1 ⊕ share2` in-VM. When
    /// absent, the legacy single-CEK path runs unchanged. Verified under the second
    /// trusted node verifying key provisioned at `init` (`authority_vk2_b64`).
    #[serde(default)]
    sealed_cek_share2_b64: Option<String>,
    /// 2-of-3 QUORUM cheater-detection (pre-audit #1): the THIRD node's re-sealed indexed share,
    /// when the rail served all three. The boundary unwraps all three and requires every pair to
    /// agree, so a single Byzantine node returning a wrong-valued share FAILS THE OPEN CLOSED
    /// rather than combining into a silently-wrong CEK. Absent ⇒ a 2-share (degraded) quorum, for
    /// which integrity rests on `cek_commitment_b64`.
    #[serde(default)]
    sealed_cek_share3_b64: Option<String>,
    /// Published CEK COMMITMENT (pre-audit #1), base64 of `ddrm_envelope::cek_commitment`. Computed
    /// by the producer (the only party that legitimately materialises the CEK) at publish and
    /// carried through the escrow. The boundary re-derives it from the reconstructed CEK and fails
    /// closed on mismatch — the integrity backstop that catches a wrong-valued share even on the
    /// 2-share (degraded) quorum and the 2-of-2 threshold rails. Absent ⇒ legacy material; integrity
    /// then relies on 3-share cheater detection (quorum only).
    #[serde(default)]
    cek_commitment_b64: Option<String>,
    ciphertext_b64: String,
    #[serde(default)]
    init_segment_b64: Option<String>,
    nonce_b64: String,
    content_hash_b64: String,
    /// MULTI-SEGMENT (optional): the segments AFTER `ciphertext_b64` (which is segment 0), in
    /// presentation order. Absent ⇒ single-segment open (the default; byte-identical behaviour).
    /// When present, all segments share the one sealed CEK and the whole ordered set is welded into
    /// the transcript AAD (`segment_digests`), so a reordered/altered set fails the unwrap closed.
    #[serde(default)]
    extra_segments_b64: Option<Vec<String>>,
    /// Optional rights-decision binding (base64 of the rights receipt hash), carried into
    /// the transcript AAD by [`DecryptProvider::prepare_bound_open`] when present.
    #[serde(default)]
    rights_receipt_hash_b64: Option<String>,
}

/// The algorithm suite a `SealedDecryptMaterialV1` declares.
#[cfg(feature = "rail-material")]
enum DecryptSuite {
    /// Product target: x25519+ML-KEM-768 KEM, ML-DSA-65 signature, transcript-bound.
    PqHybridThreshold,
    /// PC2/Lit compatibility (P-256 ECDH + AES-CBC) — migration only, not the
    /// transcript-bound product path.
    P256ClassicalCompat,
}

#[cfg(feature = "rail-material")]
const DECRYPT_SUITE_CLASSICAL_COMPAT: &str = "p256-classical-compat";

#[cfg(feature = "rail-material")]
fn parse_decrypt_suite(suite: &str) -> Option<DecryptSuite> {
    match suite {
        DECRYPT_SUITE_ID => Some(DecryptSuite::PqHybridThreshold),
        DECRYPT_SUITE_CLASSICAL_COMPAT => Some(DecryptSuite::P256ClassicalCompat),
        _ => None,
    }
}

/// The canonical decrypt transcript (feature `rail-bind`, Anders' Day-45 field set:
/// principal, session, object CID + content hash, action, viewer interface, output
/// kind, expiry, release-receipt hash, decrypt-session public key, algorithm suite,
/// provider identity, replay nonce) now lives ONCE in the shared `ddrm-envelope`
/// crate, so the key authority computes the SAME `to_aad()` it seals to and this
/// boundary rebuilds — no parallel encoder to drift. Re-used here under the
/// historical path. See `ddrm_envelope::transcript::DecryptTranscriptV1`.
#[cfg(feature = "rail-bind")]
use ddrm_envelope::transcript::DecryptTranscriptV1;

#[cfg(feature = "rail-bind")]
const DECRYPT_SUITE_ID: &str = "elastos-pq-hybrid-threshold-v0";
#[cfg(feature = "rail-bind")]
const DECRYPT_PROVIDER_ID: &str = "decrypt-provider";

/// Bind the release receipt into the transcript by hashing its identifying fields
/// (Anders: "release receipt hash"). Delegates to the shared
/// `ddrm_envelope::transcript::release_receipt_hash` so the key authority computes
/// the IDENTICAL hash it seals into the transcript — one encoder, no drift. The
/// output is byte-for-byte unchanged (the committed `rail-bind`/`rail-material`
/// goldens replay).
#[cfg(feature = "rail-bind")]
fn release_receipt_hash(receipt: &ReleaseReceiptV1) -> [u8; 32] {
    ddrm_envelope::transcript::release_receipt_hash(
        &receipt.schema,
        &receipt.request_id,
        &receipt.object_cid,
        &receipt.principal_id,
        &receipt.session_id,
        &receipt.action,
        &receipt.provider,
        &receipt.status,
        receipt.issued_at,
        receipt.expires_at,
    )
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
    // The SECOND trusted dKMS-node ML-DSA-65 verifying key (2-of-2 threshold,
    // `rail-material`). Provisioned at `init` (`authority_vk2_b64`); verifies the
    // second sealed share. Absent for the single-node rail (threshold off).
    #[cfg(feature = "rail-live")]
    authority_vk2: Option<Vec<u8>>,
    // The THIRD trusted dKMS-node ML-DSA-65 verifying key (2-of-3 QUORUM, Day
    // 113–116, `rail-material`). Provisioned at `init` (`authority_vk3_b64`). When
    // present the boundary is a 2-of-3 quorum boundary: the CEK was Shamir-split
    // across THREE nodes and ANY TWO indexed re-sealed shares reconstruct it
    // (`rail_shim::decrypt_from_carrier_quorum`); each share verifies under the
    // pinned identity its x-coordinate names. Absent → the 2-of-2/single rails.
    #[cfg(feature = "rail-live")]
    authority_vk3: Option<Vec<u8>>,
    // The published decrypt-session public key (transcript binding, `rail-bind`).
    // It is minted in-sandbox and is what the key authority seals the CEK to; it
    // is bound into the transcript so a carrier cannot be replayed against a
    // different session key.
    #[cfg(feature = "rail-bind")]
    session_pub: Option<Vec<u8>>,
}

// WARM pixel-lock render state (feature `pdf-render`). The decrypt-provider process is spawned
// per OPEN and kept alive for the view session by the helper, so a single warm slot holds the
// object decrypted + parsed ONCE plus a page-image cache: the quorum is hit once, the object is
// decrypted + parsed once, and each page is a fast in-VM rasterise (re-visits are an instant
// cache hit). Thread-local (not a struct field) because the boundary is single-threaded stdio
// and this avoids threading render state through every construction site. It lives and dies with
// the process, so the parsed plaintext is gone the moment the open ends.
#[cfg(feature = "pdf-render")]
thread_local! {
    static RENDER_CACHE: std::cell::RefCell<Option<render::RenderSession>> =
        const { std::cell::RefCell::new(None) };
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
            #[cfg(feature = "rail-audit")]
            Request::OpenSessionAudited {
                request,
                material,
                now_unix,
            } => self.open_session_audited(*request, &material, now_unix),
            #[cfg(feature = "rail-material")]
            Request::OpenSessionV1 {
                request,
                material,
                now_unix,
            } => self.open_session_v1(*request, material, now_unix),
            #[cfg(feature = "rail-stream")]
            Request::StreamSegment {
                request,
                material,
                index,
                now_unix,
                render,
            } => self.stream_segment(*request, material, index, now_unix, render),
            #[cfg(feature = "pdf-render")]
            Request::RenderPage {
                session_id,
                page,
                max_width,
                watermark,
            } => self.render_cached_page(&session_id, page, max_width, watermark.as_deref()),
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
                Err(_) => {
                    return Response::error(
                        "invalid_request",
                        "authority_vk_b64 is not valid base64",
                    )
                }
            }
        }

        // The second node verifying key for 2-of-2 threshold reconstruction. Optional:
        // present only when this boundary is provisioned against a threshold publish.
        if let Some(vk_b64) = config.get("authority_vk2_b64").and_then(Value::as_str) {
            match b64.decode(vk_b64) {
                Ok(vk) => self.authority_vk2 = Some(vk),
                Err(_) => {
                    return Response::error(
                        "invalid_request",
                        "authority_vk2_b64 is not valid base64",
                    )
                }
            }
        }

        // The THIRD node verifying key promotes this boundary to a 2-of-3 QUORUM
        // (Day 113–116). It only makes sense alongside the second — a third pinned
        // identity with no second is a misconfigured node-set; fail closed at init
        // rather than guess which rail was meant.
        if let Some(vk_b64) = config.get("authority_vk3_b64").and_then(Value::as_str) {
            if self.authority_vk2.is_none() {
                return Response::error(
                    "invalid_request",
                    "authority_vk3_b64 without authority_vk2_b64 — a 2-of-3 quorum needs all three pinned node identities",
                );
            }
            match b64.decode(vk_b64) {
                Ok(vk) => self.authority_vk3 = Some(vk),
                Err(_) => {
                    return Response::error(
                        "invalid_request",
                        "authority_vk3_b64 is not valid base64",
                    )
                }
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
    fn open_session_live(
        &self,
        request: DecryptSessionRequestV1,
        material: &RailMaterial,
    ) -> Response {
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
            other => {
                return Response::error(
                    "invalid_request",
                    format!("unsupported seal profile: {other}"),
                )
            }
        };
        let sealed_cek = match b64.decode(&material.sealed_cek_b64) {
            Ok(bytes) => bytes,
            Err(_) => {
                return Response::error("invalid_request", "sealed_cek_b64 is not valid base64")
            }
        };
        let ciphertext_segment = match b64.decode(&material.ciphertext_b64) {
            Ok(bytes) => bytes,
            Err(_) => {
                return Response::error("invalid_request", "ciphertext_b64 is not valid base64")
            }
        };
        let init_segment = match material.init_segment_b64.as_deref().map(|s| b64.decode(s)) {
            None => None,
            Some(Ok(bytes)) => Some(bytes),
            Some(Err(_)) => {
                return Response::error("invalid_request", "init_segment_b64 is not valid base64")
            }
        };

        let verifier = match crate::pq_envelope::mldsa::MlDsa65Verifier::from_encoded(authority_vk)
        {
            Some(v) => v,
            None => {
                return Response::error(
                    "not_configured",
                    "configured key-authority verifying key is malformed",
                )
            }
        };

        let carrier = rail_shim::SealedDecryptBundle {
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
    fn open_session_bound(
        &self,
        request: DecryptSessionRequestV1,
        material: &BoundRailMaterial,
    ) -> Response {
        let prepared = match self.prepare_bound_open(&request, material, None) {
            Ok(p) => p,
            Err(resp) => return resp,
        };
        match rail_shim::decrypt_from_carrier_bound(
            prepared.session,
            &prepared.carrier,
            &prepared.aad,
            &prepared.verifier,
        ) {
            Ok((_plaintext, meta)) => scoped_session_response(&request, &meta),
            Err(_) => Response::error("decrypt_failed", "decrypt session could not be opened"),
        }
    }

    /// Validate, check provisioning, decode the carrier, and rebuild the transcript
    /// AAD from the AUTHENTICATED request + the boundary's own provisioned state.
    /// Shared by the bound and audited rails so the binding is identical. Returns a
    /// fail-closed `Response` on any malformed/unprovisioned input.
    ///
    /// `node_set_id` (Day 103–104): the 2-of-2 node-set identity the threshold path
    /// derives from the boundary's OWN pinned vks — welded into the transcript AAD so
    /// the unwrap is cryptographically bound to the exact secret-holders this boundary
    /// trusts. `None` on the single-node paths (their AAD stays byte-identical).
    #[cfg(feature = "rail-bind")]
    fn prepare_bound_open(
        &self,
        request: &DecryptSessionRequestV1,
        material: &BoundRailMaterial,
        node_set_id: Option<&[u8; 32]>,
    ) -> Result<PreparedBoundOpen<'_>, Response> {
        use base64::Engine as _;

        if let Err(err) = validate_decrypt_session_request(request) {
            return Err(Response::error("invalid_request", err));
        }

        let session = self.session.as_ref().ok_or_else(|| {
            Response::error(
                "not_configured",
                "decrypt session key is not provisioned in this boundary",
            )
        })?;
        let session_pub = self.session_pub.as_ref().ok_or_else(|| {
            Response::error(
                "not_configured",
                "decrypt session public key is not published",
            )
        })?;
        let authority_vk = self.authority_vk.as_ref().ok_or_else(|| {
            Response::error(
                "not_configured",
                "trusted key-authority verifying key is not configured",
            )
        })?;

        let b64 = base64::engine::general_purpose::STANDARD;
        let decode = |s: &str, field: &str| -> Result<Vec<u8>, Response> {
            b64.decode(s).map_err(|_| {
                Response::error("invalid_request", format!("{field} is not valid base64"))
            })
        };
        let sealed_cek = decode(&material.sealed_cek_b64, "sealed_cek_b64")?;
        let ciphertext_segment = decode(&material.ciphertext_b64, "ciphertext_b64")?;
        let init_segment = match material.init_segment_b64.as_deref() {
            None => None,
            Some(s) => Some(decode(s, "init_segment_b64")?),
        };
        let nonce = decode(&material.nonce_b64, "nonce_b64")?;
        let content_hash = decode(&material.content_hash_b64, "content_hash_b64")?;
        // Rights-decision binding (live-chain gate): when the key-authority welded a
        // rights receipt hash into the seal, the boundary MUST rebuild the identical
        // AAD or the unwrap fails closed. Absent ⇒ ungated (byte-identical AAD).
        let rights_receipt_hash = match material.rights_receipt_hash_b64.as_deref() {
            None => None,
            Some(s) => Some(decode(s, "rights_receipt_hash_b64")?),
        };

        // MULTI-SEGMENT: decode the extra segments (after segment 0). When present, the WHOLE
        // ordered set (segment 0 + extras) is welded into the transcript AAD via `segment_digests`,
        // so the unwrap proves the open is for exactly this ordered, content-addressed asset.
        let extra_segments: Vec<Vec<u8>> = match material.extra_segments_b64.as_ref() {
            None => Vec::new(),
            Some(list) => {
                let mut out = Vec::with_capacity(list.len());
                for (i, s) in list.iter().enumerate() {
                    out.push(decode(s, &format!("extra_segments_b64[{i}]"))?);
                }
                out
            }
        };
        let segment_digests = if extra_segments.is_empty() {
            None
        } else {
            let mut ordered: Vec<&[u8]> = Vec::with_capacity(1 + extra_segments.len());
            ordered.push(ciphertext_segment.as_slice());
            ordered.extend(extra_segments.iter().map(|s| s.as_slice()));
            Some(ddrm_envelope::segment_digests(&ordered))
        };

        let verifier = crate::pq_envelope::mldsa::MlDsa65Verifier::from_encoded(authority_vk)
            .ok_or_else(|| {
                Response::error(
                    "not_configured",
                    "configured key-authority verifying key is malformed",
                )
            })?;

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
            node_set_id: node_set_id.map(|id| id.as_slice()),
        }
        .to_aad_with_bindings(segment_digests.as_deref(), rights_receipt_hash.as_deref());

        Ok(PreparedBoundOpen {
            session,
            verifier,
            aad,
            carrier: rail_shim::SealedDecryptBundle {
                profile: rail_shim::SealProfile::PqHybrid,
                sealed_cek,
                ciphertext_segment,
                init_segment,
            },
            extra_segments,
        })
    }

    /// Audited, expiry-enforcing open (feature `rail-audit`, Anders' "short expiry,
    /// audit"). Rejects a stale grant — `now_unix > request.expires_at` or the
    /// release receipt expired — BEFORE any unwrap, and emits a scoped, tamper-
    /// evident audit record (bound to the transcript hash, carrying NO CEK or
    /// plaintext) on every decision. The clock is an injected capability input
    /// (`now_unix`), never an ambient read.
    #[cfg(feature = "rail-audit")]
    fn open_session_audited(
        &self,
        request: DecryptSessionRequestV1,
        material: &BoundRailMaterial,
        now_unix: u64,
    ) -> Response {
        let prepared = match self.prepare_bound_open(&request, material, None) {
            Ok(p) => p,
            // Pre-decision input/provisioning failures stay fail-closed errors
            // (no grant existed to audit yet).
            Err(resp) => return resp,
        };
        let transcript_hash = {
            use sha2::{Digest, Sha256};
            let h: [u8; 32] = Sha256::digest(&prepared.aad).into();
            h
        };

        // Short-expiry enforcement — before any crypto.
        let expired =
            now_unix > request.expires_at || now_unix > request.release_receipt.expires_at;
        if expired {
            return audited_response(
                &request,
                &transcript_hash,
                now_unix,
                "denied",
                "expired",
                None,
            );
        }

        // Single segment (default) vs MULTI-SEGMENT asset: both unwrap the ONE sealed CEK in-VM and
        // keep plaintext contained; the multi-segment path loops the decrypt over the whole ordered
        // asset (already welded into the AAD), surfacing `segment_count` + the summed `sample_count`.
        let decrypt_result = if prepared.extra_segments.is_empty() {
            rail_shim::decrypt_from_carrier_bound(
                prepared.session,
                &prepared.carrier,
                &prepared.aad,
                &prepared.verifier,
            )
            .map(|(plaintext, meta)| (vec![plaintext], meta))
        } else {
            rail_shim::decrypt_from_carrier_bound_segments(
                prepared.session,
                &prepared.carrier,
                &prepared.extra_segments,
                &prepared.aad,
                &prepared.verifier,
            )
        };

        match decrypt_result {
            Ok((_plaintexts, meta)) => {
                let scoped = match scoped_session_response(&request, &meta) {
                    Response::Ok { data: Some(data) } => data,
                    other => return other,
                };
                audited_response(
                    &request,
                    &transcript_hash,
                    now_unix,
                    "opened",
                    "ok",
                    Some(scoped),
                )
            }
            Err(_) => audited_response(
                &request,
                &transcript_hash,
                now_unix,
                "denied",
                "decrypt_failed",
                None,
            ),
        }
    }

    /// Canonical open over the consolidated `SealedDecryptMaterialV1` (feature
    /// `rail-material`). The `suite` tag drives profile selection: the product
    /// PQ-hybrid suite routes through the audited/expiry-enforcing transcript-bound
    /// path; the P-256/Lit compat suite is recognised but **rejected** on this
    /// (product, transcript-bound) path — it is migration-only; an unknown suite
    /// fails closed. This is the single op the shared contract will expose.
    #[cfg(feature = "rail-material")]
    fn open_session_v1(
        &self,
        request: DecryptSessionRequestV1,
        material: SealedDecryptMaterialV1,
        now_unix: u64,
    ) -> Response {
        match parse_decrypt_suite(&material.suite) {
            Some(DecryptSuite::PqHybridThreshold) => {}
            Some(DecryptSuite::P256ClassicalCompat) => {
                return Response::error(
                    "invalid_request",
                    "the p256-classical-compat suite is migration-only; the transcript-bound open requires the PQ-hybrid suite",
                )
            }
            None => return Response::error("invalid_request", "unsupported decrypt suite"),
        }

        // 2-of-2 threshold (Day 97–98) / 2-of-3 quorum (Day 113–116): a SECOND sealed share present
        // means the CEK was split across dKMS nodes at publish. Route through the reconstruction
        // path that unwraps the shares and recombines them INSIDE the boundary — the whole CEK never
        // exists in `key-provider`. Absent → the legacy single-CEK audited path. Multi-segment is
        // wired on BOTH the single-node and the threshold/quorum rails: the CEK is reconstructed ONCE
        // and drives the whole ordered asset, which is welded into the transcript AAD.
        if let Some(share2_b64) = material.sealed_cek_share2_b64.clone() {
            return self.open_session_threshold(request, &material, &share2_b64, now_unix);
        }

        // Day 103–104: a THRESHOLD-provisioned boundary (a second trusted node vk is pinned)
        // must never open a SINGLE-share material — that would silently accept a release the
        // key authority degraded to one node. Defense-in-depth over the key-provider's own
        // refusal (it never emits a one-share material on the threshold rail).
        if self.authority_vk2.is_some() {
            return Response::error(
                "invalid_request",
                "this boundary is provisioned for a 2-of-2 threshold; a single-share material is refused",
            );
        }

        let bound = BoundRailMaterial {
            sealed_cek_b64: material.sealed_cek_b64,
            ciphertext_b64: material.ciphertext_b64,
            init_segment_b64: material.init_segment_b64,
            nonce_b64: material.nonce_b64,
            content_hash_b64: material.content_hash_b64,
            extra_segments_b64: material.extra_segments_b64,
            rights_receipt_hash_b64: material.rights_receipt_hash_b64,
        };
        self.open_session_audited(request, &bound, now_unix)
    }

    /// Session-scoped media STREAM read (feature `rail-stream`, B2 of the viewer
    /// seam). Given an opened media session's sealed material + a segment `index`,
    /// the boundary unwraps the CEK in-VM (`Zeroizing`), decrypts the transcript-
    /// bound asset, and returns ONLY that one segment's already-decrypted fMP4 bytes
    /// as the scoped `stream` output. The CEK/IV NEVER cross the boundary; only one
    /// segment's plaintext is ever produced per call (the others are dropped
    /// immediately); and the runtime relays only the CEK-free sealed material per
    /// request, holding no plaintext between segments. Because the read reuses the
    /// transcript-bound unwrap, a reordered/substituted/expired set fails CLOSED
    /// before any byte is returned. Single-node rail only for now — a threshold/
    /// quorum material (or a threshold-provisioned boundary) is refused pending the
    /// streaming wiring of those rails (the existing `open_session_v1` counts path
    /// already proves they decrypt; only the per-segment read is deferred).
    #[cfg(feature = "rail-stream")]
    fn stream_segment(
        &self,
        request: DecryptSessionRequestV1,
        material: SealedDecryptMaterialV1,
        index: usize,
        now_unix: u64,
        render: Option<RenderDirective>,
    ) -> Response {
        // The stream read is media-only: it serves the scoped `stream` output kind.
        if request.output_kind != "stream" {
            return Response::error(
                "invalid_request",
                "stream_segment serves the `stream` output kind only",
            );
        }
        match parse_decrypt_suite(&material.suite) {
            Some(DecryptSuite::PqHybridThreshold) => {}
            Some(DecryptSuite::P256ClassicalCompat) => {
                return Response::error(
                    "invalid_request",
                    "the p256-classical-compat suite is migration-only; the transcript-bound stream requires the PQ-hybrid suite",
                )
            }
            None => return Response::error("invalid_request", "unsupported decrypt suite"),
        }
        // THRESHOLD / QUORUM per-segment streaming (Day 138): a split material is
        // reconstructed through the SAME canonical path `open_session_v1` uses
        // (`open_threshold_plaintexts` — node-set–bound AAD, expiry, in-VM Shamir/XOR
        // reconstruct), then ONLY the indexed segment's already-decrypted bytes are
        // surfaced as the scoped `stream` output. The CEK materializes only in-VM and a
        // single segment's plaintext is produced per call; reordered/substituted/expired
        // sets fail closed in the shared reconstruction before any byte is returned.
        if let Some(share2_b64) = material.sealed_cek_share2_b64.clone() {
            let (plaintexts, _meta, _transcript_hash) =
                match self.open_threshold_plaintexts(&request, &material, &share2_b64, now_unix) {
                    Ok(v) => v,
                    Err(resp) => return resp,
                };
            let segment_count = plaintexts.len();
            let segment = match plaintexts.get(index) {
                Some(s) => s,
                None => {
                    return Response::error(
                        "invalid_request",
                        "segment index is past the end of the asset",
                    )
                }
            };
            return self.respond_segment(&request, &render, segment, segment_count, index);
        }
        // A threshold/quorum-provisioned boundary must receive a split material for a
        // stream read — refuse single-CEK material rather than open it single-node.
        if self.authority_vk2.is_some() {
            return Response::error(
                "not_configured",
                "threshold/quorum boundary requires a split material (sealed_cek_share2) for a stream read",
            );
        }

        let bound = BoundRailMaterial {
            sealed_cek_b64: material.sealed_cek_b64,
            ciphertext_b64: material.ciphertext_b64,
            init_segment_b64: material.init_segment_b64,
            nonce_b64: material.nonce_b64,
            content_hash_b64: material.content_hash_b64,
            extra_segments_b64: material.extra_segments_b64,
            rights_receipt_hash_b64: material.rights_receipt_hash_b64,
        };
        let prepared = match self.prepare_bound_open(&request, &bound, None) {
            Ok(p) => p,
            Err(resp) => return resp,
        };

        // Short-expiry enforcement — before any crypto (mirrors the audited path).
        if now_unix > request.expires_at || now_unix > request.release_receipt.expires_at {
            return Response::error("invalid_request", "the decrypt grant has expired");
        }

        // Unwrap ONCE and decrypt the AAD-bound asset in-VM. A reordered/substituted
        // set recomputes a different segment-digest AAD and fails the unwrap closed,
        // so we never return a byte of a tampered asset.
        let decrypt_result = if prepared.extra_segments.is_empty() {
            rail_shim::decrypt_from_carrier_bound(
                prepared.session,
                &prepared.carrier,
                &prepared.aad,
                &prepared.verifier,
            )
            .map(|(plaintext, meta)| (vec![plaintext], meta))
        } else {
            rail_shim::decrypt_from_carrier_bound_segments(
                prepared.session,
                &prepared.carrier,
                &prepared.extra_segments,
                &prepared.aad,
                &prepared.verifier,
            )
        };
        let (plaintexts, _meta) = match decrypt_result {
            Ok(v) => v,
            Err(_) => {
                return Response::error(
                    "invalid_request",
                    "the decrypt session could not be opened",
                )
            }
        };

        let segment_count = plaintexts.len();
        let segment = match plaintexts.get(index) {
            Some(s) => s,
            None => {
                return Response::error(
                    "invalid_request",
                    "segment index is past the end of the asset",
                )
            }
        };
        self.respond_segment(&request, &render, segment, segment_count, index)
    }

    /// Shape the `stream_segment` response from the decrypted segment. Default: the raw
    /// `stream` payload (ONLY this segment's bytes — no CEK/IV/other segment), byte-identical
    /// to the legacy contract. With a pixel-lock render directive: the object is extracted and
    /// rasterised to a watermarked page image IN-BOUNDARY and ONLY that image egresses — the
    /// plaintext object never leaves this sandbox. Either way the recovered CEK stays in-VM.
    fn respond_segment(
        &self,
        request: &DecryptSessionRequestV1,
        render: &Option<RenderDirective>,
        segment: &[u8],
        segment_count: usize,
        index: usize,
    ) -> Response {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;

        if let Some(directive) = render {
            #[cfg(feature = "pdf-render")]
            {
                return self.render_segment_page(request, directive, segment, index);
            }
            #[cfg(not(feature = "pdf-render"))]
            {
                let _ = directive;
                return Response::error(
                    "not_configured",
                    "this decrypt boundary was not built with the pixel-lock renderer (feature `pdf-render`)",
                );
            }
        }

        // The runtime decodes `segment_b64` and relays the raw bytes to the viewer's MSE
        // SourceBuffer at /media/{session}/segment/{i}.
        Response::ok(json!({
            "schema": "elastos.decrypt.stream-segment/v1",
            "session_id": request.session_id,
            "object_cid": request.object_cid,
            "viewer_interface": request.viewer_interface,
            "output_kind": "stream",
            "segment_index": index,
            "segment_count": segment_count,
            "segment_b64": b64.encode(segment),
        }))
    }

    /// Extract the protected object from the decrypted segment and rasterise the requested
    /// page to a watermarked JPEG, INSIDE this boundary. Returns the image plus the document's
    /// page count so the viewer can page through without ever receiving the source file. Fails
    /// closed (no bytes) on a non-pixel-lock mime, an unparseable object, or a render error —
    /// and never surfaces the plaintext in the response.
    #[cfg(feature = "pdf-render")]
    fn render_segment_page(
        &self,
        request: &DecryptSessionRequestV1,
        directive: &RenderDirective,
        segment: &[u8],
        index: usize,
    ) -> Response {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;

        if !render::is_pixel_lock(&directive.mime) {
            return Response::error(
                "invalid_request",
                "render requested for a mime that is not pixel-lock",
            );
        }
        let object = match render::extract_object_mdat(segment) {
            Ok(object) => object,
            // Coarse failure: never echo the segment/plaintext on the error path.
            Err(_) => {
                return Response::error("decrypt_failed", "could not extract the protected object")
            }
        };

        let page = directive.page.unwrap_or(0);
        let mime_norm = directive.mime.trim().to_ascii_lowercase();

        // WARM the session: decrypt + parse happened above; parse the document ONCE here and
        // hold it for the rest of the open so every later page is a fast rasterise (#1+#4).
        // Keyed by session id so a stale/foreign warm session is replaced, never reused.
        RENDER_CACHE.with(|cell| {
            let mut cache = cell.borrow_mut();
            let need_open = match cache.as_ref() {
                Some(s) => s.session_id != request.session_id || s.mime != mime_norm,
                None => true,
            };
            if need_open {
                match render::RenderSession::open(
                    request.session_id.clone(),
                    &directive.mime,
                    &object,
                ) {
                    Ok(session) => *cache = Some(session),
                    Err(e) => return Response::error("invalid_request", e),
                }
            }
            // The object plaintext (`object`) is dropped after this call; only the parsed handle
            // (which owns its own copy) stays warm. Render the requested page from the warm doc.
            let session = cache.as_mut().expect("warm session just established");
            let content_type = session.page_content_type();
            match session.page(page, directive.max_width, directive.watermark.as_deref()) {
                Ok(image) => Response::ok(json!({
                    "schema": "elastos.decrypt.render-page/v1",
                    "session_id": request.session_id,
                    "object_cid": request.object_cid,
                    "viewer_interface": request.viewer_interface,
                    "output_kind": "stream",
                    "segment_index": index,
                    "page_index": page,
                    "total_pages": session.total_pages,
                    "content_type": content_type,
                    "rendered_b64": b64.encode(image),
                })),
                // Fail closed: a render miss returns the reason only, never the plaintext.
                Err(e) => Response::error("invalid_request", e),
            }
        })
    }

    /// Render a further page from the WARM session established by a prior `StreamSegment`
    /// render — no material, no ciphertext, no quorum round-trip. Fails closed if no warm
    /// session matches `session_id` (the helper must open the document first).
    #[cfg(feature = "pdf-render")]
    fn render_cached_page(
        &self,
        session_id: &str,
        page: u32,
        max_width: Option<u32>,
        watermark: Option<&str>,
    ) -> Response {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;

        RENDER_CACHE.with(|cell| {
            let mut cache = cell.borrow_mut();
            let session = match cache.as_mut() {
                Some(s) if s.session_id == session_id => s,
                _ => {
                    return Response::error(
                        "invalid_request",
                        "no warm pixel-lock session for this id (open the document first)",
                    )
                }
            };
            let content_type = session.page_content_type();
            match session.page(page, max_width, watermark) {
                Ok(image) => Response::ok(json!({
                    "schema": "elastos.decrypt.render-page/v1",
                    "session_id": session_id,
                    "output_kind": "stream",
                    "page_index": page,
                    "total_pages": session.total_pages,
                    "content_type": content_type,
                    "rendered_b64": b64.encode(image),
                })),
                Err(e) => Response::error("invalid_request", e),
            }
        })
    }

    /// 2-of-2 threshold open (Day 97–98): the CEK was XOR-split across two dKMS nodes
    /// at publish, so no single node ever held the whole content key. Each node
    /// re-sealed ITS share to THIS boundary's session key, bound to the SAME decrypt
    /// transcript (only the node verifying key differs). Here, INSIDE the boundary, we
    /// unwrap BOTH sealed shares and reconstruct `cek = share1 ⊕ share2` in `Zeroizing`
    /// before decrypting — so the whole CEK materializes ONLY in the sandbox, never in
    /// `key-provider`. Enforces grant expiry before any crypto and emits the same
    /// scoped audit record as the single-node audited path. Share-1 is verified under
    /// the primary node vk, share-2 under the second (`authority_vk2`); a missing
    /// second vk, a wrong/forged share, or a share sealed for another session/
    /// transcript all fail closed.
    #[cfg(feature = "rail-material")]
    fn open_session_threshold(
        &self,
        request: DecryptSessionRequestV1,
        material: &SealedDecryptMaterialV1,
        share2_b64: &str,
        now_unix: u64,
    ) -> Response {
        match self.open_threshold_plaintexts(&request, material, share2_b64, now_unix) {
            Ok((_plaintexts, meta, transcript_hash)) => {
                let scoped = match scoped_session_response(&request, &meta) {
                    Response::Ok { data: Some(data) } => data,
                    other => return other,
                };
                audited_response(
                    &request,
                    &transcript_hash,
                    now_unix,
                    "opened",
                    "ok",
                    Some(scoped),
                )
            }
            Err(resp) => resp,
        }
    }

    /// Canonical 2-of-2 / 2-of-3 reconstruction (Day 97–116; #10 One Canonical Path).
    /// Provisioning checks, the node-set–bound transcript AAD, expiry enforcement, and the
    /// in-VM Shamir/XOR reconstruction live HERE, ONCE. Returns the reconstructed plaintext
    /// segment(s) + decrypt meta + the transcript hash, or a fail-closed `Response`. The
    /// CALLER chooses what to surface: the scoped counts/audit envelope
    /// (`open_session_threshold`) or one segment's already-decrypted bytes as the `stream`
    /// output (`stream_segment`). The CEK only ever materializes in this sandbox.
    #[cfg(feature = "rail-material")]
    fn open_threshold_plaintexts(
        &self,
        request: &DecryptSessionRequestV1,
        material: &SealedDecryptMaterialV1,
        share2_b64: &str,
        now_unix: u64,
    ) -> Result<(Vec<Vec<u8>>, Value, [u8; 32]), Response> {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;

        // The second node's trusted verifying key must be provisioned for a threshold
        // open — fail closed if a threshold material arrives at a single-node boundary.
        let authority_vk2 = match self.authority_vk2.as_ref() {
            Some(vk) => vk,
            None => return Err(Response::error(
                "not_configured",
                "threshold material requires a second key-authority verifying key (authority_vk2)",
            )),
        };
        // Day 103–104: derive the NODE-SET IDENTITY from the boundary's OWN pinned vks —
        // never from the request/material — and weld it into the transcript AAD below. The
        // sealed shares were bound to the node-set the AUTHORITY rail saw; this boundary
        // rebuilds the AAD over the node-set IT trusts, so a release whose node-set was
        // swapped (one node re-pointed at a different secret-holder) fails the AEAD open
        // HERE, at the cryptographic boundary — not only at descriptor parse upstream.
        let authority_vk = match self.authority_vk.as_ref() {
            Some(vk) => vk,
            None => {
                return Err(Response::error(
                    "not_configured",
                    "trusted key-authority verifying key is not configured",
                ))
            }
        };
        // Day 113–116: a THIRD pinned identity makes this a 2-of-3 QUORUM boundary — the
        // node-set identity then covers ALL THREE secret-holders (any-2 serve, but the SET
        // the producer escrowed to is the trio), byte-identical to the 2-node id otherwise.
        let node_set_id = match self.authority_vk3.as_ref() {
            Some(vk3) => {
                ddrm_envelope::threshold_node_set_id_n(2, &[authority_vk, authority_vk2, vk3])
            }
            None => ddrm_envelope::threshold_node_set_id(2, authority_vk, authority_vk2),
        };

        // Reuse the single-share prepare to get the session, transcript AAD (now node-set
        // bound), and the first node's verifier + carrier (share-1 + the ciphertext segment).
        let bound = BoundRailMaterial {
            sealed_cek_b64: material.sealed_cek_b64.clone(),
            ciphertext_b64: material.ciphertext_b64.clone(),
            init_segment_b64: material.init_segment_b64.clone(),
            nonce_b64: material.nonce_b64.clone(),
            content_hash_b64: material.content_hash_b64.clone(),
            // MULTI-SEGMENT on the threshold/quorum rail: when extras are present, the WHOLE ordered
            // asset is welded into the transcript AAD (node-set-bound AND segment-bound) by
            // `prepare_bound_open`, and the reconstructed CEK drives every segment. Absent ⇒ a
            // single-segment, node-set-bound open (byte-identical to the historical behaviour).
            extra_segments_b64: material.extra_segments_b64.clone(),
            rights_receipt_hash_b64: material.rights_receipt_hash_b64.clone(),
        };
        let prepared = match self.prepare_bound_open(request, &bound, Some(&node_set_id)) {
            Ok(p) => p,
            Err(resp) => return Err(resp),
        };
        let verifier2 =
            match crate::pq_envelope::mldsa::MlDsa65Verifier::from_encoded(authority_vk2) {
                Some(v) => v,
                None => {
                    return Err(Response::error(
                        "not_configured",
                        "configured second key-authority verifying key is malformed",
                    ))
                }
            };
        let sealed_share2 = match b64.decode(share2_b64) {
            Ok(b) => b,
            Err(_) => {
                return Err(Response::error(
                    "invalid_request",
                    "sealed_cek_share2_b64 is not valid base64",
                ))
            }
        };
        // PRE-AUDIT #1: the THIRD quorum share (when the rail served all three) enables
        // cheater detection, and the published CEK commitment is the integrity backstop.
        let sealed_share3 = match material.sealed_cek_share3_b64.as_deref() {
            None => None,
            Some(s) => match b64.decode(s) {
                Ok(b) => Some(b),
                Err(_) => {
                    return Err(Response::error(
                        "invalid_request",
                        "sealed_cek_share3_b64 is not valid base64",
                    ))
                }
            },
        };
        let cek_commitment: Option<[u8; 32]> = match material.cek_commitment_b64.as_deref() {
            None => None,
            Some(s) => match b64.decode(s) {
                Ok(b) if b.len() == 32 => {
                    let mut a = [0u8; 32];
                    a.copy_from_slice(&b);
                    Some(a)
                }
                _ => {
                    return Err(Response::error(
                        "invalid_request",
                        "cek_commitment_b64 must be 32 base64-decoded bytes",
                    ))
                }
            },
        };

        let transcript_hash = {
            use sha2::{Digest, Sha256};
            let h: [u8; 32] = Sha256::digest(&prepared.aad).into();
            h
        };

        // Short-expiry enforcement — before any crypto (mirrors the audited path).
        let expired =
            now_unix > request.expires_at || now_unix > request.release_receipt.expires_at;
        if expired {
            return Err(audited_response(
                request,
                &transcript_hash,
                now_unix,
                "denied",
                "expired",
                None,
            ));
        }

        // 2-of-3 QUORUM (Day 113–116): with a third pinned identity, the two arriving
        // shares are INDEXED Shamir shares from ANY TWO of the three nodes — unwrap each
        // under the pinned identity its x names and Lagrange-combine in `Zeroizing`.
        // Single segment (default) vs MULTI-SEGMENT asset: both reconstruct the ONE split CEK
        // in-VM; the multi-segment path loops the in-VM decrypt over the whole ordered asset
        // (already welded into the AAD), surfacing `segment_count` + the summed `sample_count`.
        let opened: Result<(Vec<Vec<u8>>, Value), String> = if let Some(vk3) =
            self.authority_vk3.as_ref()
        {
            let verifier3 = match crate::pq_envelope::mldsa::MlDsa65Verifier::from_encoded(vk3) {
                Some(v) => v,
                None => {
                    return Err(Response::error(
                        "not_configured",
                        "configured third key-authority verifying key is malformed",
                    ))
                }
            };
            let verifier1 =
                match crate::pq_envelope::mldsa::MlDsa65Verifier::from_encoded(authority_vk) {
                    Some(v) => v,
                    None => {
                        return Err(Response::error(
                            "not_configured",
                            "configured key-authority verifying key is malformed",
                        ))
                    }
                };
            // PRE-AUDIT #1: forward ALL served shares (2 or 3) so the checked reconstruction can
            // cross-check every pair (3-share cheater detection) AND verify the published CEK
            // commitment. A wrong-valued share — well-formed and correctly indexed — now fails the
            // open closed instead of combining into a silently-wrong CEK.
            let mut sealed_shares: Vec<&[u8]> = vec![
                prepared.carrier.sealed_cek.as_slice(),
                sealed_share2.as_slice(),
            ];
            if let Some(share3) = sealed_share3.as_deref() {
                sealed_shares.push(share3);
            }
            rail_shim::decrypt_from_carrier_quorum_checked(
                prepared.session,
                &sealed_shares,
                &prepared.aad,
                &[verifier1, verifier2, verifier3],
                &node_set_id,
                cek_commitment.as_ref(),
                &prepared.carrier.ciphertext_segment,
                &prepared.extra_segments,
                prepared.carrier.init_segment.as_deref(),
            )
        } else {
            // 2-of-2 threshold (XOR): no third share to cross-check — the published commitment is
            // the integrity backstop against a wrong-valued share.
            rail_shim::decrypt_from_carrier_threshold_checked(
                prepared.session,
                &prepared.carrier.sealed_cek,
                &sealed_share2,
                &prepared.aad,
                &prepared.verifier,
                &verifier2,
                &node_set_id,
                cek_commitment.as_ref(),
                &prepared.carrier.ciphertext_segment,
                &prepared.extra_segments,
                prepared.carrier.init_segment.as_deref(),
            )
        };
        match opened {
            Ok((plaintexts, meta)) => Ok((plaintexts, meta, transcript_hash)),
            Err(_) => Err(audited_response(
                request,
                &transcript_hash,
                now_unix,
                "denied",
                "decrypt_failed",
                None,
            )),
        }
    }
}

/// Prepared, transcript-bound open inputs (feature `rail-bind`).
#[cfg(feature = "rail-bind")]
struct PreparedBoundOpen<'a> {
    session: &'a rail_shim::SessionSecret,
    verifier: crate::pq_envelope::mldsa::MlDsa65Verifier,
    aad: Vec<u8>,
    carrier: rail_shim::SealedDecryptBundle,
    /// MULTI-SEGMENT (optional): segments after segment 0 (`carrier.ciphertext_segment`), in order.
    /// Empty ⇒ single-segment open. When non-empty, the whole ordered set is already welded into
    /// `aad` (`segment_digests`), and the audited rail loops the in-VM decrypt over all of them.
    extra_segments: Vec<Vec<u8>>,
}

/// Build the uniform audit envelope (feature `rail-audit`): a scoped, CEK/plaintext-
/// free record of the decision, bound to the transcript hash. On `opened` it carries
/// the scoped session; on `denied` it carries the reason only.
#[cfg(feature = "rail-audit")]
fn audited_response(
    request: &DecryptSessionRequestV1,
    transcript_hash: &[u8; 32],
    now_unix: u64,
    decision: &str,
    reason: &str,
    session: Option<Value>,
) -> Response {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;
    let audit = json!({
        "schema": "elastos.ddrm/decrypt-audit@1",
        "request_id": request.request_id,
        "principal_id": request.principal_id,
        "session_id": request.session_id,
        "object_cid": request.object_cid,
        "action": request.action,
        "suite": DECRYPT_SUITE_ID,
        "provider": DECRYPT_PROVIDER_ID,
        "decision": decision,
        "reason": reason,
        "transcript_hash_b64": b64.encode(transcript_hash),
        "timestamp": now_unix,
    });
    Response::ok(json!({
        "decision": decision,
        "audit": audit,
        // Present only when the session actually opened.
        "session": session,
    }))
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

/// Decrypt a SEQUENCE of CENC media segments under ONE presentation CEK — the multi-segment asset
/// shape (DASH/fMP4: many `moof+mdat` fragments share the content key, with globally-unique
/// per-sample IVs). Loops the single-segment decrypt in-VM, returning each segment's plaintext plus
/// aggregate metadata: `segment_count` and the `sample_count` SUMMED across segments. Fails CLOSED
/// on the FIRST segment that fails (a missing/empty/structurally-corrupt segment stops the whole
/// open — never a partially-decrypted asset), naming the offending segment index. Plaintext stays
/// contained exactly as for a single segment: the caller-facing response carries counts, not bytes.
#[allow(dead_code)]
fn decrypt_session_segments(
    cek_b64: &str,
    ciphertext_segments: &[Vec<u8>],
    init_segment: Option<&[u8]>,
) -> Result<(Vec<Vec<u8>>, Value), String> {
    if ciphertext_segments.is_empty() {
        return Err("multi-segment open requires at least one segment — fail closed".to_string());
    }
    let mut plaintexts = Vec::with_capacity(ciphertext_segments.len());
    let mut total_samples: u64 = 0;
    for (i, segment) in ciphertext_segments.iter().enumerate() {
        let (plaintext, meta) = decrypt_session_segment(cek_b64, segment, init_segment)
            .map_err(|err| format!("segment {i} failed closed: {err}"))?;
        total_samples += meta
            .get("sample_count")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        plaintexts.push(plaintext);
    }
    let meta = json!({
        "is_protected": true,
        "segment_count": ciphertext_segments.len(),
        "sample_count": total_samples,
    });
    Ok((plaintexts, meta))
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
    let mut scoped = json!({
        "schema": DECRYPT_SESSION_SCHEMA,
        "session_id": request.session_id,
        "object_cid": request.object_cid,
        "viewer_interface": request.viewer_interface,
        "output_kind": request.output_kind,
        "is_protected": decrypt_meta.get("is_protected"),
        "sample_count": decrypt_meta.get("sample_count"),
        "expires_at": request.expires_at,
    });
    // MULTI-SEGMENT: surface the segment count ONLY when the decrypt produced one (the multi-segment
    // loop), so a single-segment scoped response stays byte-identical to before.
    if let Some(segment_count) = decrypt_meta.get("segment_count") {
        scoped["segment_count"] = segment_count.clone();
    }
    Response::ok(scoped)
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

/// Offline forensic extraction: read the INVISIBLE watermark from a (possibly leaked/re-saved)
/// rendered image and print the recovered identity. Read-only, no key material — a pure analysis
/// tool over an already-public image. `decrypt-provider --extract-watermark <image>
/// [--verify-grant <grant.json>]`; exit 0 + the string on success, exit 1 if none is recoverable.
/// When the mark is an AUTHENTICATED grant-digest anchor, also prints the wallet prefix + digest;
/// with `--verify-grant`, recomputes the digest from the candidate grant and reports MATCH/NO MATCH.
#[cfg(feature = "pdf-render")]
fn run_extract_watermark(path: &str, verify_grant: Option<&str>) -> i32 {
    let img = match image::open(path) {
        Ok(img) => img.to_rgba8(),
        Err(e) => {
            eprintln!("decrypt-provider: could not read image {path}: {e}");
            return 2;
        }
    };
    let raw = match render::invisible::extract_raw(&img) {
        Some(raw) => raw,
        None => {
            eprintln!("decrypt-provider: no recoverable forensic watermark in {path}");
            return 1;
        }
    };
    if let Some(stamp) = render::invisible::payload_string(&raw) {
        println!("{stamp}");
    }
    let mark = render::invisible::parse_grant_mark(&raw);
    if let Some((prefix, digest)) = mark {
        println!("wallet-prefix: 0x{}", hex_lower(&prefix));
        println!("grant-digest:  {}", hex_lower(&digest));
    }
    match verify_grant {
        Some(grant_src) => verify_extracted_against_grant(mark, grant_src),
        None => 0,
    }
}

/// Lower-case hex of a byte slice (for the forensic CLI's human output).
#[cfg(feature = "pdf-render")]
fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Read a candidate grant's EIP-191 delegation signature hex: accepts a JSON grant file (reads
/// `delegation_sig_hex`) or a file whose contents are the raw signature hex.
#[cfg(all(feature = "pdf-render", feature = "pq-envelope"))]
fn read_grant_sig_hex(grant_src: &str) -> Option<String> {
    let contents = std::fs::read_to_string(grant_src).ok()?;
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&contents) {
        if let Some(sig) = v.get("delegation_sig_hex").and_then(|s| s.as_str()) {
            return Some(sig.to_string());
        }
    }
    let trimmed = contents.trim();
    if !trimmed.is_empty() {
        return Some(trimmed.to_string());
    }
    None
}

/// Read a candidate grant's declared `owner_address` (the buyer wallet), when the source is a JSON
/// grant. `None` for a raw-signature file (no wallet to cross-check against).
#[cfg(all(feature = "pdf-render", feature = "pq-envelope"))]
fn read_grant_owner_address(grant_src: &str) -> Option<String> {
    let contents = std::fs::read_to_string(grant_src).ok()?;
    let v = serde_json::from_str::<serde_json::Value>(&contents).ok()?;
    v.get("owner_address")
        .and_then(|s| s.as_str())
        .map(str::to_string)
}

/// Verify a recovered grant-digest mark against a candidate grant: recompute the digest from the
/// grant's signature via the SHARED `ddrm_envelope::grant_watermark_digest16` (no drift with the
/// embedder) and compare. MATCH ⇒ the buyer's own signature authenticates this leaked frame.
#[cfg(all(feature = "pdf-render", feature = "pq-envelope"))]
fn verify_extracted_against_grant(mark: Option<([u8; 4], [u8; 16])>, grant_src: &str) -> i32 {
    let (prefix, digest) = match mark {
        Some(m) => m,
        None => {
            eprintln!(
                "decrypt-provider: the recovered mark is not an authenticated grant-digest anchor; \
                 nothing to verify"
            );
            return 1;
        }
    };
    let sig_hex = match read_grant_sig_hex(grant_src) {
        Some(s) => s,
        None => {
            eprintln!("decrypt-provider: could not read a delegation signature from {grant_src}");
            return 2;
        }
    };
    // Secondary cross-check (advisory): the mark carries the first 4 wallet bytes; if the grant
    // declares an `owner_address`, confirm they agree. Normalise BOTH sides to lowercase hex first
    // (the mark is lowercase by construction; a checksummed owner_address would otherwise mismatch).
    if let Some(owner) = read_grant_owner_address(grant_src) {
        let owner_hex = render::invisible::normalize_evm_hex(&owner);
        let mark_prefix = hex_lower(&prefix);
        if owner_hex.starts_with(&mark_prefix) {
            println!("wallet cross-check: MATCH — mark prefix 0x{mark_prefix} matches grant owner {owner}");
        } else {
            println!(
                "wallet cross-check: MISMATCH — mark prefix 0x{mark_prefix} vs grant owner {owner}"
            );
        }
    }
    if ddrm_envelope::grant_watermark_digest16(&sig_hex) == digest {
        println!("VERIFY: MATCH — the grant's signature authenticates this mark");
        0
    } else {
        println!("VERIFY: NO MATCH — this grant did not produce this mark");
        1
    }
}

/// `--verify-grant` needs the envelope crate (the shared digest fn). Without `pq-envelope` the
/// forensic CLI still EXTRACTS, but cannot verify — say so rather than silently passing.
#[cfg(all(feature = "pdf-render", not(feature = "pq-envelope")))]
fn verify_extracted_against_grant(_mark: Option<([u8; 4], [u8; 16])>, _grant_src: &str) -> i32 {
    eprintln!(
        "decrypt-provider: --verify-grant requires a build with the `pq-envelope` feature \
         (the shared grant-digest function)"
    );
    2
}

fn main() {
    // Offline forensic-extraction mode (read-only analysis; no stdio protocol, no key material).
    #[cfg(feature = "pdf-render")]
    {
        let args: Vec<String> = std::env::args().collect();
        if let Some(pos) = args.iter().position(|a| a == "--extract-watermark") {
            let path = args.get(pos + 1).cloned().unwrap_or_default();
            if path.is_empty() {
                eprintln!(
                    "usage: decrypt-provider --extract-watermark <image-path> \
                     [--verify-grant <grant.json>]"
                );
                std::process::exit(2);
            }
            let verify_grant = args
                .iter()
                .position(|a| a == "--verify-grant")
                .and_then(|p| args.get(p + 1).cloned());
            std::process::exit(run_extract_watermark(&path, verify_grant.as_deref()));
        }
    }

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
        assert!(
            !serialized.contains(&cek_b64),
            "CEK must not cross the boundary"
        );
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
        assert!(
            !serialized.contains(&cek_b64),
            "CEK must not cross the boundary"
        );
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
        assert!(
            !serialized.contains(&cek_b64),
            "CEK must not cross the boundary"
        );
        assert!(
            !serialized.contains(std::str::from_utf8(&expected).unwrap()),
            "plaintext must not cross the boundary"
        );
    }

    /// Replay the producer's MULTI-SEGMENT round-trip golden (real DASH/fMP4 asset shape): the
    /// real engine sealed SEVERAL `moof+mdat` segments under ONE presentation CEK with
    /// globally-unique per-sample IVs. This provider must decrypt the WHOLE sequence
    /// segment-by-segment, recover each segment's exact bytes, report the segment count + the
    /// samples summed across segments, and leak neither the CEK nor any segment's plaintext across
    /// the scoped boundary (containment holds for the whole asset, not just one fragment).
    #[cfg(all(feature = "vectors", not(feature = "gen-vectors")))]
    #[test]
    fn encrypt_to_decrypt_multisegment_round_trip_golden() {
        let b64 = base64::engine::general_purpose::STANDARD;
        let v: crate::vector_format::RoundTripMultiSegmentVector =
            serde_json::from_str(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/vectors/roundtrip_multisegment_encrypt_to_decrypt.json"
            )))
            .unwrap();

        let cek_b64 = v.cek_b64.clone();
        let segments: Vec<Vec<u8>> = v
            .segments_b64
            .iter()
            .map(|s| b64.decode(s).unwrap())
            .collect();
        let expected: Vec<Vec<u8>> = v
            .expected_plaintexts_b64
            .iter()
            .map(|s| b64.decode(s).unwrap())
            .collect();
        assert!(segments.len() >= 2, "the golden is a multi-segment asset");
        // The segments are genuinely distinct fragments (each is independently content-addressed
        // by the content plane and fetched by its own CIDv1 before reaching this boundary).
        for i in 1..segments.len() {
            assert_ne!(
                segments[i], segments[0],
                "segments are distinct media fragments"
            );
        }

        // Decrypt the whole asset segment-by-segment under the ONE presentation CEK.
        let (plaintexts, meta) = decrypt_session_segments(&cek_b64, &segments, None).unwrap();
        assert_eq!(plaintexts.len(), expected.len(), "every segment recovered");
        for (i, (out, exp)) in plaintexts.iter().zip(expected.iter()).enumerate() {
            let mdat_off = segments[i].len() - exp.len();
            assert_eq!(
                &out[mdat_off..],
                exp.as_slice(),
                "segment {i} recovers the producer's exact bytes"
            );
        }
        assert_eq!(meta["is_protected"], json!(true));
        assert_eq!(meta["segment_count"], json!(segments.len()));
        assert!(
            meta["sample_count"].as_u64().unwrap() >= segments.len() as u64,
            "samples are summed across segments (>= one per segment)"
        );

        // Containment across the WHOLE asset: the scoped response leaks neither the (rail stand-in)
        // CEK nor ANY segment's recovered plaintext.
        let response = scoped_session_response(&decrypt_request(), &meta);
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(
            !serialized.contains(&cek_b64),
            "CEK must not cross the boundary"
        );
        for exp in &expected {
            assert!(
                !serialized.contains(std::str::from_utf8(exp).unwrap()),
                "no segment plaintext crosses the boundary"
            );
        }
    }

    /// A structurally-corrupt or missing segment fails the WHOLE multi-segment open closed (never a
    /// partially-decrypted asset), and the offending segment index is named. An empty asset (no
    /// segments) also fails closed. (Byte-level tampering of a segment is caught earlier, by the
    /// content plane's per-segment CID integrity, before the bytes ever reach this boundary.)
    #[cfg(all(feature = "vectors", not(feature = "gen-vectors")))]
    #[test]
    fn multisegment_open_fails_closed_on_a_bad_segment() {
        let b64 = base64::engine::general_purpose::STANDARD;
        let v: crate::vector_format::RoundTripMultiSegmentVector =
            serde_json::from_str(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/vectors/roundtrip_multisegment_encrypt_to_decrypt.json"
            )))
            .unwrap();
        let cek_b64 = v.cek_b64.clone();
        let mut segments: Vec<Vec<u8>> = v
            .segments_b64
            .iter()
            .map(|s| b64.decode(s).unwrap())
            .collect();

        // Truncate the LAST segment so its box structure is unparseable: the loop must refuse the
        // whole asset (and name the segment), not return the segments it managed to decrypt first.
        let last = segments.last_mut().unwrap();
        last.truncate(8);
        let err = decrypt_session_segments(&cek_b64, &segments, None)
            .expect_err("a corrupt segment must fail the open closed");
        assert!(
            err.contains(&format!("segment {}", segments.len() - 1)),
            "the failure names the offending segment: {err}"
        );

        // An empty asset (no segments) fails closed too.
        assert!(decrypt_session_segments(&cek_b64, &[], None).is_err());
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
            serde_json::to_string(&scoped_session_response(&media_decrypt_request(), &meta))
                .unwrap();

        assert!(
            !serialized.contains(&cek_b64),
            "CEK must not reach the media player"
        );
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
        let segment =
            build_encrypted_segment(b"the quick brown fox jumps over!!", &cek, &[0x22u8; 8]);

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
    fn pq_rail_material(
        seed: [u8; 32],
        cek: &[u8; 16],
        plaintext: &[u8],
    ) -> (
        RailMaterial,
        Vec<u8>,
        crate::rail_shim::SessionSecret,
        Vec<u8>,
    ) {
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
        (
            material,
            sealed,
            crate::rail_shim::SessionSecret::PqHybrid(secret),
            authority_vk,
        )
    }

    #[cfg(feature = "rail-live")]
    #[test]
    fn open_session_live_decrypts_pq_carrier_through_dispatch_without_leaking() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let cek = [0x5Au8; 16];
        let plaintext = b"rail-live: protected payload through the provider";
        let (material, _sealed, session, authority_vk) =
            pq_rail_material([0x33u8; 32], &cek, plaintext);

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
        assert!(
            serialized.contains("\"status\":\"ok\""),
            "live rail must open the session: {serialized}"
        );
        assert!(
            serialized.contains("session:test"),
            "scoped response carries the session id"
        );
        // Containment through the full provider path: neither plaintext nor CEK leak.
        assert!(
            !serialized.contains(std::str::from_utf8(plaintext).unwrap()),
            "decrypted plaintext must never cross the provider boundary to the caller"
        );
        assert!(
            !serialized.contains(&b64.encode(cek)),
            "raw CEK must never cross the boundary"
        );
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
        assert_eq!(
            error_code(resp),
            "decrypt_failed",
            "a tampered carrier must fail closed"
        );
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
    ) -> (
        BoundRailMaterial,
        crate::rail_shim::SessionSecret,
        Vec<u8>,
        Vec<u8>,
    ) {
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
            node_set_id: None,
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
            extra_segments_b64: None,
            rights_receipt_hash_b64: None,
        };
        (
            material,
            crate::rail_shim::SessionSecret::PqHybrid(secret),
            authority_vk,
            pub_bytes,
        )
    }

    #[cfg(feature = "rail-bind")]
    #[test]
    fn open_session_bound_decrypts_matching_transcript_without_leaking() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let req = decrypt_request();
        let cek = [0x5Au8; 16];
        let plaintext = b"transcript-bound payload through the provider";
        let (material, session, authority_vk, pub_bytes) = bound_setup(
            [0x55u8; 32],
            &req,
            &cek,
            plaintext,
            b"nonce-0001",
            &[0xABu8; 32],
        );

        let mut provider = DecryptProvider {
            session: Some(session),
            authority_vk: Some(authority_vk),
            session_pub: Some(pub_bytes),
            authority_vk2: None,
            authority_vk3: None,
        };
        let resp = provider.handle(Request::OpenSessionBound {
            request: Box::new(req),
            material,
        });

        let serialized = serde_json::to_string(&resp).unwrap();
        assert!(
            serialized.contains("\"status\":\"ok\""),
            "matching transcript must open: {serialized}"
        );
        assert!(
            !serialized.contains(std::str::from_utf8(plaintext).unwrap()),
            "plaintext must not leak"
        );
        assert!(!serialized.contains(&b64.encode(cek)), "CEK must not leak");
    }

    #[cfg(feature = "rail-bind")]
    #[test]
    fn open_session_bound_fails_closed_on_replay_against_different_session() {
        let seal_req = decrypt_request();
        let cek = [0x5Au8; 16];
        let (material, session, authority_vk, pub_bytes) = bound_setup(
            [0x66u8; 32],
            &seal_req,
            &cek,
            b"payload",
            b"nonce-0002",
            &[0xABu8; 32],
        );

        // Submit the SAME sealed material under a different session id — exactly the
        // replay the transcript binding must defeat. The boundary rebuilds the AAD
        // from the (different) request, so the GCM tag / signature reject it.
        let mut replay_req = decrypt_request();
        replay_req.session_id = "session:attacker".to_string();

        let mut provider = DecryptProvider {
            session: Some(session),
            authority_vk: Some(authority_vk),
            session_pub: Some(pub_bytes),
            authority_vk2: None,
            authority_vk3: None,
        };
        let resp = provider.handle(Request::OpenSessionBound {
            request: Box::new(replay_req),
            material,
        });
        assert_eq!(
            error_code(resp),
            "decrypt_failed",
            "a CEK bound to one session must not open another"
        );
    }

    #[cfg(feature = "rail-bind")]
    #[test]
    fn open_session_bound_fails_closed_on_nonce_mismatch_and_tamper() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let req = decrypt_request();
        let cek = [0x5Au8; 16];

        // (a) replayed/altered nonce -> rebuilt transcript differs -> fail closed.
        let (mut material, session, authority_vk, pub_bytes) = bound_setup(
            [0x77u8; 32],
            &req,
            &cek,
            b"payload",
            b"nonce-0003",
            &[0xABu8; 32],
        );
        material.nonce_b64 = b64.encode(b"nonce-XXXX");
        let mut provider = DecryptProvider {
            session: Some(session),
            authority_vk: Some(authority_vk),
            session_pub: Some(pub_bytes),
            authority_vk2: None,
            authority_vk3: None,
        };
        let resp = provider.handle(Request::OpenSessionBound {
            request: Box::new(decrypt_request()),
            material,
        });
        assert_eq!(
            error_code(resp),
            "decrypt_failed",
            "a swapped replay nonce must fail closed"
        );

        // (b) tampered sealed carrier -> fail closed.
        let (mut material2, session2, authority_vk2, pub_bytes2) = bound_setup(
            [0x88u8; 32],
            &req,
            &cek,
            b"payload",
            b"nonce-0004",
            &[0xABu8; 32],
        );
        let mut sealed = b64.decode(&material2.sealed_cek_b64).unwrap();
        sealed[0] ^= 0xFF;
        material2.sealed_cek_b64 = b64.encode(&sealed);
        let mut provider2 = DecryptProvider {
            session: Some(session2),
            authority_vk: Some(authority_vk2),
            session_pub: Some(pub_bytes2),
            authority_vk2: None,
            authority_vk3: None,
        };
        let resp2 = provider2.handle(Request::OpenSessionBound {
            request: Box::new(decrypt_request()),
            material: material2,
        });
        assert_eq!(
            error_code(resp2),
            "decrypt_failed",
            "a tampered carrier must fail closed"
        );
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
        assert_eq!(
            init_json["data"]["configured"],
            json!(true),
            "trusted vk pins configured"
        );
        let pub_bytes = b64
            .decode(
                init_json["data"]["decrypt_session_public_key_b64"]
                    .as_str()
                    .unwrap(),
            )
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
            node_set_id: None,
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
            extra_segments_b64: None,
            rights_receipt_hash_b64: None,
        };

        // Open using the MINTED secret the boundary holds (never injected).
        let resp = provider.handle(Request::OpenSessionBound {
            request: Box::new(req),
            material,
        });
        let serialized = serde_json::to_string(&resp).unwrap();
        assert!(
            serialized.contains("\"status\":\"ok\""),
            "minted+published flow must open: {serialized}"
        );
        assert!(
            !serialized.contains(std::str::from_utf8(plaintext).unwrap()),
            "plaintext must not leak"
        );
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

    // --- short-expiry enforcement + audit (feature `rail-audit`) ----------------
    //
    // Anders: "short expiry, audit." A fresh grant opens and emits an `opened`
    // audit record; an expired grant fails closed (`expired`) BEFORE any unwrap and
    // emits a `denied` audit record. The audit envelope is bound to the transcript
    // hash and carries no CEK and no plaintext.

    #[cfg(feature = "rail-audit")]
    #[test]
    fn audited_fresh_grant_opens_and_emits_audit_without_leaking() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let req = decrypt_request();
        let cek = [0x5Au8; 16];
        let plaintext = b"audited fresh-grant payload";
        let (material, session, authority_vk, pub_bytes) = bound_setup(
            [0xA1u8; 32],
            &req,
            &cek,
            plaintext,
            b"nonce-aud-1",
            &[0xABu8; 32],
        );
        let mut provider = DecryptProvider {
            session: Some(session),
            authority_vk: Some(authority_vk),
            session_pub: Some(pub_bytes),
            authority_vk2: None,
            authority_vk3: None,
        };
        let now = 1_850_000_000u64; // before expiry (1_900_000_000)
        let resp = provider.handle(Request::OpenSessionAudited {
            request: Box::new(req),
            material,
            now_unix: now,
        });
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["data"]["decision"], json!("opened"));
        assert_eq!(v["data"]["audit"]["decision"], json!("opened"));
        assert_eq!(v["data"]["audit"]["timestamp"], json!(now));
        assert!(v["data"]["audit"]["transcript_hash_b64"].is_string());
        assert!(
            v["data"]["session"].is_object(),
            "opened carries a scoped session"
        );
        let s = serde_json::to_string(&resp).unwrap();
        assert!(
            !s.contains(std::str::from_utf8(plaintext).unwrap()),
            "plaintext must not leak"
        );
        assert!(!s.contains(&b64.encode(cek)), "CEK must not leak");
    }

    #[cfg(feature = "rail-audit")]
    #[test]
    fn audited_expired_grant_fails_closed_and_audits_denied() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let req = decrypt_request();
        let cek = [0x5Au8; 16];
        let plaintext = b"audited expired-grant payload";
        let (material, session, authority_vk, pub_bytes) = bound_setup(
            [0xA2u8; 32],
            &req,
            &cek,
            plaintext,
            b"nonce-aud-2",
            &[0xABu8; 32],
        );
        let mut provider = DecryptProvider {
            session: Some(session),
            authority_vk: Some(authority_vk),
            session_pub: Some(pub_bytes),
            authority_vk2: None,
            authority_vk3: None,
        };
        let now = 2_000_000_000u64; // past expiry (1_900_000_000)
        let resp = provider.handle(Request::OpenSessionAudited {
            request: Box::new(req),
            material,
            now_unix: now,
        });
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["data"]["decision"], json!("denied"));
        assert_eq!(v["data"]["audit"]["reason"], json!("expired"));
        assert!(
            v["data"]["session"].is_null(),
            "a denied open carries no session"
        );
        // The audit record is CEK/plaintext-free even on deny.
        let s = serde_json::to_string(&resp).unwrap();
        assert!(
            !s.contains(std::str::from_utf8(plaintext).unwrap()),
            "plaintext must not leak"
        );
        assert!(!s.contains(&b64.encode(cek)), "CEK must not leak");
    }

    // --- consolidated suite-tagged SealedDecryptMaterialV1 (feature `rail-material`)
    //
    // The single backend-neutral envelope that folds into the shared contract: the
    // `suite` tag drives profile selection. The product PQ-hybrid suite opens
    // through the audited bound path; the compat suite is rejected on the product
    // path; an unknown suite fails closed.

    #[cfg(feature = "rail-material")]
    fn material_v1(suite: &str, bound: &BoundRailMaterial) -> SealedDecryptMaterialV1 {
        SealedDecryptMaterialV1 {
            suite: suite.to_string(),
            sealed_cek_b64: bound.sealed_cek_b64.clone(),
            sealed_cek_share2_b64: None,
            sealed_cek_share3_b64: None,
            cek_commitment_b64: None,
            ciphertext_b64: bound.ciphertext_b64.clone(),
            init_segment_b64: bound.init_segment_b64.clone(),
            nonce_b64: bound.nonce_b64.clone(),
            content_hash_b64: bound.content_hash_b64.clone(),
            extra_segments_b64: bound.extra_segments_b64.clone(),
            rights_receipt_hash_b64: bound.rights_receipt_hash_b64.clone(),
        }
    }

    #[cfg(feature = "rail-material")]
    #[test]
    fn sealed_material_v1_pq_suite_opens_through_audited_path() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let req = decrypt_request();
        let cek = [0x5Au8; 16];
        let plaintext = b"consolidated envelope payload";
        let (bound, session, authority_vk, pub_bytes) = bound_setup(
            [0xB1u8; 32],
            &req,
            &cek,
            plaintext,
            b"nonce-v1-1",
            &[0xABu8; 32],
        );
        let material = material_v1(DECRYPT_SUITE_ID, &bound);

        let mut provider = DecryptProvider {
            session: Some(session),
            authority_vk: Some(authority_vk),
            session_pub: Some(pub_bytes),
            authority_vk2: None,
            authority_vk3: None,
        };
        let resp = provider.handle(Request::OpenSessionV1 {
            request: Box::new(req),
            material,
            now_unix: 1_850_000_000,
        });
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(
            v["data"]["decision"],
            json!("opened"),
            "PQ suite opens: {v}"
        );
        let s = serde_json::to_string(&resp).unwrap();
        assert!(
            !s.contains(std::str::from_utf8(plaintext).unwrap()),
            "plaintext must not leak"
        );
        assert!(!s.contains(&b64.encode(cek)), "CEK must not leak");
    }

    /// MULTI-SEGMENT through the LIVE `open_session_v1` rail: a 2-fragment asset under ONE CEK opens
    /// in-VM (the key released once), reporting `segment_count == 2` and a SUMMED `sample_count`, with
    /// no CEK/plaintext leak — and a SUBSTITUTED fragment fails the WHOLE open closed because the
    /// transcript's segment-digest binding no longer matches the seal (the unwrap fails before a byte
    /// is decrypted). Proves the additive multi-segment wiring end-to-end at the boundary.
    #[cfg(feature = "rail-material")]
    #[test]
    fn sealed_material_v1_multi_segment_opens_and_substituted_segment_fails_closed() {
        use crate::pq_envelope::seal_support::{gen_session, mldsa_seal_keypair, seal_bound};
        use crate::pq_envelope::session_public_bytes;
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;

        let req = decrypt_request();
        let cek = [0x5Au8; 16];
        let nonce = b"nonce-v1-multiseg";
        let content_hash = [0xABu8; 32];

        let (secret, public) = gen_session();
        let pub_bytes = session_public_bytes(&public);
        let (signer, authority_vk) = mldsa_seal_keypair([0xB7u8; 32]);

        // Two REAL CENC fragments under the ONE CEK (distinct IVs → genuinely distinct fragments).
        let seg0 = build_encrypted_segment(b"multi-seg-fragment-zero", &cek, &[0x01u8; 8]);
        let seg1 = build_encrypted_segment(b"multi-seg-fragment-one!", &cek, &[0x02u8; 8]);
        assert_ne!(seg0, seg1, "the fragments are distinct");

        // Seal the CEK BOUND to the transcript that welds BOTH segments (presentation order).
        let digests = ddrm_envelope::segment_digests(&[seg0.as_slice(), seg1.as_slice()]);
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
            node_set_id: None,
        }
        .to_aad_with_segments(Some(&digests));
        let sealed = seal_bound(&public, &cek, &aad, &signer).to_bytes();

        let make_material = |extra: Vec<String>| SealedDecryptMaterialV1 {
            suite: DECRYPT_SUITE_ID.to_string(),
            sealed_cek_b64: b64.encode(&sealed),
            sealed_cek_share2_b64: None,
            sealed_cek_share3_b64: None,
            cek_commitment_b64: None,
            ciphertext_b64: b64.encode(&seg0),
            init_segment_b64: None,
            nonce_b64: b64.encode(nonce),
            content_hash_b64: b64.encode(content_hash),
            extra_segments_b64: Some(extra),
            rights_receipt_hash_b64: None,
        };

        let mut provider = DecryptProvider {
            session: Some(crate::rail_shim::SessionSecret::PqHybrid(secret)),
            authority_vk: Some(authority_vk),
            session_pub: Some(pub_bytes),
            authority_vk2: None,
            authority_vk3: None,
        };

        // Happy path: the whole 2-segment asset opens under the ONE sealed CEK.
        let resp = provider.handle(Request::OpenSessionV1 {
            request: Box::new(req.clone()),
            material: make_material(vec![b64.encode(&seg1)]),
            now_unix: 1_850_000_000,
        });
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(
            v["data"]["decision"],
            json!("opened"),
            "multi-segment opens: {v}"
        );
        assert_eq!(
            v["data"]["session"]["segment_count"],
            json!(2),
            "both fragments reported"
        );
        assert!(
            v["data"]["session"]["sample_count"].as_u64().unwrap() >= 2,
            "sample_count summed across segments: {v}"
        );
        let s = serde_json::to_string(&resp).unwrap();
        assert!(!s.contains(&b64.encode(cek)), "CEK must not leak");
        assert!(
            !s.contains("multi-seg-fragment"),
            "no fragment plaintext crosses the boundary"
        );

        // Substituted fragment: same valid sealed CEK, but the extra segment's bytes are swapped.
        // The boundary recomputes the segment digests over the tampered set → AAD mismatch → the
        // unwrap fails closed for the WHOLE open (never a partial asset).
        let mut bad_seg1 = seg1.clone();
        let last = bad_seg1.len() - 1;
        bad_seg1[last] ^= 0x01;
        let resp_bad = provider.handle(Request::OpenSessionV1 {
            request: Box::new(req),
            material: make_material(vec![b64.encode(&bad_seg1)]),
            now_unix: 1_850_000_000,
        });
        let v_bad = serde_json::to_value(&resp_bad).unwrap();
        assert_ne!(
            v_bad["data"]["decision"],
            json!("opened"),
            "a substituted fragment must fail the whole open closed: {v_bad}"
        );
    }

    /// B2 viewer seam — the media STREAM read. A 2-fragment media session yields each
    /// segment's ALREADY-DECRYPTED fMP4 bytes addressed by index (what elacity-player
    /// feeds into MSE), in order, with NO CEK and no OTHER segment's plaintext on the
    /// per-segment response. Fail-closed surfaces: an out-of-range index, a substituted
    /// fragment (transcript-bound unwrap mismatch), and a non-`stream` request are all
    /// refused — never a partial or tampered byte stream.
    #[cfg(feature = "rail-stream")]
    #[test]
    fn stream_segment_yields_decrypted_segments_in_order_and_fails_closed() {
        use crate::pq_envelope::seal_support::{gen_session, mldsa_seal_keypair, seal_bound};
        use crate::pq_envelope::session_public_bytes;
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;

        let req = media_decrypt_request();
        let cek = [0x5Au8; 16];
        let nonce = b"nonce-stream-1";
        let content_hash = [0xABu8; 32];

        let (secret, public) = gen_session();
        let pub_bytes = session_public_bytes(&public);
        let (signer, authority_vk) = mldsa_seal_keypair([0xB7u8; 32]);

        let pt0 = b"stream-fragment-zero-plaintext!!";
        let pt1 = b"stream-fragment-one-plaintext!!!";
        let seg0 = build_encrypted_segment(pt0, &cek, &[0x01u8; 8]);
        let seg1 = build_encrypted_segment(pt1, &cek, &[0x02u8; 8]);

        // Seal the CEK BOUND to the transcript welding BOTH segments (presentation order).
        let digests = ddrm_envelope::segment_digests(&[seg0.as_slice(), seg1.as_slice()]);
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
            node_set_id: None,
        }
        .to_aad_with_segments(Some(&digests));
        let sealed = seal_bound(&public, &cek, &aad, &signer).to_bytes();

        let make_material = |extra: Vec<String>| SealedDecryptMaterialV1 {
            suite: DECRYPT_SUITE_ID.to_string(),
            sealed_cek_b64: b64.encode(&sealed),
            sealed_cek_share2_b64: None,
            sealed_cek_share3_b64: None,
            cek_commitment_b64: None,
            ciphertext_b64: b64.encode(&seg0),
            init_segment_b64: None,
            nonce_b64: b64.encode(nonce),
            content_hash_b64: b64.encode(content_hash),
            extra_segments_b64: Some(extra),
            rights_receipt_hash_b64: None,
        };

        let mut provider = DecryptProvider {
            session: Some(crate::rail_shim::SessionSecret::PqHybrid(secret)),
            authority_vk: Some(authority_vk),
            session_pub: Some(pub_bytes),
            authority_vk2: None,
            authority_vk3: None,
        };

        // Each index returns ITS decrypted segment (plaintext at the tail of the fMP4),
        // reports segment_count == 2, and leaks neither the CEK nor the other fragment.
        for (index, pt, other) in [(0usize, pt0.as_slice(), pt1.as_slice()), (1, pt1, pt0)] {
            let resp = provider.handle(Request::StreamSegment {
                request: Box::new(req.clone()),
                material: make_material(vec![b64.encode(&seg1)]),
                index,
                now_unix: 1_850_000_000,
                render: None,
            });
            let v = serde_json::to_value(&resp).unwrap();
            assert_eq!(
                v["data"]["segment_index"],
                json!(index),
                "segment index echoed: {v}"
            );
            assert_eq!(
                v["data"]["segment_count"],
                json!(2),
                "both fragments counted: {v}"
            );
            let bytes = b64
                .decode(v["data"]["segment_b64"].as_str().unwrap())
                .unwrap();
            assert_eq!(
                &bytes[bytes.len() - pt.len()..],
                pt,
                "segment {index} decrypts to its plaintext"
            );
            let s = serde_json::to_string(&resp).unwrap();
            assert!(
                !s.contains(&b64.encode(cek)),
                "CEK must not appear in a stream response"
            );
            assert!(
                !s.contains(std::str::from_utf8(other).unwrap()),
                "no OTHER segment's plaintext crosses on a per-segment read"
            );
        }

        // Out-of-range index fails closed (no byte returned).
        let resp_oob = provider.stream_segment(
            req.clone(),
            make_material(vec![b64.encode(&seg1)]),
            2,
            1_850_000_000,
            None,
        );
        assert_eq!(
            error_code(resp_oob),
            "invalid_request",
            "an out-of-range index is refused"
        );

        // Substituted fragment: valid sealed CEK, but the extra segment's bytes are swapped.
        // The recomputed segment-digest AAD no longer matches the seal → unwrap fails closed.
        let mut bad_seg1 = seg1.clone();
        let last = bad_seg1.len() - 1;
        bad_seg1[last] ^= 0x01;
        let resp_bad = provider.stream_segment(
            req.clone(),
            make_material(vec![b64.encode(&bad_seg1)]),
            0,
            1_850_000_000,
            None,
        );
        assert_eq!(
            error_code(resp_bad),
            "invalid_request",
            "a substituted fragment is refused"
        );

        // A non-`stream` request is refused before any crypto.
        let resp_kind = provider.stream_segment(
            decrypt_request(),
            make_material(vec![b64.encode(&seg1)]),
            0,
            1_850_000_000,
            None,
        );
        assert_eq!(
            error_code(resp_kind),
            "invalid_request",
            "stream serves the stream output kind only"
        );
    }

    #[cfg(feature = "rail-material")]
    #[test]
    fn sealed_material_v1_unknown_suite_fails_closed() {
        let req = decrypt_request();
        let cek = [0x5Au8; 16];
        let (bound, session, authority_vk, pub_bytes) = bound_setup(
            [0xB2u8; 32],
            &req,
            &cek,
            b"payload",
            b"nonce-v1-2",
            &[0xABu8; 32],
        );
        let material = material_v1("totally-unknown-suite-v9", &bound);
        let provider = DecryptProvider {
            session: Some(session),
            authority_vk: Some(authority_vk),
            session_pub: Some(pub_bytes),
            authority_vk2: None,
            authority_vk3: None,
        };
        let resp = provider.open_session_v1(req, material, 1_850_000_000);
        assert_eq!(
            error_code(resp),
            "invalid_request",
            "an unknown suite must fail closed before any crypto"
        );
    }

    #[cfg(feature = "rail-material")]
    #[test]
    fn sealed_material_v1_classical_compat_suite_rejected_on_product_path() {
        let req = decrypt_request();
        let cek = [0x5Au8; 16];
        let (bound, session, authority_vk, pub_bytes) = bound_setup(
            [0xB3u8; 32],
            &req,
            &cek,
            b"payload",
            b"nonce-v1-3",
            &[0xABu8; 32],
        );
        let material = material_v1(DECRYPT_SUITE_CLASSICAL_COMPAT, &bound);
        let provider = DecryptProvider {
            session: Some(session),
            authority_vk: Some(authority_vk),
            session_pub: Some(pub_bytes),
            authority_vk2: None,
            authority_vk3: None,
        };
        let resp = provider.open_session_v1(req, material, 1_850_000_000);
        assert_eq!(
            error_code(resp),
            "invalid_request",
            "compat suite is migration-only on the bound product path"
        );
    }

    // --- 2-of-2 threshold reconstruction (feature `rail-material`, Day 97–98) -----
    //
    // The CEK is XOR-split across two dKMS nodes at publish; each node re-seals ITS
    // share to this boundary's session, bound to the SAME transcript. The boundary
    // unwraps BOTH and reconstructs `cek = share1 ⊕ share2` IN-VM — the whole key
    // never exists in `key-provider`. These pin: the happy 2-of-2 decrypts; an
    // attacker who cannot mint the second node's seal fails closed; and a threshold
    // material at a single-node boundary fails closed.

    #[cfg(feature = "rail-material")]
    #[allow(clippy::too_many_arguments)]
    fn threshold_setup(
        seed_a: [u8; 32],
        seed_b: [u8; 32],
        mask: &[u8; 16],
        seal_req: &DecryptSessionRequestV1,
        cek: &[u8; 16],
        plaintext: &[u8],
        nonce: &[u8],
        content_hash: &[u8],
        bind_node_set: bool,
    ) -> (
        SealedDecryptMaterialV1,
        crate::rail_shim::SessionSecret,
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
    ) {
        use crate::pq_envelope::seal_support::{gen_session, mldsa_seal_keypair, seal_bound};
        use crate::pq_envelope::session_public_bytes;
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;

        let (secret, public) = gen_session();
        let pub_bytes = session_public_bytes(&public);
        let (signer_a, vk_a) = mldsa_seal_keypair(seed_a);
        let (signer_b, vk_b) = mldsa_seal_keypair(seed_b);

        // Both nodes seal BOUND to the exact same decrypt transcript — which, on the
        // threshold rail, carries the NODE-SET IDENTITY over both nodes' vks (Day 103–104),
        // so the seal is welded to exactly this pair of secret-holders.
        let node_set_id = ddrm_envelope::threshold_node_set_id(2, &vk_a, &vk_b);
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
            node_set_id: if bind_node_set {
                Some(&node_set_id)
            } else {
                None
            },
        }
        .to_aad();

        // Producer-side split: node A escrows/re-seals share-1, node B share-2.
        let (share1, share2) = ddrm_envelope::split_cek_xor(cek, mask).expect("split");
        let sealed1 = seal_bound(&public, &share1, &aad, &signer_a).to_bytes();
        let sealed2 = seal_bound(&public, &share2, &aad, &signer_b).to_bytes();
        let segment = build_encrypted_segment(plaintext, cek, &[0x77u8; 8]);

        let material = SealedDecryptMaterialV1 {
            suite: DECRYPT_SUITE_ID.to_string(),
            sealed_cek_b64: b64.encode(&sealed1),
            sealed_cek_share2_b64: Some(b64.encode(&sealed2)),
            sealed_cek_share3_b64: None,
            cek_commitment_b64: None,
            ciphertext_b64: b64.encode(&segment),
            init_segment_b64: None,
            nonce_b64: b64.encode(nonce),
            content_hash_b64: b64.encode(content_hash),
            extra_segments_b64: None,
            rights_receipt_hash_b64: None,
        };
        (
            material,
            crate::rail_shim::SessionSecret::PqHybrid(secret),
            vk_a,
            vk_b,
            pub_bytes,
        )
    }

    #[cfg(feature = "rail-material")]
    #[test]
    fn sealed_material_v1_threshold_2of2_reconstructs_in_vm_and_opens() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let req = decrypt_request();
        let cek = [0x5Au8; 16];
        let mask = [0x3Cu8; 16];
        let plaintext = b"two-of-two threshold reconstructed payload!!";
        let (material, session, vk_a, vk_b, pub_bytes) = threshold_setup(
            [0xC1u8; 32],
            [0xC2u8; 32],
            &mask,
            &req,
            &cek,
            plaintext,
            b"nonce-thr-1",
            &[0xABu8; 32],
            true,
        );
        let mut provider = DecryptProvider {
            session: Some(session),
            authority_vk: Some(vk_a),
            session_pub: Some(pub_bytes),
            authority_vk2: Some(vk_b),
            authority_vk3: None,
        };
        let resp = provider.handle(Request::OpenSessionV1 {
            request: Box::new(req),
            material,
            now_unix: 1_850_000_000,
        });
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(
            v["data"]["decision"],
            json!("opened"),
            "2-of-2 must open: {v}"
        );
        // Neither the CEK nor either raw share crosses the boundary.
        let s = serde_json::to_string(&resp).unwrap();
        assert!(
            !s.contains(std::str::from_utf8(plaintext).unwrap()),
            "plaintext must not leak"
        );
        assert!(!s.contains(&b64.encode(cek)), "CEK must not leak");
        assert!(
            !s.contains(&b64.encode(mask)),
            "share-1 (the mask) must not leak"
        );
    }

    /// An attacker who controls only one node cannot mint the SECOND node's seal: a
    /// second share NOT sealed by the trusted node-B verifying key fails closed at the
    /// signature, before any CEK is reconstructed. (The provisioned `authority_vk2` is
    /// node B's real key; the supplied second share was signed by an unrelated key.)
    #[cfg(feature = "rail-material")]
    #[test]
    fn sealed_material_v1_threshold_unauthorized_second_share_fails_closed() {
        let req = decrypt_request();
        let cek = [0x5Au8; 16];
        let mask = [0x3Cu8; 16];
        let (material, session, vk_a, _vk_b_real, pub_bytes) = threshold_setup(
            [0xD1u8; 32],
            [0xD2u8; 32],
            &mask,
            &req,
            &cek,
            b"payload",
            b"nonce-thr-2",
            &[0xABu8; 32],
            true,
        );
        // Provision a DIFFERENT node-B key than the one that actually sealed share-2,
        // i.e. the second share was not produced by the trusted node.
        use crate::pq_envelope::seal_support::mldsa_seal_keypair;
        let (_attacker_signer, unrelated_vk) = mldsa_seal_keypair([0xEEu8; 32]);
        let mut provider = DecryptProvider {
            session: Some(session),
            authority_vk: Some(vk_a),
            session_pub: Some(pub_bytes),
            authority_vk2: Some(unrelated_vk),
            authority_vk3: None,
        };
        let resp = provider.handle(Request::OpenSessionV1 {
            request: Box::new(req),
            material,
            now_unix: 1_850_000_000,
        });
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(
            v["data"]["decision"],
            json!("denied"),
            "an unauthorized second share must be denied: {v}"
        );
        assert_eq!(v["data"]["audit"]["reason"], json!("decrypt_failed"));
    }

    /// A threshold material (second share present) arriving at a boundary provisioned
    /// for a SINGLE node (no `authority_vk2`) fails closed — never silently degrading
    /// to a one-share open.
    #[cfg(feature = "rail-material")]
    #[test]
    fn sealed_material_v1_threshold_without_second_vk_fails_closed() {
        let req = decrypt_request();
        let cek = [0x5Au8; 16];
        let mask = [0x3Cu8; 16];
        let (material, session, vk_a, _vk_b, pub_bytes) = threshold_setup(
            [0xF1u8; 32],
            [0xF2u8; 32],
            &mask,
            &req,
            &cek,
            b"payload",
            b"nonce-thr-3",
            &[0xABu8; 32],
            true,
        );
        let provider = DecryptProvider {
            session: Some(session),
            authority_vk: Some(vk_a),
            session_pub: Some(pub_bytes),
            authority_vk2: None,
            authority_vk3: None,
        };
        let resp = provider.open_session_v1(req, material, 1_850_000_000);
        assert_eq!(
            error_code(resp),
            "not_configured",
            "threshold material requires a second node vk"
        );
    }

    /// Day 103–104: the transcript AAD BINDS the node-set. Shares sealed by the GENUINE
    /// trusted nodes (both signatures verify) but under a transcript that does NOT carry
    /// the node-set identity fail closed at the AEAD open — the boundary always rebuilds
    /// its threshold AAD over the node-set IT trusts, so a release not welded to that
    /// exact set of secret-holders never opens, even with valid per-share signatures.
    #[cfg(feature = "rail-material")]
    #[test]
    fn sealed_material_v1_threshold_aad_without_node_set_fails_closed() {
        let req = decrypt_request();
        let cek = [0x5Au8; 16];
        let mask = [0x3Cu8; 16];
        // Genuine nodes, genuine shares — but the seal's AAD omits the node-set id.
        let (material, session, vk_a, vk_b, pub_bytes) = threshold_setup(
            [0xA7u8; 32],
            [0xA8u8; 32],
            &mask,
            &req,
            &cek,
            b"payload",
            b"nonce-thr-4",
            &[0xABu8; 32],
            false,
        );
        let mut provider = DecryptProvider {
            session: Some(session),
            authority_vk: Some(vk_a),
            session_pub: Some(pub_bytes),
            authority_vk2: Some(vk_b),
            authority_vk3: None,
        };
        let resp = provider.handle(Request::OpenSessionV1 {
            request: Box::new(req),
            material,
            now_unix: 1_850_000_000,
        });
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(
            v["data"]["decision"],
            json!("denied"),
            "a threshold seal NOT bound to the node-set must be denied: {v}"
        );
        assert_eq!(v["data"]["audit"]["reason"], json!("decrypt_failed"));
    }

    /// Day 103–104: a boundary PROVISIONED for a 2-of-2 threshold (second node vk pinned)
    /// refuses a SINGLE-share material outright — it never silently accepts a release the
    /// key authority degraded to one node (defense-in-depth over the key-provider's own
    /// one-share refusal).
    #[cfg(feature = "rail-material")]
    #[test]
    fn sealed_material_v1_single_share_at_threshold_boundary_fails_closed() {
        let req = decrypt_request();
        let cek = [0x5Au8; 16];
        // A perfectly VALID single-node-sealed material (node A's seal, no second share).
        let (bound, session, vk_a, pub_bytes) = bound_setup(
            [0xB9u8; 32],
            &req,
            &cek,
            b"payload",
            b"nonce-thr-5",
            &[0xABu8; 32],
        );
        let material = SealedDecryptMaterialV1 {
            suite: DECRYPT_SUITE_ID.to_string(),
            sealed_cek_b64: bound.sealed_cek_b64,
            sealed_cek_share2_b64: None,
            sealed_cek_share3_b64: None,
            cek_commitment_b64: None,
            ciphertext_b64: bound.ciphertext_b64,
            init_segment_b64: None,
            nonce_b64: bound.nonce_b64,
            content_hash_b64: bound.content_hash_b64,
            extra_segments_b64: None,
            rights_receipt_hash_b64: None,
        };
        // ... arriving at a boundary provisioned for a 2-of-2 threshold.
        use crate::pq_envelope::seal_support::mldsa_seal_keypair;
        let (_signer_b, vk_b) = mldsa_seal_keypair([0xBAu8; 32]);
        let provider = DecryptProvider {
            session: Some(session),
            authority_vk: Some(vk_a),
            session_pub: Some(pub_bytes),
            authority_vk2: Some(vk_b),
            authority_vk3: None,
        };
        let resp = provider.open_session_v1(req, material, 1_850_000_000);
        assert_eq!(
            error_code(resp),
            "invalid_request",
            "a threshold-provisioned boundary must refuse a single-share material"
        );
    }

    // --- 2-of-3 QUORUM reconstruction (feature `rail-material`, Day 113–116) -----
    //
    // The CEK is Shamir-split over GF(256) across THREE dKMS nodes at publish; each
    // share is escrowed as `x ‖ share` so its coordinate is sealed + authenticated.
    // ANY TWO re-sealed shares reconstruct the CEK in-VM — the rail survives a dead
    // node — and each share must verify under the pinned identity its x names.

    /// Seal the three indexed Shamir shares for a 2-of-3 quorum boundary. Returns the
    /// per-node sealed shares (b64, x order), the VM session, the three node vks, the
    /// session public bytes, and the encrypted segment (b64).
    #[cfg(feature = "rail-material")]
    #[allow(clippy::type_complexity)]
    fn quorum_setup(
        seal_req: &DecryptSessionRequestV1,
        cek: &[u8; 16],
        coeff: &[u8; 16],
        plaintext: &[u8],
        nonce: &[u8],
        content_hash: &[u8],
    ) -> (
        Vec<String>,
        crate::rail_shim::SessionSecret,
        Vec<Vec<u8>>,
        Vec<u8>,
        String,
        String,
    ) {
        use crate::pq_envelope::seal_support::{gen_session, mldsa_seal_keypair, seal_bound};
        use crate::pq_envelope::session_public_bytes;
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;

        let (secret, public) = gen_session();
        let pub_bytes = session_public_bytes(&public);
        let keys: Vec<_> = [[0xE1u8; 32], [0xE2u8; 32], [0xE3u8; 32]]
            .into_iter()
            .map(mldsa_seal_keypair)
            .collect();
        let vks: Vec<Vec<u8>> = keys.iter().map(|(_, vk)| vk.clone()).collect();

        // The quorum transcript binds the node-set identity over ALL THREE members.
        let node_set_id = ddrm_envelope::threshold_node_set_id_n(2, &[&vks[0], &vks[1], &vks[2]]);
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
            node_set_id: Some(&node_set_id),
        }
        .to_aad();

        let shares = ddrm_envelope::split_cek_shamir2(cek, coeff).expect("shamir split");
        let sealed: Vec<String> = shares
            .iter()
            .enumerate()
            .map(|(i, share)| {
                let payload = ddrm_envelope::indexed_share((i + 1) as u8, share);
                b64.encode(seal_bound(&public, &payload, &aad, &keys[i].0).to_bytes())
            })
            .collect();
        let segment = build_encrypted_segment(plaintext, cek, &[0x77u8; 8]);
        // PRE-AUDIT #1: the producer's published CEK commitment — the boundary verifies the
        // reconstructed CEK against it, so a wrong-valued share fails the open closed.
        let commitment_b64 = b64.encode(ddrm_envelope::cek_commitment(&node_set_id, cek));
        (
            sealed,
            crate::rail_shim::SessionSecret::PqHybrid(secret),
            vks,
            pub_bytes,
            b64.encode(&segment),
            commitment_b64,
        )
    }

    /// The happy 2-of-3: EVERY pair of the three nodes' re-sealed shares opens the
    /// session — including the pairs that skip node A or node B — proving the rail
    /// no longer needs any one fixed node, while the CEK still only materializes in-VM.
    #[cfg(feature = "rail-material")]
    #[test]
    fn sealed_material_v1_quorum_2of3_opens_with_any_two_nodes() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let cek = [0x5Au8; 16];
        let coeff = [0x9Du8; 16];
        let plaintext = b"two-of-three quorum reconstructed payload!!";

        for (a, b) in [(0usize, 1usize), (0, 2), (1, 2)] {
            let req = decrypt_request();
            let (sealed, session, vks, pub_bytes, ciphertext_b64, commitment_b64) =
                quorum_setup(&req, &cek, &coeff, plaintext, b"nonce-qrm-1", &[0xABu8; 32]);
            let material = SealedDecryptMaterialV1 {
                suite: DECRYPT_SUITE_ID.to_string(),
                sealed_cek_b64: sealed[a].clone(),
                sealed_cek_share2_b64: Some(sealed[b].clone()),
                sealed_cek_share3_b64: None,
                cek_commitment_b64: Some(commitment_b64),
                ciphertext_b64,
                init_segment_b64: None,
                nonce_b64: b64.encode(b"nonce-qrm-1"),
                content_hash_b64: b64.encode([0xABu8; 32]),
                extra_segments_b64: None,
                rights_receipt_hash_b64: None,
            };
            let mut provider = DecryptProvider {
                session: Some(session),
                authority_vk: Some(vks[0].clone()),
                session_pub: Some(pub_bytes),
                authority_vk2: Some(vks[1].clone()),
                authority_vk3: Some(vks[2].clone()),
            };
            let resp = provider.handle(Request::OpenSessionV1 {
                request: Box::new(req),
                material,
                now_unix: 1_850_000_000,
            });
            let v = serde_json::to_value(&resp).unwrap();
            assert_eq!(
                v["data"]["decision"],
                json!("opened"),
                "quorum pair (node {}, node {}) must open: {v}",
                a + 1,
                b + 1
            );
            let s = serde_json::to_string(&resp).unwrap();
            assert!(
                !s.contains(std::str::from_utf8(plaintext).unwrap()),
                "plaintext must not leak"
            );
            assert!(!s.contains(&b64.encode(cek)), "CEK must not leak");
        }
    }

    /// PRE-AUDIT FINDING #1 (the malicious-share regression). On the LIVE 2-of-3 quorum open
    /// path, a single Byzantine node that returns a VALIDLY-SEALED, correctly-INDEXED, but
    /// WRONG-VALUED share must FAIL THE OPEN CLOSED — never combine into a silently-wrong CEK
    /// the unauthenticated AES-CTR layer would not catch. With all three shares forwarded the
    /// boundary cross-checks every pair (cheater detection); the published commitment is the
    /// backstop; and a degraded (2-share) quorum WITHOUT a commitment is refused outright.
    #[cfg(feature = "rail-material")]
    #[test]
    fn sealed_material_v1_quorum_byzantine_share_fails_closed() {
        use crate::pq_envelope::seal_support::{gen_session, mldsa_seal_keypair, seal_bound};
        use crate::pq_envelope::session_public_bytes;
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;

        let cek = [0x5Au8; 16];
        let coeff = [0x9Du8; 16];
        let plaintext = b"three-share cheater-detection payload!!";
        let nonce = b"nonce-byz-1";
        let content_hash = [0xABu8; 32];
        let req = decrypt_request();

        // Build a coherent 2-of-3 quorum, keeping the node signers + session public so we can
        // forge a WELL-FORMED, correctly-indexed, WRONG-VALUED share sealed by a real node.
        let (secret, public) = gen_session();
        let pub_bytes = session_public_bytes(&public);
        let keys: Vec<_> = [[0xE1u8; 32], [0xE2u8; 32], [0xE3u8; 32]]
            .into_iter()
            .map(mldsa_seal_keypair)
            .collect();
        let vks: Vec<Vec<u8>> = keys.iter().map(|(_, vk)| vk.clone()).collect();
        let node_set_id = ddrm_envelope::threshold_node_set_id_n(2, &[&vks[0], &vks[1], &vks[2]]);
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
            node_set_id: Some(&node_set_id),
        }
        .to_aad();
        let shares = ddrm_envelope::split_cek_shamir2(&cek, &coeff).expect("split");
        let sealed: Vec<String> = shares
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let payload = ddrm_envelope::indexed_share((i + 1) as u8, s);
                b64.encode(seal_bound(&public, &payload, &aad, &keys[i].0).to_bytes())
            })
            .collect();
        let commitment_b64 = b64.encode(ddrm_envelope::cek_commitment(&node_set_id, &cek));
        let segment_b64 = b64.encode(build_encrypted_segment(plaintext, &cek, &[0x77u8; 8]));

        let mut provider = DecryptProvider {
            session: Some(crate::rail_shim::SessionSecret::PqHybrid(secret)),
            authority_vk: Some(vks[0].clone()),
            session_pub: Some(pub_bytes.clone()),
            authority_vk2: Some(vks[1].clone()),
            authority_vk3: Some(vks[2].clone()),
        };
        let base_material = |share2: String, share3: Option<String>, commit: Option<String>| {
            SealedDecryptMaterialV1 {
                suite: DECRYPT_SUITE_ID.to_string(),
                sealed_cek_b64: sealed[0].clone(),
                sealed_cek_share2_b64: Some(share2),
                sealed_cek_share3_b64: share3,
                cek_commitment_b64: commit,
                ciphertext_b64: segment_b64.clone(),
                init_segment_b64: None,
                nonce_b64: b64.encode(nonce),
                content_hash_b64: b64.encode(content_hash),
                extra_segments_b64: None,
                rights_receipt_hash_b64: None,
            }
        };

        // (1) All three HONEST shares open (cheater detection passes; commitment matches).
        let resp = provider.handle(Request::OpenSessionV1 {
            request: Box::new(decrypt_request()),
            material: base_material(
                sealed[1].clone(),
                Some(sealed[2].clone()),
                Some(commitment_b64.clone()),
            ),
            now_unix: 1_850_000_000,
        });
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(
            v["data"]["decision"],
            json!("opened"),
            "three honest shares must open: {v}"
        );

        // (2) A BYZANTINE node C: a validly-sealed, correctly-indexed, WRONG-VALUED share.
        let mut wrong = shares[2].clone();
        wrong[0] ^= 0x01; // still well-formed; just the wrong value
        let malicious = b64.encode(
            seal_bound(
                &public,
                &ddrm_envelope::indexed_share(3, &wrong),
                &aad,
                &keys[2].0,
            )
            .to_bytes(),
        );
        let resp = provider.handle(Request::OpenSessionV1 {
            request: Box::new(decrypt_request()),
            material: base_material(
                sealed[1].clone(),
                Some(malicious.clone()),
                Some(commitment_b64.clone()),
            ),
            now_unix: 1_850_000_000,
        });
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(
            v["data"]["decision"],
            json!("denied"),
            "a single Byzantine wrong-valued share must fail the open closed: {v}"
        );
        let s = serde_json::to_string(&resp).unwrap();
        assert!(
            !s.contains(std::str::from_utf8(plaintext).unwrap()),
            "no plaintext leaks on the Byzantine path"
        );
        assert!(
            !s.contains(&b64.encode(cek)),
            "no CEK leaks on the Byzantine path"
        );

        // (3) Even with the cheater as ONE of only TWO shares, the commitment catches it.
        let resp = provider.handle(Request::OpenSessionV1 {
            request: Box::new(decrypt_request()),
            material: base_material(malicious.clone(), None, Some(commitment_b64.clone())),
            now_unix: 1_850_000_000,
        });
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(
            v["data"]["decision"],
            json!("denied"),
            "the commitment catches a 2-share Byzantine open: {v}"
        );

        // (4) A degraded (2-share) quorum WITHOUT a commitment is refused outright (fail closed).
        let resp = provider.handle(Request::OpenSessionV1 {
            request: Box::new(decrypt_request()),
            material: base_material(sealed[1].clone(), None, None),
            now_unix: 1_850_000_000,
        });
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(
            v["data"]["decision"],
            json!("denied"),
            "an integrity-uncheckable 2-share quorum (no commitment) is refused: {v}"
        );
    }

    /// Day 138 — the 2-of-3 QUORUM STREAM read (the consumer-open RENDER path): every
    /// pair of the three nodes' re-sealed shares reconstructs the CEK in-VM AND the scoped
    /// `stream` output returns that segment's already-decrypted bytes (plaintext at the fMP4
    /// tail), proving the quorum rail can now feed a viewer — while the CEK never leaks.
    #[cfg(feature = "rail-stream")]
    #[test]
    fn stream_segment_quorum_2of3_yields_cleartext_for_any_two_nodes() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let cek = [0x5Au8; 16];
        let coeff = [0x9Du8; 16];
        let plaintext = b"two-of-three quorum STREAM render payload";

        for (a, b) in [(0usize, 1usize), (0, 2), (1, 2)] {
            let req = media_decrypt_request();
            let (sealed, session, vks, pub_bytes, ciphertext_b64, commitment_b64) = quorum_setup(
                &req,
                &cek,
                &coeff,
                plaintext,
                b"nonce-qstream-1",
                &[0xABu8; 32],
            );
            let material = SealedDecryptMaterialV1 {
                suite: DECRYPT_SUITE_ID.to_string(),
                sealed_cek_b64: sealed[a].clone(),
                sealed_cek_share2_b64: Some(sealed[b].clone()),
                sealed_cek_share3_b64: None,
                cek_commitment_b64: Some(commitment_b64),
                ciphertext_b64,
                init_segment_b64: None,
                nonce_b64: b64.encode(b"nonce-qstream-1"),
                content_hash_b64: b64.encode([0xABu8; 32]),
                extra_segments_b64: None,
                rights_receipt_hash_b64: None,
            };
            let mut provider = DecryptProvider {
                session: Some(session),
                authority_vk: Some(vks[0].clone()),
                session_pub: Some(pub_bytes),
                authority_vk2: Some(vks[1].clone()),
                authority_vk3: Some(vks[2].clone()),
            };
            let resp = provider.handle(Request::StreamSegment {
                request: Box::new(req),
                material,
                index: 0,
                now_unix: 1_850_000_000,
                render: None,
            });
            let v = serde_json::to_value(&resp).unwrap();
            let seg_b64 = v["data"]["segment_b64"].as_str().unwrap_or_else(|| {
                panic!(
                    "quorum pair (node {}, node {}) must stream a segment: {v}",
                    a + 1,
                    b + 1
                )
            });
            let seg = b64.decode(seg_b64).expect("segment_b64 decodes");
            assert_eq!(
                &seg[seg.len() - plaintext.len()..],
                plaintext,
                "quorum stream pair (node {}, node {}) must return the cleartext segment",
                a + 1,
                b + 1
            );
            let s = serde_json::to_string(&resp).unwrap();
            assert!(
                !s.contains(&b64.encode(cek)),
                "CEK must not leak on a quorum stream read"
            );
        }
    }

    /// Quorum fail-closed + order-free surfaces, each against its OWN coherent
    /// session: (a) one node's share supplied TWICE is not a quorum (duplicate x);
    /// (b) a share sealed by an UNRELATED key is refused (no pinned identity
    /// verifies it); (c) the quorum is order-free — (node2, node1) opens like
    /// (node1, node2). The deeper mis-index forgery (a genuine node sealing a
    /// payload that claims ANOTHER node's x) is driven cross-binary by the runtime
    /// quorum gates.
    #[cfg(feature = "rail-material")]
    #[test]
    fn sealed_material_v1_quorum_forged_or_duplicate_share_fails_closed() {
        use crate::pq_envelope::seal_support::{gen_session, mldsa_seal_keypair, seal_bound};
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let cek = [0x5Au8; 16];
        let coeff = [0x9Du8; 16];

        // Drive one open against a coherent quorum context, with the two material slots
        // chosen by `pick` from (the three genuine sealed shares, an optional forged blob).
        let drive = |pick: &dyn Fn(&[String], &str) -> (String, String)| -> Value {
            let req = decrypt_request();
            let (sealed, session, vks, pub_bytes, ciphertext_b64, commitment_b64) = quorum_setup(
                &req,
                &cek,
                &coeff,
                b"payload",
                b"nonce-qrm-2",
                &[0xABu8; 32],
            );
            // A forged share: a well-formed indexed payload sealed by an UNRELATED key (to a
            // throwaway session — the pinned-identity signature check refuses it first).
            let (forger, _forger_vk) = mldsa_seal_keypair([0xEEu8; 32]);
            let shares = ddrm_envelope::split_cek_shamir2(&cek, &coeff).expect("split");
            let payload = ddrm_envelope::indexed_share(2, &shares[1]);
            let (_throwaway_secret, throwaway_public) = gen_session();
            let forged =
                b64.encode(seal_bound(&throwaway_public, &payload, b"", &forger).to_bytes());
            let (slot_a, slot_b) = pick(&sealed, &forged);
            let material = SealedDecryptMaterialV1 {
                suite: DECRYPT_SUITE_ID.to_string(),
                sealed_cek_b64: slot_a,
                sealed_cek_share2_b64: Some(slot_b),
                sealed_cek_share3_b64: None,
                cek_commitment_b64: Some(commitment_b64),
                ciphertext_b64,
                init_segment_b64: None,
                nonce_b64: b64.encode(b"nonce-qrm-2"),
                content_hash_b64: b64.encode([0xABu8; 32]),
                extra_segments_b64: None,
                rights_receipt_hash_b64: None,
            };
            let mut provider = DecryptProvider {
                session: Some(session),
                authority_vk: Some(vks[0].clone()),
                session_pub: Some(pub_bytes),
                authority_vk2: Some(vks[1].clone()),
                authority_vk3: Some(vks[2].clone()),
            };
            let resp = provider.handle(Request::OpenSessionV1 {
                request: Box::new(req),
                material,
                now_unix: 1_850_000_000,
            });
            serde_json::to_value(&resp).unwrap()
        };

        // (a) DUPLICATE node: node 1's sealed share twice — the combine refuses x_a == x_b.
        let v = drive(&|sealed, _forged| (sealed[0].clone(), sealed[0].clone()));
        assert_eq!(
            v["data"]["decision"],
            json!("denied"),
            "one node's share twice is NOT a quorum: {v}"
        );

        // (b) FORGED share: no pinned identity verifies the unrelated signer.
        let v = drive(&|sealed, forged| (sealed[0].clone(), forged.to_string()));
        assert_eq!(
            v["data"]["decision"],
            json!("denied"),
            "a forged quorum share must be denied: {v}"
        );

        // (c) ORDER-FREE: (node2, node1) opens exactly like (node1, node2).
        let v = drive(&|sealed, _forged| (sealed[1].clone(), sealed[0].clone()));
        assert_eq!(
            v["data"]["decision"],
            json!("opened"),
            "the quorum is order-free: {v}"
        );
    }

    /// MULTI-SEGMENT on the 2-of-3 QUORUM rail: a 2-fragment asset under ONE Shamir-split CEK opens
    /// in-VM — the CEK reconstructed ONCE from any two of three sealed indexed shares (here skipping
    /// node B), then driving the WHOLE ordered asset — reporting `segment_count == 2` and a SUMMED
    /// `sample_count` with no CEK/share/plaintext leak. A SUBSTITUTED fragment fails the WHOLE open
    /// closed: the boundary's transcript welds the ordered segment digests ALONGSIDE the node-set
    /// identity, so the per-share unwrap fails before a byte is decrypted (never a partial asset).
    #[cfg(feature = "rail-material")]
    #[test]
    fn sealed_material_v1_quorum_multi_segment_opens_and_substituted_segment_fails_closed() {
        use crate::pq_envelope::seal_support::{gen_session, mldsa_seal_keypair, seal_bound};
        use crate::pq_envelope::session_public_bytes;
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;

        let req = decrypt_request();
        let cek = [0x5Au8; 16];
        let coeff = [0x9Du8; 16];
        let nonce = b"nonce-qrm-multiseg";
        let content_hash = [0xABu8; 32];

        let (secret, public) = gen_session();
        let pub_bytes = session_public_bytes(&public);
        let keys: Vec<_> = [[0xE1u8; 32], [0xE2u8; 32], [0xE3u8; 32]]
            .into_iter()
            .map(mldsa_seal_keypair)
            .collect();
        let vks: Vec<Vec<u8>> = keys.iter().map(|(_, vk)| vk.clone()).collect();

        // Two REAL CENC fragments under the ONE split CEK (distinct IVs → distinct fragments).
        let seg0 = build_encrypted_segment(b"quorum-multi-seg-zero!!", &cek, &[0x01u8; 8]);
        let seg1 = build_encrypted_segment(b"quorum-multi-seg-one!!!", &cek, &[0x02u8; 8]);
        assert_ne!(seg0, seg1, "the fragments are distinct");

        // The quorum transcript binds BOTH the node-set identity (all three members) AND the ordered
        // segment set — exactly what `open_session_threshold` rebuilds from the boundary's own state.
        let node_set_id = ddrm_envelope::threshold_node_set_id_n(2, &[&vks[0], &vks[1], &vks[2]]);
        let digests = ddrm_envelope::segment_digests(&[seg0.as_slice(), seg1.as_slice()]);
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
            node_set_id: Some(&node_set_id),
        }
        .to_aad_with_segments(Some(&digests));

        let shares = ddrm_envelope::split_cek_shamir2(&cek, &coeff).expect("shamir split");
        let sealed: Vec<String> = shares
            .iter()
            .enumerate()
            .map(|(i, share)| {
                let payload = ddrm_envelope::indexed_share((i + 1) as u8, share);
                b64.encode(seal_bound(&public, &payload, &aad, &keys[i].0).to_bytes())
            })
            .collect();

        let commitment_b64 = b64.encode(ddrm_envelope::cek_commitment(&node_set_id, &cek));
        let make_material = |extra: Vec<String>| SealedDecryptMaterialV1 {
            suite: DECRYPT_SUITE_ID.to_string(),
            // Open with nodes {0, 2} — a real quorum that SKIPS node B (the rail survives a dead node).
            sealed_cek_b64: sealed[0].clone(),
            sealed_cek_share2_b64: Some(sealed[2].clone()),
            sealed_cek_share3_b64: None,
            cek_commitment_b64: Some(commitment_b64.clone()),
            ciphertext_b64: b64.encode(&seg0),
            init_segment_b64: None,
            nonce_b64: b64.encode(nonce),
            content_hash_b64: b64.encode(content_hash),
            extra_segments_b64: Some(extra),
            rights_receipt_hash_b64: None,
        };

        let mut provider = DecryptProvider {
            session: Some(crate::rail_shim::SessionSecret::PqHybrid(secret)),
            authority_vk: Some(vks[0].clone()),
            session_pub: Some(pub_bytes),
            authority_vk2: Some(vks[1].clone()),
            authority_vk3: Some(vks[2].clone()),
        };

        // Happy path: the whole 2-segment asset opens under the ONE reconstructed quorum CEK.
        let resp = provider.handle(Request::OpenSessionV1 {
            request: Box::new(req.clone()),
            material: make_material(vec![b64.encode(&seg1)]),
            now_unix: 1_850_000_000,
        });
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(
            v["data"]["decision"],
            json!("opened"),
            "quorum multi-segment opens: {v}"
        );
        assert_eq!(
            v["data"]["session"]["segment_count"],
            json!(2),
            "both fragments reported"
        );
        assert!(
            v["data"]["session"]["sample_count"].as_u64().unwrap() >= 2,
            "sample_count summed across segments: {v}"
        );
        let s = serde_json::to_string(&resp).unwrap();
        assert!(!s.contains(&b64.encode(cek)), "CEK must not leak");
        assert!(
            !s.contains("quorum-multi-seg"),
            "no fragment plaintext crosses the boundary"
        );

        // Substituted fragment: same valid sealed shares, but the extra segment's bytes are swapped.
        // The boundary recomputes the segment digests over the tampered set → AAD mismatch → the
        // share unwrap fails closed for the WHOLE open (never a partial asset).
        let mut bad_seg1 = seg1.clone();
        let last = bad_seg1.len() - 1;
        bad_seg1[last] ^= 0x01;
        let resp_bad = provider.handle(Request::OpenSessionV1 {
            request: Box::new(req),
            material: make_material(vec![b64.encode(&bad_seg1)]),
            now_unix: 1_850_000_000,
        });
        let v_bad = serde_json::to_value(&resp_bad).unwrap();
        assert_ne!(
            v_bad["data"]["decision"],
            json!("opened"),
            "a substituted fragment must fail the whole quorum open closed: {v_bad}"
        );
    }
}
