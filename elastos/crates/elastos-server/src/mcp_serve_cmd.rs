//! `elastos mcp serve` — a clean-room Model Context Protocol bridge (core).
//!
//! Exposes the runtime's READ-ONLY reflection surface (inspect `capsules` / `capsule` /
//! `plan` / `intent`) to a local MCP client (Claude Code / Codex / Gemini) the OPERATOR
//! spawns over stdio. Clean-room: we implement the OPEN MCP spec (rev 2025-06-18),
//! hand-rolled over `serde_json` (ZERO new dependencies, no external SDK, no Astrid code).
//!
//! ## Enforcement stays in the core
//! The MCP edge terminates the wire ONLY. It holds a SINGLE explicit, scoped, time-boxed,
//! non-delegatable capability token (`elastos://inspect/*` Read, minted for the
//! [`MCP_BRIDGE_ID`] principal — operator-authorized by running the command) and routes
//! EVERY `tools/call` through the carrier's `handle_request` — the same
//! validate-then-`send_raw` the carrier uses (one canonical gate, a signed `CapabilityUse`
//! per call). The MCP client holds NO token; it is a pure conduit, and its arguments are
//! the provider request BODY only (e.g. a capsule `id`), never an identity/principal.
//!
//! ## Trust model (operator-authority, founder-decided)
//! `inspect/*` Read is FULL cross-capsule System-scope READ — the operator's own read
//! authority, delegated to their local AI tool by running the command. `discover`
//! (Admin-locked) and all effectful ops are excluded; effects are deferred to a follow-up
//! that wires the human approval flow.
//!
//! The stdio I/O loop (`run_mcp_serve`) lives in the binary (`serve_cmd`), since it needs
//! the binary's `setup_server_infrastructure`; it drives [`handle_mcp_message`] here.

use serde_json::{json, Value};

use crate::carrier_bridge::{encode_bridge_capability_token, handle_request, BridgeContext};
use elastos_runtime::capability::token::{Action, ResourceId, TokenConstraints};
use elastos_runtime::capability::CapabilityManager;
use elastos_runtime::primitives::time::SecureTimestamp;

/// The pinned bridge principal id. MUST be byte-identical at the grant (token.capsule),
/// the `BridgeContext.capsule_id`, and `validate`'s `caller_capsule_id`, or `validate`
/// check #4 (WrongCapsule) fails every call.
pub const MCP_BRIDGE_ID: &str = "mcp-bridge";

/// The bridge token's scope: System-scope READ of all installed capsules.
const MCP_BRIDGE_RESOURCE: &str = "elastos://inspect/*";

/// Time-box the operator credential (re-minted each `mcp serve` start; dies with the
/// process). Bounded + epoch-revocable + non-delegatable — never ambient standing authority.
const MCP_BRIDGE_TOKEN_TTL_SECS: u64 = 12 * 60 * 60;

/// The MCP protocol revision we implement (the open spec).
const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

/// The read-only inspect ops exposed as MCP tools (MCP name, inspect op, description).
/// `discover` is deliberately ABSENT (Admin-locked; the Read bridge token cannot reach it
/// anyway); `self` is excluded (incoherent for the bridge principal); effects excluded.
const MCP_TOOLS: &[(&str, &str, &str)] = &[
    (
        "elastos.inspect.capsules",
        "capsules",
        "List all installed capsules (id, name, role, type, state).",
    ),
    (
        "elastos.inspect.capsule",
        "capsule",
        "Inspect one capsule's full record by id (affordances, trust, grants, audit).",
    ),
    (
        "elastos.inspect.plan",
        "plan",
        "Preview the capability gate a capsule affordance call would require (read-only).",
    ),
    (
        "elastos.inspect.intent",
        "intent",
        "Preview the approval a provider operation would require (read-only).",
    ),
];

/// Map an MCP tool name to its inspect op. `None` for any tool not in the allow-list — a
/// tool not here is NEVER dispatched (the edge allow-list, on top of the capability gate).
fn tool_to_op(tool: &str) -> Option<&'static str> {
    MCP_TOOLS
        .iter()
        .find(|(name, _, _)| *name == tool)
        .map(|(_, op, _)| *op)
}

/// Mint the bridge's single scoped, time-boxed, non-delegatable Read token (operator
/// authority). `grant` signs with the runtime key and audit-logs the grant.
pub fn mint_bridge_token(capability_manager: &CapabilityManager) -> String {
    let token = capability_manager.grant(
        MCP_BRIDGE_ID,
        ResourceId::new(MCP_BRIDGE_RESOURCE),
        Action::Read,
        // current epoch (revocable via epoch advance); non-delegatable; no
        // classification ceiling; uses unbounded (reads are many — TIME bounds it).
        TokenConstraints::new(capability_manager.current_epoch(), false, None, None),
        Some(SecureTimestamp::after_secs(MCP_BRIDGE_TOKEN_TTL_SECS)),
    );
    encode_bridge_capability_token(&token)
}

/// The static `tools/list` descriptor (the read tools).
fn tools_list_result() -> Value {
    let tools: Vec<Value> = MCP_TOOLS
        .iter()
        .map(|(name, op, desc)| {
            let input_schema = match *op {
                "capsule" => json!({
                    "type": "object",
                    "properties": {"id": {"type": "string", "description": "capsule id"}},
                    "required": ["id"],
                }),
                "plan" => json!({
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"}, "interface": {"type": "string"},
                        "method": {"type": "string"}, "args": {"type": "object"},
                    },
                    "required": ["id"],
                }),
                "intent" => json!({
                    "type": "object",
                    "properties": {"id": {"type": "string"}, "operation": {"type": "string"}},
                    "required": ["id", "operation"],
                }),
                _ => json!({"type": "object", "properties": {}}),
            };
            json!({"name": name, "description": desc, "inputSchema": input_schema})
        })
        .collect();
    json!({ "tools": tools })
}

fn tool_text_error(msg: impl Into<String>) -> Value {
    json!({"content": [{"type": "text", "text": msg.into()}], "isError": true})
}

/// Translate the carrier's `handle_request` response into an MCP `CallToolResult`.
fn translate_carrier_response(resp: &Value) -> Value {
    let response = &resp["response"];
    match response["type"].as_str() {
        Some("carrier_result") => {
            let result = &response["result"];
            // The inspect op returns {"status":"ok","data":{...}} or {"status":"error",...}.
            let is_error = result["status"].as_str() == Some("error");
            let text = serde_json::to_string_pretty(result).unwrap_or_else(|_| result.to_string());
            json!({"content": [{"type": "text", "text": text}], "isError": is_error})
        }
        // type == "error": capability_denied / missing_token / invalid_token / ... — the
        // gate refused. Surface it as a tool error so the LLM sees the refusal.
        _ => {
            let code = response["code"].as_str().unwrap_or("error");
            let message = response["message"].as_str().unwrap_or("request denied");
            tool_text_error(format!("{code}: {message}"))
        }
    }
}

/// Dispatch one `tools/call` through the CANONICAL carrier gate. The MCP client's
/// arguments become the provider request BODY; the carrier sets `body.op` itself, and the
/// token + identity are server-held (never from the client).
async fn dispatch_tool(
    tool: &str,
    args: &Value,
    ctx: &Option<BridgeContext>,
    token: &str,
) -> Value {
    let Some(op) = tool_to_op(tool) else {
        return tool_text_error(format!("unknown tool: {tool}"));
    };
    let line = json!({
        "id": 1,
        "request": {
            "type": "carrier_invoke",
            "uri": format!("elastos://inspect/{op}"),
            "operation": op,
            "token": token,
            "body": args,
        }
    })
    .to_string();

    match handle_request(&line, ctx).await {
        Ok(resp) => translate_carrier_response(&resp),
        Err(e) => tool_text_error(format!("dispatch error: {e}")),
    }
}

/// Route one parsed JSON-RPC message. `Some(response)` for a request (carries an `id`),
/// `None` for a notification (no `id`, e.g. `notifications/initialized`).
pub async fn handle_mcp_message(
    msg: &Value,
    ctx: &Option<BridgeContext>,
    token: &str,
) -> Option<Value> {
    let id = msg.get("id").cloned()?; // notifications (no id) get no response
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let result: std::result::Result<Value, Value> = match method {
        "initialize" => Ok(json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "elastos", "version": env!("CARGO_PKG_VERSION")},
        })),
        "tools/list" => Ok(tools_list_result()),
        "tools/call" => {
            let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
            let tool = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            Ok(dispatch_tool(tool, &args, ctx, token).await)
        }
        "ping" => Ok(json!({})),
        other => Err(json!({"code": -32601, "message": format!("method not found: {other}")})),
    };
    Some(match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inspect_provider::{CatalogInspectSource, InspectProvider, InspectSource};
    use elastos_runtime::primitives::audit::AuditLog;
    use elastos_runtime::provider::ProviderRegistry;
    use std::sync::Arc;

    // Build a minimal gated MCP context: a tmp installed capsule, a registry with the
    // inspect provider, a capability_manager, and the bridge ctx + token.
    async fn test_ctx() -> (Option<BridgeContext>, String, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let capsule_dir = tmp.path().join("capsules").join("probe");
        std::fs::create_dir_all(&capsule_dir).unwrap();
        std::fs::write(
            capsule_dir.join("capsule.json"),
            serde_json::to_vec(&json!({
                "schema": "elastos.capsule/v1", "version": "0.1.0", "name": "probe",
                "role": "app", "type": "wasm", "entrypoint": "probe.wasm"
            }))
            .unwrap(),
        )
        .unwrap();

        let audit_log = Arc::new(AuditLog::new());
        let store = Arc::new(elastos_runtime::capability::CapabilityStore::new());
        let metrics = Arc::new(elastos_runtime::primitives::metrics::MetricsManager::new());
        let capability_manager =
            Arc::new(CapabilityManager::new(store, audit_log.clone(), metrics));
        let registry = Arc::new(ProviderRegistry::new());
        let source: Arc<dyn InspectSource> = Arc::new(CatalogInspectSource::new(
            tmp.path().join("capsules"),
            Arc::downgrade(&registry),
        ));
        registry
            .register(Arc::new(InspectProvider::new(source)))
            .await;

        let token = mint_bridge_token(&capability_manager);
        let ctx = Some(BridgeContext {
            provider_registry: registry,
            capability_manager: capability_manager.clone(),
            pending_store: Arc::new(
                elastos_runtime::capability::pending::PendingRequestStore::new(audit_log),
            ),
            capsule_id: MCP_BRIDGE_ID.to_string(),
            principal_id: None,
            data_dir: None,
        });
        (ctx, token, tmp)
    }

    #[tokio::test]
    async fn initialize_handshake_pins_protocol_version() {
        let (ctx, token, _tmp) = test_ctx().await;
        let msg = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});
        let resp = handle_mcp_message(&msg, &ctx, &token).await.unwrap();
        assert_eq!(resp["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(resp["result"]["serverInfo"]["name"], "elastos");
    }

    #[tokio::test]
    async fn notifications_get_no_response() {
        let (ctx, token, _tmp) = test_ctx().await;
        let msg = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        assert!(handle_mcp_message(&msg, &ctx, &token).await.is_none());
    }

    #[tokio::test]
    async fn tools_list_is_read_only_and_excludes_discover_and_self() {
        let (ctx, token, _tmp) = test_ctx().await;
        let msg = json!({"jsonrpc":"2.0","id":2,"method":"tools/list"});
        let resp = handle_mcp_message(&msg, &ctx, &token).await.unwrap();
        let tools = resp["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names.len(), 4);
        assert!(names.contains(&"elastos.inspect.capsules"));
        assert!(!names.iter().any(|n| n.contains("discover")));
        assert!(!names.iter().any(|n| n.contains("self")));
        assert!(!names.iter().any(|n| n.contains("revoke")));
    }

    #[tokio::test]
    async fn tools_call_capsules_passes_the_gate_and_returns_data() {
        // The load-bearing happy path: a valid bridge token clears the carrier gate and
        // the read reaches the provider (the installed "probe" capsule surfaces).
        let (ctx, token, _tmp) = test_ctx().await;
        let msg = json!({"jsonrpc":"2.0","id":3,"method":"tools/call",
            "params":{"name":"elastos.inspect.capsules","arguments":{}}});
        let resp = handle_mcp_message(&msg, &ctx, &token).await.unwrap();
        let result = &resp["result"];
        assert_eq!(result["isError"], false);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("probe"),
            "the gated read must reach the provider and list the capsule: {text}"
        );
    }

    #[tokio::test]
    async fn tools_call_with_no_token_fails_closed_at_the_gate() {
        // Prove the gate RUNS (not bypassed): the SAME op with an empty token is denied
        // by the carrier (missing_token), surfaced as a tool error.
        let (ctx, _token, _tmp) = test_ctx().await;
        let msg = json!({"jsonrpc":"2.0","id":4,"method":"tools/call",
            "params":{"name":"elastos.inspect.capsules","arguments":{}}});
        let resp = handle_mcp_message(&msg, &ctx, "").await.unwrap();
        assert_eq!(resp["result"]["isError"], true);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("token") || text.contains("denied"),
            "an empty token must be refused by the gate: {text}"
        );
    }

    #[tokio::test]
    async fn unknown_tool_is_rejected_before_dispatch() {
        let (ctx, token, _tmp) = test_ctx().await;
        // discover is NOT exposed; an attempt is rejected at the edge allow-list.
        let msg = json!({"jsonrpc":"2.0","id":5,"method":"tools/call",
            "params":{"name":"elastos.inspect.discover","arguments":{}}});
        let resp = handle_mcp_message(&msg, &ctx, &token).await.unwrap();
        assert_eq!(resp["result"]["isError"], true);
        assert!(tool_to_op("elastos.inspect.discover").is_none());
    }
}
