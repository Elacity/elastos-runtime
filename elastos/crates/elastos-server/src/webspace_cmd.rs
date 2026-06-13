use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use elastos_runtime::provider::{BridgeProviderConfig, ProviderBridge};
use elastos_server::sources::default_data_dir;

use crate::WebspaceCommand;

#[derive(Debug, Deserialize, Serialize)]
struct WebSpaceHandle {
    moniker: String,
    handle_uri: String,
    namespace_uri: Option<String>,
    target_uri: Option<String>,
    resolver_state: String,
    resolver: String,
    cache_policy: String,
    sync_policy: String,
    readonly: bool,
    kind: String,
    traversable: bool,
    object_id: String,
    head_id: String,
    cache_state: String,
    sync_state: String,
    description: String,
    forked_from: Option<String>,
    next_step: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DirEntry {
    name: String,
    is_file: bool,
    is_dir: bool,
    size: u64,
    #[serde(default)]
    resolver: Option<String>,
    #[serde(default)]
    cache_policy: Option<String>,
    #[serde(default)]
    sync_policy: Option<String>,
    #[serde(default)]
    object_id: Option<String>,
    #[serde(default)]
    head_id: Option<String>,
    #[serde(default)]
    cache_state: Option<String>,
    #[serde(default)]
    sync_state: Option<String>,
    #[serde(default)]
    target_uri: Option<String>,
    #[serde(default)]
    readonly: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
struct WebSpaceMount {
    moniker: String,
    target_uri: String,
    namespace_uri: Option<String>,
    resolver: String,
    readonly: bool,
    cache_policy: String,
    sync_policy: String,
    description: String,
    #[serde(default)]
    forked_from: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct WebSpaceMounts {
    schema: String,
    mounts: Vec<WebSpaceHandle>,
    user_mounts: Vec<WebSpaceMount>,
}

#[derive(Debug, Deserialize, Serialize)]
struct WebSpaceAdapterTable {
    schema: String,
    builtin: serde_json::Value,
    adapters: Vec<serde_json::Value>,
    configured_adapter_count: usize,
    connected_adapter_count: usize,
    #[serde(default)]
    checked_adapter_count: usize,
    note: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct WebSpaceIndexInput {
    path: String,
    kind: String,
    #[serde(default)]
    target_uri: Option<String>,
    #[serde(default)]
    resolver_state: Option<String>,
    #[serde(default)]
    readonly: Option<bool>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct WebSpaceHead {
    handle_uri: String,
    target_uri: Option<String>,
    moniker: String,
    resolver: String,
    object_id: String,
    head_id: String,
    revision: String,
    readonly: bool,
    cache_policy: String,
    sync_policy: String,
    cache_state: String,
    sync_state: String,
    forked_from: Option<String>,
    created_at: u64,
    updated_at: u64,
    last_cached_at: Option<u64>,
    last_synced_at: Option<u64>,
    dirty: bool,
    status: String,
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

    async fn mounts(&self) -> anyhow::Result<WebSpaceMounts> {
        let resp = self
            .bridge
            .send_raw(&serde_json::json!({
                "op": "mounts",
                "token": "",
            }))
            .await
            .map_err(|e| anyhow::anyhow!("webspace-provider mounts error: {}", e))?;
        parse_webspace_mounts_response(resp, "mounts")
    }

    async fn adapters(&self) -> anyhow::Result<WebSpaceAdapterTable> {
        let resp = self
            .bridge
            .send_raw(&serde_json::json!({
                "op": "adapters",
                "token": "",
            }))
            .await
            .map_err(|e| anyhow::anyhow!("webspace-provider adapters error: {}", e))?;
        serde_json::from_value(parse_webspace_value_response(resp, "adapters")?)
            .map_err(|e| anyhow::anyhow!("Invalid webspace-provider adapters response: {}", e))
    }

    #[allow(clippy::too_many_arguments)]
    async fn register_adapter(
        &self,
        resolver: String,
        label: Option<String>,
        endpoint_uri: Option<String>,
        provider: Option<String>,
        state: Option<String>,
        capabilities: Vec<String>,
        mutable_default: bool,
        description: Option<String>,
    ) -> anyhow::Result<serde_json::Value> {
        let resp = self
            .bridge
            .send_raw(&serde_json::json!({
                "op": "register_adapter",
                "resolver": resolver,
                "label": label,
                "endpoint_uri": endpoint_uri,
                "provider": provider,
                "state": state,
                "capabilities": capabilities,
                "readonly_default": !mutable_default,
                "description": description,
                "token": "",
            }))
            .await
            .map_err(|e| anyhow::anyhow!("webspace-provider register_adapter error: {}", e))?;
        parse_webspace_value_response(resp, "register_adapter")
    }

    async fn unregister_adapter(&self, resolver: String) -> anyhow::Result<serde_json::Value> {
        let resp = self
            .bridge
            .send_raw(&serde_json::json!({
                "op": "unregister_adapter",
                "resolver": resolver,
                "token": "",
            }))
            .await
            .map_err(|e| anyhow::anyhow!("webspace-provider unregister_adapter error: {}", e))?;
        parse_webspace_value_response(resp, "unregister_adapter")
    }

    async fn check_adapter(
        &self,
        resolver: String,
        result: Option<String>,
        state: Option<String>,
        error_code: Option<String>,
        capabilities: Vec<String>,
    ) -> anyhow::Result<serde_json::Value> {
        let resp = self
            .bridge
            .send_raw(&serde_json::json!({
                "op": "check_adapter",
                "resolver": resolver,
                "result": result,
                "state": state,
                "error_code": error_code,
                "capabilities": capabilities,
                "token": "",
            }))
            .await
            .map_err(|e| anyhow::anyhow!("webspace-provider check_adapter error: {}", e))?;
        parse_webspace_value_response(resp, "check_adapter")
    }

    async fn health(&self, moniker: Option<String>) -> anyhow::Result<serde_json::Value> {
        let resp = self
            .bridge
            .send_raw(&serde_json::json!({
                "op": "health",
                "moniker": moniker,
                "token": "",
            }))
            .await
            .map_err(|e| anyhow::anyhow!("webspace-provider health error: {}", e))?;
        parse_webspace_value_response(resp, "health")
    }

    #[allow(clippy::too_many_arguments)]
    async fn mount(
        &self,
        moniker: String,
        target_uri: String,
        namespace_uri: Option<String>,
        resolver: Option<String>,
        description: Option<String>,
        mutable: bool,
        cache_policy: Option<String>,
        sync_policy: Option<String>,
    ) -> anyhow::Result<serde_json::Value> {
        let resp = self
            .bridge
            .send_raw(&serde_json::json!({
                "op": "mount",
                "moniker": moniker,
                "target_uri": target_uri,
                "namespace_uri": namespace_uri,
                "resolver": resolver,
                "description": description,
                "readonly": !mutable,
                "cache_policy": cache_policy,
                "sync_policy": sync_policy,
                "token": "",
            }))
            .await
            .map_err(|e| anyhow::anyhow!("webspace-provider mount error: {}", e))?;
        parse_webspace_value_response(resp, "mount")
    }

    async fn unmount(&self, moniker: String) -> anyhow::Result<serde_json::Value> {
        let resp = self
            .bridge
            .send_raw(&serde_json::json!({
                "op": "unmount",
                "moniker": moniker,
                "token": "",
            }))
            .await
            .map_err(|e| anyhow::anyhow!("webspace-provider unmount error: {}", e))?;
        parse_webspace_value_response(resp, "unmount")
    }

    async fn index(
        &self,
        moniker: String,
        entries: Vec<WebSpaceIndexInput>,
    ) -> anyhow::Result<serde_json::Value> {
        let resp = self
            .bridge
            .send_raw(&serde_json::json!({
                "op": "index",
                "moniker": moniker,
                "entries": entries,
                "token": "",
            }))
            .await
            .map_err(|e| anyhow::anyhow!("webspace-provider index error: {}", e))?;
        parse_webspace_value_response(resp, "index")
    }

    async fn refresh(
        &self,
        target: String,
        entries: Option<Vec<WebSpaceIndexInput>>,
    ) -> anyhow::Result<serde_json::Value> {
        let resp = self
            .bridge
            .send_raw(&serde_json::json!({
                "op": "refresh",
                "path": rooted_webspace_path(&target),
                "entries": entries,
                "token": "",
            }))
            .await
            .map_err(|e| anyhow::anyhow!("webspace-provider refresh error: {}", e))?;
        parse_webspace_value_response(resp, "refresh")
    }

    async fn head(&self, target: &str) -> anyhow::Result<WebSpaceHead> {
        let resp = self
            .bridge
            .send_raw(&serde_json::json!({
                "op": "head",
                "path": rooted_webspace_path(target),
                "token": "",
            }))
            .await
            .map_err(|e| anyhow::anyhow!("webspace-provider head error: {}", e))?;
        serde_json::from_value(parse_webspace_value_response(resp, "head")?)
            .map_err(|e| anyhow::anyhow!("Invalid webspace-provider head response: {}", e))
    }

    async fn cache(&self, target: &str) -> anyhow::Result<serde_json::Value> {
        let resp = self
            .bridge
            .send_raw(&serde_json::json!({
                "op": "cache",
                "path": rooted_webspace_path(target),
                "token": "",
            }))
            .await
            .map_err(|e| anyhow::anyhow!("webspace-provider cache error: {}", e))?;
        parse_webspace_value_response(resp, "cache")
    }

    async fn cache_status(&self, target: &str) -> anyhow::Result<serde_json::Value> {
        let resp = self
            .bridge
            .send_raw(&serde_json::json!({
                "op": "cache_status",
                "path": rooted_webspace_path(target),
                "token": "",
            }))
            .await
            .map_err(|e| anyhow::anyhow!("webspace-provider cache_status error: {}", e))?;
        parse_webspace_value_response(resp, "cache_status")
    }

    async fn sync(&self, target: &str) -> anyhow::Result<serde_json::Value> {
        let resp = self
            .bridge
            .send_raw(&serde_json::json!({
                "op": "sync",
                "path": rooted_webspace_path(target),
                "token": "",
            }))
            .await
            .map_err(|e| anyhow::anyhow!("webspace-provider sync error: {}", e))?;
        parse_webspace_value_response(resp, "sync")
    }

    async fn sync_status(&self, target: &str) -> anyhow::Result<serde_json::Value> {
        let resp = self
            .bridge
            .send_raw(&serde_json::json!({
                "op": "sync_status",
                "path": rooted_webspace_path(target),
                "token": "",
            }))
            .await
            .map_err(|e| anyhow::anyhow!("webspace-provider sync_status error: {}", e))?;
        parse_webspace_value_response(resp, "sync_status")
    }

    #[allow(clippy::too_many_arguments)]
    async fn fork(
        &self,
        source: String,
        moniker: String,
        target_uri: Option<String>,
        resolver: Option<String>,
        description: Option<String>,
        readonly: bool,
        cache_policy: Option<String>,
        sync_policy: Option<String>,
    ) -> anyhow::Result<serde_json::Value> {
        let resp = self
            .bridge
            .send_raw(&serde_json::json!({
                "op": "fork",
                "source_uri": rooted_webspace_path(&source),
                "moniker": moniker,
                "target_uri": target_uri,
                "resolver": resolver,
                "description": description,
                "readonly": readonly,
                "cache_policy": cache_policy,
                "sync_policy": sync_policy,
                "token": "",
            }))
            .await
            .map_err(|e| anyhow::anyhow!("webspace-provider fork error: {}", e))?;
        parse_webspace_value_response(resp, "fork")
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
        WebspaceCommand::Mounts { json } => {
            let mounts = bridge.mounts().await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&mounts)?);
            } else {
                println!("WebSpace mounts:");
                for handle in mounts.mounts {
                    println!(
                        "  - {} -> {} [{}; cache {}; sync {}]",
                        handle.moniker,
                        handle.target_uri.as_deref().unwrap_or("(resolver-owned)"),
                        handle.resolver,
                        handle.cache_policy,
                        handle.sync_policy
                    );
                }
                if mounts.user_mounts.is_empty() {
                    println!("Persistent custom mounts: none");
                } else {
                    println!("Persistent custom mounts: {}", mounts.user_mounts.len());
                }
            }
            Ok(())
        }
        WebspaceCommand::Adapters { json } => {
            let adapters = bridge.adapters().await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&adapters)?);
            } else {
                println!("WebSpace adapters:");
                println!(
                    "  - builtin [{}]",
                    adapters
                        .builtin
                        .get("state")
                        .and_then(|value| value.as_str())
                        .unwrap_or("connected")
                );
                for adapter in &adapters.adapters {
                    println!(
                        "  - {} [{}]{}{}{}",
                        adapter
                            .get("resolver")
                            .and_then(|value| value.as_str())
                            .unwrap_or("(unknown)"),
                        adapter
                            .get("state")
                            .and_then(|value| value.as_str())
                            .unwrap_or("unknown"),
                        adapter
                            .get("health")
                            .and_then(|value| value.get("status"))
                            .and_then(|value| value.as_str())
                            .map(|health| format!(" health={health}"))
                            .unwrap_or_default(),
                        adapter
                            .get("provider")
                            .and_then(|value| value.as_str())
                            .map(|provider| format!(" provider={provider}"))
                            .unwrap_or_default(),
                        adapter
                            .get("endpoint_uri")
                            .and_then(|value| value.as_str())
                            .map(|endpoint| format!(" endpoint={endpoint}"))
                            .unwrap_or_default()
                    );
                }
                println!(
                    "Configured external adapters: {} (connected: {})",
                    adapters.configured_adapter_count, adapters.connected_adapter_count
                );
                println!(
                    "Checked external adapters: {}",
                    adapters.checked_adapter_count
                );
                if let Some(note) = adapters.note.as_deref() {
                    println!("Note: {note}");
                }
            }
            Ok(())
        }
        WebspaceCommand::RegisterAdapter {
            resolver,
            label,
            endpoint_uri,
            provider,
            state,
            capabilities,
            mutable_default,
            description,
            json,
        } => {
            let receipt = bridge
                .register_adapter(
                    resolver,
                    label,
                    endpoint_uri,
                    provider,
                    state,
                    capabilities,
                    mutable_default,
                    description,
                )
                .await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&receipt)?);
            } else {
                let adapter = receipt.get("adapter").unwrap_or(&serde_json::Value::Null);
                println!(
                    "Registered adapter {} [{}]",
                    adapter
                        .get("resolver")
                        .and_then(|value| value.as_str())
                        .unwrap_or("(unknown)"),
                    adapter
                        .get("state")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown")
                );
                if let Some(endpoint) = adapter.get("endpoint_uri").and_then(|value| value.as_str())
                {
                    println!("Endpoint: {endpoint}");
                }
            }
            Ok(())
        }
        WebspaceCommand::UnregisterAdapter { resolver, json } => {
            let receipt = bridge.unregister_adapter(resolver).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&receipt)?);
            } else {
                let adapter = receipt.get("adapter").unwrap_or(&serde_json::Value::Null);
                println!(
                    "Unregistered adapter {}",
                    adapter
                        .get("resolver")
                        .and_then(|value| value.as_str())
                        .unwrap_or("(unknown)")
                );
            }
            Ok(())
        }
        WebspaceCommand::CheckAdapter {
            resolver,
            result,
            state,
            error_code,
            capabilities,
            json,
        } => {
            let receipt = bridge
                .check_adapter(resolver, result, state, error_code, capabilities)
                .await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&receipt)?);
            } else {
                let adapter = receipt.get("adapter").unwrap_or(&serde_json::Value::Null);
                let health = adapter.get("health").unwrap_or(&serde_json::Value::Null);
                println!(
                    "Checked adapter {} [{}; health {}]",
                    adapter
                        .get("resolver")
                        .and_then(|value| value.as_str())
                        .unwrap_or("(unknown)"),
                    adapter
                        .get("state")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown"),
                    health
                        .get("status")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown")
                );
                if let Some(next) = health.get("next").and_then(|value| value.as_str()) {
                    println!("Next: {next}");
                }
                if let Some(note) = receipt.get("note").and_then(|value| value.as_str()) {
                    println!("Note: {note}");
                }
            }
            Ok(())
        }
        WebspaceCommand::Health { moniker, json } => {
            let report = bridge.health(moniker).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "WebSpace health: {}",
                    report
                        .get("state")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown")
                );
                println!(
                    "Mounts: {} (dirty heads: {}, live adapters: {})",
                    report
                        .get("mount_count")
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0),
                    report
                        .get("dirty_head_count")
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0),
                    report
                        .get("live_adapter_count")
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0)
                );
                for mount in report
                    .get("mounts")
                    .and_then(|value| value.as_array())
                    .into_iter()
                    .flatten()
                {
                    println!(
                        "  - {} [{}; resolver {}; adapter {}]",
                        mount
                            .get("moniker")
                            .and_then(|value| value.as_str())
                            .unwrap_or("(unknown)"),
                        mount
                            .get("state")
                            .and_then(|value| value.as_str())
                            .unwrap_or("unknown"),
                        mount
                            .get("resolver")
                            .and_then(|value| value.as_str())
                            .unwrap_or("unknown"),
                        mount
                            .get("adapter_state")
                            .and_then(|value| value.as_str())
                            .unwrap_or("unknown")
                    );
                }
                if let Some(note) = report.get("note").and_then(|value| value.as_str()) {
                    println!("Note: {note}");
                }
            }
            Ok(())
        }
        WebspaceCommand::Mount {
            moniker,
            target_uri,
            namespace_uri,
            resolver,
            description,
            mutable,
            cache_policy,
            sync_policy,
            json,
        } => {
            let receipt = bridge
                .mount(
                    moniker,
                    target_uri,
                    namespace_uri,
                    resolver,
                    description,
                    mutable,
                    cache_policy,
                    sync_policy,
                )
                .await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&receipt)?);
            } else {
                let mount = receipt.get("mount").unwrap_or(&serde_json::Value::Null);
                println!(
                    "Mounted {} -> {}",
                    mount
                        .get("moniker")
                        .and_then(|value| value.as_str())
                        .unwrap_or("(unknown)"),
                    mount
                        .get("target_uri")
                        .and_then(|value| value.as_str())
                        .unwrap_or("(unknown)")
                );
            }
            Ok(())
        }
        WebspaceCommand::Unmount { moniker, json } => {
            let receipt = bridge.unmount(moniker).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&receipt)?);
            } else {
                let mount = receipt.get("mount").unwrap_or(&serde_json::Value::Null);
                println!(
                    "Unmounted {}",
                    mount
                        .get("moniker")
                        .and_then(|value| value.as_str())
                        .unwrap_or("(unknown)")
                );
            }
            Ok(())
        }
        WebspaceCommand::Index {
            moniker,
            entries_json,
            json,
        } => {
            let bytes = fs::read(&entries_json).map_err(|err| {
                anyhow::anyhow!(
                    "failed to read WebSpace index file {}: {}",
                    entries_json.display(),
                    err
                )
            })?;
            let entries: Vec<WebSpaceIndexInput> =
                serde_json::from_slice(&bytes).map_err(|err| {
                    anyhow::anyhow!(
                        "invalid WebSpace index file {}: {}",
                        entries_json.display(),
                        err
                    )
                })?;
            let receipt = bridge.index(moniker, entries).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&receipt)?);
            } else {
                println!(
                    "Indexed {} entries for {}",
                    receipt
                        .get("entry_count")
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0),
                    receipt
                        .get("moniker")
                        .and_then(|value| value.as_str())
                        .unwrap_or("(unknown)")
                );
            }
            Ok(())
        }
        WebspaceCommand::Refresh {
            target,
            entries_json,
            json,
        } => {
            let entries = if let Some(entries_json) = entries_json {
                let bytes = fs::read(&entries_json).map_err(|err| {
                    anyhow::anyhow!(
                        "failed to read WebSpace index file {}: {}",
                        entries_json.display(),
                        err
                    )
                })?;
                Some(
                    serde_json::from_slice::<Vec<WebSpaceIndexInput>>(&bytes).map_err(|err| {
                        anyhow::anyhow!(
                            "invalid WebSpace index file {}: {}",
                            entries_json.display(),
                            err
                        )
                    })?,
                )
            } else {
                None
            };
            let receipt = bridge.refresh(target, entries).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&receipt)?);
            } else {
                println!(
                    "Refreshed {}",
                    receipt
                        .get("handle_uri")
                        .and_then(|value| value.as_str())
                        .unwrap_or("(unknown handle)")
                );
                if let Some(count) = receipt
                    .get("index_entry_count")
                    .and_then(|value| value.as_u64())
                {
                    println!("Index entries: {count}");
                }
                if let Some(note) = receipt.get("note").and_then(|value| value.as_str()) {
                    println!("Note: {note}");
                }
            }
            Ok(())
        }
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
                println!("Resolver:  {}", handle.resolver);
                println!("Cache:     {}", handle.cache_policy);
                println!("CacheState: {}", handle.cache_state);
                println!("Sync:      {}", handle.sync_policy);
                println!("SyncState: {}", handle.sync_state);
                println!("Object:    {}", handle.object_id);
                println!("Head:      {}", handle.head_id);
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
                println!("Readonly:  {}", if handle.readonly { "yes" } else { "no" });
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
        WebspaceCommand::Head { target, json } => {
            let head = bridge.head(&target).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&head)?);
            } else {
                println!("Handle:    {}", head.handle_uri);
                println!("Object:    {}", head.object_id);
                println!("Head:      {}", head.head_id);
                println!("Revision:  {}", head.revision);
                println!("Resolver:  {}", head.resolver);
                println!("Cache:     {} ({})", head.cache_policy, head.cache_state);
                println!("Sync:      {} ({})", head.sync_policy, head.sync_state);
                println!("Dirty:     {}", if head.dirty { "yes" } else { "no" });
                println!("Status:    {}", head.status);
                if let Some(target_uri) = head.target_uri.as_deref() {
                    println!("Target:    {target_uri}");
                }
                if let Some(forked_from) = head.forked_from.as_deref() {
                    println!("ForkedFrom:{forked_from}");
                }
            }
            Ok(())
        }
        WebspaceCommand::Cache { target, json } => {
            let receipt = bridge.cache(&target).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&receipt)?);
            } else {
                println!(
                    "Cached metadata for {}",
                    receipt
                        .get("handle_uri")
                        .and_then(|value| value.as_str())
                        .unwrap_or("(unknown handle)")
                );
                if let Some(note) = receipt.get("note").and_then(|value| value.as_str()) {
                    println!("Note: {note}");
                }
            }
            Ok(())
        }
        WebspaceCommand::CacheStatus { target, json } => {
            let status = bridge.cache_status(&target).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                println!(
                    "Cache: {} ({})",
                    status
                        .get("policy")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown"),
                    status
                        .get("state")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown")
                );
                println!(
                    "Content cached: {}",
                    status
                        .get("content_cached")
                        .and_then(|value| value.as_bool())
                        .unwrap_or(false)
                );
                if let Some(note) = status.get("note").and_then(|value| value.as_str()) {
                    println!("Note: {note}");
                }
            }
            Ok(())
        }
        WebspaceCommand::Sync { target, json } => {
            let receipt = bridge.sync(&target).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&receipt)?);
            } else {
                println!(
                    "Synced metadata for {}",
                    receipt
                        .get("handle_uri")
                        .and_then(|value| value.as_str())
                        .unwrap_or("(unknown handle)")
                );
                if let Some(note) = receipt.get("note").and_then(|value| value.as_str()) {
                    println!("Note: {note}");
                }
            }
            Ok(())
        }
        WebspaceCommand::SyncStatus { target, json } => {
            let status = bridge.sync_status(&target).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                println!(
                    "Sync: {} ({})",
                    status
                        .get("policy")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown"),
                    status
                        .get("state")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown")
                );
                println!(
                    "Dirty: {}",
                    status
                        .get("dirty")
                        .and_then(|value| value.as_bool())
                        .unwrap_or(false)
                );
                if let Some(note) = status.get("note").and_then(|value| value.as_str()) {
                    println!("Note: {note}");
                }
            }
            Ok(())
        }
        WebspaceCommand::Fork {
            source,
            moniker,
            target_uri,
            resolver,
            description,
            readonly,
            cache_policy,
            sync_policy,
            json,
        } => {
            let receipt = bridge
                .fork(
                    source,
                    moniker,
                    target_uri,
                    resolver,
                    description,
                    readonly,
                    cache_policy,
                    sync_policy,
                )
                .await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&receipt)?);
            } else {
                let mount = receipt.get("mount").unwrap_or(&serde_json::Value::Null);
                let head = receipt.get("head").unwrap_or(&serde_json::Value::Null);
                println!(
                    "Forked {} -> {}",
                    mount
                        .get("forked_from")
                        .and_then(|value| value.as_str())
                        .unwrap_or("(unknown source)"),
                    mount
                        .get("moniker")
                        .and_then(|value| value.as_str())
                        .unwrap_or("(unknown mount)")
                );
                println!(
                    "Head: {} [{}]",
                    head.get("head_id")
                        .and_then(|value| value.as_str())
                        .unwrap_or("(unknown head)"),
                    head.get("sync_state")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown")
                );
                if let Some(next_step) = receipt.get("next_step").and_then(|value| value.as_str()) {
                    println!("Next: {next_step}");
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
                    println!(
                        "  - [{}] {}{}{}",
                        kind,
                        entry.name,
                        entry
                            .target_uri
                            .as_deref()
                            .map(|target| format!(" -> {target}"))
                            .unwrap_or_default(),
                        entry
                            .cache_state
                            .as_deref()
                            .map(|state| format!(" [{state}]"))
                            .unwrap_or_default()
                    );
                }
            }
            Ok(())
        }
        _ => Err(anyhow::anyhow!(
            "this WebSpace CLI command is not wired to the current webspace-provider bridge yet; supported commands are resolve and list"
        )),
    };

    let _ = bridge.shutdown().await;
    result
}

async fn spawn_webspace_bridge() -> anyhow::Result<WebSpaceBridge> {
    let binary = resolve_webspace_provider_binary()?;
    let config = BridgeProviderConfig {
        base_path: default_data_dir().to_string_lossy().to_string(),
        read_only: false,
        ..BridgeProviderConfig::default()
    };
    let bridge = ProviderBridge::spawn(&binary, config)
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

fn parse_webspace_mounts_response(
    resp: serde_json::Value,
    op: &str,
) -> anyhow::Result<WebSpaceMounts> {
    serde_json::from_value(parse_webspace_value_response(resp, op)?)
        .map_err(|e| anyhow::anyhow!("Invalid webspace-provider {} response: {}", op, e))
}

fn parse_webspace_value_response(
    resp: serde_json::Value,
    op: &str,
) -> anyhow::Result<serde_json::Value> {
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

    resp.get("data")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("webspace-provider {} response missing data", op))
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
