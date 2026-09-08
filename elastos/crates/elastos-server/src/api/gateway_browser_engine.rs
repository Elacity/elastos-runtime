//! Browser engine and Net/Exit summary helpers.

use super::*;
use elastos_common::browser_protocol::{
    BrowserEngineReadiness, BrowserEngineReadinessReason, BROWSER_ENGINE_READINESS_SCHEMA,
};

pub(in crate::api::gateway) async fn resolve_browser_engine_adapter(
    registry: &ProviderRegistry,
    data_dir: &FsPath,
    principal_id: &str,
    requested_adapter: Option<&str>,
    display_mode: BrowserDisplayMode,
    guarantee_level: BrowserGuaranteeLevel,
) -> Result<String, BrowserCompatibilityError> {
    resolve_browser_engine_adapter_with_preparation(
        registry,
        data_dir,
        principal_id,
        requested_adapter,
        display_mode,
        guarantee_level,
        || crate::setup::ensure_browser_vm_image_for_local_engine(data_dir),
    )
    .await
}

async fn resolve_browser_engine_adapter_with_preparation<F, Fut>(
    registry: &ProviderRegistry,
    data_dir: &FsPath,
    principal_id: &str,
    requested_adapter: Option<&str>,
    display_mode: BrowserDisplayMode,
    guarantee_level: BrowserGuaranteeLevel,
    mut prepare_image: F,
) -> Result<String, BrowserCompatibilityError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let response = registry
        .send_raw(
            "browser-engine",
            &serde_json::json!({
                "op": "status",
                "principal_id": principal_id,
            }),
        )
        .await
        .map_err(|_| BrowserCompatibilityError::EngineUnavailable)?;
    if response.get("status").and_then(serde_json::Value::as_str) != Some("ok") {
        return Err(BrowserCompatibilityError::EngineUnavailable);
    }
    let data =
        provider_response_data(&response).ok_or(BrowserCompatibilityError::InvalidEngineStatus)?;
    let inventory = BrowserEngineInventory::from_status(&data)?;
    // Validate explicit choice and capability errors before probing a host.
    inventory.select(requested_adapter, display_mode, guarantee_level)?;
    let mut candidates = inventory
        .adapters
        .iter()
        .filter(|adapter| adapter.supports(display_mode, guarantee_level))
        .filter(|adapter| requested_adapter.is_none_or(|id| adapter.id == id))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|adapter| !adapter.default);
    let mut readiness_budget = std::time::Duration::from_secs(12);
    let mut unavailable_reason = BrowserEngineReadinessReason::ReadinessUnsupported;
    for adapter in candidates {
        if browser_engine_requires_runtime_image(registry, data_dir, adapter).await {
            if let Err(error) = prepare_image().await {
                tracing::warn!(adapter_id = %adapter.id, error = %error,
                    "Browser local Engine image preparation failed");
                unavailable_reason = BrowserEngineReadinessReason::PreparationRequired;
                continue;
            }
        }
        let request = serde_json::json!({
            "op": "readiness", "principal_id": principal_id, "adapter_id": adapter.id,
        });
        // Acquisition can exceed the host probe budget. Start this probe after
        // preparation, preserving the total twelve seconds across candidates.
        let probe_started = tokio::time::Instant::now();
        let deadline = probe_started + readiness_budget;
        let result =
            match tokio::time::timeout_at(deadline, registry.send_raw("browser-engine", &request))
                .await
            {
                Ok(result) => result,
                Err(_) => {
                    return Err(BrowserCompatibilityError::EngineNotReady {
                        reason: BrowserEngineReadinessReason::ControlUnavailable,
                    })
                }
            };
        readiness_budget = readiness_budget.saturating_sub(probe_started.elapsed());
        let readiness = result.ok().and_then(|response| {
            if response.get("status").and_then(serde_json::Value::as_str) != Some("ok") {
                return None;
            }
            let data = provider_response_data(&response)?;
            if data.get("schema").and_then(serde_json::Value::as_str)
                != Some(BROWSER_ENGINE_READINESS_SCHEMA)
                || data.get("adapter_id").and_then(serde_json::Value::as_str) != Some(&adapter.id)
            {
                return None;
            }
            serde_json::from_value::<BrowserEngineReadiness>(data["readiness"].clone()).ok()
        });
        match readiness {
            Some(BrowserEngineReadiness::Ready {}) => return Ok(adapter.id.clone()),
            Some(BrowserEngineReadiness::Unavailable { reason }) => unavailable_reason = reason,
            None => unavailable_reason = BrowserEngineReadinessReason::ReadinessUnsupported,
        }
    }
    Err(BrowserCompatibilityError::EngineNotReady {
        reason: unavailable_reason,
    })
}

async fn browser_engine_requires_runtime_image(
    registry: &ProviderRegistry,
    data_dir: &FsPath,
    adapter: &elastos_common::browser_protocol::BrowserEngineAdapterCapabilities,
) -> bool {
    if adapter.engine != "chromium_microvm" || adapter.backing_substrate != "local_microvm" {
        return false;
    }
    // Inventory capability strings alone do not bind a provider to Runtime's
    // image layout. Match the configured first-party host launcher as well.
    if registry
        .registration_for_uri("elastos://browser-engine/status")
        .await
        .is_none_or(|registration| registration.provider != "capsule-provider")
    {
        return false;
    }
    let raw = match std::env::var("ELASTOS_BROWSER_ENGINE_ADAPTER_CONFIG") {
        Ok(raw) => raw,
        Err(_) => {
            match tokio::fs::read_to_string(data_dir.join("config/browser-engine-adapter.json"))
                .await
            {
                Ok(raw) => raw,
                Err(_) => return false,
            }
        }
    };
    let Ok(mut config) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    // Supervisor commands inherit Runtime's environment. Explicit configuration
    // wins, just as it does when the provider starts the launcher.
    if let Some(adapters) = config["adapters"].as_array_mut() {
        for entry in adapters {
            if let Some(env) = entry["supervisor"]["env"].as_object_mut() {
                for key in [
                    "ELASTOS_BROWSER_VM_ROOTFS",
                    "ELASTOS_BROWSER_VM_ROOTFS_MANIFEST",
                    "ELASTOS_BROWSER_VM_KERNEL",
                    "ELASTOS_BROWSER_VM_INITRD",
                    "ELASTOS_BROWSER_VM_INITRAMFS",
                ] {
                    if !env.contains_key(key) {
                        if let Ok(value) = std::env::var(key) {
                            env.insert(key.into(), serde_json::Value::String(value));
                        }
                    }
                }
            }
        }
    }
    browser_engine_config_uses_runtime_image(
        &config,
        data_dir,
        &adapter.id,
        &crate::setup::detect_platform(),
    )
}

fn browser_engine_config_uses_runtime_image(
    config: &serde_json::Value,
    data_dir: &FsPath,
    adapter_id: &str,
    platform: &str,
) -> bool {
    let launcher = if platform.starts_with("darwin-") {
        "browser-vz-engine-supervisor"
    } else if platform.starts_with("linux-") {
        "browser-vm-local-crosvm-launcher"
    } else {
        return false;
    };
    let Some(adapter) = config["adapters"].as_array().and_then(|adapters| {
        adapters
            .iter()
            .find(|adapter| adapter["id"].as_str() == Some(adapter_id))
    }) else {
        return false;
    };
    let supervisor = &adapter["supervisor"];
    let env = &supervisor["env"];
    let path_matches = |value: &serde_json::Value, relative: &str| {
        value
            .as_str()
            .is_some_and(|value| FsPath::new(value) == data_dir.join(relative))
    };
    adapter["kind"] == "chromium_microvm"
        && path_matches(&supervisor["program"], "bin/browser-vm-engine-supervisor")
        && path_matches(
            &env["ELASTOS_BROWSER_VM_CONTROL_LAUNCHER"],
            &format!("bin/{launcher}"),
        )
        && path_matches(&env["ELASTOS_BROWSER_VM_DATA_DIR"], "")
        && [
            ("ELASTOS_BROWSER_VM_ROOTFS", "browser-vm/rootfs.ext4"),
            (
                "ELASTOS_BROWSER_VM_ROOTFS_MANIFEST",
                "browser-vm/browser-vm-rootfs-manifest.json",
            ),
            ("ELASTOS_BROWSER_VM_KERNEL", "bin/vmlinux"),
            if platform.starts_with("darwin-") {
                ("ELASTOS_BROWSER_VM_INITRAMFS", "bin/initrd")
            } else {
                ("ELASTOS_BROWSER_VM_INITRD", "browser-vm/initrd")
            },
        ]
        .iter()
        .all(|(key, path)| env[*key].is_null() || path_matches(&env[*key], path))
}

pub(in crate::api::gateway) async fn browser_engine_summary(
    registry: Option<&Arc<ProviderRegistry>>,
    principal_id: &str,
) -> serde_json::Value {
    let unavailable = || {
        serde_json::json!({
            "status": "unavailable",
            "provider": "elastos://browser-engine/*",
            "mode": "not_configured",
            "required": "native/webview or microVM Browser Engine Adapter",
            "stream_session_schema": "elastos.exit.stream-session/v1",
            "display_session_schema": "elastos.browser.display-session/v1",
            "supported_display_modes": [],
            "supported_guarantee_levels": [],
            "byte_transport": "not_attached",
            "direct_network": false,
            "wallet_injection": false,
            "reason": "Browser Engine Adapter is not installed. This surface can request Runtime networking, but it cannot render arbitrary sites inside ElastOS yet."
        })
    };
    let Some(registry) = registry else {
        return unavailable();
    };
    match registry
        .send_raw(
            "browser-engine",
            &serde_json::json!({
                "op": "status",
                "principal_id": principal_id,
            }),
        )
        .await
    {
        Ok(value) if value.get("status").and_then(|entry| entry.as_str()) == Some("ok") => {
            let data = provider_response_data(&value).unwrap_or(value);
            if authority_false_proof_missing(&data, "direct_network") {
                return invalid_provider_summary(
                    "elastos://browser-engine/*",
                    "Browser Engine Adapter status omitted direct_network=false proof.",
                );
            }
            if authority_false_proof_missing(&data, "wallet_injection") {
                return invalid_provider_summary(
                    "elastos://browser-engine/*",
                    "Browser Engine Adapter status omitted wallet_injection=false proof.",
                );
            }
            if let Err(error) = BrowserEngineInventory::from_status(&data) {
                let mut summary =
                    invalid_provider_summary("elastos://browser-engine/*", &error.to_string());
                summary["code"] = serde_json::to_value(error)
                    .expect("Browser compatibility error serializes")["code"]
                    .clone();
                return summary;
            }
            serde_json::json!({
                "protocol_version": BROWSER_ENGINE_PROTOCOL_VERSION,
                "status": data.get("status").cloned().unwrap_or_else(|| serde_json::json!("unavailable")),
                "provider": "elastos://browser-engine/*",
                "mode": data.get("provider").cloned().unwrap_or_else(|| serde_json::json!("browser-engine-adapter")),
                "operations": data.get("operations").cloned().unwrap_or_else(|| serde_json::json!(["status", "launch", "attach_stream", "close_page"])),
                "adapter_count": data.get("adapter_count").cloned().unwrap_or_else(|| serde_json::json!(0)),
                "adapters": browser_visible_engine_adapters(&data),
                "active_sessions": data.get("active_sessions").cloned().unwrap_or_else(|| serde_json::json!(0)),
                "max_active_sessions": data.get("max_active_sessions").cloned().unwrap_or_else(|| serde_json::json!(0)),
                "capacity_available": data.get("capacity_available").cloned().unwrap_or(serde_json::Value::Bool(false)),
                "stream_session_schema": data.get("stream_session_schema").cloned().unwrap_or_else(|| serde_json::json!("elastos.exit.stream-session/v1")),
                "display_session_schema": data.get("display_session_schema").cloned().unwrap_or_else(|| serde_json::json!("elastos.browser.display-session/v1")),
                "supported_display_modes": data.get("supported_display_modes").cloned().unwrap_or_else(|| serde_json::json!([])),
                "supported_guarantee_levels": data.get("supported_guarantee_levels").cloned().unwrap_or_else(|| serde_json::json!([])),
                "byte_transport": data.get("required_byte_transport").cloned().unwrap_or_else(|| serde_json::json!("adapter_ipc")),
                "direct_network": false,
                "wallet_injection": false,
                "reason": if data.get("status").and_then(|entry| entry.as_str()) == Some("configured") {
                    serde_json::json!("Browser Engine Adapter contract is configured; page rendering still requires attached stream byte transport.")
                } else {
                    serde_json::json!("Browser Engine Adapter provider is installed but no engine adapter is configured.")
                }
            })
        }
        _ => unavailable(),
    }
}

fn browser_visible_engine_adapters(data: &serde_json::Value) -> serde_json::Value {
    let Some(adapters) = data.get("adapters").and_then(|value| value.as_array()) else {
        return serde_json::json!([]);
    };
    serde_json::Value::Array(
        adapters
            .iter()
            .filter_map(|adapter| {
                let id = adapter.get("id").and_then(|value| value.as_str())?;
                if !is_safe_runtime_id(id) {
                    return None;
                }
                Some(serde_json::json!({
                    "id": id,
                    "engine": adapter.get("engine").and_then(|value| value.as_str()).unwrap_or("browser_engine"),
                    "default": adapter.get("default").and_then(|value| value.as_bool()).unwrap_or(false),
                    "backing_substrate": visible_runtime_string(adapter.get("backing_substrate")).unwrap_or("configured_provider"),
                    "supported_display_modes": visible_string_array(adapter.get("supported_display_modes")),
                    "supported_guarantee_levels": visible_string_array(adapter.get("supported_guarantee_levels")),
                    "network_mode": "runtime_net_only",
                    "direct_network": false,
                    "wallet_injection": false,
                }))
            })
            .collect(),
    )
}

fn visible_runtime_string(value: Option<&serde_json::Value>) -> Option<&str> {
    value
        .and_then(|value| value.as_str())
        .filter(|value| is_safe_runtime_id(value))
}

fn visible_string_array(value: Option<&serde_json::Value>) -> serde_json::Value {
    serde_json::Value::Array(
        value
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str())
                    .filter(|value| is_safe_runtime_id(value))
                    .map(|value| serde_json::Value::String(value.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
    )
}

pub(in crate::api::gateway) async fn browser_net_summary(
    registry: Option<&Arc<ProviderRegistry>>,
    principal_id: &str,
) -> serde_json::Value {
    let exit_provider = browser_exit_summary(registry, principal_id).await;
    let Some(registry) = registry else {
        return serde_json::json!({
            "status": "fail_closed",
            "provider": "elastos://net/*",
            "operations": ["resolve", "connect", "stream", "http"],
            "direct_network": false,
            "exit_provider": exit_provider,
            "reason": "No Runtime Net provider is configured for this browser capsule."
        });
    };
    match registry
        .send_raw(
            "net",
            &serde_json::json!({
                "op": "status",
                "principal_id": principal_id,
            }),
        )
        .await
    {
        Ok(value) if value.get("status").and_then(|entry| entry.as_str()) == Some("ok") => {
            let data = provider_response_data(&value).unwrap_or(value);
            if authority_false_proof_missing(&data, "direct_network") {
                return serde_json::json!({
                    "status": "invalid_provider_status",
                    "provider": "elastos://net/*",
                    "operations": ["resolve", "connect", "stream", "http"],
                    "direct_network": false,
                    "exit_provider": exit_provider,
                    "reason": "Runtime Net provider status omitted direct_network=false proof."
                });
            }
            serde_json::json!({
                "status": data.get("status").cloned().unwrap_or_else(|| serde_json::json!("fail_closed")),
                "provider": "elastos://net/*",
                "operations": data.get("operations").cloned().unwrap_or_else(|| serde_json::json!(["resolve", "connect", "stream", "http"])),
                "direct_network": false,
                "exit_count": data.get("exit_count").cloned().unwrap_or_else(|| serde_json::json!(0)),
                "exit_provider": exit_provider,
                "reason": "Runtime Net provider is installed; egress remains unavailable until an Exit Provider backend is configured."
            })
        }
        Ok(value) => serde_json::json!({
            "status": "fail_closed",
            "provider": "elastos://net/*",
            "operations": ["resolve", "connect", "stream", "http"],
            "direct_network": false,
            "exit_provider": exit_provider,
            "reason": value.get("message").and_then(|entry| entry.as_str()).unwrap_or("Runtime Net provider returned an error.")
        }),
        Err(err) => serde_json::json!({
            "status": "fail_closed",
            "provider": "elastos://net/*",
            "operations": ["resolve", "connect", "stream", "http"],
            "direct_network": false,
            "exit_provider": exit_provider,
            "reason": format!("Runtime Net provider unavailable: {err}")
        }),
    }
}

async fn browser_exit_summary(
    registry: Option<&Arc<ProviderRegistry>>,
    principal_id: &str,
) -> serde_json::Value {
    let Some(registry) = registry else {
        return serde_json::Value::Null;
    };
    match registry
        .send_raw(
            "exit",
            &serde_json::json!({
                "op": "status",
                "principal_id": principal_id,
            }),
        )
        .await
    {
        Ok(value) if value.get("status").and_then(|entry| entry.as_str()) == Some("ok") => {
            let data = provider_response_data(&value).unwrap_or(value);
            if authority_false_proof_missing(&data, "direct_network") {
                return serde_json::json!({
                    "status": "invalid_provider_status",
                    "provider": "elastos://exit/*",
                    "operations": ["quote", "open_stream", "close_stream", "http_fetch"],
                    "direct_network": false,
                    "backend_count": 0,
                    "reason": "Browser Exit provider status omitted direct_network=false proof."
                });
            }
            serde_json::json!({
                "status": data.get("status").cloned().unwrap_or_else(|| serde_json::json!("fail_closed")),
                "provider": "elastos://exit/*",
                "operations": data.get("operations").cloned().unwrap_or_else(|| serde_json::json!(["quote", "open_stream", "close_stream", "http_fetch"])),
                "direct_network": false,
                "backend_count": data.get("backend_count").cloned().unwrap_or_else(|| serde_json::json!(0)),
                "remote_carrier_exit_count": data.get("remote_carrier_exit_count").cloned().unwrap_or_else(|| serde_json::json!(0)),
                "remote_carrier_exits": browser_visible_remote_carrier_exits(&data),
            })
        }
        _ => serde_json::Value::Null,
    }
}

fn browser_visible_remote_carrier_exits(data: &serde_json::Value) -> serde_json::Value {
    let Some(exits) = data
        .get("remote_carrier_exits")
        .and_then(|value| value.as_array())
    else {
        return serde_json::json!([]);
    };
    serde_json::Value::Array(
        exits
            .iter()
            .map(|exit| {
                let mut visible = exit.clone();
                scrub_exit_authority_fields(&mut visible);
                visible
            })
            .collect(),
    )
}

fn scrub_exit_authority_fields(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            for key in [
                "allowed_principals",
                "adapter_ipc",
                "relay_ipc",
                "raw_socket",
                "connect_ticket",
                "peer_did",
            ] {
                object.remove(key);
            }
            for value in object.values_mut() {
                scrub_exit_authority_fields(value);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                scrub_exit_authority_fields(value);
            }
        }
        _ => {}
    }
}

fn authority_false_proof_missing(data: &serde_json::Value, field: &str) -> bool {
    data.get(field).and_then(|value| value.as_bool()) != Some(false)
}

fn invalid_provider_summary(provider: &str, reason: &str) -> serde_json::Value {
    serde_json::json!({
        "status": "invalid_provider_status",
        "provider": provider,
        "mode": "invalid_provider_status",
        "stream_session_schema": "elastos.exit.stream-session/v1",
        "display_session_schema": "elastos.browser.display-session/v1",
        "supported_display_modes": [],
        "supported_guarantee_levels": [],
        "byte_transport": "not_attached",
        "direct_network": false,
        "wallet_injection": false,
        "reason": reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_runtime::provider::{Provider, ProviderError, ResourceRequest, ResourceResponse};
    use serde_json::json;
    use std::sync::Mutex;

    struct ImageEngineFixture {
        name: &'static str,
        adapters: Vec<serde_json::Value>,
        calls: Arc<Mutex<Vec<String>>>,
        probe_delay: std::time::Duration,
    }

    #[async_trait::async_trait]
    impl Provider for ImageEngineFixture {
        async fn handle(&self, _: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
            Err(ProviderError::Provider("raw Engine fixture only".into()))
        }
        fn schemes(&self) -> Vec<&'static str> {
            vec![]
        }
        fn name(&self) -> &'static str {
            self.name
        }
        async fn send_raw(
            &self,
            request: &serde_json::Value,
        ) -> Result<serde_json::Value, ProviderError> {
            let op = request["op"].as_str().unwrap();
            self.calls.lock().unwrap().push(op.into());
            match op {
                "status" => Ok(json!({"status":"ok", "data":{
                    "provider":"browser-engine-adapter", "protocol_version":BROWSER_ENGINE_PROTOCOL_VERSION,
                    "status":"configured", "adapter_count":self.adapters.len(), "adapters":self.adapters,
                    "direct_network":false, "wallet_injection":false}})),
                "readiness" => {
                    tokio::time::sleep(self.probe_delay).await;
                    Ok(
                        json!({"status":"ok", "data":{"schema":BROWSER_ENGINE_READINESS_SCHEMA,
                        "adapter_id":request["adapter_id"], "readiness":{"state":"ready"}}}),
                    )
                }
                _ => panic!("unexpected Engine effect: {op}"),
            }
        }
    }

    fn image_adapter(id: &str, substrate: &str, default: bool) -> serde_json::Value {
        json!({"id":id, "engine":"chromium_microvm", "default":default,
            "backing_substrate":substrate, "supported_display_modes":["webrtc_remote_display"],
            "supported_guarantee_levels":["mechanism_microvm"], "network_mode":"runtime_net_only",
            "direct_network":false, "wallet_injection":false})
    }

    fn image_config(data: &FsPath, platform: &str) -> serde_json::Value {
        let launcher = if platform.starts_with("darwin-") {
            "browser-vz-engine-supervisor"
        } else {
            "browser-vm-local-crosvm-launcher"
        };
        json!({"adapters":[{"id":"local", "kind":"chromium_microvm", "supervisor":{
            "program":data.join("bin/browser-vm-engine-supervisor"), "env":{
                "ELASTOS_BROWSER_VM_CONTROL_LAUNCHER":data.join("bin").join(launcher),
                "ELASTOS_BROWSER_VM_DATA_DIR":data}}}]})
    }

    fn write_image_config(data: &FsPath) {
        std::fs::create_dir_all(data.join("config")).unwrap();
        std::fs::write(
            data.join("config/browser-engine-adapter.json"),
            serde_json::to_vec(&image_config(data, &crate::setup::detect_platform())).unwrap(),
        )
        .unwrap();
    }

    async fn image_registry(
        name: &'static str,
        adapters: Vec<serde_json::Value>,
        probe_delay: std::time::Duration,
    ) -> (ProviderRegistry, Arc<Mutex<Vec<String>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let registry = ProviderRegistry::new();
        registry
            .register_sub_provider(
                "browser-engine",
                Arc::new(ImageEngineFixture {
                    name,
                    adapters,
                    calls: calls.clone(),
                    probe_delay,
                }),
            )
            .await
            .unwrap();
        (registry, calls)
    }

    #[test]
    fn image_acquisition_requires_standard_local_host_paths() {
        let data = FsPath::new("/runtime-test");
        for platform in ["darwin-arm64", "linux-arm64", "linux-amd64"] {
            let config = image_config(data, platform);
            assert!(browser_engine_config_uses_runtime_image(
                &config, data, "local", platform
            ));
            for (pointer, value) in [
                ("/adapters/0/kind", "hosted_remote_browser"),
                ("/adapters/0/supervisor/program", "/custom/supervisor"),
                (
                    "/adapters/0/supervisor/env/ELASTOS_BROWSER_VM_CONTROL_LAUNCHER",
                    "/runtime-test/bin/browser-vm-remote-vz-launcher",
                ),
                (
                    "/adapters/0/supervisor/env/ELASTOS_BROWSER_VM_DATA_DIR",
                    "/other-runtime",
                ),
            ] {
                let mut other = config.clone();
                *other.pointer_mut(pointer).unwrap() = json!(value);
                assert!(
                    !browser_engine_config_uses_runtime_image(&other, data, "local", platform),
                    "{pointer}"
                );
            }
            let mut manual = config.clone();
            manual["adapters"][0]["supervisor"]["env"]["ELASTOS_BROWSER_VM_ROOTFS"] =
                json!("/manual/verified.ext4");
            assert!(!browser_engine_config_uses_runtime_image(
                &manual, data, "local", platform
            ));
            manual["adapters"][0]["supervisor"]["env"]["ELASTOS_BROWSER_VM_ROOTFS"] =
                json!(data.join("browser-vm/rootfs.ext4"));
            assert!(browser_engine_config_uses_runtime_image(
                &manual, data, "local", platform
            ));
            let (key, path) = if platform.starts_with("darwin-") {
                ("ELASTOS_BROWSER_VM_INITRAMFS", "bin/initrd")
            } else {
                ("ELASTOS_BROWSER_VM_INITRD", "browser-vm/initrd")
            };
            manual["adapters"][0]["supervisor"]["env"][key] = json!(data.join(path));
            assert!(browser_engine_config_uses_runtime_image(
                &manual, data, "local", platform
            ));
            manual["adapters"][0]["supervisor"]["env"][key] = json!(data.join("bin/initrd.img"));
            assert!(!browser_engine_config_uses_runtime_image(
                &manual, data, "local", platform
            ));
            assert!(!browser_engine_config_uses_runtime_image(
                &config, data, "absent", platform
            ));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn image_acquisition_finishes_before_local_readiness_budget_starts() {
        let dir = tempfile::tempdir().unwrap();
        write_image_config(dir.path());
        let (registry, calls) = image_registry(
            "capsule-provider",
            vec![image_adapter("local", "local_microvm", true)],
            std::time::Duration::from_secs(11),
        )
        .await;
        let started = tokio::time::Instant::now();
        let resolved = resolve_browser_engine_adapter_with_preparation(
            &registry,
            dir.path(),
            "person:test",
            None,
            BrowserDisplayMode::WebrtcRemoteDisplay,
            BrowserGuaranteeLevel::MechanismMicrovm,
            || async {
                calls.lock().unwrap().push("prepare".into());
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                calls.lock().unwrap().push("verified".into());
                Ok(())
            },
        )
        .await
        .unwrap();
        assert_eq!(resolved, "local");
        assert!(started.elapsed() >= std::time::Duration::from_secs(41));
        assert_eq!(
            *calls.lock().unwrap(),
            ["status", "prepare", "verified", "readiness"]
        );
    }

    #[tokio::test]
    async fn image_acquisition_is_lazy_for_remote_selection_and_custom_providers() {
        let dir = tempfile::tempdir().unwrap();
        write_image_config(dir.path());
        for (name, adapters, requested, expected) in [
            (
                "capsule-provider",
                vec![
                    image_adapter("remote", "remote_operator_vm", true),
                    image_adapter("local", "local_microvm", false),
                ],
                None,
                "remote",
            ),
            (
                "capsule-provider",
                vec![
                    image_adapter("local", "local_microvm", true),
                    image_adapter("remote", "remote_operator_vm", false),
                ],
                Some("remote"),
                "remote",
            ),
            (
                "custom-provider",
                vec![image_adapter("local", "local_microvm", true)],
                None,
                "local",
            ),
        ] {
            let (registry, calls) = image_registry(name, adapters, std::time::Duration::ZERO).await;
            let resolved = resolve_browser_engine_adapter_with_preparation(
                &registry,
                dir.path(),
                "person:test",
                requested,
                BrowserDisplayMode::WebrtcRemoteDisplay,
                BrowserGuaranteeLevel::MechanismMicrovm,
                || async { panic!("unselected/local-independent Engine acquired an image") },
            )
            .await
            .unwrap();
            assert_eq!(resolved, expected);
            assert_eq!(*calls.lock().unwrap(), ["status", "readiness"]);
            assert!(!dir.path().join("browser-vm").exists());
        }
    }

    #[tokio::test]
    async fn image_acquisition_missing_release_stops_selected_local_before_probe() {
        let dir = tempfile::tempdir().unwrap();
        write_image_config(dir.path());
        let (registry, calls) = image_registry(
            "capsule-provider",
            vec![image_adapter("local", "local_microvm", true)],
            std::time::Duration::ZERO,
        )
        .await;
        let error = resolve_browser_engine_adapter(
            &registry,
            dir.path(),
            "person:test",
            Some("local"),
            BrowserDisplayMode::WebrtcRemoteDisplay,
            BrowserGuaranteeLevel::MechanismMicrovm,
        )
        .await
        .unwrap_err();
        assert_eq!(
            error,
            BrowserCompatibilityError::EngineNotReady {
                reason: BrowserEngineReadinessReason::PreparationRequired
            }
        );
        assert_eq!(*calls.lock().unwrap(), ["status"]);
        assert!(!dir.path().join("browser-vm").exists());
    }

    #[tokio::test]
    async fn image_acquisition_failure_allows_only_an_unrequested_ready_alternative() {
        let dir = tempfile::tempdir().unwrap();
        write_image_config(dir.path());
        for (requested, expected) in [
            (None, Ok("remote".into())),
            (
                Some("local"),
                Err(BrowserCompatibilityError::EngineNotReady {
                    reason: BrowserEngineReadinessReason::PreparationRequired,
                }),
            ),
        ] {
            let (registry, calls) = image_registry(
                "capsule-provider",
                vec![
                    image_adapter("local", "local_microvm", true),
                    image_adapter("remote", "remote_operator_vm", false),
                ],
                std::time::Duration::ZERO,
            )
            .await;
            let result = resolve_browser_engine_adapter_with_preparation(
                &registry,
                dir.path(),
                "person:test",
                requested,
                BrowserDisplayMode::WebrtcRemoteDisplay,
                BrowserGuaranteeLevel::MechanismMicrovm,
                || async {
                    calls.lock().unwrap().push("prepare".into());
                    anyhow::bail!("missing published image bundle")
                },
            )
            .await;
            assert_eq!(result, expected);
            let expected_calls = if requested.is_some() {
                vec!["status", "prepare"]
            } else {
                vec!["status", "prepare", "readiness"]
            };
            assert_eq!(*calls.lock().unwrap(), expected_calls);
        }
    }

    #[test]
    fn browser_visible_remote_carrier_exits_redacts_transport_identity_and_authority() {
        let visible = browser_visible_remote_carrier_exits(&json!({
            "remote_carrier_exits": [{
                "id": "shared-exit",
                "grant_id": "grant:test",
                "peer_did": "did:key:z6Mktest",
                "connect_ticket": "ticket:secret",
                "allowed_principals": ["person:local:test"],
                "carrier": {
                    "peer_did": "did:key:z6Mknested",
                    "connect_ticket": "ticket:nested"
                }
            }]
        }));
        let exits = visible
            .as_array()
            .expect("remote exits should stay an array");
        assert_eq!(exits.len(), 1);
        let exit = exits[0].as_object().expect("exit should stay an object");
        assert!(!exit.contains_key("peer_did"));
        assert!(!exit.contains_key("connect_ticket"));
        assert!(!exit.contains_key("allowed_principals"));
        assert!(exit
            .get("carrier")
            .and_then(|value| value.as_object())
            .is_some_and(|carrier| {
                !carrier.contains_key("peer_did") && !carrier.contains_key("connect_ticket")
            }));
    }
}

/// Approved remote services are observed through the same Runtime provider plane.
/// Execution selections retain an opaque Runtime route to their exact grant.
pub(in crate::api::gateway) async fn browser_remote_engine_summary(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
) -> serde_json::Value {
    let read_grants = || {
        home_services_remote_engine_grants(
            &state.data_dir,
            context,
            state.collaboration_discovery_service.as_ref(),
        )
    };
    let Ok(grants) = read_grants() else {
        return serde_json::json!({"state":"unavailable","offers":[]});
    };
    let Some(registry) = state.provider_registry.as_ref() else {
        return serde_json::json!({"state":"unavailable","offers":[]});
    };
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut offers = Vec::new();
    for grant in grants {
        let mut visible = serde_json::json!({"id":grant["id"],"state":"unavailable",
            "launch_available":false,"launch_reason":"remote_runtime_binding_required"});
        if grant["expires_at"]
            .as_u64()
            .is_none_or(|expiry| expiry <= crate::auth::now_ts())
        {
            visible["state"] = serde_json::json!("expired");
            offers.push(visible);
            continue;
        }
        let observation = async {
            let status =
                crate::carrier::probe_browser_engine(registry, &grant, "status", None).await?;
            let data = provider_response_data(&status)
                .ok_or_else(|| anyhow::anyhow!("Engine status missing"))?;
            let adapters = data["adapters"]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("Engine inventory missing"))?;
            let adapter = adapters
                .iter()
                .find(|adapter| adapter["default"] == true)
                .or_else(|| adapters.first())
                .and_then(|adapter| adapter["id"].as_str())
                .ok_or_else(|| anyhow::anyhow!("Engine adapter missing"))?;
            let ready =
                crate::carrier::probe_browser_engine(registry, &grant, "readiness", Some(adapter))
                    .await?;
            let ready = provider_response_data(&ready)
                .ok_or_else(|| anyhow::anyhow!("Engine readiness missing"))?;
            Ok::<_, anyhow::Error>((data, ready))
        };
        if let Ok(Ok((data, readiness))) = tokio::time::timeout_at(deadline, observation).await {
            if read_grants().is_ok_and(|current| current.contains(&grant)) {
                visible["state"] = serde_json::json!("approved");
                if data["remote_page_binding_supported"] == true
                    && data["launch_available"] == true
                    && grant["operations"]
                        == crate::carrier::browser_engine_binding::operations(Some(
                            crate::carrier::browser_engine_binding::EXECUTION_SCOPE,
                        ))
                {
                    visible["launch_available"] = serde_json::json!(true);
                    visible["launch_reason"] = serde_json::Value::Null;
                    let mut selectable = data["adapters"].clone();
                    if let Some(adapters) = selectable.as_array_mut() {
                        for adapter in adapters {
                            if let Some(id) = adapter["id"].as_str() {
                                adapter["id"] = serde_json::json!(
                                    gateway_browser_remote::selection_id(&grant, id)
                                );
                                adapter["default"] = serde_json::json!(false);
                            }
                        }
                    }
                    visible["selectable_adapters"] = selectable;
                }
                visible["adapters"] = data["adapters"].clone();
                visible["capacity_available"] = data["capacity_available"].clone();
                visible["readiness"] = readiness["readiness"].clone();
            }
        }
        offers.push(visible);
    }
    serde_json::json!({"state":"available","offers":offers})
}
