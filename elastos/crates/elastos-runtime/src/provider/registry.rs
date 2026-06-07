//! Provider registry implementation
//!
//! The registry maintains a mapping of URL schemes to providers.
//! Supports hierarchical `elastos://` sub-dispatch: `elastos://peer/alice`
//! routes to the `peer` sub-provider with path `alice`.
//!
//! All first-party providers (did, peer, ai) use the `elastos://` namespace
//! exclusively: `elastos://did/*`, `elastos://peer/*`, `elastos://ai/*`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

use base64::Engine as _;
use elastos_common::localhost::{parse_localhost_path, parse_localhost_uri};

/// A resource request
#[derive(Debug, Clone)]
pub struct ResourceRequest {
    /// The full URI (e.g., "localhost://Users/self/Documents/photos/vacation.jpg")
    pub uri: String,
    /// The scheme (e.g., "local")
    pub _scheme: String,
    /// The path after the scheme (e.g., "photos/vacation.jpg")
    pub path: String,
    /// The capsule making the request
    pub _capsule_id: String,
    /// The action being performed
    pub action: ResourceAction,
    /// Optional content for write operations
    pub content: Option<Vec<u8>>,
    /// Whether to operate recursively (e.g., recursive delete)
    pub recursive: bool,
}

/// Action being performed on a resource
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceAction {
    /// Read the resource
    Read,
    /// Write to the resource
    Write,
    /// Delete the resource
    Delete,
    /// List resources (for directories)
    List,
    /// Check if resource exists
    Exists,
    /// Get metadata (stat)
    Stat,
    /// Create a directory
    Mkdir,
}

/// Response from a provider
#[derive(Debug, Clone)]
pub enum ResourceResponse {
    /// Read data
    Data(Vec<u8>),
    /// Write successful
    Written { bytes: usize },
    /// Delete successful
    Deleted,
    /// List of resources
    List(Vec<ResourceEntry>),
    /// Exists check result
    Exists(bool),
    /// No content (success)
    Ok,
    /// Metadata response (for stat)
    Metadata {
        size: u64,
        entry_type: EntryType,
        modified: u64,
    },
    /// Directory created
    Created,
}

/// Entry type for metadata
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryType {
    File,
    Directory,
}

/// Entry in a resource list
#[derive(Debug, Clone)]
pub struct ResourceEntry {
    /// Resource name
    pub name: String,
    /// Whether it's a directory
    pub is_directory: bool,
    /// Size in bytes (if applicable)
    pub size: Option<u64>,
    /// Last modified timestamp (unix seconds)
    pub modified: Option<u64>,
}

/// Provider errors
#[derive(Debug)]
pub enum ProviderError {
    /// Resource not found
    NotFound(String),
    /// Permission denied
    PermissionDenied(String),
    /// Invalid URI
    InvalidUri(String),
    /// Provider error
    Provider(String),
    /// No provider for scheme
    NoProvider(String),
    /// IO error
    Io(std::io::Error),
}

/// Provider-to-provider invocation transfer contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderTransfer {
    /// JSON request/response envelope.
    Json,
    /// JSON envelope whose response may carry bounded byte arrays.
    Bytes,
    /// JSON envelope whose response carries bounded byte chunks.
    Stream,
}

impl ProviderTransfer {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderTransfer::Json => "json",
            ProviderTransfer::Bytes => "bytes",
            ProviderTransfer::Stream => "stream",
        }
    }
}

const PROVIDER_STREAM_SCHEMA: &str = "elastos.provider.stream/v1";
const PROVIDER_STREAM_ENCODING: &str = "base64-chunks";
const PROVIDER_STREAM_CHUNK_BYTES: usize = 64 * 1024;
const PROVIDER_STREAM_SESSION_SCHEMA: &str = "elastos.provider.stream-session/v1";
const PROVIDER_STREAM_EVENT_SCHEMA: &str = "elastos.provider.stream-event/v1";
static NEXT_PROVIDER_STREAM_ID: AtomicU64 = AtomicU64::new(1);

/// Carrier route for Runtime-mediated provider-to-provider invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCarrierRoute {
    /// Internal Carrier connect ticket. Receipts intentionally do not expose it.
    pub connect_ticket: String,
    /// Optional expected remote peer DID/node identity for audit and policy.
    pub peer_did: Option<String>,
    /// Optional per-hop timeout. Runtime rejects zero-duration values.
    pub timeout_ms: Option<u64>,
}

/// Runtime transport used for provider-to-provider invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderInvocationTransport {
    /// In-process Runtime provider registry.
    Local,
    /// Carrier provider plane. Apps never select this directly.
    Carrier(ProviderCarrierRoute),
}

impl ProviderInvocationTransport {
    fn as_str(&self) -> &'static str {
        match self {
            ProviderInvocationTransport::Local => "runtime-local-provider-plane",
            ProviderInvocationTransport::Carrier(_) => "carrier-provider-plane",
        }
    }

    fn carrier_route(&self) -> Option<&ProviderCarrierRoute> {
        match self {
            ProviderInvocationTransport::Local => None,
            ProviderInvocationTransport::Carrier(route) => Some(route),
        }
    }
}

impl Default for ProviderInvocationTransport {
    fn default() -> Self {
        Self::Local
    }
}

/// Optional byte range requested for a provider-to-provider transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderByteRange {
    /// Inclusive start byte.
    pub start: u64,
    /// Inclusive end byte. `None` means open-ended.
    pub end: Option<u64>,
}

/// Optional progress receipt metadata for a provider-to-provider transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderProgress {
    /// Runtime- or provider-owned request id used to correlate progress receipts.
    pub request_id: String,
    /// Expected byte count when known.
    pub expected_bytes: Option<u64>,
}

/// Runtime stream-session flow-control options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderStreamOptions {
    /// Maximum chunk size returned by each `read_next` call.
    pub chunk_size: usize,
    /// Maximum chunks the consumer may request ahead of processing. Runtime
    /// sessions currently enforce one explicit read per chunk.
    pub max_in_flight_chunks: usize,
}

impl Default for ProviderStreamOptions {
    fn default() -> Self {
        Self {
            chunk_size: PROVIDER_STREAM_CHUNK_BYTES,
            max_in_flight_chunks: 1,
        }
    }
}

/// One Runtime-native stream read.
#[derive(Debug, Clone)]
pub struct ProviderStreamRead {
    /// Runtime-owned stream session id.
    pub session_id: String,
    /// Zero-based chunk index.
    pub index: usize,
    /// Byte offset of this chunk in the stream.
    pub offset: usize,
    /// Chunk bytes.
    pub bytes: Vec<u8>,
    /// Whether this read completed the stream.
    pub completed: bool,
    /// Runtime progress event for this read.
    pub progress: serde_json::Value,
}

/// Runtime-owned provider stream session.
#[derive(Debug, Clone)]
pub struct ProviderStreamSession {
    id: String,
    source: String,
    target: String,
    op: String,
    request_id: String,
    bytes: Vec<u8>,
    cursor: usize,
    read_index: usize,
    chunk_size: usize,
    max_in_flight_chunks: usize,
    cancelled: bool,
    transfer_receipt: Option<serde_json::Value>,
}

impl ProviderStreamSession {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn total_bytes(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    pub fn receipt(&self) -> serde_json::Value {
        serde_json::json!({
            "schema": PROVIDER_STREAM_SESSION_SCHEMA,
            "session_id": self.id,
            "source": self.source,
            "target": self.target,
            "op": self.op,
            "request_id": self.request_id,
            "total_bytes": self.bytes.len(),
            "chunk_size": self.chunk_size,
            "max_in_flight_chunks": self.max_in_flight_chunks,
            "transport_native_stream": true,
            "backpressure": "read_next",
            "cancel_supported": true,
            "progress_mode": "stream_events",
            "status": if self.cancelled {
                "cancelled"
            } else if self.cursor >= self.bytes.len() {
                "completed"
            } else {
                "open"
            },
            "transfer": self.transfer_receipt,
        })
    }

    pub fn read_next(&mut self) -> Result<Option<ProviderStreamRead>, ProviderError> {
        if self.cancelled {
            return Err(ProviderError::Provider(format!(
                "provider stream session {} is cancelled",
                self.id
            )));
        }
        if self.cursor >= self.bytes.len() {
            return Ok(None);
        }
        let start = self.cursor;
        let end = (start + self.chunk_size).min(self.bytes.len());
        self.cursor = end;
        let index = self.read_index;
        self.read_index += 1;
        let completed = self.cursor >= self.bytes.len();
        let chunk = self.bytes[start..end].to_vec();
        let progress = self.progress_event(
            index,
            start,
            chunk.len(),
            if completed { "completed" } else { "progress" },
        );
        Ok(Some(ProviderStreamRead {
            session_id: self.id.clone(),
            index,
            offset: start,
            bytes: chunk,
            completed,
            progress,
        }))
    }

    pub fn cancel(&mut self) -> serde_json::Value {
        self.cancelled = true;
        self.progress_event(self.read_index, self.cursor, 0, "cancelled")
    }

    pub fn drain_to_vec(&mut self) -> Result<Vec<u8>, ProviderError> {
        let mut out = Vec::with_capacity(self.bytes.len().saturating_sub(self.cursor));
        while let Some(read) = self.read_next()? {
            out.extend_from_slice(&read.bytes);
        }
        Ok(out)
    }

    fn progress_event(
        &self,
        index: usize,
        offset: usize,
        bytes: usize,
        status: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "schema": PROVIDER_STREAM_EVENT_SCHEMA,
            "session_id": self.id,
            "request_id": self.request_id,
            "source": self.source,
            "target": self.target,
            "op": self.op,
            "index": index,
            "offset": offset,
            "bytes": bytes,
            "transferred_bytes": self.cursor,
            "total_bytes": self.bytes.len(),
            "status": status,
        })
    }
}

/// Typed provider-to-provider invocation routed by the Runtime registry.
#[derive(Debug, Clone)]
pub struct ProviderInvocation {
    /// Provider initiating the call.
    pub source: String,
    /// Target provider scheme or sub-provider name.
    pub target: String,
    /// Target operation name for audit/debug validation.
    pub op: String,
    /// Raw request understood by the target provider.
    pub request: serde_json::Value,
    /// Expected transfer shape.
    pub transfer: ProviderTransfer,
    /// Optional byte range contract for bytes/stream transfers.
    pub range: Option<ProviderByteRange>,
    /// Optional progress receipt contract for large transfers.
    pub progress: Option<ProviderProgress>,
    /// Runtime-owned provider transport.
    pub transport: ProviderInvocationTransport,
}

/// Runtime plug-in point for Carrier provider-plane transport.
#[async_trait::async_trait]
pub trait ProviderCarrierInvoker: Send + Sync {
    /// Send an already Runtime-enveloped provider request to a remote Carrier peer.
    async fn invoke_carrier_provider(
        &self,
        route: &ProviderCarrierRoute,
        invocation: &ProviderInvocation,
        request: serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError>;
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderError::NotFound(uri) => write!(f, "resource not found: {}", uri),
            ProviderError::PermissionDenied(msg) => write!(f, "permission denied: {}", msg),
            ProviderError::InvalidUri(uri) => write!(f, "invalid URI: {}", uri),
            ProviderError::Provider(msg) => write!(f, "provider error: {}", msg),
            ProviderError::NoProvider(scheme) => write!(f, "no provider for scheme: {}", scheme),
            ProviderError::Io(e) => write!(f, "IO error: {}", e),
        }
    }
}

impl std::error::Error for ProviderError {}

impl From<std::io::Error> for ProviderError {
    fn from(e: std::io::Error) -> Self {
        ProviderError::Io(e)
    }
}

/// A resource provider trait
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    /// Handle a resource request
    async fn handle(&self, request: ResourceRequest) -> Result<ResourceResponse, ProviderError>;

    /// Get the schemes this provider handles
    fn schemes(&self) -> Vec<&'static str>;

    /// Get provider name
    fn name(&self) -> &'static str;

    /// Send raw JSON to the provider (for generic proxy).
    /// Default implementation returns an error.
    async fn send_raw(
        &self,
        _request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        Err(ProviderError::Provider(
            "Provider does not support raw communication".into(),
        ))
    }
}

/// Reserved sub-provider names for `elastos://` hierarchical dispatch.
/// Only these names can be registered as sub-providers (guards against typos).
const RESERVED_SUB_NAMES: &[&str] = &[
    "peer",
    "did",
    "ai",
    "llama",
    "ipfs",
    "content",
    "tunnel",
    "storage",
    "namespace",
    "message",
    "chain",
    "net",
    "exit",
    "browser-engine",
    "wallet",
    "library",
    "drm",
    "rights",
    "key",
    "decrypt",
    "availability",
    "block-graph",
];

/// Registry of providers
pub struct ProviderRegistry {
    /// Map of scheme -> provider
    providers: RwLock<HashMap<String, Arc<dyn Provider>>>,
    /// Sub-providers for elastos:// hierarchical dispatch (e.g., elastos://peer/...)
    sub_providers: RwLock<HashMap<String, Arc<dyn Provider>>>,
    /// Optional Carrier transport for Runtime-mediated provider invocation.
    carrier_invoker: RwLock<Option<Arc<dyn ProviderCarrierInvoker>>>,
}

impl ProviderRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            providers: RwLock::new(HashMap::new()),
            sub_providers: RwLock::new(HashMap::new()),
            carrier_invoker: RwLock::new(None),
        }
    }

    /// Register the Runtime-owned Carrier provider-plane invoker.
    pub async fn set_carrier_invoker(&self, invoker: Arc<dyn ProviderCarrierInvoker>) {
        *self.carrier_invoker.write().await = Some(invoker);
    }

    /// Register a provider for its schemes
    pub async fn register(&self, provider: Arc<dyn Provider>) {
        let mut providers = self.providers.write().await;
        for scheme in provider.schemes() {
            tracing::info!(
                "Registered provider '{}' for scheme '{}'",
                provider.name(),
                scheme
            );
            providers.insert(scheme.to_string(), provider.clone());
        }
    }

    /// Unregister a provider
    pub async fn unregister(&self, scheme: &str) {
        let mut providers = self.providers.write().await;
        if let Some(provider) = providers.remove(scheme) {
            tracing::info!(
                "Unregistered provider '{}' for scheme '{}'",
                provider.name(),
                scheme
            );
        }
    }

    /// Get a provider for a scheme
    pub async fn get(&self, scheme: &str) -> Option<Arc<dyn Provider>> {
        let providers = self.providers.read().await;
        providers.get(scheme).cloned()
    }

    /// Register a sub-provider for `elastos://` hierarchical dispatch.
    ///
    /// `name` must be in [`RESERVED_SUB_NAMES`]; returns an error for
    /// unknown names so callers fail fast on typos.
    pub async fn register_sub_provider(
        &self,
        name: &str,
        provider: Arc<dyn Provider>,
    ) -> Result<(), ProviderError> {
        let name = name.to_lowercase();
        if !RESERVED_SUB_NAMES.contains(&name.as_str()) {
            return Err(ProviderError::Provider(format!(
                "sub-provider '{}' is not a reserved name",
                name
            )));
        }
        tracing::info!(
            "Registered sub-provider '{}' for elastos://{}/...",
            provider.name(),
            name
        );
        self.sub_providers.write().await.insert(name, provider);
        Ok(())
    }

    /// Unregister a sub-provider from `elastos://` hierarchical dispatch.
    ///
    /// No-op if the name is not currently registered.
    pub async fn unregister_sub_provider(&self, name: &str) {
        let key = name.to_lowercase();
        if let Some(provider) = self.sub_providers.write().await.remove(&key) {
            tracing::info!(
                "Unregistered sub-provider '{}' for elastos://{}/...",
                provider.name(),
                key
            );
        }
    }

    /// Get a sub-provider by name (case-insensitive).
    async fn get_sub_provider(&self, name: &str) -> Option<Arc<dyn Provider>> {
        let key = name.to_lowercase();
        self.sub_providers.read().await.get(&key).cloned()
    }

    /// Split an `elastos://` path into `(sub_name, remainder)`.
    ///
    /// - `"peer/alice/shared"` → `Some(("peer", "alice/shared"))`
    /// - `"peer"`              → `Some(("peer", ""))`
    /// - `""`                  → `None`
    fn split_sub_path(path: &str) -> Option<(&str, &str)> {
        if path.is_empty() {
            return None;
        }
        match path.find('/') {
            Some(pos) => Some((&path[..pos], &path[pos + 1..])),
            None => Some((path, "")),
        }
    }

    /// Parse a URI and route to the appropriate provider
    pub async fn route(
        &self,
        uri: &str,
        capsule_id: &str,
        action: ResourceAction,
        content: Option<Vec<u8>>,
    ) -> Result<ResourceResponse, ProviderError> {
        self.route_with_options(uri, capsule_id, action, content, false)
            .await
    }

    /// Parse a URI and route to the appropriate provider with options.
    ///
    /// For `elastos://` URIs the first path segment is checked against
    /// registered sub-providers. `elastos://peer/alice/shared` dispatches
    /// to the `peer` sub-provider with `scheme: "peer"`, `path: "alice/shared"`.
    pub async fn route_with_options(
        &self,
        uri: &str,
        capsule_id: &str,
        action: ResourceAction,
        content: Option<Vec<u8>>,
        recursive: bool,
    ) -> Result<ResourceResponse, ProviderError> {
        let (scheme, path) = Self::parse_uri(uri)?;

        if scheme == "localhost" && Self::is_webspaces_localhost_path(&path) {
            if let Some(provider) = self.get("webspace").await {
                let request = ResourceRequest {
                    uri: uri.to_string(),
                    _scheme: "webspace".to_string(),
                    path,
                    _capsule_id: capsule_id.to_string(),
                    action,
                    content,
                    recursive,
                };
                return provider.handle(request).await;
            }
        }

        // elastos:// sub-dispatch: try sub-provider before main lookup
        if scheme == "elastos" {
            if let Some((sub_name, sub_path)) = Self::split_sub_path(&path) {
                if let Some(provider) = self.get_sub_provider(sub_name).await {
                    let request = ResourceRequest {
                        uri: uri.to_string(),
                        _scheme: sub_name.to_string(),
                        path: sub_path.to_string(),
                        _capsule_id: capsule_id.to_string(),
                        action,
                        content,
                        recursive,
                    };
                    return provider.handle(request).await;
                }
            }
            // Fall through: not a sub-provider → normal "elastos" scheme lookup
        }

        let provider = self
            .get(&scheme)
            .await
            .ok_or_else(|| ProviderError::NoProvider(scheme.clone()))?;

        let request = ResourceRequest {
            uri: uri.to_string(),
            _scheme: scheme,
            path,
            _capsule_id: capsule_id.to_string(),
            action,
            content,
            recursive,
        };

        provider.handle(request).await
    }

    /// Parse a URI into scheme and path
    fn parse_uri(uri: &str) -> Result<(String, String), ProviderError> {
        // Handle URIs like "localhost://Users/self/Documents/path" or "elastos://cid"
        if let Some(_rest) = uri.strip_prefix("://") {
            return Err(ProviderError::InvalidUri(
                "URI cannot start with ://".into(),
            ));
        }

        if let Some(pos) = uri.find("://") {
            let scheme = uri[..pos].to_string();
            let path = uri[pos + 3..].to_string();
            Ok((scheme, path))
        } else {
            Err(ProviderError::InvalidUri(format!(
                "URI must contain ://: {}",
                uri
            )))
        }
    }

    /// List all registered schemes
    pub async fn schemes(&self) -> Vec<String> {
        let providers = self.providers.read().await;
        providers.keys().cloned().collect()
    }

    /// Check if a scheme has a registered provider
    pub async fn has_provider(&self, scheme: &str) -> bool {
        let providers = self.providers.read().await;
        providers.contains_key(scheme)
    }

    /// Get total storage usage for a user (bytes).
    ///
    /// Queries the `localhost` provider to stat the user's rooted local state.
    /// Returns 0 if the provider is not registered or if the path doesn't exist.
    pub async fn storage_usage(&self, user_id: &str) -> Result<u64, ProviderError> {
        let uri = format!("localhost://Users/{}", user_id);
        match self.route(&uri, user_id, ResourceAction::Stat, None).await {
            Ok(ResourceResponse::Metadata { size, .. }) => Ok(size),
            Ok(_) => Ok(0),
            Err(ProviderError::NotFound(_)) => Ok(0),
            Err(e) => Err(e),
        }
    }

    /// Send raw JSON to a provider by scheme (for generic provider proxy).
    /// Checks main providers first, then sub-providers.
    /// Returns the raw JSON response from the provider capsule.
    pub async fn send_raw(
        &self,
        scheme: &str,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        if scheme == "localhost"
            && request
                .get("path")
                .and_then(|value| value.as_str())
                .is_some_and(Self::is_webspaces_localhost_path)
        {
            let providers = self.providers.read().await;
            if let Some(provider) = providers.get("webspace").cloned() {
                drop(providers);
                return provider.send_raw(request).await;
            }
        }

        // Try main providers first (clone Arc to avoid holding lock across await)
        {
            let providers = self.providers.read().await;
            if let Some(provider) = providers.get(scheme).cloned() {
                drop(providers);
                return provider.send_raw(request).await;
            }
        }
        // Try sub-providers (case-insensitive)
        {
            let key = scheme.to_lowercase();
            let sub = self.sub_providers.read().await;
            if let Some(provider) = sub.get(&key).cloned() {
                drop(sub);
                return provider.send_raw(request).await;
            }
        }
        Err(ProviderError::NoProvider(scheme.to_string()))
    }

    /// Invoke one provider from another through an explicit Runtime contract.
    ///
    /// This is the non-app-visible provider plane. It keeps provider-to-provider
    /// effects out of capsule UI code while giving future Carrier/streaming
    /// transports one contract to replace instead of many ad hoc `send_raw`
    /// call sites.
    pub async fn invoke_provider(
        &self,
        invocation: ProviderInvocation,
    ) -> Result<serde_json::Value, ProviderError> {
        if invocation.source.trim().is_empty() {
            return Err(ProviderError::Provider(
                "provider invocation requires source".to_string(),
            ));
        }
        if invocation.target.trim().is_empty() {
            return Err(ProviderError::Provider(
                "provider invocation requires target".to_string(),
            ));
        }
        if invocation.op.trim().is_empty() {
            return Err(ProviderError::Provider(
                "provider invocation requires op".to_string(),
            ));
        }
        validate_provider_transfer_contract(&invocation)?;
        let target_op = invocation
            .request
            .get("op")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        if target_op != invocation.op {
            return Err(ProviderError::Provider(format!(
                "provider invocation op mismatch: envelope={}, request={}",
                invocation.op, target_op
            )));
        }
        let mut request = invocation.request.clone();
        attach_provider_invocation_envelope(&mut request, &invocation)?;
        let mut response = match invocation.transport.carrier_route() {
            Some(route) => {
                let invoker = self.carrier_invoker.read().await.clone().ok_or_else(|| {
                    ProviderError::Provider(format!(
                        "Carrier provider invocation requires registered Carrier invoker: {}",
                        provider_transfer_receipt(&invocation, "failed_closed")
                    ))
                })?;
                invoker
                    .invoke_carrier_provider(route, &invocation, request)
                    .await?
            }
            None => self.send_raw(&invocation.target, &request).await?,
        };
        apply_provider_transfer_response(&mut response, &invocation)?;
        attach_provider_transfer_receipt(&mut response, &invocation, "completed");
        Ok(response)
    }

    /// Open a Runtime-native stream session for a provider `Stream` transfer.
    ///
    /// The target provider still speaks the normal provider envelope; Runtime
    /// turns the validated stream payload into a typed read/cancel channel so
    /// consumers can apply backpressure and observe progress without app-visible
    /// provider authority.
    pub async fn open_provider_stream(
        &self,
        mut invocation: ProviderInvocation,
        options: ProviderStreamOptions,
    ) -> Result<ProviderStreamSession, ProviderError> {
        invocation.transfer = ProviderTransfer::Stream;
        let progress = invocation.progress.clone();
        let response = self.invoke_provider(invocation.clone()).await?;
        if response.get("status").and_then(|status| status.as_str()) == Some("error") {
            let message = response
                .get("message")
                .and_then(|message| message.as_str())
                .unwrap_or("unknown provider stream error");
            return Err(ProviderError::Provider(format!(
                "provider stream open failed: {message}"
            )));
        }
        let data = response
            .get("data")
            .and_then(|data| data.as_object())
            .ok_or_else(|| {
                ProviderError::Provider(
                    "provider stream open requires response data object".to_string(),
                )
            })?;
        let bytes = provider_stream_response_bytes(data)?;
        let chunk_size = options.chunk_size.clamp(1, PROVIDER_STREAM_CHUNK_BYTES);
        let max_in_flight_chunks = options.max_in_flight_chunks.max(1);
        let id = format!(
            "provider-stream:{}",
            NEXT_PROVIDER_STREAM_ID.fetch_add(1, Ordering::Relaxed)
        );
        let request_id = progress
            .as_ref()
            .map(|progress| progress.request_id.clone())
            .unwrap_or_else(|| id.clone());
        Ok(ProviderStreamSession {
            id,
            source: invocation.source,
            target: invocation.target,
            op: invocation.op,
            request_id,
            bytes,
            cursor: 0,
            read_index: 0,
            chunk_size,
            max_in_flight_chunks,
            cancelled: false,
            transfer_receipt: response.get("_runtime_transfer").cloned(),
        })
    }

    fn is_webspaces_localhost_path(path: &str) -> bool {
        parse_localhost_uri(path)
            .or_else(|| parse_localhost_path(path))
            .map(|(root, _)| root == "WebSpaces")
            .unwrap_or(false)
    }
}

fn validate_provider_transfer_contract(
    invocation: &ProviderInvocation,
) -> Result<(), ProviderError> {
    if let Some(range) = invocation.range {
        if matches!(invocation.transfer, ProviderTransfer::Json) {
            return Err(ProviderError::Provider(
                "provider byte range requires bytes or stream transfer".to_string(),
            ));
        }
        if let Some(end) = range.end {
            if end < range.start {
                return Err(ProviderError::Provider(format!(
                    "provider byte range end {end} is before start {}",
                    range.start
                )));
            }
        }
    }
    if let Some(progress) = invocation.progress.as_ref() {
        if progress.request_id.trim().is_empty() {
            return Err(ProviderError::Provider(
                "provider progress receipt requires request_id".to_string(),
            ));
        }
    }
    if let Some(route) = invocation.transport.carrier_route() {
        if route.connect_ticket.trim().is_empty() {
            return Err(ProviderError::Provider(
                "Carrier provider invocation requires connect_ticket".to_string(),
            ));
        }
        if route.timeout_ms == Some(0) {
            return Err(ProviderError::Provider(
                "Carrier provider invocation timeout_ms must be greater than zero".to_string(),
            ));
        }
    }
    Ok(())
}

fn apply_provider_transfer_response(
    response: &mut serde_json::Value,
    invocation: &ProviderInvocation,
) -> Result<(), ProviderError> {
    match invocation.transfer {
        ProviderTransfer::Json => Ok(()),
        ProviderTransfer::Bytes => apply_provider_byte_range(response, invocation),
        ProviderTransfer::Stream => apply_provider_stream_response(response, invocation),
    }
}

fn attach_provider_transfer_receipt(
    response: &mut serde_json::Value,
    invocation: &ProviderInvocation,
    status: &str,
) {
    let Some(object) = response.as_object_mut() else {
        return;
    };
    object.insert(
        "_runtime_transfer".to_string(),
        provider_transfer_receipt(invocation, status),
    );
}

fn attach_provider_invocation_envelope(
    request: &mut serde_json::Value,
    invocation: &ProviderInvocation,
) -> Result<(), ProviderError> {
    let Some(object) = request.as_object_mut() else {
        return Err(ProviderError::Provider(
            "provider invocation request must be a JSON object".to_string(),
        ));
    };
    for reserved in ["_runtime_invocation", "_runtime_transfer"] {
        if object.contains_key(reserved) {
            return Err(ProviderError::Provider(format!(
                "provider invocation request must not predeclare runtime field {reserved}"
            )));
        }
    }
    object.insert(
        "_runtime_invocation".to_string(),
        provider_invocation_envelope(invocation),
    );
    Ok(())
}

fn provider_byte_range_bounds(
    bytes_len: usize,
    range: ProviderByteRange,
) -> Result<(usize, usize), ProviderError> {
    let start = usize::try_from(range.start).map_err(|_| {
        ProviderError::Provider("provider byte range start is too large".to_string())
    })?;
    if start >= bytes_len {
        return Err(ProviderError::Provider(format!(
            "provider byte range start {} exceeds payload length {}",
            range.start, bytes_len
        )));
    }
    let end = range
        .end
        .map(|end| {
            usize::try_from(end).map_err(|_| {
                ProviderError::Provider("provider byte range end is too large".to_string())
            })
        })
        .transpose()?
        .map(|end| end.min(bytes_len.saturating_sub(1)))
        .unwrap_or_else(|| bytes_len.saturating_sub(1));
    Ok((start, end))
}

fn apply_provider_byte_range(
    response: &mut serde_json::Value,
    invocation: &ProviderInvocation,
) -> Result<(), ProviderError> {
    let Some(range) = invocation.range else {
        return Ok(());
    };
    if invocation.transfer != ProviderTransfer::Bytes {
        return Ok(());
    }
    if response.get("status").and_then(|status| status.as_str()) == Some("error") {
        return Ok(());
    }
    let data_value = response
        .get_mut("data")
        .and_then(|data| data.as_object_mut())
        .and_then(|data| data.get_mut("data"))
        .ok_or_else(|| {
            ProviderError::Provider(
                "provider byte range requires response data.data base64 payload".to_string(),
            )
        })?;
    let encoded = data_value.as_str().ok_or_else(|| {
        ProviderError::Provider(
            "provider byte range requires response data.data base64 string".to_string(),
        )
    })?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|err| {
            ProviderError::Provider(format!(
                "provider byte range response has invalid base64 payload: {err}"
            ))
        })?;
    let (start, end) = provider_byte_range_bounds(bytes.len(), range)?;
    let sliced = &bytes[start..=end];
    if let Some(expected) = invocation
        .progress
        .as_ref()
        .and_then(|progress| progress.expected_bytes)
    {
        if expected != sliced.len() as u64 {
            return Err(ProviderError::Provider(format!(
                "provider byte range expected {expected} bytes but produced {}",
                sliced.len()
            )));
        }
    }
    *data_value =
        serde_json::Value::String(base64::engine::general_purpose::STANDARD.encode(sliced));
    Ok(())
}

fn apply_provider_stream_response(
    response: &mut serde_json::Value,
    invocation: &ProviderInvocation,
) -> Result<(), ProviderError> {
    if response.get("status").and_then(|status| status.as_str()) == Some("error") {
        return Ok(());
    }
    let mut bytes = {
        let data = response
            .get("data")
            .and_then(|data| data.as_object())
            .ok_or_else(|| {
                ProviderError::Provider("provider stream requires response data object".to_string())
            })?;
        provider_stream_response_bytes(data)?
    };
    if let Some(range) = invocation.range {
        let (start, end) = provider_byte_range_bounds(bytes.len(), range)?;
        bytes = bytes[start..=end].to_vec();
    }
    if let Some(expected) = invocation
        .progress
        .as_ref()
        .and_then(|progress| progress.expected_bytes)
    {
        if expected != bytes.len() as u64 {
            return Err(ProviderError::Provider(format!(
                "provider stream expected {expected} bytes but produced {}",
                bytes.len()
            )));
        }
    }
    let data = response
        .get_mut("data")
        .and_then(|data| data.as_object_mut())
        .ok_or_else(|| {
            ProviderError::Provider("provider stream requires response data object".to_string())
        })?;
    if data.get("data").and_then(|value| value.as_str()).is_some() {
        data.remove("data");
    }
    data.insert("stream".to_string(), provider_stream_payload(&bytes));
    Ok(())
}

fn provider_stream_response_bytes(
    data: &serde_json::Map<String, serde_json::Value>,
) -> Result<Vec<u8>, ProviderError> {
    if let Some(stream) = data.get("stream") {
        return decode_provider_stream_payload(stream);
    }
    let encoded = data
        .get("data")
        .and_then(|value| value.as_str())
        .ok_or_else(|| {
            ProviderError::Provider(
                "provider stream requires data.stream chunks or data.data base64 payload"
                    .to_string(),
            )
        })?;
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|err| {
            ProviderError::Provider(format!(
                "provider stream response has invalid base64 payload: {err}"
            ))
        })
}

fn decode_provider_stream_payload(stream: &serde_json::Value) -> Result<Vec<u8>, ProviderError> {
    let object = stream.as_object().ok_or_else(|| {
        ProviderError::Provider("provider stream payload must be an object".to_string())
    })?;
    let schema = object
        .get("schema")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if schema != PROVIDER_STREAM_SCHEMA {
        return Err(ProviderError::Provider(format!(
            "provider stream schema mismatch: expected {PROVIDER_STREAM_SCHEMA}, got {schema}"
        )));
    }
    let encoding = object
        .get("encoding")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if encoding != PROVIDER_STREAM_ENCODING {
        return Err(ProviderError::Provider(format!(
            "provider stream encoding mismatch: expected {PROVIDER_STREAM_ENCODING}, got {encoding}"
        )));
    }
    if !object
        .get("completed")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        return Err(ProviderError::Provider(
            "provider stream payload must be completed before response finalization".to_string(),
        ));
    }
    let chunks = object
        .get("chunks")
        .and_then(|value| value.as_array())
        .ok_or_else(|| {
            ProviderError::Provider("provider stream payload requires chunks array".to_string())
        })?;
    let mut bytes = Vec::new();
    for (expected_index, chunk) in chunks.iter().enumerate() {
        let chunk = chunk.as_object().ok_or_else(|| {
            ProviderError::Provider(format!(
                "provider stream chunk {expected_index} must be an object"
            ))
        })?;
        let index = chunk
            .get("index")
            .and_then(|value| value.as_u64())
            .ok_or_else(|| {
                ProviderError::Provider(format!(
                    "provider stream chunk {expected_index} requires index"
                ))
            })?;
        if index != expected_index as u64 {
            return Err(ProviderError::Provider(format!(
                "provider stream chunk index mismatch: expected {expected_index}, got {index}"
            )));
        }
        let offset = chunk
            .get("offset")
            .and_then(|value| value.as_u64())
            .ok_or_else(|| {
                ProviderError::Provider(format!("provider stream chunk {index} requires offset"))
            })?;
        if offset != bytes.len() as u64 {
            return Err(ProviderError::Provider(format!(
                "provider stream chunk {index} offset mismatch: expected {}, got {offset}",
                bytes.len()
            )));
        }
        let encoded = chunk
            .get("data")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                ProviderError::Provider(format!(
                    "provider stream chunk {index} requires base64 data"
                ))
            })?;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|err| {
                ProviderError::Provider(format!(
                    "provider stream chunk {index} has invalid base64 data: {err}"
                ))
            })?;
        if let Some(length) = chunk.get("length").and_then(|value| value.as_u64()) {
            if length != decoded.len() as u64 {
                return Err(ProviderError::Provider(format!(
                    "provider stream chunk {index} length {length} does not match decoded length {}",
                    decoded.len()
                )));
            }
        }
        bytes.extend_from_slice(&decoded);
    }
    if let Some(total_bytes) = object.get("total_bytes").and_then(|value| value.as_u64()) {
        if total_bytes != bytes.len() as u64 {
            return Err(ProviderError::Provider(format!(
                "provider stream total_bytes {total_bytes} does not match decoded length {}",
                bytes.len()
            )));
        }
    }
    Ok(bytes)
}

fn provider_stream_payload(bytes: &[u8]) -> serde_json::Value {
    let mut offset = 0usize;
    let chunks: Vec<serde_json::Value> = bytes
        .chunks(PROVIDER_STREAM_CHUNK_BYTES)
        .enumerate()
        .map(|(index, chunk)| {
            let chunk_offset = offset;
            offset += chunk.len();
            serde_json::json!({
                "index": index,
                "offset": chunk_offset,
                "length": chunk.len(),
                "data": base64::engine::general_purpose::STANDARD.encode(chunk),
            })
        })
        .collect();
    serde_json::json!({
        "schema": PROVIDER_STREAM_SCHEMA,
        "encoding": PROVIDER_STREAM_ENCODING,
        "chunk_size": PROVIDER_STREAM_CHUNK_BYTES,
        "total_bytes": bytes.len(),
        "completed": true,
        "chunks": chunks,
    })
}

fn provider_stream_contract(invocation: &ProviderInvocation) -> Option<serde_json::Value> {
    (invocation.transfer == ProviderTransfer::Stream).then(|| {
        serde_json::json!({
            "schema": PROVIDER_STREAM_SCHEMA,
            "encoding": PROVIDER_STREAM_ENCODING,
            "chunk_size": PROVIDER_STREAM_CHUNK_BYTES,
            "mode": "runtime_stream_session",
            "transport_native": true,
            "progress_mode": "stream_events",
            "flow_control": {
                "backpressure": "read_next",
                "cancel": "supported",
                "max_in_flight_chunks": 1
            },
        })
    })
}

fn provider_transfer_abi(invocation: &ProviderInvocation) -> serde_json::Value {
    serde_json::json!({
        "schema": "elastos.provider.transfer-abi/v1",
        "transfer": invocation.transfer.as_str(),
        "transport": invocation.transport.as_str(),
        "range_supported": !matches!(invocation.transfer, ProviderTransfer::Json),
        "progress_supported": invocation.progress.is_some(),
        "progress_mode": if matches!(invocation.transfer, ProviderTransfer::Stream) {
            "stream_events"
        } else if invocation.progress.is_some() {
            "receipt_metadata"
        } else {
            "none"
        },
        "transport_native_stream": matches!(invocation.transfer, ProviderTransfer::Stream),
        "backpressure": match invocation.transfer {
            ProviderTransfer::Stream => "read_next",
            _ => "not_applicable",
        },
        "cancel_supported": matches!(invocation.transfer, ProviderTransfer::Stream),
    })
}

fn provider_transfer_receipt(invocation: &ProviderInvocation, status: &str) -> serde_json::Value {
    let range = invocation.range.map(|range| {
        serde_json::json!({
            "start": range.start,
            "end": range.end,
        })
    });
    let progress = invocation.progress.as_ref().map(|progress| {
        serde_json::json!({
            "request_id": progress.request_id,
            "expected_bytes": progress.expected_bytes,
        })
    });
    let mut receipt = serde_json::json!({
        "schema": "elastos.provider.transfer/v1",
        "source": invocation.source,
        "target": invocation.target,
        "op": invocation.op,
        "capability": provider_invocation_capability(invocation),
        "transport": invocation.transport.as_str(),
        "carrier": provider_carrier_route_receipt(invocation),
        "transfer": invocation.transfer.as_str(),
        "range": range,
        "progress": progress,
        "abi": provider_transfer_abi(invocation),
        "status": status,
    });
    if let Some(stream) = provider_stream_contract(invocation) {
        if let Some(object) = receipt.as_object_mut() {
            object.insert("stream".to_string(), stream);
        }
    }
    receipt
}

fn provider_invocation_envelope(invocation: &ProviderInvocation) -> serde_json::Value {
    let mut envelope = serde_json::json!({
        "schema": "elastos.provider.invocation/v1",
        "source": invocation.source,
        "target": invocation.target,
        "op": invocation.op,
        "capability": provider_invocation_capability(invocation),
        "transport": invocation.transport.as_str(),
        "carrier": provider_carrier_route_receipt(invocation),
        "transfer": invocation.transfer.as_str(),
        "range": invocation.range.map(|range| serde_json::json!({
            "start": range.start,
            "end": range.end,
        })),
        "progress": invocation.progress.as_ref().map(|progress| serde_json::json!({
            "request_id": progress.request_id,
            "expected_bytes": progress.expected_bytes,
        })),
        "abi": provider_transfer_abi(invocation),
    });
    if let Some(stream) = provider_stream_contract(invocation) {
        if let Some(object) = envelope.as_object_mut() {
            object.insert("stream".to_string(), stream);
        }
    }
    envelope
}

fn provider_carrier_route_receipt(invocation: &ProviderInvocation) -> Option<serde_json::Value> {
    invocation.transport.carrier_route().map(|route| {
        serde_json::json!({
            "route": "connect_ticket",
            "peer_did": route.peer_did.as_deref(),
            "timeout_ms": route.timeout_ms,
        })
    })
}

fn provider_invocation_capability(invocation: &ProviderInvocation) -> String {
    format!(
        "provider:{}->{}:{}",
        invocation.source, invocation.target, invocation.op
    )
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl ProviderRegistry {
    /// Register a sub-provider bypassing the reserved-name guard (test only).
    async fn register_sub_provider_unchecked(&self, name: &str, provider: Arc<dyn Provider>) {
        self.sub_providers
            .write()
            .await
            .insert(name.to_lowercase(), provider);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::Mutex;

    /// In-memory mock provider for testing registry routing
    struct MockProvider {
        data: Mutex<HashMap<String, Vec<u8>>>,
    }

    impl MockProvider {
        fn new() -> Self {
            Self {
                data: Mutex::new(HashMap::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl Provider for MockProvider {
        async fn handle(
            &self,
            request: ResourceRequest,
        ) -> Result<ResourceResponse, ProviderError> {
            let mut data = self.data.lock().await;
            match request.action {
                ResourceAction::Read => data
                    .get(&request.path)
                    .cloned()
                    .map(ResourceResponse::Data)
                    .ok_or(ProviderError::NotFound(request.uri)),
                ResourceAction::Write => {
                    let content = request
                        .content
                        .ok_or_else(|| ProviderError::Provider("no content".into()))?;
                    let bytes = content.len();
                    data.insert(request.path, content);
                    Ok(ResourceResponse::Written { bytes })
                }
                ResourceAction::Delete => {
                    data.remove(&request.path);
                    Ok(ResourceResponse::Deleted)
                }
                _ => Err(ProviderError::Provider("unsupported".into())),
            }
        }

        fn schemes(&self) -> Vec<&'static str> {
            vec!["localhost"]
        }

        fn name(&self) -> &'static str {
            "mock-localhost"
        }
    }

    struct RawMockProvider;

    #[async_trait::async_trait]
    impl Provider for RawMockProvider {
        async fn handle(
            &self,
            _request: ResourceRequest,
        ) -> Result<ResourceResponse, ProviderError> {
            Err(ProviderError::Provider("raw mock only supports raw".into()))
        }

        fn schemes(&self) -> Vec<&'static str> {
            vec!["raw"]
        }

        fn name(&self) -> &'static str {
            "mock-raw"
        }

        async fn send_raw(
            &self,
            request: &serde_json::Value,
        ) -> Result<serde_json::Value, ProviderError> {
            let op = request.get("op").and_then(|value| value.as_str());
            if op == Some("cat") {
                return Ok(serde_json::json!({
                    "status": "ok",
                    "data": {
                        "data": base64::engine::general_purpose::STANDARD.encode(
                            b"0123456789abcdefghijklmnopqrstuvwxyz",
                        )
                    }
                }));
            }
            if op == Some("stream_cat") {
                return Ok(serde_json::json!({
                    "status": "ok",
                    "data": {
                        "runtime_invocation": request
                            .get("_runtime_invocation")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                        "stream": {
                            "schema": PROVIDER_STREAM_SCHEMA,
                            "encoding": PROVIDER_STREAM_ENCODING,
                            "total_bytes": 36,
                            "completed": true,
                            "chunks": [
                                {
                                    "index": 0,
                                    "offset": 0,
                                    "length": 10,
                                    "data": base64::engine::general_purpose::STANDARD.encode(
                                        b"0123456789",
                                    ),
                                },
                                {
                                    "index": 1,
                                    "offset": 10,
                                    "length": 26,
                                    "data": base64::engine::general_purpose::STANDARD.encode(
                                        b"abcdefghijklmnopqrstuvwxyz",
                                    ),
                                },
                            ],
                        },
                    }
                }));
            }
            if op == Some("bad_stream") {
                return Ok(serde_json::json!({
                    "status": "ok",
                    "data": {
                        "stream": {
                            "schema": PROVIDER_STREAM_SCHEMA,
                            "encoding": PROVIDER_STREAM_ENCODING,
                            "total_bytes": 4,
                            "completed": true,
                            "chunks": [
                                {
                                    "index": 0,
                                    "offset": 0,
                                    "length": 99,
                                    "data": base64::engine::general_purpose::STANDARD.encode(
                                        b"oops",
                                    ),
                                },
                            ],
                        },
                    }
                }));
            }
            Ok(serde_json::json!({
                "status": "ok",
                "data": {
                    "op": request.get("op").cloned().unwrap_or(serde_json::Value::Null),
                    "runtime_invocation": request
                        .get("_runtime_invocation")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                }
            }))
        }
    }

    fn decode_test_stream_response(response: &serde_json::Value) -> Vec<u8> {
        decode_provider_stream_payload(&response["data"]["stream"]).unwrap()
    }

    #[derive(Default)]
    struct MockCarrierInvoker {
        requests: Mutex<Vec<serde_json::Value>>,
    }

    #[async_trait::async_trait]
    impl ProviderCarrierInvoker for MockCarrierInvoker {
        async fn invoke_carrier_provider(
            &self,
            route: &ProviderCarrierRoute,
            invocation: &ProviderInvocation,
            request: serde_json::Value,
        ) -> Result<serde_json::Value, ProviderError> {
            self.requests.lock().await.push(serde_json::json!({
                "connect_ticket": route.connect_ticket.as_str(),
                "peer_did": route.peer_did.as_deref(),
                "timeout_ms": route.timeout_ms,
                "source": invocation.source.as_str(),
                "target": invocation.target.as_str(),
                "op": invocation.op.as_str(),
                "request": request.clone(),
            }));
            Ok(serde_json::json!({
                "status": "ok",
                "data": {
                    "data": base64::engine::general_purpose::STANDARD.encode(b"0123456789"),
                    "runtime_invocation": request
                        .get("_runtime_invocation")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                }
            }))
        }
    }

    #[test]
    fn test_parse_uri() {
        let (scheme, path) =
            ProviderRegistry::parse_uri("localhost://Users/self/Documents/photos/vacation.jpg")
                .unwrap();
        assert_eq!(scheme, "localhost");
        assert_eq!(path, "Users/self/Documents/photos/vacation.jpg");

        let (scheme, path) = ProviderRegistry::parse_uri("elastos://Qm123/file.txt").unwrap();
        assert_eq!(scheme, "elastos");
        assert_eq!(path, "Qm123/file.txt");
    }

    #[test]
    fn test_parse_uri_invalid() {
        assert!(ProviderRegistry::parse_uri("no-scheme").is_err());
        assert!(ProviderRegistry::parse_uri("://no-scheme").is_err());
    }

    #[tokio::test]
    async fn test_registry_register() {
        let registry = ProviderRegistry::new();
        let provider = Arc::new(MockProvider::new());

        registry.register(provider).await;

        assert!(registry.has_provider("localhost").await);
        assert!(!registry.has_provider("unknown").await);
    }

    #[tokio::test]
    async fn test_registry_route() {
        let registry = ProviderRegistry::new();
        let provider = Arc::new(MockProvider::new());

        registry.register(provider).await;

        // Write via registry
        let response = registry
            .route(
                "localhost://Public/routed.txt",
                "test-capsule",
                ResourceAction::Write,
                Some(b"routed content".to_vec()),
            )
            .await
            .unwrap();

        assert!(matches!(response, ResourceResponse::Written { .. }));

        // Read via registry
        let response = registry
            .route(
                "localhost://Public/routed.txt",
                "test-capsule",
                ResourceAction::Read,
                None,
            )
            .await
            .unwrap();

        match response {
            ResourceResponse::Data(data) => assert_eq!(data, b"routed content"),
            _ => panic!("Expected Data response"),
        }
    }

    #[tokio::test]
    async fn test_registry_no_provider() {
        let registry = ProviderRegistry::new();

        let result = registry
            .route(
                "localhost://Public/resource",
                "test-capsule",
                ResourceAction::Read,
                None,
            )
            .await;

        assert!(matches!(result, Err(ProviderError::NoProvider(_))));
    }

    #[tokio::test]
    async fn test_provider_invocation_routes_raw_request() {
        let registry = ProviderRegistry::new();
        registry.register(Arc::new(RawMockProvider)).await;

        let response = registry
            .invoke_provider(ProviderInvocation {
                source: "content-provider".to_string(),
                target: "raw".to_string(),
                op: "ping".to_string(),
                request: serde_json::json!({ "op": "ping" }),
                transfer: ProviderTransfer::Json,
                range: None,
                progress: None,
                transport: ProviderInvocationTransport::Local,
            })
            .await
            .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["op"], "ping");
        assert_eq!(
            response["data"]["runtime_invocation"]["schema"],
            "elastos.provider.invocation/v1"
        );
        assert_eq!(
            response["data"]["runtime_invocation"]["source"],
            "content-provider"
        );
        assert_eq!(response["data"]["runtime_invocation"]["target"], "raw");
        assert_eq!(
            response["data"]["runtime_invocation"]["capability"],
            "provider:content-provider->raw:ping"
        );
        assert_eq!(
            response["data"]["runtime_invocation"]["transport"],
            "runtime-local-provider-plane"
        );
        assert_eq!(
            response["_runtime_transfer"]["schema"],
            "elastos.provider.transfer/v1"
        );
        assert_eq!(
            response["_runtime_transfer"]["capability"],
            "provider:content-provider->raw:ping"
        );
        assert_eq!(
            response["_runtime_transfer"]["transport"],
            "runtime-local-provider-plane"
        );
        assert_eq!(response["_runtime_transfer"]["transfer"], "json");
        assert_eq!(response["_runtime_transfer"]["status"], "completed");
    }

    #[tokio::test]
    async fn test_provider_invocation_attaches_range_progress_transfer_receipt() {
        let registry = ProviderRegistry::new();
        registry.register(Arc::new(RawMockProvider)).await;

        let response = registry
            .invoke_provider(ProviderInvocation {
                source: "content-provider".to_string(),
                target: "raw".to_string(),
                op: "cat".to_string(),
                request: serde_json::json!({ "op": "cat" }),
                transfer: ProviderTransfer::Bytes,
                range: Some(ProviderByteRange {
                    start: 10,
                    end: Some(19),
                }),
                progress: Some(ProviderProgress {
                    request_id: "transfer:test".to_string(),
                    expected_bytes: Some(10),
                }),
                transport: ProviderInvocationTransport::Local,
            })
            .await
            .unwrap();

        assert_eq!(response["status"], "ok");
        let sliced = base64::engine::general_purpose::STANDARD
            .decode(response["data"]["data"].as_str().unwrap())
            .unwrap();
        assert_eq!(sliced, b"abcdefghij");
        assert_eq!(response["_runtime_transfer"]["transfer"], "bytes");
        assert_eq!(
            response["_runtime_transfer"]["capability"],
            "provider:content-provider->raw:cat"
        );
        assert_eq!(
            response["_runtime_transfer"]["transport"],
            "runtime-local-provider-plane"
        );
        assert_eq!(response["_runtime_transfer"]["range"]["start"], 10);
        assert_eq!(response["_runtime_transfer"]["range"]["end"], 19);
        assert_eq!(
            response["_runtime_transfer"]["progress"]["request_id"],
            "transfer:test"
        );
        assert_eq!(
            response["_runtime_transfer"]["progress"]["expected_bytes"],
            10
        );
    }

    #[tokio::test]
    async fn test_provider_invocation_rejects_invalid_range_contract() {
        let registry = ProviderRegistry::new();
        registry.register(Arc::new(RawMockProvider)).await;

        let err = registry
            .invoke_provider(ProviderInvocation {
                source: "content-provider".to_string(),
                target: "raw".to_string(),
                op: "cat".to_string(),
                request: serde_json::json!({ "op": "cat" }),
                transfer: ProviderTransfer::Bytes,
                range: Some(ProviderByteRange {
                    start: 20,
                    end: Some(10),
                }),
                progress: None,
                transport: ProviderInvocationTransport::Local,
            })
            .await
            .expect_err("invalid range should fail closed");

        assert!(err.to_string().contains("range end"));
    }

    #[tokio::test]
    async fn test_provider_invocation_stream_normalizes_range_progress_transfer_receipt() {
        let registry = ProviderRegistry::new();
        registry.register(Arc::new(RawMockProvider)).await;

        let response = registry
            .invoke_provider(ProviderInvocation {
                source: "content-provider".to_string(),
                target: "raw".to_string(),
                op: "stream_cat".to_string(),
                request: serde_json::json!({ "op": "stream_cat" }),
                transfer: ProviderTransfer::Stream,
                range: Some(ProviderByteRange {
                    start: 10,
                    end: Some(19),
                }),
                progress: Some(ProviderProgress {
                    request_id: "transfer:stream".to_string(),
                    expected_bytes: Some(10),
                }),
                transport: ProviderInvocationTransport::Local,
            })
            .await
            .unwrap();

        assert_eq!(response["status"], "ok");
        assert!(response["data"].get("data").is_none());
        assert_eq!(decode_test_stream_response(&response), b"abcdefghij");
        assert_eq!(response["data"]["stream"]["schema"], PROVIDER_STREAM_SCHEMA);
        assert_eq!(
            response["data"]["stream"]["encoding"],
            PROVIDER_STREAM_ENCODING
        );
        assert_eq!(response["data"]["stream"]["total_bytes"], 10);
        assert_eq!(
            response["data"]["runtime_invocation"]["stream"]["schema"],
            PROVIDER_STREAM_SCHEMA
        );
        assert_eq!(
            response["data"]["runtime_invocation"]["stream"]["mode"],
            "runtime_stream_session"
        );
        assert_eq!(
            response["data"]["runtime_invocation"]["abi"]["backpressure"],
            "read_next"
        );
        assert_eq!(response["_runtime_transfer"]["transfer"], "stream");
        assert_eq!(
            response["_runtime_transfer"]["stream"]["schema"],
            PROVIDER_STREAM_SCHEMA
        );
        assert_eq!(
            response["_runtime_transfer"]["abi"]["transport_native_stream"],
            true
        );
        assert_eq!(
            response["_runtime_transfer"]["abi"]["progress_mode"],
            "stream_events"
        );
        assert_eq!(
            response["_runtime_transfer"]["abi"]["cancel_supported"],
            true
        );
        assert_eq!(response["_runtime_transfer"]["range"]["start"], 10);
        assert_eq!(response["_runtime_transfer"]["range"]["end"], 19);
        assert_eq!(
            response["_runtime_transfer"]["progress"]["request_id"],
            "transfer:stream"
        );
        assert_eq!(
            response["_runtime_transfer"]["progress"]["expected_bytes"],
            10
        );
    }

    #[tokio::test]
    async fn test_provider_stream_session_reads_partially_and_cancels() {
        let registry = ProviderRegistry::new();
        registry.register(Arc::new(RawMockProvider)).await;

        let mut session = registry
            .open_provider_stream(
                ProviderInvocation {
                    source: "content-provider".to_string(),
                    target: "raw".to_string(),
                    op: "stream_cat".to_string(),
                    request: serde_json::json!({ "op": "stream_cat" }),
                    transfer: ProviderTransfer::Stream,
                    range: None,
                    progress: Some(ProviderProgress {
                        request_id: "transfer:session".to_string(),
                        expected_bytes: Some(36),
                    }),
                    transport: ProviderInvocationTransport::Local,
                },
                ProviderStreamOptions {
                    chunk_size: 8,
                    max_in_flight_chunks: 1,
                },
            )
            .await
            .unwrap();

        assert_eq!(session.total_bytes(), 36);
        assert_eq!(session.receipt()["schema"], PROVIDER_STREAM_SESSION_SCHEMA);
        assert_eq!(session.receipt()["backpressure"], "read_next");
        assert_eq!(session.receipt()["cancel_supported"], true);
        assert_eq!(session.receipt()["progress_mode"], "stream_events");

        let first = session.read_next().unwrap().unwrap();
        assert_eq!(first.session_id, session.id());
        assert_eq!(first.index, 0);
        assert_eq!(first.offset, 0);
        assert_eq!(first.bytes, b"01234567");
        assert!(!first.completed);
        assert_eq!(first.progress["schema"], PROVIDER_STREAM_EVENT_SCHEMA);
        assert_eq!(first.progress["status"], "progress");
        assert_eq!(first.progress["transferred_bytes"], 8);

        let cancel = session.cancel();
        assert_eq!(cancel["status"], "cancelled");
        assert!(session.is_cancelled());
        let err = session
            .read_next()
            .expect_err("cancelled session must not continue reading");
        assert!(err.to_string().contains("cancelled"));
    }

    #[tokio::test]
    async fn test_provider_invocation_rejects_malformed_stream_payload() {
        let registry = ProviderRegistry::new();
        registry.register(Arc::new(RawMockProvider)).await;

        let err = registry
            .invoke_provider(ProviderInvocation {
                source: "content-provider".to_string(),
                target: "raw".to_string(),
                op: "bad_stream".to_string(),
                request: serde_json::json!({ "op": "bad_stream" }),
                transfer: ProviderTransfer::Stream,
                range: None,
                progress: None,
                transport: ProviderInvocationTransport::Local,
            })
            .await
            .expect_err("malformed stream should fail closed");

        assert!(err.to_string().contains("provider stream chunk 0 length"));
    }

    #[tokio::test]
    async fn test_provider_invocation_rejects_op_mismatch() {
        let registry = ProviderRegistry::new();
        let err = registry
            .invoke_provider(ProviderInvocation {
                source: "content-provider".to_string(),
                target: "localhost".to_string(),
                op: "fetch".to_string(),
                request: serde_json::json!({ "op": "publish" }),
                transfer: ProviderTransfer::Json,
                range: None,
                progress: None,
                transport: ProviderInvocationTransport::Local,
            })
            .await
            .expect_err("op mismatch should fail closed");

        assert!(err.to_string().contains("op mismatch"));
    }

    #[tokio::test]
    async fn test_provider_invocation_rejects_predeclared_runtime_metadata() {
        let registry = ProviderRegistry::new();
        registry.register(Arc::new(RawMockProvider)).await;

        for reserved in ["_runtime_invocation", "_runtime_transfer"] {
            let err = registry
                .invoke_provider(ProviderInvocation {
                    source: "content-provider".to_string(),
                    target: "raw".to_string(),
                    op: "ping".to_string(),
                    request: serde_json::json!({
                        "op": "ping",
                        reserved: {
                            "schema": "spoofed"
                        }
                    }),
                    transfer: ProviderTransfer::Json,
                    range: None,
                    progress: None,
                    transport: ProviderInvocationTransport::Local,
                })
                .await
                .expect_err("runtime metadata should be reserved");

            assert!(err
                .to_string()
                .contains("provider invocation request must not predeclare runtime field"));
            assert!(err.to_string().contains(reserved));
        }
    }

    #[tokio::test]
    async fn test_provider_invocation_carrier_requires_registered_invoker() {
        let registry = ProviderRegistry::new();
        let err = registry
            .invoke_provider(ProviderInvocation {
                source: "content-provider".to_string(),
                target: "content".to_string(),
                op: "fetch".to_string(),
                request: serde_json::json!({ "op": "fetch" }),
                transfer: ProviderTransfer::Bytes,
                range: None,
                progress: None,
                transport: ProviderInvocationTransport::Carrier(ProviderCarrierRoute {
                    connect_ticket: "ticket-secret".to_string(),
                    peer_did: Some("did:key:zRemote".to_string()),
                    timeout_ms: Some(5_000),
                }),
            })
            .await
            .expect_err("Carrier invocation without invoker should fail closed");

        let error = err.to_string();
        assert!(error.contains("registered Carrier invoker"));
        assert!(error.contains("carrier-provider-plane"));
        assert!(error.contains("failed_closed"));
        assert!(!error.contains("ticket-secret"));
    }

    #[tokio::test]
    async fn test_provider_invocation_carrier_routes_through_registered_invoker() {
        let registry = ProviderRegistry::new();
        let invoker = Arc::new(MockCarrierInvoker::default());
        registry.set_carrier_invoker(invoker.clone()).await;

        let response = registry
            .invoke_provider(ProviderInvocation {
                source: "content-provider".to_string(),
                target: "content".to_string(),
                op: "fetch".to_string(),
                request: serde_json::json!({ "op": "fetch" }),
                transfer: ProviderTransfer::Bytes,
                range: Some(ProviderByteRange {
                    start: 2,
                    end: Some(5),
                }),
                progress: Some(ProviderProgress {
                    request_id: "carrier-transfer:test".to_string(),
                    expected_bytes: Some(4),
                }),
                transport: ProviderInvocationTransport::Carrier(ProviderCarrierRoute {
                    connect_ticket: "ticket-secret".to_string(),
                    peer_did: Some("did:key:zRemote".to_string()),
                    timeout_ms: Some(5_000),
                }),
            })
            .await
            .unwrap();

        let sliced = base64::engine::general_purpose::STANDARD
            .decode(response["data"]["data"].as_str().unwrap())
            .unwrap();
        assert_eq!(sliced, b"2345");
        assert_eq!(
            response["data"]["runtime_invocation"]["transport"],
            "carrier-provider-plane"
        );
        assert_eq!(
            response["data"]["runtime_invocation"]["carrier"]["route"],
            "connect_ticket"
        );
        assert_eq!(
            response["data"]["runtime_invocation"]["carrier"]["peer_did"],
            "did:key:zRemote"
        );
        assert_eq!(
            response["_runtime_transfer"]["transport"],
            "carrier-provider-plane"
        );
        assert_eq!(
            response["_runtime_transfer"]["carrier"]["route"],
            "connect_ticket"
        );
        assert_eq!(
            response["_runtime_transfer"]["progress"]["request_id"],
            "carrier-transfer:test"
        );
        assert!(!response.to_string().contains("ticket-secret"));

        let requests = invoker.requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["connect_ticket"], "ticket-secret");
        assert_eq!(
            requests[0]["request"]["_runtime_invocation"]["capability"],
            "provider:content-provider->content:fetch"
        );
    }

    // --- elastos:// sub-dispatch tests ---

    #[test]
    fn test_split_sub_path() {
        // Normal case
        let (name, rest) = ProviderRegistry::split_sub_path("peer/alice/shared").unwrap();
        assert_eq!(name, "peer");
        assert_eq!(rest, "alice/shared");

        // Single segment
        let (name, rest) = ProviderRegistry::split_sub_path("peer").unwrap();
        assert_eq!(name, "peer");
        assert_eq!(rest, "");

        // Empty → None
        assert!(ProviderRegistry::split_sub_path("").is_none());
    }

    #[tokio::test]
    async fn test_sub_provider_registration() {
        let registry = ProviderRegistry::new();
        let provider = Arc::new(MockProvider::new());

        // Reserved name succeeds
        registry
            .register_sub_provider("peer", provider.clone())
            .await
            .unwrap();
        assert!(registry.get_sub_provider("peer").await.is_some());

        // Unknown name rejected with error
        let result = registry
            .register_sub_provider("bogus", provider.clone())
            .await;
        assert!(result.is_err());
        assert!(registry.get_sub_provider("bogus").await.is_none());

        // Case-insensitive
        registry
            .register_sub_provider("DID", provider)
            .await
            .unwrap();
        assert!(registry.get_sub_provider("did").await.is_some());

        for name in [
            "chain",
            "wallet",
            "drm",
            "rights",
            "key",
            "decrypt",
            "availability",
            "block-graph",
        ] {
            registry
                .register_sub_provider(name, Arc::new(MockProvider::new()))
                .await
                .unwrap();
            assert!(registry.get_sub_provider(name).await.is_some());
        }

        // Unregister removes the route (case-insensitive)
        registry.unregister_sub_provider("DiD").await;
        assert!(registry.get_sub_provider("did").await.is_none());
    }

    #[tokio::test]
    async fn test_elastos_sub_dispatch_routes() {
        let registry = ProviderRegistry::new();
        let provider = Arc::new(MockProvider::new());
        registry
            .register_sub_provider_unchecked("mock", provider)
            .await;

        // Write via elastos://mock/file.txt
        let response = registry
            .route(
                "elastos://mock/file.txt",
                "test-capsule",
                ResourceAction::Write,
                Some(b"sub-dispatch data".to_vec()),
            )
            .await
            .unwrap();
        assert!(matches!(response, ResourceResponse::Written { bytes: 17 }));

        // Read back via elastos://mock/file.txt
        let response = registry
            .route(
                "elastos://mock/file.txt",
                "test-capsule",
                ResourceAction::Read,
                None,
            )
            .await
            .unwrap();
        match response {
            ResourceResponse::Data(data) => assert_eq!(data, b"sub-dispatch data"),
            _ => panic!("Expected Data response"),
        }
    }

    #[tokio::test]
    async fn test_elastos_unknown_sub_falls_through() {
        let registry = ProviderRegistry::new();

        // No sub-provider "foo" → falls through to main "elastos" lookup → NoProvider
        let result = registry
            .route(
                "elastos://foo/bar",
                "test-capsule",
                ResourceAction::Read,
                None,
            )
            .await;
        match result {
            Err(ProviderError::NoProvider(scheme)) => assert_eq!(scheme, "elastos"),
            other => panic!("Expected NoProvider(\"elastos\"), got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_elastos_cid_not_intercepted() {
        let registry = ProviderRegistry::new();
        let provider = Arc::new(MockProvider::new());
        registry
            .register_sub_provider_unchecked("mock", provider)
            .await;

        // CID-like first segment should not match any sub-provider
        let result = registry
            .route(
                "elastos://QmHash123/file.txt",
                "test-capsule",
                ResourceAction::Read,
                None,
            )
            .await;
        assert!(matches!(result, Err(ProviderError::NoProvider(_))));

        let result = registry
            .route(
                "elastos://bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
                "test-capsule",
                ResourceAction::Read,
                None,
            )
            .await;
        assert!(matches!(result, Err(ProviderError::NoProvider(_))));
    }

    #[tokio::test]
    async fn test_native_and_sub_dispatch_parity() {
        let registry = ProviderRegistry::new();
        let provider = Arc::new(MockProvider::new());

        // Register under both localhost:// and elastos://mock/
        registry.register(provider.clone()).await;
        registry
            .register_sub_provider_unchecked("mock", provider)
            .await;

        // Write via native localhost:// root
        registry
            .route(
                "localhost://Public/shared-key",
                "test-capsule",
                ResourceAction::Write,
                Some(b"parity-data".to_vec()),
            )
            .await
            .unwrap();

        // Read via elastos://mock/Public/shared-key — same data
        let response = registry
            .route(
                "elastos://mock/Public/shared-key",
                "test-capsule",
                ResourceAction::Read,
                None,
            )
            .await
            .unwrap();
        match response {
            ResourceResponse::Data(data) => assert_eq!(data, b"parity-data"),
            _ => panic!("Expected Data response"),
        }
    }

    #[tokio::test]
    async fn test_send_raw_to_sub_provider() {
        let registry = ProviderRegistry::new();
        let provider = Arc::new(MockProvider::new());
        registry
            .register_sub_provider_unchecked("mock", provider)
            .await;

        // send_raw should find the sub-provider after main lookup fails
        let result = registry
            .send_raw("mock", &serde_json::json!({"test": true}))
            .await;
        // MockProvider returns "does not support raw communication"
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("raw communication"), "got: {}", err);
    }

    #[tokio::test]
    async fn test_sub_dispatch_path_stripping() {
        let registry = ProviderRegistry::new();
        let provider = Arc::new(MockProvider::new());
        registry
            .register_sub_provider_unchecked("mock", provider)
            .await;

        // Write with a deep path
        registry
            .route(
                "elastos://mock/alice/shared/doc.txt",
                "test-capsule",
                ResourceAction::Write,
                Some(b"deep-path".to_vec()),
            )
            .await
            .unwrap();

        // Read back — provider should see path "alice/shared/doc.txt"
        let response = registry
            .route(
                "elastos://mock/alice/shared/doc.txt",
                "test-capsule",
                ResourceAction::Read,
                None,
            )
            .await
            .unwrap();
        match response {
            ResourceResponse::Data(data) => assert_eq!(data, b"deep-path"),
            _ => panic!("Expected Data response"),
        }
    }

    // --- End-to-end: both URI forms hit the same provider, same data ---

    #[tokio::test]
    async fn test_e2e_write_native_read_elastos_and_vice_versa() {
        let registry = ProviderRegistry::new();
        let provider = Arc::new(MockProvider::new());
        registry.register(provider.clone()).await;
        registry
            .register_sub_provider_unchecked("mock", provider)
            .await;

        // Write via localhost://, read via elastos://
        registry
            .route(
                "localhost://Public/doc.txt",
                "capsule-a",
                ResourceAction::Write,
                Some(b"native-write".to_vec()),
            )
            .await
            .unwrap();
        let resp = registry
            .route(
                "elastos://mock/Public/doc.txt",
                "capsule-a",
                ResourceAction::Read,
                None,
            )
            .await
            .unwrap();
        match resp {
            ResourceResponse::Data(d) => assert_eq!(d, b"native-write"),
            _ => panic!("Expected Data"),
        }

        // Write via elastos://, read via localhost://
        registry
            .route(
                "elastos://mock/Public/report.md",
                "capsule-b",
                ResourceAction::Write,
                Some(b"elastos-write".to_vec()),
            )
            .await
            .unwrap();
        let resp = registry
            .route(
                "localhost://Public/report.md",
                "capsule-b",
                ResourceAction::Read,
                None,
            )
            .await
            .unwrap();
        match resp {
            ResourceResponse::Data(d) => assert_eq!(d, b"elastos-write"),
            _ => panic!("Expected Data"),
        }
    }

    #[tokio::test]
    async fn test_e2e_delete_via_either_uri_form() {
        let registry = ProviderRegistry::new();
        let provider = Arc::new(MockProvider::new());
        registry.register(provider.clone()).await;
        registry
            .register_sub_provider_unchecked("mock", provider)
            .await;

        // Write via localhost://, delete via elastos://
        registry
            .route(
                "localhost://Public/temp.txt",
                "c",
                ResourceAction::Write,
                Some(b"data".to_vec()),
            )
            .await
            .unwrap();
        registry
            .route(
                "elastos://mock/Public/temp.txt",
                "c",
                ResourceAction::Delete,
                None,
            )
            .await
            .unwrap();
        let result = registry
            .route(
                "localhost://Public/temp.txt",
                "c",
                ResourceAction::Read,
                None,
            )
            .await;
        assert!(matches!(result, Err(ProviderError::NotFound(_))));
    }
}
