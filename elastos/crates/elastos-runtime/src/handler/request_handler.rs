//! Runtime-control request handler implementation.
//!
//! Processes RuntimeRequest messages and returns RuntimeResponse.
//! Enforces authorization and delegates to appropriate managers.
//! This is not the public capsule-kernel ABI exposed by `elastos-guest`.

// Used by lib crate (tests, API handlers) but not directly by main.rs binary

use std::sync::Arc;
use tokio::sync::RwLock;

use elastos_common::localhost::{is_supported_resource_scheme, rooted_localhost_uri};
use elastos_namespace::ContentUri;

use crate::capability::token::{Action, ResourceId, TokenConstraints as InternalConstraints};
use crate::capability::CapabilityManager;
use crate::capsule::{prepare_fetched_capsule, CapsuleId, CapsuleInfo, CapsuleManager};
use crate::content::ContentResolver;
use crate::messaging::Message;
use crate::messaging::MessageChannel;
use crate::primitives::audit::{AuditLog, StopReason, TrustLevel};
use crate::primitives::time::SecureTimestamp;
use crate::provider::{ProviderRegistry, ResourceAction};

use super::protocol::*;

/// The shell capsule ID (orchestrator)
/// Only this capsule can perform privileged operations
#[derive(Debug, Clone)]
pub struct ShellId(Option<CapsuleId>);

impl ShellId {
    pub fn new() -> Self {
        Self(None)
    }

    pub fn set(&mut self, id: CapsuleId) {
        self.0 = Some(id);
    }

    pub fn is_shell(&self, id: &CapsuleId) -> bool {
        self.0.as_ref() == Some(id)
    }

    pub fn get(&self) -> Option<&CapsuleId> {
        self.0.as_ref()
    }
}

impl Default for ShellId {
    fn default() -> Self {
        Self::new()
    }
}

/// Request handler for shell control and explicitly authorized internal flows.
pub struct RequestHandler {
    /// Capsule manager
    capsule_manager: Arc<CapsuleManager>,
    /// Capability manager
    capability_manager: Arc<CapabilityManager>,
    /// Message channel
    message_channel: Arc<MessageChannel>,
    /// Content resolver
    content_resolver: Arc<ContentResolver>,
    /// Audit log
    _audit_log: Arc<AuditLog>,
    /// Provider registry for resource routing
    provider_registry: Option<Arc<ProviderRegistry>>,
    /// Runtime version
    version: String,
    /// Shell capsule ID (has orchestrator privilege)
    shell_id: RwLock<ShellId>,
}

impl RequestHandler {
    /// Create a new request handler
    pub fn new(
        capsule_manager: Arc<CapsuleManager>,
        capability_manager: Arc<CapabilityManager>,
        message_channel: Arc<MessageChannel>,
        content_resolver: Arc<ContentResolver>,
        audit_log: Arc<AuditLog>,
        version: String,
        provider_registry: Option<Arc<ProviderRegistry>>,
    ) -> Self {
        Self {
            capsule_manager,
            capability_manager,
            message_channel,
            content_resolver,
            _audit_log: audit_log,
            provider_registry,
            version,
            shell_id: RwLock::new(ShellId::new()),
        }
    }

    /// Set the shell capsule ID
    pub async fn set_shell(&self, id: CapsuleId) {
        // Set on message channel so it knows who is exempt from token checks
        self.message_channel
            .set_shell_id(id.as_str().to_string())
            .await;
        let mut shell_id = self.shell_id.write().await;
        shell_id.set(id);
    }

    /// Check if a capsule is the shell
    async fn is_shell(&self, id: &CapsuleId) -> bool {
        let shell_id = self.shell_id.read().await;
        shell_id.is_shell(id)
    }

    /// Maximum length for capsule IDs
    const MAX_CAPSULE_ID_LEN: usize = 256;
    /// Maximum length for resource URIs
    const MAX_RESOURCE_LEN: usize = 4096;
    /// Maximum length for CIDs
    const MAX_CID_LEN: usize = 128;

    /// Validate that a string contains no control characters (except whitespace)
    fn has_control_chars(s: &str) -> bool {
        s.chars().any(|c| c.is_control() && !c.is_whitespace())
    }

    fn is_ipfs_cid(identifier: &str) -> bool {
        (identifier.starts_with("Qm") && identifier.len() == 46)
            || (identifier.starts_with("baf") && identifier.len() >= 50)
    }

    /// Normalize a launch request to the current explicit contract:
    /// a bare IPFS CID with no sub-path.
    fn normalize_launch_cid(cid: &str) -> Result<String, RuntimeResponse> {
        let launch_uri = if cid.starts_with("elastos://") {
            cid.to_string()
        } else {
            format!("elastos://{}", cid)
        };

        let parsed = ContentUri::parse(&launch_uri).map_err(|err| {
            RuntimeResponse::error(
                "invalid_input",
                format!("LaunchCapsule requires a bare IPFS CID: {}", err),
            )
        })?;

        if !Self::is_ipfs_cid(&parsed.identifier) {
            return Err(RuntimeResponse::error(
                "invalid_input",
                "LaunchCapsule currently accepts only bare IPFS CIDs",
            ));
        }

        if parsed.path.is_some() {
            return Err(RuntimeResponse::error(
                "invalid_input",
                "LaunchCapsule does not accept elastos:// sub-paths; pass a bare capsule CID",
            ));
        }

        Ok(parsed.identifier)
    }

    /// Handle a request from a capsule
    pub async fn handle(&self, from: &CapsuleId, request: RuntimeRequest) -> RuntimeResponse {
        match request {
            RuntimeRequest::ListCapsules => self.handle_list_capsules(from).await,
            RuntimeRequest::LaunchCapsule { cid, config } => {
                self.handle_launch_capsule(from, &cid, config).await
            }
            RuntimeRequest::StopCapsule { capsule_id } => {
                self.handle_stop_capsule(from, &capsule_id).await
            }
            RuntimeRequest::GrantCapability {
                capsule_id,
                resource,
                action,
                constraints,
            } => {
                self.handle_grant_capability(from, &capsule_id, &resource, &action, constraints)
                    .await
            }
            RuntimeRequest::RevokeCapability { token_id } => {
                self.handle_revoke_capability(from, &token_id).await
            }
            RuntimeRequest::SendMessage {
                to,
                payload,
                reply_to,
                token,
            } => {
                self.handle_send_message(from, &to, payload, reply_to, token)
                    .await
            }
            RuntimeRequest::ReceiveMessages => self.handle_receive_messages(from).await,
            RuntimeRequest::FetchContent { uri, token } => {
                self.handle_fetch_content(from, &uri, token.as_deref())
                    .await
            }
            RuntimeRequest::StorageRead { token, path } => {
                self.handle_storage_read(from, &token, &path).await
            }
            RuntimeRequest::StorageWrite {
                token,
                path,
                content,
            } => {
                self.handle_storage_write(from, &token, &path, content)
                    .await
            }
            RuntimeRequest::GetRuntimeInfo => self.handle_get_runtime_info(from).await,
            RuntimeRequest::Ping => RuntimeResponse::Pong,
            RuntimeRequest::WindowControl { .. } => RuntimeResponse::error(
                "not_implemented",
                "Window control requires shell routing (not yet implemented)",
            ),
            RuntimeRequest::ResourceRequest {
                uri,
                action,
                params,
                token,
            } => {
                self.handle_resource_request(from, &uri, &action, params, token)
                    .await
            }
        }
    }

    /// Handle ListCapsules request
    async fn handle_list_capsules(&self, from: &CapsuleId) -> RuntimeResponse {
        // Only shell can list all capsules
        if !self.is_shell(from).await {
            return RuntimeResponse::error("unauthorized", "Only shell can list capsules");
        }

        let capsule_ids = self.capsule_manager.list().await;
        let mut capsules = Vec::new();

        for id in capsule_ids {
            if let Some(info) = self.capsule_manager.get(&id).await {
                capsules.push(CapsuleListEntry {
                    id: id.to_string(),
                    name: info.manifest.name.clone(),
                    status: format!("{:?}", info.state).to_lowercase(),
                });
            }
        }

        RuntimeResponse::CapsuleList { capsules }
    }

    /// Handle LaunchCapsule request
    async fn handle_launch_capsule(
        &self,
        from: &CapsuleId,
        cid: &str,
        _config: LaunchConfig,
    ) -> RuntimeResponse {
        // Only shell can launch capsules
        if !self.is_shell(from).await {
            return RuntimeResponse::error("unauthorized", "Only shell can launch capsules");
        }

        // Input validation
        if cid.len() > Self::MAX_CID_LEN {
            return RuntimeResponse::error("invalid_input", "CID exceeds maximum length");
        }
        if Self::has_control_chars(cid) {
            return RuntimeResponse::error("invalid_input", "CID contains control characters");
        }

        let normalized_cid = match Self::normalize_launch_cid(cid) {
            Ok(cid) => cid,
            Err(response) => return response,
        };
        let uri = format!("elastos://{}", normalized_cid);

        let fetch_result = match self.content_resolver.fetch(&uri).await {
            Ok(result) => result,
            Err(e) => {
                return RuntimeResponse::error("fetch_failed", format!("Failed to fetch: {}", e));
            }
        };

        let prepared = match prepare_fetched_capsule(&normalized_cid, fetch_result) {
            Ok(prepared) => prepared,
            Err(err) => return RuntimeResponse::error("internal_error", err),
        };

        // Launch the capsule
        match self
            .capsule_manager
            .launch_from_cid(
                prepared.path(),
                prepared.manifest().clone(),
                normalized_cid,
                TrustLevel::Untrusted,
            )
            .await
        {
            Ok(capsule_id) => RuntimeResponse::CapsuleLaunched {
                capsule_id: capsule_id.to_string(),
            },
            Err(e) => RuntimeResponse::error("launch_failed", format!("Failed to launch: {}", e)),
        }
    }

    /// Handle StopCapsule request
    async fn handle_stop_capsule(&self, from: &CapsuleId, capsule_id: &str) -> RuntimeResponse {
        // Only shell can stop capsules
        if !self.is_shell(from).await {
            return RuntimeResponse::error("unauthorized", "Only shell can stop capsules");
        }

        let target_id = CapsuleId::from_string(capsule_id);

        match self
            .capsule_manager
            .stop(&target_id, StopReason::Requested)
            .await
        {
            Ok(()) => RuntimeResponse::ok(),
            Err(e) => RuntimeResponse::error("stop_failed", format!("Failed to stop: {}", e)),
        }
    }

    /// Handle GrantCapability request
    async fn handle_grant_capability(
        &self,
        from: &CapsuleId,
        capsule_id: &str,
        resource: &str,
        action: &str,
        constraints: CapabilityConstraints,
    ) -> RuntimeResponse {
        // Only shell can grant capabilities
        if !self.is_shell(from).await {
            return RuntimeResponse::error("unauthorized", "Only shell can grant capabilities");
        }

        // Input length validation
        if capsule_id.len() > Self::MAX_CAPSULE_ID_LEN {
            return RuntimeResponse::error("invalid_input", "capsule_id exceeds maximum length");
        }
        if resource.len() > Self::MAX_RESOURCE_LEN {
            return RuntimeResponse::error("invalid_input", "resource exceeds maximum length");
        }
        if Self::has_control_chars(capsule_id) || Self::has_control_chars(resource) {
            return RuntimeResponse::error("invalid_input", "input contains control characters");
        }

        // Parse action
        let action = match action.to_lowercase().as_str() {
            "read" => Action::Read,
            "write" => Action::Write,
            "execute" => Action::Execute,
            "message" => Action::Message,
            _ => {
                return RuntimeResponse::error(
                    "invalid_action",
                    format!("Unknown action: {}", action),
                );
            }
        };

        // Convert constraints
        let internal_constraints = InternalConstraints {
            epoch: self.capability_manager.current_epoch(),
            delegatable: constraints.delegatable,
            max_classification: None,
            max_uses: constraints.max_uses,
        };

        // Calculate expiry
        let expiry = constraints.expiry_secs.map(|secs| {
            let now = SecureTimestamp::now();
            SecureTimestamp::at(now.unix_secs + secs)
        });

        // Grant the capability
        let token = self.capability_manager.grant(
            capsule_id,
            ResourceId::new(resource),
            action,
            internal_constraints,
            expiry,
        );

        RuntimeResponse::CapabilityGranted {
            token_id: token.id.to_string(),
        }
    }

    /// Handle RevokeCapability request
    async fn handle_revoke_capability(&self, from: &CapsuleId, token_id: &str) -> RuntimeResponse {
        use crate::capability::token::TokenId;

        // Only shell can revoke capabilities
        if !self.is_shell(from).await {
            return RuntimeResponse::error("unauthorized", "Only shell can revoke capabilities");
        }

        // Parse hex token ID to TokenId
        let token_bytes = match hex::decode(token_id) {
            Ok(bytes) if bytes.len() == 16 => {
                let mut arr = [0u8; 16];
                arr.copy_from_slice(&bytes);
                TokenId::from_bytes(arr)
            }
            _ => {
                return RuntimeResponse::error(
                    "invalid_token_id",
                    "Token ID must be 32 hex characters",
                );
            }
        };

        self.capability_manager
            .revoke(token_bytes, "Revoked by shell")
            .await;

        RuntimeResponse::ok()
    }

    /// Handle SendMessage request
    async fn handle_send_message(
        &self,
        from: &CapsuleId,
        to: &str,
        payload: Vec<u8>,
        _reply_to: Option<String>,
        token: Option<String>,
    ) -> RuntimeResponse {
        use crate::capability::token::CapabilityToken;

        // Decode the token (if provided) for passing to the message channel.
        // The channel does authoritative validation (H1d: shell check + capability check).
        let cap_token = if !self.is_shell(from).await {
            let token_str = match &token {
                Some(t) if !t.is_empty() => t.as_str(),
                _ => {
                    return RuntimeResponse::error(
                        "missing_token",
                        "Capability token required for messaging",
                    );
                }
            };

            match CapabilityToken::from_base64(token_str) {
                Ok(t) => Some(t),
                Err(_) => {
                    return RuntimeResponse::error(
                        "invalid_token",
                        "Failed to decode capability token",
                    );
                }
            }
        } else {
            None
        };

        let message = Message::new(from.as_str().to_string(), to.to_string(), payload);

        match self.message_channel.send(message, cap_token.as_ref()).await {
            Ok(_) => RuntimeResponse::ok(),
            Err(e) => RuntimeResponse::error("send_failed", format!("Failed to send: {}", e)),
        }
    }

    /// Handle ReceiveMessages request
    async fn handle_receive_messages(&self, from: &CapsuleId) -> RuntimeResponse {
        // Any capsule can receive its own messages
        let msgs = self.message_channel.receive(from.as_str()).await;

        let messages: Vec<IncomingMessage> = msgs
            .into_iter()
            .map(|m| IncomingMessage {
                id: m.id.to_string(),
                from: m.from.clone(),
                payload: m.payload,
                timestamp: m.timestamp.unix_secs,
                reply_to: m.reply_to.map(|id| id.to_string()),
            })
            .collect();

        RuntimeResponse::Messages { messages }
    }

    /// Handle FetchContent request
    async fn handle_fetch_content(
        &self,
        from: &CapsuleId,
        uri: &str,
        token: Option<&str>,
    ) -> RuntimeResponse {
        if uri.len() > Self::MAX_RESOURCE_LEN {
            return RuntimeResponse::error("invalid_input", "URI exceeds maximum length");
        }
        if Self::has_control_chars(uri) {
            return RuntimeResponse::error("invalid_input", "URI contains control characters");
        }
        if uri.contains("://") && !is_supported_resource_scheme(uri) {
            return RuntimeResponse::error(
                "invalid_input",
                "resource URI must use localhost:// or elastos://",
            );
        }

        if !self.is_shell(from).await {
            let token_str = match token {
                Some(t) if !t.is_empty() => t,
                _ => {
                    return RuntimeResponse::error(
                        "missing_token",
                        "Capability token required for content fetch",
                    )
                }
            };

            if let Err(e) = self
                .validate_token(token_str, from, Action::Read, uri)
                .await
            {
                return e;
            }
        }

        match self.content_resolver.fetch(uri).await {
            Ok(result) => RuntimeResponse::Content {
                data: result.content,
            },
            Err(e) => RuntimeResponse::error("fetch_failed", format!("Failed to fetch: {}", e)),
        }
    }

    /// Handle StorageRead request via provider registry
    async fn handle_storage_read(
        &self,
        from: &CapsuleId,
        token: &str,
        path: &str,
    ) -> RuntimeResponse {
        // Input validation
        if path.len() > Self::MAX_RESOURCE_LEN {
            return RuntimeResponse::error("invalid_input", "path exceeds maximum length");
        }
        if Self::has_control_chars(path) {
            return RuntimeResponse::error("invalid_input", "path contains control characters");
        }

        // Shell capsules are exempt from capability checks
        if !self.is_shell(from).await {
            if let Err(e) = self.validate_token(token, from, Action::Read, path).await {
                return e;
            }
        }
        let uri = match rooted_localhost_uri(path) {
            Some(uri) => uri,
            None => {
                return RuntimeResponse::error(
                    "invalid_input",
                    "storage path must be rooted under localhost://<root>/...",
                )
            }
        };
        self.route_to_provider(from, &uri, ResourceAction::Read, None)
            .await
    }

    /// Handle StorageWrite request via provider registry
    async fn handle_storage_write(
        &self,
        from: &CapsuleId,
        token: &str,
        path: &str,
        content: Vec<u8>,
    ) -> RuntimeResponse {
        // Input validation
        if path.len() > Self::MAX_RESOURCE_LEN {
            return RuntimeResponse::error("invalid_input", "path exceeds maximum length");
        }
        if Self::has_control_chars(path) {
            return RuntimeResponse::error("invalid_input", "path contains control characters");
        }

        // Shell capsules are exempt from capability checks
        if !self.is_shell(from).await {
            if let Err(e) = self.validate_token(token, from, Action::Write, path).await {
                return e;
            }
        }
        let uri = match rooted_localhost_uri(path) {
            Some(uri) => uri,
            None => {
                return RuntimeResponse::error(
                    "invalid_input",
                    "storage path must be rooted under localhost://<root>/...",
                )
            }
        };
        self.route_to_provider(from, &uri, ResourceAction::Write, Some(content))
            .await
    }

    /// Handle ResourceRequest (URI-based provider routing)
    async fn handle_resource_request(
        &self,
        from: &CapsuleId,
        uri: &str,
        action: &str,
        params: Option<serde_json::Value>,
        token: Option<String>,
    ) -> RuntimeResponse {
        // Input length validation
        if uri.len() > Self::MAX_RESOURCE_LEN {
            return RuntimeResponse::error("invalid_input", "URI exceeds maximum length");
        }
        if Self::has_control_chars(uri) {
            return RuntimeResponse::error("invalid_input", "URI contains control characters");
        }
        if uri.contains("://") && !is_supported_resource_scheme(uri) {
            return RuntimeResponse::error(
                "invalid_input",
                "resource URI must use localhost:// or elastos://",
            );
        }

        // Read-only Capsule Inspector surface. Served directly here (not via the
        // provider registry) because it projects runtime-owned state — capsule
        // manifests, capability grants, audit — under a scoped, fail-closed
        // authorization gate. See `crate::inspect` and docs/CAPSULE_INSPECTOR.md.
        if uri == "elastos://inspect" || uri.starts_with("elastos://inspect/") {
            return self.handle_inspect(from, uri, params, token).await;
        }

        let resource_action = match action.to_lowercase().as_str() {
            "read" => ResourceAction::Read,
            "write" => ResourceAction::Write,
            "list" => ResourceAction::List,
            "delete" => ResourceAction::Delete,
            "stat" => ResourceAction::Stat,
            "mkdir" => ResourceAction::Mkdir,
            "exists" => ResourceAction::Exists,
            other => {
                return RuntimeResponse::error(
                    "invalid_action",
                    format!("Unknown resource action: {}", other),
                );
            }
        };

        // Non-shell capsules must present a valid capability token
        if !self.is_shell(from).await {
            let cap_action = match resource_action {
                ResourceAction::Read
                | ResourceAction::List
                | ResourceAction::Stat
                | ResourceAction::Exists => Action::Read,
                ResourceAction::Write | ResourceAction::Mkdir => Action::Write,
                ResourceAction::Delete => Action::Delete,
            };

            let token_str = match &token {
                Some(t) if !t.is_empty() => t.as_str(),
                _ => {
                    return RuntimeResponse::error(
                        "missing_token",
                        "Capability token required for resource access",
                    );
                }
            };

            if let Err(e) = self.validate_token(token_str, from, cap_action, uri).await {
                return e;
            }
        }

        // Extract content from params for write operations
        let content = params
            .as_ref()
            .and_then(|p| p.get("content"))
            .and_then(|c| {
                // Accept base64 string or byte array
                if let Some(s) = c.as_str() {
                    use base64::Engine;
                    base64::engine::general_purpose::STANDARD.decode(s).ok()
                } else {
                    c.as_array().map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_u64().map(|n| n as u8))
                            .collect()
                    })
                }
            });

        self.route_to_provider(from, uri, resource_action, content)
            .await
    }

    /// Validate a capability token for a resource operation.
    /// Returns Ok(()) on success, or Err(RuntimeResponse) with an error response.
    async fn validate_token(
        &self,
        token_b64: &str,
        from: &CapsuleId,
        action: Action,
        resource_uri: &str,
    ) -> Result<(), RuntimeResponse> {
        use crate::capability::token::CapabilityToken;

        if token_b64.is_empty() {
            return Err(RuntimeResponse::error(
                "missing_token",
                "Capability token required for resource access",
            ));
        }

        let token = CapabilityToken::from_base64(token_b64).map_err(|_| {
            RuntimeResponse::error("invalid_token", "Failed to decode capability token")
        })?;

        let resource = if resource_uri.starts_with("localhost://") {
            let uri = rooted_localhost_uri(resource_uri).ok_or_else(|| {
                RuntimeResponse::error(
                    "invalid_input",
                    "localhost resource must be rooted under localhost://<root>/...",
                )
            })?;
            ResourceId::new(uri)
        } else if resource_uri.contains("://") {
            if !is_supported_resource_scheme(resource_uri) {
                return Err(RuntimeResponse::error(
                    "invalid_input",
                    "resource URI must use localhost:// or elastos://",
                ));
            }
            ResourceId::new(resource_uri)
        } else {
            let uri = rooted_localhost_uri(resource_uri).ok_or_else(|| {
                RuntimeResponse::error(
                    "invalid_input",
                    "storage path must be rooted under localhost://<root>/...",
                )
            })?;
            ResourceId::new(uri)
        };

        self.capability_manager
            .validate(&token, from.as_str(), action, &resource, None)
            .await
            .map_err(|e| RuntimeResponse::error("permission_denied", e.to_string()))
    }

    /// Route a request through the provider registry
    async fn route_to_provider(
        &self,
        from: &CapsuleId,
        uri: &str,
        action: ResourceAction,
        content: Option<Vec<u8>>,
    ) -> RuntimeResponse {
        let registry = match &self.provider_registry {
            Some(r) => r,
            None => {
                return RuntimeResponse::error("no_provider", "No provider registry configured");
            }
        };

        match registry.route(uri, from.as_str(), action, content).await {
            Ok(response) => match response {
                crate::provider::ResourceResponse::Data(data) => RuntimeResponse::Content { data },
                crate::provider::ResourceResponse::List(entries) => {
                    let items: Vec<serde_json::Value> = entries
                        .iter()
                        .map(|e| {
                            serde_json::json!({
                                "name": e.name,
                                "is_directory": e.is_directory,
                                "size": e.size,
                                "modified": e.modified,
                            })
                        })
                        .collect();
                    RuntimeResponse::ResourceResponse {
                        data: None,
                        entries: Some(items),
                        exists: None,
                        stat: None,
                    }
                }
                crate::provider::ResourceResponse::Ok
                | crate::provider::ResourceResponse::Written { .. }
                | crate::provider::ResourceResponse::Deleted
                | crate::provider::ResourceResponse::Created => RuntimeResponse::ok(),
                crate::provider::ResourceResponse::Metadata {
                    size,
                    entry_type,
                    modified,
                } => RuntimeResponse::ResourceResponse {
                    data: None,
                    entries: None,
                    exists: None,
                    stat: Some(serde_json::json!({
                        "size": size,
                        "is_directory": matches!(entry_type, crate::provider::EntryType::Directory),
                        "modified": modified,
                    })),
                },
                crate::provider::ResourceResponse::Exists(exists) => {
                    RuntimeResponse::ResourceResponse {
                        data: None,
                        entries: None,
                        exists: Some(exists),
                        stat: None,
                    }
                }
            },
            Err(e) => {
                let code = match &e {
                    crate::provider::ProviderError::NotFound(_) => "not_found",
                    crate::provider::ProviderError::PermissionDenied(_) => "permission_denied",
                    crate::provider::ProviderError::NoProvider(_) => "no_provider",
                    _ => "provider_error",
                };
                RuntimeResponse::error(code, e.to_string())
            }
        }
    }

    /// Handle GetRuntimeInfo request
    async fn handle_get_runtime_info(&self, _from: &CapsuleId) -> RuntimeResponse {
        // Any capsule can get runtime info
        let running = self.capsule_manager.list_running().await;

        RuntimeResponse::RuntimeInfo {
            version: self.version.clone(),
            capsule_count: running.len(),
        }
    }

    // ===== Capsule Inspector (read-only) =====

    /// Dispatch an `elastos://inspect/*` request under a scoped, fail-closed
    /// authorization gate.
    ///
    /// Read endpoints (`capsules`, `capsule`, `self`) require a `Read` inspect
    /// capability; the write endpoint (`revoke`) requires a `Write` inspect
    /// capability. The action dimension keeps the two strictly separated: a
    /// read-only inspect grant can never drive a mutation (Principles #3, #16).
    async fn handle_inspect(
        &self,
        from: &CapsuleId,
        uri: &str,
        params: Option<serde_json::Value>,
        token: Option<String>,
    ) -> RuntimeResponse {
        use crate::capability::token::CapabilityToken;
        use crate::inspect::{self, InspectScope};

        let endpoint = uri
            .strip_prefix("elastos://inspect")
            .unwrap_or("")
            .trim_start_matches('/');

        // `revoke` mutates authority and demands a Write inspect capability;
        // everything else is read-only.
        let required_action = if endpoint == "revoke" {
            Action::Write
        } else {
            Action::Read
        };

        // Determine the caller's inspect scope. Shell is System by existing
        // orchestrator privilege; every other caller must present a valid
        // inspect capability token for the required action, and the grant
        // pattern fixes the tier.
        let is_shell = self.is_shell(from).await;
        let scope = if is_shell {
            InspectScope::System
        } else {
            let token_str = match token.as_deref() {
                Some(t) if !t.is_empty() => t,
                _ => {
                    return RuntimeResponse::error(
                        "missing_token",
                        "Capability token required for inspect access",
                    )
                }
            };
            // Authoritative capability check: the token must grant the
            // requested URI *and* action. A read grant fails a write endpoint;
            // a self-only grant cannot satisfy a system URI.
            if let Err(e) = self.validate_token(token_str, from, required_action, uri).await {
                return e;
            }
            // Defense in depth: classify the granted pattern into a scope.
            let granted = match CapabilityToken::from_base64(token_str) {
                Ok(t) => vec![t.resource().as_str().to_string()],
                Err(_) => {
                    return RuntimeResponse::error(
                        "invalid_token",
                        "Failed to decode capability token",
                    )
                }
            };
            match inspect::InspectScope::from_grants(false, granted.iter()) {
                Some(s) => s,
                None => {
                    return RuntimeResponse::error(
                        "permission_denied",
                        "Capability does not grant an inspect scope",
                    )
                }
            }
        };

        match endpoint {
            "capsules" => self.inspect_list(scope, from).await,
            "self" => self.inspect_detail(scope, from, from.as_str()).await,
            "capsule" => {
                match params
                    .as_ref()
                    .and_then(|p| p.get("id"))
                    .and_then(|v| v.as_str())
                {
                    Some(id) => self.inspect_detail(scope, from, id).await,
                    None => RuntimeResponse::error(
                        "invalid_input",
                        "inspect/capsule requires an \"id\" parameter",
                    ),
                }
            }
            "revoke" => self.inspect_revoke(scope, from, params).await,
            _ => RuntimeResponse::error("not_found", "Unknown inspect endpoint"),
        }
    }

    /// Revoke a capability by token id. Write-gated and System-scoped: only a
    /// holder of a `Write` inspect capability at System scope (or the shell)
    /// reaches here. Revocation only ever *reduces* authority and is audited.
    async fn inspect_revoke(
        &self,
        scope: crate::inspect::InspectScope,
        from: &CapsuleId,
        params: Option<serde_json::Value>,
    ) -> RuntimeResponse {
        use crate::capability::token::TokenId;
        use crate::inspect::InspectScope;
        use crate::primitives::audit::AuditEvent;

        // A self-only inspect grant must never drive a system mutation.
        if scope != InspectScope::System {
            return RuntimeResponse::error(
                "permission_denied",
                "Revoke requires system-scope inspect authority",
            );
        }

        let token_id = match params
            .as_ref()
            .and_then(|p| p.get("token_id"))
            .and_then(|v| v.as_str())
        {
            Some(id) => id,
            None => {
                return RuntimeResponse::error(
                    "invalid_input",
                    "inspect/revoke requires a \"token_id\" parameter",
                )
            }
        };

        // Parse the 32-hex-char token id (same contract as RevokeCapability).
        let parsed = match hex::decode(token_id) {
            Ok(bytes) if bytes.len() == 16 => {
                let mut arr = [0u8; 16];
                arr.copy_from_slice(&bytes);
                TokenId::from_bytes(arr)
            }
            _ => {
                return RuntimeResponse::error(
                    "invalid_token_id",
                    "Token ID must be 32 hex characters",
                )
            }
        };

        self.capability_manager
            .revoke(parsed, "Revoked via inspector")
            .await;

        // Audit who drove the revoke (the revoke itself is also audited by the
        // capability manager).
        self._audit_log.emit(AuditEvent::Custom {
            event_type: "inspect.revoke".to_string(),
            details: serde_json::json!({ "caller": from.as_str(), "token_id": token_id }),
        });

        RuntimeResponse::ok()
    }

    /// List capsules visible under the caller's scope.
    async fn inspect_list(
        &self,
        scope: crate::inspect::InspectScope,
        from: &CapsuleId,
    ) -> RuntimeResponse {
        use crate::inspect::InspectScope;

        let mut capsules = Vec::new();
        for id in self.capsule_manager.list().await {
            if !scope.can_view(from.as_str(), id.as_str()) {
                continue;
            }
            if let Some(info) = self.capsule_manager.get(&id).await {
                capsules.push(serde_json::json!({
                    "id": id.to_string(),
                    "name": info.manifest.name,
                    "role": serde_json::to_value(&info.manifest.role).ok(),
                    "type": serde_json::to_value(&info.manifest.capsule_type).ok(),
                    "state": format!("{:?}", info.state).to_lowercase(),
                }));
            }
        }

        let scope_label = match scope {
            InspectScope::System => "system",
            InspectScope::SelfOnly => "self",
        };
        RuntimeResponse::ok_with_data(serde_json::json!({
            "scope": scope_label,
            "capsules": capsules,
        }))
    }

    /// Return the full inspector view of a single capsule, gated by scope.
    /// Out-of-scope requests are denied and audited.
    async fn inspect_detail(
        &self,
        scope: crate::inspect::InspectScope,
        from: &CapsuleId,
        target: &str,
    ) -> RuntimeResponse {
        use crate::primitives::audit::AuditEvent;

        if !scope.can_view(from.as_str(), target) {
            self._audit_log.emit(AuditEvent::Custom {
                event_type: "inspect.out_of_scope".to_string(),
                details: serde_json::json!({ "caller": from.as_str(), "target": target }),
            });
            return RuntimeResponse::error("out_of_scope", "Caller may not inspect this capsule");
        }

        let mut info = None;
        for id in self.capsule_manager.list().await {
            if id.as_str() == target {
                info = self.capsule_manager.get(&id).await;
                break;
            }
        }
        match info {
            Some(info) => RuntimeResponse::ok_with_data(self.build_capsule_view(&info)),
            None => RuntimeResponse::error("not_found", "No such capsule"),
        }
    }

    /// Project a capsule's runtime-owned state into the inspector contract
    /// (see docs/CAPSULE_INSPECTOR.md). Read-only; surfaces only what the
    /// runtime actually knows and leaves unknown fields null.
    fn build_capsule_view(&self, info: &CapsuleInfo) -> serde_json::Value {
        use serde_json::{json, Value};

        fn field(v: &Value, key: &str) -> Value {
            v.get(key).cloned().unwrap_or(Value::Null)
        }

        let id = info.id.to_string();
        let cid = info.cid.clone();
        let manifest = serde_json::to_value(&info.manifest).unwrap_or_else(|_| json!({}));

        // Affordances: flatten declared interface methods.
        let mut affordances = Vec::new();
        if let Some(interfaces) = manifest.get("interfaces").and_then(|v| v.as_array()) {
            for iface in interfaces {
                let iface_id = field(iface, "id");
                if let Some(methods) = iface.get("methods").and_then(|v| v.as_array()) {
                    for m in methods {
                        affordances.push(json!({
                            "interface": iface_id,
                            "id": field(m, "id"),
                            "risk": field(m, "risk"),
                            "approval": field(m, "approval"),
                            "audit": field(m, "audit"),
                            "description": field(m, "description"),
                        }));
                    }
                }
            }
        }

        // Capability grants + recent activity, derived from the audit log —
        // the runtime's authoritative record of what this capsule did.
        let events = self._audit_log.recent_events(500);
        let mut recent = Vec::new();
        let mut grants: std::collections::BTreeMap<String, bool> = std::collections::BTreeMap::new();
        let (mut total, mut denied) = (0u64, 0u64);
        for ev in &events {
            let v = match serde_json::to_value(ev) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if v.get("capsule_id").and_then(|c| c.as_str()) != Some(id.as_str()) {
                continue;
            }
            total += 1;
            let etype = v.get("type").and_then(|t| t.as_str()).unwrap_or("").to_string();
            let resource = v.get("resource").and_then(|r| r.as_str()).map(str::to_string);
            let action = v.get("action").and_then(|a| a.as_str()).unwrap_or("").to_string();
            let success = v.get("success").and_then(|s| s.as_bool()).unwrap_or(true);
            if !success {
                denied += 1;
            }
            if let Some(res) = &resource {
                let key = format!("{} {}", res, action);
                let entry = grants.entry(key).or_insert(true);
                if etype == "capability_use" && !success {
                    *entry = false;
                }
            }
            if recent.len() < 20 {
                recent.push(json!({
                    "ts": v.get("timestamp").and_then(|t| t.get("unix_secs")).cloned(),
                    "event": etype,
                    "detail": resource
                        .map(|r| format!("{} {}", r, action))
                        .unwrap_or_default(),
                    "success": success,
                }));
            }
        }
        let granted_capabilities: Vec<Value> = grants
            .into_iter()
            .map(|(key, ok)| {
                let mut parts = key.splitn(2, ' ');
                let resource = parts.next().unwrap_or("");
                let action = parts.next().unwrap_or("");
                json!({ "resource": resource, "action": action, "granted": ok })
            })
            .collect();

        let signature_present = manifest
            .get("signature")
            .map(|s| s.is_string())
            .unwrap_or(false);

        // Provider authority — declarative powers a provider capsule is
        // authorized for (parity with the product-side inspect provider).
        let authority = manifest
            .get("authority")
            .map(|a| {
                json!({
                    "reason": a.get("reason").cloned().unwrap_or(Value::Null),
                    "capabilities": a.get("capabilities").cloned().unwrap_or(Value::Null),
                    "audit_events": a.get("audit_events").cloned().unwrap_or(Value::Null),
                })
            })
            .unwrap_or(Value::Null);

        json!({
            "id": id,
            "name": field(&manifest, "name"),
            "version": field(&manifest, "version"),
            "role": field(&manifest, "role"),
            "type": field(&manifest, "type"),
            "description": field(&manifest, "description"),
            "author": field(&manifest, "author"),
            "identity": {
                "did": Value::Null,
                "cid": cid,
                "trust_level": serde_json::to_value(&info.trust_level).ok(),
                "signature_present": signature_present,
                "signed_by": Value::Null,
            },
            "manifest": {
                "schema": field(&manifest, "schema"),
                "entrypoint": field(&manifest, "entrypoint"),
            },
            "affordances": affordances,
            "authority": authority,
            "required_capabilities": field(&manifest, "capabilities"),
            "granted_capabilities": granted_capabilities,
            "storage_namespaces": manifest.pointer("/permissions/storage").cloned().unwrap_or(Value::Null),
            "carrier": {
                "enabled": manifest.pointer("/permissions/carrier").cloned().unwrap_or(Value::Null),
                "endpoints": [],
                "peers": 0,
            },
            "provenance": {
                "signed_by": Value::Null,
                "version": field(&manifest, "version"),
                "installed_at": Value::Null,
                "cid": info.cid.clone(),
                "signature_present": signature_present,
            },
            "audit": {
                "counts": { "total": total, "denied": denied },
                "recent": recent,
            },
            "processes": [],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::CapabilityStore;
    use crate::content::{NullFetcher, ResolverConfig};
    use crate::primitives::metrics::MetricsManager;
    use elastos_common::{CapsuleManifest, CapsuleStatus, CapsuleType};
    use elastos_compute::{CapsuleHandle, CapsuleInfo as ComputeCapsuleInfo, ComputeProvider};
    use std::path::Path;

    // Mock compute provider
    struct MockComputeProvider;

    #[async_trait::async_trait]
    impl ComputeProvider for MockComputeProvider {
        fn supports(&self, _capsule_type: &CapsuleType) -> bool {
            true
        }

        async fn load(
            &self,
            _path: &Path,
            manifest: CapsuleManifest,
        ) -> elastos_common::Result<CapsuleHandle> {
            Ok(CapsuleHandle {
                id: elastos_common::CapsuleId::new(format!("handle-{}", uuid::Uuid::new_v4())),
                manifest,
                args: vec![],
            })
        }

        async fn start(&self, _handle: &CapsuleHandle) -> elastos_common::Result<()> {
            Ok(())
        }

        async fn stop(&self, _handle: &CapsuleHandle) -> elastos_common::Result<()> {
            Ok(())
        }

        async fn status(&self, _handle: &CapsuleHandle) -> elastos_common::Result<CapsuleStatus> {
            Ok(CapsuleStatus::Running)
        }

        async fn info(&self, handle: &CapsuleHandle) -> elastos_common::Result<ComputeCapsuleInfo> {
            Ok(ComputeCapsuleInfo {
                id: handle.id.clone(),
                name: handle.manifest.name.clone(),
                status: CapsuleStatus::Running,
                memory_used_mb: 0,
            })
        }
    }

    async fn create_test_handler() -> (RequestHandler, CapsuleId) {
        let compute = Arc::new(MockComputeProvider);
        let store = Arc::new(CapabilityStore::new());
        let audit_log = Arc::new(AuditLog::new());
        let metrics = Arc::new(MetricsManager::new());

        let capability_manager = Arc::new(CapabilityManager::new(
            store,
            audit_log.clone(),
            metrics.clone(),
        ));

        let capsule_manager = Arc::new(CapsuleManager::new(
            compute,
            capability_manager.clone(),
            metrics.clone(),
            audit_log.clone(),
        ));

        let message_channel = Arc::new(MessageChannel::new(
            capability_manager.clone(),
            metrics.clone(),
            audit_log.clone(),
        ));

        let content_resolver = Arc::new(ContentResolver::new(
            ResolverConfig::default(),
            audit_log.clone(),
            Arc::new(NullFetcher),
        ));

        let handler = RequestHandler::new(
            capsule_manager,
            capability_manager,
            message_channel,
            content_resolver,
            audit_log,
            "0.1.0".to_string(),
            None,
        );

        // Create and set shell ID
        let shell_id = CapsuleId::new();
        handler.set_shell(shell_id.clone()).await;

        (handler, shell_id)
    }

    /// Like `create_test_handler`, but also returns the capability and capsule
    /// managers so inspect conformance tests can mint scoped tokens and launch
    /// real capsules to introspect.
    async fn create_test_handler_with_caps(
    ) -> (RequestHandler, CapsuleId, Arc<CapabilityManager>, Arc<CapsuleManager>) {
        let compute = Arc::new(MockComputeProvider);
        let store = Arc::new(CapabilityStore::new());
        let audit_log = Arc::new(AuditLog::new());
        let metrics = Arc::new(MetricsManager::new());
        let capability_manager =
            Arc::new(CapabilityManager::new(store, audit_log.clone(), metrics.clone()));
        let capsule_manager = Arc::new(CapsuleManager::new(
            compute,
            capability_manager.clone(),
            metrics.clone(),
            audit_log.clone(),
        ));
        let message_channel = Arc::new(MessageChannel::new(
            capability_manager.clone(),
            metrics.clone(),
            audit_log.clone(),
        ));
        let content_resolver = Arc::new(ContentResolver::new(
            ResolverConfig::default(),
            audit_log.clone(),
            Arc::new(NullFetcher),
        ));
        let handler = RequestHandler::new(
            capsule_manager.clone(),
            capability_manager.clone(),
            message_channel,
            content_resolver,
            audit_log,
            "0.1.0".to_string(),
            None,
        );
        let shell_id = CapsuleId::new();
        handler.set_shell(shell_id.clone()).await;
        (handler, shell_id, capability_manager, capsule_manager)
    }

    /// A manifest with affordances, a required capability, a storage namespace,
    /// and a (sensitive) signature — used to prove the inspector renders the
    /// contract faithfully and never echoes the raw signature.
    fn probe_manifest() -> elastos_common::CapsuleManifest {
        serde_json::from_value(serde_json::json!({
            "schema": "elastos.capsule/v1",
            "version": "0.1.0",
            "name": "probe",
            "role": "app",
            "type": "wasm",
            "entrypoint": "probe.wasm",
            "capabilities": ["elastos://storage/probe"],
            "interfaces": [{
                "id": "elastos.probe/v1",
                "version": "1",
                "methods": [{
                    "id": "ping", "risk": "read", "approval": "none", "audit": "summary"
                }]
            }],
            "permissions": { "storage": ["localhost://WebSpaces/probe/"] },
            "signature": "SECRET_SIGNATURE_MUST_NOT_LEAK"
        }))
        .expect("probe manifest deserializes")
    }

    #[tokio::test]
    async fn inspect_detail_renders_contract_without_leaking_authority() {
        let (handler, shell_id, _caps, capsule_manager) = create_test_handler_with_caps().await;
        let id = capsule_manager
            .launch_local(std::path::Path::new("."), probe_manifest(), TrustLevel::Trusted)
            .await
            .expect("launch probe");

        let resp = handler
            .handle(
                &shell_id,
                inspect_request(
                    "elastos://inspect/capsule",
                    None,
                    Some(serde_json::json!({ "id": id.to_string() })),
                ),
            )
            .await;

        let data = match resp {
            RuntimeResponse::Ok { data: Some(data) } => data,
            other => panic!("expected Ok with data, got {:?}", other),
        };

        // Faithful projection of manifest-declared facts.
        assert_eq!(data["affordances"][0]["id"], "ping");
        assert_eq!(data["affordances"][0]["risk"], "read");
        assert_eq!(data["affordances"][0]["interface"], "elastos.probe/v1");
        assert_eq!(data["required_capabilities"][0], "elastos://storage/probe");
        assert_eq!(data["storage_namespaces"][0], "localhost://WebSpaces/probe/");
        assert_eq!(data["identity"]["signature_present"], true);

        // Principle #16: UI surfaces must not expose bearer tokens or mutation
        // handles. The raw signature is reduced to a boolean and never echoed,
        // and no bearer "token" field appears anywhere in the projection.
        let serialized = serde_json::to_string(&data).unwrap();
        assert!(
            !serialized.contains("SECRET_SIGNATURE_MUST_NOT_LEAK"),
            "raw signature leaked into inspect output"
        );
        assert!(
            !serialized.contains("\"token\""),
            "bearer token field leaked into inspect output"
        );
    }

    #[tokio::test]
    async fn inspect_self_returns_callers_own_record() {
        // Principle #7: any capsule (human-driven or agent) can introspect
        // itself with a minimal self grant — the same authority model for both.
        let (handler, _shell, caps, capsule_manager) = create_test_handler_with_caps().await;
        let id = capsule_manager
            .launch_local(std::path::Path::new("."), probe_manifest(), TrustLevel::Trusted)
            .await
            .expect("launch probe");

        let self_token = caps
            .grant(
                id.as_str(),
                ResourceId::new("elastos://inspect/self"),
                Action::Read,
                InternalConstraints::default(),
                None,
            )
            .to_base64()
            .expect("encode token");

        let resp = handler
            .handle(&id, inspect_request("elastos://inspect/self", Some(self_token), None))
            .await;

        match resp {
            RuntimeResponse::Ok { data: Some(data) } => {
                assert_eq!(data["id"], id.to_string());
                assert_eq!(data["name"], "probe");
            }
            other => panic!("expected Ok with own record, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn inspect_revoke_rejects_read_only_token() {
        // The crux of the read/write separation: a System *read* inspect grant
        // must never be able to drive a mutation. The action dimension blocks
        // it at the capability layer.
        let (handler, _shell, caps, _cm) = create_test_handler_with_caps().await;
        let caller = CapsuleId::new();
        let victim_capsule = CapsuleId::new();
        let victim = caps.grant(
            victim_capsule.as_str(),
            ResourceId::new("elastos://storage/x"),
            Action::Read,
            InternalConstraints::default(),
            None,
        );
        let read_token = caps
            .grant(
                caller.as_str(),
                ResourceId::new("elastos://inspect/*"),
                Action::Read,
                InternalConstraints::default(),
                None,
            )
            .to_base64()
            .expect("encode read token");

        let resp = handler
            .handle(
                &caller,
                inspect_request(
                    "elastos://inspect/revoke",
                    Some(read_token),
                    Some(serde_json::json!({ "token_id": victim.id().to_string() })),
                ),
            )
            .await;
        match resp {
            RuntimeResponse::Error { code, .. } => assert_eq!(code, "permission_denied"),
            other => panic!("read token must not revoke, got {:?}", other),
        }

        // Victim capability is still valid — the revoke did not happen.
        let res = ResourceId::new("elastos://storage/x");
        assert!(caps
            .validate(&victim, victim_capsule.as_str(), Action::Read, &res, None)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn inspect_shell_can_revoke_token() {
        let (handler, shell_id, caps, _cm) = create_test_handler_with_caps().await;
        let victim_capsule = CapsuleId::new();
        let res = ResourceId::new("elastos://storage/x");
        let victim = caps.grant(
            victim_capsule.as_str(),
            res.clone(),
            Action::Read,
            InternalConstraints::default(),
            None,
        );
        // Sanity: valid before revoke.
        assert!(caps
            .validate(&victim, victim_capsule.as_str(), Action::Read, &res, None)
            .await
            .is_ok());

        let resp = handler
            .handle(
                &shell_id,
                inspect_request(
                    "elastos://inspect/revoke",
                    None,
                    Some(serde_json::json!({ "token_id": victim.id().to_string() })),
                ),
            )
            .await;
        assert!(matches!(resp, RuntimeResponse::Ok { .. }), "shell revoke should succeed");

        // The capability is now revoked and fails validation.
        assert!(caps
            .validate(&victim, victim_capsule.as_str(), Action::Read, &res, None)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn inspect_write_token_at_system_scope_can_revoke() {
        // A non-shell System operator holding a Write inspect grant can revoke.
        let (handler, _shell, caps, _cm) = create_test_handler_with_caps().await;
        let operator = CapsuleId::new();
        let victim_capsule = CapsuleId::new();
        let res = ResourceId::new("elastos://storage/x");
        let victim = caps.grant(
            victim_capsule.as_str(),
            res.clone(),
            Action::Read,
            InternalConstraints::default(),
            None,
        );
        let write_token = caps
            .grant(
                operator.as_str(),
                ResourceId::new("elastos://inspect/*"),
                Action::Write,
                InternalConstraints::default(),
                None,
            )
            .to_base64()
            .expect("encode write token");

        let resp = handler
            .handle(
                &operator,
                inspect_request(
                    "elastos://inspect/revoke",
                    Some(write_token),
                    Some(serde_json::json!({ "token_id": victim.id().to_string() })),
                ),
            )
            .await;
        assert!(matches!(resp, RuntimeResponse::Ok { .. }), "write-scope revoke should succeed");
        assert!(caps
            .validate(&victim, victim_capsule.as_str(), Action::Read, &res, None)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn inspect_revoke_rejects_bad_token_id() {
        let (handler, shell_id, _caps, _cm) = create_test_handler_with_caps().await;
        let resp = handler
            .handle(
                &shell_id,
                inspect_request(
                    "elastos://inspect/revoke",
                    None,
                    Some(serde_json::json!({ "token_id": "not-hex" })),
                ),
            )
            .await;
        match resp {
            RuntimeResponse::Error { code, .. } => assert_eq!(code, "invalid_token_id"),
            other => panic!("expected invalid_token_id, got {:?}", other),
        }
    }

    fn inspect_request(
        uri: &str,
        token: Option<String>,
        params: Option<serde_json::Value>,
    ) -> RuntimeRequest {
        RuntimeRequest::ResourceRequest {
            uri: uri.to_string(),
            action: "read".to_string(),
            params,
            token,
        }
    }

    #[tokio::test]
    async fn inspect_shell_can_list_with_system_scope() {
        let (handler, shell_id) = create_test_handler().await;
        let resp = handler
            .handle(&shell_id, inspect_request("elastos://inspect/capsules", None, None))
            .await;
        match resp {
            RuntimeResponse::Ok { data: Some(data) } => {
                assert_eq!(data["scope"], "system");
                assert!(data["capsules"].is_array());
            }
            other => panic!("expected Ok with data, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn inspect_non_shell_without_token_is_denied() {
        let (handler, _shell) = create_test_handler().await;
        let caller = CapsuleId::new();
        let resp = handler
            .handle(&caller, inspect_request("elastos://inspect/capsules", None, None))
            .await;
        match resp {
            RuntimeResponse::Error { code, .. } => assert_eq!(code, "missing_token"),
            other => panic!("expected missing_token error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn inspect_self_only_token_cannot_reach_system_endpoints() {
        // The privilege-escalation guard at the handler boundary: a self-only
        // grant must not let a capsule enumerate or read other capsules.
        let (handler, shell_id, caps, _capsule_manager) = create_test_handler_with_caps().await;
        let caller = CapsuleId::new();
        let self_token = caps
            .grant(
                caller.as_str(),
                ResourceId::new("elastos://inspect/self"),
                Action::Read,
                InternalConstraints::default(),
                None,
            )
            .to_base64()
            .expect("encode token");

        // Read another capsule via the system detail endpoint: denied.
        let resp = handler
            .handle(
                &caller,
                inspect_request(
                    "elastos://inspect/capsule",
                    Some(self_token.clone()),
                    Some(serde_json::json!({ "id": shell_id.to_string() })),
                ),
            )
            .await;
        match resp {
            // Blocked at the capability layer: a self-only pattern cannot match
            // a system URI. (inspect::can_view is the defense-in-depth gate.)
            RuntimeResponse::Error { code, .. } => assert_eq!(code, "permission_denied"),
            other => panic!("expected permission_denied, got {:?}", other),
        }

        // Enumerate all capsules: also denied.
        let resp = handler
            .handle(
                &caller,
                inspect_request("elastos://inspect/capsules", Some(self_token), None),
            )
            .await;
        assert!(
            matches!(resp, RuntimeResponse::Error { .. }),
            "self-only grant must not list all capsules"
        );
    }

    #[tokio::test]
    async fn inspect_unknown_endpoint_is_not_found() {
        let (handler, shell_id) = create_test_handler().await;
        let resp = handler
            .handle(&shell_id, inspect_request("elastos://inspect/bogus", None, None))
            .await;
        match resp {
            RuntimeResponse::Error { code, .. } => assert_eq!(code, "not_found"),
            other => panic!("expected not_found, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_ping() {
        let (handler, shell_id) = create_test_handler().await;

        let response = handler.handle(&shell_id, RuntimeRequest::Ping).await;
        assert!(matches!(response, RuntimeResponse::Pong));
    }

    #[tokio::test]
    async fn test_get_runtime_info() {
        let (handler, shell_id) = create_test_handler().await;

        let response = handler
            .handle(&shell_id, RuntimeRequest::GetRuntimeInfo)
            .await;

        match response {
            RuntimeResponse::RuntimeInfo {
                version,
                capsule_count,
            } => {
                assert_eq!(version, "0.1.0");
                assert_eq!(capsule_count, 0);
            }
            _ => panic!("Expected RuntimeInfo response"),
        }
    }

    #[tokio::test]
    async fn test_list_capsules_authorized() {
        let (handler, shell_id) = create_test_handler().await;

        let response = handler
            .handle(&shell_id, RuntimeRequest::ListCapsules)
            .await;

        match response {
            RuntimeResponse::CapsuleList { capsules } => {
                assert!(capsules.is_empty());
            }
            _ => panic!("Expected CapsuleList response"),
        }
    }

    #[tokio::test]
    async fn test_list_capsules_unauthorized() {
        let (handler, _shell_id) = create_test_handler().await;
        let other_capsule = CapsuleId::new();

        let response = handler
            .handle(&other_capsule, RuntimeRequest::ListCapsules)
            .await;

        match response {
            RuntimeResponse::Error { code, .. } => {
                assert_eq!(code, "unauthorized");
            }
            _ => panic!("Expected Error response"),
        }
    }

    #[tokio::test]
    async fn test_launch_capsule_rejects_sha256_identifier() {
        let (handler, shell_id) = create_test_handler().await;

        let response = handler
            .handle(
                &shell_id,
                RuntimeRequest::LaunchCapsule {
                    cid: "sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
                        .to_string(),
                    config: LaunchConfig::default(),
                },
            )
            .await;

        match response {
            RuntimeResponse::Error { code, message } => {
                assert_eq!(code, "invalid_input");
                assert!(message.contains("only bare IPFS CIDs"));
            }
            _ => panic!("Expected invalid_input error, got {:?}", response),
        }
    }

    #[tokio::test]
    async fn test_launch_capsule_rejects_uri_subpath() {
        let (handler, shell_id) = create_test_handler().await;

        let response = handler
            .handle(
                &shell_id,
                RuntimeRequest::LaunchCapsule {
                    cid: "elastos://QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG/index.wasm"
                        .to_string(),
                    config: LaunchConfig::default(),
                },
            )
            .await;

        match response {
            RuntimeResponse::Error { code, message } => {
                assert_eq!(code, "invalid_input");
                assert!(message.contains("does not accept elastos:// sub-paths"));
            }
            _ => panic!("Expected invalid_input error, got {:?}", response),
        }
    }

    #[tokio::test]
    async fn test_launch_capsule_bare_ipfs_cid_reaches_fetch() {
        let (handler, shell_id) = create_test_handler().await;

        let response = handler
            .handle(
                &shell_id,
                RuntimeRequest::LaunchCapsule {
                    cid: "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG".to_string(),
                    config: LaunchConfig::default(),
                },
            )
            .await;

        match response {
            RuntimeResponse::Error { code, .. } => {
                assert_eq!(code, "fetch_failed");
            }
            _ => panic!("Expected fetch_failed error, got {:?}", response),
        }
    }

    #[tokio::test]
    async fn test_grant_capability() {
        let (handler, shell_id) = create_test_handler().await;

        let response = handler
            .handle(
                &shell_id,
                RuntimeRequest::GrantCapability {
                    capsule_id: "test-capsule".to_string(),
                    resource: "localhost://Users/self/Documents/test.txt".to_string(),
                    action: "read".to_string(),
                    constraints: CapabilityConstraints::default(),
                },
            )
            .await;

        match response {
            RuntimeResponse::CapabilityGranted { token_id } => {
                assert!(!token_id.is_empty());
            }
            _ => panic!("Expected CapabilityGranted response"),
        }
    }

    #[tokio::test]
    async fn test_grant_capability_unauthorized() {
        let (handler, _shell_id) = create_test_handler().await;
        let other_capsule = CapsuleId::new();

        let response = handler
            .handle(
                &other_capsule,
                RuntimeRequest::GrantCapability {
                    capsule_id: "test-capsule".to_string(),
                    resource: "localhost://Users/self/Documents/test.txt".to_string(),
                    action: "read".to_string(),
                    constraints: CapabilityConstraints::default(),
                },
            )
            .await;

        match response {
            RuntimeResponse::Error { code, .. } => {
                assert_eq!(code, "unauthorized");
            }
            _ => panic!("Expected Error response"),
        }
    }

    // --- Capability enforcement tests for storage ops ---

    #[tokio::test]
    async fn test_storage_read_rejected_without_token() {
        let (handler, _shell_id) = create_test_handler().await;
        let capsule = CapsuleId::new();

        let response = handler
            .handle(
                &capsule,
                RuntimeRequest::StorageRead {
                    token: String::new(),
                    path: "localhost://Users/self/Documents/test.txt".to_string(),
                },
            )
            .await;

        match response {
            RuntimeResponse::Error { code, .. } => {
                assert_eq!(code, "missing_token");
            }
            _ => panic!("Expected missing_token error, got {:?}", response),
        }
    }

    #[tokio::test]
    async fn test_storage_write_rejected_without_token() {
        let (handler, _shell_id) = create_test_handler().await;
        let capsule = CapsuleId::new();

        let response = handler
            .handle(
                &capsule,
                RuntimeRequest::StorageWrite {
                    token: String::new(),
                    path: "localhost://Users/self/Documents/test.txt".to_string(),
                    content: b"data".to_vec(),
                },
            )
            .await;

        match response {
            RuntimeResponse::Error { code, .. } => {
                assert_eq!(code, "missing_token");
            }
            _ => panic!("Expected missing_token error, got {:?}", response),
        }
    }

    #[tokio::test]
    async fn test_storage_read_rejected_with_invalid_token() {
        let (handler, _shell_id) = create_test_handler().await;
        let capsule = CapsuleId::new();

        let response = handler
            .handle(
                &capsule,
                RuntimeRequest::StorageRead {
                    token: "not-valid-base64-token".to_string(),
                    path: "localhost://Users/self/Documents/test.txt".to_string(),
                },
            )
            .await;

        match response {
            RuntimeResponse::Error { code, .. } => {
                assert_eq!(code, "invalid_token");
            }
            _ => panic!("Expected invalid_token error, got {:?}", response),
        }
    }

    #[tokio::test]
    async fn test_storage_read_shell_exempt() {
        let (handler, shell_id) = create_test_handler().await;

        // Shell doesn't need a token — should not get a token error
        // (may fail for other reasons like no provider, but not for missing token)
        let response = handler
            .handle(
                &shell_id,
                RuntimeRequest::StorageRead {
                    token: String::new(),
                    path: "localhost://Users/self/Documents/test.txt".to_string(),
                },
            )
            .await;

        if let RuntimeResponse::Error { code, .. } = response {
            assert_ne!(
                code, "missing_token",
                "Shell should be exempt from token check"
            );
            assert_ne!(
                code, "invalid_token",
                "Shell should be exempt from token check"
            );
            assert_ne!(
                code, "permission_denied",
                "Shell should be exempt from token check"
            );
        }
    }

    #[tokio::test]
    async fn test_fetch_content_rejected_without_token() {
        let (handler, _shell_id) = create_test_handler().await;
        let capsule = CapsuleId::new();

        let response = handler
            .handle(
                &capsule,
                RuntimeRequest::FetchContent {
                    uri: "elastos://QmExample".to_string(),
                    token: None,
                },
            )
            .await;

        match response {
            RuntimeResponse::Error { code, .. } => assert_eq!(code, "missing_token"),
            _ => panic!("Expected missing_token error, got {:?}", response),
        }
    }

    #[tokio::test]
    async fn test_fetch_content_shell_exempt() {
        let (handler, shell_id) = create_test_handler().await;

        let response = handler
            .handle(
                &shell_id,
                RuntimeRequest::FetchContent {
                    uri: "elastos://QmExample".to_string(),
                    token: None,
                },
            )
            .await;

        if let RuntimeResponse::Error { code, .. } = response {
            assert_ne!(code, "missing_token");
            assert_ne!(code, "permission_denied");
        }
    }

    #[tokio::test]
    async fn test_fetch_content_with_valid_token_does_not_fail_auth() {
        let (handler, _shell_id) = create_test_handler().await;
        let capsule = CapsuleId::new();

        let token = handler.capability_manager.grant(
            capsule.as_str(),
            crate::capability::token::ResourceId::new("elastos://QmExample"),
            crate::capability::token::Action::Read,
            crate::capability::token::TokenConstraints::default(),
            None,
        );

        let response = handler
            .handle(
                &capsule,
                RuntimeRequest::FetchContent {
                    uri: "elastos://QmExample".to_string(),
                    token: Some(token.to_base64().unwrap()),
                },
            )
            .await;

        if let RuntimeResponse::Error { code, .. } = &response {
            assert_ne!(code, "missing_token");
            assert_ne!(code, "invalid_token");
            assert_ne!(code, "permission_denied");
        }
    }

    // --- Capability enforcement tests for messaging ---

    #[tokio::test]
    async fn test_send_message_rejected_without_token() {
        let (handler, _shell_id) = create_test_handler().await;
        let capsule = CapsuleId::new();

        let response = handler
            .handle(
                &capsule,
                RuntimeRequest::SendMessage {
                    to: "some-capsule".to_string(),
                    payload: b"hello".to_vec(),
                    reply_to: None,
                    token: None,
                },
            )
            .await;

        match response {
            RuntimeResponse::Error { code, .. } => {
                assert_eq!(code, "missing_token");
            }
            _ => panic!("Expected missing_token error, got {:?}", response),
        }
    }

    #[tokio::test]
    async fn test_send_message_rejected_with_invalid_token() {
        let (handler, _shell_id) = create_test_handler().await;
        let capsule = CapsuleId::new();

        let response = handler
            .handle(
                &capsule,
                RuntimeRequest::SendMessage {
                    to: "some-capsule".to_string(),
                    payload: b"hello".to_vec(),
                    reply_to: None,
                    token: Some("not-valid-base64".to_string()),
                },
            )
            .await;

        match response {
            RuntimeResponse::Error { code, .. } => {
                assert_eq!(code, "invalid_token");
            }
            _ => panic!("Expected invalid_token error, got {:?}", response),
        }
    }

    #[tokio::test]
    async fn test_send_message_shell_exempt() {
        let (handler, shell_id) = create_test_handler().await;

        // Register message channel for the shell
        handler.message_channel.register(shell_id.as_str()).await;
        handler.message_channel.register("target-capsule").await;

        // Shell doesn't need a token
        let response = handler
            .handle(
                &shell_id,
                RuntimeRequest::SendMessage {
                    to: "target-capsule".to_string(),
                    payload: b"hello".to_vec(),
                    reply_to: None,
                    token: None,
                },
            )
            .await;

        assert!(
            matches!(response, RuntimeResponse::Ok { .. }),
            "Shell should be able to send messages without token, got {:?}",
            response
        );
    }

    // --- Capability enforcement tests for ResourceRequest ---

    #[tokio::test]
    async fn test_resource_request_rejected_without_token() {
        let (handler, _shell_id) = create_test_handler().await;
        let capsule = CapsuleId::new();

        let response = handler
            .handle(
                &capsule,
                RuntimeRequest::ResourceRequest {
                    uri: "localhost://Users/self/Documents/secret.txt".to_string(),
                    action: "read".to_string(),
                    params: None,
                    token: None,
                },
            )
            .await;

        match response {
            RuntimeResponse::Error { code, .. } => {
                assert_eq!(code, "missing_token");
            }
            _ => panic!("Expected missing_token error, got {:?}", response),
        }
    }

    #[tokio::test]
    async fn test_resource_request_rejected_with_invalid_token() {
        let (handler, _shell_id) = create_test_handler().await;
        let capsule = CapsuleId::new();

        let response = handler
            .handle(
                &capsule,
                RuntimeRequest::ResourceRequest {
                    uri: "localhost://Users/self/Documents/secret.txt".to_string(),
                    action: "read".to_string(),
                    params: None,
                    token: Some("not-valid-base64".to_string()),
                },
            )
            .await;

        match response {
            RuntimeResponse::Error { code, .. } => {
                assert_eq!(code, "invalid_token");
            }
            _ => panic!("Expected invalid_token error, got {:?}", response),
        }
    }

    #[tokio::test]
    async fn test_resource_request_shell_exempt() {
        let (handler, shell_id) = create_test_handler().await;

        // Shell doesn't need a token — should not get a token error
        let response = handler
            .handle(
                &shell_id,
                RuntimeRequest::ResourceRequest {
                    uri: "localhost://Users/self/Documents/file.txt".to_string(),
                    action: "read".to_string(),
                    params: None,
                    token: None,
                },
            )
            .await;

        if let RuntimeResponse::Error { code, .. } = response {
            assert_ne!(
                code, "missing_token",
                "Shell should be exempt from token check"
            );
            assert_ne!(
                code, "invalid_token",
                "Shell should be exempt from token check"
            );
            assert_ne!(
                code, "permission_denied",
                "Shell should be exempt from token check"
            );
        }
    }

    #[tokio::test]
    async fn test_resource_request_write_rejected_without_token() {
        let (handler, _shell_id) = create_test_handler().await;
        let capsule = CapsuleId::new();

        let response = handler
            .handle(
                &capsule,
                RuntimeRequest::ResourceRequest {
                    uri: "localhost://Users/self/Documents/file.txt".to_string(),
                    action: "write".to_string(),
                    params: Some(serde_json::json!({"content": [1, 2, 3]})),
                    token: None,
                },
            )
            .await;

        match response {
            RuntimeResponse::Error { code, .. } => {
                assert_eq!(code, "missing_token");
            }
            _ => panic!("Expected missing_token error, got {:?}", response),
        }
    }

    #[tokio::test]
    async fn test_resource_request_delete_rejected_without_token() {
        let (handler, _shell_id) = create_test_handler().await;
        let capsule = CapsuleId::new();

        let response = handler
            .handle(
                &capsule,
                RuntimeRequest::ResourceRequest {
                    uri: "localhost://Users/self/Documents/file.txt".to_string(),
                    action: "delete".to_string(),
                    params: None,
                    token: None,
                },
            )
            .await;

        match response {
            RuntimeResponse::Error { code, .. } => {
                assert_eq!(code, "missing_token");
            }
            _ => panic!("Expected missing_token error, got {:?}", response),
        }
    }

    // === H4c: Handler security boundary tests ===

    #[tokio::test]
    async fn test_non_shell_with_valid_token_can_read_storage() {
        let (handler, _shell_id) = create_test_handler().await;
        let capsule = CapsuleId::new();

        // Grant a token for this capsule
        let token = handler.capability_manager.grant(
            capsule.as_str(),
            crate::capability::token::ResourceId::new("localhost://Users/self/Documents/test.txt"),
            crate::capability::token::Action::Read,
            crate::capability::token::TokenConstraints::default(),
            None,
        );

        let token_b64 = token.to_base64().unwrap();

        let response = handler
            .handle(
                &capsule,
                RuntimeRequest::StorageRead {
                    token: token_b64,
                    path: "localhost://Users/self/Documents/test.txt".to_string(),
                },
            )
            .await;

        // Should NOT get a token/permission error (may fail with no_provider, which is fine)
        if let RuntimeResponse::Error { code, .. } = &response {
            assert_ne!(
                code, "missing_token",
                "Valid token should not trigger missing_token"
            );
            assert_ne!(
                code, "invalid_token",
                "Valid token should not trigger invalid_token"
            );
            assert_ne!(
                code, "permission_denied",
                "Valid token should not trigger permission_denied"
            );
        }
    }

    #[tokio::test]
    async fn test_non_shell_with_wrong_resource_token_rejected() {
        let (handler, _shell_id) = create_test_handler().await;
        let capsule = CapsuleId::new();

        // Grant a token for photos, but try to access documents
        let token = handler.capability_manager.grant(
            capsule.as_str(),
            crate::capability::token::ResourceId::new("localhost://Users/self/Documents/photos/*"),
            crate::capability::token::Action::Read,
            crate::capability::token::TokenConstraints::default(),
            None,
        );

        let token_b64 = token.to_base64().unwrap();

        let response = handler
            .handle(
                &capsule,
                RuntimeRequest::StorageRead {
                    token: token_b64,
                    path: "localhost://Users/self/Documents/documents/secret.txt".to_string(),
                },
            )
            .await;

        match response {
            RuntimeResponse::Error { code, .. } => {
                assert_eq!(
                    code, "permission_denied",
                    "Wrong-resource token must be rejected"
                );
            }
            _ => panic!("Expected permission_denied error, got {:?}", response),
        }
    }

    // === H4d: Messaging auth tests ===

    #[tokio::test]
    async fn test_non_shell_with_valid_token_can_send_message() {
        let (handler, _shell_id) = create_test_handler().await;
        let sender = CapsuleId::new();
        let receiver_id = "receiver-capsule";

        // Register both in message channel
        handler.message_channel.register(sender.as_str()).await;
        handler.message_channel.register(receiver_id).await;

        // Grant a messaging token
        let token = handler.capability_manager.grant(
            sender.as_str(),
            crate::capability::token::ResourceId::new(format!("elastos://message/{}", receiver_id)),
            crate::capability::token::Action::Message,
            crate::capability::token::TokenConstraints::default(),
            None,
        );

        let token_b64 = token.to_base64().unwrap();

        let response = handler
            .handle(
                &sender,
                RuntimeRequest::SendMessage {
                    to: receiver_id.to_string(),
                    payload: b"hello from capsule".to_vec(),
                    reply_to: None,
                    token: Some(token_b64),
                },
            )
            .await;

        assert!(
            matches!(response, RuntimeResponse::Ok { .. }),
            "Non-shell with valid messaging token should succeed, got {:?}",
            response
        );
    }

    #[tokio::test]
    async fn test_non_shell_without_token_cannot_send_message() {
        let (handler, _shell_id) = create_test_handler().await;
        let sender = CapsuleId::new();

        let response = handler
            .handle(
                &sender,
                RuntimeRequest::SendMessage {
                    to: "someone".to_string(),
                    payload: b"hello".to_vec(),
                    reply_to: None,
                    token: None,
                },
            )
            .await;

        match response {
            RuntimeResponse::Error { code, .. } => {
                assert_eq!(code, "missing_token");
            }
            _ => panic!("Expected missing_token error, got {:?}", response),
        }
    }
}
