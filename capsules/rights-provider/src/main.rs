//! ElastOS Rights Provider Capsule
//!
//! Fail-closed protected-content rights boundary. App capsules ask typed
//! questions; they never receive chain RPC, wallet RPC, contract SDK objects,
//! key-backend authority, raw CEKs, or provider credentials through this provider.

use elastos_common::protected_content::PROTECTED_CONTENT_ACTIONS;
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
    HasAccessByContentId {
        request: RightsAccessRequest,
    },
    IsSubscriptionActive {
        request: SubscriptionRequest,
    },
    CanStream {
        request: ContentRightsRequest,
    },
    CanDownload {
        request: ContentRightsRequest,
    },
    Shutdown,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RightsAccessRequest {
    principal_id: String,
    session_id: String,
    content_id: String,
    right: String,
    reason: String,
    #[serde(default)]
    policy_ref: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SubscriptionRequest {
    principal_id: String,
    session_id: String,
    plan_id: String,
    reason: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ContentRightsRequest {
    principal_id: String,
    session_id: String,
    content_id: String,
    reason: String,
    #[serde(default)]
    policy_ref: Option<String>,
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
struct RightsProvider;

impl RightsProvider {
    fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::Init { config } => self.init(config),
            Request::Status => self.status(),
            Request::HasAccessByContentId { request } => self.has_access_by_content_id(request),
            Request::IsSubscriptionActive { request } => self.is_subscription_active(request),
            Request::CanStream { request } => self.can_stream(request),
            Request::CanDownload { request } => self.can_download(request),
            Request::Shutdown => Response::empty_ok(),
        }
    }

    fn init(&mut self, _config: Value) -> Response {
        Response::ok(json!({
            "provider": "rights",
            "protocol_version": "1.0",
            "configured": false,
            "supported_operations": [
                "status",
                "has_access_by_content_id",
                "is_subscription_active",
                "can_stream",
                "can_download"
            ],
        }))
    }

    fn status(&self) -> Response {
        Response::ok(json!({
            "provider": "rights",
            "version": PROVIDER_VERSION,
            "configured": false,
            "supported_operations": [
                "status",
                "has_access_by_content_id",
                "is_subscription_active",
                "can_stream",
                "can_download"
            ],
            "supported_actions": PROTECTED_CONTENT_ACTIONS,
            "blocked_authority": [
                "contract_sdk",
                "chain_rpc",
                "wallet_rpc",
                "key_backend_sdk",
                "raw_cek",
                "provider_credentials"
            ],
            "next_required_providers": [
                "chain-provider",
                "wallet-provider",
                "key-provider",
                "decrypt-provider"
            ],
            "contract": {
                "schema": "elastos.protected-content.rights-provider/v1",
                "authority_boundary": "typed rights decisions only",
                "denied_to_apps": [
                    "contract_sdk",
                    "chain_rpc",
                    "wallet_rpc",
                    "key_backend_sdk",
                    "raw_cek",
                    "provider_credentials"
                ],
                "operations": {
                    "has_access_by_content_id": {
                        "input": [
                            "principal_id",
                            "session_id",
                            "content_id",
                            "right",
                            "reason",
                            "policy_ref?"
                        ],
                        "output": "allow/deny receipt when a dDRM policy backend is configured"
                    },
                    "can_stream": {
                        "input": ["principal_id", "session_id", "content_id", "reason", "policy_ref?"],
                        "output": "allow/deny stream receipt"
                    },
                    "can_download": {
                        "input": ["principal_id", "session_id", "content_id", "reason", "policy_ref?"],
                        "output": "allow/deny download receipt"
                    }
                },
                "status": "fail_closed_until_policy_backend_configured"
            },
        }))
    }

    fn has_access_by_content_id(&self, request: RightsAccessRequest) -> Response {
        if let Err(err) = validate_access_request(&request) {
            return Response::error("invalid_request", err);
        }
        Response::error(
            "not_configured",
            "rights checks require a configured dDRM/chain policy backend",
        )
    }

    fn is_subscription_active(&self, request: SubscriptionRequest) -> Response {
        if let Err(err) = validate_subscription_request(&request) {
            return Response::error("invalid_request", err);
        }
        Response::error(
            "not_configured",
            "subscription checks require a configured dDRM/chain policy backend",
        )
    }

    fn can_stream(&self, request: ContentRightsRequest) -> Response {
        self.content_action(request, "stream")
    }

    fn can_download(&self, request: ContentRightsRequest) -> Response {
        self.content_action(request, "download")
    }

    fn content_action(&self, request: ContentRightsRequest, action: &str) -> Response {
        if let Err(err) = validate_content_request(&request) {
            return Response::error("invalid_request", err);
        }
        if let Err(err) = validate_action(action) {
            return Response::error("invalid_request", err);
        }
        Response::error(
            "not_configured",
            format!("{action} rights require a configured dDRM/chain policy backend"),
        )
    }
}

fn validate_access_request(request: &RightsAccessRequest) -> Result<(), String> {
    require_non_empty(&request.principal_id, "principal_id")?;
    require_non_empty(&request.session_id, "session_id")?;
    require_identifier(&request.content_id, "content_id")?;
    validate_action(&request.right)?;
    require_non_empty(&request.reason, "reason")?;
    validate_optional_ref(request.policy_ref.as_deref(), "policy_ref")
}

fn validate_subscription_request(request: &SubscriptionRequest) -> Result<(), String> {
    require_non_empty(&request.principal_id, "principal_id")?;
    require_non_empty(&request.session_id, "session_id")?;
    require_identifier(&request.plan_id, "plan_id")?;
    require_non_empty(&request.reason, "reason")
}

fn validate_content_request(request: &ContentRightsRequest) -> Result<(), String> {
    require_non_empty(&request.principal_id, "principal_id")?;
    require_non_empty(&request.session_id, "session_id")?;
    require_identifier(&request.content_id, "content_id")?;
    require_non_empty(&request.reason, "reason")?;
    validate_optional_ref(request.policy_ref.as_deref(), "policy_ref")
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

fn validate_optional_ref(value: Option<&str>, field: &str) -> Result<(), String> {
    if let Some(value) = value {
        require_identifier(value, field)?;
    }
    Ok(())
}

fn main() {
    eprintln!(
        "rights-provider: starting v{} (protected content rights)",
        PROVIDER_VERSION
    );

    let mut provider = RightsProvider;
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                eprintln!("rights-provider read error: {}", err);
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

    eprintln!("rights-provider exiting");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn access_request() -> RightsAccessRequest {
        RightsAccessRequest {
            principal_id: "person:local:test".to_string(),
            session_id: "session:test".to_string(),
            content_id: "bafybeigprotectedcontent".to_string(),
            right: "view".to_string(),
            reason: "open protected document".to_string(),
            policy_ref: Some("bafybeigpolicy".to_string()),
        }
    }

    fn subscription_request() -> SubscriptionRequest {
        SubscriptionRequest {
            principal_id: "person:local:test".to_string(),
            session_id: "session:test".to_string(),
            plan_id: "plan:document".to_string(),
            reason: "open protected document".to_string(),
        }
    }

    fn content_request() -> ContentRightsRequest {
        ContentRightsRequest {
            principal_id: "person:local:test".to_string(),
            session_id: "session:test".to_string(),
            content_id: "bafybeigprotectedcontent".to_string(),
            reason: "open protected document".to_string(),
            policy_ref: Some("bafybeigpolicy".to_string()),
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
        let provider = RightsProvider;
        let data = ok_data(provider.status());

        assert_eq!(data["provider"], "rights");
        assert_eq!(data["configured"], false);
        assert!(data["blocked_authority"]
            .as_array()
            .unwrap()
            .contains(&json!("chain_rpc")));
        assert!(data["blocked_authority"]
            .as_array()
            .unwrap()
            .contains(&json!("contract_sdk")));
        assert_eq!(
            data["contract"]["schema"],
            "elastos.protected-content.rights-provider/v1"
        );
        assert_eq!(
            data["contract"]["status"],
            "fail_closed_until_policy_backend_configured"
        );
    }

    #[test]
    fn access_checks_fail_closed_until_backend_exists() {
        let provider = RightsProvider;
        assert_eq!(
            error_code(provider.has_access_by_content_id(access_request())),
            "not_configured"
        );
    }

    #[test]
    fn access_checks_reject_unsupported_actions() {
        let provider = RightsProvider;
        let mut request = access_request();
        request.right = "raw_key".to_string();

        assert_eq!(
            error_code(provider.has_access_by_content_id(request)),
            "invalid_request"
        );
    }

    #[test]
    fn subscription_checks_fail_closed_until_backend_exists() {
        let provider = RightsProvider;
        assert_eq!(
            error_code(provider.is_subscription_active(subscription_request())),
            "not_configured"
        );
    }

    #[test]
    fn stream_and_download_checks_fail_closed_until_backend_exists() {
        let provider = RightsProvider;

        assert_eq!(
            error_code(provider.can_stream(content_request())),
            "not_configured"
        );
        assert_eq!(
            error_code(provider.can_download(content_request())),
            "not_configured"
        );
    }

    #[test]
    fn content_checks_reject_path_like_identifiers() {
        let provider = RightsProvider;
        let mut request = content_request();
        request.content_id = "../secret".to_string();

        assert_eq!(error_code(provider.can_stream(request)), "invalid_request");
    }

    #[test]
    fn access_wire_request_rejects_hidden_chain_authority_fields() {
        let mut payload = serde_json::to_value(access_request()).unwrap();
        payload
            .as_object_mut()
            .unwrap()
            .insert("raw_chain_rpc".to_string(), json!("must-not-be-accepted"));

        let err = serde_json::from_value::<Request>(json!({
            "op": "has_access_by_content_id",
            "request": payload
        }))
        .unwrap_err()
        .to_string();

        assert!(err.contains("unknown field"));
    }

    #[test]
    fn subscription_wire_request_rejects_hidden_wallet_fields() {
        let mut payload = serde_json::to_value(subscription_request()).unwrap();
        payload
            .as_object_mut()
            .unwrap()
            .insert("wallet_rpc".to_string(), json!("must-not-be-accepted"));

        let err = serde_json::from_value::<Request>(json!({
            "op": "is_subscription_active",
            "request": payload
        }))
        .unwrap_err()
        .to_string();

        assert!(err.contains("unknown field"));
    }

    #[test]
    fn content_wire_request_rejects_hidden_key_authority_fields() {
        let mut payload = serde_json::to_value(content_request()).unwrap();
        payload
            .as_object_mut()
            .unwrap()
            .insert("raw_cek".to_string(), json!("must-not-be-accepted"));

        let err = serde_json::from_value::<Request>(json!({
            "op": "can_download",
            "request": payload
        }))
        .unwrap_err()
        .to_string();

        assert!(err.contains("unknown field"));
    }
}
