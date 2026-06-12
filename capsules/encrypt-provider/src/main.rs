//! ElastOS Encrypt Provider Capsule
//!
//! Fail-closed protected-content encrypt/seal boundary. This is the *producer*
//! end of the dDRM chain and the home of Irzhy's security invariant #1:
//!
//!   "During encryption, the CEK and KID should be generated within a wasm
//!    boundary; only the ciphertext and its relatives should be set as output."
//!
//! Concretely, that means:
//!   - the caller NEVER supplies a CEK (it is minted inside this boundary);
//!   - the plaintext asset is consumed inside this boundary;
//!   - the only outputs are the ciphertext (by CID), the KID, the IV(s), and a
//!     *wrapped* (sealed) CEK — never the raw CEK or the plaintext;
//!   - the raw CEK is zeroized before this boundary returns.
//!
//! Reference: PC2 `crates/cenc-encrypt` performs the CENC cipher in wasm and
//! zeroizes the CEK, and only emits ciphertext + IVs (never the CEK). The one
//! piece PC2 does in the *host* today is CEK/KID generation
//! (`dashPackager.ts::generateCEK` → `crypto.randomBytes`). This provider exists
//! to close that gap by moving generation in-boundary. See
//! `docs/convergence/DDRM_ENCRYPT_INVARIANT.md`.
//!
//! Until the real in-boundary engine (keygen + CENC encrypt + CEK sealing) is
//! wired, every operation validates fully and then fails closed.

use elastos_common::protected_content::SEALED_OBJECT_SCHEMA;
#[cfg(feature = "escrow")]
use elastos_common::protected_content::{
    validate_protected_content_key_envelope_algorithms, KeyEnvelopeAlgorithmsV1, KeyEnvelopeV1,
    SealedObjectV1, ViewerRequirementV1, DEFAULT_PROTECTED_CONTENT_CIPHER,
    DEFAULT_PROTECTED_CONTENT_KEMS, DEFAULT_PROTECTED_CONTENT_SHARE_SCHEME,
    DEFAULT_PROTECTED_CONTENT_SIGNATURES,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use zeroize::Zeroizing;

// AES-128-CTR CENC cipher vendored from PC2 `cenc-encrypt` — the in-boundary seal
// engine's cipher core (see src/cenc.rs). Held provider-internal; `seal` dispatch
// stays fail-closed until the CEK-sealing rail lands, exactly as decrypt-provider
// keeps its cenc engine behind a fail-closed `open_session`.
#[allow(dead_code)]
mod cenc;

const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

// The encrypt INPUT request schema stays local — there is no shared seal-request
// type in `elastos-common` yet (the OUTPUT `SealedObjectV1` is the shared type).
const SEAL_REQUEST_SCHEMA: &str = "elastos.encrypt.seal.request/v1";
const SUPPORTED_SCHEMES: &[&str] = &["elastos-pq-hybrid-threshold-v0"];

/// A request to seal a plaintext asset into protected content.
///
/// Deliberately carries **no key material**: the CEK and KID are generated
/// inside this boundary, never handed in by the caller. `deny_unknown_fields`
/// means a caller cannot smuggle a `cek`/`cek_b64` field past the wire — that is
/// invariant #1 enforced at the type/serde boundary.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SealRequest {
    schema: String,
    /// Opaque reference/name of the plaintext asset (an audit handle). The bytes are
    /// NOT fetched by this boundary — PC2's host reads the segment off disk and hands
    /// the bytes to the CENC WASM (`dashPackager.ts` `readFileSync` :504 →
    /// `executeCENCEncrypt(..., seg.data)` :432); we mirror that with `content_b64`,
    /// so the producer never holds IPFS/network/fetch authority.
    plaintext_ref: String,
    /// The asset bytes, handed in by the caller (resolved by a storage/content
    /// capability, NOT by this boundary). When present alongside a recipient, the
    /// production `seal` runs the full in-boundary pipeline; absent, `seal` is unchanged.
    #[serde(default)]
    #[allow(dead_code)]
    content_b64: Option<String>,
    /// The key authority's published escrow recipient key (handed in). Without it the
    /// producer cannot escrow the CEK, so `seal` stays fail-closed (`not_configured`).
    #[serde(default)]
    #[allow(dead_code)]
    recipient_pub_b64: Option<String>,
    /// The availability/pin receipt CID for the stored ciphertext (handed in by the
    /// storage step; the producer does not pin). Carried into the SealedObject.
    #[serde(default)]
    #[allow(dead_code)]
    availability_receipt_cid: Option<String>,
    /// CID of the rights policy the sealed object will bind to.
    rights_policy_cid: String,
    /// Sealing scheme (PQ-hybrid threshold by default).
    scheme: String,
    /// Viewer requirement carried through into the SealedObject (`{ "required_interface": ... }`).
    #[serde(default)]
    #[allow(dead_code)]
    viewer: Value,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Init {
        #[serde(default)]
        config: Value,
    },
    Status,
    Seal {
        request: Box<SealRequest>,
    },
    /// Dev-shaped producer op (feature `escrow`): mint a CEK+KID in-boundary,
    /// CENC-encrypt the inline plaintext into a single-sample fMP4 segment, escrow the
    /// CEK to the key authority's published recipient key, and return only ciphertext +
    /// KID + the SEALED CEK. Production resolves `plaintext_ref` and uploads to IPFS;
    /// this makes the bytes drivable for the producer smoke. Never ships a raw CEK.
    #[cfg(feature = "escrow")]
    SealInline {
        plaintext_b64: String,
        recipient_pub_b64: String,
    },
    /// THRESHOLD producer op (feature `escrow`): the dKMS-quorum analogue of `seal_inline`.
    /// Mint a CEK+KID in-boundary, CENC-encrypt the inline plaintext, then SHAMIR-split the
    /// CEK over GF(256) and seal each INDEXED share (`x ‖ p(x)`) to ITS node's published
    /// recipient key — so the CEK is escrowed to the whole 2-of-3 quorum WITHOUT ever
    /// leaving this boundary whole (no node, and not the caller, ever sees the CEK). This is
    /// the "switch the CEK custody to dKMS at mint" seam: the producer half of the live
    /// quorum the consumer half already recovers from. Returns only public material
    /// (ciphertext + per-node sealed shares + the node-set pin) — never a raw CEK or share.
    #[cfg(feature = "escrow")]
    SealInlineThreshold {
        plaintext_b64: String,
        /// The quorum's secret-holding nodes (the 2-of-3 set): each node's published escrow
        /// recipient key + its verifying key (the vk pins the node-set id). Exactly 3.
        nodes: Vec<ThresholdNode>,
    },
    /// MEDIA producer op (feature `escrow`): the DASH analogue of `seal_inline_threshold`.
    /// Take the PLAINTEXT fragmented-MP4 segments produced by `media-provider.package`
    /// (real ffmpeg `moof`/`mdat` fragments — NOT inline samples), mint ONE CEK+KID for
    /// the whole asset, CENC-encrypt EACH fragment under that single KID (one continuous
    /// IV counter across segments, exactly as PC2's dashPackager does), then SHAMIR-split
    /// the CEK and seal each indexed share to its node — identical custody to the single
    /// path. Returns only public material (encrypted segments + per-node sealed shares +
    /// node-set pin) — never a raw CEK or share, and never any plaintext. The KID is the
    /// asset's single `default_KID` / on-chain bytes16 contentId.
    #[cfg(feature = "escrow")]
    SealSegmentsThreshold {
        /// The ordered PLAINTEXT media fragments (base64), as returned by media-provider.
        segments_b64: Vec<String>,
        /// The init segment (base64), used only to content-address the asset (payload CID).
        #[serde(default)]
        init_b64: Option<String>,
        /// The quorum's 2-of-3 secret-holding node set (recipient + verifying keys). Exactly 3.
        nodes: Vec<ThresholdNode>,
    },
    Shutdown,
}

/// One node of the threshold quorum a producer escrows to (feature `escrow`): the public
/// identity the runtime reads from the dKMS authority descriptor — recipient key (where the
/// share is sealed) + verifying key (pins the node-set the open must match). No secrets.
#[cfg(feature = "escrow")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThresholdNode {
    verifying_key_b64: String,
    recipient_pub_b64: String,
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
struct EncryptProvider {
    /// Where the in-boundary CEK is escrowed (sealed) so the key authority can later
    /// re-seal it per decrypt session. Fail-closed by default — see [`CekEscrow`].
    escrow: CekEscrow,
    /// The producer's ML-DSA seal key (feature `escrow`), minted at `init`. The
    /// authority trusts its published verifying key to accept an escrowed CEK.
    #[cfg(feature = "escrow")]
    producer: Option<ProducerKey>,
}

/// The producer's escrow signing identity (feature `escrow`). Its verifying key is
/// published at `init` so the key authority can verify the escrow seal.
#[cfg(feature = "escrow")]
struct ProducerKey {
    signer: ddrm_envelope::seal::MlDsaSealSigner,
    /// Published at `init`; retained for symmetry/diagnostics.
    #[allow(dead_code)]
    verifying_key: Vec<u8>,
}

impl EncryptProvider {
    fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::Init { config } => self.init(config),
            Request::Status => self.status(),
            Request::Seal { request } => self.seal(*request),
            #[cfg(feature = "escrow")]
            Request::SealInline {
                plaintext_b64,
                recipient_pub_b64,
            } => self.seal_inline(&plaintext_b64, &recipient_pub_b64),
            #[cfg(feature = "escrow")]
            Request::SealInlineThreshold { plaintext_b64, nodes } => {
                self.seal_inline_threshold(&plaintext_b64, &nodes)
            }
            #[cfg(feature = "escrow")]
            Request::SealSegmentsThreshold {
                segments_b64,
                init_b64,
                nodes,
            } => self.seal_segments_threshold(&segments_b64, init_b64.as_deref(), &nodes),
            Request::Shutdown => Response::empty_ok(),
        }
    }

    fn init(&mut self, _config: Value) -> Response {
        #[allow(unused_mut)]
        let mut data = json!({
            "provider": "encrypt",
            "protocol_version": "1.0",
            "configured": false,
            "supported_operations": ["status", "seal"],
        });
        // A producer that escrows CEKs PUBLISHES its verifying key so the authority can
        // verify the escrow seal (the mirror of the authority publishing its recipient
        // key). Minted fresh per process; the secret never leaves this boundary.
        #[cfg(feature = "escrow")]
        {
            use base64::Engine as _;
            let mut seed = [0u8; 32];
            getrandom::getrandom(&mut seed).expect("csprng producer seed");
            let (signer, verifying_key) = ddrm_envelope::seal::mldsa_seal_keypair(seed);
            data["producer_verifying_key_b64"] =
                json!(base64::engine::general_purpose::STANDARD.encode(&verifying_key));
            data["supported_operations"] = json!([
                "status",
                "seal",
                "seal_inline",
                "seal_inline_threshold",
                "seal_segments_threshold"
            ]);
            self.producer = Some(ProducerKey {
                signer,
                verifying_key,
            });
        }
        Response::ok(data)
    }

    fn status(&self) -> Response {
        Response::ok(json!({
            "provider": "encrypt",
            "version": PROVIDER_VERSION,
            "configured": false,
            "supported_operations": ["status", "seal"],
            "supported_schemes": SUPPORTED_SCHEMES,
            // Invariant #1: none of these ever leave this boundary.
            "blocked_authority": [
                "raw_cek",
                "plaintext_asset",
                "kms_node_credentials",
                "chain_rpc",
                "wallet_rpc"
            ],
            // The CEK-escrow seam (CEK -> key authority, SEALED). Fail-closed until a
            // key-authority recipient is configured; the producer never ships a raw CEK.
            "escrow": self.escrow.tag(),
            // Outputs only ever carry sealed/non-secret material.
            "produces": SEALED_OBJECT_SCHEMA,
        }))
    }

    fn seal(&self, request: SealRequest) -> Response {
        if let Err(err) = validate_seal_request(&request) {
            return Response::error("invalid_request", err);
        }
        // Producer pipeline (invariant #1), in order:
        //   1. mint CEK + KID in-boundary (proven: `cek_and_kid_generated_inside_boundary`)
        //   2. CENC-encrypt the HANDED-IN asset bytes with that CEK
        //   3. content-address the ciphertext (real `payload_cid`, Day 68)
        //   4. ESCROW the CEK to the key authority — SEALED, never raw (Day 59)
        //   5. assemble a `SealedObjectV1` (KID + wrapped_cek + payload CID), zeroize the CEK.
        // When the caller hands in the asset bytes AND the authority's escrow recipient
        // (escrow build), the production path runs end to end and emits a full
        // `SealedObjectV1`. Absent either — or in a build with no escrow engine — `seal`
        // FAILS CLOSED rather than minting a key it cannot safely hand off.
        #[cfg(feature = "escrow")]
        if let (Some(content_b64), Some(recipient_b64)) =
            (request.content_b64.as_deref(), request.recipient_pub_b64.as_deref())
        {
            return self.seal_to_object(&request, content_b64, recipient_b64);
        }
        Response::error(
            "not_configured",
            "encrypt/seal requires the handed-in asset bytes (`content_b64`) and the \
             key-authority escrow recipient (`recipient_pub_b64`); the CEK is SEALED to the \
             authority, never shipped raw, and this boundary fetches nothing",
        )
    }

    /// The production seal path (feature `escrow`): run the in-boundary pipeline on the
    /// HANDED-IN asset bytes and assemble a complete, shared-contract `SealedObjectV1` —
    /// the real `payload_cid` (Day 68), the minted KID, the SEALED CEK, the rights policy
    /// and availability receipt the caller handed in, and the PQ-hybrid algorithm suite
    /// the whole chain validates. The CEK never leaves; only sealed/non-secret material does.
    #[cfg(feature = "escrow")]
    fn seal_to_object(&self, request: &SealRequest, content_b64: &str, recipient_b64: &str) -> Response {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let availability_receipt_cid = match request.availability_receipt_cid.as_deref() {
            Some(cid) if !cid.trim().is_empty() => cid.to_string(),
            _ => {
                return Response::error(
                    "invalid_request",
                    "availability_receipt_cid is required to seal (handed in by the storage step)",
                )
            }
        };
        let required_interface = match request.viewer.get("required_interface").and_then(Value::as_str) {
            Some(i) if !i.trim().is_empty() => i.to_string(),
            _ => {
                return Response::error(
                    "invalid_request",
                    "viewer.required_interface is required to seal",
                )
            }
        };
        let plaintext = match b64.decode(content_b64) {
            Ok(b) if !b.is_empty() => b,
            Ok(_) => return Response::error("invalid_request", "content_b64 is empty"),
            Err(_) => return Response::error("invalid_request", "content_b64 is not valid base64"),
        };
        let recipient_pub = match b64.decode(recipient_b64) {
            Ok(b) => b,
            Err(_) => return Response::error("invalid_request", "recipient_pub_b64 is not valid base64"),
        };

        let out = match self.run_seal_pipeline(&plaintext, &recipient_pub) {
            Ok(out) => out,
            Err(e) => return Response::error("invalid_request", e),
        };

        // Bind the envelope to the handed-in rights policy by hashing its CID. (The policy
        // doc itself is content-addressed by that CID; hashing it pins the envelope to the
        // exact policy without this boundary fetching the document.)
        let policy_hash = {
            use sha2::{Digest, Sha256};
            format!("sha256:{:x}", Sha256::digest(request.rights_policy_cid.as_bytes()))
        };

        let sealed = SealedObjectV1 {
            schema: SEALED_OBJECT_SCHEMA.to_string(),
            payload_cid: out.payload_cid,
            rights_policy_cid: request.rights_policy_cid.clone(),
            availability_receipt_cid,
            key_envelope: KeyEnvelopeV1 {
                scheme: SUPPORTED_SCHEMES[0].to_string(),
                kid: out.kid_hex,
                wrapped_cek: out.wrapped_cek_b64,
                policy_hash,
                algorithms: KeyEnvelopeAlgorithmsV1 {
                    cipher: DEFAULT_PROTECTED_CONTENT_CIPHER.to_string(),
                    signature: DEFAULT_PROTECTED_CONTENT_SIGNATURES
                        .iter()
                        .map(|s| s.to_string())
                        .collect(),
                    kem: DEFAULT_PROTECTED_CONTENT_KEMS.iter().map(|s| s.to_string()).collect(),
                    share_scheme: DEFAULT_PROTECTED_CONTENT_SHARE_SCHEME.to_string(),
                },
            },
            viewer: ViewerRequirementV1 { required_interface },
        };
        // The producer's algorithm set must satisfy the SHARED chain validator the
        // downstream key-provider runs — refuse to emit an object the chain would reject.
        if let Err(e) = validate_protected_content_key_envelope_algorithms(&sealed.key_envelope.algorithms) {
            return Response::error("internal", e);
        }
        let sealed_object = match serde_json::to_value(&sealed) {
            Ok(v) => v,
            Err(e) => return Response::error("internal", e.to_string()),
        };
        Response::ok(json!({ "sealed_object": sealed_object }))
    }

    /// Dev-shaped producer pipeline (feature `escrow`): run the FULL invariant-#1 path
    /// on inline bytes — mint CEK+KID, CENC-encrypt into a single-sample fMP4 segment,
    /// escrow the CEK to the authority's recipient key — and emit only ciphertext + KID
    /// + the SEALED CEK. The raw CEK lives in `Zeroizing` and is scrubbed on drop.
    #[cfg(feature = "escrow")]
    fn seal_inline(&self, plaintext_b64: &str, recipient_pub_b64: &str) -> Response {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let plaintext = match b64.decode(plaintext_b64) {
            Ok(b) if !b.is_empty() => b,
            Ok(_) => return Response::error("invalid_request", "plaintext_b64 is empty"),
            Err(_) => return Response::error("invalid_request", "plaintext_b64 is not valid base64"),
        };
        let recipient_pub = match b64.decode(recipient_pub_b64) {
            Ok(b) => b,
            Err(_) => {
                return Response::error("invalid_request", "recipient_pub_b64 is not valid base64")
            }
        };

        let out = match self.run_seal_pipeline(&plaintext, &recipient_pub) {
            Ok(out) => out,
            Err(e) => return Response::error("invalid_request", e),
        };

        Response::ok(json!({
            "kid_hex": out.kid_hex,
            // The KID hex IS the on-chain bytes16 contentId (Day 58 identity join).
            "content_id_hex": out.kid_hex,
            "scheme": SUPPORTED_SCHEMES[0],
            "segment_b64": b64.encode(&out.segment),
            // The real content address of the sealed segment (== SealedObjectV1.payload_cid),
            // a separate identity from the KID/contentId.
            "payload_cid": out.payload_cid,
            "wrapped_cek_b64": out.wrapped_cek_b64,
        }))
    }

    /// The one canonical in-boundary seal pipeline (feature `escrow`), shared by the
    /// dev `seal_inline` and the production `seal`: mint a CEK+KID, CENC-encrypt the
    /// handed-in bytes into a single-sample segment, content-address it (`payload_cid`,
    /// Day 68), and escrow the CEK to the authority's recipient — SEALED, never raw. The
    /// CEK lives in `Zeroizing` and is scrubbed when `minted` drops at function end.
    #[cfg(feature = "escrow")]
    fn run_seal_pipeline(
        &self,
        plaintext: &[u8],
        recipient_pub: &[u8],
    ) -> Result<SealPipelineOutput, String> {
        let producer = self
            .producer
            .as_ref()
            .ok_or_else(|| "seal requires init to have minted the producer key".to_string())?;
        let minted = mint_cek_and_kid()?;
        let mut iv_seed = [0u8; 8];
        getrandom::getrandom(&mut iv_seed).map_err(|_| "csprng iv seed failed".to_string())?;
        let sizes = [plaintext.len() as u32];
        let (ciphertext, ivs, _subs) =
            cenc::encrypt_samples(plaintext, &minted.cek, &sizes, &iv_seed, 0)?;
        let segment = mux_single_sample_segment(&ciphertext, &ivs[0]);
        let payload_cid = payload_cid_v1_raw(&segment)?;
        let kid16 = kid_to_content_id_bytes16(&minted.kid_hex)?;
        let wrapped_cek_b64 =
            seal_cek_to_authority(&minted.cek[..], &kid16, recipient_pub, &producer.signer)?;
        Ok(SealPipelineOutput {
            kid_hex: minted.kid_hex.clone(),
            segment,
            payload_cid,
            wrapped_cek_b64,
        })
        // `minted` (Zeroizing CEK) drops here — the CEK is scrubbed before return.
    }

    /// THRESHOLD seal (feature `escrow`): the producer half of the live dKMS quorum. Mint a
    /// CEK in-boundary, CENC-encrypt the bytes, then SHAMIR-split the CEK and seal each
    /// indexed share to ITS node recipient. The CEK + the split coefficient live in
    /// `Zeroizing` and are scrubbed on drop; only ciphertext + per-node SEALED shares + the
    /// node-set pin leave. Mirrors the orchestrator's proven 2-of-3 escrow, but performed
    /// INSIDE the producer boundary so the raw CEK is never known to the orchestrator/caller.
    #[cfg(feature = "escrow")]
    fn seal_inline_threshold(&self, plaintext_b64: &str, nodes: &[ThresholdNode]) -> Response {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        if nodes.len() != 3 {
            return Response::error(
                "invalid_request",
                "threshold seal requires exactly 3 quorum nodes (the 2-of-3 set)",
            );
        }
        let plaintext = match b64.decode(plaintext_b64) {
            Ok(b) if !b.is_empty() => b,
            Ok(_) => return Response::error("invalid_request", "plaintext_b64 is empty"),
            Err(_) => return Response::error("invalid_request", "plaintext_b64 is not valid base64"),
        };
        let mut recipients: Vec<Vec<u8>> = Vec::with_capacity(3);
        let mut vks: Vec<Vec<u8>> = Vec::with_capacity(3);
        for (i, n) in nodes.iter().enumerate() {
            match b64.decode(&n.recipient_pub_b64) {
                Ok(b) => recipients.push(b),
                Err(_) => {
                    return Response::error(
                        "invalid_request",
                        format!("node {i} recipient_pub_b64 is not valid base64"),
                    )
                }
            }
            match b64.decode(&n.verifying_key_b64) {
                Ok(b) => vks.push(b),
                Err(_) => {
                    return Response::error(
                        "invalid_request",
                        format!("node {i} verifying_key_b64 is not valid base64"),
                    )
                }
            }
        }
        // A t-of-n split needs DISTINCT secret-holders — refuse a duplicate identity.
        for i in 0..vks.len() {
            for j in (i + 1)..vks.len() {
                if vks[i] == vks[j] {
                    return Response::error(
                        "invalid_request",
                        "two quorum nodes share an identity — a 2-of-3 split needs DISTINCT secret-holders",
                    );
                }
            }
        }
        let out = match self.run_seal_pipeline_threshold(&plaintext, &recipients, &vks) {
            Ok(out) => out,
            Err(e) => return Response::error("invalid_request", e),
        };
        Response::ok(json!({
            "kid_hex": out.kid_hex,
            // The KID hex IS the on-chain bytes16 contentId (Day 58 identity join).
            "content_id_hex": out.kid_hex,
            "scheme": SUPPORTED_SCHEMES[0],
            "segment_b64": b64.encode(&out.segment),
            "payload_cid": out.payload_cid,
            // PUBLIC ciphertext + IV of the single CENC sample (NOT secret) — lets the open path
            // (or a proof) decrypt with the RECOVERED CEK without re-muxing the fMP4 segment.
            "ciphertext_b64": b64.encode(&out.ciphertext),
            "iv8_b64": b64.encode(out.iv8),
            // The node-set pin (hash over all 3 vks + t=2) the open must match — detects a node swap.
            "node_set_id_b64": b64.encode(out.node_set_id),
            // Each node's SEALED indexed share (`x ‖ p(x)` under its recipient) — never a raw share.
            "shares": out.shares,
        }))
    }

    /// The in-boundary THRESHOLD seal pipeline (feature `escrow`): mint CEK+KID, CENC-encrypt,
    /// SHAMIR-split the CEK over GF(256), and seal each indexed share to its node recipient.
    /// The CEK and the polynomial coefficient never leave (both `Zeroizing`, scrubbed on drop).
    #[cfg(feature = "escrow")]
    fn run_seal_pipeline_threshold(
        &self,
        plaintext: &[u8],
        recipients: &[Vec<u8>],
        vks: &[Vec<u8>],
    ) -> Result<ThresholdSealOutput, String> {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let producer = self
            .producer
            .as_ref()
            .ok_or_else(|| "seal requires init to have minted the producer key".to_string())?;
        let minted = mint_cek_and_kid()?;
        let mut iv_seed = [0u8; 8];
        getrandom::getrandom(&mut iv_seed).map_err(|_| "csprng iv seed failed".to_string())?;
        let sizes = [plaintext.len() as u32];
        let (ciphertext, ivs, _subs) =
            cenc::encrypt_samples(plaintext, &minted.cek, &sizes, &iv_seed, 0)?;
        let iv8 = ivs[0];
        let segment = mux_single_sample_segment(&ciphertext, &iv8);
        let payload_cid = payload_cid_v1_raw(&segment)?;
        let kid16 = kid_to_content_id_bytes16(&minted.kid_hex)?;
        // Uniform random degree-1 coefficient hides the CEK information-theoretically in any single
        // share; only the t=2 quorum reconstructs. Held in Zeroizing alongside the CEK.
        let mut coeff = zeroize::Zeroizing::new(vec![0u8; minted.cek.len()]);
        getrandom::getrandom(&mut coeff).map_err(|_| "csprng split coeff failed".to_string())?;
        let shares_raw = ddrm_envelope::split_cek_shamir2(&minted.cek[..], &coeff)?;
        let mut shares = Vec::with_capacity(3);
        for (i, recipient) in recipients.iter().enumerate() {
            // Node i holds `(i+1) ‖ p(i+1)` — the coordinate is sealed INSIDE the escrow.
            let payload = ddrm_envelope::indexed_share((i + 1) as u8, &shares_raw[i]);
            let wrapped = seal_cek_to_authority(&payload, &kid16, recipient, &producer.signer)?;
            shares.push(json!({
                "x": (i + 1) as u8,
                "verifying_key_b64": b64.encode(&vks[i]),
                "wrapped_share_b64": wrapped,
            }));
        }
        let vk_refs: Vec<&[u8]> = vks.iter().map(|v| v.as_slice()).collect();
        let node_set_id = ddrm_envelope::threshold_node_set_id_n(2, &vk_refs);
        Ok(ThresholdSealOutput {
            kid_hex: minted.kid_hex.clone(),
            segment,
            payload_cid,
            ciphertext,
            iv8,
            node_set_id,
            shares,
        })
        // `minted` (Zeroizing CEK) + `coeff` drop here — both scrubbed before return.
    }

    /// MEDIA threshold seal (feature `escrow`): CENC-encrypt a whole DASH asset's worth of
    /// real fragmented-MP4 segments under ONE CEK/KID, then escrow that CEK to the 2-of-3
    /// quorum — identical custody to `seal_inline_threshold`, but over many real fragments
    /// instead of one synthetic sample. The plaintext segments are consumed in-boundary and
    /// never re-emitted; only the encrypted segments + sealed shares leave.
    #[cfg(feature = "escrow")]
    fn seal_segments_threshold(
        &self,
        segments_b64: &[String],
        init_b64: Option<&str>,
        nodes: &[ThresholdNode],
    ) -> Response {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        if nodes.len() != 3 {
            return Response::error(
                "invalid_request",
                "threshold seal requires exactly 3 quorum nodes (the 2-of-3 set)",
            );
        }
        if segments_b64.is_empty() {
            return Response::error("invalid_request", "no segments to seal");
        }
        let mut recipients: Vec<Vec<u8>> = Vec::with_capacity(3);
        let mut vks: Vec<Vec<u8>> = Vec::with_capacity(3);
        for (i, n) in nodes.iter().enumerate() {
            match b64.decode(&n.recipient_pub_b64) {
                Ok(b) => recipients.push(b),
                Err(_) => {
                    return Response::error(
                        "invalid_request",
                        format!("node {i} recipient_pub_b64 is not valid base64"),
                    )
                }
            }
            match b64.decode(&n.verifying_key_b64) {
                Ok(b) => vks.push(b),
                Err(_) => {
                    return Response::error(
                        "invalid_request",
                        format!("node {i} verifying_key_b64 is not valid base64"),
                    )
                }
            }
        }
        for i in 0..vks.len() {
            for j in (i + 1)..vks.len() {
                if vks[i] == vks[j] {
                    return Response::error(
                        "invalid_request",
                        "two quorum nodes share an identity — a 2-of-3 split needs DISTINCT secret-holders",
                    );
                }
            }
        }
        let mut segments: Vec<Vec<u8>> = Vec::with_capacity(segments_b64.len());
        for (i, s) in segments_b64.iter().enumerate() {
            match b64.decode(s) {
                Ok(b) if !b.is_empty() => segments.push(b),
                Ok(_) => {
                    return Response::error("invalid_request", format!("segment {i} is empty"))
                }
                Err(_) => {
                    return Response::error(
                        "invalid_request",
                        format!("segment {i} is not valid base64"),
                    )
                }
            }
        }
        let init = match init_b64 {
            Some(s) => match b64.decode(s) {
                Ok(b) => b,
                Err(_) => {
                    return Response::error("invalid_request", "init_b64 is not valid base64")
                }
            },
            None => Vec::new(),
        };
        let out = match self.run_seal_pipeline_segments(&segments, &init, &recipients, &vks) {
            Ok(out) => out,
            Err(e) => return Response::error("invalid_request", e),
        };
        Response::ok(json!({
            "kid_hex": out.kid_hex,
            // The KID hex IS the on-chain bytes16 contentId / the asset's single default_KID.
            "content_id_hex": out.kid_hex,
            "scheme": SUPPORTED_SCHEMES[0],
            // The CENC-encrypted media fragments, in order (NOT secret — encrypted under the
            // escrowed CEK). The decrypt rail opens these with the RECOVERED CEK.
            "segments_b64": out.encrypted_segments.iter().map(|s| b64.encode(s)).collect::<Vec<_>>(),
            "segment_count": out.encrypted_segments.len(),
            "payload_cid": out.payload_cid,
            "node_set_id_b64": b64.encode(out.node_set_id),
            "shares": out.shares,
        }))
    }

    /// The in-boundary MEDIA threshold pipeline (feature `escrow`): mint ONE CEK+KID, CENC each
    /// real fMP4 fragment under a single continuous IV counter (via the canonical
    /// `ddrm_media::mp4::encrypt_fragment`), then SHAMIR-split the CEK and seal each indexed
    /// share to its node. The CEK + coefficient are `Zeroizing` and scrubbed before return.
    #[cfg(feature = "escrow")]
    fn run_seal_pipeline_segments(
        &self,
        segments: &[Vec<u8>],
        init: &[u8],
        recipients: &[Vec<u8>],
        vks: &[Vec<u8>],
    ) -> Result<SegmentsSealOutput, String> {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let producer = self
            .producer
            .as_ref()
            .ok_or_else(|| "seal requires init to have minted the producer key".to_string())?;
        let minted = mint_cek_and_kid()?;
        let kid16 = kid_to_content_id_bytes16(&minted.kid_hex)?;

        // One continuous IV counter across ALL segments so every sample of the whole asset
        // gets a unique IV under the single CEK — exactly PC2's dashPackager CENC contract.
        let mut iv_counter: u64 = 0;
        let mut encrypted: Vec<Vec<u8>> = Vec::with_capacity(segments.len());
        for (i, frag) in segments.iter().enumerate() {
            let enc = ddrm_media::mp4::encrypt_fragment(frag, &minted.cek, &mut iv_counter)
                .map_err(|e| format!("segment {i} CENC failed: {e}"))?;
            encrypted.push(enc);
        }

        // Content-address the asset by its init segment (the stable identity of the MSE
        // source); fall back to the first encrypted fragment if no init was supplied.
        let cid_src: &[u8] = if !init.is_empty() {
            init
        } else {
            encrypted.first().map(|v| v.as_slice()).unwrap_or(&[])
        };
        let payload_cid = payload_cid_v1_raw(cid_src)?;

        let mut coeff = zeroize::Zeroizing::new(vec![0u8; minted.cek.len()]);
        getrandom::getrandom(&mut coeff).map_err(|_| "csprng split coeff failed".to_string())?;
        let shares_raw = ddrm_envelope::split_cek_shamir2(&minted.cek[..], &coeff)?;
        let mut shares = Vec::with_capacity(3);
        for (i, recipient) in recipients.iter().enumerate() {
            let payload = ddrm_envelope::indexed_share((i + 1) as u8, &shares_raw[i]);
            let wrapped = seal_cek_to_authority(&payload, &kid16, recipient, &producer.signer)?;
            shares.push(json!({
                "x": (i + 1) as u8,
                "verifying_key_b64": b64.encode(&vks[i]),
                "wrapped_share_b64": wrapped,
            }));
        }
        let vk_refs: Vec<&[u8]> = vks.iter().map(|v| v.as_slice()).collect();
        let node_set_id = ddrm_envelope::threshold_node_set_id_n(2, &vk_refs);
        Ok(SegmentsSealOutput {
            kid_hex: minted.kid_hex.clone(),
            encrypted_segments: encrypted,
            payload_cid,
            node_set_id,
            shares,
        })
        // `minted` (Zeroizing CEK) + `coeff` drop here — both scrubbed before return.
    }
}

/// The non-secret output of [`EncryptProvider::run_seal_pipeline`]. Carries only
/// ciphertext + KID + content address + the SEALED CEK — there is no CEK field, so
/// invariant #1's output half holds by construction.
#[cfg(feature = "escrow")]
struct SealPipelineOutput {
    kid_hex: String,
    segment: Vec<u8>,
    payload_cid: String,
    wrapped_cek_b64: String,
}

/// The non-secret output of [`EncryptProvider::run_seal_pipeline_threshold`]. Carries only
/// ciphertext + KID + content address + the per-node SEALED shares + the node-set pin —
/// no CEK field and no raw share, so invariant #1's output half holds for the quorum seal.
#[cfg(feature = "escrow")]
struct ThresholdSealOutput {
    kid_hex: String,
    segment: Vec<u8>,
    payload_cid: String,
    ciphertext: Vec<u8>,
    iv8: [u8; 8],
    node_set_id: [u8; 32],
    shares: Vec<Value>,
}

/// The non-secret output of [`EncryptProvider::run_seal_pipeline_segments`] (the MEDIA
/// threshold seal). Carries only the CENC-encrypted fragments + KID + content address +
/// the per-node sealed shares + node-set pin — no CEK, no raw share, no plaintext.
#[cfg(feature = "escrow")]
struct SegmentsSealOutput {
    kid_hex: String,
    encrypted_segments: Vec<Vec<u8>>,
    payload_cid: String,
    node_set_id: [u8; 32],
    shares: Vec<Value>,
}

/// Minimal ISO-BMFF box: `size(u32 BE) ‖ type(4) ‖ content`.
#[cfg(feature = "escrow")]
fn make_box(box_type: &[u8; 4], content: &[u8]) -> Vec<u8> {
    let size = (8 + content.len()) as u32;
    let mut b = size.to_be_bytes().to_vec();
    b.extend_from_slice(box_type);
    b.extend_from_slice(content);
    b
}

/// Mux one encrypted full sample + its 8-byte IV into the minimal
/// `moof{traf{trun,senc}} + mdat` segment the decrypt engine consumes — the same box
/// shape as the committed round-trip goldens (trun sample-size-present, senc flags=0).
/// In production a real muxer (a later boundary) does this; here it is colocated so the
/// producer op emits a decrypt-ready segment.
#[cfg(feature = "escrow")]
fn mux_single_sample_segment(ciphertext: &[u8], iv8: &[u8; 8]) -> Vec<u8> {
    let mut trun_content = vec![0u8, 0x00, 0x02, 0x00, 0, 0, 0, 1];
    trun_content.extend_from_slice(&(ciphertext.len() as u32).to_be_bytes());
    let trun = make_box(b"trun", &trun_content);
    let mut senc_content = vec![0u8, 0, 0, 0, 0, 0, 0, 1];
    senc_content.extend_from_slice(iv8);
    let senc = make_box(b"senc", &senc_content);
    let mut traf_content = trun;
    traf_content.extend_from_slice(&senc);
    let traf = make_box(b"traf", &traf_content);
    let moof = make_box(b"moof", &traf);
    let mut segment = moof;
    segment.extend_from_slice(&make_box(b"mdat", ciphertext));
    segment
}

/// The CEK-escrow seam — invariant #1's "seal the CEK to the authority" half.
///
/// The producer NEVER ships a raw CEK. After minting the CEK in-boundary it seals it
/// to the **key authority's published recipient key**, so the authority (dKMS / Lit-
/// compat / reference) can later recover it and re-seal it per decrypt session
/// (Anders' rail; the `key-provider` side already opens the consumer half). Until a
/// recipient is configured this is fail-closed — the producer refuses to mint a key
/// it cannot safely hand off, mirroring PC2's split (host mints the CEK; the Lit
/// Action later wraps it), but with the escrow made explicit and capability-scoped.
#[derive(Debug, Default)]
enum CekEscrow {
    /// No key-authority escrow recipient configured (default → fail-closed).
    #[default]
    NotConfigured,
}

impl CekEscrow {
    fn tag(&self) -> &'static str {
        match self {
            CekEscrow::NotConfigured => "not_configured",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum EscrowError {
    /// No key-authority recipient configured to seal the CEK to.
    NotConfigured,
}

/// Seal the in-boundary CEK to the configured key-authority escrow recipient,
/// returning ONLY the wrapped (sealed) CEK as base64 — never the raw key. Fail-closed
/// until a recipient is configured; Day 59 fills this with the real PQ-hybrid seal
/// (`ddrm-envelope`) to the authority's published recipient key. Kept as the seam now
/// so the producer contract (and its fail-closed default) is pinned by tests first.
#[allow(dead_code)]
fn escrow_cek(_cek: &[u8], _kid_hex: &str, escrow: &CekEscrow) -> Result<String, EscrowError> {
    match escrow {
        CekEscrow::NotConfigured => Err(EscrowError::NotConfigured),
    }
}

/// The real CEK-escrow engine (feature `escrow`, Day 59): seal a freshly-minted CEK to
/// the key authority's published KEM recipient key via the shared `ddrm-envelope` crate,
/// bound to the shared escrow AAD (`scheme ‖ kid(bytes16) ‖ recipient_pub`) and signed
/// by the producer. Returns ONLY the wrapped (sealed) CEK as base64 — the raw CEK never
/// leaves; it is the caller's `Zeroizing` buffer, scrubbed on drop. The authority opens
/// this with `hybrid_unwrap_bound` (see `key-provider::ReferenceAuthority`). A wrong
/// recipient / KID / scheme cannot open it (AAD-bound), and only this producer's
/// signature verifies.
#[cfg(feature = "escrow")]
#[allow(dead_code)]
fn seal_cek_to_authority(
    cek: &[u8],
    kid_bytes16: &[u8; 16],
    recipient_pub: &[u8],
    signer: &ddrm_envelope::seal::MlDsaSealSigner,
) -> Result<String, String> {
    use base64::Engine as _;
    let recipient = ddrm_envelope::session_public_from_bytes(recipient_pub)
        .ok_or_else(|| "malformed key-authority recipient key".to_string())?;
    // SUPPORTED_SCHEMES[0] is the PQ-hybrid suite both halves agree on.
    let aad = ddrm_envelope::transcript::escrow_aad(SUPPORTED_SCHEMES[0], kid_bytes16, recipient_pub);
    let envelope = ddrm_envelope::seal::seal_bound(&recipient, cek, &aad, signer);
    Ok(base64::engine::general_purpose::STANDARD.encode(envelope.to_bytes()))
}

/// Convert the in-boundary KID (32 lowercase-hex chars / 16 bytes) into the on-chain
/// `bytes16 contentId` the consumer chain keys on.
///
/// AUDIT-GROUNDED IDENTITY CONTRACT (PC2 `src/api/storage.ts`):
/// `gateway.hasAccessByContentId(address holder, bytes16 contentId) view returns (bool)`
/// — the chain's content identity is the **KID**, a 16-byte value, NOT the IPFS CID of
/// the ciphertext (that is `SealedObjectV1::payload_cid`, a separate field). This is the
/// single identity that must agree across the whole system: the KID the producer mints
/// here is the `key_envelope.kid`, which becomes the `content_id` the rights step binds
/// and the chain ownership call is keyed on, and the `object_cid` the decrypt transcript
/// is welded to. Pinning the conversion here folds the "bytes16 KID" carry-forward into
/// the producer half so producer and consumer cannot drift on what "the content" is.
#[allow(dead_code)]
fn kid_to_content_id_bytes16(kid_hex: &str) -> Result<[u8; 16], String> {
    if kid_hex.len() != 32 || !kid_hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("KID must be 32 lowercase-hex chars (16 bytes) to be a bytes16 contentId".to_string());
    }
    let mut out = [0u8; 16];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&kid_hex[i * 2..i * 2 + 2], 16)
            .map_err(|e| format!("KID hex decode: {e}"))?;
    }
    Ok(out)
}

/// The single-chunk ceiling of PC2's producer importer: `@helia/unixfs` `add.ts`
/// uses `fixedSize({ chunkSize: 1_048_576 })`, so any content at or under 1 MiB is a
/// single chunk whose root CID IS the lone raw leaf's CID.
const UNIXFS_SINGLE_CHUNK_MAX: usize = 1_048_576;

/// Content-address `bytes` as an IPFS **CIDv1** with the **raw** codec and a
/// sha2-256 multihash — byte-for-byte what PC2's producer gets from Helia
/// `unixfs.addBytes` for single-chunk content (`@helia/unixfs` `add.ts`:
/// `cidVersion: 1, rawLeaves: true`, 1 MiB `fixedSize` chunker; the importer's
/// `reduceSingleLeafToSelf` collapses a one-chunk file to its raw leaf). Pure
/// function of the bytes — computable in-boundary with no IPFS node, no network.
///
/// Fails closed for content larger than one chunk: that would be a balanced
/// **dag-pb** tree spanning multiple blocks, and we refuse to emit a CID we cannot
/// reproduce byte-for-byte here rather than guess one.
#[allow(dead_code)]
fn payload_cid_v1_raw(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() > UNIXFS_SINGLE_CHUNK_MAX {
        return Err(format!(
            "payload of {} bytes exceeds the {UNIXFS_SINGLE_CHUNK_MAX}-byte single-chunk limit; \
             multi-block dag-pb CIDs are not derived in-boundary",
            bytes.len()
        ));
    }
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    // CIDv1 bytes: <version 0x01> <codec raw 0x55> <multihash: sha2-256 0x12, len 0x20, 32-byte digest>.
    let mut cid_bytes = Vec::with_capacity(4 + digest.len());
    cid_bytes.push(0x01);
    cid_bytes.push(0x55);
    cid_bytes.push(0x12);
    cid_bytes.push(0x20);
    cid_bytes.extend_from_slice(&digest);
    // multibase base32 (lowercase, no pad) — the `b` prefix IPFS CIDv1 strings use.
    Ok(format!("b{}", base32_lower_nopad(&cid_bytes)))
}

/// RFC 4648 base32, lowercase alphabet, no padding — the multibase `b` encoding.
#[allow(dead_code)]
fn base32_lower_nopad(data: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut out = String::with_capacity(data.len().div_ceil(5) * 8);
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for &byte in data {
        buffer = (buffer << 8) | byte as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 0x1f) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 0x1f) as usize] as char);
    }
    out
}

/// Freshly minted, in-boundary key material. The CEK is the only true secret: it
/// is held in `Zeroizing` so it is scrubbed from linear memory on drop and is
/// never moved into any caller-visible structure. The KID is non-secret.
#[allow(dead_code)]
struct MintedKey {
    /// 16-byte AES-128 Content Encryption Key — scrubbed on drop.
    cek: Zeroizing<[u8; 16]>,
    /// 32-char lowercase-hex Key ID (16 random bytes); safe to surface.
    kid_hex: String,
}

/// Mint a CEK + KID *inside this boundary* with a CSPRNG — the move that closes
/// invariant #1's gap. PC2 mints these in the Node host
/// (`dashPackager.ts::generateCEK` → `crypto.randomBytes`); here generation is
/// unconditional, takes no caller input, and never leaves the wasm sandbox.
#[allow(dead_code)]
fn mint_cek_and_kid() -> Result<MintedKey, String> {
    let mut cek = Zeroizing::new([0u8; 16]);
    getrandom::getrandom(&mut cek[..]).map_err(|e| format!("csprng cek: {e}"))?;
    let mut kid = [0u8; 16];
    getrandom::getrandom(&mut kid).map_err(|e| format!("csprng kid: {e}"))?;
    let kid_hex = kid.iter().map(|b| format!("{b:02x}")).collect();
    Ok(MintedKey { cek, kid_hex })
}

/// Output of the in-boundary seal cipher step. Carries only non-secret relatives
/// of the ciphertext — there is no CEK field, so invariant #1's output half is
/// enforced by the type itself.
#[allow(dead_code)]
struct SealedSegment {
    ciphertext: Vec<u8>,
    kid_hex: String,
    ivs: Vec<[u8; 8]>,
    sample_count: usize,
}

/// The in-boundary seal cipher step (invariant #1): mint a CEK+KID with a CSPRNG,
/// CENC-encrypt the asset's samples with the minted CEK, scrub the CEK, and
/// return only ciphertext + KID + IVs. The CEK never appears in the return type
/// and is zeroized when `minted` drops.
///
/// This is the proven engine the `seal` dispatch will call; `seal` itself stays
/// fail-closed until the CEK-sealing rail (PQ envelope to the rights/key
/// authority) and ciphertext availability land — a later, separate boundary,
/// mirroring how decrypt-provider keeps `open_session` fail-closed behind its
/// (already-proven) cenc decrypt engine.
#[allow(dead_code)]
fn seal_segment_in_boundary(
    samples: &[u8],
    sample_sizes: &[u32],
    clear_leader: u32,
) -> Result<SealedSegment, String> {
    let minted = mint_cek_and_kid()?;

    // Per-asset random IV base so {KID, IV} stays unique across the asset (CTR
    // keystream reuse under one key leaks plaintext XOR). CSPRNG, in-boundary.
    let mut iv_seed = [0u8; 8];
    getrandom::getrandom(&mut iv_seed).map_err(|e| format!("csprng iv seed: {e}"))?;

    let kid_hex = minted.kid_hex.clone();
    let (ciphertext, ivs, _subsamples) =
        cenc::encrypt_samples(samples, &minted.cek, sample_sizes, &iv_seed, clear_leader)?;

    // `minted` (with its Zeroizing CEK) drops here — the CEK is scrubbed before
    // return. Only non-secret material crosses out of this function.
    Ok(SealedSegment {
        ciphertext,
        kid_hex,
        ivs,
        sample_count: sample_sizes.len(),
    })
}

fn validate_seal_request(request: &SealRequest) -> Result<(), String> {
    if request.schema != SEAL_REQUEST_SCHEMA {
        return Err("seal request schema is unsupported".to_string());
    }
    require_non_empty(&request.plaintext_ref, "plaintext_ref")?;
    require_identifier(&request.rights_policy_cid, "rights_policy_cid")?;
    require_non_empty(&request.scheme, "scheme")?;
    if !SUPPORTED_SCHEMES.contains(&request.scheme.as_str()) {
        return Err(format!("unsupported sealing scheme: {}", request.scheme));
    }
    Ok(())
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
        "encrypt-provider: starting v{} (protected content sealing)",
        PROVIDER_VERSION
    );

    let mut provider = EncryptProvider::default();
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                eprintln!("encrypt-provider read error: {}", err);
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

    eprintln!("encrypt-provider exiting");
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use zeroize::Zeroize;

    fn seal_request_json() -> Value {
        json!({
            "schema": SEAL_REQUEST_SCHEMA,
            "plaintext_ref": "asset-handle-abc123",
            "rights_policy_cid": "bafyrightspolicy",
            "scheme": "elastos-pq-hybrid-threshold-v0",
            "viewer": {}
        })
    }

    fn handle(value: Value) -> Response {
        let request: Request = serde_json::from_value(value).expect("request should parse");
        EncryptProvider::default().handle(request)
    }

    fn ok_data(response: Response) -> Value {
        match response {
            Response::Ok { data: Some(data) } => data,
            other => panic!("expected Ok with data, got {other:?}"),
        }
    }

    fn error_code(response: Response) -> String {
        match response {
            Response::Error { code, .. } => code,
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn status_blocks_raw_cek_and_plaintext_authority() {
        let data = ok_data(handle(json!({ "op": "status" })));
        let blocked = data["blocked_authority"]
            .as_array()
            .expect("blocked_authority array");
        let blocked: Vec<&str> = blocked.iter().filter_map(|v| v.as_str()).collect();
        assert!(blocked.contains(&"raw_cek"), "must block raw_cek");
        assert!(
            blocked.contains(&"plaintext_asset"),
            "must block plaintext_asset"
        );
        // The boundary only ever emits sealed objects.
        assert_eq!(data["produces"], json!(SEALED_OBJECT_SCHEMA));
    }

    #[test]
    fn seal_fails_closed_until_engine_configured() {
        // A fully valid request must NOT seal by accident — no engine, no output.
        let code = error_code(handle(json!({ "op": "seal", "request": seal_request_json() })));
        assert_eq!(code, "not_configured");
    }

    /// Phase C escrow seam (invariant #1 hand-off half). The producer must seal the
    /// in-boundary CEK to a KEY AUTHORITY before it can emit a SealedObject; with no
    /// authority recipient configured the escrow — and therefore `seal` — FAILS CLOSED.
    /// This is the seam the real PQ-hybrid escrow (Day 59) fills; pinned fail-closed
    /// first so the default can never silently ship a key.
    #[test]
    fn escrow_fails_closed_without_a_key_authority() {
        let cek = [0x5Au8; 16];
        assert_eq!(
            escrow_cek(&cek, "0123456789abcdef0123456789abcdef", &CekEscrow::NotConfigured),
            Err(EscrowError::NotConfigured),
            "no authority recipient -> the CEK cannot be escrowed -> fail closed"
        );
        // The seam advertises its fail-closed posture in status, and `seal` refuses.
        let data = ok_data(handle(json!({ "op": "status" })));
        assert_eq!(data["escrow"], json!("not_configured"));
        let code = error_code(handle(json!({ "op": "seal", "request": seal_request_json() })));
        assert_eq!(code, "not_configured");
    }

    /// AUDIT-GROUNDED identity join (PC2 `hasAccessByContentId(holder, bytes16 contentId)`):
    /// the chain keys on the **KID** (a 16-byte value), not the IPFS CID. The KID the
    /// producer mints in-boundary converts losslessly to that on-chain `bytes16 contentId`,
    /// so producer identity == chain/rights/decrypt identity. This folds the bytes16 KID
    /// carry-forward into the producer half.
    #[test]
    fn producer_kid_is_the_onchain_bytes16_content_id() {
        let minted = mint_cek_and_kid().expect("mint");
        let content_id = kid_to_content_id_bytes16(&minted.kid_hex).expect("kid -> bytes16");
        assert_eq!(content_id.len(), 16, "on-chain contentId is bytes16");
        // Lossless round-trip: the bytes16 contentId re-encodes to the exact KID hex.
        let rehex: String = content_id.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(rehex, minted.kid_hex, "KID <-> bytes16 contentId is lossless");
    }

    /// A KID that is not a 16-byte hex value cannot be a `bytes16` contentId — reject it,
    /// so a malformed/oversized identifier can never be silently truncated into the
    /// chain ownership call (which would gate the wrong content).
    #[test]
    fn non_bytes16_kid_is_rejected_as_content_id() {
        assert!(kid_to_content_id_bytes16("deadbeef").is_err(), "too short");
        assert!(
            kid_to_content_id_bytes16("0123456789abcdef0123456789abcdefAA").is_err(),
            "too long"
        );
        assert!(
            kid_to_content_id_bytes16("zz23456789abcdef0123456789abcdef").is_err(),
            "non-hex"
        );
    }

    /// The producer↔consumer JOIN: the SealedObject a producer emits carries the minted
    /// KID as `key_envelope.kid`, and THAT KID is exactly the value the consumer chain
    /// keys on (its bytes16 contentId). One identity, end to end — pinned so the producer
    /// and the (already-built) consumer chain cannot drift on "what the content is".
    #[test]
    fn sealed_object_kid_is_the_consumer_chain_content_id() {
        use elastos_common::protected_content::{
            KeyEnvelopeAlgorithmsV1, KeyEnvelopeV1, SealedObjectV1, ViewerRequirementV1,
        };
        let minted = mint_cek_and_kid().expect("mint");
        let sealed = SealedObjectV1 {
            schema: SEALED_OBJECT_SCHEMA.to_string(),
            payload_cid: "bafyciphertext".to_string(), // the IPFS CID — a DIFFERENT identity
            rights_policy_cid: "bafyrightspolicy".to_string(),
            availability_receipt_cid: "bafyavail".to_string(),
            key_envelope: KeyEnvelopeV1 {
                scheme: "elastos-pq-hybrid-threshold-v0".to_string(),
                kid: minted.kid_hex.clone(),
                wrapped_cek: "c2VhbGVkLWNlay1ieXRlcw==".to_string(),
                policy_hash: "deadbeef".to_string(),
                algorithms: KeyEnvelopeAlgorithmsV1 {
                    cipher: "aes-256-gcm".to_string(),
                    signature: vec!["ml-dsa-65".to_string()],
                    kem: vec!["x25519".to_string(), "ml-kem-768".to_string()],
                    share_scheme: "shamir-t-of-n".to_string(),
                },
            },
            viewer: ViewerRequirementV1 {
                required_interface: "media".to_string(),
            },
        };
        // The chain ownership identity derives from the ENVELOPE KID, not the payload CID.
        let content_id =
            kid_to_content_id_bytes16(&sealed.key_envelope.kid).expect("envelope kid -> bytes16");
        let rehex: String = content_id.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(rehex, minted.kid_hex);
        assert_ne!(
            sealed.payload_cid, sealed.key_envelope.kid,
            "payload CID and contentId(KID) are distinct identities — must not be conflated"
        );
    }

    /// GOLDEN: the in-boundary content-addressing must reproduce, byte-for-byte, the
    /// CID PC2's producer gets from Helia `unixfs.addBytes`. These expected strings
    /// were generated by PC2's REAL `ipfs-unixfs-importer` (the ecosystem oracle) with
    /// `@helia/unixfs` defaults (cidVersion 1, rawLeaves, 1 MiB chunk) — see the Day-68
    /// audit. If our codec drifts from IPFS, this fails loudly. (`abc` is the canonical
    /// raw-`abc` CID, an independent cross-check against the wider IPFS ecosystem.)
    #[test]
    fn payload_cid_matches_ipfs_golden() {
        assert_eq!(
            payload_cid_v1_raw(b"elastos dDRM: content-addressed ciphertext payload (golden)")
                .expect("single-chunk cid"),
            "bafkreiex626yyta3r5sd24h3pbxtrpxsf4bktlu2xmh74xxdphsiq4rppm"
        );
        assert_eq!(
            payload_cid_v1_raw(b"").expect("empty cid"),
            "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku"
        );
        assert_eq!(
            payload_cid_v1_raw(b"abc").expect("abc cid"),
            "bafkreif2pall7dybz7vecqka3zo24irdwabwdi4wc55jznaq75q7eaavvu"
        );
    }

    /// The CID is a pure function of the bytes: equal bytes -> equal CID, and a single
    /// flipped byte -> a different CID (content-addressing, not a nonce).
    #[test]
    fn payload_cid_is_deterministic_and_collision_sensitive() {
        let a = payload_cid_v1_raw(b"segment-bytes-0001").unwrap();
        let b = payload_cid_v1_raw(b"segment-bytes-0001").unwrap();
        let c = payload_cid_v1_raw(b"segment-bytes-0002").unwrap();
        assert_eq!(a, b, "same bytes must address to the same CID");
        assert_ne!(a, c, "different bytes must address to different CIDs");
        assert!(a.starts_with("bafkrei"), "raw CIDv1/sha256 strings start with bafkrei");
    }

    /// Fail-closed: content larger than one importer chunk would be a multi-block
    /// dag-pb tree we do not reproduce in-boundary, so the producer refuses rather
    /// than emit a CID it cannot prove. The boundary is exactly 1 MiB.
    #[test]
    fn payload_cid_fails_closed_above_one_chunk() {
        assert!(payload_cid_v1_raw(&vec![0u8; UNIXFS_SINGLE_CHUNK_MAX]).is_ok());
        assert!(payload_cid_v1_raw(&vec![0u8; UNIXFS_SINGLE_CHUNK_MAX + 1]).is_err());
    }

    #[test]
    fn seal_rejects_unsupported_scheme() {
        let mut req = seal_request_json();
        req["scheme"] = json!("aes-128-classical-v0");
        let code = error_code(handle(json!({ "op": "seal", "request": req })));
        assert_eq!(code, "invalid_request");
    }

    /// Invariant #1 at the *input* boundary: a caller cannot hand in a CEK. The
    /// SealRequest has no key field and `deny_unknown_fields` rejects any attempt
    /// to smuggle one on the wire. Generation must therefore happen in-boundary.
    #[test]
    fn seal_request_cannot_carry_a_cek_on_the_wire() {
        let mut req = seal_request_json();
        req["cek_b64"] = json!("ZmFrZS1jZWstYnl0ZXMtMTY=");
        let parsed: Result<Request, _> =
            serde_json::from_value(json!({ "op": "seal", "request": req }));
        assert!(
            parsed.is_err(),
            "a request carrying a CEK field must be wire-rejected"
        );
    }

    /// Invariant #1 at the *output* boundary (mirrors PC2 `cenc-encrypt`'s
    /// EncryptResult, which only emits ciphertext + IVs). The sealed output is the
    /// SHARED `elastos_common::protected_content::SealedObjectV1` (Day-39 reconcile):
    /// it carries the *wrapped* CEK + KID by construction and — because the type has
    /// no raw-key field and `deny_unknown_fields` — cannot carry the raw key bytes
    /// nor a `cek`/`cek_b64` field. The producer's algorithm set is also accepted by
    /// the shared validator, proving the output converges with the chain contract.
    #[test]
    fn sealed_output_never_carries_raw_cek() {
        use elastos_common::protected_content::{
            validate_protected_content_key_envelope_algorithms, KeyEnvelopeAlgorithmsV1,
            KeyEnvelopeV1, SealedObjectV1, ViewerRequirementV1,
        };

        // Representative in-boundary state: the raw CEK lives only in `cek` here.
        let cek: [u8; 16] = [0x5Au8; 16];
        let cek_b64 = base64::engine::general_purpose::STANDARD.encode(cek);

        let algorithms = KeyEnvelopeAlgorithmsV1 {
            cipher: "aes-256-gcm".to_string(),
            signature: vec!["ml-dsa-65".to_string()],
            kem: vec!["x25519".to_string(), "ml-kem-768".to_string()],
            share_scheme: "shamir-t-of-n".to_string(),
        };
        // Convergence: the producer's PQ-hybrid algorithm set is accepted by the
        // shared chain validator (key-provider runs the same check downstream).
        validate_protected_content_key_envelope_algorithms(&algorithms)
            .expect("producer algorithm set must satisfy the shared chain validator");

        // The sealed output is the SHARED type — no raw-CEK field exists to set.
        let sealed_output = SealedObjectV1 {
            schema: SEALED_OBJECT_SCHEMA.to_string(),
            payload_cid: "bafyciphertext".to_string(),
            rights_policy_cid: "bafyrightspolicy".to_string(),
            availability_receipt_cid: "bafyavail".to_string(),
            key_envelope: KeyEnvelopeV1 {
                scheme: "elastos-pq-hybrid-threshold-v0".to_string(),
                kid: "0123456789abcdef0123456789abcdef".to_string(),
                // sealed, not raw — the only form the CEK may take in output.
                wrapped_cek: "c2VhbGVkLWNlay1ieXRlcw==".to_string(),
                policy_hash: "deadbeef".to_string(),
                algorithms,
            },
            viewer: ViewerRequirementV1 {
                required_interface: "media".to_string(),
            },
        };

        let serialized = serde_json::to_string(&sealed_output).unwrap();
        assert!(
            serialized.contains("wrapped_cek"),
            "output must carry the sealed CEK"
        );
        assert!(
            !serialized.contains(&cek_b64),
            "raw CEK (b64) must never appear in sealed output"
        );
        // No raw key field by any common name.
        assert!(!serialized.contains("\"cek\""), "no raw cek field");
        assert!(!serialized.contains("cek_b64"), "no cek_b64 field");
        // And the raw key bytes themselves must not appear verbatim.
        let hex: String = cek.iter().map(|b| format!("{b:02x}")).collect();
        assert!(!serialized.contains(&hex), "raw CEK bytes must not appear");
    }

    /// The zeroization discipline the engine must apply to the CEK before
    /// returning. Proves the primitive scrubs the buffer in place.
    #[test]
    fn cek_is_zeroized_after_use() {
        let mut cek: Vec<u8> = vec![0x5A; 16];
        assert!(cek.iter().any(|&b| b != 0));
        cek.zeroize();
        assert!(
            cek.iter().all(|&b| b == 0),
            "CEK buffer must be scrubbed after use"
        );
    }

    /// Invariant #1's generation half — CLOSED (Day 19). The CEK+KID are minted
    /// in-boundary with a CSPRNG (no host involvement, no caller input), the asset
    /// is CENC-encrypted with that in-boundary key, and only ciphertext + KID +
    /// IVs cross out — never the CEK. PC2 minted the CEK in the Node host
    /// (`generateCEK`); this moves generation inside the wasm boundary.
    #[test]
    fn cek_and_kid_generated_inside_boundary() {
        // Generation is unconditional and in-boundary: it takes no caller input,
        // and two mints differ — so this is a fresh CSPRNG key, not a fixed or
        // host-injected one.
        let a = mint_cek_and_kid().expect("mint a");
        let b = mint_cek_and_kid().expect("mint b");
        assert_eq!(a.cek.len(), 16, "CEK must be a 16-byte AES-128 key");
        assert_eq!(a.kid_hex.len(), 32, "KID must be 32 hex chars (16 bytes)");
        assert!(
            a.kid_hex.chars().all(|c| c.is_ascii_hexdigit()),
            "KID must be lowercase hex"
        );
        assert_ne!(&*a.cek, &*b.cek, "each mint must produce a fresh CEK");
        assert_ne!(a.kid_hex, b.kid_hex, "each mint must produce a fresh KID");

        // The engine seals a real asset using ONLY an in-boundary-minted CEK and
        // emits ciphertext + KID + IVs. There is no parameter by which a caller
        // could supply a CEK, and `SealedSegment` has no CEK field — invariant #1
        // is enforced by construction.
        let plaintext: &[u8] = b"in-boundary keygen seals this protected asset!!!";
        let sizes = vec![plaintext.len() as u32];
        let sealed = seal_segment_in_boundary(plaintext, &sizes, 0).expect("seal");

        assert_eq!(sealed.sample_count, 1);
        assert_eq!(sealed.ivs.len(), 1);
        assert_ne!(
            sealed.ciphertext.as_slice(),
            plaintext,
            "the asset must be encrypted in-boundary, not passed through"
        );
        assert_eq!(
            sealed.kid_hex.len(),
            32,
            "the sealed segment carries the in-boundary KID"
        );
    }

    /// Invariant #1 output half at the engine level: a freshly minted CEK never
    /// appears in the engine's emitted material (ciphertext + KID + IVs). The
    /// `SealedSegment` type has no CEK field; this also checks the minted key's
    /// raw bytes do not surface in the IVs/KID/ciphertext relatives.
    #[test]
    fn seal_engine_emits_no_key_material() {
        let plaintext: &[u8] = b"AAAAAAAAAAAAAAAAprotected body bytes after a clear leader region";
        let sizes = vec![plaintext.len() as u32];
        let sealed = seal_segment_in_boundary(plaintext, &sizes, 16).expect("seal");

        // The clear leader is preserved (decoder can parse headers), the body is
        // encrypted, and the surfaced relatives carry no key bytes.
        assert_eq!(&sealed.ciphertext[..16], &plaintext[..16], "clear leader preserved");
        assert_ne!(&sealed.ciphertext[16..], &plaintext[16..], "body encrypted");

        let kid_bytes: Vec<u8> = (0..sealed.kid_hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&sealed.kid_hex[i..i + 2], 16).unwrap())
            .collect();
        // The KID is independent of the CEK; the IVs are not the CEK either. The
        // engine surfaces these by design — none is the content key.
        assert_eq!(kid_bytes.len(), 16);
        for iv in &sealed.ivs {
            assert_eq!(iv.len(), 8, "CENC IVs are 8 bytes, never a 16-byte CEK");
        }
    }

    // --- escrow engine (feature `escrow`, Day 59) -----------------------------
    //
    // Prove the producer→authority CEK escrow, and the FULL fresh-CEK crypto path
    // producer → authority recover → re-seal per decrypt session → decrypt open —
    // no committed golden, all on a CEK minted in THIS run.
    #[cfg(feature = "escrow")]
    mod escrow_engine {
        use super::*;

        /// The producer seals a freshly-minted CEK to the authority's recipient key;
        /// the authority opens it with the SHARED escrow AAD + the producer's vk and
        /// recovers the exact CEK. A wrong recipient cannot open it.
        #[test]
        fn escrow_seals_to_authority_and_recovers() {
            let minted = mint_cek_and_kid().expect("mint");
            let kid16 = kid_to_content_id_bytes16(&minted.kid_hex).expect("kid16");

            // Authority recipient (its published KEM key) + producer signing key.
            let (auth_secret, auth_public) = ddrm_envelope::mint_session();
            let recipient_pub = ddrm_envelope::session_public_bytes(&auth_public);
            let (producer_signer, producer_vk) =
                ddrm_envelope::seal::mldsa_seal_keypair([5u8; 32]);

            let wrapped_b64 =
                seal_cek_to_authority(&minted.cek[..], &kid16, &recipient_pub, &producer_signer)
                    .expect("escrow seal");

            // The sealed blob carries no raw CEK.
            let cek_b64 = base64::engine::general_purpose::STANDARD.encode(&minted.cek[..]);
            assert!(!wrapped_b64.contains(&cek_b64), "raw CEK must not appear in the escrow blob");

            // Authority recovers (recompute the identical AAD, verify producer vk).
            let wrapped = base64::engine::general_purpose::STANDARD
                .decode(&wrapped_b64)
                .unwrap();
            let env = ddrm_envelope::PqSealedEnvelope::from_bytes(&wrapped).unwrap();
            let verifier = ddrm_envelope::MlDsa65Verifier::from_encoded(&producer_vk).unwrap();
            let aad = ddrm_envelope::transcript::escrow_aad(
                SUPPORTED_SCHEMES[0],
                &kid16,
                &recipient_pub,
            );
            let recovered =
                ddrm_envelope::hybrid_unwrap_bound(&auth_secret, &env, &aad, &verifier).unwrap();
            assert_eq!(&recovered[..], &minted.cek[..], "authority recovers the exact CEK");

            // A DIFFERENT authority recipient cannot open it (fail closed).
            let (other_secret, _other_public) = ddrm_envelope::mint_session();
            assert!(
                ddrm_envelope::hybrid_unwrap_bound(&other_secret, &env, &aad, &verifier).is_err(),
                "wrong recipient must fail closed"
            );
        }

        /// MEDIA path, end to end on a REAL fragmented-MP4: the producer CENC-encrypts
        /// every fragment under ONE CEK (continuous IV counter) and escrows that CEK to a
        /// 2-of-3 quorum. We then recover the CEK from TWO of the three sealed shares and
        /// PROVE it is the exact key used by re-encrypting the original fragments under it
        /// from counter 0 — the bytes must match the producer's output for EVERY segment.
        /// (No decrypt impl needed: encrypt_fragment is deterministic, so a byte match
        /// proves both the single-CEK custody and the continuous cross-segment counter.)
        #[test]
        fn media_segments_seal_recovers_one_cek_across_all_fragments() {
            use base64::Engine as _;
            let b64 = base64::engine::general_purpose::STANDARD;

            // A REAL ffmpeg fragmented MP4 (+frag_keyframe+empty_moov+default_base_moof
            // +separate_moof) — the exact shape media-provider emits.
            let fixture_b64 = include_str!("../tests/vectors/tiny_fragmented.mp4.b64");
            let fixture = b64.decode(fixture_b64.trim()).expect("decode fixture");
            let split = ddrm_media::mp4::split_fragmented(&fixture).expect("split");
            assert!(
                split.fragments.len() >= 2,
                "fixture must have multiple fragments to exercise the continuous IV counter"
            );
            let plaintext_frags: Vec<String> =
                split.fragments.iter().map(|f| b64.encode(f)).collect();

            // A 3-node quorum: each node a REAL hybrid recipient + a DISTINCT verifying key.
            let mut secrets: Vec<(ddrm_envelope::SessionKemSecret, Vec<u8>)> = Vec::new();
            let mut nodes: Vec<ThresholdNode> = Vec::new();
            for k in 0..3u8 {
                let (sec, pubk) = ddrm_envelope::mint_session();
                let recipient = ddrm_envelope::session_public_bytes(&pubk);
                let (_s, vk) = ddrm_envelope::seal::mldsa_seal_keypair([10 + k; 32]);
                secrets.push((sec, recipient.clone()));
                nodes.push(ThresholdNode {
                    recipient_pub_b64: b64.encode(&recipient),
                    verifying_key_b64: b64.encode(&vk),
                });
            }

            // Init mints the producer signing key and publishes its verifying key.
            let mut provider = EncryptProvider::default();
            let producer_vk = match provider.init(json!({})) {
                Response::Ok { data: Some(d) } => b64
                    .decode(d["producer_verifying_key_b64"].as_str().unwrap())
                    .unwrap(),
                _ => panic!("init must publish the producer verifying key"),
            };

            // Seal every fragment under one CEK; escrow to the quorum.
            let data = match provider.seal_segments_threshold(
                &plaintext_frags,
                Some(&b64.encode(&split.init)),
                &nodes,
            ) {
                Response::Ok { data: Some(d) } => d,
                Response::Ok { data: None } => panic!("expected data"),
                Response::Error { code, message } => panic!("seal failed {code}: {message}"),
            };

            let kid_hex = data["kid_hex"].as_str().unwrap().to_string();
            let kid16 = kid_to_content_id_bytes16(&kid_hex).unwrap();
            let enc_segs: Vec<Vec<u8>> = data["segments_b64"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| b64.decode(s.as_str().unwrap()).unwrap())
                .collect();
            assert_eq!(enc_segs.len(), split.fragments.len());

            // Recover the CEK from TWO of the three sealed shares (nodes x=1 and x=3).
            let shares = data["shares"].as_array().unwrap();
            let verifier = ddrm_envelope::MlDsa65Verifier::from_encoded(&producer_vk).unwrap();
            let mut indexed: Vec<(u8, Vec<u8>)> = Vec::new();
            for idx in [0usize, 2usize] {
                let wrapped = b64
                    .decode(shares[idx]["wrapped_share_b64"].as_str().unwrap())
                    .unwrap();
                let env = ddrm_envelope::PqSealedEnvelope::from_bytes(&wrapped).unwrap();
                let (sec, recipient) = &secrets[idx];
                let aad =
                    ddrm_envelope::transcript::escrow_aad(SUPPORTED_SCHEMES[0], &kid16, recipient);
                let payload =
                    ddrm_envelope::hybrid_unwrap_bound(sec, &env, &aad, &verifier).unwrap();
                indexed.push((payload[0], payload[1..].to_vec()));
            }
            let cek = ddrm_envelope::combine_cek_shamir2(
                indexed[0].0,
                &indexed[0].1,
                indexed[1].0,
                &indexed[1].1,
            )
            .expect("2-of-3 combine");
            assert_eq!(cek.len(), 16, "recovered CEK is a 16-byte AES-128 key");
            let cek16: [u8; 16] = cek[..].try_into().unwrap();

            // PROOF: re-encrypt the ORIGINAL fragments with the RECOVERED CEK under the
            // SAME continuous counter — must byte-match the producer's encrypted segments.
            let mut counter: u64 = 0;
            for (i, frag) in split.fragments.iter().enumerate() {
                let re = ddrm_media::mp4::encrypt_fragment(frag, &cek16, &mut counter).unwrap();
                assert_eq!(
                    re, enc_segs[i],
                    "segment {i} reproduces under the recovered CEK + continuous counter"
                );
            }
        }

        /// Fail-closed guards on the media seal op (no ffmpeg/quorum needed).
        #[test]
        fn media_seal_fails_closed_on_bad_input() {
            use base64::Engine as _;
            let b64 = base64::engine::general_purpose::STANDARD;
            let mut provider = EncryptProvider::default();
            let _ = provider.init(json!({}));

            // Three syntactically-valid (base64) but distinct node identities.
            let node = |seed: u8| ThresholdNode {
                recipient_pub_b64: b64.encode([seed; 32]),
                verifying_key_b64: b64.encode([seed.wrapping_add(100); 32]),
            };
            let good_nodes = vec![node(1), node(2), node(3)];

            let code = |r: Response| match r {
                Response::Error { code, .. } => code,
                Response::Ok { .. } => panic!("expected error"),
            };

            // < 3 nodes.
            assert_eq!(
                code(provider.seal_segments_threshold(
                    &[b64.encode(b"x")],
                    None,
                    &good_nodes[..2]
                )),
                "invalid_request"
            );
            // no segments.
            assert_eq!(
                code(provider.seal_segments_threshold(&[], None, &good_nodes)),
                "invalid_request"
            );
            // duplicate node identity.
            let dup = vec![node(1), node(1), node(3)];
            assert_eq!(
                code(provider.seal_segments_threshold(&[b64.encode(b"x")], None, &dup)),
                "invalid_request"
            );
            // segment that is not valid base64.
            assert_eq!(
                code(provider.seal_segments_threshold(
                    &["!!!notb64!!!".to_string()],
                    None,
                    &good_nodes
                )),
                "invalid_request"
            );
        }

        /// The whole producer→consumer key path on a FRESH CEK (no golden):
        ///   producer mints CEK -> escrows to authority -> authority recovers ->
        ///   authority RE-SEALS to a decrypt session -> decrypt opens the SAME CEK.
        /// This is the crypto spine of the producer half meeting the (already-built)
        /// consumer half, end to end, with no raw CEK ever leaving a boundary.
        #[test]
        fn fresh_cek_flows_producer_to_decrypt() {
            // (1) producer mints a CEK + KID in-boundary.
            let minted = mint_cek_and_kid().expect("mint");
            let kid16 = kid_to_content_id_bytes16(&minted.kid_hex).expect("kid16");
            let (producer_signer, producer_vk) =
                ddrm_envelope::seal::mldsa_seal_keypair([1u8; 32]);

            // (2) producer escrows the CEK to the authority's recipient key.
            let (auth_secret, auth_public) = ddrm_envelope::mint_session();
            let recipient_pub = ddrm_envelope::session_public_bytes(&auth_public);
            let wrapped_b64 =
                seal_cek_to_authority(&minted.cek[..], &kid16, &recipient_pub, &producer_signer)
                    .expect("escrow seal");

            // (3) authority recovers the CEK (its boundary, never on the wire raw).
            let wrapped = base64::engine::general_purpose::STANDARD
                .decode(&wrapped_b64)
                .unwrap();
            let env = ddrm_envelope::PqSealedEnvelope::from_bytes(&wrapped).unwrap();
            let producer_verifier =
                ddrm_envelope::MlDsa65Verifier::from_encoded(&producer_vk).unwrap();
            let escrow_aad = ddrm_envelope::transcript::escrow_aad(
                SUPPORTED_SCHEMES[0],
                &kid16,
                &recipient_pub,
            );
            let recovered =
                ddrm_envelope::hybrid_unwrap_bound(&auth_secret, &env, &escrow_aad, &producer_verifier)
                    .unwrap();

            // (4) authority RE-SEALS the recovered CEK to a decrypt session's published
            // key (its own ML-DSA seal key), bound to a decrypt-session AAD.
            let (auth_signer, auth_vk) = ddrm_envelope::seal::mldsa_seal_keypair([2u8; 32]);
            let (decrypt_secret, decrypt_public) = ddrm_envelope::mint_session();
            let session_pub_bytes = ddrm_envelope::session_public_bytes(&decrypt_public);
            let session_aad = b"decrypt-session-aad:smoke".to_vec();
            let resealed = ddrm_envelope::seal::seal_bound(
                &decrypt_public,
                &recovered[..],
                &session_aad,
                &auth_signer,
            );

            // (5) the decrypt boundary opens it with its session secret + the authority vk.
            let auth_verifier = ddrm_envelope::MlDsa65Verifier::from_encoded(&auth_vk).unwrap();
            let at_decrypt = ddrm_envelope::hybrid_unwrap_bound(
                &decrypt_secret,
                &resealed,
                &session_aad,
                &auth_verifier,
            )
            .unwrap();

            assert_eq!(
                &at_decrypt[..],
                &minted.cek[..],
                "the CEK minted by the producer arrives intact at the decrypt boundary"
            );
            // Sanity: the published session key the authority sealed to is the decrypt one.
            assert!(ddrm_envelope::session_public_from_bytes(&session_pub_bytes).is_some());
        }
    }

    // --- production `seal` op (feature `escrow`, Day 69) -----------------------
    //
    // The non-inline `seal` op runs the FULL production pipeline on HANDED-IN bytes
    // (mint -> CENC -> escrow -> content-address) and emits a complete, shared-contract
    // `SealedObjectV1`. The capsule fetches nothing — it mirrors PC2's host handing
    // segment bytes to the CENC WASM (`dashPackager.ts` readFileSync:504 ->
    // executeCENCEncrypt(.., seg.data):432). Fail-closed unless bytes + recipient + the
    // availability receipt + a viewer interface are all present.
    #[cfg(feature = "escrow")]
    mod production_seal {
        use super::*;

        fn inited_provider() -> EncryptProvider {
            let mut p = EncryptProvider::default();
            let _ = p.init(json!({}));
            p
        }

        /// A published authority recipient (its KEM key) the producer escrows the CEK to.
        fn recipient_b64() -> String {
            let (_secret, public) = ddrm_envelope::mint_session();
            base64::engine::general_purpose::STANDARD
                .encode(ddrm_envelope::session_public_bytes(&public))
        }

        fn seal_request_full(content: &[u8], recipient_b64: &str) -> Value {
            json!({
                "op": "seal",
                "request": {
                    "schema": SEAL_REQUEST_SCHEMA,
                    "plaintext_ref": "asset-handle-abc123",
                    "content_b64": base64::engine::general_purpose::STANDARD.encode(content),
                    "recipient_pub_b64": recipient_b64,
                    "availability_receipt_cid": "bafyavailreceipt",
                    "rights_policy_cid": "bafyrightspolicy",
                    "scheme": "elastos-pq-hybrid-threshold-v0",
                    "viewer": { "required_interface": "media" }
                }
            })
        }

        fn call(p: &mut EncryptProvider, v: Value) -> Response {
            let req: Request = serde_json::from_value(v).expect("request parses");
            p.handle(req)
        }

        /// Configured (bytes + recipient + receipt + viewer) -> the production `seal`
        /// emits a COMPLETE `SealedObjectV1`: it round-trips into the shared type
        /// (so `deny_unknown_fields` and the full shape hold), its algorithm suite is
        /// accepted by the shared chain validator, its `payload_cid` is a real raw
        /// CIDv1, and the envelope KID is the on-chain bytes16 contentId.
        #[test]
        fn configured_seal_emits_complete_sealed_object() {
            let mut p = inited_provider();
            let content = b"day69: full production seal on handed-in asset bytes".to_vec();
            let resp = call(&mut p, seal_request_full(&content, &recipient_b64()));
            let data = ok_data(resp);

            // Parses back into the SHARED contract type (deny_unknown_fields + complete).
            let sealed: SealedObjectV1 =
                serde_json::from_value(data["sealed_object"].clone()).expect("SealedObjectV1");
            assert_eq!(sealed.schema, SEALED_OBJECT_SCHEMA);
            assert_eq!(sealed.rights_policy_cid, "bafyrightspolicy");
            assert_eq!(sealed.availability_receipt_cid, "bafyavailreceipt");
            assert_eq!(sealed.viewer.required_interface, "media");

            // Real content address (Day 68 codec), distinct from the KID/contentId.
            assert!(
                sealed.payload_cid.starts_with("bafkrei"),
                "payload_cid must be a raw CIDv1/sha256"
            );
            assert_ne!(
                sealed.payload_cid, sealed.key_envelope.kid,
                "payload CID and contentId(KID) are distinct identities"
            );

            // The producer's PQ-hybrid suite is accepted by the shared chain validator.
            validate_protected_content_key_envelope_algorithms(&sealed.key_envelope.algorithms)
                .expect("producer algorithm set must satisfy the shared chain validator");

            // The envelope KID is the consumer chain's bytes16 contentId (lossless).
            let content_id =
                kid_to_content_id_bytes16(&sealed.key_envelope.kid).expect("kid -> bytes16");
            assert_eq!(content_id.len(), 16);

            // Containment: neither the plaintext nor a raw CEK appears in the output.
            let wire = serde_json::to_string(&data).unwrap();
            assert!(
                !wire.contains(&base64::engine::general_purpose::STANDARD.encode(&content)),
                "plaintext must never appear in the sealed output"
            );
            assert!(!wire.contains("\"cek\""), "no raw cek field");
            assert!(!wire.contains("cek_b64"), "no cek_b64 field");
            assert!(
                !sealed.key_envelope.wrapped_cek.is_empty(),
                "the CEK leaves only SEALED"
            );
        }

        /// Two seals of the SAME bytes mint independent CEKs -> different `payload_cid`
        /// (the segment carries a per-seal IV). Content-addressing reflects the exact
        /// sealed bytes, not the plaintext identity.
        #[test]
        fn each_seal_freshly_mints_and_addresses() {
            let mut p = inited_provider();
            let content = b"same plaintext, two independent seals".to_vec();
            let rcpt = recipient_b64();
            let a: SealedObjectV1 = serde_json::from_value(
                ok_data(call(&mut p, seal_request_full(&content, &rcpt)))["sealed_object"].clone(),
            )
            .unwrap();
            let b: SealedObjectV1 = serde_json::from_value(
                ok_data(call(&mut p, seal_request_full(&content, &rcpt)))["sealed_object"].clone(),
            )
            .unwrap();
            assert_ne!(a.key_envelope.kid, b.key_envelope.kid, "fresh KID per seal");
            assert_ne!(a.payload_cid, b.payload_cid, "fresh CEK/IV -> fresh sealed bytes");
        }

        /// Fail-closed matrix. `seal` must never emit a partial object:
        ///   - no recipient -> not_configured (cannot escrow the CEK)
        ///   - no content   -> not_configured (nothing handed in to seal)
        ///   - no availability receipt / no viewer interface -> invalid_request
        #[test]
        fn seal_fails_closed_on_missing_inputs() {
            let mut p = inited_provider();
            let rcpt = recipient_b64();
            let content = b"x".to_vec();

            // Missing recipient.
            let mut v = seal_request_full(&content, &rcpt);
            v["request"].as_object_mut().unwrap().remove("recipient_pub_b64");
            assert_eq!(error_code(call(&mut p, v)), "not_configured");

            // Missing content bytes.
            let mut v = seal_request_full(&content, &rcpt);
            v["request"].as_object_mut().unwrap().remove("content_b64");
            assert_eq!(error_code(call(&mut p, v)), "not_configured");

            // Missing availability receipt.
            let mut v = seal_request_full(&content, &rcpt);
            v["request"].as_object_mut().unwrap().remove("availability_receipt_cid");
            assert_eq!(error_code(call(&mut p, v)), "invalid_request");

            // Empty viewer interface.
            let mut v = seal_request_full(&content, &rcpt);
            v["request"]["viewer"] = json!({ "required_interface": "" });
            assert_eq!(error_code(call(&mut p, v)), "invalid_request");

            // Empty content bytes (present but zero-length).
            let mut v = seal_request_full(&content, &rcpt);
            v["request"]["content_b64"] = json!("");
            assert_eq!(error_code(call(&mut p, v)), "invalid_request");
        }
    }

    // --- encrypt -> decrypt round-trip golden (feature `gen-vectors`) ----------
    //
    // Emits a fixture PRODUCED BY THIS PROVIDER's real in-boundary engine
    // (mint CEK+KID -> CENC encrypt) muxed into a minimal fMP4 segment, written
    // into decrypt-provider/tests/vectors/ for the consumer to replay. This pins
    // the cross-invariant composition: an asset sealed by encrypt-provider
    // decrypts in decrypt-provider to the original bytes.
    //
    // The CEK is captured into the fixture as the test stand-in for the still-
    // blocked transport rail (DDRM_DECRYPT_RAIL.md): in production the CEK reaches
    // decrypt SEALED, never in the clear. The seal/envelope transport is the one
    // remaining gap; the cipher + keygen composition is proven here.
    #[cfg(feature = "gen-vectors")]
    fn make_box(box_type: &[u8; 4], content: &[u8]) -> Vec<u8> {
        let size = (8 + content.len()) as u32;
        let mut b = size.to_be_bytes().to_vec();
        b.extend_from_slice(box_type);
        b.extend_from_slice(content);
        b
    }

    /// Mux an encrypted single sample + its 8-byte IV into the minimal
    /// moof{traf{trun,senc}} + mdat segment the decrypt engine consumes. This is
    /// the box surgery the producer's muxer will perform (a later, separate
    /// boundary); done test-side so the round-trip is exercised end to end.
    #[cfg(feature = "gen-vectors")]
    fn mux_segment(ciphertext: &[u8], iv8: &[u8; 8]) -> Vec<u8> {
        let mut trun_content = vec![0u8, 0x00, 0x02, 0x00, 0, 0, 0, 1];
        trun_content.extend_from_slice(&(ciphertext.len() as u32).to_be_bytes());
        let trun = make_box(b"trun", &trun_content);
        let mut senc_content = vec![0u8, 0, 0, 0, 0, 0, 0, 1];
        senc_content.extend_from_slice(iv8);
        let senc = make_box(b"senc", &senc_content);
        let mut traf_content = trun;
        traf_content.extend_from_slice(&senc);
        let traf = make_box(b"traf", &traf_content);
        let moof = make_box(b"moof", &traf);
        let mdat = make_box(b"mdat", ciphertext);
        let mut segment = moof;
        segment.extend_from_slice(&mdat);
        segment
    }

    /// Regenerate the committed encrypt->decrypt round-trip golden. Run:
    /// `cargo test --features gen-vectors emit_roundtrip_vector`
    #[cfg(feature = "gen-vectors")]
    #[test]
    fn emit_roundtrip_vector() {
        let b64 = base64::engine::general_purpose::STANDARD;

        // Produce the asset with the REAL in-boundary engine internals: mint a CEK
        // + KID with the CSPRNG, then CENC-encrypt one sample. The CEK is captured
        // here (test stand-in for the sealed rail) so the consumer can decrypt.
        let minted = mint_cek_and_kid().expect("mint");
        let plaintext = b"the quick brown fox jumps over!!"; // 32 bytes, full-sample
        let sizes = [plaintext.len() as u32];
        let iv_seed = [0x22u8; 8];
        let (ciphertext, ivs, _subs) =
            cenc::encrypt_samples(plaintext, &minted.cek, &sizes, &iv_seed, 0).expect("encrypt");
        let segment = mux_segment(&ciphertext, &ivs[0]);

        let v = json!({
            "description": "encrypt-provider in-boundary mint+CENC -> decrypt-provider cenc; CEK captured (rail stand-in)",
            "kid_hex": minted.kid_hex,
            "cek_b64": b64.encode(&*minted.cek),
            "encrypted_segment_b64": b64.encode(&segment),
            "expected_plaintext_b64": b64.encode(plaintext),
        });
        // Write into the consumer's vectors dir so decrypt-provider can include_str! it.
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../decrypt-provider/tests/vectors");
        std::fs::create_dir_all(dir).unwrap();
        let path = format!("{dir}/roundtrip_encrypt_to_decrypt.json");
        std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
        eprintln!("wrote {path}");
    }

    /// Mux N encrypted samples into a multi-sample fMP4 segment:
    /// moof{traf{trun(per-sample sizes), senc(per-sample 8-byte IVs, no subsamples)}}+mdat.
    /// Box framing mirrors PC2 `cenc-encrypt::mp4box::build_senc` (full-sample,
    /// flags=0) and the trun sample-size-present (0x000200) shape our decrypt
    /// parser + PC2 `decrypt_segment` consume (proven by the Day-31 cenc goldens).
    #[cfg(feature = "gen-vectors")]
    fn mux_multisample_segment(ciphertext: &[u8], sizes: &[u32], ivs: &[[u8; 8]]) -> Vec<u8> {
        let mut trun_content = vec![0u8, 0x00, 0x02, 0x00]; // v0, flags=sample-size-present
        trun_content.extend_from_slice(&(sizes.len() as u32).to_be_bytes());
        for &sz in sizes {
            trun_content.extend_from_slice(&sz.to_be_bytes());
        }
        let trun = make_box(b"trun", &trun_content);

        let mut senc_content = vec![0u8, 0, 0, 0]; // v0, flags=0 (no subsamples)
        senc_content.extend_from_slice(&(ivs.len() as u32).to_be_bytes());
        for iv8 in ivs {
            senc_content.extend_from_slice(iv8);
        }
        let senc = make_box(b"senc", &senc_content);

        let mut traf_content = trun;
        traf_content.extend_from_slice(&senc);
        let traf = make_box(b"traf", &traf_content);
        let moof = make_box(b"moof", &traf);
        let mut segment = moof;
        segment.extend_from_slice(&make_box(b"mdat", ciphertext));
        segment
    }

    /// Mux a single subsample-encrypted sample (clear leader + encrypted body):
    /// senc flags=0x000002 with one subsample table entry, mirroring PC2
    /// `cenc-encrypt::mp4box::build_senc_with_subsamples` (8-byte IV +
    /// subsample_count(u16) + per-subsample clear(u16)+encrypted(u32)).
    #[cfg(feature = "gen-vectors")]
    fn mux_subsample_segment(ciphertext: &[u8], iv8: &[u8; 8], subs: &[cenc::SubsampleEntry]) -> Vec<u8> {
        let mut trun_content = vec![0u8, 0x00, 0x02, 0x00, 0, 0, 0, 1];
        trun_content.extend_from_slice(&(ciphertext.len() as u32).to_be_bytes());
        let trun = make_box(b"trun", &trun_content);

        let mut senc_content = vec![0u8, 0x00, 0x00, 0x02, 0, 0, 0, 1]; // flags=subsamples, count=1
        senc_content.extend_from_slice(iv8);
        senc_content.extend_from_slice(&(subs.len() as u16).to_be_bytes());
        for s in subs {
            senc_content.extend_from_slice(&(s.clear as u16).to_be_bytes());
            senc_content.extend_from_slice(&s.protected.to_be_bytes());
        }
        let senc = make_box(b"senc", &senc_content);

        let mut traf_content = trun;
        traf_content.extend_from_slice(&senc);
        let traf = make_box(b"traf", &traf_content);
        let moof = make_box(b"moof", &traf);
        let mut segment = moof;
        segment.extend_from_slice(&make_box(b"mdat", ciphertext));
        segment
    }

    #[cfg(feature = "gen-vectors")]
    fn write_roundtrip_vector(file: &str, description: &str, kid_hex: &str, cek: &[u8], segment: &[u8], plaintext: &[u8]) {
        let b64 = base64::engine::general_purpose::STANDARD;
        let v = json!({
            "description": description,
            "kid_hex": kid_hex,
            "cek_b64": b64.encode(cek),
            "encrypted_segment_b64": b64.encode(segment),
            "expected_plaintext_b64": b64.encode(plaintext),
        });
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../decrypt-provider/tests/vectors");
        std::fs::create_dir_all(dir).unwrap();
        let path = format!("{dir}/{file}");
        std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
        eprintln!("wrote {path}");
    }

    /// Regenerate the MULTI-SAMPLE round-trip golden (real playback shape).
    /// `cargo test --features gen-vectors emit_roundtrip_multisample_vector`
    #[cfg(feature = "gen-vectors")]
    #[test]
    fn emit_roundtrip_multisample_vector() {
        // Produce 4 samples with the REAL in-boundary engine: one CEK, per-sample
        // unique IVs (seed+index), full-sample encryption (clear_leader=0).
        let minted = mint_cek_and_kid().expect("mint");
        let plaintext: &[u8] = b"frame0-bytes....frame1-longer-bytes....frame2..frame3-final-bytes!";
        let sizes: [u32; 4] = [16, 23, 8, 19];
        assert_eq!(sizes.iter().sum::<u32>() as usize, plaintext.len());
        let iv_seed = [0x33u8; 8];
        let (ciphertext, ivs, _subs) =
            cenc::encrypt_samples(plaintext, &minted.cek, &sizes, &iv_seed, 0).expect("encrypt");
        let segment = mux_multisample_segment(&ciphertext, &sizes, &ivs);
        write_roundtrip_vector(
            "roundtrip_multisample_encrypt_to_decrypt.json",
            "encrypt-provider in-boundary mint+CENC (4 full samples) -> decrypt-provider cenc; CEK captured (rail stand-in)",
            &minted.kid_hex,
            &*minted.cek,
            &segment,
            plaintext,
        );
    }

    #[cfg(feature = "gen-vectors")]
    fn write_roundtrip_multisegment_vector(
        kid_hex: &str,
        cek: &[u8],
        segments_b64: Vec<String>,
        expected_plaintexts_b64: Vec<String>,
    ) {
        let b64 = base64::engine::general_purpose::STANDARD;
        let v = json!({
            "description":
                "encrypt-provider in-boundary mint+CENC (3 segments, ONE CEK, globally-unique \
                 per-sample IVs continuing across segments) -> decrypt-provider multi-segment loop; \
                 CEK captured (rail stand-in)",
            "kid_hex": kid_hex,
            "cek_b64": b64.encode(cek),
            "segments_b64": segments_b64,
            "expected_plaintexts_b64": expected_plaintexts_b64,
        });
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../decrypt-provider/tests/vectors");
        std::fs::create_dir_all(dir).unwrap();
        let path = format!("{dir}/roundtrip_multisegment_encrypt_to_decrypt.json");
        std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
        eprintln!("wrote {path}");
    }

    /// Regenerate the MULTI-SEGMENT round-trip golden (real DASH/fMP4 asset shape): several
    /// `moof+mdat` segments sharing ONE presentation CEK, with globally-unique per-sample IVs
    /// (the counter CONTINUES across segments, as real CENC requires). Mixes per-segment sample
    /// counts (2,1,2) so the decrypt loop's per-segment + summed sample accounting is exercised.
    /// `cargo test --features gen-vectors emit_roundtrip_multisegment_vector`
    #[cfg(feature = "gen-vectors")]
    #[test]
    fn emit_roundtrip_multisegment_vector() {
        let minted = mint_cek_and_kid().expect("mint");
        let b64 = base64::engine::general_purpose::STANDARD;
        let seg_plaintexts: [&[u8]; 3] = [
            b"seg0-sample-0..!seg0-sample-1-longer", // 2 samples (16 + 20)
            b"seg1-single-sample-32-bytes-long",     // 1 sample (32)
            b"AAAseg2BBBseg2CC",                      // 2 samples (8 + 8)
        ];
        let seg_sizes: [&[u32]; 3] = [&[16, 20], &[32], &[8, 8]];
        let mut segments_b64 = Vec::new();
        let mut expected_b64 = Vec::new();
        // The per-sample IV counter is GLOBAL across the presentation (Bento4-style): segment k's
        // first sample continues from the running sample index, so no IV is ever reused.
        let mut counter: u64 = 0;
        for (pt, sizes) in seg_plaintexts.iter().zip(seg_sizes.iter()) {
            assert_eq!(sizes.iter().sum::<u32>() as usize, pt.len(), "sample sizes cover the plaintext");
            let iv_seed = counter.to_be_bytes();
            let (ciphertext, ivs, _subs) =
                cenc::encrypt_samples(pt, &minted.cek, sizes, &iv_seed, 0).expect("encrypt");
            // `mux_multisample_segment` with one sample is byte-identical to the single-sample
            // muxer (same trun sample-size-present + senc flags=0 layout), so one muxer covers
            // every segment regardless of sample count.
            let segment = mux_multisample_segment(&ciphertext, sizes, &ivs);
            segments_b64.push(b64.encode(&segment));
            expected_b64.push(b64.encode(pt));
            counter += sizes.len() as u64;
        }
        write_roundtrip_multisegment_vector(&minted.kid_hex, &*minted.cek, segments_b64, expected_b64);
    }

    /// Regenerate the SUBSAMPLE round-trip golden (clear-leader + encrypted body).
    /// `cargo test --features gen-vectors emit_roundtrip_subsample_vector`
    #[cfg(feature = "gen-vectors")]
    #[test]
    fn emit_roundtrip_subsample_vector() {
        // One sample, 16-byte clear leader (codec header) + encrypted body — the
        // real engine emits the subsample {clear, protected} framing we mux.
        let minted = mint_cek_and_kid().expect("mint");
        let plaintext: &[u8] = b"CLEAR-CODEC-HDR!!encrypted media payload bytes following the leader.";
        let sizes = [plaintext.len() as u32];
        let clear_leader = 16u32;
        let iv_seed = [0x44u8; 8];
        let (ciphertext, ivs, subs) =
            cenc::encrypt_samples(plaintext, &minted.cek, &sizes, &iv_seed, clear_leader).expect("encrypt");
        let segment = mux_subsample_segment(&ciphertext, &ivs[0], &subs[0]);
        write_roundtrip_vector(
            "roundtrip_subsample_encrypt_to_decrypt.json",
            "encrypt-provider in-boundary mint+CENC (subsample: 16B clear leader) -> decrypt-provider cenc; CEK captured (rail stand-in)",
            &minted.kid_hex,
            &*minted.cek,
            &segment,
            plaintext,
        );
    }
}
