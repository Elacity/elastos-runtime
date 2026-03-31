//! Capsule manifest types

use serde::{Deserialize, Serialize};

use crate::localhost::is_supported_resource_scheme;

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

    /// Capabilities this capsule needs from other capsules (URI list)
    #[serde(default)]
    pub capabilities: Vec<String>,

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

        // Reject path traversal in viewer field (same rules as entrypoint)
        if let Some(viewer) = &self.viewer {
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
        }

        Ok(())
    }

    /// Returns true if this manifest uses the v1 schema format.
    pub fn is_v1(&self) -> bool {
        self.schema == SCHEMA_V1
    }
}

fn is_allowed_uri_scheme(uri: &str) -> bool {
    is_supported_resource_scheme(uri)
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
    fn test_parse_microvm_manifest() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "puter-shell",
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
        assert_eq!(manifest.name, "puter-shell");
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
            "name": "puter-shell-cid",
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
        assert_eq!(manifest.name, "puter-shell-cid");
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
            "name": "puter-shell",
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
            "type": "microvm",
            "entrypoint": "rootfs.ext4",
            "providers": {
                "local": "built-in",
                "google": "elastos://QmProviderCID"
            }
        }"#;

        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        let providers = manifest.providers.unwrap();
        assert_eq!(providers.get("local").unwrap(), "built-in");
        assert_eq!(providers.get("google").unwrap(), "elastos://QmProviderCID");
    }

    #[test]
    fn test_parse_manifest_without_providers() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "test-capsule",
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
    fn test_parse_manifest_without_viewer() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "test",
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
            "type": "microvm",
            "entrypoint": "ipfs-provider",
            "requires": [{"name":"kubo","kind":"external"}],
            "provides": "elastos://ipfs/*",
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
            "type": "microvm",
            "entrypoint": "rootfs.ext4",
            "provides": "https://example.com/*"
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
    fn test_v1_defaults() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "name": "minimal",
            "version": "0.1.0",
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
            "type": "microvm",
            "entrypoint": "rootfs.ext4",
            "provides": "elastos://my/*",
            "permissions": {
                "guest_network": true
            }
        }"#;
        let manifest: CapsuleManifest = serde_json::from_str(json).unwrap();
        assert!(manifest.permissions.guest_network);
    }

    #[test]
    fn test_guest_network_explicit_false() {
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "chat",
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
    }

    #[test]
    fn test_provider_with_guest_network() {
        // Provider capsule that explicitly requests guest IP networking
        let json = r#"{
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "localhost-provider",
            "type": "microvm",
            "entrypoint": "rootfs.ext4",
            "provides": "localhost://Users/*",
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
    }
}
