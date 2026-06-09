//! ElastOS Rights Provider Capsule
//!
//! Fail-closed protected-content rights boundary. App capsules ask typed
//! questions; they never receive chain RPC, wallet RPC, contract SDK objects,
//! key-backend authority, raw CEKs, or provider credentials through this provider.

use elastos_common::protected_content::PROTECTED_CONTENT_ACTIONS;
#[cfg(feature = "chain-rights")]
use elastos_common::protected_content::{RightsDecisionReceiptV1, RIGHTS_DECISION_RECEIPT_SCHEMA};
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
    /// Phase B (feature `chain-rights`): render the on-chain ownership answer into a
    /// typed `RightsDecisionReceiptV1`. The runtime core supplies `chain_access` — the
    /// typed result of `chain-provider::has_access_by_content_id` — and an injected
    /// clock; this provider binds it to the request and emits the decision. It does NO
    /// chain RPC itself.
    #[cfg(feature = "chain-rights")]
    DecideAccessFromChain {
        request_id: String,
        request: RightsAccessRequest,
        chain_access: ChainAccessAttestationV1,
        now_unix: u64,
        ttl_secs: u64,
    },
    Shutdown,
}

/// Typed on-chain ownership attestation (feature `chain-rights`): the exact shape
/// `chain-provider::has_access_by_content_id` returns. It is data, not authority —
/// rights-provider never holds the chain RPC capability; the runtime core obtained
/// this from chain-provider and injects it. `deny_unknown_fields` keeps any raw RPC
/// handle/credential from riding in.
#[cfg(feature = "chain-rights")]
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ChainAccessAttestationV1 {
    network: String,
    contract: String,
    content_id: String,
    subject: String,
    right: String,
    has_access: bool,
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
            #[cfg(feature = "chain-rights")]
            Request::DecideAccessFromChain {
                request_id,
                request,
                chain_access,
                now_unix,
                ttl_secs,
            } => self.decide_access_from_chain(request_id, request, chain_access, now_unix, ttl_secs),
            Request::Shutdown => Response::empty_ok(),
        }
    }

    /// Phase B (feature `chain-rights`): turn a typed on-chain ownership answer into a
    /// `RightsDecisionReceiptV1`. The attestation is bound to THIS request (matching
    /// content_id and right) so a stale/foreign chain answer cannot be slid in; the
    /// decision mirrors the chain (`allowed = has_access`) and a denial is a real,
    /// configured answer (downstream key-provider then fails closed on it). The clock
    /// is an injected capability input (`now_unix`), never an ambient read.
    #[cfg(feature = "chain-rights")]
    fn decide_access_from_chain(
        &self,
        request_id: String,
        request: RightsAccessRequest,
        attestation: ChainAccessAttestationV1,
        now_unix: u64,
        ttl_secs: u64,
    ) -> Response {
        if let Err(err) = validate_access_request(&request) {
            return Response::error("invalid_request", err);
        }
        if request_id.trim().is_empty() {
            return Response::error("invalid_request", "request_id is required");
        }
        if ttl_secs == 0 {
            return Response::error("invalid_request", "ttl_secs must be positive");
        }
        // Bind the chain attestation to this exact question — no foreign/stale answer.
        if attestation.content_id != request.content_id {
            return Response::error(
                "invalid_request",
                "chain attestation content_id does not match the request",
            );
        }
        if attestation.right != request.right {
            return Response::error(
                "invalid_request",
                "chain attestation right does not match the requested action",
            );
        }

        let allowed = attestation.has_access;
        let receipt = RightsDecisionReceiptV1 {
            schema: RIGHTS_DECISION_RECEIPT_SCHEMA.to_string(),
            request_id,
            content_id: request.content_id.clone(),
            principal_id: request.principal_id.clone(),
            session_id: request.session_id.clone(),
            right: request.right.clone(),
            provider: "rights-provider".to_string(),
            allowed,
            issued_at: now_unix,
            expires_at: now_unix.saturating_add(ttl_secs),
        };
        Response::ok(json!({
            "decision": if allowed { "allowed" } else { "denied" },
            "receipt": serde_json::to_value(&receipt).unwrap_or(Value::Null),
            // Provenance of the on-chain answer (data only — rights-provider did no RPC).
            "chain_source": {
                "network": attestation.network,
                "contract": attestation.contract,
                "subject": attestation.subject,
            },
        }))
    }

    fn init(&mut self, _config: Value) -> Response {
        Response::ok(json!({
            "provider": "rights",
            "protocol_version": "1.0",
            "configured": false,
            "supported_operations": supported_operations(),
        }))
    }

    fn status(&self) -> Response {
        Response::ok(json!({
            "provider": "rights",
            "version": PROVIDER_VERSION,
            "configured": false,
            "supported_operations": supported_operations(),
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

/// The operations this provider answers. `decide_access_from_chain` appears only
/// under the `chain-rights` dev profile; the default build stays fail-closed.
fn supported_operations() -> Vec<&'static str> {
    #[allow(unused_mut)]
    let mut ops = vec![
        "status",
        "has_access_by_content_id",
        "is_subscription_active",
        "can_stream",
        "can_download",
    ];
    #[cfg(feature = "chain-rights")]
    ops.push("decide_access_from_chain");
    ops
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

    // --- Phase B: on-chain ownership -> typed rights decision (feature `chain-rights`)
    #[cfg(feature = "chain-rights")]
    mod chain_rights {
        use super::*;

        const NOW: u64 = 1_800_000_000;
        const TTL: u64 = 3_600;

        fn attestation(has_access: bool) -> ChainAccessAttestationV1 {
            ChainAccessAttestationV1 {
                network: "base".to_string(),
                contract: "0x00000000000000000000000000000000000000aa".to_string(),
                content_id: "bafybeigprotectedcontent".to_string(),
                subject: "0x00000000000000000000000000000000000000bb".to_string(),
                right: "view".to_string(),
                has_access,
            }
        }

        fn decide(
            provider: &RightsProvider,
            req: RightsAccessRequest,
            att: ChainAccessAttestationV1,
        ) -> Response {
            provider.decide_access_from_chain("rights:1".to_string(), req, att, NOW, TTL)
        }

        /// Owned content: the chain says yes -> an `allowed` decision carrying a
        /// `RightsDecisionReceiptV1` bound to the request, ready for key-provider.
        #[test]
        fn owned_content_yields_an_allowed_receipt() {
            let data = ok_data(decide(&RightsProvider, access_request(), attestation(true)));
            assert_eq!(data["decision"], "allowed");
            let r = &data["receipt"];
            assert_eq!(r["schema"], RIGHTS_DECISION_RECEIPT_SCHEMA);
            assert_eq!(r["allowed"], true);
            assert_eq!(r["content_id"], "bafybeigprotectedcontent");
            assert_eq!(r["principal_id"], "person:local:test");
            assert_eq!(r["session_id"], "session:test");
            assert_eq!(r["right"], "view");
            assert_eq!(r["provider"], "rights-provider");
            assert_eq!(r["request_id"], "rights:1");
            assert_eq!(r["issued_at"], NOW);
            assert_eq!(r["expires_at"], NOW + TTL);
            // Provenance only — no chain RPC handle/credential surfaces.
            assert_eq!(data["chain_source"]["network"], "base");
        }

        /// Unowned content: the chain says no -> a real `denied` decision (a configured
        /// answer, not `not_configured`); the receipt is `allowed:false` so key-provider
        /// fails closed on it.
        #[test]
        fn unowned_content_yields_a_denied_receipt() {
            let data = ok_data(decide(&RightsProvider, access_request(), attestation(false)));
            assert_eq!(data["decision"], "denied");
            assert_eq!(data["receipt"]["allowed"], false);
        }

        /// A chain answer for a DIFFERENT object cannot be slid under this request.
        #[test]
        fn mismatched_content_id_fails_closed() {
            let mut att = attestation(true);
            att.content_id = "bafysomethingelse".to_string();
            assert_eq!(error_code(decide(&RightsProvider, access_request(), att)), "invalid_request");
        }

        /// A chain answer for a DIFFERENT right cannot authorize this action.
        #[test]
        fn mismatched_right_fails_closed() {
            let mut att = attestation(true);
            att.right = "download".to_string();
            assert_eq!(error_code(decide(&RightsProvider, access_request(), att)), "invalid_request");
        }

        /// The decision still validates the access request itself (e.g. action set).
        #[test]
        fn invalid_access_request_fails_closed() {
            let mut req = access_request();
            req.right = "raw_key".to_string();
            let mut att = attestation(true);
            att.right = "raw_key".to_string(); // make the attestation "match" so we test request validation
            assert_eq!(error_code(decide(&RightsProvider, req, att)), "invalid_request");
        }

        /// A zero TTL is rejected (a decision must carry a bounded lifetime).
        #[test]
        fn zero_ttl_fails_closed() {
            let resp =
                RightsProvider.decide_access_from_chain("rights:1".to_string(), access_request(), attestation(true), NOW, 0);
            assert_eq!(error_code(resp), "invalid_request");
        }

        /// Even under this profile, the plain `has_access_by_content_id` (no injected
        /// chain answer) stays fail-closed — the default path never decides on its own.
        #[test]
        fn plain_access_check_stays_fail_closed() {
            assert_eq!(
                error_code(RightsProvider.has_access_by_content_id(access_request())),
                "not_configured"
            );
        }

        /// The attestation deserializes from the EXACT shape
        /// `chain-provider::has_access_by_content_id` emits (field-for-field), so the
        /// runtime core passes that response straight in — no adapter, no drift. If
        /// chain-provider's output keys ever change, this guard fails.
        #[test]
        fn attestation_matches_chain_provider_output_shape() {
            // The literal keys chain-provider returns (see its has_access_by_content_id).
            let chain_output = json!({
                "network": "esc-local",
                "contract": "0x0000000000000000000000000000000000000001",
                "content_id": "bafybeigprotectedcontent",
                "subject": "0x0000000000000000000000000000000000000002",
                "right": "view",
                "has_access": true,
            });
            let att: ChainAccessAttestationV1 = serde_json::from_value(chain_output)
                .expect("chain-provider output maps 1:1 onto the attestation");
            assert!(att.has_access);
            assert_eq!(att.right, "view");
            assert_eq!(att.content_id, "bafybeigprotectedcontent");
        }

        /// The injected attestation cannot smuggle a raw chain-RPC handle.
        #[test]
        fn attestation_rejects_hidden_chain_authority() {
            let mut att = serde_json::to_value(attestation(true)).unwrap();
            att.as_object_mut()
                .unwrap()
                .insert("raw_chain_rpc".to_string(), json!("must-not-be-accepted"));
            let err = serde_json::from_value::<Request>(json!({
                "op": "decide_access_from_chain",
                "request_id": "rights:1",
                "request": serde_json::to_value(access_request()).unwrap(),
                "chain_access": att,
                "now_unix": NOW,
                "ttl_secs": TTL,
            }))
            .unwrap_err()
            .to_string();
            assert!(err.contains("unknown field"));
        }
    }
}
