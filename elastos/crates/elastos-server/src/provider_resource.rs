//! Shared provider capability-resource mapping.
//!
//! HTTP host adapters and the Carrier bridge must derive the same capability
//! resource from the same provider request. Keeping that logic here prevents
//! local and capsule-kernel calls from drifting.

use elastos_common::localhost::rooted_localhost_uri;
use elastos_runtime::capability::Action;
use serde_json::Value;

/// Map a provider OPERATION to the capability [`Action`] it actually requires (PRE-AUDIT #3).
///
/// The carrier bridge and the HTTP provider proxy used to validate the token against
/// `token.action()` — i.e. the token AGAINST ITSELF — which made the action check a no-op and let a
/// Read-granted token drive a Write/Delete operation on the same resource (the resource string is
/// identical across read/write/delete for `localhost`, `did`, `peer`, `object`, `documents`). This
/// is the single, central operation→action map the bridge consults instead, so the capability
/// manager can enforce the REAL required action.
///
/// Verbs are consistent across providers, so this is keyed on the operation name. FAIL CLOSED: any
/// operation not listed requires [`Action::Admin`] — a newly-added op that someone forgot to map is
/// DENIED to ordinary Read/Write/Execute grants until it is explicitly classified here, rather than
/// silently accepting a low-privilege token (PRINCIPLES #11).
pub fn required_action_for(op: &str) -> Action {
    // Inspect ops carry their own canonical action contract (the inspector
    // substrate's single source of truth, `inspect_op_required_action`). Honor it
    // before the generic verb map so carrier inspect calls gate at Read/Write
    // instead of the fail-closed `Admin` default a merge would otherwise impose
    // (the inspect arm DDRM's map lacked). Reads stay Read; revoke stays Write.
    if let Some(action) = inspect_op_required_action(op) {
        return action;
    }
    match op {
        // ---- Read: observation, never mutation ------------------------------------------------
        "status"
        | "read"
        | "list"
        | "stat"
        | "exists"
        | "resolve"
        | "networks"
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
        | "fetch"
        | "has_access_by_content_id"
        | "is_subscription_active"
        | "can_stream"
        | "can_download"
        | "accounts"
        | "default_account"
        | "approval_requests"
        | "get_did"
        | "get_nickname"
        | "get_persona_did"
        | "get_ticket"
        | "get_node_id"
        | "list_peers"
        | "list_topics"
        | "list_topic_peers"
        | "gossip_recv"
        | "page_status"
        | "frame"
        | "screenshot"
        | "summary"
        | "roots"
        | "ping"
        | "list_backends"
        | "list_channels" => Action::Read,
        // ---- Write: creates/changes state but does not destroy --------------------------------
        "write"
        | "mkdir"
        | "set_nickname"
        | "remember_peer"
        | "link_account"
        | "create_managed_account"
        | "set_default_account"
        | "rename_account"
        | "revoke_account"
        | "publish"
        | "ensure"
        | "repair"
        | "create"
        | "save"
        | "rename"
        | "move"
        | "copy" => Action::Write,
        // ---- Delete: destroys state -----------------------------------------------------------
        "delete" | "delete_permanently" | "trash" | "unpublish" => Action::Delete,
        // ---- Message: peer messaging ----------------------------------------------------------
        "gossip_send" => Action::Message,
        // ---- Execute: invoke an authorized capability (sign/decrypt/connect/tx-assembly) ------
        "gossip_join"
        | "gossip_leave"
        | "gossip_join_peers"
        | "connect"
        | "verify"
        | "verify_did_recovery"
        | "verify_proof"
        | "verify_bip322_proof"
        | "verify_contract_proof"
        | "sign_chat_message"
        | "request_signature"
        | "challenge"
        | "bitcoin_challenge"
        | "release"
        | "release_ref"
        | "release_from_escrow_ref"
        | "open_session"
        | "render"
        | "stream_segment"
        | "prepare_transaction"
        | "assemble_mint"
        | "assemble_create_channel"
        | "assemble_trade_approval"
        | "broadcast_transaction"
        | "decide_access_from_chain"
        | "chat_completions"
        | "stream"
        | "http"
        | "open_stream"
        | "close_stream"
        | "http_fetch"
        | "launch"
        | "attach_stream"
        | "close_page"
        | "input"
        | "webrtc_signal"
        | "open"
        | "quote" => Action::Execute,
        // ---- Admin: lifecycle + secret/approval custody (highest privilege) -------------------
        // `init`/`shutdown`, node lifecycle, secret import/export, approval workflow, and EVERY
        // unmapped operation fall here — fail closed.
        _ => Action::Admin,
    }
}

/// Build the capability resource string for a provider request.
///
/// First-party `elastos://` sub-providers use `elastos://<scheme>/...`.
/// All other schemes use their native `<scheme>://*` format.
pub fn build_capability_resource(
    scheme: &str,
    op: &str,
    request: &Value,
) -> Result<String, String> {
    match scheme {
        "localhost" => match request
            .get("path")
            .and_then(|value| value.as_str())
            .filter(|path| !path.is_empty())
        {
            Some(path) => rooted_localhost_uri(path)
                .ok_or_else(|| format!("Invalid rooted localhost path: {}", path)),
            None => Err("localhost provider request missing path".to_string()),
        },
        "ai" => {
            let backend = request.get("backend").and_then(|value| value.as_str());
            match backend {
                Some(backend) => {
                    validate_segment(backend, "backend name")?;
                    Ok(format!("elastos://ai/{backend}/{op}"))
                }
                None => Ok(format!("elastos://ai/meta/{op}")),
            }
        }
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
        "content" => content_resource(op),
        "did" | "peer" => Ok(format!("elastos://{scheme}/*")),
        // Read-only Capsule Inspector. Use the concrete requested resource so it
        // validates against an `elastos://inspect/*` capability the same way the
        // gateway/handler path does (consistent across both transports).
        "inspect" => Ok(format!("elastos://inspect/{op}")),
        _ => Ok(format!("{scheme}://*")),
    }
}

/// Canonical capability action each Capsule Inspector operation requires.
///
/// This is the **single source of truth** for the inspect op→action contract.
/// Today our carrier gate validates a token against its own action, so this is
/// authoritative documentation backed by a tripwire test. When the DDRM branch
/// lands, its `required_action_for(op)` op→action gate MUST agree with this for
/// every inspect op — otherwise inspect calls fall through to that gate's
/// `Action::Admin` fail-closed default and break silently (git auto-merges the
/// two edits without a conflict). The carrier table test
/// `carrier_inspect_ops_match_canonical_action_contract` enforces the
/// agreement: it fails loudly the moment an inspect op no longer reaches the
/// provider with a token minted at the action declared here.
///
/// Reads are `Read`; the one mutation (`revoke`) is `Write`. `None` means the op
/// is not a recognised inspect operation.
pub fn inspect_op_required_action(op: &str) -> Option<elastos_runtime::capability::token::Action> {
    use elastos_runtime::capability::token::Action;
    match op {
        "capsules" | "capsule" | "self" | "plan" | "intent" => Some(Action::Read),
        "revoke" => Some(Action::Write),
        // NOTE: `discover` (cross-capsule resolution) is DELIBERATELY unmapped. It is a
        // System-operator surface reached only through the System-pinned gateway
        // allow-list, never the carrier capability path. Leaving it unmapped makes
        // `required_action_for` fall through to its `Admin` default, so a routine
        // `inspect/*` Read token cannot drive it over the carrier (fail-closed). Do
        // NOT add it here unless you intend to open carrier reachability.
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

fn exit_resource(op: &str) -> Result<String, String> {
    match op {
        "status" => Ok("elastos://exit/meta/status".to_string()),
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
        "frame" => Ok("elastos://browser-engine/page/frame".to_string()),
        "screenshot" => Ok("elastos://browser-engine/page/screenshot".to_string()),
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
    fn required_action_classifies_operations_and_fails_closed() {
        // Read: observation.
        for op in [
            "status",
            "read",
            "list",
            "stat",
            "exists",
            "fetch",
            "balance",
            "gossip_recv",
        ] {
            assert_eq!(required_action_for(op), Action::Read, "{op} should be Read");
        }
        // Write: mutate-not-destroy.
        for op in ["write", "mkdir", "set_nickname", "publish", "link_account"] {
            assert_eq!(
                required_action_for(op),
                Action::Write,
                "{op} should be Write"
            );
        }
        // Delete: destruction (the read/write/delete collapse is the core PRE-AUDIT #3 risk).
        for op in ["delete", "delete_permanently", "trash", "unpublish"] {
            assert_eq!(
                required_action_for(op),
                Action::Delete,
                "{op} should be Delete"
            );
        }
        // Message.
        assert_eq!(required_action_for("gossip_send"), Action::Message);
        // Execute: invoke an authorized capability.
        for op in [
            "release",
            "open_session",
            "render",
            "request_signature",
            "broadcast_transaction",
        ] {
            assert_eq!(
                required_action_for(op),
                Action::Execute,
                "{op} should be Execute"
            );
        }
        // Admin: lifecycle + secret/approval custody.
        for op in [
            "init",
            "shutdown",
            "node_lifecycle",
            "export_managed_secret",
            "sign_approved",
        ] {
            assert_eq!(
                required_action_for(op),
                Action::Admin,
                "{op} should be Admin"
            );
        }
        // FAIL CLOSED: an unmapped/unknown operation requires Admin, so a Read/Write token cannot
        // drive it — a forgotten mapping denies rather than under-enforces.
        assert_eq!(
            required_action_for("totally_new_unmapped_op"),
            Action::Admin
        );
        assert_eq!(required_action_for(""), Action::Admin);
    }

    #[test]
    fn ai_resource_with_backend() {
        let request = serde_json::json!({"backend": "local", "op": "chat_completions"});
        assert_eq!(
            build_capability_resource("ai", "chat_completions", &request).unwrap(),
            "elastos://ai/local/chat_completions"
        );
    }

    #[test]
    fn inspect_resource_uses_concrete_elastos_uri() {
        // Consistent with the gateway/handler path: a concrete elastos://inspect
        // resource that validates against an `elastos://inspect/*` capability.
        assert_eq!(
            build_capability_resource("inspect", "capsules", &serde_json::json!({})).unwrap(),
            "elastos://inspect/capsules"
        );
        assert_eq!(
            build_capability_resource("inspect", "capsule", &serde_json::json!({})).unwrap(),
            "elastos://inspect/capsule"
        );
    }

    #[test]
    fn inspect_op_action_contract_is_pinned() {
        use elastos_runtime::capability::token::Action;
        // Reads are Read; revoke is the sole mutation (Write). DDRM's
        // required_action_for MUST match this for inspect ops at merge.
        assert_eq!(inspect_op_required_action("capsules"), Some(Action::Read));
        assert_eq!(inspect_op_required_action("capsule"), Some(Action::Read));
        assert_eq!(inspect_op_required_action("self"), Some(Action::Read));
        assert_eq!(inspect_op_required_action("plan"), Some(Action::Read));
        assert_eq!(inspect_op_required_action("intent"), Some(Action::Read));
        assert_eq!(inspect_op_required_action("revoke"), Some(Action::Write));
        // Unknown ops are not silently mapped — fail-closed at the caller.
        assert_eq!(inspect_op_required_action("nope"), None);
        // `discover` is deliberately unmapped: it is a System-gateway-only op, so it
        // must fall through to the Admin default and stay carrier-locked. Pinning this
        // stops a future edit from silently opening carrier reachability of the
        // cross-capsule capability map.
        assert_eq!(inspect_op_required_action("discover"), None);
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
        let request = serde_json::json!({"backend": "bad/name", "op": "chat"});
        let err = build_capability_resource("ai", "chat", &request).unwrap_err();
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
    }

    #[test]
    fn first_party_sub_provider_resource() {
        let request = serde_json::json!({});
        assert_eq!(
            build_capability_resource("did", "get_did", &request).unwrap(),
            "elastos://did/*"
        );
        assert_eq!(
            build_capability_resource("peer", "connect", &request).unwrap(),
            "elastos://peer/*"
        );
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
            build_capability_resource("browser-engine", "frame", &serde_json::json!({})).unwrap(),
            "elastos://browser-engine/page/frame"
        );
        assert_eq!(
            build_capability_resource("browser-engine", "screenshot", &serde_json::json!({}))
                .unwrap(),
            "elastos://browser-engine/page/screenshot"
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
