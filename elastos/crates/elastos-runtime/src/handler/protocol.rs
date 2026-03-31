//! Wire protocol types for capsule-to-runtime communication
//!
//! These types mirror the guest SDK types but are defined here to avoid
//! a dependency from runtime on guest SDK.
use serde::{Deserialize, Serialize};

/// Request ID for correlating requests and responses
pub type RequestId = u64;

/// Request from capsule to runtime
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeRequest {
    /// List running capsules (shell only)
    ListCapsules,

    /// Launch a capsule (shell only)
    LaunchCapsule {
        cid: String,
        #[serde(default)]
        config: LaunchConfig,
    },

    /// Stop a capsule (shell only)
    StopCapsule { capsule_id: String },

    /// Grant capability to a capsule (shell only)
    GrantCapability {
        capsule_id: String,
        resource: String,
        action: String,
        #[serde(default)]
        constraints: CapabilityConstraints,
    },

    /// Revoke a capability (shell only)
    RevokeCapability { token_id: String },

    /// Send message to another capsule
    SendMessage {
        to: String,
        payload: Vec<u8>,
        #[serde(default)]
        reply_to: Option<String>,
        /// Capability token authorizing Message action to the destination
        #[serde(default)]
        token: Option<String>,
    },

    /// Receive pending messages
    ReceiveMessages,

    /// Fetch content by elastos:// URI
    FetchContent {
        uri: String,
        /// Capability token authorizing read access for non-shell capsules
        #[serde(default)]
        token: Option<String>,
    },

    /// Request storage access (invokes capability)
    StorageRead { token: String, path: String },

    /// Request storage write (invokes capability)
    StorageWrite {
        token: String,
        path: String,
        content: Vec<u8>,
    },

    /// Get runtime info
    GetRuntimeInfo,

    /// Ping (health check)
    Ping,

    /// Window control (forwarded to shell)
    WindowControl {
        window_id: String,
        action: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        width: Option<u32>,
        #[serde(default)]
        height: Option<u32>,
    },

    /// General resource request (provider-routed)
    ResourceRequest {
        uri: String,
        action: String,
        #[serde(default)]
        params: Option<serde_json::Value>,
        /// Capability token authorizing this resource action (required for non-shell capsules)
        #[serde(default)]
        token: Option<String>,
    },
}

/// Response from runtime to capsule
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeResponse {
    /// Success with optional data
    Ok {
        #[serde(default)]
        data: Option<serde_json::Value>,
    },

    /// Error response
    Error { code: String, message: String },

    /// List of capsules
    CapsuleList { capsules: Vec<CapsuleListEntry> },

    /// Capsule launched
    CapsuleLaunched { capsule_id: String },

    /// Capability granted
    CapabilityGranted { token_id: String },

    /// Messages received
    Messages { messages: Vec<IncomingMessage> },

    /// Content fetched
    Content { data: Vec<u8> },

    /// Storage read result
    StorageData { data: Vec<u8> },

    /// Runtime info
    RuntimeInfo {
        version: String,
        capsule_count: usize,
    },

    /// Pong response
    Pong,

    /// Resource response (from ResourceRequest)
    ResourceResponse {
        #[serde(default)]
        data: Option<serde_json::Value>,
        #[serde(default)]
        entries: Option<Vec<serde_json::Value>>,
        #[serde(default)]
        exists: Option<bool>,
        #[serde(default)]
        stat: Option<serde_json::Value>,
    },
}

impl RuntimeResponse {
    /// Create an error response
    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Error {
            code: code.into(),
            message: message.into(),
        }
    }

    /// Create a simple OK response
    pub fn ok() -> Self {
        Self::Ok { data: None }
    }

    /// Create an OK response with data
    pub fn ok_with_data(data: serde_json::Value) -> Self {
        Self::Ok { data: Some(data) }
    }
}

/// Configuration for launching a capsule
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LaunchConfig {
    #[serde(default)]
    pub env: Vec<(String, String)>,
    #[serde(default)]
    pub args: Vec<String>,
}

/// Constraints for capability grants
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapabilityConstraints {
    #[serde(default)]
    pub max_uses: Option<u32>,
    #[serde(default)]
    pub expiry_secs: Option<u64>,
    #[serde(default)]
    pub delegatable: bool,
}

/// Entry in capsule list
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapsuleListEntry {
    pub id: String,
    pub name: String,
    pub status: String,
}

/// Incoming message from another capsule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingMessage {
    pub id: String,
    pub from: String,
    pub payload: Vec<u8>,
    pub timestamp: u64,
    #[serde(default)]
    pub reply_to: Option<String>,
}

/// Message envelope for wire protocol (request)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub id: RequestId,
    pub request: RuntimeRequest,
}

/// Message envelope for wire protocol (response)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub id: RequestId,
    pub response: RuntimeResponse,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_serialization() {
        let req = RuntimeRequest::ListCapsules;
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("list_capsules"));
    }

    #[test]
    fn test_response_serialization() {
        let resp = RuntimeResponse::CapsuleList {
            capsules: vec![CapsuleListEntry {
                id: "cap-1".to_string(),
                name: "test".to_string(),
                status: "running".to_string(),
            }],
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("capsule_list"));
        assert!(json.contains("cap-1"));
    }

    #[test]
    fn test_envelope_roundtrip() {
        let envelope = RequestEnvelope {
            id: 42,
            request: RuntimeRequest::Ping,
        };

        let json = serde_json::to_string(&envelope).unwrap();
        let parsed: RequestEnvelope = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.id, 42);
        assert!(matches!(parsed.request, RuntimeRequest::Ping));
    }

    #[test]
    fn test_error_response() {
        let resp = RuntimeResponse::error("not_found", "Capsule not found");
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("not_found"));
        assert!(json.contains("Capsule not found"));
    }

    /// Compatibility test: every SDK request type string must deserialize into RuntimeRequest.
    ///
    /// If this test fails, it means the SDK and runtime protocol have drifted.
    /// Fix by adding the missing variant to RuntimeRequest.
    ///
    /// SDK types from: packages/elastos-sdk/src/types.ts
    #[test]
    fn test_sdk_request_type_compatibility() {
        // Each entry: (type string, minimal valid JSON for that type)
        let sdk_request_types: Vec<(&str, &str)> = vec![
            ("list_capsules", r#"{"type":"list_capsules"}"#),
            (
                "launch_capsule",
                r#"{"type":"launch_capsule","cid":"Qm123"}"#,
            ),
            (
                "stop_capsule",
                r#"{"type":"stop_capsule","capsule_id":"cap-1"}"#,
            ),
            (
                "grant_capability",
                r#"{"type":"grant_capability","capsule_id":"cap-1","resource":"localhost://Users/self/Documents/x","action":"read"}"#,
            ),
            (
                "revoke_capability",
                r#"{"type":"revoke_capability","token_id":"abc123"}"#,
            ),
            (
                "send_message",
                r#"{"type":"send_message","to":"cap-2","payload":[1,2,3],"token":"abc123"}"#,
            ),
            ("receive_messages", r#"{"type":"receive_messages"}"#),
            (
                "fetch_content",
                r#"{"type":"fetch_content","uri":"elastos://Qm123"}"#,
            ),
            (
                "storage_read",
                r#"{"type":"storage_read","token":"tok","path":"/a"}"#,
            ),
            (
                "storage_write",
                r#"{"type":"storage_write","token":"tok","path":"/a","content":[1]}"#,
            ),
            ("get_runtime_info", r#"{"type":"get_runtime_info"}"#),
            ("ping", r#"{"type":"ping"}"#),
            // Types added to match SDK (previously missing):
            (
                "window_control",
                r#"{"type":"window_control","window_id":"w1","action":"setTitle"}"#,
            ),
            (
                "resource_request",
                r#"{"type":"resource_request","uri":"localhost://Users/self/Documents/file.txt","action":"read","token":"abc123"}"#,
            ),
        ];

        for (type_name, json) in &sdk_request_types {
            let result: Result<RuntimeRequest, _> = serde_json::from_str(json);
            assert!(
                result.is_ok(),
                "SDK request type '{}' failed to deserialize: {} (json: {})",
                type_name,
                result.unwrap_err(),
                json
            );
        }
    }

    /// Verify response types roundtrip correctly
    #[test]
    fn test_sdk_response_type_compatibility() {
        let response_types: Vec<(&str, RuntimeResponse)> = vec![
            ("ok", RuntimeResponse::ok()),
            ("error", RuntimeResponse::error("test", "test error")),
            (
                "capsule_list",
                RuntimeResponse::CapsuleList { capsules: vec![] },
            ),
            (
                "capsule_launched",
                RuntimeResponse::CapsuleLaunched {
                    capsule_id: "cap-1".into(),
                },
            ),
            (
                "capability_granted",
                RuntimeResponse::CapabilityGranted {
                    token_id: "tok-1".into(),
                },
            ),
            ("messages", RuntimeResponse::Messages { messages: vec![] }),
            (
                "content",
                RuntimeResponse::Content {
                    data: vec![1, 2, 3],
                },
            ),
            (
                "storage_data",
                RuntimeResponse::StorageData {
                    data: vec![1, 2, 3],
                },
            ),
            (
                "runtime_info",
                RuntimeResponse::RuntimeInfo {
                    version: "1.0".into(),
                    capsule_count: 0,
                },
            ),
            ("pong", RuntimeResponse::Pong),
            (
                "resource_response",
                RuntimeResponse::ResourceResponse {
                    data: None,
                    entries: None,
                    exists: Some(true),
                    stat: None,
                },
            ),
        ];

        for (expected_type, response) in &response_types {
            let json = serde_json::to_string(response).unwrap();
            assert!(
                json.contains(expected_type),
                "Response type '{}' not found in serialized JSON: {}",
                expected_type,
                json
            );
            // Verify roundtrip
            let parsed: Result<RuntimeResponse, _> = serde_json::from_str(&json);
            assert!(
                parsed.is_ok(),
                "Response type '{}' failed roundtrip: {}",
                expected_type,
                parsed.unwrap_err()
            );
        }
    }
}
