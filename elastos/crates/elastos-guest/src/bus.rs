//! Canonical ElastOS bus request bindings.
//!
//! These types mirror the public `elastos:bus@v1` WIT records used by WASM
//! Component capsules and by native tests that need to assert the same Runtime
//! request shape.

use serde::{Deserialize, Serialize};

use crate::runtime::{RequestEnvelope, RequestId, RuntimeRequest};

pub const ABI: &str = "elastos.component/v1";
pub const WIT_PACKAGE: &str = "elastos:bus@1.0.0";
pub const WIT_WORLD: &str = "product-capsule-v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityRequest {
    pub resource: String,
    pub actions: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvokeRequest {
    pub resource: String,
    pub operation: String,
    #[serde(default)]
    pub body: serde_json::Value,
    #[serde(default)]
    pub grant: Option<String>,
}

pub fn request_capability(
    resource: impl Into<String>,
    action: impl Into<String>,
) -> RuntimeRequest {
    RuntimeRequest::RequestCapability {
        resource: resource.into(),
        action: action.into(),
        reason: String::new(),
    }
}

pub fn capability_request(request: CapabilityRequest) -> Result<RuntimeRequest, String> {
    let [action] = request.actions.as_slice() else {
        return Err("component capability requests currently require exactly one action".into());
    };
    Ok(RuntimeRequest::RequestCapability {
        resource: request.resource,
        action: action.clone(),
        reason: request.reason,
    })
}

pub fn provider_invoke(
    resource: impl Into<String>,
    operation: impl Into<String>,
    body: serde_json::Value,
    grant: Option<String>,
) -> RuntimeRequest {
    RuntimeRequest::ResourceInvoke {
        uri: resource.into(),
        operation: operation.into(),
        body,
        token: grant.unwrap_or_default(),
    }
}

pub fn invoke_request(request: InvokeRequest) -> RuntimeRequest {
    RuntimeRequest::ResourceInvoke {
        uri: request.resource,
        operation: request.operation,
        body: request.body,
        token: request.grant.unwrap_or_default(),
    }
}

pub fn bridge_envelope(id: RequestId, request: RuntimeRequest) -> RequestEnvelope {
    RequestEnvelope { id, request }
}

pub fn bridge_envelope_json(id: RequestId, request: RuntimeRequest) -> serde_json::Result<String> {
    serde_json::to_string(&bridge_envelope(id, request))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_invoke_matches_runtime_resource_invoke_request() {
        let component = invoke_request(InvokeRequest {
            resource: "localhost://Users/self/Documents/a.md".into(),
            operation: "read".into(),
            body: serde_json::json!({ "path": "localhost://Users/self/Documents/a.md" }),
            grant: Some("grant-token".into()),
        });
        let runtime_request = RuntimeRequest::ResourceInvoke {
            uri: "localhost://Users/self/Documents/a.md".into(),
            operation: "read".into(),
            body: serde_json::json!({ "path": "localhost://Users/self/Documents/a.md" }),
            token: "grant-token".into(),
        };
        assert_eq!(
            serde_json::to_value(component).unwrap(),
            serde_json::to_value(runtime_request).unwrap()
        );
    }

    #[test]
    fn component_capability_request_matches_runtime_capability_request() {
        let component = capability_request(CapabilityRequest {
            resource: "elastos://wallet/account/list".into(),
            actions: vec!["read".into()],
            reason: "show accounts".into(),
        })
        .unwrap();
        let runtime_request = RuntimeRequest::RequestCapability {
            resource: "elastos://wallet/account/list".into(),
            action: "read".into(),
            reason: "show accounts".into(),
        };
        assert_eq!(
            serde_json::to_value(component).unwrap(),
            serde_json::to_value(runtime_request).unwrap()
        );
    }
}
