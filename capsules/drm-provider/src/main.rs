//! ElastOS DRM Provider Capsule
//!
//! Fail-closed protected-content boundary. App capsules never receive raw CEKs,
//! key-backend SDK objects, wallet authority, chain RPC, Kubo/IPFS APIs, or
//! Elacity SDK access through this provider.

use elastos_common::protected_content::{
    validate_protected_content_key_envelope_algorithms, SealedObjectV1, PROTECTED_CONTENT_ACTIONS,
    SEALED_OBJECT_SCHEMA,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

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
    Open {
        request: Box<DrmOpenRequest>,
    },
    Shutdown,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DrmOpenRequest {
    object: SealedObjectV1,
    principal_id: String,
    session_id: String,
    action: String,
    reason: String,
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
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<Value>,
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
            details: None,
        }
    }

    fn error_with_details(code: &str, message: impl Into<String>, details: Value) -> Self {
        Response::Error {
            code: code.to_string(),
            message: message.into(),
            details: Some(details),
        }
    }
}

#[derive(Debug, Default)]
struct DrmProvider;

impl DrmProvider {
    fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::Init { config } => self.init(config),
            Request::Status => self.status(),
            Request::Open { request } => self.open(*request),
            Request::Shutdown => Response::empty_ok(),
        }
    }

    fn init(&mut self, _config: Value) -> Response {
        Response::ok(json!({
            "provider": "drm",
            "protocol_version": "1.0",
            "configured": false,
            "supported_operations": ["status", "open"],
        }))
    }

    fn status(&self) -> Response {
        Response::ok(json!({
            "provider": "drm",
            "version": PROVIDER_VERSION,
            "configured": false,
            "supported_operations": ["status", "open"],
            "supported_actions": PROTECTED_CONTENT_ACTIONS,
            "required_sequence": drm_open_sequence(),
            "required_runtime_events": drm_required_runtime_events(),
            "blocked_authority": [
                "raw_cek",
                "key_backend_sdk",
                "wallet_rpc",
                "chain_rpc",
                "kubo_api",
                "elacity_sdk"
            ],
            "next_required_providers": drm_next_required_providers(),
        }))
    }

    fn open(&self, request: DrmOpenRequest) -> Response {
        if let Err(err) = validate_open_request(&request) {
            return Response::error("invalid_request", err);
        }
        Response::error_with_details(
            "not_configured",
            "DRM open requires the declared content, rights, key, decrypt, receipt, and audit sequence",
            drm_open_blocked_details(),
        )
    }
}

fn drm_open_sequence() -> Value {
    json!([
        {
            "step": "content_status",
            "provider": "content",
            "operation": "status",
            "resource": "elastos://content/status"
        },
        {
            "step": "content_fetch",
            "provider": "content",
            "operation": "fetch",
            "resource": "elastos://content/fetch"
        },
        {
            "step": "rights_check",
            "provider": "rights",
            "operation": "has_access_by_content_id",
            "resource": "elastos://rights/access/has_access_by_content_id"
        },
        {
            "step": "key_release",
            "provider": "key",
            "operation": "release",
            "resource": "elastos://key/release"
        },
        {
            "step": "decrypt_session",
            "provider": "decrypt",
            "operation": "open_session",
            "resource": "elastos://decrypt/session/open"
        },
        {
            "step": "render",
            "provider": "decrypt",
            "operation": "render",
            "resource": "elastos://decrypt/render"
        },
        {
            "step": "release_receipt",
            "owner": "runtime",
            "event": "release_receipt"
        },
        {
            "step": "audit",
            "owner": "runtime",
            "event": "protected_content.open.audit"
        }
    ])
}

fn drm_required_runtime_events() -> Value {
    json!(["release_receipt", "protected_content.open.audit"])
}

fn drm_next_required_providers() -> Value {
    json!(["rights-provider", "key-provider", "decrypt-provider"])
}

fn drm_open_blocked_details() -> Value {
    json!({
        "required_sequence": drm_open_sequence(),
        "required_runtime_events": drm_required_runtime_events(),
        "next_required_providers": drm_next_required_providers(),
    })
}

fn validate_open_request(request: &DrmOpenRequest) -> Result<(), String> {
    require_non_empty(&request.principal_id, "principal_id")?;
    require_non_empty(&request.session_id, "session_id")?;
    require_non_empty(&request.reason, "reason")?;
    validate_action(&request.action)?;
    validate_sealed_object(&request.object)
}

fn validate_sealed_object(object: &SealedObjectV1) -> Result<(), String> {
    if object.schema != SEALED_OBJECT_SCHEMA {
        return Err("sealed object schema is unsupported".to_string());
    }
    require_non_empty(&object.payload_cid, "payload_cid")?;
    require_non_empty(&object.rights_policy_cid, "rights_policy_cid")?;
    require_non_empty(&object.availability_receipt_cid, "availability_receipt_cid")?;
    require_non_empty(&object.key_envelope.scheme, "key_envelope.scheme")?;
    require_non_empty(&object.key_envelope.kid, "key_envelope.kid")?;
    require_non_empty(&object.key_envelope.wrapped_cek, "key_envelope.wrapped_cek")?;
    require_non_empty(&object.key_envelope.policy_hash, "key_envelope.policy_hash")?;
    validate_protected_content_key_envelope_algorithms(&object.key_envelope.algorithms)?;
    require_non_empty(
        &object.viewer.required_interface,
        "viewer.required_interface",
    )
}

fn validate_action(action: &str) -> Result<(), String> {
    if PROTECTED_CONTENT_ACTIONS.contains(&action) {
        Ok(())
    } else {
        Err(format!("unsupported protected-content action: {action}"))
    }
}

fn require_non_empty(value: &str, field: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("{field} is required"))
    } else {
        Ok(())
    }
}

fn main() {
    eprintln!(
        "drm-provider: starting v{} (protected content)",
        PROVIDER_VERSION
    );

    let mut provider = DrmProvider;
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                eprintln!("drm-provider read error: {}", err);
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

    eprintln!("drm-provider exiting");
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_common::protected_content::{
        KeyEnvelopeAlgorithmsV1, KeyEnvelopeV1, ViewerRequirementV1,
        DEFAULT_PROTECTED_CONTENT_CIPHER, DEFAULT_PROTECTED_CONTENT_KEMS,
        DEFAULT_PROTECTED_CONTENT_SHARE_SCHEME, DEFAULT_PROTECTED_CONTENT_SIGNATURES,
    };

    fn sealed_object() -> SealedObjectV1 {
        SealedObjectV1 {
            schema: SEALED_OBJECT_SCHEMA.to_string(),
            payload_cid: "bafybeigpayload".to_string(),
            rights_policy_cid: "bafybeigpolicy".to_string(),
            availability_receipt_cid: "bafybeigreceipt".to_string(),
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
            viewer: ViewerRequirementV1 {
                required_interface: "elastos.viewer/document@1".to_string(),
            },
        }
    }

    fn open_request() -> DrmOpenRequest {
        DrmOpenRequest {
            object: sealed_object(),
            principal_id: "person:local:test".to_string(),
            session_id: "session:test".to_string(),
            action: "view".to_string(),
            reason: "open protected document".to_string(),
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
        let provider = DrmProvider;
        let data = ok_data(provider.status());

        assert_eq!(data["provider"], "drm");
        assert_eq!(data["configured"], false);
        assert!(data["blocked_authority"]
            .as_array()
            .unwrap()
            .contains(&json!("raw_cek")));
        assert!(data["blocked_authority"]
            .as_array()
            .unwrap()
            .contains(&json!("chain_rpc")));
    }

    #[test]
    fn status_declares_canonical_open_sequence() {
        let provider = DrmProvider;
        let data = ok_data(provider.status());
        let sequence = data["required_sequence"].as_array().unwrap();

        assert_eq!(sequence.len(), 8);
        assert_eq!(sequence[0]["resource"], "elastos://content/status");
        assert_eq!(
            sequence[2]["resource"],
            "elastos://rights/access/has_access_by_content_id"
        );
        assert_eq!(sequence[3]["resource"], "elastos://key/release");
        assert_eq!(sequence[4]["resource"], "elastos://decrypt/session/open");
        assert_eq!(sequence[5]["resource"], "elastos://decrypt/render");
        assert_eq!(sequence[6]["step"], "release_receipt");
        assert_eq!(sequence[7]["step"], "audit");
        assert!(data["required_runtime_events"]
            .as_array()
            .unwrap()
            .contains(&json!("release_receipt")));
    }

    #[test]
    fn open_fails_closed_until_real_rights_key_and_decrypt_providers_exist() {
        let provider = DrmProvider;
        assert_eq!(error_code(provider.open(open_request())), "not_configured");
    }

    #[test]
    fn open_failure_declares_required_sequence() {
        let provider = DrmProvider;
        let response = serde_json::to_value(provider.open(open_request())).unwrap();

        assert_eq!(response["status"], "error");
        assert_eq!(response["code"], "not_configured");
        assert_eq!(
            response["details"]["required_sequence"][0]["resource"],
            "elastos://content/status"
        );
        assert_eq!(
            response["details"]["required_sequence"][3]["resource"],
            "elastos://key/release"
        );
        assert!(response["details"]["required_runtime_events"]
            .as_array()
            .unwrap()
            .contains(&json!("protected_content.open.audit")));
    }

    #[test]
    fn open_rejects_unsupported_actions_before_provider_work() {
        let provider = DrmProvider;
        let mut request = open_request();
        request.action = "raw_key".to_string();

        assert_eq!(error_code(provider.open(request)), "invalid_request");
    }

    #[test]
    fn open_rejects_non_sealed_objects_before_provider_work() {
        let provider = DrmProvider;
        let mut request = open_request();
        request.object.schema = "elastos.object/v1".to_string();

        assert_eq!(error_code(provider.open(request)), "invalid_request");
    }

    #[test]
    fn open_rejects_key_envelopes_without_algorithm_metadata() {
        let provider = DrmProvider;
        let mut request = open_request();
        request.object.key_envelope.algorithms.kem.clear();

        assert_eq!(error_code(provider.open(request)), "invalid_request");
    }

    #[test]
    fn open_rejects_key_envelopes_with_weak_cipher() {
        let provider = DrmProvider;
        let mut request = open_request();
        request.object.key_envelope.algorithms.cipher = "aes-128-gcm".to_string();

        assert_eq!(error_code(provider.open(request)), "invalid_request");
    }

    #[test]
    fn open_rejects_key_envelopes_without_hybrid_pq_kem() {
        let provider = DrmProvider;
        let mut request = open_request();
        request.object.key_envelope.algorithms.kem = vec!["x25519".to_string()];

        assert_eq!(error_code(provider.open(request)), "invalid_request");
    }

    #[test]
    fn open_wire_request_rejects_hidden_authority_fields() {
        let mut payload = serde_json::to_value(open_request()).unwrap();
        payload
            .as_object_mut()
            .unwrap()
            .insert("raw_cek".to_string(), json!("must-not-be-accepted"));

        let err = serde_json::from_value::<Request>(json!({
            "op": "open",
            "request": payload
        }))
        .unwrap_err()
        .to_string();

        assert!(err.contains("unknown field"));
    }
}
