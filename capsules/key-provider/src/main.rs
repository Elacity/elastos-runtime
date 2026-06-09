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
        /// + signed payload). Empty = unbound. The full `DecryptTranscriptV1`
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
    Shutdown,
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
}

impl KeyProvider {
    fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::Init { config } => self.init(config),
            Request::Status => self.status(),
            Request::Release { request } => self.release(*request),
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

        // Stand up the reference seal authority when that backend is selected.
        #[cfg(feature = "key-authority-ref")]
        {
            self.reference = match self.backend {
                Some(KeyAuthorityBackend::Reference) => {
                    let (signer, verifying_key) =
                        ddrm_envelope::seal::mldsa_seal_keypair(ref_seal_seed(&config));
                    let (recipient_secret, recipient_public) = ddrm_envelope::mint_session();
                    Some(ReferenceAuthority {
                        signer,
                        verifying_key,
                        recipient_public: ddrm_envelope::session_public_bytes(&recipient_public),
                        recipient_secret,
                    })
                }
                _ => None,
            };
        }

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

        // Seal — the CEK leaves this boundary only as sealed material.
        let envelope =
            ddrm_envelope::seal::seal_bound(&public, cek.as_slice(), &aad, &authority.signer);
        let sealed_cek_b64 = b64.encode(envelope.to_bytes());

        let mut material = json!({
            "suite": ddrm_envelope::SUITE_PQ_HYBRID,
            "sealed_cek_b64": sealed_cek_b64,
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
            // The vk the decrypt boundary must be configured to trust for this seal.
            "seal_verifying_key_b64": b64.encode(&authority.verifying_key),
        }))
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

    fn release(&self, request: KeyReleaseRequestV1) -> Response {
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
            Some(backend) => self.release_via_backend(backend, &request),
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
            error_code(provider.release(key_release_request())),
            "not_configured"
        );
    }

    #[test]
    fn release_rejects_unsupported_scheme() {
        let provider = KeyProvider::default();
        let mut request = key_release_request();
        request.key_envelope.scheme = "frost-only".to_string();

        assert_eq!(error_code(provider.release(request)), "invalid_request");
    }

    #[test]
    fn release_rejects_weak_cipher() {
        let provider = KeyProvider::default();
        let mut request = key_release_request();
        request.key_envelope.algorithms.cipher = "aes-128-gcm".to_string();

        assert_eq!(error_code(provider.release(request)), "invalid_request");
    }

    #[test]
    fn release_rejects_missing_pq_hybrid_kem() {
        let provider = KeyProvider::default();
        let mut request = key_release_request();
        request.key_envelope.algorithms.kem = vec!["x25519".to_string()];

        assert_eq!(error_code(provider.release(request)), "invalid_request");
    }

    #[test]
    fn release_rejects_denied_rights_receipt() {
        let provider = KeyProvider::default();
        let mut request = key_release_request();
        request.rights_receipt.allowed = false;

        assert_eq!(error_code(provider.release(request)), "invalid_request");
    }

    #[test]
    fn release_rejects_rights_receipt_bound_to_other_principal() {
        let provider = KeyProvider::default();
        let mut request = key_release_request();
        request.rights_receipt.principal_id = "person:local:attacker".to_string();

        assert_eq!(error_code(provider.release(request)), "invalid_request");
    }

    #[test]
    fn release_rejects_rights_receipt_for_other_object() {
        let provider = KeyProvider::default();
        let mut request = key_release_request();
        request.rights_receipt.content_id = "bafybeigsomethingelse".to_string();

        assert_eq!(error_code(provider.release(request)), "invalid_request");
    }

    #[test]
    fn release_rejects_rights_receipt_for_other_action() {
        let provider = KeyProvider::default();
        let mut request = key_release_request();
        request.action = "download".to_string();
        // receipt still authorizes only "view"

        assert_eq!(error_code(provider.release(request)), "invalid_request");
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
    fn release_routes_to_reference_backend_fail_closed_until_seal_engine() {
        let response = configured(KeyAuthorityBackend::Reference).release(key_release_request());
        assert_eq!(error_code_ref(&response), "not_configured");
        assert!(error_message(response).contains("reference"));
    }

    #[test]
    fn release_routes_to_dkms_backend_fail_closed_until_node() {
        let response = configured(KeyAuthorityBackend::Dkms).release(key_release_request());
        assert_eq!(error_code_ref(&response), "not_configured");
        assert!(error_message(response).contains("dKMS"));
    }

    #[test]
    fn release_routes_to_lit_backend_fail_closed_until_proxy() {
        let response = configured(KeyAuthorityBackend::Lit).release(key_release_request());
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
            error_code(configured(KeyAuthorityBackend::Reference).release(request)),
            "invalid_request"
        );
    }

    fn error_code_ref(response: &Response) -> &str {
        match response {
            Response::Error { code, .. } => code,
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
