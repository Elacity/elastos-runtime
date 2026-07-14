use elastos_common::CapsuleAffordanceDescriptor;
use elastos_runtime::provider::{ProviderRegistration, ProviderRegistry};
use serde::Serialize;

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RuntimeCapsuleAffordanceBinding {
    CatalogList,
    CapsuleLaunch,
}

impl RuntimeCapsuleAffordanceBinding {
    pub(super) fn id(self) -> &'static str {
        match self {
            Self::CatalogList => "runtime.catalog.list",
            Self::CapsuleLaunch => "runtime.capsule.launch",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub(in crate::api::gateway) struct CapsuleMethodBindingSummary {
    pub(in crate::api::gateway) method: String,
    pub(in crate::api::gateway) state: String,
    pub(in crate::api::gateway) handler_available: bool,
    pub(in crate::api::gateway) executable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) handler_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) handler: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) required_action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api::gateway) reason: Option<String>,
}

pub(super) struct ResolvedCapsuleMethodBinding {
    pub(super) summary: CapsuleMethodBindingSummary,
    pub(super) runtime_binding: Option<RuntimeCapsuleAffordanceBinding>,
    pub(super) provider_registration: Option<ProviderRegistration>,
}

pub(super) fn runtime_capsule_affordance_binding(
    resource: &str,
    operation: &str,
) -> Option<RuntimeCapsuleAffordanceBinding> {
    match (resource, operation) {
        ("elastos://capsules/*", "list") => Some(RuntimeCapsuleAffordanceBinding::CatalogList),
        ("elastos://capsules/*", "launch") => Some(RuntimeCapsuleAffordanceBinding::CapsuleLaunch),
        _ => None,
    }
}

pub(super) fn static_capsule_method_binding(
    method: &CapsuleAffordanceDescriptor,
) -> ResolvedCapsuleMethodBinding {
    let resource = method.resource.as_deref().unwrap_or_default();
    let operation = method.operation.as_deref().unwrap_or_default();
    if resource.is_empty() || operation.is_empty() {
        return unresolved_binding(
            method,
            "descriptive-only",
            "method does not declare a Runtime resource and operation",
        );
    }

    let Some(runtime_binding) = runtime_capsule_affordance_binding(resource, operation) else {
        return unresolved_binding(
            method,
            "handler-unavailable",
            "no generic Runtime handler is bound; live provider availability requires a Runtime registry",
        );
    };
    let policy = affordance_invocation_policy(method);
    ResolvedCapsuleMethodBinding {
        summary: CapsuleMethodBindingSummary {
            method: method.id.clone(),
            state: if policy.is_ok() {
                "executable"
            } else {
                "approval-required"
            }
            .to_string(),
            handler_available: true,
            executable: policy.is_ok(),
            handler_kind: Some("runtime".to_string()),
            handler: Some(runtime_binding.id().to_string()),
            required_action: None,
            reason: policy.err().map(|(_, _, message)| message.to_string()),
        },
        runtime_binding: Some(runtime_binding),
        provider_registration: None,
    }
}

pub(super) async fn resolve_capsule_method_binding(
    method: &CapsuleAffordanceDescriptor,
    registry: Option<&ProviderRegistry>,
) -> ResolvedCapsuleMethodBinding {
    let static_binding = static_capsule_method_binding(method);
    if static_binding.runtime_binding.is_some()
        || static_binding.summary.state == "descriptive-only"
    {
        return static_binding;
    }

    let resource = method.resource.as_deref().unwrap_or_default();
    let operation = method.operation.as_deref().unwrap_or_default();
    let Some(registry) = registry else {
        return static_binding;
    };
    let Some(registration) = registry.registration_for_uri(resource).await else {
        return unresolved_binding(
            method,
            "handler-unavailable",
            "no live Runtime or provider handler is registered for this method",
        );
    };
    let Some(action) =
        crate::provider_resource::provider_operation_action(&registration.route, operation)
    else {
        return ResolvedCapsuleMethodBinding {
            summary: CapsuleMethodBindingSummary {
                method: method.id.clone(),
                state: "unbound".to_string(),
                handler_available: true,
                executable: false,
                handler_kind: Some("provider".to_string()),
                handler: Some(registration.provider.clone()),
                required_action: None,
                reason: Some(
                    "provider is registered but the operation has no canonical Runtime action"
                        .to_string(),
                ),
            },
            runtime_binding: None,
            provider_registration: Some(registration),
        };
    };
    let policy = affordance_invocation_policy(method);
    ResolvedCapsuleMethodBinding {
        summary: CapsuleMethodBindingSummary {
            method: method.id.clone(),
            state: if policy.is_err() {
                "approval-required"
            } else {
                "provider-path-only"
            }
            .to_string(),
            handler_available: true,
            executable: false,
            handler_kind: Some("provider".to_string()),
            handler: Some(registration.provider.clone()),
            required_action: Some(action.to_string()),
            reason: Some(
                policy
                    .err()
                    .map(|(_, _, message)| message.to_string())
                    .unwrap_or_else(|| {
                        "available through the capability-gated provider path, not generic interface invocation"
                            .to_string()
                    }),
            ),
        },
        runtime_binding: None,
        provider_registration: Some(registration),
    }
}

fn unresolved_binding(
    method: &CapsuleAffordanceDescriptor,
    state: &str,
    reason: &str,
) -> ResolvedCapsuleMethodBinding {
    ResolvedCapsuleMethodBinding {
        summary: CapsuleMethodBindingSummary {
            method: method.id.clone(),
            state: state.to_string(),
            handler_available: false,
            executable: false,
            handler_kind: None,
            handler: None,
            required_action: None,
            reason: Some(reason.to_string()),
        },
        runtime_binding: None,
        provider_registration: None,
    }
}
