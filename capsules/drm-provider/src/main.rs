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
            "blocked_authority": drm_blocked_authority(),
            "next_required_providers": drm_next_required_providers(),
        }))
    }

    fn open(&self, request: DrmOpenRequest) -> Response {
        if let Err(err) = validate_open_request(&request) {
            return Response::error("invalid_request", err);
        }
        // The drm-provider PLANS the open; it never opens. It holds no CEK, no
        // key-backend SDK, no chain/wallet RPC — so it cannot fetch content,
        // check rights, release a key, or decrypt. It emits the single canonical
        // `drm/open` sequence (status `planned`, never `opened`) with the explicit
        // binding edges between steps, and the runtime injects the capabilities and
        // executes it. This mirrors `publish-provider`'s `prepared` mint intent
        // (Day 61) and PC2's `recoverCEKEnvelope` sequencer, which signs a request,
        // runs the access-check -> key-release -> seal action in fixed order, and
        // returns only sealed material (chipotle-client.ts:1438-1538).
        match build_open_plan(&request) {
            Ok(plan) => Response::ok(serde_json::to_value(plan).expect("plan serializes")),
            Err(err) => Response::error("invalid_request", err),
        }
    }
}

/// The single backend-neutral identifier of the executable open plan. Capsule-local
/// (like `publish-provider::UnsignedMintV1`), so the frozen shared `protected_content`
/// contract surface — and the drift gate — stays untouched.
const DRM_OPEN_PLAN_SCHEMA: &str = "elastos.drm.open.plan/v1";

/// One edge of the canonical open sequence: the artifact a `from_step` produces and
/// the request field of `into_step` it must be bound into. This is the "what flows
/// where" the runtime needs to chain the providers; it is declarative data, not an
/// invocation (the drm-provider invokes nothing).
#[derive(Debug, Serialize)]
struct DrmPlanBindingV1 {
    from_step: &'static str,
    produces: &'static str,
    into_step: &'static str,
    into_field: &'static str,
}

/// The typed, executable `drm/open` plan the runtime follows. The drm-provider emits
/// it (status `planned`) holding zero authority; the runtime injects the rights/key/
/// decrypt capabilities and runs the steps in order, binding each step's output into
/// the next per `bindings`. The CEK never appears here — `blocked_authority` is
/// advertised exactly as in `status`.
#[derive(Debug, Serialize)]
struct DrmOpenPlanV1 {
    schema: &'static str,
    status: &'static str,
    provider: &'static str,
    principal_id: String,
    session_id: String,
    /// The content identity the on-chain rights check keys on. Per the Day-58 join
    /// this is the KID (== `bytes16 contentId`), carried as `object.key_envelope.kid`.
    content_id: String,
    /// The decrypt boundary's object reference. The shared contract requires
    /// `rights_receipt.content_id == key/decrypt.object_cid` (enforced in
    /// `key-provider`), so this equals `content_id` — one identity, two field names.
    object_cid: String,
    action: String,
    viewer_interface: String,
    steps: Value,
    bindings: Vec<DrmPlanBindingV1>,
    next_required_providers: Value,
    required_runtime_events: Value,
    blocked_authority: Vec<&'static str>,
}

/// The raw authority the drm-provider can never hold or surface — advertised
/// identically by `status` and embedded in every plan so a reader can prove the
/// orchestrator is incapable of doing the dangerous work itself.
fn drm_blocked_authority() -> Vec<&'static str> {
    vec![
        "raw_cek",
        "key_backend_sdk",
        "wallet_rpc",
        "chain_rpc",
        "kubo_api",
        "elacity_sdk",
    ]
}

/// Build the executable open plan from an already-validated request. The content
/// identity is the KID; the shared contract's `content_id == object_cid` invariant
/// is honoured by emitting the same value under both names.
fn build_open_plan(request: &DrmOpenRequest) -> Result<DrmOpenPlanV1, String> {
    let content_id = request.object.key_envelope.kid.trim().to_string();
    if content_id.is_empty() {
        return Err("key_envelope.kid is required".to_string());
    }
    let viewer_interface = request.object.viewer.required_interface.trim().to_string();
    Ok(DrmOpenPlanV1 {
        schema: DRM_OPEN_PLAN_SCHEMA,
        status: "planned",
        provider: "drm",
        principal_id: request.principal_id.clone(),
        session_id: request.session_id.clone(),
        object_cid: content_id.clone(),
        content_id,
        action: request.action.clone(),
        viewer_interface,
        steps: drm_open_sequence(),
        bindings: drm_open_plan_bindings(),
        next_required_providers: drm_next_required_providers(),
        required_runtime_events: drm_required_runtime_events(),
        blocked_authority: drm_blocked_authority(),
    })
}

/// The binding edges of the canonical sequence: the content identity flows into the
/// rights check and the decrypt session; the rights decision gates the key release;
/// the release receipt gates the decrypt session; the object's declared viewer
/// requirement selects the decrypt viewer interface. Field names match the shared
/// `RightsDecisionReceiptV1` / `KeyReleaseRequestV1` / `DecryptSessionRequestV1`
/// surface (pinned by `chain_seam_tests`), so a contract rename fails loudly.
fn drm_open_plan_bindings() -> Vec<DrmPlanBindingV1> {
    vec![
        DrmPlanBindingV1 {
            from_step: "drm_open",
            produces: "content_id",
            into_step: "rights_check",
            into_field: "content_id",
        },
        DrmPlanBindingV1 {
            from_step: "rights_check",
            produces: "RightsDecisionReceiptV1",
            into_step: "key_release",
            into_field: "rights_receipt",
        },
        DrmPlanBindingV1 {
            from_step: "key_release",
            produces: "ReleaseReceiptV1",
            into_step: "decrypt_session",
            into_field: "release_receipt",
        },
        DrmPlanBindingV1 {
            from_step: "drm_open",
            produces: "object_cid",
            into_step: "decrypt_session",
            into_field: "object_cid",
        },
        DrmPlanBindingV1 {
            from_step: "drm_open",
            produces: "viewer_interface",
            into_step: "decrypt_session",
            into_field: "viewer_interface",
        },
    ]
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
    fn open_emits_a_planned_plan_never_opens_itself() {
        // The drm-provider PLANS but never OPENS: a valid request yields a typed
        // plan whose status is `planned` (not `opened`), declaring the canonical
        // sequence the runtime must execute. The capsule decrypts nothing.
        let provider = DrmProvider;
        let data = ok_data(provider.open(open_request()));

        assert_eq!(data["schema"], DRM_OPEN_PLAN_SCHEMA);
        assert_eq!(data["status"], "planned");
        assert_ne!(data["status"], "opened");
        assert_eq!(data["provider"], "drm");
        assert_eq!(data["principal_id"], "person:local:test");
        assert_eq!(data["session_id"], "session:test");
        assert_eq!(data["action"], "view");
    }

    #[test]
    fn open_plan_declares_canonical_sequence_and_runtime_events() {
        let provider = DrmProvider;
        let data = ok_data(provider.open(open_request()));
        let steps = data["steps"].as_array().unwrap();

        assert_eq!(steps.len(), 8);
        assert_eq!(steps[0]["resource"], "elastos://content/status");
        assert_eq!(
            steps[2]["resource"],
            "elastos://rights/access/has_access_by_content_id"
        );
        assert_eq!(steps[3]["resource"], "elastos://key/release");
        assert_eq!(steps[4]["resource"], "elastos://decrypt/session/open");
        assert_eq!(steps[6]["step"], "release_receipt");
        assert_eq!(steps[7]["step"], "audit");
        assert!(data["required_runtime_events"]
            .as_array()
            .unwrap()
            .contains(&json!("protected_content.open.audit")));
        let providers = data["next_required_providers"].as_array().unwrap();
        assert!(providers.contains(&json!("rights-provider")));
        assert!(providers.contains(&json!("key-provider")));
        assert!(providers.contains(&json!("decrypt-provider")));
    }

    #[test]
    fn open_plan_carries_one_content_identity_under_both_names() {
        // The shared contract requires rights `content_id == key/decrypt object_cid`
        // (enforced in key-provider). The plan emits the KID under both names so the
        // identity cannot drift between the rights check and the decrypt session.
        let provider = DrmProvider;
        let data = ok_data(provider.open(open_request()));

        assert_eq!(data["content_id"], "kid:test");
        assert_eq!(data["object_cid"], "kid:test");
        assert_eq!(data["content_id"], data["object_cid"]);
        assert_eq!(data["viewer_interface"], "elastos.viewer/document@1");
    }

    #[test]
    fn open_plan_declares_the_receipt_binding_edges() {
        let provider = DrmProvider;
        let data = ok_data(provider.open(open_request()));
        let bindings = data["bindings"].as_array().unwrap();

        // rights -> key: the RightsDecisionReceiptV1 binds into key_release.rights_receipt.
        assert!(bindings.iter().any(|b| {
            b["from_step"] == "rights_check"
                && b["produces"] == "RightsDecisionReceiptV1"
                && b["into_step"] == "key_release"
                && b["into_field"] == "rights_receipt"
        }));
        // key -> decrypt: the ReleaseReceiptV1 binds into decrypt_session.release_receipt.
        assert!(bindings.iter().any(|b| {
            b["from_step"] == "key_release"
                && b["produces"] == "ReleaseReceiptV1"
                && b["into_step"] == "decrypt_session"
                && b["into_field"] == "release_receipt"
        }));
        // content identity flows into the rights check.
        assert!(bindings.iter().any(|b| {
            b["produces"] == "content_id"
                && b["into_step"] == "rights_check"
                && b["into_field"] == "content_id"
        }));
        // the object's declared viewer requirement selects the decrypt viewer interface.
        assert!(bindings.iter().any(|b| {
            b["produces"] == "viewer_interface"
                && b["into_step"] == "decrypt_session"
                && b["into_field"] == "viewer_interface"
        }));
    }

    #[test]
    fn open_plan_carries_no_raw_authority() {
        // The plan must be incapable of doing the dangerous work: it advertises the
        // blocked authority and contains neither a CEK nor any wrapped key material.
        let provider = DrmProvider;
        let data = ok_data(provider.open(open_request()));
        let blocked = data["blocked_authority"].as_array().unwrap();

        assert!(blocked.contains(&json!("raw_cek")));
        assert!(blocked.contains(&json!("key_backend_sdk")));
        assert!(blocked.contains(&json!("chain_rpc")));

        let serialized = serde_json::to_string(&data).unwrap();
        assert!(!serialized.contains("wrapped")); // the sealed object's wrapped_cek is not echoed
        assert!(!serialized.contains("cek_b64"));
        assert!(!serialized.contains("raw_plaintext"));
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

/// Characterization tests for the inter-provider contract seams.
///
/// The drm-provider orchestrates `rights -> key -> decrypt`. These tests prove the
/// receipt each step emits deserializes *exactly* into the next step's request type,
/// so the contracts compose end-to-end. If a shared contract type drifts, these fail
/// loudly here rather than silently at runtime.
#[cfg(test)]
mod chain_seam_tests {
    use elastos_common::protected_content::{
        DecryptSessionRequestV1, KeyReleaseRequestV1, ReleaseReceiptV1, RightsDecisionReceiptV1,
        DECRYPT_SESSION_REQUEST_SCHEMA, KEY_RELEASE_REQUEST_SCHEMA, RELEASE_RECEIPT_SCHEMA,
        RIGHTS_DECISION_RECEIPT_SCHEMA,
    };
    use serde_json::json;

    const PRINCIPAL: &str = "person:local:test";
    const SESSION: &str = "session:test";
    const OBJECT: &str = "bafybeigprotectedcontent";

    fn key_envelope_json() -> serde_json::Value {
        json!({
            "scheme": "elastos-pq-hybrid-threshold-v0",
            "kid": "kid:test",
            "wrapped_cek": "wrapped",
            "policy_hash": "sha256:test",
            "algorithms": {
                "cipher": "aes-256-gcm",
                "signature": ["ed25519", "ml-dsa-65"],
                "kem": ["x25519", "ml-kem-768"],
                "share_scheme": "shamir-t-of-n"
            }
        })
    }

    /// rights -> key: a RightsDecisionReceiptV1 deserializes as the `rights_receipt`
    /// field of the key-provider's request, with binding fields intact.
    #[test]
    fn rights_receipt_flows_into_key_release_request() {
        let rights_receipt = json!({
            "schema": RIGHTS_DECISION_RECEIPT_SCHEMA,
            "request_id": "rights:test",
            "content_id": OBJECT,
            "principal_id": PRINCIPAL,
            "session_id": SESSION,
            "right": "view",
            "provider": "rights-provider",
            "allowed": true,
            "issued_at": 1_800_000_000u64,
            "expires_at": 1_900_000_000u64
        });

        // The shape stands alone as a RightsDecisionReceiptV1 ...
        serde_json::from_value::<RightsDecisionReceiptV1>(rights_receipt.clone()).unwrap();

        // ... and embeds cleanly into the next step's request.
        let request: KeyReleaseRequestV1 = serde_json::from_value(json!({
            "schema": KEY_RELEASE_REQUEST_SCHEMA,
            "request_id": "key-release:test",
            "principal_id": PRINCIPAL,
            "session_id": SESSION,
            "object_cid": OBJECT,
            "action": "view",
            "rights_receipt": rights_receipt,
            "key_envelope": key_envelope_json(),
            "reason": "open protected document",
            "expires_at": 1_900_000_000u64
        }))
        .unwrap();

        assert!(request.rights_receipt.allowed);
        assert_eq!(request.rights_receipt.content_id, request.object_cid);
        assert_eq!(request.rights_receipt.principal_id, request.principal_id);
        assert_eq!(request.rights_receipt.right, request.action);
    }

    /// key -> decrypt: a ReleaseReceiptV1 deserializes as the `release_receipt` field
    /// of the decrypt-provider's session request, with binding fields intact.
    #[test]
    fn release_receipt_flows_into_decrypt_session_request() {
        let release_receipt = json!({
            "schema": RELEASE_RECEIPT_SCHEMA,
            "request_id": "key-release:test",
            "object_cid": OBJECT,
            "principal_id": PRINCIPAL,
            "session_id": SESSION,
            "action": "view",
            "provider": "key-provider",
            "status": "released",
            "issued_at": 1_800_000_000u64,
            "expires_at": 1_900_000_000u64
        });

        serde_json::from_value::<ReleaseReceiptV1>(release_receipt.clone()).unwrap();

        let request: DecryptSessionRequestV1 = serde_json::from_value(json!({
            "schema": DECRYPT_SESSION_REQUEST_SCHEMA,
            "request_id": "decrypt:test",
            "principal_id": PRINCIPAL,
            "session_id": SESSION,
            "object_cid": OBJECT,
            "action": "view",
            "viewer_interface": "elastos.viewer/document@1",
            "release_receipt": release_receipt,
            "output_kind": "rendered",
            "reason": "open protected document",
            "expires_at": 1_900_000_000u64
        }))
        .unwrap();

        assert_eq!(request.release_receipt.status, "released");
        assert_eq!(request.release_receipt.object_cid, request.object_cid);
        assert_eq!(request.release_receipt.principal_id, request.principal_id);
        assert_eq!(request.release_receipt.action, request.action);
    }
}
