//! Provider registry implementation
//!
//! The registry maintains a mapping of URL schemes to providers.
//! Supports hierarchical `elastos://` sub-dispatch: `elastos://peer/alice`
//! routes to the `peer` sub-provider with path `alice`.
//!
//! All first-party providers (did, peer, ai) use the `elastos://` namespace
//! exclusively: `elastos://did/*`, `elastos://peer/*`, `elastos://ai/*`.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

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
    "tunnel",
    "storage",
    "namespace",
    "message",
];

/// Registry of providers
pub struct ProviderRegistry {
    /// Map of scheme -> provider
    providers: RwLock<HashMap<String, Arc<dyn Provider>>>,
    /// Sub-providers for elastos:// hierarchical dispatch (e.g., elastos://peer/...)
    sub_providers: RwLock<HashMap<String, Arc<dyn Provider>>>,
}

impl ProviderRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            providers: RwLock::new(HashMap::new()),
            sub_providers: RwLock::new(HashMap::new()),
        }
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

    fn is_webspaces_localhost_path(path: &str) -> bool {
        parse_localhost_uri(path)
            .or_else(|| parse_localhost_path(path))
            .map(|(root, _)| root == "WebSpaces")
            .unwrap_or(false)
    }
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
