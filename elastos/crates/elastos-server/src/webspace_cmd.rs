use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use elastos_runtime::provider::{BridgeProviderConfig, ProviderBridge};

use crate::WebspaceCommand;

#[derive(Debug, Deserialize, Serialize)]
struct WebSpaceHandle {
    moniker: String,
    handle_uri: String,
    namespace_uri: Option<String>,
    target_uri: Option<String>,
    resolver_state: String,
    kind: String,
    traversable: bool,
    description: String,
    next_step: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DirEntry {
    name: String,
    is_file: bool,
    is_dir: bool,
    size: u64,
}

struct WebSpaceBridge {
    bridge: ProviderBridge,
}

impl WebSpaceBridge {
    async fn resolve(&self, target: &str) -> anyhow::Result<WebSpaceHandle> {
        let mut request = serde_json::json!({
            "op": "resolve",
        });
        if is_handle_path(target) {
            request["path"] = serde_json::Value::String(rooted_webspace_path(target));
        } else {
            request["moniker"] = serde_json::Value::String(target.to_string());
        }
        let resp = self
            .bridge
            .send_raw(&request)
            .await
            .map_err(|e| anyhow::anyhow!("webspace-provider resolve error: {}", e))?;
        parse_webspace_handle_response(resp, "resolve")
    }

    async fn list(&self, path: Option<&str>) -> anyhow::Result<Vec<DirEntry>> {
        let resp = self
            .bridge
            .send_raw(&serde_json::json!({
                "op": "list",
                "path": path.map(rooted_webspace_path).unwrap_or_else(|| "localhost://WebSpaces".to_string()),
                "token": "",
            }))
            .await
            .map_err(|e| anyhow::anyhow!("webspace-provider list error: {}", e))?;
        parse_webspace_list_response(resp, "list")
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        self.bridge
            .send_raw(&serde_json::json!({ "op": "shutdown" }))
            .await
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!("webspace-provider shutdown failed: {}", e))
    }
}

pub(crate) async fn run(cmd: WebspaceCommand) -> anyhow::Result<()> {
    let bridge = spawn_webspace_bridge().await?;

    let result = match cmd {
        WebspaceCommand::Resolve { target, json } => {
            let handle = bridge.resolve(&target).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&handle)?);
            } else {
                println!("WebSpace:  {}", handle.moniker);
                println!("Handle:    {}", handle.handle_uri);
                println!("Meta:      {}", meta_uri(&handle));
                println!(
                    "Namespace: {}",
                    handle.namespace_uri.as_deref().unwrap_or("(not mapped)")
                );
                println!("State:     {}", handle.resolver_state);
                println!("Kind:      {}", handle.kind);
                println!(
                    "Contract:  {}",
                    if handle.traversable {
                        "resolver-owned folder handle"
                    } else {
                        "typed file endpoint"
                    }
                );
                println!(
                    "Target:    {}",
                    handle.target_uri.as_deref().unwrap_or("(resolver-owned)")
                );
                println!(
                    "Traverse:  {}",
                    if handle.traversable { "yes" } else { "no" }
                );
                println!("About:     {}", handle.description);
                if let Some(next_step) = handle.next_step.as_deref() {
                    println!("Next:      {}", next_step);
                }
            }
            Ok(())
        }
        WebspaceCommand::List { path, json } => {
            let entries = bridge.list(path.as_deref()).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else if entries.is_empty() {
                println!("No entries.");
            } else {
                println!(
                    "{}:",
                    path.as_deref()
                        .map(rooted_webspace_path)
                        .unwrap_or_else(|| "localhost://WebSpaces".to_string())
                );
                for entry in entries {
                    let kind = if entry.is_dir {
                        "dir"
                    } else if entry.is_file {
                        "file"
                    } else {
                        "entry"
                    };
                    println!("  - [{}] {}", kind, entry.name);
                }
            }
            Ok(())
        }
    };

    let _ = bridge.shutdown().await;
    result
}

async fn spawn_webspace_bridge() -> anyhow::Result<WebSpaceBridge> {
    let binary = resolve_webspace_provider_binary()?;
    let bridge = ProviderBridge::spawn(&binary, BridgeProviderConfig::default())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to spawn webspace-provider: {}", e))?;
    Ok(WebSpaceBridge { bridge })
}

fn resolve_webspace_provider_binary() -> anyhow::Result<PathBuf> {
    crate::resolve_verified_provider_binary(
        "webspace-provider",
        "webspace-provider not installed.\n\nRun first:\n\n  elastos setup",
    )
}

fn parse_webspace_handle_response(
    resp: serde_json::Value,
    op: &str,
) -> anyhow::Result<WebSpaceHandle> {
    if let Some("error") = resp.get("status").and_then(|v| v.as_str()) {
        let code = resp
            .get("code")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown_error");
        let message = resp
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        anyhow::bail!("webspace-provider {} failed [{}]: {}", op, code, message);
    }

    serde_json::from_value(
        resp.get("data")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("webspace-provider {} response missing data", op))?,
    )
    .map_err(|e| anyhow::anyhow!("Invalid webspace-provider {} response: {}", op, e))
}

fn parse_webspace_list_response(
    resp: serde_json::Value,
    op: &str,
) -> anyhow::Result<Vec<DirEntry>> {
    if let Some("error") = resp.get("status").and_then(|v| v.as_str()) {
        let code = resp
            .get("code")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown_error");
        let message = resp
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        anyhow::bail!("webspace-provider {} failed [{}]: {}", op, code, message);
    }

    serde_json::from_value(
        resp.get("data")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("webspace-provider {} response missing data", op))?,
    )
    .map_err(|e| anyhow::anyhow!("Invalid webspace-provider {} response: {}", op, e))
}

fn is_handle_path(target: &str) -> bool {
    target.starts_with("localhost://") || target.contains('/')
}

fn rooted_webspace_path(target: &str) -> String {
    if target.starts_with("localhost://") {
        target.to_string()
    } else {
        format!("localhost://WebSpaces/{}", target.trim_matches('/'))
    }
}

fn meta_uri(handle: &WebSpaceHandle) -> String {
    format!("{}/_meta.json", handle.handle_uri.trim_end_matches('/'))
}
