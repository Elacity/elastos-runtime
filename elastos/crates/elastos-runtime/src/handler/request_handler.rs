//! Request handler implementation
//!
//! Processes RuntimeRequest messages and returns RuntimeResponse.
//! Enforces authorization and delegates to appropriate managers.

// Used by lib crate (tests, API handlers) but not directly by main.rs binary

use std::sync::Arc;
use tokio::sync::RwLock;

use elastos_common::localhost::{is_supported_resource_scheme, rooted_localhost_uri};
use elastos_namespace::ContentUri;

use crate::capability::token::{Action, ResourceId, TokenConstraints as InternalConstraints};
use crate::capability::CapabilityManager;
use crate::capsule::{prepare_fetched_capsule, CapsuleId, CapsuleManager};
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

/// Request handler for capsule-to-runtime communication
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
