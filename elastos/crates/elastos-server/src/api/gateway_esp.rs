use super::*;

const ESP_INITIALIZE_SCHEMA: &str = "elastos.esp.initialize/v0";
const ESP_PROTOCOL: &str = "elastos-shell-protocol";
const ESP_VERSION: &str = "0";
const ESP_TRANSPORT: &str = "http-json";
const ESP_TRANSPORT_SCOPE: &str = "local_runtime_adapter";

const SUPPORTED_SCHEMAS: &[&str] = &[
    "elastos.capsules.catalog/v1",
    "elastos.capsules.interfaces/v1",
    "elastos.inspect.capsules/v1",
    "elastos.inspect.object/v1",
    "elastos.inspect.gate-preview/v1",
    "elastos.inspect.action-request/v1",
    "elastos.inspect.request-binding/v1",
    "elastos.inspect.dispatch-result/v1",
];

const FACTS: &[EspFact] = &[
    EspFact {
        family: "capsule_catalog",
        schema: "elastos.capsules.catalog/v1",
        operation: "capsules.catalog",
        method: "GET",
        route: "/api/capsules/catalog",
        auth: "Home, System, Marketplace, or launchable shell token",
        authority: "Read-only installed capsule and affordance projection; descriptors are not grants.",
    },
    EspFact {
        family: "capsule_interfaces",
        schema: "elastos.capsules.interfaces/v1",
        operation: "capsules.interfaces",
        method: "GET",
        route: "/api/capsules/interfaces",
        auth: "Home, System, Marketplace, or launchable shell token",
        authority: "Read-only interface registry derived from installed capsule manifests.",
    },
    EspFact {
        family: "inspect_capsules",
        schema: "elastos.inspect.capsules/v1",
        operation: "inspect.capsules",
        method: "POST",
        route: "/api/provider/inspect/capsules",
        auth: "System launch token",
        authority: "System mirror list; ordinary capsules do not receive the system-wide view.",
    },
    EspFact {
        family: "inspect_object",
        schema: "elastos.inspect.object/v1",
        operation: "inspect.object",
        method: "POST",
        route: "/api/provider/inspect/capsule",
        auth: "System launch token",
        authority: "Redacted capsule/provider object projection with provenance fingerprint, not raw secrets.",
    },
    EspFact {
        family: "gate_preview",
        schema: "elastos.inspect.gate-preview/v1",
        operation: "inspect.gate_preview",
        method: "POST",
        route: "/api/provider/inspect/plan",
        auth: "System launch token",
        authority: "Preview-only authority reflection; cannot dispatch or mutate.",
    },
    EspFact {
        family: "inspect_action_request",
        schema: "elastos.inspect.action-request/v1",
        operation: "inspect.request_act",
        method: "POST",
        route: "/api/provider/inspect/request_act",
        auth: "System launch token",
        authority: "Creates a principal-bound Inbox approval request; no provider dispatch happens here.",
    },
];

const VERBS: &[EspVerb] = &[
    EspVerb {
        name: "inspect.request_act",
        method: "POST",
        route: "/api/provider/inspect/request_act",
        auth: "System launch token",
        effect: "Stores a pending Inspector action request with a canonical request binding.",
        gate: "Inbox approval required before dispatch.",
    },
    EspVerb {
        name: "inbox.approve_inspect_action",
        method: "POST",
        route: "/api/apps/inbox/actions",
        auth: "Inbox launch token plus fresh same-principal passkey Home token",
        effect: "Revalidates request binding and authority plan, then dispatches through ProviderRegistry.",
        gate: "Fails closed when request body, authority plan, principal, or passkey proof does not match.",
    },
    EspVerb {
        name: "inbox.deny_inspect_action",
        method: "POST",
        route: "/api/apps/inbox/actions",
        auth: "Inbox launch token",
        effect: "Marks the Inspector action request denied without dispatch.",
        gate: "Fail-safe direction only; denial never mutates the target provider.",
    },
    EspVerb {
        name: "capsule.invoke_runtime_policy_affordance",
        method: "POST",
        route: "/api/capsules/interfaces/invoke",
        auth: "Target capsule launch token",
        effect: "Invokes only Runtime-policy, low-risk affordances with existing Runtime bindings.",
        gate: "User-approval and high-risk affordances fail closed in this slice.",
    },
];

const INVARIANTS: &[&str] = &[
    "ESP is a projection and consent surface, not an authority layer.",
    "Facts name existing Runtime routes; the route-specific gates still decide access.",
    "HTTP method and route fields describe the current local adapter, not the ESP authority model.",
    "Future Carrier transport may expose the same schemas only by preserving the same Runtime gates.",
    "Preview-only facts must not mutate or dispatch.",
    "Approved dispatch must go through Inbox approval and ProviderRegistry.",
    "Shells must ignore unknown fact fields and must not send hidden Runtime metadata.",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EspInitializeRequest {
    #[serde(default)]
    esp_version: Option<String>,
    #[serde(default)]
    accepts: Vec<String>,
}

#[derive(Debug, Serialize)]
struct EspFact {
    family: &'static str,
    schema: &'static str,
    operation: &'static str,
    method: &'static str,
    route: &'static str,
    auth: &'static str,
    authority: &'static str,
}

#[derive(Debug, Serialize)]
struct EspVerb {
    name: &'static str,
    method: &'static str,
    route: &'static str,
    auth: &'static str,
    effect: &'static str,
    gate: &'static str,
}

#[derive(Debug, Serialize)]
struct EspInitializeResponse {
    schema: &'static str,
    protocol: &'static str,
    esp_version: &'static str,
    transport: &'static str,
    transport_scope: &'static str,
    supported_schemas: &'static [&'static str],
    facts: &'static [EspFact],
    verbs: &'static [EspVerb],
    invariants: &'static [&'static str],
    accepted: Vec<String>,
    unsupported: Vec<String>,
}

pub(super) async fn esp_descriptor() -> Response {
    Json(esp_initialize_response(&[])).into_response()
}

pub(super) async fn esp_initialize(Json(request): Json<EspInitializeRequest>) -> Response {
    let version = request.esp_version.as_deref().unwrap_or(ESP_VERSION);
    if version != ESP_VERSION {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "schema": ESP_INITIALIZE_SCHEMA,
                "status": "error",
                "code": "unsupported_esp_version",
                "supported": [ESP_VERSION],
            })),
        )
            .into_response();
    }
    Json(esp_initialize_response(&request.accepts)).into_response()
}

fn esp_initialize_response(accepts: &[String]) -> EspInitializeResponse {
    let mut accepted = Vec::new();
    let mut unsupported = Vec::new();
    for schema in accepts {
        if SUPPORTED_SCHEMAS.contains(&schema.as_str()) {
            accepted.push(schema.clone());
        } else {
            unsupported.push(schema.clone());
        }
    }

    EspInitializeResponse {
        schema: ESP_INITIALIZE_SCHEMA,
        protocol: ESP_PROTOCOL,
        esp_version: ESP_VERSION,
        transport: ESP_TRANSPORT,
        transport_scope: ESP_TRANSPORT_SCOPE,
        supported_schemas: SUPPORTED_SCHEMAS,
        facts: FACTS,
        verbs: VERBS,
        invariants: INVARIANTS,
        accepted,
        unsupported,
    }
}
