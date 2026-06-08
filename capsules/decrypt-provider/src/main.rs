//! ElastOS Decrypt Provider Capsule
//!
//! Fail-closed protected-content decrypt/render boundary. App capsules never
//! receive raw CEKs, broad plaintext authority, filesystem authority,
//! key-backend SDK objects, KMS credentials, chain RPC, wallet RPC, or provider credentials
//! through this provider.

use elastos_common::protected_content::{
    DecryptSessionRequestV1, DECRYPT_SESSION_REQUEST_SCHEMA, PROTECTED_CONTENT_ACTIONS,
    PROTECTED_CONTENT_OUTPUTS, RELEASE_RECEIPT_SCHEMA,
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
    OpenSession {
        request: Box<DecryptSessionRequestV1>,
    },
    Render {
        request: Box<DecryptSessionRequestV1>,
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
struct DecryptProvider;

impl DecryptProvider {
    fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::Init { config } => self.init(config),
            Request::Status => self.status(),
            Request::OpenSession { request } => self.open_session(*request),
            Request::Render { request } => self.render(*request),
            Request::Shutdown => Response::empty_ok(),
        }
    }

    fn init(&mut self, _config: Value) -> Response {
        Response::ok(json!({
            "provider": "decrypt",
            "protocol_version": "1.0",
            "configured": false,
            "supported_operations": ["status", "open_session", "render"],
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
            "contract": {
                "schema": "elastos.protected-content.decrypt-provider/v1",
                "authority_boundary": "viewer-scoped decrypt/render sessions only",
                "denied_to_apps": [
                    "raw_cek",
                    "raw_plaintext",
                    "filesystem",
                    "key_backend_sdk",
                    "kms_node_credentials",
                    "chain_rpc",
                    "wallet_rpc",
                    "provider_credentials"
                ],
                "operations": {
                    "open_session": {
                        "input_schema": DECRYPT_SESSION_REQUEST_SCHEMA,
                        "input": [
                            "request_id",
                            "principal_id",
                            "session_id",
                            "object_cid",
                            "action",
                            "viewer_interface",
                            "release_receipt",
                            "output_kind",
                            "reason",
                            "expires_at"
                        ],
                        "output": "viewer-scoped decrypt session receipt"
                    },
                    "render": {
                        "input_schema": DECRYPT_SESSION_REQUEST_SCHEMA,
                        "output": "bounded rendered output for an authorized viewer capsule"
                    }
                },
                "supported_outputs": PROTECTED_CONTENT_OUTPUTS,
                "status": "fail_closed_until_key_release_and_decrypt_backend_configured"
            },
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
    validate_release_receipt_binding(request)?;
    validate_output_kind(&request.output_kind)?;
    require_non_empty(&request.reason, "reason")?;
    if request.expires_at == 0 {
        return Err("expires_at is required".to_string());
    }
    Ok(())
}

fn validate_release_receipt_binding(request: &DecryptSessionRequestV1) -> Result<(), String> {
    let receipt = &request.release_receipt;
    if receipt.schema != RELEASE_RECEIPT_SCHEMA {
        return Err("release receipt schema is unsupported".to_string());
    }
    require_identifier(&receipt.request_id, "release_receipt.request_id")?;
    require_identifier(&receipt.object_cid, "release_receipt.object_cid")?;
    require_non_empty(&receipt.principal_id, "release_receipt.principal_id")?;
    require_non_empty(&receipt.session_id, "release_receipt.session_id")?;
    validate_action(&receipt.action)?;
    require_non_empty(&receipt.provider, "release_receipt.provider")?;
    if receipt.provider != "key-provider" {
        return Err("release receipt provider must be key-provider".to_string());
    }
    if !matches!(receipt.status.as_str(), "released" | "issued") {
        return Err("release receipt must be issued or released".to_string());
    }
    if receipt.object_cid != request.object_cid {
        return Err("release receipt object_cid must match decrypt object_cid".to_string());
    }
    if receipt.principal_id != request.principal_id {
        return Err("release receipt principal_id must match decrypt principal_id".to_string());
    }
    if receipt.session_id != request.session_id {
        return Err("release receipt session_id must match decrypt session_id".to_string());
    }
    if receipt.action != request.action {
        return Err("release receipt action must match decrypt action".to_string());
    }
    if receipt.issued_at == 0 {
        return Err("release receipt issued_at is required".to_string());
    }
    if receipt.expires_at == 0 {
        return Err("release receipt expires_at is required".to_string());
    }
    if receipt.expires_at < request.expires_at {
        return Err("release receipt must cover the decrypt session expiry".to_string());
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

    let mut provider = DecryptProvider;
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
    use elastos_common::protected_content::ReleaseReceiptV1;

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
        let provider = DecryptProvider;
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
        assert_eq!(
            data["contract"]["schema"],
            "elastos.protected-content.decrypt-provider/v1"
        );
        assert_eq!(
            data["contract"]["status"],
            "fail_closed_until_key_release_and_decrypt_backend_configured"
        );
    }

    #[test]
    fn open_session_fails_closed_until_backend_exists() {
        let provider = DecryptProvider;
        assert_eq!(
            error_code(provider.open_session(decrypt_request())),
            "not_configured"
        );
    }

    #[test]
    fn render_fails_closed_until_backend_exists() {
        let provider = DecryptProvider;
        assert_eq!(
            error_code(provider.render(decrypt_request())),
            "not_configured"
        );
    }

    #[test]
    fn open_session_rejects_unsupported_output_kind() {
        let provider = DecryptProvider;
        let mut request = decrypt_request();
        request.output_kind = "raw_plaintext".to_string();

        assert_eq!(
            error_code(provider.open_session(request)),
            "invalid_request"
        );
    }

    #[test]
    fn open_session_rejects_path_like_object_ids() {
        let provider = DecryptProvider;
        let mut request = decrypt_request();
        request.object_cid = "../secret".to_string();

        assert_eq!(
            error_code(provider.open_session(request)),
            "invalid_request"
        );
    }

    #[test]
    fn open_session_rejects_dot_segment_object_ids() {
        let provider = DecryptProvider;
        let mut request = decrypt_request();
        request.object_cid = "..".to_string();

        assert_eq!(
            error_code(provider.open_session(request)),
            "invalid_request"
        );
    }

    #[test]
    fn open_session_rejects_denied_release_receipt() {
        let provider = DecryptProvider;
        let mut request = decrypt_request();
        request.release_receipt.status = "denied".to_string();

        assert_eq!(
            error_code(provider.open_session(request)),
            "invalid_request"
        );
    }

    #[test]
    fn open_session_rejects_mismatched_release_receipt() {
        let provider = DecryptProvider;
        let mut request = decrypt_request();
        request.release_receipt.object_cid = "bafybeigother".to_string();

        assert_eq!(
            error_code(provider.open_session(request)),
            "invalid_request"
        );
    }
}
