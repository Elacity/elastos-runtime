//! Shared provider capability-resource mapping.
//!
//! HTTP host adapters and the Carrier bridge must derive the same capability
//! resource from the same provider request. Keeping that logic here prevents
//! local and capsule-kernel calls from drifting.

use elastos_common::localhost::rooted_localhost_uri;
use elastos_runtime::capability::Action;
use serde_json::Value;

/// Build the capability resource string for a provider request.
///
/// First-party `elastos://` sub-providers use `elastos://<scheme>/...`.
/// Unknown schemes and operations fail closed instead of creating wildcard
/// authority such as `<scheme>://*`.
pub fn build_capability_resource(
    scheme: &str,
    op: &str,
    request: &Value,
) -> Result<String, String> {
    match scheme {
        "localhost" => localhost_resource(op, request),
        "ai" => {
            ensure_supported_operation("ai", op, &["chat_completions", "list_backends", "ping"])?;
            let backend = request.get("backend").and_then(|value| value.as_str());
            match backend {
                Some(backend) => {
                    validate_segment(backend, "backend name")?;
                    Ok(format!("elastos://ai/{backend}/{op}"))
                }
                None => Ok(format!("elastos://ai/meta/{op}")),
            }
        }
        "availability" => simple_elastos_resource("availability", op, &["ensure", "status"]),
        "block-graph" => simple_elastos_resource(
            "block-graph",
            op,
            &["export_graph", "import_graph", "status"],
        ),
        "chain" => {
            if op == "networks" {
                return Ok("elastos://chain/meta/networks".to_string());
            }
            let network = request
                .get("network")
                .and_then(|value| value.as_str())
                .ok_or_else(|| "chain provider request missing network".to_string())?;
            validate_chain_network(network)?;
            if op == "has_access_by_content_id" {
                return Ok(format!(
                    "elastos://chain/{network}/rights/has_access_by_content_id"
                ));
            }
            if op == "erc1271_is_valid_signature" {
                return Ok(format!("elastos://chain/{network}/proof/erc1271"));
            }
            match op {
                "status"
                | "block_number"
                | "sync_health"
                | "balance"
                | "contract_call"
                | "estimate_gas"
                | "transaction_count"
                | "gas_price"
                | "fee_history"
                | "code"
                | "logs"
                | "transaction"
                | "receipt"
                | "proof"
                | "prepare_transaction"
                | "broadcast_transaction"
                | "node_lifecycle" => Ok(format!("elastos://chain/{network}/{op}")),
                _ => Err(format!("Unsupported chain provider operation: {op}")),
            }
        }
        "drm" => drm_resource(op),
        "net" => net_resource(op),
        "exit" => exit_resource(op),
        "browser-engine" => browser_engine_resource(op),
        "rights" => rights_resource(op),
        "key" => key_resource(op),
        "decrypt" => decrypt_resource(op),
        "wallet" => wallet_resource(op, request),
        "did" => did_resource(op),
        "ipfs" => ipfs_resource(op),
        "llama" => simple_elastos_resource(
            "llama",
            op,
            &["chat_completions", "status", "health", "list_models"],
        ),
        "object" => object_resource(op),
        "operator-drive-adapter" => simple_elastos_resource(
            "operator-drive-adapter",
            op,
            &["status", "metadata_index", "read_bytes", "write_bytes"],
        ),
        "peer" => simple_elastos_resource(
            "peer",
            op,
            &[
                "init",
                "connect",
                "remember_peer",
                "get_ticket",
                "get_node_id",
                "list_peers",
                "list_topics",
                "list_topic_peers",
                "gossip_join",
                "gossip_join_peers",
                "gossip_leave",
                "gossip_send",
                "gossip_recv",
            ],
        ),
        "tunnel" => simple_elastos_resource("tunnel", op, &["start", "stop", "status", "ping"]),
        "content" => content_resource(op),
        "inspect" => inspect_resource(op),
        _ => Err(format!("Unsupported provider scheme: {scheme}")),
    }
}

/// Canonical capability action for provider operations where the Runtime owns
/// the operation-to-action contract directly.
pub fn provider_operation_action(scheme: &str, op: &str) -> Option<Action> {
    match scheme {
        "localhost" | "webspace" => localhost_op_required_action(op),
        "ai" | "llama" | "did" => execute_op_required_action(op),
        "availability" => match op {
            "status" => Some(Action::Read),
            "ensure" => Some(Action::Write),
            _ => None,
        },
        "block-graph" => match op {
            "status" | "export_graph" => Some(Action::Read),
            "import_graph" => Some(Action::Write),
            _ => None,
        },
        "chain" => chain_op_required_action(op),
        "content" => match op {
            "status" | "fetch" => Some(Action::Read),
            "publish" | "ensure" | "repair" | "unpublish" => Some(Action::Write),
            _ => None,
        },
        "drm" | "rights" | "key" | "decrypt" => read_only_provider_action(op),
        "net" => match op {
            "status" | "resolve" => Some(Action::Read),
            "connect" | "stream" | "http" => Some(Action::Write),
            _ => None,
        },
        "exit" => match op {
            "status" | "discover_remote_carrier_exits" | "quote" => Some(Action::Read),
            "open_stream" | "close_stream" | "http_fetch" => Some(Action::Write),
            _ => None,
        },
        "browser-engine" => match op {
            "status" | "page_status" | "diagnostics" => Some(Action::Read),
            "launch" | "attach_stream" | "input" | "webrtc_signal" => Some(Action::Write),
            "close_page" => Some(Action::Delete),
            _ => None,
        },
        "wallet" => wallet_op_required_action(op),
        "ipfs" => ipfs_op_required_action(op),
        "object" => object_op_required_action(op),
        "operator-drive-adapter" => match op {
            "status" | "metadata_index" | "read_bytes" => Some(Action::Read),
            "write_bytes" => Some(Action::Write),
            _ => None,
        },
        "peer" => match op {
            "init" | "connect" | "remember_peer" | "gossip_join" | "gossip_leave"
            | "gossip_send" | "get_ticket" | "get_node_id" | "list_peers" | "list_topics"
            | "list_topic_peers" | "gossip_join_peers" | "gossip_recv" => Some(Action::Message),
            _ => None,
        },
        "tunnel" => {
            Some(Action::Admin).filter(|_| matches!(op, "start" | "stop" | "status" | "ping"))
        }
        "inspect" => inspect_op_required_action(op),
        _ => None,
    }
}

fn localhost_op_required_action(op: &str) -> Option<Action> {
    match op {
        "read" | "list" | "stat" | "exists" | "resolve" | "ping" => Some(Action::Read),
        "write" | "mkdir" => Some(Action::Write),
        "delete" => Some(Action::Delete),
        _ => None,
    }
}

fn execute_op_required_action(op: &str) -> Option<Action> {
    match op {
        "chat_completions"
        | "list_backends"
        | "ping"
        | "status"
        | "health"
        | "list_models"
        | "get_did"
        | "resolve"
        | "sign_chat_message"
        | "verify"
        | "verify_did_recovery"
        | "get_nickname"
        | "set_nickname"
        | "get_persona_did" => Some(Action::Execute),
        _ => None,
    }
}

fn chain_op_required_action(op: &str) -> Option<Action> {
    match op {
        "networks"
        | "status"
        | "block_number"
        | "sync_health"
        | "balance"
        | "transaction"
        | "receipt"
        | "has_access_by_content_id"
        | "proof"
        | "erc1271_is_valid_signature"
        | "contract_call"
        | "estimate_gas"
        | "transaction_count"
        | "gas_price"
        | "fee_history"
        | "code"
        | "logs" => Some(Action::Read),
        "prepare_transaction" => Some(Action::Write),
        "broadcast_transaction" | "node_lifecycle" => Some(Action::Admin),
        _ => None,
    }
}

fn read_only_provider_action(op: &str) -> Option<Action> {
    match op {
        "status"
        | "open"
        | "has_access_by_content_id"
        | "is_subscription_active"
        | "can_stream"
        | "can_download"
        | "release"
        | "open_session"
        | "render" => Some(Action::Read),
        _ => None,
    }
}

fn wallet_op_required_action(op: &str) -> Option<Action> {
    match op {
        "status"
        | "accounts"
        | "default_account"
        | "challenge"
        | "bitcoin_challenge"
        | "verify_proof"
        | "verify_bip322_proof"
        | "verify_contract_proof"
        | "approval_requests" => Some(Action::Read),
        "link_account"
        | "set_default_account"
        | "rename_account"
        | "reject_approval"
        | "approve_approval"
        | "complete_approval" => Some(Action::Write),
        "create_managed_account" | "revoke_account" | "request_signature" | "sign_approved" => {
            Some(Action::Execute)
        }
        "export_managed_secret" => Some(Action::Execute),
        _ => None,
    }
}

fn ipfs_op_required_action(op: &str) -> Option<Action> {
    match op {
        "cat" | "cat_to_path" | "get_bytes" | "ls" | "download_directory" | "health" | "status" => {
            Some(Action::Read)
        }
        "add_bytes" | "add_path" | "add_directory" | "pin" => Some(Action::Write),
        "unpin" => Some(Action::Delete),
        _ => None,
    }
}

fn object_op_required_action(op: &str) -> Option<Action> {
    match op {
        "roots" | "list" | "stat" | "read" | "download" | "status" | "events" => Some(Action::Read),
        "write" | "mkdir" | "rename" | "move" | "copy" | "trash" | "restore" | "publish"
        | "unpublish" | "repair" | "share" => Some(Action::Write),
        "delete_permanently" | "empty_trash" => Some(Action::Delete),
        _ => None,
    }
}

/// Canonical capability action each Capsule Inspector operation requires.
pub fn inspect_op_required_action(op: &str) -> Option<Action> {
    match op {
        "capsules" | "capsule" | "self" | "plan" => Some(Action::Read),
        "revoke" => Some(Action::Write),
        _ => None,
    }
}

fn validate_segment(value: &str, label: &str) -> Result<(), String> {
    if !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Ok(());
    }
    Err(format!("Invalid {label}: {value}"))
}

fn validate_chain_network(network: &str) -> Result<(), String> {
    if !network.is_empty()
        && network
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Ok(());
    }
    Err(format!("Invalid chain network: {network}"))
}

fn ensure_supported_operation(provider: &str, op: &str, supported: &[&str]) -> Result<(), String> {
    if supported.contains(&op) {
        return Ok(());
    }
    Err(format!("Unsupported {provider} provider operation: {op}"))
}

fn simple_elastos_resource(provider: &str, op: &str, supported: &[&str]) -> Result<String, String> {
    ensure_supported_operation(provider, op, supported)?;
    Ok(format!("elastos://{provider}/{op}"))
}

fn localhost_resource(op: &str, request: &Value) -> Result<String, String> {
    ensure_supported_operation(
        "localhost",
        op,
        &[
            "read", "write", "list", "delete", "stat", "mkdir", "exists", "resolve", "ping",
        ],
    )?;
    match request
        .get("path")
        .and_then(|value| value.as_str())
        .filter(|path| !path.is_empty())
    {
        Some(path) => rooted_localhost_uri(path)
            .ok_or_else(|| format!("Invalid rooted localhost path: {}", path)),
        None => Err("localhost provider request missing path".to_string()),
    }
}

fn validate_wallet_chain_namespace(value: &str) -> Result<(), String> {
    if !value.is_empty()
        && value != "."
        && value != ".."
        && !value.chars().any(|ch| {
            ch.is_control()
                || ch == '/'
                || ch == '\\'
                || ch == '"'
                || ch == '\''
                || ch.is_whitespace()
        })
    {
        return Ok(());
    }
    Err(format!("Invalid wallet chain namespace: {value}"))
}

fn did_resource(op: &str) -> Result<String, String> {
    simple_elastos_resource(
        "did",
        op,
        &[
            "get_did",
            "resolve",
            "sign_chat_message",
            "verify",
            "verify_did_recovery",
            "get_nickname",
            "set_nickname",
            "get_persona_did",
        ],
    )
}

fn ipfs_resource(op: &str) -> Result<String, String> {
    simple_elastos_resource(
        "ipfs",
        op,
        &[
            "add_bytes",
            "add_path",
            "add_directory",
            "cat",
            "cat_to_path",
            "get_bytes",
            "ls",
            "download_directory",
            "pin",
            "unpin",
            "health",
            "status",
        ],
    )
}

fn object_resource(op: &str) -> Result<String, String> {
    simple_elastos_resource(
        "object",
        op,
        &[
            "roots",
            "list",
            "stat",
            "read",
            "download",
            "write",
            "mkdir",
            "rename",
            "move",
            "copy",
            "trash",
            "restore",
            "delete_permanently",
            "empty_trash",
            "status",
            "events",
            "publish",
            "unpublish",
            "repair",
            "share",
        ],
    )
}

fn content_resource(op: &str) -> Result<String, String> {
    match op {
        "publish" => Ok("elastos://content/publish".to_string()),
        "fetch" => Ok("elastos://content/fetch".to_string()),
        "status" => Ok("elastos://content/status".to_string()),
        "ensure" => Ok("elastos://content/ensure".to_string()),
        "repair" => Ok("elastos://content/repair".to_string()),
        "unpublish" => Ok("elastos://content/unpublish".to_string()),
        _ => Err(format!("Unsupported content provider operation: {op}")),
    }
}

fn inspect_resource(op: &str) -> Result<String, String> {
    match inspect_op_required_action(op) {
        Some(_) => Ok(format!("elastos://inspect/{op}")),
        None => Err(format!("Unsupported inspect provider operation: {op}")),
    }
}

fn exit_resource(op: &str) -> Result<String, String> {
    match op {
        "status" => Ok("elastos://exit/meta/status".to_string()),
        "discover_remote_carrier_exits" => {
            Ok("elastos://exit/discover_remote_carrier_exits".to_string())
        }
        "quote" => Ok("elastos://exit/quote".to_string()),
        "open_stream" => Ok("elastos://exit/open_stream".to_string()),
        "close_stream" => Ok("elastos://exit/close_stream".to_string()),
        "http_fetch" => Ok("elastos://exit/http_fetch".to_string()),
        _ => Err(format!("Unsupported exit provider operation: {op}")),
    }
}

fn browser_engine_resource(op: &str) -> Result<String, String> {
    match op {
        "status" => Ok("elastos://browser-engine/meta/status".to_string()),
        "launch" => Ok("elastos://browser-engine/launch".to_string()),
        "attach_stream" => Ok("elastos://browser-engine/attach_stream".to_string()),
        "close_page" => Ok("elastos://browser-engine/close_page".to_string()),
        "page_status" => Ok("elastos://browser-engine/page/status".to_string()),
        "diagnostics" => Ok("elastos://browser-engine/page/diagnostics".to_string()),
        "input" => Ok("elastos://browser-engine/page/input".to_string()),
        "webrtc_signal" => Ok("elastos://browser-engine/page/webrtc_signal".to_string()),
        _ => Err(format!(
            "Unsupported browser-engine provider operation: {op}"
        )),
    }
}

fn net_resource(op: &str) -> Result<String, String> {
    match op {
        "status" => Ok("elastos://net/meta/status".to_string()),
        "resolve" => Ok("elastos://net/resolve".to_string()),
        "connect" => Ok("elastos://net/connect".to_string()),
        "stream" => Ok("elastos://net/stream".to_string()),
        "http" => Ok("elastos://net/http".to_string()),
        _ => Err(format!("Unsupported net provider operation: {op}")),
    }
}

fn wallet_resource(op: &str, request: &Value) -> Result<String, String> {
    match op {
        "status" => Ok("elastos://wallet/meta/status".to_string()),
        "challenge" => Ok("elastos://wallet/proof/challenge".to_string()),
        "bitcoin_challenge" => Ok("elastos://wallet/proof/bip322/challenge".to_string()),
        "verify_proof" => Ok("elastos://wallet/proof/verify".to_string()),
        "verify_bip322_proof" => Ok("elastos://wallet/proof/bip322/verify".to_string()),
        "verify_contract_proof" => Ok("elastos://wallet/proof/verify_contract".to_string()),
        "link_account" => Ok("elastos://wallet/account/link".to_string()),
        "create_managed_account" => Ok("elastos://wallet/account/create_managed".to_string()),
        "accounts" => Ok("elastos://wallet/account/list".to_string()),
        "revoke_account" => Ok("elastos://wallet/account/revoke".to_string()),
        "rename_account" => Ok("elastos://wallet/account/rename".to_string()),
        "export_managed_secret" => Ok("elastos://wallet/account/export_managed_secret".to_string()),
        "set_default_account" => Ok("elastos://wallet/account/set_default".to_string()),
        "default_account" => Ok("elastos://wallet/account/default".to_string()),
        "approval_requests" => Ok("elastos://wallet/approval/list".to_string()),
        "reject_approval" => Ok("elastos://wallet/approval/reject".to_string()),
        "approve_approval" => Ok("elastos://wallet/approval/approve".to_string()),
        "complete_approval" => Ok("elastos://wallet/approval/complete".to_string()),
        "sign_approved" => Ok("elastos://wallet/approval/sign_approved".to_string()),
        "request_signature" => {
            let intent = request
                .get("intent")
                .and_then(|value| value.as_str())
                .ok_or_else(|| "wallet signature request missing intent".to_string())?;
            let chain_namespace = request
                .get("chain_namespace")
                .and_then(|value| value.as_str())
                .ok_or_else(|| "wallet signature request missing chain_namespace".to_string())?;
            validate_segment(intent, "wallet signing intent")?;
            validate_wallet_chain_namespace(chain_namespace)?;
            Ok(format!("elastos://wallet/{chain_namespace}/sign/{intent}"))
        }
        _ => Err(format!("Unsupported wallet provider operation: {op}")),
    }
}

fn drm_resource(op: &str) -> Result<String, String> {
    match op {
        "status" => Ok("elastos://drm/meta/status".to_string()),
        "open" => Ok("elastos://drm/open".to_string()),
        _ => Err(format!("Unsupported drm provider operation: {op}")),
    }
}

fn rights_resource(op: &str) -> Result<String, String> {
    match op {
        "status" => Ok("elastos://rights/meta/status".to_string()),
        "has_access_by_content_id" => {
            Ok("elastos://rights/access/has_access_by_content_id".to_string())
        }
        "is_subscription_active" => {
            Ok("elastos://rights/subscription/is_subscription_active".to_string())
        }
        "can_stream" => Ok("elastos://rights/content/can_stream".to_string()),
        "can_download" => Ok("elastos://rights/content/can_download".to_string()),
        _ => Err(format!("Unsupported rights provider operation: {op}")),
    }
}

fn key_resource(op: &str) -> Result<String, String> {
    match op {
        "status" => Ok("elastos://key/meta/status".to_string()),
        "release" => Ok("elastos://key/release".to_string()),
        _ => Err(format!("Unsupported key provider operation: {op}")),
    }
}

fn decrypt_resource(op: &str) -> Result<String, String> {
    match op {
        "status" => Ok("elastos://decrypt/meta/status".to_string()),
        "open_session" => Ok("elastos://decrypt/session/open".to_string()),
        "render" => Ok("elastos://decrypt/render".to_string()),
        _ => Err(format!("Unsupported decrypt provider operation: {op}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ai_resource_with_backend() {
        let request = serde_json::json!({"backend": "local", "op": "chat_completions"});
        assert_eq!(
            build_capability_resource("ai", "chat_completions", &request).unwrap(),
            "elastos://ai/local/chat_completions"
        );
    }

    #[test]
    fn ai_resource_without_backend() {
        let request = serde_json::json!({"op": "list_backends"});
        assert_eq!(
            build_capability_resource("ai", "list_backends", &request).unwrap(),
            "elastos://ai/meta/list_backends"
        );
    }

    #[test]
    fn ai_resource_invalid_backend_fails_closed() {
        let request = serde_json::json!({"backend": "bad/name", "op": "chat_completions"});
        let err = build_capability_resource("ai", "chat_completions", &request).unwrap_err();
        assert!(err.contains("Invalid backend name"));
    }

    #[test]
    fn localhost_resource_accepts_full_uri_and_bare_rooted_path() {
        let full = serde_json::json!({"path": "localhost://MyWebSite/Documents/demo.md"});
        assert_eq!(
            build_capability_resource("localhost", "read", &full).unwrap(),
            "localhost://MyWebSite/Documents/demo.md"
        );

        let bare = serde_json::json!({"path": "MyWebSite/Documents/demo.md"});
        assert_eq!(
            build_capability_resource("localhost", "read", &bare).unwrap(),
            "localhost://MyWebSite/Documents/demo.md"
        );
        assert_eq!(
            build_capability_resource("localhost", "delete", &full).unwrap(),
            "localhost://MyWebSite/Documents/demo.md"
        );
    }

    #[test]
    fn localhost_resource_requires_rooted_path() {
        assert!(build_capability_resource("localhost", "read", &serde_json::json!({})).is_err());
        assert!(build_capability_resource(
            "localhost",
            "read",
            &serde_json::json!({"path": "../host"})
        )
        .is_err());
        assert!(build_capability_resource(
            "localhost",
            "raw_storage",
            &serde_json::json!({"path": "localhost://TestRoot/Documents/demo.md"})
        )
        .is_err());
    }

    #[test]
    fn first_party_sub_provider_resource() {
        let request = serde_json::json!({});
        assert_eq!(
            build_capability_resource("did", "get_did", &request).unwrap(),
            "elastos://did/get_did"
        );
        assert_eq!(
            build_capability_resource("peer", "connect", &request).unwrap(),
            "elastos://peer/connect"
        );
        assert_eq!(
            build_capability_resource("peer", "gossip_join", &request).unwrap(),
            "elastos://peer/gossip_join"
        );
        assert_eq!(
            build_capability_resource("peer", "gossip_recv", &request).unwrap(),
            "elastos://peer/gossip_recv"
        );
        assert!(build_capability_resource("did", "raw_secret", &request).is_err());
        assert!(build_capability_resource("peer", "raw_socket", &request).is_err());
    }

    #[test]
    fn first_party_manifest_provider_operations_are_explicit() {
        let request = serde_json::json!({});
        for (scheme, op, resource) in [
            ("availability", "ensure", "elastos://availability/ensure"),
            (
                "block-graph",
                "export_graph",
                "elastos://block-graph/export_graph",
            ),
            ("ipfs", "cat", "elastos://ipfs/cat"),
            (
                "llama",
                "chat_completions",
                "elastos://llama/chat_completions",
            ),
            ("object", "read", "elastos://object/read"),
            (
                "operator-drive-adapter",
                "metadata_index",
                "elastos://operator-drive-adapter/metadata_index",
            ),
            ("tunnel", "start", "elastos://tunnel/start"),
            (
                "exit",
                "discover_remote_carrier_exits",
                "elastos://exit/discover_remote_carrier_exits",
            ),
            (
                "wallet",
                "rename_account",
                "elastos://wallet/account/rename",
            ),
            (
                "wallet",
                "export_managed_secret",
                "elastos://wallet/account/export_managed_secret",
            ),
        ] {
            assert_eq!(
                build_capability_resource(scheme, op, &request).unwrap(),
                resource
            );
        }

        for (scheme, op) in [
            ("availability", "raw_replication_socket"),
            ("block-graph", "raw_car"),
            ("ipfs", "raw_kubo_rpc"),
            ("llama", "raw_model_socket"),
            ("object", "raw_storage_path"),
            ("operator-drive-adapter", "resolver_credentials"),
            ("tunnel", "raw_cloudflared_admin"),
        ] {
            assert!(
                build_capability_resource(scheme, op, &request).is_err(),
                "{scheme}/{op} must fail closed"
            );
        }
    }

    #[test]
    fn first_party_provider_authority_operations_have_action_mapping() {
        let manifests = [
            ("ai", "../../../capsules/ai-provider/capsule.json"),
            (
                "availability",
                "../../../capsules/availability-provider/capsule.json",
            ),
            (
                "block-graph",
                "../../../capsules/content-block-graph-provider/capsule.json",
            ),
            ("chain", "../../../capsules/chain-provider/capsule.json"),
            ("decrypt", "../../../capsules/decrypt-provider/capsule.json"),
            ("did", "../../../capsules/did-provider/capsule.json"),
            ("drm", "../../../capsules/drm-provider/capsule.json"),
            ("exit", "../../../capsules/exit-provider/capsule.json"),
            ("ipfs", "../../../capsules/ipfs-provider/capsule.json"),
            ("key", "../../../capsules/key-provider/capsule.json"),
            ("llama", "../../../capsules/llama-provider/capsule.json"),
            ("net", "../../../capsules/net-provider/capsule.json"),
            ("object", "../../../capsules/object-provider/capsule.json"),
            (
                "operator-drive-adapter",
                "../../../capsules/operator-drive-adapter/capsule.json",
            ),
            ("rights", "../../../capsules/rights-provider/capsule.json"),
            ("tunnel", "../../../capsules/tunnel-provider/capsule.json"),
            ("wallet", "../../../capsules/wallet-provider/capsule.json"),
            (
                "browser-engine",
                "../../../capsules/browser-engine-adapter/capsule.json",
            ),
        ];

        for (scheme, manifest_path) in manifests {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(manifest_path);
            let manifest: elastos_common::CapsuleManifest =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            let authority = manifest
                .authority
                .unwrap_or_else(|| panic!("{scheme} provider manifest must declare authority"));
            for capability in authority.capabilities {
                for operation in capability.operations {
                    assert!(
                        provider_operation_action(scheme, &operation).is_some(),
                        "{scheme}/{operation} in {} must have a canonical Runtime action",
                        path.display()
                    );
                }
            }
        }
    }

    #[test]
    fn unknown_provider_scheme_fails_closed() {
        let err =
            build_capability_resource("socket", "connect", &serde_json::json!({})).unwrap_err();
        assert_eq!(err, "Unsupported provider scheme: socket");
    }

    #[test]
    fn content_resource_uses_documented_scopes() {
        let request = serde_json::json!({});
        assert_eq!(
            build_capability_resource("content", "publish", &request).unwrap(),
            "elastos://content/publish"
        );
        assert_eq!(
            build_capability_resource("content", "fetch", &request).unwrap(),
            "elastos://content/fetch"
        );
        assert_eq!(
            build_capability_resource("content", "status", &request).unwrap(),
            "elastos://content/status"
        );
        assert_eq!(
            build_capability_resource("content", "ensure", &request).unwrap(),
            "elastos://content/ensure"
        );
        assert_eq!(
            build_capability_resource("content", "repair", &request).unwrap(),
            "elastos://content/repair"
        );
        assert_eq!(
            build_capability_resource("content", "unpublish", &request).unwrap(),
            "elastos://content/unpublish"
        );
        assert_eq!(
            build_capability_resource("content", "unpin", &request).unwrap_err(),
            "Unsupported content provider operation: unpin"
        );
    }

    #[test]
    fn inspect_resource_and_actions_are_canonical() {
        let request = serde_json::json!({});
        assert_eq!(
            build_capability_resource("inspect", "capsules", &request).unwrap(),
            "elastos://inspect/capsules"
        );
        assert_eq!(
            build_capability_resource("inspect", "plan", &request).unwrap(),
            "elastos://inspect/plan"
        );
        assert_eq!(
            provider_operation_action("inspect", "capsules"),
            Some(elastos_runtime::capability::Action::Read)
        );
        assert_eq!(
            provider_operation_action("inspect", "plan"),
            Some(elastos_runtime::capability::Action::Read)
        );
        assert_eq!(
            provider_operation_action("inspect", "revoke"),
            Some(elastos_runtime::capability::Action::Write)
        );
        assert!(build_capability_resource("inspect", "raw", &request).is_err());
        assert_eq!(
            provider_operation_action("chain", "status"),
            Some(elastos_runtime::capability::Action::Read)
        );
        assert_eq!(
            provider_operation_action("chain", "broadcast_transaction"),
            Some(elastos_runtime::capability::Action::Admin)
        );
        assert_eq!(
            provider_operation_action("localhost", "read"),
            Some(elastos_runtime::capability::Action::Read)
        );
        assert_eq!(
            provider_operation_action("localhost", "write"),
            Some(elastos_runtime::capability::Action::Write)
        );
        assert_eq!(
            provider_operation_action("browser-engine", "close_page"),
            Some(elastos_runtime::capability::Action::Delete)
        );
    }

    #[test]
    fn chain_resource_is_network_scoped() {
        let request = serde_json::json!({"op": "block_number", "network": "esc-mainnet"});
        assert_eq!(
            build_capability_resource("chain", "block_number", &request).unwrap(),
            "elastos://chain/esc-mainnet/block_number"
        );
        assert_eq!(
            build_capability_resource("chain", "sync_health", &request).unwrap(),
            "elastos://chain/esc-mainnet/sync_health"
        );
        assert_eq!(
            build_capability_resource("chain", "networks", &serde_json::json!({})).unwrap(),
            "elastos://chain/meta/networks"
        );
        assert_eq!(
            build_capability_resource(
                "chain",
                "has_access_by_content_id",
                &serde_json::json!({"network": "esc-mainnet"})
            )
            .unwrap(),
            "elastos://chain/esc-mainnet/rights/has_access_by_content_id"
        );
        assert_eq!(
            build_capability_resource(
                "chain",
                "prepare_transaction",
                &serde_json::json!({"network": "esc-mainnet"})
            )
            .unwrap(),
            "elastos://chain/esc-mainnet/prepare_transaction"
        );
        assert_eq!(
            build_capability_resource(
                "chain",
                "contract_call",
                &serde_json::json!({"network": "esc-mainnet"})
            )
            .unwrap(),
            "elastos://chain/esc-mainnet/contract_call"
        );
        assert_eq!(
            build_capability_resource(
                "chain",
                "estimate_gas",
                &serde_json::json!({"network": "esc-mainnet"})
            )
            .unwrap(),
            "elastos://chain/esc-mainnet/estimate_gas"
        );
        assert_eq!(
            build_capability_resource(
                "chain",
                "transaction_count",
                &serde_json::json!({"network": "esc-mainnet"})
            )
            .unwrap(),
            "elastos://chain/esc-mainnet/transaction_count"
        );
        assert_eq!(
            build_capability_resource(
                "chain",
                "fee_history",
                &serde_json::json!({"network": "esc-mainnet"})
            )
            .unwrap(),
            "elastos://chain/esc-mainnet/fee_history"
        );
        assert_eq!(
            build_capability_resource(
                "chain",
                "logs",
                &serde_json::json!({"network": "esc-mainnet"})
            )
            .unwrap(),
            "elastos://chain/esc-mainnet/logs"
        );
        assert_eq!(
            build_capability_resource(
                "chain",
                "erc1271_is_valid_signature",
                &serde_json::json!({"network": "esc-mainnet"})
            )
            .unwrap(),
            "elastos://chain/esc-mainnet/proof/erc1271"
        );
        assert_eq!(
            build_capability_resource(
                "chain",
                "node_lifecycle",
                &serde_json::json!({"network": "btc-mainnet"})
            )
            .unwrap(),
            "elastos://chain/btc-mainnet/node_lifecycle"
        );
        assert!(build_capability_resource(
            "chain",
            "block_number",
            &serde_json::json!({"network": "../esc-mainnet"})
        )
        .is_err());
        assert!(build_capability_resource(
            "chain",
            "call",
            &serde_json::json!({"network": "esc-mainnet"})
        )
        .is_err());
    }

    #[test]
    fn node_lifecycle_resource_rejects_invalid_networks() {
        assert_eq!(
            build_capability_resource(
                "chain",
                "node_lifecycle",
                &serde_json::json!({"network": "btc-mainnet"})
            )
            .unwrap(),
            "elastos://chain/btc-mainnet/node_lifecycle"
        );
        for network in ["", "BTC-mainnet", "btc_mainnet", "../btc-mainnet"] {
            assert!(build_capability_resource(
                "chain",
                "node_lifecycle",
                &serde_json::json!({"network": network})
            )
            .is_err());
        }
    }

    #[test]
    fn drm_resource_uses_documented_scopes() {
        assert_eq!(
            build_capability_resource("drm", "status", &serde_json::json!({})).unwrap(),
            "elastos://drm/meta/status"
        );
        assert_eq!(
            build_capability_resource("drm", "open", &serde_json::json!({})).unwrap(),
            "elastos://drm/open"
        );
        assert!(build_capability_resource("drm", "raw_key", &serde_json::json!({})).is_err());
    }

    #[test]
    fn net_resource_uses_browser_net_scopes() {
        assert_eq!(
            build_capability_resource("net", "status", &serde_json::json!({})).unwrap(),
            "elastos://net/meta/status"
        );
        assert_eq!(
            build_capability_resource("net", "resolve", &serde_json::json!({})).unwrap(),
            "elastos://net/resolve"
        );
        assert_eq!(
            build_capability_resource("net", "connect", &serde_json::json!({})).unwrap(),
            "elastos://net/connect"
        );
        assert_eq!(
            build_capability_resource("net", "stream", &serde_json::json!({})).unwrap(),
            "elastos://net/stream"
        );
        assert_eq!(
            build_capability_resource("net", "http", &serde_json::json!({})).unwrap(),
            "elastos://net/http"
        );
        assert!(build_capability_resource("net", "raw_socket", &serde_json::json!({})).is_err());
    }

    #[test]
    fn exit_resource_uses_internal_exit_scopes() {
        assert_eq!(
            build_capability_resource("exit", "status", &serde_json::json!({})).unwrap(),
            "elastos://exit/meta/status"
        );
        assert_eq!(
            build_capability_resource("exit", "quote", &serde_json::json!({})).unwrap(),
            "elastos://exit/quote"
        );
        assert_eq!(
            build_capability_resource("exit", "open_stream", &serde_json::json!({})).unwrap(),
            "elastos://exit/open_stream"
        );
        assert_eq!(
            build_capability_resource("exit", "close_stream", &serde_json::json!({})).unwrap(),
            "elastos://exit/close_stream"
        );
        assert_eq!(
            build_capability_resource("exit", "http_fetch", &serde_json::json!({})).unwrap(),
            "elastos://exit/http_fetch"
        );
        assert!(build_capability_resource("exit", "raw_socket", &serde_json::json!({})).is_err());
    }

    #[test]
    fn browser_engine_resource_uses_internal_engine_scopes() {
        assert_eq!(
            build_capability_resource("browser-engine", "status", &serde_json::json!({})).unwrap(),
            "elastos://browser-engine/meta/status"
        );
        assert_eq!(
            build_capability_resource("browser-engine", "launch", &serde_json::json!({})).unwrap(),
            "elastos://browser-engine/launch"
        );
        assert_eq!(
            build_capability_resource("browser-engine", "attach_stream", &serde_json::json!({}))
                .unwrap(),
            "elastos://browser-engine/attach_stream"
        );
        assert_eq!(
            build_capability_resource("browser-engine", "close_page", &serde_json::json!({}))
                .unwrap(),
            "elastos://browser-engine/close_page"
        );
        assert_eq!(
            build_capability_resource("browser-engine", "page_status", &serde_json::json!({}))
                .unwrap(),
            "elastos://browser-engine/page/status"
        );
        assert_eq!(
            build_capability_resource("browser-engine", "diagnostics", &serde_json::json!({}))
                .unwrap(),
            "elastos://browser-engine/page/diagnostics"
        );
        assert_eq!(
            build_capability_resource("browser-engine", "input", &serde_json::json!({})).unwrap(),
            "elastos://browser-engine/page/input"
        );
        assert_eq!(
            build_capability_resource("browser-engine", "webrtc_signal", &serde_json::json!({}))
                .unwrap(),
            "elastos://browser-engine/page/webrtc_signal"
        );
        assert!(
            build_capability_resource("browser-engine", "raw_socket", &serde_json::json!({}))
                .is_err()
        );
    }

    #[test]
    fn rights_resource_uses_documented_scopes() {
        assert_eq!(
            build_capability_resource("rights", "status", &serde_json::json!({})).unwrap(),
            "elastos://rights/meta/status"
        );
        assert_eq!(
            build_capability_resource("rights", "has_access_by_content_id", &serde_json::json!({}))
                .unwrap(),
            "elastos://rights/access/has_access_by_content_id"
        );
        assert_eq!(
            build_capability_resource("rights", "can_stream", &serde_json::json!({})).unwrap(),
            "elastos://rights/content/can_stream"
        );
        assert!(build_capability_resource("rights", "raw_key", &serde_json::json!({})).is_err());
    }

    #[test]
    fn key_resource_uses_documented_scopes() {
        assert_eq!(
            build_capability_resource("key", "status", &serde_json::json!({})).unwrap(),
            "elastos://key/meta/status"
        );
        assert_eq!(
            build_capability_resource("key", "release", &serde_json::json!({})).unwrap(),
            "elastos://key/release"
        );
        assert!(build_capability_resource("key", "raw_cek", &serde_json::json!({})).is_err());
    }

    #[test]
    fn decrypt_resource_uses_documented_scopes() {
        assert_eq!(
            build_capability_resource("decrypt", "status", &serde_json::json!({})).unwrap(),
            "elastos://decrypt/meta/status"
        );
        assert_eq!(
            build_capability_resource("decrypt", "open_session", &serde_json::json!({})).unwrap(),
            "elastos://decrypt/session/open"
        );
        assert_eq!(
            build_capability_resource("decrypt", "render", &serde_json::json!({})).unwrap(),
            "elastos://decrypt/render"
        );
        assert!(build_capability_resource("decrypt", "raw_cek", &serde_json::json!({})).is_err());
    }

    #[test]
    fn wallet_resource_uses_documented_scopes() {
        assert_eq!(
            build_capability_resource("wallet", "status", &serde_json::json!({})).unwrap(),
            "elastos://wallet/meta/status"
        );
        assert_eq!(
            build_capability_resource("wallet", "challenge", &serde_json::json!({})).unwrap(),
            "elastos://wallet/proof/challenge"
        );
        assert_eq!(
            build_capability_resource("wallet", "bitcoin_challenge", &serde_json::json!({}))
                .unwrap(),
            "elastos://wallet/proof/bip322/challenge"
        );
        assert_eq!(
            build_capability_resource("wallet", "verify_bip322_proof", &serde_json::json!({}))
                .unwrap(),
            "elastos://wallet/proof/bip322/verify"
        );
        assert_eq!(
            build_capability_resource("wallet", "verify_contract_proof", &serde_json::json!({}))
                .unwrap(),
            "elastos://wallet/proof/verify_contract"
        );
        assert_eq!(
            build_capability_resource("wallet", "link_account", &serde_json::json!({})).unwrap(),
            "elastos://wallet/account/link"
        );
        assert_eq!(
            build_capability_resource("wallet", "create_managed_account", &serde_json::json!({}))
                .unwrap(),
            "elastos://wallet/account/create_managed"
        );
        assert_eq!(
            build_capability_resource("wallet", "approval_requests", &serde_json::json!({}))
                .unwrap(),
            "elastos://wallet/approval/list"
        );
        assert_eq!(
            build_capability_resource("wallet", "set_default_account", &serde_json::json!({}))
                .unwrap(),
            "elastos://wallet/account/set_default"
        );
        assert_eq!(
            build_capability_resource("wallet", "approve_approval", &serde_json::json!({}))
                .unwrap(),
            "elastos://wallet/approval/approve"
        );
        assert_eq!(
            build_capability_resource("wallet", "complete_approval", &serde_json::json!({}))
                .unwrap(),
            "elastos://wallet/approval/complete"
        );
        assert_eq!(
            build_capability_resource("wallet", "sign_approved", &serde_json::json!({})).unwrap(),
            "elastos://wallet/approval/sign_approved"
        );
        assert_eq!(
            build_capability_resource(
                "wallet",
                "request_signature",
                &serde_json::json!({
                    "chain_namespace": "eip155:20",
                    "intent": "publish_envelope"
                })
            )
            .unwrap(),
            "elastos://wallet/eip155:20/sign/publish_envelope"
        );
    }

    #[test]
    fn wallet_resource_fails_closed_for_unknown_or_broad_signing() {
        assert!(
            build_capability_resource("wallet", "request_signature", &serde_json::json!({}))
                .is_err()
        );
        assert!(build_capability_resource(
            "wallet",
            "request_signature",
            &serde_json::json!({"intent": "publish_envelope"})
        )
        .is_err());
        assert!(build_capability_resource(
            "wallet",
            "request_signature",
            &serde_json::json!({"chain_namespace": "eip155:20", "intent": "../raw"})
        )
        .is_err());
        assert!(build_capability_resource(
            "wallet",
            "request_signature",
            &serde_json::json!({"chain_namespace": "../esc", "intent": "publish_envelope"})
        )
        .is_err());
        assert!(build_capability_resource("wallet", "sign", &serde_json::json!({})).is_err());
    }

    #[test]
    fn pc2_wallet_bridge_raw_methods_do_not_become_provider_operations() {
        let pc2_raw_methods = [
            "eth_accounts",
            "eth_requestAccounts",
            "eth_chainId",
            "eth_blockNumber",
            "eth_getBalance",
            "eth_call",
            "eth_estimateGas",
            "eth_sendTransaction",
            "eth_signTransaction",
            "eth_sign",
            "personal_sign",
            "eth_signTypedData",
            "eth_signTypedData_v3",
            "eth_signTypedData_v4",
            "wallet_switchEthereumChain",
            "wallet_addEthereumChain",
        ];
        let request = serde_json::json!({
            "network": "esc-mainnet",
            "intent": "publish_envelope"
        });

        for method in pc2_raw_methods {
            assert!(
                build_capability_resource("wallet", method, &request).is_err(),
                "{method} must not be a raw wallet-provider operation"
            );
            assert!(
                build_capability_resource("chain", method, &request).is_err(),
                "{method} must not be a raw chain-provider operation"
            );
        }
    }

    #[test]
    fn pc2_wallet_bridge_authority_classes_have_typed_runtime_surfaces() {
        assert_eq!(
            build_capability_resource("wallet", "accounts", &serde_json::json!({})).unwrap(),
            "elastos://wallet/account/list"
        );
        assert_eq!(
            build_capability_resource(
                "wallet",
                "request_signature",
                &serde_json::json!({
                    "chain_namespace": "eip155:20",
                    "intent": "transaction_intent"
                })
            )
            .unwrap(),
            "elastos://wallet/eip155:20/sign/transaction_intent"
        );
        assert_eq!(
            build_capability_resource(
                "chain",
                "balance",
                &serde_json::json!({"network": "esc-mainnet"})
            )
            .unwrap(),
            "elastos://chain/esc-mainnet/balance"
        );
        assert_eq!(
            build_capability_resource(
                "chain",
                "prepare_transaction",
                &serde_json::json!({"network": "esc-mainnet"})
            )
            .unwrap(),
            "elastos://chain/esc-mainnet/prepare_transaction"
        );
        assert_eq!(
            build_capability_resource(
                "chain",
                "broadcast_transaction",
                &serde_json::json!({"network": "esc-mainnet"})
            )
            .unwrap(),
            "elastos://chain/esc-mainnet/broadcast_transaction"
        );
    }
}
