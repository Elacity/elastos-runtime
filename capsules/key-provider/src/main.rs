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

#[derive(Debug, Default)]
struct KeyProvider {
    /// Active key-delivery backend, selected by operator/runtime config at `init`.
    /// `None` = no authority configured = `release` fails closed.
    backend: Option<KeyAuthorityBackend>,
}

impl KeyProvider {
    fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::Init { config } => self.init(config),
            Request::Status => self.status(),
            Request::Release { request } => self.release(*request),
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

        Response::ok(json!({
            "provider": "key",
            "protocol_version": "1.0",
            "configured": false,
            "active_backend": self.backend.map(KeyAuthorityBackend::tag),
            "supported_operations": ["status", "release"],
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
}
