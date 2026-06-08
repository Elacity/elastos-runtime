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
struct KeyProvider;

impl KeyProvider {
    fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::Init { config } => self.init(config),
            Request::Status => self.status(),
            Request::Release { request } => self.release(*request),
            Request::Shutdown => Response::empty_ok(),
        }
    }

    fn init(&mut self, _config: Value) -> Response {
        Response::ok(json!({
            "provider": "key",
            "protocol_version": "1.0",
            "configured": false,
            "supported_operations": ["status", "release"],
        }))
    }

    fn status(&self) -> Response {
        Response::ok(json!({
            "provider": "key",
            "version": PROVIDER_VERSION,
            "configured": false,
            "supported_operations": ["status", "release"],
            "supported_schemes": SUPPORTED_SCHEMES,
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
            "contract": {
                "schema": "elastos.protected-content.key-provider/v1",
                "authority_boundary": "typed key-release receipts only",
                "denied_to_apps": [
                    "raw_cek",
                    "kms_node_credentials",
                    "chain_rpc",
                    "wallet_rpc",
                    "provider_credentials"
                ],
                "operations": {
                    "release": {
                        "input_schema": KEY_RELEASE_REQUEST_SCHEMA,
                        "input": [
                            "request_id",
                            "principal_id",
                            "session_id",
                            "object_cid",
                            "action",
                            "rights_receipt",
                            "key_envelope",
                            "reason",
                            "expires_at"
                        ],
                        "output": "key-release receipt or provider-scoped decrypt capability after an allowed rights receipt, never a raw CEK"
                    }
                },
                "supported_schemes": SUPPORTED_SCHEMES,
                "status": "fail_closed_until_dkms_backend_configured"
            },
        }))
    }

    fn release(&self, request: KeyReleaseRequestV1) -> Response {
        if let Err(err) = validate_key_release_request(&request) {
            return Response::error("invalid_request", err);
        }
        Response::error(
            "not_configured",
            "key release requires a configured ElastOS PQ-hybrid dKMS backend",
        )
    }
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
    validate_rights_receipt_binding(request)?;
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

fn validate_rights_receipt_binding(request: &KeyReleaseRequestV1) -> Result<(), String> {
    let receipt = &request.rights_receipt;
    if receipt.schema != RIGHTS_DECISION_RECEIPT_SCHEMA {
        return Err("rights receipt schema is unsupported".to_string());
    }
    require_identifier(&receipt.request_id, "rights_receipt.request_id")?;
    require_identifier(&receipt.content_id, "rights_receipt.content_id")?;
    require_non_empty(&receipt.principal_id, "rights_receipt.principal_id")?;
    require_non_empty(&receipt.session_id, "rights_receipt.session_id")?;
    validate_action(&receipt.right)?;
    require_non_empty(&receipt.provider, "rights_receipt.provider")?;
    if receipt.provider != "rights-provider" {
        return Err("rights receipt provider must be rights-provider".to_string());
    }
    if !receipt.allowed {
        return Err("rights receipt must allow the requested action".to_string());
    }
    if receipt.content_id != request.object_cid {
        return Err("rights receipt content_id must match object_cid".to_string());
    }
    if receipt.principal_id != request.principal_id {
        return Err("rights receipt principal_id must match key release principal_id".to_string());
    }
    if receipt.session_id != request.session_id {
        return Err("rights receipt session_id must match key release session_id".to_string());
    }
    if receipt.right != request.action {
        return Err("rights receipt right must match key release action".to_string());
    }
    if receipt.issued_at == 0 {
        return Err("rights receipt issued_at is required".to_string());
    }
    if receipt.expires_at == 0 {
        return Err("rights receipt expires_at is required".to_string());
    }
    if receipt.expires_at < request.expires_at {
        return Err("rights receipt must cover the key release expiry".to_string());
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

    let mut provider = KeyProvider;
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

    #[test]
    fn status_advertises_blocked_raw_authority() {
        let provider = KeyProvider;
        let data = ok_data(provider.status());

        assert_eq!(data["provider"], "key");
        assert_eq!(data["configured"], false);
        assert!(data["blocked_authority"]
            .as_array()
            .unwrap()
            .contains(&json!("raw_cek")));
        assert_eq!(
            data["contract"]["schema"],
            "elastos.protected-content.key-provider/v1"
        );
        assert_eq!(
            data["contract"]["status"],
            "fail_closed_until_dkms_backend_configured"
        );
    }

    #[test]
    fn release_fails_closed_until_backend_exists() {
        let provider = KeyProvider;
        assert_eq!(
            error_code(provider.release(key_release_request())),
            "not_configured"
        );
    }

    #[test]
    fn release_rejects_unsupported_scheme() {
        let provider = KeyProvider;
        let mut request = key_release_request();
        request.key_envelope.scheme = "frost-only".to_string();

        assert_eq!(error_code(provider.release(request)), "invalid_request");
    }

    #[test]
    fn release_rejects_weak_cipher() {
        let provider = KeyProvider;
        let mut request = key_release_request();
        request.key_envelope.algorithms.cipher = "aes-128-gcm".to_string();

        assert_eq!(error_code(provider.release(request)), "invalid_request");
    }

    #[test]
    fn release_rejects_missing_pq_hybrid_kem() {
        let provider = KeyProvider;
        let mut request = key_release_request();
        request.key_envelope.algorithms.kem = vec!["x25519".to_string()];

        assert_eq!(error_code(provider.release(request)), "invalid_request");
    }

    #[test]
    fn release_rejects_denied_rights_receipt() {
        let provider = KeyProvider;
        let mut request = key_release_request();
        request.rights_receipt.allowed = false;

        assert_eq!(error_code(provider.release(request)), "invalid_request");
    }

    #[test]
    fn release_rejects_mismatched_rights_receipt() {
        let provider = KeyProvider;
        let mut request = key_release_request();
        request.rights_receipt.principal_id = "person:local:other".to_string();

        assert_eq!(error_code(provider.release(request)), "invalid_request");
    }
}
