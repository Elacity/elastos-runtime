//! ElastOS Key Provider Capsule
//!
//! Fail-closed protected-content key-release boundary. App capsules never
//! receive raw CEKs, KMS node credentials, chain RPC, wallet RPC, or provider
//! credentials through this provider.

use elastos_common::protected_content::{
    validate_protected_content_key_envelope_algorithms, KeyReleaseRequestV1,
    KEY_RELEASE_REQUEST_SCHEMA, PROTECTED_CONTENT_ACTIONS, RIGHTS_DECISION_RECEIPT_SCHEMA,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};
const SUPPORTED_SCHEMES: &[&str] = &["elastos-pq-hybrid-threshold-v0"];

/// On-disk schema for the durable authority key store (feature `key-authority-ref`). The
/// store holds ONE 32-byte master seed; the stable signer + KEM recipient are deterministically
/// re-derived from it on every launch, so a producer can escrow a CEK to the published recipient
/// at PUBLISH time and any later authority launch resolves the identical recipient.
#[cfg(feature = "key-authority-ref")]
const AUTHORITY_KEYSTORE_SCHEMA: &str = "elastos.key_authority.seed/v1";

/// On-disk schema for the `dkms` EXTERNAL authority descriptor (feature `key-authority-ref`).
/// PUBLIC-ONLY (Day 87–88, v2): the descriptor carries the authority node's PUBLISHED identity
/// (`verifying_key_b64` + `recipient_pub_b64`) and its `authority_endpoint` (where the node lives) —
/// and NOTHING secret. The runtime (this `key-provider` CLIENT) holds only this public identity and
/// DELEGATES recovery to the node; the master key material lives ONLY in the node. A descriptor that
/// carries a master seed (the old v1 shape) is REJECTED — the secret must never reach the runtime.
/// Mirrors PC2 holding only the external authority's PUBLIC `pkpId`/`authority` and RPCing the Lit
/// network for recovery (`recoverCEKEnvelope`, `chipotle-client.ts:1438`), never the PKP secret.
#[cfg(feature = "key-authority-ref")]
const DKMS_AUTHORITY_DESCRIPTOR_SCHEMA: &str = "elastos.dkms.authority/v2";

/// Decrypt-material suite tags the hosted backends emit. These match the
/// `SealedDecryptMaterialV1.suite` values the decrypt boundary already routes on
/// (`capsules/decrypt-provider`): the PQ-hybrid product target vs the PC2/Lit
/// classical-compat migration path.
const SUITE_PQ_HYBRID: &str = "elastos-pq-hybrid-threshold-v0";
const SUITE_CLASSICAL_COMPAT: &str = "p256-classical-compat";

/// A key-delivery backend hosted *inside* the key-provider authority boundary.
///
/// `key-provider` is the authority boundary, not a single key system. Anders'
/// model (confirmed): interchangeable backends sit inside it and all produce the
/// same suite-tagged `SealedDecryptMaterialV1` handoff that the decrypt sandbox
/// consumes. This mirrors the PC2 Lit authority role (`src/api/chipotle-client.ts`
/// `recoverCEKEnvelope`/`envelopeCEK`, `data/lit-actions/universal-decrypt-chipotle.js`):
/// validate access, recover the CEK in a trusted boundary, and re-seal it to the
/// viewer's session — never returning a raw CEK.
///
/// Selection is operator/runtime config at `init`, never an app input, so the
/// shared `KeyReleaseRequestV1` contract stays byte-identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyAuthorityBackend {
    /// In-runtime native dev/reference authority (PQ-hybrid). Lets the whole dDRM
    /// loop be tested with no external dependency. Seal engine = Phase A.2.
    Reference,
    /// Production ElastOS PQ-hybrid threshold dKMS (external authority node).
    Dkms,
    /// PC2 / Lit-Chipotle compatibility backend (migration only, classical suite).
    Lit,
}

impl KeyAuthorityBackend {
    fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "reference" => Some(Self::Reference),
            "dkms" => Some(Self::Dkms),
            "lit" => Some(Self::Lit),
            _ => None,
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Self::Reference => "reference",
            Self::Dkms => "dkms",
            Self::Lit => "lit",
        }
    }

    /// The `SealedDecryptMaterialV1.suite` this backend emits.
    fn suite(self) -> &'static str {
        match self {
            Self::Reference | Self::Dkms => SUITE_PQ_HYBRID,
            Self::Lit => SUITE_CLASSICAL_COMPAT,
        }
    }

    /// Coarse provenance, surfaced in `status` so operators can see which backends
    /// are native vs compat without reading the source.
    fn kind(self) -> &'static str {
        match self {
            Self::Reference => "native-dev",
            Self::Dkms => "native-production",
            Self::Lit => "compat-migration",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Init {
        #[serde(default)]
        config: Value,
    },
    Status,
    Release {
        request: Box<KeyReleaseRequestV1>,
        /// Runtime-injected per-session context for the reference backend. ABSENT for the
        /// fail-closed/route-only path. The escrow blob + KID + scheme are read from the
        /// rights-bound `request.key_envelope` (NOT here); this carries only the material
        /// the runtime injects at open time — the decrypt session's published key, the
        /// producer's verifying key, and the decrypt-transcript binding.
        #[serde(default)]
        session: Option<ReleaseSessionContext>,
    },
    /// Reference key-authority seal (feature `key-authority-ref`, Phase A.2).
    /// Capsule-local op so the shared `KeyReleaseRequestV1` stays byte-identical:
    /// seal a recovered CEK to a decrypt session's published key and return the
    /// suite-tagged `SealedDecryptMaterialV1` the decrypt boundary opens.
    #[cfg(feature = "key-authority-ref")]
    ReleaseRef {
        request: Box<KeyReleaseRequestV1>,
        /// The decrypt boundary's published session public key (Day-47 rail-mint):
        /// base64 of `ddrm_envelope::session_public_bytes`.
        decrypt_session_pub_b64: String,
        /// The recovered CEK to seal. In production the reference authority recovers
        /// this from the dKMS-wrapped envelope; the dev reference backend is handed
        /// it directly through this capsule-local op (never on the shared contract).
        /// Sealed immediately, held in `Zeroizing`, and never echoed back.
        cek_b64: String,
        /// Canonical decrypt-transcript bytes the seal is bound to (AES-256-GCM AAD
        /// plus signed payload). Empty = unbound. The full `DecryptTranscriptV1`
        /// encoding becomes shared when the contract opens.
        #[serde(default)]
        aad_b64: String,
        /// Content fields carried straight into the material (the authority does not
        /// touch them).
        ciphertext_b64: String,
        content_hash_b64: String,
        nonce_b64: String,
        #[serde(default)]
        init_segment_b64: Option<String>,
    },
    /// Phase C (Day 60): release a CEK the PRODUCER escrowed to this authority, rather
    /// than a raw CEK. The authority recovers the CEK from the escrow blob (verifying
    /// the producer + binding the KID via the shared escrow AAD), then re-seals it to
    /// the decrypt session — closing producer→authority→decrypt with no raw CEK on any
    /// wire. Requires the `reference` backend.
    #[cfg(feature = "key-authority-ref")]
    ReleaseFromEscrowRef {
        request: Box<KeyReleaseRequestV1>,
        decrypt_session_pub_b64: String,
        /// The producer's escrow blob (`ddrm-envelope` sealed envelope, base64) — the
        /// CEK sealed to THIS authority's recipient key. Never a raw CEK.
        wrapped_cek_b64: String,
        /// The producer's published verifying key (base64) to authenticate the escrow.
        producer_vk_b64: String,
        /// Content identity (32-hex KID == on-chain bytes16 contentId) the escrow AAD
        /// is bound to; a mismatch fails closed.
        kid_hex: String,
        /// Sealing suite the escrow AAD is bound to (must match the producer).
        scheme: String,
        /// Decrypt-transcript AAD the re-seal binds to (same as `release_ref`).
        #[serde(default)]
        aad_b64: String,
        ciphertext_b64: String,
        content_hash_b64: String,
        nonce_b64: String,
        #[serde(default)]
        init_segment_b64: Option<String>,
    },
    Shutdown,
}

/// Runtime-injected session context for the canonical `release` op (reference backend).
///
/// The CEK source (the producer's escrow blob), the KID, and the scheme all come from the
/// rights-bound `KeyReleaseRequestV1.key_envelope` — so the wrapped CEK travels inside the
/// SAME object the rights step validated, not as a side-band parameter. This struct carries
/// only the per-SESSION material the runtime injects at open time: the decrypt boundary's
/// published key, the producer's verifying key (to authenticate the escrow), and the
/// decrypt-transcript binding. Mirrors the jsParams PC2's client assembles for the Lit
/// action (`recoverCEKEnvelope`, `chipotle-client.ts:1486-1510`): a session public key,
/// a signed request, and the content references — never a raw CEK.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)] // fields are read only in the `key-authority-ref` build
struct ReleaseSessionContext {
    /// The decrypt boundary's published session public key (base64 of `session_public_bytes`).
    decrypt_session_pub_b64: String,
    /// The producer's published ML-DSA verifying key (base64) authenticating the escrow.
    producer_vk_b64: String,
    /// Canonical decrypt-transcript bytes the re-seal binds to. Empty = unbound.
    #[serde(default)]
    aad_b64: String,
    /// Content fields carried straight into the sealed material (the authority never touches them).
    ciphertext_b64: String,
    content_hash_b64: String,
    nonce_b64: String,
    #[serde(default)]
    init_segment_b64: Option<String>,
    /// Optional wall-clock for expiry enforcement: if set and the request has expired, the
    /// authority refuses to release (fail-closed), never sealing a CEK past its window.
    #[serde(default)]
    now_unix: Option<u64>,
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

/// The in-runtime reference key authority: a deterministic ML-DSA-65 seal signer +
/// its published verifying key. Dev-only (feature `key-authority-ref`); production
/// uses the `dkms` backend.
#[cfg(feature = "key-authority-ref")]
struct ReferenceAuthority {
    signer: ddrm_envelope::seal::MlDsaSealSigner,
    verifying_key: Vec<u8>,
    /// The authority's PQ-hybrid KEM **recipient** keypair. The producer
    /// (`encrypt-provider`) escrows a freshly-minted CEK by sealing it to the
    /// published `recipient_public` (Phase C); the authority holds the secret and
    /// recovers the CEK to re-seal it per decrypt session. Distinct from the ML-DSA
    /// `signer` (which signs seals) — this is the encryption recipient.
    recipient_secret: ddrm_envelope::SessionKemSecret,
    recipient_public: Vec<u8>,
}

#[cfg(feature = "key-authority-ref")]
impl ReferenceAuthority {
    /// Recover a CEK the producer escrowed to THIS authority's recipient key. The
    /// producer sealed it under `escrow_aad(scheme, kid16, recipient_public)` and
    /// signed with its ML-DSA key; we recompute the IDENTICAL AAD (shared encoder)
    /// and verify the producer's published verifying key, then hybrid-unwrap with our
    /// recipient secret. Fails closed on any mismatch — wrong producer, wrong KID,
    /// wrong scheme, or a re-targeted envelope. The CEK stays in `Zeroizing`.
    fn recover_escrowed_cek(
        &self,
        wrapped_cek: &[u8],
        scheme: &str,
        kid_bytes16: &[u8; 16],
        producer_vk: &[u8],
    ) -> Result<zeroize::Zeroizing<Vec<u8>>, String> {
        let env = ddrm_envelope::PqSealedEnvelope::from_bytes(wrapped_cek)
            .map_err(|e| format!("malformed escrow envelope: {e:?}"))?;
        let verifier = ddrm_envelope::MlDsa65Verifier::from_encoded(producer_vk)
            .ok_or_else(|| "malformed producer verifying key".to_string())?;
        let aad = ddrm_envelope::transcript::escrow_aad(scheme, kid_bytes16, &self.recipient_public);
        ddrm_envelope::hybrid_unwrap_bound(&self.recipient_secret, &env, &aad, &verifier)
            .map_err(|e| format!("escrow recover failed: {e:?}"))
    }
}

#[derive(Default)]
struct KeyProvider {
    /// Active key-delivery backend, selected by operator/runtime config at `init`.
    /// `None` = no authority configured = `release` fails closed.
    backend: Option<KeyAuthorityBackend>,
    /// The reference seal authority, constructed at `init` when the `reference`
    /// backend is selected (feature `key-authority-ref`).
    #[cfg(feature = "key-authority-ref")]
    reference: Option<ReferenceAuthority>,
    /// The `dkms` EXTERNAL authority CLIENT, constructed at `init` from a PUBLIC-ONLY descriptor
    /// when the `dkms` backend is selected. Holds ONLY the node's published identity + endpoint —
    /// NO master, NO recovery secret. `release` DELEGATES recovery to the node (feature
    /// `key-authority-ref`).
    #[cfg(feature = "key-authority-ref")]
    dkms: Option<DkmsClientAuthority>,
}

/// The `dkms` EXTERNAL authority as the runtime SEES it: the node's PUBLIC identity (what the
/// producer escrowed to + what the decrypt boundary trusts) and the `authority_endpoint` where the
/// secret-holding node lives. The runtime holds NO master and NO recovery secret — it DELEGATES
/// recovery to the node. The runtime-core analogue of PC2's client holding only the external
/// authority's public `pkpId`/`authority` and RPCing the Lit network (`recoverCEKEnvelope`).
#[cfg(feature = "key-authority-ref")]
#[derive(Clone)]
struct DkmsClientAuthority {
    /// The node's published ML-DSA verifying key (base64) — the decrypt boundary trusts its seals.
    verifying_key_b64: String,
    /// The node's published KEM recipient (base64) — the producer escrows the CEK to it.
    recipient_pub_b64: String,
    /// Where the secret-holding authority node lives (its capsule binary path) — the granted
    /// endpoint the client RPCs for `recover`. NOT a secret; an address (PC2's `pkpId` analogue).
    endpoint: String,
}

impl KeyProvider {
    fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::Init { config } => self.init(config),
            Request::Status => self.status(),
            Request::Release { request, session } => self.release(*request, session),
            #[cfg(feature = "key-authority-ref")]
            Request::ReleaseRef {
                request,
                decrypt_session_pub_b64,
                cek_b64,
                aad_b64,
                ciphertext_b64,
                content_hash_b64,
                nonce_b64,
                init_segment_b64,
            } => self.release_ref(
                *request,
                &decrypt_session_pub_b64,
                &cek_b64,
                &aad_b64,
                ciphertext_b64,
                content_hash_b64,
                nonce_b64,
                init_segment_b64,
            ),
            #[cfg(feature = "key-authority-ref")]
            Request::ReleaseFromEscrowRef {
                request,
                decrypt_session_pub_b64,
                wrapped_cek_b64,
                producer_vk_b64,
                kid_hex,
                scheme,
                aad_b64,
                ciphertext_b64,
                content_hash_b64,
                nonce_b64,
                init_segment_b64,
            } => self.release_from_escrow_ref(
                *request,
                &decrypt_session_pub_b64,
                &wrapped_cek_b64,
                &producer_vk_b64,
                &kid_hex,
                &scheme,
                &aad_b64,
                ciphertext_b64,
                content_hash_b64,
                nonce_b64,
                init_segment_b64,
            ),
            Request::Shutdown => Response::empty_ok(),
        }
    }

    fn init(&mut self, config: Value) -> Response {
        match config.get("backend") {
            None | Some(Value::Null) => self.backend = None,
            Some(Value::String(tag)) => match KeyAuthorityBackend::from_tag(tag) {
                Some(backend) => self.backend = Some(backend),
                None => {
                    return Response::error(
                        "invalid_request",
                        format!("unknown key authority backend: {tag}"),
                    );
                }
            },
            Some(_) => {
                return Response::error("invalid_request", "backend must be a string");
            }
        }

        // Stand up the reference seal authority when that backend is selected. When the
        // operator configures a durable `authority_key_store` path, the authority is
        // PRODUCTION-SHAPED: its master seed is loaded (or created + persisted ONCE) from that
        // store, and both the signer and the KEM recipient are deterministically re-derived
        // from it — so the recipient is STABLE across restarts (escrow-at-publish). Without a
        // store, the dev default mints a fresh recipient per init. Fail-closed: a corrupt /
        // unreadable store fails the open rather than silently minting a divergent authority.
        #[cfg(feature = "key-authority-ref")]
        {
            match self.backend {
                Some(KeyAuthorityBackend::Reference) => {
                    self.reference = match build_reference_authority(&config) {
                        Ok(authority) => Some(authority),
                        Err(err) => return Response::error("not_configured", err),
                    };
                }
                // The `dkms` backend RESOLVES the EXTERNAL authority's PUBLIC identity + endpoint
                // from a handed-in PUBLIC-ONLY descriptor (Day 87–88) — NO master, NO recovery secret
                // reaches the runtime. No descriptor → unconfigured (release fails closed); a
                // present-but-bad (or secret-bearing) descriptor fails closed here.
                Some(KeyAuthorityBackend::Dkms) => {
                    self.dkms = match build_dkms_client(&config) {
                        Ok(client) => client,
                        Err(err) => return Response::error("not_configured", err),
                    };
                }
                _ => {}
            }
        }

        #[allow(unused_mut)]
        let mut data = json!({
            "provider": "key",
            "protocol_version": "1.0",
            "configured": false,
            "active_backend": self.backend.map(KeyAuthorityBackend::tag),
            "supported_operations": ["status", "release"],
        });
        // A key authority PUBLISHES its verifying key so the decrypt boundary can be
        // configured (at its own `init`) to trust this authority's seals BEFORE it
        // mints + publishes a session key. This is what breaks the bootstrap ordering
        // for `drm/open → rights → key → decrypt`: the vk is known up front, the
        // session pubkey is minted after, and only then is the CEK sealed.
        #[cfg(feature = "key-authority-ref")]
        if let Some(authority) = self.reference.as_ref() {
            use base64::Engine as _;
            data["seal_verifying_key_b64"] = json!(base64::engine::general_purpose::STANDARD
                .encode(&authority.verifying_key));
            // The authority also publishes its KEM RECIPIENT key so the producer
            // (encrypt-provider) can escrow a freshly-minted CEK to it (Phase C).
            data["seal_recipient_pub_b64"] = json!(base64::engine::general_purpose::STANDARD
                .encode(&authority.recipient_public));
        }
        // The `dkms` CLIENT republishes the EXTERNAL node's PUBLIC identity straight from the
        // descriptor pins (no key material is held here) — same rail contract: the decrypt boundary
        // trusts the vk, the producer escrows to the recipient.
        #[cfg(feature = "key-authority-ref")]
        if let Some(client) = self.dkms.as_ref() {
            data["seal_verifying_key_b64"] = json!(client.verifying_key_b64);
            data["seal_recipient_pub_b64"] = json!(client.recipient_pub_b64);
        }
        Response::ok(data)
    }

    /// Reference key-authority seal (feature `key-authority-ref`, Phase A.2). Runs
    /// the same fail-closed validation as `release`, requires the `reference`
    /// backend, then seals the recovered CEK to the decrypt boundary's published
    /// session key via the shared `ddrm-envelope` crate — the SAME code the decrypt
    /// boundary unwraps with. The CEK is held in `Zeroizing` and only ever leaves
    /// this boundary SEALED (the response carries no raw CEK).
    #[cfg(feature = "key-authority-ref")]
    #[allow(clippy::too_many_arguments)]
    fn release_ref(
        &self,
        request: KeyReleaseRequestV1,
        decrypt_session_pub_b64: &str,
        cek_b64: &str,
        aad_b64: &str,
        ciphertext_b64: String,
        content_hash_b64: String,
        nonce_b64: String,
        init_segment_b64: Option<String>,
    ) -> Response {
        use base64::Engine as _;
        use zeroize::Zeroizing;

        if let Err(err) = validate_key_release_request(&request) {
            return Response::error("invalid_request", err);
        }

        let authority = match (self.backend, self.reference.as_ref()) {
            (Some(KeyAuthorityBackend::Reference), Some(authority)) => authority,
            _ => {
                return Response::error(
                    "not_configured",
                    "release_ref requires the reference key authority backend",
                );
            }
        };

        let b64 = base64::engine::general_purpose::STANDARD;
        let pub_bytes = match b64.decode(decrypt_session_pub_b64) {
            Ok(bytes) => bytes,
            Err(_) => {
                return Response::error(
                    "invalid_request",
                    "decrypt_session_pub_b64 is not valid base64",
                )
            }
        };
        let public = match ddrm_envelope::session_public_from_bytes(&pub_bytes) {
            Some(public) => public,
            None => {
                return Response::error(
                    "invalid_request",
                    "decrypt_session_pub_b64 is not a valid session public key",
                )
            }
        };
        let aad = match b64.decode(aad_b64) {
            Ok(bytes) => bytes,
            Err(_) => return Response::error("invalid_request", "aad_b64 is not valid base64"),
        };
        let cek = match b64.decode(cek_b64) {
            Ok(bytes) => Zeroizing::new(bytes),
            Err(_) => return Response::error("invalid_request", "cek_b64 is not valid base64"),
        };

        seal_recovered_cek_into_material(
            authority,
            &public,
            cek.as_slice(),
            &aad,
            ciphertext_b64,
            content_hash_b64,
            nonce_b64,
            init_segment_b64,
        )
    }

    /// Phase C: recover a producer-escrowed CEK and re-seal it to the decrypt session.
    /// Same fail-closed validation + reference-backend requirement as `release_ref`,
    /// but the CEK source is the escrow blob (recovered in-boundary) rather than a raw
    /// `cek_b64` — so no raw CEK ever crosses a wire into this authority either.
    #[cfg(feature = "key-authority-ref")]
    #[allow(clippy::too_many_arguments)]
    fn release_from_escrow_ref(
        &self,
        request: KeyReleaseRequestV1,
        decrypt_session_pub_b64: &str,
        wrapped_cek_b64: &str,
        producer_vk_b64: &str,
        kid_hex: &str,
        scheme: &str,
        aad_b64: &str,
        ciphertext_b64: String,
        content_hash_b64: String,
        nonce_b64: String,
        init_segment_b64: Option<String>,
    ) -> Response {
        use base64::Engine as _;

        if let Err(err) = validate_key_release_request(&request) {
            return Response::error("invalid_request", err);
        }
        let authority = match (self.backend, self.reference.as_ref()) {
            (Some(KeyAuthorityBackend::Reference), Some(authority)) => authority,
            _ => {
                return Response::error(
                    "not_configured",
                    "release_from_escrow_ref requires the reference key authority backend",
                )
            }
        };

        let b64 = base64::engine::general_purpose::STANDARD;
        let public = match b64
            .decode(decrypt_session_pub_b64)
            .ok()
            .and_then(|bytes| ddrm_envelope::session_public_from_bytes(&bytes))
        {
            Some(public) => public,
            None => {
                return Response::error(
                    "invalid_request",
                    "decrypt_session_pub_b64 is not a valid session public key",
                )
            }
        };
        let aad = match b64.decode(aad_b64) {
            Ok(bytes) => bytes,
            Err(_) => return Response::error("invalid_request", "aad_b64 is not valid base64"),
        };
        let wrapped = match b64.decode(wrapped_cek_b64) {
            Ok(bytes) => bytes,
            Err(_) => {
                return Response::error("invalid_request", "wrapped_cek_b64 is not valid base64")
            }
        };
        let producer_vk = match b64.decode(producer_vk_b64) {
            Ok(bytes) => bytes,
            Err(_) => {
                return Response::error("invalid_request", "producer_vk_b64 is not valid base64")
            }
        };
        let kid16 = match decode_kid_bytes16(kid_hex) {
            Ok(k) => k,
            Err(e) => return Response::error("invalid_request", e),
        };

        // Recover the escrowed CEK in this boundary (fail-closed on a foreign/tampered
        // blob, a KID-swap, or a forged producer); re-seal to the decrypt session.
        let cek = match authority.recover_escrowed_cek(&wrapped, scheme, &kid16, &producer_vk) {
            Ok(cek) => cek,
            Err(_) => {
                return Response::error(
                    "invalid_request",
                    "escrowed CEK could not be recovered (foreign/tampered escrow, wrong KID, or bad producer key)",
                )
            }
        };

        seal_recovered_cek_into_material(
            authority,
            &public,
            cek.as_slice(),
            &aad,
            ciphertext_b64,
            content_hash_b64,
            nonce_b64,
            init_segment_b64,
        )
    }

    fn status(&self) -> Response {
        Response::ok(json!({
            "provider": "key",
            "version": PROVIDER_VERSION,
            "configured": false,
            "active_backend": self.backend.map(KeyAuthorityBackend::tag),
            "supported_operations": ["status", "release"],
            "supported_schemes": SUPPORTED_SCHEMES,
            "supported_backends": supported_backends_descriptor(),
            "blocked_authority": [
                "raw_cek",
                "kms_node_credentials",
                "chain_rpc",
                "wallet_rpc",
                "provider_credentials"
            ],
            "next_required_providers": [
                "rights-provider",
                "decrypt-provider"
            ],
        }))
    }

    fn release(&self, request: KeyReleaseRequestV1, session: Option<ReleaseSessionContext>) -> Response {
        // Validation (schema, rights-receipt binding, scheme, PQ-hybrid algorithms)
        // always runs *before* any backend is consulted: a malformed or
        // unauthorized request must never reach a key-delivery backend.
        if let Err(err) = validate_key_release_request(&request) {
            return Response::error("invalid_request", err);
        }

        match self.backend {
            None => Response::error(
                "not_configured",
                "key release requires a configured key authority backend (reference | dkms | lit)",
            ),
            // The reference backend ACTUALLY releases (Day 70): recover the producer-escrowed
            // CEK from the rights-bound `key_envelope` and re-seal it to the decrypt session.
            Some(KeyAuthorityBackend::Reference) => self.release_reference(&request, session),
            // The `dkms` backend DELEGATES recovery to the EXTERNAL authority node (Day 87–88):
            // the runtime holds only the node's public identity, so it RPCs the node's endpoint to
            // recover + re-seal — the master/CEK never enter the runtime. Selected-but-unprovisioned
            // (no descriptor/endpoint) falls through to the fail-closed "no dKMS node" surface.
            #[cfg(feature = "key-authority-ref")]
            Some(KeyAuthorityBackend::Dkms) if self.dkms.is_some() => {
                self.release_dkms_delegated(&request, session)
            }
            Some(backend) => self.release_via_backend(backend, &request),
        }
    }

    /// Canonical reference-backend release (Day 70): the op `drm-provider`'s `DrmOpenPlanV1`
    /// names for the key step. Mirrors PC2's Lit action (`universal-decrypt-chipotle.js`):
    /// access-check (the rights receipt, already validated) → recover the CEK
    /// (`Lit.Actions.Decrypt` ≈ recovering the producer-escrowed CEK in-boundary) →
    /// CEK↔KID↔authority bind (the escrow AAD recompute) → seal-to-session (`envelopeCEK` ≈
    /// `seal_recovered_cek_into_material`). The CEK source, KID and scheme come from the
    /// rights-bound `key_envelope` — so the wrapped CEK rides inside the validated request,
    /// not as a side-band param. The CEK stays in `Zeroizing` and leaves only SEALED.
    fn release_reference(
        &self,
        request: &KeyReleaseRequestV1,
        session: Option<ReleaseSessionContext>,
    ) -> Response {
        #[cfg(feature = "key-authority-ref")]
        {
            use base64::Engine as _;
            let b64 = base64::engine::general_purpose::STANDARD;

            let authority = match self.reference.as_ref() {
                Some(authority) => authority,
                None => {
                    return Response::error(
                        "not_configured",
                        "seal authority backend selected but not initialized \
                         (reference: init with backend=reference; dkms: provide dkms_authority_descriptor)",
                    )
                }
            };
            let session = match session {
                Some(session) => session,
                None => {
                    return Response::error(
                        "not_configured",
                        "reference key authority release requires runtime-injected session context \
                         (decrypt session key + producer vk + transcript)",
                    )
                }
            };
            // Refuse to release on an already-expired request (fail-closed), when the
            // runtime supplies a clock. The CEK must never be sealed past its window.
            if let Some(now) = session.now_unix {
                if request.expires_at <= now {
                    return Response::error("invalid_request", "key release request has expired");
                }
            }

            // Escrow material rides inside the rights-bound key_envelope.
            let wrapped = match b64.decode(&request.key_envelope.wrapped_cek) {
                Ok(bytes) => bytes,
                Err(_) => {
                    return Response::error(
                        "invalid_request",
                        "key_envelope.wrapped_cek is not valid base64",
                    )
                }
            };
            let kid16 = match decode_kid_bytes16(&request.key_envelope.kid) {
                Ok(k) => k,
                Err(e) => return Response::error("invalid_request", e),
            };
            let producer_vk = match b64.decode(&session.producer_vk_b64) {
                Ok(bytes) => bytes,
                Err(_) => {
                    return Response::error("invalid_request", "producer_vk_b64 is not valid base64")
                }
            };
            let public = match b64
                .decode(&session.decrypt_session_pub_b64)
                .ok()
                .and_then(|bytes| ddrm_envelope::session_public_from_bytes(&bytes))
            {
                Some(public) => public,
                None => {
                    return Response::error(
                        "invalid_request",
                        "decrypt_session_pub_b64 is not a valid session public key",
                    )
                }
            };
            let aad = match b64.decode(&session.aad_b64) {
                Ok(bytes) => bytes,
                Err(_) => return Response::error("invalid_request", "aad_b64 is not valid base64"),
            };

            // Recover the escrowed CEK in this boundary (fail-closed on a foreign/tampered
            // blob, a KID-swap, a scheme mismatch, or a forged producer); re-seal to the session.
            let cek = match authority.recover_escrowed_cek(
                &wrapped,
                &request.key_envelope.scheme,
                &kid16,
                &producer_vk,
            ) {
                Ok(cek) => cek,
                Err(_) => {
                    return Response::error(
                        "invalid_request",
                        "escrowed CEK could not be recovered (foreign/tampered escrow, wrong KID/scheme, or bad producer key)",
                    )
                }
            };

            seal_recovered_cek_into_material(
                authority,
                &public,
                cek.as_slice(),
                &aad,
                session.ciphertext_b64,
                session.content_hash_b64,
                session.nonce_b64,
                session.init_segment_b64,
            )
        }
        #[cfg(not(feature = "key-authority-ref"))]
        {
            let _ = (request, session);
            Response::error(
                "not_configured",
                "reference key authority requires the key-authority-ref build",
            )
        }
    }

    /// Canonical `dkms` release (Day 87–88): the runtime holds only the EXTERNAL authority's PUBLIC
    /// identity, so it DELEGATES recovery to the secret-holding node. It assembles the recover bundle
    /// (the producer-escrowed CEK from the rights-bound `key_envelope` + the per-open session material
    /// the runtime injects), RPCs the node's granted `endpoint` for `recover`, and returns the node's
    /// `SealedDecryptMaterialV1` verbatim. The master + raw CEK NEVER enter this process — exactly as
    /// PC2's client RPCs the Lit network and only ever sees the sealed envelope (`recoverCEKEnvelope`,
    /// `chipotle-client.ts:1438`; the Lit action recovers + seals in the TEE, returns only the
    /// envelope, `universal-decrypt-chipotle.js:572`/`:602`/`:610`).
    #[cfg(feature = "key-authority-ref")]
    fn release_dkms_delegated(
        &self,
        request: &KeyReleaseRequestV1,
        session: Option<ReleaseSessionContext>,
    ) -> Response {
        let client = match self.dkms.as_ref() {
            Some(client) => client,
            None => {
                return Response::error(
                    "not_configured",
                    "dkms authority selected but no external node provisioned (provide a PUBLIC-ONLY dkms_authority_descriptor)",
                )
            }
        };
        let session = match session {
            Some(session) => session,
            None => {
                return Response::error(
                    "not_configured",
                    "dkms key authority release requires runtime-injected session context \
                     (decrypt session key + producer vk + transcript)",
                )
            }
        };
        // Fail-closed on an already-expired request (the node would seal a CEK past its window).
        if let Some(now) = session.now_unix {
            if request.expires_at <= now {
                return Response::error("invalid_request", "key release request has expired");
            }
        }
        // The KID must be the on-chain bytes16 the escrow AAD binds (validated shape).
        let kid_hex = match decode_kid_bytes16(&request.key_envelope.kid) {
            Ok(_) => request.key_envelope.kid.clone(),
            Err(e) => return Response::error("invalid_request", e),
        };
        // Assemble the recover bundle and DELEGATE to the node. The escrow blob + KID + scheme come
        // from the rights-bound key_envelope; the session key + transcript come from the runtime
        // context. NO key material is held here — only forwarded to the node + sealed result returned.
        let recover_req = json!({
            "op": "recover",
            "wrapped_cek_b64": request.key_envelope.wrapped_cek,
            "scheme": request.key_envelope.scheme,
            "kid_hex": kid_hex,
            "producer_vk_b64": session.producer_vk_b64,
            "decrypt_session_pub_b64": session.decrypt_session_pub_b64,
            "aad_b64": session.aad_b64,
            "ciphertext_b64": session.ciphertext_b64,
            "content_hash_b64": session.content_hash_b64,
            "nonce_b64": session.nonce_b64,
            "init_segment_b64": session.init_segment_b64,
        });
        match delegate_to_dkms_node(&client.endpoint, &recover_req) {
            // The node returns the suite-tagged material verbatim — pass it through unchanged.
            Ok(data) => Response::ok(data),
            Err(err) => Response::error("not_configured", err),
        }
    }

    /// Route an already-validated, authorized request to the selected backend.
    ///
    /// Phase A.1 lands the routing + fail-closed surface only: every backend
    /// reports the precise capability it still needs before it can seal a CEK.
    /// The in-runtime `reference` seal engine (CEK sealed to the decrypt session's
    /// published key as a `SealedDecryptMaterialV1`) lands in Phase A.2; no backend
    /// returns a raw CEK at any point.
    fn release_via_backend(
        &self,
        backend: KeyAuthorityBackend,
        _request: &KeyReleaseRequestV1,
    ) -> Response {
        match backend {
            KeyAuthorityBackend::Reference => Response::error(
                "not_configured",
                "reference key authority is selected; the in-runtime seal engine lands in Phase A.2",
            ),
            KeyAuthorityBackend::Dkms => Response::error(
                "not_configured",
                "ElastOS PQ-hybrid dKMS backend is selected but no dKMS node is provisioned",
            ),
            KeyAuthorityBackend::Lit => Response::error(
                "not_configured",
                "Lit/Chipotle compat backend is selected but no Lit proxy is provisioned",
            ),
        }
    }
}

/// Describe the hosted backends for `status`, so operators (and the runtime) can
/// see which key authorities are available, the decrypt-material suite each emits,
/// and what each still needs — without reading the source.
fn supported_backends_descriptor() -> Value {
    json!([
        {
            "backend": KeyAuthorityBackend::Reference.tag(),
            "suite": KeyAuthorityBackend::Reference.suite(),
            "kind": KeyAuthorityBackend::Reference.kind(),
            "state": "pending_seal_engine",
        },
        {
            "backend": KeyAuthorityBackend::Dkms.tag(),
            "suite": KeyAuthorityBackend::Dkms.suite(),
            "kind": KeyAuthorityBackend::Dkms.kind(),
            "state": "not_configured",
        },
        {
            "backend": KeyAuthorityBackend::Lit.tag(),
            "suite": KeyAuthorityBackend::Lit.suite(),
            "kind": KeyAuthorityBackend::Lit.kind(),
            "state": "not_configured",
        }
    ])
}

/// The 32-byte ML-DSA-65 seed for the dev reference seal authority. Operator may
/// pin it via `config.ref_seal_seed_b64` (32 bytes); otherwise a fixed dev seed is
/// used (the reference backend is dev-only — production uses the `dkms` backend).
/// Seal a recovered CEK (raw or escrow-recovered) to the decrypt session and render
/// the suite-tagged material response. The CEK leaves this boundary ONLY as sealed
/// material; the response carries no raw CEK. Shared by `release_ref` (raw dev CEK) and
/// `release_from_escrow_ref` (producer-escrowed CEK), so both seal identically.
#[cfg(feature = "key-authority-ref")]
#[allow(clippy::too_many_arguments)]
fn seal_recovered_cek_into_material(
    authority: &ReferenceAuthority,
    public: &ddrm_envelope::SessionKemPublic,
    cek: &[u8],
    aad: &[u8],
    ciphertext_b64: String,
    content_hash_b64: String,
    nonce_b64: String,
    init_segment_b64: Option<String>,
) -> Response {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;
    let envelope = ddrm_envelope::seal::seal_bound(public, cek, aad, &authority.signer);
    let mut material = json!({
        "suite": ddrm_envelope::SUITE_PQ_HYBRID,
        "sealed_cek_b64": b64.encode(envelope.to_bytes()),
        "ciphertext_b64": ciphertext_b64,
        "nonce_b64": nonce_b64,
        "content_hash_b64": content_hash_b64,
    });
    if let Some(init) = init_segment_b64 {
        material["init_segment_b64"] = json!(init);
    }
    Response::ok(json!({
        "suite": ddrm_envelope::SUITE_PQ_HYBRID,
        "material": material,
        "seal_verifying_key_b64": b64.encode(&authority.verifying_key),
    }))
}

/// Decode a 32-hex KID into the on-chain `bytes16` contentId the escrow AAD binds.
#[cfg(feature = "key-authority-ref")]
fn decode_kid_bytes16(kid_hex: &str) -> Result<[u8; 16], String> {
    if kid_hex.len() != 32 || !kid_hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("kid_hex must be 32 lowercase-hex chars (bytes16 contentId)".to_string());
    }
    let mut out = [0u8; 16];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&kid_hex[i * 2..i * 2 + 2], 16)
            .map_err(|e| format!("kid hex: {e}"))?;
    }
    Ok(out)
}

/// Build the reference authority for an `init`. With a durable `authority_key_store` path the
/// authority is STABLE (its master seed is persisted + re-derived); without one it mints a
/// fresh recipient per init (dev default, back-compatible).
#[cfg(feature = "key-authority-ref")]
fn build_reference_authority(config: &Value) -> Result<ReferenceAuthority, String> {
    if let Some(path) = config.get("authority_key_store").and_then(|v| v.as_str()) {
        let master = load_or_create_authority_seed(path)?;
        return Ok(reference_authority_from_master(&master));
    }
    // Dev default (no durable store): the signer seed comes from config (or a fixed default)
    // so the verifying key is stable, but the KEM recipient is minted fresh each init.
    let (signer, verifying_key) = ddrm_envelope::seal::mldsa_seal_keypair(ref_seal_seed(config));
    let (recipient_secret, recipient_public) = ddrm_envelope::mint_session();
    Ok(ReferenceAuthority {
        signer,
        verifying_key,
        recipient_public: ddrm_envelope::session_public_bytes(&recipient_public),
        recipient_secret,
    })
}

/// Resolve the `dkms` EXTERNAL authority CLIENT from a handed-in PUBLIC-ONLY descriptor (Day 87–88),
/// NEVER minting and NEVER holding key material. `None` when no descriptor is configured (the backend
/// is selected but unconfigured → `release` fails closed, the PC2 "backend selected, no node
/// provisioned" shape). A present descriptor is READ (never written): it MUST carry the node's
/// PUBLISHED identity (`verifying_key_b64` AND `recipient_pub_b64`, what the producer escrowed to +
/// the decrypt boundary trusts) and its `authority_endpoint` (where the secret-holding node lives).
/// It MUST NOT carry any secret: an `authority_master_seed_b64` (the old v1 shape) is REJECTED —
/// the recovery secret must never reach the runtime. Mirrors PC2 holding only the external authority's
/// PUBLIC `pkpId`/`authority` and delegating recovery to the Lit network (`recoverCEKEnvelope`,
/// `chipotle-client.ts:1438`), never the PKP secret.
#[cfg(feature = "key-authority-ref")]
fn build_dkms_client(config: &Value) -> Result<Option<DkmsClientAuthority>, String> {
    let path = match config.get("dkms_authority_descriptor").and_then(|v| v.as_str()) {
        Some(path) => path,
        None => return Ok(None),
    };
    let bytes = std::fs::read(path).map_err(|e| format!("dkms authority descriptor {path}: {e}"))?;
    let desc: Value = serde_json::from_slice(&bytes)
        .map_err(|e| format!("dkms authority descriptor {path} is corrupt: {e}"))?;
    if desc.get("schema").and_then(|v| v.as_str()) != Some(DKMS_AUTHORITY_DESCRIPTOR_SCHEMA) {
        return Err(format!(
            "dkms authority descriptor {path} has an unexpected schema (expected {DKMS_AUTHORITY_DESCRIPTOR_SCHEMA}, the PUBLIC-ONLY shape)"
        ));
    }
    // HARD BOUNDARY: a secret-bearing descriptor is rejected. The runtime must NEVER be handed the
    // master — it holds only the node's PUBLIC identity + endpoint and delegates recovery.
    if desc.get("authority_master_seed_b64").is_some() {
        return Err(format!(
            "dkms authority descriptor {path} carries a master seed — the runtime must hold the PUBLIC identity ONLY (the secret stays in the node)"
        ));
    }
    let field = |key: &str, what: &str| {
        desc.get(key)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| format!("dkms authority descriptor {path} is missing {key} ({what})"))
    };
    let verifying_key_b64 = field("verifying_key_b64", "an external authority must publish its identity")?;
    let recipient_pub_b64 = field("recipient_pub_b64", "an external authority must publish its escrow recipient")?;
    let endpoint = field("authority_endpoint", "the runtime must know where to delegate recovery")?;
    Ok(Some(DkmsClientAuthority { verifying_key_b64, recipient_pub_b64, endpoint }))
}

/// DELEGATE one `recover` to the EXTERNAL dKMS authority node over the capsule protocol: spawn the
/// granted `endpoint` binary, `init` it (the node resolves its OWN master store — from its config or
/// the `DKMS_AUTHORITY_KEY_STORE` env the provisioner set; this client never passes or sees the
/// secret store path), send the recover bundle, and return the node's sealed material `data`. The
/// node owns spawn→teardown of its own boundary; we shut it down after the single recover. Any
/// transport/protocol error fails closed. The runtime-core analogue of PC2's client RPCing the Lit
/// network and only ever receiving the sealed envelope (`recoverCEKEnvelope`, `chipotle-client.ts:1438`).
#[cfg(feature = "key-authority-ref")]
fn delegate_to_dkms_node(endpoint: &str, recover_req: &Value) -> Result<Value, String> {
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::process::{Command, Stdio};

    let mut child = Command::new(endpoint)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("dkms authority node ({endpoint}) failed to launch: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("dkms node: no stdin")?;
    let mut stdout = BufReader::new(child.stdout.take().ok_or("dkms node: no stdout")?);

    // One request line in, one response line out (the node's protocol).
    let mut call = |req: &Value| -> Result<Value, String> {
        let line = serde_json::to_string(req).map_err(|e| e.to_string())?;
        writeln!(stdin, "{line}").map_err(|e| format!("write to dkms node: {e}"))?;
        stdin.flush().map_err(|e| e.to_string())?;
        let mut resp = String::new();
        let n = stdout
            .read_line(&mut resp)
            .map_err(|e| format!("read from dkms node: {e}"))?;
        if n == 0 {
            return Err("dkms node closed its output unexpectedly".to_string());
        }
        serde_json::from_str::<Value>(resp.trim())
            .map_err(|e| format!("dkms node sent non-JSON: {e}: {resp}"))
    };

    // The node loads its OWN master store (config-less init → it falls back to its env). We pass NO
    // store path: the secret's location is the node's concern, never the client's.
    let init_status = call(&json!({ "op": "init", "config": {} }));
    let recover = init_status.and_then(|init| {
        if init.get("status").and_then(|v| v.as_str()) != Some("ok") {
            return Err(format!(
                "dkms node init failed: {}",
                init.get("message").and_then(|v| v.as_str()).unwrap_or("unknown error")
            ));
        }
        call(recover_req)
    });

    // Tear the node down regardless of outcome, then surface the result.
    let _ = call(&json!({ "op": "shutdown" }));
    let _ = child.wait();

    let recover = recover?;
    if recover.get("status").and_then(|v| v.as_str()) != Some("ok") {
        return Err(format!(
            "dkms node recover failed: {}",
            recover.get("message").and_then(|v| v.as_str()).unwrap_or("recover rejected")
        ));
    }
    recover
        .get("data")
        .cloned()
        .ok_or_else(|| "dkms node returned ok without sealed material".to_string())
}

/// Deterministically derive the stable reference authority (signer + KEM recipient) from one
/// persisted 32-byte master seed. Domain-separated sub-seeds keep the signing key and the
/// encryption recipient independent. The SAME master always yields byte-identical keys.
#[cfg(feature = "key-authority-ref")]
fn reference_authority_from_master(master: &[u8; 32]) -> ReferenceAuthority {
    let seal_seed = ddrm_envelope::derive_seed(master, b"key-authority/seal/v1");
    let (signer, verifying_key) = ddrm_envelope::seal::mldsa_seal_keypair(seal_seed);
    let recipient_seed = ddrm_envelope::derive_seed(master, b"key-authority/recipient/v1");
    let (recipient_secret, recipient_public) = ddrm_envelope::mint_session_from_seed(recipient_seed);
    ReferenceAuthority {
        signer,
        verifying_key,
        recipient_public: ddrm_envelope::session_public_bytes(&recipient_public),
        recipient_secret,
    }
}

/// Load the authority's master seed from a durable key store, or create + persist one on first
/// launch. Mirrors PC2's stable, long-lived authority identity (`DEFAULT_AUTHORITY`, baked into
/// every video's PSSH at encode time, `dashPackager.ts:44`) — vs the per-open decrypt session
/// key. Fail-closed: a present-but-corrupt store is an error, never a silent re-mint (which
/// would strand every CEK escrowed to the prior recipient).
#[cfg(feature = "key-authority-ref")]
fn load_or_create_authority_seed(path: &str) -> Result<[u8; 32], String> {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;
    match std::fs::read(path) {
        Ok(bytes) => {
            let record: Value = serde_json::from_slice(&bytes)
                .map_err(|e| format!("authority key store {path} is corrupt: {e}"))?;
            if record.get("schema").and_then(|v| v.as_str()) != Some(AUTHORITY_KEYSTORE_SCHEMA) {
                return Err(format!("authority key store {path} has an unexpected schema"));
            }
            let seed_b64 = record
                .get("authority_seed_b64")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("authority key store {path} is missing authority_seed_b64"))?;
            let seed_bytes = b64
                .decode(seed_b64)
                .map_err(|e| format!("authority key store {path} seed is not base64: {e}"))?;
            if seed_bytes.len() != 32 {
                return Err(format!(
                    "authority key store {path} seed is {} bytes, expected 32",
                    seed_bytes.len()
                ));
            }
            let mut seed = [0u8; 32];
            seed.copy_from_slice(&seed_bytes);
            Ok(seed)
        }
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => {
            let seed = ddrm_envelope::random_seed();
            persist_authority_seed(path, &seed)?;
            Ok(seed)
        }
        Err(e) => Err(format!("authority key store {path}: {e}")),
    }
}

/// Atomically persist the authority master seed: write a temp sibling then `rename` into place
/// (a crash never leaves a torn store), best-effort `0600` on unix. The seed is the authority's
/// long-lived secret — it lives only in this durable store, never on a release wire.
#[cfg(feature = "key-authority-ref")]
fn persist_authority_seed(path: &str, seed: &[u8; 32]) -> Result<(), String> {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;
    let record = json!({
        "schema": AUTHORITY_KEYSTORE_SCHEMA,
        "authority_seed_b64": b64.encode(seed),
    });
    let bytes = serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?;
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| format!("write authority key store {tmp}: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("publish authority key store {path}: {e}")
    })
}

#[cfg(feature = "key-authority-ref")]
fn ref_seal_seed(config: &Value) -> [u8; 32] {
    use base64::Engine as _;
    if let Some(encoded) = config.get("ref_seal_seed_b64").and_then(|v| v.as_str()) {
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(encoded) {
            if bytes.len() == 32 {
                let mut seed = [0u8; 32];
                seed.copy_from_slice(&bytes);
                return seed;
            }
        }
    }
    [0x5Au8; 32]
}

fn validate_key_release_request(request: &KeyReleaseRequestV1) -> Result<(), String> {
    if request.schema != KEY_RELEASE_REQUEST_SCHEMA {
        return Err("key release request schema is unsupported".to_string());
    }
    require_non_empty(&request.request_id, "request_id")?;
    require_non_empty(&request.principal_id, "principal_id")?;
    require_non_empty(&request.session_id, "session_id")?;
    require_identifier(&request.object_cid, "object_cid")?;
    validate_action(&request.action)?;
    validate_rights_receipt(request)?;
    require_non_empty(&request.reason, "reason")?;
    require_non_empty(&request.key_envelope.scheme, "key_envelope.scheme")?;
    require_supported_scheme(&request.key_envelope.scheme)?;
    require_non_empty(&request.key_envelope.kid, "key_envelope.kid")?;
    require_non_empty(
        &request.key_envelope.wrapped_cek,
        "key_envelope.wrapped_cek",
    )?;
    require_non_empty(
        &request.key_envelope.policy_hash,
        "key_envelope.policy_hash",
    )?;
    validate_protected_content_key_envelope_algorithms(&request.key_envelope.algorithms)?;
    if request.expires_at == 0 {
        return Err("expires_at is required".to_string());
    }
    Ok(())
}

/// Verify the upstream rights decision authorizes *this* key release.
///
/// The key boundary must never release on a receipt that is denied, malformed, or
/// bound to a different principal/session/object/right. This is the `rights -> key`
/// link of the dDRM chain: rights authority lives in rights-provider; key-provider
/// fails closed unless it is handed a matching, allowed decision.
fn validate_rights_receipt(request: &KeyReleaseRequestV1) -> Result<(), String> {
    let receipt = &request.rights_receipt;
    if receipt.schema != RIGHTS_DECISION_RECEIPT_SCHEMA {
        return Err("rights receipt schema is unsupported".to_string());
    }
    require_non_empty(&receipt.request_id, "rights_receipt.request_id")?;
    if !receipt.allowed {
        return Err("rights receipt does not authorize this action".to_string());
    }
    if receipt.principal_id != request.principal_id {
        return Err("rights receipt principal does not match request".to_string());
    }
    if receipt.session_id != request.session_id {
        return Err("rights receipt session does not match request".to_string());
    }
    if receipt.content_id != request.object_cid {
        return Err("rights receipt content does not match request object".to_string());
    }
    if receipt.right != request.action {
        return Err("rights receipt right does not match requested action".to_string());
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

fn require_supported_scheme(value: &str) -> Result<(), String> {
    if SUPPORTED_SCHEMES.contains(&value) {
        Ok(())
    } else {
        Err(format!("unsupported key envelope scheme: {value}"))
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
        "key-provider: starting v{} (protected content keys)",
        PROVIDER_VERSION
    );

    let mut provider = KeyProvider::default();
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                eprintln!("key-provider read error: {}", err);
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

    eprintln!("key-provider exiting");
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_common::protected_content::{
        KeyEnvelopeAlgorithmsV1, KeyEnvelopeV1, RightsDecisionReceiptV1,
        DEFAULT_PROTECTED_CONTENT_CIPHER, DEFAULT_PROTECTED_CONTENT_KEMS,
        DEFAULT_PROTECTED_CONTENT_SHARE_SCHEME, DEFAULT_PROTECTED_CONTENT_SIGNATURES,
    };

    fn key_release_request() -> KeyReleaseRequestV1 {
        KeyReleaseRequestV1 {
            schema: KEY_RELEASE_REQUEST_SCHEMA.to_string(),
            request_id: "key-release:test".to_string(),
            principal_id: "person:local:test".to_string(),
            session_id: "session:test".to_string(),
            object_cid: "bafybeigprotectedcontent".to_string(),
            action: "view".to_string(),
            rights_receipt: RightsDecisionReceiptV1 {
                schema: RIGHTS_DECISION_RECEIPT_SCHEMA.to_string(),
                request_id: "rights:test".to_string(),
                content_id: "bafybeigprotectedcontent".to_string(),
                principal_id: "person:local:test".to_string(),
                session_id: "session:test".to_string(),
                right: "view".to_string(),
                provider: "rights-provider".to_string(),
                allowed: true,
                issued_at: 1_800_000_000,
                expires_at: 1_900_000_000,
            },
            key_envelope: KeyEnvelopeV1 {
                scheme: "elastos-pq-hybrid-threshold-v0".to_string(),
                kid: "kid:test".to_string(),
                wrapped_cek: "wrapped".to_string(),
                policy_hash: "sha256:test".to_string(),
                algorithms: KeyEnvelopeAlgorithmsV1 {
                    cipher: DEFAULT_PROTECTED_CONTENT_CIPHER.to_string(),
                    signature: DEFAULT_PROTECTED_CONTENT_SIGNATURES
                        .iter()
                        .map(|algorithm| algorithm.to_string())
                        .collect(),
                    kem: DEFAULT_PROTECTED_CONTENT_KEMS
                        .iter()
                        .map(|algorithm| algorithm.to_string())
                        .collect(),
                    share_scheme: DEFAULT_PROTECTED_CONTENT_SHARE_SCHEME.to_string(),
                },
            },
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

    fn error_message(response: Response) -> String {
        match response {
            Response::Error { message, .. } => message,
            other => panic!("expected error, got {other:?}"),
        }
    }

    fn configured(backend: KeyAuthorityBackend) -> KeyProvider {
        KeyProvider {
            backend: Some(backend),
            ..Default::default()
        }
    }

    #[test]
    fn status_advertises_blocked_raw_authority() {
        let provider = KeyProvider::default();
        let data = ok_data(provider.status());

        assert_eq!(data["provider"], "key");
        assert_eq!(data["configured"], false);
        assert!(data["blocked_authority"]
            .as_array()
            .unwrap()
            .contains(&json!("raw_cek")));
    }

    #[test]
    fn release_fails_closed_until_backend_exists() {
        let provider = KeyProvider::default();
        assert_eq!(
            error_code(provider.release(key_release_request(), None)),
            "not_configured"
        );
    }

    #[test]
    fn release_rejects_unsupported_scheme() {
        let provider = KeyProvider::default();
        let mut request = key_release_request();
        request.key_envelope.scheme = "frost-only".to_string();

        assert_eq!(error_code(provider.release(request, None)), "invalid_request");
    }

    #[test]
    fn release_rejects_weak_cipher() {
        let provider = KeyProvider::default();
        let mut request = key_release_request();
        request.key_envelope.algorithms.cipher = "aes-128-gcm".to_string();

        assert_eq!(error_code(provider.release(request, None)), "invalid_request");
    }

    #[test]
    fn release_rejects_missing_pq_hybrid_kem() {
        let provider = KeyProvider::default();
        let mut request = key_release_request();
        request.key_envelope.algorithms.kem = vec!["x25519".to_string()];

        assert_eq!(error_code(provider.release(request, None)), "invalid_request");
    }

    #[test]
    fn release_rejects_denied_rights_receipt() {
        let provider = KeyProvider::default();
        let mut request = key_release_request();
        request.rights_receipt.allowed = false;

        assert_eq!(error_code(provider.release(request, None)), "invalid_request");
    }

    #[test]
    fn release_rejects_rights_receipt_bound_to_other_principal() {
        let provider = KeyProvider::default();
        let mut request = key_release_request();
        request.rights_receipt.principal_id = "person:local:attacker".to_string();

        assert_eq!(error_code(provider.release(request, None)), "invalid_request");
    }

    #[test]
    fn release_rejects_rights_receipt_for_other_object() {
        let provider = KeyProvider::default();
        let mut request = key_release_request();
        request.rights_receipt.content_id = "bafybeigsomethingelse".to_string();

        assert_eq!(error_code(provider.release(request, None)), "invalid_request");
    }

    #[test]
    fn release_rejects_rights_receipt_for_other_action() {
        let provider = KeyProvider::default();
        let mut request = key_release_request();
        request.action = "download".to_string();
        // receipt still authorizes only "view"

        assert_eq!(error_code(provider.release(request, None)), "invalid_request");
    }

    // --- pluggable key authority backends (Phase A.1) -----------------------

    #[test]
    fn status_advertises_the_hosted_backends_with_suites() {
        let provider = KeyProvider::default();
        let data = ok_data(provider.status());

        // No backend configured by default; the surface is honest about it.
        assert!(data["active_backend"].is_null());

        let backends = data["supported_backends"].as_array().unwrap();
        let tags: Vec<&str> = backends
            .iter()
            .map(|b| b["backend"].as_str().unwrap())
            .collect();
        assert_eq!(tags, vec!["reference", "dkms", "lit"]);

        // Native backends emit the PQ-hybrid product suite; Lit is classical-compat.
        let by_tag = |tag: &str| {
            backends
                .iter()
                .find(|b| b["backend"] == tag)
                .unwrap()
                .clone()
        };
        assert_eq!(by_tag("reference")["suite"], SUITE_PQ_HYBRID);
        assert_eq!(by_tag("dkms")["suite"], SUITE_PQ_HYBRID);
        assert_eq!(by_tag("lit")["suite"], SUITE_CLASSICAL_COMPAT);
        assert_eq!(by_tag("lit")["kind"], "compat-migration");
    }

    #[test]
    fn init_selects_a_known_backend() {
        let mut provider = KeyProvider::default();
        let data = ok_data(provider.init(json!({ "backend": "reference" })));
        assert_eq!(data["active_backend"], "reference");
        assert_eq!(provider.backend, Some(KeyAuthorityBackend::Reference));

        // status reflects the active backend after init.
        let status = ok_data(provider.status());
        assert_eq!(status["active_backend"], "reference");
    }

    #[test]
    fn init_without_backend_leaves_authority_unconfigured() {
        let mut provider = KeyProvider::default();
        let data = ok_data(provider.init(json!({})));
        assert!(data["active_backend"].is_null());
        assert_eq!(provider.backend, None);
    }

    #[test]
    fn init_rejects_unknown_backend() {
        let mut provider = KeyProvider::default();
        assert_eq!(
            error_code(provider.init(json!({ "backend": "frost-cloud" }))),
            "invalid_request"
        );
        // A bad config must not silently configure an authority.
        assert_eq!(provider.backend, None);
    }

    #[test]
    fn init_rejects_non_string_backend() {
        let mut provider = KeyProvider::default();
        assert_eq!(
            error_code(provider.init(json!({ "backend": 7 }))),
            "invalid_request"
        );
        assert_eq!(provider.backend, None);
    }

    #[test]
    fn release_reference_fails_closed_without_session_context() {
        // The reference backend now ACTUALLY releases (Day 70), but only with the
        // runtime-injected session context; the canonical request alone fails closed
        // (and an uninitialized authority, as here, fails closed too).
        let response = configured(KeyAuthorityBackend::Reference).release(key_release_request(), None);
        assert_eq!(error_code_ref(&response), "not_configured");
        assert!(error_message(response).contains("reference"));
    }

    #[test]
    fn release_routes_to_dkms_backend_fail_closed_until_node() {
        let response = configured(KeyAuthorityBackend::Dkms).release(key_release_request(), None);
        assert_eq!(error_code_ref(&response), "not_configured");
        assert!(error_message(response).contains("dKMS"));
    }

    #[test]
    fn release_routes_to_lit_backend_fail_closed_until_proxy() {
        let response = configured(KeyAuthorityBackend::Lit).release(key_release_request(), None);
        assert_eq!(error_code_ref(&response), "not_configured");
        assert!(error_message(response).contains("Lit"));
    }

    #[test]
    fn validation_precedes_backend_routing() {
        // Even with a backend selected, an unauthorized request must be rejected
        // as invalid *before* any backend is consulted — never reaching the
        // key-delivery path with a denied receipt.
        let mut request = key_release_request();
        request.rights_receipt.allowed = false;

        assert_eq!(
            error_code(configured(KeyAuthorityBackend::Reference).release(request, None)),
            "invalid_request"
        );
    }

    fn error_code_ref(response: &Response) -> &str {
        match response {
            Response::Error { code, .. } => code,
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[cfg(feature = "key-authority-ref")]
    fn error_message_ref(response: &Response) -> &str {
        match response {
            Response::Error { message, .. } => message,
            other => panic!("expected error, got {other:?}"),
        }
    }

    // --- reference key-authority seal engine (Phase A.2) --------------------

    #[cfg(feature = "key-authority-ref")]
    mod reference_backend {
        use super::*;
        use base64::Engine as _;

        fn b64() -> base64::engine::general_purpose::GeneralPurpose {
            base64::engine::general_purpose::STANDARD
        }

        fn unique_store_path(tag: &str) -> String {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            std::env::temp_dir()
                .join(format!("ddrm-authority-{tag}-{}-{nanos}.json", std::process::id()))
                .to_string_lossy()
                .into_owned()
        }

        /// Production-shaped authority: a durable key store makes the published verifying key
        /// AND the KEM recipient STABLE across separate `init`s (separate processes), so a CEK
        /// escrowed to the recipient at publish time still recovers after a relaunch — vs the
        /// dev default minting a fresh recipient every init.
        #[test]
        fn authority_key_store_yields_a_stable_recipient_across_inits() {
            let path = unique_store_path("stable");
            let cfg = json!({ "backend": "reference", "authority_key_store": path });

            let mut first = KeyProvider::default();
            let d1 = ok_data(first.init(cfg.clone()));
            let mut second = KeyProvider::default();
            let d2 = ok_data(second.init(cfg.clone()));

            assert_eq!(
                d1["seal_verifying_key_b64"], d2["seal_verifying_key_b64"],
                "the verifying key is stable across launches"
            );
            assert_eq!(
                d1["seal_recipient_pub_b64"], d2["seal_recipient_pub_b64"],
                "the escrow recipient is stable across launches (escrow-at-publish works)"
            );

            // A CEK escrowed to the FIRST launch's published recipient recovers on the SECOND.
            let recip_bytes = b64().decode(d1["seal_recipient_pub_b64"].as_str().unwrap()).unwrap();
            let recipient_public =
                ddrm_envelope::session_public_from_bytes(&recip_bytes).unwrap();
            let (producer_signer, producer_vk) = ddrm_envelope::seal::mldsa_seal_keypair([5u8; 32]);
            let cek: Vec<u8> = (0u8..16).collect();
            let kid = [0x42u8; 16];
            let aad =
                ddrm_envelope::transcript::escrow_aad(ddrm_envelope::SUITE_PQ_HYBRID, &kid, &recip_bytes);
            let wrapped =
                ddrm_envelope::seal::seal_bound(&recipient_public, &cek, &aad, &producer_signer).to_bytes();
            let recovered = second
                .reference
                .as_ref()
                .unwrap()
                .recover_escrowed_cek(&wrapped, ddrm_envelope::SUITE_PQ_HYBRID, &kid, &producer_vk)
                .expect("the relaunched authority recovers a CEK escrowed at publish time");
            assert_eq!(&recovered[..], &cek[..]);

            // Distinct from the dev default, which mints a fresh recipient each init.
            let mut ephemeral_a = KeyProvider::default();
            let ea = ok_data(ephemeral_a.init(json!({ "backend": "reference" })));
            let mut ephemeral_b = KeyProvider::default();
            let eb = ok_data(ephemeral_b.init(json!({ "backend": "reference" })));
            assert_ne!(
                ea["seal_recipient_pub_b64"], eb["seal_recipient_pub_b64"],
                "without a store the recipient is freshly minted per init"
            );

            let _ = std::fs::remove_file(&path);
        }

        /// Fail-closed: a present-but-corrupt key store must error, never silently re-mint a
        /// divergent authority (which would strand every CEK escrowed to the prior recipient).
        #[test]
        fn authority_key_store_fails_closed_on_a_corrupt_store() {
            let path = unique_store_path("corrupt");
            std::fs::write(&path, b"{ not valid json").unwrap();
            let mut provider = KeyProvider::default();
            let resp = provider.init(json!({ "backend": "reference", "authority_key_store": path }));
            assert_eq!(error_code_ref(&resp), "not_configured");
            let _ = std::fs::remove_file(&path);
        }

        /// Write a `dkms` PUBLIC-ONLY authority descriptor (Day 87–88): the node's published identity
        /// (verifying key + recipient) + its endpoint — and NOTHING secret. Returns the descriptor path.
        fn write_dkms_descriptor(tag: &str, master: &[u8; 32], endpoint: &str) -> String {
            // The node would derive + publish these; we derive them here only to FORGE a realistic
            // public descriptor for the client tests (the master itself is NEVER written to it).
            let authority = reference_authority_from_master(master);
            let desc = json!({
                "schema": DKMS_AUTHORITY_DESCRIPTOR_SCHEMA,
                "verifying_key_b64": b64().encode(&authority.verifying_key),
                "recipient_pub_b64": b64().encode(&authority.recipient_public),
                "authority_endpoint": endpoint,
            });
            let path = unique_store_path(tag);
            std::fs::write(&path, serde_json::to_vec(&desc).unwrap()).unwrap();
            path
        }

        /// The `dkms` backend RESOLVES the EXTERNAL node's PUBLIC identity + endpoint from a handed-in
        /// PUBLIC-ONLY descriptor (never minting, never holding a secret), and REPUBLISHES that
        /// identity on the rail (same vk + recipient the producer escrows to / the decrypt boundary
        /// trusts). The client holds the endpoint to DELEGATE recovery — but no key material. Mirrors
        /// PC2 holding only the external authority's public `pkpId`/`authority` (`recoverCEKEnvelope`).
        #[test]
        fn dkms_resolves_a_public_only_external_identity_from_a_descriptor() {
            let master = [0x7au8; 32];
            let path = write_dkms_descriptor("dkms-pub", &master, "/path/to/dkms-authority");
            let cfg = json!({ "backend": "dkms", "dkms_authority_descriptor": path });

            // Two separate inits resolve the SAME published identity from the SAME descriptor.
            let mut first = KeyProvider::default();
            let d1 = ok_data(first.init(cfg.clone()));
            let mut second = KeyProvider::default();
            let d2 = ok_data(second.init(cfg.clone()));
            assert_eq!(d1["seal_verifying_key_b64"], d2["seal_verifying_key_b64"]);
            assert_eq!(d1["seal_recipient_pub_b64"], d2["seal_recipient_pub_b64"]);

            // The client holds the PUBLIC identity + endpoint, and NO recovery secret: `reference`
            // (which owns a recipient_secret + signer) is never populated for `dkms`.
            assert!(second.dkms.is_some(), "dkms resolved a public client from the descriptor");
            assert!(
                second.reference.is_none(),
                "the dkms client must hold NO recovery secret (no ReferenceAuthority)"
            );
            // The republished identity matches the descriptor pins exactly (no derivation here).
            let client = second.dkms.as_ref().unwrap();
            assert_eq!(d2["seal_verifying_key_b64"].as_str().unwrap(), client.verifying_key_b64);
            assert_eq!(d2["seal_recipient_pub_b64"].as_str().unwrap(), client.recipient_pub_b64);
            assert_eq!(client.endpoint, "/path/to/dkms-authority");
            let _ = std::fs::remove_file(&path);
        }

        /// HARD BOUNDARY (Day 87–88): a descriptor that carries a master seed is REJECTED — the
        /// recovery secret must NEVER reach the runtime, even if a misconfigured provisioner leaks it.
        #[test]
        fn dkms_fails_closed_on_a_secret_bearing_descriptor() {
            let master = [0x21u8; 32];
            let authority = reference_authority_from_master(&master);
            let path = unique_store_path("dkms-secret");
            // A public descriptor that ALSO (wrongly) carries the master seed.
            let desc = json!({
                "schema": DKMS_AUTHORITY_DESCRIPTOR_SCHEMA,
                "verifying_key_b64": b64().encode(&authority.verifying_key),
                "recipient_pub_b64": b64().encode(&authority.recipient_public),
                "authority_endpoint": "/path/to/dkms-authority",
                "authority_master_seed_b64": b64().encode(master), // MUST be rejected
            });
            std::fs::write(&path, serde_json::to_vec(&desc).unwrap()).unwrap();
            let mut p = KeyProvider::default();
            let resp = p.init(json!({ "backend": "dkms", "dkms_authority_descriptor": path }));
            assert_eq!(error_code_ref(&resp), "not_configured");
            assert!(
                error_message_ref(&resp).contains("master seed"),
                "the rejection must name the leaked secret"
            );
            assert!(p.dkms.is_none());
            let _ = std::fs::remove_file(&path);
        }

        /// Fail-closed: a selected `dkms` backend with NO descriptor stays unconfigured (the
        /// "no dKMS node provisioned" surface); a present-but-bad descriptor (corrupt JSON, wrong
        /// schema) fails the init — never a silent or divergent authority.
        #[test]
        fn dkms_fails_closed_on_a_missing_or_bad_descriptor() {
            // No descriptor -> selected but unconfigured -> release fails closed with "no dKMS node".
            let mut bare = KeyProvider::default();
            assert!(matches!(bare.init(json!({ "backend": "dkms" })), Response::Ok { .. }));
            assert!(bare.dkms.is_none());
            assert_eq!(
                error_code_ref(&bare.release(key_release_request(), None)),
                "not_configured"
            );

            // Corrupt JSON.
            let corrupt = unique_store_path("dkms-corrupt");
            std::fs::write(&corrupt, b"{ not json").unwrap();
            let mut p = KeyProvider::default();
            assert_eq!(
                error_code_ref(&p.init(json!({ "backend": "dkms", "dkms_authority_descriptor": corrupt }))),
                "not_configured"
            );
            let _ = std::fs::remove_file(&corrupt);
        }

        /// A real external authority ALWAYS publishes its identity AND its endpoint, so a descriptor
        /// missing any of `verifying_key_b64` / `recipient_pub_b64` / `authority_endpoint` is rejected —
        /// the runtime never half-resolves an external authority it cannot trust or reach.
        #[test]
        fn dkms_fails_closed_on_an_incomplete_public_descriptor() {
            let master = [0x33u8; 32];
            let authority = reference_authority_from_master(&master);
            let vk = b64().encode(&authority.verifying_key);
            let recip = b64().encode(&authority.recipient_public);

            // No pins / no endpoint at all.
            for desc in [
                json!({ "schema": DKMS_AUTHORITY_DESCRIPTOR_SCHEMA }),
                // vk only (missing recipient + endpoint).
                json!({ "schema": DKMS_AUTHORITY_DESCRIPTOR_SCHEMA, "verifying_key_b64": vk }),
                // pins present but NO endpoint (cannot delegate recovery).
                json!({ "schema": DKMS_AUTHORITY_DESCRIPTOR_SCHEMA, "verifying_key_b64": vk, "recipient_pub_b64": recip }),
            ] {
                let path = unique_store_path("dkms-incomplete");
                std::fs::write(&path, serde_json::to_vec(&desc).unwrap()).unwrap();
                let mut p = KeyProvider::default();
                assert_eq!(
                    error_code_ref(&p.init(json!({ "backend": "dkms", "dkms_authority_descriptor": path }))),
                    "not_configured"
                );
                let _ = std::fs::remove_file(&path);
            }
        }

        fn reference_provider() -> KeyProvider {
            let mut provider = KeyProvider::default();
            // init must succeed and stand up the reference authority.
            assert!(matches!(
                provider.init(json!({ "backend": "reference" })),
                Response::Ok { .. }
            ));
            assert!(provider.reference.is_some());
            provider
        }

        /// The reference authority publishes its ML-DSA-65 verifying key at `init`, so
        /// the decrypt boundary can be configured to trust it BEFORE minting a session
        /// (breaks the rail bootstrap ordering). The published vk is the SAME one the
        /// seal is verified against, and it builds a real verifier.
        #[test]
        fn reference_init_publishes_the_seal_verifying_key() {
            let b64 = b64();
            let mut provider = KeyProvider::default();
            let resp = provider.init(json!({ "backend": "reference" }));
            let data = ok_data(resp);
            let vk_b64 = data["seal_verifying_key_b64"]
                .as_str()
                .expect("reference init publishes the verifying key");
            let vk = b64.decode(vk_b64).expect("vk is valid base64");
            assert!(
                ddrm_envelope::MlDsa65Verifier::from_encoded(&vk).is_some(),
                "the published vk must build a real verifier"
            );
            // A non-reference backend publishes no seal key.
            let mut other = KeyProvider::default();
            let other_data = ok_data(other.init(json!({ "backend": "lit" })));
            assert!(other_data.get("seal_verifying_key_b64").is_none());
        }

        /// Phase C escrow destination: the authority publishes a KEM RECIPIENT key
        /// (distinct from its ML-DSA verifying key), and recovers a CEK the producer
        /// escrowed to it under the SHARED escrow AAD. Wrong KID or a forged producer
        /// fail closed — proving the producer→authority half of the fresh-CEK path.
        #[test]
        fn reference_recovers_a_cek_escrowed_to_its_recipient_key() {
            let b64 = b64();
            let mut provider = KeyProvider::default();
            let data = ok_data(provider.init(json!({ "backend": "reference" })));

            // (1) recipient key published, distinct from the verifying key.
            let recip_b64 = data["seal_recipient_pub_b64"]
                .as_str()
                .expect("recipient pub published");
            assert_ne!(
                recip_b64,
                data["seal_verifying_key_b64"].as_str().unwrap(),
                "recipient (KEM) key is distinct from the (ML-DSA) verifying key"
            );
            let recip_bytes = b64.decode(recip_b64).expect("recipient b64");
            let recipient_public = ddrm_envelope::session_public_from_bytes(&recip_bytes)
                .expect("recipient pub parses");

            // (2) a producer mints a CEK + KID and escrows it to that recipient under
            // the shared escrow AAD, signed by the producer's ML-DSA key.
            let (producer_signer, producer_vk) =
                ddrm_envelope::seal::mldsa_seal_keypair([7u8; 32]);
            let cek: Vec<u8> = (0u8..16).collect();
            let kid = [0xABu8; 16];
            let aad = ddrm_envelope::transcript::escrow_aad(
                ddrm_envelope::SUITE_PQ_HYBRID,
                &kid,
                &recip_bytes,
            );
            let env =
                ddrm_envelope::seal::seal_bound(&recipient_public, &cek, &aad, &producer_signer);
            let wrapped = env.to_bytes();

            // (3) the authority recovers the EXACT CEK.
            let authority = provider.reference.as_ref().unwrap();
            let recovered = authority
                .recover_escrowed_cek(
                    &wrapped,
                    ddrm_envelope::SUITE_PQ_HYBRID,
                    &kid,
                    &producer_vk,
                )
                .expect("authority recovers the escrowed CEK");
            assert_eq!(&recovered[..], &cek[..], "recovered CEK matches the escrowed CEK");

            // (4) wrong KID fails closed (AAD mismatch at the GCM tag).
            let mut bad_kid = kid;
            bad_kid[0] ^= 1;
            assert!(
                authority
                    .recover_escrowed_cek(
                        &wrapped,
                        ddrm_envelope::SUITE_PQ_HYBRID,
                        &bad_kid,
                        &producer_vk,
                    )
                    .is_err(),
                "a KID-swap must fail closed"
            );

            // (5) a forged producer (different signer) fails closed at the signature.
            let (_other_signer, other_vk) = ddrm_envelope::seal::mldsa_seal_keypair([9u8; 32]);
            assert!(
                authority
                    .recover_escrowed_cek(
                        &wrapped,
                        ddrm_envelope::SUITE_PQ_HYBRID,
                        &kid,
                        &other_vk,
                    )
                    .is_err(),
                "a forged producer signature must fail closed"
            );
        }

        /// Day 60 wire op: `release_from_escrow_ref` recovers a producer-escrowed CEK
        /// and re-seals it to the decrypt session — closing producer→authority→decrypt
        /// with no raw CEK on any wire. A foreign/tampered escrow blob fails closed.
        #[test]
        fn release_from_escrow_re_seals_to_the_session_and_fails_closed_on_tamper() {
            let b64 = b64();
            let mut provider = KeyProvider::default();
            let data = ok_data(provider.init(json!({ "backend": "reference" })));
            let recip_b64 = data["seal_recipient_pub_b64"].as_str().unwrap();
            let recip_bytes = b64.decode(recip_b64).unwrap();
            let recipient_public =
                ddrm_envelope::session_public_from_bytes(&recip_bytes).unwrap();

            // Producer mints a CEK + KID and escrows it to the authority's recipient.
            let (producer_signer, producer_vk) =
                ddrm_envelope::seal::mldsa_seal_keypair([3u8; 32]);
            let cek: Vec<u8> = (10u8..26).collect();
            let kid = [0x5Cu8; 16];
            let kid_hex: String = kid.iter().map(|b| format!("{b:02x}")).collect();
            let escrow_aad = ddrm_envelope::transcript::escrow_aad(
                ddrm_envelope::SUITE_PQ_HYBRID,
                &kid,
                &recip_bytes,
            );
            let wrapped = ddrm_envelope::seal::seal_bound(
                &recipient_public,
                &cek,
                &escrow_aad,
                &producer_signer,
            )
            .to_bytes();
            let wrapped_b64 = b64.encode(&wrapped);

            // Decrypt session + transcript AAD the authority re-seals to.
            let (session_secret, session_public) = ddrm_envelope::mint_session();
            let session_pub_b64 = b64.encode(ddrm_envelope::session_public_bytes(&session_public));
            let transcript = b"transcript:producer-smoke";

            let resp = provider.release_from_escrow_ref(
                key_release_request(),
                &session_pub_b64,
                &wrapped_b64,
                &b64.encode(&producer_vk),
                &kid_hex,
                ddrm_envelope::SUITE_PQ_HYBRID,
                &b64.encode(transcript),
                b64.encode(b"ciphertext"),
                b64.encode(b"content-hash"),
                b64.encode(b"nonce"),
                None,
            );
            let out = ok_data(resp);

            // The re-sealed material opens with the SAME unwrap the decrypt boundary
            // uses, yielding the EXACT CEK the producer escrowed — no raw CEK anywhere.
            let sealed = b64
                .decode(out["material"]["sealed_cek_b64"].as_str().unwrap())
                .unwrap();
            let envelope = ddrm_envelope::PqSealedEnvelope::from_bytes(&sealed).unwrap();
            let vk = b64
                .decode(out["seal_verifying_key_b64"].as_str().unwrap())
                .unwrap();
            let verifier = ddrm_envelope::MlDsa65Verifier::from_encoded(&vk).unwrap();
            let recovered =
                ddrm_envelope::hybrid_unwrap_bound(&session_secret, &envelope, transcript, &verifier)
                    .unwrap();
            assert_eq!(recovered.as_slice(), cek.as_slice());
            assert!(!serde_json::to_string(&out).unwrap().contains(&b64.encode(&cek)));

            // A tampered escrow blob fails closed (no material).
            let mut bad = wrapped.clone();
            *bad.last_mut().unwrap() ^= 1;
            assert_eq!(
                error_code(provider.release_from_escrow_ref(
                    key_release_request(),
                    &session_pub_b64,
                    &b64.encode(&bad),
                    &b64.encode(&producer_vk),
                    &kid_hex,
                    ddrm_envelope::SUITE_PQ_HYBRID,
                    &b64.encode(transcript),
                    b64.encode(b"ciphertext"),
                    b64.encode(b"content-hash"),
                    b64.encode(b"nonce"),
                    None,
                )),
                "invalid_request",
                "a tampered escrow blob must fail closed"
            );

            // A foreign producer vk fails closed at the signature.
            let (_other, other_vk) = ddrm_envelope::seal::mldsa_seal_keypair([8u8; 32]);
            assert_eq!(
                error_code(provider.release_from_escrow_ref(
                    key_release_request(),
                    &session_pub_b64,
                    &wrapped_b64,
                    &b64.encode(&other_vk),
                    &kid_hex,
                    ddrm_envelope::SUITE_PQ_HYBRID,
                    &b64.encode(transcript),
                    b64.encode(b"ciphertext"),
                    b64.encode(b"content-hash"),
                    b64.encode(b"nonce"),
                    None,
                )),
                "invalid_request",
                "a foreign producer vk must fail closed"
            );
        }

        // --- canonical `release` op, reference backend (Day 70) -------------------
        //
        // The op `drm-provider`'s DrmOpenPlanV1 names for the key step now ACTUALLY
        // releases: it recovers the producer-escrowed CEK from the rights-bound
        // key_envelope and re-seals it to the runtime-injected decrypt session. No raw
        // CEK shim — the CEK reaches the authority SEALED and leaves SEALED.

        /// A full escrow scenario: a producer escrows `cek` (bound to `kid`) to the
        /// reference authority's recipient key, and we assemble the rights-bound request
        /// (escrow blob inside the key_envelope) + the per-session material.
        struct Scenario {
            provider: KeyProvider,
            request: KeyReleaseRequestV1,
            producer_vk: Vec<u8>,
            session_secret: ddrm_envelope::SessionKemSecret,
            session_pub_b64: String,
            transcript: Vec<u8>,
            cek: Vec<u8>,
        }

        fn escrow_scenario() -> Scenario {
            let b64 = b64();
            let mut provider = KeyProvider::default();
            let data = ok_data(provider.init(json!({ "backend": "reference" })));
            let recip_bytes = b64
                .decode(data["seal_recipient_pub_b64"].as_str().unwrap())
                .unwrap();
            let recipient_public =
                ddrm_envelope::session_public_from_bytes(&recip_bytes).unwrap();

            let (producer_signer, producer_vk) =
                ddrm_envelope::seal::mldsa_seal_keypair([3u8; 32]);
            let cek: Vec<u8> = (10u8..26).collect();
            let kid = [0x5Cu8; 16];
            let kid_hex: String = kid.iter().map(|b| format!("{b:02x}")).collect();
            let escrow_aad =
                ddrm_envelope::transcript::escrow_aad(SUITE_PQ_HYBRID, &kid, &recip_bytes);
            let wrapped = ddrm_envelope::seal::seal_bound(
                &recipient_public,
                &cek,
                &escrow_aad,
                &producer_signer,
            )
            .to_bytes();

            let (session_secret, session_public) = ddrm_envelope::mint_session();
            let session_pub_b64 =
                b64.encode(ddrm_envelope::session_public_bytes(&session_public));

            // The escrow blob rides INSIDE the rights-bound key_envelope (not side-band).
            let mut request = key_release_request();
            request.key_envelope.wrapped_cek = b64.encode(&wrapped);
            request.key_envelope.kid = kid_hex;
            // key_envelope.scheme is already SUPPORTED_SCHEMES[0] == SUITE_PQ_HYBRID.

            Scenario {
                provider,
                request,
                producer_vk,
                session_secret,
                session_pub_b64,
                transcript: b"transcript:canonical-release".to_vec(),
                cek,
            }
        }

        fn session_ctx(
            s: &Scenario,
            producer_vk: &[u8],
            now_unix: Option<u64>,
        ) -> Option<ReleaseSessionContext> {
            let b64 = b64();
            let mut v = json!({
                "decrypt_session_pub_b64": s.session_pub_b64,
                "producer_vk_b64": b64.encode(producer_vk),
                "aad_b64": b64.encode(&s.transcript),
                "ciphertext_b64": b64.encode(b"ciphertext"),
                "content_hash_b64": b64.encode(b"content-hash"),
                "nonce_b64": b64.encode(b"nonce"),
            });
            if let Some(now) = now_unix {
                v["now_unix"] = json!(now);
            }
            Some(serde_json::from_value(v).expect("session context parses"))
        }

        /// Grant → release: the canonical `release` recovers the escrowed CEK and seals it
        /// to the session; the SAME unwrap the decrypt boundary uses opens it to the EXACT
        /// producer CEK — with no raw CEK on the wire. The consumer half now runs without
        /// the dev raw-CEK shim.
        #[test]
        fn canonical_release_recovers_escrow_and_seals_to_session() {
            let b64 = b64();
            let s = escrow_scenario();
            let session = session_ctx(&s, &s.producer_vk, Some(1_850_000_000));
            let out = ok_data(s.provider.release(s.request.clone(), session));

            let sealed = b64
                .decode(out["material"]["sealed_cek_b64"].as_str().unwrap())
                .unwrap();
            let envelope = ddrm_envelope::PqSealedEnvelope::from_bytes(&sealed).unwrap();
            let vk = b64
                .decode(out["seal_verifying_key_b64"].as_str().unwrap())
                .unwrap();
            let verifier = ddrm_envelope::MlDsa65Verifier::from_encoded(&vk).unwrap();
            let recovered = ddrm_envelope::hybrid_unwrap_bound(
                &s.session_secret,
                &envelope,
                &s.transcript,
                &verifier,
            )
            .unwrap();
            assert_eq!(
                recovered.as_slice(),
                s.cek.as_slice(),
                "the canonical release delivers the producer's CEK to the session"
            );
            assert_eq!(out["suite"], json!(ddrm_envelope::SUITE_PQ_HYBRID));
            assert!(
                !serde_json::to_string(&out).unwrap().contains(&b64.encode(&s.cek)),
                "no raw CEK may appear in the release response"
            );
        }

        /// A denied rights receipt is rejected BEFORE the escrow is ever touched
        /// (validation precedes recover/seal) — the key boundary never releases on it.
        #[test]
        fn canonical_release_fails_closed_on_denied_receipt() {
            let mut s = escrow_scenario();
            s.request.rights_receipt.allowed = false;
            let session = session_ctx(&s, &s.producer_vk, None);
            assert_eq!(error_code(s.provider.release(s.request, session)), "invalid_request");
        }

        /// An already-expired request is refused when the runtime supplies a clock — the
        /// authority never seals a CEK past its window.
        #[test]
        fn canonical_release_fails_closed_when_expired() {
            let s = escrow_scenario();
            let expires_at = s.request.expires_at;
            let session = session_ctx(&s, &s.producer_vk, Some(expires_at)); // now == expiry -> expired
            assert_eq!(error_code(s.provider.release(s.request, session)), "invalid_request");
        }

        /// A KID-swap (the request's key_envelope.kid no longer matches the escrow's bound
        /// KID) fails closed at the AAD recompute — the wrong content's escrow can't open.
        #[test]
        fn canonical_release_fails_closed_on_kid_swap() {
            let mut s = escrow_scenario();
            s.request.key_envelope.kid = "ab".repeat(16); // a different, valid 32-hex KID
            let session = session_ctx(&s, &s.producer_vk, None);
            assert_eq!(error_code(s.provider.release(s.request, session)), "invalid_request");
        }

        /// A forged producer (the session vk doesn't match the escrow's signer) fails
        /// closed at the signature verification inside recover.
        #[test]
        fn canonical_release_fails_closed_on_forged_producer() {
            let s = escrow_scenario();
            let (_other, other_vk) = ddrm_envelope::seal::mldsa_seal_keypair([8u8; 32]);
            let session = session_ctx(&s, &other_vk, None);
            assert_eq!(error_code(s.provider.release(s.request, session)), "invalid_request");
        }

        /// Without the runtime-injected session context, the canonical `release` fails
        /// closed — the rights-bound request alone is not enough to seal to a session.
        #[test]
        fn canonical_release_requires_session_context() {
            let s = escrow_scenario();
            assert_eq!(error_code(s.provider.release(s.request, None)), "not_configured");
        }

        #[test]
        fn reference_seal_round_trips_through_the_decrypt_unwrap() {
            let b64 = b64();
            let (session_secret, session_public) = ddrm_envelope::mint_session();
            let pub_b64 = b64.encode(ddrm_envelope::session_public_bytes(&session_public));
            let cek: Vec<u8> = (0u8..16).collect();
            let aad = b"transcript:principal=alice;session=s1;object=cid1";

            let response = reference_provider().release_ref(
                key_release_request(),
                &pub_b64,
                &b64.encode(&cek),
                &b64.encode(aad),
                b64.encode(b"ciphertext"),
                b64.encode(b"content-hash"),
                b64.encode(b"nonce"),
                None,
            );
            let data = ok_data(response);

            // Material is the exact suite-tagged shape the decrypt boundary opens.
            let material = &data["material"];
            assert_eq!(material["suite"], ddrm_envelope::SUITE_PQ_HYBRID);
            assert!(material["sealed_cek_b64"].is_string());
            assert_eq!(material["ciphertext_b64"], b64.encode(b"ciphertext"));

            // The sealed material the reference authority produced is opened by the
            // SAME unwrap the decrypt boundary uses — the key->decrypt handoff is
            // wire-compatible end to end, with no raw CEK on the wire.
            let sealed = b64
                .decode(material["sealed_cek_b64"].as_str().unwrap())
                .unwrap();
            let envelope = ddrm_envelope::PqSealedEnvelope::from_bytes(&sealed).unwrap();
            let vk = b64
                .decode(data["seal_verifying_key_b64"].as_str().unwrap())
                .unwrap();
            let verifier = ddrm_envelope::MlDsa65Verifier::from_encoded(&vk).unwrap();
            let recovered =
                ddrm_envelope::hybrid_unwrap_bound(&session_secret, &envelope, aad, &verifier)
                    .unwrap();
            assert_eq!(recovered.as_slice(), cek.as_slice());

            // The raw CEK appears nowhere in the response.
            let serialized = serde_json::to_string(&data).unwrap();
            assert!(!serialized.contains(&b64.encode(&cek)));
        }

        #[test]
        fn reference_seal_binds_the_transcript() {
            let b64 = b64();
            let (session_secret, session_public) = ddrm_envelope::mint_session();
            let pub_b64 = b64.encode(ddrm_envelope::session_public_bytes(&session_public));
            let cek: Vec<u8> = (0u8..16).collect();

            let data = ok_data(reference_provider().release_ref(
                key_release_request(),
                &pub_b64,
                &b64.encode(&cek),
                &b64.encode(b"transcript-A"),
                b64.encode(b"c"),
                b64.encode(b"h"),
                b64.encode(b"n"),
                None,
            ));
            let sealed = b64
                .decode(data["material"]["sealed_cek_b64"].as_str().unwrap())
                .unwrap();
            let envelope = ddrm_envelope::PqSealedEnvelope::from_bytes(&sealed).unwrap();
            let vk = b64
                .decode(data["seal_verifying_key_b64"].as_str().unwrap())
                .unwrap();
            let verifier = ddrm_envelope::MlDsa65Verifier::from_encoded(&vk).unwrap();

            // A CEK sealed for transcript-A cannot be opened under a different one.
            assert!(ddrm_envelope::hybrid_unwrap_bound(
                &session_secret,
                &envelope,
                b"transcript-B",
                &verifier
            )
            .is_err());
        }

        #[test]
        fn reference_seal_fails_closed_on_malformed_session_pub() {
            let b64 = b64();
            let response = reference_provider().release_ref(
                key_release_request(),
                "!!! not base64 !!!",
                &b64.encode([0u8; 16]),
                "",
                b64.encode(b"c"),
                b64.encode(b"h"),
                b64.encode(b"n"),
                None,
            );
            assert_eq!(error_code(response), "invalid_request");
        }

        #[test]
        fn release_ref_requires_the_reference_backend() {
            let b64 = b64();
            let (_secret, public) = ddrm_envelope::mint_session();
            // Configure a different backend; the reference seal op must fail closed.
            let mut provider = KeyProvider::default();
            provider.init(json!({ "backend": "lit" }));

            let response = provider.release_ref(
                key_release_request(),
                &b64.encode(ddrm_envelope::session_public_bytes(&public)),
                &b64.encode([0u8; 16]),
                "",
                b64.encode(b"c"),
                b64.encode(b"h"),
                b64.encode(b"n"),
                None,
            );
            assert_eq!(error_code(response), "not_configured");
        }

        #[test]
        fn release_ref_validation_precedes_seal() {
            let b64 = b64();
            let (_secret, public) = ddrm_envelope::mint_session();
            let mut request = key_release_request();
            request.rights_receipt.allowed = false;

            let response = reference_provider().release_ref(
                request,
                &b64.encode(ddrm_envelope::session_public_bytes(&public)),
                &b64.encode([0u8; 16]),
                "",
                b64.encode(b"c"),
                b64.encode(b"h"),
                b64.encode(b"n"),
                None,
            );
            assert_eq!(error_code(response), "invalid_request");
        }

        /// Orchestration handoff at the transcript level. The key authority builds the
        /// CANONICAL shared `DecryptTranscriptV1` (the same field set + encoder the
        /// decrypt boundary uses), computes `to_aad()`, and seals the CEK to it.
        /// Sealing to the SHARED encoder — not an opaque blob — is precisely what lets
        /// this SEPARATE capsule produce material the decrypt boundary opens: the
        /// boundary rebuilds the identical transcript and the CEK unwraps; any field
        /// change (a replayed nonce here) yields a different AAD and fails closed.
        #[test]
        fn reference_seal_binds_the_shared_decrypt_transcript() {
            let b64 = b64();
            let (session_secret, session_public) = ddrm_envelope::mint_session();
            let pub_bytes = ddrm_envelope::session_public_bytes(&session_public);
            let cek: Vec<u8> = (0u8..16).collect();

            let transcript = ddrm_envelope::transcript::DecryptTranscriptV1 {
                suite_id: ddrm_envelope::SUITE_PQ_HYBRID,
                provider_id: "decrypt-provider",
                principal_id: "did:elastos:alice",
                session_id: "sess-1",
                object_cid: "bafyobject",
                content_hash: b"content-hash",
                action: "decrypt",
                viewer_interface: "video",
                output_kind: "frames",
                expires_at: 1_900_000_000,
                release_receipt_hash: [7u8; 32],
                decrypt_session_pub: &pub_bytes,
                nonce: b"replay-nonce-1",
            };
            let aad = transcript.to_aad();

            let data = ok_data(reference_provider().release_ref(
                key_release_request(),
                &b64.encode(&pub_bytes),
                &b64.encode(&cek),
                &b64.encode(&aad),
                b64.encode(b"ciphertext"),
                b64.encode(b"content-hash"),
                b64.encode(b"nonce"),
                None,
            ));

            let sealed = b64
                .decode(data["material"]["sealed_cek_b64"].as_str().unwrap())
                .unwrap();
            let envelope = ddrm_envelope::PqSealedEnvelope::from_bytes(&sealed).unwrap();
            let vk = b64
                .decode(data["seal_verifying_key_b64"].as_str().unwrap())
                .unwrap();
            let verifier = ddrm_envelope::MlDsa65Verifier::from_encoded(&vk).unwrap();

            // The decrypt boundary rebuilds the IDENTICAL shared transcript -> opens.
            let recovered =
                ddrm_envelope::hybrid_unwrap_bound(&session_secret, &envelope, &aad, &verifier)
                    .expect("matching shared transcript opens");
            assert_eq!(recovered.as_slice(), cek.as_slice());

            // A replayed/altered transcript field -> different AAD -> fail closed.
            let mut replayed = transcript;
            replayed.nonce = b"replay-nonce-2";
            assert!(
                ddrm_envelope::hybrid_unwrap_bound(
                    &session_secret,
                    &envelope,
                    &replayed.to_aad(),
                    &verifier
                )
                .is_err(),
                "a replayed/altered transcript must fail closed across the capsule boundary"
            );
        }
    }
}
