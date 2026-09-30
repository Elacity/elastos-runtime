//! Read-only Engine admission behind the existing authenticated provider plane.
//! Launch remains unavailable until Runtime can bind remote page resources.

use anyhow::{Context, Result};
use elastos_runtime::provider::{
    ProviderCarrierRoute, ProviderInvocation, ProviderInvocationTransport, ProviderRegistry,
    ProviderTransfer,
};
use serde_json::{json, Value};
use std::{path::Path, sync::Arc, time::Duration};

pub(crate) const ENGINE_GRANT_TTL_SECS: u64 = 3600;
pub(crate) const ENGINE_SERVICE_URI: &str = "elastos://peer/browser-engine";
pub(crate) const ENGINE_SERVICE_KIND: &str = "browser_engine";
pub(crate) const ENGINE_LOCAL_OFFER: &str = "local:provider:browser-engine";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BrowserEngineGrant {
    pub provider_principal_id: String,
    pub requester_principal_id: String,
    pub requester_endpoint: iroh::PublicKey,
    pub grant_id: String,
    pub revision: u64,
    pub expires_at: u64,
    pub execution_allowed: bool,
}

impl BrowserEngineGrant {
    pub fn validate(&self, source: &iroh::PublicKey, request: &Value, now: u64) -> Result<()> {
        anyhow::ensure!(
            self.requester_endpoint == *source
                && request["principal_id"].as_str() == Some(&self.requester_principal_id)
                && request["grant_id"].as_str() == Some(&self.grant_id),
            "Engine grant requester mismatch"
        );
        anyhow::ensure!(
            self.revision > 0
                && self.revision <= now.saturating_add(30)
                && self.expires_at > now
                && self.expires_at > self.revision
                && self.expires_at - self.revision <= ENGINE_GRANT_TTL_SECS,
            "Engine grant expired or invalid"
        );
        anyhow::ensure!(
            request["op"]
                .as_str()
                .is_some_and(|op| super::browser_engine_binding::allows(
                    self.execution_allowed
                        .then_some(super::browser_engine_binding::EXECUTION_SCOPE),
                    op
                )),
            "Engine operation requires remote Runtime resource binding"
        );
        Ok(())
    }
}

pub(crate) fn grant_id(request_id: &str) -> String {
    use sha2::{Digest, Sha256};
    format!(
        "services-remote-engine-grant-{}",
        hex::encode(&Sha256::digest(request_id.as_bytes())[..8])
    )
}

fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':'))
}

/// The serving Runtime supplies only bounded contract facts, never host routes.
fn project_response(operation: &str, request: &Value, response: Value) -> Result<Value> {
    anyhow::ensure!(response["status"] == "ok", "Engine unavailable");
    let data = &response["data"];
    if operation == "readiness" {
        anyhow::ensure!(
            data["schema"] == "elastos.browser.engine-readiness/v1"
                && data["adapter_id"] == request["adapter_id"],
            "Engine readiness binding mismatch"
        );
        let readiness: elastos_common::browser_protocol::BrowserEngineReadiness =
            serde_json::from_value(data["readiness"].clone())?;
        return Ok(
            json!({"status":"ok","data":{"schema":"elastos.browser.engine-readiness/v1",
            "adapter_id":request["adapter_id"],"readiness":readiness}}),
        );
    }
    anyhow::ensure!(
        data["direct_network"] == false && data["wallet_injection"] == false,
        "Engine authority proof missing"
    );
    let inventory = elastos_common::browser_protocol::BrowserEngineInventory::from_status(data)?;
    anyhow::ensure!(inventory.adapters.len() <= 16, "Engine inventory too large");
    let mut visible = Vec::new();
    for adapter in inventory.adapters {
        anyhow::ensure!(
            safe_id(&adapter.id)
                && safe_id(&adapter.engine)
                && safe_id(&adapter.backing_substrate)
                && adapter.network_mode == "runtime_net_only"
                && !adapter.direct_network
                && !adapter.wallet_injection,
            "Engine adapter contract invalid"
        );
        visible.push(adapter);
    }
    let capacity = data["capacity_available"]
        .as_bool()
        .context("Engine capacity missing")?;
    let maximum = data["max_active_sessions"]
        .as_u64()
        .filter(|count| *count <= 1024)
        .context("Engine capacity invalid")?;
    Ok(
        json!({"status":"ok","data":{"schema":"elastos.browser.remote-engine-status/v1",
        "provider":inventory.provider,"status":inventory.status,"adapter_count":visible.len(),
        "protocol_version":inventory.protocol_version,
        "adapters":visible,"capacity_available":capacity,"max_active_sessions":maximum,
        "direct_network":false,"wallet_injection":false,"launch_available":false}}),
    )
}

async fn read_authority(
    data_dir: &Path,
    network: &crate::collaboration_network::VerifiedCollaborationNetworkProfile,
    source: iroh::PublicKey,
    request: &Value,
) -> Result<BrowserEngineGrant> {
    let data_dir = data_dir.to_path_buf();
    let network = network.clone();
    let request = request.clone();
    tokio::time::timeout(
        Duration::from_secs(1),
        tokio::task::spawn_blocking(move || {
            crate::api::gateway::authorize_home_service_engine(
                &data_dir,
                &network,
                &source,
                &request,
                crate::auth::now_ts(),
            )
        }),
    )
    .await
    .context("Engine authority deadline")??
}

pub(super) async fn invoke(
    registry: Arc<ProviderRegistry>,
    data_dir: &Path,
    network: Option<crate::collaboration_network::VerifiedCollaborationNetworkProfile>,
    slots: Arc<tokio::sync::Semaphore>,
    endpoint: iroh::Endpoint,
    source: iroh::PublicKey,
    data: &Value,
) -> Value {
    let operation = data["operation"].as_str().unwrap_or_default();
    if !matches!(operation, "status" | "readiness") || data["request"].get("page_id").is_some() {
        let mut stage = "execution_envelope";
        let outcome = async {
            anyhow::ensure!(
                serde_json::to_vec(data)?.len() <= 256 * 1024
                    && data["source"] == "browser"
                    && data["target"] == "browser-engine"
                    && data["transfer"] == "json"
                    && super::browser_engine_binding::EXECUTION_OPERATIONS.contains(&operation),
                "Engine execution envelope invalid"
            );
            stage = "invocation_contract";
            super::validate_carrier_provider_invocation(
                "browser",
                "browser-engine",
                operation,
                "json",
                &data["request"],
            )
            .map_err(anyhow::Error::msg)?;
            if operation == "close_page" && data["request"]["cancel_preparation"] == true {
                stage = "cancellation_authority";
                let root = data_dir.to_owned();
                let request = data["request"].clone();
                tokio::time::timeout(
                    Duration::from_secs(1),
                    tokio::task::spawn_blocking(move || {
                        crate::api::gateway::authorize_home_engine_preparation_cancellation(
                            &root, &source, &request,
                        )
                    }),
                )
                .await
                .context("Engine cancellation authority deadline")???;
            }
            stage = "grant_authority";
            let grant = if matches!(operation, "close_page" | "status") {
                None
            } else {
                Some(
                    read_authority(
                        data_dir,
                        network
                            .as_ref()
                            .context("Engine contact authority missing")?,
                        source,
                        &data["request"],
                    )
                    .await?,
                )
            };
            stage = "runtime_operation";
            let response = crate::api::gateway::invoke_remote_browser_engine(
                data_dir,
                registry.clone(),
                endpoint,
                source,
                network
                    .clone()
                    .context("Engine Runtime network unavailable")?,
                grant.clone(),
                &data["request"],
            )
            .await?;
            if let Some(grant) = grant {
                stage = "post_operation_authority";
                anyhow::ensure!(
                    read_authority(
                        data_dir,
                        network.as_ref().unwrap(),
                        source,
                        &data["request"]
                    )
                    .await?
                        == grant,
                    "Engine authority changed during operation"
                );
            }
            Ok::<_, anyhow::Error>(response)
        };
        let outcome = tokio::time::timeout(
            Duration::from_secs(if matches!(operation, "prepare_launch" | "launch") {
                55
            } else {
                5
            }),
            outcome,
        )
        .await;
        return match outcome {
            Ok(Ok(result)) => json!({"ok":true,"result":result}),
            Ok(Err(_)) => execution_failure(operation, stage, "rejected"),
            Err(_) => execution_failure(operation, stage, "deadline"),
        };
    }
    let outcome = async {
        let _slot = slots
            .try_acquire_owned()
            .context("Engine probe capacity unavailable")?;
        anyhow::ensure!(
            serde_json::to_vec(data)?.len() <= 4096,
            "Engine request too large"
        );
        anyhow::ensure!(
            data["source"] == "browser"
                && data["target"] == "browser-engine"
                && data["transfer"] == "json",
            "Engine invocation scope mismatch"
        );
        let operation = data["operation"]
            .as_str()
            .context("Engine operation missing")?;
        anyhow::ensure!(
            matches!(operation, "status" | "readiness"),
            "Engine operation unsupported"
        );
        super::validate_carrier_provider_invocation(
            "browser",
            "browser-engine",
            operation,
            "json",
            &data["request"],
        )
        .map_err(anyhow::Error::msg)?;
        let request = data["request"]
            .as_object()
            .context("Engine request missing")?;
        anyhow::ensure!(
            request.keys().all(|key| matches!(
                key.as_str(),
                "op" | "grant_id" | "principal_id" | "adapter_id" | "_runtime_invocation"
            )),
            "Engine request has unsupported fields"
        );
        if operation == "readiness" {
            anyhow::ensure!(
                request
                    .get("adapter_id")
                    .and_then(Value::as_str)
                    .is_some_and(safe_id),
                "Engine adapter invalid"
            );
        }
        let network = network.context("Engine contact authority unavailable")?;
        let grant = read_authority(data_dir, &network, source, &data["request"]).await?;
        let mut provider_request =
            json!({"op":operation,"principal_id":grant.provider_principal_id});
        if operation == "readiness" {
            provider_request["adapter_id"] = data["request"]["adapter_id"].clone();
        }
        let response = registry
            .send_raw("browser-engine", &provider_request)
            .await?;
        // Recheck current authority after the provider await before publishing facts.
        anyhow::ensure!(
            read_authority(data_dir, &network, source, &data["request"]).await? == grant,
            "Engine grant changed during observation"
        );
        let mut projected = project_response(operation, &provider_request, response)?;
        if operation == "status" {
            projected["data"]["remote_page_binding_supported"] = json!(true);
            projected["data"]["launch_available"] = json!(grant.execution_allowed);
        }
        Ok(projected)
    };
    match tokio::time::timeout(Duration::from_secs(4), outcome).await {
        Ok(Ok(result)) => json!({"ok":true,"result":result}),
        _ => json!({"ok":false,"code":"browser_engine_unavailable",
            "error":"Runtime Engine grant or readiness is unavailable"}),
    }
}

fn execution_failure(operation: &str, stage: &str, outcome: &str) -> Value {
    // Fixed contract labels identify the boundary without logging provider
    // errors, request fields, tickets or native host paths.
    let operation = super::browser_engine_binding::EXECUTION_OPERATIONS
        .iter()
        .copied()
        .find(|supported| *supported == operation)
        .unwrap_or("unsupported");
    let stage = match stage {
        "execution_envelope"
        | "invocation_contract"
        | "cancellation_authority"
        | "grant_authority"
        | "runtime_operation"
        | "post_operation_authority" => stage,
        _ => "unknown",
    };
    let outcome = if outcome == "deadline" {
        "deadline"
    } else {
        "rejected"
    };
    tracing::warn!(operation, stage, outcome, "Remote Engine execution failed");
    json!({"ok":false,"code":"browser_engine_unavailable",
        "error":"Runtime Engine operation is unavailable or pending settlement",
        "operation":operation,"stage":stage,"outcome":outcome})
}

pub(crate) async fn probe(
    registry: &ProviderRegistry,
    grant: &Value,
    operation: &str,
    adapter_id: Option<&str>,
) -> Result<Value> {
    anyhow::ensure!(
        matches!(operation, "status" | "readiness"),
        "Engine launch resource binding unavailable"
    );
    let mut request = json!({"op":operation});
    if let Some(adapter) = adapter_id {
        request["adapter_id"] = json!(adapter);
    }
    call(registry, grant, operation, request).await
}

pub(crate) async fn call(
    registry: &ProviderRegistry,
    grant: &Value,
    operation: &str,
    mut request: Value,
) -> Result<Value> {
    let operation = super::browser_engine_binding::EXECUTION_OPERATIONS
        .iter()
        .copied()
        .find(|supported| *supported == operation)
        .context("Engine operation unsupported")?;
    anyhow::ensure!(
        grant["operations"]
            .as_array()
            .is_some_and(|ops| ops.iter().any(|op| op == operation)),
        "Engine operation was not approved"
    );
    let endpoint = grant["peer_did"]
        .as_str()
        .context("Engine peer required")?
        .parse::<iroh::PublicKey>()?;
    let ticket = grant["connect_ticket"]
        .as_str()
        .context("Engine ticket required")?;
    request["op"] = json!(operation);
    request["grant_id"] = grant["grant_id"].clone();
    request["principal_id"] = grant["principal_id"].clone();
    let result = registry
        .invoke_provider(ProviderInvocation {
            source: "browser".into(),
            target: "browser-engine".into(),
            op: operation.into(),
            request,
            transfer: ProviderTransfer::Json,
            range: None,
            progress: None,
            transport: ProviderInvocationTransport::Carrier(ProviderCarrierRoute::ConnectTicket {
                connect_ticket: ticket.into(),
                peer_did: Some(super::public_key_to_did(&endpoint)?),
                timeout_ms: Some(if matches!(operation, "prepare_launch" | "launch") {
                    60_000
                } else {
                    5_000
                }),
            }),
        })
        .await
        .map_err(|_| {
            // Provider errors can contain private tickets or backend details.
            // Preserve the fixed operation and stage at this public boundary.
            tracing::warn!(
                operation,
                stage = "carrier_provider_invocation",
                "Remote Engine call failed"
            );
            anyhow::anyhow!("Remote Engine {operation} failed at carrier_provider_invocation")
        })?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_execution_failure_keeps_only_fixed_diagnostic_labels() {
        let value = execution_failure("input", "runtime_operation", "deadline");
        assert_eq!(value["operation"], "input");
        assert_eq!(value["stage"], "runtime_operation");
        assert_eq!(value["outcome"], "deadline");
        assert_eq!(value["ok"], false);
        let value = execution_failure("ticket:private", "/private/host", "secret");
        assert_eq!(value["operation"], "unsupported");
        assert_eq!(value["stage"], "unknown");
        assert_eq!(value["outcome"], "rejected");
        assert!(!value.to_string().contains("private"));
        assert!(!value.to_string().contains("secret"));
    }

    struct EngineCallInvoker {
        fail: bool,
        calls: std::sync::Mutex<Vec<ProviderInvocation>>,
    }

    #[async_trait::async_trait]
    impl elastos_runtime::provider::ProviderCarrierInvoker for EngineCallInvoker {
        async fn invoke_carrier_provider(
            &self,
            _: &ProviderCarrierRoute,
            invocation: &ProviderInvocation,
            _: Value,
        ) -> std::result::Result<Value, elastos_runtime::provider::ProviderError> {
            self.calls.lock().unwrap().push(invocation.clone());
            if self.fail {
                Err(elastos_runtime::provider::ProviderError::Provider(
                    "Carrier provider invocation failed: ticket[0] connect failed; ticket:private-secret turn:private-credential".into(),
                ))
            } else {
                Ok(json!({"status":"ok","data":{"receipt":"unchanged"}}))
            }
        }
    }

    #[tokio::test]
    async fn engine_call_failure_identifies_operation_without_provider_secrets() {
        let registry = ProviderRegistry::new();
        let invoker = Arc::new(EngineCallInvoker {
            fail: true,
            calls: Default::default(),
        });
        registry.set_carrier_invoker(invoker.clone()).await;
        let grant = json!({"peer_did":iroh::SecretKey::from_bytes(&[19;32]).public().to_string(),
            "connect_ticket":"ticket:private-secret","grant_id":"grant:approved","principal_id":"consumer",
            "operations":super::super::browser_engine_binding::EXECUTION_OPERATIONS});
        for operation in super::super::browser_engine_binding::EXECUTION_OPERATIONS {
            let error = call(&registry, &grant, operation, json!({}))
                .await
                .unwrap_err();
            let expected =
                format!("Remote Engine {operation} failed at carrier_provider_invocation");
            assert_eq!(error.to_string(), expected);
            assert_eq!(format!("{error:#}"), expected);
        }
        let calls = invoker.calls.lock().unwrap();
        assert_eq!(
            calls.len(),
            super::super::browser_engine_binding::EXECUTION_OPERATIONS.len()
        );
        for (invocation, operation) in calls
            .iter()
            .zip(super::super::browser_engine_binding::EXECUTION_OPERATIONS)
        {
            assert_eq!(invocation.op, *operation);
            assert_eq!(invocation.source, "browser");
            assert_eq!(invocation.target, "browser-engine");
            assert_eq!(invocation.request["grant_id"], "grant:approved");
            assert_eq!(invocation.request["principal_id"], "consumer");
            let ProviderInvocationTransport::Carrier(route) = &invocation.transport else {
                panic!("Expected Carrier transport");
            };
            assert_eq!(
                route.timeout_ms(),
                Some(if matches!(*operation, "prepare_launch" | "launch") {
                    60_000
                } else {
                    5_000
                })
            );
        }
    }

    #[tokio::test]
    async fn engine_call_keeps_success_and_rejects_unapproved_or_unknown_operations() {
        let registry = ProviderRegistry::new();
        let invoker = Arc::new(EngineCallInvoker {
            fail: false,
            calls: Default::default(),
        });
        registry.set_carrier_invoker(invoker.clone()).await;
        let mut grant = json!({"peer_did":iroh::SecretKey::from_bytes(&[20;32]).public().to_string(),
            "connect_ticket":"ticket:private-secret","grant_id":"grant:approved","principal_id":"consumer",
            "operations":["readiness","private-unknown-operation"]});
        let result = call(&registry, &grant, "readiness", json!({}))
            .await
            .unwrap();
        assert_eq!(result["status"], "ok");
        assert_eq!(result["data"], json!({"receipt":"unchanged"}));
        let error = call(&registry, &grant, "private-unknown-operation", json!({}))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "Engine operation unsupported");
        grant["operations"] = json!([]);
        let error = call(&registry, &grant, "launch", json!({}))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "Engine operation was not approved");
        assert_eq!(invoker.calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn engine_grant_binds_endpoint_principal_revision_expiry_and_read_only_operations() {
        let source = iroh::SecretKey::from_bytes(&[12; 32]).public();
        let grant = BrowserEngineGrant {
            provider_principal_id: "provider".into(),
            requester_principal_id: "consumer".into(),
            requester_endpoint: source,
            grant_id: "grant".into(),
            revision: 100,
            expires_at: 3700,
            execution_allowed: false,
        };
        let request = json!({"op":"status","grant_id":"grant","principal_id":"consumer"});
        assert!(grant.validate(&source, &request, 100).is_ok());
        assert!(grant.validate(&source, &request, 3700).is_err());
        assert!(grant.validate(&source, &request, 69).is_err());
        for (field, value) in [
            ("principal_id", "provider"),
            ("grant_id", "other"),
            ("op", "launch"),
            ("op", "close_page"),
            ("op", "input"),
        ] {
            let mut request = request.clone();
            request[field] = json!(value);
            assert!(
                grant.validate(&source, &request, 100).is_err(),
                "{field}/{value}"
            );
        }
        assert!(grant
            .validate(
                &iroh::SecretKey::from_bytes(&[13; 32]).public(),
                &request,
                100
            )
            .is_err());
        let mut longer = grant.clone();
        longer.expires_at += 1;
        assert!(longer.validate(&source, &request, 100).is_err());
    }

    fn status() -> Value {
        json!({"status":"ok","data":{"provider":"browser-engine-adapter",
            "protocol_version":elastos_common::browser_protocol::BROWSER_ENGINE_PROTOCOL_VERSION,
            "status":"configured","adapter_count":1,"direct_network":false,"wallet_injection":false,
            "capacity_available":true,"max_active_sessions":4,"private_route":"/private/socket",
            "credential":"private-secret","adapters":[{"id":"engine","engine":"chromium_microvm","default":true,
            "backing_substrate":"local_microvm","supported_display_modes":["webrtc_remote_display"],
            "supported_guarantee_levels":["mechanism_microvm"],"network_mode":"runtime_net_only",
            "direct_network":false,"wallet_injection":false,"control_socket":"/private/control"}]}})
    }

    #[test]
    fn engine_response_preserves_contract_facts_and_omits_private_fields() {
        let public = project_response("status", &json!({}), status()).unwrap();
        assert_eq!(public["data"]["adapters"][0]["id"], "engine");
        assert_eq!(public["data"]["capacity_available"], true);
        assert!(!public.to_string().contains("private"));
        assert_eq!(public["data"]["launch_available"], false);
        elastos_common::browser_protocol::BrowserEngineInventory::from_status(&public["data"])
            .expect("remote status must remain consumable by the shared Engine selector");
        for pointer in [
            "/data/direct_network",
            "/data/wallet_injection",
            "/data/adapters/0/direct_network",
        ] {
            let mut invalid = status();
            *invalid.pointer_mut(pointer).unwrap() = json!(true);
            assert!(project_response("status", &json!({}), invalid).is_err());
        }
        let mut invalid = status();
        invalid["data"]["protocol_version"] = json!("incompatible");
        assert!(project_response("status", &json!({}), invalid).is_err());
        let mut invalid = status();
        invalid["data"]["adapters"][0]["id"] = json!("/host/path");
        assert!(project_response("status", &json!({}), invalid).is_err());
        let mut invalid = status();
        invalid["data"]["adapters"] = json!(vec![status()["data"]["adapters"][0].clone(); 17]);
        assert!(project_response("status", &json!({}), invalid).is_err());
    }

    #[test]
    fn engine_readiness_must_match_exact_adapter_and_typed_reason() {
        let request = json!({"adapter_id":"chosen"});
        let mut response = json!({"status":"ok","data":{"schema":"elastos.browser.engine-readiness/v1",
            "adapter_id":"other","readiness":{"state":"ready"}}});
        assert!(project_response("readiness", &request, response.clone()).is_err());
        response["data"]["adapter_id"] = json!("chosen");
        response["data"]["readiness"] =
            json!({"state":"unavailable","reason":"preparation_required"});
        assert_eq!(
            project_response("readiness", &request, response.clone()).unwrap()["data"]["readiness"]
                ["reason"],
            "preparation_required"
        );
        response["data"]["readiness"]["reason"] = json!("/private/failure");
        assert!(project_response("readiness", &request, response).is_err());
    }

    #[tokio::test]
    async fn engine_probe_quota_rejects_without_authority_or_provider_dispatch() {
        let slots = Arc::new(tokio::sync::Semaphore::new(1));
        let _held = slots.clone().acquire_owned().await.unwrap();
        let response = invoke(
            Arc::new(ProviderRegistry::new()),
            Path::new("unused"),
            None,
            slots.clone(),
            iroh::Endpoint::builder(iroh::endpoint::presets::N0)
                .relay_mode(iroh::RelayMode::Disabled)
                .bind()
                .await
                .unwrap(),
            iroh::SecretKey::from_bytes(&[15; 32]).public(),
            &json!({}),
        )
        .await;
        assert_eq!(response["ok"], false);
        assert!(!response.to_string().contains("unused"));
        assert_eq!(slots.available_permits(), 0);
        drop(_held);
        assert_eq!(slots.available_permits(), 1);
    }
}
