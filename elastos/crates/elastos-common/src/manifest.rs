//! Capsule manifest types

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::localhost::{
    is_runtime_system_service_resource, is_supported_resource_scheme,
    is_system_only_backend_resource,
};

/// The main capsule manifest structure (capsule.json)
///
/// Strict v1 format:
/// - `schema: "elastos.capsule/v1"`
/// - `version: "<semver-like string>"`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapsuleManifest {
    /// Schema identifier (must be `elastos.capsule/v1`).
    pub schema: String,

    pub version: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    pub role: CapsuleRole,

    #[serde(rename = "type")]
    pub capsule_type: CapsuleType,

    pub entrypoint: String,

    /// Typed requirements needed by this capsule.
    /// - `capsule`: another capsule that must be running
    /// - `external`: an external tool/materialized artifact
    #[serde(default)]
    pub requires: Vec<CapsuleRequirement>,

    /// Protocol URI this capsule provides (e.g. "elastos://ipfs/*")
    #[serde(default)]
    pub provides: Option<String>,

    /// Provider-only authority declaration for reviewable service exceptions.
    #[serde(default)]
    pub authority: Option<ProviderAuthority>,

    /// Capabilities this capsule needs from other capsules (URI list)
    #[serde(default)]
    pub capabilities: Vec<String>,

    /// Typed callable interfaces exposed by this capsule.
    ///
    /// Interface descriptors are discoverability metadata only. They do not
    /// grant authority; runtime grants, approval policy, and audit still decide
    /// who may invoke an affordance.
    #[serde(default)]
    pub interfaces: Vec<CapsuleInterfaceDescriptor>,

    #[serde(default)]
    pub resources: ResourceLimits,

    #[serde(default)]
    pub permissions: Permissions,

    /// Configuration for MicroVM capsules
    #[serde(default)]
    pub microvm: Option<MicroVmConfig>,

    /// Required providers (scheme -> source, e.g. "local" -> "built-in")
    #[serde(default)]
    pub providers: Option<std::collections::HashMap<String, String>>,

    /// Viewer capsule: path or CID of a capsule that can display this data capsule
    #[serde(default)]
    pub viewer: Option<String>,

    /// Optional base64-encoded signature
    #[serde(default)]
    pub signature: Option<String>,
}

/// A single typed capsule requirement.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CapsuleRequirement {
    pub name: String,
    pub kind: RequirementKind,
}

/// Requirement kind for manifest `requires`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RequirementKind {
    Capsule,
    External,
}

/// Typed callable interface exposed by a capsule.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CapsuleInterfaceDescriptor {
    pub id: String,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub methods: Vec<CapsuleAffordanceDescriptor>,
}

impl CapsuleInterfaceDescriptor {
    fn validate(&self) -> Result<(), String> {
        validate_descriptor_id("interface id", &self.id)?;
        if self.version.trim().is_empty() {
            return Err(format!("interface {} version must not be empty", self.id));
        }
        if self.methods.is_empty() {
            return Err(format!(
                "interface {} must declare at least one method",
                self.id
            ));
        }

        let mut method_ids = BTreeSet::new();
        for method in &self.methods {
            method.validate(&self.id)?;
            if !method_ids.insert(method.id.as_str()) {
                return Err(format!(
                    "interface {} declares duplicate method id {}",
                    self.id, method.id
                ));
            }
        }
        Ok(())
    }
}

/// A single user/agent-visible affordance method on an interface.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CapsuleAffordanceDescriptor {
    pub id: String,
    #[serde(default)]
    pub description: Option<String>,
    pub risk: AffordanceRisk,
    pub approval: AffordanceApprovalMode,
    pub audit: AffordanceAuditMode,
    #[serde(default)]
    pub resource: Option<String>,
    #[serde(default)]
    pub operation: Option<String>,
    #[serde(default)]
    pub input_schema: Option<Value>,
    #[serde(default)]
    pub output_schema: Option<Value>,
}

impl CapsuleAffordanceDescriptor {
    fn validate(&self, interface_id: &str) -> Result<(), String> {
        validate_descriptor_id("method id", &self.id)?;
        if let Some(resource) = &self.resource {
            if !is_allowed_uri_scheme(resource) {
                return Err(format!(
                    "interface {} method {} resource {} uses an unsupported URI scheme; allowed: elastos://, localhost://",
                    interface_id, self.id, resource
                ));
            }
        }
        if let Some(operation) = &self.operation {
            validate_descriptor_id("method operation", operation)?;
        }
        validate_json_schema(
            &format!("interface {} method {} input", interface_id, self.id),
            self.input_schema.as_ref(),
        )?;
        validate_json_schema(
            &format!("interface {} method {} output", interface_id, self.id),
            self.output_schema.as_ref(),
        )?;
        Ok(())
    }
}

/// Whether Runtime can run the affordance without human approval.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AffordanceApprovalMode {
    None,
    RuntimePolicy,
    User,
}

/// Safety class declared by the capsule for review and policy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AffordanceRisk {
    Read,
    Write,
    Launch,
    Payment,
    Rights,
    Actuator,
    Privileged,
}

/// Audit detail expected for an affordance invocation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AffordanceAuditMode {
    None,
    Summary,
    Event,
    Full,
}

/// Current schema identifier for v1 manifests
pub const SCHEMA_V1: &str = "elastos.capsule/v1";

impl CapsuleManifest {
    /// Validate manifest fields after deserialization.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != SCHEMA_V1 {
            return Err(format!(
                "unsupported schema \"{}\", expected \"{}\"",
                self.schema, SCHEMA_V1
            ));
        }

        if self.version.trim().is_empty() {
            return Err("manifest version must not be empty".to_string());
        }

        // Reject path traversal and absolute paths in entrypoint
        if self.entrypoint.contains("..") {
            return Err(format!(
                "entrypoint \"{}\" contains path traversal (\"..\" is not allowed)",
                self.entrypoint
            ));
        }
        if self.entrypoint.starts_with('/') || self.entrypoint.starts_with('\\') {
            return Err(format!(
                "entrypoint \"{}\" must be a relative path",
                self.entrypoint
            ));
        }

        if self.name.trim().is_empty() {
            return Err("manifest name must not be empty".to_string());
        }

        if self.role == CapsuleRole::Content && self.capsule_type != CapsuleType::Data {
            return Err("content capsules must use type=data".to_string());
        }
        if self.role.is_app_viewer_or_content() {
            if self.permissions.guest_network {
                return Err(format!(
                    "{} capsules cannot request guest_network; use a provider capsule for raw network backends",
                    self.role.as_str()
                ));
            }
            if self.permissions.carrier {
                return Err(format!(
                    "{} capsules cannot request carrier host execution; use a provider capsule for host services",
                    self.role.as_str()
                ));
            }
            if self.provides.is_some() {
                return Err(format!(
                    "{} capsules cannot provide protocol namespaces",
                    self.role.as_str()
                ));
            }
            if self
                .requires
                .iter()
                .any(|req| req.kind == RequirementKind::External)
            {
                return Err(format!(
                    "{} capsules cannot require external host dependencies; put protocol/tool adapters behind provider capsules",
                    self.role.as_str()
                ));
            }
            if self
                .providers
                .as_ref()
                .is_some_and(|providers| !providers.is_empty())
            {
                return Err(format!(
                    "{} capsules cannot choose provider implementations; the runtime/provider layer owns local vs network routing",
                    self.role.as_str()
                ));
            }
            if self
                .microvm
                .as_ref()
                .and_then(|microvm| microvm.http_port)
                .is_some()
            {
                return Err(format!(
                    "{} capsules cannot expose microVM HTTP ports; use a host adapter or a provider capsule",
                    self.role.as_str()
                ));
            }
        }
        if self.permissions.carrier
            && (self.role != CapsuleRole::Provider || self.provides.is_none())
        {
            return Err(
                "carrier host execution requires a provider capsule with an explicit provides namespace"
                    .to_string(),
            );
        }
        if self.permissions.guest_network
            && (self.role != CapsuleRole::Provider || self.provides.is_none())
        {
            return Err(
                "guest_network requires a provider capsule with an explicit provides namespace"
                    .to_string(),
            );
        }
        if self.provides.is_some() && self.role != CapsuleRole::Provider {
            return Err("only provider capsules can provide protocol namespaces".to_string());
        }
        if self.role == CapsuleRole::Provider && self.provides.is_none() {
            return Err("provider capsules must declare a provides namespace".to_string());
        }
        if self.role == CapsuleRole::Provider {
            let authority = self
                .authority
                .as_ref()
                .ok_or_else(|| "provider capsules must declare authority metadata".to_string())?;
            authority.validate()?;
        } else if self.authority.is_some() {
            return Err("authority metadata is only valid on provider capsules".to_string());
        }

        // Reject path traversal in viewer field (same rules as entrypoint)
        if let Some(viewer) = &self.viewer {
            if self.role != CapsuleRole::Content {
                return Err("viewer is only valid on content capsules".to_string());
            }
            if viewer.contains("..") {
                return Err(format!(
                    "viewer \"{}\" contains path traversal (\"..\" is not allowed)",
                    viewer
                ));
            }
            if viewer.starts_with('/') || viewer.starts_with('\\') {
                return Err(format!(
                    "viewer \"{}\" must be a relative path or capsule name",
                    viewer
                ));
            }
        }

        for req in &self.requires {
            if req.name.trim().is_empty() {
                return Err("requirement name must not be empty".to_string());
            }
        }

        if let Some(provides) = &self.provides {
            if !is_allowed_uri_scheme(provides) {
                return Err(format!(
                    "unsupported URI scheme in provides \"{}\"; allowed: elastos://, localhost://",
                    provides
                ));
            }
        }

        for cap in &self.capabilities {
            if !is_allowed_uri_scheme(cap) {
                return Err(format!(
                    "unsupported URI scheme in capability \"{}\"; allowed: elastos://, localhost://",
                    cap
                ));
            }
            if self.role.is_app_viewer_or_content() {
                if is_system_only_backend_resource(cap) {
                    return Err(format!(
                        "{} capsules cannot request system-only backend capability \"{}\"; use the product/provider contract instead",
                        self.role.as_str(),
                        cap
                    ));
                }
                if is_runtime_system_service_resource(cap) {
                    return Err(format!(
                        "{} capsules cannot request runtime system-service storage \"{}\"",
                        self.role.as_str(),
                        cap
                    ));
                }
            }
        }

        self.validate_interfaces()?;

        for storage in &self.permissions.storage {
            if !is_allowed_uri_scheme(storage) {
                return Err(format!(
                    "unsupported URI scheme in storage permission \"{}\"; allowed: elastos://, localhost://",
                    storage
                ));
            }
            if self.role.is_app_viewer_or_content() && is_runtime_system_service_resource(storage) {
                return Err(format!(
                    "{} capsules cannot request runtime system-service storage \"{}\"",
                    self.role.as_str(),
                    storage
                ));
            }
            if self.role.is_app_viewer_or_content() && is_system_only_backend_resource(storage) {
                return Err(format!(
                    "{} capsules cannot request system-only backend storage \"{}\"; use the product/provider contract instead",
                    self.role.as_str(),
                    storage
                ));
            }
        }

        Ok(())
    }

    /// Returns true if this manifest uses the v1 schema format.
    pub fn is_v1(&self) -> bool {
        self.schema == SCHEMA_V1
    }

    fn validate_interfaces(&self) -> Result<(), String> {
        let mut interface_ids = BTreeSet::new();
        for interface in &self.interfaces {
            interface.validate()?;
            if !interface_ids.insert(interface.id.as_str()) {
                return Err(format!(
                    "manifest declares duplicate interface id {}",
                    interface.id
                ));
            }
        }
        Ok(())
    }
}

fn is_allowed_uri_scheme(uri: &str) -> bool {
    is_supported_resource_scheme(uri)
}

fn validate_descriptor_id(kind: &str, value: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{} must not be empty", kind));
    }
    if trimmed != value {
        return Err(format!("{} must not contain surrounding whitespace", kind));
    }
    if value.len() > 128 {
        return Err(format!("{} must be 128 bytes or shorter", kind));
    }
    if value
        .chars()
        .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err(format!(
            "{} must not contain whitespace or control characters",
            kind
        ));
    }
    Ok(())
}

fn validate_json_schema(kind: &str, schema: Option<&Value>) -> Result<(), String> {
    let Some(schema) = schema else {
        return Ok(());
    };
    if schema.is_object() || schema.is_boolean() {
        return Ok(());
    }
    Err(format!("{} schema must be a JSON object or boolean", kind))
}

/// Provider-only authority metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderAuthority {
    pub reason: String,
    pub capabilities: Vec<ProviderCapabilitySchema>,
    pub audit_events: Vec<String>,
}

impl ProviderAuthority {
    fn validate(&self) -> Result<(), String> {
        if self.reason.trim().is_empty() {
            return Err("provider authority reason must not be empty".to_string());
        }
        if self.capabilities.is_empty() {
            return Err(
                "provider authority must declare at least one capability schema".to_string(),
            );
        }
        if self.audit_events.is_empty() {
            return Err("provider authority must declare expected audit events".to_string());
        }
        for capability in &self.capabilities {
            capability.validate()?;
        }
        for event in &self.audit_events {
            if event.trim().is_empty() {
                return Err("provider authority audit event must not be empty".to_string());
            }
        }
        Ok(())
    }
}

/// A reviewable provider capability surface.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderCapabilitySchema {
    pub resource: String,
    pub actions: Vec<String>,
    pub operations: Vec<String>,
}

impl ProviderCapabilitySchema {
    fn validate(&self) -> Result<(), String> {
        if !is_allowed_uri_scheme(&self.resource) {
            return Err(format!(
                "unsupported URI scheme in provider authority resource \"{}\"; allowed: elastos://, localhost://",
                self.resource
            ));
        }
        if self.actions.is_empty() {
            return Err("provider authority capability must declare actions".to_string());
        }
        if self.operations.is_empty() {
            return Err("provider authority capability must declare operations".to_string());
        }
        for action in &self.actions {
            if !matches!(
                action.as_str(),
                "read" | "write" | "execute" | "delete" | "message" | "admin"
            ) {
                return Err(format!(
                    "unsupported provider authority action \"{}\"",
                    action
                ));
            }
        }
        for operation in &self.operations {
            if operation.trim().is_empty() {
                return Err("provider authority operation must not be empty".to_string());
            }
        }
        Ok(())
    }
}

/// Supported capsule types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CapsuleType {
    Wasm,
    MicroVM,
    Oci,
    Media,
    Data,
}

/// Product role of a capsule.
///
/// This is distinct from the execution substrate (`type`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CapsuleRole {
    Shell,
    App,
    Viewer,
    Provider,
    Content,
}

impl CapsuleRole {
    pub fn is_shell_launchable(&self) -> bool {
        matches!(self, Self::Shell | Self::App | Self::Viewer)
    }

    pub fn is_content(&self) -> bool {
        matches!(self, Self::Content)
    }

    pub fn is_app_viewer_or_content(&self) -> bool {
        matches!(self, Self::App | Self::Viewer | Self::Content)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::App => "app",
            Self::Viewer => "viewer",
            Self::Provider => "provider",
            Self::Content => "content",
        }
    }
}

/// Resource limits for a capsule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    #[serde(default = "default_memory")]
    pub memory_mb: u32,

    #[serde(default = "default_cpu")]
    pub cpu_shares: u32,

    #[serde(default)]
    pub gpu: bool,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            memory_mb: default_memory(),
            cpu_shares: default_cpu(),
            gpu: false,
        }
    }
}

fn default_memory() -> u32 {
    64
}

fn default_cpu() -> u32 {
    100
}

/// Permissions requested by a capsule
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Permissions {
    /// Carrier-plane service: runs as a host process (not in a VM).
    /// Used by WebSpace providers that need real network/system access.
    #[serde(default)]
    pub carrier: bool,

    /// Request explicit guest IP networking (TAP) for the microVM.
    /// Default: false — capsules use the serial Carrier bridge (rootless).
    /// Set to true only for compatibility capsules that genuinely need a guest
    /// NIC (for example, a provider exposing a TCP port). Requires
    /// CAP_NET_ADMIN on the runtime.
    #[serde(default)]
    pub guest_network: bool,

    #[serde(default)]
    pub storage: Vec<String>,

    #[serde(default)]
    pub messaging: Vec<String>,
}

/// Configuration for MicroVM capsules (crosvm)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MicroVmConfig {
    /// Path to the vmlinux kernel (relative to capsule directory)
    #[serde(default)]
    pub kernel: Option<String>,

    /// Kernel boot arguments
    #[serde(default = "default_boot_args")]
    pub boot_args: String,

    /// HTTP port to forward for browser access
    #[serde(default)]
    pub http_port: Option<u16>,

    /// Number of vCPUs (default: 1)
    #[serde(default)]
    pub vcpu_count: Option<u8>,

    /// CID of the rootfs image for content-addressed loading
    #[serde(default)]
    pub rootfs_cid: Option<String>,

    /// CID of the kernel image for content-addressed loading
    #[serde(default)]
    pub kernel_cid: Option<String>,

    /// Size of the rootfs in bytes (for progress display during download)
    #[serde(default)]
    pub rootfs_size: Option<u64>,

    /// Persistent data disk size in MB (survives VM restarts)
    #[serde(default)]
    pub persistent_storage_mb: Option<u32>,
}

fn default_boot_args() -> String {
    "console=ttyS0 reboot=k panic=1 init=/init".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Core parse/validation tests (strict v1 schema) ──────────────

    #[test]
    fn test_parse_manifest() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "test-capsule",
            "role": "app",
            "type": "wasm",
            "entrypoint": "main.wasm"
        }"#;

        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.name, "test-capsule");
        assert_eq!(manifest.capsule_type, CapsuleType::Wasm);
        assert_eq!(manifest.resources.memory_mb, 64);
        assert!(manifest.is_v1());
    }

    #[test]
    fn test_parse_full_manifest() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "full-capsule",
            "description": "A test capsule",
            "author": "test",
            "role": "app",
            "type": "wasm",
            "entrypoint": "main.wasm",
            "resources": {
                "memory_mb": 128,
                "cpu_shares": 200
            },
            "permissions": {
                "storage": ["read", "write"],
                "messaging": ["other-capsule"]
            }
        }"#;

        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.resources.memory_mb, 128);
        assert_eq!(manifest.permissions.storage, vec!["read", "write"]);
    }

    #[test]
    fn test_parse_manifest_with_typed_interfaces() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "documents",
            "description": "Document viewer",
            "author": "elastos",
            "role": "viewer",
            "type": "wasm",
            "entrypoint": "documents.wasm",
            "interfaces": [{
                "id": "elastos.documents.viewer",
                "version": "0.1.0",
                "description": "Open and preview document objects.",
                "methods": [{
                    "id": "document.open",
                    "description": "Open a document object in the viewer.",
                    "risk": "read",
                    "approval": "runtime_policy",
                    "audit": "event",
                    "resource": "localhost://Users/self/Documents/*",
                    "operation": "open",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "uri": { "type": "string" }
                        },
                        "required": ["uri"]
                    },
                    "output_schema": {
                        "type": "object"
                    }
                }]
            }]
        }"#;

        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        manifest.validate().unwrap();
        assert_eq!(manifest.interfaces.len(), 1);
        let interface = &manifest.interfaces[0];
        assert_eq!(interface.id, "elastos.documents.viewer");
        assert_eq!(interface.methods[0].id, "document.open");
        assert_eq!(interface.methods[0].risk, AffordanceRisk::Read);
        assert_eq!(
            interface.methods[0].approval,
            AffordanceApprovalMode::RuntimePolicy
        );
    }

    #[test]
    fn test_typed_interface_rejects_duplicate_interface_ids() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "duplicate-interface",
            "role": "app",
            "type": "wasm",
            "entrypoint": "main.wasm",
            "interfaces": [
                {
                    "id": "elastos.test",
                    "version": "0.1.0",
                    "methods": [{
                        "id": "test.read",
                        "risk": "read",
                        "approval": "runtime_policy",
                        "audit": "event"
                    }]
                },
                {
                    "id": "elastos.test",
                    "version": "0.1.0",
                    "methods": [{
                        "id": "test.write",
                        "risk": "write",
                        "approval": "user",
                        "audit": "full"
                    }]
                }
            ]
        }"#;

        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(err.contains("duplicate interface id"));
    }

    #[test]
    fn test_typed_interface_rejects_duplicate_method_ids() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "duplicate-method",
            "role": "app",
            "type": "wasm",
            "entrypoint": "main.wasm",
            "interfaces": [{
                "id": "elastos.test",
                "version": "0.1.0",
                "methods": [
                    {
                        "id": "test.read",
                        "risk": "read",
                        "approval": "runtime_policy",
                        "audit": "event"
                    },
                    {
                        "id": "test.read",
                        "risk": "read",
                        "approval": "runtime_policy",
                        "audit": "event"
                    }
                ]
            }]
        }"#;

        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(err.contains("duplicate method id"));
    }

    #[test]
    fn test_typed_interface_rejects_unknown_method_fields() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "unknown-method-field",
            "role": "app",
            "type": "wasm",
            "entrypoint": "main.wasm",
            "interfaces": [{
                "id": "elastos.test",
                "version": "0.1.0",
                "methods": [{
                    "id": "test.read",
                    "risk": "read",
                    "approval": "runtime_policy",
                    "audit": "event",
                    "ambient_host_access": true
                }]
            }]
        }"#;

        let err = serde_json::from_str::<CapsuleManifest>(json).unwrap_err();
        assert!(err.to_string().contains("unknown field"));
    }

    #[test]
    fn test_typed_interface_rejects_unsupported_resource_scheme() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "unsupported-resource-scheme",
            "role": "app",
            "type": "wasm",
            "entrypoint": "main.wasm",
            "interfaces": [{
                "id": "elastos.test",
                "version": "0.1.0",
                "methods": [{
                    "id": "test.fetch",
                    "risk": "read",
                    "approval": "runtime_policy",
                    "audit": "event",
                    "resource": "https://example.invalid/object"
                }]
            }]
        }"#;

        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(err.contains("unsupported URI scheme"));
    }

    #[test]
    fn test_typed_interface_rejects_non_schema_json_values() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "bad-schema-shape",
            "role": "app",
            "type": "wasm",
            "entrypoint": "main.wasm",
            "interfaces": [{
                "id": "elastos.test",
                "version": "0.1.0",
                "methods": [{
                    "id": "test.read",
                    "risk": "read",
                    "approval": "runtime_policy",
                    "audit": "event",
                    "input_schema": "string"
                }]
            }]
        }"#;

        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(err.contains("schema must be a JSON object or boolean"));
    }

    #[test]
    fn test_typed_interface_rejects_whitespace_in_descriptor_ids() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "bad-method-id",
            "role": "app",
            "type": "wasm",
            "entrypoint": "main.wasm",
            "interfaces": [{
                "id": "elastos.test",
                "version": "0.1.0",
                "methods": [{
                    "id": "test read",
                    "risk": "read",
                    "approval": "runtime_policy",
                    "audit": "event"
                }]
            }]
        }"#;

        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(err.contains("must not contain whitespace"));
    }

    #[test]
    fn test_parse_microvm_manifest() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "fixture-microvm",
            "role": "app",
            "type": "microvm",
            "entrypoint": "rootfs.ext4",
            "resources": {
                "memory_mb": 512,
                "cpu_shares": 200
            },
            "permissions": {
                "storage": ["localhost://Users/self/Documents/*"]
            },
            "microvm": {
                "kernel": "kernel/vmlinux",
                "http_port": 4100,
                "vcpu_count": 2
            }
        }"#;

        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.name, "fixture-microvm");
        assert_eq!(manifest.capsule_type, CapsuleType::MicroVM);
        assert_eq!(manifest.entrypoint, "rootfs.ext4");
        assert_eq!(manifest.resources.memory_mb, 512);

        let microvm = manifest.microvm.unwrap();
        assert_eq!(microvm.kernel, Some("kernel/vmlinux".to_string()));
        assert_eq!(microvm.http_port, Some(4100));
        assert_eq!(microvm.vcpu_count, Some(2));
        assert_eq!(
            microvm.boot_args,
            "console=ttyS0 reboot=k panic=1 init=/init"
        );
    }

    #[test]
    fn test_parse_microvm_manifest_with_cids() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "fixture-microvm-cid",
            "role": "app",
            "type": "microvm",
            "entrypoint": "rootfs.ext4",
            "resources": {
                "memory_mb": 512
            },
            "microvm": {
                "http_port": 4100,
                "rootfs_cid": "QmXoypizjW3WknFiJnKLwHCnL72vedxjQkDDP1mXWo6uco",
                "kernel_cid": "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG",
                "rootfs_size": 2147483648
            }
        }"#;

        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.name, "fixture-microvm-cid");
        assert_eq!(manifest.capsule_type, CapsuleType::MicroVM);

        let microvm = manifest.microvm.unwrap();
        assert_eq!(
            microvm.rootfs_cid,
            Some("QmXoypizjW3WknFiJnKLwHCnL72vedxjQkDDP1mXWo6uco".to_string())
        );
        assert_eq!(
            microvm.kernel_cid,
            Some("QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG".to_string())
        );
        assert_eq!(microvm.rootfs_size, Some(2147483648));
    }

    #[test]
    fn test_parse_microvm_persistent_storage() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "fixture-microvm",
            "role": "app",
            "type": "microvm",
            "entrypoint": "rootfs.ext4",
            "microvm": {
                "http_port": 4100,
                "vcpu_count": 2,
                "persistent_storage_mb": 2048
            }
        }"#;

        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let microvm = manifest.microvm.unwrap();
        assert_eq!(microvm.persistent_storage_mb, Some(2048));
    }

    #[test]
    fn test_parse_manifest_with_providers() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "test-capsule",
            "role": "app",
            "type": "microvm",
            "entrypoint": "rootfs.ext4",
            "providers": {
                "local": "built-in",
                "cloud": "elastos://QmProviderCID"
            }
        }"#;

        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let providers = manifest.providers.unwrap();
        assert_eq!(providers.get("local").unwrap(), "built-in");
        assert_eq!(providers.get("cloud").unwrap(), "elastos://QmProviderCID");
    }

    #[test]
    fn test_parse_manifest_without_providers() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "test-capsule",
            "role": "app",
            "type": "wasm",
            "entrypoint": "main.wasm"
        }"#;

        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        assert!(manifest.providers.is_none());
    }

    #[test]
    fn test_parse_microvm_no_persistent_storage() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "test",
            "role": "app",
            "type": "microvm",
            "entrypoint": "rootfs.ext4",
            "microvm": {
                "http_port": 4100
            }
        }"#;

        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let microvm = manifest.microvm.unwrap();
        assert_eq!(microvm.persistent_storage_mb, None);
    }

    #[test]
    fn test_parse_data_capsule_with_viewer() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "gba-ucity",
            "role": "content",
            "type": "data",
            "entrypoint": "ucity.gba",
            "viewer": "gba-emulator",
            "permissions": { "storage": ["localhost://Users/self/.AppData/LocalHost/GBA/gba-ucity/*"] }
        }"#;

        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.name, "gba-ucity");
        assert_eq!(manifest.capsule_type, CapsuleType::Data);
        assert_eq!(manifest.viewer, Some("gba-emulator".to_string()));
        manifest.validate().unwrap();
    }

    #[test]
    fn test_viewer_path_traversal_rejected() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "evil",
            "role": "content",
            "type": "data",
            "entrypoint": "data.bin",
            "viewer": "../../../etc/passwd"
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(
            err.contains("path traversal"),
            "expected traversal error: {}",
            err
        );
    }

    #[test]
    fn test_viewer_rejected_for_non_content_role() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "bad-viewer",
            "role": "viewer",
            "type": "data",
            "entrypoint": "index.html",
            "viewer": "documents"
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(err.contains("viewer is only valid on content capsules"));
    }

    #[test]
    fn test_content_role_requires_data_type() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "bad-content",
            "role": "content",
            "type": "wasm",
            "entrypoint": "main.wasm"
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(err.contains("content capsules must use type=data"));
    }

    #[test]
    fn test_parse_manifest_without_viewer() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "test",
            "role": "app",
            "type": "wasm",
            "entrypoint": "main.wasm"
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        assert!(manifest.viewer.is_none());
    }

    #[test]
    fn test_validate_version_ok() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "test",
            "role": "app",
            "type": "wasm",
            "entrypoint": "main.wasm"
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn test_validate_version_empty() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "   ",
            "name": "test",
            "role": "app",
            "type": "wasm",
            "entrypoint": "main.wasm"
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(err.contains("version must not be empty"));
    }

    #[test]
    fn test_reject_missing_schema_field() {
        let json = r#"{
            "version": "0.1.0",
            "name": "test",
            "role": "app",
            "type": "wasm",
            "entrypoint": "main.wasm"
        }"#;
        let err = serde_json::from_str::<CapsuleManifest>(json).unwrap_err();
        assert!(err.to_string().contains("missing field `schema`"));
    }

    #[test]
    fn test_validate_entrypoint_traversal() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "evil",
            "role": "app",
            "type": "wasm",
            "entrypoint": "../../../etc/passwd"
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(err.contains("path traversal"));
    }

    #[test]
    fn test_validate_entrypoint_absolute_path() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "evil",
            "role": "app",
            "type": "wasm",
            "entrypoint": "/etc/passwd"
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(err.contains("relative path"));
    }

    #[test]
    fn test_validate_entrypoint_ok() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "test",
            "role": "app",
            "type": "wasm",
            "entrypoint": "my-capsule.wasm"
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        assert!(manifest.validate().is_ok());
    }

    // ── v1 schema format tests ───────────────────────────────────────

    #[test]
    fn test_parse_v1_manifest() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "name": "ipfs-provider",
            "version": "0.1.0",
            "role": "provider",
            "type": "microvm",
            "entrypoint": "ipfs-provider",
            "requires": [{"name":"kubo","kind":"external"}],
            "provides": "elastos://ipfs/*",
            "authority": {
                "reason": "Low-level content availability backend owned by the runtime.",
                "capabilities": [{
                    "resource": "elastos://ipfs/*",
                    "actions": ["read", "write", "delete", "admin"],
                    "operations": ["add_bytes", "cat", "download_directory", "unpin", "status"]
                }],
                "audit_events": ["ipfs.publish", "ipfs.fetch", "ipfs.unpin"]
            },
            "capabilities": ["localhost://ElastOS/SystemServices/IPFS/*"],
            "resources": { "memory_mb": 128, "gpu": false }
        }"#;

        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        assert!(manifest.is_v1());
        assert!(manifest.validate().is_ok());
        assert_eq!(manifest.name, "ipfs-provider");
        assert_eq!(manifest.version, "0.1.0");
        assert_eq!(manifest.capsule_type, CapsuleType::MicroVM);
        assert_eq!(
            manifest.requires,
            vec![CapsuleRequirement {
                name: "kubo".to_string(),
                kind: RequirementKind::External
            }]
        );
        assert_eq!(manifest.provides, Some("elastos://ipfs/*".to_string()));
        assert_eq!(
            manifest.capabilities,
            vec!["localhost://ElastOS/SystemServices/IPFS/*"]
        );
        assert_eq!(manifest.resources.memory_mb, 128);
        assert!(!manifest.resources.gpu);
    }

    #[test]
    fn test_v1_with_gpu() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "name": "llama-provider",
            "version": "0.1.0",
            "role": "provider",
            "type": "microvm",
            "entrypoint": "llama-provider",
            "resources": { "memory_mb": 4096, "gpu": true }
        }"#;

        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        assert!(manifest.resources.gpu);
    }

    #[test]
    fn test_v1_validate_bad_schema() {
        let json = r#"{
            "schema": "elastos.capsule/v99",
            "name": "test",
            "version": "0.1.0",
            "role": "app",
            "type": "microvm",
            "entrypoint": "test"
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(err.contains("v99"));
        assert!(err.contains("elastos.capsule/v1"));
    }

    #[test]
    fn test_v1_reject_disallowed_provides_scheme() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "name": "bad-provides",
            "version": "0.1.0",
            "role": "provider",
            "type": "microvm",
            "entrypoint": "rootfs.ext4",
            "provides": "https://example.com/*",
            "authority": {
                "reason": "Bad provider authority.",
                "capabilities": [{
                    "resource": "elastos://bad/*",
                    "actions": ["execute"],
                    "operations": ["status"]
                }],
                "audit_events": ["bad.status"]
            }
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(err.contains("unsupported URI scheme"));
        assert!(err.contains("provides"));
    }

    #[test]
    fn test_v1_reject_disallowed_capability_scheme() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "name": "bad-capability",
            "version": "0.1.0",
            "role": "app",
            "type": "microvm",
            "entrypoint": "rootfs.ext4",
            "capabilities": ["ftp://example.com/resource"]
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(err.contains("unsupported URI scheme"));
        assert!(err.contains("capability"));
    }

    #[test]
    fn test_app_capsule_rejects_system_backend_capability() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "name": "bad-backend-capability",
            "version": "0.1.0",
            "role": "app",
            "type": "microvm",
            "entrypoint": "rootfs.ext4",
            "capabilities": ["elastos://ipfs/add"]
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(err.contains("system-only backend capability"));
    }

    #[test]
    fn test_viewer_capsule_allows_content_contract_capability() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "name": "content-aware-viewer",
            "version": "0.1.0",
            "role": "viewer",
            "type": "data",
            "entrypoint": "index.html",
            "capabilities": ["elastos://content/fetch"]
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn test_content_capsule_rejects_runtime_system_service_storage() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "name": "bad-system-service-capability",
            "version": "0.1.0",
            "role": "content",
            "type": "data",
            "entrypoint": "asset.bin",
            "permissions": {
                "storage": ["localhost://ElastOS/SystemServices/IPFS/*"]
            }
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(err.contains("runtime system-service storage"));
    }

    #[test]
    fn test_app_capsule_rejects_system_backend_storage() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "name": "bad-system-backend-storage",
            "version": "0.1.0",
            "role": "app",
            "type": "wasm",
            "entrypoint": "main.wasm",
            "permissions": {
                "storage": ["elastos://ipfs-provider/add"]
            }
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(err.contains("system-only backend storage"));
    }

    #[test]
    fn test_v1_reject_disallowed_storage_permission_scheme() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "name": "bad-storage-permission",
            "version": "0.1.0",
            "role": "app",
            "type": "microvm",
            "entrypoint": "rootfs.ext4",
            "permissions": {
                "storage": ["../host"]
            }
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(err.contains("unsupported URI scheme in storage permission"));
    }

    #[test]
    fn test_v1_defaults() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "name": "minimal",
            "version": "0.1.0",
            "role": "app",
            "type": "microvm",
            "entrypoint": "minimal"
        }"#;

        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        assert!(manifest.validate().is_ok());
        assert!(manifest.requires.is_empty());
        assert!(manifest.provides.is_none());
        assert!(manifest.capabilities.is_empty());
        assert!(!manifest.resources.gpu);
    }

    #[test]
    fn test_reject_legacy_dependencies_field() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "name": "bad",
            "version": "0.1.0",
            "role": "app",
            "type": "microvm",
            "entrypoint": "bad",
            "dependencies": ["kubo"]
        }"#;
        let err = serde_json::from_str::<CapsuleManifest>(json).unwrap_err();
        assert!(err.to_string().contains("unknown field `dependencies`"));
    }

    #[test]
    fn test_guest_network_defaults_false() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "chat",
            "role": "app",
            "type": "microvm",
            "entrypoint": "rootfs.ext4"
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        assert!(!manifest.permissions.guest_network);
    }

    #[test]
    fn test_guest_network_explicit_true() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "my-provider",
            "role": "provider",
            "type": "microvm",
            "entrypoint": "rootfs.ext4",
            "provides": "elastos://my/*",
            "authority": {
                "reason": "Test provider authority.",
                "capabilities": [{
                    "resource": "elastos://my/*",
                    "actions": ["execute"],
                    "operations": ["status"]
                }],
                "audit_events": ["my.status"]
            },
            "permissions": {
                "guest_network": true
            }
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        assert!(manifest.permissions.guest_network);
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn test_guest_network_explicit_false() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "chat",
            "role": "app",
            "type": "microvm",
            "entrypoint": "rootfs.ext4",
            "permissions": {
                "guest_network": false
            }
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        assert!(!manifest.permissions.guest_network);
    }

    #[test]
    fn test_app_capsule_is_carrier_only() {
        // Regular app capsule: no provides, no guest_network → Carrier-only
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "chat",
            "role": "app",
            "type": "microvm",
            "entrypoint": "rootfs.ext4",
            "permissions": {
                "messaging": ["*"]
            }
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        assert!(
            !manifest.permissions.guest_network,
            "app capsules are Carrier-only by default"
        );
        assert!(
            !manifest.permissions.carrier,
            "app capsules don't run on the host"
        );
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn test_app_capsule_guest_network_rejected() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "bad-app",
            "role": "app",
            "type": "microvm",
            "entrypoint": "rootfs.ext4",
            "permissions": {
                "guest_network": true
            }
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(err.contains("app capsules cannot request guest_network"));
    }

    #[test]
    fn test_app_capsule_external_requirement_rejected() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "bad-app",
            "role": "app",
            "type": "microvm",
            "entrypoint": "rootfs.ext4",
            "requires": [{"name": "kubo", "kind": "external"}]
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(err.contains("app capsules cannot require external host dependencies"));
    }

    #[test]
    fn test_viewer_capsule_provider_source_override_rejected() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "bad-viewer",
            "role": "viewer",
            "type": "data",
            "entrypoint": "index.html",
            "providers": {
                "content": "elastos://QmProviderCID"
            }
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(err.contains("viewer capsules cannot choose provider implementations"));
    }

    #[test]
    fn test_app_capsule_microvm_http_port_rejected() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "bad-app",
            "role": "app",
            "type": "microvm",
            "entrypoint": "rootfs.ext4",
            "microvm": {
                "http_port": 4100
            }
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(err.contains("app capsules cannot expose microVM HTTP ports"));
    }

    #[test]
    fn test_viewer_capsule_carrier_permission_rejected() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "bad-viewer",
            "role": "viewer",
            "type": "wasm",
            "entrypoint": "viewer.wasm",
            "permissions": {
                "carrier": true
            }
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(err.contains("viewer capsules cannot request carrier host execution"));
    }

    #[test]
    fn test_content_capsule_provides_rejected() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "bad-content",
            "role": "content",
            "type": "data",
            "entrypoint": "asset.bin",
            "provides": "elastos://bad/*"
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(err.contains("content capsules cannot provide protocol namespaces"));
    }

    #[test]
    fn test_carrier_permission_requires_provider_with_namespace() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "bad-provider",
            "role": "provider",
            "type": "microvm",
            "entrypoint": "rootfs.ext4",
            "permissions": {
                "carrier": true
            }
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(err.contains("carrier host execution requires a provider capsule"));
    }

    #[test]
    fn test_provider_requires_provides_namespace() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "webspace-provider",
            "role": "provider",
            "type": "microvm",
            "entrypoint": "rootfs.ext4",
            "capabilities": ["localhost://WebSpaces/*"]
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(err.contains("provider capsules must declare a provides namespace"));
    }

    #[test]
    fn test_provider_requires_authority_metadata() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "thin-provider",
            "role": "provider",
            "type": "microvm",
            "entrypoint": "rootfs.ext4",
            "provides": "elastos://thin/*"
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(err.contains("provider capsules must declare authority metadata"));
    }

    #[test]
    fn test_authority_metadata_rejected_for_app_capsules() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "bad-app-authority",
            "role": "app",
            "type": "wasm",
            "entrypoint": "main.wasm",
            "authority": {
                "reason": "not allowed",
                "capabilities": [{
                    "resource": "elastos://peer/*",
                    "actions": ["execute"],
                    "operations": ["connect"]
                }],
                "audit_events": ["peer.connect"]
            }
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let err = manifest.validate().unwrap_err();
        assert!(err.contains("authority metadata is only valid on provider capsules"));
    }

    #[test]
    fn test_provider_with_guest_network() {
        // Provider capsule that explicitly requests guest IP networking
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "localhost-provider",
            "role": "provider",
            "type": "microvm",
            "entrypoint": "rootfs.ext4",
            "provides": "localhost://Users/*",
            "authority": {
                "reason": "Test localhost provider authority.",
                "capabilities": [{
                    "resource": "localhost://Users/*",
                    "actions": ["read", "write", "delete"],
                    "operations": ["read", "write", "delete"]
                }],
                "audit_events": ["localhost.read", "localhost.write"]
            },
            "permissions": {
                "guest_network": true,
                "storage": ["localhost://Users/"]
            }
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        assert!(
            manifest.permissions.guest_network,
            "provider capsule explicitly requests TAP"
        );
        assert!(manifest.provides.is_some());
        assert!(manifest.validate().is_ok());
    }
}
